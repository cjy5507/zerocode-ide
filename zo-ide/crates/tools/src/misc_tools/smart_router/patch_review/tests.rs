//! The seat, held to its contract on a fake wire: off is today to the byte,
//! the door is the only road, a recording seat never holds the result, an
//! acting one hands back its line, a row carries its receipt, and hindsight
//! writes one label per answered review.

use std::path::Path;
use std::time::{Duration, Instant};

use api::SystemOneClient;
use runtime::patch_review::{ask_for, Verdict};
use runtime::{ContentBlock, ConversationMessage, MessageRole, PatchAsk};
use zerocode_core::jev::door::{JevSettings, Refused};
use zerocode_core::jev::{digest_of, fingerprint_of, JevMode, PATCH_REVIEW, PATCH_REVIEW_REGRET_TURNS};

use super::super::jev_gate::JevDoor;
use super::super::jev_mock::{machine, Mock};
use super::super::shadow_ledger::read_shadow_rows;
use super::*;

const WORKSPACE: &str = "/work/zo";

fn door(home: &Path) -> JevDoor {
    let settings = JevSettings {
        enabled: true,
        workspaces: vec![WORKSPACE.to_string()],
        daily_requests: None,
        model: zerocode_core::jev::DEFAULT_MODEL.to_string(),
    };
    JevDoor::at(settings, Path::new(WORKSPACE), home)
}

fn user_text(text: &str) -> ConversationMessage {
    ConversationMessage {
        role: MessageRole::User,
        blocks: vec![ContentBlock::Text { text: text.to_string() }],
        usage: None,
        thought_signature: None,
        reasoning_replay: None,
        model: None,
    }
}

fn call(id: &str, tool: &str, input: &str) -> ConversationMessage {
    ConversationMessage::assistant(vec![ContentBlock::ToolUse {
        id: id.to_string(),
        name: tool.to_string(),
        input: input.to_string(),
    }])
}

fn result(id: &str, tool: &str, output: &str) -> ConversationMessage {
    ConversationMessage::tool_result(id, tool, output, false)
}

/// An edit's result in the shape `edit_file` writes it: one hunk at `line`
/// of `path` replacing one line.
fn edit_output(path: &str, line: usize, old: &str, new: &str) -> String {
    serde_json::to_string_pretty(&serde_json::json!({
        "filePath": path,
        "oldString": old,
        "newString": new,
        "structuredPatch": [{
            "oldStart": line, "oldLines": 1, "newStart": line, "newLines": 1,
            "lines": [format!("-{old}"), format!("+{new}")],
        }],
        "userModified": false,
        "replaceAll": false,
        "gitDiff": null,
    }))
    .expect("an envelope")
}

/// The patch every test asks about: line 12 of `/work/zo/src/flag.rs`.
fn patch_output() -> String {
    edit_output("/work/zo/src/flag.rs", 12, "let flag = old;", "let flag = new;")
}

