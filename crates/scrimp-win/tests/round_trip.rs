//! Real compression of real files, checking that contents never change.
//!
//! These run on Windows only, in a temporary folder on the system drive,
//! which on GitHub's runners and on ordinary PCs is NTFS with Windows'
//! compression driver attached. They are the proof behind rule one in
//! AI_AGENT_PROMPT.md: never lose or alter a file's contents.

#![cfg(windows)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use scrimp_core::{Algorithm, CompressionState, UnsupportedReason};
use scrimp_win::{DriveType, Error, compress, decompress, inspect, volume};

const MB: usize = 1024 * 1024;

/// Skips a test that needs something this computer may not have, but never
/// on CI, where GitHub's runners have it and a skip would hide a failure.
fn skip(why: &str) {
    assert!(std::env::var_os("CI").is_none(), "this must run on CI, but {why}");
    eprintln!("skipped: {why}");
}

/// Repetitive text that compresses well. Made up, as all examples are.
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

/// Bytes that won't compress.
fn random(len: usize) -> Vec<u8> {
    let mut x = 0x2545_F491_4F6C_DD1Du64;
    let mut out = Vec::with_capacity(len + 8);
    while out.len() < len {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        out.extend_from_slice(&x.to_le_bytes());
    }
    out.truncate(len);
    out
}

fn file(dir: &Path, name: &str, data: &[u8]) -> PathBuf {
    let p = dir.join(name);
    fs::write(&p, data).unwrap();
    p
}

fn set_read_only(path: &Path, on: bool) {
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_readonly(on);
    fs::set_permissions(path, perms).unwrap();
}

/// Puts a file into a state by name: `None` for uncompressed.
fn put(path: &Path, state: Option<Algorithm>) {
    match state {
        Some(a) => {
            compress(path, a).unwrap();
        }
        None => {
            decompress(path).unwrap();
        }
    }
}

#[test]
fn every_method_compresses_reads_back_and_keeps_the_content() {
    let dir = tempfile::tempdir().unwrap();
    let data = text(2 * MB);
    for algorithm in Algorithm::ALL {
        let p = file(dir.path(), &format!("{}.txt", algorithm.cli_name()), &data);
        let modified = fs::metadata(&p).unwrap().modified().unwrap();

        let info = compress(&p, algorithm).unwrap();

        assert_eq!(info.state, CompressionState::Compressed(algorithm), "{algorithm}");
        assert_eq!(info.logical, data.len() as u64);
        assert!(
            info.on_disk < info.logical / 2,
            "{algorithm} saved too little: {info:?}"
        );
        assert_eq!(inspect(&p).unwrap().state, CompressionState::Compressed(algorithm));
        assert_eq!(fs::read(&p).unwrap(), data, "{algorithm} changed the content");
        assert_eq!(
            fs::metadata(&p).unwrap().modified().unwrap(),
            modified,
            "{algorithm} changed the date"
        );
    }
}

#[test]
fn every_change_between_methods_keeps_the_content_and_lands_exactly() {
    let dir = tempfile::tempdir().unwrap();
    let data = text(MB);
    let states: Vec<Option<Algorithm>> = std::iter::once(None).chain(Algorithm::ALL.map(Some)).collect();
    for &from in &states {
        for &to in &states {
            if from == to {
                continue;
            }
            let p = file(dir.path(), "change.txt", &data);
            put(&p, from);
            put(&p, to);
            let want = to.map_or(CompressionState::Uncompressed, CompressionState::Compressed);
            assert_eq!(inspect(&p).unwrap().state, want, "{from:?} to {to:?}");
            assert_eq!(fs::read(&p).unwrap(), data, "{from:?} to {to:?} changed the content");
            fs::remove_file(&p).unwrap();
        }
    }
}

#[test]
fn asking_for_what_a_file_already_is_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let p = file(dir.path(), "same.txt", &text(MB));
    let first = compress(&p, Algorithm::Lzx).unwrap();
    assert_eq!(compress(&p, Algorithm::Lzx).unwrap(), first);
    decompress(&p).unwrap();
    assert_eq!(decompress(&p).unwrap().state, CompressionState::Uncompressed);
}

#[test]
fn files_that_wont_shrink_are_left_as_they_were() {
    let dir = tempfile::tempdir().unwrap();
    for (name, data) in [
        ("random.bin", random(MB)),
        ("empty.txt", Vec::new()),
        ("tiny.txt", text(100)),
    ] {
        for algorithm in [Algorithm::Lzx, Algorithm::Xpress4k, Algorithm::Lznt1] {
            let p = file(dir.path(), name, &data);
            assert_eq!(
                compress(&p, algorithm),
                Err(Error::NotBeneficial),
                "{name} with {algorithm}"
            );
            assert_eq!(
                inspect(&p).unwrap().state,
                CompressionState::Uncompressed,
                "{name} with {algorithm} wasn't left uncompressed"
            );
            assert_eq!(fs::read(&p).unwrap(), data);
        }
    }
}

#[test]
fn read_only_files_are_compressed_and_stay_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let data = text(MB);
    for algorithm in [Algorithm::Lzx, Algorithm::Lznt1] {
        let p = file(dir.path(), &format!("read-only-{}.txt", algorithm.cli_name()), &data);
        set_read_only(&p, true);

        let info = compress(&p, algorithm).unwrap();
        assert_eq!(info.state, CompressionState::Compressed(algorithm));
        assert!(info.read_only, "{algorithm} lost the read-only mark");

        let info = decompress(&p).unwrap();
        assert_eq!(info.state, CompressionState::Uncompressed);
        assert!(info.read_only, "decompressing {algorithm} lost the read-only mark");
        assert_eq!(fs::read(&p).unwrap(), data);
        set_read_only(&p, false);
    }
}

