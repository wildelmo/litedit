//! Remembering the last session and recent files (lite-xl `workspace` +
//! `core.recent_projects`), stored in `%APPDATA%\litedit\session.txt`.
//!
//! A tiny line format (`key<TAB>value`) — no extra dependencies, tolerant of
//! anything unexpected: unknown keys are ignored, a damaged file just means
//! starting fresh. Written with the same atomic save as documents.

use std::path::{Path, PathBuf};

use crate::view::ViewPos;

pub const MAX_RECENT: usize = 15;
const HEADER: &str = "litedit-session 1";

#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    pub root: Option<PathBuf>,
    /// More folders shown in the tree (workspace folders after the first).
    pub folders: Vec<PathBuf>,
    pub tabs: Vec<PathBuf>,
    /// Side of each tab (0 left, 1 right; missing = left).
    pub groups: Vec<u8>,
    /// Where the reader was in each tab (missing or None = at the top).
    pub positions: Vec<Option<ViewPos>>,
    pub active: usize,
    /// Most recent first; folders and files.
    pub recent: Vec<PathBuf>,
    pub tree_width: f32,
    pub tree_visible: bool,
    pub zoom: f32,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            root: None,
            folders: Vec::new(),
            tabs: Vec::new(),
            groups: Vec::new(),
            positions: Vec::new(),
            active: 0,
            recent: Vec::new(),
            tree_width: 260.0,
            tree_visible: true,
            zoom: 1.0,
        }
    }
}

/// Where litedit keeps its settings.
pub fn config_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("litedit"))
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .map(|d| d.join("litedit"))
    }
}

pub fn session_file() -> Option<PathBuf> {
    config_dir().map(|d| d.join("session.txt"))
}

/// Add `p` to the front of a recent list (deduplicated, capped).
pub fn push_recent(list: &mut Vec<PathBuf>, p: &Path) {
    list.retain(|x| x != p);
    list.insert(0, p.to_path_buf());
    list.truncate(MAX_RECENT);
}

impl Session {
    pub fn parse(text: &str) -> Session {
        let mut s = Session::default();
        let mut lines = text.lines();
        if lines.next().map(str::trim) != Some(HEADER) {
            return s;
        }
        // A `pos` line belongs to the tab line before it.
        let mut pos_for: Option<usize> = None;
        for line in lines {
            let Some((k, v)) = line.split_once('\t') else {
                continue;
            };
            let v = v.trim_end_matches('\r');
            if matches!(k, "tab" | "tab-right") {
                pos_for = None;
            }
            match k {
                "root" if !v.is_empty() => s.root = Some(PathBuf::from(v)),
                "folder" if !v.is_empty() && s.folders.len() < 32 => {
                    s.folders.push(PathBuf::from(v))
                }
                "tab" | "tab-right" if !v.is_empty() => {
                    s.tabs.push(PathBuf::from(v));
                    s.groups.push(u8::from(k == "tab-right"));
                    pos_for = Some(s.positions.len());
                    s.positions.push(None);
                }
                "pos" => {
                    if let Some(slot) = pos_for.and_then(|i| s.positions.get_mut(i)) {
                        *slot = Some(ViewPos::parse(v)).filter(|p| *p != ViewPos::default());
                    }
                }
                "active" => s.active = v.parse().unwrap_or(0),
                "recent" if !v.is_empty() && s.recent.len() < MAX_RECENT => {
                    s.recent.push(PathBuf::from(v))
                }
                "tree_width" => {
                    s.tree_width = v
                        .parse::<f32>()
                        .ok()
                        .filter(|w| w.is_finite())
                        .unwrap_or(260.0)
                        .clamp(100.0, 2000.0)
                }
                "tree_visible" => s.tree_visible = v != "0",
                "zoom" => {
                    s.zoom = v
                        .parse::<f32>()
                        .ok()
                        .filter(|z| z.is_finite())
                        .unwrap_or(1.0)
                        .clamp(0.5, 3.0)
                }
                _ => {}
            }
        }
        s.tabs.truncate(100);
        s.groups.truncate(100);
        s.positions.truncate(100);
        s
    }

