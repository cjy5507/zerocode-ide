//! zo's model tier seat (t-45961): asks Jev where each model of a provider's
//! lineup stands, keeps the answer beside the model catalog cache, and hands
//! the tier classifier the judgment that stands.
//!
//! A lineup is asked once per fingerprint — its catalog facts and its record
//! counted in steps (`runtime::tier_fingerprint`) — so a model arriving, a
//! price or class moving, or the recorded outcomes crossing a step asks
//! again, and nothing else does. A lineup asked and left unanswered (the
//! seat off, no key, the wall passed, a refused or malformed answer) keeps
//! its last judgment, under which a model that arrived since stands
//! provisionally by the facts, or none — the fact rule. Under `shadow` the
//! lineup is asked and kept and its row written, and nothing is applied.
//! What leaves is catalog facts and counts, never a word a person wrote.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use api::{SystemOneConfig, SystemOneFailure, SystemOneRequest, SYSTEMONE_MODEL};
use runtime::{
    read_tier_answers, tier_fingerprint, tier_questions, tier_state, JudgedPlace, ModelDescriptor, TierJudgment,
    TierOutcomes,
};
use serde::{Deserialize, Serialize};
use zerocode_core::jev::door::Refused;
use zerocode_core::jev::{JevMode, MODEL_TIERS};

use super::decision_shadow::OUTCOME_ANSWERED;
use super::jev_gate::{self, JevDoor};
use super::shadow_ledger::{append_shadow_row, shadow_ledger_path, SHADOW_LEDGER_MAX_BYTES};

/// The file the lineups' judgments are kept in, beside the discovery cache.
pub const TIER_JUDGMENT_FILE: &str = "tier-judgment.json";

/// The wall one lineup's judgment is held to.
pub const TIER_JUDGMENT_DEADLINE: Duration = Duration::from_millis(zerocode_core::jev::MODEL_TIERS_DEADLINE_MS);

/// Where the lineups' judgments are kept: beside zo's model catalog cache.
#[must_use]
pub fn tier_judgment_path() -> PathBuf {
    runtime::model_discovery::cache_dir().join(TIER_JUDGMENT_FILE)
}

/// The kept file: every provider's last judgment, under the question's
/// version — a judgment asked under another version is never applied.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Kept {
    rubric_version: u32,
    #[serde(default)]
    providers: BTreeMap<String, KeptLineup>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KeptLineup {
    fingerprint: String,
    judged_at_ms: i64,
    model: String,
    places: BTreeMap<String, KeptPlace>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KeptPlace {
    score_permille: u16,
    confidence_permille: u16,
}

fn read_kept(path: &Path) -> Kept {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<Kept>(&raw).ok())
        .filter(|kept| kept.rubric_version == zerocode_core::jev::questions::MODEL_TIERS_RUBRIC_VERSION)
        .unwrap_or_else(|| Kept {
            rubric_version: zerocode_core::jev::questions::MODEL_TIERS_RUBRIC_VERSION,
            providers: BTreeMap::new(),
        })
}

/// Write the kept file whole, through a sibling, so a reader never meets
/// half of it.
fn write_kept(path: &Path, kept: &Kept) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let sibling = path.with_extension("json.tmp");
    std::fs::write(&sibling, serde_json::to_vec_pretty(kept).map_err(std::io::Error::other)?)?;
    std::fs::rename(&sibling, path)
}

/// One lineup's row in the seat's ledger: what was asked, what came back,
/// how long it took and whether it was applied.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TierRow {
    pub at_ms: i64,
    pub provider: String,
    pub fingerprint: String,
    pub models: usize,
    pub outcome: String,
    pub judged: usize,
    pub applied: bool,
    #[serde(default)]
    pub elapsed_ms: u64,
    #[serde(default)]
    pub retries: u32,
    #[serde(default)]
    pub requests: u32,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub input_tokens: Option<u64>,
}

/// One provider's lineup, its models in the fact rule's order (superseded
/// and retired releases left out).
pub type Lineup<'a> = (String, Vec<&'a ModelDescriptor>);

/// What the seat did over every lineup: the judgment to apply, and a row per
/// lineup it asked or answered from the kept file.
#[derive(Debug, Clone, Default)]
pub struct TierSeatPass {
    pub judgment: TierJudgment,
    pub rows: Vec<TierRow>,
}

fn places_of(kept: &KeptLineup) -> impl Iterator<Item = (String, JudgedPlace)> + '_ {
    kept.places.iter().map(|(id, place)| {
        (
            id.clone(),
            JudgedPlace { score_permille: place.score_permille, confidence_permille: place.confidence_permille },
        )
    })
}

