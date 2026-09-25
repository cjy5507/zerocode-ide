//! The label audit (t-6342, `tools/label-audit/README.md`): every Jev seat's
//! ledger on this machine, graded again by the rules t-6342 shipped, beside
//! the marks the ledgers already hold — with the seat's cheapest baseline and
//! its confidence bands on the same marks, and what the new rules would have
//! asked on the same events.
//!
//! Nothing is asked: every number re-reads rows the ledgers hold. The grading
//! is the shipped functions' (`stall_cause::mark`, `worker_placement::mark`,
//! `notify_call::agreed`, `rerank_shadow::mark`, `step_effort::move_mark`,
//! `orchestration::runs_model`, …), each handed a row's own facts; this file
//! only joins rows and counts.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;

use serde_json::{json, Value};
use zerocode_core::jev::summary::{
    self, wilson_lower, ConfidenceTally, AGREED, AT, BASELINE_AGREED, LABEL, NOT_COMPARED, REQUESTS, WILSON_Z_95,
};
use zerocode_core::jev::{
    Band, JevUse, AGENT_TOOL, CLAIM, COMMAND_GUARD, COMPACTION, FILE_PICK, JEV_USES, MENTION_RERANK, NOTIFY, PATCH_REVIEW,
    PLACEMENT, PLACEMENT_SEEN_DWELL_MS, RECALL, ROUTING, SKILLS, SKILL_SUGGESTION, STALL, SUMMON, TOOL_TEXT_GUARD, VAULT_PAIRS,
    ZO_STEP_EFFORT,
};
use zerocode_core::{notify_call, orchestration, stall_cause, step_effort, worker_placement};

use crate::misc_tools::smart_router::rerank_shadow;
use crate::misc_tools::smart_router::step_effort as step_rows;

/// The seed `tools/label-audit/seed.py` wrote.
const SEED_ENV: &str = "ZEROCODE_LABEL_AUDIT_SEED";

/// Where the audit's numbers go as JSON, beside the table it prints.
const OUT_ENV: &str = "ZEROCODE_LABEL_AUDIT_OUT";

/// The seats zo's tools file under a project's state (`shadow_ledger_path`,
/// each writer's `*_FILE` is its seat's `ledger`); every other seat's ledger
/// is the window's, under the zo home's `jev/`. A name alone is not a seat:
/// zo's step governor wrote the window effort seat's file name before it had
/// a ledger of its own.
const ZO_WRITES: [&JevUse; 14] = [
    &ROUTING, &RECALL, &SKILLS, &SKILL_SUGGESTION, &ZO_STEP_EFFORT, &COMPACTION, &AGENT_TOOL, &MENTION_RERANK, &PATCH_REVIEW,
    &CLAIM, &VAULT_PAIRS, &FILE_PICK, &COMMAND_GUARD, &TOOL_TEXT_GUARD,
];

/// Marks counted one way.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Marks {
    compared: usize,
    agreed: usize,
}

impl Marks {
    fn add(&mut self, agreed: bool) {
        self.compared += 1;
        self.agreed += usize::from(agreed);
    }

    fn lower(self) -> f64 {
        wilson_lower(self.agreed, self.compared, WILSON_Z_95)
    }

    #[allow(clippy::cast_precision_loss)]
    fn share(self) -> Option<f64> {
        (self.compared > 0).then(|| self.agreed as f64 / self.compared as f64)
    }

    fn disagreed(self) -> usize {
        self.compared - self.agreed
    }

    fn json(self) -> Value {
        json!({
            "compared": self.compared,
            "agreed": self.agreed,
            "share": self.share(),
            "lowerBound": (self.compared > 0).then(|| self.lower()),
        })
    }
}

/// One seat's audit.
#[derive(Debug, Default)]
struct Audit {
    rows: usize,
    /// What the ledger says was sent.
    requests: u64,
    /// What the new rules would have sent on the same events.
    requests_after: u64,
    /// The marks as the ledger holds them.
    before: Marks,
    /// The same events, graded again.
    after: Marks,
    /// Why the rest compare nothing, by word.
    not_compared: BTreeMap<String, usize>,
    /// The seat's baseline on the facts the new marks were read off.
    baseline: Marks,
    /// Each new mark beside its answer's confidence, where the ledger holds it.
    graded: Vec<(f64, bool)>,
    notes: Vec<String>,
}

