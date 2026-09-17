use std::fmt::Write as _;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use api::context_window_for_model;
use commands::{
    handle_agents_slash_command, handle_mcp_slash_command, handle_skills_slash_command,
};
use core_types::{CardModel, CardTone, ModelPricing};
use runtime::{PermissionMode, TeamInboxSnapshot, TeamInboxSnapshotRow};

use super::BuiltRuntime;
use crate::resume::{StatusContext, StatusUsage};
use crate::{
    format_status_report, render_config_report, render_diff_report, render_memory_report,
    render_version_report, status_context,
};

pub(crate) fn status_report(
    model: &str,
    runtime: &BuiltRuntime,
    session_path: &Path,
    permission_mode: PermissionMode,
) -> String {
    let cumulative = runtime.usage().cumulative_usage();
    let latest = runtime.usage().current_turn_usage();
    format_status_report(
        model,
        StatusUsage {
            message_count: runtime.session().messages.len(),
            turns: runtime.usage().turns(),
            latest,
            cumulative,
            estimated_tokens: runtime.estimated_tokens(),
        },
        permission_mode.as_str(),
        &match status_context(Some(session_path)) {
            Ok(context) => context,
            // `/status` is a diagnostic surface — degrade with the reason
            // (config parse error, deleted cwd, git failure) instead of
            // panicking the whole session over a report.
            Err(error) => return format!("Status unavailable: {error}"),
        },
    )
}

pub(crate) fn cost_report(runtime: &BuiltRuntime) -> String {
    crate::formatting::format_cost_report(runtime.usage().cumulative_usage())
}

/// Structured `/status` card — same data as [`status_report`], rendered as
/// a gauged dashboard instead of flat text.
pub(crate) fn status_card(
    model: &str,
    runtime: &BuiltRuntime,
    session_path: &Path,
    permission_mode: PermissionMode,
) -> CardModel {
    let usage = StatusUsage {
        message_count: runtime.session().messages.len(),
        turns: runtime.usage().turns(),
        latest: runtime.usage().current_turn_usage(),
        cumulative: runtime.usage().cumulative_usage(),
        estimated_tokens: runtime.estimated_tokens(),
    };
    let context = match status_context(Some(session_path)) {
        Ok(context) => context,
        // Same degradation policy as `status_report` above.
        Err(error) => {
            return CardModel::from_report_text("Status", &format!("Status unavailable: {error}"));
        }
    };
    build_status_card(model, usage, permission_mode.as_str(), &context)
}

/// `/cost` card — cumulative token + estimated dollar breakdown, priced at
/// the active model's own rate. An unknown model falls back to the Sonnet
/// tier and the metric label says so instead of presenting the guess as
/// authoritative (the HUD prices per-model, so a silent fallback here showed
/// two contradictory dollar figures for one session).
pub(crate) fn cost_card(model: &str, runtime: &BuiltRuntime) -> CardModel {
    let usage = runtime.usage().cumulative_usage();
    let pricing = core_types::pricing_for_model(model);
    let cost = usage
        .estimate_cost_usd_with_pricing(pricing.unwrap_or_else(ModelPricing::default_sonnet_tier));
    let cost_label = if pricing.is_some() {
        "total (est.)"
    } else {
        "total (est., unknown rate)"
    };
    CardModel::new(" /cost ")
        .section("Tokens (cumulative)")
        .metric("input", fmt_k(usage.input_tokens), CardTone::Default)
        .metric("output", fmt_k(usage.output_tokens), CardTone::Default)
        .metric(
            "cache create",
            fmt_k(usage.cache_creation_input_tokens),
            CardTone::Muted,
        )
        .metric(
            "cache read",
            fmt_k(usage.cache_read_input_tokens),
            CardTone::Muted,
        )
        .metric("total", fmt_k(usage.total_tokens()), CardTone::Default)
        .section("Estimated cost")
        .metric(
            cost_label,
            core_types::format_usd(cost.total_cost_usd()),
            CardTone::Accent,
        )
}

