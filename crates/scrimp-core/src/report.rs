//! What happened in a job, added up, and told in plain words.
//!
//! The worker, the command line and any later window all build their
//! summaries from here, so the numbers and the words are the same
//! everywhere. The sentences follow the "Words people see" table in
//! docs/plan.md section 4.
//!
//! Sizes are shown the way Explorer shows them, in units of 1,024 bytes
//! labelled KB, MB and GB, so Scrimp's numbers match what people see in a
//! file's Properties.

use crate::outcome::{FailReason, Outcome, SkipReason};
use crate::state::UnsupportedReason;

/// Whether the job was compressing or decompressing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Compress,
    Decompress,
}

/// One group of files left alone or not finished, with the sentence that
/// describes them. Groups are reported in this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Group {
    InUse,
    AccessDenied,
    NoSpace,
    Gone,
    OtherFailure,
    AlreadySmall,
    ChangesOften,
    TooSmall,
    Windows,
    CloudFiles,
    Encrypted,
    Links,
    DriveCantDoIt,
    WofNotRunning,
    Unknown,
    FolderUnreadable,
    Cancelled,
}

impl Group {
    /// The sentence for `n` files, as (one file, several files).
    fn sentence(self, n: u64) -> String {
        let count = format_count(n);
        let (one, many) = match self {
            Group::InUse => (
                "1 file is in use by another app.",
                "{n} files are in use by another app.",
            ),
            Group::AccessDenied => (
                "Windows didn't allow changes to 1 file.",
                "Windows didn't allow changes to {n} files.",
            ),
            Group::NoSpace => (
                "1 file couldn't be finished because the drive is full.",
                "{n} files couldn't be finished because the drive is full.",
            ),
            Group::Gone => (
                "1 file was moved or deleted while Scrimp was working.",
                "{n} files were moved or deleted while Scrimp was working.",
            ),
            Group::OtherFailure => ("1 file couldn't be changed.", "{n} files couldn't be changed."),
            Group::AlreadySmall => (
                "1 file is already as small as it can be.",
                "{n} files are already as small as they can be.",
            ),
            Group::ChangesOften => (
                "1 file changes often, so Windows would undo the saving.",
                "{n} files change often, so Windows would undo the saving.",
            ),
            Group::TooSmall => (
                "1 file is too small to be worth compressing.",
                "{n} files are too small to be worth compressing.",
            ),
            Group::Windows => (
                "1 file belongs to Windows and was left alone.",
                "{n} files belong to Windows and were left alone.",
            ),
            Group::CloudFiles => (
                "1 cloud file was left alone, so nothing was downloaded.",
                "{n} cloud files were left alone, so nothing was downloaded.",
            ),
            Group::Encrypted => (
                "1 encrypted file can't be compressed.",
                "{n} encrypted files can't be compressed.",
            ),
            Group::Links => (
                "1 link was left alone. Scrimp never follows links.",
                "{n} links were left alone. Scrimp never follows links.",
            ),
            Group::DriveCantDoIt => (
                "1 file is on a drive that can't save space this way.",
                "{n} files are on a drive that can't save space this way.",
            ),
            Group::WofNotRunning => (
                "1 file wasn't compressed because Windows' compression driver isn't running on its drive. Choose For Files That Change to use NTFS compression there instead.",
                "{n} files weren't compressed because Windows' compression driver isn't running on their drive. Choose For Files That Change to use NTFS compression there instead.",
            ),
            Group::FolderUnreadable => (
                "1 folder couldn't be opened, so the files in it were left alone.",
                "{n} folders couldn't be opened, so the files in them were left alone.",
            ),
            Group::Unknown => (
                "1 file was left alone because Scrimp couldn't tell how it's stored.",
                "{n} files were left alone because Scrimp couldn't tell how they're stored.",
            ),
            Group::Cancelled => ("Stopped before 1 file.", "Stopped before {n} files."),
        };
        if n == 1 {
            one.to_string()
        } else {
            many.replace("{n}", &count)
        }
    }

