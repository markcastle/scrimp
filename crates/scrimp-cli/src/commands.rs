//! Running each command on Windows: settings, the engine, progress, output
//! and the exit code.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use scrimp_core::{
    Algorithm, CompressionState, Control, Disk, DiskError, EntryKind, Event, FailReason, FileFacts, Job, Outcome,
    Policy, Settings, SkipReason, Target, VolumeFacts, Workers, format_count, format_size,
};
use scrimp_win::WindowsDisk;
use serde_json::json;

use crate::cli::{Change, Cli, Command, Look, Update, exit};
use crate::{json, words};

pub fn run(cli: Cli) -> i32 {
    let settings = match load_settings(cli.settings.as_deref()) {
        Ok(s) => s,
        Err(message) => {
            eprintln!("{message}");
            return exit::FATAL;
        }
    };
    match cli.command {
        Command::Compress(c) => change(&settings, &c.change, c.algorithm.unwrap_or(settings.default_target)),
        Command::Decompress(c) => change(&settings, &c, Target::Uncompressed),
        Command::Inspect(look) => inspect(&look),
        Command::Analyse(look) => analyse(&settings, &look),
        Command::Update(u) => update(&u),
    }
}

/// `update`: the newest release, checked, put in place the way this copy
/// was installed.
fn update(args: &Update) -> i32 {
    let Ok(exe) = std::env::current_exe() else {
        eprintln!("Couldn't tell where Scrimp is installed.");
        return exit::FATAL;
    };
    let web = scrimp_update::GitHub::from_env();
    match scrimp_update::run(&web, env!("CARGO_PKG_VERSION"), &exe, args.check, &mut |line| {
        println!("{line}")
    }) {
        Ok(()) => exit::OK,
        Err(e) => {
            eprintln!("{e}.");
            exit::SOME_FAILED
        }
    }
}

fn load_settings(path: Option<&Path>) -> Result<Settings, String> {
    match path.map(Path::to_path_buf).or_else(Settings::default_path) {
        Some(p) => Settings::load(&p).map_err(|e| e.to_string()),
        None => Ok(Settings::default()),
    }
}

/// Cancels the job when Ctrl+C is pressed, after the files in progress.
fn cancel_on_ctrl_c() -> Arc<Control> {
    let control = Arc::new(Control::new());
    let handler = Arc::clone(&control);
    let _ = ctrlc::set_handler(move || handler.cancel());
    control
}

/// The name a summary uses for what was selected: a single file or
/// folder's name, otherwise none.
fn name_of(paths: &[PathBuf]) -> Option<String> {
    match paths {
        [one] => Some(
            one.file_name()
                .map_or_else(|| one.display().to_string(), |n| n.to_string_lossy().into_owned()),
        ),
        _ => None,
    }
}

/// A one-line progress count on the console, rewritten in place, only
/// when a person is watching.
struct Progress {
    on: bool,
    verb: &'static str,
    files: u64,
    shown: Option<Instant>,
}

impl Progress {
    fn new(on: bool, verb: &'static str) -> Self {
        Progress {
            on,
            verb,
            files: 0,
            shown: None,
        }
    }

    fn tick(&mut self) {
        self.files += 1;
        if self.on && self.shown.is_none_or(|t| t.elapsed() > Duration::from_millis(200)) {
            eprint!("\r{}… {} files", self.verb, format_count(self.files));
            let _ = std::io::stderr().flush();
            self.shown = Some(Instant::now());
        }
    }

    fn clear(&mut self) {
        if self.on && self.shown.take().is_some() {
            eprint!("\r{:60}\r", "");
        }
    }
}

/// What a job's events add up to, for the exit code and for dry runs.
#[derive(Default)]
struct Tally {
    files: u64,
    failed: u64,
    cant: u64,
    would_change: u64,
    would_bytes: u64,
    duplicates: HashSet<PathBuf>,
}

