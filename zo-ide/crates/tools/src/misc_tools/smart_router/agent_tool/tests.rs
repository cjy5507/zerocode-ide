//! What the seat promises: a question is checked before a byte leaves; `off`
//! sends nothing and writes nothing; `shadow` asks, writes and withholds;
//! `on` hands the answer over by place; a score is even shards folded into
//! one answer; and the row is spelled the way every Jev counter reads it.
//!
//! Hermetic: the wire is the shared fake (`jev_mock`), the settings are a
//! temp home, and the only key is the word `test-key`.

use std::path::Path;

use serde_json::{json, Value};
use zerocode_core::jev::summary::{self, LEDGER_KEYS};
use zerocode_core::jev::{AGENT_TOOL, AGENT_TOOL_ITEM_CAP, AGENT_TOOL_OPTION_CAP};

use super::super::jev_mock::{machine, machine_live, Mock};
use super::super::shadow_ledger::read_shadow_rows;
use super::*;

fn strings(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| (*word).to_string()).collect()
}

fn choose(options: &[&str]) -> JevQuestion {
    JevQuestion::new(
        JevShape::Choose,
        "Which topic is this page about?",
        Some("The page describes the worker ledger and its seats."),
        &strings(options),
        &[],
        &[],
    )
    .expect("a well-formed choose")
}

fn ask() -> JevQuestion {
    JevQuestion::new(JevShape::Ask, "Does the diff touch the wire?", Some("+ fn send_once()"), &[], &[], &[])
        .expect("a well-formed ask")
}

fn score(items: usize) -> JevQuestion {
    let items: Vec<String> = (0..items).map(|at| format!("item number {at}")).collect();
    JevQuestion::new(
        JevShape::Score,
        "How risky is the change?",
        None,
        &[],
        &strings(&["Comments or docs only.", "Logic in one function.", "A public contract changes."]),
        &items,
    )
    .expect("a well-formed score")
}

/// A choice answer that puts the whole probability on `chosen` among `offered`.
fn choice_reply(id: &str, offered: &[&str], chosen: &str) -> String {
    let probabilities: serde_json::Map<String, Value> = offered
        .iter()
        .map(|name| ((*name).to_string(), json!(if *name == chosen { 1.0 } else { 0.0 })))
        .collect();
    json!({
        "model": "jev-test",
        "answers": { id: { "type": "choice", "choice": chosen, "probabilities": probabilities, "confidence": 0.9 } },
        "usage": { "input_tokens": 321, "output_tokens": 12 }
    })
    .to_string()
}

/// A score answer whose whole probability sits on `level` of a three-level scale.
fn score_answer(level: usize) -> Value {
    let probabilities: serde_json::Map<String, Value> =
        (0..3).map(|at| (at.to_string(), json!(if at == level { 1.0 } else { 0.0 }))).collect();
    let legend: serde_json::Map<String, Value> =
        (0..3).map(|at| (at.to_string(), json!(format!("level {at}")))).collect();
    #[allow(clippy::cast_precision_loss)]
    let score = level as f64;
    json!({ "type": "score", "score": score, "confidence": 0.8, "legend": legend, "probabilities": probabilities })
}

/// The question ids one request body asked under.
fn asked_ids(request: &str) -> Vec<String> {
    let body: Value = serde_json::from_str(request).expect("a JSON body");
    body["questions"].as_object().map(|questions| questions.keys().cloned().collect()).unwrap_or_default()
}

/// A responder that answers every `i<n>` a request asked with level `n % 3`.
fn score_responder(request: &str) -> (u16, String) {
    let answers: serde_json::Map<String, Value> = asked_ids(request)
        .into_iter()
        .map(|id| {
            let at: usize = id.trim_start_matches('i').parse().expect("an item id");
            (id, score_answer(at % 3))
        })
        .collect();
    (
        200,
        json!({ "model": "jev-test", "answers": answers, "usage": { "input_tokens": 1_000, "output_tokens": 0 } })
            .to_string(),
    )
}

fn rows(cwd: &Path) -> Vec<AgentToolRow> {
    read_shadow_rows(&agent_tool_path(cwd))
}

