//! The code / text editor view (lite-xl `DocView`).
//!
//! Each visible line is shaped into its own cosmic-text buffer, cached by the
//! line's content (so edits can never leave a stale line on screen). One
//! mapping between buffer columns and display x-positions — tabs, control
//! pictures, the long-line cut, ligatures and wide characters included — is
//! shared by caret drawing and mouse hit-testing.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use cosmic_text::{Attrs, Buffer, Family, Metrics, Shaping, Style, Weight, Wrap};

use crate::canvas::{Canvas, Rect};
use crate::doc::Document;
use crate::editor::buffer::{Eol, Pos};
use crate::editor::doc::{EditDoc, Move};
use crate::input::{Cursor, Input, Key, MouseButton};
use crate::syntax::{LineHighlighter, Span, find_syntax};
use crate::text::display_char;
use crate::theme;
use crate::ui::{Ctx, FontKind};
use crate::view::{Scroll, Tick, View, draw_vscrollbar};

/// Lines longer than this are cut for display (minified files). Editing
/// beyond the cut still works; the caret is drawn at the cut (D-015).
pub const MAX_DISPLAY_CHARS: usize = 4000;
pub const TAB_WIDTH: usize = 4;

/// A source line prepared for display, with a byte-offset mapping.
pub struct DisplayLine {
    pub text: String,
    /// Source byte offset of each char start (plus the line end), ascending.
    src: Vec<u32>,
    /// Display byte offset for the matching `src` entry.
    disp: Vec<u32>,
}

impl DisplayLine {
    pub fn new(line: &str, max_cols: usize) -> Self {
        let mut text = String::with_capacity(line.len().min(max_cols + 8));
        let mut src = Vec::with_capacity(line.len() + 1);
        let mut disp = Vec::with_capacity(line.len() + 1);
        let mut col = 0;
        let mut cut = false;
        for (b, c) in line.char_indices() {
            src.push(b as u32);
            disp.push(text.len() as u32);
            if col >= max_cols {
                text.push('…');
                cut = true;
                break;
            }
            if c == '\t' {
                let n = TAB_WIDTH - col % TAB_WIDTH;
                text.extend(std::iter::repeat_n(' ', n));
                col += n;
            } else {
                text.push(display_char(c));
                col += 1;
            }
        }
        if !cut {
            src.push(line.len() as u32);
            disp.push(text.len() as u32);
        }
        Self { text, src, disp }
    }

    /// Display byte offset for a source byte offset (clamped to the cut).
    pub fn to_disp(&self, src_byte: usize) -> usize {
        match self.src.binary_search(&(src_byte as u32)) {
            Ok(i) => self.disp[i] as usize,
            Err(i) => self.disp.get(i.saturating_sub(1)).copied().unwrap_or(0) as usize,
        }
        .min(self.text.len())
    }

    /// Source byte offset for a display byte offset.
    pub fn to_src(&self, disp_byte: usize) -> usize {
        match self.disp.binary_search(&(disp_byte as u32)) {
            Ok(i) => self.src[i] as usize,
            Err(i) => self.src.get(i.saturating_sub(1)).copied().unwrap_or(0) as usize,
        }
    }
}

/// Split a line into styled pieces (tabs expanded, cut at `max_cols`).
/// `spans` are byte ranges into the original line.
pub fn styled_pieces(line: &str, spans: &[Span], max_cols: usize) -> Vec<(String, Option<Span>)> {
    let dl = DisplayLine::new(line, max_cols);
    styled_from_display(line, &dl, spans)
}

fn styled_from_display(
    line: &str,
    dl: &DisplayLine,
    spans: &[Span],
) -> Vec<(String, Option<Span>)> {
    if spans.is_empty() {
        return vec![(dl.text.clone(), None)];
    }
    let mut out: Vec<(String, Option<Span>)> = Vec::new();
    let mut pos = 0usize; // display byte
    for s in spans {
        let (a, b) = (
            dl.to_disp(s.start as usize),
            dl.to_disp((s.end as usize).min(line.len())),
        );
        if a > pos {
            out.push((dl.text[pos..a].to_string(), None));
        }
        if b > a.max(pos) {
            out.push((dl.text[a.max(pos)..b].to_string(), Some(*s)));
            pos = b;
        }
    }
    if pos < dl.text.len() {
        out.push((dl.text[pos..].to_string(), None));
    }
    out
}

