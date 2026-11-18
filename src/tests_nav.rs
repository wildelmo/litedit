//! S7.3: Back / Forward and "files reopen where you left off", driven
//! through the app like a user would.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::app::App;
use crate::input::{Cursor, Input, MouseButton, WheelUnit};
use crate::session::Session;
use crate::tests::{
    click_tree, fixture, key, many_files, mouse, new_app, render_hash, run_command, run_menu_item,
    tmp_dir, type_text,
};
use crate::view::ViewPos;

const W: u32 = 800;
const H: u32 = 600;

fn draw(app: &mut App) -> u64 {
    app.settle(Duration::from_secs(5));
    render_hash(app, W, H)
}

fn wheel(app: &mut App, notches: f32) {
    // Over the editor area.
    app.handle(Input::MouseMove { x: 500.0, y: 300.0 });
    app.handle(Input::Wheel {
        dx: 0.0,
        dy: notches,
        unit: WheelUnit::Lines,
    });
}

fn goto_line(app: &mut App, n: usize) {
    key(app, "ctrl+g");
    type_text(app, &n.to_string());
    key(app, "enter");
}

fn goto_heading(app: &mut App, name: &str) {
    key(app, "ctrl+shift+g");
    type_text(app, name);
    key(app, "enter");
}

fn back(app: &mut App) {
    key(app, "alt+left");
}

fn forward(app: &mut App) {
    key(app, "alt+right");
}

fn ln(app: &App) -> String {
    let s = app.status_text();
    match s.find("Ln ") {
        Some(i) => s[i..].split(',').next().unwrap_or("").to_string(),
        None => format!("(no caret: {s})"),
    }
}

fn titles(app: &App) -> Vec<String> {
    app.tab_titles().into_iter().map(|(t, _)| t).collect()
}

fn close(a: f32, b: f32, tol: f32) -> bool {
    (a - b).abs() <= tol
}

/// Window y of each link in the preview, top to bottom (the links start at
/// the left edge of the page, the tree is hidden).
fn link_rows(app: &mut App) -> Vec<f32> {
    draw(app);
    let mut rows = Vec::new();
    let mut inside = false;
    let mut y = 45.0;
    while y < H as f32 - 40.0 {
        app.handle(Input::MouseMove { x: 50.0, y });
        let on = app.cursor() == Cursor::Pointer;
        if on && !inside {
            rows.push(y + 3.0);
        }
        inside = on;
        y += 2.0;
    }
    app.handle(Input::MouseMove { x: 500.0, y: 300.0 });
    rows
}

fn click(app: &mut App, x: f32, y: f32) {
    app.handle(Input::MouseMove { x, y });
    app.handle(Input::MouseDown {
        x,
        y,
        button: MouseButton::Left,
        clicks: 1,
    });
    app.handle(Input::MouseUp {
        x,
        y,
        button: MouseButton::Left,
    });
}

/// index.md (two links at the top, two headings far below), other.md (a
/// long document), code.txt (600 numbered lines).
fn library(tag: &str) -> PathBuf {
    let d = tmp_dir(&format!("nav-{tag}"));
    let mut index = String::from("[deep](#deep-section)\n\n[other](other.md)\n\n");
    for i in 0..50 {
        index.push_str(&format!("para {i} of the index\n\n"));
    }
    index.push_str("## Deep section\n\n");
    for i in 0..50 {
        index.push_str(&format!("middle {i}\n\n"));
    }
    index.push_str("## Far section\n\n");
    for i in 0..50 {
        index.push_str(&format!("tail {i}\n\n"));
    }
    std::fs::write(d.join("index.md"), index).unwrap();
    let mut other = String::from("# Other\n\n");
    for i in 0..150 {
        other.push_str(&format!(
            "Paragraph {i} of the other document, long enough to wrap when the window is narrow \
             and to fill a good part of a line when it is wide.\n\n"
        ));
    }
    std::fs::write(d.join("other.md"), other).unwrap();
    let code: String = (1..=600).map(|i| format!("line {i}\n")).collect();
    std::fs::write(d.join("code.txt"), code).unwrap();
    d
}

fn line_of(path: &Path, needle: &str) -> usize {
    let text = std::fs::read_to_string(path).unwrap();
    text[..text.find(needle).unwrap()].matches('\n').count()
}

