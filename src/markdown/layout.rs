//! Lay out a parsed markdown document into positioned, shaped elements.
//!
//! The result is a flat display list with document-relative coordinates, so
//! rendering is a simple cull-and-draw loop and hit testing is a scan.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use cosmic_text::{
    Align as CAlign, Attrs, Buffer, Family, FontSystem, Metrics, Shaping, Style, Weight, Wrap,
};

use crate::canvas::Color;
use crate::markdown::parse::{Align, Block, BlockKind, Callout, Inline, ListItem, MdDoc};
use crate::syntax::{LineHighlighter, find_syntax_by_token};
use crate::theme;
use crate::view::code::styled_pieces;

/// Preview limits (hostile or generated files); Ctrl+E shows everything.
pub const MAX_CODE_BLOCK_LINES: usize = 3000;
pub const CODE_CHUNK_LINES: usize = 64;
pub const MAX_PARAGRAPH_CHARS: usize = 30_000;
pub const MAX_TABLE_ROWS: usize = 2000;
pub const MAX_TABLE_COLS: usize = 64;

/// Truncate rich-text pieces to about `max` characters in total.
fn cap_pieces(pieces: &mut Vec<(String, Attrs)>, max: usize) {
    let mut total = 0;
    for i in 0..pieces.len() {
        let n = pieces[i].0.chars().count();
        if total + n > max {
            let keep = max - total;
            let cut = pieces[i]
                .0
                .char_indices()
                .nth(keep)
                .map(|(b, _)| b)
                .unwrap_or(pieces[i].0.len());
            pieces[i].0.truncate(cut);
            pieces[i].0.push_str(" …");
            pieces.truncate(i + 1);
            return;
        }
        total += n;
    }
}

/// Glyph metadata bit for inline code (background box).
pub const META_CODE: usize = 1;

/// Encode a link index into glyph metadata.
pub fn meta_link(idx: usize) -> usize {
    (idx + 1) << 1
}

/// Decode a link index from glyph metadata.
pub fn link_from_meta(meta: usize) -> Option<usize> {
    (meta >> 1).checked_sub(1)
}

pub enum Element {
    Text {
        x: f32,
        y: f32,
        h: f32,
        buffer: Buffer,
        color: Color,
        /// Part of selectable / copyable text (false for list bullets).
        sel: bool,
    },
    Fill {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: Color,
    },
    Checkbox {
        x: f32,
        y: f32,
        size: f32,
        checked: bool,
        /// Source byte offset of its `[ ]` marker.
        offset: Option<usize>,
    },
    /// An image (or its placeholder when it cannot be shown).
    Image {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        alt: String,
        link: Option<usize>,
        path: Option<PathBuf>,
        note: Option<String>,
    },
}

impl Element {
    pub fn y_range(&self) -> (f32, f32) {
        match self {
            Element::Text { y, h, .. } => (*y, y + h),
            Element::Fill { y, h, .. } | Element::Image { y, h, .. } => (*y, y + h),
            Element::Checkbox { y, size, .. } => (*y, y + size),
        }
    }
}

#[derive(Default)]
pub struct Layout {
    pub elements: Vec<Element>,
    pub height: f32,
    /// (heading slug, y) for `#anchor` links.
    pub anchors: Vec<(String, f32)>,
    /// (source byte offset, y) for every block, in document order.
    pub offsets: Vec<(usize, f32)>,
    /// Some text is not shown (very long code blocks are shortened).
    pub truncated: bool,
}

/// What layout knows about an image source.
#[derive(Clone, Debug, PartialEq)]
pub enum ImgInfo {
    /// Loaded; natural size in pixels.
    Size { w: u32, h: u32, path: PathBuf },
    /// Not shown, with the reason.
    Note(String),
}

