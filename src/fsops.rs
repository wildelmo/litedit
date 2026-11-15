//! File operations from the tree: new file, new folder, rename, delete.
//!
//! Names are validated as a single Windows path component before anything
//! touches the disk. Delete moves to the Recycle Bin; if Windows cannot
//! recycle (network drive, file too large), it shows its own "delete
//! permanently?" warning instead of silently destroying the file.

use std::path::{Path, PathBuf};

/// Check a user-typed file or folder name.
pub fn validate_name(name: &str) -> Result<&str, String> {
    let n = name.trim_start();
    if n.is_empty() {
        return Err("the name is empty".into());
    }
    if n == "." || n == ".." {
        return Err("that name is not allowed".into());
    }
    if n.ends_with('.') || n.ends_with(' ') {
        return Err("names cannot end with a dot or space on Windows".into());
    }
    if let Some(c) = n.chars().find(|c| {
        matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control()
    }) {
        return Err(format!("names cannot contain {c:?}"));
    }
    if n.chars().count() > 240 {
        return Err("the name is too long".into());
    }
    if crate::doc::is_reserved_name(Path::new(n)) {
        return Err("that name is reserved by Windows".into());
    }
    Ok(n)
}

/// Create an empty file `dir/name` (never overwrites).
pub fn create_file(dir: &Path, name: &str) -> Result<PathBuf, String> {
    let p = dir.join(validate_name(name)?);
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&p)
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                "a file with that name already exists".into()
            } else {
                e.to_string()
            }
        })?;
    Ok(p)
}

pub fn create_folder(dir: &Path, name: &str) -> Result<PathBuf, String> {
    let p = dir.join(validate_name(name)?);
    if p.exists() {
        return Err("something with that name already exists".into());
    }
    std::fs::create_dir(&p).map_err(|e| e.to_string())?;
    Ok(p)
}

/// Rename within the same folder (never overwrites).
pub fn rename(path: &Path, new_name: &str) -> Result<PathBuf, String> {
    let name = validate_name(new_name)?;
    let parent = path.parent().ok_or("cannot rename this")?;
    let target = parent.join(name);
    if target == path {
        return Ok(target);
    }
    if target.exists() {
        // A case-only rename (a.md → A.md) is allowed when it is the same
        // file: on a case-insensitive folder `exists` sees ourselves. If an
        // entry with exactly the new name is listed, it is another file
        // (case-sensitive folder) and must not be overwritten.
        let case_only =
            target.to_string_lossy().to_lowercase() == path.to_string_lossy().to_lowercase();
        let exact_exists = std::fs::read_dir(parent)
            .map(|rd| {
                rd.flatten()
                    .any(|e| e.file_name() == std::ffi::OsStr::new(name))
            })
            .unwrap_or(true);
        if !case_only || exact_exists {
            return Err("something with that name already exists".into());
        }
    }
    std::fs::rename(path, &target).map_err(|e| e.to_string())?;
    Ok(target)
}

/// Move a file or folder into another folder (tree drag and drop). Never
/// overwrites; refuses to move a folder into itself.
pub fn move_into(path: &Path, dir: &Path) -> Result<PathBuf, String> {
    let name = path.file_name().ok_or("cannot move this")?;
    if dir.starts_with(path) {
        return Err("cannot move a folder into itself".into());
    }
    if !dir.is_dir() {
        return Err("the destination is not a folder".into());
    }
    let target = dir.join(name);
    if target == path {
        return Ok(target);
    }
    // (A dangling link counts as something there.)
    if std::fs::symlink_metadata(&target).is_ok() {
        return Err(format!(
            "{} already has something named {}",
            dir.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            name.to_string_lossy()
        ));
    }
    std::fs::rename(path, &target).map_err(|e| e.to_string())?;
    Ok(target)
}

/// Move a file or folder to the Recycle Bin.
pub fn move_to_recycle_bin(path: &Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("refusing to delete a relative path".into());
    }
    if crate::links::is_network_path(&path.to_string_lossy()) {
        return Err("network locations have no Recycle Bin; delete it in Explorer".into());
    }
    if !path.exists() {
        return Err("it no longer exists".into());
    }
    #[cfg(test)]
    if let Some(bin) = TEST_RECYCLE.with(|t| t.borrow().clone()) {
        return test_recycle(path, &bin);
    }
    recycle(path)
}

#[cfg(test)]
thread_local! {
    /// Tests only: a folder that stands in for the Recycle Bin on this
    /// thread, so "delete" can be tested (also on Windows CI) without
    /// touching the real one.
    static TEST_RECYCLE: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

/// Tests only: "recycled" items go into `bin` (None = the real behaviour).
#[cfg(test)]
pub fn set_test_recycle_dir(bin: Option<PathBuf>) {
    TEST_RECYCLE.with(|t| *t.borrow_mut() = bin);
}

#[cfg(test)]
fn test_recycle(path: &Path, bin: &Path) -> Result<(), String> {
    let name = path.file_name().ok_or("cannot delete this")?;
    // Like the real bin: several items with one name can sit in it.
    for n in 0..10_000 {
        let mut slot = std::ffi::OsString::from(format!("{n:04}-"));
        slot.push(name);
        let target = bin.join(slot);
        if std::fs::symlink_metadata(&target).is_err() {
            return std::fs::rename(path, &target).map_err(|e| e.to_string());
        }
    }
    Err("the test recycle folder is full".into())
}

#[cfg(windows)]
fn recycle(path: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::{
        FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_SILENT, FOF_WANTNUKEWARNING,
        SHFILEOPSTRUCTW, SHFileOperationW,
    };
    // pFrom must be double-NUL terminated.
    let mut from: Vec<u16> = path.as_os_str().encode_wide().collect();
    from.extend([0, 0]);
    let mut op = SHFILEOPSTRUCTW {
        hwnd: std::ptr::null_mut(),
        wFunc: FO_DELETE,
        pFrom: from.as_ptr(),
        pTo: std::ptr::null(),
        // Recycle; no "are you sure" (litedit asked already), no progress UI,
        // but DO warn if the item would be destroyed instead of recycled.
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_WANTNUKEWARNING) as u16,
        fAnyOperationsAborted: 0,
        hNameMappings: std::ptr::null_mut(),
        lpszProgressTitle: std::ptr::null(),
    };
    // SAFETY: `op` is fully initialised; `from` outlives the call.
    let r = unsafe { SHFileOperationW(&mut op) };
    if r != 0 {
        return Err(format!(
            "Windows could not move it to the Recycle Bin (code {r})"
        ));
    }
    if op.fAnyOperationsAborted != 0 {
        return Err("cancelled".into());
    }
    Ok(())
}

