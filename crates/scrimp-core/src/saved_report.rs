//! The report **Save Report…** writes: one row per file, as CSV, so it
//! opens in Excel or any spreadsheet and can be sorted and filtered.
//!
//! The summary says what happened in a few sentences; this is the detail
//! behind it, technical names included, for the person who wants to know
//! exactly what happened to which file. It is built from the typed
//! outcomes, never from the summary's words.
//!
//! The text is UTF-8 with a byte order mark, which is how Excel knows to
//! read names such as `写真.jpg` correctly.

use std::path::Path;

use crate::outcome::{FailReason, Outcome, SkipReason};
use crate::state::CompressionState;

/// The first line of the file: a byte order mark and the column names.
pub const CSV_HEADER: &str =
    "\u{feff}Path,Result,Method,Size (bytes),On disk before (bytes),On disk after (bytes),Why\r\n";

/// The row for one file. `on_disk` is the space it takes now, for files
/// that weren't changed.
pub fn csv_row(path: &Path, outcome: &Outcome, on_disk: u64) -> String {
    let (result, method, sizes, why) = match outcome {
        Outcome::Compressed { algorithm, sizes } => (
            "Compressed",
            algorithm.technical_name().to_string(),
            [Some(sizes.logical), Some(sizes.before), Some(sizes.after)],
            String::new(),
        ),
        Outcome::Decompressed { sizes } => (
            "Decompressed",
            String::new(),
            [Some(sizes.logical), Some(sizes.before), Some(sizes.after)],
            String::new(),
        ),
        Outcome::AlreadyDone => ("Already done", String::new(), unchanged(on_disk), String::new()),
        Outcome::NotBeneficial => (
            "Left alone",
            String::new(),
            unchanged(on_disk),
            "Windows found it wouldn't get any smaller".into(),
        ),
        Outcome::Skipped(reason) => ("Left alone", String::new(), unchanged(on_disk), skip_why(reason)),
        Outcome::Failed { reason, left_as } => (
            "Couldn't change",
            state_method(left_as),
            unchanged(on_disk),
            fail_why(reason),
        ),
        Outcome::Cancelled => (
            "Not reached",
            String::new(),
            [None, None, None],
            "The job was stopped first".into(),
        ),
    };
    let number = |n: Option<u64>| n.map_or_else(String::new, |n| n.to_string());
    format!(
        "{},{},{},{},{},{},{}\r\n",
        field(&path.display().to_string()),
        result,
        field(&method),
        number(sizes[0]),
        number(sizes[1]),
        number(sizes[2]),
        field(&why)
    )
}

/// The row for a folder that couldn't be opened, so the files in it were
/// left alone.
pub fn csv_folder_row(path: &Path, reason: &FailReason) -> String {
    format!(
        "{},Couldn't open folder,,,,,{}\r\n",
        field(&path.display().to_string()),
        field(&fail_why(reason))
    )
}

/// A file Scrimp didn't change takes the same room before and after; its
/// full size isn't known without another look, so it's left blank.
fn unchanged(on_disk: u64) -> [Option<u64>; 3] {
    [None, Some(on_disk), Some(on_disk)]
}

fn state_method(state: &CompressionState) -> String {
    match state {
        CompressionState::Compressed(a) => a.technical_name().to_string(),
        _ => String::new(),
    }
}

fn sentence(text: &str) -> String {
    let mut chars = text.chars();
    let mut out: String = chars.next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
    out.push_str(chars.as_str());
    out.trim_end_matches('.').to_string()
}

fn skip_why(reason: &SkipReason) -> String {
    match reason {
        SkipReason::AlreadyCompressedFormat => "Already compressed, as photos, videos and archives are".into(),
        SkipReason::ChangesOften => "It changes often, so Windows would undo the saving".into(),
        SkipReason::TooSmall => "Too small to be worth compressing".into(),
        SkipReason::ProtectedLocation => "In a folder Scrimp protects, such as Windows itself".into(),
        SkipReason::WimBacked => "A Windows system file".into(),
        SkipReason::CantCompressHere(r) => sentence(&r.plain()),
        SkipReason::SameFileAlreadyDone => "Another name for a file already handled".into(),
        SkipReason::StateUnknown => "Scrimp couldn't tell how it's stored".into(),
    }
}

fn fail_why(reason: &FailReason) -> String {
    sentence(&reason.plain())
}

