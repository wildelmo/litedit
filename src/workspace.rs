//! Workspaces (lite-xl `workspace` plugin + VS Code's workspace files):
//! the folders in the tree, the open files (and which side of a split each
//! is on) and the active one.
//!
//! - Every folder remembers its own workspace automatically, in
//!   `%APPDATA%\litedit\workspaces\` (opening the folder again brings its
//!   tabs back, like lite-xl).
//! - "Save Workspace As…" writes a `.litedit-workspace` file anywhere;
//!   opening it (dialog, double-click, drag) restores it. Paths inside the
//!   folder of the workspace file are stored relative to it, so a project
//!   can be moved or shared.
//!
//! A workspace file may come from someone else: network shares and device
//! paths in it are ignored (reading one would contact that server).

use std::path::{Path, PathBuf};

use crate::view::ViewPos;

pub const EXT: &str = "litedit-workspace";
const HEADER: &str = "litedit-workspace 1";
/// Bounds on what a (possibly hostile) file can ask for.
const MAX_FOLDERS: usize = 32;
const MAX_TABS: usize = 200;
const MAX_BYTES: u64 = 256 * 1024;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Workspace {
    /// Folders shown in the tree (the first one is the main folder).
    pub folders: Vec<PathBuf>,
    /// Open files with the side (0 = left, 1 = right) each is on.
    pub tabs: Vec<(PathBuf, u8)>,
    /// Index into `tabs` of the file to show.
    pub active: usize,
    /// Where the reader was in each tab (same order as `tabs`; missing or
    /// None = at the top).
    pub positions: Vec<Option<ViewPos>>,
}

pub fn is_workspace_file(p: &Path) -> bool {
    p.extension()
        .is_some_and(|e| e.to_string_lossy().eq_ignore_ascii_case(EXT))
}

/// Where the automatic workspace of `folder` is kept, inside `store`.
pub fn folder_file(store: &Path, folder: &Path) -> PathBuf {
    use std::hash::{Hash, Hasher};
    // Windows paths are case-insensitive: one file per folder, any spelling.
    let key = folder.to_string_lossy().replace('/', "\\").to_lowercase();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut h);
    let name = key
        .trim_end_matches('\\')
        .rsplit('\\')
        .next()
        .filter(|n| !n.is_empty())
        .unwrap_or("root")
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .take(40)
        .collect::<String>();
    store.join(format!("{name}-{:016x}.txt", h.finish()))
}

/// The default store: `%APPDATA%\litedit\workspaces`.
pub fn default_store() -> Option<PathBuf> {
    crate::session::config_dir().map(|d| d.join("workspaces"))
}

impl Workspace {
    pub fn for_folder(folder: &Path) -> Self {
        Self {
            folders: vec![folder.to_path_buf()],
            ..Self::default()
        }
    }

    /// Parse workspace text; relative paths resolve against `base`.
    /// Unsafe paths (network, device) and unknown lines are skipped.
    pub fn parse(text: &str, base: Option<&Path>) -> Option<Self> {
        let mut lines = text.strip_prefix('\u{feff}').unwrap_or(text).lines();
        if lines.next().map(str::trim) != Some(HEADER) {
            return None;
        }
        let resolve = |v: &str| -> Option<PathBuf> {
            let v = v.trim_end_matches('\r').trim();
            if v.is_empty() || crate::links::is_unsafe_local_path(v) {
                return None;
            }
            let p = PathBuf::from(v);
            if crate::doc::is_reserved_name(&p) {
                return None;
            }
            Some(match base {
                Some(b) if p.is_relative() => b.join(p),
                _ => p,
            })
        };
        let mut w = Self::default();
        // A `pos` line belongs to the tab line before it — only if that
        // tab was accepted.
        let mut pos_for: Option<usize> = None;
        for line in lines {
            let Some((k, v)) = line.split_once('\t') else {
                continue;
            };
            if matches!(k, "tab" | "tab-right") {
                pos_for = None;
            }
            match k {
                "folder" if w.folders.len() < MAX_FOLDERS => {
                    if let Some(p) = resolve(v)
                        && !w.folders.contains(&p)
                    {
                        w.folders.push(p);
                    }
                }
                "tab" | "tab-right" if w.tabs.len() < MAX_TABS => {
                    if let Some(p) = resolve(v) {
                        pos_for = Some(w.tabs.len());
                        w.tabs.push((p, u8::from(k == "tab-right")));
                        w.positions.push(None);
                    }
                }
                "pos" => {
                    if let Some(slot) = pos_for.and_then(|i| w.positions.get_mut(i)) {
                        *slot = Some(ViewPos::parse(v)).filter(|p| *p != ViewPos::default());
                    }
                }
                "active" => w.active = v.trim().parse().unwrap_or(0),
                _ => {}
            }
        }
        w.active = w.active.min(w.tabs.len().saturating_sub(1));
        Some(w)
    }

