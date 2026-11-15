//! Syntax highlighting (lite-xl `tokenizer.lua` + `doc/highlighter.lua`).
//!
//! Uses syntect with two-face's extended grammar set (~200 languages) on the
//! pure-Rust fancy-regex engine. Highlighting runs incrementally under a time
//! budget: the view draws unhighlighted text until lines are ready, so opening
//! a large file or pressing End never blocks the UI.

use std::path::Path;
use std::str::FromStr;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use syntect::highlighting::{
    Color as SColor, FontStyle, HighlightIterator, HighlightState, Highlighter, ScopeSelectors,
    StyleModifier, Theme, ThemeItem, ThemeSettings,
};
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet};

use crate::canvas::Color;
use crate::theme;

/// Lines longer than this (bytes) are not parsed — regex engines can take
/// a very long time on minified files. They are drawn in the default colour.
pub const MAX_HIGHLIGHT_BYTES: usize = 4096;
/// Save parser state every this many lines so editing (Epic 2) can resume
/// highlighting from a nearby line instead of the top.
const CHECKPOINT_EVERY: usize = 128;

pub fn syntax_set() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(two_face::syntax::extra_newlines)
}

fn scol(c: Color) -> SColor {
    SColor {
        r: c.r,
        g: c.g,
        b: c.b,
        a: 255,
    }
}

/// The syntect theme for the active palette (one per palette, built once).
pub fn theme() -> &'static Theme {
    static DARK: OnceLock<Theme> = OnceLock::new();
    static LIGHT: OnceLock<Theme> = OnceLock::new();
    let cell = if theme::is_light() { &LIGHT } else { &DARK };
    cell.get_or_init(|| build_theme(theme::p()))
}

/// A syntect theme built from a palette's syntax colours (lite-xl's scopes).
fn build_theme(pal: &theme::Palette) -> Theme {
    {
        let item = |scopes: &str, c: Color, style: Option<FontStyle>| ThemeItem {
            scope: ScopeSelectors::from_str(scopes).expect("valid scope selector"),
            style: StyleModifier {
                foreground: Some(scol(c)),
                background: None,
                font_style: style,
            },
        };
        let none = None;
        Theme {
            name: Some("lite-xl default".into()),
            author: None,
            settings: ThemeSettings {
                foreground: Some(scol(pal.syntax_normal)),
                background: Some(scol(pal.background)),
                ..ThemeSettings::default()
            },
            scopes: vec![
                item(
                    "comment, punctuation.definition.comment",
                    pal.syntax_comment,
                    none,
                ),
                item(
                    "string, punctuation.definition.string, constant.character.escape",
                    pal.syntax_string,
                    none,
                ),
                item("constant.numeric", pal.syntax_number, none),
                item(
                    "constant.language, constant.character, constant.other, support.constant, variable.other.constant",
                    pal.syntax_literal,
                    none,
                ),
                item(
                    "keyword, keyword.control, storage, storage.type, storage.modifier",
                    pal.syntax_keyword,
                    none,
                ),
                item(
                    "support.type, entity.name.type, entity.name.class, \
                     entity.name.struct, entity.name.enum, entity.name.trait, entity.other.inherited-class, \
                     variable.language, support.class",
                    pal.syntax_keyword2,
                    none,
                ),
                item(
                    "keyword.operator, punctuation.accessor",
                    pal.syntax_operator,
                    none,
                ),
                item(
                    "entity.name.function, support.function, meta.function-call variable.function, \
                     variable.function, entity.name.macro, support.macro",
                    pal.syntax_function,
                    none,
                ),
                item("entity.name.tag", pal.syntax_keyword, none),
                item("entity.other.attribute-name", pal.syntax_function, none),
                item("meta.attribute, meta.annotation", pal.syntax_keyword2, none),
                // Markdown / markup source
                item(
                    "markup.heading, markup.heading punctuation",
                    pal.syntax_keyword2,
                    Some(FontStyle::BOLD),
                ),
                item("markup.bold", pal.syntax_normal, Some(FontStyle::BOLD)),
                item("markup.italic", pal.syntax_normal, Some(FontStyle::ITALIC)),
                item("markup.raw, markup.inline.raw", pal.syntax_string, none),
                item(
                    "markup.underline.link, meta.link, string.other.link",
                    pal.syntax_function,
                    none,
                ),
                item(
                    "markup.list punctuation, punctuation.definition.list",
                    pal.syntax_number,
                    none,
                ),
                item("markup.quote", pal.syntax_string, none),
                item("markup.inserted", pal.good, none),
                item("markup.deleted, invalid", pal.error, none),
            ],
        }
    }
}

