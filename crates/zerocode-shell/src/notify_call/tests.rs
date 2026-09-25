//! What the notify seat promises the bell and whoever reads its ledger back
//! (t-6043). Every case here crosses a loopback socket or none at all; no
//! case reads this machine's keychain, settings or ledgers.

use serde_json::json;
use zerocode_core::jev::door::{REQUESTS_KEY, Refused};
use zerocode_core::jev::{NOTIFY_LABEL_WINDOW_MS, SMART_SETTINGS_KEY};

use super::*;
use crate::systemone::tests::Endpoint;

/// zo's settings in a folder of the case's own, consenting to `consented`,
/// with the seat at `mode`. Answers the settings file's path.
fn settings_consenting_to(
    home: &tempfile::TempDir,
    consented: &str,
    mode: JevMode,
) -> std::path::PathBuf {
    let settings = home.path().join("settings.json");
    std::fs::write(
        &settings,
        json!({
            SMART_SETTINGS_KEY: {
                NOTIFY.setting: mode.key(),
                "jev": { "workspaces": [consented] },
            }
        })
        .to_string(),
    )
    .expect("zo's settings");
    settings
}

/// The endpoint's answer: `chosen`, with the rest of the room split over
/// the other two.
fn a_call_answer(chosen: &str) -> String {
    let mut spread = serde_json::Map::new();
    for call in Call::ALL {
        spread.insert(
            call.word().to_string(),
            json!(if call.word() == chosen { 0.8 } else { 0.1 }),
        );
    }
    json!({
        "model": "jev-1.13.0",
        "answers": {
            "call": {
                "type": "choice",
                "choice": chosen,
                "probabilities": spread,
                "confidence": 0.64,
            }
        },
        "usage": { "input_tokens": 300, "output_tokens": 0 },
    })
    .to_string()
}

/// One question about a completion on pane 7 of `workspace`.
fn a_question(workspace: &str, asked_ms: i64) -> Question {
    let look = NotifyLook {
        ring: Ring::Completion,
        interrupted: false,
        agent: "claude",
        pane: "api",
        attendance: Attendance::Present,
        words: "tests green; ready to merge",
        since_last_ms: Some(30_000),
        waiting_panes: 1,
        recent: &[],
    };
    Question {
        key: format!("7@{asked_ms}"),
        term: 7,
        workspace: workspace.to_string(),
        agent: "claude".to_string(),
        pane: "api".to_string(),
        ring: Ring::Completion,
        interrupted: false,
        attendance: Attendance::Present,
        mode: JevMode::Shadow,
        asked_ms,
        since_last_ms: Some(30_000),
        waiting_panes: 1,
        asked: notify_call::ask(&look),
    }
}

/// A handoff carries one value from the asking thread to the bell inside
/// the wall, hands it back to the giver past it, and never loses it in
/// between — the case a channel drops on the floor.
#[test]
fn a_handoff_never_loses_the_row() {
    let handoff = Handoff::new();
    assert_eq!(handoff.give(1), Ok(()));
    assert_eq!(handoff.take(Duration::from_millis(10)), Some(1));
    // Taken once: the slot is abandoned after, so a late second give is the
    // giver's to keep.
    assert_eq!(handoff.give(2), Err(2));

    let waited = Handoff::new();
    let began = Instant::now();
    assert_eq!(waited.take(Duration::from_millis(20)), None);
    assert!(began.elapsed() >= Duration::from_millis(20));
    assert_eq!(waited.give(3), Err(3), "past the wall the bell has gone on");

    let shadow = Handoff::new();
    shadow.abandon();
    assert_eq!(shadow.give(4), Err(4), "nobody waits under shadow");

    // Across threads: the bell takes what the thread gives inside the wall.
    let shared = Arc::new(Handoff::new());
    let giver = Arc::clone(&shared);
    let thread = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(5));
        giver.give(5)
    });
    assert_eq!(shared.take(Duration::from_secs(2)), Some(5));
    assert_eq!(thread.join().expect("the giver"), Ok(()));
}

/// The bell acts on an answer only when the seat acts AND the answer came:
/// off, shadow, an unraised auto, a timeout and a refusal are today's rule.
/// And where the seat's labels drew an act line (t-9468), only an answer
/// that reaches it: one under it rings today's way, as an unanswered ring
/// does; with no line every answer is acted on, as before.
#[test]
fn the_bell_rings_todays_way_unless_the_seat_acts_and_answered() {
    for call in Call::ALL {
        assert_eq!(chosen(true, Some((call, 0.1)), None), (call, true));
        assert_eq!(
            chosen(false, Some((call, 0.9)), None),
            (Call::today(), false)
        );
        assert_eq!(chosen(true, Some((call, 0.7)), Some(700)), (call, true));
        assert_eq!(
            chosen(true, Some((call, 0.69)), Some(700)),
            (Call::today(), false),
            "under the line the labels drew"
        );
    }
    assert_eq!(chosen(true, None, None), (Call::today(), false));
    assert_eq!(chosen(false, None, Some(700)), (Call::today(), false));
    assert_eq!(Call::today(), Call::Interrupt);
}

