//! Side-by-side compare (VS Code's diff editor, read-only): old file on the
//! left, new on the right, aligned row by row. Removed lines are tinted red,
//! added lines green, and inside a changed line the changed characters are
//! highlighted more strongly. One scroll moves both sides. F7 / Shift+F7
//! (or Alt+F5 / Shift+Alt+F5) jump to the next / previous change.
//!
//! Alt+W ignores whitespace: lines that differ only in indentation, trailing
//! blanks or runs of spaces / tabs count as unchanged (the text shown stays
//! the original). Double-click a row, or press Enter on the current change
//! (Shift+Enter for the left side), to open that file at that line.
//!
//! The diff is recomputed when either file changes on disk.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use cosmic_text::Buffer;

use crate::canvas::{Canvas, Color, Rect};
use crate::diff::{self, Kind, Model};
use crate::doc::Document;
use crate::input::{Cursor, Input, Key, MouseButton, Request};
use crate::syntax::{LineHighlighter, find_syntax};
use crate::theme;
use crate::ui::{Ctx, FontKind};
use crate::view::code::{
    DisplayLine, HIGHLIGHT_MIN_AHEAD, HIGHLIGHT_SCREENS_AHEAD, MAX_DISPLAY_CHARS, shape_code_line,
};
use crate::view::{Scroll, Tick, View, draw_vscrollbar};

/// Time allowed for the line diff before it settles for a coarser answer.
const DIFF_BUDGET: Duration = Duration::from_millis(1500);

/// One side of the comparison.
pub struct Side {
    /// Shown above the side, e.g. "notes.md" or "notes.md (unsaved)".
    pub label: String,
    /// File to reload when it changes on disk (None for a snapshot).
    pub path: Option<PathBuf>,
    /// The file a snapshot was taken from (an editor's unsaved text):
    /// where "open this line" goes. Never read or reloaded.
    pub origin: Option<PathBuf>,
    pub lines: Vec<String>,
    highlighter: LineHighlighter,
}

impl Side {
    /// Read a file from disk. Errors are plain messages for the user.
    pub fn from_file(path: &Path, label: String) -> Result<Self, String> {
        let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
        if meta.len() as usize > diff::MAX_BYTES {
            return Err(format!(
                "{} is larger than {} MB",
                path.display(),
                diff::MAX_BYTES / (1024 * 1024)
            ));
        }
        let doc = Document::open(path).map_err(|e| e.to_string())?;
        // Counted before splitting, like a snapshot: a file may hold far
        // more lines than are compared.
        if crate::doc::count_line_breaks(&doc.text, diff::MAX_LINES) > diff::MAX_LINES {
            return Err(too_many_lines());
        }
        let mut side = Self::from_text(&doc.text, label, path);
        side.path = Some(path.to_path_buf());
        Ok(side)
    }

    /// Text that isn't (only) on disk, e.g. an editor's unsaved buffer.
    /// `syntax_path` picks the colouring; the side is never reloaded.
    fn from_text(text: &str, label: String, syntax_path: &Path) -> Self {
        let lines = diff::lines(text);
        let first = lines.first().map(String::as_str).unwrap_or("");
        Self {
            highlighter: LineHighlighter::new(find_syntax(syntax_path, first)),
            label,
            path: None,
            origin: None,
            lines,
        }
    }

    /// [`Self::from_text`] with the size limit of files, for text of any
    /// size (an unsaved buffer, the clipboard). `origin` is the file the
    /// text belongs to, if any.
    pub fn from_snapshot(
        text: &str,
        label: String,
        syntax_path: &Path,
        origin: Option<&Path>,
    ) -> Result<Self, String> {
        if text.len() > diff::MAX_BYTES {
            return Err(format!(
                "{label} is larger than {} MB",
                diff::MAX_BYTES / (1024 * 1024)
            ));
        }
        // Counted before splitting: a clipboard full of line breaks would
        // otherwise become millions of strings just to be refused.
        if crate::doc::count_line_breaks(text, diff::MAX_LINES) > diff::MAX_LINES {
            return Err(too_many_lines());
        }
        let mut side = Self::from_text(text, label, syntax_path);
        if side.lines.len() > diff::MAX_LINES {
            return Err(too_many_lines());
        }
        side.origin = origin.map(Path::to_path_buf);
        Ok(side)
    }

    /// The file "open this line" goes to.
    fn file(&self) -> Option<&PathBuf> {
        self.path.as_ref().or(self.origin.as_ref())
    }
}

