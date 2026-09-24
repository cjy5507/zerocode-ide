//! The routing seat replayed on this machine's own turns (t-6346,
//! `tools/routing-replay/README.md`).
//!
//! `tools/routing-replay/seed.py` picks the transcripts and the ledgers and
//! reads nothing in them. Everything here is the shipped functions': a turn
//! is split where a person spoke (`runtime::patch_review::persons_turns`), its
//! words are read by the keyword tables (`decision_shadow::todays_rule`) and
//! by the chat probe's gate (`turn::probe_gate_admits`), what it did is read
//! by `route_label`, and both versions of the question are asked and read by
//! `runtime`'s own builders and readers, through the door (`JevDoor::at`, a
//! home of its own: the person's ledger and day count do not move).
//!
//! The label a turn is graded on is what it did — whether the router,
//! reading the version's complexity, would have picked the tier the level
//! its work reads as needed (`route_label::same_tier`) — and the recorded
//! chat probe answers the routing ledgers hold are the second reader, joined
//! by the task's fingerprint. Numbers are pooled shares with their Wilson
//! lower bounds, beside the keyword tables and a reader that always gives
//! one answer, on the same marks.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::time::Duration;

use api::{SystemOneClient, SystemOneFailure, SYSTEMONE_MODEL};
use runtime::{ContentBlock, ConversationMessage, MessageRole, RouteTaskComplexity};
use serde_json::{json, Value};
use zerocode_core::jev::door::JevSettings;
use zerocode_core::jev::summary::{wilson_lower, WILSON_Z_95};
use zerocode_core::jev::{Band, ROUTING, ROUTING_APPLY_DEADLINE_MS, ZO_STEP_EFFORT};

use super::jev_gate::{self, JevDoor};
use super::route_label::{observed_level, same_tier, turn_work};

/// The seed `tools/routing-replay/seed.py` wrote.
const SEED_ENV: &str = "ZEROCODE_ROUTING_REPLAY_SEED";
/// Where the numbers go as JSON, beside what is printed.
const OUT_ENV: &str = "ZEROCODE_ROUTING_REPLAY_OUT";
/// How many tasks to ask, spread evenly over what the seed holds.
const LIMIT_ENV: &str = "ZEROCODE_ROUTING_REPLAY_LIMIT";
const DEFAULT_LIMIT: usize = 400;
/// What the replay may spend on the wire, both versions together: at
/// `jev-1.13`'s input rate a second-version request is about a tenth of a
/// cent's hundredth, so the cap is a guard against a runaway, not a budget.
const REPLAY_SPEND_CAP_USD: f64 = 0.15;
/// Requests out at once — far under the wire's 1,200 a minute.
const IN_FLIGHT: usize = 6;
/// The wall each replayed request waits: the chat probe's own, so a slow
/// answer is measured rather than cut; the acting wall is read beside it.
const REPLAY_WALL: Duration = super::probe_exec::PROBE_TIMEOUT;
/// The workspace the replay's own door consents to.
const WORKSPACE: &str = "/work/routing-replay";

/// One task the seat would have been asked about.
struct Task {
    turn: bool,
    description: String,
    prompt: String,
    fingerprint: u64,
    rule: BTreeMap<String, String>,
    /// The chat probe's own gate admits it: a turn by its band, a spawn by
    /// this machine's classifier word (`probed`).
    probe_gate: bool,
    /// What the turn did, read as a level — a spawn's work is its own
    /// session's, not this transcript's.
    observed: Option<RouteTaskComplexity>,
    hangul_permille: Option<u16>,
    /// The chat probe's answer a routing ledger recorded for these words,
    /// and the first version's answer beside it, axis by axis.
    recorded: Option<Recorded>,
}

#[derive(Clone)]
struct Recorded {
    probe: BTreeMap<String, String>,
    first: Option<BTreeMap<String, String>>,
}

/// One version's answer to one task.
#[derive(Default, Clone)]
struct Answer {
    outcome: String,
    elapsed_ms: u64,
    input_tokens: u64,
    /// The router's words on the judged axes, and the confidence each came with.
    axes: BTreeMap<String, (String, f64)>,
    /// The second version's whole reading.
    reading: Option<runtime::RoutingReading>,
}

fn words_of(message: &ConversationMessage) -> Option<&str> {
    message.blocks.iter().find_map(|block| match block {
        ContentBlock::Text { text } => Some(text.as_str()),
        _ => None,
    })
}

