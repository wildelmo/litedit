//! S7.5: several entries selected in the tree, delete / move for all of
//! them, copy / paste / duplicate, and the file details in the status bar.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::app::App;
use crate::input::{Input, Mods, MouseButton, Request};
use crate::tests::{click_tree, drag, key, mouse, new_app, render_hash, run_menu_item};

const CTRL: Mods = Mods {
    ctrl: true,
    shift: false,
    alt: false,
};
const SHIFT: Mods = Mods {
    ctrl: false,
    shift: true,
    alt: false,
};

/// A folder `ws` with docs/ (empty), notes/{b.md, c.md}, a.txt, d.txt and
/// e.txt, opened in a new app with the keyboard in the tree; and a `bin`
/// folder standing in for the Recycle Bin.
fn setup(tag: &str) -> (App, PathBuf, PathBuf) {
    let base = std::env::temp_dir().join(format!("litedit-s75-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let (ws, bin) = (base.join("ws"), base.join("bin"));
    std::fs::create_dir_all(ws.join("docs")).unwrap();
    std::fs::create_dir_all(ws.join("notes")).unwrap();
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(ws.join("notes/b.md"), "bee\n").unwrap();
    std::fs::write(ws.join("notes/c.md"), "sea\n").unwrap();
    std::fs::write(ws.join("a.txt"), "hello").unwrap();
    std::fs::write(ws.join("d.txt"), "dee").unwrap();
    std::fs::write(ws.join("e.txt"), "eee").unwrap();
    crate::fsops::set_test_recycle_dir(Some(bin.clone()));
    let mut app = new_app();
    app.open(&ws);
    render_hash(&mut app, 800, 600);
    (app, ws, bin)
}

fn cleanup(ws: &Path) {
    crate::fsops::set_test_recycle_dir(None);
    if let Some(base) = ws.parent() {
        let _ = std::fs::remove_dir_all(base);
    }
}

fn row_y(app: &mut App, p: &Path) -> f32 {
    app.tree_row_y(p)
        .unwrap_or_else(|| panic!("no tree row for {}", p.display()))
}

/// Click a tree row while holding modifier keys.
fn click_with(app: &mut App, p: &Path, mods: Mods) {
    let y = row_y(app, p);
    app.handle(Input::Modifiers(mods));
    app.handle(Input::MouseDown {
        x: 60.0,
        y,
        button: MouseButton::Left,
        clicks: 1,
    });
    app.handle(Input::MouseUp {
        x: 60.0,
        y,
        button: MouseButton::Left,
    });
    app.handle(Input::Modifiers(Mods::default()));
}

fn right_click(app: &mut App, p: &Path) {
    let y = row_y(app, p);
    mouse(app, 60.0, y, MouseButton::Right);
}

fn drag_rows(app: &mut App, from: &Path, to: &Path) {
    let (a, b) = (row_y(app, from), row_y(app, to));
    drag(app, (60.0, a), (60.0, b));
}

/// Press a key and let a copy it started run to its end.
fn key_done(app: &mut App, combo: &str) {
    key(app, combo);
    app.settle(Duration::from_secs(20));
    assert!(!app.copy_running());
}

/// Pick a menu entry and let a copy it started run to its end.
fn menu_done(app: &mut App, label: &str) {
    run_menu_item(app, label);
    app.settle(Duration::from_secs(20));
    assert!(!app.copy_running());
}

fn focus_tree(app: &mut App) {
    key(app, "ctrl+shift+e");
}

fn names(paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect()
}

fn selected(app: &mut App) -> Vec<String> {
    names(&app.tree_selection())
}

/// The bytes of every file under `dir`, sorted (a multiset).
fn contents(dir: &Path) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(d) = pending.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let t = e.file_type().unwrap();
            if t.is_dir() {
                pending.push(e.path());
            } else if t.is_file() {
                out.push(std::fs::read(e.path()).unwrap());
            }
        }
    }
    out.sort();
    out
}

/// Every file under `dir` (sorted).
fn file_paths(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(d) = pending.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            if e.file_type().unwrap().is_dir() {
                pending.push(e.path());
            } else {
                out.push(e.path());
            }
        }
    }
    out.sort();
    out
}

