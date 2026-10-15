//! The tier seat on a scripted wire (t-45961): what leaves, when it leaves
//! again, what is kept and what is applied. No machine settings and no
//! network — a mock on a loopback port, a door told its facts, a kept file
//! in a directory of its own. The lineup is one made-up provider whose
//! models carry no vendor word.

use std::path::Path;
use std::time::Instant;

use api::SystemOneClient;
use runtime::{
    classify_model_tiers_judged, EffortCeiling, ImplRung, ModelCapability, ModelTier, TierSource, TiersProvenance,
};
use zerocode_core::jev::door::JevSettings;
use zerocode_core::jev::JevMode;

use super::super::jev_mock::Mock;
use super::*;

/// The workspace the door consents, at a door that reads no machine's
/// settings.
const WORKSPACE: &str = "/work/zo";
const NOW_MS: i64 = 1_791_680_000_000;

fn door(home: &Path) -> JevDoor {
    door_as(home, zerocode_core::jev::DEFAULT_MODEL)
}

fn door_as(home: &Path, backend_identity: &str) -> JevDoor {
    let settings = JevSettings {
        enabled: true,
        workspaces: vec![WORKSPACE.to_string()],
        daily_requests: None,
        model: backend_identity.to_string(),
    };
    JevDoor::at(settings, Path::new(WORKSPACE), home)
}

fn model(id: &str, tiers: [ModelTier; 2], output_micro_usd: u64) -> ModelDescriptor {
    model_of("p", id, tiers, output_micro_usd)
}

fn model_of(provider: &str, id: &str, tiers: [ModelTier; 2], output_micro_usd: u64) -> ModelDescriptor {
    ModelDescriptor::new(id, provider, id)
        .capabilities([ModelCapability::Default, ModelCapability::Coding])
        .tiers(tiers)
        .tiers_provenance(TiersProvenance::ProviderDeclared)
        .effort_ceiling(EffortCeiling::Max)
        .release_rank(1)
        .list_price_micro_usd(output_micro_usd / 5, output_micro_usd)
}

const FRONTIER: [ModelTier; 2] = [ModelTier::Deep, ModelTier::Strong];
const BALANCED: [ModelTier; 2] = [ModelTier::Balanced, ModelTier::Strong];

/// A response scoring the state's models in its order, each with `0.8`
/// confidence.
fn reply(scores: &[f64]) -> String {
    let answers: serde_json::Map<String, serde_json::Value> = scores
        .iter()
        .enumerate()
        .map(|(index, score)| {
            (
                runtime::tier_question_id(index),
                serde_json::json!({
                    "type": "score", "score": score, "confidence": 0.8,
                    "legend": {"0": "a", "1": "b", "2": "c", "3": "d"},
                    "probabilities": {"0": 0.25, "1": 0.25, "2": 0.25, "3": 0.25},
                }),
            )
        })
        .collect();
    serde_json::json!({
        "model": "jev-test",
        "answers": answers,
        "usage": {"input_tokens": 400, "output_tokens": 20},
    })
    .to_string()
}

fn no_record(_: &str) -> Option<TierOutcomes> {
    None
}

struct Rig {
    home: tempfile::TempDir,
    kept: std::path::PathBuf,
}

impl Rig {
    fn new() -> Self {
        let home = tempfile::tempdir().expect("a config home");
        let kept = tier_judgment_path_for(home.path(), zerocode_core::jev::DEFAULT_MODEL);
        Self { home, kept }
    }

    fn pass(&self, mock: Option<&Mock>, mode: JevMode, lineup: &[&ModelDescriptor]) -> TierSeatPass {
        self.pass_at(mock, mode, lineup, NOW_MS)
    }

