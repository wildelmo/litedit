//! Command palette (Ctrl+Shift+P), quick open (Ctrl+P) and go to line
//! (Ctrl+G) — lite-xl's CommandView with fuzzy matching.
//!
//! Commands replay their keyboard shortcut through the normal input path, so
//! the palette can never drift from what the keys actually do.

use std::path::{Path, PathBuf};

use crate::canvas::{Canvas, Rect};
use crate::input::{Input, MouseButton};
use crate::inputbar::{BarEvent, InputBar};
use crate::theme;
use crate::tree::{is_ignored, read_dir_limited};
use crate::ui::{Ctx, FontKind};

/// Quick open lists at most this many files.
pub const MAX_INDEX_FILES: usize = 20_000;
const MAX_ROWS: usize = 12;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Replay a key combo, e.g. "ctrl+s".
    Keys(&'static str),
    Open(PathBuf),
    RevealInExplorer,
    ToggleTheme,
    CompareWithSaved,
    /// Split view: the left file against the right file.
    CompareSides,
    CompareWithClipboard,
    SaveWorkspace,
    OpenWorkspace,
    AddFolder,
    /// A named editing command run by the active view.
    Command(&'static str),
    /// Add / remove litedit in "Open with" for markdown files.
    OpenWith(bool),
    /// Go to the heading on this 0-based source line.
    Heading(usize),
}

#[derive(Clone, Debug)]
pub struct Item {
    pub label: String,
    pub detail: String,
    pub action: Action,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Commands,
    Files,
    GoToLine,
    /// A free-text prompt (new file name, rename).
    Text,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Close,
    Run(Action),
    GoToLine(usize),
    Text(String),
}

pub struct Palette {
    pub mode: Mode,
    bar: InputBar,
    items: Vec<Item>,
    /// (item index, score), best first.
    shown: Vec<usize>,
    selected: usize,
    rows: Vec<Rect>,
    max_line: usize,
    hint: String,
}

/// The command list: (label, shortcut).
pub const COMMANDS: &[(&str, &str)] = &[
    ("File: Open File…", "ctrl+o"),
    ("File: Open Folder…", "ctrl+shift+o"),
    ("File: Open Recent…", "ctrl+r"),
    ("File: Save", "ctrl+s"),
    ("File: Save As…", "ctrl+shift+s"),
    ("File: Close Tab", "ctrl+w"),
    ("Go to File…", "ctrl+p"),
    ("Go to Line…", "ctrl+g"),
    ("Go Back", "alt+left"),
    ("Go Forward", "alt+right"),
    ("Markdown: Go to Heading…", "ctrl+shift+g"),
    ("Find", "ctrl+f"),
    ("Replace", "ctrl+h"),
    ("Find in Folder…", "ctrl+shift+f"),
    ("Find Next", "f3"),
    ("Find Previous", "shift+f3"),
    ("Edit: Undo", "ctrl+z"),
    ("Edit: Redo", "ctrl+y"),
    ("Edit: Select All", "ctrl+a"),
    ("Edit: Select Line", "ctrl+l"),
    ("Edit: Duplicate Line", "ctrl+shift+d"),
    ("Edit: Delete Line", "ctrl+shift+k"),
    ("Edit: Join Lines", "ctrl+j"),
    ("Edit: New Line Below", "ctrl+enter"),
    ("Edit: New Line Above", "ctrl+shift+enter"),
    ("Edit: Move Line Up", "alt+up"),
    ("Edit: Move Line Down", "alt+down"),
    ("Edit: Toggle Comment", "ctrl+/"),
    ("Edit: Select Next Occurrence", "ctrl+d"),
    ("Edit: Select All Occurrences", "ctrl+shift+l"),
    ("Edit: Add Caret Above", "ctrl+shift+up"),
    ("Edit: Add Caret Below", "ctrl+shift+down"),
    ("File: New File…", "ctrl+n"),
    ("File: Reopen Closed Tab", "ctrl+shift+t"),
    ("View: Full Screen", "f11"),
    ("Edit: Indent", "ctrl+]"),
    ("Edit: Outdent", "ctrl+["),
    ("Markdown: Toggle Preview / Edit", "ctrl+e"),
    ("Markdown: Side-by-Side Preview", "ctrl+shift+v"),
    ("View: Toggle Word Wrap", "alt+z"),
    ("View: Split — Move Tab to Right Side", "ctrl+alt+right"),
    ("View: Split — Move Tab to Left Side", "ctrl+alt+left"),
    ("View: Focus Left Side", "ctrl+1"),
    ("View: Focus Right Side", "ctrl+2"),
    ("View: Toggle File Tree", "ctrl+\\"),
    ("View: Focus File Tree", "ctrl+shift+e"),
    ("View: Toggle Hidden Files", "ctrl+shift+h"),
    ("View: Zoom In", "ctrl+plus"),
    ("View: Zoom Out", "ctrl+minus"),
    ("View: Reset Zoom", "ctrl+0"),
    ("Preferences: Open Settings", "ctrl+,"),
    ("Tab: Next", "ctrl+tab"),
    ("Tab: Previous", "ctrl+shift+tab"),
];

/// lite-xl style fuzzy match: all pattern chars must appear in order;
/// consecutive runs and word starts score higher; shorter texts win ties.
pub fn fuzzy_score(pattern: &str, text: &str) -> Option<i32> {
    if pattern.is_empty() {
        return Some(0);
    }
    let pat: Vec<char> = pattern
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|c| !c.is_whitespace())
        .collect();
    if pat.is_empty() {
        return Some(0);
    }
    let mut pi = 0;
    let mut score = 0i32;
    let mut run = 0i32;
    let mut prev: Option<char> = None;
    for c in text.chars() {
        let lc = c.to_lowercase().next().unwrap_or(c);
        if pi < pat.len() && lc == pat[pi] {
            let word_start =
                prev.is_none_or(|p| !p.is_alphanumeric() || (p.is_lowercase() && c.is_uppercase()));
            score += 1 + run * 5 + if word_start { 10 } else { 0 };
            run += 1;
            pi += 1;
        } else {
            run = 0;
            score -= 1;
        }
        prev = Some(c);
    }
    (pi == pat.len()).then_some(score - text.chars().count() as i32 / 10)
}

