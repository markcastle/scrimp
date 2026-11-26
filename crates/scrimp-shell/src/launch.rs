//! Handing a selection to the worker: write a job file, start `scrimpw.exe`.

use std::path::{Path, PathBuf};

use scrimp_core::{Action, JobRequest};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW,
    GetModuleHandleExW,
};
use windows::core::PCWSTR;

/// This DLL's own path, found from the address of one of its functions.
pub(crate) fn this_dll() -> Option<PathBuf> {
    let mut module = HMODULE::default();
    let address = this_dll as *const () as *const u16;
    // SAFETY: asks which module holds an address inside this DLL, without
    // changing its reference count; `module` is a live local.
    unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            PCWSTR(address),
            &mut module,
        )
    }
    .ok()?;
    let mut buf = vec![0u16; 32_768];
    // SAFETY: `buf` is a live buffer and its length is passed with it.
    let len = unsafe { GetModuleFileNameW(Some(module), &mut buf) } as usize;
    (len > 0 && len < buf.len()).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..len])))
}

/// Where `scrimpw.exe` is, given this DLL's path: two folders up in the
/// installed layout (`shell\<version>\scrimp_shell.dll`), or beside it in a
/// development build.
pub(crate) fn worker_beside(dll: &Path) -> Option<PathBuf> {
    let folder = dll.parent()?;
    let candidates = [
        folder
            .parent()
            .and_then(Path::parent)
            .map(|root| root.join("scrimpw.exe")),
        Some(folder.join("scrimpw.exe")),
    ];
    candidates.into_iter().flatten().find(|p| p.is_file())
}

/// Writes the job and starts the worker on it. The worker is left running
/// on its own; Explorer doesn't wait for it.
pub(crate) fn submit(paths: Vec<PathBuf>, action: Action, jobs: &Path, worker: &Path) -> std::io::Result<PathBuf> {
    let job = JobRequest { action, paths }.write_to(jobs)?;
    std::process::Command::new(worker).arg("--job").arg(&job).spawn()?;
    Ok(job)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_worker_is_found_in_either_layout() {
        let dir = tempfile::tempdir().unwrap();
        let installed = dir.path().join("shell").join("0.2.0").join("scrimp_shell.dll");
        std::fs::create_dir_all(installed.parent().unwrap()).unwrap();
        assert_eq!(worker_beside(&installed), None);
        std::fs::write(dir.path().join("scrimpw.exe"), "").unwrap();
        assert_eq!(worker_beside(&installed), Some(dir.path().join("scrimpw.exe")));

        let built = dir.path().join("debug").join("scrimp_shell.dll");
        std::fs::create_dir_all(built.parent().unwrap()).unwrap();
        std::fs::write(dir.path().join("debug").join("scrimpw.exe"), "").unwrap();
        assert_eq!(
            worker_beside(&built),
            Some(dir.path().join("debug").join("scrimpw.exe"))
        );
    }

    #[test]
    fn submitting_writes_the_job_and_starts_the_worker() {
        let dir = tempfile::tempdir().unwrap();
        let jobs = dir.path().join("jobs");
        // Something that starts and exits at once stands in for the worker.
        let worker = PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join(r"System32\where.exe");
        let job = submit(
            vec![PathBuf::from(r"D:\Games")],
            Action::Change(scrimp_core::Target::Automatic),
            &jobs,
            &worker,
        )
        .unwrap();
        let read = JobRequest::from_json(&std::fs::read_to_string(job).unwrap()).unwrap();
        assert_eq!(read.paths, [PathBuf::from(r"D:\Games")]);

        let missing = dir.path().join("scrimpw.exe");
        assert!(
            submit(
                vec![PathBuf::from(r"D:\Games")],
                Action::Change(scrimp_core::Target::Automatic),
                &jobs,
                &missing
            )
            .is_err()
        );
    }

    #[test]
    fn this_dll_knows_where_it_is() {
        // In the tests "this DLL" is the test program itself.
        let me = this_dll().unwrap();
        assert!(me.is_file(), "{me:?}");
    }
}
