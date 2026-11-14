//! Local image loading for the markdown preview.
//!
//! Decoding runs on one background thread so a large photo never freezes the
//! UI. Hostile files are bounded: file size, pixel dimensions and decoder
//! allocations are all limited, and stored images are downscaled.

use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

pub const MAX_FILE_BYTES: u64 = 40 * 1024 * 1024;
pub const MAX_DIMENSION: u32 = 16_384;
pub const MAX_DECODE_ALLOC: u64 = 128 * 1024 * 1024;
/// Largest image decoded (a 24-megapixel photo fits). Checked from the
/// header before decoding, so a tiny file can't ask for gigabytes.
pub const MAX_PIXELS: u64 = 25_000_000;
/// Images are stored at most this large (longest side), to bound memory.
pub const STORE_MAX_SIDE: u32 = 2400;

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
}

type Done = (PathBuf, Result<Decoded, String>);

pub struct ImageStore {
    map: HashMap<PathBuf, ImgState>,
    tx: Option<Sender<PathBuf>>,
    rx: Option<Receiver<Done>>,
    in_flight: usize,
}

impl Default for ImageStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ImageStore {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
            tx: None,
            rx: None,
            in_flight: 0,
        }
    }

    fn worker(&mut self) -> &Sender<PathBuf> {
        if self.tx.is_none() {
            let (req_tx, req_rx) = channel::<PathBuf>();
            let (done_tx, done_rx) = channel::<Done>();
            let spawned = std::thread::Builder::new()
                .name("litedit-images".into())
                .spawn(move || {
                    // Ends when the store (and its sender) is dropped.
                    while let Ok(p) = req_rx.recv() {
                        let r = decode_file(&p);
                        if done_tx.send((p, r)).is_err() {
                            break;
                        }
                    }
                });
            if spawned.is_ok() {
                self.rx = Some(done_rx);
            }
            self.tx = Some(req_tx);
        }
        self.tx.as_ref().expect("worker sender")
    }

    /// Current state of `path`, starting a load if it was never requested.
    pub fn get(&mut self, path: &Path) -> ImgState {
        if let Some(s) = self.map.get(path) {
            return s.clone();
        }
        let sent = self.worker().send(path.to_path_buf()).is_ok() && self.rx.is_some();
        let state = if sent {
            self.in_flight += 1;
            ImgState::Loading
        } else {
            ImgState::Failed("image loader unavailable".into())
        };
        self.map.insert(path.to_path_buf(), state.clone());
        state
    }

    /// Collect finished loads. Returns true if anything changed.
    pub fn poll(&mut self) -> bool {
        let Some(rx) = &self.rx else { return false };
        let mut changed = false;
        while let Ok((p, r)) = rx.try_recv() {
            self.in_flight = self.in_flight.saturating_sub(1);
            let state = match r {
                Ok(d) => ImgState::Ready(Arc::new(d)),
                Err(e) => ImgState::Failed(e),
            };
            self.map.insert(p, state);
            changed = true;
        }
        changed
    }

    pub fn pending(&self) -> bool {
        self.in_flight > 0
    }

    /// Forget everything (after the document reloads, files may have changed).
    pub fn clear(&mut self) {
        self.map.retain(|_, s| matches!(s, ImgState::Loading));
    }
}

pub fn decode_file(path: &Path) -> Result<Decoded, String> {
    if crate::doc::is_reserved_name(path) {
        return Err("not a file".into());
    }
    // A symlink could point at a network share (reading it authenticates
    // to that server); local images are opened only as plain files.
    let link = std::fs::symlink_metadata(path).map_err(|_| "image not found".to_string())?;
    if link.file_type().is_symlink() || crate::links::link_reaches_unsafe(path) {
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
    decode_bytes(&bytes)
}

pub fn decode_bytes(bytes: &[u8]) -> Result<Decoded, String> {
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
    let img = reader
        .decode()
        .map_err(|e| format!("cannot decode image: {e}"))?;
    let natural = (img.width(), img.height());
    let img = if natural.0.max(natural.1) > STORE_MAX_SIDE {
        img.resize(
            STORE_MAX_SIDE,
            STORE_MAX_SIDE,
            image::imageops::FilterType::Triangle,
        )
    } else {
        img
    };
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Ok(Decoded {
        w,
        h,
        rgba: rgba.into_raw(),
        natural,
    })
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
}
