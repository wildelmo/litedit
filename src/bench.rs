//! `--bench <out.md>`: performance numbers measured on the machine running
//! litedit (Windows CI prints them in the run summary). Uses the same code
//! paths as the window: input through `App::handle`, frames through
//! `App::render`.

use std::path::Path;
use std::time::{Duration, Instant};

use crate::app::App;
use crate::canvas::Canvas;
use crate::input::{Input, parse_key_combo};

struct Stats {
    avg: f64,
    p95: f64,
    max: f64,
}

fn stats(mut v: Vec<f64>) -> Stats {
    v.sort_by(|a, b| a.total_cmp(b));
    let avg = v.iter().sum::<f64>() / v.len().max(1) as f64;
    let p95 = v
        .get(((v.len() as f64 * 0.95) as usize).min(v.len().saturating_sub(1)))
        .copied()
        .unwrap_or(0.0);
    Stats {
        avg,
        p95,
        max: v.last().copied().unwrap_or(0.0),
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn key(app: &mut App, combo: &str) {
    if let Some((key, mods)) = parse_key_combo(combo) {
        app.handle(Input::Key { key, mods });
    }
}

fn frame(app: &mut App, buf: &mut [u32], w: u32, h: u32) {
    let mut c = Canvas::new(buf, w, h);
    app.render(&mut c);
}

/// Type `n` characters, rendering after each one like the window does, plus
/// one 8 ms slice of background work (the window's tick budget).
fn keystrokes(app: &mut App, n: usize, buf: &mut [u32], w: u32, h: u32) -> Stats {
    let mut times = Vec::with_capacity(n);
    for i in 0..n {
        let t = Instant::now();
        app.handle(Input::Text(((b'a' + (i % 26) as u8) as char).to_string()));
        frame(app, buf, w, h);
        times.push(ms(t.elapsed()));
        app.tick(Duration::from_millis(8));
    }
    stats(times)
}

pub fn run(out: &Path, mut app: App) -> std::io::Result<()> {
    let (w, h) = (1280u32, 800u32);
    let mut buf = vec![0u32; (w * h) as usize];
    app.handle(Input::Resize(w, h));
    let dir = std::env::temp_dir().join(format!("litedit-bench-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let mut rows: Vec<(String, String)> = Vec::new();

    // 1. A 100k-line source file.
    let big = dir.join("big.rs");
    let text: String = (0..100_000)
        .map(|i| format!("fn f{i}(x: i32) -> i32 {{ let s = \"text {i}\"; x + {i} }} // note\n"))
        .collect();
    std::fs::write(&big, &text)?;
    let t = Instant::now();
    app.open(&big);
    frame(&mut app, &mut buf, w, h);
    rows.push((
        "open 100k-line file + first frame".into(),
        format!(
            "{:.0} ms ({:.1} MB)",
            ms(t.elapsed()),
            text.len() as f64 / 1e6
        ),
    ));
    key(&mut app, "ctrl+g");
    app.handle(Input::Text("50000".into()));
    key(&mut app, "enter");
    frame(&mut app, &mut buf, w, h);
    let s = keystrokes(&mut app, 60, &mut buf, w, h);
    rows.push((
        "keystroke → frame, 100k lines".into(),
        format!("avg {:.1} / p95 {:.1} / max {:.1} ms", s.avg, s.p95, s.max),
    ));

    // 2. Same with the find bar open (every visible line is matched).
    key(&mut app, "ctrl+f");
    app.handle(Input::Text("text".into()));
    key(&mut app, "escape"); // close the bar but keep… reopen to keep highlights
    key(&mut app, "ctrl+f");
    key(&mut app, "tab"); // no replace row: stays on find; typing goes to the bar
    key(&mut app, "escape");
    key(&mut app, "ctrl+f");
    // Focus back to the editor with a click so typing edits the document
    // while the bar (and its highlights + count) stays open.
    app.handle(Input::MouseDown {
        x: 600.0,
        y: 300.0,
        button: crate::input::MouseButton::Left,
        clicks: 1,
    });
    app.handle(Input::MouseUp {
        x: 600.0,
        y: 300.0,
        button: crate::input::MouseButton::Left,
    });
    let s = keystrokes(&mut app, 40, &mut buf, w, h);
    rows.push((
        "keystroke → frame, find bar open".into(),
        format!("avg {:.1} / p95 {:.1} / max {:.1} ms", s.avg, s.p95, s.max),
    ));
    key(&mut app, "escape");

    // 3. Wrapped markdown, 50k lines of prose.
    let md = dir.join("big.md");
    let prose: String = (0..50_000).map(|i| format!("Paragraph {i} with enough words to wrap around the right edge of the editor at least once.\n")).collect();
    std::fs::write(&md, &prose)?;
    app.open(&md);
    let t = Instant::now();
    frame(&mut app, &mut buf, w, h);
    rows.push((
        "open 50k-line markdown preview, first frame".into(),
        format!("{:.0} ms", ms(t.elapsed())),
    ));
    key(&mut app, "ctrl+e");
    frame(&mut app, &mut buf, w, h);
    let s = keystrokes(&mut app, 40, &mut buf, w, h);
    rows.push((
        "keystroke → frame, wrapped markdown source".into(),
        format!("avg {:.1} / p95 {:.1} / max {:.1} ms", s.avg, s.p95, s.max),
    ));

    // 4. Quick open index and find in folder on a large folder.
    let tree = dir.join("tree");
    let n_files = 25_000;
    for i in 0..n_files {
        let d = tree.join(format!("d{}", i / 500));
        if i % 500 == 0 {
            std::fs::create_dir_all(&d)?;
        }
        std::fs::write(
            d.join(format!("file{i}.txt")),
            if i % 1000 == 0 {
                "the needle is here\n"
            } else {
                "nothing\n"
            },
        )?;
    }
    let t = Instant::now();
    let idx = crate::palette::index_files(&tree);
    rows.push((
        format!("Ctrl+P index of {n_files} files"),
        format!(
            "{:.0} ms, {} listed{}",
            ms(t.elapsed()),
            idx.len(),
            if idx.len() < n_files { " (capped)" } else { "" }
        ),
    ));
    let t = Instant::now();
    let mut search = crate::view::search::SearchView::start(&tree, "needle");
    let mut dummy_fonts = crate::text::Fonts::load();
    let (mut reqs, mut cur, mut clip) = (
        Vec::new(),
        crate::input::Cursor::Default,
        crate::clipboard::MemClipboard::default(),
    );
    loop {
        let mut ctx = crate::ui::Ctx {
            fonts: &mut dummy_fonts,
            scale: 1.0,
            zoom: 1.0,
            requests: &mut reqs,
            cursor: &mut cur,
            focused: true,
            clipboard: &mut clip,
            mods: crate::input::Mods::default(),
        };
        use crate::view::View;
        if !search.tick(&mut ctx, Duration::from_millis(8)).more
            || t.elapsed() > Duration::from_secs(60)
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    rows.push((
        format!("find in folder over {n_files} files"),
        format!("{:.0} ms, {} hits", ms(t.elapsed()), search.hit_count()),
    ));

    let _ = std::fs::remove_dir_all(&dir);
    let mut md_out =
        String::from("\n### Performance (litedit --bench)\n| measurement | result |\n|---|---|\n");
    for (k, v) in rows {
        md_out.push_str(&format!("| {k} | {v} |\n"));
    }
    print!("{md_out}");
    std::fs::write(out, md_out)
}
