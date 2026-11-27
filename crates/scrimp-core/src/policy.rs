//! What to do with each file: the automatic policy and the rules that
//! protect files Scrimp must never touch.
//!
//! Rules apply in order and the first match wins (docs/plan.md section 6):
//!
//! 1. **Never touch** Windows' own files, protected folders, links, cloud
//!    files, encrypted files and files smaller than the minimum. This rule
//!    applies whatever was asked for.
//! 2. **Files that change often** are skipped, because writing to a
//!    WOF-compressed file undoes the compression. A setting can use NTFS
//!    compression for them instead, which survives writes.
//! 3. **Already compressed** photos, videos and archives are skipped.
//! 4. **Text and source code** get LZX, the smallest.
//! 5. **Programs and libraries** get XPRESS16K.
//! 6. **Everything else** gets XPRESS8K, but only if a sample of it shrinks.
//!    A file of a kind Scrimp doesn't know may well be compressed already
//!    (game archives, disk images, encrypted containers), and compressing
//!    a large one only to find it didn't shrink wastes minutes. So files of
//!    1 MB or more are sampled first ([`Plan::Sample`]).
//!
//! Rules 2 to 6 only apply in automatic mode. Choosing a method yourself
//! always wins over them, but never over rule 1.
//!
//! Paths are compared as text, splitting on both kinds of slash, so these
//! rules behave the same when tested on any operating system.

use std::collections::HashMap;
use std::path::Path;

use crate::algorithm::{Algorithm, Target};
use crate::outcome::SkipReason;
use crate::state::CompressionState;

/// What to do with one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// Compress it with this method.
    Compress(Algorithm),
    /// Compress it with this method if a sample of it shrinks: automatic
    /// mode's choice for a large file whose kind gives no clue.
    Sample(Algorithm),
    /// Remove its compression.
    Decompress,
    /// It is already as asked; leave it.
    AlreadyDone,
    /// Leave it alone, for this reason.
    Skip(SkipReason),
}

/// What automatic mode does with one kind of file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// Compress with this method.
    Use(Algorithm),
    /// Leave it uncompressed.
    Skip,
}

/// What automatic mode does with files that change often.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChangesOften {
    /// Leave them alone (the default).
    #[default]
    Skip,
    /// Use NTFS compression, which stays in place when they change.
    UseNtfs,
}

/// Files that change often, so WOF compression would soon be undone.
const CHANGES_OFTEN: &[&str] = &[
    "vhd", "vhdx", "avhdx", "vmdk", "vdi", "qcow2", "pst", "ost", "db", "sqlite", "sqlite3", "db-wal", "db-shm", "mdf",
    "ldf", "ndf", "edb", "log", "etl",
];

/// Formats that are already compressed, so compressing them again saves
/// little or nothing.
const ALREADY_COMPRESSED: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "webp", "avif", "heic", "heif", "jxl", "mp3", "aac", "m4a", "ogg", "opus", "flac",
    "mp4", "m4v", "mkv", "mov", "webm", "avi", "wmv", "zip", "7z", "rar", "gz", "tgz", "bz2", "xz", "zst", "lz4",
    "cab", "msi", "msix", "appx", "docx", "xlsx", "pptx", "odt", "ods", "odp", "epub", "jar", "apk",
];

/// Text, source code and uncompressed images, which LZX shrinks the most.
const TEXT: &[&str] = &[
    "txt", "csv", "tsv", "json", "xml", "yaml", "yml", "toml", "ini", "cfg", "conf", "md", "html", "htm", "css", "js",
    "mjs", "ts", "tsx", "jsx", "rs", "c", "h", "cpp", "hpp", "cc", "cs", "java", "kt", "py", "rb", "go", "php", "sql",
    "sh", "ps1", "bat", "svg", "bmp", "tif", "tiff",
];

/// Programs and libraries.
const PROGRAMS: &[&str] = &["exe", "dll", "sys", "ocx", "pdb", "lib", "obj", "node"];

