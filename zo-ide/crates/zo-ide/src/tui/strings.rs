//! English phrase table for live Working-line and helper progress facts.
//!
//! The TUI does not have a locale subsystem. Keeping the new wording here
//! still gives every renderer and wire publisher one source of truth.
//!
//! None of these words is a tool name. `ACTIVITY_WAITING`,
//! `ACTIVITY_RECONNECTING` and `ACTIVITY_QUIET` are the verbs of
//! `session_status.activity` — status facts — and they never travel as a
//! `PreToolUse` `tool_name`: the window reads that hook as "the agent reached
//! for a tool" and drops a parked approval on it (t-2550).

use std::time::Duration;

use super::activity::Activity;
use super::shimmer::fmt_elapsed_compact;

pub const WORKING: &str = "Working";
/// The `/model` picker's description for a row its source stopped listing
/// while a session had it selected — the row stays, dimmed, with the local
/// clock of the first refresh that missed it (t-3054). Choosing it still
/// works; the wire decides.
pub const UNLISTED_SINCE: &str = "출처가 목록에서 뺐음";

/// `출처가 목록에서 뺐음 · <시각>` for one unlisted picker row.
#[must_use]
pub fn unlisted_since(when: &str) -> String {
    format!("{UNLISTED_SINCE} · {when}")
}
pub const ACTIVITY_WAITING: &str = "waiting";
pub const ACTIVITY_RECONNECTING: &str = "reconnecting";
pub const ACTIVITY_QUIET: &str = "quiet";
pub const ACTIVITY_REASONING_SILENTLY: &str = "reasoning silently";
pub const ACTIVITY_MODEL_TARGET: &str = "model";
pub const ACTIVITY_PHASE_STARTED: &str = "started";
/// The reason beside a call that never got its result — the turn was
/// cancelled, panicked, or was replayed without one. A row that says nothing
/// beside its ✘ read as the tool's own failure ("Failed to apply patch"
/// without a reason, 2026-09-07).
pub const TOOL_CALL_INTERRUPTED: &str = "interrupted before its result";

/// `… N more lines` — the lines of a cell the viewport did not draw: a created
/// file's tail in the transcript, or a live cell taller than the rows above
/// the composer (t-3063).
#[must_use]
pub fn more_lines(more: usize) -> String {
    format!("… {more} more lines")
}
/// The header of a message a peer agent sent with `SendMessage`.
pub const PEER_MESSAGE: &str = "Message";

/// `from <name>` — who sent a peer message, after the header's separator.
#[must_use]
pub fn peer_message_from(from: &str) -> String {
    format!("from {from}")
}

#[must_use]
pub fn current_tool(tool: &str, target: Option<&str>, elapsed: Duration) -> String {
    let elapsed = fmt_elapsed_compact(elapsed.as_secs());
    match target {
        Some(target) => format!("{tool} · {target} · {elapsed}"),
        None => format!("{tool} · {elapsed}"),
    }
}

#[must_use]
pub fn helper_wave(total: usize, running: usize) -> String {
    let done = total.saturating_sub(running);
    format!("agents {total} · running {running} · done {done}")
}

#[must_use]
pub fn helper(
    label: &str,
    tool_calls: u64,
    elapsed: Duration,
    activity: &Activity,
) -> String {
    let uses = core_types::helper_run::tool_uses(tool_calls);
    let elapsed = fmt_elapsed_compact(elapsed.as_secs());
    match activity.target.as_deref() {
        Some(target) => format!(
            "{label} · {uses} · {elapsed} · {} · {target}",
            activity.tool
        ),
        None => format!("{label} · {uses} · {elapsed} · {}", activity.tool),
    }
}

#[must_use]
pub fn waiting_for_model(elapsed: Duration) -> String {
    format!("waiting for the model {}", fmt_elapsed_compact(elapsed.as_secs()))
}

/// The stream is alive and the model is reasoning without visible output —
/// the status line's word for it, with its own clock.
#[must_use]
pub fn reasoning_silently(elapsed: Duration) -> String {
    format!("model reasoning silently · {}", fmt_elapsed_compact(elapsed.as_secs()))
}

#[must_use]
pub fn reconnecting(attempt: u32, seconds_left: u64) -> String {
    format!("reconnecting · {}", reconnecting_fact(attempt, seconds_left))
}

