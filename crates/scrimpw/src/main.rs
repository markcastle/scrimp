//! `scrimpw.exe`: the worker the Explorer menu starts.
//!
//! The menu writes a job file and starts the worker. One worker runs at a
//! time for each user; it takes jobs from the jobs folder, oldest first,
//! shows Windows' own progress dialog while each runs, then a summary, and
//! exits when there are none left. A worker started while another is
//! running simply exits, and the running one picks up its job.
//!
//! It is a Windows program with no console window, so a right-click never
//! flashes a black box.
//!
//! ```text
//! scrimpw.exe [--job FILE]         take jobs (the file is a hint; every waiting job is taken)
//! scrimpw.exe --quiet              no windows: write the summary to the console, for tests
//! scrimpw.exe --jobs-folder DIR    use another jobs folder, for tests
//! scrimpw.exe --register           add Scrimp to Explorer's menu for this user
//! scrimpw.exe --unregister         take it out again
//! ```

#![cfg_attr(windows, windows_subsystem = "windows")]
#![cfg_attr(not(windows), allow(dead_code))]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

#[cfg(windows)]
mod worker;

use std::path::PathBuf;
use std::process::ExitCode;

/// What the worker was started with. Anything it doesn't know is ignored,
/// so an older menu can never stop a newer worker.
#[derive(Debug, Default, PartialEq, Eq)]
struct Args {
    quiet: bool,
    jobs_folder: Option<PathBuf>,
    register: bool,
    unregister: bool,
}

impl Args {
    fn parse(args: impl IntoIterator<Item = String>) -> Args {
        let mut out = Args::default();
        let mut args = args.into_iter();
        while let Some(a) = args.next() {
            match a.as_str() {
                "--quiet" => out.quiet = true,
                "--register" => out.register = true,
                "--unregister" => out.unregister = true,
                "--jobs-folder" => out.jobs_folder = args.next().map(PathBuf::from),
                // `--job FILE` names the job just written. Every waiting job
                // is taken anyway, so the name only needs skipping.
                "--job" => {
                    args.next();
                }
                _ => {}
            }
        }
        out
    }
}

fn main() -> ExitCode {
    let args = Args::parse(std::env::args().skip(1));
    #[cfg(windows)]
    {
        worker::main(&args)
    }
    #[cfg(not(windows))]
    {
        let _ = args;
        eprintln!("Scrimp only runs on Windows.");
        ExitCode::from(5)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Args {
        Args::parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn arguments_are_read_and_unknown_ones_ignored() {
        assert_eq!(parse(&[]), Args::default());
        assert_eq!(
            parse(&[
                "--job",
                r"C:\Users\You\AppData\Local\Scrimp\jobs\1.json",
                "--quiet",
                "--from-the-future"
            ]),
            Args {
                quiet: true,
                ..Args::default()
            }
        );
        assert_eq!(
            parse(&["--jobs-folder", r"D:\jobs"]).jobs_folder,
            Some(PathBuf::from(r"D:\jobs"))
        );
        assert_eq!(parse(&["--job"]), Args::default());
        assert!(parse(&["--register"]).register);
        assert!(parse(&["--unregister", "--quiet"]).unregister);
    }
}
