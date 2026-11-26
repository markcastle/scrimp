//! Reading, compressing and decompressing one file, following the Phase 0
//! findings in `docs/windows-api.md`.
//!
//! Each change is checked by reading the file back: a change is only
//! reported as done when the file is in the state that was asked for.

use std::os::windows::fs::MetadataExt;
use std::path::Path;

use scrimp_core::{Algorithm, CompressionState, Target, UnsupportedReason};

use crate::error::Error;
use crate::ffi::{self, Access, FORMAT_LZNT1, FORMAT_NONE, Handle, WofReading};

const ATTRIBUTE_READONLY: u32 = 0x1;
const ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
const ATTRIBUTE_OFFLINE: u32 = 0x1000;
const ATTRIBUTE_ENCRYPTED: u32 = 0x4000;
const ATTRIBUTE_RECALL_ON_OPEN: u32 = 0x4_0000;
const ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x40_0000;

const TAG_WOF: u32 = 0x8000_0017;
const TAG_MOUNT_POINT: u32 = 0xA000_0003;
const TAG_SYMLINK: u32 = 0xA000_000C;
/// Cloud file tags (OneDrive and others) share these bits.
const TAG_CLOUD_MASK: u32 = 0xFFFF_0FFF;
const TAG_CLOUD: u32 = 0x9000_001A;

/// A file's compression and sizes, as read from Windows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileInfo {
    /// The file's compression, as Scrimp sees it.
    pub state: CompressionState,
    /// The file's real size in bytes, which compression never changes.
    pub logical: u64,
    /// The space it takes on disk, in bytes. For files Scrimp leaves alone
    /// without looking inside (links, cloud files), this is the logical size.
    pub on_disk: u64,
    /// Whether the file is marked read-only. Scrimp keeps it that way.
    pub read_only: bool,
    /// How many names the file has. More than one means a hardlink.
    pub links: u32,
    /// The file's identity on its drive (volume serial number, file index),
    /// the same under each of its names. `None` for files left alone
    /// without being opened.
    pub id: Option<(u64, u64)>,
    raw: Raw,
}

/// What Windows reported, before it is reduced to a [`CompressionState`].
/// A file can have both WOF and NTFS compression at once, which the state
/// shows only as "unknown" but decompression needs to undo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Raw {
    wof: WofReading,
    ntfs: bool,
}

/// Reads a file's compression and sizes without changing anything.
///
/// Links, cloud files and encrypted files are recognised from their
/// attributes and reported as unsupported without being opened, so reading
/// them never follows a link or downloads anything.
pub fn inspect(path: &Path) -> Result<FileInfo, Error> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| Error::from_io(&e))?;
    if meta.is_dir() {
        return Err(Error::NotAFile);
    }
    let attributes = meta.file_attributes();
    let logical = meta.len();
    let read_only = attributes & ATTRIBUTE_READONLY != 0;
    let untouched = |reason| FileInfo {
        state: CompressionState::Unsupported(reason),
        logical,
        on_disk: logical,
        read_only,
        links: 1,
        id: None,
        raw: Raw {
            wof: WofReading::None,
            ntfs: false,
        },
    };

    if attributes & (ATTRIBUTE_OFFLINE | ATTRIBUTE_RECALL_ON_OPEN | ATTRIBUTE_RECALL_ON_DATA_ACCESS) != 0 {
        return Ok(untouched(UnsupportedReason::CloudPlaceholder));
    }
    if attributes & ATTRIBUTE_REPARSE_POINT != 0 {
        let tag = ffi::reparse_tag(path)?;
        if tag & TAG_CLOUD_MASK == TAG_CLOUD {
            return Ok(untouched(UnsupportedReason::CloudPlaceholder));
        }
        if tag == TAG_SYMLINK || tag == TAG_MOUNT_POINT {
            return Ok(untouched(UnsupportedReason::Link));
        }
        if tag != TAG_WOF {
            return Ok(untouched(UnsupportedReason::SpecialFile));
        }
    }
    if attributes & ATTRIBUTE_ENCRYPTED != 0 {
        return Ok(untouched(UnsupportedReason::Encrypted));
    }

    let wof = ffi::wof_reading(path)?;
    let handle = Handle::open(path, Access::Inspect)?;
    let ntfs = ffi::ntfs_format(&handle)? != FORMAT_NONE;
    let (volume, file, links) = ffi::identity(&handle)?;
    let raw = Raw { wof, ntfs };
    Ok(FileInfo {
        state: state_of(raw),
        logical,
        on_disk: ffi::on_disk(path)?,
        read_only,
        links,
        id: Some((volume, file)),
        raw,
    })
}

