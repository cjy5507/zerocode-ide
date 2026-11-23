//! The rollup every five-counter token ledger shares.
//!
//! Codex and OpenCode record the same five figures per turn and draw the same
//! pane, so the arithmetic between the two — roll turns into sessions and
//! per-day rows, merge what several files each learned, keep the rows one
//! scope and one range asked for, and total the result — is written once here.
//! What is NOT shared is how each vendor's turns are READ (a JSONL rollout, a
//! SQLite row) and how they are PRICED, and those stay in the vendor modules
//! next door with their own tests.
//!
//! Claude is not one of them: it reports four independent counters and no
//! total, so [`crate::usage_stats`] keeps its own rollup for its own shape.
//!
//! The reader supplies each request's reported or estimated cost before rollup.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::civil::{epoch_ms_of_iso, iso_date_of};
use crate::usage_stats::{Range, Scope, WorktreeRef, place, range_cutoff};

/* ---- what this pane draws ------------------------------------------------
 *
 * The five-counter shapes live in [`crate::usage_report`], because the pane
 * that draws one vendor draws the other. */
pub use crate::usage_report::{BreakdownRow, DailyPoint, Report, SessionRow, Summary};

/// One billable turn, as some vendor's reader recovered it.
///
/// `cached_input_tokens` and `reasoning_output_tokens` are SHARES of the two
/// counters beside them for one vendor and separate buckets for another, which
/// is why nothing here adds the counters up: `total_tokens` is what the vendor
/// called the total, and it is the only figure that means "how much was this".
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    pub session_id: String,
    pub timestamp: String,
    pub model: Option<String>,
    pub cwd: Option<String>,
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_output_tokens: i64,
    pub total_tokens: i64,
    /// Reported or estimated cost for this request. Missing prices stay unknown.
    pub estimated_cost_usd: Option<f64>,
    /// The identity this turn is deduped by, when its reader needs one.
    ///
    /// A rollout copied by a fork repeats records byte for byte, so the reader
    /// that can meet the same turn twice carries a key it can claim. A reader
    /// whose rows are unique by construction leaves this empty.
    #[serde(default)]
    pub event_key: String,
}

use model_prices::{CostTotal, sum_cost};

/// Where one session spent its events.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LocationBreakdown {
    pub location_key: String,
    pub project_label: String,
    pub repo_id: Option<String>,
    pub worktree_id: Option<String>,
    pub event_count: i64,
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_output_tokens: i64,
    pub total_tokens: i64,
}

/// One model a session used, and what it spent there.
///
/// Only the presence of a model in a session is read today — the model list
/// under the chart counts the sessions that used each model, and a session
/// that switched models used both. The counters ride along because they cost
/// two additions and answer the next question anyone asks of such a row.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelBreakdown {
    pub model_key: String,
    pub event_count: i64,
    pub total_tokens: i64,
}

/// One conversation, totalled.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    pub session_id: String,
    pub first_timestamp: String,
    pub last_timestamp: String,
    pub model: Option<String>,
    pub last_cwd: Option<String>,
    pub event_count: i64,
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_output_tokens: i64,
    pub total_tokens: i64,
    pub location_breakdown: Vec<LocationBreakdown>,
    /// Every model this session used, not just the last one it announced.
    #[serde(default)]
    pub model_breakdown: Vec<ModelBreakdown>,
    /// What the vendor said the session's turns cost, summed — the per-day
    /// rows' figure ([`DailyAggregate::estimated_cost_usd`]) kept per
    /// conversation, so one conversation can be priced the way its vendor
    /// priced it (a task's cost, t-9470). `None` where no turn reported one.
    #[serde(default)]
    pub estimated_cost_usd: Option<f64>,
    #[serde(default)]
    pub unpriced_requests: u64,
}

