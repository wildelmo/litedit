//! Line diff for the compare view (VS Code's side-by-side diff editor):
//! which rows show which old/new lines, and the changed characters inside
//! a changed line. Uses the `similar` crate (Myers) with a deadline, so two
//! large unrelated files give a coarser diff instead of a frozen window.

use std::time::{Duration, Instant};

use similar::{Algorithm, DiffOp, TextDiff, capture_diff_slices_deadline};

/// Files above these limits are not compared (the view says so).
pub const MAX_BYTES: usize = 20 * 1024 * 1024;
pub const MAX_LINES: usize = 500_000;
/// Character-level highlighting is skipped for very long lines.
const MAX_INLINE_CHARS: usize = 4000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Same,
    /// Only on the old side.
    Removed,
    /// Only on the new side.
    Added,
    /// Old line replaced by new line (shown on one row).
    Changed,
}

/// One displayed row: an old line, a new line, or both.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub old: Option<usize>,
    pub new: Option<usize>,
    pub kind: Kind,
}

#[derive(Debug, Default)]
pub struct Model {
    pub rows: Vec<Row>,
    /// First row of each block of changes (F7 / Shift+F7 jump between them).
    pub hunks: Vec<usize>,
    /// The deadline hit: the diff is correct but may not be minimal.
    pub simplified: bool,
}

impl Model {
    pub fn identical(&self) -> bool {
        self.hunks.is_empty()
    }
}

/// Diff two files given as lines (without line endings).
pub fn compute(old: &[String], new: &[String], budget: Duration) -> Model {
    let start = Instant::now();
    let deadline = start + budget;
    let ops = capture_diff_slices_deadline(Algorithm::Myers, old, new, Some(deadline));
    let mut rows = Vec::with_capacity(old.len().max(new.len()));
    // Removed/added runs next to each other are paired up row by row.
    let mut pending_del: Vec<usize> = Vec::new();
    let mut pending_ins: Vec<usize> = Vec::new();
    let flush = |rows: &mut Vec<Row>, del: &mut Vec<usize>, ins: &mut Vec<usize>| {
        let n = del.len().max(ins.len());
        for k in 0..n {
            let (o, w) = (del.get(k).copied(), ins.get(k).copied());
            let kind = match (o, w) {
                (Some(_), Some(_)) => Kind::Changed,
                (Some(_), None) => Kind::Removed,
                _ => Kind::Added,
            };
            rows.push(Row {
                old: o,
                new: w,
                kind,
            });
        }
        del.clear();
        ins.clear();
    };
    for op in &ops {
        match *op {
            DiffOp::Equal {
                old_index,
                new_index,
                len,
            } => {
                flush(&mut rows, &mut pending_del, &mut pending_ins);
                for k in 0..len {
                    rows.push(Row {
                        old: Some(old_index + k),
                        new: Some(new_index + k),
                        kind: Kind::Same,
                    });
                }
            }
            DiffOp::Delete {
                old_index, old_len, ..
            } => pending_del.extend(old_index..old_index + old_len),
            DiffOp::Insert {
                new_index, new_len, ..
            } => pending_ins.extend(new_index..new_index + new_len),
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => {
                pending_del.extend(old_index..old_index + old_len);
                pending_ins.extend(new_index..new_index + new_len);
            }
        }
    }
    flush(&mut rows, &mut pending_del, &mut pending_ins);
    let hunks = rows
        .iter()
        .enumerate()
        .filter(|(i, r)| r.kind != Kind::Same && (*i == 0 || rows[i - 1].kind == Kind::Same))
        .map(|(i, _)| i)
        .collect();
    Model {
        rows,
        hunks,
        simplified: Instant::now() >= deadline,
    }
}

/// Changed byte ranges in a changed pair of lines: (old ranges, new ranges).
/// Small unchanged gaps between changes are merged so the highlight reads
/// as words, not confetti.
pub fn inline_changes(old: &str, new: &str) -> (Vec<(usize, usize)>, Vec<(usize, usize)>) {
    if old.chars().count() > MAX_INLINE_CHARS || new.chars().count() > MAX_INLINE_CHARS {
        return (vec![(0, old.len())], vec![(0, new.len())]);
    }
    let diff = TextDiff::configure()
        .algorithm(Algorithm::Myers)
        .timeout(Duration::from_millis(20))
        .diff_chars(old, new);
    let old_b: Vec<usize> = old
        .char_indices()
        .map(|(b, _)| b)
        .chain([old.len()])
        .collect();
    let new_b: Vec<usize> = new
        .char_indices()
        .map(|(b, _)| b)
        .chain([new.len()])
        .collect();
    let (mut o, mut n) = (Vec::new(), Vec::new());
    for op in diff.ops() {
        match *op {
            DiffOp::Equal { .. } => {}
            DiffOp::Delete {
                old_index, old_len, ..
            } => o.push((old_b[old_index], old_b[old_index + old_len])),
            DiffOp::Insert {
                new_index, new_len, ..
            } => n.push((new_b[new_index], new_b[new_index + new_len])),
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => {
                o.push((old_b[old_index], old_b[old_index + old_len]));
                n.push((new_b[new_index], new_b[new_index + new_len]));
            }
        }
    }
    (merge(o, 2), merge(n, 2))
}

