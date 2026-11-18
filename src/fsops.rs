//! File operations from the tree: new file, new folder, rename, delete.
//!
//! Names are validated as a single Windows path component before anything
//! touches the disk. Delete moves to the Recycle Bin; if Windows cannot
//! recycle (network drive, file too large), it shows its own "delete
//! permanently?" warning instead of silently destroying the file.

use std::path::{Path, PathBuf};

// Copy traversal is deliberately capability based. Once a directory has
// been selected or created, every child lookup is relative to that retained
// directory handle; mutable pathnames are kept only for messages and UI
// updates. The platform modules also open final components without following
// links/reparse points.
#[cfg(unix)]
mod copy_handles {
    use std::ffi::{CStr, CString, OsStr, OsString};
    use std::fs::File;
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::path::{Component, Path};

    pub enum Entry {
        File(File),
        Dir(Dir),
        Skip,
    }

    pub struct Dir(File);

    impl Entry {
        pub fn is_dir(&self) -> bool {
            matches!(self, Self::Dir(_))
        }
    }

    fn invalid(message: &'static str) -> io::Error {
        io::Error::new(io::ErrorKind::InvalidInput, message)
    }

    fn c_name(name: &OsStr) -> io::Result<CString> {
        CString::new(name.as_bytes()).map_err(|_| invalid("a path name contains NUL"))
    }

    fn open_at(dir: &File, name: &OsStr) -> io::Result<Entry> {
        let name = c_name(name)?;
        // O_NONBLOCK prevents an attacker-created FIFO/device from hanging
        // classification. It has no effect on ordinary files/directories.
        let flags = libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK;
        // SAFETY: `name` is NUL terminated and `dir` remains open for the call.
        let fd = unsafe { libc::openat(dir.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ELOOP) {
                return Ok(Entry::Skip);
            }
            return Err(error);
        }
        // SAFETY: openat returned a new owned descriptor.
        let file = unsafe { File::from_raw_fd(fd) };
        let file_type = file.metadata()?.file_type();
        if file_type.is_dir() {
            Ok(Entry::Dir(Dir(file)))
        } else if file_type.is_file() {
            Ok(Entry::File(file))
        } else {
            Ok(Entry::Skip)
        }
    }

    pub fn open_path(path: &Path) -> io::Result<Entry> {
        let mut current = Dir(File::open(if path.is_absolute() { "/" } else { "." })?);
        let parts: Vec<OsString> = path
            .components()
            .filter_map(|part| match part {
                Component::Normal(name) => Some(name.to_os_string()),
                Component::ParentDir => Some(OsString::from("..")),
                Component::CurDir | Component::RootDir => None,
                Component::Prefix(_) => None,
            })
            .collect();
        if parts.is_empty() {
            return Ok(Entry::Dir(current));
        }
        for (index, name) in parts.iter().enumerate() {
            let entry = current.open(name)?;
            if index + 1 == parts.len() {
                return Ok(entry);
            }
            current = match entry {
                Entry::Dir(dir) => dir,
                Entry::File(_) | Entry::Skip => {
                    return Err(io::Error::new(
                        io::ErrorKind::NotADirectory,
                        "a path component is not a folder",
                    ));
                }
            };
        }
        unreachable!()
    }

