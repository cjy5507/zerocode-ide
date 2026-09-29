//! The live lineup a summons picks from (t-14437).
//!
//! The launch table (`summon-profiles.json`) named one model per difficulty,
//! and nothing in it moved when a provider shipped a model: a new release
//! reached a worker only when somebody typed `--model` by hand. This module
//! reads the lineup the window already reads for the model chip (`zo models
//! --json`, one road, each row carrying the band and rungs zo's own tier
//! classifier gave it and the efforts the model accepts) and turns it into
//! choices: for every difficulty, the model and effort that run, where that
//! came from, and what else today's lineup offers there.
//!
//! - A difficulty's default is the lineup's model for its rung (`easy`,
//!   `medium`, `hard`), at the ladder's effort brought inside what that model
//!   accepts. The shipped table is only the road back when no lineup was read
//!   or it has no model for the rung, and a row says which road it took.
//! - A row the person wrote is theirs and never moves; a saved row equal to
//!   the shipped one is the shipped one.
//! - A model without a band is offered only at the lowest difficulty: nothing
//!   says it can carry more. A folded model — unlisted, or an older release
//!   of its own line — is nobody's choice.
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{LADDER, PROFILES_SETTING, Profile};

/// How long a model counts as newly arrived after this window first saw it.
pub const NEW_FOR_MS: i64 = 7 * 24 * 60 * 60 * 1000;
/// The effort words, lowest first — the order a clamp walks down.
pub const EFFORT_SCALE: [&str; 6] = ["low", "medium", "high", "xhigh", "max", "ultra"];

/// Where a model stands within its provider, as zo's tier classifier
/// (`runtime::model_router::tiering`) put it on the `zo models --json` row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Band {
    Top,
    Second,
    Rest,
    Superseded,
}

/// The implementation rung a model serves, from the same classifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Rung {
    Easy,
    Medium,
    Hard,
}

/// One row of today's lineup for one agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveModel {
    pub id: String,
    /// `None` when the row carries no band — an older zo, or a model the
    /// classifier left out.
    pub band: Option<Band>,
    pub rungs: BTreeSet<Rung>,
    /// The efforts the model accepts, lowest first; `None` when unknown.
    pub efforts: Option<Vec<String>>,
    /// Shipped in the binary, as opposed to discovered after it was built.
    pub builtin: bool,
    /// The source stopped listing it; kept only because a session had it
    /// selected.
    pub unlisted: bool,
}

impl LiveModel {
    /// Out of every choice: unlisted, or an older release of its own line.
    #[must_use]
    pub fn folded(&self) -> bool {
        let _ = self;
        false
    }

    /// The difficulties this model is offered at.
    #[must_use]
    pub fn difficulties(&self) -> BTreeSet<&'static str> {
        BTreeSet::new()
    }
}

/// Today's lineup for one agent, with the aliases its CLI resolves.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Lineup {
    pub models: Vec<LiveModel>,
    pub aliases: BTreeMap<String, String>,
}

impl Lineup {
    /// The rows of `provider` in a `zo models --json` answer; `None` when
    /// the answer lists none (no zo, or a provider it does not serve).
    #[must_use]
    pub fn from_catalog(_catalog: &Value, _provider: &str) -> Option<Self> {
        None
    }
}

/// Every model this window has seen in an agent's lineup, and when first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seen {
    #[serde(default)]
    pub agents: BTreeMap<String, BTreeMap<String, SeenModel>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeenModel {
    /// Zero for a model the binary shipped with when the window first looked.
    pub first_ms: i64,
    pub folded: bool,
}

/// What one look at a lineup changed, for the one-line notice.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Change {
    pub entered: Vec<String>,
    pub folded: Vec<String>,
}

impl Change {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entered.is_empty() && self.folded.is_empty()
    }
}

impl Seen {
    /// Record `lineup` for `agent` and say what arrived and what folded.
    pub fn observe(&mut self, _agent: &str, _lineup: &Lineup, _now_ms: i64) -> Change {
        Change::default()
    }

