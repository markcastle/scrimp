//! The compression methods Windows offers, and what a person can ask for.
//!
//! Windows has two families. WOF compression (XPRESS4K, XPRESS8K, XPRESS16K
//! and LZX) gives the best savings but is undone when a file is written to.
//! NTFS compression (LZNT1) saves less but survives writes. People never see
//! these names in the menu; they see what each one is good for.

use std::fmt;
use std::str::FromStr;

/// One of the five compression methods built into Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Algorithm {
    /// WOF XPRESS with 4 KB chunks: the quickest to open.
    Xpress4k,
    /// WOF XPRESS with 8 KB chunks: a balance of speed and size.
    Xpress8k,
    /// WOF XPRESS with 16 KB chunks: smaller, a little slower.
    Xpress16k,
    /// WOF LZX: the smallest, and the slowest to compress.
    Lzx,
    /// Classic NTFS compression: saves less, but stays in place when a file changes.
    Lznt1,
}

impl Algorithm {
    /// Every method, in the order they are listed to people.
    pub const ALL: [Algorithm; 5] = [
        Algorithm::Xpress4k,
        Algorithm::Xpress8k,
        Algorithm::Xpress16k,
        Algorithm::Lzx,
        Algorithm::Lznt1,
    ];

    /// The name used on the command line and in `settings.json`.
    pub fn cli_name(self) -> &'static str {
        match self {
            Algorithm::Xpress4k => "xpress4k",
            Algorithm::Xpress8k => "xpress8k",
            Algorithm::Xpress16k => "xpress16k",
            Algorithm::Lzx => "lzx",
            Algorithm::Lznt1 => "ntfs",
        }
    }

    /// The technical name, shown only behind "Details" and in reports.
    pub fn technical_name(self) -> &'static str {
        match self {
            Algorithm::Xpress4k => "XPRESS4K",
            Algorithm::Xpress8k => "XPRESS8K",
            Algorithm::Xpress16k => "XPRESS16K",
            Algorithm::Lzx => "LZX",
            Algorithm::Lznt1 => "NTFS (LZNT1)",
        }
    }

    /// The label in the Shift + right-click menu, for the methods that have one.
    /// XPRESS8K and XPRESS16K have none: automatic mode uses them, and they stay
    /// available on the command line and in settings.
    pub fn menu_label(self) -> Option<&'static str> {
        match self {
            Algorithm::Xpress4k => Some("Faster to Open"),
            Algorithm::Lzx => Some("Smallest"),
            Algorithm::Lznt1 => Some("For Files That Change"),
            Algorithm::Xpress8k | Algorithm::Xpress16k => None,
        }
    }

    /// Whether this is WOF compression, which is undone when the file is written to.
    pub fn is_wof(self) -> bool {
        !matches!(self, Algorithm::Lznt1)
    }

    /// Whether a file stays compressed after an app writes to it.
    pub fn survives_writes(self) -> bool {
        !self.is_wof()
    }
}

impl fmt::Display for Algorithm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.technical_name())
    }
}

/// What a person asked Scrimp to do to a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Target {
    /// Let Scrimp choose a method for each file, or leave it alone.
    Automatic,
    /// Use this method, whatever the file.
    Algorithm(Algorithm),
    /// Remove any compression.
    Uncompressed,
}

/// A name that isn't a compression method Scrimp knows.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("\"{given}\" isn't a compression method. Use one of: auto, xpress4k, xpress8k, xpress16k, lzx, ntfs.")]
pub struct ParseError {
    given: String,
}

impl FromStr for Algorithm {
    type Err = ParseError;

    /// Reads a command-line name, ignoring case. `lznt1` is accepted as well as `ntfs`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let wanted = s.trim().to_ascii_lowercase();
        if wanted == "lznt1" {
            return Ok(Algorithm::Lznt1);
        }
        Algorithm::ALL
            .into_iter()
            .find(|a| a.cli_name() == wanted)
            .ok_or_else(|| ParseError { given: s.to_string() })
    }
}

impl FromStr for Target {
    type Err = ParseError;

    /// Reads `auto` (or `automatic`) or a method name, ignoring case.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" | "automatic" => Ok(Target::Automatic),
            _ => s.parse().map(Target::Algorithm),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_method_reads_back_from_its_own_name() {
        for a in Algorithm::ALL {
            assert_eq!(a.cli_name().parse::<Algorithm>(), Ok(a));
            assert_eq!(a.cli_name().to_uppercase().parse::<Algorithm>(), Ok(a));
        }
        assert_eq!("LZNT1".parse::<Algorithm>(), Ok(Algorithm::Lznt1));
        assert_eq!(" lzx ".parse::<Algorithm>(), Ok(Algorithm::Lzx));
    }

    #[test]
    fn an_unknown_name_says_what_would_work() {
        let err = "zip".parse::<Algorithm>().unwrap_err().to_string();
        assert!(err.starts_with("\"zip\" isn't a compression method."), "{err}");
        assert!(err.contains("xpress16k"), "{err}");
        assert!("".parse::<Target>().is_err());
    }

    #[test]
    fn auto_is_a_target_but_not_a_method() {
        assert_eq!("auto".parse::<Target>(), Ok(Target::Automatic));
        assert_eq!("Automatic".parse::<Target>(), Ok(Target::Automatic));
        assert_eq!("lzx".parse::<Target>(), Ok(Target::Algorithm(Algorithm::Lzx)));
        assert!("auto".parse::<Algorithm>().is_err());
    }

    #[test]
    fn only_ntfs_compression_survives_writes() {
        let survivors: Vec<_> = Algorithm::ALL.into_iter().filter(|a| a.survives_writes()).collect();
        assert_eq!(survivors, [Algorithm::Lznt1]);
        assert!(Algorithm::ALL.into_iter().all(|a| a.is_wof() != a.survives_writes()));
    }

    #[test]
    fn the_menu_shows_plain_words_for_three_methods() {
        let labelled: Vec<_> = Algorithm::ALL.into_iter().filter_map(Algorithm::menu_label).collect();
        assert_eq!(labelled, ["Faster to Open", "Smallest", "For Files That Change"]);
        for label in labelled {
            assert!(!label.contains("XPRESS") && !label.contains("LZ"), "{label}");
        }
    }

    #[test]
    fn display_is_the_technical_name() {
        assert_eq!(Algorithm::Xpress16k.to_string(), "XPRESS16K");
        assert_eq!(Algorithm::Lznt1.to_string(), "NTFS (LZNT1)");
    }
}
