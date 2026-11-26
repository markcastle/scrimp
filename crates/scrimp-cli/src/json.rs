//! The `--json` output: one JSON object per line, each with
//! `"schemaVersion"` and `"type"`. Documented in docs/command-line.md; a
//! change to a field's meaning needs a new schema version.

use std::path::Path;

use scrimp_core::{CompressionState, Event, Outcome, Report};
use serde_json::{Value, json};

use crate::words::{fail_name, skip_name, state_name, unsupported_name};

/// The version of this output format.
pub const SCHEMA_VERSION: u32 = 1;

fn object(kind: &str, mut fields: Value) -> Value {
    if let Some(map) = fields.as_object_mut() {
        map.insert("schemaVersion".into(), json!(SCHEMA_VERSION));
        map.insert("type".into(), json!(kind));
    }
    fields
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// One file's outcome.
pub fn outcome(path: &Path, outcome: &Outcome, on_disk: u64) -> Value {
    let path = path_text(path);
    let fields = match outcome {
        Outcome::Compressed { algorithm, sizes } => json!({
            "path": path, "outcome": "compressed", "method": algorithm.cli_name(),
            "logicalBytes": sizes.logical, "beforeBytes": sizes.before, "afterBytes": sizes.after,
        }),
        Outcome::Decompressed { sizes } => json!({
            "path": path, "outcome": "decompressed",
            "logicalBytes": sizes.logical, "beforeBytes": sizes.before, "afterBytes": sizes.after,
        }),
        Outcome::AlreadyDone => json!({ "path": path, "outcome": "alreadyDone", "onDiskBytes": on_disk }),
        Outcome::NotBeneficial => json!({ "path": path, "outcome": "notBeneficial", "onDiskBytes": on_disk }),
        Outcome::Skipped(reason) => json!({
            "path": path, "outcome": "skipped", "reason": skip_name(reason), "onDiskBytes": on_disk,
        }),
        Outcome::Failed { reason, left_as } => {
            let mut v = json!({
                "path": path, "outcome": "failed", "reason": fail_name(reason),
                "leftAs": state_name(left_as), "onDiskBytes": on_disk,
            });
            if let scrimp_core::FailReason::Windows { code, message } = reason {
                v["code"] = json!(code);
                v["message"] = json!(message);
            }
            v
        }
        Outcome::Cancelled => json!({ "path": path, "outcome": "cancelled" }),
    };
    object("file", fields)
}

/// Any engine event.
pub fn event(event: &Event) -> Value {
    match event {
        Event::File {
            path,
            outcome: o,
            on_disk,
        } => outcome(path, o, *on_disk),
        Event::FolderUnreadable { path, reason } => object(
            "folderUnreadable",
            json!({ "path": path_text(path), "reason": fail_name(reason) }),
        ),
        Event::WofUnavailable { root } => object("wofUnavailable", json!({ "drive": path_text(root) })),
        Event::VirtualDisk { root } => object("virtualDisk", json!({ "drive": path_text(root) })),
    }
}

/// The end of a job.
pub fn summary(report: &Report, name: Option<&str>, dry_run: bool) -> Value {
    let (before, after) = report.sizes();
    object(
        "summary",
        json!({
            "dryRun": dry_run,
            "beforeBytes": before,
            "afterBytes": after,
            "savedBytes": report.saved().to_string().parse::<i64>().unwrap_or(0),
            "failures": report.failures(),
            "lines": report.summary(name),
        }),
    )
}

/// One file, as `inspect` reads it.
pub fn inspected(path: &Path, state: &CompressionState, logical: u64, on_disk: u64) -> Value {
    let mut v = json!({
        "path": path_text(path), "state": state_name(state), "logicalBytes": logical, "onDiskBytes": on_disk,
    });
    if let CompressionState::Unsupported(reason) = state {
        v["reason"] = json!(unsupported_name(reason));
    }
    object("file", v)
}

/// A path `inspect` couldn't read.
pub fn unreadable(path: &Path, message: &str) -> Value {
    object("error", json!({ "path": path_text(path), "message": message }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use scrimp_core::{Algorithm, FailReason, Mode, Sizes, SkipReason, UnsupportedReason};
    use std::path::PathBuf;

    fn get<'a>(v: &'a Value, key: &str) -> &'a Value {
        &v[key]
    }

    #[test]
    fn every_line_says_its_version_and_type() {
        let p = Path::new(r"D:\Games\game.exe");
        let lines = [
            outcome(p, &Outcome::Cancelled, 0),
            event(&Event::WofUnavailable {
                root: PathBuf::from(r"W:\"),
            }),
            event(&Event::VirtualDisk {
                root: PathBuf::from(r"V:\"),
            }),
            event(&Event::FolderUnreadable {
                path: PathBuf::from(r"D:\Games\Private"),
                reason: FailReason::AccessDenied,
            }),
            summary(&Report::new(Mode::Compress), None, false),
            inspected(p, &CompressionState::Uncompressed, 1, 1),
            unreadable(p, "the file can't be found"),
        ];
        for line in &lines {
            assert_eq!(get(line, "schemaVersion"), &json!(1), "{line}");
            assert!(get(line, "type").is_string(), "{line}");
            assert!(!line.to_string().contains('\n'), "one object per line");
        }
        assert_eq!(lines[1]["drive"], json!(r"W:\"));
        assert_eq!(
            (lines[2]["type"].as_str(), &lines[2]["drive"]),
            (Some("virtualDisk"), &json!(r"V:\"))
        );
        assert_eq!(lines[3]["reason"], json!("accessDenied"));
    }

    #[test]
    fn outcomes_carry_their_numbers_and_reasons() {
        let p = Path::new(r"D:\Games\game.exe");
        let sizes = Sizes {
            logical: 100,
            before: 100,
            after: 40,
        };
        let c = outcome(
            p,
            &Outcome::Compressed {
                algorithm: Algorithm::Xpress16k,
                sizes,
            },
            40,
        );
        assert_eq!(
            (c["outcome"].clone(), c["method"].clone(), c["afterBytes"].clone()),
            (json!("compressed"), json!("xpress16k"), json!(40))
        );
        let d = outcome(p, &Outcome::Decompressed { sizes }, 100);
        assert_eq!(d["outcome"], json!("decompressed"));
        let s = outcome(
            p,
            &Outcome::Skipped(SkipReason::CantCompressHere(UnsupportedReason::Link)),
            0,
        );
        assert_eq!(s["reason"], json!("link"));
        let f = outcome(
            p,
            &Outcome::Failed {
                reason: FailReason::Windows {
                    code: 87,
                    message: "The parameter is incorrect.".into(),
                },
                left_as: CompressionState::Uncompressed,
            },
            100,
        );
        assert_eq!(
            (f["code"].clone(), f["leftAs"].clone()),
            (json!(87), json!("uncompressed"))
        );
        assert_eq!(outcome(p, &Outcome::AlreadyDone, 5)["onDiskBytes"], json!(5));
        assert_eq!(
            outcome(p, &Outcome::NotBeneficial, 5)["outcome"],
            json!("notBeneficial")
        );
        let e = event(&Event::File {
            path: p.into(),
            outcome: Outcome::NotBeneficial,
            on_disk: 5,
        });
        assert_eq!(e["type"], json!("file"));
    }

    #[test]
    fn the_summary_holds_the_report() {
        let mut r = Report::new(Mode::Compress);
        r.add(
            &Outcome::Compressed {
                algorithm: Algorithm::Lzx,
                sizes: Sizes {
                    logical: 2048,
                    before: 2048,
                    after: 1024,
                },
            },
            1024,
        );
        let s = summary(&r, Some("Games"), true);
        assert_eq!(s["savedBytes"], json!(1024));
        assert_eq!(s["dryRun"], json!(true));
        assert_eq!(
            s["lines"][0],
            json!("Saved 1.0 KB. Games now takes up 1.0 KB instead of 2.0 KB.")
        );
        let unsupported = inspected(
            Path::new(r"X:\notes.txt"),
            &CompressionState::Unsupported(UnsupportedReason::FileSystem("exFAT".into())),
            1,
            1,
        );
        assert_eq!(unsupported["reason"], json!("fileSystem"));
    }
}