#[test]
fn a_question_is_checked_before_anything_leaves() {
    let refused = |shape, question: &str, context: Option<&str>, options: &[&str], levels: &[&str], items: &[&str]| {
        JevQuestion::new(shape, question, context, &strings(options), &strings(levels), &strings(items))
            .expect_err("refused")
            .0
    };
    assert_eq!(refused(JevShape::Ask, "  ", None, &[], &[], &[]), "question is empty");
    assert_eq!(refused(JevShape::Ask, "q", None, &["a"], &[], &[]), "`ask` does not read options");
    assert_eq!(refused(JevShape::Choose, "q", None, &["only"], &[], &[]), "choose needs at least two options");
    let many: Vec<&str> = std::iter::repeat_n("x", AGENT_TOOL_OPTION_CAP + 1).collect();
    assert_eq!(
        refused(JevShape::Choose, "q", None, &many, &[], &[]),
        format!("choose takes at most {AGENT_TOOL_OPTION_CAP} options, not {}", AGENT_TOOL_OPTION_CAP + 1)
    );
    assert_eq!(refused(JevShape::Choose, "q", None, &["a", " "], &[], &[]), "options[1] is empty");
    assert_eq!(refused(JevShape::Score, "q", None, &[], &["one"], &["i"]), "score takes between 2 and 10 levels, not 1");
    let eleven: Vec<&str> = std::iter::repeat_n("l", 11).collect();
    assert_eq!(refused(JevShape::Score, "q", None, &[], &eleven, &["i"]), "score takes between 2 and 10 levels, not 11");
    assert_eq!(refused(JevShape::Score, "q", None, &[], &["a", "b"], &[]), "score needs at least one item");
    let too_many: Vec<&str> = std::iter::repeat_n("i", AGENT_TOOL_ITEM_CAP + 1).collect();
    assert_eq!(
        refused(JevShape::Score, "q", None, &[], &["a", "b"], &too_many),
        format!("score takes at most {AGENT_TOOL_ITEM_CAP} items per call, not {}", AGENT_TOOL_ITEM_CAP + 1)
    );
    assert_eq!(refused(JevShape::Score, "q", Some("c"), &[], &["a", "b"], &["i"]), "`score` does not read context");

    // Trimmed, and the parts a shape reads are kept as the caller wrote them.
    let kept = JevQuestion::new(JevShape::Choose, " q ", Some("  "), &strings(&[" a ", "b"]), &[], &[]).expect("kept");
    assert_eq!(
        kept,
        JevQuestion::Choose { question: "q".to_string(), options: strings(&["a", "b"]), context: None }
    );
    assert_eq!(kept.candidates(), 2);
    assert_eq!(ask().candidates(), 2, "an ask decides between yes and no");
    assert_eq!(score(7).candidates(), 7);
}

/// The seat off is today: nothing leaves, nothing is written, and the
/// verdict says so in the switch's own word.
#[test]
fn off_sends_nothing_writes_nothing_and_says_so() {
    let mock = Mock::serving(200, choice_reply("c", &["o0", "o1"], "o1"));
    let (verdict, written) = machine(&AGENT_TOOL, JevMode::Off.key(), &mock.base_url, |cwd| {
        (decide(cwd, JevCaller::Tool, &choose(&["a", "b"])), agent_tool_path(cwd).exists())
    });
    assert_eq!(verdict.outcome, "off");
    assert_eq!(verdict.route_use, "off");
    assert_eq!(verdict.answer, None);
    assert_eq!(verdict.requests, 0);
    assert!(verdict.note.contains("smart.agentTool is off"), "{}", verdict.note);
    assert!(mock.requests().is_empty(), "off sent {:?}", mock.requests());
    assert!(!written, "off wrote a row");
}

/// `shadow` asks and writes the row, and hands the caller nothing — what a
/// person reads on the dashboard before letting agents act on the answers.
#[test]
fn shadow_asks_writes_the_row_and_withholds_the_answer() {
    let mock = Mock::serving(200, choice_reply("c", &["o0", "o1", "o2"], "o1"));
    let (verdict, rows) = machine(&AGENT_TOOL, JevMode::Shadow.key(), &mock.base_url, |cwd| {
        (decide(cwd, JevCaller::Cli, &choose(&["jev", "terminal", "release"])), rows(cwd))
    });
    assert_eq!(verdict.outcome, AGENT_TOOL_OUTCOME_ANSWERED);
    assert_eq!(verdict.route_use, JevMode::Shadow.key());
    assert_eq!(verdict.answer, None, "a recorded answer is withheld");
    assert_eq!(verdict.requests, 1);
    assert_eq!(verdict.input_tokens, Some(321));
    assert!(verdict.note.contains("withheld"), "{}", verdict.note);

    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.caller, JevCaller::Cli);
    assert_eq!(row.shape, JevShape::Choose);
    assert_eq!(row.outcome, AGENT_TOOL_OUTCOME_ANSWERED);
    assert_eq!(row.route_use, JevMode::Shadow.key());
    assert_eq!(row.candidates, 3);
    assert_eq!((row.shards, row.shards_answered), (1, 1));
    assert_eq!(row.chosen.as_deref(), Some("o1"), "the row names the place, never the word");
    assert_eq!(row.requests, Some(1));
    assert_eq!(row.model.as_deref(), Some("jev-test"));

    // What left: the caller's words in the state and the criteria, under
    // ids of ours — and the door's fixed sentence, not the caller's, as the
    // instructions.
    let sent = mock.requests();
    assert_eq!(sent.len(), 1);
    let body: Value = serde_json::from_str(&sent[0]).expect("a JSON body");
    assert_eq!(body["state"]["question"], "Which topic is this page about?");
    assert_eq!(body["questions"]["c"]["type"], "choice");
    assert_eq!(body["questions"]["c"]["instructions"], CHOOSE_INSTRUCTIONS);
    assert_eq!(body["questions"]["c"]["criteria"], json!({ "o0": "jev", "o1": "terminal", "o2": "release" }));
}