/// One day of one model in one project.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DailyAggregate {
    pub day: String,
    pub model: Option<String>,
    pub project_key: String,
    pub project_label: String,
    pub repo_id: Option<String>,
    pub worktree_id: Option<String>,
    pub event_count: i64,
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_output_tokens: i64,
    pub total_tokens: i64,
    /// Sum of the known request costs.
    #[serde(default)]
    pub estimated_cost_usd: Option<f64>,
    #[serde(default)]
    pub unpriced_requests: u64,
}

/// Everything one scan learned, before scope or range is applied.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Ledger {
    pub sessions: Vec<Session>,
    pub daily_aggregates: Vec<DailyAggregate>,
}

/// Rolls placed turns into sessions and per-day rows.
#[must_use]
pub fn aggregate(entries: Vec<Entry>, worktrees: &[WorktreeRef], offset_minutes: i32) -> Ledger {
    let mut sessions: BTreeMap<String, Session> = BTreeMap::new();
    let mut daily: BTreeMap<String, DailyAggregate> = BTreeMap::new();

    for entry in entries {
        let Some(placed) = place(
            entry.cwd.as_deref(),
            &entry.timestamp,
            worktrees,
            offset_minutes,
        ) else {
            continue;
        };

        let session = sessions
            .entry(entry.session_id.clone())
            .or_insert_with(|| Session {
                session_id: entry.session_id.clone(),
                first_timestamp: entry.timestamp.clone(),
                last_timestamp: entry.timestamp.clone(),
                model: entry.model.clone(),
                last_cwd: entry.cwd.clone(),
                event_count: 0,
                input_tokens: 0,
                cached_input_tokens: 0,
                output_tokens: 0,
                reasoning_output_tokens: 0,
                total_tokens: 0,
                location_breakdown: Vec::new(),
                model_breakdown: Vec::new(),
                estimated_cost_usd: None,
                unpriced_requests: 0,
            });
        if entry.timestamp < session.first_timestamp {
            session.first_timestamp = entry.timestamp.clone();
        }
        if entry.timestamp > session.last_timestamp {
            session.last_timestamp = entry.timestamp.clone();
            session.last_cwd = entry.cwd.clone();
        }
        if entry.model.is_some() {
            session.model = entry.model.clone();
        }
        session.event_count += 1;
        session.input_tokens += entry.input_tokens;
        session.cached_input_tokens += entry.cached_input_tokens;
        session.output_tokens += entry.output_tokens;
        session.reasoning_output_tokens += entry.reasoning_output_tokens;
        session.total_tokens += entry.total_tokens;
        session.unpriced_requests += u64::from(entry.estimated_cost_usd.is_none());
        session.estimated_cost_usd = sum_cost(session.estimated_cost_usd, entry.estimated_cost_usd);

        match session
            .location_breakdown
            .iter_mut()
            .find(|entry| entry.location_key == placed.project_key)
        {
            Some(location) => {
                location.event_count += 1;
                location.input_tokens += entry.input_tokens;
                location.cached_input_tokens += entry.cached_input_tokens;
                location.output_tokens += entry.output_tokens;
                location.reasoning_output_tokens += entry.reasoning_output_tokens;
                location.total_tokens += entry.total_tokens;
            }
            None => session.location_breakdown.push(LocationBreakdown {
                location_key: placed.project_key.clone(),
                project_label: placed.project_label.clone(),
                repo_id: placed.repo_id.clone(),
                worktree_id: placed.worktree_id.clone(),
                event_count: 1,
                input_tokens: entry.input_tokens,
                cached_input_tokens: entry.cached_input_tokens,
                output_tokens: entry.output_tokens,
                reasoning_output_tokens: entry.reasoning_output_tokens,
                total_tokens: entry.total_tokens,
            }),
        }

        let model_key = entry.model.clone().unwrap_or_else(|| "unknown".to_string());
        match session
            .model_breakdown
            .iter_mut()
            .find(|held| held.model_key == model_key)
        {
            Some(held) => {
                held.event_count += 1;
                held.total_tokens += entry.total_tokens;
            }
            None => session.model_breakdown.push(ModelBreakdown {
                model_key: model_key.clone(),
                event_count: 1,
                total_tokens: entry.total_tokens,
            }),
        }

        let row = daily
            .entry(format!(
                "{}::{model_key}::{}",
                placed.day, placed.project_key
            ))
            .or_insert_with(|| DailyAggregate {
                day: placed.day.clone(),
                model: entry.model.clone(),
                project_key: placed.project_key.clone(),
                project_label: placed.project_label.clone(),
                repo_id: placed.repo_id.clone(),
                worktree_id: placed.worktree_id.clone(),
                event_count: 0,
                input_tokens: 0,
                cached_input_tokens: 0,
                output_tokens: 0,
                reasoning_output_tokens: 0,
                total_tokens: 0,
                estimated_cost_usd: None,
                unpriced_requests: 0,
            });
        row.event_count += 1;
        row.input_tokens += entry.input_tokens;
        row.cached_input_tokens += entry.cached_input_tokens;
        row.output_tokens += entry.output_tokens;
        row.reasoning_output_tokens += entry.reasoning_output_tokens;
        row.total_tokens += entry.total_tokens;
        row.estimated_cost_usd = sum_cost(row.estimated_cost_usd, entry.estimated_cost_usd);
        row.unpriced_requests += u64::from(entry.estimated_cost_usd.is_none());
    }

    let mut ledger = Ledger {
        sessions: sessions.into_values().collect(),
        daily_aggregates: daily.into_values().collect(),
    };
    finalize(&mut ledger);
    ledger
}

