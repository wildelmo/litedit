//! Fonts and text drawing on top of cosmic-text.
//!
//! On Windows we load only the font files we actually use from
//! `%WINDIR%\Fonts` instead of scanning every installed font, which keeps
//! startup time and memory low. Elsewhere (dev/CI on Linux) we fall back to the
//! system font scan.

#[cfg(windows)]
use std::path::PathBuf;

use std::time::Instant;

use cosmic_text::{Buffer, FontSystem, SwashCache, SwashContent, fontdb};

use crate::canvas::{Canvas, Color};

/// Font files loaded on Windows. Missing ones are skipped.
#[cfg(windows)]
const WINDOWS_FONT_FILES: &[&str] = &[
    // UI / prose
    "segoeui.ttf",
    "segoeuib.ttf",
    "segoeuii.ttf",
    "segoeuiz.ttf",
    "seguisb.ttf",
    // Code
    "CascadiaMono.ttf",
    "consola.ttf",
    "consolab.ttf",
    "consolai.ttf",
    "consolaz.ttf",
    // Fallback for symbols and emoji
    "seguisym.ttf",
    "seguiemj.ttf",
];

const SANS_PREFERENCE: &[&str] = &["Segoe UI", "Inter", "DejaVu Sans", "Noto Sans"];
const MONO_PREFERENCE: &[&str] = &[
    "Cascadia Mono",
    "Consolas",
    "JetBrains Mono",
    "DejaVu Sans Mono",
    "Noto Sans Mono",
];

/// Map a character to what is safe to shape and draw.
///
/// - C0 controls become visible "control pictures" (␀ ␛ …).
/// - Bidi paragraph separators (U+0085, U+2028, U+2029) would split a line
///   into several bidi paragraphs, which cosmic-text does not support (it
///   asserts), so they become U+FFFD.
/// - Bidi embedding/override controls become U+FFFD so text can never be
///   visually reordered ("Trojan Source", CVE-2021-42574).
///
/// `\t` and `\n` pass through; callers handle them.
pub fn display_char(c: char) -> char {
    match c {
        '\t' | '\n' => c,
        '\0'..='\u{1f}' => char::from_u32(0x2400 + c as u32).unwrap_or('\u{fffd}'),
        '\u{7f}' => '\u{2421}',
        '\u{85}' | '\u{2028}' | '\u{2029}' => '\u{fffd}',
        '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' => '\u{fffd}',
        _ => c,
    }
}

/// Apply [`display_char`] to a whole string (borrowed when nothing changes).
pub fn sanitize(s: &str) -> std::borrow::Cow<'_, str> {
    if s.chars().all(|c| display_char(c) == c) {
        std::borrow::Cow::Borrowed(s)
    } else {
        std::borrow::Cow::Owned(s.chars().map(display_char).collect())
    }
}

pub struct Fonts {
    pub system: FontSystem,
    pub cache: SwashCache,
    pub sans: String,
    pub mono: String,
    pub load_ms: u128,
}

