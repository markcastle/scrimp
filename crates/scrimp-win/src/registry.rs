//! Reading and writing the current user's registry, as plain strings.
//!
//! Values are read exactly as stored: an expandable string such as
//! `%USERPROFILE%\bin` comes back unexpanded, with its type, so it can be
//! written back unchanged. That matters for PATH (docs/developing.md).

use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_ALL_ACCESS, REG_EXPAND_SZ, REG_OPTION_NON_VOLATILE, REG_SZ, REG_VALUE_TYPE,
    RRF_NOEXPAND, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteKeyValueW,
    RegDeleteTreeW, RegGetValueW, RegSetValueExW,
};
use windows::core::PCWSTR;

use crate::error::{Call, Error};

/// The kind of string a value holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `REG_SZ`: plain text.
    Text,
    /// `REG_EXPAND_SZ`: text with `%VARIABLES%` Windows expands when reading it.
    Expandable,
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

fn name_ptr(name: &Option<Vec<u16>>) -> PCWSTR {
    name.as_ref().map_or(PCWSTR::null(), |n| PCWSTR(n.as_ptr()))
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

/// Sets a string value under `HKEY_CURRENT_USER`, creating the key if
/// needed. `name` of `None` is the key's default value.
pub fn set(key: &str, name: Option<&str>, value: &str, kind: Kind) -> Result<(), Error> {
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
    // The UTF-16 value, NUL included, as the bytes the registry stores.
    let bytes: Vec<u8> = wide(value).iter().flat_map(|u| u.to_le_bytes()).collect();
    let kind = match kind {
        Kind::Text => REG_SZ,
        Kind::Expandable => REG_EXPAND_SZ,
    };
    // SAFETY: the name and the bytes outlive the call, and `handle` is open.
    let result = check(unsafe { RegSetValueExW(handle, name_ptr(&name_w), None, kind, Some(&bytes)) });
    // SAFETY: closes the key opened above, once.
    unsafe {
        let _ = RegCloseKey(handle);
    }
    result
}

/// Reads a string value exactly as stored, with its kind, or `None` if it
/// isn't there.
pub fn get_raw(key: &str, name: Option<&str>) -> Result<Option<(String, Kind)>, Error> {
    let key_w = wide(key);
    let name_w = name.map(wide);
    let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND;
    let mut bytes = 0u32;
    // SAFETY: asks only for the size; both strings are NUL-terminated.
    let first = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(key_w.as_ptr()),
            name_ptr(&name_w),
            flags,
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
    let mut kind = REG_VALUE_TYPE::default();
    // SAFETY: `buf` holds `bytes` bytes, as the first call said is needed;
    // `kind` is a live local.
    check(unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(key_w.as_ptr()),
            name_ptr(&name_w),
            flags,
            Some(&mut kind),
            Some(buf.as_mut_ptr().cast()),
            Some(&mut bytes),
        )
    })?;
    let len = buf.iter().position(|&u| u == 0).unwrap_or(buf.len());
    let kind = if kind == REG_EXPAND_SZ {
        Kind::Expandable
    } else {
        Kind::Text
    };
    Ok(Some((String::from_utf16_lossy(&buf[..len]), kind)))
}

/// Reads a string value, or `None` if it isn't there.
pub fn get(key: &str, name: Option<&str>) -> Result<Option<String>, Error> {
    Ok(get_raw(key, name)?.map(|(value, _)| value))
}

/// Deletes a key and everything under it. A key that isn't there is fine.
pub fn delete_tree(key: &str) -> Result<(), Error> {
    let key_w = wide(key);
    // SAFETY: `key_w` is NUL-terminated and outlives the call.
    match unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(key_w.as_ptr())) } {
        ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
        other => check(other),
    }
}

/// Deletes one value. A value that isn't there is fine.
pub fn delete_value(key: &str, name: &str) -> Result<(), Error> {
    let (key_w, name_w) = (wide(key), wide(name));
    // SAFETY: both strings are NUL-terminated and outlive the call.
    match unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, PCWSTR(key_w.as_ptr()), PCWSTR(name_w.as_ptr())) } {
        ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
        other => check(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_kept_exactly_as_stored() {
        let key = format!(r"Software\ScrimpTest-registry-{}", std::process::id());
        set(&key, Some("Plain"), r"C:\Users\You", Kind::Text).unwrap();
        set(&key, Some("Expand"), r"%USERPROFILE%\bin", Kind::Expandable).unwrap();
        set(&key, None, "Default", Kind::Text).unwrap();
        assert_eq!(
            get_raw(&key, Some("Plain")).unwrap(),
            Some((r"C:\Users\You".into(), Kind::Text))
        );
        assert_eq!(
            get_raw(&key, Some("Expand")).unwrap(),
            Some((r"%USERPROFILE%\bin".into(), Kind::Expandable)),
            "never expanded"
        );
        assert_eq!(get(&key, None).unwrap().as_deref(), Some("Default"));
        delete_value(&key, "Plain").unwrap();
        delete_value(&key, "Plain").unwrap();
        assert_eq!(get(&key, Some("Plain")).unwrap(), None);
        delete_tree(&key).unwrap();
        assert_eq!(get(&key, None).unwrap(), None);
    }
}