/// Merges one file's ledger into a running one.
pub fn merge(into: &mut Ledger, from: Ledger) {
    let mut at_session: HashMap<String, usize> = into
        .sessions
        .iter()
        .enumerate()
        .map(|(index, session)| (session.session_id.clone(), index))
        .collect();
    for session in from.sessions {
        let Some(&index) = at_session.get(&session.session_id) else {
            at_session.insert(session.session_id.clone(), into.sessions.len());
            into.sessions.push(session);
            continue;
        };
        let held = &mut into.sessions[index];
        if session.first_timestamp < held.first_timestamp {
            held.first_timestamp = session.first_timestamp.clone();
        }
        if session.last_timestamp > held.last_timestamp {
            held.last_timestamp = session.last_timestamp.clone();
            held.last_cwd = session.last_cwd.clone();
        }
        if session.model.is_some() {
            held.model = session.model.clone();
        }
        held.event_count += session.event_count;
        held.input_tokens += session.input_tokens;
        held.cached_input_tokens += session.cached_input_tokens;
        held.output_tokens += session.output_tokens;
        held.reasoning_output_tokens += session.reasoning_output_tokens;
        held.total_tokens += session.total_tokens;
        held.estimated_cost_usd = sum_cost(held.estimated_cost_usd, session.estimated_cost_usd);
        held.unpriced_requests += session.unpriced_requests;
        for model in session.model_breakdown {
            match held
                .model_breakdown
                .iter_mut()
                .find(|entry| entry.model_key == model.model_key)
            {
                Some(existing) => {
                    existing.event_count += model.event_count;
                    existing.total_tokens += model.total_tokens;
                }
                None => held.model_breakdown.push(model),
            }
        }
        for location in session.location_breakdown {
            match held
                .location_breakdown
                .iter_mut()
                .find(|entry| entry.location_key == location.location_key)
            {
                Some(existing) => {
                    existing.event_count += location.event_count;
                    existing.input_tokens += location.input_tokens;
                    existing.cached_input_tokens += location.cached_input_tokens;
                    existing.output_tokens += location.output_tokens;
                    existing.reasoning_output_tokens += location.reasoning_output_tokens;
                    existing.total_tokens += location.total_tokens;
                }
                None => held.location_breakdown.push(location),
            }
        }
    }

    let key_of = |row: &DailyAggregate| {
        format!(
            "{}::{}::{}",
            row.day,
            row.model.clone().unwrap_or_else(|| "unknown".to_string()),
            row.project_key
        )
    };
    let mut at_daily: HashMap<String, usize> = into
        .daily_aggregates
        .iter()
        .enumerate()
        .map(|(index, row)| (key_of(row), index))
        .collect();
    for row in from.daily_aggregates {
        let key = key_of(&row);
        let Some(&index) = at_daily.get(&key) else {
            at_daily.insert(key, into.daily_aggregates.len());
            into.daily_aggregates.push(row);
            continue;
        };
        let held = &mut into.daily_aggregates[index];
        held.event_count += row.event_count;
        held.input_tokens += row.input_tokens;
        held.cached_input_tokens += row.cached_input_tokens;
        held.output_tokens += row.output_tokens;
        held.reasoning_output_tokens += row.reasoning_output_tokens;
        held.total_tokens += row.total_tokens;
        held.estimated_cost_usd = sum_cost(held.estimated_cost_usd, row.estimated_cost_usd);
        held.unpriced_requests += row.unpriced_requests;
    }
}