/// `on` hands the choice over by its place in the caller's list, with one
/// probability per option in the caller's order, and prices the call.
#[test]
fn on_hands_the_choice_over_by_place() {
    let mock = Mock::serving(200, choice_reply("c", &["o0", "o1", "o2"], "o2"));
    let verdict = machine(&AGENT_TOOL, JevMode::On.key(), &mock.base_url, |cwd| {
        decide(cwd, JevCaller::Tool, &choose(&["jev", "terminal", "release"]))
    });
    assert_eq!(verdict.route_use, ROUTE_USE_APPLIED);
    assert_eq!(
        verdict.answer,
        Some(JevAnswer::Choose {
            chosen: "release".to_string(),
            index: 2,
            probabilities: vec![0.0, 0.0, 1.0],
            confidence: 0.9,
        })
    );
    assert!(verdict.answered());
    let cost = verdict.cost_usd.expect("the price table names jev-latest");
    assert!(cost > 0.0 && cost < 0.001, "321 tokens at the wire's rate: {cost}");
    assert_eq!(verdict.model.as_deref(), Some("jev-test"));
}

/// An `ask` is a choice over the two words of the table, and the answer's
/// `yes` is whether the affirmative was chosen. Each of the two says what it
/// means, of the state the question reads (t-10010): version 1 offered them
/// by name alone.
#[test]
fn ask_is_a_choice_over_yes_and_no() {
    let mock = Mock::serving(200, choice_reply("a", &["yes", "no"], "yes"));
    let (verdict, rows) = machine(&AGENT_TOOL, JevMode::On.key(), &mock.base_url, |cwd| {
        (decide(cwd, JevCaller::Cli, &ask()), rows(cwd))
    });
    assert_eq!(verdict.answer, Some(JevAnswer::Ask { yes: true, p_yes: 1.0, confidence: 0.9 }));
    assert_eq!(rows[0].chosen.as_deref(), Some("yes"));
    assert_eq!(rows[0].candidates, 2);
    let body: Value = serde_json::from_str(&mock.requests()[0]).expect("a JSON body");
    assert_eq!(body["questions"]["a"]["instructions"], ASK_INSTRUCTIONS);
    let criteria = body["questions"]["a"]["criteria"].as_object().expect("named options");
    let mut offered: Vec<&str> = criteria.keys().map(String::as_str).collect();
    offered.sort_unstable();
    assert_eq!(offered, ["no", "yes"]);
    for (option, means) in criteria {
        let means = means.as_str().unwrap_or_default();
        for key in ["`question`", "`context`"] {
            assert!(means.contains(key), "{option} says nothing of {key}: {means:?}");
        }
    }
    assert_eq!(body["state"]["context"], "+ fn send_once()");
}

/// A score of more items than one request carries is even shards asked at
/// once, and their readings fold back into the caller's order.
#[test]
fn score_asks_one_question_per_item_in_even_shards_and_folds_them() {
    let mock = Mock::answering(score_responder);
    let (verdict, rows) = machine(&AGENT_TOOL, JevMode::On.key(), &mock.base_url, |cwd| {
        (decide(cwd, JevCaller::Tool, &score(60)), rows(cwd))
    });
    let Some(JevAnswer::Score { items }) = &verdict.answer else {
        panic!("a score answer: {verdict:?}");
    };
    assert_eq!(items.len(), 60);
    for (at, item) in items.iter().enumerate() {
        assert_eq!(item.index, at, "the caller's order");
        assert_eq!(item.item, format!("item number {at}"));
        #[allow(clippy::cast_precision_loss)]
        let expected = (at % 3) as f64;
        assert!((item.score - expected).abs() < f64::EPSILON, "{item:?}");
        assert!((item.normalised - expected / 2.0).abs() < f64::EPSILON, "{item:?}");
    }
    assert_eq!(verdict.requests, 2, "sixty items are two shards of thirty");
    assert_eq!(verdict.input_tokens, Some(2_000), "both shards' tokens");
    let sent = mock.requests();
    assert_eq!(sent.len(), 2);
    let mut asked: Vec<String> = sent.iter().flat_map(|request| asked_ids(request)).collect();
    asked.sort();
    let mut expected: Vec<String> = (0..60).map(|at| format!("i{at}")).collect();
    expected.sort();
    assert_eq!(asked, expected, "every item asked exactly once, under its own id");
    // The instructions name the item's place in THIS shard's state.
    let body: Value = serde_json::from_str(&sent[0]).expect("a JSON body");
    let first = body["questions"].as_object().and_then(|questions| questions.values().next()).expect("a question");
    assert!(first["instructions"].as_str().is_some_and(|words| words.contains("`items[")), "{first}");
    assert_eq!(first["criteria"], json!(["Comments or docs only.", "Logic in one function.", "A public contract changes."]));
    assert_eq!(body["state"]["items"].as_array().map(Vec::len), Some(30));
    assert_eq!((rows[0].shards, rows[0].shards_answered, rows[0].candidates), (2, 2, 60));
    assert_eq!(rows[0].chosen, None, "a score names no one word");
}

