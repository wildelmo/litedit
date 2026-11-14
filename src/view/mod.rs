//! Views: things that fill a rectangle and react to input (lite-xl `View`).

pub mod code;
pub mod markdown;
pub mod message;

use std::time::Duration;

use crate::canvas::{Canvas, Rect};
use crate::input::Input;
use crate::theme;
use crate::ui::Ctx;

/// Result of a unit of deferred work.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tick {
    /// More work remains.
    pub more: bool,
    /// Something visible changed.
    pub redraw: bool,
}

pub trait View {
    /// Draw into `rect`. The canvas clip is already set to `rect`.
    fn render(&mut self, canvas: &mut Canvas, rect: Rect, ctx: &mut Ctx);

    /// React to input. Mouse coordinates are window-relative; `rect` is this
    /// view's area. Returns true if a redraw is needed.
    fn handle(&mut self, input: &Input, rect: Rect, ctx: &mut Ctx) -> bool;

    /// Do up to `budget` of deferred work (e.g. syntax highlighting).
    fn tick(&mut self, _ctx: &mut Ctx, _budget: Duration) -> Tick {
        Tick::default()
    }

    /// Window title fragment (usually the file name).
    fn title(&self) -> String;

    /// Items for the status bar, left to right.
    fn status(&self) -> Vec<String> {
        Vec::new()
    }

    /// The file shown, if any.
    fn path(&self) -> Option<&std::path::Path> {
        None
    }

    /// Reload from disk after an external change. Returns true if reloaded.
    fn reload(&mut self, _ctx: &mut Ctx) -> bool {
        false
    }

    /// Called when metrics change (DPI scale or zoom).
    fn metrics_changed(&mut self) {}
}

/// Vertical + horizontal scroll state, in physical pixels.
#[derive(Clone, Copy, Debug, Default)]
pub struct Scroll {
    pub x: f32,
    pub y: f32,
}

impl Scroll {
    /// Clamp so you can scroll until the last line sits mid-view (lite-xl
    /// lets you scroll past the end like this).
    pub fn clamp(&mut self, content_w: f32, content_h: f32, view_w: f32, view_h: f32) {
        let max_y = (content_h - view_h * 0.5).max(0.0);
        let max_x = (content_w - view_w * 0.5).max(0.0);
        self.y = self.y.clamp(0.0, max_y);
        self.x = self.x.clamp(0.0, max_x);
    }

    /// Apply a wheel event. Returns true if it was a wheel event.
    pub fn wheel(&mut self, input: &Input, line_h: f32) -> bool {
        if let Input::Wheel { dx, dy, unit } = input {
            let f = match unit {
                crate::input::WheelUnit::Lines => line_h * 3.0,
                crate::input::WheelUnit::Pixels => 1.0,
            };
            self.y += dy * f;
            self.x += dx * f;
            true
        } else {
            false
        }
    }
}

/// Draw a thin lite-xl style vertical scrollbar thumb.
pub fn draw_vscrollbar(canvas: &mut Canvas, rect: Rect, scroll_y: f32, content_h: f32, ctx: &Ctx) {
    let view_h = rect.h as f32;
    if content_h <= view_h || rect.h <= 0 {
        return;
    }
    let sb_w = ctx.px(theme::SCROLLBAR_SIZE).max(2.0) as i32;
    let min_thumb = ctx.px(20.0);
    let thumb_h = ((view_h / content_h) * view_h).max(min_thumb).min(view_h) as i32;
    let max_scroll = (content_h - view_h * 0.5).max(1.0);
    let t = (scroll_y / max_scroll).clamp(0.0, 1.0);
    let thumb_y = rect.y + (t * (rect.h - thumb_h) as f32) as i32;
    let margin = ctx.px(2.0) as i32;
    canvas.fill_rect(
        Rect::new(rect.right() - sb_w - margin, thumb_y, sb_w, thumb_h),
        theme::SCROLLBAR,
    );
}