fn too_many_lines() -> String {
    format!(
        "Files with more than {} lines are not compared",
        diff::MAX_LINES
    )
}

struct CachedRow {
    buffer: Buffer,
    dl: DisplayLine,
    /// Changed byte ranges (source offsets) to highlight.
    marks: Vec<(usize, usize)>,
    spans_drawn: bool,
}

pub struct DiffView {
    old: Side,
    new: Side,
    model: Model,
    scroll: Scroll,
    /// Shaped rows per side (0 = old, 1 = new), keyed by line index.
    cache: [HashMap<usize, CachedRow>; 2],
    /// Index into `model.hunks` of the change last jumped to.
    current: Option<usize>,
    last_rect: Rect,
    /// Set when a file is too large or unreadable on refresh.
    error: Option<String>,
    /// Alt+W: lines that differ only in whitespace count as unchanged.
    ignore_ws: bool,
    /// No changes while ignoring whitespace, but the texts are not equal.
    only_ws: bool,
    /// Row height at the last render / input (to keep the place on Alt+W).
    row_px: f32,
    /// Syntax-colour each side up to this line (exclusive): its last line
    /// on screen plus a margin, set by every frame (like the code view).
    want: [usize; 2],
}

impl DiffView {
    pub fn new(old: Side, new: Side) -> Result<Self, String> {
        if old.lines.len() > diff::MAX_LINES || new.lines.len() > diff::MAX_LINES {
            return Err(too_many_lines());
        }
        let mut v = Self {
            old,
            new,
            model: Model::default(),
            scroll: Scroll::default(),
            cache: [HashMap::new(), HashMap::new()],
            current: None,
            last_rect: Rect::new(0, 0, 0, 0),
            error: None,
            ignore_ws: false,
            only_ws: false,
            row_px: 0.0,
            want: [HIGHLIGHT_MIN_AHEAD; 2],
        };
        v.rebuild();
        Ok(v)
    }

    /// Diff the two sides as they are now, with the current options. Every
    /// (re)computation goes through here.
    fn rebuild(&mut self) {
        self.model = diff::compute(
            &self.old.lines,
            &self.new.lines,
            DIFF_BUDGET,
            self.ignore_ws,
        );
        self.only_ws = self.ignore_ws && self.model.identical() && self.old.lines != self.new.lines;
        // Cached rows hold the highlights of the previous diff.
        self.cache = [HashMap::new(), HashMap::new()];
        self.current = None;
    }

    /// Alt+W: switch "ignore whitespace", staying on the line at the top.
    fn toggle_ignore_ws(&mut self) {
        // (side, line, pixels scrolled into its row)
        let rh = self.row_px;
        let anchor = (rh > 0.0 && self.error.is_none())
            .then(|| {
                let top = (self.scroll.y / rh).floor().max(0.0) as usize;
                let row = self.model.rows.get(top)?;
                let at = row.new.map(|n| (1, n)).or(row.old.map(|o| (0, o)))?;
                Some((at, self.scroll.y - top as f32 * rh))
            })
            .flatten();
        self.ignore_ws = !self.ignore_ws;
        self.rebuild();
        if let Some(((s, line), within)) = anchor {
            let want = Some(line);
            let row = self
                .model
                .rows
                .iter()
                .position(|r| if s == 0 { r.old == want } else { r.new == want });
            if let Some(r) = row {
                self.scroll.y = r as f32 * rh + within;
            }
        }
    }

    /// The line of side `s` on row `r`. A filler row (nothing on that side)
    /// stands for the nearest line above it, or the first line.
    fn line_at(&self, s: usize, r: usize) -> usize {
        self.model
            .rows
            .get(..=r)
            .unwrap_or(&self.model.rows)
            .iter()
            .rev()
            .find_map(|row| if s == 0 { row.old } else { row.new })
            .unwrap_or(0)
    }

    /// Where "open this line" goes for row `r` of side `s`: (file, 1-based
    /// line). None when that side has no file (the clipboard).
    fn target(&self, s: usize, r: usize) -> Option<(PathBuf, usize)> {
        if self.error.is_some() || r >= self.model.rows.len() {
            return None;
        }
        let path = self.side(s).file()?.clone();
        // The saved copy beside its own unsaved text: the open tab shows the
        // unsaved text, so its line numbers are the ones that fit.
        let other = self.side(1 - s);
        let lines_of = if self.side(s).path.is_some()
            && other.path.is_none()
            && other.origin.as_ref() == Some(&path)
        {
            1 - s
        } else {
            s
        };
        Some((path, self.line_at(lines_of, r) + 1))
    }

