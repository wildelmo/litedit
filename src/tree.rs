//! File tree side panel (lite-xl `plugins/treeview.lua`).
//!
//! Directories are read lazily (only when expanded) so opening a huge folder
//! is instant. Symlinked / junction directories are shown but never expanded,
//! so the tree cannot loop or wander outside the opened folder.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::canvas::{Canvas, Rect};
use crate::input::{Cursor, Input, Key, MouseButton, Request};
use crate::theme;
use crate::ui::{Ctx, FontKind};
use crate::view::{Scroll, draw_vscrollbar};

/// Names never shown (lite-xl's default ignore list, plus Windows clutter).
pub const IGNORED: &[&str] = &[
    ".git",
    ".svn",
    ".hg",
    "CVS",
    "node_modules",
    "__pycache__",
    ".cache",
    ".DS_Store",
    "Thumbs.db",
    "desktop.ini",
    "$RECYCLE.BIN",
    "System Volume Information",
];
/// Extra names from the `ignore` setting (shared with the watcher thread).
static EXTRA_IGNORED: std::sync::RwLock<Vec<String>> = std::sync::RwLock::new(Vec::new());

/// Set the user's extra ignored names (setting `ignore`).
pub fn set_extra_ignored(names: Vec<String>) {
    if let Ok(mut v) = EXTRA_IGNORED.write() {
        *v = names;
    }
}

/// Hidden from the tree, quick open, find in folder and the watcher?
pub fn is_ignored(name: &str) -> bool {
    IGNORED.iter().any(|i| i.eq_ignore_ascii_case(name))
        || EXTRA_IGNORED
            .read()
            .is_ok_and(|v| v.iter().any(|i| i.eq_ignore_ascii_case(name)))
}

/// At most this many entries are listed per directory.
pub const MAX_ENTRIES: usize = 5000;
/// At most this many entries of one directory are read at all (well above
/// what is shown; a folder with millions of entries must not be walked,
/// stat-ed and sorted in full).
pub const MAX_READ: usize = 50_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    /// Symlink or junction: shown, never expanded.
    pub is_link: bool,
}

/// One visible row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub entry: Entry,
    pub depth: usize,
    pub expanded: bool,
}

pub struct Tree {
    /// Folders shown (the first is the main folder; more can be added to the
    /// workspace, like lite-xl's "add directory").
    roots: Vec<PathBuf>,
    entries: HashMap<PathBuf, Vec<Entry>>,
    /// Directories whose listing was truncated: (total seen, reading
    /// stopped early so the real total is higher).
    truncated: HashMap<PathBuf, (usize, bool)>,
    expanded: HashSet<PathBuf>,
    rows: Vec<Row>,
    rows_dirty: bool,
    pub visible: bool,
    pub width: f32,
    /// Keyboard cursor: the entry last clicked or moved to. Kept by path, so
    /// a refresh can never slide it (or the selection) onto another file.
    cursor: Option<PathBuf>,
    /// Its row when last seen: where the cursor lands if the entry is gone.
    cursor_row: usize,
    /// Selected entries, by path; always visible rows. A plain click or an
    /// arrow key leaves just the cursor's entry selected.
    sel: HashSet<PathBuf>,
    /// Where a Shift+click range starts.
    anchor: Option<PathBuf>,
    hover: Option<usize>,
    scroll: Scroll,
    view_h: f32,
    show_hidden: bool,
    /// Left button pressed on an entry: a drag starts once the mouse moves.
    press: Option<Press>,
    /// Folder the dragged entry would move into (valid targets only).
    drop_target: Option<PathBuf>,
    /// A finished drag: (entries, destination folder). The app asks first.
    pub requested_move: Option<(Vec<PathBuf>, PathBuf)>,
    /// An entry that is a link was not opened: why (the app shows it).
    pub refused: Option<String>,
}

struct Press {
    path: PathBuf,
    y: f32,
    dragging: bool,
    /// Pressed on one of several selected entries: they stay selected so a
    /// drag can move them all; a release without a drag selects just this
    /// one and opens it.
    deferred: bool,
}

/// Can these entries be dropped on folder `dir`? Never onto or into one of
/// them, and at least one must actually move.
pub fn can_move_all_into(from: &[PathBuf], dir: &Path) -> bool {
    !from.iter().any(|f| dir.starts_with(f)) && from.iter().any(|f| f.parent() != Some(dir))
}

fn is_hidden(name: &str, meta: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const HIDDEN: u32 = 0x2;
        const SYSTEM: u32 = 0x4;
        if meta.file_attributes() & (HIDDEN | SYSTEM) != 0 {
            return true;
        }
    }
    let _ = meta;
    name.starts_with('.') && name != "." && name != ".."
}

/// Read one directory: ignored names removed, dirs first, case-insensitive,
/// cut at MAX_ENTRIES (the tree's display limit). Returns (entries, total).
#[cfg(test)]
pub fn read_dir_sorted(dir: &Path, show_hidden: bool) -> (Vec<Entry>, usize) {
    let (out, total, _) = read_dir_shown(dir, show_hidden);
    (out, total)
}

