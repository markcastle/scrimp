//! What went wrong, as a few plain kinds rather than raw Windows codes.
//!
//! The mapping from Windows error codes is a pure function, so it is tested
//! on every platform. Which Windows call failed sometimes matters: error 1,
//! "Incorrect function", from a WOF call on an NTFS drive means Windows'
//! compression driver isn't attached there, while from an NTFS compression
//! call it means the drive doesn't support it.

use scrimp_core::{CompressionState, FailReason};

/// Which family of call failed, where it changes what an error means.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Call {
    /// A WOF call: compressing with XPRESS or LZX, or removing it.
    Wof,
    /// Anything else: NTFS compression, opening files, reading sizes.
    Other,
}

/// Why a Windows call didn't do what was asked.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// Windows looked at the file and found compressing it wouldn't save
    /// space. A normal result, not a failure; the file is left as it was.
    #[error("compressing this file wouldn't save space")]
    NotBeneficial,
    /// Windows refused. Running as administrator may help.
    #[error("Windows didn't allow the change")]
    AccessDenied,
    /// Another program has the file open and doesn't allow changes.
    #[error("the file is in use by another app")]
    InUse,
    /// The file isn't there any more.
    #[error("the file can't be found")]
    NotFound,
    /// Not enough free space to finish.
    #[error("there isn't enough free space")]
    NotEnoughSpace,
    /// A WOF call failed with "Incorrect function". On an NTFS drive this
    /// means Windows' compression driver isn't attached to it, which
    /// Phase 0 found happens with a VHDX mounted after boot.
    #[error("Windows' compression driver isn't running on this drive")]
    WofUnavailable,
    /// The drive or the file doesn't support this kind of compression.
    #[error("this drive doesn't support this kind of compression")]
    NotSupported,
    /// The path is a folder, not a file.
    #[error("this is a folder, not a file")]
    NotAFile,
    /// Scrimp leaves files in this state alone and changed nothing.
    #[error("this file is left alone ({0:?})")]
    LeftAlone(CompressionState),
    /// Windows said it worked, but reading the file back showed it isn't in
    /// the state that was asked for. The file is in the state given.
    #[error("the change didn't take effect; the file is {0:?}")]
    NotApplied(CompressionState),
    /// Any other Windows error, kept whole for the report.
    #[error("Windows error {code}: {message}")]
    Windows { code: u32, message: String },
}

impl Error {
    /// The error a Windows error code means, for the given kind of call.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub(crate) fn from_code(code: u32, call: Call, message: impl FnOnce() -> String) -> Error {
        match code {
            1 if call == Call::Wof => Error::WofUnavailable,
            1 | 50 => Error::NotSupported,
            2 | 3 => Error::NotFound,
            5 => Error::AccessDenied,
            32 | 33 => Error::InUse,
            39 | 112 => Error::NotEnoughSpace,
            344 => Error::NotBeneficial,
            _ => Error::Windows {
                code,
                message: message(),
            },
        }
    }

    /// How a report should show this, or `None` when it isn't a failure.
    pub fn fail_reason(&self) -> Option<FailReason> {
        match self {
            Error::NotBeneficial | Error::LeftAlone(_) => None,
            Error::AccessDenied => Some(FailReason::AccessDenied),
            Error::InUse => Some(FailReason::InUse),
            Error::NotFound => Some(FailReason::Gone),
            Error::NotEnoughSpace => Some(FailReason::NotEnoughSpace),
            Error::WofUnavailable | Error::NotSupported | Error::NotAFile => Some(FailReason::Unsupported),
            Error::NotApplied(state) => Some(FailReason::Windows {
                code: 0,
                message: format!("the change didn't take effect; the file is {state:?}"),
            }),
            Error::Windows { code, message } => Some(FailReason::Windows {
                code: *code,
                message: message.clone(),
            }),
        }
    }
}

#[cfg(windows)]
impl Error {
    /// Converts a `windows` crate error, keeping its Win32 code where it has one.
    pub(crate) fn from_windows(e: &windows::core::Error, call: Call) -> Error {
        let hresult = e.code().0 as u32;
        let code = if hresult & 0xFFFF_0000 == 0x8007_0000 {
            hresult & 0xFFFF
        } else {
            hresult
        };
        Error::from_code(code, call, || e.message().trim().to_string())
    }

    /// Converts a standard library I/O error.
    pub(crate) fn from_io(e: &std::io::Error) -> Error {
        match e.raw_os_error() {
            Some(code) => Error::from_code(code as u32, Call::Other, || e.to_string()),
            None if e.kind() == std::io::ErrorKind::NotFound => Error::NotFound,
            None => Error::Windows {
                code: 0,
                message: e.to_string(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(code: u32, call: Call) -> Error {
        Error::from_code(code, call, || "something else".into())
    }

    #[test]
    fn incorrect_function_means_different_things_for_wof_and_for_ntfs() {
        assert_eq!(map(1, Call::Wof), Error::WofUnavailable);
        assert_eq!(map(1, Call::Other), Error::NotSupported);
    }

    #[test]
    fn the_codes_phase_0_saw_have_plain_meanings() {
        for (code, want) in [
            (2, Error::NotFound),
            (3, Error::NotFound),
            (5, Error::AccessDenied),
            (32, Error::InUse),
            (33, Error::InUse),
            (39, Error::NotEnoughSpace),
            (112, Error::NotEnoughSpace),
            (50, Error::NotSupported),
            (344, Error::NotBeneficial),
        ] {
            assert_eq!(map(code, Call::Wof), want, "code {code}");
            assert_eq!(map(code, Call::Other), want, "code {code}");
        }
        assert_eq!(
            map(87, Call::Wof),
            Error::Windows {
                code: 87,
                message: "something else".into()
            }
        );
    }

    #[test]
    fn not_beneficial_and_left_alone_are_not_failures() {
        assert_eq!(Error::NotBeneficial.fail_reason(), None);
        assert_eq!(Error::LeftAlone(CompressionState::WimBacked).fail_reason(), None);
        assert_eq!(Error::InUse.fail_reason(), Some(FailReason::InUse));
        assert_eq!(Error::AccessDenied.fail_reason(), Some(FailReason::AccessDenied));
        assert_eq!(Error::NotFound.fail_reason(), Some(FailReason::Gone));
        assert_eq!(Error::NotEnoughSpace.fail_reason(), Some(FailReason::NotEnoughSpace));
        for unsupported in [Error::WofUnavailable, Error::NotSupported, Error::NotAFile] {
            assert_eq!(unsupported.fail_reason(), Some(FailReason::Unsupported));
        }
        let other = Error::Windows {
            code: 87,
            message: "The parameter is incorrect.".into(),
        };
        assert_eq!(
            other.fail_reason(),
            Some(FailReason::Windows {
                code: 87,
                message: "The parameter is incorrect.".into()
            })
        );
        assert!(matches!(
            Error::NotApplied(CompressionState::Uncompressed).fail_reason(),
            Some(FailReason::Windows { code: 0, .. })
        ));
    }

    #[test]
    fn messages_are_plain_words() {
        assert_eq!(Error::InUse.to_string(), "the file is in use by another app");
        assert_eq!(
            Error::WofUnavailable.to_string(),
            "Windows' compression driver isn't running on this drive"
        );
    }
}
