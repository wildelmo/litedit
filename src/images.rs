//! Local image loading for the markdown preview.
//!
//! Decoding runs on one background thread so a large photo never freezes the
//! UI. Hostile files are bounded: file size, pixel dimensions and decoder
//! allocations are all limited, and stored images are downscaled.

use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel, sync_channel};

pub const MAX_FILE_BYTES: u64 = 40 * 1024 * 1024;
pub const MAX_DIMENSION: u32 = 16_384;
pub const MAX_DECODE_ALLOC: u64 = 128 * 1024 * 1024;
/// Largest image decoded (a 24-megapixel photo fits). Checked from the
/// header before decoding, so a tiny file can't ask for gigabytes.
pub const MAX_PIXELS: u64 = 25_000_000;
/// Images are stored at most this large (longest side), to bound memory.
pub const STORE_MAX_SIDE: u32 = 2400;

/// Most distinct pictures one document's preview keeps track of. Pictures
/// past this show a note and are never queued.
pub const MAX_DOC_IMAGES: usize = 1000;
/// Most decoded pixel bytes one document's preview holds at once (the sum of
/// every shown picture's RGBA). The picture that would go past it, and every
/// one after it, shows a note instead.
pub const MAX_DOC_IMAGE_BYTES: usize = 512 * 1024 * 1024;

#[derive(Debug)]
pub struct Decoded {
    pub w: u32,
    pub h: u32,
    /// Straight-alpha RGBA, `w * h * 4` bytes.
    pub rgba: Vec<u8>,
    /// Size of the original image before any downscaling.
    pub natural: (u32, u32),
}

#[derive(Debug, Clone)]
pub enum ImgState {
    Loading,
    Ready(Arc<Decoded>),
    Failed(String),
    /// Pixels were dropped while the tab was hidden; the natural size is
    /// kept so the page layout doesn't change. `reloading`: asked again.
    Unloaded {
        natural: (u32, u32),
        reloading: bool,
    },
}

/// (store key, decode result)
type Done = (PathBuf, Result<Decoded, String>);
/// (store key, path to open)
type Req = (PathBuf, PathBuf);

pub struct ImageStore {
    /// Keyed by [`store_key`], so every spelling of one file is one entry.
    map: HashMap<PathBuf, ImgState>,
    tx: Option<Sender<Req>>,
    rx: Option<Receiver<Done>>,
    in_flight: usize,
    /// Sum of `rgba.len()` over the `Ready` entries.
    held: usize,
    /// The byte budget was reached: new pictures are not loaded.
    over_budget: bool,
    max_images: usize,
    max_bytes: usize,
    /// Decodes asked of the worker (tests).
    #[cfg(test)]
    requested: usize,
}

impl Default for ImageStore {
    fn default() -> Self {
        Self::new()
    }
}

/// One spelling per file: `.` dropped, `..` resolved against the component
/// before it (Windows resolves `..` the same way, without looking at the
/// disk). Returns (the path to open, the key to store it under); on Windows
/// the key ignores case. Never touches the file system.
fn store_key(path: &Path) -> (PathBuf, PathBuf) {
    use std::path::Component;
    let verbatim = matches!(
        path.components().next(),
        Some(Component::Prefix(p)) if p.kind().is_verbatim()
    );
    let open = if verbatim {
        // `\\?\` paths reach the file system exactly as written.
        path.to_path_buf()
    } else {
        let mut out = PathBuf::new();
        let mut names = 0usize;
        for c in path.components() {
            match c {
                Component::CurDir => {}
                Component::ParentDir if names > 0 => {
                    out.pop();
                    names -= 1;
                }
                // `..` at the root stays at the root.
                Component::ParentDir if out.has_root() => {}
                Component::Normal(s) => {
                    out.push(s);
                    names += 1;
                }
                other => out.push(other),
            }
        }
        if out.as_os_str().is_empty() {
            out.push(".");
        }
        out
    };
    let key = if cfg!(windows) {
        match open.to_str() {
            Some(s) => PathBuf::from(s.to_lowercase()),
            None => open.clone(),
        }
    } else {
        open.clone()
    };
    (open, key)
}

impl ImageStore {
    pub fn new() -> Self {
        Self::with_limits(MAX_DOC_IMAGES, MAX_DOC_IMAGE_BYTES)
    }

    /// A store tracking at most `max_images` pictures and holding at most
    /// `max_bytes` of decoded pixels.
    pub fn with_limits(max_images: usize, max_bytes: usize) -> Self {
        Self {
            map: HashMap::new(),
            tx: None,
            rx: None,
            in_flight: 0,
            held: 0,
            over_budget: false,
            max_images,
            max_bytes,
            #[cfg(test)]
            requested: 0,
        }
    }

