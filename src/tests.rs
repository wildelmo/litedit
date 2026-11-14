//! Scenario tests: drive the whole app through `Input` exactly like the
//! window does, and render headlessly.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::app::App;
use crate::canvas::Canvas;
use crate::input::{Input, Key, Mods, Request, parse_key_combo};
use crate::session::Session;
use crate::text::Fonts;
use crate::{settings, theme};

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
    assert!(
        app.status_text().contains(" 2 lines"),
        "{}",
        app.status_text()
    );
    std::fs::write(&path, "one\ntwo\nthree\n").unwrap();
    assert!(app.handle(Input::FsChanged(vec![path.clone()])));
    assert!(
        app.status_text().contains(" 4 lines"),
        "{}",
        app.status_text()
    );
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

fn temp_file(name: &str, text: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("litedit-ed-{}-{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(name);
    std::fs::write(&p, text).unwrap();
    p
}

fn type_text(app: &mut App, s: &str) {
    for c in s.chars() {
        app.handle(Input::Text(c.to_string()));
    }
}

#[test]
fn typing_moves_and_undo_through_the_app() {
    let p = temp_file("edit.txt", "hello\nworld\n");
    let mut app = new_app();
    app.open(&p);
    key(&mut app, "end");
    type_text(&mut app, ", there");
    key(&mut app, "enter");
    type_text(&mut app, "new line");
    assert!(
        app.title().starts_with("● edit.txt"),
        "dirty marker: {}",
        app.title()
    );
    assert!(
        app.status_text().starts_with("Ln 2, Col 9"),
        "{}",
        app.status_text()
    );
    render_hash(&mut app, 800, 600);
    // Select the word "world" by keyboard and replace it.
    key(&mut app, "down");
    key(&mut app, "home");
    key(&mut app, "ctrl+shift+right");
    type_text(&mut app, "planet");
    key(&mut app, "ctrl+z");
    key(&mut app, "ctrl+z");
    key(&mut app, "ctrl+z");
    key(&mut app, "ctrl+z");
    render_hash(&mut app, 800, 600);
    std::fs::remove_dir_all(p.parent().unwrap()).ok();
}

#[test]
fn mouse_places_caret_and_selects_words() {
    let p = temp_file("mouse.rs", "fn alpha() {}\nlet beta = 1;\n");
    let mut app = new_app();
    app.handle(Input::Key {
        key: Key::Char('\\'),
        mods: Mods {
            ctrl: true,
            ..Mods::default()
        },
    }); // hide tree
    app.open(&p);
    key(&mut app, "ctrl+\\");
    render_hash(&mut app, 800, 600);
    // Click far right of line 2 → caret at its end.
    let lh = (15.0f32 * 1.4).round();
    let tabs = lh + 14.0; // tab bar height
    let y2 = tabs + 7.0 + lh * 1.5;
    app.handle(Input::MouseMove { x: 700.0, y: y2 });
    app.handle(Input::MouseDown {
        x: 700.0,
        y: y2,
        button: crate::input::MouseButton::Left,
        clicks: 1,
    });
    app.handle(Input::MouseUp {
        x: 700.0,
        y: y2,
        button: crate::input::MouseButton::Left,
    });
    assert!(
        app.status_text().starts_with("Ln 2, Col 14"),
        "{}",
        app.status_text()
    );
    assert_eq!(app.cursor(), crate::input::Cursor::Text);
    // Double-click a word on line 1 and type over it.
    let y1 = tabs + 7.0 + lh * 0.5;
    let gutter_and_some = 80.0;
    app.handle(Input::MouseDown {
        x: gutter_and_some,
        y: y1,
        button: crate::input::MouseButton::Left,
        clicks: 2,
    });
    type_text(&mut app, "X");
    let text = std::fs::read_to_string(&p).unwrap();
    assert_eq!(text, "fn alpha() {}\nlet beta = 1;\n", "nothing saved yet");
    render_hash(&mut app, 800, 600);
    // Drag-select from start of line 1 to line 2.
    app.handle(Input::MouseDown {
        x: 60.0,
        y: y1,
        button: crate::input::MouseButton::Left,
        clicks: 1,
    });
    app.handle(Input::MouseMove { x: 300.0, y: y2 });
    app.handle(Input::MouseUp {
        x: 300.0,
        y: y2,
        button: crate::input::MouseButton::Left,
    });
    render_hash(&mut app, 800, 600);
    std::fs::remove_dir_all(p.parent().unwrap()).ok();
}

#[test]
fn copy_cut_paste_through_the_app() {
    let p = temp_file("clip.txt", "alpha\nbeta\n");
    let mut app = new_app();
    app.open(&p);
    // Line-mode copy (no selection) then paste above line 2.
    key(&mut app, "ctrl+c");
    assert_eq!(app.clipboard_text().as_deref(), Some("alpha\n"));
    key(&mut app, "down");
    key(&mut app, "ctrl+v");
    // The caret stayed on "beta" (now line 3): select it and cut.
    assert!(
        app.status_text().starts_with("Ln 3, Col 1"),
        "{}",
        app.status_text()
    );
    key(&mut app, "shift+end");
    key(&mut app, "ctrl+x");
    assert_eq!(app.clipboard_text().as_deref(), Some("beta"));
    key(&mut app, "ctrl+home");
    key(&mut app, "ctrl+v");
    assert!(
        app.status_text().starts_with("Ln 1, Col 5"),
        "{}",
        app.status_text()
    );
    std::fs::remove_dir_all(p.parent().unwrap()).ok();
}

fn press(app: &mut App, combo: &str) {
    key(app, combo);
}

#[test]
fn save_preserves_endings_and_ignores_own_change_event() {
    let p = temp_file("crlf.txt", "one\r\ntwo\r\n");
    let mut app = new_app();
    app.open(&p);
    press(&mut app, "end");
    type_text(&mut app, "!");
    press(&mut app, "enter");
    type_text(&mut app, "new");
    assert!(app.title().starts_with('●'));
    press(&mut app, "ctrl+s");
    assert_eq!(std::fs::read(&p).unwrap(), b"one!\r\nnew\r\ntwo\r\n");
    assert!(app.title().starts_with("crlf.txt"), "{}", app.title());
    assert!(app.status_text().contains("Saved crlf.txt"));
    // The watcher reports our own save: no prompt, no reload.
    app.handle(Input::FsChanged(vec![p.clone()]));
    assert_eq!(app.nag_message(), None);
    std::fs::remove_dir_all(p.parent().unwrap()).ok();
}

#[test]
fn tabs_open_without_discarding_and_closing_dirty_tabs_asks() {
    let p = temp_file("dirty.txt", "text\n");
    let other = p.parent().unwrap().join("other.txt");
    std::fs::write(&other, "other\n").unwrap();
    let mut app = new_app();
    app.open(&p);
    type_text(&mut app, "edit ");

    // Opening another file (dialog, tree, link, drop) adds a tab: nothing is lost.
    app.handle(Input::Drop(other.clone()));
    assert_eq!(app.nag_message(), None);
    assert_eq!(app.tab_count(), 2);
    assert!(app.title().starts_with("other.txt"));
    // Re-opening an open file just switches to it.
    app.open(&p);
    assert_eq!(app.tab_count(), 2);
    assert!(app.title().starts_with("● dirty.txt"));

    // Ctrl+W on the dirty tab asks; Cancel keeps it.
    press(&mut app, "ctrl+w");
    assert!(
        app.nag_message()
            .unwrap()
            .contains("Save changes to dirty.txt")
    );
    press(&mut app, "escape");
    assert_eq!(app.tab_count(), 2);

    // Ctrl+Tab cycles; closing a clean tab does not ask.
    press(&mut app, "ctrl+tab");
    assert!(app.title().starts_with("other.txt"));
    press(&mut app, "ctrl+w");
    assert_eq!(app.nag_message(), None);
    assert_eq!(app.tab_count(), 1);
    assert!(app.title().starts_with("● dirty.txt"));

    // Closing the window asks about the dirty tab; Don't save quits and
    // leaves the file on disk untouched.
    app.handle(Input::CloseRequested);
    assert!(app.nag_message().is_some());
    assert!(!app.take_requests().contains(&Request::Quit));
    press(&mut app, "d");
    assert!(app.take_requests().contains(&Request::Quit));
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "text\n");
    std::fs::remove_dir_all(p.parent().unwrap()).ok();
}

#[test]
fn quit_asks_about_each_dirty_tab() {
    let a = temp_file("a.txt", "a");
    let b = a.parent().unwrap().join("b.txt");
    std::fs::write(&b, "b").unwrap();
    let mut app = new_app();
    app.open(&a);
    type_text(&mut app, "1");
    app.open(&b);
    type_text(&mut app, "2");
    app.handle(Input::CloseRequested);
    assert!(
        app.nag_message().unwrap().contains("a.txt")
            || app.nag_message().unwrap().contains("b.txt")
    );
    press(&mut app, "enter"); // save the first
    assert!(app.nag_message().is_some(), "asks about the second");
    assert!(!app.take_requests().contains(&Request::Quit));
    press(&mut app, "d"); // discard the second
    assert!(app.take_requests().contains(&Request::Quit));
    let saved = [
        std::fs::read_to_string(&a).unwrap(),
        std::fs::read_to_string(&b).unwrap(),
    ];
    assert!(
        saved.contains(&"1a".to_string()) || saved.contains(&"2b".to_string()),
        "{saved:?}"
    );
    std::fs::remove_dir_all(a.parent().unwrap()).ok();
}

