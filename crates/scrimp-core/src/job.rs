//! Job files: how the Explorer menu hands work to the worker.
//!
//! The menu writes a small JSON file into `%LOCALAPPDATA%\Scrimp\jobs` and
//! starts `scrimpw.exe`. Paths never pass through a command line or a
//! shell, so a selection of any size and any file names is safe.
//!
//! Only one worker runs at a time for a user. It takes a job by renaming
//! its file, which only one process can do, so no job is ever run twice,
//! and it keeps taking jobs, oldest first, until the folder is empty.
//!
//! The worker treats a job file as untrusted: it must be valid, name only
//! absolute paths on drives or shares, and nothing that addresses a device.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::algorithm::Target;

/// The version of the job file format.
pub const JOB_SCHEMA_VERSION: u32 = 1;

/// Jobs with more paths than this are refused; Explorer can't select more.
const MOST_PATHS: usize = 1_000_000;

/// Files a worker is running end with this, so other workers leave them.
const CLAIMED: &str = "running";

/// What one right-click asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobRequest {
    pub action: Action,
    pub paths: Vec<PathBuf>,
}

/// What to do with a job's paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Compress or decompress: Scrimp Compress, its Shift choices, and
    /// Scrimp Decompress.
    Change(Target),
    /// Look and report, changing nothing: Scrimp Savings….
    Savings,
}

/// Why a job file was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("This job can't be run: {0}")]
pub struct JobError(String);

impl JobRequest {
    /// The file's contents.
    pub fn to_json(&self) -> String {
        let (operation, method) = match self.action {
            Action::Change(Target::Uncompressed) => ("decompress", None),
            Action::Change(Target::Automatic) => ("compress", Some("auto")),
            Action::Change(Target::Algorithm(a)) => ("compress", Some(a.cli_name())),
            Action::Savings => ("savings", None),
        };
        let paths: Vec<String> = self.paths.iter().map(|p| p.to_string_lossy().into_owned()).collect();
        let mut v = json!({ "schemaVersion": JOB_SCHEMA_VERSION, "operation": operation, "paths": paths });
        if let Some(m) = method {
            v["method"] = json!(m);
        }
        v.to_string()
    }

    /// Reads and checks a job file's contents.
    pub fn from_json(text: &str) -> Result<JobRequest, JobError> {
        let v: Value = serde_json::from_str(text).map_err(|_| JobError("the job file isn't valid.".into()))?;
        if v["schemaVersion"].as_u64() != Some(u64::from(JOB_SCHEMA_VERSION)) {
            return Err(JobError("it was written by a different version of Scrimp.".into()));
        }
        let action = match (v["operation"].as_str(), v["method"].as_str()) {
            (Some("decompress"), _) => Action::Change(Target::Uncompressed),
            (Some("savings"), _) => Action::Savings,
            (Some("compress"), Some(m)) => Action::Change(
                m.parse::<Target>()
                    .map_err(|_| JobError(format!("\"{m}\" isn't a compression method.")))?,
            ),
            _ => return Err(JobError("it doesn't say what to do.".into())),
        };
        let paths: Vec<PathBuf> = v["paths"]
            .as_array()
            .ok_or_else(|| JobError("it has no files or folders.".into()))?
            .iter()
            .map(|p| p.as_str().map(PathBuf::from))
            .collect::<Option<_>>()
            .ok_or_else(|| JobError("one of its paths isn't text.".into()))?;
        if paths.is_empty() {
            return Err(JobError("it has no files or folders.".into()));
        }
        if paths.len() > MOST_PATHS {
            return Err(JobError("it has too many paths.".into()));
        }
        if let Some(bad) = paths.iter().find(|p| !is_plain_absolute(p)) {
            return Err(JobError(format!(
                "\"{}\" isn't a file or folder Scrimp can work on.",
                bad.display()
            )));
        }
        Ok(JobRequest { action, paths })
    }

    /// Writes the job into `folder` under a new name, never replacing
    /// another job, and returns its path.
    pub fn write_to(&self, folder: &Path) -> std::io::Result<PathBuf> {
        use std::io::Write;
        std::fs::create_dir_all(folder)?;
        loop {
            let path = folder.join(format!("{}.json", unique_name()));
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut f) => {
                    f.write_all(self.to_json().as_bytes())?;
                    return Ok(path);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
    }
}

/// `%LOCALAPPDATA%\Scrimp\jobs`, or `None` off Windows.
pub fn jobs_folder() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| Path::new(&d).join("Scrimp").join("jobs"))
}

/// `%LOCALAPPDATA%\Scrimp\admin-jobs`: jobs for a worker running as
/// administrator, made by **Try Again as Administrator**. It has its own
/// folder, and so its own lock, so it never takes the everyday worker's jobs.
/// The everyday user can write here, so that worker shows the paths it was
/// given and asks before it changes anything.
pub fn admin_jobs_folder() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| Path::new(&d).join("Scrimp").join("admin-jobs"))
}

