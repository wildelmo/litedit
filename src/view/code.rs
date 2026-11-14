//! Code / plain-text view (lite-xl `DocView`, read-only for now).
//!
//! Each visible line is shaped into its own cosmic-text `Buffer` and cached,
//! so scrolling a huge file only ever shapes what is on screen.

use std::collections::HashMap;
use std::path::Path;

use cosmic_text::{Attrs, Buffer, Family, Shaping, Wrap};

use crate::canvas::{Canvas, Rect};
use crate::doc::Document;
use crate::input::{Input, Key};
use crate::theme;
use crate::ui::{Ctx, FontKind};
use crate::view::{Scroll, View, draw_vscrollbar};

/// Lines longer than this are cut for display (minified files). The full line
/// is kept in memory; only shaping is capped.
pub const MAX_DISPLAY_CHARS: usize = 4000;
pub const TAB_WIDTH: usize = 4;

pub struct CodeView {
    doc: Document,
    lines: Vec<String>,
    scroll: Scroll,
    cache: HashMap<usize, Buffer>,
    /// (font size, char width) — recomputed when the font size changes.
    char_w: Option<(f32, f32)>,
    longest_line_chars: usize,
    view_w: f32,
    view_h: f32,
}

impl CodeView {
    pub fn new(doc: Document) -> Self {
        let mut v = Self {
            doc,
            lines: Vec::new(),
            scroll: Scroll::default(),
            cache: HashMap::new(),
            char_w: None,
            longest_line_chars: 0,
            view_w: 0.0,
            view_h: 0.0,
        };
        v.load_lines();
        v
    }

    fn load_lines(&mut self) {
        self.lines = self.doc.text.split('\n').map(str::to_owned).collect();
        if self.lines.len() > 1 && self.lines.last().is_some_and(String::is_empty) {
            self.lines.pop(); // trailing newline is not an extra line
        }
        self.longest_line_chars = self
            .lines
            .iter()
            .map(|l| display_len(l).min(MAX_DISPLAY_CHARS))
            .max()
            .unwrap_or(0);
        self.cache.clear();
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    pub fn scroll_y(&self) -> f32 {
        self.scroll.y
    }

    pub fn scroll_x(&self) -> f32 {
        self.scroll.x
    }

    fn char_width(&mut self, ctx: &mut Ctx) -> f32 {
        let size = ctx.code_metrics().font_size;
        match self.char_w {
            Some((s, w)) if s == size => w,
            _ => {
                let w = (ctx.text_width("0000000000", FontKind::Mono) / 10.0).max(1.0);
                self.char_w = Some((size, w));
                self.cache.clear();
                w
            }
        }
    }

    fn gutter_width(&mut self, ctx: &mut Ctx) -> f32 {
        let digits = self.lines.len().to_string().len().max(3) as f32;
        (digits * self.char_width(ctx)).ceil() + ctx.px(theme::PADDING_X) * 2.0
    }

    fn content_size(&mut self, ctx: &mut Ctx) -> (f32, f32) {
        let lh = ctx.code_metrics().line_height;
        let w =
            self.longest_line_chars as f32 * self.char_width(ctx) + ctx.px(theme::PADDING_X) * 2.0;
        (
            w,
            self.lines.len() as f32 * lh + ctx.px(theme::PADDING_Y) * 2.0,
        )
    }

    fn clamp(&mut self, ctx: &mut Ctx) {
        let (cw, ch) = self.content_size(ctx);
        let (vw, vh) = (self.view_w, self.view_h);
        self.scroll.clamp(cw, ch, vw, vh);
    }

    fn line_buffer(&mut self, i: usize, ctx: &mut Ctx) -> &mut Buffer {
        let m = ctx.code_metrics();
        let fam = ctx.family(FontKind::Mono);
        let line = &self.lines[i];
        self.cache.entry(i).or_insert_with(|| {
            let mut b = Buffer::new_empty(m);
            b.set_wrap(Wrap::None);
            let text = expand_tabs(line, MAX_DISPLAY_CHARS);
            let attrs = Attrs::new().family(Family::Name(&fam));
            b.set_text(&text, &attrs, Shaping::Advanced, None);
            b
        })
    }
}

/// Visible width of a line in columns, with tabs expanded.
fn display_len(line: &str) -> usize {
    let mut col = 0;
    for c in line.chars() {
        col += if c == '\t' {
            TAB_WIDTH - col % TAB_WIDTH
        } else {
            1
        };
    }
    col
}

/// Expand tabs to spaces and cut at `max` columns.
pub fn expand_tabs(line: &str, max: usize) -> String {
    let mut out = String::with_capacity(line.len().min(max));
    let mut col = 0;
    for c in line.chars() {
        if col >= max {
            out.push('…');
            break;
        }
        if c == '\t' {
            let n = TAB_WIDTH - col % TAB_WIDTH;
            out.extend(std::iter::repeat_n(' ', n));
            col += n;
        } else {
            out.push(c);
            col += 1;
        }
    }
    out
}

impl View for CodeView {
    fn render(&mut self, canvas: &mut Canvas, rect: Rect, ctx: &mut Ctx) {
        canvas.fill_rect(rect, theme::BACKGROUND);
        self.view_w = rect.w as f32;
        self.view_h = rect.h as f32;
        self.clamp(ctx);

        let m = ctx.code_metrics();
        let lh = m.line_height;
        let pad_x = ctx.px(theme::PADDING_X);
        let pad_y = ctx.px(theme::PADDING_Y);
        let gutter_w = self.gutter_width(ctx);
        let char_w = self.char_width(ctx);

        let top = rect.y as f32 + pad_y - self.scroll.y;
        let first = ((self.scroll.y - pad_y) / lh).floor().max(0.0) as usize;
        let last = (((self.scroll.y - pad_y + rect.h as f32) / lh)
            .ceil()
            .max(0.0) as usize)
            .min(self.lines.len());

        // Drop cached lines far from the viewport.
        if self.cache.len() > 4 * (last - first + 1) + 64 {
            let keep = first.saturating_sub(64)..last + 64;
            self.cache.retain(|k, _| keep.contains(k));
        }

        let text_x = rect.x as f32 + gutter_w;
        let text_rect = Rect::new(
            text_x as i32,
            rect.y,
            (rect.w - gutter_w as i32).max(0),
            rect.h,
        );
        let gutter_rect = Rect::new(rect.x, rect.y, gutter_w as i32, rect.h);
        let outer = canvas.clip();

        for i in first..last {
            let y = (top + i as f32 * lh).round() as i32;
            // Line number, right-aligned in the gutter.
            canvas.set_clip(gutter_rect.intersect(&outer));
            let label = (i + 1).to_string();
            let lw = (label.len() as f32 * char_w) as i32;
            let lx = rect.x + gutter_w as i32 - pad_x as i32 - lw;
            ctx.label(
                canvas,
                &label,
                lx,
                y,
                theme::LINE_NUMBER,
                lw + 2,
                FontKind::Mono,
            );
            // Text.
            canvas.set_clip(text_rect.intersect(&outer));
            let x = (text_x - self.scroll.x).round() as i32;
            let mut buf = std::mem::replace(self.line_buffer(i, ctx), Buffer::new_empty(m));
            ctx.fonts
                .draw_buffer(canvas, &mut buf, x, y, theme::SYNTAX_NORMAL);
            *self.line_buffer(i, ctx) = buf;
        }
        canvas.set_clip(outer);

        let (_, content_h) = self.content_size(ctx);
        draw_vscrollbar(canvas, rect, self.scroll.y, content_h, ctx);
    }