    fn budget_note(&self) -> String {
        format!(
            "not shown: the pictures on this page already use {} MB",
            self.max_bytes / (1024 * 1024)
        )
    }

    /// Store `state` under `key`, keeping `held` right.
    fn set(&mut self, key: PathBuf, state: ImgState) {
        if let ImgState::Ready(d) = &state {
            self.held += d.rgba.len();
        }
        if let Some(ImgState::Ready(old)) = self.map.insert(key, state) {
            self.held = self.held.saturating_sub(old.rgba.len());
        }
    }

    /// Ask the worker to decode `open`; false if there is no worker.
    fn request(&mut self, key: &Path, open: PathBuf) -> bool {
        if self.tx.is_none() {
            let (req_tx, req_rx) = channel::<Req>();
            // At most one finished picture waits to be collected, so the
            // worker never runs far ahead of `poll`.
            let (done_tx, done_rx) = sync_channel::<Done>(1);
            let spawned = std::thread::Builder::new()
                .name("litedit-images".into())
                .spawn(move || {
                    // Ends when the store (and its sender) is dropped, or
                    // when the store stops listening.
                    while let Ok((key, p)) = req_rx.recv() {
                        let r = decode_file(&p);
                        if done_tx.send((key, r)).is_err() {
                            break;
                        }
                    }
                });
            if spawned.is_ok() {
                self.rx = Some(done_rx);
            }
            self.tx = Some(req_tx);
        }
        let sent = self.rx.is_some()
            && self
                .tx
                .as_ref()
                .is_some_and(|tx| tx.send((key.to_path_buf(), open)).is_ok());
        if sent {
            self.in_flight += 1;
            #[cfg(test)]
            {
                self.requested += 1;
            }
        }
        sent
    }

    /// Current state of `path`, starting a load if it was never requested.
    pub fn get(&mut self, path: &Path) -> ImgState {
        let (open, key) = store_key(path);
        match self.map.get(&key) {
            Some(ImgState::Unloaded {
                natural,
                reloading: false,
            }) => {
                // Shown again: decode in the background, keep the size meanwhile.
                let natural = *natural;
                let sent = self.request(&key, open);
                let s = ImgState::Unloaded {
                    natural,
                    reloading: sent,
                };
                self.set(key, s.clone());
                return s;
            }
            Some(s) => return s.clone(),
            None => {}
        }
        if self.map.len() >= self.max_images {
            // Not stored, so the map (and the queue) stay bounded.
            return ImgState::Failed("too many pictures in this document".into());
        }
        let state = if self.over_budget {
            ImgState::Failed(self.budget_note())
        } else if self.request(&key, open) {
            ImgState::Loading
        } else {
            ImgState::Failed("image loader unavailable".into())
        };
        self.set(key, state.clone());
        state
    }

    /// Collect finished loads. Returns true if anything changed.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        // One at a time: a result that goes over the budget stops the rest.
        while let Some(Ok((key, r))) = self.rx.as_ref().map(Receiver::try_recv) {
            self.in_flight = self.in_flight.saturating_sub(1);
            changed = true;
            match r {
                Ok(d) if self.held.saturating_add(d.rgba.len()) > self.max_bytes => {
                    drop(d);
                    self.stop_for_budget(key);
                }
                Ok(d) => self.set(key, ImgState::Ready(Arc::new(d))),
                Err(e) => self.set(key, ImgState::Failed(e)),
            }
        }
        changed
    }

    /// The picture under `key` doesn't fit: it and every picture still
    /// waiting show a note, and the worker stops (it finishes at most the
    /// file it is decoding, then finds nobody listening).
    fn stop_for_budget(&mut self, key: PathBuf) {
        self.over_budget = true;
        self.tx = None;
        self.rx = None;
        self.in_flight = 0;
        let note = self.budget_note();
        self.set(key, ImgState::Failed(note.clone()));
        for s in self.map.values_mut() {
            if matches!(s, ImgState::Loading | ImgState::Unloaded { .. }) {
                *s = ImgState::Failed(note.clone());
            }
        }
    }

    pub fn pending(&self) -> bool {
        self.in_flight > 0
    }

    /// Drop decoded pixels (the tab is hidden), remembering each image's
    /// size. Returns how many images were unloaded.
    pub fn unload(&mut self) -> usize {
        // Take what has finished, then stop the worker: results of loads
        // still running must not pile up unseen while the tab is hidden.
        self.poll();
        self.tx = None;
        self.rx = None;
        self.in_flight = 0;
        self.map.retain(|_, s| !matches!(s, ImgState::Loading));
        let mut n = 0;
        for s in self.map.values_mut() {
            match s {
                ImgState::Ready(d) => {
                    *s = ImgState::Unloaded {
                        natural: d.natural,
                        reloading: false,
                    };
                    n += 1;
                }
                ImgState::Unloaded { reloading, .. } => *reloading = false,
                _ => {}
            }
        }
        // Pictures that didn't fit keep their note; the ones that did are
        // decoded again when the tab is shown, and fit again.
        self.held = 0;
        n
    }

    /// Decoded images currently held in memory (tests).
    #[cfg(test)]
    pub fn loaded(&self) -> usize {
        self.map
            .values()
            .filter(|s| matches!(s, ImgState::Ready(_)))
            .count()
    }

    /// Pictures tracked, and decodes asked for so far (tests).
    #[cfg(test)]
    pub fn counts(&self) -> (usize, usize) {
        (self.map.len(), self.requested)
    }

    /// Decoded pixel bytes held (tests).
    #[cfg(test)]
    pub fn held(&self) -> usize {
        self.held
    }

    /// Forget everything (after the document reloads, files may have changed).
    pub fn clear(&mut self) {
        self.map.retain(|_, s| matches!(s, ImgState::Loading));
        self.held = 0;
        self.over_budget = false;
    }
}