fn read(p: PathBuf) -> String {
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

#[test]
fn ctrl_and_shift_clicks_select_entries_without_opening_them() {
    let (mut app, ws, _bin) = setup("select");
    let p = |n: &str| ws.join(n);
    click_tree(&mut app, &p("a.txt"), 1);
    assert!(app.title().starts_with("a.txt"), "{}", app.title());
    assert_eq!(selected(&mut app), ["a.txt"]);
    let (tabs, rows) = (app.tab_count(), app.tree_rows().len());

    // Ctrl+click adds and removes; nothing opens, no folder expands.
    click_with(&mut app, &p("d.txt"), CTRL);
    assert_eq!(selected(&mut app), ["a.txt", "d.txt"]);
    click_with(&mut app, &p("notes"), CTRL);
    assert_eq!(selected(&mut app), ["notes", "a.txt", "d.txt"]);
    click_with(&mut app, &p("a.txt"), CTRL);
    assert_eq!(selected(&mut app), ["notes", "d.txt"]);
    assert!(app.status_text().contains("2 selected in the tree"));
    // The workspace folder itself never joins a selection.
    click_with(&mut app, &ws, CTRL);
    assert_eq!(selected(&mut app), ["notes", "d.txt"]);

    // Shift+click: the range from the entry clicked last.
    click_with(&mut app, &p("e.txt"), SHIFT);
    assert_eq!(selected(&mut app), ["a.txt", "d.txt", "e.txt"]);
    click_with(&mut app, &p("docs"), SHIFT);
    assert_eq!(selected(&mut app), ["docs", "notes", "a.txt"]);
    click_with(&mut app, &ws, SHIFT);
    assert_eq!(selected(&mut app), ["docs", "notes", "a.txt"]);

    assert!(app.title().starts_with("a.txt"), "{}", app.title());
    assert_eq!(app.tab_count(), tabs, "no tab was opened");
    assert_eq!(app.tree_rows().len(), rows, "no folder was expanded");
    assert!(
        !app.take_requests()
            .iter()
            .any(|r| matches!(r, Request::OpenPath(_) | Request::PeekPath(_))),
    );

    // A plain click goes back to one entry (and shows it, as before).
    click_tree(&mut app, &p("d.txt"), 1);
    assert_eq!(selected(&mut app), ["d.txt"]);
    assert!(app.title().starts_with("d.txt"), "{}", app.title());
    // So does an arrow key.
    click_with(&mut app, &p("notes"), CTRL);
    click_with(&mut app, &p("a.txt"), CTRL);
    assert_eq!(selected(&mut app), ["notes", "a.txt", "d.txt"]);
    key(&mut app, "down");
    assert_eq!(selected(&mut app), ["d.txt"]);

    // Shift+arrows extend from there; Ctrl+A takes every shown entry of the
    // folder, but not the folder.
    key(&mut app, "shift+down");
    assert_eq!(selected(&mut app), ["d.txt", "e.txt"]);
    key(&mut app, "shift+up");
    key(&mut app, "shift+up");
    assert_eq!(selected(&mut app), ["a.txt", "d.txt"]);
    key(&mut app, "ctrl+a");
    assert_eq!(
        selected(&mut app),
        ["docs", "notes", "a.txt", "d.txt", "e.txt"]
    );
    assert!(app.status_text().contains("5 selected in the tree"));
    cleanup(&ws);
}

#[test]
fn clicking_one_of_several_selected_entries_selects_just_it_on_release() {
    let (mut app, ws, _bin) = setup("deferred");
    click_with(&mut app, &ws.join("d.txt"), CTRL);
    click_with(&mut app, &ws.join("e.txt"), CTRL);
    let y = row_y(&mut app, &ws.join("e.txt"));
    app.handle(Input::MouseDown {
        x: 60.0,
        y,
        button: MouseButton::Left,
        clicks: 1,
    });
    // Still both while the button is down (a drag would move both).
    assert_eq!(selected(&mut app), ["d.txt", "e.txt"]);
    assert!(!app.title().starts_with("e.txt"));
    app.handle(Input::MouseUp {
        x: 60.0,
        y,
        button: MouseButton::Left,
    });
    assert_eq!(selected(&mut app), ["e.txt"]);
    assert!(app.title().starts_with("e.txt"), "{}", app.title());
    cleanup(&ws);
}

#[test]
fn a_selection_of_several_entries_is_drawn_in_both_themes_and_without_focus() {
    let (mut app, ws, _bin) = setup("draw");
    for light in [false, true] {
        app.set_light(light);
        focus_tree(&mut app);
        click_with(&mut app, &ws.join("d.txt"), CTRL);
        let one = render_hash(&mut app, 800, 600);
        click_with(&mut app, &ws.join("e.txt"), CTRL);
        let two = render_hash(&mut app, 800, 600);
        assert_ne!(
            one, two,
            "light={light}: the second entry shows as selected"
        );
        // With the keyboard elsewhere the selection still shows, because the
        // right-click menu acts on it.
        key(&mut app, "escape");
        assert_eq!(app.focus(), crate::app::Focus::Main);
        let unfocused = render_hash(&mut app, 800, 600);
        click_with(&mut app, &ws.join("e.txt"), CTRL);
        click_with(&mut app, &ws.join("d.txt"), CTRL);
        key(&mut app, "escape");
        assert_ne!(
            unfocused,
            render_hash(&mut app, 800, 600),
            "light={light}: selected entries show without focus"
        );
    }
    app.set_light(false);
    cleanup(&ws);
}

/// The colour of the tree row of `p` at its right end (clear of the text).
fn row_pixel(app: &mut App, p: &Path) -> u32 {
    let y = row_y(app, p) as i32;
    let (w, h) = (800u32, 600u32);
    let mut buf = vec![0u32; (w * h) as usize];
    let mut c = crate::canvas::Canvas::new(&mut buf, w, h);
    app.render(&mut c);
    c.pixel(200, y)
}

#[test]
fn selected_rows_are_filled_with_the_selection_colour_in_both_themes() {
    let (mut app, ws, _bin) = setup("fill");
    let packed =
        |c: crate::canvas::Color| (u32::from(c.r) << 16) | (u32::from(c.g) << 8) | u32::from(c.b);
    for light in [false, true] {
        app.set_light(light);
        let sel = packed(crate::theme::p().selection);
        focus_tree(&mut app);
        click_tree(&mut app, &ws.join("d.txt"), 1);
        click_with(&mut app, &ws.join("e.txt"), CTRL);
        for name in ["d.txt", "e.txt"] {
            assert_eq!(row_pixel(&mut app, &ws.join(name)), sel, "{name} {light}");
        }
        assert_ne!(row_pixel(&mut app, &ws.join("a.txt")), sel, "{light}");
        // The keyboard elsewhere: several selected entries still show.
        key(&mut app, "escape");
        assert_eq!(row_pixel(&mut app, &ws.join("e.txt")), sel, "{light}");
        assert_ne!(row_pixel(&mut app, &ws.join("a.txt")), sel, "{light}");
    }
    app.set_light(false);
    cleanup(&ws);
}

#[test]
fn the_selection_follows_paths_when_the_tree_refreshes() {
    let (mut app, ws, bin) = setup("refresh");
    let p = |n: &str| ws.join(n);
    click_with(&mut app, &p("a.txt"), CTRL);
    click_with(&mut app, &p("e.txt"), CTRL);
    // A new file sorts in above them: every row below moves down.
    std::fs::write(p("0-first.txt"), "new").unwrap();
    app.handle(Input::FsChanged(vec![p("0-first.txt")]));
    assert_eq!(selected(&mut app), ["a.txt", "e.txt"]);
    // One of them disappears: the other stays selected, nothing else is.
    std::fs::remove_file(p("e.txt")).unwrap();
    app.handle(Input::FsChanged(vec![p("e.txt")]));
    assert_eq!(selected(&mut app), ["a.txt"]);

    // Delete asks about the entry that was selected, not about whatever
    // now sits on its old row.
    key(&mut app, "down"); // from a.txt (the cursor's row) to d.txt
    assert_eq!(selected(&mut app), ["d.txt"]);
    std::fs::write(p("b-before.txt"), "new").unwrap();
    app.handle(Input::FsChanged(vec![p("b-before.txt")]));
    key(&mut app, "delete");
    assert_eq!(
        app.nag_message().as_deref(),
        Some("Move d.txt to the Recycle Bin?")
    );
    key(&mut app, "escape");

    // The selected entry is deleted by another program: nothing is
    // selected, so Delete does nothing (the next file is not offered).
    std::fs::remove_file(p("d.txt")).unwrap();
    app.handle(Input::FsChanged(vec![p("d.txt")]));
    assert!(selected(&mut app).is_empty());
    key(&mut app, "delete");
    assert_eq!(app.nag_message(), None);
    key(&mut app, "f2");
    render_hash(&mut app, 800, 600);
    key(&mut app, "escape");
    assert_eq!(read(p("a.txt")), "hello");
    assert!(contents(&bin).is_empty());
    // The keyboard still moves on from that place in the list.
    focus_tree(&mut app);
    key(&mut app, "up");
    assert_eq!(selected(&mut app).len(), 1);
    // Collapsing a folder takes what is inside out of the selection.
    click_tree(&mut app, &p("notes"), 1);
    click_with(&mut app, &p("notes/b.md"), CTRL);
    click_with(&mut app, &p("a.txt"), CTRL);
    assert_eq!(selected(&mut app), ["notes", "b.md", "a.txt"]);
    key(&mut app, "ctrl+a");
    assert!(selected(&mut app).contains(&"c.md".to_string()));
    click_with(&mut app, &p("notes/c.md"), CTRL);
    click_tree(&mut app, &p("notes"), 1); // deferred click: selects + collapses
    assert_eq!(selected(&mut app), ["notes"]);
    cleanup(&ws);
}

#[test]
fn deleting_several_entries_asks_once_and_cancel_leaves_everything() {
    let (mut app, ws, bin) = setup("delete");
    let p = |n: &str| ws.join(n);
    let before = contents(&ws);
    click_tree(&mut app, &p("a.txt"), 1); // shown in a tab
    click_with(&mut app, &p("d.txt"), CTRL);
    click_with(&mut app, &p("notes"), CTRL);
    key(&mut app, "delete");
    assert_eq!(
        app.nag_message().as_deref(),
        Some("Move 3 items to the Recycle Bin? (notes, a.txt, d.txt)")
    );
    assert_eq!(app.nag_options(), ["Move to Recycle Bin", "Cancel"]);
    assert_eq!(contents(&ws), before, "nothing happens before the answer");
    key(&mut app, "escape");
    assert_eq!(contents(&ws), before);
    assert!(contents(&bin).is_empty());
    assert_eq!(selected(&mut app), ["notes", "a.txt", "d.txt"]);
    assert!(app.tab_titles().iter().any(|t| t.0 == "a.txt"));

    key(&mut app, "delete");
    key(&mut app, "enter");
    assert_eq!(app.nag_message(), None);
    assert!(
        app.status_text()
            .contains("Moved 3 items to the Recycle Bin"),
        "{}",
        app.status_text()
    );
    assert_eq!(names(&app.tree_rows()), ["ws", "docs", "e.txt"]);
    assert_eq!(contents(&ws), [b"eee".to_vec()]);
    assert_eq!(
        contents(&bin),
        [
            b"bee\n".to_vec(),
            b"dee".to_vec(),
            b"hello".to_vec(),
            b"sea\n".to_vec()
        ]
    );
    assert!(
        !app.tab_titles().iter().any(|t| t.0 == "a.txt"),
        "the tab of a deleted file (no edits) closes: {:?}",
        app.tab_titles()
    );

    // More than three: the first few are named, the rest counted. The
    // right-click menu on a selected entry is about all of them.
    for i in 1..=5 {
        std::fs::write(p(&format!("g{i}.txt")), format!("g{i}")).unwrap();
    }
    app.handle(Input::FsChanged(vec![p("g1.txt")]));
    focus_tree(&mut app);
    key(&mut app, "ctrl+a");
    right_click(&mut app, &p("g2.txt"));
    let labels = app.menu_labels();
    assert_eq!(
        labels,
        [
            "Copy 7 items",
            "Duplicate 7 items",
            "Delete 7 items (to Recycle Bin)"
        ]
    );
    run_menu_item(&mut app, "Delete 7 items (to Recycle Bin)");
    assert_eq!(
        app.nag_message().as_deref(),
        Some("Move 7 items to the Recycle Bin? (docs, e.txt, g1.txt and 4 more)")
    );
    key(&mut app, "escape");
    assert!(p("g5.txt").exists() && p("docs").exists());
    // Right-click on an entry outside the selection: the menu for that one.
    click_with(&mut app, &p("g2.txt"), CTRL);
    right_click(&mut app, &p("g2.txt"));
    let labels = app.menu_labels();
    assert!(labels.contains(&"Rename…".to_string()), "{labels:?}");
    run_menu_item(&mut app, "Delete (to Recycle Bin)");
    assert_eq!(
        app.nag_message().as_deref(),
        Some("Move g2.txt to the Recycle Bin?")
    );
    key(&mut app, "escape");
    // F2 with several selected renames nothing.
    focus_tree(&mut app);
    key(&mut app, "f2");
    assert!(app.status_text().contains("Rename works on one entry"));
    type_nothing_was_prompted(&mut app);
    cleanup(&ws);
}

/// No text prompt is open: typed text does not land in one.
fn type_nothing_was_prompted(app: &mut App) {
    let before = render_hash(app, 800, 600);
    app.handle(Input::Text("zzz".into()));
    assert_eq!(before, render_hash(app, 800, 600));
}

#[test]
fn an_open_file_with_unsaved_edits_blocks_the_whole_delete_and_move() {
    let (mut app, ws, bin) = setup("dirty");
    let p = |n: &str| ws.join(n);
    click_tree(&mut app, &p("notes"), 1);
    app.open(&p("notes/b.md"));
    key(&mut app, "ctrl+e"); // markdown: edit the source
    mouse(&mut app, 500.0, 300.0, MouseButton::Left);
    app.handle(Input::Text("unsaved ".into()));
    assert!(app.tab_titles().iter().any(|t| t.0.contains("b.md") && t.1));
    let before = contents(&ws);

    // The folder holding it, plus an unrelated file.
    click_with(&mut app, &p("notes"), CTRL);
    click_with(&mut app, &p("notes/b.md"), CTRL);
    click_with(&mut app, &p("a.txt"), CTRL);
    assert_eq!(selected(&mut app), ["notes", "a.txt"]);
    key(&mut app, "delete");
    assert_eq!(
        app.nag_message().as_deref(),
        Some("Nothing was deleted: b.md has unsaved edits. Save or close it first.")
    );
    assert_eq!(app.nag_options(), ["OK"]);
    key(&mut app, "enter");
    assert_eq!(contents(&ws), before);
    assert!(contents(&bin).is_empty());

    // The same for a move: a.txt is not moved either.
    drag_rows(&mut app, &p("a.txt"), &p("docs"));
    assert_eq!(
        app.nag_message().as_deref(),
        Some("Nothing was moved: b.md has unsaved edits. Save or close it first.")
    );
    key(&mut app, "enter");
    assert!(p("a.txt").exists() && p("notes/b.md").exists() && !p("docs/a.txt").exists());
    assert!(
        app.view_text().contains("unsaved "),
        "the edit is still there"
    );

    // Once it is saved, the same delete goes through.
    key(&mut app, "ctrl+s");
    assert!(read(p("notes/b.md")).contains("unsaved "));
    focus_tree(&mut app);
    assert_eq!(selected(&mut app), ["notes", "a.txt"]);
    key(&mut app, "delete");
    assert_eq!(
        app.nag_message().as_deref(),
        Some("Move 2 items to the Recycle Bin? (notes, a.txt)")
    );
    key(&mut app, "enter");
    assert_eq!(app.nag_message(), None);
    assert!(!p("notes").exists() && !p("a.txt").exists());
    assert_eq!(contents(&bin).len(), 3);
    cleanup(&ws);
}

#[test]
fn an_answer_arriving_after_a_file_got_edits_still_refuses() {
    let (mut app, ws, bin) = setup("late-edit");
    let p = |n: &str| ws.join(n);
    app.open(&p("notes/b.md"));
    key(&mut app, "ctrl+e");
    mouse(&mut app, 500.0, 300.0, MouseButton::Left);
    app.handle(Input::Text("late ".into()));
    let before = contents(&ws);
    // As if the question had been answered "yes" just now.
    app.answer_delete(vec![p("notes"), p("a.txt")]);
    assert_eq!(
        app.nag_message().as_deref(),
        Some("Nothing was deleted: b.md has unsaved edits. Save or close it first.")
    );
    key(&mut app, "enter");
    let move_only = vec![p("notes"), p("a.txt")];
    app.answer_move_only(move_only, p("docs"));
    assert_eq!(
        app.nag_message().as_deref(),
        Some("Nothing was moved: b.md has unsaved edits. Save or close it first.")
    );
    assert_eq!(contents(&ws), before);
    assert!(contents(&bin).is_empty());
    assert!(app.view_text().contains("late "));
    cleanup(&ws);
}

#[test]
fn a_folder_and_something_inside_it_are_acted_on_once() {
    let (mut app, ws, bin) = setup("nested");
    let p = |n: &str| ws.join(n);
    click_tree(&mut app, &p("notes"), 1);
    click_with(&mut app, &p("notes/b.md"), CTRL);
    assert_eq!(selected(&mut app), ["notes", "b.md"]);
    // Only the folder counts: the question is the one for a single entry.
    key(&mut app, "delete");
    assert_eq!(
        app.nag_message().as_deref(),
        Some("Move notes to the Recycle Bin?")
    );
    key(&mut app, "escape");
    key(&mut app, "ctrl+c");
    assert_eq!(app.copied_entries(), [p("notes")]);

    click_with(&mut app, &p("a.txt"), CTRL);
    key(&mut app, "delete");
    assert_eq!(
        app.nag_message().as_deref(),
        Some("Move 2 items to the Recycle Bin? (notes, a.txt)")
    );
    key(&mut app, "enter");
    // No "could not delete b.md": it went with its folder.
    assert_eq!(app.nag_message(), None);
    assert!(!p("notes").exists() && !p("a.txt").exists());
    assert_eq!(
        contents(&bin),
        [b"bee\n".to_vec(), b"hello".to_vec(), b"sea\n".to_vec()]
    );
    cleanup(&ws);
}

#[test]
fn moving_several_entries_reports_a_name_clash_and_moves_the_rest() {
    let (mut app, ws, _bin) = setup("move");
    let p = |n: &str| ws.join(n);
    std::fs::write(p("docs/a.txt"), "other").unwrap();
    app.handle(Input::FsChanged(vec![p("docs/a.txt")]));
    app.open(&p("d.txt")); // a tab that has to follow its file
    click_tree(&mut app, &p("a.txt"), 1);
    click_with(&mut app, &p("d.txt"), CTRL);
    click_with(&mut app, &p("e.txt"), CTRL);
    let before = contents(&ws);

    drag_rows(&mut app, &p("a.txt"), &p("docs"));
    assert_eq!(
        app.nag_message().as_deref(),
        Some("Move 3 items into docs? (a.txt, d.txt, e.txt)")
    );
    assert!(p("d.txt").exists(), "nothing moves before the answer");
    key(&mut app, "escape");
    assert!(p("a.txt").exists() && p("d.txt").exists() && p("e.txt").exists());
    assert_eq!(selected(&mut app), ["a.txt", "d.txt", "e.txt"]);

    drag_rows(&mut app, &p("d.txt"), &p("docs"));
    key(&mut app, "enter");
    assert_eq!(
        app.nag_message().as_deref(),
        Some("Moved 2 of 3 into docs. Not moved: a.txt (that name is already used there)")
    );
    key(&mut app, "enter");
    assert_eq!(read(p("a.txt")), "hello", "the clashing file stays");
    assert_eq!(read(p("docs/a.txt")), "other", "and nothing is replaced");
    assert_eq!(read(p("docs/d.txt")), "dee");
    assert_eq!(read(p("docs/e.txt")), "eee");
    assert!(!p("d.txt").exists() && !p("e.txt").exists());
    assert_eq!(contents(&ws), before, "same files, same bytes");
    assert_eq!(selected(&mut app), ["a.txt"], "what is left is selected");

    // The open tab followed d.txt: opening the new path shows that tab,
    // and saving writes there.
    let tabs = app.tab_count();
    app.open(&p("docs/d.txt"));
    assert_eq!(app.tab_count(), tabs, "{:?}", app.tab_titles());
    mouse(&mut app, 500.0, 300.0, MouseButton::Left);
    key(&mut app, "ctrl+end");
    app.handle(Input::Text("!".into()));
    key(&mut app, "ctrl+s");
    assert_eq!(read(p("docs/d.txt")), "dee!");
    assert!(!p("d.txt").exists());
    cleanup(&ws);
}

#[test]
fn a_selection_is_never_dropped_into_one_of_its_own_folders() {
    let (mut app, ws, _bin) = setup("into-itself");
    let p = |n: &str| ws.join(n);
    std::fs::create_dir_all(p("notes/sub")).unwrap();
    click_tree(&mut app, &p("notes"), 1);
    click_with(&mut app, &p("a.txt"), CTRL);
    assert_eq!(selected(&mut app), ["notes", "a.txt"]);
    let before = contents(&ws);
    for target in ["notes/sub", "notes", "notes/b.md"] {
        drag_rows(&mut app, &p("a.txt"), &p(target));
        assert_eq!(app.nag_message(), None, "dropping on {target}");
    }
    assert!(p("a.txt").exists() && p("notes/sub").is_dir() && !p("notes/sub/notes").exists());
    assert_eq!(contents(&ws), before);
    // A valid folder still works for both.
    drag_rows(&mut app, &p("a.txt"), &p("docs"));
    assert_eq!(
        app.nag_message().as_deref(),
        Some("Move 2 items into docs? (notes, a.txt)")
    );
    key(&mut app, "enter");
    assert_eq!(app.nag_message(), None);
    assert_eq!(read(p("docs/notes/b.md")), "bee\n");
    assert_eq!(read(p("docs/a.txt")), "hello");
    assert_eq!(selected(&mut app), ["notes", "a.txt"], "the moved entries");
    cleanup(&ws);
}

#[test]
fn paste_and_duplicate_never_replace_a_file_and_name_the_copies() {
    let (mut app, ws, _bin) = setup("paste");
    let p = |n: &str| ws.join(n);
    // Nothing copied yet: no Paste in the menu, Ctrl+V says so.
    right_click(&mut app, &p("docs"));
    assert!(!app.menu_labels().contains(&"Paste".to_string()));
    key(&mut app, "escape");
    focus_tree(&mut app);
    key_done(&mut app, "ctrl+v");
    assert!(app.status_text().contains("Nothing to paste"));

    click_tree(&mut app, &p("a.txt"), 1);
    key(&mut app, "ctrl+c");
    assert_eq!(app.copied_entries(), [p("a.txt")]);
    assert!(app.status_text().contains("Copied a.txt"));
    assert_eq!(
        app.clipboard_text(),
        None,
        "the text clipboard is untouched"
    );

    // Paste into a folder; again with the pasted file selected (= into its
    // folder): the name is taken, so copies are numbered.
    click_tree(&mut app, &p("docs"), 1);
    key_done(&mut app, "ctrl+v");
    assert_eq!(read(p("docs/a.txt")), "hello");
    assert!(app.status_text().contains("Pasted a.txt into docs"));
    assert_eq!(selected(&mut app), ["a.txt"]);
    assert_eq!(app.tree_selection(), [p("docs/a.txt")]);
    std::fs::write(p("docs/a.txt"), "changed since").unwrap();
    key_done(&mut app, "ctrl+v");
    key_done(&mut app, "ctrl+v");
    assert_eq!(read(p("docs/a.txt")), "changed since", "never replaced");
    assert_eq!(read(p("docs/a - Copy.txt")), "hello");
    assert_eq!(read(p("docs/a - Copy (2).txt")), "hello");
    assert!(
        app.status_text()
            .contains("Pasted a - Copy (2).txt into docs"),
        "{}",
        app.status_text()
    );
    assert_eq!(read(p("a.txt")), "hello");

    // Duplicate: a copy next to the original.
    click_tree(&mut app, &p("e.txt"), 1);
    focus_tree(&mut app);
    key_done(&mut app, "ctrl+d");
    assert_eq!(read(p("e - Copy.txt")), "eee");
    assert!(app.status_text().contains("Duplicated as e - Copy.txt"));
    click_with(&mut app, &p("e - Copy.txt"), CTRL);
    click_with(&mut app, &p("e.txt"), CTRL);
    assert_eq!(selected(&mut app), ["e.txt"]);
    key_done(&mut app, "ctrl+d");
    assert_eq!(read(p("e - Copy (2).txt")), "eee");
    assert_eq!(read(p("e - Copy.txt")), "eee");

    // The menu: copy a folder, paste it on a file (= into its folder),
    // duplicate a folder; exact bytes.
    let bytes: Vec<u8> = (0..=255u8).cycle().take(70_000).collect();
    std::fs::write(p("notes/x.bin"), &bytes).unwrap();
    right_click(&mut app, &p("notes"));
    run_menu_item(&mut app, "Copy");
    right_click(&mut app, &p("docs/a.txt"));
    menu_done(&mut app, "Paste");
    assert_eq!(std::fs::read(p("docs/notes/x.bin")).unwrap(), bytes);
    assert_eq!(read(p("docs/notes/b.md")), "bee\n");
    right_click(&mut app, &p("notes"));
    menu_done(&mut app, "Duplicate");
    assert_eq!(contents(&p("notes - Copy")), contents(&p("notes")));
    right_click(&mut app, &p("notes"));
    menu_done(&mut app, "Duplicate");
    assert_eq!(contents(&p("notes - Copy (2)")), contents(&p("notes")));

    // A folder is never pasted into itself (or below itself).
    let inside = std::fs::read_dir(p("notes")).unwrap().count();
    right_click(&mut app, &p("notes"));
    menu_done(&mut app, "Paste");
    assert_eq!(
        app.nag_message().as_deref(),
        Some("Pasted 0 of 1 into notes. Not copied: notes (a folder cannot be copied into itself)")
    );
    key(&mut app, "enter");
    assert_eq!(std::fs::read_dir(p("notes")).unwrap().count(), inside);
    // Several at once, with one gone meanwhile: the rest is pasted.
    click_tree(&mut app, &p("d.txt"), 1);
    click_with(&mut app, &p("e.txt"), CTRL);
    click_with(&mut app, &p("a.txt"), CTRL);
    key(&mut app, "ctrl+c");
    assert!(app.status_text().contains("Copied 3 items"));
    std::fs::remove_file(p("e.txt")).unwrap();
    click_tree(&mut app, &p("notes"), 1);
    key_done(&mut app, "ctrl+v");
    assert_eq!(
        app.nag_message().as_deref(),
        Some("Pasted 2 of 3 into notes. Not copied: e.txt (it no longer exists)")
    );
    key(&mut app, "enter");
    assert_eq!(read(p("notes/a.txt")), "hello");
    assert_eq!(read(p("notes/d.txt")), "dee");
    cleanup(&ws);
}

#[test]
fn copies_over_the_limit_are_refused_and_big_ones_run_in_slices() {
    let (mut app, ws, _bin) = setup("slices");
    let p = |n: &str| ws.join(n);
    std::fs::create_dir_all(p("many")).unwrap();
    for i in 0..5 {
        std::fs::write(p(&format!("many/f{i}")), "0123456789").unwrap();
    }
    app.handle(Input::FsChanged(vec![p("many")]));

    // 6 entries, 50 bytes: over a limit of 5 entries / of 49 bytes.
    for (entries, bytes, why) in [
        (5, 1000, "more than 5 files and folders"),
        (100, 49, "more than 49 bytes"),
    ] {
        app.set_copy_limits(entries, bytes);
        right_click(&mut app, &p("many"));
        run_menu_item(&mut app, "Copy");
        right_click(&mut app, &p("docs"));
        menu_done(&mut app, "Paste");
        assert_eq!(
            app.nag_message(),
            Some(format!(
                "Nothing was copied: that is {why}, the most one copy may hold."
            ))
        );
        key(&mut app, "enter");
        assert_eq!(std::fs::read_dir(p("docs")).unwrap().count(), 0);
        assert!(!app.copy_running());
    }
    app.set_copy_limits(6, 50);
    right_click(&mut app, &p("docs"));
    menu_done(&mut app, "Paste");
    assert_eq!(app.nag_message(), None);
    assert_eq!(contents(&p("docs/many")), contents(&p("many")));

    // A bigger file: the copy goes on in the background, a slice per tick,
    // with its progress in the status bar.
    app.set_copy_limits(10_000, 1 << 30);
    app.set_copy_first_slice(Duration::ZERO);
    let big: Vec<u8> = (0..2_000_000u32).map(|i| (i % 253) as u8).collect();
    std::fs::write(p("big.bin"), &big).unwrap();
    app.handle(Input::FsChanged(vec![p("big.bin")]));
    right_click(&mut app, &p("big.bin"));
    run_menu_item(&mut app, "Copy");
    right_click(&mut app, &p("docs"));
    run_menu_item(&mut app, "Paste");
    assert!(app.copy_running() && app.has_pending_work());
    assert!(
        app.status_text().contains("Copying…"),
        "{}",
        app.status_text()
    );
    // Meanwhile nothing else changes files in the tree.
    click_tree(&mut app, &p("a.txt"), 1);
    click_with(&mut app, &p("d.txt"), CTRL);
    key(&mut app, "delete");
    assert_eq!(app.nag_message(), None);
    assert!(app.status_text().contains("A copy is still running"));
    key(&mut app, "ctrl+d");
    assert!(app.status_text().contains("A copy is still running"));
    click_tree(&mut app, &p("e.txt"), 1);
    focus_tree(&mut app);
    key(&mut app, "delete");
    assert_eq!(app.nag_message(), None);
    key(&mut app, "f2");
    render_hash(&mut app, 800, 600);
    assert!(app.status_text().contains("A copy is still running"));
    let mut ticks = 0;
    while app.copy_running() {
        assert!(app.tick(Duration::ZERO).more || !app.copy_running());
        ticks += 1;
        assert!(ticks < 10_000);
    }
    assert!(ticks >= 5, "the copy took {ticks} ticks");
    assert!(!app.has_pending_work());
    assert_eq!(std::fs::read(p("docs/big.bin")).unwrap(), big);
    assert!(app.status_text().contains("Pasted big.bin into docs"));
    assert!(p("a.txt").exists() && p("d.txt").exists() && p("e.txt").exists());

    // Closing the window mid-copy: the half-written file is removed.
    right_click(&mut app, &p("docs"));
    run_menu_item(&mut app, "Paste");
    app.tick(Duration::ZERO);
    app.tick(Duration::ZERO);
    assert!(app.copy_running());
    let partial = std::fs::metadata(p("docs/big - Copy.bin")).unwrap().len();
    assert!(partial < big.len() as u64, "{partial}");
    app.handle(Input::CloseRequested);
    assert!(app.take_requests().contains(&Request::Quit));
    assert!(!app.copy_running());
    assert!(!p("docs/big - Copy.bin").exists());
    assert_eq!(std::fs::read(p("docs/big.bin")).unwrap(), big);
    cleanup(&ws);
}

#[cfg(unix)]
#[test]
fn copying_a_folder_skips_links_and_says_so() {
    let (mut app, ws, _bin) = setup("links");
    let p = |n: &str| ws.join(n);
    let outside = ws.parent().unwrap().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("secret.txt"), "secret").unwrap();
    std::os::unix::fs::symlink(&outside, p("notes/out")).unwrap();
    std::os::unix::fs::symlink(&ws, p("notes/loop")).unwrap();
    app.handle(Input::FsChanged(vec![p("notes/out")]));
    right_click(&mut app, &p("notes"));
    run_menu_item(&mut app, "Copy");
    right_click(&mut app, &p("docs"));
    menu_done(&mut app, "Paste");
    let msg = app.nag_message().unwrap_or_default();
    assert!(
        msg.starts_with("Pasted notes into docs. Skipped 2 links or special files ("),
        "{msg}"
    );
    assert!(msg.contains("out") && msg.contains("loop"), "{msg}");
    key(&mut app, "enter");
    assert_eq!(
        contents(&p("docs/notes")),
        [b"bee\n".to_vec(), b"sea\n".to_vec()]
    );
    assert_eq!(std::fs::read_dir(p("docs/notes")).unwrap().count(), 2);
    assert_eq!(read(outside.join("secret.txt")), "secret");

    // A link on its own is not copied either.
    click_tree(&mut app, &p("notes"), 1);
    right_click(&mut app, &p("notes/out"));
    menu_done(&mut app, "Duplicate");
    let msg = app.nag_message().unwrap_or_default();
    assert!(
        msg.starts_with("Duplicated nothing. Skipped 1 link or special file (out)"),
        "{msg}"
    );
    key(&mut app, "enter");
    assert!(!p("notes/out - Copy").exists());
    cleanup(&ws);
}

