//! The notify seat at the window's bell (t-6043, `zerocode_core::jev::NOTIFY`):
//! one ring, asked of Jev at the point today's rule table decides "ring or
//! not", and the person's own hand as its label.
//!
//! [`pane_runtime::ring_now`] is the one ladder every bell walks — the
//! master switch, the kind, the watched screen, the cooldown — and the seat
//! is asked there, after the switches and the watched-screen rule have had
//! their say and before the cooldown: a watched screen is today's own
//! ignore and is not a question, and a cooldown is a rate, not a judgment.
//! Under `off`, `shadow`, a timeout, a refusal, or a lane's ring that has no
//! pane to label, the bell rings exactly as today ([`Call::today`]). Under
//! a person's `on`, or an `auto` the judge raised on this seat's own ledger,
//! the bell waits at
//! most [`NOTIFY_CALL_DEADLINE`] for the answer, and a `batch` or an `ignore`
//! then takes the ring away: `ignore` drops it, `batch` holds it in
//! [`NotifyBook`] until the person's next hand on the window, where every
//! held ring is folded into one notice (`notify::batched`). Either tells the
//! webview to hush the pane's attention mark ([`HUSH_EVENT`]).
//!
//! The row is written by whoever learns the outcome last: the bell, when it
//! took the answer inside the wall (`applied: true`); the asking thread,
//! when the bell had already gone on without it ([`Handoff`]). The label is
//! written by the hand — a key or a paste into the ring's pane inside
//! [`zerocode_core::jev::NOTIFY_LABEL_WINDOW_MS`] ([`note_hand`]) — or by the
//! sweep that closes the window ([`settle_labels`]); the mark itself is
//! `notify_call::agreed`'s.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{Value, json};
use zerocode_core::jev::door::{REDACTED_LINES_KEY, REQUESTS_KEY};
use zerocode_core::jev::summary::{AGREED, APPLIED, LABEL};
use zerocode_core::jev::{JevMode, NOTIFY, NOTIFY_APPLY_DEADLINE_MS, NOTIFY_RECENT_CAP};
use zerocode_core::notify::{self, Notice, Ring};
use zerocode_core::notify_call::{
    self, Attendance, Call, NOTIFY_CALL_RUBRIC_VERSION, NotifyAsk, NotifyLook, Recent,
};

use super::*;
use crate::systemone::{SCHEMA, Wire, request_body};

/// The wall the bell holds a ring for the seat's answer when the seat acts —
/// the row's own number, so the bell that waits and the judge that reads the
/// wait cannot disagree.
pub(crate) const NOTIFY_CALL_DEADLINE: Duration = Duration::from_millis(NOTIFY_APPLY_DEADLINE_MS);

/// The row's outcome for a question Jev answered in shape.
const ANSWERED: &str = "answered";

/// The event the window sends its webview when a seat's call took a ring
/// away: the pane and the call, so the tab strip can drop the attention mark
/// the hook already painted.
pub(crate) const HUSH_EVENT: &str = "notify:call";

/// The row key an answered row is named by, and a label row names it back
/// under [`LABEL`].
const KEY: &str = "notify";

/// What the webview hears under [`HUSH_EVENT`].
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Hushed {
    pub(crate) term: TermId,
    pub(crate) call: &'static str,
}

/// One ring the bell is about to decide on, as the ladder found it.
pub(crate) struct Bell<'a> {
    pub(crate) worktree: &'a str,
    /// The pane, when the ring has one — a lane's ring has none, and a ring
    /// with no pane has nothing a hand could land on, so it is not asked.
    pub(crate) term: Option<TermId>,
    pub(crate) agent: &'a str,
    pub(crate) ring: Ring,
    pub(crate) interrupted: bool,
    /// What the OS would show, composed before the ladder.
    pub(crate) notice: &'a Notice,
    /// Whether the main window has focus right now.
    pub(crate) focused: bool,
}