/// The name of the lock that makes one worker at a time take jobs from
/// `folder`. It lives in the session's `Local\` namespace, so it is the
/// user's own.
pub fn worker_lock_name(folder: &Path) -> String {
    // FNV-1a over the folder's name, ignoring case as Windows does.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in folder.to_string_lossy().to_lowercase().bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("Local\\Scrimp.Worker.{hash:016x}")
}

/// Whether any job is waiting in `folder`.
pub fn has_waiting(folder: &Path) -> bool {
    std::fs::read_dir(folder).is_ok_and(|entries| {
        entries
            .filter_map(Result::ok)
            .any(|e| e.path().extension().is_some_and(|x| x == "json"))
    })
}

/// Takes the oldest waiting job in `folder`: renames its file so no other
/// worker can take it, reads it, and deletes it. `None` when there are no
/// waiting jobs. A job file that can't be read or is invalid is deleted and
/// reported, so it can't block the jobs behind it.
pub fn take_next(folder: &Path) -> Option<(PathBuf, Result<JobRequest, JobError>)> {
    let mut waiting: Vec<(SystemTime, PathBuf)> = std::fs::read_dir(folder)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .map(|p| {
            (
                std::fs::metadata(&p).and_then(|m| m.modified()).unwrap_or(UNIX_EPOCH),
                p,
            )
        })
        .collect();
    waiting.sort();
    for (_, path) in waiting {
        let claimed = path.with_extension(CLAIMED);
        if std::fs::rename(&path, &claimed).is_err() {
            // Another worker took it first.
            continue;
        }
        let job = std::fs::read_to_string(&claimed)
            .map_err(|_| JobError("the job file couldn't be read.".into()))
            .and_then(|text| JobRequest::from_json(&text));
        let _ = std::fs::remove_file(&claimed);
        return Some((path, job));
    }
    None
}