impl Audit {
    fn of(rows: &[Value]) -> Self {
        let mut audit = Self { rows: rows.len(), ..Self::default() };
        for row in rows {
            audit.requests += requests(row);
            if let Some(agreed) = agreed(row) {
                audit.before.add(agreed);
            }
        }
        audit.requests_after = audit.requests;
        audit
    }

    fn not_compared(&mut self, why: &str) {
        *self.not_compared.entry(why.to_string()).or_default() += 1;
    }

    fn note(&mut self, line: String) {
        self.notes.push(line);
    }

    fn json(&self, seat: &JevUse) -> Value {
        let lift = match (self.after.compared, self.baseline.share()) {
            (0, _) | (_, None) => None,
            (_, Some(share)) => Some(self.after.lower() - share),
        };
        let bands = summary::band_tally(seat, self.graded.iter().copied()).map(|tally| {
            Band::ALL.iter().zip(tally).map(|(band, tally)| json!({band.word(): tally_json(tally)})).collect::<Vec<_>>()
        });
        json!({
            "seat": seat.id,
            "rows": self.rows,
            "requests": self.requests,
            "requestsAfter": self.requests_after,
            "before": self.before.json(),
            "after": self.after.json(),
            "notCompared": self.not_compared,
            "baseline": {"kind": seat.baseline.kind(), "marks": self.baseline.json()},
            "liftOverBaseline": lift,
            "negativesOnRecord": self.after.disagreed(),
            "negativesWanted": seat.negatives_wanted,
            "curve": summary::confidence_curve(self.graded.iter().copied()).map(tally_json),
            "bands": bands,
            "graded": self.graded.len(),
            "notes": self.notes,
        })
    }
}

fn tally_json(tally: ConfidenceTally) -> Value {
    json!({"marks": tally.marks, "agreed": tally.agreed})
}

fn str_of<'a>(row: &'a Value, key: &str) -> Option<&'a str> {
    row.get(key).and_then(Value::as_str)
}

fn requests(row: &Value) -> u64 {
    REQUESTS.read(row).and_then(Value::as_u64).unwrap_or(0)
}

fn agreed(row: &Value) -> Option<bool> {
    AGREED.read(row).and_then(Value::as_bool)
}

fn confidence(row: &Value) -> Option<f64> {
    row.get("confidence").and_then(Value::as_f64)
}

fn label_of(row: &Value) -> Option<&str> {
    LABEL.read(row).and_then(Value::as_str)
}

/// A seat none of t-6342's rules re-grade: its marks as written, its
/// baseline marks where its writer already stamps them, and the confidence
/// its own row carries beside a mark.
fn as_written(rows: &[Value]) -> Audit {
    let mut audit = Audit::of(rows);
    for row in rows {
        if let Some(mark) = agreed(row) {
            audit.after.add(mark);
            if let Some(confidence) = confidence(row) {
                audit.graded.push((confidence, mark));
            }
        } else if let Some(why) = NOT_COMPARED.read(row).and_then(Value::as_str) {
            audit.not_compared(why);
        }
        if let Some(baseline) = BASELINE_AGREED.read(row).and_then(Value::as_bool) {
            audit.baseline.add(baseline);
        }
    }
    audit
}

/// The stall seat: each label's follow-up against the cause its question
/// answered, by the one question the rule asks now.
fn stall(rows: &[Value]) -> Audit {
    let mut audit = Audit::of(rows);
    let asked: HashMap<&str, &Value> =
        rows.iter().filter(|row| label_of(row).is_none()).filter_map(|row| Some((str_of(row, "stall")?, row))).collect();
    for label in rows.iter().filter(|row| label_of(row).is_some()) {
        let followed = str_of(label, "followed")
            .and_then(|word| stall_cause::Followed::ALL.into_iter().find(|followed| followed.word() == word));
        let ask = label_of(label).and_then(|key| asked.get(key));
        let cause = ask.and_then(|ask| str_of(ask, "cause")).and_then(stall_cause::Cause::from_word);
        let (Some(followed), Some(cause)) = (followed, cause) else {
            audit.not_compared("no_answer");
            continue;
        };
        match stall_cause::mark(cause, followed) {
            Ok(mark) => {
                audit.after.add(mark);
                if let Some(confidence) = ask.and_then(|ask| confidence(ask)) {
                    audit.graded.push((confidence, mark));
                }
                if let Some(baseline) = stall_cause::baseline_mark(followed) {
                    audit.baseline.add(baseline);
                }
            }
            Err(why) => audit.not_compared(why),
        }
    }
    audit
}

