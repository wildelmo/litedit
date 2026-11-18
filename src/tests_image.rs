//! Image viewer tabs (S7.2), driven through the app like a user would.

use std::path::{Path, PathBuf};
use std::time::Duration;

use image::{ImageFormat, Rgba, RgbaImage};

use crate::app::App;
use crate::canvas::Canvas;
use crate::input::{Input, Mods, MouseButton, Request, WheelUnit};
use crate::tests::{click_tree, fixture, key, new_app, settled_hash, tmp_dir};
use crate::theme;

const W: u32 = 800;
const H: u32 = 600;

const MAGENTA: [u8; 4] = [255, 0, 255, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];

fn is_magenta(p: u32) -> bool {
    let (r, g, b) = ((p >> 16) & 255, (p >> 8) & 255, p & 255);
    r > 200 && g < 70 && b > 200
}

fn is_green(p: u32) -> bool {
    let (r, g, b) = ((p >> 16) & 255, (p >> 8) & 255, p & 255);
    r < 70 && g > 200 && b < 70
}

fn is_blue(p: u32) -> bool {
    let (r, g, b) = ((p >> 16) & 255, (p >> 8) & 255, p & 255);
    r < 70 && g < 70 && b > 200
}

/// The error colour of the dark theme, as drawn by solid parts of letters.
fn is_error_red(p: u32) -> bool {
    let (r, g, b) = ((p >> 16) & 255, (p >> 8) & 255, p & 255);
    r > 200 && g < 90 && b < 90
}

/// Write a picture in the format its extension names.
fn save(img: &RgbaImage, path: &Path) {
    let ext = path.extension().unwrap().to_str().unwrap().to_lowercase();
    let format = ImageFormat::from_extension(&ext).unwrap();
    let img = image::DynamicImage::ImageRgba8(img.clone());
    if format == ImageFormat::Jpeg {
        img.to_rgb8().save_with_format(path, format).unwrap();
    } else {
        img.save_with_format(path, format).unwrap();
    }
}

fn solid(w: u32, h: u32, c: [u8; 4]) -> RgbaImage {
    RgbaImage::from_pixel(w, h, Rgba(c))
}

/// Left half green, right half blue.
fn halves(w: u32, h: u32) -> RgbaImage {
    RgbaImage::from_fn(w, h, |x, _| Rgba(if x < w / 2 { GREEN } else { BLUE }))
}

fn frame(app: &mut App) -> Vec<u32> {
    let mut buf = vec![0u32; (W * H) as usize];
    {
        let mut c = Canvas::new(&mut buf, W, H);
        app.render(&mut c);
    }
    buf
}

/// What the window shows once background work is done.
fn shown(app: &mut App) -> Vec<u32> {
    app.settle(Duration::from_secs(10));
    frame(app); // drawing can start a decode
    app.settle(Duration::from_secs(10));
    frame(app)
}

fn count(f: &[u32], pred: fn(u32) -> bool) -> usize {
    f.iter().filter(|p| pred(**p)).count()
}

/// Bounding box (x0, y0, x1, y1), inclusive, of the pixels matching `pred`.
fn bbox(f: &[u32], pred: impl Fn(u32) -> bool) -> Option<(u32, u32, u32, u32)> {
    let mut b: Option<(u32, u32, u32, u32)> = None;
    for (i, p) in f.iter().enumerate() {
        if pred(*p) {
            let (x, y) = (i as u32 % W, i as u32 / W);
            b = Some(match b {
                None => (x, y, x, y),
                Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
            });
        }
    }
    b
}

fn titles(app: &App) -> Vec<String> {
    app.tab_titles().into_iter().map(|t| t.0).collect()
}

fn wheel(app: &mut App, dy: f32) {
    app.handle(Input::MouseMove { x: 400.0, y: 300.0 });
    app.handle(Input::Wheel {
        dx: 0.0,
        dy,
        unit: WheelUnit::Lines,
    });
}

