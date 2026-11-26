//! Thin wrappers over the Windows calls the probe needs. Every call returns
//! a `Result` with the Windows error kept intact, because the error codes
//! are the findings.

use std::ffi::OsStr;
use std::fmt;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::IO::{DeviceIoControl, OVERLAPPED};
use windows::Win32::System::Ioctl::*;
use windows::core::{BOOL, PCWSTR};

pub const GENERIC_READ: u32 = 0x8000_0000;
pub const GENERIC_WRITE: u32 = 0x4000_0000;
pub const READ_DATA: u32 = 0x0001;
pub const WRITE_DATA: u32 = 0x0002;
pub const READ_ATTRIBUTES: u32 = 0x0080;
pub const WRITE_ATTRIBUTES: u32 = 0x0100;
pub const SYNCHRONIZE: u32 = 0x0010_0000;

/// A Windows error, shown as its Win32 code where there is one.
#[derive(Clone, Debug)]
pub struct WinError {
    pub hresult: i32,
    pub message: String,
}

impl WinError {
    /// The Win32 error code, when the HRESULT wraps one.
    pub fn win32(&self) -> Option<u32> {
        let h = self.hresult as u32;
        (h & 0xFFFF_0000 == 0x8007_0000).then_some(h & 0xFFFF)
    }
}

impl From<windows::core::Error> for WinError {
    fn from(e: windows::core::Error) -> Self {
        WinError {
            hresult: e.code().0,
            message: e.message().trim().to_string(),
        }
    }
}

impl fmt::Display for WinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.win32() {
            Some(code) => write!(f, "Win32 {code}: {}", self.message),
            None => write!(f, "HRESULT 0x{:08X}: {}", self.hresult as u32, self.message),
        }
    }
}

pub type WinResult<T> = Result<T, WinError>;

/// Shows a result as "ok" or the error, for report tables.
pub fn show<T>(r: &WinResult<T>) -> String {
    match r {
        Ok(_) => "ok".into(),
        Err(e) => e.to_string(),
    }
}

pub fn wide(path: &Path) -> Vec<u16> {
    OsStr::new(path).encode_wide().chain(Some(0)).collect()
}

