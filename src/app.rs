//! Application state and rendering. Platform-independent: the window and the
//! headless screenshot mode both call [`App::render`].

use std::path::{Path, PathBuf};

use cosmic_text::{Attrs, Buffer, Family, Metrics, Scroll, Shaping, Wrap};

use crate::canvas::{Canvas, Rect};
use crate::doc::Document;
use crate::text::Fonts;
use crate::theme;

pub struct App {
    pub fonts: Fonts,
    pub scale: f32,
    doc: Option<Document>,
    error: Option<String>,
    /// Full file text, laid out without wrapping (code-view style).
    body: Buffer,
    body_dirty: bool,
    line_count: usize,
    /// Vertical scroll position in physical pixels.
    scroll_px: f32,
    /// Size of the text area at the last render, for clamping.
    view_h: f32,
}

impl App {
    pub fn new(fonts: Fonts, scale: f32) -> Self {
        let body = Buffer::new_empty(Metrics::new(1.0, 1.0));
        let mut app = Self {
            fonts,
            scale,
            doc: None,
            error: None,
            body,
            body_dirty: true,
            line_count: 0,
            scroll_px: 0.0,
            view_h: 0.0,
        };
        app.apply_metrics();
        app
    }

    pub fn open(&mut self, path: &Path) {
        match Document::open(path) {
            Ok(doc) => {
                self.line_count = doc.text.lines().count().max(1);
                self.doc = Some(doc);
                self.error = None;
            }
            Err(e) => {
                self.doc = None;
                self.error = Some(format!("Could not open {}: {e}", path.display()));
            }
        }
        self.scroll_px = 0.0;
        self.body_dirty = true;
    }

    pub fn title(&self) -> String {
        match &self.doc {
            Some(d) => format!("{} - litedit", d.file_name()),
            None => "litedit".to_string(),
        }
    }

    pub fn set_scale(&mut self, scale: f32) {
        if (scale - self.scale).abs() > f32::EPSILON {
            self.scale = scale;
            self.apply_metrics();
        }
    }

    fn px(&self, v: f32) -> f32 {
        (v * self.scale).round()
    }

    fn code_metrics(&self) -> Metrics {
        let size = self.px(theme::CODE_FONT_SIZE);
        Metrics::new(size, (size * theme::LINE_HEIGHT).round())
    }

    fn apply_metrics(&mut self) {
        let m = self.code_metrics();
        self.body.set_metrics(m);
        self.body_dirty = true;
    }

    /// Scroll by a number of lines (positive = down).
    pub fn scroll_lines(&mut self, lines: f32) {
        let lh = self.code_metrics().line_height;
        self.scroll_px += lines * lh;
        self.clamp_scroll();
    }

    /// Scroll by physical pixels (touchpads).
    pub fn scroll_pixels(&mut self, px: f32) {
        self.scroll_px += px;
        self.clamp_scroll();
    }

    pub fn scroll_pages(&mut self, pages: f32) {
        self.scroll_px += pages * (self.view_h * 0.9);
        self.clamp_scroll();
    }

    pub fn scroll_to_start(&mut self) {
        self.scroll_px = 0.0;
    }

    pub fn scroll_to_end(&mut self) {
        self.scroll_px = f32::MAX;
        self.clamp_scroll();
    }

    fn content_height(&self) -> f32 {
        self.line_count as f32 * self.code_metrics().line_height
    }

    fn clamp_scroll(&mut self) {
        let max = (self.content_height() - self.view_h * 0.5).max(0.0);
        self.scroll_px = self.scroll_px.clamp(0.0, max);
    }

    fn rebuild_body(&mut self) {
        let mono = self.fonts.mono.clone();
        let attrs = Attrs::new().family(Family::Name(&mono));
        let text = self.doc.as_ref().map(|d| d.text.as_str()).unwrap_or("");
        self.body.set_wrap(Wrap::None);
        self.body.set_text(text, &attrs, Shaping::Advanced, None);
        self.body_dirty = false;
    }

