//! S7.4 compare extras: compare the two sides of a split, ignore whitespace,
//! jump from a row to the file, compare with the clipboard.

use std::path::{Path, PathBuf};

use crate::app::App;
use crate::clipboard::MemClipboard;
use crate::input::{Input, MouseButton};
use crate::tests::{
    compare_dir, key, mouse, new_app, render_hash, run_command, run_menu_item, settled_hash,
    tmp_dir,
};
use crate::theme;

const OLD: &str = "a\nb\nc\nd\ne\n";
/// Against OLD the rows are: a= | +X | +Y | b= | c= | -d | e=
const NEW: &str = "a\nX\nY\nb\nc\ne\n";

fn write(d: &Path, name: &str, text: &str) -> PathBuf {
    let p = d.join(name);
    std::fs::write(&p, text).unwrap();
    p
}

/// A folder with `old.txt` (OLD) and `new.txt` (NEW).
fn pair(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
    let d = tmp_dir(&format!("cmp-{tag}"));
    let (o, n) = (write(&d, "old.txt", OLD), write(&d, "new.txt", NEW));
    (d, o, n)
}

/// Open `left` and `right` side by side (focus ends on the right).
fn split(app: &mut App, left: &Path, right: &Path) {
    app.open(left);
    app.open(right);
    key(app, "ctrl+alt+right");
    assert!(app.is_split_for_test());
}

fn set_clipboard(app: &mut App, text: Option<&str>) {
    app.set_clipboard(Box::new(MemClipboard(text.map(str::to_string))));
}

fn titles(app: &App) -> Vec<String> {
    app.tab_titles().into_iter().map(|(t, _)| t).collect()
}

fn no_tab_is_dirty(app: &App) -> bool {
    app.tab_titles().iter().all(|(_, dirty)| !dirty)
}

/// Window position of the middle of row `row` on side `side` (0 = left)
/// of the comparison in the active tab, unscrolled.
fn row_xy(app: &mut App, side: usize, row: usize) -> (f32, f32) {
    render_hash(app, 800, 600);
    let m = app.main_rect();
    let ui_lh = (theme::font_size().round() * theme::LINE_HEIGHT).round();
    let header = ui_lh + theme::PADDING_Y.round() * 2.0;
    let row_h = (theme::code_font_size().round() * theme::LINE_HEIGHT).round();
    let x = m.x as f32 + m.w as f32 * if side == 0 { 0.25 } else { 0.75 };
    (x, m.y as f32 + header + (row as f32 + 0.5) * row_h)
}

fn double_click(app: &mut App, x: f32, y: f32) {
    for clicks in 1..=2 {
        app.handle(Input::MouseDown {
            x,
            y,
            button: MouseButton::Left,
            clicks,
        });
        app.handle(Input::MouseUp {
            x,
            y,
            button: MouseButton::Left,
        });
    }
}

fn double_click_row(app: &mut App, side: usize, row: usize) {
    let (x, y) = row_xy(app, side, row);
    double_click(app, x, y);
}

/// Bring the tab titled `title` to the front.
fn show_tab(app: &mut App, title: &str) {
    let i = titles(app)
        .iter()
        .position(|t| t == title)
        .unwrap_or_else(|| panic!("no tab {title} in {:?}", titles(app)));
    app.activate(i);
}

fn status(app: &App) -> String {
    app.status_text()
}

// ---- 1. compare the two sides of a split ---------------------------------