/// A request that was refused takes its own items out of the answer and
/// nothing else, and the note says how many came back.
#[test]
fn a_refused_shard_leaves_the_other_shards_items_standing() {
    let mock = Mock::answering(|request| {
        if asked_ids(request).iter().any(|id| id == "i0") {
            (422, r#"{"detail":"refused"}"#.to_string())
        } else {
            score_responder(request)
        }
    });
    let (verdict, rows) = machine(&AGENT_TOOL, JevMode::On.key(), &mock.base_url, |cwd| {
        (decide(cwd, JevCaller::Tool, &score(60)), rows(cwd))
    });
    let Some(JevAnswer::Score { items }) = &verdict.answer else {
        panic!("a score answer: {verdict:?}");
    };
    assert_eq!(items.len(), 30);
    assert_eq!(items[0].index, 30, "the refused shard's items are gone, the rest stand");
    assert_eq!(verdict.outcome, AGENT_TOOL_OUTCOME_ANSWERED);
    assert!(verdict.note.starts_with("30 of 60 items were scored"), "{}", verdict.note);
    assert_eq!((rows[0].shards, rows[0].shards_answered), (2, 1));
}

/// Nothing answered: the verdict says why in the wire's word, the answer is
/// absent, and the row carries the same word.
#[test]
fn a_wire_that_refuses_is_a_fallback_that_says_why() {
    let mock = Mock::serving(401, r#"{"detail":"bad key"}"#.to_string());
    let (verdict, rows) = machine(&AGENT_TOOL, JevMode::On.key(), &mock.base_url, |cwd| {
        (decide(cwd, JevCaller::Tool, &ask()), rows(cwd))
    });
    assert_eq!(verdict.outcome, "unauthorized");
    assert_eq!(verdict.route_use, ROUTE_USE_FALLBACK);
    assert_eq!(verdict.answer, None);
    assert!(verdict.note.starts_with("No answer (unauthorized)"), "{}", verdict.note);
    assert_eq!(rows[0].outcome, "unauthorized");
    assert_eq!(rows[0].route_use, ROUTE_USE_FALLBACK);
}

/// A reply that names an option nobody offered is refused whole, and the
/// row says which rule refused it.
#[test]
fn a_reply_naming_an_unoffered_option_is_refused_by_the_shared_reader() {
    let mock = Mock::serving(200, choice_reply("c", &["o0", "o1", "o9"], "o9"));
    let (verdict, rows) = machine(&AGENT_TOOL, JevMode::On.key(), &mock.base_url, |cwd| {
        (decide(cwd, JevCaller::Tool, &choose(&["a", "b", "c"])), rows(cwd))
    });
    assert_eq!(verdict.outcome, "schema");
    assert_eq!(verdict.answer, None);
    assert_eq!(rows[0].rejected.as_deref(), Some("schema_unknown_option"));
    assert!(verdict.note.contains("schema_unknown_option"), "{}", verdict.note);
}

/// The door stands in front of this seat as in front of every other: a
/// workspace nobody consented to sends nothing and the row says so.
#[test]
fn the_door_refuses_a_workspace_nobody_consented_to() {
    let mock = Mock::serving(200, choice_reply("a", &["yes", "no"], "yes"));
    let elsewhere = tempfile::tempdir().expect("an unconsented workspace");
    let elsewhere = std::fs::canonicalize(elsewhere.path()).expect("resolved");
    let (verdict, rows) = machine(&AGENT_TOOL, JevMode::On.key(), &mock.base_url, |_consented| {
        (decide(&elsewhere, JevCaller::Cli, &ask()), rows(&elsewhere))
    });
    assert_eq!(verdict.outcome, Refused::NotConsented.token());
    assert_eq!(verdict.route_use, ROUTE_USE_FALLBACK);
    assert_eq!(verdict.requests, 0);
    assert!(mock.requests().is_empty());
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].outcome, Refused::NotConsented.token());
    assert_eq!(rows[0].requests, Some(0));
}

