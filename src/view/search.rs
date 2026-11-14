//! Find in folder results (lite-xl `projectsearch`), shown in a tab.
//!
//! A background thread walks the folder with the tree's rules (ignore list,
//! no symlinked folders, cache folders skipped), skips binary and huge files,
//! and streams matches back. Closing the tab or starting another search
//! cancels the old one.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use crate::canvas::{Canvas, Rect};
use crate::input::{Cursor, Input, Key, MouseButton, Request};
use crate::palette::index_files;
use crate::theme;
use crate::ui::{Ctx, FontKind};
use crate::view::{Scroll, Tick, View, draw_vscrollbar};

pub const MAX_RESULTS: usize = 5000;
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct Hit {
    pub path: PathBuf,
    /// 1-based line number.
    pub line: usize,
    pub text: String,
    /// Match byte range within `text`.
    pub range: (usize, usize),
}

enum Msg {
    Hits(Vec<Hit>),
    Progress(usize),
    Done,
}

pub struct SearchView {
    query: String,
    root: PathBuf,
    hits: Vec<Hit>,
    rx: Receiver<Msg>,
    cancel: Arc<AtomicBool>,
    done: bool,
    scanned: usize,
    scroll: Scroll,
    selected: usize,
    last_rect: Rect,
}

/// Search one file's text: case-insensitive, literal, per line.
pub fn search_text(path: &Path, text: &str, needle_lower: &str, out: &mut Vec<Hit>) {
    for (i, line) in text.lines().enumerate() {
        let lower = line.to_lowercase();
        if let Some(pos) = lower.find(needle_lower) {
            // Map the lower-cased offset back to the original line safely.
            let start = line
                .char_indices()
                .map(|(b, _)| b)
                .find(|&b| line[..b].to_lowercase().len() >= pos)
                .unwrap_or(0);
            let end_target = pos + needle_lower.len();
            let end = line
                .char_indices()
                .map(|(b, _)| b)
                .chain(std::iter::once(line.len()))
                .find(|&b| b >= start && line[..b].to_lowercase().len() >= end_target)
                .unwrap_or(line.len());
            let shown = crate::ui::clip_chars(line, 300);
            let end = end.min(shown.len());
            out.push(Hit {
                path: path.to_path_buf(),
                line: i + 1,
                text: shown.to_string(),
                range: (start.min(end), end),
            });
            if out.len() >= MAX_RESULTS {
                return;
            }
        }
    }
}

fn looks_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8192).any(|b| *b == 0)
}

impl SearchView {
    pub fn start(root: &Path, query: &str) -> Self {
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = cancel.clone();
        let root_c = root.to_path_buf();
        let needle = query.to_lowercase();
        let _ = std::thread::Builder::new()
            .name("litedit-search".into())
            .spawn(move || {
                let files = index_files(&root_c);
                let mut total = 0;
                for (n, f) in files.iter().enumerate() {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    let Ok(meta) = std::fs::metadata(f) else {
                        continue;
                    };
                    if meta.len() > MAX_FILE_BYTES {
                        continue;
                    }
                    let Ok(bytes) = std::fs::read(f) else {
                        continue;
                    };
                    if looks_binary(&bytes) {
                        continue;
                    }
                    let (text, _) = crate::doc::decode(&bytes);
                    let mut hits = Vec::new();
                    search_text(f, &text, &needle, &mut hits);
                    total += hits.len();
                    if !hits.is_empty() && tx.send(Msg::Hits(hits)).is_err() {
                        return;
                    }
                    if n % 50 == 0 && tx.send(Msg::Progress(n + 1)).is_err() {
                        return;
                    }
                    if total >= MAX_RESULTS {
                        break;
                    }
                }
                let _ = tx.send(Msg::Progress(files.len()));
                let _ = tx.send(Msg::Done);
            });
        Self {
            query: query.to_string(),
            root: root.to_path_buf(),
            hits: Vec::new(),
            rx,
            cancel,
            done: false,
            scanned: 0,
            scroll: Scroll::default(),
            selected: 0,
            last_rect: Rect::new(0, 0, 0, 0),
        }
    }

    fn row_h(ctx: &Ctx) -> f32 {
        ctx.code_metrics().line_height + ctx.px(4.0)
    }

