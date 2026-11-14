//! Find / replace within one document (lite-xl `find-replace` + `doc/search`).
//!
//! Plain text and regex share one path: plain text is escaped into a regex.
//! Matching is per line (like lite-xl), zero-length matches are skipped (and
//! always advance), the regex engine has a backtracking limit so a bad pattern
//! cannot hang the editor, and counts/replace-all are capped.

use fancy_regex::{Regex, RegexBuilder};

use crate::editor::buffer::{Buffer, Pos};
use crate::inputbar::InputBar;

/// Backtracking steps per search before fancy-regex gives up on a line.
const BACKTRACK_LIMIT: usize = 200_000;
/// Matches reported per line (highlighting) and in total (count/replace all).
pub const MAX_LINE_MATCHES: usize = 1000;
pub const MAX_TOTAL_MATCHES: usize = 100_000;
/// Lines longer than this are not searched (minified files).
const MAX_LINE_BYTES: usize = 100_000;

pub struct Finder {
    pub find: InputBar,
    pub replace: Option<InputBar>,
    pub case_sensitive: bool,
    pub regex: bool,
    /// Keyboard focus is on the replace field.
    pub on_replace: bool,
    compiled: Option<(String, bool, bool, Result<Regex, String>)>,
}

impl Finder {
    pub fn new(initial: &str, with_replace: bool) -> Self {
        Self {
            find: InputBar::new("Find", initial),
            replace: with_replace.then(|| InputBar::new("Replace", "")),
            case_sensitive: false,
            regex: false,
            on_replace: false,
            compiled: None,
        }
    }

    pub fn pattern(&self) -> &str {
        self.find.text()
    }

    /// The compiled search, or an error message for an invalid regex.
    pub fn regex(&mut self) -> Option<Result<&Regex, String>> {
        let pat = self.find.text().to_string();
        if pat.is_empty() {
            return None;
        }
        let key = (pat.clone(), self.case_sensitive, self.regex);
        let stale = self
            .compiled
            .as_ref()
            .is_none_or(|(p, c, r, _)| (p, c, r) != (&key.0, &key.1, &key.2));
        if stale {
            let src = if self.regex {
                pat.clone()
            } else {
                fancy_regex::escape(&pat).into_owned()
            };
            let built = RegexBuilder::new(&src)
                .case_insensitive(!self.case_sensitive)
                .backtrack_limit(BACKTRACK_LIMIT)
                .build()
                .map_err(|e| short_error(&e.to_string()));
            self.compiled = Some((key.0, key.1, key.2, built));
        }
        self.compiled
            .as_ref()
            .map(|(_, _, _, r)| r.as_ref().map_err(Clone::clone))
    }

    pub fn error(&mut self) -> Option<String> {
        match self.regex() {
            Some(Err(e)) => Some(e),
            _ => None,
        }
    }

    /// Non-empty matches in one line, as byte ranges.
    pub fn line_matches(&mut self, line: &str) -> Vec<(usize, usize)> {
        let Some(Ok(re)) = self.regex() else {
            return Vec::new();
        };
        matches_in(re, line)
    }

    /// The next match strictly after `from` (or before it when `back`),
    /// wrapping around the document.
    pub fn next(&mut self, buf: &Buffer, from: Pos, back: bool) -> Option<(Pos, Pos)> {
        let Some(Ok(re)) = self.regex() else {
            return None;
        };
        let re = re.clone();
        let n = buf.line_count();
        if back {
            for k in 0..=n {
                let line = (from.line + n - k % n) % n;
                let text = buf.line(line);
                let ms = matches_in(&re, text);
                let hit = ms.into_iter().rev().find(|(a, _)| k > 0 || *a < from.col);
                if let Some((a, b)) = hit {
                    return Some((Pos::new(line, a), Pos::new(line, b)));
                }
            }
        } else {
            for k in 0..=n {
                let line = (from.line + k) % n;
                let text = buf.line(line);
                let ms = matches_in(&re, text);
                let hit = ms.into_iter().find(|(a, _)| k > 0 || *a >= from.col);
                if let Some((a, b)) = hit {
                    return Some((Pos::new(line, a), Pos::new(line, b)));
                }
            }
        }
        None
    }

    /// Count matches (capped). Returns (count, capped).
    pub fn count(&mut self, buf: &Buffer) -> (usize, bool) {
        let Some(Ok(re)) = self.regex() else {
            return (0, false);
        };
        let re = re.clone();
        let mut total = 0;
        for l in buf.lines() {
            total += matches_in(&re, l).len();
            if total >= MAX_TOTAL_MATCHES {
                return (MAX_TOTAL_MATCHES, true);
            }
        }
        (total, false)
    }

