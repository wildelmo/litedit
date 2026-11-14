//! Soft wrap (lite-xl `linewrapping` plugin): which visual rows each line
//! occupies, and the line ↔ row mapping used by everything that turns a
//! position into a y-coordinate or back.
//!
//! Wrapping is word-based on display columns (tabs expanded, like the
//! renderer), computed from the text alone — no shaping — so the total height
//! of a 170k-line file is known instantly. When wrapping is off the mapping
//! is the identity and costs nothing.

use crate::view::code::TAB_WIDTH;

/// Source byte offsets where each visual row of `line` starts (first is 0).
/// Breaks after the last space that fits; a word longer than the row is cut.
pub fn wrap_breaks(line: &str, width_cols: usize) -> Vec<usize> {
    let width = width_cols.max(4);
    let mut breaks = vec![0usize];
    let mut col = 0usize; // column within the current row
    let mut abs_col = 0usize; // column within the whole line (for tabs)
    let mut last_space: Option<(usize, usize)> = None; // (byte after space, col after space)
    // The view shows at most MAX_DISPLAY_CHARS of a line; wrap only that.
    for (b, c) in line
        .char_indices()
        .take(crate::view::code::MAX_DISPLAY_CHARS)
    {
        let w = if c == '\t' {
            TAB_WIDTH - abs_col % TAB_WIDTH
        } else {
            1
        };
        if col + w > width && col > 0 {
            match last_space.take() {
                Some((at, at_col)) if at > *breaks.last().unwrap_or(&0) => {
                    breaks.push(at);
                    col -= at_col;
                }
                _ => {
                    breaks.push(b);
                    col = 0;
                }
            }
        }
        col += w;
        abs_col += w;
        if c == ' ' || c == '\t' {
            last_space = Some((b + c.len_utf8(), col));
        }
    }
    breaks
}

/// Which row of a line a byte column is on, given its breaks.
pub fn row_of(breaks: &[usize], col: usize) -> usize {
    breaks.iter().rposition(|&b| b <= col).unwrap_or(0)
}

/// Line ↔ visual-row mapping for a whole document.
#[derive(Default)]
pub struct Rows {
    /// None = no wrapping (identity mapping).
    cols: Option<usize>,
    /// `first[i]` = first row of line i; `first[n]` = total rows.
    first: Vec<usize>,
    key: (u64, Option<usize>, usize),
    lines: usize,
}

impl Rows {
    /// Recompute if the text version, width or line count changed.
    pub fn update(&mut self, lines: &[String], version: u64, cols: Option<usize>) {
        let key = (version, cols, lines.len());
        if key == self.key && (self.cols.is_none() || !self.first.is_empty()) {
            return;
        }
        self.key = key;
        self.cols = cols;
        self.lines = lines.len();
        self.first.clear();
        if let Some(c) = cols {
            self.first.reserve(lines.len() + 1);
            let mut row = 0;
            for l in lines {
                self.first.push(row);
                // Fast path only when no tab can widen the line.
                row += if l.len() <= c && !l.contains('\t') {
                    1
                } else {
                    wrap_breaks(l, c).len()
                };
            }
            self.first.push(row);
        }
    }

    pub fn wrapping(&self) -> Option<usize> {
        self.cols
    }

    pub fn total(&self) -> usize {
        if self.cols.is_some() {
            *self.first.last().unwrap_or(&0)
        } else {
            self.lines
        }
    }

    pub fn first_row(&self, line: usize) -> usize {
        if self.cols.is_some() {
            self.first
                .get(line)
                .copied()
                .unwrap_or_else(|| self.total())
        } else {
            line
        }
    }

    pub fn row_count(&self, line: usize) -> usize {
        if self.cols.is_some() {
            self.first
                .get(line + 1)
                .copied()
                .unwrap_or(0)
                .saturating_sub(self.first_row(line))
                .max(1)
        } else {
            1
        }
    }