/// One earlier ring of a pane, as the book remembers it for the question.
struct Rang {
    at: i64,
    ring: Ring,
    interrupted: bool,
    /// Whether the person turned to the pane inside the label window;
    /// `None` while the window is open.
    reacted: Option<bool>,
}

/// An answered question waiting for the person's hand, or for its window
/// to close.
#[derive(Debug)]
struct Waiting {
    key: String,
    term: TermId,
    asked_ms: i64,
    call: Call,
    attendance: Attendance,
}

/// A ring the seat held under `batch`, until the person's next hand.
pub(crate) struct Held {
    pub(crate) worktree: String,
    pub(crate) term: Option<TermId>,
    pub(crate) ring: Ring,
    pub(crate) notice: Notice,
}

/// What this window remembers about the rings it asked about.
#[derive(Default)]
pub(crate) struct NotifyBook {
    /// When the person's hand was last on the window — a key or a paste
    /// into any pane — which is the attendance fact beside focus.
    last_hand_ms: Option<i64>,
    /// Each pane's last rings, newest last, at most [`NOTIFY_RECENT_CAP`].
    recent: HashMap<TermId, VecDeque<Rang>>,
    /// The answered rows without a label yet.
    waiting: Vec<Waiting>,
    /// The rings held under `batch`, oldest first.
    held: Vec<Held>,
    /// Whether the rows a window restart left unlabeled have been read off
    /// the ledger's tail and closed.
    swept_after_boot: bool,
}

/// Everything the question needs from the book, read under one lock.
struct Context {
    attendance: Attendance,
    since_last_ms: Option<i64>,
    recent: Vec<Recent>,
}

impl NotifyBook {
    /// The person's hand landed on `term` at `now_ms`.
    pub(crate) fn note_hand(&mut self, now_ms: i64) {
        self.last_hand_ms = Some(now_ms);
    }

    /// Read what the question carries about `term`, and remember this ring
    /// as the newest of its pane.
    fn look_at(
        &mut self,
        term: TermId,
        ring: Ring,
        interrupted: bool,
        focused: bool,
        now_ms: i64,
    ) -> Context {
        let attendance = Attendance::of(focused, self.last_hand_ms, now_ms);
        let rang = self.recent.entry(term).or_default();
        let since_last_ms = rang.back().map(|last| now_ms.saturating_sub(last.at));
        let recent = rang
            .iter()
            .map(|one| Recent {
                ring: one.ring,
                interrupted: one.interrupted,
                ago_ms: now_ms.saturating_sub(one.at),
                reacted: one.reacted,
            })
            .collect();
        rang.push_back(Rang {
            at: now_ms,
            ring,
            interrupted,
            reacted: None,
        });
        while rang.len() > NOTIFY_RECENT_CAP {
            rang.pop_front();
        }
        Context {
            attendance,
            since_last_ms,
            recent,
        }
    }

    /// The label rows a hand on `term` at `now_ms` writes: every waiting row
    /// of that pane whose window the hand is inside.
    fn labels_for_hand(&mut self, term: TermId, now_ms: i64) -> Vec<Value> {
        let mut labels = Vec::new();
        self.waiting.retain(|one| {
            if one.term != term || !notify_call::reacted_within(one.asked_ms, now_ms) {
                return true;
            }
            labels.push(label_row(one, true, now_ms));
            false
        });
        for one in &labels {
            self.mark_reacted(term, one["askedMs"].as_i64().unwrap_or_default(), true);
        }
        labels
    }

    /// The label rows the clock writes at `now_ms`: every waiting row whose
    /// window has closed with no hand inside it.
    fn labels_for_closed_windows(&mut self, now_ms: i64) -> Vec<Value> {
        let mut labels = Vec::new();
        self.waiting.retain(|one| {
            if !notify_call::window_closed(one.asked_ms, now_ms) {
                return true;
            }
            labels.push(label_row(one, false, now_ms));
            false
        });
        for one in &labels {
            let term = u32::try_from(one["term"].as_u64().unwrap_or_default()).unwrap_or_default();
            self.mark_reacted(term, one["askedMs"].as_i64().unwrap_or_default(), false);
        }
        labels
    }

