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
use crate::menubar;
use crate::nag::{Nag, NagAction, Pending};
use crate::nav::{History, Loc, Remembered};
use crate::palette::{Action, Outcome, Palette, index_files};
use crate::session::{Session, push_recent};
use crate::settings::{self, Settings};
use crate::text::Fonts;
use crate::theme;
use crate::tree::Tree;
use crate::ui::{Ctx, FontKind};
use crate::view::about::AboutView;
use crate::view::code::CodeView;
use crate::view::diff::{DiffView, Side};
use crate::view::image::{ImageView, is_image};
use crate::view::markdown::{MarkdownView, is_markdown};
use crate::view::message::MessageView;
use crate::view::search::SearchView;
use crate::view::start::StartView;
use crate::view::{DiskState, Tick, View, ViewPos};
use crate::workspace::{self, Workspace};

mod treeops;

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
    /// Files handed over by another launch while a question was showing;
    /// opened once it is answered.
    queued_opens: Vec<PathBuf>,
    /// Back / Forward history (Alt+Left / Alt+Right).
    nav: History,
    /// The tab the history last saw in front: (id, shows a file).
    nav_tab: (u64, bool),
    /// Where that tab was when it was closed (its view is gone).
    nav_closed: Option<(u64, Loc)>,
    /// Where the reader was in files closed during this run.
    file_pos: Remembered,
    /// Tree copy / paste state and the active file's size and date.
    ops: treeops::TreeOps,
    /// How many bytes of files one restore (session, workspace) may open.
    restore_budget: u64,
    /// How many line objects text buffers from one restore may retain.
    restore_line_budget: usize,
    /// Tabs of the restored workspace that did not fit in the budget:
    /// (path, side, position). Kept in the workspace when it is saved.
    not_reopened: Vec<(PathBuf, u8, Option<ViewPos>)>,
    /// The workspace file the tree came from (opened or saved as), if any;
    /// shown in the tree header. Cleared when another folder is opened or
    /// the folder is closed.
    workspace_file: Option<PathBuf>,
    /// The mouse is over the tree header.
    header_hover: bool,
    /// Setting `menu_bar`: the menu bar is shown.
    menu_bar_on: bool,
    /// The menu bar's menu that is open (it is `menu`); the bar shows while
    /// one is open even when it is hidden.
    bar_menu: Option<usize>,
    /// The menu bar title under the mouse.
    bar_hover: Option<usize>,
}

/// At most this much file text is opened by one restore (a session or a
/// workspace, which may come from someone else and name 200 big files).
pub const MAX_RESTORE_BYTES: u64 = 512 * 1024 * 1024;
/// Line `String`s have substantial per-object overhead even when their text
/// is tiny, so raw bytes alone cannot bound restored editor-buffer memory.
/// Two per-file line budgets keep that overhead to roughly 200 MB per restore.
pub const MAX_RESTORE_LINES: usize = crate::doc::MAX_LINES * 2;