/// Line-comment syntax for Ctrl+/: (start, end for block-style languages).
/// Matched on exact grammar names (lower-case), so "Plain Text" can never
/// pick up "tex".
pub fn comment_tokens(syntax_name: &str) -> Option<(&'static str, Option<&'static str>)> {
    let n = syntax_name.to_ascii_lowercase();
    let line = |t: &'static str| Some((t, None));
    match n.as_str() {
        "plain text" | "csv" | "tsv" | "diff" | "log" | "" => None,
        "html" | "html (rails)" | "html (jinja2)" | "xml" | "markdown" | "multimarkdown"
        | "svg" | "vue" | "svelte" | "xhtml" | "asp" | "html (tcl)" => Some(("<!--", Some("-->"))),
        "css" | "scss" | "sass" | "less" => Some(("/*", Some("*/"))),
        "ocaml" => Some(("(*", Some("*)"))),
        "python" | "ruby" | "perl" | "makefile" | "dockerfile" | "powershell" | "r" | "cmake"
        | "nim" | "julia" | "elixir" | "nix" | "graphql" | "crystal" | "tcl" | "fish" | "yaml"
        | "toml" | "git commit" | "git ignore" | "git config" | "git attributes" | "ssh config"
        | "hosts" | "fstab" | "requirements.txt" | "terraform" | "hcl" | "nginx"
        | "apache conf" | "awk" | "sed" | "coffeescript" | "gnuplot" | "cabal" | "zsh" => line("#"),
        "lua" | "sql" | "haskell" | "elm" | "ada" | "vhdl" | "purescript" | "agda" => line("--"),
        "lisp" | "common lisp" | "clojure" | "scheme" | "racket" | "ini" | "properties"
        | "assembly (x86_64)" | "arm assembly" | "nasm" | "autohotkey" => line(";"),
        "batch file" => line("REM"),
        "viml" | "vim script" => line("\""),
        "latex" | "tex" | "bibtex" | "erlang" | "matlab" | "octave" | "prolog" => line("%"),
        "fortran" | "fortran (modern)" => line("!"),
        _ if n.contains("shell") || n.contains("bash") => line("#"),
        // C family and most others (Rust, C, C++, C#, Java, JavaScript,
        // TypeScript, Go, Swift, Kotlin, Dart, PHP, Zig, JSON with
        // comments, …).
        _ => line("//"),
    }
}

/// Pick a grammar from the file name, then the first line (shebang etc.).
pub fn find_syntax(path: &Path, first_line: &str) -> &'static SyntaxReference {
    let ss = syntax_set();
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase());
    ext.as_deref()
        .and_then(|e| ss.find_syntax_by_extension(e))
        .or_else(|| ss.find_syntax_by_extension(&name))
        .or_else(|| ss.find_syntax_by_first_line(first_line))
        .unwrap_or_else(|| ss.find_syntax_plain_text())
}

/// Find a grammar by a markdown fence token like `rust`, `py`, `js`.
pub fn find_syntax_by_token(token: &str) -> Option<&'static SyntaxReference> {
    let t = token.trim().to_ascii_lowercase();
    if t.is_empty() {
        return None;
    }
    syntax_set().find_syntax_by_token(&t)
}

/// A coloured byte range within one line. Lines with no spans use the
/// default colour throughout. Offsets fit in 16 bits because lines longer
/// than [`MAX_HIGHLIGHT_BYTES`] are never highlighted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: u16,
    pub end: u16,
    pub color: Color,
    pub bold: bool,
    pub italic: bool,
}

const _: () = assert!(MAX_HIGHLIGHT_BYTES < u16::MAX as usize);

