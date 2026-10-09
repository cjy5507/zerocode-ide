//! The sidebar's 「나를 기다림」 list (t-26595): which pane waits on the person,
//! and for what — a block, a question, or a finish nobody has looked at.
//!
//! The decisions are `zerocode_core::notify::waiting_for`, and a finish is listed
//! only where the device's finish setting would ring it. What lives here is the
//! lock around the rows and the names a click needs. The locks are taken one at
//! a time: every row is read first, and the names are read after.

use zerocode_core::notify::{Look, Waiting, turn_ms, waiting_for};

use crate::*;

/// One row of the list, as the sidebar paints it.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct WaitingRow {
    /// The pane's id in the board's own form (`term:<n>`), which a click opens.
    pub(crate) pane: String,
    /// The agent's slug (`claude`, `codex`, …).
    pub(crate) agent: &'static str,
    /// The worktree the pane works in, as the ledger knows it.
    pub(crate) worktree: String,
    /// `blocked`, `question` or `finished`.
    pub(crate) waiting: &'static str,
    /// When the pane entered the state it waits in, in epoch milliseconds.
    pub(crate) since_ms: i64,
}

/// The rank a kind sorts at: a pane that cannot go on without the person first.
const fn rank(waiting: Waiting) -> u8 {
    match waiting {
        Waiting::Blocked => 0,
        Waiting::Question => 1,
        Waiting::Finished => 2,
    }
}

/// The word a row keeps for a kind.
const fn word(waiting: Waiting) -> &'static str {
    match waiting {
        Waiting::Blocked => "blocked",
        Waiting::Question => "question",
        Waiting::Finished => "finished",
    }
}

/// Every pane that waits on the person now. Blocks and questions come first,
/// then finishes; each kind lists its newest first.
#[tauri::command]
pub(crate) fn waiting_on_me(state: State<'_, AppState>) -> Vec<WaitingRow> {
    let _crumb = crate::crumbs::Command::enter("waiting_on_me");
    let mode = load_settings_for_boot(state.settings())
        .document
        .notifications
        .agent_completion;
    let waiting: Vec<(TermId, Waiting, i64)> = {
        let states = state.pane_states();
        states
            .iter()
            .filter_map(|(term, held)| {
                let look = Look {
                    state: held.state,
                    asking: held.ask.is_some() || held.ask_prompt.is_some(),
                    mark: held.finish_mark && !held.interrupted && !held.session_boundary,
                    turn_ms: turn_ms(held.turn_started_at, held.state_started_at),
                };
                waiting_for(look, mode).map(|kind| (*term, kind, held.state_started_at))
            })
            .collect()
    };
    let mut rows: Vec<(Waiting, WaitingRow)> = waiting
        .into_iter()
        .map(|(term, kind, since_ms)| {
            let agent = state.agent_terms().get(&term).copied().unwrap_or("");
            let seat = state.last_status_seats().get(&term).cloned();
            let worktree = seat
                .and_then(|seat| {
                    state
                        .last_statuses()
                        .get(&seat)
                        .map(|held| held.worktree.clone())
                })
                .unwrap_or_default();
            let row = WaitingRow {
                pane: format!("term:{term}"),
                agent,
                worktree,
                waiting: word(kind),
                since_ms,
            };
            (kind, row)
        })
        .collect();
    rows.sort_by_key(|(kind, row)| (rank(*kind), std::cmp::Reverse(row.since_ms)));
    rows.into_iter().map(|(_, row)| row).collect()
}

/// The person looked at a pane, so its finish mark is released. True when a
/// mark stood and was released; a pane id that names no terminal releases nothing.
#[tauri::command]
pub(crate) fn clear_finish_mark(state: State<'_, AppState>, pane: String) -> bool {
    let _crumb = crate::crumbs::Command::enter("clear_finish_mark");
    let Some(term) = pane
        .strip_prefix("term:")
        .and_then(|id| id.parse::<TermId>().ok())
    else {
        return false;
    };
    let mut states = state.pane_states();
    match states.get_mut(&term) {
        Some(held) if held.finish_mark => {
            held.finish_mark = false;
            true
        }
        _ => false,
    }
}