    fn mark_reacted(&mut self, term: TermId, asked_ms: i64, reacted: bool) {
        if let Some(rang) = self.recent.get_mut(&term)
            && let Some(one) = rang.iter_mut().find(|one| one.at == asked_ms)
        {
            one.reacted = Some(reacted);
        }
    }

    /// The pane's rings the book still holds — what a test reads back.
    #[cfg(test)]
    fn rings_of(&self, term: TermId) -> Vec<(i64, Option<bool>)> {
        self.recent
            .get(&term)
            .map(|rang| rang.iter().map(|one| (one.at, one.reacted)).collect())
            .unwrap_or_default()
    }

    /// How many answered rows wait for a label.
    #[cfg(test)]
    fn waiting_len(&self) -> usize {
        self.waiting.len()
    }
}

/// The label row for one waiting row: the person's reaction, how long after
/// the ring, the attendance the ring was judged under, and the mark — when
/// the rule leaves one.
fn label_row(one: &Waiting, reacted: bool, now_ms: i64) -> Value {
    let mut label = json!({
        "at": now_ms,
        (LABEL.canonical): one.key,
        "term": one.term,
        "askedMs": one.asked_ms,
        "call": one.call.word(),
        "attendance": one.attendance.word(),
        "reacted": reacted,
        "afterMs": now_ms.saturating_sub(one.asked_ms),
    });
    if let Some(agreed) = notify_call::agreed(one.call, reacted, one.attendance) {
        label[AGREED.canonical] = json!(agreed);
    }
    label
}

/// One question on its way: the ring, the switch it was asked under, when,
/// and what it asks.
struct Question {
    key: String,
    term: TermId,
    /// The workspace the words come from — the pane's worktree, which the
    /// door asks consent for.
    workspace: String,
    agent: String,
    pane: String,
    ring: Ring,
    interrupted: bool,
    attendance: Attendance,
    mode: JevMode,
    asked_ms: i64,
    since_last_ms: Option<i64>,
    waiting_panes: usize,
    asked: NotifyAsk,
}

/// The row an asked question came to, and — for an answer — its wait for a
/// label.
type Settled = (Value, Option<Waiting>);

/// One value handed from the asking thread to the bell, or kept by the
/// thread when the bell has gone on without it.
///
/// A channel would lose the value sent in the instant between the bell's
/// timeout and its dropping of the receiver — a row written by nobody. Here
/// the two sides meet under one lock: the bell marks the slot abandoned when
/// it gives up, and a `give` that finds it abandoned hands the value back to
/// its own thread, which then writes the row itself.
pub(crate) struct Handoff<T> {
    slot: Mutex<Slot<T>>,
    ready: Condvar,
}

enum Slot<T> {
    Empty,
    Full(T),
    Abandoned,
}

impl<T> Handoff<T> {
    pub(crate) fn new() -> Self {
        Self {
            slot: Mutex::new(Slot::Empty),
            ready: Condvar::new(),
        }
    }

    /// Hand `value` to the waiting side; back to the caller when nobody is
    /// waiting any more.
    pub(crate) fn give(&self, value: T) -> Result<(), T> {
        let mut slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        if matches!(*slot, Slot::Abandoned) {
            return Err(value);
        }
        *slot = Slot::Full(value);
        self.ready.notify_all();
        Ok(())
    }

