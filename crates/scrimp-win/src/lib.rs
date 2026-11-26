//! Safe wrappers over the Windows calls Scrimp needs: reading a file's
//! compression, compressing and decompressing it, and finding out what a
//! drive supports.
//!
//! Every decision in here comes from Phase 0, written up in
//! `docs/windows-api.md`. The most important ones:
//!
//! - WOF work always opens files read-only. Opening a WOF-compressed file
//!   for writing decompresses it.
//! - NTFS compression is removed before WOF is applied, because applying
//!   WOF on top leaves both in place.
//! - What a drive supports is decided from its filesystem name, never from
//!   error codes alone: ReFS answers WOF with "would not save space".
//! - A change is only reported as done once the file has been read back
//!   and is in the state that was asked for.
//!
//! This crate is one of two in the workspace allowed `unsafe` (the other is
//! the Explorer menu DLL), because it calls Windows directly. Every
//! `unsafe` block says why it is sound. Nothing outside the `ffi` module
//! touches a raw handle or pointer.
//!
//! On other systems the crate builds but is empty, so the workspace still
//! builds and `scrimp-core`'s tests still run there.

#![allow(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod error;

pub use error::Error;

#[cfg(windows)]
mod ffi;
#[cfg(windows)]
mod file;
#[cfg(windows)]
mod path;
#[cfg(windows)]
mod volume;

#[cfg(windows)]
pub use file::{FileInfo, compress, decompress, inspect};
#[cfg(windows)]
pub use volume::{DriveType, VolumeInfo, volume};
