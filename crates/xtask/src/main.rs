//! `cargo xtask`: the project's own tooling.
//!
//! ```text
//! cargo xtask release              cut a release; the changelog decides the number
//! cargo xtask release 0.3.0        or you do
//! cargo xtask version              print the version in Cargo.toml
//! cargo xtask changelog section 0.2.0     print that version's notes
//! cargo xtask changelog next 0.2.0        print what should follow 0.2.0
//! cargo xtask changelog release 0.3.0     turn Unreleased into 0.3.0, dated today
//! cargo xtask changelog note "text"       add a line under Unreleased's Changed
//! ```
//!
//! Ported from Keevoro, so both projects release the same way. It is Rust
//! rather than a shell script so that it runs the same everywhere with
//! nothing extra installed, and so the rule that picks a version number is
//! covered by `cargo test` like everything else. The Release workflow on
//! GitHub runs these same commands, so the button and the terminal cannot
//! drift apart.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod changelog;

use anyhow::{Context, Result, bail};
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const CHANGELOG: &str = "CHANGELOG.md";
const MANIFEST: &str = "Cargo.toml";

const USAGE: &str = "Usage:
  cargo xtask release [<major.minor.patch>]
  cargo xtask version
  cargo xtask changelog section <version>
  cargo xtask changelog next <current version>
  cargo xtask changelog release <version>
  cargo xtask changelog note \"what changed\"";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    if let Err(e) = run(&args, Path::new(".")) {
        eprintln!("{e:#}");
        std::process::exit(1);
    }
}

fn run(args: &[&str], root: &Path) -> Result<()> {
    let read = |name: &str| std::fs::read_to_string(root.join(name)).with_context(|| format!("Couldn't read {name}"));
    let write = |name: &str, text: String| {
        std::fs::write(root.join(name), text).with_context(|| format!("Couldn't write {name}"))
    };
    match args {
        ["version"] => println!("{}", manifest_version(&read(MANIFEST)?)?),
        ["changelog", "section", version] => {
            changelog::parse_version(version)?;
            println!("{}", changelog::section(&read(CHANGELOG)?, version)?);
        }
        ["changelog", "next", current] => {
            // The number alone on stdout, for scripts; the reason on
            // stderr, for people.
            let next = changelog::next_version(&read(CHANGELOG)?, current)?;
            eprintln!("After {current} comes {}: {}.", next.version, next.why);
            println!("{}", next.version);
        }
        ["changelog", "release", version] => {
            changelog::parse_version(version)?;
            let today = today();
            write(CHANGELOG, changelog::release(&read(CHANGELOG)?, version, &today)?)?;
            eprintln!("CHANGELOG.md: Unreleased is now {version} ({today}).");
        }
        ["changelog", "note", line] if !line.trim().is_empty() => {
            write(CHANGELOG, changelog::note(&read(CHANGELOG)?, line.trim())?)?;
            eprintln!("CHANGELOG.md: noted under Unreleased.");
        }
        ["release"] => cut_release(root, None)?,
        ["release", version] => cut_release(root, Some(version))?,
        _ => bail!("{USAGE}"),
    }
    Ok(())
}

/// Dates the changelog, sets the version, commits and tags. Pushing is left
/// to the caller: `git push --follow-tags`, or the Release workflow.
fn cut_release(root: &Path, asked: Option<&str>) -> Result<()> {
    let version = match asked {
        Some(v) => v.to_string(),
        None => {
            // Releases cut from the button are tagged on GitHub, not here.
            // Without them this would count up from a stale number.
            if git(root, &["fetch", "--tags", "--quiet"]).is_err() {
                eprintln!("Could not fetch tags; working from the ones already here.");
            }
            let current = changelog::highest(&git(root, &["tag", "-l", "v*"])?);
            let text = std::fs::read_to_string(root.join(CHANGELOG)).context("Couldn't read CHANGELOG.md")?;
            let next = changelog::next_version(&text, &current)?;
            eprintln!("Releasing {}: {}.", next.version, next.why);
            next.version
        }
    };
    changelog::parse_version(&version)?;
    if !git(root, &["tag", "-l", &format!("v{version}")])?.trim().is_empty() {
        bail!("v{version} already exists.");
    }
    if !git(root, &["status", "--porcelain"])?.trim().is_empty() {
        bail!("Commit or stash your changes first; a release commit should carry only the version.");
    }

    // The changelog first, so one with nothing to say stops the release
    // before any version number has moved.
    let log = std::fs::read_to_string(root.join(CHANGELOG)).context("Couldn't read CHANGELOG.md")?;
    let dated = changelog::release(&log, &version, &today())?;
    let manifest = std::fs::read_to_string(root.join(MANIFEST)).context("Couldn't read Cargo.toml")?;
    let bumped = set_manifest_version(&manifest, &version)?;
    std::fs::write(root.join(CHANGELOG), dated)?;
    std::fs::write(root.join(MANIFEST), bumped)?;

    // Cargo.lock records the workspace crates' versions too.
    let lock = Command::new("cargo")
        .args(["update", "--workspace", "--quiet"])
        .current_dir(root)
        .status();
    if !lock.is_ok_and(|s| s.success()) {
        bail!("Couldn't update Cargo.lock for the new version.");
    }
    git(root, &["commit", "-am", &format!("Release v{version}"), "--quiet"])?;
    git(
        root,
        &["tag", "-a", &format!("v{version}"), "-m", &format!("Scrimp v{version}")],
    )?;
    eprintln!("\nTagged v{version}. Now push it to build and publish:\n\n  git push --follow-tags\n");
    Ok(())
}

