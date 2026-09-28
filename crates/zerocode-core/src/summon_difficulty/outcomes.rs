//! Outcomes of executed summonses. Unobserved work and unlinked usage stay null.
use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::jev::promote::{Agreement, Line};
use crate::orchestration::{Dispatch, MessageKind, Run, task_cost};

pub const KEY: &str = "executionOutcome";
pub const MIN_EXECUTIONS: usize = 30;
/// A measured cohort can have no failures; its label can still say no.
pub const MIN_NEGATIVES: usize = 0;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub first_attempt_success: Option<bool>,
    pub done_with_receipts: bool,
    pub landed: bool,
    pub rework_rounds: usize,
    pub retry_count: usize,
    pub wall_ms: Option<i64>,
    pub tokens: Option<i64>,
    pub tokens_reason: Option<task_cost::UsdReason>,
    /// Promotion includes retries and waits, not only the cheap first try.
    #[serde(default)]
    pub task_tokens: Option<i64>,
    #[serde(default)]
    pub task_wall_ms: Option<i64>,
}

/// Review facts come from the ledger's attempt/source-bound reader. A worker
/// claiming its own merge never supplies a positive outcome.
#[must_use]
pub fn observe(
    run: &Run,
    dispatch: &Dispatch,
    generation: &task_cost::GenerationCost,
    total: &task_cost::TaskCost,
) -> Option<Outcome> {
    let task = run.task(&dispatch.task)?;
    let attempts = task_cost::attempts(run, &task.id);
    let rework_rounds = attempts.len().saturating_sub(1);
    let retry_count = attempts.iter().filter(|one| one.retry_of.is_some()).count();
    let address = crate::orchestration::worker_address(&dispatch.worker);
    let report = run.messages().iter().rev().find(|m| {
        m.kind == MessageKind::WorkerDone
            && m.from == address
            && m.dispatch.as_deref() == Some(dispatch.id.as_str())
    });
    let done_with_receipts = report.is_some_and(|m| {
        let body: Value = serde_json::from_str(m.body.as_str()).unwrap_or_default();
        let payload: Value = serde_json::from_str(m.payload.as_str()).unwrap_or_default();
        body["ok"] == true
            && [body.get("head"), payload.get("reportPath")]
                .into_iter()
                .flatten()
                .any(|value| value.as_str().is_some_and(|s| !s.trim().is_empty()))
    });
    let review = run.review_of(task);
    let landed = review.verified && review.merged;
    let first_attempt_success =
        if rework_rounds > 0 || retry_count > 0 || dispatch.succeeded == Some(false) {
            Some(false)
        } else if landed && dispatch.ended_ms.is_some() {
            Some(done_with_receipts)
        } else {
            None
        };
    let tokens = known_tokens(generation);
    Some(Outcome {
        first_attempt_success,
        done_with_receipts,
        landed,
        rework_rounds,
        retry_count,
        wall_ms: dispatch
            .ended_ms
            .map(|end| end.saturating_sub(dispatch.started_ms)),
        tokens,
        tokens_reason: tokens.is_none().then_some(generation.usd_reason).flatten(),
        task_tokens: known_tokens(&total.generation),
        task_wall_ms: total.wall_ms,
    })
}

fn known_tokens(generation: &task_cost::GenerationCost) -> Option<i64> {
    let known = generation.sessions_linked > 0
        && generation.sessions_linked == generation.sessions_known
        && !matches!(
            generation.usd_reason,
            Some(
                task_cost::UsdReason::Unlinked
                    | task_cost::UsdReason::Unscanned
                    | task_cost::UsdReason::UnsupportedAgent
            )
        );
    known.then_some(
        generation.input_tokens
            + generation.output_tokens
            + generation.cache_read_tokens
            + generation.cache_write_tokens,
    )
}

/// Latest observation per request; a later retry revokes an earlier success.
/// The caller supplies only the current rubric/model's joined marks.
pub fn latest<'a>(rows: impl IntoIterator<Item = &'a Value>) -> Vec<&'a Value> {
    let mut latest = BTreeMap::new();
    for row in rows {
        if row.get(KEY).is_some() {
            latest.insert(
                (
                    row["run"].as_str(),
                    row["dispatch"].as_str(),
                    row["requestAt"].as_i64(),
                ),
                row,
            );
        }
    }
    latest.into_values().collect()
}