    fn header_h(ctx: &Ctx) -> f32 {
        ctx.ui_metrics().line_height + ctx.px(theme::PADDING_Y) * 2.0
    }

    fn open_selected(&self, ctx: &mut Ctx) {
        if let Some(h) = self.hits.get(self.selected) {
            ctx.requests
                .push(Request::OpenPathAt(h.path.clone(), h.line));
        }
    }

    fn keep_selected_visible(&mut self, ctx: &Ctx) {
        let rh = Self::row_h(ctx);
        let y = self.selected as f32 * rh;
        let view = self.last_rect.h as f32 - Self::header_h(ctx);
        if y < self.scroll.y {
            self.scroll.y = y;
        } else if y + rh > self.scroll.y + view {
            self.scroll.y = y + rh - view;
        }
    }

    fn clamp(&mut self, ctx: &Ctx) {
        let content = self.hits.len() as f32 * Self::row_h(ctx);
        let view = self.last_rect.h as f32 - Self::header_h(ctx);
        self.scroll.y = self.scroll.y.clamp(0.0, (content - view).max(0.0));
    }

    pub fn hit_count(&self) -> usize {
        self.hits.len()
    }
}

impl Drop for SearchView {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl View for SearchView {
    fn render(&mut self, canvas: &mut Canvas, rect: Rect, ctx: &mut Ctx) {
        self.last_rect = rect;
        canvas.fill_rect(rect, theme::BACKGROUND);
        self.clamp(ctx);
        let pad = ctx.px(theme::PADDING_X) as i32;
        let hh = Self::header_h(ctx) as i32;
        let lh_ui = ctx.ui_metrics().line_height as i32;
        let header = Rect::new(rect.x, rect.y, rect.w, hh);
        canvas.fill_rect(header, theme::BACKGROUND2);
        let status = format!(
            "{} result{} for \"{}\" in {}{}",
            self.hits.len(),
            if self.hits.len() == 1 { "" } else { "s" },
            self.query,
            self.root.display(),
            if self.done {
                String::new()
            } else {
                format!("  — searching ({} files)…", self.scanned)
            }
        );
        ctx.label(
            canvas,
            &status,
            rect.x + pad,
            rect.y + (hh - lh_ui) / 2,
            theme::ACCENT,
            rect.w - pad * 2,
            FontKind::Ui,
        );

        let rh = Self::row_h(ctx);
        let list = Rect::new(rect.x, rect.y + hh, rect.w, rect.h - hh);
        let prev = canvas.clip();
        canvas.set_clip(list.intersect(&prev));
        let first = (self.scroll.y / rh).floor() as usize;
        let lh = ctx.code_metrics().line_height as i32;
        let char_w = (ctx.text_width("0000000000", FontKind::Mono) / 10.0).max(1.0);
        for i in first
            ..self
                .hits
                .len()
                .min(first + (list.h as f32 / rh) as usize + 2)
        {
            let y = list.y + (i as f32 * rh - self.scroll.y) as i32;
            let row = Rect::new(list.x, y, list.w, rh as i32);
            if i == self.selected {
                canvas.fill_rect(
                    row,
                    if ctx.focused {
                        theme::SELECTION
                    } else {
                        theme::LINE_HIGHLIGHT
                    },
                );
            }
            let h = &self.hits[i];
            let rel = h
                .path
                .strip_prefix(&self.root)
                .unwrap_or(&h.path)
                .to_string_lossy()
                .replace('\\', "/");
            let loc = format!("{rel}:{}", h.line);
            let loc_w = (ctx.text_width(&loc, FontKind::Ui) as i32).min(list.w / 2);
            let ty = y + (rh as i32 - lh) / 2;
            ctx.label(
                canvas,
                &loc,
                list.x + pad,
                ty,
                theme::SYNTAX_FUNCTION,
                loc_w,
                FontKind::Ui,
            );
            let tx = list.x + pad + loc_w + pad;
            let text = h.text.trim_start();
            let trimmed = h.text.len() - text.len();
            let (a, b) = (
                h.range.0.saturating_sub(trimmed),
                h.range.1.saturating_sub(trimmed),
            );
            let before = text.get(..a).unwrap_or("");
            let mx = tx + (before.chars().count() as f32 * char_w) as i32;
            let mw = (text.get(a..b).unwrap_or("").chars().count() as f32 * char_w) as i32;
            canvas.fill_rect(Rect::new(mx, ty, mw.max(2), lh), theme::FIND_MATCH);
            ctx.label(
                canvas,
                text,
                tx,
                ty,
                theme::SYNTAX_NORMAL,
                list.right() - tx - pad,
                FontKind::Mono,
            );
        }
        canvas.set_clip(prev);
        let content = self.hits.len() as f32 * rh;
        draw_vscrollbar(
            canvas,
            list,
            self.scroll.y,
            content + list.h as f32 * 0.5,
            ctx,
        );
    }

