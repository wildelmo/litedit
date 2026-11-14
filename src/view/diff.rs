//! Side-by-side compare (VS Code's diff editor, read-only): old file on the
//! left, new on the right, aligned row by row. Removed lines are tinted red,
//! added lines green, and inside a changed line the changed characters are
//! highlighted more strongly. One scroll moves both sides. F7 / Shift+F7
//! (or Alt+F5 / Shift+Alt+F5) jump to the next / previous change.
//!
//! The diff is recomputed when either file changes on disk.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use cosmic_text::Buffer;

use crate::canvas::{Canvas, Color, Rect};
use crate::diff::{self, Kind, Model};
use crate::doc::Document;
use crate::input::{Cursor, Input, Key};
use crate::syntax::{LineHighlighter, find_syntax};
use crate::theme;
use crate::ui::{Ctx, FontKind};
use crate::view::code::{DisplayLine, MAX_DISPLAY_CHARS, shape_code_line};
use crate::view::{Scroll, Tick, View, draw_vscrollbar};

/// Time allowed for the line diff before it settles for a coarser answer.
const DIFF_BUDGET: Duration = Duration::from_millis(1500);

/// One side of the comparison.
pub struct Side {
    /// Shown above the side, e.g. "notes.md" or "notes.md (unsaved)".
    pub label: String,
    /// File to reload when it changes on disk (None for a snapshot).
    pub path: Option<PathBuf>,
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
        let mut side = Self::from_text(&doc.text, label, path);
        side.path = Some(path.to_path_buf());
        Ok(side)
    }

    /// Text that isn't (only) on disk, e.g. an editor's unsaved buffer.
    /// `syntax_path` picks the colouring; the side is never reloaded.
    pub fn from_text(text: &str, label: String, syntax_path: &Path) -> Self {
        let lines = diff::lines(text);
        let first = lines.first().map(String::as_str).unwrap_or("");
        Self {
            highlighter: LineHighlighter::new(find_syntax(syntax_path, first)),
            label,
            path: None,
            lines,
        }
    }
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
}

impl DiffView {
    pub fn new(old: Side, new: Side) -> Result<Self, String> {
        if old.lines.len() > diff::MAX_LINES || new.lines.len() > diff::MAX_LINES {
            return Err(format!(
                "Files with more than {} lines are not compared",
                diff::MAX_LINES
            ));
        }
        let model = diff::compute(&old.lines, &new.lines, DIFF_BUDGET);
        Ok(Self {
            old,
            new,
            model,
            scroll: Scroll::default(),
            cache: [HashMap::new(), HashMap::new()],
            current: None,
            last_rect: Rect::new(0, 0, 0, 0),
            error: None,
        })
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
        let mut changed = false;
        for side in [&mut self.old, &mut self.new] {
            if let Some(p) = side.path.clone() {
                match Side::from_file(&p, side.label.clone()) {
                    Ok(s) => {
                        if s.lines != side.lines {
                            *side = s;
                            changed = true;
                        }
                    }
                    Err(e) => {
                        self.error = Some(e);
                        return true;
                    }
                }
            }
        }
        if changed {
            self.error = None;
            self.model = diff::compute(&self.old.lines, &self.new.lines, DIFF_BUDGET);
            self.cache = [HashMap::new(), HashMap::new()];
            self.current = None;
        }
        changed
    }

    fn side(&self, s: usize) -> &Side {
        if s == 0 { &self.old } else { &self.new }
    }

    /// Changed ranges for a line on side `s`, given its row.
    fn marks(&self, s: usize, row: &diff::Row) -> Vec<(usize, usize)> {
        if row.kind != Kind::Changed {
            return Vec::new();
        }
        let (Some(o), Some(n)) = (row.old, row.new) else {
            return Vec::new();
        };
        let (a, b) = diff::inline_changes(&self.old.lines[o], &self.new.lines[n]);
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
                let src = &self.side(s).lines[li];
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
            let msg = "The two files are identical";
            let w = ctx.text_width(msg, FontKind::Ui) as i32;
            ctx.label(
                canvas,
                msg,
                rect.x + (rect.w - w) / 2,
                body_top + ctx.px(8.0) as i32,
                pal.dim,
                w + 4,
                FontKind::Ui,
            );
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
        let page = (rect.h as f32 - Self::header_h(ctx) - rh).max(rh);
        let before = (self.scroll.x, self.scroll.y);
        match input {
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
        let a = self.old.highlighter.step(&self.old.lines, half);
        let b = self.new.highlighter.step(&self.new.lines, half);
        Tick {
            more: a || b,
            redraw: true,
        }
    }

    fn title(&self) -> String {
        format!("{} ↔ {}", self.old.label, self.new.label)
    }

    fn status(&self) -> Vec<String> {
        let mut v = vec![match self.change_count() {
            0 => "No changes".to_string(),
            1 => "1 change".to_string(),
            n => format!("{n} changes"),
        }];
        if let Some(k) = self.current {
            v.push(format!("Change {} of {}", k + 1, self.change_count()));
        }
        if self.model.simplified {
            v.push("Simplified (files very different)".into());
        }
        v.push("F7 / Shift+F7: next / previous change".into());
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

    fn metrics_changed(&mut self) {
        self.cache = [HashMap::new(), HashMap::new()];
    }

    fn theme_changed(&mut self) {
        self.old.highlighter.retheme();
        self.new.highlighter.retheme();
        self.metrics_changed();
    }
}
