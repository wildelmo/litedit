//! Application state, layout and input dispatch. Platform-independent: the
//! window, `--script` mode and tests all drive [`App`] through [`Input`].
//!
//! Layout (lite-xl RootView): `[tree | divider | main view]` above a status bar.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::canvas::{Canvas, Rect};
use crate::doc::Document;
use crate::input::{Cursor, Input, Key, MouseButton, Request};
use crate::text::Fonts;
use crate::theme;
use crate::tree::Tree;
use crate::ui::{Ctx, FontKind};
use crate::view::code::CodeView;
use crate::view::markdown::{MarkdownView, is_markdown};
use crate::view::message::MessageView;
use crate::view::{Tick, View};

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
    view: Box<dyn View>,
    tree: Tree,
    focus: Focus,
    requests: Vec<Request>,
    cursor: Cursor,
    mouse: (f32, f32),
    dragging_divider: bool,
    clipboard: Box<dyn crate::clipboard::Clipboard>,
    /// Set when a view may have deferred work; cleared when `tick` finishes it.
    pending: bool,
}

struct Layout {
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
            tree: Tree::new(),
            focus: Focus::Main,
            requests: Vec::new(),
            cursor: Cursor::Default,
            mouse: (0.0, 0.0),
            dragging_divider: false,
            clipboard: Box::new(crate::clipboard::MemClipboard::default()),
            pending: false,
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

    /// Open a file (into the main view) or a folder (into the tree).
    pub fn open(&mut self, path: &Path) {
        if path.is_dir() {
            self.tree.set_root(path);
            self.tree.visible = true;
            self.view = Box::new(MessageView::folder_opened());
            self.focus = Focus::Tree;
            return;
        }
        // D-012: opening a file with no folder open shows its folder.
        if self.tree.root().is_none()
            && let Some(parent) = path.parent().filter(|p| p.is_dir())
        {
            self.tree.set_root(parent);
        }
        self.view = match Document::open(path) {
            Ok(doc) if is_markdown(path) => Box::new(MarkdownView::new(doc)),
            Ok(doc) => Box::new(CodeView::new(doc)),
            Err(e) => Box::new(MessageView::error(
                "Error",
                format!("Could not open {}: {e}", path.display()),
            )),
        };
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
        if let Some(dir) = self.view.path().and_then(Path::parent)
            && !self.tree.root().is_some_and(|r| dir.starts_with(r))
        {
            v.push((dir.to_path_buf(), false));
        }
        v
    }

    fn ui_line_height(&self) -> f32 {
        let s = (theme::FONT_SIZE * self.scale * self.zoom).round().max(4.0);
        (s * theme::LINE_HEIGHT).round()
    }

    fn layout(&self) -> Layout {
        let (w, h) = (self.size.0 as i32, self.size.1 as i32);
        let pad_y = (theme::PADDING_Y * self.scale).round() as i32;
        let status_h = self.ui_line_height() as i32 + pad_y * 2;
        let status = Rect::new(0, (h - status_h).max(0), w, status_h.min(h));
        let body_h = (h - status_h).max(0);
        if self.tree.is_shown() && w > 200 {
            let tw = (self.tree.width * self.scale)
                .clamp(
                    TREE_MIN_W * self.scale,
                    (w as f32 * 0.6).max(TREE_MIN_W * self.scale),
                )
                .min(w as f32 - 100.0)
                .round() as i32;
            let div = (self.scale.round() as i32).max(1);
            Layout {
                tree: Some(Rect::new(0, 0, tw, body_h)),
                divider: Some(Rect::new(tw, 0, div, body_h)),
                main: Rect::new(tw + div, 0, (w - tw - div).max(0), body_h),
                status,
            }
        } else {
            Layout {
                tree: None,
                divider: None,
                main: Rect::new(0, 0, w, body_h),
                status,
            }
        }
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
                open = Some(p.clone());
                false
            }
            _ => true,
        });
        if let Some(p) = open {
            self.open(&p);
            return true;
        }
        if redraw {
            self.pending = true;
        }
        redraw
    }

    fn handle_inner(&mut self, input: &Input) -> bool {
        match input {
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
                let mut changed = self.tree.invalidate(paths);
                if let Some(p) = self.view.path().map(Path::to_path_buf)
                    && paths.iter().any(|c| c == &p)
                {
                    let mut ctx = ctx!(self);
                    changed |= self.view.reload(&mut ctx);
                }
                return changed;
            }
            Input::Key { key, mods } if mods.ctrl => {
                let handled = match key {
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
            _ => {}
        }

        let layout = self.layout();
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

        let in_tree = layout.tree.is_some_and(|t| self.mouse.0 < t.right() as f32);
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
        canvas.fill_rect(canvas.bounds(), theme::BACKGROUND);

        let current = self.view.path().map(Path::to_path_buf);
        let focus = self.focus;
        let mut ctx = ctx!(self);

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
            canvas.fill_rect(d, theme::DIVIDER);
        }

        canvas.set_clip(layout.main);
        self.view.render(canvas, layout.main, &mut ctx);

        // Status bar (lite-xl StatusView).
        canvas.set_clip(layout.status);
        canvas.fill_rect(layout.status, theme::BACKGROUND2);
        canvas.fill_rect(
            Rect::new(layout.status.x, layout.status.y, layout.status.w, 1),
            theme::DIVIDER,
        );
        let pad_x = ctx.px(theme::PADDING_X) as i32;
        let pad_y = ctx.px(theme::PADDING_Y) as i32;
        let mut x = layout.status.x + pad_x;
        let y = layout.status.y + pad_y;
        let mut items = self.view.status();
        if let Some(n) = self.tree.truncated_note() {
            items.push(n);
        }
        for (i, item) in items.iter().enumerate() {
            let color = if i == 0 { theme::ACCENT } else { theme::TEXT };
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
                theme::TEXT,
                w + 2,
                FontKind::Ui,
            );
        }
        canvas.set_clip(canvas.bounds());
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
