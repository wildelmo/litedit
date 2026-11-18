//! Bounded reads for files whose paths may change while they are opened.
//!
//! Metadata and content always come from one handle, and the content read has
//! its own hard limit. Image callers additionally ask for a local, non-link
//! handle so a pathname race cannot change the object accepted for decoding.

use std::fs::{File, Metadata};
use std::io::{self, Read};
use std::path::Path;

#[derive(Debug)]
pub(crate) enum ReadError {
    Open(io::Error),
    Metadata(io::Error),
    Read(io::Error),
    NotFile,
    TooLarge(u64),
    UnsafeLink,
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Open(e) | Self::Metadata(e) | Self::Read(e) => e.fmt(f),
            Self::NotFile => f.write_str("not a regular file"),
            Self::TooLarge(_) => f.write_str("file is too large"),
            Self::UnsafeLink => f.write_str("linked files are not opened"),
        }
    }
}

pub(crate) struct BoundedFile {
    pub bytes: Vec<u8>,
    pub metadata: Metadata,
    #[cfg(windows)]
    pub file: File,
}

/// Open `path` once, inspect that handle, and read no more than `limit + 1`
/// bytes. The extra byte distinguishes a file exactly at the limit from one
/// that grew after its metadata was inspected.
pub(crate) fn read_bounded(path: &Path, limit: u64) -> Result<BoundedFile, ReadError> {
    let file = File::open(path).map_err(ReadError::Open)?;
    read_opened(file, limit)
}

/// [`read_bounded`] for image content, refusing links/reparse points anywhere
/// in the path and mapped/network drives before any content is read.
pub(crate) fn read_local_bounded(path: &Path, limit: u64) -> Result<BoundedFile, ReadError> {
    let file = open_local(path)?;
    read_opened(file, limit)
}

fn read_opened(file: File, limit: u64) -> Result<BoundedFile, ReadError> {
    let metadata = file.metadata().map_err(ReadError::Metadata)?;
    if !metadata.is_file() {
        return Err(ReadError::NotFile);
    }
    if metadata.len() > limit {
        return Err(ReadError::TooLarge(metadata.len()));
    }
    let bytes = read_at_most(&file, limit)?;
    Ok(BoundedFile {
        bytes,
        metadata,
        #[cfg(windows)]
        file,
    })
}

fn read_at_most(reader: impl Read, limit: u64) -> Result<Vec<u8>, ReadError> {
    let mut bytes = Vec::new();
    reader
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(ReadError::Read)?;
    let len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if len > limit {
        return Err(ReadError::TooLarge(len));
    }
    Ok(bytes)
}

#[cfg(windows)]
fn local_drive_type(kind: u32) -> bool {
    use windows_sys::Win32::System::WindowsProgramming::{
        DRIVE_CDROM, DRIVE_FIXED, DRIVE_RAMDISK, DRIVE_REMOVABLE,
    };

    matches!(
        kind,
        DRIVE_FIXED | DRIVE_REMOVABLE | DRIVE_CDROM | DRIVE_RAMDISK
    )
}

#[cfg(windows)]
pub(crate) struct PinnedLocalPath {
    pub path: std::path::PathBuf,
    _directories: Vec<File>,
}

#[cfg(windows)]
impl PinnedLocalPath {
    /// The atomic rename opens its destination directory with write access.
    /// Allow that only after the staged file exists in the verified directory.
    /// Keep an unbroken no-delete pin on the parent and retain all ancestors.
    pub(crate) fn allow_rename(&mut self) -> Result<(), ReadError> {
        let parent = self.path.parent().ok_or(ReadError::NotFile)?;
        *self._directories.last_mut().ok_or(ReadError::NotFile)? =
            open_guard_directory(parent, true)?;
        Ok(())
    }

    pub(crate) fn open_file(&self) -> Result<File, ReadError> {
        use std::fs::OpenOptions;
        use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE,
            FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        let file = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&self.path)
            .map_err(ReadError::Open)?;
        let metadata = file.metadata().map_err(ReadError::Metadata)?;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(ReadError::UnsafeLink);
        }
        if !metadata.is_file() {
            return Err(ReadError::NotFile);
        }
        Ok(file)
    }
}

#[cfg(windows)]
fn open_local(path: &Path) -> Result<File, ReadError> {
    pin_local_path(path)?.open_file()
}

#[cfg(windows)]
fn retry_sharing<T>(mut operation: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    use windows_sys::Win32::Foundation::ERROR_SHARING_VIOLATION;

    for delay in [2, 4, 8, 16] {
        match operation() {
            Err(error) if error.raw_os_error() == Some(ERROR_SHARING_VIOLATION as i32) => {
                std::thread::sleep(std::time::Duration::from_millis(delay));
            }
            result => return result,
        }
    }
    operation()
}