    /// Wait up to `most` for the value; past it, abandon the slot so the
    /// giver keeps what it has.
    pub(crate) fn take(&self, most: Duration) -> Option<T> {
        let deadline = Instant::now() + most;
        let mut slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Slot::Full(_) = *slot {
                let Slot::Full(value) = std::mem::replace(&mut *slot, Slot::Abandoned) else {
                    unreachable!("checked full");
                };
                return Some(value);
            }
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                *slot = Slot::Abandoned;
                return None;
            };
            let (held, _) = self
                .ready
                .wait_timeout(slot, left)
                .unwrap_or_else(PoisonError::into_inner);
            slot = held;
        }
    }

    /// Nobody will wait: the giver keeps what it has.
    pub(crate) fn abandon(&self) {
        *self.slot.lock().unwrap_or_else(PoisonError::into_inner) = Slot::Abandoned;
    }
}

/// What the bell does with the seat's answer, given whether the seat acts
/// and whether an answer arrived inside the wall: the answer's call when
/// both, today's otherwise — and whether the row says `applied`.
pub(crate) fn chosen(applies: bool, answered: Option<Call>) -> (Call, bool) {
    match answered {
        Some(call) if applies => (call, true),
        _ => (Call::today(), false),
    }
}

/// The call for one ring: today's, or the seat's when it acts and answered
/// in time. Asks nothing under `off`, for a lane, or without a ledger to
/// write; under `shadow` and an unraised `auto` asks off this thread and
/// rings today's way at once.
pub(crate) fn call_at_the_bell(app: &AppHandle, bell: &Bell<'_>) -> Call {
    let today = Call::today();
    let Some(term) = bell.term else {
        return today;
    };
    let wire = Wire::of_this_machine();
    let mode = NOTIFY.mode_in(&wire.settings_root());
    if !mode.asks() {
        return today;
    }
    let Some(ledger) = crate::systemone::ledger_of(&wire, &NOTIFY) else {
        return today;
    };
    let applies = crate::systemone::applies(&wire, &NOTIFY);
    let now_ms = crate::usage_runtime::epoch_ms_now();
    sweep_after_boot(app, &ledger, now_ms);
    let state = app.state::<AppState>();
    let waiting_panes = state
        .pane_states()
        .values()
        .filter(|held| held.state == zerocode_core::hook::HookState::NeedsAttention)
        .count();
    let context =
        state
            .notify_book()
            .look_at(term, bell.ring, bell.interrupted, bell.focused, now_ms);
    let pane = crate::pane_runtime::place_of(bell.worktree);
    let look = NotifyLook {
        ring: bell.ring,
        interrupted: bell.interrupted,
        agent: bell.agent,
        pane: &pane,
        attendance: context.attendance,
        words: &bell.notice.body,
        since_last_ms: context.since_last_ms,
        waiting_panes,
        recent: &context.recent,
    };
    let asked = notify_call::ask(&look);
    let question = Question {
        key: format!("{term}@{now_ms}"),
        term,
        workspace: bell.worktree.to_string(),
        agent: bell.agent.to_string(),
        pane,
        ring: bell.ring,
        interrupted: bell.interrupted,
        attendance: context.attendance,
        mode,
        asked_ms: now_ms,
        since_last_ms: context.since_last_ms,
        waiting_panes,
        asked,
    };
    let handoff = Arc::new(Handoff::new());
    let thread_handoff = Arc::clone(&handoff);
    let thread_app = app.clone();
    let thread_ledger = ledger.clone();
    let spawned = std::thread::Builder::new()
        .name("jev-notify-call".to_string())
        .spawn(move || {
            let settled = settle(&wire, question);
            if let Err(kept) = thread_handoff.give(settled) {
                // The bell went on without this answer: the row says so.
                let (mut row, waiting) = kept;
                row[APPLIED.canonical] = json!(false);
                record(&thread_app, &thread_ledger, row, waiting);
            }
        });
    if spawned.is_err() {
        return today;
    }
    if !applies {
        handoff.abandon();
        return today;
    }
    let Some((mut row, waiting)) = handoff.take(NOTIFY_CALL_DEADLINE) else {
        return today;
    };
    let (call, applied) = chosen(true, waiting.as_ref().map(|one| one.call));
    row[APPLIED.canonical] = json!(applied);
    record(app, &ledger, row, waiting);
    call
}