#[derive(Clone, Copy, PartialEq, Eq)]
enum RestoreLimit {
    Bytes,
    Lines,
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
    /// The menu bar along the top.
    menubar: Option<Rect>,
    nag: Option<Rect>,
    /// Tab bar and editor area of the focused group.
    tabs: Option<Rect>,
    /// The other group, when the view is split.
    other: Option<OtherPane>,
    /// The line between the two groups.
    split_line: Option<Rect>,
    /// The row above the tree naming what is open (folder / workspace).
    tree_header: Option<Rect>,
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
            view: Box::new(StartView::new(&[])),
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
            queued_opens: Vec::new(),
            nav: History::default(),
            nav_tab: (1, false),
            nav_closed: None,
            file_pos: Remembered::default(),
            ops: Default::default(),
            mods: crate::input::Mods::default(),
            restore_budget: MAX_RESTORE_BYTES,
            restore_line_budget: MAX_RESTORE_LINES,
            not_reopened: Vec::new(),
            workspace_file: None,
            header_hover: false,
            menu_bar_on: true,
            bar_menu: None,
            bar_hover: None,
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
        self.menu_bar_on = s.menu_bar;
        if self.tree.show_hidden() != s.hidden_files {
            self.tree.set_show_hidden(s.hidden_files);
            self.file_index = None;
        }
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
        self.file_details_changed(&[p.to_path_buf()]);
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
        let (value, name) = if light {
            ("light", "Light theme")
        } else {
            ("dark", "Dark theme")
        };
        self.save_setting("theme", value, name);
    }

    /// Record `key = value` in the settings file after a change made from a
    /// command (theme, menu bar); only that line changes. `name` says what
    /// changed, for the status bar. If the settings file is open with
    /// unsaved edits, or can't be rewritten safely, it is left alone.
    fn save_setting(&mut self, key: &str, value: &str, name: &str) {
        let Some(path) = self.settings_path.clone() else {
            self.notice = Some(name.to_string());
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
                "{name} for now; settings.txt has unsaved edits, so it was not changed"
            ));
            return;
        }
        let old = match crate::fileio::read_bounded(&path, settings::MAX_BYTES) {
            Err(crate::fileio::ReadError::TooLarge(_)) => {
                self.notice = Some(format!("{name} for now; settings.txt is too large"));
                return;
            }
            // Only plain UTF-8 is rewritten; anything else would be damaged.
            Ok(loaded) => match String::from_utf8(loaded.bytes) {
                Ok(t) => Some(t),
                Err(_) => {
                    self.notice = Some(format!(
                        "{name} for now; settings.txt isn't UTF-8, so it was not changed"
                    ));
                    return;
                }
            },
            Err(crate::fileio::ReadError::Open(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                Some(settings::TEMPLATE.to_string())
            }
            Err(_) => None,
        };
        let Some(old) = old else {
            self.notice = Some(format!("{name} for now; could not read settings.txt"));
            return;
        };
        let new = settings::with_setting(&old, key, value);
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match crate::editor::save::save_bytes(&path, new.as_bytes()) {
            Ok(()) => {
                self.notice = Some(format!("{name} (saved in settings)"));
                // A clean tab showing the file picks up the change.
                let mut changed: Vec<PathBuf> = open_as.into_iter().map(|(p, _)| p).collect();
                changed.push(path);
                self.handle(Input::FsChanged(changed));
            }
            Err(e) => {
                self.notice = Some(format!("{name} for now; could not save settings: {e}"));
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

    #[cfg(test)]
    pub fn highlight_probe(
        &self,
        line: usize,
    ) -> Option<(usize, Option<Vec<crate::syntax::Span>>)> {
        self.view.highlight_probe(line)
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

    /// The active view's area (tests).
    #[cfg(test)]
    pub fn main_rect(&self) -> Rect {
        self.layout().main
    }

    /// Height of the row above the tree (tests: tree rows start below it).
    #[cfg(test)]
    pub fn tree_header_height(&self) -> f32 {
        let pad_y = (theme::PADDING_Y * self.scale).round();
        (self.ui_line_height() + pad_y * 2.0).round()
    }

    #[cfg(test)]
    pub fn tree_header_rect(&self) -> Option<Rect> {
        self.layout().tree_header
    }

    /// Centre of menu bar title `i` (None while the bar is hidden).
    #[cfg(test)]
    pub fn bar_title_pos(&mut self, i: usize) -> Option<(f32, f32)> {
        let bar = self.layout().menubar?;
        let mut ctx = ctx!(self);
        let r = *menubar::title_rects(bar, &mut ctx).get(i)?;
        Some((r.x as f32 + r.w as f32 / 2.0, r.y as f32 + r.h as f32 / 2.0))
    }

    #[cfg(test)]
    pub fn bar_menu_open(&self) -> Option<usize> {
        self.bar_menu
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
                Ok(w) => {
                    if self.apply_workspace(w) {
                        self.workspace_file = Some(path.to_path_buf());
                    }
                }
                Err(e) => self.nag = Some(Nag::error(format!("Cannot open workspace: {e}"))),
            }
            return;
        }
        self.open_now(path);
        self.nav_sync();
    }

    // ---- back / forward --------------------------------------------------

    /// Where the reader is now, if the active tab shows a file.
    fn nav_here(&self) -> Option<Loc> {
        self.nav_loc(self.active)
    }

    fn nav_loc(&self, i: usize) -> Option<Loc> {
        let v = self.tab_view(i);
        Some(Loc {
            path: v.path()?.to_path_buf(),
            pos: v.position()?,
            peek: self.tabs.get(i).is_some_and(|t| t.peek),
        })
    }

    fn nav_key(&self) -> (u64, bool) {
        (
            self.tabs.get(self.active).map_or(0, |t| t.id),
            self.view.path().is_some(),
        )
    }

    /// A jump inside the active tab may have happened since `before` was
    /// taken (go to line, go to heading, a search result): if the reader
    /// really moved, Back returns there.
    fn nav_jumped(&mut self, before: Option<Loc>) {
        if let Some(b) = before
            && self.nav_here().is_none_or(|h| !h.same(&b))
        {
            self.nav.jumped(b);
        }
        self.nav_sync();
    }

    /// Notice that another tab came to the front (whatever did it: a click
    /// on a tab, a link, the tree, quick open, closing a tab) and remember
    /// the place that was left. Called after every input.
    fn nav_sync(&mut self) {
        // A `#heading` link scrolled the page.
        if let Some(pos) = self.view.take_jump_origin()
            && let Some(here) = self.nav_here()
        {
            let from = Loc {
                pos,
                ..here.clone()
            };
            if !from.same(&here) {
                self.nav.jumped(from);
            }
        }
        let now = self.nav_key();
        let (old_id, had_file) = std::mem::replace(&mut self.nav_tab, now);
        let left = match self.nav_closed.take() {
            // It was closed (the last tab keeps its id for whatever is
            // shown next, so the id alone can't tell).
            Some((id, loc)) if id == old_id => Some(loc),
            _ if had_file && old_id != now.0 => {
                self.tab_index(old_id).and_then(|i| self.nav_loc(i))
            }
            _ => None,
        };
        if let Some(l) = left {
            self.nav.jumped(l);
        }
    }

    /// Alt+Left / Alt+Right: show the previous (next) place in the history,
    /// reopening its file if the tab was closed. Places whose file is gone
    /// are skipped. Uses the ordinary ways of showing a file, so no tab
    /// with unsaved edits is ever replaced or closed, and nothing moves
    /// while a question is showing.
    pub fn navigate(&mut self, back: bool) {
        if self.nag.is_some() {
            return;
        }
        self.nav_sync();
        let here = self.nav_here();
        let open: Vec<PathBuf> = (0..self.tabs.len())
            .filter_map(|i| self.tab_view(i).path().map(Path::to_path_buf))
            .collect();
        let target = self.nav.step(back, here.as_ref(), |l| {
            open.contains(&l.path) || l.path.is_file()
        });
        let Some(loc) = target else {
            self.notice = Some(
                if back {
                    "No earlier place to go back to"
                } else {
                    "No later place to go forward to"
                }
                .into(),
            );
            return;
        };
        if let Some(i) = self.find_tab(&loc.path) {
            // An open tab is only brought to the front.
            self.activate(i);
            self.focus = Focus::Main;
        } else if loc.peek && !workspace::is_workspace_file(&loc.path) {
            // A peeked file comes back in the peek tab.
            self.open_peek(&loc.path);
        } else {
            self.open_now(&loc.path);
        }
        if self.view.path() == Some(loc.path.as_path()) {
            self.view.restore_position(loc.pos);
        }
        // This move is not a jump to remember.
        self.nav_tab = self.nav_key();
        self.nav_closed = None;
        self.pending = true;
    }

    /// (back, forward) history lengths (tests).
    #[cfg(test)]
    pub fn nav_lens(&self) -> (usize, usize) {
        self.nav.lens()
    }

    /// The active view's position (tests).
    #[cfg(test)]
    pub fn view_position(&self) -> Option<ViewPos> {
        self.view.position()
    }

    #[cfg(test)]
    pub fn restore_view_position(&mut self, pos: ViewPos) {
        self.view.restore_position(pos);
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

    #[cfg(test)]
    pub fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    fn tab_index(&self, id: u64) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }

    /// The tab showing `path`, however it is spelled (`.`/`..` parts; case
    /// and `/` vs `\` on Windows): one file is never open in two tabs.
    fn find_tab(&self, path: &Path) -> Option<usize> {
        (0..self.tabs.len()).find(|&i| {
            self.tab_view(i)
                .path()
                .is_some_and(|p| crate::fsops::same_file(p, path))
        })
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
        if self.view.path().is_none() && self.tabs.len() == 1 && !self.view.keeps_its_tab() {
            self.view = view;
            self.tabs[0].peek = false;
            self.tabs[0].trimmed = false;
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

    /// Help → About: the version and who made this build, in a tab. Asking
    /// again shows the tab that is already open.
    fn open_about(&mut self) {
        let open = (0..self.tabs.len()).find(|&i| {
            let v = self.tab_view(i);
            v.path().is_none() && v.title() == crate::view::about::TITLE
        });
        match open {
            Some(i) => self.activate(i),
            None => self.add_tab(Box::new(AboutView::new(crate::branding::embedded()))),
        }
        self.focus = Focus::Main;
        self.pending = true;
    }

    /// Test hook: an About tab for this identity (the window always shows
    /// the embedded one).
    #[cfg(test)]
    pub fn add_about(&mut self, branding: crate::branding::Branding) {
        self.add_tab(Box::new(AboutView::new(branding)));
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
        // Opening the file again continues where the reader was, and Back
        // can return to a closed tab.
        if let Some(loc) = self.nav_loc(i) {
            self.file_pos.put(&loc.path, loc.pos);
            if id == self.nav_tab.0 {
                self.nav_closed = Some((id, loc));
            }
        }
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
            // The slot stays (it shows the start page): it is no longer a
            // peek tab.
            self.tabs[0].peek = false;
            self.view = if self.tree.root().is_some() {
                Box::new(MessageView::folder_opened())
            } else {
                self.start_view()
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
                    // (A running tree copy is stopped when the window
                    // really closes: this quit can still be called off.)
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
        if self.view.path().is_none() || !self.view.can_save() {
            return false;
        }
        let name = self.view_name();
        // Setting `trim_trailing_whitespace` (off by default, like lite-xl);
        // never for markdown, where two trailing spaces mean a line break.
        if self.view.is_read_only() && !self.view.is_dirty() {
            // Locked and nothing changed: there is nothing to write (and
            // a read-only file must never be rewritten by accident).
            self.notice = Some(format!(
                "{name} is read-only and unchanged; nothing to save"
            ));
            if let Some(p) = then {
                self.perform(p);
            }
            return true;
        }
        if self.trim_on_save
            && self.view.is_dirty()
            && !self.view.is_read_only()
            && !self.view.path().is_some_and(is_markdown)
        {
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
            NagAction::OpenWith(add) => self.requests.push(Request::OpenWith(add)),
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
            NagAction::RecycleMany(paths) => self.recycle_many(paths),
            NagAction::MoveMany(paths, dir) => self.move_many(paths, dir),
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
                    all.extend(index_files(r, self.tree.show_hidden()));
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
            Outcome::Run(Action::CloseFolder) => self.close_folder(),
            Outcome::Run(Action::ToggleMenuBar) => self.toggle_menu_bar(),
            Outcome::Run(Action::OpenWith(add)) => {
                let exe = std::env::current_exe()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|_| "litedit".into());
                self.nag = Some(if add {
                    Nag::new(
                        format!(
                            "Add {exe} to \"Open with\" for .md files? (Changes only your Windows user settings.)"
                        ),
                        vec![
                            ("Add", NagAction::OpenWith(true)),
                            ("Cancel", NagAction::Dismiss),
                        ],
                        false,
                    )
                } else {
                    Nag::new(
                        "Remove litedit from \"Open with\" for .md files?".into(),
                        vec![
                            ("Remove", NagAction::OpenWith(false)),
                            ("Cancel", NagAction::Dismiss),
                        ],
                        false,
                    )
                });
            }
            Outcome::Run(Action::Heading(line)) => {
                let before = self.nav_here();
                self.view.goto_heading(line);
                self.focus = Focus::Main;
                self.nav_jumped(before);
            }
            Outcome::Run(Action::Command(c)) => {
                if self.view.run_command(c) {
                    self.pending = true;
                } else if c == "ignore-whitespace" {
                    self.notice = Some("Ignore Whitespace works in a comparison".into());
                } else {
                    self.notice = Some("That command works in a text file".into());
                }
            }
            Outcome::Run(Action::CompareWithSaved) => {
                let id = self.tabs[self.active].id;
                self.compare_with_saved(id);
            }
            Outcome::Run(Action::CompareSides) => self.compare_sides(),
            Outcome::Run(Action::CompareWithClipboard) => self.compare_with_clipboard(),
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
                let before = self.nav_here();
                self.view.goto_line(n);
                self.focus = Focus::Main;
                self.nav_jumped(before);
            }
        }
        self.pending = true;
    }

    /// The refusal for deleting / moving / renaming a folder that has a
    /// workspace folder somewhere inside it: workspace folders are only ever
    /// removed from the workspace by name, never as part of something else.
    fn workspace_folder_refusal(&self, paths: &[PathBuf], done: &str) -> Option<String> {
        let roots = self.tree.roots();
        paths.iter().find_map(|p| {
            let inside = roots
                .iter()
                .find(|r| r.as_path() != p.as_path() && crate::fsops::is_under(r, p))?;
            Some(format!(
                "Nothing was {done}: {} contains the workspace folder {}. Remove that folder from the workspace first.",
                display_name(p),
                display_name(inside)
            ))
        })
    }

    /// Paths of tabs at or under `path`, with their dirty state.
    fn tabs_under(&self, path: &Path) -> Vec<(u64, PathBuf, bool)> {
        (0..self.tabs.len())
            .filter_map(|i| {
                let v = self.tab_view(i);
                let p = v.path()?.to_path_buf();
                crate::fsops::is_under(&p, path).then(|| (self.tabs[i].id, p, v.is_dirty()))
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
            self.open(&crate::fsops::rebase(p, old, new));
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
        if let Some(refusal) = self.workspace_folder_refusal(std::slice::from_ref(&from), "moved") {
            self.nag = Some(Nag::error(refusal));
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
        // One of several selected entries: a menu for all of them.
        if self.selection_menu(x, y, &path, &dir) {
            return;
        }
        let mut items = vec![
            ("New file…", MenuAction::NewFile(dir.clone())),
            ("New folder…", MenuAction::NewFolder(dir.clone())),
        ];
        if is_root {
            items.push(("Add Folder to Workspace…", MenuAction::AddFolder));
        }
        if !is_root {
            items.push(("Rename…", MenuAction::Rename(path.clone())));
            items.push(("Delete (to Recycle Bin)", MenuAction::Delete(path.clone())));
            items.push(("Copy", MenuAction::CopyEntries(vec![path.clone()])));
        }
        if self.can_paste() {
            items.push(("Paste", MenuAction::PasteInto(dir)));
        }
        if !is_root {
            items.push(("Duplicate", MenuAction::Duplicate(vec![path.clone()])));
        }
        if is_root && self.tree.roots().len() > 1 {
            items.push((
                "Remove Folder from Workspace",
                MenuAction::RemoveFolder(path.clone()),
            ));
        }
        if is_root && self.tree.root() == Some(path.as_path()) {
            items.push(("Close Folder", MenuAction::CloseFolder));
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
        if self.run_tree_menu(&a) {
            return;
        }
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
            MenuAction::EditorKeys(combo) => {
                // Edit menu: the open file, even if the tree had the keyboard.
                self.focus = Focus::Main;
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
            MenuAction::CloseFolder => {
                self.close_folder();
                return;
            }
            MenuAction::Palette(action) => {
                self.run_palette(Outcome::Run(action.clone()));
                return;
            }
            MenuAction::Exit => {
                // The normal close: unsaved edits are asked about first.
                self.handle(Input::CloseRequested);
                return;
            }
            MenuAction::About => {
                self.open_about();
                return;
            }
            MenuAction::ToggleMenuBar => {
                self.toggle_menu_bar();
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
                // Picked in the tree, where an entry may be a link to a
                // network share: never read through one.
                for p in [&old, &new] {
                    if let Err(why) = crate::links::content_may_open(p) {
                        self.notice = Some(refused_notice(why, &[p.as_path()]));
                        return;
                    }
                }
                self.compare_files(&old, &new);
                return;
            }
            MenuAction::CompareWithSaved(id) => {
                self.compare_with_saved(*id);
                return;
            }
            MenuAction::CompareSides => {
                self.compare_sides();
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
                if let Some(refusal) =
                    self.workspace_folder_refusal(std::slice::from_ref(p), "renamed")
                {
                    self.nag = Some(Nag::error(refusal));
                    return;
                }
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
                if let Some(refusal) =
                    self.workspace_folder_refusal(std::slice::from_ref(p), "deleted")
                {
                    self.nag = Some(Nag::error(refusal));
                    return;
                }
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
                    let view = SearchView::start(root, text.trim(), self.tree.show_hidden());
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
            self.refresh_start_page();
        }
        self.session_dirty = false;
    }

    /// What to remember for next time.
    pub fn session(&self) -> Session {
        let mut s = self.current_session();
        if !self.owns_workspace {
            s.root = self.stored.root.clone();
            s.tabs = self.stored.tabs.clone();
            s.positions = self.stored.positions.clone();
            s.active = self.stored.active;
            s.workspace = self.stored.workspace.clone();
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
            positions: ws.positions.clone(),
            active: ws.active,
            recent: self.recent.clone(),
            tree_width: self.tree.width,
            tree_visible: self.tree.visible,
            zoom: self.zoom,
            workspace: self.workspace_file.clone(),
        }
    }

    /// Reopen the last session (only what still exists on disk).
    pub fn restore(&mut self, s: Session) {
        self.not_reopened.clear();
        self.recent = s.recent;
        self.tree.width = s.tree_width;
        self.zoom = s.zoom;
        if let Some(r) = s.root.filter(|r| r.is_dir()) {
            self.tree.set_root(&r);
            for f in s.folders.iter().filter(|f| f.is_dir()) {
                self.tree.add_root(f);
            }
            self.view = Box::new(MessageView::folder_opened());
            // Only a name for the tree header: the file is not read.
            self.workspace_file = s
                .workspace
                .filter(|f| workspace::is_workspace_file(f) && f.is_file());
        }
        let tabs: Vec<(PathBuf, u8)> = s
            .tabs
            .iter()
            .enumerate()
            .map(|(i, t)| (t.clone(), s.groups.get(i).copied().unwrap_or(0)))
            .collect();
        // The reader's own session: not gated (a link they made to their
        // own share keeps working), but within the restore budget.
        self.open_workspace_tabs(&tabs, &s.positions, s.active, false);
        self.tree.visible = s.tree_visible;
        self.focus = Focus::Main;
        self.refresh_start_page();
        self.session_dirty = false;
    }

    /// The start page (no folder, no file): what to open, and the recent
    /// folders, workspaces and files that still exist.
    fn start_view(&self) -> Box<dyn View> {
        Box::new(StartView::new(&self.recent))
    }

    /// Show the start page again if it is what the window shows (its list of
    /// recent entries was built before the session was loaded).
    fn refresh_start_page(&mut self) {
        if self.tree.root().is_none()
            && self.tabs.len() == 1
            && self.view.path().is_none()
            && !self.view.is_dirty()
            && self.view.title().is_empty()
        {
            self.view = self.start_view();
        }
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
        if !self.begin_open(path) {
            return;
        }
        let view = self.make_view(path);
        self.finish_open(path, view);
    }

    /// Apply the path-level effects of opening and focus an existing tab.
    /// Returns true when the caller still needs to construct a new view.
    fn begin_open(&mut self, path: &Path) -> bool {
        // Opened after all: from now on it is an ordinary tab (closing it
        // takes it out of the workspace).
        self.not_reopened
            .retain(|(p, ..)| !crate::fsops::same_file(p, path));
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
            return false;
        }
        true
    }

    fn finish_open(&mut self, path: &Path, view: Box<dyn View>) {
        self.add_tab(view);
        self.tree.reveal(path);
        self.focus = Focus::Main;
        self.pending = true;
    }

    fn make_view(&self, path: &Path) -> Box<dyn View> {
        self.with_remembered_position(path, self.make_view_at_top(path))
    }

    fn with_remembered_position(&self, path: &Path, mut view: Box<dyn View>) -> Box<dyn View> {
        // Closed earlier in this run: continue where the reader was.
        if let Some(pos) = self.file_pos.get(path) {
            view.restore_position(pos);
        }
        view
    }

    fn make_view_at_top(&self, path: &Path) -> Box<dyn View> {
        if is_image(path) {
            return Box::new(ImageView::new(path));
        }
        match Document::open(path) {
            Ok(doc) => self.view_for_document(path, doc),
            Err(e) => Box::new(MessageView::error(
                "Error",
                format!("Could not open {}: {e}", path.display()),
            )),
        }
    }

    fn view_for_document(&self, path: &Path, doc: Document) -> Box<dyn View> {
        if is_markdown(path) {
            Box::new(MarkdownView::new(doc))
        } else {
            let mut v = CodeView::new(doc);
            v.wrap = self.wrap_code;
            Box::new(v)
        }
    }

    /// Build a restored view only when its decoded text fits the remaining
    /// aggregate line-object budget. This check uses the same bytes that are
    /// passed to the editor, so a stale planning scan cannot admit a buffer.
    fn make_restored_view(
        &self,
        path: &Path,
        remaining_lines: usize,
    ) -> Option<(Box<dyn View>, usize)> {
        if is_image(path) {
            let view = self.with_remembered_position(path, Box::new(ImageView::new(path)));
            return Some((view, 0));
        }
        match Document::open_counted(path) {
            Ok((doc, lines)) if lines <= remaining_lines => {
                let view = self.view_for_document(path, doc);
                Some((self.with_remembered_position(path, view), lines))
            }
            Ok(_) => None,
            Err(e) => Some((
                Box::new(MessageView::error(
                    "Error",
                    format!("Could not open {}: {e}", path.display()),
                )),
                0,
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
            // Already open: show it. The keyboard stays where it is (Space
            // in the tree must not end up typed into the file).
            self.activate(i);
            return;
        }
        let g = self.focused_group();
        let old = (0..self.tabs.len()).find(|&i| {
            self.tabs[i].peek && self.tabs[i].group == g && !self.tab_view(i).is_dirty()
        });
        let view = self.make_view(path);
        match old {
            // The start page: becomes the peek tab.
            None if self.view.path().is_none()
                && self.tabs.len() == 1
                && !self.view.keeps_its_tab() =>
            {
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

    #[cfg(test)]
    pub fn focus(&self) -> Focus {
        self.focus
    }

    pub fn take_requests(&mut self) -> Vec<Request> {
        let requests = std::mem::take(&mut self.requests);
        // The window is told to close now (a quit asked for earlier can
        // still be called off): stop a running tree copy, removing the
        // file it was in the middle of writing.
        if requests.contains(&Request::Quit) {
            self.cancel_copy();
        }
        requests
    }

    pub fn has_pending_work(&self) -> bool {
        self.pending
    }

    pub fn status_items(&self) -> Vec<String> {
        let mut v = self.view.status();
        v.extend(self.status_tree_items());
        if let Some(n) = &self.notice {
            v.push(n.clone());
        }
        if let Some(n) = self.tree.truncated_note() {
            v.push(n);
        }
        // Drawn at the right end of the bar.
        v.extend(self.status_file_details());
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
        let bar_h = if self.menu_bar_on || self.bar_menu.is_some() {
            (self.ui_line_height() as i32 + pad_y * 2)
                .min(h - status_h)
                .max(0)
        } else {
            0
        };
        let menubar = (bar_h > 0).then(|| Rect::new(0, 0, w, bar_h));
        let nag_h = if self.nag.is_some() {
            (self.ui_line_height() as i32 + pad_y * 3)
                .min(h - status_h - bar_h)
                .max(0)
        } else {
            0
        };
        let nag = (nag_h > 0).then(|| Rect::new(0, bar_h, w, nag_h));
        let top = bar_h + nag_h;
        let body_h = (h - status_h - top).max(0);
        let (tree_header, tree, divider, col) = if self.tree.is_shown() && w > 200 {
            let tw = (self.tree.width * self.scale)
                .clamp(
                    TREE_MIN_W * self.scale,
                    (w as f32 * 0.6).max(TREE_MIN_W * self.scale),
                )
                .min(w as f32 - 100.0)
                .round() as i32;
            let div = (self.scale.round() as i32).max(1);
            let hh = (self.ui_line_height() as i32 + pad_y * 2).min(body_h);
            (
                Some(Rect::new(0, top, tw, hh)),
                Some(Rect::new(0, top + hh, tw, body_h - hh)),
                Some(Rect::new(tw, top, div, body_h)),
                Rect::new(tw + div, top, (w - tw - div).max(0), body_h),
            )
        } else {
            (None, None, None, Rect::new(0, top, w, body_h))
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
            menubar,
            nag,
            tabs,
            other,
            split_line,
            tree_header,
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
            if self.split_files().is_some() {
                items.push(("Compare Left ↔ Right", MenuAction::CompareSides));
            }
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
            && crate::links::content_may_open(sel).is_ok()
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
        // Never switch tabs, or replace a question, while one is showing.
        if self.nag.is_some() {
            self.notice = Some("Answer the question above first, then compare again".into());
            return;
        }
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
        let new = Side::from_snapshot(&text, format!("{name} (unsaved)"), &path, Some(&path));
        self.open_compare(Side::from_file(&path, format!("{name} (saved)")), new);
    }

    /// Split view with a file showing on each side: (left tab, right tab).
    fn split_files(&self) -> Option<(usize, usize)> {
        if !self.is_split() {
            return None;
        }
        let (l, r) = (self.shown_tab(0)?, self.shown_tab(1)?);
        (self.tab_view(l).path().is_some() && self.tab_view(r).path().is_some()).then_some((l, r))
    }

    /// What file tab `i` shows, as one side of a comparison: the file on
    /// disk, or the tab's text when it has unsaved edits. Nothing is changed
    /// in the tab.
    fn tab_side(&self, i: usize, name: String) -> Result<Side, String> {
        let v = self.tab_view(i);
        let Some(path) = v.path().map(Path::to_path_buf) else {
            return Err("not a file".into());
        };
        let unsaved = if v.is_dirty() { v.text() } else { None };
        match unsaved {
            Some(text) => {
                Side::from_snapshot(&text, format!("{name} (unsaved)"), &path, Some(&path))
            }
            None => Side::from_file(&path, name),
        }
    }

    /// "File: Compare Left ↔ Right": the file shown on the left of a split
    /// (old) against the one shown on the right (new).
    pub fn compare_sides(&mut self) {
        if !self.is_split() {
            self.notice = Some(
                "Compare Left ↔ Right needs a split view (Ctrl+Alt+Right moves a tab to the other side)"
                    .into(),
            );
            return;
        }
        let Some((l, r)) = self.split_files() else {
            self.notice = Some("Compare Left ↔ Right needs a file on both sides".into());
            return;
        };
        let name = |i: usize| {
            self.tab_view(i)
                .path()
                .map(display_name)
                .unwrap_or_default()
        };
        let (mut ln, mut rn) = (name(l), name(r));
        if ln == rn {
            // Same name in two folders: say which is which.
            let with_folder = |i: usize| {
                self.tab_view(i)
                    .path()
                    .and_then(Path::parent)
                    .map(|d| format!("{}/{}", display_name(d), name(i)))
                    .unwrap_or_else(|| name(i))
            };
            (ln, rn) = (with_folder(l), with_folder(r));
        }
        let (old, new) = (self.tab_side(l, ln), self.tab_side(r, rn));
        self.open_compare(old, new);
    }

    /// "File: Compare with Clipboard": the clipboard's text (old) against
    /// the whole current text of the active file (new), unsaved edits
    /// included.
    pub fn compare_with_clipboard(&mut self) {
        let Some(path) = self
            .view
            .path()
            .filter(|_| self.view.text().is_some())
            .map(Path::to_path_buf)
        else {
            self.notice = Some("Compare with Clipboard works in a file tab".into());
            return;
        };
        let clip = self.clipboard.get().unwrap_or_default();
        if clip.is_empty() {
            self.notice = Some("The clipboard has no text to compare".into());
            return;
        }
        let old = Side::from_snapshot(&clip, "Clipboard".into(), &path, None);
        let new = self.tab_side(self.active, display_name(&path));
        self.open_compare(old, new);
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
                let pos = self.tab_view(i).position();
                w.positions.push(pos.filter(|p| *p != ViewPos::default()));
            }
        }
        // Tabs that were too big to reopen stay in the workspace: saving it
        // never drops a file the reader did not close.
        for (p, g, pos) in &self.not_reopened {
            if !w.tabs.iter().any(|(t, _)| crate::fsops::same_file(t, p)) {
                w.tabs.push((p.clone(), *g));
                w.positions.push(*pos);
            }
        }
        w
    }

    /// Lower the restore budget (tests: no need for 512 MB of files).
    #[cfg(test)]
    pub fn set_restore_budget(&mut self, bytes: u64) {
        self.restore_budget = bytes;
    }

    /// Lower the retained line-object budget (tests).
    #[cfg(test)]
    pub fn set_restore_line_budget(&mut self, lines: usize) {
        self.restore_line_budget = lines;
    }

    /// Save the current folder's workspace in the store (best effort).
    pub fn persist_workspace(&self) {
        if let (Some(store), Some(root)) = (&self.workspace_store, self.tree.root()) {
            let _ = self
                .current_workspace()
                .save(&workspace::folder_file(store, root));
        }
    }

    /// Which of `tabs` to open: one entry per file (however it is spelled),
    /// existing files only, and together within both restore budgets (files
    /// already open cost nothing). The active tab is counted first, so it is
    /// restored whenever it fits on its own; then the others in order. With
    /// `gate` (the list comes from a workspace file) tabs that content may not
    /// open are refused before anything touches them. Returns the indices to
    /// open (in order), those left out with their limiting budget, and the
    /// refused ones.
    fn plan_restore(
        &self,
        tabs: &[(PathBuf, u8)],
        active: usize,
        gate: bool,
    ) -> (
        Vec<usize>,
        Vec<(usize, RestoreLimit)>,
        Vec<(usize, &'static str)>,
    ) {
        let mut unique: Vec<usize> = Vec::new();
        let mut refused = Vec::new();
        for (k, (p, _)) in tabs.iter().enumerate() {
            if gate && let Err(why) = crate::links::content_may_open(p) {
                refused.push((k, why));
                continue;
            }
            if !unique
                .iter()
                .any(|&j| crate::fsops::same_file(&tabs[j].0, p))
            {
                unique.push(k);
            }
        }
        // The active tab may be a second spelling of an earlier one.
        let active = tabs.get(active).and_then(|(a, _)| {
            unique
                .iter()
                .copied()
                .find(|&j| crate::fsops::same_file(&tabs[j].0, a))
        });
        let order = active
            .into_iter()
            .chain(unique.iter().copied().filter(|&k| Some(k) != active));
        let (mut open, mut skipped) = (Vec::new(), Vec::new());
        let (mut used_bytes, mut used_lines) = (0u64, 0usize);
        for k in order {
            let p = &tabs[k].0;
            if self.find_tab(p).is_some() {
                open.push(k);
                continue;
            }
            let Some(size) = std::fs::metadata(p)
                .ok()
                .filter(|m| m.is_file())
                .map(|m| m.len())
            else {
                continue;
            };
            let Some(bytes) = used_bytes
                .checked_add(size)
                .filter(|&u| u <= self.restore_budget)
            else {
                skipped.push((k, RestoreLimit::Bytes));
                continue;
            };
            let remaining = self.restore_line_budget.saturating_sub(used_lines);
            // Images and document errors do not create editor line objects.
            // The decoded Document is dropped after this preflight, before
            // any Buffer or per-line String is allocated.
            let lines = if is_image(p) {
                0
            } else {
                Document::open_counted(p)
                    .map(|(_, lines)| lines.min(remaining.saturating_add(1)))
                    .unwrap_or(0)
            };
            let Some(line_total) = used_lines
                .checked_add(lines)
                .filter(|&u| u <= self.restore_line_budget)
            else {
                skipped.push((k, RestoreLimit::Lines));
                continue;
            };
            used_bytes = bytes;
            used_lines = line_total;
            open.push(k);
        }
        open.sort_unstable();
        skipped.sort_unstable_by_key(|&(k, _)| k);
        (open, skipped, refused)
    }

    /// Open `tabs` (existing files only, within the restore budgets) on
    /// their sides, each where the reader was (`positions`, by index); show
    /// `active`. Tabs left out for size are remembered (and kept when the
    /// workspace is saved) and the reader is told once. With `gate`, tabs
    /// content may not open are refused; returns what to say about them.
    fn open_workspace_tabs(
        &mut self,
        tabs: &[(PathBuf, u8)],
        positions: &[Option<ViewPos>],
        active: usize,
        gate: bool,
    ) -> Option<String> {
        let (planned, mut skipped, refused) = self.plan_restore(tabs, active, gate);
        let mut open = Vec::new();
        let mut used_lines = 0usize;
        for &k in &planned {
            let p = &tabs[k].0;
            // A tab that is already open stays where its reader is.
            let was_open = self.find_tab(p).is_some();
            if was_open {
                self.open_now(p);
            } else {
                let remaining = self.restore_line_budget.saturating_sub(used_lines);
                let Some((view, lines)) = self.make_restored_view(p, remaining) else {
                    skipped.push((k, RestoreLimit::Lines));
                    continue;
                };
                used_lines += lines;
                if self.begin_open(p) {
                    self.finish_open(p, view);
                }
            }
            open.push(k);
            if !was_open
                && let Some(pos) = positions.get(k).copied().flatten()
                && self.view.path() == Some(p.as_path())
            {
                self.view.restore_position(pos);
            }
        }
        if !skipped.is_empty() {
            skipped.sort_unstable_by_key(|&(k, _)| k);
            for &(k, _) in &skipped {
                let (p, g) = &tabs[k];
                self.not_reopened
                    .push((p.clone(), *g, positions.get(k).copied().flatten()));
            }
            let n = skipped.len();
            let byte_limited = skipped.iter().any(|&(_, l)| l == RestoreLimit::Bytes);
            let line_limited = skipped.iter().any(|&(_, l)| l == RestoreLimit::Lines);
            let msg = if byte_limited && !line_limited {
                let b = self.restore_budget;
                let limit = if b >= 1 << 20 && b.is_multiple_of(1 << 20) {
                    format!("{} MB", b >> 20)
                } else {
                    human_size(b as usize)
                };
                if n == 1 {
                    format!(
                        "1 file was not reopened: together with the others it would be larger \
                         than {limit}. It stays in the workspace."
                    )
                } else {
                    format!(
                        "{n} files were not reopened: together they are larger than {limit}. \
                         They stay in the workspace."
                    )
                }
            } else if line_limited && !byte_limited {
                let limit = match self.restore_line_budget {
                    1 => "1 text line".into(),
                    l if l >= 1_000_000 && l.is_multiple_of(1_000_000) => {
                        format!("{} million text lines", l / 1_000_000)
                    }
                    l => format!("{l} text lines"),
                };
                if n == 1 {
                    format!(
                        "1 file was not reopened: together with the others it would exceed the \
                         restore limit of {limit}. It stays in the workspace."
                    )
                } else {
                    format!(
                        "{n} files were not reopened: together they would exceed the restore \
                         limit of {limit}. They stay in the workspace."
                    )
                }
            } else {
                format!(
                    "{n} files were not reopened: the restore's file-size or text-line limit \
                     was reached. They stay in the workspace."
                )
            };
            if self.nag.is_none() {
                self.nag = Some(Nag::error(msg));
            } else {
                self.notice = Some(msg);
            }
        }
        // Sides: only when both sides end up with tabs.
        let right: Vec<usize> = open
            .iter()
            .map(|&k| &tabs[k])
            .filter(|(_, g)| *g == 1)
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
        if let Some((p, _)) = tabs.get(active)
            && let Some(i) = self.find_tab(p)
        {
            self.activate(i);
        }
        let paths: Vec<&Path> = refused.iter().map(|&(k, _)| tabs[k].0.as_path()).collect();
        refused.first().map(|&(_, why)| refused_notice(why, &paths))
    }

    /// Switch to workspace `w` read from a workspace file (which may come
    /// from anyone): folders and tabs that content may not open are
    /// refused before anything touches them.
    /// True if the workspace's folder is now shown.
    pub fn apply_workspace(&mut self, w: Workspace) -> bool {
        self.switch_workspace(w, true)
    }

    /// Switch to workspace `w`: remember the current folder's tabs, close
    /// tabs that are saved and outside the new folder (unsaved ones stay —
    /// nothing is ever dropped), show the new folder and open its files.
    /// `gate`: `w` comes from a workspace file, not from our own store.
    /// Returns false (with a message) if the workspace's folder can't be shown.
    fn switch_workspace(&mut self, mut w: Workspace, gate: bool) -> bool {
        let mut refused: Vec<(PathBuf, &'static str)> = Vec::new();
        if gate {
            if let Some(Err(why)) = w.folders.first().map(|f| crate::links::content_may_open(f)) {
                self.nag = Some(Nag::error(format!(
                    "The workspace's folder was not opened ({why}): {}",
                    w.folders[0].display()
                )));
                return false;
            }
            w.folders
                .retain(|f| match crate::links::content_may_open(f) {
                    Ok(()) => true,
                    Err(why) => {
                        refused.push((f.clone(), why));
                        false
                    }
                });
        }
        let Some(root) = w.folders.first().filter(|f| f.is_dir()).cloned() else {
            self.nag = Some(Nag::error(
                "The workspace's folder doesn't exist any more.".into(),
            ));
            return false;
        };
        self.persist_workspace();
        // Those belonged to the folder that is left (saved just above).
        self.not_reopened.clear();
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
        let tabs_note = self.open_workspace_tabs(&w.tabs, &w.positions, w.active, gate);
        if self.view.path().is_none() && self.tabs.len() == 1 {
            self.view = Box::new(MessageView::folder_opened());
            self.focus = Focus::Tree;
        } else {
            self.focus = Focus::Main;
        }
        let kept = (keep_dirty > 0).then(|| {
            format!("Kept {keep_dirty} tab(s) with unsaved edits from the previous folder")
        });
        let folders_note = refused.first().map(|&(_, why)| {
            let paths: Vec<&Path> = refused.iter().map(|(p, _)| p.as_path()).collect();
            refused_notice(why, &paths)
        });
        let notes: Vec<String> = [kept, folders_note, tabs_note]
            .into_iter()
            .flatten()
            .collect();
        if !notes.is_empty() {
            self.notice = Some(notes.join(". "));
        }
        self.session_dirty = true;
        self.file_index = None;
        self.pending = true;
        true
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
        // Our own store, written from what the reader opened: not gated.
        if self.switch_workspace(w, false) {
            // A plain folder now, not the workspace file it may have come from.
            self.workspace_file = None;
        }
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

    /// Show or hide the menu bar (tests: most start without it).
    #[cfg(test)]
    pub fn set_menu_bar(&mut self, on: bool) {
        self.menu_bar_on = on;
        self.pending = true;
    }

    /// Open menu `i` of the menu bar (`keyboard`: select its first item).
    fn open_bar_menu(&mut self, i: usize, keyboard: bool) {
        let i = i % menubar::TITLES.len();
        // Set first: a hidden bar shows while one of its menus is open.
        self.bar_menu = Some(i);
        let Some(bar) = self.layout().menubar else {
            self.bar_menu = None;
            return;
        };
        let rects = {
            let mut ctx = ctx!(self);
            menubar::title_rects(bar, &mut ctx)
        };
        let x = rects.get(i).map_or(bar.x, |r| r.x);
        let mut m = Menu::with_items(
            x as f32,
            bar.bottom() as f32,
            menubar::items(i, self.menu_bar_on),
        );
        if keyboard {
            m.select_first();
        }
        self.menu = Some(m);
    }

    /// The menu bar title at this mouse position, if any.
    fn bar_title_at(&mut self, x: f32, y: f32) -> Option<usize> {
        let bar = self.layout().menubar?;
        if (y as i32) < bar.y || (y as i32) >= bar.bottom() {
            return None;
        }
        let mut ctx = ctx!(self);
        menubar::title_rects(bar, &mut ctx)
            .iter()
            .position(|r| (x as i32) >= r.x && (x as i32) < r.right())
    }

    /// "View: Hide / Show Menu Bar": switch now and record it in the
    /// settings file (its `menu_bar` line).
    fn toggle_menu_bar(&mut self) {
        self.menu_bar_on = !self.menu_bar_on;
        let (value, what) = if self.menu_bar_on {
            ("on", "Menu bar shown")
        } else {
            ("off", "Menu bar hidden (Alt+F or F10 opens it)")
        };
        self.save_setting("menu_bar", value, what);
    }

    /// Ctrl+Shift+H: hidden files in the tree, Ctrl+P and find in folder.
    /// Remembered in settings.
    fn toggle_hidden_files(&mut self) {
        let on = !self.tree.show_hidden();
        self.tree.set_show_hidden(on);
        self.file_index = None;
        let (value, what) = if on {
            ("on", "Hidden files shown")
        } else {
            ("off", "Hidden files not shown")
        };
        self.save_setting("hidden_files", value, what);
    }

    /// What the tree header says: the kind of thing open and, for a
    /// workspace file, its name. (The folder names are the rows below.)
    pub fn tree_header_text(&self) -> String {
        match &self.workspace_file {
            Some(f) => {
                let name = f
                    .file_stem()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                format!("WORKSPACE · {name}")
            }
            None if self.tree.roots().len() > 1 => "FOLDERS".into(),
            None => "FOLDER".into(),
        }
    }

    /// The tree header's menu: everything about the folder or workspace.
    fn open_tree_header_menu(&mut self, header: Rect) {
        let items = vec![
            ("Open Folder…", MenuAction::Keys("ctrl+shift+o")),
            ("Open Recent…", MenuAction::Keys("ctrl+r")),
            (
                "Open Workspace…",
                MenuAction::Palette(Action::OpenWorkspace),
            ),
            ("Add Folder to Workspace…", MenuAction::AddFolder),
            (
                "Save Workspace As…",
                MenuAction::Palette(Action::SaveWorkspace),
            ),
            ("Close Folder", MenuAction::CloseFolder),
            ("Show / Hide Hidden Files", MenuAction::Keys("ctrl+shift+h")),
            ("Hide File Tree", MenuAction::Keys("ctrl+\\")),
        ];
        let x = header.x as f32 + 6.0 * self.scale;
        self.menu = Some(Menu::new(x, header.bottom() as f32, items));
    }

    /// "File: Close Folder": remember the folder's tabs (as switching to
    /// another folder does), close the saved tabs, keep tabs with unsaved
    /// edits, and show the start page. The next launch starts without a
    /// folder. Nothing on disk is touched.
    pub fn close_folder(&mut self) {
        let Some(root) = self.tree.root().map(Path::to_path_buf) else {
            self.notice = Some("No folder is open".into());
            return;
        };
        if self.nag.is_some() {
            return;
        }
        // Saved before any tab closes, so reopening the folder brings them back.
        self.persist_workspace();
        // An explicit close is what the next launch should see, also in a
        // window started with a path (`litedit .`), which otherwise leaves
        // the saved session alone.
        self.owns_workspace = true;
        self.not_reopened.clear();
        let close: Vec<u64> = (0..self.tabs.len())
            .filter(|&i| !self.tab_view(i).is_dirty())
            .map(|i| self.tabs[i].id)
            .collect();
        let kept = self.tabs.len() - close.len();
        for id in close {
            self.remove_tab(id);
        }
        for t in &mut self.tabs {
            t.group = 0;
        }
        self.group_shown = [self.tabs.get(self.active).map(|t| t.id), None];
        self.tree.clear();
        self.workspace_file = None;
        self.focus = Focus::Main;
        self.file_index = None;
        self.refresh_start_page();
        let mut note = format!(
            "Closed {}; its tabs come back when you open it again",
            display_name(&root)
        );
        if kept > 0 {
            note.push_str(&format!(". Kept {kept} tab(s) with unsaved edits"));
        }
        self.notice = Some(note);
        self.session_dirty = true;
        self.pending = true;
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
                self.workspace_file = Some(path.clone());
                self.session_dirty = true;
                self.notice = Some(format!("Saved workspace {}", display_name(&path)));
                self.after_fs_change(&path);
            }
            Err(e) => self.nag = Some(Nag::error(format!("Could not save the workspace: {e}"))),
        }
    }

    fn suggested_workspace_path(&self) -> PathBuf {
        if let Some(f) = &self.workspace_file {
            return f.clone();
        }
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
        let redraw = self.handle_input(input);
        self.nav_sync();
        redraw
    }

    fn handle_input(&mut self, input: Input) -> bool {
        let mut redraw = self.handle_inner(&input);
        if let Some(n) = self.tree.refused.take() {
            self.notice = Some(n);
            redraw = true;
        }
        self.sync_nag_owner();
        // Requests the app handles itself (opening a file from a link or the tree).
        self.drain_queued_opens();
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
            let before = self.nav_here();
            self.open(&p);
            if let Some(l) = line {
                self.view.goto_line(l);
                // One step back from a search result, even in this file.
                self.nav_jumped(before);
            }
            return true;
        }
        if redraw {
            self.pending = true;
        }
        redraw
    }

    /// Result of adding / removing litedit in "Open with" (from the window).
    pub fn open_with_done(&mut self, add: bool, result: Result<(), String>) {
        match result {
            Ok(()) if add => {
                self.nag = Some(Nag::new(
                    "Added. To make litedit the default: right-click a .md file in Explorer → Open with → Choose another app → litedit → Always.".into(),
                    vec![("OK", NagAction::Dismiss)],
                    false,
                ));
            }
            Ok(()) => self.notice = Some("Removed litedit from \"Open with\"".into()),
            Err(e) => self.nag = Some(Nag::error(format!("Could not change \"Open with\": {e}"))),
        }
        self.pending = true;
    }

    /// Files from a later launch of litedit (one window). They open as tabs;
    /// while a question is showing they wait, so its answer can't land on
    /// the wrong tab and nothing is dropped.
    pub fn open_from_other_launch(&mut self, paths: Vec<PathBuf>) {
        let mut refused: Vec<(PathBuf, &'static str)> = Vec::new();
        for p in paths {
            // Anything on this PC can write to the pipe: check before
            // `is_file` follows a link. (A launch whose own file is behind
            // such a link opens its own window instead; see
            // `single::forwardable`.)
            if let Err(why) = crate::links::content_may_open(&p) {
                refused.push((p, why));
                continue;
            }
            if p.is_file()
                && !workspace::is_workspace_file(&p)
                && !self.queued_opens.contains(&p)
                && self.queued_opens.len() < 200
            {
                self.queued_opens.push(p);
            }
        }
        if let Some(&(_, why)) = refused.first() {
            let paths: Vec<&Path> = refused.iter().map(|(p, _)| p.as_path()).collect();
            self.notice = Some(refused_notice(why, &paths));
        }
        self.drain_queued_opens();
        self.pending = true;
    }

    fn drain_queued_opens(&mut self) {
        // Wait while anything modal is open: a question, the palette (its
        // command would land on the newly opened file) or a menu.
        if self.queued_opens.is_empty()
            || self.nag.is_some()
            || self.palette.is_some()
            || self.menu.is_some()
        {
            return;
        }
        // Files arrived while the window was closing: stay open for them
        // (the launch that sent them has already exited).
        if self.requests.contains(&Request::Quit) {
            self.requests.retain(|r| *r != Request::Quit);
            self.quit_skip.clear();
            self.notice = Some("A file was opened while closing; litedit stays open".into());
        }
        for p in std::mem::take(&mut self.queued_opens) {
            if p.is_file() {
                self.open(&p);
            }
        }
        self.sync_nag_owner();
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
                Input::Key { .. }
                    | Input::MouseDown { .. }
                    | Input::MouseMove { .. }
                    | Input::Text(_)
            )
        {
            // A menu bar menu: another title under the mouse switches to its
            // menu; a click on the open menu's own title closes it.
            if let Some(open) = self.bar_menu {
                match input {
                    Input::MouseMove { x, y } => {
                        self.mouse = (*x, *y);
                        if let Some(j) = self.bar_title_at(*x, *y) {
                            if j != open {
                                self.open_bar_menu(j, false);
                            }
                            return true;
                        }
                    }
                    Input::MouseDown { x, y, .. } => {
                        if let Some(j) = self.bar_title_at(*x, *y) {
                            if j == open {
                                self.menu = None;
                                self.bar_menu = None;
                            } else {
                                self.open_bar_menu(j, false);
                            }
                            return true;
                        }
                    }
                    _ => {}
                }
            }
            match self.menu.as_mut().map(|m| m.handle(input)) {
                Some(MenuResult::Run(a)) => {
                    self.menu = None;
                    self.bar_menu = None;
                    self.run_menu(a);
                }
                Some(MenuResult::Close) => {
                    self.menu = None;
                    self.bar_menu = None;
                }
                Some(MenuResult::Prev) | Some(MenuResult::Next) if self.bar_menu.is_some() => {
                    let n = menubar::TITLES.len();
                    let open = self.bar_menu.unwrap_or(0);
                    let next = if matches!(input, Input::Key { key: Key::Left, .. }) {
                        (open + n - 1) % n
                    } else {
                        (open + 1) % n
                    };
                    self.open_bar_menu(next, true);
                }
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
        // Tree keys for the selection: Ctrl+C / V / D, Delete of several.
        if self.focus == Focus::Tree
            && let Input::Key { key, mods } = input
            && self.tree_key(key, *mods)
        {
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
            // The menu bar from the keyboard: F10, or Alt + a title's first
            // letter (Alt+F File, Alt+E Edit, Alt+V View, Alt+G Go, Alt+H Help).
            Input::Key {
                key: Key::F(10),
                mods,
            } if !mods.ctrl && !mods.alt && !mods.shift => {
                self.open_bar_menu(0, true);
                return true;
            }
            Input::Key {
                key: Key::Char(c),
                mods,
            } if mods.alt && !mods.ctrl && !mods.shift && menubar::mnemonic(*c).is_some() => {
                if let Some(i) = menubar::mnemonic(*c) {
                    self.open_bar_menu(i, true);
                }
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
                let rescan = paths.is_empty();
                self.file_details_changed(paths);
                self.file_index = None;
                let mut changed = if rescan {
                    self.tree.reload_all();
                    true
                } else {
                    self.tree.invalidate(paths)
                };
                // Comparisons showing a changed file recompute.
                let hits =
                    |v: &dyn View| rescan || v.watched_paths().iter().any(|w| paths.contains(w));
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
                    && (rescan || paths.iter().any(|c| c == &p))
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
                    if !rescan && !paths.iter().any(|c| c == &p) {
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
            Input::Key {
                key: k @ (Key::Left | Key::Right),
                mods,
            } if mods.alt && !mods.ctrl && !mods.shift => {
                self.navigate(*k == Key::Left);
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
                        if self.view.can_save()
                            && let Some(p) = self.view.path()
                        {
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
                        self.toggle_hidden_files();
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
        // The menu bar: a click on a title opens its menu.
        if let Some(bar) = layout.menubar {
            match input {
                Input::MouseDown { x, y, button, .. }
                    if (*y as i32) >= bar.y && (*y as i32) < bar.bottom() =>
                {
                    self.mouse = (*x, *y);
                    if *button == MouseButton::Left
                        && let Some(j) = self.bar_title_at(*x, *y)
                    {
                        self.bar_hover = None;
                        self.open_bar_menu(j, false);
                    }
                    return true;
                }
                Input::MouseMove { x, y } if (*y as i32) >= bar.y && (*y as i32) < bar.bottom() => {
                    self.mouse = (*x, *y);
                    self.cursor = Cursor::Default;
                    let h = self.bar_title_at(*x, *y);
                    let changed = h != self.bar_hover;
                    self.bar_hover = h;
                    return changed;
                }
                Input::MouseMove { .. } | Input::MouseLeave if self.bar_hover.is_some() => {
                    self.bar_hover = None;
                    self.pending = true;
                }
                _ => {}
            }
        }
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

        // The tree header: a click (either button) opens its menu.
        if let Some(hr) = layout.tree_header
            && !self.tree.dragging()
            && !self.dragging_divider
        {
            let inside = |x: f32, y: f32| {
                x >= hr.x as f32
                    && x < hr.right() as f32
                    && y >= hr.y as f32
                    && y < hr.bottom() as f32
            };
            match input {
                Input::MouseDown { x, y, button, .. } if inside(*x, *y) => {
                    self.mouse = (*x, *y);
                    if *button != MouseButton::Middle {
                        self.open_tree_header_menu(hr);
                    }
                    return true;
                }
                Input::MouseMove { x, y } if inside(*x, *y) => {
                    self.cursor = Cursor::Default;
                    let was = self.header_hover;
                    self.header_hover = true;
                    if let Some(t) = layout.tree {
                        let mut ctx = ctx!(self);
                        self.tree.handle(&Input::MouseLeave, t, &mut ctx);
                    }
                    return !was;
                }
                Input::MouseMove { .. } | Input::MouseLeave => self.header_hover = false,
                _ => {}
            }
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
                self.ask_move_many(from, dir);
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
        // A paste / duplicate in the tree continues here, a slice at a time.
        let copy = self.copy_tick(budget);
        t.more |= copy.more;
        t.redraw |= copy.redraw;
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
        self.sync_file_details();
        let (status_tree, status_details) = (self.status_tree_items(), self.status_file_details());
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
        let (bar_open, bar_hover) = (self.bar_menu, self.bar_hover);
        let header_text = self.tree_header_text();
        let header_hover = self.header_hover;
        let current = self.view.path().map(Path::to_path_buf);
        let focus = self.focus;
        let mut ctx = ctx!(self);

        if let Some(bar) = layout.menubar {
            canvas.set_clip(bar);
            canvas.fill_rect(bar, theme::p().background2);
            canvas.fill_rect(
                Rect::new(bar.x, bar.bottom() - 1, bar.w, 1),
                theme::p().divider,
            );
            let lh = ctx.ui_metrics().line_height as i32;
            let pad = ctx.px(10.0) as i32;
            let rects = menubar::title_rects(bar, &mut ctx);
            for (i, (t, r)) in menubar::TITLES.iter().zip(rects).enumerate() {
                let r = Rect::new(r.x, r.y, r.w, r.h - 1);
                if bar_open == Some(i) {
                    canvas.fill_rect(r, theme::p().selection);
                } else if bar_hover == Some(i) {
                    canvas.fill_rect(r, theme::p().line_highlight);
                }
                ctx.label(
                    canvas,
                    t,
                    r.x + pad,
                    r.y + (r.h - lh) / 2,
                    theme::p().text,
                    r.w,
                    FontKind::Ui,
                );
            }
        }
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
        if let Some(hr) = layout.tree_header {
            canvas.set_clip(hr);
            canvas.fill_rect(
                hr,
                if header_hover {
                    theme::p().line_highlight
                } else {
                    theme::p().background2
                },
            );
            canvas.fill_rect(
                Rect::new(hr.x, hr.bottom() - 1, hr.w, 1),
                theme::p().divider,
            );
            let pad = ctx.px(10.0) as i32;
            let lh = ctx.ui_metrics().line_height as i32;
            // "⋯" at the right: the header is a menu button.
            let dot = ctx.px(1.6).max(1.0);
            let step = ctx.px(5.0);
            let cx = hr.right() as f32 - pad as f32 - step;
            let cy = hr.y as f32 + hr.h as f32 / 2.0;
            for k in -1..=1 {
                canvas.fill_circle(cx + k as f32 * step, cy, dot, theme::p().text);
            }
            let room = (cx - step * 2.0) as i32 - hr.x - pad;
            ctx.label(
                canvas,
                &header_text,
                hr.x + pad,
                hr.y + (hr.h - lh) / 2,
                theme::p().dim,
                room.max(0),
                FontKind::Ui,
            );
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
        items.extend(status_tree);
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
        let mut right = layout.status.right() - pad_x;
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
            right -= w + pad_x * 2;
        }
        // The active file's size and date, at the right end when there is
        // room after the other items.
        if let Some(d) = status_details {
            let w = ctx.text_width(&d, FontKind::Ui) as i32;
            if right - w >= x {
                ctx.label(
                    canvas,
                    &d,
                    right - w,
                    y,
                    theme::p().dim,
                    w + 2,
                    FontKind::Ui,
                );
            }
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

/// What the status bar says when content asked to open `paths` and was
/// refused (`why` is the first one's reason).
fn refused_notice(why: &str, paths: &[&Path]) -> String {
    let first = paths
        .first()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    match paths.len() {
        0 | 1 => format!("Not opened ({why}): {first}"),
        n => format!("Not opened ({why}): {first} and {} more", n - 1),
    }
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
