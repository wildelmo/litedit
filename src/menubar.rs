//! The menu bar: File, Edit, View, Go, Help, drawn by us (so it follows
//! the theme) as one row at the top of the window. Each menu is a `Menu`
//! from `menu.rs`. Almost every item replays its keyboard shortcut, so a
//! menu can never do something different from the keys (the command
//! palette works the same way); the shortcut is shown next to it.

use crate::canvas::Rect;
use crate::menu::{MenuAction, MenuItem};
use crate::palette::{Action, pretty_keys};
use crate::ui::{Ctx, FontKind};

pub const TITLES: [&str; 5] = ["File", "Edit", "View", "Go", "Help"];

/// Alt + this letter opens menu `i` (the first letter of its title).
pub fn mnemonic(c: char) -> Option<usize> {
    TITLES
        .iter()
        .position(|t| t.chars().next().is_some_and(|f| f.eq_ignore_ascii_case(&c)))
}

enum Entry {
    /// Replays this key combo.
    Keys(&'static str, &'static str),
    /// Replays this key combo in the open file (the tree may have focus).
    Doc(&'static str, &'static str),
    /// Runs this action; the text is the shortcut to show, if any.
    Act(&'static str, &'static str, MenuAction),
    Sep,
}

use Entry::{Act, Doc, Keys, Sep};

/// The items of menu `i`. `menu_bar_on`: the bar is shown (View offers to
/// hide it, or to show it again).
pub fn items(i: usize, menu_bar_on: bool) -> Vec<MenuItem> {
    let entries: Vec<Entry> = match i {
        0 => vec![
            Keys("New File…", "ctrl+n"),
            Keys("Open File…", "ctrl+o"),
            Keys("Open Folder…", "ctrl+shift+o"),
            Keys("Open Recent…", "ctrl+r"),
            Act(
                "Open Workspace…",
                "",
                MenuAction::Palette(Action::OpenWorkspace),
            ),
            Sep,
            Keys("Save", "ctrl+s"),
            Keys("Save As…", "ctrl+shift+s"),
            Act(
                "Save Workspace As…",
                "",
                MenuAction::Palette(Action::SaveWorkspace),
            ),
            Sep,
            Act("Add Folder to Workspace…", "", MenuAction::AddFolder),
            Keys("Close Tab", "ctrl+w"),
            Keys("Reopen Closed Tab", "ctrl+shift+t"),
            Act("Close Folder", "", MenuAction::CloseFolder),
            Sep,
            Keys("Settings", "ctrl+,"),
            Act("Exit", "Alt+F4", MenuAction::Exit),
        ],
        1 => vec![
            Doc("Undo", "ctrl+z"),
            Doc("Redo", "ctrl+y"),
            Sep,
            Doc("Cut", "ctrl+x"),
            Doc("Copy", "ctrl+c"),
            Doc("Paste", "ctrl+v"),
            Doc("Select All", "ctrl+a"),
            Sep,
            Doc("Find", "ctrl+f"),
            Doc("Replace", "ctrl+h"),
            Doc("Find in Folder…", "ctrl+shift+f"),
            Sep,
            Doc("Toggle Comment", "ctrl+/"),
            Doc("Duplicate Line", "ctrl+shift+d"),
            Doc("Delete Line", "ctrl+shift+k"),
            Doc("Move Line Up", "alt+up"),
            Doc("Move Line Down", "alt+down"),
        ],
        2 => vec![
            Keys("Command Palette…", "ctrl+shift+p"),
            Sep,
            Doc("Markdown: Preview / Edit", "ctrl+e"),
            Doc("Markdown: Side by Side", "ctrl+shift+v"),
            Doc("Word Wrap", "alt+z"),
            Sep,
            Keys("File Tree", "ctrl+\\"),
            Keys("Hidden Files", "ctrl+shift+h"),
            Keys("Move Tab to Right Side", "ctrl+alt+right"),
            Keys("Move Tab to Left Side", "ctrl+alt+left"),
            Sep,
            Keys("Zoom In", "ctrl+plus"),
            Keys("Zoom Out", "ctrl+minus"),
            Keys("Reset Zoom", "ctrl+0"),
            Keys("Full Screen", "f11"),
            Act(
                "Light / Dark Theme",
                "",
                MenuAction::Palette(Action::ToggleTheme),
            ),
            Act(
                if menu_bar_on {
                    "Hide Menu Bar"
                } else {
                    "Show Menu Bar"
                },
                "",
                MenuAction::ToggleMenuBar,
            ),
        ],
        3 => vec![
            Keys("Go to File…", "ctrl+p"),
            Doc("Go to Line…", "ctrl+g"),
            Doc("Go to Heading…", "ctrl+shift+g"),
            Sep,
            Keys("Back", "alt+left"),
            Keys("Forward", "alt+right"),
            Sep,
            Keys("Next Tab", "ctrl+tab"),
            Keys("Previous Tab", "ctrl+shift+tab"),
            Keys("Focus File Tree", "ctrl+shift+e"),
        ],
        _ => vec![
            Keys("All Commands and Shortcuts…", "ctrl+shift+p"),
            Act("About litedit", "", MenuAction::About),
        ],
    };
    entries
        .into_iter()
        .map(|e| match e {
            Keys(label, combo) => (
                label.to_string(),
                pretty_keys(combo),
                Some(MenuAction::Keys(combo)),
            ),
            Doc(label, combo) => (
                label.to_string(),
                pretty_keys(combo),
                Some(MenuAction::EditorKeys(combo)),
            ),
            Act(label, keys, a) => (label.to_string(), keys.to_string(), Some(a)),
            Sep => (String::new(), String::new(), None),
        })
        .collect()
}

/// Where each title sits in the bar.
pub fn title_rects(bar: Rect, ctx: &mut Ctx) -> Vec<Rect> {
    let pad = ctx.px(10.0) as i32;
    let mut x = bar.x + ctx.px(4.0) as i32;
    TITLES
        .iter()
        .map(|t| {
            let w = ctx.text_width(t, FontKind::Ui) as i32 + pad * 2;
            let r = Rect::new(x, bar.y, w, bar.h);
            x += w;
            r
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::parse_key_combo;

    #[test]
    fn every_shortcut_in_the_menus_is_a_real_key_combo() {
        for i in 0..TITLES.len() {
            for on in [true, false] {
                let items = items(i, on);
                assert!(
                    items.iter().any(|(_, _, a)| a.is_some()),
                    "menu {i} is empty"
                );
                for (label, _, a) in items {
                    if let Some(MenuAction::Keys(combo) | MenuAction::EditorKeys(combo)) = a {
                        assert!(parse_key_combo(combo).is_some(), "{label}: {combo}");
                    }
                }
            }
        }
    }

    #[test]
    fn mnemonics_are_the_first_letters() {
        assert_eq!(mnemonic('f'), Some(0));
        assert_eq!(mnemonic('E'), Some(1));
        assert_eq!(mnemonic('v'), Some(2));
        assert_eq!(mnemonic('g'), Some(3));
        assert_eq!(mnemonic('h'), Some(4));
        // Alt+Z (wrap), Alt+W (whitespace), Alt+C / Alt+R (find) stay free.
        for c in ['z', 'w', 'c', 'r', '1'] {
            assert_eq!(mnemonic(c), None);
        }
    }
}