    /// Ask the app to open row `r` of side `s` in a normal tab.
    fn open_row(&self, s: usize, r: usize, ctx: &mut Ctx) -> bool {
        match self.target(s, r) {
            Some((path, line)) => {
                ctx.requests.push(Request::OpenPathAt(path, line));
                true
            }
            None => false,
        }
    }

    pub fn change_count(&self) -> usize {
        self.model.hunks.len()
    }

    fn row_h(ctx: &Ctx) -> f32 {
        ctx.code_metrics().line_height
    }

    fn header_h(ctx: &Ctx) -> f32 {
        (ctx.ui_metrics().line_height + ctx.px(theme::PADDING_Y) * 2.0).round()
    }

    /// Scroll so change `k` sits a third of the way down.
    fn jump(&mut self, forward: bool, ctx: &Ctx) -> bool {
        let n = self.model.hunks.len();
        if n == 0 {
            return false;
        }
        let rh = Self::row_h(ctx);
        let top_row = (self.scroll.y / rh) as usize;
        let k = match (self.current, forward) {
            (Some(c), true) => (c + 1) % n,
            (Some(c), false) => (c + n - 1) % n,
            // First jump: relative to what is on screen.
            (None, true) => self
                .model
                .hunks
                .iter()
                .position(|&h| h > top_row)
                .unwrap_or(0),
            (None, false) => self
                .model
                .hunks
                .iter()
                .rposition(|&h| h < top_row)
                .unwrap_or(n - 1),
        };
        self.current = Some(k);
        let view_h = (self.last_rect.h as f32 - Self::header_h(ctx)).max(rh);
        self.scroll.y = (self.model.hunks[k] as f32 * rh - view_h / 3.0).max(0.0);
        true
    }

    fn clamp(&mut self, ctx: &Ctx) {
        let rh = Self::row_h(ctx);
        let view_h = self.last_rect.h as f32 - Self::header_h(ctx);
        let content = self.model.rows.len() as f32 * rh;
        self.scroll.clamp(0.0, content, 0.0, view_h);
        self.scroll.x = self.scroll.x.max(0.0);
    }

    /// Re-read the files from disk and diff again (keeps the scroll).
    fn recompute(&mut self) -> bool {
        // An earlier problem (file missing, too large) may be gone now.
        let had_error = self.error.take().is_some();
        // Read both sides before touching either: the rows on screen belong
        // to the stored lines, so a side is only replaced together with a
        // fresh comparison.
        let mut fresh: [Option<Side>; 2] = [None, None];
        for (i, side) in [&self.old, &self.new].into_iter().enumerate() {
            let Some(p) = side.path.clone() else { continue };
            match Side::from_file(&p, side.label.clone()) {
                Ok(s) if s.lines.len() > diff::MAX_LINES => {
                    self.error = Some(too_many_lines());
                    return true;
                }
                Ok(s) => {
                    if s.lines != side.lines {
                        fresh[i] = Some(s);
                    }
                }
                Err(e) => {
                    self.error = Some(e);
                    return true;
                }
            }
        }
        let [old, new] = fresh;
        let changed = old.is_some() || new.is_some();
        if let Some(s) = old {
            self.old = s;
        }
        if let Some(s) = new {
            self.new = s;
        }
        if changed {
            self.rebuild();
        }
        changed || had_error
    }

    fn side(&self, s: usize) -> &Side {
        if s == 0 { &self.old } else { &self.new }
    }

    /// Is syntax colouring still owed for what is on screen?
    fn highlight_pending(&self) -> bool {
        (0..2).any(|s| {
            let side = self.side(s);
            side.highlighter.needs(self.want[s], side.lines.len())
        })
    }

