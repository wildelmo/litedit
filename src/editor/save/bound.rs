//! Atomic document saves on local Windows filesystems.
//!
//! Use the identity read with the document, pin each parent without following
//! reparse points, and rename the staged handle to the checked directory entry.
//! A rename replaces the entry itself; it never writes to a link's target.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::mem::{offset_of, size_of};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::path::Path;

use windows_sys::Win32::Foundation::GENERIC_WRITE;
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_ATTRIBUTE_ARCHIVE, FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_NORMAL,
    FILE_ATTRIBUTE_NOT_CONTENT_INDEXED, FILE_ATTRIBUTE_READONLY, FILE_ATTRIBUTE_SYSTEM,
    FILE_ATTRIBUTE_TEMPORARY, FILE_BASIC_INFO, FILE_DISPOSITION_INFO, FILE_ID_INFO,
    FILE_READ_ATTRIBUTES, FILE_STREAM_INFO, FileBasicInfo, FileDispositionInfo, FileIdInfo,
    FileRenameInfoEx, FileStreamInfo, GetFileInformationByHandleEx, GetVolumeInformationByHandleW,
    SetFileInformationByHandle, WRITE_DAC,
};
use windows_sys::Win32::System::WindowsProgramming::{
    FILE_RENAME_FLAG_POSIX_SEMANTICS, FILE_RENAME_FLAG_REPLACE_IF_EXISTS,
};

use super::{WindowsDacl, is_reserved_name, temp_path};
use crate::fileio::{PinnedLocalPath, ReadError, pin_local_path};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Identity(u64, [u8; 16], u64);

impl Identity {
    fn read(file: &File) -> io::Result<Self> {
        let mut info = FILE_ID_INFO::default();
        // SAFETY: the file and the writable result structure are live.
        check(unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FileIdInfo,
                (&mut info as *mut FILE_ID_INFO).cast(),
                size_of::<FILE_ID_INFO>() as u32,
            )
        })?;
        if info.FileId.Identifier == [0; 16] {
            return Err(io::Error::other(
                "the filesystem has no stable file identity",
            ));
        }
        Ok(Self(
            info.VolumeSerialNumber,
            info.FileId.Identifier,
            file.metadata()?.creation_time(),
        ))
    }
}

#[derive(Clone, Debug)]
pub(crate) struct SaveTarget {
    identity: Identity,
}

impl SaveTarget {
    pub(crate) fn from_file(file: &File) -> io::Result<Self> {
        // Keep the ID and creation time, not the handle. An open tab must
        // not prevent other programs from moving or deleting its folder.
        Ok(Self {
            identity: Identity::read(file)?,
        })
    }

    pub(crate) fn save(&self, path: &Path, bytes: &[u8]) -> Result<Self, String> {
        save_document(path, bytes, Some(self.identity))
    }
}

pub(crate) fn save_as_bound(path: &Path, bytes: &[u8]) -> Result<SaveTarget, String> {
    save_document(path, bytes, None)
}

fn save_document(
    path: &Path,
    bytes: &[u8],
    expected: Option<Identity>,
) -> Result<SaveTarget, String> {
    if is_reserved_name(path) {
        return Err("that name is reserved by Windows".into());
    }
    let mut pinned = pin_local_path(path).map_err(path_error)?;
    let original = match pinned.open_file() {
        Ok(file) => Some(file),
        Err(ReadError::Open(error)) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(path_error(error)),
    };
    let original_id = original
        .as_ref()
        .map(Identity::read)
        .transpose()
        .map_err(|e| format!("cannot verify the original file: {e}"))?;
    if let (Some(expected), Some(original_id)) = (expected, original_id)
        && expected != original_id
    {
        return Err("the original file was replaced after it was opened; use Save As".into());
    }
    // A missing ordinary file can be recovered at its old name. In that
    // case the commit uses no-replace semantics so an intervening creation
    // remains untouched.
    let properties = original
        .as_ref()
        .map(ReplacementProperties::read)
        .transpose()
        .map_err(|e| format!("cannot safely preserve the file: {e}; use Save As to a new file"))?;

    let mut staged = OpenOptions::new()
        .write(true)
        .access_mode(GENERIC_WRITE | FILE_READ_ATTRIBUTES | DELETE | WRITE_DAC)
        .share_mode(0)
        .create_new(true)
        .open(temp_path(&pinned.path))
        .map_err(|e| format!("cannot stage the save in this folder: {e}"))?;

    let result = (|| -> Result<Identity, String> {
        if let Some(props) = &properties {
            props
                .dacl
                .apply_to(&staged)
                .map_err(|e| format!("cannot preserve the file's permissions: {e}"))?;
        }
        staged
            .write_all(bytes)
            .map_err(|e| format!("write failed: {e}"))?;
        if let Some(props) = &properties {
            props
                .apply_metadata(&staged)
                .map_err(|e| format!("cannot preserve file attributes: {e}"))?;
        }
        staged
            .sync_all()
            .map_err(|e| format!("flush failed: {e}"))?;
        let staged_id =
            Identity::read(&staged).map_err(|e| format!("cannot verify the staged file: {e}"))?;
        if let Some(expected) = original_id {
            let current = pinned.open_file().map_err(path_error)?;
            if Identity::read(&current).ok() != Some(expected) {
                return Err("the file changed while preparing the save; use Save As".into());
            }
        }
        pinned.allow_rename().map_err(path_error)?;
        rename_staged(&staged, &pinned, original_id.is_some())
            .map_err(|e| format!("the file could not be replaced atomically: {e}; use Save As"))?;
        Ok(staged_id)
    })();
    if result.is_err() {
        discard_staged(&staged);
    }
    drop(staged);
    let staged_id = result?;
    let saved = pinned
        .open_file()
        .map_err(|_| "the file was saved, but the destination changed; use Save As".to_string())?;
    let binding = SaveTarget::from_file(&saved)
        .map_err(|e| format!("the file was saved, but cannot be bound for later saves: {e}"))?;
    if binding.identity != staged_id {
        return Err("the file was saved, but the destination changed; use Save As".into());
    }
    Ok(binding)
}