pub fn decode_file(path: &Path) -> Result<Decoded, String> {
    decode_file_max(path, STORE_MAX_SIDE)
}

/// [`decode_file`], keeping at most `max_side` pixels on the longest side
/// (`u32::MAX`: every pixel, for the image viewer at 100%).
pub fn decode_file_max(path: &Path, max_side: u32) -> Result<Decoded, String> {
    if crate::doc::is_reserved_name(path) {
        return Err("not a file".into());
    }
    // A symlink could point at a network share (reading it authenticates
    // to that server); local images are opened only as plain files. The
    // folders on the way are checked first: looking at the file itself
    // already goes through a linked folder.
    if crate::links::link_reaches_unsafe(path) {
        return Err("linked images are not opened".into());
    }
    let link = std::fs::symlink_metadata(path).map_err(|_| "image not found".to_string())?;
    if link.file_type().is_symlink() {
        return Err("linked images are not opened".into());
    }
    let meta = std::fs::metadata(path).map_err(|_| "image not found".to_string())?;
    if !meta.is_file() {
        return Err("not a file".into());
    }
    if meta.len() > MAX_FILE_BYTES {
        return Err(format!(
            "image too large ({} MB)",
            meta.len() / (1024 * 1024)
        ));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read image: {e}"))?;
    let img = decode_image(&bytes)?;
    // The file's bytes aren't needed while the pixels are reduced.
    drop(bytes);
    Ok(store(img, max_side))
}

#[cfg(test)]
pub fn decode_bytes(bytes: &[u8]) -> Result<Decoded, String> {
    Ok(store(decode_image(bytes)?, STORE_MAX_SIDE))
}

fn decode_image(bytes: &[u8]) -> Result<image::DynamicImage, String> {
    let (w, h) = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| format!("unreadable image: {e}"))?
        .into_dimensions()
        .map_err(|e| format!("cannot decode image: {e}"))?;
    if w as u64 * h as u64 > MAX_PIXELS {
        return Err(format!("image too large ({w} × {h})"));
    }
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| format!("unreadable image: {e}"))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    reader.limits(limits);
    reader
        .decode()
        .map_err(|e| format!("cannot decode image: {e}"))
}

/// Turn a decoded image into stored RGBA, at most `max_side` on its longest
/// side. Peak memory is the decoder's buffer plus the stored copy: nothing
/// is converted or resampled at full size.
fn store(img: image::DynamicImage, max_side: u32) -> Decoded {
    let natural = (img.width(), img.height());
    let longest = natural.0.max(natural.1);
    if longest <= max_side.max(1) {
        use image::DynamicImage as I;
        // Kept whole: the decoder's own buffer becomes the stored pixels.
        let rgba = match img {
            I::ImageRgba8(b) => b.into_raw(),
            I::ImageRgb8(b) => expand_to_rgba(b.into_raw(), 3),
            I::ImageLuma8(b) => expand_to_rgba(b.into_raw(), 1),
            I::ImageLumaA8(b) => expand_to_rgba(b.into_raw(), 2),
            other => other.into_rgba8().into_raw(),
        };
        return Decoded {
            w: natural.0,
            h: natural.1,
            rgba,
            natural,
        };
    }
    let k = f64::from(max_side.max(1)) / f64::from(longest);
    let w = ((f64::from(natural.0) * k).round() as u32).clamp(1, max_side.max(1));
    let h = ((f64::from(natural.1) * k).round() as u32).clamp(1, max_side.max(1));
    let channels = usize::from(img.color().channel_count());
    let rgba = if usize::from(img.color().bytes_per_pixel()) == channels {
        // 8 bits per channel (nearly every file): straight from the
        // decoder's buffer.
        downscale(img.as_bytes(), natural.0, natural.1, channels, w, h)
    } else {
        let full = img.into_rgba8();
        downscale(full.as_raw(), natural.0, natural.1, 4, w, h)
    };
    Decoded {
        w,
        h,
        rgba,
        natural,
    }
}

