//! Shared drawing context handed to views.

use cosmic_text::{Attrs, Buffer, Family, Metrics, Shaping, Wrap};

use crate::canvas::{Canvas, Color, Rect};
use crate::input::{Cursor, Request};
use crate::text::Fonts;
use crate::theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontKind {
    Ui,
    Mono,
}

/// Everything a view needs from the app while rendering or handling input.
pub struct Ctx<'a> {
    pub fonts: &'a mut Fonts,
    pub scale: f32,
    /// Extra user zoom (Ctrl +/-), multiplied into text sizes.
    pub zoom: f32,
    pub requests: &'a mut Vec<Request>,
    pub cursor: &'a mut Cursor,
    /// Whether the view being drawn has keyboard focus (caret shown).
    pub focused: bool,
}

impl Ctx<'_> {
    /// Scale a logical length to physical pixels.
    pub fn px(&self, v: f32) -> f32 {
        (v * self.scale).round()
    }

    /// Scale a logical font size to physical pixels (includes zoom).
    pub fn font_px(&self, v: f32) -> f32 {
        (v * self.scale * self.zoom).round().max(4.0)
    }

    pub fn ui_metrics(&self) -> Metrics {
        let s = self.font_px(theme::FONT_SIZE);
        Metrics::new(s, (s * theme::LINE_HEIGHT).round())
    }

    pub fn code_metrics(&self) -> Metrics {
        let s = self.font_px(theme::CODE_FONT_SIZE);
        Metrics::new(s, (s * theme::LINE_HEIGHT).round())
    }

    pub fn family(&self, kind: FontKind) -> String {
        match kind {
            FontKind::Ui => self.fonts.sans.clone(),
            FontKind::Mono => self.fonts.mono.clone(),
        }
    }

    /// Draw one line of text clipped to `max_w` (and the current clip).
    #[allow(clippy::too_many_arguments)]
    pub fn label(
        &mut self,
        canvas: &mut Canvas,
        text: &str,
        x: i32,
        y: i32,
        color: Color,
        max_w: i32,
        kind: FontKind,
    ) {
        if max_w <= 0 || text.is_empty() {
            return;
        }
        let m = if kind == FontKind::Mono {
            self.code_metrics()
        } else {
            self.ui_metrics()
        };
        let fam = self.family(kind);
        let mut b = Buffer::new_empty(m);
        b.set_wrap(Wrap::None);
        b.set_size(Some(max_w as f32), Some(m.line_height));
        b.set_text(
            &crate::text::sanitize(clip_chars(text, 2000)),
            &Attrs::new().family(Family::Name(&fam)),
            Shaping::Advanced,
            None,
        );
        let prev = canvas.clip();
        canvas.set_clip(Rect::new(x, y, max_w, m.line_height as i32).intersect(&prev));
        self.fonts.draw_buffer(canvas, &mut b, x, y, color);
        canvas.set_clip(prev);
    }

    /// Width in pixels of `text` on one line.
    pub fn text_width(&mut self, text: &str, kind: FontKind) -> f32 {
        let m = if kind == FontKind::Mono {
            self.code_metrics()
        } else {
            self.ui_metrics()
        };
        let fam = self.family(kind);
        let mut b = Buffer::new_empty(m);
        b.set_wrap(Wrap::None);
        b.set_text(
            &crate::text::sanitize(clip_chars(text, 2000)),
            &Attrs::new().family(Family::Name(&fam)),
            Shaping::Advanced,
            None,
        );
        b.shape_until_scroll(&mut self.fonts.system, false);
        b.layout_runs().map(|r| r.line_w).fold(0.0, f32::max)
    }
}

/// Truncate to at most `max` chars on a char boundary.
pub fn clip_chars(s: &str, max: usize) -> &str {
    match s.char_indices().nth(max) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}