/// The book: attendance from the last hand, the pane's last rings capped
/// and oldest first, and the interval since the pane last rang.
#[test]
fn the_book_remembers_a_panes_last_rings_and_the_persons_last_hand() {
    let mut book = NotifyBook::default();
    let now = 1_000_000;
    let first = book.look_at(7, Ring::Completion, false, true, now);
    assert_eq!(first.attendance, Attendance::Away, "no hand yet");
    assert_eq!(first.since_last_ms, None);
    assert!(first.recent.is_empty());

    book.note_hand(now + 1_000);
    let second = book.look_at(7, Ring::Attention, false, true, now + 5_000);
    assert_eq!(second.attendance, Attendance::Present);
    assert_eq!(second.since_last_ms, Some(5_000));
    assert_eq!(second.recent.len(), 1);
    assert_eq!(second.recent[0].ring, Ring::Completion);
    assert_eq!(second.recent[0].ago_ms, 5_000);
    assert_eq!(second.recent[0].reacted, None);

    // Another pane's rings are its own.
    let other = book.look_at(8, Ring::Push, false, true, now + 6_000);
    assert!(other.recent.is_empty());

    // The cap holds the newest.
    for step in 0..(NOTIFY_RECENT_CAP as i64 + 3) {
        book.look_at(7, Ring::Completion, false, false, now + 10_000 + step);
    }
    let rings = book.rings_of(7);
    assert_eq!(rings.len(), NOTIFY_RECENT_CAP);
    assert_eq!(
        rings.last().map(|(at, _)| *at),
        Some(now + 10_000 + NOTIFY_RECENT_CAP as i64 + 2)
    );

    // Away after the attendance window, however focused the window.
    let late = book.look_at(
        7,
        Ring::Completion,
        false,
        true,
        now + 1_000 + zerocode_core::jev::NOTIFY_ATTENDANCE_WINDOW_MS + 1,
    );
    assert_eq!(late.attendance, Attendance::Away);
}

fn waiting(term: TermId, asked_ms: i64, call: Call, attendance: Attendance) -> Waiting {
    Waiting {
        key: format!("{term}@{asked_ms}"),
        term,
        asked_ms,
        call,
        confidence: 0.9,
        attendance,
    }
}

/// A hand on the pane inside the minute labels every waiting row of that
/// pane as reacted — `agreed` iff the call rang — and leaves the other
/// panes' rows waiting.
#[test]
fn a_hand_inside_the_minute_labels_the_panes_rows_as_reacted() {
    let mut book = NotifyBook::default();
    let asked = 5_000_000;
    book.look_at(7, Ring::Attention, false, true, asked);
    book.waiting
        .push(waiting(7, asked, Call::Interrupt, Attendance::Present));
    book.waiting
        .push(waiting(7, asked + 1, Call::Batch, Attendance::Away));
    book.waiting
        .push(waiting(8, asked, Call::Ignore, Attendance::Present));

    // A hand on the line: the window is closed on the far side.
    let hand = asked + NOTIFY_LABEL_WINDOW_MS;
    let labels = book.labels_for_hand(7, hand);
    assert_eq!(labels.len(), 2);
    let by_key = |key: &str| {
        labels
            .iter()
            .find(|row| row[LABEL.canonical] == key)
            .unwrap_or_else(|| panic!("a label for {key}"))
    };
    let rang = by_key(&format!("7@{asked}"));
    assert_eq!(rang["reacted"], true);
    assert_eq!(rang[AGREED.canonical], true, "it rang and the person came");
    assert_eq!(rang["afterMs"], NOTIFY_LABEL_WINDOW_MS);
    assert_eq!(rang["attendance"], "present");
    let held = by_key(&format!("7@{}", asked + 1));
    assert_eq!(held["reacted"], true);
    assert_eq!(
        held[AGREED.canonical], false,
        "it held and the person came anyway"
    );
    assert_eq!(book.waiting_len(), 1, "pane 8 still waits");
    assert_eq!(book.rings_of(7), vec![(asked, Some(true))]);

    // A hand past the window labels nothing by hand.
    let mut late = NotifyBook::default();
    late.waiting
        .push(waiting(7, asked, Call::Interrupt, Attendance::Present));
    assert!(
        late.labels_for_hand(7, asked + NOTIFY_LABEL_WINDOW_MS + 1)
            .is_empty()
    );
    assert_eq!(late.waiting_len(), 1);
}