    fn pass_as(&self, mock: &Mock, backend_identity: &str, lineup: &[&ModelDescriptor]) -> TierSeatPass {
        let lineups = [("p".to_string(), lineup.to_vec())];
        let client = SystemOneClient::new(&mock.base_url, "test-key");
        let door = door_as(self.home.path(), backend_identity);
        let kept_at = tier_judgment_path_for(self.home.path(), backend_identity);
        let ask = TierAsk {
            door: &door,
            client: Some(&client),
            mode: JevMode::On,
            kept_at: &kept_at,
            now_ms: NOW_MS,
            wall: TIER_JUDGMENT_DEADLINE,
        };
        api::sync_bridge::run_blocking(judge_lineups(&ask, &lineups, &no_record))
    }

    /// One provider's lineup asked at `now_ms`.
    fn pass_at(&self, mock: Option<&Mock>, mode: JevMode, lineup: &[&ModelDescriptor], now_ms: i64) -> TierSeatPass {
        let lineups = [("p".to_string(), lineup.to_vec())];
        self.pass_lineups(mock, mode, &lineups, now_ms, TIER_JUDGMENT_DEADLINE)
    }

    /// Every lineup of `lineups` asked at `now_ms`, each question held to
    /// `wall`.
    fn pass_lineups(
        &self,
        mock: Option<&Mock>,
        mode: JevMode,
        lineups: &[Lineup<'_>],
        now_ms: i64,
        wall: Duration,
    ) -> TierSeatPass {
        let client = mock.map(|mock| SystemOneClient::new(&mock.base_url, "test-key"));
        let door = door(self.home.path());
        let ask = TierAsk { door: &door, client: client.as_ref(), mode, kept_at: &self.kept, now_ms, wall };
        api::sync_bridge::run_blocking(judge_lineups(&ask, lineups, &no_record))
    }
}

fn backend_identity(base_url: &str, model: &str) -> String {
    judgment_provider::Provider::from_root(&serde_json::json!({
        "smart": {
            "judgmentProvider": {
                "kind": "local", "protocol": "system_one", "baseUrl": base_url,
                "model": model, "requiresAuth": false,
            }
        }
    }))
    .expect("a valid local judgment backend")
    .identity()
}

#[test]
fn backend_switch_does_not_reuse_answers_or_cross_backend_fallbacks() {
    let answering = Mock::serving(200, reply(&[3.0, 1.0]));
    let refusing = Mock::serving(402, "{}".to_string());
    let original = backend_identity(&answering.base_url, "decider");
    let alternate = backend_identity(&refusing.base_url, "decider");
    let rig = Rig::new();
    let top = model("top", FRONTIER, 50_000_000);
    let balanced = model("balanced", BALANCED, 25_000_000);
    let lineup = [&top, &balanced];

    let first = rig.pass_as(&answering, &original, &lineup);
    assert_eq!(first.judgment.places.len(), 2);
    let same = rig.pass_as(&answering, &original, &lineup);
    assert!(same.rows.is_empty());
    assert_eq!(same.judgment, first.judgment);
    assert_eq!(answering.requests().len(), 1);

    let changed = rig.pass_as(&refusing, &alternate, &lineup);
    assert!(changed.judgment.places.is_empty(), "another backend's answer is not a fallback");
    assert_eq!(changed.rows.len(), 1);
    assert!(!changed.rows[0].applied);
    assert_eq!(refusing.requests().len(), 1);
    let refused_again = rig.pass_as(&refusing, &alternate, &lineup);
    assert!(refused_again.rows.is_empty());
    assert!(refused_again.judgment.places.is_empty());
    assert_eq!(refusing.requests().len(), 1);

    let restored = rig.pass_as(&answering, &original, &lineup);
    assert!(restored.rows.is_empty());
    assert_eq!(restored.judgment, first.judgment);
    assert_eq!(answering.requests().len(), 1);
}

#[test]
fn backend_switch_does_not_reuse_refusals_or_same_server_model_answers() {
    let refusing = Mock::serving(402, "{}".to_string());
    let answering = Mock::serving(200, reply(&[3.0, 2.0]));
    let original = backend_identity(&refusing.base_url, "decider");
    let alternate = backend_identity(&answering.base_url, "decider");
    let renamed = backend_identity(&answering.base_url, "another-decider");
    let rig = Rig::new();
    let top = model("top", FRONTIER, 50_000_000);
    let balanced = model("balanced", BALANCED, 25_000_000);
    let lineup = [&top, &balanced];

    let refused = rig.pass_as(&refusing, &original, &lineup);
    assert_eq!(refused.rows.len(), 1);
    assert!(refused.judgment.places.is_empty());
    assert!(rig.pass_as(&refusing, &original, &lineup).rows.is_empty());
    assert_eq!(refusing.requests().len(), 1);

    let changed = rig.pass_as(&answering, &alternate, &lineup);
    assert_eq!(changed.judgment.places.len(), 2);
    assert_eq!(answering.requests().len(), 1, "another backend's refusal does not suppress this request");
    assert!(rig.pass_as(&answering, &alternate, &lineup).rows.is_empty());
    let new_model = rig.pass_as(&answering, &renamed, &lineup);
    assert_eq!(new_model.judgment.places.len(), 2);
    assert_eq!(answering.requests().len(), 2, "a changed model at the same server is asked separately");
    assert!(rig.pass_as(&answering, &renamed, &lineup).rows.is_empty());
    assert_eq!(answering.requests().len(), 2);
}

#[test]
fn backend_identity_is_checked_inside_the_kept_file_as_well_as_its_path() {
    let answering = Mock::serving(200, reply(&[3.0, 2.0]));
    let refusing = Mock::serving(402, "{}".to_string());
    let original = backend_identity(&answering.base_url, "decider");
    let alternate = backend_identity(&refusing.base_url, "decider");
    let rig = Rig::new();
    let top = model("top", FRONTIER, 50_000_000);
    let balanced = model("balanced", BALANCED, 25_000_000);
    let lineup = [&top, &balanced];
    assert_eq!(rig.pass_as(&answering, &original, &lineup).judgment.places.len(), 2);

    let original_path = tier_judgment_path_for(rig.home.path(), &original);
    let alternate_path = tier_judgment_path_for(rig.home.path(), &alternate);
    std::fs::create_dir_all(alternate_path.parent().expect("a backend cache directory")).expect("create the directory");
    std::fs::copy(&original_path, &alternate_path).expect("put the wrong backend's answer at the selected path");
    let changed = rig.pass_as(&refusing, &alternate, &lineup);
    assert!(changed.judgment.places.is_empty());
    assert_eq!(refusing.requests().len(), 1, "a mismatched answer is ignored");

    std::fs::copy(&alternate_path, &original_path).expect("put the wrong backend's refusal at the selected path");
    let restored = rig.pass_as(&answering, &original, &lineup);
    assert_eq!(restored.judgment.places.len(), 2);
    assert_eq!(answering.requests().len(), 2, "a mismatched refusal is ignored");

    let mut legacy: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&original_path).unwrap()).unwrap();
    legacy.as_object_mut().unwrap().remove("backendIdentity");
    std::fs::write(&original_path, serde_json::to_vec(&legacy).unwrap()).unwrap();
    assert!(read_kept(&original_path, &original).providers.is_empty(), "unattributed old answers are not adopted");
    assert!(read_kept(&alternate_path, "").unanswered.is_empty(), "invalid settings inherit no refusal memory");
}

