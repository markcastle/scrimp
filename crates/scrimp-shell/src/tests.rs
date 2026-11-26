//! The menu driven the way Explorer drives it: through `DllGetClassObject`,
//! with real shell item arrays built from real files.

use std::path::{Path, PathBuf};
use std::time::Instant;

use scrimp_core::Algorithm;
use scrimp_core::shell_ids::{COMPRESS_COMMAND, DECOMPRESS_COMMAND};
use windows::Win32::Foundation::{CLASS_E_CLASSNOTAVAILABLE, E_NOTIMPL, E_UNEXPECTED, S_FALSE};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree, IClassFactory};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    ECF_DEFAULT, ECS_ENABLED, ECS_HIDDEN, IExplorerCommand, IShellItemArray, SHCreateShellItemArrayFromIDLists,
    SHParseDisplayName,
};
use windows_core::{GUID, HRESULT, Interface, PCWSTR, PWSTR};

use crate::command::{Command, Kind};
use crate::exports::{DllCanUnloadNow, DllGetClassObject};

fn com() {
    // SAFETY: initialises COM for this test thread, as Explorer's threads are.
    let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
}

/// A shell item array for these paths, as Explorer passes a selection.
fn items(paths: &[&Path]) -> IShellItemArray {
    let mut pidls: Vec<*mut ITEMIDLIST> = Vec::new();
    for p in paths {
        let wide: Vec<u16> = p.as_os_str().to_string_lossy().encode_utf16().chain(Some(0)).collect();
        let mut pidl = std::ptr::null_mut();
        // SAFETY: `wide` is NUL-terminated; `pidl` receives a new item list.
        unsafe { SHParseDisplayName(PCWSTR(wide.as_ptr()), None, &mut pidl, 0, None) }.unwrap();
        pidls.push(pidl);
    }
    let list: Vec<*const ITEMIDLIST> = pidls.iter().map(|p| p.cast_const()).collect();
    // SAFETY: every item list is valid; the array copies what it needs.
    let array = unsafe { SHCreateShellItemArrayFromIDLists(&list) }.unwrap();
    for p in pidls {
        // SAFETY: frees each item list once.
        unsafe { CoTaskMemFree(Some(p.cast())) };
    }
    array
}

/// One of the menu entries, made the way Explorer makes it.
fn command(id: u128) -> IExplorerCommand {
    let mut factory: *mut std::ffi::c_void = std::ptr::null_mut();
    let clsid = GUID::from_u128(id);
    // SAFETY: valid pointers to locals, as COM passes.
    let hr = unsafe { DllGetClassObject(&clsid, &IClassFactory::IID, &mut factory) };
    hr.ok().unwrap();
    // SAFETY: on success `factory` holds an IClassFactory with one reference, now owned here.
    let factory = unsafe { IClassFactory::from_raw(factory) };
    // SAFETY: an ordinary call on a live interface.
    unsafe { factory.CreateInstance::<_, IExplorerCommand>(None) }.unwrap()
}

fn title(c: &IExplorerCommand) -> String {
    // SAFETY: an ordinary call; the returned string is ours to free.
    let p: PWSTR = unsafe { c.GetTitle(None) }.unwrap();
    // SAFETY: a NUL-terminated string just returned, freed once.
    let text = unsafe { p.to_string() }.unwrap();
    // SAFETY: frees the string GetTitle allocated.
    unsafe { CoTaskMemFree(Some(p.0.cast())) };
    text
}

fn state(c: &IExplorerCommand, selection: &IShellItemArray, slow: bool) -> Result<u32, HRESULT> {
    // SAFETY: an ordinary call on live interfaces.
    unsafe { c.GetState(selection, slow) }.map_err(|e| e.code())
}

const SHOWN: Result<u32, HRESULT> = Ok(ECS_ENABLED.0 as u32);
const HIDDEN: Result<u32, HRESULT> = Ok(ECS_HIDDEN.0 as u32);

fn text_file(dir: &Path, name: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, "Row 1: Example.dat moved to D:\\Archive.\n".repeat(30_000)).unwrap();
    p
}

#[test]
fn explorer_gets_both_commands_and_nothing_else() {
    com();
    assert_eq!(title(&command(COMPRESS_COMMAND)), "Scrimp Compress");
    assert_eq!(title(&command(DECOMPRESS_COMMAND)), "Scrimp Decompress");

    let mut object: *mut std::ffi::c_void = std::ptr::null_mut();
    let other = GUID::from_u128(0x1234);
    // SAFETY: valid pointers to locals.
    let hr = unsafe { DllGetClassObject(&other, &IClassFactory::IID, &mut object) };
    assert_eq!(hr, CLASS_E_CLASSNOTAVAILABLE);
    assert!(object.is_null());
    // SAFETY: null pointers must be refused, not followed.
    assert!(unsafe { DllGetClassObject(std::ptr::null(), &IClassFactory::IID, &mut object) }.is_err());
}

