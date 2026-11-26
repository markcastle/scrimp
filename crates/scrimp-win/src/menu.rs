//! Adding Scrimp to Explorer's right-click menu, and taking it away again.
//!
//! The menu entries go in the user's own part of the registry, so no
//! administrator is needed, and they show in Windows 10's menu and in
//! Windows 11's "Show more options" menu without code signing. Windows 11's
//! modern menu comes later, with a signed package (docs/plan.md).
//!
//! For each of Scrimp Compress and Scrimp Decompress this writes:
//!
//! ```text
//! HKCU\Software\Classes\CLSID\{id}                    "Scrimp Compress"
//! HKCU\Software\Classes\CLSID\{id}\InprocServer32     the DLL's path, ThreadingModel=Apartment
//! HKCU\Software\Classes\<type>\shell\ScrimpCompress   ExplorerCommandHandler={id}, NeverDefault
//! ```
//!
//! for files (`*`), folders (`Directory`) and drives (`Drive`). The DLL
//! decides what each entry says and when it shows. `NeverDefault` stops
//! either ever becoming what a double-click does.

use std::path::Path;

use scrimp_core::shell_ids::{COMPRESS_COMMAND, DECOMPRESS_COMMAND, braced};
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_ALL_ACCESS, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ, RegCloseKey,
    RegCreateKeyExW, RegDeleteTreeW, RegGetValueW, RegSetValueExW,
};
use windows::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
use windows::core::PCWSTR;

use crate::error::{Call, Error};

/// Where per-user class registrations live.
pub const CLASSES: &str = r"Software\Classes";

const ITEM_TYPES: [&str; 3] = ["*", "Directory", "Drive"];

/// The two menu entries: verb name, title, class ID.
const ENTRIES: [(&str, &str, u128); 2] = [
    ("ScrimpCompress", "Scrimp Compress", COMPRESS_COMMAND),
    ("ScrimpDecompress", "Scrimp Decompress", DECOMPRESS_COMMAND),
];

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

fn check(code: WIN32_ERROR) -> Result<(), Error> {
    if code == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(Error::from_code(code.0, Call::Other, || {
            format!("registry error {}", code.0)
        }))
    }
}

/// Sets a string value, creating the key if needed. `name` of `None` is the
/// key's default value.
fn set(key: &str, name: Option<&str>, value: &str) -> Result<(), Error> {
    let key_w = wide(key);
    let mut handle = HKEY::default();
    // SAFETY: `key_w` is NUL-terminated and outlives the call; `handle` is a
    // live local the call fills in.
    check(unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(key_w.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_ALL_ACCESS,
            None,
            &mut handle,
            None,
        )
    })?;
    let name_w = name.map(wide);
    let value_w = wide(value);
    // The UTF-16 value, NUL included, as the bytes the registry stores.
    let bytes: Vec<u8> = value_w.iter().flat_map(|u| u.to_le_bytes()).collect();
    // SAFETY: the name and the bytes outlive the call, and `handle` is open.
    let result = check(unsafe {
        RegSetValueExW(
            handle,
            name_w.as_ref().map_or(PCWSTR::null(), |n| PCWSTR(n.as_ptr())),
            None,
            REG_SZ,
            Some(&bytes),
        )
    });
    // SAFETY: closes the key opened above, once.
    unsafe {
        let _ = RegCloseKey(handle);
    }
    result
}

/// Reads a string value, or `None` if it isn't there.
pub fn get(key: &str, name: Option<&str>) -> Result<Option<String>, Error> {
    let key_w = wide(key);
    let name_w = name.map(wide);
    let name_p = name_w.as_ref().map_or(PCWSTR::null(), |n| PCWSTR(n.as_ptr()));
    let mut bytes = 0u32;
    // SAFETY: asks only for the size; both strings are NUL-terminated.
    let first = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(key_w.as_ptr()),
            name_p,
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut bytes),
        )
    };
    if first == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    check(first)?;
    let mut buf = vec![0u16; (bytes as usize).div_ceil(2).max(1)];
    // SAFETY: `buf` holds `bytes` bytes, as the first call said is needed.
    check(unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(key_w.as_ptr()),
            name_p,
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut bytes),
        )
    })?;
    let len = buf.iter().position(|&u| u == 0).unwrap_or(buf.len());
    Ok(Some(String::from_utf16_lossy(&buf[..len])))
}