    /// Changed ranges for a line on side `s`, given its row.
    fn marks(&self, s: usize, row: &diff::Row) -> Vec<(usize, usize)> {
        if row.kind != Kind::Changed {
            return Vec::new();
        }
        let (Some(o), Some(n)) = (row.old, row.new) else {
            return Vec::new();
        };
        let (Some(old), Some(new)) = (self.old.lines.get(o), self.new.lines.get(n)) else {
            return Vec::new();
        };
        let (a, b) = if self.ignore_ws {
            diff::inline_changes_ignoring_ws(old, new)
        } else {
            diff::inline_changes(old, new)
        };
        if s == 0 { a } else { b }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_side(
        &mut self,
        canvas: &mut Canvas,
        s: usize,
        area: Rect,
        first: usize,
        last: usize,
        gutter_w: i32,
        ctx: &mut Ctx,
    ) {
        let rh = Self::row_h(ctx);
        let pal = *theme::p();
        let (tint, mark) = if s == 0 {
            (pal.diff_removed, pal.diff_removed_hl)
        } else {
            (pal.diff_added, pal.diff_added_hl)
        };
        let fam = ctx.family(FontKind::Mono);
        let text_x0 = area.x + gutter_w;
        let prev = canvas.clip();
        for r in first..last {
            let row = self.model.rows[r].clone();
            let y = (area.y as f32 + r as f32 * rh - self.scroll.y).round() as i32;
            let row_rect = Rect::new(area.x, y, area.w, rh.ceil() as i32);
            let line = if s == 0 { row.old } else { row.new };
            let Some(li) = line else {
                // Nothing on this side: a filler row keeps the sides aligned.
                canvas.fill_rect(row_rect, pal.diff_filler);
                continue;
            };
            let changed_here = match row.kind {
                Kind::Same => false,
                Kind::Removed => s == 0,
                Kind::Added => s == 1,
                Kind::Changed => true,
            };
            if changed_here {
                canvas.fill_rect(row_rect, tint);
            }
            // Line number.
            let num = (li + 1).to_string();
            let num_w = ctx.text_width(&num, FontKind::Mono) as i32;
            ctx.label(
                canvas,
                &num,
                text_x0 - num_w - ctx.px(10.0) as i32,
                y,
                if changed_here {
                    pal.line_number2
                } else {
                    pal.line_number
                },
                num_w + 2,
                FontKind::Mono,
            );
            // Text (cached per line; rebuilt when syntax colours arrive).
            let spans_ready = self.side(s).highlighter.spans(li).is_some();
            let stale = self.cache[s]
                .get(&li)
                .is_none_or(|c| spans_ready && !c.spans_drawn);
            if stale {
                // (Rows always belong to the stored lines; never index.)
                let Some(src) = self.side(s).lines.get(li) else {
                    continue;
                };
                let spans = self.side(s).highlighter.spans(li);
                let mut buffer = shape_code_line(src, spans, &fam, ctx.code_metrics());
                buffer.shape_until_scroll(&mut ctx.fonts.system, false);
                let marks = self.marks(s, &row);
                let dl = DisplayLine::new(src, MAX_DISPLAY_CHARS);
                self.cache[s].insert(
                    li,
                    CachedRow {
                        buffer,
                        dl,
                        marks,
                        spans_drawn: spans_ready,
                    },
                );
            }
            let text_area = Rect::new(text_x0, y, area.right() - text_x0, row_rect.h);
            canvas.set_clip(text_area.intersect(&prev));
            let x = text_x0 + ctx.px(4.0) as i32 - self.scroll.x as i32;
            if let Some(c) = self.cache[s].get_mut(&li) {
                for &(a, b) in &c.marks {
                    let (xa, xb) = (
                        x_of(&c.buffer, c.dl.to_disp(a)),
                        x_of(&c.buffer, c.dl.to_disp(b)),
                    );
                    let w = ((xb - xa).round() as i32).max(ctx.px(2.0) as i32);
                    canvas.fill_rect(Rect::new(x + xa.round() as i32, y, w, row_rect.h), mark);
                }
                ctx.fonts
                    .draw_buffer(canvas, &mut c.buffer, x, y, pal.syntax_normal);
            }
            canvas.set_clip(prev);
        }
    }
}

/// x of a display byte offset within a shaped single-line buffer.
fn x_of(b: &Buffer, disp_byte: usize) -> f32 {
    let Some(run) = b.layout_runs().next() else {
        return 0.0;
    };
    for g in run.glyphs {
        if g.start >= disp_byte {
            return g.x;
        }
        if g.end > disp_byte {
            return g.x;
        }
    }
    run.line_w
}

impl View for DiffView {
    fn render(&mut self, canvas: &mut Canvas, rect: Rect, ctx: &mut Ctx) {
        self.last_rect = rect;
        self.row_px = Self::row_h(ctx);
        self.clamp(ctx);
        let pal = *theme::p();
        canvas.fill_rect(rect, pal.background);
        let hh = Self::header_h(ctx) as i32;
        let half = rect.w / 2;
        let sides = [
            Rect::new(rect.x, rect.y, half, rect.h),
            Rect::new(rect.x + half + 1, rect.y, rect.w - half - 1, rect.h),
        ];
        // Headers.
        for (s, area) in sides.iter().enumerate() {
            let head = Rect::new(area.x, area.y, area.w, hh);
            canvas.fill_rect(head, pal.background2);
            let label = self.side(s).label.clone();
            let pad = ctx.px(theme::PADDING_X) as i32;
            let lh = ctx.ui_metrics().line_height as i32;
            ctx.label(
                canvas,
                &label,
                head.x + pad,
                head.y + (hh - lh) / 2,
                pal.accent,
                head.w - pad * 2,
                FontKind::Ui,
            );
        }
        canvas.fill_rect(Rect::new(rect.x + half, rect.y, 1, rect.h), pal.divider);
        let body_top = rect.y + hh;
        let body_h = rect.h - hh;
        if let Some(e) = &self.error {
            let e = e.clone();
            ctx.label(
                canvas,
                &e,
                rect.x + ctx.px(20.0) as i32,
                body_top + ctx.px(20.0) as i32,
                pal.error,
                rect.w - ctx.px(40.0) as i32,
                FontKind::Ui,
            );
            return;
        }
        if self.model.identical() {
            // In the right header, beside the name (not over the text).
            let msg = if self.only_ws {
                "The two files differ only in whitespace"
            } else {
                "The two files are identical"
            };
            let pad = ctx.px(theme::PADDING_X) as i32;
            let lh = ctx.ui_metrics().line_height as i32;
            let w = ctx.text_width(msg, FontKind::Ui) as i32 + 4;
            let name_w = ctx.text_width(&self.new.label, FontKind::Ui) as i32;
            let x = sides[1].right() - pad - w;
            if x >= sides[1].x + pad + name_w + pad {
                ctx.label(
                    canvas,
                    msg,
                    x,
                    rect.y + (hh - lh) / 2,
                    pal.dim,
                    w,
                    FontKind::Ui,
                );
            }
        }
        let rh = Self::row_h(ctx);
        let first = (self.scroll.y / rh).floor().max(0.0) as usize;
        let last =
            (((self.scroll.y + body_h as f32) / rh).ceil() as usize + 1).min(self.model.rows.len());
        let digits = self
            .old
            .lines
            .len()
            .max(self.new.lines.len())
            .to_string()
            .len()
            .max(3);
        let gutter_w =
            (ctx.text_width(&"9".repeat(digits), FontKind::Mono) + ctx.px(20.0)).round() as i32;
        // Colour each side as far as its last line on screen, plus a margin.
        let ahead = ((body_h.max(0) as f32 / rh.max(1.0)) as usize)
            .saturating_mul(HIGHLIGHT_SCREENS_AHEAD)
            .max(HIGHLIGHT_MIN_AHEAD);
        let shown = self.model.rows.get(first..last.max(first)).unwrap_or(&[]);
        for s in 0..2 {
            let bottom = shown
                .iter()
                .rev()
                .find_map(|r| if s == 0 { r.old } else { r.new });
            if let Some(l) = bottom {
                self.want[s] = l.saturating_add(1).saturating_add(ahead);
            }
        }
        let prev = canvas.clip();
        for (s, area) in sides.iter().enumerate() {
            let body = Rect::new(area.x, body_top, area.w, body_h);
            canvas.set_clip(body.intersect(&prev));
            self.draw_side(canvas, s, body, first, last.max(first), gutter_w, ctx);
            canvas.set_clip(prev);
        }
        // Change markers along the right edge (where the changes are).
        let total = self.model.rows.len().max(1) as f32;
        let ruler_w = ctx.px(4.0).max(2.0) as i32;
        let rx = rect.right() - ruler_w;
        for (i, row) in self.model.rows.iter().enumerate() {
            if row.kind == Kind::Same || (i > 0 && self.model.rows[i - 1].kind == row.kind) {
                continue;
            }
            let c: Color = match row.kind {
                Kind::Removed => pal.diff_removed_hl,
                Kind::Added => pal.diff_added_hl,
                _ => pal.find_match,
            };
            let y = body_top + (i as f32 / total * body_h as f32) as i32;
            canvas.fill_rect(Rect::new(rx, y, ruler_w, ctx.px(3.0).max(2.0) as i32), c);
        }
        let content_h = self.model.rows.len() as f32 * rh;
        draw_vscrollbar(
            canvas,
            Rect::new(rect.x, body_top, rect.w - ruler_w - 1, body_h),
            self.scroll.y,
            content_h,
            ctx,
        );
    }