#[test]
fn back_and_forward_retrace_links_headings_lines_and_tab_switches() {
    let d = library("walk");
    let index = d.join("index.md");
    let mut app = new_app();
    // Both tabs exist from the start, so pictures can be compared.
    app.open(&d.join("code.txt"));
    app.open(&index);
    key(&mut app, "ctrl+\\");
    let rows = link_rows(&mut app);
    assert_eq!(rows.len(), 2, "two links on screen: {rows:?}");
    let top_hash = draw(&mut app);
    assert_eq!(app.view_scroll_y(), 0.0);
    assert_eq!(app.nav_lens(), (1, 0));

    // 1. A #heading link scrolls the page.
    click(&mut app, 50.0, rows[0]);
    app.handle(Input::MouseMove { x: 500.0, y: 300.0 });
    draw(&mut app);
    let y_deep = app.view_scroll_y();
    assert!(y_deep > 500.0, "{y_deep}");
    assert_eq!(app.nav_lens(), (2, 0));
    // 2. Go to Heading.
    goto_heading(&mut app, "far");
    draw(&mut app);
    let y_far = app.view_scroll_y();
    assert!(y_far > y_deep + 500.0, "{y_far}");
    let far_hash = draw(&mut app);
    // 3. Go to Line (the markdown file switches to its source).
    goto_line(&mut app, 7);
    draw(&mut app);
    assert!(app.status_text().starts_with("Markdown source"));
    assert_eq!(ln(&app), "Ln 7");
    // 4. Another file, 5. a line in it, 6. back to the first tab.
    app.open(&d.join("code.txt"));
    draw(&mut app);
    goto_line(&mut app, 300);
    draw(&mut app);
    let y_300 = app.view_scroll_y();
    assert!(y_300 > 1000.0);
    let code_hash = draw(&mut app);
    key(&mut app, "ctrl+tab");
    draw(&mut app);
    assert!(app.title().starts_with("index.md"));
    assert_eq!(app.nav_lens(), (7, 0));

    // Back, step by step.
    back(&mut app);
    assert!(app.title().starts_with("code.txt"), "{}", app.title());
    assert_eq!(ln(&app), "Ln 300");
    assert_eq!(
        draw(&mut app),
        code_hash,
        "the tab is exactly as it was left"
    );
    back(&mut app);
    draw(&mut app);
    assert!(app.title().starts_with("code.txt"));
    assert_eq!(ln(&app), "Ln 1");
    assert_eq!(app.view_scroll_y(), 0.0);
    back(&mut app);
    draw(&mut app);
    assert!(app.title().starts_with("index.md"));
    assert!(app.status_text().starts_with("Markdown source"));
    assert_eq!(ln(&app), "Ln 7");
    back(&mut app);
    assert!(app.status_text().starts_with("Markdown preview"));
    assert_eq!(
        draw(&mut app),
        far_hash,
        "the preview is back at the far heading"
    );
    assert!(close(app.view_scroll_y(), y_far, 0.5));
    back(&mut app);
    draw(&mut app);
    assert!(
        close(app.view_scroll_y(), y_deep, 0.5),
        "{}",
        app.view_scroll_y()
    );
    back(&mut app);
    assert_eq!(draw(&mut app), top_hash, "and at the very top");
    assert_eq!(app.view_scroll_y(), 0.0);
    assert_eq!(app.nav_lens(), (1, 6));
    // The file that was open before this one.
    back(&mut app);
    draw(&mut app);
    assert!(app.title().starts_with("code.txt"));
    assert_eq!(ln(&app), "Ln 1");
    // Nowhere further back: nothing moves.
    back(&mut app);
    assert!(app.status_text().contains("No earlier place"));
    assert!(app.title().starts_with("code.txt"));
    assert_eq!(ln(&app), "Ln 1");
    assert_eq!(app.nav_lens(), (0, 7));

    // Forward retraces the same places.
    forward(&mut app);
    assert_eq!(draw(&mut app), top_hash);
    forward(&mut app);
    draw(&mut app);
    assert!(close(app.view_scroll_y(), y_deep, 0.5));
    forward(&mut app);
    draw(&mut app);
    assert!(close(app.view_scroll_y(), y_far, 0.5));
    forward(&mut app);
    draw(&mut app);
    assert!(app.status_text().starts_with("Markdown source"));
    assert_eq!(ln(&app), "Ln 7");
    forward(&mut app);
    draw(&mut app);
    assert!(app.title().starts_with("code.txt"));
    assert_eq!(ln(&app), "Ln 1");
    forward(&mut app);
    draw(&mut app);
    assert_eq!(ln(&app), "Ln 300");
    assert!(close(app.view_scroll_y(), y_300, 0.5));
    forward(&mut app);
    draw(&mut app);
    assert!(app.title().starts_with("index.md"));
    assert_eq!(ln(&app), "Ln 7");
    assert_eq!(app.nav_lens(), (7, 0));
    forward(&mut app);
    assert!(app.status_text().contains("No later place"));
    assert!(app.title().starts_with("index.md"));
    assert_eq!(app.tab_count(), 2, "no tab was opened or closed on the way");
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn a_link_to_another_file_and_the_palette_commands() {
    let d = library("link");
    let mut app = new_app();
    app.open(&d.join("index.md"));
    key(&mut app, "ctrl+\\");
    let rows = link_rows(&mut app);
    click(&mut app, 50.0, rows[1]);
    assert!(app.title().starts_with("other.md"), "{}", app.title());
    wheel(&mut app, 8.0);
    draw(&mut app);
    let y = app.view_scroll_y();
    assert!(y > 100.0);
    run_command(&mut app, "Go Back");
    assert!(app.title().starts_with("index.md"), "{}", app.title());
    run_command(&mut app, "Go Forward");
    assert!(app.title().starts_with("other.md"), "{}", app.title());
    draw(&mut app);
    assert_eq!(app.view_scroll_y(), y, "where the reader had scrolled to");
    // Ctrl+Alt+Left / Right still move the tab to the other side.
    key(&mut app, "ctrl+alt+right");
    assert!(app.is_split_for_test());
    assert!(app.title().starts_with("other.md"));
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn jumps_made_without_a_redraw_in_between_are_remembered_exactly() {
    let d = library("fast");
    let mut app = new_app();
    app.open(&d.join("code.txt"));
    // No draw at all so far.
    goto_line(&mut app, 100);
    goto_line(&mut app, 200);
    goto_line(&mut app, 300);
    assert_eq!(app.nav_lens(), (3, 0));
    back(&mut app);
    back(&mut app);
    assert_eq!(ln(&app), "Ln 100");
    back(&mut app);
    assert_eq!(ln(&app), "Ln 1");
    // Line 100 was left before it was ever drawn: it still comes back
    // with the line on screen.
    forward(&mut app);
    draw(&mut app);
    assert_eq!(ln(&app), "Ln 100");
    let y_100 = app.view_scroll_y();
    assert!(y_100 > 1000.0, "line 100 is on screen: {y_100}");
    forward(&mut app);
    forward(&mut app);
    assert_eq!(ln(&app), "Ln 300");
    draw(&mut app);
    let y = app.view_scroll_y();
    assert!(y > y_100 + 1000.0, "line 300 is on screen: {y}");
    // The same with headings in a preview that was never drawn.
    let index = d.join("index.md");
    let deep = line_of(&index, "## Deep");
    let far = line_of(&index, "## Far");
    app.open(&index);
    goto_heading(&mut app, "deep");
    goto_heading(&mut app, "far");
    assert_eq!(app.view_position().unwrap().ptop, far);
    back(&mut app);
    assert_eq!(app.view_position().unwrap().ptop, deep);
    draw(&mut app);
    let y_deep = app.view_scroll_y();
    assert!(y_deep > 500.0);
    forward(&mut app);
    draw(&mut app);
    assert!(app.view_scroll_y() > y_deep + 500.0);
    // Going to the place the view already shows is not a jump.
    let before = app.nav_lens();
    key(&mut app, "ctrl+tab");
    assert!(app.title().starts_with("code.txt"));
    assert_eq!(app.nav_lens().0, before.0 + 1);
    goto_line(&mut app, 300);
    draw(&mut app);
    let lens = app.nav_lens();
    goto_line(&mut app, 300);
    assert_eq!(app.nav_lens(), lens);
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn a_new_jump_after_going_back_drops_the_forward_places() {
    let (d, f) = many_files("nav-drop", 4);
    let mut app = new_app();
    for p in &f[..3] {
        app.open(p);
    }
    assert_eq!(app.nav_lens(), (2, 0));
    back(&mut app);
    back(&mut app);
    assert!(app.title().starts_with("f00.txt"));
    assert_eq!(app.nav_lens(), (0, 2));
    // A new jump from here.
    app.open(&f[3]);
    assert_eq!(app.nav_lens(), (1, 0));
    forward(&mut app);
    assert!(
        app.title().starts_with("f03.txt"),
        "nothing to go forward to"
    );
    back(&mut app);
    assert!(app.title().starts_with("f00.txt"));
    assert_eq!(app.nav_lens(), (0, 1));
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn back_reopens_a_closed_tab_and_skips_files_that_are_gone() {
    let d = library("closed");
    let (_, f) = many_files("nav-closed", 3);
    let code = d.join("code.txt");
    let mut app = new_app();
    app.open(&code);
    draw(&mut app);
    goto_line(&mut app, 400);
    draw(&mut app);
    let y = app.view_scroll_y();
    app.open(&f[0]);
    app.open(&f[1]);
    app.open(&f[2]);
    // Close every tab but the one in front (tab menu → Close Others).
    let (_, rects) = app.tab_layout().unwrap();
    let r = rects[3];
    let at = (r.x as f32 + r.w as f32 / 3.0, r.y as f32 + r.h as f32 / 2.0);
    mouse(&mut app, at.0, at.1, MouseButton::Right);
    run_menu_item(&mut app, "Close Others");
    assert_eq!(titles(&app), vec!["f02.txt"]);
    assert_eq!(app.nav_lens(), (4, 0), "closing tabs behind is not a jump");
    std::fs::remove_file(&f[1]).unwrap();
    // f01 is gone: skipped. f00 comes back in a new tab.
    back(&mut app);
    assert_eq!(titles(&app), vec!["f02.txt", "f00.txt"]);
    assert_eq!(app.active_tab(), 1);
    // And code.txt, where the reader was.
    back(&mut app);
    draw(&mut app);
    assert!(app.title().starts_with("code.txt"), "{}", app.title());
    assert_eq!(ln(&app), "Ln 400");
    assert!(
        close(app.view_scroll_y(), y, 0.5),
        "{}",
        app.view_scroll_y()
    );
    assert_eq!(app.tab_count(), 3);
    assert_eq!(app.nag_message(), None);
    // The file in front is deleted while its tab is open: Back still
    // leaves it, and Forward shows the open tab again (nothing is reopened
    // from disk, nothing is skipped).
    back(&mut app);
    assert_eq!(ln(&app), "Ln 1");
    std::fs::remove_file(&f[0]).unwrap();
    forward(&mut app);
    assert_eq!(ln(&app), "Ln 400");
    forward(&mut app);
    assert!(app.title().contains("f00.txt"), "{}", app.title());
    std::fs::remove_dir_all(&d).ok();
    std::fs::remove_dir_all(f[2].parent().unwrap()).ok();
}

#[test]
fn closing_the_tab_in_front_is_a_jump_back_returns_to() {
    let d = library("ctrlw");
    let mut app = new_app();
    app.open(&d.join("code.txt"));
    app.open(&d.join("other.md"));
    wheel(&mut app, 10.0);
    draw(&mut app);
    let y = app.view_scroll_y();
    assert!(y > 100.0);
    key(&mut app, "ctrl+w");
    assert_eq!(titles(&app), vec!["code.txt"]);
    back(&mut app);
    draw(&mut app);
    assert_eq!(titles(&app), vec!["code.txt", "other.md"]);
    assert!(app.title().starts_with("other.md"));
    assert!(
        close(app.view_scroll_y(), y, 0.5),
        "{}",
        app.view_scroll_y()
    );
    // The only tab: closing it leaves the start page; Back reopens it.
    let mut app = new_app();
    app.open(&d.join("code.txt"));
    goto_line(&mut app, 50);
    key(&mut app, "ctrl+w");
    assert!(!app.title().contains("code.txt"));
    back(&mut app);
    assert!(app.title().starts_with("code.txt"), "{}", app.title());
    assert_eq!(ln(&app), "Ln 50");
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn nothing_moves_while_a_question_is_showing() {
    let (d, f) = many_files("nav-nag", 3);
    let mut app = new_app();
    app.open(&f[0]);
    app.open(&f[1]);
    type_text(&mut app, "edit ");
    key(&mut app, "ctrl+w");
    assert_eq!(
        app.nag_message().as_deref(),
        Some("Save changes to f01.txt?")
    );
    let lens = app.nav_lens();
    assert_eq!(lens, (1, 0));
    let shown = draw(&mut app);
    // The keys: nothing moves, not even the highlighted answer.
    back(&mut app);
    assert_eq!(draw(&mut app), shown);
    forward(&mut app);
    assert_eq!(draw(&mut app), shown);
    // Nor through the door the keys don't use.
    for dir in [true, false, true] {
        app.navigate(dir);
        assert!(app.nag_message().is_some());
        assert!(app.title().contains("f01.txt"), "{}", app.title());
        assert_eq!(app.nav_lens(), lens);
        assert_eq!(draw(&mut app), shown);
    }
    assert_eq!(app.view_text(), "edit file 1\n");
    // Enter still picks the first answer (Save), for the tab it asked about.
    key(&mut app, "enter");
    assert_eq!(std::fs::read_to_string(&f[1]).unwrap(), "edit file 1\n");
    assert_eq!(titles(&app), vec!["f00.txt"]);
    // With the question gone, Back works: it returns to the closed tab.
    back(&mut app);
    assert!(app.title().starts_with("f01.txt"), "{}", app.title());
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn a_tab_with_unsaved_edits_is_never_closed_or_replaced_by_navigation() {
    let (d, f) = many_files("nav-dirty", 5);
    let mut app = new_app();
    app.open(&d);
    // Browse with single clicks: one peek tab.
    click_tree(&mut app, &f[0], 1);
    click_tree(&mut app, &f[1], 1);
    click_tree(&mut app, &f[2], 1);
    assert_eq!(titles(&app), vec!["f02.txt"]);
    // Back / Forward browse the same way, in the same peek tab.
    back(&mut app);
    assert_eq!(titles(&app), vec!["f01.txt"]);
    back(&mut app);
    assert_eq!(titles(&app), vec!["f00.txt"]);
    assert_eq!(app.peek_tabs(), vec![true]);
    forward(&mut app);
    forward(&mut app);
    assert_eq!(titles(&app), vec!["f02.txt"]);
    assert_eq!(app.peek_tabs(), vec![true]);
    // Edit the peeked file: now Back must not take its place.
    click(&mut app, 500.0, 300.0);
    key(&mut app, "ctrl+home");
    type_text(&mut app, "mine ");
    back(&mut app);
    assert!(app.title().starts_with("f01.txt"), "{}", app.title());
    assert_eq!(app.tab_count(), 2);
    assert!(app.tab_titles().contains(&("f02.txt".to_string(), true)));
    back(&mut app);
    assert!(app.title().starts_with("f00.txt"), "{}", app.title());
    assert_eq!(
        app.tab_count(),
        2,
        "f00 took the peek tab of f01, not the edited tab"
    );
    forward(&mut app);
    forward(&mut app);
    assert!(app.title().starts_with("● f02.txt"), "{}", app.title());
    assert_eq!(app.view_text(), "mine file 2\n");
    assert_eq!(app.nag_message(), None, "no question, nothing to lose");
    // An edited ordinary tab: left and returned to with its edits, even
    // though the caret moved since the place was remembered.
    key(&mut app, "ctrl+end");
    type_text(&mut app, "more");
    app.open(&f[4]);
    back(&mut app);
    assert_eq!(app.view_text(), "mine file 2\nmore");
    assert!(
        app.tab_titles()
            .iter()
            .all(|(t, dirty)| *dirty == (t == "f02.txt"))
    );
    assert_eq!(std::fs::read_to_string(&f[2]).unwrap(), "file 2\n");
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn a_closed_file_reopens_where_the_reader_was() {
    let d = library("reopen");
    let code = d.join("code.txt");
    let other = d.join("other.md");
    let mut app = new_app();
    // Code: caret and scroll position (the caret is not on screen).
    app.open(&code);
    draw(&mut app);
    goto_line(&mut app, 250);
    key(&mut app, "end");
    draw(&mut app);
    wheel(&mut app, 20.0);
    draw(&mut app);
    let y = app.view_scroll_y();
    let pos = app.view_position().unwrap();
    assert_eq!((pos.line, pos.col), (249, 8));
    assert!(pos.top > 260, "{pos:?}");
    key(&mut app, "ctrl+w");
    app.open(&code);
    draw(&mut app);
    assert_eq!(ln(&app), "Ln 250");
    assert!(app.status_text().contains("Col 9"), "{}", app.status_text());
    assert!(
        close(app.view_scroll_y(), y, 0.1),
        "{} vs {y}",
        app.view_scroll_y()
    );
    assert_eq!(app.view_position(), Some(pos));

    // Markdown preview: the scroll position.
    app.open(&other);
    draw(&mut app);
    wheel(&mut app, 25.0);
    draw(&mut app);
    let y = app.view_scroll_y();
    let pos = app.view_position().unwrap();
    assert!(y > 1000.0 && pos.ptop > 20, "{y} {pos:?}");
    key(&mut app, "ctrl+w");
    app.open(&other);
    draw(&mut app);
    assert!(
        close(app.view_scroll_y(), y, 0.5),
        "{} vs {y}",
        app.view_scroll_y()
    );
    // Reopened in a window of another width: other pixels, the same place.
    key(&mut app, "ctrl+w");
    app.handle(Input::Resize(520, 700));
    app.open(&other);
    app.settle(Duration::from_secs(5));
    render_hash(&mut app, 520, 700);
    let narrow = app.view_position().unwrap();
    assert_eq!(narrow.ptop, pos.ptop);
    assert!(
        (narrow.poff as i64 - pos.poff as i64).abs() < 5_000,
        "{narrow:?} vs {pos:?}"
    );
    assert!(app.view_scroll_y() > y + 100.0, "the text wraps more now");
    app.handle(Input::Resize(W, H));

    // Ctrl+E in a reopened preview still shows the source at the place
    // the preview shows (not where the source was scrolled to long ago).
    key(&mut app, "ctrl+e");
    draw(&mut app);
    wheel(&mut app, 150.0);
    draw(&mut app);
    assert!(app.view_position().unwrap().top > 200);
    key(&mut app, "ctrl+e");
    draw(&mut app);
    key(&mut app, "ctrl+home");
    key(&mut app, "home");
    wheel(&mut app, 25.0);
    draw(&mut app);
    let pos = app.view_position().unwrap();
    assert!(pos.ptop > 20 && pos.ptop < 150 && pos.top > 200, "{pos:?}");
    key(&mut app, "ctrl+w");
    app.open(&other);
    draw(&mut app);
    assert_eq!(app.view_position(), Some(pos));
    key(&mut app, "ctrl+e");
    draw(&mut app);
    let top = app.view_position().unwrap().top;
    assert!(
        top.abs_diff(pos.ptop) <= 4,
        "source at line {top}, preview was at {}",
        pos.ptop
    );
    key(&mut app, "ctrl+e");
    draw(&mut app);

    // Markdown left in its source view comes back in its source view.
    key(&mut app, "ctrl+e");
    goto_line(&mut app, 41);
    draw(&mut app);
    key(&mut app, "ctrl+w");
    app.open(&other);
    draw(&mut app);
    assert!(app.status_text().starts_with("Markdown source"));
    assert_eq!(ln(&app), "Ln 41");
    // Side by side, with the preview scrolled away from the caret: both
    // halves come back as they were.
    key(&mut app, "ctrl+shift+v");
    draw(&mut app);
    app.handle(Input::MouseMove { x: 700.0, y: 300.0 });
    app.handle(Input::Wheel {
        dx: 0.0,
        dy: 40.0,
        unit: WheelUnit::Lines,
    });
    draw(&mut app);
    let y = app.view_scroll_y();
    let pos = app.view_position().unwrap();
    assert!(pos.ptop > 50 && pos.line == 40, "{pos:?}");
    key(&mut app, "ctrl+w");
    app.open(&other);
    draw(&mut app);
    assert!(app.status_text().starts_with("Markdown side by side"));
    assert_eq!(ln(&app), "Ln 41");
    assert!(
        close(app.view_scroll_y(), y, 0.5),
        "{} vs {y}",
        app.view_scroll_y()
    );
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn a_preview_with_images_reopens_at_the_same_paragraph() {
    // Images get their size only once they are loaded: the page grows
    // after the first draw, and the restored place must follow.
    let d = tmp_dir("nav-img");
    std::fs::copy(fixture("img/banner.png"), d.join("banner.png")).unwrap();
    let mut text = String::new();
    for i in 0..4 {
        text.push_str(&format!("![banner {i}](banner.png)\n\n"));
    }
    for i in 0..80 {
        text.push_str(&format!("paragraph {i}\n\n"));
    }
    let doc = d.join("doc.md");
    std::fs::write(&doc, text).unwrap();
    let mut app = new_app();
    app.open(&doc);
    draw(&mut app);
    draw(&mut app);
    wheel(&mut app, 12.0);
    draw(&mut app);
    let y = app.view_scroll_y();
    let pos = app.view_position().unwrap();
    assert!(pos.ptop > 10, "below the images: {pos:?}");
    key(&mut app, "ctrl+w");
    app.open(&doc);
    // The first draw has no image sizes yet; the next ones do.
    render_hash(&mut app, W, H);
    draw(&mut app);
    draw(&mut app);
    assert_eq!(app.view_position().map(|p| p.ptop), Some(pos.ptop));
    assert!(
        close(app.view_scroll_y(), y, 0.5),
        "{} vs {y}",
        app.view_scroll_y()
    );
    // Scrolling ends the hold: the page stays where the reader puts it.
    wheel(&mut app, 3.0);
    draw(&mut app);
    assert!(app.view_scroll_y() > y + 10.0);
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn a_remembered_position_clamps_when_the_file_shrank() {
    let d = library("shrank");
    let code = d.join("code.txt");
    let other = d.join("other.md");
    let mut app = new_app();
    app.open(&code);
    goto_line(&mut app, 590);
    key(&mut app, "end");
    draw(&mut app);
    app.open(&other);
    draw(&mut app);
    key(&mut app, "end");
    draw(&mut app);
    assert!(app.view_scroll_y() > 3000.0);
    key(&mut app, "ctrl+w");
    key(&mut app, "ctrl+w");
    std::fs::write(&code, "one\ntwo\nthré").unwrap();
    std::fs::write(&other, "# Short\n\nnow\n").unwrap();
    app.open(&code);
    draw(&mut app);
    assert_eq!(ln(&app), "Ln 3");
    assert!(app.status_text().contains("Col 5"), "{}", app.status_text());
    assert_eq!(app.view_scroll_y(), 0.0);
    app.open(&other);
    draw(&mut app);
    assert_eq!(app.view_scroll_y(), 0.0);
    // Back to a place that no longer exists in the file clamps as well.
    let mut app = new_app();
    app.open(&d.join("index.md"));
    draw(&mut app);
    goto_heading(&mut app, "far");
    draw(&mut app);
    app.open(&code);
    std::fs::write(d.join("index.md"), "tiny\n").unwrap();
    app.handle(Input::FsChanged(vec![d.join("index.md")]));
    back(&mut app);
    draw(&mut app);
    assert!(app.title().starts_with("index.md"));
    assert_eq!(app.view_scroll_y(), 0.0);
    assert_eq!(app.view_text(), "tiny\n");
    std::fs::remove_dir_all(&d).ok();
}

/// Open the library's three files and leave each somewhere recognisable.
/// Returns (code scroll, other.md preview scroll).
fn read_around(app: &mut App, d: &Path) -> (f32, f32) {
    app.open(d);
    app.open(&d.join("code.txt"));
    draw(app);
    goto_line(app, 333);
    draw(app);
    let y_code = app.view_scroll_y();
    app.open(&d.join("other.md"));
    draw(app);
    wheel(app, 30.0);
    draw(app);
    let y_other = app.view_scroll_y();
    app.open(&d.join("index.md"));
    draw(app);
    key(app, "ctrl+e");
    goto_line(app, 12);
    draw(app);
    assert!(y_code > 1000.0 && y_other > 1000.0);
    (y_code, y_other)
}

/// The three files are open again, each where `read_around` left it.
fn assert_read_around(app: &mut App, y_code: f32, y_other: f32) {
    assert_eq!(titles(app), vec!["code.txt", "other.md", "index.md"]);
    // The active tab, then the ones that were never drawn in this run.
    draw(app);
    assert!(app.title().starts_with("index.md"), "{}", app.title());
    assert!(app.status_text().starts_with("Markdown source"));
    assert_eq!(ln(app), "Ln 12");
    app.activate(0);
    draw(app);
    assert_eq!(ln(app), "Ln 333");
    assert!(
        close(app.view_scroll_y(), y_code, 0.1),
        "{}",
        app.view_scroll_y()
    );
    app.activate(1);
    draw(app);
    assert!(app.status_text().starts_with("Markdown preview"));
    assert!(
        close(app.view_scroll_y(), y_other, 0.5),
        "{}",
        app.view_scroll_y()
    );
}

#[test]
fn positions_survive_a_workspace_file_and_the_folder_store() {
    let d = library("ws");
    let store = tmp_dir("nav-ws-store");
    let elsewhere = tmp_dir("nav-ws-else");
    std::fs::write(elsewhere.join("x.txt"), "x\n").unwrap();
    let mut app = new_app();
    app.use_workspace_store(store.clone());
    let (y_code, y_other) = read_around(&mut app, &d);
    let file = d.join("lib.litedit-workspace");
    app.save_workspace_as(&file);
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains("tab\tcode.txt\npos\tl=332 "), "{text}");
    assert!(text.contains("tab\tindex.md\npos\tl=11 "), "{text}");
    // The workspace file, in a fresh run.
    let mut fresh = new_app();
    fresh.open(&file);
    assert_read_around(&mut fresh, y_code, y_other);
    assert_eq!(fresh.nav_lens(), (0, 0), "restoring is not navigating");
    // The folder's own remembered workspace: leave the folder and return
    // (tabs that were hidden when it was saved included), in a fresh run.
    app.activate(2);
    app.open(&elsewhere);
    assert_eq!(app.tab_count(), 1);
    let mut fresh = new_app();
    fresh.use_workspace_store(store.clone());
    fresh.open(&d);
    assert_read_around(&mut fresh, y_code, y_other);
    // A tab that is already open keeps its own place when a workspace
    // naming another place for it is opened.
    key(&mut fresh, "ctrl+tab");
    assert!(fresh.title().starts_with("index.md"));
    goto_line(&mut fresh, 30);
    type_text(&mut fresh, "edit");
    fresh.open(&file);
    assert!(fresh.title().starts_with("● index.md"), "{}", fresh.title());
    assert_eq!(ln(&fresh), "Ln 30");
    for p in [d, store, elsewhere] {
        std::fs::remove_dir_all(p).ok();
    }
}

#[test]
fn positions_survive_a_session_round_trip() {
    let d = library("session");
    let mut app = new_app();
    let (y_code, y_other) = read_around(&mut app, &d);
    let text = app.session().to_text();
    let mut fresh = new_app();
    fresh.start_with_session(Session::parse(&text), true);
    assert_read_around(&mut fresh, y_code, y_other);
    // An instance started with a file doesn't own the saved tabs: it hands
    // their positions back untouched.
    let saved = Session::parse(&text);
    let mut other = new_app();
    other.start_with_session(saved.clone(), false);
    other.open(&d.join("code.txt"));
    goto_line(&mut other, 5);
    let s = other.session();
    assert_eq!(s.tabs, saved.tabs);
    assert_eq!(s.positions, saved.positions);
    assert!(s.positions.iter().all(Option::is_some));
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn hostile_positions_clamp_and_never_panic() {
    let d = library("hostile");
    std::fs::write(d.join("empty.txt"), "").unwrap();
    std::fs::write(d.join("empty.md"), "").unwrap();
    let huge = "l=4294967295 c=4294967295 t=4294967295 o=1000000 p=4294967295 f=1000000 m=2";
    let mut ws = String::from("litedit-workspace 1\nfolder\t.\n");
    for f in ["code.txt", "other.md", "index.md", "empty.txt", "empty.md"] {
        ws.push_str(&format!("tab\t{f}\npos\t{huge}\n"));
    }
    ws.push_str("active\t0\n");
    let file = d.join("h.litedit-workspace");
    std::fs::write(&file, ws).unwrap();
    let mut app = new_app();
    app.open(&file);
    assert_eq!(app.tab_count(), 5);
    for i in 0..5 {
        app.activate(i);
        for (w, h) in [(W, H), (1, 1), (60, 40), (W, H)] {
            app.handle(Input::Resize(w, h));
            app.settle(Duration::from_secs(5));
            render_hash(&mut app, w, h);
        }
        let p = app.view_position().unwrap();
        let lines = app.view_text().split('\n').count();
        assert!(
            p.line < lines && p.top < lines,
            "{i}: {p:?} in {lines} lines"
        );
        key(&mut app, "ctrl+e");
        render_hash(&mut app, W, H);
        key(&mut app, "pagedown");
        type_text(&mut app, "x");
        render_hash(&mut app, W, H);
    }
    app.activate(0);
    draw(&mut app);
    assert_eq!(ln(&app), "Ln 601", "the last line of code.txt");
    // Fed straight to a view, in every combination of extremes.
    let mut app = new_app();
    app.open(&d.join("other.md"));
    for line in [0, 7, usize::MAX] {
        for off in [0, 999_999, u32::MAX] {
            for mode in [0, 1, 2, 255] {
                let p = ViewPos {
                    line,
                    col: line,
                    top: line,
                    top_off: off,
                    ptop: line,
                    poff: off,
                    mode,
                };
                app.restore_view_position(p);
                draw(&mut app);
                app.restore_view_position(p);
                assert!(app.view_position().is_some());
            }
        }
    }
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn a_search_result_is_one_step_back() {
    let d = library("search");
    let mut app = new_app();
    app.open(&d);
    app.open(&d.join("index.md"));
    draw(&mut app);
    goto_heading(&mut app, "deep");
    draw(&mut app);
    let y = app.view_scroll_y();
    key(&mut app, "ctrl+shift+f");
    type_text(&mut app, "line 444");
    key(&mut app, "enter");
    app.settle(Duration::from_secs(5));
    render_hash(&mut app, W, H);
    assert!(
        app.status_text().starts_with("1 result "),
        "{}",
        app.status_text()
    );
    key(&mut app, "enter");
    assert!(app.title().starts_with("code.txt"), "{}", app.title());
    assert_eq!(ln(&app), "Ln 444");
    back(&mut app);
    draw(&mut app);
    assert!(app.title().starts_with("index.md"), "{}", app.title());
    assert!(close(app.view_scroll_y(), y, 0.5));
    forward(&mut app);
    assert_eq!(ln(&app), "Ln 444");
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn the_history_and_the_remembered_positions_are_bounded() {
    let (d, f) = many_files("nav-bound", 3);
    let mut app = new_app();
    for i in 0..250 {
        app.open(&f[i % 3]);
    }
    assert_eq!(app.nav_lens(), (crate::nav::MAX_HISTORY, 0));
    for _ in 0..400 {
        back(&mut app);
    }
    let (b, fw) = app.nav_lens();
    assert_eq!(b, 0);
    assert!(fw <= crate::nav::MAX_HISTORY);
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn back_and_forward_work_across_the_two_sides_of_a_split() {
    let (d, f) = many_files("nav-split", 3);
    let mut app = new_app();
    app.open(&f[0]);
    app.open(&f[1]);
    key(&mut app, "ctrl+alt+right");
    assert!(app.is_split_for_test());
    app.open(&f[0]);
    app.open(&f[1]);
    app.open(&f[2]);
    let tabs = titles(&app);
    assert!(app.title().starts_with("f02.txt"));
    back(&mut app);
    assert!(app.title().starts_with("f01.txt"), "{}", app.title());
    back(&mut app);
    assert!(app.title().starts_with("f00.txt"), "{}", app.title());
    forward(&mut app);
    forward(&mut app);
    assert!(app.title().starts_with("f02.txt"), "{}", app.title());
    // Only the front tab changes; no tab is opened, closed or moved, and
    // the split stays.
    assert_eq!(titles(&app), tabs);
    assert!(app.is_split_for_test());
    draw(&mut app);
    // A tab closed on one side comes back on Back (no panic, no loss).
    key(&mut app, "ctrl+w");
    back(&mut app);
    assert!(app.title().starts_with("f02.txt"), "{}", app.title());
    assert_eq!(titles(&app).len(), tabs.len());
    draw(&mut app);
    std::fs::remove_dir_all(&d).ok();
}
