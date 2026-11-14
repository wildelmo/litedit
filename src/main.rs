// No console window on Windows release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod canvas;
mod doc;
mod text;
mod theme;
mod window;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use app::App;
use canvas::Canvas;
use text::Fonts;

struct Args {
    path: Option<String>,
    screenshot: Option<PathBuf>,
    size: (u32, u32),
    scale: f32,
    stats: Option<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args { path: None, screenshot: None, size: (1280, 800), scale: 1.0, stats: None };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--screenshot" => a.screenshot = Some(it.next().ok_or("--screenshot needs a file")?.into()),
            "--stats" => a.stats = Some(it.next().ok_or("--stats needs a file")?.into()),
            "--size" => {
                let v = it.next().ok_or("--size needs WxH")?;
                let (w, h) = v.split_once('x').ok_or("--size needs WxH")?;
                a.size = (
                    w.parse().map_err(|_| "bad width")?,
                    h.parse().map_err(|_| "bad height")?,
                );
                if a.size.0 == 0 || a.size.1 == 0 || a.size.0 > 8192 || a.size.1 > 8192 {
                    return Err("--size out of range".into());
                }
            }
            "--scale" => {
                a.scale = it.next().ok_or("--scale needs a number")?.parse().map_err(|_| "bad scale")?;
                if !(0.5..=4.0).contains(&a.scale) {
                    return Err("--scale out of range".into());
                }
            }
            "--version" => {
                println!("litedit {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            _ if arg.starts_with("--") => return Err(format!("unknown option {arg}")),
            _ => a.path = Some(arg),
        }
    }
    Ok(a)
}

/// A panic in a windowed app is invisible, so write it to a log file next to
/// the exe (or the temp dir if that is not writable).
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let msg = format!("litedit {} crashed: {info}\n", env!("CARGO_PKG_VERSION"));
        eprint!("{msg}");
        let dirs = [
            std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)),
            Some(std::env::temp_dir()),
        ];
        for dir in dirs.into_iter().flatten() {
            let path = dir.join("litedit-crash.log");
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
                let _ = f.write_all(msg.as_bytes());
                break;
            }
        }
    }));
}

fn screenshot(app: &mut App, size: (u32, u32), out: &Path) -> std::io::Result<()> {
    let (w, h) = size;
    let mut buf = vec![0u32; (w * h) as usize];
    {
        let mut canvas = Canvas::new(&mut buf, w, h);
        app.render(&mut canvas);
    }
    let mut rgb = Vec::with_capacity(buf.len() * 3);
    for p in &buf {
        rgb.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8]);
    }
    let file = std::io::BufWriter::new(std::fs::File::create(out)?);
    let mut enc = png::Encoder::new(file, w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(std::io::Error::other)?;
    writer.write_image_data(&rgb).map_err(std::io::Error::other)?;
    Ok(())
}

fn main() {
    let started = Instant::now();
    install_panic_hook();
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("litedit: {e}");
            std::process::exit(2);
        }
    };

    let fonts = Fonts::load();
    let font_ms = fonts.load_ms;
    let mut app = App::new(fonts, args.scale);
    if let Some(p) = &args.path {
        app.open(&app::resolve_arg_path(p));
    }

    if let Some(out) = &args.screenshot {
        let t = Instant::now();
        if let Err(e) = screenshot(&mut app, args.size, out) {
            eprintln!("litedit: screenshot failed: {e}");
            std::process::exit(1);
        }
        let render_ms = t.elapsed().as_millis();
        if let Some(stats) = &args.stats {
            let report = format!(
                "font_load_ms={font_ms}\nfirst_render_ms={render_ms}\ntotal_ms={}\nsans={}\nmono={}\n",
                started.elapsed().as_millis(),
                app.fonts.sans,
                app.fonts.mono
            );
            let _ = std::fs::write(stats, report);
        }
        return;
    }

    window::run(app);
}