/// Grey, grey + alpha or RGB pixels made RGBA in the same buffer: it grows
/// once and is filled from the end backwards, so the picture is never held
/// twice (a 12-megapixel photo would be 36 MB + 48 MB).
fn expand_to_rgba(mut px: Vec<u8>, channels: usize) -> Vec<u8> {
    if !(1..=3).contains(&channels) {
        return px;
    }
    let n = px.len() / channels;
    px.truncate(n * channels);
    px.reserve_exact(n * (4 - channels));
    px.resize(n * 4, 0);
    for i in (0..n).rev() {
        let s = i * channels;
        let rgba = match channels {
            1 => [px[s], px[s], px[s], 255],
            2 => [px[s], px[s], px[s], px[s + 1]],
            _ => [px[s], px[s + 1], px[s + 2], 255],
        };
        // Pixel i ends before pixel i + 1 started, in both layouts, so
        // nothing still to be read is overwritten.
        px[i * 4..i * 4 + 4].copy_from_slice(&rgba);
    }
    px
}

/// The source pixels `start..end` one output pixel covers, with how much of
/// the first and last it covers (the ones between count fully).
struct Span {
    start: usize,
    end: usize,
    first: f32,
    last: f32,
    total: f32,
}

impl Span {
    fn weight(&self, i: usize) -> f32 {
        if i == self.start {
            self.first
        } else if i + 1 == self.end {
            self.last
        } else {
            1.0
        }
    }
}

fn spans(src: usize, dst: usize) -> Vec<Span> {
    let r = src as f64 / dst as f64;
    (0..dst)
        .map(|i| {
            let a = i as f64 * r;
            let b = ((i + 1) as f64 * r).min(src as f64);
            let start = (a.floor() as usize).min(src - 1);
            let end = (b.ceil() as usize).clamp(start + 1, src);
            let (first, last) = if end - start == 1 {
                (b - a, 0.0)
            } else {
                ((start + 1) as f64 - a, b - (end - 1) as f64)
            };
            Span {
                start,
                end,
                first: first as f32,
                last: last as f32,
                total: (b - a) as f32,
            }
        })
        .collect()
}

/// Resample `src` (`sw x sh` pixels of `channels` bytes: grey, grey + alpha,
/// RGB or RGBA) to `dw x dh` straight-alpha RGBA by averaging the area each
/// output pixel covers. Working memory is one row of the output (the `image`
/// crate's filters build a full-height float copy first), and colours are
/// weighted by alpha so transparent pixels don't darken edges.
pub fn downscale(src: &[u8], sw: u32, sh: u32, channels: usize, dw: u32, dh: u32) -> Vec<u8> {
    let (sw, sh) = (sw as usize, sh as usize);
    let (dw, dh) = (dw.max(1) as usize, dh.max(1) as usize);
    let mut out = vec![0u8; dw * dh * 4];
    let enough = sw
        .checked_mul(sh)
        .and_then(|n| n.checked_mul(channels))
        .is_some_and(|n| src.len() >= n);
    if sw == 0 || sh == 0 || !(1..=4).contains(&channels) || !enough {
        return out;
    }
    let xs = spans(sw, dw);
    let ys = spans(sh, dh);
    // Per output pixel of the current row: sums of r*a, g*a, b*a and a.
    let mut acc = vec![[0f32; 4]; dw];
    for (y, out_row) in ys.iter().zip(out.chunks_exact_mut(dw * 4)) {
        acc.fill([0.0; 4]);
        for sy in y.start..y.end {
            let wy = y.weight(sy);
            let row = &src[sy * sw * channels..(sy + 1) * sw * channels];
            for (x, acc) in xs.iter().zip(acc.iter_mut()) {
                let mut sum = [0f32; 4];
                for sx in x.start..x.end {
                    let s = &row[sx * channels..(sx + 1) * channels];
                    let (r, g, b, a) = match channels {
                        1 => (s[0], s[0], s[0], 255),
                        2 => (s[0], s[0], s[0], s[1]),
                        3 => (s[0], s[1], s[2], 255),
                        _ => (s[0], s[1], s[2], s[3]),
                    };
                    let aw = x.weight(sx) * f32::from(a);
                    sum[0] += f32::from(r) * aw;
                    sum[1] += f32::from(g) * aw;
                    sum[2] += f32::from(b) * aw;
                    sum[3] += aw;
                }
                for (a, s) in acc.iter_mut().zip(sum) {
                    *a += s * wy;
                }
            }
        }
        for ((x, acc), px) in xs.iter().zip(&acc).zip(out_row.chunks_exact_mut(4)) {
            let [r, g, b, alpha] = *acc;
            if alpha <= 0.0 {
                continue;
            }
            let area = (x.total * y.total).max(f32::MIN_POSITIVE);
            let byte = |v: f32| v.round().clamp(0.0, 255.0) as u8;
            px.copy_from_slice(&[
                byte(r / alpha),
                byte(g / alpha),
                byte(b / alpha),
                byte(alpha / area),
            ]);
        }
    }
    out
}