/// Puts a merged ledger back in the order the pane reads it in.
pub fn finalize(ledger: &mut Ledger) {
    for session in &mut ledger.sessions {
        session
            .location_breakdown
            .sort_by(|left, right| right.total_tokens.cmp(&left.total_tokens));
        session
            .model_breakdown
            .sort_by(|left, right| right.total_tokens.cmp(&left.total_tokens));
    }
    ledger
        .sessions
        .sort_by(|left, right| right.last_timestamp.cmp(&left.last_timestamp));
    ledger.daily_aggregates.sort_by(|left, right| {
        left.day
            .cmp(&right.day)
            .then_with(|| left.project_label.cmp(&right.project_label))
    });
}

fn kept_daily<'a>(
    ledger: &'a Ledger,
    scope: Scope,
    cutoff: Option<&str>,
) -> Vec<&'a DailyAggregate> {
    ledger
        .daily_aggregates
        .iter()
        .filter(|row| {
            if let Some(cutoff) = cutoff
                && row.day.as_str() < cutoff
            {
                return false;
            }
            !(scope == Scope::Zerocode && row.worktree_id.is_none())
        })
        .collect()
}

fn kept_sessions<'a>(
    ledger: &'a Ledger,
    scope: Scope,
    cutoff: Option<&str>,
    offset_minutes: i32,
) -> Vec<&'a Session> {
    ledger
        .sessions
        .iter()
        .filter(|session| {
            let Some(epoch_ms) = epoch_ms_of_iso(&session.last_timestamp) else {
                return false;
            };
            if let Some(cutoff) = cutoff
                && iso_date_of(epoch_ms, offset_minutes).as_str() < cutoff
            {
                return false;
            }
            if scope == Scope::Zerocode {
                return session
                    .location_breakdown
                    .iter()
                    .any(|entry| entry.worktree_id.is_some());
            }
            true
        })
        .collect()
}

