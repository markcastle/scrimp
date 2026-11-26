//! The changelog, read and written by the release tooling.
//!
//! `CHANGELOG.md` follows Keep a Changelog. "## [Unreleased]" at the top
//! collects changes as they land, written for the person using Scrimp.
//! Cutting a release gives that section the version and today's date and
//! opens a fresh Unreleased above it. The Release workflow copies a
//! version's section into its GitHub Release, so those words are exactly
//! what someone reads when they are offered an update.
//!
//! The functions here are pure text in, text out, so the rules (above all
//! the one that picks the next version number) can be tested without
//! touching a file or a git repository.

use anyhow::{Result, anyhow, bail};
use regex::Regex;

/// What an empty Unreleased section says.
pub const EMPTY: &str = "Nothing yet.";
pub const REPO: &str = "https://github.com/markcastle/scrimp";

fn heading(version: &str) -> Result<Regex> {
    Ok(Regex::new(&format!(r"(?m)^## \[{}\]", regex::escape(version)))?)
}

fn any_heading() -> Result<Regex> {
    Ok(Regex::new(r"(?m)^## \[")?)
}

/// Where a version's body starts and ends in the file, or `None` when the
/// version is not there.
fn body_range(text: &str, version: &str) -> Result<Option<(usize, usize)>> {
    let Some(found) = heading(version)?.find(text) else {
        return Ok(None);
    };
    let start = text[found.start()..]
        .find('\n')
        .map_or(text.len(), |i| found.start() + i + 1);
    let end = any_heading()?
        .find(&text[start..])
        .map_or(text.len(), |m| start + m.start());
    Ok(Some((start, end)))
}

/// The notes for one version, trimmed, without the link definitions at the
/// foot of the file. Empty when the version is not in the changelog.
pub fn section(text: &str, version: &str) -> Result<String> {
    let Some((start, end)) = body_range(text, version)? else {
        return Ok(String::new());
    };
    let links = Regex::new(r"(?m)^\[[^\]]+\]: .*$")?;
    Ok(links.replace_all(&text[start..end], "").trim().to_string())
}

/// What the next release should be numbered, and why.
#[derive(Debug, PartialEq, Eq)]
pub struct Next {
    pub version: String,
    pub why: String,
}

/// Works out the next version from what Unreleased holds.
///
/// The rule, made mechanical so nobody has to remember it: something
/// **Added** or **Removed** is something a person would notice as new or
/// gone, which is a minor release; only **Changed** and **Fixed** is a
/// patch. The changelog is written as the work lands, so by release time it
/// already knows which it is. Fails when there is nothing to release.
pub fn next_version(text: &str, current: &str) -> Result<Next> {
    let notes = section(text, "Unreleased")?;
    if notes.is_empty() || notes == EMPTY {
        bail!(
            "CHANGELOG.md has nothing under \"## [Unreleased]\", so there is nothing to release. \
             Say what changed first."
        );
    }
    let [major, minor, patch] = parse_version(current)?;
    let kinds = Regex::new(r"(?m)^### (\w+)\s*$")?;
    let noticeable: Vec<&str> = kinds
        .captures_iter(&notes)
        .filter_map(|c| c.get(1).map(|m| m.as_str()))
        .filter(|kind| matches!(*kind, "Added" | "Removed"))
        .collect();
    if noticeable.is_empty() {
        Ok(Next {
            version: format!("{major}.{minor}.{}", patch + 1),
            why: "a patch, because the changelog lists only changes and fixes".into(),
        })
    } else {
        Ok(Next {
            version: format!("{major}.{}.0", minor + 1),
            why: format!(
                "a minor release, because the changelog lists something {}",
                noticeable.join(" and ")
            ),
        })
    }
}

/// Retitles Unreleased as `version` dated `date`, adds the compare link and
/// opens a new Unreleased. Refuses when there is nothing to release: a
/// release with no notes is a release nobody can read about.
pub fn release(text: &str, version: &str, date: &str) -> Result<String> {
    let notes = section(text, "Unreleased")?;
    if notes.is_empty() || notes == EMPTY {
        bail!("CHANGELOG.md has nothing under \"## [Unreleased]\". Say what changed in {version} before releasing it.");
    }
    if heading(version)?.is_match(text) {
        bail!("CHANGELOG.md already has a section for {version}.");
    }
    let previous = Regex::new(r"(?m)^## \[(\d+\.\d+\.\d+)\]")?
        .captures(text)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string());

    let unreleased = Regex::new(r"(?m)^## \[Unreleased\]$")?;
    let out = unreleased.replace(text, format!("## [Unreleased]\n\n{EMPTY}\n\n## [{version}] - {date}"));
    let this_link = match previous {
        Some(p) => format!("[{version}]: {REPO}/compare/v{p}...v{version}"),
        None => format!("[{version}]: {REPO}/releases/tag/v{version}"),
    };
    let links = format!("[Unreleased]: {REPO}/compare/v{version}...HEAD\n{this_link}");
    let link_line = Regex::new(r"(?m)^\[Unreleased\]: .*$")?;
    if link_line.is_match(&out) {
        Ok(link_line.replace(&out, links.as_str()).into_owned())
    } else {
        Ok(format!("{}\n\n{links}\n", out.trim_end()))
    }
}

