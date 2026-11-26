//! The rules behind Scrimp, with no Windows calls.
//!
//! Everything here is plain data and plain decisions: which compression
//! methods exist, what state a file can be in, and what happened when Scrimp
//! tried to change it. Keeping it free of Windows calls means it can be
//! tested on any computer, and it keeps the decisions in one place where the
//! menu, the worker and the command line all share them.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod algorithm;
mod engine;
mod outcome;
mod policy;
mod report;
mod settings;
mod space;
mod state;

pub use algorithm::{Algorithm, ParseError, Target};
pub use engine::{
    Control, Disk, DiskError, EntryKind, Event, FileFacts, FileId, Job, Storage, VolumeFacts, Workers, run,
};
pub use outcome::{FailReason, Outcome, Sizes, SkipReason};
pub use policy::{ChangesOften, Choice, Plan, Policy};
pub use report::{Mode, Report, format_count, format_size};
pub use settings::{SCHEMA_VERSION, Settings, SettingsError};
pub use space::{Shortfall, check_room, growth, reserve};
pub use state::{CompressionState, UnsupportedReason};
