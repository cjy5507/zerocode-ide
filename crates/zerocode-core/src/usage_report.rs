//! The shape a five-counter token report has, whichever vendor filled it.
//!
//! Codex and OpenCode both report the same five figures — input, the cached
//! share of it, output, the reasoning share of that, and a total the vendor
//! computed itself — so the pane that draws one draws the other. These shapes
//! live here rather than in either ledger so that the second vendor does not
//! arrive with a second set of identical structs and a window branch to tell
//! them apart.
//!
//! Claude is NOT one of them: it reports four independent counters (input,
//! output, cache read, cache write) with no total and no reasoning line, so it
//! keeps its own shapes next to its own rules. Two shapes for two data models
//! is the honest count; three would be one too many and one would be a lie.
//!
//! Nothing here decides anything. The rules — what a turn is worth, what a day
//! is, which rows a scope keeps — belong to the ledger modules, and each has
//! its tests beside it.

use serde::{Deserialize, Serialize};

use crate::usage_stats::{Range, Scope};

/// The headline figures, for one scope and range.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Summary {
    pub scope: Scope,
    pub range: Range,
    pub sessions: i64,
    pub events: i64,
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_output_tokens: i64,
    pub total_tokens: i64,
    pub estimated_cost_usd: Option<f64>,
    pub top_model: Option<String>,
    pub top_project: Option<String>,
    pub has_any_data: bool,
}

/// One bar of the daily chart.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DailyPoint {
    pub day: String,
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_output_tokens: i64,
    /// The total the VENDOR reported, which is not the four above added up —
    /// cached input is a share of input and reasoning output is a share of
    /// output. Anything that wants "how much was this day" must use this, or
    /// it counts the same tokens twice.
    pub total_tokens: i64,
}

/// One row of a by-model or by-project list.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BreakdownRow {
    pub key: String,
    pub label: String,
    pub sessions: i64,
    pub events: i64,
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    pub estimated_cost_usd: Option<f64>,
}

/// One row of the recent-sessions table.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionRow {
    pub session_id: String,
    pub last_active_at: String,
    pub duration_minutes: i64,
    pub project_label: String,
    pub model: Option<String>,
    pub events: i64,
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub output_tokens: i64,
    /// The vendor's own total for this session, which is the column the table
    /// ends with — the counters beside it overlap for one of these vendors and
    /// not for the other, so a reader who adds them is right only by luck.
    pub total_tokens: i64,
}

/// Everything one of these panes draws for one scope and range.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Report {
    pub summary: Summary,
    pub daily: Vec<DailyPoint>,
    pub model_breakdown: Vec<BreakdownRow>,
    pub project_breakdown: Vec<BreakdownRow>,
    pub recent_sessions: Vec<SessionRow>,
}