#[test]
fn the_status_bar_shows_size_and_age_and_follows_saves_and_changes() {
    let (mut app, ws, _bin) = setup("status");
    let p = |n: &str| ws.join(n);
    assert!(
        !app.status_text().contains("changed"),
        "no file, no details"
    );
    // An older file: 3 hours and a bit.
    let old = std::time::SystemTime::now() - Duration::from_secs(3 * 3600 + 1200);
    std::fs::File::options()
        .write(true)
        .open(p("d.txt"))
        .unwrap()
        .set_modified(old)
        .unwrap();

    app.open(&p("a.txt"));
    render_hash(&mut app, 800, 600);
    assert!(
        app.status_text().ends_with("5 bytes · changed just now"),
        "{}",
        app.status_text()
    );
    // Drawing again and again does not touch the disk.
    let stats = app.file_stat_count();
    let with_details = render_hash(&mut app, 800, 600);
    for _ in 0..5 {
        assert_eq!(render_hash(&mut app, 800, 600), with_details);
        app.handle(Input::MouseMove { x: 500.0, y: 300.0 });
    }
    assert_eq!(app.file_stat_count(), stats);

    // Saving updates it.
    mouse(&mut app, 500.0, 300.0, MouseButton::Left);
    key(&mut app, "ctrl+end");
    app.handle(Input::Text(" world".into()));
    assert!(app.status_text().contains("5 bytes"), "not until saved");
    key(&mut app, "ctrl+s");
    assert!(
        app.status_text().ends_with("11 bytes · changed just now"),
        "{}",
        app.status_text()
    );
    assert_eq!(app.file_stat_count(), stats + 1);
    assert_ne!(render_hash(&mut app, 800, 600), with_details);

    // So does a change by another program.
    std::fs::write(p("a.txt"), vec![b'x'; 12_698]).unwrap();
    app.handle(Input::FsChanged(vec![p("a.txt")]));
    assert!(
        app.status_text().ends_with("12.4 KB · changed just now"),
        "{}",
        app.status_text()
    );
    // A change to some other file does not.
    let stats = app.file_stat_count();
    app.handle(Input::FsChanged(vec![p("e.txt")]));
    assert_eq!(app.file_stat_count(), stats);

    // Another tab: its own details, read once.
    app.open(&p("d.txt"));
    render_hash(&mut app, 800, 600);
    render_hash(&mut app, 800, 600);
    assert!(
        app.status_text().ends_with("3 bytes · changed 3 h ago"),
        "{}",
        app.status_text()
    );
    assert_eq!(app.file_stat_count(), stats + 1);
    cleanup(&ws);
}

