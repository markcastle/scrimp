//! Adding Scrimp's folder to the user's PATH, and taking it away again,
//! without disturbing anything else (docs/developing.md, "How Scrimp is
//! added to PATH").
//!
//! - Only the user's PATH, never the system's.
//! - Exactly one entry, never twice: entries are compared ignoring case and
//!   a trailing backslash, with `%VARIABLES%` expanded for the comparison only.
//! - Every other entry is left exactly as it was, in the same order,
//!   including unexpanded ones such as `%USERPROFILE%\bin`.
//! - The value keeps its registry type, usually `REG_EXPAND_SZ`.
//! - Running programs are told, so new terminals see the change.
//! - Removing takes away only Scrimp's entry; an emptied PATH is deleted
//!   rather than left blank.
//!
//! The rules are pure functions over the PATH text, tested on their own.

use std::path::Path;

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE,
};

use crate::error::Error;
use crate::registry::{self, Kind};

const ENVIRONMENT: &str = "Environment";
const PATH: &str = "Path";

/// Expands `%NAME%` from the environment, leaving unknown names as they are.
fn expand(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) => {
                let name = &after[..end];
                match std::env::var(name) {
                    Ok(value) if !name.is_empty() => out.push_str(&value),
                    _ => out.push_str(&rest[start..start + end + 2]),
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// An entry in the form entries are compared in.
fn key(entry: &str, expand: &dyn Fn(&str) -> String) -> String {
    expand(entry.trim()).trim_end_matches('\\').to_lowercase()
}

/// `path` with `folder` added at the end, or `None` if it's already there.
fn with_entry(path: &str, folder: &str, expand: &dyn Fn(&str) -> String) -> Option<String> {
    let wanted = key(folder, expand);
    if path
        .split(';')
        .any(|e| !e.trim().is_empty() && key(e, expand) == wanted)
    {
        return None;
    }
    let separator = if path.is_empty() || path.ends_with(';') {
        ""
    } else {
        ";"
    };
    Some(format!("{path}{separator}{folder}"))
}

/// `path` without `folder`, or `None` if it isn't there. Other entries,
/// and any empty ones between them, are kept as they were.
fn without_entry(path: &str, folder: &str, expand: &dyn Fn(&str) -> String) -> Option<String> {
    let wanted = key(folder, expand);
    let entries: Vec<&str> = path.split(';').collect();
    let kept: Vec<&str> = entries
        .iter()
        .copied()
        .filter(|e| e.trim().is_empty() || key(e, expand) != wanted)
        .collect();
    (kept.len() != entries.len()).then(|| kept.join(";"))
}

/// Tells running programs the environment changed. Some never answer, so
/// this waits at most five seconds and doesn't mind.
fn announce() {
    let text: Vec<u16> = ENVIRONMENT.encode_utf16().chain(Some(0)).collect();
    // SAFETY: `text` is NUL-terminated and outlives the call, which returns
    // within its timeout; no result is needed.
    unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            WPARAM(0),
            LPARAM(text.as_ptr() as isize),
            SMTO_ABORTIFHUNG,
            5000,
            None,
        );
    }
}

/// Adds `folder` to the user's PATH. Returns whether anything changed.
pub fn add(folder: &Path) -> Result<bool, Error> {
    let folder = folder.to_string_lossy();
    let (current, kind) = registry::get_raw(ENVIRONMENT, Some(PATH))?.unwrap_or((String::new(), Kind::Expandable));
    let Some(updated) = with_entry(&current, &folder, &expand) else {
        return Ok(false);
    };
    registry::set(ENVIRONMENT, Some(PATH), &updated, kind)?;
    announce();
    Ok(true)
}

