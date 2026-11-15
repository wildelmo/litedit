//! S7.1 big files: syntax colours only as far as the view needs them, a
//! lighter load path, and the read-only lock.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::app::App;
use crate::editor::buffer::Buffer;
use crate::input::{Input, WheelUnit};
use crate::syntax::{LineHighlighter, Span, find_syntax};
use crate::tests::{key, new_app, render_hash, tmp_dir, type_text};

/// Lines in the big test file: far more than any margin, small enough for
/// a debug build.
const BIG: usize = 12_000;
/// Opening the file may colour this many lines at the most (one screen plus
/// the margin is about 250).
const PREFIX_LIMIT: usize = 1_000;

/// A Rust file whose colours near the end depend on its top: a block
/// comment opened on line 10 is closed 12 lines before the end. Colouring
/// that started anywhere but the first line gets the tail wrong.
fn stateful_source(lines: usize) -> String {
    let close = lines - 12;
    (0..lines)
        .map(|i| {
            if i == 10 {
                "/* opened here\n".to_string()
            } else if i == close {
                "closed here */ fn after() -> u32 { 7 }\n".to_string()
            } else {
                format!("fn f{i}(x: i32) -> i32 {{ let s = \"text {i}\"; x + {i} }} // note\n")
            }
        })
        .collect()
}

fn big_file(tag: &str, name: &str) -> (PathBuf, PathBuf) {
    let dir = tmp_dir(&format!("bigfile-{tag}"));
    let p = dir.join(name);
    std::fs::write(&p, stateful_source(BIG)).unwrap();
    (dir, p)
}

fn frame(app: &mut App) -> u64 {
    render_hash(app, 800, 600)
}

/// What the window does after input: background work runs in 8 ms slices
/// only while the app says some is pending, and a frame is drawn when a
/// slice asks for one. The first slice can come before or after the frame
/// (`tick_first`). Returns the number of slices; fails if the app never
/// goes idle.
fn pump_sized(app: &mut App, (w, h): (u32, u32), tick_first: bool) -> usize {
    let mut ticks = 0;
    if tick_first && app.has_pending_work() {
        app.tick(Duration::from_millis(8));
        ticks += 1;
    }
    render_hash(app, w, h);
    while app.has_pending_work() {
        ticks += 1;
        assert!(ticks < 100_000, "background work never finished");
        if app.tick(Duration::from_millis(8)).redraw {
            render_hash(app, w, h);
        }
    }
    ticks
}

fn pump(app: &mut App) -> usize {
    pump_sized(app, (800, 600), true)
}

/// Every line's colours from one uninterrupted pass over the whole text.
fn reference(app: &App, path: &Path) -> Vec<Vec<Span>> {
    let (text, _) = app.snapshot().expect("a text view");
    reference_for(Buffer::from_text(&text).lines(), path)
}

fn reference_for(lines: &[String], path: &Path) -> Vec<Vec<Span>> {
    let mut h = LineHighlighter::new(find_syntax(path, &lines[0]));
    while h.step(lines, Duration::from_secs(1)) {}
    (0..lines.len())
        .map(|i| h.spans(i).unwrap_or(&[]).to_vec())
        .collect()
}

fn done(app: &App) -> usize {
    app.highlight_probe(0).expect("a text view").0
}

/// The colours line `i` was last drawn with.
fn drawn(app: &App, i: usize) -> Option<Vec<Span>> {
    app.highlight_probe(i).expect("a text view").1
}

#[track_caller]
fn assert_drawn_like(app: &App, want: &[Vec<Span>], lines: std::ops::Range<usize>) {
    for i in lines {
        assert_eq!(
            drawn(app, i).as_ref(),
            Some(&want[i]),
            "line {} is not drawn with its colours",
            i + 1
        );
    }
}

#[test]
fn opening_a_big_file_colours_only_what_is_shown() {
    let (dir, p) = big_file("open", "big.rs");
    let mut app = new_app();
    app.open(&p);
    pump(&mut app);
    let want = reference(&app, &p);
    assert!(
        done(&app) < PREFIX_LIMIT,
        "{} lines coloured just for opening",
        done(&app)
    );
    // …but everything on screen is coloured, the open comment included.
    assert_drawn_like(&app, &want, 0..20);
    assert!(!want[5].is_empty() && want[11].len() == 1);
    // Finishing "all deferred work" (scripts, screenshots) is not a reason
    // to colour the rest either.
    app.settle(Duration::from_secs(20));
    frame(&mut app);
    app.settle(Duration::from_secs(20));
    assert!(done(&app) < PREFIX_LIMIT);
    // Moving around the top doesn't change that.
    for k in ["down", "pagedown", "pagedown", "pageup", "end", "ctrl+home"] {
        key(&mut app, k);
        pump(&mut app);
    }
    assert!(done(&app) < PREFIX_LIMIT);
    std::fs::remove_dir_all(dir).ok();
}

