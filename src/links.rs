//! Classifying markdown link / image destinations, safely.
//!
//! - Only `http`, `https` and `mailto` ever leave the app (to the browser).
//! - Every other scheme (`file:`, `javascript:`, `ms-settings:` …) is blocked.
//! - Network paths (`\\server\share`, `//server/share`) are blocked: merely
//!   reading one makes Windows authenticate to that server.
//! - Relative paths resolve against the markdown file's folder.

use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DriveSafety {
    Local,
    Unsafe,
}

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
/// Links followed from one path component before giving up.
const MAX_LINK_HOPS: usize = 16;

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

/// Ask Windows about a drive prefix before content can make us touch it.
/// Mapped drives look like ordinary `X:\\` paths but may reconnect to a
/// network server as soon as filesystem metadata is requested.
fn drive_path_is_unsafe_with(p: &str, classify: impl FnOnce(&[u16; 4]) -> DriveSafety) -> bool {
    let p = p.trim_start();
    let p = p
        .strip_prefix("\\\\?\\")
        .or_else(|| p.strip_prefix("\\??\\"))
        .unwrap_or(p);
    let b = p.as_bytes();
    if b.len() < 2 || !b[0].is_ascii_alphabetic() || b[1] != b':' {
        return false;
    }
    let root = [
        u16::from(b[0].to_ascii_uppercase()),
        b':' as u16,
        b'\\' as u16,
        0,
    ];
    classify(&root) == DriveSafety::Unsafe
}

#[cfg(windows)]
fn classify_drive_root(root: &[u16; 4]) -> DriveSafety {
    use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;
    use windows_sys::Win32::System::WindowsProgramming::{
        DRIVE_CDROM, DRIVE_FIXED, DRIVE_NO_ROOT_DIR, DRIVE_RAMDISK, DRIVE_REMOTE, DRIVE_REMOVABLE,
        DRIVE_UNKNOWN,
    };

    // SAFETY: `root` is a NUL-terminated `X:\\` string and remains alive for
    // the duration of the call.
    let kind = unsafe { GetDriveTypeW(root.as_ptr()) };
    match kind {
        DRIVE_REMOVABLE | DRIVE_FIXED | DRIVE_CDROM | DRIVE_RAMDISK => DriveSafety::Local,
        DRIVE_UNKNOWN | DRIVE_NO_ROOT_DIR | DRIVE_REMOTE => DriveSafety::Unsafe,
        _ => DriveSafety::Unsafe,
    }
}

#[cfg(not(windows))]
fn classify_drive_root(_: &[u16; 4]) -> DriveSafety {
    DriveSafety::Local
}

/// Paths Windows treats specially and that a markdown file must never make
/// us open: network shares, `\\?\` / `\??\` / `\\.\` device and NT paths
/// (they can reach UNC shares too), mapped/unavailable drive prefixes, and
/// alternate data streams (`a.png:x`).
fn is_unsafe_local_path_with(p: &str, classify: impl FnOnce(&[u16; 4]) -> DriveSafety) -> bool {
    let p = p.trim_start();
    if is_network_path(p) {
        return true;
    }
    if drive_path_is_unsafe_with(p, classify) {
        return true;
    }
    let b = p.as_bytes();
    let slash = |c: u8| c == b'\\' || c == b'/';
    // \??\, /??/, \.\ … (a single leading slash followed by ? or .)
    if b.len() >= 2
        && slash(b[0])
        && (b[1] == b'?' || b[1] == b'.' && b.get(2).is_some_and(|&c| slash(c)))
    {
        return true;
    }
    // A colon anywhere but right after a drive letter.
    p.char_indices()
        .any(|(i, c)| c == ':' && !(i == 1 && b[0].is_ascii_alphabetic()))
}

pub fn is_unsafe_local_path(p: &str) -> bool {
    is_unsafe_local_path_with(p, classify_drive_root)
}

/// Is a link target a drive path written in verbatim/NT form
/// (`\\?\C:\…`, `\??\C:\…`, as Windows junctions report)?
fn is_local_drive_target(t: &str) -> bool {
    let rest = t
        .strip_prefix("\\\\?\\")
        .or_else(|| t.strip_prefix("\\??\\"));
    rest.is_some_and(|r| {
        let b = r.as_bytes();
        b.len() >= 3
            && b[0].is_ascii_alphabetic()
            && b[1] == b':'
            && (b[2] == b'\\' || b[2] == b'/')
    })
}

/// Reparse points report local drive targets in verbatim/NT form. Keep that
/// spelling exception only when Windows also says the target drive is local.
fn is_unsafe_link_target_with(t: &str, classify: impl FnOnce(&[u16; 4]) -> DriveSafety) -> bool {
    if is_local_drive_target(t) {
        drive_path_is_unsafe_with(t, classify)
    } else {
        is_unsafe_local_path_with(t, classify)
    }
}

