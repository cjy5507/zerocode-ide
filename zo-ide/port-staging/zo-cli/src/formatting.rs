use std::fmt::{Display, Write as _};

use runtime::TokenUsage;

use crate::{LATEST_SESSION_REFERENCE, PRIMARY_SESSION_EXTENSION};

pub(crate) const REPORT_LABEL_FIELD: usize = 17;

pub(crate) fn report_row(label: &str, value: impl Display) -> String {
    format!("  {label:<REPORT_LABEL_FIELD$}{value}")
}

pub(crate) fn format_model_delegation_rows() -> String {
    [
        report_row(
            "Delegation",
            "smart-routed per role (this pin binds the main turn only)",
        ),
        report_row("Override", "/smart off or /smart pin <role> <model>"),
    ]
    .join("\n")
}

#[cfg(test)]
pub(crate) fn format_unknown_slash_command_message(name: &str) -> String {
    let suggestions = crate::cli_args::suggest_slash_commands(name);
    if suggestions.is_empty() {
        format!("unknown slash command: /{name}. Use /help to list available commands.")
    } else {
        format!(
            "unknown slash command: /{name}. Did you mean {}? Use /help to list available commands.",
            suggestions.join(", ")
        )
    }
}

pub(crate) fn format_model_report(model: &str, message_count: usize, turns: u32) -> String {
    [
        "Model".to_string(),
        report_row("Current model", model),
        report_row("Session messages", message_count),
        report_row("Session turns", turns),
        String::new(),
        "Usage".to_string(),
        "  Inspect current model with /model".to_string(),
        "  Switch models with /model <name>".to_string(),
    ]
    .join("\n")
}

pub(crate) fn format_model_switch_report(
    previous: &str,
    next: &str,
    message_count: usize,
) -> String {
    [
        "Model updated".to_string(),
        report_row("Previous", previous),
        report_row("Current", next),
        report_row("Preserved msgs", message_count),
    ]
    .join("\n")
}

pub(crate) fn format_permissions_report(mode: &str) -> String {
    let modes = [
        ("read-only", "Read/search tools only", mode == "read-only"),
        (
            "workspace-write",
            "Edit files inside the workspace",
            mode == "workspace-write",
        ),
        (
            "danger-full-access",
            "Unrestricted tool access",
            mode == "danger-full-access",
        ),
    ]
    .into_iter()
    .map(|(name, description, is_current)| {
        let marker = if is_current {
            "● current"
        } else {
            "○ available"
        };
        format!("  {name:<18} {marker:<11} {description}")
    })
    .collect::<Vec<_>>()
    .join(
        "
",
    );

    [
        "Permissions".to_string(),
        report_row("Active mode", mode),
        report_row("Mode status", "live session default"),
        String::new(),
        "Modes".to_string(),
        modes,
        String::new(),
        "Rules (settings.json -> permissions.allow / deny / ask)".to_string(),
        report_row("Form", "tool(subject)"),
        report_row(
            "Subject",
            "exact (git status) | prefix `name:*` | glob `* ? [..]`",
        ),
        report_row("Examples", "bash(git:*), edit_file(*.env), bash(git push*)"),
        report_row("Precedence", "deny > ask > allow"),
        String::new(),
        "Usage".to_string(),
        "  Inspect current mode with /permissions".to_string(),
        "  Switch modes with /permissions <mode>".to_string(),
    ]
    .join("\n")
}

pub(crate) fn format_permissions_switch_report(previous: &str, next: &str) -> String {
    [
        "Permissions updated".to_string(),
        report_row("Result", "mode switched"),
        report_row("Previous mode", previous),
        report_row("Active mode", next),
        report_row("Applies to", "subsequent tool calls"),
        report_row("Usage", "/permissions to inspect current mode"),
    ]
    .join("\n")
}

pub(crate) fn format_cost_report(usage: TokenUsage) -> String {
    [
        "Cost".to_string(),
        report_row("Input tokens", usage.input_tokens),
        report_row("Output tokens", usage.output_tokens),
        report_row("Cache create", usage.cache_creation_input_tokens),
        report_row("Cache read", usage.cache_read_input_tokens),
        report_row("Total tokens", usage.total_tokens()),
    ]
    .join("\n")
}

pub(crate) fn format_resume_report(session_path: &str, message_count: usize, turns: u32) -> String {
    [
        "Session resumed".to_string(),
        report_row("Session file", session_path),
        report_row("Messages", message_count),
        report_row("Turns", turns),
    ]
    .join("\n")
}