#[test]
fn every_supported_type_opens_as_a_centred_picture() {
    let d = tmp_dir("img-types");
    let mut app = new_app();
    app.open(&d);
    key(&mut app, "ctrl+\\"); // hide the tree: the view spans the window
    for name in [
        "a.png", "b.jpg", "c.jpeg", "d.gif", "e.webp", "f.bmp", "G.PNG", "h.JpG",
    ] {
        let p = d.join(name);
        save(&solid(40, 30, MAGENTA), &p);
        app.open(&p);
        let f = shown(&mut app);
        assert_eq!(count(&f, is_magenta), 40 * 30, "{name}: drawn 1:1");
        let (x0, y0, x1, y1) = bbox(&f, is_magenta).unwrap();
        assert_eq!((x1 - x0 + 1, y1 - y0 + 1), (40, 30), "{name}");
        assert_eq!(x0 + x1 + 1, W, "{name}: centred");
        assert!(y0 > 100 && y1 < H - 100, "{name}: centred vertically");
        assert!(app.title().starts_with(name), "{}", app.title());
        let status = app.status_text();
        assert!(status.contains("40 × 30 px"), "{name}: {status}");
        assert!(status.contains("100%"), "{name}: {status}");
        let size = std::fs::metadata(&p).unwrap().len();
        let size = if size < 1024 {
            format!("{size} bytes")
        } else {
            format!("{:.1} KB", size as f64 / 1024.0)
        };
        assert!(status.contains(&size), "{name}: {status} / {size}");
    }
    assert!(app.tab_titles().iter().all(|t| !t.1), "never unsaved");
    assert_eq!(titles(&app).len(), 8);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn other_binary_files_and_svg_open_as_before() {
    let d = tmp_dir("img-other");
    let bin = d.join("data.bin");
    std::fs::write(&bin, b"\0\x01\x02binary\0\0").unwrap();
    let svg = d.join("drawing.svg");
    std::fs::write(&svg, "<svg xmlns=\"http://www.w3.org/2000/svg\"/>\n").unwrap();
    let ico = d.join("icon.ico");
    std::fs::write(&ico, b"\0\0\x01\0\x01\0").unwrap();
    let mut app = new_app();
    app.open(&bin);
    shown(&mut app);
    assert_eq!(titles(&app), ["Error"], "the binary-file message");
    let mut app = new_app();
    app.open(&ico);
    shown(&mut app);
    assert_eq!(titles(&app), ["Error"]);
    let mut app = new_app();
    app.open(&svg);
    shown(&mut app);
    assert_eq!(titles(&app), ["drawing.svg"]);
    assert!(app.view_text().starts_with("<svg"), "svg opens as text");
    let _ = std::fs::remove_dir_all(&d);
}

/// A PNG header claiming `w x h` with no real pixel data behind it.
fn png_header_only(w: u32, h: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut enc = png::Encoder::new(&mut bytes, w, h);
    enc.set_color(png::ColorType::Rgba);
    let mut wr = enc.write_header().unwrap();
    wr.write_chunk(
        png::chunk::IDAT,
        &[0x78, 0x9c, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01],
    )
    .unwrap();
    std::mem::forget(wr); // don't let the writer finish the stream
    bytes
}

#[test]
fn pictures_that_cannot_be_shown_give_a_message_not_a_crash() {
    let d = tmp_dir("img-bad");
    let huge = d.join("huge.png");
    std::fs::write(&huge, png_header_only(6000, 6000)).unwrap();
    let text = d.join("notes.png");
    std::fs::write(&text, "these are not pixels\n").unwrap();
    let empty = d.join("empty.jpg");
    std::fs::write(&empty, "").unwrap();
    let cut = d.join("cut.png");
    save(&solid(64, 64, MAGENTA), &cut);
    let whole = std::fs::read(&cut).unwrap();
    std::fs::write(&cut, &whole[..whole.len() / 2]).unwrap();
    #[cfg_attr(not(unix), allow(unused_mut))] // the link is added on unix only
    let mut bad = vec![
        fixture("img/broken.png"),
        huge,
        text,
        empty,
        cut,
        d.join("missing.png"),
    ];
    #[cfg(unix)]
    {
        let link = d.join("link.png");
        std::os::unix::fs::symlink(fixture("img/banner.png"), &link).unwrap();
        bad.push(link);
    }
    theme::set_light(false);
    let mut app = new_app();
    for p in &bad {
        app.open(p);
        let f = shown(&mut app);
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        assert!(app.title().starts_with(&name), "{}", app.title());
        assert!(count(&f, is_error_red) > 20, "{name}: a message in red");
        assert!(!app.status_text().contains("px"), "{}", app.status_text());
        assert!(!app.status_text().contains("loading"), "{name}");
        // No panic at awkward sizes either.
        for (w, h) in [(1, 1), (40, 40), (300, 90)] {
            app.handle(Input::Resize(w, h));
            crate::tests::render_hash(&mut app, w, h);
        }
        app.handle(Input::Resize(W, H));
        // Keys and the mouse do nothing harmful.
        for k in ["1", "0", "down", "end", "pagedown", "ctrl+s"] {
            key(&mut app, k);
        }
        app.handle(Input::MouseDown {
            x: 400.0,
            y: 300.0,
            button: MouseButton::Left,
            clicks: 2,
        });
        wheel(&mut app, 3.0);
        shown(&mut app);
        assert!(app.nag_message().is_none());
    }
    assert_eq!(app.tab_count(), bad.len());
    assert!(
        app.tabs_with_caches().iter().all(|c| !c),
        "no pixels are held for a picture that failed"
    );
    assert!(std::fs::read(fixture("img/broken.png")).is_ok());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn fit_to_window_and_actual_size() {
    let d = tmp_dir("img-fit");
    let p = d.join("wide.png");
    save(&halves(1600, 1200), &p);
    let mut app = new_app();
    app.open(&p);
    key(&mut app, "ctrl+\\");
    // Fitted: the whole picture, both halves, 4:3, inside the window.
    let fit = shown(&mut app);
    let pic = |p: u32| is_green(p) || is_blue(p);
    let (x0, y0, x1, y1) = bbox(&fit, pic).unwrap();
    let (w, h) = (x1 - x0 + 1, y1 - y0 + 1);
    assert!(w < W && h < H - 40, "{w}x{h}");
    assert!((w as f32 / h as f32 - 4.0 / 3.0).abs() < 0.01, "{w}x{h}");
    assert!((x0 + x1 + 1).abs_diff(W) <= 1, "centred");
    let (g, b) = (count(&fit, is_green), count(&fit, is_blue));
    assert!(g.abs_diff(b) <= h as usize, "{g} green, {b} blue");
    let status = app.status_text();
    assert!(status.contains("1600 × 1200 px"), "{status}");
    let percent = format!("{}%", (w as f32 / 16.0).round());
    assert!(status.contains(&percent), "{status} / {percent}");
    assert_eq!(app.view_scroll_y(), 0.0);
    // Nothing to scroll while fitted.
    wheel(&mut app, 3.0);
    key(&mut app, "down");
    assert_eq!(shown(&mut app), fit);
    // A smaller window: a smaller picture, still whole.
    app.handle(Input::Resize(400, 300));
    let mut small = vec![0u32; 400 * 300];
    app.render(&mut Canvas::new(&mut small, 400, 300));
    let (g, b) = (count(&small, is_green), count(&small, is_blue));
    assert!(g > 10_000 && g.abs_diff(b) <= 300, "{g} green, {b} blue");
    let rows = small.chunks(400).filter(|r| r.iter().any(|p| pic(*p)));
    let rows = rows.count();
    let cols = (g + b) / rows;
    assert!(rows < 300 && cols < 400, "{cols}x{rows}");
    assert!(cols.abs_diff(rows * 4 / 3) <= 1, "{cols}x{rows}");
    app.handle(Input::Resize(W, H));
    assert_eq!(shown(&mut app), fit);

    // 1: actual size, centred on what was in the middle.
    key(&mut app, "1");
    let actual = shown(&mut app);
    assert!(app.status_text().contains("100%"), "{}", app.status_text());
    let (ax0, ay0, ax1, ay1) = bbox(&actual, pic).unwrap();
    assert_eq!((ax0, ax1), (0, W - 1), "fills the width");
    assert!(ay1 - ay0 + 1 >= h, "and all the height");
    let (g, b) = (count(&actual, is_green), count(&actual, is_blue));
    assert!(g.abs_diff(b) <= 2 * H as usize, "{g} green, {b} blue");
    let mid_y = app.view_scroll_y();
    assert!(mid_y > 100.0, "{mid_y}");
    // Wheel and arrow keys scroll; the ends stop it.
    wheel(&mut app, 1.0);
    let after_wheel = app.view_scroll_y();
    assert!(after_wheel > mid_y);
    key(&mut app, "down");
    assert!(app.view_scroll_y() > after_wheel);
    key(&mut app, "up");
    assert_eq!(app.view_scroll_y(), after_wheel);
    key(&mut app, "home");
    assert_eq!(app.view_scroll_y(), 0.0);
    let top_left = shown(&mut app);
    assert_eq!(count(&top_left, is_blue), 0, "only the left half");
    key(&mut app, "up");
    key(&mut app, "left");
    assert_eq!(shown(&mut app), top_left, "can't scroll past the corner");
    key(&mut app, "end");
    let bottom_right = shown(&mut app);
    assert_eq!(count(&bottom_right, is_green), 0, "only the right half");
    let end_y = app.view_scroll_y();
    wheel(&mut app, 50.0);
    key(&mut app, "pagedown");
    key(&mut app, "right");
    assert_eq!(app.view_scroll_y(), end_y);
    assert_eq!(shown(&mut app), bottom_right);
    key(&mut app, "pageup");
    assert!(app.view_scroll_y() < end_y);
    // Shift+wheel goes sideways: back to where green shows.
    app.handle(Input::Modifiers(Mods {
        shift: true,
        ..Mods::default()
    }));
    let y_before = app.view_scroll_y();
    wheel(&mut app, -100.0);
    app.handle(Input::Modifiers(Mods::default()));
    assert_eq!(app.view_scroll_y(), y_before);
    assert_eq!(count(&shown(&mut app), is_blue), 0);
    // Dragging pans (the picture follows the mouse).
    key(&mut app, "home");
    app.handle(Input::MouseDown {
        x: 500.0,
        y: 400.0,
        button: MouseButton::Left,
        clicks: 1,
    });
    app.handle(Input::MouseMove { x: 380.0, y: 330.0 });
    assert_eq!(app.view_scroll_y(), 70.0);
    app.handle(Input::MouseUp {
        x: 380.0,
        y: 330.0,
        button: MouseButton::Left,
    });
    app.handle(Input::MouseMove { x: 100.0, y: 100.0 });
    assert_eq!(app.view_scroll_y(), 70.0, "released: no more panning");
    // Ctrl+wheel is the app's zoom, not a scroll.
    let zoom = app.zoom();
    app.handle(Input::Modifiers(Mods {
        ctrl: true,
        ..Mods::default()
    }));
    wheel(&mut app, -1.0);
    app.handle(Input::Modifiers(Mods::default()));
    assert!(app.zoom() > zoom);
    assert_eq!(app.view_scroll_y(), 70.0);
    key(&mut app, "ctrl+0");
    assert_eq!(app.zoom(), zoom);

    // 0: fitted again, exactly as before.
    key(&mut app, "0");
    assert_eq!(shown(&mut app), fit);
    assert_eq!(app.view_scroll_y(), 0.0);
    // Double-click toggles, keeping the clicked spot under the mouse.
    let click = |app: &mut App, x: f32, y: f32| {
        app.handle(Input::MouseDown {
            x,
            y,
            button: MouseButton::Left,
            clicks: 2,
        });
        app.handle(Input::MouseUp {
            x,
            y,
            button: MouseButton::Left,
        });
    };
    let (cx, cy) = ((x0 + w / 8) as f32, (y0 + h / 2) as f32);
    click(&mut app, cx, cy);
    let zoomed = shown(&mut app);
    assert!(app.status_text().contains("100%"));
    // The picture's column under the mouse, before and after: the blue
    // half starts at column 800 of the picture.
    let before = (cx - x0 as f32) * 1600.0 / w as f32;
    let (blue_from, ..) = bbox(&zoomed, is_blue).unwrap();
    let after = 800.0 - (blue_from as f32 - cx);
    assert!((before - after).abs() <= 2.0, "{before} -> {after}");
    assert!(is_green(zoomed[(cy as u32 * W + cx as u32) as usize]));
    click(&mut app, cx, cy);
    assert_eq!(shown(&mut app), fit);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_small_picture_is_never_enlarged() {
    let d = tmp_dir("img-small");
    let p = d.join("dot.png");
    save(&solid(20, 10, MAGENTA), &p);
    let mut app = new_app();
    app.open(&p);
    let fit = shown(&mut app);
    assert_eq!(count(&fit, is_magenta), 200);
    assert!(app.status_text().contains("100%"));
    key(&mut app, "1");
    assert_eq!(shown(&mut app), fit, "already at actual size");
    key(&mut app, "down");
    wheel(&mut app, 2.0);
    assert_eq!(shown(&mut app), fit);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_very_large_picture_shows_every_pixel_at_actual_size() {
    // Wider than the copy kept for fitted viewing (2400 px).
    let d = tmp_dir("img-big");
    let p = d.join("strip.png");
    let img = RgbaImage::from_fn(2600, 300, |x, _| Rgba(if x < 2500 { GREEN } else { BLUE }));
    save(&img, &p);
    let mut app = new_app();
    app.open(&p);
    key(&mut app, "ctrl+\\");
    let fit = shown(&mut app);
    let status = app.status_text();
    assert!(status.contains("2600 × 300 px"), "{status}");
    assert!(status.contains("31%"), "{status}"); // 800 / 2600
    let (x0, _, x1, _) = bbox(&fit, |p| is_green(p) || is_blue(p)).unwrap();
    assert_eq!((x0, x1), (0, W - 1));
    // At 100% the last 100 columns are exactly 100 screen pixels wide.
    key(&mut app, "1");
    key(&mut app, "end");
    let end = shown(&mut app);
    assert!(app.status_text().contains("100%"));
    let (bx0, by0, bx1, by1) = bbox(&end, is_blue).unwrap();
    assert_eq!((bx0, bx1), (W - 100, W - 1));
    assert_eq!(by1 - by0 + 1, 300);
    assert_eq!(count(&end, is_blue), 100 * 300);
    assert_eq!(count(&end, is_green), 700 * 300);
    // Back to fitted: the same picture as before.
    key(&mut app, "0");
    assert_eq!(shown(&mut app), fit);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_hidden_image_tab_frees_its_pixels_and_comes_back_identical() {
    let d = tmp_dir("img-hide");
    let p = d.join("pic.png");
    save(&halves(1600, 1200), &p);
    let other = d.join("notes.txt");
    std::fs::write(&other, "notes\n").unwrap();
    let mut app = new_app();
    app.open(&p);
    settled_hash(&mut app);
    assert_eq!(app.tabs_with_caches(), [true]);
    app.open(&other);
    key(&mut app, "alt+1");
    let fit = settled_hash(&mut app);
    let status = app.status_text();
    key(&mut app, "alt+2");
    settled_hash(&mut app);
    assert_eq!(
        app.tabs_with_caches(),
        [false, true],
        "the hidden picture holds no pixels"
    );
    key(&mut app, "alt+1");
    assert_eq!(settled_hash(&mut app), fit);
    assert_eq!(app.status_text(), status);
    // At actual size and scrolled: the same spot afterwards.
    key(&mut app, "1");
    key(&mut app, "down");
    let actual = settled_hash(&mut app);
    let y = app.view_scroll_y();
    assert!(y > 0.0);
    assert_ne!(actual, fit);
    key(&mut app, "alt+2");
    settled_hash(&mut app);
    assert_eq!(app.tabs_with_caches(), [false, true]);
    key(&mut app, "alt+1");
    // The size is remembered while the pixels are away.
    assert!(app.status_text().contains("1600 × 1200 px"));
    assert_eq!(settled_hash(&mut app), actual);
    assert_eq!(app.view_scroll_y(), y);
    key(&mut app, "0");
    assert_eq!(settled_hash(&mut app), fit);
    assert_eq!(app.status_text(), status);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn only_the_tab_on_screen_ever_decodes() {
    let d = tmp_dir("img-many");
    let files: Vec<PathBuf> = (0..10)
        .map(|i| {
            let p = d.join(format!("p{i}.png"));
            save(&solid(300 + i, 200, MAGENTA), &p);
            p
        })
        .collect();
    // Opened in one go (as a restored session does): nothing decodes until
    // a tab is shown, and then only that one.
    let mut app = new_app();
    for f in &files {
        app.open(f);
    }
    assert!(app.tabs_with_caches().iter().all(|c| !c));
    shown(&mut app);
    let mut expect = vec![false; 10];
    expect[9] = true;
    assert_eq!(app.tabs_with_caches(), expect);
    // Each viewed once: still just the one in front.
    for i in (0..10).rev() {
        app.activate(i);
        let f = shown(&mut app);
        assert_eq!(count(&f, is_magenta), (300 + i) * 200);
        let held: Vec<usize> = (0..10).filter(|&k| app.tabs_with_caches()[k]).collect();
        assert_eq!(held, [i]);
    }
    // Hidden while still loading: the decode is dropped, not kept.
    let mut app = new_app();
    app.open(&files[0]);
    frame(&mut app); // starts decoding
    app.open(&files[1]);
    frame(&mut app);
    assert!(!app.tabs_with_caches()[0]);
    shown(&mut app);
    assert_eq!(app.tabs_with_caches(), [false, true]);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_picture_changed_on_disk_is_reloaded() {
    let d = tmp_dir("img-reload");
    let p = d.join("plot.png");
    save(&solid(40, 30, MAGENTA), &p);
    let other = d.join("notes.txt");
    std::fs::write(&other, "notes\n").unwrap();
    let mut app = new_app();
    app.open(&p);
    assert_eq!(count(&shown(&mut app), is_magenta), 1200);
    // Another program writes a different picture.
    save(&solid(80, 60, GREEN), &p);
    app.handle(Input::FsChanged(vec![p.clone()]));
    let f = shown(&mut app);
    assert_eq!(count(&f, is_magenta), 0);
    assert_eq!(count(&f, is_green), 80 * 60);
    assert!(app.status_text().contains("80 × 60 px"));
    assert!(app.nag_message().is_none());
    // An unrelated change doesn't flicker the picture.
    app.handle(Input::FsChanged(vec![other.clone()]));
    assert_eq!(app.tabs_with_caches(), [true]);
    app.handle(Input::FsChanged(vec![p.clone()]));
    assert_eq!(app.tabs_with_caches(), [true], "nothing really changed");
    // Changed while its tab is hidden: nothing is decoded until it is shown.
    app.open(&other);
    shown(&mut app);
    save(&solid(50, 50, BLUE), &p);
    app.handle(Input::FsChanged(vec![p.clone()]));
    shown(&mut app);
    assert_eq!(app.tabs_with_caches(), [false, true]);
    key(&mut app, "alt+1");
    let f = shown(&mut app);
    assert_eq!(count(&f, is_blue), 2500);
    assert_eq!(count(&f, is_green), 0);
    assert!(app.status_text().contains("50 × 50 px"));
    // Deleted: a message, and no talk of saving it again.
    std::fs::remove_file(&p).unwrap();
    app.handle(Input::FsChanged(vec![p.clone()]));
    let f = shown(&mut app);
    assert_eq!(count(&f, is_blue), 0);
    assert!(count(&f, is_error_red) > 20);
    assert!(
        !app.status_text().contains("Ctrl+S"),
        "{}",
        app.status_text()
    );
    assert_eq!(titles(&app), ["plot.png", "notes.txt"]);
    // Written again: the picture is back.
    save(&solid(40, 30, MAGENTA), &p);
    app.handle(Input::FsChanged(vec![p.clone()]));
    assert_eq!(count(&shown(&mut app), is_magenta), 1200);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn tree_clicks_peek_and_keep_pictures() {
    let d = tmp_dir("img-tree");
    let a = d.join("a.png");
    let b = d.join("b.jpg");
    let c = d.join("c.txt");
    save(&solid(40, 30, MAGENTA), &a);
    save(&solid(60, 50, GREEN), &b);
    std::fs::write(&c, "text\n").unwrap();
    let mut app = new_app();
    app.open(&d);
    // Single clicks browse in one peek tab.
    click_tree(&mut app, &a, 1);
    assert_eq!(titles(&app), ["a.png"]);
    assert_eq!(app.peek_tabs(), [true]);
    assert_eq!(count(&shown(&mut app), is_magenta), 1200);
    click_tree(&mut app, &b, 1);
    assert_eq!(titles(&app), ["b.jpg"]);
    assert_eq!(app.peek_tabs(), [true]);
    let f = shown(&mut app);
    assert_eq!(count(&f, is_green), 3000);
    assert_eq!(count(&f, is_magenta), 0);
    click_tree(&mut app, &c, 1);
    assert_eq!(titles(&app), ["c.txt"]);
    click_tree(&mut app, &a, 1);
    assert_eq!(titles(&app), ["a.png"]);
    // A double click keeps the picture open.
    click_tree(&mut app, &a, 2);
    assert_eq!(app.peek_tabs(), [false]);
    click_tree(&mut app, &b, 1);
    assert_eq!(titles(&app), ["a.png", "b.jpg"]);
    assert_eq!(app.peek_tabs(), [false, true]);
    assert_eq!(count(&shown(&mut app), is_green), 3000);
    // Looking at a picture (keys, clicks) never makes it "edited".
    app.handle(Input::MouseDown {
        x: 500.0,
        y: 300.0,
        button: MouseButton::Left,
        clicks: 1,
    });
    app.handle(Input::Text("x".into()));
    key(&mut app, "1");
    assert_eq!(app.peek_tabs(), [false, true]);
    assert!(app.tab_titles().iter().all(|t| !t.1));
    // Clicking the one already open shows that tab.
    click_tree(&mut app, &a, 1);
    assert_eq!(titles(&app), ["a.png", "b.jpg"]);
    assert_eq!(app.active_tab(), 0);
    assert_eq!(count(&shown(&mut app), is_magenta), 1200);
    // "Show in Explorer" and friends work from the file's path.
    assert!(
        app.watch_targets().iter().any(|(p, _)| p == &d),
        "the folder is watched"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn theme_switch_matches_a_fresh_render_both_ways() {
    // The fixture has transparency: the checkerboard is in theme colours.
    let banner = fixture("img/banner.png");
    theme::set_light(false);
    let mut a = new_app();
    a.open(&banner);
    let dark = settled_hash(&mut a);
    a.set_light(true);
    let switched = settled_hash(&mut a);
    let mut b = new_app();
    b.open(&banner);
    let fresh = settled_hash(&mut b);
    assert_eq!(switched, fresh, "switched vs fresh light");
    assert_ne!(dark, fresh, "light differs from dark");
    a.set_light(false);
    assert_eq!(settled_hash(&mut a), dark, "back to dark");
    // Switched while the tab was hidden.
    a.open(&fixture("sample.rs"));
    settled_hash(&mut a);
    a.set_light(true);
    key(&mut a, "alt+1");
    b.open(&fixture("sample.rs"));
    key(&mut b, "alt+1");
    assert_eq!(settled_hash(&mut a), settled_hash(&mut b));
    theme::set_light(false);
}

#[test]
fn a_picture_is_never_saved_or_written_to() {
    let d = tmp_dir("img-save");
    let p = d.join("pic.png");
    save(&solid(40, 30, MAGENTA), &p);
    let before = std::fs::read(&p).unwrap();
    let mut app = new_app();
    app.open(&p);
    shown(&mut app);
    app.take_requests();
    key(&mut app, "ctrl+s");
    assert!(app.nag_message().is_none(), "{:?}", app.nag_message());
    key(&mut app, "ctrl+shift+s");
    assert!(
        !app.take_requests()
            .iter()
            .any(|r| matches!(r, Request::SaveAsDialog(_))),
        "no Save As for a picture"
    );
    app.handle(Input::Text("typed".into()));
    key(&mut app, "delete");
    key(&mut app, "ctrl+z");
    assert!(!app.tab_titles()[0].1);
    // Changed behind our back, then Ctrl+S: no "save mine over it" question.
    save(&solid(41, 30, GREEN), &p);
    let changed = std::fs::read(&p).unwrap();
    key(&mut app, "ctrl+s");
    assert!(app.nag_message().is_none(), "{:?}", app.nag_message());
    assert_eq!(std::fs::read(&p).unwrap(), changed);
    assert_ne!(before, changed);
    // Closing needs no question.
    key(&mut app, "ctrl+w");
    assert!(app.nag_message().is_none());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn an_image_tab_comes_back_with_the_session() {
    let d = tmp_dir("img-session");
    let p = d.join("pic.png");
    save(&solid(40, 30, MAGENTA), &p);
    let t = d.join("notes.txt");
    std::fs::write(&t, "notes\n").unwrap();
    let mut app = new_app();
    app.open(&d);
    app.open(&t);
    app.open(&p);
    shown(&mut app);
    let s = app.session();
    assert!(s.tabs.contains(&p));
    let mut again = new_app();
    again.restore(s);
    assert_eq!(titles(&again), ["notes.txt", "pic.png"]);
    assert_eq!(count(&shown(&mut again), is_magenta), 1200);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_picture_on_each_side_of_a_split() {
    let d = tmp_dir("img-split");
    let a = d.join("a.png");
    let b = d.join("b.png");
    save(&solid(40, 30, MAGENTA), &a);
    save(&solid(60, 50, GREEN), &b);
    let mut app = new_app();
    app.open(&a);
    app.open(&b);
    key(&mut app, "ctrl+alt+right");
    let f = shown(&mut app);
    assert_eq!(count(&f, is_magenta), 1200);
    assert_eq!(count(&f, is_green), 3000);
    assert_eq!(app.tabs_with_caches(), [true, true]);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn comparing_a_picture_is_refused_with_a_message() {
    let d = tmp_dir("img-compare");
    let p = d.join("pic.png");
    save(&solid(40, 30, MAGENTA), &p);
    let t = d.join("notes.txt");
    std::fs::write(&t, "notes\n").unwrap();
    let mut app = new_app();
    app.open(&p);
    shown(&mut app);
    let before = titles(&app);
    app.compare_files(&p, &t);
    assert!(
        app.nag_text().starts_with("Cannot compare"),
        "{:?}",
        app.nag_text()
    );
    assert_eq!(titles(&app), before, "no comparison tab was opened");
    // Compare with Saved needs text: a picture just says so.
    key(&mut app, "escape");
    crate::tests::run_command(&mut app, "Compare with Saved");
    assert_eq!(titles(&app), before);
    shown(&mut app);
    assert_eq!(count(&frame(&mut app), is_magenta), 1200, "still shown");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn no_command_harms_or_crashes_on_a_picture_tab() {
    let d = tmp_dir("img-cmds");
    let p = d.join("pic.png");
    save(&solid(40, 30, MAGENTA), &p);
    let before = std::fs::read(&p).unwrap();
    let mut app = new_app();
    app.open(&d);
    app.open(&p);
    shown(&mut app);
    for (label, _) in crate::palette::COMMANDS {
        if label.contains("Settings") || label.contains("New File") || label.contains("Full Screen")
        {
            continue; // these act on the window or other files, not the tab
        }
        if titles(&app).first().map(String::as_str) != Some("pic.png") {
            app.open(&p);
        }
        crate::tests::run_command(&mut app, label);
        for _ in 0..2 {
            key(&mut app, "escape");
        }
        app.take_requests();
        shown(&mut app);
        assert!(!app.tab_titles().iter().any(|t| t.1), "{label}: modified");
        assert_eq!(std::fs::read(&p).unwrap(), before, "{label}");
        let _ = d.read_dir().map(|r| {
            assert_eq!(r.count(), 1, "{label}: a file appeared in the folder");
        });
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn transparent_pictures_sit_on_a_theme_coloured_checkerboard() {
    let d = tmp_dir("img-alpha");
    let opaque = d.join("opaque.png");
    let clear = d.join("clear.png");
    save(&solid(40, 30, MAGENTA), &opaque);
    // Left half see-through, right half magenta.
    let half = RgbaImage::from_fn(40, 30, |x, _| {
        Rgba(if x < 20 { [0, 0, 0, 0] } else { MAGENTA })
    });
    save(&half, &clear);
    let packed =
        |c: crate::canvas::Color| (u32::from(c.r) << 16) | (u32::from(c.g) << 8) | u32::from(c.b);
    for light in [false, true] {
        theme::set_light(light);
        let (a, b) = (packed(theme::p().background2), packed(theme::p().scrollbar));
        let mut app = new_app();
        app.open(&d);
        key(&mut app, "ctrl+\\");
        app.open(&clear);
        let f = shown(&mut app);
        let (x0, y0, x1, y1) = bbox(&f, is_magenta).unwrap();
        assert_eq!((x1 - x0 + 1, y1 - y0 + 1), (20, 30));
        // The left half of the picture: both squares colours, nothing else.
        let left: Vec<u32> = (y0..=y1)
            .flat_map(|y| (x0 - 20..x0).map(move |x| (x, y)))
            .map(|(x, y)| f[(y * W + x) as usize])
            .collect();
        assert!(left.contains(&a) && left.contains(&b), "light={light}");
        assert!(left.iter().all(|p| *p == a || *p == b), "light={light}");
        // An opaque picture has no squares behind it.
        app.open(&opaque);
        let f = shown(&mut app);
        let (x0, y0, x1, y1) = bbox(&f, is_magenta).unwrap();
        let ring = (y0 - 2..=y1 + 2)
            .flat_map(|y| (x0 - 2..=x1 + 2).map(move |x| (x, y)))
            .filter(|&(x, y)| !(x0..=x1).contains(&x) || !(y0..=y1).contains(&y))
            .all(|(x, y)| f[(y * W + x) as usize] != b);
        assert!(
            ring,
            "light={light}: no checkerboard around an opaque picture"
        );
    }
    theme::set_light(false);
    let _ = std::fs::remove_dir_all(&d);
}
