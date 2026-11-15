//! User settings: `%APPDATA%\litedit\settings.txt`, plain `key = value`
//! lines with `#` comments (lite-xl keeps its config in a user Lua module;
//! litedit runs no user code, so this is data only).
//!
//! Only the window reads this file; headless modes (`--screenshot`,
//! `--script`, `--bench`) and tests always use the defaults so their output
//! never depends on the machine. Saving the file from litedit applies it.

use std::path::{Path, PathBuf};

use crate::theme;

/// Larger files are ignored (it is a handful of lines).
pub const MAX_BYTES: u64 = 64 * 1024;
const SIZE_RANGE: (f32, f32) = (8.0, 40.0);

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub light: bool,
    pub ui_font_size: f32,
    pub code_font_size: f32,
    pub markdown_font_size: f32,
    /// Font family names; read at startup only (font loading is not redone
    /// while running).
    pub ui_font: Option<String>,
    pub code_font: Option<String>,
    /// Soft wrap for code files when they open (markdown always wraps).
    pub wrap_code: bool,
    /// Extra file/folder names to hide, on top of the built-in list.
    pub ignore: Vec<String>,
    /// Remove trailing spaces when saving (not in markdown).
    pub trim_trailing_whitespace: bool,
    /// Files opened while litedit is running go to the running window.
    pub single_window: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            light: false,
            ui_font_size: theme::FONT_SIZE,
            code_font_size: theme::CODE_FONT_SIZE,
            markdown_font_size: theme::MD_FONT_SIZE,
            ui_font: None,
            code_font: None,
            wrap_code: false,
            ignore: Vec::new(),
            trim_trailing_whitespace: false,
            single_window: true,
        }
    }
}

/// Written by "Preferences: Open Settings" when there is no file yet.
pub const TEMPLATE: &str = "\
# litedit settings. Save this file (Ctrl+S) to apply changes.
# Lines are `key = value`; anything after # is a comment.

# dark or light
theme = dark

# Font sizes in pixels at 100% scaling (8 to 40). Ctrl+plus / Ctrl+minus zoom on top of these.
ui_font_size = 15
code_font_size = 15
markdown_font_size = 16

# Font families, e.g. \"Cascadia Mono\" or \"Segoe UI\". Applied at next start.
# ui_font = Segoe UI
# code_font = Consolas

# Soft wrap code files when they open (on/off). Markdown always wraps.
wrap_code = off

# Remove spaces at the ends of lines when saving (on/off; never in markdown).
trim_trailing_whitespace = off

# Opening a file while litedit is already running adds a tab to the running
# window (on) or starts another window (off). `litedit --new-window file`
# always starts another window.
single_window = on

# More file or folder names to hide from the tree, Ctrl+P and find in folder,
# comma-separated (.git, node_modules and similar are always hidden).
# ignore = target, dist, .venv
";

pub fn settings_file() -> Option<PathBuf> {
    crate::session::config_dir().map(|d| d.join("settings.txt"))
}

fn parse_bool(v: &str) -> Option<bool> {
    match v.to_ascii_lowercase().as_str() {
        "on" | "true" | "yes" | "1" => Some(true),
        "off" | "false" | "no" | "0" => Some(false),
        _ => None,
    }
}

fn parse_size(v: &str) -> Option<f32> {
    let n: f32 = v.parse().ok()?;
    n.is_finite().then(|| n.clamp(SIZE_RANGE.0, SIZE_RANGE.1))
}

fn unquote(v: &str) -> &str {
    v.strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(v)
}

