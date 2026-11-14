//! Application state, layout and input dispatch. Platform-independent: the
//! window, `--script` mode and tests all drive [`App`] through [`Input`].
//!
//! Layout (lite-xl RootView): `[tree | divider | main view]` above a status bar.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::canvas::{Canvas, Rect};
use crate::doc::Document;
use crate::editor::doc::SaveError;
use crate::input::parse_key_combo;
use crate::input::{Cursor, Input, Key, MouseButton, Request};
use crate::menu::{Menu, MenuAction, MenuResult};
use crate::nag::{Nag, NagAction, Pending};
use crate::palette::{Action, Outcome, Palette, index_files};
use crate::session::{Session, push_recent};
use crate::settings::{self, Settings};
use crate::text::Fonts;
use crate::theme;
use crate::tree::Tree;
use crate::ui::{Ctx, FontKind};
use crate::view::code::CodeView;
use crate::view::markdown::{MarkdownView, is_markdown};
use crate::view::message::MessageView;
use crate::view::search::SearchView;
use crate::view::{DiskState, Tick, View};

pub const MIN_ZOOM: f32 = 0.5;
pub const MAX_ZOOM: f32 = 3.0;
/// Logical width limits of the tree panel.
const TREE_MIN_W: f32 = 140.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Main,
    Tree,
}

pub struct App {
    pub fonts: Fonts,
    scale: f32,
    zoom: f32,
    size: (u32, u32),
    /// The active tab's view. Inactive tabs live in `tabs` (their slot for
    /// the active tab is empty while it is borrowed here).
    view: Box<dyn View>,
    tabs: Vec<TabSlot>,
    active: usize,
    next_tab_id: u64,
    /// Tabs whose file changed on disk while they had unsaved edits; the
    /// question is asked when the tab is shown.
    disk_prompt: Vec<u64>,
    tree: Tree,
    focus: Focus,
    requests: Vec<Request>,
    cursor: Cursor,
    mouse: (f32, f32),
    dragging_divider: bool,
    clipboard: Box<dyn crate::clipboard::Clipboard>,
    /// Modal question bar (save changes?, changed on disk?, errors).
    nag: Option<Nag>,
    /// Command palette / quick open / go to line overlay.
    palette: Option<Palette>,
    /// Files under the tree root for quick open (rebuilt after changes).
    file_index: Option<(PathBuf, Vec<PathBuf>)>,
    /// Right-click menu (tree).
    menu: Option<Menu>,
    /// What the open text prompt is for.
    prompt_for: Option<MenuAction>,
    /// Recently opened folders and files, most recent first.
    recent: Vec<PathBuf>,
    /// Something worth remembering changed (the window saves the session).
    session_dirty: bool,
    /// The session as loaded at startup.
    stored: Session,
    /// This instance restored the workspace, so it may write folder and
    /// tabs back. An instance started with a file argument only updates
    /// recents and preferences, never the saved workspace.
    owns_workspace: bool,
    /// Tabs whose edits the user chose to drop during a quit sequence; they
    /// are only dropped once every prompt is answered.
    quit_skip: Vec<u64>,
    /// What to do after "save anyway" for a lossy file.
    after_lossy: Option<Pending>,
    /// One-shot message in the status bar (e.g. "Saved").
    notice: Option<String>,
    /// Set when a view may have deferred work; cleared when `tick` finishes it.
    pending: bool,
    /// The settings file (window mode only; headless runs use defaults).
    settings_path: Option<PathBuf>,
    /// Soft wrap for newly opened code files (setting `wrap_code`).
    wrap_code: bool,
    /// Extra ignored names currently in effect (setting `ignore`).
    ignore: Vec<String>,
}

struct TabSlot {
    id: u64,
    view: Option<Box<dyn View>>,
}

struct Layout {
    nag: Option<Rect>,
    tabs: Option<Rect>,
    tree: Option<Rect>,
    divider: Option<Rect>,
    main: Rect,
    status: Rect,
}

macro_rules! ctx {
    ($s:ident) => {
        Ctx {
            fonts: &mut $s.fonts,
            scale: $s.scale,
            zoom: $s.zoom,
            requests: &mut $s.requests,
            cursor: &mut $s.cursor,
            focused: $s.focus == Focus::Main,
            clipboard: &mut *$s.clipboard,
        }
    };
}

impl App {
    pub fn new(fonts: Fonts, scale: f32) -> Self {
        Self {
            fonts,
            scale,
            zoom: 1.0,
            size: (1, 1),
            view: Box::new(MessageView::welcome()),
            tabs: vec![TabSlot { id: 1, view: None }],
            active: 0,
            next_tab_id: 2,
            disk_prompt: Vec::new(),
            tree: Tree::new(),
            focus: Focus::Main,
            requests: Vec::new(),
            cursor: Cursor::Default,
            mouse: (0.0, 0.0),
            dragging_divider: false,
            clipboard: Box::new(crate::clipboard::MemClipboard::default()),
            nag: None,
            palette: None,
            file_index: None,
            menu: None,
            prompt_for: None,
            recent: Vec::new(),
            session_dirty: false,
            stored: Session::default(),
            // No saved session loaded: this instance's workspace is the one
            // to save. `start_with_session(.., false)` turns this off.
            owns_workspace: true,
            quit_skip: Vec::new(),
            after_lossy: None,
            notice: None,
            pending: false,
            settings_path: None,
            wrap_code: false,
            ignore: Vec::new(),
        }
    }

    /// Read settings from `path` and keep it as the settings file (the
    /// window does this at startup; problems go to the status bar).
    pub fn use_settings(&mut self, path: PathBuf) {
        let (s, mut problems) = Settings::load(&path);
        self.settings_path = Some(path);
        for (want, got) in self.font_mismatches(&s) {
            problems.push(format!("font '{want}' not found; using {got}"));
        }
        self.apply_settings(&s, &problems);
    }

    /// Requested font families that are not the ones in use.
    fn font_mismatches(&self, s: &Settings) -> Vec<(String, String)> {
        [
            (&s.ui_font, &self.fonts.sans),
            (&s.code_font, &self.fonts.mono),
        ]
        .into_iter()
        .filter_map(|(want, got)| {
            want.as_ref()
                .filter(|w| !w.eq_ignore_ascii_case(got))
                .map(|w| (w.clone(), got.clone()))
        })
        .collect()
    }

    /// Make `s` current: palette, font sizes and the wrap default.
    fn apply_settings(&mut self, s: &Settings, problems: &[String]) {
        let sizes_before = (
            theme::font_size(),
            theme::code_font_size(),
            theme::md_font_size(),
        );
        let light_before = theme::is_light();
        let ignore_changed = self.ignore != s.ignore;
        s.apply_globals();
        if ignore_changed {
            self.ignore = s.ignore.clone();
            crate::tree::set_extra_ignored(s.ignore.clone());
            self.tree.reload_all();
            self.file_index = None;
        }
        self.wrap_code = s.wrap_code;
        if theme::is_light() != light_before {
            self.each_view(|v| v.theme_changed());
        }
        let sizes = (
            theme::font_size(),
            theme::code_font_size(),
            theme::md_font_size(),
        );
        if sizes != sizes_before {
            self.each_view(|v| v.metrics_changed());
        }
        if let Some(first) = problems.first() {
            let more = if problems.len() > 1 {
                format!(" (+{} more)", problems.len() - 1)
            } else {
                String::new()
            };
            self.notice = Some(format!("Settings: {first}{more}"));
        }
        self.pending = true;
    }