/// Scale decoded pixels to exactly `w x h` for display.
pub fn scale_to(d: &Decoded, w: u32, h: u32) -> Vec<u8> {
    if (w, h) == (d.w, d.h) {
        return d.rgba.clone();
    }
    match image::RgbaImage::from_raw(d.w, d.h, d.rgba.clone()) {
        Some(img) => image::imageops::resize(
            &img,
            w.max(1),
            h.max(1),
            image::imageops::FilterType::Triangle,
        )
        .into_raw(),
        None => vec![0; (w * h * 4) as usize],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn an_image_under_a_folder_linked_to_a_network_share_is_refused() {
        let d = std::env::temp_dir().join(format!("litedit-img-netdir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::os::unix::fs::symlink("//evil/share", d.join("pics")).unwrap();
        let e = decode_file(&d.join("pics").join("a.png")).unwrap_err();
        assert!(e.contains("linked images"), "refused as a link, got: {e}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn huge_dimensions_are_refused_from_the_header_alone() {
        // A PNG header claiming 6000 x 6000 with no pixel data.
        let mut bytes = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut bytes, 6000, 6000);
            enc.set_color(png::ColorType::Rgba);
            let mut w = enc.write_header().unwrap();
            // A tiny (incomplete) data chunk: enough for the header to be read.
            w.write_chunk(
                png::chunk::IDAT,
                &[0x78, 0x9c, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01],
            )
            .unwrap();
            std::mem::forget(w); // don't let the writer finish the stream
        }
        assert!(bytes.len() < 200);
        let e = decode_bytes(&bytes).unwrap_err();
        assert!(e.contains("too large"), "{e}");
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_images_are_refused() {
        let d = std::env::temp_dir().join(format!("litedit-imglink-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let real = crate::tests::fixture("img/banner.png");
        std::os::unix::fs::symlink(&real, d.join("link.png")).unwrap();
        assert_eq!(
            decode_file(&d.join("link.png")).err().as_deref(),
            Some("linked images are not opened")
        );
        assert!(decode_file(&real).is_ok(), "the file itself still opens");
        let _ = std::fs::remove_dir_all(&d);
    }

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([255, 0, 0, 255]));
        let mut out = Vec::new();
        img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn decodes_and_downscales() {
        let d = decode_bytes(&png(10, 5)).unwrap();
        assert_eq!((d.w, d.h), (10, 5));
        assert_eq!(d.rgba.len(), 10 * 5 * 4);
        let big = decode_bytes(&png(STORE_MAX_SIDE * 2, 10)).unwrap();
        assert_eq!(big.w, STORE_MAX_SIDE);
        assert_eq!(big.natural, (STORE_MAX_SIDE * 2, 10));
    }

    #[test]
    fn garbage_and_truncated_files_fail_cleanly() {
        assert!(decode_bytes(b"not an image").is_err());
        let p = png(50, 50);
        for cut in [8, 20, 40, p.len() / 2] {
            assert!(decode_bytes(&p[..cut]).is_err());
        }
    }

    #[test]
    fn huge_declared_dimensions_are_rejected() {
        // A PNG header claiming 100000x100000 pixels.
        let mut p = png(1, 1);
        p[16..20].copy_from_slice(&100_000u32.to_be_bytes());
        p[20..24].copy_from_slice(&100_000u32.to_be_bytes());
        assert!(decode_bytes(&p).is_err());
    }

    #[test]
    fn downscale_averages_areas_and_weights_colour_by_alpha() {
        // A solid colour stays exactly that colour at any ratio.
        let solid: Vec<u8> = [10u8, 200, 30, 255].repeat(7 * 5);
        for (w, h) in [(7, 5), (3, 2), (1, 1), (6, 4)] {
            let d = downscale(&solid, 7, 5, 4, w, h);
            assert_eq!(d.len(), (w * h * 4) as usize);
            assert!(d.chunks(4).all(|p| p == [10, 200, 30, 255]), "{w}x{h}");
        }
        // 2x2 -> 1: the plain average.
        let four = [
            0u8, 0, 0, 255, 100, 0, 0, 255, 0, 100, 0, 255, 0, 0, 200, 255,
        ];
        assert_eq!(downscale(&four, 2, 2, 4, 1, 1), [25, 25, 50, 255]);
        // Opaque red next to a fully transparent pixel: half as opaque,
        // still pure red (not darkened by the invisible pixel's colour).
        let edge = [255u8, 0, 0, 255, 0, 255, 0, 0];
        assert_eq!(downscale(&edge, 2, 1, 4, 1, 1), [255, 0, 0, 128]);
        // 3 -> 2: each output covers one and a half source pixels.
        let grey = [0u8, 90, 255];
        assert_eq!(
            downscale(&grey, 3, 1, 1, 2, 1),
            [30, 30, 30, 255, 200, 200, 200, 255]
        );
        // RGB and grey + alpha sources.
        assert_eq!(downscale(&[1, 2, 3], 1, 1, 3, 1, 1), [1, 2, 3, 255]);
        assert_eq!(downscale(&[9, 40], 1, 1, 2, 1, 1), [9, 9, 9, 40]);
        // Nonsense input gives transparent pixels of the asked size, no panic.
        assert_eq!(downscale(&[1, 2, 3], 5, 5, 4, 2, 2), [0; 16]);
        assert_eq!(downscale(&[], 0, 0, 4, 1, 1), [0; 4]);
        assert_eq!(downscale(&[1; 64], 4, 4, 9, 1, 1), [0; 4]);
        // Asked to enlarge: pixels repeat.
        assert_eq!(
            downscale(&[5, 50], 2, 1, 1, 4, 1),
            [5, 5, 5, 255, 5, 5, 5, 255, 50, 50, 50, 255, 50, 50, 50, 255]
        );
    }

    #[test]
    fn pixels_become_rgba_in_place() {
        assert_eq!(
            expand_to_rgba(vec![1, 2, 3, 4, 5, 6], 3),
            [1, 2, 3, 255, 4, 5, 6, 255]
        );
        assert_eq!(
            expand_to_rgba(vec![7, 8, 9], 1),
            [7, 7, 7, 255, 8, 8, 8, 255, 9, 9, 9, 255]
        );
        assert_eq!(
            expand_to_rgba(vec![7, 100, 9, 0], 2),
            [7, 7, 7, 100, 9, 9, 9, 0]
        );
        assert_eq!(expand_to_rgba(vec![1, 2, 3, 4], 4), [1, 2, 3, 4]);
        assert_eq!(expand_to_rgba(Vec::new(), 3), [0u8; 0]);
        // Whole decoded images of each kind match the image crate's own
        // conversion.
        use image::DynamicImage as I;
        let rgb = image::RgbImage::from_fn(13, 7, |x, y| image::Rgb([x as u8, y as u8, 99]));
        let la = image::GrayAlphaImage::from_fn(13, 7, |x, y| image::LumaA([x as u8, y as u8]));
        let l = image::GrayImage::from_fn(13, 7, |x, y| image::Luma([(x * y) as u8]));
        for img in [I::ImageRgb8(rgb), I::ImageLumaA8(la), I::ImageLuma8(l)] {
            let want = img.to_rgba8().into_raw();
            let d = store(img, STORE_MAX_SIDE);
            assert_eq!((d.w, d.h, d.natural), (13, 7, (13, 7)));
            assert_eq!(d.rgba, want);
        }
    }

    #[test]
    fn every_pixel_format_is_stored_as_rgba_when_reduced() {
        use image::{DynamicImage, GrayAlphaImage, GrayImage, RgbImage};
        let side = STORE_MAX_SIDE + 100;
        let imgs = [
            DynamicImage::ImageLuma8(GrayImage::from_pixel(side, 2, image::Luma([77]))),
            DynamicImage::ImageLumaA8(GrayAlphaImage::from_pixel(side, 2, image::LumaA([77, 255]))),
            DynamicImage::ImageRgb8(RgbImage::from_pixel(side, 2, image::Rgb([77, 77, 77]))),
            DynamicImage::ImageRgb16(image::ImageBuffer::from_pixel(
                side,
                2,
                image::Rgb([77 * 257, 77 * 257, 77 * 257]),
            )),
        ];
        for img in imgs {
            let d = store(img, STORE_MAX_SIDE);
            assert_eq!((d.w, d.h), (STORE_MAX_SIDE, 2));
            assert_eq!(d.natural, (side, 2));
            assert!(d.rgba.chunks(4).all(|p| p == [77, 77, 77, 255]));
        }
        // Not reduced when asked for every pixel.
        let d = store(
            DynamicImage::ImageRgb8(RgbImage::from_pixel(side, 2, image::Rgb([1, 2, 3]))),
            u32::MAX,
        );
        assert_eq!((d.w, d.h), (side, 2));
    }

    #[test]
    fn store_loads_in_background() {
        let dir = std::env::temp_dir().join(format!("litedit-img-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.png");
        std::fs::write(&path, png(4, 4)).unwrap();
        let mut store = ImageStore::new();
        assert!(matches!(store.get(&path), ImgState::Loading));
        let start = std::time::Instant::now();
        while store.pending() && start.elapsed().as_secs() < 5 {
            store.poll();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(matches!(store.get(&path), ImgState::Ready(_)));
        assert!(matches!(
            store.get(&dir.join("missing.png")),
            ImgState::Loading
        ));
        std::fs::remove_dir_all(&dir).ok();
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("litedit-imgdoc-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Collect results until nothing is left to decode.
    fn settle(store: &mut ImageStore) {
        let start = std::time::Instant::now();
        while store.pending() {
            assert!(start.elapsed().as_secs() < 10, "decoding never finished");
            store.poll();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    #[test]
    fn paths_are_spelled_one_way_without_touching_the_disk() {
        let k = |s: &str| store_key(Path::new(s)).0;
        assert_eq!(k("/x/./y/../a.png"), Path::new("/x/a.png"));
        assert_eq!(k("/x/y/z/../../a.png"), Path::new("/x/a.png"));
        assert_eq!(k("/../../a.png"), Path::new("/a.png"));
        assert_eq!(k("./a.png"), Path::new("a.png"));
        assert_eq!(k("../../a.png"), Path::new("../../a.png"));
        assert_eq!(k("x/../../a.png"), Path::new("../a.png"));
        assert_eq!(k("x/.."), Path::new("."));
        assert_eq!(k("/x//y/a.png"), Path::new("/x/y/a.png"));
        if cfg!(windows) {
            assert_eq!(k(r"C:\x\..\A.png"), Path::new(r"C:\A.png"));
            assert_eq!(k(r"C:\..\A.png"), Path::new(r"C:\A.png"));
            // The key ignores case, the path opened keeps it.
            let (open, key) = store_key(Path::new(r"C:\Pics\A.PNG"));
            assert_eq!(open, Path::new(r"C:\Pics\A.PNG"));
            assert_eq!(key, store_key(Path::new(r"c:\pics\.\a.png")).1);
        } else {
            let (open, key) = store_key(Path::new("/p/A.PNG"));
            assert_eq!(open, key, "case matters off Windows");
        }
    }

    #[test]
    fn every_spelling_of_one_picture_is_one_entry_and_one_decode() {
        let d = tmp("spell");
        std::fs::create_dir_all(d.join("sub")).unwrap();
        std::fs::write(d.join("a.png"), png(4, 4)).unwrap();
        let mut spellings = vec![
            d.join("a.png"),
            d.join("./a.png"),
            d.join("sub/../a.png"),
            d.join("sub/./../sub/../a.png"),
            d.join("nothere/../a.png"),
        ];
        if cfg!(windows) {
            spellings.push(d.join("A.PNG"));
            spellings.push(d.join("SUB\\..\\a.Png"));
        }
        let mut store = ImageStore::new();
        for p in &spellings {
            store.get(p);
        }
        assert_eq!(store.counts(), (1, 1));
        settle(&mut store);
        for p in &spellings {
            assert!(matches!(store.get(p), ImgState::Ready(_)), "{p:?}");
        }
        assert_eq!(store.counts(), (1, 1));
        assert_eq!(store.held(), 4 * 4 * 4);
        std::fs::remove_dir_all(&d).ok();
    }

    #[cfg(not(windows))]
    #[test]
    fn names_differing_only_in_case_stay_apart_off_windows() {
        let d = tmp("case");
        std::fs::write(d.join("a.png"), png(4, 4)).unwrap();
        std::fs::write(d.join("A.png"), png(2, 2)).unwrap();
        if std::fs::read_dir(&d).unwrap().count() != 2 {
            return; // a case-insensitive file system: nothing to show here
        }
        let mut store = ImageStore::new();
        store.get(&d.join("a.png"));
        store.get(&d.join("A.png"));
        assert_eq!(store.counts(), (2, 2));
        settle(&mut store);
        let size = |p: &Path, s: &mut ImageStore| match s.get(p) {
            ImgState::Ready(x) => (x.w, x.h),
            other => panic!("{other:?}"),
        };
        assert_eq!(size(&d.join("a.png"), &mut store), (4, 4));
        assert_eq!(size(&d.join("A.png"), &mut store), (2, 2));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn pictures_past_the_count_limit_show_a_note_and_are_never_queued() {
        let d = tmp("count");
        for i in 0..4 {
            std::fs::write(d.join(format!("{i}.png")), png(2, 2)).unwrap();
        }
        let mut store = ImageStore::with_limits(2, MAX_DOC_IMAGE_BYTES);
        let p = |i: usize| d.join(format!("{i}.png"));
        assert!(matches!(store.get(&p(0)), ImgState::Loading));
        assert!(matches!(store.get(&p(1)), ImgState::Loading));
        for _ in 0..3 {
            match store.get(&p(2)) {
                ImgState::Failed(e) => assert_eq!(e, "too many pictures in this document"),
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(store.counts(), (2, 2), "nothing stored or queued");
        settle(&mut store);
        assert_eq!(store.loaded(), 2);
        assert!(matches!(store.get(&p(3)), ImgState::Failed(_)));
        // Another spelling of a picture already in is still that picture.
        assert!(matches!(
            store.get(&d.join("x/../1.png")),
            ImgState::Ready(_)
        ));
        assert_eq!(store.counts(), (2, 2));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn pictures_past_the_byte_budget_show_a_note_and_stop_decoding() {
        let d = tmp("bytes");
        for i in 0..6 {
            std::fs::write(d.join(format!("{i}.png")), png(4, 4)).unwrap();
        }
        let p = |i: usize| d.join(format!("{i}.png"));
        // Room for two 64-byte pictures, not three.
        let mut store = ImageStore::with_limits(MAX_DOC_IMAGES, 150);
        for i in 0..5 {
            assert!(matches!(store.get(&p(i)), ImgState::Loading));
        }
        settle(&mut store);
        assert!(!store.pending());
        let check = |store: &mut ImageStore| {
            for i in 0..5 {
                match store.get(&p(i)) {
                    ImgState::Ready(_) => assert!(i < 2, "{i} should not fit"),
                    ImgState::Failed(e) => {
                        assert!(i >= 2, "{i} should fit");
                        assert_eq!(e, "not shown: the pictures on this page already use 0 MB");
                    }
                    other => panic!("{i}: {other:?}"),
                }
            }
            assert_eq!(store.held(), 2 * 64);
            assert!(!store.pending());
        };
        check(&mut store);
        // A picture asked for afterwards is not decoded.
        let before = store.counts().1;
        assert!(matches!(store.get(&p(5)), ImgState::Failed(_)));
        assert_eq!(store.counts().1, before);
        // Hidden and shown again: the same two come back, nothing else.
        assert_eq!(store.unload(), 2);
        assert_eq!((store.loaded(), store.held()), (0, 0));
        for i in 0..5 {
            store.get(&p(i));
        }
        assert_eq!(store.counts().1, before + 2, "only the two that fit");
        settle(&mut store);
        check(&mut store);
        // A reloaded document starts over.
        store.clear();
        assert_eq!(store.held(), 0);
        assert!(matches!(store.get(&p(3)), ImgState::Loading));
        settle(&mut store);
        assert!(matches!(store.get(&p(3)), ImgState::Ready(_)));
        assert_eq!(store.held(), 64);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_picture_larger_than_the_whole_budget_never_stays_loading() {
        let d = tmp("onebig");
        std::fs::write(d.join("a.png"), png(8, 8)).unwrap();
        let mut store = ImageStore::with_limits(MAX_DOC_IMAGES, 100);
        store.get(&d.join("a.png"));
        settle(&mut store);
        assert!(matches!(store.get(&d.join("a.png")), ImgState::Failed(_)));
        assert_eq!(store.held(), 0);
        std::fs::remove_dir_all(&d).ok();
    }
}