/// The spawns a turn's calls started, as the words each was briefed with.
fn spawned(turn: &[ConversationMessage]) -> Vec<(String, String)> {
    let mut found = Vec::new();
    for block in turn.iter().flat_map(|message| &message.blocks) {
        let ContentBlock::ToolUse { name, input, .. } = block else {
            continue;
        };
        let Ok(input) = serde_json::from_str::<Value>(input) else {
            continue;
        };
        let brief = |agent: &Value| {
            (
                agent.get("description").and_then(Value::as_str).unwrap_or_default().to_string(),
                agent.get("prompt").and_then(Value::as_str).unwrap_or_default().to_string(),
            )
        };
        match name.as_str() {
            "Agent" | "Task" => found.push(brief(&input)),
            "SpawnMultiAgent" => {
                found.extend(input.get("agents").and_then(Value::as_array).into_iter().flatten().map(brief));
            }
            _ => {}
        }
    }
    found.retain(|(description, prompt)| !(description.trim().is_empty() && prompt.trim().is_empty()));
    found
}

fn task(turn: bool, description: String, prompt: String, observed: Option<RouteTaskComplexity>) -> Task {
    let whole = runtime::rubric_task_whole(&description, &prompt);
    Task {
        turn,
        fingerprint: super::probe_exec::task_fingerprint(&description, &prompt),
        rule: super::decision_shadow::todays_rule(&description, &prompt),
        probe_gate: !turn || super::turn::probe_gate_admits(&prompt),
        hangul_permille: zerocode_core::jev::hangul_share_permille(&whole),
        observed,
        recorded: None,
        description,
        prompt,
    }
}

/// Every task the seed's transcripts hold: each turn a person began, and
/// each spawn a turn started.
fn tasks_of(seed: &Value) -> (Vec<Task>, usize, usize) {
    let (mut tasks, mut read, mut skipped) = (Vec::new(), 0, 0);
    for row in seed["transcripts"].as_array().into_iter().flatten() {
        let Some(path) = row["path"].as_str() else { continue };
        let Ok(session) = runtime::Session::load_from_path(path) else {
            skipped += 1;
            continue;
        };
        let Some(history) = super::replay_support::history_as_it_stood(&session) else {
            skipped += 1;
            continue;
        };
        read += 1;
        for turn in runtime::patch_review::persons_turns(&history) {
            let Some(first) = turn.first().filter(|message| message.role == MessageRole::User) else {
                continue;
            };
            let Some(words) = words_of(first).filter(|words| !words.trim().is_empty()) else {
                continue;
            };
            if words.trim_start().starts_with(runtime::patch_review::HARNESS_TAG_OPEN) {
                continue;
            }
            let observed = observed_level(&turn_work(turn));
            tasks.push(task(true, String::new(), words.to_string(), Some(observed)));
            for (description, prompt) in spawned(turn) {
                tasks.push(task(false, description, prompt, None));
            }
        }
    }
    (tasks, read, skipped)
}

/// The chat probe's recorded answers, by task fingerprint: the first row a
/// routing ledger holds for the words with the probe's answer on it.
fn recorded_of(seed: &Value) -> HashMap<u64, Recorded> {
    let mut found = HashMap::new();
    for row in seed["ledgers"].as_array().into_iter().flatten() {
        let Some(path) = row["path"].as_str() else { continue };
        for row in super::jev_summary::read_rows(Path::new(path)) {
            let (Some(task), Some(probe)) = (row.get("task").and_then(Value::as_str), row.get("probe").and_then(Value::as_object)) else {
                continue;
            };
            let Ok(task) = u64::from_str_radix(task, 16) else { continue };
            let tokens = |object: &serde_json::Map<String, Value>, read: &dyn Fn(&Value) -> Option<String>| {
                runtime::judged_axes()
                    .filter_map(|axis| object.get(axis.name).and_then(read).map(|token| (axis.name.to_string(), token)))
                    .collect::<BTreeMap<_, _>>()
            };
            let probe = tokens(probe, &|value| value.as_str().map(str::to_string));
            let first = row.get("jev").and_then(Value::as_object).map(|jev| {
                tokens(jev, &|value| value.get("choice").and_then(Value::as_str).map(str::to_string))
            });
            found.entry(task).or_insert(Recorded { probe, first });
        }
    }
    found
}

