//! Image viewer tab: a picture file shown fitted to the window or at 100%.
//!
//! Memory is bounded and only held while the tab is on screen:
//! - decoding runs on a short-lived background thread, one picture at a time
//!   for the whole app, and a decode nobody waits for any more is skipped;
//! - fitted to the window, the picture is kept at most
//!   [`STORE_MAX_SIDE`] pixels on its longest side plus one copy at the size
//!   drawn; every pixel of a larger picture is decoded only at 100%;
//! - at 100% the decoded pixels are drawn directly (no second copy);
//! - a hidden tab holds no pixels at all, just the picture's size.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use crate::canvas::{Canvas, Color, Rect};
use crate::images::{Decoded, STORE_MAX_SIDE, decode_file_max, downscale};
use crate::input::{Input, Key, MouseButton, WheelUnit};
use crate::theme;
use crate::ui::{Ctx, FontKind};
use crate::view::{DiskState, Scroll, Tick, View};

/// File types opened in the viewer (what the `image` crate is built with).
const EXTENSIONS: [&str; 6] = ["png", "jpg", "jpeg", "gif", "webp", "bmp"];

pub fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// Only one picture is decoded at a time, however many tabs ask: decoding a
/// photo briefly needs several times the memory of the result.
static DECODING: Mutex<()> = Mutex::new(());

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// Whole picture visible; never enlarged.
    Fit,
    /// One picture pixel per screen pixel.
    Actual,
}

struct Picture {
    d: Decoded,
    /// Any pixel not fully opaque (gets a checkerboard behind it).
    alpha: bool,
    /// Every pixel of the file is here (asked for, or it was small anyway).
    full: bool,
}

enum State {
    /// No pixels: not shown yet, hidden, or being decoded.
    Unloaded,
    Ready(Picture),
    Failed(String),
}

/// A decode in progress. Dropping it tells the thread not to bother (if it
/// hasn't started) and throws its result away (if it has).
struct Job {
    rx: Receiver<Result<Picture, String>>,
    cancel: Arc<AtomicBool>,
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Size and modification time: enough to notice the file changed.
type Stamp = Option<(u64, Option<SystemTime>)>;

fn stamp(path: &Path) -> Stamp {
    let m = std::fs::metadata(path).ok()?;
    Some((m.len(), m.modified().ok()))
}

pub struct ImageView {
    path: PathBuf,
    state: State,
    job: Option<Job>,
    mode: Mode,
    /// Offset of the view into the picture (100% only).
    scroll: Scroll,
    /// The picture at the size drawn in fit mode: (w, h, RGBA).
    scaled: Option<(u32, u32, Vec<u8>)>,
    /// The picture's real size, kept while the pixels are dropped.
    natural: Option<(u32, u32)>,
    /// Size on screen at the last draw.
    shown: Option<(i32, i32)>,
    /// The file when it was last (asked to be) read.
    stamp: Stamp,
    /// Dragging to pan: mouse position and scroll when the button went down.
    drag: Option<(f32, f32, Scroll)>,
    /// Drawn since it was last hidden. Decoding starts with the first draw,
    /// which comes after the app has told the tab shown before to free its
    /// pixels: two pictures are never held because of a tab switch.
    on_screen: bool,
}

impl ImageView {
    /// Nothing is read until the tab is first shown.
    pub fn new(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            state: State::Unloaded,
            job: None,
            mode: Mode::Fit,
            scroll: Scroll::default(),
            scaled: None,
            natural: None,
            shown: None,
            stamp: stamp(path),
            drag: None,
            on_screen: false,
        }
    }

