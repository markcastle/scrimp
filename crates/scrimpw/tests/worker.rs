//! scrimpw.exe run for real, without windows (`--quiet`), on a jobs folder
//! of its own.

#![cfg(windows)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use scrimp_core::{Action, Algorithm, CompressionState, JobRequest, Target, has_waiting, worker_lock_name};
use scrimp_win::{SingleInstance, inspect};

fn text(len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len + 100);
    let mut i = 0u64;
    while out.len() < len {
        out.extend(format!("Row {i}: Example.dat moved to D:\\Archive on day {}.\n", i % 365).bytes());
        i += 1;
    }
    out.truncate(len);
    out
}

fn worker(jobs: &Path) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_scrimpw"))
        .args(["--quiet", "--jobs-folder"])
        .arg(jobs)
        .output()
        .unwrap();
    (out.status.code().unwrap(), String::from_utf8(out.stdout).unwrap())
}

fn folder_of_text(root: &Path, name: &str) -> PathBuf {
    let dir = root.join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("notes.txt"), text(1024 * 1024)).unwrap();
    dir
}

fn submit(jobs: &Path, target: Target, path: &Path) {
    JobRequest {
        action: Action::Change(target),
        paths: vec![path.to_path_buf()],
    }
    .write_to(jobs)
    .unwrap();
}

#[test]
fn every_waiting_job_is_run_then_the_worker_exits() {
    let dir = tempfile::tempdir().unwrap();
    let jobs = dir.path().join("jobs");
    let games = folder_of_text(dir.path(), "Games");
    let archive = folder_of_text(dir.path(), "Archive");
    submit(&jobs, Target::Automatic, &games);
    submit(&jobs, Target::Algorithm(Algorithm::Lznt1), &archive);

    let (code, out) = worker(&jobs);

    assert_eq!(code, 0, "{out}");
    assert_eq!(out.matches("Saved ").count(), 2, "{out}");
    assert!(out.contains("Games now takes up"), "{out}");
    assert!(!has_waiting(&jobs));
    assert_eq!(
        inspect(&games.join("notes.txt")).unwrap().state,
        CompressionState::Compressed(Algorithm::Lzx)
    );
    assert_eq!(
        inspect(&archive.join("notes.txt")).unwrap().state,
        CompressionState::Compressed(Algorithm::Lznt1)
    );

    submit(&jobs, Target::Uncompressed, &games);
    let (code, out) = worker(&jobs);
    assert_eq!(code, 0, "{out}");
    assert!(out.starts_with("Decompressed 1 file."), "{out}");
    assert_eq!(fs::read(games.join("notes.txt")).unwrap(), text(1024 * 1024));
}

#[test]
fn a_second_worker_leaves_the_job_to_the_one_already_running() {
    let dir = tempfile::tempdir().unwrap();
    let jobs = dir.path().join("jobs");
    let games = folder_of_text(dir.path(), "Games");
    submit(&jobs, Target::Automatic, &games);

    // This test stands in for the worker that is already running.
    let running = SingleInstance::try_acquire(&worker_lock_name(&jobs)).unwrap().unwrap();
    let (code, out) = worker(&jobs);
    assert_eq!(code, 0);
    assert!(out.is_empty(), "the second worker did something: {out}");
    assert!(has_waiting(&jobs), "the job must wait for the running worker");
    drop(running);

    let (code, out) = worker(&jobs);
    assert_eq!(code, 0, "{out}");
    assert!(out.starts_with("Saved "), "{out}");
    assert!(!has_waiting(&jobs));
}

#[test]
fn a_broken_job_is_reported_and_doesnt_block_the_next() {
    let dir = tempfile::tempdir().unwrap();
    let jobs = dir.path().join("jobs");
    fs::create_dir_all(&jobs).unwrap();
    fs::write(
        jobs.join("0-broken.json"),
        r#"{"schemaVersion":1,"operation":"compress","method":"auto","paths":["\\\\.\\PhysicalDrive0"]}"#,
    )
    .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    let games = folder_of_text(dir.path(), "Games");
    submit(&jobs, Target::Automatic, &games);

    let (code, out) = worker(&jobs);
    assert_eq!(code, 0, "{out}");
    assert!(out.starts_with("This job couldn't be run"), "{out}");
    assert!(out.contains("isn't a file or folder Scrimp can work on"), "{out}");
    assert!(out.contains("Saved "), "the next job still ran: {out}");
}