    /// (line, row within line) for a visual row (clamped to the document).
    pub fn line_of_row(&self, row: usize) -> (usize, usize) {
        if self.lines == 0 {
            return (0, 0);
        }
        if self.cols.is_none() {
            return (row.min(self.lines - 1), 0);
        }
        let row = row.min(self.total().saturating_sub(1));
        let line = match self.first[..self.lines].binary_search(&row) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        (line, row - self.first[line])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breaks_at_spaces_and_cuts_long_words() {
        assert_eq!(wrap_breaks("short", 10), vec![0]);
        //                       0123456789012345678
        assert_eq!(wrap_breaks("hello world again", 10), vec![0, 6, 12]);
        assert_eq!(wrap_breaks("hello world again", 12), vec![0, 12]);
        assert_eq!(wrap_breaks("abcdefghijklmnop", 5), vec![0, 5, 10, 15]);
        assert_eq!(wrap_breaks("", 5), vec![0]);
        let unicode = "ééééé ééééé";
        for b in wrap_breaks(unicode, 6) {
            assert!(unicode.is_char_boundary(b));
        }
    }

    #[test]
    fn every_char_lands_on_exactly_one_row_within_width() {
        let line = "The quick brown fox jumps over the lazy dog, again and again — ok?";
        for w in [4, 7, 10, 20, 80] {
            let br = wrap_breaks(line, w);
            assert_eq!(br[0], 0);
            assert!(br.windows(2).all(|p| p[0] < p[1]), "{br:?}");
            for k in 0..br.len() {
                let end = br.get(k + 1).copied().unwrap_or(line.len());
                let row = &line[br[k]..end];
                assert!(
                    row.trim_end().chars().count() <= w.max(4),
                    "w={w} row={row:?}"
                );
            }
        }
    }

    #[test]
    fn rows_mapping_round_trips() {
        let lines: Vec<String> = vec!["a".into(), "word ".repeat(10), "".into(), "x".repeat(25)];
        let mut r = Rows::default();
        r.update(&lines, 1, Some(10));
        assert_eq!(r.row_count(0), 1);
        assert!(r.row_count(1) >= 5);
        assert_eq!(r.row_count(3), 3);
        for row in 0..r.total() {
            let (l, k) = r.line_of_row(row);
            assert_eq!(r.first_row(l) + k, row);
        }
        r.update(&lines, 1, None);
        assert_eq!(r.total(), 4);
        assert_eq!(r.line_of_row(2), (2, 0));
        assert_eq!(r.line_of_row(99), (3, 0));
    }

    #[test]
    fn big_documents_are_fast() {
        let lines: Vec<String> = (0..170_000)
            .map(|i| format!("line {i} with some words to wrap around the edge"))
            .collect();
        let t = std::time::Instant::now();
        let mut r = Rows::default();
        r.update(&lines, 1, Some(20));
        assert!(t.elapsed().as_millis() < 2000, "took {:?}", t.elapsed());
        assert!(r.total() > 170_000);
    }

    #[test]
    fn tabs_and_very_long_lines_wrap_like_they_are_drawn() {
        let tabbed = format!("{}{}", "\t".repeat(8), "x".repeat(20));
        let lines = vec![tabbed.clone()];
        let mut r = Rows::default();
        r.update(&lines, 1, Some(40));
        assert!(
            r.row_count(0) > 1,
            "8 tabs = 32 columns + 20 chars needs 2 rows"
        );
        assert_eq!(r.row_count(0), wrap_breaks(&tabbed, 40).len());
        let long = "word ".repeat(10_000);
        let rows = wrap_breaks(&long, 80).len();
        assert!(
            rows <= crate::view::code::MAX_DISPLAY_CHARS / 70,
            "{rows} rows for the shown part only"
        );
    }

    #[test]
    fn row_of_col() {
        let br = vec![0, 6, 12];
        assert_eq!(row_of(&br, 0), 0);
        assert_eq!(row_of(&br, 5), 0);
        assert_eq!(row_of(&br, 6), 1);
        assert_eq!(row_of(&br, 99), 2);
    }
}
