//! `scrimp.exe` run for real on Windows: each command, its output and its
//! exit code. Every run uses a settings file of its own, so a developer's
//! own settings never change the results.

#![cfg(windows)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;

const MB: usize = 1024 * 1024;

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

struct Run {
    code: i32,
    out: String,
    err: String,
}

fn scrimp(settings: &Path, args: &[&str]) -> Run {
    let Output { status, stdout, stderr } = Command::new(env!("CARGO_BIN_EXE_scrimp"))
        .arg("--settings")
        .arg(settings)
        .args(args)
        .output()
        .unwrap();
    Run {
        code: status.code().unwrap(),
        out: String::from_utf8(stdout).unwrap(),
        err: String::from_utf8(stderr).unwrap(),
    }
}

/// A folder of files to work on, and a settings file path that doesn't exist.
fn setup() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let games = dir.path().join("Games");
    fs::create_dir_all(&games).unwrap();
    fs::write(games.join("notes.txt"), text(2 * MB)).unwrap();
    fs::write(games.join("game.exe"), text(2 * MB)).unwrap();
    fs::write(games.join("Holiday.mp4"), text(MB)).unwrap();
    let settings = dir.path().join("settings.json");
    (dir, games, settings)
}

fn lines(out: &str) -> Vec<Value> {
    out.lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

fn state(settings: &Path, file: &Path) -> String {
    let run = scrimp(settings, &["inspect", "--json", &file.to_string_lossy()]);
    lines(&run.out)[0]["state"].as_str().unwrap().to_string()
}

#[test]
fn a_folder_is_compressed_and_decompressed() {
    let (_dir, games, settings) = setup();
    let g = games.to_string_lossy().into_owned();

    let run = scrimp(&settings, &["compress", &g]);
    assert_eq!(run.code, 0, "{}{}", run.out, run.err);
    assert!(run.out.starts_with("Saved "), "{}", run.out);
    assert!(run.out.contains("Games now takes up"), "{}", run.out);
    assert!(
        run.out.contains("1 file is already as small as it can be."),
        "{}",
        run.out
    );
    assert_eq!(state(&settings, &games.join("notes.txt")), "lzx");
    assert_eq!(state(&settings, &games.join("game.exe")), "xpress16k");
    assert_eq!(state(&settings, &games.join("Holiday.mp4")), "uncompressed");

    let run = scrimp(&settings, &["decompress", &g]);
    assert_eq!(run.code, 0, "{}{}", run.out, run.err);
    assert!(run.out.starts_with("Decompressed 2 files."), "{}", run.out);
    assert_eq!(fs::read(games.join("notes.txt")).unwrap(), text(2 * MB));
    assert_eq!(state(&settings, &games.join("notes.txt")), "uncompressed");
}

#[test]
fn a_chosen_method_is_used() {
    let (_dir, games, settings) = setup();
    let file = games.join("notes.txt");
    let run = scrimp(&settings, &["compress", "-a", "ntfs", &file.to_string_lossy()]);
    assert_eq!(run.code, 0, "{}{}", run.out, run.err);
    assert_eq!(state(&settings, &file), "ntfs");
}

#[test]
fn a_dry_run_changes_nothing() {
    let (_dir, games, settings) = setup();
    let run = scrimp(&settings, &["compress", "--dry-run", &games.to_string_lossy()]);
    assert_eq!(run.code, 0, "{}{}", run.out, run.err);
    assert!(
        run.out.starts_with("Scrimp would compress 2 files, 4.0 MB in all."),
        "{}",
        run.out
    );
    assert_eq!(state(&settings, &games.join("notes.txt")), "uncompressed");

    let run = scrimp(
        &settings,
        &["compress", "--dry-run", "--json", &games.to_string_lossy()],
    );
    let out = lines(&run.out);
    assert!(out.iter().any(|l| l["outcome"] == "wouldCompress"), "{}", run.out);
    let summary = out.last().unwrap();
    assert_eq!(summary["type"], "summary");
    assert_eq!(summary["wouldChangeFiles"], 2);
    assert!(summary.get("savedBytes").is_none());
    assert_eq!(state(&settings, &games.join("game.exe")), "uncompressed");
}

#[test]
fn json_output_is_one_object_per_line() {
    let (_dir, games, settings) = setup();
    let run = scrimp(&settings, &["compress", "--json", &games.to_string_lossy()]);
    assert_eq!(run.code, 0, "{}{}", run.out, run.err);
    let out = lines(&run.out);
    assert_eq!(out.len(), 4, "three files and a summary: {}", run.out);
    assert!(out.iter().all(|l| l["schemaVersion"] == 1));
    let notes = out
        .iter()
        .find(|l| l["path"].as_str().is_some_and(|p| p.ends_with("notes.txt")))
        .unwrap();
    assert_eq!(
        (notes["outcome"].as_str(), notes["method"].as_str()),
        (Some("compressed"), Some("lzx"))
    );
    assert!(notes["afterBytes"].as_u64().unwrap() < notes["beforeBytes"].as_u64().unwrap());
    assert_eq!(out[3]["type"], "summary");
    assert!(out[3]["savedBytes"].as_i64().unwrap() > 0);
    assert!(run.err.is_empty(), "nothing else is written: {}", run.err);
}

#[test]
fn inspect_and_analyse_describe_without_changing() {
    let (_dir, games, settings) = setup();
    let run = scrimp(&settings, &["inspect", &games.join("notes.txt").to_string_lossy()]);
    assert_eq!(run.code, 0);
    assert!(run.out.contains("notes.txt: not compressed. 2.0 MB."), "{}", run.out);

    let run = scrimp(&settings, &["inspect", &games.to_string_lossy()]);
    assert_eq!(run.code, 1);
    assert!(run.err.contains("is a folder. Use scrimp analyse"), "{}", run.err);

    let run = scrimp(&settings, &["analyse", &games.to_string_lossy()]);
    assert_eq!(run.code, 0, "{}{}", run.out, run.err);
    assert!(
        run.out.starts_with("Games: 3 files, 5.0 MB, taking 5.0 MB on disk."),
        "{}",
        run.out
    );
    assert!(run.out.contains("  Not compressed: 3 files, 5.0 MB."), "{}", run.out);
    assert!(
        run.out.contains("Scrimp would compress 2 files, 4.0 MB in all."),
        "{}",
        run.out
    );

    let run = scrimp(&settings, &["analyse", "--json", &games.to_string_lossy()]);
    let a = &lines(&run.out)[0];
    assert_eq!((a["type"].as_str(), a["files"].as_u64()), (Some("analysis"), Some(3)));
    assert_eq!(a["byState"]["uncompressed"]["files"], 3);
    assert_eq!(a["wouldCompress"]["files"], 2);
    assert_eq!(state(&settings, &games.join("notes.txt")), "uncompressed");
}

#[test]
fn problems_have_their_own_exit_codes() {
    let (dir, games, settings) = setup();
    let missing = games.join("Example.dat");
    let run = scrimp(&settings, &["compress", &missing.to_string_lossy()]);
    assert_eq!(run.code, 1, "{}{}", run.out, run.err);
    assert!(
        run.err.contains("Couldn't change") && run.err.contains("can't be found"),
        "{}",
        run.err
    );

    let broken = dir.path().join("broken.json");
    fs::write(&broken, r#"{ "defaultMethod": "zip" }"#).unwrap();
    let run = scrimp(&broken, &["compress", &games.to_string_lossy()]);
    assert_eq!(run.code, 5);
    assert!(run.err.contains("\"defaultMethod\" is \"zip\""), "{}", run.err);
    assert_eq!(
        state(&settings, &games.join("notes.txt")),
        "uncompressed",
        "nothing ran"
    );

    let run = scrimp(&settings, &["compress", "-a", "zip", &games.to_string_lossy()]);
    assert_eq!(run.code, 2);
}

#[test]
fn settings_change_what_automatic_mode_does() {
    let (dir, games, _) = setup();
    let settings = dir.path().join("custom.json");
    fs::write(
        &settings,
        r#"{ "defaultMethod": "xpress4k", "extensions": { "mp4": "lzx" } }"#,
    )
    .unwrap();
    let run = scrimp(&settings, &["compress", &games.to_string_lossy()]);
    assert_eq!(run.code, 0, "{}{}", run.out, run.err);
    // The default method is a chosen method, so it applies to every file,
    // the video included.
    assert_eq!(state(&settings, &games.join("notes.txt")), "xpress4k");
    assert_eq!(state(&settings, &games.join("Holiday.mp4")), "xpress4k");
}

#[test]
fn a_drive_that_cant_compress_exits_with_nothing_possible() {
    let Some(root) = std::env::var_os("SCRIMP_TEST_EXFAT") else {
        assert!(std::env::var_os("CI").is_none(), "SCRIMP_TEST_EXFAT must be set on CI");
        return;
    };
    let dir = tempfile::tempdir_in(root).unwrap();
    fs::write(dir.path().join("notes.txt"), text(MB)).unwrap();
    let settings = dir.path().join("settings.json");
    let run = scrimp(&settings, &["compress", &dir.path().to_string_lossy()]);
    assert_eq!(run.code, 3, "{}{}", run.out, run.err);
    assert!(
        run.out.contains("on a drive that can't save space this way"),
        "{}",
        run.out
    );
}