/// Shape one code line (optionally syntax-coloured) into a buffer.
pub fn shape_code_line(line: &str, spans: Option<&[Span]>, family: &str, m: Metrics) -> Buffer {
    shape_display(
        line,
        &DisplayLine::new(line, MAX_DISPLAY_CHARS),
        spans,
        family,
        m,
    )
}

fn shape_display(
    line: &str,
    dl: &DisplayLine,
    spans: Option<&[Span]>,
    family: &str,
    m: Metrics,
) -> Buffer {
    let mut b = Buffer::new_empty(m);
    b.set_wrap(Wrap::None);
    let base = Attrs::new().family(Family::Name(family));
    let pieces = styled_from_display(line, dl, spans.unwrap_or(&[]));
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
    b
}

/// Expand tabs to spaces and cut at `max` columns.
pub fn expand_tabs(line: &str, max: usize) -> String {
    DisplayLine::new(line, max).text
}

struct CachedLine {
    source: String,
    colored: bool,
    buffer: Buffer,
    dl: DisplayLine,
}

pub struct CodeView {
    pub doc: EditDoc,
    scroll: Scroll,
    cache: HashMap<usize, CachedLine>,
    highlighter: LineHighlighter,
    /// (font size, char width) — recomputed when the font size changes.
    char_w: Option<(f32, f32)>,
    longest: (u64, usize),
    view_w: f32,
    view_h: f32,
    last_rect: Rect,
    visible: (usize, usize),
    dragging: bool,
    /// Scroll so the caret is visible at the next render.
    reveal_caret: bool,
    pub read_only: bool,
}

impl CodeView {
    pub fn new(doc: Document) -> Self {
        let first_line = doc.text.lines().next().unwrap_or("").to_string();
        let highlighter = LineHighlighter::new(find_syntax(&doc.path, &first_line));
        Self {
            doc: EditDoc::new(doc),
            scroll: Scroll::default(),
            cache: HashMap::new(),
            highlighter,
            char_w: None,
            longest: (u64::MAX, 0),
            view_w: 0.0,
            view_h: 0.0,
            last_rect: Rect::new(0, 0, 0, 0),
            visible: (0, usize::MAX),
            dragging: false,
            reveal_caret: false,
            read_only: false,
        }
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
        let digits = self.doc.buf.line_count().to_string().len().max(3) as f32;
        (digits * self.char_width(ctx)).ceil() + ctx.px(theme::PADDING_X) * 2.0
    }

    fn longest_line(&mut self) -> usize {
        let v = self.doc.version();
        if self.longest.0 != v {
            let n = self
                .doc
                .buf
                .lines()
                .iter()
                .map(|l| l.chars().count().min(MAX_DISPLAY_CHARS))
                .max()
                .unwrap_or(0);
            self.longest = (v, n);
        }
        self.longest.1
    }

    fn content_size(&mut self, ctx: &mut Ctx) -> (f32, f32) {
        let lh = ctx.code_metrics().line_height;
        let w = (self.longest_line() + 2) as f32 * self.char_width(ctx)
            + ctx.px(theme::PADDING_X) * 2.0;
        (
            w,
            self.doc.buf.line_count() as f32 * lh + ctx.px(theme::PADDING_Y) * 2.0,
        )
    }

    fn clamp(&mut self, ctx: &mut Ctx) {
        let (cw, ch) = self.content_size(ctx);
        let (vw, vh) = (self.view_w, self.view_h);
        self.scroll.clamp(cw, ch, vw, vh);
    }

    fn line(&mut self, i: usize, ctx: &mut Ctx) -> &CachedLine {
        let spans = self.highlighter.spans(i);
        let source = self.doc.buf.line(i);
        let stale = match self.cache.get(&i) {
            Some(c) => c.source != source || (!c.colored && spans.is_some()),
            None => true,
        };
        if stale {
            let fam = ctx.family(FontKind::Mono);
            let dl = DisplayLine::new(source, MAX_DISPLAY_CHARS);
            let mut buffer = shape_display(source, &dl, spans, &fam, ctx.code_metrics());
            buffer.shape_until_scroll(&mut ctx.fonts.system, false);
            self.cache.insert(
                i,
                CachedLine {
                    source: source.to_string(),
                    colored: spans.is_some(),
                    buffer,
                    dl,
                },
            );
        }
        &self.cache[&i]
    }

