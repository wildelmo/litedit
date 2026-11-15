//! Tree actions on several entries, copy / paste / duplicate, and the file
//! details in the status bar (S7.5). A child module of `app` so these stay
//! out of app.rs but can use the app's internals.
//!
//! Rules shared by every action here:
//! - workspace folders themselves, entries that are not shown, and entries
//!   inside another chosen folder are never acted on ([`App::tree_targets`]);
//! - a delete or move of several entries is refused as a whole while any
//!   file in it is open with unsaved edits (checked when asking and again
//!   when the answer arrives);
//! - nothing is ever replaced; what could not be done is reported by name.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::{App, display_name};
use crate::fsops::{self, CopyJob, CopyLimits};
use crate::input::{Key, Mods};
use crate::menu::MenuAction;
use crate::nag::{Nag, NagAction};
use crate::view::Tick;

#[derive(Default)]
pub(super) struct TreeOps {
    /// Entries chosen with Copy. An in-app list: the Windows file clipboard
    /// is not used.
    copied: Vec<PathBuf>,
    /// A paste or duplicate in progress (done in slices from `tick`).
    job: Option<Running>,
    /// The progress text last drawn (redraw only when it changes).
    progress: String,
    /// The outcome of a batch that could not be shown yet because another
    /// question was up: (message, is an error).
    report: Option<(String, bool)>,
    /// Size and date of the active file, read once per file / save / change
    /// on disk (never per frame).
    details: Option<Details>,
    /// Tests: how often a file's details were read from disk.
    #[cfg(test)]
    stats: usize,
    /// Tests: smaller copy limits.
    #[cfg(test)]
    limits: Option<CopyLimits>,
    /// Tests: how long a copy works before the first frame.
    #[cfg(test)]
    first_slice: Option<Duration>,
}

struct Running {
    job: CopyJob,
    /// "Pasted" / "Duplicated".
    verb: &'static str,
    /// Paste: the folder pasted into.
    into: Option<PathBuf>,
    count: usize,
}

struct Details {
    path: PathBuf,
    /// (size, last change); None if the file cannot be read.
    info: Option<(u64, Option<SystemTime>)>,
}

/// How long a paste works right away, before the window draws again.
const FIRST_SLICE: Duration = Duration::from_millis(20);

/// "a.txt, b.md, docs and 2 more".
fn name_list(paths: &[PathBuf], max: usize) -> String {
    let mut s = paths
        .iter()
        .take(max)
        .map(|p| display_name(p))
        .collect::<Vec<_>>()
        .join(", ");
    if paths.len() > max {
        s.push_str(&format!(" and {} more", paths.len() - max));
    }
    s
}

/// "3 items" / the name of the only one.
fn count_or_name(paths: &[PathBuf]) -> String {
    match paths {
        [one] => display_name(one),
        _ => format!("{} items", paths.len()),
    }
}

/// The time shown as "now". Frozen in tests so two renders of one scene
/// always match.
fn now() -> SystemTime {
    #[cfg(test)]
    {
        static NOW: std::sync::OnceLock<SystemTime> = std::sync::OnceLock::new();
        *NOW.get_or_init(SystemTime::now)
    }
    #[cfg(not(test))]
    SystemTime::now()
}

/// "just now", "5 min ago", "2 h ago", "3 days ago", … (coarse on purpose;
/// relative, so no time zones are involved).
pub(super) fn ago(age: Duration) -> String {
    let s = age.as_secs();
    let (min, hour, day) = (60, 3600, 86_400);
    if s < min {
        "just now".into()
    } else if s < hour {
        format!("{} min ago", s / min)
    } else if s < 48 * hour {
        format!("{} h ago", s / hour)
    } else if s < 60 * day {
        format!("{} days ago", s / day)
    } else if s < 730 * day {
        format!("{} months ago", s / (30 * day))
    } else {
        format!("{} years ago", s / (365 * day))
    }
}

/// "12.4 KB · changed 2 h ago".
pub(super) fn details_text(size: u64, modified: Option<SystemTime>, now: SystemTime) -> String {
    let size = fsops::human_bytes(size);
    match modified {
        // A date in the future (clock changes) reads as "just now".
        Some(m) => format!(
            "{size} · changed {}",
            ago(now.duration_since(m).unwrap_or_default())
        ),
        None => size,
    }
}