#[derive(Default)]
struct Cohort {
    n: usize,
    success: usize,
    costs: usize,
    pending: usize,
    tokens: i128,
    wall: i128,
}
impl Cohort {
    fn add(&mut self, outcome: &Outcome) {
        let Some(success) = outcome.first_attempt_success else {
            self.pending += 1;
            return;
        };
        self.n += 1;
        self.success += usize::from(success);
        if let (Some(tokens), Some(wall)) = (outcome.task_tokens, outcome.task_wall_ms) {
            self.costs += 1;
            self.tokens += i128::from(tokens);
            self.wall += i128::from(wall);
        }
    }
}

/// Same difficulty AND agent, distinct tasks. Both observed baselines can
/// fail. Shadow predictions contribute only the work that actually ran.
/// Requiring complete costs avoids making missing usage look like savings.
pub fn evidence(rows: &[&Value]) -> (Agreement, Option<Line>) {
    let mut groups: BTreeMap<(&str, &str), [Cohort; 3]> = BTreeMap::new();
    let mut tasks = HashSet::new();
    let mut agreement = Agreement::default();
    for row in rows {
        // Repeated attempts are reported, but do not multiply the sample size.
        if row["attempt"].as_u64() != Some(0) || row["retryOf"].as_bool() != Some(false) {
            continue;
        }
        let (Some(run), Some(task), Some(agent), Some(difficulty)) = (
            row["run"].as_str(),
            row["task"].as_str(),
            row["agent"].as_str(),
            row["chosen"].as_str(),
        ) else {
            continue;
        };
        if !tasks.insert((run, task)) {
            continue;
        }
        let Ok(outcome) = serde_json::from_value::<Outcome>(row[KEY].clone()) else {
            continue;
        };
        let group = groups.entry((agent, difficulty)).or_default();
        if row["applied"].as_bool() == Some(true) {
            group[0].add(&outcome);
        } else {
            group[1].add(&outcome);
            if row["baselineHigh"] == true {
                group[2].add(&outcome);
            }
        }
    }
    for group in groups.values() {
        agreement.compared += group[0].n;
        agreement.agreed += group[0].success;
        agreement.baseline_compared += group[1].n;
        agreement.baseline_agreed += group[1].success;
    }
    if agreement.compared < MIN_EXECUTIONS {
        return (
            agreement,
            Some(Line::TooFewCompared {
                compared: agreement.compared,
                wanted: MIN_EXECUTIONS,
            }),
        );
    }
    for group in groups.values().filter(|g| g[0].n + g[0].pending > 0) {
        let pending = group[0].pending + group[1].pending;
        if pending > 0 {
            return (agreement, Some(Line::Unlabeled { withheld: pending }));
        }
        let applied = &group[0];
        if applied.n < MIN_EXECUTIONS {
            return (
                agreement,
                Some(Line::TooFewCompared {
                    compared: applied.n,
                    wanted: MIN_EXECUTIONS,
                }),
            );
        }
        for baseline in &group[1..] {
            if baseline.n < MIN_EXECUTIONS {
                return (
                    agreement,
                    Some(Line::TooFewBaseline {
                        compared: baseline.n,
                        wanted: MIN_EXECUTIONS,
                    }),
                );
            }
            if applied.success * baseline.n < baseline.success * applied.n {
                return (agreement, Some(Line::ExecutionQuality));
            }
            if applied.costs != applied.n || baseline.costs != baseline.n {
                return (agreement, Some(Line::ExecutionCostsMissing));
            }
            if applied.tokens * baseline.n as i128 >= baseline.tokens * applied.n as i128
                || applied.wall * baseline.n as i128 >= baseline.wall * applied.n as i128
            {
                return (agreement, Some(Line::ExecutionSavings));
            }
        }
    }
    (agreement, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cohort(applied: bool, success: bool, tokens: Option<i64>, wall: i64) -> Vec<Value> {
        (0..MIN_EXECUTIONS).map(|i| json!({
            "run":"r", "task":format!("{applied}-{i}"), "dispatch":format!("d-{applied}-{i}"), "requestAt": i,
            "agent":"claude", "chosen":super::super::LADDER[0].0,
            "pinnedEffort":super::super::LADDER[2].2, "baselineHigh":true, "applied":applied, "attempt":0, "retryOf":false,
            KEY: Outcome { first_attempt_success:Some(success), done_with_receipts:true, landed:true,
                rework_rounds:0, retry_count:0, wall_ms:Some(wall), tokens, tokens_reason:None, task_tokens:tokens, task_wall_ms:Some(wall) }
        })).collect()
    }
    fn grade(rows: &[Value]) -> (Agreement, Option<Line>) {
        evidence(&latest(rows))
    }

    #[test]
    fn baselines_can_fail_and_only_executed_savings_with_preserved_success_can_rise() {
        let mut rows = cohort(false, false, Some(200), 200);
        rows.extend(cohort(true, true, Some(100), 100));
        assert_eq!(grade(&rows).1, None);
        assert_eq!(grade(&rows).0.baseline_agreed, 0);
        for row in rows.iter_mut().filter(|r| r["applied"] == true) {
            row[KEY]["taskTokens"] = json!(200);
        }
        assert_eq!(grade(&rows).1, Some(Line::ExecutionSavings));
    }
    #[test]
    fn cheap_failures_missing_usage_and_unmatched_difficulty_never_promote() {
        let mut rows = cohort(false, true, Some(200), 200);
        rows.extend(cohort(true, false, Some(100), 100));
        assert_eq!(grade(&rows).1, Some(Line::ExecutionQuality));
        for row in rows.iter_mut().filter(|r| r["applied"] == true) {
            row[KEY]["firstAttemptSuccess"] = json!(true);
            row[KEY]["taskTokens"] = Value::Null;
        }
        assert_eq!(grade(&rows).1, Some(Line::ExecutionCostsMissing));
        for row in rows.iter_mut().filter(|r| r["applied"] == false) {
            row["chosen"] = json!(super::super::LADDER[2].0);
        }
        assert!(matches!(
            grade(&rows).1,
            Some(Line::TooFewBaseline { compared: 0, .. })
        ));
    }
    #[test]
    fn retry_costs_cannot_hide_behind_a_cheap_first_attempt() {
        let mut rows = cohort(false, false, Some(200), 200);
        rows.extend(cohort(true, false, Some(100), 100));
        for row in rows.iter_mut().filter(|r| r["applied"] == true) {
            row[KEY]["taskTokens"] = json!(400);
            row[KEY]["taskWallMs"] = json!(400);
            row[KEY]["reworkRounds"] = json!(2);
        }
        assert_eq!(grade(&rows).1, Some(Line::ExecutionSavings));
    }
    #[test]
    fn a_later_retry_revokes_a_success_without_multiplying_samples() {
        let mut rows = cohort(false, true, Some(200), 200);
        rows.extend(cohort(true, true, Some(100), 100));
        let mut revision = rows.last().unwrap().clone();
        revision[KEY]["firstAttemptSuccess"] = json!(false);
        revision[KEY]["reworkRounds"] = json!(1);
        rows.push(revision.clone());
        rows.push(revision);
        let (agreement, line) = grade(&rows);
        assert_eq!(agreement.compared, MIN_EXECUTIONS);
        assert_eq!(agreement.agreed, MIN_EXECUTIONS - 1);
        assert_eq!(line, Some(Line::ExecutionQuality));
    }
    #[test]
    fn a_shadow_answer_does_not_become_a_success_for_the_choice_never_run() {
        let rows = cohort(false, true, Some(100), 100);
        let (agreement, line) = grade(&rows);
        assert_eq!(agreement.compared, 0);
        assert!(matches!(
            line,
            Some(Line::TooFewCompared { compared: 0, .. })
        ));
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::jev::{SUMMON_DIFFICULTY, promote};
    use serde_json::json;

    /// A window's worth of first attempts on each side, as the window writes
    /// them: each summons' request row, then the row its outcome was
    /// observed in. The carried-out answers' attempts succeed as
    /// `applied_success` says and cost 100; the coordinators' own pins at the
    /// same difficulty succeed as `pinned_success` says and cost 200.
    fn executions(applied_success: bool, pinned_success: bool) -> Vec<Value> {
        let n = promote::window_wanted_for(&SUMMON_DIFFICULTY)
            .unwrap()
            .max(MIN_EXECUTIONS);
        let mut rows = Vec::new();
        for applied in [false, true] {
            for i in 0..n {
                let id = format!("{applied}-{i}");
                let mut row = json!({"at": i, "requestAt":i, "dispatch":id, "task":id, "run":"r",
                    "rubricVersion":super::super::RUBRIC_VERSION, "model":"jev-test",
                    "outcome":"answered", "elapsedMs":1, "confidence":0.9,
                    "agent":"claude", "chosen":super::super::LADDER[0].0,
                    "pinnedEffort":super::super::LADDER[2].2, "baselineHigh":true,
                    "applied":applied, "attempt":0, "retryOf":false});
                rows.push(row.clone());
                row.as_object_mut().unwrap().remove("outcome");
                row["label"] = json!(id);
                let cost = if applied { 100 } else { 200 };
                row[KEY] = serde_json::to_value(Outcome {
                    first_attempt_success: Some(if applied {
                        applied_success
                    } else {
                        pinned_success
                    }),
                    done_with_receipts: true,
                    landed: true,
                    rework_rounds: 0,
                    retry_count: 0,
                    wall_ms: Some(cost),
                    tokens: Some(cost),
                    tokens_reason: None,
                    task_tokens: Some(cost),
                    task_wall_ms: Some(cost),
                })
                .unwrap();
                rows.push(row);
            }
        }
        rows
    }

    /// A fall of the words the seat asks now, as the judge writes one.
    fn fell() -> Value {
        promote::transition_row(
            &SUMMON_DIFFICULTY,
            0,
            promote::Verdict::Fall(promote::Line::ExecutionQuality),
            &crate::jev::summary::Tally::default(),
        )
        .unwrap()
    }

    #[test]
    fn the_real_judge_joins_outcomes_and_never_promotes_old_pin_marks() {
        let n = promote::window_wanted_for(&SUMMON_DIFFICULTY)
            .unwrap()
            .max(MIN_EXECUTIONS);
        // Recording: the seat fell once, so the rows are what it rises on.
        let mut rows = vec![fell()];
        rows.extend(executions(true, false));
        let judged = promote::judge_seat(&SUMMON_DIFFICULTY, &rows).unwrap();
        assert_eq!(judged.agreement.compared, n);
        assert_eq!(judged.verdict, promote::Verdict::Rise);
        for row in &mut rows {
            row["rubricVersion"] = json!(1);
            row["agreed"] = json!(true);
            row["baselineAgreed"] = json!(false);
        }
        assert_ne!(
            promote::judge_seat(&SUMMON_DIFFICULTY, &rows)
                .unwrap()
                .verdict,
            promote::Verdict::Rise
        );
    }

    // The difficulty seat acts from its first summons under `auto`
    // (t-11989): its marks come only from answers that were carried out, so
    // a seat that recorded first could never earn one. Its judge still reads
    // those marks and stops it.

    /// With no transition, a record whose carried-out answers failed their
    /// first attempt where the coordinators' own pins at the same difficulty
    /// succeeded falls on the execution line — and the fall row stands the
    /// seat at recording, so its next answer is not carried out.
    #[test]
    fn an_acting_difficulty_seat_falls_on_its_own_bad_marks() {
        let rows = executions(false, true);
        assert_eq!(
            promote::standing(&SUMMON_DIFFICULTY, &rows),
            promote::Stand::Applying,
            "the difficulty seat acts until its judge says otherwise"
        );
        let judged = promote::judge_seat(&SUMMON_DIFFICULTY, &rows).unwrap();
        assert_eq!(
            judged.verdict,
            promote::Verdict::Fall(promote::Line::ExecutionQuality)
        );
        let mut after = rows;
        after.push(
            promote::transition_row(&SUMMON_DIFFICULTY, 1, judged.verdict, &judged.window)
                .expect("a fall is written down"),
        );
        assert_eq!(
            promote::standing(&SUMMON_DIFFICULTY, &after),
            promote::Stand::Recording
        );
    }

    /// Evidence that is not in yet takes nothing back from an acting seat: a
    /// full window of summonses none of whose work has ended, and executions
    /// whose first attempts held up but whose costs nobody could measure — a
    /// gap in the bookkeeping, not a mark against the seat.
    #[test]
    fn an_acting_difficulty_seat_keeps_acting_while_its_evidence_is_thin() {
        let asked: Vec<serde_json::Value> = executions(true, true)
            .into_iter()
            .filter(|row| row.get(KEY).is_none())
            .collect();
        let judged = promote::judge_seat(&SUMMON_DIFFICULTY, &asked).unwrap();
        assert_eq!(judged.verdict, promote::Verdict::Keep, "{judged:?}");
        let mut unmeasured = executions(true, true);
        for row in unmeasured
            .iter_mut()
            .filter(|row| row.get(KEY).is_some() && row["applied"] == true)
        {
            row[KEY]["taskTokens"] = serde_json::Value::Null;
        }
        let judged = promote::judge_seat(&SUMMON_DIFFICULTY, &unmeasured).unwrap();
        assert_eq!(judged.verdict, promote::Verdict::Keep, "{judged:?}");
    }
}
