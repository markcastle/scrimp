//! The raw Windows calls, each wrapped once with its safety reasoning.
//!
//! Nothing outside this module sees a handle or a pointer. Each function
//! takes a path, does one thing, and returns a typed [`Error`] on failure.
//! The rules from Phase 0 live in `file.rs`; this module only knows how to
//! make the calls correctly.

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{CloseHandle, GetLastError, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FindClose,
    FindFirstFileW, GetCompressedFileSizeW, GetDiskFreeSpaceExW, GetDiskFreeSpaceW, GetDriveTypeW,
    GetVolumeInformationW, GetVolumePathNameW, OPEN_EXISTING, SetFileAttributesW, WIN32_FIND_DATAW,
    WOF_FILE_COMPRESSION_INFO_V1, WOF_PROVIDER_FILE, WofIsExternalFile, WofSetFileDataLocation,
};
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::Ioctl::{FSCTL_DELETE_EXTERNAL_BACKING, FSCTL_GET_COMPRESSION, FSCTL_SET_COMPRESSION};
use windows::core::{BOOL, PCWSTR};

use crate::error::{Call, Error};
use crate::path::{from_wide, wide};

const GENERIC_READ: u32 = 0x8000_0000;
const READ_DATA: u32 = 0x0001;
const WRITE_DATA: u32 = 0x0002;
const READ_ATTRIBUTES: u32 = 0x0080;
const SYNCHRONIZE: u32 = 0x0010_0000;

/// NTFS compression formats for `FSCTL_SET_COMPRESSION` and `FSCTL_GET_COMPRESSION`.
pub(crate) const FORMAT_NONE: u16 = 0;
pub(crate) const FORMAT_LZNT1: u16 = 2;

/// How a file is opened. There is deliberately no way to open for writing
/// except for NTFS compression, because opening a WOF-compressed file for
/// writing decompresses it.
#[derive(Clone, Copy)]
pub(crate) enum Access {
    /// Enough to read a file's compression. Never undoes anything.
    Inspect,
    /// What WOF needs to compress and decompress: reading only.
    Wof,
    /// What NTFS compression needs. Only use on a file with no WOF compression.
    Ntfs,
}

impl Access {
    fn rights(self) -> u32 {
        match self {
            Access::Inspect => READ_ATTRIBUTES | READ_DATA | SYNCHRONIZE,
            Access::Wof => GENERIC_READ,
            Access::Ntfs => READ_ATTRIBUTES | READ_DATA | WRITE_DATA | SYNCHRONIZE,
        }
    }
}

/// An open file, closed when dropped.
pub(crate) struct Handle(HANDLE);

impl Handle {
    /// Opens a file, sharing it fully with other programs so Scrimp never
    /// blocks them.
    pub(crate) fn open(path: &Path, access: Access) -> Result<Handle, Error> {
        let w = wide(path);
        // SAFETY: `w` is a NUL-terminated UTF-16 path that outlives the call,
        // and every other argument is a plain value or `None`.
        let h = unsafe {
            CreateFileW(
                PCWSTR(w.as_ptr()),
                access.rights(),
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(0),
                None,
            )
        }
        .map_err(|e| Error::from_windows(&e, Call::Other))?;
        Ok(Handle(h))
    }