    fn name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }

    /// Does the decoded copy suit the mode? 100% needs every pixel; fitted,
    /// a copy larger than the store limit is more than is worth keeping.
    fn suits(&self, p: &Picture) -> bool {
        match self.mode {
            Mode::Actual => p.full,
            Mode::Fit => !p.full || p.d.w.max(p.d.h) <= STORE_MAX_SIDE,
        }
    }

    /// Make sure the pixels this view needs are there or on their way
    /// (only while the tab is on screen).
    fn ensure(&mut self) {
        if self.job.is_some() || !self.on_screen {
            return;
        }
        let wanted = match &self.state {
            State::Unloaded => true,
            State::Ready(p) => !self.suits(p),
            // Tried and failed: again only when the file changes.
            State::Failed(_) => false,
        };
        if !wanted {
            return;
        }
        // Free what is there before decoding again.
        self.state = State::Unloaded;
        self.scaled = None;
        self.stamp = stamp(&self.path);
        let max_side = match self.mode {
            Mode::Actual => u32::MAX,
            Mode::Fit => STORE_MAX_SIDE,
        };
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let (path, cancelled) = (self.path.clone(), cancel.clone());
        let spawned = std::thread::Builder::new()
            .name("litedit-image".into())
            .spawn(move || {
                let _one_at_a_time = DECODING.lock().unwrap_or_else(|e| e.into_inner());
                if cancelled.load(Ordering::Relaxed) {
                    return;
                }
                let r = decode_file_max(&path, max_side).map(|d| Picture {
                    alpha: d.rgba.chunks_exact(4).any(|p| p[3] != 255),
                    full: max_side == u32::MAX || (d.w, d.h) == d.natural,
                    d,
                });
                // Nobody listening (tab hidden or closed): pixels are freed.
                let _ = tx.send(r);
            });
        match spawned {
            Ok(_) => self.job = Some(Job { rx, cancel }),
            Err(_) => self.state = State::Failed("image loader unavailable".into()),
        }
    }

    /// Collect a finished decode. Returns true if something changed.
    fn poll(&mut self) -> bool {
        let Some(job) = &self.job else { return false };
        let r = match job.rx.try_recv() {
            Ok(r) => r,
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => Err("image loader stopped".into()),
        };
        self.job = None;
        self.state = match r {
            Ok(p) => {
                self.natural = Some(p.d.natural);
                State::Ready(p)
            }
            Err(e) => State::Failed(e),
        };
        true
    }

    /// How far the view can scroll: nothing when fitted, the part of the
    /// picture that doesn't fit at 100%.
    fn scroll_limits(&self, rect: Rect) -> (f32, f32) {
        match (self.mode, self.natural) {
            (Mode::Actual, Some((w, h))) => (
                (w as f32 - rect.w as f32).max(0.0),
                (h as f32 - rect.h as f32).max(0.0),
            ),
            _ => (0.0, 0.0),
        }
    }

    /// Returns true if the scroll position changed.
    fn scroll_to(&mut self, x: f32, y: f32, rect: Rect) -> bool {
        let (mx, my) = self.scroll_limits(rect);
        let new = (x.clamp(0.0, mx).round(), y.clamp(0.0, my).round());
        let changed = new != (self.scroll.x, self.scroll.y);
        (self.scroll.x, self.scroll.y) = new;
        changed
    }

    /// Switch between fitted and 100%. The point of the picture under
    /// `anchor` (the middle of the view if none) stays where it is.
    fn set_mode(&mut self, mode: Mode, anchor: Option<(f32, f32)>, rect: Rect) -> bool {
        if self.mode == mode {
            return false;
        }
        self.mode = mode;
        self.drag = None;
        self.scaled = None;
        let mut target = (0.0, 0.0);
        if mode == Mode::Actual
            && let (Some((nw, nh)), Some((dw, dh))) = (self.natural, self.shown)
            && dw > 0
            && dh > 0
        {
            let (ax, ay) = anchor.unwrap_or((
                rect.x as f32 + rect.w as f32 / 2.0,
                rect.y as f32 + rect.h as f32 / 2.0,
            ));
            // Where the fitted picture was drawn.
            let ox = rect.x as f32 + ((rect.w - dw) / 2).max(0) as f32;
            let oy = rect.y as f32 + ((rect.h - dh) / 2).max(0) as f32;
            let fx = ((ax - ox) / dw as f32).clamp(0.0, 1.0);
            let fy = ((ay - oy) / dh as f32).clamp(0.0, 1.0);
            target = (
                fx * nw as f32 - (ax - rect.x as f32),
                fy * nh as f32 - (ay - rect.y as f32),
            );
        }
        self.scroll_to(target.0, target.1, rect);
        true
    }

    fn message(canvas: &mut Canvas, rect: Rect, ctx: &mut Ctx, lines: &[(&str, Color)]) {
        let lh = ctx.ui_metrics().line_height as i32;
        let pad = ctx.px(theme::PADDING_X) as i32;
        let mut y = rect.y + (rect.h - lh * lines.len() as i32) / 2;
        for (text, color) in lines {
            let w = ctx.text_width(text, FontKind::Ui) as i32;
            let x = rect.x + ((rect.w - w) / 2).max(pad);
            ctx.label(
                canvas,
                text,
                x,
                y,
                *color,
                rect.right() - pad - x,
                FontKind::Ui,
            );
            y += lh;
        }
    }
}

