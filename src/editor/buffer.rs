//! The text buffer: lines plus the exact line ending after each one.
//!
//! Keeping every line ending (instead of normalising to `\n`) makes
//! open → save byte-identical, even for files with mixed CRLF/LF endings or
//! no final newline. New line breaks typed by the user use the buffer's
//! default ending (the file's dominant one).

/// A line ending as it appears in the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Eol {
    Lf,
    CrLf,
    Cr,
}

impl Eol {
    pub fn as_str(self) -> &'static str {
        match self {
            Eol::Lf => "\n",
            Eol::CrLf => "\r\n",
            Eol::Cr => "\r",
        }
    }

    /// The ending used for new files: CRLF on Windows (like Notepad), LF elsewhere.
    pub fn platform() -> Eol {
        if cfg!(windows) { Eol::CrLf } else { Eol::Lf }
    }
}

/// A position: line index and byte column (always on a char boundary).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pos {
    pub line: usize,
    pub col: usize,
}

impl Pos {
    pub const fn new(line: usize, col: usize) -> Self {
        Self { line, col }
    }
}

/// A run of text with explicit endings: `lines[i]` is followed by `eols[i]`
/// (so `eols.len() == lines.len() - 1`). Used for inserted/removed text so
/// undo restores endings exactly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chunk {
    pub lines: Vec<String>,
    pub eols: Vec<Eol>,
}

impl Chunk {
    /// Parse text, keeping each ending as written. Every line is copied
    /// at its exact size (no growth slack: a big file has many lines).
    pub fn parse(text: &str) -> Chunk {
        let bytes = text.as_bytes();
        let mut lines = Vec::new();
        let mut eols = Vec::new();
        let mut start = 0;
        let mut i = 0;
        // `\r` and `\n` are ASCII, so every cut is on a char boundary.
        while let Some(off) = bytes
            .get(i..)
            .and_then(|r| r.iter().position(|&b| b == b'\r' || b == b'\n'))
        {
            let at = i + off;
            let (eol, len) = match (bytes.get(at), bytes.get(at + 1)) {
                (Some(b'\r'), Some(b'\n')) => (Eol::CrLf, 2),
                (Some(b'\r'), _) => (Eol::Cr, 1),
                _ => (Eol::Lf, 1),
            };
            lines.push(text.get(start..at).unwrap_or_default().to_owned());
            eols.push(eol);
            i = at + len;
            start = i;
        }
        lines.push(text.get(start..).unwrap_or_default().to_owned());
        Chunk { lines, eols }
    }

    /// Parse typed/pasted text: every kind of line break becomes `eol`.
    pub fn typed(text: &str, eol: Eol) -> Chunk {
        let mut c = Chunk::parse(text);
        for e in &mut c.eols {
            *e = eol;
        }
        c
    }

    pub fn to_text(&self) -> String {
        let mut s = String::with_capacity(self.lines.iter().map(|l| l.len() + 2).sum());
        for (i, l) in self.lines.iter().enumerate() {
            s.push_str(l);
            if let Some(e) = self.eols.get(i) {
                s.push_str(e.as_str());
            }
        }
        s
    }

    /// Text with `\n` line breaks (clipboard, previews).
    pub fn to_text_lf(&self) -> String {
        self.lines.join("\n")
    }

    pub fn is_empty(&self) -> bool {
        self.lines.len() == 1 && self.lines[0].is_empty()
    }

    /// Where this chunk ends if inserted at `at`.
    pub fn end_from(&self, at: Pos) -> Pos {
        if self.lines.len() == 1 {
            Pos::new(at.line, at.col + self.lines[0].len())
        } else {
            Pos::new(
                at.line + self.lines.len() - 1,
                self.lines.last().map_or(0, String::len),
            )
        }
    }
}

/// The document text. Invariant: `lines.len() >= 1` and
/// `eols.len() == lines.len() - 1`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Buffer {
    lines: Vec<String>,
    eols: Vec<Eol>,
    default_eol: Eol,
}

impl Default for Buffer {
    fn default() -> Self {
        Self {
            lines: vec![String::new()],
            eols: Vec::new(),
            default_eol: Eol::platform(),
        }
    }
}

