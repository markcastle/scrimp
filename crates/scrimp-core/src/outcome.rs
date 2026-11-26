//! What happened to one file, as a typed result.
//!
//! The menu, the worker and the command line all report from these, never
//! from strings, so "not beneficial" can never be shown as a failure and a
//! failure can never be shown as a success.

use crate::algorithm::Algorithm;

/// A file's sizes in bytes: how big it is, and how much disk it used before
/// and after.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sizes {
    /// The file's real size, which compression never changes.
    pub logical: u64,
    /// Space on disk before the operation.
    pub before: u64,
    /// Space on disk after it.
    pub after: u64,
}

impl Sizes {
    /// Space saved, in bytes. Negative when the file now takes more room,
    /// which is what decompressing does.
    pub fn saved(&self) -> i128 {
        i128::from(self.before) - i128::from(self.after)
    }
}

/// Why a file was left alone. None of these is a failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    /// Already compressed media or archives (photos, videos, ZIP files).
    AlreadyCompressedFormat,
    /// A file that changes often, so Windows would soon undo the saving.
    ChangesOften,
    /// Smaller than the minimum size worth compressing.
    TooSmall,
    /// In a folder Scrimp protects, such as Windows itself.
    ProtectedLocation,
    /// A Windows system file backed by an image file.
    WimBacked,
    /// A cloud file that isn't on this computer yet.
    CloudPlaceholder,
    /// A link to another file or folder, which Scrimp doesn't follow.
    Link,
    /// Another name for a file already handled in this job (a hardlink).
    SameFileAlreadyDone,
    /// The file's state couldn't be read, so it was left alone.
    StateUnknown,
}

/// Why changing a file failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailReason {
    /// Windows didn't allow the change. Running as administrator may help.
    AccessDenied,
    /// Another app has the file open.
    InUse,
    /// Not enough free space to finish.
    NotEnoughSpace,
    /// The file was moved or deleted while the job ran.
    Gone,
    /// The drive doesn't support this kind of compression.
    Unsupported,
    /// Windows reported an error Scrimp doesn't have a plain explanation for.
    Windows { code: u32, message: String },
}

/// What happened to one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Compressed with this method.
    Compressed { algorithm: Algorithm, sizes: Sizes },
    /// Compression removed.
    Decompressed { sizes: Sizes },
    /// Already exactly as asked; nothing was done.
    AlreadyDone,
    /// Windows tried and found the file wouldn't get smaller. A normal result.
    NotBeneficial,
    /// Left alone on purpose.
    Skipped(SkipReason),
    /// The change failed. The file is as `left_as` says, which after a failed
    /// change of method can be uncompressed.
    Failed {
        reason: FailReason,
        left_as: crate::state::CompressionState,
    },
    /// The job was cancelled before this file was reached.
    Cancelled,
}

impl Outcome {
    /// Whether this file counts against the job. Skips, "not beneficial" and
    /// "already done" never do.
    pub fn is_failure(&self) -> bool {
        matches!(self, Outcome::Failed { .. })
    }

    /// Bytes saved by this file, if anything changed on disk.
    pub fn saved(&self) -> i128 {
        match self {
            Outcome::Compressed { sizes, .. } | Outcome::Decompressed { sizes } => sizes.saved(),
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::CompressionState;

    const GB: u64 = 1_000_000_000;

    #[test]
    fn savings_add_up_both_ways() {
        let compressed = Sizes {
            logical: 864 * GB / 10,
            before: 864 * GB / 10,
            after: 512 * GB / 10,
        };
        assert_eq!(compressed.saved(), i128::from(352 * GB / 10));
        let decompressed = Sizes {
            logical: 4 * GB,
            before: GB,
            after: 4 * GB,
        };
        assert_eq!(decompressed.saved(), -i128::from(3 * GB));
        let huge = Sizes {
            logical: u64::MAX,
            before: 0,
            after: u64::MAX,
        };
        assert_eq!(huge.saved(), -i128::from(u64::MAX), "no overflow at the extremes");
    }

    #[test]
    fn only_a_real_failure_counts_as_one() {
        let fine = [
            Outcome::AlreadyDone,
            Outcome::NotBeneficial,
            Outcome::Skipped(SkipReason::ChangesOften),
            Outcome::Cancelled,
            Outcome::Decompressed {
                sizes: Sizes {
                    logical: 10,
                    before: 4,
                    after: 10,
                },
            },
        ];
        assert!(fine.iter().all(|o| !o.is_failure()));
        let failed = Outcome::Failed {
            reason: FailReason::InUse,
            left_as: CompressionState::Uncompressed,
        };
        assert!(failed.is_failure());
        assert_eq!(failed.saved(), 0);
    }

    #[test]
    fn a_failed_change_of_method_says_what_the_file_was_left_as() {
        let failed = Outcome::Failed {
            reason: FailReason::NotEnoughSpace,
            left_as: CompressionState::Uncompressed,
        };
        let Outcome::Failed { left_as, .. } = failed else {
            unreachable!()
        };
        assert_eq!(left_as, CompressionState::Uncompressed);
    }

    #[test]
    fn saved_comes_from_the_sizes() {
        let o = Outcome::Compressed {
            algorithm: Algorithm::Lzx,
            sizes: Sizes {
                logical: 100,
                before: 100,
                after: 40,
            },
        };
        assert_eq!(o.saved(), 60);
        assert_eq!(Outcome::NotBeneficial.saved(), 0);
    }
}
