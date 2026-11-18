//! Markdown → a small block tree, ready for layout.
//!
//! pulldown-cmark does the CommonMark/GFM parsing; this module folds its event
//! stream into blocks with inline runs. Raw HTML is never rendered: `<br>`
//! becomes a line break, comments disappear, other tags are dropped and their
//! text kept (D-004).

use pulldown_cmark::{
    Alignment, BlockQuoteKind, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd,
};

/// Containers nested deeper than this are flattened into their parent, so a
/// hostile file cannot make layout recurse without bound.
pub const MAX_DEPTH: usize = 24;

/// Whole-document preview budget (D-011 limits are per block). A document
/// over any of these opens in source view; see `Budget`.
pub const MAX_PREVIEW_BYTES: usize = 1024 * 1024;
pub const MAX_PREVIEW_LINES: usize = 100_000;
pub const MAX_PREVIEW_BLOCKS: usize = 30_000;

/// How much document the preview will take on. `bytes` and `lines` are
/// checked on the source before parsing; `blocks` counts block-level
/// elements while parsing (paragraphs, headings, code blocks, quotes,
/// lists and list items, footnotes, tables, table rows and cells, rules,
/// images), at every nesting level, and parsing stops at the first one
/// over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    pub bytes: usize,
    pub lines: usize,
    pub blocks: usize,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            bytes: MAX_PREVIEW_BYTES,
            lines: MAX_PREVIEW_LINES,
            blocks: MAX_PREVIEW_BLOCKS,
        }
    }
}

impl Budget {
    /// Most layout elements a document within this budget can need (a
    /// block makes at most a few; long code blocks one per chunk of
    /// lines). Layout stops past this, as a second line of defence.
    pub fn max_elements(&self) -> usize {
        self.blocks
            .saturating_mul(3)
            .saturating_add(self.lines / 16)
            .saturating_add(64)
    }
}