    fn of_skip(reason: &SkipReason) -> Option<Group> {
        Some(match reason {
            SkipReason::AlreadyCompressedFormat => Group::AlreadySmall,
            SkipReason::ChangesOften => Group::ChangesOften,
            SkipReason::TooSmall => Group::TooSmall,
            SkipReason::ProtectedLocation | SkipReason::WimBacked => Group::Windows,
            SkipReason::StateUnknown => Group::Unknown,
            // The same data under another name is simply counted once.
            SkipReason::SameFileAlreadyDone => return None,
            SkipReason::CantCompressHere(reason) => match reason {
                UnsupportedReason::CloudPlaceholder => Group::CloudFiles,
                UnsupportedReason::Encrypted => Group::Encrypted,
                UnsupportedReason::Link => Group::Links,
                UnsupportedReason::SpecialFile => Group::Unknown,
                UnsupportedReason::WofNotRunning => Group::WofNotRunning,
                UnsupportedReason::FileSystem(_)
                | UnsupportedReason::ClusterTooLarge
                | UnsupportedReason::NetworkLocation => Group::DriveCantDoIt,
            },
        })
    }

    fn of_failure(reason: &FailReason) -> Group {
        match reason {
            FailReason::InUse => Group::InUse,
            FailReason::AccessDenied => Group::AccessDenied,
            FailReason::NotEnoughSpace => Group::NoSpace,
            FailReason::Gone => Group::Gone,
            FailReason::Unsupported => Group::DriveCantDoIt,
            FailReason::Windows { .. } => Group::OtherFailure,
        }
    }
}

/// A job's results, added up file by file.
#[derive(Debug, Clone)]
pub struct Report {
    mode: Mode,
    changed: u64,
    already_done: u64,
    failed: u64,
    before: u64,
    after: u64,
    groups: std::collections::BTreeMap<Group, u64>,
    /// Whether any of the selection is on a disk kept in a file.
    virtual_disk: bool,
}

impl Report {
    pub fn new(mode: Mode) -> Self {
        Report {
            mode,
            changed: 0,
            already_done: 0,
            failed: 0,
            before: 0,
            after: 0,
            groups: Default::default(),
            virtual_disk: false,
        }
    }

    /// Adds one file. `on_disk` is the space it takes, used for files the
    /// job didn't change, so the totals cover the whole selection.
    pub fn add(&mut self, outcome: &Outcome, on_disk: u64) {
        let (before, after) = match outcome {
            Outcome::Compressed { sizes, .. } | Outcome::Decompressed { sizes } => {
                self.changed += 1;
                (sizes.before, sizes.after)
            }
            Outcome::AlreadyDone => {
                self.already_done += 1;
                (on_disk, on_disk)
            }
            Outcome::NotBeneficial => {
                self.count(Group::AlreadySmall);
                (on_disk, on_disk)
            }
            Outcome::Skipped(reason) => {
                if let Some(group) = Group::of_skip(reason) {
                    self.count(group);
                } else {
                    // Counted once already, under its other name.
                    return;
                }
                (on_disk, on_disk)
            }
            Outcome::Failed { reason, .. } => {
                self.failed += 1;
                self.count(Group::of_failure(reason));
                (on_disk, on_disk)
            }
            Outcome::Cancelled => {
                self.count(Group::Cancelled);
                (on_disk, on_disk)
            }
        };
        self.before += before;
        self.after += after;
    }

    /// Notes that some of the selection is on a disk kept in a file, such as
    /// a mounted VHDX, whose file only shrinks once it is compacted.
    pub fn add_virtual_disk(&mut self) {
        self.virtual_disk = true;
    }

    /// Adds a folder that couldn't be opened, so its files were never seen.
    pub fn add_unreadable_folder(&mut self) {
        self.count(Group::FolderUnreadable);
    }

    fn count(&mut self, group: Group) {
        *self.groups.entry(group).or_default() += 1;
    }

    /// Bytes saved across the job. Negative after decompressing.
    pub fn saved(&self) -> i128 {
        i128::from(self.before) - i128::from(self.after)
    }

    /// Space the selection took before the job, and takes now.
    pub fn sizes(&self) -> (u64, u64) {
        (self.before, self.after)
    }

    /// How many files couldn't be changed because of an error.
    pub fn failures(&self) -> u64 {
        self.failed
    }

