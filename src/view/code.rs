//! Code / plain-text view (lite-xl `DocView`, read-only for now).
//!
//! Each visible line is shaped into its own cosmic-text `Buffer` and cached,
//! so scrolling a huge file only ever shapes what is on screen.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use cosmic_text::{Attrs, Buffer, Family, Metrics, Shaping, Style, Weight, Wrap};

use crate::canvas::{Canvas, Rect};
use crate::doc::Document;
use crate::input::{Input, Key};
use crate::syntax::{LineHighlighter, Span, find_syntax};
use crate::text::display_char;
use crate::theme;
use crate::ui::{Ctx, FontKind};
use crate::view::{Scroll, Tick, View, draw_vscrollbar};

/// Lines longer than this are cut for display (minified files). The full line
/// is kept in memory; only shaping is capped.
pub const MAX_DISPLAY_CHARS: usize = 4000;
pub const TAB_WIDTH: usize = 4;

pub struct CodeView {
    doc: Document,
    lines: Vec<String>,
    scroll: Scroll,
    /// Shaped lines around the viewport; the flag says whether the line was
    /// shaped with syntax colours (so it is rebuilt once highlighting arrives).
    cache: HashMap<usize, (Buffer, bool)>,
    highlighter: LineHighlighter,
    /// (font size, char width) — recomputed when the font size changes.
    char_w: Option<(f32, f32)>,
    longest_line_chars: usize,
    view_w: f32,
    view_h: f32,
    /// Visible line range at the last render.
    visible: (usize, usize),
}

impl CodeView {
    pub fn new(doc: Document) -> Self {
        let first_line = doc.text.lines().next().unwrap_or("");
        let highlighter = LineHighlighter::new(find_syntax(&doc.path, first_line));
        let mut v = Self {
            doc,
            lines: Vec::new(),
            scroll: Scroll::default(),
            cache: HashMap::new(),
            highlighter,
            char_w: None,
            longest_line_chars: 0,
            view_w: 0.0,
            view_h: 0.0,
            visible: (0, usize::MAX),
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

    /// First line at the top of the view.
    pub fn top_line(&self, ctx: &Ctx) -> usize {
        let lh = ctx.code_metrics().line_height;
        let pad = ctx.px(theme::PADDING_Y);
        (((self.scroll.y - pad).max(0.0)) / lh).floor() as usize
    }

    /// Scroll so `line` is at the top (clamped on next render).
    pub fn scroll_to_line(&mut self, line: usize, ctx: &Ctx) {
        self.scroll.y = line as f32 * ctx.code_metrics().line_height;
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
        let spans = self.highlighter.spans(i);
        let stale = match self.cache.get(&i) {
            Some((_, colored)) => !colored && spans.is_some(),
            None => true,
        };
        if stale {
            let fam = ctx.family(FontKind::Mono);
            let b = shape_code_line(&self.lines[i], spans, &fam, ctx.code_metrics());
            self.cache.insert(i, (b, spans.is_some()));
        }
        &mut self.cache.get_mut(&i).expect("just inserted").0
    }
}

/// Split a line into styled pieces with tabs expanded, cut at `max_cols`.
/// `spans` are byte ranges into the original (unexpanded) line.
pub fn styled_pieces(line: &str, spans: &[Span], max_cols: usize) -> Vec<(String, Option<Span>)> {
    let mut pieces: Vec<(String, Option<Span>)> = Vec::new();
    let mut col = 0;
    let mut si = 0;
    for (b, c) in line.char_indices() {
        let b = b as u32;
        while si < spans.len() && spans[si].end <= b {
            si += 1;
        }
        let span = spans.get(si).copied().filter(|s| s.start <= b);
        if col >= max_cols {
            pieces.push(("…".into(), None));
            break;
        }
        let text: String = if c == '\t' {
            let n = TAB_WIDTH - col % TAB_WIDTH;
            col += n;
            " ".repeat(n)
        } else {
            col += 1;
            display_char(c).to_string()
        };
        match pieces.last_mut() {
            Some((t, s)) if *s == span => t.push_str(&text),
            _ => pieces.push((text, span)),
        }
    }
    pieces
}

/// Shape one code line (optionally syntax-coloured) into a buffer.
pub fn shape_code_line(line: &str, spans: Option<&[Span]>, family: &str, m: Metrics) -> Buffer {
    let mut b = Buffer::new_empty(m);
    b.set_wrap(Wrap::None);
    let base = Attrs::new().family(Family::Name(family));
    match spans {
        Some(spans) if !spans.is_empty() => {
            let pieces = styled_pieces(line, spans, MAX_DISPLAY_CHARS);
            let rich = pieces.iter().map(|(t, s)| {
                let mut a = base.clone();
                if let Some(s) = s {
                    a = a.color(cosmic_text::Color::rgb(s.color.r, s.color.g, s.color.b));
                    if s.bold {
                        a = a.weight(Weight::BOLD);
                    }
                    if s.italic {
                        a = a.style(Style::Italic);
                    }
                }
                (t.as_str(), a)
            });
            b.set_rich_text(rich, &base, Shaping::Advanced, None);
        }
        _ => b.set_text(
            &expand_tabs(line, MAX_DISPLAY_CHARS),
            &base,
            Shaping::Advanced,
            None,
        ),
    }
    b
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
            out.push(display_char(c));
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

        self.visible = (first, last);
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
            let Input::Key { key, .. } = input else {
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

    fn tick(&mut self, _ctx: &mut Ctx, budget: Duration) -> Tick {
        let before = self.highlighter.done();
        let more = self.highlighter.step(&self.lines, budget);
        let after = self.highlighter.done();
        let (first, last) = self.visible;
        // Redraw only if newly highlighted lines are on screen.
        let redraw = after > before && before < last && after > first;
        Tick { more, redraw }
    }

    fn title(&self) -> String {
        self.doc.file_name()
    }

    fn status(&self) -> Vec<String> {
        vec![
            format!("{} lines", self.lines.len()),
            self.highlighter.name().to_string(),
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
                let first_line = self.lines.first().map(String::as_str).unwrap_or("");
                self.highlighter = LineHighlighter::new(find_syntax(&self.doc.path, first_line));
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
