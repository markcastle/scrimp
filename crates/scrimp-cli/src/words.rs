//! What the command line says about one file, in plain words, and the names
//! its JSON output uses. The summary of a whole job comes from
//! `scrimp_core::Report`, so those words match everywhere.

use std::path::Path;

use scrimp_core::{Algorithm, CompressionState, FailReason, Outcome, SkipReason, UnsupportedReason, format_size};

/// A state as the JSON output names it.
pub fn state_name(state: &CompressionState) -> &'static str {
    match state {
        CompressionState::Uncompressed => "uncompressed",
        CompressionState::Compressed(a) => a.cli_name(),
        CompressionState::WimBacked => "wimBacked",
        CompressionState::Unsupported(_) => "unsupported",
        CompressionState::Unknown => "unknown",
    }
}

pub fn unsupported_name(reason: &UnsupportedReason) -> &'static str {
    match reason {
        UnsupportedReason::FileSystem(_) => "fileSystem",
        UnsupportedReason::ClusterTooLarge => "clusterTooLarge",
        UnsupportedReason::NetworkLocation => "networkLocation",
        UnsupportedReason::Encrypted => "encrypted",
        UnsupportedReason::CloudPlaceholder => "cloudFile",
        UnsupportedReason::Link => "link",
        UnsupportedReason::SpecialFile => "specialFile",
        UnsupportedReason::WofNotRunning => "wofNotRunning",
    }
}

pub fn skip_name(reason: &SkipReason) -> &'static str {
    match reason {
        SkipReason::AlreadyCompressedFormat => "alreadyCompressedFormat",
        SkipReason::ChangesOften => "changesOften",
        SkipReason::TooSmall => "tooSmall",
        SkipReason::ProtectedLocation => "protectedLocation",
        SkipReason::WimBacked => "wimBacked",
        SkipReason::SameFileAlreadyDone => "sameFileAlreadyDone",
        SkipReason::StateUnknown => "stateUnknown",
        SkipReason::CantCompressHere(r) => unsupported_name(r),
    }
}

pub fn fail_name(reason: &FailReason) -> &'static str {
    match reason {
        FailReason::AccessDenied => "accessDenied",
        FailReason::InUse => "inUse",
        FailReason::NotEnoughSpace => "notEnoughSpace",
        FailReason::Gone => "notFound",
        FailReason::Unsupported => "unsupported",
        FailReason::Windows { .. } => "windowsError",
    }
}

/// A line to show for a file whose outcome needs attention, or `None` for
/// the ordinary outcomes the summary covers.
pub fn attention(path: &Path, outcome: &Outcome) -> Option<String> {
    match outcome {
        Outcome::Failed { reason, .. } => Some(format!("Couldn't change {}: {}", path.display(), reason.plain())),
        _ => None,
    }
}

/// What `scrimp inspect` says about one file.
pub fn describe(path: &Path, state: &CompressionState, logical: u64, on_disk: u64) -> String {
    let size = format_size(logical);
    let p = path.display();
    match state {
        CompressionState::Uncompressed => format!("{p}: not compressed. {size}."),
        CompressionState::Compressed(algorithm) => {
            let saved = if logical > 0 {
                (logical.saturating_sub(on_disk) as f64 / logical as f64 * 100.0).round()
            } else {
                0.0
            };
            format!(
                "{p}: compressed with {}. {size}, taking {} on disk ({saved:.0}% saved).",
                method(*algorithm),
                format_size(on_disk)
            )
        }
        CompressionState::WimBacked => format!("{p}: a Windows system file. Scrimp leaves it alone. {size}."),
        CompressionState::Unsupported(reason) => {
            format!("{p}: left alone, because {}. {size}.", reason.plain())
        }
        CompressionState::Unknown => {
            format!("{p}: Scrimp can't tell how this file is stored, so leaves it alone. {size}.")
        }
    }
}

/// A method's name for people who asked for detail, as `inspect` does.
fn method(algorithm: Algorithm) -> &'static str {
    algorithm.technical_name()
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn inspect_describes_each_state_plainly() {
        let p = Path::new(r"D:\Archive\Example.dat");
        assert_eq!(
            describe(p, &CompressionState::Compressed(Algorithm::Lzx), 5 * GB, 2 * GB),
            r"D:\Archive\Example.dat: compressed with LZX. 5.0 GB, taking 2.0 GB on disk (60% saved)."
        );
        assert_eq!(
            describe(p, &CompressionState::Uncompressed, 1024, 1024),
            r"D:\Archive\Example.dat: not compressed. 1.0 KB."
        );
        assert!(describe(p, &CompressionState::WimBacked, 0, 0).contains("a Windows system file"));
        assert!(describe(p, &CompressionState::Unknown, 0, 0).contains("can't tell"));
        assert!(
            describe(p, &CompressionState::Unsupported(UnsupportedReason::Link), 0, 0)
                .contains("left alone, because it's a link")
        );
        assert!(describe(p, &CompressionState::Compressed(Algorithm::Lzx), 0, 0).contains("(0% saved)"));
    }

    #[test]
    fn every_reason_has_words_and_a_json_name() {
        let unsupported = [
            UnsupportedReason::FileSystem("exFAT".into()),
            UnsupportedReason::ClusterTooLarge,
            UnsupportedReason::NetworkLocation,
            UnsupportedReason::Encrypted,
            UnsupportedReason::CloudPlaceholder,
            UnsupportedReason::Link,
            UnsupportedReason::SpecialFile,
            UnsupportedReason::WofNotRunning,
        ];
        for r in &unsupported {
            assert!(!r.plain().is_empty());
            assert_eq!(skip_name(&SkipReason::CantCompressHere(r.clone())), unsupported_name(r));
        }
        for r in [
            SkipReason::AlreadyCompressedFormat,
            SkipReason::ChangesOften,
            SkipReason::TooSmall,
            SkipReason::ProtectedLocation,
            SkipReason::WimBacked,
            SkipReason::SameFileAlreadyDone,
            SkipReason::StateUnknown,
        ] {
            assert!(skip_name(&r).chars().next().unwrap().is_lowercase());
        }
        let p = Path::new(r"D:\Archive\Example.dat");
        for r in [
            FailReason::AccessDenied,
            FailReason::InUse,
            FailReason::NotEnoughSpace,
            FailReason::Gone,
            FailReason::Unsupported,
            FailReason::Windows {
                code: 87,
                message: "The parameter is incorrect.".into(),
            },
        ] {
            let line = attention(
                p,
                &Outcome::Failed {
                    reason: r.clone(),
                    left_as: CompressionState::Uncompressed,
                },
            )
            .unwrap();
            assert!(line.starts_with(r"Couldn't change D:\Archive\Example.dat: "), "{line}");
            assert!(!fail_name(&r).is_empty());
        }
        assert_eq!(attention(p, &Outcome::NotBeneficial), None);
    }

    #[test]
    fn states_have_json_names() {
        assert_eq!(state_name(&CompressionState::Uncompressed), "uncompressed");
        assert_eq!(state_name(&CompressionState::Compressed(Algorithm::Lznt1)), "ntfs");
        assert_eq!(state_name(&CompressionState::WimBacked), "wimBacked");
        assert_eq!(
            state_name(&CompressionState::Unsupported(UnsupportedReason::Encrypted)),
            "unsupported"
        );
        assert_eq!(state_name(&CompressionState::Unknown), "unknown");
    }
}
