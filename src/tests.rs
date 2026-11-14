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