    fn handle(&mut self, input: &Input, rect: Rect, ctx: &mut Ctx) -> bool {
        self.last_rect = rect;
        let rh = Self::row_h(ctx);
        self.row_px = rh;
        let page = (rect.h as f32 - Self::header_h(ctx) - rh).max(rh);
        let before = (self.scroll.x, self.scroll.y);
        match input {
            // Double-click a row: open that side's file at that line.
            Input::MouseDown {
                x,
                y,
                button: MouseButton::Left,
                clicks: 2,
            } => {
                let top = rect.y as f32 + Self::header_h(ctx);
                if *y < top || rh <= 0.0 {
                    return false;
                }
                let r = ((*y - top + self.scroll.y) / rh).floor().max(0.0) as usize;
                let s = usize::from(*x >= (rect.x + rect.w / 2) as f32);
                return self.open_row(s, r, ctx);
            }
            Input::Wheel { .. } => {
                self.scroll.wheel(input, rh);
            }
            Input::MouseMove { .. } => {
                *ctx.cursor = Cursor::Default;
                return false;
            }
            Input::Key { key, mods } => match key {
                Key::F(7) => return self.jump(!mods.shift, ctx),
                Key::F(5) if mods.alt => return self.jump(!mods.shift, ctx),
                Key::Char('w') if mods.alt && !mods.ctrl && !mods.shift => {
                    self.toggle_ignore_ws();
                    self.clamp(ctx);
                    return true;
                }
                // The current change in its file: the right side (the left
                // one with Shift, or when the right side has no file).
                Key::Enter if !mods.ctrl && !mods.alt => {
                    let Some(&r) = self.current.and_then(|k| self.model.hunks.get(k)) else {
                        return false;
                    };
                    return if mods.shift {
                        self.open_row(0, r, ctx)
                    } else {
                        self.open_row(1, r, ctx) || self.open_row(0, r, ctx)
                    };
                }
                Key::Down => self.scroll.y += rh,
                Key::Up => self.scroll.y -= rh,
                Key::PageDown | Key::Char(' ') => self.scroll.y += page,
                Key::PageUp => self.scroll.y -= page,
                Key::Home if mods.ctrl => self.scroll.y = 0.0,
                Key::End if mods.ctrl => self.scroll.y = f32::MAX,
                Key::Home => self.scroll.x = 0.0,
                Key::Right => self.scroll.x += rh * 2.0,
                Key::Left => self.scroll.x -= rh * 2.0,
                _ => return false,
            },
            _ => return false,
        }
        self.clamp(ctx);
        before != (self.scroll.x, self.scroll.y)
    }

