//! Writing files safely.
//!
//! 1. Write the new bytes to a temporary file in the same folder and flush it.
//!    For an existing Windows target, keep the creation handle exclusive while
//!    copying and protecting its DACL, then write the bytes.
//! 2. Swap it in:
//!    - Windows, existing file: `ReplaceFileW`, which keeps the original's
//!      ACLs, attributes and creation time. If another program holds the file
//!      open so it cannot be replaced, fall back to overwriting in place.
//!    - New file (or other platforms): rename over the target.
//! 3. On any failure, a temporary file created by this save is removed and an
//!    error is returned; the caller keeps the buffer dirty.
//!
//! Symlinks are followed so the link stays a link. Read-only files and
//! Windows device names are refused.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::doc::is_reserved_name;

#[cfg(windows)]
mod bound;
#[cfg(windows)]
pub(crate) use bound::{SaveTarget, save_as_bound};

/// Where a save to `path` should actually write (follows symlinks).
fn resolve_target(path: &Path) -> PathBuf {
    // Follow a chain of links (a → b → file) to the real file, so the
    // links themselves stay links. Bounded, in case of a loop.
    let mut cur = path.to_path_buf();
    for _ in 0..32 {
        match fs::symlink_metadata(&cur) {
            Ok(m) if m.file_type().is_symlink() => match fs::read_link(&cur) {
                Ok(t) if t.is_absolute() => cur = t,
                Ok(t) => cur = cur.parent().map(|p| p.join(&t)).unwrap_or(t),
                Err(_) => return cur,
            },
            _ => return cur,
        }
    }
    cur
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
    // Keep the temp name short: long file names plus the suffix could pass
    // the 255-character limit and make saving impossible.
    let short: String = name.chars().take(40).collect();
    dir.join(format!(
        ".{short}.litedit-{}-{stamp}.tmp",
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
    match write_and_swap(&target, &tmp, bytes, existing.as_ref()) {
        Ok(()) => Ok(()),
        // The target is untouched; write_and_swap removed any temp it created.
        Err(Failure {
            msg,
            keep_temp: false,
        }) => Err(msg),
        // The target may be damaged or gone: never delete the only good copy.
        Err(Failure {
            msg,
            keep_temp: true,
        }) => Err(format!(
            "{msg}. A copy of your text was kept at {}",
            tmp.display()
        )),
    }
}

struct Failure {
    msg: String,
    /// True when the temp file may be the only intact copy of the new text.
    keep_temp: bool,
}

fn fail(msg: impl Into<String>) -> Failure {
    Failure {
        msg: msg.into(),
        keep_temp: false,
    }
}

/// A temporary file owned by this save. Cleanup is armed only after
/// `create_new` succeeds, so a path that already collided is not removed.
struct CreatedTemp<'a> {
    path: &'a Path,
    file: Option<fs::File>,
    keep: bool,
}

impl<'a> CreatedTemp<'a> {
    fn new(path: &'a Path, file: fs::File) -> Self {
        Self {
            path,
            file: Some(file),
            keep: false,
        }
    }

    fn file_mut(&mut self) -> &mut fs::File {
        self.file.as_mut().expect("temporary file is open")
    }

    fn close(&mut self) {
        drop(self.file.take());
    }

    fn keep(&mut self) {
        self.keep = true;
    }
}

impl Drop for CreatedTemp<'_> {
    fn drop(&mut self) {
        self.close();
        if !self.keep {
            let _ = fs::remove_file(self.path);
        }
    }
}

/// The access list of an existing Windows target. The descriptor owns the
/// allocation that `dacl` points into.
#[cfg(windows)]
struct WindowsDacl {
    descriptor: windows_sys::Win32::Security::PSECURITY_DESCRIPTOR,
    dacl: *mut windows_sys::Win32::Security::ACL,
}

#[cfg(windows)]
impl WindowsDacl {
    fn read(target: &Path) -> std::io::Result<Self> {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, READ_CONTROL,
        };

