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

    /// Test hook: window coordinates of the centre of each task checkbox.
    #[cfg(test)]
    fn checkboxes(&self) -> Vec<(f32, f32)> {
        Vec::new()
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

    /// Where the reader is in this file (None: nothing to come back to).
    /// Must work for a hidden tab: no layout, no `Ctx`.
    fn position(&self) -> Option<ViewPos> {
        None
    }

    /// Go back to a remembered place. The text may have changed since (or
    /// the value may come from a file someone else wrote): everything is
    /// clamped. Scrolling is resolved at the next draw.
    fn restore_position(&mut self, _pos: ViewPos) {}

    /// The view just jumped within its file by itself (a `#heading` link):
    /// where it was before, once.
    fn take_jump_origin(&mut self) -> Option<ViewPos> {
        None
    }
}

/// A place in a file that survives the text being laid out again (another
/// width or zoom, a restart): line numbers and fractions, never pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ViewPos {
    /// Caret: 0-based line and byte column.
    pub line: usize,
    pub col: usize,
    /// Editor: the line at the top of the view, and how far the view is
    /// scrolled past that line's first row, in thousandths of a row.
    pub top: usize,
    pub top_off: u32,
    /// Markdown preview: source line of the block at the top, and how far
    /// through that block the view is, in millionths.
    pub ptop: usize,
    pub poff: u32,
    /// Markdown: 0 preview, 1 source, 2 side by side.
    pub mode: u8,
}

impl ViewPos {
    /// Largest line / column accepted from a file.
    const MAX_LINE: u64 = u32::MAX as u64;
    pub const MAX_TOP_OFF: u32 = 1_000_000;
    pub const MAX_POFF: u32 = 1_000_000;

    /// One line of `name=number` words (zero values are left out).
    pub fn to_text(self) -> String {
        let fields: [(&str, u64); 7] = [
            ("l", self.line as u64),
            ("c", self.col as u64),
            ("t", self.top as u64),
            ("o", self.top_off as u64),
            ("p", self.ptop as u64),
            ("f", self.poff as u64),
            ("m", self.mode as u64),
        ];
        fields
            .iter()
            .filter(|(_, v)| *v != 0)
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Read `to_text` output. Unknown words and bad numbers are ignored and
    /// every value is capped (the file may be hand-made or hostile); the
    /// view clamps to the actual text when the position is restored.
    pub fn parse(text: &str) -> ViewPos {
        let mut p = ViewPos::default();
        for word in text.split_whitespace().take(32) {
            let Some((k, v)) = word.split_once('=') else {
                continue;
            };
            let Ok(n) = v.parse::<u64>() else { continue };
            let index = n.min(Self::MAX_LINE) as usize;
            match k {
                "l" => p.line = index,
                "c" => p.col = index,
                "t" => p.top = index,
                "o" => p.top_off = n.min(Self::MAX_TOP_OFF as u64) as u32,
                "p" => p.ptop = index,
                "f" => p.poff = n.min(Self::MAX_POFF as u64) as u32,
                "m" => p.mode = n.min(2) as u8,
                _ => {}
            }
        }
        p
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_pos_round_trips_and_caps_hostile_values() {
        let p = ViewPos {
            line: 12,
            col: 4,
            top: 3,
            top_off: 250,
            ptop: 40,
            poff: 123_456,
            mode: 2,
        };
        assert_eq!(p.to_text(), "l=12 c=4 t=3 o=250 p=40 f=123456 m=2");
        assert_eq!(ViewPos::parse(&p.to_text()), p);
        assert_eq!(ViewPos::default().to_text(), "");
        assert_eq!(ViewPos::parse(""), ViewPos::default());
        let hostile = ViewPos::parse(
            "l=99999999999999999999 c=-5 t=18446744073709551615 o=4000000000 \
             p=1e9 f=18446744073709551615 m=77 zz=1 l junk=== =3 c=7",
        );
        assert_eq!(hostile.line, 0, "too big for a number: ignored");
        assert_eq!(hostile.col, 7, "the last good value wins");
        assert_eq!(hostile.top, u32::MAX as usize);
        assert_eq!(hostile.top_off, ViewPos::MAX_TOP_OFF);
        assert_eq!(hostile.ptop, 0);
        assert_eq!(hostile.poff, ViewPos::MAX_POFF);
        assert_eq!(hostile.mode, 2);
    }
}