/// The clock closes the windows nobody's hand landed in: present says the
/// call was right iff it did not ring; away leaves no mark, and says why —
/// the person was away (t-9427): 325 of this machine's 460 label rows
/// (2026-09-26) compared nothing and named no reason, so the judge counted
/// none of them as rows that compare nothing.
#[test]
fn a_closed_window_labels_the_rows_nobody_turned_to() {
    let mut book = NotifyBook::default();
    let asked = 5_000_000;
    book.look_at(7, Ring::Completion, false, true, asked);
    book.waiting
        .push(waiting(7, asked, Call::Interrupt, Attendance::Present));
    book.waiting
        .push(waiting(8, asked, Call::Batch, Attendance::Present));
    book.waiting
        .push(waiting(9, asked, Call::Ignore, Attendance::Away));
    book.waiting
        .push(waiting(9, asked + 30_000, Call::Ignore, Attendance::Away));

    assert!(
        book.labels_for_closed_windows(asked + NOTIFY_LABEL_WINDOW_MS)
            .is_empty(),
        "on the line the window is still open"
    );
    let labels = book.labels_for_closed_windows(asked + NOTIFY_LABEL_WINDOW_MS + 1);
    assert_eq!(labels.len(), 3, "the newest row's window is still open");
    let by_term = |term: u64| {
        labels
            .iter()
            .find(|row| row["term"] == term)
            .unwrap_or_else(|| panic!("a label for pane {term}"))
    };
    assert_eq!(by_term(7)["reacted"], false);
    assert_eq!(by_term(7)[AGREED.canonical], false, "it rang for nothing");
    assert_eq!(by_term(8)[AGREED.canonical], true, "it held, rightly");
    assert!(
        by_term(9).get(AGREED.canonical).is_none(),
        "away says nothing either way"
    );
    assert!(by_term(9).get(BASELINE_AGREED.canonical).is_none());
    assert_eq!(
        by_term(9)[zerocode_core::jev::summary::NOT_COMPARED.canonical],
        Attendance::Away.word(),
        "and says why"
    );
    assert_eq!(book.waiting_len(), 1);
    assert_eq!(book.rings_of(7), vec![(asked, Some(false))]);
}

/// A question crosses the socket with the door's bytes and comes back as an
/// answered row carrying the call, the numbers and the wait for its label —
/// and under shadow the bell still rings today's way with that row beside
/// it.
#[test]
fn an_answered_question_is_a_row_and_a_wait_and_shadow_changes_nothing() {
    let work = tempfile::tempdir().expect("a checkout");
    let home = tempfile::tempdir().expect("a zo home");
    let workspace = work.path().display().to_string();
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", a_call_answer("batch"), 0);
    let wire = Wire::at(
        &endpoint.base(),
        "key",
        Some(settings_consenting_to(&home, &workspace, JevMode::Shadow)),
    );
    let asked_ms = 1_789_600_000_000;
    let (row, waiting) = settle(&wire, a_question(&workspace, asked_ms));
    assert_eq!(row["outcome"], ANSWERED);
    assert_eq!(row["call"], "batch");
    assert_eq!(row["today"], "interrupt");
    assert_eq!(row[KEY], format!("7@{asked_ms}"));
    assert_eq!(row["term"], 7);
    assert_eq!(row["ring"], "completion");
    assert_eq!(row["event"], "finished");
    assert_eq!(row["attendance"], "present");
    assert_eq!(row["mode"], "shadow");
    assert_eq!(row["rubricVersion"], NOTIFY_CALL_RUBRIC_VERSION);
    assert_eq!(row[REQUESTS_KEY], 1);
    assert_eq!(
        row[zerocode_core::jev::summary::MODEL.canonical],
        "jev-1.13.0",
        "the version that answered"
    );
    assert_eq!(row["confidence"], 0.64);
    assert!(row["requestBytes"].as_u64().is_some_and(|bytes| bytes > 0));
    let waiting = waiting.expect("an answer waits for its label");
    assert_eq!(waiting.call, Call::Batch);
    assert_eq!(waiting.term, 7);
    assert_eq!(waiting.asked_ms, asked_ms);
    // What left: the door's bytes, one request, with the state the question
    // built and nothing else a person wrote.
    let sent = endpoint.asked();
    assert_eq!(sent.len(), 1);
    let body: Value =
        serde_json::from_str(sent[0].split("\r\n\r\n").nth(1).expect("a body")).expect("json");
    assert_eq!(body["state"]["event"], "finished");
    assert_eq!(body["state"]["pane"], "api");
    assert_eq!(body["state"]["words"], "tests green; ready to merge");
    assert_eq!(body["questions"]["call"]["type"], "choice");
    // Shadow: the answer is a row, never an order.
    assert_eq!(
        chosen(false, Some((waiting.call, waiting.confidence)), None),
        (Call::Interrupt, false)
    );
}

