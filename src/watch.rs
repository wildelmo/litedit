//! Watching the open folder for external changes (lite-xl `dirwatch.lua`).
//!
//! notify delivers raw events on its own thread; we drop reads and ignored
//! folders, batch bursts for 150 ms, and hand each batch to a callback (the
//! window wakes the UI thread with it).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{RecvTimeoutError, TrySendError, sync_channel};
use std::time::{Duration, Instant};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::tree::is_ignored;

const BATCH_WINDOW: Duration = Duration::from_millis(150);
const MAX_BATCH_WINDOW: Duration = Duration::from_secs(1);
const MAX_BATCH_PATHS: usize = 10_000;
const EVENT_QUEUE_CAPACITY: usize = 1_024;

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
    notification_pending: Arc<AtomicBool>,
}

impl FsWatcher {
    /// `notify_fn` is called (from a background thread) with each batch.
    /// An empty batch means the bounded queue overflowed and all watched
    /// state must be rescanned.
    pub fn new(notify_fn: impl Fn(Vec<PathBuf>) -> bool + Send + 'static) -> Self {
        let (tx, rx) = sync_channel::<PathBuf>(EVENT_QUEUE_CAPACITY);
        let overflowed = Arc::new(AtomicBool::new(false));
        let producer_overflowed = overflowed.clone();
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(ev) = res else { return };
            if matches!(ev.kind, EventKind::Access(_) | EventKind::Other) {
                return; // reads (including our own) are not changes
            }
            for p in ev.paths {
                if interesting(&p) {
                    match tx.try_send(p) {
                        Ok(()) => {}
                        Err(TrySendError::Full(_)) => {
                            producer_overflowed.store(true, Ordering::Release);
                        }
                        Err(TrySendError::Disconnected(_)) => return,
                    }
                }
            }
        })
        .ok();
        let notification_pending = Arc::new(AtomicBool::new(false));
        let worker_pending = notification_pending.clone();
        let _ = std::thread::Builder::new()
            .name("litedit-watch".into())
            .spawn(move || {
                let mut batch = BTreeSet::new();
                let mut rescan = false;
                let mut started = None;
                let mut last_event = None;
                loop {
                    if overflowed.swap(false, Ordering::AcqRel) {
                        batch.clear();
                        rescan = true;
                    }

                    if batch.is_empty() && !rescan {
                        let Ok(first) = rx.recv() else { return };
                        let now = Instant::now();
                        batch.insert(first);
                        started = Some(now);
                        last_event = Some(now);
                        continue;
                    }

                    let now = Instant::now();
                    let quiet = last_event
                        .is_some_and(|at| now.saturating_duration_since(at) >= BATCH_WINDOW);
                    let expired = started
                        .is_some_and(|at| now.saturating_duration_since(at) >= MAX_BATCH_WINDOW);
                    let ready = rescan || batch.len() >= MAX_BATCH_PATHS || quiet || expired;

                    if ready {
                        if worker_pending
                            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                            .is_ok()
                        {
                            let paths = if rescan {
                                Vec::new()
                            } else {
                                std::mem::take(&mut batch).into_iter().collect()
                            };
                            if !notify_fn(paths) {
                                return; // the UI is gone
                            }
                            batch.clear();
                            rescan = false;
                            started = None;
                            last_event = None;
                        } else {
                            // One UI event already represents all earlier
                            // changes. Keep only a level-triggered rescan for
                            // changes arriving while it is being handled.
                            batch.clear();
                            rescan = true;
                            std::thread::park_timeout(BATCH_WINDOW);
                        }
                        continue;
                    }

                    let quiet_in = last_event.map_or(BATCH_WINDOW, |at| {
                        BATCH_WINDOW.saturating_sub(now.saturating_duration_since(at))
                    });
                    let expires_in = started.map_or(MAX_BATCH_WINDOW, |at| {
                        MAX_BATCH_WINDOW.saturating_sub(now.saturating_duration_since(at))
                    });
                    match rx.recv_timeout(quiet_in.min(expires_in)) {
                        Ok(p) => {
                            let now = Instant::now();
                            batch.insert(p);
                            started.get_or_insert(now);
                            last_event = Some(now);
                        }
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
            });
        Self {
            watcher,
            targets: Vec::new(),
            notification_pending,
        }
    }

    /// Allow the worker to send the next coalesced batch after the UI has
    /// handled the previous one.
    pub fn notification_handled(&self) {
        self.notification_pending.store(false, Ordering::Release);
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
    fn event_queue_has_fixed_capacity() {
        let (tx, _rx) = sync_channel::<PathBuf>(EVENT_QUEUE_CAPACITY);
        for i in 0..EVENT_QUEUE_CAPACITY {
            assert!(tx.try_send(PathBuf::from(i.to_string())).is_ok());
        }
        assert!(matches!(
            tx.try_send(PathBuf::from("overflow")),
            Err(TrySendError::Full(_))
        ));
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