#[test]
fn backend_cache_does_not_bypass_a_withdrawn_door() {
    let answering = Mock::serving(200, reply(&[3.0, 2.0]));
    let selected = backend_identity(&answering.base_url, "decider");
    let rig = Rig::new();
    let top = model("top", FRONTIER, 50_000_000);
    let balanced = model("balanced", BALANCED, 25_000_000);
    let lineup = [&top, &balanced];
    assert_eq!(rig.pass_as(&answering, &selected, &lineup).judgment.places.len(), 2);
    let kept_at = tier_judgment_path_for(rig.home.path(), &selected);
    let lineups = [("p".to_string(), lineup.to_vec())];

    for (enabled, consented) in [(false, true), (true, false)] {
        let settings = JevSettings {
            enabled,
            workspaces: if consented { vec![WORKSPACE.to_string()] } else { Vec::new() },
            daily_requests: None,
            model: selected.clone(),
        };
        let door = JevDoor::at(settings, Path::new(WORKSPACE), rig.home.path());
        let ask = TierAsk {
            door: &door,
            client: None,
            mode: JevMode::On,
            kept_at: &kept_at,
            now_ms: NOW_MS,
            wall: TIER_JUDGMENT_DEADLINE,
        };
        let pass = api::sync_bridge::run_blocking(judge_lineups(&ask, &lineups, &no_record));
        assert!(pass.judgment.places.is_empty(), "stored answers cannot bypass current consent or enablement");
        assert!(pass.rows.iter().all(|row| !row.applied));
    }
    assert_eq!(answering.requests().len(), 1);
}