fn path_error(error: ReadError) -> String {
    match error {
        ReadError::UnsafeLink => {
            "cannot save through a link, junction or network path; use Save As to a local file"
                .into()
        }
        error => format!("cannot prepare a safe save: {error}"),
    }
}

struct ReplacementProperties {
    dacl: WindowsDacl,
    basic: FILE_BASIC_INFO,
}

// Features that would need more than basic metadata and DACL preservation
// are explicitly refused so that a successful save cannot silently drop data.
const BASIC_ATTRIBUTES: u32 = FILE_ATTRIBUTE_ARCHIVE
    | FILE_ATTRIBUTE_HIDDEN
    | FILE_ATTRIBUTE_NORMAL
    | FILE_ATTRIBUTE_NOT_CONTENT_INDEXED
    | FILE_ATTRIBUTE_SYSTEM
    | FILE_ATTRIBUTE_TEMPORARY;

impl ReplacementProperties {
    fn read(file: &File) -> io::Result<Self> {
        let mut basic = FILE_BASIC_INFO::default();
        // SAFETY: the file and the writable result structure are live.
        check(unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FileBasicInfo,
                (&mut basic as *mut FILE_BASIC_INFO).cast(),
                size_of::<FILE_BASIC_INFO>() as u32,
            )
        })?;
        if basic.FileAttributes & FILE_ATTRIBUTE_READONLY != 0 {
            return Err(io::Error::other("the file is read-only"));
        }
        if basic.FileAttributes & !BASIC_ATTRIBUTES != 0 {
            return Err(io::Error::other(
                "the file has specialized Windows attributes",
            ));
        }
        if has_named_streams(file)? {
            return Err(io::Error::other("the file has alternate data streams"));
        }
        Ok(Self {
            dacl: WindowsDacl::from_file(file)?,
            basic,
        })
    }

    fn apply_metadata(&self, file: &File) -> io::Result<()> {
        let basic = FILE_BASIC_INFO {
            CreationTime: self.basic.CreationTime,
            LastAccessTime: self.basic.LastAccessTime,
            LastWriteTime: 0,
            ChangeTime: 0,
            FileAttributes: self.basic.FileAttributes,
        };
        // SAFETY: the file and the input structure are live.
        check(unsafe {
            SetFileInformationByHandle(
                file.as_raw_handle(),
                FileBasicInfo,
                (&basic as *const FILE_BASIC_INFO).cast(),
                size_of::<FILE_BASIC_INFO>() as u32,
            )
        })
    }
}

fn has_named_streams(file: &File) -> io::Result<bool> {
    // GetVolumeInformationByHandleW filesystem flag (absent from windows-sys).
    const FILE_NAMED_STREAMS: u32 = 0x0004_0000;
    let mut flags = 0;
    // SAFETY: only the filesystem flags are requested; other outputs are optional.
    check(unsafe {
        GetVolumeInformationByHandleW(
            file.as_raw_handle(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut flags,
            std::ptr::null_mut(),
            0,
        )
    })?;
    if flags & FILE_NAMED_STREAMS == 0 {
        return Ok(false);
    }
    // A buffer that cannot describe all streams fails closed.
    let mut buf = [0u64; 128];
    // SAFETY: this storage is aligned for FILE_STREAM_INFO and fully writable.
    check(unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileStreamInfo,
            buf.as_mut_ptr().cast(),
            size_of::<[u64; 128]>() as u32,
        )
    })?;
    // SAFETY: the successful API call wrote a FILE_STREAM_INFO header.
    let first = unsafe { &*buf.as_ptr().cast::<FILE_STREAM_INFO>() };
    let stream_offset = offset_of!(FILE_STREAM_INFO, StreamName);
    let unnamed = [
        b':' as u16,
        b':' as u16,
        b'$' as u16,
        b'D' as u16,
        b'A' as u16,
        b'T' as u16,
        b'A' as u16,
    ];
    if first.NextEntryOffset != 0 || first.StreamNameLength as usize != size_of::<[u16; 7]>() {
        return Ok(true);
    }
    // SAFETY: the checked seven UTF-16 units fit within the aligned buffer.
    let name = unsafe {
        std::slice::from_raw_parts(
            buf.as_ptr().cast::<u8>().add(stream_offset).cast::<u16>(),
            unnamed.len(),
        )
    };
    Ok(name != unnamed)
}

