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

/* ---- the replay: the patches this machine's sessions wrote ------------------ */

/// Where the replay seed says which transcripts to read
/// (`tools/patch-review-replay/seed.py`).
const REPLAY_SEED_ENV: &str = "ZEROCODE_PATCH_REVIEW_REPLAY_SEED";

/// About how many reviews one replay asks for, when a person wants fewer
/// than the seed holds — spread evenly over its points.
const REPLAY_LIMIT_ENV: &str = "ZEROCODE_PATCH_REVIEW_REPLAY_LIMIT";

/// What one replay may spend, in dollars — the brief's ceiling: the replay
/// stops asking once the reviews it has paid for reach it.
const REPLAY_SPEND_CAP_USD: f64 = 0.20;

/// How many of the first reviews to print whole — the state that was sent and
/// the four answers — when a person wants to read what the seat was asked.
/// The person's own words and code go to their own terminal and nowhere else.
const REPLAY_SHOW_ENV: &str = "ZEROCODE_PATCH_REVIEW_REPLAY_SHOW";

/// Where to write one JSON line per review — its tool, size, answers, verdict,
/// hindsight and wall; no words and no path — so a person can read the
/// numbers again without asking again.
const REPLAY_OUT_ENV: &str = "ZEROCODE_PATCH_REVIEW_REPLAY_OUT";

/// Reviews in flight at once. The wire's own limit is 1,200 a minute; four
/// at a time keeps a replay of two thousand patches to minutes and far
/// under it.
const REPLAY_IN_FLIGHT: usize = 4;

/// One patch of a transcript, as the replay asks about it and grades it.
struct ReplayPoint {
    ask: PatchAsk,
    hindsight: Option<runtime::patch_review::Hindsight>,
}

/// What became of the patch `ask` names, read off the turns from the one it
/// was written in onward — `None` when the transcript ends before its window
/// does. Only the label reads past the patch; the ask was made from the
/// messages before it.
fn hindsight_in(history: &[ConversationMessage], at: usize, ask: &PatchAsk) -> Option<runtime::patch_review::Hindsight> {
    let turns = runtime::patch_review::persons_turns(history);
    let mut start = 0;
    let mut watched = vec![runtime::patch_review::Watched::of(ask)];
    for turn in turns {
        let end = start + turn.len();
        if end > at {
            let decided = runtime::patch_review::hindsight_of_turn(&mut watched, turn);
            if let Some((_, hindsight)) = decided.into_iter().next() {
                return Some(hindsight);
            }
        }
        start = end;
    }
    None
}

/// Every patch in `history` a review would have been asked about: an edit's
/// result that wrote one, asked from the messages before it.
fn replay_points(history: &[ConversationMessage]) -> Vec<ReplayPoint> {
    let mut points = Vec::new();
    for (at, message) in history.iter().enumerate() {
        for block in &message.blocks {
            let ContentBlock::ToolResult { tool_use_id, tool_name, output, is_error, .. } = block else {
                continue;
            };
            if *is_error {
                continue;
            }
            let Some(asked) = ask_for(&history[..at], "replay", tool_use_id, tool_name, output) else {
                continue;
            };
            let hindsight = hindsight_in(history, at, &asked);
            points.push(ReplayPoint { ask: asked, hindsight });
        }
    }
    points
}

/// How well `score` puts the regretted patches above the ones that stood —
/// the chance a regretted patch outscores one that stood, a tie counting half
/// (the area under the ROC curve, 0.5 being no signal at any line). `None`
/// with no patch of either kind.
#[allow(clippy::cast_precision_loss)]
fn separation(scored: &[(f64, bool)]) -> Option<f64> {
    let regretted: Vec<f64> = scored.iter().filter(|(_, stood)| !stood).map(|(score, _)| *score).collect();
    let stood: Vec<f64> = scored.iter().filter(|(_, stood)| *stood).map(|(score, _)| *score).collect();
    if regretted.is_empty() || stood.is_empty() {
        return None;
    }
    let mut wins = 0.0;
    for high in &regretted {
        for low in &stood {
            wins += match high.partial_cmp(low) {
                Some(std::cmp::Ordering::Greater) => 1.0,
                Some(std::cmp::Ordering::Equal) => 0.5,
                _ => 0.0,
            };
        }
    }
    Some(wins / (regretted.len() * stood.len()) as f64)
}

