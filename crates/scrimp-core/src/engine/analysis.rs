//! Looking without changing: what a selection takes up now, how it's
//! stored, and what compressing it would do. `scrimp analyse` and
//! **Scrimp Savings…** both use this, so they always agree.
//!
//! It runs the engine for real, with Automatic as the target, against a
//! [`Preview`] disk that reads but never writes. Every decision the engine
//! would make shows up, and no file changes.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use super::{Control, Disk, DiskError, EntryKind, Event, FileFacts, Job, VolumeFacts, Workers, run};
use crate::algorithm::{Algorithm, Target};
use crate::outcome::{FailReason, Outcome, SkipReason};
use crate::policy::Policy;
use crate::report::{format_count, format_size};
use crate::state::CompressionState;

/// A disk that reads but never changes anything: for dry runs and
/// analysis. Asked to compress or decompress, it reports the file as it is,
/// so the engine's decisions show without any file changing. It remembers
/// every file it was asked about.
pub struct Preview<'a, D: Disk> {
    inner: &'a D,
    seen: Mutex<HashMap<PathBuf, FileFacts>>,
}

impl<'a, D: Disk> Preview<'a, D> {
    pub fn new(inner: &'a D) -> Self {
        Preview {
            inner,
            seen: Mutex::new(HashMap::new()),
        }
    }

    /// Every file looked at, with what was found.
    pub fn into_seen(self) -> HashMap<PathBuf, FileFacts> {
        self.seen.into_inner().unwrap_or_else(PoisonError::into_inner)
    }
}

impl<D: Disk> Disk for Preview<'_, D> {
    fn kind(&self, path: &Path) -> Result<EntryKind, FailReason> {
        self.inner.kind(path)
    }
    fn list(&self, folder: &Path) -> Result<Vec<(PathBuf, EntryKind)>, FailReason> {
        self.inner.list(folder)
    }
    fn volume(&self, path: &Path) -> Result<VolumeFacts, FailReason> {
        self.inner.volume(path)
    }
    fn free_space(&self, root: &Path) -> Result<u64, FailReason> {
        self.inner.free_space(root)
    }
    fn inspect(&self, path: &Path) -> Result<FileFacts, DiskError> {
        let facts = self.inner.inspect(path)?;
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(path.to_path_buf(), facts.clone());
        Ok(facts)
    }
    fn compress(&self, path: &Path, _: Algorithm) -> Result<FileFacts, DiskError> {
        self.inner.inspect(path)
    }
    /// Sampling only reads, so a preview samples for real, and shows what a
    /// real run would leave alone.
    fn sample(&self, path: &Path) -> Result<super::Sampled, DiskError> {
        self.inner.sample(path)
    }
    fn decompress(&self, path: &Path) -> Result<FileFacts, DiskError> {
        self.inner.inspect(path)
    }
}

/// How many files are in one state, and their sizes.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Group {
    pub files: u64,
    /// Their full size.
    pub logical: u64,
    /// The space they take on disk.
    pub on_disk: u64,
}

impl Group {
    fn add(&mut self, facts: &FileFacts) {
        self.files += 1;
        self.logical += facts.logical;
        self.on_disk += facts.on_disk;
    }

    /// How much less room these files take than their full size.
    pub fn saved(&self) -> u64 {
        self.logical.saturating_sub(self.on_disk)
    }
}

/// One line of the breakdown by how files are stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateGroup {
    /// For people, such as "Compressed with LZX".
    pub label: String,
    /// For scripts, such as "lzx".
    pub key: &'static str,
    pub group: Group,
}

/// What a selection takes up and what compressing it would do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Analysis {
    /// Every file, each counted once however many names it has.
    pub total: Group,
    /// By how they're stored, in a fixed order: not compressed, then each
    /// method, then what Scrimp leaves alone.
    pub by_state: Vec<StateGroup>,
    /// Files Scrimp Compress would change, and their full size.
    pub would_compress: Group,
    /// Why files would be left alone, as the summary's plain sentences.
    pub notes: Vec<String>,
}

