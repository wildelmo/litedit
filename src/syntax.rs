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
pub fn comment_tokens(syntax_name: &str) -> Option<(&'static str, Option<&'static str>)> {
    let n = syntax_name.to_ascii_lowercase();
    let line = |t: &'static str| Some((t, None));
    let has = |k: &str| n.contains(k);
    if has("html") || has("xml") || has("markdown") || has("svg") || n == "vue" {
        return Some(("<!--", Some("-->")));
    }
    if n == "css" || has("scss") || has("less") {
        return Some(("/*", Some("*/")));
    }
    if has("python")
        || has("shell")
        || has("bash")
        || has("yaml")
        || has("toml")
        || has("ruby")
        || has("perl")
        || has("makefile")
        || has("dockerfile")
        || has("powershell")
        || has("r ")
        || n == "r"
        || has("cmake")
        || has("nim")
        || has("julia")
        || has("elixir")
        || has("nix")
        || has("graphql")
        || has("git")
        || has("ssh")
        || has("crystal")
        || has("tcl")
        || n == "fish"
    {
        return line("#");
    }
    if n == "lua" || has("sql") || has("haskell") || has("elm") || has("ada") {
        return line("--");
    }
    if has("lisp")
        || has("clojure")
        || has("scheme")
        || has("racket")
        || has("ini")
        || has("assembly")
        || has("asm")
    {
        return line(";");
    }
    if has("batch") {
        return line("REM");
    }
    if has("vim") {
        return line("\"");
    }
    if has("latex") || has("tex") || has("erlang") || has("matlab") {
        return line("%");
    }
    if n == "plain text" || n.is_empty() {
        return None;
    }
    // C family and most others (Rust, C, C++, C#, Java, JavaScript,
    // TypeScript, Go, Swift, Kotlin, Dart, PHP, JSON with comments, …).
    line("//")
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
/// default colour throughout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: u32,
    pub end: u32,
    pub color: Color,
    pub bold: bool,
    pub italic: bool,
}

pub struct LineHighlighter {
    syntax: &'static SyntaxReference,
    plain: bool,
    highlighter: Highlighter<'static>,
    done: Vec<Vec<Span>>,
    parse: ParseState,
    state: HighlightState,
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
            done: Vec::new(),
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
        self.done.len()
    }

    pub fn is_plain(&self) -> bool {
        self.plain
    }

    /// Spans for line `i`, if it has been highlighted.
    pub fn spans(&self, i: usize) -> Option<&[Span]> {
        self.done.get(i).map(Vec::as_slice)
    }

    /// Highlight more lines for up to `budget`. Returns true if lines remain.
    pub fn step(&mut self, lines: &[String], budget: Duration) -> bool {
        if self.plain {
            return false;
        }
        let start = Instant::now();
        let mut text = String::new();
        while self.done.len() < lines.len() {
            let i = self.done.len();
            if i % CHECKPOINT_EVERY == 0 && self.checkpoints.len() == i / CHECKPOINT_EVERY {
                self.checkpoints
                    .push((self.parse.clone(), self.state.clone()));
            }
            let line = &lines[i];
            let spans = if line.len() > MAX_HIGHLIGHT_BYTES {
                Vec::new()
            } else {
                text.clear();
                text.push_str(line);
                text.push('\n');
                self.highlight_one(&text, line.len())
            };
            self.done.push(spans);
            if self.done.len() % 16 == 0 && start.elapsed() >= budget {
                break;
            }
        }
        self.done.len() < lines.len()
    }

    fn highlight_one(&mut self, text: &str, line_len: usize) -> Vec<Span> {
        let ops = match self.parse.parse_line(text, syntax_set()) {
            Ok(ops) => ops,
            Err(_) => {
                // Broken grammar state: reset and show this line plain.
                self.parse = ParseState::new(self.syntax);
                self.state = HighlightState::new(&self.highlighter, ScopeStack::new());
                return Vec::new();
            }
        };
        let normal = theme::p().syntax_normal;
        let mut spans = Vec::new();
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
            spans.push(Span {
                start: start as u32,
                end: end as u32,
                color,
                bold,
                italic,
            });
        }
        spans
    }

    /// Forget highlighting from line `line` onwards (after an edit) and
    /// resume from the nearest checkpoint at or before it.
    #[allow(dead_code)] // used by the editor (Epic 2)
    pub fn invalidate_from(&mut self, line: usize) {
        if self.plain || line >= self.done.len() {
            return;
        }
        let cp = (line / CHECKPOINT_EVERY).min(self.checkpoints.len().saturating_sub(1));
        if let Some((p, s)) = self.checkpoints.get(cp).cloned() {
            self.parse = p;
            self.state = s;
            self.done.truncate(cp * CHECKPOINT_EVERY);
            self.checkpoints.truncate(cp + 1);
        } else {
            *self = Self::new(self.syntax);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let color_at = |spans: &[Span], b: u32| {
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

    #[test]
    fn plain_text_needs_no_work() {
        let mut h = LineHighlighter::new(find_syntax(Path::new("a.txt"), ""));
        assert!(h.is_plain());
        assert!(!h.step(&lines("a\nb"), Duration::from_millis(1)));
    }
}
