//! In-window question bar (lite-xl `NagView`), e.g. "Save changes?".
//!
//! Modal: while shown, keys and clicks go to the bar. Answer with a click,
//! the first letter of an option, ←/→ + Enter, or Esc (the last option).

use crate::canvas::{Canvas, Rect};
use crate::input::{Input, Key, MouseButton};
use crate::theme;
use crate::ui::{Ctx, FontKind};

/// What to do after a "save changes?" question is answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pending {
    /// Close the tab with this id.
    CloseTab(u64),
    /// Quit (asks about each dirty tab in turn).
    Quit,
    /// Close these tabs, asking about each dirty one in turn (Close Others,
    /// Close All, ...). "Don't save" closes that tab at once; Cancel stops.
    CloseTabs(Vec<u64>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NagAction {
    SaveThen(Pending),
    DiscardThen(Pending),
    ReloadFromDisk,
    KeepMine,
    SaveAnyway,
    /// Move this path to the Recycle Bin.
    Recycle(std::path::PathBuf),
    /// Save As anyway although the file had invalid characters.
    SaveAsAnyway(std::path::PathBuf),
    /// The file changed on disk since it was opened: save over it anyway.
    Overwrite(Option<Pending>),
    /// Show the file on disk next to the unsaved text.
    CompareWithDisk,
    /// Add / remove litedit in Windows' "Open with" list (confirmed).
    OpenWith(bool),
    /// Move an entry into a folder (tree drag and drop).
    MoveInto(std::path::PathBuf, std::path::PathBuf),
    Dismiss,
}

pub struct Nag {
    pub message: String,
    pub options: Vec<(String, NagAction)>,
    selected: usize,
    pub error: bool,
    /// Button rects from the last render (window coordinates).
    buttons: Vec<Rect>,
}

impl Nag {
    pub fn new(message: String, options: Vec<(&str, NagAction)>, error: bool) -> Self {
        Self {
            message,
            options: options
                .into_iter()
                .map(|(l, a)| (l.to_string(), a))
                .collect(),
            selected: 0,
            error,
            buttons: Vec::new(),
        }
    }

    pub fn unsaved(name: &str, then: Pending) -> Self {
        Nag::new(
            format!("Save changes to {name}?"),
            vec![
                ("Save", NagAction::SaveThen(then.clone())),
                ("Don't save", NagAction::DiscardThen(then)),
                ("Cancel", NagAction::Dismiss),
            ],
            false,
        )
    }

    pub fn changed_on_disk(name: &str) -> Self {
        Nag::new(
            format!("{name} changed on disk, and you have unsaved edits."),
            vec![
                // The safe answer first (Enter / Space pick it).
                ("Keep my version", NagAction::KeepMine),
                ("Compare", NagAction::CompareWithDisk),
                ("Reload from disk", NagAction::ReloadFromDisk),
            ],
            false,
        )
    }

    pub fn error(message: String) -> Self {
        Nag::new(message, vec![("OK", NagAction::Dismiss)], true)
    }

    pub fn height(ctx: &Ctx) -> i32 {
        (ctx.ui_metrics().line_height + ctx.px(theme::PADDING_Y) * 3.0) as i32
    }

    /// Feed input to the bar. Returns an action when the user answered.
    pub fn handle(&mut self, input: &Input) -> Option<NagAction> {
        let n = self.options.len();
        match input {
            // Alt+Left / Alt+Right are Back / Forward: not for the question.
            Input::Key {
                key: Key::Left | Key::Right,
                mods,
            } if mods.alt => {}
            Input::Key { key, .. } => match key {
                Key::Left => self.selected = (self.selected + n - 1) % n,
                Key::Right | Key::Tab => self.selected = (self.selected + 1) % n,
                Key::Enter | Key::Char(' ') => return Some(self.options[self.selected].1.clone()),
                Key::Escape => return Some(self.options[n - 1].1.clone()),
                Key::Char(c) => {
                    let hit = self.options.iter().find(|(l, _)| {
                        l.chars()
                            .next()
                            .is_some_and(|f| f.to_ascii_lowercase() == c.to_ascii_lowercase())
                    });
                    if let Some((_, a)) = hit {
                        return Some(a.clone());
                    }
                }
                _ => {}
            },
            Input::MouseDown {
                x,
                y,
                button: MouseButton::Left,
                ..
            } => {
                for (i, r) in self.buttons.iter().enumerate() {
                    if (*x as i32) >= r.x
                        && (*x as i32) < r.right()
                        && (*y as i32) >= r.y
                        && (*y as i32) < r.bottom()
                    {
                        return Some(self.options[i].1.clone());
                    }
                }
            }
            _ => {}
        }
        None
    }

    pub fn render(&mut self, canvas: &mut Canvas, rect: Rect, ctx: &mut Ctx) {
        let bg = if self.error {
            theme::p().nag_error
        } else {
            theme::p().nag_bg
        };
        canvas.fill_rect(rect, bg);
        canvas.fill_rect(
            Rect::new(rect.x, rect.bottom() - 1, rect.w, 1),
            theme::p().divider,
        );
        let pad_x = ctx.px(theme::PADDING_X) as i32;
        let lh = ctx.ui_metrics().line_height as i32;
        let ty = rect.y + (rect.h - lh) / 2;
        // Buttons right-aligned.
        let bpad = ctx.px(10.0) as i32;
        let mut x = rect.right() - pad_x;
        let mut rects = Vec::new();
        for (label, _) in self.options.iter().rev() {
            let w = ctx.text_width(label, FontKind::Ui) as i32 + bpad * 2;
            x -= w;
            rects.push(Rect::new(
                x,
                rect.y + ctx.px(5.0) as i32,
                w,
                rect.h - ctx.px(10.0) as i32,
            ));
            x -= ctx.px(8.0) as i32;
        }
        rects.reverse();
        let msg_w = (x - rect.x - pad_x * 2).max(0);
        ctx.label(
            canvas,
            &self.message,
            rect.x + pad_x,
            ty,
            theme::p().nag_text,
            msg_w,
            FontKind::Ui,
        );
        for (i, r) in rects.iter().enumerate() {
            let selected = i == self.selected;
            canvas.fill_rect(
                *r,
                if selected {
                    theme::p().nag_text
                } else {
                    theme::p().nag_button
                },
            );
            let fg = if selected { bg } else { theme::p().nag_text };
            ctx.label(
                canvas,
                &self.options[i].0,
                r.x + bpad,
                ty,
                fg,
                r.w - bpad,
                FontKind::Ui,
            );
        }
        self.buttons = rects;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Mods;

    fn k(key: Key) -> Input {
        Input::Key {
            key,
            mods: Mods::default(),
        }
    }

    #[test]
    fn keyboard_answers() {
        let mut n = Nag::unsaved("a.md", Pending::Quit);
        assert_eq!(
            n.handle(&k(Key::Enter)),
            Some(NagAction::SaveThen(Pending::Quit))
        );
        assert_eq!(
            n.handle(&k(Key::Char('d'))),
            Some(NagAction::DiscardThen(Pending::Quit))
        );
        assert_eq!(n.handle(&k(Key::Escape)), Some(NagAction::Dismiss));
        n.handle(&k(Key::Right));
        assert_eq!(
            n.handle(&k(Key::Enter)),
            Some(NagAction::DiscardThen(Pending::Quit))
        );
        assert_eq!(n.handle(&k(Key::Char('z'))), None);
    }
}