impl Fonts {
    pub fn load() -> Self {
        let start = Instant::now();
        let mut db = fontdb::Database::new();

        #[allow(unused_mut)]
        let mut loaded_any = false;
        #[cfg(windows)]
        {
            let dir = std::env::var_os("WINDIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
                .join("Fonts");
            for f in WINDOWS_FONT_FILES {
                let p = dir.join(f);
                if p.is_file() && db.load_font_file(&p).is_ok() {
                    loaded_any = true;
                }
            }
        }
        if !loaded_any || db.len() == 0 {
            db.load_system_fonts();
        }

        let sans = pick_family(&db, SANS_PREFERENCE);
        let mono = pick_family(&db, MONO_PREFERENCE);
        db.set_sans_serif_family(sans.clone());
        db.set_monospace_family(mono.clone());

        let system = FontSystem::new_with_locale_and_db("en-US".to_string(), db);
        Self {
            system,
            cache: SwashCache::new(),
            sans,
            mono,
            load_ms: start.elapsed().as_millis(),
        }
    }

    /// Draw every visible glyph of `buffer` with its top-left at (x, y).
    pub fn draw_buffer(
        &mut self,
        canvas: &mut Canvas,
        buffer: &mut Buffer,
        x: i32,
        y: i32,
        color: Color,
    ) {
        buffer.shape_until_scroll(&mut self.system, false);
        let default = cosmic_text::Color::rgba(color.r, color.g, color.b, color.a);
        let clip = canvas.clip();
        for run in buffer.layout_runs() {
            // Skip lines outside the clip (big markdown paragraphs).
            let top = y as f32 + run.line_top;
            if top > clip.bottom() as f32 {
                break;
            }
            if top + run.line_height < clip.y as f32 {
                continue;
            }
            for glyph in run.glyphs {
                let phys = glyph.physical((x as f32, y as f32 + run.line_y), 1.0);
                let gc = glyph.color_opt.unwrap_or(default);
                let tint = Color {
                    r: gc.r(),
                    g: gc.g(),
                    b: gc.b(),
                    a: gc.a(),
                };
                let Some(img) = self.cache.get_image(&mut self.system, phys.cache_key) else {
                    continue;
                };
                let gx = phys.x + img.placement.left;
                let gy = phys.y - img.placement.top;
                let (w, h) = (img.placement.width as i32, img.placement.height as i32);
                match img.content {
                    SwashContent::Mask => canvas.draw_mask(gx, gy, w, h, &img.data, tint),
                    SwashContent::Color => canvas.draw_rgba(gx, gy, w, h, &img.data),
                    SwashContent::SubpixelMask => {}
                }
            }
            // Underline / strikethrough.
            if !run.decorations.is_empty() {
                let mut r = RectRenderer {
                    canvas: &mut *canvas,
                    dx: x,
                    dy: y,
                };
                cosmic_text::render_decoration(&mut r, &run, default);
            }
        }
    }
}

/// Adapter so cosmic-text can draw decoration rectangles onto our canvas.
struct RectRenderer<'a, 'b> {
    canvas: &'a mut Canvas<'b>,
    dx: i32,
    dy: i32,
}

impl cosmic_text::Renderer for RectRenderer<'_, '_> {
    fn rectangle(&mut self, x: i32, y: i32, w: u32, h: u32, c: cosmic_text::Color) {
        let rect = crate::canvas::Rect::new(x + self.dx, y + self.dy, w as i32, h as i32);
        self.canvas.fill_rect(
            rect,
            Color {
                r: c.r(),
                g: c.g(),
                b: c.b(),
                a: c.a(),
            },
        );
    }

    fn glyph(&mut self, _g: cosmic_text::PhysicalGlyph, _c: cosmic_text::Color) {}
}

fn pick_family(db: &fontdb::Database, prefs: &[&str]) -> String {
    for want in prefs {
        let found = db.faces().any(|f| {
            f.families
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case(want))
        });
        if found {
            return (*want).to_string();
        }
    }
    db.faces()
        .next()
        .and_then(|f| f.families.first().map(|(n, _)| n.clone()))
        .unwrap_or_else(|| "sans-serif".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_char_maps_dangerous_characters() {
        assert_eq!(display_char('\0'), '\u{2400}');
        assert_eq!(display_char('\u{1b}'), '\u{241b}');
        assert_eq!(display_char('\u{2029}'), '\u{fffd}');
        assert_eq!(display_char('\u{202e}'), '\u{fffd}');
        assert_eq!(display_char('\t'), '\t');
        assert_eq!(display_char('é'), 'é');
        assert!(matches!(sanitize("plain"), std::borrow::Cow::Borrowed(_)));
    }

    /// Regression: cosmic-text asserts when one line holds several bidi
    /// paragraphs of different direction (found by the hostile-file test).
    #[test]
    fn mixed_direction_paragraph_separators_do_not_crash() {
        let mut fonts = Fonts::load();
        let nasty = "مرحبا\u{2029}hello\u{85}שלום\u{1c}world";
        let mut b = Buffer::new_empty(cosmic_text::Metrics::new(14.0, 20.0));
        b.set_text(
            &sanitize(nasty),
            &cosmic_text::Attrs::new(),
            cosmic_text::Shaping::Advanced,
            None,
        );
        b.shape_until_scroll(&mut fonts.system, false);
        assert!(b.layout_runs().count() >= 1);
    }
}