/// An open file handle, closed on drop.
pub struct Handle(pub HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle came from CreateFileW and is closed once.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

pub fn open(path: &Path, access: u32, flags: u32) -> WinResult<Handle> {
    let w = wide(path);
    // SAFETY: `w` is a NUL-terminated UTF-16 path that outlives the call.
    let h = unsafe {
        CreateFileW(
            PCWSTR(w.as_ptr()),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(flags | FILE_FLAG_BACKUP_SEMANTICS.0),
            None,
        )
    }?;
    Ok(Handle(h))
}

/// Opens without FILE_FLAG_BACKUP_SEMANTICS, to rule that flag out.
pub fn open_plain(path: &Path, access: u32) -> WinResult<Handle> {
    let w = wide(path);
    // SAFETY: as in `open`.
    let h = unsafe {
        CreateFileW(
            PCWSTR(w.as_ptr()),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    }?;
    Ok(Handle(h))
}

/// The WOF driver's version for a provider on this file's volume. Fails
/// when that provider isn't available there.
pub fn wof_driver_version(h: &Handle, provider: u32) -> WinResult<u32> {
    // SAFETY: the handle is open.
    Ok(unsafe { WofGetDriverVersion(h.0, provider) }?)
}

/// Opens a file with a chosen share mode, to test what compression does
/// while another program has it open.
pub fn open_shared(path: &Path, access: u32, share: u32) -> WinResult<Handle> {
    let w = wide(path);
    // SAFETY: as in `open`.
    let h = unsafe {
        CreateFileW(
            PCWSTR(w.as_ptr()),
            access,
            FILE_SHARE_MODE(share),
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    }?;
    Ok(Handle(h))
}

/// The WOF algorithm number for a method name.
pub fn wof_id(name: &str) -> u32 {
    match name {
        "XPRESS4K" => FILE_PROVIDER_COMPRESSION_XPRESS4K,
        "XPRESS8K" => FILE_PROVIDER_COMPRESSION_XPRESS8K,
        "XPRESS16K" => FILE_PROVIDER_COMPRESSION_XPRESS16K,
        _ => FILE_PROVIDER_COMPRESSION_LZX,
    }
}

pub fn wof_name(id: u32) -> &'static str {
    match id {
        FILE_PROVIDER_COMPRESSION_XPRESS4K => "XPRESS4K",
        FILE_PROVIDER_COMPRESSION_XPRESS8K => "XPRESS8K",
        FILE_PROVIDER_COMPRESSION_XPRESS16K => "XPRESS16K",
        FILE_PROVIDER_COMPRESSION_LZX => "LZX",
        _ => "unknown algorithm",
    }
}

/// Compresses through wofutil.dll. Note the structure: wofutil takes
/// WOF_FILE_COMPRESSION_INFO_V1 (Algorithm, Flags), not the
/// FILE_PROVIDER_EXTERNAL_INFO_V1 (Version, Algorithm, Flags) that the
/// FSCTL takes. Passing the FSCTL one was the first run's bug.
pub fn wof_set_api(h: &Handle, algorithm: u32, flags: u32) -> WinResult<()> {
    let info = WOF_FILE_COMPRESSION_INFO_V1 {
        Algorithm: algorithm,
        Flags: flags,
    };
    // SAFETY: `info` is a valid structure for the duration of the call.
    unsafe {
        WofSetFileDataLocation(
            h.0,
            WOF_PROVIDER_FILE,
            &info as *const _ as *const _,
            std::mem::size_of::<WOF_FILE_COMPRESSION_INFO_V1>() as u32,
        )
    }?;
    Ok(())
}

/// The input FSCTL_SET_EXTERNAL_BACKING expects: the WOF header followed
/// directly by the file provider's structure.
#[repr(C)]
pub struct ExternalBacking {
    pub wof: WOF_EXTERNAL_INFO,
    pub file: FILE_PROVIDER_EXTERNAL_INFO_V1,
}

impl ExternalBacking {
    pub fn new(algorithm: u32) -> Self {
        Self::with_flags(algorithm, 0)
    }

    pub fn with_flags(algorithm: u32, flags: u32) -> Self {
        ExternalBacking {
            wof: WOF_EXTERNAL_INFO {
                Version: WOF_CURRENT_VERSION,
                Provider: WOF_PROVIDER_FILE,
            },
            file: FILE_PROVIDER_EXTERNAL_INFO_V1 {
                Version: FILE_PROVIDER_CURRENT_VERSION,
                Algorithm: algorithm,
                Flags: flags,
            },
        }
    }
}

fn ioctl(
    h: &Handle,
    code: u32,
    input: Option<(*const core::ffi::c_void, u32)>,
    output: Option<(*mut core::ffi::c_void, u32)>,
) -> WinResult<u32> {
    let mut returned = 0u32;
    let (inp, inlen) = input.map_or((None, 0), |(p, l)| (Some(p), l));
    let (outp, outlen) = output.map_or((None, 0), |(p, l)| (Some(p), l));
    // SAFETY: the buffers are owned by the caller and outlive this
    // synchronous call; the handle is open.
    unsafe { DeviceIoControl(h.0, code, inp, inlen, outp, outlen, Some(&mut returned), None) }?;
    Ok(returned)
}

/// Compresses through DeviceIoControl, as compact.exe does.
pub fn wof_set_fsctl(h: &Handle, algorithm: u32) -> WinResult<()> {
    wof_set_fsctl_flags(h, algorithm, 0)
}

pub fn wof_set_fsctl_flags(h: &Handle, algorithm: u32, flags: u32) -> WinResult<()> {
    let input = ExternalBacking::with_flags(algorithm, flags);
    ioctl(
        h,
        FSCTL_SET_EXTERNAL_BACKING,
        Some((
            &input as *const _ as *const _,
            std::mem::size_of::<ExternalBacking>() as u32,
        )),
        None,
    )?;
    Ok(())
}

/// Starts FSCTL_SET_EXTERNAL_BACKING on a handle opened for overlapped I/O.
/// Returns what DeviceIoControl said straight away (pending, or done).
///
/// # Safety
/// `input` and `ov` must stay alive and unmoved until the operation completes.
pub unsafe fn wof_set_fsctl_overlapped(h: &Handle, input: &ExternalBacking, ov: *mut OVERLAPPED) -> WinResult<()> {
    // SAFETY: the caller keeps `input` and `ov` alive until completion.
    unsafe {
        DeviceIoControl(
            h.0,
            FSCTL_SET_EXTERNAL_BACKING,
            Some(input as *const _ as *const _),
            std::mem::size_of::<ExternalBacking>() as u32,
            None,
            0,
            None,
            Some(ov),
        )
    }?;
    Ok(())
}

/// Removes WOF compression.
pub fn wof_remove(h: &Handle) -> WinResult<()> {
    ioctl(h, FSCTL_DELETE_EXTERNAL_BACKING, None, None)?;
    Ok(())
}

/// Sets classic NTFS compression: 0 none, 1 default, 2 LZNT1.
pub fn ntfs_set(h: &Handle, format: u16) -> WinResult<()> {
    ioctl(
        h,
        FSCTL_SET_COMPRESSION,
        Some((&format as *const _ as *const _, 2)),
        None,
    )?;
    Ok(())
}

pub fn ntfs_get(h: &Handle) -> WinResult<u16> {
    let mut format = 0u16;
    ioctl(
        h,
        FSCTL_GET_COMPRESSION,
        None,
        Some((&mut format as *mut _ as *mut _, 2)),
    )?;
    Ok(format)
}

pub fn set_sparse(h: &Handle) -> WinResult<()> {
    ioctl(h, FSCTL_SET_SPARSE, None, None)?;
    Ok(())
}

/// What WofIsExternalFile says: (is external, provider, algorithm if the file provider).
pub fn wof_state(path: &Path) -> WinResult<(bool, u32, Option<u32>)> {
    let w = wide(path);
    let mut external = BOOL(0);
    let mut provider = 0u32;
    let mut info = WOF_FILE_COMPRESSION_INFO_V1::default();
    let mut len = std::mem::size_of::<WOF_FILE_COMPRESSION_INFO_V1>() as u32;
    // SAFETY: every out-pointer is a live local; `len` is the buffer size.
    unsafe {
        WofIsExternalFile(
            PCWSTR(w.as_ptr()),
            Some(&mut external),
            Some(&mut provider),
            Some(&mut info as *mut _ as *mut _),
            Some(&mut len),
        )
    }?;
    let algorithm = (external.as_bool() && provider == WOF_PROVIDER_FILE).then_some(info.Algorithm);
    Ok((external.as_bool(), provider, algorithm))
}

/// A plain-text reading of a file's compression, for the report.
pub fn describe_state(path: &Path) -> String {
    let wof = match wof_state(path) {
        Ok((true, p, Some(a))) if p == WOF_PROVIDER_FILE => format!("WOF {}", wof_name(a)),
        Ok((true, p, _)) if p == WOF_PROVIDER_WIM => "WIM-backed".into(),
        Ok((true, p, _)) => format!("external provider {p}"),
        Ok((false, _, _)) => String::new(),
        Err(e) => format!("WofIsExternalFile failed ({e})"),
    };
    let ntfs = match open(path, READ_ATTRIBUTES | READ_DATA, 0).and_then(|h| ntfs_get(&h)) {
        Ok(0) => String::new(),
        Ok(2) => "NTFS LZNT1".into(),
        Ok(n) => format!("NTFS format {n}"),
        Err(e) => format!("FSCTL_GET_COMPRESSION failed ({e})"),
    };
    match (wof.is_empty(), ntfs.is_empty()) {
        (true, true) => "uncompressed".into(),
        (false, true) => wof,
        (true, false) => ntfs,
        (false, false) => format!("{wof} + {ntfs}"),
    }
}

pub fn compressed_size(path: &Path) -> WinResult<u64> {
    let w = wide(path);
    let mut high = 0u32;
    // SAFETY: `w` is NUL-terminated and `high` is a live local.
    let low = unsafe { GetCompressedFileSizeW(PCWSTR(w.as_ptr()), Some(&mut high)) };
    if low == u32::MAX {
        // SAFETY: reads this thread's last error, set by the call above.
        let code = unsafe { windows::Win32::Foundation::GetLastError() };
        if code.0 != 0 {
            return Err(windows::core::Error::from(code.to_hresult()).into());
        }
    }
    Ok((u64::from(high) << 32) | u64::from(low))
}

pub fn allocation_size(path: &Path) -> WinResult<u64> {
    let h = open(path, READ_ATTRIBUTES, 0)?;
    let mut info = FILE_STANDARD_INFO::default();
    // SAFETY: `info` is sized for FileStandardInfo.
    unsafe {
        GetFileInformationByHandleEx(
            h.0,
            FileStandardInfo,
            &mut info as *mut _ as *mut _,
            std::mem::size_of::<FILE_STANDARD_INFO>() as u32,
        )
    }?;
    Ok(info.AllocationSize as u64)
}

/// The file's named streams and their sizes, as FindFirstStreamW lists them.
pub fn streams(path: &Path) -> WinResult<Vec<(String, u64)>> {
    let w = wide(path);
    let mut data = WIN32_FIND_STREAM_DATA {
        StreamSize: 0,
        cStreamName: [0; 296],
    };
    // SAFETY: `data` is a WIN32_FIND_STREAM_DATA, as FindStreamInfoStandard expects.
    let find = unsafe {
        FindFirstStreamW(
            PCWSTR(w.as_ptr()),
            FindStreamInfoStandard,
            &mut data as *mut _ as *mut _,
            None,
        )
    }?;
    let mut out = Vec::new();
    loop {
        let len = data.cStreamName.iter().position(|&c| c == 0).unwrap_or(0);
        out.push((
            String::from_utf16_lossy(&data.cStreamName[..len]),
            data.StreamSize as u64,
        ));
        // SAFETY: `find` is a live stream search handle.
        if unsafe { FindNextStreamW(find, &mut data as *mut _ as *mut _) }.is_err() {
            break;
        }
    }
    // SAFETY: closes the search handle once.
    unsafe {
        let _ = FindClose(find);
    }
    Ok(out)
}

/// Free bytes on the volume holding `path`.
pub fn free_space(path: &Path) -> WinResult<u64> {
    let w = wide(path);
    let mut free = 0u64;
    // SAFETY: `free` is a live local.
    unsafe { GetDiskFreeSpaceExW(PCWSTR(w.as_ptr()), None, None, Some(&mut free)) }?;
    Ok(free)
}

/// Filesystem name, flags and cluster size of the volume at `root` (such as `N:\`).
pub fn volume(root: &Path) -> WinResult<(String, u32, u64)> {
    let w = wide(root);
    let mut name = [0u16; 64];
    let mut flags = 0u32;
    // SAFETY: the buffers are live locals of the stated sizes.
    unsafe { GetVolumeInformationW(PCWSTR(w.as_ptr()), None, None, None, Some(&mut flags), Some(&mut name)) }?;
    let len = name.iter().position(|&c| c == 0).unwrap_or(0);
    let (mut spc, mut bps) = (0u32, 0u32);
    // SAFETY: as above.
    unsafe { GetDiskFreeSpaceW(PCWSTR(w.as_ptr()), Some(&mut spc), Some(&mut bps), None, None) }?;
    Ok((
        String::from_utf16_lossy(&name[..len]),
        flags,
        u64::from(spc) * u64::from(bps),
    ))
}

pub fn set_attributes(path: &Path, attributes: u32) -> WinResult<()> {
    let w = wide(path);
    // SAFETY: `w` is NUL-terminated.
    unsafe { SetFileAttributesW(PCWSTR(w.as_ptr()), FILE_FLAGS_AND_ATTRIBUTES(attributes)) }?;
    Ok(())
}

pub fn hard_link(new: &Path, existing: &Path) -> WinResult<()> {
    let (n, e) = (wide(new), wide(existing));
    // SAFETY: both paths are NUL-terminated.
    unsafe { CreateHardLinkW(PCWSTR(n.as_ptr()), PCWSTR(e.as_ptr()), None) }?;
    Ok(())
}

pub fn encrypt(path: &Path) -> WinResult<()> {
    let w = wide(path);
    // SAFETY: `w` is NUL-terminated.
    unsafe { EncryptFileW(PCWSTR(w.as_ptr())) }?;
    Ok(())
}
