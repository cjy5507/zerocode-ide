//! Which room a worker's window belongs in, asked of Jev and written down
//! (t-4781, `zerocode_core::worker_placement`).
//!
//! **Nothing calls this yet, on purpose.** The seat it belongs to offers no
//! mode that applies (`zerocode_core::jev::PLACEMENT`), and the reason is not
//! that the rooms are unreachable — the window has a surface for each of the
//! three — but that no row anywhere says a judgment would place a worker
//! better than the two facts the window is already certain of: which checkout
//! the pane sits in, and which seat it took. Today every ledger worker goes
//! to the same room, its own unfocused tab. This door is what a person would
//! point the window at once those rows exist, and its whole job until then is
//! to make that pointing small: a look in, one question, one row out.
//!
//! The question is asked from the WINDOW's side rather than from the beat,
//! because the window holds most of the look — what is on the stage, how many
//! panes the tab in front already has, and whether the layout's measured rule
//! (`tilePlacement`) would cut one at all. The ledger holds the rest (why the
//! worker was summoned, whether the run dispatches its own work), which is
//! what `orchestration::PreparedWorkerStart::placement_shadow` carries.
//!
//! An answer can only ever name a room the look already said the window could
//! carry out: the option set is built by `worker_placement::ask`, and a word
//! outside it is refused by the reader before this file sees it. `applied` is
//! the row's own word for whether anything was done about it, and it is
//! `false` for every row this seat can write — the mode table decides that,
//! not this file.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use zerocode_core::jev::PLACEMENT;
use zerocode_core::jev::door::{REDACTED_LINES_KEY, REQUESTS_KEY};
use zerocode_core::worker_placement::{
    self, InFront, PlacementLook, StartedBy, WORKER_PLACEMENT_RUBRIC_VERSION,
};

use crate::systemone::{SCHEMA, Wire, request_body};

/// How long one placement question may wait for its answer.
///
/// The wall is set by what an answer would DO rather than by the answers a
/// shorter one would lose, because this is the only question the window asks
/// whose answer is a move on somebody's screen: a pane that rearranges itself
/// long after it appeared is worse than one that never moved. Two seconds is
/// 3.1× the slowest of 24 answers measured on this machine against jev-1.13.0
/// (2026-09-18, `zo decision-shadow check --json`: min 490 ms, p50 559,
/// p90 612, p95 633, max 649), so it costs nothing a measured answer would
/// have arrived inside; past it the row says `timeout`, which names a slow
/// service as plainly as a missing row would not.
const WORKER_ROOM_DEADLINE: Duration =
    Duration::from_millis(zerocode_core::jev::PLACEMENT_APPLY_DEADLINE_MS);

/// The row's outcome for a question Jev answered in shape.
const ANSWERED: &str = "answered";

/// The row's outcome for a look with nothing to judge: no brief reached it,
/// so there was no question to ask.
const NO_BRIEF: &str = "no_brief";

/// The whole look, as a caller would report it: the ledger's half and the
/// window's own.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkerRoomLook {
    /// The head of the summons' brief, already cut to the use's cap by the
    /// ledger. Empty is a summons with no words, which asks nothing.
    brief: String,
    /// Characters of the whole brief, which that cut threw away.
    brief_chars: usize,
    /// `person` or `schedule` — one word folded from two facts: whether
    /// anybody is at this keyboard (the window's) and whether the run fires
    /// its own work (the ledger's). An unknown word reads as `schedule`,
    /// which is the reading that assumes nobody is waiting.
    started_by: String,
    /// `terminal`, `page` or `nothing`. An unknown word reads as `nothing`.
    in_front: String,
    same_workspace: bool,
    panes: usize,
    /// Whether the measured layout rule would cut a pane at all. The caller
    /// decides it, because the cap and the sides are that rule's.
    may_split: bool,
    run: String,
    worker: String,
    dispatch: Option<String>,
    task: Option<String>,
    /// The checkout the pane landed in — the workspace the Jev door asks the
    /// person's consent for. The words travel with the work.
    checkout: Option<String>,
}

/// What came of one placement question.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkerRoomJudged {
    /// The row's outcome word: `answered`, `no_brief`, the mode when the
    /// switch asks nothing, or the door's or the wire's own refusal.
    outcome: String,
    /// The room the judgment named, when one came back whole.
    chosen: Option<String>,
    /// Whether the seat acts right now — a person's `on`, or `auto` raised by
    /// the judge its own ledger recorded — so the surface that asked seats
    /// the worker where the answer says (`ui/shell.js`, `term:worker`) and
    /// otherwise only records what it would have done.
    applied: bool,
    /// The rooms the question offered, in the order it offered them.
    offered: Vec<String>,
}