/// Work can start before the first frame, when the view doesn't know its
/// size yet: a window taller than the default look-ahead still ends up
/// coloured to its last line.
#[test]
fn a_very_tall_window_is_coloured_to_the_bottom() {
    let (dir, p) = big_file("tall", "big.rs");
    let mut app = new_app();
    app.handle(Input::Resize(800, 6000));
    app.open(&p);
    // All the work there is before the first frame: the default look-ahead.
    app.settle(Duration::from_secs(60));
    assert!(!app.has_pending_work());
    assert!((200..260).contains(&done(&app)), "{}", done(&app));
    // The frame shows more lines than that; the rest follows.
    pump_sized(&mut app, (800, 6000), false);
    let want = reference(&app, &p);
    assert_drawn_like(&app, &want, 0..270);
    assert!(done(&app) < 3 * PREFIX_LIMIT, "{}", done(&app));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn jumping_to_the_end_is_coloured_like_a_full_pass() {
    let (dir, p) = big_file("end", "big.rs");
    let mut app = new_app();
    app.open(&p);
    pump(&mut app);
    let want = reference(&app, &p);
    let n = want.len();
    key(&mut app, "ctrl+end");
    // The frame right after the key is drawn without waiting for colours.
    frame(&mut app);
    assert_eq!(drawn(&app, n - 5), None, "the jump waited for colouring");
    pump(&mut app);
    assert_eq!(done(&app), n);
    // Comment lines (state from line 11), the line closing it, code after.
    assert_drawn_like(&app, &want, n - 20..n);
    assert_eq!(want[n - 15].len(), 1, "still inside the comment");
    assert!(want[n - 5].len() > 3, "code again");

    // The same when the frame comes before the first slice of work.
    let mut other = new_app();
    other.open(&p);
    pump_sized(&mut other, (800, 600), false);
    assert!(done(&other) < PREFIX_LIMIT);
    key(&mut other, "ctrl+end");
    pump_sized(&mut other, (800, 600), false);
    assert_drawn_like(&other, &want, n - 20..n);
    assert_eq!(frame(&mut other), frame(&mut app));
    std::fs::remove_dir_all(dir).ok();
}

/// A script (or `--screenshot`) has no frame between a key and `settle`:
/// the jump itself must be enough to know how far to colour.
#[test]
fn settle_after_a_jump_needs_no_frame_in_between() {
    let (dir, p) = big_file("script", "big.rs");
    let mut windowed = new_app();
    windowed.open(&p);
    pump(&mut windowed);
    key(&mut windowed, "ctrl+end");
    pump(&mut windowed);
    let want = frame(&mut windowed);

    let mut scripted = new_app();
    scripted.open(&p);
    key(&mut scripted, "ctrl+end");
    scripted.settle(Duration::from_secs(60));
    assert_eq!(frame(&mut scripted), want);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn go_to_line_find_wheel_and_wrap_reach_coloured_text() {
    let (dir, p) = big_file("jumps", "big.rs");
    let mut app = new_app();
    app.open(&p);
    pump(&mut app);
    let want = reference(&app, &p);
    let n = want.len();

    // Go to line: the target sits a third of the way down.
    key(&mut app, "ctrl+g");
    app.handle(Input::Text("3000".into()));
    key(&mut app, "enter");
    pump(&mut app);
    assert_drawn_like(&app, &want, 2995..3010);
    let after_goto = done(&app);
    assert!(
        (3010..3000 + PREFIX_LIMIT).contains(&after_goto),
        "{after_goto}"
    );

    // The wheel: 60 lines at a time, well past what is coloured.
    let y0 = app.view_scroll_y();
    let wheel = |app: &mut App, notches: f32| {
        app.handle(Input::MouseMove { x: 400.0, y: 300.0 });
        app.handle(Input::Wheel {
            dx: 0.0,
            dy: notches,
            unit: WheelUnit::Lines,
        });
    };
    wheel(&mut app, 1.0);
    let line_h = (app.view_scroll_y() - y0) / 3.0;
    assert!(line_h > 4.0);
    for _ in 0..40 {
        wheel(&mut app, 20.0);
        pump(&mut app);
    }
    let top = (app.view_scroll_y() / line_h) as usize;
    assert!(top > 5000, "scrolled to line {top}");
    assert_drawn_like(&app, &want, top + 1..top + 15);
    assert!(done(&app) < top + PREFIX_LIMIT);

    // Find: the only match is near the end.
    key(&mut app, "ctrl+f");
    app.handle(Input::Text("closed here".into()));
    pump(&mut app);
    let close = n - 13;
    // (The match is revealed at the bottom edge of the view.)
    assert_drawn_like(&app, &want, close - 10..close + 1);
    key(&mut app, "escape");

    // Soft wrap: every line takes two rows here, so rows are not lines.
    let mut app = new_app();
    app.open(&p);
    key(&mut app, "alt+z");
    pump(&mut app);
    assert!(app.status_text().contains("Wrap"));
    assert!(done(&app) < PREFIX_LIMIT, "{}", done(&app));
    assert_drawn_like(&app, &want, 0..8);
    assert_eq!(
        drawn(&app, 20),
        None,
        "line 21 is on screen: nothing wrapped"
    );
    key(&mut app, "ctrl+end");
    pump(&mut app);
    assert_drawn_like(&app, &want, n - 8..n);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn editing_near_the_top_recolours_what_is_shown() {
    let (dir, p) = big_file("edit", "big.rs");
    let mut app = new_app();
    app.open(&p);
    key(&mut app, "ctrl+end");
    pump(&mut app);
    let original = reference(&app, &p);
    assert_eq!(done(&app), original.len());

    // Open a comment on line 3: lines 3 to 10 change colour, and so would
    // everything below if the colouring didn't restart from there.
    key(&mut app, "ctrl+g");
    app.handle(Input::Text("3".into()));
    key(&mut app, "enter");
    pump(&mut app);
    type_text(&mut app, "/* ");
    pump(&mut app);
    let edited = reference(&app, &p);
    assert_ne!(edited[5], original[5]);
    assert_drawn_like(&app, &edited, 0..20);
    assert!(
        done(&app) < PREFIX_LIMIT,
        "an edit at the top re-coloured {} lines",
        done(&app)
    );
    // The end follows when it is looked at again.
    key(&mut app, "ctrl+end");
    pump(&mut app);
    let n = edited.len();
    assert_drawn_like(&app, &edited, n - 20..n);

    // Undo from down here: the top is right again when shown.
    for _ in 0..3 {
        key(&mut app, "ctrl+z");
    }
    pump(&mut app);
    assert!(!app.title().contains('●'), "undo restored the saved text");
    key(&mut app, "ctrl+home");
    pump(&mut app);
    assert_drawn_like(&app, &original, 0..20);
    key(&mut app, "ctrl+end");
    pump(&mut app);
    assert_drawn_like(&app, &original, n - 20..n);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn theme_switch_at_the_end_recolours_from_the_top() {
    let (dir, p) = big_file("theme", "big.rs");
    let mut app = new_app();
    app.open(&p);
    key(&mut app, "ctrl+end");
    pump(&mut app);
    let dark = reference(&app, &p);
    let n = dark.len();
    app.set_light(true);
    pump(&mut app);
    let light = reference(&app, &p);
    assert_ne!(light[n - 5], dark[n - 5]);
    assert_drawn_like(&app, &light, n - 20..n);
    app.set_light(false);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn plain_text_and_markdown_source_go_idle() {
    let dir = tmp_dir("bigfile-idle");
    // Plain text has nothing to colour: no work may stay pending.
    let txt = dir.join("big.txt");
    std::fs::write(&txt, stateful_source(BIG)).unwrap();
    let mut app = new_app();
    app.open(&txt);
    assert!(pump(&mut app) < 10);
    key(&mut app, "ctrl+end");
    assert!(pump(&mut app) < 10);
    assert_eq!(done(&app), 0);

    // Markdown: nothing is coloured while the preview shows; the source
    // view colours what it shows.
    let md = dir.join("big.md");
    let text: String = (0..BIG)
        .map(|i| format!("Line {i} with `code` and **bold** text.\n"))
        .collect();
    std::fs::write(&md, text).unwrap();
    app.open(&md);
    pump(&mut app);
    assert_eq!(done(&app), 0, "the preview needs no source colours");
    key(&mut app, "ctrl+e");
    pump(&mut app);
    let want = reference(&app, &md);
    assert!((20..PREFIX_LIMIT).contains(&done(&app)), "{}", done(&app));
    assert_drawn_like(&app, &want, 0..10);
    assert!(!want[3].is_empty());
    key(&mut app, "ctrl+end");
    pump(&mut app);
    assert_drawn_like(&app, &want, BIG - 10..BIG);
    std::fs::remove_dir_all(dir).ok();
}

/// Two big files that differ on a few lines: near the top, in the middle
/// and near the end.
fn compare_files(tag: &str, ext: &str) -> (PathBuf, PathBuf, PathBuf) {
    let dir = tmp_dir(&format!("bigfile-{tag}"));
    let old = stateful_source(BIG);
    let new: String = old
        .lines()
        .enumerate()
        .map(|(i, l)| {
            if [4, BIG / 2, BIG - 4].contains(&i) {
                format!("fn changed{i}() -> &'static str {{ \"new\" }}\n")
            } else {
                format!("{l}\n")
            }
        })
        .collect();
    let (a, b) = (
        dir.join(format!("old.{ext}")),
        dir.join(format!("new.{ext}")),
    );
    std::fs::write(&a, old).unwrap();
    std::fs::write(&b, new).unwrap();
    (dir, a, b)
}

#[test]
fn compare_view_colours_only_what_is_shown_and_follows_jumps() {
    let (dir, a, b) = compare_files("cmp", "rs");
    let new_lines = crate::diff::lines(&std::fs::read_to_string(&b).unwrap());
    let want = reference_for(&new_lines, &b);
    let n = new_lines.len();
    let mut app = new_app();
    app.compare_files(&a, &b);
    pump(&mut app);
    assert!(
        app.status_text().contains("3 changes"),
        "{}",
        app.status_text()
    );
    assert!(done(&app) < PREFIX_LIMIT, "{}", done(&app));
    assert_drawn_like(&app, &want, 0..15);

    // The end: both sides coloured with the state carried from the top.
    key(&mut app, "ctrl+end");
    assert!(app.view_scroll_y() > 10_000.0);
    pump(&mut app);
    assert_eq!(done(&app), n);
    assert_drawn_like(&app, &want, n - 10..n);
    // One notch up shows the end of the comment that began on line 11.
    app.handle(Input::MouseMove { x: 400.0, y: 300.0 });
    app.handle(Input::Wheel {
        dx: 0.0,
        dy: -1.0,
        unit: WheelUnit::Lines,
    });
    pump(&mut app);
    assert_drawn_like(&app, &want, n - 14..n - 4);
    assert_eq!(want[n - 14].len(), 1, "inside the comment");
    assert!(want[n - 5].len() > 3, "code after it");
    // What is on screen is the finished picture: drawing everything again
    // from nothing gives the same frame.
    let shown = frame(&mut app);
    app.handle(Input::Scale(2.0));
    pump(&mut app);
    app.handle(Input::Scale(1.0));
    pump(&mut app);
    assert_eq!(frame(&mut app), shown);

    // F7 walks the changes: the first one, then the one in the middle.
    key(&mut app, "ctrl+home");
    key(&mut app, "f7");
    pump(&mut app);
    assert!(app.status_text().contains("Change 1 of 3"));
    assert_drawn_like(&app, &want, 0..10);
    key(&mut app, "f7");
    pump(&mut app);
    assert!(app.status_text().contains("Change 2 of 3"));
    assert_drawn_like(&app, &want, BIG / 2 - 3..BIG / 2 + 3);
    std::fs::remove_dir_all(dir).ok();
}

/// Sides with nothing to colour (plain text) or nothing more to colour (a
/// short file next to a long one) must not keep the app busy.
#[test]
fn compare_view_goes_idle_for_plain_and_short_sides() {
    let (dir, a, b) = compare_files("cmp-plain", "txt");
    let mut app = new_app();
    app.compare_files(&a, &b);
    assert!(pump(&mut app) < 10);
    key(&mut app, "ctrl+end");
    assert!(pump(&mut app) < 10);
    assert_eq!(done(&app), 0);

    let short = dir.join("short.rs");
    std::fs::write(&short, "fn only() {}\n").unwrap();
    let long = dir.join("long.rs");
    std::fs::write(&long, stateful_source(BIG)).unwrap();
    let want = reference_for(
        &crate::diff::lines(&std::fs::read_to_string(&long).unwrap()),
        &long,
    );
    app.compare_files(&short, &long);
    pump(&mut app);
    assert!(done(&app) < PREFIX_LIMIT);
    key(&mut app, "ctrl+end");
    pump(&mut app);
    assert_drawn_like(&app, &want, BIG - 10..BIG);
    // And the other way round: the long side on the left.
    app.compare_files(&long, &short);
    pump(&mut app);
    key(&mut app, "ctrl+end");
    pump(&mut app);
    assert!(!app.has_pending_work());
    std::fs::remove_dir_all(dir).ok();
}

// ---- hidden tabs ---------------------------------------------------------

/// A long stretch of colours is rebuilt on demand rather than kept for a tab
/// nobody is looking at; a short one is kept (no flash when the tab returns).
#[test]
fn hidden_tabs_free_long_colours_and_get_them_back() {
    let (dir, p) = big_file("hide", "big.rs");
    let small = dir.join("small.rs");
    std::fs::write(&small, stateful_source(100)).unwrap();
    let other = dir.join("other.txt");
    std::fs::write(&other, "x\n").unwrap();
    let mut app = new_app();
    app.open(&p);
    pump(&mut app);
    let want = reference(&app, &p);
    key(&mut app, "ctrl+end");
    pump(&mut app);
    assert_eq!(done(&app), BIG + 1);

    app.open(&other); // the big file is hidden now
    frame(&mut app); // (hidden tabs are trimmed as a frame starts)
    key(&mut app, "ctrl+shift+tab");
    assert!(app.title().contains("big.rs"), "{}", app.title());
    assert_eq!(done(&app), 0, "colours of a hidden tab were kept");
    pump(&mut app);
    assert_eq!(done(&app), BIG + 1, "the end is where the view is");
    assert_drawn_like(&app, &want, BIG - 10..BIG);
    assert!(!app.has_pending_work());

    // A short file keeps its colours.
    app.open(&small);
    pump(&mut app);
    assert_eq!(done(&app), 101);
    app.open(&other);
    frame(&mut app);
    key(&mut app, "ctrl+shift+tab");
    assert!(app.title().contains("small.rs"), "{}", app.title());
    assert_eq!(done(&app), 101);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn a_hidden_compare_frees_long_colours_and_gets_them_back() {
    let (dir, a, b) = compare_files("cmp-hide", "rs");
    let new_lines = crate::diff::lines(&std::fs::read_to_string(&b).unwrap());
    let want = reference_for(&new_lines, &b);
    let other = dir.join("other.txt");
    std::fs::write(&other, "x\n").unwrap();
    let mut app = new_app();
    app.open(&other);
    app.compare_files(&a, &b);
    key(&mut app, "ctrl+end");
    pump(&mut app);
    assert_eq!(done(&app), BIG);
    key(&mut app, "ctrl+shift+tab");
    frame(&mut app); // the compare is hidden now
    key(&mut app, "ctrl+tab");
    assert!(app.title().contains('↔'), "{:?}", app.tab_titles());
    assert_eq!(done(&app), 0, "colours of a hidden compare were kept");
    pump(&mut app);
    assert_drawn_like(&app, &want, BIG - 10..BIG);
    std::fs::remove_dir_all(dir).ok();
}

/// Both sides of a split colour what they show, and then the app is idle.
#[test]
fn both_sides_of_a_split_are_coloured() {
    let (dir, p) = big_file("split", "big.rs");
    let q = dir.join("big2.rs");
    std::fs::write(&q, stateful_source(BIG)).unwrap();
    let mut app = new_app();
    app.open(&p);
    app.open(&q);
    key(&mut app, "ctrl+alt+right");
    key(&mut app, "ctrl+end");
    key(&mut app, "ctrl+1");
    pump(&mut app);
    assert!(!app.has_pending_work());
    let left = done(&app);
    key(&mut app, "ctrl+2");
    // (No further work: the unfocused side was coloured while it waited.)
    let right = done(&app);
    assert!((1..PREFIX_LIMIT).contains(&left), "left {left}");
    assert_eq!(right, BIG + 1, "right side was at the end");
    pump(&mut app);
    assert!(!app.has_pending_work());
    std::fs::remove_dir_all(dir).ok();
}

// ---- read-only lock ------------------------------------------------------

use crate::tests::{mouse, press, run_command, temp_file};

/// CRLF for most lines, one LF; 4-space indent; trailing spaces; "foo" twice.
const RO_TEXT: &str = "fn a() {\r\n    let x = 1;  \r\n}\r\nfoo bar foo\nlast";
const TOGGLE: &str = "View: Toggle Read-Only";

fn locked_app(name: &str, text: &str) -> (App, PathBuf) {
    let p = temp_file(name, text);
    let mut app = new_app();
    app.open(&p);
    // Something on the clipboard, so that paste has something to insert.
    press(&mut app, "ctrl+a");
    press(&mut app, "ctrl+c");
    press(&mut app, "ctrl+home");
    run_command(&mut app, TOGGLE);
    assert!(
        app.status_text().contains("Read-only"),
        "{}",
        app.status_text()
    );
    (app, p)
}

fn clean(app: &App) -> bool {
    !app.title().starts_with('●')
}

#[test]
fn no_editing_path_changes_a_locked_text() {
    let keys = [
        "enter",
        "tab",
        "shift+tab",
        "backspace",
        "delete",
        "ctrl+backspace",
        "ctrl+delete",
        "ctrl+x",
        "ctrl+v",
        "shift+insert",
        "shift+delete",
        "ctrl+z",
        "ctrl+y",
        "ctrl+shift+z",
        "ctrl+j",
        "ctrl+enter",
        "ctrl+shift+enter",
        "alt+up",
        "alt+down",
        "ctrl+/",
        "ctrl+shift+d",
        "ctrl+shift+k",
        "ctrl+]",
        "ctrl+[",
        "ctrl+h",
    ];
    let commands = [
        "Edit: Upper Case",
        "Edit: Lower Case",
        "Edit: Trim Trailing Whitespace",
        "Indentation: Convert to Spaces",
        "Indentation: Convert to Tabs",
        "Line Endings: CRLF (Windows)",
        "Line Endings: LF (Unix)",
        "Edit: Join Lines",
        "Edit: Duplicate Line",
        "Edit: Delete Line",
        "Edit: Move Line Down",
        "Edit: Toggle Comment",
    ];
    // Carets: one at the start, everything selected, three carets, a word
    // selected, a selection that is a block of lines.
    let setups: [&[&str]; 5] = [
        &[],
        &["ctrl+a"],
        &["ctrl+shift+down", "ctrl+shift+down"],
        &["ctrl+end"],
        &["down", "down", "down", "ctrl+d", "ctrl+shift+l"],
    ];
    for (n, setup) in setups.iter().enumerate() {
        let (mut app, p) = locked_app(&format!("ro-table{n}.rs"), RO_TEXT);
        for k in *setup {
            press(&mut app, k);
        }
        let carets = app.snapshot().unwrap().1;
        let same = |app: &mut App, what: &str| {
            assert_eq!(
                app.view_text(),
                RO_TEXT,
                "setup {n}: {what} changed the text"
            );
            assert!(clean(app), "setup {n}: {what} made the tab dirty");
            // (A frame and the status bar must cope with whatever happened.)
            render_hash(app, 800, 600);
            let _ = app.status_text();
            assert_eq!(
                app.snapshot().unwrap().1,
                carets,
                "setup {n}: {what} moved the carets"
            );
        };
        for c in ["x", "é", "\n", "foo\r\nbar", "\t"] {
            app.handle(Input::Text(c.into()));
            same(&mut app, &format!("typing {c:?}"));
        }
        assert!(
            app.status_text().contains("Locked"),
            "{}",
            app.status_text()
        );
        for k in keys {
            press(&mut app, k);
            same(&mut app, k);
        }
        for c in commands {
            run_command(&mut app, c);
            same(&mut app, c);
            assert!(
                !app.status_text().contains("works in a text file"),
                "{c}: {}",
                app.status_text()
            );
        }
        // Find / replace: even with a replacement typed, nothing is replaced.
        press(&mut app, "ctrl+f");
        type_text(&mut app, "foo");
        press(&mut app, "ctrl+h");
        press(&mut app, "tab");
        type_text(&mut app, "BAR");
        for k in ["ctrl+enter", "enter", "shift+enter", "ctrl+enter"] {
            press(&mut app, k);
            // (Finding moves the selection, which is fine.)
            assert_eq!(app.view_text(), RO_TEXT, "setup {n}: {k} replaced text");
            assert!(clean(&app));
        }
        press(&mut app, "escape");
        // Nothing was recorded or left half-changed: unlocking gives back a
        // fresh document (undo has nothing; settings are as they were).
        run_command(&mut app, TOGGLE);
        assert!(!app.status_text().contains("Read-only"));
        press(&mut app, "ctrl+z");
        assert_eq!(app.view_text(), RO_TEXT, "undo after unlocking");
        assert!(clean(&app));
        press(&mut app, "ctrl+home");
        press(&mut app, "tab");
        assert!(
            app.view_text().starts_with("    fn a()"),
            "indent setting changed: {:?}",
            app.view_text()
        );
        press(&mut app, "ctrl+z");
        press(&mut app, "ctrl+home");
        press(&mut app, "enter");
        assert!(
            app.view_text().starts_with("\r\nfn a()"),
            "line ending setting changed: {:?}",
            app.view_text()
        );
        std::fs::remove_dir_all(p.parent().unwrap()).ok();
    }
}

/// Undo and redo are edits: with history behind or ahead, they are refused.
#[test]
fn undo_and_redo_are_locked_too() {
    let p = temp_file("ro-undo.txt", "one\ntwo\n");
    let mut app = new_app();
    app.open(&p);
    type_text(&mut app, "A");
    press(&mut app, "enter");
    type_text(&mut app, "B");
    press(&mut app, "ctrl+z");
    let undone = app.view_text();
    run_command(&mut app, TOGGLE);
    press(&mut app, "ctrl+y");
    assert_eq!(app.view_text(), undone, "redo went through the lock");
    run_command(&mut app, TOGGLE);
    press(&mut app, "ctrl+y");
    let redone = app.view_text();
    assert_ne!(redone, undone);
    run_command(&mut app, TOGGLE);
    press(&mut app, "ctrl+z");
    press(&mut app, "ctrl+shift+z");
    assert_eq!(app.view_text(), redone, "undo went through the lock");
    run_command(&mut app, TOGGLE);
    press(&mut app, "ctrl+z");
    assert_eq!(app.view_text(), undone, "undo is back after unlocking");
    std::fs::remove_dir_all(p.parent().unwrap()).ok();
}

#[test]
fn locking_closes_the_replace_row_and_unlocking_edits_again() {
    let p = temp_file("ro-replace.txt", "foo foo\n");
    let mut app = new_app();
    app.open(&p);
    press(&mut app, "ctrl+h");
    type_text(&mut app, "foo");
    press(&mut app, "tab");
    type_text(&mut app, "BAR");
    run_command(&mut app, TOGGLE);
    press(&mut app, "ctrl+enter");
    assert_eq!(app.view_text(), "foo foo\n");
    assert!(clean(&app));
    // Unlocked again, replace all works.
    run_command(&mut app, TOGGLE);
    press(&mut app, "ctrl+h");
    press(&mut app, "ctrl+a");
    type_text(&mut app, "foo");
    press(&mut app, "tab");
    type_text(&mut app, "BAR");
    press(&mut app, "ctrl+enter");
    assert_eq!(app.view_text(), "BAR BAR\n");
    std::fs::remove_dir_all(p.parent().unwrap()).ok();
}

#[test]
fn lock_status_notice_and_saving() {
    let (mut app, p) = locked_app("ro-save.txt", "one\ntwo\n");
    type_text(&mut app, "zzz");
    assert!(app.status_text().contains("Read-only"));
    assert!(app.status_text().contains("Locked: nothing was changed"));
    // The notice goes away with the next key.
    press(&mut app, "down");
    assert!(
        !app.status_text().contains("Locked:"),
        "{}",
        app.status_text()
    );
    assert!(app.status_text().contains("Read-only"));

    // Saving a locked, unchanged file does nothing (not even a nag when the
    // file was changed behind our back).
    std::fs::write(&p, "changed elsewhere\n").unwrap();
    press(&mut app, "ctrl+s");
    assert!(
        app.status_text().contains("nothing to save"),
        "{}",
        app.status_text()
    );
    assert!(app.nag_message().is_none());
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "changed elsewhere\n");
    std::fs::remove_dir_all(p.parent().unwrap()).ok();

    // Edits made before locking are still saved, with the lock on.
    let (mut app, p) = {
        let p = temp_file("ro-save2.txt", "one\ntwo\n");
        let mut app = new_app();
        app.open(&p);
        type_text(&mut app, "A");
        run_command(&mut app, TOGGLE);
        (app, p)
    };
    assert!(!clean(&app));
    press(&mut app, "ctrl+s");
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "Aone\ntwo\n");
    assert!(clean(&app));
    // And closing a locked, unchanged tab asks nothing.
    press(&mut app, "ctrl+w");
    assert!(app.nag_message().is_none());
    std::fs::remove_dir_all(p.parent().unwrap()).ok();
}

#[test]
fn a_locked_markdown_keeps_its_checkboxes_and_source() {
    let text = "# T\n\n- [ ] first\n- [x] second\n";
    let p = temp_file("ro-tasks.md", text);
    let mut app = new_app();
    app.open(&p);
    key(&mut app, "ctrl+\\");
    render_hash(&mut app, 800, 600);
    run_command(&mut app, TOGGLE);
    render_hash(&mut app, 800, 600);
    assert!(
        app.status_text().contains("Read-only"),
        "{}",
        app.status_text()
    );
    for i in 0..2 {
        let (x, y) = app.preview_checkboxes()[i];
        mouse(&mut app, x, y, crate::input::MouseButton::Left);
        render_hash(&mut app, 800, 600);
        assert_eq!(app.view_text(), text);
    }
    assert!(clean(&app));
    assert!(
        app.status_text().contains("Locked"),
        "{}",
        app.status_text()
    );
    press(&mut app, "ctrl+z");
    press(&mut app, "ctrl+y");
    assert_eq!(app.view_text(), text);
    // Saving a locked, unchanged markdown does nothing either.
    std::fs::write(&p, "changed elsewhere\n").unwrap();
    press(&mut app, "ctrl+s");
    assert!(
        app.status_text().contains("nothing to save"),
        "{}",
        app.status_text()
    );
    assert!(app.nag_message().is_none());
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "changed elsewhere\n");
    std::fs::write(&p, text).unwrap();
    app.handle(Input::FsChanged(vec![p.clone()]));
    assert_eq!(app.view_text(), text);
    // The source view is locked too, and so is the side-by-side one.
    press(&mut app, "ctrl+e");
    type_text(&mut app, "zzz");
    press(&mut app, "enter");
    assert_eq!(app.view_text(), text);
    press(&mut app, "ctrl+e");
    press(&mut app, "ctrl+shift+v");
    type_text(&mut app, "zzz");
    assert_eq!(app.view_text(), text);
    press(&mut app, "ctrl+shift+v");
    press(&mut app, "ctrl+e");
    // Unlocked, a checkbox ticks again.
    run_command(&mut app, TOGGLE);
    render_hash(&mut app, 800, 600);
    let (x, y) = app.preview_checkboxes()[0];
    mouse(&mut app, x, y, crate::input::MouseButton::Left);
    assert!(
        app.view_text().contains("- [x] first"),
        "{}",
        app.view_text()
    );
    std::fs::remove_dir_all(p.parent().unwrap()).ok();
}

#[test]
fn an_external_reload_still_updates_a_locked_tab() {
    let (mut app, p) = locked_app("ro-reload.txt", "one\n");
    std::fs::write(&p, "two\n").unwrap();
    app.handle(Input::FsChanged(vec![p.clone()]));
    assert_eq!(app.view_text(), "two\n");
    std::fs::remove_dir_all(p.parent().unwrap()).ok();
}

/// Saving edits made before locking does not trim (an edit) and does not
/// claim the lock stopped something.
#[test]
fn saving_a_locked_dirty_file_does_not_trim() {
    let d = tmp_dir("ro-trim");
    let set = d.join("settings.txt");
    std::fs::write(&set, "trim_trailing_whitespace = on\n").unwrap();
    let f = d.join("t.py");
    std::fs::write(&f, "x = 1   \n").unwrap();
    let mut app = new_app();
    app.use_settings(set);
    app.open(&f);
    key(&mut app, "ctrl+end");
    type_text(&mut app, "z");
    run_command(&mut app, TOGGLE);
    key(&mut app, "ctrl+s");
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "x = 1   \nz");
    assert!(app.status_text().contains("Saved"), "{}", app.status_text());
    assert!(
        !app.status_text().contains("Locked:"),
        "{}",
        app.status_text()
    );
    std::fs::remove_dir_all(d).ok();
}

/// Replace works on text nobody has scrolled to yet, and the result is
/// coloured like a fresh pass over the new text.
#[test]
fn replace_all_reaches_uncoloured_regions() {
    let (dir, p) = big_file("replace", "big.rs");
    let mut app = new_app();
    app.open(&p);
    pump(&mut app);
    assert!(done(&app) < PREFIX_LIMIT);
    key(&mut app, "ctrl+h");
    type_text(&mut app, "closed here");
    key(&mut app, "tab");
    type_text(&mut app, "closed there");
    key(&mut app, "ctrl+enter");
    key(&mut app, "escape");
    pump(&mut app);
    assert!(
        app.view_text().contains("closed there */ fn after()"),
        "the match near the end was replaced"
    );
    let want = reference(&app, &p);
    key(&mut app, "ctrl+end");
    pump(&mut app);
    let n = want.len();
    assert_drawn_like(&app, &want, n - 10..n);
    // Find still sees an uncoloured region, too.
    key(&mut app, "ctrl+home");
    pump(&mut app);
    key(&mut app, "ctrl+f");
    type_text(&mut app, "closed there");
    pump(&mut app);
    assert!(
        app.status_text().starts_with("Ln 11989,"),
        "{}",
        app.status_text()
    );
    std::fs::remove_dir_all(dir).ok();
}

/// A refused command on a locked big file does not throw its colours away.
#[test]
fn a_refused_command_keeps_the_colours() {
    let (dir, p) = big_file("refused", "big.rs");
    let mut app = new_app();
    app.open(&p);
    key(&mut app, "ctrl+end");
    pump(&mut app);
    run_command(&mut app, TOGGLE);
    let before = done(&app);
    assert_eq!(before, BIG + 1);
    run_command(&mut app, "Edit: Upper Case");
    run_command(&mut app, "Line Endings: CRLF (Windows)");
    assert_eq!(done(&app), before);
    std::fs::remove_dir_all(dir).ok();
}
