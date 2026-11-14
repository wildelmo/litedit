//! A small right-click menu (lite-xl `contextmenu.lua`).

use std::path::PathBuf;

use crate::canvas::{Canvas, Rect};
use crate::input::{Input, Key, MouseButton};
use crate::theme;
use crate::ui::{Ctx, FontKind};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuAction {
    NewFile(PathBuf),
    NewFolder(PathBuf),
    Rename(PathBuf),
    Delete(PathBuf),
    Reveal(PathBuf),
    CopyPath(PathBuf),
    /// Search inside this folder (also used for the Ctrl+Shift+F prompt).
    FindInFolder(PathBuf),
}

pub struct Menu {
    x: f32,
    y: f32,
    items: Vec<(String, MenuAction)>,
    rects: Vec<Rect>,
    selected: Option<usize>,
}

/// What the menu wants after an input.
pub enum MenuResult {
    Keep,
    Close,
    Run(MenuAction),
}

impl Menu {
    pub fn new(x: f32, y: f32, items: Vec<(&str, MenuAction)>) -> Self {
        Self {
            x,
            y,
            items: items.into_iter().map(|(l, a)| (l.to_string(), a)).collect(),
            rects: Vec::new(),
            selected: None,
        }
    }

    pub fn handle(&mut self, input: &Input) -> MenuResult {
        let hit = |rects: &[Rect], x: f32, y: f32| {
            rects.iter().position(|r| {
                (x as i32) >= r.x
                    && (x as i32) < r.right()
                    && (y as i32) >= r.y
                    && (y as i32) < r.bottom()
            })
        };
        match input {
            Input::MouseMove { x, y } => {
                self.selected = hit(&self.rects, *x, *y);
                MenuResult::Keep
            }
            Input::MouseDown { x, y, button, .. } => match hit(&self.rects, *x, *y) {
                Some(i) if *button == MouseButton::Left => MenuResult::Run(self.items[i].1.clone()),
                Some(_) => MenuResult::Keep,
                None => MenuResult::Close,
            },
            Input::Key { key, .. } => {
                let n = self.items.len();
                match key {
                    Key::Escape => return MenuResult::Close,
                    Key::Down => self.selected = Some(self.selected.map_or(0, |s| (s + 1) % n)),
                    Key::Up => {
                        self.selected = Some(self.selected.map_or(n - 1, |s| (s + n - 1) % n))
                    }
                    Key::Enter => {
                        if let Some(s) = self.selected {
                            return MenuResult::Run(self.items[s].1.clone());
                        }
                    }
                    _ => {}
                }
                MenuResult::Keep
            }
            _ => MenuResult::Keep,
        }
    }

    pub fn render(&mut self, canvas: &mut Canvas, bounds: Rect, ctx: &mut Ctx) {
        let row_h = (ctx.ui_metrics().line_height + ctx.px(8.0)) as i32;
        let pad = ctx.px(12.0) as i32;
        let w = self
            .items
            .iter()
            .map(|(l, _)| ctx.text_width(l, FontKind::Ui) as i32)
            .max()
            .unwrap_or(0)
            + pad * 2;
        let h = row_h * self.items.len() as i32 + ctx.px(8.0) as i32;
        // Keep the menu inside the window.
        let x = (self.x as i32).min(bounds.right() - w - 2).max(bounds.x);
        let y = (self.y as i32).min(bounds.bottom() - h - 2).max(bounds.y);
        let frame = Rect::new(x, y, w, h);
        canvas.fill_rect(Rect::new(x - 1, y - 1, w + 2, h + 2), theme::DIVIDER);
        canvas.fill_rect(frame, theme::BACKGROUND2);
        let lh = ctx.ui_metrics().line_height as i32;
        self.rects.clear();
        for (i, (label, _)) in self.items.iter().enumerate() {
            let r = Rect::new(x, y + ctx.px(4.0) as i32 + i as i32 * row_h, w, row_h);
            if self.selected == Some(i) {
                canvas.fill_rect(r, theme::SELECTION);
            }
            ctx.label(
                canvas,
                label,
                r.x + pad,
                r.y + (row_h - lh) / 2,
                theme::ACCENT,
                w - pad * 2,
                FontKind::Ui,
            );
            self.rects.push(r);
        }
    }
}