/// [`read_dir_sorted`] that also says whether reading stopped at
/// [`MAX_READ`] (then `total` is only what was seen: "more than").
pub fn read_dir_shown(dir: &Path, show_hidden: bool) -> (Vec<Entry>, usize, bool) {
    let (mut out, total, more) = read_dir_limited(dir, show_hidden, MAX_READ);
    out.truncate(MAX_ENTRIES);
    (out, total, more)
}

/// Read at most `limit` entries of `dir` from the file system (ignored and
/// hidden ones count: they cost a look too), sorted. The last value is true
/// when more entries were left unread.
pub fn read_dir_limited(dir: &Path, show_hidden: bool, limit: usize) -> (Vec<Entry>, usize, bool) {
    let mut out = Vec::new();
    let Ok(mut rd) = std::fs::read_dir(dir) else {
        return (out, 0, false);
    };
    for e in rd.by_ref().take(limit).flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if is_ignored(&name) {
            continue;
        }
        let Ok(ft) = e.file_type() else { continue };
        let is_link = ft.is_symlink();
        let path = e.path();
        // For links, look at the target only to decide folder vs file.
        let meta = if is_link {
            // Never touch a link that points at a network share or device
            // path: even reading its details authenticates to the server.
            if crate::links::link_reaches_unsafe(&path) {
                None
            } else {
                std::fs::metadata(&path).ok()
            }
        } else {
            e.metadata().ok()
        };
        let Some(meta) = meta else {
            if is_link {
                out.push(Entry {
                    name,
                    path,
                    is_dir: false,
                    is_link,
                }); // dangling link
            }
            continue;
        };
        if !show_hidden && is_hidden(&name, &meta) {
            continue;
        }
        out.push(Entry {
            name,
            path,
            is_dir: meta.is_dir(),
            is_link,
        });
    }
    let more = rd.next().is_some();
    // Dirs first, then by lowercase name (each name lowercased once).
    out.sort_by_cached_key(|e| (!e.is_dir, e.name.to_lowercase()));
    let total = out.len();
    (out, total, more)
}

impl Default for Tree {
    fn default() -> Self {
        Self::new()
    }
}

impl Tree {
    pub fn new() -> Self {
        Self {
            roots: Vec::new(),
            entries: HashMap::new(),
            truncated: HashMap::new(),
            expanded: HashSet::new(),
            rows: Vec::new(),
            rows_dirty: true,
            visible: true,
            width: 260.0,
            cursor: None,
            cursor_row: 0,
            sel: HashSet::new(),
            anchor: None,
            hover: None,
            scroll: Scroll::default(),
            view_h: 0.0,
            show_hidden: true,
            press: None,
            drop_target: None,
            requested_move: None,
            refused: None,
        }
    }

    pub fn root(&self) -> Option<&Path> {
        self.roots.first().map(PathBuf::as_path)
    }

    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// The shown folder that contains `p` (the deepest one).
    pub fn root_of(&self, p: &Path) -> Option<&Path> {
        self.roots
            .iter()
            .filter(|r| p.starts_with(r))
            .max_by_key(|r| r.components().count())
            .map(PathBuf::as_path)
    }

    /// Add another folder to the tree. False if it is already shown.
    pub fn add_root(&mut self, dir: &Path) -> bool {
        if self.roots.iter().any(|r| r == dir) {
            return false;
        }
        self.roots.push(dir.to_path_buf());
        self.expanded.insert(dir.to_path_buf());
        self.rows_dirty = true;
        true
    }

    /// Take a folder out of the tree (never the last one).
    pub fn remove_root(&mut self, dir: &Path) -> bool {
        if self.roots.len() < 2 {
            return false;
        }
        let before = self.roots.len();
        self.roots.retain(|r| r != dir);
        self.clear_selection();
        self.rows_dirty = true;
        self.roots.len() != before
    }

    /// Show no folder at all ("Close Folder").
    pub fn clear(&mut self) {
        self.roots.clear();
        self.entries.clear();
        self.truncated.clear();
        self.expanded.clear();
        self.clear_selection();
        self.scroll = Scroll::default();
        self.press = None;
        self.drop_target = None;
        self.hover = None;
        self.rows_dirty = true;
    }

    pub fn is_shown(&self) -> bool {
        self.visible && !self.roots.is_empty()
    }

    pub fn set_root(&mut self, dir: &Path) {
        self.roots = vec![dir.to_path_buf()];
        self.entries.clear();
        self.truncated.clear();
        self.expanded.clear();
        self.expanded.insert(dir.to_path_buf());
        self.clear_selection();
        self.scroll = Scroll::default();
        self.rows_dirty = true;
    }

    /// Re-read every directory (after the ignore list changed).
    pub fn reload_all(&mut self) {
        self.entries.clear();
        self.rows_dirty = true;
    }

    /// Are hidden files listed (names starting with a dot, and what
    /// Windows marks hidden or system)? On unless the user turned it off.
    pub fn show_hidden(&self) -> bool {
        self.show_hidden
    }

    pub fn set_show_hidden(&mut self, on: bool) {
        if self.show_hidden != on {
            self.show_hidden = on;
            self.entries.clear();
            self.rows_dirty = true;
        }
    }