/// Join ranges separated by at most `gap` bytes.
fn merge(mut v: Vec<(usize, usize)>, gap: usize) -> Vec<(usize, usize)> {
    v.sort_unstable();
    let mut out: Vec<(usize, usize)> = Vec::with_capacity(v.len());
    for (a, b) in v {
        match out.last_mut() {
            Some(last) if a <= last.1 + gap => last.1 = last.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

/// Split text into lines without line endings (CRLF, LF or CR).
pub fn lines(text: &str) -> Vec<String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut out: Vec<String> = text
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l).to_string())
        .collect();
    if out.len() > 1 && out.last().is_some_and(String::is_empty) {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn l(s: &str) -> Vec<String> {
        lines(s)
    }

    fn kinds(m: &Model) -> String {
        m.rows
            .iter()
            .map(|r| match r.kind {
                Kind::Same => '=',
                Kind::Removed => '-',
                Kind::Added => '+',
                Kind::Changed => '~',
            })
            .collect()
    }

    #[test]
    fn rows_pair_changes_and_cover_every_line_once() {
        let old = l("a\nb\nc\nd\ne\n");
        let new = l("a\nB\nc\nd\nx\ny\ne\n");
        let m = compute(&old, &new, Duration::from_secs(1));
        assert_eq!(kinds(&m), "=~==++=");
        assert_eq!(m.hunks, vec![1, 4]);
        let olds: Vec<usize> = m.rows.iter().filter_map(|r| r.old).collect();
        let news: Vec<usize> = m.rows.iter().filter_map(|r| r.new).collect();
        assert_eq!(olds, (0..old.len()).collect::<Vec<_>>());
        assert_eq!(news, (0..new.len()).collect::<Vec<_>>());
        assert!(!m.simplified);
    }

    #[test]
    fn identical_empty_and_one_sided() {
        let a = l("same\ntext\n");
        assert!(compute(&a, &a, Duration::from_secs(1)).identical());
        let m = compute(&[], &a, Duration::from_secs(1));
        assert_eq!(kinds(&m), "++");
        let m = compute(&a, &[], Duration::from_secs(1));
        assert_eq!(kinds(&m), "--");
        assert_eq!(l("x\r\ny\r\n"), l("x\ny\n"), "line endings are ignored");
        assert_eq!(l(""), vec![String::new()]);
    }

    #[test]
    fn inline_highlights_the_changed_word() {
        let (o, n) = inline_changes("let x = 42;", "let y = 42;");
        assert_eq!(o, vec![(4, 5)]);
        assert_eq!(n, vec![(4, 5)]);
        let (o, n) = inline_changes("héllo wörld", "héllo world");
        for (a, b) in o {
            assert!("héllo wörld".is_char_boundary(a) && "héllo wörld".is_char_boundary(b));
        }
        assert_eq!(n, vec![(8, 9)], "the o after w");
        let long = "x".repeat(10_000);
        assert_eq!(inline_changes(&long, "y").0, vec![(0, 10_000)]);
    }

    #[test]
    fn big_unrelated_files_finish_within_the_budget() {
        let old: Vec<String> = (0..60_000usize)
            .map(|i| format!("old line {i} {}", i * 7919 % 1000))
            .collect();
        let new: Vec<String> = (0..60_000usize)
            .map(|i| format!("new row {i} {}", i * 104729 % 997))
            .collect();
        let t = Instant::now();
        let m = compute(&old, &new, Duration::from_millis(300));
        assert!(
            t.elapsed() < Duration::from_secs(5),
            "took {:?}",
            t.elapsed()
        );
        let olds = m.rows.iter().filter(|r| r.old.is_some()).count();
        let news = m.rows.iter().filter(|r| r.new.is_some()).count();
        assert_eq!((olds, news), (60_000, 60_000));
    }
}
