//! Edit operations, position transforms and undo/redo (lite-xl doc undo).
//!
//! Every edit is an [`Op`]; applying one returns its inverse. A user action
//! (typing a character, pasting, deleting a selection) becomes one undo group
//! holding the inverse ops plus the selections before and after. Consecutive
//! typing within [`MERGE_WINDOW`] merges into one group, as in lite-xl.

use std::time::{Duration, Instant};

use crate::editor::buffer::{Buffer, Chunk, Pos};

pub const MERGE_WINDOW: Duration = Duration::from_millis(300);
pub const MAX_UNDO: usize = 10_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    Insert { at: Pos, chunk: Chunk },
    Remove { at: Pos, chunk: Chunk },
}

impl Op {
    /// Apply to the buffer; returns the inverse op.
    pub fn apply(&self, buf: &mut Buffer) -> Op {
        match self {
            Op::Insert { at, chunk } => {
                let at = buf.clamp(*at);
                buf.insert(at, chunk);
                Op::Remove {
                    at,
                    chunk: chunk.clone(),
                }
            }
            Op::Remove { at, chunk } => {
                let at = buf.clamp(*at);
                let end = chunk.end_from(at);
                let removed = buf.remove(at, end);
                Op::Insert { at, chunk: removed }
            }
        }
    }
}