#[cfg(not(windows))]
fn recycle(_path: &Path) -> Result<(), String> {
    Err("the Recycle Bin is only available on Windows".into())
}

// ---- copy / paste / duplicate ---------------------------------------------

/// How deep a copied folder may nest (a hostile or runaway tree).
pub const MAX_COPY_DEPTH: usize = 64;

/// Bounds for one paste or duplicate (everything in it, added up).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CopyLimits {
    /// Files plus folders.
    pub entries: usize,
    pub bytes: u64,
}

impl Default for CopyLimits {
    fn default() -> Self {
        Self {
            entries: 10_000,
            bytes: 1 << 30,
        }
    }
}

impl CopyLimits {
    fn entries_message(&self) -> String {
        format!("more than {} files and folders", group_digits(self.entries))
    }

    fn bytes_message(&self) -> String {
        format!("more than {}", human_bytes(self.bytes))
    }
}

/// 1234567 → "1,234,567".
pub fn group_digits(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// "532 bytes", "12.4 KB", "3.1 MB", "1.0 GB".
pub fn human_bytes(n: u64) -> String {
    const KB: f64 = 1024.0;
    let f = n as f64;
    if n < 1024 {
        format!("{n} bytes")
    } else if f < KB * KB {
        format!("{:.1} KB", f / KB)
    } else if f < KB * KB * KB {
        format!("{:.1} MB", f / (KB * KB))
    } else {
        format!("{:.1} GB", f / (KB * KB * KB))
    }
}

/// The `n`-th name for a copy of `name`: "notes - Copy.md", "notes - Copy (2).md".
/// Folders keep dots in their name ("v1.2 - Copy").
pub fn copy_name(name: &str, is_dir: bool, n: usize) -> String {
    let suffix = if n <= 1 {
        " - Copy".to_string()
    } else {
        format!(" - Copy ({n})")
    };
    let p = Path::new(name);
    match (p.file_stem(), p.extension()) {
        (Some(stem), Some(ext)) if !is_dir => format!(
            "{}{suffix}.{}",
            stem.to_string_lossy(),
            ext.to_string_lossy()
        ),
        _ => format!("{name}{suffix}"),
    }
}

/// Is there anything at `p` (a dangling link counts)?
fn occupied(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok()
}

/// Where a copy of `src` goes inside `dir` without replacing anything: its
/// own name if that is free, otherwise "name - Copy.ext", "name - Copy (2).ext", …
pub fn free_copy_target(src: &Path, dir: &Path, is_dir: bool) -> Result<PathBuf, String> {
    let name = src.file_name().ok_or("cannot copy this")?;
    let plain = dir.join(name);
    if !occupied(&plain) {
        return Ok(plain);
    }
    let name = name.to_string_lossy();
    for n in 1..=999 {
        let p = dir.join(copy_name(&name, is_dir, n));
        if !occupied(&p) {
            return Ok(p);
        }
    }
    Err("there are too many copies of it already".into())
}

/// Paths that are not inside another path of the list (and no repeats): when
/// a folder and something in it are both chosen, only the folder is acted on.
/// Order is kept.
pub fn without_nested(paths: &[PathBuf]) -> Vec<PathBuf> {
    // One pass over each path's parents (a whole folder may be selected:
    // comparing every pair would freeze the window).
    let all: std::collections::HashSet<&Path> = paths.iter().map(PathBuf::as_path).collect();
    let mut seen: std::collections::HashSet<&Path> = std::collections::HashSet::new();
    paths
        .iter()
        .filter(|p| {
            let nested = p.ancestors().skip(1).any(|a| all.contains(a));
            !nested && seen.insert(p.as_path())
        })
        .cloned()
        .collect()
}

/// Is `p` at or under `base`? On Windows names differ only by case are the
/// same file, however each path was typed.
pub fn is_under(p: &Path, base: &Path) -> bool {
    is_under_fold(p, base, cfg!(windows))
}

fn is_under_fold(p: &Path, base: &Path, fold: bool) -> bool {
    if !fold {
        return p.starts_with(base);
    }
    let lower = |c: std::path::Component| c.as_os_str().to_string_lossy().to_lowercase();
    let mut mine = p.components();
    base.components()
        .all(|b| mine.next().is_some_and(|m| lower(m) == lower(b)))
}

/// Where `p` (at or under `old`) is after `old` became `new`. Counts path
/// parts instead of comparing them, so a different spelling of `old` (case
/// on Windows) still gives the right place.
pub fn rebase(p: &Path, old: &Path, new: &Path) -> PathBuf {
    let mut out = new.to_path_buf();
    out.extend(p.components().skip(old.components().count()));
    out
}

/// What a source is, without following links.
enum Kind {
    File(u64),
    Dir,
    /// Symlink / junction, or something that is neither file nor folder.
    Skip,
}

fn kind_of(p: &Path) -> std::io::Result<Kind> {
    let m = std::fs::symlink_metadata(p)?;
    let t = m.file_type();
    Ok(if t.is_symlink() {
        Kind::Skip
    } else if t.is_dir() {
        Kind::Dir
    } else if t.is_file() {
        Kind::File(m.len())
    } else {
        Kind::Skip
    })
}

/// A file being copied: `to` was created by us and is incomplete.
struct OpenCopy {
    from: std::fs::File,
    to: std::fs::File,
    to_path: PathBuf,
    from_path: PathBuf,
}

/// One paste or duplicate, done a slice at a time (`step`) so a big copy
/// never freezes the window.
///
/// Rules: nothing is ever replaced (files are made with `create_new`,
/// folders with `create_dir`); links and junctions are never followed
/// (skipped and counted); a folder is never copied into itself; the whole
/// job is refused up front if it is over the limits, and stops if the
/// sources grow past them meanwhile. A file that could not be finished is
/// removed again, so no half-written file stays behind.
pub struct CopyJob {
    limits: CopyLimits,
    /// Top-level items not started yet: (source, destination folder).
    queue: std::collections::VecDeque<(PathBuf, PathBuf)>,
    /// The item being copied: (source, the copy's path).
    item: Option<(PathBuf, PathBuf)>,
    /// Entries of that item still to copy: (source, destination).
    stack: Vec<(PathBuf, PathBuf)>,
    open: Option<OpenCopy>,
    buf: Vec<u8>,
    /// Files and folders made / bytes written so far.
    entries: usize,
    bytes: u64,
    /// A limit was passed while copying: nothing more is started.
    over: Option<String>,
    /// From the scan before the copy started (for the progress text).
    total_entries: usize,
    total_bytes: u64,
    /// Finished top-level copies.
    pub done: Vec<PathBuf>,
    /// Top-level items that were not (fully) copied, and why.
    pub failed: Vec<(PathBuf, String)>,
    /// Links and special files left out.
    pub skipped: usize,
    /// The first few of them, for the report.
    pub skipped_names: Vec<String>,
}

impl CopyJob {
    /// Plan copying each of `items` (source, destination folder). Scans the
    /// sources first (bounded) and refuses the whole job if it is too big;
    /// nothing has been written when this returns an error.
    pub fn new(items: Vec<(PathBuf, PathBuf)>, limits: CopyLimits) -> Result<CopyJob, String> {
        let mut job = CopyJob {
            limits,
            queue: Default::default(),
            item: None,
            stack: Vec::new(),
            open: None,
            buf: Vec::new(),
            entries: 0,
            bytes: 0,
            over: None,
            total_entries: 0,
            total_bytes: 0,
            done: Vec::new(),
            failed: Vec::new(),
            skipped: 0,
            skipped_names: Vec::new(),
        };
        for (src, dir) in items {
            if dir.starts_with(&src) {
                job.failed
                    .push((src, "a folder cannot be copied into itself".into()));
                continue;
            }
            // A source that cannot be read fails later, with its reason.
            job.scan(&src)?;
            job.queue.push_back((src, dir));
        }
        Ok(job)
    }

    /// Count what copying `src` would create; error once over the limits.
    fn scan(&mut self, src: &Path) -> Result<(), String> {
        let mut pending = vec![src.to_path_buf()];
        while let Some(p) = pending.pop() {
            match kind_of(&p) {
                Ok(Kind::File(len)) => {
                    self.total_entries += 1;
                    self.total_bytes = self.total_bytes.saturating_add(len);
                }
                Ok(Kind::Dir) => {
                    self.total_entries += 1;
                    if let Ok(rd) = std::fs::read_dir(&p) {
                        for e in rd.flatten() {
                            pending.push(e.path());
                            if self.total_entries + pending.len() > self.limits.entries {
                                return Err(self.limits.entries_message());
                            }
                        }
                    }
                }
                Ok(Kind::Skip) | Err(_) => {}
            }
            if self.total_entries > self.limits.entries {
                return Err(self.limits.entries_message());
            }
            if self.total_bytes > self.limits.bytes {
                return Err(self.limits.bytes_message());
            }
        }
        Ok(())
    }

    pub fn finished(&self) -> bool {
        self.queue.is_empty() && self.item.is_none() && self.open.is_none()
    }

    /// "37%" of the bytes (or of the entries when there are no bytes).
    pub fn progress(&self) -> String {
        let (done, total) = if self.total_bytes > 0 {
            (self.bytes as f64, self.total_bytes as f64)
        } else {
            (self.entries as f64, self.total_entries.max(1) as f64)
        };
        format!("{:.0}%", (done / total * 100.0).clamp(0.0, 100.0))
    }

    /// Work for about `budget`. Returns true while there is more to do.
    pub fn step(&mut self, budget: std::time::Duration) -> bool {
        let start = std::time::Instant::now();
        loop {
            if self.finished() {
                return false;
            }
            if let Err(e) = self.advance() {
                self.fail_item(e);
            }
            if start.elapsed() >= budget {
                return !self.finished();
            }
        }
    }

    /// Stop now: the unfinished file is removed, the rest is reported as
    /// not copied.
    pub fn cancel(&mut self) {
        if self.item.is_some() {
            self.fail_item("cancelled".into());
        }
        for (src, _) in std::mem::take(&mut self.queue) {
            self.failed.push((src, "cancelled".into()));
        }
    }

    /// Remove the half-written file, if any.
    fn drop_open(&mut self) {
        if let Some(o) = self.open.take() {
            let path = o.to_path.clone();
            drop(o);
            let _ = std::fs::remove_file(path);
        }
    }

    /// The current item cannot be completed.
    fn fail_item(&mut self, why: String) {
        self.drop_open();
        self.stack.clear();
        if let Some((src, to)) = self.item.take() {
            // Say so when part of a folder was already copied.
            let why = if kind_of(&to).is_ok() {
                format!(
                    "{why}; \"{}\" is incomplete",
                    to.file_name().unwrap_or_default().to_string_lossy()
                )
            } else {
                why
            };
            self.failed.push((src, why));
        }
    }

    fn skip(&mut self, p: &Path) {
        self.skipped += 1;
        if self.skipped_names.len() < 3 {
            self.skipped_names.push(
                p.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
            );
        }
    }

    /// The sources grew past a limit since the scan: stop the whole job.
    fn over_limit(&mut self, msg: String) -> String {
        self.over = Some(msg.clone());
        msg
    }

    fn count_entry(&mut self) -> Result<(), String> {
        self.entries += 1;
        if self.entries > self.limits.entries {
            return Err(self.over_limit(self.limits.entries_message()));
        }
        Ok(())
    }

    /// One small unit of work: a chunk of a file, or starting one entry.
    fn advance(&mut self) -> Result<(), String> {
        const CHUNK: usize = 256 * 1024;
        if let Some(o) = self.open.as_mut() {
            use std::io::{Read, Write};
            if self.buf.len() != CHUNK {
                self.buf = vec![0; CHUNK];
            }
            let name = |p: &Path| {
                p.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            };
            let n = match o.from.read(&mut self.buf) {
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => return Ok(()),
                Err(e) => return Err(format!("{}: {e}", name(&o.from_path))),
            };
            if n == 0 {
                // Complete: keep the file's date and read-only flag.
                if let Some(done) = self.open.take() {
                    let meta = done.from.metadata().ok();
                    if let Some(t) = meta.as_ref().and_then(|m| m.modified().ok()) {
                        let _ = done.to.set_modified(t);
                    }
                    let to_path = done.to_path.clone();
                    drop(done);
                    if let Some(m) = meta {
                        let _ = std::fs::set_permissions(&to_path, m.permissions());
                    }
                }
                return Ok(());
            }
            self.bytes += n as u64;
            if self.bytes > self.limits.bytes {
                let msg = self.limits.bytes_message();
                return Err(self.over_limit(msg));
            }
            return o
                .to
                .write_all(&self.buf[..n])
                .map_err(|e| format!("{}: {e}", name(&o.to_path)));
        }
        if let Some((src, to)) = self.stack.pop() {
            return self.start_entry(&src, &to);
        }
        if let Some((_, to)) = self.item.take() {
            self.done.push(to);
            return Ok(());
        }
        // Start the next top-level item.
        let Some((src, dir)) = self.queue.pop_front() else {
            return Ok(());
        };
        if let Some(why) = &self.over {
            self.failed.push((src, format!("not copied ({why})")));
            return Ok(());
        }
        let kind = match kind_of(&src) {
            Ok(k) => k,
            Err(_) => {
                self.failed.push((src, "it no longer exists".into()));
                return Ok(());
            }
        };
        if matches!(kind, Kind::Skip) {
            self.skip(&src);
            return Ok(());
        }
        if !matches!(kind_of(&dir), Ok(Kind::Dir)) {
            self.failed
                .push((src, "the destination is not a folder".into()));
            return Ok(());
        }
        match free_copy_target(&src, &dir, matches!(kind, Kind::Dir)) {
            Ok(to) => {
                self.item = Some((src.clone(), to.clone()));
                self.stack.push((src, to));
            }
            Err(e) => self.failed.push((src, e)),
        }
        Ok(())
    }

    /// Create `to` as a copy of `src` (a folder's entries are queued).
    fn start_entry(&mut self, src: &Path, to: &Path) -> Result<(), String> {
        let name = || {
            src.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        };
        let exists = |e: std::io::Error| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                format!("{}: something with that name already exists", name())
            } else {
                format!("{}: {e}", name())
            }
        };
        match kind_of(src).map_err(|e| format!("{}: {e}", name()))? {
            Kind::Skip => self.skip(src),
            Kind::Dir => {
                // Depth below the folder this item's copy started in.
                let depth = self.item.as_ref().map_or(0, |(_, root)| {
                    to.components()
                        .count()
                        .saturating_sub(root.components().count())
                });
                if depth >= MAX_COPY_DEPTH {
                    return Err(format!(
                        "{}: folders nested more than {MAX_COPY_DEPTH} deep",
                        name()
                    ));
                }
                self.count_entry()?;
                std::fs::create_dir(to).map_err(exists)?;
                let rd = std::fs::read_dir(src).map_err(|e| format!("{}: {e}", name()))?;
                for e in rd {
                    let e = e.map_err(|e| format!("{}: {e}", name()))?;
                    self.stack.push((e.path(), to.join(e.file_name())));
                    if self.entries + self.stack.len() > self.limits.entries {
                        return Err(self.over_limit(self.limits.entries_message()));
                    }
                }
            }
            Kind::File(_) => {
                self.count_entry()?;
                let from = std::fs::File::open(src).map_err(|e| format!("{}: {e}", name()))?;
                let out = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(to)
                    .map_err(exists)?;
                self.open = Some(OpenCopy {
                    from,
                    to: out,
                    to_path: to.to_path_buf(),
                    from_path: src.to_path_buf(),
                });
            }
        }
        Ok(())
    }
}