/// The terminal a worker's pane stood in at `at`, as the black box named it
/// last before then.
fn terminal_of(worker_terms: &HashMap<String, Vec<(i64, u64)>>, worker: &str, at: i64) -> Option<u64> {
    worker_terms.get(worker)?.iter().rev().find(|(named, _)| *named <= at).map(|(_, terminal)| *terminal)
}

/// Whether `terminal` stood on the main stage for the dwell without a break
/// between `from` and `to`, as the stage declarations say. The black box
/// holds no focus, so this is the stage half of the window's rule — a pane
/// on the stage of a window in the background reads as seen here.
fn stood_on_stage(watch: &[(i64, Vec<u64>)], terminal: u64, from: i64, to: i64) -> bool {
    let mut since: Option<i64> = None;
    for (index, (at, terms)) in watch.iter().enumerate() {
        let until = watch.get(index + 1).map_or(to, |(next, _)| *next).min(to);
        if until <= from {
            continue;
        }
        if *at >= to {
            break;
        }
        if terms.contains(&terminal) {
            let start = *since.get_or_insert((*at).max(from));
            if until - start >= PLACEMENT_SEEN_DWELL_MS {
                return true;
            }
        } else {
            since = None;
        }
    }
    false
}

/// The placement seat: each label graded against the room the pane stood in
/// (`stood_in`, or the room it was moved to), and only where somebody could
/// have seen it — the row's own `seen` when it has one, a move, or the
/// black box's stage declarations.
fn placement(rows: &[Value], watch: &[(i64, Vec<u64>)], worker_terms: &HashMap<String, Vec<(i64, u64)>>) -> Audit {
    let mut audit = Audit::of(rows);
    let asked: HashMap<&str, &Value> =
        rows.iter().filter(|row| label_of(row).is_none()).filter_map(|row| Some((str_of(row, "placement")?, row))).collect();
    let (mut from_row, mut from_stage, mut unknown) = (0, 0, 0);
    for label in rows.iter().filter(|row| label_of(row).is_some()) {
        let Some(chosen) = str_of(label, "chosen").and_then(worker_placement::Placement::of) else {
            audit.not_compared("no_answer");
            continue;
        };
        let applied = label.get("applied").and_then(Value::as_bool).unwrap_or(false);
        let moved = label.get("moved").and_then(Value::as_bool).unwrap_or(false);
        let ended_in = if moved {
            str_of(label, "followed").and_then(worker_placement::Placement::of).unwrap_or(chosen)
        } else {
            worker_placement::stood_in(chosen, applied)
        };
        let worker = str_of(label, "worker").unwrap_or_default();
        let ask = label_of(label).and_then(|key| asked.get(key));
        let asked_at = ask.and_then(|ask| AT.read(ask)).and_then(Value::as_i64);
        let labeled_at = AT.read(label).and_then(Value::as_i64).unwrap_or(0);
        let seen = if let Some(seen) = label.get("seen").and_then(Value::as_bool) {
            from_row += 1;
            Some(seen)
        } else if moved {
            from_row += 1;
            Some(true)
        } else {
            match (asked_at, terminal_of(worker_terms, worker, labeled_at)) {
                (Some(from), Some(terminal)) => {
                    from_stage += 1;
                    Some(stood_on_stage(watch, terminal, from, labeled_at))
                }
                _ => None,
            }
        };
        let Some(seen) = seen else {
            unknown += 1;
            audit.not_compared("sight_unknown");
            continue;
        };
        match worker_placement::mark(chosen, ended_in, moved, seen) {
            Ok(mark) => {
                audit.after.add(mark);
                if let Some(confidence) = ask.and_then(|ask| confidence(ask)) {
                    audit.graded.push((confidence, mark));
                }
                if let Some(baseline) = worker_placement::baseline_mark(chosen, ended_in, moved, seen) {
                    audit.baseline.add(baseline);
                }
            }
            Err(why) => audit.not_compared(why),
        }
    }
    audit.note(format!(
        "sight: {from_row} from the row or a move, {from_stage} from the black box's stage (no focus there), {unknown} unknown"
    ));
    // The room fix alone, as if every pane had been seen: what the label
    // says once it grades the room the pane stood in.
    let (mut room, mut room_baseline) = (Marks::default(), Marks::default());
    for label in rows.iter().filter(|row| label_of(row).is_some()) {
        let Some(chosen) = str_of(label, "chosen").and_then(worker_placement::Placement::of) else { continue };
        let applied = label.get("applied").and_then(Value::as_bool).unwrap_or(false);
        let moved = label.get("moved").and_then(Value::as_bool).unwrap_or(false);
        let ended_in = if moved {
            str_of(label, "followed").and_then(worker_placement::Placement::of).unwrap_or(chosen)
        } else {
            worker_placement::stood_in(chosen, applied)
        };
        if let Ok(mark) = worker_placement::mark(chosen, ended_in, moved, true) {
            room.add(mark);
        }
        if let Some(baseline) = worker_placement::baseline_mark(chosen, ended_in, moved, true) {
            room_baseline.add(baseline);
        }
    }
    audit.note(format!(
        "room fix alone (every pane counted as seen): {}/{} against today's room {}/{}",
        room.agreed, room.compared, room_baseline.agreed, room_baseline.compared
    ));
    // The room the old quiet label wrote even when the seat only recorded.
    let recorded_rooms = rows
        .iter()
        .filter(|row| label_of(row).is_some())
        .filter(|row| {
            row.get("applied").and_then(Value::as_bool) == Some(false)
                && row.get("moved").and_then(Value::as_bool) != Some(true)
                && str_of(row, "chosen").and_then(worker_placement::Placement::of)
                    != Some(worker_placement::Placement::TODAYS)
        })
        .count();
    audit.note(format!("{recorded_rooms} labels named a room the pane never stood in (recorded, not applied)"));
    audit
}