/// Files of an unknown kind this size or larger are sampled before they're
/// compressed. Below it, compressing is quick enough that a sample saves
/// nothing: Windows itself refuses a file that wouldn't shrink.
pub const SAMPLE_FROM: u64 = 1024 * 1024;

/// File names at a drive's root that belong to Windows.
const WINDOWS_FILES: &[&str] = &["pagefile.sys", "hiberfil.sys", "swapfile.sys"];

/// Folder names that belong to Windows wherever they are.
const WINDOWS_FOLDERS: &[&str] = &["system volume information", "$recycle.bin"];

/// The rules for one job.
#[derive(Debug, Clone)]
pub struct Policy {
    /// Files smaller than this are never compressed.
    pub minimum_size: u64,
    /// What automatic mode does with files that change often.
    pub changes_often: ChangesOften,
    overrides: HashMap<String, Choice>,
    protected: Vec<Vec<String>>,
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            minimum_size: 8192,
            changes_often: ChangesOften::default(),
            overrides: HashMap::new(),
            protected: Vec::new(),
        }
    }
}

impl Policy {
    /// The default rules, protecting Windows' folders on this computer as
    /// named by the `SystemRoot`, `ProgramFiles` and `ProgramData`
    /// environment variables.
    pub fn for_this_computer() -> Self {
        let var = |name| std::env::var_os(name).map(|v| Path::new(&v).to_path_buf());
        let mut roots = Vec::new();
        roots.extend(var("SystemRoot"));
        roots.extend(var("ProgramFiles").map(|p| p.join("WindowsApps")));
        roots.extend(var("ProgramData").map(|p| p.join("Microsoft")));
        Policy::default().protecting(roots)
    }

    /// Adds folders that are never touched, nor anything inside them.
    pub fn protecting(mut self, folders: impl IntoIterator<Item = impl AsRef<Path>>) -> Self {
        self.protected.extend(folders.into_iter().map(|f| parts(f.as_ref())));
        self
    }

    /// Makes automatic mode treat files with this extension (without the
    /// dot, any case) this way, whatever the built-in rules say.
    pub fn with_override(mut self, extension: &str, choice: Choice) -> Self {
        self.overrides.insert(normalise_extension(extension), choice);
        self
    }

    /// Whether a path is somewhere Scrimp never touches.
    pub fn is_protected(&self, path: &Path) -> bool {
        let p = parts(path);
        let in_windows_folder = p[..p.len().saturating_sub(1)]
            .iter()
            .any(|part| WINDOWS_FOLDERS.contains(&part.as_str()));
        let windows_file = p.len() == 2 && WINDOWS_FILES.contains(&p[1].as_str());
        in_windows_folder || windows_file || self.protected.iter().any(|root| p.starts_with(root))
    }

    /// What to do with one file, given what was asked for, the file's
    /// current state and its real size.
    pub fn plan(&self, path: &Path, state: &CompressionState, logical: u64, target: Target) -> Plan {
        // Rule 1: never touch, whatever was asked for.
        match state {
            CompressionState::WimBacked => return Plan::Skip(SkipReason::WimBacked),
            CompressionState::Unknown => return Plan::Skip(SkipReason::StateUnknown),
            CompressionState::Unsupported(reason) => return Plan::Skip(SkipReason::CantCompressHere(reason.clone())),
            CompressionState::Uncompressed | CompressionState::Compressed(_) => {}
        }
        if self.is_protected(path) {
            return Plan::Skip(SkipReason::ProtectedLocation);
        }

        let algorithm = match target {
            Target::Uncompressed => {
                return if state.is_compressed() {
                    Plan::Decompress
                } else {
                    Plan::AlreadyDone
                };
            }
            Target::Algorithm(a) => (a, false),
            // Automatic mode leaves a file that is already compressed as it
            // is, rather than spending time recompressing it.
            Target::Automatic if state.is_compressed() => return Plan::AlreadyDone,
            Target::Automatic => match self.choose(path) {
                Ok(chosen) => chosen,
                Err(reason) => return Plan::Skip(reason),
            },
        };
        let (algorithm, unknown_kind) = algorithm;
        if logical < self.minimum_size {
            return Plan::Skip(SkipReason::TooSmall);
        }
        if state.already_is(Target::Algorithm(algorithm)) {
            return Plan::AlreadyDone;
        }
        if unknown_kind && logical >= SAMPLE_FROM {
            return Plan::Sample(algorithm);
        }
        Plan::Compress(algorithm)
    }