impl WorkerRoomJudged {
    /// Nothing was asked, or nothing came back whole.
    fn unanswered(outcome: &str, offered: Vec<String>) -> Self {
        Self {
            outcome: outcome.to_string(),
            chosen: None,
            applied: false,
            offered,
        }
    }
}

/// Where this use's rows go.
fn ledger_path(wire: &Wire) -> Option<PathBuf> {
    crate::systemone::ledger_of(wire, &PLACEMENT)
}

/// Ask where one worker's window belongs and write the row.
///
/// Blocking on the wire, so it runs off the window's own runtime. Registered
/// so the door exists; no surface knocks on it (see this module's head).
#[tauri::command]
pub(crate) async fn judge_worker_room(look: WorkerRoomLook) -> Result<WorkerRoomJudged, String> {
    tauri::async_runtime::spawn_blocking(move || {
        judged(&Wire::of_this_machine(), &look, crate::now_epoch_ms())
    })
    .await
    .map_err(|error| error.to_string())
}

/// The whole question, off any runtime: read the switch, ask once, record.
///
/// The wire is an argument rather than read here, for the reason every other
/// use of it gives: only a call that crosses a real socket catches a
/// wire-shaped failure, and a test pointing one at its own listener is how
/// that is caught.
fn judged(wire: &Wire, look: &WorkerRoomLook, now_ms: i64) -> WorkerRoomJudged {
    // A summons with no words says nothing about why it was made, and a room
    // judged on nothing is a row that proves nothing.
    if look.brief.trim().is_empty() {
        return WorkerRoomJudged::unanswered(NO_BRIEF, Vec::new());
    }
    // The switch is read here, at the moment of asking, rather than taken
    // from a caller: what a caller knows is whether asking is worth the
    // round trip, and what decides whether anything is sent is the word in
    // the person's settings file now.
    let mode = PLACEMENT.mode_in(&wire.settings_root());
    // The switch asks nothing, or no zo home resolves to keep the row in.
    // Either way nothing is sent, and the mode word IS the reason.
    let Some(ledger) = ledger_path(wire).filter(|_| mode.asks()) else {
        return WorkerRoomJudged::unanswered(mode.key(), Vec::new());
    };
    let look_of = PlacementLook {
        brief: &look.brief,
        started_by: StartedBy::of(&look.started_by).unwrap_or(StartedBy::Schedule),
        in_front: InFront::of(&look.in_front).unwrap_or(InFront::Nothing),
        same_workspace: look.same_workspace,
        panes: look.panes,
        may_split: look.may_split,
    };
    let ask = worker_placement::ask(&look_of);
    let offered = ask.options();
    let mut row = json!({
        "at": now_ms,
        // One worker, one room, one row: the worker is the name every later
        // reader already has for this pane.
        "placement": look.worker,
        "run": look.run,
        "worker": look.worker,
        "dispatch": look.dispatch,
        "task": look.task,
        "mode": mode.key(),
        "rubricVersion": WORKER_PLACEMENT_RUBRIC_VERSION,
        // The look that was asked about, so a reader never has to trust that
        // the question carried what this row says it did.
        "briefChars": look.brief_chars,
        "startedBy": look_of.started_by.key(),
        "inFront": look_of.in_front.key(),
        "sameWorkspace": look.same_workspace,
        "panes": look.panes,
        // The set the answer is judged against, written down BEFORE it is
        // asked: a row whose options came from the answer would prove nothing.
        "offered": offered,
        // Whether anything was done about the answer. `false` on every row
        // this seat can write, and here rather than inferred from the mode so
        // a reader of the ledger alone can separate the two populations.
        "applied": crate::systemone::applies(wire, &PLACEMENT),
        REQUESTS_KEY: 0,
        REDACTED_LINES_KEY: 0,
    });
    let began = Instant::now();
    let answer = wire.ask(
        &PLACEMENT,
        look.checkout.as_deref().map(std::path::Path::new),
        request_body(&ask.state, &ask.questions),
        WORKER_ROOM_DEADLINE,
    );
    row["elapsedMs"] = json!(u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX));
    row["requestBytes"] = json!(answer.request_bytes);
    row[REQUESTS_KEY] = json!(answer.spent.requests);
    row[REDACTED_LINES_KEY] = json!(answer.spent.redacted_lines);
    let read = answer.answer.and_then(|body| {
        serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|parsed| ask.read(parsed.get("answers")?).ok())
            .ok_or_else(|| SCHEMA.to_string())
    });
    let judged = match read {
        Ok(pick) => {
            row["outcome"] = json!(ANSWERED);
            row["chosen"] = json!(pick.chosen.key());
            row["probabilities"] = json!(pick.probabilities);
            row["confidence"] = json!(pick.confidence);
            WorkerRoomJudged {
                outcome: ANSWERED.to_string(),
                chosen: Some(pick.chosen.key().to_string()),
                applied: crate::systemone::applies(wire, &PLACEMENT),
                offered,
            }
        }
        Err(token) => {
            row["outcome"] = json!(token);
            WorkerRoomJudged::unanswered(&token, offered)
        }
    };
    crate::systemone::record_rows(&PLACEMENT, &ledger, std::slice::from_ref(&row), now_ms);
    judged
}

