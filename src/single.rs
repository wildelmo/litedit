//! One window: starting litedit with files while it is already running
//! opens them as tabs in the running window (like double-clicking a second
//! document in most editors).
//!
//! How: the first window listens on a local named pipe. A later launch
//! connects, sends its file paths, waits for "ok" and exits. This is local
//! inter-process communication on the same PC, not networking:
//! - the pipe is created with `PIPE_REJECT_REMOTE_CLIENTS` and the default
//!   access list (only the same user, administrators and the system can
//!   write to it);
//! - the name contains a hash of the user and session, so two users on one
//!   PC never share a window;
//! - the message is only a list of absolute file paths. It is parsed by
//!   [`parse`], which is pure, bounded and never panics; anything that is
//!   not an existing regular file on a local path is dropped;
//! - if anything goes wrong (no answer in time, refused), the launch simply
//!   opens its own window. Nothing is ever lost by forwarding.
//!
//! Folders and workspace files are not forwarded (they would switch the
//! running window's workspace): they open a new window.

use std::path::{Path, PathBuf};

const HEADER: &str = "litedit-open 1";
const END: &str = "end";
/// Limits on one message.
pub const MAX_BYTES: usize = 64 * 1024;
pub const MAX_PATHS: usize = 100;
const MAX_PATH_CHARS: usize = 4096;

/// The message a second launch sends: header, one path per line, `end`.
pub fn encode(paths: &[PathBuf]) -> Vec<u8> {
    let mut s = format!("{HEADER}\n");
    for p in paths.iter().take(MAX_PATHS) {
        let t = p.to_string_lossy();
        if !t.contains(['\n', '\r']) {
            s.push_str(&t);
            s.push('\n');
        }
    }
    s.push_str(END);
    s.push('\n');
    s.into_bytes()
}

/// True once `bytes` holds a whole message (or can never become one).
pub fn is_complete(bytes: &[u8]) -> bool {
    bytes.len() >= MAX_BYTES
        || bytes.ends_with(b"\nend\n")
        || bytes.ends_with(b"\nend\r\n")
        || (bytes.len() >= HEADER.len() && !bytes.starts_with(HEADER.as_bytes()))
}

/// Parse a message into the paths it asks to open. Returns None unless it
/// is a complete, well-formed message. Only absolute, local, plain paths
/// survive (no network shares, device paths or reserved names); whether
/// they exist is checked by the caller.
pub fn parse(bytes: &[u8]) -> Option<Vec<PathBuf>> {
    if bytes.len() > MAX_BYTES {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;
    let mut lines = text.lines();
    if lines.next()? != HEADER {
        return None;
    }
    let mut out = Vec::new();
    let mut ended = false;
    for line in lines {
        if line == END {
            ended = true;
            break;
        }
        if line.is_empty() || line.chars().count() > MAX_PATH_CHARS || line.contains('\0') {
            continue;
        }
        let p = PathBuf::from(line);
        if !is_plain_absolute(line, &p) {
            continue;
        }
        if out.len() < MAX_PATHS {
            out.push(p);
        }
    }
    ended.then_some(out)
}

/// Absolute path on a local drive, not a device or network path.
fn is_plain_absolute(text: &str, p: &Path) -> bool {
    if crate::links::is_unsafe_local_path(text) || crate::doc::is_reserved_name(p) {
        return false;
    }
    // `C:\…` on Windows; `/…` elsewhere (tests run on Linux).
    let b = text.as_bytes();
    let drive = b.len() >= 3
        && b[0].is_ascii_alphabetic()
        && b[1] == b':'
        && (b[2] == b'\\' || b[2] == b'/');
    drive || (cfg!(not(windows)) && p.is_absolute())
}

/// Should this launch hand its arguments to a running window? Only when
/// every argument is an existing regular file (not a folder, not a
/// workspace file) and there is at least one.
pub fn forwardable(paths: &[PathBuf]) -> bool {
    !paths.is_empty()
        && paths.len() <= MAX_PATHS
        && paths
            .iter()
            .all(|p| p.is_file() && !crate::workspace::is_workspace_file(p))
        // The running window must accept every path exactly as sent
        // (network shares, `\\?\` paths and odd names are dropped by
        // `parse`): otherwise this launch opens its own window.
        && parse(&encode(paths)).as_deref() == Some(paths)
}

/// The pipe's name for this user and session.
pub fn pipe_name() -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for k in ["USERDOMAIN", "USERNAME", "SESSIONNAME", "USERPROFILE"] {
        std::env::var_os(k).hash(&mut h);
    }
    format!(r"\\.\pipe\litedit-open-{:016x}", h.finish())
}

