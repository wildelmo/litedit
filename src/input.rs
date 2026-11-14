//! Platform-independent input events and requests.
//!
//! The window (winit) only *translates* into [`Input`]; the app never sees
//! winit types. This lets `--script` mode and unit tests drive the exact same
//! code paths as a real user.

use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Home,
    End,
    Enter,
    Escape,
    Tab,
    Backspace,
    Delete,
    Insert,
    F(u8),
    /// A printable key, lower-cased for letters (shift is in [`Mods`]).
    Char(char),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WheelUnit {
    Lines,
    Pixels,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    Resize(u32, u32),
    Scale(f32),
    MouseMove {
        x: f32,
        y: f32,
    },
    MouseDown {
        x: f32,
        y: f32,
        button: MouseButton,
        clicks: u32,
    },
    MouseUp {
        x: f32,
        y: f32,
        button: MouseButton,
    },
    MouseLeave,
    /// Positive `dy` scrolls content down (towards the end of the file).
    Wheel {
        dx: f32,
        dy: f32,
        unit: WheelUnit,
    },
    /// Modifier keys held now (for Shift+click, Ctrl+click, Ctrl+wheel).
    Modifiers(Mods),
    Key {
        key: Key,
        mods: Mods,
    },
    Text(String),
    Drop(PathBuf),
    /// Files changed on disk (from the watcher thread).
    FsChanged(Vec<PathBuf>),
    /// The user asked to close the window.
    CloseRequested,
}

/// Things the app asks the platform layer to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// Already validated as http/https/mailto.
    OpenUrl(String),
    OpenFileDialog,
    OpenFolderDialog,
    /// Ask where to save the current file (suggested path).
    SaveAsDialog(PathBuf),
    /// Ctrl+N with no folder open: ask where to create a new file.
    NewFileDialog,
    /// Ask where to save the workspace file (suggested path).
    SaveWorkspaceDialog(PathBuf),
    /// Ask which workspace file to open.
    OpenWorkspaceDialog,
    /// Open a local file in litedit (handled by the app itself).
    OpenPath(PathBuf),
    /// Open a file at a 1-based line (search results).
    OpenPathAt(PathBuf, usize),
    /// Show a file in Windows Explorer.
    RevealInExplorer(PathBuf),
    Quit,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Cursor {
    #[default]
    Default,
    Pointer,
    Text,
    ResizeHorizontal,
}

/// Turn one physical key press into app inputs. Pure, so it is unit-tested.
///
/// - Printable text is delivered as [`Input::Text`]; the key itself as
///   [`Input::Key`] (views ignore plain `Key::Char` when editing).
/// - On Windows, AltGr arrives as Ctrl+Alt. A Ctrl+Alt press that produces
///   printable text (e.g. `@`, `{`, `€` on German/French layouts) is typing,
///   not a shortcut, so only the text is delivered.
pub fn translate_key(key: Option<Key>, text: Option<&str>, mods: Mods) -> Vec<Input> {
    let printable = text.filter(|t| !t.is_empty() && t.chars().all(|c| !c.is_control()));
    if mods.ctrl && mods.alt {
        if let Some(t) = printable {
            return vec![Input::Text(t.to_string())];
        }
    }
    let mut out = Vec::new();
    if let Some(k) = key {
        out.push(Input::Key { key: k, mods });
    }
    if !mods.ctrl
        && !mods.alt
        && let Some(t) = printable
    {
        out.push(Input::Text(t.to_string()));
    }
    out
}

/// Parse "ctrl+shift+o", "pagedown", "a", "F5" (used by scripts and tests).
pub fn parse_key_combo(s: &str) -> Option<(Key, Mods)> {
    let mut mods = Mods::default();
    let parts: Vec<&str> = s.split('+').collect();
    let (last, prefix) = parts.split_last()?;
    for p in prefix {
        match p.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => mods.ctrl = true,
            "shift" => mods.shift = true,
            "alt" => mods.alt = true,
            _ => return None,
        }
    }
    let l = last.to_ascii_lowercase();
    let key = match l.as_str() {
        "up" => Key::Up,
        "down" => Key::Down,
        "left" => Key::Left,
        "right" => Key::Right,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        "home" => Key::Home,
        "end" => Key::End,
        "enter" | "return" => Key::Enter,
        "escape" | "esc" => Key::Escape,
        "tab" => Key::Tab,
        "backspace" => Key::Backspace,
        "delete" | "del" => Key::Delete,
        "insert" => Key::Insert,
        "space" => Key::Char(' '),
        "plus" => Key::Char('+'),
        "minus" => Key::Char('-'),
        _ if l.len() > 1 && l.starts_with('f') && l[1..].parse::<u8>().is_ok() => {
            Key::F(l[1..].parse().ok()?)
        }
        _ => {
            let mut chars = l.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            Key::Char(c)
        }
    };
    Some((key, mods))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combos() {
        assert_eq!(
            parse_key_combo("ctrl+shift+o"),
            Some((
                Key::Char('o'),
                Mods {
                    ctrl: true,
                    shift: true,
                    alt: false
                }
            ))
        );
        assert_eq!(
            parse_key_combo("PageDown"),
            Some((Key::PageDown, Mods::default()))
        );
        assert_eq!(parse_key_combo("f5"), Some((Key::F(5), Mods::default())));
        assert_eq!(
            parse_key_combo("ctrl+plus").map(|k| k.0),
            Some(Key::Char('+'))
        );
        assert_eq!(parse_key_combo("hyper+x"), None);
    }

    #[test]
    fn key_translation() {
        let none = Mods::default();
        let ctrl = Mods { ctrl: true, ..none };
        let altgr = Mods {
            ctrl: true,
            alt: true,
            ..none
        };
        // Plain typing: key + text.
        assert_eq!(
            translate_key(Some(Key::Char('a')), Some("a"), none),
            vec![
                Input::Key {
                    key: Key::Char('a'),
                    mods: none
                },
                Input::Text("a".into())
            ]
        );
        // Shortcut: key only, no text.
        assert_eq!(
            translate_key(Some(Key::Char('s')), Some("s"), ctrl),
            vec![Input::Key {
                key: Key::Char('s'),
                mods: ctrl
            }]
        );
        // AltGr (Ctrl+Alt) producing '@' or '€' is typing, not Ctrl+Alt+Q.
        assert_eq!(
            translate_key(Some(Key::Char('@')), Some("@"), altgr),
            vec![Input::Text("@".into())]
        );
        assert_eq!(
            translate_key(Some(Key::Char('€')), Some("€"), altgr),
            vec![Input::Text("€".into())]
        );
        // Enter / Tab produce control text, which is not typed.
        assert_eq!(
            translate_key(Some(Key::Enter), Some("\r"), none),
            vec![Input::Key {
                key: Key::Enter,
                mods: none
            }]
        );
        assert_eq!(
            translate_key(Some(Key::Tab), Some("\t"), none),
            vec![Input::Key {
                key: Key::Tab,
                mods: none
            }]
        );
        // Dead key / IME composition produces no text yet.
        assert!(translate_key(None, None, none).is_empty());
        assert_eq!(parse_key_combo("ab"), None);
    }
}