#[test]
fn ctrl_c_v_d_in_the_editor_still_copy_paste_and_select_the_next_match() {
    let (mut app, ws, _bin) = setup("editor-keys");
    let p = |n: &str| ws.join(n);
    std::fs::write(p("words.txt"), "foo bar foo\n").unwrap();
    app.handle(Input::FsChanged(vec![p("words.txt")]));
    app.open(&p("words.txt"));
    render_hash(&mut app, 800, 600);
    mouse(&mut app, 500.0, 300.0, MouseButton::Left);
    assert_eq!(app.focus(), crate::app::Focus::Main);
    let files = contents(&ws).len();

    // Ctrl+D: the word at the caret, then the next match; typing replaces both.
    key(&mut app, "ctrl+home");
    key_done(&mut app, "ctrl+d");
    key(&mut app, "ctrl+c");
    assert_eq!(app.clipboard_text().as_deref(), Some("foo"));
    key_done(&mut app, "ctrl+d");
    app.handle(Input::Text("x".into()));
    assert_eq!(app.view_text(), "x bar x\n");
    // Ctrl+V pastes the text.
    key(&mut app, "escape");
    key(&mut app, "ctrl+end");
    key_done(&mut app, "ctrl+v");
    assert_eq!(app.view_text(), "x bar x\nfoo");
    // None of that touched files or the tree's copy list.
    assert!(app.copied_entries().is_empty());
    assert_eq!(contents(&ws).len(), files);
    assert_eq!(app.nag_message(), None);

    // With the tree focused the same keys are about files, and the text
    // stays as it is.
    focus_tree(&mut app);
    click_with(&mut app, &p("e.txt"), CTRL);
    click_with(&mut app, &p("words.txt"), CTRL);
    assert_eq!(selected(&mut app), ["e.txt"]);
    key(&mut app, "ctrl+c");
    key_done(&mut app, "ctrl+d");
    key_done(&mut app, "ctrl+v");
    assert_eq!(app.copied_entries(), [p("e.txt")]);
    assert_eq!(read(p("e - Copy.txt")), "eee");
    assert_eq!(read(p("e - Copy (2).txt")), "eee");
    assert_eq!(app.view_text(), "x bar x\nfoo");
    assert_eq!(app.clipboard_text().as_deref(), Some("foo"));
    cleanup(&ws);
}

