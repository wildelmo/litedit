//! A single-line text field (lite-xl `CommandView`), used by find/replace,
//! go to line, rename and the palettes. It reuses [`EditDoc`] for all editing
//! so there is exactly one text-editing implementation in litedit.

use std::path::PathBuf;

use cosmic_text::{Attrs, Buffer, Family, Shaping, Wrap};

use crate::canvas::{Canvas, Rect};
use crate::doc::{Document, Encoding};
use crate::editor::buffer::Pos;
use crate::editor::doc::{EditDoc, Move};
use crate::input::{Input, Key, Mods};
use crate::theme;
use crate::ui::{Ctx, FontKind};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BarEvent {
    /// Nothing the owner needs to react to (caret moved, etc.).
    None,
    /// The text changed.
    Changed,
    /// Enter (with Shift held: `back` = true).
    Submit {
        back: bool,
    },
    Cancel,
    /// Up / Down (palette selection, history).
    Nav(i32),
    /// The key was not for the bar.
    Ignored,
}

pub struct InputBar {
    pub label: String,
    doc: EditDoc,
    scroll_x: f32,
}

impl InputBar {
    pub fn new(label: &str, initial: &str) -> Self {
        let mut doc = EditDoc::new(Document {
            path: PathBuf::from("<input>"),
            text: single_line(initial),
            encoding: Encoding::Utf8,
        });
        doc.select_all();
        Self {
            label: label.to_string(),
            doc,
            scroll_x: 0.0,
        }
    }

    pub fn text(&self) -> &str {
        self.doc.buf.line(0)
    }

    pub fn set_text(&mut self, s: &str) {
        self.doc.select_all();
        self.doc.insert_text(&single_line(s));
    }

    pub fn handle(&mut self, input: &Input, ctx: &mut Ctx) -> BarEvent {
        let before = self.doc.version();
        let ev = match input {
            Input::Text(t) => {
                self.doc.insert_text(&single_line(t));
                BarEvent::None
            }
            Input::Key { key, mods } => match self.key(key, *mods, ctx) {
                Some(ev) => ev,
                None => return BarEvent::Ignored,
            },
            _ => return BarEvent::Ignored,
        };
        if ev == BarEvent::None && self.doc.version() != before {
            BarEvent::Changed
        } else {
            ev
        }
    }

    fn key(&mut self, key: &Key, mods: Mods, ctx: &mut Ctx) -> Option<BarEvent> {
        let shift = mods.shift;
        let d = &mut self.doc;
        match (key, mods.ctrl) {
            (Key::Enter, _) => return Some(BarEvent::Submit { back: shift }),
            (Key::Escape, _) => return Some(BarEvent::Cancel),
            (Key::Up, false) => return Some(BarEvent::Nav(-1)),
            (Key::Down, false) => return Some(BarEvent::Nav(1)),
            (Key::PageUp, _) => return Some(BarEvent::Nav(-10)),
            (Key::PageDown, _) => return Some(BarEvent::Nav(10)),
            (Key::Left, c) => d.move_carets(if c { Move::WordLeft } else { Move::Left }, shift),
            (Key::Right, c) => d.move_carets(if c { Move::WordRight } else { Move::Right }, shift),
            (Key::Home, _) => d.move_carets(Move::DocStart, shift),
            (Key::End, _) => d.move_carets(Move::DocEnd, shift),
            (Key::Backspace, c) => d.backspace(c),
            (Key::Delete, c) => d.delete_forward(c),
            (Key::Char('a'), true) => d.select_all(),
            (Key::Char('z'), true) if !shift => {
                d.undo();
            }
            (Key::Char('y'), true) | (Key::Char('z'), true) => {
                d.redo();
            }
            (Key::Char('c'), true) => {
                let t = d.selected_text();
                if !t.is_empty() {
                    ctx.clipboard.set(t);
                }
            }
            (Key::Char('x'), true) => {
                let t = d.selected_text();
                if !t.is_empty() {
                    ctx.clipboard.set(t);
                    d.delete_selection();
                }
            }
            (Key::Char('v'), true) => {
                if let Some(t) = ctx.clipboard.get() {
                    d.insert_text(&single_line(&t));
                }
            }
            // Plain character keys arrive as Text; swallow their Key events.
            (Key::Char(_), false) => {}
            _ => return None,
        }
        Some(BarEvent::None)
    }

