//! `retrieve_tool_output` — recover the full original of a truncated tool
//! output from the Phase-4 artifact store.
//!
//! When a tool result exceeds its truncation limit, the dispatch pipeline
//! preserves the untruncated content content-addressed under
//! `.zo/artifacts/<sha256>` and appends a notice naming that hash. This
//! tool is the read side of that contract (the CCR "retrieve" half): the
//! model passes the hash back — optionally with a 0-based `offset`/`limit`
//! line window, same semantics as `read_file` — and gets the original bytes.

use std::fmt::Write as _;

use serde::Deserialize;

use super::ToolError;
use crate::artifacts::read_artifact;

#[derive(Debug, Deserialize)]
pub(crate) struct RetrieveToolOutputInput {
    /// Content address from the truncation notice (64 lowercase hex chars).
    pub sha256: String,
    /// 0-based first line of the window (same semantics as `read_file`).
    pub offset: Option<usize>,
    /// Maximum number of lines to return.
    pub limit: Option<usize>,
}

pub(crate) fn run_retrieve_tool_output(
    input: &RetrieveToolOutputInput,
) -> Result<String, ToolError> {
    retrieve_with_reader(input, read_artifact)
}

/// Core logic with the artifact reader injected, so tests can supply an
/// in-memory store instead of mutating the process-global env/cwd.
fn retrieve_with_reader(
    input: &RetrieveToolOutputInput,
    read: impl Fn(&str) -> Option<String>,
) -> Result<String, ToolError> {
    let sha = input.sha256.trim();
    // The hash becomes a file name inside the store — accept only an exact
    // SHA-256 hex string so a crafted "hash" can never traverse out of the
    // artifact directory.
    if sha.len() != 64 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(ToolError::InvalidInput(
            "sha256 must be the 64-char hex id from the truncation notice".to_owned(),
        ));
    }
    let sha = sha.to_ascii_lowercase();
    let Some(content) = read(&sha) else {
        return Err(ToolError::Execution(format!(
            "no stored artifact for sha256={sha} — it may belong to another \
             workspace or predate the artifact store"
        )));
    };

    let is_structured = matches!(content.trim_start().as_bytes().first(), Some(b'{' | b'['));
    let mut out = String::with_capacity(content.len().min(64 * 1024) + 128);
    if is_structured && (input.offset.is_some() || input.limit.is_some()) {
        let _ = writeln!(
            out,
            "[artifact {}…] structured output — offset/limit ignored",
            &sha[..12],
        );
        out.push_str(&content);
        return Ok(out);
    }

    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();
    let start = input.offset.unwrap_or(0).min(total);
    let end = input
        .limit
        .map_or(total, |limit| start.saturating_add(limit).min(total));

    let _ = writeln!(
        out,
        "[artifact {}…] lines {}-{} of {total}",
        &sha[..12],
        start + 1,
        end.max(start + 1).min(total.max(1))
    );
    out.push_str(&lines[start..end].join("\n"));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_hex_hashes() {
        for bad in ["", "abc", "../escape", &"z".repeat(64), &"a".repeat(63)] {
            let result = run_retrieve_tool_output(&RetrieveToolOutputInput {
                sha256: (*bad).to_string(),
                offset: None,
                limit: None,
            });
            assert!(result.is_err(), "{bad:?} must be rejected");
        }
    }

    /// In-memory single-artifact store: full disk round-trips live in
    /// `crate::artifacts::tests`; here we exercise validation + windowing.
    fn reader_for(sha: &str, content: &str) -> impl Fn(&str) -> Option<String> {
        let sha = sha.to_string();
        let content = content.to_string();
        move |requested: &str| (requested == sha).then(|| content.clone())
    }

    #[test]
    fn retrieves_full_content_with_header() {
        let sha = "a".repeat(64);
        let content = (0..200)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let full = retrieve_with_reader(
            &RetrieveToolOutputInput {
                sha256: sha.clone(),
                offset: None,
                limit: None,
            },
            reader_for(&sha, &content),
        )
        .expect("retrieve");
        assert!(full.starts_with(&format!("[artifact {}…] lines 1-200 of 200", &sha[..12])));
        assert!(full.contains("line 0"));
        assert!(full.ends_with("line 199"));
    }

    #[test]
    fn windows_with_read_file_offset_semantics() {
        let sha = "b".repeat(64);
        let content = (0..200)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let window = retrieve_with_reader(
            &RetrieveToolOutputInput {
                sha256: sha.clone(),
                offset: Some(50),
                limit: Some(2),
            },
            reader_for(&sha, &content),
        )
        .expect("retrieve window");
        // 0-based offset → 1-based display, exactly `limit` lines.
        assert!(window.contains("lines 51-52 of 200"));
        assert!(window.contains("line 50"));
        assert!(window.contains("line 51"));
        assert!(!window.contains("line 52"), "window must stop at the limit");
    }

    #[test]
    fn out_of_range_window_degrades_gracefully() {
        let sha = "c".repeat(64);
        let result = retrieve_with_reader(
            &RetrieveToolOutputInput {
                sha256: sha.clone(),
                offset: Some(10_000),
                limit: Some(5),
            },
            reader_for(&sha, "only\ntwo"),
        )
        .expect("clamped window");
        assert!(result.contains("of 2"));
    }

    #[test]
    fn uppercase_hex_is_accepted_and_normalized() {
        let sha = "d".repeat(64);
        let result = retrieve_with_reader(
            &RetrieveToolOutputInput {
                sha256: sha.to_ascii_uppercase(),
                offset: None,
                limit: None,
            },
            reader_for(&sha, "content"),
        );
        assert!(
            result.is_ok(),
            "uppercase hex must normalize to the stored id"
        );
    }

    #[test]
    fn structured_artifact_ignores_line_window() {
        let sha = "e".repeat(64);
        let content = r#"{"lines":["a","b","c"]}"#;
        let result = retrieve_with_reader(
            &RetrieveToolOutputInput {
                sha256: sha.clone(),
                offset: Some(1),
                limit: Some(1),
            },
            reader_for(&sha, content),
        )
        .expect("retrieve structured");
        assert!(result.contains("structured output — offset/limit ignored"));
        assert!(result.contains(content));
    }

    #[test]
    fn missing_artifact_is_a_clear_error() {
        let result = retrieve_with_reader(
            &RetrieveToolOutputInput {
                sha256: "a".repeat(64),
                offset: None,
                limit: None,
            },
            |_| None,
        );
        let error = result.expect_err("absent artifact");
        assert!(error.to_string().contains("no stored artifact"));
    }
}