    pub fn open_dir_path(path: &Path) -> io::Result<Dir> {
        match open_path(path)? {
            Entry::Dir(dir) => Ok(dir),
            Entry::File(_) | Entry::Skip => Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                "the path is not a folder",
            )),
        }
    }

    impl Dir {
        pub fn try_clone(&self) -> io::Result<Self> {
            self.0.try_clone().map(Self)
        }

        pub fn open(&self, name: &OsStr) -> io::Result<Entry> {
            open_at(&self.0, name)
        }

        /// Read at most `limit + 1` names. The boolean says the directory
        /// contained more than the caller's remaining entry budget.
        pub fn names(&self, limit: usize) -> io::Result<(Vec<OsString>, bool)> {
            // fdopendir owns its descriptor, so give it a duplicate.
            // SAFETY: fcntl only observes the valid retained descriptor.
            let fd = unsafe { libc::fcntl(self.0.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: `fd` is a new directory descriptor.
            let raw = unsafe { libc::fdopendir(fd) };
            if raw.is_null() {
                let error = io::Error::last_os_error();
                // SAFETY: fdopendir did not take ownership on failure.
                unsafe { libc::close(fd) };
                return Err(error);
            }
            struct ReadDir(*mut libc::DIR);
            impl Drop for ReadDir {
                fn drop(&mut self) {
                    // SAFETY: this pointer came from fdopendir and is owned here.
                    unsafe { libc::closedir(self.0) };
                }
            }
            let raw = ReadDir(raw);
            let mut names = Vec::new();
            loop {
                // SAFETY: the DIR is retained for this loop and used serially.
                let entry = unsafe { libc::readdir(raw.0) };
                if entry.is_null() {
                    break;
                }
                // SAFETY: POSIX guarantees d_name is NUL terminated.
                let bytes = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
                if bytes == b"." || bytes == b".." {
                    continue;
                }
                if names.len() == limit {
                    return Ok((names, true));
                }
                names.push(OsString::from_vec(bytes.to_vec()));
            }
            Ok((names, false))
        }

        pub fn occupied(&self, name: &OsStr) -> bool {
            match self.open(name) {
                Ok(_) => true,
                Err(error) if error.kind() == io::ErrorKind::NotFound => false,
                Err(_) => true,
            }
        }

        pub fn create_dir(&self, name: &OsStr) -> io::Result<Self> {
            let name = c_name(name)?;
            // SAFETY: the parent and NUL-terminated name remain valid.
            let result = unsafe { libc::mkdirat(self.0.as_raw_fd(), name.as_ptr(), 0o777) };
            if result != 0 {
                return Err(io::Error::last_os_error());
            }
            match open_at(&self.0, OsStr::from_bytes(name.as_bytes()))? {
                Entry::Dir(dir) => Ok(dir),
                Entry::File(_) | Entry::Skip => Err(io::Error::new(
                    io::ErrorKind::NotADirectory,
                    "the created entry is not a folder",
                )),
            }
        }

        pub fn create_file(&self, name: &OsStr) -> io::Result<File> {
            let name = c_name(name)?;
            let flags =
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW;
            // SAFETY: the parent and NUL-terminated name remain valid.
            let fd = unsafe { libc::openat(self.0.as_raw_fd(), name.as_ptr(), flags, 0o666) };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: openat returned a new owned descriptor.
            Ok(unsafe { File::from_raw_fd(fd) })
        }

        pub fn remove_file(&self, name: &OsStr, _file: &File) -> io::Result<()> {
            let name = c_name(name)?;
            // unlinkat never follows the final component and is anchored to
            // the retained destination directory.
            // SAFETY: the parent and NUL-terminated name remain valid.
            let result = unsafe { libc::unlinkat(self.0.as_raw_fd(), name.as_ptr(), 0) };
            if result == 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        }
    }
}

#[cfg(windows)]
mod copy_handles {
    use std::ffi::{OsStr, OsString, c_void};
    use std::fs::File;
    use std::io;
    use std::mem::size_of;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use std::path::{Component, Path, PathBuf};

    type Handle = *mut c_void;

    const GENERIC_READ: u32 = 0x8000_0000;
    const GENERIC_WRITE: u32 = 0x4000_0000;
    const DELETE: u32 = 0x0001_0000;
    const SYNCHRONIZE: u32 = 0x0010_0000;
    const FILE_LIST_DIRECTORY: u32 = 0x0000_0001;
    const FILE_ADD_FILE: u32 = 0x0000_0002;
    const FILE_ADD_SUBDIRECTORY: u32 = 0x0000_0004;
    const FILE_TRAVERSE: u32 = 0x0000_0020;
    const FILE_READ_ATTRIBUTES: u32 = 0x0000_0080;
    const DESTINATION_DIRECTORY_ACCESS: u32 = FILE_LIST_DIRECTORY
        | FILE_ADD_FILE
        | FILE_ADD_SUBDIRECTORY
        | FILE_TRAVERSE
        | FILE_READ_ATTRIBUTES
        | SYNCHRONIZE;
    const FILE_SHARE_ALL: u32 = 0x0000_0007;
    const FILE_OPEN: u32 = 1;
    const FILE_CREATE: u32 = 2;
    const OPEN_EXISTING: u32 = 3;
    const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0000_0010;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    const FILE_DIRECTORY_FILE: u32 = 0x0000_0001;
    const FILE_SYNCHRONOUS_IO_NONALERT: u32 = 0x0000_0020;
    const FILE_NON_DIRECTORY_FILE: u32 = 0x0000_0040;
    const FILE_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const OBJ_CASE_INSENSITIVE: u32 = 0x0000_0040;
    const FILE_ATTRIBUTE_TAG_INFORMATION: i32 = 9;
    const FILE_NAMES_INFORMATION: u32 = 12;
    const FILE_DISPOSITION_INFORMATION: u32 = 13;
    const STATUS_NO_MORE_FILES: i32 = 0x8000_0006u32 as i32;
    const STATUS_BUFFER_OVERFLOW: i32 = 0x8000_0005u32 as i32;

    #[repr(C)]
    struct UnicodeString {
        length: u16,
        maximum_length: u16,
        buffer: *mut u16,
    }

    #[repr(C)]
    struct ObjectAttributes {
        length: u32,
        root_directory: Handle,
        object_name: *mut UnicodeString,
        attributes: u32,
        security_descriptor: *mut c_void,
        security_quality_of_service: *mut c_void,
    }

    #[repr(C)]
    struct IoStatusBlock {
        status_or_pointer: usize,
        information: usize,
    }

    #[repr(C)]
    struct FileAttributeTagInfo {
        attributes: u32,
        reparse_tag: u32,
    }

    #[repr(C)]
    struct FileDispositionInfo {
        delete_file: u8,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateFileW(
            name: *const u16,
            desired_access: u32,
            share_mode: u32,
            security_attributes: *const c_void,
            creation_disposition: u32,
            flags_and_attributes: u32,
            template_file: Handle,
        ) -> Handle;
        fn GetFileInformationByHandleEx(
            file: Handle,
            class: i32,
            information: *mut c_void,
            size: u32,
        ) -> i32;
    }

    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtCreateFile(
            file: *mut Handle,
            desired_access: u32,
            attributes: *mut ObjectAttributes,
            status: *mut IoStatusBlock,
            allocation_size: *const i64,
            file_attributes: u32,
            share_access: u32,
            disposition: u32,
            create_options: u32,
            ea_buffer: *const c_void,
            ea_length: u32,
        ) -> i32;
        fn NtQueryDirectoryFile(
            file: Handle,
            event: Handle,
            apc_routine: *const c_void,
            apc_context: *const c_void,
            status: *mut IoStatusBlock,
            information: *mut c_void,
            length: u32,
            information_class: u32,
            return_single_entry: u8,
            file_name: *const UnicodeString,
            restart_scan: u8,
        ) -> i32;
        fn NtSetInformationFile(
            file: Handle,
            status: *mut IoStatusBlock,
            information: *const c_void,
            length: u32,
            information_class: u32,
        ) -> i32;
        fn RtlNtStatusToDosError(status: i32) -> u32;
    }

    pub enum Entry {
        File(File),
        Dir(Dir),
        Skip,
    }

    pub struct Dir(File);

    impl Entry {
        pub fn is_dir(&self) -> bool {
            matches!(self, Self::Dir(_))
        }
    }

    fn nt_error(status: i32) -> io::Error {
        // SAFETY: this conversion routine accepts every NTSTATUS value.
        let error = unsafe { RtlNtStatusToDosError(status) };
        io::Error::from_raw_os_error(error as i32)
    }

    fn attributes(file: &File) -> io::Result<u32> {
        let mut info = FileAttributeTagInfo {
            attributes: 0,
            reparse_tag: 0,
        };
        // SAFETY: `info` is correctly sized and the file handle is retained.
        let ok = unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FILE_ATTRIBUTE_TAG_INFORMATION,
                (&mut info as *mut FileAttributeTagInfo).cast(),
                size_of::<FileAttributeTagInfo>() as u32,
            )
        };
        if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(info.attributes)
        }
    }

    fn classify(file: File) -> io::Result<Entry> {
        let attributes = attributes(&file)?;
        if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            Ok(Entry::Skip)
        } else if attributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
            Ok(Entry::Dir(Dir(file)))
        } else {
            Ok(Entry::File(file))
        }
    }

    fn nt_open(
        parent: &File,
        name: &OsStr,
        desired_access: u32,
        disposition: u32,
        file_attributes: u32,
        create_options: u32,
    ) -> io::Result<File> {
        let mut name: Vec<u16> = name.encode_wide().collect();
        let byte_len = name
            .len()
            .checked_mul(2)
            .filter(|length| *length <= u16::MAX as usize)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "the path is too long"))?;
        let mut unicode = UnicodeString {
            length: byte_len as u16,
            maximum_length: byte_len as u16,
            buffer: name.as_mut_ptr(),
        };
        let mut attributes = ObjectAttributes {
            length: size_of::<ObjectAttributes>() as u32,
            root_directory: parent.as_raw_handle(),
            object_name: &mut unicode,
            attributes: OBJ_CASE_INSENSITIVE,
            security_descriptor: std::ptr::null_mut(),
            security_quality_of_service: std::ptr::null_mut(),
        };
        let mut status_block = IoStatusBlock {
            status_or_pointer: 0,
            information: 0,
        };
        let mut handle: Handle = std::ptr::null_mut();
        // SAFETY: all pointed-to structures and the name buffer outlive the
        // synchronous call; a successful call returns one owned handle.
        let status = unsafe {
            NtCreateFile(
                &mut handle,
                desired_access,
                &mut attributes,
                &mut status_block,
                std::ptr::null(),
                file_attributes,
                FILE_SHARE_ALL,
                disposition,
                create_options,
                std::ptr::null(),
                0,
            )
        };
        if status < 0 {
            return Err(nt_error(status));
        }
        // SAFETY: NtCreateFile returned one owned handle on success.
        Ok(unsafe { File::from_raw_handle(handle) })
    }

    fn open_relative(parent: &File, name: &OsStr) -> io::Result<Entry> {
        let file = nt_open(
            parent,
            name,
            GENERIC_READ | SYNCHRONIZE,
            FILE_OPEN,
            0,
            FILE_SYNCHRONOUS_IO_NONALERT | FILE_OPEN_REPARSE_POINT,
        )?;
        classify(file)
    }

    fn open_destination(parent: &File, name: &OsStr) -> io::Result<Entry> {
        let file = nt_open(
            parent,
            name,
            DESTINATION_DIRECTORY_ACCESS,
            FILE_OPEN,
            0,
            FILE_DIRECTORY_FILE | FILE_SYNCHRONOUS_IO_NONALERT | FILE_OPEN_REPARSE_POINT,
        )?;
        classify(file)
    }

    fn open_root(path: &Path, writable: bool) -> io::Result<Entry> {
        let mut name: Vec<u16> = path.as_os_str().encode_wide().collect();
        name.push(0);
        let access = if writable {
            DESTINATION_DIRECTORY_ACCESS
        } else {
            GENERIC_READ
        };
        // SAFETY: the path is NUL terminated and every optional pointer is null.
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                access,
                FILE_SHARE_ALL,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_OPEN_REPARSE_POINT,
                std::ptr::null_mut(),
            )
        };
        if handle as isize == -1 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: CreateFileW returned one owned handle.
        classify(unsafe { File::from_raw_handle(handle) })
    }

    fn open_path_with(path: &Path, writable: bool) -> io::Result<Entry> {
        let mut root = if path.is_absolute() {
            PathBuf::new()
        } else {
            PathBuf::from(".")
        };
        let mut parts = Vec::new();
        for part in path.components() {
            match part {
                Component::Prefix(_) | Component::RootDir if path.is_absolute() => {
                    root.push(part.as_os_str())
                }
                Component::Prefix(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "drive-relative paths are not supported",
                    ));
                }
                Component::RootDir => {}
                Component::CurDir => {}
                Component::ParentDir => parts.push(OsString::from("..")),
                Component::Normal(name) => parts.push(name.to_os_string()),
            }
        }
        if root.as_os_str().is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the path has no filesystem root",
            ));
        }
        let mut current = match open_root(&root, writable && parts.is_empty())? {
            Entry::Dir(dir) => dir,
            Entry::File(_) | Entry::Skip => {
                return Err(io::Error::new(
                    io::ErrorKind::NotADirectory,
                    "the filesystem root is not a folder",
                ));
            }
        };
        if parts.is_empty() {
            return Ok(Entry::Dir(current));
        }
        for (index, name) in parts.iter().enumerate() {
            let entry = if writable && index + 1 == parts.len() {
                open_destination(&current.0, name)?
            } else {
                open_relative(&current.0, name)?
            };
            if index + 1 == parts.len() {
                return Ok(entry);
            }
            current = match entry {
                Entry::Dir(dir) => dir,
                Entry::File(_) | Entry::Skip => {
                    return Err(io::Error::new(
                        io::ErrorKind::NotADirectory,
                        "a path component is not a folder",
                    ));
                }
            };
        }
        unreachable!()
    }

    pub fn open_path(path: &Path) -> io::Result<Entry> {
        open_path_with(path, false)
    }

    pub fn open_dir_path(path: &Path) -> io::Result<Dir> {
        match open_path_with(path, true)? {
            Entry::Dir(dir) => Ok(dir),
            Entry::File(_) | Entry::Skip => Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                "the path is not a folder",
            )),
        }
    }

    impl Dir {
        pub fn try_clone(&self) -> io::Result<Self> {
            self.0.try_clone().map(Self)
        }

        pub fn open(&self, name: &OsStr) -> io::Result<Entry> {
            open_relative(&self.0, name)
        }

        pub fn names(&self, limit: usize) -> io::Result<(Vec<OsString>, bool)> {
            // FILE_NAMES_INFORMATION entries are naturally aligned in this buffer.
            let mut buffer = vec![0u64; 8192];
            let mut names = Vec::new();
            let mut restart = 1u8;
            loop {
                let mut status_block = IoStatusBlock {
                    status_or_pointer: 0,
                    information: 0,
                };
                // SAFETY: the retained directory and aligned output buffer are
                // valid for this synchronous call.
                let status = unsafe {
                    NtQueryDirectoryFile(
                        self.0.as_raw_handle(),
                        std::ptr::null_mut(),
                        std::ptr::null(),
                        std::ptr::null(),
                        &mut status_block,
                        buffer.as_mut_ptr().cast(),
                        (buffer.len() * size_of::<u64>()) as u32,
                        FILE_NAMES_INFORMATION,
                        0,
                        std::ptr::null(),
                        restart,
                    )
                };
                restart = 0;
                if status == STATUS_NO_MORE_FILES {
                    break;
                }
                if status < 0 && status != STATUS_BUFFER_OVERFLOW {
                    return Err(nt_error(status));
                }
                let used = status_block
                    .information
                    .min(buffer.len() * size_of::<u64>());
                if used == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Windows returned an empty directory result",
                    ));
                }
                let bytes = buffer.as_ptr().cast::<u8>();
                let mut offset = 0usize;
                while offset + 12 <= used {
                    // SAFETY: the bounds above cover each fixed header.
                    let next = unsafe { bytes.add(offset).cast::<u32>().read_unaligned() } as usize;
                    // SAFETY: FileNameLength is the third u32 in the header.
                    let name_bytes =
                        unsafe { bytes.add(offset + 8).cast::<u32>().read_unaligned() } as usize;
                    if name_bytes % 2 != 0 || offset + 12 + name_bytes > used {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "Windows returned an invalid directory entry",
                        ));
                    }
                    // SAFETY: the validated byte range contains UTF-16 code units.
                    let wide = unsafe {
                        std::slice::from_raw_parts(
                            bytes.add(offset + 12).cast::<u16>(),
                            name_bytes / 2,
                        )
                    };
                    let name = OsString::from_wide(wide);
                    if name != "." && name != ".." {
                        if names.len() == limit {
                            return Ok((names, true));
                        }
                        names.push(name);
                    }
                    if next == 0 {
                        break;
                    }
                    if next < 12 || offset + next > used {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "Windows returned an invalid directory offset",
                        ));
                    }
                    offset += next;
                }
            }
            Ok((names, false))
        }

        pub fn occupied(&self, name: &OsStr) -> bool {
            match open_relative(&self.0, name) {
                Ok(_) => true,
                Err(error) if error.kind() == io::ErrorKind::NotFound => false,
                Err(_) => true,
            }
        }

        pub fn create_dir(&self, name: &OsStr) -> io::Result<Self> {
            let file = nt_open(
                &self.0,
                name,
                DESTINATION_DIRECTORY_ACCESS,
                FILE_CREATE,
                FILE_ATTRIBUTE_DIRECTORY,
                FILE_DIRECTORY_FILE | FILE_SYNCHRONOUS_IO_NONALERT | FILE_OPEN_REPARSE_POINT,
            )?;
            match classify(file)? {
                Entry::Dir(dir) => Ok(dir),
                Entry::File(_) | Entry::Skip => Err(io::Error::new(
                    io::ErrorKind::NotADirectory,
                    "the created entry is not a folder",
                )),
            }
        }

        pub fn create_file(&self, name: &OsStr) -> io::Result<File> {
            nt_open(
                &self.0,
                name,
                GENERIC_WRITE | DELETE | SYNCHRONIZE,
                FILE_CREATE,
                0,
                FILE_NON_DIRECTORY_FILE | FILE_SYNCHRONOUS_IO_NONALERT | FILE_OPEN_REPARSE_POINT,
            )
        }

        pub fn remove_file(&self, _name: &OsStr, file: &File) -> io::Result<()> {
            let info = FileDispositionInfo { delete_file: 1 };
            let mut status_block = IoStatusBlock {
                status_or_pointer: 0,
                information: 0,
            };
            // SAFETY: the disposition structure remains valid for this
            // synchronous call and the destination handle has DELETE access.
            let status = unsafe {
                NtSetInformationFile(
                    file.as_raw_handle(),
                    &mut status_block,
                    (&info as *const FileDispositionInfo).cast(),
                    size_of::<FileDispositionInfo>() as u32,
                    FILE_DISPOSITION_INFORMATION,
                )
            };
            if status < 0 {
                Err(nt_error(status))
            } else {
                Ok(())
            }
        }
    }
}

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
    // (A dangling link counts as something there.)
    if occupied(&target) {
        // A case-only rename (a.md → A.md) is allowed when it is the same
        // file: on a case-insensitive folder the check sees ourselves. If an
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
    verify_recycle_path(path)?;
    #[cfg(test)]
    if let Some(bin) = TEST_RECYCLE.with(|t| t.borrow().clone()) {
        return test_recycle(path, &bin);
    }
    recycle(path)
}