/// Adds one line under "### Changed" in Unreleased, making the heading if
/// it is missing. The Roll back workflow uses it to record what it did.
pub fn note(text: &str, line: &str) -> Result<String> {
    let (start, end) =
        body_range(text, "Unreleased")?.ok_or_else(|| anyhow!("CHANGELOG.md has no \"## [Unreleased]\" section."))?;
    let body = text[start..end].replace(&format!("{EMPTY}\n"), "");
    let changed = "### Changed\n\n";
    let body = if body.contains(changed) {
        body.replacen(changed, &format!("{changed}- {line}\n"), 1)
    } else {
        format!("\n{changed}- {line}\n\n{}\n\n", body.trim())
    };
    let tidy = Regex::new(r"\n{3,}")?.replace_all(&body, "\n\n").into_owned();
    Ok(format!("{}{tidy}{}", &text[..start], &text[end..]))
}

/// Splits `1.2.3` into numbers.
pub fn parse_version(version: &str) -> Result<[u64; 3]> {
    let parts: Vec<&str> = version.split('.').collect();
    let number = |s: &&str| (!s.is_empty() && s.chars().all(|c| c.is_ascii_digit())).then(|| s.parse::<u64>().ok())?;
    match parts.as_slice() {
        [a, b, c] => match (number(a), number(b), number(c)) {
            (Some(a), Some(b), Some(c)) => Ok([a, b, c]),
            _ => bail!("\"{version}\" is not a version. Use major.minor.patch, such as 0.9.0."),
        },
        _ => bail!("\"{version}\" is not a version. Use major.minor.patch, such as 0.9.0."),
    }
}