/// Files under `root`, following the tree's rules (ignore list, hidden
/// files, no symlinked folders), relative paths, capped.
pub fn index_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        // Never read more of one folder than the overall cap still allows.
        let room = MAX_INDEX_FILES.saturating_sub(out.len());
        if room == 0 {
            return out;
        }
        let (entries, _, _) = read_dir_limited(&dir, false, room);
        for e in entries {
            if out.len() >= MAX_INDEX_FILES {
                return out;
            }
            if e.is_dir {
                if !e.is_link && depth < 24 && !is_ignored(&e.name) && !is_cache_dir(&e.path) {
                    stack.push((e.path, depth + 1));
                }
            } else if !(e.is_link && crate::links::link_reaches_unsafe(&e.path)) {
                // (A link to a network share or device path is shown in the
                // tree but never read: searching it would contact the server.)
                out.push(e.path);
            }
        }
    }
    out
}

/// Build/cache folders (Cargo's `target`, many others) contain a standard
/// `CACHEDIR.TAG` file; they are skipped by quick open and folder search.
pub fn is_cache_dir(dir: &Path) -> bool {
    dir.join("CACHEDIR.TAG").is_file()
}

impl Palette {
    pub fn commands(has_file: bool) -> Self {
        let mut items: Vec<Item> = COMMANDS
            .iter()
            .map(|(l, k)| Item {
                label: l.to_string(),
                detail: pretty_keys(k),
                action: Action::Keys(k),
            })
            .collect();
        for (label, cmd) in [
            ("Edit: Upper Case", "upper-case"),
            ("Edit: Lower Case", "lower-case"),
            ("Edit: Trim Trailing Whitespace", "trim-whitespace"),
            ("View: Toggle Read-Only", "toggle-read-only"),
            ("Indentation: Convert to Spaces", "indent-spaces"),
            ("Indentation: Convert to Tabs", "indent-tabs"),
            ("Line Endings: LF (Unix)", "eol-lf"),
            ("Line Endings: CRLF (Windows)", "eol-crlf"),
        ] {
            items.push(Item {
                label: label.into(),
                detail: String::new(),
                action: Action::Command(cmd),
            });
        }
        items.push(Item {
            label: "Preferences: Add litedit to \"Open with\" for Markdown…".into(),
            detail: String::new(),
            action: Action::OpenWith(true),
        });
        items.push(Item {
            label: "Preferences: Remove litedit from \"Open with\"…".into(),
            detail: String::new(),
            action: Action::OpenWith(false),
        });
        items.push(Item {
            label: "File: Save Workspace As…".into(),
            detail: String::new(),
            action: Action::SaveWorkspace,
        });
        items.push(Item {
            label: "File: Open Workspace…".into(),
            detail: String::new(),
            action: Action::OpenWorkspace,
        });
        items.push(Item {
            label: "File: Add Folder to Workspace…".into(),
            detail: String::new(),
            action: Action::AddFolder,
        });
        items.push(Item {
            label: "View: Toggle Light / Dark Theme".into(),
            detail: String::new(),
            action: Action::ToggleTheme,
        });
        // Always listed: where they don't apply, the status bar says why.
        items.push(Item {
            label: "File: Compare Left ↔ Right".into(),
            detail: String::new(),
            action: Action::CompareSides,
        });
        items.push(Item {
            label: "File: Compare with Clipboard".into(),
            detail: String::new(),
            action: Action::CompareWithClipboard,
        });
        items.push(Item {
            label: "Compare: Ignore Whitespace (on / off)".into(),
            detail: "Alt+W".into(),
            action: Action::Command("ignore-whitespace"),
        });
        if has_file {
            items.push(Item {
                label: "File: Compare with Saved".into(),
                detail: String::new(),
                action: Action::CompareWithSaved,
            });
            items.push(Item {
                label: "File: Show in Explorer".into(),
                detail: String::new(),
                action: Action::RevealInExplorer,
            });
        }
        Self::new(Mode::Commands, "Command", items, 0)
    }