impl Settings {
    /// Parse settings text. Returns the settings (defaults for anything
    /// missing or invalid) and one message per line that was not understood.
    pub fn parse(text: &str) -> (Self, Vec<String>) {
        let mut s = Self::default();
        let mut problems = Vec::new();
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        for (n, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                problems.push(format!("line {}: expected key = value", n + 1));
                continue;
            };
            let (k, v) = (k.trim().to_ascii_lowercase(), unquote(v.trim()));
            let ok = match k.as_str() {
                "theme" => match v.to_ascii_lowercase().as_str() {
                    "dark" => {
                        s.light = false;
                        true
                    }
                    "light" => {
                        s.light = true;
                        true
                    }
                    _ => false,
                },
                "ui_font_size" => parse_size(v).map(|x| s.ui_font_size = x).is_some(),
                "code_font_size" => parse_size(v).map(|x| s.code_font_size = x).is_some(),
                "markdown_font_size" => parse_size(v).map(|x| s.markdown_font_size = x).is_some(),
                "ui_font" | "code_font" => {
                    let name = (!v.is_empty() && v.len() <= 100).then(|| v.to_string());
                    if k == "ui_font" {
                        s.ui_font = name;
                    } else {
                        s.code_font = name;
                    }
                    true
                }
                "wrap_code" => parse_bool(v).map(|b| s.wrap_code = b).is_some(),
                "single_window" => parse_bool(v).map(|b| s.single_window = b).is_some(),
                "trim_trailing_whitespace" => parse_bool(v)
                    .map(|b| s.trim_trailing_whitespace = b)
                    .is_some(),
                "ignore" => {
                    s.ignore = v
                        .split(',')
                        .map(|n| unquote(n.trim()).to_string())
                        .filter(|n| !n.is_empty() && n.len() <= 255)
                        .take(200)
                        .collect();
                    true
                }
                _ => {
                    problems.push(format!("line {}: unknown setting '{}'", n + 1, k));
                    continue;
                }
            };
            if !ok {
                problems.push(format!("line {}: bad value for {k}", n + 1));
            }
        }
        (s, problems)
    }

    /// Read a settings file. A missing file gives the defaults silently.
    pub fn load(path: &Path) -> (Self, Vec<String>) {
        match std::fs::metadata(path) {
            Err(_) => (Self::default(), Vec::new()),
            Ok(m) if m.len() > MAX_BYTES => (
                Self::default(),
                vec![format!(
                    "settings file is over {} KB; ignored",
                    MAX_BYTES / 1024
                )],
            ),
            Ok(_) => match std::fs::read(path) {
                Ok(bytes) => Self::parse(&String::from_utf8_lossy(&bytes)),
                Err(e) => (
                    Self::default(),
                    vec![format!("could not read settings: {e}")],
                ),
            },
        }
    }

    /// Make the theme and sizes current (fonts are startup-only; the ignore
    /// list is applied by the app, which also refreshes the tree).
    pub fn apply_globals(&self) {
        theme::set_light(self.light);
        theme::set_sizes(
            self.ui_font_size,
            self.code_font_size,
            self.markdown_font_size,
        );
    }
}

/// `text` with its `theme = …` line set to `light`/`dark`: the first active
/// theme line is rewritten in place (keeping any trailing comment and the
/// line ending); otherwise a line is appended. Everything else is kept
/// byte for byte.
pub fn with_theme(text: &str, light: bool) -> String {
    let value = if light { "light" } else { "dark" };
    let mut out = String::with_capacity(text.len() + 16);
    let mut done = false;
    for piece in text.split_inclusive('\n') {
        if !done {
            let body = piece.trim_end_matches(['\n', '\r']);
            let code = body.split('#').next().unwrap_or("");
            let key = code
                .split_once('=')
                .map(|(k, _)| k.trim().trim_start_matches('\u{feff}').to_ascii_lowercase());
            if key.as_deref() == Some("theme") {
                let eol = &piece[body.len()..];
                let comment = &body[code.len()..];
                let bom = if body.starts_with('\u{feff}') {
                    "\u{feff}"
                } else {
                    ""
                };
                let sep = if comment.is_empty() { "" } else { " " };
                out.push_str(&format!("{bom}theme = {value}{sep}{comment}{eol}"));
                done = true;
                continue;
            }
        }
        out.push_str(piece);
    }
    if !done {
        let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
        if !out.is_empty() && !out.ends_with('\n') {
            out.push_str(eol);
        }
        out.push_str(&format!("theme = {value}{eol}"));
    }
    out
}

