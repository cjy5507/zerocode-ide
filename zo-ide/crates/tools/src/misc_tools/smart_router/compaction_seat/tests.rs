//! The seat on a scripted wire: what leaves, what comes back, what the row
//! says, what the label writes. No machine settings, no network — a mock on
//! a loopback port, a door told its facts, a ledger in a directory of its own.

use std::path::Path;
use std::sync::Arc;

use api::SystemOneClient;
use runtime::compaction_relevance::{ask_for, candidates, drop_blocks, summary_input_tokens};
use runtime::{prepare_compaction, CompactionConfig, Session, MICROCOMPACT_MIN_OUTPUT_BYTES};
use zerocode_core::jev::door::JevSettings;
use zerocode_core::jev::{
    JevMode, COMPACTION_DROP, COMPACTION_KEEP, COMPACTION_REGRET_TURNS, ROUTE_USE_APPLIED,
    ROUTE_USE_FALLBACK,
};

use super::super::settings::JEV_COMPACTION_SETTING;
use super::super::jev_mock::Mock;
use super::*;

/// The workspace every ask here comes from, consented at a door that reads
/// no machine's settings.
const WORKSPACE: &str = "/work/zo";

fn door(home: &Path) -> JevDoor {
    let settings = JevSettings {
        enabled: true,
        workspaces: vec![WORKSPACE.to_string()],
        daily_requests: None,
    };
    JevDoor::at(settings, Path::new(WORKSPACE), home)
}

fn user_text(text: &str) -> ConversationMessage {
    ConversationMessage {
        role: MessageRole::User,
        blocks: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
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

fn result(id: &str, tool: &str, output: &str, is_error: bool) -> ConversationMessage {
    ConversationMessage {
        role: MessageRole::User,
        blocks: vec![ContentBlock::ToolResult {
            tool_use_id: id.to_string(),
            tool_name: tool.to_string(),
            output: output.to_string(),
            is_error,
            images: Vec::new(),
        }],
        usage: None,
        thought_signature: None,
        reasoning_replay: None,
        model: None,
    }
}

fn big(tag: &str) -> String {
    format!("{tag}\n{}", "line of output\n".repeat(60))
}

/// A session whose compaction set holds three droppable results — a read,
/// a shell command and a grep — behind a goal, with a fresh request and a
/// reply in the tail.
fn session() -> Session {
    let mut session = Session::new();
    for message in [
        user_text("make the failing test in src/lib.rs pass"),
        call("t1", "Read", r#"{"path":"/work/src/lib.rs"}"#),
        result("t1", "Read", &big("fn add(a: i32, b: i32) -> i32 { a + b }"), false),
        call("t2", "Bash", r#"{"command":"cargo test"}"#),
        result("t2", "Bash", &big("test result: FAILED. 1 failed"), false),
        call("t3", "Grep", r#"{"pattern":"fn add"}"#),
        result("t3", "Grep", &big("src/lib.rs:1:fn add"), false),
        ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "fixed the test".to_string(),
        }]),
        user_text("now the warning"),
        ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "looking".to_string(),
        }]),
    ] {
        session.push_message(message).expect("push");
    }
    session
}

fn ask_of(session: &Session) -> (CompactionAsk, runtime::CompactionPlan) {
    let plan = prepare_compaction(
        session,
        CompactionConfig {
            preserve_recent_messages: 2,
            max_estimated_tokens: 0,
        },
    )
    .expect("a plan");
    let ask = ask_for(session, &plan, MICROCOMPACT_MIN_OUTPUT_BYTES, "s@1").expect("an ask");
    (ask, plan)
}

fn answer(chosen: &str, drop: f64) -> serde_json::Value {
    serde_json::json!({
        "type": "choice",
        "choice": chosen,
        "probabilities": { COMPACTION_KEEP: 1.0 - drop, COMPACTION_DROP: drop },
        "confidence": 0.9,
    })
}

