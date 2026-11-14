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
    Key {
        key: Key,
        mods: Mods,
    },
    Text(String),
    Drop(PathBuf),
    /// Files changed on disk (from the watcher thread).
    FsChanged(Vec<PathBuf>),
}

/// Things the app asks the platform layer to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// Already validated as http/https/mailto.
    OpenUrl(String),
    OpenFileDialog,
    OpenFolderDialog,
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
        assert_eq!(parse_key_combo("ab"), None);
    }
}