/// Check every path component without following the component being checked.
/// Windows' Recycle Bin API does not guarantee that it will avoid traversing
/// reparse points, so none may reach that API, including junctions in parents.
fn verify_recycle_path(path: &Path) -> Result<(), String> {
    let mut prefix = PathBuf::new();
    let mut components = path.components().peekable();
    while let Some(component) = components.next() {
        prefix.push(component.as_os_str());
        if matches!(
            component,
            std::path::Component::Prefix(_) | std::path::Component::CurDir
        ) {
            continue;
        }
        let final_component = components.peek().is_none();
        let metadata = match std::fs::symlink_metadata(&prefix) {
            Ok(metadata) => metadata,
            Err(error) if final_component && error.kind() == std::io::ErrorKind::NotFound => {
                return Err("it no longer exists".into());
            }
            Err(_) => {
                return Err("could not safely verify this path for the Recycle Bin".into());
            }
        };
        if metadata_is_link_or_reparse(&metadata) {
            return Err(
                "links and reparse points cannot be safely moved to the Recycle Bin".into(),
            );
        }
    }
    Ok(())
}

fn metadata_is_link_or_reparse(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        has_reparse_attribute(metadata.file_attributes())
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(windows)]
fn has_reparse_attribute(attributes: u32) -> bool {
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
    attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
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

/// Are `a` and `b` the same file, however each was typed? Compared as text
/// only (nothing on disk is touched, no `canonicalize`): `.` and `..` parts
/// are resolved, and on Windows names that differ only by case, and `/`
/// against `\`, are the same.
pub fn same_file(a: &Path, b: &Path) -> bool {
    path_key(a, cfg!(windows)) == path_key(b, cfg!(windows))
}

/// The parts of `p` with `.` dropped and `..` applied (lower-cased when
/// `fold`). A `..` above the start is kept; one above the root is dropped.
fn path_key(p: &Path, fold: bool) -> Vec<(u8, String)> {
    use std::path::Component;
    let text = |c: Component| {
        let s = c.as_os_str().to_string_lossy();
        if fold {
            s.to_lowercase()
        } else {
            s.into_owned()
        }
    };
    let mut out: Vec<(u8, String)> = Vec::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => match out.last() {
                Some((2, _)) => {
                    out.pop();
                }
                Some((0 | 1, _)) => {}
                _ => out.push((3, String::new())),
            },
            // Separator style inside a prefix (`\\?\` vs `//?/`) is not
            // folded: such paths are never opened anyway.
            Component::Prefix(_) => out.push((0, text(c))),
            Component::RootDir => out.push((1, String::new())),
            Component::Normal(_) => out.push((2, text(c))),
        }
    }
    out
}