/// The version every crate inherits, from `[workspace.package]`.
fn manifest_version(manifest: &str) -> Result<String> {
    manifest
        .lines()
        .find_map(|l| l.strip_prefix("version = \"")?.strip_suffix('"'))
        .map(str::to_string)
        .context("Cargo.toml has no version line under [workspace.package]")
}

fn set_manifest_version(manifest: &str, version: &str) -> Result<String> {
    // Already there is fine: the very first release ships the number the
    // manifest was created with. A repeat is caught by the tag check.
    let current = manifest_version(manifest)?;
    Ok(manifest.replacen(
        &format!("version = \"{current}\""),
        &format!("version = \"{version}\""),
        1,
    ))
}

fn git(root: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .context("Couldn't run git")?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Today's date in UTC, as `2026-11-15`.
fn today() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0);
    date_from_unix_seconds(secs)
}

/// The UTC calendar date for a Unix time, as `YYYY-MM-DD`. Howard Hinnant's
/// `civil_from_days`, so no date crate is needed for one line of tooling.
fn date_from_unix_seconds(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const MANIFEST_TEXT: &str = "[workspace]\nresolver = \"2\"\n\n[workspace.package]\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace.dependencies]\nserde = { version = \"1\" }\n";

    /// A tiny git repository shaped like this one.
    fn repo(unreleased: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(
            root.join("Cargo.toml"),
            format!(
                "{MANIFEST_TEXT}\n[package]\nname = \"demo\"\nversion.workspace = true\nedition.workspace = true\n"
            ),
        )
        .unwrap();
        fs::create_dir(root.join("src")).unwrap();
        fs::write(root.join("src/lib.rs"), "").unwrap();
        fs::write(
            root.join("CHANGELOG.md"),
            format!("# Changelog\n\n## [Unreleased]\n\n{unreleased}\n"),
        )
        .unwrap();
        fs::write(root.join(".gitignore"), "target\n").unwrap();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.name", "Test"],
            vec!["config", "user.email", "test@example.invalid"],
            vec!["config", "commit.gpgsign", "false"],
            vec!["config", "tag.gpgsign", "false"],
        ] {
            git(root, &args).unwrap();
        }
        // The lock file is part of a real repository and of the release commit.
        assert!(
            Command::new("cargo")
                .args(["generate-lockfile", "--quiet"])
                .current_dir(root)
                .status()
                .unwrap()
                .success()
        );
        git(root, &["add", "-A"]).unwrap();
        git(root, &["commit", "-qm", "Start"]).unwrap();
        dir
    }

    #[test]
    fn the_changelog_picks_the_number_and_the_release_is_one_tidy_commit() {
        let dir = repo("### Added\n\n- The first thing.");
        let root = dir.path();
        run(&["release"], root).unwrap();

        assert_eq!(
            git(root, &["tag", "-l"]).unwrap().trim(),
            "v0.1.0",
            "nothing tagged before, so 0.0.0 then Added"
        );
        assert_eq!(
            git(root, &["log", "-1", "--format=%s"]).unwrap().trim(),
            "Release v0.1.0"
        );
        assert!(git(root, &["status", "--porcelain"]).unwrap().trim().is_empty());
        let log = fs::read_to_string(root.join("CHANGELOG.md")).unwrap();
        assert_eq!(
            changelog::section(&log, "0.1.0").unwrap(),
            "### Added\n\n- The first thing."
        );
        assert_eq!(changelog::section(&log, "Unreleased").unwrap(), changelog::EMPTY);
    }

    #[test]
    fn a_chosen_version_is_set_everywhere() {
        let dir = repo("### Fixed\n\n- A thing.");
        let root = dir.path();
        run(&["release", "0.4.0"], root).unwrap();
        assert_eq!(
            manifest_version(&fs::read_to_string(root.join("Cargo.toml")).unwrap()).unwrap(),
            "0.4.0"
        );
        assert!(
            fs::read_to_string(root.join("Cargo.lock"))
                .unwrap()
                .contains("name = \"demo\"\nversion = \"0.4.0\"")
        );
        let changed = git(root, &["show", "--stat", "--format=", "HEAD"]).unwrap();
        for file in ["CHANGELOG.md", "Cargo.toml", "Cargo.lock"] {
            assert!(changed.contains(file), "{changed}");
        }

        // The next one counts on from the tag.
        fs::write(root.join("src/lib.rs"), "// fixed\n").unwrap();
        run(&["changelog", "note", "Tidied."], root).unwrap();
        git(root, &["commit", "-qam", "Tidy"]).unwrap();
        run(&["release"], root).unwrap();
        assert!(git(root, &["tag", "-l"]).unwrap().contains("v0.4.1"));
    }

    #[test]
    fn a_release_refuses_rather_than_do_half_a_job() {
        let empty = repo("Nothing yet.");
        let err = run(&["release"], empty.path()).unwrap_err().to_string();
        assert!(err.contains("nothing to release"), "{err}");
        let err = run(&["release", "0.2.0"], empty.path()).unwrap_err().to_string();
        assert!(err.contains("Say what changed in 0.2.0"), "{err}");
        assert_eq!(
            manifest_version(&fs::read_to_string(empty.path().join("Cargo.toml")).unwrap()).unwrap(),
            "0.1.0"
        );

        let dirty = repo("### Fixed\n\n- A thing.");
        fs::write(dirty.path().join("src/lib.rs"), "// half-finished\n").unwrap();
        let err = run(&["release", "0.2.0"], dirty.path()).unwrap_err().to_string();
        assert!(err.starts_with("Commit or stash your changes first"), "{err}");

        let same = repo("### Fixed\n\n- A thing.");
        run(&["release", "0.1.0"], same.path()).unwrap();
        run(&["changelog", "note", "More."], same.path()).unwrap();
        git(same.path(), &["commit", "-qam", "More"]).unwrap();
        let err = run(&["release", "0.1.0"], same.path()).unwrap_err().to_string();
        assert_eq!(err, "v0.1.0 already exists.");
        assert!(run(&["release", "next"], same.path()).is_err());
        assert_eq!(git(same.path(), &["tag", "-l"]).unwrap().trim(), "v0.1.0");
        assert!(
            git(same.path(), &["status", "--porcelain"]).unwrap().trim().is_empty(),
            "nothing was half-written"
        );
    }

    #[test]
    fn the_small_commands_work_and_mistakes_show_the_usage() {
        let dir = repo("### Fixed\n\n- A thing.");
        let root = dir.path();
        run(&["version"], root).unwrap();
        run(&["changelog", "section", "0.1.0"], root).unwrap();
        run(&["changelog", "next", "0.1.0"], root).unwrap();
        run(&["changelog", "release", "0.1.1"], root).unwrap();
        assert!(
            fs::read_to_string(root.join("CHANGELOG.md"))
                .unwrap()
                .contains(&format!("## [0.1.1] - {}", today()))
        );

        for bad in [
            &["changelog", "section", "latest"][..],
            &["changelog", "note", " "],
            &["publish"],
            &[],
        ] {
            assert!(run(bad, root).is_err(), "{bad:?}");
        }
        assert!(
            run(&["frobnicate"], root)
                .unwrap_err()
                .to_string()
                .starts_with("Usage:")
        );
        assert!(run(&["version"], Path::new("/definitely/not/here")).is_err());
    }

    #[test]
    fn the_version_line_is_found_and_only_it_is_changed() {
        assert_eq!(manifest_version(MANIFEST_TEXT).unwrap(), "0.1.0");
        let bumped = set_manifest_version(MANIFEST_TEXT, "0.2.0").unwrap();
        assert!(bumped.contains("version = \"0.2.0\"\nedition"));
        assert!(
            bumped.contains("serde = { version = \"1\" }"),
            "dependency versions are left alone"
        );
        assert_eq!(set_manifest_version(MANIFEST_TEXT, "0.1.0").unwrap(), MANIFEST_TEXT);
        assert!(manifest_version("[workspace]\n").is_err());
        assert_eq!(today().len(), 10);
    }

    #[test]
    fn dates_are_worked_out_without_a_date_crate() {
        assert_eq!(date_from_unix_seconds(0), "1970-01-01");
        assert_eq!(date_from_unix_seconds(951_782_400), "2000-02-29");
        assert_eq!(date_from_unix_seconds(1_795_651_200), "2026-11-26");
        assert_eq!(date_from_unix_seconds(-86_400), "1969-12-31");
    }
}
