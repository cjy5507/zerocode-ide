//! Human-facing formatting for session references and ages.
//!
//! A small, self-contained concern lifted out of `main.rs`: the strings shown
//! when a `--resume` reference cannot be found, when no managed sessions
//! exist, and the relative "age" label in session listings. The crate root
//! re-exports these so existing `crate::…` call sites are unchanged.
//!
//! The two "session not found" / "no managed sessions" hints are owned by
//! `runtime::session_control` (single source of truth) and re-exported here so
//! the CLI and the runtime never drift — previously both crates hand-rolled
//! the strings and disagreed about where sessions live. Only the relative-age
//! label, which has no runtime equivalent, is defined locally.


pub(crate) use runtime::session_control::{
    format_missing_session_reference, format_no_managed_sessions,
};

#[cfg(test)]
mod tests {

}