/// The spans of `CHECKPOINT_EVERY` consecutive lines, stored exactly sized:
/// a big file is a few hundred of these instead of one allocation per line.
struct Block {
    spans: Box<[Span]>,
    /// `ends[k]` = index into `spans` just past line k's last span.
    ends: Box<[u32]>,
}

/// A hidden tab keeps its colours up to this many lines (see
/// [`LineHighlighter::release_if_long`]).
pub const HIDDEN_KEEP_LINES: usize = 2000;

pub struct LineHighlighter {
    syntax: &'static SyntaxReference,
    plain: bool,
    highlighter: Highlighter<'static>,
    /// Finished blocks, then the block being filled (same layout).
    blocks: Vec<Block>,
    cur_spans: Vec<Span>,
    cur_ends: Vec<u32>,
    parse: ParseState,
    state: HighlightState,
    /// Parser state before the first line of each block.
    checkpoints: Vec<(ParseState, HighlightState)>,
}

impl LineHighlighter {
    pub fn new(syntax: &'static SyntaxReference) -> Self {
        let highlighter = Highlighter::new(theme());
        let state = HighlightState::new(&highlighter, ScopeStack::new());
        let plain = syntax.name == "Plain Text";
        Self {
            syntax,
            plain,
            highlighter,
            blocks: Vec::new(),
            cur_spans: Vec::new(),
            cur_ends: Vec::new(),
            parse: ParseState::new(syntax),
            state,
            checkpoints: Vec::new(),
        }
    }

    /// Start over with the active palette's colours (theme switch): every
    /// span, parse state and checkpoint is rebuilt.
    pub fn retheme(&mut self) {
        *self = Self::new(self.syntax);
    }

    pub fn name(&self) -> &str {
        &self.syntax.name
    }

    /// Number of lines (from the top) that are highlighted.
    pub fn done(&self) -> usize {
        self.blocks.len() * CHECKPOINT_EVERY + self.cur_ends.len()
    }

    pub fn is_plain(&self) -> bool {
        self.plain
    }

    /// Spans for line `i`, if it has been highlighted.
    pub fn spans(&self, i: usize) -> Option<&[Span]> {
        let (b, k) = (i / CHECKPOINT_EVERY, i % CHECKPOINT_EVERY);
        let (spans, ends): (&[Span], &[u32]) = match self.blocks.get(b) {
            Some(block) => (&block.spans, &block.ends),
            None if b == self.blocks.len() => (&self.cur_spans, &self.cur_ends),
            None => return None,
        };
        let end = *ends.get(k)? as usize;
        let start = match k.checked_sub(1) {
            Some(prev) => *ends.get(prev)? as usize,
            None => 0,
        };
        spans.get(start..end)
    }

    /// Is there highlighting left to do above line `upto` (exclusive) of a
    /// document with `line_count` lines?
    pub fn needs(&self, upto: usize, line_count: usize) -> bool {
        !self.plain && self.done() < upto.min(line_count)
    }

    /// Highlight more lines for up to `budget`. Returns true if lines remain.
    pub fn step(&mut self, lines: &[String], budget: Duration) -> bool {
        self.step_to(lines, usize::MAX, budget)
    }

    /// Highlight more lines for up to `budget`, but no further than line
    /// `upto` (exclusive): colouring is stateful from the top of the file,
    /// so a view asks only for as far as it shows. Returns true if lines
    /// before `upto` remain (see [`Self::needs`]).
    pub fn step_to(&mut self, lines: &[String], upto: usize, budget: Duration) -> bool {
        if self.plain {
            return false;
        }
        let start = Instant::now();
        let stop = upto.min(lines.len());
        let mut text = String::new();
        let mut i = self.done();
        while i < stop {
            if i.is_multiple_of(CHECKPOINT_EVERY) && self.checkpoints.len() == i / CHECKPOINT_EVERY
            {
                self.checkpoints
                    .push((self.parse.clone(), self.state.clone()));
            }
            let Some(line) = lines.get(i) else { break };
            if line.len() <= MAX_HIGHLIGHT_BYTES {
                text.clear();
                text.push_str(line);
                text.push('\n');
                self.highlight_one(&text, line.len());
            }
            self.cur_ends.push(self.cur_spans.len() as u32);
            i += 1;
            if self.cur_ends.len() == CHECKPOINT_EVERY {
                self.blocks.push(Block {
                    spans: std::mem::take(&mut self.cur_spans).into_boxed_slice(),
                    ends: std::mem::take(&mut self.cur_ends).into_boxed_slice(),
                });
            }
            if i.is_multiple_of(16) && start.elapsed() >= budget {
                break;
            }
        }
        i < stop
    }