/// A CSV field, quoted when it needs to be. A field starting with a
/// character a spreadsheet would treat as a formula gets a leading
/// apostrophe, so a file named `=SUM(A1).txt` can't run anything.
fn field(text: &str) -> String {
    let safe = if text.starts_with(['=', '+', '-', '@']) {
        format!("'{text}")
    } else {
        text.to_string()
    };
    if safe.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", safe.replace('"', "\"\""))
    } else {
        safe
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::algorithm::Algorithm;
    use crate::outcome::Sizes;
    use crate::state::UnsupportedReason;

    fn row(path: &str, outcome: Outcome, on_disk: u64) -> String {
        csv_row(Path::new(path), &outcome, on_disk)
    }

    #[test]
    fn each_outcome_has_its_row() {
        let sizes = Sizes {
            logical: 2048,
            before: 2048,
            after: 1024,
        };
        assert_eq!(
            row(
                r"D:\Games\notes.txt",
                Outcome::Compressed {
                    algorithm: Algorithm::Lzx,
                    sizes
                },
                1024
            ),
            "D:\\Games\\notes.txt,Compressed,LZX,2048,2048,1024,\r\n"
        );
        assert_eq!(
            row(r"D:\Games\notes.txt", Outcome::Decompressed { sizes }, 0),
            "D:\\Games\\notes.txt,Decompressed,,2048,2048,1024,\r\n"
        );
        assert_eq!(
            row(
                r"D:\Games\intro.mp4",
                Outcome::Skipped(SkipReason::AlreadyCompressedFormat),
                900
            ),
            "D:\\Games\\intro.mp4,Left alone,,,900,900,\"Already compressed, as photos, videos and archives are\"\r\n"
        );
        assert_eq!(
            row(
                r"D:\Games\link",
                Outcome::Skipped(SkipReason::CantCompressHere(UnsupportedReason::Link)),
                0
            ),
            "D:\\Games\\link,Left alone,,,0,0,\"It's a link, and Scrimp never follows links\"\r\n"
        );
        assert_eq!(
            row(
                r"D:\Games\save.dat",
                Outcome::Failed {
                    reason: FailReason::InUse,
                    left_as: CompressionState::Compressed(Algorithm::Xpress4k)
                },
                50
            ),
            "D:\\Games\\save.dat,Couldn't change,XPRESS4K,,50,50,It's in use by another app\r\n"
        );
        assert!(row(r"D:\a", Outcome::NotBeneficial, 1).contains("Left alone"));
        assert!(row(r"D:\a", Outcome::AlreadyDone, 1).contains("Already done"));
        assert_eq!(
            row(r"D:\a", Outcome::Cancelled, 1),
            "D:\\a,Not reached,,,,,The job was stopped first\r\n"
        );
        assert_eq!(
            csv_folder_row(Path::new(r"D:\Games\Private"), &FailReason::AccessDenied),
            "D:\\Games\\Private,Couldn't open folder,,,,,Windows didn't allow it. Running as administrator may help\r\n"
        );
    }

    #[test]
    fn every_reason_reads_as_a_sentence() {
        for reason in [
            SkipReason::AlreadyCompressedFormat,
            SkipReason::ChangesOften,
            SkipReason::TooSmall,
            SkipReason::ProtectedLocation,
            SkipReason::WimBacked,
            SkipReason::SameFileAlreadyDone,
            SkipReason::StateUnknown,
            SkipReason::CantCompressHere(UnsupportedReason::FileSystem("exFAT".into())),
        ] {
            let why = skip_why(&reason);
            assert!(why.starts_with(char::is_uppercase) && !why.ends_with('.'), "{why}");
        }
    }

    #[test]
    fn odd_names_stay_one_field_and_never_become_formulas() {
        assert_eq!(field("plain"), "plain");
        assert_eq!(field(r#"say "hi", twice"#), r#""say ""hi"", twice""#);
        assert_eq!(field("line\nbreak"), "\"line\nbreak\"");
        assert_eq!(field("=SUM(A1)"), "'=SUM(A1)");
        assert_eq!(field("-x,y"), "\"'-x,y\"");
        assert!(CSV_HEADER.starts_with('\u{feff}'));
        assert_eq!(CSV_HEADER.matches(',').count(), 6, "seven columns, as every row has");
    }
}
