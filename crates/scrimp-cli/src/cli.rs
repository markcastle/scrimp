//! The command line's shape: commands, options and their help text.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use scrimp_core::Target;

/// Save disk space with the compression built into Windows.
///
/// Your files stay exactly as they are, just smaller on disk, and apps open
/// them as normal. Folders are always processed in full, without following
/// links.
#[derive(Debug, Parser)]
#[command(name = "scrimp", version, arg_required_else_help = true)]
pub struct Cli {
    /// Use this settings file instead of %LOCALAPPDATA%\Scrimp\settings.json.
    #[arg(long, global = true, value_name = "FILE")]
    pub settings: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Compress files and folders.
    Compress(Compress),
    /// Remove compression from files and folders.
    Decompress(Change),
    /// Show how individual files are stored.
    Inspect(Look),
    /// Show how much space files and folders take, and what compressing them would do.
    #[command(alias = "analyze")]
    Analyse(Look),
    /// Update Scrimp to the newest release. Only ever contacts GitHub when you run it.
    Update(Update),
}

#[derive(Debug, Args)]
pub struct Update {
    /// Only say whether a newer version is available; change nothing.
    #[arg(long)]
    pub check: bool,
}

#[derive(Debug, Args)]
pub struct Compress {
    /// How to compress: auto, xpress4k, xpress8k, xpress16k, lzx or ntfs.
    /// Without it, the setting `defaultMethod` decides, which is auto unless changed.
    #[arg(short, long, value_name = "METHOD", value_parser = parse_target)]
    pub algorithm: Option<Target>,

    #[command(flatten)]
    pub change: Change,
}

#[derive(Debug, Args)]
pub struct Change {
    /// Files and folders to work on.
    #[arg(required = true, value_name = "PATH")]
    pub paths: Vec<PathBuf>,

    /// Show what would happen without changing anything.
    #[arg(long)]
    pub dry_run: bool,

    /// Write one JSON object per line, for scripts and other programs.
    #[arg(long)]
    pub json: bool,

    /// Work on this many files at once on every drive, instead of choosing
    /// by the kind of drive.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u16).range(1..=16))]
    pub workers: Option<u16>,
}

#[derive(Debug, Args)]
pub struct Look {
    /// Files and folders to look at.
    #[arg(required = true, value_name = "PATH")]
    pub paths: Vec<PathBuf>,

    /// Write one JSON object per line, for scripts and other programs.
    #[arg(long)]
    pub json: bool,
}

fn parse_target(s: &str) -> Result<Target, String> {
    s.parse::<Target>().map_err(|e| e.to_string())
}

/// What `scrimp` exits with. Documented in docs/command-line.md.
pub mod exit {
    /// Every file reached its target, or was left alone for a normal reason.
    pub const OK: i32 = 0;
    /// The job finished, but some files couldn't be changed.
    pub const SOME_FAILED: i32 = 1;
    /// The command line was wrong. clap exits with this itself, so only the
    /// tests name it.
    #[cfg_attr(not(test), allow(dead_code))]
    pub const USAGE: i32 = 2;
    /// Nothing could be done: every file is on a drive that can't compress.
    pub const NOTHING_POSSIBLE: i32 = 3;
    /// Stopped with Ctrl+C.
    pub const CANCELLED: i32 = 4;
    /// Something went wrong before any work began, such as a broken settings file.
    pub const FATAL: i32 = 5;
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    use scrimp_core::Algorithm;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("scrimp").chain(args.iter().copied()))
    }

    #[test]
    fn the_command_line_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn compress_takes_a_method_and_paths() {
        let cli = parse(&[
            "compress",
            "-a",
            "lzx",
            r"D:\Games",
            r"D:\Archive",
            "--json",
            "--workers",
            "1",
        ])
        .unwrap();
        let Command::Compress(c) = cli.command else {
            panic!("not compress")
        };
        assert_eq!(c.algorithm, Some(Target::Algorithm(Algorithm::Lzx)));
        assert_eq!(c.change.paths.len(), 2);
        assert!(c.change.json && !c.change.dry_run);
        assert_eq!(c.change.workers, Some(1));

        let cli = parse(&["compress", "--algorithm", "auto", "--dry-run", r"D:\Games"]).unwrap();
        let Command::Compress(c) = cli.command else {
            panic!("not compress")
        };
        assert_eq!(c.algorithm, Some(Target::Automatic));
        assert!(c.change.dry_run);
    }

    #[test]
    fn mistakes_are_refused_with_the_usage_code() {
        for bad in [
            &["compress"][..],
            &["compress", "-a", "zip", r"D:\Games"],
            &["compress", "--workers", "0", r"D:\Games"],
            &["compress", "--workers", "17", r"D:\Games"],
            &["inspect"],
            &["frobnicate"],
        ] {
            let err = parse(bad).unwrap_err();
            assert_eq!(err.exit_code(), exit::USAGE, "{bad:?}");
        }
        let err = parse(&["compress", "-a", "zip", r"D:\Games"]).unwrap_err().to_string();
        assert!(err.contains("isn't a compression method"), "{err}");
    }

    #[test]
    fn the_other_commands_parse() {
        assert!(matches!(
            parse(&["decompress", r"D:\Games"]).unwrap().command,
            Command::Decompress(_)
        ));
        assert!(matches!(
            parse(&["inspect", r"D:\Games\game.exe"]).unwrap().command,
            Command::Inspect(_)
        ));
        assert!(matches!(
            parse(&["analyse", r"D:\Games"]).unwrap().command,
            Command::Analyse(_)
        ));
        assert!(matches!(
            parse(&["analyze", r"D:\Games"]).unwrap().command,
            Command::Analyse(_)
        ));
        let cli = parse(&[
            "--settings",
            r"C:\Users\You\settings.json",
            "inspect",
            r"D:\Games\game.exe",
        ])
        .unwrap();
        assert!(cli.settings.is_some());
        let Command::Update(u) = parse(&["update", "--check"]).unwrap().command else {
            panic!("not update")
        };
        assert!(u.check);
        let Command::Update(u) = parse(&["update"]).unwrap().command else {
            panic!("not update")
        };
        assert!(!u.check);
    }
}
