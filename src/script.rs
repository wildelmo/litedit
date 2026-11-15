//! `--script` mode: drive the app with a list of input events and take
//! screenshots/assertions along the way. This is how features get exercised
//! in CI without a human at the keyboard.
//!
//! One command per line; `#` starts a comment.
//!
//! ```text
//! size 1280 800          # window size in physical pixels
//! scale 1.5              # DPI scale
//! open README.md         # open a file or folder
//! theme light            # switch palette (headless runs start dark)
//! settings path.txt      # use this settings file (as the window does)
//! compare old.rs new.rs  # open a side-by-side comparison
//! mods shift             # hold modifiers for following clicks/wheel (mods none)
//! saveworkspace a.litedit-workspace   # save the workspace (open one with `open`)
//! workspaces dir         # remember per-folder workspaces in dir (as the window does)
//! addfolder dir          # add another folder to the tree
//! move 100 200           # mouse move
//! click 100 200 [right|middle] [double]
//! down 100 200 / up 100 200
//! wheel 3 [dx] [px]      # +3 = scroll down 3 notches
//! key ctrl+shift+o
//! text hello world
//! drop path/to/file
//! fschanged path/to/file # simulate an external file change
//! settle                 # finish deferred work
//! shot out.png           # screenshot (settles first)
//! expect request OpenFolderDialog
//! expect title README
//! expect status lines
//! ```

use std::path::Path;
use std::time::Duration;

use crate::app::{App, resolve_arg_path};
use crate::canvas::Canvas;
use crate::input::{Input, MouseButton, Request, WheelUnit, parse_key_combo};

