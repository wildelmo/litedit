//! Scenario tests: drive the whole app through `Input` exactly like the
//! window does, and render headlessly.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::app::App;
use crate::canvas::Canvas;
use crate::input::{Input, Key, Mods, Request, parse_key_combo};
use crate::text::Fonts;

pub fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

pub fn fixtures() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(fixture(""))
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file())
        .collect();
    v.sort();
    v
}

pub fn new_app() -> App {
    let mut app = App::new(Fonts::load(), 1.0);
    app.handle(Input::Resize(800, 600));
    app
}

pub fn render_hash(app: &mut App, w: u32, h: u32) -> u64 {
    let mut buf = vec![0u32; (w * h) as usize];
    {
        let mut c = Canvas::new(&mut buf, w, h);
        app.render(&mut c);
    }
    let mut hasher = DefaultHasher::new();
    buf.hash(&mut hasher);
    hasher.finish()
}

pub fn key(app: &mut App, combo: &str) -> bool {
    let (key, mods) = parse_key_combo(combo).unwrap();
    app.handle(Input::Key { key, mods })
}

#[test]
fn every_fixture_renders_at_any_size() {
    let mut app = new_app();
    for f in fixtures() {
        app.open(&f);
        for (w, h) in [(1, 1), (40, 40), (300, 90), (800, 600)] {
            app.handle(Input::Resize(w, h));
            app.settle(Duration::from_secs(5));
            render_hash(&mut app, w, h);
        }
    }
}

