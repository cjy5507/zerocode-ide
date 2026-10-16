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
//! provisionally by the facts, or none — the fact rule; and when its
//! question went out and came back unanswered, the same facts are not asked
//! again for [`TIER_REFUSAL_MEMORY_MS`]. The lineups due a question are asked
//! together, each under the one wall. Under `shadow` the lineup is asked and
//! kept and its row written, and nothing is applied. What leaves is catalog
//! facts and counts; the one string a person may have written in it — a
//! custom provider's name or model id — passes the door as declared.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use api::{SystemOneConfig, SystemOneFailure, SystemOneRequest, SYSTEMONE_MODEL};
use runtime::{
    read_tier_answers, tier_fingerprint, tier_questions, tier_state, JudgedPlace, ModelDescriptor, TierJudgment,
    TierOutcomes,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zerocode_core::jev::door::Refused;
use zerocode_core::jev::{JevMode, MODEL_TIERS};

use super::decision_shadow::OUTCOME_ANSWERED;
use super::jev_gate::{self, JevDoor};
use super::shadow_ledger::{append_shadow_row, shadow_ledger_path, SHADOW_LEDGER_MAX_BYTES};

/// The file the lineups' judgments are kept in, beside the discovery cache.
pub const TIER_JUDGMENT_FILE: &str = "tier-judgment.json";

/// The wall each lineup's question is held to. The lineups due a question
/// are asked together, so it is also the longest one pass waits on them.
pub const TIER_JUDGMENT_DEADLINE: Duration = Duration::from_millis(zerocode_core::jev::MODEL_TIERS_DEADLINE_MS);

/// How long a lineup's question that went out and came back unanswered —
/// refused (HTTP 402 on an account without credit), failed, late or
/// malformed — is remembered for the facts it was asked under: inside it,
/// `zo models` asks that lineup nothing and places it as an unanswered one.
/// An hour is the window's own lineup cadence (it reads `zo models` in the
/// background once an hour, and its model menu at most once a minute), so a
/// refusal costs one request an hour instead of one a minute, and an account
/// put right is asked again within the hour — at once when its lineup's facts
/// move, since a new fingerprint is a new question.
pub const TIER_REFUSAL_MEMORY_MS: i64 = 60 * 60 * 1000;

/// Where the lineups' judgments are kept: beside zo's model catalog cache.
#[must_use]
pub fn tier_judgment_path() -> PathBuf {
    let door = JevDoor::for_key_check();
    tier_judgment_path_for(&runtime::model_discovery::cache_dir(), door.model())
}

fn tier_judgment_path_for(cache_dir: &Path, backend_identity: &str) -> PathBuf {
    cache_dir
        .join("judgment-backends")
        .join(format!("{:x}", Sha256::digest(backend_identity.as_bytes())))
        .join(TIER_JUDGMENT_FILE)
}

/// The kept file: every provider's last judgment, and its last question
/// that went out unanswered, under the question's version — a judgment asked
/// under another version is never applied.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Kept {
    rubric_version: u32,
    #[serde(default)]
    backend_identity: String,
    #[serde(default)]
    providers: BTreeMap<String, KeptLineup>,
    #[serde(default)]
    unanswered: BTreeMap<String, KeptUnanswered>,
}

/// A lineup's question that went out and came back unanswered: the facts it
/// was asked under, when, and what came back instead (`http_402`,
/// `timeout`, …).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KeptUnanswered {
    fingerprint: String,
    asked_at_ms: i64,
    outcome: String,
}

impl Kept {
    /// Whether `provider`'s lineup was asked under `fingerprint` and left
    /// unanswered within the memory before `now_ms`.
    fn left_unanswered(&self, provider: &str, fingerprint: &str, now_ms: i64) -> bool {
        self.unanswered.get(provider).is_some_and(|last| {
            last.fingerprint == fingerprint && now_ms.saturating_sub(last.asked_at_ms) < TIER_REFUSAL_MEMORY_MS
        })
    }
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

fn read_kept(path: &Path, backend_identity: &str) -> Kept {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<Kept>(&raw).ok())
        .filter(|kept| kept.rubric_version == zerocode_core::jev::questions::MODEL_TIERS_RUBRIC_VERSION)
        .filter(|kept| !backend_identity.is_empty() && kept.backend_identity == backend_identity)
        .unwrap_or_else(|| Kept {
            rubric_version: zerocode_core::jev::questions::MODEL_TIERS_RUBRIC_VERSION,
            backend_identity: backend_identity.to_string(),
            ..Kept::default()
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
    /// Kept under `at`, the key every Jev ledger's time is read by; rows
    /// written before t-55423 said `atMs`.
    #[serde(rename = "at", alias = "atMs")]
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
    /// What the server said beside a refusal (t-58193).
    #[serde(flatten)]
    pub server_said: jev_gate::ServerSaid,
}

/// One provider's lineup, its models in the fact rule's order (superseded
/// and retired releases left out).
pub type Lineup<'a> = (String, Vec<&'a ModelDescriptor>);

/// What one pass over the lineups asks with: the door and the client, the
/// seat's mode, the kept file, the moment, and the wall every question of the
/// pass is held to.
pub struct TierAsk<'a> {
    pub door: &'a JevDoor,
    pub client: Option<&'a api::SystemOneClient>,
    pub mode: JevMode,
    pub kept_at: &'a Path,
    pub now_ms: i64,
    pub wall: Duration,
}

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

