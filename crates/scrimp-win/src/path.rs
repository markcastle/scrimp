//! Turning a Rust path into the UTF-16 Windows expects.
//!
//! Paths are passed through exactly, never via a lossy conversion. Long
//! paths get the `\\?\` prefix, which lifts the old 260-character limit for
//! the raw Windows calls in this crate (the standard library already does
//! this for its own file functions).

use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::Path;

/// Paths at least this long get the `\\?\` prefix. Some calls fail a little
/// below 260 characters, so this leaves room.
const LONG: usize = 240;

/// A NUL-terminated UTF-16 copy of `path`, ready for a Windows call.
pub(crate) fn wide(path: &Path) -> Vec<u16> {
    let mut units: Vec<u16> = path.as_os_str().encode_wide().collect();
    if units.len() >= LONG && path.is_absolute() && !units.starts_with(&verbatim_prefix()) {
        units = long_form(&units);
    }
    units.push(0);
    units
}

fn verbatim_prefix() -> [u16; 4] {
    [b'\\' as u16, b'\\' as u16, b'?' as u16, b'\\' as u16]
}

/// `C:\a\b` becomes `\\?\C:\a\b`, and `\\server\share\a` becomes
/// `\\?\UNC\server\share\a`. Forward slashes become backslashes, because
/// Windows takes the verbatim form literally.
fn long_form(units: &[u16]) -> Vec<u16> {
    let slash = b'/' as u16;
    let back = b'\\' as u16;
    let units: Vec<u16> = units.iter().map(|&u| if u == slash { back } else { u }).collect();
    let mut out: Vec<u16> = verbatim_prefix().to_vec();
    if units.starts_with(&[back, back]) {
        out.extend("UNC\\".encode_utf16());
        out.extend_from_slice(&units[2..]);
    } else {
        out.extend_from_slice(&units);
    }
    out
}

/// A Rust path from a NUL-terminated (or full) UTF-16 buffer.
pub(crate) fn from_wide(units: &[u16]) -> std::path::PathBuf {
    let len = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    OsString::from_wide(&units[..len]).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(units: &[u16]) -> String {
        String::from_utf16_lossy(&units[..units.len() - 1])
    }

    #[test]
    fn short_paths_are_passed_through_exactly() {
        let p = Path::new(r"C:\Users\You\Documents\Report-2019.docx");
        assert_eq!(text(&wide(p)), r"C:\Users\You\Documents\Report-2019.docx");
        assert_eq!(wide(p).last(), Some(&0));
    }

    #[test]
    fn long_paths_get_the_verbatim_prefix() {
        let deep = format!(r"C:\Users\You\{}\Example.dat", "folder\\".repeat(40));
        assert_eq!(text(&wide(Path::new(&deep))), format!(r"\\?\{deep}"));

        let share = format!(r"\\NAS\Archive\{}\Example.dat", "folder\\".repeat(40));
        assert_eq!(
            text(&wide(Path::new(&share))),
            format!(r"\\?\UNC\NAS\Archive\{}\Example.dat", "folder\\".repeat(40))
        );

        let slashes = format!("C:/Users/You/{}Example.dat", "folder/".repeat(40));
        assert!(!text(&wide(Path::new(&slashes))).contains('/'));
    }

    #[test]
    fn already_verbatim_and_relative_paths_are_left_alone() {
        let verbatim = format!(r"\\?\C:\{}Example.dat", "folder\\".repeat(40));
        assert_eq!(text(&wide(Path::new(&verbatim))), verbatim);
        let relative = format!(r"{}Example.dat", "folder\\".repeat(40));
        assert_eq!(text(&wide(Path::new(&relative))), relative);
    }

    #[test]
    fn wide_buffers_read_back_as_paths() {
        let mut buf: Vec<u16> = r"C:\".encode_utf16().collect();
        buf.extend([0, 0, 0]);
        assert_eq!(from_wide(&buf), Path::new(r"C:\"));
        assert_eq!(from_wide(&r"D:\".encode_utf16().collect::<Vec<_>>()), Path::new(r"D:\"));
    }
}