    /// x offset (from the text origin) of byte column `col` on line `i`.
    fn col_to_x(&mut self, i: usize, col: usize, ctx: &mut Ctx) -> f32 {
        let c = self.line(i, ctx);
        let d = c.dl.to_disp(col);
        let Some(run) = c.buffer.layout_runs().next() else {
            return 0.0;
        };
        for g in run.glyphs {
            if d >= g.start && d < g.end.max(g.start + 1) {
                // Inside a ligature/cluster: interpolate by bytes.
                let span = (g.end - g.start).max(1) as f32;
                return g.x + g.w * ((d - g.start) as f32 / span);
            }
        }
        run.line_w
    }

    /// Byte column on line `i` nearest to x (from the text origin).
    fn x_to_col(&mut self, i: usize, x: f32, ctx: &mut Ctx) -> usize {
        let c = self.line(i, ctx);
        let Some(run) = c.buffer.layout_runs().next() else {
            return 0;
        };
        for g in run.glyphs {
            if x < g.x + g.w {
                let span = (g.end - g.start).max(1);
                let frac = ((x - g.x) / g.w.max(0.01)).clamp(0.0, 1.0);
                let d = g.start + (frac * span as f32).round() as usize;
                let src = c.dl.to_src(d);
                return self.doc.buf.clamp(Pos::new(i, src)).col;
            }
        }
        if c.dl.text.ends_with('…') && c.source.chars().count() > MAX_DISPLAY_CHARS {
            return c.dl.to_src(c.dl.text.len());
        }
        c.source.len()
    }

    fn pos_at(&mut self, x: f32, y: f32, ctx: &mut Ctx) -> Pos {
        let lh = ctx.code_metrics().line_height;
        let pad_y = ctx.px(theme::PADDING_Y);
        let rel = y - self.last_rect.y as f32 - pad_y + self.scroll.y;
        let line = if rel < 0.0 {
            0
        } else {
            ((rel / lh) as usize).min(self.doc.buf.line_count() - 1)
        };
        let gutter = self.gutter_width(ctx);
        let tx = x - self.last_rect.x as f32 - gutter + self.scroll.x;
        Pos::new(line, self.x_to_col(line, tx, ctx))
    }

    fn ensure_caret_visible(&mut self, ctx: &mut Ctx) {
        let lh = ctx.code_metrics().line_height;
        let pad_y = ctx.px(theme::PADDING_Y);
        let head = self.doc.primary().head;
        let y = head.line as f32 * lh + pad_y;
        let margin = lh;
        if y - margin < self.scroll.y {
            self.scroll.y = (y - margin).max(0.0);
        } else if y + lh + margin > self.scroll.y + self.view_h {
            self.scroll.y = y + lh + margin - self.view_h;
        }
        let gutter = self.gutter_width(ctx);
        let x = self.col_to_x(head.line, head.col, ctx);
        let cw = self.char_width(ctx);
        let text_w = (self.view_w - gutter).max(cw * 4.0);
        if x - cw * 2.0 < self.scroll.x {
            self.scroll.x = (x - cw * 4.0).max(0.0);
        } else if x + cw * 2.0 > self.scroll.x + text_w {
            self.scroll.x = x + cw * 4.0 - text_w;
        }
    }

    fn page_lines(&self, ctx: &Ctx) -> usize {
        let lh = ctx.code_metrics().line_height;
        ((self.view_h / lh) as usize).saturating_sub(2).max(1)
    }

    /// Something changed the text: re-highlight from the first touched line.
    fn after_edit(&mut self, from_line: usize) {
        self.highlighter.invalidate_from(from_line);
        self.cache.retain(|&k, _| k < from_line);
        self.reveal_caret = true;
    }

    fn first_sel_line(&self) -> usize {
        self.doc
            .sels
            .iter()
            .map(|s| s.range().0.line)
            .min()
            .unwrap_or(0)
    }