    pub fn files(roots: &[PathBuf], files: &[PathBuf]) -> Self {
        let items = files
            .iter()
            .map(|p| {
                // Relative to its folder; with several folders, prefixed by
                // that folder's name.
                let root = roots
                    .iter()
                    .filter(|r| p.starts_with(r))
                    .max_by_key(|r| r.components().count());
                let rel = match root {
                    Some(r) => {
                        let rel = p
                            .strip_prefix(r)
                            .unwrap_or(p)
                            .to_string_lossy()
                            .replace('\\', "/");
                        if roots.len() > 1 {
                            let name = r
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_default();
                            format!("{name}/{rel}")
                        } else {
                            rel
                        }
                    }
                    None => p.to_string_lossy().replace('\\', "/"),
                };
                Item {
                    label: rel,
                    detail: String::new(),
                    action: Action::Open(p.clone()),
                }
            })
            .collect();
        // Say so when the folder has more files than quick open lists.
        let label = if files.len() >= MAX_INDEX_FILES {
            format!("Open file (first {MAX_INDEX_FILES} files)")
        } else {
            "Open file".to_string()
        };
        Self::new(Mode::Files, &label, items, 0)
    }

    /// "Go to Heading": the document outline, indented by level.
    pub fn headings(list: &[(u8, String, usize)]) -> Self {
        let top = list.iter().map(|h| h.0).min().unwrap_or(1);
        let items = list
            .iter()
            .map(|(level, text, line)| Item {
                label: format!("{}{}", "    ".repeat((level - top) as usize), text),
                detail: format!("line {}", line + 1),
                action: Action::Heading(*line),
            })
            .collect();
        Self::new(Mode::Files, "Go to heading", items, 0)
    }

    /// Ask for a line of text (e.g. a file name), prefilled with `initial`.
    pub fn text(label: &str, initial: &str, hint: &str) -> Self {
        let mut p = Self::new(Mode::Text, label, Vec::new(), 0);
        p.bar.set_text(initial);
        p.hint = hint.to_string();
        p
    }

    /// Recently opened folders and files.
    pub fn recent(paths: &[PathBuf]) -> Self {
        let items = paths
            .iter()
            .map(|p| Item {
                label: p.display().to_string(),
                detail: if p.is_dir() {
                    "folder".into()
                } else {
                    "file".into()
                },
                action: Action::Open(p.clone()),
            })
            .collect();
        Self::new(Mode::Files, "Open recent", items, 0)
    }

    pub fn goto_line(max_line: usize) -> Self {
        Self::new(Mode::GoToLine, "Go to line", Vec::new(), max_line)
    }

    fn new(mode: Mode, label: &str, items: Vec<Item>, max_line: usize) -> Self {
        let mut p = Self {
            mode,
            bar: InputBar::new(label, ""),
            items,
            shown: Vec::new(),
            selected: 0,
            rows: Vec::new(),
            max_line,
            hint: String::new(),
        };
        p.refilter();
        p
    }

