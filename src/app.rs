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
use crate::view::diff::{DiffView, Side};
use crate::view::markdown::{MarkdownView, is_markdown};
use crate::view::message::MessageView;
use crate::view::search::SearchView;
use crate::view::{DiskState, Tick, View};
use crate::workspace::{self, Workspace};

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
    file_index: Option<(Vec<PathBuf>, Vec<PathBuf>)>,
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
    /// That save had already been confirmed over an external change.
    lossy_overwrite: bool,
    /// The tab a showing question is about (answers apply to it).
    nag_tab: Option<u64>,
    /// One-shot message in the status bar (e.g. "Saved").
    notice: Option<String>,
    /// Set when a view may have deferred work; cleared when `tick` finishes it.
    pending: bool,
    /// The settings file (window mode only; headless runs use defaults).
    settings_path: Option<PathBuf>,
    /// Soft wrap for newly opened code files (setting `wrap_code`).
    wrap_code: bool,
    /// Setting `trim_trailing_whitespace`.
    trim_on_save: bool,
    /// Extra ignored names currently in effect (setting `ignore`).
    ignore: Vec<String>,
    /// Tab bar scroll offset in pixels per group (when tabs don't fit).
    tab_scroll: [i32; 2],
    /// The tab last scrolled into view per group (so wheel scrolling isn't
    /// undone).
    tab_revealed: [Option<u64>; 2],
    /// The tab each group shows (the focused group's is the active tab).
    group_shown: [Option<u64>; 2],
    /// Left button pressed on a tab: (index, start x, dragging).
    tab_drag: Option<(usize, f32, bool)>,
    /// Recently closed files, most recent last (Ctrl+Shift+T).
    closed: Vec<PathBuf>,
    /// Modifier keys held (from the window).
    mods: crate::input::Mods,
    /// File chosen with "Select for Compare".
    compare_with: Option<PathBuf>,
    /// Where per-folder workspaces are kept (window mode; None = off).
    workspace_store: Option<PathBuf>,
}

struct TabSlot {
    id: u64,
    view: Option<Box<dyn View>>,
    /// Editor group: 0 = left (or only), 1 = right (split view).
    group: u8,
    /// Its rebuildable caches were freed since it was last on screen.
    trimmed: bool,
    /// A "peek" tab (single click in the tree): the next peek replaces it
    /// unless it was kept (double-click, edit, explicit open).
    peek: bool,
}

/// The editor group that doesn't have focus (split view).
#[derive(Clone, Copy)]
struct OtherPane {
    group: u8,
    bar: Rect,
    main: Rect,
}

struct Layout {
    nag: Option<Rect>,
    /// Tab bar and editor area of the focused group.
    tabs: Option<Rect>,
    /// The other group, when the view is split.
    other: Option<OtherPane>,
    /// The line between the two groups.
    split_line: Option<Rect>,
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
            mods: $s.mods,
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
            tabs: vec![TabSlot {
                id: 1,
                view: None,
                group: 0,
                trimmed: false,
                peek: false,
            }],
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
            lossy_overwrite: false,
            nag_tab: None,
            notice: None,
            pending: false,
            settings_path: None,
            wrap_code: false,
            trim_on_save: false,
            ignore: Vec::new(),
            tab_scroll: [0, 0],
            tab_revealed: [None, None],
            group_shown: [Some(1), None],
            tab_drag: None,
            closed: Vec::new(),
            compare_with: None,
            workspace_store: None,
            mods: crate::input::Mods::default(),
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
        self.trim_on_save = s.trim_trailing_whitespace;
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
            // Only plain UTF-8 is rewritten; anything else would be damaged.
            Ok(_) => match std::fs::read(&path) {
                Ok(b) => match String::from_utf8(b) {
                    Ok(t) => Some(t),
                    Err(_) => {
                        self.notice = Some(format!(
                            "{name} theme for now; settings.txt isn't UTF-8, so it was not changed"
                        ));
                        return;
                    }
                },
                Err(_) => None,
            },
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

    /// Tab bar rect and each tab's rect (tests).
    #[cfg(test)]
    pub fn tab_layout(&mut self) -> Option<(Rect, Vec<Rect>)> {
        let bar = self.layout().tabs?;
        self.reveal_active_tab(bar);
        Some((
            bar,
            self.tab_rects(bar).into_iter().map(|(_, r, _)| r).collect(),
        ))
    }

    /// (title, dirty) of every tab in order (tests).
    #[cfg(test)]
    pub fn tab_titles(&self) -> Vec<(String, bool)> {
        (0..self.tabs.len())
            .map(|i| {
                let v = self.tab_view(i);
                (v.title().trim_start_matches("● ").to_string(), v.is_dirty())
            })
            .collect()
    }

    /// Centre of a right-click menu item, after the menu was drawn (tests).
    #[cfg(test)]
    pub fn menu_item_pos(&self, label: &str) -> Option<(f32, f32)> {
        let r = self.menu.as_ref()?.item_rect(label)?;
        Some((r.x as f32 + r.w as f32 / 2.0, r.y as f32 + r.h as f32 / 2.0))
    }

    #[cfg(test)]
    pub fn preview_checkboxes(&self) -> Vec<(f32, f32)> {
        self.view.checkboxes()
    }

    #[cfg(test)]
    pub fn view_scroll_y(&self) -> f32 {
        self.view.scroll_y()
    }

    /// Which tabs are peek tabs (tests).
    #[cfg(test)]
    pub fn peek_tabs(&self) -> Vec<bool> {
        self.tabs.iter().map(|t| t.peek).collect()
    }

    #[cfg(test)]
    pub fn tree_roots(&self) -> Vec<PathBuf> {
        self.tree.roots().to_vec()
    }

    /// Window y of the middle of the tree row showing `p` (tests).
    #[cfg(test)]
    pub fn tree_row_y(&mut self, p: &Path) -> Option<f32> {
        let t = self.layout().tree?;
        let ctx = ctx!(self);
        let rh = (ctx.ui_metrics().line_height + ctx.px(6.0)).round();
        let top = t.y as f32 + ctx.px(theme::PADDING_Y);
        let i = self.tree.rows().iter().position(|r| r.entry.path == p)?;
        Some(top + i as f32 * rh + rh / 2.0)
    }

    #[cfg(test)]
    pub fn menu_labels(&self) -> Vec<String> {
        self.menu.as_ref().map(|m| m.labels()).unwrap_or_default()
    }

    #[cfg(test)]
    pub fn is_split_for_test(&self) -> bool {
        self.is_split()
    }