/// Builds every figure one of these panes draws.
#[must_use]
pub fn report(
    ledger: &Ledger,
    scope: Scope,
    range: Range,
    now_ms: i64,
    offset_minutes: i32,
) -> Report {
    let cutoff = range_cutoff(range, now_ms, offset_minutes);
    let cutoff = cutoff.as_deref();
    let daily_rows = kept_daily(ledger, scope, cutoff);
    let sessions = kept_sessions(ledger, scope, cutoff, offset_minutes);

    let mut summary = Summary {
        scope,
        range,
        sessions: sessions.len() as i64,
        events: 0,
        input_tokens: 0,
        cached_input_tokens: 0,
        output_tokens: 0,
        reasoning_output_tokens: 0,
        total_tokens: 0,
        estimated_cost_usd: None,
        unpriced_requests: 0,
        top_model: None,
        top_project: None,
        has_any_data: !sessions.is_empty() || !daily_rows.is_empty(),
    };
    let mut by_model: BTreeMap<String, i64> = BTreeMap::new();
    let mut by_project: BTreeMap<String, i64> = BTreeMap::new();
    let mut cost = CostTotal::default();
    for row in &daily_rows {
        summary.events += row.event_count;
        summary.input_tokens += row.input_tokens;
        summary.cached_input_tokens += row.cached_input_tokens;
        summary.output_tokens += row.output_tokens;
        summary.reasoning_output_tokens += row.reasoning_output_tokens;
        summary.total_tokens += row.total_tokens;
        // The vendor's own total, because the counters beside it overlap for
        // one vendor and do not for the other — adding them would rank a
        // cache-heavy model by a quarter of the tokens it actually moved.
        let weight = row.total_tokens;
        *by_model
            .entry(
                row.model
                    .clone()
                    .unwrap_or_else(|| "Unknown model".to_string()),
            )
            .or_insert(0) += weight;
        *by_project.entry(row.project_label.clone()).or_insert(0) += weight;
        cost.merge(
            CostTotal {
                cost_usd: row.estimated_cost_usd,
                unpriced_requests: row.unpriced_requests,
            }
            .for_requests(row.event_count),
        );
    }
    let heaviest = |counts: BTreeMap<String, i64>| {
        counts
            .into_iter()
            .max_by(|left, right| left.1.cmp(&right.1))
            .map(|(label, _)| label)
    };
    summary.top_model = heaviest(by_model);
    summary.top_project = heaviest(by_project);
    summary.estimated_cost_usd = cost.cost_usd;
    summary.unpriced_requests = cost.unpriced_requests;

    let mut by_day: BTreeMap<String, DailyPoint> = BTreeMap::new();
    for row in &daily_rows {
        let point = by_day.entry(row.day.clone()).or_insert_with(|| DailyPoint {
            day: row.day.clone(),
            input_tokens: 0,
            cached_input_tokens: 0,
            output_tokens: 0,
            reasoning_output_tokens: 0,
            total_tokens: 0,
        });
        point.input_tokens += row.input_tokens;
        point.cached_input_tokens += row.cached_input_tokens;
        point.output_tokens += row.output_tokens;
        point.reasoning_output_tokens += row.reasoning_output_tokens;
        point.total_tokens += row.total_tokens;
    }

    Report {
        summary,
        daily: by_day.into_values().collect(),
        model_breakdown: breakdown(&daily_rows, &sessions, scope, true),
        project_breakdown: breakdown(&daily_rows, &sessions, scope, false),
        recent_sessions: recent_sessions(&sessions, scope, 12),
    }
}

/// One of the two lists under the chart.
fn breakdown(
    daily_rows: &[&DailyAggregate],
    sessions: &[&Session],
    scope: Scope,
    by_model: bool,
) -> Vec<BreakdownRow> {
    let mut rows: BTreeMap<String, BreakdownRow> = BTreeMap::new();
    for row in daily_rows {
        let (key, label) = if by_model {
            (
                row.model.clone().unwrap_or_else(|| "unknown".to_string()),
                row.model
                    .clone()
                    .unwrap_or_else(|| "Unknown model".to_string()),
            )
        } else {
            (row.project_key.clone(), row.project_label.clone())
        };
        let held = rows.entry(key.clone()).or_insert_with(|| BreakdownRow {
            key,
            label,
            sessions: 0,
            events: 0,
            input_tokens: 0,
            cached_input_tokens: 0,
            output_tokens: 0,
            total_tokens: 0,
            estimated_cost_usd: None,
            unpriced_requests: 0,
        });
        held.events += row.event_count;
        held.input_tokens += row.input_tokens;
        held.cached_input_tokens += row.cached_input_tokens;
        held.output_tokens += row.output_tokens;
        held.total_tokens += row.total_tokens;
        held.estimated_cost_usd = sum_cost(held.estimated_cost_usd, row.estimated_cost_usd);
        held.unpriced_requests += CostTotal {
            cost_usd: row.estimated_cost_usd,
            unpriced_requests: row.unpriced_requests,
        }
        .for_requests(row.event_count)
        .unpriced_requests;
    }

    for session in sessions {
        if by_model {
            // Every model the session used, not the last one it announced: a
            // session that switched models used both, and counting only the
            // last leaves a row saying "0 sessions" above a real token count.
            for model in &session.model_breakdown {
                if let Some(row) = rows.get_mut(&model.model_key) {
                    row.sessions += 1;
                }
            }
            continue;
        }
        let mut seen: HashSet<&str> = HashSet::new();
        for location in &session.location_breakdown {
            if scope == Scope::Zerocode && location.worktree_id.is_none() {
                continue;
            }
            if !seen.insert(location.location_key.as_str()) {
                continue;
            }
            if let Some(row) = rows.get_mut(&location.location_key) {
                row.sessions += 1;
            }
        }
    }

    let mut rows: Vec<BreakdownRow> = rows.into_values().collect();
    rows.sort_by(|left, right| right.total_tokens.cmp(&left.total_tokens));
    rows
}