impl Drop for CopyJob {
    /// Dropped mid-copy (the window closed): no half-written file stays.
    fn drop(&mut self) {
        self.drop_open();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("litedit-copy-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn run(job: &mut CopyJob) {
        let mut steps = 0;
        // Tiny slices, so files are copied across many steps.
        while job.step(std::time::Duration::ZERO) {
            steps += 1;
            assert!(steps < 1_000_000, "the copy never finishes");
        }
    }

    fn copy_all(items: &[(PathBuf, PathBuf)], limits: CopyLimits) -> Result<CopyJob, String> {
        let mut job = CopyJob::new(items.to_vec(), limits)?;
        run(&mut job);
        Ok(job)
    }

    /// Every file under `dir` with its bytes (relative paths, sorted).
    fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut out = Vec::new();
        let mut pending = vec![dir.to_path_buf()];
        while let Some(d) = pending.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let t = e.file_type().unwrap();
                if t.is_dir() {
                    pending.push(e.path());
                } else if t.is_file() {
                    out.push((
                        e.path().strip_prefix(dir).unwrap().to_path_buf(),
                        std::fs::read(e.path()).unwrap(),
                    ));
                }
            }
        }
        out.sort();
        out
    }

    #[test]
    fn copy_names() {
        assert_eq!(copy_name("notes.md", false, 1), "notes - Copy.md");
        assert_eq!(copy_name("notes.md", false, 2), "notes - Copy (2).md");
        assert_eq!(
            copy_name("archive.tar.gz", false, 1),
            "archive.tar - Copy.gz"
        );
        assert_eq!(copy_name("Makefile", false, 3), "Makefile - Copy (3)");
        assert_eq!(copy_name(".gitignore", false, 1), ".gitignore - Copy");
        assert_eq!(copy_name("v1.2", true, 1), "v1.2 - Copy");
        assert_eq!(group_digits(10_000), "10,000");
        assert_eq!(group_digits(999), "999");
        assert_eq!(group_digits(1_234_567), "1,234,567");
        assert_eq!(human_bytes(532), "532 bytes");
        assert_eq!(human_bytes(12_698), "12.4 KB");
        assert_eq!(human_bytes(1 << 30), "1.0 GB");
    }

    #[test]
    fn without_nested_is_quick_for_a_whole_folder_and_matches_the_simple_rule() {
        let simple = |paths: &[PathBuf]| -> Vec<PathBuf> {
            let mut out: Vec<PathBuf> = Vec::new();
            for p in paths {
                let nested = paths.iter().any(|q| q != p && p.starts_with(q));
                if !nested && !out.contains(p) {
                    out.push(p.clone());
                }
            }
            out
        };
        let names = ["a", "b", "a/x", "a/x/y", "b/z", "c/d", "a", "ab", "c"];
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        for _ in 0..500 {
            let mut v = Vec::new();
            for _ in 0..(seed % 7) {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                v.push(PathBuf::from("/r").join(names[(seed % names.len() as u64) as usize]));
            }
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            assert_eq!(without_nested(&v), simple(&v), "{v:?}");
        }
        let many: Vec<PathBuf> = (0..20_000)
            .map(|i| PathBuf::from(format!("/r/dir/file{i}.txt")))
            .collect();
        let t = std::time::Instant::now();
        assert_eq!(without_nested(&many).len(), 20_000);
        assert!(
            t.elapsed() < std::time::Duration::from_secs(2),
            "{:?}",
            t.elapsed()
        );
    }

    #[test]
    fn under_and_rebase_ignore_case_only_where_the_file_system_does() {
        let (p, base) = (Path::new("/Work/Notes/b.md"), Path::new("/work/notes"));
        assert!(is_under_fold(p, base, true));
        assert!(!is_under_fold(p, base, false));
        assert!(is_under_fold(base, base, true) && is_under_fold(base, base, false));
        assert!(!is_under_fold(Path::new("/work/notes2/b.md"), base, true));
        assert!(!is_under_fold(Path::new("/work"), base, true));
        assert_eq!(
            rebase(p, base, Path::new("/x/moved")),
            PathBuf::from("/x/moved/b.md")
        );
        assert_eq!(rebase(base, base, Path::new("/x/n")), PathBuf::from("/x/n"));
    }

    #[test]
    fn nested_choices_collapse_to_the_folder() {
        let p = |s: &str| PathBuf::from(s);
        assert_eq!(
            without_nested(&[
                p("/r/a/x.md"),
                p("/r/a"),
                p("/r/ab"),
                p("/r/a/sub/y"),
                p("/r/b.md"),
                p("/r/a"),
            ]),
            vec![p("/r/a"), p("/r/ab"), p("/r/b.md")]
        );
    }

    #[test]
    fn copies_are_exact_and_never_replace_anything() {
        let d = scratch("exact");
        // Every byte value, longer than one chunk, no final newline.
        let big: Vec<u8> = (0..700_000u32).map(|i| (i * 31 % 251) as u8).collect();
        std::fs::create_dir_all(d.join("src/deep/er")).unwrap();
        std::fs::create_dir_all(d.join("src/empty")).unwrap();
        std::fs::write(d.join("src/big.bin"), &big).unwrap();
        std::fs::write(d.join("src/deep/crlf.txt"), b"a\r\nb\r\n\xff\xfe").unwrap();
        std::fs::write(d.join("src/deep/er/zero"), b"").unwrap();
        std::fs::write(d.join("one.md"), b"\xef\xbb\xbfbom").unwrap();
        std::fs::create_dir_all(d.join("out")).unwrap();
        std::fs::write(d.join("out/one.md"), "already here").unwrap();

        let job = copy_all(
            &[
                (d.join("src"), d.join("out")),
                (d.join("one.md"), d.join("out")),
                (d.join("one.md"), d.join("out")),
                // Duplicate: a copy next to the original.
                (d.join("src"), d.clone()),
            ],
            CopyLimits::default(),
        )
        .unwrap();
        assert!(job.failed.is_empty(), "{:?}", job.failed);
        assert_eq!(
            job.done,
            vec![
                d.join("out/src"),
                d.join("out/one - Copy.md"),
                d.join("out/one - Copy (2).md"),
                d.join("src - Copy"),
            ]
        );
        assert_eq!(snapshot(&d.join("out/src")), snapshot(&d.join("src")));
        assert_eq!(snapshot(&d.join("src - Copy")), snapshot(&d.join("src")));
        assert!(d.join("out/src/empty").is_dir(), "empty folders are copied");
        assert_eq!(std::fs::read(d.join("out/src/big.bin")).unwrap(), big);
        assert_eq!(
            std::fs::read_to_string(d.join("out/one.md")).unwrap(),
            "already here"
        );
        assert_eq!(
            std::fs::read(d.join("out/one - Copy (2).md")).unwrap(),
            b"\xef\xbb\xbfbom"
        );
        // The copy keeps the file's date.
        let m = |p: PathBuf| std::fs::metadata(p).unwrap().modified().unwrap();
        assert_eq!(m(d.join("out/src/big.bin")), m(d.join("src/big.bin")));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_copy_over_the_limits_is_refused_before_anything_is_written() {
        let d = scratch("limits");
        std::fs::create_dir_all(d.join("many/sub")).unwrap();
        for i in 0..20 {
            std::fs::write(d.join(format!("many/sub/f{i}")), "0123456789").unwrap();
        }
        std::fs::create_dir_all(d.join("out")).unwrap();
        let items = vec![(d.join("many"), d.join("out"))];
        // 22 entries (2 folders + 20 files), 200 bytes.
        let too_many = CopyJob::new(
            items.clone(),
            CopyLimits {
                entries: 21,
                bytes: 1 << 20,
            },
        );
        assert_eq!(
            too_many.err().as_deref(),
            Some("more than 21 files and folders")
        );
        let too_big = CopyJob::new(
            items.clone(),
            CopyLimits {
                entries: 100,
                bytes: 199,
            },
        );
        assert_eq!(too_big.err().as_deref(), Some("more than 199 bytes"));
        assert_eq!(std::fs::read_dir(d.join("out")).unwrap().count(), 0);
        // Exactly at the limits: fine.
        let job = copy_all(
            &items,
            CopyLimits {
                entries: 22,
                bytes: 200,
            },
        )
        .unwrap();
        assert!(job.failed.is_empty(), "{:?}", job.failed);
        assert_eq!(snapshot(&d.join("out/many")), snapshot(&d.join("many")));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn sources_growing_during_the_copy_stop_it_without_a_half_written_file() {
        let d = scratch("grow");
        std::fs::create_dir_all(d.join("src")).unwrap();
        std::fs::create_dir_all(d.join("out")).unwrap();
        std::fs::write(d.join("src/a.bin"), vec![7u8; 600_000]).unwrap();
        std::fs::write(d.join("later.txt"), "x").unwrap();
        let mut job = CopyJob::new(
            vec![
                (d.join("src"), d.join("out")),
                (d.join("later.txt"), d.join("out")),
            ],
            CopyLimits::default(),
        )
        .unwrap();
        // As if the file had grown after the scan.
        job.limits.bytes = 300_000;
        run(&mut job);
        assert_eq!(
            job.failed,
            vec![
                (
                    d.join("src"),
                    "more than 293.0 KB; \"src\" is incomplete".to_string()
                ),
                (
                    d.join("later.txt"),
                    "not copied (more than 293.0 KB)".to_string()
                ),
            ]
        );
        assert!(job.done.is_empty());
        assert_eq!(
            std::fs::read_dir(d.join("out/src")).unwrap().count(),
            0,
            "only the folder made so far is left"
        );
        assert!(
            !d.join("out/src/a.bin").exists(),
            "the half-written file is removed"
        );
        assert_eq!(std::fs::read(d.join("src/a.bin")).unwrap().len(), 600_000);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn closing_mid_copy_leaves_no_half_written_file() {
        let d = scratch("drop");
        std::fs::create_dir_all(d.join("out")).unwrap();
        std::fs::write(d.join("a.bin"), vec![1u8; 900_000]).unwrap();
        let mut job = CopyJob::new(
            vec![(d.join("a.bin"), d.join("out"))],
            CopyLimits::default(),
        )
        .unwrap();
        // Start the item, create the file, write one chunk.
        for _ in 0..3 {
            assert!(job.step(std::time::Duration::ZERO));
        }
        let partial = std::fs::metadata(d.join("out/a.bin")).unwrap().len();
        assert!(partial > 0 && partial < 900_000, "{partial}");
        drop(job);
        assert!(!d.join("out/a.bin").exists());
        // Cancelling does the same and says what was not copied.
        let mut job = CopyJob::new(
            vec![
                (d.join("a.bin"), d.join("out")),
                (d.join("a.bin"), d.join("out")),
            ],
            CopyLimits::default(),
        )
        .unwrap();
        for _ in 0..3 {
            job.step(std::time::Duration::ZERO);
        }
        job.cancel();
        assert!(job.finished() && !job.step(std::time::Duration::ZERO));
        assert_eq!(job.failed.len(), 2);
        assert!(job.failed.iter().all(|(_, why)| why == "cancelled"));
        assert_eq!(std::fs::read_dir(d.join("out")).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_folder_is_never_copied_into_itself() {
        let d = scratch("self");
        std::fs::create_dir_all(d.join("a/inner")).unwrap();
        std::fs::write(d.join("a/f.txt"), "f").unwrap();
        let job = copy_all(
            &[
                (d.join("a"), d.join("a")),
                (d.join("a"), d.join("a/inner")),
                (d.join("a/f.txt"), d.join("a/inner")),
            ],
            CopyLimits::default(),
        )
        .unwrap();
        assert_eq!(job.failed.len(), 2);
        assert!(job.failed.iter().all(|(p, why)| p == &d.join("a")
            && why == "a folder cannot be copied into itself"));
        assert_eq!(job.done, vec![d.join("a/inner/f.txt")]);
        assert_eq!(std::fs::read_dir(d.join("a")).unwrap().count(), 2);
        // Missing sources and destinations are reported, not fatal.
        let job = copy_all(
            &[
                (d.join("gone.txt"), d.join("a")),
                (d.join("a/f.txt"), d.join("nowhere")),
            ],
            CopyLimits::default(),
        )
        .unwrap();
        assert_eq!(
            job.failed,
            vec![
                (d.join("gone.txt"), "it no longer exists".to_string()),
                (
                    d.join("a/f.txt"),
                    "the destination is not a folder".to_string()
                ),
            ]
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn very_deep_folders_fail_that_item_and_say_so() {
        let d = scratch("deep");
        let mut deep = d.join("src");
        for _ in 0..(MAX_COPY_DEPTH + 5) {
            deep.push("d");
        }
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::create_dir_all(d.join("shallow/a/b")).unwrap();
        std::fs::write(d.join("shallow/a/b/f.txt"), "f").unwrap();
        std::fs::create_dir_all(d.join("out")).unwrap();
        let job = copy_all(
            &[
                (d.join("src"), d.join("out")),
                (d.join("shallow"), d.join("out")),
            ],
            CopyLimits::default(),
        )
        .unwrap();
        assert_eq!(job.done, vec![d.join("out/shallow")]);
        assert_eq!(job.failed.len(), 1);
        assert!(
            job.failed[0].1.contains("nested more than"),
            "{:?}",
            job.failed
        );
        assert_eq!(
            std::fs::read(d.join("out/shallow/a/b/f.txt")).unwrap(),
            b"f"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn a_dangling_link_at_the_target_is_not_replaced_by_a_move() {
        let d = scratch("movelink");
        std::fs::create_dir_all(d.join("dst")).unwrap();
        std::fs::write(d.join("f.txt"), "mine").unwrap();
        std::os::unix::fs::symlink(d.join("nowhere"), d.join("dst/f.txt")).unwrap();
        assert!(move_into(&d.join("f.txt"), &d.join("dst")).is_err());
        assert_eq!(std::fs::read(d.join("f.txt")).unwrap(), b"mine");
        assert!(
            std::fs::symlink_metadata(d.join("dst/f.txt"))
                .unwrap()
                .is_symlink()
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn links_are_skipped_and_counted_never_followed() {
        let d = scratch("links");
        std::fs::create_dir_all(d.join("outside")).unwrap();
        std::fs::write(d.join("outside/secret.txt"), "secret").unwrap();
        std::fs::create_dir_all(d.join("src/sub")).unwrap();
        std::fs::write(d.join("src/real.txt"), "real").unwrap();
        let link = std::os::unix::fs::symlink;
        link(d.join("outside"), d.join("src/dirlink")).unwrap();
        link(d.join("outside/secret.txt"), d.join("src/sub/filelink")).unwrap();
        link(d.join("src"), d.join("src/sub/loop")).unwrap();
        link(d.join("nowhere"), d.join("src/dangling")).unwrap();
        link(d.join("outside"), d.join("toplink")).unwrap();
        std::fs::create_dir_all(d.join("out")).unwrap();
        let job = copy_all(
            &[
                (d.join("src"), d.join("out")),
                (d.join("toplink"), d.join("out")),
            ],
            // A followed loop would run into these.
            CopyLimits {
                entries: 50,
                bytes: 1000,
            },
        )
        .unwrap();
        assert!(job.failed.is_empty(), "{:?}", job.failed);
        assert_eq!(job.skipped, 5);
        assert_eq!(job.skipped_names.len(), 3);
        assert_eq!(job.done, vec![d.join("out/src")]);
        assert_eq!(
            snapshot(&d.join("out")),
            vec![(PathBuf::from("src/real.txt"), b"real".to_vec())]
        );
        let names: Vec<_> = std::fs::read_dir(d.join("out/src"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(names.len(), 2, "only real.txt and sub: {names:?}");
        assert_eq!(std::fs::read_dir(d.join("out/src/sub")).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_test_recycle_folder_keeps_what_was_deleted() {
        let d = scratch("bin");
        std::fs::create_dir_all(d.join("bin")).unwrap();
        std::fs::create_dir_all(d.join("dir")).unwrap();
        std::fs::write(d.join("a.txt"), "one").unwrap();
        std::fs::write(d.join("dir/a.txt"), "two").unwrap();
        set_test_recycle_dir(Some(d.join("bin")));
        move_to_recycle_bin(&d.join("a.txt")).unwrap();
        move_to_recycle_bin(&d.join("dir/a.txt")).unwrap();
        assert!(
            move_to_recycle_bin(&d.join("a.txt")).is_err(),
            "gone already"
        );
        set_test_recycle_dir(None);
        let kept: Vec<Vec<u8>> = snapshot(&d.join("bin")).into_iter().map(|e| e.1).collect();
        assert_eq!(kept, vec![b"one".to_vec(), b"two".to_vec()]);
        assert!(!d.join("a.txt").exists() && !d.join("dir/a.txt").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn name_validation() {
        for ok in ["notes.md", "My File (2).txt", "über.md", ".gitignore"] {
            assert!(validate_name(ok).is_ok(), "{ok}");
        }
        for bad in [
            "", "  ", ".", "..", "a/b", "a\\b", "c:x", "what?", "bad.", "bad ", "CON", "nul.txt",
            "a\u{0}b", "x|y",
        ] {
            assert!(validate_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn create_and_rename_never_overwrite() {
        let d = std::env::temp_dir().join(format!("litedit-fsops-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let a = create_file(&d, "a.md").unwrap();
        std::fs::write(&a, "keep").unwrap();
        assert!(create_file(&d, "a.md").is_err());
        let b = create_file(&d, "b.md").unwrap();
        assert!(rename(&b, "a.md").is_err(), "rename must not overwrite");
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "keep");
        let c = rename(&b, "c.md").unwrap();
        assert!(c.exists() && !b.exists());
        let f = create_folder(&d, "sub").unwrap();
        assert!(f.is_dir());
        assert!(create_folder(&d, "sub").is_err());
        assert!(
            rename(&c, "../escape.md").is_err(),
            "names are a single component"
        );
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn case_only_rename_never_overwrites_another_file() {
        let d = std::env::temp_dir().join(format!("litedit-case-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        // Probe the folder with separate files first.
        std::fs::write(d.join("probe"), "x").unwrap();
        let case_sensitive = !d.join("PROBE").exists();
        std::fs::remove_file(d.join("probe")).unwrap();
        std::fs::write(d.join("a.md"), "lower").unwrap();
        if case_sensitive {
            std::fs::write(d.join("A.md"), "upper").unwrap();
            // Two distinct files: renaming one onto the other is refused.
            assert!(rename(&d.join("a.md"), "A.md").is_err());
            assert_eq!(std::fs::read_to_string(d.join("A.md")).unwrap(), "upper");
            std::fs::remove_file(d.join("A.md")).unwrap();
        }
        // Same file, new case: allowed (Windows-style rename).
        let new = rename(&d.join("a.md"), "A.md").unwrap();
        assert_eq!(std::fs::read_to_string(&new).unwrap(), "lower");
        // Non-ASCII case change of the same file.
        std::fs::write(d.join("é.md"), "e").unwrap();
        assert!(rename(&d.join("é.md"), "É.md").is_ok());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn move_into_never_overwrites_or_nests() {
        let d = std::env::temp_dir().join(format!("litedit-mv-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("a/inner")).unwrap();
        std::fs::create_dir_all(d.join("b")).unwrap();
        std::fs::write(d.join("f.txt"), "one").unwrap();
        std::fs::write(d.join("b/f.txt"), "two").unwrap();
        assert!(
            move_into(&d.join("f.txt"), &d.join("b")).is_err(),
            "would overwrite"
        );
        assert_eq!(std::fs::read_to_string(d.join("b/f.txt")).unwrap(), "two");
        assert!(
            move_into(&d.join("a"), &d.join("a/inner")).is_err(),
            "into itself"
        );
        assert!(move_into(&d.join("a"), &d.join("a")).is_err());
        let moved = move_into(&d.join("f.txt"), &d.join("a")).unwrap();
        assert_eq!(moved, d.join("a/f.txt"));
        assert_eq!(std::fs::read_to_string(&moved).unwrap(), "one");
        let moved = move_into(&d.join("a"), &d.join("b")).unwrap();
        assert!(moved.join("inner").is_dir() && moved.join("f.txt").is_file());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn recycle_refuses_relative_and_missing_paths() {
        assert!(move_to_recycle_bin(Path::new("relative.txt")).is_err());
        assert!(
            move_to_recycle_bin(&std::env::temp_dir().join("litedit-definitely-missing-xyz"))
                .is_err()
        );
    }

    /// Number of items in the Recycle Bin for the drive holding `p`.
    #[cfg(windows)]
    fn recycle_bin_items(p: &Path) -> i64 {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::UI::Shell::{SHQUERYRBINFO, SHQueryRecycleBinW};
        let root: String = p.to_string_lossy().chars().take(3).collect(); // "C:\"
        let w: Vec<u16> = std::ffi::OsStr::new(&root)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut info = SHQUERYRBINFO {
            cbSize: std::mem::size_of::<SHQUERYRBINFO>() as u32,
            i64Size: 0,
            i64NumItems: 0,
        };
        // SAFETY: valid NUL-terminated root path and a correctly sized struct.
        let hr = unsafe { SHQueryRecycleBinW(w.as_ptr(), &mut info) };
        assert_eq!(hr, 0, "SHQueryRecycleBinW failed");
        info.i64NumItems
    }

    /// The file must land in the Recycle Bin — not be deleted permanently.
    #[cfg(windows)]
    #[test]
    fn recycle_puts_the_file_in_the_recycle_bin() {
        let d = std::env::temp_dir().join(format!("litedit-recycle-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("trash-me.txt");
        std::fs::write(&p, "x").unwrap();
        let before = recycle_bin_items(&p);
        move_to_recycle_bin(&p).unwrap();
        assert!(!p.exists());
        let after = recycle_bin_items(&d);
        assert_eq!(after, before + 1, "the file should be in the Recycle Bin");
        std::fs::remove_dir_all(&d).ok();
    }
}
