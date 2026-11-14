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