/// One line for the outcome of a batch where something went wrong:
/// "Moved 2 of 4 into docs. Not moved: a.txt, b.md (reason); c (other reason)".
/// The line has to fit the question bar, so entries that failed for the
/// same reason share it, and only the first few are named (the app selects
/// all of them in the tree).
fn batch_report(
    done: &str,
    ok: usize,
    total: usize,
    tail: &str,
    not: &str,
    failed: &[(PathBuf, String)],
) -> String {
    const NAMED: usize = 4;
    let mut groups: Vec<(&str, Vec<String>)> = Vec::new();
    for (p, why) in failed.iter().take(NAMED) {
        match groups.iter_mut().find(|g| g.0 == why) {
            Some(g) => g.1.push(display_name(p)),
            None => groups.push((why, vec![display_name(p)])),
        }
    }
    let mut s = format!("{done} {ok} of {total}{tail}. {not}: ");
    s.push_str(
        &groups
            .iter()
            .map(|(why, names)| format!("{} ({why})", names.join(", ")))
            .collect::<Vec<_>>()
            .join("; "),
    );
    if failed.len() > NAMED {
        s.push_str(&format!(
            " and {} more (selected in the tree)",
            failed.len() - NAMED
        ));
    }
    s
}

impl App {
    // ---- which entries an action is about ----------------------------------

    /// From `paths`: no workspace folders, nothing twice, nothing that lies
    /// inside another chosen folder.
    fn plan(&self, paths: Vec<PathBuf>) -> Vec<PathBuf> {
        let roots = self.tree.roots();
        let paths: Vec<PathBuf> = paths.into_iter().filter(|p| !roots.contains(p)).collect();
        fsops::without_nested(&paths)
    }

    /// The tree's selection as the entries an action works on.
    fn tree_targets(&mut self) -> Vec<PathBuf> {
        let rows = self.tree.selection();
        self.plan(
            rows.into_iter()
                .filter(|r| r.depth > 0)
                .map(|r| r.entry.path)
                .collect(),
        )
    }

