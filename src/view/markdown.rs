//! Markdown preview (the default for `.md` files), with `Ctrl+E` switching
//! to the editable source. The preview is re-parsed from the editor's text
//! whenever it changes, and the scroll position follows across the switch.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::canvas::{Canvas, Rect};
use crate::doc::Document;
use crate::editor::doc::SaveError;
use crate::images::{ImageStore, ImgState, scale_to};
use crate::input::{Cursor as MouseCursor, Input, Key, MouseButton, Request};
use crate::links::{LinkTarget, classify};
use crate::markdown::layout::{
    Element, ImgInfo, Layout, LayoutCtx, META_CODE, layout, link_from_meta,
};
use crate::markdown::parse::{Block, BlockKind, Inline, MdDoc, parse};
use crate::theme;
use crate::ui::{Ctx, FontKind};
use crate::view::code::CodeView;
use crate::view::{DiskState, Scroll, Tick, View, draw_vscrollbar};
use cosmic_text::Cursor;

pub fn is_markdown(path: &Path) -> bool {
    path.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .is_some_and(|e| {
            matches!(
                e.as_str(),
                "md" | "markdown" | "mdown" | "mkd" | "mkdn" | "mdwn"
            )
        })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Preview,
    Source,
    /// Editor on the left, live preview on the right.
    Split,
}

/// Re-parsing a large document on every keystroke would lag; wait this long
/// after the last edit before refreshing the side-by-side preview.
const LIVE_DEBOUNCE: Duration = Duration::from_millis(180);
const LIVE_DEBOUNCE_MIN_BYTES: usize = 30_000;

pub struct MarkdownView {
    /// The source editor; the single owner of the document text.
    editor: CodeView,
    md: MdDoc,
    /// Sanitized `\n` text the preview was parsed from, and its version.
    md_text: String,
    md_version: u64,
    /// Layout and the (width, scale, image version) it was made for.
    layout: Option<((i32, u32, u32), Layout)>,
    images: ImageStore,
    /// Bumped whenever an image finishes loading (forces relayout).
    images_version: u32,
    /// Scaled pixels per (path, w, h), for the current display size.
    scaled: HashMap<(PathBuf, u32, u32), Vec<u8>>,
    scroll: Scroll,
    mode: Mode,
    hover_link: Option<usize>,
    /// The preview's rectangle (the right half in split mode).
    last_rect: Rect,
    last_edit: std::time::Instant,
    mouse_x: f32,
    seen_version: u64,
    /// Caret line the split preview last followed.
    followed_line: usize,
    /// Preview text selection: (element index, cursor) anchor and head.
    sel: Option<((usize, Cursor), (usize, Cursor))>,
    selecting: bool,
    /// Message shown in the status bar (e.g. blocked link).
    notice: Option<String>,
    /// Scroll the preview to this source line once the layout exists.
    pending_scroll_line: Option<usize>,
}

impl MarkdownView {
    pub fn new(doc: Document) -> Self {
        let mut editor = CodeView::new(doc);
        editor.wrap = true; // prose reads better wrapped (D-022)
        let md_text = crate::text::sanitize(&editor.doc.buf.to_text_lf()).into_owned();
        let md = parse(&md_text);
        let md_version = editor.doc.version();
        let mut v = Self {
            editor,
            md,
            md_text,
            md_version,
            layout: None,
            images: ImageStore::new(),
            images_version: 0,
            scaled: HashMap::new(),
            scroll: Scroll::default(),
            mode: Mode::Preview,
            hover_link: None,
            last_rect: Rect::new(0, 0, 0, 0),
            last_edit: std::time::Instant::now(),
            mouse_x: 0.0,
            seen_version: 0,
            followed_line: usize::MAX,
            sel: None,
            selecting: false,
            notice: None,
            pending_scroll_line: None,
        };
        v.request_images();
        v
    }

