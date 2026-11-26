//! The engine against an in-memory disk, so its rules are tested anywhere.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::*;
use crate::algorithm::{Algorithm, Target};

const GIB: u64 = 1024 * 1024 * 1024;

#[derive(Clone)]
struct Node {
    kind: EntryKind,
    state: CompressionState,
    logical: u64,
    on_disk: u64,
    id: Option<FileId>,
    incompressible: bool,
    locked: bool,
}

/// A disk in memory. Paths look like `D:/Games/Example.dat`; the first part
/// names the drive.
#[derive(Default)]
struct FakeDisk {
    nodes: Mutex<BTreeMap<PathBuf, Node>>,
    volumes: HashMap<String, VolumeFacts>,
    free: Mutex<HashMap<String, u64>>,
    unreadable: HashSet<PathBuf>,
    /// Drives where WOF reports that its driver isn't running.
    no_wof_driver: HashSet<String>,
    compress_calls: AtomicUsize,
    delay: Option<Duration>,
    /// Presses Cancel in the middle of this numbered compression.
    cancel_during: Option<(usize, Arc<Control>)>,
    busy: Mutex<HashMap<String, (usize, usize)>>,
}

fn drive_of(path: &Path) -> String {
    path.components()
        .next()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn volume(root: &str, storage: Storage) -> VolumeFacts {
    VolumeFacts {
        root: PathBuf::from(root),
        storage,
        size: 100 * GIB,
        no_wof: None,
        no_ntfs: None,
    }
}

impl FakeDisk {
    fn new() -> Self {
        let mut disk = FakeDisk::default();
        disk.add_volume(volume("D:", Storage::Ssd), 50 * GIB);
        disk
    }

    fn add_volume(&mut self, facts: VolumeFacts, free: u64) {
        let name = facts.root.to_string_lossy().into_owned();
        self.free.lock().unwrap().insert(name.clone(), free);
        self.volumes.insert(name, facts);
    }

    fn put(&self, path: &str, node: Node) {
        let path = PathBuf::from(path);
        let mut nodes = self.nodes.lock().unwrap();
        let mut parent = path.parent();
        while let Some(p) = parent {
            if p.parent().is_none() || p.as_os_str().is_empty() {
                break;
            }
            nodes.entry(p.to_path_buf()).or_insert(Node {
                kind: EntryKind::Folder,
                ..blank()
            });
            parent = p.parent();
        }
        nodes.insert(path, node);
    }

    fn file(&self, path: &str, logical: u64) {
        self.put(
            path,
            Node {
                logical,
                on_disk: logical,
                ..blank()
            },
        );
    }

    fn with(&self, path: &str, change: impl FnOnce(&mut Node)) {
        let mut nodes = self.nodes.lock().unwrap();
        change(nodes.get_mut(Path::new(path)).unwrap());
    }

    fn state(&self, path: &str) -> CompressionState {
        self.nodes.lock().unwrap()[Path::new(path)].state.clone()
    }

    fn facts(node: &Node) -> FileFacts {
        FileFacts {
            state: node.state.clone(),
            logical: node.logical,
            on_disk: node.on_disk,
            shared_id: node.id,
        }
    }

    /// Records how many files are being worked on at once on each drive.
    fn enter(&self, path: &Path) {
        let mut busy = self.busy.lock().unwrap();
        let entry = busy.entry(drive_of(path)).or_default();
        entry.0 += 1;
        entry.1 = entry.1.max(entry.0);
    }

    fn leave(&self, path: &Path) {
        self.busy.lock().unwrap().entry(drive_of(path)).or_default().0 -= 1;
    }

    fn most_at_once(&self, drive: &str) -> usize {
        self.busy.lock().unwrap().get(drive).map_or(0, |b| b.1)
    }

    /// Applies a change to a file and every other name it has.
    fn update(&self, path: &Path, change: impl Fn(&mut Node)) -> FileFacts {
        let mut nodes = self.nodes.lock().unwrap();
        match nodes[path].id {
            // Only a file with several names needs every node checked.
            Some(id) => nodes.values_mut().filter(|n| n.id == Some(id)).for_each(&change),
            None => change(nodes.get_mut(path).unwrap()),
        }
        Self::facts(&nodes[path])
    }
}

fn blank() -> Node {
    Node {
        kind: EntryKind::File,
        state: CompressionState::Uncompressed,
        logical: 0,
        on_disk: 0,
        id: None,
        incompressible: false,
        locked: false,
    }
}

impl Disk for FakeDisk {
    fn kind(&self, path: &Path) -> Result<EntryKind, FailReason> {
        self.nodes
            .lock()
            .unwrap()
            .get(path)
            .map(|n| n.kind)
            .ok_or(FailReason::Gone)
    }

    fn list(&self, folder: &Path) -> Result<Vec<(PathBuf, EntryKind)>, FailReason> {
        if self.unreadable.contains(folder) {
            return Err(FailReason::AccessDenied);
        }
        Ok(self
            .nodes
            .lock()
            .unwrap()
            .iter()
            .filter(|(p, _)| p.parent() == Some(folder))
            .map(|(p, n)| (p.clone(), n.kind))
            .collect())
    }

    fn volume(&self, path: &Path) -> Result<VolumeFacts, FailReason> {
        self.volumes.get(&drive_of(path)).cloned().ok_or(FailReason::Gone)
    }

    fn free_space(&self, root: &Path) -> Result<u64, FailReason> {
        Ok(self.free.lock().unwrap()[&root.to_string_lossy().into_owned()])
    }

    fn inspect(&self, path: &Path) -> Result<FileFacts, DiskError> {
        let nodes = self.nodes.lock().unwrap();
        nodes
            .get(path)
            .map(Self::facts)
            .ok_or(DiskError::Failed(FailReason::Gone))
    }

    fn compress(&self, path: &Path, algorithm: Algorithm) -> Result<FileFacts, DiskError> {
        let call = self.compress_calls.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some((n, control)) = &self.cancel_during
            && *n == call
        {
            control.cancel();
        }
        self.enter(path);
        if let Some(d) = self.delay {
            std::thread::sleep(d);
        }
        self.leave(path);
        let node = self.nodes.lock().unwrap()[path].clone();
        if node.locked {
            return Err(DiskError::Failed(FailReason::InUse));
        }
        if algorithm.is_wof() && self.no_wof_driver.contains(&drive_of(path)) {
            return Err(DiskError::WofUnavailable);
        }
        if node.incompressible {
            return Err(DiskError::NotBeneficial);
        }
        Ok(self.update(path, |n| {
            n.state = CompressionState::Compressed(algorithm);
            n.on_disk = n.logical / 2;
        }))
    }

    fn decompress(&self, path: &Path) -> Result<FileFacts, DiskError> {
        let grew = {
            let nodes = self.nodes.lock().unwrap();
            nodes[path].logical - nodes[path].on_disk
        };
        *self.free.lock().unwrap().get_mut(&drive_of(path)).unwrap() -= grew;
        Ok(self.update(path, |n| {
            n.state = CompressionState::Uncompressed;
            n.on_disk = n.logical;
        }))
    }
}

/// Runs a job and returns each file's outcome, the events and the report.
fn run_job(
    disk: &FakeDisk,
    paths: &[&str],
    target: Target,
    workers: Workers,
    control: &Control,
    mut also: impl FnMut(&Event),
) -> (BTreeMap<String, Outcome>, Vec<Event>, crate::report::Report) {
    let job = Job {
        paths: paths.iter().map(PathBuf::from).collect(),
        target,
    };
    let mut outcomes = BTreeMap::new();
    let mut events = Vec::new();
    let report = run(&job, &Policy::default(), &workers, disk, control, |e| {
        if let Event::File { path, outcome, .. } = e {
            assert!(
                outcomes
                    .insert(path.to_string_lossy().into_owned(), outcome.clone())
                    .is_none(),
                "{path:?} reported twice"
            );
        }
        events.push(e.clone());
        also(e);
    });
    (outcomes, events, report)
}

fn simple(
    disk: &FakeDisk,
    paths: &[&str],
    target: Target,
) -> (BTreeMap<String, Outcome>, Vec<Event>, crate::report::Report) {
    run_job(disk, paths, target, Workers::default(), &Control::new(), |_| {})
}

fn compressed_with(outcome: &Outcome) -> Option<Algorithm> {
    match outcome {
        Outcome::Compressed { algorithm, .. } => Some(*algorithm),
        _ => None,
    }
}

#[test]
fn a_folder_is_walked_and_each_file_gets_what_automatic_mode_picks() {
    let disk = FakeDisk::new();
    disk.file("D:/Games/game.exe", 100_000);
    disk.file("D:/Games/data/level.dat", 200_000);
    disk.file("D:/Games/readme.txt", 50_000);
    disk.file("D:/Games/intro.mp4", 900_000);
    disk.file("D:/Games/save.db", 40_000);
    disk.file("D:/Games/tiny.txt", 100);

    let (outcomes, _, report) = simple(&disk, &["D:/Games"], Target::Automatic);

    assert_eq!(outcomes.len(), 6);
    assert_eq!(
        compressed_with(&outcomes["D:/Games/game.exe"]),
        Some(Algorithm::Xpress16k)
    );
    assert_eq!(
        compressed_with(&outcomes["D:/Games/data/level.dat"]),
        Some(Algorithm::Xpress8k)
    );
    assert_eq!(compressed_with(&outcomes["D:/Games/readme.txt"]), Some(Algorithm::Lzx));
    assert_eq!(
        outcomes["D:/Games/intro.mp4"],
        Outcome::Skipped(SkipReason::AlreadyCompressedFormat)
    );
    assert_eq!(outcomes["D:/Games/save.db"], Outcome::Skipped(SkipReason::ChangesOften));
    assert_eq!(outcomes["D:/Games/tiny.txt"], Outcome::Skipped(SkipReason::TooSmall));
    assert_eq!(report.sizes(), (1_290_100, 1_115_100));
    assert_eq!(
        report.summary(Some("Games")),
        [
            "Saved 170.9 KB. Games now takes up 1.1 MB instead of 1.2 MB.",
            "1 file is already as small as it can be.",
            "1 file changes often, so Windows would undo the saving.",
            "1 file is too small to be worth compressing.",
        ]
    );
}

#[test]
fn single_files_and_several_selections_work_too() {
    let disk = FakeDisk::new();
    disk.file("D:/Archive/Report-2019.csv", 10_000);
    disk.file("D:/Games/game.exe", 10_000);
    let (outcomes, _, _) = simple(&disk, &["D:/Archive/Report-2019.csv", "D:/Games"], Target::Automatic);
    assert_eq!(outcomes.len(), 2);
    assert!(outcomes.values().all(|o| compressed_with(o).is_some()));
}

#[test]
fn links_are_never_followed() {
    let disk = FakeDisk::new();
    disk.file("D:/Games/game.exe", 10_000);
    disk.put(
        "D:/Games/Shortcut to Archive",
        Node {
            kind: EntryKind::Link,
            ..blank()
        },
    );
    let (outcomes, _, _) = simple(&disk, &["D:/Games", "D:/Games/Shortcut to Archive"], Target::Automatic);
    let link = Outcome::Skipped(SkipReason::CantCompressHere(UnsupportedReason::Link));
    assert_eq!(outcomes["D:/Games/Shortcut to Archive"], link);
    assert_eq!(disk.compress_calls.load(Ordering::SeqCst), 1);
}

#[test]
fn a_file_with_two_names_is_handled_and_counted_once() {
    let disk = FakeDisk::new();
    let id = Some(FileId { volume: 7, file: 48 });
    for name in ["D:/Games/first.dat", "D:/Games/second.dat"] {
        disk.put(
            name,
            Node {
                logical: 10_000,
                on_disk: 10_000,
                id,
                ..blank()
            },
        );
    }
    let (outcomes, _, report) = run_job(
        &disk,
        &["D:/Games"],
        Target::Automatic,
        Workers {
            fixed: Some(1),
            ..Workers::default()
        },
        &Control::new(),
        |_| {},
    );
    assert_eq!(disk.compress_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        compressed_with(&outcomes["D:/Games/first.dat"]),
        Some(Algorithm::Xpress8k)
    );
    assert_eq!(
        outcomes["D:/Games/second.dat"],
        Outcome::Skipped(SkipReason::SameFileAlreadyDone)
    );
    assert_eq!(report.sizes(), (10_000, 5_000), "the data is counted once");
}

#[test]
fn without_a_wof_driver_wof_is_tried_once_and_ntfs_still_works() {
    let mut disk = FakeDisk::new();
    disk.add_volume(volume("W:", Storage::Ssd), 50 * GIB);
    disk.no_wof_driver.insert("W:".into());
    for i in 0..5 {
        disk.file(&format!("W:/Archive/notes-{i}.txt"), 10_000);
    }
    let one = Workers {
        fixed: Some(1),
        ..Workers::default()
    };
    let (outcomes, events, report) = run_job(&disk, &["W:/Archive"], Target::Automatic, one, &Control::new(), |_| {});
    let not_running = Outcome::Skipped(SkipReason::CantCompressHere(UnsupportedReason::WofNotRunning));
    assert!(outcomes.values().all(|o| *o == not_running), "{outcomes:?}");
    assert_eq!(disk.compress_calls.load(Ordering::SeqCst), 1, "WOF was tried again");
    let warned: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, Event::WofUnavailable { .. }))
        .collect();
    assert_eq!(warned, [&Event::WofUnavailable { root: "W:".into() }]);
    assert!(report.summary(None)[1].starts_with("5 files weren't compressed because Windows' compression driver"));

    let (outcomes, _, _) = simple(&disk, &["W:/Archive"], Target::Algorithm(Algorithm::Lznt1));
    assert!(outcomes.values().all(|o| compressed_with(o) == Some(Algorithm::Lznt1)));
}