pub struct LayoutCtx<'a> {
    pub fonts: &'a mut FontSystem,
    /// Image sources (as written in the markdown) → what we know.
    pub images: &'a HashMap<String, ImgInfo>,
    pub sans: String,
    pub mono: String,
    /// Physical pixels per logical pixel, including zoom.
    pub k: f32,
}

#[derive(Clone, Copy, PartialEq)]
enum Tone {
    Normal,
    Muted,
}

impl LayoutCtx<'_> {
    fn px(&self, v: f32) -> f32 {
        (v * self.k).round()
    }

    fn body(&self) -> Metrics {
        let s = self.px(theme::md_font_size()).max(4.0);
        Metrics::new(s, (s * theme::MD_LINE_HEIGHT).round())
    }

    fn code(&self) -> Metrics {
        let s = self.px(theme::md_code_size()).max(4.0);
        Metrics::new(s, (s * 1.45).round())
    }

    fn heading(&self, level: u8) -> Metrics {
        let f = [2.0, 1.55, 1.3, 1.12, 1.0, 0.92][(level.clamp(1, 6) - 1) as usize];
        let s = self.px(theme::md_font_size() * f).max(4.0);
        Metrics::new(s, (s * 1.3).round())
    }
}

fn ccol(c: Color) -> cosmic_text::Color {
    cosmic_text::Color::rgb(c.r, c.g, c.b)
}

/// Height of a shaped buffer.
fn buffer_height(b: &Buffer) -> f32 {
    b.layout_runs()
        .last()
        .map(|r| r.line_top + r.line_height)
        .unwrap_or(0.0)
}

fn buffer_width(b: &Buffer) -> f32 {
    b.layout_runs().map(|r| r.line_w).fold(0.0, f32::max)
}