    fn handle(&mut self, input: &Input, rect: Rect, ctx: &mut Ctx) -> bool {
        self.last_rect = rect;
        let n = self.hits.len();
        let rh = Self::row_h(ctx);
        match input {
            Input::Wheel { .. } => {
                self.scroll.wheel(input, rh);
            }
            Input::Key { key, .. } if n > 0 => {
                let page = ((rect.h as f32 / rh) as usize).max(1);
                match key {
                    Key::Down => self.selected = (self.selected + 1).min(n - 1),
                    Key::Up => self.selected = self.selected.saturating_sub(1),
                    Key::PageDown => self.selected = (self.selected + page).min(n - 1),
                    Key::PageUp => self.selected = self.selected.saturating_sub(page),
                    Key::Home => self.selected = 0,
                    Key::End => self.selected = n - 1,
                    Key::Enter => {
                        self.open_selected(ctx);
                        return true;
                    }
                    _ => return false,
                }
                self.keep_selected_visible(ctx);
            }
            Input::MouseMove { .. } => {
                *ctx.cursor = Cursor::Pointer;
                return false;
            }
            Input::MouseDown {
                y,
                button: MouseButton::Left,
                ..
            } => {
                let top = rect.y as f32 + Self::header_h(ctx);
                if *y >= top {
                    let i = ((*y - top + self.scroll.y) / rh) as usize;
                    if i < n {
                        self.selected = i;
                        self.open_selected(ctx);
                    }
                }
            }
            _ => return false,
        }
        self.clamp(ctx);
        true
    }

    fn tick(&mut self, _ctx: &mut Ctx, _budget: Duration) -> Tick {
        let mut redraw = false;
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Hits(h) => {
                    let room = MAX_RESULTS.saturating_sub(self.hits.len());
                    self.hits.extend(h.into_iter().take(room));
                }
                Msg::Progress(n) => self.scanned = n,
                Msg::Done => self.done = true,
            }
            redraw = true;
        }
        Tick {
            more: !self.done,
            redraw,
        }
    }

    fn title(&self) -> String {
        format!("Search: {}", self.query)
    }

    fn status(&self) -> Vec<String> {
        vec![
            format!("{} results", self.hits.len()),
            if self.done {
                "done".into()
            } else {
                "searching…".into()
            },
            "Enter or click: open".into(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_case_insensitive_hits() {
        let mut out = Vec::new();
        search_text(
            Path::new("x"),
            "Hello World\nnothing\nworld again",
            "world",
            &mut out,
        );
        assert_eq!(out.len(), 2);
        assert_eq!((out[0].line, out[0].range), (1, (6, 11)));
        assert_eq!(&out[1].text[out[1].range.0..out[1].range.1], "world");
    }

    #[test]
    fn background_search_finds_and_skips_binary() {
        let d = std::env::temp_dir().join(format!("litedit-srch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("sub")).unwrap();
        std::fs::write(d.join("a.md"), "needle here\n").unwrap();
        std::fs::write(d.join("sub/b.rs"), "fn x() {}\n// NEEDLE\n").unwrap();
        std::fs::write(d.join("bin.dat"), b"needle\0\0\0").unwrap();
        let mut v = SearchView::start(&d, "needle");
        let start = std::time::Instant::now();
        while !v.done && start.elapsed().as_secs() < 5 {
            v.rx.try_recv()
                .map(|m| match m {
                    Msg::Hits(h) => v.hits.extend(h),
                    Msg::Done => v.done = true,
                    Msg::Progress(_) => {}
                })
                .ok();
        }
        assert!(v.done);
        assert_eq!(v.hits.len(), 2, "{:?}", v.hits);
        std::fs::remove_dir_all(&d).ok();
    }
}