fn ask(tool_use_id: &str) -> PatchAsk {
    let messages = vec![
        user_text("rename the flag to new"),
        call("t1", "bash", r#"{"command":"cargo test"}"#),
        result("t1", "bash", r#"{"stdout":"test flag ... FAILED\nexpected new","stderr":""}"#),
    ];
    ask_for(&messages, "turn-1", tool_use_id, "edit_file", &patch_output()).expect("an edit is asked about")
}

fn noul(yes: f64) -> serde_json::Value {
    serde_json::json!({"type": "noul", "noul": yes})
}

/// A reply of four Nouls, in the question order.
fn reply(yes: [f64; 4]) -> String {
    serde_json::json!({
        "model": "jev-test",
        "answers": {
            "addresses_task": noul(yes[0]),
            "evidence_supports": noul(yes[1]),
            "unrelated_changes": noul(yes[2]),
            "needs_clarification": noul(yes[3]),
        },
        "usage": {"input_tokens": 612, "output_tokens": 4}
    })
    .to_string()
}

const PERMITS: [f64; 4] = [0.95, 0.9, 0.05, 0.05];
const UNRELATED: [f64; 4] = [0.95, 0.9, 0.82, 0.05];

fn judged(base_url: &str, ask: &PatchAsk) -> (PatchReviewRow, Option<Answers>) {
    let home = tempfile::tempdir().expect("a config home");
    let client = SystemOneClient::new(base_url, "test-key");
    api::sync_bridge::run_blocking(judge(&door(home.path()), Some(&client), ask))
}

/// The rows of `cwd`'s ledger, waited for until `count` are there — a
/// recording review writes its row after the result has gone back.
fn rows_of(cwd: &Path, count: usize) -> Vec<serde_json::Value> {
    let ledger = patch_review_path(cwd);
    let started = Instant::now();
    loop {
        let rows: Vec<serde_json::Value> = read_shadow_rows(&ledger);
        if rows.len() >= count || started.elapsed() > Duration::from_secs(10) {
            return rows;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn the_setting_reads_the_tables_words_and_a_slip_is_off() {
    let home = tempfile::tempdir().expect("home");
    let cwd = tempfile::tempdir().expect("cwd");
    for (word, expected) in [
        ("shadow", Some(JevMode::Shadow)),
        ("on", Some(JevMode::On)),
        ("auto", Some(JevMode::Auto)),
        ("shadwo", Some(JevMode::Off)),
    ] {
        std::fs::write(
            home.path().join("settings.json"),
            serde_json::json!({ "smart": { JEV_PATCH_REVIEW_SETTING_WORD: word } }).to_string(),
        )
        .expect("settings");
        let loader = runtime::ConfigLoader::new(cwd.path(), home.path());
        assert_eq!(jev_patch_review_mode_from(&loader), expected, "jevPatchReview = {word}");
    }
    std::fs::write(home.path().join("settings.json"), "{}").expect("settings");
    let loader = runtime::ConfigLoader::new(cwd.path(), home.path());
    assert_eq!(jev_patch_review_mode_from(&loader), Some(JevMode::Off), "no word is off");
}

/// The key the setting lives under, read from the table rather than spelled.
const JEV_PATCH_REVIEW_SETTING_WORD: &str = PATCH_REVIEW.setting;

/// `off` is today: nothing asked, nothing written, no line, nothing waiting
/// on a label — whatever the wire would have said.
#[test]
fn off_asks_nothing_writes_nothing_and_adds_no_line() {
    let mock = Mock::serving(200, reply(UNRELATED));
    machine(&PATCH_REVIEW, "off", &mock.base_url, |cwd| {
        let judge = PatchReviewJudge::at(cwd);
        let review = api::sync_bridge::run_blocking(judge.review(ask("edit-off")));
        assert_eq!(review, PatchReview::default());
        assert!(mock.requests().is_empty(), "nothing left the machine");
        assert!(!patch_review_path(cwd).exists(), "no ledger");
        assert_eq!(note_patch_review_turn(cwd, Some(&[])), 0);
        assert!(book().lock().expect("book").get(cwd).is_none(), "nothing waits on a label");
    });
}

#[test]
fn a_keyless_review_is_refused_at_the_door_and_is_unavailable() {
    let home = tempfile::tempdir().expect("a config home");
    let (row, answers) = api::sync_bridge::run_blocking(judge(&door(home.path()), None, &ask("edit-1")));
    assert_eq!(row.outcome, Refused::NoKey.token());
    assert!(answers.is_none());
    assert_eq!((row.requests, row.request_digest.as_ref()), (0, None), "nothing was sent, nothing to vouch for");
    let (row, verdict, line) = settle(JevMode::On, true, row, None);
    assert_eq!(verdict, Verdict::Unavailable);
    assert_eq!((row.verdict.as_str(), row.route_use.as_str(), row.applied, row.noted), ("unavailable", "fallback", false, false));
    assert_eq!(line, None);
}

/// One answered review: four Nouls over one state that carries the hunks,
/// the evidence's tail and the path's fingerprint — never the path — and a
/// row whose receipt is the digest of exactly the bytes the wire was handed.
#[test]
fn an_answered_review_asks_four_nouls_and_its_row_carries_the_receipt() {
    let mock = Mock::serving(200, reply(UNRELATED));
    let asked = ask("edit-1");
    let (row, answers) = judged(&mock.base_url, &asked);
    assert_eq!(row.outcome, PATCH_REVIEW_OUTCOME_ANSWERED, "{row:?}");
    let read = answers.expect("answers").yes;
    assert!(read.iter().zip(UNRELATED).all(|(got, want)| (got - want).abs() < f64::EPSILON), "{read:?}");
    assert_eq!(row.model.as_deref(), Some("jev-test"), "the answering version");
    assert_eq!(row.input_tokens, Some(612));
    assert_eq!(row.requests, 1);
    assert_eq!((row.tool.as_str(), row.hunks), ("edit_file", 1));
    assert_eq!(row.path, fingerprint_of("/work/zo/src/flag.rs"));
    assert_eq!(row.answers.as_ref().map(BTreeMap::len), Some(4));
    assert_eq!(row.attempt.as_deref(), Some("turn-1"));

    let sent = mock.requests();
    assert_eq!(sent.len(), 1);
    let body: serde_json::Value = serde_json::from_str(&sent[0]).expect("a request body");
    assert_eq!(body["state"]["task"], "rename the flag to new");
    assert_eq!(body["state"]["patch"], "@@ -12,1 +12,1 @@\n-let flag = old;\n+let flag = new;");
    assert!(body["state"]["evidence"].as_str().expect("evidence").contains("test flag ... FAILED\nexpected new"));
    assert_eq!(body["state"]["path"], fingerprint_of("/work/zo/src/flag.rs"));
    assert!(!sent[0].contains("/work/zo/src/flag.rs"), "the path never leaves");
    for id in ["addresses_task", "evidence_supports", "unrelated_changes", "needs_clarification"] {
        assert_eq!(body["questions"][id]["type"], "noul", "{id}");
    }
    assert_eq!(
        row.request_digest.as_deref(),
        Some(digest_of(PATCH_REVIEW.id, PATCH_REVIEW_RUBRIC_VERSION, zerocode_core::jev::DEFAULT_MODEL, sent[0].as_bytes()).as_str()),
        "the receipt is the digest of the bytes that left"
    );
}

#[test]
fn a_reply_that_breaks_the_contract_is_a_schema_row() {
    let broken = serde_json::json!({
        "model": "jev-test",
        "answers": {"addresses_task": noul(0.9), "evidence_supports": noul(0.9), "unrelated_changes": noul(0.1)},
        "usage": {"input_tokens": 600, "output_tokens": 3}
    });
    let mock = Mock::serving(200, broken.to_string());
    let (row, answers) = judged(&mock.base_url, &ask("edit-1"));
    assert_eq!(row.outcome, api::SystemOneFailure::Schema.ledger_token());
    assert_eq!(row.rejected.as_deref(), Some(zerocode_core::jev::noul::NoulRefusal::NoAnswer.token()));
    assert!(answers.is_none());
}

/// A wire that never answers misses the wall, and the review is unavailable:
/// the result reads as it did without the seat, at the cost of the wall.
#[test]
fn a_silent_wire_misses_the_wall() {
    let mock = Mock::silent();
    let started = Instant::now();
    let (row, answers) = judged(&mock.base_url, &ask("edit-1"));
    assert_eq!(row.outcome, api::SystemOneFailure::Timeout.ledger_token(), "{row:?}");
    assert!(answers.is_none());
    assert!(started.elapsed() < PATCH_REVIEW_DEADLINE * 2, "the wall is the row's");
}

/// The row's reader and the line, mode by mode.
#[test]
fn an_acting_seat_notes_a_proposal_and_a_recording_seat_notes_nothing() {
    let answers = |yes| Some(runtime::patch_review::Answers { yes });
    let row = || PatchReviewRow::new(&ask("edit-1"));

    let (acted, verdict, line) = settle(JevMode::On, true, row(), answers(UNRELATED).as_ref());
    assert_eq!(verdict, Verdict::ProposalOnly);
    assert_eq!((acted.route_use.as_str(), acted.applied, acted.noted), (ROUTE_USE_APPLIED, true, true));
    assert_eq!(
        line.as_deref(),
        Some("[zo:patch-review] unrelated changes 0.82 — narrow the edit to the task or confirm the extra change is wanted.")
    );

    let (permitted, verdict, line) = settle(JevMode::On, true, row(), answers(PERMITS).as_ref());
    assert_eq!((verdict, permitted.verdict.as_str(), permitted.noted, line), (Verdict::Permit, "permit", false, None));

    let (recorded, verdict, line) = settle(JevMode::Shadow, false, row(), answers(UNRELATED).as_ref());
    assert_eq!(verdict, Verdict::ProposalOnly);
    assert_eq!((recorded.route_use.as_str(), recorded.applied, recorded.noted, line), ("shadow", false, false, None));

    let (waiting, _, line) = settle(JevMode::Auto, false, row(), answers(UNRELATED).as_ref());
    assert_eq!((waiting.route_use.as_str(), line), ("auto", None), "an auto nothing raised records");
}

/// The whole road under `on`: the result waits inside the wall, the line
/// comes back, the row is written, and the patch waits on its label.
#[test]
fn an_acting_seat_hands_back_its_line_and_writes_its_row() {
    let mock = Mock::serving(200, reply(UNRELATED));
    machine(&PATCH_REVIEW, "on", &mock.base_url, |cwd| {
        forget_waiting(cwd);
        let review = api::sync_bridge::run_blocking(PatchReviewJudge::at(cwd).review(ask("edit-on")));
        assert!(review.note.as_deref().is_some_and(|line| line.starts_with("[zo:patch-review] unrelated changes 0.82")));
        let rows = rows_of(cwd, 1);
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0]["verdict"], "proposal_only");
        assert_eq!(rows[0]["routeUse"], ROUTE_USE_APPLIED);
        assert_eq!(rows[0]["noted"], true);
        assert!(rows[0]["requestDigest"].as_str().is_some_and(|digest| digest.len() == 64));
        let waiting = book().lock().expect("book").get(cwd).map(Vec::len);
        assert_eq!(waiting, Some(1), "the patch waits on its label");
        forget_waiting(cwd);
    });
}

/// Under `shadow` the seat hands the result back at once — before the wire
/// has answered — and writes its row when the answer comes.
#[test]
fn a_recording_seat_holds_nothing_and_writes_its_row_after() {
    let mock = Mock::slow_first(Duration::from_millis(400), reply(UNRELATED));
    machine(&PATCH_REVIEW, "shadow", &mock.base_url, |cwd| {
        forget_waiting(cwd);
        let started = Instant::now();
        let review = api::sync_bridge::run_blocking(PatchReviewJudge::at(cwd).review(ask("edit-shadow")));
        let held = started.elapsed();
        assert_eq!(review, PatchReview::default(), "no line under shadow");
        assert!(held < Duration::from_millis(400), "the result was not held for the wire: {held:?}");
        let rows = rows_of(cwd, 1);
        assert_eq!(rows.len(), 1, "the row is written once the answer comes: {rows:?}");
        assert_eq!(rows[0]["routeUse"], "shadow");
        assert_eq!(rows[0]["verdict"], "proposal_only");
        assert_eq!(rows[0]["noted"], false);
        forget_waiting(cwd);
    });
}

/* ---- the label ------------------------------------------------------------ */

/// A patch in the book, as the seam leaves it, with its verdict in.
fn waiting_with(cwd: &Path, ledger: &Path, tool_use_id: &str, verdict: Verdict) -> String {
    let asked = ask(tool_use_id);
    let label = judged_key(&asked).to_string();
    watch(cwd, &asked, &label);
    settle_verdict(cwd, ledger, &label, verdict, false);
    label
}

fn green_check() -> [ConversationMessage; 2] {
    [
        call("c1", "bash", r#"{"command":"cargo test -p flag"}"#),
        result("c1", "bash", r#"{"stdout":"test result: ok","stderr":""}"#),
    ]
}

/// A permit on a patch whose line the next turn edited again disagreed; a
/// proposal on the same would have agreed.
#[test]
fn a_permit_on_a_patch_fixed_again_disagrees_and_a_proposal_agrees() {
    let cwd = tempfile::tempdir().expect("cwd");
    let ledger = cwd.path().join(PATCH_REVIEW_FILE);
    forget_waiting(cwd.path());
    waiting_with(cwd.path(), &ledger, "edit-a", Verdict::Permit);
    waiting_with(cwd.path(), &ledger, "edit-b", Verdict::ProposalOnly);
    let own = vec![
        user_text("rename the flag to new"),
        call("edit-a", "edit_file", "{}"),
        result("edit-a", "edit_file", &patch_output()),
    ];
    assert_eq!(label_turn(cwd.path(), &ledger, Some(&own)), 0, "no hindsight yet");
    let again = vec![
        user_text("that broke it"),
        call("e2", "edit_file", "{}"),
        result("e2", "edit_file", &edit_output("/work/zo/src/flag.rs", 12, "let flag = new;", "let flag = newer;")),
    ];
    assert_eq!(label_turn(cwd.path(), &ledger, Some(&again)), 2);
    let labels: Vec<PatchReviewLabelRow> = read_shadow_rows(&ledger);
    let label = |verdict: &str| labels.iter().find(|row| row.verdict == verdict).expect("a label").clone();
    let permit = label("permit");
    assert_eq!((permit.agreed, permit.hindsight.as_str(), permit.turns_later), (false, "regret", 1));
    assert_eq!(permit.kind, runtime::LABEL_ROW_KIND);
    assert_eq!(permit.path, fingerprint_of("/work/zo/src/flag.rs"));
    let proposal = label("proposal_only");
    assert!(proposal.agreed, "a proposal on a patch that was fixed again called it");
    assert!(book().lock().expect("book").get(cwd.path()).is_none(), "nothing left waiting");
}

/// The receipt settles a patch first: a check green after the turn's last
/// edit, and the permit agreed.
#[test]
fn a_receipt_settles_a_permit_as_agreed() {
    let cwd = tempfile::tempdir().expect("cwd");
    let ledger = cwd.path().join(PATCH_REVIEW_FILE);
    forget_waiting(cwd.path());
    waiting_with(cwd.path(), &ledger, "edit-a", Verdict::Permit);
    let mut turn = vec![
        user_text("rename the flag to new"),
        call("edit-a", "edit_file", "{}"),
        result("edit-a", "edit_file", &patch_output()),
    ];
    turn.extend(green_check());
    assert_eq!(label_turn(cwd.path(), &ledger, Some(&turn)), 1);
    let labels: Vec<PatchReviewLabelRow> = read_shadow_rows(&ledger);
    assert_eq!((labels[0].agreed, labels[0].hindsight.as_str()), (true, "receipt"));
}

/// A recording review still on the wire at the turn's end: its hindsight is
/// kept, and the label is written the moment the verdict comes in.
#[test]
fn a_label_waits_for_a_review_still_on_the_wire() {
    let cwd = tempfile::tempdir().expect("cwd");
    let ledger = cwd.path().join(PATCH_REVIEW_FILE);
    forget_waiting(cwd.path());
    let asked = ask("edit-a");
    let label = judged_key(&asked).to_string();
    watch(cwd.path(), &asked, &label);
    let mut turn = vec![
        user_text("rename the flag to new"),
        call("edit-a", "edit_file", "{}"),
        result("edit-a", "edit_file", &patch_output()),
    ];
    turn.extend(green_check());
    assert_eq!(label_turn(cwd.path(), &ledger, Some(&turn)), 0, "no verdict yet");
    settle_verdict(cwd.path(), &ledger, &label, Verdict::ProposalOnly, false);
    let labels: Vec<PatchReviewLabelRow> = read_shadow_rows(&ledger);
    assert_eq!(labels.len(), 1);
    assert_eq!((labels[0].verdict.as_str(), labels[0].agreed, labels[0].hindsight.as_str()), ("proposal_only", false, "receipt"));
    assert!(book().lock().expect("book").get(cwd.path()).is_none_or(Vec::is_empty));
}

/// A review that never answered is taken out of the book: there is nothing
/// to grade.
#[test]
fn an_unavailable_review_is_never_labeled() {
    let cwd = tempfile::tempdir().expect("cwd");
    let ledger = cwd.path().join(PATCH_REVIEW_FILE);
    forget_waiting(cwd.path());
    waiting_with(cwd.path(), &ledger, "edit-a", Verdict::Unavailable);
    let quiet = vec![user_text("next")];
    for _ in 0..PATCH_REVIEW_REGRET_TURNS {
        assert_eq!(label_turn(cwd.path(), &ledger, Some(&quiet)), 0);
    }
    assert!(!ledger.exists());
}

/// A cancelled turn is no turn of the window, but what it wrote is behind the
/// next one: an edit of the same line in the next turn is the patch's regret.
#[test]
fn a_cancelled_turn_counts_no_turn_but_leaves_the_patch_behind_it() {
    let cwd = tempfile::tempdir().expect("cwd");
    let ledger = cwd.path().join(PATCH_REVIEW_FILE);
    forget_waiting(cwd.path());
    waiting_with(cwd.path(), &ledger, "edit-a", Verdict::Permit);
    assert_eq!(label_turn(cwd.path(), &ledger, None), 0);
    let again = vec![
        user_text("that broke it"),
        result("e2", "edit_file", &edit_output("/work/zo/src/flag.rs", 12, "let flag = new;", "let flag = newer;")),
    ];
    assert_eq!(label_turn(cwd.path(), &ledger, Some(&again)), 1);
    let labels: Vec<PatchReviewLabelRow> = read_shadow_rows(&ledger);
    assert_eq!((labels[0].hindsight.as_str(), labels[0].turns_later), ("regret", 0), "regretted in the first turn it waited");
}
