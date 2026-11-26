//! `scrimp.exe`: everything Scrimp does from Explorer, from the command line.
//!
//! `compress`, `decompress`, `inspect` and `analyse`, with `--json` output
//! for scripts and documented exit codes. See docs/command-line.md.
//!
//! The command line's shape and words are plain Rust and tested anywhere;
//! running the commands needs Windows.

#![cfg_attr(not(windows), allow(dead_code))]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod cli;
#[cfg(windows)]
mod commands;
mod json;
mod words;

use std::process::ExitCode;

use clap::Parser;

/// Returns the exit code rather than calling `std::process::exit`, so the
/// program shuts down normally: buffered output is written, and so is the
/// coverage data CI measures.
fn main() -> ExitCode {
    let cli = cli::Cli::parse();
    ExitCode::from(u8::try_from(run(cli)).unwrap_or(1))
}

#[cfg(windows)]
fn run(cli: cli::Cli) -> i32 {
    commands::run(cli)
}

#[cfg(not(windows))]
fn run(_cli: cli::Cli) -> i32 {
    eprintln!("Scrimp only runs on Windows.");
    cli::exit::FATAL
}
