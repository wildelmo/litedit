//! Epic 9: Close Folder and the start page, the tree header, the menu bar.

use crate::app::App;
use crate::input::{Input, MouseButton, Request};
use crate::session::Session;
use crate::settings::Settings;
use crate::tests::{
    key, mouse, new_app, render_hash, run_command, run_menu_item, settings_dir, settled_hash,
    temp_file, tmp_dir, two_projects, type_text,
};

fn titles(app: &App) -> Vec<String> {
    app.tab_titles().into_iter().map(|t| t.0).collect()
}

fn bar_app() -> App {
    let mut app = new_app();
    app.set_menu_bar(true);
    render_hash(&mut app, 800, 600);
    app
}

fn click_title(app: &mut App, i: usize) {
    render_hash(app, 800, 600);
    let (x, y) = app.bar_title_pos(i).expect("menu bar shown");
    app.handle(Input::MouseMove { x, y });
    mouse(app, x, y, MouseButton::Left);
}

// ---- S9.1 Close Folder and the start page ---------------------------------

#[test]
fn close_folder_keeps_unsaved_tabs_remembers_the_rest_and_starts_empty() {
    let (a, _b, store) = two_projects("closefolder");
    let mut app = new_app();
    app.use_workspace_store(store);
    app.open(&a);
    app.open(&a.join("a1.md"));
    app.open(&a.join("src/a2.rs"));
    type_text(&mut app, "x");
    assert!(
        app.tab_titles()
            .iter()
            .any(|(t, dirty)| t.contains("a2.rs") && *dirty)
    );

    run_command(&mut app, "File: Close Folder");
    assert!(app.tree_roots().is_empty(), "no folder in the tree");
    assert_eq!(
        titles(&app).len(),
        1,
        "only the edited tab stays: {:?}",
        titles(&app)
    );
    assert!(titles(&app)[0].contains("a2.rs"));
    assert!(
        app.status_text().contains("Kept 1 tab"),
        "{}",
        app.status_text()
    );
    assert!(
        app.snapshot().is_some_and(|(t, _)| t.starts_with('x')),
        "edit kept"
    );
    // The next launch starts without a folder.
    assert_eq!(app.session().root, None);
    assert!(app.session().folders.is_empty());

    // Opening the folder again brings its tabs back (they were remembered
    // before anything closed).
    app.open(&a);
    let names = titles(&app);
    assert!(names.iter().any(|n| n.contains("a1.md")), "{names:?}");
    assert!(names.iter().any(|n| n.contains("a2.rs")), "{names:?}");
    // Closing again with nothing open: just a note.
    let mut empty = new_app();
    run_command(&mut empty, "File: Close Folder");
    assert!(empty.status_text().contains("No folder is open"));
}

#[test]
fn the_start_page_opens_things_and_lists_recent_folders() {
    let (a, _b, _store) = two_projects("startpage");
    let mut app = new_app();
    app.open(&a.join("a1.md"));
    app.open(&a); // recent: [a, a1.md]
    run_command(&mut app, "File: Close Folder");
    assert!(app.tree_roots().is_empty());
    assert!(
        titles(&app).iter().all(|t| !t.contains("a1.md")),
        "clean tab closed"
    );
    render_hash(&mut app, 800, 600);

    // Arrow keys walk the entries: Open File, Open Folder, Open Workspace,
    // then the recent ones.
    app.take_requests();
    for k in ["down", "down", "enter"] {
        key(&mut app, k);
    }
    assert!(app.take_requests().contains(&Request::OpenFolderDialog));
    key(&mut app, "down");
    key(&mut app, "enter");
    assert!(app.take_requests().contains(&Request::OpenWorkspaceDialog));
    // The first recent entry is the folder that was just closed.
    key(&mut app, "down");
    key(&mut app, "enter");
    assert_eq!(
        app.tree_roots(),
        vec![a.clone()],
        "reopened from the start page"
    );

    // A recent entry that no longer exists is not listed: with the folder
    // gone, the first recent entry is the file.
    run_command(&mut app, "File: Close Folder");
    let gone = tmp_dir("startpage-gone");
    app.open(&gone);
    run_command(&mut app, "File: Close Folder");
    std::fs::remove_dir_all(&gone).unwrap();
    // Rebuild the page from the current recents (as a fresh start would).
    app.restore(Session::parse(&app.session().to_text()));
    render_hash(&mut app, 800, 600);
    for _ in 0..4 {
        key(&mut app, "down");
    }
    key(&mut app, "enter");
    assert_eq!(app.tree_roots(), vec![a], "the missing folder was skipped");
}

