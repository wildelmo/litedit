//! Views: things that fill a rectangle and react to input (lite-xl `View`).

pub mod code;
pub mod diff;
pub mod markdown;
pub mod message;
pub mod search;
pub mod wrap;

use std::time::Duration;

use crate::canvas::{Canvas, Rect};
use crate::editor::doc::SaveError;
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

    /// Other files this view shows (a comparison): when one changes on
    /// disk the app calls `reload`.
    fn watched_paths(&self) -> Vec<std::path::PathBuf> {
        Vec::new()
    }

    /// The full text, for "Compare with Saved".
    fn text(&self) -> Option<String> {
        None
    }

    /// The document's headings for "Go to Heading": (level, text, 0-based
    /// source line). None if this view has no outline.
    fn headings(&mut self) -> Option<Vec<(u8, String, usize)>> {
        None
    }

    /// Show the heading on 0-based source `line`.
    fn goto_heading(&mut self, _line: usize) {}

    /// A named editing command from the command palette ("upper-case",
    /// "trim-whitespace", …). Returns false if this view doesn't do it.
    fn run_command(&mut self, _name: &str) -> bool {
        false
    }

    /// Called when metrics change (DPI scale or zoom).
    fn metrics_changed(&mut self) {}

    /// The tab is no longer on screen: free everything that can be rebuilt
    /// (shaped text, laid-out previews, decoded images). Text, undo history,
    /// scroll position and selections stay.
    fn on_hide(&mut self) {
        self.metrics_changed();
    }

    /// Did drawing start background work (e.g. decoding an image again)
    /// that needs `tick` to be called until it finishes?
    fn has_background_work(&self) -> bool {
        false
    }

    /// Test hook: vertical scroll offset in pixels.
    #[cfg(test)]
    fn scroll_y(&self) -> f32 {
        0.0
    }

    /// Test hook: does this view hold rebuildable caches right now?
    #[cfg(test)]
    fn has_caches(&self) -> bool {
        false
    }

    /// Called after the palette changed: drop everything holding colours.
    fn theme_changed(&mut self) {
        self.metrics_changed();
    }

    /// Unsaved edits?
    fn is_dirty(&self) -> bool {
        false
    }

    fn save(&mut self, _allow_lossy: bool) -> Result<usize, SaveError> {
        Err(SaveError::Failed("nothing to save here".into()))
    }

    fn save_as(&mut self, _path: &std::path::Path, _allow_lossy: bool) -> Result<usize, SaveError> {
        Err(SaveError::Failed("nothing to save here".into()))
    }

    /// Compare the file on disk with what is shown.
    fn disk_state(&self) -> DiskState {
        DiskState::Same
    }

    /// Throw away edits and load the file from disk.
    fn reload_discarding(&mut self, _ctx: &mut Ctx) -> bool {
        false
    }

    /// The file vanished from disk: keep the text, mark it modified.
    fn mark_orphaned(&mut self) {}

    /// Screen rectangle of the primary caret at the last render (for
    /// placing the IME candidate window).
    fn caret_rect(&self) -> Option<Rect> {
        None
    }

    /// Number of lines (for go to line), if this is a text view.
    fn line_count(&self) -> Option<usize> {
        None
    }

    /// Move the caret to a 1-based line. Returns false if not supported.
    fn goto_line(&mut self, _line: usize) -> bool {
        false
    }

    /// Test hook: exact text and every selection endpoint.
    #[cfg(test)]
    fn snapshot(&self) -> Option<(String, Vec<crate::editor::buffer::Pos>)> {
        None
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiskState {
    Same,
    Differs,
    Missing,
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
        theme::p().scrollbar,
    );
}