impl Buffer {
    pub fn from_text(text: &str) -> Buffer {
        let Chunk {
            mut lines,
            mut eols,
        } = Chunk::parse(text);
        // The vectors grew by doubling while parsing; a loaded file's
        // buffer does not need the spare room (edits regrow it).
        lines.shrink_to_fit();
        eols.shrink_to_fit();
        let default_eol = dominant(&eols).unwrap_or_else(Eol::platform);
        Buffer {
            lines,
            eols,
            default_eol,
        }
    }

    /// The exact text, with every original line ending.
    pub fn to_text(&self) -> String {
        let mut s = String::with_capacity(self.lines.iter().map(|l| l.len() + 2).sum());
        for (i, l) in self.lines.iter().enumerate() {
            s.push_str(l);
            if let Some(e) = self.eols.get(i) {
                s.push_str(e.as_str());
            }
        }
        s
    }

    /// The text with `\n` line breaks (for the markdown preview).
    pub fn to_text_lf(&self) -> String {
        self.lines.join("\n")
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    pub fn line(&self, i: usize) -> &str {
        self.lines.get(i).map_or("", String::as_str)
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    pub fn default_eol(&self) -> Eol {
        self.default_eol
    }

    pub fn set_default_eol(&mut self, e: Eol) {
        self.default_eol = e;
    }

    /// True if the file mixes different line endings.
    pub fn has_mixed_eols(&self) -> bool {
        self.eols.windows(2).any(|w| w[0] != w[1])
    }

    pub fn end(&self) -> Pos {
        let last = self.lines.len() - 1;
        Pos::new(last, self.lines[last].len())
    }

    /// Clamp a position into the buffer, onto a char boundary.
    pub fn clamp(&self, p: Pos) -> Pos {
        let line = p.line.min(self.lines.len() - 1);
        let s = &self.lines[line];
        let mut col = p.col.min(s.len());
        while !s.is_char_boundary(col) {
            col -= 1;
        }
        Pos::new(line, col)
    }

    /// Text between two positions (any order), endings preserved.
    pub fn slice(&self, a: Pos, b: Pos) -> Chunk {
        let (a, b) = order(self.clamp(a), self.clamp(b));
        if a.line == b.line {
            return Chunk {
                lines: vec![self.lines[a.line][a.col..b.col].to_string()],
                eols: Vec::new(),
            };
        }
        let mut lines = vec![self.lines[a.line][a.col..].to_string()];
        lines.extend(self.lines[a.line + 1..b.line].iter().cloned());
        lines.push(self.lines[b.line][..b.col].to_string());
        Chunk {
            lines,
            eols: self.eols[a.line..b.line].to_vec(),
        }
    }

    /// Insert a chunk at `at`; returns the position after it.
    pub fn insert(&mut self, at: Pos, chunk: &Chunk) -> Pos {
        let at = self.clamp(at);
        let end = chunk.end_from(at);
        let tail = self.lines[at.line].split_off(at.col);
        if chunk.lines.len() == 1 {
            let l = &mut self.lines[at.line];
            l.push_str(&chunk.lines[0]);
            l.push_str(&tail);
        } else {
            self.lines[at.line].push_str(&chunk.lines[0]);
            let mut new_lines: Vec<String> = chunk.lines[1..].to_vec();
            if let Some(last) = new_lines.last_mut() {
                last.push_str(&tail);
            }
            self.lines.splice(at.line + 1..at.line + 1, new_lines);
            self.eols
                .splice(at.line..at.line, chunk.eols.iter().copied());
        }
        debug_assert_eq!(self.eols.len() + 1, self.lines.len());
        end
    }

    /// Remove the text between two positions; returns what was removed.
    pub fn remove(&mut self, a: Pos, b: Pos) -> Chunk {
        let (a, b) = order(self.clamp(a), self.clamp(b));
        let removed = self.slice(a, b);
        if a.line == b.line {
            self.lines[a.line].replace_range(a.col..b.col, "");
        } else {
            let tail = self.lines[b.line][b.col..].to_string();
            self.lines[a.line].truncate(a.col);
            self.lines[a.line].push_str(&tail);
            self.lines.drain(a.line + 1..=b.line);
            self.eols.drain(a.line..b.line);
        }
        debug_assert_eq!(self.eols.len() + 1, self.lines.len());
        removed
    }
}

pub fn order(a: Pos, b: Pos) -> (Pos, Pos) {
    if a <= b { (a, b) } else { (b, a) }
}

fn dominant(eols: &[Eol]) -> Option<Eol> {
    if eols.is_empty() {
        return None;
    }
    let count = |e: Eol| eols.iter().filter(|x| **x == e).count();
    let (lf, crlf, cr) = (count(Eol::Lf), count(Eol::CrLf), count(Eol::Cr));
    Some(if crlf >= lf && crlf >= cr {
        Eol::CrLf
    } else if lf >= cr {
        Eol::Lf
    } else {
        Eol::Cr
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_keeps_every_ending() {
        for t in [
            "",
            "a",
            "a\n",
            "a\r\nb",
            "a\r\nb\nc\rd\r\n",
            "\n\n",
            "x\r",
            "\r\n\r\n",
        ] {
            assert_eq!(Buffer::from_text(t).to_text(), t, "{t:?}");
        }
    }

    /// The original char-by-char parser, kept as the oracle.
    fn parse_slowly(text: &str) -> Chunk {
        let mut lines = Vec::new();
        let mut eols = Vec::new();
        let mut cur = String::new();
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\r' if chars.peek() == Some(&'\n') => {
                    chars.next();
                    lines.push(std::mem::take(&mut cur));
                    eols.push(Eol::CrLf);
                }
                '\r' => {
                    lines.push(std::mem::take(&mut cur));
                    eols.push(Eol::Cr);
                }
                '\n' => {
                    lines.push(std::mem::take(&mut cur));
                    eols.push(Eol::Lf);
                }
                c => cur.push(c),
            }
        }
        lines.push(cur);
        Chunk { lines, eols }
    }

    #[test]
    fn parse_matches_the_char_by_char_oracle_and_is_exactly_sized() {
        let alphabet = ["a", "é", "\u{1F600}", "\r", "\n", "\r\n", " ", "xyz"];
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        for _ in 0..3000 {
            let mut t = String::new();
            for _ in 0..(seed % 12) {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                t.push_str(alphabet[(seed % alphabet.len() as u64) as usize]);
            }
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            assert_eq!(Chunk::parse(&t), parse_slowly(&t), "{t:?}");
        }
        let c = Chunk::parse("one\r\ntwo\rthree\nfour");
        assert!(c.lines.iter().all(|l| l.capacity() == l.len()));
        let b = Buffer::from_text(&"line of text\n".repeat(1000));
        assert_eq!(b.lines.capacity(), b.lines.len());
        assert_eq!(b.eols.capacity(), b.eols.len());
    }

    #[test]
    fn insert_and_remove() {
        let mut b = Buffer::from_text("hello\r\nworld");
        let end = b.insert(Pos::new(0, 5), &Chunk::typed(",\nnew", b.default_eol()));
        assert_eq!(end, Pos::new(1, 3));
        assert_eq!(b.to_text(), "hello,\r\nnew\r\nworld");
        let removed = b.remove(Pos::new(0, 5), Pos::new(1, 3));
        assert_eq!(removed.to_text(), ",\r\nnew");
        assert_eq!(b.to_text(), "hello\r\nworld");
    }

    #[test]
    fn clamp_respects_char_boundaries() {
        let b = Buffer::from_text("héllo\nx");
        assert_eq!(b.clamp(Pos::new(0, 2)), Pos::new(0, 1)); // inside 'é'
        assert_eq!(b.clamp(Pos::new(9, 99)), Pos::new(1, 1));
    }

    #[test]
    fn dominant_ending_is_default() {
        assert_eq!(Buffer::from_text("a\r\nb\r\nc\n").default_eol(), Eol::CrLf);
        assert_eq!(Buffer::from_text("a\nb\nc\r\n").default_eol(), Eol::Lf);
        assert!(Buffer::from_text("a\nb\r\n").has_mixed_eols());
    }
}