/// The head of the session list, totalled over the locations this scope kept.
fn recent_sessions(sessions: &[&Session], scope: Scope, limit: usize) -> Vec<SessionRow> {
    sessions
        .iter()
        .take(limit)
        .map(|session| {
            let matching: Vec<&LocationBreakdown> = session
                .location_breakdown
                .iter()
                .filter(|entry| scope == Scope::All || entry.worktree_id.is_some())
                .collect();
            let scoped: Vec<&LocationBreakdown> = if matching.is_empty() {
                session.location_breakdown.iter().collect()
            } else {
                matching
            };
            let mut row = SessionRow {
                session_id: session.session_id.clone(),
                last_active_at: session.last_timestamp.clone(),
                duration_minutes: 0,
                project_label: match scoped.len() {
                    0 => "Unknown location".to_string(),
                    1 => scoped[0].project_label.clone(),
                    _ => "Multiple locations".to_string(),
                },
                model: session.model.clone(),
                events: 0,
                input_tokens: 0,
                cached_input_tokens: 0,
                output_tokens: 0,
                total_tokens: 0,
            };
            for entry in scoped {
                row.events += entry.event_count;
                row.input_tokens += entry.input_tokens;
                row.cached_input_tokens += entry.cached_input_tokens;
                row.output_tokens += entry.output_tokens;
                row.total_tokens += entry.total_tokens;
            }
            if let (Some(first), Some(last)) = (
                epoch_ms_of_iso(&session.first_timestamp),
                epoch_ms_of_iso(&session.last_timestamp),
            ) {
                row.duration_minutes = (((last - first) as f64) / 60_000.0).round().max(0.0) as i64;
            }
            row
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const KST: i32 = 9 * 60;

    fn entry(session: &str, when: &str, cwd: &str, input: i64) -> Entry {
        Entry {
            session_id: session.to_string(),
            timestamp: when.to_string(),
            event_key: format!("{session}{when}"),
            model: Some("priced-model".to_string()),
            cwd: Some(cwd.to_string()),
            estimated_cost_usd: Some(input as f64 / 1_000.0),
            input_tokens: input,
            cached_input_tokens: input / 2,
            output_tokens: 10,
            reasoning_output_tokens: 3,
            total_tokens: input + 10,
        }
    }

    fn managed() -> WorktreeRef {
        WorktreeRef {
            repo_id: "repo".to_string(),
            worktree_id: "wt".to_string(),
            path: "/w/repo".to_string(),
            display_name: "repo".to_string(),
        }
    }

    /// The report answers for one scope and one range, as Claude's does.
    #[test]
    fn the_report_answers_for_one_scope_and_one_range() {
        let ledger = aggregate(
            vec![
                entry("in", "2026-08-19T01:00:00Z", "/w/repo/src", 100),
                entry("out", "2026-08-12T01:00:00Z", "/elsewhere", 50),
            ],
            &[managed()],
            KST,
        );
        let now = epoch_ms_of_iso("2026-08-19T02:00:00Z").expect("stamp");

        let everything = report(&ledger, Scope::All, Range::All, now, KST);
        assert_eq!(everything.summary.sessions, 2);
        assert_eq!(everything.summary.input_tokens, 150);
        assert_eq!(everything.summary.events, 2);
        assert_eq!(everything.daily.len(), 2);
        // A day carries the total the VENDOR reported, which is not its four
        // counters added up: for one vendor the cached count is a share of the
        // input and the reasoning a share of the output, so anyone adding them
        // counts the same tokens twice.
        let heaviest = everything
            .daily
            .iter()
            .max_by_key(|day| day.total_tokens)
            .expect("a day");
        assert_eq!(heaviest.total_tokens, 110);
        assert!(
            heaviest.total_tokens
                < heaviest.input_tokens
                    + heaviest.cached_input_tokens
                    + heaviest.output_tokens
                    + heaviest.reasoning_output_tokens,
            "the reported total became the sum of the parts, which double counts"
        );

        let ours = report(&ledger, Scope::Zerocode, Range::All, now, KST);
        assert_eq!(
            ours.summary.sessions, 1,
            "an unmanaged session survived the scope"
        );
        assert_eq!(ours.summary.input_tokens, 100);
        assert_eq!(ours.project_breakdown[0].label, "repo");
        assert_eq!(ours.summary.top_model.as_deref(), Some("priced-model"));
        assert!(ours.summary.estimated_cost_usd.is_some());

        let week = report(&ledger, Scope::All, Range::Week, now, KST);
        assert_eq!(
            week.summary.sessions, 1,
            "a session outside the range was counted"
        );
        assert_eq!(week.recent_sessions.len(), 1);
        assert_eq!(week.recent_sessions[0].project_label, "repo");

        let empty = report(&Ledger::default(), Scope::All, Range::All, now, KST);
        assert!(!empty.summary.has_any_data);
        assert_eq!(empty.summary.estimated_cost_usd, None);
    }

    /// A session that switched models is counted under both of them.
    #[test]
    fn a_switched_model_is_not_forgotten_by_the_list() {
        let mut first = entry("mixed", "2026-08-19T01:00:00Z", "/w/repo", 100);
        first.model = Some("early-model".to_string());
        let mut second = entry("mixed", "2026-08-19T02:00:00Z", "/w/repo", 100);
        second.model = Some("late-model".to_string());
        let ledger = aggregate(vec![first, second], &[managed()], KST);
        let now = epoch_ms_of_iso("2026-08-19T03:00:00Z").expect("stamp");
        let answer = report(&ledger, Scope::All, Range::All, now, KST);

        assert_eq!(answer.summary.sessions, 1, "one session used two models");
        assert_eq!(answer.model_breakdown.len(), 2);
        for row in &answer.model_breakdown {
            assert_eq!(
                row.sessions, 1,
                "{} reported {} sessions beside a real token count",
                row.key, row.sessions
            );
        }
    }

    /// A vendor that reports dollars is believed rather than re-priced.
    #[test]
    fn a_reported_cost_is_carried_and_not_invented() {
        let mut paid = entry("paid", "2026-08-19T01:00:00Z", "/w/repo", 100);
        paid.estimated_cost_usd = Some(0.25);
        let mut also = entry("paid", "2026-08-19T02:00:00Z", "/w/repo", 100);
        also.estimated_cost_usd = Some(0.75);
        let ledger = aggregate(vec![paid, also], &[managed()], KST);
        let now = epoch_ms_of_iso("2026-08-19T03:00:00Z").expect("stamp");

        let answer = report(&ledger, Scope::All, Range::All, now, KST);
        assert_eq!(answer.summary.estimated_cost_usd, Some(1.0));
        assert_eq!(answer.model_breakdown[0].estimated_cost_usd, Some(1.0));
        assert_eq!(
            answer.project_breakdown[0].estimated_cost_usd,
            Some(1.0),
            "a reported cost reaches the project list too"
        );
    }

    /// A conversation keeps what its turns reported, summed across the files
    /// a merge joins — one conversation priced the way its vendor priced it
    /// (t-9470) — and one no turn priced says nothing rather than free.
    #[test]
    fn a_session_keeps_the_dollars_its_turns_reported() {
        let mut paid = entry("paid", "2026-08-19T01:00:00Z", "/w/repo", 100);
        paid.estimated_cost_usd = Some(0.25);
        let mut unpriced = entry("paid", "2026-08-19T01:30:00Z", "/w/repo", 100);
        unpriced.estimated_cost_usd = None;
        let mut ledger = aggregate(vec![paid, unpriced], &[managed()], KST);
        let mut later = entry("paid", "2026-08-19T02:00:00Z", "/w/repo", 100);
        later.estimated_cost_usd = Some(0.5);
        merge(&mut ledger, aggregate(vec![later], &[managed()], KST));
        let mut unpriced = entry("unpriced", "2026-08-19T01:00:00Z", "/w/repo", 100);
        unpriced.estimated_cost_usd = None;
        merge(&mut ledger, aggregate(vec![unpriced], &[managed()], KST));
        let cost = |id: &str| {
            ledger
                .sessions
                .iter()
                .find(|session| session.session_id == id)
                .map(|session| session.estimated_cost_usd)
        };
        assert_eq!(cost("paid"), Some(Some(0.75)));
        assert_eq!(cost("unpriced"), Some(None));
    }

    #[test]
    fn a_report_distinguishes_unpriced_usage_from_a_reported_zero() {
        let mut unpriced = entry("unpriced", "2026-08-19T01:00:00Z", "/w/repo", 100);
        unpriced.model = Some("synthetic-unpriced".to_string());
        unpriced.estimated_cost_usd = None;
        let mut free = entry("free", "2026-08-19T02:00:00Z", "/w/repo", 100);
        free.model = Some("synthetic-free".to_string());
        free.estimated_cost_usd = Some(0.0);
        let ledger = aggregate(vec![unpriced, free], &[managed()], KST);
        let now = epoch_ms_of_iso("2026-08-19T03:00:00Z").unwrap();
        let answer = report(&ledger, Scope::All, Range::All, now, KST);
        let json = serde_json::to_value(&answer).unwrap();
        assert_eq!(json["summary"]["estimated_cost_usd"], 0.0);
        assert_eq!(json["summary"]["unpriced_requests"], 1);
        let unknown = answer
            .model_breakdown
            .iter()
            .find(|row| row.key == "synthetic-unpriced")
            .unwrap();
        assert_eq!(unknown.estimated_cost_usd, None);
        assert_eq!(unknown.unpriced_requests, 1);
    }

    /// The heaviest model is the one that moved the most tokens the vendor
    /// counted, not the most tokens that happened to be fresh.
    #[test]
    fn the_top_model_is_weighed_by_the_reported_total() {
        let mut cached_heavy = entry("cache", "2026-08-19T01:00:00Z", "/w/repo", 10);
        cached_heavy.model = Some("cache-heavy".to_string());
        cached_heavy.cached_input_tokens = 90;
        cached_heavy.output_tokens = 0;
        cached_heavy.total_tokens = 100;
        let mut fresh = entry("fresh", "2026-08-19T01:00:00Z", "/w/repo", 40);
        fresh.model = Some("fresh-input".to_string());
        fresh.cached_input_tokens = 0;
        fresh.output_tokens = 0;
        fresh.total_tokens = 40;

        let ledger = aggregate(vec![cached_heavy, fresh], &[managed()], KST);
        let now = epoch_ms_of_iso("2026-08-19T02:00:00Z").expect("stamp");
        let answer = report(&ledger, Scope::All, Range::All, now, KST);
        assert_eq!(answer.summary.top_model.as_deref(), Some("cache-heavy"));
        assert_eq!(
            answer.model_breakdown[0].key, "cache-heavy",
            "the list was ordered by the fresh tokens rather than the total"
        );
    }
}
