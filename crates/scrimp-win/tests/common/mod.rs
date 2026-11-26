//! Helpers shared by scrimp-win's Windows tests.

#![allow(dead_code)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};

pub const MB: usize = 1024 * 1024;

/// Skips a test that needs something this computer may not have, but never
/// on CI, where GitHub's runners have it and a skip would hide a failure.
pub fn skip(why: &str) {
    assert!(std::env::var_os("CI").is_none(), "this must run on CI, but {why}");
    eprintln!("skipped: {why}");
}

/// Repetitive text that compresses well. Made up, as all examples are.
pub fn text(len: usize) -> Vec<u8> {
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
pub fn random(len: usize) -> Vec<u8> {
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

pub fn file(dir: &Path, name: &str, data: &[u8]) -> PathBuf {
    let p = dir.join(name);
    fs::write(&p, data).unwrap();
    p
}

/// A fresh folder on the test drive named by `variable` (set by
/// scripts/test-volumes.ps1), or `None`, after skipping, when it isn't set.
pub fn test_drive(variable: &str) -> Option<tempfile::TempDir> {
    match std::env::var_os(variable) {
        Some(root) => Some(tempfile::tempdir_in(root).unwrap()),
        None => {
            skip(&format!(
                "{variable} isn't set; run scripts/test-volumes.ps1 as an administrator"
            ));
            None
        }
    }
}