    /// Switch palettes now (script `theme`, `--theme`, the toggle command).
    pub fn set_light(&mut self, light: bool) {
        if theme::is_light() != light {
            theme::set_light(light);
            self.each_view(|v| v.theme_changed());
            self.pending = true;
        }
    }

    fn each_view(&mut self, mut f: impl FnMut(&mut dyn View)) {
        f(&mut *self.view);
        for t in &mut self.tabs {
            if let Some(v) = t.view.as_mut() {
                f(&mut **v);
            }
        }
    }

    fn is_settings_file(&self, p: &Path) -> bool {
        self.settings_path
            .as_deref()
            .is_some_and(|s| settings::same_path(s, p))
    }

    /// After saving `p`: if it is the settings file, apply it.
    fn after_save(&mut self, p: &Path) {
        if self.is_settings_file(p) {
            let (s, problems) = Settings::load(p);
            self.apply_settings(&s, &problems);
            if problems.is_empty() {
                self.notice = Some(if self.font_mismatches(&s).is_empty() {
                    "Settings applied".into()
                } else {
                    "Settings applied; font changes take effect at next start".into()
                });
            }
        }
    }

    /// "Preferences: Open Settings" (Ctrl+,): open the file, creating it from
    /// the template if there is none (an existing file is never replaced).
    fn open_settings(&mut self) {
        let Some(path) = self.settings_path.clone() else {
            self.notice = Some("No settings file in this mode".into());
            return;
        };
        if !path.exists() {
            let created = path
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|_| {
                    use std::io::Write;
                    std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&path)?
                        .write_all(settings::TEMPLATE.as_bytes())
                });
            if let Err(e) = created
                && !path.exists()
            {
                self.nag = Some(Nag::error(format!(
                    "Could not create {}: {e}",
                    path.display()
                )));
                return;
            }
        }
        self.open(&path);
    }

    /// "View: Toggle Light / Dark Theme": switch now and record it in the
    /// settings file (only its `theme` line changes). If the settings file is
    /// open with unsaved edits the file is left alone.
    fn toggle_theme(&mut self) {
        let light = !theme::is_light();
        self.set_light(light);
        let name = if light { "Light" } else { "Dark" };
        let Some(path) = self.settings_path.clone() else {
            self.notice = Some(format!("{name} theme"));
            return;
        };
        // The tab may spell the path differently (case); use its own path
        // for the change notice so the reload matches.
        let open_as: Vec<(PathBuf, bool)> = (0..self.tabs.len())
            .filter_map(|i| {
                let v = self.tab_view(i);
                v.path()
                    .filter(|p| settings::same_path(p, &path))
                    .map(|p| (p.to_path_buf(), v.is_dirty()))
            })
            .collect();
        let dirty_open = open_as.iter().any(|(_, dirty)| *dirty);
        if dirty_open {
            self.notice = Some(format!(
                "{name} theme for now; settings.txt has unsaved edits, so it was not changed"
            ));
            return;
        }
        let old = match std::fs::metadata(&path) {
            Ok(m) if m.len() > settings::MAX_BYTES => {
                self.notice = Some(format!("{name} theme for now; settings.txt is too large"));
                return;
            }
            Ok(_) => std::fs::read(&path)
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .ok(),
            Err(_) => Some(settings::TEMPLATE.to_string()),
        };
        let Some(old) = old else {
            self.notice = Some(format!("{name} theme for now; could not read settings.txt"));
            return;
        };
        let new = settings::with_theme(&old, light);
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match crate::editor::save::save_bytes(&path, new.as_bytes()) {
            Ok(()) => {
                self.notice = Some(format!("{name} theme (saved in settings)"));
                // A clean tab showing the file picks up the change.
                let mut changed: Vec<PathBuf> = open_as.into_iter().map(|(p, _)| p).collect();
                changed.push(path);
                self.handle(Input::FsChanged(changed));
            }
            Err(e) => {
                self.notice = Some(format!(
                    "{name} theme for now; could not save settings: {e}"
                ));
            }
        }
    }

    /// Use the system clipboard (the real window does this).
    pub fn set_clipboard(&mut self, c: Box<dyn crate::clipboard::Clipboard>) {
        self.clipboard = c;
    }

    #[cfg(test)]
    pub fn clipboard_text(&mut self) -> Option<String> {
        self.clipboard.get()
    }

    /// Open a file (in a tab) or a folder (in the tree). Opening never
    /// discards anything: an already-open file is brought to the front,
    /// otherwise a new tab is added.
    pub fn open(&mut self, path: &Path) {
        if path.exists() {
            push_recent(&mut self.recent, path);
            self.session_dirty = true;
        }
        if path.is_dir() {
            // A folder only changes the tree; open files stay open.
            self.tree.set_root(path);
            self.tree.visible = true;
            if self.view.path().is_none() {
                self.view = Box::new(MessageView::folder_opened());
            } else if let Some(p) = self.view.path().map(Path::to_path_buf) {
                self.tree.reveal(&p);
            }
            self.focus = Focus::Tree;
            return;
        }
        self.open_now(path);
    }

    // ---- tabs ------------------------------------------------------------

    fn tab_view(&self, i: usize) -> &dyn View {
        if i == self.active {
            &*self.view
        } else {
            self.tabs[i]
                .view
                .as_deref()
                .expect("inactive tab has its view")
        }
    }

    pub fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    fn tab_index(&self, id: u64) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }

    fn find_tab(&self, path: &Path) -> Option<usize> {
        (0..self.tabs.len()).find(|&i| self.tab_view(i).path() == Some(path))
    }

    /// Bring tab `i` to the front.
    pub fn activate(&mut self, i: usize) {
        if i == self.active || i >= self.tabs.len() {
            return;
        }
        let incoming = self.tabs[i].view.take().expect("inactive tab has its view");
        let outgoing = std::mem::replace(&mut self.view, incoming);
        self.tabs[self.active].view = Some(outgoing);
        self.active = i;
        self.view.metrics_changed();
        if let Some(p) = self.view.path().map(Path::to_path_buf) {
            self.tree.reveal(&p);
        }
        let id = self.tabs[i].id;
        if let Some(k) = self.disk_prompt.iter().position(|t| *t == id) {
            self.disk_prompt.remove(k);
            if self.view.is_dirty() && self.view.disk_state() == DiskState::Differs {
                let name = self.view_name();
                self.nag = Some(Nag::changed_on_disk(&name));
            }
        }
        self.pending = true;
    }

    /// Add a view as a new tab after the active one (or replace the welcome
    /// screen) and show it.
    fn add_tab(&mut self, view: Box<dyn View>) {
        if self.view.path().is_none() && self.tabs.len() == 1 {
            self.view = view;
            return;
        }
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let at = self.active + 1;
        self.tabs.insert(
            at,
            TabSlot {
                id,
                view: Some(view),
            },
        );
        self.activate(at);
    }

    /// Close tab `i`, asking first if it has unsaved edits.
    pub fn close_tab(&mut self, i: usize) {
        if i >= self.tabs.len() {
            return;
        }
        let id = self.tabs[i].id;
        self.activate(i);
        self.guard(Pending::CloseTab(id));
    }

    /// Remove a tab without asking (its edits are dropped).
    fn remove_tab(&mut self, id: u64) {
        self.session_dirty = true;
        let Some(i) = self.tab_index(id) else { return };
        self.disk_prompt.retain(|t| *t != id);
        if self.tabs.len() == 1 {
            self.view = if self.tree.root().is_some() {
                Box::new(MessageView::folder_opened())
            } else {
                Box::new(MessageView::welcome())
            };
            return;
        }
        if i == self.active {
            let next = if i + 1 < self.tabs.len() {
                i + 1
            } else {
                i - 1
            };
            self.activate(next);
        }
        // `activate` moved the old active view into its slot.
        self.tabs.remove(i);
        if self.active > i {
            self.active -= 1;
        }
        self.pending = true;
    }

    /// The single discard gate: run `then` now, or ask first if the active
    /// document has unsaved edits.
    fn guard(&mut self, then: Pending) {
        if self.view.is_dirty() {
            let name = self.view_name();
            self.nag = Some(Nag::unsaved(&name, then));
        } else {
            self.perform(then);
        }
    }

    fn perform(&mut self, p: Pending) {
        match p {
            Pending::CloseTab(id) => self.remove_tab(id),
            Pending::Quit => {
                // Ask about each dirty tab in turn (skipping ones already
                // answered with "Don't save"), then quit.
                let skip = self.quit_skip.clone();
                match (0..self.tabs.len())
                    .find(|&i| self.tab_view(i).is_dirty() && !skip.contains(&self.tabs[i].id))
                {
                    Some(i) => {
                        self.activate(i);
                        let name = self.view_name();
                        self.nag = Some(Nag::unsaved(&name, Pending::Quit));
                    }
                    None => self.requests.push(Request::Quit),
                }
            }
        }
    }

    fn view_name(&self) -> String {
        self.view
            .path()
            .and_then(Path::file_name)
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "this file".into())
    }

    /// Ctrl+S.
    pub fn save(&mut self) -> bool {
        self.save_with(false, None)
    }

    fn save_with(&mut self, allow_lossy: bool, then: Option<Pending>) -> bool {
        if self.view.path().is_none() {
            return false;
        }
        let name = self.view_name();
        match self.view.save(allow_lossy) {
            Ok(n) => {
                self.notice = Some(format!("Saved {name} ({})", human_size(n)));
                if let Some(p) = self.view.path().map(Path::to_path_buf) {
                    self.after_save(&p);
                }
                if let Some(p) = then {
                    self.perform(p);
                }
            }
            Err(SaveError::NeedsLossyConfirm) => {
                self.after_lossy = then;
                self.nag = Some(Nag::new(
                    format!("{name} had invalid characters (shown as �). Saving writes them as �."),
                    vec![
                        ("Save anyway", NagAction::SaveAnyway),
                        ("Cancel", NagAction::Dismiss),
                    ],
                    false,
                ));
            }
            Err(SaveError::Failed(e)) => {
                self.nag = Some(Nag::error(format!("Could not save {name}: {e}")));
            }
        }
        true
    }

    /// Save the current file under a new name (from the Save As dialog).
    pub fn save_as(&mut self, path: &Path) {
        match self.view.save_as(path) {
            Ok(n) => {
                self.notice = Some(format!("Saved as {} ({})", path.display(), human_size(n)));
                self.tree.invalidate(&[path.to_path_buf()]);
                self.tree.reveal(path);
                self.after_save(path);
            }
            Err(SaveError::Failed(e)) => {
                self.nag = Some(Nag::error(format!(
                    "Could not save {}: {e}",
                    path.display()
                )));
            }
            Err(SaveError::NeedsLossyConfirm) => {}
        }
        self.pending = true;
    }

    fn apply_nag(&mut self, action: NagAction) {
        self.nag = None;
        match action {
            NagAction::SaveThen(p) => {
                self.save_with(false, Some(p));
            }
            NagAction::DiscardThen(Pending::Quit) => {
                // Remember the answer; nothing is dropped unless the whole
                // quit goes ahead.
                let id = self.tabs[self.active].id;
                self.quit_skip.push(id);
                self.perform(Pending::Quit);
            }
            NagAction::DiscardThen(p) => self.perform(p),
            NagAction::SaveAnyway => {
                let then = self.after_lossy.take();
                self.save_with(true, then);
            }
            NagAction::ReloadFromDisk => {
                let mut ctx = ctx!(self);
                self.view.reload_discarding(&mut ctx);
                self.pending = true;
            }
            NagAction::MoveInto(from, dir) => {
                // Re-check: things may have changed while the question was up.
                if self.tabs_under(&from).iter().any(|(_, _, dirty)| *dirty) {
                    self.nag = Some(Nag::error(format!(
                        "Save or close the open file(s) in {} before moving it.",
                        display_name(&from)
                    )));
                    return;
                }
                let open = self.open_tabs_under(&from);
                match crate::fsops::move_into(&from, &dir) {
                    Ok(new) => {
                        self.relocated(&from, &new, &open);
                        self.notice = Some(format!(
                            "Moved {} into {}",
                            display_name(&from),
                            display_name(&dir)
                        ));
                    }
                    Err(e) => {
                        self.nag = Some(Nag::error(format!(
                            "Could not move {}: {e}",
                            display_name(&from)
                        )))
                    }
                }
            }
            NagAction::Recycle(path) => {
                match crate::fsops::move_to_recycle_bin(&path) {
                    Ok(()) => {
                        self.close_clean_tabs_under(&path);
                        self.notice =
                            Some(format!("Moved {} to the Recycle Bin", display_name(&path)));
                    }
                    Err(e) => {
                        self.nag = Some(Nag::error(format!(
                            "Could not delete {}: {e}",
                            display_name(&path)
                        )))
                    }
                }
                self.after_fs_change(&path);
            }
            // Cancel (or an error acknowledged) aborts any quit in progress.
            NagAction::KeepMine | NagAction::Dismiss => self.quit_skip.clear(),
        }
    }

    fn open_quick_open(&mut self) {
        let Some(root) = self.tree.root().map(Path::to_path_buf) else {
            self.notice = Some("Open a folder first (Ctrl+Shift+O)".into());
            return;
        };
        if self.file_index.as_ref().is_none_or(|(r, _)| *r != root) {
            self.file_index = Some((root.clone(), index_files(&root)));
        }
        let files = &self.file_index.as_ref().expect("just built").1;
        self.palette = Some(Palette::files(&root, files));
    }

    fn run_palette(&mut self, o: Outcome) {
        match o {
            Outcome::Close => {}
            Outcome::Run(Action::Keys(combo)) => {
                if let Some((key, mods)) = parse_key_combo(combo) {
                    self.handle(Input::Key { key, mods });
                }
            }
            Outcome::Run(Action::Open(p)) => self.open(&p),
            Outcome::Run(Action::ToggleTheme) => self.toggle_theme(),
            Outcome::Run(Action::RevealInExplorer) => {
                if let Some(p) = self.view.path() {
                    self.requests
                        .push(Request::RevealInExplorer(p.to_path_buf()));
                }
            }
            Outcome::Text(t) => {
                if let Some(kind) = self.prompt_for.take() {
                    self.finish_prompt(kind, &t);
                }
            }
            Outcome::GoToLine(n) => {
                self.view.goto_line(n);
                self.focus = Focus::Main;
            }
        }
        self.pending = true;
    }

    /// Paths of tabs at or under `path`, with their dirty state.
    fn tabs_under(&self, path: &Path) -> Vec<(u64, PathBuf, bool)> {
        (0..self.tabs.len())
            .filter_map(|i| {
                let v = self.tab_view(i);
                let p = v.path()?.to_path_buf();
                p.starts_with(path)
                    .then(|| (self.tabs[i].id, p, v.is_dirty()))
            })
            .collect()
    }

    fn open_tabs_under(&self, path: &Path) -> Vec<(u64, PathBuf)> {
        self.tabs_under(path)
            .into_iter()
            .map(|(id, p, _)| (id, p))
            .collect()
    }

    /// After `old` was renamed or moved to `new`: reopen its (clean) tabs at
    /// their new paths and refresh the tree.
    fn relocated(&mut self, old: &Path, new: &Path, open: &[(u64, PathBuf)]) {
        for (id, p) in open {
            self.remove_tab(*id);
            let target = if p == old {
                new.to_path_buf()
            } else {
                new.join(p.strip_prefix(old).unwrap_or(Path::new("")))
            };
            self.open(&target);
        }
        self.after_fs_change(old);
        self.after_fs_change(new);
        self.tree.reveal(new);
    }

    /// A tree drag ended on a folder: ask before moving.
    fn ask_move(&mut self, from: PathBuf, dir: PathBuf) {
        if self.tabs_under(&from).iter().any(|(_, _, dirty)| *dirty) {
            self.nag = Some(Nag::error(format!(
                "Save or close the open file(s) in {} before moving it.",
                display_name(&from)
            )));
            return;
        }
        self.nag = Some(Nag::new(
            format!("Move {} into {}?", display_name(&from), display_name(&dir)),
            vec![
                ("Move", NagAction::MoveInto(from, dir)),
                ("Cancel", NagAction::Dismiss),
            ],
            false,
        ));
    }

    fn close_clean_tabs_under(&mut self, path: &Path) {
        for (id, _, dirty) in self.tabs_under(path) {
            if !dirty {
                self.remove_tab(id);
            }
        }
    }

    fn after_fs_change(&mut self, path: &Path) {
        self.tree.invalidate(&[path.to_path_buf()]);
        self.file_index = None;
        self.pending = true;
    }

    /// Right-click on the tree: open a menu for that entry.
    fn open_tree_menu(&mut self, x: f32, y: f32, tree_rect: Rect) {
        let row = {
            let ctx = ctx!(self);
            self.tree.entry_at(tree_rect, y, &ctx)
        };
        let Some(root) = self.tree.root().map(Path::to_path_buf) else {
            return;
        };
        let (path, is_dir, is_root) = match &row {
            Some(r) => (r.entry.path.clone(), r.entry.is_dir, r.depth == 0),
            None => (root.clone(), true, true),
        };
        let dir = if is_dir {
            path.clone()
        } else {
            path.parent().map(Path::to_path_buf).unwrap_or(root)
        };
        let mut items = vec![
            ("New file…", MenuAction::NewFile(dir.clone())),
            ("New folder…", MenuAction::NewFolder(dir)),
        ];
        if !is_root {
            items.push(("Rename…", MenuAction::Rename(path.clone())));
            items.push(("Delete (to Recycle Bin)", MenuAction::Delete(path.clone())));
        }
        if is_dir {
            items.push(("Find in folder…", MenuAction::FindInFolder(path.clone())));
        }
        items.push(("Show in Explorer", MenuAction::Reveal(path.clone())));
        items.push(("Copy path", MenuAction::CopyPath(path)));
        self.menu = Some(Menu::new(x, y, items));
    }

    fn run_menu(&mut self, a: MenuAction) {
        match &a {
            MenuAction::NewFile(_) => {
                self.palette = Some(Palette::text(
                    "New file",
                    "",
                    "Name of the new file, then Enter",
                ))
            }
            MenuAction::NewFolder(_) => {
                self.palette = Some(Palette::text(
                    "New folder",
                    "",
                    "Name of the new folder, then Enter",
                ))
            }
            MenuAction::Rename(p) => {
                if self.tabs_under(p).iter().any(|(_, _, dirty)| *dirty) {
                    self.nag = Some(Nag::error(format!(
                        "Save or close the open file(s) in {} before renaming.",
                        display_name(p)
                    )));
                    return;
                }
                self.palette = Some(Palette::text(
                    "Rename",
                    &display_name(p),
                    "New name, then Enter",
                ));
            }
            MenuAction::Delete(p) => {
                if self.tabs_under(p).iter().any(|(_, _, dirty)| *dirty) {
                    self.nag = Some(Nag::error(format!(
                        "Save or close the open file(s) in {} before deleting.",
                        display_name(p)
                    )));
                    return;
                }
                self.nag = Some(Nag::new(
                    format!("Move {} to the Recycle Bin?", display_name(p)),
                    vec![
                        ("Move to Recycle Bin", NagAction::Recycle(p.clone())),
                        ("Cancel", NagAction::Dismiss),
                    ],
                    false,
                ));
                return;
            }
            MenuAction::Reveal(p) => {
                self.requests.push(Request::RevealInExplorer(p.clone()));
                return;
            }
            MenuAction::CopyPath(p) => {
                self.clipboard.set(p.display().to_string());
                self.notice = Some("Path copied".into());
                return;
            }
            MenuAction::FindInFolder(_) => {
                self.palette = Some(Palette::text(
                    "Find in folder",
                    "",
                    "Text to search for, then Enter",
                ));
            }
        }
        self.prompt_for = Some(a);
    }

    fn finish_prompt(&mut self, kind: MenuAction, text: &str) {
        let result = match &kind {
            MenuAction::NewFile(dir) => crate::fsops::create_file(dir, text).map(|p| {
                self.after_fs_change(&p);
                self.open(&p);
                p
            }),
            MenuAction::NewFolder(dir) => crate::fsops::create_folder(dir, text).inspect(|p| {
                self.after_fs_change(p);
                self.tree.reveal(&p.join("x"));
            }),
            MenuAction::Rename(old) => {
                let open = self.open_tabs_under(old);
                crate::fsops::rename(old, text).inspect(|new| self.relocated(old, new, &open))
            }
            MenuAction::FindInFolder(root) => {
                if !text.trim().is_empty() {
                    let view = SearchView::start(root, text.trim());
                    self.add_tab(Box::new(view));
                    self.focus = Focus::Main;
                }
                return;
            }
            _ => return,
        };
        if let Err(e) = result {
            self.nag = Some(Nag::error(e));
        }
    }

    /// Apply the saved session at startup: recents and preferences always;
    /// the workspace (folder, tabs) only when `restore` (no file arguments).
    pub fn start_with_session(&mut self, s: Session, restore: bool) {
        self.stored = s.clone();
        self.owns_workspace = restore;
        if restore {
            self.restore(s);
        } else {
            self.recent = s.recent;
            self.tree.width = s.tree_width;
            self.tree.visible = s.tree_visible;
            self.zoom = s.zoom;
        }
        self.session_dirty = false;
    }

    /// What to remember for next time.
    pub fn session(&self) -> Session {
        let mut s = self.current_session();
        if !self.owns_workspace {
            s.root = self.stored.root.clone();
            s.tabs = self.stored.tabs.clone();
            s.active = self.stored.active;
        }
        s
    }

    fn current_session(&self) -> Session {
        let mut tabs = Vec::new();
        let mut active = 0;
        for i in 0..self.tabs.len() {
            if let Some(p) = self.tab_view(i).path() {
                if i == self.active {
                    active = tabs.len();
                }
                tabs.push(p.to_path_buf());
            }
        }
        Session {
            root: self.tree.root().map(Path::to_path_buf),
            tabs,
            active,
            recent: self.recent.clone(),
            tree_width: self.tree.width,
            tree_visible: self.tree.visible,
            zoom: self.zoom,
        }
    }

    /// Reopen the last session (only what still exists on disk).
    pub fn restore(&mut self, s: Session) {
        self.recent = s.recent;
        self.tree.width = s.tree_width;
        self.zoom = s.zoom;
        if let Some(r) = s.root.filter(|r| r.is_dir()) {
            self.tree.set_root(&r);
            self.view = Box::new(MessageView::folder_opened());
        }
        for t in s.tabs.iter().filter(|t| t.is_file()) {
            self.open_now(t);
        }
        if let Some(p) = s.tabs.get(s.active).filter(|p| p.is_file())
            && let Some(i) = self.find_tab(p)
        {
            self.activate(i);
        }
        self.tree.visible = s.tree_visible;
        self.focus = Focus::Main;
        self.session_dirty = false;
    }

    #[cfg(test)]
    pub fn zoom(&self) -> f32 {
        self.zoom
    }

    /// True once after something worth saving changed.
    pub fn take_session_dirty(&mut self) -> bool {
        std::mem::take(&mut self.session_dirty)
    }

    #[cfg(test)]
    pub fn snapshot(&self) -> Option<(String, Vec<crate::editor::buffer::Pos>)> {
        self.view.snapshot()
    }

    #[cfg(test)]
    pub fn nag_message(&self) -> Option<String> {
        self.nag.as_ref().map(|n| n.message.clone())
    }

    pub fn nag_text(&self) -> String {
        self.nag
            .as_ref()
            .map(|n| n.message.clone())
            .unwrap_or_default()
    }

    fn open_now(&mut self, path: &Path) {
        // D-012: opening a file with no folder open shows its folder.
        if self.tree.root().is_none()
            && let Some(parent) = path.parent().filter(|p| p.is_dir())
        {
            self.tree.set_root(parent);
        }
        if let Some(i) = self.find_tab(path) {
            self.activate(i);
            self.focus = Focus::Main;
            return;
        }
        let view: Box<dyn View> = match Document::open(path) {
            Ok(doc) if is_markdown(path) => Box::new(MarkdownView::new(doc)),
            Ok(doc) => {
                let mut v = CodeView::new(doc);
                v.wrap = self.wrap_code;
                Box::new(v)
            }
            Err(e) => Box::new(MessageView::error(
                "Error",
                format!("Could not open {}: {e}", path.display()),
            )),
        };
        self.add_tab(view);
        self.tree.reveal(path);
        self.focus = Focus::Main;
        self.pending = true;
    }

    pub fn title(&self) -> String {
        let t = self.view.title();
        if t.is_empty() {
            "litedit".into()
        } else {
            format!("{t} - litedit")
        }
    }

    /// Where the IME candidate window should appear (primary caret).
    pub fn ime_area(&self) -> Option<Rect> {
        if self.focus == Focus::Main && self.nag.is_none() {
            self.view.caret_rect()
        } else {
            None
        }
    }

    pub fn cursor(&self) -> Cursor {
        self.cursor
    }

    pub fn focus(&self) -> Focus {
        self.focus
    }

    pub fn take_requests(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.requests)
    }

    pub fn has_pending_work(&self) -> bool {
        self.pending
    }

    pub fn status_items(&self) -> Vec<String> {
        let mut v = self.view.status();
        if let Some(n) = &self.notice {
            v.push(n.clone());
        }
        if let Some(n) = self.tree.truncated_note() {
            v.push(n);
        }
        v
    }

    pub fn status_text(&self) -> String {
        self.status_items().join("   ")
    }

    /// Folders to watch for external changes: (path, recursive).
    pub fn watch_targets(&self) -> Vec<(PathBuf, bool)> {
        let mut v = Vec::new();
        if let Some(r) = self.tree.root() {
            v.push((r.to_path_buf(), true));
        }
        for i in 0..self.tabs.len() {
            if let Some(dir) = self.tab_view(i).path().and_then(Path::parent)
                && !self.tree.root().is_some_and(|r| dir.starts_with(r))
                && !v.iter().any(|(d, _)| d == dir)
            {
                v.push((dir.to_path_buf(), false));
            }
        }
        v
    }

    fn ui_line_height(&self) -> f32 {
        let s = (theme::font_size() * self.scale * self.zoom)
            .round()
            .max(4.0);
        (s * theme::LINE_HEIGHT).round()
    }

    fn tab_bar_height(&self) -> i32 {
        let pad_y = (theme::PADDING_Y * self.scale).round() as i32;
        self.ui_line_height() as i32 + pad_y * 2
    }

    fn show_tab_bar(&self) -> bool {
        self.tabs.len() > 1 || self.view.path().is_some()
    }

    fn layout(&self) -> Layout {
        let (w, h) = (self.size.0 as i32, self.size.1 as i32);
        let pad_y = (theme::PADDING_Y * self.scale).round() as i32;
        let status_h = self.ui_line_height() as i32 + pad_y * 2;
        let status = Rect::new(0, (h - status_h).max(0), w, status_h.min(h));
        let nag_h = if self.nag.is_some() {
            (self.ui_line_height() as i32 + pad_y * 3)
                .min(h - status_h)
                .max(0)
        } else {
            0
        };
        let nag = (nag_h > 0).then(|| Rect::new(0, 0, w, nag_h));
        let top = nag_h;
        let body_h = (h - status_h - top).max(0);
        let (tree, divider, col) = if self.tree.is_shown() && w > 200 {
            let tw = (self.tree.width * self.scale)
                .clamp(
                    TREE_MIN_W * self.scale,
                    (w as f32 * 0.6).max(TREE_MIN_W * self.scale),
                )
                .min(w as f32 - 100.0)
                .round() as i32;
            let div = (self.scale.round() as i32).max(1);
            (
                Some(Rect::new(0, top, tw, body_h)),
                Some(Rect::new(tw, top, div, body_h)),
                Rect::new(tw + div, top, (w - tw - div).max(0), body_h),
            )
        } else {
            (None, None, Rect::new(0, top, w, body_h))
        };
        let (tabs, main) = if self.show_tab_bar() {
            let th = self.tab_bar_height().min(col.h);
            (
                Some(Rect::new(col.x, col.y, col.w, th)),
                Rect::new(col.x, col.y + th, col.w, (col.h - th).max(0)),
            )
        } else {
            (None, col)
        };
        Layout {
            nag,
            tabs,
            tree,
            divider,
            main,
            status,
        }
    }

    /// Rectangles of each tab and its close button in the tab bar.
    fn tab_rects(&self, bar: Rect) -> Vec<(Rect, Rect)> {
        let n = self.tabs.len().max(1) as i32;
        let max_w = (theme::TAB_WIDTH * self.scale * self.zoom * theme::ui_ratio()) as i32;
        let min_w = (90.0 * self.scale) as i32;
        let w = (bar.w / n).clamp(min_w.min(bar.w), max_w.max(1));
        let cs = (bar.h as f32 * 0.5) as i32;
        (0..self.tabs.len() as i32)
            .map(|i| {
                let r = Rect::new(bar.x + i * w, bar.y, w, bar.h);
                let close = Rect::new(
                    r.right() - cs - (8.0 * self.scale) as i32,
                    r.y + (r.h - cs) / 2,
                    cs,
                    cs,
                );
                (r, close)
            })
            .collect()
    }

    fn near_divider(&self, x: f32) -> bool {
        let grab = (4.0 * self.scale).max(3.0);
        self.layout()
            .divider
            .is_some_and(|d| (x - d.x as f32).abs() <= grab)
    }

    /// Handle one input event. Returns true if the window should redraw.
    pub fn handle(&mut self, input: Input) -> bool {
        let redraw = self.handle_inner(&input);
        // Requests the app handles itself (opening a file from a link or the tree).
        let mut open = None;
        self.requests.retain(|r| match r {
            Request::OpenPath(p) => {
                open = Some((p.clone(), None));
                false
            }
            Request::OpenPathAt(p, line) => {
                open = Some((p.clone(), Some(*line)));
                false
            }
            _ => true,
        });
        if let Some((p, line)) = open {
            self.open(&p);
            if let Some(l) = line {
                self.view.goto_line(l);
            }
            return true;
        }
        if redraw {
            self.pending = true;
        }
        redraw
    }

    fn handle_inner(&mut self, input: &Input) -> bool {
        if matches!(input, Input::Key { .. } | Input::MouseDown { .. }) {
            self.notice = None;
        }
        // Right-click menu.
        if self.nag.is_none()
            && self.palette.is_none()
            && self.menu.is_some()
            && matches!(
                input,
                Input::Key { .. } | Input::MouseDown { .. } | Input::MouseMove { .. }
            )
        {
            match self.menu.as_mut().map(|m| m.handle(input)) {
                Some(MenuResult::Run(a)) => {
                    self.menu = None;
                    self.run_menu(a);
                }
                Some(MenuResult::Close) => self.menu = None,
                _ => {}
            }
            return true;
        }
        // The palette is modal (below the question bar).
        if self.nag.is_none()
            && self.palette.is_some()
            && matches!(
                input,
                Input::Key { .. } | Input::MouseDown { .. } | Input::Text(_)
            )
        {
            let outcome = {
                let mut ctx = ctx!(self);
                self.palette
                    .as_mut()
                    .and_then(|p| p.handle(input, &mut ctx))
            };
            if let Some(o) = outcome {
                self.palette = None;
                self.run_palette(o);
            }
            return true;
        }
        // The question bar is modal.
        if self.nag.is_some()
            && matches!(
                input,
                Input::Key { .. } | Input::MouseDown { .. } | Input::Text(_)
            )
        {
            if let Some(action) = self.nag.as_mut().and_then(|n| n.handle(input)) {
                self.apply_nag(action);
            }
            return true;
        }
        match input {
            Input::CloseRequested => {
                self.guard(Pending::Quit);
                return true;
            }
            Input::Resize(w, h) => {
                self.size = ((*w).max(1), (*h).max(1));
                self.pending = true;
                return true;
            }
            Input::Scale(s) => {
                if (s - self.scale).abs() > f32::EPSILON {
                    self.scale = *s;
                    self.view.metrics_changed();
                    self.pending = true;
                }
                return true;
            }
            Input::Drop(p) => {
                let p = p.clone();
                self.open(&p);
                return true;
            }
            Input::FsChanged(paths) => {
                self.file_index = None;
                let mut changed = self.tree.invalidate(paths);
                if let Some(p) = self.view.path().map(Path::to_path_buf)
                    && paths.iter().any(|c| c == &p)
                {
                    let name = self.view_name();
                    match self.view.disk_state() {
                        // Our own save, or a no-op touch.
                        DiskState::Same => {}
                        DiskState::Missing => {
                            self.view.mark_orphaned();
                            self.notice =
                                Some(format!("{name} was deleted on disk; Ctrl+S saves it again"));
                            changed = true;
                        }
                        DiskState::Differs if self.view.is_dirty() => {
                            if self.nag.is_none() {
                                self.nag = Some(Nag::changed_on_disk(&name));
                            }
                            changed = true;
                        }
                        DiskState::Differs => {
                            let mut ctx = ctx!(self);
                            changed |= self.view.reload(&mut ctx);
                        }
                    }
                }
                // Background tabs: reload clean ones, remember dirty ones.
                for i in 0..self.tabs.len() {
                    if i == self.active {
                        continue;
                    }
                    let id = self.tabs[i].id;
                    let Some(v) = self.tabs[i].view.as_mut() else {
                        continue;
                    };
                    let Some(p) = v.path().map(Path::to_path_buf) else {
                        continue;
                    };
                    if !paths.iter().any(|c| c == &p) {
                        continue;
                    }
                    match v.disk_state() {
                        DiskState::Same => {}
                        DiskState::Missing => v.mark_orphaned(),
                        DiskState::Differs if v.is_dirty() => {
                            if !self.disk_prompt.contains(&id) {
                                self.disk_prompt.push(id);
                            }
                        }
                        DiskState::Differs => {
                            let mut ctx = Ctx {
                                fonts: &mut self.fonts,
                                scale: self.scale,
                                zoom: self.zoom,
                                requests: &mut self.requests,
                                cursor: &mut self.cursor,
                                focused: false,
                                clipboard: &mut *self.clipboard,
                            };
                            v.reload(&mut ctx);
                        }
                    }
                    changed = true;
                }
                return changed;
            }
            Input::Key {
                key: Key::Tab,
                mods,
            } if mods.ctrl => {
                let n = self.tabs.len();
                let next = if mods.shift {
                    (self.active + n - 1) % n
                } else {
                    (self.active + 1) % n
                };
                self.activate(next);
                return true;
            }
            Input::Key {
                key: Key::Char(c),
                mods,
            } if mods.alt && !mods.ctrl && c.is_ascii_digit() && *c != '0' => {
                let i = (*c as usize - '1' as usize).min(self.tabs.len() - 1);
                self.activate(i);
                return true;
            }
            Input::Key { key, mods } if mods.ctrl => {
                let handled = match key {
                    Key::Char('p') if mods.shift => {
                        self.palette = Some(Palette::commands(self.view.path().is_some()));
                        true
                    }
                    Key::Char('p') => {
                        self.open_quick_open();
                        true
                    }
                    Key::Char('r') if !mods.shift => {
                        let items: Vec<PathBuf> =
                            self.recent.iter().filter(|p| p.exists()).cloned().collect();
                        self.palette = Some(Palette::recent(&items));
                        true
                    }
                    Key::Char('g') if !mods.shift => {
                        if let Some(n) = self.view.line_count() {
                            self.palette = Some(Palette::goto_line(n));
                        }
                        true
                    }
                    Key::Char('f') if mods.shift => {
                        match self.tree.root().map(Path::to_path_buf) {
                            Some(root) => {
                                self.palette = Some(Palette::text(
                                    "Find in folder",
                                    "",
                                    "Text to search for, then Enter",
                                ));
                                self.prompt_for = Some(MenuAction::FindInFolder(root));
                            }
                            None => self.notice = Some("Open a folder first (Ctrl+Shift+O)".into()),
                        }
                        true
                    }
                    Key::Char('s') if mods.shift => {
                        if let Some(p) = self.view.path() {
                            self.requests.push(Request::SaveAsDialog(p.to_path_buf()));
                        }
                        true
                    }
                    Key::Char('s') => self.save(),
                    Key::Char('w') => {
                        let i = self.active;
                        self.close_tab(i);
                        true
                    }
                    Key::PageDown if !mods.shift => {
                        let n = self.tabs.len();
                        self.activate((self.active + 1) % n);
                        true
                    }
                    Key::PageUp if !mods.shift => {
                        let n = self.tabs.len();
                        self.activate((self.active + n - 1) % n);
                        true
                    }
                    Key::Char('o') if mods.shift => {
                        self.requests.push(Request::OpenFolderDialog);
                        true
                    }
                    Key::Char('o') => {
                        self.requests.push(Request::OpenFileDialog);
                        true
                    }
                    Key::Char('\\') => {
                        self.tree.visible = !self.tree.visible;
                        if !self.tree.is_shown() {
                            self.focus = Focus::Main;
                        }
                        true
                    }
                    Key::Char('e') if mods.shift => {
                        // Focus the file tree (as in VS Code).
                        if self.tree.root().is_some() {
                            self.tree.visible = true;
                            self.focus = Focus::Tree;
                        }
                        true
                    }
                    Key::Char('h') if mods.shift => {
                        self.tree.toggle_hidden();
                        true
                    }
                    Key::Char('=') | Key::Char('+') => self.set_zoom(self.zoom * 1.1),
                    Key::Char('-') => self.set_zoom(self.zoom / 1.1),
                    Key::Char('0') => self.set_zoom(1.0),
                    Key::Char(',') if !mods.shift => {
                        self.open_settings();
                        true
                    }
                    _ => false,
                };
                if handled {
                    return true;
                }
            }
            Input::Key {
                key: Key::Escape, ..
            } if self.focus == Focus::Tree => {
                self.focus = Focus::Main;
                return true;
            }
            Input::Key {
                key: k @ (Key::F(2) | Key::Delete),
                mods,
            } if self.focus == Focus::Tree && !mods.ctrl && !mods.alt => {
                if let Some(row) = self.tree.selected_row()
                    && row.depth > 0
                {
                    let action = if *k == Key::Delete {
                        MenuAction::Delete(row.entry.path)
                    } else {
                        MenuAction::Rename(row.entry.path)
                    };
                    self.run_menu(action);
                }
                return true;
            }
            _ => {}
        }

        let layout = self.layout();
        // Tab bar: activate / close.
        if let Some(bar) = layout.tabs {
            let inside = |x: f32, y: f32| {
                x >= bar.x as f32
                    && x < bar.right() as f32
                    && y >= bar.y as f32
                    && y < bar.bottom() as f32
            };
            match input {
                Input::MouseDown { x, y, button, .. } if inside(*x, *y) => {
                    self.mouse = (*x, *y);
                    let hit = self
                        .tab_rects(bar)
                        .iter()
                        .position(|(r, _)| (*x as i32) >= r.x && (*x as i32) < r.right());
                    if let Some(i) = hit {
                        let (_, close) = self.tab_rects(bar)[i];
                        let on_close = (*x as i32) >= close.x && (*x as i32) < close.right();
                        match button {
                            MouseButton::Middle => self.close_tab(i),
                            MouseButton::Left if on_close => self.close_tab(i),
                            MouseButton::Left => {
                                self.activate(i);
                                self.focus = Focus::Main;
                            }
                            _ => {}
                        }
                    }
                    return true;
                }
                Input::MouseMove { x, y } if inside(*x, *y) => {
                    self.mouse = (*x, *y);
                    self.cursor = Cursor::Default;
                    return true; // hover highlight
                }
                _ => {}
            }
        }
        // Mouse: divider drag, then route by position.
        match input {
            Input::MouseMove { x, y } => {
                self.mouse = (*x, *y);
                if self.dragging_divider {
                    self.tree.width = (*x / self.scale).max(TREE_MIN_W);
                    self.cursor = Cursor::ResizeHorizontal;
                    return true;
                }
                if self.near_divider(*x) {
                    self.cursor = Cursor::ResizeHorizontal;
                    return false;
                }
            }
            Input::MouseDown {
                x,
                y,
                button: MouseButton::Left,
                ..
            } => {
                self.mouse = (*x, *y);
                if self.near_divider(*x) {
                    self.dragging_divider = true;
                    return false;
                }
            }
            Input::MouseUp { .. } if self.dragging_divider => {
                self.dragging_divider = false;
                return true;
            }
            _ => {}
        }

        // A drag that started in the tree gets the mouse until release.
        if self.tree.dragging() && matches!(input, Input::MouseMove { .. } | Input::MouseUp { .. })
        {
            let mut ctx = ctx!(self);
            let r = match layout.tree {
                Some(t) => self.tree.handle(input, t, &mut ctx),
                None => false,
            };
            if let Some((from, dir)) = self.tree.requested_move.take() {
                self.ask_move(from, dir);
                return true;
            }
            return r;
        }
        let in_tree = layout.tree.is_some_and(|t| self.mouse.0 < t.right() as f32);
        if let (
            Input::MouseDown {
                x,
                y,
                button: MouseButton::Right,
                ..
            },
            Some(t),
        ) = (input, layout.tree)
            && *x < t.right() as f32
        {
            self.open_tree_menu(*x, *y, t);
            return true;
        }
        let target = match input {
            Input::MouseMove { .. }
            | Input::MouseDown { .. }
            | Input::MouseUp { .. }
            | Input::Wheel { .. } => {
                if in_tree {
                    Focus::Tree
                } else {
                    Focus::Main
                }
            }
            Input::MouseLeave => {
                let mut ctx = ctx!(self);
                let a = layout
                    .tree
                    .is_some_and(|t| self.tree.handle(input, t, &mut ctx));
                let b = self.view.handle(input, layout.main, &mut ctx);
                return a || b;
            }
            _ => self.focus,
        };
        if matches!(input, Input::MouseDown { .. }) && self.focus != target {
            self.focus = target;
        }
        let mut ctx = ctx!(self);
        match (target, layout.tree) {
            (Focus::Tree, Some(t)) => {
                let r = self.tree.handle(input, t, &mut ctx);
                // Moving from the tree into the main view resets hover there, and vice versa.
                r || matches!(input, Input::MouseDown { .. })
            }
            _ => {
                if matches!(input, Input::MouseMove { .. }) && layout.tree.is_some() {
                    self.tree.handle(
                        &Input::MouseLeave,
                        layout.tree.unwrap_or(layout.main),
                        &mut ctx,
                    );
                }
                self.view.handle(input, layout.main, &mut ctx)
                    || matches!(input, Input::MouseDown { .. })
            }
        }
    }

    fn set_zoom(&mut self, z: f32) -> bool {
        let z = z.clamp(MIN_ZOOM, MAX_ZOOM);
        let z = if (z - 1.0).abs() < 0.03 { 1.0 } else { z };
        if (z - self.zoom).abs() > f32::EPSILON {
            self.zoom = z;
            self.view.metrics_changed();
            self.pending = true;
        }
        true
    }

    /// Run deferred work for at most `budget`.
    pub fn tick(&mut self, budget: Duration) -> Tick {
        let mut ctx = ctx!(self);
        let t = self.view.tick(&mut ctx, budget);
        self.pending = t.more;
        t
    }

    /// Finish all deferred work (scripts and screenshots), bounded by `limit`.
    pub fn settle(&mut self, limit: Duration) {
        let start = Instant::now();
        while self.tick(Duration::from_millis(50)).more && start.elapsed() < limit {
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    pub fn render(&mut self, canvas: &mut Canvas) {
        if (canvas.width() as u32, canvas.height() as u32) != self.size {
            self.size = (canvas.width().max(1) as u32, canvas.height().max(1) as u32);
        }
        let layout = self.layout();
        canvas.set_clip(canvas.bounds());
        canvas.fill_rect(canvas.bounds(), theme::p().background);

        let tab_info = layout.tabs.map(|bar| {
            let titles: Vec<(String, bool)> = (0..self.tabs.len())
                .map(|i| {
                    let v = self.tab_view(i);
                    let t = v.title();
                    (
                        if t.is_empty() {
                            "untitled".to_string()
                        } else {
                            t
                        },
                        v.is_dirty(),
                    )
                })
                .collect();
            (bar, self.tab_rects(bar), titles)
        });
        let active_tab = self.active;
        let mouse = self.mouse;
        let current = self.view.path().map(Path::to_path_buf);
        let focus = self.focus;
        let mut ctx = ctx!(self);

        if let (Some(r), Some(nag)) = (layout.nag, self.nag.as_mut()) {
            canvas.set_clip(r);
            nag.render(canvas, r, &mut ctx);
        }
        if let (Some(t), Some(d)) = (layout.tree, layout.divider) {
            canvas.set_clip(t);
            self.tree.render(
                canvas,
                t,
                &mut ctx,
                current.as_deref(),
                focus == Focus::Tree,
            );
            canvas.set_clip(d);
            canvas.fill_rect(d, theme::p().divider);
        }

        if let Some((bar, rects, titles)) = &tab_info {
            let bar = *bar;
            canvas.set_clip(bar);
            canvas.fill_rect(bar, theme::p().background2);
            let pad = ctx.px(theme::PADDING_X) as i32;
            let lh = ctx.ui_metrics().line_height as i32;
            for (i, ((r, close), (title, dirty))) in rects.iter().zip(titles.iter()).enumerate() {
                // Unsaved changes: a dot in the close button's place (the
                // close X shows while the mouse is over the tab).
                let title = title.strip_prefix("● ").unwrap_or(title);
                let active = i == active_tab;
                let hovered = (mouse.0 as i32) >= r.x
                    && (mouse.0 as i32) < r.right()
                    && (mouse.1 as i32) >= r.y
                    && (mouse.1 as i32) < r.bottom();
                if active {
                    canvas.fill_rect(*r, theme::p().background);
                } else if hovered {
                    canvas.fill_rect(*r, theme::p().line_highlight);
                }
                canvas.fill_rect(
                    Rect::new(
                        r.right() - 1,
                        r.y + ctx.px(6.0) as i32,
                        1,
                        r.h - ctx.px(12.0) as i32,
                    ),
                    theme::p().divider,
                );
                let color = if active {
                    theme::p().accent
                } else {
                    theme::p().text
                };
                let text_w = (close.x - r.x - pad - ctx.px(4.0) as i32).max(0);
                ctx.label(
                    canvas,
                    title,
                    r.x + pad,
                    r.y + (r.h - lh) / 2,
                    color,
                    text_w,
                    FontKind::Ui,
                );
                if *dirty && !hovered {
                    canvas.fill_circle(
                        close.x as f32 + close.w as f32 / 2.0,
                        close.y as f32 + close.h as f32 / 2.0,
                        ctx.px(4.0),
                        if active {
                            theme::p().accent
                        } else {
                            theme::p().text
                        },
                    );
                } else if active || hovered {
                    let (cx, cy, s) = (
                        close.x as f32 + close.w as f32 / 2.0,
                        close.y as f32 + close.h as f32 / 2.0,
                        close.w as f32 * 0.28,
                    );
                    let lw = ctx.px(1.5).max(1.0);
                    canvas.draw_line(cx - s, cy - s, cx + s, cy + s, lw, theme::p().text);
                    canvas.draw_line(cx - s, cy + s, cx + s, cy - s, lw, theme::p().text);
                }
            }
        }

        canvas.set_clip(layout.main);
        self.view.render(canvas, layout.main, &mut ctx);
        if let Some(p) = self.palette.as_mut() {
            canvas.set_clip(layout.main);
            p.render(canvas, layout.main, &mut ctx);
        }
        if let Some(m) = self.menu.as_mut() {
            let all = canvas.bounds();
            canvas.set_clip(all);
            m.render(canvas, all, &mut ctx);
        }

        // Status bar (lite-xl StatusView).
        canvas.set_clip(layout.status);
        canvas.fill_rect(layout.status, theme::p().background2);
        canvas.fill_rect(
            Rect::new(layout.status.x, layout.status.y, layout.status.w, 1),
            theme::p().divider,
        );
        let pad_x = ctx.px(theme::PADDING_X) as i32;
        let pad_y = ctx.px(theme::PADDING_Y) as i32;
        let mut x = layout.status.x + pad_x;
        let y = layout.status.y + pad_y;
        let mut items = self.view.status();
        if let Some(n) = &self.notice {
            items.push(n.clone());
        }
        if let Some(n) = self.tree.truncated_note() {
            items.push(n);
        }
        for (i, item) in items.iter().enumerate() {
            let color = if i == 0 {
                theme::p().accent
            } else {
                theme::p().text
            };
            let w = ctx.text_width(item, FontKind::Ui) as i32;
            ctx.label(
                canvas,
                item,
                x,
                y,
                color,
                layout.status.right() - x,
                FontKind::Ui,
            );
            x += w + pad_x * 2;
        }
        if (self.zoom - 1.0).abs() > f32::EPSILON {
            let z = format!("{:.0}%", self.zoom * 100.0);
            let w = ctx.text_width(&z, FontKind::Ui) as i32;
            ctx.label(
                canvas,
                &z,
                layout.status.right() - pad_x - w,
                y,
                theme::p().text,
                w + 2,
                FontKind::Ui,
            );
        }
        canvas.set_clip(canvas.bounds());
    }
}

fn display_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.display().to_string())
}

fn human_size(n: usize) -> String {
    if n < 1024 {
        format!("{n} bytes")
    } else if n < 1024 * 1024 {
        format!("{:.1} KB", n as f32 / 1024.0)
    } else {
        format!("{:.1} MB", n as f32 / (1024.0 * 1024.0))
    }
}

/// Resolve a command-line path without `canonicalize` (which yields
/// `\\?\C:\...` on Windows).
pub fn resolve_arg_path(arg: &str) -> PathBuf {
    let p = PathBuf::from(arg);
    if p.is_absolute() {
        p
    } else {
        std::env::current_dir().map(|d| d.join(&p)).unwrap_or(p)
    }
}