#[test]
fn a_single_file_shows_the_entry_that_makes_sense() {
    com();
    let dir = tempfile::tempdir().unwrap();
    let file = text_file(dir.path(), "notes.txt");
    let selection = items(&[&file]);
    let (compress, decompress) = (command(COMPRESS_COMMAND), command(DECOMPRESS_COMMAND));

    // When Explorer can't wait, nothing touches the disk and the entry shows.
    assert_eq!(state(&compress, &selection, false), SHOWN);
    assert_eq!(state(&compress, &selection, true), SHOWN);
    assert_eq!(state(&decompress, &selection, true), HIDDEN, "nothing to decompress");

    scrimp_win::compress(&file, Algorithm::Lzx).unwrap();
    assert_eq!(
        state(&compress, &selection, true),
        HIDDEN,
        "already compressed, and Shift isn't held"
    );
    assert_eq!(state(&decompress, &selection, true), SHOWN);
}

#[test]
fn folders_and_several_items_show_both_entries() {
    com();
    let dir = tempfile::tempdir().unwrap();
    let a = text_file(dir.path(), "a.txt");
    let b = text_file(dir.path(), "b.txt");
    for selection in [items(&[dir.path()]), items(&[&a, &b])] {
        assert_eq!(state(&command(COMPRESS_COMMAND), &selection, true), SHOWN);
        assert_eq!(state(&command(DECOMPRESS_COMMAND), &selection, true), SHOWN);
    }
    // A network location never shows, decided without touching the network.
    let share = items(&[Path::new(r"\\localhost\C$")]);
    assert_eq!(state(&command(COMPRESS_COMMAND), &share, false), HIDDEN);

    // Nothing selected: nothing shows.
    let compress = command(COMPRESS_COMMAND);
    // SAFETY: an ordinary call with no selection, as Explorer may make.
    let nothing = unsafe { compress.GetState(None, true) };
    assert_eq!(nothing.map_err(|e| e.code()), HIDDEN);
}

#[test]
fn building_the_menu_is_quick() {
    com();
    let dir = tempfile::tempdir().unwrap();
    let file = text_file(dir.path(), "notes.txt");
    let (folder, single) = (items(&[dir.path()]), items(&[&file]));
    let compress = command(COMPRESS_COMMAND);
    // Warm up once, as Explorer has loaded everything by the time it asks.
    let _ = state(&compress, &single, true);

    let started = Instant::now();
    let _ = title(&compress);
    // SAFETY: an ordinary call.
    assert_eq!(unsafe { compress.GetFlags() }.unwrap(), ECF_DEFAULT.0 as u32);
    let _ = state(&compress, &folder, false);
    let fast = started.elapsed();
    let started = Instant::now();
    let _ = state(&compress, &single, true);
    let slow = started.elapsed();
    assert!(fast.as_millis() < 50, "the quick path took {fast:?}");
    assert!(slow.as_millis() < 50, "the full check of one file took {slow:?}");
}

#[test]
fn a_panic_comes_back_as_an_error_and_never_reaches_explorer() {
    com();
    let mut broken = Command::new(Kind::Compress);
    broken.panic = true;
    let broken: IExplorerCommand = broken.into();
    // SAFETY: ordinary calls; each must return, not unwind.
    unsafe {
        assert_eq!(broken.GetTitle(None).map_err(|e| e.code()), Err(E_UNEXPECTED));
        assert_eq!(broken.GetState(None, true).map_err(|e| e.code()), Err(E_UNEXPECTED));
        assert_eq!(broken.GetFlags().map_err(|e| e.code()), Err(E_UNEXPECTED));
        assert_eq!(broken.Invoke(None, None).map_err(|e| e.code()), Err(E_UNEXPECTED));
    }
}

#[test]
fn the_shift_submenu_offers_four_plain_choices() {
    com();
    // SAFETY: ordinary calls on live interfaces.
    let choices = unsafe { command(COMPRESS_COMMAND).EnumSubCommands() }.unwrap();
    let mut out: [Option<IExplorerCommand>; 6] = Default::default();
    let mut fetched = 0u32;
    // SAFETY: room for 6 items and a place for the count.
    let hr = unsafe { choices.Next(&mut out, Some(&mut fetched)) };
    assert_eq!((hr, fetched), (S_FALSE, 4), "fewer than asked for");
    let titles: Vec<String> = out.iter().flatten().map(title).collect();
    assert_eq!(
        titles,
        ["Automatic", "Faster to Open", "Smallest", "For Files That Change"]
    );
    let dir = tempfile::tempdir().unwrap();
    let selection = items(&[dir.path()]);
    assert_eq!(
        state(out[2].as_ref().unwrap(), &selection, false),
        SHOWN,
        "a choice shows without waiting"
    );

    // SAFETY: ordinary calls.
    unsafe {
        choices.Reset().unwrap();
        assert_eq!(
            command(DECOMPRESS_COMMAND)
                .EnumSubCommands()
                .map(|_| ())
                .map_err(|e| e.code()),
            Err(E_NOTIMPL)
        );
    }
}

#[test]
fn the_dll_stays_loaded_while_its_objects_are_in_use() {
    com();
    let held = command(COMPRESS_COMMAND);
    assert_eq!(DllCanUnloadNow(), S_FALSE);
    drop(held);
}