/// What a placement row promises whoever reads the ledger back. Every case
/// here crosses a real socket or none at all.
#[cfg(test)]
mod tests {
    use serde_json::json;
    use zerocode_core::jev::JevMode;
    use zerocode_core::jev::door::Refused;

    use super::*;
    use crate::systemone::tests::Endpoint;

    /// The look every case here is about: a worker summoned for the work in
    /// front, into the same checkout as the terminal tab on the stage, with room
    /// for one more pane.
    fn look() -> WorkerRoomLook {
        WorkerRoomLook {
            brief: "measure the terminal's frame time again and put the numbers in the commit"
                .to_string(),
            brief_chars: 2_480,
            started_by: "person".to_string(),
            in_front: "terminal".to_string(),
            same_workspace: true,
            panes: 1,
            may_split: true,
            run: "run-4275".to_string(),
            worker: "w-4781".to_string(),
            dispatch: Some("dp-4782".to_string()),
            task: Some("t-4781".to_string()),
            checkout: None,
        }
    }

    /// The endpoint's answer: `chosen`, with the rest of the room going to the
    /// other two.
    fn a_room_answer(chosen: &str) -> String {
        json!({
            "model": "jev-1.13.0",
            "answers": {
                "placement": {
                    "type": "choice",
                    "choice": chosen,
                    "probabilities": { "tab": 0.2, "split": 0.7, "background": 0.1 },
                    "confidence": 0.58,
                }
            },
            "usage": { "input_tokens": 400, "output_tokens": 0 },
        })
        .to_string()
    }

    /// zo's settings in a folder of the case's own: the placement seat at `mode`,
    /// consenting to `consented`. Answers the settings file's path.
    fn settings(home: &tempfile::TempDir, mode: JevMode, consented: &str) -> std::path::PathBuf {
        use zerocode_core::jev::SMART_SETTINGS_KEY;
        let path = home.path().join("settings.json");
        std::fs::write(
            &path,
            json!({
                SMART_SETTINGS_KEY: {
                    PLACEMENT.setting: mode.key(),
                    "jev": { "workspaces": [consented] },
                }
            })
            .to_string(),
        )
        .expect("zo's settings");
        path
    }

