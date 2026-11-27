//! The engine's view of Windows: `scrimp-core`'s [`Disk`] trait, implemented
//! with the calls in this crate.

use std::path::{Path, PathBuf};

use scrimp_core::{
    Algorithm, Disk, DiskError, EntryKind, FailReason, FileFacts, FileId, Sampled, Storage, VolumeFacts,
};

use crate::error::Error;
use crate::file::{self, FileInfo};
use crate::volume::{self, DriveType};

/// The real disk, for running jobs.
#[derive(Debug, Clone, Copy, Default)]
pub struct WindowsDisk;

fn failure(e: &Error) -> FailReason {
    e.fail_reason().unwrap_or(FailReason::Unsupported)
}

fn io_failure(e: &std::io::Error) -> FailReason {
    failure(&Error::from_io(e))
}

fn disk_error(e: Error) -> DiskError {
    match e {
        Error::NotBeneficial => DiskError::NotBeneficial,
        Error::WofUnavailable => DiskError::WofUnavailable,
        Error::LeftAlone(state) => DiskError::LeftAlone(state),
        other => DiskError::Failed(failure(&other)),
    }
}

fn facts(info: FileInfo) -> FileFacts {
    FileFacts {
        shared_id: match info.id {
            Some((volume, file)) if info.links > 1 => Some(FileId { volume, file }),
            _ => None,
        },
        state: info.state,
        logical: info.logical,
        on_disk: info.on_disk,
    }
}

/// What kind of thing is at a path. Symbolic links and junctions count as
/// links; the standard library recognises both.
fn kind_of(file_type: std::fs::FileType) -> EntryKind {
    if file_type.is_symlink() {
        EntryKind::Link
    } else if file_type.is_dir() {
        EntryKind::Folder
    } else {
        EntryKind::File
    }
}

impl Disk for WindowsDisk {
    fn kind(&self, path: &Path) -> Result<EntryKind, FailReason> {
        std::fs::symlink_metadata(path)
            .map(|m| kind_of(m.file_type()))
            .map_err(|e| io_failure(&e))
    }

    fn list(&self, folder: &Path) -> Result<Vec<(PathBuf, EntryKind)>, FailReason> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(folder).map_err(|e| io_failure(&e))? {
            let entry = entry.map_err(|e| io_failure(&e))?;
            let kind = entry.file_type().map_err(|e| io_failure(&e))?;
            out.push((entry.path(), kind_of(kind)));
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }

    fn volume(&self, path: &Path) -> Result<VolumeFacts, FailReason> {
        let v = volume::volume(path).map_err(|e| failure(&e))?;
        let storage = match (v.drive_type, v.spinning) {
            (DriveType::Removable, _) => Storage::Removable,
            (_, Some(true)) => Storage::Hdd,
            (_, Some(false)) => Storage::Ssd,
            (_, None) => Storage::Unknown,
        };
        Ok(VolumeFacts {
            no_wof: v.unsupported(Algorithm::Lzx),
            no_ntfs: v.unsupported(Algorithm::Lznt1),
            root: v.root,
            storage,
            size: v.size,
            virtual_disk: v.virtual_disk,
        })
    }

    fn free_space(&self, root: &Path) -> Result<u64, FailReason> {
        volume::volume(root).map(|v| v.free_bytes).map_err(|e| failure(&e))
    }

    fn inspect(&self, path: &Path) -> Result<FileFacts, DiskError> {
        file::inspect(path).map(facts).map_err(disk_error)
    }

    fn compress(&self, path: &Path, algorithm: Algorithm) -> Result<FileFacts, DiskError> {
        file::compress(path, algorithm).map(facts).map_err(disk_error)
    }

    fn decompress(&self, path: &Path) -> Result<FileFacts, DiskError> {
        file::decompress(path).map(facts).map_err(disk_error)
    }

    fn sample(&self, path: &Path) -> Result<Sampled, DiskError> {
        crate::sample::sample(path)
            .map(|s| Sampled {
                read: s.read,
                compressed: s.compressed,
            })
            .map_err(disk_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scrimp_core::CompressionState;

    #[test]
    fn errors_become_what_the_engine_expects() {
        assert_eq!(disk_error(Error::NotBeneficial), DiskError::NotBeneficial);
        assert_eq!(disk_error(Error::WofUnavailable), DiskError::WofUnavailable);
        assert_eq!(
            disk_error(Error::LeftAlone(CompressionState::WimBacked)),
            DiskError::LeftAlone(CompressionState::WimBacked)
        );
        assert_eq!(disk_error(Error::InUse), DiskError::Failed(FailReason::InUse));
        assert_eq!(disk_error(Error::NotAFile), DiskError::Failed(FailReason::Unsupported));
    }

    #[test]
    fn errors_with_no_failure_reason_count_as_unsupported() {
        let link = CompressionState::Unsupported(scrimp_core::UnsupportedReason::Link);
        assert_eq!(failure(&Error::LeftAlone(link)), FailReason::Unsupported);
    }
}