#[allow(clippy::cast_precision_loss)]
fn percent(part: usize, whole: usize) -> f64 {
    if whole == 0 {
        0.0
    } else {
        part as f64 * 100.0 / whole as f64
    }
}

// One measurement, read top to bottom: the points, the asking, the grading and
// the table are one story, and a split would only move the reader between them.
#[allow(clippy::too_many_lines)]
#[test]
#[ignore = "reads this machine's transcripts through tools/patch-review-replay/seed.py and asks the real endpoint"]
fn the_patches_this_machine_wrote_reviewed_in_hindsight() {
    use futures_util::StreamExt as _;
    use runtime::patch_review::Hindsight;
    use zerocode_core::jev::summary::{percentile, wilson_lower, WILSON_Z_95};

    let seed_path = std::env::var(REPLAY_SEED_ENV).expect("ZEROCODE_PATCH_REVIEW_REPLAY_SEED names the seed");
    let seed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&seed_path).expect("the seed reads")).expect("the seed parses");
    assert_eq!(
        seed["regretTurns"].as_u64(),
        Some(u64::from(PATCH_REVIEW_REGRET_TURNS)),
        "the seed was made for another window"
    );
    let limit: usize = std::env::var(REPLAY_LIMIT_ENV).ok().and_then(|raw| raw.trim().parse().ok()).unwrap_or(usize::MAX);
    let client = api::SystemOneConfig::from_env().expect("TYPESAFE_API_KEY in the environment").into_client();
    // A home of the replay's own: the person's ledgers and day count never move.
    let home = tempfile::tempdir().expect("a config home of the replay's own");
    let door = door(home.path());
    let rate = api::systemone_rate(api::SYSTEMONE_MODEL).expect("the wire's rate is in the price table");

    let mut points: Vec<ReplayPoint> = Vec::new();
    let (mut read, mut skipped) = (0usize, 0usize);
    for transcript in seed["transcripts"].as_array().expect("transcripts") {
        let path = transcript["path"].as_str().expect("a path");
        let Ok(session) = runtime::Session::load_from_path(path) else {
            skipped += 1;
            continue;
        };
        let Some(history) = super::super::replay_support::history_as_it_stood(&session) else {
            skipped += 1;
            continue;
        };
        read += 1;
        points.extend(replay_points(&history));
    }
    // A limit takes points spread evenly over the seed, not its head: the
    // seed is ordered by how many patches a transcript wrote, and its head is
    // a handful of the busiest sessions.
    if limit < points.len() {
        let every = points.len().div_ceil(limit);
        points = points.into_iter().step_by(every).collect();
    }

    let mut rows: Vec<(PatchReviewRow, Option<runtime::patch_review::Answers>, Option<Hindsight>)> = Vec::new();
    let mut spent = 0.0;
    for chunk in points.chunks(REPLAY_IN_FLIGHT) {
        if spent >= REPLAY_SPEND_CAP_USD {
            break;
        }
        let asked: Vec<(PatchReviewRow, Option<runtime::patch_review::Answers>, Option<Hindsight>)> =
            api::sync_bridge::run_blocking(
                futures_util::stream::iter(chunk)
                    .map(|point| async {
                        let (row, answers) = judge(&door, Some(&client), &point.ask).await;
                        (row, answers, point.hindsight)
                    })
                    .buffered(REPLAY_IN_FLIGHT)
                    .collect(),
            );
        spent += asked.iter().map(|(row, _, _)| rate.input_cost_usd(row.input_tokens.unwrap_or(0))).sum::<f64>();
        rows.extend(asked);
    }

    let show: usize = std::env::var(REPLAY_SHOW_ENV).ok().and_then(|raw| raw.trim().parse().ok()).unwrap_or(0);
    for (point, (row, answers, hindsight)) in points.iter().zip(&rows).take(show) {
        println!(
            "=== {} {} hunks={} verdict={} hindsight={:?}\n{}\nanswers={:?}",
            row.tool,
            row.path,
            row.hunks,
            runtime::patch_review::verdict(answers.as_ref()).word(),
            hindsight.map(Hindsight::word),
            serde_json::to_string_pretty(&runtime::patch_review::state(&point.ask)).unwrap_or_default(),
            answers.map(|answers| answers.by_id())
        );
    }
    let mut outcomes: BTreeMap<String, usize> = BTreeMap::new();
    let (mut permits, mut proposals) = (0usize, 0usize);
    let (mut graded, mut agreed, mut permits_that_stood) = (0usize, 0usize, 0usize);
    let (mut stood, mut regretted, mut open, mut receipts) = (0usize, 0usize, 0usize, 0usize);
    let (mut permit_stood, mut permit_regretted) = (0usize, 0usize);
    let mut yes_by_fate: BTreeMap<(&str, bool), (f64, usize)> = BTreeMap::new();
    let mut walls: Vec<u64> = Vec::new();
    let mut input_tokens = 0u64;
    for (row, answers, hindsight) in &rows {
        *outcomes.entry(row.outcome.clone()).or_default() += 1;
        input_tokens += row.input_tokens.unwrap_or(0);
        if row.requests > 0 {
            walls.push(row.elapsed_ms);
        }
        match hindsight {
            Some(Hindsight::Stood { receipt }) => {
                stood += 1;
                receipts += usize::from(*receipt);
            }
            Some(Hindsight::Regretted) => regretted += 1,
            None => open += 1,
        }
        let verdict = runtime::patch_review::verdict(answers.as_ref());
        match verdict {
            Verdict::Permit => permits += 1,
            Verdict::ProposalOnly => proposals += 1,
            Verdict::Unavailable => {}
        }
        let (Some(hindsight), Some(answers)) = (hindsight, answers) else {
            continue;
        };
        let Some(called) = hindsight.agrees_with(verdict) else {
            continue;
        };
        graded += 1;
        agreed += usize::from(called);
        permits_that_stood += usize::from(hindsight.stood());
        if verdict == Verdict::Permit {
            if hindsight.stood() {
                permit_stood += 1;
            } else {
                permit_regretted += 1;
            }
        }
        for (question, yes) in runtime::patch_review::REVIEW_QUESTIONS.iter().zip(answers.yes) {
            let held = yes_by_fate.entry((question.id, hindsight.stood())).or_insert((0.0, 0));
            held.0 += yes;
            held.1 += 1;
        }
    }
    walls.sort_unstable();
    let blanked = rows
        .iter()
        .zip(&points)
        .filter(|(_, point)| point.ask.evidence == runtime::MICROCOMPACT_PLACEHOLDER)
        .count();
    let answered = permits + proposals;
    let decided_stood = stood;
    println!("--- patch review replay: {} reviews over {read} transcripts ({skipped} skipped), {} points in the seed's transcripts", rows.len(), points.len());
    println!("outcomes: {outcomes:?}");
    println!("evidence still a cleared placeholder after healing: {blanked} of {}", rows.len());
    println!(
        "verdicts: permit {permits} ({:.1}% of {answered} answered), proposal_only {proposals}",
        percent(permits, answered)
    );
    println!(
        "hindsight: stood {decided_stood} (receipt {receipts}), regret {regretted}, open {open} (the transcript ended inside the window)"
    );
    let bound = |part: usize, whole: usize| 100.0 * wilson_lower(part, whole, WILSON_Z_95);
    println!(
        "agreement: {agreed} of {graded} graded ({:.1}% pooled; Wilson 95% lower {:.1}%)",
        percent(agreed, graded),
        bound(agreed, graded)
    );
    println!(
        "  the always-permit reader on the same rows: {permits_that_stood} of {graded} ({:.1}%; Wilson lower {:.1}%)",
        percent(permits_that_stood, graded),
        bound(permits_that_stood, graded)
    );
    let stood_graded = yes_by_fate.get(&("addresses_task", true)).map_or(0, |held| held.1);
    let regretted_graded = yes_by_fate.get(&("addresses_task", false)).map_or(0, |held| held.1);
    println!(
        "  permit rate among patches that stood {:.1}% ({permit_stood}/{stood_graded}), among regretted {:.1}% ({permit_regretted}/{regretted_graded})",
        percent(permit_stood, stood_graded),
        percent(permit_regretted, regretted_graded)
    );
    // Whether the answers carry any signal at any line, not only at the
    // permit line: each question's lean against the patch, and the verdict's
    // own statistic (the least a patch leaned toward standing, as a lean
    // against it), each as how well it ranks regretted over stood.
    let graded_rows: Vec<(&runtime::patch_review::Answers, bool)> = rows
        .iter()
        .filter_map(|(_, answers, hindsight)| Some((answers.as_ref()?, hindsight.as_ref()?.stood())))
        .collect();
    for (at, question) in runtime::patch_review::REVIEW_QUESTIONS.iter().enumerate() {
        let scored: Vec<(f64, bool)> = graded_rows
            .iter()
            .map(|(answers, stood)| {
                let yes = answers.yes[at];
                (if question.yes_stands { 1.0 - yes } else { yes }, *stood)
            })
            .collect();
        println!("  regret ranked by {:<20} AUC {:.3}", question.id, separation(&scored).unwrap_or(f64::NAN));
    }
    let leaned: Vec<(f64, bool)> = graded_rows
        .iter()
        .map(|(answers, stood)| {
            let least = runtime::patch_review::REVIEW_QUESTIONS
                .iter()
                .zip(answers.yes)
                .map(|(question, yes)| if question.yes_stands { yes } else { 1.0 - yes })
                .fold(f64::INFINITY, f64::min);
            (1.0 - least, *stood)
        })
        .collect();
    println!("  regret ranked by the verdict's statistic   AUC {:.3}", separation(&leaned).unwrap_or(f64::NAN));
    if let Ok(out) = std::env::var(REPLAY_OUT_ENV) {
        let lines: Vec<String> = rows
            .iter()
            .map(|(row, answers, hindsight)| {
                serde_json::json!({
                    "tool": row.tool,
                    "hunks": row.hunks,
                    "patchBytes": row.patch_bytes,
                    "outcome": row.outcome,
                    "answers": answers.map(|answers| answers.by_id()),
                    "verdict": runtime::patch_review::verdict(answers.as_ref()).word(),
                    "hindsight": hindsight.map(Hindsight::word),
                    "elapsedMs": row.elapsed_ms,
                    "inputTokens": row.input_tokens,
                })
                .to_string()
            })
            .collect();
        std::fs::write(&out, lines.join("\n") + "\n").expect("the replay's rows are written");
        println!("rows written to {out}");
    }
    #[allow(clippy::cast_precision_loss)]
    for question in &runtime::patch_review::REVIEW_QUESTIONS {
        let mean = |fate: bool| {
            yes_by_fate
                .get(&(question.id, fate))
                .map_or(f64::NAN, |(sum, count)| sum / *count as f64)
        };
        println!("  mean p(yes) {:<20} stood {:.3}  regretted {:.3}", question.id, mean(true), mean(false));
    }
    let wall = |share| percentile(&walls, share).unwrap_or(0);
    println!("wall ms: p50 {} p95 {} max {}", wall(0.5), wall(0.95), walls.last().copied().unwrap_or(0));
    #[allow(clippy::cast_precision_loss)]
    let per_review = if rows.is_empty() { 0.0 } else { spent / rows.len() as f64 };
    println!(
        "input tokens {input_tokens}, cost ${spent:.4} (${per_review:.6} per review), cap ${REPLAY_SPEND_CAP_USD:.2}"
    );
}