/// The retry's standing, as the `target` of a `reconnecting` status fact.
#[must_use]
pub fn reconnecting_fact(attempt: u32, seconds_left: u64) -> String {
    if seconds_left > 0 {
        format!("attempt {attempt} in {seconds_left}s")
    } else {
        format!("attempt {attempt}")
    }
}

/// How long nothing has happened and what happened last — the body of the
/// `quiet` line.
#[must_use]
pub fn quiet_fact(elapsed: Duration, last: Option<&Activity>) -> String {
    let elapsed = fmt_elapsed_compact(elapsed.as_secs());
    match last {
        Some(last) => format!("{elapsed} · {}", last_activity(last)),
        None => elapsed,
    }
}

#[must_use]
pub fn quiet(elapsed: Duration, last: Option<&Activity>) -> String {
    format!("quiet {}", quiet_fact(elapsed, last))
}

#[must_use]
pub fn last_activity(last: &Activity) -> String {
    match last.target.as_deref() {
        Some(target) => format!("last: {} {target}", last.tool),
        None => format!("last: {}", last.tool),
    }
}

/* ---- PushNotification's one transcript line (t-2943) ---- */

/// Where a `PushNotification` went and what it said — the line the person
/// scrolling back reads to know the bell they heard was this one. Every road
/// has a word, though only the delivered two are ever drawn.
#[must_use]
pub fn push_notification_line(road: runtime::message_stream::NotificationRoad, body: &str) -> String {
    use runtime::message_stream::NotificationRoad;
    let went = match road {
        NotificationRoad::Window => "창에 알림",
        NotificationRoad::Terminal => "터미널 벨",
        NotificationRoad::SkippedAttended => "알림 생략 · 자리에 있음",
        NotificationRoad::SkippedNowhere => "알림 생략 · 보낼 곳 없음",
    };
    format!("{went} · {body}")
}

/* ---- a teammate pane's own lines (t-2513 §2.2) ---- */

/// The first line a teammate pane shows: who sent it and what kind of helper
/// it is. `{parent}` is the parent session id.
#[must_use]
pub fn teammate_banner(parent: Option<&str>, kind: &str) -> String {
    match parent {
        Some(parent) => format!("부모 {parent} 가 맡긴 일 · {kind}"),
        None => format!("부모가 맡긴 일 · {kind}"),
    }
}

/// After a turn: the answer went up, and the pane waits for the parent's
/// next word. A person at the keyboard may close it.
/// The idle teammate's line — who it waits for and the way out by hand. The
/// coordinator's own way out is `StopAgent`; the line fits a sixty-column
/// pane, so it does not say so. The segments are split on ` · ` (the e2es
/// wait for the first one).
pub const TEAMMATE_IDLE: &str = "오케스트레이션 코디네이터 대기 중 · Esc 두 번이면 닫기";
/// A line a person typed into an idle teammate pane: the parent drives it.
pub const TEAMMATE_PARENT_DRIVES: &str = "이 판은 부모가 몬다 · 부모의 SendMessage 로 다음 턴이 열린다";
/// The parent's next word arrived and opens turn `n`.
#[must_use]
pub fn teammate_next_turn(turn: u32) -> String {
    format!("부모의 다음 지시 · 턴 {turn}")
}
/// The last line a teammate pane leaves on screen.
///
/// The pane STAYS after the process exits — the window keeps a closed pane's
/// scrollback — so this is the sentence somebody scrolling back reads to know
/// the child did not die: it finished and handed its answers up.
pub const TEAMMATE_CLOSING: &str = "끝 · 부모에게 돌려줌";
/// Why the pane is leaving, one line per reason.
#[must_use]
pub fn teammate_closing_reason(reason: runtime::subagent_panes::CloseReason) -> &'static str {
    use runtime::subagent_panes::CloseReason;
    match reason {
        CloseReason::ParentLost => "부모 채널이 사라져 닫는다",
        CloseReason::IdleBudget => "유휴 상한이 지나 닫는다",
        CloseReason::ClosedByParent => "부모가 닫았다",
        CloseReason::UserExit => "사람이 닫았다",
        CloseReason::LaneDone => "답이 부모에게 닿아 닫는다",
    }
}

/* ---- a background command's completion cell (t-3177) ---- */

/// The one dim word after the command in a background bash's `Ran` header —
/// the only glyphs that tell its completion cell from the cell the same
/// command draws in the foreground. Everything else (verb, bullet colour,
/// output preview, elision) is the foreground formatter's.
pub const COMMAND_BACKGROUND: &str = "· background";