    /// Whether `model` arrived within [`NEW_FOR_MS`] of `now_ms`.
    #[must_use]
    pub fn fresh(&self, _agent: &str, _model: &str, _now_ms: i64) -> bool {
        false
    }
}

/// Where a row's model came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// The person wrote it in settings.
    Person,
    /// Today's lineup, by the model's rung.
    Lineup,
    /// The shipped table: no lineup was read, or it had no model here.
    Table,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Clamp {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub model: String,
    pub effort: String,
    pub fresh: bool,
}

/// One difficulty of one agent: what runs, where it came from, and what else
/// today's lineup offers there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    pub difficulty: &'static str,
    pub model: String,
    pub effort: String,
    pub from: Source,
    /// The ladder's effort, brought inside what the model accepts; for a
    /// person's row the value stands and this only says it is outside.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort_clamped: Option<Clamp>,
    pub candidates: Vec<Candidate>,
}

impl Row {
    #[must_use]
    pub fn profile(&self) -> Profile {
        Profile {
            model: self.model.clone(),
            effort: self.effort.clone(),
        }
    }
}

/// Every difficulty `agent` has a row at, read against today's lineup.
///
/// # Errors
/// A person's row that cannot launch (the refusal [`super::profile`] gives).
pub fn rows(
    root: &Value,
    agent: &str,
    lineup: Option<&Lineup>,
    seen: Option<&Seen>,
    now_ms: i64,
) -> Result<Vec<Row>, String> {
    let defaults: Value =
        serde_json::from_str(super::DEFAULT_PROFILES).map_err(|e| e.to_string())?;
    rows_with_defaults(root, &defaults, agent, lineup, seen, now_ms)
}