/// Where `p` (at or under `old`) is after `old` became `new`. Counts path
/// parts instead of comparing them, so a different spelling of `old` (case
/// on Windows) still gives the right place.
pub fn rebase(p: &Path, old: &Path, new: &Path) -> PathBuf {
    let mut out = new.to_path_buf();
    out.extend(p.components().skip(old.components().count()));
    out
}

fn free_copy_name(
    src: &Path,
    dir: &copy_handles::Dir,
    is_dir: bool,
) -> Result<std::ffi::OsString, String> {
    let name = src.file_name().ok_or("cannot copy this")?;
    if !dir.occupied(name) {
        return Ok(name.to_os_string());
    }
    let name = name.to_string_lossy();
    for n in 1..=999 {
        let candidate = std::ffi::OsString::from(copy_name(&name, is_dir, n));
        if !dir.occupied(&candidate) {
            return Ok(candidate);
        }
    }
    Err("there are too many copies of it already".into())
}

/// A file being copied: `to` was created by us and is incomplete.
struct OpenCopy {
    from: std::fs::File,
    to: std::fs::File,
    to_parent: copy_handles::Dir,
    to_name: std::ffi::OsString,
    to_path: PathBuf,
    from_path: PathBuf,
}

struct CopyItem {
    source_path: PathBuf,
    destination_path: PathBuf,
    created_directory: bool,
}