fn reply(answers: &serde_json::Map<String, serde_json::Value>) -> String {
    serde_json::json!({
        "model": "jev-test",
        "answers": answers,
        "usage": {"input_tokens": 321, "output_tokens": 12}
    })
    .to_string()
}

fn judged(base_url: &str, ask: &CompactionAsk, acting: bool) -> (CompactionRow, Vec<usize>) {
    let home = tempfile::tempdir().expect("a config home");
    let client = SystemOneClient::new(base_url, "test-key");
    let door = door(home.path());
    api::sync_bridge::run_blocking(judge(&door, Some(&client), ask, acting))
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
            serde_json::json!({ "smart": { JEV_COMPACTION_SETTING: word } }).to_string(),
        )
        .expect("settings");
        let loader = runtime::ConfigLoader::new(cwd.path(), home.path());
        assert_eq!(jev_compaction_mode_from(&loader), expected, "jevCompaction = {word}");
    }
    std::fs::write(home.path().join("settings.json"), "{}").expect("settings");
    let loader = runtime::ConfigLoader::new(cwd.path(), home.path());
    assert_eq!(jev_compaction_mode_from(&loader), Some(JevMode::Off), "no word is off");
    assert_eq!(JEV_COMPACTION_SETTING, "jevCompaction");
}

#[test]
fn a_keyless_ask_is_refused_at_the_door_and_keeps_every_block() {
    let session = session();
    let (ask, _) = ask_of(&session);
    let home = tempfile::tempdir().expect("a config home");
    let door = door(home.path());
    let (row, dropped) = api::sync_bridge::run_blocking(judge(&door, None, &ask, true));
    assert_eq!(row.outcome, Refused::NoKey.token());
    assert!(dropped.is_empty());
    assert_eq!((row.candidates, row.shards, row.shards_answered, row.requests), (3, 1, 0, 0));
    assert_eq!(row.rubric_version, COMPACTION_RUBRIC_VERSION);
    assert_eq!(row.attempt.as_deref(), Some("s@1"));
}