impl Tally {
    fn add(&mut self, event: &Event) {
        match event {
            Event::File { path, outcome, .. } => {
                self.files += 1;
                match outcome {
                    Outcome::Failed {
                        reason: FailReason::Unsupported,
                        ..
                    }
                    | Outcome::Skipped(SkipReason::CantCompressHere(_)) => self.cant += 1,
                    Outcome::Failed { .. } => self.failed += 1,
                    Outcome::Compressed { sizes, .. } => {
                        self.would_change += 1;
                        self.would_bytes += sizes.logical;
                    }
                    Outcome::Decompressed { sizes } => {
                        self.would_change += 1;
                        self.would_bytes += sizes.logical.saturating_sub(sizes.before);
                    }
                    Outcome::Skipped(SkipReason::SameFileAlreadyDone) => {
                        self.duplicates.insert(path.clone());
                    }
                    _ => {}
                }
            }
            Event::FolderUnreadable { .. } => self.failed += 1,
            Event::WofUnavailable { .. } => {}
        }
    }

    fn exit_code(&self, control: &Control) -> i32 {
        if control.is_cancelled() {
            exit::CANCELLED
        } else if self.failed > 0 {
            exit::SOME_FAILED
        } else if self.files > 0 && self.cant == self.files {
            exit::NOTHING_POSSIBLE
        } else {
            exit::OK
        }
    }
}

/// A disk that reads but never changes anything: for `--dry-run` and
/// `analyse`. Asked to compress or decompress, it reports the file as it
/// is, so the engine's decisions show without any file changing.
struct Preview<'a, D: Disk> {
    inner: &'a D,
    seen: Mutex<HashMap<PathBuf, FileFacts>>,
}

impl<'a, D: Disk> Preview<'a, D> {
    fn new(inner: &'a D) -> Self {
        Preview {
            inner,
            seen: Mutex::new(HashMap::new()),
        }
    }
}

impl<D: Disk> Disk for Preview<'_, D> {
    fn kind(&self, path: &Path) -> Result<EntryKind, FailReason> {
        self.inner.kind(path)
    }
    fn list(&self, folder: &Path) -> Result<Vec<(PathBuf, EntryKind)>, FailReason> {
        self.inner.list(folder)
    }
    fn volume(&self, path: &Path) -> Result<VolumeFacts, FailReason> {
        self.inner.volume(path)
    }
    fn free_space(&self, root: &Path) -> Result<u64, FailReason> {
        self.inner.free_space(root)
    }
    fn inspect(&self, path: &Path) -> Result<FileFacts, DiskError> {
        let facts = self.inner.inspect(path)?;
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(path.to_path_buf(), facts.clone());
        Ok(facts)
    }
    fn compress(&self, path: &Path, _: Algorithm) -> Result<FileFacts, DiskError> {
        self.inner.inspect(path)
    }
    fn decompress(&self, path: &Path) -> Result<FileFacts, DiskError> {
        self.inner.inspect(path)
    }
}