    #[cfg(test)]
    pub fn active_tab(&self) -> usize {
        self.active
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
            self.open_folder(path);
            return;
        }
        if workspace::is_workspace_file(path) {
            match Workspace::load(path) {
                Ok(w) => self.apply_workspace(w),
                Err(e) => self.nag = Some(Nag::error(format!("Cannot open workspace: {e}"))),
            }
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
        self.group_shown[self.tabs[i].group as usize] = Some(self.tabs[i].id);
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
                // This question belongs to this tab, whatever happens next.
                self.nag_tab = Some(id);
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
                group: self.focused_group(),
                trimmed: false,
                peek: false,
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
        if let Some(p) = self.tab_view(i).path().map(Path::to_path_buf)
            && !self.tabs[i].peek
        {
            self.closed.retain(|c| c != &p);
            self.closed.push(p);
            if self.closed.len() > 30 {
                self.closed.remove(0);
            }
        }
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
            // Prefer the neighbour on the same side (split view).
            let g = self.tabs[i].group;
            let same: Vec<usize> = self.group_tabs(g).into_iter().filter(|&j| j != i).collect();
            let next = same
                .iter()
                .copied()
                .find(|&j| j > i)
                .or_else(|| same.last().copied())
                .or_else(|| self.shown_tab(1 - g))
                .unwrap_or(if i + 1 < self.tabs.len() {
                    i + 1
                } else {
                    i - 1
                });
            self.activate(next);
        }
        // `activate` moved the old active view into its slot.
        self.tabs.remove(i);
        if self.active > i {
            self.active -= 1;
        }
        self.normalize_groups();
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
            Pending::CloseTabs(ids) => {
                let ids: Vec<u64> = ids
                    .into_iter()
                    .filter(|id| self.tab_index(*id).is_some())
                    .collect();
                // Clean ones go at once.
                for &id in &ids {
                    if let Some(i) = self.tab_index(id)
                        && !self.tab_view(i).is_dirty()
                    {
                        self.remove_tab(id);
                    }
                }
                let dirty: Vec<u64> = ids
                    .into_iter()
                    .filter(|id| {
                        self.tab_index(*id)
                            .is_some_and(|i| self.tab_view(i).is_dirty())
                    })
                    .collect();
                if let Some(&first) = dirty.first()
                    && let Some(i) = self.tab_index(first)
                {
                    self.activate(i);
                    let name = self.view_name();
                    self.nag = Some(Nag::unsaved(&name, Pending::CloseTabs(dirty)));
                }
            }
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
        self.save_checked(allow_lossy, then, false)
    }

    /// Save the active tab. Unless `overwrite`, first check that nobody else
    /// changed the file since it was opened or last saved.
    fn save_checked(&mut self, allow_lossy: bool, then: Option<Pending>, overwrite: bool) -> bool {
        if self.view.path().is_none() {
            return false;
        }
        let name = self.view_name();
        // Setting `trim_trailing_whitespace` (off by default, like lite-xl);
        // never for markdown, where two trailing spaces mean a line break.
        if self.trim_on_save && self.view.is_dirty() && !self.view.path().is_some_and(is_markdown) {
            self.view.run_command("trim-whitespace");
        }
        if !overwrite && self.view.disk_state() == DiskState::Differs {
            self.nag = Some(Nag::new(
                format!("{name} was changed by another program since you opened it."),
                vec![
                    ("Compare", NagAction::CompareWithDisk),
                    ("Save mine over it", NagAction::Overwrite(then)),
                    ("Cancel", NagAction::Dismiss),
                ],
                false,
            ));
            return true;
        }
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
                self.lossy_overwrite = overwrite;
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
        self.save_as_with(path, false);
    }

    fn save_as_with(&mut self, path: &Path, allow_lossy: bool) {
        match self.view.save_as(path, allow_lossy) {
            Ok(n) => {
                self.notice = Some(format!("Saved as {} ({})", path.display(), human_size(n)));
                self.tree.invalidate(&[path.to_path_buf()]);
                self.tree.reveal(path);
                self.after_save(path);
                // Another tab showing the old contents of that file: close it
                // if it has no edits, so one file isn't open twice.
                let active_id = self.tabs[self.active].id;
                let others: Vec<(u64, bool)> = (0..self.tabs.len())
                    .filter(|&i| self.tabs[i].id != active_id)
                    .filter(|&i| self.tab_view(i).path() == Some(path))
                    .map(|i| (self.tabs[i].id, self.tab_view(i).is_dirty()))
                    .collect();
                for (id, dirty) in others {
                    if dirty {
                        self.notice = Some(format!(
                            "Another tab still has unsaved edits for {}",
                            display_name(path)
                        ));
                    } else {
                        self.remove_tab(id);
                    }
                }
            }
            Err(SaveError::Failed(e)) => {
                self.nag = Some(Nag::error(format!(
                    "Could not save {}: {e}",
                    path.display()
                )));
            }
            Err(SaveError::NeedsLossyConfirm) => {
                self.nag = Some(Nag::new(
                    format!(
                        "{} had invalid characters (shown as �). Saving writes them as �.",
                        self.view_name()
                    ),
                    vec![
                        ("Save anyway", NagAction::SaveAsAnyway(path.to_path_buf())),
                        ("Cancel", NagAction::Dismiss),
                    ],
                    false,
                ));
            }
        }
        self.pending = true;
    }

    fn apply_nag(&mut self, action: NagAction) {
        self.nag = None;
        // Answer for the tab the question was about, even if another tab
        // came to the front meanwhile.
        if let Some(id) = self.nag_tab.take() {
            match self.tab_index(id) {
                Some(i) if i != self.active => self.activate(i),
                Some(_) => {}
                None if !matches!(action, NagAction::Dismiss | NagAction::KeepMine) => {
                    self.notice = Some("That tab is gone; nothing was done".into());
                    return;
                }
                None => {}
            }
            // `activate` may have raised that tab's own pending question.
            if self.nag.is_some() {
                return;
            }
        }
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
            NagAction::DiscardThen(Pending::CloseTabs(ids)) => {
                let id = self.tabs[self.active].id;
                self.remove_tab(id);
                self.perform(Pending::CloseTabs(ids));
            }
            NagAction::DiscardThen(p) => self.perform(p),
            NagAction::SaveAnyway => {
                let then = self.after_lossy.take();
                let overwrite = std::mem::take(&mut self.lossy_overwrite);
                self.save_checked(true, then, overwrite);
            }
            NagAction::SaveAsAnyway(path) => self.save_as_with(&path, true),
            NagAction::Overwrite(then) => {
                self.save_checked(false, then, true);
            }
            NagAction::CompareWithDisk => {
                let id = self.tabs[self.active].id;
                self.compare_with_saved(id);
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
        let roots = self.tree.roots().to_vec();
        if roots.is_empty() {
            self.notice = Some("Open a folder first (Ctrl+Shift+O)".into());
            return;
        }
        let files = match &self.file_index {
            Some((r, f)) if *r == roots => f.clone(),
            _ => {
                let mut all = Vec::new();
                for r in &roots {
                    all.extend(index_files(r));
                    if all.len() >= crate::palette::MAX_INDEX_FILES {
                        all.truncate(crate::palette::MAX_INDEX_FILES);
                        break;
                    }
                }
                self.file_index = Some((roots.clone(), all.clone()));
                all
            }
        };
        self.palette = Some(Palette::files(&roots, &files));
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
            Outcome::Run(Action::SaveWorkspace) => {
                let p = self.suggested_workspace_path();
                if self.tree.root().is_none() {
                    self.notice = Some(
                        "Open a folder first: a workspace is a folder plus its open files".into(),
                    );
                } else {
                    self.requests.push(Request::SaveWorkspaceDialog(p));
                }
            }
            Outcome::Run(Action::OpenWorkspace) => self.requests.push(Request::OpenWorkspaceDialog),
            Outcome::Run(Action::AddFolder) => self.requests.push(Request::AddFolderDialog),
            Outcome::Run(Action::Heading(line)) => {
                self.view.goto_heading(line);
                self.focus = Focus::Main;
            }
            Outcome::Run(Action::Command(c)) => {
                if self.view.run_command(c) {
                    self.pending = true;
                } else {
                    self.notice = Some("That command works in a text file".into());
                }
            }
            Outcome::Run(Action::CompareWithSaved) => {
                let id = self.tabs[self.active].id;
                self.compare_with_saved(id);
            }
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
        // Never replace a question that is already showing.
        if self.nag.is_some() {
            self.notice = Some("Answer the question above first, then move it again".into());
            return;
        }
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
        if is_root {
            items.push(("Add Folder to Workspace…", MenuAction::AddFolder));
        }
        if !is_root {
            items.push(("Rename…", MenuAction::Rename(path.clone())));
            items.push(("Delete (to Recycle Bin)", MenuAction::Delete(path.clone())));
        }
        if is_root && self.tree.roots().len() > 1 {
            items.push((
                "Remove Folder from Workspace",
                MenuAction::RemoveFolder(path.clone()),
            ));
        }
        if is_dir {
            items.push(("Find in folder…", MenuAction::FindInFolder(path.clone())));
        } else {
            items.extend(self.compare_items(&path));
        }
        items.push(("Show in Explorer", MenuAction::Reveal(path.clone())));
        items.push(("Copy path", MenuAction::CopyPath(path)));
        self.menu = Some(Menu::new(x, y, items));
    }

    fn run_menu(&mut self, a: MenuAction) {
        let ids = |s: &Self, keep: &dyn Fn(usize, u64) -> bool| -> Vec<u64> {
            s.tabs
                .iter()
                .enumerate()
                .filter(|(i, t)| keep(*i, t.id))
                .map(|(_, t)| t.id)
                .collect()
        };
        match &a {
            MenuAction::KeepTab(id) => {
                if let Some(i) = self.tab_index(*id) {
                    self.keep_tab(i);
                }
                return;
            }
            MenuAction::CloseTab(id) => {
                if let Some(i) = self.tab_index(*id) {
                    self.close_tab(i);
                }
                return;
            }
            // These act on one side of a split: the clicked tab's side.
            MenuAction::CloseOthers(id) => {
                let g = self.tab_index(*id).map_or(0, |i| self.tabs[i].group);
                let list = ids(self, &|i, t| t != *id && self.tabs[i].group == g);
                self.perform(Pending::CloseTabs(list));
                return;
            }
            MenuAction::CloseToRight(id) => {
                let at = self.tab_index(*id).unwrap_or(usize::MAX);
                let g = self.tabs.get(at).map_or(0, |t| t.group);
                let list = ids(self, &|i, _| i > at && self.tabs[i].group == g);
                self.perform(Pending::CloseTabs(list));
                return;
            }
            MenuAction::CloseSaved => {
                let g = self.focused_group();
                let clean: Vec<u64> = (0..self.tabs.len())
                    .filter(|&i| self.tabs[i].group == g && !self.tab_view(i).is_dirty())
                    .map(|i| self.tabs[i].id)
                    .collect();
                self.perform(Pending::CloseTabs(clean));
                return;
            }
            MenuAction::CloseAll => {
                let g = self.focused_group();
                let list = ids(self, &|i, _| self.tabs[i].group == g);
                self.perform(Pending::CloseTabs(list));
                return;
            }
            MenuAction::Keys(combo) => {
                if let Some((key, mods)) = parse_key_combo(combo) {
                    self.handle(Input::Key { key, mods });
                }
                return;
            }
            MenuAction::RemoveFolder(p) => {
                let p = p.clone();
                self.remove_folder(&p);
                return;
            }
            MenuAction::AddFolder => {
                self.requests.push(Request::AddFolderDialog);
                return;
            }
            MenuAction::SelectForCompare(p) => {
                self.compare_with = Some(p.clone());
                self.notice = Some(format!(
                    "Selected {} — right-click another file and choose Compare with Selected",
                    display_name(p)
                ));
                return;
            }
            MenuAction::Compare(old, new) => {
                let (old, new) = (old.clone(), new.clone());
                self.compare_files(&old, &new);
                return;
            }
            MenuAction::CompareWithSaved(id) => {
                self.compare_with_saved(*id);
                return;
            }
            MenuAction::RevealInTree(p) => {
                self.tree.visible = true;
                self.tree.reveal(p);
                self.focus = Focus::Tree;
                return;
            }
            _ => {}
        }
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
            // Handled above.
            _ => return,
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
        let ws = self.current_workspace();
        Session {
            root: self.tree.root().map(Path::to_path_buf),
            folders: self.tree.roots().iter().skip(1).cloned().collect(),
            tabs: ws.tabs.iter().map(|(p, _)| p.clone()).collect(),
            groups: ws.tabs.iter().map(|(_, g)| *g).collect(),
            active: ws.active,
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
            for f in s.folders.iter().filter(|f| f.is_dir()) {
                self.tree.add_root(f);
            }
            self.view = Box::new(MessageView::folder_opened());
        }
        let tabs: Vec<(PathBuf, u8)> = s
            .tabs
            .iter()
            .enumerate()
            .map(|(i, t)| (t.clone(), s.groups.get(i).copied().unwrap_or(0)))
            .collect();
        self.open_workspace_tabs(&tabs, s.active);
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

    /// Button labels of the open question (tests).
    #[cfg(test)]
    pub fn nag_options(&self) -> Vec<String> {
        self.nag
            .as_ref()
            .map(|n| n.options.iter().map(|(l, _)| l.clone()).collect())
            .unwrap_or_default()
    }

    /// Full text of the active view (tests).
    #[cfg(test)]
    pub fn view_text(&self) -> String {
        self.view.text().unwrap_or_default()
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
            // Opening it on purpose keeps a peeked file open.
            self.tabs[i].peek = false;
            self.activate(i);
            self.focus = Focus::Main;
            return;
        }
        let view = self.make_view(path);
        self.add_tab(view);
        self.tree.reveal(path);
        self.focus = Focus::Main;
        self.pending = true;
    }

    fn make_view(&self, path: &Path) -> Box<dyn View> {
        match Document::open(path) {
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
        }
    }

    /// Single click in the tree: show `path` in the peek tab of the focused
    /// side, replacing the file peeked before. A peek tab that was edited
    /// or kept is an ordinary tab and is never replaced.
    pub fn open_peek(&mut self, path: &Path) {
        if path.is_dir() || workspace::is_workspace_file(path) {
            self.open(path);
            return;
        }
        if let Some(i) = self.find_tab(path) {
            self.activate(i);
            self.focus = Focus::Main;
            return;
        }
        let g = self.focused_group();
        let old = (0..self.tabs.len()).find(|&i| {
            self.tabs[i].peek && self.tabs[i].group == g && !self.tab_view(i).is_dirty()
        });
        let view = self.make_view(path);
        match old {
            // The start page: becomes the peek tab.
            None if self.view.path().is_none() && self.tabs.len() == 1 => {
                self.add_tab(view);
                self.tabs[0].peek = true;
            }
            None => {
                self.add_tab(view);
                self.tabs[self.active].peek = true;
            }
            // Take the old peek's place: add the new tab right after it
            // (so a side never becomes empty), then remove the old one.
            Some(i) => {
                let old_id = self.tabs[i].id;
                let id = self.next_tab_id;
                self.next_tab_id += 1;
                self.tabs.insert(
                    i + 1,
                    TabSlot {
                        id,
                        view: Some(view),
                        group: g,
                        trimmed: false,
                        peek: true,
                    },
                );
                if self.active > i {
                    self.active += 1;
                }
                self.activate(i + 1);
                self.remove_tab(old_id);
            }
        }
        self.tree.reveal(path);
        self.pending = true;
    }

    /// A peek tab becomes an ordinary tab (kept, edited, saved).
    fn keep_tab(&mut self, i: usize) {
        if let Some(t) = self.tabs.get_mut(i)
            && t.peek
        {
            t.peek = false;
            if let Some(p) = self.tab_view(i).path().map(Path::to_path_buf) {
                push_recent(&mut self.recent, &p);
            }
            self.session_dirty = true;
        }
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
        for r in self.tree.roots() {
            v.push((r.to_path_buf(), true));
        }
        for i in 0..self.tabs.len() {
            let view = self.tab_view(i);
            let files = view
                .path()
                .map(Path::to_path_buf)
                .into_iter()
                .chain(view.watched_paths());
            for f in files {
                if let Some(dir) = f.parent()
                    && self.tree.root_of(dir).is_none()
                    && !v.iter().any(|(d, _)| d == dir)
                {
                    v.push((dir.to_path_buf(), false));
                }
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

    // ---- editor groups (split view) ------------------------------------

    fn focused_group(&self) -> u8 {
        self.tabs.get(self.active).map_or(0, |t| t.group)
    }

    fn is_split(&self) -> bool {
        self.tabs.iter().any(|t| t.group == 1)
    }

    /// Global tab indices of group `g`, in order.
    fn group_tabs(&self, g: u8) -> Vec<usize> {
        (0..self.tabs.len())
            .filter(|&i| self.tabs[i].group == g)
            .collect()
    }

    /// The tab group `g` shows.
    fn shown_tab(&self, g: u8) -> Option<usize> {
        if g == self.focused_group() {
            return Some(self.active);
        }
        self.group_shown[g as usize]
            .and_then(|id| self.tab_index(id))
            .filter(|&i| self.tabs[i].group == g)
            .or_else(|| self.group_tabs(g).first().copied())
    }

    /// Keep group 0 non-empty: if it emptied, the right group becomes it.
    fn normalize_groups(&mut self) {
        if !self.tabs.iter().any(|t| t.group == 0) {
            for t in &mut self.tabs {
                t.group = 0;
            }
            self.group_shown = [self.tabs.get(self.active).map(|t| t.id), None];
            self.tab_scroll = [self.tab_scroll[1], 0];
            self.tab_revealed = [None, None];
        }
    }

    /// Move the active tab to the other side (Ctrl+Alt+Right / Left, tab
    /// menu "Move to Other Side"). The first move creates the split.
    fn move_active_to(&mut self, g: u8) -> bool {
        let cur = self.focused_group();
        if g == cur {
            return false;
        }
        if self.tabs.len() < 2 {
            self.notice = Some("Open another file to split the view".into());
            return false;
        }
        self.keep_tab(self.active);
        let id = self.tabs[self.active].id;
        // The group left behind shows its nearest other tab.
        let left_behind = self.group_tabs(cur);
        let pos = left_behind
            .iter()
            .position(|&i| i == self.active)
            .unwrap_or(0);
        let neighbour = left_behind
            .get(pos + 1)
            .or_else(|| pos.checked_sub(1).and_then(|p| left_behind.get(p)))
            .map(|&i| self.tabs[i].id);
        self.tabs[self.active].group = g;
        self.group_shown[cur as usize] = neighbour;
        self.group_shown[g as usize] = Some(id);
        self.tab_revealed = [None, None];
        self.normalize_groups();
        self.session_dirty = true;
        self.pending = true;
        true
    }

    /// Next / previous tab within the focused group (Ctrl+Tab, Ctrl+PgDn).
    fn cycle_tab(&mut self, forward: bool) {
        let group = self.group_tabs(self.focused_group());
        let n = group.len();
        if n < 2 {
            return;
        }
        let k = group.iter().position(|&i| i == self.active).unwrap_or(0);
        let next = if forward {
            (k + 1) % n
        } else {
            (k + n - 1) % n
        };
        self.activate(group[next]);
    }

    /// Focus group `g` (Ctrl+1 / Ctrl+2, or a click in it).
    fn focus_group(&mut self, g: u8) -> bool {
        if g == self.focused_group() {
            return false;
        }
        match self.shown_tab(g) {
            Some(i) => {
                self.activate(i);
                self.focus = Focus::Main;
                true
            }
            None => false,
        }
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
        // Split view: left half group 0, right half group 1.
        let (col, other, split_line) = if self.is_split() && col.w > 2 {
            let half = col.w / 2;
            let left = Rect::new(col.x, col.y, half, col.h);
            let right = Rect::new(col.x + half + 1, col.y, col.w - half - 1, col.h);
            let line = Rect::new(col.x + half, col.y, 1, col.h);
            let th = self.tab_bar_height().min(col.h);
            let (mine, theirs) = if self.focused_group() == 0 {
                (left, right)
            } else {
                (right, left)
            };
            let other = OtherPane {
                group: 1 - self.focused_group(),
                bar: Rect::new(theirs.x, theirs.y, theirs.w, th),
                main: Rect::new(theirs.x, theirs.y + th, theirs.w, (theirs.h - th).max(0)),
            };
            (mine, Some(other), Some(line))
        } else {
            (col, None, None)
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
            other,
            split_line,
            tree,
            divider,
            main,
            status,
        }
    }

    /// Rectangles of each tab and its close button in the tab bar.
    /// Tab width and the largest scroll offset. Tabs share the bar equally,
    /// shrink to a minimum when crowded, and then the bar scrolls.
    fn tab_metrics(&self, bar: Rect) -> (i32, i32) {
        self.tab_metrics_n(bar, self.group_tabs(self.focused_group()).len())
    }

    fn tab_metrics_n(&self, bar: Rect, n: usize) -> (i32, i32) {
        let n = n.max(1) as i32;
        let max_w = (theme::TAB_WIDTH * self.scale * self.zoom * theme::ui_ratio()) as i32;
        let min_w = (130.0 * self.scale * self.zoom * theme::ui_ratio()) as i32;
        let w = (bar.w / n).clamp(min_w.min(bar.w).max(1), max_w.max(1));
        (w, (n * w - bar.w).max(0))
    }

    /// (global tab index, tab rect, close-button rect) for the focused group.
    fn tab_rects(&self, bar: Rect) -> Vec<(usize, Rect, Rect)> {
        self.group_tab_rects(bar, self.focused_group())
    }

    fn group_tab_rects(&self, bar: Rect, g: u8) -> Vec<(usize, Rect, Rect)> {
        let idx = self.group_tabs(g);
        let (w, max_scroll) = self.tab_metrics_n(bar, idx.len());
        let scroll = self.tab_scroll[g as usize].clamp(0, max_scroll);
        let cs = (bar.h as f32 * 0.5) as i32;
        idx.into_iter()
            .enumerate()
            .map(|(k, i)| {
                let r = Rect::new(bar.x + k as i32 * w - scroll, bar.y, w, bar.h);
                let close = Rect::new(
                    r.right() - cs - (8.0 * self.scale) as i32,
                    r.y + (r.h - cs) / 2,
                    cs,
                    cs,
                );
                (i, r, close)
            })
            .collect()
    }

    /// Scroll the tab bar so the active tab is fully visible (once per
    /// activation, so wheel scrolling sticks).
    fn reveal_active_tab(&mut self, bar: Rect) {
        let g = self.focused_group() as usize;
        let (w, max_scroll) = self.tab_metrics(bar);
        let id = self.tabs.get(self.active).map(|t| t.id);
        self.tab_scroll[g] = self.tab_scroll[g].clamp(0, max_scroll);
        if self.tab_revealed[g] == id {
            return;
        }
        self.tab_revealed[g] = id;
        let k = self
            .group_tabs(g as u8)
            .iter()
            .position(|&i| i == self.active)
            .unwrap_or(0) as i32;
        let left = k * w;
        if left < self.tab_scroll[g] {
            self.tab_scroll[g] = left;
        } else if left + w > self.tab_scroll[g] + bar.w {
            self.tab_scroll[g] = (left + w - bar.w).min(max_scroll);
        }
    }

    /// Index of the tab under window x (visible part of the bar only).
    fn tab_at(&self, bar: Rect, x: f32) -> Option<usize> {
        if x < bar.x as f32 || x >= bar.right() as f32 {
            return None;
        }
        self.tab_rects(bar)
            .iter()
            .find(|(_, r, _)| (x as i32) >= r.x && (x as i32) < r.right())
            .map(|(i, _, _)| *i)
    }

    /// Move tab `from` to position `to` (drag reorder), keeping which tab
    /// is active.
    fn move_tab(&mut self, from: usize, to: usize) {
        if from >= self.tabs.len() || to >= self.tabs.len() || from == to {
            return;
        }
        let active_id = self.tabs[self.active].id;
        let slot = self.tabs.remove(from);
        self.tabs.insert(to, slot);
        self.active = self.tab_index(active_id).unwrap_or(0);
        self.tab_revealed = [None, None];
        self.session_dirty = true;
    }

    /// Right-click on a tab.
    fn open_tab_menu(&mut self, i: usize, x: f32, y: f32) {
        let id = self.tabs[i].id;
        let path = self.tab_view(i).path().map(Path::to_path_buf);
        let mut items = Vec::new();
        if self.tabs[i].peek {
            items.push(("Keep Open", MenuAction::KeepTab(id)));
        }
        items.extend([
            ("Close", MenuAction::CloseTab(id)),
            ("Close Others", MenuAction::CloseOthers(id)),
            ("Close to the Right", MenuAction::CloseToRight(id)),
            ("Close Saved", MenuAction::CloseSaved),
            ("Close All", MenuAction::CloseAll),
            (
                if self.tabs[i].group == 0 {
                    "Move to Right Side"
                } else {
                    "Move to Left Side"
                },
                MenuAction::Keys(if self.tabs[i].group == 0 {
                    "ctrl+alt+right"
                } else {
                    "ctrl+alt+left"
                }),
            ),
        ]);
        if let Some(p) = path {
            if self.tab_view(i).is_dirty() {
                items.push(("Compare with Saved", MenuAction::CompareWithSaved(id)));
            }
            items.extend(self.compare_items(&p));
            items.push(("Copy Path", MenuAction::CopyPath(p.clone())));
            items.push(("Reveal in File Tree", MenuAction::RevealInTree(p.clone())));
            items.push(("Show in Explorer", MenuAction::Reveal(p)));
        }
        self.menu = Some(Menu::new(x, y, items));
    }

    /// Menu entries for comparing `path` (tree and tab menus).
    fn compare_items(&self, path: &Path) -> Vec<(&'static str, MenuAction)> {
        let mut v = Vec::new();
        if let Some(sel) = &self.compare_with
            && sel != path
            && sel.is_file()
        {
            v.push((
                "Compare with Selected",
                MenuAction::Compare(sel.clone(), path.to_path_buf()),
            ));
        }
        v.push((
            "Select for Compare",
            MenuAction::SelectForCompare(path.to_path_buf()),
        ));
        v
    }

    /// Open a comparison tab (old on the left, new on the right).
    fn open_compare(&mut self, old: Result<Side, String>, new: Result<Side, String>) {
        let view = old.and_then(|o| new.and_then(|n| DiffView::new(o, n)));
        match view {
            Ok(v) => {
                self.add_tab(Box::new(v));
                self.focus = Focus::Main;
                self.pending = true;
            }
            Err(e) => self.nag = Some(Nag::error(format!("Cannot compare: {e}"))),
        }
    }

    pub fn compare_files(&mut self, old: &Path, new: &Path) {
        let label = |p: &Path| display_name(p);
        self.open_compare(
            Side::from_file(old, label(old)),
            Side::from_file(new, label(new)),
        );
    }

    /// The saved file (left) against the tab's unsaved text (right).
    fn compare_with_saved(&mut self, id: u64) {
        let Some(i) = self.tab_index(id) else { return };
        let v = self.tab_view(i);
        let (Some(path), Some(text)) = (v.path().map(Path::to_path_buf), v.text()) else {
            self.notice = Some("Only files can be compared with the saved copy".into());
            return;
        };
        let name = display_name(&path);
        let new = Side::from_text(&text, format!("{name} (unsaved)"), &path);
        self.open_compare(Side::from_file(&path, format!("{name} (saved)")), Ok(new));
    }

    /// Ctrl+N: name a new file in the current file's folder (or the tree's
    /// folder); with neither, the system's save dialog picks the place.
    fn new_file(&mut self) -> bool {
        let dir = self
            .view
            .path()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .or_else(|| self.tree.root().map(Path::to_path_buf));
        match dir {
            Some(d) => self.run_menu(MenuAction::NewFile(d)),
            None => self.requests.push(Request::NewFileDialog),
        }
        true
    }

    /// Create an empty file (never overwriting) and open it.
    pub fn create_and_open(&mut self, path: &Path) {
        let res = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path);
        match res {
            Ok(_) => {
                self.after_fs_change(path);
                self.open(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => self.open(path),
            Err(e) => {
                self.nag = Some(Nag::error(format!(
                    "Could not create {}: {e}",
                    path.display()
                )))
            }
        }
    }

    /// Right-click in a text view.
    fn open_editor_menu(&mut self, x: f32, y: f32) {
        let id = self.tabs[self.active].id;
        let mut items: Vec<(&'static str, MenuAction)> = vec![
            ("Cut", MenuAction::Keys("ctrl+x")),
            ("Copy", MenuAction::Keys("ctrl+c")),
            ("Paste", MenuAction::Keys("ctrl+v")),
            ("Select All", MenuAction::Keys("ctrl+a")),
            ("Find…", MenuAction::Keys("ctrl+f")),
            ("Toggle Comment", MenuAction::Keys("ctrl+/")),
        ];
        if self.view.is_dirty() {
            items.push(("Compare with Saved", MenuAction::CompareWithSaved(id)));
        }
        self.menu = Some(Menu::new(x, y, items));
    }

    // ---- workspaces ------------------------------------------------------

    /// Remember each folder's tabs in `dir` (the window uses
    /// `%APPDATA%\litedit\workspaces`; headless runs and tests don't).
    pub fn use_workspace_store(&mut self, dir: PathBuf) {
        self.workspace_store = Some(dir);
    }

    /// The folders, open files (with their side) and active file.
    pub fn current_workspace(&self) -> Workspace {
        let mut w = Workspace {
            folders: self.tree.roots().to_vec(),
            ..Workspace::default()
        };
        for i in 0..self.tabs.len() {
            if let Some(p) = self.tab_view(i).path() {
                if i == self.active {
                    w.active = w.tabs.len();
                }
                w.tabs.push((p.to_path_buf(), self.tabs[i].group));
            }
        }
        w
    }

    /// Save the current folder's workspace in the store (best effort).
    pub fn persist_workspace(&self) {
        if let (Some(store), Some(root)) = (&self.workspace_store, self.tree.root()) {
            let _ = self
                .current_workspace()
                .save(&workspace::folder_file(store, root));
        }
    }

    /// Open `tabs` (existing files only) on their sides; show `active`.
    fn open_workspace_tabs(&mut self, tabs: &[(PathBuf, u8)], active: usize) {
        for (p, _) in tabs.iter().filter(|(p, _)| p.is_file()) {
            self.open_now(p);
        }
        // Sides: only when both sides end up with tabs.
        let right: Vec<usize> = tabs
            .iter()
            .filter(|(p, g)| *g == 1 && p.is_file())
            .filter_map(|(p, _)| self.find_tab(p))
            .collect();
        if !right.is_empty() && right.len() < self.tabs.len() {
            for &i in &right {
                self.tabs[i].group = 1;
            }
            self.group_shown = [None, None];
            self.tab_revealed = [None, None];
            if let Some(&i) = right.last() {
                self.group_shown[1] = Some(self.tabs[i].id);
            }
            if let Some(&i) = self.group_tabs(0).first() {
                self.group_shown[0] = Some(self.tabs[i].id);
            }
        }
        if let Some((p, _)) = tabs.get(active).filter(|(p, _)| p.is_file())
            && let Some(i) = self.find_tab(p)
        {
            self.activate(i);
        }
    }

    /// Switch to workspace `w`: remember the current folder's tabs, close
    /// tabs that are saved and outside the new folder (unsaved ones stay —
    /// nothing is ever dropped), show the new folder and open its files.
    pub fn apply_workspace(&mut self, w: Workspace) {
        let Some(root) = w.folders.first().filter(|f| f.is_dir()).cloned() else {
            self.nag = Some(Nag::error(
                "The workspace's folder doesn't exist any more.".into(),
            ));
            return;
        };
        self.persist_workspace();
        let keep_dirty = (0..self.tabs.len())
            .filter(|&i| self.tab_view(i).is_dirty())
            .count();
        let close: Vec<u64> = (0..self.tabs.len())
            .filter(|&i| {
                let v = self.tab_view(i);
                !v.is_dirty()
                    && !v
                        .path()
                        .is_some_and(|p| w.folders.iter().any(|f| p.starts_with(f)))
            })
            .map(|i| self.tabs[i].id)
            .collect();
        for id in close {
            self.remove_tab(id);
        }
        for t in &mut self.tabs {
            t.group = 0;
        }
        self.group_shown = [self.tabs.get(self.active).map(|t| t.id), None];
        self.tree.set_root(&root);
        for f in w.folders.iter().skip(1).filter(|f| f.is_dir()) {
            self.tree.add_root(f);
        }
        self.tree.visible = true;
        self.open_workspace_tabs(&w.tabs, w.active);
        if self.view.path().is_none() && self.tabs.len() == 1 {
            self.view = Box::new(MessageView::folder_opened());
            self.focus = Focus::Tree;
        } else {
            self.focus = Focus::Main;
        }
        if keep_dirty > 0 {
            self.notice = Some(format!(
                "Kept {keep_dirty} tab(s) with unsaved edits from the previous folder"
            ));
        }
        self.session_dirty = true;
        self.file_index = None;
        self.pending = true;
    }

    /// Open a folder: its remembered workspace if there is one.
    fn open_folder(&mut self, dir: &Path) {
        if self.tree.roots().iter().any(|r| r == dir) {
            self.tree.visible = true;
            self.tree.reveal(dir);
            self.focus = Focus::Tree;
            return;
        }
        let w = self
            .workspace_store
            .as_ref()
            .and_then(|s| Workspace::load(&workspace::folder_file(s, dir)).ok())
            .filter(|w| w.folders.first().is_some_and(|f| f == dir))
            .unwrap_or_else(|| Workspace::for_folder(dir));
        self.apply_workspace(w);
    }

    /// "File: Add Folder to Workspace…": show another folder in the tree.
    pub fn add_folder(&mut self, dir: &Path) {
        if !dir.is_dir() {
            return;
        }
        if self.tree.root().is_none() {
            self.open_folder(dir);
            return;
        }
        if self.tree.add_root(dir) {
            push_recent(&mut self.recent, dir);
            self.file_index = None;
            self.session_dirty = true;
            self.tree.visible = true;
            self.notice = Some(format!("Added {} to the workspace", display_name(dir)));
        }
    }

    /// "Remove Folder from Workspace" (tree menu on an added folder).
    fn remove_folder(&mut self, dir: &Path) {
        if self.tree.remove_root(dir) {
            self.file_index = None;
            self.session_dirty = true;
            self.notice = Some(format!(
                "Removed {} from the workspace (nothing was deleted)",
                display_name(dir)
            ));
        }
    }

    /// "File: Save Workspace As…" (from the dialog or a script).
    pub fn save_workspace_as(&mut self, path: &Path) {
        let path = if workspace::is_workspace_file(path) {
            path.to_path_buf()
        } else {
            path.with_extension(workspace::EXT)
        };
        if self.tree.root().is_none() {
            self.notice =
                Some("Open a folder first: a workspace is a folder plus its open files".into());
            return;
        }
        match self.current_workspace().save(&path) {
            Ok(()) => {
                push_recent(&mut self.recent, &path);
                self.session_dirty = true;
                self.notice = Some(format!("Saved workspace {}", display_name(&path)));
                self.after_fs_change(&path);
            }
            Err(e) => self.nag = Some(Nag::error(format!("Could not save the workspace: {e}"))),
        }
    }

    fn suggested_workspace_path(&self) -> PathBuf {
        match self.tree.root() {
            Some(r) => r.join(format!(
                "{}.{}",
                r.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "workspace".into()),
                workspace::EXT
            )),
            None => PathBuf::from(format!("workspace.{}", workspace::EXT)),
        }
    }

    /// Ctrl+Shift+T: reopen the most recently closed file that still exists.
    fn reopen_closed_tab(&mut self) -> bool {
        while let Some(p) = self.closed.pop() {
            if p.is_file() && self.find_tab(&p).is_none() {
                self.open(&p);
                return true;
            }
        }
        self.notice = Some("No closed tabs to reopen".into());
        true
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
        self.sync_nag_owner();
        // Requests the app handles itself (opening a file from a link or the tree).
        // Editing a peeked file keeps it open.
        if self.tabs[self.active].peek && self.view.is_dirty() {
            self.keep_tab(self.active);
        }
        let mut open = None;
        let mut peek = None;
        self.requests.retain(|r| match r {
            Request::PeekPath(p) => {
                peek = Some(p.clone());
                false
            }
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
        if let Some(p) = peek.filter(|_| open.is_none()) {
            if self.nag.is_some() {
                // Don't switch tabs under an open question.
                return redraw;
            }
            self.open_peek(&p);
            return true;
        }
        if let Some((p, line)) = open {
            if self.nag.is_some() {
                // Don't switch tabs under an open question.
                return redraw;
            }
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

    /// A question bar is about the tab that was active when it appeared;
    /// remember which, so the answer goes to that tab.
    fn sync_nag_owner(&mut self) {
        match (&self.nag, self.nag_tab) {
            (None, _) => self.nag_tab = None,
            (Some(_), None) => self.nag_tab = self.tabs.get(self.active).map(|t| t.id),
            _ => {}
        }
    }

    fn handle_inner(&mut self, input: &Input) -> bool {
        if let Input::Modifiers(m) = input {
            self.mods = *m;
            return false;
        }
        // Ctrl+wheel zooms (like every editor and browser).
        if let Input::Wheel { dy, .. } = input
            && self.mods.ctrl
            && self.nag.is_none()
            && *dy != 0.0
        {
            return self.set_zoom(if *dy < 0.0 {
                self.zoom * 1.1
            } else {
                self.zoom / 1.1
            });
        }
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
            Input::Key {
                key: Key::F(11),
                mods,
            } if !mods.ctrl && !mods.alt && !mods.shift => {
                self.requests.push(Request::ToggleFullscreen);
                return true;
            }
            Input::CloseRequested => {
                // A fresh quit: earlier "Don't save" answers don't carry over
                // from a quit that was interrupted (e.g. by a save question).
                self.quit_skip.clear();
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
                if self.nag.is_some() {
                    self.notice = Some("Answer the question above first".into());
                    return true;
                }
                let p = p.clone();
                self.open(&p);
                return true;
            }
            Input::FsChanged(paths) => {
                self.file_index = None;
                let mut changed = self.tree.invalidate(paths);
                // Comparisons showing a changed file recompute.
                let hits = |v: &dyn View| v.watched_paths().iter().any(|w| paths.contains(w));
                if hits(&*self.view) {
                    let mut ctx = ctx!(self);
                    changed |= self.view.reload(&mut ctx);
                }
                for i in 0..self.tabs.len() {
                    if i != self.active
                        && let Some(v) = self.tabs[i].view.as_mut()
                        && hits(&**v)
                    {
                        let mut ctx = Ctx {
                            fonts: &mut self.fonts,
                            scale: self.scale,
                            zoom: self.zoom,
                            requests: &mut self.requests,
                            cursor: &mut self.cursor,
                            focused: false,
                            clipboard: &mut *self.clipboard,
                            mods: self.mods,
                        };
                        v.reload(&mut ctx);
                    }
                }
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
                                mods: self.mods,
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
                self.cycle_tab(!mods.shift);
                return true;
            }
            Input::Key {
                key: Key::Char(c),
                mods,
            } if mods.alt && !mods.ctrl && c.is_ascii_digit() && *c != '0' => {
                let group = self.group_tabs(self.focused_group());
                let k = (*c as usize - '1' as usize).min(group.len().saturating_sub(1));
                if let Some(&i) = group.get(k) {
                    self.activate(i);
                }
                return true;
            }
            Input::Key {
                key: k @ (Key::Left | Key::Right),
                mods,
            } if mods.ctrl && mods.alt && !mods.shift => {
                return self.move_active_to(if *k == Key::Right { 1 } else { 0 });
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
                    Key::Char('g') if mods.shift => {
                        match self.view.headings() {
                            Some(h) if !h.is_empty() => {
                                self.palette = Some(Palette::headings(&h));
                            }
                            Some(_) => self.notice = Some("This document has no headings".into()),
                            None => {
                                self.notice = Some("Go to Heading works in markdown files".into())
                            }
                        }
                        true
                    }
                    Key::Char('g') if !mods.shift => {
                        if let Some(n) = self.view.line_count() {
                            self.palette = Some(Palette::goto_line(n));
                        }
                        true
                    }
                    Key::Char('f') if mods.shift => {
                        let here = self
                            .view
                            .path()
                            .and_then(|p| self.tree.root_of(p))
                            .map(Path::to_path_buf);
                        match here.or_else(|| self.tree.root().map(Path::to_path_buf)) {
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
                    Key::Char('t') if mods.shift => self.reopen_closed_tab(),
                    Key::Char('n') if !mods.shift => self.new_file(),
                    Key::Char('w') => {
                        let i = self.active;
                        self.close_tab(i);
                        true
                    }
                    Key::PageDown if !mods.shift => {
                        self.cycle_tab(true);
                        true
                    }
                    Key::PageUp if !mods.shift => {
                        self.cycle_tab(false);
                        true
                    }
                    Key::Char('1') if !mods.shift && !mods.alt => self.focus_group(0),
                    Key::Char('2') if !mods.shift && !mods.alt => self.focus_group(1),
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
        // Split view: a click anywhere in the other group focuses it first
        // (then the click is handled there); the wheel scrolls it in place.
        let mut layout = layout;
        if let Some(o) = layout.other {
            let inside = |r: Rect, x: f32, y: f32| {
                x >= r.x as f32 && x < r.right() as f32 && y >= r.y as f32 && y < r.bottom() as f32
            };
            let pane = Rect::new(o.bar.x, o.bar.y, o.bar.w, o.bar.h + o.main.h);
            match input {
                Input::MouseDown { x, y, .. }
                    if inside(pane, *x, *y) && self.tab_drag.is_none() =>
                {
                    self.focus_group(o.group);
                    // Focusing may raise that tab's own question (it changed
                    // on disk): stop here so this click can't land somewhere
                    // else in the new layout.
                    if self.nag.is_some() {
                        return true;
                    }
                    layout = self.layout();
                }
                Input::Wheel { .. } if inside(o.main, self.mouse.0, self.mouse.1) => {
                    if let Some(i) = self.shown_tab(o.group) {
                        let mut ctx = Ctx {
                            fonts: &mut self.fonts,
                            scale: self.scale,
                            zoom: self.zoom,
                            requests: &mut self.requests,
                            cursor: &mut self.cursor,
                            focused: false,
                            clipboard: &mut *self.clipboard,
                            mods: self.mods,
                        };
                        if let Some(v) = self.tabs[i].view.as_mut() {
                            return v.handle(input, o.main, &mut ctx);
                        }
                    }
                }
                Input::MouseMove { x, y } if inside(pane, *x, *y) && self.tab_drag.is_none() => {
                    self.mouse = (*x, *y);
                    self.cursor = Cursor::Default;
                    return true;
                }
                _ => {}
            }
        }
        // Tab bar: activate / close / reorder / scroll / menu.
        if let Some(bar) = layout.tabs {
            self.reveal_active_tab(bar);
            let inside = |x: f32, y: f32| {
                x >= bar.x as f32
                    && x < bar.right() as f32
                    && y >= bar.y as f32
                    && y < bar.bottom() as f32
            };
            // Dragging a tab: it follows the mouse until release.
            if let Some((from, start, dragging)) = self.tab_drag {
                match input {
                    Input::MouseMove { x, y } => {
                        self.mouse = (*x, *y);
                        let dragging = dragging || (x - start).abs() > 6.0 * self.scale;
                        self.tab_drag = Some((from, start, dragging));
                        if dragging {
                            let (w, _) = self.tab_metrics(bar);
                            let g = self.focused_group() as usize;
                            let group = self.group_tabs(g as u8);
                            let scroll = self.tab_scroll[g];
                            let k = (((x - bar.x as f32) + scroll as f32) / w.max(1) as f32)
                                .floor()
                                .clamp(0.0, (group.len().max(1) - 1) as f32)
                                as usize;
                            let to = group.get(k).copied().unwrap_or(from);
                            if to != from {
                                self.move_tab(from, to);
                                self.tab_drag = Some((to, start, true));
                            }
                        }
                        return true;
                    }
                    Input::MouseUp { .. } => {
                        self.tab_drag = None;
                        return true;
                    }
                    Input::MouseDown { .. } => self.tab_drag = None,
                    _ => {}
                }
            }
            match input {
                Input::MouseDown {
                    x,
                    y,
                    button,
                    clicks,
                } if inside(*x, *y) => {
                    self.mouse = (*x, *y);
                    let hit = self.tab_at(bar, *x);
                    if hit.is_none() && *clicks >= 2 && *button == MouseButton::Left {
                        // Double-click on the empty part of the bar: new file.
                        self.new_file();
                        return true;
                    }
                    if let Some(i) = hit {
                        let close = self
                            .tab_rects(bar)
                            .into_iter()
                            .find(|(j, _, _)| *j == i)
                            .map(|(_, _, c)| c)
                            .unwrap_or(Rect::new(0, 0, 0, 0));
                        let on_close = (*x as i32) >= close.x && (*x as i32) < close.right();
                        match button {
                            MouseButton::Middle => self.close_tab(i),
                            MouseButton::Left if on_close => self.close_tab(i),
                            MouseButton::Left => {
                                if *clicks >= 2 {
                                    self.keep_tab(i);
                                }
                                self.activate(i);
                                self.focus = Focus::Main;
                                self.tab_drag = Some((i, *x, false));
                            }
                            MouseButton::Right => self.open_tab_menu(i, *x, *y),
                        }
                    }
                    return true;
                }
                Input::MouseMove { x, y } if inside(*x, *y) => {
                    self.mouse = (*x, *y);
                    self.cursor = Cursor::Default;
                    return true; // hover highlight
                }
                Input::Wheel { dx, dy, .. } if inside(self.mouse.0, self.mouse.1) => {
                    let (_, max_scroll) = self.tab_metrics(bar);
                    let step = 40.0 * self.scale;
                    let d = if dx.abs() > dy.abs() { *dx } else { *dy };
                    let g = self.focused_group() as usize;
                    self.tab_scroll[g] =
                        (self.tab_scroll[g] + (d * step) as i32).clamp(0, max_scroll);
                    return true;
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
                // The tree was hidden mid-drag: the drag is over.
                None => {
                    self.tree.cancel_drag();
                    true
                }
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
        if let Input::MouseDown {
            x,
            y,
            button: MouseButton::Right,
            ..
        } = input
            && !in_tree
            && self.view.text().is_some()
            && (*y as i32) >= layout.main.y
        {
            self.focus = Focus::Main;
            self.open_editor_menu(*x, *y);
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
        self.sync_nag_owner();
        // The other side of a split keeps working too (highlighting, image
        // loads, search results), with part of the budget.
        let other = if self.is_split() {
            self.shown_tab(1 - self.focused_group())
        } else {
            None
        };
        let main_budget = if other.is_some() {
            budget * 2 / 3
        } else {
            budget
        };
        let mut ctx = ctx!(self);
        let mut t = self.view.tick(&mut ctx, main_budget);
        if let Some(i) = other
            && let Some(v) = self.tabs[i].view.as_mut()
        {
            let mut ctx = Ctx {
                fonts: &mut self.fonts,
                scale: self.scale,
                zoom: self.zoom,
                requests: &mut self.requests,
                cursor: &mut self.cursor,
                focused: false,
                clipboard: &mut *self.clipboard,
                mods: self.mods,
            };
            let o = v.tick(&mut ctx, budget - main_budget);
            t.more |= o.more;
            t.redraw |= o.redraw;
        }
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

    /// Free the rebuildable caches of every tab that is not on screen (not
    /// active, and not the tab shown on the other side of a split). Runs
    /// before each frame, so it covers every way a tab gets hidden.
    fn trim_hidden_tabs(&mut self) {
        let other = if self.is_split() {
            self.shown_tab(1 - self.focused_group())
        } else {
            None
        };
        for i in 0..self.tabs.len() {
            let on_screen = i == self.active || Some(i) == other;
            if on_screen {
                self.tabs[i].trimmed = false;
            } else if !self.tabs[i].trimmed {
                if let Some(v) = self.tabs[i].view.as_mut() {
                    v.on_hide();
                }
                self.tabs[i].trimmed = true;
            }
        }
    }

    /// (index, holds caches) for every tab (tests).
    #[cfg(test)]
    pub fn tabs_with_caches(&self) -> Vec<bool> {
        (0..self.tabs.len())
            .map(|i| self.tab_view(i).has_caches())
            .collect()
    }

    pub fn render(&mut self, canvas: &mut Canvas) {
        if (canvas.width() as u32, canvas.height() as u32) != self.size {
            self.size = (canvas.width().max(1) as u32, canvas.height().max(1) as u32);
        }
        self.trim_hidden_tabs();
        let layout = self.layout();
        canvas.set_clip(canvas.bounds());
        canvas.fill_rect(canvas.bounds(), theme::p().background);

        if let Some(bar) = layout.tabs {
            self.reveal_active_tab(bar);
        }
        // Tab bars to draw: the focused group's, and the other group's when
        // split. (bar, rects, titles, shown tab, overflow, focused)
        type BarInfo = (
            Rect,
            Vec<(usize, Rect, Rect)>,
            Vec<(String, bool)>,
            Option<usize>,
            (i32, i32, i32),
            bool,
        );
        let title_of = |app: &Self, i: usize| -> (String, bool) {
            let v = app.tab_view(i);
            let t = v.title();
            (
                if t.is_empty() {
                    "untitled".to_string()
                } else {
                    t
                },
                v.is_dirty(),
            )
        };
        let mut bars: Vec<BarInfo> = Vec::new();
        let focused_g = self.focused_group();
        let mut groups: Vec<(Rect, u8, bool)> = Vec::new();
        if let Some(bar) = layout.tabs {
            groups.push((bar, focused_g, true));
        }
        if let Some(o) = layout.other {
            groups.push((o.bar, o.group, false));
        }
        for (bar, g, focused) in groups {
            let rects = self.group_tab_rects(bar, g);
            let titles = rects.iter().map(|(i, _, _)| title_of(self, *i)).collect();
            let n = rects.len();
            let (w, max_scroll) = self.tab_metrics_n(bar, n);
            let scroll = self.tab_scroll[g as usize].clamp(0, max_scroll);
            bars.push((
                bar,
                rects,
                titles,
                self.shown_tab(g),
                (max_scroll, scroll, w * n as i32),
                focused,
            ));
        }
        let other_view = layout
            .other
            .and_then(|o| self.shown_tab(o.group).map(|i| (o, i)));
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

        for (bar, rects, titles, shown, (max_scroll, scroll, total), focused) in &bars {
            let bar = *bar;
            canvas.set_clip(bar);
            canvas.fill_rect(bar, theme::p().background2);
            let pad = ctx.px(theme::PADDING_X) as i32;
            let lh = ctx.ui_metrics().line_height as i32;
            for ((i, r, close), (title, dirty)) in rects.iter().zip(titles.iter()) {
                // Unsaved changes: a dot in the close button's place (the
                // close X shows while the mouse is over the tab).
                let title = title.strip_prefix("● ").unwrap_or(title);
                let active = Some(*i) == *shown;
                let hovered = (mouse.0 as i32) >= r.x
                    && (mouse.0 as i32) < r.right()
                    && (mouse.1 as i32) >= r.y
                    && (mouse.1 as i32) < r.bottom();
                if active {
                    canvas.fill_rect(*r, theme::p().background);
                    if *focused && layout.other.is_some() {
                        // Which side has the keyboard: a line on its tab.
                        let t = ctx.px(2.0).max(1.0) as i32;
                        canvas.fill_rect(Rect::new(r.x, r.y, r.w, t), theme::p().caret);
                    }
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
                // Peek tabs (replaced by the next single click) are italic.
                let peek = self.tabs.get(*i).is_some_and(|t| t.peek);
                ctx.label_styled(
                    canvas,
                    title,
                    r.x + pad,
                    r.y + (r.h - lh) / 2,
                    color,
                    text_w,
                    FontKind::Ui,
                    peek,
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
            // More tabs than fit: a thin bar along the bottom shows which
            // part is in view (scroll with the mouse wheel over the tabs).
            if *max_scroll > 0 && *total > 0 {
                let h = ctx.px(3.0).max(2.0) as i32;
                let w = (bar.w as i64 * bar.w as i64 / *total as i64) as i32;
                let x = bar.x + (*scroll as i64 * bar.w as i64 / *total as i64) as i32;
                canvas.fill_rect(
                    Rect::new(x, bar.bottom() - h, w.max(h * 4), h),
                    theme::p().scrollbar,
                );
            }
        }
        // The other side of a split: its shown tab, without keyboard focus.
        if let Some(line) = layout.split_line {
            canvas.set_clip(line);
            canvas.fill_rect(line, theme::p().divider);
        }
        if let Some((o, i)) = other_view
            && let Some(v) = self.tabs[i].view.as_mut()
        {
            canvas.set_clip(o.main);
            let was = ctx.focused;
            ctx.focused = false;
            v.render(canvas, o.main, &mut ctx);
            ctx.focused = was;
        }

        canvas.set_clip(layout.main);
        self.view.render(canvas, layout.main, &mut ctx);
        if let Some(p) = self.palette.as_mut() {
            canvas.set_clip(layout.main);
            p.render(canvas, layout.main, &mut ctx);
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
        // The right-click menu goes over everything, the status bar included.
        if let Some(m) = self.menu.as_mut() {
            let all = canvas.bounds();
            canvas.set_clip(all);
            m.render(canvas, all, &mut ctx);
        }
        canvas.set_clip(canvas.bounds());
        // Drawing may have started background work (an image decoded again
        // after its tab was hidden): keep ticking until it is done.
        let other_busy = other_view
            .and_then(|(_, i)| self.tabs[i].view.as_ref())
            .is_some_and(|v| v.has_background_work());
        if self.view.has_background_work() || other_busy {
            self.pending = true;
        }
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