    /// Rules 2 to 6: the method automatic mode picks for a file's type, and
    /// whether that was rule 6, for a kind of file it doesn't know.
    fn choose(&self, path: &Path) -> Result<(Algorithm, bool), SkipReason> {
        let extension = extension(path);
        let is = |list: &[&str]| list.contains(&extension.as_str());
        if let Some(choice) = self.overrides.get(&extension) {
            return match choice {
                Choice::Use(a) => Ok((*a, false)),
                Choice::Skip => Err(SkipReason::AlreadyCompressedFormat),
            };
        }
        if is(CHANGES_OFTEN) {
            return match self.changes_often {
                ChangesOften::Skip => Err(SkipReason::ChangesOften),
                ChangesOften::UseNtfs => Ok((Algorithm::Lznt1, false)),
            };
        }
        if is(ALREADY_COMPRESSED) {
            return Err(SkipReason::AlreadyCompressedFormat);
        }
        if is(TEXT) {
            return Ok((Algorithm::Lzx, false));
        }
        if is(PROGRAMS) {
            return Ok((Algorithm::Xpress16k, false));
        }
        Ok((Algorithm::Xpress8k, true))
    }
}

/// A path's parts in lower case, split on either slash, without a `\\?\`
/// prefix or empty parts.
fn parts(path: &Path) -> Vec<String> {
    let text = path.to_string_lossy().to_lowercase();
    let text = text.strip_prefix(r"\\?\").unwrap_or(&text);
    text.split(['\\', '/'])
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect()
}

/// A file's extension in lower case, without the dot. Empty when it has none.
fn extension(path: &Path) -> String {
    let name = parts(path).pop().unwrap_or_default();
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => ext.to_string(),
        _ => String::new(),
    }
}