    fn refilter(&mut self) {
        let q = self.bar.text().to_string();
        let mut scored: Vec<(usize, i32)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, it)| fuzzy_score(&q, &it.label).map(|s| (i, s)))
            .collect();
        if !q.is_empty() {
            scored.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        }
        scored.truncate(500);
        self.shown = scored.into_iter().map(|(i, _)| i).collect();
        self.selected = 0;
    }

    pub fn handle(&mut self, input: &Input, ctx: &mut Ctx) -> Option<Outcome> {
        if let Input::MouseDown {
            x,
            y,
            button: MouseButton::Left,
            ..
        } = input
        {
            let hit = self.rows.iter().position(|r| {
                (*x as i32) >= r.x
                    && (*x as i32) < r.right()
                    && (*y as i32) >= r.y
                    && (*y as i32) < r.bottom()
            });
            return match hit {
                Some(i) => self
                    .shown
                    .get(i)
                    .map(|&k| Outcome::Run(self.items[k].action.clone())),
                None => Some(Outcome::Close),
            };
        }
        match self.bar.handle(input, ctx) {
            BarEvent::Changed => {
                self.refilter();
                None
            }
            BarEvent::Nav(d) => {
                let n = self.shown.len().max(1) as i32;
                self.selected = (self.selected as i32 + d).rem_euclid(n) as usize;
                None
            }
            BarEvent::Submit { .. } => match self.mode {
                Mode::GoToLine => {
                    let t = self.bar.text().trim().to_string();
                    // All digits but too big to parse: the last line.
                    let n: usize = match t.parse() {
                        Ok(n) => n,
                        Err(_) if !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()) => {
                            usize::MAX
                        }
                        Err(_) => return None,
                    };
                    Some(Outcome::GoToLine(n.clamp(1, self.max_line.max(1))))
                }
                Mode::Text => Some(Outcome::Text(self.bar.text().to_string())),
                _ => self
                    .shown
                    .get(self.selected)
                    .map(|&k| Outcome::Run(self.items[k].action.clone())),
            },
            BarEvent::Cancel => Some(Outcome::Close),
            _ => None,
        }
    }

    /// Draw the palette near the top of `area`.
    pub fn render(&mut self, canvas: &mut Canvas, area: Rect, ctx: &mut Ctx) {
        let w = (ctx.px(640.0 * theme::ui_ratio()) as i32)
            .min(area.w - ctx.px(24.0) as i32)
            .max(ctx.px(160.0) as i32);
        let x = area.x + (area.w - w) / 2;
        let row_h = (ctx.ui_metrics().line_height + ctx.px(8.0)) as i32;
        let bar_h = row_h + ctx.px(8.0) as i32;
        let n = self.shown.len().min(MAX_ROWS);
        let hint = match self.mode {
            Mode::GoToLine => Some(format!("Line number (1–{})", self.max_line)),
            Mode::Text => Some(self.hint.clone()),
            _ if self.shown.is_empty() => Some("No matches".to_string()),
            _ => None,
        };
        let rows_h = if hint.is_some() {
            row_h
        } else {
            row_h * n as i32
        };
        let y = area.y + ctx.px(8.0) as i32;
        let frame = Rect::new(x, y, w, bar_h + rows_h + ctx.px(6.0) as i32);
        canvas.fill_rect(
            Rect::new(frame.x - 1, frame.y - 1, frame.w + 2, frame.h + 2),
            theme::p().divider,
        );
        canvas.fill_rect(frame, theme::p().background2);
        self.bar
            .render(canvas, Rect::new(x, y, w, bar_h), ctx, true);
        let lh = ctx.ui_metrics().line_height as i32;
        let pad = ctx.px(12.0) as i32;
        self.rows.clear();
        if let Some(h) = hint {
            ctx.label(
                canvas,
                &h,
                x + pad,
                y + bar_h + (row_h - lh) / 2,
                theme::p().dim,
                w - pad * 2,
                FontKind::Ui,
            );
            return;
        }
        // Keep the selection in view.
        let start = self.selected.saturating_sub(MAX_ROWS - 1);
        for (row, &k) in self.shown.iter().skip(start).take(MAX_ROWS).enumerate() {
            let r = Rect::new(x, y + bar_h + row as i32 * row_h, w, row_h);
            if start + row == self.selected {
                canvas.fill_rect(r, theme::p().selection);
            }
            let item = &self.items[k];
            let dw = if item.detail.is_empty() {
                0
            } else {
                ctx.text_width(&item.detail, FontKind::Ui) as i32 + pad
            };
            let ty = r.y + (row_h - lh) / 2;
            ctx.label(
                canvas,
                &item.label,
                r.x + pad,
                ty,
                theme::p().accent,
                w - pad * 2 - dw,
                FontKind::Ui,
            );
            if dw > 0 {
                ctx.label(
                    canvas,
                    &item.detail,
                    r.right() - dw,
                    ty,
                    theme::p().dim,
                    dw,
                    FontKind::Ui,
                );
            }
            self.rows.push(r);
        }
    }
}

