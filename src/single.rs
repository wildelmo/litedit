//! One window: starting litedit with files while it is already running
//! opens them as tabs in the running window (like double-clicking a second
//! document in most editors).
//!
//! How: the first window listens on a local named pipe. A later launch
//! connects, checks who made the pipe, sends its file paths, waits for "ok"
//! and exits. This is local inter-process communication on the same PC,
//! not networking:
//! - the pipe is created with `PIPE_REJECT_REMOTE_CLIENTS` and an access
//!   list naming only the current user (`O:<user>D:P(A;;GA;;;<user>)`):
//!   no other account can open it, not even to read. If that access list
//!   cannot be built, the window does not listen at all;
//! - the name contains a hash of the user and session, so two users on one
//!   PC never share a window. The name can be guessed, so a later launch
//!   does not trust whoever answers: before sending anything it reads the
//!   pipe's owner and sends only if that is the current user. Only the user
//!   (or an administrator, who could do anything anyway) can create an
//!   object owned by the user;
//! - the message is only a list of absolute file paths. It is parsed by
//!   [`parse`], which is pure, bounded and never panics; anything that is
//!   not an existing regular file on a local path is dropped;
//! - the window serves one launch at a time and never waits on it for
//!   long: the whole message must arrive within one second of connecting
//!   ([`READ_DEADLINE`]), and after "ok" it waits at most one more second
//!   for the launch to hang up ([`ACK_DEADLINE`]). A launch that is too
//!   slow is cut off and nothing is opened for it;
//! - if anything goes wrong (no answer in time, refused, a pipe made by
//!   someone else), the launch simply opens its own window. Nothing is ever
//!   lost by forwarding.
//!
//! Folders and workspace files are not forwarded (they would switch the
//! running window's workspace): they open a new window.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const HEADER: &str = "litedit-open 1";
const END: &str = "end";
/// Limits on one message.
pub const MAX_BYTES: usize = 64 * 1024;
pub const MAX_PATHS: usize = 100;
const MAX_PATH_CHARS: usize = 4096;
/// A connected launch must have sent its whole message by then.
#[cfg_attr(not(windows), allow(dead_code))]
pub const READ_DEADLINE: Duration = Duration::from_secs(1);
/// After "ok", how long the window waits for the launch to hang up.
#[cfg_attr(not(windows), allow(dead_code))]
pub const ACK_DEADLINE: Duration = Duration::from_secs(1);
/// How often the window looks at the pipe while it waits.
#[cfg_attr(not(windows), allow(dead_code))]
const POLL: Duration = Duration::from_millis(5);
#[cfg_attr(not(windows), allow(dead_code))]
const ACK: &[u8] = b"ok\n";

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
        // The running window refuses paths that go through a link to a
        // network share (anything can write to the pipe); the reader chose
        // these, so this launch opens them in its own window instead.
        && paths
            .iter()
            .all(|p| crate::links::content_may_open(p).is_ok())
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

/// SDDL for the pipe: owned by `sid`, and a protected access list with a
/// single entry giving `sid` full access (nothing inherited, nobody else).
/// The user is named explicitly, not through "owner rights": a window
/// started elevated would otherwise be owned by Administrators and normal
/// launches by the same person could not reach it. None unless `sid` looks
/// like a SID string (`S-1-…`, digits and dashes), so it cannot add SDDL.
#[cfg_attr(not(windows), allow(dead_code))]
fn pipe_sddl(sid: &str) -> Option<String> {
    let rest = sid.strip_prefix("S-1-")?;
    let well_formed = sid.len() <= 256
        && !rest.is_empty()
        && rest.split('-').all(|part| {
            !part.is_empty() && part.len() <= 20 && part.bytes().all(|b| b.is_ascii_digit())
        });
    well_formed.then(|| format!("O:{sid}D:P(A;;GA;;;{sid})"))
}