    fn children(&mut self, dir: &Path) -> Vec<Entry> {
        if !self.entries.contains_key(dir) {
            let (list, total, more) = read_dir_shown(dir, self.show_hidden);
            if total > list.len() || more {
                self.truncated.insert(dir.to_path_buf(), (total, more));
            } else {
                self.truncated.remove(dir);
            }
            self.entries.insert(dir.to_path_buf(), list);
        }
        self.entries[dir].clone()
    }

    pub fn rows(&mut self) -> &[Row] {
        if self.rows_dirty {
            let mut rows = Vec::new();
            for root in self.roots.clone() {
                let name = root
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| root.display().to_string());
                rows.push(Row {
                    entry: Entry {
                        name,
                        path: root.clone(),
                        is_dir: true,
                        is_link: false,
                    },
                    depth: 0,
                    expanded: true,
                });
                self.push_children(&root, 1, &mut rows);
            }
            self.rows = rows;
            self.rows_dirty = false;
            self.resync_selection();
        }
        &self.rows
    }

    /// After the rows were rebuilt: the selection keeps only entries that
    /// are still shown (nothing out of sight is ever acted on), and the
    /// cursor follows its entry to its new row.
    fn resync_selection(&mut self) {
        if !self.sel.is_empty() {
            let shown: HashSet<&Path> = self.rows.iter().map(|r| r.entry.path.as_path()).collect();
            self.sel.retain(|p| shown.contains(p.as_path()));
            if self.anchor.as_deref().is_some_and(|a| !shown.contains(a)) {
                self.anchor = None;
            }
        }
        let Some(cursor) = &self.cursor else { return };
        match self.rows.iter().position(|r| &r.entry.path == cursor) {
            Some(i) => self.cursor_row = i,
            None => {
                // Its entry is gone: the cursor stays at that place in the
                // list, but the entry now there is not selected.
                self.cursor_row = self.cursor_row.min(self.rows.len().saturating_sub(1));
                self.cursor = self.rows.get(self.cursor_row).map(|r| r.entry.path.clone());
                self.anchor = None;
            }
        }
    }

    fn clear_selection(&mut self) {
        self.cursor = None;
        self.cursor_row = 0;
        self.sel.clear();
        self.anchor = None;
    }

    fn cursor_index(&mut self) -> Option<usize> {
        self.rows();
        let cursor = self.cursor.as_ref()?;
        // Usually still where it was last seen.
        if self
            .rows
            .get(self.cursor_row)
            .is_some_and(|r| &r.entry.path == cursor)
        {
            return Some(self.cursor_row);
        }
        self.rows.iter().position(|r| &r.entry.path == cursor)
    }

    /// Select just row `i` and put the cursor there.
    fn select_only(&mut self, i: usize) {
        let Some(path) = self.rows().get(i).map(|r| r.entry.path.clone()) else {
            return;
        };
        self.sel.clear();
        self.sel.insert(path.clone());
        self.anchor = Some(path.clone());
        self.cursor = Some(path);
        self.cursor_row = i;
    }

    /// Ctrl+click: add row `i` to the selection or take it out. The
    /// workspace folders themselves are never part of a selection of
    /// several entries.
    fn toggle_selected(&mut self, i: usize) {
        let Some(row) = self.rows().get(i).cloned() else {
            return;
        };
        if row.depth == 0 {
            return;
        }
        let roots = &self.roots;
        self.sel.retain(|p| !roots.contains(p));
        let path = row.entry.path;
        if !self.sel.remove(&path) {
            self.sel.insert(path.clone());
        }
        self.anchor = Some(path.clone());
        self.cursor = Some(path);
        self.cursor_row = i;
    }

    /// Shift+click / Shift+arrow: select everything from where the range
    /// started to row `to` (workspace folders left out).
    fn select_range(&mut self, to: usize) {
        self.rows();
        let Some(target) = self.rows.get(to).map(|r| r.entry.path.clone()) else {
            return;
        };
        let from = match self.anchor.clone() {
            Some(a) => self.rows.iter().position(|r| r.entry.path == a),
            None => self.cursor_index(),
        }
        .unwrap_or(to);
        let (lo, hi) = (from.min(to), from.max(to));
        let range = self.rows.get(lo..=hi).unwrap_or(&[]);
        let mut sel: HashSet<PathBuf> = range
            .iter()
            .filter(|r| r.depth > 0)
            .map(|r| r.entry.path.clone())
            .collect();
        if sel.is_empty() {
            sel.insert(target.clone());
        }
        if self.anchor.is_none() {
            self.anchor = self.rows.get(from).map(|r| r.entry.path.clone());
        }
        self.sel = sel;
        self.cursor = Some(target);
        self.cursor_row = to;
    }

    /// Ctrl+A: select every shown entry of the folder the cursor is in.
    fn select_all_in_root(&mut self) {
        let at = self.cursor_index().unwrap_or(0);
        let rows = &self.rows;
        let Some(start) = rows
            .get(..=at)
            .and_then(|r| r.iter().rposition(|r| r.depth == 0))
        else {
            return;
        };
        let inside = rows.get(start + 1..).unwrap_or(&[]);
        let len = inside
            .iter()
            .position(|r| r.depth == 0)
            .unwrap_or(inside.len());
        self.sel = inside
            .iter()
            .take(len)
            .map(|r| r.entry.path.clone())
            .collect();
    }

    /// The selected entries, top to bottom.
    pub fn selection(&mut self) -> Vec<Row> {
        self.rows();
        self.rows
            .iter()
            .filter(|r| self.sel.contains(&r.entry.path))
            .cloned()
            .collect()
    }

    /// Are several entries selected?
    pub fn is_multi(&self) -> bool {
        self.sel.len() > 1
    }

    /// The path of the only selected entry (nothing when none or several).
    pub fn single_selected(&self) -> Option<&Path> {
        match self.sel.len() {
            1 => self.sel.iter().next().map(PathBuf::as_path),
            _ => None,
        }
    }

    pub fn selected_count(&self) -> usize {
        self.sel.len()
    }

    pub fn is_selected(&self, p: &Path) -> bool {
        self.sel.contains(p)
    }

    /// The entry the keyboard cursor is on (where Ctrl+V pastes).
    pub fn cursor_entry(&mut self) -> Option<Row> {
        let i = self.cursor_index()?;
        self.rows.get(i).cloned()
    }

    /// Select exactly these entries (those that are shown), e.g. what a
    /// move or paste just produced.
    pub fn select_paths(&mut self, paths: &[PathBuf]) {
        self.rows();
        let rows = &self.rows;
        let shown: Vec<(usize, &PathBuf)> = paths
            .iter()
            .filter_map(|p| Some((rows.iter().position(|r| &r.entry.path == p)?, p)))
            .collect();
        let Some((first, path)) = shown.iter().min_by_key(|(i, _)| *i) else {
            return;
        };
        self.cursor = Some((*path).clone());
        self.cursor_row = *first;
        self.anchor = self.cursor.clone();
        self.sel = shown.iter().map(|(_, p)| (*p).clone()).collect();
    }

    fn push_children(&mut self, dir: &Path, depth: usize, rows: &mut Vec<Row>) {
        if depth > 64 {
            return;
        }
        for e in self.children(dir) {
            let expanded = e.is_dir && !e.is_link && self.expanded.contains(&e.path);
            let path = e.path.clone();
            rows.push(Row {
                entry: e,
                depth,
                expanded,
            });
            if expanded {
                self.push_children(&path, depth + 1, rows);
            }
        }
    }

    pub fn toggle(&mut self, dir: &Path) {
        if self.roots.iter().any(|r| r == dir) {
            return; // the folders themselves stay open
        }
        if !self.expanded.remove(dir) {
            self.expanded.insert(dir.to_path_buf());
        }
        self.rows_dirty = true;
    }

    /// Files or folders changed on disk: forget cached listings of their
    /// parent folders. Returns true if the tree changed.
    pub fn invalidate(&mut self, paths: &[PathBuf]) -> bool {
        let mut changed = false;
        for p in paths {
            for dir in [Some(p.as_path()), p.parent()].into_iter().flatten() {
                if self.entries.remove(dir).is_some() {
                    changed = true;
                }
            }
        }
        if changed {
            self.rows_dirty = true;
        }
        changed
    }

    /// Expand parents so `file` is visible and select it.
    pub fn reveal(&mut self, file: &Path) {
        let Some(root) = self.root_of(file).map(Path::to_path_buf) else {
            return;
        };
        let mut p = file.parent();
        while let Some(dir) = p {
            if !dir.starts_with(&root) {
                break;
            }
            self.expanded.insert(dir.to_path_buf());
            p = dir.parent();
        }
        self.rows_dirty = true;
        match self.rows().iter().position(|r| r.entry.path == file) {
            Some(i) => self.select_only(i),
            None => self.clear_selection(),
        }
    }

    fn row_height(ctx: &Ctx) -> f32 {
        (ctx.ui_metrics().line_height + ctx.px(6.0)).round()
    }

    fn row_at(&mut self, rect: Rect, y: f32, ctx: &Ctx) -> Option<usize> {
        let rh = Self::row_height(ctx);
        let top = rect.y as f32 + ctx.px(theme::PADDING_Y) - self.scroll.y;
        if y < top {
            return None;
        }
        let i = ((y - top) / rh) as usize;
        (i < self.rows().len()).then_some(i)
    }

    /// The entry under window y, if any.
    pub fn entry_at(&mut self, rect: Rect, y: f32, ctx: &Ctx) -> Option<Row> {
        let i = self.row_at(rect, y, ctx)?;
        self.rows().get(i).cloned()
    }

    /// Open the entry on row `i`: folders expand / collapse; files open in
    /// the peek tab (`keep` = false: single click, Space) or stay open
    /// (`keep` = true: double-click, Enter).
    fn activate(&mut self, i: usize, keep: bool, ctx: &mut Ctx) {
        let Some(row) = self.rows().get(i).cloned() else {
            return;
        };
        self.select_only(i);
        if row.entry.is_dir {
            if !row.entry.is_link {
                self.toggle(&row.entry.path);
            }
        } else if row.entry.is_link
            && let Err(why) = crate::links::content_may_open(&row.entry.path)
        {
            // Whatever the folder holds may be a link to a network share
            // (an unpacked zip): opening it would contact that server.
            self.refused = Some(format!("Not opened ({why}): {}", row.entry.name));
        } else if keep {
            ctx.requests.push(Request::OpenPath(row.entry.path));
        } else {
            ctx.requests.push(Request::PeekPath(row.entry.path));
        }
    }

    fn ensure_selected_visible(&mut self, ctx: &Ctx) {
        let Some(s) = self.cursor_index() else { return };
        let rh = Self::row_height(ctx);
        let y = s as f32 * rh;
        if y < self.scroll.y {
            self.scroll.y = y;
        } else if y + rh > self.scroll.y + self.view_h - ctx.px(theme::PADDING_Y) * 2.0 {
            self.scroll.y = y + rh - self.view_h + ctx.px(theme::PADDING_Y) * 2.0;
        }
    }

    fn clamp(&mut self, ctx: &Ctx) {
        let rh = Self::row_height(ctx);
        let content = self.rows().len() as f32 * rh + ctx.px(theme::PADDING_Y) * 2.0;
        let max = (content - self.view_h).max(0.0);
        self.scroll.y = self.scroll.y.clamp(0.0, max);
        self.scroll.x = 0.0;
    }

    /// A drag from the tree is in progress (the app routes the mouse here
    /// even outside the panel until the button is released).
    pub fn dragging(&self) -> bool {
        self.press.is_some()
    }

    /// Forget a drag in progress (e.g. the tree was hidden).
    pub fn cancel_drag(&mut self) {
        self.press = None;
        self.drop_target = None;
    }

    /// The selected entry when exactly one is selected (F2 / Delete).
    pub fn selected_row(&mut self) -> Option<Row> {
        if self.sel.len() != 1 {
            return None;
        }
        self.selection().into_iter().next()
    }

    /// What a drag that started on `path` moves: every selected entry if it
    /// is one of several, otherwise just it.
    fn drag_set(&mut self, path: &Path) -> Vec<PathBuf> {
        if self.sel.len() > 1 && self.sel.contains(path) {
            self.selection()
                .into_iter()
                .filter(|r| r.depth > 0)
                .map(|r| r.entry.path)
                .collect()
        } else {
            vec![path.to_path_buf()]
        }
    }

    /// The folder a drop at (x, y) would move into, if valid.
    fn drop_folder(
        &mut self,
        rect: Rect,
        x: f32,
        y: f32,
        from: &[PathBuf],
        ctx: &Ctx,
    ) -> Option<PathBuf> {
        if x < rect.x as f32
            || x >= rect.right() as f32
            || y < rect.y as f32
            || y >= rect.bottom() as f32
        {
            return None;
        }
        let row = match self.row_at(rect, y, ctx) {
            Some(i) => self.rows()[i].clone(),
            // Below the last row: the root folder.
            None => self.rows().first()?.clone(),
        };
        let dir = if row.entry.is_dir && !row.entry.is_link {
            row.entry.path
        } else {
            row.entry.path.parent()?.to_path_buf()
        };
        can_move_all_into(from, &dir).then_some(dir)
    }

    /// Handle input inside the tree panel. Returns true if a redraw is needed.
    pub fn handle(&mut self, input: &Input, rect: Rect, ctx: &mut Ctx) -> bool {
        self.view_h = rect.h as f32;
        // Dragging an entry onto a folder.
        match input {
            Input::MouseMove { x, y } if self.press.is_some() => {
                let threshold = ctx.px(6.0);
                let (from, start, mut dragging) = {
                    let p = self.press.as_ref().expect("checked");
                    (p.path.clone(), p.y, p.dragging)
                };
                if !dragging && (y - start).abs() > threshold {
                    dragging = true;
                    if let Some(p) = self.press.as_mut() {
                        p.dragging = true;
                    }
                }
                if dragging {
                    let moving = self.drag_set(&from);
                    self.drop_target = self.drop_folder(rect, *x, *y, &moving, ctx);
                    self.hover = None;
                    *ctx.cursor = if self.drop_target.is_some() {
                        Cursor::Pointer
                    } else {
                        Cursor::Default
                    };
                    return true;
                }
            }
            Input::MouseUp {
                button: MouseButton::Left,
                ..
            } if self.press.is_some() => {
                let p = self.press.take().expect("checked");
                if let (true, Some(dir)) = (p.dragging, self.drop_target.take()) {
                    self.requested_move = Some((self.drag_set(&p.path), dir));
                } else if p.deferred
                    && !p.dragging
                    && let Some(i) = self.rows().iter().position(|r| r.entry.path == p.path)
                {
                    // A click (no drag) on one of several selected entries.
                    self.activate(i, false, ctx);
                }
                self.drop_target = None;
                return true;
            }
            Input::MouseLeave if self.press.as_ref().is_some_and(|p| p.dragging) => {
                return false;
            }
            _ => {}
        }
        match input {
            Input::MouseMove { y, .. } => {
                let h = self.row_at(rect, *y, ctx);
                *ctx.cursor = if h.is_some() {
                    Cursor::Pointer
                } else {
                    Cursor::Default
                };
                let changed = h != self.hover;
                self.hover = h;
                changed
            }
            Input::MouseLeave => {
                let changed = self.hover.is_some();
                self.hover = None;
                changed
            }
            Input::MouseDown {
                y,
                button: MouseButton::Left,
                clicks,
                ..
            } => {
                if let Some(i) = self.row_at(rect, *y, ctx) {
                    let row = self.rows()[i].clone();
                    // Ctrl / Shift+click only select: nothing opens, no
                    // folder expands, no drag starts.
                    if ctx.mods.shift {
                        self.select_range(i);
                        return true;
                    }
                    if ctx.mods.ctrl {
                        self.toggle_selected(i);
                        return true;
                    }
                    let deferred =
                        row.depth > 0 && self.sel.len() > 1 && self.sel.contains(&row.entry.path);
                    if row.depth > 0 {
                        self.press = Some(Press {
                            path: row.entry.path.clone(),
                            y: *y,
                            dragging: false,
                            deferred,
                        });
                    }
                    if deferred {
                        return true;
                    }
                    // A double-click's second click on a folder would undo
                    // the first (expand, collapse): only files react to it.
                    if *clicks < 2 || !row.entry.is_dir {
                        self.activate(i, *clicks >= 2, ctx);
                    }
                }
                true
            }
            Input::Wheel { .. } => {
                let before = self.scroll.y;
                self.scroll.wheel(input, Self::row_height(ctx));
                self.clamp(ctx);
                before != self.scroll.y
            }
            Input::Key { key, mods } => {
                let n = self.rows().len();
                if n == 0 {
                    return false;
                }
                if *key == Key::Char('a') && mods.ctrl && !mods.shift && !mods.alt {
                    self.select_all_in_root();
                    return true;
                }
                let sel = self.cursor_index().unwrap_or(0);
                let to = match key {
                    Key::Down => Some((sel + 1).min(n - 1)),
                    Key::Up => Some(sel.saturating_sub(1)),
                    Key::Home => Some(0),
                    Key::End => Some(n - 1),
                    Key::PageDown => Some((sel + 20).min(n - 1)),
                    Key::PageUp => Some(sel.saturating_sub(20)),
                    _ => None,
                };
                match (to, key) {
                    // Shift extends the selection; without it, one entry.
                    (Some(to), _) if mods.shift && !mods.ctrl => self.select_range(to),
                    (Some(to), _) => self.select_only(to),
                    (None, Key::Enter) => self.activate(sel, true, ctx),
                    (None, Key::Char(' ')) => self.activate(sel, false, ctx),
                    (None, Key::Right) => {
                        self.select_only(sel);
                        let row = self.rows()[sel].clone();
                        if row.entry.is_dir && !row.expanded && !row.entry.is_link {
                            self.toggle(&row.entry.path);
                        } else if row.expanded {
                            self.select_only((sel + 1).min(n - 1));
                        }
                    }
                    (None, Key::Left) => {
                        self.select_only(sel);
                        let row = self.rows()[sel].clone();
                        if row.expanded && row.depth > 0 {
                            self.toggle(&row.entry.path);
                        } else if row.depth > 0 {
                            // Jump to the parent folder.
                            let parent =
                                self.rows()[..sel].iter().rposition(|r| r.depth < row.depth);
                            self.select_only(parent.unwrap_or(0));
                        }
                    }
                    _ => return false,
                }
                self.ensure_selected_visible(ctx);
                self.clamp(ctx);
                true
            }
            _ => false,
        }
    }

    pub fn render(
        &mut self,
        canvas: &mut Canvas,
        rect: Rect,
        ctx: &mut Ctx,
        current: Option<&Path>,
        focused: bool,
    ) {
        self.view_h = rect.h as f32;
        self.clamp(ctx);
        canvas.fill_rect(rect, theme::p().background2);
        let rh = Self::row_height(ctx);
        let pad_y = ctx.px(theme::PADDING_Y);
        let indent = ctx.px(14.0);
        let lh = ctx.ui_metrics().line_height;
        let top = rect.y as f32 + pad_y - self.scroll.y;
        let first = ((self.scroll.y - pad_y) / rh).floor().max(0.0) as usize;
        let last =
            (((self.scroll.y + rect.h as f32) / rh).ceil() as usize + 1).min(self.rows().len());
        let rows: Vec<Row> = self.rows()[first..last].to_vec();
        let multi = self.sel.len() > 1;
        let cursor = if focused { self.cursor_index() } else { None };
        for (k, row) in rows.iter().enumerate() {
            let i = first + k;
            let y = top + i as f32 * rh;
            let row_rect = Rect::new(rect.x, y.round() as i32, rect.w, rh as i32);
            if Some(row.entry.path.as_path()) == current {
                canvas.fill_rect(row_rect, theme::p().line_highlight);
            }
            if self.drop_target.as_deref() == Some(row.entry.path.as_path()) {
                // Drop target: filled and outlined in the caret colour.
                canvas.fill_rect(row_rect, theme::p().selection);
                let t = ctx.px(1.5).max(1.0) as i32;
                let c = theme::p().caret;
                canvas.fill_rect(Rect::new(row_rect.x, row_rect.y, row_rect.w, t), c);
                canvas.fill_rect(
                    Rect::new(row_rect.x, row_rect.bottom() - t, row_rect.w, t),
                    c,
                );
                canvas.fill_rect(Rect::new(row_rect.x, row_rect.y, t, row_rect.h), c);
                canvas.fill_rect(
                    Rect::new(row_rect.right() - t, row_rect.y, t, row_rect.h),
                    c,
                );
            } else if self.sel.contains(&row.entry.path) && (focused || multi) {
                canvas.fill_rect(row_rect, theme::p().selection);
                if multi {
                    // Several selected: a caret-coloured bar marks each one
                    // (also while the keyboard is elsewhere, because the
                    // right-click menu acts on all of them).
                    let w = ctx.px(3.0).max(2.0) as i32;
                    canvas.fill_rect(
                        Rect::new(row_rect.x, row_rect.y, w, row_rect.h),
                        theme::p().caret,
                    );
                }
            } else if self.hover == Some(i) {
                canvas.fill_rect(row_rect, theme::p().line_highlight);
            }
            if cursor == Some(i) && !self.sel.contains(&row.entry.path) {
                // The keyboard cursor on an entry that is not selected.
                let t = ctx.px(1.0).max(1.0) as i32;
                let c = theme::p().dim;
                canvas.fill_rect(Rect::new(row_rect.x, row_rect.y, row_rect.w, t), c);
                canvas.fill_rect(
                    Rect::new(row_rect.x, row_rect.bottom() - t, row_rect.w, t),
                    c,
                );
                canvas.fill_rect(Rect::new(row_rect.x, row_rect.y, t, row_rect.h), c);
                canvas.fill_rect(
                    Rect::new(row_rect.right() - t, row_rect.y, t, row_rect.h),
                    c,
                );
            }
            let x = rect.x as f32 + ctx.px(10.0) + row.depth.saturating_sub(1) as f32 * indent;
            let cy = y + rh / 2.0;
            let icon = ctx.px(6.0);
            let color = if row.entry.is_dir {
                theme::p().accent
            } else {
                theme::p().text
            };
            let lw = ctx.px(1.6).max(1.2);
            if row.depth == 0 {
                // Root: just the folder name, in accent.
            } else if row.entry.is_dir && !row.entry.is_link {
                // Chevron ▸ / ▾
                let (cx, s) = (x + icon * 0.5, icon * 0.55);
                if row.expanded {
                    canvas.draw_line(cx - s, cy - s * 0.4, cx, cy + s * 0.5, lw, theme::p().text);
                    canvas.draw_line(cx, cy + s * 0.5, cx + s, cy - s * 0.4, lw, theme::p().text);
                } else {
                    canvas.draw_line(cx - s * 0.4, cy - s, cx + s * 0.5, cy, lw, theme::p().text);
                    canvas.draw_line(cx + s * 0.5, cy, cx - s * 0.4, cy + s, lw, theme::p().text);
                }
            } else {
                // File: small type-coloured dot.
                let c = file_color(&row.entry.name);
                let d = ctx.px(5.0).max(3.0);
                canvas.fill_rect(
                    Rect::new(
                        (x + icon * 0.5 - d / 2.0) as i32,
                        (cy - d / 2.0) as i32,
                        d as i32,
                        d as i32,
                    ),
                    c,
                );
            }
            let tx = if row.depth == 0 {
                rect.x as f32 + ctx.px(10.0)
            } else {
                x + icon + ctx.px(8.0)
            };
            let mut name = row.entry.name.clone();
            if row.entry.is_link {
                name.push_str(" ↗");
            }
            let ty = (cy - lh / 2.0).round() as i32;
            let label_color = if row.depth == 0 {
                theme::p().accent
            } else {
                color
            };
            ctx.label(
                canvas,
                &name,
                tx as i32,
                ty,
                label_color,
                rect.right() - tx as i32 - ctx.px(6.0) as i32,
                FontKind::Ui,
            );
        }
        // "… N more" note for truncated folders is shown in the status bar by the app.
        let content = self.rows().len() as f32 * rh + pad_y * 2.0;
        draw_vscrollbar(canvas, rect, self.scroll.y, content, ctx);
    }

    /// Folders whose listing was cut at MAX_ENTRIES (or whose reading was
    /// stopped at MAX_READ: then the count is "more than").
    pub fn truncated_note(&self) -> Option<String> {
        self.truncated.values().max().map(|&(n, more)| {
            let more = if more { "more than " } else { "" };
            format!("folder has {more}{n} items; first {MAX_ENTRIES} shown")
        })
    }
}

