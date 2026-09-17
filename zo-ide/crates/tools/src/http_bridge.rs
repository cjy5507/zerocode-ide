//! Sync → async bridge for HTTP calls made from synchronous tool handlers.
//!
//! Delegates to the canonical [`api::sync_bridge::run_blocking`]. This file
//! used to carry its own copy with a bare `Handle::block_on` and no
//! runtime-flavor check — the exact per-copy drift the shared bridge
//! eliminates (a direct `block_on` panics when the handler is reached from
//! an async execution context, and can stall on a `current_thread` ambient
//! runtime; see the `sync_bridge` module docs).

use std::future::Future;

use crate::ToolError;

/// Run an async HTTP future from synchronous code.
pub(crate) fn run_http<F, T>(future: F) -> Result<T, ToolError>
where
    F: Future<Output = Result<T, ToolError>>,
{
    api::sync_bridge::run_blocking(future)
}