/// Takes `folder` out of the user's PATH. Returns whether anything changed.
pub fn remove(folder: &Path) -> Result<bool, Error> {
    let folder = folder.to_string_lossy();
    let Some((current, kind)) = registry::get_raw(ENVIRONMENT, Some(PATH))? else {
        return Ok(false);
    };
    let Some(updated) = without_entry(&current, &folder, &expand) else {
        return Ok(false);
    };
    if updated.split(';').all(|e| e.trim().is_empty()) {
        registry::delete_value(ENVIRONMENT, PATH)?;
    } else {
        registry::set(ENVIRONMENT, Some(PATH), &updated, kind)?;
    }
    announce();
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIR: &str = r"C:\Users\You\AppData\Local\Programs\Scrimp";

    fn fake(text: &str) -> String {
        text.replace("%LOCALAPPDATA%", r"C:\Users\You\AppData\Local")
            .replace("%USERPROFILE%", r"C:\Users\You")
    }

    #[test]
    fn adding_appends_one_entry_and_leaves_the_rest_alone() {
        let path = r"%USERPROFILE%\bin;C:\Tools";
        assert_eq!(
            with_entry(path, DIR, &fake).unwrap(),
            format!(r"%USERPROFILE%\bin;C:\Tools;{DIR}")
        );
        assert_eq!(with_entry("", DIR, &fake).unwrap(), DIR, "an empty PATH");
        assert_eq!(
            with_entry(r"C:\Tools;", DIR, &fake).unwrap(),
            format!(r"C:\Tools;{DIR}"),
            "a trailing ; is kept as the separator"
        );
    }

    #[test]
    fn adding_twice_never_adds_twice() {
        for present in [
            DIR.to_string(),
            DIR.to_lowercase(),
            format!(r"{DIR}\"),
            format!(r" {DIR} "),
            r"%LOCALAPPDATA%\Programs\Scrimp".to_string(),
        ] {
            let path = format!(r"C:\Tools;{present};D:\More");
            assert_eq!(with_entry(&path, DIR, &fake), None, "{present}");
        }
    }

    #[test]
    fn removing_takes_away_only_scrimp() {
        let path = format!(r"%USERPROFILE%\bin;{DIR};C:\Tools");
        assert_eq!(without_entry(&path, DIR, &fake).unwrap(), r"%USERPROFILE%\bin;C:\Tools");
        let added_by_hand = r"C:\Tools;%LOCALAPPDATA%\Programs\Scrimp\";
        assert_eq!(without_entry(added_by_hand, DIR, &fake).unwrap(), r"C:\Tools");
        assert_eq!(without_entry(r"C:\Tools", DIR, &fake), None, "nothing to remove");
        assert_eq!(without_entry(DIR, DIR, &fake).unwrap(), "", "only Scrimp was there");
        assert_eq!(
            without_entry(&format!(r"C:\Tools;;{DIR}"), DIR, &fake).unwrap(),
            r"C:\Tools;",
            "an empty entry someone else left is kept"
        );
    }

    #[test]
    fn a_long_path_is_handled_whole() {
        let long: Vec<String> = (0..200).map(|i| format!(r"C:\Tools\Folder number {i}")).collect();
        let path = long.join(";");
        assert!(path.len() > 4000);
        let added = with_entry(&path, DIR, &fake).unwrap();
        assert!(added.starts_with(&path) && added.ends_with(DIR));
        assert_eq!(
            without_entry(&added, DIR, &fake).unwrap(),
            path,
            "round trip leaves PATH as it was"
        );
    }

    #[test]
    fn variables_expand_from_the_environment() {
        let home = std::env::var("USERPROFILE").unwrap_or_default();
        assert_eq!(expand(r"%USERPROFILE%\bin"), format!(r"{home}\bin"));
        assert_eq!(
            expand(r"%NOT_A_REAL_VARIABLE_SCRIMP%\bin"),
            r"%NOT_A_REAL_VARIABLE_SCRIMP%\bin"
        );
        assert_eq!(expand("100% sure"), "100% sure");
        assert_eq!(expand("%%"), "%%");
    }
}