fn is_unsafe_link_target(t: &str) -> bool {
    is_unsafe_link_target_with(t, classify_drive_root)
}

/// Does any symlink/junction along `p` — or any hop of a chain of links —
/// point at a network share or device path? (Following it would make
/// Windows contact that server.) A chain longer than [`MAX_LINK_HOPS`] or a
/// loop counts as unsafe: what was not checked is not followed.
pub fn link_reaches_unsafe(p: &Path) -> bool {
    let mut prefix = PathBuf::new();
    for comp in p.components() {
        prefix.push(comp);
        let mut cur = prefix.clone();
        let mut settled = false;
        for _ in 0..MAX_LINK_HOPS {
            match std::fs::symlink_metadata(&cur) {
                Ok(m) if m.file_type().is_symlink() => match std::fs::read_link(&cur) {
                    Ok(t) => {
                        let ts = t.to_string_lossy();
                        if is_unsafe_link_target(&ts) {
                            return true;
                        }
                        cur = if t.is_absolute() || is_local_drive_target(&ts) {
                            t
                        } else {
                            cur.parent().map(|d| d.join(&t)).unwrap_or(t)
                        };
                    }
                    Err(_) => return true,
                },
                _ => {
                    settled = true;
                    break;
                }
            }
        }
        if !settled {
            return true;
        }
    }
    false
}

