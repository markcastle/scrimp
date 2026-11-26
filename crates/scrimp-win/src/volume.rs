//! What a drive can do.
//!
//! Decided from the filesystem's name, its cluster size and the kind of
//! drive, never from error codes alone. Phase 0 found that ReFS (and so a
//! Dev Drive) answers a WOF request with "would not save space", which would
//! make a whole drive look incompressible if Scrimp trusted the error.

use std::path::{Path, PathBuf};

use scrimp_core::{Algorithm, UnsupportedReason};

use crate::error::Error;
use crate::ffi;

/// NTFS compression needs clusters of this size or smaller.
const NTFS_MAX_CLUSTER: u64 = 4096;
/// `FILE_FILE_COMPRESSION`: the volume supports NTFS compression.
const SUPPORTS_NTFS_COMPRESSION: u32 = 0x10;

/// The kind of drive a volume is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriveType {
    /// An internal drive, or a mounted VHDX.
    Fixed,
    /// A USB stick or memory card.
    Removable,
    /// A network share.
    Network,
    /// Anything else: an optical disc, a RAM disk, or unknown.
    Other,
}

/// What Scrimp needs to know about the volume a file is on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeInfo {
    /// Where the volume starts, such as `C:\`.
    pub root: PathBuf,
    /// The filesystem, such as `NTFS`, `ReFS` or `exFAT`.
    pub filesystem: String,
    /// The cluster size in bytes.
    pub cluster_size: u64,
    /// What kind of drive it is.
    pub drive_type: DriveType,
    /// Free space available to this user, in bytes.
    pub free_bytes: u64,
    /// The volume's total size in bytes.
    pub size: u64,
    /// Whether the drive is a spinning disk, if the device says.
    pub spinning: Option<bool>,
    /// Whether the drive is a disk kept in a file: a mounted VHD or VHDX.
    pub virtual_disk: bool,
    flags: u32,
}

impl VolumeInfo {
    fn is_ntfs(&self) -> bool {
        self.filesystem.eq_ignore_ascii_case("NTFS")
    }

    /// Why this volume can't use `algorithm`, or `None` if it can.
    ///
    /// A `None` for a WOF algorithm doesn't promise that Windows' WOF driver
    /// is attached to the volume; Phase 0 found it isn't for a VHDX mounted
    /// after boot. That shows up as [`Error::WofUnavailable`] on the first
    /// file.
    pub fn unsupported(&self, algorithm: Algorithm) -> Option<UnsupportedReason> {
        if self.drive_type == DriveType::Network {
            return Some(UnsupportedReason::NetworkLocation);
        }
        if !self.is_ntfs() {
            return Some(UnsupportedReason::FileSystem(self.filesystem.clone()));
        }
        let ntfs_ok = self.cluster_size <= NTFS_MAX_CLUSTER && self.flags & SUPPORTS_NTFS_COMPRESSION != 0;
        if !algorithm.is_wof() && !ntfs_ok {
            return Some(UnsupportedReason::ClusterTooLarge);
        }
        None
    }
}

/// Reads what the volume holding `path` supports.
pub fn volume(path: &Path) -> Result<VolumeInfo, Error> {
    let root = plain_root(&ffi::volume_root(path)?);
    let drive_type = match ffi::drive_type(&root) {
        2 => DriveType::Removable,
        3 => DriveType::Fixed,
        4 => DriveType::Network,
        _ => DriveType::Other,
    };
    let (filesystem, flags) = ffi::volume_information(&root)?;
    let (free_bytes, size) = ffi::space(&root)?;
    Ok(VolumeInfo {
        cluster_size: ffi::cluster_size(&root)?,
        spinning: device_of(&root).and_then(|d| ffi::seek_penalty(&d)),
        virtual_disk: device_of(&root).and_then(|d| ffi::file_backed(&d)).unwrap_or(false),
        free_bytes,
        size,
        root,
        filesystem,
        drive_type,
        flags,
    })
}

