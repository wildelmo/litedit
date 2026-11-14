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
