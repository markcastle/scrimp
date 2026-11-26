//! `scrimp_shell.dll`: Scrimp Compress and Scrimp Decompress in Explorer's
//! right-click menu.
//!
//! This runs inside Explorer, so it follows strict rules
//! (docs/plan.md section 3, "Shell extension rules"):
//!
//! - It never compresses anything. Choosing an item writes a job file and
//!   starts `scrimpw.exe`, which does the work in its own process.
//! - It never panics into Explorer. Every exported function and every COM
//!   method catches panics and returns an error instead.
//! - When Explorer says it can't wait, it only looks at the paths' text.
//!   The checks that touch the disk happen when Explorer asks again on a
//!   background thread.
//! - It never walks into folders and never touches the network.
//!
//! The DLL implements Windows' `IExplorerCommand`, which both the Windows 10
//! menu and Windows 11's menus use, so one implementation serves them all.
//! It's registered by `scrimpw.exe --register` (see `scrimp_win::menu`).
//!
//! One of two crates in the workspace allowed `unsafe`, for the COM exports
//! and calls. Every block says why it is sound.

#![allow(unsafe_code)]
#![cfg_attr(not(windows), allow(dead_code))]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

#[cfg(windows)]
mod command;
#[cfg(windows)]
mod exports;
#[cfg(windows)]
mod launch;
#[cfg(all(windows, test))]
mod tests;

#[cfg(windows)]
pub use exports::{DllCanUnloadNow, DllGetClassObject};