/// A path the worker may be asked to work on: `C:\...` or `\\server\share\...`,
/// and never one that reaches a device (`\\.\`, `\\?\GLOBALROOT`), a
/// relative path, or a name with a NUL in it.
fn is_plain_absolute(path: &Path) -> bool {
    let text = path.to_string_lossy();
    if text.contains('\0') {
        return false;
    }
    let text = text.strip_prefix(r"\\?\").unwrap_or(&text);
    let b = text.as_bytes();
    let drive = b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/');
    let share = (text.starts_with(r"\\") || text.starts_with("UNC\\"))
        && !text.starts_with(r"\\.\")
        && !text.starts_with(r"\\?\")
        && text
            .trim_start_matches(['\\'])
            .trim_start_matches("UNC\\")
            .split('\\')
            .filter(|s| !s.is_empty())
            .count()
            >= 2;
    let device = text.to_ascii_lowercase().starts_with("globalroot");
    (drive || share) && !device && !text.split(['\\', '/']).any(|part| part == "..")
}

/// A name no other job file has: the time, this process, and a counter.
fn unique_name() -> String {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    format!(
        "{nanos:x}-{:x}-{:x}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::algorithm::Algorithm;

    fn job(target: Target, paths: &[&str]) -> JobRequest {
        JobRequest {
            action: Action::Change(target),
            paths: paths.iter().map(PathBuf::from).collect(),
        }
    }

    #[test]
    fn a_job_reads_back_exactly() {
        for target in [
            Target::Automatic,
            Target::Uncompressed,
            Target::Algorithm(Algorithm::Lzx),
        ] {
            let j = job(
                target,
                &[
                    r"D:\Games",
                    r"C:\Users\You\Documents\Report-2019.docx",
                    r"\\NAS\Archive\Old",
                ],
            );
            assert_eq!(JobRequest::from_json(&j.to_json()), Ok(j));
        }
        let savings = JobRequest {
            action: Action::Savings,
            paths: vec![PathBuf::from(r"D:\Games")],
        };
        assert!(savings.to_json().contains(r#""operation":"savings""#));
        assert_eq!(JobRequest::from_json(&savings.to_json()), Ok(savings));
    }

    #[test]
    fn odd_but_real_names_survive() {
        let j = job(
            Target::Automatic,
            &[r#"D:\Games\"Quoted" & <odd> %names%.dat"#, r"D:\Ünïcödé\写真.jpg"],
        );
        assert_eq!(JobRequest::from_json(&j.to_json()).unwrap().paths, j.paths);
    }

    #[test]
    fn anything_but_plain_absolute_paths_is_refused() {
        for bad in [
            r"Games",
            r"..\Games",
            r"D:\Games\..\..\Windows",
            r"\\.\PhysicalDrive0",
            r"\\?\GLOBALROOT\Device\HarddiskVolume1",
            r"\\NAS",
            "D:\\Games\0evil",
        ] {
            let text = job(Target::Automatic, &[bad]).to_json();
            assert!(JobRequest::from_json(&text).is_err(), "{bad}");
        }
        for good in [
            r"D:\Games",
            r"\\?\D:\Games",
            r"\\NAS\Archive",
            r"\\?\UNC\NAS\Archive",
            "D:/Games",
        ] {
            assert!(is_plain_absolute(Path::new(good)), "{good}");
        }
    }

    #[test]
    fn broken_or_foreign_job_files_are_refused_plainly() {
        let cases = [
            ("not json", "the job file isn't valid."),
            (
                r#"{"schemaVersion":2,"operation":"compress","method":"auto","paths":["D:\\Games"]}"#,
                "different version",
            ),
            (
                r#"{"schemaVersion":1,"operation":"shred","paths":["D:\\Games"]}"#,
                "doesn't say what to do",
            ),
            (
                r#"{"schemaVersion":1,"operation":"compress","paths":["D:\\Games"]}"#,
                "doesn't say what to do",
            ),
            (
                r#"{"schemaVersion":1,"operation":"compress","method":"zip","paths":["D:\\Games"]}"#,
                "isn't a compression method",
            ),
            (
                r#"{"schemaVersion":1,"operation":"decompress","paths":[]}"#,
                "no files or folders",
            ),
            (r#"{"schemaVersion":1,"operation":"decompress"}"#, "no files or folders"),
            (
                r#"{"schemaVersion":1,"operation":"decompress","paths":[7]}"#,
                "isn't text",
            ),
        ];
        for (text, wanted) in cases {
            let err = JobRequest::from_json(text).unwrap_err().to_string();
            assert!(
                err.starts_with("This job can't be run: ") && err.contains(wanted),
                "{text}: {err}"
            );
        }
    }

    #[test]
    fn jobs_are_taken_once_oldest_first_and_bad_ones_dont_block() {
        let dir = tempfile::tempdir().unwrap();
        let first = job(Target::Automatic, &[r"D:\Games"]).write_to(dir.path()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let broken = dir.path().join("zz-broken.json");
        std::fs::write(&broken, "not json").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        job(Target::Uncompressed, &[r"D:\Archive"])
            .write_to(dir.path())
            .unwrap();
        std::fs::write(dir.path().join("notes.txt"), "not a job").unwrap();

        let (path, taken) = take_next(dir.path()).unwrap();
        assert_eq!(path, first);
        assert_eq!(taken.unwrap().paths, [PathBuf::from(r"D:\Games")]);
        let (_, taken) = take_next(dir.path()).unwrap();
        assert!(taken.is_err(), "the broken job is reported");
        let (_, taken) = take_next(dir.path()).unwrap();
        assert_eq!(taken.unwrap().action, Action::Change(Target::Uncompressed));
        assert!(take_next(dir.path()).is_none());
        let left: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(left, ["notes.txt"], "taken jobs are deleted");
    }

    #[test]
    fn a_job_being_run_is_left_alone_by_other_workers() {
        let dir = tempfile::tempdir().unwrap();
        let path = job(Target::Automatic, &[r"D:\Games"]).write_to(dir.path()).unwrap();
        std::fs::rename(&path, path.with_extension(CLAIMED)).unwrap();
        assert!(take_next(dir.path()).is_none());
        assert!(take_next(&dir.path().join("missing")).is_none());
    }

    #[test]
    fn each_jobs_folder_has_its_own_lock() {
        let a = worker_lock_name(Path::new(r"C:\Users\You\AppData\Local\Scrimp\jobs"));
        assert!(a.starts_with("Local\\Scrimp.Worker."));
        assert_eq!(
            a,
            worker_lock_name(Path::new(r"c:\users\you\appdata\local\scrimp\jobs"))
        );
        assert_ne!(a, worker_lock_name(Path::new(r"D:\Scrimp\jobs")));
        assert!(!a[6..].contains('\\'), "only the namespace may have a backslash");
    }

    #[test]
    fn waiting_jobs_are_noticed() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!has_waiting(dir.path()));
        std::fs::write(dir.path().join("notes.txt"), "").unwrap();
        assert!(!has_waiting(dir.path()));
        job(Target::Automatic, &[r"D:\Games"]).write_to(dir.path()).unwrap();
        assert!(has_waiting(dir.path()));
        assert!(!has_waiting(&dir.path().join("missing")));
    }

    #[test]
    fn job_names_never_repeat() {
        let names: std::collections::HashSet<_> = (0..1000).map(|_| unique_name()).collect();
        assert_eq!(names.len(), 1000);
        if let Some(folder) = jobs_folder() {
            assert!(folder.ends_with(Path::new("Scrimp").join("jobs")));
        }
        if let (Some(everyday), Some(admin)) = (jobs_folder(), admin_jobs_folder()) {
            assert_eq!(everyday.parent(), admin.parent());
            assert_ne!(
                worker_lock_name(&everyday),
                worker_lock_name(&admin),
                "separate workers"
            );
        }
    }
}
