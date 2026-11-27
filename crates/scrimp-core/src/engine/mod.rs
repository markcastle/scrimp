//! Runs a job: walks the selection, decides what to do with each file, and
//! does it, a few files at a time per drive.
//!
//! The engine never calls Windows itself. It works through the [`Disk`]
//! trait, which `scrimp-win` implements for real and the tests implement in
//! memory, so every rule here is tested on any system.
//!
//! How a job runs:
//!
//! - One thread walks the selection, never following links, and hands each
//!   file to its drive's queue. Queues are bounded, so memory stays flat
//!   however many files there are.
//! - Each drive has its own workers: one for a spinning disk, removable or
//!   unknown drive, more for an SSD. Two drives work at the same time.
//! - A file with several names (a hardlink) is handled once.
//! - Pause and cancel take effect between files. A file being compressed
//!   always finishes, because Windows can't stop part-way (docs/windows-api.md).
//! - When a drive reports that Windows' compression driver isn't running,
//!   the engine stops trying WOF there for the rest of the job.
//! - Before decompressing each file it checks there is room, keeping the
//!   reserve in [`crate::space`].

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::{Arc, Condvar, Mutex, PoisonError};

mod analysis;

pub use analysis::{Analysis, Group, Preview, StateGroup, analyse};

use crate::algorithm::{Algorithm, Target};
use crate::outcome::{FailReason, Outcome, Sizes, SkipReason};
use crate::policy::{Plan, Policy};
use crate::report::{Mode, Report};
use crate::space::{check_room, growth};
use crate::state::{CompressionState, UnsupportedReason};

/// One file's identity on its drive, the same for all of a hardlinked
/// file's names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileId {
    pub volume: u64,
    pub file: u64,
}

/// What the engine needs to know about one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileFacts {
    pub state: CompressionState,
    pub logical: u64,
    pub on_disk: u64,
    /// Set when the file has more than one name, so it is handled once.
    pub shared_id: Option<FileId>,
}

/// The kind of thing at a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Folder,
    /// A symbolic link or junction, which is never followed.
    Link,
}

/// What a drive is made of, which decides how many files are worked on at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Storage {
    /// A spinning hard disk.
    Hdd,
    /// A solid-state drive.
    Ssd,
    /// A USB stick or memory card.
    Removable,
    /// Couldn't tell.
    Unknown,
}

/// What the engine needs to know about a drive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeFacts {
    /// Where the drive starts, which also identifies it.
    pub root: PathBuf,
    pub storage: Storage,
    /// The drive's total size in bytes, for the free-space reserve.
    pub size: u64,
    /// Why WOF compression can't be used here, if it can't.
    pub no_wof: Option<UnsupportedReason>,
    /// Why NTFS compression can't be used here, if it can't.
    pub no_ntfs: Option<UnsupportedReason>,
    /// A disk kept in a file, such as a mounted VHDX. Space saved inside it
    /// doesn't make the file itself smaller until it is compacted.
    pub virtual_disk: bool,
}

impl VolumeFacts {
    fn unsupported(&self, algorithm: Algorithm) -> Option<&UnsupportedReason> {
        if algorithm.is_wof() {
            self.no_wof.as_ref()
        } else {
            self.no_ntfs.as_ref()
        }
    }
}

/// Why a change to a file didn't happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiskError {
    /// Windows found the file wouldn't shrink. Not a failure.
    NotBeneficial,
    /// Windows' compression driver isn't running on this drive.
    WofUnavailable,
    /// The file is in a state Scrimp leaves alone.
    LeftAlone(CompressionState),
    /// It failed.
    Failed(FailReason),
}