/// A refusal, a wire failure and an answer out of shape are rows with the
/// wire's word and no wait — and today's call.
#[test]
fn a_refused_or_broken_answer_is_a_row_with_no_wait() {
    let work = tempfile::tempdir().expect("a checkout");
    let home = tempfile::tempdir().expect("a zo home");
    let workspace = work.path().display().to_string();
    let asked_ms = 1_789_600_000_000;

    // No consent for the words' workspace: the door refuses, nothing leaves.
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", a_call_answer("ignore"), 0);
    let stranger = tempfile::tempdir().expect("another checkout");
    let wire = Wire::at(
        &endpoint.base(),
        "key",
        Some(settings_consenting_to(
            &home,
            &stranger.path().display().to_string(),
            JevMode::Shadow,
        )),
    );
    let (row, waiting) = settle(&wire, a_question(&workspace, asked_ms));
    assert_eq!(row["outcome"], Refused::NotConsented.token());
    assert!(waiting.is_none());
    assert_eq!(row[REQUESTS_KEY], 0);
    assert!(
        endpoint.asked().is_empty(),
        "a refused request never leaves"
    );

    // An answer naming an option nobody offered is a schema row.
    let broken = Endpoint::serving("HTTP/1.1 200 OK", a_call_answer("snooze"), 0);
    let wire = Wire::at(
        &broken.base(),
        "key",
        Some(settings_consenting_to(&home, &workspace, JevMode::Auto)),
    );
    let (row, waiting) = settle(&wire, a_question(&workspace, asked_ms + 1));
    assert_eq!(row["outcome"], SCHEMA);
    assert!(waiting.is_none());
    assert_eq!(row[REQUESTS_KEY], 1);

    // The wire's own refusal keeps its word.
    let down = Endpoint::serving("HTTP/1.1 503 Service Unavailable", "{}".to_string(), 0);
    let wire = Wire::at(
        &down.base(),
        "key",
        Some(settings_consenting_to(&home, &workspace, JevMode::Auto)),
    );
    let (row, waiting) = settle(&wire, a_question(&workspace, asked_ms + 2));
    assert_eq!(row["outcome"], "http_503");
    assert!(waiting.is_none());
    assert_eq!(chosen(true, None, None), (Call::Interrupt, false));
}

/// An answer slower than the wall is a timeout row, and the bell has rung
/// today's way long before it lands.
#[test]
fn an_answer_past_the_wall_is_a_timeout_and_the_bell_rang_todays_way() {
    let work = tempfile::tempdir().expect("a checkout");
    let home = tempfile::tempdir().expect("a zo home");
    let workspace = work.path().display().to_string();
    let hold_ms = NOTIFY_APPLY_DEADLINE_MS + 400;
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", a_call_answer("ignore"), hold_ms);
    let wire = Wire::at(
        &endpoint.base(),
        "key",
        Some(settings_consenting_to(&home, &workspace, JevMode::Auto)),
    );
    let handoff = Arc::new(Handoff::new());
    let giver = Arc::clone(&handoff);
    let question = a_question(&workspace, 1_789_600_000_000);
    let thread = std::thread::spawn(move || {
        let settled = settle(&wire, question);
        giver.give(settled)
    });
    let began = Instant::now();
    // The wire's own wall and the bell's are the same number, so the timeout
    // row may land a hair before the bell gives up or a hair after: either
    // way the bell is back inside the wall with today's call, and the row —
    // whichever side kept it — says `timeout` and waits for no label.
    let taken = handoff.take(NOTIFY_CALL_DEADLINE);
    assert!(began.elapsed() < Duration::from_millis(hold_ms));
    let (row, waiting) = match taken {
        Some(settled) => {
            thread
                .join()
                .expect("the asking thread")
                .expect("the bell took it");
            settled
        }
        None => thread
            .join()
            .expect("the asking thread")
            .expect_err("the bell had gone on; the thread keeps its row"),
    };
    assert_eq!(row["outcome"], crate::systemone::TIMEOUT);
    assert!(waiting.is_none());
    assert_eq!(chosen(true, None, None), (Call::Interrupt, false));
}