/// Rich-text pieces for a run of inlines.
fn inline_pieces<'a>(
    inlines: &'a [Inline],
    mono: &'a str,
    base: &Attrs<'a>,
    tone: Tone,
) -> Vec<(String, Attrs<'a>)> {
    let text_color = if tone == Tone::Muted {
        theme::p().md_muted
    } else {
        theme::p().md_text
    };
    let mut out: Vec<(String, Attrs<'a>)> = Vec::new();
    for inl in inlines {
        match inl {
            Inline::Text { text, style, link } => {
                let mut a = base.clone().color(ccol(text_color));
                if style.bold {
                    a = a.weight(Weight::BOLD).color(ccol(theme::p().md_heading));
                }
                if style.italic {
                    a = a.style(Style::Italic);
                }
                let mut deco = cosmic_text::TextDecoration::new();
                if style.strike {
                    deco.strikethrough = true;
                }
                let mut meta = 0;
                if style.code {
                    a = a
                        .family(Family::Name(mono))
                        .color(ccol(theme::p().syntax_string));
                    meta |= META_CODE;
                }
                if let Some(l) = link {
                    a = a.color(ccol(theme::p().md_link));
                    deco.underline = cosmic_text::UnderlineStyle::Single;
                    meta |= meta_link(*l);
                }
                a.text_decoration = deco;
                a = a.metadata(meta);
                out.push((text.clone(), a));
            }
            Inline::Break => out.push(("\n".into(), base.clone())),
            Inline::Image { alt, link, .. } => {
                let mut a = base
                    .clone()
                    .color(ccol(theme::p().md_muted))
                    .style(Style::Italic);
                if let Some(l) = link {
                    a = a.metadata(meta_link(*l));
                }
                let label = if alt.is_empty() {
                    "[image]".to_string()
                } else {
                    format!("[image: {alt}]")
                };
                out.push((label, a));
            }
        }
    }
    out
}

fn shape_rich(
    ctx: &mut LayoutCtx,
    pieces: &[(String, Attrs)],
    base: &Attrs,
    m: Metrics,
    width: f32,
    align: Option<CAlign>,
) -> Buffer {
    let mut b = Buffer::new_empty(m);
    b.set_wrap(Wrap::WordOrGlyph);
    b.set_size(Some(width.max(1.0)), None);
    b.set_rich_text(
        pieces.iter().map(|(t, a)| (t.as_str(), a.clone())),
        base,
        Shaping::Advanced,
        align,
    );
    b.shape_until_scroll(ctx.fonts, false);
    b
}

struct Out {
    elements: Vec<Element>,
    anchors: Vec<(String, f32)>,
    offsets: Vec<(usize, f32)>,
    truncated: bool,
}

pub fn layout(doc: &MdDoc, ctx: &mut LayoutCtx, view_width: f32) -> Layout {
    let pad_x = ctx.px(40.0).min(view_width * 0.06).max(ctx.px(12.0));
    let pad_y = ctx.px(28.0);
    let max_w = ctx.px(theme::MD_MAX_WIDTH);
    let content_w = (view_width - pad_x * 2.0).clamp(ctx.px(40.0), max_w.max(ctx.px(40.0)));
    let x = ((view_width - content_w) / 2.0)
        .max(pad_x.min(view_width / 2.0))
        .floor();
    let mut out = Out {
        elements: Vec::new(),
        anchors: Vec::new(),
        offsets: Vec::new(),
        truncated: false,
    };
    let y = layout_blocks(
        &doc.blocks,
        ctx,
        &mut out,
        x,
        content_w,
        pad_y,
        0,
        Tone::Normal,
    );
    Layout {
        elements: out.elements,
        height: y + pad_y,
        anchors: out.anchors,
        offsets: out.offsets,
        truncated: out.truncated,
    }
}

#[allow(clippy::too_many_arguments)]
fn layout_blocks(
    blocks: &[Block],
    ctx: &mut LayoutCtx,
    out: &mut Out,
    x: f32,
    w: f32,
    mut y: f32,
    depth: usize,
    tone: Tone,
) -> f32 {
    let gap = ctx.px(14.0);
    for (i, block) in blocks.iter().enumerate() {
        if i > 0 {
            y += if depth > 0 && matches!(block.kind, BlockKind::List { .. }) {
                ctx.px(4.0)
            } else {
                gap
            };
        }
        out.offsets.push((block.offset, y));
        y = layout_block(block, ctx, out, x, w, y, depth, tone);
    }
    y
}

#[allow(clippy::too_many_arguments)]
fn layout_block(
    block: &Block,
    ctx: &mut LayoutCtx,
    out: &mut Out,
    x: f32,
    w: f32,
    y: f32,
    depth: usize,
    tone: Tone,
) -> f32 {
    let w = w.max(ctx.px(8.0));
    let sans = ctx.sans.clone();
    let mono = ctx.mono.clone();
    let base = Attrs::new().family(Family::Name(&sans));
    match &block.kind {
        BlockKind::Heading {
            level,
            inlines,
            slug,
        } => {
            let mut y = y;
            if out.elements.len() > 0 {
                y += ctx.px(if *level <= 2 { 14.0 } else { 8.0 });
            }
            let m = ctx.heading(*level);
            let hb = base
                .clone()
                .weight(Weight::BOLD)
                .color(ccol(theme::p().md_heading));
            let mut pieces = inline_pieces(inlines, &mono, &hb, tone);
            for (_, a) in pieces.iter_mut() {
                if a.color_opt == Some(ccol(theme::p().md_text)) {
                    *a = a.clone().color(ccol(theme::p().md_heading));
                }
                *a = a.clone().weight(Weight::BOLD);
            }
            let buf = shape_rich(ctx, &pieces, &hb, m, w, None);
            let h = buffer_height(&buf);
            out.anchors.push((slug.clone(), y));
            out.elements.push(Element::Text {
                x,
                y,
                h,
                buffer: buf,
                color: theme::p().md_heading,
                sel: true,
            });
            let mut y = y + h;
            if *level <= 2 {
                y += ctx.px(6.0);
                out.elements.push(Element::Fill {
                    x,
                    y,
                    w,
                    h: ctx.px(1.0).max(1.0),
                    color: theme::p().md_border,
                });
                y += ctx.px(2.0);
            }
            y
        }
        BlockKind::Paragraph(inlines) => {
            let only_images = inlines.iter().all(|i| match i {
                Inline::Image { .. } | Inline::Break => true,
                Inline::Text { text, .. } => text.trim().is_empty(),
            }) && inlines.iter().any(|i| matches!(i, Inline::Image { .. }));
            if only_images {
                let mut y = y;
                for (n, i) in inlines.iter().enumerate() {
                    if let Inline::Image { src, alt, link } = i {
                        if n > 0 {
                            y += ctx.px(8.0);
                        }
                        let (iw, ih, path, note) = match ctx.images.get(src) {
                            Some(ImgInfo::Size { w: nw, h: nh, path }) if *nw > 0 && *nh > 0 => {
                                let dw = (*nw as f32 * ctx.k).min(w).max(1.0).round();
                                let dh = (*nh as f32 * dw / *nw as f32).max(1.0).round();
                                (dw, dh, Some(path.clone()), None)
                            }
                            other => {
                                let note = match other {
                                    Some(ImgInfo::Note(n)) => n.clone(),
                                    _ => "loading…".to_string(),
                                };
                                (
                                    w.min(ctx.px(460.0)),
                                    ctx.body().line_height * 2.0,
                                    None,
                                    Some(note),
                                )
                            }
                        };
                        out.elements.push(Element::Image {
                            x,
                            y,
                            w: iw,
                            h: ih,
                            alt: alt.clone(),
                            link: *link,
                            path,
                            note,
                        });
                        y += ih;
                    }
                }
                return y;
            }
            let mut pieces = inline_pieces(inlines, &mono, &base, tone);
            cap_pieces(&mut pieces, MAX_PARAGRAPH_CHARS);
            let m = ctx.body();
            let buf = shape_rich(ctx, &pieces, &base, m, w, None);
            let h = buffer_height(&buf);
            let color = if tone == Tone::Muted {
                theme::p().md_muted
            } else {
                theme::p().md_text
            };
            out.elements.push(Element::Text {
                x,
                y,
                h,
                buffer: buf,
                color,
                sel: true,
            });
            y + h
        }
        BlockKind::Code { lang, text } => {
            let pad = ctx.px(12.0);
            let m = ctx.code();
            let mono_attrs = Attrs::new()
                .family(Family::Name(&mono))
                .color(ccol(theme::p().syntax_normal));
            let mut lines: Vec<String> = text.split('\n').map(str::to_owned).collect();
            let hidden = lines.len().saturating_sub(MAX_CODE_BLOCK_LINES);
            lines.truncate(MAX_CODE_BLOCK_LINES);
            let mut hl = find_syntax_by_token(lang).map(LineHighlighter::new);
            if let Some(h) = hl.as_mut() {
                h.step(&lines, Duration::from_millis(80));
            }
            // Shape in chunks so rendering only touches what is on screen.
            let start_idx = out.elements.len();
            let mut cy = y + pad;
            for (ci, chunk) in lines.chunks(CODE_CHUNK_LINES).enumerate() {
                let mut pieces: Vec<(String, Attrs)> = Vec::new();
                for (j, line) in chunk.iter().enumerate() {
                    if j > 0 {
                        pieces.push(("\n".into(), mono_attrs.clone()));
                    }
                    let i = ci * CODE_CHUNK_LINES + j;
                    let spans = hl.as_ref().and_then(|h| h.spans(i)).unwrap_or(&[]);
                    for (t, s) in styled_pieces(line, spans, 2000) {
                        let mut a = mono_attrs.clone();
                        if let Some(s) = s {
                            a = a.color(ccol(s.color));
                            if s.bold {
                                a = a.weight(Weight::BOLD);
                            }
                            if s.italic {
                                a = a.style(Style::Italic);
                            }
                        }
                        pieces.push((t, a));
                    }
                }
                let buf = shape_rich(ctx, &pieces, &mono_attrs, m, w - pad * 2.0, None);
                let h = buffer_height(&buf).max(m.line_height);
                out.elements.push(Element::Text {
                    x: x + pad,
                    y: cy,
                    h,
                    buffer: buf,
                    color: theme::p().syntax_normal,
                    sel: true,
                });
                cy += h;
            }
            if hidden > 0 {
                out.truncated = true;
                let a = Attrs::new()
                    .family(Family::Name(&sans))
                    .color(ccol(theme::p().md_muted))
                    .style(Style::Italic);
                let note = format!("… {hidden} more lines (Ctrl+E shows the full source)");
                let buf = shape_rich(
                    ctx,
                    &[(note, a.clone())],
                    &a,
                    ctx.body(),
                    w - pad * 2.0,
                    None,
                );
                let h = buffer_height(&buf);
                out.elements.push(Element::Text {
                    x: x + pad,
                    y: cy,
                    h,
                    buffer: buf,
                    color: theme::p().md_muted,
                    sel: true,
                });
                cy += h;
            }
            let total = (cy - y - pad).max(m.line_height) + pad * 2.0;
            out.elements.insert(
                start_idx,
                Element::Fill {
                    x,
                    y,
                    w,
                    h: total,
                    color: theme::p().md_block_bg,
                },
            );
            y + total
        }
        BlockKind::Quote { callout, blocks } => {
            let bar = ctx.px(4.0).max(2.0);
            let indent = ctx.px(16.0);
            let start_idx = out.elements.len();
            let mut cy = y;
            let bar_color = match callout {
                Some(Callout::Note) => theme::p().md_note,
                Some(Callout::Tip) => theme::p().md_tip,
                Some(Callout::Important) => theme::p().md_important,
                Some(Callout::Warning) => theme::p().md_warning,
                Some(Callout::Caution) => theme::p().md_caution,
                None => theme::p().md_border,
            };
            if let Some(c) = callout {
                let title = match c {
                    Callout::Note => "Note",
                    Callout::Tip => "Tip",
                    Callout::Important => "Important",
                    Callout::Warning => "Warning",
                    Callout::Caution => "Caution",
                };
                let a = base.clone().weight(Weight::BOLD).color(ccol(bar_color));
                let m = ctx.body();
                let buf = shape_rich(
                    ctx,
                    &[(title.to_string(), a.clone())],
                    &a,
                    m,
                    w - indent,
                    None,
                );
                let h = buffer_height(&buf);
                out.elements.push(Element::Text {
                    x: x + indent,
                    y: cy,
                    h,
                    buffer: buf,
                    color: bar_color,
                    sel: true,
                });
                cy += h + ctx.px(4.0);
            }
            let inner_tone = if callout.is_some() { tone } else { Tone::Muted };
            let end = layout_blocks(
                blocks,
                ctx,
                out,
                x + indent,
                w - indent,
                cy,
                depth + 1,
                inner_tone,
            );
            out.elements.insert(
                start_idx,
                Element::Fill {
                    x,
                    y,
                    w: bar,
                    h: (end - y).max(bar),
                    color: bar_color,
                },
            );
            end
        }
        BlockKind::List { start, items } => {
            layout_list(*start, items, ctx, out, x, w, y, depth, tone)
        }
        BlockKind::Table { aligns, head, rows } => {
            layout_table(aligns, head, rows, ctx, out, x, w, y, tone)
        }
        BlockKind::Rule => {
            let h = ctx.px(2.0).max(1.0);
            out.elements.push(Element::Fill {
                x,
                y: y + ctx.px(6.0),
                w,
                h,
                color: theme::p().md_border,
            });
            y + h + ctx.px(12.0)
        }
        BlockKind::Footnote { label, blocks } => {
            let m = ctx.body();
            let a = base.clone().color(ccol(theme::p().md_muted));
            let lbl = format!("[{label}]");
            let buf = shape_rich(ctx, &[(lbl, a.clone())], &a, m, w, None);
            let lw = buffer_width(&buf) + ctx.px(8.0);
            let h = buffer_height(&buf);
            out.elements.push(Element::Text {
                x,
                y,
                h,
                buffer: buf,
                color: theme::p().md_muted,
                sel: true,
            });
            layout_blocks(blocks, ctx, out, x + lw, w - lw, y, depth + 1, Tone::Muted).max(y + h)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn layout_list(
    start: Option<u64>,
    items: &[ListItem],
    ctx: &mut LayoutCtx,
    out: &mut Out,
    x: f32,
    w: f32,
    y: f32,
    depth: usize,
    tone: Tone,
) -> f32 {
    let indent = ctx.px(26.0);
    let m = ctx.body();
    let sans = ctx.sans.clone();
    let base = Attrs::new()
        .family(Family::Name(&sans))
        .color(ccol(theme::p().md_muted));
    let mut y = y;
    let bullet = ["•", "◦", "▪"][depth.min(30) % 3];
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            y += ctx.px(4.0);
        }
        if let Some(done) = item.task {
            let size = (m.font_size * 0.95).round();
            out.elements.push(Element::Checkbox {
                x: x + indent - size - ctx.px(8.0),
                y: y + ((m.line_height - size) / 2.0).round(),
                size,
                checked: done,
                offset: item.task_offset,
            });
        } else {
            let marker = match start {
                Some(n) => format!("{}.", n.saturating_add(i as u64)),
                None => bullet.to_string(),
            };
            let buf = shape_rich(
                ctx,
                &[(marker, base.clone())],
                &base,
                m,
                indent,
                Some(CAlign::Right),
            );
            let h = buffer_height(&buf);
            out.elements.push(Element::Text {
                x: x - ctx.px(8.0),
                y,
                h,
                buffer: buf,
                color: theme::p().md_muted,
                sel: false,
            });
        }
        let end = layout_blocks(
            &item.blocks,
            ctx,
            out,
            x + indent,
            w - indent,
            y,
            depth + 1,
            tone,
        );
        y = end.max(y + m.line_height);
    }
    y
}

#[allow(clippy::too_many_arguments)]
fn layout_table(
    aligns: &[Align],
    head: &[Vec<Inline>],
    rows: &[Vec<Vec<Inline>>],
    ctx: &mut LayoutCtx,
    out: &mut Out,
    x: f32,
    w: f32,
    y: f32,
    tone: Tone,
) -> f32 {
    let rows = &rows[..rows.len().min(MAX_TABLE_ROWS)];
    let ncols = head
        .len()
        .max(rows.iter().map(Vec::len).max().unwrap_or(0))
        .clamp(1, MAX_TABLE_COLS);
    let pad_x = ctx.px(12.0);
    let pad_y = ctx.px(6.0);
    let m = ctx.body();
    let sans = ctx.sans.clone();
    let mono = ctx.mono.clone();
    let base = Attrs::new().family(Family::Name(&sans));
    let bold = base.clone().weight(Weight::BOLD);
    let empty: Vec<Inline> = Vec::new();

    let cell = |r: Option<usize>, c: usize| -> &[Inline] {
        match r {
            None => head.get(c).map(Vec::as_slice).unwrap_or(&empty),
            Some(r) => rows[r].get(c).map(Vec::as_slice).unwrap_or(&empty),
        }
    };

    // Natural (unwrapped) width of each column.
    let mut natural = vec![0.0f32; ncols];
    for r in std::iter::once(None).chain((0..rows.len()).map(Some)) {
        for (c, nat) in natural.iter_mut().enumerate() {
            let a = if r.is_none() { &bold } else { &base };
            let pieces = inline_pieces(cell(r, c), &mono, a, tone);
            let mut b = Buffer::new_empty(m);
            b.set_wrap(Wrap::None);
            b.set_rich_text(
                pieces.iter().map(|(t, a)| (t.as_str(), a.clone())),
                a,
                Shaping::Advanced,
                None,
            );
            b.shape_until_scroll(ctx.fonts, false);
            *nat = nat.max(buffer_width(&b).ceil());
        }
    }
    // Fit columns into the available width (water-filling).
    let avail = (w - pad_x * 2.0 * ncols as f32).max(ctx.px(40.0) * ncols as f32);
    let mut widths = natural.clone();
    if natural.iter().sum::<f32>() > avail {
        let mut order: Vec<usize> = (0..ncols).collect();
        order.sort_by(|a, b| natural[*a].total_cmp(&natural[*b]));
        let mut remaining = avail;
        for (k, &c) in order.iter().enumerate() {
            let share = remaining / (ncols - k) as f32;
            widths[c] = natural[c].min(share).max(ctx.px(40.0));
            remaining -= widths[c];
        }
    }
    let col_w: Vec<f32> = widths.iter().map(|w| w + pad_x * 2.0).collect();
    let table_w: f32 = col_w.iter().sum();
    let line = ctx.px(1.0).max(1.0);

    let mut cy = y;
    let start_idx = out.elements.len();
    let mut row_tops = Vec::new();
    for (ri, r) in std::iter::once(None)
        .chain((0..rows.len()).map(Some))
        .enumerate()
    {
        let mut cx = x;
        let mut row_h = m.line_height;
        let mut cells = Vec::new();
        for c in 0..ncols {
            let a = if r.is_none() { &bold } else { &base };
            let pieces = inline_pieces(cell(r, c), &mono, a, tone);
            let align = match aligns.get(c) {
                Some(Align::Center) => Some(CAlign::Center),
                Some(Align::Right) => Some(CAlign::Right),
                _ => None,
            };
            let buf = shape_rich(ctx, &pieces, a, m, widths[c], align);
            row_h = row_h.max(buffer_height(&buf));
            cells.push((cx + pad_x, buf));
            cx += col_w[c];
        }
        let full_h = row_h + pad_y * 2.0;
        if r.is_none() {
            out.elements.push(Element::Fill {
                x,
                y: cy,
                w: table_w,
                h: full_h,
                color: theme::p().md_block_bg,
            });
        } else if ri % 2 == 0 {
            out.elements.push(Element::Fill {
                x,
                y: cy,
                w: table_w,
                h: full_h,
                color: theme::p().md_zebra,
            });
        }
        for (cx, buf) in cells {
            out.elements.push(Element::Text {
                x: cx,
                y: cy + pad_y,
                h: row_h,
                buffer: buf,
                color: theme::p().md_text,
                sel: true,
            });
        }
        row_tops.push(cy);
        cy += full_h;
    }
    // Grid lines on top of the backgrounds.
    let _ = start_idx;
    for ty in row_tops.iter().copied().chain(std::iter::once(cy)) {
        out.elements.push(Element::Fill {
            x,
            y: ty,
            w: table_w,
            h: line,
            color: theme::p().md_border,
        });
    }
    let mut gx = x;
    for cw in col_w.iter().copied() {
        out.elements.push(Element::Fill {
            x: gx,
            y,
            w: line,
            h: cy - y,
            color: theme::p().md_border,
        });
        gx += cw;
    }
    out.elements.push(Element::Fill {
        x: gx - line,
        y,
        w: line,
        h: cy - y,
        color: theme::p().md_border,
    });
    cy + line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_meta_roundtrip() {
        for i in [0, 1, 7, 1000] {
            assert_eq!(link_from_meta(meta_link(i)), Some(i));
            assert_eq!(link_from_meta(meta_link(i) | META_CODE), Some(i));
        }
        assert_eq!(link_from_meta(0), None);
        assert_eq!(link_from_meta(META_CODE), None);
    }
}