/// The device path for a volume root: `C:\` becomes `\\.\C:`, and a
/// volume GUID path loses its trailing backslash. A volume mounted in a
/// folder has no device path Scrimp can work out, so `None`.
fn device_of(root: &Path) -> Option<PathBuf> {
    let text = root.to_string_lossy();
    let bytes = text.as_bytes();
    if bytes.len() == 3 && bytes[1] == b':' && bytes[2] == b'\\' && bytes[0].is_ascii_alphabetic() {
        return Some(PathBuf::from(format!(r"\\.\{}", &text[..2])));
    }
    if text.starts_with(r"\\?\Volume{") {
        return Some(PathBuf::from(text.trim_end_matches('\\')));
    }
    None
}

/// `\\?\C:\` becomes `C:\`, because some volume calls don't accept the
/// verbatim form for a drive root. Other roots are kept as they are.
fn plain_root(root: &Path) -> PathBuf {
    let text = root.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if rest.len() == 3 && rest.ends_with(":\\") => PathBuf::from(rest),
        _ => root.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(filesystem: &str, cluster_size: u64, drive_type: DriveType, flags: u32) -> VolumeInfo {
        VolumeInfo {
            root: PathBuf::from(r"D:\"),
            filesystem: filesystem.into(),
            cluster_size,
            drive_type,
            free_bytes: 0,
            size: 0,
            spinning: None,
            virtual_disk: false,
            flags,
        }
    }

    #[test]
    fn ntfs_with_small_clusters_supports_everything() {
        let v = info("NTFS", 4096, DriveType::Fixed, SUPPORTS_NTFS_COMPRESSION);
        assert!(Algorithm::ALL.into_iter().all(|a| v.unsupported(a).is_none()));
    }

    #[test]
    fn large_clusters_rule_out_only_ntfs_compression() {
        let v = info("NTFS", 65536, DriveType::Fixed, 0);
        assert_eq!(
            v.unsupported(Algorithm::Lznt1),
            Some(UnsupportedReason::ClusterTooLarge)
        );
        assert_eq!(v.unsupported(Algorithm::Lzx), None);
    }

    #[test]
    fn other_filesystems_and_network_drives_support_nothing() {
        for fs in ["ReFS", "exFAT", "FAT32"] {
            let v = info(fs, 4096, DriveType::Fixed, 0);
            for a in Algorithm::ALL {
                assert_eq!(v.unsupported(a), Some(UnsupportedReason::FileSystem(fs.into())), "{fs}");
            }
        }
        let share = info("NTFS", 4096, DriveType::Network, SUPPORTS_NTFS_COMPRESSION);
        assert_eq!(
            share.unsupported(Algorithm::Lzx),
            Some(UnsupportedReason::NetworkLocation)
        );
    }

    #[test]
    fn device_paths_are_worked_out_from_roots() {
        assert_eq!(device_of(Path::new(r"C:\")), Some(PathBuf::from(r"\\.\C:")));
        assert_eq!(
            device_of(Path::new(r"\\?\Volume{0000}\")),
            Some(PathBuf::from(r"\\?\Volume{0000}"))
        );
        assert_eq!(device_of(Path::new(r"C:\Mounts\Archive\")), None);
    }

    #[test]
    fn verbatim_drive_roots_are_made_plain() {
        assert_eq!(plain_root(Path::new(r"\\?\C:\")), Path::new(r"C:\"));
        assert_eq!(plain_root(Path::new(r"C:\")), Path::new(r"C:\"));
        assert_eq!(
            plain_root(Path::new(r"C:\Mounts\Archive\")),
            Path::new(r"C:\Mounts\Archive\")
        );
        assert_eq!(
            plain_root(Path::new(r"\\?\Volume{0000}\")),
            Path::new(r"\\?\Volume{0000}\")
        );
    }
}
