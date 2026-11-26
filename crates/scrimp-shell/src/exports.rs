//! What Explorer calls to load the menu: the class factory and the two
//! standard COM exports.

use std::ffi::c_void;
use std::sync::atomic::Ordering;

use windows::Win32::Foundation::{
    CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_POINTER, E_UNEXPECTED, S_FALSE, S_OK,
};
use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl};
use windows::Win32::UI::Shell::IExplorerCommand;
use windows_core::{BOOL, GUID, HRESULT, IUnknown, Interface, Ref, Result, implement};

use crate::command::{Alive, Command, Kind, LIVE, guard};

#[implement(IClassFactory)]
pub(crate) struct Factory {
    kind: Kind,
    _alive: Alive,
}

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(&self, outer: Ref<IUnknown>, iid: *const GUID, object: *mut *mut c_void) -> Result<()> {
        guard(|| {
            if object.is_null() {
                return Err(E_POINTER.into());
            }
            // SAFETY: checked not null; cleared before anything else can fail.
            unsafe { object.write(std::ptr::null_mut()) };
            if outer.is_some() {
                return Err(CLASS_E_NOAGGREGATION.into());
            }
            let command: IExplorerCommand = Command::new(self.kind).into();
            // SAFETY: `iid` and `object` come from the caller as COM requires.
            unsafe { command.query(iid, object) }.ok()
        })
    }

    fn LockServer(&self, _lock: BOOL) -> Result<()> {
        Ok(())
    }
}

/// Explorer asks for a class factory for one of Scrimp's two class IDs.
///
/// # Safety
/// Called by COM with valid pointers, as for any `DllGetClassObject`.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllGetClassObject(
    clsid: *const GUID,
    iid: *const GUID,
    object: *mut *mut c_void,
) -> HRESULT {
    std::panic::catch_unwind(|| {
        if clsid.is_null() || iid.is_null() || object.is_null() {
            return E_POINTER;
        }
        // SAFETY: checked not null above.
        unsafe { object.write(std::ptr::null_mut()) };
        // SAFETY: checked not null above; COM passes a valid GUID.
        let Some(kind) = Kind::from_id(unsafe { *clsid }.to_u128()) else {
            return CLASS_E_CLASSNOTAVAILABLE;
        };
        let factory: IClassFactory = Factory {
            kind,
            _alive: Alive::new(),
        }
        .into();
        // SAFETY: `iid` and `object` are valid, as checked above.
        unsafe { factory.query(iid, object) }
    })
    .unwrap_or(E_UNEXPECTED)
}

/// Explorer asks whether the DLL can be unloaded: yes once no object of
/// ours is alive.
#[unsafe(no_mangle)]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    if LIVE.load(Ordering::SeqCst) == 0 {
        S_OK
    } else {
        S_FALSE
    }
}