/// The summon seat: the options a pinned model leaves (`runs_model`), the
/// summonses the code decides alone, and the old answer wherever it survives
/// the narrower question.
fn summon(rows: &[Value]) -> Audit {
    let mut audit = Audit::of(rows);
    let (mut decided, mut decided_right, mut still_asked, mut re_asked) = (0, 0, 0, 0);
    let mut codex = BTreeMap::<&str, usize>::new();
    for row in rows {
        let Some(landed) = str_of(row, "agent") else { continue };
        let pinned = row.get("modelWasPinned").and_then(Value::as_bool).unwrap_or(false);
        let model = str_of(row, "workerModel").or_else(|| str_of(row, "model")).filter(|_| pinned);
        let options: Vec<&str> = row
            .get("options")
            .and_then(Value::as_array)
            .map(|options| options.iter().filter_map(|option| option.as_str().or_else(|| str_of(option, "id"))).collect())
            .unwrap_or_default();
        let left: Vec<&str> = match model {
            Some(model) => options.iter().copied().filter(|agent| orchestration::runs_model(agent, model)).collect(),
            None => options.clone(),
        };
        let on_openai = model.is_some_and(|model| orchestration::native_agent(model) == Some("codex"));
        if on_openai {
            *codex.entry("rows").or_default() += 1;
            *codex.entry("landedCodex").or_default() += usize::from(landed == "codex");
            *codex.entry("codexOffered").or_default() += usize::from(options.contains(&"codex"));
            *codex.entry("chosenCodexBefore").or_default() += usize::from(str_of(row, "chosen") == Some("codex"));
            *codex.entry("codexLeft").or_default() += usize::from(left.contains(&"codex"));
            *codex.entry("codeDecidesCodex").or_default() += usize::from(left == ["codex"]);
            // The old answer's odds over the agents the pinned model leaves
            // — what it would have leaned to among them, read without asking
            // (an estimate: the narrower question is a new question).
            if let Some(leaning) = leaning_among(row, &left) {
                *codex.entry("estimatedCodexAfter").or_default() += usize::from(leaning == "codex");
                *codex.entry("estimated").or_default() += 1;
            }
        }
        if left.len() <= 1 {
            decided += 1;
            decided_right += usize::from(left.first() == Some(&landed));
            audit.requests_after -= requests(row).min(audit.requests_after);
            audit.not_compared("decided_by_code");
            continue;
        }
        still_asked += 1;
        match str_of(row, "chosen") {
            Some(chosen) if left.contains(&chosen) => {
                let mark = chosen == landed;
                audit.after.add(mark);
                if let Some(confidence) = confidence(row) {
                    audit.graded.push((confidence, mark));
                }
                // The pinned model's own CLI on the same summons.
                if let Some(native) = model.and_then(orchestration::native_agent) {
                    audit.baseline.add(native == landed);
                }
            }
            Some(_) => {
                re_asked += 1;
                audit.not_compared("answer_left_the_options");
            }
            None => audit.not_compared("no_answer"),
        }
    }
    audit.note(format!(
        "{decided} summonses the code decides alone ({decided_right} of them the agent that landed); {still_asked} still asked, {re_asked} of whose old answers the narrower options no longer hold"
    ));
    audit.note(format!("OpenAI-model summonses: {codex:?}"));
    audit
}

