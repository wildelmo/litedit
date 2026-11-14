//! Software canvas over a `0x00RRGGBB` pixel buffer (the softbuffer format).
//!
//! Mirrors lite-xl's `renderer.c`: filled rectangles, alpha-blended glyph
//! masks, colour glyphs and a clip rectangle. Everything is bounds-checked
//! against the clip, so no draw call can write outside the buffer.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// Parse `0xRRGGBB`.
    pub const fn hex(v: u32) -> Self {
        Self::rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
    }

    pub const fn with_alpha(self, a: u8) -> Self {
        Self { a, ..self }
    }

    fn packed(self) -> u32 {
        (u32::from(self.r) << 16) | (u32::from(self.g) << 8) | u32::from(self.b)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    pub fn right(&self) -> i32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }

    pub fn intersect(&self, o: &Rect) -> Rect {
        let x = self.x.max(o.x);
        let y = self.y.max(o.y);
        let r = self.right().min(o.right());
        let b = self.bottom().min(o.bottom());
        Rect::new(x, y, (r - x).max(0), (b - y).max(0))
    }

    pub fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }
}

#[inline]
fn blend_channel(src: u32, dst: u32, a: u32) -> u32 {
    // (src*a + dst*(255-a)) / 255, rounded.
    let v = src * a + dst * (255 - a) + 128;
    (v + (v >> 8)) >> 8
}

#[inline]
fn blend(dst: u32, c: Color, alpha: u32) -> u32 {
    if alpha == 0 {
        return dst;
    }
    if alpha >= 255 {
        return c.packed();
    }
    let r = blend_channel(u32::from(c.r), (dst >> 16) & 0xff, alpha);
    let g = blend_channel(u32::from(c.g), (dst >> 8) & 0xff, alpha);
    let b = blend_channel(u32::from(c.b), dst & 0xff, alpha);
    (r << 16) | (g << 8) | b
}

pub struct Canvas<'a> {
    buf: &'a mut [u32],
    width: i32,
    height: i32,
    clip: Rect,
}

impl<'a> Canvas<'a> {
    pub fn new(buf: &'a mut [u32], width: u32, height: u32) -> Self {
        let (w, h) = (width as i32, height as i32);
        assert!(buf.len() >= (width as usize) * (height as usize));
        Self {
            buf,
            width: w,
            height: h,
            clip: Rect::new(0, 0, w, h),
        }
    }

    pub fn width(&self) -> i32 {
        self.width
    }

    pub fn height(&self) -> i32 {
        self.height
    }