/// A later launch sends its paths only when the pipe's owner is known to be
/// the current user (`Some(true)`); a different owner or a failed check
/// means "open your own window".
#[cfg_attr(not(windows), allow(dead_code))]
fn may_send(owner_is_me: Option<bool>) -> bool {
    owner_is_me == Some(true)
}

/// True once `limit` has passed since `start`.
#[cfg_attr(not(windows), allow(dead_code))]
fn expired(start: Instant, now: Instant, limit: Duration) -> bool {
    now.saturating_duration_since(start) >= limit
}

/// The server's end of one connection, as the serving logic sees it. The
/// Windows pipe implements it; tests use a fake.
#[cfg_attr(not(windows), allow(dead_code))]
trait PipeIo {
    /// Bytes waiting to be read, or None once the client is gone (or on any
    /// error). Never blocks.
    fn available(&mut self) -> Option<usize>;
    /// Read into `buf`; only called with at most `available()` bytes, so it
    /// does not block. None on error.
    fn read(&mut self, buf: &mut [u8]) -> Option<usize>;
    /// Write all of `data` (small enough to fit the pipe's buffer, so it does
    /// not block). False on error.
    fn write(&mut self, data: &[u8]) -> bool;
}

/// Read one message, reading only what is already there, until it is
/// complete. None if the client hangs up first or `limit` passes.
#[cfg_attr(not(windows), allow(dead_code))]
fn read_message(p: &mut impl PipeIo, limit: Duration) -> Option<Vec<u8>> {
    let start = Instant::now();
    let mut msg = Vec::new();
    let mut buf = [0u8; 4096];
    while !is_complete(&msg) {
        if expired(start, Instant::now(), limit) {
            return None;
        }
        let want = p.available()?.min(buf.len());
        if want == 0 {
            std::thread::sleep(POLL);
            continue;
        }
        let n = p.read(&mut buf[..want])?.min(want);
        if n == 0 {
            return None;
        }
        msg.extend_from_slice(&buf[..n]);
    }
    Some(msg)
}

/// Wait until the client hangs up or `limit` passes. True if it hung up.
#[cfg_attr(not(windows), allow(dead_code))]
fn wait_closed(p: &mut impl PipeIo, limit: Duration) -> bool {
    let start = Instant::now();
    loop {
        if p.available().is_none() {
            return true;
        }
        if expired(start, Instant::now(), limit) {
            return false;
        }
        std::thread::sleep(POLL);
    }
}

/// Serve one connected launch. "ok" is sent only when there is something
/// to open, and `deliver` gets the paths exactly when "ok" was written: the
/// launch may then have read it and exited, so the files must open here
/// even if it never hangs up. They are handed over before the wait for the
/// hang-up, so they open at once however long that takes. A launch that
/// hangs up before "ok" could be written gets nothing and opens its own
/// window. True if the files were delivered.
#[cfg_attr(not(windows), allow(dead_code))]
fn serve_one(
    p: &mut impl PipeIo,
    read_limit: Duration,
    ack_limit: Duration,
    deliver: impl FnOnce(Vec<PathBuf>),
) -> bool {
    let Some(msg) = read_message(p, read_limit) else {
        return false;
    };
    // An empty or rejected list makes the sender open its own window.
    let Some(paths) = parse(&msg).filter(|p| !p.is_empty()) else {
        return false;
    };
    if !p.write(ACK) {
        return false;
    }
    deliver(paths);
    let _hung_up = wait_closed(p, ack_limit); // either way the files open
    true
}

