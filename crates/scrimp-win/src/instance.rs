//! One worker at a time for each signed-in user.
//!
//! A named mutex in the session's `Local\` namespace marks the running
//! worker. A second worker that finds it taken leaves its job in the jobs
//! folder for the running one and exits.

use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
use windows::Win32::System::Threading::{CreateMutexW, ReleaseMutex};
use windows::core::PCWSTR;

use crate::error::{Call, Error};

/// Held while this process is the worker. Released when dropped, on the
/// thread that took it, as a mutex must be.
pub struct SingleInstance(HANDLE);

impl SingleInstance {
    /// Becomes the one instance named `name`, or `None` if another process
    /// already is.
    pub fn try_acquire(name: &str) -> Result<Option<SingleInstance>, Error> {
        let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        // SAFETY: `wide` is NUL-terminated and outlives the call; asking for
        // initial ownership only succeeds if this call creates the mutex.
        let handle = unsafe { CreateMutexW(None, true, PCWSTR(wide.as_ptr())) }
            .map_err(|e| Error::from_windows(&e, Call::Other))?;
        // SAFETY: reads the last error the call above set.
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            // SAFETY: the handle was just opened and is closed once, here.
            unsafe {
                let _ = CloseHandle(handle);
            }
            return Ok(None);
        }
        Ok(Some(SingleInstance(handle)))
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        // SAFETY: this process owns the mutex, taken on this thread, and the
        // handle is released and closed exactly once, here.
        unsafe {
            let _ = ReleaseMutex(self.0);
            let _ = CloseHandle(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_one_holder_at_a_time() {
        let name = format!("Local\\Scrimp.Test.{}", std::process::id());
        let first = SingleInstance::try_acquire(&name).unwrap();
        assert!(first.is_some());
        // Another thread stands in for another process.
        let name2 = name.clone();
        let second = std::thread::spawn(move || SingleInstance::try_acquire(&name2).unwrap().is_some());
        assert!(!second.join().unwrap(), "a second holder got in");
        drop(first);
        let again = std::thread::spawn(move || SingleInstance::try_acquire(&name).unwrap().is_some());
        assert!(again.join().unwrap(), "released, so free again");
    }
}