/// Ask one lineup and say what came back.
async fn ask_lineup(
    door: &JevDoor,
    client: Option<&api::SystemOneClient>,
    provider: &str,
    models: &[&ModelDescriptor],
    outcomes: &dyn Fn(&str) -> Option<TierOutcomes>,
) -> (Result<(TierJudgment, String), String>, TierRow) {
    let mut row = TierRow {
        at_ms: 0,
        provider: provider.to_string(),
        fingerprint: String::new(),
        models: models.len(),
        outcome: String::new(),
        judged: 0,
        applied: false,
        elapsed_ms: 0,
        retries: 0,
        requests: 0,
        model: None,
        input_tokens: None,
    };
    let state = tier_state(provider, models, outcomes);
    let questions = tier_questions(models.len());
    let request = SystemOneRequest { state: &state, model: SYSTEMONE_MODEL, questions: &questions };
    let Some(body) = jev_gate::body_of(&request) else {
        row.outcome = SystemOneFailure::InvalidRequest.ledger_token();
        return (Err(row.outcome.clone()), row);
    };
    let (cleared, client) = match (door.pass(&MODEL_TIERS, client.is_some(), body), client) {
        (Ok(cleared), Some(client)) => (cleared, client),
        (passed, _) => {
            row.outcome = passed.err().unwrap_or(Refused::NoKey).token().to_string();
            return (Err(row.outcome.clone()), row);
        }
    };
    let call = jev_gate::send(client, cleared, TIER_JUDGMENT_DEADLINE, None).await;
    row.elapsed_ms = jev_gate::millis(call.elapsed);
    row.retries = call.retries;
    row.requests = call.requests;
    let response = match call.outcome {
        Ok(response) => response,
        Err(failure) => {
            row.outcome = failure.ledger_token();
            return (Err(row.outcome.clone()), row);
        }
    };
    row.model = Some(response.model.clone());
    row.input_tokens = Some(response.usage.input_tokens);
    let ids: Vec<&str> = models.iter().map(|model| model.id()).collect();
    if let Ok(judgment) = read_tier_answers(&response, &ids) {
        row.outcome = OUTCOME_ANSWERED.to_string();
        row.judged = judgment.places.len();
        (Ok((judgment, response.model)), row)
    } else {
        row.outcome = SystemOneFailure::Schema.ledger_token();
        (Err(row.outcome.clone()), row)
    }
}

/// The judgment that stands over `lineups` under `mode`, read from and
/// written to the kept file at `kept_at`. Each lineup is answered from the
/// kept file when its fingerprint stands there; else, under a mode that asks,
/// it is asked once inside the wall; unanswered, it keeps its last judgment.
/// Only `on` applies what stands.
pub async fn judge_lineups(
    door: &JevDoor,
    client: Option<&api::SystemOneClient>,
    mode: JevMode,
    kept_at: &Path,
    lineups: &[Lineup<'_>],
    outcomes: &dyn Fn(&str) -> Option<TierOutcomes>,
    now_ms: i64,
) -> TierSeatPass {
    let mut pass = TierSeatPass::default();
    if !mode.asks() {
        return pass;
    }
    let applies = mode.applies();
    let mut kept = read_kept(kept_at);
    let mut changed = false;
    for (provider, models) in lineups.iter().filter(|(_, models)| !models.is_empty()) {
        let fingerprint = tier_fingerprint(provider, models, outcomes);
        let standing = kept.providers.get(provider).filter(|lineup| lineup.fingerprint == fingerprint);
        if let Some(lineup) = standing {
            if applies {
                pass.judgment.places.extend(places_of(lineup));
            }
            continue;
        }
        let (answer, mut row) = ask_lineup(door, client, provider, models, outcomes).await;
        row.at_ms = now_ms;
        row.fingerprint.clone_from(&fingerprint);
        if let Ok((judgment, model)) = answer {
            row.applied = applies;
            if applies {
                pass.judgment.places.extend(judgment.places.iter().map(|(id, place)| (id.clone(), *place)));
            }
            kept.providers.insert(
                provider.clone(),
                KeptLineup {
                    fingerprint,
                    judged_at_ms: now_ms,
                    model,
                    places: judgment
                        .places
                        .into_iter()
                        .map(|(id, place)| {
                            (
                                id,
                                KeptPlace {
                                    score_permille: place.score_permille,
                                    confidence_permille: place.confidence_permille,
                                },
                            )
                        })
                        .collect(),
                },
            );
            changed = true;
        } else if let Some(last) = kept.providers.get(provider).filter(|_| applies) {
            // The last judgment still places the models it covered; a model
            // that arrived since stands provisionally by the facts.
            row.applied = true;
            pass.judgment.places.extend(places_of(last));
        }
        pass.rows.push(row);
    }
    if changed {
        let _ = write_kept(kept_at, &kept);
    }
    pass
}

/// The seat's mode for `cwd`: the merged settings' word for it, read the way
/// the use table reads every seat's.
#[must_use]
pub fn tier_seat_mode(cwd: &Path) -> JevMode {
    super::settings::merged_settings_root(cwd).map_or(JevMode::Off, |root| MODEL_TIERS.mode_in(&root))
}

/// The judgment that stands for `lineups` on this machine, its rows written
/// to the seat's ledger for `cwd` — what `zo models` applies before it
/// prints a row's band.
#[must_use]
pub fn judgment_for(
    cwd: &Path,
    lineups: &[Lineup<'_>],
    outcomes: &dyn Fn(&str) -> Option<TierOutcomes>,
    now_ms: i64,
) -> TierJudgment {
    let mode = tier_seat_mode(cwd);
    if !mode.asks() {
        return TierJudgment::default();
    }
    let door = JevDoor::open(cwd);
    let client = SystemOneConfig::from_env().ok().map(SystemOneConfig::into_client);
    let pass = api::sync_bridge::run_blocking(judge_lineups(
        &door,
        client.as_ref(),
        mode,
        &tier_judgment_path(),
        lineups,
        outcomes,
        now_ms,
    ));
    let ledger = shadow_ledger_path(cwd, MODEL_TIERS.ledger);
    for row in &pass.rows {
        let _ = append_shadow_row(&ledger, row, SHADOW_LEDGER_MAX_BYTES);
    }
    pass.judgment
}

#[cfg(test)]
mod tests;