/// Ask one lineup, inside `wall`, and say what came back.
async fn ask_lineup(
    door: &JevDoor,
    client: Option<&api::SystemOneClient>,
    provider: &str,
    models: &[&ModelDescriptor],
    outcomes: &dyn Fn(&str) -> Option<TierOutcomes>,
    wall: Duration,
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
        server_said: jev_gate::ServerSaid::default(),
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
    let call = jev_gate::send(client, cleared, wall, None).await;
    row.elapsed_ms = jev_gate::millis(call.elapsed);
    row.retries = call.retries;
    row.requests = call.requests;
    row.server_said = jev_gate::ServerSaid::of(&call);
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

/// The judgment that stands over `lineups` under `ask.mode`, read from and
/// written to the kept file at `ask.kept_at`. Each lineup is answered from
/// the kept file when its fingerprint stands there; it is asked nothing when
/// its question under the same fingerprint went out and came back
/// unanswered within [`TIER_REFUSAL_MEMORY_MS`]; else, under a mode that
/// asks, it is asked once — every lineup due a question at the same moment,
/// each under `ask.wall`, so a service that never answers holds the pass for
/// one wall and not for one per provider. Unanswered, a lineup keeps its last
/// judgment. Only `on` applies what stands.
pub async fn judge_lineups(
    ask: &TierAsk<'_>,
    lineups: &[Lineup<'_>],
    outcomes: &dyn Fn(&str) -> Option<TierOutcomes>,
) -> TierSeatPass {
    let mut pass = TierSeatPass::default();
    if !ask.mode.asks() {
        return pass;
    }
    let (applies, now_ms) = (ask.mode.applies(), ask.now_ms);
    let mut kept = read_kept(ask.kept_at, ask.door.model());
    let mut due: Vec<(&String, &[&ModelDescriptor], String)> = Vec::new();
    for (provider, models) in lineups.iter().filter(|(_, models)| !models.is_empty()) {
        let fingerprint = tier_fingerprint(provider, models, outcomes);
        let standing = kept.providers.get(provider).filter(|lineup| lineup.fingerprint == fingerprint);
        if let Some(lineup) = standing {
            if applies {
                pass.judgment.places.extend(places_of(lineup));
            }
        } else if kept.left_unanswered(provider, &fingerprint, now_ms) {
            // Asked under these facts a short while ago and left unanswered:
            // nothing is sent, and the last judgment stands as it would have.
            if let Some(last) = kept.providers.get(provider).filter(|_| applies) {
                pass.judgment.places.extend(places_of(last));
            }
        } else {
            due.push((provider, models.as_slice(), fingerprint));
        }
    }
    let asked = due.iter().map(|(provider, models, _)| ask_lineup(ask.door, ask.client, provider, models, outcomes, ask.wall));
    let answers = futures_util::future::join_all(asked).await;
    let mut changed = false;
    for ((provider, _, fingerprint), (answer, mut row)) in due.into_iter().zip(answers) {
        row.at_ms = now_ms;
        row.fingerprint.clone_from(&fingerprint);
        match answer {
            Ok((judgment, model)) => {
                row.applied = applies;
                if applies {
                    pass.judgment.places.extend(judgment.places.iter().map(|(id, place)| (id.clone(), *place)));
                }
                kept.unanswered.remove(provider);
                changed = true;
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
            }
            Err(outcome) => {
                // Only a question that left zo is remembered: a refusal at
                // the door (no key, no consent, the day's budget) sent
                // nothing, and is asked again as soon as it would pass.
                if row.requests > 0 {
                    kept.unanswered
                        .insert(provider.clone(), KeptUnanswered { fingerprint, asked_at_ms: now_ms, outcome });
                    changed = true;
                }
                if let Some(last) = kept.providers.get(provider).filter(|_| applies) {
                    // The last judgment still places the models it covered; a
                    // model that arrived since stands provisionally by the facts.
                    row.applied = true;
                    pass.judgment.places.extend(places_of(last));
                }
            }
        }
        pass.rows.push(row);
    }
    if changed {
        let _ = write_kept(ask.kept_at, &kept);
    }
    if !ask.door.permits_application_now() {
        pass.judgment = TierJudgment::default();
        for row in &mut pass.rows {
            row.applied = false;
        }
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
    let client = SystemOneConfig::from_env_for(Some(door.model())).ok().map(SystemOneConfig::into_client);
    let kept_at = tier_judgment_path_for(&runtime::model_discovery::cache_dir(), door.model());
    let ask = TierAsk {
        door: &door,
        client: client.as_ref(),
        mode,
        kept_at: &kept_at,
        now_ms,
        wall: TIER_JUDGMENT_DEADLINE,
    };
    let pass = api::sync_bridge::run_blocking(judge_lineups(&ask, lineups, outcomes));
    let ledger = shadow_ledger_path(cwd, MODEL_TIERS.ledger);
    for row in &pass.rows {
        let _ = append_shadow_row(&ledger, row, SHADOW_LEDGER_MAX_BYTES);
    }
    pass.judgment
}

#[cfg(test)]
mod tests;