/// Assemble the `/status` [`CardModel`] from already-collected data. Pure
/// (no I/O) so it is unit-testable without a live runtime.
pub(crate) fn build_status_card(
    model: &str,
    usage: StatusUsage,
    permission: &str,
    context: &StatusContext,
) -> CardModel {
    // The gauge denominator is the real *input context window* for the
    // active model (`context_window_for_model`, e.g. 1M for Opus 4.8) — NOT
    // `max_tokens_for_model`, which is the per-response output cap. The
    // numerator is the live session token estimate, so the bar fills in
    // true proportion to how full the context actually is.
    let window = u32::try_from(context_window_for_model(model))
        .unwrap_or(u32::MAX)
        .max(1);
    let used = u32::try_from(usage.estimated_tokens).unwrap_or(u32::MAX);
    let ratio = f64::from(used) / f64::from(window);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let pct = (ratio.clamp(0.0, 1.0) * 100.0).round() as u32;
    let caption = format!("{} / {} · {pct}%", fmt_k(used), fmt_k(window));

    CardModel::new(" /status ")
        .section("Session")
        .key_value("model", model)
        .key_value("permission", permission)
        .metric(
            "messages",
            usage.message_count.to_string(),
            CardTone::Default,
        )
        .metric("turns", usage.turns.to_string(), CardTone::Default)
        .section("Context window")
        .gauge("usage", ratio, caption)
        .section("Tokens (cumulative)")
        .metric(
            "input",
            fmt_k(usage.cumulative.input_tokens),
            CardTone::Default,
        )
        .metric(
            "output",
            fmt_k(usage.cumulative.output_tokens),
            CardTone::Default,
        )
        .metric(
            "cache read",
            fmt_k(usage.cumulative.cache_read_input_tokens),
            CardTone::Muted,
        )
        .section("Workspace")
        .key_value("branch", context.git_branch.as_deref().unwrap_or("—"))
        .key_value("cwd", context.cwd.display().to_string())
        .key_value(
            "config files",
            format!(
                "{}/{} loaded",
                context.loaded_config_files, context.discovered_config_files
            ),
        )
        .key_value(
            "instruction files",
            context.instruction_file_count.to_string(),
        )
}

/// `1234` → `1.2k`, `1_200_000` → `1.2M` — compact token read-out.
fn fmt_k(value: u32) -> String {
    let v = f64::from(value);
    if value >= 1_000_000 {
        format!("{:.1}M", v / 1_000_000.0)
    } else if value >= 1_000 {
        format!("{:.1}k", v / 1_000.0)
    } else {
        value.to_string()
    }
}


pub(crate) fn inbox_report(cwd: &Path, session_id: &str) -> String {
    let snapshot = runtime::team_inbox_snapshot(cwd, session_id, 50);
    render_team_inbox_report(&snapshot)
}

/// `/inbox` with optional args. `ack <update-id>` acks through the runtime
/// session-consumer seam and then prints the refreshed report (mirroring the
/// viewer modal's `a` action for headless/resume modes); anything else prints
/// usage; no args falls back to the plain report.
pub(crate) fn inbox_command(cwd: &Path, session_id: &str, args: Option<&str>) -> String {
    let Some(command) = args.map(str::trim).filter(|args| !args.is_empty()) else {
        return inbox_report(cwd, session_id);
    };
    let mut tokens = command.split_whitespace();
    match (tokens.next(), tokens.next(), tokens.next()) {
        (Some("ack"), Some(update_id), None) => {
            let mut out = String::new();
            match runtime::team_inbox_manual_ack(cwd, session_id, update_id) {
                Ok(()) => {
                    let _ = writeln!(out, "Acked {update_id}");
                }
                Err(error) => {
                    let _ = writeln!(out, "Not acked: {error}");
                }
            }
            out.push_str(&inbox_report(cwd, session_id));
            out
        }
        _ => "Usage: /inbox [ack <update-id>]".to_string(),
    }
}

pub(crate) fn render_team_inbox_report(snapshot: &TeamInboxSnapshot) -> String {
    let joined = if snapshot.joined_channels.is_empty() {
        "none".to_string()
    } else {
        snapshot.joined_channels.join(", ")
    };
    let mut out = String::new();
    let _ = writeln!(out, "Team inbox");
    let _ = writeln!(out, "  Joined channels  {joined}");
    let _ = writeln!(out, "  Unread           {}", snapshot.unread);
    if snapshot.rows.is_empty() {
        out.push_str("  Updates          none");
        return out;
    }
    out.push_str("\nState        Priority  Channel       Id            Age       Summary\n");
    out.push_str("-----------  --------  ------------  ------------  --------  ----------------\n");
    for row in &snapshot.rows {
        let _ = writeln!(
            out,
            "{:<11}  {:<8}  {:<12}  {:<12}  {:>8}  {}",
            report_state(row),
            row.priority,
            truncate_cell(&row.channel, 12),
            truncate_cell(&row.id, 12),
            report_age(row.created_at_unix),
            one_line(&row.summary),
        );
    }
    out.trim_end().to_string()
}