/// A small colour cue per file type (lite-xl style accents).
fn file_color(name: &str) -> crate::canvas::Color {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "md" | "markdown" | "txt" => theme::p().syntax_function,
        "rs" | "toml" => theme::p().syntax_number,
        "js" | "ts" | "jsx" | "tsx" | "json" => theme::p().syntax_string,
        "py" | "rb" | "go" | "c" | "h" | "cpp" | "cs" | "java" => theme::p().syntax_keyword,
        "html" | "css" | "xml" | "svg" => theme::p().syntax_keyword2,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" => theme::p().good,
        _ => theme::p().dim,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn links_to_network_paths_are_listed_without_being_followed() {
        let d = std::env::temp_dir().join(format!("litedit-netlink-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("real")).unwrap();
        std::os::unix::fs::symlink("//evil-host/share", d.join("share")).unwrap();
        std::os::unix::fs::symlink(d.join("real"), d.join("local")).unwrap();
        let (list, _) = read_dir_sorted(&d, true);
        let get = |n: &str| list.iter().find(|e| e.name == n).cloned().unwrap();
        assert!(get("share").is_link && !get("share").is_dir);
        assert!(
            get("local").is_link && get("local").is_dir,
            "local links still show as folders"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("litedit-tree-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn sorted_dirs_first_ignored_and_hidden_removed() {
        let d = tmp("sort");
        for f in ["b.txt", "A.md", ".hidden", "c.rs"] {
            std::fs::write(d.join(f), "").unwrap();
        }
        for dir in ["zdir", "Adir", "node_modules", ".git"] {
            std::fs::create_dir(d.join(dir)).unwrap();
        }
        let (list, total) = read_dir_sorted(&d, false);
        let names: Vec<&str> = list.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["Adir", "zdir", "A.md", "b.txt", "c.rs"]);
        assert_eq!(total, 5);
        let (with_hidden, _) = read_dir_sorted(&d, true);
        assert!(with_hidden.iter().any(|e| e.name == ".hidden"));
        assert!(
            !with_hidden.iter().any(|e| e.name == ".git"),
            "ignored even when showing hidden"
        );
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn reading_a_folder_stops_at_the_limit_and_says_so() {
        let d = tmp("limit");
        for i in 0..10 {
            std::fs::write(d.join(format!("f{i}.txt")), "").unwrap();
        }
        let (list, total, more) = read_dir_limited(&d, false, 4);
        assert_eq!((list.len(), total, more), (4, 4, true));
        let (list, total, more) = read_dir_limited(&d, false, 10);
        assert_eq!((list.len(), total, more), (10, 10, false));
        let (list, _, more) = read_dir_limited(&d, false, 100);
        assert_eq!((list.len(), more), (10, false));
        // Sorted order is unchanged by the early stop: dirs first, then
        // names ignoring case.
        std::fs::create_dir(d.join("Zdir")).unwrap();
        std::fs::write(d.join("B.txt"), "").unwrap();
        std::fs::write(d.join("a.txt"), "").unwrap();
        let (list, _, _) = read_dir_limited(&d, false, 100);
        let names: Vec<&str> = list.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(&names[..4], ["Zdir", "a.txt", "B.txt", "f0.txt"]);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn the_truncation_note_says_more_than_when_reading_stopped_early() {
        let mut t = Tree::new();
        t.truncated.insert(PathBuf::from("a"), (7000, false));
        assert_eq!(
            t.truncated_note().unwrap(),
            format!("folder has 7000 items; first {MAX_ENTRIES} shown")
        );
        t.truncated.insert(PathBuf::from("b"), (50_000, true));
        assert_eq!(
            t.truncated_note().unwrap(),
            format!("folder has more than 50000 items; first {MAX_ENTRIES} shown")
        );
    }

    #[test]
    fn lazy_expand_collapse_and_invalidate() {
        let d = tmp("expand");
        std::fs::create_dir_all(d.join("sub/deeper")).unwrap();
        std::fs::write(d.join("sub/inner.md"), "").unwrap();
        std::fs::write(d.join("top.txt"), "").unwrap();
        let mut t = Tree::new();
        t.set_root(&d);
        assert_eq!(t.rows().len(), 3); // root, sub, top.txt
        assert!(
            !t.entries.contains_key(&d.join("sub")),
            "not read until expanded"
        );
        t.toggle(&d.join("sub"));
        assert_eq!(t.rows().len(), 5);
        t.toggle(&d.join("sub"));
        assert_eq!(t.rows().len(), 3);
        std::fs::write(d.join("new.txt"), "").unwrap();
        assert_eq!(t.rows().len(), 3, "cached until invalidated");
        assert!(t.invalidate(&[d.join("new.txt")]));
        assert_eq!(t.rows().len(), 4);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn reveal_expands_parents_and_selects() {
        let d = tmp("reveal");
        std::fs::create_dir_all(d.join("a/b")).unwrap();
        std::fs::write(d.join("a/b/f.md"), "").unwrap();
        let mut t = Tree::new();
        t.set_root(&d);
        t.reveal(&d.join("a/b/f.md"));
        assert_eq!(t.selected_row().unwrap().entry.path, d.join("a/b/f.md"));
        t.reveal(Path::new("/somewhere/else.md")); // outside root: ignored
        std::fs::remove_dir_all(&d).ok();
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_dirs_are_not_expanded() {
        let d = tmp("link");
        std::fs::create_dir(d.join("real")).unwrap();
        std::os::unix::fs::symlink(&d, d.join("loop")).unwrap();
        let mut t = Tree::new();
        t.set_root(&d);
        let n = t.rows().len();
        t.toggle(&d.join("loop"));
        let row = t
            .rows()
            .iter()
            .find(|r| r.entry.name == "loop")
            .unwrap()
            .clone();
        assert!(row.entry.is_link && !row.expanded);
        assert_eq!(t.rows().len(), n);
        std::fs::remove_dir_all(&d).ok();
    }
}