/// A row carries the columns every Jev counter reads, in the canonical
/// spelling, and `zo jev summary` counts the seat from its own ledger with
/// no reader written for it.
#[test]
fn a_row_is_spelled_the_way_every_jev_counter_reads_it_and_the_summary_counts_it() {
    let question = choose(&["a", "b"]);
    let mut row = AgentToolRow::new(JevCaller::Tool, &question, 1, AGENT_TOOL_OUTCOME_ANSWERED.to_string());
    row.at = 1_700_000_000_000;
    row.elapsed_ms = 410;
    row.input_tokens = Some(321);
    row.requests = Some(1);
    row.route_use = ROUTE_USE_APPLIED.to_string();
    row.chosen = Some("o1".to_string());
    row.confidence = Some(0.9);
    let value: Value = serde_json::to_value(&row).expect("a row serializes");
    for spelled in ["at", "outcome", "elapsedMs", "requests", "redactedLines", "inputTokens", "routeUse", "candidates"] {
        assert!(value.get(spelled).is_some(), "a row says `{spelled}`: {value}");
    }
    for key in LEDGER_KEYS {
        if let Some(read) = key.read(&value) {
            assert_eq!(Some(read), value.get(key.canonical), "`{}` is read from a spelling this row does not write", key.canonical);
        }
    }
    assert_eq!(value["caller"], "tool");
    assert_eq!(value["shape"], "choose");
    assert_eq!(value["rubricVersion"], AGENT_TOOL_RUBRIC_VERSION);
    assert!(summary::asked_something(&value).is_some());
    // The dashboard's digest reads the choice off the row's own columns.
    let digest = zerocode_core::jev::recent::recent(std::slice::from_ref(&value), 1);
    assert_eq!(digest[0].answered, json!("o1"));
    assert_eq!(digest[0].confidence, Some(0.9));
    assert_eq!(digest[0].asked.get("candidates"), Some(&json!(2)));

    let root = tempfile::tempdir().expect("a temp root");
    let ledger = root.path().join(AGENT_TOOL_FILE);
    let mut refused = AgentToolRow::new(JevCaller::Cli, &question, 1, Refused::NoKey.token().to_string());
    refused.at = 1_700_000_001_000;
    for row in [&row, &refused] {
        append_shadow_row(&ledger, row, SHADOW_LEDGER_MAX_BYTES).expect("append");
    }
    let roots = vec![root.path().to_path_buf()];
    let report = super::super::jev_summary::report(
        &roots,
        None,
        Some(&json!({ "smart": { AGENT_TOOL.setting: "on" } })),
        1_700_000_002_000,
        0,
    );
    let seat = report.iter().find(|seat| seat.id == AGENT_TOOL.id).expect("the seat is in the report");
    assert_eq!(seat.mode, JevMode::On);
    assert_eq!(seat.found.as_deref(), Some(ledger.as_path()));
    assert_eq!((seat.week.rows, seat.week.answered, seat.week.refused, seat.week.applied), (2, 1, 1, 1));
    assert_eq!(seat.week.input_tokens, 321);
    assert_eq!(seat.rise_floor_permille, None, "the seat never rises");
    assert_eq!(seat.judged, None, "nothing judges a seat with no floor");
}

/// Every sentence this seat asks with, in one string, for the pin: the three
/// instructions, what an `ask`'s two answers mean, and the state's keys.
fn rubric_words() -> String {
    format!(
        "{ASK_INSTRUCTIONS}\n{CHOOSE_INSTRUCTIONS}\n{}\n{}\n{}",
        score_instructions(0),
        ASK_OPTION_MEANS.join("\n"),
        STATE_KEYS.join(",")
    )
}

/// The sentences this seat asks with are pinned to its rubric version: a
/// word changed without a bump is a red test, not a quiet drift of every
/// count the ledger holds.
#[test]
fn the_rubric_is_pinned_to_its_version() {
    assert_eq!(AGENT_TOOL_RUBRIC_VERSION, 2);
    assert_eq!(zerocode_core::jev::rubric_fingerprint(rubric_words), "27dc981795271706", "{}", rubric_words());
}

/// The shape words are the CLI's verbs and the schema's enum, and the
/// deadline is the table's.
#[test]
fn the_shape_words_and_the_wall_are_the_tables() {
    for shape in JevShape::ALL {
        assert_eq!(JevShape::from_word(shape.word()), Some(shape));
        assert_eq!(serde_json::to_value(shape).expect("a word"), json!(shape.word()));
    }
    assert_eq!(JevShape::from_word("pick"), None);
    assert_eq!(AGENT_TOOL_DEADLINE, Duration::from_millis(AGENT_TOOL_DEADLINE_MS));
    assert_eq!(AGENT_TOOL_FILE, "agent-tool.jsonl");
}