/// The document is too big to preview (`parse_within` stopped early).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OverBudget;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Inline {
    Text {
        text: String,
        style: Style,
        link: Option<usize>,
    },
    /// A hard line break (`<br>`, two trailing spaces, backslash).
    Break,
    Image {
        src: String,
        alt: String,
        link: Option<usize>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Callout {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

#[derive(Clone, Debug, PartialEq)]
pub enum BlockKind {
    Heading {
        level: u8,
        inlines: Vec<Inline>,
        slug: String,
    },
    Paragraph(Vec<Inline>),
    Code {
        lang: String,
        text: String,
    },
    Quote {
        callout: Option<Callout>,
        blocks: Vec<Block>,
    },
    List {
        start: Option<u64>,
        items: Vec<ListItem>,
    },
    Table {
        aligns: Vec<Align>,
        head: Vec<Vec<Inline>>,
        rows: Vec<Vec<Vec<Inline>>>,
    },
    Rule,
    /// Footnote definition: label + content.
    Footnote {
        label: String,
        blocks: Vec<Block>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub kind: BlockKind,
    /// Byte offset of the block in the source (for preview ↔ source sync).
    pub offset: usize,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct ListItem {
    pub task: Option<bool>,
    /// Source byte offset of the task marker (`[ ]` / `[x]`), for ticking
    /// it from the preview.
    pub task_offset: Option<usize>,
    pub blocks: Vec<Block>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MdDoc {
    pub blocks: Vec<Block>,
    /// Link destinations, indexed by `Inline::*::link`.
    pub links: Vec<String>,
}

/// Every heading in document order: (level, plain text, source byte offset).
pub fn headings(doc: &MdDoc) -> Vec<(u8, String, usize)> {
    fn walk(blocks: &[Block], out: &mut Vec<(u8, String, usize)>, depth: usize) {
        if depth > 64 {
            return;
        }
        for b in blocks {
            match &b.kind {
                BlockKind::Heading { level, inlines, .. } => {
                    let text = plain_text(inlines);
                    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
                    if !text.is_empty() {
                        out.push((*level, text, b.offset));
                    }
                }
                BlockKind::Quote { blocks, .. } | BlockKind::Footnote { blocks, .. } => {
                    walk(blocks, out, depth + 1)
                }
                BlockKind::List { items, .. } => {
                    for it in items {
                        walk(&it.blocks, out, depth + 1);
                    }
                }
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(&doc.blocks, &mut out, 0);
    out.truncate(5000);
    out
}

enum Container {
    Root(Vec<Block>),
    Quote {
        callout: Option<Callout>,
        blocks: Vec<Block>,
        offset: usize,
    },
    List {
        start: Option<u64>,
        items: Vec<ListItem>,
        offset: usize,
    },
    Item(ListItem),
    Footnote {
        label: String,
        blocks: Vec<Block>,
        offset: usize,
    },
    Table(TableBuild),
}

struct TableBuild {
    aligns: Vec<Align>,
    head: Vec<Vec<Inline>>,
    rows: Vec<Vec<Vec<Inline>>>,
    row: Vec<Vec<Inline>>,
    offset: usize,
}

#[derive(PartialEq)]
enum InlineTarget {
    Paragraph,
    Heading(u8),
    Cell,
}

struct Builder {
    stack: Vec<Container>,
    /// For each container-start event: whether a container was pushed (false
    /// when flattened because of MAX_DEPTH).
    pushed: Vec<bool>,
    inline: Option<(InlineTarget, Vec<Inline>, usize)>,
    style: Style,
    bold: u32,
    italic: u32,
    strike: u32,
    link: Vec<Option<usize>>,
    image: Option<(String, String, Option<usize>)>,
    code: Option<(String, String, usize)>,
    html: Option<(String, usize)>,
    links: Vec<String>,
    in_metadata: bool,
    /// Block-level elements seen so far, and the most allowed.
    blocks: usize,
    max_blocks: usize,
}

fn heading_level(l: HeadingLevel) -> u8 {
    match l {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// GitHub-style heading anchor: lower-case, spaces → '-', punctuation dropped.
pub fn slugify(text: &str) -> String {
    let mut s = String::new();
    for c in text.trim().chars() {
        if c.is_alphanumeric() || c == '-' || c == '_' {
            s.extend(c.to_lowercase());
        } else if c == ' ' {
            s.push('-');
        }
    }
    s
}

pub fn plain_text(inlines: &[Inline]) -> String {
    let mut s = String::new();
    for i in inlines {
        match i {
            Inline::Text { text, .. } => s.push_str(text),
            Inline::Break => s.push(' '),
            Inline::Image { alt, .. } => s.push_str(alt),
        }
    }
    s
}

/// Convert an HTML fragment to text: `<br>` → newline, comments and other
/// tags removed. Never interprets anything.
pub fn html_to_text(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        rest = &rest[lt..];
        if let Some(stripped) = rest.strip_prefix("<!--") {
            rest = stripped
                .find("-->")
                .map(|e| &stripped[e + 3..])
                .unwrap_or("");
            continue;
        }
        let Some(gt) = rest.find('>') else {
            out.push_str(rest);
            rest = "";
            break;
        };
        let tag = rest[1..gt]
            .trim()
            .trim_start_matches('/')
            .to_ascii_lowercase();
        let name = tag
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or("");
        if matches!(
            name,
            "br" | "p" | "div" | "li" | "tr" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
        ) {
            out.push('\n');
        }
        rest = &rest[gt + 1..];
    }
    out.push_str(rest);
    out.replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

impl Builder {
    fn new(max_blocks: usize) -> Self {
        Self {
            blocks: 0,
            max_blocks,
            stack: vec![Container::Root(Vec::new())],
            pushed: Vec::new(),
            inline: None,
            style: Style::default(),
            bold: 0,
            italic: 0,
            strike: 0,
            link: Vec::new(),
            image: None,
            code: None,
            html: None,
            links: Vec::new(),
            in_metadata: false,
        }
    }

    fn depth(&self) -> usize {
        self.stack.len()
    }

    fn update_style(&mut self) {
        self.style.bold = self.bold > 0;
        self.style.italic = self.italic > 0;
        self.style.strike = self.strike > 0;
    }

    fn cur_link(&self) -> Option<usize> {
        self.link.last().copied().flatten()
    }

    fn push_block(&mut self, block: Block) {
        match self.stack.last_mut() {
            Some(Container::Root(v)) | Some(Container::Quote { blocks: v, .. }) => v.push(block),
            Some(Container::Footnote { blocks: v, .. }) => v.push(block),
            Some(Container::Item(item)) => item.blocks.push(block),
            Some(Container::List { items, .. }) => {
                // Content directly in a list (should not happen) → new item.
                items.push(ListItem {
                    task: None,
                    task_offset: None,
                    blocks: vec![block],
                });
            }
            Some(Container::Table(_)) | None => {}
        }
    }

    /// Start collecting inline content, opening an implicit paragraph (tight
    /// list items have text without a Paragraph tag).
    fn ensure_inline(&mut self, offset: usize) {
        if self.inline.is_none() {
            self.inline = Some((InlineTarget::Paragraph, Vec::new(), offset));
        }
    }

    fn flush_inline(&mut self) {
        let Some((target, inlines, offset)) = self.inline.take() else {
            return;
        };
        match target {
            InlineTarget::Paragraph => {
                if !inlines
                    .iter()
                    .all(|i| matches!(i, Inline::Text { text, .. } if text.trim().is_empty()))
                {
                    self.push_block(Block {
                        kind: BlockKind::Paragraph(inlines),
                        offset,
                    });
                }
            }
            InlineTarget::Heading(level) => {
                let slug = slugify(&plain_text(&inlines));
                self.push_block(Block {
                    kind: BlockKind::Heading {
                        level,
                        inlines,
                        slug,
                    },
                    offset,
                });
            }
            InlineTarget::Cell => {
                if let Some(Container::Table(t)) = self.stack.last_mut() {
                    t.row.push(inlines);
                }
            }
        }
    }

    fn text(&mut self, t: &str, offset: usize) {
        if let Some((_, alt, _)) = &mut self.image {
            alt.push_str(t);
            return;
        }
        self.ensure_inline(offset);
        let style = self.style;
        let link = self.cur_link();
        if let Some((_, inlines, _)) = &mut self.inline {
            match inlines.last_mut() {
                Some(Inline::Text {
                    text,
                    style: s,
                    link: l,
                }) if *s == style && *l == link => text.push_str(t),
                _ => inlines.push(Inline::Text {
                    text: t.to_string(),
                    style,
                    link,
                }),
            }
        }
    }

    fn inline_push(&mut self, i: Inline, offset: usize) {
        self.ensure_inline(offset);
        if let Some((_, inlines, _)) = &mut self.inline {
            inlines.push(i);
        }
    }

    fn start_container(&mut self, c: Container) {
        self.flush_inline();
        if self.depth() >= MAX_DEPTH {
            self.pushed.push(false);
        } else {
            self.stack.push(c);
            self.pushed.push(true);
        }
    }

    fn end_container(&mut self) {
        self.flush_inline();
        if !self.pushed.pop().unwrap_or(false) || self.stack.len() <= 1 {
            return;
        }
        let Some(c) = self.stack.pop() else { return };
        match c {
            Container::Root(_) => {}
            Container::Quote {
                callout,
                blocks,
                offset,
            } => self.push_block(Block {
                kind: BlockKind::Quote { callout, blocks },
                offset,
            }),
            Container::List {
                start,
                items,
                offset,
            } => self.push_block(Block {
                kind: BlockKind::List { start, items },
                offset,
            }),
            Container::Item(item) => match self.stack.last_mut() {
                Some(Container::List { items, .. }) => items.push(item),
                _ => {
                    for b in item.blocks {
                        self.push_block(b);
                    }
                }
            },
            Container::Footnote {
                label,
                blocks,
                offset,
            } => self.push_block(Block {
                kind: BlockKind::Footnote { label, blocks },
                offset,
            }),
            Container::Table(t) => self.push_block(Block {
                kind: BlockKind::Table {
                    aligns: t.aligns,
                    head: t.head,
                    rows: t.rows,
                },
                offset: t.offset,
            }),
        }
    }

    /// Whether the document went over its block budget (parsing stops).
    fn over(&self) -> bool {
        self.blocks > self.max_blocks
    }

    fn event(&mut self, ev: Event, offset: usize) {
        if matches!(
            ev,
            Event::Rule
                | Event::Start(
                    Tag::Paragraph
                        | Tag::Heading { .. }
                        | Tag::BlockQuote(_)
                        | Tag::CodeBlock(_)
                        | Tag::HtmlBlock
                        | Tag::List(_)
                        | Tag::Item
                        | Tag::FootnoteDefinition(_)
                        | Tag::Table(_)
                        | Tag::TableHead
                        | Tag::TableRow
                        | Tag::TableCell
                        | Tag::Image { .. }
                )
        ) {
            self.blocks += 1;
            if self.over() {
                return;
            }
        }
        if self.in_metadata {
            if matches!(ev, Event::End(TagEnd::MetadataBlock(_))) {
                self.in_metadata = false;
            }
            return;
        }
        if let Some((_, text, _)) = &mut self.code {
            match ev {
                Event::Text(t) => text.push_str(&t),
                Event::End(TagEnd::CodeBlock) => {
                    let (lang, mut text, off) = self.code.take().expect("in code block");
                    if text.ends_with('\n') {
                        text.pop();
                    }
                    self.push_block(Block {
                        kind: BlockKind::Code { lang, text },
                        offset: off,
                    });
                }
                _ => {}
            }
            return;
        }
        if let Some((html, _)) = &mut self.html {
            match ev {
                Event::Html(t) | Event::Text(t) => html.push_str(&t),
                Event::End(TagEnd::HtmlBlock) => {
                    let (html, off) = self.html.take().expect("in html block");
                    let text = html_to_text(&html);
                    let mut inlines = Vec::new();
                    for (i, line) in text
                        .split('\n')
                        .map(str::trim)
                        .filter(|l| !l.is_empty())
                        .enumerate()
                    {
                        if i > 0 {
                            inlines.push(Inline::Break);
                        }
                        inlines.push(Inline::Text {
                            text: line.to_string(),
                            style: Style::default(),
                            link: None,
                        });
                    }
                    if !inlines.is_empty() {
                        self.push_block(Block {
                            kind: BlockKind::Paragraph(inlines),
                            offset: off,
                        });
                    }
                }
                _ => {}
            }
            return;
        }

        match ev {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {
                    self.flush_inline();
                    self.inline = Some((InlineTarget::Paragraph, Vec::new(), offset));
                }
                Tag::Heading { level, .. } => {
                    self.flush_inline();
                    self.inline = Some((
                        InlineTarget::Heading(heading_level(level)),
                        Vec::new(),
                        offset,
                    ));
                }
                Tag::BlockQuote(kind) => {
                    let callout = kind.map(|k| match k {
                        BlockQuoteKind::Note => Callout::Note,
                        BlockQuoteKind::Tip => Callout::Tip,
                        BlockQuoteKind::Important => Callout::Important,
                        BlockQuoteKind::Warning => Callout::Warning,
                        BlockQuoteKind::Caution => Callout::Caution,
                    });
                    self.start_container(Container::Quote {
                        callout,
                        blocks: Vec::new(),
                        offset,
                    });
                }
                Tag::CodeBlock(kind) => {
                    self.flush_inline();
                    let lang = match kind {
                        CodeBlockKind::Fenced(l) => {
                            l.split([' ', ',', '{']).next().unwrap_or("").to_string()
                        }
                        CodeBlockKind::Indented => String::new(),
                    };
                    self.code = Some((lang, String::new(), offset));
                }
                Tag::HtmlBlock => {
                    self.flush_inline();
                    self.html = Some((String::new(), offset));
                }
                Tag::List(start) => self.start_container(Container::List {
                    start,
                    items: Vec::new(),
                    offset,
                }),
                Tag::Item => self.start_container(Container::Item(ListItem::default())),
                Tag::FootnoteDefinition(label) => self.start_container(Container::Footnote {
                    label: label.to_string(),
                    blocks: Vec::new(),
                    offset,
                }),
                Tag::Table(aligns) => {
                    let aligns = aligns
                        .iter()
                        .map(|a| match a {
                            Alignment::Center => Align::Center,
                            Alignment::Right => Align::Right,
                            _ => Align::Left,
                        })
                        .collect();
                    self.start_container(Container::Table(TableBuild {
                        aligns,
                        head: Vec::new(),
                        rows: Vec::new(),
                        row: Vec::new(),
                        offset,
                    }));
                }
                Tag::TableHead | Tag::TableRow => {
                    if let Some(Container::Table(t)) = self.stack.last_mut() {
                        t.row.clear();
                    }
                }
                Tag::TableCell => {
                    self.inline = Some((InlineTarget::Cell, Vec::new(), offset));
                }
                Tag::Emphasis => {
                    self.italic += 1;
                    self.update_style();
                }
                Tag::Strong => {
                    self.bold += 1;
                    self.update_style();
                }
                Tag::Strikethrough => {
                    self.strike += 1;
                    self.update_style();
                }
                Tag::Link { dest_url, .. } => {
                    self.links.push(dest_url.to_string());
                    self.link.push(Some(self.links.len() - 1));
                }
                Tag::Image { dest_url, .. } => {
                    self.image = Some((dest_url.to_string(), String::new(), self.cur_link()));
                }
                Tag::MetadataBlock(_) => self.in_metadata = true,
                _ => {}
            },
            Event::End(end) => match end {
                TagEnd::Paragraph | TagEnd::Heading(_) => self.flush_inline(),
                TagEnd::BlockQuote(_)
                | TagEnd::List(_)
                | TagEnd::Item
                | TagEnd::FootnoteDefinition
                | TagEnd::Table => self.end_container(),
                TagEnd::TableCell => self.flush_inline(),
                TagEnd::TableHead => {
                    if let Some(Container::Table(t)) = self.stack.last_mut() {
                        t.head = std::mem::take(&mut t.row);
                    }
                }
                TagEnd::TableRow => {
                    if let Some(Container::Table(t)) = self.stack.last_mut() {
                        let row = std::mem::take(&mut t.row);
                        t.rows.push(row);
                    }
                }
                TagEnd::Emphasis => {
                    self.italic = self.italic.saturating_sub(1);
                    self.update_style();
                }
                TagEnd::Strong => {
                    self.bold = self.bold.saturating_sub(1);
                    self.update_style();
                }
                TagEnd::Strikethrough => {
                    self.strike = self.strike.saturating_sub(1);
                    self.update_style();
                }
                TagEnd::Link => {
                    self.link.pop();
                }
                TagEnd::Image => {
                    if let Some((src, alt, link)) = self.image.take() {
                        self.inline_push(Inline::Image { src, alt, link }, offset);
                    }
                }
                _ => {}
            },
            Event::Text(t) => self.text(&t, offset),
            Event::Code(t) => {
                let saved = self.style;
                self.style.code = true;
                self.text(&t, offset);
                self.style = saved;
            }
            Event::InlineMath(t) | Event::DisplayMath(t) => {
                let saved = self.style;
                self.style.code = true;
                self.text(&t, offset);
                self.style = saved;
            }
            Event::SoftBreak => self.text(" ", offset),
            Event::HardBreak => self.inline_push(Inline::Break, offset),
            Event::InlineHtml(h) | Event::Html(h) => {
                let lower = h.to_ascii_lowercase();
                if lower.starts_with("<br") {
                    self.inline_push(Inline::Break, offset);
                }
                // Every other inline tag is dropped; its text arrives as Text.
            }
            Event::FootnoteReference(label) => {
                let saved = self.style;
                self.style = Style::default();
                self.text(&format!("[{label}]"), offset);
                self.style = saved;
            }
            Event::TaskListMarker(done) => {
                if let Some(Container::Item(item)) = self.stack.last_mut() {
                    item.task = Some(done);
                    item.task_offset = Some(offset);
                }
            }
            Event::Rule => {
                self.flush_inline();
                self.push_block(Block {
                    kind: BlockKind::Rule,
                    offset,
                });
            }
        }
    }

    fn finish(mut self) -> MdDoc {
        self.flush_inline();
        while self.stack.len() > 1 {
            self.pushed.push(true);
            self.end_container();
        }
        let blocks = match self.stack.pop() {
            Some(Container::Root(b)) => b,
            _ => Vec::new(),
        };
        MdDoc {
            blocks,
            links: self.links,
        }
    }
}

/// Parse with no whole-document budget (tests and small inputs).
#[cfg(test)]
pub fn parse(src: &str) -> MdDoc {
    let unlimited = Budget {
        bytes: usize::MAX,
        lines: usize::MAX,
        blocks: usize::MAX,
    };
    parse_within(src, &unlimited).expect("no budget")
}

/// Parse `src` unless it is over `budget`: too many bytes or lines (checked
/// first, nothing is parsed), or too many block-level elements (parsing
/// stops at the first one over, so nothing past the budget is built).
pub fn parse_within(src: &str, budget: &Budget) -> Result<MdDoc, OverBudget> {
    parse_counted(src, budget).0
}

/// `parse_within`, plus how many block-level elements were seen (tests
/// check that parsing stopped right at the budget).
fn parse_counted(src: &str, budget: &Budget) -> (Result<MdDoc, OverBudget>, usize) {
    if src.len() > budget.bytes {
        return (Err(OverBudget), 0);
    }
    let lines = src.bytes().filter(|b| *b == b'\n').count() + 1;
    if lines > budget.lines {
        return (Err(OverBudget), 0);
    }
    let opts = Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_GFM
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS;
    let mut b = Builder::new(budget.blocks);
    for (ev, range) in Parser::new_ext(src, opts).into_offset_iter() {
        b.event(ev, range.start);
        if b.over() {
            return (Err(OverBudget), b.blocks);
        }
    }
    let seen = b.blocks;
    (Ok(b.finish()), seen)
}

/// ATX headings (`# Title`) found by scanning lines, skipping fenced code:
/// Go to Heading for a document too large to parse. (level, text, line).
pub fn scan_headings<'a>(lines: impl IntoIterator<Item = &'a str>) -> Vec<(u8, String, usize)> {
    let mut out = Vec::new();
    let mut fence: Option<(u8, usize)> = None;
    for (i, line) in lines.into_iter().enumerate() {
        let t = line.trim_start_matches(' ');
        if line.len() - t.len() > 3 {
            continue;
        }
        let fc = t.bytes().next().unwrap_or(0);
        if fc == b'`' || fc == b'~' {
            let n = t.bytes().take_while(|b| *b == fc).count();
            if n >= 3 {
                match fence {
                    None => fence = Some((fc, n)),
                    Some((c, m)) if c == fc && n >= m && t[n..].trim().is_empty() => fence = None,
                    _ => {}
                }
                continue;
            }
        }
        if fence.is_some() {
            continue;
        }
        let level = t.bytes().take_while(|b| *b == b'#').count();
        if !(1..=6).contains(&level) {
            continue;
        }
        let rest = &t[level..];
        if !(rest.is_empty() || rest.starts_with([' ', '\t'])) {
            continue;
        }
        let rest = rest.trim();
        let rest = rest.trim_end_matches('#');
        let text = rest.split_whitespace().collect::<Vec<_>>().join(" ");
        if !text.is_empty() {
            out.push((level as u8, text, i));
            if out.len() >= 5000 {
                break;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> Inline {
        Inline::Text {
            text: s.into(),
            style: Style::default(),
            link: None,
        }
    }

    #[test]
    fn headings_and_paragraphs() {
        let d = parse("# Hello World!\n\nSome *em* text.\n");
        assert_eq!(d.blocks.len(), 2);
        match &d.blocks[0].kind {
            BlockKind::Heading { level, slug, .. } => {
                assert_eq!(*level, 1);
                assert_eq!(slug, "hello-world");
            }
            k => panic!("{k:?}"),
        }
        match &d.blocks[1].kind {
            BlockKind::Paragraph(i) => {
                assert_eq!(i[0], text("Some "));
                assert!(matches!(&i[1], Inline::Text { style, .. } if style.italic));
            }
            k => panic!("{k:?}"),
        }
        assert_eq!(d.blocks[1].offset, 16);
    }

    #[test]
    fn tight_and_task_lists() {
        let d = parse("- a\n- [x] b\n  - c\n");
        let BlockKind::List { items, start } = &d.blocks[0].kind else {
            panic!()
        };
        assert_eq!(*start, None);
        assert_eq!(items.len(), 2);
        assert_eq!(items[1].task, Some(true));
        assert!(matches!(items[1].blocks[1].kind, BlockKind::List { .. }));
    }

    #[test]
    fn code_tables_links_images() {
        let d = parse(
            "```rust,ignore\nfn x() {}\n```\n\n| a | b |\n|:-|-:|\n| 1 | 2 |\n\n[l](http://x) ![alt](i.png)\n",
        );
        assert_eq!(
            d.blocks[0].kind,
            BlockKind::Code {
                lang: "rust".into(),
                text: "fn x() {}".into()
            }
        );
        let BlockKind::Table { aligns, head, rows } = &d.blocks[1].kind else {
            panic!()
        };
        assert_eq!(aligns, &[Align::Left, Align::Right]);
        assert_eq!(head.len(), 2);
        assert_eq!(rows.len(), 1);
        assert_eq!(d.links, vec!["http://x".to_string()]);
        let BlockKind::Paragraph(i) = &d.blocks[2].kind else {
            panic!()
        };
        assert!(matches!(&i[0], Inline::Text { link: Some(0), .. }));
        assert!(i.iter().any(
            |x| matches!(x, Inline::Image { src, alt, .. } if src == "i.png" && alt == "alt")
        ));
    }

    #[test]
    fn html_is_never_rendered() {
        let d = parse(
            "a<br>b <span style=x>c</span>\n\n<div>\n<script>alert(1)</script>\n</div>\n\n<!-- hidden -->\n",
        );
        let BlockKind::Paragraph(i) = &d.blocks[0].kind else {
            panic!()
        };
        assert!(i.contains(&Inline::Break));
        assert_eq!(plain_text(i), "a b c");
        // Script body becomes inert text; comment produces nothing.
        assert_eq!(d.blocks.len(), 2);
        assert_eq!(html_to_text("x<!-- y -->z"), "xz");
        assert_eq!(html_to_text("unterminated <b"), "unterminated <b");
    }

    #[test]
    fn deep_nesting_is_flattened() {
        let src = ">".repeat(500) + " deep\n";
        let d = parse(&src);
        fn depth(b: &[Block]) -> usize {
            b.iter()
                .map(|b| match &b.kind {
                    BlockKind::Quote { blocks, .. } => 1 + depth(blocks),
                    _ => 0,
                })
                .max()
                .unwrap_or(0)
        }
        assert!(depth(&d.blocks) < MAX_DEPTH);
        let src = (0..300)
            .map(|i| format!("{}- x\n", "  ".repeat(i)))
            .collect::<String>();
        parse(&src);
    }

    #[test]
    fn front_matter_is_hidden_and_callouts_parsed() {
        let d = parse("---\ntitle: x\n---\n\n> [!WARNING]\n> careful\n");
        assert_eq!(d.blocks.len(), 1);
        assert!(matches!(
            d.blocks[0].kind,
            BlockKind::Quote {
                callout: Some(Callout::Warning),
                ..
            }
        ));
    }

    fn budget(bytes: usize, lines: usize, blocks: usize) -> Budget {
        Budget {
            bytes,
            lines,
            blocks,
        }
    }

    #[test]
    fn budget_bytes_and_lines_are_checked_before_parsing() {
        let src = "# a\n\nsome text\n";
        let roomy = budget(1000, 1000, 1000);
        assert_eq!(parse_within(src, &roomy), Ok(parse(src)));
        assert_eq!(
            parse_counted(src, &budget(src.len() - 1, 1000, 1000)),
            (Err(OverBudget), 0)
        );
        assert!(parse_within(src, &budget(src.len(), 1000, 1000)).is_ok());
        // Four lines (the last one empty).
        assert_eq!(
            parse_counted(src, &budget(1000, 3, 1000)),
            (Err(OverBudget), 0)
        );
        assert!(parse_within(src, &budget(1000, 4, 1000)).is_ok());
    }

    #[test]
    fn parsing_stops_at_the_block_budget() {
        // Small in bytes, enormous in blocks: nothing past the budget is
        // built.
        let src = "# x\n".repeat(200_000);
        let b = budget(usize::MAX, usize::MAX, 100);
        assert_eq!(parse_counted(&src, &b), (Err(OverBudget), 101));
        let exact = "# x\n".repeat(100);
        let (r, seen) = parse_counted(&exact, &b);
        assert_eq!(seen, 100);
        assert_eq!(r.unwrap().blocks.len(), 100);
        assert!(parse_within(&"# x\n".repeat(101), &b).is_err());
    }

    #[test]
    fn nested_blocks_count() {
        let unlimited = budget(usize::MAX, usize::MAX, usize::MAX);
        let count = |src: &str| parse_counted(src, &unlimited).1;
        // Three quotes and the paragraph inside.
        assert_eq!(count("> > > x\n"), 4);
        // List, two items, nested list and its item (tight: no paragraphs).
        assert_eq!(count("- a\n- b\n  - c\n"), 5);
        // Table, head, two rows, 2 × 3 cells.
        assert_eq!(count("| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n"), 10);
        // Reference paragraph, footnote definition and its paragraph.
        assert_eq!(count("x[^1]\n\n[^1]: note\n"), 3);
        // Paragraph and two images.
        assert_eq!(count("![a](a.png) ![b](b.png)\n"), 3);
        // Containers flattened past MAX_DEPTH still count.
        let deep = ">".repeat(100) + " x\n";
        assert_eq!(count(&deep), 101);
        assert!(parse_within(&deep, &budget(usize::MAX, usize::MAX, 100)).is_err());
        for (src, n) in [("> > > x\n", 4), ("- a\n- b\n  - c\n", 5)] {
            assert!(parse_within(src, &budget(usize::MAX, usize::MAX, n)).is_ok());
            assert!(parse_within(src, &budget(usize::MAX, usize::MAX, n - 1)).is_err());
        }
    }

    #[test]
    fn default_budget_previews_ordinary_documents() {
        let sample = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/sample.md"
        ))
        .unwrap();
        assert_eq!(
            parse_within(&sample, &Budget::default()),
            Ok(parse(&sample))
        );
        // A long book: 940 paragraphs of ~1 KB plus chapter headings.
        let para = "A sentence of ordinary prose that goes on for a while. ".repeat(18);
        let book: String = (0..940)
            .map(|i| {
                let h = if i % 30 == 0 { "# Chapter\n\n" } else { "" };
                format!("{h}{para}\n\n")
            })
            .collect();
        assert!(book.len() > 900_000);
        assert!(parse_within(&book, &Budget::default()).is_ok());
    }

    #[test]
    fn scan_headings_skips_code_and_non_headings() {
        let src = "# One\n#nope\n  ## Two ##\n```\n# not a heading\n````\n\
                   ~~~md\n# also not\n```\n~~~\n    # indented code\n###### Six\n####### seven\n#\n";
        let got = scan_headings(src.lines());
        assert_eq!(
            got,
            vec![
                (1, "One".to_string(), 0),
                (2, "Two".to_string(), 2),
                (6, "Six".to_string(), 11),
            ]
        );
        let many = "# h\n".repeat(6000);
        assert_eq!(scan_headings(many.lines()).len(), 5000);
    }

    #[test]
    fn every_prefix_parses() {
        let src = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/sample.md"
        ))
        .unwrap();
        for (i, _) in src.char_indices() {
            parse(&src[..i]);
        }
    }
}