#[test]
fn a_file_another_app_has_locked_is_reported_as_in_use() {
    let dir = tempfile::tempdir().unwrap();
    let data = text(MB);
    let p = file(dir.path(), "locked.txt", &data);
    let other = fs::OpenOptions::new().read(true).share_mode(0).open(&p).unwrap();
    assert_eq!(compress(&p, Algorithm::Lzx), Err(Error::InUse));
    drop(other);
    assert_eq!(inspect(&p).unwrap().state, CompressionState::Uncompressed);
    assert_eq!(fs::read(&p).unwrap(), data);
}

#[test]
fn a_file_another_app_is_reading_keeps_its_content_whatever_happens() {
    let dir = tempfile::tempdir().unwrap();
    let data = text(MB);
    let p = file(dir.path(), "reading.txt", &data);
    // Another app reading the file and letting others read but not write.
    let other = fs::OpenOptions::new().read(true).share_mode(1).open(&p).unwrap();
    let result = compress(&p, Algorithm::Lzx);
    drop(other);
    assert!(
        matches!(&result, Ok(info) if info.state == CompressionState::Compressed(Algorithm::Lzx))
            || result == Err(Error::InUse),
        "{result:?}"
    );
    assert_eq!(fs::read(&p).unwrap(), data);
}

#[test]
fn missing_files_and_folders_are_told_apart() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(inspect(&dir.path().join("Example.dat")), Err(Error::NotFound));
    assert_eq!(
        compress(&dir.path().join("Example.dat"), Algorithm::Lzx),
        Err(Error::NotFound)
    );
    assert_eq!(inspect(dir.path()), Err(Error::NotAFile));
}

#[test]
fn paths_longer_than_260_characters_work() {
    let dir = tempfile::tempdir().unwrap();
    let mut deep = dir.path().to_path_buf();
    while deep.as_os_str().len() < 300 {
        deep.push("A folder with a long name for testing");
    }
    fs::create_dir_all(&deep).unwrap();
    let data = text(MB);
    let p = file(&deep, "Example.dat", &data);
    assert!(p.as_os_str().len() > 260);

    assert_eq!(
        compress(&p, Algorithm::Lzx).unwrap().state,
        CompressionState::Compressed(Algorithm::Lzx)
    );
    assert_eq!(
        compress(&p, Algorithm::Lznt1).unwrap().state,
        CompressionState::Compressed(Algorithm::Lznt1)
    );
    assert_eq!(decompress(&p).unwrap().state, CompressionState::Uncompressed);
    assert_eq!(fs::read(&p).unwrap(), data);
}

#[test]
fn compressing_one_name_of_a_hardlinked_file_compresses_them_all() {
    let dir = tempfile::tempdir().unwrap();
    let first = file(dir.path(), "first.txt", &text(MB));
    let second = dir.path().join("second.txt");
    fs::hard_link(&first, &second).unwrap();
    compress(&second, Algorithm::Lzx).unwrap();
    assert_eq!(
        inspect(&first).unwrap().state,
        CompressionState::Compressed(Algorithm::Lzx)
    );
}

#[test]
fn links_are_never_followed() {
    let dir = tempfile::tempdir().unwrap();
    let target = file(dir.path(), "target.txt", &text(MB));
    let link = dir.path().join("link.txt");
    // Creating a link needs Developer Mode or an administrator. GitHub's
    // runners are administrators; skip elsewhere rather than fail.
    if std::os::windows::fs::symlink_file(&target, &link).is_err() {
        skip("this account can't create symbolic links");
        return;
    }
    assert_eq!(
        inspect(&link).unwrap().state,
        CompressionState::Unsupported(UnsupportedReason::Link)
    );
    assert!(matches!(compress(&link, Algorithm::Lzx), Err(Error::LeftAlone(_))));
    assert_eq!(
        inspect(&target).unwrap().state,
        CompressionState::Uncompressed,
        "the target was touched"
    );
}

#[test]
fn encrypted_files_are_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let data = text(MB);
    let p = file(dir.path(), "secret.txt", &data);
    let encrypted = Command::new("cipher").arg("/e").arg(&p).output();
    if !encrypted.is_ok_and(|o| o.status.success()) || inspect(&p).unwrap().state == CompressionState::Uncompressed {
        skip("EFS isn't available here");
        return;
    }
    assert_eq!(
        inspect(&p).unwrap().state,
        CompressionState::Unsupported(UnsupportedReason::Encrypted)
    );
    assert!(matches!(compress(&p, Algorithm::Lzx), Err(Error::LeftAlone(_))));
    assert_eq!(fs::read(&p).unwrap(), data);
}

#[test]
fn the_system_drive_supports_every_method() {
    let dir = tempfile::tempdir().unwrap();
    let v = volume(dir.path()).unwrap();
    assert_eq!(v.filesystem, "NTFS", "{v:?}");
    assert_eq!(v.drive_type, DriveType::Fixed, "{v:?}");
    assert!(v.free_bytes > 0, "{v:?}");
    assert!(v.root.is_absolute(), "{v:?}");
    if v.cluster_size <= 4096 {
        assert!(Algorithm::ALL.into_iter().all(|a| v.unsupported(a).is_none()), "{v:?}");
    }
}