    fn handle(&mut self, input: &Input, _rect: Rect, ctx: &mut Ctx) -> bool {
        let lh = ctx.code_metrics().line_height;
        let before = (self.scroll.x, self.scroll.y);
        if !self.scroll.wheel(input, lh) {
            let Input::Key { key, mods } = input else {
                return false;
            };
            let page = (self.view_h - lh * 2.0).max(lh);
            let col = self.char_width(ctx) * 4.0;
            match key {
                Key::Down => self.scroll.y += lh,
                Key::Up => self.scroll.y -= lh,
                Key::PageDown => self.scroll.y += page,
                Key::PageUp => self.scroll.y -= page,
                Key::Home => self.scroll.y = 0.0,
                Key::End => self.scroll.y = f32::MAX,
                Key::Right => self.scroll.x += col,
                Key::Left => self.scroll.x -= col,
                _ => return false,
            }
        }
        self.clamp(ctx);
        before != (self.scroll.x, self.scroll.y)
    }

    fn title(&self) -> String {
        self.doc.file_name()
    }

    fn status(&self) -> Vec<String> {
        vec![
            format!("{} lines", self.lines.len()),
            match self.doc.line_ending {
                crate::doc::LineEnding::Lf => "LF".into(),
                crate::doc::LineEnding::CrLf => "CRLF".into(),
                crate::doc::LineEnding::Cr => "CR".into(),
            },
            match self.doc.encoding {
                crate::doc::Encoding::Utf8 => "UTF-8".into(),
                crate::doc::Encoding::Utf8Bom => "UTF-8 BOM".into(),
                crate::doc::Encoding::Utf16Le => "UTF-16 LE".into(),
                crate::doc::Encoding::Utf16Be => "UTF-16 BE".into(),
                crate::doc::Encoding::Utf8Lossy => "UTF-8 (invalid bytes replaced)".into(),
            },
        ]
    }

    fn path(&self) -> Option<&Path> {
        Some(&self.doc.path)
    }

    fn reload(&mut self, _ctx: &mut Ctx) -> bool {
        match Document::open(&self.doc.path) {
            Ok(d) if d.text != self.doc.text => {
                self.doc = d;
                self.load_lines();
                true
            }
            _ => false,
        }
    }

    fn metrics_changed(&mut self) {
        self.cache.clear();
        self.char_w = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_expand_to_columns() {
        assert_eq!(expand_tabs("\tx", 100), "    x");
        assert_eq!(expand_tabs("ab\tc", 100), "ab  c");
        assert_eq!(display_len("ab\tc"), 5);
    }

    #[test]
    fn long_lines_are_cut() {
        let long = "x".repeat(10_000);
        let out = expand_tabs(&long, 100);
        assert_eq!(out.chars().count(), 101);
        assert!(out.ends_with('…'));
    }
}