fn normalise_extension(extension: &str) -> String {
    extension.trim().trim_start_matches('.').to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::UnsupportedReason;

    const BIG: u64 = 1_000_000;

    fn auto(policy: &Policy, path: &str) -> Plan {
        policy.plan(Path::new(path), &CompressionState::Uncompressed, BIG, Target::Automatic)
    }

    #[test]
    fn automatic_mode_picks_by_kind_of_file() {
        let p = Policy::default();
        assert_eq!(auto(&p, r"D:\Archive\Report-2019.csv"), Plan::Compress(Algorithm::Lzx));
        assert_eq!(auto(&p, r"D:\Archive\Notes.TXT"), Plan::Compress(Algorithm::Lzx));
        assert_eq!(
            auto(&p, r"D:\Games\Example\game.exe"),
            Plan::Compress(Algorithm::Xpress16k)
        );
        assert_eq!(
            auto(&p, r"D:\Games\Example\engine.dll"),
            Plan::Compress(Algorithm::Xpress16k)
        );
        assert_eq!(
            auto(&p, r"D:\Games\Example\Example.dat"),
            Plan::Compress(Algorithm::Xpress8k)
        );
        assert_eq!(
            auto(&p, r"D:\Games\Example\README"),
            Plan::Compress(Algorithm::Xpress8k)
        );
    }

    #[test]
    fn already_compressed_formats_and_files_that_change_are_skipped() {
        let p = Policy::default();
        for path in [
            r"D:\Videos\Holiday.mp4",
            r"D:\Photos\IMG_0048.JPG",
            r"D:\Archive\Old.zip",
        ] {
            assert_eq!(
                auto(&p, path),
                Plan::Skip(SkipReason::AlreadyCompressedFormat),
                "{path}"
            );
        }
        for path in [r"D:\VMs\Example.vhdx", r"C:\Users\You\Mail.pst", r"D:\App\data.sqlite"] {
            assert_eq!(auto(&p, path), Plan::Skip(SkipReason::ChangesOften), "{path}");
        }
    }

    #[test]
    fn files_that_change_can_use_ntfs_compression_instead() {
        let p = Policy {
            changes_often: ChangesOften::UseNtfs,
            ..Policy::default()
        };
        assert_eq!(auto(&p, r"D:\VMs\Example.vhdx"), Plan::Compress(Algorithm::Lznt1));
    }

    #[test]
    fn overrides_beat_the_built_in_rules() {
        let p = Policy::default()
            .with_override(".BAK", Choice::Use(Algorithm::Lzx))
            .with_override("mp4", Choice::Use(Algorithm::Xpress4k))
            .with_override("dat", Choice::Skip);
        assert_eq!(auto(&p, r"D:\Archive\Old.bak"), Plan::Compress(Algorithm::Lzx));
        assert_eq!(auto(&p, r"D:\Videos\Holiday.mp4"), Plan::Compress(Algorithm::Xpress4k));
        assert_eq!(
            auto(&p, r"D:\Games\Example.dat"),
            Plan::Skip(SkipReason::AlreadyCompressedFormat)
        );
    }

    #[test]
    fn choosing_a_method_beats_the_kind_of_file_but_not_rule_one() {
        let p = Policy::for_this_computer().protecting([r"C:\Windows"]);
        let lzx = Target::Algorithm(Algorithm::Lzx);
        let video = Path::new(r"D:\Videos\Holiday.mp4");
        assert_eq!(
            p.plan(video, &CompressionState::Uncompressed, BIG, lzx),
            Plan::Compress(Algorithm::Lzx)
        );
        assert_eq!(
            p.plan(
                Path::new(r"C:\Windows\notepad.exe"),
                &CompressionState::Uncompressed,
                BIG,
                lzx
            ),
            Plan::Skip(SkipReason::ProtectedLocation)
        );
        assert_eq!(
            p.plan(video, &CompressionState::Uncompressed, 100, lzx),
            Plan::Skip(SkipReason::TooSmall)
        );
        assert_eq!(
            p.plan(video, &CompressionState::WimBacked, BIG, lzx),
            Plan::Skip(SkipReason::WimBacked)
        );
        assert_eq!(
            p.plan(video, &CompressionState::Unknown, BIG, lzx),
            Plan::Skip(SkipReason::StateUnknown)
        );
        let link = CompressionState::Unsupported(UnsupportedReason::Link);
        assert_eq!(
            p.plan(video, &link, BIG, Target::Uncompressed),
            Plan::Skip(SkipReason::CantCompressHere(UnsupportedReason::Link))
        );
    }

    #[test]
    fn already_done_is_recognised() {
        let p = Policy::default();
        let path = Path::new(r"D:\Archive\Report-2019.csv");
        let lzx = CompressionState::Compressed(Algorithm::Lzx);
        assert_eq!(
            p.plan(path, &lzx, BIG, Target::Algorithm(Algorithm::Lzx)),
            Plan::AlreadyDone
        );
        assert_eq!(
            p.plan(path, &lzx, BIG, Target::Algorithm(Algorithm::Xpress4k)),
            Plan::Compress(Algorithm::Xpress4k)
        );
        assert_eq!(
            p.plan(path, &lzx, BIG, Target::Automatic),
            Plan::AlreadyDone,
            "automatic leaves it be"
        );
        assert_eq!(p.plan(path, &lzx, BIG, Target::Uncompressed), Plan::Decompress);
        assert_eq!(
            p.plan(path, &CompressionState::Uncompressed, BIG, Target::Uncompressed),
            Plan::AlreadyDone
        );
        // Small files are decompressed too: the minimum is about compressing.
        assert_eq!(p.plan(path, &lzx, 10, Target::Uncompressed), Plan::Decompress);
    }

    #[test]
    fn windows_own_places_are_protected_wherever_they_are() {
        let p = Policy::default().protecting([r"C:\Windows", r"C:\Program Files\WindowsApps"]);
        for path in [
            r"C:\Windows\System32\ntdll.dll",
            r"c:\WINDOWS\explorer.exe",
            r"\\?\C:\Windows\System32\drivers\etc\hosts",
            r"C:\Program Files\WindowsApps\Example\app.exe",
            r"D:\System Volume Information\tracking.log",
            r"E:\$Recycle.Bin\S-1-5-21\Example.dat",
            r"C:\pagefile.sys",
            r"C:/Windows/notepad.exe",
        ] {
            assert!(p.is_protected(Path::new(path)), "{path}");
        }
        for path in [
            r"C:\Windows.old\Example.dat",
            r"C:\Users\You\Windows\notes.txt",
            r"D:\Games\pagefile.sys",
            r"C:\Users\You\Documents\Report-2019.docx",
        ] {
            assert!(!p.is_protected(Path::new(path)), "{path}");
        }
    }

    #[test]
    fn this_computers_windows_folder_is_protected() {
        let p = Policy::for_this_computer();
        if let Some(root) = std::env::var_os("SystemRoot") {
            assert!(p.is_protected(&Path::new(&root).join("notepad.exe")));
        }
        assert_eq!(p.minimum_size, 8192);
    }

    #[test]
    fn extensions_are_read_from_the_name_alone() {
        assert_eq!(extension(Path::new(r"D:\Archive.v2\Report")), "");
        assert_eq!(extension(Path::new(r"D:\Archive\.gitignore")), "");
        assert_eq!(extension(Path::new(r"D:\Archive\Old.Tar.GZ")), "gz");
        assert_eq!(normalise_extension(" .BAK "), "bak");
    }

    #[test]
    fn large_files_of_an_unknown_kind_are_sampled_first() {
        let p = Policy::default();
        let plan = |path: &str, size| {
            p.plan(
                Path::new(path),
                &CompressionState::Uncompressed,
                size,
                Target::Automatic,
            )
        };
        let big = 4 * SAMPLE_FROM;
        assert_eq!(
            plan(r"D:\Games\Example\Example.pak", big),
            Plan::Sample(Algorithm::Xpress8k)
        );
        assert_eq!(plan(r"D:\Games\Example\README", big), Plan::Sample(Algorithm::Xpress8k));
        assert_eq!(
            plan(r"D:\Games\Example\Example.pak", SAMPLE_FROM - 1),
            Plan::Compress(Algorithm::Xpress8k),
            "small enough that Windows' own check is quicker"
        );
        // Kinds Scrimp knows aren't sampled: it already knows they shrink.
        assert_eq!(
            plan(r"D:\Games\Example\game.exe", big),
            Plan::Compress(Algorithm::Xpress16k)
        );
        assert_eq!(plan(r"D:\Archive\notes.txt", big), Plan::Compress(Algorithm::Lzx));
        // A method someone chose is never second-guessed.
        assert_eq!(
            p.plan(
                Path::new(r"D:\Games\Example\Example.pak"),
                &CompressionState::Uncompressed,
                big,
                Target::Algorithm(Algorithm::Lzx)
            ),
            Plan::Compress(Algorithm::Lzx)
        );
        // Nor is a method set for an extension in settings.
        let chosen = Policy::default().with_override("pak", Choice::Use(Algorithm::Lzx));
        assert_eq!(
            chosen.plan(
                Path::new(r"D:\Games\Example\Example.pak"),
                &CompressionState::Uncompressed,
                big,
                Target::Automatic
            ),
            Plan::Compress(Algorithm::Lzx)
        );
    }
}