#[test]
fn the_tree_root_menu_offers_close_folder_and_remove_for_added_folders() {
    let (a, b, _store) = two_projects("rootmenu");
    let mut app = new_app();
    app.open(&a);
    render_hash(&mut app, 800, 600);
    // One folder: Close Folder, nothing to remove it from.
    let ya = app.tree_row_y(&a).unwrap();
    mouse(&mut app, 60.0, ya, MouseButton::Right);
    let labels = app.menu_labels();
    assert!(labels.contains(&"Close Folder".to_string()), "{labels:?}");
    assert!(
        !labels.iter().any(|l| l.starts_with("Remove Folder")),
        "{labels:?}"
    );
    key(&mut app, "escape");
    // Two folders: the main one can still be removed (as before), or the
    // whole thing closed.
    app.add_folder(&b);
    render_hash(&mut app, 800, 600);
    mouse(&mut app, 60.0, ya, MouseButton::Right);
    let labels = app.menu_labels();
    assert!(labels.contains(&"Close Folder".to_string()), "{labels:?}");
    assert!(
        labels.contains(&"Remove Folder from Workspace".to_string()),
        "{labels:?}"
    );
    key(&mut app, "escape");
    let yb = app.tree_row_y(&b).unwrap();
    mouse(&mut app, 60.0, yb, MouseButton::Right);
    let labels = app.menu_labels();
    assert!(
        labels.contains(&"Remove Folder from Workspace".to_string()),
        "{labels:?}"
    );
    assert!(!labels.contains(&"Close Folder".to_string()), "{labels:?}");
}

// ---- S9.3 Tree header ------------------------------------------------------

#[test]
fn the_tree_header_says_what_is_open_and_its_menu_works() {
    let (a, b, _store) = two_projects("header");
    let c = tmp_dir("header-c");
    let mut app = new_app();
    app.open(&a);
    assert_eq!(app.tree_header_text(), "FOLDER");
    app.add_folder(&b);
    assert_eq!(app.tree_header_text(), "FOLDERS");
    let ws = a.join("proj.litedit-workspace");
    app.save_workspace_as(&ws);
    assert_eq!(app.tree_header_text(), "WORKSPACE · proj");
    assert_eq!(app.session().workspace, Some(ws.clone()));

    // Remembered across a restart.
    let mut again = new_app();
    again.restore(Session::parse(&app.session().to_text()));
    assert_eq!(again.tree_header_text(), "WORKSPACE · proj");
    // ...but not if the file is gone (it is only a name, never read).
    let mut s = app.session();
    s.workspace = Some(a.join("missing.litedit-workspace"));
    let mut gone = new_app();
    gone.restore(s);
    assert_eq!(gone.tree_header_text(), "FOLDERS");

    // Another folder is a plain folder; the workspace file brings it back.
    app.open(&c);
    assert_eq!(app.tree_header_text(), "FOLDER");
    app.open(&ws);
    assert_eq!(app.tree_header_text(), "WORKSPACE · proj");
    assert_eq!(app.tree_roots(), vec![a.clone(), b.clone()]);

    // A click on the header opens its menu (either button).
    render_hash(&mut app, 800, 600);
    let r = app.tree_header_rect().expect("header shown");
    let (x, y) = (r.x as f32 + 20.0, r.y as f32 + r.h as f32 / 2.0);
    app.handle(Input::MouseMove { x, y });
    mouse(&mut app, x, y, MouseButton::Right);
    let labels = app.menu_labels();
    for l in [
        "Open Folder…",
        "Add Folder to Workspace…",
        "Save Workspace As…",
        "Close Folder",
    ] {
        assert!(labels.contains(&l.to_string()), "{l} in {labels:?}");
    }
    key(&mut app, "escape");
    mouse(&mut app, x, y, MouseButton::Left);
    run_menu_item(&mut app, "Close Folder");
    assert!(app.tree_roots().is_empty());
    assert_eq!(app.tree_header_rect(), None, "no header without a folder");
    assert_eq!(app.session().workspace, None);
}

