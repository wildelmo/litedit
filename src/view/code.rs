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
use crate::editor::doc::SaveError;
use crate::editor::doc::{EditDoc, Move, snap_grapheme};
use crate::find::Finder;
use crate::input::{Cursor, Input, Key, MouseButton};
use crate::inputbar::BarEvent;
use crate::syntax::{LineHighlighter, Span, find_syntax};
use crate::text::display_char;
use crate::theme;
use crate::ui::{Ctx, FontKind};
use crate::view::wrap::{Rows, row_of, wrap_breaks};
use crate::view::{DiskState, Scroll, Tick, View, draw_vscrollbar};

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
    /// The highlight spans this line was shaped with (None = plain).
    spans: Option<Vec<Span>>,
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
    /// Text of the last line-mode copy (pasting it inserts whole lines).
    line_clip: Option<String>,
    caret_screen: Option<Rect>,
    /// Find / replace bar (open when Some).
    find: Option<Finder>,
    find_focus: bool,
    /// Last search, kept for F3 after the bar is closed.
    last_find: Option<Finder>,
    /// Where the caret was when the find bar opened (incremental search origin).
    find_origin: Pos,
    /// Status of the last replace-all, etc.
    find_note: Option<String>,
    /// Center this line on the next render (go to line).
    goto_center: Option<usize>,
    /// Match count text keyed by (version, pattern, case, regex).
    find_count: Option<((u64, String, bool, bool), String)>,
    find_count_stale: bool,
    /// Soft wrap (Alt+Z). On by default for markdown (D-022).
    pub wrap: bool,
    rows: Rows,
    /// Remembered x for repeated visual up/down moves in wrap mode.
    goal_x: Option<f32>,
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
            line_clip: None,
            caret_screen: None,
            find: None,
            find_focus: false,
            last_find: None,
            find_origin: Pos::default(),
            find_note: None,
            goto_center: None,
            find_count: None,
            find_count_stale: false,
            wrap: false,
            rows: Rows::default(),
            goal_x: None,
        }
    }

    /// First line at the top of the view.
    pub fn top_line(&self, ctx: &Ctx) -> usize {
        let lh = ctx.code_metrics().line_height;
        let pad = ctx.px(theme::PADDING_Y);
        let row = (((self.scroll.y - pad).max(0.0)) / lh).floor() as usize;
        self.rows.line_of_row(row).0
    }

    /// Scroll so `line` is at the top (clamped on next render).
    pub fn scroll_to_line(&mut self, line: usize, ctx: &Ctx) {
        self.scroll.y = self.rows.first_row(line) as f32 * ctx.code_metrics().line_height;
    }

    /// Columns per row when wrapping, from the current view width.
    fn wrap_cols(&mut self, ctx: &mut Ctx) -> Option<usize> {
        if !self.wrap {
            return None;
        }
        let gutter = self.gutter_width(ctx);
        let cw = self.char_width(ctx);
        let usable = self.view_w - gutter - ctx.px(theme::PADDING_X);
        Some(((usable / cw).floor() as usize).max(8))
    }

    fn update_rows(&mut self, ctx: &mut Ctx) {
        let cols = self.wrap_cols(ctx);
        self.rows
            .update(self.doc.buf.lines(), self.doc.version(), cols);
    }

    /// Row start offsets for line `i` (just `[0]` when not wrapping).
    fn breaks(&self, i: usize) -> Vec<usize> {
        match self.rows.wrapping() {
            Some(c) => wrap_breaks(self.doc.buf.line(i), c),
            None => vec![0],
        }
    }

    /// Visual row of a position and its x offset within that row.
    fn pos_xy(&mut self, p: Pos, ctx: &mut Ctx) -> (usize, f32) {
        let br = self.breaks(p.line);
        let k = row_of(&br, p.col);
        let x = self.col_to_x(p.line, p.col, ctx) - self.col_to_x(p.line, br[k], ctx);
        (self.rows.first_row(p.line) + k, x)
    }

    /// Byte column on visual row (line, k) nearest to x within the row.
    fn row_x_to_col(&mut self, line: usize, k: usize, x: f32, ctx: &mut Ctx) -> usize {
        let br = self.breaks(line);
        let k = k.min(br.len() - 1);
        let start = br[k];
        let start_x = self.col_to_x(line, start, ctx);
        let col = self.x_to_col(line, x + start_x, ctx);
        let text = self.doc.buf.line(line);
        match br.get(k + 1) {
            // Not the last row: stay before the next row's first char.
            Some(&next) => {
                let last = text[..next]
                    .char_indices()
                    .next_back()
                    .map_or(start, |(b, _)| b)
                    .max(start);
                snap_grapheme(text, col.clamp(start, last))
            }
            None => col.max(start),
        }
    }

    /// Move the primary caret by visual rows (wrap mode).
    fn move_visual(&mut self, drows: isize, extend: bool, ctx: &mut Ctx) {
        self.update_rows(ctx); // never rely on a previous frame
        let head = self.doc.primary().head;
        let (row, x) = self.pos_xy(head, ctx);
        let gx = *self.goal_x.get_or_insert(x);
        let target =
            (row as isize + drows).clamp(0, self.rows.total().saturating_sub(1) as isize) as usize;
        let (line, k) = self.rows.line_of_row(target);
        let col = self.row_x_to_col(line, k, gx, ctx);
        self.doc.set_caret(Pos::new(line, col), extend);
        self.reveal_caret = true;
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
        let w = if self.wrap {
            0.0 // no horizontal scrolling while wrapping
        } else {
            (self.longest_line() + 2) as f32 * self.char_width(ctx) + ctx.px(theme::PADDING_X) * 2.0
        };
        (
            w,
            self.rows.total().max(1) as f32 * lh + ctx.px(theme::PADDING_Y) * 2.0,
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
        // Rebuild when the text changed, or new highlighting differs from
        // what was drawn. While a line's spans are being recomputed (after an
        // edit above it), keep showing its previous colours — no flash.
        let stale = match self.cache.get(&i) {
            Some(c) => c.source != source || (spans.is_some() && c.spans.as_deref() != spans),
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
                    spans: spans.map(<[Span]>::to_vec),
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
                let col = self.doc.buf.clamp(Pos::new(i, src)).col;
                return snap_grapheme(self.doc.buf.line(i), col);
            }
        }
        if c.dl.text.ends_with('…') && c.source.chars().count() > MAX_DISPLAY_CHARS {
            return c.dl.to_src(c.dl.text.len());
        }
        c.source.len()
    }

    fn pos_at(&mut self, x: f32, y: f32, ctx: &mut Ctx) -> Pos {
        self.update_rows(ctx);
        let lh = ctx.code_metrics().line_height;
        let pad_y = ctx.px(theme::PADDING_Y);
        let rel = y - self.last_rect.y as f32 - pad_y + self.scroll.y;
        let row = if rel < 0.0 { 0 } else { (rel / lh) as usize };
        let (line, k) = self.rows.line_of_row(row);
        let gutter = self.gutter_width(ctx);
        let tx = x - self.last_rect.x as f32 - gutter + self.scroll.x;
        Pos::new(line, self.row_x_to_col(line, k, tx, ctx))
    }

    fn ensure_caret_visible(&mut self, ctx: &mut Ctx) {
        self.update_rows(ctx);
        let lh = ctx.code_metrics().line_height;
        let pad_y = ctx.px(theme::PADDING_Y);
        let head = self.doc.primary().head;
        let (row, row_x) = self.pos_xy(head, ctx);
        let y = row as f32 * lh + pad_y;
        let margin = lh;
        if y - margin < self.scroll.y {
            self.scroll.y = (y - margin).max(0.0);
        } else if y + lh + margin > self.scroll.y + self.view_h {
            self.scroll.y = y + lh + margin - self.view_h;
        }
        if self.wrap {
            self.scroll.x = 0.0;
            return;
        }
        let gutter = self.gutter_width(ctx);
        let x = row_x;
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
        // Cached lines stay: they are re-shaped only if their text or their
        // colours actually change.
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
        if !matches!(key, Key::Up | Key::Down | Key::PageUp | Key::PageDown) {
            self.goal_x = None;
        }
        if self.wrap && !mods.ctrl && !mods.alt {
            let rows = match key {
                Key::Up => Some(-1),
                Key::Down => Some(1),
                Key::PageUp => Some(-(page as isize)),
                Key::PageDown => Some(page as isize),
                _ => None,
            };
            if let Some(d) = rows {
                self.move_visual(d, shift, ctx);
                if matches!(key, Key::PageUp | Key::PageDown) {
                    self.scroll.y += d as f32 * lh;
                }
                return true;
            }
        }
        if let (Key::Char('z'), true, false) = (key, mods.alt, mods.ctrl) {
            self.wrap = !self.wrap;
            self.scroll.x = 0.0;
            self.reveal_caret = true;
            return true;
        }
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
            (Key::Char('c'), true, false) | (Key::Insert, true, false) => {
                let (t, line_mode) = self.doc.copy_text();
                self.line_clip = line_mode.then(|| t.clone());
                ctx.clipboard.set(t);
            }
            (Key::Char('x'), true, false) | (Key::Delete, false, false)
                if !ro && (mods.ctrl || shift) =>
            {
                let (t, line_mode) = self.doc.copy_text();
                self.line_clip = line_mode.then(|| t.clone());
                ctx.clipboard.set(t);
                if line_mode {
                    self.doc.delete_lines();
                } else {
                    self.doc.delete_selection();
                }
                self.after_edit(edit_from);
            }
            (Key::Char('v'), true, false) | (Key::Insert, false, false)
                if !ro && (mods.ctrl || shift) =>
            {
                if let Some(t) = ctx.clipboard.get().filter(|t| !t.is_empty()) {
                    let all_carets = self.doc.sels.iter().all(|s| s.is_empty());
                    if all_carets && self.line_clip.as_deref() == Some(t.as_str()) {
                        self.doc.paste_lines(&t);
                    } else {
                        self.doc.insert_text(&t);
                    }
                    self.after_edit(edit_from);
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
        if self.wrap {
            v.push("Wrap".into());
        }
        if self.read_only {
            v.push("read-only".into());
        }
        v
    }
}

impl CodeView {
    fn bar_row_height(ctx: &Ctx) -> i32 {
        (ctx.ui_metrics().line_height + ctx.px(theme::PADDING_Y) * 2.0) as i32 + ctx.px(4.0) as i32
    }

    /// Split the view into the editor area and the find bar (if open).
    fn split(&self, rect: Rect, ctx: &Ctx) -> (Rect, Option<Rect>) {
        let Some(f) = &self.find else {
            return (rect, None);
        };
        let rows = if f.replace.is_some() { 2 } else { 1 };
        let h = (Self::bar_row_height(ctx) * rows).min(rect.h);
        (
            Rect::new(rect.x, rect.y, rect.w, rect.h - h),
            Some(Rect::new(rect.x, rect.bottom() - h, rect.w, h)),
        )
    }

    /// Open the find bar (Ctrl+F / Ctrl+H), prefilled from the selection.
    pub fn open_find(&mut self, with_replace: bool) {
        let sel = self.doc.selected_text();
        let prefill = if !sel.is_empty() && !sel.contains('\n') {
            sel
        } else {
            self.find
                .as_ref()
                .or(self.last_find.as_ref())
                .map(|f| f.pattern().to_string())
                .unwrap_or_default()
        };
        let mut f = Finder::new(&prefill, with_replace);
        if let Some(old) = self.find.take().or_else(|| self.last_find.take()) {
            f.case_sensitive = old.case_sensitive;
            f.regex = old.regex;
            if let (Some(r), Some(old_r)) = (f.replace.as_mut(), old.replace.as_ref()) {
                r.set_text(old_r.text());
            }
        }
        self.find_origin = self
            .doc
            .sels
            .iter()
            .map(|s| s.range().0)
            .min()
            .unwrap_or_default();
        self.find = Some(f);
        self.find_focus = true;
        self.find_note = None;
    }

    fn close_find(&mut self) {
        self.last_find = self.find.take();
        self.find_focus = false;
    }

    /// Select the next (or previous) match from the caret.
    pub fn find_step(&mut self, back: bool) -> bool {
        let finder = match self.find.as_mut().or(self.last_find.as_mut()) {
            Some(f) => f,
            None => return false,
        };
        let (a, b) = self.doc.primary().range();
        let from = if back { a } else { b };
        if let Some((m0, m1)) = finder.next(&self.doc.buf, from, back) {
            self.doc.sels = vec![crate::editor::history::Sel {
                anchor: m0,
                head: m1,
            }];
            self.reveal_caret = true;
            return true;
        }
        false
    }

    /// Incremental search: select the first match at/after the origin.
    fn find_incremental(&mut self) {
        let origin = self.find_origin;
        let Some(f) = self.find.as_mut() else { return };
        // Search from just before the origin so a match at the origin counts.
        if let Some((m0, m1)) = f.next(&self.doc.buf, origin, false) {
            self.doc.sels = vec![crate::editor::history::Sel {
                anchor: m0,
                head: m1,
            }];
            self.reveal_caret = true;
        }
    }

    /// Replace the current match (if the selection is one) and go to the next.
    fn replace_one(&mut self) {
        let Some(f) = self.find.as_mut() else { return };
        let (a, b) = self.doc.primary().range();
        let is_match = a.line == b.line
            && f.line_matches(self.doc.buf.line(a.line))
                .contains(&(a.col, b.col));
        if is_match {
            let line = self.doc.buf.line(a.line).to_string();
            if let Some(r) = f.replacement_for(&line, a.col) {
                self.doc.replace_ranges(vec![(a, b, r)]);
                self.after_edit(a.line.saturating_sub(1));
            }
        }
        self.find_step(false);
    }

    /// Replace every match as one undo step.
    fn replace_all(&mut self) {
        let Some(f) = self.find.as_mut() else { return };
        let all = f.all(&self.doc.buf);
        let mut edits = Vec::with_capacity(all.len());
        for (a, b) in all {
            let line = self.doc.buf.line(a.line).to_string();
            if let Some(r) = f.replacement_for(&line, a.col) {
                edits.push((a, b, r));
            }
        }
        let n = edits.len();
        if n > 0 {
            self.doc.replace_ranges(edits);
            self.after_edit(0);
        }
        self.find_note = Some(format!("replaced {n}"));
    }

    /// Route input to the find bar when it has focus. Returns Some(redraw)
    /// if the input was consumed.
    fn handle_find(
        &mut self,
        input: &Input,
        edit: Rect,
        bar: Option<Rect>,
        ctx: &mut Ctx,
    ) -> Option<bool> {
        // Shortcuts that work whether or not the bar has focus.
        if let Input::Key { key, mods } = input {
            match (key, mods.ctrl, mods.shift, mods.alt) {
                (Key::Char('f'), true, false, false) => {
                    self.open_find(false);
                    return Some(true);
                }
                (Key::Char('h'), true, false, false) if !self.read_only => {
                    self.open_find(true);
                    return Some(true);
                }
                (Key::F(3), false, back, false) => {
                    self.find_step(back);
                    return Some(true);
                }
                (Key::Char('c'), false, false, true) if self.find.is_some() => {
                    if let Some(f) = self.find.as_mut() {
                        f.case_sensitive = !f.case_sensitive;
                    }
                    self.find_incremental();
                    return Some(true);
                }
                (Key::Char('r'), false, false, true) if self.find.is_some() => {
                    if let Some(f) = self.find.as_mut() {
                        f.regex = !f.regex;
                    }
                    self.find_incremental();
                    return Some(true);
                }
                _ => {}
            }
        }
        // Clicks: on the bar focus it, in the editor focus the editor.
        if let (Input::MouseDown { y, .. }, Some(b)) = (input, bar) {
            let on_bar = (*y as i32) >= b.y;
            self.find_focus = on_bar;
            if on_bar {
                if let Some(f) = self.find.as_mut()
                    && f.replace.is_some()
                {
                    f.on_replace = (*y as i32) >= b.y + Self::bar_row_height(ctx);
                }
                return Some(true);
            }
            let _ = edit;
            return None;
        }
        if !self.find_focus
            || self.find.is_none()
            || !matches!(input, Input::Key { .. } | Input::Text(_))
        {
            return None;
        }
        // Tab switches between the find and replace fields.
        if let Input::Key { key: Key::Tab, .. } = input {
            if let Some(f) = self.find.as_mut()
                && f.replace.is_some()
            {
                f.on_replace = !f.on_replace;
            }
            return Some(true);
        }
        let ctrl_enter = matches!(input, Input::Key { key: Key::Enter, mods } if mods.ctrl);
        let f = self.find.as_mut()?;
        let on_replace = f.on_replace && f.replace.is_some();
        let ev = if on_replace {
            f.replace
                .as_mut()
                .map_or(BarEvent::Ignored, |r| r.handle(input, ctx))
        } else {
            f.find.handle(input, ctx)
        };
        match ev {
            BarEvent::Changed if !on_replace => self.find_incremental(),
            BarEvent::Submit { .. } if ctrl_enter && !self.read_only => self.replace_all(),
            BarEvent::Submit { .. } if on_replace && !self.read_only => self.replace_one(),
            BarEvent::Submit { back } => {
                self.find_step(back);
            }
            BarEvent::Cancel => self.close_find(),
            BarEvent::Ignored => return None,
            _ => {}
        }
        Some(true)
    }

    fn render_find(&mut self, canvas: &mut Canvas, bar: Rect, ctx: &mut Ctx) {
        let row_h = Self::bar_row_height(ctx);
        canvas.fill_rect(bar, theme::p().background2);
        canvas.fill_rect(Rect::new(bar.x, bar.y, bar.w, 1), theme::p().divider);
        let focused = self.find_focus && ctx.focused;
        let buf = &self.doc.buf;
        let version = self.doc.version();
        let Some(f) = self.find.as_mut() else { return };
        // Info on the right: count / error (cached: counting scans the whole
        // document), then toggles.
        let key = (version, f.pattern().to_string(), f.case_sensitive, f.regex);
        let info = match &self.find_count {
            Some((k, v)) if *k == key => v.clone(),
            // Only the text changed (typing with the bar open): show the old
            // count and recount after the frame (in `tick`), so keystrokes
            // never wait for a whole-document scan.
            Some((k, v)) if (&k.1, k.2, k.3) == (&key.1, key.2, key.3) => {
                self.find_count_stale = true;
                if v.ends_with('…') {
                    v.clone()
                } else {
                    format!("{v} …")
                }
            }
            _ => {
                let v = match f.error() {
                    Some(e) => e,
                    None if f.pattern().is_empty() => String::new(),
                    None => match f.count(buf) {
                        (0, _) => "no matches".into(),
                        (n, false) => format!("{n} {}", if n == 1 { "match" } else { "matches" }),
                        (n, true) => format!("{n}+ matches"),
                    },
                };
                self.find_count = Some((key, v.clone()));
                v
            }
        };
        let info = match &self.find_note {
            Some(n) => format!("{n} · {info}"),
            None => info,
        };
        let pad = ctx.px(theme::PADDING_X) as i32;
        let lh = ctx.ui_metrics().line_height as i32;
        let toggles = format!(
            "{}  {}",
            if f.case_sensitive { "[Aa]" } else { " Aa " },
            if f.regex { "[.*]" } else { " .* " }
        );
        let tw = ctx.text_width(&toggles, FontKind::Mono) as i32;
        let iw = ctx.text_width(&info, FontKind::Ui) as i32;
        let right = tw + iw + pad * 3;
        let row1 = Rect::new(bar.x, bar.y, bar.w, row_h);
        let field1 = Rect::new(
            row1.x,
            row1.y,
            (row1.w - right).max(ctx.px(120.0) as i32),
            row1.h,
        );
        f.find.render(canvas, field1, ctx, focused && !f.on_replace);
        let ty = row1.y + (row1.h - lh) / 2;
        let err = info.starts_with("invalid pattern") || info == "no matches";
        ctx.label(
            canvas,
            &info,
            field1.right() + pad,
            ty,
            if err {
                theme::p().error
            } else {
                theme::p().text
            },
            iw + 2,
            FontKind::Ui,
        );
        ctx.label(
            canvas,
            &toggles,
            row1.right() - tw - pad,
            ty,
            theme::p().accent,
            tw + 2,
            FontKind::Mono,
        );
        if let Some(r) = f.replace.as_mut() {
            let row2 = Rect::new(
                bar.x,
                bar.y + row_h,
                (bar.w - right).max(ctx.px(120.0) as i32),
                row_h,
            );
            r.render(canvas, row2, ctx, focused && f.on_replace);
            let hint = "Enter: replace · Ctrl+Enter: all";
            let hw = ctx.text_width(hint, FontKind::Ui) as i32;
            ctx.label(
                canvas,
                hint,
                row2.right() + pad,
                row2.y + (row2.h - lh) / 2,
                theme::p().dim,
                hw + 2,
                FontKind::Ui,
            );
        }
    }

    fn render_editor(&mut self, canvas: &mut Canvas, rect: Rect, ctx: &mut Ctx) {
        canvas.fill_rect(rect, theme::p().background);
        self.last_rect = rect;
        self.view_w = rect.w as f32;
        self.view_h = rect.h as f32;
        self.update_rows(ctx);
        if let Some(l) = self.goto_center.take() {
            let lh = ctx.code_metrics().line_height;
            self.scroll.y = (self.rows.first_row(l) as f32 * lh - rect.h as f32 / 3.0).max(0.0);
        }
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
        let first_row = ((self.scroll.y - pad_y) / lh).floor().max(0.0) as usize;
        let last_row = (((self.scroll.y - pad_y + rect.h as f32) / lh)
            .ceil()
            .max(0.0) as usize)
            .min(self.rows.total());
        let first = self.rows.line_of_row(first_row).0;
        let last = if last_row == 0 {
            0
        } else {
            (self.rows.line_of_row(last_row - 1).0 + 1).min(line_count)
        };
        self.visible = (first, last);
        if self.cache.len() > 4 * (last - first + 1) + 64 {
            let keep = first.saturating_sub(64)..last + 64;
            self.cache.retain(|k, _| keep.contains(k));
        }

        self.caret_screen = None;
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
        let mut br: Vec<usize> = Vec::new();
        let mut br_line = usize::MAX;

        for r in first_row..last_row {
            let (i, k) = self.rows.line_of_row(r);
            if br_line != i {
                br = self.breaks(i);
                br_line = i;
            }
            let k = k.min(br.len() - 1);
            let line_len = self.doc.buf.line(i).len();
            let (row_start, row_end) = (br[k], br.get(k + 1).copied().unwrap_or(line_len));
            let last_row_of_line = k + 1 == br.len();
            let row_x0 = self.col_to_x(i, row_start, ctx);
            let row_x1 = self.col_to_x(i, row_end, ctx);
            let y = (top + r as f32 * lh).round() as i32;
            let row = Rect::new(rect.x, y, rect.w, lh as i32);
            let is_caret_line = caret_lines.contains(&i);
            if is_caret_line && !any_selection && ctx.focused {
                canvas.fill_rect(row.intersect(&outer), theme::p().line_highlight);
            }
            // Line number on the first row of each line, right-aligned.
            if k == 0 {
                canvas.set_clip(gutter_rect.intersect(&outer));
                let label = (i + 1).to_string();
                let lw = (label.len() as f32 * char_w) as i32;
                let lx = rect.x + gutter_w as i32 - pad_x as i32 - lw;
                let num_color = if is_caret_line {
                    theme::p().line_number2
                } else {
                    theme::p().line_number
                };
                ctx.label(canvas, &label, lx, y, num_color, lw + 2, FontKind::Mono);
            }
            // Clip to this row's own text so other rows of the line don't show.
            let band = if last_row_of_line {
                text_rect
            } else {
                Rect::new(
                    text_x as i32,
                    y,
                    (row_x1 - row_x0).ceil() as i32 + 1,
                    lh as i32,
                )
            };
            canvas.set_clip(band.intersect(&text_rect).intersect(&outer));
            let seg = |a: usize, b: usize| (a.max(row_start), b.min(row_end));
            // Find matches.
            if let Some(f) = self.find.as_mut() {
                let ms = f.line_matches(self.doc.buf.line(i));
                for (a, b) in ms {
                    let (a, b) = seg(a, b);
                    if a >= b && !(a == b && a == row_end) {
                        continue;
                    }
                    let (x0, x1) = (
                        self.col_to_x(i, a, ctx) - row_x0,
                        self.col_to_x(i, b, ctx) - row_x0,
                    );
                    canvas.fill_rect(
                        Rect::new(
                            (ox + x0).round() as i32,
                            y,
                            (x1 - x0).round().max(2.0) as i32,
                            lh as i32,
                        ),
                        theme::p().find_match,
                    );
                }
            }
            // Selections.
            canvas.set_clip(text_rect.intersect(&outer));
            for s in &sels {
                let (a, b) = s.range();
                if a == b || i < a.line || i > b.line {
                    continue;
                }
                let sa = if i == a.line { a.col } else { 0 };
                let sb = if i == b.line { b.col } else { line_len };
                let (ca, cb) = seg(sa, sb);
                if ca > cb || (ca == cb && !(i < b.line && last_row_of_line)) {
                    continue;
                }
                let x0 = self.col_to_x(i, ca, ctx) - row_x0;
                let mut x1 = self.col_to_x(i, cb, ctx) - row_x0;
                if i < b.line && last_row_of_line {
                    x1 += char_w * 0.6; // the line break is selected too
                }
                canvas.fill_rect(
                    Rect::new(
                        (ox + x0).round() as i32,
                        y,
                        (x1 - x0).round().max(1.0) as i32,
                        lh as i32,
                    ),
                    theme::p().selection,
                );
            }
            // Text (the whole line, shifted so this row's start is at the margin).
            canvas.set_clip(band.intersect(&text_rect).intersect(&outer));
            let x = (ox - row_x0).round() as i32;
            self.line(i, ctx);
            if let Some(c) = self.cache.get_mut(&i) {
                ctx.fonts
                    .draw_buffer(canvas, &mut c.buffer, x, y, theme::p().syntax_normal);
            }
            // Carets.
            canvas.set_clip(text_rect.intersect(&outer));
            if ctx.focused {
                let cw = ctx.px(2.0).max(1.0) as i32;
                for (n, s) in sels.iter().enumerate() {
                    if s.head.line == i && row_of(&br, s.head.col) == k {
                        let cx = (ox + self.col_to_x(i, s.head.col, ctx) - row_x0).round() as i32;
                        let rr = Rect::new(cx, y, cw, lh as i32);
                        canvas.fill_rect(rr, theme::p().caret);
                        if n == 0 {
                            self.caret_screen = Some(rr);
                        }
                    }
                }
            }
        }
        canvas.set_clip(outer);

        let (_, content_h) = self.content_size(ctx);
        draw_vscrollbar(canvas, rect, self.scroll.y, content_h, ctx);
    }

    fn handle_editor(&mut self, input: &Input, rect: Rect, ctx: &mut Ctx) -> bool {
        // Know our size even before the first frame (wrap mapping needs it).
        self.last_rect = rect;
        self.view_w = rect.w as f32;
        self.view_h = rect.h as f32;
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
}

impl View for CodeView {
    fn render(&mut self, canvas: &mut Canvas, rect: Rect, ctx: &mut Ctx) {
        let (edit, bar) = self.split(rect, ctx);
        self.render_editor(canvas, edit, ctx);
        if let Some(bar) = bar {
            canvas.set_clip(bar);
            self.render_find(canvas, bar, ctx);
        }
    }

    fn handle(&mut self, input: &Input, rect: Rect, ctx: &mut Ctx) -> bool {
        let (edit, bar) = self.split(rect, ctx);
        if let Some(r) = self.handle_find(input, edit, bar, ctx) {
            return r;
        }
        self.handle_editor(input, edit, ctx)
    }

    fn tick(&mut self, _ctx: &mut Ctx, budget: Duration) -> Tick {
        if std::mem::take(&mut self.find_count_stale) {
            // Recount now that the frame is out; next render shows it.
            self.find_count = None;
            return Tick {
                more: true,
                redraw: true,
            };
        }
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

    fn theme_changed(&mut self) {
        self.highlighter.retheme();
        self.metrics_changed();
    }

    fn is_dirty(&self) -> bool {
        self.doc.is_dirty()
    }

    fn text(&self) -> Option<String> {
        Some(self.doc.buf.to_text())
    }

    fn save(&mut self, allow_lossy: bool) -> Result<usize, SaveError> {
        self.doc.save(allow_lossy)
    }

    fn save_as(&mut self, path: &Path) -> Result<usize, SaveError> {
        let n = self.doc.save_as(path)?;
        let first = self.doc.buf.line(0).to_string();
        self.highlighter = LineHighlighter::new(find_syntax(path, &first));
        self.cache.clear();
        Ok(n)
    }

    fn disk_state(&self) -> DiskState {
        match Document::open(self.doc.path()) {
            Ok(d) if d.text == self.doc.buf.to_text() => DiskState::Same,
            Ok(_) => DiskState::Differs,
            Err(_) if !self.doc.path().exists() => DiskState::Missing,
            Err(_) => DiskState::Same,
        }
    }

    fn reload_discarding(&mut self, _ctx: &mut Ctx) -> bool {
        match Document::open(self.doc.path()) {
            Ok(d) => {
                self.doc.replace_all_from_disk(d);
                self.after_edit(0);
                true
            }
            Err(_) => false,
        }
    }

    fn mark_orphaned(&mut self) {
        self.doc.force_dirty();
    }

    fn caret_rect(&self) -> Option<Rect> {
        self.caret_screen
    }

    fn line_count(&self) -> Option<usize> {
        Some(self.doc.buf.line_count())
    }

    fn goto_line(&mut self, line: usize) -> bool {
        let l = line.saturating_sub(1).min(self.doc.buf.line_count() - 1);
        let first = self.doc.buf.line(l).len() - self.doc.buf.line(l).trim_start().len();
        self.doc.set_caret(Pos::new(l, first), false);
        // Put the line about a third of the way down.
        self.goto_center = Some(l);
        self.reveal_caret = true;
        true
    }

    #[cfg(test)]
    fn snapshot(&self) -> Option<(String, Vec<Pos>)> {
        let pts = self
            .doc
            .sels
            .iter()
            .flat_map(|s| [s.anchor, s.head])
            .collect();
        Some((self.doc.buf.to_text(), pts))
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

    /// Typing on one line must not make the lines below lose their colours
    /// for a frame while highlighting catches up.
    #[test]
    fn edits_do_not_flash_uncoloured_lines() {
        use crate::canvas::Canvas;
        use crate::input::Request;
        use crate::text::Fonts;
        let dir = std::env::temp_dir().join(format!("litedit-flash-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("f.rs");
        let src: String = (0..50)
            .map(|i| format!("let s{i} = \"str\"; // c\n"))
            .collect();
        std::fs::write(&p, &src).unwrap();
        let mut fonts = Fonts::load();
        let (mut reqs, mut cur, mut clip) = (
            Vec::<Request>::new(),
            Cursor::Default,
            crate::clipboard::MemClipboard::default(),
        );
        let mut v = CodeView::new(Document::open(&p).unwrap());
        let mut buf = vec![0u32; 800 * 600];
        macro_rules! ctx {
            () => {
                Ctx {
                    fonts: &mut fonts,
                    scale: 1.0,
                    zoom: 1.0,
                    requests: &mut reqs,
                    cursor: &mut cur,
                    focused: true,
                    clipboard: &mut clip,
                }
            };
        }
        while v.tick(&mut ctx!(), Duration::from_millis(50)).more {}
        let rect = Rect::new(0, 0, 800, 600);
        v.render(&mut Canvas::new(&mut buf, 800, 600), rect, &mut ctx!());
        assert!(v.cache[&10].spans.as_ref().is_some_and(|s| !s.is_empty()));
        v.handle(&Input::Text("x".into()), rect, &mut ctx!());
        v.render(&mut Canvas::new(&mut buf, 800, 600), rect, &mut ctx!()); // no tick yet
        assert!(
            v.cache[&10].spans.as_ref().is_some_and(|s| !s.is_empty()),
            "line 10 lost its colours"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn styled_pieces_follow_spans_across_tabs() {
        let span = Span {
            start: 1,
            end: 3,
            color: theme::p().syntax_string,
            bold: false,
            italic: false,
        };
        let p = styled_pieces("x\tyz", &[span], 100);
        assert_eq!(p[0].0, "x");
        assert_eq!(p[1].0, "   y");
        assert_eq!(p[2].0, "z");
    }
}
