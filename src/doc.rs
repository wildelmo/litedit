//! Loading files from disk, tolerant of anything Windows throws at us.
//!
//! - UTF-8 with or without BOM
//! - UTF-16 LE/BE with BOM (Notepad's "Unicode" option)
//! - Invalid UTF-8 is decoded lossily — never panics
//! - CRLF / LF / lone CR line endings are detected and remembered
//!
//! Text is normalised to `\n` internally; the original line ending is kept so
//! saving (Epic 2) can write it back unchanged.

use std::io;
use std::path::{Path, PathBuf};

/// Refuse to open files larger than this in one go (protects memory).
pub const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    CrLf,
    Cr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
    /// Bytes were not valid UTF-8; invalid sequences were replaced.
    Utf8Lossy,
}

#[derive(Debug)]
pub struct Document {
    pub path: PathBuf,
    pub text: String,
    pub line_ending: LineEnding,
    pub encoding: Encoding,
}

impl Document {
    pub fn open(path: &Path) -> io::Result<Self> {
        let meta = std::fs::metadata(path)?;
        if meta.len() > MAX_FILE_BYTES {
            return Err(io::Error::other(format!(
                "file is too large to open ({} MB)",
                meta.len() / (1024 * 1024)
            )));
        }
        let bytes = std::fs::read(path)?;
        let (text, encoding) = decode(&bytes);
        let (text, line_ending) = normalize_newlines(text);
        Ok(Self {
            path: path.to_path_buf(),
            text,
            line_ending,
            encoding,
        })
    }

    pub fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }
}

pub fn decode(bytes: &[u8]) -> (String, Encoding) {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return (
            String::from_utf8_lossy(rest).into_owned(),
            Encoding::Utf8Bom,
        );
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return (decode_utf16(rest, u16::from_le_bytes), Encoding::Utf16Le);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return (decode_utf16(rest, u16::from_be_bytes), Encoding::Utf16Be);
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => (s.to_owned(), Encoding::Utf8),
        Err(_) => (
            String::from_utf8_lossy(bytes).into_owned(),
            Encoding::Utf8Lossy,
        ),
    }
}

fn decode_utf16(bytes: &[u8], f: fn([u8; 2]) -> u16) -> String {
    let units = bytes.chunks_exact(2).map(|c| f([c[0], c[1]]));
    char::decode_utf16(units)
        .map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

/// Convert all line endings to `\n`; report the dominant original style.
pub fn normalize_newlines(text: String) -> (String, LineEnding) {
    if !text.contains('\r') {
        return (text, LineEnding::Lf);
    }
    let (mut crlf, mut cr, mut lf) = (0usize, 0usize, 0usize);
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' if chars.peek() == Some(&'\n') => {
                chars.next();
                crlf += 1;
                out.push('\n');
            }
            '\r' => {
                cr += 1;
                out.push('\n');
            }
            '\n' => {
                lf += 1;
                out.push('\n');
            }
            c => out.push(c),
        }
    }
    let ending = if crlf >= lf && crlf >= cr {
        LineEnding::CrLf
    } else if cr > lf {
        LineEnding::Cr
    } else {
        LineEnding::Lf
    };
    (out, ending)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_bom_is_stripped() {
        let (s, e) = decode(b"\xEF\xBB\xBFhi");
        assert_eq!(s, "hi");
        assert_eq!(e, Encoding::Utf8Bom);
    }

    #[test]
    fn invalid_utf8_is_lossy_not_panic() {
        let (s, e) = decode(b"ok \xFF\xFE\xFD done");
        assert!(s.starts_with("ok "));
        assert!(s.ends_with(" done"));
        assert_eq!(e, Encoding::Utf8Lossy);
    }

    #[test]
    fn utf16le_with_bom() {
        let (s, e) = decode(&[0xFF, 0xFE, b'h', 0, b'i', 0]);
        assert_eq!(s, "hi");
        assert_eq!(e, Encoding::Utf16Le);
    }

    #[test]
    fn odd_length_utf16_does_not_panic() {
        let (s, _) = decode(&[0xFF, 0xFE, b'h', 0, b'i']);
        assert_eq!(s, "h");
    }

    #[test]
    fn crlf_detected_and_normalized() {
        let (s, e) = normalize_newlines("a\r\nb\r\nc\n".into());
        assert_eq!(s, "a\nb\nc\n");
        assert_eq!(e, LineEnding::CrLf);
    }

    #[test]
    fn lone_cr() {
        let (s, e) = normalize_newlines("a\rb\rc".into());
        assert_eq!(s, "a\nb\nc");
        assert_eq!(e, LineEnding::Cr);
    }
}
