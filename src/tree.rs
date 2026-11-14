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
    root: Option<PathBuf>,
    entries: HashMap<PathBuf, Vec<Entry>>,
    /// Directories whose listing was truncated: (shown, total).
    truncated: HashMap<PathBuf, usize>,
    expanded: HashSet<PathBuf>,
    rows: Vec<Row>,
    rows_dirty: bool,
    pub visible: bool,
    pub width: f32,
    selected: Option<usize>,
    hover: Option<usize>,
    scroll: Scroll,
    view_h: f32,
    show_hidden: bool,
    /// Left button pressed on an entry: a drag starts once the mouse moves.
    press: Option<Press>,
    /// Folder the dragged entry would move into (valid targets only).
    drop_target: Option<PathBuf>,
    /// A finished drag: (entry, destination folder). The app asks first.
    pub requested_move: Option<(PathBuf, PathBuf)>,
}

struct Press {
    path: PathBuf,
    y: f32,
    dragging: bool,
}

/// Can `from` be moved into folder `dir`? Not into its own folder (no-op),
/// itself, or anything inside it.
pub fn can_move_into(from: &Path, dir: &Path) -> bool {
    from.parent() != Some(dir) && !dir.starts_with(from)
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

/// Read one directory: ignored names removed, dirs first, case-insensitive.
pub fn read_dir_sorted(dir: &Path, show_hidden: bool) -> (Vec<Entry>, usize) {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return (out, 0);
    };
    for e in rd.flatten() {
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
            match std::fs::read_link(&path) {
                Ok(t) if !crate::links::is_unsafe_local_path(&t.to_string_lossy()) => {
                    std::fs::metadata(&path).ok()
                }
                _ => None,
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
    out.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    let total = out.len();
    out.truncate(MAX_ENTRIES);
    (out, total)
}

impl Default for Tree {
    fn default() -> Self {
        Self::new()
    }
}

impl Tree {
    pub fn new() -> Self {
        Self {
            root: None,
            entries: HashMap::new(),
            truncated: HashMap::new(),
            expanded: HashSet::new(),
            rows: Vec::new(),
            rows_dirty: true,
            visible: true,
            width: 260.0,
            selected: None,
            hover: None,
            scroll: Scroll::default(),
            view_h: 0.0,
            show_hidden: false,
            press: None,
            drop_target: None,
            requested_move: None,
        }
    }

    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    pub fn is_shown(&self) -> bool {
        self.visible && self.root.is_some()
    }

    pub fn set_root(&mut self, dir: &Path) {
        self.root = Some(dir.to_path_buf());
        self.entries.clear();
        self.truncated.clear();
        self.expanded.clear();
        self.expanded.insert(dir.to_path_buf());
        self.selected = None;
        self.scroll = Scroll::default();
        self.rows_dirty = true;
    }

    /// Re-read every directory (after the ignore list changed).
    pub fn reload_all(&mut self) {
        self.entries.clear();
        self.rows_dirty = true;
    }

    pub fn toggle_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
        self.entries.clear();
        self.rows_dirty = true;
    }

    fn children(&mut self, dir: &Path) -> Vec<Entry> {
        if !self.entries.contains_key(dir) {
            let (list, total) = read_dir_sorted(dir, self.show_hidden);
            if total > list.len() {
                self.truncated.insert(dir.to_path_buf(), total);
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
            if let Some(root) = self.root.clone() {
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
            if let Some(s) = self.selected
                && s >= self.rows.len()
            {
                self.selected = self.rows.len().checked_sub(1);
            }
        }
        &self.rows
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
        if Some(dir) == self.root.as_deref() {
            return; // the root stays open
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
        let Some(root) = self.root.clone() else {
            return;
        };
        if !file.starts_with(&root) {
            return;
        }
        let mut p = file.parent();
        while let Some(dir) = p {
            if !dir.starts_with(&root) {
                break;
            }
            self.expanded.insert(dir.to_path_buf());
            p = dir.parent();
        }
        self.rows_dirty = true;
        let idx = self.rows().iter().position(|r| r.entry.path == file);
        self.selected = idx;
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

    fn activate(&mut self, i: usize, ctx: &mut Ctx) {
        let Some(row) = self.rows().get(i).cloned() else {
            return;
        };
        self.selected = Some(i);
        if row.entry.is_dir {
            if !row.entry.is_link {
                self.toggle(&row.entry.path);
            }
        } else {
            ctx.requests.push(Request::OpenPath(row.entry.path));
        }
    }

    fn ensure_selected_visible(&mut self, ctx: &Ctx) {
        let Some(s) = self.selected else { return };
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

    /// The selected entry (keyboard actions like F2 / Delete).
    pub fn selected_row(&mut self) -> Option<Row> {
        let i = self.selected?;
        self.rows().get(i).cloned()
    }

    /// The folder a drop at (x, y) would move into, if valid.
    fn drop_folder(
        &mut self,
        rect: Rect,
        x: f32,
        y: f32,
        from: &Path,
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
        can_move_into(from, &dir).then_some(dir)
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
                    self.drop_target = self.drop_folder(rect, *x, *y, &from, ctx);
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
                    self.requested_move = Some((p.path, dir));
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
                ..
            } => {
                if let Some(i) = self.row_at(rect, *y, ctx) {
                    let row = self.rows()[i].clone();
                    if row.depth > 0 {
                        self.press = Some(Press {
                            path: row.entry.path,
                            y: *y,
                            dragging: false,
                        });
                    }
                    self.activate(i, ctx);
                }
                true
            }
            Input::Wheel { .. } => {
                let before = self.scroll.y;
                self.scroll.wheel(input, Self::row_height(ctx));
                self.clamp(ctx);
                before != self.scroll.y
            }
            Input::Key { key, .. } => {
                let n = self.rows().len();
                if n == 0 {
                    return false;
                }
                let sel = self.selected.unwrap_or(0);
                match key {
                    Key::Down => self.selected = Some((sel + 1).min(n - 1)),
                    Key::Up => self.selected = Some(sel.saturating_sub(1)),
                    Key::Home => self.selected = Some(0),
                    Key::End => self.selected = Some(n - 1),
                    Key::PageDown => self.selected = Some((sel + 20).min(n - 1)),
                    Key::PageUp => self.selected = Some(sel.saturating_sub(20)),
                    Key::Enter | Key::Char(' ') => self.activate(sel, ctx),
                    Key::Right => {
                        let row = self.rows()[sel].clone();
                        if row.entry.is_dir && !row.expanded && !row.entry.is_link {
                            self.toggle(&row.entry.path);
                        } else if row.expanded {
                            self.selected = Some((sel + 1).min(n - 1));
                        }
                    }
                    Key::Left => {
                        let row = self.rows()[sel].clone();
                        if row.expanded && row.depth > 0 {
                            self.toggle(&row.entry.path);
                        } else if row.depth > 0 {
                            // Jump to the parent folder.
                            let parent =
                                self.rows()[..sel].iter().rposition(|r| r.depth < row.depth);
                            self.selected = parent.or(Some(0));
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
            } else if self.selected == Some(i) && focused {
                canvas.fill_rect(row_rect, theme::p().selection);
            } else if self.hover == Some(i) {
                canvas.fill_rect(row_rect, theme::p().line_highlight);
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

    /// Folders whose listing was cut at MAX_ENTRIES.
    pub fn truncated_note(&self) -> Option<String> {
        self.truncated
            .values()
            .max()
            .map(|n| format!("folder has {n} items; first {MAX_ENTRIES} shown"))
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
        let sel = t.selected.unwrap();
        assert_eq!(t.rows()[sel].entry.path, d.join("a/b/f.md"));
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
