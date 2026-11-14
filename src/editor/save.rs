//! Writing files safely.
//!
//! 1. Write the new bytes to a temporary file in the same folder and flush it.
//! 2. Swap it in:
//!    - Windows, existing file: `ReplaceFileW`, which keeps the original's
//!      ACLs, attributes and creation time. If another program holds the file
//!      open so it cannot be replaced, fall back to overwriting in place.
//!    - New file (or other platforms): rename over the target.
//! 3. On any failure the temporary file is removed and an error is returned;
//!    the caller keeps the buffer dirty.
//!
//! Symlinks are followed so the link stays a link. Read-only files and
//! Windows device names are refused.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::doc::is_reserved_name;

/// Where a save to `path` should actually write (follows symlinks).
fn resolve_target(path: &Path) -> PathBuf {
    match fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => match fs::read_link(path) {
            Ok(t) if t.is_absolute() => t,
            Ok(t) => path.parent().map(|p| p.join(&t)).unwrap_or(t),
            Err(_) => path.to_path_buf(),
        },
        _ => path.to_path_buf(),
    }
}

fn temp_path(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    let dir = target.parent().unwrap_or(Path::new("."));
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    dir.join(format!(
        ".{name}.litedit-{}-{stamp}.tmp",
        std::process::id()
    ))
}

pub fn save_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if is_reserved_name(path) {
        return Err("that name is reserved by Windows".into());
    }
    let target = resolve_target(path);
    let existing = fs::metadata(&target).ok();
    if let Some(m) = &existing {
        if !m.is_file() {
            return Err("not a regular file".into());
        }
        if m.permissions().readonly() {
            return Err("the file is read-only".into());
        }
    }
    let tmp = temp_path(&target);
    let result = write_and_swap(&target, &tmp, bytes, existing.as_ref());
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn write_and_swap(
    target: &Path,
    tmp: &Path,
    bytes: &[u8],
    existing: Option<&fs::Metadata>,
) -> Result<(), String> {
    {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(tmp)
            .map_err(|e| format!("cannot write in this folder: {e}"))?;
        f.write_all(bytes)
            .map_err(|e| format!("write failed: {e}"))?;
        f.sync_all().map_err(|e| format!("write failed: {e}"))?;
    }
    if existing.is_none() {
        return fs::rename(tmp, target).map_err(|e| format!("cannot create the file: {e}"));
    }
    #[cfg(windows)]
    {
        if replace_file(target, tmp).is_ok() {
            return Ok(());
        }
        // Held open by another program (or replace not supported here):
        // overwrite in place, then drop the temp copy.
        overwrite_in_place(target, bytes)?;
        let _ = fs::remove_file(tmp);
        Ok(())
    }
    #[cfg(not(windows))]
    {
        if let Some(m) = existing {
            let _ = fs::set_permissions(tmp, m.permissions());
        }
        fs::rename(tmp, target).map_err(|e| format!("cannot replace the file: {e}"))
    }
}

#[cfg(windows)]
fn overwrite_in_place(target: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut f = fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(target)
        .map_err(|e| format!("the file is in use or locked: {e}"))?;
    f.write_all(bytes)
        .map_err(|e| format!("write failed: {e}"))?;
    f.sync_all().map_err(|e| format!("write failed: {e}"))
}

