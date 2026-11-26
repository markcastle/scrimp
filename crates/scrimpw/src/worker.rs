//! Taking jobs and running them, with a progress dialog and a summary.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Mutex, PoisonError};

use scrimp_core::{
    Control, Event, FailReason, Job, JobError, JobRequest, Mode, Outcome, Report, Settings, Target, admin_jobs_folder,
    format_count, format_size, has_waiting, jobs_folder, take_next, worker_lock_name,
};
use scrimp_win::ui::{self, Button, Finish, ProgressView};
use scrimp_win::{SingleInstance, WindowsDisk};

use crate::Args;

const TITLE: &str = "Scrimp";

pub fn main(args: &Args) -> ExitCode {
    if args.register || args.unregister {
        return menu(args);
    }
    let Some(folder) = args.jobs_folder.clone().or_else(jobs_folder) else {
        return ExitCode::from(5);
    };
    let lock_name = worker_lock_name(&folder);
    loop {
        let lock = match SingleInstance::try_acquire(&lock_name) {
            Ok(Some(lock)) => lock,
            // Another worker is running and will take the job.
            Ok(None) => return ExitCode::SUCCESS,
            Err(_) => return ExitCode::from(5),
        };
        while let Some((_, job)) = take_next(&folder) {
            run(job, args);
        }
        drop(lock);
        // A job may have arrived while letting go, after a new worker saw
        // the lock still held and left. If so, carry on with it.
        if !has_waiting(&folder) {
            return ExitCode::SUCCESS;
        }
    }
}

/// Where the menu DLL is: in the installed layout,
/// `shell\<version>\scrimp_shell.dll` beside this program (one folder per
/// version, because Explorer keeps the old one loaded); in a development
/// build, `scrimp_shell.dll` beside it.
fn find_dll(exe_folder: &Path) -> Option<PathBuf> {
    let installed = exe_folder
        .join("shell")
        .join(env!("CARGO_PKG_VERSION"))
        .join("scrimp_shell.dll");
    let built = exe_folder.join("scrimp_shell.dll");
    [installed, built].into_iter().find(|p| p.is_file())
}

