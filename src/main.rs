// No console window on Windows release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod canvas;
mod doc;
mod images;
mod input;
mod links;
mod markdown;
mod platform;
mod script;
mod syntax;
mod text;
mod theme;
mod ui;
mod view;
mod window;

#[cfg(test)]
mod tests;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use app::App;
use text::Fonts;

struct Args {
    path: Option<String>,
    screenshot: Option<PathBuf>,
    script: Option<PathBuf>,
    size: (u32, u32),
    scale: f32,
    stats: Option<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        path: None,
        screenshot: None,
        script: None,
        size: (1280, 800),
        scale: 1.0,
        stats: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--screenshot" => {
                a.screenshot = Some(it.next().ok_or("--screenshot needs a file")?.into())
            }
            "--script" => a.script = Some(it.next().ok_or("--script needs a file")?.into()),
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
                a.scale = it
                    .next()
                    .ok_or("--scale needs a number")?
                    .parse()
                    .map_err(|_| "bad scale")?;
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
            std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(Path::to_path_buf)),
            Some(std::env::temp_dir()),
        ];
        for dir in dirs.into_iter().flatten() {
            let path = dir.join("litedit-crash.log");
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
            {
                let _ = f.write_all(msg.as_bytes());
                break;
            }
        }
    }));
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

    if let Some(script_path) = &args.script {
        let text = match std::fs::read_to_string(script_path) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("litedit: cannot read script: {e}");
                std::process::exit(2);
            }
        };
        if let Err(e) = script::run(&mut app, &text) {
            eprintln!("litedit: {e}");
            std::process::exit(1);
        }
        return;
    }

    if let Some(out) = &args.screenshot {
        let t = Instant::now();
        app.handle(input::Input::Scale(args.scale));
        app.handle(input::Input::Resize(args.size.0, args.size.1));
        app.settle(std::time::Duration::from_secs(10));
        if let Err(e) = script::screenshot(&mut app, args.size, out) {
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