    pub fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.width, self.height)
    }

    pub fn clip(&self) -> Rect {
        self.clip
    }

    /// Set the clip rectangle (always intersected with the canvas bounds).
    /// Returns the previous clip so callers can restore it.
    pub fn set_clip(&mut self, r: Rect) -> Rect {
        let prev = self.clip;
        self.clip = r.intersect(&self.bounds());
        prev
    }

    pub fn fill_rect(&mut self, r: Rect, c: Color) {
        let r = r.intersect(&self.clip);
        if r.is_empty() || c.a == 0 {
            return;
        }
        let a = u32::from(c.a);
        for y in r.y..r.bottom() {
            let row = (y * self.width) as usize;
            let px = &mut self.buf[row + r.x as usize..row + r.right() as usize];
            if a == 255 {
                px.fill(c.packed());
            } else {
                for p in px {
                    *p = blend(*p, c, a);
                }
            }
        }
    }

    /// Blend an 8-bit coverage mask (`w*h` bytes) tinted with `c` at (x, y).
    pub fn draw_mask(&mut self, x: i32, y: i32, w: i32, h: i32, mask: &[u8], c: Color) {
        let dst = Rect::new(x, y, w, h).intersect(&self.clip);
        if dst.is_empty() || mask.len() < (w * h) as usize {
            return;
        }
        let ca = u32::from(c.a);
        for py in dst.y..dst.bottom() {
            let mrow = ((py - y) * w) as usize;
            let row = (py * self.width) as usize;
            for px in dst.x..dst.right() {
                let m = u32::from(mask[mrow + (px - x) as usize]);
                let a = m * ca / 255;
                let i = row + px as usize;
                self.buf[i] = blend(self.buf[i], c, a);
            }
        }
    }

    /// Blend straight-alpha RGBA pixels (`w*h*4` bytes) at (x, y).
    pub fn draw_rgba(&mut self, x: i32, y: i32, w: i32, h: i32, rgba: &[u8]) {
        let dst = Rect::new(x, y, w, h).intersect(&self.clip);
        if dst.is_empty() || rgba.len() < (w * h * 4) as usize {
            return;
        }
        for py in dst.y..dst.bottom() {
            let srow = ((py - y) * w) as usize;
            let row = (py * self.width) as usize;
            for px in dst.x..dst.right() {
                let s = (srow + (px - x) as usize) * 4;
                let c = Color::rgb(rgba[s], rgba[s + 1], rgba[s + 2]);
                let i = row + px as usize;
                self.buf[i] = blend(self.buf[i], c, u32::from(rgba[s + 3]));
            }
        }
    }

    /// Anti-aliased line segment of thickness `width` (round caps).
    /// Anti-aliased filled circle.
    pub fn fill_circle(&mut self, cx: f32, cy: f32, r: f32, c: Color) {
        if r <= 0.0 {
            return;
        }
        let x0 = (cx - r - 1.0).floor() as i32;
        let y0 = (cy - r - 1.0).floor() as i32;
        let size = (2.0 * r + 3.0).ceil() as i32;
        let mut mask = vec![0u8; (size * size) as usize];
        for j in 0..size {
            for i in 0..size {
                let dx = (x0 + i) as f32 + 0.5 - cx;
                let dy = (y0 + j) as f32 + 0.5 - cy;
                let cover = (r + 0.5 - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0);
                mask[(j * size + i) as usize] = (cover * 255.0) as u8;
            }
        }
        self.draw_mask(x0, y0, size, size, &mask, c);
    }

    pub fn draw_line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, width: f32, c: Color) {
        let r = width / 2.0;
        let bx = Rect::new(
            (x0.min(x1) - r - 1.0).floor() as i32,
            (y0.min(y1) - r - 1.0).floor() as i32,
            ((x0 - x1).abs() + width + 3.0).ceil() as i32,
            ((y0 - y1).abs() + width + 3.0).ceil() as i32,
        )
        .intersect(&self.clip);
        if bx.is_empty() {
            return;
        }
        let (dx, dy) = (x1 - x0, y1 - y0);
        let len2 = (dx * dx + dy * dy).max(1e-6);
        for py in bx.y..bx.bottom() {
            for px in bx.x..bx.right() {
                let (cx, cy) = (px as f32 + 0.5, py as f32 + 0.5);
                let t = (((cx - x0) * dx + (cy - y0) * dy) / len2).clamp(0.0, 1.0);
                let (qx, qy) = (x0 + t * dx - cx, y0 + t * dy - cy);
                let d = (qx * qx + qy * qy).sqrt();
                let cov = (r + 0.5 - d).clamp(0.0, 1.0);
                if cov > 0.0 {
                    let i = (py * self.width + px) as usize;
                    let a = (cov * f32::from(c.a)) as u32;
                    self.buf[i] = blend(self.buf[i], c, a);
                }
            }
        }
    }

    #[cfg(test)]
    pub fn pixel(&self, x: i32, y: i32) -> u32 {
        self.buf[(y * self.width + x) as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_respects_clip_and_bounds() {
        let mut buf = vec![0u32; 10 * 10];
        let mut c = Canvas::new(&mut buf, 10, 10);
        c.set_clip(Rect::new(2, 2, 4, 4));
        c.fill_rect(Rect::new(-50, -50, 500, 500), Color::hex(0xffffff));
        assert_eq!(c.pixel(1, 1), 0);
        assert_eq!(c.pixel(2, 2), 0xffffff);
        assert_eq!(c.pixel(5, 5), 0xffffff);
        assert_eq!(c.pixel(6, 6), 0);
    }

    #[test]
    fn blend_half() {
        let mut buf = vec![0u32; 1];
        let mut c = Canvas::new(&mut buf, 1, 1);
        c.fill_rect(Rect::new(0, 0, 1, 1), Color::hex(0xff00ff).with_alpha(128));
        let p = c.pixel(0, 0);
        assert!((((p >> 16) & 0xff) as i32 - 128).abs() <= 1);
        assert_eq!((p >> 8) & 0xff, 0);
    }

    #[test]
    fn line_is_clipped_and_drawn() {
        let mut buf = vec![0u32; 20 * 20];
        let mut c = Canvas::new(&mut buf, 20, 20);
        c.draw_line(-100.0, 10.0, 100.0, 10.0, 2.0, Color::hex(0xffffff));
        assert_eq!(c.pixel(5, 10), 0xffffff);
        assert_eq!(c.pixel(5, 2), 0);
    }

    #[test]
    fn mask_offscreen_is_safe() {
        let mut buf = vec![0u32; 4];
        let mut c = Canvas::new(&mut buf, 2, 2);
        let mask = [255u8; 9];
        c.draw_mask(-1, -1, 3, 3, &mask, Color::hex(0x00ff00));
        c.draw_mask(100, 100, 3, 3, &mask, Color::hex(0x00ff00));
        c.draw_mask(0, 0, 3, 3, &mask[..2], Color::hex(0x00ff00)); // short mask ignored
        assert_eq!(c.pixel(0, 0), 0x00ff00);
        assert_eq!(c.pixel(1, 1), 0x00ff00);
    }
}
