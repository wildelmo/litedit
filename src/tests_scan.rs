//! Findings of the security scan: a restore never opens one file twice or
//! more than its budget, and content (workspace files, links in documents,
//! the tree, other launches) never makes us follow a link to a network
//! share.

use std::path::{Path, PathBuf};

use crate::app::{App, MAX_RESTORE_BYTES};
use crate::session::Session;
use crate::tests::{new_app, tmp_dir};

fn titles(app: &App) -> Vec<String> {
    app.tab_titles().into_iter().map(|t| t.0).collect()
}

fn write_workspace(dir: &Path, name: &str, lines: &str) -> PathBuf {
    let f = dir.join(format!("{name}.litedit-workspace"));
    std::fs::write(&f, format!("litedit-workspace 1\nfolder\t.\n{lines}")).unwrap();
    f
}

#[test]
fn a_workspace_naming_one_file_many_ways_restores_one_tab() {
    let d = tmp_dir("scan-spell");
    std::fs::create_dir_all(d.join("x/y")).unwrap();
    std::fs::write(d.join("big.txt"), "big").unwrap();
    std::fs::write(d.join("other.txt"), "other").unwrap();
    let ws = write_workspace(
        &d,
        "many",
        "tab\tbig.txt\ntab\t./big.txt\ntab\tx/../big.txt\ntab\tx/y/../../big.txt\n\
         tab\tother.txt\ntab-right\tx/./../big.txt\nactive\t3\n",
    );
    let mut app = new_app();
    app.open(&ws);
    assert_eq!(titles(&app), ["big.txt", "other.txt"]);
    assert_eq!(app.nag_message(), None);
    // The active entry was a second spelling of big.txt: big.txt is shown.
    assert_eq!(app.active_path(), Some(d.join("big.txt")));
    // Opening yet another spelling focuses the open tab.
    app.open(&d.join("x/../other.txt"));
    assert_eq!(titles(&app).len(), 2);
    assert_eq!(app.active_path(), Some(d.join("other.txt")));
    // Saved: each file once.
    assert_eq!(app.current_workspace().tabs.len(), 2);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_restore_budget_opens_the_active_tab_first_then_the_others_in_order() {
    assert_eq!(MAX_RESTORE_BYTES, 512 * 1024 * 1024);
    let d = tmp_dir("scan-budget");
    for n in ["a", "b", "c", "d"] {
        std::fs::write(d.join(format!("{n}.txt")), "0123456789").unwrap();
    }
    let ws = write_workspace(
        &d,
        "budget",
        "tab\ta.txt\ntab\tb.txt\ntab\tc.txt\ntab\td.txt\nactive\t2\n",
    );
    let mut app = new_app();
    app.set_restore_budget(25);
    app.open(&ws);
    // c (active) first, then a; b and d would go over 25 bytes.
    assert_eq!(titles(&app), ["a.txt", "c.txt"]);
    assert_eq!(app.active_path(), Some(d.join("c.txt")));
    assert_eq!(
        app.nag_message().as_deref(),
        Some(
            "2 files were not reopened: together they are larger than 25 bytes. \
             They stay in the workspace."
        )
    );
    crate::tests::press(&mut app, "enter");
    assert_eq!(app.nag_message(), None);
    // Saving the workspace keeps the files that were left out.
    let saved = d.join("saved.litedit-workspace");
    app.save_workspace_as(&saved);
    let text = std::fs::read_to_string(&saved).unwrap();
    for n in ["a", "b", "c", "d"] {
        assert!(text.contains(&format!("tab\t{n}.txt\n")), "{text}");
    }
    // Opening a left-out file makes it an ordinary tab: closing it takes it
    // out of the workspace.
    app.open(&d.join("b.txt"));
    assert_eq!(app.current_workspace().tabs.len(), 4);
    crate::tests::press(&mut app, "ctrl+w");
    let w = app.current_workspace();
    assert_eq!(w.tabs.len(), 3, "{:?}", w.tabs);
    assert!(w.tabs.iter().all(|(p, _)| !p.ends_with("b.txt")));

    // The active tab alone is too big: nothing is opened, nothing is lost.
    let mut app = new_app();
    app.set_restore_budget(5);
    app.open(&ws);
    assert!(titles(&app).iter().all(|t| !t.ends_with(".txt")));
    assert!(
        app.nag_message()
            .unwrap_or_default()
            .starts_with("4 files were not reopened")
    );
    assert_eq!(app.current_workspace().tabs.len(), 4);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn session_restore_has_the_same_budget_and_one_tab_per_file() {
    let d = tmp_dir("scan-session");
    for n in ["a", "b", "c"] {
        std::fs::write(d.join(format!("{n}.txt")), "0123456789").unwrap();
    }
    let s = Session {
        root: Some(d.clone()),
        tabs: vec![
            d.join("a.txt"),
            d.join("x/../a.txt"),
            d.join("b.txt"),
            d.join("c.txt"),
        ],
        active: 0,
        ..Session::default()
    };
    let mut app = new_app();
    app.set_restore_budget(20);
    app.start_with_session(s, true);
    assert_eq!(titles(&app), ["a.txt", "b.txt"]);
    assert!(app.nag_message().unwrap_or_default().starts_with(
        "1 file was not reopened: together with the others it would be larger than 20 bytes."
    ));
    // Remembered for next time: c.txt is still in the session.
    let tabs = app.session().tabs;
    assert_eq!(tabs, [d.join("a.txt"), d.join("b.txt"), d.join("c.txt")]);
    let _ = std::fs::remove_dir_all(&d);
}

/// A folder with ok.md, real/r.md, and links: net.md and netdir reach a
/// network share; local.md and localdir reach files here.
#[cfg(unix)]
fn linked_folder(tag: &str) -> PathBuf {
    let d = tmp_dir(tag);
    std::fs::create_dir_all(d.join("real")).unwrap();
    std::fs::write(d.join("ok.md"), "# ok").unwrap();
    std::fs::write(d.join("real/r.md"), "# r").unwrap();
    std::os::unix::fs::symlink("//evil/share/x.md", d.join("net.md")).unwrap();
    std::os::unix::fs::symlink("//evil/share", d.join("netdir")).unwrap();
    std::os::unix::fs::symlink(d.join("real/r.md"), d.join("local.md")).unwrap();
    std::os::unix::fs::symlink(d.join("real"), d.join("localdir")).unwrap();
    d
}

#[cfg(unix)]
const WHY: &str = "Not opened (it is a link to a network share or device path)";

#[cfg(unix)]
#[test]
fn a_workspace_file_never_opens_tabs_or_folders_through_a_link_to_a_share() {
    let d = linked_folder("scan-wslink");
    let ws = write_workspace(
        &d,
        "evil",
        "folder\tnetdir\nfolder\tlocaldir\ntab\tnet.md\ntab\tnetdir/x.md\ntab\tok.md\n\
         tab\tlocal.md\ntab\tlocaldir/r.md\nactive\t0\n",
    );
    let mut app = new_app();
    app.open(&ws);
    assert_eq!(titles(&app), ["ok.md", "local.md", "r.md"]);
    let roots = app.current_workspace().folders;
    assert_eq!(roots, [d.clone(), d.join("localdir")]);
    let status = app.status_text();
    assert!(
        status.contains(&format!("{WHY}: {}", d.join("netdir").display())),
        "{status}"
    );
    assert!(
        status.contains(&format!("{WHY}: {} and 1 more", d.join("net.md").display())),
        "{status}"
    );
    // The refused entries are not written back either.
    let w = app.current_workspace();
    assert!(
        w.tabs
            .iter()
            .all(|(p, _)| !p.starts_with(d.join("netdir")) && *p != d.join("net.md"))
    );

    // The main folder itself behind such a link: nothing is opened.
    let ws2 = d.join("evil2.litedit-workspace");
    std::fs::write(&ws2, "litedit-workspace 1\nfolder\tnetdir\ntab\tok.md\n").unwrap();
    let mut app = new_app();
    app.open(&ws2);
    assert_eq!(
        app.nag_message(),
        Some(format!(
            "The workspace's folder was not opened (it is a link to a network share or \
             device path): {}",
            d.join("netdir").display()
        ))
    );
    assert!(titles(&app).iter().all(|t| !t.ends_with(".md")));
    assert_eq!(app.current_workspace().folders, Vec::<PathBuf>::new());
    let _ = std::fs::remove_dir_all(&d);
}

#[cfg(unix)]
#[test]
fn a_tree_entry_that_is_a_link_to_a_share_is_never_opened_or_peeked() {
    use crate::tests::{click_tree, key};
    let d = linked_folder("scan-tree");
    let mut app = new_app();
    app.open(&d);
    let net = d.join("net.md");
    assert!(app.tree_rows().contains(&net), "{:?}", app.tree_rows());
    for clicks in [1, 2] {
        click_tree(&mut app, &net, clicks);
        assert!(
            titles(&app).iter().all(|t| t != "net.md"),
            "clicks {clicks}"
        );
        assert!(
            app.status_text().contains(&format!("{WHY}: net.md")),
            "{}",
            app.status_text()
        );
    }
    // Keyboard: Space peeks, Enter opens — neither does.
    key(&mut app, "space");
    key(&mut app, "enter");
    assert!(titles(&app).iter().all(|t| t != "net.md"));
    assert_eq!(app.active_path(), None);
    // A link to a file here opens as before.
    click_tree(&mut app, &d.join("local.md"), 2);
    assert_eq!(app.active_path(), Some(d.join("local.md")));
    assert_eq!(app.nag_message(), None);
    let _ = std::fs::remove_dir_all(&d);
}

#[cfg(unix)]
#[test]
fn paths_from_another_launch_through_a_link_to_a_share_are_dropped() {
    let d = linked_folder("scan-pipe");
    let mut app = new_app();
    app.open_from_other_launch(vec![
        d.join("net.md"),
        d.join("netdir/x.md"),
        d.join("ok.md"),
        d.join("localdir/r.md"),
    ]);
    assert_eq!(titles(&app), ["ok.md", "r.md"]);
    assert!(
        app.status_text()
            .contains(&format!("{WHY}: {} and 1 more", d.join("net.md").display())),
        "{}",
        app.status_text()
    );
    // What the receiving window gets from the pipe parser is checked the
    // same way (the pipe itself is Windows-only).
    let msg = crate::single::encode(&[d.join("net.md"), d.join("ok.md")]);
    let parsed = crate::single::parse(&msg).unwrap();
    let mut app = new_app();
    app.open_from_other_launch(parsed);
    assert_eq!(titles(&app), ["ok.md"]);
    // A launch with a file of its own behind a local link still forwards.
    assert!(crate::single::forwardable(&[d.join("local.md")]));
    let _ = std::fs::remove_dir_all(&d);
}

#[cfg(unix)]
#[test]
fn explicitly_opened_paths_through_local_links_still_open() {
    let d = linked_folder("scan-explicit");
    let mut app = new_app();
    app.open(&d.join("local.md"));
    app.open(&d.join("localdir/r.md"));
    assert_eq!(titles(&app), ["local.md", "r.md"]);
    assert_eq!(app.nag_message(), None);
    // The reader's own session is not gated either (only budgeted).
    let s = Session {
        root: Some(d.clone()),
        tabs: vec![d.join("local.md"), d.join("localdir/r.md")],
        ..Session::default()
    };
    let mut app = new_app();
    app.start_with_session(s, true);
    assert_eq!(titles(&app), ["local.md", "r.md"]);
    let _ = std::fs::remove_dir_all(&d);
}