fn state_of(raw: Raw) -> CompressionState {
    match (raw.wof, raw.ntfs) {
        (WofReading::None, false) => CompressionState::Uncompressed,
        (WofReading::None, true) => CompressionState::Compressed(Algorithm::Lznt1),
        (WofReading::File(id), false) => {
            algorithm_of(id).map_or(CompressionState::Unknown, CompressionState::Compressed)
        }
        // Windows' own system image files.
        (WofReading::Provider(1), _) => CompressionState::WimBacked,
        // Both kinds at once, an unknown WOF algorithm, or an unknown provider.
        _ => CompressionState::Unknown,
    }
}

/// WOF's algorithm numbers: XPRESS4K 0, LZX 1, XPRESS8K 2, XPRESS16K 3.
fn wof_id(algorithm: Algorithm) -> Option<u32> {
    match algorithm {
        Algorithm::Xpress4k => Some(0),
        Algorithm::Lzx => Some(1),
        Algorithm::Xpress8k => Some(2),
        Algorithm::Xpress16k => Some(3),
        Algorithm::Lznt1 => None,
    }
}

fn algorithm_of(id: u32) -> Option<Algorithm> {
    Algorithm::ALL.into_iter().find(|a| wof_id(*a) == Some(id))
}

/// Compresses a file with `algorithm`, replacing any compression it has.
///
/// Returns the file as it is afterwards. A file already compressed this way
/// is returned unchanged. When Windows finds the file wouldn't shrink the
/// result is [`Error::NotBeneficial`]; changing from NTFS compression to WOF
/// on such a file leaves it uncompressed, because NTFS compression has to be
/// removed before WOF is tried.
pub fn compress(path: &Path, algorithm: Algorithm) -> Result<FileInfo, Error> {
    let before = inspect(path)?;
    if !before.state.can_change() {
        return Err(Error::LeftAlone(before.state));
    }
    if before.state.already_is(Target::Algorithm(algorithm)) {
        return Ok(before);
    }

    match wof_id(algorithm) {
        Some(id) => {
            // WOF on top of NTFS compression leaves both, so NTFS goes first.
            if before.raw.ntfs {
                set_ntfs(path, FORMAT_NONE, before.read_only)?;
            }
            ffi::wof_set(&Handle::open(path, Access::Wof)?, id)?;
        }
        None => {
            // Opening a WOF file for writing would undo it anyway; do it
            // explicitly, with read-only access, so it is clear what happened.
            if before.raw.wof != WofReading::None {
                ffi::wof_remove(&Handle::open(path, Access::Wof)?)?;
            }
            let uncompressed = ffi::on_disk(path)?;
            set_ntfs(path, FORMAT_LZNT1, before.read_only)?;
            // NTFS compression "succeeds" even when it saves nothing, and
            // the file then pays its costs for no benefit. Undo it.
            if ffi::on_disk(path)? >= uncompressed {
                set_ntfs(path, FORMAT_NONE, before.read_only)?;
                return Err(Error::NotBeneficial);
            }
        }
    }
    confirm(path, CompressionState::Compressed(algorithm))
}

/// Removes any compression Scrimp can change, including the case where a
/// file has both WOF and NTFS compression. An uncompressed file is returned
/// unchanged.
pub fn decompress(path: &Path) -> Result<FileInfo, Error> {
    let before = inspect(path)?;
    let both = before.raw.ntfs && matches!(before.raw.wof, WofReading::File(_));
    if !before.state.can_change() && !both {
        return Err(Error::LeftAlone(before.state));
    }
    if matches!(before.raw.wof, WofReading::File(_)) {
        ffi::wof_remove(&Handle::open(path, Access::Wof)?)?;
    }
    if before.raw.ntfs {
        set_ntfs(path, FORMAT_NONE, before.read_only)?;
    }
    confirm(path, CompressionState::Uncompressed)
}

/// Reads the file back and checks it is in the state that was asked for.
fn confirm(path: &Path, wanted: CompressionState) -> Result<FileInfo, Error> {
    let after = inspect(path)?;
    if after.state == wanted {
        Ok(after)
    } else {
        Err(Error::NotApplied(after.state))
    }
}