    fn handle_key(&mut self, key: &Key, mods: crate::input::Mods, ctx: &mut Ctx) -> bool {
        let shift = mods.shift;
        let edit_from = self.first_sel_line().saturating_sub(1);
        let ro = self.read_only;
        let page = self.page_lines(ctx);
        let lh = ctx.code_metrics().line_height;
        let mut mv = |v: &mut Self, m: Move| {
            v.doc.move_carets(m, shift);
            v.reveal_caret = true;
        };
        match (key, mods.ctrl, mods.alt) {
            (Key::Left, ctrl, false) => mv(self, if ctrl { Move::WordLeft } else { Move::Left }),
            (Key::Right, ctrl, false) => mv(self, if ctrl { Move::WordRight } else { Move::Right }),
            (Key::Up, false, false) => mv(self, Move::Up),
            (Key::Down, false, false) => mv(self, Move::Down),
            (Key::Up, true, false) => self.scroll.y -= lh,
            (Key::Down, true, false) => self.scroll.y += lh,
            (Key::Home, ctrl, _) => mv(self, if ctrl { Move::DocStart } else { Move::Home }),
            (Key::End, ctrl, _) => mv(self, if ctrl { Move::DocEnd } else { Move::End }),
            (Key::PageUp, _, _) => {
                mv(self, Move::PageUp(page));
                self.scroll.y -= page as f32 * lh;
            }
            (Key::PageDown, _, _) => {
                mv(self, Move::PageDown(page));
                self.scroll.y += page as f32 * lh;
            }
            (Key::Char('a'), true, false) if !shift => self.doc.select_all(),
            (Key::Char('l'), true, false) if !shift => {
                let line = self.doc.primary().head.line;
                self.doc.select_line_at(line);
            }
            (Key::Enter, false, false) if !ro => {
                self.doc.newline();
                self.after_edit(edit_from);
            }
            (Key::Tab, false, false) if !ro => {
                if shift {
                    self.doc.outdent_lines()
                } else {
                    self.doc.indent_lines()
                }
                self.after_edit(edit_from);
            }
            (Key::Backspace, ctrl, false) if !ro => {
                self.doc.backspace(ctrl);
                self.after_edit(edit_from);
            }
            (Key::Delete, ctrl, false) if !ro && !shift => {
                self.doc.delete_forward(ctrl);
                self.after_edit(edit_from);
            }
            (Key::Char('z'), true, false) if !ro && !shift => {
                if self.doc.undo() {
                    self.after_edit(0);
                }
            }
            (Key::Char('y'), true, false) | (Key::Char('z'), true, false) if !ro => {
                if self.doc.redo() {
                    self.after_edit(0);
                }
            }
            (Key::Char('d'), true, false) if !ro && shift => {
                self.doc.duplicate_lines();
                self.after_edit(edit_from);
            }
            (Key::Char('k'), true, false) if !ro && shift => {
                self.doc.delete_lines();
                self.after_edit(edit_from);
            }
            (Key::Char(']'), true, false) if !ro => {
                self.doc.indent_lines();
                self.after_edit(edit_from);
            }
            (Key::Char('['), true, false) if !ro => {
                self.doc.outdent_lines();
                self.after_edit(edit_from);
            }
            _ => return false,
        }
        true
    }

    pub fn status_items(&self) -> Vec<String> {
        let head = self.doc.primary().head;
        let col = self.doc.buf.line(head.line)[..head.col].chars().count() + 1;
        let n = self.doc.buf.line_count();
        let mut v = vec![
            format!("Ln {}, Col {}", head.line + 1, col),
            format!("{n} {}", if n == 1 { "line" } else { "lines" }),
            self.highlighter.name().to_string(),
        ];
        let eol = match self.doc.buf.default_eol() {
            Eol::Lf => "LF",
            Eol::CrLf => "CRLF",
            Eol::Cr => "CR",
        };
        v.push(if self.doc.buf.has_mixed_eols() {
            format!("{eol} (mixed)")
        } else {
            eol.to_string()
        });
        v.push(
            match self.doc.encoding {
                crate::doc::Encoding::Utf8 => "UTF-8",
                crate::doc::Encoding::Utf8Bom => "UTF-8 BOM",
                crate::doc::Encoding::Utf16Le => "UTF-16 LE",
                crate::doc::Encoding::Utf16Be => "UTF-16 BE",
                crate::doc::Encoding::Utf8Lossy => "UTF-8 (invalid bytes replaced)",
            }
            .to_string(),
        );
        if self.read_only {
            v.push("read-only".into());
        }
        v
    }
}