    pub fn render(&mut self, canvas: &mut Canvas) {
        let (w, h) = (canvas.width(), canvas.height());
        canvas.set_clip(canvas.bounds());
        canvas.fill_rect(canvas.bounds(), theme::BACKGROUND);

        let pad_x = self.px(theme::PADDING_X) as i32;
        let pad_y = self.px(theme::PADDING_Y) as i32;
        let ui_size = self.px(theme::FONT_SIZE);
        let ui_metrics = Metrics::new(ui_size, (ui_size * theme::LINE_HEIGHT).round());
        let bar_h = ui_metrics.line_height as i32 + pad_y * 2;

        // Title bar
        let title_rect = Rect::new(0, 0, w, bar_h);
        canvas.fill_rect(title_rect, theme::BACKGROUND2);
        canvas.fill_rect(Rect::new(0, bar_h - 1, w, 1), theme::DIVIDER);
        let title = match (&self.doc, &self.error) {
            (Some(d), _) => d.path.display().to_string(),
            (None, Some(_)) => "Error".to_string(),
            (None, None) => "litedit — open a file: litedit <path>".to_string(),
        };
        self.draw_label(canvas, &title, pad_x, pad_y, ui_metrics, theme::ACCENT, w - pad_x * 2);

        // Status bar
        let status_y = h - bar_h;
        canvas.fill_rect(Rect::new(0, status_y, w, bar_h), theme::BACKGROUND2);
        let status = match &self.doc {
            Some(d) => format!(
                "{} lines   {:?}   {:?}   font {} / {}",
                self.line_count, d.line_ending, d.encoding, self.fonts.mono, self.fonts.sans
            ),
            None => String::new(),
        };
        self.draw_label(canvas, &status, pad_x, status_y + pad_y, ui_metrics, theme::TEXT, w - pad_x * 2);

        // Document area
        let area = Rect::new(0, bar_h, w, (status_y - bar_h).max(0));
        self.view_h = area.h as f32;
        self.clamp_scroll();
        canvas.set_clip(area);

        if let Some(err) = self.error.clone() {
            self.draw_label(canvas, &err, pad_x, area.y + pad_y, ui_metrics, theme::ERROR, w - pad_x * 2);
            canvas.set_clip(canvas.bounds());
            return;
        }
        if self.doc.is_none() {
            canvas.set_clip(canvas.bounds());
            return;
        }
        if self.body_dirty {
            self.rebuild_body();
        }

        let m = self.code_metrics();
        let lh = m.line_height;
        let gutter_digits = self.line_count.to_string().len().max(3);
        let digit_w = self.measure_mono_digit(m);
        let gutter_w = (gutter_digits as f32 * digit_w).ceil() as i32 + pad_x * 2;

        let first_line = (self.scroll_px / lh).floor() as usize;
        let offset = self.scroll_px - first_line as f32 * lh;
        let text_x = area.x + gutter_w;
        let text_w = (area.w - gutter_w).max(1);

        self.body.set_size(Some(text_w as f32), Some(area.h as f32 + lh));
        self.body.set_scroll(Scroll::new(first_line, offset, 0.0));
        let top = area.y + pad_y;

        // Line numbers
        let visible = (area.h as f32 / lh).ceil() as usize + 1;
        for i in 0..visible {
            let n = first_line + i;
            if n >= self.line_count {
                break;
            }
            let y = top + (i as f32 * lh - offset) as i32;
            let label = (n + 1).to_string();
            let lw = (label.len() as f32 * digit_w) as i32;
            self.draw_label(canvas, &label, area.x + gutter_w - pad_x - lw, y, m, theme::LINE_NUMBER, gutter_w);
        }

        // Text
        let text_clip = Rect::new(text_x, area.y, text_w, area.h);
        canvas.set_clip(text_clip);
        let mut body = std::mem::replace(&mut self.body, Buffer::new_empty(m));
        self.fonts.draw_buffer(canvas, &mut body, text_x, top, theme::SYNTAX_NORMAL);
        self.body = body;

        // Scrollbar
        canvas.set_clip(area);
        let content_h = self.content_height().max(1.0);
        if content_h > area.h as f32 {
            let sb_w = self.px(theme::SCROLLBAR_SIZE) as i32;
            let thumb_h = ((area.h as f32 / content_h) * area.h as f32).max(self.px(20.0)) as i32;
            let max_scroll = (content_h - area.h as f32 * 0.5).max(1.0);
            let thumb_y = area.y + ((self.scroll_px / max_scroll) * (area.h - thumb_h) as f32) as i32;
            canvas.fill_rect(Rect::new(area.right() - sb_w - 2, thumb_y, sb_w, thumb_h), theme::SCROLLBAR);
        }
        canvas.set_clip(canvas.bounds());
    }

    fn measure_mono_digit(&mut self, m: Metrics) -> f32 {
        let mono = self.fonts.mono.clone();
        let mut b = Buffer::new(&mut self.fonts.system, m);
        let attrs = Attrs::new().family(Family::Name(&mono));
        b.set_text("0000000000", &attrs, Shaping::Advanced, None);
        b.shape_until_scroll(&mut self.fonts.system, false);
        b.layout_runs().next().map(|r| r.line_w / 10.0).unwrap_or(m.font_size * 0.6)
    }

    /// Draw a single line of UI text (sans font), clipped to `max_w`.
    fn draw_label(
        &mut self,
        canvas: &mut Canvas,
        text: &str,
        x: i32,
        y: i32,
        m: Metrics,
        color: crate::canvas::Color,
        max_w: i32,
    ) {
        let mut b = Buffer::new(&mut self.fonts.system, m);
        b.set_wrap(Wrap::None);
        b.set_size(Some(max_w as f32), Some(m.line_height));
        b.set_text(text, &Attrs::new(), Shaping::Advanced, None);
        let prev = canvas.clip();
        canvas.set_clip(Rect::new(x, y, max_w, m.line_height as i32).intersect(&prev));
        self.fonts.draw_buffer(canvas, &mut b, x, y, color);
        canvas.set_clip(prev);
    }
}

/// Resolve the path given on the command line without `canonicalize`
/// (which yields `\\?\C:\...` paths on Windows).
pub fn resolve_arg_path(arg: &str) -> PathBuf {
    let p = PathBuf::from(arg);
    if p.is_absolute() {
        p
    } else {
        std::env::current_dir().map(|d| d.join(&p)).unwrap_or(p)
    }
}