/// Second launch: try to hand `paths` to a running window. True only if
/// that window confirmed it; on any doubt the caller opens its own window.
#[cfg(windows)]
pub fn try_forward(paths: &[PathBuf]) -> bool {
    use std::io::{Read, Write};
    use std::sync::mpsc::channel;
    use std::time::Duration;
    let name = pipe_name();
    let msg = encode(paths);
    // Let the running window come to the front when it opens the files.
    unsafe {
        // ASFW_ANY
        windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(u32::MAX);
    }
    let (tx, rx) = channel::<bool>();
    // The exchange runs on a helper thread so a stuck server can't hang
    // this launch: no answer in time means "open your own window".
    let spawned = std::thread::Builder::new()
        .name("litedit-forward".into())
        .spawn(move || {
            let ok = (|| -> std::io::Result<bool> {
                use std::os::windows::fs::OpenOptionsExt;
                // SECURITY_IDENTIFICATION: whoever answers on the pipe can
                // see who we are but can never act as us.
                const SECURITY_IDENTIFICATION: u32 = 0x0001_0000;
                const ERROR_PIPE_BUSY: i32 = 231;
                let mut tries = 0;
                let mut pipe = loop {
                    match std::fs::OpenOptions::new()
                        .read(true)
                        .write(true)
                        .security_qos_flags(SECURITY_IDENTIFICATION)
                        .open(&name)
                    {
                        Ok(p) => break p,
                        // Another launch is being served (several files
                        // opened at once from Explorer): wait briefly.
                        Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && tries < 25 => {
                            tries += 1;
                            std::thread::sleep(Duration::from_millis(40));
                        }
                        Err(e) => return Err(e),
                    }
                };
                pipe.write_all(&msg)?;
                pipe.flush()?;
                let mut ack = [0u8; 3];
                pipe.read_exact(&mut ack)?;
                Ok(&ack == b"ok\n")
            })()
            .unwrap_or(false);
            let _ = tx.send(ok);
        });
    if spawned.is_err() {
        return false;
    }
    rx.recv_timeout(Duration::from_millis(1500))
        .unwrap_or(false)
}

#[cfg(not(windows))]
pub fn try_forward(_paths: &[PathBuf]) -> bool {
    false
}

/// First window: listen for later launches and pass their files to `deliver`
/// (called on the listener thread; it should only post to the event loop).
/// Does nothing if another window already listens.
#[cfg(windows)]
pub fn listen(deliver: impl Fn(Vec<PathBuf>) + Send + 'static) {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_FIRST_PIPE_INSTANCE, FlushFileBuffers, PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
    };
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
        PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
    };
    let name: Vec<u16> = pipe_name()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: plain Win32 calls with valid, NUL-terminated arguments; the
    // handle is owned by the listener thread for the life of the process.
    let handle = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            4096,
            MAX_BYTES as u32,
            0,
            std::ptr::null(),
        )
    };
    if handle == INVALID_HANDLE_VALUE || handle.is_null() {
        return; // another window owns the name: this one just doesn't listen
    }
    let handle = handle as usize; // HANDLE isn't Send; the thread owns it
    let spawned = std::thread::Builder::new()
        .name("litedit-single".into())
        .spawn(move || {
            let h = handle as windows_sys::Win32::Foundation::HANDLE;
            loop {
                // SAFETY: `h` is our pipe; buffers are valid for the lengths
                // passed; no overlapped I/O.
                unsafe {
                    // Returns 0 with ERROR_PIPE_CONNECTED if the client got
                    // in first; reading works either way.
                    ConnectNamedPipe(h, std::ptr::null_mut());
                    let mut msg: Vec<u8> = Vec::new();
                    let mut buf = [0u8; 4096];
                    while !is_complete(&msg) {
                        let mut n = 0u32;
                        let ok = ReadFile(
                            h,
                            buf.as_mut_ptr(),
                            buf.len() as u32,
                            &mut n,
                            std::ptr::null_mut(),
                        );
                        if ok == 0 || n == 0 {
                            break;
                        }
                        let take = (n as usize).min(buf.len());
                        msg.extend_from_slice(&buf[..take]);
                    }
                    // Confirm only when there is something to open; an empty
                    // or rejected list makes the sender open its own window.
                    let paths = parse(&msg).filter(|p| !p.is_empty());
                    if paths.is_some() {
                        let ack = b"ok\n";
                        let mut w = 0u32;
                        WriteFile(
                            h,
                            ack.as_ptr(),
                            ack.len() as u32,
                            &mut w,
                            std::ptr::null_mut(),
                        );
                        FlushFileBuffers(h);
                    }
                    DisconnectNamedPipe(h);
                    if let Some(p) = paths {
                        deliver(p);
                    }
                }
            }
        });
    if spawned.is_err() {
        // SAFETY: the handle was not handed to a thread.
        unsafe {
            CloseHandle(handle as windows_sys::Win32::Foundation::HANDLE);
        }
    }
}