        // Ask only for READ_CONTROL so this does not conflict with programs
        // that share file data but prevent replacement.
        let source = fs::OpenOptions::new()
            .read(true)
            .access_mode(READ_CONTROL)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .open(target)?;
        Self::from_file(&source)
    }

    fn from_file(source: &fs::File) -> std::io::Result<Self> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::{ERROR_SUCCESS, HANDLE};
        use windows_sys::Win32::Security::Authorization::{GetSecurityInfo, SE_FILE_OBJECT};
        use windows_sys::Win32::Security::{DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR};

        let mut dacl = std::ptr::null_mut();
        let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        // SAFETY: `source` is a live file handle with READ_CONTROL, and all
        // requested output pointers are valid. Windows allocates descriptor;
        // dacl points inside it and remains valid until this value is dropped.
        let err = unsafe {
            GetSecurityInfo(
                source.as_raw_handle() as HANDLE,
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut dacl,
                std::ptr::null_mut(),
                &mut descriptor,
            )
        };
        let result = Self { descriptor, dacl };
        if err != ERROR_SUCCESS {
            return Err(std::io::Error::from_raw_os_error(err as i32));
        }
        if result.descriptor.is_null() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Windows returned an empty security descriptor",
            ));
        }
        Ok(result)
    }

    fn apply_to(&self, file: &fs::File) -> std::io::Result<()> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::{ERROR_SUCCESS, HANDLE};
        use windows_sys::Win32::Security::Authorization::{SE_FILE_OBJECT, SetSecurityInfo};
        use windows_sys::Win32::Security::{
            DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
        };

        // Protect the copied DACL so the directory cannot merge weaker
        // inherited entries into the temporary file before it is populated.
        // SAFETY: `file` is a live handle opened with WRITE_DAC; `dacl`
        // points inside the live descriptor owned by `self`.
        let err = unsafe {
            SetSecurityInfo(
                file.as_raw_handle() as HANDLE,
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                self.dacl,
                std::ptr::null_mut(),
            )
        };
        if err == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(std::io::Error::from_raw_os_error(err as i32))
        }
    }
}

#[cfg(windows)]
impl Drop for WindowsDacl {
    fn drop(&mut self) {
        if !self.descriptor.is_null() {
            // SAFETY: GetSecurityInfo allocated this descriptor with
            // LocalAlloc, and this value is its only owner.
            unsafe {
                windows_sys::Win32::Foundation::LocalFree(self.descriptor);
            }
        }
    }
}

fn write_and_swap(
    target: &Path,
    tmp: &Path,
    bytes: &[u8],
    existing: Option<&fs::Metadata>,
) -> Result<(), Failure> {
    #[cfg(windows)]
    let target_dacl = if existing.is_some() {
        Some(WindowsDacl::read(target).map_err(|e| {
            fail(format!(
                "cannot preserve the file's access permissions: {e}"
            ))
        })?)
    } else {
        None
    };
    {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(windows)]
        if target_dacl.is_some() {
            use std::os::windows::fs::OpenOptionsExt;
            use windows_sys::Win32::Foundation::GENERIC_WRITE;
            use windows_sys::Win32::Storage::FileSystem::WRITE_DAC;
            // The inherited directory DACL exists briefly at creation. Deny
            // all sharing so no other process can obtain a handle during that
            // interval; the protected target DACL is installed before bytes
            // are written and remains in force after this handle is closed.
            options.access_mode(GENERIC_WRITE | WRITE_DAC).share_mode(0);
        }
        let f = options
            .open(tmp)
            .map_err(|e| fail(format!("cannot write in this folder: {e}")))?;
        let mut temp = CreatedTemp::new(tmp, f);
        #[cfg(windows)]
        if let Some(dacl) = &target_dacl {
            dacl.apply_to(temp.file_mut()).map_err(|e| {
                fail(format!(
                    "cannot preserve the file's access permissions: {e}"
                ))
            })?;
        }
        temp.file_mut()
            .write_all(bytes)
            .map_err(|e| fail(format!("write failed: {e}")))?;
        temp.file_mut()
            .sync_all()
            .map_err(|e| fail(format!("write failed: {e}")))?;
        temp.close();

        if existing.is_none() {
            return fs::rename(tmp, target)
                .map_err(|e| fail(format!("cannot create the file: {e}")));
        }
        #[cfg(windows)]
        {
            if replace_file(target, tmp).is_ok() {
                return Ok(());
            }
            // ReplaceFileW can fail *after* moving the original away (errors
            // 1176/1177). If the target is gone, put the new copy in its place.
            if !target.exists() {
                return match fs::rename(tmp, target) {
                    Ok(()) => Ok(()),
                    Err(e) => {
                        temp.keep();
                        Err(Failure {
                            msg: format!(
                                "the original file was moved away and the new copy could not be put in place: {e}"
                            ),
                            keep_temp: true,
                        })
                    }
                };
            }
            // Target intact but could not be replaced (held open by another
            // program): overwrite in place, keeping the temp copy until the new
            // bytes are verified on disk.
            if let Err(err) = overwrite_in_place(target, bytes) {
                if err.keep_temp {
                    temp.keep();
                }
                return Err(err);
            }
            Ok(())
        }
        #[cfg(not(windows))]
        {
            if let Some(m) = existing {
                let _ = fs::set_permissions(tmp, m.permissions());
            }
            fs::rename(tmp, target).map_err(|e| fail(format!("cannot replace the file: {e}")))
        }
    }
}