/// Size on screen of a picture stored at `w x h` pixels.
fn shown_size(mode: Mode, (w, h): (u32, u32), rect: Rect) -> (i32, i32) {
    match mode {
        Mode::Actual => (w as i32, h as i32),
        Mode::Fit => {
            let k = (rect.w as f32 / w.max(1) as f32)
                .min(rect.h as f32 / h.max(1) as f32)
                .min(1.0);
            (
                ((w as f32 * k).round() as i32).clamp(1, rect.w.max(1)),
                ((h as f32 * k).round() as i32).clamp(1, rect.h.max(1)),
            )
        }
    }
}

/// Two-tone squares behind a picture with transparency, in theme colours.
fn checkerboard(canvas: &mut Canvas, img: Rect, cell: i32) {
    let prev = canvas.clip();
    let vis = img.intersect(&prev);
    if vis.is_empty() {
        return;
    }
    canvas.set_clip(vis);
    canvas.fill_rect(vis, theme::p().background2);
    let cell = cell.max(2);
    // Squares are anchored to the picture so they move with it.
    let (c0, r0) = ((vis.x - img.x) / cell, (vis.y - img.y) / cell);
    let (c1, r1) = (
        (vis.right() - img.x - 1) / cell,
        (vis.bottom() - img.y - 1) / cell,
    );
    for r in r0..=r1 {
        for c in c0..=c1 {
            if (r + c) % 2 == 1 {
                canvas.fill_rect(
                    Rect::new(img.x + c * cell, img.y + r * cell, cell, cell),
                    theme::p().scrollbar,
                );
            }
        }
    }
    canvas.set_clip(prev);
}

fn human_size(n: u64) -> String {
    if n < 1024 {
        format!("{n} bytes")
    } else if n < 1024 * 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    }
}