impl View for CodeView {
    fn render(&mut self, canvas: &mut Canvas, rect: Rect, ctx: &mut Ctx) {
        canvas.fill_rect(rect, theme::BACKGROUND);
        self.last_rect = rect;
        self.view_w = rect.w as f32;
        self.view_h = rect.h as f32;
        if self.reveal_caret {
            self.ensure_caret_visible(ctx);
            self.reveal_caret = false;
        }
        self.clamp(ctx);

        let m = ctx.code_metrics();
        let lh = m.line_height;
        let pad_x = ctx.px(theme::PADDING_X);
        let pad_y = ctx.px(theme::PADDING_Y);
        let gutter_w = self.gutter_width(ctx);
        let char_w = self.char_width(ctx);
        let line_count = self.doc.buf.line_count();

        let top = rect.y as f32 + pad_y - self.scroll.y;
        let first = ((self.scroll.y - pad_y) / lh).floor().max(0.0) as usize;
        let last = (((self.scroll.y - pad_y + rect.h as f32) / lh)
            .ceil()
            .max(0.0) as usize)
            .min(line_count);
        self.visible = (first, last);
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
        let sels = self.doc.sels.clone();
        let caret_lines: Vec<usize> = sels.iter().map(|s| s.head.line).collect();
        let any_selection = sels.iter().any(|s| !s.is_empty());
        let ox = text_x - self.scroll.x;

        for i in first..last {
            let y = (top + i as f32 * lh).round() as i32;
            let row = Rect::new(rect.x, y, rect.w, lh as i32);
            let is_caret_line = caret_lines.contains(&i);
            if is_caret_line && !any_selection && ctx.focused {
                canvas.fill_rect(row.intersect(&outer), theme::LINE_HIGHLIGHT);
            }
            // Line number, right-aligned in the gutter.
            canvas.set_clip(gutter_rect.intersect(&outer));
            let label = (i + 1).to_string();
            let lw = (label.len() as f32 * char_w) as i32;
            let lx = rect.x + gutter_w as i32 - pad_x as i32 - lw;
            let num_color = if is_caret_line {
                theme::LINE_NUMBER2
            } else {
                theme::LINE_NUMBER
            };
            ctx.label(canvas, &label, lx, y, num_color, lw + 2, FontKind::Mono);

            canvas.set_clip(text_rect.intersect(&outer));
            // Selections.
            for s in &sels {
                let (a, b) = s.range();
                if a == b || i < a.line || i > b.line {
                    continue;
                }
                let x0 = if i == a.line {
                    self.col_to_x(i, a.col, ctx)
                } else {
                    0.0
                };
                let x1 = if i == b.line {
                    self.col_to_x(i, b.col, ctx)
                } else {
                    let end = self.doc.buf.line(i).len();
                    self.col_to_x(i, end, ctx) + char_w * 0.6
                };
                canvas.fill_rect(
                    Rect::new(
                        (ox + x0).round() as i32,
                        y,
                        (x1 - x0).round().max(1.0) as i32,
                        lh as i32,
                    ),
                    theme::SELECTION,
                );
            }
            // Text.
            let x = ox.round() as i32;
            self.line(i, ctx);
            if let Some(c) = self.cache.get_mut(&i) {
                ctx.fonts
                    .draw_buffer(canvas, &mut c.buffer, x, y, theme::SYNTAX_NORMAL);
            }
            // Carets.
            if ctx.focused {
                let cw = ctx.px(2.0).max(1.0) as i32;
                for s in &sels {
                    if s.head.line == i {
                        let cx = (ox + self.col_to_x(i, s.head.col, ctx)).round() as i32;
                        canvas.fill_rect(Rect::new(cx, y, cw, lh as i32), theme::CARET);
                    }
                }
            }
        }
        canvas.set_clip(outer);

        let (_, content_h) = self.content_size(ctx);
        draw_vscrollbar(canvas, rect, self.scroll.y, content_h, ctx);
    }

