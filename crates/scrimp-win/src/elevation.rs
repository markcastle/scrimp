//! Running as administrator: whether this process is, and starting another
//! program that is.
//!
//! Scrimp never asks for administrator rights by itself. When Windows
//! refuses to change some files, the summary offers **Try Again as
//! Administrator**, and only then does Windows show its own prompt.

use std::path::Path;

use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, HANDLE};
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::UI::Shell::{SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::PCWSTR;

use crate::error::{Call, Error};

/// Whether this process is running as administrator.
pub fn is_elevated() -> bool {
    let mut token = HANDLE::default();
    // SAFETY: opens this process's own token for reading into a live local.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.is_err() {
        return false;
    }
    let mut elevation = TOKEN_ELEVATION::default();
    let mut size = 0u32;
    // SAFETY: `elevation` is a live TOKEN_ELEVATION and its exact size is
    // passed with it; `token` was opened above.
    let read = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            Some((&mut elevation as *mut TOKEN_ELEVATION).cast()),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut size,
        )
    };
    // SAFETY: the token was opened above and is closed once, here.
    unsafe {
        let _ = CloseHandle(token);
    }
    read.is_ok() && elevation.TokenIsElevated != 0
}

/// Starts `program` with `args` as administrator. Windows asks the person
/// first; `Ok(false)` means they said no.
pub fn run_as_administrator(program: &Path, args: &[&str]) -> Result<bool, Error> {
    let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() };
    let verb = wide("runas");
    let file = wide(&program.to_string_lossy());
    let parameters = wide(&command_line(args));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOASYNC,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(parameters.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    // SAFETY: `info` and every string it points to live until this call returns.
    match unsafe { ShellExecuteExW(&mut info) } {
        Ok(()) => Ok(true),
        Err(e) if e.code() == ERROR_CANCELLED.to_hresult() => Ok(false),
        Err(e) => Err(Error::from_windows(&e, Call::Other)),
    }
}

/// Joins arguments into one command line that a Rust or C program splits
/// back into exactly the same arguments.
pub(crate) fn command_line(args: &[&str]) -> String {
    args.iter().map(|a| quote(a)).collect::<Vec<_>>().join(" ")
}

fn quote(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.to_string();
    }
    let mut out = String::from('"');
    let mut slashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => slashes += 1,
            '"' => {
                // Backslashes before a quote are doubled, and the quote escaped.
                out.extend(std::iter::repeat_n('\\', slashes * 2 + 1));
                out.push('"');
                slashes = 0;
                continue;
            }
            _ => {}
        }
        if c != '\\' {
            out.extend(std::iter::repeat_n('\\', slashes));
            slashes = 0;
            out.push(c);
        }
    }
    // Backslashes before the closing quote are doubled too.
    out.extend(std::iter::repeat_n('\\', slashes * 2));
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_survive_the_command_line() {
        assert_eq!(
            command_line(&["--jobs-folder", r"C:\Scrimp\admin-jobs"]),
            r"--jobs-folder C:\Scrimp\admin-jobs"
        );
        assert_eq!(
            command_line(&[r"C:\Users\Jo Bloggs\AppData\Local\Scrimp\admin-jobs"]),
            r#""C:\Users\Jo Bloggs\AppData\Local\Scrimp\admin-jobs""#
        );
        assert_eq!(command_line(&[r"C:\with space\"]), r#""C:\with space\\""#);
        assert_eq!(command_line(&[r#"say "hi""#]), r#""say \"hi\"""#);
        assert_eq!(command_line(&[r#"a\"b"#]), r#""a\\\"b""#);
        assert_eq!(command_line(&[""]), r#""""#);
    }

    #[test]
    fn elevation_can_be_read() {
        // GitHub's runners are administrators and people usually aren't;
        // either way the answer must come back without failing.
        let _ = is_elevated();
    }
}
