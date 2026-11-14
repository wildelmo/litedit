//! Colours and metrics, copied from lite-xl's `data/colors/default.lua` and
//! `data/core/style.lua`. All pixel metrics are multiplied by the DPI scale.

use crate::canvas::Color;

pub const BACKGROUND: Color = Color::hex(0x2e2e32); // document area
pub const BACKGROUND2: Color = Color::hex(0x252529); // side panels, title, status
pub const TEXT: Color = Color::hex(0x97979c);
pub const ACCENT: Color = Color::hex(0xe1e1e6);
pub const DIM: Color = Color::hex(0x525257);
pub const DIVIDER: Color = Color::hex(0x202024);
pub const LINE_NUMBER: Color = Color::hex(0x525259);
pub const SCROLLBAR: Color = Color::hex(0x414146);
pub const ERROR: Color = Color::hex(0xff3333);

pub const GOOD: Color = Color::hex(0x72b886);

// Syntax colours (lite-xl `style.syntax`).
pub const SYNTAX_NORMAL: Color = Color::hex(0xe1e1e6);
pub const SYNTAX_COMMENT: Color = Color::hex(0x676b6f);
pub const SYNTAX_KEYWORD: Color = Color::hex(0xe58ac9);
pub const SYNTAX_KEYWORD2: Color = Color::hex(0xf77483);
pub const SYNTAX_NUMBER: Color = Color::hex(0xffa94d);
pub const SYNTAX_LITERAL: Color = Color::hex(0xffa94d);
pub const SYNTAX_STRING: Color = Color::hex(0xf7c95c);
pub const SYNTAX_OPERATOR: Color = Color::hex(0x93ddfa);
pub const SYNTAX_FUNCTION: Color = Color::hex(0x93ddfa);

/// Base sizes in logical pixels (lite-xl defaults).
pub const FONT_SIZE: f32 = 15.0;
pub const CODE_FONT_SIZE: f32 = 15.0;
pub const LINE_HEIGHT: f32 = 1.4;
pub const PADDING_X: f32 = 14.0;
pub const PADDING_Y: f32 = 7.0;
pub const SCROLLBAR_SIZE: f32 = 4.0;