/// The highest of a list of tags like `v0.9.0`, compared as numbers so that
/// 0.10.0 comes after 0.9.0. `0.0.0` when there are none.
pub fn highest(tags: &str) -> String {
    tags.lines()
        .filter_map(|t| parse_version(t.trim().trim_start_matches('v')).ok())
        .max()
        .map_or_else(|| "0.0.0".to_string(), |[a, b, c]| format!("{a}.{b}.{c}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = "# Changelog

Intro.

## [Unreleased]

### Fixed

- A thing that was broken.

## [0.2.0] - 2026-11-01

### Added

- Something new.

## [0.1.0] - 2026-10-01

### Added

- The first thing.

[Unreleased]: https://github.com/markcastle/scrimp/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/markcastle/scrimp/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/markcastle/scrimp/releases/tag/v0.1.0
";

    fn with_unreleased(body: &str) -> String {
        LOG.replace("### Fixed\n\n- A thing that was broken.", body)
    }

    #[test]
    fn a_versions_notes_can_be_read_out() {
        assert_eq!(section(LOG, "0.2.0").unwrap(), "### Added\n\n- Something new.");
        assert_eq!(
            section(LOG, "Unreleased").unwrap(),
            "### Fixed\n\n- A thing that was broken."
        );
        // The last section stops before the link definitions.
        assert_eq!(section(LOG, "0.1.0").unwrap(), "### Added\n\n- The first thing.");
        assert_eq!(section(LOG, "9.9.9").unwrap(), "");
        // 0.1.0 must not match 0.10.0, nor a dot match any character.
        assert_eq!(section("## [0.10.0]\n\nten\n", "0.1.0").unwrap(), "");
        assert_eq!(section("## [0x1y0]\n\nodd\n", "0.1.0").unwrap(), "");
    }

    #[test]
    fn fixes_and_changes_make_a_patch() {
        let next = next_version(LOG, "0.2.0").unwrap();
        assert_eq!(next.version, "0.2.1");
        assert!(next.why.starts_with("a patch"));
        let changed = with_unreleased("### Changed\n\n- Reworded.\n\n### Fixed\n\n- Mended.");
        assert_eq!(next_version(&changed, "0.2.9").unwrap().version, "0.2.10");
    }

    #[test]
    fn something_added_or_removed_makes_a_minor_release() {
        let added = with_unreleased("### Added\n\n- A new command.\n\n### Fixed\n\n- Mended.");
        let next = next_version(&added, "0.2.7").unwrap();
        assert_eq!(next.version, "0.3.0");
        assert_eq!(next.why, "a minor release, because the changelog lists something Added");
        let removed = with_unreleased("### Removed\n\n- An old flag.");
        assert_eq!(next_version(&removed, "0.9.3").unwrap().version, "0.10.0");
        let both = with_unreleased("### Added\n\n- X.\n\n### Removed\n\n- Y.");
        assert!(next_version(&both, "1.4.0").unwrap().why.ends_with("Added and Removed"));
        // The very first release counts up from nothing.
        assert_eq!(next_version(&added, "0.0.0").unwrap().version, "0.1.0");
    }

    #[test]
    fn the_word_added_in_a_sentence_is_not_a_heading() {
        let text = with_unreleased("### Fixed\n\n- Added back the missing comma.");
        assert_eq!(next_version(&text, "0.2.0").unwrap().version, "0.2.1");
    }

    #[test]
    fn nothing_to_say_means_nothing_to_release() {
        for empty in ["Nothing yet.", ""] {
            let text = with_unreleased(empty);
            let err = next_version(&text, "0.2.0").unwrap_err().to_string();
            assert!(err.contains("nothing to release"), "{err}");
            let err = release(&text, "0.3.0", "2026-11-15").unwrap_err().to_string();
            assert!(err.contains("Say what changed in 0.3.0"), "{err}");
        }
        assert!(next_version(LOG, "two").is_err());
    }

    #[test]
    fn cutting_a_release_dates_the_notes_and_opens_a_fresh_section() {
        let out = release(LOG, "0.2.1", "2026-11-15").unwrap();
        assert!(
            out.contains("## [Unreleased]\n\nNothing yet.\n\n## [0.2.1] - 2026-11-15\n\n### Fixed"),
            "{out}"
        );
        assert_eq!(
            section(&out, "0.2.1").unwrap(),
            "### Fixed\n\n- A thing that was broken."
        );
        assert_eq!(section(&out, "Unreleased").unwrap(), EMPTY);
        assert!(out.contains("[Unreleased]: https://github.com/markcastle/scrimp/compare/v0.2.1...HEAD\n"));
        assert!(out.contains("[0.2.1]: https://github.com/markcastle/scrimp/compare/v0.2.0...v0.2.1\n"));
        assert!(out.contains("[0.2.0]: https://github.com/markcastle/scrimp/compare/v0.1.0...v0.2.0\n"));
        // And it can be done again straight after the next change lands.
        let again = note(&out, "Something else.").unwrap();
        assert_eq!(next_version(&again, "0.2.1").unwrap().version, "0.2.2");
        assert!(
            release(LOG, "0.2.0", "2026-11-15")
                .unwrap_err()
                .to_string()
                .contains("already has a section")
        );
    }

    #[test]
    fn the_first_release_links_to_its_own_page() {
        let first = "# Changelog\n\n## [Unreleased]\n\n### Added\n\n- Everything.\n";
        let out = release(first, "0.1.0", "2026-11-15").unwrap();
        assert!(out.contains("## [0.1.0] - 2026-11-15"));
        assert!(
            out.ends_with(
                "[Unreleased]: https://github.com/markcastle/scrimp/compare/v0.1.0...HEAD\n\
             [0.1.0]: https://github.com/markcastle/scrimp/releases/tag/v0.1.0\n"
            ),
            "{out}"
        );
    }

    #[test]
    fn a_note_goes_under_changed() {
        let fresh = with_unreleased("Nothing yet.");
        let out = note(&fresh, "Rolled back to the code of 0.1.0. It ate a file.").unwrap();
        assert_eq!(
            section(&out, "Unreleased").unwrap(),
            "### Changed\n\n- Rolled back to the code of 0.1.0. It ate a file."
        );
        assert!(
            out.contains("- Rolled back to the code of 0.1.0. It ate a file.\n\n## [0.2.0]"),
            "{out}"
        );

        // Beside what is already there, newest first.
        let twice = note(&out, "Second.").unwrap();
        assert_eq!(
            section(&twice, "Unreleased").unwrap(),
            "### Changed\n\n- Second.\n- Rolled back to the code of 0.1.0. It ate a file."
        );
        let with_fixed = note(LOG, "Noted.").unwrap();
        assert_eq!(
            section(&with_fixed, "Unreleased").unwrap(),
            "### Changed\n\n- Noted.\n\n### Fixed\n\n- A thing that was broken."
        );
        assert!(note("# Changelog\n", "x").is_err());
    }

    #[test]
    fn versions_are_compared_as_numbers() {
        assert_eq!(highest("v0.9.0\nv0.10.0\nv0.2.1\n"), "0.10.0");
        assert_eq!(highest("v1.0.0\nnightly\nv1.0\nv0.99.99"), "1.0.0");
        assert_eq!(highest(""), "0.0.0");
        assert_eq!(parse_version("0.12.3").unwrap(), [0, 12, 3]);
        for bad in ["1.2", "1.2.3.4", "v1.2.3", "1.2.x", "1..3", "+1.2.3", ""] {
            assert!(parse_version(bad).is_err(), "{bad}");
        }
    }
}
