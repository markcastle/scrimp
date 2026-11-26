//! Taking jobs and running them, with a progress dialog and a summary.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Mutex, PoisonError};

use scrimp_core::{
    Control, Event, Job, JobError, JobRequest, Mode, Outcome, Report, Settings, Target, format_count, format_size,
    has_waiting, jobs_folder, take_next, worker_lock_name,
};
use scrimp_win::ui::{self, ProgressView};
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

fn menu(args: &Args) -> ExitCode {
    if args.unregister {
        return match scrimp_win::menu::unregister() {
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
    let dll = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().and_then(find_dll));
    let Some(dll) = dll else {
        say(
            args,
            "Scrimp couldn't be added to Explorer's menu",
            "scrimp_shell.dll isn't next to scrimpw.exe. Reinstalling Scrimp should put it back.",
            None,
        );
        return ExitCode::from(5);
    };
    match scrimp_win::menu::register(&dll) {
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
    say(args, &lines[0], &body.join("\n"), details.as_deref());
}

/// What the progress dialog shows, kept up to date as files finish.
struct Live {
    report: Report,
    files: u64,
    last: Option<String>,
    problems: Vec<String>,
    finished: bool,
}

impl Live {
    fn new(mode: Mode) -> Self {
        Live {
            report: Report::new(mode),
            files: 0,
            last: None,
            problems: Vec::new(),
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
                }
            }
            Event::FolderUnreadable { path, .. } => {
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
    fn the_selection_is_named_plainly() {
        assert_eq!(name_of(&[PathBuf::from(r"D:\Games")]), "Games");
        assert_eq!(name_of(&[PathBuf::from(r"D:\")]), r"D:\");
        assert_eq!(
            name_of(&[PathBuf::from(r"D:\Games"), PathBuf::from(r"D:\Archive")]),
            "2 items"
        );
    }
}