    /// Sends a control code with an optional input and output buffer.
    fn control(&self, code: u32, input: &[u8], output: &mut [u8], call: Call) -> Result<(), Error> {
        let mut returned = 0u32;
        let input_ptr = (!input.is_empty()).then(|| input.as_ptr().cast());
        let output_ptr = (!output.is_empty()).then(|| output.as_mut_ptr().cast());
        // SAFETY: the handle is open for the life of `self`; the buffers are
        // borrowed for the whole call and their lengths are passed with them;
        // the call is synchronous because the handle has no OVERLAPPED flag.
        unsafe {
            DeviceIoControl(
                self.0,
                code,
                input_ptr,
                input.len() as u32,
                output_ptr,
                output.len() as u32,
                Some(&mut returned),
                None,
            )
        }
        .map_err(|e| Error::from_windows(&e, call))
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle came from CreateFileW and is closed exactly once,
        // here. A failure to close can't be acted on, so it is ignored.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// What WOF says about a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WofReading {
    /// Not backed by WOF.
    None,
    /// Compressed by the WOF file provider with this algorithm number.
    File(u32),
    /// Backed by another provider: WIM (1) or something newer.
    Provider(u32),
}

/// Reads a file's WOF state. A drive that has no WOF at all (exFAT, a
/// network share) reads as `None`, which is true: nothing there can be
/// WOF-compressed.
pub(crate) fn wof_reading(path: &Path) -> Result<WofReading, Error> {
    let w = wide(path);
    let mut external = BOOL(0);
    let mut provider = 0u32;
    let mut info = WOF_FILE_COMPRESSION_INFO_V1::default();
    let mut len = size_of::<WOF_FILE_COMPRESSION_INFO_V1>() as u32;
    // SAFETY: `w` is NUL-terminated; every out-pointer is a live local, and
    // `len` holds the size of `info`. This takes the two-field
    // WOF_FILE_COMPRESSION_INFO_V1, not the FSCTL's three-field structure.
    let result = unsafe {
        WofIsExternalFile(
            PCWSTR(w.as_ptr()),
            Some(&mut external),
            Some(&mut provider),
            Some((&mut info as *mut WOF_FILE_COMPRESSION_INFO_V1).cast()),
            Some(&mut len),
        )
    };
    match result.map_err(|e| Error::from_windows(&e, Call::Wof)) {
        Ok(()) if !external.as_bool() => Ok(WofReading::None),
        Ok(()) if provider == WOF_PROVIDER_FILE => Ok(WofReading::File(info.Algorithm)),
        Ok(()) => Ok(WofReading::Provider(provider)),
        Err(Error::WofUnavailable | Error::NotSupported) => Ok(WofReading::None),
        Err(e) => Err(e),
    }
}

/// Compresses with WOF. The handle must be opened with [`Access::Wof`].
pub(crate) fn wof_set(handle: &Handle, algorithm: u32) -> Result<(), Error> {
    let info = WOF_FILE_COMPRESSION_INFO_V1 {
        Algorithm: algorithm,
        Flags: 0,
    };
    // SAFETY: the handle is open; `info` is the two-field structure this
    // call expects, alive for the whole call, and its size is passed.
    unsafe {
        WofSetFileDataLocation(
            handle.0,
            WOF_PROVIDER_FILE,
            (&info as *const WOF_FILE_COMPRESSION_INFO_V1).cast(),
            size_of::<WOF_FILE_COMPRESSION_INFO_V1>() as u32,
        )
    }
    .map_err(|e| Error::from_windows(&e, Call::Wof))
}

/// Removes WOF compression. The handle must be opened with [`Access::Wof`].
pub(crate) fn wof_remove(handle: &Handle) -> Result<(), Error> {
    handle.control(FSCTL_DELETE_EXTERNAL_BACKING, &[], &mut [], Call::Wof)
}

/// Reads a file's NTFS compression format. Drives without NTFS compression
/// read as [`FORMAT_NONE`].
pub(crate) fn ntfs_format(handle: &Handle) -> Result<u16, Error> {
    let mut out = [0u8; 2];
    match handle.control(FSCTL_GET_COMPRESSION, &[], &mut out, Call::Other) {
        Ok(()) => Ok(u16::from_le_bytes(out)),
        Err(Error::NotSupported) => Ok(FORMAT_NONE),
        Err(e) => Err(e),
    }
}

/// Sets NTFS compression. The handle must be opened with [`Access::Ntfs`].
pub(crate) fn ntfs_set(handle: &Handle, format: u16) -> Result<(), Error> {
    handle.control(FSCTL_SET_COMPRESSION, &format.to_le_bytes(), &mut [], Call::Other)
}

/// The space a file takes on disk, which Phase 0 found matches the free
/// space it actually uses, for every kind of compression.
pub(crate) fn on_disk(path: &Path) -> Result<u64, Error> {
    let w = wide(path);
    let mut high = 0u32;
    // SAFETY: `w` is NUL-terminated and `high` is a live local.
    let low = unsafe { GetCompressedFileSizeW(PCWSTR(w.as_ptr()), Some(&mut high)) };
    if low == u32::MAX {
        // SAFETY: reads this thread's last error, which the call above set
        // if it failed. A size whose low half is all ones is real when the
        // last error is zero.
        let last = unsafe { GetLastError() };
        if last.0 != 0 {
            return Err(Error::from_code(last.0, Call::Other, || {
                format!("GetCompressedFileSizeW failed ({})", last.0)
            }));
        }
    }
    Ok((u64::from(high) << 32) | u64::from(low))
}

/// Sets a file's attributes.
pub(crate) fn set_attributes(path: &Path, attributes: u32) -> Result<(), Error> {
    let w = wide(path);
    // SAFETY: `w` is NUL-terminated and outlives the call.
    unsafe { SetFileAttributesW(PCWSTR(w.as_ptr()), FILE_FLAGS_AND_ATTRIBUTES(attributes)) }
        .map_err(|e| Error::from_windows(&e, Call::Other))
}

/// A reparse point's tag, read from the directory entry without opening the
/// file or following a link.
pub(crate) fn reparse_tag(path: &Path) -> Result<u32, Error> {
    let w = wide(path);
    let mut data = WIN32_FIND_DATAW::default();
    // SAFETY: `w` is NUL-terminated; `data` is a live, correctly sized
    // WIN32_FIND_DATAW.
    let find =
        unsafe { FindFirstFileW(PCWSTR(w.as_ptr()), &mut data) }.map_err(|e| Error::from_windows(&e, Call::Other))?;
    // SAFETY: `find` is the search handle just returned, closed once here.
    unsafe {
        let _ = FindClose(find);
    }
    // For a reparse point, dwReserved0 holds its tag.
    Ok(data.dwReserved0)
}

/// The root of the volume a path is on, such as `C:\` or a mount folder.
pub(crate) fn volume_root(path: &Path) -> Result<PathBuf, Error> {
    let w = wide(path);
    let mut root = vec![0u16; w.len().max(64) + 2];
    // SAFETY: `w` is NUL-terminated; `root` is at least as long as the
    // input, which is the most the answer can need.
    unsafe { GetVolumePathNameW(PCWSTR(w.as_ptr()), &mut root) }.map_err(|e| Error::from_windows(&e, Call::Other))?;
    Ok(from_wide(&root))
}

/// The volume's filesystem name and flags.
pub(crate) fn volume_information(root: &Path) -> Result<(String, u32), Error> {
    let w = wide(root);
    let mut name = [0u16; 64];
    let mut flags = 0u32;
    // SAFETY: `w` is NUL-terminated; the name buffer and flags are live
    // locals and the buffer's length is passed with it.
    unsafe { GetVolumeInformationW(PCWSTR(w.as_ptr()), None, None, None, Some(&mut flags), Some(&mut name)) }
        .map_err(|e| Error::from_windows(&e, Call::Other))?;
    Ok((from_wide(&name).to_string_lossy().into_owned(), flags))
}

/// The volume's cluster size in bytes.
pub(crate) fn cluster_size(root: &Path) -> Result<u64, Error> {
    let w = wide(root);
    let (mut sectors, mut bytes) = (0u32, 0u32);
    // SAFETY: `w` is NUL-terminated; the out-values are live locals.
    unsafe { GetDiskFreeSpaceW(PCWSTR(w.as_ptr()), Some(&mut sectors), Some(&mut bytes), None, None) }
        .map_err(|e| Error::from_windows(&e, Call::Other))?;
    Ok(u64::from(sectors) * u64::from(bytes))
}

/// Free bytes available to this user on the volume.
pub(crate) fn free_bytes(root: &Path) -> Result<u64, Error> {
    let w = wide(root);
    let mut free = 0u64;
    // SAFETY: `w` is NUL-terminated; `free` is a live local.
    unsafe { GetDiskFreeSpaceExW(PCWSTR(w.as_ptr()), Some(&mut free), None, None) }
        .map_err(|e| Error::from_windows(&e, Call::Other))?;
    Ok(free)
}

/// Windows' drive type code for a volume root.
pub(crate) fn drive_type(root: &Path) -> u32 {
    let w = wide(root);
    // SAFETY: `w` is NUL-terminated; the call only reads it.
    unsafe { GetDriveTypeW(PCWSTR(w.as_ptr())) }
}