    /// The replacement text for a match at `(line, a..b)`: literal for plain
    /// searches, with `$1` / `${name}` expansion for regex searches.
    pub fn replacement_for(&mut self, line: &str, a: usize) -> Option<String> {
        let repl = self
            .replace
            .as_ref()
            .map(|r| r.text().to_string())
            .unwrap_or_default();
        if !self.regex {
            return Some(repl);
        }
        let Some(Ok(re)) = self.regex() else {
            return None;
        };
        let caps = re.captures_from_pos(line, a).ok().flatten()?;
        let mut out = String::new();
        caps.expand(&repl, &mut out);
        Some(out)
    }

    /// Every match in the document (capped), for replace-all.
    pub fn all(&mut self, buf: &Buffer) -> Vec<(Pos, Pos)> {
        let Some(Ok(re)) = self.regex() else {
            return Vec::new();
        };
        let re = re.clone();
        let mut out = Vec::new();
        for (i, l) in buf.lines().iter().enumerate() {
            for (a, b) in matches_in(&re, l) {
                out.push((Pos::new(i, a), Pos::new(i, b)));
                if out.len() >= MAX_TOTAL_MATCHES {
                    return out;
                }
            }
        }
        out
    }
}

/// Non-empty matches in `line`. Zero-length matches are skipped and the
/// search always advances, so patterns like `a*` cannot loop forever.
fn matches_in(re: &Regex, line: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    if line.len() > MAX_LINE_BYTES {
        return out;
    }
    let mut pos = 0;
    while pos <= line.len() && out.len() < MAX_LINE_MATCHES {
        match re.find_from_pos(line, pos) {
            Ok(Some(m)) => {
                if m.end() > m.start() {
                    out.push((m.start(), m.end()));
                    pos = m.end();
                } else {
                    // Empty match: step past one char.
                    pos = line[m.end()..]
                        .chars()
                        .next()
                        .map_or(line.len() + 1, |c| m.end() + c.len_utf8());
                }
            }
            // No more matches, or the backtrack limit was hit on this line.
            _ => break,
        }
    }
    out
}

fn short_error(e: &str) -> String {
    let first = e.lines().next().unwrap_or(e);
    format!("invalid pattern: {}", crate::ui::clip_chars(first, 80))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finder(p: &str) -> Finder {
        Finder::new(p, true)
    }

    #[test]
    fn plain_text_is_literal_and_case_insensitive_by_default() {
        let mut f = finder("a.b");
        assert_eq!(f.line_matches("A.B axb a.b"), vec![(0, 3), (8, 11)]);
        f.case_sensitive = true;
        assert_eq!(f.line_matches("A.B axb a.b"), vec![(8, 11)]);
    }

    #[test]
    fn next_and_previous_wrap() {
        let buf = Buffer::from_text("foo\nbar foo\nbaz");
        let mut f = finder("foo");
        assert_eq!(
            f.next(&buf, Pos::new(0, 1), false),
            Some((Pos::new(1, 4), Pos::new(1, 7)))
        );
        assert_eq!(
            f.next(&buf, Pos::new(1, 5), false),
            Some((Pos::new(0, 0), Pos::new(0, 3))),
            "wraps"
        );
        assert_eq!(
            f.next(&buf, Pos::new(1, 4), true),
            Some((Pos::new(0, 0), Pos::new(0, 3)))
        );
        assert_eq!(
            f.next(&buf, Pos::new(0, 0), true),
            Some((Pos::new(1, 4), Pos::new(1, 7))),
            "wraps back"
        );
        assert_eq!(f.count(&buf), (2, false));
    }

    #[test]
    fn regex_traps() {
        let mut f = finder("a*");
        f.regex = true;
        // Zero-length matches are skipped, the scan still terminates.
        assert_eq!(f.line_matches("baab"), vec![(1, 3)]);
        f.find.set_text("(");
        assert!(f.error().unwrap().starts_with("invalid pattern"));
        // Catastrophic backtracking hits the limit instead of hanging.
        f.find.set_text("(a+)+$");
        let evil = "a".repeat(40) + "!";
        let t = std::time::Instant::now();
        let _ = f.line_matches(&evil);
        assert!(t.elapsed().as_secs() < 2);
    }

    #[test]
    fn regex_replacement_expands_groups() {
        let mut f = finder(r"(\w+)@(\w+)");
        f.regex = true;
        f.replace.as_mut().unwrap().set_text("$2 at $1");
        assert_eq!(
            f.replacement_for("mail bob@home now", 5),
            Some("home at bob".into())
        );
        f.regex = false;
        f.find.set_text("bob");
        f.replace.as_mut().unwrap().set_text("$1");
        assert_eq!(
            f.replacement_for("bob", 0),
            Some("$1".into()),
            "plain replace is literal"
        );
    }

    #[test]
    fn unicode_case_insensitive_offsets_are_valid() {
        let mut f = finder("straße");
        let line = "Die STRASSE und die Straße";
        for (a, b) in f.line_matches(line) {
            assert!(line.is_char_boundary(a) && line.is_char_boundary(b));
        }
    }
}