#[test]
fn the_status_bar_shows_the_details_of_the_file_selected_in_the_tree() {
    let (mut app, ws, _bin) = setup("status-tree");
    let p = |n: &str| ws.join(n);
    click_tree(&mut app, &p("a.txt"), 1); // the open file: 5 bytes
    // Select d.txt (3 bytes) without opening it: Ctrl+click on and off.
    click_with(&mut app, &p("d.txt"), CTRL);
    click_with(&mut app, &p("a.txt"), CTRL);
    assert_eq!(selected(&mut app), ["d.txt"]);
    assert_eq!(app.active_path(), Some(p("a.txt")));
    render_hash(&mut app, 800, 600);
    assert!(
        app.status_text().ends_with("3 bytes · changed just now"),
        "the selected file, not the open one: {}",
        app.status_text()
    );
    // Several selected: the open file again.
    click_with(&mut app, &p("e.txt"), CTRL);
    render_hash(&mut app, 800, 600);
    assert!(app.status_text().ends_with("5 bytes · changed just now"));
    // A folder: no size shown.
    click_tree(&mut app, &p("docs"), 1);
    render_hash(&mut app, 800, 600);
    assert!(
        !app.status_text().contains("changed"),
        "{}",
        app.status_text()
    );
    // Keyboard back in the editor: the open file's details.
    click_with(&mut app, &p("d.txt"), CTRL);
    click_with(&mut app, &p("docs"), CTRL);
    assert_eq!(selected(&mut app), ["d.txt"]);
    render_hash(&mut app, 800, 600);
    assert!(app.status_text().ends_with("3 bytes · changed just now"));
    mouse(&mut app, 500.0, 300.0, MouseButton::Left);
    render_hash(&mut app, 800, 600);
    assert!(
        app.status_text().ends_with("5 bytes · changed just now"),
        "{}",
        app.status_text()
    );
    cleanup(&ws);
}

