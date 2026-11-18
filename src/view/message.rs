//! A view that just shows a centred message (welcome screen, errors).

use crate::canvas::{Canvas, Color, Rect};
use crate::input::Input;
use crate::theme;
use crate::ui::{Ctx, FontKind};
use crate::view::View;

pub struct MessageView {
    title: String,
    lines: Vec<String>,
    color: Color,
}

impl MessageView {
    pub fn folder_opened() -> Self {
        Self {
            title: String::new(),
            lines: vec![
                "Folder opened".into(),
                String::new(),
                "Pick a file in the tree on the left".into(),
                "Ctrl+\\  show / hide the tree    Ctrl+Shift+E  focus the tree".into(),
            ],
            color: theme::p().text,
        }
    }

    pub fn error(title: &str, msg: String) -> Self {
        Self {
            title: title.to_string(),
            lines: vec![msg],
            color: theme::p().error,
        }
    }
}

impl View for MessageView {
    fn render(&mut self, canvas: &mut Canvas, rect: Rect, ctx: &mut Ctx) {
        canvas.fill_rect(rect, theme::p().background);
        let lh = ctx.ui_metrics().line_height as i32;
        let total = lh * self.lines.len() as i32;
        let mut y = rect.y + (rect.h - total) / 2;
        for (i, line) in self.lines.clone().iter().enumerate() {
            let w = ctx.text_width(line, FontKind::Ui) as i32;
            let x = rect.x + ((rect.w - w) / 2).max(ctx.px(theme::PADDING_X) as i32);
            let color = if i == 0 && self.title.is_empty() {
                theme::p().accent
            } else {
                self.color
            };
            ctx.label(canvas, line, x, y, color, rect.right() - x, FontKind::Ui);
            y += lh;
        }
    }

    fn handle(&mut self, _input: &Input, _rect: Rect, _ctx: &mut Ctx) -> bool {
        false
    }

    fn title(&self) -> String {
        self.title.clone()
    }
}