    /// Text form; paths inside `base` are written relative to it.
    pub fn to_text(&self, base: Option<&Path>) -> String {
        let show = |p: &Path| -> String {
            let rel = base
                .and_then(|b| p.strip_prefix(b).ok())
                .filter(|r| !r.as_os_str().is_empty());
            let s = match rel {
                Some(r) => r.display().to_string(),
                None if base.is_some_and(|b| b == p) => ".".to_string(),
                None => p.display().to_string(),
            };
            s.replace(['\t', '\n', '\r'], " ")
        };
        let mut out = format!("{HEADER}\n");
        for f in &self.folders {
            out.push_str(&format!("folder\t{}\n", show(f)));
        }
        for (i, (t, g)) in self.tabs.iter().enumerate() {
            let k = if *g == 1 { "tab-right" } else { "tab" };
            out.push_str(&format!("{k}\t{}\n", show(t)));
            // Older versions ignore this line.
            if let Some(p) = self.positions.get(i).copied().flatten() {
                out.push_str(&format!("pos\t{}\n", p.to_text()));
            }
        }
        out.push_str(&format!("active\t{}\n", self.active));
        out
    }

    /// Read a workspace file (relative paths are relative to its folder).
    pub fn load(path: &Path) -> Result<Self, String> {
        let bytes = match crate::fileio::read_bounded(path, MAX_BYTES) {
            Ok(loaded) => loaded.bytes,
            Err(crate::fileio::ReadError::TooLarge(_)) => {
                return Err("workspace file is too large".into());
            }
            Err(e) => return Err(e.to_string()),
        };
        let text = String::from_utf8_lossy(&bytes);
        Self::parse(&text, path.parent()).ok_or_else(|| "not a litedit workspace file".into())
    }