    /// Draw `label [field]` into `rect`.
    pub fn render(&mut self, canvas: &mut Canvas, rect: Rect, ctx: &mut Ctx, focused: bool) {
        let pad_x = ctx.px(theme::PADDING_X) as i32;
        let m = ctx.ui_metrics();
        let lh = m.line_height as i32;
        let ty = rect.y + (rect.h - lh) / 2;
        let label_w = if self.label.is_empty() {
            0
        } else {
            ctx.text_width(&self.label, FontKind::Ui) as i32 + pad_x
        };
        ctx.label(
            canvas,
            &self.label,
            rect.x + pad_x,
            ty,
            theme::p().text,
            label_w,
            FontKind::Ui,
        );
        let field = Rect::new(
            rect.x + pad_x + label_w,
            rect.y + ctx.px(4.0) as i32,
            rect.w - label_w - pad_x * 2,
            rect.h - ctx.px(8.0) as i32,
        );
        if field.w <= 0 {
            return;
        }
        canvas.fill_rect(field, theme::p().background);
        let inner_pad = ctx.px(6.0);
        let text_w = (field.w as f32 - inner_pad * 2.0).max(1.0);

        // Shape the text to find caret/selection x positions.
        let fam = ctx.family(FontKind::Ui);
        let mut b = Buffer::new_empty(m);
        b.set_wrap(Wrap::None);
        let shown = crate::text::sanitize(self.text()).into_owned();
        b.set_text(
            &shown,
            &Attrs::new().family(Family::Name(&fam)),
            Shaping::Advanced,
            None,
        );
        b.shape_until_scroll(&mut ctx.fonts.system, false);
        let x_of = |col: usize| -> f32 {
            let Some(run) = b.layout_runs().next() else {
                return 0.0;
            };
            for g in run.glyphs {
                if col >= g.start && col < g.end.max(g.start + 1) {
                    return g.x;
                }
            }
            run.line_w
        };
        let sel = self.doc.primary();
        let (a, z) = sel.range();
        let caret_x = x_of(sel.head.col);
        if caret_x - self.scroll_x > text_w {
            self.scroll_x = caret_x - text_w;
        } else if caret_x < self.scroll_x {
            self.scroll_x = caret_x;
        }
        let ox = field.x as f32 + inner_pad - self.scroll_x;
        let prev = canvas.clip();
        canvas.set_clip(field.intersect(&prev));
        if a != z {
            let (x0, x1) = (x_of(a.col), x_of(z.col));
            canvas.fill_rect(
                Rect::new((ox + x0) as i32, ty, (x1 - x0).max(1.0) as i32, lh),
                theme::p().selection,
            );
        }
        ctx.fonts
            .draw_buffer(canvas, &mut b, ox.round() as i32, ty, theme::p().accent);
        if focused {
            canvas.fill_rect(
                Rect::new((ox + caret_x) as i32, ty, ctx.px(2.0).max(1.0) as i32, lh),
                theme::p().caret,
            );
        }
        canvas.set_clip(prev);
    }

    /// Place the caret at the end (e.g. after programmatic set_text).
    pub fn caret_to_end(&mut self) {
        let end = Pos::new(0, self.text().len());
        self.doc.set_caret(end, false);
    }
}

fn single_line(s: &str) -> String {
    s.chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clipboard::MemClipboard;
    use crate::input::{Cursor, Request};
    use crate::text::Fonts;

    fn k(key: Key) -> Input {
        Input::Key {
            key,
            mods: Mods::default(),
        }
    }

    #[test]
    fn edits_and_events() {
        let mut fonts = Fonts::load();
        let (mut reqs, mut cur, mut clip) = (
            Vec::<Request>::new(),
            Cursor::Default,
            MemClipboard::default(),
        );
        let mut ctx = Ctx {
            fonts: &mut fonts,
            scale: 1.0,
            zoom: 1.0,
            requests: &mut reqs,
            cursor: &mut cur,
            focused: true,
            clipboard: &mut clip,
        };
        let mut bar = InputBar::new("Find:", "old");
        // Initial text is selected: typing replaces it.
        assert_eq!(
            bar.handle(&Input::Text("n".into()), &mut ctx),
            BarEvent::Changed
        );
        assert_eq!(
            bar.handle(&Input::Text("ew\nline".into()), &mut ctx),
            BarEvent::Changed
        );
        assert_eq!(bar.text(), "new line");
        assert_eq!(bar.handle(&k(Key::Backspace), &mut ctx), BarEvent::Changed);
        assert_eq!(bar.handle(&k(Key::Left), &mut ctx), BarEvent::None);
        assert_eq!(
            bar.handle(&k(Key::Enter), &mut ctx),
            BarEvent::Submit { back: false }
        );
        assert_eq!(bar.handle(&k(Key::Escape), &mut ctx), BarEvent::Cancel);
        assert_eq!(bar.handle(&k(Key::Down), &mut ctx), BarEvent::Nav(1));
        assert_eq!(bar.handle(&k(Key::F(5)), &mut ctx), BarEvent::Ignored);
        let ctrl = |c| Input::Key {
            key: Key::Char(c),
            mods: Mods {
                ctrl: true,
                ..Mods::default()
            },
        };
        bar.handle(&ctrl('a'), &mut ctx);
        bar.handle(&ctrl('x'), &mut ctx);
        assert_eq!(bar.text(), "");
        bar.handle(&ctrl('v'), &mut ctx);
        assert_eq!(bar.text(), "new lin");
        let mut buf = vec![0u32; 400 * 40];
        bar.render(
            &mut Canvas::new(&mut buf, 400, 40),
            Rect::new(0, 0, 400, 40),
            &mut ctx,
            true,
        );
    }
}