/// [`rows`] over a given shipped table.
///
/// # Errors
/// As [`rows`].
pub fn rows_with_defaults(
    _root: &Value,
    _defaults: &Value,
    _agent: &str,
    _lineup: Option<&Lineup>,
    _seen: Option<&Seen>,
    _now_ms: i64,
) -> Result<Vec<Row>, String> {
    let _ = PROFILES_SETTING;
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NOW: i64 = 1_790_000_000_000;
    const LOW: &str = LADDER[0].0;
    const MID: &str = LADDER[1].0;
    const HIGH: &str = LADDER[2].0;

    /// One synthetic `zo models --json` row.
    struct M {
        id: &'static str,
        band: Option<&'static str>,
        rungs: &'static [&'static str],
        efforts: Option<&'static [&'static str]>,
        builtin: bool,
        unlisted: bool,
    }

    const fn m(id: &'static str, band: &'static str, rungs: &'static [&'static str]) -> M {
        M {
            id,
            band: Some(band),
            rungs,
            efforts: None,
            builtin: false,
            unlisted: false,
        }
    }

    fn catalog(rows: &[M]) -> Value {
        json!({
            "models": rows.iter().map(|row| {
                let mut out = json!({
                    "provider": "claude", "id": row.id, "builtin": row.builtin,
                    "discovered": !row.builtin,
                    "unlistedSince": row.unlisted.then_some(1),
                });
                if let Some(band) = row.band {
                    out["band"] = json!(band);
                    out["rungs"] = json!(row.rungs);
                }
                if let Some(efforts) = row.efforts {
                    out["efforts"] = json!(efforts);
                }
                out
            }).chain([json!({"provider": "openai", "id": "model-other", "band": "top"})])
            .collect::<Vec<_>>(),
            "aliases": [{"alias": "model-alias", "canonical": "model-a", "provider": "anthropic"}],
        })
    }

    fn lineup(rows: &[M]) -> Lineup {
        let read = Lineup::from_catalog(&catalog(rows), "claude");
        assert!(
            read.is_some() || rows.is_empty(),
            "the claude rows are a lineup"
        );
        read.unwrap_or_default()
    }

    fn settings(profiles: Value) -> Value {
        json!({crate::jev::SMART_SETTINGS_KEY: {PROFILES_SETTING: profiles}})
    }

    fn shipped() -> Value {
        json!({"claude": {
            LOW: {"model": "model-old", "effort": "medium"},
            MID: {"model": "model-alias", "effort": "high"},
            HIGH: {"model": "model-old", "effort": "max"},
        }})
    }

    fn row<'a>(rows: &'a [Row], difficulty: &str) -> &'a Row {
        let found = rows.iter().find(|row| row.difficulty == difficulty);
        assert!(found.is_some(), "no {difficulty} row in {rows:?}");
        found.unwrap()
    }

    fn offered(rows: &[Row], model: &str) -> Vec<&'static str> {
        rows.iter()
            .filter(|row| row.candidates.iter().any(|c| c.model == model))
            .map(|row| row.difficulty)
            .collect()
    }

    /// The difficulties `model` runs at or is offered at.
    fn reaches(rows: &[Row], model: &str) -> Vec<&'static str> {
        rows.iter()
            .filter(|row| row.model == model || row.candidates.iter().any(|c| c.model == model))
            .map(|row| row.difficulty)
            .collect()
    }

    fn read(root: &Value, lineup: Option<&Lineup>, seen: Option<&Seen>) -> Vec<Row> {
        rows_with_defaults(root, &shipped(), "claude", lineup, seen, NOW).unwrap()
    }

    #[test]
    fn the_lineup_reads_only_its_providers_rows_and_the_aliases() {
        let held = lineup(&[m("model-a", "second", &["hard"])]);
        assert_eq!(held.models.len(), 1, "the other provider's row stays out");
        assert_eq!(held.models[0].band, Some(Band::Second));
        assert_eq!(
            held.aliases.get("model-alias").map(String::as_str),
            Some("model-a")
        );
        assert!(Lineup::from_catalog(&json!({"models": []}), "claude").is_none());
    }

    #[test]
    fn a_model_absent_yesterday_is_offered_today_and_said_once() {
        let mut shipped_a = m("model-a", "second", &["hard"]);
        shipped_a.builtin = true;
        let yesterday = lineup(&[shipped_a]);
        let mut seen = Seen::default();
        let first = seen.observe("claude", &yesterday, NOW);
        assert!(first.is_empty(), "a shipped model is not news: {first:?}");
        let mut again_a = m("model-a", "second", &["hard"]);
        again_a.builtin = true;
        let today = lineup(&[again_a, m("model-b", "rest", &["easy", "medium"])]);
        let change = seen.observe("claude", &today, NOW + 1);
        assert_eq!(change.entered, ["model-b"], "the new model is announced");
        assert!(seen.observe("claude", &today, NOW + 2).is_empty(), "once");
        assert!(seen.fresh("claude", "model-b", NOW + 2), "marked new");
        assert!(
            !seen.fresh("claude", "model-b", NOW + 2 + NEW_FOR_MS),
            "for a while"
        );
        assert!(
            !seen.fresh("claude", "model-a", NOW + 2),
            "shipped is not new"
        );
        let rows = read(&Value::Null, Some(&today), Some(&seen));
        assert_eq!(row(&rows, LOW).model, "model-b", "the easy rung runs low");
        assert_eq!(row(&rows, LOW).from, Source::Lineup);
        assert_eq!(row(&rows, MID).model, "model-b");
        assert_eq!(row(&rows, HIGH).model, "model-a", "the hard rung runs high");
        let mut top = today.clone();
        top.models.push(m("model-t", "top", &[]).into_live());
        let rows = read(&Value::Null, Some(&top), Some(&seen));
        assert_eq!(
            row(&rows, HIGH).model,
            "model-a",
            "the top band plans, never the default"
        );
        assert_eq!(offered(&rows, "model-t"), [HIGH], "but is offered at high");
        assert!(
            offered(&rows, "model-a").is_empty(),
            "a row's own model is no candidate"
        );
    }

    #[test]
    fn a_row_the_person_wrote_never_moves() {
        let mut person = shipped()["claude"].clone();
        person[LOW] = json!({"model": "model-gone", "effort": "high"});
        let root = settings(json!({"claude": person}));
        let today = lineup(&[
            m("model-b", "rest", &["easy", "medium"]),
            m("model-a", "second", &["hard"]),
        ]);
        let rows = read(&root, Some(&today), None);
        let written = row(&rows, LOW);
        assert_eq!(written.from, Source::Person, "the person's row");
        assert_eq!(
            written.model, "model-gone",
            "kept though its model left the lineup"
        );
        assert_eq!(written.effort, "high");
        // A saved row equal to the shipped one is the shipped one: it follows
        // the lineup like any default.
        assert_eq!(row(&rows, MID).from, Source::Lineup);
        assert_eq!(row(&rows, MID).model, "model-b");
    }

    #[test]
    fn a_model_without_a_band_is_offered_only_at_the_lowest_difficulty() {
        let today = lineup(&[M {
            band: None,
            ..m("model-c", "rest", &[])
        }]);
        let rows = read(&Value::Null, Some(&today), None);
        assert_eq!(reaches(&rows, "model-c"), [LOW]);
    }

    #[test]
    fn a_folded_model_leaves_the_default_row() {
        let mut old = m("model-old", "superseded", &[]);
        old.builtin = true;
        let mut gone = m("model-d", "rest", &["easy"]);
        gone.unlisted = true;
        let today = lineup(&[old, gone]);
        let rows = read(&Value::Null, Some(&today), None);
        assert!(
            rows.iter().all(|row| row.model != "model-old"),
            "a superseded table model is nobody's default: {rows:?}"
        );
        for row in &rows {
            assert!(
                row.candidates
                    .iter()
                    .all(|c| c.model != "model-old" && c.model != "model-d"),
                "folded models are nobody's choice: {row:?}"
            );
        }
        // Without any lineup the table stands, and says so.
        let rows = read(&Value::Null, None, None);
        assert_eq!(row(&rows, LOW).model, "model-old");
        assert_eq!(row(&rows, LOW).from, Source::Table);
        let mut seen = Seen::default();
        let live = lineup(&[m("model-e", "rest", &["easy"])]);
        seen.observe("claude", &live, NOW);
        let folded = lineup(&[m("model-a", "second", &["hard"])]);
        assert_eq!(seen.observe("claude", &folded, NOW + 1).folded, ["model-e"]);
    }

    #[test]
    fn the_effort_comes_down_inside_what_the_model_accepts() {
        let mut capped = m("model-a", "second", &["hard"]);
        capped.efforts = Some(&["low", "medium", "high"]);
        let rows = read(&Value::Null, Some(&lineup(&[capped])), None);
        let high = row(&rows, HIGH);
        assert_eq!(
            high.effort, "high",
            "max is not accepted; the nearest below is"
        );
        assert_eq!(
            high.effort_clamped,
            Some(Clamp {
                from: LADDER[2].2.into(),
                to: "high".into()
            })
        );
        // A person's effort stands; the row only says it is outside.
        let mut person = shipped()["claude"].clone();
        person[HIGH] = json!({"model": "model-a", "effort": "max"});
        let mut capped = m("model-a", "second", &["hard"]);
        capped.efforts = Some(&["low", "medium", "high"]);
        let root = settings(json!({"claude": person}));
        let rows = read(&root, Some(&lineup(&[capped])), None);
        assert_eq!(row(&rows, HIGH).effort, "max");
        assert!(row(&rows, HIGH).effort_clamped.is_some());
    }

    impl M {
        fn into_live(self) -> LiveModel {
            let mut held = lineup(&[self]).models;
            assert_eq!(held.len(), 1, "one row, one model");
            held.remove(0)
        }
    }
}
