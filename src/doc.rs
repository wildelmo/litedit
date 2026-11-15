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
/// About 100 MB of memory at the most per open file.
pub const MAX_LINES: usize = 4_000_000;

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
    /// Size and modified time taken *before* reading (so a write during the
    /// read is noticed later).
    pub meta: Option<(u64, std::time::SystemTime)>,
}

impl Document {
    pub fn open(path: &Path) -> io::Result<Self> {
        if is_reserved_name(path) {
            return Err(io::Error::other("reserved Windows device name"));
        }
        let meta = std::fs::metadata(path)?;
        let stamp = meta.modified().ok().map(|t| (meta.len(), t));
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
        if looks_binary(&bytes) {
            return Err(io::Error::other(
                "this looks like a binary file (an image, program or archive), not text",
            ));
        }
        let (text, encoding, lossy) = decode_owned(bytes);
        // Each line costs memory beyond its text; millions of tiny lines
        // would need gigabytes. Counted on the decoded text, exactly as the
        // editor splits it (a lone CR ends a line too), before any line is
        // allocated.
        let lines = count_line_breaks(&text, MAX_LINES);
        if lines > MAX_LINES {
            return Err(io::Error::other(format!(
                "file has too many lines to open (more than {} million)",
                MAX_LINES / 1_000_000
            )));
        }
        Ok(Self {
            path: path.to_path_buf(),
            text,
            encoding,
            lossy,
            meta: stamp,
        })
    }

    pub fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }
}

/// Line breaks in `text` as the editor buffer and the compare view split
/// it: LF, CRLF (one break) and a lone CR. Stops counting once the number
/// is above `limit` (the result is then just "more than `limit`").
pub fn count_line_breaks(text: &str, limit: usize) -> usize {
    let b = text.as_bytes();
    let mut n = 0;
    for (i, &c) in b.iter().enumerate() {
        if c == b'\n' || (c == b'\r' && b.get(i + 1) != Some(&b'\n')) {
            n += 1;
            if n > limit {
                break;
            }
        }
    }
    n
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

/// NUL bytes in the first 8 KB of a file that isn't UTF-16: binary.
fn looks_binary(bytes: &[u8]) -> bool {
    !(bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]))
        && bytes.iter().take(8192).any(|&b| b == 0)
}