#[cfg(not(windows))]
pub fn listen(_deliver: impl Fn(Vec<PathBuf>) + Send + 'static) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn abs(name: &str) -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(format!(r"C:\tmp\{name}"))
        } else {
            PathBuf::from(format!("/tmp/{name}"))
        }
    }

    #[test]
    fn round_trip() {
        let paths = vec![abs("a.md"), abs("dir with space/b c.txt"), abs("é.md")];
        let msg = encode(&paths);
        assert!(is_complete(&msg));
        assert_eq!(parse(&msg), Some(paths));
        assert_eq!(parse(&encode(&[])), Some(vec![]));
    }

    #[test]
    fn incomplete_or_foreign_messages_are_rejected() {
        let msg = encode(&[abs("a.md")]);
        for cut in 0..msg.len() - 1 {
            assert_eq!(parse(&msg[..cut]), None, "cut at {cut}");
        }
        assert!(!is_complete(&msg[..msg.len() - 2]));
        assert_eq!(parse(b"GET / HTTP/1.1\r\n\r\n"), None);
        assert!(
            is_complete(b"GET / HTTP/1.1\r\n\r\n"),
            "stop reading garbage early"
        );
        assert_eq!(parse(&[0xff, 0xfe, 0x00]), None);
        assert_eq!(parse(&vec![b'a'; MAX_BYTES + 1]), None);
        assert!(is_complete(&vec![b'a'; MAX_BYTES]));
    }

    #[test]
    fn only_plain_local_absolute_paths_survive() {
        let mut text = format!("{HEADER}\n");
        for bad in [
            r"\\evil\share\x.md",
            "//evil/share/x.md",
            r"\\?\UNC\evil\x.md",
            r"\??\C:\x.md",
            r"\\.\pipe\x",
            "relative.md",
            r"..\up.md",
            "C:stream.md:hidden",
            "NUL",
            "",
        ] {
            text.push_str(bad);
            text.push('\n');
        }
        text.push_str(&abs("good.md").to_string_lossy());
        text.push_str("\nend\n");
        assert_eq!(parse(text.as_bytes()), Some(vec![abs("good.md")]));
        // More paths than the limit: the extra ones are dropped.
        let many: Vec<PathBuf> = (0..MAX_PATHS + 50)
            .map(|i| abs(&format!("f{i}.md")))
            .collect();
        assert_eq!(parse(&encode(&many)).map(|v| v.len()), Some(MAX_PATHS));
    }

    #[test]
    fn parser_never_panics_on_random_bytes() {
        let mut x = 0x9E3779B97F4A7C15u64;
        let mut rnd = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        let seed = encode(&[abs("a.md"), abs("b.md")]);
        for _ in 0..20_000 {
            let mut m = seed.clone();
            for _ in 0..(rnd() % 6) {
                let i = (rnd() as usize) % m.len().max(1);
                match rnd() % 4 {
                    0 => m[i] = rnd() as u8,
                    1 => {
                        m.insert(i, rnd() as u8);
                    }
                    2 => {
                        m.remove(i);
                    }
                    _ => m.truncate(i),
                }
                if m.is_empty() {
                    break;
                }
            }
            let _ = is_complete(&m);
            if let Some(paths) = parse(&m) {
                assert!(paths.len() <= MAX_PATHS);
            }
        }
    }

    #[test]
    fn folders_and_workspaces_are_not_forwarded() {
        let d = std::env::temp_dir().join(format!("litedit-single-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("a.md");
        let w = d.join("p.litedit-workspace");
        std::fs::write(&f, "x").unwrap();
        std::fs::write(&w, "x").unwrap();
        assert!(forwardable(&[f.clone()]));
        // A name the receiver would reject (it looks like a stream on
        // Windows) must not be handed over and then silently dropped.
        if cfg!(unix) {
            let odd = d.join("a:b.md");
            std::fs::write(&odd, "x").unwrap();
            assert!(!forwardable(&[odd.clone()]));
            assert!(!forwardable(&[f.clone(), odd]));
        }
        assert!(!forwardable(&[]));
        assert!(!forwardable(&[d.clone()]), "a folder opens its own window");
        assert!(!forwardable(&[f.clone(), w]), "a workspace file too");
        assert!(!forwardable(&[f, d.join("missing.md")]));
        assert!(pipe_name().starts_with(r"\\.\pipe\litedit-open-"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