#[test]
fn tab_bar_click_switches_and_middle_click_closes() {
    let a = temp_file("one.txt", "1");
    let b = a.parent().unwrap().join("two.txt");
    std::fs::write(&b, "2").unwrap();
    let mut app = new_app();
    app.open(&a);
    app.open(&b);
    key(&mut app, "ctrl+\\"); // hide tree: tab bar starts at x = 0
    render_hash(&mut app, 800, 600);
    app.handle(Input::MouseDown {
        x: 20.0,
        y: 10.0,
        button: crate::input::MouseButton::Left,
        clicks: 1,
    });
    assert!(app.title().starts_with("one.txt"), "{}", app.title());
    app.handle(Input::MouseDown {
        x: 200.0,
        y: 10.0,
        button: crate::input::MouseButton::Middle,
        clicks: 1,
    });
    assert_eq!(app.tab_count(), 1);
    assert!(app.title().starts_with("one.txt"));
    std::fs::remove_dir_all(a.parent().unwrap()).ok();
}

#[test]
fn save_then_quit_from_the_close_prompt() {
    let p = temp_file("quit.txt", "a");
    let mut app = new_app();
    app.open(&p);
    press(&mut app, "end");
    type_text(&mut app, "b");
    app.handle(Input::CloseRequested);
    press(&mut app, "enter"); // Save (default)
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "ab");
    assert!(app.take_requests().contains(&Request::Quit));
    std::fs::remove_dir_all(p.parent().unwrap()).ok();
}

#[test]
fn external_change_to_dirty_file_prompts_and_can_reload() {
    let p = temp_file("ext.txt", "mine\n");
    let mut app = new_app();
    app.open(&p);
    type_text(&mut app, "x");
    std::fs::write(&p, "theirs\n").unwrap();
    app.handle(Input::FsChanged(vec![p.clone()]));
    assert!(app.nag_message().unwrap().contains("changed on disk"));
    press(&mut app, "r"); // Reload from disk
    assert!(!app.title().starts_with('●'));
    press(&mut app, "ctrl+s");
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "theirs\n");
    // Deleted on disk: content kept, marked modified, saving restores it.
    std::fs::remove_file(&p).unwrap();
    app.handle(Input::FsChanged(vec![p.clone()]));
    assert!(app.title().starts_with('●'));
    assert!(app.status_text().contains("deleted on disk"));
    press(&mut app, "ctrl+s");
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "theirs\n");
    std::fs::remove_dir_all(p.parent().unwrap()).ok();
}