struct PendingEntry {
    source: copy_handles::Entry,
    source_path: PathBuf,
    destination_parent: copy_handles::Dir,
    destination_name: std::ffi::OsString,
    destination_path: PathBuf,
    depth: usize,
}

struct PendingChildren {
    source: copy_handles::Dir,
    destination: copy_handles::Dir,
    source_path: PathBuf,
    destination_path: PathBuf,
    names: Vec<std::ffi::OsString>,
    depth: usize,
}

enum CopyWork {
    Entry(PendingEntry),
    Children(PendingChildren),
}

/// One paste or duplicate, done a slice at a time (`step`) so a big copy
/// never freezes the window.
///
/// Rules: nothing is ever replaced (files and folders are created
/// exclusively relative to retained directory handles); links and junctions
/// are never followed (skipped and counted); a folder is never copied into
/// itself; the whole job is refused up front if it is over the limits, and
/// stops if the sources grow past them meanwhile. A file that could not be
/// finished is removed again, so no half-written file stays behind.
pub struct CopyJob {
    limits: CopyLimits,
    /// Top-level items not started yet: (source, destination folder).
    queue: std::collections::VecDeque<(PathBuf, PathBuf)>,
    /// The item being copied. Paths are display values only; all I/O uses
    /// retained handles in `stack` and `open`.
    item: Option<CopyItem>,
    /// Handle-relative entries and directory enumerations still to copy.
    stack: Vec<CopyWork>,
    /// Entry work discovered but not started (children frames do not count).
    pending_entries: usize,
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
            pending_entries: 0,
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
        enum ScanWork {
            Entry(copy_handles::Entry),
            Children(copy_handles::Dir, Vec<std::ffi::OsString>),
        }