    /// The summary, one sentence per line. `name` is what was selected,
    /// such as a folder's name, for the headline.
    pub fn summary(&self, name: Option<&str>) -> Vec<String> {
        let mut lines = vec![self.headline(name)];
        if self.already_done > 0 {
            let n = self.already_done;
            lines.push(match (self.mode, n) {
                (Mode::Compress, 1) => "1 file was already compressed.".into(),
                (Mode::Compress, _) => format!("{} files were already compressed.", format_count(n)),
                (Mode::Decompress, 1) => "1 file wasn't compressed.".into(),
                (Mode::Decompress, _) => format!("{} files weren't compressed.", format_count(n)),
            });
        }
        lines.extend(self.groups.iter().map(|(group, n)| group.sentence(*n)));
        if self.virtual_disk && self.changed > 0 {
            lines.push(match self.mode {
                Mode::Compress => "This is a virtual disk (VHDX). The space is saved inside it, but the VHDX file itself only gets smaller once it's compacted.".into(),
                Mode::Decompress => "This is a virtual disk (VHDX), so its VHDX file may grow to make room.".into(),
            });
        }
        lines
    }

    fn headline(&self, name: Option<&str>) -> String {
        let then = format_size(self.before);
        let now = format_size(self.after);
        let it = name.map(|n| format!(" {n} now takes up {now} instead of {then}."));
        match self.mode {
            Mode::Compress if self.changed == 0 => "Nothing needed compressing.".into(),
            Mode::Compress => {
                let saved = format_size(self.before.saturating_sub(self.after));
                format!("Saved {saved}.{}", it.unwrap_or_default())
            }
            Mode::Decompress if self.changed == 0 => "Nothing needed decompressing.".into(),
            Mode::Decompress => {
                let files = if self.changed == 1 {
                    "1 file".to_string()
                } else {
                    format!("{} files", format_count(self.changed))
                };
                format!("Decompressed {files}.{}", it.unwrap_or_default())
            }
        }
    }
}