    pub fn to_text(&self) -> String {
        let mut out = format!("{HEADER}\n");
        let clean = |p: &Path| p.display().to_string().replace(['\t', '\n', '\r'], " ");
        if let Some(r) = &self.root {
            out.push_str(&format!("root\t{}\n", clean(r)));
        }
        for f in &self.folders {
            out.push_str(&format!("folder\t{}\n", clean(f)));
        }
        for (i, t) in self.tabs.iter().enumerate() {
            let k = if self.groups.get(i) == Some(&1) {
                "tab-right"
            } else {
                "tab"
            };
            out.push_str(&format!("{k}\t{}\n", clean(t)));
            // Older versions ignore this line.
            if let Some(p) = self.positions.get(i).copied().flatten() {
                out.push_str(&format!("pos\t{}\n", p.to_text()));
            }
        }
        out.push_str(&format!("active\t{}\n", self.active));
        for r in &self.recent {
            out.push_str(&format!("recent\t{}\n", clean(r)));
        }
        out.push_str(&format!("tree_width\t{}\n", self.tree_width.round()));
        out.push_str(&format!("tree_visible\t{}\n", u8::from(self.tree_visible)));
        out.push_str(&format!("zoom\t{:.2}\n", self.zoom));
        out
    }

    pub fn load(path: &Path) -> Session {
        std::fs::read_to_string(path)
            .map(|t| Session::parse(&t))
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        crate::editor::save::save_bytes(path, self.to_text().as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_tolerance() {
        let s = Session {
            folders: Vec::new(),
            groups: vec![0, 1],
            positions: vec![
                None,
                Some(ViewPos {
                    line: 9,
                    col: 2,
                    top: 4,
                    ptop: 30,
                    poff: 500_000,
                    mode: 1,
                    ..ViewPos::default()
                }),
            ],
            root: Some(PathBuf::from("/notes")),
            tabs: vec![PathBuf::from("/notes/a.md"), PathBuf::from("/notes/b c.md")],
            active: 1,
            recent: vec![PathBuf::from("/notes")],
            tree_width: 300.0,
            tree_visible: false,
            zoom: 1.2,
        };
        assert_eq!(Session::parse(&s.to_text()), s);
        assert_eq!(Session::parse("garbage"), Session::default());
        assert_eq!(Session::parse(""), Session::default());
        let odd =
            format!("{HEADER}\nunknown\tx\nzoom\tNaN\ntree_width\t-5\nnoTabHere\nactive\tbig\n");
        let p = Session::parse(&odd);
        assert_eq!((p.zoom, p.tree_width, p.active), (1.0, 100.0, 0));
        // A session from a version without positions, and damaged positions.
        let old = format!("{HEADER}\ntab\t/a.md\ntab-right\t/b.md\nactive\t1\n");
        let p = Session::parse(&old);
        assert_eq!((p.tabs.len(), p.positions.clone()), (2, vec![None, None]));
        let odd = format!(
            "{HEADER}\npos\tl=3\ntab\t/a.md\npos\tl=99999999999999999999999 t=18446744073709551615 m=200\ntab\t/b.md\npos\tnonsense\n"
        );
        let p = Session::parse(&odd);
        let a = p.positions[0].unwrap();
        assert_eq!((a.line, a.top, a.mode), (0, u32::MAX as usize, 2));
        assert_eq!(p.positions[1], None);
    }

    #[test]
    fn recent_is_deduped_and_capped() {
        let mut r = Vec::new();
        for i in 0..20 {
            push_recent(&mut r, Path::new(&format!("/f{i}")));
        }
        push_recent(&mut r, Path::new("/f10"));
        assert_eq!(r.len(), MAX_RECENT);
        assert_eq!(r[0], PathBuf::from("/f10"));
        assert_eq!(r.iter().filter(|p| **p == PathBuf::from("/f10")).count(), 1);
    }

    #[test]
    fn save_and_load_file() {
        let d = std::env::temp_dir().join(format!("litedit-sess-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let p = d.join("deeper/session.txt");
        let s = Session {
            folders: Vec::new(),
            groups: Vec::new(),
            positions: Vec::new(),
            zoom: 1.5,
            ..Session::default()
        };
        s.save(&p).unwrap();
        assert_eq!(Session::load(&p).zoom, 1.5);
        assert_eq!(Session::load(&d.join("missing.txt")), Session::default());
        std::fs::remove_dir_all(&d).ok();
    }
}