/// Windows plumbing: the current user's SID, the pipe's access list and
/// owner check, and the pipe end used by [`serve_one`].
#[cfg(windows)]
mod win {
    use std::ffi::c_void;
    use std::os::windows::io::{FromRawHandle, OwnedHandle};
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::{ERROR_SUCCESS, HANDLE, LocalFree};
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        GetSecurityInfo, SDDL_REVISION_1, SE_KERNEL_OBJECT,
    };
    use windows_sys::Win32::Security::{
        EqualSid, GetLengthSid, GetTokenInformation, IsValidSid, OWNER_SECURITY_INFORMATION,
        PSECURITY_DESCRIPTOR, PSID, TOKEN_QUERY, TOKEN_USER, TokenUser,
    };
    use windows_sys::Win32::Storage::FileSystem::{ReadFile, WriteFile};
    use windows_sys::Win32::System::Pipes::PeekNamedPipe;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    /// Memory Windows allocated with LocalAlloc for us; freed exactly once,
    /// on drop. Null is allowed and means "nothing to free".
    pub struct LocalMem(pub *mut c_void);

    impl Drop for LocalMem {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: the pointer came from a Windows call documented to
                // allocate with LocalAlloc, and this is its only owner. The
                // result (null on success) leaves nothing to do on failure.
                unsafe {
                    LocalFree(self.0);
                }
            }
        }
    }

    /// The current user's SID, copied out of the process token. `buf` holds
    /// the TOKEN_USER answer; the SID it points to lives inside `buf` at
    /// `sid_off` (checked), so it is valid for as long as this value is.
    pub struct UserSid {
        buf: Vec<u64>, // u64: TOKEN_USER needs pointer alignment
        sid_off: usize,
    }

    impl UserSid {
        pub fn current() -> Option<UserSid> {
            let mut raw: HANDLE = null_mut();
            // SAFETY: GetCurrentProcess returns a pseudo-handle that needs no
            // closing; `raw` is a valid out pointer.
            if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) } == 0
                || raw.is_null()
            {
                return None;
            }
            // SAFETY: `raw` is a token handle we just opened and own; the
            // OwnedHandle closes it once, on every path out of here.
            let owned = unsafe { OwnedHandle::from_raw_handle(raw) };
            let token = std::os::windows::io::AsRawHandle::as_raw_handle(&owned) as HANDLE;
            let mut need = 0u32;
            // SAFETY: a size query: no buffer, length 0, valid out pointer.
            // It is expected to fail with ERROR_INSUFFICIENT_BUFFER.
            let ok = unsafe { GetTokenInformation(token, TokenUser, null_mut(), 0, &mut need) };
            let need = need as usize;
            if ok != 0 || need < size_of::<TOKEN_USER>() || need > 1024 {
                return None;
            }
            let mut buf = vec![0u64; need.div_ceil(8)];
            let bytes = buf.len() * 8;
            let mut got = 0u32;
            // SAFETY: `buf` is writable for exactly `bytes` bytes, the
            // length passed; `got` is a valid out pointer.
            let ok = unsafe {
                GetTokenInformation(
                    token,
                    TokenUser,
                    buf.as_mut_ptr().cast(),
                    bytes as u32,
                    &mut got,
                )
            };
            if ok == 0 || (got as usize) > bytes || (got as usize) < size_of::<TOKEN_USER>() {
                return None;
            }
            // SAFETY: the buffer holds a TOKEN_USER written by Windows, is at
            // least that large and 8-byte aligned.
            let sid = unsafe { (*buf.as_ptr().cast::<TOKEN_USER>()).User.Sid };
            let base = buf.as_ptr() as usize;
            let addr = sid as usize;
            // The SID must lie inside our buffer, after the TOKEN_USER, with
            // room for at least its 8-byte header (the part IsValidSid reads).
            if addr < base + size_of::<TOKEN_USER>() || addr + 8 > base + bytes {
                return None;
            }
            let sid_off = addr - base;
            // SAFETY: `sid` points into `buf` (checked above). IsValidSid
            // reads the fixed header first, GetLengthSid only after the SID
            // is known to be valid; the length is then checked to fit.
            let fits =
                unsafe { IsValidSid(sid) != 0 && sid_off + GetLengthSid(sid) as usize <= bytes };
            fits.then_some(UserSid { buf, sid_off })
        }

        /// The SID, for read-only Win32 calls (they take a mutable pointer
        /// type but do not write). Valid while `self` is.
        pub fn psid(&self) -> PSID {
            // SAFETY: `sid_off` is within `buf` (checked in `current`).
            unsafe { self.buf.as_ptr().cast::<u8>().add(self.sid_off) as PSID }
        }

        /// The SID as a string, `S-1-5-21-…`.
        pub fn to_sid_string(&self) -> Option<String> {
            let mut s: *mut u16 = null_mut();
            // SAFETY: `psid` is a valid SID; `s` is a valid out pointer.
            if unsafe { ConvertSidToStringSidW(self.psid(), &mut s) } == 0 || s.is_null() {
                return None;
            }
            let _free = LocalMem(s.cast()); // freed after the copy below
            // SAFETY: `s` is a NUL-terminated string from Windows, alive
            // until `_free` drops at the end of this function. The scan stops
            // at the NUL (or gives up at 256, well past any real SID) and
            // the slice covers only characters before it.
            unsafe {
                let len = (0..256).position(|i| *s.add(i) == 0)?;
                String::from_utf16(std::slice::from_raw_parts(s, len)).ok()
            }
        }
    }

    /// A security descriptor that lets only the current user open the pipe
    /// (see [`super::pipe_sddl`]). None if any step fails: then the caller
    /// must not listen at all.
    pub fn user_only_descriptor() -> Option<LocalMem> {
        let me = UserSid::current()?;
        let sddl = super::pipe_sddl(&me.to_sid_string()?)?;
        let wide: Vec<u16> = sddl.encode_utf16().chain(std::iter::once(0)).collect();
        let mut sd: PSECURITY_DESCRIPTOR = null_mut();
        // SAFETY: `wide` is NUL-terminated; `sd` is a valid out pointer; the
        // size out pointer may be null.
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide.as_ptr(),
                SDDL_REVISION_1,
                &mut sd,
                null_mut(),
            )
        };
        let sd = LocalMem(sd); // freed on every path, null or not
        (ok != 0 && !sd.0.is_null()).then_some(sd)
    }

    /// Is the object behind `h` owned by the current user? None if that
    /// cannot be found out (e.g. the handle lacks READ_CONTROL).
    pub fn owner_is_me(h: HANDLE) -> Option<bool> {
        let me = UserSid::current()?;
        let mut owner: PSID = null_mut();
        let mut sd: PSECURITY_DESCRIPTOR = null_mut();
        // SAFETY: `h` is an open handle owned by the caller; the out pointers
        // are valid; the parts not asked for are null.
        let err = unsafe {
            GetSecurityInfo(
                h,
                SE_KERNEL_OBJECT,
                OWNER_SECURITY_INFORMATION,
                &mut owner,
                null_mut(),
                null_mut(),
                null_mut(),
                &mut sd,
            )
        };
        // `owner` points into `sd`, which stays allocated until this
        // function returns.
        let _sd = LocalMem(sd);
        if err != ERROR_SUCCESS || owner.is_null() || sd.is_null() {
            return None;
        }
        // SAFETY: both are valid SIDs: `owner` inside `_sd`, the other
        // inside `me`, both alive here.
        Some(unsafe { EqualSid(owner, me.psid()) } != 0)
    }

    /// The server's end of the pipe while a client is connected.
    pub struct ServerEnd(pub HANDLE);

    impl super::PipeIo for ServerEnd {
        fn available(&mut self) -> Option<usize> {
            let mut avail = 0u32;
            // SAFETY: `self.0` is our pipe; no buffer is passed (length 0),
            // only the "total available" out pointer, which is valid.
            let ok =
                unsafe { PeekNamedPipe(self.0, null_mut(), 0, null_mut(), &mut avail, null_mut()) };
            (ok != 0).then_some(avail as usize)
        }

        fn read(&mut self, buf: &mut [u8]) -> Option<usize> {
            let len = u32::try_from(buf.len()).ok()?;
            let mut n = 0u32;
            // SAFETY: `buf` is writable for `len` bytes; no overlapped I/O.
            let ok = unsafe { ReadFile(self.0, buf.as_mut_ptr(), len, &mut n, null_mut()) };
            (ok != 0).then_some((n as usize).min(buf.len()))
        }

        fn write(&mut self, data: &[u8]) -> bool {
            let Ok(len) = u32::try_from(data.len()) else {
                return false;
            };
            let mut w = 0u32;
            // SAFETY: `data` is readable for `len` bytes; no overlapped I/O.
            let ok = unsafe { WriteFile(self.0, data.as_ptr(), len, &mut w, null_mut()) };
            ok != 0 && w == len
        }
    }
}