#[test]
fn the_header_menu_runs_workspace_commands_through_their_dialogs() {
    let (a, _b, _store) = two_projects("headerdlg");
    let mut app = new_app();
    app.open(&a);
    render_hash(&mut app, 800, 600);
    let r = app.tree_header_rect().unwrap();
    let (x, y) = (r.x as f32 + 20.0, r.y as f32 + r.h as f32 / 2.0);
    app.take_requests();
    for (label, want) in [
        ("Open Folder…", Request::OpenFolderDialog),
        ("Open Workspace…", Request::OpenWorkspaceDialog),
        ("Add Folder to Workspace…", Request::AddFolderDialog),
        (
            "Save Workspace As…",
            Request::SaveWorkspaceDialog(a.join("alpha.litedit-workspace")),
        ),
    ] {
        mouse(&mut app, x, y, MouseButton::Left);
        run_menu_item(&mut app, label);
        assert!(app.take_requests().contains(&want), "{label}");
    }
}

// ---- S9.2 Menu bar ---------------------------------------------------------

#[test]
fn menu_bar_titles_open_menus_hover_switches_and_items_run() {
    let (a, _b, _store) = two_projects("bar");
    let mut app = bar_app();
    app.open(&a);
    click_title(&mut app, 0);
    assert_eq!(app.bar_menu_open(), Some(0));
    let labels = app.menu_labels();
    for l in ["Open Folder…", "Close Folder", "Save", "Exit"] {
        assert!(labels.contains(&l.to_string()), "{l} in {labels:?}");
    }
    // Moving to another title switches menus while one is open.
    let (ex, ey) = app.bar_title_pos(1).unwrap();
    app.handle(Input::MouseMove { x: ex, y: ey });
    assert_eq!(app.bar_menu_open(), Some(1));
    assert!(app.menu_labels().contains(&"Undo".to_string()));
    // A click on the open menu's title closes it.
    mouse(&mut app, ex, ey, MouseButton::Left);
    assert_eq!(app.bar_menu_open(), None);
    assert!(app.menu_labels().is_empty());
    // A click outside closes it too.
    click_title(&mut app, 2);
    mouse(&mut app, 700.0, 500.0, MouseButton::Left);
    assert_eq!(app.bar_menu_open(), None);
    // An item runs and the menu closes.
    click_title(&mut app, 0);
    run_menu_item(&mut app, "Close Folder");
    assert!(app.tree_roots().is_empty());
    assert_eq!(app.bar_menu_open(), None);
    // Shortcut items do what their keys do.
    app.take_requests();
    click_title(&mut app, 0);
    run_menu_item(&mut app, "Open File…");
    assert!(app.take_requests().contains(&Request::OpenFileDialog));
}

#[test]
fn menu_bar_from_the_keyboard() {
    let mut app = bar_app();
    key(&mut app, "alt+v");
    assert_eq!(app.bar_menu_open(), Some(2));
    key(&mut app, "left");
    assert_eq!(app.bar_menu_open(), Some(1));
    key(&mut app, "right");
    key(&mut app, "right");
    assert_eq!(app.bar_menu_open(), Some(3));
    key(&mut app, "right");
    key(&mut app, "right");
    assert_eq!(app.bar_menu_open(), Some(0), "wraps around");
    key(&mut app, "escape");
    assert_eq!(app.bar_menu_open(), None);
    // F10: File with its first item (New File…) selected; Down, Enter runs
    // Open File….
    app.take_requests();
    key(&mut app, "f10");
    assert_eq!(app.bar_menu_open(), Some(0));
    render_hash(&mut app, 800, 600);
    key(&mut app, "down");
    key(&mut app, "enter");
    assert_eq!(app.bar_menu_open(), None);
    assert!(app.take_requests().contains(&Request::OpenFileDialog));
    // Alt keys that already do something are not menus.
    for k in ["alt+z", "alt+w", "alt+c", "alt+r", "alt+1"] {
        key(&mut app, k);
        assert_eq!(app.bar_menu_open(), None, "{k}");
    }
    // AltGr (Ctrl+Alt) + E is typing, not the Edit menu.
    key(&mut app, "ctrl+alt+e");
    assert_eq!(app.bar_menu_open(), None);
}