/// Everything the engine asks of the computer. `scrimp-win` implements it
/// for Windows.
pub trait Disk: Sync {
    /// What is at a path, without following a link.
    fn kind(&self, path: &Path) -> Result<EntryKind, FailReason>;
    /// The things directly inside a folder.
    fn list(&self, folder: &Path) -> Result<Vec<(PathBuf, EntryKind)>, FailReason>;
    /// The drive a path is on.
    fn volume(&self, path: &Path) -> Result<VolumeFacts, FailReason>;
    /// Free bytes on a drive, given its root.
    fn free_space(&self, root: &Path) -> Result<u64, FailReason>;
    fn inspect(&self, path: &Path) -> Result<FileFacts, DiskError>;
    fn compress(&self, path: &Path, algorithm: Algorithm) -> Result<FileFacts, DiskError>;
    /// Compresses a few pieces of the file in memory, changing nothing, to
    /// see whether the whole file would shrink.
    fn sample(&self, path: &Path) -> Result<Sampled, DiskError>;
    fn decompress(&self, path: &Path) -> Result<FileFacts, DiskError>;
}

/// What compressing a few pieces of a file in memory found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sampled {
    /// Bytes read from the file.
    pub read: u64,
    /// What they compressed to.
    pub compressed: u64,
}

impl Sampled {
    /// Whether the file looks worth compressing: its samples shrank by at
    /// least 5%. Less than that rarely survives being stored in clusters.
    pub fn shrinks(&self) -> bool {
        self.read > 0 && self.compressed.saturating_mul(100) <= self.read.saturating_mul(95)
    }
}

/// How many files are worked on at once on each kind of drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Workers {
    pub hdd: usize,
    pub ssd: usize,
    pub removable: usize,
    pub unknown: usize,
    /// When set, every drive uses this many, as `--workers` asks.
    pub fixed: Option<usize>,
}

impl Default for Workers {
    fn default() -> Self {
        Workers {
            hdd: 1,
            ssd: 4,
            removable: 1,
            unknown: 1,
            fixed: None,
        }
    }
}

impl Workers {
    /// Workers for a drive: its kind's limit, never more than the computer's
    /// processors less one (LZX keeps a processor busy), and at least one.
    pub fn for_storage(&self, storage: Storage, processors: usize) -> usize {
        let limit = self.fixed.unwrap_or(match storage {
            Storage::Hdd => self.hdd,
            Storage::Ssd => self.ssd,
            Storage::Removable => self.removable,
            Storage::Unknown => self.unknown,
        });
        limit.min(processors.saturating_sub(1)).max(1)
    }
}

/// Pause and cancel for a running job, usable from another thread.
#[derive(Debug, Default)]
pub struct Control {
    cancelled: AtomicBool,
    paused: Mutex<bool>,
    resumed: Condvar,
}

impl Control {
    pub fn new() -> Self {
        Control::default()
    }

    /// Stops the job after the files being worked on now.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.resume();
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Holds the job after the files being worked on now, until [`resume`](Self::resume).
    pub fn pause(&self) {
        *self.paused.lock().unwrap_or_else(PoisonError::into_inner) = true;
    }

    pub fn resume(&self) {
        *self.paused.lock().unwrap_or_else(PoisonError::into_inner) = false;
        self.resumed.notify_all();
    }

    fn wait_while_paused(&self) {
        let mut paused = self.paused.lock().unwrap_or_else(PoisonError::into_inner);
        while *paused && !self.is_cancelled() {
            paused = self.resumed.wait(paused).unwrap_or_else(PoisonError::into_inner);
        }
    }
}

/// What a job is asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub paths: Vec<PathBuf>,
    pub target: Target,
}

/// Something that happened, reported as the job runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A file was dealt with. `on_disk` is the space it takes now.
    File {
        path: PathBuf,
        outcome: Outcome,
        on_disk: u64,
    },
    /// A folder couldn't be opened, so its files were left alone.
    FolderUnreadable { path: PathBuf, reason: FailReason },
    /// Windows' compression driver isn't running on this drive. Sent once
    /// per drive; WOF isn't tried there again in this job.
    WofUnavailable { root: PathBuf },
    /// This drive is a disk kept in a file, such as a mounted VHDX. Sent
    /// once per drive, before its files.
    VirtualDisk { root: PathBuf },
}

struct Drive {
    facts: VolumeFacts,
    wof_unavailable: AtomicBool,
}

