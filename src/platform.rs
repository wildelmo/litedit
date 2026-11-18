//! The only places litedit hands something to the operating system.
//!
//! URLs go to `ShellExecuteW` directly — never through `cmd /c start`, where
//! characters like `&` or `^` in a crafted URL could run commands.

use std::ffi::OsStr;
use std::path::Path;

#[cfg(windows)]
use std::path::PathBuf;

use crate::links::{LinkTarget, classify};

/// Open an http/https/mailto URL in the user's default browser / mail app.
/// The URL is re-validated here so no other caller can bypass the check.
pub fn open_url(url: &str) -> Result<(), String> {
    match classify(url, Path::new("")) {
        LinkTarget::Web(u) if u == url => shell_open(OsStr::new(url), None),
        _ => Err(format!("refusing to open {url}")),
    }
}

/// Show a file selected in Windows Explorer.
pub fn reveal_in_explorer(path: &Path) -> Result<(), String> {
    let p = path.to_string_lossy();
    if crate::links::is_network_path(&p) || p.contains('"') {
        return Err("refusing to reveal this path".into());
    }
    // Paths cannot contain '"' on Windows, so quoting is unambiguous.
    reveal_selected(&format!("/select,\"{p}\""))
}

#[cfg(windows)]
fn reveal_selected(params: &str) -> Result<(), String> {
    let explorer = windows_explorer_path()?;
    shell_open(explorer.as_os_str(), Some(params))
}

#[cfg(not(windows))]
fn reveal_selected(_params: &str) -> Result<(), String> {
    // Development builds on other platforms: do nothing.
    Ok(())
}

#[cfg(windows)]
fn windows_explorer_path() -> Result<PathBuf, String> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::SystemInformation::GetWindowsDirectoryW;

    let mut directory = vec![0u16; 260];
    loop {
        let capacity = u32::try_from(directory.len())
            .map_err(|_| "Windows directory path is too long")?;
        // SAFETY: `directory` is writable for `capacity` UTF-16 code units.
        let len = unsafe { GetWindowsDirectoryW(directory.as_mut_ptr(), capacity) } as usize;
        if len == 0 {
            return Err("GetWindowsDirectoryW failed".into());
        }
        if len >= directory.len() {
            let capacity = len
                .checked_add(1)
                .ok_or("Windows directory path is too long")?;
            directory.resize(capacity, 0);
            continue;
        }

        let mut explorer = PathBuf::from(OsString::from_wide(&directory[..len]));
        if !explorer.is_absolute() {
            return Err("Windows directory is not absolute".into());
        }
        explorer.push("explorer.exe");
        return Ok(explorer);
    }
}

#[cfg(windows)]
fn shell_open(file: &OsStr, params: Option<&str>) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let op: Vec<u16> = "open".encode_utf16().chain(std::iter::once(0)).collect();
    let file_w: Vec<u16> = file.encode_wide().chain(std::iter::once(0)).collect();
    let params_w = params.map(|p| {
        p.encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>()
    });
    // SAFETY: all pointers are valid, NUL-terminated UTF-16 buffers that
    // outlive the call; a null HWND and null directory are permitted.
    let r = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            op.as_ptr(),
            file_w.as_ptr(),
            params_w.as_ref().map_or(std::ptr::null(), |p| p.as_ptr()),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecuteW returns a value > 32 on success.
    if r as usize > 32 {
        Ok(())
    } else {
        Err(format!("ShellExecuteW failed ({})", r as usize))
    }
}

#[cfg(not(windows))]
fn shell_open(_file: &OsStr, _params: Option<&str>) -> Result<(), String> {
    // Development builds on other platforms: do nothing.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_web_urls_are_opened() {
        assert!(open_url("https://example.com").is_ok());
        assert!(open_url("javascript:alert(1)").is_err());
        assert!(open_url("C:\\Windows\\System32\\calc.exe").is_err());
        assert!(open_url("https://x.com/\"&calc").is_err());
        assert!(open_url("file:///C:/x").is_err());
    }

    #[test]
    fn network_paths_are_not_revealed() {
        assert!(reveal_in_explorer(Path::new("\\\\server\\share\\x")).is_err());
    }
}