#[test]
fn a_hidden_menu_bar_shows_while_one_of_its_menus_is_open() {
    let mut app = new_app(); // bar off
    render_hash(&mut app, 800, 600);
    let top = app.main_rect().y;
    assert_eq!(app.bar_title_pos(0), None);
    key(&mut app, "alt+f");
    assert_eq!(app.bar_menu_open(), Some(0));
    assert!(app.main_rect().y > top, "the bar shows");
    render_hash(&mut app, 800, 600);
    key(&mut app, "escape");
    assert_eq!(app.main_rect().y, top, "and hides again");
}

#[test]
fn exit_from_the_menu_asks_about_unsaved_edits() {
    let p = temp_file("menu-exit.txt", "hello\n");
    let mut app = bar_app();
    app.open(&p);
    type_text(&mut app, "x");
    click_title(&mut app, 0);
    run_menu_item(&mut app, "Exit");
    assert!(app.nag_message().is_some(), "asks first");
    assert!(!app.take_requests().contains(&Request::Quit));
    // Under a question the bar does nothing.
    click_title(&mut app, 0);
    assert_eq!(app.bar_menu_open(), None);
    key(&mut app, "alt+f");
    assert_eq!(app.bar_menu_open(), None);

    let mut clean = bar_app();
    clean.open(&p);
    click_title(&mut clean, 0);
    run_menu_item(&mut clean, "Exit");
    assert!(clean.take_requests().contains(&Request::Quit));
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "hello\n");
}

fn open_about(app: &mut App) {
    click_title(app, 4);
    run_menu_item(app, "About litedit");
}

fn about_tabs(app: &App) -> usize {
    titles(app).iter().filter(|t| *t == "About").count()
}

#[test]
fn about_is_one_tab_that_gives_way_like_the_start_page() {
    let p = temp_file("menu-about.txt", "hello\n");
    let q = temp_file("menu-about-2.txt", "world\n");
    let mut app = bar_app();
    let start = render_hash(&mut app, 800, 600);
    open_about(&mut app);
    assert_eq!(titles(&app), ["About"], "takes the start page's place");
    assert!(app.nag_message().is_none(), "a tab, not a question");
    assert_ne!(render_hash(&mut app, 800, 600), start);
    // The same tab when asked again.
    open_about(&mut app);
    assert_eq!(titles(&app), ["About"]);
    // Closing it leaves the start page, which still works.
    key(&mut app, "ctrl+w");
    assert_eq!(about_tabs(&app), 0);
    app.take_requests();
    for k in ["down", "down", "enter"] {
        key(&mut app, k);
    }
    assert!(app.take_requests().contains(&Request::OpenFolderDialog));

    // A file opened while About is the only tab takes its place.
    open_about(&mut app);
    app.open(&p);
    assert_eq!(titles(&app), ["menu-about.txt"]);

    // Next to files it is a tab of its own, found again from any tab.
    app.open(&q);
    open_about(&mut app);
    assert_eq!(app.tab_count(), 3);
    let at = app.active_tab();
    assert_eq!(titles(&app)[at], "About");
    key(&mut app, "ctrl+tab");
    assert_ne!(app.active_tab(), at);
    open_about(&mut app);
    assert_eq!((app.tab_count(), app.active_tab()), (3, at));
    assert_eq!(about_tabs(&app), 1);

    // It is not a file: the saved session has the two files and nothing else,
    // and typing or saving in it changes nothing.
    type_text(&mut app, "x");
    key(&mut app, "ctrl+s");
    let text = app.session().to_text();
    assert!(!text.contains("About"), "{text}");
    let mut again = new_app();
    again.restore(Session::parse(&text));
    assert_eq!(titles(&again), ["menu-about.txt", "menu-about-2.txt"]);
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "hello\n");
    assert_eq!(std::fs::read_to_string(&q).unwrap(), "world\n");
}