/// The one answered shard: heads leave, bodies do not, and only a `drop`
/// past the lean drops.
#[test]
fn an_answered_shard_drops_only_past_the_lean_and_the_row_counts_it() {
    let session = session();
    let (ask, plan) = ask_of(&session);
    let mut answers = serde_json::Map::new();
    answers.insert("b0".to_string(), answer(COMPACTION_DROP, 0.91));
    answers.insert("b1".to_string(), answer(COMPACTION_KEEP, 0.2));
    answers.insert("b2".to_string(), answer(COMPACTION_DROP, 0.55));
    let mock = Mock::serving(200, reply(&answers));
    let (row, dropped) = judged(&mock.base_url, &ask, false);

    assert_eq!(row.outcome, COMPACTION_OUTCOME_ANSWERED, "{row:?}");
    assert_eq!(dropped, vec![0], "b0 past the lean, b2 under it, b1 kept");
    assert_eq!((row.candidates, row.shards, row.shards_answered, row.dropped), (3, 1, 1, 1));
    assert_eq!(row.dropped_bytes, ask.blocks[0].output_bytes);
    assert_eq!(row.model.as_deref(), Some("jev-test"));
    assert_eq!(row.input_tokens, Some(321));
    assert_eq!(row.requests, 1);
    assert!(!row.cached);

    let sent = mock.requests();
    assert_eq!(sent.len(), 1);
    let body: serde_json::Value = serde_json::from_str(&sent[0]).expect("a request body");
    assert_eq!(body["state"]["goal"], "now the warning");
    assert_eq!(body["state"]["recent"], "looking");
    assert_eq!(body["state"]["blocks"].as_array().map(Vec::len), Some(3));
    assert_eq!(body["state"]["blocks"][1]["tool"], "Bash");
    assert_eq!(body["state"]["blocks"][1]["input"], r#"{"command":"cargo test"}"#);
    assert!(!sent[0].contains(&"line of output\n".repeat(30)), "a body never leaves");
    assert_eq!(body["questions"]["b0"]["type"], "choice");
    assert!(body["questions"]["b0"]["criteria"].get(COMPACTION_KEEP).is_some());
    assert!(body["questions"]["b0"]["criteria"].get(COMPACTION_DROP).is_some());
    assert_eq!(body["model"], api::SYSTEMONE_MODEL);

    // What the runtime would do with it, on a session that seals nowhere:
    // the dropped block's body leaves the summary's input and nothing else.
    let before = summary_input_tokens(&plan.messages_to_compact);
    let mut applied = plan.clone();
    assert_eq!(drop_blocks(&mut applied, &session, &ask.blocks, &dropped), 1);
    let after = summary_input_tokens(&applied.messages_to_compact);
    assert!(after < before, "{after} < {before}");
    assert_eq!(candidates(&applied, MICROCOMPACT_MIN_OUTPUT_BYTES).len(), 2, "one fewer to ask about");
}

#[test]
fn a_reply_that_breaks_the_contract_is_a_schema_row_that_keeps_every_block() {
    let session = session();
    let (ask, _) = ask_of(&session);
    let mut answers = serde_json::Map::new();
    answers.insert("b0".to_string(), answer(COMPACTION_DROP, 0.9));
    answers.insert("b9".to_string(), answer(COMPACTION_DROP, 0.9));
    let mock = Mock::serving(200, reply(&answers));
    let (row, dropped) = judged(&mock.base_url, &ask, true);
    assert_eq!(row.outcome, api::SystemOneFailure::Schema.ledger_token());
    assert_eq!(row.rejected.as_deref(), Some("unknown_answer"));
    assert_eq!(row.rejected_at, None);
    assert!(dropped.is_empty());
    assert_eq!(row.shards_answered, 0);
}

/// A wire that never answers misses the wall, and every block it was asked
/// about is kept — today's compaction, at the cost of the wall.
#[test]
fn a_silent_wire_misses_the_wall_and_keeps_every_block() {
    let session = session();
    let (ask, _) = ask_of(&session);
    let mock = Mock::silent();
    let started = std::time::Instant::now();
    let (row, dropped) = judged(&mock.base_url, &ask, true);
    assert_eq!(row.outcome, api::SystemOneFailure::Timeout.ledger_token(), "{row:?}");
    assert!(dropped.is_empty());
    assert!(
        started.elapsed() < COMPACTION_JUDGMENT_DEADLINE * 2,
        "the wall is the row's, not the client's own patience"
    );
    let wall = u64::try_from(COMPACTION_JUDGMENT_DEADLINE.as_millis()).unwrap_or(u64::MAX);
    assert!(row.elapsed_ms >= wall * 9 / 10);
}

/// The row's `routeUse` and the runtime's `applies`, mode by mode: a
/// recording mode hands its drops back for the label and applies none.
#[test]
fn a_recording_mode_records_its_drops_and_applies_nothing_an_acting_one_applies() {
    let session = session();
    let (ask, _) = ask_of(&session);
    let row = |outcome: &str| CompactionRow::new(&ask, 1, outcome.to_string());

    let (shadow, judgment) = settle(JevMode::Shadow, false, row(COMPACTION_OUTCOME_ANSWERED), vec![0, 2]);
    assert_eq!((shadow.route_use.as_str(), shadow.applied), ("shadow", false));
    assert_eq!(judgment, CompactionJudgment { dropped: vec![0, 2], applies: false });

    let (on, judgment) = settle(JevMode::On, true, row(COMPACTION_OUTCOME_ANSWERED), vec![0]);
    assert_eq!((on.route_use.as_str(), on.applied), (ROUTE_USE_APPLIED, true));
    assert_eq!(judgment, CompactionJudgment { dropped: vec![0], applies: true });

    let (kept_all, judgment) = settle(JevMode::On, true, row(COMPACTION_OUTCOME_ANSWERED), vec![]);
    assert_eq!((kept_all.route_use.as_str(), kept_all.applied), (ROUTE_USE_APPLIED, true), "the judgment kept everything, and that is what the summary reads");
    assert!(judgment.applies && judgment.dropped.is_empty());

    let (refused, judgment) = settle(JevMode::On, true, row(Refused::NoKey.token()), vec![]);
    assert_eq!((refused.route_use.as_str(), refused.applied), (ROUTE_USE_FALLBACK, false));
    assert_eq!(judgment, CompactionJudgment::default());

    // `auto` unraised is a recording mode; raised, it acts.
    let (auto, _) = settle(JevMode::Auto, JevMode::Auto.applies_with(false), row(COMPACTION_OUTCOME_ANSWERED), vec![1]);
    assert_eq!((auto.route_use.as_str(), auto.applied), ("auto", false));
    let (risen, judgment) = settle(JevMode::Auto, JevMode::Auto.applies_with(true), row(COMPACTION_OUTCOME_ANSWERED), vec![1]);
    assert_eq!((risen.route_use.as_str(), risen.applied), (ROUTE_USE_APPLIED, true));
    assert!(judgment.applies);
}

/// The label: a dropped block read again inside the window is regret, the
/// rest agree at the window's end, and a cancelled turn is not a turn.
#[test]
fn a_dropped_block_read_again_inside_the_window_is_regret_and_the_rest_agree_at_its_end() {
    let session = session();
    let (ask, _) = ask_of(&session);
    let cwd = tempfile::tempdir().expect("a project");
    let ledger_dir = tempfile::tempdir().expect("a ledger");
    let ledger = ledger_dir.path().join(COMPACTION_RELEVANCE_FILE);
    forget_pending(cwd.path());
    let (row, _) = settle(JevMode::On, true, CompactionRow::new(&ask, 1, COMPACTION_OUTCOME_ANSWERED.to_string()), vec![0, 1]);
    remember_dropped(cwd.path(), &row, &ask, &[0, 1]);

    let turn = |messages: Vec<ConversationMessage>| messages;
    // Turn 1 reads another file: nothing is decided.
    assert_eq!(
        label_turn(cwd.path(), &ledger, Some(&turn(vec![
            call("r1", "Read", r#"{"path":"/work/src/other.rs"}"#),
            result("r1", "Read", "other", false),
        ]))),
        0
    );
    // A cancelled turn is not a turn of the window.
    assert_eq!(label_turn(cwd.path(), &ledger, None), 0);
    // Turn 2 reads the dropped file again — but the read failed, which is
    // not a read; then reads it for real: regret, at turn 2.
    assert_eq!(
        label_turn(cwd.path(), &ledger, Some(&turn(vec![
            call("r2", "Read", r#"{"path":"/work/src/lib.rs"}"#),
            result("r2", "Read", "no such file", true),
            call("r3", "Read", r#"{"path":"/work/src/lib.rs"}"#),
            result("r3", "Read", "fn add", false),
        ]))),
        1
    );
    // Turns 3 and 4: quiet.
    for _ in 0..2 {
        assert_eq!(label_turn(cwd.path(), &ledger, Some(&turn(vec![user_text("carry on")]))), 0);
    }
    // Turn 5 closes the window: the shell command was never run again.
    assert_eq!(COMPACTION_REGRET_TURNS, 5, "the turns below are the window's");
    assert_eq!(label_turn(cwd.path(), &ledger, Some(&turn(vec![user_text("done")]))), 1);
    // And nothing waits any more.
    assert_eq!(label_turn(cwd.path(), &ledger, Some(&turn(vec![
        call("r4", "Bash", r#"{"command":"cargo test"}"#),
        result("r4", "Bash", "ok", false),
    ]))), 0);

    let rows: Vec<CompactionLabelRow> = super::super::shadow_ledger::read_shadow_rows(&ledger);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!((rows[0].tool.as_str(), rows[0].agreed, rows[0].turns_later), ("Read", false, 2));
    assert_eq!((rows[1].tool.as_str(), rows[1].agreed, rows[1].turns_later), ("Bash", true, 5));
    for label in &rows {
        assert_eq!(label.kind, LABEL_ROW_KIND);
        assert_eq!(label.label, row.judged.to_string());
        assert!(label.applied);
    }
    // The judge reads them as this seat's agreement: one compared, one agreed.
    let read = super::super::jev_summary::read_rows(&ledger);
    let agreement = zerocode_core::jev::summary::agreement_since(&read, i64::MIN);
    assert_eq!((agreement.compared, agreement.agreed), (2, 1));
    assert!(
        read.iter().all(|row| zerocode_core::jev::summary::asked_something(row).is_none()),
        "a label is not a request"
    );
}

/// The same call made again — not a read, a re-run — is regret too.
#[test]
fn the_same_call_made_again_is_regret() {
    let session = session();
    let (ask, _) = ask_of(&session);
    let cwd = tempfile::tempdir().expect("a project");
    let ledger_dir = tempfile::tempdir().expect("a ledger");
    let ledger = ledger_dir.path().join(COMPACTION_RELEVANCE_FILE);
    forget_pending(cwd.path());
    let (row, _) = settle(JevMode::Shadow, false, CompactionRow::new(&ask, 1, COMPACTION_OUTCOME_ANSWERED.to_string()), vec![1]);
    remember_dropped(cwd.path(), &row, &ask, &[1]);
    assert_eq!(
        label_turn(cwd.path(), &ledger, Some(&[
            call("r1", "Bash", r#"{"command":"cargo test"}"#),
            result("r1", "Bash", "ok", false),
        ])),
        1
    );
    let rows: Vec<CompactionLabelRow> = super::super::shadow_ledger::read_shadow_rows(&ledger);
    assert_eq!((rows[0].agreed, rows[0].applied, rows[0].turns_later), (false, false, 1));
}

/// The seat a host installs answers through the same road, keyed by the
/// project it was opened for.
#[test]
fn the_installed_seat_is_the_projects_own() {
    let seat = CompactionJudge::at(Path::new("/work/zo"));
    assert!(format!("{seat:?}").contains("/work/zo"));
    let installed: Arc<dyn CompactionSeat> = Arc::new(seat);
    assert_eq!(Arc::strong_count(&installed), 1);
}

/* ---- the replay: this machine's own compaction points -------------------- */

/// Where the replay seed says which transcripts to read
/// (`tools/compaction-replay/seed.py`).
const REPLAY_SEED_ENV: &str = "ZEROCODE_COMPACTION_REPLAY_SEED";

/// A token budget in place of each model's own compaction threshold, so a
/// machine with few transcripts at the real threshold still yields points.
const REPLAY_BUDGET_ENV: &str = "ZEROCODE_COMPACTION_REPLAY_BUDGET_TOKENS";

/// Every message a transcript ever held, in order: the vault's evicted
/// records below `first_message_index`, then what is still live. `None`
/// when the vault has a hole, since an index would then not be a seq.
fn whole_history(session: &Session) -> Option<Vec<ConversationMessage>> {
    let first_live = session.first_message_index();
    let mut evicted: Vec<ConversationMessage> = Vec::new();
    for record in session.read_vault() {
        if record.vault_seq >= first_live {
            break;
        }
        if record.vault_seq != u32::try_from(evicted.len()).ok()? {
            return None;
        }
        evicted.push(record.message);
    }
    if u32::try_from(evicted.len()).ok()? != first_live {
        return None;
    }
    evicted.extend(session.messages.iter().cloned());
    Some(evicted)
}

/// The turns after a cut, as the label counts them: each begins at a user
/// message that spoke.
fn turns_after(messages: &[ConversationMessage]) -> Vec<&[ConversationMessage]> {
    let mut starts: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter(|(_, message)| {
            message.role == MessageRole::User
                && message.blocks.iter().any(|block| matches!(block, ContentBlock::Text { .. }))
        })
        .map(|(index, _)| index)
        .collect();
    if starts.first() != Some(&0) {
        starts.insert(0, 0);
    }
    starts.push(messages.len());
    starts.windows(2).map(|pair| &messages[pair[0]..pair[1]]).collect()
}

fn percentile(sorted: &[u64], share: f64) -> u64 {
    zerocode_core::jev::summary::percentile(sorted, share).unwrap_or(0)
}

/// The one point a transcript is replayed at (t-6155 F5): its first recorded
/// compaction when it has one, else the first message at which its own
/// estimate crossed `threshold`. One and not both, because a threshold point
/// on a transcript that later compacted sits inside the recorded point's
/// blocks — the same tool results asked about twice, which the README says
/// never happens — and because two points on one transcript are not two
/// samples of this machine's compactions.
fn replay_point(recorded: &[usize], history: &[ConversationMessage], threshold: u64) -> Option<(&'static str, usize)> {
    if let Some(cut) = recorded.first() {
        return Some(("recorded", *cut));
    }
    let mut running = 0usize;
    for (index, message) in history.iter().enumerate() {
        running += runtime::estimate_session_tokens(&{
            let mut one = Session::new();
            one.push_message(message.clone()).expect("push");
            one
        });
        if running as u64 >= threshold {
            return Some(("threshold", index + 1));
        }
    }
    None
}

/// The median of a set of shares, for a table that must not let one point
/// speak for the rest.
fn median(shares: &mut [f64]) -> f64 {
    if shares.is_empty() {
        return 0.0;
    }
    shares.sort_by(f64::total_cmp);
    let mid = shares.len() / 2;
    if shares.len().is_multiple_of(2) {
        f64::midpoint(shares[mid - 1], shares[mid])
    } else {
        shares[mid]
    }
}

/// The one point a transcript is replayed at is its recorded cut when it
/// has one, else its threshold crossing, never both (t-6155 F5).
#[test]
fn a_transcript_is_replayed_at_one_point_recorded_first() {
    let history: Vec<ConversationMessage> = (0..6).map(|n| user_text(&"x".repeat(4_000 * (n + 1)))).collect();
    assert_eq!(replay_point(&[3, 5], &history, 1), Some(("recorded", 3)));
    let (kind, cut) = replay_point(&[], &history, 2_000).expect("the estimate crosses");
    assert_eq!(kind, "threshold");
    assert!((1..=6).contains(&cut));
    assert_eq!(replay_point(&[], &history, u64::MAX), None, "an estimate that never crosses is no point");
    for (shares, expected) in [(vec![3.0, 1.0, 2.0], 2.0), (vec![4.0, 1.0, 3.0, 2.0], 2.5), (Vec::new(), 0.0)] {
        let mut shares = shares;
        assert!((median(&mut shares) - expected).abs() < f64::EPSILON, "{shares:?} -> {expected}");
    }
}

/// Replays this machine's own compaction points against the real endpoint
/// and prints the table the brief asks for: summary-input tokens before and
/// after, the share of blocks dropped, the regret rate over the turns after
/// each cut, the batch wall's p50/p95 and the cost — pooled, and per point
/// as medians, because one long transcript once carried half the pooled
/// blocks (t-6155 F5). One point per transcript, the recorded cut first.
/// The regret share carries a Wilson lower bound on "not regretted"; it is
/// a floor, since a re-read is only counted when the same call or the same
/// path comes back (`turn_reads::read_path`).
///
/// Every input a judgment reads is from before its cut — the session is
/// rebuilt from the messages up to it — and the turns after are read only
/// as the label. Rows go nowhere: the door stands in a temporary home, and
/// `judge` writes no ledger.
///
/// ```sh
/// python3 tools/compaction-replay/seed.py --out /tmp/compaction-replay/seed.json
/// TYPESAFE_API_KEY=$(security find-generic-password -s dev.zerocode.key.TYPESAFE_API_KEY -a "$USER" -w) \
/// ZEROCODE_COMPACTION_REPLAY_SEED=/tmp/compaction-replay/seed.json \
///   cargo test -p tools --lib compaction_seat::tests::the_compactions_this_machine_would_have_made -- --ignored --nocapture
/// ```
// One measurement, read top to bottom: the cut, the ask, the label and the
// table are one story, and a split would only move the reader between them.
#[allow(clippy::too_many_lines)]
#[test]
#[ignore = "reads this machine's transcripts through tools/compaction-replay/seed.py and asks the real endpoint"]
fn the_compactions_this_machine_would_have_made() {
    let seed_path = std::env::var(REPLAY_SEED_ENV).expect("ZEROCODE_COMPACTION_REPLAY_SEED names the seed");
    let seed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&seed_path).expect("the seed reads")).expect("the seed parses");
    let budget: Option<u64> = std::env::var(REPLAY_BUDGET_ENV).ok().and_then(|raw| raw.trim().parse().ok());
    assert_eq!(
        seed["regretTurns"].as_u64(),
        Some(u64::from(COMPACTION_REGRET_TURNS)),
        "the seed was made for another window"
    );
    let client = api::SystemOneConfig::from_env().expect("TYPESAFE_API_KEY in the environment").into_client();
    let home = tempfile::tempdir().expect("a config home of the replay's own");
    let door = door(home.path());

    let mut cuts = 0usize;
    let mut skipped = 0usize;
    let (mut candidates_total, mut dropped_total, mut regretted_total) = (0usize, 0usize, 0usize);
    let (mut tokens_before, mut tokens_after) = (0usize, 0usize);
    let (mut input_tokens, mut requests) = (0u64, 0u32);
    let mut walls: Vec<u64> = Vec::new();
    let mut outcomes: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let mut per_cut: Vec<String> = Vec::new();
    // Per point (t-6155 F5): the drop share and the token share removed,
    // for medians beside the pooled shares.
    let (mut drop_shares, mut removed_shares): (Vec<f64>, Vec<f64>) = (Vec::new(), Vec::new());

    for transcript in seed["transcripts"].as_array().expect("transcripts") {
        let path = transcript["path"].as_str().expect("a path");
        let Ok(session) = Session::load_from_path(path) else {
            skipped += 1;
            continue;
        };
        let Some(history) = whole_history(&session) else {
            skipped += 1;
            continue;
        };
        let model = transcript["model"].as_str();
        let window = model.map_or(0, api::context_window_for_model);
        let threshold = budget.unwrap_or_else(|| u64::from(runtime::auto_compaction_threshold_for_model(model, window)));
        // The transcript's name in the table: the file's stem, which is a
        // session id and not a path.
        let id = Path::new(path).file_stem().and_then(|stem| stem.to_str()).unwrap_or("?");
        let recorded: Vec<usize> = transcript["recordedCuts"]
            .as_array()
            .map(|cuts| {
                cuts.iter()
                    .filter_map(serde_json::Value::as_u64)
                    .map(|cut| usize::try_from(cut).unwrap_or(usize::MAX))
                    .collect()
            })
            .unwrap_or_default();
        // One point per transcript: the recorded cut first, else the
        // threshold crossing.
        let points = replay_point(&recorded, &history, threshold).into_iter();
        for (kind, cut) in points {
            if cut == 0 || cut > history.len() {
                continue;
            }
            let mut at_cut = Session::new();
            for message in &history[..cut] {
                at_cut.push_message(message.clone()).expect("push");
            }
            let tail = runtime::preserved_tail_len_for_budget(&at_cut.messages, runtime::auto_compaction_tail_budget(window));
            let Some(plan) = prepare_compaction(&at_cut, CompactionConfig { preserve_recent_messages: tail, max_estimated_tokens: 0 }) else {
                continue;
            };
            let Some(ask) = ask_for(&at_cut, &plan, MICROCOMPACT_MIN_OUTPUT_BYTES, "replay") else {
                continue;
            };
            let (row, dropped) = api::sync_bridge::run_blocking(judge(&door, Some(&client), &ask, false));
            *outcomes.entry(row.outcome.clone()).or_default() += 1;
            walls.push(row.elapsed_ms);
            input_tokens += row.input_tokens.unwrap_or(0);
            requests += row.requests;
            let before = summary_input_tokens(&plan.messages_to_compact);
            let mut applied = plan.clone();
            let left = drop_blocks(&mut applied, &at_cut, &ask.blocks, &dropped);
            let after = summary_input_tokens(&applied.messages_to_compact);
            // The label: the window of turns after the cut, read for what
            // they went back for.
            let after_cut = turns_after(&history[cut..]);
            let mut regretted = 0usize;
            let dropped_blocks: Vec<&BlockHead> = ask.blocks.iter().filter(|block| dropped.contains(&block.position)).collect();
            let mut still: Vec<(u64, Option<String>)> = dropped_blocks
                .iter()
                .map(|block| (call_key(&block.tool_name, &block.input), read_path(&block.tool_name, &block.input)))
                .collect();
            for turn in after_cut.iter().take(COMPACTION_REGRET_TURNS as usize) {
                let (read, calls) = went_back_for(turn);
                still.retain(|(key, path)| {
                    let back = calls.contains(key) || path.as_ref().is_some_and(|path| read.contains(path));
                    regretted += usize::from(back);
                    !back
                });
            }
            cuts += 1;
            candidates_total += ask.blocks.len();
            dropped_total += left;
            regretted_total += regretted;
            tokens_before += before;
            tokens_after += after;
            drop_shares.push(share(left, ask.blocks.len()));
            removed_shares.push(share(before.saturating_sub(after), before));
            per_cut.push(format!(
                "{kind:>9} cut={cut:>4} asked={:>3} dropped={:>3} regret={:>2} tokens {before:>7}→{after:>7} wall={:>5} ms {} {id}",
                ask.blocks.len(),
                left,
                regretted,
                row.elapsed_ms,
                row.outcome
            ));
        }
    }
    let mut sorted = walls.clone();
    sorted.sort_unstable();
    let cost = api::systemone_rate(api::SYSTEMONE_MODEL).map(|rate| rate.input_cost_usd(input_tokens));
    println!("--- compaction replay: {cuts} cuts ({skipped} transcripts skipped), budget={budget:?}");
    for line in &per_cut {
        println!("{line}");
    }
    println!("outcomes: {outcomes:?}");
    println!(
        "blocks asked: {candidates_total}, dropped: {dropped_total} ({:.1}% pooled; per-point median {:.1}%)",
        share(dropped_total, candidates_total),
        median(&mut drop_shares)
    );
    println!(
        "summary input tokens: {tokens_before} → {tokens_after} ({:.1}% removed pooled; per-point median {:.1}%)",
        share(tokens_before.saturating_sub(tokens_after), tokens_before),
        median(&mut removed_shares)
    );
    let kept_bound = if dropped_total == 0 {
        0.0
    } else {
        zerocode_core::jev::summary::wilson_lower(
            dropped_total.saturating_sub(regretted_total),
            dropped_total,
            zerocode_core::jev::summary::WILSON_Z_95,
        )
    };
    println!(
        "regret: {regretted_total} of {dropped_total} dropped ({:.1}%; a floor — not-regretted Wilson lower {:.1}%)",
        share(regretted_total, dropped_total),
        100.0 * kept_bound
    );
    println!("batch wall ms: p50 {} p95 {} max {}", percentile(&sorted, 0.5), percentile(&sorted, 0.95), sorted.last().copied().unwrap_or(0));
    println!("requests: {requests}, input tokens: {input_tokens}, cost: {cost:?} USD");
}

#[allow(clippy::cast_precision_loss)]
fn share(part: usize, whole: usize) -> f64 {
    if whole == 0 {
        0.0
    } else {
        part as f64 * 100.0 / whole as f64
    }
}