#[test]
fn several_workspace_folders_moves_follow_open_tabs_and_roots_are_refused() {
    let (mut app, ws, bin) = setup("roots");
    let p = |n: &str| ws.join(n);
    let other = ws.parent().unwrap().join("other");
    std::fs::create_dir_all(other.join("inbox")).unwrap();
    std::fs::write(other.join("o.txt"), "oh").unwrap();
    app.add_folder(&other);
    render_hash(&mut app, 800, 600);
    app.open(&p("d.txt"));
    click_tree(&mut app, &p("a.txt"), 1);
    click_with(&mut app, &p("d.txt"), CTRL);
    // Drag both into the other workspace folder (one question, a count).
    drag_rows(&mut app, &p("a.txt"), &other.join("inbox"));
    assert_eq!(
        app.nag_message().as_deref(),
        Some("Move 2 items into inbox? (a.txt, d.txt)")
    );
    key(&mut app, "enter");
    assert_eq!(app.nag_message(), None);
    assert_eq!(read(other.join("inbox/a.txt")), "hello");
    assert_eq!(read(other.join("inbox/d.txt")), "dee");
    assert!(!p("a.txt").exists() && !p("d.txt").exists());
    // The tab that showed d.txt now shows the moved file.
    let tabs = app.tab_count();
    app.open(&other.join("inbox/d.txt"));
    assert_eq!(app.tab_count(), tabs, "{:?}", app.tab_titles());

    // A workspace folder in a selection is never acted on.
    focus_tree(&mut app);
    click_tree(&mut app, &ws, 1);
    click_with(&mut app, &other, CTRL);
    key(&mut app, "delete");
    assert_eq!(app.nag_message(), None, "roots cannot be deleted");
    key(&mut app, "ctrl+c");
    assert!(app.copied_entries().is_empty());
    key(&mut app, "ctrl+d");
    assert_eq!(app.nag_message(), None);
    // Even asked for directly, a workspace folder is dropped from the list.
    app.ask_delete_paths(vec![ws.clone(), other.clone()]);
    assert_eq!(app.nag_message(), None);
    app.ask_delete_paths(vec![ws.clone(), p("e.txt")]);
    assert_eq!(
        app.nag_message().as_deref(),
        Some("Move e.txt to the Recycle Bin?")
    );
    key(&mut app, "escape");
    // A range over both roots takes the entries and leaves the roots.
    click_tree(&mut app, &p("e.txt"), 1);
    click_with(&mut app, &other.join("o.txt"), SHIFT);
    let sel = app.tree_selection();
    assert!(!sel.contains(&ws) && !sel.contains(&other), "{sel:?}");
    assert!(sel.contains(&p("e.txt")) && sel.contains(&other.join("o.txt")));
    key(&mut app, "delete");
    let msg = app.nag_message().unwrap();
    assert!(msg.starts_with("Move "), "{msg}");
    key(&mut app, "escape");
    assert!(p("e.txt").exists() && other.join("o.txt").exists());
    assert!(contents(&bin).is_empty());
    cleanup(&ws);
    let _ = std::fs::remove_dir_all(&other);
}