/// Same file? Compared without `canonicalize` (it makes `\\?\` paths) and,
/// on Windows, ignoring case and separator style.
pub fn same_path(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        let norm = |p: &Path| p.to_string_lossy().replace('/', "\\").to_lowercase();
        norm(a) == norm(b)
    } else {
        a == b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_parses_to_defaults() {
        let (s, problems) = Settings::parse(TEMPLATE);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(s, Settings::default());
    }

    #[test]
    fn values_are_read_clamped_and_reported() {
        let (s, p) = Settings::parse(
            "\u{feff}theme = Light # bright\r\ncode_font_size=99\nui_font_size = x\n\
             code_font = \"Cascadia Mono\"\nwrap_code = on\nnonsense\ncolour = red\n",
        );
        assert!(s.light);
        assert_eq!(s.code_font_size, 40.0);
        assert_eq!(s.ui_font_size, theme::FONT_SIZE);
        assert_eq!(s.code_font.as_deref(), Some("Cascadia Mono"));
        assert!(s.wrap_code);
        assert_eq!(p.len(), 3, "{p:?}");
        let (s, _) = Settings::parse("ignore = target, \"dist\" ,, .venv\n");
        assert_eq!(s.ignore, vec!["target", "dist", ".venv"]);
        assert!(p[0].contains("ui_font_size"));
        assert!(p[2].contains("colour"));
        // Hostile input never panics.
        let _ = Settings::parse("=\n==\n#\n\u{0}=\u{0}\ntheme=\nui_font_size=NaN\n");
        assert_eq!(
            Settings::parse("ui_font_size = inf").0.ui_font_size,
            theme::FONT_SIZE
        );
    }

    #[test]
    fn theme_line_is_rewritten_in_place() {
        let t = "# c\r\ntheme = dark  # pick one\r\nx = 1\r\n";
        assert_eq!(
            with_theme(t, true),
            "# c\r\ntheme = light # pick one\r\nx = 1\r\n"
        );
        // Commented-out theme lines are not touched; first active one wins.
        let t = "# theme = dark\ntheme=dark\ntheme=dark\n";
        assert_eq!(
            with_theme(t, true),
            "# theme = dark\ntheme = light\ntheme=dark\n"
        );
        assert_eq!(with_theme("a = 1", true), "a = 1\ntheme = light\n");
        assert_eq!(with_theme("", false), "theme = dark\n");
        assert_eq!(
            with_theme("\u{feff}theme = dark\n", true),
            "\u{feff}theme = light\n"
        );
        assert!(Settings::parse(&with_theme(TEMPLATE, true)).0.light);
    }

    #[test]
    fn ignore_setting_hides_names_everywhere() {
        let dir = std::env::temp_dir().join(format!("litedit-ign-{}", std::process::id()));
        let name = format!("zz-ignored-{}", std::process::id());
        std::fs::create_dir_all(dir.join(&name)).unwrap();
        std::fs::write(dir.join(&name).join("a.txt"), "x").unwrap();
        std::fs::write(dir.join("kept.txt"), "x").unwrap();
        let listed = |d: &Path| -> Vec<String> {
            crate::tree::read_dir_sorted(d, true)
                .0
                .into_iter()
                .map(|e| e.name)
                .collect()
        };
        assert!(listed(&dir).contains(&name));
        let (s, _) = Settings::parse(&format!("ignore = {}", name.to_uppercase()));
        crate::tree::set_extra_ignored(s.ignore);
        assert!(
            !listed(&dir).contains(&name),
            "case-insensitive, like Windows"
        );
        let idx = crate::palette::index_files(&dir);
        assert!(
            idx.iter().all(|p| !p.to_string_lossy().contains(&name)),
            "{idx:?}"
        );
        assert!(!crate::watch::interesting(&dir.join(&name).join("a.txt")));
        crate::tree::set_extra_ignored(Vec::new());
        assert!(listed(&dir).contains(&name));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn big_or_missing_files_fall_back_to_defaults() {
        let dir = std::env::temp_dir().join(format!("litedit-set-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (s, p) = Settings::load(&dir.join("none.txt"));
        assert_eq!((s, p.len()), (Settings::default(), 0));
        let big = dir.join("big.txt");
        std::fs::write(&big, "theme = light\n".repeat(10_000)).unwrap();
        let (s, p) = Settings::load(&big);
        assert!(!s.light && p.len() == 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