#[repr(C)]
struct RenameInfo {
    flags: u32,
    root: *mut std::ffi::c_void,
    name_bytes: u32,
    name: [u16; 1],
}

fn rename_staged(staged: &File, pinned: &PinnedLocalPath, replace: bool) -> io::Result<()> {
    // pin_local_path accepts ordinary absolute drive paths only.
    let name: Vec<u16> = std::ffi::OsStr::new(r"\\?\")
        .encode_wide()
        .chain(pinned.path.as_os_str().encode_wide())
        .collect();
    let name_bytes = name.len() * size_of::<u16>();
    // Keep one zeroed trailing wchar for Win32's variable-length structure.
    let name_offset = offset_of!(RenameInfo, name);
    let byte_len = (name_offset + name_bytes + size_of::<u16>()).max(size_of::<RenameInfo>());
    let mut buf = vec![0usize; byte_len.div_ceil(size_of::<usize>())];
    let header = buf.as_mut_ptr().cast::<RenameInfo>();
    // SAFETY: the allocation is pointer-aligned and large enough for the
    // header and entire name. The API uses the handle as the rename source.
    unsafe {
        (*header).flags = if replace {
            FILE_RENAME_FLAG_REPLACE_IF_EXISTS | FILE_RENAME_FLAG_POSIX_SEMANTICS
        } else {
            0
        };
        (*header).name_bytes = name_bytes as u32;
        std::ptr::copy_nonoverlapping(
            name.as_ptr(),
            buf.as_mut_ptr().cast::<u8>().add(name_offset).cast::<u16>(),
            name.len(),
        );
        check(SetFileInformationByHandle(
            staged.as_raw_handle(),
            FileRenameInfoEx,
            header.cast(),
            byte_len as u32,
        ))
    }
}

fn discard_staged(file: &File) {
    let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
    // SAFETY: dispose only of the exact staging file owned by this handle.
    unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileDispositionInfo,
            (&disposition as *const FILE_DISPOSITION_INFO).cast(),
            size_of::<FILE_DISPOSITION_INFO>() as u32,
        );
    }
}

fn check(ok: i32) -> io::Result<()> {
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch(label: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("litedit-bound-save-{label}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn saves_replace_the_file_and_bind_the_next_save() {
        let dir = scratch("atomic");
        let path = dir.join("note.txt");
        let initial = save_as_bound(&path, b"first").unwrap();
        let again = initial.save(&path, b"second").unwrap();
        assert_ne!(initial.identity, again.identity);
        let final_target = again.save(&path, b"third").unwrap();
        assert_ne!(again.identity, final_target.identity);
        assert_eq!(fs::read(&path).unwrap(), b"third");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        drop((initial, again, final_target));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn atomic_save_preserves_basic_windows_attributes() {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::SetFileAttributesW;

        let dir = scratch("attributes");
        let path = dir.join("hidden.txt");
        let original = save_as_bound(&path, b"first").unwrap();
        let before = fs::metadata(&path).unwrap().creation_time();
        let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        wide.push(0);
        // SAFETY: the path is a NUL-terminated UTF-16 string.
        check(unsafe { SetFileAttributesW(wide.as_ptr(), FILE_ATTRIBUTE_HIDDEN) }).unwrap();
        let saved = original.save(&path, b"second").unwrap();
        let after = fs::metadata(&path).unwrap();
        assert_eq!(after.creation_time(), before);
        assert_ne!(after.file_attributes() & FILE_ATTRIBUTE_HIDDEN, 0);
        drop((original, saved));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn opening_a_document_leaves_its_folder_movable() {
        let dir = scratch("movable-folder");
        let path = dir.join("note.txt");
        let original = save_as_bound(&path, b"first").unwrap();
        let doc = crate::doc::Document::open(&path).unwrap();
        let moved = dir.with_extension("moved");
        fs::rename(&dir, &moved).unwrap();
        assert_eq!(fs::read(moved.join("note.txt")).unwrap(), b"first");
        assert_eq!(doc.save_target.unwrap().identity, original.identity);
        fs::remove_dir_all(moved).unwrap();
    }

    #[test]
    fn saving_requires_the_original_document_identity() {
        let dir = scratch("identity");
        let original = dir.join("original.txt");
        let other = dir.join("other.txt");
        let binding = save_as_bound(&original, b"original").unwrap();
        fs::write(&other, b"other").unwrap();
        assert!(binding.save(&other, b"edited").is_err());
        assert_eq!(fs::read(&original).unwrap(), b"original");
        assert_eq!(fs::read(&other).unwrap(), b"other");
        drop(binding);
        fs::remove_dir_all(dir).unwrap();
    }
}
