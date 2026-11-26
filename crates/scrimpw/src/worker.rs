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

/// Everything the worker shows the person or asks of Windows on their
/// behalf, so the tests can stand in for both.
pub(crate) trait Screen {
    /// Shows progress until `poll` says the work is finished.
    fn progress(&self, headline: &str, poll: &mut dyn FnMut() -> ProgressView, cancel: &mut dyn FnMut());
    /// Shows a message or question; see [`ui::ask`].
    fn ask(
        &self,
        headline: &str,
        body: &str,
        details: Option<&str>,
        buttons: &[Button<'_>],
        finish: Finish,
    ) -> Option<usize>;
    /// Whether this worker is running as administrator.
    fn elevated(&self) -> bool;
    /// Starts `program` as administrator; `Ok(false)` if the person said no.
    fn run_as_administrator(&self, program: &Path, args: &[&str]) -> Result<bool, String>;
}

/// The real thing: Windows' dialogs and Windows' administrator prompt.
struct Dialogs;

impl Screen for Dialogs {
    fn progress(&self, _headline: &str, poll: &mut dyn FnMut() -> ProgressView, cancel: &mut dyn FnMut()) {
        ui::progress(TITLE, poll, cancel);
    }
    fn ask(
        &self,
        headline: &str,
        body: &str,
        details: Option<&str>,
        buttons: &[Button<'_>],
        finish: Finish,
    ) -> Option<usize> {
        ui::ask(TITLE, headline, body, details, buttons, finish)
    }
    fn elevated(&self) -> bool {
        scrimp_win::is_elevated()
    }
    fn run_as_administrator(&self, program: &Path, args: &[&str]) -> Result<bool, String> {
        scrimp_win::run_as_administrator(program, args).map_err(|e| e.to_string())
    }
}

/// Where the worker keeps what it remembers and hands on.
pub(crate) struct Places {
    /// Exists once the first-use message has been agreed to.
    pub first_use_note: Option<PathBuf>,
    /// Where Try Again as Administrator puts its job.
    pub admin_jobs: Option<PathBuf>,
}

impl Places {
    fn usual() -> Places {
        Places {
            first_use_note: std::env::var_os("LOCALAPPDATA")
                .map(|d| Path::new(&d).join("Scrimp").join("first-use-seen")),
            admin_jobs: admin_jobs_folder(),
        }
    }
}

fn run(job: Result<JobRequest, JobError>, args: &Args) {
    if args.quiet {
        run_on(job, &Quiet, &Places::usual(), true);
    } else {
        run_on(job, &Dialogs, &Places::usual(), false);
    }
}

/// No windows: everything is written to the console, for tests and scripts.
struct Quiet;

impl Screen for Quiet {
    fn progress(&self, _: &str, poll: &mut dyn FnMut() -> ProgressView, _: &mut dyn FnMut()) {
        while !poll().finished {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
    fn ask(&self, headline: &str, body: &str, details: Option<&str>, _: &[Button<'_>], _: Finish) -> Option<usize> {
        println!("{headline}");
        if !body.is_empty() {
            println!("{body}");
        }
        if let Some(d) = details {
            println!("{d}");
        }
        None
    }
    fn elevated(&self) -> bool {
        false
    }
    fn run_as_administrator(&self, _: &Path, _: &[&str]) -> Result<bool, String> {
        Ok(false)
    }
}

/// Runs one job, showing it on `screen`. `quiet` skips every question, so
/// a job given on the console just runs.
pub(crate) fn run_on(job: Result<JobRequest, JobError>, screen: &dyn Screen, places: &Places, quiet: bool) {
    let tell = |headline: &str, body: &str| {
        screen.ask(headline, body, None, &[], Finish::Close);
    };
    let request = match job {
        Ok(r) => r,
        Err(e) => return tell("This job couldn't be run", &e.to_string()),
    };
    let settings = match Settings::default_path().map(|p| Settings::load(&p)) {
        Some(Err(e)) => return tell("Scrimp's settings need fixing", &e.to_string()),
        Some(Ok(s)) => s,
        None => Settings::default(),
    };
    let name = name_of(&request.paths);
    let decompressing = request.target == Target::Uncompressed;
    let doing = if decompressing { "Decompressing" } else { "Compressing" };
    let headline = format!("{doing} {name}");
    let elevated = screen.elevated();

    if !quiet {
        if elevated && !confirm_as_administrator(screen, &request, decompressing) {
            return;
        }
        if !decompressing && !elevated && !first_use_agreed(screen, places.first_use_note.as_deref()) {
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
        let mut poll = || {
            let state = live.lock().unwrap_or_else(PoisonError::into_inner);
            ProgressView {
                headline: headline.clone(),
                detail: state.detail(control.is_cancelled()),
                finished: state.finished,
            }
        };
        let mut cancel = || control.cancel();
        screen.progress(&headline, &mut poll, &mut cancel);
        work.join()
    });

    let Ok(report) = report else {
        return tell(
            "Something went wrong",
            "Scrimp stopped unexpectedly. No file was left half-changed.",
        );
    };
    let state = live.into_inner().unwrap_or_else(PoisonError::into_inner);
    let lines = report.summary(Some(&name));
    let mut body: Vec<String> = lines.iter().skip(1).cloned().collect();
    if control.is_cancelled() {
        body.push("Stopped. Files already done keep their new size.".into());
    }
    let details = (!state.problems.is_empty()).then(|| state.problems.join("\n"));
    let offer = !quiet && !elevated && !state.denied.is_empty();
    let again = Button {
        text: "Try Again as Administrator",
        needs_admin: true,
    };
    let buttons: &[Button<'_>] = if offer { &[again] } else { &[] };
    if screen.ask(&lines[0], &body.join("\n"), details.as_deref(), buttons, Finish::Close) == Some(0) && offer {
        try_again_as_administrator(screen, places.admin_jobs.as_deref(), target, state.denied);
    }
}

/// The first time Scrimp compresses anything for this person, says what it
/// does and how to undo it. Returns whether to go ahead.
fn first_use_agreed(screen: &dyn Screen, note: Option<&Path>) -> bool {
    let Some(note) = note else {
        return true;
    };
    if note.exists() {
        return true;
    }
    let go = screen.ask(
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
        let _ = std::fs::write(note, "");
    }
    go
}

/// Running as administrator, the job came from a folder the everyday user
/// can write to, so the person is shown exactly what will change and asked.
fn confirm_as_administrator(screen: &dyn Screen, request: &JobRequest, decompressing: bool) -> bool {
    let (body, details) = paths_to_confirm(&request.paths, decompressing);
    screen.ask(
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
fn try_again_as_administrator(screen: &dyn Screen, folder: Option<&Path>, target: Target, paths: Vec<PathBuf>) {
    let failed = |why: &str| {
        screen.ask(
            "Scrimp couldn't start as administrator",
            &format!("{why}."),
            None,
            &[],
            Finish::Close,
        );
    };
    let (Some(folder), Ok(exe)) = (folder, std::env::current_exe()) else {
        return failed("Windows didn't say where Scrimp is");
    };
    let job = match (JobRequest { target, paths }).write_to(folder) {
        Ok(job) => job,
        Err(e) => return failed(&e.to_string()),
    };
    let folder_arg = folder.to_string_lossy();
    match screen.run_as_administrator(&exe, &["--jobs-folder", &folder_arg]) {
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

    /// Stands in for the person: answers each question in turn, and
    /// remembers what was asked and what was started as administrator.
    struct Scripted {
        answers: Mutex<Vec<Option<usize>>>,
        asked: Mutex<Vec<(String, String, usize)>>,
        elevated: bool,
        admin: Result<bool, String>,
        started: Mutex<Vec<Vec<String>>>,
    }

    impl Scripted {
        fn new(answers: &[Option<usize>]) -> Self {
            Scripted {
                answers: Mutex::new(answers.iter().rev().copied().collect()),
                asked: Mutex::new(Vec::new()),
                elevated: false,
                admin: Ok(true),
                started: Mutex::new(Vec::new()),
            }
        }
        fn headlines(&self) -> Vec<String> {
            self.asked.lock().unwrap().iter().map(|a| a.0.clone()).collect()
        }
    }

    impl Screen for Scripted {
        fn progress(&self, _: &str, poll: &mut dyn FnMut() -> ProgressView, _: &mut dyn FnMut()) {
            Quiet.progress("", poll, &mut || {});
        }
        fn ask(&self, headline: &str, body: &str, _: Option<&str>, buttons: &[Button<'_>], _: Finish) -> Option<usize> {
            self.asked
                .lock()
                .unwrap()
                .push((headline.to_string(), body.to_string(), buttons.len()));
            self.answers.lock().unwrap().pop().flatten()
        }
        fn elevated(&self) -> bool {
            self.elevated
        }
        fn run_as_administrator(&self, _: &Path, args: &[&str]) -> Result<bool, String> {
            self.started
                .lock()
                .unwrap()
                .push(args.iter().map(|a| a.to_string()).collect());
            self.admin.clone()
        }
    }

    fn a_folder_to_compress(root: &Path) -> (PathBuf, PathBuf) {
        let folder = root.join("Games");
        std::fs::create_dir_all(&folder).unwrap();
        let file = folder.join("notes.txt");
        std::fs::write(&file, "Example.dat moved to D:\\Archive.\n".repeat(20_000)).unwrap();
        (folder, file)
    }

    fn compress(folder: &Path) -> Result<JobRequest, JobError> {
        Ok(JobRequest {
            target: Target::Automatic,
            paths: vec![folder.to_path_buf()],
        })
    }

    fn places(root: &Path) -> Places {
        Places {
            first_use_note: Some(root.join("Scrimp").join("first-use-seen")),
            admin_jobs: Some(root.join("Scrimp").join("admin-jobs")),
        }
    }

    fn is_compressed(file: &Path) -> bool {
        scrimp_win::inspect(file).unwrap().state.is_compressed()
    }

    #[test]
    fn the_first_compress_explains_itself_once_and_cancel_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (folder, file) = a_folder_to_compress(dir.path());
        let places = places(dir.path());

        let screen = Scripted::new(&[None]);
        run_on(compress(&folder), &screen, &places, false);
        assert_eq!(screen.headlines().len(), 1);
        assert!(screen.headlines()[0].starts_with("Scrimp keeps your files exactly as they are"));
        assert!(!is_compressed(&file), "Cancel changes nothing");
        assert!(!places.first_use_note.as_ref().unwrap().exists());

        let screen = Scripted::new(&[Some(0), None]);
        run_on(compress(&folder), &screen, &places, false);
        assert!(is_compressed(&file));
        assert!(places.first_use_note.as_ref().unwrap().exists());
        assert!(screen.headlines()[1].starts_with("Saved "), "{:?}", screen.headlines());

        let screen = Scripted::new(&[None]);
        run_on(
            Ok(JobRequest {
                target: Target::Uncompressed,
                paths: vec![folder.clone()],
            }),
            &screen,
            &places,
            false,
        );
        assert_eq!(
            screen.headlines().len(),
            1,
            "never asked again: {:?}",
            screen.headlines()
        );
        assert!(!is_compressed(&file));
    }

    #[test]
    fn as_administrator_nothing_runs_until_the_paths_are_agreed() {
        let dir = tempfile::tempdir().unwrap();
        let (folder, file) = a_folder_to_compress(dir.path());
        let places = places(dir.path());

        let mut screen = Scripted::new(&[None]);
        screen.elevated = true;
        run_on(compress(&folder), &screen, &places, false);
        let asked = screen.asked.lock().unwrap().clone();
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].0, "Scrimp is running as administrator");
        assert!(asked[0].1.contains(&folder.display().to_string()), "{}", asked[0].1);
        assert!(!is_compressed(&file));

        let mut screen = Scripted::new(&[Some(0), None]);
        screen.elevated = true;
        run_on(compress(&folder), &screen, &places, false);
        assert!(is_compressed(&file));
        let asked = screen.asked.lock().unwrap().clone();
        assert_eq!(asked.len(), 2, "no first-use message as administrator");
        assert_eq!(asked[1].2, 0, "and no Try Again as Administrator");
    }

    #[test]
    fn quiet_runs_without_asking_and_problems_are_told() {
        let dir = tempfile::tempdir().unwrap();
        let (folder, file) = a_folder_to_compress(dir.path());
        let screen = Scripted::new(&[]);
        run_on(compress(&folder), &screen, &places(dir.path()), true);
        assert!(is_compressed(&file));
        assert_eq!(screen.headlines().len(), 1, "just the summary");

        let screen = Scripted::new(&[]);
        run_on(
            Err(JobRequest::from_json("{}").unwrap_err()),
            &screen,
            &places(dir.path()),
            true,
        );
        assert_eq!(screen.headlines(), ["This job couldn't be run"]);
    }

    #[test]
    fn try_again_hands_only_the_refused_files_to_an_administrator_worker() {
        let dir = tempfile::tempdir().unwrap();
        let admin = dir.path().join("admin-jobs");
        let refused = vec![PathBuf::from(r"D:\Games\locked.dat")];

        let screen = Scripted::new(&[]);
        try_again_as_administrator(&screen, Some(&admin), Target::Automatic, refused.clone());
        let started = screen.started.lock().unwrap().clone();
        assert_eq!(
            started,
            [vec!["--jobs-folder".to_string(), admin.display().to_string()]]
        );
        let (_, job) = take_next(&admin).unwrap();
        assert_eq!(job.unwrap().paths, refused);

        let mut screen = Scripted::new(&[]);
        screen.admin = Ok(false);
        try_again_as_administrator(&screen, Some(&admin), Target::Automatic, refused.clone());
        assert!(!has_waiting(&admin), "saying no to Windows leaves nothing behind");
        assert!(screen.headlines().is_empty());

        let mut screen = Scripted::new(&[]);
        screen.admin = Err("Windows refused".into());
        try_again_as_administrator(&screen, Some(&admin), Target::Automatic, refused.clone());
        assert!(!has_waiting(&admin));
        assert_eq!(screen.headlines(), ["Scrimp couldn't start as administrator"]);

        let screen = Scripted::new(&[]);
        try_again_as_administrator(&screen, None, Target::Automatic, refused);
        assert_eq!(screen.headlines(), ["Scrimp couldn't start as administrator"]);
    }
}