/// A window that restarts reads the rows it left without a label back off
/// the ledger's tail: answered ones only, and none a label already names.
#[test]
fn a_restart_finds_the_answers_still_waiting_for_their_label() {
    let dir = tempfile::tempdir().expect("a zo home");
    let ledger = dir.path().join(NOTIFY.ledger);
    let row = |key: &str, term: u32, outcome: &str| json!({ "at": 5, KEY: key, "term": term, "outcome": outcome });
    crate::systemone::append_rows(
        &ledger,
        &[
            row("7@1", 7, ANSWERED),
            row("7@2", 7, "not_consented"),
            row("8@3", 8, ANSWERED),
            json!({ "at": 9, (LABEL.canonical): "7@1", "reacted": true }),
        ],
    );
    assert_eq!(unlabeled_in(&ledger), vec![("8@3".to_string(), 8, 5)]);
    assert!(unlabeled_in(&dir.path().join("gone.jsonl")).is_empty());
}

/// Off is today's bytes: the bell asks the seat only where today's table
/// would ring, falls through to the same last rungs on `interrupt`, and the
/// seat itself answers today's call before it reads anything but the switch.
#[test]
fn off_is_todays_bytes_and_the_seat_sits_where_the_table_decides() {
    let bell = include_str!("../pane_runtime.rs");
    let ringing = crate::tests::block_after(bell, "fn ring_now(");
    let suppressed = ringing
        .find("zerocode_core::notify::suppressed(worktree, &active, focused)")
        .expect("the watched-screen rule");
    let asked = ringing
        .find("notify_call::call_at_the_bell(app, &bell)")
        .expect("the seat is asked in the ladder");
    let watched_return = ringing
        .find("if watched {\n        return;\n    }")
        .expect("the watched screen still returns");
    let last_rungs = ringing
        .find("ring_composed(app, worktree, term, &composed);")
        .expect("the last rungs");
    assert!(
        suppressed < asked && asked < watched_return && watched_return < last_rungs,
        "the seat sits after the switches and the watched-screen rule and before the cooldown:\n{ringing}"
    );
    assert!(
        ringing.contains("if !watched {\n        let bell = notify_call::Bell {"),
        "a watched screen is not a question:\n{ringing}"
    );
    assert!(
        ringing.contains("zerocode_core::notify_call::Call::Interrupt => {}"),
        "interrupt falls through to today's rungs:\n{ringing}"
    );
    let composed = crate::tests::block_after(bell, "fn ring_composed(");
    assert!(
        composed.contains("may_ring(worktree, epoch_ms_now())")
            && composed.contains("show_notice(app, composed)")
            && composed.contains("*state.last_ring() = Some(LastRing {"),
        "the last rungs are the cooldown, the OS and the address:\n{composed}"
    );
    let held = crate::tests::block_after(bell, "fn ring_held(");
    assert!(
        !held.contains("call_at_the_bell"),
        "a held ring is not asked of the seat again:\n{held}"
    );

    let seat = include_str!("../notify_call.rs");
    let calling = crate::tests::block_after(seat, "pub(crate) fn call_at_the_bell(");
    let switch = calling
        .find("if !mode.asks() {")
        .expect("the switch is read");
    for later in [
        "crate::systemone::ledger_of(&wire, &NOTIFY)",
        "crate::systemone::applies(&wire, &NOTIFY)",
        ".look_at(term, bell.ring, bell.interrupted, bell.focused, now_ms)",
        "std::thread::Builder::new()",
    ] {
        let at = calling
            .find(later)
            .unwrap_or_else(|| panic!("{later} is gone"));
        assert!(
            switch < at,
            "under off, {later} runs before the switch is read:\n{calling}"
        );
    }
    assert!(
        calling.contains("if !applies {\n        handoff.abandon();\n        return today;\n    }"),
        "shadow rings today's way at once:\n{calling}"
    );
}

/* ---- the replay: this machine's rings, asked of the real endpoint ---- */

/// The seed `tools/notify-replay/seed.py` wrote.
const SEED_ENV: &str = "ZEROCODE_NOTIFY_REPLAY_SEED";
/// How many times each ring is asked (default 1): passes beyond the first
/// measure the seat's repeatability, not more evidence.
const RUNS_ENV: &str = "ZEROCODE_NOTIFY_REPLAY_RUNS";
/// At most this many rings, oldest first, when the whole seed is too much.
const ROWS_ENV: &str = "ZEROCODE_NOTIFY_REPLAY_ROWS";
/// How many rings are asked at once (default 4): one socket each.
const LANES_ENV: &str = "ZEROCODE_NOTIFY_REPLAY_LANES";
const LANES_DEFAULT: usize = 4;

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReplaySeed {
    label_window_ms: i64,
    attendance_window_ms: i64,
    recent_cap: usize,
    words_char_cap: usize,
    rows: Vec<ReplayRow>,
}