    /// Parse one line (with its line break) and append its spans.
    fn highlight_one(&mut self, text: &str, line_len: usize) {
        let ops = match self.parse.parse_line(text, syntax_set()) {
            Ok(ops) => ops,
            Err(_) => {
                // Broken grammar state: reset and show this line plain.
                self.parse = ParseState::new(self.syntax);
                self.state = HighlightState::new(&self.highlighter, ScopeStack::new());
                return;
            }
        };
        let normal = theme::p().syntax_normal;
        let line_start = self.cur_spans.len();
        let mut pos = 0usize;
        for (style, piece) in HighlightIterator::new(&mut self.state, &ops, text, &self.highlighter)
        {
            let start = pos;
            pos += piece.len();
            let end = pos.min(line_len);
            if start >= end {
                continue;
            }
            let fg = style.foreground;
            let color = Color::rgb(fg.r, fg.g, fg.b);
            let bold = style.font_style.contains(FontStyle::BOLD);
            let italic = style.font_style.contains(FontStyle::ITALIC);
            if color == normal && !bold && !italic {
                continue;
            }
            let (start, end) = (start as u16, end as u16);
            // A neighbour with the same look continues the previous span
            // (a string and its quotes, a comment and its marker).
            if self.cur_spans.len() > line_start
                && let Some(last) = self.cur_spans.last_mut()
                && last.end == start
                && (last.color, last.bold, last.italic) == (color, bold, italic)
            {
                last.end = end;
                continue;
            }
            self.cur_spans.push(Span {
                start,
                end,
                color,
                bold,
                italic,
            });
        }
    }

    /// A view was hidden: drop the colours of a long stretch (they are
    /// rebuilt on demand, as far as the view shows). A short stretch is
    /// kept, so small files don't flash uncoloured when their tab returns.
    pub fn release_if_long(&mut self) {
        if self.done() > HIDDEN_KEEP_LINES {
            *self = Self::new(self.syntax);
        }
    }

