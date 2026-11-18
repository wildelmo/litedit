//! Help → About litedit: the version of the running program and who made
//! this build (see `branding`). Shown as a tab; it has nothing to click.

use crate::branding::Branding;
use crate::canvas::{Canvas, Rect};
use crate::input::Input;
use crate::theme;
use crate::ui::{Ctx, FontKind};
use crate::view::View;

/// The tab's title; also how the app finds an About tab that is open.
pub const TITLE: &str = "About";

pub struct AboutView {
    branding: Branding,
}

impl AboutView {
    pub fn new(branding: Branding) -> Self {
        Self { branding }
    }

    /// The first line: the program and the version compiled into it.
    pub fn heading() -> String {
        format!("litedit {}", env!("CARGO_PKG_VERSION"))
    }
}

impl View for AboutView {
    fn render(&mut self, canvas: &mut Canvas, rect: Rect, ctx: &mut Ctx) {
        // Colours are read here, every time: nothing to drop on a theme change.
        let p = theme::p();
        canvas.fill_rect(rect, p.background);
        let lh = ctx.ui_metrics().line_height as i32;
        let row_h = lh + ctx.px(6.0) as i32;
        let gap = ctx.px(16.0) as i32;
        let pad = ctx.px(10.0) as i32;
        let rows = self.branding.rows();
        let total = lh * 2 + gap + row_h * rows.len() as i32 + gap + lh;
        let width = ctx.px(460.0).min((rect.w - 2 * pad).max(0) as f32) as i32;
        let x = rect.x + ((rect.w - width) / 2).max(pad.min(rect.w / 2));
        let mut y = rect.y + ((rect.h - total) / 2).max(pad);

        ctx.label(
            canvas,
            &Self::heading(),
            x,
            y,
            p.accent,
            width,
            FontKind::Ui,
        );
        y += lh;
        ctx.label(
            canvas,
            env!("CARGO_PKG_DESCRIPTION"),
            x,
            y,
            p.text,
            width,
            FontKind::Ui,
        );
        y += lh + gap;

        // Labels in one column, values in the next.
        let label_w = rows
            .iter()
            .map(|(l, _)| ctx.text_width(l, FontKind::Ui) as i32)
            .max()
            .unwrap_or(0)
            + gap;
        for (label, value) in &rows {
            let ty = y + (row_h - lh) / 2;
            ctx.label(canvas, label, x, ty, p.dim, width, FontKind::Ui);
            let room = width - label_w;
            if room > 0 {
                ctx.label(canvas, value, x + label_w, ty, p.text, room, FontKind::Ui);
            }
            y += row_h;
        }
        y += gap;
        ctx.label(
            canvas,
            "MIT licence. Designed after lite-xl (see NOTICE).",
            x,
            y,
            p.dim,
            width,
            FontKind::Ui,
        );
    }

    fn handle(&mut self, _input: &Input, _rect: Rect, _ctx: &mut Ctx) -> bool {
        false
    }

    fn title(&self) -> String {
        TITLE.to_string()
    }
}
