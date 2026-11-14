//! Application state, layout and input dispatch. Platform-independent: the
//! window, `--script` mode and tests all drive [`App`] through [`Input`].

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::canvas::{Canvas, Rect};
use crate::doc::Document;
use crate::input::{Cursor, Input, Key, Request};
use crate::text::Fonts;
use crate::theme;
use crate::ui::{Ctx, FontKind};
use crate::view::code::CodeView;
use crate::view::markdown::{MarkdownView, is_markdown};
use crate::view::message::MessageView;
use crate::view::{Tick, View};

pub const MIN_ZOOM: f32 = 0.5;
pub const MAX_ZOOM: f32 = 3.0;

pub struct App {
    pub fonts: Fonts,
    scale: f32,
    zoom: f32,
    size: (u32, u32),
    view: Box<dyn View>,
    requests: Vec<Request>,
    cursor: Cursor,
    /// Set when a view may have deferred work; cleared when `tick` finishes it.
    pending: bool,
}

struct Layout {
    main: Rect,
    status: Rect,
}

impl App {
    pub fn new(fonts: Fonts, scale: f32) -> Self {
        Self {
            fonts,
            scale,
            zoom: 1.0,
            size: (1, 1),
            view: Box::new(MessageView::welcome()),
            requests: Vec::new(),
            cursor: Cursor::Default,
            pending: false,
        }
    }

    pub fn open(&mut self, path: &Path) {
        if path.is_dir() {
            // File tree arrives in S1.10.
            self.view = Box::new(MessageView::error(
                "Folder",
                format!("Opening folders is not supported yet: {}", path.display()),
            ));
            return;
        }
        self.view = match Document::open(path) {
            Ok(doc) if is_markdown(path) => Box::new(MarkdownView::new(doc)),
            Ok(doc) => Box::new(CodeView::new(doc)),
            Err(e) => Box::new(MessageView::error(
                "Error",
                format!("Could not open {}: {e}", path.display()),
            )),
        };
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

    pub fn take_requests(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.requests)
    }

    pub fn has_pending_work(&self) -> bool {
        self.pending
    }

    pub fn status_text(&self) -> String {
        self.view.status().join("   ")
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
        let main = Rect::new(0, 0, w, (h - status_h).max(0));
        Layout { main, status }
    }

    /// Handle one input event. Returns true if the window should redraw.
    pub fn handle(&mut self, input: Input) -> bool {
        match &input {
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
                    Key::Char('=') | Key::Char('+') => self.set_zoom(self.zoom * 1.1),
                    Key::Char('-') => self.set_zoom(self.zoom / 1.1),
                    Key::Char('0') => self.set_zoom(1.0),
                    _ => false,
                };
                if handled {
                    return true;
                }
            }
            Input::FsChanged(paths) => {
                let mine = self.view.path().map(Path::to_path_buf);
                if let Some(p) = mine
                    && paths.iter().any(|c| c == &p)
                {
                    let mut ctx = Ctx {
                        fonts: &mut self.fonts,
                        scale: self.scale,
                        zoom: self.zoom,
                        requests: &mut self.requests,
                        cursor: &mut self.cursor,
                    };
                    if self.view.reload(&mut ctx) {
                        self.pending = true;
                        return true;
                    }
                }
                return false;
            }
            _ => {}
        }
        let rect = self.layout().main;
        let mut ctx = Ctx {
            fonts: &mut self.fonts,
            scale: self.scale,
            zoom: self.zoom,
            requests: &mut self.requests,
            cursor: &mut self.cursor,
        };
        let mut redraw = self.view.handle(&input, rect, &mut ctx);
        // Requests the app handles itself (e.g. following a link to a local file).
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
            redraw = true;
        }
        if redraw {
            self.pending = true;
        }
        redraw
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
        let mut ctx = Ctx {
            fonts: &mut self.fonts,
            scale: self.scale,
            zoom: self.zoom,
            requests: &mut self.requests,
            cursor: &mut self.cursor,
        };
        let t = self.view.tick(&mut ctx, budget);
        self.pending = t.more;
        t
    }

    /// Finish all deferred work (scripts and screenshots), bounded by `limit`.
    pub fn settle(&mut self, limit: Duration) {
        let start = Instant::now();
        while self.tick(Duration::from_millis(50)).more && start.elapsed() < limit {}
    }

    pub fn render(&mut self, canvas: &mut Canvas) {
        if (canvas.width() as u32, canvas.height() as u32) != self.size {
            self.size = (canvas.width().max(1) as u32, canvas.height().max(1) as u32);
        }
        let layout = self.layout();
        canvas.set_clip(canvas.bounds());
        canvas.fill_rect(canvas.bounds(), theme::BACKGROUND);

        let mut ctx = Ctx {
            fonts: &mut self.fonts,
            scale: self.scale,
            zoom: self.zoom,
            requests: &mut self.requests,
            cursor: &mut self.cursor,
        };

        canvas.set_clip(layout.main);
        self.view.render(canvas, layout.main, &mut ctx);

        // Status bar (lite-xl StatusView).
        canvas.set_clip(layout.status);
        canvas.fill_rect(layout.status, theme::BACKGROUND2);
        let pad_x = ctx.px(theme::PADDING_X) as i32;
        let pad_y = ctx.px(theme::PADDING_Y) as i32;
        let mut x = layout.status.x + pad_x;
        let y = layout.status.y + pad_y;
        let items = self.view.status();
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