    /// Names of open files with unsaved edits at or under any of `paths`.
    fn unsaved_under(&self, paths: &[PathBuf]) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = Vec::new();
        for p in paths {
            for (_, file, dirty) in self.tabs_under(p) {
                if dirty && !out.contains(&file) {
                    out.push(file);
                }
            }
        }
        out
    }

    /// The refusal for a delete / move that would take away a file with
    /// unsaved edits.
    fn unsaved_refusal(&self, paths: &[PathBuf], done: &str) -> Option<String> {
        let unsaved = self.unsaved_under(paths);
        (!unsaved.is_empty()).then(|| {
            format!(
                "Nothing was {done}: {} {} unsaved edits. Save or close {} first.",
                name_list(&unsaved, 3),
                if unsaved.len() == 1 { "has" } else { "have" },
                if unsaved.len() == 1 { "it" } else { "them" },
            )
        })
    }

    /// Show the outcome of a batch: at once, or after the question that is
    /// showing now (never replacing it).
    fn report(&mut self, message: String, error: bool) {
        if self.nag.is_some() {
            self.ops.report = Some((message, error));
        } else {
            self.nag = Some(Nag::new(message, vec![("OK", NagAction::Dismiss)], error));
        }
        self.pending = true;
    }

    /// A copy is running: other changes to files wait until it is done.
    fn copy_busy(&mut self) -> bool {
        if self.ops.job.is_some() {
            self.notice = Some("A copy is still running; try again when it has finished".into());
            return true;
        }
        false
    }

    // ---- keys and menu -----------------------------------------------------

    /// Keys for the focused tree that work on the selection. Returns false
    /// for everything else (single-entry F2 / Delete stay where they were).
    pub(super) fn tree_key(&mut self, key: &Key, mods: Mods) -> bool {
        if mods.alt {
            return false;
        }
        match key {
            Key::Delete if !mods.ctrl && self.tree.is_multi() => {
                let targets = self.tree_targets();
                self.ask_recycle_many(targets);
            }
            Key::F(2) if !mods.ctrl && self.tree.is_multi() => {
                self.notice = Some("Rename works on one entry: select just that one".into());
            }
            Key::Char('c') if mods.ctrl && !mods.shift => {
                let targets = self.tree_targets();
                self.tree_copy(targets);
            }
            Key::Char('v') if mods.ctrl && !mods.shift => {
                // Into the folder under the cursor, or next to the file.
                let dir = match self.tree.cursor_entry() {
                    Some(r) if r.entry.is_dir && !r.entry.is_link => Some(r.entry.path),
                    Some(r) => r.entry.path.parent().map(Path::to_path_buf),
                    None => self.tree.root().map(Path::to_path_buf),
                };
                if let Some(dir) = dir {
                    self.tree_paste(dir);
                }
            }
            Key::Char('d') if mods.ctrl && !mods.shift => {
                let targets = self.tree_targets();
                self.tree_duplicate(targets);
            }
            _ => return false,
        }
        self.pending = true;
        true
    }

    /// Right-click on one of several selected entries: a menu for all of
    /// them. Returns false if the click was not on such an entry.
    pub(super) fn selection_menu(&mut self, x: f32, y: f32, path: &Path, dir: &Path) -> bool {
        if !(self.tree.is_multi() && self.tree.is_selected(path)) {
            return false;
        }
        let targets = self.tree_targets();
        if targets.is_empty() {
            return false;
        }
        let n = match targets.len() {
            1 => String::new(),
            n => format!(" {n} items"),
        };
        let (copy, duplicate, delete) = (
            format!("Copy{n}"),
            format!("Duplicate{n}"),
            format!("Delete{n} (to Recycle Bin)"),
        );
        let mut items = vec![(copy.as_str(), MenuAction::CopyEntries(targets.clone()))];
        if self.can_paste() {
            items.push(("Paste", MenuAction::PasteInto(dir.to_path_buf())));
        }
        items.push((duplicate.as_str(), MenuAction::Duplicate(targets.clone())));
        items.push((delete.as_str(), MenuAction::DeleteMany(targets)));
        self.menu = Some(crate::menu::Menu::new(x, y, items));
        true
    }

    /// Is there something to paste?
    pub(super) fn can_paste(&self) -> bool {
        !self.ops.copied.is_empty()
    }

    /// Menu entries handled here. Returns false for all others.
    pub(super) fn run_tree_menu(&mut self, a: &MenuAction) -> bool {
        match a {
            MenuAction::CopyEntries(p) => self.tree_copy(p.clone()),
            MenuAction::PasteInto(dir) => self.tree_paste(dir.clone()),
            MenuAction::Duplicate(p) => self.tree_duplicate(p.clone()),
            MenuAction::DeleteMany(p) => self.ask_recycle_many(p.clone()),
            // Renaming or deleting under a running copy would cut it short.
            MenuAction::Rename(_) | MenuAction::Delete(_) if self.ops.job.is_some() => {
                self.copy_busy();
            }
            _ => return false,
        }
        self.pending = true;
        true
    }

    // ---- delete several ----------------------------------------------------

    /// Ask once before moving `paths` to the Recycle Bin.
    pub(super) fn ask_recycle_many(&mut self, paths: Vec<PathBuf>) {
        let paths = self.plan(paths);
        match paths.as_slice() {
            [] => return,
            // One entry: the question and answer that were always there.
            [one] => {
                self.run_menu(MenuAction::Delete(one.clone()));
                return;
            }
            _ => {}
        }
        if self.copy_busy() {
            return;
        }
        if self.nag.is_some() {
            self.notice = Some("Answer the question above first, then delete again".into());
            return;
        }
        if let Some(refusal) = self.unsaved_refusal(&paths, "deleted") {
            self.nag = Some(Nag::error(refusal));
            return;
        }
        self.nag = Some(Nag::new(
            format!(
                "Move {} items to the Recycle Bin? ({})",
                paths.len(),
                name_list(&paths, 3)
            ),
            vec![
                ("Move to Recycle Bin", NagAction::RecycleMany(paths)),
                ("Cancel", NagAction::Dismiss),
            ],
            false,
        ));
    }

    /// The answer was "Move to Recycle Bin".
    pub(super) fn recycle_many(&mut self, paths: Vec<PathBuf>) {
        // Things may have changed while the question was up.
        if let Some(refusal) = self.unsaved_refusal(&paths, "deleted") {
            self.nag = Some(Nag::error(refusal));
            return;
        }
        let mut failed: Vec<(PathBuf, String)> = Vec::new();
        for p in &paths {
            match fsops::move_to_recycle_bin(p) {
                Ok(()) => self.close_clean_tabs_under(p),
                Err(e) => failed.push((p.clone(), e)),
            }
            self.after_fs_change(p);
        }
        let ok = paths.len() - failed.len();
        if failed.is_empty() {
            self.notice = Some(format!("Moved {ok} items to the Recycle Bin"));
            return;
        }
        let left: Vec<PathBuf> = failed.iter().map(|(p, _)| p.clone()).collect();
        self.tree.select_paths(&left);
        self.report(
            batch_report(
                "Moved",
                ok,
                paths.len(),
                " to the Recycle Bin",
                "Not deleted",
                &failed,
            ),
            true,
        );
    }

    // ---- move several ------------------------------------------------------

    /// A tree drag ended on folder `dir`: ask once before moving `paths`.
    pub(super) fn ask_move_many(&mut self, paths: Vec<PathBuf>, dir: PathBuf) {
        // Entries already in that folder have nowhere to go.
        let paths: Vec<PathBuf> = self
            .plan(paths)
            .into_iter()
            .filter(|p| p.parent() != Some(dir.as_path()))
            .collect();
        if paths.is_empty() || self.copy_busy() {
            return;
        }
        if let [one] = paths.as_slice() {
            self.ask_move(one.clone(), dir);
            return;
        }
        // Never replace a question that is already showing.
        if self.nag.is_some() {
            self.notice = Some("Answer the question above first, then move them again".into());
            return;
        }
        if let Some(refusal) = self.unsaved_refusal(&paths, "moved") {
            self.nag = Some(Nag::error(refusal));
            return;
        }
        self.nag = Some(Nag::new(
            format!(
                "Move {} items into {}? ({})",
                paths.len(),
                display_name(&dir),
                name_list(&paths, 3)
            ),
            vec![
                ("Move", NagAction::MoveMany(paths, dir)),
                ("Cancel", NagAction::Dismiss),
            ],
            false,
        ));
    }

    /// The answer was "Move".
    pub(super) fn move_many(&mut self, paths: Vec<PathBuf>, dir: PathBuf) {
        if let Some(refusal) = self.unsaved_refusal(&paths, "moved") {
            self.nag = Some(Nag::error(refusal));
            return;
        }
        let mut moved: Vec<PathBuf> = Vec::new();
        let mut failed: Vec<(PathBuf, String)> = Vec::new();
        for p in &paths {
            // Its open tabs (all without edits, checked above) follow it.
            let open = self.open_tabs_under(p);
            match fsops::move_into(p, &dir) {
                Ok(new) => {
                    self.relocated(p, &new, &open);
                    moved.push(new);
                }
                // (The usual reason, kept short: the report is one line.)
                Err(_) if p.file_name().is_some_and(|n| dir.join(n).exists()) => {
                    failed.push((p.clone(), "that name is already used there".into()))
                }
                Err(e) => failed.push((p.clone(), e)),
            }
        }
        if failed.is_empty() {
            self.tree.select_paths(&moved);
            self.notice = Some(format!(
                "Moved {} items into {}",
                moved.len(),
                display_name(&dir)
            ));
            return;
        }
        let left: Vec<PathBuf> = failed.iter().map(|(p, _)| p.clone()).collect();
        self.tree.select_paths(&left);
        self.report(
            batch_report(
                "Moved",
                moved.len(),
                paths.len(),
                &format!(" into {}", display_name(&dir)),
                "Not moved",
                &failed,
            ),
            true,
        );
    }

    // ---- copy / paste / duplicate ------------------------------------------

    pub(super) fn tree_copy(&mut self, paths: Vec<PathBuf>) {
        let paths = self.plan(paths);
        if paths.is_empty() {
            self.notice = Some("Select a file or folder in the tree to copy it".into());
            return;
        }
        self.notice = Some(format!(
            "Copied {} — Ctrl+V in the tree pastes into the selected folder",
            count_or_name(&paths)
        ));
        self.ops.copied = paths;
    }

    pub(super) fn tree_paste(&mut self, dir: PathBuf) {
        if self.ops.copied.is_empty() {
            self.notice = Some("Nothing to paste: copy a file or folder in the tree first".into());
            return;
        }
        let items = self
            .ops
            .copied
            .iter()
            .map(|p| (p.clone(), dir.clone()))
            .collect();
        self.start_copy(items, "Pasted", Some(dir));
    }

    pub(super) fn tree_duplicate(&mut self, paths: Vec<PathBuf>) {
        let items: Vec<(PathBuf, PathBuf)> = self
            .plan(paths)
            .into_iter()
            .filter_map(|p| {
                let dir = p.parent()?.to_path_buf();
                Some((p, dir))
            })
            .collect();
        if items.is_empty() {
            self.notice = Some("Select a file or folder in the tree to duplicate it".into());
            return;
        }
        self.start_copy(items, "Duplicated", None);
    }

    fn start_copy(
        &mut self,
        items: Vec<(PathBuf, PathBuf)>,
        verb: &'static str,
        into: Option<PathBuf>,
    ) {
        if self.copy_busy() {
            return;
        }
        #[cfg(test)]
        let limits = self.ops.limits.unwrap_or_default();
        #[cfg(not(test))]
        let limits = CopyLimits::default();
        let count = items.len();
        match CopyJob::new(items, limits) {
            Ok(job) => {
                self.ops.job = Some(Running {
                    job,
                    verb,
                    into,
                    count,
                });
                // Small copies are done before the next frame; big ones go
                // on in `tick` with their progress in the status bar.
                #[cfg(test)]
                let first = self.ops.first_slice.unwrap_or(FIRST_SLICE);
                #[cfg(not(test))]
                let first = FIRST_SLICE;
                self.copy_tick(first);
            }
            // Too big: refused before anything was written.
            Err(why) => self.report(
                format!("Nothing was copied: that is {why}, the most one copy may hold."),
                true,
            ),
        }
        self.pending = true;
    }

    /// Stop a running copy (the window is closing): the file being written
    /// is removed; finished files stay.
    pub(super) fn cancel_copy(&mut self) {
        if let Some(mut run) = self.ops.job.take() {
            run.job.cancel();
        }
    }

    /// Part of `App::tick`: continue a running copy, show a waiting report.
    pub(super) fn copy_tick(&mut self, budget: Duration) -> Tick {
        let mut tick = Tick::default();
        if self.nag.is_none()
            && let Some((message, error)) = self.ops.report.take()
        {
            self.report(message, error);
            tick.redraw = true;
        }
        let Some(run) = self.ops.job.as_mut() else {
            return tick;
        };
        if run.job.step(budget) {
            let progress = run.job.progress();
            tick.redraw |= progress != self.ops.progress;
            self.ops.progress = progress;
            tick.more = true;
            return tick;
        }
        if let Some(run) = self.ops.job.take() {
            self.finish_copy(run);
        }
        self.ops.progress.clear();
        tick.redraw = true;
        tick
    }

    fn finish_copy(&mut self, mut run: Running) {
        let done = std::mem::take(&mut run.job.done);
        let failed = std::mem::take(&mut run.job.failed);
        for p in &done {
            self.after_fs_change(p);
        }
        for (p, _) in &failed {
            // A folder copy that stopped midway left its finished part.
            if let Some(dir) = run.into.as_deref().or(p.parent()) {
                self.after_fs_change(&dir.join("x"));
            }
        }
        if let Some(first) = done.first() {
            self.tree.reveal(first);
            self.tree.select_paths(&done);
        }
        let place = run
            .into
            .as_deref()
            .map(|d| format!(" into {}", display_name(d)))
            .unwrap_or_default();
        let verb = run.verb;
        let skipped = match run.job.skipped {
            0 => String::new(),
            n => format!(
                "Skipped {n} link{} or special file{} ({}{}), which are never copied.",
                if n == 1 { "" } else { "s" },
                if n == 1 { "" } else { "s" },
                run.job.skipped_names.join(", "),
                if n > run.job.skipped_names.len() {
                    ", …"
                } else {
                    ""
                },
            ),
        };
        let summary = match done.as_slice() {
            [] => format!("{verb} nothing{place}"),
            [one] if run.into.is_none() => format!("{verb} as {}", display_name(one)),
            [one] => format!("{verb} {}{place}", display_name(one)),
            _ => format!("{verb} {} items{place}", done.len()),
        };
        if failed.is_empty() && skipped.is_empty() {
            self.notice = Some(summary);
        } else if failed.is_empty() {
            self.report(format!("{summary}. {skipped}"), false);
        } else {
            let mut message =
                batch_report(verb, done.len(), run.count, &place, "Not copied", &failed);
            if !skipped.is_empty() {
                message.push_str(". ");
                message.push_str(&skipped);
            }
            self.report(message, true);
        }
    }

    // ---- status bar --------------------------------------------------------

    fn stat_details(&mut self, path: &Path) -> Details {
        #[cfg(test)]
        {
            self.ops.stats += 1;
        }
        let info = std::fs::metadata(path)
            .ok()
            .filter(|m| m.is_file())
            .map(|m| (m.len(), m.modified().ok()));
        Details {
            path: path.to_path_buf(),
            info,
        }
    }

    /// Whose details the status bar shows: with the keyboard in the tree,
    /// the one selected entry (a folder shows nothing); otherwise the file
    /// in the active tab.
    fn details_path(&self) -> Option<PathBuf> {
        if self.focus == super::Focus::Tree
            && let Some(p) = self.tree.single_selected()
        {
            return Some(p.to_path_buf());
        }
        self.view.path().map(Path::to_path_buf)
    }

    /// Before drawing: read the active file's size and date if another file
    /// became active. Compares paths only; the disk is touched once per file.
    pub(super) fn sync_file_details(&mut self) {
        let current = self.details_path();
        if self.ops.details.as_ref().map(|d| &d.path) == current.as_ref() {
            return;
        }
        self.ops.details = current.map(|p| self.stat_details(&p));
    }

    /// These files were saved or changed on disk: read the active file's
    /// details again if it is one of them.
    pub(super) fn file_details_changed(&mut self, paths: &[PathBuf]) {
        if let Some(path) = self.ops.details.as_ref().map(|d| d.path.clone())
            && paths.contains(&path)
        {
            self.ops.details = Some(self.stat_details(&path));
        }
    }

    /// For the left of the status bar: a running copy, a selection of
    /// several tree entries.
    pub(super) fn status_tree_items(&self) -> Vec<String> {
        let mut v = Vec::new();
        if self.ops.job.is_some() {
            v.push(format!("Copying… {}", self.ops.progress));
        }
        let n = self.tree.selected_count();
        if n > 1 {
            v.push(format!("{n} selected in the tree"));
        }
        v
    }

    /// For the right of the status bar: "12.4 KB · changed 2 h ago" for the
    /// selected or active file (nothing until its details were read).
    pub(super) fn status_file_details(&self) -> Option<String> {
        let d = self.ops.details.as_ref()?;
        if Some(&d.path) != self.details_path().as_ref() {
            return None;
        }
        let (size, modified) = d.info?;
        Some(details_text(size, modified, now()))
    }

    // ---- test hooks --------------------------------------------------------

    /// The tree's selected entries, top to bottom.
    #[cfg(test)]
    pub fn tree_selection(&mut self) -> Vec<PathBuf> {
        self.tree
            .selection()
            .into_iter()
            .map(|r| r.entry.path)
            .collect()
    }

    /// The file in the active tab.
    #[cfg(test)]
    pub fn active_path(&self) -> Option<PathBuf> {
        self.view.path().map(Path::to_path_buf)
    }

    /// Ask to delete exactly these paths (as a menu action would).
    #[cfg(test)]
    pub fn ask_delete_paths(&mut self, paths: Vec<PathBuf>) {
        self.ask_recycle_many(paths);
    }

    /// The answer "Move to Recycle Bin" arriving for these paths
    /// without a question first (to test the checks made at answer time).
    #[cfg(test)]
    pub fn answer_delete(&mut self, paths: Vec<PathBuf>) {
        self.recycle_many(paths);
    }

    #[cfg(test)]
    pub fn answer_move_only(&mut self, paths: Vec<PathBuf>, dir: PathBuf) {
        self.move_many(paths, dir);
    }

    #[cfg(test)]
    pub fn copied_entries(&self) -> Vec<PathBuf> {
        self.ops.copied.clone()
    }

    #[cfg(test)]
    pub fn set_copy_limits(&mut self, entries: usize, bytes: u64) {
        self.ops.limits = Some(CopyLimits { entries, bytes });
    }

    /// Make copies start with (almost) no work, so tests see them running.
    #[cfg(test)]
    pub fn set_copy_first_slice(&mut self, d: Duration) {
        self.ops.first_slice = Some(d);
    }

    /// Paths of the tree's rows, top to bottom.
    #[cfg(test)]
    pub fn tree_rows(&mut self) -> Vec<PathBuf> {
        self.tree
            .rows()
            .iter()
            .map(|r| r.entry.path.clone())
            .collect()
    }

    #[cfg(test)]
    pub fn copy_running(&self) -> bool {
        self.ops.job.is_some()
    }

    /// How often file details were read from disk.
    #[cfg(test)]
    pub fn file_stat_count(&self) -> usize {
        self.ops.stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_times_are_coarse() {
        let s = Duration::from_secs;
        assert_eq!(ago(s(0)), "just now");
        assert_eq!(ago(s(59)), "just now");
        assert_eq!(ago(s(60)), "1 min ago");
        assert_eq!(ago(s(59 * 60 + 59)), "59 min ago");
        assert_eq!(ago(s(3600)), "1 h ago");
        assert_eq!(ago(s(2 * 3600 + 1800)), "2 h ago");
        assert_eq!(ago(s(47 * 3600)), "47 h ago");
        assert_eq!(ago(s(48 * 3600)), "2 days ago");
        assert_eq!(ago(s(59 * 86_400)), "59 days ago");
        assert_eq!(ago(s(60 * 86_400)), "2 months ago");
        assert_eq!(ago(s(729 * 86_400)), "24 months ago");
        assert_eq!(ago(s(730 * 86_400)), "2 years ago");
    }

    #[test]
    fn details_show_size_and_age() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let two_hours = now - Duration::from_secs(2 * 3600 + 5);
        assert_eq!(
            details_text(12_698, Some(two_hours), now),
            "12.4 KB · changed 2 h ago"
        );
        // A file dated in the future (clock set back) is not an error.
        let future = now + Duration::from_secs(500);
        assert_eq!(
            details_text(0, Some(future), now),
            "0 bytes · changed just now"
        );
        assert_eq!(details_text(3 << 20, None, now), "3.0 MB");
    }

    #[test]
    fn reports_name_what_failed_and_why() {
        let f = |n: &str, why: &str| (PathBuf::from(format!("/r/{n}")), why.to_string());
        assert_eq!(
            batch_report(
                "Moved",
                2,
                3,
                " into docs",
                "Not moved",
                &[f("a.txt", "docs already has something named a.txt")]
            ),
            "Moved 2 of 3 into docs. Not moved: a.txt (docs already has something named a.txt)"
        );
        // The same reason is said once; only the first few are named.
        let mut many: Vec<_> = (0..3).map(|i| f(&format!("f{i}"), "gone")).collect();
        many.insert(1, f("x", "in use"));
        many.push(f("f3", "gone"));
        many.push(f("f4", "gone"));
        assert_eq!(
            batch_report("Moved", 0, 6, " to the Recycle Bin", "Not deleted", &many),
            "Moved 0 of 6 to the Recycle Bin. Not deleted: f0, f1, f2 (gone); x (in use) \
             and 2 more (selected in the tree)"
        );
        let p = |s: &str| PathBuf::from(s);
        assert_eq!(name_list(&[p("/a/x"), p("/a/y")], 3), "x, y");
        assert_eq!(
            name_list(&[p("/a/x"), p("/a/y"), p("/a/z"), p("/a/w")], 2),
            "x, y and 2 more"
        );
    }
}
