//! The settings file, `settings.json`, read and checked.
//!
//! Every setting is optional: a missing file, or a missing setting, means
//! the default. Settings this version doesn't know (from a newer Scrimp) are
//! ignored rather than refused, so going back to an older version never
//! stops Scrimp starting. A file from a newer schema version is refused,
//! because its settings may not mean what this version thinks.
//!
//! A mistake is reported in plain words naming the setting, so a person
//! editing the file by hand can fix it.
//!
//! ```json
//! {
//!   "schemaVersion": 1,
//!   "defaultMethod": "auto",
//!   "minimumFileSizeBytes": 8192,
//!   "filesThatChangeOften": "skip",
//!   "workers": { "hdd": 1, "ssd": 4, "removable": 1, "unknown": 1 },
//!   "extensions": { "bak": "lzx", "dat": "skip" }
//! }
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::algorithm::{Algorithm, Target};
use crate::engine::Workers;
use crate::policy::{ChangesOften, Choice, Policy};

/// The schema version this Scrimp reads and writes.
pub const SCHEMA_VERSION: u32 = 1;

/// The most files Scrimp will work on at once on one drive.
const MOST_WORKERS: usize = 16;

/// The file as written, before it is checked.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Raw {
    schema_version: Option<u32>,
    default_method: Option<String>,
    minimum_file_size_bytes: Option<u64>,
    files_that_change_often: Option<String>,
    workers: Option<RawWorkers>,
    extensions: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Deserialize)]
struct RawWorkers {
    hdd: Option<usize>,
    ssd: Option<usize>,
    removable: Option<usize>,
    unknown: Option<usize>,
}

/// Checked settings, ready to use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// What Scrimp Compress does when no method is chosen.
    pub default_target: Target,
    pub minimum_size: u64,
    pub changes_often: ChangesOften,
    pub workers: Workers,
    /// Per-extension choices for automatic mode, extensions in lower case
    /// without the dot.
    pub extensions: BTreeMap<String, Choice>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            default_target: Target::Automatic,
            minimum_size: Policy::default().minimum_size,
            changes_often: ChangesOften::default(),
            workers: Workers::default(),
            extensions: BTreeMap::new(),
        }
    }
}

/// What is wrong with a settings file, in plain words.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct SettingsError(String);

impl Settings {
    /// Reads settings from text.
    pub fn parse(text: &str) -> Result<Settings, SettingsError> {
        let raw: Raw = serde_json::from_str(text).map_err(|e| {
            SettingsError(format!(
                "settings.json isn't valid JSON at line {}, column {}: {}.",
                e.line(),
                e.column(),
                plain_json_error(&e)
            ))
        })?;
        let defaults = Settings::default();

        if let Some(v) = raw.schema_version
            && v > SCHEMA_VERSION
        {
            return Err(SettingsError(format!(
                "settings.json was written by a newer version of Scrimp (schema version {v}). Update Scrimp, or delete the file to start again."
            )));
        }

        let default_target = match raw.default_method {
            None => defaults.default_target,
            Some(m) => m.parse::<Target>().map_err(|_| {
                SettingsError(format!(
                    "\"defaultMethod\" is \"{m}\", which isn't a compression method. Use one of: auto, xpress4k, xpress8k, xpress16k, lzx, ntfs."
                ))
            })?,
        };

        let changes_often = match raw.files_that_change_often.as_deref() {
            None => defaults.changes_often,
            Some("skip") => ChangesOften::Skip,
            Some("ntfs") => ChangesOften::UseNtfs,
            Some(other) => {
                return Err(SettingsError(format!(
                    "\"filesThatChangeOften\" is \"{other}\". Use \"skip\" or \"ntfs\"."
                )));
            }
        };

        let mut workers = defaults.workers;
        if let Some(w) = raw.workers {
            let check = |name: &str, value: Option<usize>, default: usize| match value {
                None => Ok(default),
                Some(n) if (1..=MOST_WORKERS).contains(&n) => Ok(n),
                Some(n) => Err(SettingsError(format!(
                    "\"workers.{name}\" is {n}. Use a number from 1 to {MOST_WORKERS}."
                ))),
            };
            workers.hdd = check("hdd", w.hdd, workers.hdd)?;
            workers.ssd = check("ssd", w.ssd, workers.ssd)?;
            workers.removable = check("removable", w.removable, workers.removable)?;
            workers.unknown = check("unknown", w.unknown, workers.unknown)?;
        }

        let mut extensions = BTreeMap::new();
        for (ext, choice) in raw.extensions.unwrap_or_default() {
            let key = ext.trim().trim_start_matches('.').to_lowercase();
            if key.is_empty() {
                return Err(SettingsError("\"extensions\" has an empty extension.".into()));
            }
            let value = if choice.eq_ignore_ascii_case("skip") {
                Choice::Skip
            } else {
                Choice::Use(choice.parse::<Algorithm>().map_err(|_| {
                    SettingsError(format!(
                        "\"extensions.{ext}\" is \"{choice}\". Use \"skip\" or one of: xpress4k, xpress8k, xpress16k, lzx, ntfs."
                    ))
                })?)
            };
            extensions.insert(key, value);
        }

        Ok(Settings {
            default_target,
            minimum_size: raw.minimum_file_size_bytes.unwrap_or(defaults.minimum_size),
            changes_often,
            workers,
            extensions,
        })
    }