/// `compress` and `decompress`, for real or as a dry run.
fn change(settings: &Settings, args: &Change, target: Target) -> i32 {
    let control = cancel_on_ctrl_c();
    let mut workers: Workers = settings.workers;
    workers.fixed = args.workers.map(usize::from);
    let policy = settings.policy();
    let job = Job {
        paths: args.paths.clone(),
        target,
    };
    let decompressing = target == Target::Uncompressed;
    let verb = match (args.dry_run, decompressing) {
        (true, _) => "Checking",
        (false, true) => "Decompressing",
        (false, false) => "Compressing",
    };
    let mut progress = Progress::new(!args.json && std::io::stderr().is_terminal(), verb);
    let mut tally = Tally::default();

    let mut on_event = |e: &Event| {
        tally.add(e);
        if args.json {
            let line = json::event(e);
            println!("{}", if args.dry_run { as_preview(line) } else { line });
            return;
        }
        let message = match e {
            Event::File { path, outcome, .. } => words::attention(path, outcome),
            Event::FolderUnreadable { path, .. } => Some(format!(
                "Couldn't open {}, so the files in it were left alone.",
                path.display()
            )),
            Event::WofUnavailable { root } => Some(format!(
                "Windows' compression driver isn't running on {}. Use --algorithm ntfs to compress files there.",
                root.display()
            )),
        };
        if let Some(m) = message {
            progress.clear();
            eprintln!("{m}");
        }
        if matches!(e, Event::File { .. }) {
            progress.tick();
        }
    };
    let report = if args.dry_run {
        scrimp_core::run(
            &job,
            &policy,
            &workers,
            &Preview::new(&WindowsDisk),
            &control,
            &mut on_event,
        )
    } else {
        scrimp_core::run(&job, &policy, &workers, &WindowsDisk, &control, &mut on_event)
    };
    progress.clear();

    let name = name_of(&args.paths);
    if args.json {
        let summary = json::summary(&report, name.as_deref(), args.dry_run);
        println!(
            "{}",
            if args.dry_run {
                preview_summary(summary, &tally)
            } else {
                summary
            }
        );
    } else {
        let lines = report.summary(name.as_deref());
        if args.dry_run {
            println!("{}", dry_run_headline(&tally, decompressing));
            lines.iter().skip(1).for_each(|l| println!("{l}"));
        } else {
            lines.iter().for_each(|l| println!("{l}"));
        }
        if control.is_cancelled() {
            println!("Stopped. Files already done keep their new size.");
        }
    }
    tally.exit_code(&control)
}

fn dry_run_headline(tally: &Tally, decompressing: bool) -> String {
    let files = |n| {
        if n == 1 {
            "1 file".to_string()
        } else {
            format!("{} files", format_count(n))
        }
    };
    match (tally.would_change, decompressing) {
        (0, false) => "Nothing needs compressing.".into(),
        (0, true) => "Nothing needs decompressing.".into(),
        (n, false) => format!(
            "Scrimp would compress {}, {} in all. How much that saves depends on what's in them.",
            files(n),
            format_size(tally.would_bytes)
        ),
        (n, true) => format!(
            "Scrimp would decompress {}, which would then take {} more space.",
            files(n),
            format_size(tally.would_bytes)
        ),
    }
}

/// In a dry run nothing changes, so "compressed" is "would compress" and
/// there is no size afterwards.
fn as_preview(mut line: serde_json::Value) -> serde_json::Value {
    let renamed = match line["outcome"].as_str() {
        Some("compressed") => "wouldCompress",
        Some("decompressed") => "wouldDecompress",
        _ => return line,
    };
    line["outcome"] = json!(renamed);
    if let Some(map) = line.as_object_mut() {
        map.remove("afterBytes");
    }
    line
}

fn preview_summary(mut summary: serde_json::Value, tally: &Tally) -> serde_json::Value {
    if let Some(map) = summary.as_object_mut() {
        map.remove("afterBytes");
        map.remove("savedBytes");
        map.insert("wouldChangeFiles".into(), json!(tally.would_change));
        map.insert("wouldChangeBytes".into(), json!(tally.would_bytes));
    }
    summary
}

/// `inspect`: one line per file.
fn inspect(look: &Look) -> i32 {
    let mut code = exit::OK;
    for path in &look.paths {
        match scrimp_win::inspect(path) {
            Ok(info) => {
                if look.json {
                    println!("{}", json::inspected(path, &info.state, info.logical, info.on_disk));
                } else {
                    println!("{}", words::describe(path, &info.state, info.logical, info.on_disk));
                }
            }
            Err(e) => {
                code = exit::SOME_FAILED;
                let message = match e {
                    scrimp_win::Error::NotAFile => {
                        "is a folder. Use scrimp analyse to look inside folders.".to_string()
                    }
                    other => format!("{other}."),
                };
                if look.json {
                    println!("{}", json::unreadable(path, &message));
                } else {
                    eprintln!("{}: {message}", path.display());
                }
            }
        }
    }
    code
}