/// Looks at `paths` as Scrimp Compress (Automatic) would, changing nothing.
/// `on_event` sees each file as it's looked at, for progress.
pub fn analyse<D: Disk>(
    paths: &[PathBuf],
    policy: &Policy,
    workers: &Workers,
    disk: &D,
    control: &Control,
    mut on_event: impl FnMut(&Event),
) -> Analysis {
    let job = Job {
        paths: paths.to_vec(),
        target: Target::Automatic,
    };
    let preview = Preview::new(disk);
    let mut duplicates = HashSet::new();
    let mut would_compress = Group::default();
    let report = run(&job, policy, workers, &preview, control, |e| {
        if let Event::File { path, outcome, .. } = e {
            match outcome {
                Outcome::Compressed { sizes, .. } => {
                    would_compress.files += 1;
                    would_compress.logical += sizes.logical;
                    would_compress.on_disk += sizes.before;
                }
                Outcome::Skipped(SkipReason::SameFileAlreadyDone) => {
                    duplicates.insert(path.clone());
                }
                _ => {}
            }
        }
        on_event(e);
    });

    let mut total = Group::default();
    let mut by_order: BTreeMap<usize, StateGroup> = BTreeMap::new();
    for (path, facts) in &preview.into_seen() {
        if duplicates.contains(path) {
            continue;
        }
        total.add(facts);
        let (order, label, key) = match &facts.state {
            CompressionState::Uncompressed => (0, "Not compressed".to_string(), "uncompressed"),
            CompressionState::Compressed(a) => (
                1 + Algorithm::ALL.iter().position(|x| x == a).unwrap_or(0),
                format!("Compressed with {}", a.technical_name()),
                a.cli_name(),
            ),
            _ => (
                Algorithm::ALL.len() + 1,
                "Left alone by Scrimp".to_string(),
                "leftAlone",
            ),
        };
        by_order
            .entry(order)
            .or_insert_with(|| StateGroup {
                label,
                key,
                group: Group::default(),
            })
            .group
            .add(facts);
    }
    Analysis {
        total,
        by_state: by_order.into_values().collect(),
        would_compress,
        notes: report.summary(None).into_iter().skip(1).collect(),
    }
}

fn files(n: u64) -> String {
    if n == 1 {
        "1 file".to_string()
    } else {
        format!("{} files", format_count(n))
    }
}

impl Analysis {
    /// What compressing would do, in one sentence.
    pub fn would_compress_line(&self) -> String {
        match self.would_compress.files {
            0 => "Nothing needs compressing.".into(),
            n => format!(
                "Scrimp would compress {}, {} in all. How much that saves depends on what's in them.",
                files(n),
                format_size(self.would_compress.logical)
            ),
        }
    }

    /// The whole analysis as `scrimp analyse` prints it, starting with a
    /// line naming the selection.
    pub fn lines(&self, name: &str) -> Vec<String> {
        let t = &self.total;
        let mut out = vec![format!(
            "{name}: {}, {}, taking {} on disk.",
            files(t.files),
            format_size(t.logical),
            format_size(t.on_disk)
        )];
        for s in &self.by_state {
            let g = &s.group;
            out.push(if g.on_disk == g.logical {
                format!("  {}: {}, {}.", s.label, files(g.files), format_size(g.logical))
            } else {
                format!(
                    "  {}: {}, {}, taking {} on disk.",
                    s.label,
                    files(g.files),
                    format_size(g.logical),
                    format_size(g.on_disk)
                )
            });
        }
        out.push(self.would_compress_line());
        out.extend(self.notes.iter().cloned());
        out
    }

    /// For **Scrimp Savings…**: a headline, and what's under it.
    pub fn savings(&self, name: &str) -> (String, Vec<String>) {
        let t = &self.total;
        let headline = if t.files == 0 {
            format!("{name} has no files Scrimp can look at")
        } else if t.saved() > 0 {
            format!("Compression saves {} in {name}", format_size(t.saved()))
        } else {
            format!("{name} isn't compressed")
        };
        let mut body = Vec::new();
        if t.files > 0 {
            body.push(if t.saved() > 0 {
                format!(
                    "It takes up {} instead of {}.",
                    format_size(t.on_disk),
                    format_size(t.logical)
                )
            } else {
                format!("It takes up {}.", format_size(t.on_disk))
            });
            if self.would_compress.files > 0 {
                body.push(self.would_compress_line());
            }
            body.extend(self.notes.iter().cloned());
        }
        (headline, body)
    }

    /// The breakdown by how files are stored, for "Show details".
    pub fn details(&self) -> String {
        self.by_state
            .iter()
            .map(|s| {
                let g = &s.group;
                format!(
                    "{}: {}, {}, taking {} on disk",
                    s.label,
                    files(g.files),
                    format_size(g.logical),
                    format_size(g.on_disk)
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}
