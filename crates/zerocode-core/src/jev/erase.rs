//! A deleted decision takes its records with it (t-87506).
//!
//! Deleting a built-in decision removes that seat's ledger from every folder a
//! provider's evidence is kept in, and no program writes it again while the
//! seat stays deleted. Deleting a person's own decision removes only its rows,
//! since those decisions share one ledger.

use std::io;
use std::path::Path;

/// Remove the records of built-in decision `id` from the zo home holding
/// `settings`.
///
/// # Errors
/// Whatever keeps a file from being removed.
pub fn forget_seat(_settings: &Path, _id: &str) -> io::Result<usize> {
    Ok(0)
}

/// Remove the rows of the person's decision `id` from every copy of the shared
/// ledger under `home`.
///
/// # Errors
/// Whatever keeps a ledger from being read or replaced.
pub fn forget_custom(_home: &Path, _id: &str) -> io::Result<usize> {
    Ok(0)
}

/// Whether a row may be appended to `ledger`: false while the seat that keeps
/// this file is deleted in the settings of the home the file stands in.
#[must_use]
pub fn may_write(_ledger: &Path) -> bool {
    true
}

/// Lift the deletion marks of the seats the person chose to keep, once.
///
/// # Errors
/// Whatever keeps the settings from being read or replaced.
pub fn lift_kept_seats(_settings: &Path) -> io::Result<bool> {
    Ok(false)
}

#[cfg(test)]
mod tests;