struct Shared<'a, D: Disk> {
    disk: &'a D,
    policy: &'a Policy,
    target: Target,
    control: &'a Control,
    seen: Mutex<HashSet<FileId>>,
}

type Queue = SyncSender<(PathBuf, Arc<Drive>)>;
/// Each drive's queue and its shared record, so every selection on a drive
/// shares one set of workers and one "WOF isn't running" flag.
type Drives = HashMap<PathBuf, (Queue, Arc<Drive>)>;

/// Runs a job to the end, calling `on_event` for each event on this thread,
/// and returns the report.
pub fn run<D: Disk>(
    job: &Job,
    policy: &Policy,
    workers: &Workers,
    disk: &D,
    control: &Control,
    mut on_event: impl FnMut(&Event),
) -> Report {
    let mode = if job.target == Target::Uncompressed {
        Mode::Decompress
    } else {
        Mode::Compress
    };
    let mut report = Report::new(mode);
    let processors = std::thread::available_parallelism().map_or(1, usize::from);
    let shared = Shared {
        disk,
        policy,
        target: job.target,
        control,
        seen: Mutex::new(HashSet::new()),
    };
    // Only the walker and the workers hold senders, so the channel closes,
    // and the loop below ends, exactly when they have all finished.
    let (events, received) = mpsc::channel();

    std::thread::scope(|scope| {
        let shared = &shared;
        scope.spawn(move || {
            let mut queues: Drives = HashMap::new();
            for root in distinct_roots(&job.paths) {
                if shared.control.is_cancelled() {
                    break;
                }
                walk_root(&root, shared, &events, &mut queues, |drive| {
                    let count = workers.for_storage(drive.facts.storage, processors);
                    let (send, receive) = mpsc::sync_channel(count * 2);
                    let receive = Arc::new(Mutex::new(receive));
                    for _ in 0..count {
                        let receive = Arc::clone(&receive);
                        let events = events.clone();
                        scope.spawn(move || work(&receive, shared, &events));
                    }
                    send
                });
            }
            // Dropping the queues here lets the workers finish once they're empty.
        });
        for event in received {
            match &event {
                Event::File { outcome, on_disk, .. } => report.add(outcome, *on_disk),
                Event::FolderUnreadable { .. } => report.add_unreadable_folder(),
                Event::WofUnavailable { .. } => {}
                Event::VirtualDisk { .. } => report.add_virtual_disk(),
            }
            on_event(&event);
        }
    });
    report
}

/// The selected paths without repeats, and without any path that is inside
/// another selected one, so nothing is handled twice. Compared ignoring
/// case, as Windows does.
fn distinct_roots(paths: &[PathBuf]) -> Vec<PathBuf> {
    let key = |p: &Path| -> Vec<String> {
        p.to_string_lossy()
            .to_lowercase()
            .split(['\\', '/'])
            .filter(|part| !part.is_empty())
            .map(str::to_string)
            .collect()
    };
    let keys: Vec<Vec<String>> = paths.iter().map(|p| key(p)).collect();
    let mut kept = Vec::new();
    for (i, path) in paths.iter().enumerate() {
        let inside_another = keys
            .iter()
            .enumerate()
            .any(|(j, other)| j != i && keys[i].starts_with(other) && (keys[i].len() > other.len() || j < i));
        if !inside_another {
            kept.push(path.clone());
        }
    }
    kept
}