    /// Forget highlighting from line `line` onwards (after an edit) and
    /// resume from the nearest checkpoint at or before it.
    pub fn invalidate_from(&mut self, line: usize) {
        if self.plain || line >= self.done() {
            return;
        }
        let cp = (line / CHECKPOINT_EVERY).min(self.checkpoints.len().saturating_sub(1));
        if let Some((p, s)) = self.checkpoints.get(cp).cloned() {
            self.parse = p;
            self.state = s;
            self.blocks.truncate(cp);
            self.cur_spans.clear();
            self.cur_ends.clear();
            self.checkpoints.truncate(cp + 1);
        } else {
            *self = Self::new(self.syntax);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comment_tokens_by_exact_name() {
        assert_eq!(comment_tokens("Plain Text"), None);
        assert_eq!(comment_tokens("Rust"), Some(("//", None)));
        assert_eq!(comment_tokens("Python"), Some(("#", None)));
        assert_eq!(
            comment_tokens("Bourne Again Shell (bash)"),
            Some(("#", None))
        );
        assert_eq!(comment_tokens("LaTeX"), Some(("%", None)));
        assert_eq!(comment_tokens("Markdown"), Some(("<!--", Some("-->"))));
        assert_eq!(comment_tokens("INI"), Some((";", None)));
        assert_eq!(comment_tokens("Lua"), Some(("--", None)));
        // Every grammar the editor ships gets an answer without panicking.
        for syn in syntax_set().syntaxes() {
            let _ = comment_tokens(&syn.name);
        }
        let txt = find_syntax(std::path::Path::new("notes.txt"), "");
        assert_eq!(comment_tokens(&txt.name), None, "{}", txt.name);
    }

    fn lines(s: &str) -> Vec<String> {
        s.lines().map(str::to_owned).collect()
    }

    #[test]
    fn finds_common_languages() {
        for (file, want) in [
            ("a.rs", "Rust"),
            ("a.py", "Python"),
            ("a.md", "Markdown"),
            ("a.toml", "TOML"),
            ("a.ts", "TypeScript"),
            ("a.json", "JSON"),
            ("a.bat", "Batch File"),
            ("Cargo.lock", "TOML"),
            ("a.unknownext", "Plain Text"),
        ] {
            let name = &find_syntax(Path::new(file), "").name;
            assert!(name.contains(want), "{file}: got {name}");
        }
        assert_eq!(
            find_syntax(Path::new("script"), "#!/bin/bash").name,
            "Bourne Again Shell (bash)"
        );
    }

    #[test]
    fn highlights_rust_keywords_and_strings() {
        let src = lines("fn main() {\n    let s = \"hi\"; // c\n}");
        let mut h = LineHighlighter::new(find_syntax(Path::new("x.rs"), ""));
        while h.step(&src, Duration::from_millis(100)) {}
        assert_eq!(h.done(), 3);
        let l1 = h.spans(1).unwrap();
        let color_at = |spans: &[Span], b: u16| {
            spans
                .iter()
                .find(|s| s.start <= b && b < s.end)
                .map(|s| s.color)
        };
        assert_eq!(color_at(l1, 4), Some(theme::p().syntax_keyword), "let");
        assert_eq!(color_at(l1, 13), Some(theme::p().syntax_string), "string");
        assert_eq!(color_at(l1, 19), Some(theme::p().syntax_comment), "comment");
    }

    #[test]
    fn budget_is_respected_and_resumes() {
        let src: Vec<String> = (0..20_000)
            .map(|i| format!("let x{i} = {i}; // n"))
            .collect();
        let mut h = LineHighlighter::new(find_syntax(Path::new("x.rs"), ""));
        let more = h.step(&src, Duration::from_micros(1));
        assert!(more);
        assert!(h.done() < src.len());
        while h.step(&src, Duration::from_millis(50)) {}
        assert_eq!(h.done(), src.len());
    }

    #[test]
    fn long_lines_are_skipped_not_parsed() {
        let src = vec![format!(
            "let s = \"{}\";",
            "x".repeat(MAX_HIGHLIGHT_BYTES * 4)
        )];
        let mut h = LineHighlighter::new(find_syntax(Path::new("x.rs"), ""));
        h.step(&src, Duration::from_secs(1));
        assert_eq!(h.spans(0), Some(&[][..]));
    }

    #[test]
    fn invalidate_resumes_from_checkpoint() {
        let src: Vec<String> = (0..1000).map(|i| format!("let x{i} = 1;")).collect();
        let mut h = LineHighlighter::new(find_syntax(Path::new("x.rs"), ""));
        while h.step(&src, Duration::from_secs(1)) {}
        let before = h.spans(700).unwrap().to_vec();
        h.invalidate_from(700);
        assert!(h.done() <= 700 && h.done() >= 700 - CHECKPOINT_EVERY);
        while h.step(&src, Duration::from_secs(1)) {}
        assert_eq!(h.spans(700).unwrap(), &before[..]);
    }

    /// Lines whose colours depend on what came before (a block comment
    /// and a raw string, each spanning many lines).
    fn stateful_lines(n: usize) -> Vec<String> {
        (0..n)
            .map(|i| match i % 97 {
                5 => "/* comment starts".to_string(),
                40 => "ends */ let r = r#\"raw starts".to_string(),
                60 => "raw ends\"#; struct S;".to_string(),
                _ => format!("let x{i} = \"s\"; // c"),
            })
            .collect()
    }

    fn full_pass(src: &[String]) -> Vec<Vec<Span>> {
        let mut h = LineHighlighter::new(find_syntax(Path::new("x.rs"), ""));
        while h.step(src, Duration::from_secs(1)) {}
        assert_eq!(h.done(), src.len());
        (0..src.len())
            .map(|i| h.spans(i).unwrap().to_vec())
            .collect()
    }

    #[test]
    fn step_to_stops_at_the_line_asked_for() {
        let src = stateful_lines(1000);
        let want = full_pass(&src);
        let mut h = LineHighlighter::new(find_syntax(Path::new("x.rs"), ""));
        assert!(h.needs(1, src.len()));
        assert!(!h.step_to(&src, 300, Duration::from_secs(5)));
        assert_eq!(h.done(), 300);
        assert!(!h.needs(300, src.len()) && h.needs(301, src.len()));
        assert!(h.spans(300).is_none() && h.spans(5000).is_none());
        // Nothing to do for a target already reached.
        assert!(!h.step_to(&src, 200, Duration::from_secs(5)));
        assert_eq!(h.done(), 300);
        // A target past the end means the end.
        assert!(!h.step_to(&src, usize::MAX, Duration::from_secs(5)));
        assert_eq!(h.done(), src.len());
        assert!(!h.needs(usize::MAX, src.len()));
        for (i, w) in want.iter().enumerate() {
            assert_eq!(h.spans(i).unwrap(), &w[..], "line {i}");
        }
    }

    /// However little time a call gets, it does a bounded amount of work
    /// (so input is never held up) and still makes progress.
    #[test]
    fn one_step_without_budget_is_small_but_not_nothing() {
        let src = stateful_lines(1000);
        let mut h = LineHighlighter::new(find_syntax(Path::new("x.rs"), ""));
        let mut calls = 0;
        while h.step_to(&src, 500, Duration::ZERO) {
            calls += 1;
            assert_eq!(h.done(), calls * 16);
        }
        assert_eq!(h.done(), 500);
    }

    /// Colouring in pieces — any targets, any invalidations in between —
    /// ends with the same colours as one pass over the final text.
    #[test]
    fn piecewise_highlighting_matches_one_pass() {
        let mut src = stateful_lines(700);
        let mut h = LineHighlighter::new(find_syntax(Path::new("x.rs"), ""));
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut rnd = |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n as u64) as usize
        };
        for round in 0..200 {
            let upto = rnd(src.len() + 50);
            let budget = if round % 3 == 0 {
                Duration::ZERO
            } else {
                Duration::from_secs(1)
            };
            let more = h.step_to(&src, upto, budget);
            assert_eq!(more, h.needs(upto, src.len()));
            if round % 4 == 1 {
                // Edit a line (sometimes opening or closing a comment).
                let at = rnd(src.len());
                src[at] = match rnd(3) {
                    0 => "/* opened".to_string(),
                    1 => "closed */ fn g() {}".to_string(),
                    _ => format!("const C{round}: u8 = {round};"),
                };
                let before = h.done();
                h.invalidate_from(at);
                assert!(h.done() <= before && (before <= at || h.done() <= at));
            }
            // Whatever is coloured so far is what one pass would give.
            if round % 20 == 19 {
                let want = full_pass(&src);
                for (i, w) in want.iter().enumerate().take(h.done()) {
                    assert_eq!(h.spans(i).unwrap(), &w[..], "round {round} line {i}");
                }
                assert!(h.spans(h.done()).is_none());
            }
        }
        while h.step(&src, Duration::from_secs(1)) {}
        let want = full_pass(&src);
        for (i, w) in want.iter().enumerate() {
            assert_eq!(h.spans(i).unwrap(), &w[..], "line {i}");
        }
    }

    #[test]
    fn neighbouring_spans_of_one_colour_are_one_span() {
        let src = lines("let s = \"text\"; // note");
        let mut h = LineHighlighter::new(find_syntax(Path::new("x.rs"), ""));
        h.step(&src, Duration::from_secs(1));
        let spans = h.spans(0).unwrap();
        let string: Vec<_> = spans
            .iter()
            .filter(|s| s.color == theme::p().syntax_string)
            .collect();
        assert_eq!(string.len(), 1, "{spans:?}");
        assert_eq!((string[0].start, string[0].end), (8, 14));
        for w in spans.windows(2) {
            assert!(
                w[0].end < w[1].start || (w[0].color, w[0].bold) != (w[1].color, w[1].bold),
                "{spans:?}"
            );
        }
    }

    #[test]
    fn plain_text_needs_no_work() {
        let mut h = LineHighlighter::new(find_syntax(Path::new("a.txt"), ""));
        assert!(h.is_plain());
        assert!(!h.step(&lines("a\nb"), Duration::from_millis(1)));
    }
}