    /// Every row the ledger holds, oldest first.
    fn rows(ledger: &std::path::Path) -> Vec<Value> {
        std::fs::read_to_string(ledger)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    /// The request body a heard request carried.
    fn heard_body(request: &str) -> Value {
        let body = request
            .split_once("\r\n\r\n")
            .map(|(_, body)| body)
            .expect("a request body");
        serde_json::from_str(body).expect("a json body")
    }

    /// One room, asked and written down. What leaves the machine is the look and
    /// the three rooms the window could carry out; what comes back is a row that
    /// says which room was named and — the point of this seat today — that
    /// nothing was done about it.
    #[test]
    fn a_room_row_names_the_answer_and_says_nothing_was_done_about_it() {
        let work = tempfile::tempdir().expect("a checkout");
        let home = tempfile::tempdir().expect("a zo home");
        let endpoint = Endpoint::serving("HTTP/1.1 200 OK", a_room_answer("split"), 0);
        let wire = Wire::at(
            &endpoint.base(),
            "test-key",
            Some(settings(
                &home,
                JevMode::Shadow,
                &work.path().display().to_string(),
            )),
        );
        let mut look = look();
        look.checkout = Some(work.path().display().to_string());
        let judged = judged(&wire, &look, 1_789_700_000_000);

        // What was sent: the brief's head and the four facts, and every room the
        // look said the window could carry out.
        let heard = endpoint.asked();
        assert_eq!(heard.len(), 1, "asked {} times", heard.len());
        let sent = heard_body(&heard[0]);
        assert_eq!(
            sent["state"]["brief"],
            json!("measure the terminal's frame time again and put the numbers in the commit")
        );
        assert_eq!(sent["state"]["startedBy"], json!("person"));
        assert_eq!(sent["state"]["inFront"], json!("terminal"));
        assert_eq!(sent["state"]["sameWorkspace"], json!(true));
        assert_eq!(sent["state"]["panes"], json!(1));
        let criteria = &sent["questions"]["placement"]["criteria"];
        assert_eq!(
            criteria.as_object().map(|held| held.len()),
            Some(3),
            "every room the window could use was offered: {criteria}"
        );

        // What came back.
        assert_eq!(judged.outcome, ANSWERED);
        assert_eq!(judged.chosen.as_deref(), Some("split"));
        assert!(
            !judged.applied,
            "a seat with no apply stage moved a pane: {judged:?}"
        );
        assert_eq!(judged.offered, ["tab", "split", "background"]);

        // And the row, which is the whole product of this seat.
        let ledger = home
            .path()
            .join(zerocode_core::jev::count::REQUESTS_DIR)
            .join(PLACEMENT.ledger);
        let held = rows(&ledger);
        assert_eq!(held.len(), 1, "wrote {} rows", held.len());
        let row = &held[0];
        assert_eq!(row["placement"], json!("w-4781"));
        assert_eq!(row["run"], json!("run-4275"));
        assert_eq!(row["dispatch"], json!("dp-4782"));
        assert_eq!(row["task"], json!("t-4781"));
        assert_eq!(row["mode"], json!("shadow"));
        assert_eq!(row["rubricVersion"], json!(1));
        assert_eq!(row["outcome"], json!("answered"));
        assert_eq!(row["chosen"], json!("split"));
        assert_eq!(row["confidence"], json!(0.58));
        assert_eq!(row["offered"], json!(["tab", "split", "background"]));
        assert_eq!(row["briefChars"], json!(2_480));
        assert_eq!(row["applied"], json!(false), "the row claimed a move");
        assert_eq!(row[REQUESTS_KEY], json!(1));
        assert_eq!(row[REDACTED_LINES_KEY], json!(0));
        assert!(row["elapsedMs"].is_u64() && row["requestBytes"].as_u64() > Some(0));
    }

    /// A window with no room to cut offers two rooms, not three — and a judgment
    /// that names the room it was not offered is refused rather than recorded as
    /// an answer. The question a caller may ask and the set its answer is judged
    /// against travel together, so they cannot disagree here.
    #[test]
    fn a_room_nobody_could_carry_out_is_never_offered_and_never_read() {
        let work = tempfile::tempdir().expect("a checkout");
        let home = tempfile::tempdir().expect("a zo home");
        let endpoint = Endpoint::serving("HTTP/1.1 200 OK", a_room_answer("split"), 0);
        let wire = Wire::at(
            &endpoint.base(),
            "test-key",
            Some(settings(
                &home,
                JevMode::Auto,
                &work.path().display().to_string(),
            )),
        );
        let mut look = look();
        look.checkout = Some(work.path().display().to_string());
        // A browser page is on the stage: a shell cannot tile into one.
        look.in_front = "page".to_string();
        look.may_split = false;
        let judged = judged(&wire, &look, 1_789_700_000_000);

        let sent = heard_body(&endpoint.asked()[0]);
        let criteria = &sent["questions"]["placement"]["criteria"];
        assert!(
            criteria.get("split").is_none(),
            "a pane was offered to a stage that cannot hold one: {criteria}"
        );
        assert_eq!(judged.offered, ["tab", "background"]);
        assert_eq!(
            judged.outcome, SCHEMA,
            "an answer outside the offered set was read: {judged:?}"
        );
        assert_eq!(judged.chosen, None);

        let ledger = home
            .path()
            .join(zerocode_core::jev::count::REQUESTS_DIR)
            .join(PLACEMENT.ledger);
        let row = rows(&ledger).pop().expect("a row either way");
        assert_eq!(row["outcome"], json!(SCHEMA));
        assert_eq!(row["offered"], json!(["tab", "background"]));
        assert_eq!(row["chosen"], Value::Null);
    }

    /// Two silences that never reach the wire, each with its own word: the switch
    /// standing at `off`, and a summons that brought no words to judge a room by.
    /// Neither writes a row — a row about a question nobody asked is noise in the
    /// evidence this seat exists to gather.
    #[test]
    fn a_switch_that_asks_nothing_and_a_summons_with_no_words_send_nothing() {
        let work = tempfile::tempdir().expect("a checkout");
        let home = tempfile::tempdir().expect("a zo home");
        let endpoint = Endpoint::serving("HTTP/1.1 200 OK", a_room_answer("tab"), 0);
        let consented = work.path().display().to_string();
        let mut look = look();
        look.checkout = Some(consented.clone());

        let off = Wire::at(
            &endpoint.base(),
            "test-key",
            Some(settings(&home, JevMode::Off, &consented)),
        );
        assert_eq!(judged(&off, &look, 1).outcome, JevMode::Off.key());

        let asking = Wire::at(
            &endpoint.base(),
            "test-key",
            Some(settings(&home, JevMode::Shadow, &consented)),
        );
        let mut wordless = look.clone();
        wordless.brief = "   ".to_string();
        assert_eq!(judged(&asking, &wordless, 1).outcome, NO_BRIEF);

        assert!(
            endpoint.asked().is_empty(),
            "something was sent: {:?}",
            endpoint.asked()
        );
        let ledger = home
            .path()
            .join(zerocode_core::jev::count::REQUESTS_DIR)
            .join(PLACEMENT.ledger);
        assert!(rows(&ledger).is_empty(), "a row was written for nothing");
    }

    /// A workspace the person has not consented to is refused by the door before
    /// anything leaves — and the row still lands, with the door's own word. The
    /// refusal is the evidence: a reader counting this seat's requests must see
    /// the ones that never happened.
    #[test]
    fn an_unconsented_checkout_is_refused_at_the_door_and_still_recorded() {
        let work = tempfile::tempdir().expect("a checkout");
        let home = tempfile::tempdir().expect("a zo home");
        let endpoint = Endpoint::serving("HTTP/1.1 200 OK", a_room_answer("tab"), 0);
        let wire = Wire::at(
            &endpoint.base(),
            "test-key",
            Some(settings(&home, JevMode::Shadow, "/somewhere/else")),
        );
        let mut look = look();
        look.checkout = Some(work.path().display().to_string());
        let judged = judged(&wire, &look, 1_789_700_000_000);

        assert_eq!(judged.outcome, Refused::NotConsented.token());
        assert_eq!(judged.chosen, None);
        assert!(endpoint.asked().is_empty(), "the words left the machine");

        let ledger = home
            .path()
            .join(zerocode_core::jev::count::REQUESTS_DIR)
            .join(PLACEMENT.ledger);
        let row = rows(&ledger).pop().expect("a refusal is a row");
        assert_eq!(row["outcome"], json!(Refused::NotConsented.token()));
        assert_eq!(row[REQUESTS_KEY], json!(0));
        assert_eq!(row["requestBytes"], json!(0));
    }

    /// The rows sit beside the day's count, in the folder the Jev door keeps —
    /// the same folder every other seat the window asks writes into.
    #[test]
    fn the_rows_sit_beside_the_days_count() {
        let home = tempfile::tempdir().expect("a zo home");
        let wire = Wire::at(
            "http://127.0.0.1:1",
            "k",
            Some(home.path().join("settings.json")),
        );
        assert_eq!(
            ledger_path(&wire),
            Some(
                home.path()
                    .join(zerocode_core::jev::count::REQUESTS_DIR)
                    .join("worker-placement.jsonl")
            )
        );
        assert_eq!(PLACEMENT.ledger, "worker-placement.jsonl");
    }

    /// The seat offers a mode that applies (2026-09-20: every seat records
    /// under `auto` and acts once its evidence stands), and the door's answer
    /// says whether it does right now, so the surface that seats the worker
    /// reads one word rather than the table and the ledger both.
    #[test]
    fn the_seat_offers_apply_and_the_answer_says_whether_it_acts() {
        assert!(
            PLACEMENT.modes.iter().any(|mode| mode.applies()),
            "{:?}",
            PLACEMENT.modes
        );
        const { assert!(PLACEMENT.promotes) };
    }
}