impl View for ImageView {
    fn render(&mut self, canvas: &mut Canvas, rect: Rect, ctx: &mut Ctx) {
        canvas.fill_rect(rect, theme::p().background);
        if rect.is_empty() {
            return;
        }
        self.on_screen = true;
        self.poll();
        self.ensure();
        let name = self.name();
        let p = match &self.state {
            State::Ready(p) => p,
            State::Failed(e) => {
                let lines = [
                    (name.as_str(), theme::p().text),
                    (e.as_str(), theme::p().error),
                ];
                Self::message(canvas, rect, ctx, &lines);
                return;
            }
            State::Unloaded => {
                Self::message(canvas, rect, ctx, &[("loading…", theme::p().dim)]);
                return;
            }
        };
        let (dw, dh) = shown_size(self.mode, (p.d.w, p.d.h), rect);
        self.shown = Some((dw, dh));
        let (max_x, max_y) = ((dw - rect.w).max(0) as f32, (dh - rect.h).max(0) as f32);
        self.scroll.x = self.scroll.x.clamp(0.0, max_x).round();
        self.scroll.y = self.scroll.y.clamp(0.0, max_y).round();
        let x = rect.x + ((rect.w - dw) / 2).max(0) - self.scroll.x as i32;
        let y = rect.y + ((rect.h - dh) / 2).max(0) - self.scroll.y as i32;
        if p.alpha {
            checkerboard(canvas, Rect::new(x, y, dw, dh), ctx.px(8.0) as i32);
        }
        if (dw as u32, dh as u32) == (p.d.w, p.d.h) {
            // 1:1: straight from the decoded pixels (the canvas clips).
            self.scaled = None;
            canvas.draw_rgba(x, y, dw, dh, &p.d.rgba);
        } else {
            let (w, h) = (dw as u32, dh as u32);
            if !matches!(&self.scaled, Some((sw, sh, _)) if (*sw, *sh) == (w, h)) {
                // Drop the old copy before making the new one.
                self.scaled = None;
                self.scaled = Some((w, h, downscale(&p.d.rgba, p.d.w, p.d.h, 4, w, h)));
            }
            if let Some((_, _, px)) = &self.scaled {
                canvas.draw_rgba(x, y, dw, dh, px);
            }
        }
        // Thin thumbs showing which part of a larger picture is in view.
        let size = ctx.px(theme::SCROLLBAR_SIZE).max(2.0) as i32;
        let margin = ctx.px(2.0) as i32;
        let min_len = ctx.px(20.0) as i32;
        if dh > rect.h {
            let len = (rect.h as f32 * rect.h as f32 / dh as f32) as i32;
            let len = len.clamp(min_len.min(rect.h), rect.h);
            let at = (self.scroll.y / max_y.max(1.0) * (rect.h - len) as f32) as i32;
            canvas.fill_rect(
                Rect::new(rect.right() - size - margin, rect.y + at, size, len),
                theme::p().scrollbar,
            );
        }
        if dw > rect.w {
            let len = (rect.w as f32 * rect.w as f32 / dw as f32) as i32;
            let len = len.clamp(min_len.min(rect.w), rect.w);
            let at = (self.scroll.x / max_x.max(1.0) * (rect.w - len) as f32) as i32;
            canvas.fill_rect(
                Rect::new(rect.x + at, rect.bottom() - size - margin, len, size),
                theme::p().scrollbar,
            );
        }
    }

    fn handle(&mut self, input: &Input, rect: Rect, ctx: &mut Ctx) -> bool {
        let (sx, sy) = (self.scroll.x, self.scroll.y);
        match input {
            Input::Key { key, mods } if !mods.ctrl && !mods.alt => {
                let step = ctx.px(60.0);
                let page = (rect.h as f32 * 0.9).max(step);
                match key {
                    Key::Char('1') => self.set_mode(Mode::Actual, None, rect),
                    Key::Char('0') => self.set_mode(Mode::Fit, None, rect),
                    Key::Left => self.scroll_to(sx - step, sy, rect),
                    Key::Right => self.scroll_to(sx + step, sy, rect),
                    Key::Up => self.scroll_to(sx, sy - step, rect),
                    Key::Down => self.scroll_to(sx, sy + step, rect),
                    Key::PageUp => self.scroll_to(sx, sy - page, rect),
                    Key::PageDown => self.scroll_to(sx, sy + page, rect),
                    Key::Home => self.scroll_to(0.0, 0.0, rect),
                    Key::End => self.scroll_to(f32::MAX, f32::MAX, rect),
                    _ => false,
                }
            }
            // (Ctrl+wheel is the app's zoom and never gets here.)
            Input::Wheel { dx, dy, unit } => {
                let f = match unit {
                    WheelUnit::Lines => ctx.px(60.0),
                    WheelUnit::Pixels => 1.0,
                };
                // Shift turns the wheel sideways.
                let (dx, dy) = if ctx.mods.shift && *dx == 0.0 {
                    (*dy, 0.0)
                } else {
                    (*dx, *dy)
                };
                self.scroll_to(sx + dx * f, sy + dy * f, rect)
            }
            Input::MouseDown {
                x,
                y,
                button: MouseButton::Left,
                clicks,
            } => {
                if *clicks >= 2 {
                    let to = match self.mode {
                        Mode::Fit => Mode::Actual,
                        Mode::Actual => Mode::Fit,
                    };
                    self.set_mode(to, Some((*x, *y)), rect)
                } else {
                    self.drag = Some((*x, *y, self.scroll));
                    false
                }
            }
            Input::MouseMove { x, y } => match self.drag {
                Some((x0, y0, from)) => self.scroll_to(from.x - (x - x0), from.y - (y - y0), rect),
                None => false,
            },
            Input::MouseUp {
                button: MouseButton::Left,
                ..
            }
            | Input::MouseLeave => {
                self.drag = None;
                false
            }
            _ => false,
        }
    }