/// Sets NTFS compression, which needs write access. A read-only file has
/// the attribute cleared for the change and put back afterwards, on every
/// path out. Only call this on a file with no WOF compression.
fn set_ntfs(path: &Path, format: u16, read_only: bool) -> Result<(), Error> {
    let guard = if read_only {
        Some(ReadOnlyCleared::new(path)?)
    } else {
        None
    };
    let result = Handle::open(path, Access::Ntfs).and_then(|h| ffi::ntfs_set(&h, format));
    match guard {
        Some(g) => {
            let restored = g.restore();
            result.and(restored)
        }
        None => result,
    }
}

/// Clears the read-only attribute while it exists, and puts it back when
/// [`restore`](Self::restore) is called or, failing that, when dropped.
struct ReadOnlyCleared<'a> {
    path: &'a Path,
    original: u32,
    restored: bool,
}

impl<'a> ReadOnlyCleared<'a> {
    fn new(path: &'a Path) -> Result<Self, Error> {
        let original = std::fs::symlink_metadata(path)
            .map_err(|e| Error::from_io(&e))?
            .file_attributes();
        ffi::set_attributes(path, original & !ATTRIBUTE_READONLY)?;
        Ok(ReadOnlyCleared {
            path,
            original,
            restored: false,
        })
    }

    fn restore(mut self) -> Result<(), Error> {
        self.restored = true;
        ffi::set_attributes(self.path, self.original)
    }
}

impl Drop for ReadOnlyCleared<'_> {
    fn drop(&mut self) {
        if !self.restored {
            let _ = ffi::set_attributes(self.path, self.original);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wof_algorithm_numbers_round_trip() {
        for a in Algorithm::ALL {
            match wof_id(a) {
                Some(id) => assert_eq!(algorithm_of(id), Some(a)),
                None => assert_eq!(a, Algorithm::Lznt1),
            }
        }
        assert_eq!(algorithm_of(4), None);
    }

    #[test]
    fn both_kinds_at_once_reads_as_unknown() {
        let raw = |wof, ntfs| state_of(Raw { wof, ntfs });
        assert_eq!(raw(WofReading::None, false), CompressionState::Uncompressed);
        assert_eq!(
            raw(WofReading::None, true),
            CompressionState::Compressed(Algorithm::Lznt1)
        );
        assert_eq!(
            raw(WofReading::File(1), false),
            CompressionState::Compressed(Algorithm::Lzx)
        );
        assert_eq!(raw(WofReading::File(1), true), CompressionState::Unknown);
        assert_eq!(raw(WofReading::File(9), false), CompressionState::Unknown);
        assert_eq!(raw(WofReading::Provider(1), false), CompressionState::WimBacked);
        assert_eq!(raw(WofReading::Provider(3), false), CompressionState::Unknown);
    }

    /// Phase 0 found that WOF applied on top of NTFS compression leaves both
    /// in place. Scrimp's own `compress` never does that, but a file could
    /// arrive like it, so it must read as unknown, be refused by `compress`,
    /// and be fully undone by `decompress`.
    #[test]
    fn a_file_with_both_kinds_is_left_alone_until_decompressed() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("both.txt");
        let data: Vec<u8> = "Row 1: Example.dat moved to D:\\Archive.\n".repeat(30_000).into_bytes();
        std::fs::write(&p, &data).unwrap();
        ffi::ntfs_set(&Handle::open(&p, Access::Ntfs).unwrap(), FORMAT_LZNT1).unwrap();
        ffi::wof_set(&Handle::open(&p, Access::Wof).unwrap(), 1).unwrap();

        assert_eq!(inspect(&p).unwrap().state, CompressionState::Unknown);
        assert_eq!(
            compress(&p, Algorithm::Lzx),
            Err(Error::LeftAlone(CompressionState::Unknown))
        );
        assert_eq!(decompress(&p).unwrap().state, CompressionState::Uncompressed);
        assert_eq!(std::fs::read(&p).unwrap(), data);
    }

    #[test]
    fn windows_own_image_files_are_refused() {
        let wim = FileInfo {
            state: state_of(Raw {
                wof: WofReading::Provider(1),
                ntfs: false,
            }),
            logical: 0,
            on_disk: 0,
            read_only: false,
            links: 1,
            id: None,
            raw: Raw {
                wof: WofReading::Provider(1),
                ntfs: false,
            },
        };
        assert_eq!(wim.state, CompressionState::WimBacked);
        assert!(!wim.state.can_change());
    }
}