pub fn run(app: &mut App, script: &str) -> Result<(), String> {
    let mut size = (1280u32, 800u32);
    app.handle(Input::Resize(size.0, size.1));
    let mut requests: Vec<Request> = Vec::new();

    for (n, raw) in script.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let err = |m: &str| format!("script line {}: {m}: {raw}", n + 1);
        let (cmd, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let rest = rest.trim();
        let nums: Vec<f32> = rest
            .split_whitespace()
            .filter_map(|t| t.parse().ok())
            .collect();
        let num = |i: usize| nums.get(i).copied().ok_or_else(|| err("missing number"));

        match cmd {
            "size" => {
                size = (num(0)? as u32, num(1)? as u32);
                if size.0 == 0 || size.1 == 0 || size.0 > 8192 || size.1 > 8192 {
                    return Err(err("size out of range"));
                }
                app.handle(Input::Resize(size.0, size.1));
            }
            "scale" => {
                app.handle(Input::Scale(num(0)?.clamp(0.5, 4.0)));
            }
            "open" => app.open(&resolve_arg_path(rest)),
            "theme" => match rest {
                "light" => app.set_light(true),
                "dark" => app.set_light(false),
                _ => return Err(err("theme needs light or dark")),
            },
            "settings" => app.use_settings(resolve_arg_path(rest)),
            "saveworkspace" => app.save_workspace_as(&resolve_arg_path(rest)),
            "addfolder" => app.add_folder(&resolve_arg_path(rest)),
            "workspaces" => app.use_workspace_store(resolve_arg_path(rest)),
            "mods" => {
                let m = crate::input::Mods {
                    ctrl: rest.contains("ctrl"),
                    shift: rest.contains("shift"),
                    alt: rest.contains("alt"),
                };
                app.handle(Input::Modifiers(m));
            }
            "compare" => {
                let (a, b) = rest
                    .split_once(' ')
                    .ok_or_else(|| err("compare needs two paths"))?;
                app.compare_files(&resolve_arg_path(a), &resolve_arg_path(b.trim()));
            }
            "drop" => {
                app.handle(Input::Drop(resolve_arg_path(rest)));
            }
            "fschanged" => {
                app.handle(Input::FsChanged(vec![resolve_arg_path(rest)]));
            }
            "move" => {
                app.handle(Input::MouseMove {
                    x: num(0)?,
                    y: num(1)?,
                });
            }
            "click" | "down" | "up" => {
                let (x, y) = (num(0)?, num(1)?);
                let button = if rest.contains("right") {
                    MouseButton::Right
                } else if rest.contains("middle") {
                    MouseButton::Middle
                } else {
                    MouseButton::Left
                };
                let clicks = if rest.contains("double") { 2 } else { 1 };
                app.handle(Input::MouseMove { x, y });
                if cmd != "up" {
                    app.handle(Input::MouseDown {
                        x,
                        y,
                        button,
                        clicks,
                    });
                }
                if cmd != "down" {
                    app.handle(Input::MouseUp { x, y, button });
                }
            }
            "wheel" => {
                let unit = if rest.contains("px") {
                    WheelUnit::Pixels
                } else {
                    WheelUnit::Lines
                };
                app.handle(Input::Wheel {
                    dx: nums.get(1).copied().unwrap_or(0.0),
                    dy: num(0)?,
                    unit,
                });
            }
            "key" => {
                let (key, mods) = parse_key_combo(rest).ok_or_else(|| err("bad key"))?;
                app.handle(Input::Key { key, mods });
            }
            "text" => {
                app.handle(Input::Text(unescape(rest)));
            }
            "close" => {
                app.handle(Input::CloseRequested);
            }
            "save" => {
                app.save();
            }
            "saveas" => app.save_as(&resolve_arg_path(rest)),
            "settle" => app.settle(Duration::from_secs(10)),
            "shot" => {
                app.settle(Duration::from_secs(10));
                screenshot(app, size, Path::new(rest)).map_err(|e| err(&e.to_string()))?;
            }
            "expect" => {
                requests.extend(app.take_requests());
                if rest == "no-nag" {
                    if !app.nag_text().is_empty() {
                        return Err(err(&format!("unexpected question bar: {}", app.nag_text())));
                    }
                    continue;
                }
                let (what, needle) = rest
                    .split_once(char::is_whitespace)
                    .ok_or_else(|| err("bad expect"))?;
                if what == "file" {
                    // expect file <path> <text with \n \r \t escapes>
                    let (path, want) = needle
                        .split_once(char::is_whitespace)
                        .unwrap_or((needle, ""));
                    let want = unescape(want);
                    let got =
                        std::fs::read(resolve_arg_path(path)).map_err(|e| err(&e.to_string()))?;
                    if got != want.as_bytes() {
                        return Err(err(&format!(
                            "file {path}: expected {want:?}, got {:?}",
                            String::from_utf8_lossy(&got)
                        )));
                    }
                    continue;
                }
                let hay = match what {
                    "request" => format!("{requests:?}"),
                    "title" => app.title(),
                    "status" => app.status_text(),
                    "nag" => app.nag_text(),
                    _ => return Err(err("unknown expect target")),
                };
                if !hay.contains(needle.trim()) {
                    return Err(err(&format!("expected {needle:?} in {what}, got {hay:?}")));
                }
            }
            _ => return Err(err("unknown command")),
        }
    }
    Ok(())
}

/// Unescape `\n`, `\r`, `\t`, `\\` and `\s` (space) in script text.
fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('t') => out.push('\t'),
                Some('s') => out.push(' '),
                Some(o) => out.push(o),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Render the app at `size` and write a PNG.
pub fn screenshot(app: &mut App, size: (u32, u32), out: &Path) -> std::io::Result<()> {
    let (w, h) = size;
    let mut buf = vec![0u32; (w * h) as usize];
    {
        let mut canvas = Canvas::new(&mut buf, w, h);
        app.render(&mut canvas);
    }
    // Drawing can start background work (images); finish it and draw again.
    if app.has_pending_work() {
        app.settle(std::time::Duration::from_secs(10));
        let mut canvas = Canvas::new(&mut buf, w, h);
        app.render(&mut canvas);
    }
    let mut rgb = Vec::with_capacity(buf.len() * 3);
    for p in &buf {
        rgb.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8]);
    }
    if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let file = std::io::BufWriter::new(std::fs::File::create(out)?);
    let mut enc = png::Encoder::new(file, w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(std::io::Error::other)?;
    writer
        .write_image_data(&rgb)
        .map_err(std::io::Error::other)?;
    Ok(())
}