#[test]
fn about_draws_the_rows_it_was_built_with() {
    use crate::branding::Branding;
    let shot = |b: &Branding| {
        let mut app = new_app();
        app.add_about(b.clone());
        assert_eq!(titles(&app), ["About"]);
        render_hash(&mut app, 800, 600)
    };
    let name = Branding {
        name: "Ada Lovelace".into(),
        ..Branding::default()
    };
    let company = Branding {
        company: "Analytical Engines".into(),
        ..name.clone()
    };
    let email = Branding {
        email: "ada@example.com".into(),
        ..company.clone()
    };
    let other = Branding {
        email: "ada@example.org".into(),
        ..company.clone()
    };
    let hashes = [&Branding::default(), &name, &company, &email, &other].map(shot);
    for i in 0..hashes.len() {
        for j in 0..i {
            assert_ne!(hashes[i], hashes[j], "{i} and {j} look the same");
        }
    }
    assert_eq!(shot(&email), hashes[3], "the same identity draws the same");
}

#[test]
fn about_follows_the_theme_and_renders_at_any_size() {
    crate::theme::set_light(false);
    let mut a = bar_app();
    open_about(&mut a);
    let dark = settled_hash(&mut a);
    a.set_light(true);
    let switched = settled_hash(&mut a);
    let mut b = bar_app();
    open_about(&mut b);
    let fresh = settled_hash(&mut b);
    assert_ne!(dark, switched);
    assert_eq!(switched, fresh, "switched to light vs light from the start");
    a.set_light(false);
    assert_eq!(settled_hash(&mut a), dark, "back to dark");
    crate::theme::set_light(false);

    for (w, h) in [(1, 1), (40, 40), (300, 90), (200, 600), (800, 600)] {
        a.handle(Input::Resize(w, h));
        render_hash(&mut a, w, h);
    }
}

#[test]
fn hidden_files_show_by_default_and_the_toggle_is_remembered() {
    assert!(Settings::default().hidden_files);
    assert!(!Settings::parse("hidden_files = off").0.hidden_files);
    let (a, _b, _store) = two_projects("hiddenfiles");
    std::fs::create_dir_all(a.join(".docs")).unwrap();
    std::fs::write(a.join(".docs/zzplan.md"), "# plan").unwrap();
    std::fs::write(a.join(".env"), "x").unwrap();
    std::fs::create_dir_all(a.join(".git")).unwrap();
    let path = settings_dir("hiddenfiles").join("settings.txt");
    let shown = |app: &mut App, name: &str| {
        render_hash(app, 800, 600);
        app.tree_row_y(&a.join(name)).is_some()
    };
    // Ctrl+P, the file's name, Enter: is it found and opened?
    let quick_open = |app: &mut App| {
        key(app, "ctrl+p");
        type_text(app, "zzplan");
        key(app, "enter");
        key(app, "escape");
        let found = titles(app).iter().any(|t| t == "zzplan.md");
        if found {
            key(app, "ctrl+w");
        }
        found
    };
    // Find in folder for a word only the hidden file has: is it found?
    let find = |app: &mut App| {
        key(app, "ctrl+shift+f");
        type_text(app, "plan");
        key(app, "enter");
        app.settle(std::time::Duration::from_secs(5));
        render_hash(app, 800, 600);
        let found = app.status_text().starts_with("1 result ");
        key(app, "ctrl+w");
        found
    };

    // Without any settings (headless, tests): shown.
    let mut plain = new_app();
    plain.open(&a);
    assert!(shown(&mut plain, ".docs"));
    // The switch works there too, for now (nothing to save it in). Ctrl+P's
    // list of files, built with the hidden ones, does not outlive it.
    key(&mut plain, "ctrl+p");
    key(&mut plain, "escape");
    key(&mut plain, "ctrl+shift+h");
    assert!(!quick_open(&mut plain));
    assert!(!shown(&mut plain, ".docs"));

    let mut app = new_app();
    app.use_settings(path.clone()); // defaults: hidden files shown
    app.open(&a);
    assert!(shown(&mut app, ".docs") && shown(&mut app, ".env"));
    assert!(!shown(&mut app, ".git"), "ignored, whatever the setting");
    assert!(quick_open(&mut app), "Ctrl+P looks where the tree looks");
    assert!(find(&mut app), "and so does find in folder");

    key(&mut app, "ctrl+shift+h");
    assert!(
        app.status_text().contains("not shown"),
        "{}",
        app.status_text()
    );
    assert!(!quick_open(&mut app), "not in Ctrl+P any more");
    assert!(!shown(&mut app, ".docs") && !shown(&mut app, ".env"));
    assert!(shown(&mut app, "a1.md"));
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(saved.contains("hidden_files = off"), "{saved}");
    assert!(!quick_open(&mut app), "not in Ctrl+P either");
    assert!(!find(&mut app), "nor in find in folder");

    // The next start reads the choice.
    let mut fresh = new_app();
    fresh.use_settings(path.clone());
    fresh.open(&a);
    assert!(!shown(&mut fresh, ".docs"));
    assert!(!quick_open(&mut fresh));
    // View → Hidden Files turns them back on, and that is saved too.
    fresh.set_menu_bar(true);
    click_title(&mut fresh, 2);
    run_menu_item(&mut fresh, "Hidden Files");
    assert!(shown(&mut fresh, ".docs"));
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(saved.contains("hidden_files = on"), "{saved}");
    assert!(quick_open(&mut fresh));
}

