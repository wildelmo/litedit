// No console window on Windows release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod assoc;
mod bench;
mod canvas;
mod clipboard;
mod diff;
mod doc;
mod editor;
mod find;
mod fsops;
mod images;
mod input;
mod inputbar;
mod links;
mod markdown;
mod menu;
mod nag;
mod nav;
mod palette;
mod platform;
mod script;
mod session;
mod settings;
mod single;
mod syntax;
mod text;
mod theme;
mod tree;
mod ui;
mod view;
mod watch;
mod window;
mod workspace;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_compare;
#[cfg(test)]
mod tests_image;
#[cfg(test)]
mod tests_nav;
#[cfg(test)]
mod tests_tree;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use app::App;
use text::Fonts;

struct Args {
    paths: Vec<String>,
    screenshot: Option<PathBuf>,
    script: Option<PathBuf>,
    bench: Option<PathBuf>,
    size: (u32, u32),
    scale: f32,
    stats: Option<PathBuf>,
    light: bool,
    new_window: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        paths: Vec::new(),
        screenshot: None,
        script: None,
        bench: None,
        size: (1280, 800),
        scale: 1.0,
        stats: None,
        light: false,
        new_window: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--screenshot" => {
                a.screenshot = Some(it.next().ok_or("--screenshot needs a file")?.into())
            }
            "--script" => a.script = Some(it.next().ok_or("--script needs a file")?.into()),
            "--bench" => a.bench = Some(it.next().ok_or("--bench needs an output file")?.into()),
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
            "--new-window" => a.new_window = true,
            "--theme" => match it.next().as_deref() {
                Some("light") => a.light = true,
                Some("dark") => a.light = false,
                _ => return Err("--theme needs light or dark".into()),
            },
            "--version" => {
                println!("litedit {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            _ if arg.starts_with("--") => return Err(format!("unknown option {arg}")),
            _ => a.paths.push(arg),
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

/// `--theme` wins; otherwise the window follows settings, headless is dark.
fn app_light(args: &Args, user: &settings::Settings, window_mode: bool) -> bool {
    if std::env::args().any(|a| a == "--theme") {
        args.light
    } else {
        window_mode && user.light
    }
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

    // Only the window reads the user's settings; headless runs (screenshot,
    // script, bench) use the defaults so their output is reproducible.
    let window_mode = args.script.is_none() && args.screenshot.is_none() && args.bench.is_none();
    let settings_path = window_mode.then(settings::settings_file).flatten();
    let user = settings_path
        .as_deref()
        .map(|p| settings::Settings::load(p).0)
        .unwrap_or_default();
    // One window: if litedit is already running, hand it the files and
    // exit (before loading fonts, so it is instant). Only plain files are
    // handed over; on any doubt this launch opens its own window.
    let single = window_mode && user.single_window && !args.new_window;
    if single {
        let paths: Vec<PathBuf> = args
            .paths
            .iter()
            .map(|p| app::resolve_arg_path(p))
            .collect();
        if single::forwardable(&paths) && single::try_forward(&paths) {
            return;
        }
    }
    let fonts = Fonts::load_with(user.ui_font.as_deref(), user.code_font.as_deref());
    let font_ms = fonts.load_ms;
    let mut app = App::new(fonts, args.scale);
    if let Some(p) = settings_path {
        app.use_settings(p);
    }
    app.set_light(app_light(&args, &user, window_mode));
    // Window mode: load the saved session first (recents, preferences; the
    // folder and tabs only when started without files), then open arguments.
    if let Some(out) = &args.bench {
        if let Err(e) = bench::run(out, app) {
            eprintln!("litedit: bench failed: {e}");
            std::process::exit(1);
        }
        return;
    }
    if window_mode && let Some(store) = workspace::default_store() {
        app.use_workspace_store(store);
    }
    if window_mode && let Some(f) = session::session_file() {
        app.start_with_session(session::Session::load(&f), args.paths.is_empty());
    }
    // Each path opens in its own tab (a folder goes to the tree).
    for p in &args.paths {
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

    window::run(app, single);
}
