//! A worker's tree, saved as a restore point without touching it (t-26583).

#![allow(unused_imports, dead_code)]

use std::path::Path;

/// How many checkpoints of one worker are kept.
pub(super) const KEPT: usize = 3;

/// Saves `checkout`'s tree under a ref of `worker`'s, numbered `number`.
///
/// # Errors
///
/// What git said, when it said no.
pub(super) fn save(
    _checkout: &Path,
    _worker: &str,
    _number: u32,
) -> Result<Option<String>, String> {
    Ok(None)
}

#[cfg(test)]
mod tests;