/// May content make us open `p`? "Content" is anything the reader did not
/// choose themselves: a link in a document, a workspace file, a path sent
/// by another launch, a tree entry that is itself a link. Every link along
/// the path is checked (never followed), so this must be called before
/// anything that follows links (`is_file`, `metadata`, `Document::open` …).
/// `Err` is a plain reason for the status bar.
///
/// The path's spelling is not judged here. The part that content supplies
/// is checked as text where it is read ([`classify`], `Workspace::parse`,
/// `single::parse`); the folder it is resolved against is where the reader
/// opened the document or workspace, which may be a network share they
/// chose (`\\server\docs\notes.md` and the files next to it keep working).
///
/// Not for the reader's own choices (file dialog, command line, drag and
/// drop, their saved session, Save As): a link they made to their own
/// share keeps working.
pub fn content_may_open(p: &Path) -> Result<(), &'static str> {
    if crate::doc::is_reserved_name(p) {
        return Err("device name");
    }
    if link_reaches_unsafe(p) {
        return Err("it is a link to a network share or device path");
    }
    Ok(())
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
    // Check the whole thing first: `\??\` contains a `?` that the query
    // split below would cut away.
    let whole = percent_decode(dest);
    if is_network_path(&whole) || is_unsafe_local_path(whole.split('#').next().unwrap_or("")) {
        return LinkTarget::Blocked("this kind of path is not opened");
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
    if is_unsafe_local_path(&decoded) {
        return LinkTarget::Blocked("this kind of path is not opened");
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

    #[cfg(unix)]
    #[test]
    fn long_chains_and_loops_of_links_count_as_unsafe() {
        let d = std::env::temp_dir().join(format!("litedit-chain-len-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("real.txt"), b"x").unwrap();
        // l0 -> real.txt, l1 -> l0, ... : a chain of `n` local links.
        let chain = |n: usize| {
            std::os::unix::fs::symlink(d.join("real.txt"), d.join("l0")).unwrap();
            for i in 1..n {
                std::os::unix::fs::symlink(d.join(format!("l{}", i - 1)), d.join(format!("l{i}")))
                    .unwrap();
            }
            d.join(format!("l{}", n - 1))
        };
        let short = chain(3);
        assert!(!link_reaches_unsafe(&short), "a short local chain is fine");
        for i in 0..3 {
            std::fs::remove_file(d.join(format!("l{i}"))).unwrap();
        }
        let long = chain(MAX_LINK_HOPS + 2);
        assert!(
            link_reaches_unsafe(&long),
            "too long a chain is not followed"
        );
        std::os::unix::fs::symlink(d.join("loop-b"), d.join("loop-a")).unwrap();
        std::os::unix::fs::symlink(d.join("loop-a"), d.join("loop-b")).unwrap();
        assert!(link_reaches_unsafe(&d.join("loop-a")), "a loop is unsafe");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn link_chains_and_linked_folders_to_network_paths_are_caught() {
        let d = std::env::temp_dir().join(format!("litedit-chain-net-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("real")).unwrap();
        std::fs::write(d.join("real/x.png"), b"x").unwrap();
        std::os::unix::fs::symlink("//evil/share", d.join("mid")).unwrap();
        std::os::unix::fs::symlink(d.join("mid"), d.join("top")).unwrap();
        std::os::unix::fs::symlink(d.join("real"), d.join("ok")).unwrap();
        assert!(link_reaches_unsafe(&d.join("top")));
        assert!(
            link_reaches_unsafe(&d.join("top/x.png")),
            "linked folder mid-path"
        );
        assert!(
            !link_reaches_unsafe(&d.join("ok/x.png")),
            "local links are fine"
        );
        assert!(!link_reaches_unsafe(&d.join("real/x.png")));
        assert!(is_local_drive_target("\\\\?\\C:\\x") && is_local_drive_target("\\??\\D:\\y"));
        assert!(!is_local_drive_target("\\\\?\\UNC\\evil\\x"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn content_may_open_refuses_links_to_network_paths_without_following_them() {
        let d = std::env::temp_dir().join(format!("litedit-may-open-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("real")).unwrap();
        std::fs::write(d.join("real/a.md"), b"a").unwrap();
        std::os::unix::fs::symlink("//evil/share/x.md", d.join("net.md")).unwrap();
        std::os::unix::fs::symlink("//evil/share", d.join("netdir")).unwrap();
        std::os::unix::fs::symlink(d.join("real/a.md"), d.join("local.md")).unwrap();
        std::os::unix::fs::symlink(d.join("real"), d.join("localdir")).unwrap();
        let why = content_may_open(&d.join("net.md")).unwrap_err();
        assert_eq!(why, "it is a link to a network share or device path");
        assert!(content_may_open(&d.join("netdir/x.md")).is_err());
        assert!(content_may_open(&d.join("netdir")).is_err());
        assert_eq!(content_may_open(&d.join("local.md")), Ok(()));
        assert_eq!(content_may_open(&d.join("localdir/a.md")), Ok(()));
        assert_eq!(content_may_open(&d.join("real/a.md")), Ok(()));
        // Missing files are not this check's business.
        assert_eq!(content_may_open(&d.join("nope.md")), Ok(()));
        assert!(content_may_open(Path::new("/docs/CON")).is_err());
        // A document the reader opened on a share: the files next to it are
        // reached through the same spelling and stay allowed (on Linux a
        // leading `//` is the same folder, which makes this testable).
        let shared = PathBuf::from(format!("/{}", d.display()));
        assert!(is_network_path(&shared.to_string_lossy()) && shared.join("real/a.md").is_file());
        assert_eq!(content_may_open(&shared.join("real/a.md")), Ok(()));
        assert!(content_may_open(&shared.join("net.md")).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn mapped_drive_paths_and_reparse_targets_are_refused_before_io() {
        fn z_is_remote(root: &[u16; 4]) -> DriveSafety {
            assert_eq!(root[1..], [b':' as u16, b'\\' as u16, 0]);
            if root[0] == b'Z' as u16 {
                DriveSafety::Unsafe
            } else {
                DriveSafety::Local
            }
        }

        for path in ["Z:\\pics\\a.png", "z:/pics/a.png", "Z:pics\\a.png"] {
            assert!(is_unsafe_local_path_with(path, z_is_remote), "{path}");
        }
        assert!(!is_unsafe_local_path_with("C:\\pics\\a.png", z_is_remote));
        assert!(is_unsafe_link_target_with(
            "\\\\?\\Z:\\pics\\a.png",
            z_is_remote
        ));
        assert!(!is_unsafe_link_target_with(
            "\\??\\C:\\pics\\a.png",
            z_is_remote
        ));
        assert!(is_unsafe_link_target_with(
            "\\\\?\\UNC\\evil\\share\\a.png",
            z_is_remote
        ));
    }

    #[test]
    fn relative_content_paths_do_not_reclassify_the_reader_chosen_base() {
        assert!(!is_unsafe_local_path_with("pics/a.png", |_| {
            DriveSafety::Unsafe
        }));
    }

    #[test]
    fn device_nt_and_stream_paths_are_blocked() {
        let base = std::path::Path::new("/docs");
        for d in [
            "%5C%3F%3F%5CUNC%5Cevil%5Cshare%5Cx.png",
            "\\??\\C:\\x.png",
            "\\??\\GLOBALROOT\\Device\\Mup\\evil\\x.png",
            "\\\\?\\UNC\\evil\\x.png",
            "\\\\.\\pipe\\x",
            "/??/C:/x.png",
            "\\.\\x",
            "img.png:secret",
            "C:\\a\\b.png:stream",
        ] {
            assert!(
                matches!(classify(d, base), LinkTarget::Blocked(_)),
                "{d} must be blocked"
            );
        }
        for d in [
            "img/a.png",
            "C:\\pics\\a.png",
            "C:/pics/a.png",
            "../up.png",
            "a%20b.png",
        ] {
            assert!(matches!(classify(d, base), LinkTarget::File(_)), "{d}");
        }
    }

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
