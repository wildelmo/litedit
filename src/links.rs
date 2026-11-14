//! Classifying markdown link / image destinations, safely.
//!
//! - Only `http`, `https` and `mailto` ever leave the app (to the browser).
//! - Every other scheme (`file:`, `javascript:`, `ms-settings:` …) is blocked.
//! - Network paths (`\\server\share`, `//server/share`) are blocked: merely
//!   reading one makes Windows authenticate to that server.
//! - Relative paths resolve against the markdown file's folder.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkTarget {
    /// http/https/mailto URL, safe to hand to the system browser.
    Web(String),
    /// `#heading` within the same document (already slugified).
    Anchor(String),
    /// A local file path.
    File(PathBuf),
    Blocked(&'static str),
}

const MAX_URL_LEN: usize = 2048;

/// Decode `%XX` escapes; invalid escapes are kept literally.
pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(v) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The URL scheme, if `dest` has one. A single letter followed by `:` is a
/// Windows drive (`C:\`), not a scheme.
fn scheme(dest: &str) -> Option<String> {
    let colon = dest.find(':')?;
    let s = &dest[..colon];
    if s.len() < 2 || !s.starts_with(|c: char| c.is_ascii_alphabetic()) {
        return None;
    }
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.')
    {
        Some(s.to_ascii_lowercase())
    } else {
        None
    }
}

pub fn is_network_path(p: &str) -> bool {
    let p = p.trim_start();
    p.starts_with("\\\\") || p.starts_with("//") || p.starts_with("\\/") || p.starts_with("/\\")
}

pub fn classify(dest: &str, base_dir: &Path) -> LinkTarget {
    let dest = dest.trim();
    if dest.is_empty() {
        return LinkTarget::Blocked("empty link");
    }
    if dest.len() > MAX_URL_LEN || dest.chars().any(char::is_control) {
        return LinkTarget::Blocked("malformed link");
    }
    if let Some(anchor) = dest.strip_prefix('#') {
        return LinkTarget::Anchor(percent_decode(anchor).to_lowercase());
    }
    if let Some(s) = scheme(dest) {
        return match s.as_str() {
            "http" | "https" | "mailto" => {
                // No characters that could be misread by a shell or URL parser.
                if dest.chars().any(|c| {
                    c.is_whitespace() || matches!(c, '"' | '<' | '>' | '`' | '^' | '|' | '\\')
                }) {
                    LinkTarget::Blocked("malformed URL")
                } else if s != "mailto" && !dest[s.len() + 1..].starts_with("//") {
                    LinkTarget::Blocked("malformed URL")
                } else {
                    LinkTarget::Web(dest.to_string())
                }
            }
            _ => LinkTarget::Blocked("link type not allowed"),
        };
    }
    // A local path: drop ?query and #fragment, decode %20 etc.
    let path_part = dest.split(['#', '?']).next().unwrap_or("");
    let decoded = percent_decode(path_part);
    if decoded.is_empty() {
        return LinkTarget::Blocked("empty link");
    }
    if is_network_path(&decoded) {
        return LinkTarget::Blocked("network paths are not opened");
    }
    let p = PathBuf::from(&decoded);
    if crate::doc::is_reserved_name(&p) {
        return LinkTarget::Blocked("device name");
    }
    let full = if p.is_absolute() || decoded.starts_with('/') || decoded.starts_with('\\') {
        p
    } else {
        base_dir.join(p)
    };
    LinkTarget::File(full)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> PathBuf {
        PathBuf::from("/docs")
    }

    #[test]
    fn web_links() {
        assert_eq!(
            classify("https://example.com/a?b=1", &base()),
            LinkTarget::Web("https://example.com/a?b=1".into())
        );
        assert_eq!(
            classify("mailto:a@b.c", &base()),
            LinkTarget::Web("mailto:a@b.c".into())
        );
        assert_eq!(
            classify("HTTP://x.y", &base()),
            LinkTarget::Web("HTTP://x.y".into())
        );
    }

    #[test]
    fn dangerous_schemes_blocked() {
        for d in [
            "javascript:alert(1)",
            "file:///C:/Windows/system32/calc.exe",
            "file://server/share/x",
            "ms-settings:privacy",
            "vbscript:x",
            "data:text/html,hi",
            "search-ms:query=x",
        ] {
            assert!(
                matches!(classify(d, &base()), LinkTarget::Blocked(_)),
                "{d}"
            );
        }
    }

    #[test]
    fn shell_metacharacters_blocked() {
        for d in [
            "https://x.com/a&calc",
            "https://x.com/\"&calc",
            "https://x.com/a b",
            "https://x.com/^",
            "https:x.com",
        ] {
            let r = classify(d, &base());
            // `&` alone is legal in URLs and safe with ShellExecuteW; the rest are rejected.
            if d.contains("a&calc") && !d.contains('"') {
                assert!(matches!(r, LinkTarget::Web(_)));
            } else {
                assert!(matches!(r, LinkTarget::Blocked(_)), "{d}");
            }
        }
    }

    #[test]
    fn network_paths_blocked() {
        for d in [
            "\\\\server\\share\\x.md",
            "//server/share/x.md",
            "%5C%5Cserver%5Cshare",
            "\\/server/x",
        ] {
            assert!(
                matches!(classify(d, &base()), LinkTarget::Blocked(_)),
                "{d}"
            );
        }
    }

    #[test]
    fn relative_and_anchor() {
        assert_eq!(
            classify("other.md#sec", &base()),
            LinkTarget::File(PathBuf::from("/docs/other.md"))
        );
        assert_eq!(
            classify("my%20file.md", &base()),
            LinkTarget::File(PathBuf::from("/docs/my file.md"))
        );
        assert_eq!(
            classify("#Install-Guide", &base()),
            LinkTarget::Anchor("install-guide".into())
        );
        assert!(matches!(classify("", &base()), LinkTarget::Blocked(_)));
        assert!(matches!(
            classify("a\u{0}b", &base()),
            LinkTarget::Blocked(_)
        ));
    }

    #[test]
    fn drive_letters_are_paths_not_schemes() {
        assert!(matches!(
            classify("C:\\notes\\a.md", &base()),
            LinkTarget::File(_)
        ));
    }

    #[test]
    fn percent_decode_edge_cases() {
        assert_eq!(percent_decode("a%20b"), "a b");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
        assert_eq!(percent_decode("%4"), "%4");
    }
}