#[test]
fn lossy_files_need_confirmation_and_failures_keep_edits() {
    let dir = std::env::temp_dir().join(format!("litedit-lossy-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("bad.txt");
    std::fs::write(&p, b"ok \xFF\xFE end").unwrap();
    let mut app = new_app();
    app.open(&p);
    press(&mut app, "end");
    type_text(&mut app, "!");
    press(&mut app, "ctrl+s");
    assert!(app.nag_message().unwrap().contains("invalid characters"));
    assert_eq!(
        std::fs::read(&p).unwrap(),
        b"ok \xFF\xFE end",
        "not written yet"
    );
    press(&mut app, "s"); // Save anyway
    assert_eq!(
        std::fs::read_to_string(&p).unwrap(),
        "ok \u{fffd}\u{fffd} end!"
    );

    // A failing save (read-only) shows an error and keeps the edit.
    type_text(&mut app, "?");
    let mut perm = std::fs::metadata(&p).unwrap().permissions();
    perm.set_readonly(true);
    std::fs::set_permissions(&p, perm.clone()).unwrap();
    press(&mut app, "ctrl+s");
    assert!(app.nag_message().unwrap().contains("read-only"));
    assert!(app.title().starts_with('●'));
    press(&mut app, "enter");
    #[allow(clippy::permissions_set_readonly_false)]
    perm.set_readonly(false);
    std::fs::set_permissions(&p, perm).unwrap();
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn save_as_switches_file_and_script_expect_file_works() {
    let p = temp_file("orig.txt", "hi\n");
    let copy = p.parent().unwrap().join("copy.txt");
    let mut app = new_app();
    let script = format!(
        "open {}\nkey end\ntext !\nsaveas {}\nexpect title copy.txt\nexpect file {} hi!\\n\nexpect file {} hi\\n\nexpect no-nag\n",
        p.display(),
        copy.display(),
        copy.display(),
        p.display()
    );
    crate::script::run(&mut app, &script).unwrap();
    assert!(!app.title().starts_with('●'));
    assert!(
        crate::script::run(&mut app, &format!("expect file {} wrong", copy.display())).is_err()
    );
    std::fs::remove_dir_all(p.parent().unwrap()).ok();
}

#[test]
fn markdown_edit_updates_preview_and_links_open_in_tabs() {
    let p = temp_file("notes.md", "[go](other.md)\n\n# Title\n");
    std::fs::write(p.parent().unwrap().join("other.md"), "# Other\n").unwrap();
    let mut app = new_app();
    app.open(&p);
    press(&mut app, "ctrl+\\");
    let before = render_hash(&mut app, 800, 600);
    // Edit the source: add a word to the heading.
    press(&mut app, "ctrl+e");
    assert!(app.status_text().starts_with("Markdown source"));
    press(&mut app, "ctrl+end");
    press(&mut app, "up");
    press(&mut app, "end");
    type_text(&mut app, " edited");
    press(&mut app, "ctrl+e");
    assert!(app.status_text().starts_with("Markdown preview"));
    assert!(app.title().starts_with("● notes.md"), "{}", app.title());
    assert_ne!(
        before,
        render_hash(&mut app, 800, 600),
        "preview follows the edit"
    );
    // Following a link opens it in a new tab; the edited file stays open.
    let y = 40.0 + 35.0; // below the tab bar
    app.handle(Input::MouseMove { x: 50.0, y });
    app.handle(Input::MouseDown {
        x: 50.0,
        y,
        button: crate::input::MouseButton::Left,
        clicks: 1,
    });
    assert_eq!(app.nag_message(), None);
    assert!(app.title().starts_with("other.md"), "{}", app.title());
    assert_eq!(app.tab_count(), 2);
    press(&mut app, "ctrl+shift+tab");
    press(&mut app, "ctrl+s");
    assert_eq!(
        std::fs::read_to_string(&p).unwrap(),
        "[go](other.md)\n\n# Title edited\n"
    );
    std::fs::remove_dir_all(p.parent().unwrap()).ok();
}

/// App-level fuzz: random typing, keys, clicks and drags through the real
/// input path. After every step: render, carets on grapheme boundaries.
/// Finally undo everything and compare the exact original bytes.
#[test]
fn fuzz_editor_through_the_app() {
    use crate::editor::doc::snap_grapheme;
    let keys = [
        "left",
        "right",
        "up",
        "down",
        "home",
        "end",
        "ctrl+left",
        "ctrl+right",
        "shift+left",
        "shift+right",
        "shift+down",
        "shift+end",
        "backspace",
        "delete",
        "ctrl+backspace",
        "ctrl+delete",
        "enter",
        "tab",
        "shift+tab",
        "ctrl+z",
        "ctrl+y",
        "ctrl+a",
        "ctrl+c",
        "ctrl+x",
        "ctrl+v",
        "ctrl+shift+d",
        "ctrl+shift+k",
        "pageup",
        "pagedown",
        "ctrl+home",
        "ctrl+end",
        "ctrl+l",
        "ctrl+]",
        "ctrl+[",
        "ctrl+e",
    ];
    let pieces = [
        "a",
        "é",
        "👨\u{200d}👩\u{200d}👧",
        "\t",
        " ",
        "日本",
        "x\ny",
        "\u{301}",
        "{",
        "}",
    ];
    let starts = [
        ("f.txt", "hello\r\nworld 👋🏽 tab\there\r\n\r\nend"),
        (
            "f.md",
            "# Title\n\n- item **bold**\n- 👨\u{200d}👩\u{200d}👧 family\n\n```rust\nfn x() {}\n```\n",
        ),
        ("f.rs", "fn main() {\n\tlet s = \"é\";\n}\n"),
    ];
    for seed in 0..9u64 {
        let (name, start) = starts[seed as usize % starts.len()];
        let p = temp_file(&format!("fz{seed}-{name}"), start);
        let mut app = new_app();
        app.open(&p);
        key(&mut app, "ctrl+\\"); // hide the tree so clicks land in the editor
        if name.ends_with(".md") {
            key(&mut app, "ctrl+e"); // edit the source
        }
        let mut x = seed.wrapping_mul(0x9E3779B97F4A7C15) | 1;
        let mut rnd = |n: usize| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x % n as u64) as usize
        };
        let movement = [
            "left",
            "right",
            "up",
            "down",
            "home",
            "end",
            "ctrl+left",
            "ctrl+right",
            "pageup",
            "pagedown",
            "ctrl+home",
            "ctrl+end",
        ];
        for _ in 0..220 {
            // Typing a base char before a combining mark legitimately leaves the
            // caret inside the new cluster (as in other editors); movement and
            // clicks must always land on a grapheme boundary.
            let mut moved = false;
            let mut last = String::new();
            match rnd(10) {
                0..=2 => {
                    app.handle(Input::Text(pieces[rnd(pieces.len())].to_string()));
                }
                3..=7 => {
                    let k = keys[rnd(keys.len())];
                    last = k.to_string();
                    moved = movement.contains(&k);
                    key(&mut app, k);
                }
                8 => {
                    moved = true;
                    last = "click".into();
                    let (cx, cy) = (rnd(500) as f32, 40.0 + rnd(260) as f32); // below the tab bar
                    let clicks = 1 + rnd(3) as u32;
                    app.handle(Input::MouseMove { x: cx, y: cy });
                    app.handle(Input::MouseDown {
                        x: cx,
                        y: cy,
                        button: crate::input::MouseButton::Left,
                        clicks,
                    });
                    let (dx, dy) = (rnd(500) as f32, 40.0 + rnd(260) as f32);
                    app.handle(Input::MouseMove { x: dx, y: dy });
                    app.handle(Input::MouseUp {
                        x: dx,
                        y: dy,
                        button: crate::input::MouseButton::Left,
                    });
                }
                _ => {
                    app.handle(Input::Wheel {
                        dx: 0.0,
                        dy: rnd(7) as f32 - 3.0,
                        unit: crate::input::WheelUnit::Lines,
                    });
                }
            }
            render_hash(&mut app, 500, 300);
            let preview = app.status_text().starts_with("Markdown preview");
            if let Some((text, pts)) = app.snapshot().filter(|_| !preview) {
                let lines: Vec<&str> = crate::editor::buffer::Chunk::parse(&text)
                    .lines
                    .iter()
                    .map(|_| "")
                    .collect();
                let chunk = crate::editor::buffer::Chunk::parse(&text);
                for p in pts {
                    assert!(p.line < lines.len(), "seed {seed}: caret line out of range");
                    let l = &chunk.lines[p.line];
                    assert!(
                        l.is_char_boundary(p.col),
                        "seed {seed}: caret inside a char"
                    );
                    if moved {
                        assert_eq!(
                            snap_grapheme(l, p.col),
                            p.col,
                            "seed {seed}: caret inside a grapheme in {l:?} after {last}"
                        );
                    }
                }
            }
        }
        assert!(app.nag_message().is_none());
        // Undo everything: the exact original bytes come back.
        if name.ends_with(".md") && app.status_text().starts_with("Markdown preview") {
            key(&mut app, "ctrl+e");
        }
        for _ in 0..3000 {
            key(&mut app, "ctrl+z");
        }
        let snap = app.snapshot().unwrap_or_else(|| {
            panic!(
                "seed {seed}: no editor; title {:?} status {:?}",
                app.title(),
                app.status_text()
            )
        });
        assert_eq!(
            snap.0, start,
            "seed {seed}: undo-all must restore the original"
        );
        std::fs::remove_dir_all(p.parent().unwrap()).ok();
    }
}

#[test]
fn find_and_replace_through_the_app() {
    let p = temp_file("find.txt", "alpha beta\nbeta gamma\nBETA\n");
    let mut app = new_app();
    app.open(&p);
    press(&mut app, "ctrl+f");
    type_text(&mut app, "beta");
    // Incremental search selected the first match.
    assert!(
        app.status_text().starts_with("Ln 1, Col 11"),
        "{}",
        app.status_text()
    );
    press(&mut app, "enter");
    assert!(
        app.status_text().starts_with("Ln 2, Col 5"),
        "{}",
        app.status_text()
    );
    press(&mut app, "enter");
    assert!(
        app.status_text().starts_with("Ln 3, Col 5"),
        "case-insensitive: {}",
        app.status_text()
    );
    press(&mut app, "shift+enter");
    assert!(app.status_text().starts_with("Ln 2, Col 5"));
    render_hash(&mut app, 800, 600);
    press(&mut app, "escape");
    // F3 still repeats the search with the bar closed.
    press(&mut app, "f3");
    assert!(app.status_text().starts_with("Ln 3, Col 5"));

    // Replace all is one undo step.
    press(&mut app, "ctrl+h");
    press(&mut app, "ctrl+a");
    type_text(&mut app, "beta");
    press(&mut app, "tab");
    type_text(&mut app, "B");
    press(&mut app, "ctrl+enter");
    render_hash(&mut app, 800, 600);
    press(&mut app, "escape");
    press(&mut app, "ctrl+s");
    assert_eq!(
        std::fs::read_to_string(&p).unwrap(),
        "alpha B\nB gamma\nB\n"
    );
    press(&mut app, "ctrl+z");
    press(&mut app, "ctrl+s");
    assert_eq!(
        std::fs::read_to_string(&p).unwrap(),
        "alpha beta\nbeta gamma\nBETA\n"
    );

    // Regex mode with groups, case-sensitive.
    press(&mut app, "ctrl+h");
    press(&mut app, "alt+r");
    press(&mut app, "alt+c");
    press(&mut app, "ctrl+a");
    type_text(&mut app, "(\\w+) (\\w+)");
    press(&mut app, "tab");
    press(&mut app, "ctrl+a");
    type_text(&mut app, "$2 $1");
    press(&mut app, "ctrl+enter");
    press(&mut app, "escape");
    press(&mut app, "ctrl+s");
    assert_eq!(
        std::fs::read_to_string(&p).unwrap(),
        "beta alpha\ngamma beta\nBETA\n"
    );
    std::fs::remove_dir_all(p.parent().unwrap()).ok();
}

#[test]
fn find_in_markdown_preview_switches_to_source() {
    let p = temp_file("f.md", "# Hello\n\nworld\n");
    let mut app = new_app();
    app.open(&p);
    press(&mut app, "ctrl+f");
    assert!(app.status_text().starts_with("Markdown source"));
    type_text(&mut app, "world");
    assert!(
        app.status_text().contains("Ln 3, Col 6"),
        "{}",
        app.status_text()
    );
    std::fs::remove_dir_all(p.parent().unwrap()).ok();
}

/// The CI smoke script must pass locally too (it is run on Windows CI).
#[test]
fn ci_smoke_script_passes() {
    let script =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/ci/smoke.script")).unwrap();
    std::env::set_current_dir(env!("CARGO_MANIFEST_DIR")).unwrap();
    let mut app = new_app();
    crate::script::run(&mut app, &script).unwrap();
}

#[test]
fn side_by_side_preview_follows_typing() {
    let p = temp_file("live.md", "# Title\n\nbody\n");
    let mut app = new_app();
    app.open(&p);
    press(&mut app, "ctrl+\\");
    press(&mut app, "ctrl+shift+v");
    assert!(
        app.status_text().starts_with("Markdown side by side"),
        "{}",
        app.status_text()
    );
    let before = render_hash(&mut app, 1000, 600);
    press(&mut app, "ctrl+end");
    type_text(&mut app, "**new bold text**");
    let after = render_hash(&mut app, 1000, 600);
    assert_ne!(before, after, "preview and editor both change");
    // Wheel over the preview half scrolls the preview, not the editor.
    app.handle(Input::MouseMove { x: 800.0, y: 300.0 });
    app.handle(Input::Wheel {
        dx: 0.0,
        dy: 3.0,
        unit: crate::input::WheelUnit::Lines,
    });
    render_hash(&mut app, 1000, 600);
    press(&mut app, "ctrl+shift+v");
    assert!(app.status_text().starts_with("Markdown source"));
    std::fs::remove_dir_all(p.parent().unwrap()).ok();
}

#[test]
fn palette_quick_open_and_goto_line() {
    let p = temp_file(
        "pal.txt",
        &(1..=50).map(|i| format!("line {i}\n")).collect::<String>(),
    );
    let dir = p.parent().unwrap().to_path_buf();
    std::fs::create_dir_all(dir.join("notes")).unwrap();
    std::fs::write(dir.join("notes/meeting-notes.md"), "# Meeting\n").unwrap();
    let mut app = new_app();
    app.open(&p);
    // Go to line.
    press(&mut app, "ctrl+g");
    type_text(&mut app, "42");
    press(&mut app, "enter");
    assert!(
        app.status_text().starts_with("Ln 42, Col 1"),
        "{}",
        app.status_text()
    );
    // Quick open with a fuzzy query.
    press(&mut app, "ctrl+p");
    type_text(&mut app, "mtgnotes");
    render_hash(&mut app, 900, 600);
    press(&mut app, "enter");
    assert!(
        app.title().starts_with("meeting-notes.md"),
        "{}",
        app.title()
    );
    // Command palette runs a command by replaying its shortcut.
    press(&mut app, "ctrl+shift+p");
    type_text(&mut app, "side by side");
    render_hash(&mut app, 900, 600);
    press(&mut app, "enter");
    assert!(
        app.status_text().starts_with("Markdown side by side"),
        "{}",
        app.status_text()
    );
    // Escape closes without running anything.
    press(&mut app, "ctrl+shift+p");
    press(&mut app, "escape");
    assert!(app.status_text().starts_with("Markdown side by side"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn tree_context_menu_new_rename_delete() {
    let dir = std::env::temp_dir().join(format!("litedit-ctx-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.md"), "# A\n").unwrap();
    let mut app = new_app();
    app.open(&dir);
    render_hash(&mut app, 900, 600);
    let rh = (15.0f32 * 1.4).round() + 6.0;
    let row_y = |i: f32| 7.0 + rh * i + rh / 2.0;
    let right_click = |app: &mut App, y: f32| {
        app.handle(Input::MouseMove { x: 60.0, y });
        app.handle(Input::MouseDown {
            x: 60.0,
            y,
            button: crate::input::MouseButton::Right,
            clicks: 1,
        });
    };
    // New file via the menu (first item) and the name prompt.
    right_click(&mut app, row_y(1.0)); // a.md
    render_hash(&mut app, 900, 600);
    press(&mut app, "down");
    press(&mut app, "enter");
    type_text(&mut app, "new note.md");
    press(&mut app, "enter");
    assert!(dir.join("new note.md").is_file());
    assert!(app.title().starts_with("new note.md"), "{}", app.title());
    // Invalid names are refused with a message.
    right_click(&mut app, row_y(1.0));
    press(&mut app, "down");
    press(&mut app, "enter");
    type_text(&mut app, "bad/name");
    press(&mut app, "enter");
    assert!(app.nag_message().unwrap().contains("cannot contain"));
    press(&mut app, "enter");
    // Rename a.md (row 1: a.md sorts before 'new note.md'); the open tab follows.
    app.open(&dir.join("a.md"));
    right_click(&mut app, row_y(1.0));
    for _ in 0..3 {
        press(&mut app, "down");
    }
    press(&mut app, "enter");
    press(&mut app, "ctrl+a");
    type_text(&mut app, "b.md");
    press(&mut app, "enter");
    assert!(dir.join("b.md").is_file() && !dir.join("a.md").exists());
    assert!(app.title().starts_with("b.md"), "{}", app.title());
    // Delete asks first; on non-Windows the Recycle Bin is unavailable, so
    // nothing is deleted and an error is shown.
    right_click(&mut app, row_y(1.0));
    for _ in 0..4 {
        press(&mut app, "down");
    }
    press(&mut app, "enter");
    assert!(app.nag_message().unwrap().contains("Recycle Bin"));
    press(&mut app, "enter");
    if cfg!(windows) {
        assert!(!dir.join("b.md").exists());
    } else {
        assert!(dir.join("b.md").exists());
        assert!(app.nag_message().unwrap().contains("Could not delete"));
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn find_in_folder_opens_results_at_line() {
    let dir = std::env::temp_dir().join(format!("litedit-fif-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("docs")).unwrap();
    std::fs::write(dir.join("docs/guide.md"), "intro\n\nthe Secret word\n").unwrap();
    std::fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();
    let mut app = new_app();
    app.open(&dir);
    press(&mut app, "ctrl+shift+f");
    type_text(&mut app, "secret");
    press(&mut app, "enter");
    assert!(app.title().starts_with("Search: secret"), "{}", app.title());
    app.settle(Duration::from_secs(5));
    render_hash(&mut app, 900, 600);
    assert!(
        app.status_text().starts_with("1 results"),
        "{}",
        app.status_text()
    );
    press(&mut app, "enter");
    assert!(app.title().starts_with("guide.md"), "{}", app.title());
    assert!(app.status_text().contains("Ln 3"), "{}", app.status_text());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn session_round_trip_and_recent() {
    let dir = std::env::temp_dir().join(format!("litedit-session-app-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("one.md"), "# one\n").unwrap();
    std::fs::write(dir.join("two.txt"), "two\n").unwrap();
    let mut app = new_app();
    app.open(&dir);
    app.open(&dir.join("one.md"));
    app.open(&dir.join("two.txt"));
    press(&mut app, "ctrl+plus");
    press(&mut app, "ctrl+shift+tab"); // back to one.md
    assert!(app.take_session_dirty());
    let s = app.session();
    assert_eq!(s.root.as_deref(), Some(dir.as_path()));
    assert_eq!(s.tabs.len(), 2);
    assert_eq!(s.active, 0);
    assert!(s.recent.iter().any(|p| p.ends_with("two.txt")));
    // A fresh app restores folder, tabs, active tab and zoom; missing files are skipped.
    let mut s2 = Session::parse(&s.to_text());
    s2.tabs.push(dir.join("gone.md"));
    let mut app2 = new_app();
    app2.restore(s2);
    assert_eq!(app2.tab_count(), 2);
    assert!(app2.title().starts_with("one.md"), "{}", app2.title());
    assert!(
        (app2.zoom() - app.zoom()).abs() < 0.01 && app2.zoom() > 1.0,
        "zoom restored"
    );
    // Ctrl+R lists recent items and opens one.
    press(&mut app2, "ctrl+r");
    type_text(&mut app2, "two");
    press(&mut app2, "enter");
    assert!(app2.title().starts_with("two.txt"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn starting_with_a_file_does_not_overwrite_the_saved_workspace() {
    let dir = std::env::temp_dir().join(format!("litedit-own-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for f in ["a.md", "b.md", "other.txt"] {
        std::fs::write(dir.join(f), f).unwrap();
    }
    let saved = Session {
        root: Some(dir.clone()),
        tabs: vec![dir.join("a.md"), dir.join("b.md")],
        active: 1,
        recent: vec![dir.join("a.md")],
        ..Session::default()
    };
    // Like double-clicking other.txt in Explorer: litedit other.txt
    let mut app = new_app();
    app.start_with_session(saved.clone(), false);
    app.open(&dir.join("other.txt"));
    let s = app.session();
    assert_eq!(s.root, saved.root, "folder kept");
    assert_eq!(s.tabs, saved.tabs, "tabs kept");
    assert_eq!(s.active, 1);
    assert_eq!(s.recent[0], dir.join("other.txt"), "recents still updated");
    assert!(s.recent.contains(&dir.join("a.md")));
    // Started without files: the workspace is restored and owned.
    let mut app2 = new_app();
    app2.start_with_session(saved, true);
    assert_eq!(app2.tab_count(), 2);
    app2.open(&dir.join("other.txt"));
    assert_eq!(app2.session().tabs.len(), 3);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn cancelling_a_quit_midway_keeps_every_edit() {
    let a = temp_file("qa.txt", "a");
    let b = a.parent().unwrap().join("qb.txt");
    std::fs::write(&b, "b").unwrap();
    let mut app = new_app();
    app.open(&a);
    type_text(&mut app, "1");
    app.open(&b);
    type_text(&mut app, "2");
    app.handle(Input::CloseRequested);
    press(&mut app, "d"); // Don't save the first…
    assert!(app.nag_message().is_some(), "asks about the second");
    press(&mut app, "escape"); // …then Cancel
    assert_eq!(app.tab_count(), 2, "nothing was dropped");
    assert!(!app.take_requests().contains(&Request::Quit));
    // Both tabs are still dirty: quitting again asks about both again.
    app.handle(Input::CloseRequested);
    press(&mut app, "d");
    press(&mut app, "d");
    assert!(app.take_requests().contains(&Request::Quit));
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "a");
    std::fs::remove_dir_all(a.parent().unwrap()).ok();
}

/// Whole-app fuzz covering Epic 3: find/replace, split preview, palette,
/// quick open, go to line, tabs, prompts answered at random, tree
/// right-clicks, close requests and resizes down to 1x1. Invariant: never
/// panic, carets stay on char boundaries, every frame renders.
#[test]
fn fuzz_whole_app_with_epic3_features() {
    let keys = [
        "ctrl+f",
        "ctrl+h",
        "enter",
        "shift+enter",
        "f3",
        "shift+f3",
        "escape",
        "ctrl+enter",
        "alt+c",
        "alt+r",
        "ctrl+shift+v",
        "ctrl+e",
        "ctrl+p",
        "ctrl+shift+p",
        "ctrl+g",
        "ctrl+r",
        "ctrl+tab",
        "ctrl+shift+tab",
        "ctrl+w",
        "tab",
        "down",
        "up",
        "left",
        "right",
        "ctrl+z",
        "ctrl+y",
        "ctrl+s",
        "backspace",
        "ctrl+a",
        "ctrl+shift+f",
        "ctrl+\\",
        "ctrl+shift+e",
        "ctrl+plus",
        "ctrl+minus",
        "alt+2",
        "d",
        "c",
        "s",
        "r",
        "k",
        "pagedown",
        "ctrl+end",
        "home",
    ];
    let pieces = ["a", "b", "line", "#", "(", "*", "1", "é", "\t", " ", "md"];
    for seed in 0..6u64 {
        let dir = std::env::temp_dir().join(format!("litedit-fz3-{seed}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(
            dir.join("notes.md"),
            "# Notes\n\n- one\n- two **bold**\n\n```rust\nfn x() {}\n```\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("code.rs"),
            "fn main() {\n    let a = 1; // line\n}\n",
        )
        .unwrap();
        std::fs::write(dir.join("sub/plain.txt"), "line one\nline two\n").unwrap();
        let mut app = new_app();
        app.open(&dir);
        app.open(&dir.join("notes.md"));
        app.open(&dir.join("code.rs"));
        let mut x = seed.wrapping_mul(0xA24BAED4963EE407) | 1;
        let mut rnd = |n: usize| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x % n as u64) as usize
        };
        let mut size = (900u32, 600u32);
        for _ in 0..300 {
            match rnd(14) {
                0..=4 => {
                    key(&mut app, keys[rnd(keys.len())]);
                }
                5..=7 => {
                    app.handle(Input::Text(pieces[rnd(pieces.len())].to_string()));
                }
                8 | 9 => {
                    let (cx, cy) = (rnd(size.0 as usize) as f32, rnd(size.1 as usize) as f32);
                    let button = match rnd(4) {
                        0 => crate::input::MouseButton::Right,
                        1 => crate::input::MouseButton::Middle,
                        _ => crate::input::MouseButton::Left,
                    };
                    app.handle(Input::MouseMove { x: cx, y: cy });
                    app.handle(Input::MouseDown {
                        x: cx,
                        y: cy,
                        button,
                        clicks: 1 + rnd(2) as u32,
                    });
                    app.handle(Input::MouseUp {
                        x: cx,
                        y: cy,
                        button,
                    });
                }
                10 => {
                    app.handle(Input::Wheel {
                        dx: 0.0,
                        dy: rnd(9) as f32 - 4.0,
                        unit: crate::input::WheelUnit::Lines,
                    });
                }
                11 => {
                    size = [(1, 1), (40, 30), (320, 200), (900, 600), (1400, 500)][rnd(5)];
                    app.handle(Input::Resize(size.0, size.1));
                }
                12 => {
                    app.handle(Input::CloseRequested);
                }
                _ => {
                    app.handle(Input::FsChanged(vec![
                        dir.join("code.rs"),
                        dir.join("notes.md"),
                    ]));
                }
            }
            app.take_requests();
            app.settle(Duration::from_millis(200));
            render_hash(&mut app, size.0, size.1);
            if let Some((text, pts)) = app.snapshot() {
                let chunk = crate::editor::buffer::Chunk::parse(&text);
                for p in pts {
                    assert!(
                        p.line < chunk.lines.len(),
                        "seed {seed}: caret line out of range"
                    );
                    assert!(
                        chunk.lines[p.line].is_char_boundary(p.col),
                        "seed {seed}: caret inside a char"
                    );
                }
            }
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[test]
fn soft_wrap_moves_by_visual_rows_and_maps_clicks() {
    let long = "word ".repeat(60); // ~300 chars: several rows at 800 px
    let p = temp_file("wrap.txt", &format!("{long}\nshort\n"));
    let mut app = new_app();
    app.open(&p);
    press(&mut app, "ctrl+\\");
    render_hash(&mut app, 800, 600);
    press(&mut app, "alt+z");
    assert!(app.status_text().contains("Wrap"), "{}", app.status_text());
    render_hash(&mut app, 800, 600);
    // Down moves to the next visual row of the same line, not the next line.
    press(&mut app, "down");
    let st = app.status_text();
    assert!(st.starts_with("Ln 1, Col "), "still line 1: {st}");
    let col: usize = st["Ln 1, Col ".len()..]
        .split_whitespace()
        .next()
        .unwrap()
        .trim_end_matches(',')
        .parse()
        .unwrap();
    assert!(col > 20, "moved one visual row: {st}");
    // Keep going down: eventually reaches line 2.
    for _ in 0..10 {
        press(&mut app, "down");
    }
    assert!(
        app.status_text().starts_with("Ln 2") || app.status_text().starts_with("Ln 3"),
        "{}",
        app.status_text()
    );
    // Clicking on the second visual row places the caret inside line 1.
    let lh = (15.0f32 * 1.4).round();
    let y = 35.0 + 7.0 + lh * 1.5;
    app.handle(Input::MouseDown {
        x: 120.0,
        y,
        button: crate::input::MouseButton::Left,
        clicks: 1,
    });
    let st = app.status_text();
    assert!(st.starts_with("Ln 1, Col "), "{st}");
    // Typing at the end of a wrapped line works and renders.
    press(&mut app, "end");
    type_text(&mut app, "tail");
    render_hash(&mut app, 300, 400);
    press(&mut app, "alt+z");
    assert!(!app.status_text().contains("Wrap"));
    std::fs::remove_dir_all(p.parent().unwrap()).ok();
}

#[test]
fn markdown_source_wraps_by_default() {
    let p = temp_file("w.md", &format!("# T\n\n{}\n", "lorem ipsum ".repeat(40)));
    let mut app = new_app();
    app.open(&p);
    press(&mut app, "ctrl+e");
    assert!(app.status_text().contains("Wrap"), "{}", app.status_text());
    std::fs::remove_dir_all(p.parent().unwrap()).ok();
}

/// Visual movement must work before the first frame is drawn (scripts,
/// fast typing): the row mapping is refreshed on demand.
#[test]
fn wrap_movement_works_without_a_render() {
    let p = temp_file("wrap2.md", &format!("{}\nsecond\n", "word ".repeat(80)));
    let mut app = new_app();
    app.handle(Input::Resize(600, 400));
    app.open(&p);
    press(&mut app, "ctrl+e");
    press(&mut app, "down");
    let st = app.status_text();
    assert!(
        st.contains("Ln 1, Col ") && !st.contains("Ln 1, Col 1 "),
        "moved within line 1: {st}"
    );
    // The row width matches the real view width (not a tiny default):
    // one visual row of a 600 px view holds well over 20 characters.
    let col: usize = st
        .split("Col ")
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert!(col > 30, "row width too small: {st}");
    std::fs::remove_dir_all(p.parent().unwrap()).ok();
}

#[test]
fn preview_text_can_be_selected_and_copied() {
    let p = temp_file(
        "sel.md",
        "# Heading\n\nFirst paragraph here.\n\n- item one\n- item two\n",
    );
    let mut app = new_app();
    app.open(&p);
    press(&mut app, "ctrl+\\");
    render_hash(&mut app, 900, 600);
    // Select all + copy: text of every block, list bullets excluded.
    press(&mut app, "ctrl+a");
    render_hash(&mut app, 900, 600);
    press(&mut app, "ctrl+c");
    let t = app.clipboard_text().unwrap_or_default();
    assert!(
        t.contains("Heading") && t.contains("First paragraph here.") && t.contains("item two"),
        "{t:?}"
    );
    assert!(!t.contains('•'), "bullets are not copied: {t:?}");
    // Drag-select inside the paragraph.
    let y = 35.0 + 28.0 + 70.0; // roughly the first paragraph line
    app.handle(Input::MouseDown {
        x: 45.0,
        y,
        button: crate::input::MouseButton::Left,
        clicks: 1,
    });
    app.handle(Input::MouseMove { x: 120.0, y });
    app.handle(Input::MouseUp {
        x: 120.0,
        y,
        button: crate::input::MouseButton::Left,
    });
    press(&mut app, "ctrl+c");
    let t2 = app.clipboard_text().unwrap_or_default();
    assert!(
        !t2.is_empty() && t2.len() < t.len(),
        "partial selection: {t2:?}"
    );
    render_hash(&mut app, 900, 600);
    std::fs::remove_dir_all(p.parent().unwrap()).ok();
}

/// Pixel hashes of representative scenes. The runtime-theme refactor must
/// leave dark mode byte-identical; this prints them for comparison.
#[test]
#[ignore]
fn print_scene_hashes() {
    let mut out = String::new();
    let scenes: Vec<(&str, Vec<&str>)> = vec![
        ("sample.rs", vec![]),
        ("sample.md", vec![]),
        ("sample.md", vec!["ctrl+e"]),
        ("sample.md", vec!["ctrl+shift+v"]),
        ("sample.rs", vec!["ctrl+f"]),
        ("crlf.txt", vec!["ctrl+shift+p"]),
    ];
    for (f, keys) in scenes {
        let mut app = new_app();
        app.open(&fixture(f));
        for k in keys {
            key(&mut app, k);
        }
        app.settle(Duration::from_secs(5));
        out.push_str(&format!("{f} {:?}\n", render_hash(&mut app, 1000, 700)));
    }
    std::fs::write(std::env::var("HASH_OUT").unwrap(), out).unwrap();
}

/// Render after `steps`, settling first.
fn settled_hash(app: &mut App) -> u64 {
    app.settle(Duration::from_secs(5));
    render_hash(app, 1000, 700)
}

#[test]
fn theme_switch_matches_a_fresh_render_both_ways() {
    // (files to open in order, keys afterwards)
    let scenes: Vec<(Vec<&str>, Vec<&str>)> = vec![
        (vec!["sample.rs"], vec![]),
        (vec!["sample.md"], vec![]),
        (vec!["sample.md"], vec!["ctrl+shift+v"]),
        (vec!["sample.rs"], vec!["ctrl+f"]),
        // Inactive tab, switched to after the theme change.
        (vec!["sample.rs", "sample.md"], vec!["@switch", "ctrl+tab"]),
        (vec!["sample.md", "sample.rs"], vec!["@switch", "ctrl+tab"]),
    ];
    for (files, keys) in scenes {
        let run = |app: &mut App, switch_at_marker: bool| {
            for f in &files {
                app.open(&fixture(f));
            }
            for k in &keys {
                if *k == "@switch" {
                    if switch_at_marker {
                        app.settle(Duration::from_secs(5));
                        app.set_light(true);
                    }
                } else {
                    key(app, k);
                }
            }
        };
        let has_marker = keys.contains(&"@switch");
        // Dark, then switched to light (at the marker, or at the end).
        theme::set_light(false);
        let mut a = new_app();
        run(&mut a, true);
        let dark = (!has_marker).then(|| settled_hash(&mut a));
        a.set_light(true);
        let switched = settled_hash(&mut a);
        // A fresh app that was light from the start.
        let mut b = new_app();
        run(&mut b, false);
        let fresh = settled_hash(&mut b);
        assert_eq!(
            switched, fresh,
            "{files:?} {keys:?}: switched vs fresh light"
        );
        if let Some(dark) = dark {
            assert_ne!(dark, fresh, "{files:?}: light differs from dark");
            a.set_light(false);
            assert_eq!(settled_hash(&mut a), dark, "{files:?}: back to dark");
        }
    }
    theme::set_light(false);
}

fn settings_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("litedit-settings-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn run_command(app: &mut App, label: &str) {
    key(app, "ctrl+shift+p");
    app.handle(Input::Text(label.into()));
    key(app, "enter");
}

#[test]
fn open_settings_creates_the_template_once_and_saving_applies_it() {
    let dir = settings_dir("open");
    let path = dir.join("sub").join("settings.txt");
    let mut app = new_app();
    app.use_settings(path.clone());
    key(&mut app, "ctrl+,");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), settings::TEMPLATE);
    assert!(app.title().contains("settings.txt"));
    // Edit the theme line and save: applied immediately.
    key(&mut app, "ctrl+f");
    app.handle(Input::Text("theme = dark".into()));
    key(&mut app, "escape");
    app.handle(Input::Text("theme = light".into()));
    assert!(!theme::is_light());
    key(&mut app, "ctrl+s");
    assert!(theme::is_light(), "saving the settings file applies it");
    assert!(
        app.status_text().contains("Settings applied"),
        "{}",
        app.status_text()
    );
    // Bad lines are reported, not fatal.
    key(&mut app, "ctrl+end");
    app.handle(Input::Text("\nbogus = 1\n".into()));
    key(&mut app, "ctrl+s");
    assert!(
        app.status_text().contains("unknown setting 'bogus'"),
        "{}",
        app.status_text()
    );
    assert!(theme::is_light());
    // Opening again never replaces an existing file.
    let before = std::fs::read_to_string(&path).unwrap();
    key(&mut app, "ctrl+w");
    key(&mut app, "ctrl+,");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    // Font sizes apply to every tab.
    std::fs::write(&path, "code_font_size = 30\n").unwrap();
    app.use_settings(path.clone());
    assert_eq!(theme::code_font_size(), 30.0);
    assert!(!theme::is_light(), "missing theme line means dark");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn toggle_theme_rewrites_only_the_theme_line() {
    let dir = settings_dir("toggle");
    let path = dir.join("settings.txt");
    std::fs::write(
        &path,
        "# mine\r\ncode_font_size = 15 # keep\r\ntheme = dark\r\n",
    )
    .unwrap();
    let mut app = new_app();
    app.use_settings(path.clone());
    run_command(&mut app, "toggle light");
    assert!(theme::is_light());
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "# mine\r\ncode_font_size = 15 # keep\r\ntheme = light\r\n"
    );
    // Open (clean) in a tab: toggling reloads the tab with the new text.
    key(&mut app, "ctrl+,");
    run_command(&mut app, "toggle light");
    assert!(!theme::is_light());
    assert!(
        app.status_text().contains("saved in settings"),
        "{}",
        app.status_text()
    );
    key(&mut app, "ctrl+a");
    key(&mut app, "ctrl+c");
    assert!(app.clipboard_text().unwrap().contains("theme = dark"));
    // Dirty in a tab: the file is left alone, the theme still switches.
    key(&mut app, "ctrl+end");
    app.handle(Input::Text("# unsaved".into()));
    let on_disk = std::fs::read_to_string(&path).unwrap();
    run_command(&mut app, "toggle light");
    assert!(theme::is_light());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), on_disk);
    assert!(
        app.status_text().contains("unsaved edits"),
        "{}",
        app.status_text()
    );
    // No file yet: the toggle creates one from the template.
    let fresh = dir.join("new").join("settings.txt");
    let mut app2 = new_app();
    app2.use_settings(fresh.clone());
    run_command(&mut app2, "toggle light");
    let (s, problems) = settings::Settings::parse(&std::fs::read_to_string(&fresh).unwrap());
    assert!(problems.is_empty());
    assert_eq!(s.light, theme::is_light());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn headless_script_theme_command() {
    let mut app = new_app();
    app.open(&fixture("sample.rs"));
    crate::script::run(&mut app, "theme light\nexpect title sample.rs\n").unwrap();
    assert!(theme::is_light());
    assert!(crate::script::run(&mut app, "theme purple\n").is_err());
}

#[test]
fn font_size_change_matches_a_fresh_render() {
    let dir = settings_dir("sizes");
    let path = dir.join("settings.txt");
    std::fs::write(
        &path,
        "ui_font_size = 22\ncode_font_size = 19\nmarkdown_font_size = 21\n",
    )
    .unwrap();
    let open_two = |app: &mut App| {
        app.open(&fixture("sample.md"));
        app.open(&fixture("sample.rs"));
        key(app, "ctrl+f");
    };
    let mut a = new_app();
    open_two(&mut a);
    let before = settled_hash(&mut a);
    a.use_settings(path.clone());
    let changed = settled_hash(&mut a);
    key(&mut a, "escape");
    key(&mut a, "ctrl+tab");
    let md_changed = settled_hash(&mut a);
    theme::set_sizes(theme::FONT_SIZE, theme::CODE_FONT_SIZE, theme::MD_FONT_SIZE);
    let mut b = new_app();
    b.use_settings(path);
    open_two(&mut b);
    assert_ne!(before, changed);
    assert_eq!(changed, settled_hash(&mut b), "live size change vs fresh");
    key(&mut b, "escape");
    key(&mut b, "ctrl+tab");
    assert_eq!(md_changed, settled_hash(&mut b), "inactive markdown tab");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn missing_font_is_reported() {
    let dir = settings_dir("font");
    let path = dir.join("settings.txt");
    std::fs::write(&path, "code_font = No Such Font Xyz\n").unwrap();
    let mut app = new_app();
    app.use_settings(path);
    let st = app.status_text();
    assert!(st.contains("'No Such Font Xyz' not found"), "{st}");
    let _ = std::fs::remove_dir_all(&dir);
}

fn drag(app: &mut App, from: (f32, f32), to: (f32, f32)) {
    app.handle(Input::MouseDown {
        x: from.0,
        y: from.1,
        button: crate::input::MouseButton::Left,
        clicks: 1,
    });
    let mid = (from.0, (from.1 + to.1) / 2.0);
    app.handle(Input::MouseMove { x: mid.0, y: mid.1 });
    app.handle(Input::MouseMove { x: to.0, y: to.1 });
    app.handle(Input::MouseUp {
        x: to.0,
        y: to.1,
        button: crate::input::MouseButton::Left,
    });
}

/// A folder with docs/, notes/b.md and a.txt; rows at y ≈ 19 (root), 46
/// (docs), 73 (notes), 100 (b.md once notes is open) / a.txt.
fn drag_fixture(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("litedit-drag-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("docs")).unwrap();
    std::fs::create_dir_all(d.join("notes")).unwrap();
    std::fs::write(d.join("a.txt"), "hello").unwrap();
    std::fs::write(d.join("notes/b.md"), "md").unwrap();
    d
}

#[test]
fn dragging_a_file_onto_a_folder_asks_then_moves_and_keeps_the_tab() {
    let d = drag_fixture("move");
    let mut app = new_app();
    app.open(&d);
    app.open(&d.join("a.txt"));
    render_hash(&mut app, 800, 600);
    // Rows: root 19, docs 46, notes 73, a.txt 100.
    drag(&mut app, (60.0, 100.0), (60.0, 46.0));
    assert_eq!(app.nag_message().as_deref(), Some("Move a.txt into docs?"));
    assert!(d.join("a.txt").exists(), "nothing moves before the answer");
    key(&mut app, "enter");
    assert!(app.nag_message().is_none(), "{:?}", app.nag_message());
    assert!(d.join("docs/a.txt").exists() && !d.join("a.txt").exists());
    assert!(
        app.title().starts_with("a.txt"),
        "tab follows the file: {}",
        app.title()
    );
    // Saving afterwards writes the new location.
    key(&mut app, "ctrl+end");
    app.handle(Input::Text("!".into()));
    key(&mut app, "ctrl+s");
    assert_eq!(
        std::fs::read_to_string(d.join("docs/a.txt")).unwrap(),
        "hello!"
    );
    assert!(!d.join("a.txt").exists());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn drag_cancel_dirty_and_invalid_targets_move_nothing() {
    let d = drag_fixture("guard");
    let mut app = new_app();
    app.open(&d);
    render_hash(&mut app, 800, 600);
    // Cancel.
    drag(&mut app, (60.0, 100.0), (60.0, 46.0));
    key(&mut app, "escape");
    assert!(d.join("a.txt").exists());
    // A short wiggle is a click, not a drag.
    app.handle(Input::MouseDown {
        x: 60.0,
        y: 46.0,
        button: crate::input::MouseButton::Left,
        clicks: 1,
    });
    app.handle(Input::MouseMove { x: 60.0, y: 48.0 });
    app.handle(Input::MouseUp {
        x: 60.0,
        y: 48.0,
        button: crate::input::MouseButton::Left,
    });
    assert!(app.nag_message().is_none());
    // A folder dropped onto itself: drag away and back. Pressing on notes
    // also expands it, so a.txt moves down to y = 127.
    app.handle(Input::MouseDown {
        x: 60.0,
        y: 73.0,
        button: crate::input::MouseButton::Left,
        clicks: 1,
    });
    app.handle(Input::MouseMove { x: 60.0, y: 110.0 });
    app.handle(Input::MouseMove { x: 60.0, y: 73.0 });
    app.handle(Input::MouseUp {
        x: 60.0,
        y: 73.0,
        button: crate::input::MouseButton::Left,
    });
    assert!(app.nag_message().is_none(), "into itself");
    // Onto its own folder (the root): nothing to do.
    drag(&mut app, (60.0, 127.0), (60.0, 19.0));
    assert!(app.nag_message().is_none(), "already in the root");
    // Dropped outside the tree: nothing.
    drag(&mut app, (60.0, 127.0), (600.0, 127.0));
    assert!(app.nag_message().is_none());
    // A file with unsaved edits is not moved.
    app.open(&d.join("a.txt"));
    app.handle(Input::Text("x".into()));
    render_hash(&mut app, 800, 600);
    drag(&mut app, (60.0, 127.0), (60.0, 46.0));
    assert!(
        app.nag_message()
            .unwrap_or_default()
            .contains("Save or close")
    );
    assert!(d.join("a.txt").exists());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn tree_keys_f2_renames_and_delete_asks_first() {
    let d = drag_fixture("keys");
    let mut app = new_app();
    app.open(&d);
    render_hash(&mut app, 800, 600);
    key(&mut app, "ctrl+shift+e");
    // Selection starts on the root; F2/Delete on the root do nothing.
    key(&mut app, "f2");
    key(&mut app, "delete");
    assert!(app.nag_message().is_none());
    key(&mut app, "end"); // a.txt
    key(&mut app, "f2");
    key(&mut app, "ctrl+a");
    app.handle(Input::Text("renamed.txt".into()));
    key(&mut app, "enter");
    assert!(d.join("renamed.txt").exists() && !d.join("a.txt").exists());
    key(&mut app, "ctrl+shift+e");
    key(&mut app, "end");
    key(&mut app, "delete");
    assert_eq!(
        app.nag_message().as_deref(),
        Some("Move renamed.txt to the Recycle Bin?")
    );
    key(&mut app, "escape");
    assert!(d.join("renamed.txt").exists());
    let _ = std::fs::remove_dir_all(&d);
}

fn many_files(tag: &str, n: usize) -> (PathBuf, Vec<PathBuf>) {
    let d = std::env::temp_dir().join(format!("litedit-tabs-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let files = (0..n)
        .map(|i| {
            let p = d.join(format!("f{i:02}.txt"));
            std::fs::write(&p, format!("file {i}\n")).unwrap();
            p
        })
        .collect();
    (d, files)
}

fn mouse(app: &mut App, x: f32, y: f32, button: crate::input::MouseButton) {
    app.handle(Input::MouseDown {
        x,
        y,
        button,
        clicks: 1,
    });
    app.handle(Input::MouseUp { x, y, button });
}

#[test]
fn many_tabs_scroll_and_the_active_tab_stays_reachable() {
    use crate::input::{MouseButton, WheelUnit};
    let (d, files) = many_files("scroll", 20);
    let mut app = new_app();
    for f in &files {
        app.open(f);
    }
    let (bar, rects) = app.tab_layout().unwrap();
    let last = rects[19];
    assert!(
        last.x >= bar.x && last.right() <= bar.right(),
        "active (last) tab in view"
    );
    assert!(rects[0].right() <= bar.x, "first tabs scrolled out of view");
    // Wheel over the bar scrolls back to the start.
    app.handle(Input::MouseMove {
        x: bar.x as f32 + 20.0,
        y: 10.0,
    });
    for _ in 0..50 {
        app.handle(Input::Wheel {
            dx: 0.0,
            dy: -3.0,
            unit: WheelUnit::Lines,
        });
    }
    let (_, rects) = app.tab_layout().unwrap();
    assert_eq!(rects[0].x, bar.x, "scrolled to the first tab");
    assert_eq!(
        app.active_tab(),
        19,
        "scrolling doesn't change the active tab"
    );
    // Clicking what is shown activates exactly that tab.
    let r1 = rects[1];
    mouse(
        &mut app,
        r1.x as f32 + 10.0,
        r1.y as f32 + 10.0,
        MouseButton::Left,
    );
    assert_eq!(app.active_tab(), 1);
    assert!(app.title().starts_with("f01.txt"));
    // Ctrl+Tab through to the end keeps the active tab visible.
    for _ in 0..18 {
        key(&mut app, "ctrl+tab");
    }
    let (bar, rects) = app.tab_layout().unwrap();
    let a = rects[app.active_tab()];
    assert!(a.x >= bar.x && a.right() <= bar.right());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn close_others_asks_for_each_unsaved_tab_and_cancel_stops() {
    use crate::input::MouseButton;
    let (d, files) = many_files("closeothers", 5);
    let mut app = new_app();
    for f in &files {
        app.open(f);
    }
    // Dirty f01 and f03.
    for i in [1usize, 3] {
        key(&mut app, &format!("alt+{}", i + 1));
        app.handle(Input::Text("edit".into()));
    }
    key(&mut app, "alt+3"); // f02
    let (_, rects) = app.tab_layout().unwrap();
    let r = rects[2];
    mouse(
        &mut app,
        r.x as f32 + 10.0,
        r.y as f32 + 10.0,
        MouseButton::Right,
    );
    run_menu_item(&mut app, "Close Others");
    // Clean f00, f04 closed; asked about f01 first.
    assert_eq!(
        app.nag_message().as_deref(),
        Some("Save changes to f01.txt?")
    );
    let names: Vec<String> = app.tab_titles().into_iter().map(|t| t.0).collect();
    assert_eq!(names, ["f01.txt", "f02.txt", "f03.txt"]);
    key(&mut app, "d"); // Don't save: f01 closes, then f03 is asked about.
    assert_eq!(
        app.nag_message().as_deref(),
        Some("Save changes to f03.txt?")
    );
    key(&mut app, "escape"); // Cancel: f03 stays, with its edits.
    assert!(app.nag_message().is_none());
    assert_eq!(
        app.tab_titles(),
        vec![("f02.txt".into(), false), ("f03.txt".into(), true)]
    );
    assert_eq!(
        std::fs::read_to_string(&files[1]).unwrap(),
        "file 1\n",
        "not saved"
    );
    // Close All with Save writes the edit.
    key(&mut app, "alt+1");
    let (_, rects) = app.tab_layout().unwrap();
    let r = rects[0];
    mouse(
        &mut app,
        r.x as f32 + 10.0,
        r.y as f32 + 10.0,
        MouseButton::Right,
    );
    run_menu_item(&mut app, "Close All");
    assert_eq!(
        app.nag_message().as_deref(),
        Some("Save changes to f03.txt?")
    );
    key(&mut app, "s");
    assert!(app.nag_message().is_none(), "{:?}", app.nag_message());
    assert!(std::fs::read_to_string(&files[3]).unwrap().contains("edit"));
    assert_eq!(app.tab_titles().len(), 1, "back to the start view");
    assert!(app.tab_titles()[0].0.is_empty() || !app.tab_titles()[0].1);
    // Ctrl+Shift+T brings the last closed file back.
    key(&mut app, "ctrl+shift+t");
    assert!(app.title().starts_with("f03.txt"), "{}", app.title());
    let _ = std::fs::remove_dir_all(&d);
}

/// Click a right-click menu item by label (the menu is open).
fn run_menu_item(app: &mut App, label: &str) {
    render_hash(app, 800, 600);
    let pos = app
        .menu_item_pos(label)
        .unwrap_or_else(|| panic!("no menu item {label}"));
    mouse(app, pos.0, pos.1, crate::input::MouseButton::Left);
}

#[test]
fn tab_reorder_and_close_fuzz_never_loses_edits() {
    use crate::input::MouseButton;
    let (d, files) = many_files("fuzz", 8);
    let mut app = new_app();
    let mut seed = 0x2545F4914F6CDD1Du64;
    let mut rnd = |n: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % n as u64) as usize
    };
    for f in &files {
        app.open(f);
    }
    // Every tab gets a unique edit; it must survive whatever happens.
    for i in 0..files.len() {
        key(&mut app, &format!("alt+{}", (i % 9) + 1));
    }
    let mut dirty: Vec<String> = Vec::new();
    for step in 0..300 {
        let n = app.tab_titles().len();
        let Some((bar, rects)) = app.tab_layout() else {
            break;
        };
        match rnd(6) {
            0 if n > 0 => {
                // Drag a tab somewhere else.
                let a = rects[rnd(n)];
                let b = rects[rnd(n)];
                let y = a.y as f32 + 10.0;
                app.handle(Input::MouseDown {
                    x: a.x as f32 + 5.0,
                    y,
                    button: MouseButton::Left,
                    clicks: 1,
                });
                app.handle(Input::MouseMove {
                    x: (a.x as f32 + b.x as f32) / 2.0 + 5.0,
                    y,
                });
                app.handle(Input::MouseMove {
                    x: (b.x as f32 + 5.0).clamp(bar.x as f32, bar.right() as f32 - 1.0),
                    y,
                });
                app.handle(Input::MouseUp {
                    x: b.x as f32 + 5.0,
                    y,
                    button: MouseButton::Left,
                });
            }
            1 => {
                app.handle(Input::Text(format!("<{step}>")));
            }
            2 => {
                key(&mut app, "ctrl+tab");
            }
            3 => {
                // Close the active tab; answer Cancel if asked (keeps edits).
                key(&mut app, "ctrl+w");
                if app.nag_message().is_some() {
                    key(&mut app, "escape");
                }
            }
            4 => {
                let r = rects[rnd(n.max(1)).min(rects.len().saturating_sub(1))];
                mouse(
                    &mut app,
                    r.x as f32 + 10.0,
                    r.y as f32 + 10.0,
                    MouseButton::Right,
                );
                let item = [
                    "Close Others",
                    "Close to the Right",
                    "Close Saved",
                    "Close All",
                ][rnd(4)];
                render_hash(&mut app, 800, 600);
                if app.menu_item_pos(item).is_some() {
                    run_menu_item(&mut app, item);
                }
                while app.nag_message().is_some() {
                    key(&mut app, "escape");
                }
                key(&mut app, "escape");
            }
            _ => {
                key(&mut app, "ctrl+shift+t");
            }
        }
        // Invariants: a tab with unsaved edits is never closed (only Cancel
        // is ever answered), and no file on disk was written.
        let _ = render_hash(&mut app, 800, 600);
        let now: Vec<String> = app
            .tab_titles()
            .into_iter()
            .filter(|t| t.1)
            .map(|t| t.0)
            .collect();
        for t in &dirty {
            assert!(
                now.contains(t),
                "step {step}: unsaved {t} was closed; tabs {:?}",
                app.tab_titles()
            );
        }
        dirty = now;
        for (i, f) in files.iter().enumerate() {
            assert_eq!(std::fs::read_to_string(f).unwrap(), format!("file {i}\n"));
        }
    }
    let _ = std::fs::remove_dir_all(&d);
}

fn compare_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("litedit-cmp-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::copy(fixture("compare/old.rs"), d.join("old.rs")).unwrap();
    std::fs::copy(fixture("compare/new.rs"), d.join("new.rs")).unwrap();
    d
}

#[test]
fn compare_from_the_tree_menu_shows_changes_and_f7_walks_them() {
    use crate::input::MouseButton;
    let d = compare_dir("tree");
    let mut app = new_app();
    app.open(&d);
    render_hash(&mut app, 800, 600);
    // Rows: root 19, new.rs 46, old.rs 73.
    mouse(&mut app, 60.0, 73.0, MouseButton::Right);
    run_menu_item(&mut app, "Select for Compare");
    mouse(&mut app, 60.0, 46.0, MouseButton::Right);
    run_menu_item(&mut app, "Compare with Selected");
    assert_eq!(app.title(), "old.rs ↔ new.rs - litedit");
    assert!(
        app.status_text().contains("5 changes"),
        "{}",
        app.status_text()
    );
    key(&mut app, "f7");
    assert!(app.status_text().contains("Change 1 of 5"));
    key(&mut app, "shift+f7");
    assert!(app.status_text().contains("Change 5 of 5"), "wraps around");
    // The new file changes on disk: the comparison follows.
    std::fs::copy(d.join("old.rs"), d.join("new.rs")).unwrap();
    app.handle(Input::FsChanged(vec![d.join("new.rs")]));
    assert!(
        app.status_text().contains("No changes"),
        "{}",
        app.status_text()
    );
    // Folders don't offer compare.
    mouse(&mut app, 60.0, 19.0, MouseButton::Right);
    render_hash(&mut app, 800, 600);
    assert!(app.menu_item_pos("Select for Compare").is_none());
    key(&mut app, "escape");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn compare_with_saved_uses_the_unsaved_text() {
    let d = compare_dir("saved");
    let mut app = new_app();
    app.open(&d.join("old.rs"));
    key(&mut app, "ctrl+end");
    app.handle(Input::Text("// added while editing\n".into()));
    run_command(&mut app, "compare with saved");
    assert_eq!(app.title(), "old.rs (saved) ↔ old.rs (unsaved) - litedit");
    assert!(
        app.status_text().contains("1 change"),
        "{}",
        app.status_text()
    );
    // The edited tab is still open and still has its edit.
    key(&mut app, "ctrl+shift+tab");
    assert!(app.title().starts_with("● old.rs"), "{}", app.title());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn compare_refuses_huge_files_and_survives_odd_input() {
    let d = compare_dir("odd");
    let big = d.join("big.txt");
    std::fs::write(&big, "x\n".repeat(crate::diff::MAX_LINES + 10)).unwrap();
    let mut app = new_app();
    app.compare_files(&d.join("old.rs"), &big);
    assert!(
        app.nag_message()
            .unwrap_or_default()
            .contains("Cannot compare")
    );
    key(&mut app, "escape");
    app.compare_files(&d.join("old.rs"), &d.join("missing.rs"));
    assert!(
        app.nag_message()
            .unwrap_or_default()
            .contains("Cannot compare")
    );
    key(&mut app, "escape");
    // Binary-ish and invalid UTF-8 content, empty files: no panic.
    std::fs::write(d.join("bin"), [0u8, 159, 146, 150, 10, 255, 0, 1]).unwrap();
    std::fs::write(d.join("empty"), b"").unwrap();
    app.compare_files(&d.join("bin"), &d.join("empty"));
    for k in [
        "f7",
        "ctrl+end",
        "pagedown",
        "right",
        "shift+f7",
        "ctrl+home",
    ] {
        key(&mut app, k);
        render_hash(&mut app, 800, 600);
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn compare_view_theme_switch_matches_fresh() {
    theme::set_light(false);
    let mut a = new_app();
    a.compare_files(&fixture("compare/old.rs"), &fixture("compare/new.rs"));
    let dark = settled_hash(&mut a);
    a.set_light(true);
    let switched = settled_hash(&mut a);
    let mut b = new_app();
    b.compare_files(&fixture("compare/old.rs"), &fixture("compare/new.rs"));
    assert_eq!(switched, settled_hash(&mut b));
    a.set_light(false);
    assert_eq!(settled_hash(&mut a), dark);
}

fn tmp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("litedit-s50-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn a_question_is_answered_for_its_own_tab_even_if_a_file_is_dropped() {
    let d = tmp_dir("nagtab");
    std::fs::write(d.join("a.txt"), "A").unwrap();
    std::fs::write(d.join("c.txt"), "C").unwrap();
    let mut app = new_app();
    app.open(&d.join("c.txt"));
    app.handle(Input::Text("c-edit ".into()));
    app.open(&d.join("a.txt"));
    app.handle(Input::Text("a-edit ".into()));
    key(&mut app, "ctrl+w");
    assert_eq!(app.nag_message().as_deref(), Some("Save changes to a.txt?"));
    app.handle(Input::Drop(d.join("c.txt")));
    assert!(
        app.title().contains("a.txt"),
        "drop doesn't switch tabs under a question"
    );
    key(&mut app, "s");
    assert_eq!(
        std::fs::read_to_string(d.join("a.txt")).unwrap(),
        "a-edit A"
    );
    assert_eq!(
        std::fs::read_to_string(d.join("c.txt")).unwrap(),
        "C",
        "c untouched"
    );
    assert_eq!(app.tab_titles(), vec![("c.txt".to_string(), true)]);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn own_saves_and_touches_are_not_disk_conflicts() {
    let d = tmp_dir("touch");
    let f = d.join("fs.txt");
    std::fs::write(&f, "start").unwrap();
    let mut app = new_app();
    app.open(&f);
    app.handle(Input::Text("A".into()));
    app.handle(Input::FsChanged(vec![f.clone()]));
    assert!(app.nag_message().is_none(), "a touch is not a change");
    key(&mut app, "ctrl+s");
    app.handle(Input::Text("B".into()));
    // The watcher reports our own save a moment later.
    app.handle(Input::FsChanged(vec![f.clone()]));
    assert!(app.nag_message().is_none(), "{:?}", app.nag_message());
    app.handle(Input::Text(" ".into()));
    assert!(
        app.view_text().contains("AB "),
        "typing continues: {}",
        app.view_text()
    );
    // A real external change while dirty asks; Enter keeps my version.
    std::fs::write(&f, "EXTERNAL").unwrap();
    app.handle(Input::FsChanged(vec![f.clone()]));
    assert!(
        app.nag_message()
            .unwrap_or_default()
            .contains("changed on disk")
    );
    key(&mut app, "enter");
    assert!(app.view_text().contains("AB "), "Enter is the safe answer");
    // Saving now warns that another program changed the file.
    key(&mut app, "ctrl+s");
    assert!(
        app.nag_message()
            .unwrap_or_default()
            .contains("changed by another program"),
        "{:?}",
        app.nag_message()
    );
    key(&mut app, "escape");
    assert_eq!(
        std::fs::read_to_string(&f).unwrap(),
        "EXTERNAL",
        "Cancel writes nothing"
    );
    key(&mut app, "ctrl+s");
    run_nag_option(&mut app, "Save mine over it");
    assert!(std::fs::read_to_string(&f).unwrap().contains("AB "));
    // And after that, saving again is quiet.
    app.handle(Input::Text("!".into()));
    key(&mut app, "ctrl+s");
    assert!(app.nag_message().is_none());
    let _ = std::fs::remove_dir_all(&d);
}

/// Pick a question-bar button by label.
fn run_nag_option(app: &mut App, label: &str) {
    let opts = app.nag_options();
    let i = opts
        .iter()
        .position(|o| o == label)
        .unwrap_or_else(|| panic!("no option {label} in {opts:?}"));
    for _ in 0..i {
        key(app, "right");
    }
    key(app, "enter");
}

#[test]
fn files_that_cannot_round_trip_always_ask_before_saving() {
    let d = tmp_dir("lossy");
    let cases: Vec<(&str, Vec<u8>)> = vec![
        (
            "bom-invalid.txt",
            vec![0xEF, 0xBB, 0xBF, b'a', 0xFF, 0xFE, b'b', 0xE9],
        ),
        (
            "utf16-surrogate.txt",
            vec![0xFF, 0xFE, b'a', 0, 0x00, 0xD8, b'b', 0],
        ),
        ("utf16-odd.txt", vec![0xFF, 0xFE, b'a', 0, b'b']),
        ("plain-invalid.txt", vec![b'a', 0xFF, b'b']),
    ];
    for (name, bytes) in &cases {
        let f = d.join(name);
        std::fs::write(&f, bytes).unwrap();
        let mut app = new_app();
        app.open(&f);
        app.handle(Input::Text("x".into()));
        key(&mut app, "ctrl+s");
        assert!(
            app.nag_message()
                .unwrap_or_default()
                .contains("invalid characters"),
            "{name}: {:?}",
            app.nag_message()
        );
        key(&mut app, "escape");
        assert_eq!(
            &std::fs::read(&f).unwrap(),
            bytes,
            "{name}: Cancel keeps the bytes"
        );
        // Save As onto itself asks too.
        app.save_as(&f);
        assert!(
            app.nag_message()
                .unwrap_or_default()
                .contains("invalid characters"),
            "{name}"
        );
        key(&mut app, "escape");
        assert_eq!(&std::fs::read(&f).unwrap(), bytes);
    }
    // Valid files still save without a question, byte for byte.
    let ok = d.join("ok.txt");
    std::fs::write(&ok, b"\xEF\xBB\xBFline\r\n").unwrap();
    let mut app = new_app();
    app.open(&ok);
    key(&mut app, "ctrl+end");
    app.handle(Input::Text("x".into()));
    key(&mut app, "ctrl+s");
    assert!(app.nag_message().is_none());
    assert_eq!(std::fs::read(&ok).unwrap(), b"\xEF\xBB\xBFline\r\nx");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn theme_toggle_never_rewrites_a_settings_file_it_cannot_read_exactly() {
    let d = tmp_dir("setenc");
    let path = d.join("settings.txt");
    let ansi = b"# caf\xe9 \xff\ntheme = dark\n".to_vec();
    std::fs::write(&path, &ansi).unwrap();
    let mut app = new_app();
    app.use_settings(path.clone());
    run_command(&mut app, "toggle light");
    assert!(theme::is_light(), "still switches for this session");
    assert_eq!(std::fs::read(&path).unwrap(), ansi);
    assert!(
        app.status_text().contains("isn't UTF-8"),
        "{}",
        app.status_text()
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn zooming_out_to_the_limit_with_small_fonts_never_panics() {
    let d = tmp_dir("zoom");
    let set = d.join("settings.txt");
    std::fs::write(&set, "ui_font_size = 8\n").unwrap();
    let (_, files) = many_files("zoomfiles", 6);
    for size in [(1280u32, 800u32), (300, 200), (90, 60)] {
        let mut app = new_app();
        app.use_settings(set.clone());
        for f in &files {
            app.open(f);
        }
        app.handle(Input::Resize(size.0, size.1));
        for _ in 0..30 {
            key(&mut app, "ctrl+minus");
            render_hash(&mut app, size.0, size.1);
        }
        for _ in 0..60 {
            key(&mut app, "ctrl+plus");
        }
        render_hash(&mut app, size.0, size.1);
    }
    let _ = std::fs::remove_dir_all(&d);
}