/// Removes the folders of older versions' menu DLLs from `shell\`. Explorer
/// may still have one loaded, which Windows won't let go; that folder is
/// left for next time.
fn clean_old_shells(exe_folder: &Path) {
    let Ok(entries) = std::fs::read_dir(exe_folder.join("shell")) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name() != env!("CARGO_PKG_VERSION") && entry.path().is_dir() {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

fn menu(args: &Args) -> ExitCode {
    let exe_folder = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    if args.unregister {
        let removed = scrimp_win::menu::unregister().and_then(|()| match &exe_folder {
            Some(folder) => scrimp_win::user_path::remove(folder).map(|_| ()),
            None => Ok(()),
        });
        return match removed {
            Ok(()) => {
                say(args, "Scrimp is no longer in Explorer's menu", "", None);
                ExitCode::SUCCESS
            }
            Err(e) => {
                say(
                    args,
                    "Scrimp couldn't be taken out of Explorer's menu",
                    &format!("{e}."),
                    None,
                );
                ExitCode::from(5)
            }
        };
    }
    let Some((folder, dll)) = exe_folder.and_then(|f| find_dll(&f).map(|d| (f, d))) else {
        say(
            args,
            "Scrimp couldn't be added to Explorer's menu",
            "scrimp_shell.dll isn't next to scrimpw.exe. Reinstalling Scrimp should put it back.",
            None,
        );
        return ExitCode::from(5);
    };
    let added = scrimp_win::menu::register(&dll).and_then(|()| {
        if args.add_to_path {
            scrimp_win::user_path::add(&folder).map(|_| ())
        } else {
            Ok(())
        }
    });
    clean_old_shells(&folder);
    match added {
        Ok(()) => {
            say(
                args,
                "Scrimp is in Explorer's menu",
                "Right-click a file or folder and choose Scrimp Compress. On Windows 11, it's under Show more options.",
                None,
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            say(
                args,
                "Scrimp couldn't be added to Explorer's menu",
                &format!("{e}."),
                None,
            );
            ExitCode::from(5)
        }
    }
}

fn say(args: &Args, headline: &str, body: &str, details: Option<&str>) {
    if args.quiet {
        println!("{headline}");
        if !body.is_empty() {
            println!("{body}");
        }
        if let Some(d) = details {
            println!("{d}");
        }
    } else {
        ui::message(TITLE, headline, body, details);
    }
}

fn run(job: Result<JobRequest, JobError>, args: &Args) {
    let request = match job {
        Ok(r) => r,
        Err(e) => return say(args, "This job couldn't be run", &e.to_string(), None),
    };
    let settings = match Settings::default_path().map(|p| Settings::load(&p)) {
        Some(Err(e)) => return say(args, "Scrimp's settings need fixing", &e.to_string(), None),
        Some(Ok(s)) => s,
        None => Settings::default(),
    };
    let name = name_of(&request.paths);
    let decompressing = request.target == Target::Uncompressed;
    let doing = if decompressing { "Decompressing" } else { "Compressing" };
    let headline = format!("{doing} {name}");
    let elevated = scrimp_win::is_elevated();

    if !args.quiet {
        if elevated && !confirm_as_administrator(&request, decompressing) {
            return;
        }
        if !decompressing && !elevated && !first_use_agreed() {
            return;
        }
    }

    let target = request.target;
    let control = Control::new();
    let live = Mutex::new(Live::new(if decompressing {
        Mode::Decompress
    } else {
        Mode::Compress
    }));
    let job = Job {
        paths: request.paths,
        target: request.target,
    };
    let policy = settings.policy();

    let report = std::thread::scope(|scope| {
        let work = scope.spawn(|| {
            let report = scrimp_core::run(&job, &policy, &settings.workers, &WindowsDisk, &control, |e| {
                live.lock().unwrap_or_else(PoisonError::into_inner).add(e);
            });
            live.lock().unwrap_or_else(PoisonError::into_inner).finished = true;
            report
        });
        if !args.quiet {
            let mut poll = || {
                let state = live.lock().unwrap_or_else(PoisonError::into_inner);
                ProgressView {
                    headline: headline.clone(),
                    detail: state.detail(control.is_cancelled()),
                    finished: state.finished,
                }
            };
            let mut cancel = || control.cancel();
            ui::progress(TITLE, &mut poll, &mut cancel);
        }
        work.join()
    });

    let Ok(report) = report else {
        return say(
            args,
            "Something went wrong",
            "Scrimp stopped unexpectedly. No file was left half-changed.",
            None,
        );
    };
    let state = live.into_inner().unwrap_or_else(PoisonError::into_inner);
    let lines = report.summary(Some(&name));
    let mut body: Vec<String> = lines.iter().skip(1).cloned().collect();
    if control.is_cancelled() {
        body.push("Stopped. Files already done keep their new size.".into());
    }
    let details = (!state.problems.is_empty()).then(|| state.problems.join("\n"));
    if args.quiet || elevated || state.denied.is_empty() {
        return say(args, &lines[0], &body.join("\n"), details.as_deref());
    }
    let again = Button {
        text: "Try Again as Administrator",
        needs_admin: true,
    };
    if ui::ask(
        TITLE,
        &lines[0],
        &body.join("\n"),
        details.as_deref(),
        &[again],
        Finish::Close,
    ) == Some(0)
    {
        try_again_as_administrator(target, state.denied);
    }
}

/// Where the note that the first-use message was seen is kept.
fn first_use_note() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| Path::new(&d).join("Scrimp").join("first-use-seen"))
}

/// The first time Scrimp compresses anything for this person, says what it
/// does and how to undo it. Returns whether to go ahead.
fn first_use_agreed() -> bool {
    let Some(note) = first_use_note() else {
        return true;
    };
    if note.exists() {
        return true;
    }
    let go = ui::ask(
        TITLE,
        "Scrimp keeps your files exactly as they are, just smaller on disk",
        "Apps open them as normal. Use Scrimp Decompress to undo it at any time.",
        None,
        &[Button {
            text: "Continue",
            needs_admin: false,
        }],
        Finish::Cancel,
    ) == Some(0);
    if go {
        if let Some(folder) = note.parent() {
            let _ = std::fs::create_dir_all(folder);
        }
        let _ = std::fs::write(&note, "");
    }
    go
}

/// Running as administrator, the job came from a folder the everyday user
/// can write to, so the person is shown exactly what will change and asked.
fn confirm_as_administrator(request: &JobRequest, decompressing: bool) -> bool {
    let (body, details) = paths_to_confirm(&request.paths, decompressing);
    ui::ask(
        TITLE,
        "Scrimp is running as administrator",
        &body,
        details.as_deref(),
        &[Button {
            text: "Continue",
            needs_admin: false,
        }],
        Finish::Cancel,
    ) == Some(0)
}

/// How many paths are listed before "and N more".
const LISTED: usize = 10;

/// The text asking to go ahead with `paths`, and the full list behind
/// "Show details" when they don't all fit.
fn paths_to_confirm(paths: &[PathBuf], decompressing: bool) -> (String, Option<String>) {
    let verb = if decompressing { "decompress" } else { "compress" };
    let mut body = format!("It will {verb} these, and nothing else:\n");
    for p in paths.iter().take(LISTED) {
        body.push_str(&format!("\n{}", p.display()));
    }
    let details = (paths.len() > LISTED).then(|| {
        body.push_str(&format!("\n…and {} more", format_count((paths.len() - LISTED) as u64)));
        paths
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    });
    (body, details)
}

/// Hands the files Windows refused to a worker running as administrator.
/// Windows asks the person first; if they say no, nothing is left behind.
fn try_again_as_administrator(target: Target, paths: Vec<PathBuf>) {
    let failed = |e: &dyn std::fmt::Display| {
        ui::message(TITLE, "Scrimp couldn't start as administrator", &format!("{e}."), None)
    };
    let (Some(folder), Ok(exe)) = (admin_jobs_folder(), std::env::current_exe()) else {
        return failed(&"Windows didn't say where Scrimp is");
    };
    let job = match (JobRequest { target, paths }).write_to(&folder) {
        Ok(job) => job,
        Err(e) => return failed(&e),
    };
    let folder_arg = folder.to_string_lossy();
    match scrimp_win::run_as_administrator(&exe, &["--jobs-folder", &folder_arg]) {
        Ok(true) => {}
        Ok(false) => {
            let _ = std::fs::remove_file(&job);
        }
        Err(e) => {
            let _ = std::fs::remove_file(&job);
            failed(&e);
        }
    }
}

/// What the progress dialog shows, kept up to date as files finish.
struct Live {
    report: Report,
    files: u64,
    last: Option<String>,
    problems: Vec<String>,
    /// What Windows refused to change, for Try Again as Administrator.
    denied: Vec<PathBuf>,
    finished: bool,
}

impl Live {
    fn new(mode: Mode) -> Self {
        Live {
            report: Report::new(mode),
            files: 0,
            last: None,
            problems: Vec::new(),
            denied: Vec::new(),
            finished: false,
        }
    }

    fn add(&mut self, event: &Event) {
        match event {
            Event::File { path, outcome, on_disk } => {
                self.files += 1;
                self.report.add(outcome, *on_disk);
                self.last = path.file_name().map(|n| n.to_string_lossy().into_owned());
                if let Outcome::Failed { reason, .. } = outcome {
                    self.problems.push(format!("Couldn't change {}: {}", path.display(), reason.plain()));
                    if *reason == FailReason::AccessDenied {
                        self.denied.push(path.clone());
                    }
                }
            }
            Event::FolderUnreadable { path, reason } => {
                if *reason == FailReason::AccessDenied {
                    self.denied.push(path.clone());
                }
                self.problems.push(format!("Couldn't open {}, so the files in it were left alone.", path.display()));
            }
            Event::WofUnavailable { root } => self.problems.push(format!(
                "Windows' compression driver isn't running on {}. Choose For Files That Change to use NTFS compression there.",
                root.display()
            )),
        }
    }

    fn detail(&self, stopping: bool) -> String {
        let files = if self.files == 1 {
            "1 file".to_string()
        } else {
            format!("{} files", format_count(self.files))
        };
        let saved = self.report.saved();
        let space = match saved {
            0 => String::new(),
            s if s > 0 => format!(" Saved {} so far.", format_size(u64::try_from(s).unwrap_or(u64::MAX))),
            s => format!(
                " {} restored so far.",
                format_size(u64::try_from(-s).unwrap_or(u64::MAX))
            ),
        };
        let now = if stopping {
            "\nStopping after the file in progress…".to_string()
        } else {
            self.last.as_ref().map_or_else(String::new, |l| format!("\nLast: {l}"))
        };
        format!("{files} done.{space}{now}")
    }
}

/// What the dialogs call the selection: one item's name, or how many.
fn name_of(paths: &[PathBuf]) -> String {
    match paths {
        [one] => one
            .file_name()
            .map_or_else(|| one.display().to_string(), |n| n.to_string_lossy().into_owned()),
        many => format!("{} items", format_count(many.len() as u64)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scrimp_core::{Algorithm, Sizes};

    #[test]
    fn the_progress_text_reads_naturally() {
        let mut live = Live::new(Mode::Compress);
        assert_eq!(live.detail(false), "0 files done.");
        live.add(&Event::File {
            path: PathBuf::from(r"D:\Games\notes.txt"),
            outcome: Outcome::Compressed {
                algorithm: Algorithm::Lzx,
                sizes: Sizes {
                    logical: 2048,
                    before: 2048,
                    after: 1024,
                },
            },
            on_disk: 1024,
        });
        assert_eq!(live.detail(false), "1 file done. Saved 1.0 KB so far.\nLast: notes.txt");
        assert_eq!(
            live.detail(true),
            "1 file done. Saved 1.0 KB so far.\nStopping after the file in progress…"
        );
    }

    #[test]
    fn problems_are_kept_for_the_details() {
        let mut live = Live::new(Mode::Decompress);
        live.add(&Event::File {
            path: PathBuf::from(r"D:\Games\save.dat"),
            outcome: Outcome::Failed {
                reason: scrimp_core::FailReason::InUse,
                left_as: scrimp_core::CompressionState::Uncompressed,
            },
            on_disk: 0,
        });
        live.add(&Event::WofUnavailable {
            root: PathBuf::from(r"W:\"),
        });
        live.add(&Event::FolderUnreadable {
            path: PathBuf::from(r"D:\Games\Private"),
            reason: scrimp_core::FailReason::AccessDenied,
        });
        assert_eq!(live.problems.len(), 3);
        assert!(live.problems[0].contains("in use by another app"));
        assert_eq!(
            live.denied,
            [PathBuf::from(r"D:\Games\Private")],
            "only what Windows refused"
        );
        live.add(&Event::File {
            path: PathBuf::from(r"D:\Games\locked.dat"),
            outcome: Outcome::Failed {
                reason: scrimp_core::FailReason::AccessDenied,
                left_as: scrimp_core::CompressionState::Uncompressed,
            },
            on_disk: 0,
        });
        assert_eq!(live.denied.len(), 2);
    }

    #[test]
    fn running_as_administrator_shows_what_will_change() {
        let few = [PathBuf::from(r"D:\Games"), PathBuf::from(r"D:\Archive\2019.dat")];
        let (body, details) = paths_to_confirm(&few, false);
        assert_eq!(
            body,
            "It will compress these, and nothing else:\n\nD:\\Games\nD:\\Archive\\2019.dat"
        );
        assert_eq!(details, None);

        let many: Vec<PathBuf> = (0..12).map(|i| PathBuf::from(format!(r"D:\f{i}"))).collect();
        let (body, details) = paths_to_confirm(&many, true);
        assert!(body.starts_with("It will decompress these"));
        assert!(body.ends_with("D:\\f9\n…and 2 more"), "{body}");
        assert_eq!(details.unwrap().lines().count(), 12);
    }

    #[test]
    fn the_menu_dll_is_found_installed_or_built() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(find_dll(dir.path()), None);
        let built = dir.path().join("scrimp_shell.dll");
        std::fs::write(&built, "").unwrap();
        assert_eq!(find_dll(dir.path()), Some(built));
        let versioned = dir.path().join("shell").join(env!("CARGO_PKG_VERSION"));
        std::fs::create_dir_all(&versioned).unwrap();
        std::fs::write(versioned.join("scrimp_shell.dll"), "").unwrap();
        assert_eq!(
            find_dll(dir.path()),
            Some(versioned.join("scrimp_shell.dll")),
            "installed wins"
        );
    }

    #[test]
    fn old_menu_dlls_are_cleared_but_this_versions_kept() {
        let dir = tempfile::tempdir().unwrap();
        let shell = dir.path().join("shell");
        for v in ["0.1.0", env!("CARGO_PKG_VERSION")] {
            std::fs::create_dir_all(shell.join(v)).unwrap();
            std::fs::write(shell.join(v).join("scrimp_shell.dll"), "").unwrap();
        }
        clean_old_shells(dir.path());
        let left: Vec<_> = std::fs::read_dir(&shell)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(left, [env!("CARGO_PKG_VERSION")]);
        clean_old_shells(&dir.path().join("missing"));
    }

    #[test]
    fn the_selection_is_named_plainly() {
        assert_eq!(name_of(&[PathBuf::from(r"D:\Games")]), "Games");
        assert_eq!(name_of(&[PathBuf::from(r"D:\")]), r"D:\");
        assert_eq!(
            name_of(&[PathBuf::from(r"D:\Games"), PathBuf::from(r"D:\Archive")]),
            "2 items"
        );
    }
}
