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

use scrimp_core::shell_ids::{COMPRESS_COMMAND, DECOMPRESS_COMMAND, SAVINGS_COMMAND, braced};
use windows::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};

use crate::error::Error;
use crate::registry::{self, Kind};

/// Where per-user class registrations live.
pub const CLASSES: &str = r"Software\Classes";

const ITEM_TYPES: [&str; 3] = ["*", "Directory", "Drive"];

/// The menu entries: verb name, title, class ID. Explorer lists them in
/// the order of their verb names. Scrimp Savings… only shows with Shift
/// held; the menu DLL decides that.
const ENTRIES: [(&str, &str, u128); 3] = [
    ("ScrimpCompress", "Scrimp Compress", COMPRESS_COMMAND),
    ("ScrimpDecompress", "Scrimp Decompress", DECOMPRESS_COMMAND),
    ("ScrimpSavings", "Scrimp Savings", SAVINGS_COMMAND),
];

fn set(key: &str, name: Option<&str>, value: &str) -> Result<(), Error> {
    registry::set(key, name, value, Kind::Text)
}

/// Reads a string value, or `None` if it isn't there.
pub fn get(key: &str, name: Option<&str>) -> Result<Option<String>, Error> {
    registry::get(key, name)
}

fn delete(key: &str) -> Result<(), Error> {
    registry::delete_tree(key)
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
        let savings = format!(r"{classes}\Directory\shell\ScrimpSavings");
        assert_eq!(
            get(&savings, Some("ExplorerCommandHandler")).unwrap(),
            Some(braced(SAVINGS_COMMAND))
        );
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