    fn tick(&mut self, _ctx: &mut Ctx, budget: Duration) -> Tick {
        let half = budget / 2;
        let before = (self.old.highlighter.done(), self.new.highlighter.done());
        self.old
            .highlighter
            .step_to(&self.old.lines, self.want[0], half);
        self.new
            .highlighter
            .step_to(&self.new.lines, self.want[1], half);
        Tick {
            more: self.highlight_pending(),
            redraw: before != (self.old.highlighter.done(), self.new.highlighter.done()),
        }
    }

    fn has_background_work(&self) -> bool {
        self.highlight_pending()
    }

    fn on_hide(&mut self) {
        self.metrics_changed();
        self.old.highlighter.release_if_long();
        self.new.highlighter.release_if_long();
    }

    /// Lines coloured so far (the further side), and the new side's colours
    /// for `line` if that line was drawn with them.
    #[cfg(test)]
    fn highlight_probe(&self, line: usize) -> Option<(usize, Option<Vec<crate::syntax::Span>>)> {
        let done = self.old.highlighter.done().max(self.new.highlighter.done());
        let drawn = self.cache[1]
            .get(&line)
            .filter(|c| c.spans_drawn)
            .and_then(|_| self.new.highlighter.spans(line))
            .map(<[_]>::to_vec);
        Some((done, drawn))
    }

    fn title(&self) -> String {
        format!("{} ↔ {}", self.old.label, self.new.label)
    }