/// One lineup is asked once; the same facts are answered from the kept file
/// with nothing sent; a model arriving asks again. The judged order is the
/// one the classifier applies: the facts put the dearer `b` first, the
/// answer puts `c` first, and `c` implements the hard work.
#[test]
fn a_lineup_is_asked_once_and_kept_until_its_facts_move() {
    let mock = Mock::serving(200, reply(&[3.0, 1.0, 2.0]));
    let rig = Rig::new();
    let (top, dear, cheap) = (model("a", FRONTIER, 50_000_000), model("b", BALANCED, 25_000_000), model("c", BALANCED, 15_000_000));

    let first = rig.pass(Some(&mock), JevMode::On, &[&top, &dear, &cheap]);
    assert_eq!(mock.requests().len(), 1, "one request for the lineup");
    assert_eq!(first.rows.len(), 1);
    assert_eq!(first.rows[0].outcome, OUTCOME_ANSWERED, "{:?}", first.rows[0]);
    assert!(first.rows[0].applied);
    assert_eq!(first.rows[0].judged, 3);
    assert!(rig.kept.exists(), "the answer is kept beside the catalog cache");
    let body: serde_json::Value = serde_json::from_str(&mock.requests()[0]).expect("a JSON body");
    assert_eq!(body["state"]["models"][1]["id"], "b");
    assert!(body["questions"][runtime::tier_question_id(2)]["instructions"].as_str().unwrap().contains("models[2]"));

    let models = [top.clone(), dear.clone(), cheap.clone()];
    let tiers = classify_model_tiers_judged(&models, Some(&first.judgment));
    let of = |id: &str| tiers.iter().find(|entry| entry.id == id).expect("classified");
    assert_eq!(of("c").rungs.iter().copied().collect::<Vec<_>>(), vec![ImplRung::Hard], "{tiers:?}");
    assert_eq!(of("c").source, TierSource::Judged);
    assert_eq!(of("c").confidence_permille, Some(800));

    let again = rig.pass(Some(&mock), JevMode::On, &[&top, &dear, &cheap]);
    assert_eq!(mock.requests().len(), 1, "the same facts send nothing");
    assert!(again.rows.is_empty());
    assert_eq!(again.judgment, first.judgment);

    let late = model("d", BALANCED, 20_000_000);
    let moved = rig.pass(Some(&mock), JevMode::On, &[&top, &dear, &late, &cheap]);
    assert_eq!(mock.requests().len(), 2, "a model arrived: the lineup is asked again");
    assert_eq!(moved.rows.len(), 1);
}

/// Under `shadow` the lineup is asked and kept and nothing is applied.
#[test]
fn under_shadow_the_lineup_is_asked_and_kept_and_nothing_applies() {
    let mock = Mock::serving(200, reply(&[3.0, 2.0]));
    let rig = Rig::new();
    let (top, dear) = (model("a", FRONTIER, 50_000_000), model("b", BALANCED, 25_000_000));
    let pass = rig.pass(Some(&mock), JevMode::Shadow, &[&top, &dear]);
    assert_eq!(mock.requests().len(), 1);
    assert!(pass.judgment.places.is_empty(), "shadow applies nothing");
    assert!(!pass.rows[0].applied);
    assert!(rig.kept.exists());
}

