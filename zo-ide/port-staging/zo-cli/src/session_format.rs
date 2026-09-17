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

use std::time::UNIX_EPOCH;

pub(crate) use runtime::session_control::{
    format_missing_session_reference, format_no_managed_sessions,
};

/// Compact relative-age label (`just-now`, `5s-ago`, `3m-ago`, …) for a
/// session's last-modified timestamp in epoch milliseconds.
pub(crate) fn format_session_modified_age(modified_epoch_millis: u128) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map_or(modified_epoch_millis, |duration| duration.as_millis());
    let delta_seconds = now
        .saturating_sub(modified_epoch_millis)
        .checked_div(1_000)
        .unwrap_or_default();
    match delta_seconds {
        0..=4 => "just-now".to_string(),
        5..=59 => format!("{delta_seconds}s-ago"),
        60..=3_599 => format!("{}m-ago", delta_seconds / 60),
        3_600..=86_399 => format!("{}h-ago", delta_seconds / 3_600),
        _ => format!("{}d-ago", delta_seconds / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::format_session_modified_age;

    #[test]
    fn formats_relative_session_age_buckets() {
        // `now` is far in the future relative to these fixed timestamps, so
        // each bucket is exercised deterministically against the live clock.
        let now_ms = std::time::SystemTime::now()
            .duration_since(super::UNIX_EPOCH)
            .expect("after epoch")
            .as_millis();
        assert_eq!(format_session_modified_age(now_ms), "just-now");
        assert!(format_session_modified_age(now_ms.saturating_sub(10_000)).ends_with("s-ago"));
        assert!(format_session_modified_age(now_ms.saturating_sub(120_000)).ends_with("m-ago"));
        assert!(format_session_modified_age(now_ms.saturating_sub(7_200_000)).ends_with("h-ago"));
        assert!(format_session_modified_age(now_ms.saturating_sub(172_800_000)).ends_with("d-ago"));
    }
}