/// Ask one question and write down what came of it: the row, and — for an
/// answer — the wait for its label.
fn settle(wire: &Wire, question: Question) -> Settled {
    let Question {
        key,
        term,
        workspace,
        agent,
        pane,
        ring,
        interrupted,
        attendance,
        mode,
        asked_ms,
        since_last_ms,
        waiting_panes,
        asked,
    } = question;
    let mut row = json!({
        "at": asked_ms,
        KEY: key,
        "term": term,
        "pane": pane,
        "agent": agent,
        "ring": ring.word(),
        "event": notify::verb(ring, interrupted),
        "attendance": attendance.word(),
        "mode": mode.key(),
        "rubricVersion": NOTIFY_CALL_RUBRIC_VERSION,
        "today": Call::today().word(),
        "sinceLastMs": since_last_ms,
        "waitingPanes": waiting_panes,
    });
    let began = Instant::now();
    let answer = wire.ask(
        &NOTIFY,
        Some(Path::new(&workspace)),
        request_body(&asked.state, &asked.questions),
        NOTIFY_CALL_DEADLINE,
    );
    row["elapsedMs"] = json!(u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX));
    row["requestBytes"] = json!(answer.request_bytes);
    row[REQUESTS_KEY] = json!(answer.spent.requests);
    row[REDACTED_LINES_KEY] = json!(answer.spent.redacted_lines);
    let read = answer.answer.and_then(|body| {
        serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|parsed| asked.read(parsed.get("answers")?).ok())
            .ok_or_else(|| SCHEMA.to_string())
    });
    match read {
        Ok(choice) => {
            row["outcome"] = json!(ANSWERED);
            row["call"] = json!(choice.call.word());
            row["probabilities"] = json!(choice.probabilities);
            row["confidence"] = json!(choice.confidence);
            let waiting = Waiting {
                key,
                term,
                asked_ms,
                call: choice.call,
                attendance,
            };
            (row, Some(waiting))
        }
        Err(token) => {
            row["outcome"] = json!(token);
            (row, None)
        }
    }
}

/// Write one settled row and, for an answer, keep its wait for the hand —
/// and arm the clock that closes its window.
fn record(app: &AppHandle, ledger: &Path, row: Value, waiting: Option<Waiting>) {
    let now_ms = crate::usage_runtime::epoch_ms_now();
    crate::systemone::record_rows(&NOTIFY, ledger, &[row], now_ms);
    let Some(waiting) = waiting else {
        return;
    };
    app.state::<AppState>().notify_book().waiting.push(waiting);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(label_window_sleep()).await;
        settle_labels(&app);
    });
}

/// How long the clock sleeps before it closes a ring's label window: the
/// window and one more second, so the sweep never finds the window still
/// open by a millisecond.
fn label_window_sleep() -> Duration {
    Duration::from_millis(zerocode_core::jev::NOTIFY_LABEL_WINDOW_MS.unsigned_abs() + 1_000)
}

/// Close every label window the clock has passed: the rows nobody turned to.
pub(crate) fn settle_labels(app: &AppHandle) {
    let now_ms = crate::usage_runtime::epoch_ms_now();
    let labels = app
        .state::<AppState>()
        .notify_book()
        .labels_for_closed_windows(now_ms);
    write_labels(app, labels, now_ms);
}

/// The person's hand landed on `term`: the attendance clock moves, the
/// rings of that pane inside their window are labeled as reacted to, and
/// every ring held under `batch` is told now, as one notice.
///
/// Cheap on the hot path — a keystroke is one — because every file is
/// touched off it: the lock is taken once, and the ledger and the OS are
/// reached from a thread only when the book had something for them.
pub(crate) fn note_hand(app: &AppHandle, term: TermId) {
    let now_ms = crate::usage_runtime::epoch_ms_now();
    let (labels, held) = {
        let state = app.state::<AppState>();
        let mut book = state.notify_book();
        book.note_hand(now_ms);
        let labels = book.labels_for_hand(term, now_ms);
        let held = std::mem::take(&mut book.held);
        (labels, held)
    };
    write_labels(app, labels, now_ms);
    if !held.is_empty() {
        let app = app.clone();
        std::thread::spawn(move || flush_held(&app, held));
    }
}