    fn tick(&mut self, _ctx: &mut Ctx, _budget: Duration) -> Tick {
        let redraw = self.poll();
        self.ensure();
        Tick {
            more: self.job.is_some(),
            redraw,
        }
    }

    fn title(&self) -> String {
        self.name()
    }

    fn status(&self) -> Vec<String> {
        let (Some((w, h)), false) = (self.natural, matches!(self.state, State::Failed(_))) else {
            return match self.state {
                State::Failed(_) => Vec::new(),
                _ => vec!["loading…".into()],
            };
        };
        let mut v = vec![format!("{w} × {h} px")];
        if let Some((len, _)) = self.stamp {
            v.push(human_size(len));
        }
        let percent = match (self.mode, self.shown) {
            (Mode::Actual, _) => Some(100.0),
            (Mode::Fit, Some((dw, _))) => Some(dw as f32 * 100.0 / w.max(1) as f32),
            (Mode::Fit, None) => None,
        };
        if let Some(p) = percent {
            v.push(format!("{}%", (p.round() as u32).max(1)));
        }
        v
    }

    fn path(&self) -> Option<&Path> {
        Some(&self.path)
    }

    fn can_save(&self) -> bool {
        false
    }

    /// The file changed (or vanished, or came back): forget the picture.
    /// It is decoded again when the tab is next drawn, not before.
    fn reload(&mut self, _ctx: &mut Ctx) -> bool {
        self.job = None;
        self.state = State::Unloaded;
        self.scaled = None;
        self.natural = None;
        self.shown = None;
        self.drag = None;
        self.stamp = stamp(&self.path);
        true
    }

    /// A deleted file counts as changed too: reloading shows "image not
    /// found" (there are no edits here to keep, unlike a text file).
    fn disk_state(&self) -> DiskState {
        if stamp(&self.path) == self.stamp {
            DiskState::Same
        } else {
            DiskState::Differs
        }
    }

    // Zoom and DPI changes don't touch the pixels.
    fn metrics_changed(&mut self) {}

    fn on_hide(&mut self) {
        // A decode still running is abandoned; its pixels are never kept.
        self.on_screen = false;
        self.job = None;
        if matches!(self.state, State::Ready(_)) {
            self.state = State::Unloaded;
        }
        self.scaled = None;
        self.drag = None;
    }

    fn has_background_work(&self) -> bool {
        self.job.is_some()
    }

    #[cfg(test)]
    fn scroll_y(&self) -> f32 {
        self.scroll.y
    }