/// Deletes a key and everything under it. A key that isn't there is fine.
fn delete(key: &str) -> Result<(), Error> {
    let key_w = wide(key);
    // SAFETY: `key_w` is NUL-terminated and outlives the call.
    match unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(key_w.as_ptr())) } {
        ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
        other => check(other),
    }
}

/// The registry keys Scrimp's menu uses, under `classes`, for tests and
/// for removing them.
fn keys(classes: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (verb, _, id) in ENTRIES {
        out.push(format!(r"{classes}\CLSID\{}", braced(id)));
        for item in ITEM_TYPES {
            out.push(format!(r"{classes}\{item}\shell\{verb}"));
        }
    }
    out
}

/// Adds the menu entries under `classes`, pointing at the menu DLL at `dll`.
pub fn register_under(classes: &str, dll: &Path) -> Result<(), Error> {
    let dll = dll.to_string_lossy();
    for (verb, title, id) in ENTRIES {
        let clsid = format!(r"{classes}\CLSID\{}", braced(id));
        set(&clsid, None, title)?;
        set(&format!(r"{clsid}\InprocServer32"), None, &dll)?;
        set(&format!(r"{clsid}\InprocServer32"), Some("ThreadingModel"), "Apartment")?;
        for item in ITEM_TYPES {
            let key = format!(r"{classes}\{item}\shell\{verb}");
            set(&key, Some("ExplorerCommandHandler"), &braced(id))?;
            set(&key, Some("NeverDefault"), "")?;
        }
    }
    Ok(())
}

/// Removes every menu entry under `classes`, leaving nothing behind.
pub fn unregister_under(classes: &str) -> Result<(), Error> {
    keys(classes).iter().try_for_each(|k| delete(k))
}

/// Tells Explorer the menu has changed, so open windows pick it up.
fn notify_explorer() {
    // SAFETY: a notification with no items, which Windows copies at once.
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
}

/// Adds Scrimp to Explorer's menu for this user.
pub fn register(dll: &Path) -> Result<(), Error> {
    register_under(CLASSES, dll)?;
    notify_explorer();
    Ok(())
}

/// Takes Scrimp out of Explorer's menu for this user.
pub fn unregister() -> Result<(), Error> {
    unregister_under(CLASSES)?;
    notify_explorer();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A part of the registry no real setting uses, so tests never touch
    /// the actual menu.
    fn scratch() -> String {
        format!(r"Software\ScrimpTest-{}\Classes", std::process::id())
    }

    #[test]
    fn registering_writes_everything_and_unregistering_removes_it_all() {
        let classes = scratch();
        let dll = Path::new(r"C:\Users\You\AppData\Local\Programs\Scrimp\shell\0.2.0\scrimp_shell.dll");
        register_under(&classes, dll).unwrap();

        let clsid = format!(r"{classes}\CLSID\{}", braced(COMPRESS_COMMAND));
        assert_eq!(get(&clsid, None).unwrap().as_deref(), Some("Scrimp Compress"));
        assert_eq!(
            get(&format!(r"{clsid}\InprocServer32"), None).unwrap().as_deref(),
            Some(dll.to_str().unwrap())
        );
        assert_eq!(
            get(&format!(r"{clsid}\InprocServer32"), Some("ThreadingModel"))
                .unwrap()
                .as_deref(),
            Some("Apartment")
        );
        for item in ITEM_TYPES {
            let verb = format!(r"{classes}\{item}\shell\ScrimpDecompress");
            assert_eq!(
                get(&verb, Some("ExplorerCommandHandler")).unwrap(),
                Some(braced(DECOMPRESS_COMMAND)),
                "{item}"
            );
            assert_eq!(get(&verb, Some("NeverDefault")).unwrap().as_deref(), Some(""));
        }
        // Registering again is harmless.
        register_under(&classes, dll).unwrap();

        unregister_under(&classes).unwrap();
        for key in keys(&classes) {
            assert_eq!(get(&key, None).unwrap(), None, "{key} was left behind");
        }
        // Unregistering twice is harmless too.
        unregister_under(&classes).unwrap();
        delete(&format!(r"Software\ScrimpTest-{}", std::process::id())).unwrap();
    }

    #[test]
    fn a_missing_value_reads_as_none() {
        assert_eq!(get(r"Software\ScrimpTest-none\Nothing", None).unwrap(), None);
    }
}
