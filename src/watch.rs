//! Watching the open folder for external changes (lite-xl `dirwatch.lua`).
//!
//! notify delivers raw events on its own thread; we drop reads and ignored
//! folders, batch bursts for 150 ms, and hand each batch to a callback (the
//! window wakes the UI thread with it).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::time::Duration;

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::tree::is_ignored;

const BATCH_WINDOW: Duration = Duration::from_millis(150);

/// True if a change to `p` is worth telling the app about.
pub fn interesting(p: &Path) -> bool {
    !p.components().any(|c| {
        let s = c.as_os_str().to_string_lossy();
        is_ignored(&s)
    })
}

pub struct FsWatcher {
    watcher: Option<RecommendedWatcher>,
    targets: Vec<(PathBuf, bool)>,
}

impl FsWatcher {
    /// `notify_fn` is called (from a background thread) with each batch.
    pub fn new(notify_fn: impl Fn(Vec<PathBuf>) -> bool + Send + 'static) -> Self {
        let (tx, rx) = channel::<PathBuf>();
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(ev) = res else { return };
            if matches!(ev.kind, EventKind::Access(_) | EventKind::Other) {
                return; // reads (including our own) are not changes
            }
            for p in ev.paths {
                if interesting(&p) {
                    let _ = tx.send(p);
                }
            }
        })
        .ok();
        let _ = std::thread::Builder::new()
            .name("litedit-watch".into())
            .spawn(move || {
                while let Ok(first) = rx.recv() {
                    let mut batch = BTreeSet::new();
                    batch.insert(first);
                    loop {
                        match rx.recv_timeout(BATCH_WINDOW) {
                            Ok(p) => {
                                batch.insert(p);
                                if batch.len() > 10_000 {
                                    break;
                                }
                            }
                            Err(RecvTimeoutError::Timeout) => break,
                            Err(RecvTimeoutError::Disconnected) => return,
                        }
                    }
                    if !notify_fn(batch.into_iter().collect()) {
                        return; // the UI is gone
                    }
                }
            });
        Self {
            watcher,
            targets: Vec::new(),
        }
    }

    /// Watch exactly these folders (path, recursive). Cheap if unchanged.
    pub fn set_targets(&mut self, targets: Vec<(PathBuf, bool)>) {
        if targets == self.targets {
            return;
        }
        let Some(w) = self.watcher.as_mut() else {
            return;
        };
        for (p, _) in &self.targets {
            let _ = w.unwatch(p);
        }
        for (p, recursive) in &targets {
            let mode = if *recursive {
                RecursiveMode::Recursive
            } else {
                RecursiveMode::NonRecursive
            };
            let _ = w.watch(p, mode);
        }
        self.targets = targets;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn ignored_folders_are_filtered() {
        assert!(interesting(Path::new("/p/notes/a.md")));
        assert!(!interesting(Path::new("/p/.git/index")));
        assert!(!interesting(Path::new("/p/web/node_modules/x/y.js")));
    }

    #[test]
    fn changes_arrive_batched() {
        let dir = std::env::temp_dir().join(format!("litedit-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let got: Arc<Mutex<Vec<Vec<PathBuf>>>> = Arc::default();
        let g = got.clone();
        let mut w = FsWatcher::new(move |batch| {
            g.lock().unwrap().push(batch);
            true
        });
        w.set_targets(vec![(dir.clone(), true)]);
        std::thread::sleep(Duration::from_millis(100));
        for i in 0..5 {
            std::fs::write(dir.join(format!("f{i}.md")), "x").unwrap();
        }
        let start = std::time::Instant::now();
        while got.lock().unwrap().is_empty() && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        let batches = got.lock().unwrap().clone();
        assert!(!batches.is_empty(), "no change notification arrived");
        assert!(batches[0].iter().any(|p| p.ends_with("f0.md")));
        // Reading a file must not produce a change.
        got.lock().unwrap().clear();
        std::thread::sleep(Duration::from_millis(300));
        got.lock().unwrap().clear();
        let _ = std::fs::read(dir.join("f0.md"));
        std::thread::sleep(Duration::from_millis(400));
        assert!(
            got.lock().unwrap().is_empty(),
            "reads must not count as changes"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