    /// Write a workspace file (atomic save; paths relative to its folder).
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        crate::editor::save::save_bytes(path, self.to_text(path.parent()).as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_with_relative_paths() {
        let base = Path::new("/proj");
        let w = Workspace {
            folders: vec![base.to_path_buf(), PathBuf::from("/other/lib")],
            tabs: vec![
                (base.join("src/main.rs"), 0),
                (base.join("README.md"), 1),
                (PathBuf::from("/other/lib/x.rs"), 0),
            ],
            active: 1,
            positions: vec![
                Some(ViewPos {
                    line: 41,
                    col: 7,
                    top: 30,
                    top_off: 500,
                    ..ViewPos::default()
                }),
                None,
                Some(ViewPos {
                    ptop: 12,
                    poff: 250_000,
                    mode: 2,
                    ..ViewPos::default()
                }),
            ],
        };
        let text = w.to_text(Some(base));
        assert!(text.contains("folder\t.\n"), "{text}");
        assert!(
            text.contains("tab\tsrc/main.rs") || text.contains("tab\tsrc\\main.rs"),
            "{text}"
        );
        assert!(text.contains("tab-right\tREADME.md"));
        // Positions are their own lines: the tab lines are as before, so a
        // path may contain anything and stays relative.
        assert!(
            text.contains("main.rs\npos\tl=41 c=7 t=30 o=500\ntab-right\tREADME.md\ntab\t"),
            "{text}"
        );
        assert_eq!(Workspace::parse(&text, Some(base)).unwrap(), w);
        // Moved elsewhere: relative entries follow the file.
        let moved = Workspace::parse(&text, Some(Path::new("/new"))).unwrap();
        assert_eq!(moved.tabs[0].0, Path::new("/new").join("src/main.rs"));
    }

    #[test]
    fn hostile_files_are_bounded_and_network_paths_skipped() {
        let mut t = format!("{HEADER}\n");
        t.push_str("folder\t\\\\evil\\share\n");
        t.push_str("tab\t//evil/share/x.md\n");
        t.push_str("tab\t\\??\\UNC\\evil\\x.md\n");
        t.push_str("tab\tCON\n");
        t.push_str("tab\tok.md\n");
        t.push_str("active\t999\n");
        for i in 0..1000 {
            t.push_str(&format!("tab\tf{i}.md\n"));
        }
        let w = Workspace::parse(&t, Some(Path::new("/p"))).unwrap();
        assert!(w.folders.is_empty());
        assert_eq!(w.tabs[0].0, Path::new("/p/ok.md"));
        assert_eq!(w.tabs.len(), MAX_TABS);
        assert!(w.active < w.tabs.len());
        assert!(Workspace::parse("something else", None).is_none());
    }

    #[test]
    fn load_accepts_a_valid_file_and_rejects_bytes_over_the_limit() {
        let dir =
            std::env::temp_dir().join(format!("litedit-workspace-load-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("project.litedit-workspace");
        let text = format!("{HEADER}\ntab\tnotes.md\nactive\t0\n");
        std::fs::write(&path, &text).unwrap();
        let loaded = Workspace::load(&path).unwrap();
        assert_eq!(loaded.tabs, vec![(dir.join("notes.md"), 0)]);

        std::fs::write(&path, vec![b'x'; MAX_BYTES as usize + 1]).unwrap();
        assert_eq!(
            Workspace::load(&path).unwrap_err(),
            "workspace file is too large"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn files_without_positions_still_load_and_hostile_positions_are_capped() {
        // Written by a version that knew nothing about positions.
        let old = format!("{HEADER}\nfolder\t.\ntab\ta.md\ntab-right\tb.md\nactive\t1\n");
        let w = Workspace::parse(&old, Some(Path::new("/p"))).unwrap();
        assert_eq!(w.tabs.len(), 2);
        assert_eq!(w.positions, vec![None, None]);
        assert_eq!(w.active, 1);
        assert!(!w.to_text(Some(Path::new("/p"))).contains("pos\t"));

        let mut t = format!("{HEADER}\n");
        // Before any tab: belongs to nothing.
        t.push_str("pos\tl=5\n");
        t.push_str("tab\ta.md\n");
        t.push_str(
            "pos\tl=18446744073709551615 c=99999999999 t=4294967296 o=99999999999 p=-3 f=x m=9\n",
        );
        // A rejected tab's position must not land on the tab before it.
        t.push_str("tab\tb.md\n");
        t.push_str("tab\t\\\\evil\\share\\c.md\n");
        t.push_str("pos\tl=77\n");
        t.push_str("tab\td.md\n");
        t.push_str("pos\t\n");
        t.push_str("pos\tgarbage \u{0} = == l=\n");
        let w = Workspace::parse(&t, Some(Path::new("/p"))).unwrap();
        assert_eq!(w.tabs.len(), 3);
        let a = w.positions[0].unwrap();
        let cap = u32::MAX as usize;
        assert_eq!((a.line, a.col, a.top), (cap, cap, cap));
        assert_eq!((a.top_off, a.ptop, a.poff, a.mode), (1_000_000, 0, 0, 2));
        assert_eq!(w.positions[1], None, "b.md has no position of its own");
        assert_eq!(w.positions[2], None, "nothing usable: at the top");
        // Past the tab limit nothing is kept, positions included.
        for i in 0..1000 {
            t.push_str(&format!("tab\tf{i}.md\npos\tl={i}\n"));
        }
        let w = Workspace::parse(&t, Some(Path::new("/p"))).unwrap();
        assert_eq!(w.tabs.len(), MAX_TABS);
        assert_eq!(w.positions.len(), MAX_TABS);
        assert_eq!(
            w.positions[MAX_TABS - 1].map(|p| p.line),
            Some(MAX_TABS - 4)
        );
    }

    #[test]
    fn folder_store_names_are_stable_and_case_insensitive() {
        let s = Path::new("/store");
        let a = folder_file(s, Path::new("C:/Code/Proj"));
        let b = folder_file(s, Path::new("c:\\code\\proj"));
        assert_eq!(a, b);
        assert_ne!(a, folder_file(s, Path::new("C:/Code/Other")));
        assert!(a.to_string_lossy().contains("proj-"), "{}", a.display());
    }
}