        let Ok(source) = copy_handles::open_path(src) else {
            return Ok(());
        };
        let mut pending = vec![ScanWork::Entry(source)];
        while let Some(work) = pending.pop() {
            match work {
                ScanWork::Entry(copy_handles::Entry::File(file)) => {
                    self.total_entries += 1;
                    self.total_bytes = self
                        .total_bytes
                        .saturating_add(file.metadata().map_or(0, |m| m.len()));
                }
                ScanWork::Entry(copy_handles::Entry::Dir(dir)) => {
                    self.total_entries += 1;
                    if self.total_entries > self.limits.entries {
                        return Err(self.limits.entries_message());
                    }
                    let remaining = self.limits.entries - self.total_entries;
                    if let Ok((names, over)) = dir.names(remaining) {
                        if over {
                            return Err(self.limits.entries_message());
                        }
                        pending.push(ScanWork::Children(dir, names));
                    }
                }
                ScanWork::Entry(copy_handles::Entry::Skip) => {}
                ScanWork::Children(dir, mut names) => {
                    if let Some(name) = names.pop() {
                        let entry = dir.open(&name);
                        pending.push(ScanWork::Children(dir, names));
                        if let Ok(entry) = entry {
                            pending.push(ScanWork::Entry(entry));
                        }
                    }
                }
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
            let _ = o.to_parent.remove_file(&o.to_name, &o.to);
        }
    }