/// One arm's reading of one golden item.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct Reading {
    file: String,
    label: String,
    answered: Option<String>,
    /// Why there is no answer, when there is none: the seat's outcome word
    /// for Jev; `rate_limited`, `timeout` or `error` for the frontier.
    #[serde(skip_serializing_if = "Option::is_none")]
    unanswered: Option<String>,
    agreed: bool,
    elapsed_ms: u64,
    input_tokens: u64,
    output_tokens: u64,
}

/// One arm's table: pooled shares and bounds over its readings, in the
/// units the seat table is written in.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct Arm {
    name: String,
    model: String,
    items: usize,
    answered: usize,
    agreed: usize,
    agreement_share: f64,
    agreement_lower_bound: f64,
    p50_ms: Option<u64>,
    p95_ms: Option<u64>,
    mean_input_tokens: f64,
    mean_output_tokens: f64,
    cost_usd: f64,
    cost_usd_per_item: f64,
    /// Items with no answer, by the reason each gave.
    unanswered: std::collections::BTreeMap<String, usize>,
    /// Per golden label: items, answered, agreed — the pass by label.
    by_label: std::collections::BTreeMap<String, (usize, usize, usize)>,
    readings: Vec<Reading>,
}

impl Arm {
    #[allow(clippy::cast_precision_loss)]
    fn of(name: &str, model: &str, readings: Vec<Reading>, cost: impl Fn(u64, u64) -> f64) -> Self {
        let items = readings.len().max(1);
        let answered = readings.iter().filter(|reading| reading.answered.is_some()).count();
        let agreed = readings.iter().filter(|reading| reading.agreed).count();
        let mut latencies: Vec<u64> =
            readings.iter().filter(|reading| reading.answered.is_some()).map(|reading| reading.elapsed_ms).collect();
        latencies.sort_unstable();
        let input: u64 = readings.iter().map(|reading| reading.input_tokens).sum();
        let output: u64 = readings.iter().map(|reading| reading.output_tokens).sum();
        let cost_usd = cost(input, output);
        let mut unanswered = std::collections::BTreeMap::new();
        let mut by_label: std::collections::BTreeMap<String, (usize, usize, usize)> =
            std::collections::BTreeMap::new();
        for reading in &readings {
            if let Some(why) = &reading.unanswered {
                *unanswered.entry(why.clone()).or_insert(0) += 1;
            }
            let row = by_label.entry(reading.label.clone()).or_insert((0, 0, 0));
            row.0 += 1;
            row.1 += usize::from(reading.answered.is_some());
            row.2 += usize::from(reading.agreed);
        }
        Self {
            name: name.to_string(),
            model: model.to_string(),
            items: readings.len(),
            answered,
            agreed,
            agreement_share: agreed as f64 / items as f64,
            agreement_lower_bound: summary::wilson_lower(agreed, readings.len(), summary::WILSON_Z_95),
            p50_ms: summary::percentile(&latencies, 0.5),
            p95_ms: summary::percentile(&latencies, 0.95),
            mean_input_tokens: input as f64 / items as f64,
            mean_output_tokens: output as f64 / items as f64,
            cost_usd,
            cost_usd_per_item: cost_usd / items as f64,
            unanswered,
            by_label,
            readings,
        }
    }

    fn line(&self) -> String {
        let unanswered: Vec<String> =
            self.unanswered.iter().map(|(why, count)| format!("{why} {count}")).collect();
        format!(
            "| {} ({}) | {} | {} | {} | {:.1}% | {:.1}% | {} | {} | {:.0} | {:.1} | ${:.4} | ${:.6} | {} |",
            self.name,
            self.model,
            self.items,
            self.answered,
            self.agreed,
            self.agreement_share * 100.0,
            self.agreement_lower_bound * 100.0,
            self.p50_ms.map_or_else(|| "—".to_string(), |ms| ms.to_string()),
            self.p95_ms.map_or_else(|| "—".to_string(), |ms| ms.to_string()),
            self.mean_input_tokens,
            self.mean_output_tokens,
            self.cost_usd,
            self.cost_usd_per_item,
            if unanswered.is_empty() { "—".to_string() } else { unanswered.join(" · ") }
        )
    }
}

/// The wall one frontier item may hold the measurement. The provider client
/// re-sends a 429 up to its own five retries with backoff (attempt 6/6 at
/// most), which is inside the coordinator's line of six; past this wall the
/// item is counted unanswered rather than waited for.
const FRONTIER_ITEM_WALL: Duration = Duration::from_secs(90);

/// Why a frontier call answered nothing, in one closed word.
fn frontier_failure(error: &api::ApiError) -> String {
    let said = error.to_string();
    if said.contains("429") || said.contains("rate_limit") {
        "rate_limited".to_string()
    } else {
        "error".to_string()
    }
}

/// `limit` items spread evenly over the whole seed — every k-th — so a cut
/// sample carries every label in the golden's own proportions rather than
/// the first pages by name.
fn spread<T: Clone>(all: &[T], limit: usize) -> Vec<T> {
    if limit >= all.len() {
        return all.to_vec();
    }
    (0..limit).map(|at| all[at * all.len() / limit].clone()).collect()
}