#[test]
fn paging_scrolls_long_files() {
    let dir = std::env::temp_dir().join(format!("litedit-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("long.txt");
    let text: String = (0..5000).map(|i| format!("line {i}\n")).collect();
    std::fs::write(&path, text).unwrap();

    let mut app = new_app();
    app.open(&path);
    let a = render_hash(&mut app, 800, 600);
    assert!(key(&mut app, "pagedown"));
    let b = render_hash(&mut app, 800, 600);
    assert_ne!(a, b, "page down should change the view");
    assert!(key(&mut app, "ctrl+end"));
    render_hash(&mut app, 800, 600);
    assert!(key(&mut app, "ctrl+home"));
    assert_eq!(
        render_hash(&mut app, 800, 600),
        a,
        "ctrl+home returns to the top"
    );
    // Already at the top: nothing to redraw.
    assert!(!key(&mut app, "up"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn long_lines_scroll_horizontally() {
    let dir = std::env::temp_dir().join(format!("litedit-hscroll-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("wide.txt");
    std::fs::write(&path, format!("{}\n", "abcdefghij".repeat(2000))).unwrap();
    let mut app = new_app();
    app.open(&path);
    let a = render_hash(&mut app, 800, 600);
    app.handle(Input::MouseMove { x: 600.0, y: 300.0 }); // over the main view, not the tree
    assert!(app.handle(Input::Wheel {
        dx: 5.0,
        dy: 0.0,
        unit: crate::input::WheelUnit::Lines
    }));
    assert_ne!(a, render_hash(&mut app, 800, 600));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn ctrl_o_requests_dialogs() {
    let mut app = new_app();
    key(&mut app, "ctrl+o");
    key(&mut app, "ctrl+shift+o");
    assert_eq!(
        app.take_requests(),
        vec![Request::OpenFileDialog, Request::OpenFolderDialog]
    );
}

#[test]
fn zoom_changes_rendering_and_resets() {
    let mut app = new_app();
    app.open(&fixture("sample.rs"));
    let a = render_hash(&mut app, 800, 600);
    key(&mut app, "ctrl+plus");
    assert_ne!(a, render_hash(&mut app, 800, 600));
    key(&mut app, "ctrl+0");
    assert_eq!(a, render_hash(&mut app, 800, 600));
}

#[test]
fn drop_opens_file_and_sets_title() {
    let mut app = new_app();
    app.handle(Input::Drop(fixture("sample.rs")));
    assert!(app.title().starts_with("sample.rs"));
}

#[test]
fn missing_file_shows_error_not_panic() {
    let mut app = new_app();
    app.open(&fixture("nope-does-not-exist.txt"));
    assert_eq!(app.title(), "Error - litedit");
    render_hash(&mut app, 400, 300);
}

#[test]
fn external_change_reloads_open_file() {
    let dir = std::env::temp_dir().join(format!("litedit-reload-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a.txt");
    std::fs::write(&path, "one\n").unwrap();
    let mut app = new_app();
    app.open(&path);
    assert!(app.status_text().starts_with("1 lines"));
    std::fs::write(&path, "one\ntwo\nthree\n").unwrap();
    assert!(app.handle(Input::FsChanged(vec![path.clone()])));
    assert!(app.status_text().starts_with("3 lines"));
    // Unrelated paths are ignored.
    assert!(!app.handle(Input::FsChanged(vec![dir.join("other.txt")])));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn script_mode_runs_smoke_script() {
    let mut app = new_app();
    let out = std::env::temp_dir().join(format!("litedit-script-{}", std::process::id()));
    let script = format!(
        "size 640 480\nopen {}\nexpect title sample.rs\nkey pagedown\nkey ctrl+o\nexpect request OpenFileDialog\nshot {}\n",
        fixture("sample.rs").display(),
        out.join("s.png").display()
    );
    crate::script::run(&mut app, &script).unwrap();
    assert!(out.join("s.png").is_file());
    assert!(crate::script::run(&mut app, "expect title nothing-like-this").is_err());
    assert!(crate::script::run(&mut app, "bogus").is_err());
    std::fs::remove_dir_all(&out).ok();
}

#[test]
fn plain_keys_without_ctrl_do_not_trigger_shortcuts() {
    let mut app = new_app();
    app.handle(Input::Key {
        key: Key::Char('o'),
        mods: Mods::default(),
    });
    assert!(app.take_requests().is_empty());
}

#[test]
fn highlighting_big_files_is_budgeted() {
    let dir = std::env::temp_dir().join(format!("litedit-hl-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("big.rs");
    let text: String = (0..60_000)
        .map(|i| format!("fn f{i}() -> i32 {{ let s = \"x\"; {i} }} // c\n"))
        .collect();
    std::fs::write(&path, text).unwrap();
    let mut app = new_app();
    app.open(&path);
    render_hash(&mut app, 800, 600);
    let t = std::time::Instant::now();
    let tick = app.tick(Duration::from_millis(5));
    assert!(
        t.elapsed() < Duration::from_millis(250),
        "tick took {:?}",
        t.elapsed()
    );
    assert!(tick.more, "60k lines cannot finish in 5ms");
    assert!(tick.redraw, "first lines are visible and just got colours");
    // Jump to the end: unhighlighted text renders fine meanwhile.
    key(&mut app, "ctrl+end");
    render_hash(&mut app, 800, 600);
    std::fs::remove_dir_all(&dir).ok();
}

/// Deterministic pseudo-random bytes (no extra crates).
fn noise(seed: u64, len: usize) -> Vec<u8> {
    let mut x = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect()
}

fn hostile_inputs() -> Vec<(&'static str, Vec<u8>)> {
    let mut v: Vec<(&'static str, Vec<u8>)> = vec![
        ("deep-quotes.md", (">".repeat(2000) + " x\n").into_bytes()),
        ("deep-lists.md", (0..400).map(|i| format!("{}- item\n", "  ".repeat(i))).collect::<String>().into_bytes()),
        ("deep-emphasis.md", ("*_".repeat(3000) + "x" + &"_*".repeat(3000)).into_bytes()),
        (
            "wide-table.md",
            {
                let head = (0..300).map(|i| format!("h{i}")).collect::<Vec<_>>().join(" | ");
                let sep = vec!["---"; 300].join(" | ");
                let row = vec!["cell"; 300].join(" | ");
                format!("| {head} |\n| {sep} |\n| {row} |\n").into_bytes()
            },
        ),
        ("long-line.md", "word ".repeat(200_000).into_bytes()),
        ("long-line.rs", format!("let s = \"{}\";\n", "x".repeat(1_000_000)).into_bytes()),
        ("unterminated-fence.md", ("```rust\n".to_string() + &"fn x() {}\n".repeat(20_000)).into_bytes()),
        ("nul-bytes.md", b"# a\0b\n\n\0\0\0 text \0\n".to_vec()),
        ("unicode.md", "# 標題 🚀 مرحبا\n\nZ\u{0351}\u{0352}\u{0353}algo \u{202e}RTL override\u{202c} 👨\u{200d}👩\u{200d}👧\u{200d}👦\n".as_bytes().to_vec()),
        (
            "weird-images.md",
            b"![a](\\\\\\\\server\\\\x.png)\n\n![b](//server/x.png)\n\n![c](con.png)\n\n![d](NUL)\n\n![e](../../../../../../etc/passwd)\n\n![f](data:image/png;base64,AAAA)\n\n![g]()\n".to_vec(),
        ),
        ("many-links.md", (0..5000).map(|i| format!("[l{i}](#a{i}) ")).collect::<String>().into_bytes()),
        ("html-soup.md", "<div><p><b>x<i>y</b>z</p><script>alert(1)<!-- <img src=x onerror=y> -->\n\n<table><tr><td>a".repeat(200).into_bytes()),
    ];
    for i in 0..6 {
        v.push(("noise.md", noise(i, 4000)));
        v.push(("noise.rs", noise(i + 100, 4000)));
    }
    v
}

#[test]
fn hostile_files_never_panic() {
    let dir = std::env::temp_dir().join(format!("litedit-hostile-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = new_app();
    for (i, (name, bytes)) in hostile_inputs().into_iter().enumerate() {
        let path = dir.join(format!("{i}-{name}"));
        std::fs::write(&path, &bytes).unwrap();
        app.open(&path);
        for (w, h) in [(1, 1), (40, 40), (320, 200), (1280, 800)] {
            app.handle(Input::Resize(w, h));
            app.settle(Duration::from_secs(3));
            render_hash(&mut app, w, h);
        }
        key(&mut app, "ctrl+end");
        key(&mut app, "ctrl+e");
        render_hash(&mut app, 640, 480);
        key(&mut app, "ctrl+e");
        render_hash(&mut app, 640, 480);
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn every_prefix_of_sample_markdown_renders() {
    let src = std::fs::read_to_string(fixture("sample.md")).unwrap();
    let dir = std::env::temp_dir().join(format!("litedit-prefix-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("p.md");
    let mut app = new_app();
    let cuts: Vec<usize> = src.char_indices().map(|(i, _)| i).step_by(9).collect();
    for cut in cuts {
        std::fs::write(&path, &src[..cut]).unwrap();
        app.open(&path);
        render_hash(&mut app, 300, 200);
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn folder_tree_click_opens_files_and_keyboard_navigates() {
    let dir = std::env::temp_dir().join(format!("litedit-treeapp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("notes")).unwrap();
    std::fs::write(dir.join("notes/todo.md"), "# Todo\n\n- [ ] thing\n").unwrap();
    std::fs::write(dir.join("readme.txt"), "hello\n").unwrap();
    let mut app = new_app();
    app.open(&dir);
    assert_eq!(app.focus(), crate::app::Focus::Tree);
    render_hash(&mut app, 800, 600);
    // Rows: [root, notes/, readme.txt]. Keyboard: down to notes, expand, down to todo.md, open.
    for k in ["down", "right", "down", "enter"] {
        key(&mut app, k);
    }
    assert!(app.title().starts_with("todo.md"), "got {}", app.title());
    assert_eq!(app.focus(), crate::app::Focus::Main);
    assert!(app.status_text().contains("Markdown preview"));
    // Ctrl+\ hides the tree; the main view gets the full width.
    let with_tree = render_hash(&mut app, 800, 600);
    key(&mut app, "ctrl+\\");
    assert_ne!(with_tree, render_hash(&mut app, 800, 600));
    key(&mut app, "ctrl+\\");
    // Mouse: click the readme row (third row) in the tree.
    let rh = (15.0f32 * 1.4).round() + 6.0;
    let y = 7.0 + rh * 2.0 + rh / 2.0;
    app.handle(Input::MouseMove { x: 60.0, y });
    app.handle(Input::MouseDown {
        x: 60.0,
        y,
        button: crate::input::MouseButton::Left,
        clicks: 1,
    });
    // todo.md is expanded under notes, so row 3 is todo.md, row 4 readme.
    let y4 = 7.0 + rh * 3.0 + rh / 2.0;
    app.handle(Input::MouseDown {
        x: 60.0,
        y: y4,
        button: crate::input::MouseButton::Left,
        clicks: 1,
    });
    assert!(app.title().starts_with("readme.txt"), "got {}", app.title());
    // External change: a new file appears in the tree.
    std::fs::write(dir.join("new.md"), "x").unwrap();
    assert!(app.handle(Input::FsChanged(vec![dir.join("new.md")])));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn dragging_the_divider_resizes_the_tree() {
    let mut app = new_app();
    app.open(&fixture("sample.rs")); // opens the fixtures folder in the tree
    let before = render_hash(&mut app, 800, 600);
    let x0 = 260.0;
    app.handle(Input::MouseMove { x: x0, y: 200.0 });
    assert_eq!(app.cursor(), crate::input::Cursor::ResizeHorizontal);
    app.handle(Input::MouseDown {
        x: x0,
        y: 200.0,
        button: crate::input::MouseButton::Left,
        clicks: 1,
    });
    app.handle(Input::MouseMove { x: 380.0, y: 200.0 });
    app.handle(Input::MouseUp {
        x: 380.0,
        y: 200.0,
        button: crate::input::MouseButton::Left,
    });
    assert_ne!(before, render_hash(&mut app, 800, 600));
}