    #[cfg(test)]
    fn has_caches(&self) -> bool {
        matches!(self.state, State::Ready(_)) || self.scaled.is_some() || self.job.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{fixture, tmp_dir};

    /// Show the view (as render and tick do) until its decode is finished.
    fn load(v: &mut ImageView) -> Option<String> {
        let start = std::time::Instant::now();
        v.on_screen = true;
        loop {
            v.poll();
            v.ensure();
            if v.job.is_none() || start.elapsed().as_secs() > 20 {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        match &v.state {
            State::Failed(e) => Some(e.clone()),
            State::Ready(_) => None,
            State::Unloaded => Some("still loading".into()),
        }
    }

    #[test]
    fn the_decoders_reason_is_shown_when_a_picture_cannot_be() {
        let d = tmp_dir("imgview-bad");
        let open = |p: &Path| load(&mut ImageView::new(p)).unwrap_or_default();
        assert_eq!(open(&d.join("missing.png")), "image not found");
        assert!(open(&fixture("img/broken.png")).starts_with("cannot decode image"));
        let text = d.join("text.gif");
        std::fs::write(&text, "not a picture").unwrap();
        assert!(
            open(&text).starts_with("cannot decode image"),
            "{}",
            open(&text)
        );
        // Too many pixels: refused from the header, before any are decoded.
        let huge = d.join("huge.bmp");
        let mut bmp = Vec::new();
        image::RgbImage::new(1, 1)
            .write_to(&mut std::io::Cursor::new(&mut bmp), image::ImageFormat::Bmp)
            .unwrap();
        bmp[18..22].copy_from_slice(&6000u32.to_le_bytes());
        bmp[22..26].copy_from_slice(&6000u32.to_le_bytes());
        std::fs::write(&huge, &bmp).unwrap();
        assert_eq!(open(&huge), "image too large (6000 × 6000)");
        assert_eq!(open(&d), "not a file");
        #[cfg(unix)]
        {
            let link = d.join("link.png");
            std::os::unix::fs::symlink(fixture("img/banner.png"), &link).unwrap();
            assert_eq!(open(&link), "linked images are not opened");
        }
        assert_eq!(open(&fixture("img/banner.png")), "", "a good one loads");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn hiding_drops_pixels_and_a_running_decode() {
        let mut v = ImageView::new(&fixture("img/banner.png"));
        v.ensure();
        assert!(
            !v.has_caches() && !v.has_background_work(),
            "nothing is read before the tab is drawn"
        );
        v.on_screen = true;
        v.ensure();
        assert!(v.has_background_work());
        v.on_hide();
        assert!(!v.has_background_work() && !v.has_caches());
        assert_eq!(load(&mut v), None);
        assert!(v.has_caches());
        assert_eq!(v.natural, Some((360, 120)));
        v.on_hide();
        assert!(!v.has_caches());
        assert_eq!(v.natural, Some((360, 120)), "the size is remembered");
        assert!(v.status()[0].contains("360 × 120 px"));
        // A failure is not retried on every frame.
        let mut bad = ImageView::new(&fixture("img/broken.png"));
        assert!(load(&mut bad).is_some());
        bad.ensure();
        assert!(bad.on_screen && !bad.has_background_work());
    }

    #[test]
    fn picks_image_files_by_extension() {
        for yes in [
            "a.png", "A.PNG", "b.Jpg", "c.jpeg", "d.gif", "e.webp", "f.BMP",
        ] {
            assert!(is_image(Path::new(yes)), "{yes}");
        }
        for no in ["a.svg", "png", "a.png.txt", "a.md", "a.ico", "a.tiff", "a"] {
            assert!(!is_image(Path::new(no)), "{no}");
        }
    }

    #[test]
    fn fitted_size_never_enlarges_and_never_overflows() {
        let r = Rect::new(0, 0, 400, 300);
        assert_eq!(shown_size(Mode::Fit, (100, 50), r), (100, 50));
        assert_eq!(shown_size(Mode::Fit, (800, 300), r), (400, 150));
        assert_eq!(shown_size(Mode::Fit, (300, 600), r), (150, 300));
        assert_eq!(shown_size(Mode::Fit, (10_000, 1), r), (400, 1));
        assert_eq!(shown_size(Mode::Actual, (800, 600), r), (800, 600));
        assert_eq!(shown_size(Mode::Fit, (5, 5), Rect::new(0, 0, 0, 0)), (1, 1));
    }
}