/// Walks one selected path, sending each file to its drive's queue.
fn walk_root<D: Disk>(
    root: &Path,
    shared: &Shared<'_, D>,
    events: &Sender<Event>,
    queues: &mut Drives,
    mut start_drive: impl FnMut(&Arc<Drive>) -> Queue,
) {
    let failed = |path: &Path, reason: FailReason| Event::File {
        path: path.to_path_buf(),
        outcome: Outcome::Failed {
            reason,
            left_as: CompressionState::Unknown,
        },
        on_disk: 0,
    };
    let kind = match shared.disk.kind(root) {
        Ok(k) => k,
        Err(reason) => {
            let _ = events.send(failed(root, reason));
            return;
        }
    };
    if kind == EntryKind::Link {
        let _ = events.send(Event::File {
            path: root.to_path_buf(),
            outcome: Outcome::Skipped(SkipReason::CantCompressHere(UnsupportedReason::Link)),
            on_disk: 0,
        });
        return;
    }
    // A selection is on one drive, because the walk never crosses into a
    // mounted folder (those are links), so the drive is looked up once.
    let facts = match shared.disk.volume(root) {
        Ok(f) => f,
        Err(reason) => {
            let _ = events.send(failed(root, reason));
            return;
        }
    };
    let (queue, drive) = queues
        .entry(facts.root.clone())
        .or_insert_with(|| {
            if facts.virtual_disk {
                let _ = events.send(Event::VirtualDisk {
                    root: facts.root.clone(),
                });
            }
            let drive = Arc::new(Drive {
                facts,
                wof_unavailable: AtomicBool::new(false),
            });
            (start_drive(&drive), drive)
        })
        .clone();

    let mut folders = vec![(root.to_path_buf(), kind)];
    while let Some((path, kind)) = folders.pop() {
        if shared.control.is_cancelled() {
            return;
        }
        match kind {
            EntryKind::File => {
                if queue.send((path, Arc::clone(&drive))).is_err() {
                    return;
                }
            }
            EntryKind::Link => {
                let _ = events.send(Event::File {
                    path,
                    outcome: Outcome::Skipped(SkipReason::CantCompressHere(UnsupportedReason::Link)),
                    on_disk: 0,
                });
            }
            EntryKind::Folder => match shared.disk.list(&path) {
                Ok(mut entries) => {
                    // Reversed so the stack visits them in listed order.
                    entries.reverse();
                    folders.extend(entries);
                }
                Err(reason) => {
                    let _ = events.send(Event::FolderUnreadable { path, reason });
                }
            },
        }
    }
}

/// A worker: takes files from its drive's queue until it is empty and closed.
fn work<D: Disk>(queue: &Mutex<Receiver<(PathBuf, Arc<Drive>)>>, shared: &Shared<'_, D>, events: &Sender<Event>) {
    loop {
        let next = queue.lock().unwrap_or_else(PoisonError::into_inner).recv();
        let Ok((path, drive)) = next else { return };
        shared.control.wait_while_paused();
        let (outcome, on_disk) = if shared.control.is_cancelled() {
            (Outcome::Cancelled, 0)
        } else {
            process(&path, &drive, shared, events)
        };
        let _ = events.send(Event::File { path, outcome, on_disk });
    }
}