/// What an edit's result waits on the calling thread for the seat, off and
/// shadow in turn and then the other way round, warm, the wire slower than
/// anything measured here: under shadow the review leaves on its own, so
/// what the result pays is the setting, the door and the book — never the
/// wire.
#[test]
#[ignore = "a timing, printed: what the seat costs the edit's own thread"]
fn what_the_seat_costs_the_edits_own_thread() {
    const ROUNDS: usize = 200;
    let quantiles = |mut held: Vec<Duration>| {
        held.sort_unstable();
        let at = |share: f64| {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
            let index = ((held.len() - 1) as f64 * share).round() as usize;
            held[index].as_secs_f64() * 1_000.0
        };
        (at(0.5), at(0.95))
    };
    for mode in ["off", "shadow", "shadow", "off"] {
        let mock = Mock::slow_first(Duration::from_secs(2), reply(UNRELATED));
        let (p50, p95) = machine(&PATCH_REVIEW, mode, &mock.base_url, |cwd| {
            forget_waiting(cwd);
            let judge = PatchReviewJudge::at(cwd);
            // Warm: the first asks open files and a client nothing else has.
            for at in 0..10 {
                let _ = api::sync_bridge::run_blocking(judge.review(ask(&format!("warm-{at}"))));
            }
            let held: Vec<Duration> = (0..ROUNDS)
                .map(|at| {
                    let asked = ask(&format!("edit-{at}"));
                    let started = Instant::now();
                    let _ = api::sync_bridge::run_blocking(judge.review(asked));
                    started.elapsed()
                })
                .collect();
            forget_waiting(cwd);
            quantiles(held)
        });
        println!("{mode:>6}: the edit's thread held p50 {p50:.3} ms, p95 {p95:.3} ms over {ROUNDS} edits");
    }
}
