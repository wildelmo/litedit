//! Adding litedit to Windows' "Open with" list for markdown files (D-006:
//! only when the user asks, never silently).
//!
//! Windows does not let a program make itself the *default* app (that
//! choice is protected and only the user can make it), so this registers
//! litedit as a choice: afterwards "Open with → litedit → Always" in
//! Explorer makes it the default. Only the current user's settings
//! (`HKEY_CURRENT_USER\Software\Classes`) are touched, and "Remove" deletes
//! exactly what "Add" wrote.

/// File types offered.
pub const EXTENSIONS: &[&str] = &[".md", ".markdown"];
const PROG_ID: &str = "litedit.md";
const APP_KEY: &str = r"Software\Classes\Applications\litedit.exe";

/// One registry string value under HKEY_CURRENT_USER: (key, name, data).
/// An empty name is the key's default value.
pub type Entry = (String, String, String);

/// Everything "Add" writes for the program at `exe`. None if the path
/// can't be quoted safely in a command line.
pub fn entries(exe: &str) -> Option<Vec<Entry>> {
    // `%` would be expanded by the shell inside the command line.
    if exe.is_empty() || exe.contains(['"', '%', '\n', '\r', '\0']) || exe.len() > 1024 {
        return None;
    }
    let command = format!("\"{exe}\" \"%1\"");
    let class = format!(r"Software\Classes\{PROG_ID}");
    let mut v = vec![
        (
            class.clone(),
            String::new(),
            "Markdown document".to_string(),
        ),
        (
            format!(r"{class}\DefaultIcon"),
            String::new(),
            format!("{exe},0"),
        ),
        (
            format!(r"{class}\shell\open\command"),
            String::new(),
            command.clone(),
        ),
        (
            APP_KEY.to_string(),
            "FriendlyAppName".to_string(),
            "litedit".to_string(),
        ),
        (
            format!(r"{APP_KEY}\shell\open\command"),
            String::new(),
            command,
        ),
    ];
    for ext in EXTENSIONS {
        v.push((
            format!(r"Software\Classes\{ext}\OpenWithProgids"),
            PROG_ID.to_string(),
            String::new(),
        ));
        v.push((
            format!(r"{APP_KEY}\SupportedTypes"),
            ext.to_string(),
            String::new(),
        ));
    }
    Some(v)
}

/// Keys "Remove" deletes entirely, and single values it deletes.
pub fn removals() -> (Vec<String>, Vec<(String, String)>) {
    (
        vec![format!(r"Software\Classes\{PROG_ID}"), APP_KEY.to_string()],
        EXTENSIONS
            .iter()
            .map(|ext| {
                (
                    format!(r"Software\Classes\{ext}\OpenWithProgids"),
                    PROG_ID.to_string(),
                )
            })
            .collect(),
    )
}

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Add litedit (the running exe) to "Open with" for markdown files.
#[cfg(windows)]
pub fn register() -> Result<String, String> {
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
        RegCreateKeyExW, RegSetValueExW,
    };
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = exe.to_string_lossy().into_owned();
    let list = entries(&exe).ok_or("the program's path can't be registered")?;
    for (key, name, data) in list {
        let (k, n, d) = (wide(&key), wide(&name), wide(&data));
        let mut h: HKEY = std::ptr::null_mut();
        // SAFETY: valid NUL-terminated strings; `h` is closed below.
        let rc = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                k.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                std::ptr::null(),
                &mut h,
                std::ptr::null_mut(),
            )
        };
        if rc != 0 {
            return Err(format!("could not write {key} (error {rc})"));
        }
        // SAFETY: `d` holds `d.len()` UTF-16 units including the NUL.
        let rc = unsafe {
            let rc = RegSetValueExW(
                h,
                if name.is_empty() {
                    std::ptr::null()
                } else {
                    n.as_ptr()
                },
                0,
                REG_SZ,
                d.as_ptr().cast(),
                (d.len() * 2) as u32,
            );
            RegCloseKey(h);
            rc
        };
        if rc != 0 {
            return Err(format!("could not write {key} (error {rc})"));
        }
    }
    notify_shell();
    Ok(exe)
}

/// Remove what [`register`] wrote.
#[cfg(windows)]
pub fn unregister() -> Result<(), String> {
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, RegDeleteKeyValueW, RegDeleteTreeW,
    };
    let (keys, values) = removals();
    for key in keys {
        let k = wide(&key);
        // SAFETY: valid NUL-terminated string. "Not found" is fine.
        unsafe {
            RegDeleteTreeW(HKEY_CURRENT_USER, k.as_ptr());
        }
    }
    for (key, name) in values {
        let (k, n) = (wide(&key), wide(&name));
        // SAFETY: valid NUL-terminated strings. "Not found" is fine.
        unsafe {
            RegDeleteKeyValueW(HKEY_CURRENT_USER, k.as_ptr(), n.as_ptr());
        }
    }
    notify_shell();
    Ok(())
}

/// Tell Explorer the "Open with" lists changed.
#[cfg(windows)]
fn notify_shell() {
    use windows_sys::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
    // SAFETY: no pointers are passed.
    unsafe {
        SHChangeNotify(
            SHCNE_ASSOCCHANGED as i32,
            SHCNF_IDLIST,
            std::ptr::null(),
            std::ptr::null(),
        );
    }
}

#[cfg(not(windows))]
pub fn register() -> Result<String, String> {
    Err("only available on Windows".into())
}

#[cfg(not(windows))]
pub fn unregister() -> Result<(), String> {
    Err("only available on Windows".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_quote_the_exe_and_stay_under_the_user_classes() {
        let exe = r"C:\Program Files\litedit\litedit.exe";
        let e = entries(exe).unwrap();
        assert!(
            e.iter()
                .all(|(k, _, _)| k.starts_with(r"Software\Classes\"))
        );
        let cmd = e
            .iter()
            .find(|(k, _, _)| k == r"Software\Classes\litedit.md\shell\open\command")
            .map(|(_, _, d)| d.clone())
            .unwrap();
        assert_eq!(cmd, r#""C:\Program Files\litedit\litedit.exe" "%1""#);
        for ext in EXTENSIONS {
            assert!(
                e.iter()
                    .any(|(k, n, _)| k.ends_with(&format!(r"{ext}\OpenWithProgids"))
                        && n == "litedit.md")
            );
        }
        // A path that could break out of the quotes is refused.
        assert!(entries("C:\\a\" & calc & \"\\litedit.exe").is_none());
        assert!(entries("").is_none());
        assert!(entries(r"C:\tools\%L\litedit.exe").is_none());
    }

    #[test]
    fn remove_covers_everything_add_writes() {
        let e = entries(r"C:\x\litedit.exe").unwrap();
        let (keys, values) = removals();
        for (k, n, _) in e {
            let under_key = keys
                .iter()
                .any(|d| k == *d || k.starts_with(&format!(r"{d}\")));
            let as_value = values.iter().any(|(vk, vn)| *vk == k && *vn == n);
            assert!(under_key || as_value, "{k} / {n} would be left behind");
        }
        // And it never deletes a key it doesn't own (e.g. `.md` itself).
        assert!(keys.iter().all(|k| k.contains("litedit")));
    }
}
