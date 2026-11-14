//! The only places litedit hands something to the operating system.
//!
//! URLs go to `ShellExecuteW` directly — never through `cmd /c start`, where
//! characters like `&` or `^` in a crafted URL could run commands.

use std::path::Path;

use crate::links::{LinkTarget, classify};

/// Open an http/https/mailto URL in the user's default browser / mail app.
/// The URL is re-validated here so no other caller can bypass the check.
pub fn open_url(url: &str) -> Result<(), String> {
    match classify(url, Path::new("")) {
        LinkTarget::Web(u) if u == url => shell_open(url, None),
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
    shell_open("explorer.exe", Some(&format!("/select,\"{p}\"")))
}

#[cfg(windows)]
fn shell_open(file: &str, params: Option<&str>) -> Result<(), String> {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }
    let op = wide("open");
    let file_w = wide(file);
    let params_w = params.map(wide);
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
fn shell_open(_file: &str, _params: Option<&str>) -> Result<(), String> {
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