/// Overwrite the file's contents directly. Opening first (before any
/// truncation) means a locked file is reported untouched; once truncated,
/// any failure keeps the temp copy, and success is verified by reading back.
#[cfg(windows)]
fn overwrite_in_place(target: &Path, bytes: &[u8]) -> Result<(), Failure> {
    let mut f = fs::OpenOptions::new()
        .write(true)
        .open(target)
        .map_err(|e| fail(format!("the file is in use or locked: {e}")))?;
    let damaged = |e: std::io::Error| Failure {
        msg: format!("write failed: {e}"),
        keep_temp: true,
    };
    f.set_len(0).map_err(damaged)?;
    f.write_all(bytes).map_err(damaged)?;
    f.sync_all().map_err(damaged)?;
    drop(f);
    match fs::read(target) {
        Ok(back) if back == bytes => Ok(()),
        _ => Err(Failure {
            msg: "the saved file could not be verified".into(),
            keep_temp: true,
        }),
    }
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

    #[test]
    fn very_long_file_names_can_be_saved() {
        let d = std::env::temp_dir().join(format!("litedit-longname-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join(format!("{}.txt", "n".repeat(240)));
        std::fs::write(&f, "old").unwrap();
        save_bytes(&f, b"new").unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "new");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn saving_through_a_chain_of_links_writes_the_real_file() {
        let d = std::env::temp_dir().join(format!("litedit-chain-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("real.txt"), "old").unwrap();
        std::os::unix::fs::symlink(d.join("real.txt"), d.join("mid")).unwrap();
        std::os::unix::fs::symlink("mid", d.join("top")).unwrap();
        save_bytes(&d.join("top"), b"new").unwrap();
        assert_eq!(std::fs::read_to_string(d.join("real.txt")).unwrap(), "new");
        assert!(
            std::fs::symlink_metadata(d.join("mid"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(
            std::fs::symlink_metadata(d.join("top"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        let _ = std::fs::remove_dir_all(&d);
    }

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

    #[test]
    fn failed_create_keeps_unowned_temp() {
        let d = dir("unowned-temp");
        let target = d.join("new.txt");
        let tmp = d.join(".collision.litedit-temp");
        fs::write(&tmp, b"not ours").unwrap();

        let err = write_and_swap(&target, &tmp, b"new", None).unwrap_err();

        assert!(!err.keep_temp);
        assert_eq!(fs::read(&tmp).unwrap(), b"not ours");
        assert!(!target.exists());
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
