//! An editable document: buffer + selections + undo history + save state.
//!
//! All editing commands live here (not in the view) so they are unit-tested
//! without rendering. Commands work on every selection, processed from the
//! end of the document backwards, with positions transformed after each op —
//! multiple carets come for free later.

use std::path::{Path, PathBuf};

use unicode_segmentation::UnicodeSegmentation;

use crate::doc::{Document, Encoding};
use crate::editor::buffer::{Buffer, Chunk, Eol, Pos, order};
use crate::editor::history::{GroupKind, History, Op, Sel, transform};

/// lite-xl `config.non_word_chars`.
const NON_WORD: &str = " \t\n/\\()\"':,.;<>~!@#$%^&*|+=[]{}`?-";

pub fn is_word_char(c: char) -> bool {
    !NON_WORD.contains(c) && !c.is_whitespace()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Indent {
    Spaces(usize),
    Tab,
}

impl Indent {
    pub fn unit(self) -> String {
        match self {
            Indent::Spaces(n) => " ".repeat(n),
            Indent::Tab => "\t".into(),
        }
    }
}

/// Guess the file's indentation (lite-xl `detectindent` plugin, simplified).
pub fn detect_indent(lines: &[String]) -> Indent {
    let (mut tabs, mut spaces) = (0usize, [0usize; 9]);
    let mut prev = 0usize;
    for l in lines.iter().take(2000) {
        if l.starts_with('\t') {
            tabs += 1;
            continue;
        }
        let n = l.chars().take_while(|c| *c == ' ').count();
        if n > 0 && n < l.len() {
            let d = n.abs_diff(prev);
            if (2..=8).contains(&d) {
                spaces[d] += 1;
            }
        }
        if !l.trim().is_empty() {
            prev = n;
        }
    }
    let best = (2..=8).max_by_key(|&d| (spaces[d], d == 4)).unwrap_or(4);
    if tabs > spaces.iter().sum::<usize>() {
        Indent::Tab
    } else if spaces[best] == 0 {
        Indent::Spaces(4)
    } else {
        Indent::Spaces(best)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SaveError {
    /// The file had invalid UTF-8; saving would write the replacements.
    NeedsLossyConfirm,
    Failed(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    Left,
    Right,
    Up,
    Down,
    WordLeft,
    WordRight,
    /// Smart home: first non-blank, then column 0.
    Home,
    End,
    DocStart,
    DocEnd,
    /// Up/down by this many lines.
    PageUp(usize),
    PageDown(usize),
}

pub struct EditDoc {
    pub buf: Buffer,
    pub sels: Vec<Sel>,
    hist: History,
    pub path: PathBuf,
    pub encoding: Encoding,
    pub indent: Indent,
    /// Increments on every change (caches key off it).
    version: u64,
    /// History state at the last save (None = modified no matter what,
    /// e.g. the file was deleted on disk).
    saved_state: Option<u64>,
    /// Remembered column (in chars) for up/down movement.
    goal_col: Option<usize>,
}

impl EditDoc {
    pub fn new(doc: Document) -> Self {
        let buf = Buffer::from_text(&doc.text);
        let indent = detect_indent(buf.lines());
        Self {
            buf,
            sels: vec![Sel::default()],
            hist: History::default(),
            path: doc.path,
            encoding: doc.encoding,
            indent,
            version: 0,
            saved_state: Some(0),
            goal_col: None,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    /// Unsaved changes? Undoing back to the saved text makes it clean again.
    pub fn is_dirty(&self) -> bool {
        self.saved_state != Some(self.hist.state_id())
    }

    pub fn mark_saved(&mut self) {
        self.hist.seal();
        self.saved_state = Some(self.hist.state_id());
    }

    /// Save to the current path. Files that were not valid UTF-8 need
    /// `allow_lossy` (the invalid bytes were replaced when opened).
    pub fn save(&mut self, allow_lossy: bool) -> Result<usize, SaveError> {
        let path = self.path.clone();
        self.save_to(&path, allow_lossy)
    }

    /// Save to a new path and switch to it.
    pub fn save_as(&mut self, path: &Path) -> Result<usize, SaveError> {
        let n = self.save_to(path, true)?;
        self.path = path.to_path_buf();
        Ok(n)
    }

    fn save_to(&mut self, path: &Path, allow_lossy: bool) -> Result<usize, SaveError> {
        if self.encoding == Encoding::Utf8Lossy && !allow_lossy {
            return Err(SaveError::NeedsLossyConfirm);
        }
        let bytes = crate::doc::encode(&self.buf.to_text(), self.encoding);
        crate::editor::save::save_bytes(path, &bytes).map_err(SaveError::Failed)?;
        if self.encoding == Encoding::Utf8Lossy {
            self.encoding = Encoding::Utf8; // now it is valid UTF-8
        }
        self.mark_saved();
        Ok(bytes.len())
    }

    /// Mark as modified (e.g. the file was deleted on disk).
    pub fn force_dirty(&mut self) {
        self.saved_state = None;
    }

    pub fn primary(&self) -> Sel {
        self.sels[0]
    }

    /// Replace the whole text (reload from disk), keeping carets in range.
    pub fn replace_all_from_disk(&mut self, doc: Document) {
        self.buf = Buffer::from_text(&doc.text);
        self.encoding = doc.encoding;
        self.hist = History::default();
        for s in &mut self.sels {
            s.anchor = self.buf.clamp(s.anchor);
            s.head = self.buf.clamp(s.head);
        }
        self.version += 1;
        self.saved_state = Some(self.hist.state_id());
    }

    // ---- low level ----------------------------------------------------

    fn apply(&mut self, ops: Vec<Op>, kind: GroupKind, sels_before: Vec<Sel>) {
        if ops.is_empty() {
            return;
        }
        let mut inverse = Vec::with_capacity(ops.len());
        for op in ops {
            inverse.push(op.apply(&mut self.buf));
        }
        let after = self.sels.clone();
        self.hist.record(inverse, sels_before, after, kind);
        self.version += 1;
        self.goal_col = None;
    }

    /// Run `f(doc, sel_index)` for each selection from last to first; each
    /// call returns ops to apply immediately (positions of all selections are
    /// transformed through them).
    fn edit_each(
        &mut self,
        kind: GroupKind,
        mut f: impl FnMut(&Buffer, Sel) -> (Vec<Op>, Option<Pos>),
    ) {
        let before = self.sels.clone();
        let mut order_idx: Vec<usize> = (0..self.sels.len()).collect();
        order_idx.sort_by_key(|&i| std::cmp::Reverse(self.sels[i].range().0));
        let mut all_inverse = Vec::new();
        for i in order_idx {
            let (ops, caret) = f(&self.buf, self.sels[i]);
            for op in ops {
                let inv = op.apply(&mut self.buf);
                for (j, s) in self.sels.iter_mut().enumerate() {
                    if j != i {
                        s.anchor = transform(s.anchor, &op, true);
                        s.head = transform(s.head, &op, true);
                    }
                }
                all_inverse.push(inv);
            }
            if let Some(c) = caret {
                self.sels[i] = Sel::caret(self.buf.clamp(c));
            }
        }
        if !all_inverse.is_empty() {
            let after = self.sels.clone();
            self.hist.record(all_inverse, before, after, kind);
            self.version += 1;
        }
        self.goal_col = None;
    }

    // ---- text commands --------------------------------------------------

    /// Type or paste text at every selection (replacing selected text).
    pub fn insert_text(&mut self, text: &str) {
        let chunk = Chunk::typed(text, self.buf.default_eol());
        let typing = chunk.lines.len() == 1
            && text.chars().count() == 1
            && self.sels.iter().all(Sel::is_empty)
            && !text.starts_with(char::is_whitespace);
        let kind = if typing {
            GroupKind::Typing
        } else {
            GroupKind::Other
        };
        self.edit_each(kind, |buf, sel| {
            let (a, b) = sel.range();
            let mut ops = Vec::new();
            if a != b {
                ops.push(Op::Remove {
                    at: a,
                    chunk: buf.slice(a, b),
                });
            }
            ops.push(Op::Insert {
                at: a,
                chunk: chunk.clone(),
            });
            (ops, Some(chunk.end_from(a)))
        });
    }

    /// Enter: new line keeping the current line's indentation.
    pub fn newline(&mut self) {
        let eol = self.buf.default_eol();
        self.edit_each(GroupKind::Other, |buf, sel| {
            let (a, b) = sel.range();
            let line = buf.line(a.line);
            let indent: String = line
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect();
            let indent = &indent[..indent.len().min(a.col)];
            let mut ops = Vec::new();
            if a != b {
                ops.push(Op::Remove {
                    at: a,
                    chunk: buf.slice(a, b),
                });
            }
            let chunk = Chunk {
                lines: vec![String::new(), indent.to_string()],
                eols: vec![eol],
            };
            let end = chunk.end_from(a);
            ops.push(Op::Insert { at: a, chunk });
            (ops, Some(end))
        });
    }

    /// Backspace (or Ctrl+Backspace for a word).
    pub fn backspace(&mut self, word: bool) {
        self.delete(true, word);
    }

    /// Delete (or Ctrl+Delete for a word).
    pub fn delete_forward(&mut self, word: bool) {
        self.delete(false, word);
    }

    fn delete(&mut self, back: bool, word: bool) {
        let indent = self.indent;
        self.edit_each(GroupKind::Other, |buf, sel| {
            let (mut a, mut b) = sel.range();
            if a == b {
                if back {
                    // Backspace in leading spaces removes one indent level.
                    let line = buf.line(a.line);
                    let lead = &line[..a.col];
                    a = if !word && !lead.is_empty() && lead.chars().all(|c| c == ' ') {
                        let unit = match indent {
                            Indent::Spaces(n) => n,
                            Indent::Tab => 1,
                        };
                        let rem = a.col % unit;
                        Pos::new(a.line, a.col - if rem == 0 { unit.min(a.col) } else { rem })
                    } else {
                        moved(buf, a, if word { Move::WordLeft } else { Move::Left }, None).0
                    };
                } else {
                    b = moved(
                        buf,
                        b,
                        if word { Move::WordRight } else { Move::Right },
                        None,
                    )
                    .0;
                }
            }
            if a == b {
                return (Vec::new(), None);
            }
            (
                vec![Op::Remove {
                    at: a,
                    chunk: buf.slice(a, b),
                }],
                Some(a),
            )
        });
    }

    /// Tab: indent selected lines, or insert one indent at the caret.
    pub fn indent_lines(&mut self) {
        let unit = self.indent.unit();
        if self.sels.iter().all(|s| s.anchor.line == s.head.line) {
            // Insert an indent unit (spaces up to the next stop).
            let indent = self.indent;
            self.edit_each(GroupKind::Other, |buf, sel| {
                let (a, b) = sel.range();
                let mut ops = Vec::new();
                if a != b {
                    ops.push(Op::Remove {
                        at: a,
                        chunk: buf.slice(a, b),
                    });
                }
                let text = match indent {
                    Indent::Spaces(n) => " ".repeat(n - a.col % n),
                    Indent::Tab => "\t".into(),
                };
                let chunk = Chunk::typed(&text, Eol::Lf);
                let end = chunk.end_from(a);
                ops.push(Op::Insert { at: a, chunk });
                (ops, Some(end))
            });
            return;
        }
        self.for_selected_lines(|buf, line| {
            if buf.line(line).is_empty() {
                Vec::new()
            } else {
                vec![Op::Insert {
                    at: Pos::new(line, 0),
                    chunk: Chunk::typed(&unit, Eol::Lf),
                }]
            }
        });
    }

    /// Shift+Tab: remove one indent level from selected lines.
    pub fn outdent_lines(&mut self) {
        let indent = self.indent;
        self.for_selected_lines(|buf, line| {
            let l = buf.line(line);
            let n = if l.starts_with('\t') {
                1
            } else {
                let max = match indent {
                    Indent::Spaces(n) => n,
                    Indent::Tab => 4,
                };
                l.chars().take(max).take_while(|c| *c == ' ').count()
            };
            if n == 0 {
                Vec::new()
            } else {
                vec![Op::Remove {
                    at: Pos::new(line, 0),
                    chunk: buf.slice(Pos::new(line, 0), Pos::new(line, n)),
                }]
            }
        });
    }

    /// Apply per-line ops to every line touched by a selection, keeping the
    /// selections over the same text.
    fn for_selected_lines(&mut self, mut f: impl FnMut(&Buffer, usize) -> Vec<Op>) {
        let before = self.sels.clone();
        let mut lines: Vec<usize> = Vec::new();
        for s in &self.sels {
            let (a, b) = s.range();
            let last = if b.col == 0 && b.line > a.line {
                b.line - 1
            } else {
                b.line
            };
            lines.extend(a.line..=last);
        }
        lines.sort_unstable();
        lines.dedup();
        let mut inverse = Vec::new();
        for &line in lines.iter().rev() {
            for op in f(&self.buf, line) {
                inverse.push(op.apply(&mut self.buf));
                for s in &mut self.sels {
                    s.anchor = transform(s.anchor, &op, s.anchor.col != 0 || s.anchor == s.head);
                    s.head = transform(s.head, &op, s.head.col != 0 || s.anchor == s.head);
                }
            }
        }
        if !inverse.is_empty() {
            let after = self.sels.clone();
            self.hist.record(inverse, before, after, GroupKind::Other);
            self.version += 1;
        }
    }

    pub fn undo(&mut self) -> bool {
        match self.hist.undo(&mut self.buf) {
            Some(sels) => {
                self.sels = sels.into_iter().map(|s| self.clamp_sel(s)).collect();
                if self.sels.is_empty() {
                    self.sels.push(Sel::default());
                }
                self.version += 1;
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self) -> bool {
        match self.hist.redo(&mut self.buf) {
            Some(sels) => {
                self.sels = sels.into_iter().map(|s| self.clamp_sel(s)).collect();
                if self.sels.is_empty() {
                    self.sels.push(Sel::default());
                }
                self.version += 1;
                true
            }
            None => false,
        }
    }

    fn clamp_sel(&self, s: Sel) -> Sel {
        Sel {
            anchor: self.buf.clamp(s.anchor),
            head: self.buf.clamp(s.head),
        }
    }

    // ---- selection commands --------------------------------------------

    /// Move every caret; `extend` keeps the anchor (Shift).
    pub fn move_carets(&mut self, m: Move, extend: bool) {
        self.hist.seal();
        let vertical = matches!(
            m,
            Move::Up | Move::Down | Move::PageUp(_) | Move::PageDown(_)
        );
        let goal = if vertical { self.goal_col } else { None };
        let mut new_goal = None;
        for s in &mut self.sels {
            // Collapsing a selection with Left/Right jumps to its edge.
            if !extend && !s.is_empty() && matches!(m, Move::Left | Move::Right) {
                let (a, b) = s.range();
                *s = Sel::caret(if m == Move::Left { a } else { b });
                continue;
            }
            let (p, g) = moved(&self.buf, s.head, m, goal);
            new_goal = g;
            s.head = p;
            if !extend {
                s.anchor = p;
            }
        }
        self.goal_col = if vertical { new_goal } else { None };
        self.merge_sels();
    }

    pub fn set_caret(&mut self, p: Pos, extend: bool) {
        self.hist.seal();
        let p = self.buf.clamp(p);
        let anchor = if extend { self.sels[0].anchor } else { p };
        self.sels = vec![Sel { anchor, head: p }];
        self.goal_col = None;
    }

    pub fn select_all(&mut self) {
        self.sels = vec![Sel {
            anchor: Pos::default(),
            head: self.buf.end(),
        }];
    }

    /// Select the word (or run of separators) around `p`.
    pub fn select_word_at(&mut self, p: Pos) {
        let p = self.buf.clamp(p);
        let line = self.buf.line(p.line);
        let is_w = |c: char| is_word_char(c);
        let here = line[p.col..]
            .chars()
            .next()
            .or_else(|| line[..p.col].chars().next_back());
        let Some(kind) = here.map(is_w) else {
            self.set_caret(p, false);
            return;
        };
        let start = line[..p.col]
            .char_indices()
            .rev()
            .take_while(|(_, c)| is_w(*c) == kind)
            .last()
            .map_or(p.col, |(i, _)| i);
        let end = line[p.col..]
            .char_indices()
            .take_while(|(_, c)| is_w(*c) == kind)
            .last()
            .map_or(p.col, |(i, c)| p.col + i + c.len_utf8());
        // Word edges must not cut a grapheme cluster.
        let start = snap_grapheme(line, start);
        let end = {
            let s2 = snap_grapheme(line, end);
            if s2 == end {
                end
            } else {
                next_grapheme(line, s2)
            }
        };
        self.sels = vec![Sel {
            anchor: Pos::new(p.line, start),
            head: Pos::new(p.line, end),
        }];
    }

    /// Select whole line(s) including the line break.
    pub fn select_line_at(&mut self, line: usize) {
        let line = line.min(self.buf.line_count() - 1);
        let head = if line + 1 < self.buf.line_count() {
            Pos::new(line + 1, 0)
        } else {
            Pos::new(line, self.buf.line(line).len())
        };
        self.sels = vec![Sel {
            anchor: Pos::new(line, 0),
            head,
        }];
    }

    /// Extend the primary selection's head (mouse drag).
    pub fn drag_to(&mut self, p: Pos) {
        let p = self.buf.clamp(p);
        self.sels.truncate(1);
        self.sels[0].head = p;
    }

    fn merge_sels(&mut self) {
        if self.sels.len() < 2 {
            return;
        }
        self.sels.sort_by_key(|s| s.range().0);
        self.sels.dedup_by(|b, a| {
            b.range().0 <= a.range().1 && {
                let (a0, a1) = a.range();
                let (_, b1) = b.range();
                *a = Sel {
                    anchor: a0,
                    head: a1.max(b1),
                };
                true
            }
        });
    }

    /// The selected text with `\n` line breaks (clipboard). Empty when
    /// nothing is selected.
    pub fn selected_text(&self) -> String {
        let mut parts = Vec::new();
        for s in &self.sels {
            if !s.is_empty() {
                let (a, b) = s.range();
                parts.push(self.buf.slice(a, b).to_text_lf());
            }
        }
        parts.join("\n")
    }

    /// Text for copy/cut: the selection, or whole line(s) when nothing is
    /// selected (returns `true` for line mode, as in lite-xl).
    pub fn copy_text(&self) -> (String, bool) {
        if self.sels.iter().any(|s| !s.is_empty()) {
            return (self.selected_text(), false);
        }
        let mut lines: Vec<usize> = self.sels.iter().map(|s| s.head.line).collect();
        lines.sort_unstable();
        lines.dedup();
        let mut t = String::new();
        for l in lines {
            t.push_str(self.buf.line(l));
            t.push('\n');
        }
        (t, true)
    }

    /// Paste whole lines above each caret (after a line-mode copy).
    pub fn paste_lines(&mut self, text: &str) {
        let chunk = Chunk::typed(text, self.buf.default_eol());
        self.edit_each(GroupKind::Other, |_, sel| {
            let at = Pos::new(sel.head.line, 0);
            let op = Op::Insert {
                at,
                chunk: chunk.clone(),
            };
            let caret = transform(sel.head, &op, true);
            (vec![op], Some(caret))
        });
    }

    pub fn delete_selection(&mut self) {
        self.edit_each(GroupKind::Other, |buf, sel| {
            let (a, b) = sel.range();
            if a == b {
                (Vec::new(), None)
            } else {
                (
                    vec![Op::Remove {
                        at: a,
                        chunk: buf.slice(a, b),
                    }],
                    Some(a),
                )
            }
        });
    }

    /// Remove the whole line under each caret (cut a line with no selection).
    pub fn delete_lines(&mut self) {
        self.edit_each(GroupKind::Other, |buf, sel| {
            let (a, b) = sel.range();
            let start = Pos::new(a.line, 0);
            let end = if b.line + 1 < buf.line_count() {
                Pos::new(b.line + 1, 0)
            } else if a.line > 0 {
                // Last line: also remove the break before it.
                return (
                    vec![Op::Remove {
                        at: Pos::new(a.line - 1, buf.line(a.line - 1).len()),
                        chunk: buf.slice(
                            Pos::new(a.line - 1, buf.line(a.line - 1).len()),
                            Pos::new(b.line, buf.line(b.line).len()),
                        ),
                    }],
                    Some(Pos::new(a.line - 1, 0)),
                );
            } else {
                Pos::new(b.line, buf.line(b.line).len())
            };
            if start == end {
                return (Vec::new(), None);
            }
            (
                vec![Op::Remove {
                    at: start,
                    chunk: buf.slice(start, end),
                }],
                Some(start),
            )
        });
    }

    /// Duplicate the current line(s) below.
    pub fn duplicate_lines(&mut self) {
        let eol = self.buf.default_eol();
        self.edit_each(GroupKind::Other, |buf, sel| {
            let (a, b) = sel.range();
            let text: Vec<String> = (a.line..=b.line).map(|l| buf.line(l).to_string()).collect();
            let at = Pos::new(b.line, buf.line(b.line).len());
            let mut lines = vec![String::new()];
            lines.extend(text);
            let eols = vec![eol; lines.len() - 1];
            let n = b.line - a.line + 1;
            (
                vec![Op::Insert {
                    at,
                    chunk: Chunk { lines, eols },
                }],
                Some(Pos::new(sel.head.line + n, sel.head.col)),
            )
        });
    }
}

/// Snap a byte column to the grapheme-cluster boundary at or before it, so
/// a mouse click can never land inside an emoji sequence.
pub fn snap_grapheme(line: &str, col: usize) -> usize {
    let col = col.min(line.len());
    let mut last = 0;
    for (i, _) in line.grapheme_indices(true) {
        if i > col {
            break;
        }
        last = i;
    }
    if col == line.len() { col } else { last }
}

/// Grapheme boundary before `col` in `line` (or 0).
fn prev_grapheme(line: &str, col: usize) -> usize {
    line[..col]
        .grapheme_indices(true)
        .next_back()
        .map_or(0, |(i, _)| i)
}

/// Grapheme boundary after `col` in `line` (or line end).
fn next_grapheme(line: &str, col: usize) -> usize {
    line[col..]
        .graphemes(true)
        .next()
        .map_or(line.len(), |g| col + g.len())
}

fn char_col(line: &str, byte: usize) -> usize {
    line[..byte].chars().count()
}

fn byte_at_char_col(line: &str, chars: usize) -> usize {
    let b = line
        .char_indices()
        .nth(chars)
        .map_or(line.len(), |(i, _)| i);
    // Never land inside a grapheme cluster.
    let g = prev_grapheme(line, b);
    if next_grapheme(line, g) == b || b == line.len() {
        b
    } else {
        g
    }
}

/// Compute where a caret goes. Returns (position, goal column for vertical moves).
pub fn moved(buf: &Buffer, p: Pos, m: Move, goal: Option<usize>) -> (Pos, Option<usize>) {
    let p = buf.clamp(p);
    let line = buf.line(p.line);
    let last = buf.line_count() - 1;
    let vertical = |target: usize| {
        let g = goal.unwrap_or_else(|| char_col(line, p.col));
        let t = buf.line(target);
        (Pos::new(target, byte_at_char_col(t, g)), Some(g))
    };
    match m {
        Move::Left => {
            if p.col > 0 {
                (Pos::new(p.line, prev_grapheme(line, p.col)), None)
            } else if p.line > 0 {
                (Pos::new(p.line - 1, buf.line(p.line - 1).len()), None)
            } else {
                (p, None)
            }
        }
        Move::Right => {
            if p.col < line.len() {
                (Pos::new(p.line, next_grapheme(line, p.col)), None)
            } else if p.line < last {
                (Pos::new(p.line + 1, 0), None)
            } else {
                (p, None)
            }
        }
        Move::Up if p.line == 0 => (Pos::new(0, 0), None),
        Move::Up => vertical(p.line - 1),
        Move::Down if p.line == last => (Pos::new(last, line.len()), None),
        Move::Down => vertical(p.line + 1),
        Move::PageUp(n) => vertical(p.line.saturating_sub(n.max(1))),
        Move::PageDown(n) => vertical((p.line + n.max(1)).min(last)),
        Move::WordLeft => {
            if p.col == 0 {
                return moved(buf, p, Move::Left, None);
            }
            let before: Vec<(usize, char)> = line[..p.col].char_indices().collect();
            let mut i = before.len();
            while i > 0 && before[i - 1].1.is_whitespace() {
                i -= 1;
            }
            if i > 0 {
                let kind = is_word_char(before[i - 1].1);
                while i > 0
                    && !before[i - 1].1.is_whitespace()
                    && is_word_char(before[i - 1].1) == kind
                {
                    i -= 1;
                }
            }
            let col = before.get(i).map_or(0, |(b, _)| *b);
            (Pos::new(p.line, snap_grapheme(line, col)), None)
        }
        Move::WordRight => {
            if p.col == line.len() {
                return moved(buf, p, Move::Right, None);
            }
            let after: Vec<(usize, char)> = line[p.col..]
                .char_indices()
                .map(|(b, c)| (p.col + b, c))
                .collect();
            let mut i = 0;
            while i < after.len() && after[i].1.is_whitespace() {
                i += 1;
            }
            if i < after.len() {
                let kind = is_word_char(after[i].1);
                while i < after.len()
                    && !after[i].1.is_whitespace()
                    && is_word_char(after[i].1) == kind
                {
                    i += 1;
                }
            }
            let col = after.get(i).map_or(line.len(), |(b, _)| *b);
            // Never stop inside a cluster (e.g. a space + combining accent).
            let snapped = snap_grapheme(line, col);
            let col = if snapped == col {
                col
            } else {
                next_grapheme(line, snapped)
            };
            (Pos::new(p.line, col), None)
        }
        Move::Home => {
            let first = line.len() - line.trim_start_matches([' ', '\t']).len();
            (
                Pos::new(p.line, if p.col == first { 0 } else { first }),
                None,
            )
        }
        Move::End => (Pos::new(p.line, line.len()), None),
        Move::DocStart => (Pos::new(0, 0), None),
        Move::DocEnd => (buf.end(), None),
    }
}

/// Order helper re-exported for views.
pub fn sel_range(s: &Sel) -> (Pos, Pos) {
    order(s.anchor, s.head)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(text: &str) -> EditDoc {
        EditDoc::new(Document {
            path: PathBuf::from("t.txt"),
            text: text.into(),
            encoding: Encoding::Utf8,
        })
    }

    #[test]
    fn typing_backspace_and_undo() {
        let mut d = doc("");
        for c in ["h", "i", "!"] {
            d.insert_text(c);
        }
        assert_eq!(d.buf.to_text(), "hi!");
        assert!(d.is_dirty());
        d.backspace(false);
        assert_eq!(d.buf.to_text(), "hi");
        d.undo();
        assert_eq!(d.buf.to_text(), "hi!");
        d.undo();
        assert_eq!(d.buf.to_text(), "", "typing burst undoes together");
        d.redo();
        assert_eq!(d.buf.to_text(), "hi!");
    }

    #[test]
    fn undo_back_to_saved_text_is_clean() {
        let mut d = doc("abc");
        d.insert_text("x");
        assert!(d.is_dirty());
        d.undo();
        assert!(!d.is_dirty(), "back to the loaded text");
        d.redo();
        assert!(d.is_dirty());
        d.mark_saved();
        assert!(!d.is_dirty());
        d.undo();
        assert!(d.is_dirty(), "differs from the saved text");
        d.redo();
        assert!(!d.is_dirty());
        d.force_dirty();
        assert!(d.is_dirty());
    }

    #[test]
    fn selection_is_replaced_by_typing() {
        let mut d = doc("hello world");
        d.set_caret(Pos::new(0, 6), false);
        d.set_caret(Pos::new(0, 11), true);
        d.insert_text("there");
        assert_eq!(d.buf.to_text(), "hello there");
        assert_eq!(d.primary(), Sel::caret(Pos::new(0, 11)));
    }

    #[test]
    fn newline_keeps_indent_and_file_ending() {
        let mut d = doc("    fn x() {\r\n}");
        d.set_caret(Pos::new(0, 12), false);
        d.newline();
        assert_eq!(d.buf.to_text(), "    fn x() {\r\n    \r\n}");
        assert_eq!(d.primary().head, Pos::new(1, 4));
    }

    #[test]
    fn grapheme_movement_does_not_split_emoji() {
        let family = "👨\u{200d}👩\u{200d}👧";
        let mut d = doc(&format!("a{family}b"));
        d.set_caret(Pos::new(0, 1), false);
        d.move_carets(Move::Right, false);
        assert_eq!(d.primary().head.col, 1 + family.len());
        d.backspace(false);
        assert_eq!(d.buf.to_text(), "ab");
    }

    #[test]
    fn snapping_to_graphemes() {
        let s = "a👨\u{200d}👩b";
        assert_eq!(snap_grapheme(s, 0), 0);
        assert_eq!(snap_grapheme(s, 3), 1); // inside the family emoji
        assert_eq!(snap_grapheme(s, s.len()), s.len());
        assert_eq!(snap_grapheme(s, s.len() - 1), s.len() - 1); // before 'b'
    }

    #[test]
    fn word_moves_and_deletes() {
        let mut d = doc("let foo_bar = baz(1);");
        d.move_carets(Move::WordRight, false);
        assert_eq!(d.primary().head.col, 3);
        d.move_carets(Move::WordRight, false);
        assert_eq!(d.primary().head.col, 11);
        d.move_carets(Move::End, false);
        d.backspace(true);
        assert_eq!(d.buf.to_text(), "let foo_bar = baz(1");
        d.set_caret(Pos::new(0, 0), false);
        d.delete_forward(true);
        assert_eq!(d.buf.to_text(), " foo_bar = baz(1");
    }

    #[test]
    fn vertical_moves_remember_column() {
        let mut d = doc("long line here\nab\nanother long line");
        d.set_caret(Pos::new(0, 10), false);
        d.move_carets(Move::Down, false);
        assert_eq!(d.primary().head, Pos::new(1, 2));
        d.move_carets(Move::Down, false);
        assert_eq!(d.primary().head, Pos::new(2, 10));
    }

    #[test]
    fn smart_home_toggles() {
        let mut d = doc("    code");
        d.move_carets(Move::End, false);
        d.move_carets(Move::Home, false);
        assert_eq!(d.primary().head.col, 4);
        d.move_carets(Move::Home, false);
        assert_eq!(d.primary().head.col, 0);
    }

    #[test]
    fn indent_and_outdent_selected_lines() {
        let mut d = doc("a\nb\n\nc");
        d.indent = Indent::Spaces(4);
        d.set_caret(Pos::new(0, 0), false);
        d.set_caret(Pos::new(3, 1), true);
        d.indent_lines();
        assert_eq!(d.buf.to_text(), "    a\n    b\n\n    c");
        d.outdent_lines();
        assert_eq!(d.buf.to_text(), "a\nb\n\nc");
        d.undo();
        assert_eq!(d.buf.to_text(), "    a\n    b\n\n    c");
    }

    #[test]
    fn backspace_removes_indent_level_in_leading_spaces() {
        let mut d = doc("        x");
        d.indent = Indent::Spaces(4);
        d.set_caret(Pos::new(0, 8), false);
        d.backspace(false);
        assert_eq!(d.buf.to_text(), "    x");
    }

    #[test]
    fn detect_indent_styles() {
        let lines = |s: &str| s.lines().map(str::to_owned).collect::<Vec<_>>();
        assert_eq!(
            detect_indent(&lines("a\n  b\n    c\n  d")),
            Indent::Spaces(2)
        );
        assert_eq!(detect_indent(&lines("a\n\tb\n\t\tc")), Indent::Tab);
        assert_eq!(detect_indent(&lines("plain")), Indent::Spaces(4));
    }

    #[test]
    fn word_and_line_selection() {
        let mut d = doc("foo bar_baz qux\nnext");
        d.select_word_at(Pos::new(0, 6));
        assert_eq!(d.selected_text(), "bar_baz");
        d.select_line_at(0);
        assert_eq!(d.selected_text(), "foo bar_baz qux\n");
    }

    #[test]
    fn delete_and_duplicate_lines() {
        let mut d = doc("one\ntwo\nthree");
        d.set_caret(Pos::new(1, 1), false);
        d.duplicate_lines();
        assert_eq!(d.buf.to_text(), "one\ntwo\ntwo\nthree");
        d.delete_lines();
        assert_eq!(d.buf.to_text(), "one\ntwo\nthree");
        d.set_caret(Pos::new(2, 0), false);
        d.delete_lines();
        assert_eq!(d.buf.to_text(), "one\ntwo");
    }

    #[test]
    fn line_mode_copy_and_paste() {
        let mut d = doc("one\ntwo");
        d.set_caret(Pos::new(1, 2), false);
        let (t, line_mode) = d.copy_text();
        assert_eq!((t.as_str(), line_mode), ("two\n", true));
        d.set_caret(Pos::new(0, 1), false);
        d.paste_lines(&t);
        assert_eq!(d.buf.to_text(), "two\none\ntwo");
        assert_eq!(d.primary().head, Pos::new(1, 1));
        d.set_caret(Pos::new(0, 0), false);
        d.set_caret(Pos::new(0, 3), true);
        assert_eq!(d.copy_text(), ("two".to_string(), false));
    }

    #[test]
    fn multiple_carets_type_together() {
        let mut d = doc("a\nb\nc");
        d.sels = vec![
            Sel::caret(Pos::new(0, 1)),
            Sel::caret(Pos::new(1, 1)),
            Sel::caret(Pos::new(2, 1)),
        ];
        d.insert_text("!");
        assert_eq!(d.buf.to_text(), "a!\nb!\nc!");
        d.undo();
        assert_eq!(d.buf.to_text(), "a\nb\nc");
    }
}