#[cfg(windows)]
fn open_guard_directory(path: &Path, share_write: bool) -> Result<File, ReadError> {
    use std::fs::OpenOptions;
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    let share = if share_write {
        FILE_SHARE_READ | FILE_SHARE_WRITE
    } else {
        FILE_SHARE_READ
    };
    let file = retry_sharing(|| {
        OpenOptions::new()
            .read(true)
            .share_mode(share)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
    })
    .map_err(ReadError::Open)?;
    let metadata = file.metadata().map_err(ReadError::Metadata)?;
    if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(ReadError::UnsafeLink);
    }
    Ok(file)
}

#[cfg(windows)]
pub(crate) fn pin_local_path(path: &Path) -> Result<PinnedLocalPath, ReadError> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Component, PathBuf, Prefix};
    use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;

    // `absolute` expands relative and drive-relative spellings without
    // resolving filesystem links. Only ordinary DOS drive paths are allowed;
    // UNC, device, and verbatim paths never reach an open call here.
    let absolute = std::path::absolute(path).map_err(ReadError::Open)?;
    let mut components = absolute.components();
    let prefix = match components.next() {
        Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(_)) => prefix,
        _ => return Err(ReadError::UnsafeLink),
    };
    if !matches!(components.next(), Some(Component::RootDir)) {
        return Err(ReadError::UnsafeLink);
    }
    let names: Vec<OsString> = components
        .map(|component| match component {
            Component::Normal(name) => {
                let bytes = name.as_encoded_bytes();
                // The save also uses the verbatim spelling, where trailing
                // spaces, dots and stream names have different meanings.
                if bytes.iter().any(|&b| matches!(b, b':' | 0))
                    || matches!(bytes.last(), Some(b' ' | b'.'))
                {
                    return Err(ReadError::UnsafeLink);
                }
                Ok(name.to_os_string())
            }
            _ => Err(ReadError::UnsafeLink),
        })
        .collect::<Result<_, _>>()?;

    let mut root = PathBuf::from(prefix.as_os_str());
    root.push(r"\");
    let mut root_wide: Vec<u16> = root.as_os_str().encode_wide().collect();
    root_wide.push(0);
    // SAFETY: `root_wide` is a live, NUL-terminated UTF-16 drive-root path.
    let drive_type = unsafe { GetDriveTypeW(root_wide.as_ptr()) };
    if !local_drive_type(drive_type) {
        return Err(ReadError::UnsafeLink);
    }

    let Some((file_name, directory_names)) = names.split_last() else {
        return Err(ReadError::NotFile);
    };
    let mut held_directories = Vec::with_capacity(names.len());
    let mut current = root;
    let mut last = open_guard_directory(&current, false)?;
    for name in directory_names {
        let next = current.join(name);
        let guarded_child = open_guard_directory(&next, false)?;
        // With the child pinned, this directory cannot become empty or
        // receive a reparse point. Permit unrelated sibling operations again
        // while continuously holding a handle that denies directory deletion.
        let shared_parent = open_guard_directory(&current, true)?;
        held_directories.push(shared_parent);
        last = guarded_child; // closes the original, stricter parent handle
        current = next;
    }
    held_directories.push(last);

    current.push(file_name);
    Ok(PinnedLocalPath {
        path: current,
        _directories: held_directories,
    })
}

#[cfg(not(windows))]
fn open_local(path: &Path) -> Result<File, ReadError> {
    let metadata = std::fs::symlink_metadata(path).map_err(ReadError::Metadata)?;
    if metadata.file_type().is_symlink() {
        return Err(ReadError::UnsafeLink);
    }
    File::open(path).map_err(ReadError::Open)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_reader_accepts_the_limit_and_rejects_one_extra_byte() {
        assert_eq!(read_at_most(&b"four"[..], 4).unwrap(), b"four");
        assert!(matches!(
            read_at_most(&b"four"[..], 3),
            Err(ReadError::TooLarge(4))
        ));
    }

    #[cfg(windows)]
    #[test]
    fn windows_drive_policy_accepts_local_media_and_rejects_remote_drives() {
        use windows_sys::Win32::System::WindowsProgramming::{
            DRIVE_CDROM, DRIVE_FIXED, DRIVE_RAMDISK, DRIVE_REMOTE, DRIVE_REMOVABLE,
        };

        assert!(local_drive_type(DRIVE_FIXED));
        assert!(local_drive_type(DRIVE_REMOVABLE));
        assert!(local_drive_type(DRIVE_CDROM));
        assert!(local_drive_type(DRIVE_RAMDISK));
        assert!(!local_drive_type(DRIVE_REMOTE));
    }
}
