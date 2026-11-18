//! The start page shown when no folder and no file is open: what you can
//! open, and the folders, workspaces and files you opened recently (like
//! VS Code's welcome page, much smaller). Every entry is clickable and can
//! be reached with the arrow keys and Enter.

use std::path::{Path, PathBuf};

use crate::canvas::{Canvas, Rect};
use crate::input::{Cursor, Input, Key, MouseButton, Request};
use crate::theme;
use crate::ui::{Ctx, FontKind};
use crate::view::View;

/// At most this many recent entries are listed.
pub const MAX_SHOWN: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Entry {
    OpenFile,
    OpenFolder,
    OpenWorkspace,
    /// A recent path and what it is ("folder", "workspace", "file").
    Recent(PathBuf, &'static str),
}

pub struct StartView {
    entries: Vec<Entry>,
    /// Where each entry was drawn (same order as `entries`).
    rects: Vec<Rect>,
    hover: Option<usize>,
    /// Keyboard selection.
    selected: Option<usize>,
}

impl StartView {
    /// `recent`: most recent first; only entries that still exist are kept
    /// (checked once, here, not on every frame).
    pub fn new(recent: &[PathBuf]) -> Self {
        let mut entries = vec![Entry::OpenFile, Entry::OpenFolder, Entry::OpenWorkspace];
        entries.extend(
            recent
                .iter()
                .filter(|p| p.exists())
                .take(MAX_SHOWN)
                .map(|p| Entry::Recent(p.clone(), kind(p))),
        );
        Self {
            entries,
            rects: Vec::new(),
            hover: None,
            selected: None,
        }
    }

    fn run(&self, i: usize, ctx: &mut Ctx) {
        let r = match self.entries.get(i) {
            Some(Entry::OpenFile) => Request::OpenFileDialog,
            Some(Entry::OpenFolder) => Request::OpenFolderDialog,
            Some(Entry::OpenWorkspace) => Request::OpenWorkspaceDialog,
            Some(Entry::Recent(p, _)) => Request::OpenPath(p.clone()),
            None => return,
        };
        ctx.requests.push(r);
    }

    fn hit(&self, x: f32, y: f32) -> Option<usize> {
        self.rects.iter().position(|r| {
            (x as i32) >= r.x
                && (x as i32) < r.right()
                && (y as i32) >= r.y
                && (y as i32) < r.bottom()
        })
    }
}

/// What a recent entry is, for the label after its name.
fn kind(p: &Path) -> &'static str {
    if p.is_dir() {
        "folder"
    } else if crate::workspace::is_workspace_file(p) {
        "workspace"
    } else {
        "file"
    }
}

fn name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.display().to_string())
}

impl View for StartView {
    fn render(&mut self, canvas: &mut Canvas, rect: Rect, ctx: &mut Ctx) {
        canvas.fill_rect(rect, theme::p().background);
        let lh = ctx.ui_metrics().line_height as i32;
        let row_h = lh + ctx.px(8.0) as i32;
        let gap = ctx.px(16.0) as i32;
        let pad = ctx.px(10.0) as i32;
        let n_recent = self.entries.len().saturating_sub(3);
        // Title, gap, 3 actions, then (heading + recents).
        let mut total = lh + gap + row_h * 3;
        if n_recent > 0 {
            total += gap + lh + ctx.px(4.0) as i32 + row_h * n_recent as i32;
        }
        let width = ctx.px(460.0).min((rect.w - 2 * pad).max(0) as f32) as i32;
        let x = rect.x + ((rect.w - width) / 2).max(pad.min(rect.w / 2));
        let mut y = rect.y + ((rect.h - total) / 2).max(pad);

        ctx.label(
            canvas,
            "litedit",
            x + pad,
            y,
            theme::p().accent,
            width,
            FontKind::Ui,
        );
        y += lh + gap;

        self.rects.clear();
        let shortcut = |e: &Entry| match e {
            Entry::OpenFile => "Ctrl+O",
            Entry::OpenFolder => "Ctrl+Shift+O",
            _ => "",
        };
        for (i, e) in self.entries.clone().iter().enumerate() {
            if i == 3 {
                y += gap;
                ctx.label(
                    canvas,
                    "Recent",
                    x + pad,
                    y,
                    theme::p().dim,
                    width,
                    FontKind::Ui,
                );
                y += lh + ctx.px(4.0) as i32;
            }
            let r = Rect::new(x, y, width, row_h);
            if self.hover == Some(i) || self.selected == Some(i) {
                canvas.fill_rect(r, theme::p().line_highlight);
            }
            let ty = y + (row_h - lh) / 2;
            match e {
                Entry::Recent(p, kind) => {
                    let label = name(p);
                    let lw = ctx.text_width(&label, FontKind::Ui) as i32;
                    ctx.label(
                        canvas,
                        &label,
                        x + pad,
                        ty,
                        theme::p().text,
                        width - 2 * pad,
                        FontKind::Ui,
                    );
                    // Kind and parent folder, dimmed, after the name.
                    let parent = p
                        .parent()
                        .map(|d| d.display().to_string())
                        .unwrap_or_default();
                    let detail = format!("{kind}  {parent}");
                    let dx = x + pad + lw + pad;
                    let room = x + width - pad - dx;
                    if room > 0 {
                        ctx.label(canvas, &detail, dx, ty, theme::p().dim, room, FontKind::Ui);
                    }
                }
                _ => {
                    let label = match e {
                        Entry::OpenFile => "Open File…",
                        Entry::OpenFolder => "Open Folder…",
                        _ => "Open Workspace…",
                    };
                    ctx.label(
                        canvas,
                        label,
                        x + pad,
                        ty,
                        theme::p().accent,
                        width - 2 * pad,
                        FontKind::Ui,
                    );
                    let k = shortcut(e);
                    if !k.is_empty() {
                        let kw = ctx.text_width(k, FontKind::Ui) as i32;
                        ctx.label(
                            canvas,
                            k,
                            x + width - pad - kw,
                            ty,
                            theme::p().dim,
                            kw + 2,
                            FontKind::Ui,
                        );
                    }
                }
            }
            self.rects.push(r);
            y += row_h;
        }
    }

    fn handle(&mut self, input: &Input, _rect: Rect, ctx: &mut Ctx) -> bool {
        match input {
            Input::MouseMove { x, y } => {
                let h = self.hit(*x, *y);
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
                x,
                y,
                button: MouseButton::Left,
                ..
            } => {
                if let Some(i) = self.hit(*x, *y) {
                    self.run(i, ctx);
                }
                true
            }
            Input::Key { key, mods } if !mods.ctrl && !mods.alt => {
                let n = self.entries.len();
                match key {
                    Key::Down => self.selected = Some(self.selected.map_or(0, |s| (s + 1) % n)),
                    Key::Up => {
                        self.selected = Some(self.selected.map_or(n - 1, |s| (s + n - 1) % n))
                    }
                    Key::Enter => {
                        if let Some(s) = self.selected {
                            self.run(s, ctx);
                        }
                    }
                    Key::Escape => self.selected = None,
                    _ => return false,
                }
                true
            }
            _ => false,
        }
    }

    fn title(&self) -> String {
        String::new()
    }
}
