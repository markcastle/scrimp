//! `scrimp.exe`: everything Scrimp does from Explorer, from the command line.
//!
//! For now it only knows its own version, which the Release workflow checks
//! before publishing. The commands (`compress`, `decompress`, `inspect` and
//! `analyse`) arrive in phase 3; see TASKS.md.

use clap::Parser;

/// Save disk space with the compression built into Windows.
#[derive(Parser)]
#[command(name = "scrimp", version, arg_required_else_help = true)]
struct Cli {}

fn main() {
    Cli::parse();
}