/// A lineup asked and left unanswered keeps its last judgment, and a model
/// that arrived since stands provisionally by the facts.
#[test]
fn an_unanswered_lineup_keeps_its_last_judgment_and_a_new_model_is_provisional() {
    let answering = Mock::serving(200, reply(&[3.0, 1.0, 2.0]));
    let rig = Rig::new();
    let (top, dear, cheap) = (model("a", FRONTIER, 50_000_000), model("b", BALANCED, 25_000_000), model("c", BALANCED, 15_000_000));
    let first = rig.pass(Some(&answering), JevMode::On, &[&top, &dear, &cheap]);
    assert_eq!(first.judgment.places.len(), 3);

    let failing = Mock::serving(500, "{}".to_string());
    let late = model("d", BALANCED, 20_000_000);
    let pass = rig.pass(Some(&failing), JevMode::On, &[&top, &dear, &late, &cheap]);
    assert_ne!(pass.rows[0].outcome, OUTCOME_ANSWERED, "{:?}", pass.rows[0]);
    assert!(pass.rows[0].applied, "the last judgment is what stands");
    assert_eq!(pass.judgment, first.judgment);
    let models = [top, dear, cheap, late];
    let tiers = classify_model_tiers_judged(&models, Some(&pass.judgment));
    let new = tiers.iter().find(|entry| entry.id == "d").expect("classified");
    assert_eq!(new.source, TierSource::Provisional, "{tiers:?}");
    assert_eq!(new.confidence_permille, None);
}

/// `off` sends nothing and applies nothing; without a key the door refuses
/// and the fact rule stands.
#[test]
fn off_sends_nothing_and_without_a_key_the_fact_rule_stands() {
    let mock = Mock::serving(200, reply(&[3.0, 2.0]));
    let rig = Rig::new();
    let (top, dear) = (model("a", FRONTIER, 50_000_000), model("b", BALANCED, 25_000_000));
    let off = rig.pass(Some(&mock), JevMode::Off, &[&top, &dear]);
    assert!(off.rows.is_empty() && off.judgment.places.is_empty());
    assert!(mock.requests().is_empty(), "off sends nothing");

    let keyless = rig.pass(None, JevMode::On, &[&top, &dear]);
    assert!(keyless.judgment.places.is_empty());
    assert_eq!(keyless.rows[0].outcome, zerocode_core::jev::door::Refused::NoKey.token());
    assert!(mock.requests().is_empty());
}

/// A lineup whose question went out and came back unanswered — here refused
/// with HTTP 402, the answer this machine's account gets — is not asked
/// again for the same facts until the memory ends: the next pass sends
/// nothing, writes no row and leaves the fact rule standing (t-45961: every
/// `zo models` call asked every provider again). New facts are a new
/// question, memory or not.
#[test]
fn a_refused_lineup_is_not_asked_again_until_its_memory_ends() {
    let refusing = Mock::serving(402, "{\"error\":\"refused\"}".to_string());
    let rig = Rig::new();
    let (top, dear) = (model("a", FRONTIER, 50_000_000), model("b", BALANCED, 25_000_000));

    let first = rig.pass(Some(&refusing), JevMode::On, &[&top, &dear]);
    assert_eq!(refusing.requests().len(), 1, "the lineup is asked once");
    assert_eq!(first.rows.len(), 1);
    assert_ne!(first.rows[0].outcome, OUTCOME_ANSWERED, "{:?}", first.rows[0]);
    assert!(first.judgment.places.is_empty(), "nothing judged: the facts stand");

    let again = rig.pass_at(Some(&refusing), JevMode::On, &[&top, &dear], NOW_MS + 1);
    assert_eq!(refusing.requests().len(), 1, "the same facts, refused a moment ago: nothing is sent");
    assert!(again.rows.is_empty(), "nothing asked, no row: {:?}", again.rows);
    assert!(again.judgment.places.is_empty());

    let later = rig.pass_at(Some(&refusing), JevMode::On, &[&top, &dear], NOW_MS + TIER_REFUSAL_MEMORY_MS);
    assert_eq!(refusing.requests().len(), 2, "the memory ended: the lineup is asked again");
    assert_eq!(later.rows.len(), 1);

    let late = model("d", BALANCED, 20_000_000);
    let moved = rig.pass_at(Some(&refusing), JevMode::On, &[&top, &dear, &late], NOW_MS + TIER_REFUSAL_MEMORY_MS + 1);
    assert_eq!(refusing.requests().len(), 3, "a model arrived: new facts are asked inside the memory");
    assert_eq!(moved.rows.len(), 1);
}