#[test]
fn entries_that_vanished_before_the_answer_are_reported_not_fatal() {
    let (mut app, ws, bin) = setup("vanished");
    let p = |n: &str| ws.join(n);
    click_tree(&mut app, &p("a.txt"), 1);
    click_with(&mut app, &p("d.txt"), CTRL);
    click_with(&mut app, &p("e.txt"), CTRL);
    key(&mut app, "delete");
    assert!(app.nag_message().unwrap().starts_with("Move 3 items"));
    std::fs::remove_file(p("d.txt")).unwrap();
    key(&mut app, "enter");
    assert_eq!(
        app.nag_message().as_deref(),
        Some("Moved 2 of 3 to the Recycle Bin. Not deleted: d.txt (it no longer exists)")
    );
    key(&mut app, "enter");
    assert_eq!(contents(&bin), [b"eee".to_vec(), b"hello".to_vec()]);

    // A file that vanished from the tree leaves the selection: the count in
    // the status bar and "several selected" follow what is really there.
    click_tree(&mut app, &p("docs"), 1);
    std::fs::write(p("g.txt"), "g").unwrap();
    std::fs::write(p("h.txt"), "h").unwrap();
    app.handle(Input::FsChanged(vec![p("g.txt")]));
    click_tree(&mut app, &p("g.txt"), 1);
    click_with(&mut app, &p("h.txt"), CTRL);
    render_hash(&mut app, 800, 600);
    assert!(app.status_text().contains("2 selected in the tree"));
    std::fs::remove_file(p("g.txt")).unwrap();
    app.handle(Input::FsChanged(vec![p("g.txt")]));
    render_hash(&mut app, 800, 600);
    assert_eq!(selected(&mut app), ["h.txt"]);
    assert!(
        !app.status_text().contains("selected in the tree"),
        "{}",
        app.status_text()
    );
    key(&mut app, "delete");
    assert_eq!(
        app.nag_message().as_deref(),
        Some("Move h.txt to the Recycle Bin?"),
        "one entry left: the single question"
    );
    key(&mut app, "escape");

    // The same for a move (new names: the tab of g.txt, whose file is gone,
    // counts as having edits to keep).
    std::fs::write(p("i.txt"), "i").unwrap();
    std::fs::write(p("j.txt"), "j").unwrap();
    app.handle(Input::FsChanged(vec![p("i.txt")]));
    click_tree(&mut app, &p("i.txt"), 1);
    click_with(&mut app, &p("j.txt"), CTRL);
    drag_rows(&mut app, &p("i.txt"), &p("docs"));
    std::fs::remove_file(p("i.txt")).unwrap();
    key(&mut app, "enter");
    let msg = app.nag_message().unwrap();
    assert!(
        msg.starts_with("Moved 1 of 2 into docs. Not moved: i.txt ("),
        "{msg}"
    );
    key(&mut app, "enter");
    assert_eq!(read(p("docs/j.txt")), "j");
    cleanup(&ws);
}

/// Random tree actions (select, delete, drag, copy, paste, duplicate, edit
/// and save, with every question answered at random): no file content is
/// ever lost. After every step each file that existed before is still
/// somewhere under the folder or in the recycle folder, byte for byte.
#[test]
fn tree_action_fuzz_never_loses_file_content() {
    let (mut app, ws, bin) = setup("fuzz");
    let p = |n: &str| ws.join(n);
    // Same names in several folders, so moves and pastes clash.
    std::fs::create_dir_all(p("docs/deep")).unwrap();
    for (name, text) in [
        ("docs/a.txt", "docs a"),
        ("docs/b.md", "docs b\n"),
        ("docs/deep/a.txt", "deep a"),
        ("docs/deep/z.txt", "deep z"),
        ("notes/a.txt", "notes a"),
        ("notes/d.txt", "notes d"),
    ] {
        std::fs::write(p(name), text).unwrap();
    }
    app.handle(Input::FsChanged(vec![p("docs/a.txt"), p("notes/a.txt")]));
    click_tree(&mut app, &p("docs"), 1);
    click_tree(&mut app, &p("notes"), 1);

    let mut seed = 0x9E3779B97F4A7C15u64;
    let mut rnd = |n: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % n.max(1) as u64) as usize
    };
    let all = |ws: &Path, bin: &Path| {
        let mut v = contents(ws);
        v.extend(contents(bin));
        v.sort();
        v
    };
    // `need` ⊆ `have`, as multisets (both sorted).
    let missing = |need: &[Vec<u8>], have: &[Vec<u8>]| -> Option<Vec<u8>> {
        let mut have = have.iter().peekable();
        for n in need {
            loop {
                match have.next() {
                    Some(h) if h == n => break,
                    Some(h) if h < n => continue,
                    _ => return Some(n.clone()),
                }
            }
        }
        None
    };
    let (mut recycled, mut moved, mut copied, mut saved, mut refused) = (0, 0, 0, 0, 0);
    for step in 0..300 {
        let before = all(&ws, &bin);
        let in_bin = contents(&bin).len();
        let files = before.len() - in_bin;
        let rows = app.tree_rows();
        let visible: Vec<PathBuf> = rows
            .iter()
            .filter(|r| app.tree_row_y(r).is_some_and(|y| y < 560.0))
            .cloned()
            .collect();
        let pick = |rnd: &mut dyn FnMut(usize) -> usize| visible[rnd(visible.len())].clone();
        // A file being saved in this step, with its bytes before.
        let mut saving: Option<(PathBuf, Vec<u8>)> = None;
        let paths_before = file_paths(&ws);
        match rnd(12) {
            0 => click_tree(&mut app, &pick(&mut rnd), 1),
            1 | 2 => click_with(&mut app, &pick(&mut rnd), CTRL),
            3 => click_with(&mut app, &pick(&mut rnd), SHIFT),
            4 => {
                focus_tree(&mut app);
                key(&mut app, "delete");
            }
            5 | 6 => {
                // Drag an entry (or the selection it is part of) onto another
                // row, more often than not a folder.
                let a = pick(&mut rnd);
                let folders: Vec<&PathBuf> = visible.iter().filter(|r| r.is_dir()).collect();
                let b = if rnd(3) > 0 {
                    folders[rnd(folders.len())].clone()
                } else {
                    pick(&mut rnd)
                };
                drag_rows(&mut app, &a, &b);
            }
            7 => {
                focus_tree(&mut app);
                key(
                    &mut app,
                    ["ctrl+c", "ctrl+a", "down", "up", "shift+down"][rnd(5)],
                );
            }
            8 if files < 120 => {
                focus_tree(&mut app);
                key(&mut app, ["ctrl+v", "ctrl+d"][rnd(2)]);
            }
            9 => {
                right_click(&mut app, &pick(&mut rnd));
                let labels = app.menu_labels();
                let wanted: Vec<&String> = labels
                    .iter()
                    .filter(|l| {
                        l.starts_with("Delete")
                            || l.starts_with("Copy ")
                            || *l == "Copy"
                            || (files < 120 && (*l == "Paste" || l.starts_with("Duplicate")))
                    })
                    .collect();
                if wanted.is_empty() {
                    key(&mut app, "escape");
                } else {
                    let label = wanted[rnd(wanted.len())].clone();
                    run_menu_item(&mut app, &label);
                }
            }
            10 => {
                // Type into the shown file; sometimes save it.
                if !app.view_text().is_empty() {
                    mouse(&mut app, 500.0, 300.0, MouseButton::Left);
                    app.handle(Input::Text(format!("<{step}>")));
                    if rnd(2) == 0
                        && let Some(f) = app.active_path()
                        && let Ok(old) = std::fs::read(&f)
                    {
                        key(&mut app, "ctrl+s");
                        saving = Some((f, old));
                    }
                }
            }
            _ => {
                key(&mut app, "ctrl+w");
                if app.nag_message().is_some() {
                    key(&mut app, "escape");
                }
            }
        }
        // Answer whatever is asked: yes or no at random; reports with OK.
        let mut asked = 0;
        while let Some(q) = app.nag_message() {
            if q.starts_with("Nothing was") {
                refused += 1;
            }
            let options = app.nag_options();
            let yes = options.len() == 1 || (rnd(3) > 0 && !options.contains(&"Save".to_string()));
            key(&mut app, if yes { "enter" } else { "escape" });
            app.settle(Duration::from_secs(10));
            asked += 1;
            assert!(asked < 10, "step {step}: questions never end ({q})");
        }
        key(&mut app, "escape"); // a menu or prompt left open
        app.settle(Duration::from_secs(10));
        assert!(!app.copy_running());
        let _ = render_hash(&mut app, 800, 600);

        let after = all(&ws, &bin);
        let mut need = before.clone();
        // A save replaces exactly that file's bytes by the new text.
        if let Some((f, old)) = saving
            && let Ok(new) = std::fs::read(&f)
            && new != old
        {
            let i = need.iter().position(|c| *c == old).unwrap();
            need[i] = new;
            need.sort();
            saved += 1;
        }
        if let Some(lost) = missing(&need, &after) {
            panic!(
                "step {step}: a file with these bytes is gone: {:?}",
                String::from_utf8_lossy(&lost)
            );
        }
        let now_bin = contents(&bin).len();
        recycled += now_bin - in_bin;
        copied += after.len().saturating_sub(before.len());
        if after.len() == before.len() && now_bin == in_bin && file_paths(&ws) != paths_before {
            moved += 1;
        }
    }
    eprintln!("recycled {recycled} copied {copied} moved {moved} saved {saved} refused {refused}");
    // The run really did all of these.
    assert!(recycled > 5, "recycled {recycled}");
    assert!(copied > 5, "copied {copied}");
    assert!(moved > 5, "moved {moved}");
    assert!(saved > 2, "saved {saved}");
    assert!(refused > 0, "refused {refused}");
    cleanup(&ws);
}