    fn handle(&mut self, input: &Input, rect: Rect, ctx: &mut Ctx) -> bool {
        self.last_rect = rect;
        let lh = ctx.code_metrics().line_height;
        let before = (
            self.scroll.x,
            self.scroll.y,
            self.doc.version(),
            self.doc.sels.clone(),
        );
        match input {
            Input::Wheel { .. } => {
                self.scroll.wheel(input, lh);
            }
            Input::Text(t) if !self.read_only => {
                let from = self.first_sel_line();
                self.doc.insert_text(t);
                self.after_edit(from.saturating_sub(1));
            }
            Input::Key { key, mods } => {
                if !self.handle_key(key, *mods, ctx) {
                    return false;
                }
            }
            Input::MouseMove { x, y } => {
                let gutter = self.gutter_width(ctx);
                let over_text = *x >= rect.x as f32 + gutter;
                *ctx.cursor = if over_text || self.dragging {
                    Cursor::Text
                } else {
                    Cursor::Default
                };
                if self.dragging {
                    let p = self.pos_at(*x, *y, ctx);
                    self.doc.drag_to(p);
                    self.reveal_caret = true;
                }
            }
            Input::MouseDown {
                x,
                y,
                button: MouseButton::Left,
                clicks,
            } => {
                let p = self.pos_at(*x, *y, ctx);
                let gutter = self.gutter_width(ctx);
                if *x < rect.x as f32 + gutter {
                    self.doc.select_line_at(p.line);
                } else {
                    match clicks {
                        2 => self.doc.select_word_at(p),
                        3 => self.doc.select_line_at(p.line),
                        _ => self.doc.set_caret(p, false),
                    }
                    self.dragging = *clicks == 1;
                }
            }
            Input::MouseUp { .. } => {
                self.dragging = false;
                return false;
            }
            _ => return false,
        }
        self.clamp(ctx);
        before
            != (
                self.scroll.x,
                self.scroll.y,
                self.doc.version(),
                self.doc.sels.clone(),
            )
    }

    fn tick(&mut self, _ctx: &mut Ctx, budget: Duration) -> Tick {
        let before = self.highlighter.done();
        let more = self.highlighter.step(self.doc.buf.lines(), budget);
        let after = self.highlighter.done();
        let (first, last) = self.visible;
        let redraw = after > before && before < last && after > first;
        Tick { more, redraw }
    }

    fn title(&self) -> String {
        let name = self
            .doc
            .path()
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.doc.path().display().to_string());
        if self.doc.is_dirty() {
            format!("● {name}")
        } else {
            name
        }
    }

    fn status(&self) -> Vec<String> {
        self.status_items()
    }

    fn path(&self) -> Option<&Path> {
        Some(self.doc.path())
    }

    fn reload(&mut self, _ctx: &mut Ctx) -> bool {
        if self.doc.is_dirty() {
            return false; // never discard edits silently (S2.6 prompts)
        }
        match Document::open(self.doc.path()) {
            Ok(d) if d.text != self.doc.buf.to_text() => {
                self.doc.replace_all_from_disk(d);
                self.after_edit(0);
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
    }

    #[test]
    fn long_lines_are_cut() {
        let long = "x".repeat(10_000);
        let out = expand_tabs(&long, 100);
        assert_eq!(out.chars().count(), 101);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn display_mapping_round_trips() {
        let line = "a\tb\u{1}é";
        let dl = DisplayLine::new(line, 100);
        assert_eq!(dl.text, "a   b\u{2401}é");
        for (b, _) in line
            .char_indices()
            .chain(std::iter::once((line.len(), ' ')))
        {
            assert_eq!(dl.to_src(dl.to_disp(b)), b, "byte {b}");
        }
        assert_eq!(dl.to_disp(1), 1);
        assert_eq!(dl.to_disp(2), 4); // after the tab
    }

    #[test]
    fn styled_pieces_follow_spans_across_tabs() {
        let span = Span {
            start: 1,
            end: 3,
            color: theme::SYNTAX_STRING,
            bold: false,
            italic: false,
        };
        let p = styled_pieces("x\tyz", &[span], 100);
        assert_eq!(p[0].0, "x");
        assert_eq!(p[1].0, "   y");
        assert_eq!(p[2].0, "z");
    }
}