#[test]
fn hiding_the_menu_bar_is_saved_in_settings() {
    assert!(Settings::default().menu_bar);
    assert!(!Settings::parse("menu_bar = off\n").0.menu_bar);
    let dir = settings_dir("menubar");
    let path = dir.join("settings.txt");
    let mut app = new_app();
    app.use_settings(path.clone()); // defaults: bar on
    render_hash(&mut app, 800, 600);
    let with_bar = app.main_rect().y;
    run_command(&mut app, "View: Show / Hide Menu Bar");
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("menu_bar = off")
    );
    assert!(app.main_rect().y < with_bar);
    assert!(app.status_text().contains("Alt+F"), "{}", app.status_text());
    // View → Show Menu Bar (from the hidden bar) turns it back on.
    key(&mut app, "alt+v");
    render_hash(&mut app, 800, 600);
    run_menu_item(&mut app, "Show Menu Bar");
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("menu_bar = on")
    );
    assert_eq!(app.main_rect().y, with_bar);
    // The settings file with the line off starts without the bar.
    std::fs::write(&path, "menu_bar = off\n").unwrap();
    let mut fresh = new_app();
    fresh.use_settings(path);
    render_hash(&mut fresh, 800, 600);
    assert_eq!(fresh.bar_title_pos(0), None);
}

#[test]
fn fuzz_the_menus_never_panics_or_deletes_files() {
    let (a, _b, _store) = two_projects("menufuzz");
    let p = a.join("a1.md");
    let mut app = bar_app();
    app.open(&a);
    app.open(&p);
    let keys = [
        "alt+f", "alt+e", "alt+v", "alt+g", "alt+h", "f10", "left", "right", "up", "down", "enter",
        "escape", "escape",
    ];
    let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut rnd = |n: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % n
    };
    for step in 0..600 {
        if app.nag_message().is_some() {
            key(&mut app, "escape");
            continue;
        }
        match rnd(3) {
            0 => {
                key(&mut app, keys[rnd(keys.len() as u64) as usize]);
            }
            1 => {
                let (x, y) = (rnd(800) as f32, rnd(420) as f32);
                app.handle(Input::MouseMove { x, y });
            }
            _ => {
                // Clicks stay near the top: titles and the menus below them.
                let (x, y) = (rnd(500) as f32, rnd(420) as f32);
                mouse(&mut app, x, y, MouseButton::Left);
            }
        }
        if step % 25 == 0 {
            render_hash(&mut app, 800, 600);
        }
    }
    render_hash(&mut app, 800, 600);
    // Whatever ran, the file on disk was only ever changed by a save the
    // menus asked for (Save), never deleted or emptied.
    assert!(p.exists());
}

