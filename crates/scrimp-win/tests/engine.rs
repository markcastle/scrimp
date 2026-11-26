//! The whole engine on real files: walking, automatic choices, hardlinks,
//! links, and compressing then decompressing a folder, with every file's
//! content checked at the end.

#![cfg(windows)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use common::{MB, file, random, skip, test_drive, text};
use scrimp_core::{
    Algorithm, CompressionState, Control, Event, Job, Outcome, Policy, SkipReason, Target, UnsupportedReason, Workers,
    run,
};
use scrimp_win::{WindowsDisk, inspect};

fn job(paths: &[&Path], target: Target) -> (BTreeMap<String, Outcome>, Vec<Event>, scrimp_core::Report) {
    let job = Job {
        paths: paths.iter().map(|p| p.to_path_buf()).collect(),
        target,
    };
    let mut outcomes = BTreeMap::new();
    let mut events = Vec::new();
    let report = run(
        &job,
        &Policy::default(),
        &Workers::default(),
        &WindowsDisk,
        &Control::new(),
        |e| {
            if let Event::File { path, outcome, .. } = e {
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                outcomes.insert(name, outcome.clone());
            }
            events.push(e.clone());
        },
    );
    (outcomes, events, report)
}

fn algorithm(outcome: &Outcome) -> Option<Algorithm> {
    match outcome {
        Outcome::Compressed { algorithm, .. } => Some(*algorithm),
        _ => None,
    }
}

#[test]
fn a_folder_is_compressed_and_decompressed_with_every_file_intact() {
    let dir = tempfile::tempdir().unwrap();
    let games = dir.path().join("Games");
    fs::create_dir_all(games.join("data")).unwrap();
    let mut originals: Vec<(PathBuf, Vec<u8>)> = Vec::new();
    let mut add = |name: &str, data: Vec<u8>| {
        let p = file(&games, name, &data);
        originals.push((p, data));
    };
    add("notes.txt", text(2 * MB));
    add("game.exe", text(2 * MB));
    add("data/level.dat", text(2 * MB));
    add("Holiday.mp4", text(MB));
    add("save.db", text(MB));
    add("tiny.txt", text(100));
    add("random.dat", random(MB));
    add("first.pak", text(MB));
    fs::hard_link(games.join("first.pak"), games.join("second.pak")).unwrap();
    let linked = std::os::windows::fs::symlink_file(games.join("notes.txt"), games.join("link.txt")).is_ok();
    if !linked {
        skip("this account can't create symbolic links");
    }

    let (outcomes, _, report) = job(&[&games], Target::Automatic);

    assert_eq!(algorithm(&outcomes["notes.txt"]), Some(Algorithm::Lzx));
    assert_eq!(algorithm(&outcomes["game.exe"]), Some(Algorithm::Xpress16k));
    assert_eq!(algorithm(&outcomes["level.dat"]), Some(Algorithm::Xpress8k));
    assert_eq!(
        outcomes["Holiday.mp4"],
        Outcome::Skipped(SkipReason::AlreadyCompressedFormat)
    );
    assert_eq!(outcomes["save.db"], Outcome::Skipped(SkipReason::ChangesOften));
    assert_eq!(outcomes["tiny.txt"], Outcome::Skipped(SkipReason::TooSmall));
    assert_eq!(outcomes["random.dat"], Outcome::NotBeneficial);
    let pak: Vec<_> = [&outcomes["first.pak"], &outcomes["second.pak"]].into_iter().collect();
    assert!(
        pak.contains(&&Outcome::Skipped(SkipReason::SameFileAlreadyDone)) && pak.iter().any(|o| algorithm(o).is_some()),
        "a hardlinked file should be compressed once: {pak:?}"
    );
    if linked {
        assert_eq!(
            outcomes["link.txt"],
            Outcome::Skipped(SkipReason::CantCompressHere(UnsupportedReason::Link))
        );
    }
    assert!(report.saved() > 0);
    assert!(report.summary(Some("Games"))[0].starts_with("Saved "));
    assert_eq!(report.failures(), 0);

    let (outcomes, _, report) = job(&[&games], Target::Uncompressed);
    assert_eq!(report.failures(), 0, "{outcomes:?}");
    for (path, data) in &originals {
        assert_eq!(inspect(path).unwrap().state, CompressionState::Uncompressed, "{path:?}");
        assert_eq!(&fs::read(path).unwrap(), data, "{path:?} changed");
    }
}

#[test]
fn a_drive_without_wof_is_tried_once_then_left_for_ntfs_compression() {
    let Some(dir) = test_drive("SCRIMP_TEST_NO_WOF") else {
        return;
    };
    for i in 0..6 {
        file(dir.path(), &format!("notes-{i}.txt"), &text(MB));
    }
    let (outcomes, events, _) = job(&[dir.path()], Target::Automatic);
    let not_running = Outcome::Skipped(SkipReason::CantCompressHere(UnsupportedReason::WofNotRunning));
    assert!(outcomes.values().all(|o| *o == not_running), "{outcomes:?}");
    let warnings = events
        .iter()
        .filter(|e| matches!(e, Event::WofUnavailable { .. }))
        .count();
    assert_eq!(warnings, 1);

    let (outcomes, _, _) = job(&[dir.path()], Target::Algorithm(Algorithm::Lznt1));
    assert!(
        outcomes.values().all(|o| algorithm(o) == Some(Algorithm::Lznt1)),
        "{outcomes:?}"
    );
}

#[test]
fn an_exfat_drive_is_never_asked_to_compress() {
    let Some(dir) = test_drive("SCRIMP_TEST_EXFAT") else {
        return;
    };
    file(dir.path(), "notes.txt", &text(MB));
    let (outcomes, events, _) = job(&[dir.path()], Target::Automatic);
    assert_eq!(
        outcomes["notes.txt"],
        Outcome::Skipped(SkipReason::CantCompressHere(UnsupportedReason::FileSystem(
            "exFAT".into()
        )))
    );
    assert!(
        !events.iter().any(|e| matches!(e, Event::WofUnavailable { .. })),
        "exFAT isn't 'WOF not running'"
    );
}
