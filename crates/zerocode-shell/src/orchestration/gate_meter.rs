//! A worker's model-call costs, read as its CLI's transcript grows (t-26583).

#![allow(unused_imports, dead_code)]

use std::path::{Path, PathBuf};

use serde::Serialize;
use zerocode_core::continue_gate::spend::CallCost;

/// How often one transcript is looked at.
pub(super) const POLL_MS: i64 = 3_000;

/// How much of a transcript's end a worker first seen mid-run is read from.
pub(super) const ATTACH_TAIL_BYTES: u64 = 1024 * 1024;

/// The most one look reads.
pub(super) const READ_MAX_BYTES: u64 = 2 * 1024 * 1024;

/// Why a worker's cost is, or is not, a number.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CostNote {
    /// Read from the CLI's transcript.
    Read,
    /// This CLI's transcript format has no reader yet.
    #[default]
    NoReader,
    /// The CLI has not said where it writes its conversation.
    NoTranscript,
    /// The transcript could not be opened.
    Unreadable,
}

/// What a worker's cost is read through.
pub(super) struct Meter;

impl Meter {
    pub(super) fn new(_agent: &str) -> Self {
        Self
    }

    /// One look, when it is time.
    pub(super) fn poll(&mut self, _path: Option<&str>, _now_ms: i64) -> Vec<CallCost> {
        Vec::new()
    }

    /// Why the cost is, or is not, a number.
    pub(super) fn note(&self, _path_known: bool) -> CostNote {
        CostNote::NoReader
    }
}

#[cfg(test)]
mod tests;