/// Ask one task under one version through the replay's door.
async fn ask(door: &JevDoor, client: &SystemOneClient, task: &Task, second: bool) -> Answer {
    let whole = runtime::rubric_task_whole(&task.description, &task.prompt);
    let state = runtime::routing_state(&whole, runtime::RoutingFacts::default());
    // The first version sends the task as the plain string the step governor
    // still sends, so it is cleared under that row's pointer.
    let body = if second {
        jev_gate::body_of(&runtime::routing_request(SYSTEMONE_MODEL, &state))
    } else {
        jev_gate::body_of(&runtime::decision_request(SYSTEMONE_MODEL, &whole))
    };
    let row = if second { &ROUTING } else { &ZO_STEP_EFFORT };
    let Some(cleared) = body.and_then(|body| door.pass(row, true, body).ok()) else {
        return Answer { outcome: "refused".to_string(), ..Answer::default() };
    };
    let call = jev_gate::send(client, cleared, REPLAY_WALL, None).await;
    let mut answer = Answer { elapsed_ms: jev_gate::millis(call.elapsed), ..Answer::default() };
    match call.outcome {
        Err(failure) => answer.outcome = failure.ledger_token(),
        Ok(response) => {
            answer.input_tokens = response.usage.input_tokens;
            let verdict = if second {
                runtime::validate_routing(&response).ok().map(|reading| {
                    let verdict = reading.verdict();
                    answer.reading = Some(reading);
                    verdict
                })
            } else {
                runtime::validate_decision(&response).ok()
            };
            match verdict {
                Some(verdict) => {
                    answer.outcome = "answered".to_string();
                    answer.axes = verdict
                        .readings()
                        .iter()
                        .map(|reading| {
                            (reading.axis.name.to_string(), (reading.axis.tokens[reading.position].to_string(), reading.confidence))
                        })
                        .collect();
                }
                None => answer.outcome = SystemOneFailure::Schema.token().to_string(),
            }
        }
    }
    answer
}

/// A share and its Wilson lower bound, as a report prints them.
fn share(agreed: usize, compared: usize) -> Value {
    #[allow(clippy::cast_precision_loss)]
    let point = (compared > 0).then(|| agreed as f64 / compared as f64);
    json!({
        "agreed": agreed,
        "compared": compared,
        "share": point,
        "lowerBound": (compared > 0).then(|| wilson_lower(agreed, compared, WILSON_Z_95)),
    })
}

fn percentile(values: &mut [u64], at: f64) -> Option<u64> {
    values.sort_unstable();
    zerocode_core::jev::summary::percentile(values, at)
}

/// Marks on a label, one reader: how often the router would have picked the
/// tier the work needed from its level, and how often it named the level.
#[derive(Default, Clone, Copy)]
struct Marks {
    same_tier: usize,
    exact: usize,
    compared: usize,
}

impl Marks {
    fn add(&mut self, said: RouteTaskComplexity, was: RouteTaskComplexity) {
        if let Some(same) = same_tier(said, was) {
            self.compared += 1;
            self.same_tier += usize::from(same);
            self.exact += usize::from(said == was);
        }
    }

    fn json(self) -> Value {
        json!({ "sameTier": share(self.same_tier, self.compared), "exact": share(self.exact, self.compared) })
    }
}

fn level(token: Option<&String>) -> Option<RouteTaskComplexity> {
    token.and_then(|token| RouteTaskComplexity::from_label(token))
}

/// Agreement with the chat probe's recorded answer, axis by axis.
#[derive(Default, Clone, Copy)]
struct Beside {
    agreed: usize,
    compared: usize,
}

impl Beside {
    fn add(&mut self, said: Option<&String>, probe: Option<&String>) {
        if let (Some(said), Some(probe)) = (said, probe) {
            self.compared += 1;
            self.agreed += usize::from(said == probe);
        }
    }
}

/// The probe's own record on this machine, from the route-outcome ledgers:
/// each call's wall time and what became of it (`route-tax` rows).
fn probe_record(seed: &Value) -> (Vec<u64>, usize, usize) {
    let (mut durations, mut calls, mut failed) = (Vec::new(), 0, 0);
    for row in seed["outcomes"].as_array().into_iter().flatten() {
        let Some(path) = row["path"].as_str() else { continue };
        for row in super::jev_summary::read_rows(Path::new(path)) {
            if row.get("routeKey").and_then(Value::as_str) != Some("route-tax") {
                continue;
            }
            calls += 1;
            failed += usize::from(row.get("status").and_then(Value::as_str) != Some("completed"));
            if let Some(ms) = row.get("durationMs").and_then(Value::as_u64) {
                durations.push(ms);
            }
        }
    }
    (durations, calls, failed)
}

