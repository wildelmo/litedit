//! Colours and metrics. The dark palette is lite-xl's default
//! (`data/colors/default.lua`); the light palette is GitHub-like. All pixel
//! metrics are multiplied by the DPI scale.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::canvas::Color;

/// Every colour litedit draws with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub background: Color,
    pub background2: Color,
    pub text: Color,
    pub accent: Color,
    pub dim: Color,
    pub divider: Color,
    pub line_number: Color,
    pub line_number2: Color,
    pub caret: Color,
    pub find_match: Color,
    pub line_highlight: Color,
    pub selection: Color,
    pub scrollbar: Color,
    pub error: Color,
    pub good: Color,
    pub nag_bg: Color,
    pub nag_error: Color,
    pub nag_button: Color,
    pub nag_text: Color,
    pub syntax_normal: Color,
    pub syntax_comment: Color,
    pub syntax_keyword: Color,
    pub syntax_keyword2: Color,
    pub syntax_number: Color,
    pub syntax_literal: Color,
    pub syntax_string: Color,
    pub syntax_operator: Color,
    pub syntax_function: Color,
    pub md_text: Color,
    pub md_heading: Color,
    pub md_muted: Color,
    pub md_link: Color,
    pub md_code_bg: Color,
    pub md_block_bg: Color,
    pub md_border: Color,
    pub md_zebra: Color,
    pub md_note: Color,
    pub md_tip: Color,
    pub md_important: Color,
    pub md_warning: Color,
    pub md_caution: Color,
}

pub const DARK: Palette = Palette {
    background: Color::hex(0x2e2e32),
    background2: Color::hex(0x252529),
    text: Color::hex(0x97979c),
    accent: Color::hex(0xe1e1e6),
    dim: Color::hex(0x525257),
    divider: Color::hex(0x202024),
    line_number: Color::hex(0x525259),
    line_number2: Color::hex(0x83838f),
    caret: Color::hex(0x93ddfa),
    find_match: Color::hex(0x5a4d2b),
    line_highlight: Color::hex(0x343438),
    selection: Color::hex(0x48484f),
    scrollbar: Color::hex(0x414146),
    error: Color::hex(0xff3333),
    good: Color::hex(0x72b886),
    nag_bg: Color::hex(0x1c7c9c),
    nag_error: Color::hex(0x9c3434),
    nag_button: Color::hex(0x2f2f35),
    nag_text: Color::hex(0xf2f2f5),
    syntax_normal: Color::hex(0xe1e1e6),
    syntax_comment: Color::hex(0x676b6f),
    syntax_keyword: Color::hex(0xe58ac9),
    syntax_keyword2: Color::hex(0xf77483),
    syntax_number: Color::hex(0xffa94d),
    syntax_literal: Color::hex(0xffa94d),
    syntax_string: Color::hex(0xf7c95c),
    syntax_operator: Color::hex(0x93ddfa),
    syntax_function: Color::hex(0x93ddfa),
    md_text: Color::hex(0xd8d8dc),
    md_heading: Color::hex(0xf2f2f5),
    md_muted: Color::hex(0xa3a3aa),
    md_link: Color::hex(0x93ddfa),
    md_code_bg: Color::hex(0x3b3b42),
    md_block_bg: Color::hex(0x252529),
    md_border: Color::hex(0x48484f),
    md_zebra: Color::hex(0x323237),
    md_note: Color::hex(0x93ddfa),
    md_tip: Color::hex(0x72b886),
    md_important: Color::hex(0xe58ac9),
    md_warning: Color::hex(0xffa94d),
    md_caution: Color::hex(0xf77483),
};

pub const LIGHT: Palette = Palette {
    background: Color::hex(0xfbfbfc),
    background2: Color::hex(0xf0f0f2),
    text: Color::hex(0x5f5f66),
    accent: Color::hex(0x1f1f24),
    dim: Color::hex(0xa4a4ab),
    divider: Color::hex(0xdedee3),
    line_number: Color::hex(0xc2c2c8),
    line_number2: Color::hex(0x707078),
    caret: Color::hex(0x1f6feb),
    find_match: Color::hex(0xfbe7a1),
    line_highlight: Color::hex(0xf0f0f3),
    selection: Color::hex(0xcfe0f7),
    scrollbar: Color::hex(0xd4d4da),
    error: Color::hex(0xcf222e),
    good: Color::hex(0x1a7f37),
    nag_bg: Color::hex(0x1f6f8f),
    nag_error: Color::hex(0xb3262e),
    nag_button: Color::hex(0x18556e),
    nag_text: Color::hex(0xffffff),
    syntax_normal: Color::hex(0x1f2328),
    syntax_comment: Color::hex(0x8b9198),
    syntax_keyword: Color::hex(0xcf222e),
    syntax_keyword2: Color::hex(0x8250df),
    syntax_number: Color::hex(0x0550ae),
    syntax_literal: Color::hex(0x0550ae),
    syntax_string: Color::hex(0x0a3069),
    syntax_operator: Color::hex(0xcf222e),
    syntax_function: Color::hex(0x8250df),
    md_text: Color::hex(0x24292f),
    md_heading: Color::hex(0x111418),
    md_muted: Color::hex(0x636c76),
    md_link: Color::hex(0x0969da),
    md_code_bg: Color::hex(0xeceef1),
    md_block_bg: Color::hex(0xf3f5f7),
    md_border: Color::hex(0xd0d7de),
    md_zebra: Color::hex(0xf6f8fa),
    md_note: Color::hex(0x0969da),
    md_tip: Color::hex(0x1a7f37),
    md_important: Color::hex(0x8250df),
    md_warning: Color::hex(0x9a6700),
    md_caution: Color::hex(0xcf222e),
};

static LIGHT_MODE: AtomicBool = AtomicBool::new(false);

/// The active palette.
pub fn p() -> &'static Palette {
    if LIGHT_MODE.load(Ordering::Relaxed) {
        &LIGHT
    } else {
        &DARK
    }
}

pub fn is_light() -> bool {
    LIGHT_MODE.load(Ordering::Relaxed)
}

/// Switch palettes. Views must drop colour caches afterwards
/// (`View::theme_changed`).
pub fn set_light(light: bool) {
    LIGHT_MODE.store(light, Ordering::Relaxed);
}

pub const MD_FONT_SIZE: f32 = 16.0;
pub const MD_LINE_HEIGHT: f32 = 1.55;
pub const MD_CODE_SIZE: f32 = 14.0;
pub const MD_MAX_WIDTH: f32 = 880.0;
/// Base sizes in logical pixels (lite-xl defaults).
pub const FONT_SIZE: f32 = 15.0;
pub const CODE_FONT_SIZE: f32 = 15.0;
pub const LINE_HEIGHT: f32 = 1.4;
pub const PADDING_X: f32 = 14.0;
pub const PADDING_Y: f32 = 7.0;
pub const SCROLLBAR_SIZE: f32 = 4.0;
pub const TAB_WIDTH: f32 = 170.0;
