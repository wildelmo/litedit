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

fn file_meta(p: &Path) -> Option<(u64, std::time::SystemTime)> {
    let m = std::fs::metadata(p).ok()?;
    Some((m.len(), m.modified().ok()?))
}

fn text_hash(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
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
    /// Saving needs confirmation: the file could not be decoded exactly.
    pub lossy: bool,
    /// Hash of the text as last read from or written to disk (to tell a
    /// real external change from our own save or a touch).
    disk_hash: u64,
    /// Size and modification time at that moment: if both are unchanged the
    /// file isn't re-read at all (saves stay fast for big files).
    disk_meta: Option<(u64, std::time::SystemTime)>,
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
            lossy: doc.lossy,
            disk_hash: text_hash(&doc.text),
            disk_meta: doc.meta,
        }
    }

    /// Size and mtime unchanged since we last read or wrote the file? Only
    /// trusted when the time has sub-second precision (NTFS); FAT/exFAT USB
    /// drives store 2-second or 10 ms times, so a quick same-size change
    /// could hide there — those are always compared by content.
    pub fn disk_unchanged_fast(&self) -> bool {
        let precise = self.disk_meta.is_some_and(|(_, t)| {
            t.duration_since(std::time::UNIX_EPOCH)
                .is_ok_and(|d| d.subsec_nanos() % 10_000_000 != 0)
        });
        precise && file_meta(&self.path) == self.disk_meta
    }

    /// Is `disk_text` what we last read or wrote (or what we're showing)?
    pub fn disk_matches(&self, disk_text: &str) -> bool {
        text_hash(disk_text) == self.disk_hash || disk_text == self.buf.to_text()
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
    pub fn save_as(&mut self, path: &Path, allow_lossy: bool) -> Result<usize, SaveError> {
        let n = self.save_to(path, allow_lossy)?;
        self.path = path.to_path_buf();
        Ok(n)
    }

    fn save_to(&mut self, path: &Path, allow_lossy: bool) -> Result<usize, SaveError> {
        if self.lossy && !allow_lossy {
            return Err(SaveError::NeedsLossyConfirm);
        }
        let text = self.buf.to_text();
        let bytes = crate::doc::encode(&text, self.encoding);
        crate::editor::save::save_bytes(path, &bytes).map_err(SaveError::Failed)?;
        if self.encoding == Encoding::Utf8Lossy {
            self.encoding = Encoding::Utf8; // now it is valid UTF-8
        }
        self.lossy = false;
        self.disk_hash = text_hash(&text);
        self.disk_meta = file_meta(path);
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
        self.lossy = doc.lossy;
        self.disk_hash = text_hash(&doc.text);
        self.disk_meta = doc.meta;
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
        if self.sels.iter().all(|s| s.anchor == s.head) {
            // No selection: insert an indent unit (spaces up to the next
            // stop). With a selection, indent the lines (lite-xl) — never
            // replace the selected text.
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
        self.one_selection_per_line_block();
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

    /// Replace several ranges (any order, non-overlapping) as ONE undo step.
    /// Used by replace / replace-all.
    pub fn replace_ranges(&mut self, mut edits: Vec<(Pos, Pos, String)>) {
        if edits.is_empty() {
            return;
        }
        edits.sort_by_key(|e| std::cmp::Reverse(e.0));
        let before = self.sels.clone();
        let eol = self.buf.default_eol();
        let mut inverse = Vec::with_capacity(edits.len() * 2);
        let mut last_caret = None;
        for (a, b, text) in edits {
            let (a, b) = (self.buf.clamp(a), self.buf.clamp(b));
            let mut ops = Vec::new();
            if a != b {
                ops.push(Op::Remove {
                    at: a,
                    chunk: self.buf.slice(a, b),
                });
            }
            let chunk = Chunk::typed(&text, eol);
            if !chunk.is_empty() {
                ops.push(Op::Insert {
                    at: a,
                    chunk: chunk.clone(),
                });
            }
            for op in ops {
                inverse.push(op.apply(&mut self.buf));
                for s in &mut self.sels {
                    s.anchor = transform(s.anchor, &op, true);
                    s.head = transform(s.head, &op, true);
                }
            }
            if last_caret.is_none() {
                last_caret = Some(chunk.end_from(a));
            }
        }
        // Leave the caret after the first (top-most processed last) edit.
        if let Some(c) = last_caret
            && self.sels.len() == 1
        {
            let c = self.buf.clamp(c);
            self.sels[0] = Sel::caret(c);
        }
        let after = self.sels.clone();
        self.hist.record(inverse, before, after, GroupKind::Other);
        self.version += 1;
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
        self.one_selection_per_line_block();
        self.edit_each(GroupKind::Other, |buf, sel| {
            let (a, b) = sel.range();
            let b = last_line_of(a, b);
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

    /// Ctrl+click: add a caret at `p` (or remove the caret already there).
    pub fn add_caret(&mut self, p: Pos) {
        self.hist.seal();
        let p = self.buf.clamp(p);
        if let Some(i) = self.sels.iter().position(|s| s.is_empty() && s.head == p) {
            if self.sels.len() > 1 {
                self.sels.remove(i);
            }
            return;
        }
        self.sels.push(Sel::caret(p));
        self.merge_sels();
    }

    /// Back to one selection (Escape with several carets).
    pub fn single_selection(&mut self) -> bool {
        if self.sels.len() < 2 {
            return false;
        }
        // Like VS Code: back to the primary (first) caret.
        self.sels.truncate(1);
        true
    }

    /// The selected text of `sel` if it lies on one line.
    fn one_line_text(&self, sel: Sel) -> Option<String> {
        let (a, b) = sel.range();
        (a.line == b.line && a != b).then(|| self.buf.line(a.line)[a.col..b.col].to_string())
    }

    /// Occurrences of `needle` (case-sensitive, within lines) in order.
    fn occurrences(&self, needle: &str) -> Vec<Sel> {
        let mut out = Vec::new();
        if needle.is_empty() {
            return out;
        }
        for (i, line) in self.buf.lines().iter().enumerate() {
            let mut from = 0;
            while let Some(k) = line[from..].find(needle) {
                let a = from + k;
                out.push(Sel {
                    anchor: Pos::new(i, a),
                    head: Pos::new(i, a + needle.len()),
                });
                from = a + needle.len();
                if out.len() >= 10_000 {
                    return out;
                }
            }
        }
        out
    }

    /// Ctrl+D (VS Code): with only a caret, select the word under it; then
    /// add the next occurrence of the selected text as another selection.
    pub fn select_next_occurrence(&mut self) {
        self.hist.seal();
        let last = *self.sels.last().unwrap_or(&self.sels[0]);
        if last.is_empty() {
            let i = self.sels.len() - 1;
            let keep: Vec<Sel> = self.sels[..i].to_vec();
            self.sels = vec![last];
            self.select_word_at(last.head);
            let word = self.sels[0];
            self.sels = keep;
            self.sels.push(word);
            return;
        }
        let Some(needle) = self.one_line_text(last) else {
            return;
        };
        let all = self.occurrences(&needle);
        let after = last.range().1;
        let next = all
            .iter()
            .find(|o| o.anchor >= after)
            .or_else(|| all.first())
            .copied();
        if let Some(n) = next
            && !self.sels.iter().any(|s| s.range() == n.range())
        {
            // Keep the newest selection last (Ctrl+D continues from it).
            self.sels.push(n);
            let newest = n;
            self.merge_sels();
            if let Some(i) = self.sels.iter().position(|s| s.range() == newest.range()) {
                let s = self.sels.remove(i);
                self.sels.push(s);
            }
        }
    }

    /// Ctrl+Shift+L: select every occurrence of the selected text (or of
    /// the word under the caret).
    pub fn select_all_occurrences(&mut self) {
        self.hist.seal();
        let mut cur = *self.sels.last().unwrap_or(&self.sels[0]);
        if cur.is_empty() {
            self.sels = vec![cur];
            self.select_word_at(cur.head);
            cur = self.sels[0];
        }
        let Some(needle) = self.one_line_text(cur) else {
            return;
        };
        let all = self.occurrences(&needle);
        if !all.is_empty() {
            self.sels = all;
        }
    }

    /// Whole-line commands act on each line once: when several selections
    /// touch the same lines (two carets on one line), they become one
    /// selection per block of lines. A lone selection is left as it is.
    fn one_selection_per_line_block(&mut self) {
        let blocks = self.selected_blocks();
        if blocks.len() == self.sels.len() {
            return;
        }
        self.sels = blocks
            .into_iter()
            .map(|(a, b)| Sel {
                anchor: Pos::new(a, 0),
                head: Pos::new(b, self.buf.line(b).len()),
            })
            .collect();
    }

    /// Line blocks covered by the selections (merged, ascending).
    fn selected_blocks(&self) -> Vec<(usize, usize)> {
        let mut v: Vec<(usize, usize)> = self
            .sels
            .iter()
            .map(|s| {
                let (a, b) = s.range();
                (a.line, last_line_of(a, b).line)
            })
            .collect();
        v.sort_unstable();
        let mut out: Vec<(usize, usize)> = Vec::new();
        for (a, b) in v {
            match out.last_mut() {
                Some(l) if a <= l.1 + 1 => l.1 = l.1.max(b),
                _ => out.push((a, b)),
            }
        }
        out
    }

    /// Alt+Up / Alt+Down: move the selected lines, keeping line endings.
    pub fn move_lines(&mut self, up: bool) {
        let blocks = self.selected_blocks();
        let n = self.buf.line_count();
        if blocks.is_empty()
            || (up && blocks[0].0 == 0)
            || (!up && blocks.last().is_some_and(|b| b.1 + 1 >= n))
        {
            return;
        }
        let before = self.sels.clone();
        let len = |buf: &Buffer, l: usize| buf.line(l).len();
        let mut ops = Vec::new();
        for &(a, b) in blocks.iter().rev() {
            let block = self
                .buf
                .slice(Pos::new(a, 0), Pos::new(b, len(&self.buf, b)));
            let (start, end, chunk) = if up {
                let prev = self.buf.slice(Pos::new(a - 1, 0), Pos::new(a, 0));
                let mut lines = block.lines.clone();
                lines.push(prev.lines[0].clone());
                let mut eols = block.eols.clone();
                eols.push(prev.eols[0]);
                (
                    Pos::new(a - 1, 0),
                    Pos::new(b, len(&self.buf, b)),
                    Chunk { lines, eols },
                )
            } else {
                let next = self.buf.slice(
                    Pos::new(b, len(&self.buf, b)),
                    Pos::new(b + 1, len(&self.buf, b + 1)),
                );
                let mut lines = vec![next.lines[1].clone()];
                lines.extend(block.lines.clone());
                let mut eols = vec![next.eols[0]];
                eols.extend(block.eols.clone());
                (
                    Pos::new(a, 0),
                    Pos::new(b + 1, len(&self.buf, b + 1)),
                    Chunk { lines, eols },
                )
            };
            ops.push(Op::Remove {
                at: start,
                chunk: self.buf.slice(start, end),
            });
            ops.push(Op::Insert { at: start, chunk });
        }
        // Shift every selection with its lines. An end at column 0 of the
        // line after a block (Shift+Down selections) moves with it; on the
        // last line there is no next line, so it becomes the end of the
        // moved block's last line instead.
        let old_len: Vec<usize> = self.buf.lines().iter().map(String::len).collect();
        let shift = |p: Pos| {
            if up {
                Pos::new(p.line - 1, p.col)
            } else if p.line + 1 < n {
                Pos::new(p.line + 1, p.col)
            } else {
                Pos::new(p.line, old_len[p.line.saturating_sub(1)])
            }
        };
        for s in &mut self.sels {
            s.anchor = shift(s.anchor);
            s.head = shift(s.head);
        }
        self.apply(ops, GroupKind::Other, before);
    }

    /// Ctrl+/: comment or uncomment the selected lines with `start` (and
    /// `end` for block-style languages like HTML).
    pub fn toggle_comment(&mut self, start: &str, end: Option<&str>) {
        let blocks = self.selected_blocks();
        let lines: Vec<usize> = blocks.iter().flat_map(|&(a, b)| a..=b).collect();
        let body = |l: &str| l.trim_start().to_string();
        let non_empty: Vec<usize> = lines
            .iter()
            .copied()
            .filter(|&l| !self.buf.line(l).trim().is_empty())
            .collect();
        if non_empty.is_empty() {
            return;
        }
        // Commented = starts with `start` and (block style) ends with
        // `end`, the two not overlapping ("/*/" is not a comment).
        let commented = non_empty.iter().all(|&l| {
            let t = body(self.buf.line(l));
            let t = t.trim_end();
            t.starts_with(start)
                && end.is_none_or(|e| t.len() >= start.len() + e.len() && t.ends_with(e))
        });
        let indent = non_empty
            .iter()
            .map(|&l| self.buf.line(l).len() - self.buf.line(l).trim_start().len())
            .min()
            .unwrap_or(0);
        let (start, end) = (start.to_string(), end.map(str::to_string));
        self.for_selected_lines(|buf, line| {
            let text = buf.line(line);
            if text.trim().is_empty() {
                return Vec::new();
            }
            let lead = text.len() - text.trim_start().len();
            let mut ops = Vec::new();
            if commented {
                // Ranges to remove: the start token (+ one space) and the end
                // token (+ one space before it), never overlapping.
                let mut s1 = lead + start.len();
                if text[s1..].starts_with(' ') {
                    s1 += 1;
                }
                let mut tail: Option<(usize, usize)> = None;
                if let Some(e) = &end {
                    let t = text.trim_end();
                    if t.len() >= e.len() && t.ends_with(e.as_str()) {
                        let k = t.len() - e.len();
                        let k0 = if k > 0 && t[..k].ends_with(' ') {
                            k - 1
                        } else {
                            k
                        };
                        let e0 = k0.max(s1.min(k));
                        let s1c = s1.min(k);
                        s1 = s1c;
                        if e0 < t.len() {
                            tail = Some((e0, t.len()));
                        }
                    }
                }
                if let Some((e0, e1)) = tail {
                    ops.push(Op::Remove {
                        at: Pos::new(line, e0),
                        chunk: buf.slice(Pos::new(line, e0), Pos::new(line, e1)),
                    });
                }
                if s1 > lead {
                    ops.push(Op::Remove {
                        at: Pos::new(line, lead),
                        chunk: buf.slice(Pos::new(line, lead), Pos::new(line, s1)),
                    });
                }
            } else {
                if let Some(e) = &end {
                    let at = Pos::new(line, text.trim_end().len());
                    ops.push(Op::Insert {
                        at,
                        chunk: Chunk::typed(&format!(" {e}"), Eol::Lf),
                    });
                }
                ops.push(Op::Insert {
                    at: Pos::new(line, indent.min(lead)),
                    chunk: Chunk::typed(&format!("{start} "), Eol::Lf),
                });
            }
            ops
        });
    }

    /// Replace the whole text in one undo step (whole-document commands).
    /// Selections are kept where they were, clamped to the new text.
    fn replace_all(&mut self, new_text: &str) {
        if self.buf.to_text() == new_text {
            return;
        }
        let before = self.sels.clone();
        let ops = vec![
            Op::Remove {
                at: Pos::default(),
                chunk: self.buf.slice(Pos::default(), self.buf.end()),
            },
            Op::Insert {
                at: Pos::default(),
                chunk: Chunk::parse(new_text),
            },
        ];
        let mut inverse = Vec::with_capacity(2);
        for op in ops {
            inverse.push(op.apply(&mut self.buf));
        }
        for s in &mut self.sels {
            s.anchor = self.buf.clamp(s.anchor);
            s.head = self.buf.clamp(s.head);
        }
        let after = self.sels.clone();
        self.hist.record(inverse, before, after, GroupKind::Other);
        self.version += 1;
        self.goal_col = None;
    }

    /// Rebuild the text line by line, keeping each line's own ending.
    fn map_lines(&mut self, f: impl Fn(&str) -> String) {
        let n = self.buf.line_count();
        let mut out = String::with_capacity(self.buf.lines().iter().map(|l| l.len() + 2).sum());
        for i in 0..n {
            out.push_str(&f(self.buf.line(i)));
            if i + 1 < n {
                let eol = self
                    .buf
                    .slice(Pos::new(i, self.buf.line(i).len()), Pos::new(i + 1, 0));
                out.push_str(eol.eols.first().map_or("\n", |e| e.as_str()));
            }
        }
        self.replace_all(&out);
    }

    /// "Line Endings: LF / CRLF": every line ending becomes `eol`.
    pub fn set_line_endings(&mut self, eol: Eol) {
        let text = self.buf.lines().join(eol.as_str());
        self.replace_all(&text);
        self.buf.set_default_eol(eol);
    }

    /// Remove spaces and tabs at the ends of lines.
    pub fn trim_trailing_whitespace(&mut self) {
        self.map_lines(|l| l.trim_end_matches([' ', '\t']).to_string());
    }

    /// "Indentation: Convert to Spaces / Tabs" (leading whitespace only).
    pub fn convert_indentation(&mut self, to: Indent) {
        let width = match self.indent {
            Indent::Spaces(n) => n,
            Indent::Tab => 4,
        };
        let unit = match to {
            Indent::Spaces(n) => n,
            Indent::Tab => width,
        };
        self.map_lines(|l| {
            let lead: String = l.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
            let mut cols = 0;
            for c in lead.chars() {
                cols = if c == '\t' {
                    cols - cols % width + width
                } else {
                    cols + 1
                };
            }
            let new_lead = match to {
                Indent::Spaces(_) => " ".repeat(cols),
                Indent::Tab => format!("{}{}", "\t".repeat(cols / unit), " ".repeat(cols % unit)),
            };
            format!("{new_lead}{}", &l[lead.len()..])
        });
        self.indent = to;
    }

    /// Upper / lower case the selections (the word at a caret if nothing is
    /// selected).
    pub fn change_case(&mut self, upper: bool) {
        if self.sels.iter().all(Sel::is_empty) && self.sels.len() == 1 {
            let p = self.sels[0].head;
            self.select_word_at(p);
        }
        self.edit_each(GroupKind::Other, |buf, sel| {
            let (a, b) = sel.range();
            if a == b {
                return (Vec::new(), None);
            }
            let old = buf.slice(a, b);
            let new = Chunk {
                lines: old
                    .lines
                    .iter()
                    .map(|l| {
                        if upper {
                            l.to_uppercase()
                        } else {
                            l.to_lowercase()
                        }
                    })
                    .collect(),
                eols: old.eols.clone(),
            };
            (
                vec![
                    Op::Remove { at: a, chunk: old },
                    Op::Insert { at: a, chunk: new },
                ],
                None,
            )
        });
    }

    /// Ctrl+J: join the selected lines (or the line with the next one),
    /// with one space instead of the line break and leading indent.
    pub fn join_lines(&mut self) {
        self.edit_each(GroupKind::Other, |buf, sel| {
            let (a, b) = sel.range();
            let last = b.line.max(a.line + 1).min(buf.line_count() - 1);
            let mut ops = Vec::new();
            for l in (a.line..last).rev() {
                let end = Pos::new(l, buf.line(l).len());
                let next = buf.line(l + 1);
                let lead = next.len() - next.trim_start().len();
                let to = Pos::new(l + 1, lead);
                ops.push(Op::Remove {
                    at: end,
                    chunk: buf.slice(end, to),
                });
                let sep = if buf.line(l).ends_with(' ')
                    || next.trim().is_empty()
                    || buf.line(l).is_empty()
                {
                    ""
                } else {
                    " "
                };
                if !sep.is_empty() {
                    ops.push(Op::Insert {
                        at: end,
                        chunk: Chunk::typed(sep, Eol::Lf),
                    });
                }
            }
            (ops, None)
        });
    }

    /// Ctrl+Enter / Ctrl+Shift+Enter: start a new line below / above the
    /// current one, keeping its indentation.
    pub fn new_line_beside(&mut self, above: bool) {
        let eol = self.buf.default_eol();
        self.edit_each(GroupKind::Other, |buf, sel| {
            let line = sel.head.line;
            let text = buf.line(line);
            let indent: String = text
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect();
            if above {
                let chunk = Chunk {
                    lines: vec![indent.clone(), String::new()],
                    eols: vec![eol],
                };
                (
                    vec![Op::Insert {
                        at: Pos::new(line, 0),
                        chunk,
                    }],
                    Some(Pos::new(line, indent.len())),
                )
            } else {
                let at = Pos::new(line, text.len());
                let chunk = Chunk {
                    lines: vec![String::new(), indent.clone()],
                    eols: vec![eol],
                };
                (
                    vec![Op::Insert { at, chunk }],
                    Some(Pos::new(line + 1, indent.len())),
                )
            }
        });
    }

    /// Ctrl+Shift+Up / Down: add a caret on the line above / below the
    /// newest one (same column where possible).
    pub fn add_caret_vertical(&mut self, up: bool) {
        self.hist.seal();
        let last = *self.sels.last().unwrap_or(&self.sels[0]);
        let h = last.head;
        let line = if up {
            match h.line.checked_sub(1) {
                Some(l) => l,
                None => return,
            }
        } else if h.line + 1 < self.buf.line_count() {
            h.line + 1
        } else {
            return;
        };
        // Keep the first caret's column across shorter lines (VS Code).
        let first = self.sels[0].head;
        let chars = self.buf.line(first.line)[..first.col].chars().count();
        let target = self.buf.line(line);
        let col = target
            .char_indices()
            .nth(chars)
            .map_or(target.len(), |(i, _)| i);
        let p = Pos::new(line, col);
        if !self.sels.iter().any(|s| s.head == p) {
            self.sels.push(Sel::caret(p));
        }
    }

    /// Duplicate the current line(s) below.
    pub fn duplicate_lines(&mut self) {
        self.one_selection_per_line_block();
        let eol = self.buf.default_eol();
        self.edit_each(GroupKind::Other, |buf, sel| {
            let (a, b) = sel.range();
            let b = last_line_of(a, b);
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

/// A selection ending at column 0 of a later line (Ctrl+L, Shift+Down)
/// doesn't include that line: its end is the previous line's end.
fn last_line_of(a: Pos, b: Pos) -> Pos {
    if b.col == 0 && b.line > a.line {
        Pos::new(b.line - 1, 0)
    } else {
        b
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

    #[test]
    fn ctrl_d_selects_word_then_next_occurrences_and_typing_edits_all() {
        let mut d = doc("let x = 1;\nx += x;\n");
        d.set_caret(Pos::new(0, 4), false);
        d.select_next_occurrence(); // the word "x"
        assert_eq!(d.selected_text(), "x");
        d.select_next_occurrence();
        d.select_next_occurrence();
        assert_eq!(d.sels.len(), 3);
        d.insert_text("y");
        assert_eq!(d.buf.to_text(), "let y = 1;\ny += y;\n");
        assert!(d.undo());
        assert_eq!(d.buf.to_text(), "let x = 1;\nx += x;\n", "one undo step");
        // Ctrl+Shift+L: all at once; wraps and never duplicates.
        let mut d = doc("ab ab ab");
        d.set_caret(Pos::new(0, 0), false);
        d.select_all_occurrences();
        assert_eq!(d.sels.len(), 3);
        for _ in 0..5 {
            d.select_next_occurrence();
        }
        assert_eq!(d.sels.len(), 3);
        assert!(d.single_selection() && d.sels.len() == 1);
    }

    #[test]
    fn move_lines_keeps_text_endings_and_selection() {
        let mut d = doc("a\r\nb\nc");
        d.set_caret(Pos::new(1, 1), false);
        d.move_lines(true);
        assert_eq!(d.buf.to_text(), "b\r\na\nc");
        assert_eq!(d.primary().head, Pos::new(0, 1));
        d.move_lines(true); // already at the top
        assert_eq!(d.buf.to_text(), "b\r\na\nc");
        d.set_caret(Pos::new(1, 0), false);
        d.move_lines(false); // onto the last line without a newline
        assert_eq!(d.buf.to_text(), "b\r\nc\na");
        assert!(d.undo());
        assert_eq!(d.buf.to_text(), "b\r\na\nc");
        // A selection ending at column 0 of the last line, moved down.
        let mut d = doc("a\nb\nc");
        d.set_caret(Pos::new(1, 0), false);
        d.set_caret(Pos::new(2, 0), true);
        d.move_lines(false);
        assert_eq!(d.buf.to_text(), "a\nc\nb");
        for s in &d.sels {
            assert!(s.anchor.line < 3 && s.head.line < 3, "{:?}", d.sels);
        }
        // A two-line selection moves as a block.
        let mut d = doc("1\n2\n3\n4\n");
        d.set_caret(Pos::new(1, 0), false);
        d.set_caret(Pos::new(2, 1), true);
        d.move_lines(false);
        assert_eq!(d.buf.to_text(), "1\n4\n2\n3\n");
    }

    #[test]
    fn toggle_comment_adds_and_removes_at_a_common_indent() {
        let mut d = doc("fn a() {\n    x();\n\n    y();\n}\n");
        d.set_caret(Pos::new(1, 0), false);
        d.set_caret(Pos::new(3, 2), true);
        d.toggle_comment("//", None);
        assert_eq!(d.buf.to_text(), "fn a() {\n    // x();\n\n    // y();\n}\n");
        d.toggle_comment("//", None);
        assert_eq!(d.buf.to_text(), "fn a() {\n    x();\n\n    y();\n}\n");
        // Lines too short to hold both tokens never panic.
        for t in ["/*/", "/* */", "/**/", "<!-->", "<!-- -->", "<!---->"] {
            let mut d = doc(t);
            d.toggle_comment(
                if t.starts_with('<') { "<!--" } else { "/*" },
                Some(if t.starts_with('<') { "-->" } else { "*/" }),
            );
            d.toggle_comment(
                if t.starts_with('<') { "<!--" } else { "/*" },
                Some(if t.starts_with('<') { "-->" } else { "*/" }),
            );
            assert!(d.undo() || true);
        }
        let mut d = doc("<p>hi</p>");
        d.toggle_comment("<!--", Some("-->"));
        assert_eq!(d.buf.to_text(), "<!-- <p>hi</p> -->");
        d.toggle_comment("<!--", Some("-->"));
        assert_eq!(d.buf.to_text(), "<p>hi</p>");
    }

    #[test]
    fn whole_document_commands_keep_line_endings_and_undo_in_one_step() {
        let mut d = doc("a  \r\n\tb\t\nc ");
        d.trim_trailing_whitespace();
        assert_eq!(d.buf.to_text(), "a\r\n\tb\nc");
        assert!(d.undo());
        assert_eq!(d.buf.to_text(), "a  \r\n\tb\t\nc ");
        let mut d = doc("x\r\ny\nz");
        d.set_line_endings(Eol::Lf);
        assert_eq!(d.buf.to_text(), "x\ny\nz");
        d.set_line_endings(Eol::CrLf);
        assert_eq!(d.buf.to_text(), "x\r\ny\r\nz");
        let mut d = doc("\tif x {\n\t\ty();\n\t}\n");
        d.convert_indentation(Indent::Spaces(4));
        assert_eq!(d.buf.to_text(), "    if x {\n        y();\n    }\n");
        d.convert_indentation(Indent::Tab);
        assert_eq!(d.buf.to_text(), "\tif x {\n\t\ty();\n\t}\n");
    }

    #[test]
    fn case_join_and_new_lines() {
        let mut d = doc("hello world");
        d.set_caret(Pos::new(0, 1), false);
        d.change_case(true);
        assert_eq!(d.buf.to_text(), "HELLO world");
        let mut d = doc("a(\n    b,\n    c)\n");
        d.join_lines();
        assert_eq!(d.buf.to_text(), "a( b,\n    c)\n");
        d.select_all();
        d.join_lines();
        assert_eq!(d.buf.to_text(), "a( b, c)");
        let mut d = doc("    x = 1\ny");
        d.set_caret(Pos::new(0, 3), false);
        d.new_line_beside(false);
        d.insert_text("z");
        assert_eq!(d.buf.to_text(), "    x = 1\n    z\ny");
        d.new_line_beside(true);
        d.insert_text("w");
        assert_eq!(d.buf.to_text(), "    x = 1\n    w\n    z\ny");
        let mut d = doc("abc\nd\nefg");
        d.set_caret(Pos::new(0, 2), false);
        d.add_caret_vertical(false);
        d.add_caret_vertical(false);
        assert_eq!(d.sels.len(), 3);
        d.insert_text("|");
        assert_eq!(d.buf.to_text(), "ab|c\nd|\nef|g");
    }

    #[test]
    fn line_commands_with_two_carets_on_one_line_touch_that_line_once() {
        let mut d = doc("one two three\nkeep me\nlast\n");
        d.set_caret(Pos::new(0, 1), false);
        d.add_caret(Pos::new(0, 5));
        let (t, line_mode) = d.copy_text();
        assert!(line_mode);
        assert_eq!(t, "one two three\n");
        d.delete_lines();
        assert_eq!(d.buf.to_text(), "keep me\nlast\n");
        let mut d = doc("ab\ncd\n");
        d.set_caret(Pos::new(0, 0), false);
        d.add_caret(Pos::new(0, 1));
        d.duplicate_lines();
        assert_eq!(d.buf.to_text(), "ab\nab\ncd\n");
        let mut d = doc("x + x\nkeep\n");
        d.set_caret(Pos::new(0, 0), false);
        d.select_next_occurrence();
        d.select_next_occurrence();
        assert_eq!(d.sels.len(), 2);
        d.delete_lines();
        assert_eq!(d.buf.to_text(), "keep\n");
        let mut d = doc("a\nb\n");
        d.set_caret(Pos::new(1, 0), false);
        d.add_caret(Pos::new(1, 1));
        d.paste_lines("new\n");
        assert_eq!(d.buf.to_text(), "a\nnew\nb\n");
    }

    #[test]
    fn ctrl_click_adds_and_removes_carets() {
        let mut d = doc("abc\ndef");
        d.set_caret(Pos::new(0, 1), false);
        d.add_caret(Pos::new(1, 2));
        assert_eq!(d.sels.len(), 2);
        d.insert_text("-");
        assert_eq!(d.buf.to_text(), "a-bc\nde-f");
        d.add_caret(Pos::new(1, 3)); // the caret that is there: removed
        assert_eq!(d.sels.len(), 1);
    }

    #[test]
    fn line_commands_respect_a_selection_ending_at_column_zero() {
        // Ctrl+L on line 1 selects "1\n" (ends at 2:0): only line 1 counts.
        let mut d = doc("1\n2\n3\n4\n");
        d.select_line_at(0);
        d.delete_lines();
        assert_eq!(d.buf.to_text(), "2\n3\n4\n");
        let mut d = doc("1\n2\n3\n4\n");
        d.select_line_at(0);
        d.duplicate_lines();
        assert_eq!(d.buf.to_text(), "1\n1\n2\n3\n4\n");
        // Shift+Down selection over two lines ending at column 0.
        let mut d = doc("a\nb\nc\nd\n");
        d.set_caret(Pos::new(1, 0), false);
        d.set_caret(Pos::new(3, 0), true);
        d.delete_lines();
        assert_eq!(d.buf.to_text(), "a\nd\n");
    }

    #[test]
    fn tab_on_a_selection_indents_instead_of_replacing() {
        let mut d = doc("hello world");
        d.select_all();
        d.indent_lines();
        assert_eq!(d.buf.to_text(), "    hello world");
        let mut d = doc("x\nlast");
        d.set_caret(Pos::new(1, 0), false);
        d.set_caret(Pos::new(1, 2), true);
        d.indent_lines();
        assert_eq!(d.buf.to_text(), "x\n    last");
        // No selection: still inserts spaces at the caret.
        let mut d = doc("ab");
        d.set_caret(Pos::new(0, 1), false);
        d.indent_lines();
        assert_eq!(d.buf.to_text(), "a   b");
    }

    fn doc(text: &str) -> EditDoc {
        EditDoc::new(Document {
            path: PathBuf::from("t.txt"),
            text: text.into(),
            encoding: Encoding::Utf8,
            lossy: false,
            meta: None,
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
