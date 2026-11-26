//! `cargo xtask coverage lcov.info`: the stricter coverage bar for the code
//! that changes files.
//!
//! CI already fails under 90% of lines overall. The plan holds the code
//! that compresses, decompresses, verifies and reads a file's state to 95%,
//! because a mistake there can damage someone's files. `cargo llvm-cov`
//! can only apply one bar to everything, so CI writes an lcov report and
//! this checks those files against it. When one falls short, it lists the
//! lines no test runs, so the fix starts from the right place.

use anyhow::{Result, bail};

/// The files held to [`STRICT`], as lcov names them, matched by ending.
pub const STRICT_FILES: [&str; 2] = ["scrimp-win/src/file.rs", "scrimp-core/src/state.rs"];

/// Percent of lines.
pub const STRICT: f64 = 95.0;

/// One source file's line coverage.
#[derive(Debug, PartialEq)]
pub struct FileLines {
    pub path: String,
    pub found: u64,
    pub hit: u64,
    /// Lines no test ran.
    pub missed: Vec<u64>,
}

impl FileLines {
    pub fn percent(&self) -> f64 {
        if self.found == 0 {
            100.0
        } else {
            self.hit as f64 * 100.0 / self.found as f64
        }
    }
}

/// Reads an lcov report: per file, the `DA:<line>,<count>` records.
pub fn parse(lcov: &str) -> Vec<FileLines> {
    let mut out = Vec::new();
    let mut current: Option<FileLines> = None;
    for line in lcov.lines() {
        if let Some(path) = line.strip_prefix("SF:") {
            current = Some(FileLines {
                path: path.replace('\\', "/"),
                found: 0,
                hit: 0,
                missed: Vec::new(),
            });
        } else if let Some(record) = line.strip_prefix("DA:") {
            let mut parts = record.split(',');
            if let (Some(file), Some(Ok(number)), Some(Ok(count))) = (
                current.as_mut(),
                parts.next().map(str::parse::<u64>),
                parts.next().map(str::parse::<u64>),
            ) {
                file.found += 1;
                if count > 0 {
                    file.hit += 1;
                } else {
                    file.missed.push(number);
                }
            }
        } else if line == "end_of_record"
            && let Some(file) = current.take()
        {
            out.push(file);
        }
    }
    out
}

/// Checks the strict files, returning a line about each.
pub fn check(lcov: &str) -> Result<Vec<String>> {
    let files = parse(lcov);
    let mut report = Vec::new();
    let mut short = false;
    for wanted in STRICT_FILES {
        let Some(file) = files.iter().find(|f| f.path.ends_with(wanted)) else {
            bail!(
                "The coverage report has no {wanted}. Was it renamed? Update STRICT_FILES in crates/xtask/src/coverage.rs."
            );
        };
        let percent = file.percent();
        if percent + f64::EPSILON < STRICT {
            short = true;
            report.push(format!(
                "{wanted}: {percent:.2}% of lines, under {STRICT}%. Lines no test runs: {}",
                ranges(&file.missed)
            ));
        } else {
            report.push(format!("{wanted}: {percent:.2}% of lines"));
        }
    }
    if short {
        bail!("{}", report.join("\n"));
    }
    Ok(report)
}

/// `[3, 4, 5, 9]` as "3-5, 9".
fn ranges(lines: &[u64]) -> String {
    let mut sorted = lines.to_vec();
    sorted.sort_unstable();
    let mut parts: Vec<String> = Vec::new();
    let mut i = 0;
    while i < sorted.len() {
        let start = sorted[i];
        let mut end = start;
        while i + 1 < sorted.len() && sorted[i + 1] == end + 1 {
            i += 1;
            end = sorted[i];
        }
        parts.push(if start == end {
            start.to_string()
        } else {
            format!("{start}-{end}")
        });
        i += 1;
    }
    parts.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(path: &str, lines: &[(u64, u64)]) -> String {
        let mut out = format!("SF:{path}\n");
        for (line, count) in lines {
            out.push_str(&format!("DA:{line},{count}\n"));
        }
        out.push_str("end_of_record\n");
        out
    }

    fn all_hit(n: u64) -> Vec<(u64, u64)> {
        (1..=n).map(|l| (l, 1)).collect()
    }

    #[test]
    fn files_are_read_with_windows_paths_too() {
        let lcov = record(
            r"D:\a\scrimp\crates\scrimp-win\src\file.rs",
            &[(1, 3), (2, 0), (5, 0), (6, 1)],
        );
        let files = parse(&lcov);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "D:/a/scrimp/crates/scrimp-win/src/file.rs");
        assert_eq!((files[0].found, files[0].hit), (4, 2));
        assert_eq!(files[0].missed, [2, 5]);
        assert_eq!(files[0].percent(), 50.0);
    }

    #[test]
    fn the_strict_files_pass_at_95_and_fail_below_with_the_lines_to_test() {
        let state = record("/r/crates/scrimp-core/src/state.rs", &all_hit(10));
        let mut lines = all_hit(100);
        for l in [10, 11, 12, 40, 70] {
            lines[l as usize - 1].1 = 0;
        }
        let at_95 = record("/r/crates/scrimp-win/src/file.rs", &lines);
        assert!(check(&format!("{state}{at_95}")).is_ok());

        lines[0].1 = 0;
        let under = record("/r/crates/scrimp-win/src/file.rs", &lines);
        let err = check(&format!("{state}{under}")).unwrap_err().to_string();
        assert!(err.contains("94.00% of lines, under 95%"), "{err}");
        assert!(err.contains("Lines no test runs: 1, 10-12, 40, 70"), "{err}");
    }

    #[test]
    fn a_missing_file_is_an_error_not_a_pass() {
        let err = check(&record("/r/crates/scrimp-core/src/state.rs", &all_hit(3)))
            .unwrap_err()
            .to_string();
        assert!(err.contains("has no scrimp-win/src/file.rs"), "{err}");
    }

    #[test]
    fn ranges_read_naturally() {
        assert_eq!(ranges(&[]), "");
        assert_eq!(ranges(&[9, 3, 4, 5]), "3-5, 9");
        assert_eq!(ranges(&[1]), "1");
    }
}