/// Move a position to account for an op that was just applied.
/// `bias_after`: a position exactly at an insertion point moves after it.
pub fn transform(p: Pos, op: &Op, bias_after: bool) -> Pos {
    match op {
        Op::Insert { at, chunk } => {
            if p < *at || (p == *at && !bias_after) {
                return p;
            }
            let end = chunk.end_from(*at);
            if p.line == at.line {
                // Same line, at or after the insertion point.
                Pos::new(end.line, end.col + (p.col - at.col))
            } else {
                Pos::new(p.line + (end.line - at.line), p.col)
            }
        }
        Op::Remove { at, chunk } => {
            let end = chunk.end_from(*at);
            if p <= *at {
                p
            } else if p <= end {
                *at
            } else if p.line == end.line {
                Pos::new(at.line, at.col + (p.col - end.col))
            } else {
                Pos::new(p.line - (end.line - at.line), p.col)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sel {
    pub anchor: Pos,
    pub head: Pos,
}

impl Sel {
    pub fn caret(p: Pos) -> Self {
        Self { anchor: p, head: p }
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    pub fn range(&self) -> (Pos, Pos) {
        crate::editor::buffer::order(self.anchor, self.head)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupKind {
    Typing,
    Other,
}

#[derive(Clone, Debug)]
struct Group {
    /// Unique id; the document is clean when the top group's id matches
    /// the id recorded at save time.
    id: u64,
    /// Inverse ops, in the order the edits were applied.
    inverse: Vec<Op>,
    sels_before: Vec<Sel>,
    sels_after: Vec<Sel>,
    kind: GroupKind,
    time: Instant,
}

#[derive(Default)]
pub struct History {
    undo: Vec<Group>,
    redo: Vec<Group>,
    next_id: u64,
    /// State id with nothing left to undo. 0 = as loaded; once old groups
    /// are dropped (MAX_UNDO) it is the state after the last dropped group,
    /// so undoing everything can't look like the saved file.
    base: u64,
}

impl History {
    pub fn record(
        &mut self,
        inverse: Vec<Op>,
        sels_before: Vec<Sel>,
        sels_after: Vec<Sel>,
        kind: GroupKind,
    ) {
        if inverse.is_empty() {
            return;
        }
        self.redo.clear();
        let now = Instant::now();
        if kind == GroupKind::Typing
            && let Some(last) = self.undo.last_mut()
            && last.kind == GroupKind::Typing
            && now.duration_since(last.time) < MERGE_WINDOW
        {
            last.inverse.extend(inverse);
            last.sels_after = sels_after;
            last.time = now;
            return;
        }
        self.next_id += 1;
        self.undo.push(Group {
            id: self.next_id,
            inverse,
            sels_before,
            sels_after,
            kind,
            time: now,
        });
        if self.undo.len() > MAX_UNDO {
            let dropped = self.undo.remove(0);
            self.base = dropped.id;
        }
    }

    /// Break typing merges (e.g. after the caret moves).
    pub fn seal(&mut self) {
        if let Some(last) = self.undo.last_mut() {
            last.kind = GroupKind::Other;
        }
    }

    /// Undo one group; returns the selections to restore.
    pub fn undo(&mut self, buf: &mut Buffer) -> Option<Vec<Sel>> {
        let g = self.undo.pop()?;
        let mut redo_ops = Vec::with_capacity(g.inverse.len());
        for op in g.inverse.iter().rev() {
            redo_ops.push(op.apply(buf));
        }
        redo_ops.reverse();
        let sels = g.sels_before.clone();
        self.redo.push(Group {
            inverse: redo_ops,
            ..g
        });
        Some(sels)
    }

    pub fn redo(&mut self, buf: &mut Buffer) -> Option<Vec<Sel>> {
        let g = self.redo.pop()?;
        let mut undo_ops = Vec::with_capacity(g.inverse.len());
        for op in g.inverse.iter() {
            undo_ops.push(op.apply(buf));
        }
        let sels = g.sels_after.clone();
        self.undo.push(Group {
            inverse: undo_ops,
            kind: GroupKind::Other,
            ..g
        });
        Some(sels)
    }

    /// Id of the state the document is in (0 = as loaded).
    pub fn state_id(&self) -> u64 {
        self.undo.last().map_or(self.base, |g| g.id)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::buffer::Eol;

    #[test]
    fn transform_through_insert_and_remove() {
        let ins = Op::Insert {
            at: Pos::new(1, 2),
            chunk: Chunk::typed("ab\ncd", Eol::Lf),
        };
        assert_eq!(transform(Pos::new(1, 1), &ins, true), Pos::new(1, 1));
        assert_eq!(transform(Pos::new(1, 2), &ins, false), Pos::new(1, 2));
        assert_eq!(transform(Pos::new(1, 2), &ins, true), Pos::new(2, 2));
        assert_eq!(transform(Pos::new(1, 5), &ins, true), Pos::new(2, 5));
        assert_eq!(transform(Pos::new(3, 0), &ins, true), Pos::new(4, 0));
        let rem = Op::Remove {
            at: Pos::new(1, 2),
            chunk: Chunk::typed("ab\ncd", Eol::Lf),
        };
        assert_eq!(transform(Pos::new(2, 1), &rem, true), Pos::new(1, 2)); // inside → start
        assert_eq!(transform(Pos::new(2, 4), &rem, true), Pos::new(1, 4));
        assert_eq!(transform(Pos::new(5, 1), &rem, true), Pos::new(4, 1));
    }

    #[test]
    fn dropping_old_groups_never_fakes_the_loaded_state() {
        let mut h = History::default();
        let mut b = Buffer::from_text("");
        assert_eq!(h.state_id(), 0);
        for i in 0..MAX_UNDO + 5 {
            let op = Op::Insert {
                at: Pos::new(0, 0),
                chunk: Chunk::typed("x", Eol::Lf),
            };
            let inv = op.apply(&mut b);
            h.record(vec![inv], vec![], vec![], GroupKind::Other);
            let _ = i;
        }
        while h.undo(&mut b).is_some() {}
        assert_ne!(h.state_id(), 0, "text still differs from the loaded file");
        assert_eq!(b.to_text().len(), 5);
    }

    #[test]
    fn undo_redo_round_trip() {
        let mut b = Buffer::from_text("one\r\ntwo");
        let mut h = History::default();
        let op = Op::Remove {
            at: Pos::new(0, 1),
            chunk: b.slice(Pos::new(0, 1), Pos::new(1, 1)),
        };
        let inv = op.apply(&mut b);
        assert_eq!(b.to_text(), "owo");
        h.record(vec![inv], vec![], vec![], GroupKind::Other);
        h.undo(&mut b);
        assert_eq!(b.to_text(), "one\r\ntwo");
        h.redo(&mut b);
        assert_eq!(b.to_text(), "owo");
        h.undo(&mut b);
        assert_eq!(b.to_text(), "one\r\ntwo");
    }

    #[test]
    fn typing_merges_within_window() {
        let mut b = Buffer::default();
        let mut h = History::default();
        for (i, c) in "abc".chars().enumerate() {
            let inv = Op::Insert {
                at: Pos::new(0, i),
                chunk: Chunk::typed(&c.to_string(), Eol::Lf),
            }
            .apply(&mut b);
            h.record(vec![inv], vec![], vec![], GroupKind::Typing);
        }
        h.undo(&mut b);
        assert_eq!(b.to_text(), "", "one undo removes the whole burst");
    }
}