/// The frontier arm's one-shot prompt: the same question, the same
/// options with the same meanings, the same context — and an answer of
/// exactly one option id, so the two arms are read by the same rule.
fn frontier_prompt(question: &str, options: &[(String, String)], context: &str) -> String {
    let listed: Vec<String> = options.iter().map(|(id, text)| format!("- {id}: {text}")).collect();
    format!(
        "{question}\n\nOptions (answer with exactly one id, nothing else):\n{}\n\nContext:\n{context}",
        listed.join("\n")
    )
}

/// The option id a frontier reply names: the whole reply as an id, else the
/// first id the reply contains, else none.
fn frontier_choice(reply: &str, options: &[(String, String)]) -> Option<String> {
    let said = reply.trim().trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-').to_ascii_lowercase();
    options
        .iter()
        .find(|(id, _)| id.eq_ignore_ascii_case(&said))
        .or_else(|| options.iter().find(|(id, _)| said.contains(&id.to_ascii_lowercase())))
        .map(|(id, _)| id.clone())
}

/// The measurement t-6040 asks for: Jev against this machine's main-turn
/// model on a golden of ≥200 classification items, on tokens, latency and
/// agreement with the golden label. Reads `tools/agent-tool-replay/seed.py`'s
/// seed (`ZO_AGENT_TOOL_REPLAY_SEED`), asks the REAL wire through the seat's
/// own door with the key in `TYPESAFE_API_KEY`, and asks the frontier model
/// named by `ZO_AGENT_TOOL_REPLAY_MODEL` one item at a time on zo's own
/// provider road. Prints a Markdown table; writes it as JSON to
/// `ZO_AGENT_TOOL_REPLAY_OUT` when set (`tools/agent-tool-replay/README.md`).
#[test]
#[ignore = "asks the real wire and a real frontier model; see tools/agent-tool-replay/README.md"]
#[allow(clippy::too_many_lines)] // one measurement, read top to bottom: the two arms and the table
fn jev_against_the_frontier_on_the_golden() {
    let seed_path = std::env::var("ZO_AGENT_TOOL_REPLAY_SEED").expect("ZO_AGENT_TOOL_REPLAY_SEED names the seed");
    // The frontier arm is optional: unset, the run measures Jev alone and
    // the report says the comparison is deferred — what a day of 429s from
    // an account five workers share leaves a measurement able to do.
    let frontier_model = std::env::var("ZO_AGENT_TOOL_REPLAY_MODEL").ok().filter(|model| !model.trim().is_empty());
    let limit: usize = std::env::var("ZO_AGENT_TOOL_REPLAY_LIMIT").ok().and_then(|n| n.parse().ok()).unwrap_or(usize::MAX);
    let seed: Value = serde_json::from_str(&std::fs::read_to_string(&seed_path).expect("the seed reads")).expect("the seed is JSON");
    let question = seed["question"].as_str().expect("a question").to_string();
    let options: Vec<(String, String)> = seed["options"]
        .as_array()
        .expect("options")
        .iter()
        .map(|option| (option["id"].as_str().expect("id").to_string(), option["text"].as_str().expect("text").to_string()))
        .collect();
    let option_texts: Vec<String> = options.iter().map(|(_, text)| text.clone()).collect();
    let every: Vec<(String, String, String)> = seed["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|item| {
            (
                item["file"].as_str().expect("file").to_string(),
                item["label"].as_str().expect("label").to_string(),
                item["context"].as_str().expect("context").to_string(),
            )
        })
        .collect();
    assert!(every.len() >= 200, "a golden of {} items is under the 200 the brief asks", every.len());
    let items = spread(&every, limit);
    eprintln!("golden: {} items, {} measured (spread evenly), {} options", every.len(), items.len(), options.len());

    // Arm one: Jev, through the seat as the CLI would ask it.
    let jev_readings: Vec<Reading> = machine_live(&AGENT_TOOL, JevMode::On.key(), |cwd| {
        items
            .iter()
            .map(|(file, label, context)| {
                let asked = JevQuestion::new(JevShape::Choose, &question, Some(context), &option_texts, &[], &[])
                    .expect("a golden item is a well-formed choose");
                let verdict = decide(cwd, JevCaller::Cli, &asked);
                let answered = match &verdict.answer {
                    Some(JevAnswer::Choose { index, .. }) => options.get(*index).map(|(id, _)| id.clone()),
                    _ => None,
                };
                eprintln!(
                    "jev {}/{}: {} ({} ms)",
                    file,
                    label,
                    answered.as_deref().unwrap_or(&verdict.outcome),
                    verdict.elapsed_ms
                );
                Reading {
                    file: file.clone(),
                    label: label.clone(),
                    agreed: answered.as_deref() == Some(label.as_str()),
                    unanswered: answered.is_none().then(|| verdict.outcome.clone()),
                    answered,
                    elapsed_ms: verdict.elapsed_ms,
                    input_tokens: verdict.input_tokens.unwrap_or(0),
                    output_tokens: 0,
                }
            })
            .collect()
    });
    let jev_model = jev_readings.iter().map(|_| SYSTEMONE_MODEL.to_string()).next().unwrap_or_default();
    let jev = Arm::of("jev", &jev_model, jev_readings, |input, _| {
        api::systemone_rate(SYSTEMONE_MODEL).map_or(0.0, |rate| rate.input_cost_usd(input))
    });

    // Arm two: the frontier model, one item per request, on zo's own road.
    let frontier = frontier_model.as_deref().map(|frontier_model| {
    let client = std::sync::Arc::new(
        crate::misc_tools::agent_tools::build_provider_client_for_agent(frontier_model)
            .unwrap_or_else(|why| panic!("no provider client for {frontier_model}: {why}")),
    );
    let frontier_readings: Vec<Reading> = items
        .iter()
        .map(|(file, label, context)| {
            let request = api::MessageRequest {
                model: frontier_model.to_string(),
                max_tokens: 16,
                messages: vec![api::InputMessage::user_text(frontier_prompt(&question, &options, context))],
                system: Some(api::system_from_string("You classify. Reply with exactly one option id from the list and nothing else.")),
                tools: None,
                tool_choice: None,
                stream: false,
                thinking: None,
                output_config: None,
                effort: Some(api::EffortLevel::Low),
                effort_band_ceiling: None,
            };
            let started = std::time::Instant::now();
            let response = api::sync_bridge::run_blocking(async {
                tokio::time::timeout(FRONTIER_ITEM_WALL, client.send_message(&request))
                    .await
                    .map_err(|_| "timeout".to_string())
                    .and_then(|sent| sent.map_err(|error| frontier_failure(&error)))
            });
            let elapsed_ms = jev_gate::millis(started.elapsed());
            let (answered, unanswered, input_tokens, output_tokens) = match response {
                Ok(response) => {
                    let text: String = response
                        .content
                        .iter()
                        .filter_map(|block| match block {
                            api::OutputContentBlock::Text { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect();
                    let chosen = frontier_choice(&text, &options);
                    (
                        chosen.clone(),
                        chosen.is_none().then(|| "unreadable".to_string()),
                        u64::from(response.usage.input_tokens)
                            + u64::from(response.usage.cache_read_input_tokens)
                            + u64::from(response.usage.cache_creation_input_tokens),
                        u64::from(response.usage.output_tokens),
                    )
                }
                Err(why) => (None, Some(why), 0, 0),
            };
            eprintln!(
                "frontier {}/{}: {} ({} ms)",
                file,
                label,
                answered.as_deref().unwrap_or_else(|| unanswered.as_deref().unwrap_or("?")),
                elapsed_ms
            );
            Reading {
                file: file.clone(),
                label: label.clone(),
                agreed: answered.as_deref() == Some(label.as_str()),
                unanswered,
                answered,
                elapsed_ms,
                input_tokens,
                output_tokens,
            }
        })
        .collect();
    Arm::of("frontier", frontier_model, frontier_readings, |input, output| {
        #[allow(clippy::cast_precision_loss)]
        api::model_price(frontier_model).map_or(0.0, |price| {
            (input as f64 * price.input + output as f64 * price.output) / 1_000_000.0
        })
    })
    });

    eprintln!("| arm (model) | items | answered | agreed | share | Wilson 95% lower | p50 ms | p95 ms | mean in tokens | mean out tokens | cost | cost/item | unanswered |");
    eprintln!("| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |");
    eprintln!("{}", jev.line());
    match &frontier {
        Some(frontier) => eprintln!("{}", frontier.line()),
        None => eprintln!("| frontier | deferred: set ZO_AGENT_TOOL_REPLAY_MODEL and rerun on the same seed | | | | | | | | | | | |"),
    }
    for (label, (items, answered, agreed)) in &jev.by_label {
        eprintln!("jev by label: {label}: {agreed}/{answered} agreed of {items}");
    }
    if let Ok(out) = std::env::var("ZO_AGENT_TOOL_REPLAY_OUT") {
        let arms: Vec<&Arm> = std::iter::once(&jev).chain(frontier.as_ref()).collect();
        let report = json!({
            "seed": seed_path,
            "question": question,
            "options": options,
            "measured": items.len(),
            "frontier": frontier.as_ref().map_or_else(|| json!("deferred"), |_| json!("measured")),
            "arms": arms,
        });
        std::fs::write(&out, serde_json::to_string_pretty(&report).expect("a report")).expect("the report writes");
        eprintln!("written: {out}");
    }
}