/// `decode` for a file just read, without a second copy of valid UTF-8
/// (the common case: the bytes become the text), and without encoding the
/// text back to learn whether it would save unchanged. Returns the text,
/// the encoding and whether saving would not write back the same bytes.
fn decode_owned(mut bytes: Vec<u8>) -> (String, Encoding, bool) {
    let bom = bytes.starts_with(&[0xEF, 0xBB, 0xBF]);
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        // UTF-16 can be broken in ways only a round trip shows.
        let (text, enc) = decode(&bytes);
        let lossy = encode(&text, enc) != bytes;
        return (text, enc, lossy);
    }
    if bom {
        bytes.drain(..3);
    }
    let enc = if bom {
        Encoding::Utf8Bom
    } else {
        Encoding::Utf8
    };
    match String::from_utf8(bytes) {
        // Valid UTF-8 encodes back to exactly the bytes it came from.
        Ok(text) => (text, enc, false),
        Err(e) => {
            let text = String::from_utf8_lossy(e.as_bytes()).into_owned();
            (text, if bom { enc } else { Encoding::Utf8Lossy }, true)
        }
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
    fn count_line_breaks_matches_the_editor_split() {
        let n = |t: &str| count_line_breaks(t, usize::MAX);
        assert_eq!(n(""), 0);
        assert_eq!(n("a"), 0);
        assert_eq!(n("a\nb"), 1);
        assert_eq!(n("a\r\nb"), 1, "CRLF is one break");
        assert_eq!(n("a\rb"), 1, "a lone CR is a break");
        assert_eq!(n("\r\r\n\n"), 3);
        assert_eq!(n("a\r"), 1, "a trailing CR is a break");
        // Over the limit it stops early: the result is only "more than".
        assert_eq!(count_line_breaks("\n\n\n\n\n", 2), 3);
        assert_eq!(count_line_breaks("\r\r\r\r\r", 2), 3);
        assert_eq!(count_line_breaks("\n\n", 2), 2);
        assert_eq!(count_line_breaks("\r\n\r\n\r\n", 2), 3);
    }

    fn tmp_file(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("litedit-doc-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join("f.txt")
    }

    #[test]
    fn a_file_of_lone_cr_line_ends_counts_against_the_line_limit() {
        let f = tmp_file("cr");
        std::fs::write(&f, "\r".repeat(MAX_LINES + 1)).unwrap();
        let e = Document::open(&f).unwrap_err();
        assert!(e.to_string().contains("too many lines"), "{e}");
        std::fs::write(&f, "\r".repeat(MAX_LINES)).unwrap();
        assert!(Document::open(&f).is_ok());
        std::fs::remove_dir_all(f.parent().unwrap()).ok();
    }

    #[test]
    fn crlf_lines_count_once_toward_the_line_limit() {
        // 2.5 million lines: 5 million LF + CR bytes, but under the limit.
        let f = tmp_file("crlf");
        std::fs::write(&f, "x\r\n".repeat(2_500_000)).unwrap();
        let d = Document::open(&f).expect("2.5 million CRLF lines open");
        assert!(d.text.starts_with("x"));
        std::fs::remove_dir_all(f.parent().unwrap()).ok();
    }

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

    /// `decode_owned` is `decode` plus the round-trip test, without the copies.
    #[test]
    fn decode_owned_agrees_with_decode_and_a_round_trip() {
        let samples: Vec<Vec<u8>> = vec![
            b"".to_vec(),
            b"plain\r\ntext\n".to_vec(),
            "h\u{e9}llo \u{1F600}".as_bytes().to_vec(),
            b"\xEF\xBB\xBFbom text".to_vec(),
            b"\xEF\xBB\xBF".to_vec(),
            b"\xEF\xBB\xBFbad \xFF byte".to_vec(),
            b"ok \xFF\xFE\xFD done".to_vec(),
            b"\xC3".to_vec(),
            vec![0xFF, 0xFE, b'h', 0, b'i', 0],
            vec![0xFF, 0xFE, b'h', 0, b'i'],
            vec![0xFF, 0xFE, 0x00, 0xD8, b'x', 0],
            vec![0xFE, 0xFF, 0, b'h', 0, b'i'],
            vec![0xFE, 0xFF],
        ];
        for b in samples {
            let (t, e) = decode(&b);
            let lossy = e == Encoding::Utf8Lossy || encode(&t, e) != b;
            assert_eq!(decode_owned(b.clone()), (t, e, lossy), "{b:?}");
        }
    }

    #[test]
    fn open_flags_lossy_files_and_not_clean_ones() {
        let dir = std::env::temp_dir().join(format!("litedit-doc-open-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, bytes, lossy) in [
            ("clean.txt", b"a\r\nb".to_vec(), false),
            ("bom.txt", b"\xEF\xBB\xBFa".to_vec(), false),
            ("bad.txt", b"a\xFFb".to_vec(), true),
            ("bombad.txt", b"\xEF\xBB\xBFa\xFFb".to_vec(), true),
        ] {
            let p = dir.join(name);
            std::fs::write(&p, &bytes).unwrap();
            assert_eq!(Document::open(&p).unwrap().lossy, lossy, "{name}");
        }
        let utf16 = dir.join("u16.txt");
        std::fs::write(&utf16, [0xFF, 0xFE, b'a', 0, b'b']).unwrap();
        assert!(Document::open(&utf16).unwrap().lossy);
        let nul = dir.join("nul.bin");
        std::fs::write(&nul, b"ab\0cd").unwrap();
        assert!(Document::open(&nul).is_err());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn odd_length_utf16_does_not_panic() {
        let (s, _) = decode(&[0xFF, 0xFE, b'h', 0, b'i']);
        assert_eq!(s, "h");
    }
}