/// Every lineup due a question is asked at once, each under the one wall, so
/// a service that never answers holds a pass for one wall and not for one
/// wall per provider (t-45961: three providers asked in turn held `zo models`
/// three walls); and the next pass asks none of them again.
#[test]
fn the_lineups_due_a_question_are_asked_together_under_one_wall() {
    let silent = Mock::silent();
    let rig = Rig::new();
    let wall = TIER_JUDGMENT_DEADLINE / 4;
    let owned: Vec<(String, [ModelDescriptor; 2])> = ["p1", "p2", "p3"]
        .into_iter()
        .map(|provider| {
            let top = model_of(provider, &format!("{provider}-top"), FRONTIER, 50_000_000);
            let dear = model_of(provider, &format!("{provider}-dear"), BALANCED, 25_000_000);
            (provider.to_string(), [top, dear])
        })
        .collect();
    let lineups: Vec<Lineup<'_>> = owned.iter().map(|(provider, models)| (provider.clone(), models.iter().collect())).collect();

    let started = Instant::now();
    let first = rig.pass_lineups(Some(&silent), JevMode::On, &lineups, NOW_MS, wall);
    let took = started.elapsed();
    assert_eq!(first.rows.len(), 3, "{:?}", first.rows);
    assert!(first.rows.iter().all(|row| row.outcome != OUTCOME_ANSWERED), "{:?}", first.rows);
    assert!(took < wall * 2, "three lineups held the pass {took:?}, more than one wall of {wall:?}");

    let started = Instant::now();
    let second = rig.pass_lineups(Some(&silent), JevMode::On, &lineups, NOW_MS + 1, wall);
    assert!(second.rows.is_empty(), "nothing is asked again: {:?}", second.rows);
    assert!(started.elapsed() < wall, "the second pass waits on nothing");
}

/// A tier row says its moment under `at`, as every other seat's row does; the
/// rows written before said it under `atMs`, the camelCase of its field. Both
/// are the same moment in the same unit, milliseconds since the epoch: both
/// read back as the row they were, and both count in the window the moment
/// falls in (t-55423).
#[test]
fn a_model_tier_row_written_with_at_ms_counts_in_its_window() {
    use zerocode_core::jev::summary::{summarize, AT};

    let row = TierRow {
        at_ms: NOW_MS,
        provider: "p1".to_string(),
        fingerprint: "f1".to_string(),
        models: 2,
        outcome: OUTCOME_ANSWERED.to_string(),
        judged: 2,
        applied: true,
        elapsed_ms: 5,
        retries: 0,
        requests: 1,
        model: None,
        input_tokens: Some(10),
        server_said: crate::misc_tools::smart_router::jev_gate::ServerSaid::default(),
    };
    let new = serde_json::to_value(&row).expect("a tier row serializes");
    assert_eq!(new.get(AT.canonical), Some(&serde_json::json!(NOW_MS)), "a new row says `at`: {new}");
    let mut old = new.clone();
    if let Some(fields) = old.as_object_mut() {
        let at = fields.remove(AT.canonical).unwrap_or_default();
        fields.insert("atMs".to_string(), at);
    }
    for (written, value) in [("new", &new), ("old", &old)] {
        assert_eq!(serde_json::from_value::<TierRow>(value.clone()).ok().as_ref(), Some(&row), "the {written} row reads back: {value}");
        let counted = |since_ms| summarize(std::slice::from_ref(value), since_ms).rows;
        assert_eq!(counted(NOW_MS), 1, "the {written} row's moment is inside a window that starts there: {value}");
        assert_eq!(counted(NOW_MS + 1), 0, "a window that starts after the {written} row leaves it out");
    }
}