#[cfg(windows)]
fn replace_file(target: &Path, replacement: &Path) -> Result<(), u32> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::Storage::FileSystem::{
        REPLACEFILE_IGNORE_ACL_ERRORS, REPLACEFILE_IGNORE_MERGE_ERRORS, ReplaceFileW,
    };
    fn wide(p: &Path) -> Vec<u16> {
        p.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }
    let (t, r) = (wide(target), wide(replacement));
    // SAFETY: both buffers are valid NUL-terminated UTF-16 paths that live
    // across the call; backup/exclude/reserved may be null.
    let ok = unsafe {
        ReplaceFileW(
            t.as_ptr(),
            r.as_ptr(),
            std::ptr::null(),
            REPLACEFILE_IGNORE_MERGE_ERRORS | REPLACEFILE_IGNORE_ACL_ERRORS,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if ok != 0 {
        Ok(())
    } else {
        Err(unsafe { GetLastError() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("litedit-save-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn leftovers(d: &Path) -> Vec<String> {
        fs::read_dir(d)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".litedit-"))
            .collect()
    }

    #[test]
    fn creates_and_replaces() {
        let d = dir("basic");
        let p = d.join("a.txt");
        save_bytes(&p, b"one").unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"one");
        save_bytes(&p, b"two\r\n").unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"two\r\n");
        assert!(leftovers(&d).is_empty());
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn read_only_is_refused_and_untouched() {
        let d = dir("ro");
        let p = d.join("ro.txt");
        fs::write(&p, b"keep").unwrap();
        let mut perm = fs::metadata(&p).unwrap().permissions();
        perm.set_readonly(true);
        fs::set_permissions(&p, perm).unwrap();
        let err = save_bytes(&p, b"changed").unwrap_err();
        assert!(err.contains("read-only"), "{err}");
        assert_eq!(fs::read(&p).unwrap(), b"keep");
        assert!(leftovers(&d).is_empty());
        let mut perm = fs::metadata(&p).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perm.set_readonly(false);
        fs::set_permissions(&p, perm).unwrap();
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn reserved_names_and_missing_folders_fail_cleanly() {
        let d = dir("bad");
        assert!(save_bytes(&d.join("CON.txt"), b"x").is_err());
        assert!(save_bytes(&d.join("no/such/dir/a.txt"), b"x").is_err());
        assert!(leftovers(&d).is_empty());
        fs::remove_dir_all(&d).ok();
    }

    /// Another program holds the file open without delete sharing (like many
    /// editors/sync tools): replace fails, overwrite-in-place must succeed.
    #[cfg(windows)]
    #[test]
    fn held_open_without_delete_sharing_still_saves() {
        use std::os::windows::fs::OpenOptionsExt;
        let d = dir("held");
        let p = d.join("held.txt");
        fs::write(&p, b"before").unwrap();
        const SHARE_READ: u32 = 1;
        const SHARE_WRITE: u32 = 2;
        let _h = fs::OpenOptions::new()
            .read(true)
            .share_mode(SHARE_READ | SHARE_WRITE)
            .open(&p)
            .unwrap();
        save_bytes(&p, b"after").unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"after");
        assert!(leftovers(&d).is_empty());
        drop(_h);
        fs::remove_dir_all(&d).ok();
    }

    /// Exclusively locked file: save fails cleanly, original untouched.
    #[cfg(windows)]
    #[test]
    fn exclusively_locked_file_fails_cleanly() {
        use std::os::windows::fs::OpenOptionsExt;
        let d = dir("locked");
        let p = d.join("locked.txt");
        fs::write(&p, b"keep").unwrap();
        const SHARE_READ: u32 = 1;
        let h = fs::OpenOptions::new()
            .read(true)
            .share_mode(SHARE_READ)
            .open(&p)
            .unwrap();
        let err = save_bytes(&p, b"lost?").unwrap_err();
        assert!(err.contains("in use") || err.contains("locked"), "{err}");
        drop(h);
        assert_eq!(fs::read(&p).unwrap(), b"keep");
        assert!(leftovers(&d).is_empty());
        fs::remove_dir_all(&d).ok();
    }

    /// ReplaceFileW keeps file attributes (e.g. hidden).
    #[cfg(windows)]
    #[test]
    fn attributes_are_preserved() {
        use std::os::windows::ffi::OsStrExt;
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_HIDDEN, SetFileAttributesW};
        let d = dir("attrs");
        let p = d.join("hidden.txt");
        fs::write(&p, b"x").unwrap();
        let w: Vec<u16> = p
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: valid NUL-terminated path.
        assert_ne!(
            unsafe { SetFileAttributesW(w.as_ptr(), FILE_ATTRIBUTE_HIDDEN) },
            0
        );
        save_bytes(&p, b"y").unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"y");
        assert_ne!(
            fs::metadata(&p).unwrap().file_attributes() & FILE_ATTRIBUTE_HIDDEN,
            0
        );
        fs::remove_dir_all(&d).ok();
    }

    #[cfg(unix)]
    #[test]
    fn symlink_is_followed_and_kept() {
        let d = dir("link");
        let real = d.join("real.md");
        fs::write(&real, b"old").unwrap();
        let link = d.join("link.md");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        save_bytes(&link, b"new").unwrap();
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read(&real).unwrap(), b"new");
        fs::remove_dir_all(&d).ok();
    }

    #[cfg(unix)]
    #[test]
    fn permissions_are_preserved() {
        use std::os::unix::fs::PermissionsExt;
        let d = dir("perm");
        let p = d.join("x.sh");
        fs::write(&p, b"#!/bin/sh").unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        save_bytes(&p, b"#!/bin/sh\necho hi").unwrap();
        assert_eq!(
            fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o755
        );
        fs::remove_dir_all(&d).ok();
    }
}