#[derive(serde::Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ReplayRow {
    pane: String,
    agent: String,
    verb: String,
    interrupted: bool,
    attendance: String,
    words: String,
    since_last_ms: Option<i64>,
    waiting_panes: usize,
    recent: Vec<ReplayRecent>,
    label: ReplayLabel,
}

#[derive(serde::Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ReplayRecent {
    verb: String,
    interrupted: bool,
    ago_ms: i64,
    reacted: Option<bool>,
}

#[derive(serde::Deserialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
struct ReplayLabel {
    reacted: bool,
}

/// One ring's outcome in one pass.
struct Outcome {
    index: usize,
    call: Option<Call>,
    confidence: f64,
    elapsed_ms: u64,
    input_tokens: u64,
    refusal: Option<String>,
}

fn ask_one(wire: &Wire, workspace: &Path, row: &ReplayRow) -> Option<(Call, f64, u64)> {
    let ring = notify::from_verb(&row.verb)?;
    let recent: Vec<Recent> = row
        .recent
        .iter()
        .filter_map(|one| {
            Some(Recent {
                ring: notify::from_verb(&one.verb)?,
                interrupted: one.interrupted,
                ago_ms: one.ago_ms,
                reacted: one.reacted,
            })
        })
        .collect();
    let look = NotifyLook {
        ring,
        interrupted: row.interrupted,
        agent: &row.agent,
        pane: &row.pane,
        attendance: Attendance::from_word(&row.attendance)?,
        words: &row.words,
        since_last_ms: row.since_last_ms,
        waiting_panes: row.waiting_panes,
        recent: &recent,
    };
    let asked = notify_call::ask(&look);
    let answer = wire.ask(
        &NOTIFY,
        Some(workspace),
        request_body(&asked.state, &asked.questions),
        NOTIFY_CALL_DEADLINE,
    );
    let body = answer.answer.ok()?;
    let parsed: Value = serde_json::from_str(&body).ok()?;
    let read = asked.read(parsed.get("answers")?).ok()?;
    let tokens = parsed["usage"]["input_tokens"].as_u64().unwrap_or_default();
    Some((read.call, read.confidence, tokens))
}

/// One agreement's share, as a line prints it.
fn share_of(held: &zerocode_core::jev::promote::Agreement) -> String {
    match held.compared {
        0 => "—".to_string(),
        #[allow(clippy::cast_precision_loss)]
        compared => format!("{:.1}%", held.agreed as f64 / compared as f64 * 100.0),
    }
}

/// One agreement's 95% Wilson lower bound, as a line prints it.
fn bound_of(held: &zerocode_core::jev::promote::Agreement) -> String {
    held.lower_bound()
        .map_or_else(|| "—".to_string(), |bound| format!("{:.1}%", bound * 100.0))
}

fn percentile_of(sorted: &[u64], share: f64) -> String {
    zerocode_core::jev::summary::percentile(sorted, share)
        .map_or_else(|| "—".to_string(), |held| held.to_string())
}