#[test]
fn a_drive_that_cant_compress_is_never_asked_to() {
    let mut disk = FakeDisk::new();
    let exfat = UnsupportedReason::FileSystem("exFAT".into());
    disk.add_volume(
        VolumeFacts {
            no_wof: Some(exfat.clone()),
            no_ntfs: Some(exfat.clone()),
            ..volume("X:", Storage::Removable)
        },
        GIB,
    );
    disk.file("X:/Photos/notes.txt", 10_000);
    let (outcomes, _, _) = simple(&disk, &["X:/Photos"], Target::Automatic);
    assert_eq!(
        outcomes["X:/Photos/notes.txt"],
        Outcome::Skipped(SkipReason::CantCompressHere(exfat))
    );
    assert_eq!(disk.compress_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn not_beneficial_and_failures_are_reported_as_they_are() {
    let disk = FakeDisk::new();
    disk.file("D:/Archive/random.bin", 10_000);
    disk.with("D:/Archive/random.bin", |n| n.incompressible = true);
    disk.file("D:/Archive/open.bin", 10_000);
    disk.with("D:/Archive/open.bin", |n| n.locked = true);
    let (outcomes, _, report) = simple(&disk, &["D:/Archive"], Target::Algorithm(Algorithm::Lzx));
    assert_eq!(outcomes["D:/Archive/random.bin"], Outcome::NotBeneficial);
    assert_eq!(
        outcomes["D:/Archive/open.bin"],
        Outcome::Failed {
            reason: FailReason::InUse,
            left_as: CompressionState::Uncompressed
        }
    );
    assert_eq!(report.failures(), 1);
}

#[test]
fn decompressing_keeps_room_on_the_drive() {
    let disk = FakeDisk::new();
    let lzx = CompressionState::Compressed(Algorithm::Lzx);
    // 50 GB free, less the 2 GB reserve: room for 48 GB more.
    for (name, logical, on_disk) in [
        ("D:/Archive/small.dat", 10 * GIB, 2 * GIB),
        ("D:/Archive/large.dat", 60 * GIB, 6 * GIB),
    ] {
        disk.put(
            name,
            Node {
                state: lzx.clone(),
                logical,
                on_disk,
                ..blank()
            },
        );
    }
    let (outcomes, _, report) = simple(&disk, &["D:/Archive"], Target::Uncompressed);
    assert!(matches!(outcomes["D:/Archive/small.dat"], Outcome::Decompressed { .. }));
    assert_eq!(
        outcomes["D:/Archive/large.dat"],
        Outcome::Failed {
            reason: FailReason::NotEnoughSpace,
            left_as: lzx.clone()
        }
    );
    assert_eq!(disk.state("D:/Archive/large.dat"), lzx);
    assert_eq!(report.summary(None)[0], "Decompressed 1 file.");
}

#[test]
fn cancelling_lets_the_file_in_progress_finish_and_starts_no_more() {
    let mut disk = FakeDisk::new();
    for i in 0..50 {
        disk.file(&format!("D:/Archive/file-{i:02}.txt"), 10_000);
    }
    let control = Arc::new(Control::new());
    // Cancel is pressed while the third file is being compressed.
    disk.cancel_during = Some((3, Arc::clone(&control)));
    let one = Workers {
        fixed: Some(1),
        ..Workers::default()
    };
    let (outcomes, _, report) = run_job(&disk, &["D:/Archive"], Target::Automatic, one, &control, |_| {});
    let done = outcomes.values().filter(|o| compressed_with(o).is_some()).count();
    assert_eq!(done, 3, "the file in progress finishes, and no other starts");
    assert_eq!(disk.compress_calls.load(Ordering::SeqCst), 3);
    assert!(
        outcomes
            .values()
            .all(|o| compressed_with(o).is_some() || *o == Outcome::Cancelled)
    );
    assert!(report.summary(None).last().unwrap().starts_with("Stopped before"));
}

#[test]
fn a_cancelled_job_starts_nothing() {
    let disk = FakeDisk::new();
    disk.file("D:/Archive/notes.txt", 10_000);
    let control = Control::new();
    control.cancel();
    let (outcomes, _, _) = run_job(
        &disk,
        &["D:/Archive"],
        Target::Automatic,
        Workers::default(),
        &control,
        |_| {},
    );
    assert!(outcomes.is_empty());
    assert_eq!(disk.compress_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn pausing_holds_the_job_until_resumed() {
    let disk = FakeDisk::new();
    for i in 0..5 {
        disk.file(&format!("D:/Archive/file-{i}.txt"), 10_000);
    }
    let control = Control::new();
    control.pause();
    std::thread::scope(|s| {
        let job = s.spawn(|| simple_with(&disk, &control));
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(
            disk.compress_calls.load(Ordering::SeqCst),
            0,
            "work happened while paused"
        );
        control.resume();
        assert_eq!(job.join().unwrap(), 5);
    });
}

fn simple_with(disk: &FakeDisk, control: &Control) -> usize {
    let (outcomes, _, _) = run_job(
        disk,
        &["D:/Archive"],
        Target::Automatic,
        Workers::default(),
        control,
        |_| {},
    );
    outcomes.values().filter(|o| compressed_with(o).is_some()).count()
}

#[test]
fn a_spinning_disk_gets_one_file_at_a_time_while_an_ssd_gets_several() {
    let mut disk = FakeDisk::new();
    disk.delay = Some(Duration::from_millis(15));
    disk.add_volume(volume("H:", Storage::Hdd), 50 * GIB);
    for i in 0..12 {
        disk.file(&format!("H:/Archive/file-{i:02}.txt"), 10_000);
        disk.file(&format!("D:/Archive/file-{i:02}.txt"), 10_000);
    }
    let (outcomes, _, _) = simple(&disk, &["H:/Archive", "D:/Archive"], Target::Automatic);
    assert_eq!(outcomes.len(), 24);
    assert_eq!(disk.most_at_once("H:"), 1);
    let processors = std::thread::available_parallelism().map_or(1, usize::from);
    assert!(disk.most_at_once("D:") <= Workers::default().for_storage(Storage::Ssd, processors));
}

#[test]
fn folders_that_cant_be_opened_and_missing_paths_are_reported() {
    let mut disk = FakeDisk::new();
    disk.file("D:/Archive/Private/notes.txt", 10_000);
    disk.file("D:/Archive/notes.txt", 10_000);
    disk.unreadable.insert(PathBuf::from("D:/Archive/Private"));
    let (outcomes, events, report) = simple(&disk, &["D:/Archive", "D:/Gone"], Target::Automatic);
    assert!(events.contains(&Event::FolderUnreadable {
        path: "D:/Archive/Private".into(),
        reason: FailReason::AccessDenied
    }));
    assert!(matches!(
        outcomes["D:/Gone"],
        Outcome::Failed {
            reason: FailReason::Gone,
            ..
        }
    ));
    assert!(compressed_with(&outcomes["D:/Archive/notes.txt"]).is_some());
    assert!(
        report
            .summary(None)
            .contains(&"1 folder couldn't be opened, so the files in it were left alone.".to_string())
    );
}

#[test]
fn ten_thousand_files_go_through_the_bounded_queues() {
    let disk = FakeDisk::new();
    for i in 0..10_000 {
        disk.file(&format!("D:/Archive/{:02}/file-{i}.txt", i % 37), 10_000);
    }
    let (outcomes, _, report) = simple(&disk, &["D:/Archive"], Target::Automatic);
    assert_eq!(outcomes.len(), 10_000);
    assert_eq!(report.sizes(), (100_000_000, 50_000_000));
}

#[test]
fn worker_counts_follow_the_drive_and_the_processors() {
    let w = Workers::default();
    assert_eq!(w.for_storage(Storage::Hdd, 16), 1);
    assert_eq!(w.for_storage(Storage::Ssd, 16), 4);
    assert_eq!(w.for_storage(Storage::Ssd, 3), 2, "one processor stays free");
    assert_eq!(w.for_storage(Storage::Ssd, 1), 1, "never zero");
    assert_eq!(w.for_storage(Storage::Removable, 16), 1);
    assert_eq!(w.for_storage(Storage::Unknown, 16), 1);
    let fixed = Workers {
        fixed: Some(2),
        ..Workers::default()
    };
    assert_eq!(fixed.for_storage(Storage::Hdd, 16), 2, "--workers wins");
}
