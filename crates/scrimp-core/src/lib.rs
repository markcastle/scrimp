//! The rules behind Scrimp, with no Windows calls.
//!
//! Everything here is plain data and plain decisions: which compression
//! methods exist, what state a file can be in, and what happened when Scrimp
//! tried to change it. Keeping it free of Windows calls means it can be
//! tested on any computer, and it keeps the decisions in one place where the
//! menu, the worker and the command line all share them.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod algorithm;
mod outcome;
mod state;

pub use algorithm::{Algorithm, ParseError, Target};
pub use outcome::{FailReason, Outcome, Sizes, SkipReason};
pub use state::{CompressionState, UnsupportedReason};