/// The option the answer's own probabilities favour among `left` — `None`
/// for an answer that carried none, or none of `left`'s.
fn leaning_among<'a>(row: &Value, left: &[&'a str]) -> Option<&'a str> {
    let odds = row.get("probabilities")?.as_object()?;
    left.iter()
        .filter_map(|agent| Some((*agent, odds.get(*agent)?.as_f64()?)))
        .max_by(|one, other| one.1.total_cmp(&other.1))
        .map(|(agent, _)| agent)
}

/// The notify seat: its label is unchanged; today's rule is marked on the
/// same rings, and each mark meets its answer's confidence.
fn notify(rows: &[Value]) -> Audit {
    let mut audit = Audit::of(rows);
    let asked: HashMap<&str, &Value> =
        rows.iter().filter(|row| label_of(row).is_none()).filter_map(|row| Some((str_of(row, "notify")?, row))).collect();
    for label in rows.iter().filter(|row| label_of(row).is_some()) {
        let call = str_of(label, "call").and_then(notify_call::Call::from_word);
        let attendance = str_of(label, "attendance").and_then(notify_call::Attendance::from_word);
        let reacted = label.get("reacted").and_then(Value::as_bool);
        let (Some(call), Some(attendance), Some(reacted)) = (call, attendance, reacted) else {
            audit.not_compared("no_answer");
            continue;
        };
        let mark = match notify_call::agreed(call, reacted, attendance) {
            Ok(mark) => mark,
            Err(why) => {
                audit.not_compared(why);
                continue;
            }
        };
        audit.after.add(mark);
        if let Some(confidence) = label_of(label).and_then(|key| asked.get(key)).and_then(|ask| confidence(ask)) {
            audit.graded.push((confidence, mark));
        }
        if let Ok(baseline) = notify_call::agreed(notify_call::Call::today(), reacted, attendance) {
            audit.baseline.add(baseline);
        }
    }
    audit
}

/// The recall seat: a turn that touched no note compares nothing. The old
/// label rows hold the rank of the first note touched, so a row without one
/// touched none; recall's own first note was not written down before t-6342.
fn recall(rows: &[Value]) -> Audit {
    let mut audit = Audit::of(rows);
    for label in rows.iter().filter(|row| agreed(row).is_some() || NOT_COMPARED.read(row).is_some()) {
        let rank = label.get("rank").and_then(Value::as_u64).and_then(|rank| usize::try_from(rank).ok());
        let touched: Vec<usize> = rank.into_iter().collect();
        // The written mark where there is one: it read every note touched,
        // where the row keeps only the first one's rank.
        match rerank_shadow::mark(&touched) {
            Ok(fresh) => audit.after.add(agreed(label).unwrap_or(fresh)),
            Err(why) => audit.not_compared(why),
        }
        if let Some(baseline) = BASELINE_AGREED.read(label).and_then(Value::as_bool) {
            audit.baseline.add(baseline);
        }
    }
    audit
}