fn report_state(row: &TeamInboxSnapshotRow) -> &'static str {
    match row.delivery_state.as_deref() {
        Some("failed") => "failed",
        Some("stale") => "stale",
        Some("injected") => "injected",
        Some("acked") => "acked",
        _ => "unread",
    }
}

fn one_line(value: &str) -> String {
    value.replace(['\r', '\n'], " ")
}

fn truncate_cell(value: &str, width: usize) -> String {
    let mut out = String::new();
    for ch in value.chars().take(width) {
        out.push(ch);
    }
    out
}

fn report_age(created_at_unix: i64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_secs()).ok())
        .unwrap_or(created_at_unix);
    let secs = now.saturating_sub(created_at_unix).max(0);
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

pub(crate) fn config_report(section: Option<&str>) -> Result<String, Box<dyn std::error::Error>> {
    render_config_report(section)
}

pub(crate) fn memory_report() -> Result<String, Box<dyn std::error::Error>> {
    render_memory_report()
}

pub(crate) fn agents_report(args: Option<&str>) -> Result<String, Box<dyn std::error::Error>> {
    let cwd = crate::current_cli_cwd()?;
    Ok(handle_agents_slash_command(args, &cwd)?)
}

pub(crate) fn mcp_report(args: Option<&str>) -> Result<String, Box<dyn std::error::Error>> {
    let cwd = crate::current_cli_cwd()?;
    Ok(handle_mcp_slash_command(args, &cwd)?)
}

pub(crate) fn skills_report(args: Option<&str>) -> Result<String, Box<dyn std::error::Error>> {
    let cwd = crate::current_cli_cwd()?;
    Ok(handle_skills_slash_command(args, &cwd)?)
}

pub(crate) fn diff_report() -> Result<String, Box<dyn std::error::Error>> {
    render_diff_report()
}

pub(crate) fn version_report() -> String {
    render_version_report()
}

#[cfg(test)]
mod tests {
    use super::{inbox_command, render_team_inbox_report, version_report};
    use crate::render_version_report;
    use runtime::{TeamInboxSnapshot, TeamInboxSnapshotRow};

    /// `/inbox` argument handling: empty args fall back to the plain report,
    /// a well-formed `ack` on a missing store surfaces the runtime error and
    /// still appends the report, and malformed args print usage.
    #[test]
    fn inbox_command_routes_args_to_report_ack_and_usage() {
        let dir = std::env::temp_dir().join(format!(
            "zo-inbox-cmd-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");

        let report = inbox_command(&dir, "s1", None);
        assert!(report.starts_with("Team inbox"));
        assert_eq!(report, inbox_command(&dir, "s1", Some("   ")));

        let acked = inbox_command(&dir, "s1", Some("ack u1"));
        assert!(
            acked.starts_with("Not acked: "),
            "ack against a missing store must surface the error: {acked}"
        );
        assert!(acked.contains("Team inbox"), "refreshed report must follow");

        for bad in ["ack", "ack u1 extra", "list", "help"] {
            assert_eq!(
                inbox_command(&dir, "s1", Some(bad)),
                "Usage: /inbox [ack <update-id>]",
                "malformed args {bad:?} must print usage"
            );
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn version_report_delegates_to_shared_renderer() {
        assert_eq!(version_report(), render_version_report());
    }

    #[test]
    fn team_inbox_report_renders_summary_table_without_raw_body() {
        let report = render_team_inbox_report(&TeamInboxSnapshot {
            joined_channels: vec!["ci".to_string()],
            unread: 1,
            rows: vec![TeamInboxSnapshotRow {
                seq: 7,
                id: "update-1".to_string(),
                channel: "ci".to_string(),
                source: "agent".to_string(),
                created_at_unix: 1,
                priority: "high".to_string(),
                summary: "safe summary".to_string(),
                delivery_state: None,
                retry_count: 0,
                task_id: Some("task-1".to_string()),
                status: Some("done".to_string()),
            }],
        });
        assert!(report.contains("Joined channels  ci"));
        assert!(report.contains("Unread           1"));
        assert!(report.contains("unread"));
        assert!(report.contains("safe summary"));
        assert!(!report.contains("raw body"));
    }
}
