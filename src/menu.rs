//! A small right-click menu (lite-xl `contextmenu.lua`); the menu bar's
//! menus are the same thing with shortcuts and separators.

use std::path::PathBuf;

use crate::canvas::{Canvas, Rect};
use crate::input::{Input, Key, MouseButton};
use crate::theme;
use crate::ui::{Ctx, FontKind};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuAction {
    NewFile(PathBuf),
    NewFolder(PathBuf),
    Rename(PathBuf),
    Delete(PathBuf),
    Reveal(PathBuf),
    CopyPath(PathBuf),
    /// Search inside this folder (also used for the Ctrl+Shift+F prompt).
    FindInFolder(PathBuf),
    // Tab menu (ids are tab ids).
    KeepTab(u64),
    CloseTab(u64),
    CloseOthers(u64),
    CloseToRight(u64),
    CloseSaved,
    CloseAll,
    RevealInTree(PathBuf),
    // Compare.
    SelectForCompare(PathBuf),
    /// (old, new)
    Compare(PathBuf, PathBuf),
    CompareWithSaved(u64),
    /// Split view: the left file against the right file.
    CompareSides,
    /// Replay a key combo through the normal input path (as if typed).
    Keys(&'static str),
    /// The same, with the keyboard moved to the open file first (menu bar
    /// items that edit or show the document).
    EditorKeys(&'static str),
    /// Workspace folders.
    AddFolder,
    RemoveFolder(PathBuf),
    CloseFolder,
    /// Run a command-palette action (menu bar, tree header).
    Palette(crate::palette::Action),
    // Menu bar.
    Exit,
    About,
    ToggleMenuBar,
    // Tree: copy / paste / duplicate, and delete for several entries.
    CopyEntries(Vec<PathBuf>),
    /// Paste the copied entries into this folder.
    PasteInto(PathBuf),
    Duplicate(Vec<PathBuf>),
    DeleteMany(Vec<PathBuf>),
}

/// One menu row: (label, shortcut shown at the right, action). An item
/// without an action is a separator line.
pub type MenuItem = (String, String, Option<MenuAction>);

pub struct Menu {
    x: f32,
    y: f32,
    items: Vec<MenuItem>,
    rects: Vec<Rect>,
    selected: Option<usize>,
}

/// What the menu wants after an input.
pub enum MenuResult {
    Keep,
    Close,
    Run(MenuAction),
    /// Left / Right arrow: the menu bar moves to the menu beside this one.
    Prev,
    Next,
}

impl Menu {
    pub fn new(x: f32, y: f32, items: Vec<(&str, MenuAction)>) -> Self {
        Self::with_items(
            x,
            y,
            items
                .into_iter()
                .map(|(l, a)| (l.to_string(), String::new(), Some(a)))
                .collect(),
        )
    }

    /// Items with shortcuts and separators (the menu bar's menus).
    pub fn with_items(x: f32, y: f32, items: Vec<MenuItem>) -> Self {
        Self {
            x,
            y,
            items,
            rects: Vec::new(),
            selected: None,
        }
    }

    #[cfg(test)]
    pub fn labels(&self) -> Vec<String> {
        self.items
            .iter()
            .filter(|(_, _, a)| a.is_some())
            .map(|(l, _, _)| l.clone())
            .collect()
    }

    /// Where the item with this label was drawn (tests).
    #[cfg(test)]
    pub fn item_rect(&self, label: &str) -> Option<Rect> {
        let i = self
            .items
            .iter()
            .position(|(l, _, a)| l == label && a.is_some())?;
        self.rects.get(i).copied()
    }

    /// The next item that can be chosen after `from` (or from the start /
    /// end), skipping separators. None if there is none.
    fn step(&self, from: Option<usize>, forward: bool) -> Option<usize> {
        let n = self.items.len();
        if n == 0 {
            return None;
        }
        let mut i = match (from, forward) {
            (Some(s), _) => s.min(n - 1),
            (None, true) => n - 1,
            (None, false) => 0,
        };
        for _ in 0..n {
            i = if forward {
                (i + 1) % n
            } else {
                (i + n - 1) % n
            };
            if self.items[i].2.is_some() {
                return Some(i);
            }
        }
        None
    }

    /// Select the first item (a menu opened from the keyboard).
    pub fn select_first(&mut self) {
        self.selected = self.step(None, true);
    }

    fn hit(&self, x: f32, y: f32) -> Option<usize> {
        self.rects.iter().position(|r| {
            (x as i32) >= r.x
                && (x as i32) < r.right()
                && (y as i32) >= r.y
                && (y as i32) < r.bottom()
        })
    }

    pub fn handle(&mut self, input: &Input) -> MenuResult {
        let action = |m: &Self, i: usize| m.items.get(i).and_then(|it| it.2.clone());
        match input {
            Input::MouseMove { x, y } => {
                self.selected = self.hit(*x, *y).filter(|&i| action(self, i).is_some());
                MenuResult::Keep
            }
            Input::MouseDown { x, y, button, .. } => match self.hit(*x, *y) {
                Some(i) if *button == MouseButton::Left => match action(self, i) {
                    Some(a) => MenuResult::Run(a),
                    // A separator is inside the menu: nothing happens.
                    None => MenuResult::Keep,
                },
                Some(_) => MenuResult::Keep,
                None => MenuResult::Close,
            },
            Input::Key { key, .. } => {
                match key {
                    Key::Escape => return MenuResult::Close,
                    Key::Down => self.selected = self.step(self.selected, true),
                    Key::Up => self.selected = self.step(self.selected, false),
                    Key::Left => return MenuResult::Prev,
                    Key::Right => return MenuResult::Next,
                    Key::Enter => {
                        if let Some(a) = self.selected.and_then(|s| action(self, s)) {
                            return MenuResult::Run(a);
                        }
                    }
                    _ => {}
                }
                MenuResult::Keep
            }
            _ => MenuResult::Keep,
        }
    }

    pub fn render(&mut self, canvas: &mut Canvas, bounds: Rect, ctx: &mut Ctx) {
        let row_h = (ctx.ui_metrics().line_height + ctx.px(8.0)) as i32;
        let sep_h = ctx.px(9.0) as i32;
        let pad = ctx.px(12.0) as i32;
        let slack = ctx.px(4.0) as i32; // so measured labels never get "…"
        let label_w = self
            .items
            .iter()
            .map(|(l, _, _)| ctx.text_width(l, FontKind::Ui) as i32)
            .max()
            .unwrap_or(0);
        let detail_w = self
            .items
            .iter()
            .map(|(_, d, _)| ctx.text_width(d, FontKind::Ui) as i32)
            .max()
            .unwrap_or(0);
        let w = label_w
            + if detail_w > 0 {
                ctx.px(32.0) as i32 + detail_w
            } else {
                0
            }
            + pad * 2
            + slack;
        let h = self
            .items
            .iter()
            .map(|(_, _, a)| if a.is_some() { row_h } else { sep_h })
            .sum::<i32>()
            + ctx.px(8.0) as i32;
        // Keep the menu inside the window.
        let x = (self.x as i32).min(bounds.right() - w - 2).max(bounds.x);
        let y = (self.y as i32).min(bounds.bottom() - h - 2).max(bounds.y);
        let frame = Rect::new(x, y, w, h);
        canvas.fill_rect(Rect::new(x - 1, y - 1, w + 2, h + 2), theme::p().divider);
        canvas.fill_rect(frame, theme::p().background2);
        let lh = ctx.ui_metrics().line_height as i32;
        self.rects.clear();
        let mut ry = y + ctx.px(4.0) as i32;
        for (i, (label, detail, action)) in self.items.iter().enumerate() {
            if action.is_none() {
                canvas.fill_rect(
                    Rect::new(x + pad / 2, ry + sep_h / 2, w - pad, 1),
                    theme::p().divider,
                );
                self.rects.push(Rect::new(x, ry, w, sep_h));
                ry += sep_h;
                continue;
            }
            let r = Rect::new(x, ry, w, row_h);
            if self.selected == Some(i) {
                canvas.fill_rect(r, theme::p().selection);
            }
            let ty = r.y + (row_h - lh) / 2;
            ctx.label(
                canvas,
                label,
                r.x + pad,
                ty,
                theme::p().accent,
                label_w + slack,
                FontKind::Ui,
            );
            if !detail.is_empty() {
                let dw = ctx.text_width(detail, FontKind::Ui) as i32;
                ctx.label(
                    canvas,
                    detail,
                    r.right() - pad - dw,
                    ty,
                    theme::p().dim,
                    dw + 2,
                    FontKind::Ui,
                );
            }
            self.rects.push(r);
            ry += row_h;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Mods;

    fn press(m: &mut Menu, key: Key) -> MenuResult {
        m.handle(&Input::Key {
            key,
            mods: Mods::default(),
        })
    }

    #[test]
    fn arrows_skip_separators_and_never_fail_on_odd_menus() {
        let sep = || (String::new(), String::new(), None);
        let item = |l: &str| (l.to_string(), String::new(), Some(MenuAction::CloseAll));
        let mut m = Menu::with_items(
            0.0,
            0.0,
            vec![sep(), item("a"), sep(), sep(), item("b"), sep()],
        );
        let mut seen = Vec::new();
        for _ in 0..4 {
            press(&mut m, Key::Down);
            seen.push(m.selected);
        }
        assert_eq!(seen, vec![Some(1), Some(4), Some(1), Some(4)]);
        press(&mut m, Key::Up);
        assert_eq!(m.selected, Some(1));
        assert!(matches!(press(&mut m, Key::Left), MenuResult::Prev));
        assert!(matches!(press(&mut m, Key::Right), MenuResult::Next));
        // Empty and separator-only menus: nothing to select, nothing panics.
        for items in [vec![], vec![sep(), sep()]] {
            let mut m = Menu::with_items(0.0, 0.0, items);
            m.select_first();
            for k in [Key::Down, Key::Up, Key::Enter] {
                assert!(matches!(press(&mut m, k), MenuResult::Keep));
            }
            assert_eq!(m.selected, None);
        }
    }
}
