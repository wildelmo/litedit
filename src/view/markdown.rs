//! Markdown preview (the default for `.md` files), with `Ctrl+E` switching
//! to the source view. Scroll position is kept in sync across the switch.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::canvas::{Canvas, Rect};
use crate::doc::Document;
use crate::images::{ImageStore, ImgState, scale_to};
use crate::input::{Cursor, Input, Key, MouseButton, Request};
use crate::links::{LinkTarget, classify};
use crate::markdown::layout::{
    Element, ImgInfo, Layout, LayoutCtx, META_CODE, layout, link_from_meta,
};
use crate::markdown::parse::{Block, BlockKind, Inline, MdDoc, parse};
use crate::theme;
use crate::ui::{Ctx, FontKind};
use crate::view::code::CodeView;
use crate::view::{Scroll, Tick, View, draw_vscrollbar};

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
}

pub struct MarkdownView {
    doc: Document,
    md: MdDoc,
    /// Layout and the (width, scale, image version) it was made for.
    layout: Option<((i32, u32, u32), Layout)>,
    images: ImageStore,
    /// Bumped whenever an image finishes loading (forces relayout).
    images_version: u32,
    /// Scaled pixels per (path, w, h), for the current display size.
    scaled: HashMap<(PathBuf, u32, u32), Vec<u8>>,
    scroll: Scroll,
    mode: Mode,
    source: Option<CodeView>,
    hover_link: Option<usize>,
    last_rect: Rect,
    /// Message shown in the status bar (e.g. blocked link).
    notice: Option<String>,
}