    /// Reads settings from a file. A file that doesn't exist means the defaults.
    pub fn load(path: &Path) -> Result<Settings, SettingsError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Settings::parse(&text).map_err(|e| SettingsError(format!("{} ({})", e.0, path.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
            Err(e) => Err(SettingsError(format!("Couldn't read {}: {e}.", path.display()))),
        }
    }

    /// Where the settings file lives: `%LOCALAPPDATA%\Scrimp\settings.json`.
    /// `None` when `LOCALAPPDATA` isn't set, which only happens off Windows.
    pub fn default_path() -> Option<PathBuf> {
        std::env::var_os("LOCALAPPDATA").map(|dir| Path::new(&dir).join("Scrimp").join("settings.json"))
    }

    /// The policy these settings describe, protecting this computer's
    /// Windows folders.
    pub fn policy(&self) -> Policy {
        let mut policy = Policy::for_this_computer();
        policy.minimum_size = self.minimum_size;
        policy.changes_often = self.changes_often;
        for (ext, choice) in &self.extensions {
            policy = policy.with_override(ext, *choice);
        }
        policy
    }
}

/// serde_json's messages, minus the position it repeats and the jargon.
fn plain_json_error(e: &serde_json::Error) -> String {
    let text = e.to_string();
    let text = text.split(" at line ").next().unwrap_or(&text);
    text.replace("invalid type: ", "wrong kind of value: ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outcome::SkipReason;
    use crate::policy::Plan;
    use crate::state::CompressionState;

    #[test]
    fn an_empty_file_or_no_file_means_the_defaults() {
        assert_eq!(Settings::parse("{}").unwrap(), Settings::default());
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            Settings::load(&dir.path().join("settings.json")).unwrap(),
            Settings::default()
        );
    }

    #[test]
    fn every_setting_is_read() {
        let s = Settings::parse(
            r#"{
              "schemaVersion": 1,
              "defaultMethod": "LZX",
              "minimumFileSizeBytes": 65536,
              "filesThatChangeOften": "ntfs",
              "workers": { "hdd": 2, "ssd": 8 },
              "extensions": { ".BAK": "lzx", "dat": "skip" }
            }"#,
        )
        .unwrap();
        assert_eq!(s.default_target, Target::Algorithm(Algorithm::Lzx));
        assert_eq!(s.minimum_size, 65536);
        assert_eq!(s.changes_often, ChangesOften::UseNtfs);
        assert_eq!((s.workers.hdd, s.workers.ssd, s.workers.removable), (2, 8, 1));
        assert_eq!(s.extensions["bak"], Choice::Use(Algorithm::Lzx));
        assert_eq!(s.extensions["dat"], Choice::Skip);
    }

    #[test]
    fn settings_from_a_newer_scrimp_are_ignored_but_a_newer_schema_is_refused() {
        let s = Settings::parse(r#"{ "schemaVersion": 1, "somethingNew": true }"#).unwrap();
        assert_eq!(s, Settings::default());
        let err = Settings::parse(r#"{ "schemaVersion": 2 }"#).unwrap_err().to_string();
        assert!(
            err.starts_with("settings.json was written by a newer version of Scrimp"),
            "{err}"
        );
    }

    #[test]
    fn mistakes_name_the_setting_and_say_what_would_work() {
        let cases = [
            (
                r#"{ "defaultMethod": "zip" }"#,
                "\"defaultMethod\" is \"zip\", which isn't a compression method.",
            ),
            (
                r#"{ "filesThatChangeOften": "always" }"#,
                "\"filesThatChangeOften\" is \"always\". Use \"skip\" or \"ntfs\".",
            ),
            (
                r#"{ "workers": { "ssd": 0 } }"#,
                "\"workers.ssd\" is 0. Use a number from 1 to 16.",
            ),
            (r#"{ "workers": { "hdd": 99 } }"#, "\"workers.hdd\" is 99."),
            (
                r#"{ "extensions": { "bak": "best" } }"#,
                "\"extensions.bak\" is \"best\".",
            ),
            (
                r#"{ "extensions": { ".": "lzx" } }"#,
                "\"extensions\" has an empty extension.",
            ),
            (r#"{ "minimumFileSizeBytes": "big" }"#, "wrong kind of value"),
            ("{ \"defaultMethod\": ", "isn't valid JSON at line 1, column"),
        ];
        for (text, wanted) in cases {
            let err = Settings::parse(text).unwrap_err().to_string();
            assert!(err.contains(wanted), "{text}: {err}");
        }
    }

    #[test]
    fn errors_from_a_file_say_which_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{ "defaultMethod": "zip" }"#).unwrap();
        let err = Settings::load(&path).unwrap_err().to_string();
        assert!(
            err.contains("settings.json") && err.contains(&path.display().to_string()),
            "{err}"
        );
        let folder = Settings::load(dir.path()).unwrap_err().to_string();
        assert!(folder.starts_with("Couldn't read"), "{folder}");
    }

    #[test]
    fn the_policy_follows_the_settings() {
        let s = Settings::parse(
            r#"{ "minimumFileSizeBytes": 100, "filesThatChangeOften": "ntfs", "extensions": { "mp4": "xpress4k" } }"#,
        )
        .unwrap();
        let p = s.policy();
        let plan = |path: &str, size| {
            p.plan(
                Path::new(path),
                &CompressionState::Uncompressed,
                size,
                Target::Automatic,
            )
        };
        assert_eq!(
            plan(r"D:\Videos\Holiday.mp4", 1000),
            Plan::Compress(Algorithm::Xpress4k)
        );
        assert_eq!(plan(r"D:\VMs\Example.vhdx", 1000), Plan::Compress(Algorithm::Lznt1));
        assert_eq!(plan(r"D:\Archive\notes.txt", 200), Plan::Compress(Algorithm::Lzx));
        assert_eq!(plan(r"D:\Archive\notes.txt", 50), Plan::Skip(SkipReason::TooSmall));
    }

    #[test]
    fn the_file_lives_in_local_app_data() {
        if let Some(path) = Settings::default_path() {
            assert!(path.ends_with(Path::new("Scrimp").join("settings.json")));
        }
    }
}