#[test]
fn split_compare_puts_the_left_file_left_whichever_side_has_focus() {
    let d = compare_dir("s74-split");
    let (old, new) = (d.join("old.rs"), d.join("new.rs"));
    let before = (std::fs::read(&old).unwrap(), std::fs::read(&new).unwrap());
    for focus_left in [false, true] {
        let mut app = new_app();
        split(&mut app, &old, &new);
        if focus_left {
            key(&mut app, "ctrl+1");
            assert!(app.title().starts_with("old.rs"));
        }
        run_command(&mut app, "compare left");
        assert_eq!(app.title(), "old.rs ↔ new.rs - litedit", "{focus_left}");
        assert!(status(&app).starts_with("5 changes"), "{}", status(&app));
        assert_eq!(app.tab_count(), 3);
        assert!(no_tab_is_dirty(&app));
        render_hash(&mut app, 800, 600);
    }
    // From the tab menu, with the sides swapped: left is still "old".
    let mut app = new_app();
    split(&mut app, &new, &old);
    let (bar, rects) = app.tab_layout().unwrap();
    let r = rects[0];
    mouse(
        &mut app,
        (r.x + r.w / 2) as f32,
        (bar.y + bar.h / 2) as f32,
        MouseButton::Right,
    );
    run_menu_item(&mut app, "Compare Left ↔ Right");
    assert_eq!(app.title(), "new.rs ↔ old.rs - litedit");
    assert!(status(&app).starts_with("5 changes"), "{}", status(&app));
    // Comparing changed nothing on disk.
    assert_eq!(
        (std::fs::read(&old).unwrap(), std::fs::read(&new).unwrap()),
        before
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn split_compare_uses_unsaved_text_and_leaves_the_edit_alone() {
    let (d, old, new) = pair("split-unsaved");
    let mut app = new_app();
    split(&mut app, &old, &new);
    // Unsaved edit on the right: a new first line.
    key(&mut app, "ctrl+home");
    app.handle(Input::Text("fresh\n".into()));
    run_command(&mut app, "compare left");
    assert_eq!(app.title(), "old.txt ↔ new.txt (unsaved) - litedit");
    // +fresh, +X +Y, -d
    assert!(status(&app).starts_with("3 changes"), "{}", status(&app));
    // The snapshot does not follow the file on disk (the edit is unsaved).
    std::fs::write(&new, OLD).unwrap();
    app.handle(Input::FsChanged(vec![new.clone()]));
    show_tab(&mut app, "old.txt ↔ new.txt (unsaved)");
    assert!(status(&app).starts_with("3 changes"), "{}", status(&app));
    // The edited tab still has its edit, and only that tab is modified.
    assert_eq!(
        app.tab_titles(),
        vec![
            ("old.txt".to_string(), false),
            ("new.txt".to_string(), true),
            ("old.txt ↔ new.txt (unsaved)".to_string(), false),
        ]
    );
    show_tab(&mut app, "new.txt");
    assert!(app.snapshot().unwrap().0.starts_with("fresh\na\nX\n"));
    assert_eq!(std::fs::read_to_string(&old).unwrap(), OLD);
    // An unsaved left side is labelled too.
    let mut app = new_app();
    std::fs::write(&new, NEW).unwrap();
    split(&mut app, &old, &new);
    key(&mut app, "ctrl+1");
    app.handle(Input::Text("z".into()));
    run_command(&mut app, "compare left");
    assert_eq!(app.title(), "old.txt (unsaved) ↔ new.txt - litedit");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn split_compare_says_why_when_it_does_not_apply() {
    let (d, old, new) = pair("split-na");
    let mut app = new_app();
    // Nothing open, then one file, then two tabs on one side: no split.
    run_command(&mut app, "compare left");
    assert!(
        status(&app).contains("needs a split view"),
        "{}",
        status(&app)
    );
    assert_eq!(app.tab_count(), 1);
    app.open(&old);
    app.open(&new);
    run_command(&mut app, "compare left");
    assert!(
        status(&app).contains("needs a split view"),
        "{}",
        status(&app)
    );
    assert_eq!(app.tab_count(), 2);
    assert!(app.title().starts_with("new.txt"), "{}", app.title());
    // The tab menu only offers it when it works.
    let open_menu = |app: &mut App| {
        let (bar, rects) = app.tab_layout().unwrap();
        let r = rects[0];
        mouse(
            app,
            (r.x + r.w / 2) as f32,
            (bar.y + bar.h / 2) as f32,
            MouseButton::Right,
        );
        let labels = app.menu_labels();
        key(app, "escape");
        labels
    };
    assert!(!open_menu(&mut app).contains(&"Compare Left ↔ Right".to_string()));
    key(&mut app, "ctrl+alt+right");
    assert!(open_menu(&mut app).contains(&"Compare Left ↔ Right".to_string()));
    // A comparison showing on one side is not a file.
    app.compare_files(&old, &new);
    assert_eq!(app.title(), "old.txt ↔ new.txt - litedit");
    let n = app.tab_count();
    run_command(&mut app, "compare left");
    assert!(
        status(&app).contains("needs a file on both sides"),
        "{}",
        status(&app)
    );
    assert_eq!(app.tab_count(), n);
    assert_eq!(app.title(), "old.txt ↔ new.txt - litedit");
    assert!(!open_menu(&mut app).contains(&"Compare Left ↔ Right".to_string()));
    assert!(no_tab_is_dirty(&app));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn split_compare_names_the_folders_of_same_named_files_and_caps_size() {
    let d = tmp_dir("cmp-samename");
    std::fs::create_dir_all(d.join("v1")).unwrap();
    std::fs::create_dir_all(d.join("v2")).unwrap();
    let a = write(&d.join("v1"), "conf.txt", "x = 1\n");
    let b = write(&d.join("v2"), "conf.txt", "x = 2\n");
    let mut app = new_app();
    split(&mut app, &a, &b);
    run_command(&mut app, "compare left");
    assert_eq!(app.title(), "v1/conf.txt ↔ v2/conf.txt - litedit");
    assert!(status(&app).starts_with("1 change"), "{}", status(&app));
    // Too many lines on one side: refused with a message, nothing hangs.
    std::fs::write(&b, "x\n".repeat(crate::diff::MAX_LINES + 10)).unwrap();
    let mut app = new_app();
    split(&mut app, &a, &b);
    let n = app.tab_count();
    let t = std::time::Instant::now();
    run_command(&mut app, "compare left");
    assert!(t.elapsed() < std::time::Duration::from_secs(5));
    assert!(
        app.nag_message()
            .unwrap_or_default()
            .contains("Cannot compare"),
        "{:?}",
        app.nag_message()
    );
    key(&mut app, "escape");
    assert_eq!(app.tab_count(), n);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_file_of_lone_cr_lines_is_refused_by_compare_quickly() {
    let d = tmp_dir("cmp-cr");
    let a = write(&d, "a.txt", "x\n");
    let b = write(&d, "b.txt", "x\n");
    std::fs::write(&b, "\r".repeat(crate::diff::MAX_LINES + 10)).unwrap();
    let mut app = new_app();
    split(&mut app, &a, &b);
    let n = app.tab_count();
    let t = std::time::Instant::now();
    run_command(&mut app, "compare left");
    assert!(t.elapsed() < std::time::Duration::from_secs(5));
    assert!(
        app.nag_message()
            .unwrap_or_default()
            .contains("Cannot compare"),
        "{:?}",
        app.nag_message()
    );
    key(&mut app, "escape");
    assert_eq!(app.tab_count(), n);
    let _ = std::fs::remove_dir_all(&d);
}

// ---- 2. ignore whitespace -------------------------------------------------

/// Re-indented with tabs, CRLF endings, trailing blanks, doubled spaces —
/// and one real change (b = 2 → 3).
const WS_OLD: &str =
    "fn main() {\n    let a = 1;\n    let b = 2;\n\n    if a < b {\n        done();\n    }\n}\n";
const WS_NEW: &str = "fn main() {\r\n\tlet a  =  1;\r\n\tlet b = 3;\r\n  \r\n\tif a < b {\r\n\t\tdone();   \r\n\t}\r\n}\r\n";

fn ws_pair(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
    let d = tmp_dir(&format!("cmp-{tag}"));
    let (o, n) = (write(&d, "old.rs", WS_OLD), write(&d, "new.rs", WS_NEW));
    (d, o, n)
}

#[test]
fn alt_w_ignores_whitespace_and_counts_only_real_changes() {
    let (d, old, new) = ws_pair("ws");
    let mut app = new_app();
    app.compare_files(&old, &new);
    // Lines 2-7 all differ in whitespace: one block of changes.
    assert!(status(&app).starts_with("1 change "), "{}", status(&app));
    assert!(status(&app).contains("Alt+W: ignore whitespace"));
    assert!(!status(&app).contains("Ignoring whitespace"));
    key(&mut app, "f7");
    assert!(status(&app).contains("Change 1 of 1"));
    key(&mut app, "alt+w");
    assert!(status(&app).starts_with("1 change "), "{}", status(&app));
    assert!(
        status(&app).contains("Ignoring whitespace"),
        "{}",
        status(&app)
    );
    assert!(
        !status(&app).contains("Change 1 of"),
        "recomputed: no current"
    );
    // The one change left is the real one, on line 3.
    key(&mut app, "f7");
    key(&mut app, "enter");
    assert!(app.title().starts_with("new.rs"), "{}", app.title());
    assert!(status(&app).starts_with("Ln 3, "), "{}", status(&app));
    show_tab(&mut app, "old.rs ↔ new.rs");
    // The real change goes away on disk: only whitespace is left, and the
    // option survives the recompute.
    std::fs::write(&new, WS_NEW.replace("b = 3", "b = 2")).unwrap();
    app.handle(Input::FsChanged(vec![new.clone()]));
    assert!(
        status(&app).starts_with("Only whitespace differs"),
        "{}",
        status(&app)
    );
    assert!(status(&app).contains("Ignoring whitespace"));
    let only_ws = render_hash(&mut app, 800, 600);
    // Alt+W again: whitespace counts, lines 2-7 are one block of changes.
    key(&mut app, "alt+w");
    assert!(status(&app).starts_with("1 change "), "{}", status(&app));
    assert!(!status(&app).contains("Ignoring whitespace"));
    // Truly identical files say so, and look different from "only
    // whitespace differs".
    key(&mut app, "alt+w");
    std::fs::write(&new, WS_OLD).unwrap();
    app.handle(Input::FsChanged(vec![new.clone()]));
    assert!(status(&app).starts_with("No changes"), "{}", status(&app));
    assert_ne!(render_hash(&mut app, 800, 600), only_ws);
    assert!(no_tab_is_dirty(&app));
    assert_eq!(std::fs::read_to_string(&old).unwrap(), WS_OLD);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn ignore_whitespace_from_the_palette_and_its_hint_elsewhere() {
    let (d, old, new) = ws_pair("ws-palette");
    let mut app = new_app();
    // Not in a comparison: a hint, nothing else.
    run_command(&mut app, "ignore whitespace");
    assert!(
        status(&app).contains("Ignore Whitespace works in a comparison"),
        "{}",
        status(&app)
    );
    app.open(&old);
    run_command(&mut app, "ignore whitespace");
    assert!(
        status(&app).contains("Ignore Whitespace works in a comparison"),
        "{}",
        status(&app)
    );
    key(&mut app, "alt+w");
    assert_eq!(app.snapshot().unwrap().0, WS_OLD, "Alt+W edits nothing");
    assert!(no_tab_is_dirty(&app));
    app.compare_files(&old, &new);
    std::fs::write(&new, WS_NEW.replace("b = 3", "b = 2")).unwrap();
    app.handle(Input::FsChanged(vec![new.clone()]));
    assert!(status(&app).starts_with("1 change "), "{}", status(&app));
    run_command(&mut app, "ignore whitespace");
    assert!(
        status(&app).starts_with("Only whitespace differs"),
        "{}",
        status(&app)
    );
    assert!(status(&app).contains("Ignoring whitespace"));
    run_command(&mut app, "ignore whitespace");
    assert!(status(&app).starts_with("1 change "), "{}", status(&app));
    assert!(!status(&app).contains("Ignoring whitespace"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn ignore_whitespace_redraws_like_a_fresh_view_and_keeps_its_place() {
    let d = tmp_dir("cmp-ws-render");
    // Long enough to scroll; whitespace-only and real changes in view.
    let old: String = (0..300).map(|i| format!("    line {i}\n")).collect();
    let new: String = (0..300)
        .map(|i| match i {
            40 | 41 | 60 => format!("\tline {i}  \n"),
            45 => "    line forty-five\n".to_string(),
            _ => format!("    line {i}\n"),
        })
        .collect();
    let (o, n) = (write(&d, "old.txt", &old), write(&d, "new.txt", &new));
    let scroll = |app: &mut App| {
        for _ in 0..10 {
            app.handle(Input::Wheel {
                dx: 0.0,
                dy: 1.0,
                unit: crate::input::WheelUnit::Lines,
            });
        }
    };
    theme::set_light(false);
    // a: drawn with whitespace changes first, then Alt+W.
    let mut a = new_app();
    a.compare_files(&o, &n);
    render_hash(&mut a, 1000, 700);
    let mid = app_mid(&mut a);
    a.handle(Input::MouseMove { x: mid.0, y: mid.1 });
    scroll(&mut a);
    let strict = settled_hash(&mut a);
    let y = a.view_scroll_y();
    assert!(y > 0.0);
    key(&mut a, "alt+w");
    assert_eq!(a.view_scroll_y(), y, "same rows: same place");
    let loose = settled_hash(&mut a);
    assert_ne!(strict, loose, "highlights of whitespace changes are gone");
    // b: Alt+W before anything was drawn at that place.
    let mut b = new_app();
    b.compare_files(&o, &n);
    key(&mut b, "alt+w");
    render_hash(&mut b, 1000, 700);
    b.handle(Input::MouseMove { x: mid.0, y: mid.1 });
    scroll(&mut b);
    assert_eq!(settled_hash(&mut b), loose, "no stale rows after Alt+W");
    // Off again: exactly as before.
    key(&mut a, "alt+w");
    assert_eq!(settled_hash(&mut a), strict);
    assert_eq!(a.view_scroll_y(), y);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn ignoring_whitespace_highlights_only_the_real_change_in_a_line() {
    // The same real change (1 → 2), once with the indentation changed from
    // spaces to a tab as well, once without. A tab is drawn as 4 columns.
    let d = tmp_dir("cmp-ws-marks");
    std::fs::create_dir_all(d.join("tab")).unwrap();
    std::fs::create_dir_all(d.join("plain")).unwrap();
    let old = "fn f() {\n    let x = 1;\n}\n";
    let t_old = write(&d.join("tab"), "old.rs", old);
    let t_new = write(&d.join("tab"), "new.rs", "fn f() {\n\tlet x = 2;\n}\n");
    let p_old = write(&d.join("plain"), "old.rs", old);
    let p_new = write(&d.join("plain"), "new.rs", "fn f() {\n    let x = 2;\n}\n");
    theme::set_light(false);
    let shot = |a: &Path, b: &Path, ignore: bool| {
        let mut app = new_app();
        app.compare_files(a, b);
        if ignore {
            key(&mut app, "alt+w");
        }
        // The status bar says "Ignoring whitespace" either way.
        settled_hash(&mut app)
    };
    assert_ne!(
        shot(&t_old, &t_new, false),
        shot(&p_old, &p_new, false),
        "whitespace counts: the changed indentation is highlighted"
    );
    assert_eq!(
        shot(&t_old, &t_new, true),
        shot(&p_old, &p_new, true),
        "whitespace ignored: only the 1 → 2 is highlighted"
    );
    let _ = std::fs::remove_dir_all(&d);
}

fn app_mid(app: &mut App) -> (f32, f32) {
    let m = app.main_rect();
    ((m.x + m.w / 2) as f32, (m.y + m.h / 2) as f32)
}

// ---- 3. jump to the file --------------------------------------------------

#[test]
fn double_click_opens_that_side_at_that_line() {
    let (d, old, new) = pair("jump");
    let mut app = new_app();
    app.open(&old);
    app.compare_files(&old, &new);
    let cmp = "old.txt ↔ new.txt";
    // (side, row, file, line): see the rows at NEW.
    for (side, row, file, line) in [
        (0, 4, "old.txt", 3),
        (1, 4, "new.txt", 5),
        (0, 5, "old.txt", 4),
        (1, 1, "new.txt", 2),
        (1, 6, "new.txt", 6),
        // Filler rows (nothing on that side): the line above.
        (0, 2, "old.txt", 1),
        (1, 5, "new.txt", 5),
    ] {
        show_tab(&mut app, cmp);
        double_click_row(&mut app, side, row);
        assert!(
            app.title().starts_with(file),
            "side {side} row {row}: {}",
            app.title()
        );
        assert!(
            status(&app).starts_with(&format!("Ln {line}, Col 1 ")),
            "side {side} row {row}: {}",
            status(&app)
        );
    }
    // Each file is one normal tab; the comparison is still there.
    assert_eq!(titles(&app), ["old.txt", cmp, "new.txt"]);
    // Places that are not a row do nothing.
    show_tab(&mut app, cmp);
    let (x, y) = row_xy(&mut app, 1, 0);
    let header_y = app.main_rect().y as f32 + 5.0;
    double_click(&mut app, x, header_y);
    assert_eq!(app.title(), format!("{cmp} - litedit"));
    let (_, below) = row_xy(&mut app, 1, 9);
    double_click(&mut app, x, below);
    assert_eq!(app.title(), format!("{cmp} - litedit"));
    // A single click doesn't open anything either.
    mouse(&mut app, x, y, MouseButton::Left);
    assert_eq!(app.title(), format!("{cmp} - litedit"));
    assert!(no_tab_is_dirty(&app));
    assert_eq!(std::fs::read_to_string(&old).unwrap(), OLD);
    assert_eq!(std::fs::read_to_string(&new).unwrap(), NEW);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn enter_opens_the_current_change_and_a_lone_comparison_stays_open() {
    let (d, old, new) = pair("jump-enter");
    let mut app = new_app();
    // The comparison is the only tab (compare from the tree, nothing open).
    app.compare_files(&old, &new);
    assert_eq!(app.tab_count(), 1);
    let cmp = "old.txt ↔ new.txt";
    // No current change yet: Enter does nothing.
    key(&mut app, "enter");
    assert_eq!(app.title(), format!("{cmp} - litedit"));
    assert!(app.take_requests().is_empty());
    key(&mut app, "f7"); // change 1: rows 1-2 (+X +Y)
    key(&mut app, "enter");
    assert!(app.title().starts_with("new.txt"), "{}", app.title());
    assert!(status(&app).starts_with("Ln 2, Col 1 "), "{}", status(&app));
    assert_eq!(titles(&app), [cmp, "new.txt"], "the comparison was kept");
    show_tab(&mut app, cmp);
    assert!(status(&app).contains("Change 1 of 2"), "{}", status(&app));
    key(&mut app, "f7"); // change 2: row 5 (-d)
    key(&mut app, "shift+enter");
    assert!(app.title().starts_with("old.txt"), "{}", app.title());
    assert!(status(&app).starts_with("Ln 4, Col 1 "), "{}", status(&app));
    assert_eq!(titles(&app), [cmp, "old.txt", "new.txt"]);
    // Enter on the removed line: the right side has no such line, so it
    // lands on the line above it there.
    show_tab(&mut app, cmp);
    key(&mut app, "enter");
    assert!(app.title().starts_with("new.txt"), "{}", app.title());
    assert!(status(&app).starts_with("Ln 5, Col 1 "), "{}", status(&app));
    assert_eq!(app.tab_count(), 3);
    assert!(no_tab_is_dirty(&app));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_peek_from_the_tree_does_not_replace_a_lone_comparison() {
    let (d, old, new) = pair("jump-peek");
    let mut app = new_app();
    app.open(&d);
    app.compare_files(&old, &new);
    assert_eq!(app.tab_count(), 1);
    crate::tests::click_tree(&mut app, &old, 1);
    assert_eq!(titles(&app), ["old.txt ↔ new.txt", "old.txt"]);
    assert_eq!(app.peek_tabs(), [false, true]);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn jumping_from_compare_with_saved_lands_in_the_edited_tab() {
    let (d, old, _new) = pair("jump-saved");
    let mut app = new_app();
    app.open(&old);
    key(&mut app, "ctrl+home");
    app.handle(Input::Text("one\ntwo\n".into()));
    run_command(&mut app, "compare with saved");
    let cmp = "old.txt (saved) ↔ old.txt (unsaved)";
    assert_eq!(app.title(), format!("{cmp} - litedit"));
    // Rows: +one | +two | a= | b= | c= ...  Row 4 is "c": line 3 saved,
    // line 5 in the tab that opens (it shows the unsaved text).
    for side in [0, 1] {
        show_tab(&mut app, cmp);
        double_click_row(&mut app, side, 4);
        assert!(app.title().starts_with("● old.txt"), "{}", app.title());
        assert!(status(&app).starts_with("Ln 5, Col 1 "), "{}", status(&app));
    }
    assert_eq!(
        app.tab_titles(),
        vec![("old.txt".to_string(), true), (cmp.to_string(), false)]
    );
    assert_eq!(app.snapshot().unwrap().0, format!("one\ntwo\n{OLD}"));
    assert_eq!(std::fs::read_to_string(&old).unwrap(), OLD);
    let _ = std::fs::remove_dir_all(&d);
}

// ---- 4. compare with the clipboard ---------------------------------------

#[test]
fn clipboard_compare_shows_clipboard_against_the_current_text() {
    let (d, old, new) = pair("clip");
    let mut app = new_app();
    app.open(&new);
    // Several lines, CRLF endings: the same text as old.txt.
    set_clipboard(&mut app, Some("a\r\nb\r\nc\r\nd\r\ne\r\n"));
    run_command(&mut app, "compare with clipboard");
    assert_eq!(app.title(), "Clipboard ↔ new.txt - litedit");
    assert!(status(&app).starts_with("2 changes"), "{}", status(&app));
    assert_eq!(
        app.clipboard_text().as_deref(),
        Some("a\r\nb\r\nc\r\nd\r\ne\r\n")
    );
    render_hash(&mut app, 800, 600);
    // The clipboard side has no file: a double-click there does nothing…
    double_click_row(&mut app, 0, 0);
    assert_eq!(app.title(), "Clipboard ↔ new.txt - litedit");
    // …and Enter goes to the file side.
    key(&mut app, "f7");
    key(&mut app, "enter");
    assert!(app.title().starts_with("new.txt"), "{}", app.title());
    assert!(status(&app).starts_with("Ln 2, Col 1 "), "{}", status(&app));
    // Shift+Enter asks for the clipboard side: nothing to open.
    show_tab(&mut app, "Clipboard ↔ new.txt");
    key(&mut app, "shift+enter");
    assert_eq!(app.title(), "Clipboard ↔ new.txt - litedit");
    // Line endings alone are no difference.
    app.open(&old);
    run_command(&mut app, "compare with clipboard");
    assert_eq!(app.title(), "Clipboard ↔ old.txt - litedit");
    assert!(status(&app).starts_with("No changes"), "{}", status(&app));
    // Unsaved edits are what gets compared, and they stay unsaved.
    show_tab(&mut app, "old.txt");
    key(&mut app, "ctrl+home");
    app.handle(Input::Text("top\n".into()));
    run_command(&mut app, "compare with clipboard");
    assert_eq!(app.title(), "Clipboard ↔ old.txt (unsaved) - litedit");
    assert!(status(&app).starts_with("1 change "), "{}", status(&app));
    double_click_row(&mut app, 1, 3);
    assert!(app.title().starts_with("● old.txt"), "{}", app.title());
    assert!(status(&app).starts_with("Ln 4, Col 1 "), "{}", status(&app));
    let dirty: Vec<String> = app
        .tab_titles()
        .into_iter()
        .filter(|(_, d)| *d)
        .map(|(t, _)| t)
        .collect();
    assert_eq!(dirty, ["old.txt"]);
    assert_eq!(std::fs::read_to_string(&old).unwrap(), OLD);
    assert_eq!(std::fs::read_to_string(&new).unwrap(), NEW);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn clipboard_compare_handles_empty_huge_and_no_file() {
    let (d, old, _new) = pair("clip-na");
    let mut app = new_app();
    // No file tab: a hint (even with text on the clipboard).
    set_clipboard(&mut app, Some("text"));
    run_command(&mut app, "compare with clipboard");
    assert!(
        status(&app).contains("Compare with Clipboard works in a file tab"),
        "{}",
        status(&app)
    );
    assert_eq!(app.tab_count(), 1);
    app.open(&old);
    // Empty clipboard: a hint, not an error.
    for empty in [None, Some("")] {
        set_clipboard(&mut app, empty);
        run_command(&mut app, "compare with clipboard");
        assert!(
            status(&app).contains("The clipboard has no text to compare"),
            "{}",
            status(&app)
        );
        assert!(app.nag_message().is_none());
        assert_eq!(app.tab_count(), 1);
        assert!(app.title().starts_with("old.txt"));
    }
    // More than the 20 MB cap, and more lines than the cap: refused.
    let big = "x".repeat(crate::diff::MAX_BYTES + 1);
    let many = "y\n".repeat(crate::diff::MAX_LINES + 10);
    for huge in [big.as_str(), many.as_str()] {
        set_clipboard(&mut app, Some(huge));
        let t = std::time::Instant::now();
        run_command(&mut app, "compare with clipboard");
        assert!(t.elapsed() < std::time::Duration::from_secs(5));
        assert!(
            app.nag_message()
                .unwrap_or_default()
                .starts_with("Cannot compare: "),
            "{:?}",
            app.nag_message()
        );
        key(&mut app, "escape");
        assert_eq!(app.tab_count(), 1);
    }
    // In a comparison (not a file tab): the hint again.
    set_clipboard(&mut app, Some("a\n"));
    run_command(&mut app, "compare with clipboard");
    assert_eq!(app.title(), "Clipboard ↔ old.txt - litedit");
    run_command(&mut app, "compare with clipboard");
    assert!(
        status(&app).contains("Compare with Clipboard works in a file tab"),
        "{}",
        status(&app)
    );
    assert_eq!(app.tab_count(), 2);
    // Odd clipboard content: no panic.
    show_tab(&mut app, "old.txt");
    set_clipboard(&mut app, Some("\u{feff}\0\r\r\n\t\u{202e}x\u{0301}\n\n\n"));
    run_command(&mut app, "compare with clipboard");
    for k in ["f7", "alt+w", "enter", "ctrl+end", "shift+f7"] {
        key(&mut app, k);
        render_hash(&mut app, 800, 600);
    }
    assert!(no_tab_is_dirty(&app));
    assert_eq!(std::fs::read_to_string(&old).unwrap(), OLD);
    let _ = std::fs::remove_dir_all(&d);
}

// ---- 5. themes, caches, session ------------------------------------------

#[test]
fn new_compare_states_theme_switch_matches_fresh() {
    let (d, old, new) = ws_pair("theme");
    let same_ws = write(&d, "same.rs", &WS_NEW.replace("b = 3", "b = 2"));
    type Scene = Box<dyn Fn(&mut App)>;
    let (o, n, s) = (old.clone(), new.clone(), same_ws.clone());
    let scenes: Vec<(&str, Scene)> = vec![
        ("ignoring whitespace, change current", {
            let (o, n) = (o.clone(), n.clone());
            Box::new(move |app| {
                app.compare_files(&o, &n);
                key(app, "alt+w");
                key(app, "f7");
            })
        }),
        ("only whitespace differs", {
            let (o, s) = (o.clone(), s.clone());
            Box::new(move |app| {
                app.compare_files(&o, &s);
                key(app, "alt+w");
            })
        }),
        ("split compare", {
            let (o, n) = (o.clone(), n.clone());
            Box::new(move |app| {
                split(app, &o, &n);
                run_command(app, "compare left");
                key(app, "alt+w");
            })
        }),
        ("clipboard", {
            let n = n.clone();
            Box::new(move |app| {
                app.open(&n);
                set_clipboard(app, Some(WS_OLD));
                run_command(app, "compare with clipboard");
                key(app, "f7");
            })
        }),
    ];
    for (name, scene) in &scenes {
        theme::set_light(false);
        let mut a = new_app();
        scene(&mut a);
        let dark = settled_hash(&mut a);
        a.set_light(true);
        let switched = settled_hash(&mut a);
        let mut b = new_app();
        scene(&mut b);
        assert_eq!(switched, settled_hash(&mut b), "{name}: switched vs fresh");
        assert_ne!(dark, switched, "{name}: light differs from dark");
        a.set_light(false);
        assert_eq!(settled_hash(&mut a), dark, "{name}: back to dark");
    }
    theme::set_light(false);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn comparisons_free_caches_when_hidden_and_stay_out_of_the_workspace() {
    let (d, old, new) = pair("ws-session");
    let mut app = new_app();
    split(&mut app, &old, &new);
    run_command(&mut app, "compare left");
    key(&mut app, "alt+w");
    show_tab(&mut app, "new.txt");
    set_clipboard(&mut app, Some("a\nb\n"));
    run_command(&mut app, "compare with clipboard");
    key(&mut app, "alt+w");
    assert_eq!(
        titles(&app),
        [
            "old.txt",
            "new.txt",
            "Clipboard ↔ new.txt",
            "old.txt ↔ new.txt"
        ]
    );
    // Drawn, the active comparison holds shaped rows; hidden, none.
    render_hash(&mut app, 800, 600);
    assert_eq!(app.tabs_with_caches()[2..], [true, false]);
    show_tab(&mut app, "old.txt ↔ new.txt");
    render_hash(&mut app, 800, 600);
    assert_eq!(app.tabs_with_caches()[2..], [false, true]);
    show_tab(&mut app, "new.txt");
    render_hash(&mut app, 800, 600);
    assert_eq!(app.tabs_with_caches()[2..], [false, false]);
    // With a comparison active, the workspace and session list only the
    // files, and point at a valid tab.
    show_tab(&mut app, "old.txt ↔ new.txt");
    let w = app.current_workspace();
    assert_eq!(w.tabs, vec![(old.clone(), 0), (new.clone(), 1)]);
    assert!(w.active < w.tabs.len());
    let file = d.join("saved.litedit-workspace");
    w.save(&file).unwrap();
    let s = app.session();
    assert_eq!(s.tabs, vec![old.clone(), new.clone()]);
    assert!(s.active < s.tabs.len().max(1));
    // Opening that workspace again brings the files back, split, unharmed.
    let mut again = new_app();
    again.open(&file);
    assert_eq!(titles(&again), ["old.txt", "new.txt"]);
    assert!(again.is_split_for_test());
    assert!(no_tab_is_dirty(&app) && no_tab_is_dirty(&again));
    assert_eq!(std::fs::read_to_string(&old).unwrap(), OLD);
    assert_eq!(std::fs::read_to_string(&new).unwrap(), NEW);
    let _ = std::fs::remove_dir_all(&d);
}

// ---- 6. hardening: questions, missing files, non-text tabs ----------------

#[test]
fn comparing_under_an_open_question_changes_nothing() {
    let (d, old, new) = pair("nag");
    let mut app = new_app();
    split(&mut app, &old, &new);
    // A question is showing (too-big clipboard).
    set_clipboard(&mut app, Some(&"x".repeat(crate::diff::MAX_BYTES + 1)));
    run_command(&mut app, "compare with clipboard");
    let q = app.nag_message().expect("question");
    assert!(q.starts_with("Cannot compare: "), "{q}");
    let (tabs, title) = (app.tab_count(), app.title());
    // Every way in is refused while it shows; the question stays as it was.
    set_clipboard(&mut app, Some("a\n"));
    app.compare_with_clipboard();
    app.compare_sides();
    app.compare_files(&old, &new);
    assert_eq!(app.nag_message().as_deref(), Some(q.as_str()));
    assert_eq!((app.tab_count(), app.title()), (tabs, title));
    assert!(status(&app).contains("Answer the question above first"));
    // Answered, the same command works.
    key(&mut app, "escape");
    assert!(app.nag_message().is_none());
    app.compare_with_clipboard();
    assert_eq!(app.tab_count(), tabs + 1);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn jumping_to_a_deleted_or_closed_file_is_safe() {
    let (d, old, new) = pair("gone");
    let mut app = new_app();
    // Neither file is open: Enter opens the right one at the line.
    app.compare_files(&old, &new);
    render_hash(&mut app, 800, 600);
    key(&mut app, "f7");
    key(&mut app, "enter");
    assert!(app.title().starts_with("new.txt"), "{}", app.title());
    // Deleted after the comparison was made: no panic, nothing dirty, the
    // comparison stays.
    show_tab(&mut app, "old.txt ↔ new.txt");
    std::fs::remove_file(&new).unwrap();
    let n = app.tab_count();
    for _ in 0..2 {
        double_click_row(&mut app, 1, 2);
        key(&mut app, "enter");
        render_hash(&mut app, 800, 600);
    }
    // (The already open new.txt tab is orphaned by the delete: that is the
    // existing "deleted on disk" handling, not the comparison's doing.)
    let dirty: Vec<String> = app
        .tab_titles()
        .into_iter()
        .filter(|(_, d)| *d)
        .map(|(t, _)| t)
        .collect();
    assert!(dirty.iter().all(|t| t == "new.txt"), "{dirty:?}");
    assert!(titles(&app).contains(&"old.txt ↔ new.txt".to_string()));
    assert!(app.tab_count() >= n);
    // Shift+Enter still reaches the file that exists.
    show_tab(&mut app, "old.txt ↔ new.txt");
    key(&mut app, "shift+enter");
    assert_eq!(std::fs::read_to_string(&old).unwrap(), OLD);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_comparison_is_never_saved_or_edited() {
    let (d, old, new) = pair("noedit");
    let mut app = new_app();
    split(&mut app, &old, &new);
    run_command(&mut app, "compare left");
    let t = app.title();
    for k in [
        "ctrl+s",
        "ctrl+shift+s",
        "ctrl+z",
        "ctrl+y",
        "ctrl+x",
        "ctrl+v",
        "ctrl+a",
        "delete",
        "backspace",
        "tab",
    ] {
        key(&mut app, k);
        key(&mut app, "escape");
    }
    app.handle(Input::Text("typed".into()));
    run_command(&mut app, "sort lines");
    assert_eq!(app.title(), t);
    assert!(no_tab_is_dirty(&app));
    assert_eq!(std::fs::read_to_string(&old).unwrap(), OLD);
    assert_eq!(std::fs::read_to_string(&new).unwrap(), NEW);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn non_text_tabs_are_refused_with_a_notice() {
    let d = tmp_dir("cmp-nontext");
    let md = write(&d, "n.md", "# Title\n\nsome *text*\n");
    let txt = write(&d, "t.txt", "# Title\n\nother\n");
    let bin = d.join("b.bin");
    std::fs::write(&bin, [0u8, 159, 146, 150, 0, 0, 1, 2]).unwrap();
    let mut app = new_app();
    // Start page: nothing to compare.
    set_clipboard(&mut app, Some("x\n"));
    app.compare_with_clipboard();
    app.compare_sides();
    assert_eq!(app.tab_count(), 1);
    // A markdown preview compares its source, like any file.
    app.open(&md);
    app.open(&txt);
    key(&mut app, "ctrl+alt+right");
    app.compare_sides();
    assert_eq!(app.title(), "n.md ↔ t.txt - litedit");
    // Binary / unreadable file in the split: no panic, no half-made tab.
    app.open(&bin);
    key(&mut app, "ctrl+alt+right");
    let n = app.tab_count();
    app.compare_sides();
    app.compare_with_clipboard();
    render_hash(&mut app, 800, 600);
    assert!(app.tab_count() <= n + 1);
    assert!(no_tab_is_dirty(&app));
    let _ = std::fs::remove_dir_all(&d);
}

// ---- from the independent review of Epic 7 ----

#[test]
fn a_file_error_that_clears_never_leaves_rows_of_the_old_text() {
    let d = tmp_dir("cmp-stale");
    let body = |mark: usize| -> String {
        (0..200)
            .map(|i| {
                if i == mark {
                    format!("changed {i}\n")
                } else {
                    format!("line {i}\n")
                }
            })
            .collect()
    };
    let (o, n) = (
        write(&d, "old.txt", &body(usize::MAX)),
        write(&d, "new.txt", &body(150)),
    );
    let mut app = new_app();
    app.compare_files(&o, &n);
    key(&mut app, "ctrl+end");
    // One side shrinks while the other is briefly missing…
    std::fs::write(&o, "short\n").unwrap();
    std::fs::remove_file(&n).unwrap();
    app.handle(Input::FsChanged(vec![o.clone(), n.clone()]));
    render_hash(&mut app, 900, 500);
    // …and comes back unchanged: the rows must be those of the new texts.
    std::fs::write(&n, body(150)).unwrap();
    app.handle(Input::FsChanged(vec![n.clone()]));
    render_hash(&mut app, 900, 500);
    key(&mut app, "ctrl+home");
    render_hash(&mut app, 900, 500);
    key(&mut app, "ctrl+end");
    render_hash(&mut app, 900, 500);
    // Same picture as a comparison of these two files made from scratch.
    let now = settled_hash(&mut app);
    let mut fresh = new_app();
    fresh.compare_files(&o, &n);
    key(&mut fresh, "ctrl+end");
    let other = settled_hash(&mut fresh);
    assert_eq!(now, other);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_clipboard_of_line_breaks_is_refused_before_it_is_split() {
    let d = tmp_dir("cmp-breaks");
    let f = write(&d, "a.txt", OLD);
    let mut app = new_app();
    app.open(&f);
    let tabs = app.tab_titles().len();
    set_clipboard(&mut app, Some(&"\n".repeat(3_000_000)));
    let t = std::time::Instant::now();
    run_command(&mut app, "compare with clipboard");
    assert!(
        t.elapsed() < std::time::Duration::from_secs(2),
        "{:?}",
        t.elapsed()
    );
    assert_eq!(app.tab_titles().len(), tabs, "no comparison tab");
    let _ = std::fs::remove_dir_all(&d);
}