/// zo's step seat: a step is graded only where the answer moved what was
/// carried. A judgment asked about a step the wire held asks nothing now.
fn zo_step(rows: &[Value]) -> Audit {
    let mut audit = Audit::of(rows);
    let key = |row: &Value| -> Option<(String, u64, u64)> {
        Some((
            str_of(row, "attempt")?.to_string(),
            row.get("step").and_then(Value::as_u64)?,
            row.get("_project").and_then(Value::as_u64).unwrap_or(0),
        ))
    };
    let kind = |row: &Value| str_of(row, "kind").unwrap_or_default().to_string();
    let (judgment, label) = (step_rows::JUDGMENT_ROW_KIND, runtime::LABEL_ROW_KIND);
    let steps: HashMap<(String, u64, u64), &Value> =
        rows.iter().filter(|row| kind(row) == runtime::STEP_ROW_KIND).filter_map(|row| Some((key(row)?, row))).collect();
    let mut held_requests = 0;
    for row in rows {
        let Some(step) = key(row).and_then(|key| steps.get(&key)) else { continue };
        let held = step.get("held").is_some_and(|held| !held.is_null());
        if kind(row) == judgment && held {
            held_requests += requests(row);
        }
        let Some(progressed) = agreed(row).filter(|_| kind(row) == label) else { continue };
        let carried = step.get("applied").and_then(Value::as_bool).unwrap_or(false) && !held;
        let ruled = step.get("delta").and_then(Value::as_i64);
        let answered = step.get("jev").and_then(|jev| jev.get("delta")).and_then(Value::as_i64);
        let seat_moved_it = answered.is_some() && answered != ruled;
        match step_effort::move_mark(seat_moved_it, carried, progressed) {
            Ok(mark) => audit.after.add(mark),
            Err(why) => audit.not_compared(why),
        }
    }
    audit.requests_after -= held_requests.min(audit.requests_after);
    let held_steps = steps.values().filter(|step| step.get("held").is_some_and(|held| !held.is_null())).count();
    audit.note(format!("{held_steps} of {} steps held on their wire; {held_requests} requests were about a held step", steps.len()));
    audit
}

/// The patch review: its label is unchanged; "always permit" is marked on
/// the same hindsight. Under its recommendation (`off`) it asks nothing now.
fn patch_review(rows: &[Value]) -> Audit {
    let mut audit = as_written(rows);
    audit.baseline = Marks::default();
    audit.requests_after = 0;
    audit.note("requests after: under its recommendation, `off` (a word a person wrote still asks)".to_string());
    for label in rows.iter().filter(|row| agreed(row).is_some()) {
        let hindsight = str_of(label, "hindsight").unwrap_or_default();
        let stood = [runtime::patch_review::Hindsight::Stood { receipt: true }, runtime::patch_review::Hindsight::Stood {
            receipt: false,
        }]
        .iter()
        .any(|stood| stood.word() == hindsight);
        audit.baseline.add(stood);
    }
    audit
}

/// Every seat's audit, in the table's order.
fn audit_seed(seed: &Value) -> Vec<(&'static JevUse, Audit)> {
    let rows_in = |home: &str, ledger: &str| -> Vec<Value> {
        seed["ledgers"][home][ledger].as_array().cloned().unwrap_or_default()
    };
    let watch: Vec<(i64, Vec<u64>)> = seed["watch"]
        .as_array()
        .map(|declared| {
            declared
                .iter()
                .filter_map(|one| {
                    let at = one.get(0)?.as_i64()?;
                    let terms = one.get(1)?.as_array()?.iter().filter_map(Value::as_u64).collect();
                    Some((at, terms))
                })
                .collect()
        })
        .unwrap_or_default();
    let worker_terms: HashMap<String, Vec<(i64, u64)>> = seed["workerTerms"]
        .as_object()
        .map(|named| {
            named
                .iter()
                .map(|(worker, terminals)| {
                    let terminals = terminals
                        .as_array()
                        .map(|named_at| {
                            named_at
                                .iter()
                                .filter_map(|one| Some((one.get(0)?.as_i64()?, one.get(1)?.as_u64()?)))
                                .collect()
                        })
                        .unwrap_or_default();
                    (worker.clone(), terminals)
                })
                .collect()
        })
        .unwrap_or_default();
    JEV_USES
        .iter()
        .map(|seat| {
            let home = if ZO_WRITES.iter().any(|zo| zo.id == seat.id) { "projects" } else { "window" };
            let rows = rows_in(home, seat.ledger);
            let audit = match seat.id {
                id if id == STALL.id => stall(&rows),
                id if id == PLACEMENT.id => placement(&rows, &watch, &worker_terms),
                id if id == SUMMON.id => summon(&rows),
                id if id == NOTIFY.id => notify(&rows),
                id if id == RECALL.id => recall(&rows),
                id if id == ZO_STEP_EFFORT.id => zo_step(&rows),
                id if id == PATCH_REVIEW.id => patch_review(&rows),
                _ => as_written(&rows),
            };
            (seat, audit)
        })
        .collect()
}