/// Append label rows off the calling thread; nothing to write costs
/// nothing.
fn write_labels(app: &AppHandle, labels: Vec<Value>, now_ms: i64) {
    if labels.is_empty() {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let wire = Wire::of_this_machine();
        let Some(ledger) = crate::systemone::ledger_of(&wire, &NOTIFY) else {
            return;
        };
        crate::systemone::record_rows(&NOTIFY, &ledger, &labels, now_ms);
        drop(app);
    });
}

/// Tell the person once about every ring the seat held: the held notices
/// folded into one, rung through the ladder's own gates at the first held
/// ring's address, so the click comes back to a pane that waited.
fn flush_held(app: &AppHandle, held: Vec<Held>) {
    let Some(first) = held.first() else {
        return;
    };
    let notices: Vec<Notice> = held.iter().map(|one| one.notice.clone()).collect();
    let Some(folded) = notify::batched(&notices) else {
        return;
    };
    crate::pane_runtime::ring_held(app, &first.worktree, first.term, first.ring, &folded);
}

/// Hold one ring under `batch`, and hush its pane's mark.
pub(crate) fn hold(app: &AppHandle, bell: &Bell<'_>) {
    app.state::<AppState>().notify_book().held.push(Held {
        worktree: bell.worktree.to_string(),
        term: bell.term,
        ring: bell.ring,
        notice: bell.notice.clone(),
    });
    hush(app, bell.term, Call::Batch);
}

/// Tell the webview a seat's call took `term`'s ring away.
pub(crate) fn hush(app: &AppHandle, term: Option<TermId>, call: Call) {
    if let Some(term) = term {
        let _ = app.emit(
            HUSH_EVENT,
            Hushed {
                term,
                call: call.word(),
            },
        );
    }
}

/// The answered rows in the tail of `ledger` a window restart left without a
/// label, closed once: a restarted window did not see the person's hand, so
/// the row says the reaction is unknown and carries no mark.
fn sweep_after_boot(app: &AppHandle, ledger: &Path, now_ms: i64) {
    {
        let state = app.state::<AppState>();
        let mut book = state.notify_book();
        if book.swept_after_boot {
            return;
        }
        book.swept_after_boot = true;
    }
    let labels: Vec<Value> = unlabeled_in(ledger)
        .into_iter()
        .map(|(key, term, asked_ms)| {
            json!({
                "at": now_ms,
                (LABEL.canonical): key,
                "term": term,
                "askedMs": asked_ms,
                "reacted": Value::Null,
                "afterMs": now_ms.saturating_sub(asked_ms),
            })
        })
        .collect();
    write_labels(app, labels, now_ms);
}

/// The keys, panes and times of the answered rows in `ledger`'s tail that no
/// label row names yet.
fn unlabeled_in(ledger: &Path) -> Vec<(String, TermId, i64)> {
    let Some(lines) = zerocode_core::transcript::tail_lines(ledger) else {
        return Vec::new();
    };
    let rows: Vec<Value> = lines
        .iter()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    let labeled: HashSet<&str> = rows
        .iter()
        .filter_map(|row| LABEL.read(row).and_then(Value::as_str))
        .collect();
    rows.iter()
        .filter(|row| row["outcome"] == ANSWERED)
        .filter_map(|row| {
            let key = row[KEY].as_str()?;
            if labeled.contains(key) {
                return None;
            }
            Some((
                key.to_string(),
                u32::try_from(row["term"].as_u64()?).ok()?,
                row["at"].as_i64()?,
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests;
