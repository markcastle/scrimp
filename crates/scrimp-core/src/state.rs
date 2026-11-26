//! What state a file's compression is in, as far as Scrimp can tell.
//!
//! The important rule lives here: "unknown" is never treated as
//! "uncompressed". If Scrimp can't tell what a file is, it leaves it alone.

use crate::algorithm::{Algorithm, Target};

/// A file's compression, as read from Windows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompressionState {
    /// No filesystem compression.
    Uncompressed,
    /// Compressed with one of the methods Scrimp can change.
    Compressed(Algorithm),
    /// Backed by a Windows image file (WIM). These are Windows' own system
    /// files and are never changed by Scrimp.
    WimBacked,
    /// The file can't be compressed where it is, and why.
    Unsupported(UnsupportedReason),
    /// Scrimp couldn't tell. The file is left alone.
    Unknown,
}

/// Why a file can't be compressed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnsupportedReason {
    /// The drive isn't NTFS (FAT32, exFAT, or ReFS such as a Dev Drive).
    FileSystem(String),
    /// NTFS compression needs clusters of 4 KB or smaller.
    ClusterTooLarge,
    /// On a network location, which Scrimp leaves alone.
    NetworkLocation,
    /// Encrypted with Windows' file encryption (EFS), which can't be combined with compression.
    Encrypted,
    /// A cloud file (OneDrive and similar). Compressing it could download it,
    /// or upset the sync app that owns it.
    CloudPlaceholder,
    /// A symbolic link or junction. Scrimp never follows links.
    Link,
    /// Another special kind of file Windows manages itself, such as a
    /// deduplicated one.
    SpecialFile,
    /// Windows' compression driver (WOF) isn't running on this drive, which
    /// happens with a VHDX mounted after boot. NTFS compression still works.
    WofNotRunning,
}

impl UnsupportedReason {
    /// Why, in plain words, to follow "left alone, because ".
    pub fn plain(&self) -> String {
        match self {
            UnsupportedReason::FileSystem(fs) => format!("it's on a {fs} drive, which can't save space this way"),
            UnsupportedReason::ClusterTooLarge => "its drive can't use NTFS compression".into(),
            UnsupportedReason::NetworkLocation => "it's on a network location".into(),
            UnsupportedReason::Encrypted => "it's encrypted".into(),
            UnsupportedReason::CloudPlaceholder => "it's a cloud file, and compressing it could download it".into(),
            UnsupportedReason::Link => "it's a link, and Scrimp never follows links".into(),
            UnsupportedReason::SpecialFile => "it's a special file Windows manages itself".into(),
            UnsupportedReason::WofNotRunning => {
                "Windows' compression driver isn't running on its drive (NTFS compression still works there)".into()
            }
        }
    }
}

impl CompressionState {
    /// Whether Scrimp may change this file at all.
    pub fn can_change(&self) -> bool {
        matches!(self, CompressionState::Uncompressed | CompressionState::Compressed(_))
    }

    /// Whether the file is already exactly what was asked for, so there is
    /// nothing to do. Automatic is never "already": the policy decides per file.
    pub fn already_is(&self, target: Target) -> bool {
        match (self, target) {
            (CompressionState::Uncompressed, Target::Uncompressed) => true,
            (CompressionState::Compressed(have), Target::Algorithm(want)) => *have == want,
            _ => false,
        }
    }

    /// Whether the file is compressed in a way Scrimp Decompress can undo.
    pub fn is_compressed(&self) -> bool {
        matches!(self, CompressionState::Compressed(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_is_never_treated_as_uncompressed() {
        let unknown = CompressionState::Unknown;
        assert!(!unknown.can_change());
        assert!(!unknown.already_is(Target::Uncompressed));
        assert!(!unknown.is_compressed());
    }

    #[test]
    fn windows_own_files_and_unsupported_ones_are_left_alone() {
        assert!(!CompressionState::WimBacked.can_change());
        for reason in [
            UnsupportedReason::FileSystem("exFAT".into()),
            UnsupportedReason::ClusterTooLarge,
            UnsupportedReason::NetworkLocation,
            UnsupportedReason::Encrypted,
            UnsupportedReason::CloudPlaceholder,
            UnsupportedReason::Link,
            UnsupportedReason::SpecialFile,
            UnsupportedReason::WofNotRunning,
        ] {
            assert!(!CompressionState::Unsupported(reason).can_change());
        }
    }

    #[test]
    fn already_done_means_exactly_what_was_asked() {
        let lzx = CompressionState::Compressed(Algorithm::Lzx);
        assert!(lzx.already_is(Target::Algorithm(Algorithm::Lzx)));
        assert!(!lzx.already_is(Target::Algorithm(Algorithm::Xpress8k)));
        assert!(!lzx.already_is(Target::Uncompressed));
        assert!(!lzx.already_is(Target::Automatic), "automatic decides per file");
        assert!(CompressionState::Uncompressed.already_is(Target::Uncompressed));
        assert!(!CompressionState::Uncompressed.already_is(Target::Automatic));
    }

    #[test]
    fn only_files_scrimp_can_change_count_as_compressed() {
        assert!(CompressionState::Compressed(Algorithm::Lznt1).is_compressed());
        assert!(!CompressionState::Uncompressed.is_compressed());
        assert!(!CompressionState::WimBacked.is_compressed());
    }
}