/// A size as Explorer shows it: `512 bytes`, `4.0 KB`, `35.2 GB`.
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["KB", "MB", "GB", "TB", "PB"];
    if bytes < 1024 {
        return if bytes == 1 {
            "1 byte".into()
        } else {
            format!("{bytes} bytes")
        };
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// A count with thousands separators: `6,000`.
pub fn format_count(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::algorithm::Algorithm;
    use crate::outcome::Sizes;
    use crate::state::CompressionState;

    const GB: u64 = 1024 * 1024 * 1024;

    fn gb(tenths: u64) -> u64 {
        tenths * GB / 10
    }

    fn compressed(before: u64, after: u64) -> Outcome {
        Outcome::Compressed {
            algorithm: Algorithm::Lzx,
            sizes: Sizes {
                logical: before,
                before,
                after,
            },
        }
    }

    #[test]
    fn the_headline_adds_up() {
        let mut r = Report::new(Mode::Compress);
        r.add(&compressed(gb(600), gb(352)), 0);
        r.add(&compressed(gb(200), gb(120)), 0);
        r.add(&Outcome::NotBeneficial, gb(40));
        assert_eq!(r.sizes(), (gb(840), gb(512)));
        assert_eq!(r.saved(), i128::from(gb(840) - gb(512)));
        assert_eq!(
            r.summary(Some("Games")),
            [
                "Saved 32.8 GB. Games now takes up 51.2 GB instead of 84.0 GB.",
                "1 file is already as small as it can be.",
            ]
        );
        assert_eq!(r.summary(None)[0], "Saved 32.8 GB.");
    }

    #[test]
    fn every_kind_of_file_left_alone_has_its_own_sentence() {
        let mut r = Report::new(Mode::Compress);
        let skips = [
            SkipReason::AlreadyCompressedFormat,
            SkipReason::ChangesOften,
            SkipReason::TooSmall,
            SkipReason::ProtectedLocation,
            SkipReason::WimBacked,
            SkipReason::StateUnknown,
            SkipReason::CantCompressHere(UnsupportedReason::CloudPlaceholder),
            SkipReason::CantCompressHere(UnsupportedReason::Encrypted),
            SkipReason::CantCompressHere(UnsupportedReason::Link),
            SkipReason::CantCompressHere(UnsupportedReason::SpecialFile),
            SkipReason::CantCompressHere(UnsupportedReason::FileSystem("exFAT".into())),
            SkipReason::CantCompressHere(UnsupportedReason::ClusterTooLarge),
            SkipReason::CantCompressHere(UnsupportedReason::NetworkLocation),
            SkipReason::CantCompressHere(UnsupportedReason::WofNotRunning),
            SkipReason::SameFileAlreadyDone,
        ];
        for reason in skips {
            r.add(&Outcome::Skipped(reason), 10);
        }
        for reason in [
            FailReason::InUse,
            FailReason::AccessDenied,
            FailReason::NotEnoughSpace,
            FailReason::Gone,
            FailReason::Unsupported,
            FailReason::Windows {
                code: 87,
                message: "The parameter is incorrect.".into(),
            },
        ] {
            r.add(
                &Outcome::Failed {
                    reason,
                    left_as: CompressionState::Uncompressed,
                },
                10,
            );
        }
        r.add(&Outcome::Cancelled, 10);
        r.add(&Outcome::AlreadyDone, 10);
        r.add_unreadable_folder();
        assert_eq!(r.failures(), 6);
        // 22 files of 10 bytes, less the hardlink's second name, counted once.
        assert_eq!(r.sizes(), (220, 220));
        assert_eq!(
            r.summary(None),
            [
                "Nothing needed compressing.",
                "1 file was already compressed.",
                "1 file is in use by another app.",
                "Windows didn't allow changes to 1 file.",
                "1 file couldn't be finished because the drive is full.",
                "1 file was moved or deleted while Scrimp was working.",
                "1 file couldn't be changed.",
                "1 file is already as small as it can be.",
                "1 file changes often, so Windows would undo the saving.",
                "1 file is too small to be worth compressing.",
                "2 files belong to Windows and were left alone.",
                "1 cloud file was left alone, so nothing was downloaded.",
                "1 encrypted file can't be compressed.",
                "1 link was left alone. Scrimp never follows links.",
                "4 files are on a drive that can't save space this way.",
                "1 file wasn't compressed because Windows' compression driver isn't running on its drive. Choose For Files That Change to use NTFS compression there instead.",
                "2 files were left alone because Scrimp couldn't tell how they're stored.",
                "1 folder couldn't be opened, so the files in it were left alone.",
                "Stopped before 1 file.",
            ]
        );
    }

    #[test]
    fn many_files_read_naturally() {
        let mut r = Report::new(Mode::Compress);
        for _ in 0..862 {
            r.add(&Outcome::NotBeneficial, 1);
        }
        for _ in 0..1200 {
            r.add(&Outcome::Cancelled, 1);
        }
        for _ in 0..3 {
            r.add(&Outcome::AlreadyDone, 1);
        }
        assert_eq!(
            r.summary(None)[1..],
            [
                "3 files were already compressed.",
                "862 files are already as small as they can be.",
                "Stopped before 1,200 files.",
            ]
        );
    }

    #[test]
    fn decompressing_says_what_it_did() {
        let mut r = Report::new(Mode::Decompress);
        assert_eq!(r.summary(None), ["Nothing needed decompressing."]);
        let grew = |before, after| Outcome::Decompressed {
            sizes: Sizes {
                logical: after,
                before,
                after,
            },
        };
        r.add(&grew(gb(512), gb(864)), 0);
        assert_eq!(
            r.summary(Some("Games"))[0],
            "Decompressed 1 file. Games now takes up 86.4 GB instead of 51.2 GB."
        );
        r.add(&grew(0, 0), 0);
        r.add(&Outcome::AlreadyDone, 0);
        r.add(&Outcome::AlreadyDone, 0);
        assert_eq!(r.saved(), -i128::from(gb(864) - gb(512)));
        assert_eq!(
            r.summary(None),
            ["Decompressed 2 files.", "2 files weren't compressed."]
        );
        let mut one = Report::new(Mode::Decompress);
        one.add(&Outcome::AlreadyDone, 0);
        assert_eq!(one.summary(None)[1], "1 file wasn't compressed.");
    }

    #[test]
    fn sizes_and_counts_look_like_explorer() {
        assert_eq!(format_size(0), "0 bytes");
        assert_eq!(format_size(1), "1 byte");
        assert_eq!(format_size(1023), "1023 bytes");
        assert_eq!(format_size(4096), "4.0 KB");
        assert_eq!(format_size(gb(352)), "35.2 GB");
        assert_eq!(format_size(3 * 1024 * GB), "3.0 TB");
        assert_eq!(format_size(u64::MAX), "16384.0 PB");
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(6000), "6,000");
        assert_eq!(format_count(1_234_567), "1,234,567");
    }
}