    /// Every image source in the document.
    fn image_sources(&self) -> Vec<String> {
        fn walk(blocks: &[Block], out: &mut Vec<String>) {
            for b in blocks {
                match &b.kind {
                    BlockKind::Paragraph(i) | BlockKind::Heading { inlines: i, .. } => {
                        for x in i {
                            if let Inline::Image { src, .. } = x {
                                out.push(src.clone());
                            }
                        }
                    }
                    BlockKind::Quote { blocks, .. } | BlockKind::Footnote { blocks, .. } => {
                        walk(blocks, out)
                    }
                    BlockKind::List { items, .. } => {
                        for it in items {
                            walk(&it.blocks, out);
                        }
                    }
                    _ => {}
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.md.blocks, &mut out);
        out.sort();
        out.dedup();
        out
    }

    /// What layout should know about each image source; starts loads.
    fn request_images(&mut self) -> HashMap<String, ImgInfo> {
        let base = self.base_dir();
        let mut map = HashMap::new();
        for src in self.image_sources() {
            let info = match classify(&src, &base) {
                LinkTarget::File(p) => {
                    let ext = p
                        .extension()
                        .map(|e| e.to_string_lossy().to_ascii_lowercase())
                        .unwrap_or_default();
                    if ext == "svg" {
                        ImgInfo::Note("SVG images are not supported".into())
                    } else {
                        match self.images.get(&p) {
                            ImgState::Ready(d) => ImgInfo::Size {
                                w: d.natural.0,
                                h: d.natural.1,
                                path: p,
                            },
                            // Pixels dropped while hidden: same size, so
                            // nothing moves while it is decoded again.
                            ImgState::Unloaded { natural, .. } => ImgInfo::Size {
                                w: natural.0,
                                h: natural.1,
                                path: p,
                            },
                            ImgState::Loading => ImgInfo::Note("loading…".into()),
                            ImgState::Failed(e) => ImgInfo::Note(e),
                        }
                    }
                }
                LinkTarget::Web(_) => ImgInfo::Note("remote images are not loaded".into()),
                LinkTarget::Anchor(_) => ImgInfo::Note("not an image".into()),
                LinkTarget::Blocked(why) => ImgInfo::Note(why.into()),
            };
            map.insert(src, info);
        }
        map
    }

    fn base_dir(&self) -> PathBuf {
        self.editor
            .doc
            .path()
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default()
    }

    /// The selectable text element and buffer cursor under a window point.
    fn text_hit(&self, px: f32, py: f32) -> Option<(usize, Cursor)> {
        let (_, l) = self.layout.as_ref()?;
        let x = px - self.last_rect.x as f32;
        let y = py - self.last_rect.y as f32 + self.scroll.y;
        let mut best: Option<(usize, f32)> = None;
        for (i, el) in l.elements.iter().enumerate() {
            if let Element::Text {
                y: ey,
                h,
                sel: true,
                ..
            } = el
            {
                let d = if y < *ey {
                    ey - y
                } else if y > ey + h {
                    y - ey - h
                } else {
                    0.0
                };
                if best.is_none_or(|(_, bd)| d < bd) {
                    best = Some((i, d));
                }
            }
        }
        let (i, _) = best?;
        let Element::Text {
            x: ex,
            y: ey,
            h,
            buffer,
            ..
        } = &l.elements[i]
        else {
            return None;
        };
        let ly = (y - ey).clamp(0.0, (h - 1.0).max(0.0));
        buffer.hit(x - ex, ly).map(|c| (i, c))
    }

    fn sel_range(&self) -> Option<((usize, Cursor), (usize, Cursor))> {
        let (a, b) = self.sel?;
        let key = |p: &(usize, Cursor)| (p.0, p.1.line, p.1.index);
        if a == b {
            return None;
        }
        Some(if key(&a) <= key(&b) { (a, b) } else { (b, a) })
    }

    /// The selected preview text as plain text.
    pub fn selected_text(&self) -> String {
        let (Some((lo, hi)), Some((_, l))) = (self.sel_range(), self.layout.as_ref()) else {
            return String::new();
        };
        let mut parts = Vec::new();
        for i in lo.0..=hi.0 {
            let Some(Element::Text {
                buffer, sel: true, ..
            }) = l.elements.get(i)
            else {
                continue;
            };
            let mut lines = Vec::new();
            for (li, bl) in buffer.lines.iter().enumerate() {
                let t = bl.text();
                let start = if i == lo.0 && li == lo.1.line {
                    lo.1.index.min(t.len())
                } else if i == lo.0 && li < lo.1.line {
                    continue;
                } else {
                    0
                };
                let end = if i == hi.0 && li == hi.1.line {
                    hi.1.index.min(t.len())
                } else if i == hi.0 && li > hi.1.line {
                    break;
                } else {
                    t.len()
                };
                if let Some(seg) = t.get(start..end.max(start)) {
                    lines.push(seg.to_string());
                }
            }
            parts.push(lines.join("\n"));
        }
        parts.join("\n")
    }

    fn select_all(&mut self) {
        let Some((_, l)) = &self.layout else { return };
        let texts: Vec<usize> = l
            .elements
            .iter()
            .enumerate()
            .filter(|(_, e)| matches!(e, Element::Text { sel: true, .. }))
            .map(|(i, _)| i)
            .collect();
        let (Some(&first), Some(&last)) = (texts.first(), texts.last()) else {
            return;
        };
        let Element::Text { buffer, .. } = &l.elements[last] else {
            return;
        };
        let end_line = buffer.lines.len().saturating_sub(1);
        let end_idx = buffer.lines.last().map_or(0, |b| b.text().len());
        self.sel = Some((
            (first, Cursor::new(0, 0)),
            (last, Cursor::new(end_line, end_idx)),
        ));
    }

    fn preview_stale(&self) -> bool {
        self.editor.doc.version() != self.md_version
    }

    /// In split mode, large documents refresh only after typing pauses.
    fn live_ready(&self) -> bool {
        self.mode != Mode::Split
            || self.editor.doc.buf.line_count() * 40 < LIVE_DEBOUNCE_MIN_BYTES
            || self.last_edit.elapsed() >= LIVE_DEBOUNCE
    }

    fn split_rects(rect: Rect, ctx: &Ctx) -> (Rect, Rect, Rect) {
        let div = ctx.px(1.0).max(1.0) as i32;
        let lw = (rect.w - div) / 2;
        (
            Rect::new(rect.x, rect.y, lw, rect.h),
            Rect::new(rect.x + lw, rect.y, div, rect.h),
            Rect::new(rect.x + lw + div, rect.y, rect.w - lw - div, rect.h),
        )
    }

    /// Keep the block containing the editor's caret visible in the preview.
    fn follow_caret(&mut self) {
        let line = self.editor.doc.primary().head.line;
        if line == self.followed_line {
            return;
        }
        self.followed_line = line;
        let Some((_, l)) = &self.layout else { return };
        let byte = self
            .md_text
            .match_indices('\n')
            .nth(line.saturating_sub(1))
            .map(|(i, _)| i + 1)
            .filter(|_| line > 0)
            .unwrap_or(0);
        let y = l
            .offsets
            .iter()
            .take_while(|(o, _)| *o <= byte)
            .last()
            .map(|(_, y)| *y)
            .unwrap_or(0.0);
        let vh = self.last_rect.h as f32;
        if y < self.scroll.y + vh * 0.1 || y > self.scroll.y + vh * 0.8 {
            self.scroll.y = (y - vh / 3.0).max(0.0);
            self.clamp();
        }
    }

    /// Re-parse the preview if the text changed since the last parse.
    fn sync_preview(&mut self) {
        if self.editor.doc.version() == self.md_version {
            return;
        }
        self.md_text = crate::text::sanitize(&self.editor.doc.buf.to_text_lf()).into_owned();
        self.md = parse(&self.md_text);
        self.md_version = self.editor.doc.version();
        self.layout = None;
        self.scaled.clear();
        self.request_images();
    }

    fn ensure_layout(&mut self, width: i32, ctx: &mut Ctx) -> &Layout {
        let k = ctx.scale * ctx.zoom;
        let key = (width, k.to_bits(), self.images_version);
        if self.layout.as_ref().map(|(k, _)| *k) != Some(key) {
            let images = self.request_images();
            let mut lctx = LayoutCtx {
                images: &images,
                sans: ctx.fonts.sans.clone(),
                mono: ctx.fonts.mono.clone(),
                fonts: &mut ctx.fonts.system,
                k,
            };
            let l = layout(&self.md, &mut lctx, width as f32);
            // Keep a selection across relayouts that only change sizes (image
            // loaded, window resized): same elements, same order.
            let same_shape = self
                .layout
                .as_ref()
                .is_some_and(|(_, old)| old.elements.len() == l.elements.len());
            if !same_shape {
                self.sel = None;
            }
            self.layout = Some((key, l));
        }
        &self.layout.as_ref().expect("just laid out").1
    }

    fn content_height(&self) -> f32 {
        self.layout.as_ref().map(|(_, l)| l.height).unwrap_or(0.0)
    }

    fn clamp(&mut self) {
        let h = self.content_height();
        let (w, vh) = (self.last_rect.w as f32, self.last_rect.h as f32);
        self.scroll.clamp(w, h, w, vh);
        self.scroll.x = 0.0;
    }

    /// Link index under a window-space point, if any.
    /// The task marker offset of the checkbox under the mouse (with a few
    /// pixels of slack around the box).
    fn checkbox_at(&self, px: f32, py: f32) -> Option<usize> {
        let (_, l) = self.layout.as_ref()?;
        let dx = self.last_rect.x as f32;
        let dy = self.last_rect.y as f32 - self.scroll.y;
        let (x, y) = (px - dx, py - dy);
        l.elements.iter().find_map(|el| match el {
            Element::Checkbox {
                x: bx,
                y: by,
                size,
                offset: Some(off),
                ..
            } if x >= bx - 4.0 && x < bx + size + 4.0 && y >= by - 4.0 && y < by + size + 4.0 => {
                Some(*off)
            }
            _ => None,
        })
    }

    /// Tick or untick the task whose marker is at byte `offset` of the
    /// preview text. The preview text is the buffer with line endings and
    /// unsafe characters replaced one for one, so the marker is found by
    /// line and character position, and the edit only happens if the buffer
    /// really has `[ ]` / `[x]` there. One undo step.
    fn toggle_task(&mut self, offset: usize) -> bool {
        if self.preview_stale() || self.editor.is_read_only() {
            return false;
        }
        let text = &self.md_text;
        if offset > text.len() || !text.is_char_boundary(offset) {
            return false;
        }
        let line = text.as_bytes()[..offset]
            .iter()
            .filter(|b| **b == b'\n')
            .count();
        let line_start = text[..offset].rfind('\n').map_or(0, |i| i + 1);
        let chars_before = text[line_start..offset].chars().count();
        let buf = &self.editor.doc.buf;
        if line >= buf.line_count() {
            return false;
        }
        let src = buf.line(line);
        let Some((col, _)) = src.char_indices().nth(chars_before) else {
            return false;
        };
        let new = match src.get(col..col + 3) {
            Some("[ ]") => "x",
            Some("[x]") | Some("[X]") => " ",
            _ => return false,
        };
        use crate::editor::buffer::Pos;
        self.editor.doc.replace_ranges(vec![(
            Pos::new(line, col + 1),
            Pos::new(line, col + 2),
            new.to_string(),
        )]);
        self.editor.text_changed(line);
        self.sync_preview();
        true
    }

    fn link_at(&self, px: f32, py: f32) -> Option<usize> {
        let (_, l) = self.layout.as_ref()?;
        let dx = self.last_rect.x as f32;
        let dy = self.last_rect.y as f32 - self.scroll.y;
        let (x, y) = (px - dx, py - dy);
        for el in &l.elements {
            match el {
                Element::Text {
                    x: ex,
                    y: ey,
                    h,
                    buffer,
                    ..
                } => {
                    if y < *ey || y >= ey + h {
                        continue;
                    }
                    for run in buffer.layout_runs() {
                        let top = ey + run.line_top;
                        if y < top || y >= top + run.line_height {
                            continue;
                        }
                        for g in run.glyphs {
                            let gx = ex + g.x;
                            if x >= gx && x < gx + g.w {
                                return link_from_meta(g.metadata);
                            }
                        }
                    }
                }
                Element::Image {
                    x: ex,
                    y: ey,
                    w,
                    h,
                    link,
                    ..
                } => {
                    if x >= *ex && x < ex + w && y >= *ey && y < ey + h {
                        return *link;
                    }
                }
                _ => {}
            }
        }
        None
    }

    fn activate_link(&mut self, idx: usize, ctx: &mut Ctx) {
        let Some(dest) = self.md.links.get(idx).cloned() else {
            return;
        };
        match classify(&dest, &self.base_dir()) {
            LinkTarget::Web(url) => {
                ctx.requests.push(Request::OpenUrl(url));
                self.notice = None;
            }
            LinkTarget::Anchor(slug) => {
                if let Some((_, l)) = &self.layout
                    && let Some((_, y)) = l.anchors.iter().find(|(s, _)| *s == slug)
                {
                    self.scroll.y = (*y - ctx.px(12.0)).max(0.0);
                    self.clamp();
                }
            }
            LinkTarget::File(p) => {
                if p.is_file() {
                    ctx.requests.push(Request::OpenPath(p));
                } else {
                    self.notice = Some(format!("Link target not found: {}", p.display()));
                }
            }
            LinkTarget::Blocked(why) => self.notice = Some(format!("Link blocked ({why}): {dest}")),
        }
    }

    fn toggle_mode(&mut self, ctx: &mut Ctx) {
        match self.mode {
            Mode::Preview => {
                // Preview → source: keep the block at the top of the view.
                let line = self.top_source_line();
                self.editor.scroll_to_line(line, ctx);
                self.mode = Mode::Source;
            }
            Mode::Source | Mode::Split => {
                let line = self.editor.top_line(ctx);
                self.sync_preview();
                // Lay out now so the scroll position can be mapped.
                let w = self.last_rect.w.max(1);
                self.ensure_layout(w, ctx);
                self.scroll_to_source_line(line);
                self.mode = Mode::Preview;
            }
        }
    }

    fn top_source_line(&self) -> usize {
        let Some((_, l)) = &self.layout else { return 0 };
        let off = l
            .offsets
            .iter()
            .find(|(_, y)| *y >= self.scroll.y - 1.0)
            .map(|(o, _)| *o)
            .unwrap_or(0);
        let off = off.min(self.md_text.len());
        self.md_text.as_bytes()[..off]
            .iter()
            .filter(|b| **b == b'\n')
            .count()
    }

    fn scroll_to_source_line(&mut self, line: usize) {
        let Some((_, l)) = &self.layout else { return };
        let byte = self
            .md_text
            .match_indices('\n')
            .nth(line.saturating_sub(1))
            .map(|(i, _)| i + 1)
            .filter(|_| line > 0)
            .unwrap_or(0);
        let y = l
            .offsets
            .iter()
            .take_while(|(o, _)| *o <= byte)
            .last()
            .map(|(_, y)| *y)
            .unwrap_or(0.0);
        self.scroll.y = y;
    }

    /// Draw the previous layout (the text changed but the debounce has not
    /// expired yet).
    fn render_preview_stale(&mut self, canvas: &mut Canvas, rect: Rect, ctx: &mut Ctx) {
        let v = self.md_version;
        self.md_version = self.editor.doc.version(); // pretend fresh for this frame
        self.render_preview(canvas, rect, ctx);
        self.md_version = v;
    }

    fn render_preview(&mut self, canvas: &mut Canvas, rect: Rect, ctx: &mut Ctx) {
        self.sync_preview();
        canvas.fill_rect(rect, theme::p().background);
        self.ensure_layout(rect.w, ctx);
        if let Some(line) = self.pending_scroll_line.take() {
            self.scroll_to_source_line(line);
        }
        self.clamp();
        let dx = rect.x as f32;
        let dy = rect.y as f32 - self.scroll.y;
        let view_top = self.scroll.y;
        let view_bottom = self.scroll.y + rect.h as f32;
        let hover = self.hover_link;
        let selection = self.sel_range();
        let Some((_, layout)) = self.layout.as_mut() else {
            return;
        };

        for (ei, el) in layout.elements.iter_mut().enumerate() {
            let (y0, y1) = el.y_range();
            if y1 < view_top || y0 > view_bottom {
                continue;
            }
            match el {
                Element::Fill { x, y, w, h, color } => {
                    canvas.fill_rect(frect(*x + dx, *y + dy, *w, *h), *color);
                }
                Element::Text {
                    x,
                    y,
                    buffer,
                    color,
                    sel: selectable,
                    ..
                } => {
                    let (tx, ty) = (*x + dx, *y + dy);
                    // Selection highlight.
                    if let Some((lo, hi)) = selection
                        && ei >= lo.0
                        && ei <= hi.0
                        && *selectable
                    {
                        let start = if ei == lo.0 { lo.1 } else { Cursor::new(0, 0) };
                        let end = if ei == hi.0 {
                            hi.1
                        } else {
                            let last = buffer.lines.len().saturating_sub(1);
                            Cursor::new(last, buffer.lines.last().map_or(0, |b| b.text().len()))
                        };
                        for run in buffer.layout_runs() {
                            for (hx, hw) in run.highlight(start, end) {
                                canvas.fill_rect(
                                    frect(tx + hx, ty + run.line_top, hw.max(2.0), run.line_height),
                                    theme::p().selection,
                                );
                            }
                        }
                    }
                    // Inline code backgrounds and link hover underline.
                    for run in buffer.layout_runs() {
                        let mut span: Option<(f32, f32)> = None;
                        let top = ty + run.line_top;
                        if top > rect.bottom() as f32 {
                            break;
                        }
                        if top + run.line_height < rect.y as f32 {
                            continue;
                        }
                        let flush = |canvas: &mut Canvas, s: Option<(f32, f32)>| {
                            if let Some((a, b)) = s {
                                let pad = 3.0;
                                canvas.fill_rect(
                                    frect(
                                        tx + a - pad,
                                        top + 2.0,
                                        b - a + pad * 2.0,
                                        run.line_height - 4.0,
                                    ),
                                    theme::p().md_code_bg,
                                );
                            }
                        };
                        for g in run.glyphs {
                            if g.metadata & META_CODE != 0 {
                                span = Some(match span {
                                    Some((a, _)) => (a, g.x + g.w),
                                    None => (g.x, g.x + g.w),
                                });
                            } else {
                                flush(canvas, span.take());
                            }
                            if hover.is_some() && link_from_meta(g.metadata) == hover {
                                canvas.fill_rect(
                                    frect(tx + g.x, top + 1.0, g.w, run.line_height - 2.0),
                                    theme::p().md_link.with_alpha(28),
                                );
                            }
                        }
                        flush(canvas, span);
                    }
                    ctx.fonts.draw_buffer(
                        canvas,
                        buffer,
                        tx.round() as i32,
                        ty.round() as i32,
                        *color,
                    );
                }
                Element::Checkbox {
                    x,
                    y,
                    size,
                    checked,
                    ..
                } => {
                    let r = frect(*x + dx, *y + dy, *size, *size);
                    let b = ctx.px(1.5).max(1.0) as i32;
                    if *checked {
                        canvas.fill_rect(r, theme::p().md_link);
                        let (bx, by, s) = (*x + dx, *y + dy, *size);
                        let lw = (s * 0.14).max(1.5);
                        canvas.draw_line(
                            bx + s * 0.24,
                            by + s * 0.52,
                            bx + s * 0.42,
                            by + s * 0.70,
                            lw,
                            theme::p().background,
                        );
                        canvas.draw_line(
                            bx + s * 0.42,
                            by + s * 0.70,
                            bx + s * 0.76,
                            by + s * 0.32,
                            lw,
                            theme::p().background,
                        );
                    } else {
                        canvas.fill_rect(r, theme::p().md_muted);
                        canvas.fill_rect(
                            Rect::new(r.x + b, r.y + b, r.w - 2 * b, r.h - 2 * b),
                            theme::p().background,
                        );
                    }
                }
                Element::Image {
                    x,
                    y,
                    w,
                    h,
                    alt,
                    path,
                    note,
                    ..
                } => {
                    let r = frect(*x + dx, *y + dy, *w, *h);
                    let ready = path.as_ref().and_then(|p| match self.images.get(p) {
                        ImgState::Ready(d) => Some((p.clone(), d)),
                        _ => None,
                    });
                    if let Some((p, d)) = ready {
                        let key = (p, r.w.max(1) as u32, r.h.max(1) as u32);
                        if !self.scaled.contains_key(&key) {
                            if self.scaled.len() > 32 {
                                self.scaled.clear();
                            }
                            let px = scale_to(&d, key.1, key.2);
                            self.scaled.insert(key.clone(), px);
                        }
                        canvas.draw_rgba(r.x, r.y, key.1 as i32, key.2 as i32, &self.scaled[&key]);
                    } else {
                        canvas.fill_rect(r, theme::p().md_border);
                        canvas.fill_rect(
                            Rect::new(r.x + 1, r.y + 1, r.w - 2, r.h - 2),
                            theme::p().md_block_bg,
                        );
                        let what = if alt.is_empty() {
                            "image".to_string()
                        } else {
                            alt.clone()
                        };
                        let label = match note {
                            Some(n) => format!("{what} — {n}"),
                            None => what,
                        };
                        let lh = ctx.ui_metrics().line_height as i32;
                        let pad = ctx.px(10.0) as i32;
                        ctx.label(
                            canvas,
                            &label,
                            r.x + pad,
                            r.y + (r.h - lh) / 2,
                            theme::p().md_muted,
                            r.w - pad * 2,
                            FontKind::Ui,
                        );
                    }
                }
            }
        }
        let h = layout.height;
        draw_vscrollbar(canvas, rect, self.scroll.y, h, ctx);
    }
}

fn frect(x: f32, y: f32, w: f32, h: f32) -> Rect {
    let (x0, y0) = (x.round() as i32, y.round() as i32);
    Rect::new(
        x0,
        y0,
        ((x + w).round() as i32 - x0).max(0),
        ((y + h).round() as i32 - y0).max(0),
    )
}

impl View for MarkdownView {
    fn render(&mut self, canvas: &mut Canvas, rect: Rect, ctx: &mut Ctx) {
        self.last_rect = rect;
        match self.mode {
            Mode::Preview => self.render_preview(canvas, rect, ctx),
            Mode::Source => self.editor.render(canvas, rect, ctx),
            Mode::Split => {
                let (left, div, right) = Self::split_rects(rect, ctx);
                canvas.set_clip(left);
                self.editor.render(canvas, left, ctx);
                canvas.set_clip(div);
                canvas.fill_rect(div, theme::p().divider);
                canvas.set_clip(right);
                self.last_rect = right;
                // Large documents refresh once typing pauses (tick redraws).
                if !self.preview_stale() || self.live_ready() || self.layout.is_none() {
                    self.render_preview(canvas, right, ctx);
                    self.follow_caret();
                } else {
                    self.render_preview_stale(canvas, right, ctx);
                }
            }
        }
    }

    fn handle(&mut self, input: &Input, rect: Rect, ctx: &mut Ctx) -> bool {
        self.last_rect = rect;
        if let Input::Key {
            key: Key::Char('e'),
            mods,
        } = input
            && mods.ctrl
            && !mods.shift
        {
            self.toggle_mode(ctx);
            *ctx.cursor = MouseCursor::Default;
            return true;
        }
        // Ctrl+Shift+V: side-by-side live preview on/off.
        if let Input::Key {
            key: Key::Char('v'),
            mods,
        } = input
            && mods.ctrl
            && mods.shift
        {
            self.mode = if self.mode == Mode::Split {
                Mode::Source
            } else {
                Mode::Split
            };
            self.followed_line = usize::MAX;
            return true;
        }
        if let Input::MouseMove { x, .. } = input {
            self.mouse_x = *x;
        }
        if self.mode == Mode::Split {
            let (left, _, right) = Self::split_rects(rect, ctx);
            let over_preview = match input {
                Input::MouseMove { x, .. }
                | Input::MouseDown { x, .. }
                | Input::MouseUp { x, .. } => *x >= right.x as f32,
                Input::Wheel { .. } => self.mouse_x >= right.x as f32,
                _ => false,
            };
            if !over_preview {
                let v = self.editor.doc.version();
                let r = self.editor.handle(input, left, ctx);
                if self.editor.doc.version() != v {
                    self.last_edit = std::time::Instant::now();
                }
                return r;
            }
            self.last_rect = right;
            return self.handle_preview(input, right, ctx);
        }
        // Find / replace work on the source: switch to editing first.
        if self.mode == Mode::Preview
            && let Input::Key { key, mods } = input
            && ((mods.ctrl && matches!(key, Key::Char('f') | Key::Char('h')))
                || matches!(key, Key::F(3)))
        {
            self.toggle_mode(ctx);
        }
        if self.mode == Mode::Source {
            return self.editor.handle(input, rect, ctx);
        }
        self.handle_preview(input, rect, ctx)
    }

    fn tick(&mut self, ctx: &mut Ctx, budget: Duration) -> Tick {
        let mut t = match self.mode {
            Mode::Source | Mode::Split => self.editor.tick(ctx, budget),
            Mode::Preview => Tick::default(),
        };
        if self.mode == Mode::Split && self.preview_stale() {
            if self.live_ready() {
                t.redraw = true;
            } else {
                t.more = true; // come back when the debounce expires
            }
        }
        if self.images.poll() {
            self.images_version += 1;
            t.redraw |= self.mode != Mode::Source;
        }
        t.more |= self.images.pending();
        t
    }

    fn title(&self) -> String {
        self.editor.title()
    }

    fn status(&self) -> Vec<String> {
        let mut v = match self.mode {
            Mode::Preview => vec!["Markdown preview".to_string(), "Ctrl+E: edit".to_string()],
            Mode::Split => {
                let mut s = self.editor.status();
                s.insert(0, "Markdown side by side".into());
                s.insert(1, "Ctrl+Shift+V: single view".into());
                s
            }
            Mode::Source => {
                let mut s = self.editor.status();
                s.insert(0, "Markdown source".into());
                s.insert(1, "Ctrl+E: preview".into());
                s
            }
        };
        if let Some(n) = &self.notice {
            v.push(n.clone());
        }
        v
    }

    fn path(&self) -> Option<&Path> {
        self.editor.path()
    }

    fn reload(&mut self, ctx: &mut Ctx) -> bool {
        let changed = self.editor.reload(ctx);
        if changed {
            self.images.clear();
            self.sync_preview();
        }
        changed
    }

    fn metrics_changed(&mut self) {
        self.layout = None;
        self.editor.metrics_changed();
    }

    fn theme_changed(&mut self) {
        self.layout = None;
        self.editor.theme_changed();
    }

    fn has_background_work(&self) -> bool {
        self.images.pending()
    }

    fn on_hide(&mut self) {
        self.layout = None;
        self.scaled = HashMap::new();
        self.images.unload();
        self.editor.on_hide();
    }

    #[cfg(test)]
    fn scroll_y(&self) -> f32 {
        self.scroll.y
    }

    #[cfg(test)]
    fn checkboxes(&self) -> Vec<(f32, f32)> {
        let Some((_, l)) = &self.layout else {
            return Vec::new();
        };
        let dx = self.last_rect.x as f32;
        let dy = self.last_rect.y as f32 - self.scroll.y;
        l.elements
            .iter()
            .filter_map(|e| match e {
                Element::Checkbox { x, y, size, .. } => {
                    Some((x + dx + size / 2.0, y + dy + size / 2.0))
                }
                _ => None,
            })
            .collect()
    }

    #[cfg(test)]
    fn has_caches(&self) -> bool {
        self.layout.is_some()
            || !self.scaled.is_empty()
            || self.images.loaded() > 0
            || self.editor.has_caches()
    }

    fn is_dirty(&self) -> bool {
        self.editor.is_dirty()
    }

    fn text(&self) -> Option<String> {
        self.editor.text()
    }

    fn headings(&mut self) -> Option<Vec<(u8, String, usize)>> {
        self.sync_preview();
        let text = self.md_text.as_bytes();
        Some(
            crate::markdown::parse::headings(&self.md)
                .into_iter()
                .map(|(level, title, off)| {
                    let off = off.min(text.len());
                    let line = text[..off].iter().filter(|b| **b == b'\n').count();
                    (level, title, line)
                })
                .collect(),
        )
    }

    fn goto_heading(&mut self, line: usize) {
        if self.mode != Mode::Preview {
            self.editor.goto_line(line + 1);
        }
        if self.mode != Mode::Source {
            // The layout may not exist yet (hidden tab, resized window):
            // resolved at the next draw.
            self.pending_scroll_line = Some(line);
        }
    }

    fn run_command(&mut self, name: &str) -> bool {
        let done = self.editor.run_command(name);
        if done {
            self.layout = None;
        }
        done
    }

    fn save(&mut self, allow_lossy: bool) -> Result<usize, SaveError> {
        self.editor.save(allow_lossy)
    }

    fn save_as(&mut self, path: &Path, allow_lossy: bool) -> Result<usize, SaveError> {
        let n = self.editor.save_as(path, allow_lossy)?;
        self.layout = None;
        self.images.clear();
        self.request_images();
        Ok(n)
    }

    fn disk_state(&self) -> DiskState {
        self.editor.disk_state()
    }

    fn reload_discarding(&mut self, ctx: &mut Ctx) -> bool {
        let r = self.editor.reload_discarding(ctx);
        self.images.clear();
        self.sync_preview();
        r
    }

    fn mark_orphaned(&mut self) {
        self.editor.mark_orphaned();
    }

    fn line_count(&self) -> Option<usize> {
        self.editor.line_count()
    }

    fn goto_line(&mut self, line: usize) -> bool {
        if self.mode == Mode::Preview {
            self.mode = Mode::Source;
        }
        self.editor.goto_line(line)
    }

    fn caret_rect(&self) -> Option<Rect> {
        matches!(self.mode, Mode::Source | Mode::Split)
            .then(|| self.editor.caret_rect())
            .flatten()
    }

    #[cfg(test)]
    fn snapshot(&self) -> Option<(String, Vec<crate::editor::buffer::Pos>)> {
        self.editor.snapshot()
    }
}

impl MarkdownView {
    fn handle_preview(&mut self, input: &Input, rect: Rect, ctx: &mut Ctx) -> bool {
        // Hit-testing needs a layout even before the first frame.
        if matches!(
            input,
            Input::MouseDown { .. } | Input::MouseMove { .. } | Input::Key { .. }
        ) {
            self.sync_preview();
            self.ensure_layout(rect.w.max(1), ctx);
        }
        let lh = ctx.font_px(theme::md_font_size()) * theme::MD_LINE_HEIGHT;
        let before = self.scroll.y;
        match input {
            Input::Wheel { .. } => {
                self.scroll.wheel(input, lh);
            }
            Input::Key {
                key: Key::Char('a'),
                mods,
            } if mods.ctrl => {
                self.select_all();
                return true;
            }
            // Undo / redo work in the preview too (a ticked checkbox can be
            // taken back without switching to the source).
            Input::Key {
                key: k @ (Key::Char('z') | Key::Char('y')),
                mods,
            } if mods.ctrl && !mods.alt => {
                let redo = *k == Key::Char('y') || mods.shift;
                if self.editor.undo_redo(redo) {
                    self.sync_preview();
                }
                return true;
            }
            Input::Key {
                key: Key::Char('c'),
                mods,
            } if mods.ctrl => {
                let t = self.selected_text();
                if !t.is_empty() {
                    ctx.clipboard.set(t);
                }
                return false;
            }
            Input::Key {
                key: Key::Escape, ..
            } if self.sel.is_some() => {
                self.sel = None;
                return true;
            }
            Input::Key { key, mods } => {
                let page = (rect.h as f32 - lh * 2.0).max(lh);
                match key {
                    Key::Down => self.scroll.y += lh,
                    Key::Up => self.scroll.y -= lh,
                    Key::PageDown => self.scroll.y += page,
                    Key::PageUp => self.scroll.y -= page,
                    Key::Char(' ') if !mods.ctrl => {
                        self.scroll.y += if mods.shift { -page } else { page }
                    }
                    Key::Home => self.scroll.y = 0.0,
                    Key::End => self.scroll.y = f32::MAX,
                    _ => return false,
                }
            }
            Input::MouseMove { x, y } if self.selecting => {
                if let (Some(h), Some((a, _))) = (self.text_hit(*x, *y), self.sel) {
                    self.sel = Some((a, h));
                }
                *ctx.cursor = MouseCursor::Text;
                return true;
            }
            Input::MouseMove { x, y } => {
                let link = self.link_at(*x, *y);
                let changed = link != self.hover_link;
                self.hover_link = link;
                *ctx.cursor = if link.is_some() || self.checkbox_at(*x, *y).is_some() {
                    MouseCursor::Pointer
                } else {
                    MouseCursor::Default
                };
                return changed;
            }
            Input::MouseLeave => {
                let changed = self.hover_link.is_some();
                self.hover_link = None;
                *ctx.cursor = MouseCursor::Default;
                return changed;
            }
            Input::MouseDown {
                x,
                y,
                button: MouseButton::Left,
                clicks,
            } => {
                if let Some(idx) = self.link_at(*x, *y) {
                    self.activate_link(idx, ctx);
                    return true;
                }
                if let Some(off) = self.checkbox_at(*x, *y) {
                    self.toggle_task(off);
                    return true;
                }
                // Start a text selection (double-click: the whole block).
                let hit = self.text_hit(*x, *y);
                self.sel = hit.map(|h| (h, h));
                self.selecting = hit.is_some();
                if *clicks >= 2
                    && let (Some((i, _)), Some((_, l))) = (hit, self.layout.as_ref())
                    && let Element::Text { buffer, .. } = &l.elements[i]
                {
                    let end_line = buffer.lines.len().saturating_sub(1);
                    let end = buffer.lines.last().map_or(0, |b| b.text().len());
                    self.sel = Some(((i, Cursor::new(0, 0)), (i, Cursor::new(end_line, end))));
                    self.selecting = false;
                }
                return true;
            }
            Input::MouseUp { .. } => {
                self.selecting = false;
                return false;
            }
            _ => return false,
        }
        self.clamp();
        self.scroll.y != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::Canvas;
    use crate::text::Fonts;

    struct Harness {
        fonts: Fonts,
        requests: Vec<Request>,
        cursor: MouseCursor,
        clip: crate::clipboard::MemClipboard,
    }

    impl Harness {
        fn new() -> Self {
            Self {
                fonts: Fonts::load(),
                requests: Vec::new(),
                cursor: MouseCursor::Default,
                clip: crate::clipboard::MemClipboard::default(),
            }
        }
        fn ctx(&mut self) -> Ctx<'_> {
            Ctx {
                fonts: &mut self.fonts,
                scale: 1.0,
                zoom: 1.0,
                requests: &mut self.requests,
                cursor: &mut self.cursor,
                focused: true,
                clipboard: &mut self.clip,
                mods: crate::input::Mods::default(),
            }
        }
    }

    fn view(text: &str, dir: &Path) -> MarkdownView {
        let path = dir.join("doc.md");
        std::fs::write(&path, text).unwrap();
        MarkdownView::new(Document::open(&path).unwrap())
    }

    fn render(v: &mut MarkdownView, h: &mut Harness, w: u32, hgt: u32) {
        let mut buf = vec![0u32; (w * hgt) as usize];
        let mut c = Canvas::new(&mut buf, w, hgt);
        let rect = c.bounds();
        v.render(&mut c, rect, &mut h.ctx());
    }

    /// Window coordinates of the first glyph belonging to link `idx`.
    fn link_point(v: &MarkdownView, idx: usize) -> (f32, f32) {
        let (_, l) = v.layout.as_ref().unwrap();
        for el in &l.elements {
            if let Element::Text { x, y, buffer, .. } = el {
                for run in buffer.layout_runs() {
                    for g in run.glyphs {
                        if link_from_meta(g.metadata) == Some(idx) {
                            return (
                                x + g.x + g.w / 2.0,
                                y + run.line_top + run.line_height / 2.0 - v.scroll.y,
                            );
                        }
                    }
                }
            }
        }
        panic!("link {idx} not laid out");
    }

    fn click(v: &mut MarkdownView, h: &mut Harness, p: (f32, f32)) -> bool {
        let rect = v.last_rect;
        v.handle(&Input::MouseMove { x: p.0, y: p.1 }, rect, &mut h.ctx());
        v.handle(
            &Input::MouseDown {
                x: p.0,
                y: p.1,
                button: MouseButton::Left,
                clicks: 1,
            },
            rect,
            &mut h.ctx(),
        )
    }

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("litedit-md-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn links_open_safely() {
        let dir = tmpdir("links");
        std::fs::write(dir.join("other.md"), "# other").unwrap();
        let text = "[web](https://example.com) [bad](javascript:alert(1)) [unc](//evil/share/x.md) \
                    [file](other.md) [missing](nope.md) [jump](#far-away)\n\n"
            .to_string()
            + &"filler\n\n".repeat(200) + "## Far away\n";
        let mut v = view(&text, &dir);
        let mut h = Harness::new();
        render(&mut v, &mut h, 900, 600);

        assert!({
            let p = link_point(&v, 0);
            click(&mut v, &mut h, p)
        });
        assert_eq!(h.cursor, MouseCursor::Pointer);
        assert_eq!(
            h.requests.pop(),
            Some(Request::OpenUrl("https://example.com".into()))
        );

        {
            let p = link_point(&v, 1);
            click(&mut v, &mut h, p)
        };
        assert!(h.requests.is_empty());
        assert!(v.status().iter().any(|s| s.contains("blocked")));

        {
            let p = link_point(&v, 2);
            click(&mut v, &mut h, p)
        };
        assert!(h.requests.is_empty(), "network path must not be opened");

        {
            let p = link_point(&v, 3);
            click(&mut v, &mut h, p)
        };
        assert_eq!(
            h.requests.pop(),
            Some(Request::OpenPath(dir.join("other.md")))
        );

        {
            let p = link_point(&v, 4);
            click(&mut v, &mut h, p)
        };
        assert!(h.requests.is_empty());
        assert!(v.status().iter().any(|s| s.contains("not found")));

        let before = v.scroll.y;
        {
            let p = link_point(&v, 5);
            click(&mut v, &mut h, p)
        };
        assert!(
            v.scroll.y > before + 1000.0,
            "anchor should scroll far down"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ctrl_e_toggles_and_keeps_position() {
        let dir = tmpdir("toggle");
        let text: String = (0..300)
            .map(|i| format!("Paragraph number {i}.\n\n"))
            .collect();
        let mut v = view(&text, &dir);
        let mut h = Harness::new();
        render(&mut v, &mut h, 800, 600);
        let rect = v.last_rect;
        for _ in 0..10 {
            v.handle(
                &Input::Key {
                    key: Key::PageDown,
                    mods: Default::default(),
                },
                rect,
                &mut h.ctx(),
            );
        }
        render(&mut v, &mut h, 800, 600);
        let ctrl_e = Input::Key {
            key: Key::Char('e'),
            mods: crate::input::Mods {
                ctrl: true,
                ..Default::default()
            },
        };
        assert!(v.handle(&ctrl_e, rect, &mut h.ctx()));
        assert_eq!(v.status()[0], "Markdown source");
        render(&mut v, &mut h, 800, 600);
        let line = v.editor.top_line(&h.ctx());
        assert!(
            line > 100,
            "source should open near the same place, got line {line}"
        );
        assert!(v.handle(&ctrl_e, rect, &mut h.ctx()));
        assert_eq!(v.status()[0], "Markdown preview");
        assert!(v.scroll.y > 1000.0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn renders_at_tiny_sizes() {
        let dir = tmpdir("tiny");
        let src = std::fs::read_to_string(crate::tests::fixture("sample.md")).unwrap();
        let mut v = view(&src, &dir);
        let mut h = Harness::new();
        for (w, hh) in [(1, 1), (10, 10), (40, 40), (200, 100)] {
            render(&mut v, &mut h, w, hh);
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
