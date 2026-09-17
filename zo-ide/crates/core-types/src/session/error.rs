//! Error type raised while loading, parsing, or saving sessions.

use std::fmt::{Display, Formatter};

use crate::json::JsonError;

/// Errors raised while loading, parsing, or saving sessions.
#[derive(Debug)]
pub enum SessionError {
    Io(std::io::Error),
    Json(JsonError),
    Format(String),
    /// A persistence-bound write was refused because the on-disk session file
    /// changed out from under this writer (another process holds the writer
    /// lease, or the file's fingerprint no longer matches what this `Session`
    /// last observed). Failing loudly here is what prevents a stale full
    /// snapshot from silently clobbering a peer's newer messages.
    Conflict(String),
}

impl Display for SessionError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::Json(error) => write!(f, "{error}"),
            Self::Format(error) | Self::Conflict(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for SessionError {}

impl From<std::io::Error> for SessionError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<JsonError> for SessionError {
    fn from(value: JsonError) -> Self {
        Self::Json(value)
    }
}