// ---- from the independent review of Epic 7 ----

#[test]
fn a_folder_holding_a_workspace_folder_is_never_deleted_moved_or_renamed() {
    let (mut app, ws, bin) = setup("nestedroot");
    let p = |n: &str| ws.join(n);
    // (A target above `docs`, so its row doesn't move when `docs` opens.)
    std::fs::create_dir_all(p("aaa")).unwrap();
    std::fs::create_dir_all(p("docs/inner")).unwrap();
    std::fs::write(p("docs/inner/keep.txt"), "keep").unwrap();
    app.handle(Input::FsChanged(vec![p("docs"), p("aaa")]));
    app.add_folder(&p("docs/inner"));
    render_hash(&mut app, 800, 600);
    let before = contents(&ws);
    let refused = |app: &mut App, done: &str| {
        let m = app.nag_message().unwrap_or_default();
        assert!(
            m.starts_with(&format!(
                "Nothing was {done}: docs contains the workspace folder inner"
            )),
            "{done}: {m:?} status {:?} sel {:?}",
            app.status_text(),
            app.tree_selection()
        );
        assert_eq!(app.nag_options(), ["OK"]);
        key(app, "enter");
        assert_eq!(app.nag_message(), None);
    };
    // Several entries: delete, and drag onto another folder.
    click_tree(&mut app, &p("docs"), 1);
    click_with(&mut app, &p("a.txt"), CTRL);
    key(&mut app, "delete");
    refused(&mut app, "deleted");
    drag_rows(&mut app, &p("a.txt"), &p("aaa"));
    refused(&mut app, "moved");
    // The folder alone: delete, rename, drag.
    click_tree(&mut app, &p("docs"), 1);
    key(&mut app, "delete");
    refused(&mut app, "deleted");
    key(&mut app, "f2");
    refused(&mut app, "renamed");
    drag_rows(&mut app, &p("docs"), &p("aaa"));
    refused(&mut app, "moved");
    assert_eq!(contents(&ws), before);
    assert!(contents(&bin).is_empty());
    assert!(p("docs/inner/keep.txt").exists());
    cleanup(&ws);
}

#[cfg(unix)]
#[test]
fn file_details_are_never_read_through_a_link() {
    let (mut app, ws, _bin) = setup("linkstat");
    let p = |n: &str| ws.join(n);
    let outside = ws.parent().unwrap().join("outside.bin");
    std::fs::write(&outside, vec![7u8; 12_698]).unwrap();
    std::os::unix::fs::symlink(&outside, p("link.bin")).unwrap();
    app.handle(Input::FsChanged(vec![p("link.bin")]));
    render_hash(&mut app, 800, 600);
    click_with(&mut app, &p("a.txt"), CTRL);
    click_with(&mut app, &p("a.txt"), CTRL);
    click_with(&mut app, &p("a.txt"), CTRL);
    render_hash(&mut app, 800, 600);
    assert!(
        app.status_text().contains("5 bytes"),
        "{}",
        app.status_text()
    );
    click_with(&mut app, &p("a.txt"), CTRL);
    click_with(&mut app, &p("link.bin"), CTRL);
    render_hash(&mut app, 800, 600);
    let s = app.status_text();
    assert!(!s.contains("KB") && !s.contains("bytes"), "{s}");
    cleanup(&ws);
}

#[test]
fn a_quit_that_is_called_off_does_not_stop_a_running_copy() {
    let (mut app, ws, _bin) = setup("quitcopy");
    let p = |n: &str| ws.join(n);
    app.set_copy_first_slice(Duration::ZERO);
    let big: Vec<u8> = (0..2_000_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(p("big.bin"), &big).unwrap();
    app.handle(Input::FsChanged(vec![p("big.bin")]));
    // A file with unsaved edits, so closing asks first.
    app.open(&p("a.txt"));
    app.handle(Input::Text("x".into()));
    right_click(&mut app, &p("big.bin"));
    run_menu_item(&mut app, "Copy");
    right_click(&mut app, &p("docs"));
    run_menu_item(&mut app, "Paste");
    app.tick(Duration::ZERO);
    assert!(app.copy_running());
    app.handle(Input::CloseRequested);
    assert!(app.nag_message().is_some());
    // Another launch sends a file: the window stays open after the answer.
    app.open_from_other_launch(vec![p("d.txt")]);
    key(&mut app, "d");
    while app.nag_message().is_some() {
        key(&mut app, "d");
    }
    assert!(!app.take_requests().contains(&Request::Quit));
    assert!(app.copy_running(), "the copy goes on");
    app.settle(Duration::from_secs(20));
    assert!(!app.copy_running());
    assert_eq!(std::fs::read(p("docs/big.bin")).unwrap(), big);
    cleanup(&ws);
}

#[test]
fn deleting_a_whole_big_folder_selection_does_not_freeze() {
    let (mut app, ws, bin) = setup("bigdelete");
    std::fs::create_dir_all(ws.join("many")).unwrap();
    for i in 0..3000 {
        std::fs::write(ws.join(format!("many/f{i:04}.txt")), "x").unwrap();
    }
    app.handle(Input::FsChanged(vec![ws.join("many")]));
    click_tree(&mut app, &ws.join("many"), 1); // open the folder
    render_hash(&mut app, 800, 600);
    key(&mut app, "down");
    let t = std::time::Instant::now();
    for _ in 0..2999 {
        key(&mut app, "shift+down");
    }
    let selecting = t.elapsed();
    assert!(
        selecting < Duration::from_secs(20),
        "selecting took {selecting:?}"
    );
    let t = std::time::Instant::now();
    key(&mut app, "delete");
    let asked = t.elapsed();
    let m = app.nag_message().unwrap_or_default();
    assert!(
        m.starts_with("Move 3,000 items") || m.starts_with("Move 3000 items"),
        "{m}"
    );
    let t = std::time::Instant::now();
    key(&mut app, "enter");
    let done = t.elapsed();
    assert_eq!(std::fs::read_dir(ws.join("many")).unwrap().count(), 0);
    assert_eq!(std::fs::read_dir(&bin).unwrap().count(), 3000);
    assert!(
        asked < Duration::from_secs(3) && done < Duration::from_secs(15),
        "asking took {asked:?}, deleting {done:?}"
    );
    cleanup(&ws);
}