/// "ctrl+shift+p" → "Ctrl+Shift+P".
fn pretty_keys(k: &str) -> String {
    k.split('+')
        .map(|p| match p {
            "plus" => "+".to_string(),
            "minus" => "-".to_string(),
            _ => {
                let mut c = p.chars();
                c.next()
                    .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                    .unwrap_or_default()
            }
        })
        .collect::<Vec<_>>()
        .join("+")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_sees_every_file_of_a_big_folder() {
        let d = std::env::temp_dir().join(format!("litedit-bigdir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        for i in 0..crate::tree::MAX_ENTRIES + 200 {
            std::fs::write(d.join(format!("file{i:05}.txt")), "").unwrap();
        }
        let idx = index_files(&d);
        assert_eq!(idx.len(), crate::tree::MAX_ENTRIES + 200);
        assert!(idx.iter().any(|p| p.ends_with("file05199.txt")));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn index_skips_links_to_network_paths_but_lists_local_ones() {
        let d = std::env::temp_dir().join(format!("litedit-idx-net-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("real.txt"), "x").unwrap();
        std::os::unix::fs::symlink("//evil/share/x", d.join("net.txt")).unwrap();
        std::os::unix::fs::symlink(d.join("real.txt"), d.join("local.txt")).unwrap();
        let names: Vec<String> = index_files(&d)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"real.txt".to_string()));
        assert!(names.contains(&"local.txt".to_string()), "{names:?}");
        assert!(!names.contains(&"net.txt".to_string()), "{names:?}");
        let _ = std::fs::remove_dir_all(&d);
    }

    use crate::input::parse_key_combo;

    #[test]
    fn fuzzy_prefers_word_starts_and_runs() {
        assert!(fuzzy_score("sv", "File: Save").is_some());
        assert!(fuzzy_score("xyz", "File: Save").is_none());
        let a = fuzzy_score("save", "File: Save").unwrap();
        let b = fuzzy_score("save", "Markdown: Side-by-Side Preview").unwrap_or(i32::MIN);
        assert!(a > b);
        assert!(
            fuzzy_score("ma", "src/main.rs").unwrap()
                > fuzzy_score("ma", "src/view/markdown.rs").unwrap_or(i32::MIN) - 50
        );
    }

    #[test]
    fn every_command_shortcut_parses() {
        for (label, k) in COMMANDS {
            assert!(parse_key_combo(k).is_some(), "{label}: {k}");
        }
        assert_eq!(pretty_keys("ctrl+shift+p"), "Ctrl+Shift+P");
        assert_eq!(pretty_keys("ctrl+plus"), "Ctrl++");
    }

    #[test]
    fn file_index_follows_tree_rules() {
        let d = std::env::temp_dir().join(format!("litedit-idx-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("src/deep")).unwrap();
        std::fs::create_dir_all(d.join("node_modules/x")).unwrap();
        std::fs::write(d.join("src/deep/a.rs"), "").unwrap();
        std::fs::write(d.join("node_modules/x/b.js"), "").unwrap();
        std::fs::write(d.join(".hidden"), "").unwrap();
        std::fs::write(d.join("README.md"), "").unwrap();
        std::fs::create_dir_all(d.join("target/debug")).unwrap();
        std::fs::write(
            d.join("target/CACHEDIR.TAG"),
            "Signature: 8a477f597d28d172789f06886806bc55",
        )
        .unwrap();
        std::fs::write(d.join("target/debug/junk.o"), "").unwrap();
        let files = index_files(&d);
        let names: Vec<String> = files
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"a.rs".to_string()));
        assert!(names.contains(&"README.md".to_string()));
        assert!(!names.contains(&"b.js".to_string()));
        assert!(!names.contains(&".hidden".to_string()));
        assert!(
            !names.contains(&"junk.o".to_string()),
            "cache dirs are skipped"
        );
        std::fs::remove_dir_all(&d).ok();
    }
}