pub(crate) fn render_resume_usage() -> String {
    let mut out = format!(
        "Resume\n{}\n{}\n",
        report_row(
            "Usage",
            format!("/resume <session-path|session-id|{LATEST_SESSION_REFERENCE}>")
        ),
        report_row(
            "Auto-save",
            format!(
                "~/.zo/projects/<project>/sessions/<session-id>.{PRIMARY_SESSION_EXTENSION}"
            )
        )
    );
    match list_recent_sessions(10) {
        Ok(entries) if !entries.is_empty() => {
            out.push_str("  Recent sessions\n");
            for entry in entries {
                let _ = writeln!(out, "    • {entry}");
            }
            out.push_str(&report_row(
                "Tip",
                "/resume <session-id> to restore; /session list for details",
            ));
        }
        _ => {
            out.push_str(&report_row(
                "Recent sessions",
                "(none found in .zo/sessions/)",
            ));
            out.push('\n');
            out.push_str(&report_row(
                "Tip",
                "use /session list to inspect saved sessions",
            ));
        }
    }
    out
}

fn list_recent_sessions(limit: usize) -> std::io::Result<Vec<String>> {
    use std::fs;
    let dir = std::path::Path::new(".zo/sessions");
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut entries: Vec<(std::time::SystemTime, String)> = fs::read_dir(dir)?
        .filter_map(Result::ok)
        .filter_map(|e| {
            let path = e.path();
            let ext = path.extension()?.to_str()?;
            if ext != PRIMARY_SESSION_EXTENSION && ext != "json" {
                return None;
            }
            let meta = e.metadata().ok()?;
            let mtime = meta.modified().ok()?;
            let stem = path.file_stem()?.to_str()?.to_string();
            let size = meta.len();
            Some((mtime, format!("{stem}  ({size} bytes)")))
        })
        .collect();
    entries.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(entries.into_iter().take(limit).map(|(_, s)| s).collect())
}

pub(crate) fn format_compact_report(
    removed: usize,
    resulting_messages: usize,
    skipped: bool,
) -> String {
    let rows = if skipped {
        vec![
            report_row("Result", "skipped"),
            report_row("Reason", "session below compaction threshold"),
            report_row("Messages kept", resulting_messages),
        ]
    } else {
        vec![
            report_row("Result", "compacted"),
            report_row("Messages removed", removed),
            report_row("Messages kept", resulting_messages),
        ]
    };
    std::iter::once("Compact".to_string())
        .chain(rows)
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn format_auto_compaction_notice(removed: usize) -> String {
    format!("Compacted conversation · {removed} messages summarized")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_row_locks_the_value_column() {
        for label in ["Result", "Previous mode", "Session messages"] {
            let row = report_row(label, "value");
            assert_eq!(row.find("value"), Some(2 + REPORT_LABEL_FIELD), "{row:?}");
        }
    }

    #[test]
    fn model_delegation_rows_are_split_and_plain() {
        assert_eq!(
            format_model_delegation_rows(),
            "  Delegation       smart-routed per role (this pin binds the main turn only)\n  Override         /smart off or /smart pin <role> <model>"
        );
    }

    #[test]
    fn structured_report_builders_use_the_shared_rows() {
        let usage = TokenUsage {
            input_tokens: 20,
            output_tokens: 8,
            cache_creation_input_tokens: 3,
            cache_read_input_tokens: 1,
        };
        let cases = [
            (
                format_model_report("MODEL", 12, 4),
                vec![
                    report_row("Current model", "MODEL"),
                    report_row("Session messages", 12),
                    report_row("Session turns", 4),
                ],
            ),
            (
                format_model_switch_report("OLD", "NEW", 9),
                vec![
                    report_row("Previous", "OLD"),
                    report_row("Current", "NEW"),
                    report_row("Preserved msgs", 9),
                ],
            ),
            (
                format_permissions_switch_report("read-only", "workspace-write"),
                vec![
                    report_row("Result", "mode switched"),
                    report_row("Previous mode", "read-only"),
                    report_row("Active mode", "workspace-write"),
                ],
            ),
            (
                format_cost_report(usage),
                vec![
                    report_row("Input tokens", 20),
                    report_row("Output tokens", 8),
                    report_row("Total tokens", 32),
                ],
            ),
            (
                format_resume_report("session.jsonl", 14, 6),
                vec![
                    report_row("Session file", "session.jsonl"),
                    report_row("Messages", 14),
                    report_row("Turns", 6),
                ],
            ),
            (
                format_compact_report(8, 5, false),
                vec![
                    report_row("Result", "compacted"),
                    report_row("Messages removed", 8),
                    report_row("Messages kept", 5),
                ],
            ),
        ];

        for (report, rows) in cases {
            for row in rows {
                assert!(report.lines().any(|line| line == row), "{row:?} missing from {report:?}");
            }
        }
    }
}