    /// The current item cannot be completed.
    fn fail_item(&mut self, why: String) {
        self.drop_open();
        self.stack.clear();
        self.pending_entries = 0;
        if let Some(item) = self.item.take() {
            // Say so when part of a folder was already copied.
            let why = if item.created_directory {
                format!(
                    "{why}; \"{}\" is incomplete",
                    item.destination_path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                )
            } else {
                why
            };
            self.failed.push((item.source_path, why));
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
                    if let Some(m) = meta {
                        let _ = done.to.set_permissions(m.permissions());
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
        if let Some(work) = self.stack.pop() {
            return match work {
                CopyWork::Entry(entry) => {
                    self.pending_entries -= 1;
                    self.start_entry(entry)
                }
                CopyWork::Children(mut children) => {
                    let Some(name) = children.names.pop() else {
                        return Ok(());
                    };
                    let source = children
                        .source
                        .open(&name)
                        .map_err(|e| format!("{}: {e}", name.to_string_lossy()))?;
                    let destination_parent = children
                        .destination
                        .try_clone()
                        .map_err(|e| format!("{}: {e}", name.to_string_lossy()))?;
                    let source_path = children.source_path.join(&name);
                    let destination_path = children.destination_path.join(&name);
                    let depth = children.depth + 1;
                    self.stack.push(CopyWork::Children(children));
                    self.stack.push(CopyWork::Entry(PendingEntry {
                        source,
                        source_path,
                        destination_parent,
                        destination_name: name,
                        destination_path,
                        depth,
                    }));
                    Ok(())
                }
            };
        }
        if let Some(item) = self.item.take() {
            self.done.push(item.destination_path);
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
        let source = match copy_handles::open_path(&src) {
            Ok(source) => source,
            Err(_) => {
                self.failed.push((src, "it no longer exists".into()));
                return Ok(());
            }
        };
        if matches!(source, copy_handles::Entry::Skip) {
            self.skip(&src);
            return Ok(());
        }
        let destination = match copy_handles::open_dir_path(&dir) {
            Ok(destination) => destination,
            Err(_) => {
                self.failed
                    .push((src, "the destination is not a folder".into()));
                return Ok(());
            }
        };
        match free_copy_name(&src, &destination, source.is_dir()) {
            Ok(name) => {
                let to = dir.join(&name);
                self.item = Some(CopyItem {
                    source_path: src.clone(),
                    destination_path: to.clone(),
                    created_directory: false,
                });
                self.pending_entries = 1;
                self.stack.push(CopyWork::Entry(PendingEntry {
                    source,
                    source_path: src,
                    destination_parent: destination,
                    destination_name: name,
                    destination_path: to,
                    depth: 0,
                }));
            }
            Err(e) => self.failed.push((src, e)),
        }
        Ok(())
    }

    /// Create one entry relative to its retained destination directory.
    fn start_entry(&mut self, pending: PendingEntry) -> Result<(), String> {
        let name = || {
            pending
                .source_path
                .file_name()
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
        match pending.source {
            copy_handles::Entry::Skip => self.skip(&pending.source_path),
            copy_handles::Entry::Dir(source) => {
                if pending.depth >= MAX_COPY_DEPTH {
                    return Err(format!(
                        "{}: folders nested more than {MAX_COPY_DEPTH} deep",
                        name()
                    ));
                }
                self.count_entry()?;
                let destination = pending
                    .destination_parent
                    .create_dir(&pending.destination_name)
                    .map_err(exists)?;
                if pending.depth == 0 {
                    if let Some(item) = self.item.as_mut() {
                        item.created_directory = true;
                    }
                }
                let remaining = self
                    .limits
                    .entries
                    .saturating_sub(self.entries + self.pending_entries);
                let (names, over) = source
                    .names(remaining)
                    .map_err(|e| format!("{}: {e}", name()))?;
                if over {
                    return Err(self.over_limit(self.limits.entries_message()));
                }
                self.pending_entries += names.len();
                self.stack.push(CopyWork::Children(PendingChildren {
                    source,
                    destination,
                    source_path: pending.source_path,
                    destination_path: pending.destination_path,
                    names,
                    depth: pending.depth,
                }));
            }
            copy_handles::Entry::File(from) => {
                self.count_entry()?;
                let out = pending
                    .destination_parent
                    .create_file(&pending.destination_name)
                    .map_err(exists)?;
                self.open = Some(OpenCopy {
                    from,
                    to: out,
                    to_parent: pending.destination_parent,
                    to_name: pending.destination_name,
                    to_path: pending.destination_path,
                    from_path: pending.source_path,
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
    fn same_file_resolves_dots_and_folds_case_only_where_the_file_system_does() {
        let key = |s: &str, fold: bool| path_key(Path::new(s), fold);
        let big = key("/w/big.txt", false);
        for s in [
            "/w/./big.txt",
            "/w/x/../big.txt",
            "/w/x/y/../../big.txt",
            "/../w/big.txt",
        ] {
            assert_eq!(key(s, false), big, "{s}");
        }
        assert_ne!(key("/w/BIG.TXT", false), big);
        assert_eq!(key("/w/BIG.TXT", true), key("/w/big.txt", true));
        assert_ne!(key("/w/big.txt", false), key("/w/big.txt2", false));
        assert_ne!(key("../a", false), key("a", false));
        assert_eq!(key("a/../../b", false), key("../b", false));
        assert!(same_file(Path::new("/w/./a"), Path::new("/w/a")));
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
        std::fs::create_dir_all(d.join("empty-dir")).unwrap();
        std::fs::write(d.join("a.txt"), "one").unwrap();
        std::fs::write(d.join("dir/a.txt"), "two").unwrap();
        set_test_recycle_dir(Some(d.join("bin")));
        move_to_recycle_bin(&d.join("a.txt")).unwrap();
        move_to_recycle_bin(&d.join("dir/a.txt")).unwrap();
        move_to_recycle_bin(&d.join("empty-dir")).unwrap();
        assert!(
            move_to_recycle_bin(&d.join("a.txt")).is_err(),
            "gone already"
        );
        set_test_recycle_dir(None);
        let kept: Vec<Vec<u8>> = snapshot(&d.join("bin")).into_iter().map(|e| e.1).collect();
        assert_eq!(kept, vec![b"one".to_vec(), b"two".to_vec()]);
        assert!(!d.join("a.txt").exists() && !d.join("dir/a.txt").exists());
        assert!(d.join("bin/0000-empty-dir").is_dir());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn recycle_refuses_link_entries_and_linked_parents() {
        let d = scratch("recycle-links");
        let bin = d.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let network_target = PathBuf::from("//attacker.invalid/share/secret.txt");
        let missing_target = d.join("missing.txt");
        std::os::unix::fs::symlink(&network_target, d.join("network-link")).unwrap();
        std::os::unix::fs::symlink(&missing_target, d.join("dangling-link")).unwrap();
        std::os::unix::fs::symlink(&network_target, d.join("network-parent")).unwrap();

        set_test_recycle_dir(Some(bin.clone()));
        for path in [
            d.join("network-link"),
            d.join("dangling-link"),
            d.join("network-parent/child.txt"),
        ] {
            assert_eq!(
                move_to_recycle_bin(&path).unwrap_err(),
                "links and reparse points cannot be safely moved to the Recycle Bin"
            );
        }
        set_test_recycle_dir(None);

        assert_eq!(
            std::fs::read_link(d.join("network-link")).unwrap(),
            network_target
        );
        assert_eq!(
            std::fs::read_link(d.join("dangling-link")).unwrap(),
            missing_target
        );
        assert_eq!(std::fs::read_dir(&bin).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(windows)]
    #[test]
    fn recycle_recognizes_windows_reparse_attributes() {
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

        assert!(has_reparse_attribute(FILE_ATTRIBUTE_REPARSE_POINT));
        assert!(has_reparse_attribute(FILE_ATTRIBUTE_REPARSE_POINT | 0x20));
        assert!(!has_reparse_attribute(0x20));
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

    #[cfg(unix)]
    #[test]
    fn rename_never_replaces_a_dangling_link() {
        let d = std::env::temp_dir().join(format!("litedit-dangle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::os::unix::fs::symlink(d.join("nowhere"), d.join("link")).unwrap();
        let f = create_file(&d, "f.md").unwrap();
        assert!(rename(&f, "link").is_err(), "a dangling link is something");
        assert!(f.exists());
        let m = std::fs::symlink_metadata(d.join("link")).unwrap();
        assert!(m.file_type().is_symlink(), "the link is still there");
        assert_eq!(
            std::fs::read_link(d.join("link")).unwrap(),
            d.join("nowhere")
        );
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