fn percent(share: Option<f64>) -> String {
    share.map_or_else(|| "—".to_string(), |share| format!("{:.1}%", share * 100.0))
}

fn table(audits: &[(&'static JevUse, Audit)]) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "| seat | rows | before | after | after lower | not compared | baseline | lift | negatives | requests → after |"
    );
    let _ = writeln!(out, "|---|---:|---:|---:|---:|---|---:|---:|---:|---:|");
    for (seat, audit) in audits {
        let lift = match (audit.after.compared, audit.baseline.share()) {
            (0, _) | (_, None) => "—".to_string(),
            (_, Some(share)) => format!("{:+.1}pt", (audit.after.lower() - share) * 100.0),
        };
        let _ = writeln!(
            out,
            "| {} | {} | {}/{} ({}) | {}/{} ({}) | {} | {:?} | {} {}/{} ({}) | {} | {} | {} → {} |",
            seat.id,
            audit.rows,
            audit.before.agreed,
            audit.before.compared,
            percent(audit.before.share()),
            audit.after.agreed,
            audit.after.compared,
            percent(audit.after.share()),
            if audit.after.compared > 0 { format!("{:.1}%", audit.after.lower() * 100.0) } else { "—".into() },
            audit.not_compared,
            seat.baseline.kind(),
            audit.baseline.agreed,
            audit.baseline.compared,
            percent(audit.baseline.share()),
            lift,
            audit.after.disagreed(),
            audit.requests,
            audit.requests_after,
        );
    }
    for (seat, audit) in audits.iter().filter(|(_, audit)| !audit.notes.is_empty()) {
        for note in &audit.notes {
            let _ = writeln!(out, "- {}: {note}", seat.id);
        }
    }
    out
}

/// A seed of a handful of rows, graded the way the measurement grades the
/// machine's: the eleven stall marks the old table read wrong read right,
/// a turn that touched no note compares nothing, a summons a pinned model
/// leaves one agent for asks nothing, a step held on its wire is no mark,
/// and a recorded split whose pane sat in its tab untouched was never tried
/// (t-9427).
#[test]
fn the_audit_grades_old_rows_by_the_new_rules_and_counts_what_they_would_ask() {
    let seed = json!({
        "ledgers": {
            "window": {
                "stall-cause.jsonl": [
                    {"at": 1, "stall": "s-1", "cause": "long_running_tool", "confidence": 0.9, "outcome": "answered", "requests": 1},
                    {"at": 2, "label": "s-1", "followed": "worker_done", "agreed": false},
                ],
                "summon-choice.jsonl": [
                    {"at": 1, "agent": "codex", "model": "gpt", "modelWasPinned": true, "options": ["codex", "claude"], "chosen": "claude", "agreed": false, "requests": 1, "outcome": "answered"},
                    {"at": 2, "agent": "claude", "model": "opus", "modelWasPinned": true, "options": ["claude", "zo", "codex"], "chosen": "zo", "confidence": 0.4, "agreed": false, "requests": 1, "outcome": "answered"},
                ],
                "worker-placement.jsonl": [
                    {"at": 1, "placement": "w-1", "chosen": "split", "applied": false, "outcome": "answered", "requests": 1},
                    {"at": 2, "label": "w-1", "worker": "w-1", "chosen": "split", "applied": false, "followed": "tab", "moved": false, "seen": true, "agreed": false, "baselineAgreed": true},
                ],
            },
            "projects": {
                "rerank-shadow.jsonl": [
                    {"at": 1, "label": "1:2", "query": 1, "notes": 2, "applied": false, "agreed": false},
                ],
                "step-effort-zo.jsonl": [
                    {"kind": "step", "at": 1, "attempt": "a", "step": 1, "delta": 0, "applied": false, "held": "anthropic_cache_prefix", "jev": {"delta": -1}},
                    {"kind": "judgment", "at": 1, "attempt": "a", "step": 1, "requests": 1, "outcome": "answered"},
                    {"kind": "label", "at": 2, "attempt": "a", "step": 1, "agreed": true},
                ],
                // zo's governor's rows under the window effort seat's file
                // name: not the window's.
                "step-effort.jsonl": [{"kind": "step", "at": 1, "attempt": "b", "step": 1}],
            },
        },
        "watch": [],
        "workerTerms": {},
    });
    let audits = audit_seed(&seed);
    let of = |id: &str| audits.iter().find(|(seat, _)| seat.id == id).map(|(_, audit)| audit).expect("a seat");

    let stall = of(STALL.id);
    assert_eq!((stall.before, stall.after), (Marks { compared: 1, agreed: 0 }, Marks { compared: 1, agreed: 1 }));
    assert_eq!(stall.baseline, Marks { compared: 1, agreed: 1 }, "always long_running_tool");
    assert_eq!(stall.graded, vec![(0.9, true)]);

    let summon = of(SUMMON.id);
    assert_eq!(summon.not_compared.get("decided_by_code"), Some(&1), "an OpenAI model leaves codex alone of the two");
    assert_eq!((summon.requests, summon.requests_after), (2, 1));
    // Opus leaves claude and zo: the old answer stands, and missed where the
    // model's own CLI would have landed.
    assert_eq!(summon.after, Marks { compared: 1, agreed: 0 });
    assert_eq!(summon.baseline, Marks { compared: 1, agreed: 1 });

    let placement = of(PLACEMENT.id);
    assert_eq!((placement.before.compared, placement.after.compared), (1, 0));
    assert_eq!(placement.not_compared.get(worker_placement::NOT_CARRIED), Some(&1));
    assert_eq!(placement.baseline.compared, 0, "nor is today's tab graded on it");

    let recall = of(RECALL.id);
    assert_eq!(recall.after.compared, 0);
    assert_eq!(recall.not_compared.get(rerank_shadow::NO_NOTE_TOUCHED), Some(&1));

    let step = of(ZO_STEP_EFFORT.id);
    assert_eq!((step.before.compared, step.after.compared), (1, 0));
    assert_eq!(step.not_compared.get(step_effort::NOT_CARRIED), Some(&1));
    assert_eq!((step.requests, step.requests_after), (1, 0), "a held step asks nothing now");

    assert_eq!(of(zerocode_core::jev::STEP_EFFORT.id).rows, 0, "the window's effort seat wrote nothing");
    for (seat, audit) in &audits {
        assert!(audit.requests_after <= audit.requests, "{} asks more than before", seat.id);
    }
    assert!(table(&audits).contains("| stall | 2 | 0/1"));
}

/// The measurement: this machine's seed, every seat's table row, and the
/// numbers as JSON for the report.
#[test]
#[ignore = "reads the seed tools/label-audit/seed.py wrote; run by hand"]
fn the_labels_this_machine_holds_graded_again() {
    let path = std::env::var(SEED_ENV).expect("ZEROCODE_LABEL_AUDIT_SEED names the seed");
    let seed: Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("the seed")).expect("seed JSON");
    let audits = audit_seed(&seed);
    println!("{}", table(&audits));
    let total = |pick: fn(&Audit) -> u64| audits.iter().map(|(_, audit)| pick(audit)).sum::<u64>();
    println!("requests: {} → {}", total(|audit| audit.requests), total(|audit| audit.requests_after));
    for (seat, audit) in &audits {
        assert!(audit.requests_after <= audit.requests, "{} asks more than before", seat.id);
    }
    if let Ok(out) = std::env::var(OUT_ENV) {
        let numbers: Vec<Value> = audits.iter().map(|(seat, audit)| audit.json(seat)).collect();
        std::fs::write(out, serde_json::to_string_pretty(&numbers).expect("json")).expect("write");
    }
}
