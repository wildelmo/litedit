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
    // Allow case-only renames on Windows (same file, different case).
    let case_only = target
        .to_string_lossy()
        .eq_ignore_ascii_case(&path.to_string_lossy());
    if target.exists() && !case_only {
        return Err("something with that name already exists".into());
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
    recycle(path)
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn recycle_refuses_relative_and_missing_paths() {
        assert!(move_to_recycle_bin(Path::new("relative.txt")).is_err());
        assert!(
            move_to_recycle_bin(&std::env::temp_dir().join("litedit-definitely-missing-xyz"))
                .is_err()
        );
    }

    #[cfg(windows)]
    #[test]
    fn recycle_removes_the_file() {
        let d = std::env::temp_dir().join(format!("litedit-recycle-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("trash-me.txt");
        std::fs::write(&p, "x").unwrap();
        move_to_recycle_bin(&p).unwrap();
        assert!(!p.exists());
        std::fs::remove_dir_all(&d).ok();
    }
}
