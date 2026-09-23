//! Record-only pair judgments for the second-brain weekly review.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use api::{SystemOneConfig, SystemOneQuestion, SystemOneQuestionKind, SystemOneRequest, SystemOneResponse, SYSTEMONE_MODEL};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use zerocode_core::jev::{VAULT_PAIRS, VAULT_PAIR_DEADLINE_MS, digest_of};
use zerocode_core::jev::noul;
use zerocode_core::jev::questions::{
    VAULT_PAIR_LINK_LEVELS, VAULT_PAIR_LINK_QUESTION, VAULT_PAIR_NO,
    VAULT_PAIR_OPPOSITE_CLAIM, VAULT_PAIR_REPLACES, VAULT_PAIR_RUBRIC_VERSION,
    VAULT_PAIR_SAME_CLAIM, VAULT_PAIR_YES,
    vault_pair_rubric_fingerprint,
};
use zerocode_core::second_brain_graph::VaultGraph;
use zerocode_core::second_brain_pairs::{Pair, ProposalRecord, Suggestion, candidates, suggest, valid_proposals};

use super::jev_gate::{self, JevDoor};
use super::shadow_ledger::{SHADOW_LEDGER_MAX_BYTES, append_shadow_row, read_shadow_rows, shadow_ledger_path};

/// One row keeps the pair and model answer, without copying private page text.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairJudgment {
    pub at: i64,
    pub left: String,
    pub right: String,
    pub left_modified_ms: i64,
    pub right_modified_ms: i64,
    pub reason: String,
    pub outcome: String,
    pub proposal: Suggestion,
    pub link_state: Option<f64>,
    pub link_confidence: Option<f64>,
    pub same_claim: Option<f64>,
    pub opposite_claim: Option<f64>,
    pub replaces: Option<f64>,
    pub model: Option<String>,
    pub input_tokens: Option<u64>,
    pub elapsed_ms: u64,
    pub requests: u32,
    pub redacted_lines: u32,
    pub request_digest: Option<String>,
    pub rubric_version: u32,
    pub rubric_fingerprint: String,
    pub confidence: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairRun {
    pub status: String,
    pub mode: String,
    pub pages: usize,
    pub candidates: usize,
    pub asked: usize,
    pub answered: usize,
    pub proposals: usize,
    pub input_tokens: u64,
    pub cost_usd: Option<f64>,
    pub requests: u32,
    pub rows: Vec<PairJudgment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairLabel {
    pub kind: String,
    pub at: i64,
    pub left: String,
    pub right: String,
    pub left_modified_ms: i64,
    pub right_modified_ms: i64,
    pub proposed: Suggestion,
    pub actual: Suggestion,
    pub accepted: bool,
    pub agreed: bool,
    pub baseline_agreed: bool,
    pub confidence: Option<f64>,
}

/// Rebuild the private proposal cache from answered ledger rows after an
/// interrupted run. A failed request and a changed page cannot be reused.
#[must_use]
pub fn recorded_vault_pair_proposals(graph: &VaultGraph, vault: &Path) -> Vec<ProposalRecord> {
    let ledger = shadow_ledger_path(vault, VAULT_PAIRS.ledger);
    let rows: Vec<Value> = read_shadow_rows(&ledger);
    proposals_in(graph, &rows)
}

fn proposals_in(graph: &VaultGraph, rows: &[Value]) -> Vec<ProposalRecord> {
    let mut found = BTreeMap::new();
    for row in rows {
        if row["outcome"] != "answered" { continue; }
        let (Some(left), Some(right), Some(reason), Some(suggestion)) = (
            row["left"].as_str(), row["right"].as_str(), row["reason"].as_str(),
            row["proposal"].as_str().and_then(Suggestion::from_word),
        ) else { continue; };
        let Some(a) = graph.nodes.iter().find(|node| node.id == left) else { continue; };
        let Some(b) = graph.nodes.iter().find(|node| node.id == right) else { continue; };
        if row["leftModifiedMs"].as_i64() != Some(a.modified_ms)
            || row["rightModifiedMs"].as_i64() != Some(b.modified_ms)
        { continue; }
        found.insert((left.to_string(), right.to_string()), ProposalRecord {
            left: left.to_string(), right: right.to_string(), reason: reason.to_string(),
            left_modified_ms: a.modified_ms, right_modified_ms: b.modified_ms, suggestion,
        });
    }
    found.into_values().collect()
}

/// A weekly reviewer records the relation they chose, including `none` for
/// an explicit rejection. An absent review remains unlabeled.
pub fn mark_vault_pair(
    graph: &VaultGraph, vault: &Path, left: &str, right: &str,
    actual: Suggestion, now_ms: i64,
) -> Result<PairLabel, String> {
    let proposal = valid_proposals(vault, graph).into_iter().find(|row|
        (row.left == left && row.right == right) || (row.left == right && row.right == left))
        .ok_or("no current proposal for that pair")?;
    let ledger = shadow_ledger_path(vault, VAULT_PAIRS.ledger);
    let old: Vec<Value> = read_shadow_rows(&ledger);
    if old.iter().any(|row| row["kind"] == "label"
        && row["left"] == proposal.left && row["right"] == proposal.right
        && row["leftModifiedMs"] == proposal.left_modified_ms
        && row["rightModifiedMs"] == proposal.right_modified_ms)
    {
        return Err("pair already reviewed at this page version".into());
    }
    let confidence = old.iter().rev().find(|row| row["outcome"] == "answered"
        && row["left"] == proposal.left && row["right"] == proposal.right
        && row["leftModifiedMs"] == proposal.left_modified_ms
        && row["rightModifiedMs"] == proposal.right_modified_ms)
        .and_then(|row| row["confidence"].as_f64());
    let label = PairLabel {
        kind: "label".into(), at: now_ms,
        left: proposal.left, right: proposal.right,
        left_modified_ms: proposal.left_modified_ms,
        right_modified_ms: proposal.right_modified_ms,
        proposed: proposal.suggestion, actual,
        accepted: actual != Suggestion::None,
        agreed: proposal.suggestion == actual,
        baseline_agreed: actual == Suggestion::None,
        confidence,
    };
    append_shadow_row(&ledger, &label, SHADOW_LEDGER_MAX_BYTES)
        .map_err(|error| error.to_string())?;
    Ok(label)
}

fn questions() -> BTreeMap<String, SystemOneQuestion> {
    BTreeMap::from([
        ("link_state".into(), SystemOneQuestion::score(
            VAULT_PAIR_LINK_QUESTION,
            VAULT_PAIR_LINK_LEVELS,
        )),
        ("same_claim".into(), SystemOneQuestion::noul(
            VAULT_PAIR_SAME_CLAIM, VAULT_PAIR_YES, VAULT_PAIR_NO,
        )),
        ("opposite_claim".into(), SystemOneQuestion::noul(
            VAULT_PAIR_OPPOSITE_CLAIM, VAULT_PAIR_YES, VAULT_PAIR_NO,
        )),
        ("replaces".into(), SystemOneQuestion::noul(
            VAULT_PAIR_REPLACES, VAULT_PAIR_YES, VAULT_PAIR_NO,
        )),
    ])
}

fn state(graph: &VaultGraph, pair: &Pair) -> Option<Value> {
    let a = graph.nodes.iter().find(|node| node.id == pair.left)?;
    let b = graph.nodes.iter().find(|node| node.id == pair.right)?;
    let page = |node: &zerocode_core::second_brain_graph::GraphNode| json!({
        "title": node.title, "summary": node.excerpt, "tags": node.tags,
    });
    Some(json!({"page_a": page(a), "page_b": page(b)}))
}

fn read(response: &SystemOneResponse) -> Option<(f64, f64, f64, f64, f64)> {
    if response.answers.len() != 4 { return None; }
    let score = response.score_answer("link_state")?.ok()?;
    if score.kind != SystemOneQuestionKind::Score || !(0.0..=2.0).contains(&score.score)
        || !(0.0..=1.0).contains(&score.confidence)
        || score.probabilities.len() != 3
        || !["0", "1", "2"].iter().all(|key| score.probabilities.contains_key(*key))
        || !score.probabilities.values().all(|probability| (0.0..=1.0).contains(probability))
        || (score.probabilities.values().sum::<f64>() - 1.0).abs()
            > 3.0 * zerocode_core::jev::WIRE_ROUNDING
    { return None; }
    let answers = serde_json::to_value(&response.answers).ok()?;
    Some((
        score.score,
        score.confidence,
        noul::read(&answers, "same_claim").ok()?,
        noul::read(&answers, "opposite_claim").ok()?,
        noul::read(&answers, "replaces").ok()?,
    ))
}

/// Ask each selected pair through zo's one Jev door. The call never edits a
/// page; the only durable output is the existing project Jev ledger.
pub async fn judge_vault_pairs(graph: &VaultGraph, vault: &Path, limit: usize, now_ms: i64) -> PairRun {
    let pairs = candidates(graph);
    let held: std::collections::HashSet<(String, String)> = valid_proposals(vault, graph)
        .into_iter().chain(recorded_vault_pair_proposals(graph, vault))
        .map(|row| (row.left, row.right)).collect();
    let mode = jev_gate::mode_in_person_settings(&VAULT_PAIRS);
    let mut report = PairRun {
        status: "ready".into(), mode: mode.key().into(),
        pages: graph.pages, candidates: pairs.len(), asked: 0, answered: 0,
        proposals: 0, input_tokens: 0, cost_usd: None, requests: 0, rows: Vec::new(),
    };
    if !mode.asks() {
        report.status = "off".into();
        return report;
    }
    let door = JevDoor::open(vault);
    let client = SystemOneConfig::from_env().ok().map(SystemOneConfig::into_client);
    if client.is_none() {
        report.status = "no_key".into();
        return report;
    }
    let asked = questions();
    let ledger = shadow_ledger_path(vault, VAULT_PAIRS.ledger);
    for pair in pairs.into_iter().filter(|pair| !held.contains(&(pair.left.clone(), pair.right.clone()))).take(limit) {
        let mut row = PairJudgment {
            at: now_ms, left: pair.left.clone(), right: pair.right.clone(),
            left_modified_ms: graph.nodes.iter().find(|node| node.id == pair.left)
                .map_or(0, |node| node.modified_ms),
            right_modified_ms: graph.nodes.iter().find(|node| node.id == pair.right)
                .map_or(0, |node| node.modified_ms),
            reason: pair.reason.clone(), outcome: "unavailable".into(), proposal: Suggestion::None,
            link_state: None, link_confidence: None,
            same_claim: None, opposite_claim: None, replaces: None,
            model: None, input_tokens: None, elapsed_ms: 0, requests: 0,
            redacted_lines: 0, request_digest: None, rubric_version: VAULT_PAIR_RUBRIC_VERSION,
            rubric_fingerprint: vault_pair_rubric_fingerprint(),
            confidence: None,
        };
        let Some(state) = state(graph, &pair) else { continue; };
        let request = SystemOneRequest { state: &state, model: SYSTEMONE_MODEL, questions: &asked };
        let Some(body) = jev_gate::body_of(&request) else { continue; };
        match (door.pass(&VAULT_PAIRS, client.is_some(), body), client.as_ref()) {
            (Ok(cleared), Some(client)) => {
                row.redacted_lines = u32::try_from(cleared.withheld_lines()).unwrap_or(u32::MAX);
                row.request_digest = Some(digest_of(VAULT_PAIRS.id, VAULT_PAIR_RUBRIC_VERSION, door.model(), cleared.bytes()));
                report.asked += 1;
                let call = jev_gate::send(client, cleared, Duration::from_millis(VAULT_PAIR_DEADLINE_MS), None).await;
                row.elapsed_ms = jev_gate::millis(call.elapsed);
                row.requests = call.requests;
                report.requests += call.requests;
                match call.outcome {
                    Ok(response) => {
                        row.model = Some(response.model.clone());
                        row.input_tokens = Some(response.usage.input_tokens);
                        report.input_tokens += response.usage.input_tokens;
                        if let Some((link, score_confidence, same, opposite, replaces)) = read(&response) {
                            row.outcome = "answered".into();
                            row.link_state = Some(link);
                            row.link_confidence = Some(score_confidence);
                            row.same_claim = Some(same);
                            row.opposite_claim = Some(opposite);
                            row.replaces = Some(replaces);
                            row.proposal = suggest(link, same, opposite, replaces);
                            row.confidence = match row.proposal {
                                Suggestion::Merge => Some((2.0 * same - 1.0).abs()),
                                Suggestion::Supersedes => Some((2.0 * replaces - 1.0).abs()),
                                Suggestion::Contradicts => Some((2.0 * opposite - 1.0).abs()),
                                Suggestion::Related | Suggestion::None => None,
                            };
                            report.answered += 1;
                            report.proposals += usize::from(row.proposal != Suggestion::None);
                        } else { row.outcome = "schema".into(); }
                    }
                    Err(failure) => row.outcome = failure.ledger_token(),
                }
            }
            (passed, _) => row.outcome = passed.err().map_or_else(|| "no_key".into(), |refusal| refusal.token().into()),
        }
        let stop = matches!(row.outcome.as_str(), "not_consented" | "budget" | "off" | "no_key");
        let _ = append_shadow_row(&ledger, &row, SHADOW_LEDGER_MAX_BYTES);
        if stop { report.status = row.outcome.clone(); }
        report.rows.push(row);
        if stop { break; }
    }
    report.cost_usd = api::systemone_rate(SYSTEMONE_MODEL)
        .map(|rate| rate.input_cost_usd(report.input_tokens));
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_request_has_four_atomic_questions_and_a_no_match_level() {
        let asked = questions();
        assert_eq!(asked.len(), 4);
        let api::SystemOneCriteria::Ordered(levels) = &asked["link_state"].criteria else {
            panic!("score levels");
        };
        assert_eq!(levels.first().map(String::as_str), Some(VAULT_PAIR_LINK_LEVELS[0]));
    }

    #[test]
    fn a_typed_answer_keeps_the_score_and_three_nouls_separate() {
        let response: SystemOneResponse = serde_json::from_value(json!({
            "model": "jev-1.13.0",
            "usage": {"input_tokens": 100, "output_tokens": 4},
            "answers": {
                "link_state": {"type": "score", "score": 1.8,
                    "legend": {"0": VAULT_PAIR_LINK_LEVELS[0], "1": VAULT_PAIR_LINK_LEVELS[1], "2": VAULT_PAIR_LINK_LEVELS[2]},
                    "probabilities": {"0": 0.1, "1": 0.0, "2": 0.9}, "confidence": 0.85},
                "same_claim": {"type": "noul", "noul": 0.9},
                "opposite_claim": {"type": "noul", "noul": 0.1},
                "replaces": {"type": "noul", "noul": 0.1}
            }
        })).expect("typed response");
        let (score, _, same, opposite, replaces) = read(&response).expect("valid");
        assert_eq!(suggest(score, same, opposite, replaces), Suggestion::Merge);
    }

    #[test]
    fn a_recovered_answer_requires_the_same_page_versions() {
        let vault = tempfile::tempdir().expect("vault");
        let wiki = vault.path().join("wiki");
        std::fs::create_dir(&wiki).expect("wiki");
        std::fs::write(wiki.join("a.md"), "---\ntitle: A\n---\nFirst note.").expect("a");
        std::fs::write(wiki.join("b.md"), "---\ntitle: B\n---\nSecond note.").expect("b");
        let graph = zerocode_core::second_brain_graph::GraphCache::new().scan(vault.path(), false);
        let a = graph.nodes.iter().find(|node| node.id == "wiki/a.md").expect("a");
        let b = graph.nodes.iter().find(|node| node.id == "wiki/b.md").expect("b");
        let row = json!({"outcome":"answered", "left":a.id, "right":b.id,
            "reason":"bm25", "proposal":"related",
            "leftModifiedMs":a.modified_ms, "rightModifiedMs":b.modified_ms});
        assert_eq!(proposals_in(&graph, std::slice::from_ref(&row)).len(), 1);
        let mut stale = row;
        stale["leftModifiedMs"] = json!(a.modified_ms - 1);
        assert!(proposals_in(&graph, &[stale]).is_empty());
    }

    /// Run with `VAULT_PAIR_REPLAY_SEED` pointing at seed.py's private output.
    #[test]
    #[ignore = "requires an explicitly reviewed vault-pair seed"]
    fn replay_reports_precision_baseline_and_cost() {
        #[derive(Deserialize)]
        struct Sample {
            proposed: String,
            actual: Option<String>,
            input_tokens: u64,
        }
        #[derive(Deserialize)]
        struct Seed { samples: Vec<Sample> }
        let path = std::env::var("VAULT_PAIR_REPLAY_SEED").expect("seed path");
        let bytes = std::fs::read(path).expect("seed");
        let seed: Seed = serde_json::from_slice(&bytes).expect("seed shape");
        let labeled: Vec<&Sample> = seed.samples.iter().filter(|sample| sample.actual.is_some()).collect();
        let accepted = labeled.iter().filter(|sample| sample.actual.as_deref() != Some("none")).count();
        let proposed = labeled.iter().filter(|sample| sample.proposed != "none").count();
        let precise = labeled.iter().filter(|sample| sample.proposed != "none"
            && sample.actual.as_deref() == Some(sample.proposed.as_str())).count();
        let baseline = labeled.iter().filter(|sample| sample.actual.as_deref() == Some("none")).count();
        let agreement = labeled.iter().filter(|sample|
            sample.actual.as_deref() == Some(sample.proposed.as_str())).count();
        let after_lower = (!labeled.is_empty()).then(|| zerocode_core::jev::summary::wilson_lower(
            agreement, labeled.len(), zerocode_core::jev::summary::WILSON_Z_95));
        let baseline_lower = (!labeled.is_empty()).then(|| zerocode_core::jev::summary::wilson_lower(
            baseline, labeled.len(), zerocode_core::jev::summary::WILSON_Z_95));
        let tokens: u64 = seed.samples.iter().map(|sample| sample.input_tokens).sum();
        let cost = api::systemone_rate(SYSTEMONE_MODEL).map(|rate| rate.input_cost_usd(tokens));
        let precision = (proposed != 0).then(|| {
            f64::from(u32::try_from(precise).expect("bounded replay count"))
                / f64::from(u32::try_from(proposed).expect("bounded replay count"))
        });
        eprintln!("pairs={} labeled={} accepted={} proposed_labeled={} precise={} precision={:?} after_agreed={} after_lower={after_lower:?} baseline_agreed={} baseline_lower={baseline_lower:?} tokens={} cost_usd={cost:?}",
            seed.samples.len(), labeled.len(), accepted, proposed, precise,
            precision, agreement, baseline, tokens);
        assert!(labeled.len() <= seed.samples.len());
    }
}
