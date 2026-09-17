//! Shared byte cap for every MCP transport read.
//!
//! One JSON-RPC message (a stdio ndjson line, an HTTP/SSE body, or an in-progress
//! SSE event) may accumulate at most [`MAX_MCP_MESSAGE_BYTES`] before the read is
//! rejected. Keeping the policy in this transport-agnostic module lets stdio,
//! HTTP, and SSE all reuse it without stdio depending on the HTTP-common module
//! (and its `reqwest`/remote-transport machinery).

use std::io;

/// Maximum bytes a single MCP transport read is allowed to accumulate before it
/// is rejected: one stdio JSON-RPC line, one HTTP/SSE response body, or the
/// retained bytes of an in-progress SSE event. 32 MiB is far beyond any real
/// JSON-RPC message (tool lists, resource reads) yet bounds a server — remote or
/// local — that streams without ever terminating a frame, so it cannot exhaust
/// memory. This is the single cap every MCP transport shares via
/// [`guard_mcp_read_growth`], so the policy lives in one place.
pub(crate) const MAX_MCP_MESSAGE_BYTES: usize = 32 * 1024 * 1024;

/// Reject an MCP transport read whose retained buffer would exceed
/// [`MAX_MCP_MESSAGE_BYTES`] once `incoming` more bytes are appended to the
/// `retained` already held. `context` names the read site for the error message.
/// Shared by the stdio, HTTP, and SSE transports so the overflow policy is
/// defined once rather than re-derived per transport.
pub(crate) fn guard_mcp_read_growth(
    retained: usize,
    incoming: usize,
    context: &str,
) -> io::Result<()> {
    guard_mcp_read_growth_capped(retained, incoming, context, MAX_MCP_MESSAGE_BYTES)
}

/// The cap-parameterized core of [`guard_mcp_read_growth`]. Production always
/// passes [`MAX_MCP_MESSAGE_BYTES`] (via that wrapper), so the policy stays
/// single-sourced; a test can pass a tiny `cap` to exercise the rejection path
/// without allocating 32 MiB.
pub(crate) fn guard_mcp_read_growth_capped(
    retained: usize,
    incoming: usize,
    context: &str,
    cap: usize,
) -> io::Result<()> {
    if incoming > cap.saturating_sub(retained) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("MCP {context} exceeded the {cap}-byte limit without a complete message"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_growth_guard_allows_normal_and_split_frames() {
        // A normal message and one split across chunks both stay under the cap.
        assert!(guard_mcp_read_growth(0, 4096, "test").is_ok());
        let half = MAX_MCP_MESSAGE_BYTES / 2;
        assert!(guard_mcp_read_growth(half, half, "test").is_ok());
    }

    #[test]
    fn read_growth_guard_rejects_one_byte_over_cap() {
        let error = guard_mcp_read_growth(0, MAX_MCP_MESSAGE_BYTES + 1, "test")
            .expect_err("a read past the cap must be rejected");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        // An already-full buffer rejects even a single further byte.
        let error = guard_mcp_read_growth(MAX_MCP_MESSAGE_BYTES, 1, "test")
            .expect_err("appending past a full buffer must be rejected");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}