    fn status(&self) -> Vec<String> {
        let mut v = vec![match self.change_count() {
            0 if self.only_ws => "Only whitespace differs".to_string(),
            0 => "No changes".to_string(),
            1 => "1 change".to_string(),
            n => format!("{n} changes"),
        }];
        if let Some(k) = self.current {
            v.push(format!("Change {} of {}", k + 1, self.change_count()));
        }
        if self.ignore_ws {
            v.push("Ignoring whitespace (Alt+W)".into());
        }
        if self.model.simplified {
            v.push("Simplified (files very different)".into());
        }
        v.push("F7 / Shift+F7: next / previous change".into());
        if !self.ignore_ws {
            v.push("Alt+W: ignore whitespace".into());
        }
        if self.old.file().is_some() || self.new.file().is_some() {
            v.push("Enter / double-click: open file".into());
        }
        v
    }

    fn reload(&mut self, _ctx: &mut Ctx) -> bool {
        self.recompute()
    }

    fn watched_paths(&self) -> Vec<PathBuf> {
        [&self.old.path, &self.new.path]
            .into_iter()
            .flatten()
            .cloned()
            .collect()
    }

    fn run_command(&mut self, name: &str) -> bool {
        if name != "ignore-whitespace" {
            return false;
        }
        self.toggle_ignore_ws();
        true
    }

    fn keeps_its_tab(&self) -> bool {
        true
    }

    #[cfg(test)]
    fn scroll_y(&self) -> f32 {
        self.scroll.y
    }

    #[cfg(test)]
    fn has_caches(&self) -> bool {
        self.cache.iter().any(|c| !c.is_empty())
    }

    fn metrics_changed(&mut self) {
        self.cache = [HashMap::new(), HashMap::new()];
    }