/// Every ring this machine's transcripts recorded, put to the real endpoint
/// as the shipped question, and read against the person's own hand: what
/// today's rule and the seat each would have done, how often each named
/// what the hand then said, the seat's latency, and what it cost.
///
/// Every row is asked with only what the bell knew at its own clock: the
/// seed carries no fact from after a ring except its label, and the label
/// is read after the answer. The person's ledger and day count are never
/// touched — the question leaves from a temporary home. The key comes from
/// the environment (`tools/notify-replay/README.md`).
#[test]
#[ignore = "crosses the real endpoint; run by hand with the seed and the key in the environment"]
fn the_calls_this_machine_would_have_made() {
    let seed_at = std::env::var(SEED_ENV)
        .unwrap_or_else(|_| panic!("{SEED_ENV} names the seed tools/notify-replay/seed.py wrote"));
    let seed: ReplaySeed =
        serde_json::from_str(&std::fs::read_to_string(&seed_at).expect("the seed reads"))
            .expect("the seed's shape");
    assert_eq!(
        (
            seed.label_window_ms,
            seed.attendance_window_ms,
            seed.recent_cap,
            seed.words_char_cap
        ),
        (
            NOTIFY_LABEL_WINDOW_MS,
            zerocode_core::jev::NOTIFY_ATTENDANCE_WINDOW_MS,
            NOTIFY_RECENT_CAP,
            zerocode_core::jev::NOTIFY_WORDS_CHAR_CAP
        ),
        "a seed made for another window is refused"
    );
    let work = tempfile::tempdir().expect("a checkout");
    let home = tempfile::tempdir().expect("a zo home of this measurement's own");
    let key = std::env::var(zerocode_harness::TYPESAFE_API_KEY_ENV).unwrap_or_else(|_| {
        panic!(
            "{} carries this machine's TypeSafe key",
            zerocode_harness::TYPESAFE_API_KEY_ENV
        )
    });
    let wire = Wire::at(
        crate::systemone::SYSTEMONE_BASE_URL,
        &key,
        Some(settings_consenting_to(
            &home,
            &work.path().display().to_string(),
            JevMode::Shadow,
        )),
    );
    let runs: usize = std::env::var(RUNS_ENV)
        .ok()
        .and_then(|said| said.parse().ok())
        .unwrap_or(1)
        .max(1);
    let most: usize = std::env::var(ROWS_ENV)
        .ok()
        .and_then(|said| said.parse().ok())
        .unwrap_or(usize::MAX);
    let lanes: usize = std::env::var(LANES_ENV)
        .ok()
        .and_then(|said| said.parse().ok())
        .unwrap_or(LANES_DEFAULT)
        .max(1);
    let rows: Vec<ReplayRow> = seed.rows.into_iter().take(most).collect();
    let rate = model_prices::systemone_rate(crate::systemone::SYSTEMONE_MODEL);
    println!(
        "seed={seed_at} rows={} runs={runs} lanes={lanes} wall={}ms",
        rows.len(),
        NOTIFY_APPLY_DEADLINE_MS
    );

    // Per pass: the seat's hit on the rows the person turned to, the seat's
    // own mark, today's mark under the same label, and the confusion.
    let mut hit_by_pass = vec![zerocode_core::jev::promote::Agreement::default(); runs];
    let mut mark_by_pass = vec![zerocode_core::jev::promote::Agreement::default(); runs];
    let mut todays_mark = zerocode_core::jev::promote::Agreement::default();
    let mut calls: std::collections::BTreeMap<(String, &'static str), usize> =
        std::collections::BTreeMap::new();
    let mut confusion: std::collections::BTreeMap<(String, &'static str), usize> =
        std::collections::BTreeMap::new();
    let mut confident: std::collections::BTreeMap<&'static str, (usize, f64)> =
        std::collections::BTreeMap::new();
    let mut first_call: Vec<Option<Call>> = vec![None; rows.len()];
    let mut repeated = zerocode_core::jev::promote::Agreement::default();
    let mut elapsed = Vec::new();
    let mut refusals: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let mut input_tokens = 0u64;
    let mut asked = 0usize;
    let shared_rows = Arc::new(rows.clone());
    for pass in 0..runs {
        let mut handles = Vec::new();
        for lane in 0..lanes {
            let rows = Arc::clone(&shared_rows);
            let wire = wire.clone();
            let workspace = work.path().to_path_buf();
            handles.push(std::thread::spawn(move || {
                let mut held = Vec::new();
                for (index, row) in rows.iter().enumerate() {
                    if index % lanes != lane {
                        continue;
                    }
                    let began = Instant::now();
                    let answered = ask_one(&wire, &workspace, row);
                    let took = u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX);
                    held.push(match answered {
                        Some((call, confidence, tokens)) => Outcome {
                            index,
                            call: Some(call),
                            confidence,
                            elapsed_ms: took,
                            input_tokens: tokens,
                            refusal: None,
                        },
                        None => Outcome {
                            index,
                            call: None,
                            confidence: 0.0,
                            elapsed_ms: took,
                            input_tokens: 0,
                            refusal: Some(if took >= NOTIFY_APPLY_DEADLINE_MS {
                                crate::systemone::TIMEOUT.to_string()
                            } else {
                                "refused_or_schema".to_string()
                            }),
                        },
                    });
                }
                held
            }));
        }
        let mut outcomes: Vec<Outcome> = handles
            .into_iter()
            .flat_map(|handle| handle.join().expect("a lane"))
            .collect();
        outcomes.sort_by_key(|one| one.index);
        for one in outcomes {
            asked += 1;
            let row = &rows[one.index];
            let Some(call) = one.call else {
                *refusals.entry(one.refusal.unwrap_or_default()).or_default() += 1;
                continue;
            };
            elapsed.push(one.elapsed_ms);
            input_tokens += one.input_tokens;
            let reacted = row.label.reacted;
            let attendance = Attendance::from_word(&row.attendance).expect("a seed attendance");
            *calls.entry((row.verb.clone(), call.word())).or_default() += 1;
            let label = format!(
                "{}/{}",
                if reacted { "reacted" } else { "quiet" },
                row.attendance
            );
            *confusion.entry((label, call.word())).or_default() += 1;
            let held = confident.entry(call.word()).or_default();
            held.0 += 1;
            held.1 += one.confidence;
            if reacted {
                hit_by_pass[pass].compared += 1;
                hit_by_pass[pass].agreed += usize::from(call.rings());
            }
            if let Ok(mark) = notify_call::agreed(call, reacted, attendance) {
                mark_by_pass[pass].compared += 1;
                mark_by_pass[pass].agreed += usize::from(mark);
            }
            if pass == 0 {
                if let Ok(mark) = notify_call::agreed(Call::today(), reacted, attendance) {
                    todays_mark.compared += 1;
                    todays_mark.agreed += usize::from(mark);
                }
                first_call[one.index] = Some(call);
            } else if let Some(first) = first_call[one.index] {
                repeated.compared += 1;
                repeated.agreed += usize::from(first == call);
            }
        }
    }
    elapsed.sort_unstable();
    let answered = elapsed.len();
    println!("\nasked={asked} answered={answered} refusals={refusals:?}");
    println!("\n| verb | interrupt | batch | ignore | today |");
    println!("| --- | --- | --- | --- | --- |");
    let verbs: std::collections::BTreeSet<&str> =
        rows.iter().map(|row| row.verb.as_str()).collect();
    for verb in verbs {
        let of = |call: &str| {
            calls
                .get(&(verb.to_string(), call))
                .copied()
                .unwrap_or_default()
        };
        println!(
            "| {verb} | {} | {} | {} | interrupt {} |",
            of(Call::Interrupt.word()),
            of(Call::Batch.word()),
            of(Call::Ignore.word()),
            of(Call::Interrupt.word()) + of(Call::Batch.word()) + of(Call::Ignore.word()),
        );
    }
    println!("\n| label \\ call | interrupt | batch | ignore |");
    println!("| --- | --- | --- | --- |");
    let labels: std::collections::BTreeSet<String> =
        confusion.keys().map(|(label, _)| label.clone()).collect();
    for label in labels {
        let of = |call: &str| {
            confusion
                .get(&(label.clone(), call))
                .copied()
                .unwrap_or_default()
        };
        println!(
            "| {label} | {} | {} | {} |",
            of(Call::Interrupt.word()),
            of(Call::Batch.word()),
            of(Call::Ignore.word())
        );
    }
    println!("\n| call | n | mean confidence |");
    println!("| --- | --- | --- |");
    for (call, (n, sum)) in &confident {
        #[allow(clippy::cast_precision_loss)]
        let mean = if *n == 0 { 0.0 } else { sum / *n as f64 };
        println!("| {call} | {n} | {mean:.2} |");
    }
    println!("\nRepeated asks are dependent observations; pooled shares have no Wilson interval.");
    println!(
        "A hand is a key or a paste into the pane (`term_key`/`term_paste`); a tab clicked to read is not one, so 'present and quiet' counts a read-only reaction against interrupting (t-6155 F9)."
    );
    println!(
        "| pass | reacted rows | called interrupt | hit | Wilson lower | marks | agreed | share | Wilson lower |"
    );
    println!("| --- | --- | --- | --- | --- | --- | --- | --- | --- |");
    for (pass, (hit, mark)) in hit_by_pass.iter().zip(&mark_by_pass).enumerate() {
        println!(
            "| #{pass} | {} | {} | {} | {} | {} | {} | {} | {} |",
            hit.compared,
            hit.agreed,
            share_of(hit),
            bound_of(hit),
            mark.compared,
            mark.agreed,
            share_of(mark),
            bound_of(mark)
        );
    }
    println!(
        "| today | — | — | 100% (every ring rings) | — | {} | {} | {} | {} |",
        todays_mark.compared,
        todays_mark.agreed,
        share_of(&todays_mark),
        bound_of(&todays_mark)
    );
    if runs > 1 {
        println!(
            "\nrepeatability: {} of {} later asks named the first pass's call ({})",
            repeated.agreed,
            repeated.compared,
            share_of(&repeated)
        );
    }
    let cost = rate.map(|rate| rate.input_cost_usd(input_tokens));
    println!(
        "\nlatency p50 {} ms · p95 {} ms · max {} ms over {answered} answered; input tokens {input_tokens} ({} per ask); cost {}",
        percentile_of(&elapsed, 0.50),
        percentile_of(&elapsed, 0.95),
        elapsed.last().copied().unwrap_or_default(),
        if answered == 0 {
            0
        } else {
            input_tokens / answered as u64
        },
        cost.map_or_else(
            || "unpriced".to_string(),
            |usd| format!(
                "${usd:.4} total, ${:.6} per ask",
                if answered == 0 {
                    0.0
                } else {
                    usd / answered as f64
                }
            )
        )
    );
}