/// What the route-outcome ledgers can say about grades: how many rows name
/// the task they routed, and how the keyword tables' own grade on each
/// subagent row ended.
fn outcomes_by_grade(seed: &Value) -> Value {
    let (mut rows, mut keyed) = (0, 0);
    let mut by_grade: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    for row in seed["outcomes"].as_array().into_iter().flatten() {
        let Some(path) = row["path"].as_str() else { continue };
        for row in super::jev_summary::read_rows(Path::new(path)) {
            rows += 1;
            keyed += usize::from(row.get("runId").is_some());
            let (Some(grade), Some(status)) = (row.get("complexity").and_then(Value::as_str), row.get("status").and_then(Value::as_str)) else {
                continue;
            };
            *by_grade.entry(grade.to_string()).or_default().entry(status.to_string()).or_default() += 1;
        }
    }
    json!({ "rows": rows, "keyedToATurn": keyed, "byKeywordGrade": by_grade })
}

#[test]
#[ignore = "reads this machine's private transcripts and asks Jev"]
#[allow(clippy::too_many_lines)]
fn the_routing_seat_replayed_on_this_machines_turns() {
    let seed_path = std::env::var(SEED_ENV).expect("the seed's path");
    let seed: Value = serde_json::from_str(&std::fs::read_to_string(seed_path).expect("the seed reads")).expect("the seed parses");
    let (mut tasks, transcripts, skipped) = tasks_of(&seed);
    let recorded = recorded_of(&seed);
    for task in &mut tasks {
        task.recorded = recorded.get(&task.fingerprint).cloned();
    }
    let turns = tasks.iter().filter(|task| task.turn).count();
    let limit = std::env::var(LIMIT_ENV).ok().and_then(|raw| raw.parse().ok()).unwrap_or(DEFAULT_LIMIT);
    // Every task a recorded probe answer grades, then an even spread of the rest.
    let (graded, rest): (Vec<usize>, Vec<usize>) = (0..tasks.len()).partition(|at| tasks[*at].recorded.is_some());
    let stride = rest.len().div_ceil(limit.saturating_sub(graded.len()).max(1)).max(1);
    let sample: Vec<usize> = graded.iter().copied().chain(rest.iter().copied().step_by(stride)).take(limit.max(graded.len())).collect();

    let home = tempfile::tempdir().expect("a door home of its own");
    let settings = JevSettings {
        enabled: true,
        workspaces: vec![WORKSPACE.to_string()],
        daily_requests: None,
        model: zerocode_core::jev::DEFAULT_MODEL.to_string(),
    };
    let door = JevDoor::at(settings, Path::new(WORKSPACE), home.path());
    let client = api::SystemOneConfig::from_env().ok().map(api::SystemOneConfig::into_client).expect("TYPESAFE_API_KEY in this command's environment");
    let rate = api::systemone_rate(SYSTEMONE_MODEL).expect("the table prices the alias");

    let mut answers: HashMap<usize, (Answer, Answer)> = HashMap::new();
    let mut spent = 0.0;
    for chunk in sample.chunks(IN_FLIGHT) {
        if spent >= REPLAY_SPEND_CAP_USD {
            break;
        }
        let asked = api::sync_bridge::run_blocking(futures_util::future::join_all(chunk.iter().map(|at| {
            let (door, client, task) = (&door, &client, &tasks[*at]);
            async move { (*at, ask(door, client, task, false).await, ask(door, client, task, true).await) }
        })));
        for (at, first, second) in asked {
            spent += rate.input_cost_usd(first.input_tokens + second.input_tokens);
            answers.insert(at, (first, second));
        }
    }

    // What each version answered, and how fast.
    let mut first_ms: Vec<u64> = Vec::new();
    let mut second_ms: Vec<u64> = Vec::new();
    let (mut first_answered, mut second_answered) = (0, 0);
    let mut failures: BTreeMap<String, usize> = BTreeMap::new();
    let (mut first_tokens, mut second_tokens) = (0_u64, 0_u64);
    let (mut first_other, mut second_other, mut second_folded_other) = (0, 0, 0);
    let mut intents: BTreeMap<String, usize> = BTreeMap::new();
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    let mut facts: BTreeMap<String, usize> = BTreeMap::new();
    let mut bands: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut first_authority: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut within_wall = 0;
    for (first, second) in sample.iter().filter_map(|at| answers.get(at)) {
        for (version, answer) in [("first", first), ("second", second)] {
            if answer.outcome == "answered" {
                if version == "first" {
                    first_answered += 1;
                    first_ms.push(answer.elapsed_ms);
                } else {
                    second_answered += 1;
                    second_ms.push(answer.elapsed_ms);
                    within_wall += usize::from(answer.elapsed_ms <= ROUTING_APPLY_DEADLINE_MS);
                }
            } else {
                *failures.entry(format!("{version}:{}", answer.outcome)).or_default() += 1;
            }
        }
        first_tokens += first.input_tokens;
        second_tokens += second.input_tokens;
        if first.outcome == "answered" {
            first_other += usize::from(first.axes.get("intent").is_some_and(|(token, _)| token == "other"));
            let sure = first.axes.values().all(|(_, confidence)| *confidence >= 0.5);
            *first_authority.entry(if sure { "medium" } else { "low" }).or_default() += 1;
        }
        if let Some(reading) = second.reading.as_ref() {
            second_other += usize::from(reading.intent.chosen == "other");
            second_folded_other += usize::from(second.axes.get("intent").is_some_and(|(token, _)| token == "other"));
            *intents.entry(reading.intent.chosen.clone()).or_default() += 1;
            *kinds.entry(reading.reasoning.chosen.clone()).or_default() += 1;
            for id in reading.facts.keys() {
                *facts.entry(id.clone()).or_default() += usize::from(reading.holds(id) == Some(true));
            }
            *bands.entry(reading.band().word()).or_default() += 1;
        }
    }

    // Beside the chat probe's recorded answer: each version, the recorded
    // first-version answer, and the keyword tables, on the same axes.
    let (mut probe_first, mut probe_second, mut probe_recorded, mut probe_rule) =
        (Beside::default(), Beside::default(), Beside::default(), Beside::default());
    let mut probe_tasks = 0;
    // The work each turn did: each version's complexity, the tables', and a
    // reader that gives one level to every turn — the cheapest baseline.
    let (mut work_first, mut work_second, mut work_rule) = (Marks::default(), Marks::default(), Marks::default());
    let mut work_constant: BTreeMap<&'static str, Marks> = BTreeMap::new();
    let mut by_language: BTreeMap<&'static str, (Marks, Marks)> = BTreeMap::new();
    for (at, (first, second)) in sample.iter().filter_map(|at| answers.get(at).map(|answer| (at, answer))) {
        let task = &tasks[*at];
        if let Some(recorded) = task.recorded.as_ref() {
            probe_tasks += 1;
            for axis in runtime::judged_axes() {
                let probe = recorded.probe.get(axis.name);
                probe_first.add(first.axes.get(axis.name).map(|(token, _)| token), probe);
                probe_second.add(second.axes.get(axis.name).map(|(token, _)| token), probe);
                probe_recorded.add(recorded.first.as_ref().and_then(|first| first.get(axis.name)), probe);
                probe_rule.add(task.rule.get(axis.name), probe);
            }
        }
        let Some(was) = task.observed else { continue };
        let rule = level(task.rule.get("complexity"));
        let first_said = level(first.axes.get("complexity").map(|(token, _)| token));
        let second_said = level(second.axes.get("complexity").map(|(token, _)| token));
        if let (Some(first_said), Some(second_said), Some(rule)) = (first_said, second_said, rule) {
            work_first.add(first_said, was);
            work_second.add(second_said, was);
            work_rule.add(rule, was);
            for token in runtime::COMPLEXITY_AXIS.tokens {
                if let Some(said) = RouteTaskComplexity::from_label(token) {
                    work_constant.entry(token).or_default().add(said, was);
                }
            }
            let language = if task.hangul_permille.unwrap_or(0) >= 500 { "korean" } else { "other" };
            let entry = by_language.entry(language).or_default();
            entry.0.add(second_said, was);
            entry.1.add(rule, was);
        }
    }

    // The probe's calls and the turn's first request, per policy, over the
    // replayed turns: today recording (the probe routes every turn its gate
    // admits), the first version acting behind the probe's gate (the probe
    // only where the judgment failed), and the second version acting on
    // every turn (the probe only where it abstained or failed, on a turn the
    // gate admits). The probe's own wall times are the ones its route-tax
    // rows recorded on this machine, taken in turn.
    let (mut probe_durations, probe_calls_recorded, probe_failed_recorded) = probe_record(&seed);
    let probe_p50 = percentile(&mut probe_durations, 0.5);
    let probe_p95 = percentile(&mut probe_durations, 0.95);
    let mut probe_cycle = probe_durations.iter().copied().cycle();
    let (mut replayed_turns, mut today_probe, mut first_probe, mut second_probe) = (0, 0, 0, 0);
    let (mut first_asked, mut first_applied, mut first_moving, mut second_applied) = (0, 0, 0, 0);
    let (mut today_delay, mut first_delay, mut second_delay): (Vec<u64>, Vec<u64>, Vec<u64>) = (Vec::new(), Vec::new(), Vec::new());
    for (at, (first, second)) in sample.iter().filter_map(|at| answers.get(at).map(|answer| (at, answer))) {
        let task = &tasks[*at];
        if !task.turn {
            continue;
        }
        replayed_turns += 1;
        let probe_ms = probe_cycle.next().unwrap_or_default();
        let wall = |ms: u64| ms.min(ROUTING_APPLY_DEADLINE_MS);
        // Today: the probe on the path of every turn its gate admits.
        today_probe += usize::from(task.probe_gate);
        today_delay.push(if task.probe_gate { probe_ms } else { 0 });
        // The first version acting: asked only behind the gate; the probe
        // only where it failed.
        if task.probe_gate {
            first_asked += 1;
            let answered = first.outcome == "answered";
            first_applied += usize::from(answered);
            first_moving += usize::from(answered && first.axes.values().all(|(_, confidence)| *confidence >= 0.5));
            first_probe += usize::from(!answered);
            first_delay.push(wall(first.elapsed_ms) + if answered { 0 } else { probe_ms });
        } else {
            first_delay.push(0);
        }
        // The second version acting: asked on every turn; the probe where it
        // abstained or failed and the gate admits the turn.
        let acted = second.reading.as_ref().is_some_and(|reading| reading.band() != Band::Abstain);
        second_applied += usize::from(acted);
        let probed = !acted && task.probe_gate;
        second_probe += usize::from(probed);
        second_delay.push(wall(second.elapsed_ms) + if probed { probe_ms } else { 0 });
    }
    #[allow(clippy::cast_precision_loss)]
    let probe_failure_share = (probe_calls_recorded > 0).then(|| probe_failed_recorded as f64 / probe_calls_recorded as f64);
    let per_hundred = |count: usize| {
        #[allow(clippy::cast_precision_loss)]
        let rate = (replayed_turns > 0).then(|| count as f64 * 100.0 / replayed_turns as f64);
        rate
    };
    let delays = |values: &mut Vec<u64>| json!({ "p50Ms": percentile(values, 0.5), "p95Ms": percentile(values, 0.95) });

    // The curve the bands are read off (confidence.md: "plot confidence
    // against accuracy on your data"): each version's complexity confidence
    // against the work label, per fifth, and the numbers of every replayed
    // task — levels, confidences, spreads and facts, never a word.
    let mut first_curve: Vec<(f64, bool)> = Vec::new();
    let mut second_curve: Vec<(f64, bool)> = Vec::new();
    let mut rows: Vec<Value> = Vec::new();
    for (at, (first, second)) in sample.iter().filter_map(|at| answers.get(at).map(|answer| (at, answer))) {
        let task = &tasks[*at];
        let was = task.observed;
        let first_level = level(first.axes.get("complexity").map(|(token, _)| token));
        if let (Some(was), Some(said), Some((_, confidence))) = (was, first_level, first.axes.get("complexity")) {
            if let Some(same) = same_tier(said, was) {
                first_curve.push((*confidence, same));
            }
        }
        if let (Some(was), Some(reading)) = (was, second.reading.as_ref()) {
            if let Some(same) = level(second.axes.get("complexity").map(|(token, _)| token)).and_then(|said| same_tier(said, was)) {
                second_curve.push((reading.complexity.confidence, same));
            }
        }
        rows.push(json!({
            "turn": task.turn,
            "probeGate": task.probe_gate,
            "hangulPermille": task.hangul_permille,
            "observed": was.map(RouteTaskComplexity::as_label),
            "rule": task.rule,
            "probe": task.recorded.as_ref().map(|recorded| &recorded.probe),
            "firstRecorded": task.recorded.as_ref().and_then(|recorded| recorded.first.as_ref()),
            "first": {"outcome": first.outcome, "ms": first.elapsed_ms, "axes": first.axes},
            "second": {"outcome": second.outcome, "ms": second.elapsed_ms, "axes": second.axes, "reading": second.reading},
        }));
    }
    let curve = |graded: &[(f64, bool)]| {
        zerocode_core::jev::summary::confidence_curve(graded.iter().copied())
            .iter()
            .map(|fifth| share(fifth.agreed, fifth.marks))
            .collect::<Vec<_>>()
    };

    let metrics = json!({
        "transcripts": transcripts,
        "skipped": skipped,
        "tasks": tasks.len(),
        "turns": turns,
        "spawns": tasks.len() - turns,
        "recordedProbeAnswersJoined": tasks.iter().filter(|task| task.recorded.is_some()).count(),
        "sampled": sample.len(),
        "asked": answers.len(),
        "spentUsd": spent,
        "first": {
            "answered": first_answered, "p50Ms": percentile(&mut first_ms, 0.5), "p95Ms": percentile(&mut first_ms, 0.95),
            "inputTokens": first_tokens, "costUsd": rate.input_cost_usd(first_tokens),
            "intentOther": share(first_other, first_answered), "authority": first_authority,
        },
        "second": {
            "answered": second_answered, "p50Ms": percentile(&mut second_ms, 0.5), "p95Ms": percentile(&mut second_ms, 0.95),
            "withinApplyWall": share(within_wall, second_answered),
            "inputTokens": second_tokens, "costUsd": rate.input_cost_usd(second_tokens),
            "intentOther": share(second_other, second_answered), "foldedOther": share(second_folded_other, second_answered),
            "intents": intents, "kinds": kinds, "factsTrue": facts, "bands": bands,
        },
        "failures": failures,
        "besideTheProbe": {
            "tasks": probe_tasks,
            "firstAskedNow": share(probe_first.agreed, probe_first.compared),
            "firstAsRecorded": share(probe_recorded.agreed, probe_recorded.compared),
            "second": share(probe_second.agreed, probe_second.compared),
            "keywordTables": share(probe_rule.agreed, probe_rule.compared),
        },
        "againstTheWork": {
            "first": work_first.json(), "second": work_second.json(), "keywordTables": work_rule.json(),
            "always": work_constant.iter().map(|(token, marks)| (token.to_string(), marks.json())).collect::<BTreeMap<_, _>>(),
            "byLanguage": by_language.iter().map(|(language, (second, rule))| (language.to_string(), json!({"second": second.json(), "keywordTables": rule.json()}))).collect::<BTreeMap<_, _>>(),
        },
        "probe": {
            "recordedCalls": probe_calls_recorded, "recordedFailureShare": probe_failure_share,
            "recordedP50Ms": probe_p50, "recordedP95Ms": probe_p95,
            "callsPerHundredTurns": {
                "todayRecording": per_hundred(today_probe),
                "firstActing": per_hundred(first_probe),
                "secondActing": per_hundred(second_probe),
            },
        },
        "applied": {
            "turns": replayed_turns,
            "firstActingAsked": first_asked, "firstActingApplied": share(first_applied, replayed_turns),
            "firstActingCouldMoveARoute": share(first_moving, replayed_turns),
            "secondActingApplied": share(second_applied, replayed_turns),
        },
        "turnStartDelay": {
            "todayRecording": delays(&mut today_delay),
            "firstActing": delays(&mut first_delay),
            "secondActing": delays(&mut second_delay),
        },
        "routeOutcomes": outcomes_by_grade(&seed),
        "curveAgainstTheWork": { "first": curve(&first_curve), "second": curve(&second_curve) },
        "rows": rows,
    });
    if let Ok(path) = std::env::var(OUT_ENV) {
        std::fs::write(path, serde_json::to_vec_pretty(&metrics).expect("metrics JSON")).expect("metrics write");
    }
    let mut shown = metrics.clone();
    if let Some(object) = shown.as_object_mut() {
        object.remove("rows");
    }
    println!("{}", serde_json::to_string_pretty(&shown).expect("metrics JSON"));
}