#[test]
fn close_folder_sticks_in_a_window_started_with_a_path() {
    // `litedit .` starts without taking over the saved session; an explicit
    // Close Folder must still be what the next launch sees.
    let (a, _b, store) = two_projects("closeowned");
    let mut app = new_app();
    app.use_workspace_store(store);
    let saved = Session {
        root: Some(a.clone()),
        ..Session::default()
    };
    app.start_with_session(saved, false);
    app.open(&a);
    assert_eq!(app.session().root, Some(a.clone()));
    run_command(&mut app, "File: Close Folder");
    assert_eq!(app.session().root, None);
}

#[test]
fn typing_while_a_menu_is_open_does_not_edit_the_file() {
    let p = temp_file("menu-typing.txt", "hello\n");
    let mut app = bar_app();
    app.open(&p);
    key(&mut app, "alt+f");
    app.handle(Input::Text("s".into()));
    assert_eq!(app.snapshot().map(|s| s.0), Some("hello\n".to_string()));
    assert_eq!(app.bar_menu_open(), Some(0), "the menu stays open");
    // A right-click menu too.
    key(&mut app, "escape");
    render_hash(&mut app, 800, 600);
    let m = app.main_rect();
    let (x, y) = (m.x as f32 + 100.0, m.y as f32 + 10.0);
    app.handle(Input::MouseMove { x, y });
    mouse(&mut app, x, y, MouseButton::Right);
    assert!(!app.menu_labels().is_empty());
    app.handle(Input::Text("s".into()));
    assert_eq!(app.snapshot().map(|s| s.0), Some("hello\n".to_string()));
}

#[test]
fn edit_menu_items_act_on_the_file_even_when_the_tree_has_the_keyboard() {
    let (a, _b, _store) = two_projects("editfocus");
    let p = a.join("src/a2.rs");
    let mut app = bar_app();
    app.open(&a);
    app.open(&p);
    type_text(&mut app, "zz");
    assert!(app.snapshot().is_some_and(|s| s.0.starts_with("zz")));
    key(&mut app, "ctrl+shift+e"); // the tree has the keyboard
    assert_eq!(app.focus(), crate::app::Focus::Tree);
    click_title(&mut app, 1);
    run_menu_item(&mut app, "Undo");
    assert_eq!(
        app.snapshot().map(|s| s.0),
        Some("fn a2() {}".to_string()),
        "Edit → Undo undid the typing"
    );
    assert_eq!(app.focus(), crate::app::Focus::Main);
}

#[test]
fn the_menu_bar_renders_at_any_size() {
    let (a, _b, _store) = two_projects("barsizes");
    let mut app = bar_app();
    app.open(&a);
    app.open(&a.join("a1.md"));
    for (w, h) in [(1, 1), (40, 40), (300, 90), (800, 600)] {
        app.handle(Input::Resize(w, h));
        render_hash(&mut app, w, h);
        key(&mut app, "alt+v");
        render_hash(&mut app, w, h);
        key(&mut app, "down");
        render_hash(&mut app, w, h);
        key(&mut app, "escape");
        let r = app.tree_header_rect();
        if let Some(r) = r {
            mouse(
                &mut app,
                r.x as f32 + 5.0,
                r.y as f32 + 2.0,
                MouseButton::Left,
            );
            render_hash(&mut app, w, h);
            key(&mut app, "escape");
        }
    }
    // The start page at small sizes too.
    run_command(&mut app, "File: Close Folder");
    for (w, h) in [(1, 1), (40, 40), (300, 90)] {
        app.handle(Input::Resize(w, h));
        render_hash(&mut app, w, h);
        key(&mut app, "down");
        render_hash(&mut app, w, h);
    }
}