/// How many files are in one state, and their sizes.
#[derive(Default, Clone, Copy)]
struct Group {
    files: u64,
    logical: u64,
    on_disk: u64,
}

impl Group {
    fn add(&mut self, facts: &FileFacts) {
        self.files += 1;
        self.logical += facts.logical;
        self.on_disk += facts.on_disk;
    }
}

/// `analyse`: what the selection takes now and what compressing would do,
/// without changing anything.
fn analyse(settings: &Settings, look: &Look) -> i32 {
    let control = cancel_on_ctrl_c();
    let job = Job {
        paths: look.paths.clone(),
        target: Target::Automatic,
    };
    let preview = Preview::new(&WindowsDisk);
    let policy: Policy = settings.policy();
    let mut tally = Tally::default();
    let mut progress = Progress::new(!look.json && std::io::stderr().is_terminal(), "Looking");
    let report = scrimp_core::run(&job, &policy, &settings.workers, &preview, &control, |e| {
        tally.add(e);
        if matches!(e, Event::File { .. }) {
            progress.tick();
        }
    });
    progress.clear();

    // Each file once, by state, in a fixed order.
    let seen = preview.seen.into_inner().unwrap_or_else(PoisonError::into_inner);
    let mut total = Group::default();
    let mut by_state: BTreeMap<usize, (String, &'static str, Group)> = BTreeMap::new();
    for (path, facts) in &seen {
        if tally.duplicates.contains(path) {
            continue;
        }
        total.add(facts);
        let (order, label, key) = match &facts.state {
            CompressionState::Uncompressed => (0, "Not compressed".to_string(), "uncompressed"),
            CompressionState::Compressed(a) => (
                1 + Algorithm::ALL.iter().position(|x| x == a).unwrap_or(0),
                format!("Compressed with {}", a.technical_name()),
                a.cli_name(),
            ),
            _ => (9, "Left alone by Scrimp".to_string(), "leftAlone"),
        };
        by_state
            .entry(order)
            .or_insert((label, key, Group::default()))
            .2
            .add(facts);
    }
    let name = name_of(&look.paths).unwrap_or_else(|| "The selection".into());
    let lines: Vec<String> = report.summary(None).into_iter().skip(1).collect();

    if look.json {
        let states: serde_json::Map<String, serde_json::Value> = by_state
            .values()
            .map(|(_, key, g)| {
                (
                    (*key).to_string(),
                    json!({ "files": g.files, "logicalBytes": g.logical, "onDiskBytes": g.on_disk }),
                )
            })
            .collect();
        let mut v = json!({
            "schemaVersion": json::SCHEMA_VERSION, "type": "analysis",
            "files": total.files, "logicalBytes": total.logical, "onDiskBytes": total.on_disk,
            "byState": states,
            "wouldCompress": { "files": tally.would_change, "logicalBytes": tally.would_bytes },
            "lines": lines,
        });
        if control.is_cancelled() {
            v["cancelled"] = json!(true);
        }
        println!("{v}");
    } else {
        let files = |n: u64| {
            if n == 1 {
                "1 file".to_string()
            } else {
                format!("{} files", format_count(n))
            }
        };
        println!(
            "{name}: {}, {}, taking {} on disk.",
            files(total.files),
            format_size(total.logical),
            format_size(total.on_disk)
        );
        for (label, _, g) in by_state.values() {
            if g.on_disk == g.logical {
                println!("  {label}: {}, {}.", files(g.files), format_size(g.logical));
            } else {
                println!(
                    "  {label}: {}, {}, taking {} on disk.",
                    files(g.files),
                    format_size(g.logical),
                    format_size(g.on_disk)
                );
            }
        }
        println!("{}", dry_run_headline(&tally, false));
        lines.iter().for_each(|l| println!("{l}"));
    }
    tally.exit_code(&control)
}