/// Second launch: try to hand `paths` to a running window. True only if
/// that window confirmed it; on any doubt the caller opens its own window.
#[cfg(windows)]
pub fn try_forward(paths: &[PathBuf]) -> bool {
    use std::io::{Read, Write};
    use std::sync::mpsc::channel;
    let name = pipe_name();
    let msg = encode(paths);
    // Let the running window come to the front when it opens the files.
    // SAFETY: a plain Win32 call with a constant argument.
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
                use std::os::windows::io::AsRawHandle;
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
                // The name can be guessed, so another account could have
                // made this pipe first. Only the user (or an administrator,
                // who could do anything anyway) can create an object owned
                // by the user's SID, so the owner identifies the server.
                // Nothing about the files is written before this check.
                let h = pipe.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE;
                if !may_send(win::owner_is_me(h)) {
                    return Ok(false);
                }
                pipe.write_all(&msg)?;
                pipe.flush()?;
                let mut ack = [0u8; 3];
                pipe.read_exact(&mut ack)?;
                // `pipe` is dropped right here, as the closure returns: the
                // window sees the hang-up and moves on.
                Ok(ack == *ACK)
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
/// Does nothing if another window already listens, or if the pipe's access
/// list cannot be built (later launches then open their own windows).
#[cfg(windows)]
pub fn listen(deliver: impl Fn(Vec<PathBuf>) + Send + 'static) {
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_PIPE_CONNECTED, GetLastError, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX,
    };
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
        PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
    };
    let name: Vec<u16> = pipe_name()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    // Only the current user may open the pipe. Never fall back to the
    // default access list (it lets everyone read).
    let Some(sd) = win::user_only_descriptor() else {
        return;
    };
    let sa = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.0,
        bInheritHandle: 0,
    };
    // SAFETY: plain Win32 call with valid, NUL-terminated arguments; `sa`
    // and the descriptor it points to are alive for the call (the pipe keeps
    // its own copy). The handle is owned by the listener thread for the life
    // of the process.
    let handle = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            4096,
            MAX_BYTES as u32,
            0,
            &sa,
        )
    };
    drop(sd);
    if handle == INVALID_HANDLE_VALUE || handle.is_null() {
        return; // another window owns the name: this one just doesn't listen
    }
    let handle = handle as usize; // HANDLE isn't Send; the thread owns it
    let spawned = std::thread::Builder::new()
        .name("litedit-single".into())
        .spawn(move || {
            let h = handle as windows_sys::Win32::Foundation::HANDLE;
            loop {
                // Waiting here for a launch is the idle state.
                // SAFETY: `h` is our pipe; no overlapped I/O.
                let connected = unsafe {
                    ConnectNamedPipe(h, std::ptr::null_mut()) != 0
                        || GetLastError() == ERROR_PIPE_CONNECTED // client got in first
                };
                if connected {
                    serve_one(
                        &mut win::ServerEnd(h),
                        READ_DEADLINE,
                        ACK_DEADLINE,
                        &deliver,
                    );
                } else {
                    // E.g. the client came and went already. Don't spin if
                    // the pipe is broken for good.
                    std::thread::sleep(Duration::from_millis(10));
                }
                // SAFETY: `h` is our pipe. Cuts off a client that is still
                // there (too slow, or never hung up); a failure only means
                // nobody was connected.
                unsafe {
                    DisconnectNamedPipe(h);
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
    fn sddl_names_only_the_user() {
        let sid = "S-1-5-21-1004336348-1177238915-682003330-512";
        assert_eq!(
            pipe_sddl(sid).as_deref(),
            Some(
                "O:S-1-5-21-1004336348-1177238915-682003330-512\
                 D:P(A;;GA;;;S-1-5-21-1004336348-1177238915-682003330-512)"
            )
        );
        assert!(pipe_sddl("S-1-5-18").is_some());
        for bad in [
            "",
            "S-1-",
            "S-1-5-",
            "S-1-5--21",
            "S-2-5-21",
            "s-1-5-21",
            "WD",
            "S-1-5-21)(A;;GA;;;WD",
            "S-1-5-21;WD",
            "S-1-5-21 ",
            "S-1-5-٣",
            "S-1-123456789012345678901",
        ] {
            assert_eq!(pipe_sddl(bad), None, "{bad:?}");
        }
        assert_eq!(pipe_sddl(&format!("S-1-{}", "1-".repeat(200) + "1")), None);
    }

    #[test]
    fn only_a_pipe_owned_by_the_user_gets_the_paths() {
        assert!(may_send(Some(true)));
        assert!(!may_send(Some(false)), "someone else made the pipe");
        assert!(!may_send(None), "the owner could not be read");
    }

    #[test]
    fn deadlines() {
        let t = Instant::now();
        let s = Duration::from_secs(1);
        assert!(!expired(t, t, s));
        assert!(!expired(t, t + Duration::from_millis(999), s));
        assert!(expired(t, t + s, s));
        assert!(expired(t, t + Duration::from_secs(5), s));
        // A clock that seems to go backwards is not "expired".
        assert!(!expired(t + s, t, s));
    }

    /// What a fake client does next, as seen by the server.
    enum Step {
        Data(Vec<u8>),
        /// Nothing waiting this time.
        Idle,
        Close,
    }

    /// A fake client end. When the script runs out the client stays
    /// connected and silent forever.
    struct Fake {
        steps: std::collections::VecDeque<Step>,
        written: Vec<u8>,
        write_ok: bool,
        reads_asked: Vec<usize>,
    }

    impl Fake {
        fn new(steps: Vec<Step>) -> Fake {
            Fake {
                steps: steps.into(),
                written: Vec::new(),
                write_ok: true,
                reads_asked: Vec::new(),
            }
        }
    }

    impl PipeIo for Fake {
        fn available(&mut self) -> Option<usize> {
            match self.steps.front() {
                Some(Step::Data(d)) => Some(d.len()),
                Some(Step::Idle) => {
                    self.steps.pop_front();
                    Some(0)
                }
                Some(Step::Close) => None,
                None => Some(0),
            }
        }
        fn read(&mut self, buf: &mut [u8]) -> Option<usize> {
            self.reads_asked.push(buf.len());
            let Some(Step::Data(d)) = self.steps.front_mut() else {
                panic!("read without data available: it would block");
            };
            assert!(buf.len() <= d.len(), "read more than available");
            let n = buf.len();
            buf.copy_from_slice(&d[..n]);
            d.drain(..n);
            if d.is_empty() {
                self.steps.pop_front();
            }
            Some(n)
        }
        fn write(&mut self, data: &[u8]) -> bool {
            if self.write_ok {
                self.written.extend_from_slice(data);
            }
            self.write_ok
        }
    }

    const SHORT: Duration = Duration::from_millis(60);

    /// [`serve_one`] with short deadlines: what was delivered, if anything.
    fn serve(p: &mut Fake) -> Option<Vec<PathBuf>> {
        let mut got = None;
        let delivered = serve_one(p, SHORT, SHORT, |paths| got = Some(paths));
        assert_eq!(delivered, got.is_some());
        got
    }

    #[test]
    fn a_launch_is_served_and_acknowledged() {
        let msg = encode(&[abs("a.md")]);
        let (head, tail) = msg.split_at(5);
        let mut p = Fake::new(vec![
            Step::Idle,
            Step::Data(head.to_vec()),
            Step::Idle,
            Step::Data(tail.to_vec()),
            Step::Idle,
            Step::Close,
        ]);
        let t = Instant::now();
        assert_eq!(serve(&mut p), Some(vec![abs("a.md")]));
        assert_eq!(p.written, ACK);
        assert!(t.elapsed() < SHORT, "the hang-up ends the wait");
        // A big message is read in chunks no larger than what is waiting.
        let many: Vec<PathBuf> = (0..MAX_PATHS).map(|i| abs(&format!("{i:0>200}"))).collect();
        let msg = encode(&many);
        assert!(msg.len() > 4096 * 4);
        let mut p = Fake::new(vec![Step::Data(msg), Step::Close]);
        assert_eq!(serve(&mut p), Some(many));
        assert!(p.reads_asked.iter().all(|&n| n <= 4096));
    }

    #[test]
    fn a_silent_or_slow_launch_is_cut_off() {
        // Connected, sends nothing.
        let mut p = Fake::new(vec![]);
        let t = Instant::now();
        assert_eq!(serve(&mut p), None);
        assert!(t.elapsed() >= SHORT && t.elapsed() < SHORT * 10);
        assert!(p.written.is_empty(), "no acknowledgement");
        // Dribbles one byte at a time, slower than the deadline allows.
        let mut steps = Vec::new();
        for b in encode(&[abs("a.md")]) {
            steps.push(Step::Data(vec![b]));
            steps.extend((0..4).map(|_| Step::Idle));
        }
        let mut p = Fake::new(steps);
        let t = Instant::now();
        assert_eq!(serve(&mut p), None);
        assert!(t.elapsed() < SHORT * 10);
        assert!(p.written.is_empty());
        // Half a message, then gone.
        let msg = encode(&[abs("a.md")]);
        let mut p = Fake::new(vec![Step::Data(msg[..10].to_vec()), Step::Close]);
        assert_eq!(serve(&mut p), None);
        assert!(p.written.is_empty());
    }

    #[test]
    fn acknowledgement_orderings() {
        let msg = encode(&[abs("a.md")]);
        // Never reads "ok" and never hangs up: the files still open here
        // (the launch may have read it), after a bounded wait.
        let mut p = Fake::new(vec![Step::Data(msg.clone())]);
        let t = Instant::now();
        let mut delivered_after = None;
        assert!(serve_one(&mut p, SHORT, SHORT, |paths| {
            assert_eq!(paths, vec![abs("a.md")]);
            delivered_after = Some(t.elapsed());
        }));
        assert!(t.elapsed() >= SHORT && t.elapsed() < SHORT * 10);
        // The files were handed over before that wait, not after it.
        assert!(delivered_after.is_some_and(|d| d < SHORT));
        // Hung up before "ok" could be written: nothing opens here, the
        // launch opens its own window.
        let mut p = Fake::new(vec![Step::Data(msg), Step::Close]);
        p.write_ok = false;
        assert_eq!(serve(&mut p), None);
        // Garbage, an empty list: no "ok", nothing opens.
        for m in [b"GET / HTTP/1.1\r\n\r\n".to_vec(), encode(&[])] {
            let mut p = Fake::new(vec![Step::Data(m), Step::Close]);
            assert_eq!(serve(&mut p), None);
            assert!(p.written.is_empty());
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