    fn theme_changed(&mut self) {
        self.old.highlighter.retheme();
        self.new.highlighter.retheme();
        self.metrics_changed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(old: &str, new: &str) -> DiffView {
        let p = Path::new("x.txt");
        DiffView::new(
            Side::from_text(old, "old".into(), p),
            Side::from_text(new, "new".into(), p),
        )
        .unwrap()
    }

    /// (new line, pixels into the row) at the top of the view.
    fn top(v: &DiffView) -> (Option<usize>, f32) {
        let r = (v.scroll.y / v.row_px).floor() as usize;
        (v.model.rows[r].new, v.scroll.y - r as f32 * v.row_px)
    }

    #[test]
    fn toggling_ignore_whitespace_keeps_the_top_line_in_place() {
        // Lines 10..20 are re-indented *and* five lines were added before
        // them, so the rows above the top line change with the option.
        let old: String = (0..200).map(|i| format!("line {i}\n")).collect();
        let new: String = (0..200)
            .map(|i| match i {
                5 => "added a\nadded b\nadded c\nline 5\n".to_string(),
                10..20 => format!("\t line {i}  \n"),
                150 => "line one-fifty\n".to_string(),
                _ => format!("line {i}\n"),
            })
            .collect();
        let mut v = view(&old, &new);
        v.row_px = 20.0;
        let strict_rows = v.model.rows.len();
        assert_eq!(v.change_count(), 3);
        for start_row in [0usize, 9, 40, 120] {
            v.scroll.y = start_row as f32 * 20.0 + 7.0;
            let before = top(&v);
            v.toggle_ignore_ws();
            assert!(v.ignore_ws);
            assert_eq!(v.change_count(), 2);
            assert_eq!(top(&v), before, "from row {start_row}, option on");
            v.toggle_ignore_ws();
            assert_eq!(top(&v), before, "from row {start_row}, option off again");
            assert_eq!(v.model.rows.len(), strict_rows);
        }
        // Two changed rows become three (-p | a= | +q) when whitespace is
        // ignored, so every row below moves down by one: the line at the
        // top must move with it.
        let old: String = (0..200)
            .map(|i| match i {
                30 => "p\n".to_string(),
                31 => "  a\n".to_string(),
                _ => format!("line {i}\n"),
            })
            .collect();
        let new: String = (0..200)
            .map(|i| match i {
                30 => "a\n".to_string(),
                31 => "q\n".to_string(),
                _ => format!("line {i}\n"),
            })
            .collect();
        let mut v = view(&old, &new);
        v.row_px = 20.0;
        assert_eq!(v.model.rows.len(), 200);
        v.scroll.y = 40.0 * 20.0 + 3.0;
        assert_eq!(top(&v), (Some(40), 3.0));
        v.toggle_ignore_ws();
        assert_eq!(v.model.rows.len(), 201);
        assert_eq!(top(&v), (Some(40), 3.0));
        assert_eq!(v.scroll.y, 41.0 * 20.0 + 3.0);
        v.toggle_ignore_ws();
        assert_eq!(v.scroll.y, 40.0 * 20.0 + 3.0);
        // Never rendered (no row height yet): stays at the top, no panic.
        let mut v = view(&old, &new);
        v.toggle_ignore_ws();
        assert_eq!(v.scroll.y, 0.0);
    }

    #[test]
    fn toggle_drops_cached_rows_and_the_current_change() {
        let mut v = view("a\n  b\nc\n", "a\nb\nC\n");
        v.current = Some(1);
        assert_eq!(v.change_count(), 1, "b and c changed, side by side");
        assert!(!v.only_ws);
        v.toggle_ignore_ws();
        assert_eq!((v.change_count(), v.current), (1, None));
        let mut w = view("a\n  b\n", "a\r\nb\t\r\n");
        assert!(!w.model.identical() && !w.only_ws);
        w.toggle_ignore_ws();
        assert!(w.model.identical() && w.only_ws);
        assert_eq!(w.status()[0], "Only whitespace differs");
        // Truly identical text is not "only whitespace".
        let mut same = view("a\nb\n", "a\r\nb\r\n");
        same.toggle_ignore_ws();
        assert!(same.model.identical() && !same.only_ws);
        assert_eq!(same.status()[0], "No changes");
    }

    #[test]
    fn rows_map_to_the_right_file_and_line() {
        // Rows: a= | +X | +Y | b= | c= | -d | e=
        let mut v = view("a\nb\nc\nd\ne\n", "a\nX\nY\nb\nc\ne\n");
        assert_eq!(v.model.rows.len(), 7);
        // Neither side has a file: nothing to open.
        assert_eq!(v.target(0, 4), None);
        v.old.path = Some(PathBuf::from("old.txt"));
        v.new.origin = Some(PathBuf::from("new.txt"));
        let at = |v: &DiffView, s, r| v.target(s, r).map(|(p, l)| (p.display().to_string(), l));
        assert_eq!(at(&v, 0, 0), Some(("old.txt".into(), 1)));
        assert_eq!(at(&v, 0, 4), Some(("old.txt".into(), 3)));
        assert_eq!(at(&v, 1, 4), Some(("new.txt".into(), 5)));
        assert_eq!(at(&v, 0, 5), Some(("old.txt".into(), 4)));
        // Filler rows stand for the line above them.
        assert_eq!(at(&v, 0, 2), Some(("old.txt".into(), 1)));
        assert_eq!(at(&v, 1, 5), Some(("new.txt".into(), 5)));
        // Past the last row: nothing.
        assert_eq!(at(&v, 1, 7), None);
        assert_eq!(at(&v, 0, usize::MAX), None);
        // The saved copy beside its own unsaved text opens the tab that
        // shows the unsaved text, so it uses that side's line numbers.
        v.new.origin = Some(PathBuf::from("old.txt"));
        assert_eq!(at(&v, 0, 4), Some(("old.txt".into(), 5)));
        assert_eq!(at(&v, 1, 4), Some(("old.txt".into(), 5)));
    }

    #[test]
    fn snapshots_are_capped_like_files_and_never_watched() {
        let p = Path::new("x.rs");
        let big = "x".repeat(diff::MAX_BYTES + 1);
        let e = Side::from_snapshot(&big, "Clipboard".into(), p, None)
            .err()
            .unwrap();
        assert_eq!(e, "Clipboard is larger than 20 MB");
        let ok = Side::from_snapshot(&big[..diff::MAX_BYTES], "Clipboard".into(), p, Some(p));
        let side = ok.ok().unwrap();
        assert!(side.path.is_none() && side.origin.as_deref() == Some(p));
        let v = DiffView::new(side, Side::from_text("x", "t".into(), p)).unwrap();
        assert!(v.watched_paths().is_empty());
        let many = "\n".repeat(diff::MAX_LINES + 5);
        // Refused before the text is split into lines.
        assert!(Side::from_snapshot(&many, "Clipboard".into(), p, None).is_err());
        let most = "\n".repeat(diff::MAX_LINES - 1);
        assert!(Side::from_snapshot(&most, "Clipboard".into(), p, None).is_ok());
    }
}