/// Decides and does what one file needs. Returns what happened and the
/// space the file takes afterwards.
fn process<D: Disk>(path: &Path, drive: &Drive, shared: &Shared<'_, D>, events: &Sender<Event>) -> (Outcome, u64) {
    let disk = shared.disk;
    let facts = match disk.inspect(path) {
        Ok(f) => f,
        Err(DiskError::Failed(reason)) => {
            return (
                Outcome::Failed {
                    reason,
                    left_as: CompressionState::Unknown,
                },
                0,
            );
        }
        Err(_) => return (Outcome::Skipped(SkipReason::StateUnknown), 0),
    };
    if let Some(id) = facts.shared_id {
        let first = shared.seen.lock().unwrap_or_else(PoisonError::into_inner).insert(id);
        if !first {
            return (Outcome::Skipped(SkipReason::SameFileAlreadyDone), facts.on_disk);
        }
    }

    let skip = |reason| (Outcome::Skipped(reason), facts.on_disk);
    let plan = shared.policy.plan(path, &facts.state, facts.logical, shared.target);
    match plan {
        Plan::AlreadyDone => (Outcome::AlreadyDone, facts.on_disk),
        Plan::Skip(reason) => skip(reason),
        Plan::Compress(algorithm) | Plan::Sample(algorithm) => {
            if let Some(reason) = drive.facts.unsupported(algorithm) {
                return skip(SkipReason::CantCompressHere(reason.clone()));
            }
            if algorithm.is_wof() && drive.wof_unavailable.load(Ordering::SeqCst) {
                return skip(SkipReason::CantCompressHere(UnsupportedReason::WofNotRunning));
            }
            // A sample that can't be taken decides nothing: the file is
            // compressed, and Windows still refuses it if it won't shrink.
            if let Plan::Sample(_) = plan
                && disk.sample(path).is_ok_and(|s| !s.shrinks())
            {
                return skip(SkipReason::WontShrink);
            }
            match disk.compress(path, algorithm) {
                Ok(after) => (
                    Outcome::Compressed {
                        algorithm,
                        sizes: Sizes {
                            logical: facts.logical,
                            before: facts.on_disk,
                            after: after.on_disk,
                        },
                    },
                    after.on_disk,
                ),
                Err(DiskError::WofUnavailable) => {
                    if !drive.wof_unavailable.swap(true, Ordering::SeqCst) {
                        let _ = events.send(Event::WofUnavailable {
                            root: drive.facts.root.clone(),
                        });
                    }
                    skip(SkipReason::CantCompressHere(UnsupportedReason::WofNotRunning))
                }
                Err(e) => after_error(path, e, &facts, disk),
            }
        }
        Plan::Decompress => {
            let room = disk
                .free_space(&drive.facts.root)
                .map_err(|_| FailReason::NotEnoughSpace)
                .and_then(|free| {
                    check_room(growth(facts.logical, facts.on_disk), free, drive.facts.size)
                        .map_err(|_| FailReason::NotEnoughSpace)
                });
            if let Err(reason) = room {
                return (
                    Outcome::Failed {
                        reason,
                        left_as: facts.state.clone(),
                    },
                    facts.on_disk,
                );
            }
            match disk.decompress(path) {
                Ok(after) => (
                    Outcome::Decompressed {
                        sizes: Sizes {
                            logical: facts.logical,
                            before: facts.on_disk,
                            after: after.on_disk,
                        },
                    },
                    after.on_disk,
                ),
                Err(e) => after_error(path, e, &facts, disk),
            }
        }
    }
}

/// The outcome of a change that didn't happen. A failure reports the state
/// the file was actually left in, read again, because a failed change of
/// method can leave a file uncompressed.
fn after_error<D: Disk>(path: &Path, error: DiskError, before: &FileFacts, disk: &D) -> (Outcome, u64) {
    match error {
        DiskError::NotBeneficial => (Outcome::NotBeneficial, before.on_disk),
        DiskError::LeftAlone(_) => (Outcome::Skipped(SkipReason::StateUnknown), before.on_disk),
        DiskError::WofUnavailable => (
            Outcome::Skipped(SkipReason::CantCompressHere(UnsupportedReason::WofNotRunning)),
            before.on_disk,
        ),
        DiskError::Failed(reason) => {
            let now = disk.inspect(path).ok();
            let left_as = now.as_ref().map_or(CompressionState::Unknown, |f| f.state.clone());
            let on_disk = now.map_or(before.on_disk, |f| f.on_disk);
            (Outcome::Failed { reason, left_as }, on_disk)
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod root_tests {
    use super::distinct_roots;
    use std::path::PathBuf;

    fn roots(paths: &[&str]) -> Vec<String> {
        let paths: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
        distinct_roots(&paths)
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn nothing_selected_twice_is_walked_twice() {
        assert_eq!(roots(&[r"D:\Games", r"D:\Games\game.exe"]), [r"D:\Games"]);
        assert_eq!(roots(&[r"D:\Games\game.exe", r"d:\games"]), [r"d:\games"]);
        assert_eq!(roots(&[r"D:\Games", r"D:\Games\"]), [r"D:\Games"]);
        assert_eq!(roots(&[r"D:\Games", r"D:\Games2"]), [r"D:\Games", r"D:\Games2"]);
        assert_eq!(roots(&["D:/Games", r"D:\Games\data"]), ["D:/Games"]);
    }
}