impl MarkdownView {
    pub fn new(doc: Document) -> Self {
        let md = parse(&crate::text::sanitize(&doc.text));
        let mut v = Self {
            doc,
            md,
            layout: None,
            images: ImageStore::new(),
            images_version: 0,
            scaled: HashMap::new(),
            scroll: Scroll::default(),
            mode: Mode::Preview,
            source: None,
            hover_link: None,
            last_rect: Rect::new(0, 0, 0, 0),
            notice: None,
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
        self.doc
            .path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default()
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
                let doc = self.doc.clone();
                let src = self.source.get_or_insert_with(|| {
                    let mut v = CodeView::new(doc);
                    v.read_only = true; // editable from S2.7
                    v
                });
                src.scroll_to_line(line, ctx);
                self.mode = Mode::Source;
            }
            Mode::Source => {
                if let Some(src) = &self.source {
                    let line = src.top_line(ctx);
                    self.scroll_to_source_line(line);
                }
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
        let off = off.min(self.doc.text.len());
        self.doc.text.as_bytes()[..off]
            .iter()
            .filter(|b| **b == b'\n')
            .count()
    }

    fn scroll_to_source_line(&mut self, line: usize) {
        let Some((_, l)) = &self.layout else { return };
        let byte = self
            .doc
            .text
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

    fn render_preview(&mut self, canvas: &mut Canvas, rect: Rect, ctx: &mut Ctx) {
        canvas.fill_rect(rect, theme::BACKGROUND);
        self.ensure_layout(rect.w, ctx);
        self.clamp();
        let dx = rect.x as f32;
        let dy = rect.y as f32 - self.scroll.y;
        let view_top = self.scroll.y;
        let view_bottom = self.scroll.y + rect.h as f32;
        let hover = self.hover_link;
        let Some((_, layout)) = self.layout.as_mut() else {
            return;
        };

        for el in layout.elements.iter_mut() {
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
                    ..
                } => {
                    let (tx, ty) = (*x + dx, *y + dy);
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
                                    theme::MD_CODE_BG,
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
                                    theme::MD_LINK.with_alpha(28),
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
                } => {
                    let r = frect(*x + dx, *y + dy, *size, *size);
                    let b = ctx.px(1.5).max(1.0) as i32;
                    if *checked {
                        canvas.fill_rect(r, theme::MD_LINK);
                        let (bx, by, s) = (*x + dx, *y + dy, *size);
                        let lw = (s * 0.14).max(1.5);
                        canvas.draw_line(
                            bx + s * 0.24,
                            by + s * 0.52,
                            bx + s * 0.42,
                            by + s * 0.70,
                            lw,
                            theme::BACKGROUND,
                        );
                        canvas.draw_line(
                            bx + s * 0.42,
                            by + s * 0.70,
                            bx + s * 0.76,
                            by + s * 0.32,
                            lw,
                            theme::BACKGROUND,
                        );
                    } else {
                        canvas.fill_rect(r, theme::MD_MUTED);
                        canvas.fill_rect(
                            Rect::new(r.x + b, r.y + b, r.w - 2 * b, r.h - 2 * b),
                            theme::BACKGROUND,
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
                        canvas.fill_rect(r, theme::MD_BORDER);
                        canvas.fill_rect(
                            Rect::new(r.x + 1, r.y + 1, r.w - 2, r.h - 2),
                            theme::MD_BLOCK_BG,
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
                            theme::MD_MUTED,
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
            Mode::Source => {
                if let Some(src) = self.source.as_mut() {
                    src.render(canvas, rect, ctx);
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
            *ctx.cursor = Cursor::Default;
            return true;
        }
        if self.mode == Mode::Source {
            return self
                .source
                .as_mut()
                .is_some_and(|s| s.handle(input, rect, ctx));
        }
        let lh = ctx.font_px(theme::MD_FONT_SIZE) * theme::MD_LINE_HEIGHT;
        let before = self.scroll.y;
        match input {
            Input::Wheel { .. } => {
                self.scroll.wheel(input, lh);
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
            Input::MouseMove { x, y } => {
                let link = self.link_at(*x, *y);
                let changed = link != self.hover_link;
                self.hover_link = link;
                *ctx.cursor = if link.is_some() {
                    Cursor::Pointer
                } else {
                    Cursor::Default
                };
                return changed;
            }
            Input::MouseLeave => {
                let changed = self.hover_link.is_some();
                self.hover_link = None;
                *ctx.cursor = Cursor::Default;
                return changed;
            }
            Input::MouseDown {
                x,
                y,
                button: MouseButton::Left,
                ..
            } => {
                if let Some(idx) = self.link_at(*x, *y) {
                    self.activate_link(idx, ctx);
                    return true;
                }
                return false;
            }
            _ => return false,
        }
        self.clamp();
        self.scroll.y != before
    }

    fn tick(&mut self, ctx: &mut Ctx, budget: Duration) -> Tick {
        let mut t = match (self.mode, self.source.as_mut()) {
            (Mode::Source, Some(src)) => src.tick(ctx, budget),
            _ => Tick::default(),
        };
        if self.images.poll() {
            self.images_version += 1;
            t.redraw |= self.mode == Mode::Preview;
        }
        t.more |= self.images.pending();
        t
    }

    fn title(&self) -> String {
        self.doc.file_name()
    }

    fn status(&self) -> Vec<String> {
        let mut v = match self.mode {
            Mode::Preview => vec!["Markdown preview".to_string(), "Ctrl+E: source".to_string()],
            Mode::Source => {
                let mut s = self.source.as_ref().map(|s| s.status()).unwrap_or_default();
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
        Some(&self.doc.path)
    }

    fn reload(&mut self, ctx: &mut Ctx) -> bool {
        match Document::open(&self.doc.path) {
            Ok(d) if d.text != self.doc.text => {
                self.md = parse(&crate::text::sanitize(&d.text));
                self.doc = d;
                self.layout = None;
                self.images.clear();
                self.scaled.clear();
                self.request_images();
                if let Some(src) = self.source.as_mut() {
                    src.reload(ctx);
                }
                true
            }
            _ => false,
        }
    }

    fn metrics_changed(&mut self) {
        self.layout = None;
        if let Some(s) = self.source.as_mut() {
            s.metrics_changed();
        }
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
        cursor: Cursor,
        clip: crate::clipboard::MemClipboard,
    }

    impl Harness {
        fn new() -> Self {
            Self {
                fonts: Fonts::load(),
                requests: Vec::new(),
                cursor: Cursor::Default,
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
        assert_eq!(h.cursor, Cursor::Pointer);
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
        let line = v.source.as_ref().unwrap().top_line(&h.ctx());
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
