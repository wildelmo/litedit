//! Loading files from disk, tolerant of anything Windows throws at us.
//!
//! - UTF-8 with or without BOM
//! - UTF-16 LE/BE with BOM (Notepad's "Unicode" option)
//! - Invalid UTF-8 is decoded lossily — never panics
//! - CRLF / LF / lone CR line endings are detected and remembered
//!
//! The decoded text keeps its original line endings; the editor buffer
//! records them per line so saving writes them back unchanged.

use std::io;
use std::path::{Path, PathBuf};

/// Refuse to open files larger than this in one go (protects memory).
pub const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
    /// Bytes were not valid UTF-8; invalid sequences were replaced.
    Utf8Lossy,
}

#[derive(Clone, Debug)]
pub struct Document {
    pub path: PathBuf,
    /// Decoded text with original line endings.
    pub text: String,
    pub encoding: Encoding,
    /// Saving would not write back the exact bytes that were read (invalid
    /// UTF-8, broken UTF-16, …): the user must confirm before saving.
    pub lossy: bool,
}

impl Document {
    pub fn open(path: &Path) -> io::Result<Self> {
        if is_reserved_name(path) {
            return Err(io::Error::other("reserved Windows device name"));
        }
        let meta = std::fs::metadata(path)?;
        if !meta.is_file() {
            return Err(io::Error::other("not a regular file"));
        }
        if meta.len() > MAX_FILE_BYTES {
            return Err(io::Error::other(format!(
                "file is too large to open ({} MB)",
                meta.len() / (1024 * 1024)
            )));
        }
        let bytes = std::fs::read(path)?;
        let (text, encoding) = decode(&bytes);
        // The only reliable test: does it encode back to the same bytes?
        let lossy = encoding == Encoding::Utf8Lossy || encode(&text, encoding) != bytes;
        Ok(Self {
            path: path.to_path_buf(),
            text,
            encoding,
            lossy,
        })
    }

    pub fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }
}

/// Windows device names (CON, NUL, COM1, …, with any extension) refer to
/// devices, not files; opening or saving them must never be attempted.
pub fn is_reserved_name(path: &Path) -> bool {
    let Some(name) = path
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_uppercase())
    else {
        return false;
    };
    let stem = name
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end_matches([' ', '.']);
    if matches!(stem, "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$") {
        return true;
    }
    // COM0-9 / LPT0-9, and the superscript digits ¹ ² ³ Windows also accepts.
    let mut chars = stem.chars();
    let prefix: String = chars.by_ref().take(3).collect();
    let rest: Vec<char> = chars.collect();
    (prefix == "COM" || prefix == "LPT")
        && rest.len() == 1
        && (rest[0].is_ascii_digit() || matches!(rest[0], '¹' | '²' | '³'))
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

/// Encode text back into bytes in the file's original encoding.
/// `Utf8Lossy` is written as plain UTF-8 (the invalid bytes are gone).
pub fn encode(text: &str, enc: Encoding) -> Vec<u8> {
    match enc {
        Encoding::Utf8 | Encoding::Utf8Lossy => text.as_bytes().to_vec(),
        Encoding::Utf8Bom => {
            let mut v = vec![0xEF, 0xBB, 0xBF];
            v.extend_from_slice(text.as_bytes());
            v
        }
        Encoding::Utf16Le => {
            let mut v = vec![0xFF, 0xFE];
            v.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
            v
        }
        Encoding::Utf16Be => {
            let mut v = vec![0xFE, 0xFF];
            v.extend(text.encode_utf16().flat_map(u16::to_be_bytes));
            v
        }
    }
}

fn decode_utf16(bytes: &[u8], f: fn([u8; 2]) -> u16) -> String {
    let units = bytes.chunks_exact(2).map(|c| f([c[0], c[1]]));
    char::decode_utf16(units)
        .map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_device_names() {
        for n in [
            "CON",
            "con.txt",
            "NUL",
            "nul.md",
            "COM1",
            "lpt9.log",
            "aux.png",
            "COM¹",
            "lpt³.txt",
            "nul .txt",
        ] {
            assert!(is_reserved_name(Path::new(n)), "{n}");
        }
        for n in ["console.txt", "com10", "notes.md", "nul_list.md"] {
            assert!(!is_reserved_name(Path::new(n)), "{n}");
        }
    }

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
}
