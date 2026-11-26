//! The Release workflow refuses to publish a build that doesn't say exactly
//! `scrimp <version>`, so that line is pinned here too.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::process::Command;

#[test]
fn says_its_name_and_version() {
    let out = Command::new(env!("CARGO_BIN_EXE_scrimp"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8(out.stdout).unwrap().trim(),
        format!("scrimp {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn with_nothing_to_do_it_shows_help() {
    let out = Command::new(env!("CARGO_BIN_EXE_scrimp")).output().unwrap();
    let text = String::from_utf8(out.stderr).unwrap();
    assert!(text.contains("Save disk space"), "{text}");
}
