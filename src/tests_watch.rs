//! Watcher overflow scenarios at the app boundary.

use crate::input::Input;
use crate::tests::{new_app, type_text};

#[test]
fn overflow_rescan_reloads_open_file() {
    let dir = std::env::temp_dir().join(format!("litedit-watch-rescan-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("clean.txt");
    std::fs::write(&path, "one\n").unwrap();
    let mut app = new_app();
    app.open(&path);

    std::fs::write(&path, "one\ntwo\nthree\n").unwrap();
    assert!(app.handle(Input::FsChanged(Vec::new())));
    assert!(
        app.status_text().contains(" 4 lines"),
        "{}",
        app.status_text()
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn overflow_rescan_preserves_dirty_edits() {
    let dir = std::env::temp_dir().join(format!("litedit-watch-dirty-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("dirty.txt");
    std::fs::write(&path, "mine\n").unwrap();
    let mut app = new_app();
    app.open(&path);
    type_text(&mut app, "edit ");

    std::fs::write(&path, "theirs\n").unwrap();
    assert!(app.handle(Input::FsChanged(Vec::new())));
    assert!(app.title().starts_with('●'));
    assert!(app.nag_message().unwrap().contains("changed on disk"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "theirs\n");
    std::fs::remove_dir_all(&dir).ok();
}
