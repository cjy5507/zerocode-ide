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
//!   `medium`, `hard`), at the shipped table's effort for that difficulty
//!   brought inside what the model accepts — data a person can override,
//!   never a rule in code (t-74181: a coded "easy and ordinary work at the
//!   model's highest effort" put every default row at `max`). An agent the
//!   table has no row for runs its ladder's effort. The table's model is
//!   only the road back when no lineup was read or it has no model for the
//!   rung, and a row says which road it took. Which pair a summons runs is
//!   the model seat's to judge ([`crate::summon_model`]); these rows are what
//!   runs when it abstains or does not act.
//! - A row the person wrote is theirs and never moves; a saved row equal to
//!   the shipped one is the shipped one.
//! - A model without a band is offered only at the lowest difficulty: nothing
//!   says it can carry more. A folded model — unlisted, or an older release
//!   of its own line — is nobody's choice.
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{LADDER, PROFILES_SETTING, Profile};
use crate::capabilities::EFFORT_ORDER;
use crate::jev::SMART_SETTINGS_KEY;

/// How long a model counts as newly arrived after this window first saw it.
pub const NEW_FOR_MS: i64 = 7 * 24 * 60 * 60 * 1000;
/// One summons in this many, at a difficulty that takes a turn, tries a
/// model this ledger has too little record of — how a new release earns the
/// evidence the model seat reads (`crate::summon_model`).
pub const CHALLENGE_ONE_IN: usize = 4;
/// A model with this many ended summonses here has a record; it no longer
/// takes a challenger's turn.
pub const CHALLENGE_MIN_SAMPLES: usize = 5;
/// The difficulties whose summonses take a challenger's turn: the easy and
/// the ordinary, where a model that falls short costs a retry, not a design.
pub const CHALLENGED: [&str; 2] = [LADDER[0].0, LADDER[1].0];

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

/// The implementation rung a model serves, from the same classifier — one
/// per difficulty, in the ladder's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Rung {
    Easy,
    Medium,
    Hard,
}

const RUNGS: [Rung; 3] = [Rung::Easy, Rung::Medium, Rung::Hard];

/// Who placed a model in its provider's order, as the same classifier's row
/// says (t-45961): the Jev seat's judgment of the lineup, the fact rule
/// where no judgment stands, or the facts for a model that arrived after
/// its provider's judgment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TierFrom {
    Jev,
    Facts,
    Provisional,
}

impl Rung {
    fn difficulty(self) -> &'static str {
        LADDER[RUNGS.iter().position(|rung| *rung == self).unwrap_or(0)].0
    }

    fn of(difficulty: &str) -> Option<Self> {
        LADDER
            .iter()
            .position(|(key, _, _)| *key == difficulty)
            .map(|at| RUNGS[at])
    }
}

/// One row of today's lineup for one agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveModel {
    pub id: String,
    /// The provider key its row names — what an agent of every provider
    /// (zo) tells its rows apart by.
    pub provider: String,
    /// `None` when the row carries no band — an older zo, or a model the
    /// classifier left out.
    pub band: Option<Band>,
    pub rungs: BTreeSet<Rung>,
    /// Who placed it there; `None` when the row does not say (an older zo).
    pub tier_from: Option<TierFrom>,
    /// How sure the judgment that placed it was, in thousandths; `None`
    /// when no judgment did.
    pub tier_confidence_permille: Option<u16>,
    /// The efforts the model accepts, lowest first; `None` when unknown.
    pub efforts: Option<Vec<String>>,
    /// Shipped in the binary, as opposed to discovered after it was built.
    pub builtin: bool,
    /// The source stopped listing it; kept only because a session had it
    /// selected.
    pub unlisted: bool,
    /// The newer release of its own line that stands in its place, when the
    /// classifier superseded it and said which (`supersededBy`, t-46882);
    /// `None` for a current model, a twin of the same release, and an older
    /// zo that does not say.
    pub superseded_by: Option<String>,
}

impl LiveModel {
    /// Out of every choice: unlisted, or an older release of its own line.
    #[must_use]
    pub fn folded(&self) -> bool {
        self.unlisted || self.band == Some(Band::Superseded)
    }

    /// The difficulties this model is offered at: its rungs; the top band
    /// (which plans and verifies) and a second band without a rung at the
    /// highest; anything else — no band at all included — at the lowest.
    #[must_use]
    pub fn difficulties(&self) -> BTreeSet<&'static str> {
        if self.folded() {
            return BTreeSet::new();
        }
        let mut at: BTreeSet<&'static str> =
            self.rungs.iter().map(|rung| rung.difficulty()).collect();
        match self.band {
            Some(Band::Top) => {
                at.insert(LADDER[2].0);
            }
            Some(Band::Second) if at.is_empty() => {
                at.insert(LADDER[2].0);
            }
            Some(Band::Rest) | None if at.is_empty() => {
                at.insert(LADDER[0].0);
            }
            _ => {}
        }
        at
    }

    /// The highest effort the model says it accepts; `None` when unknown.
    #[must_use]
    pub fn highest_effort(&self) -> Option<&str> {
        self.efforts
            .as_ref()?
            .iter()
            .filter_map(|effort| {
                Some((
                    EFFORT_ORDER.iter().position(|known| known == effort)?,
                    effort,
                ))
            })
            .max()
            .map(|(_, effort)| effort.as_str())
    }

    /// The effort this model runs at when `wanted` is the default effort the
    /// rule `rule` named for the difficulty (the shipped table's word, or the
    /// agent's ladder), brought inside what the model accepts.
    fn effort_at(&self, wanted: &str, rule: EffortRule) -> (String, EffortRule, Option<Clamp>) {
        let clamped = self.clamp(wanted);
        (
            clamped.clone().unwrap_or_else(|| wanted.to_string()),
            rule,
            clamped.map(|to| Clamp {
                from: wanted.to_string(),
                to,
            }),
        )
    }

    /// `wanted` brought inside what the model accepts.
    #[must_use]
    pub fn effort_for(&self, wanted: &str) -> String {
        self.clamp(wanted).unwrap_or_else(|| wanted.to_string())
    }

    /// `wanted`, or the nearest effort below it the model accepts (the
    /// lowest it accepts when none is below); `None` when it stands as is.
    fn clamp(&self, wanted: &str) -> Option<String> {
        let accepted = self.efforts.as_ref()?;
        if accepted.iter().any(|effort| effort == wanted) {
            return None;
        }
        let rank = |effort: &str| EFFORT_ORDER.iter().position(|known| *known == effort);
        let wanted_rank = rank(wanted)?;
        let mut known: Vec<(usize, &String)> = accepted
            .iter()
            .filter_map(|effort| Some((rank(effort)?, effort)))
            .collect();
        known.sort();
        known
            .iter()
            .rev()
            .find(|(at, _)| *at < wanted_rank)
            .or_else(|| known.first())
            .map(|(_, effort)| (*effort).clone())
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
    pub fn from_catalog(catalog: &Value, provider: &str) -> Option<Self> {
        Self::from_rows(catalog, |row| {
            row.get("provider").and_then(Value::as_str) == Some(provider)
        })
    }

    /// Every provider's rows — the lineup of an agent that runs any
    /// provider's model (zo) — and the models of every custom provider the
    /// answer says is usable, which the person configured and so are never
    /// news; `None` when the answer lists none.
    #[must_use]
    pub fn from_catalog_all(catalog: &Value) -> Option<Self> {
        let mut all = Self::from_rows(catalog, |_| true).unwrap_or_default();
        let custom = catalog
            .get("customProviders")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|provider| provider.get("usable").and_then(Value::as_bool) == Some(true))
            .flat_map(|provider| {
                let name = provider
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                provider
                    .get("models")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(move |id| (name, id))
            })
            .map(|(provider, id)| LiveModel {
                id: id.to_string(),
                provider: provider.to_string(),
                band: None,
                rungs: BTreeSet::new(),
                tier_from: None,
                tier_confidence_permille: None,
                efforts: None,
                builtin: true,
                unlisted: false,
                superseded_by: None,
            });
        all.models.extend(custom);
        (!all.models.is_empty()).then_some(all)
    }

    fn from_rows(catalog: &Value, wanted: impl Fn(&Value) -> bool) -> Option<Self> {
        let models: Vec<LiveModel> = catalog
            .get("models")?
            .as_array()?
            .iter()
            .filter(|row| wanted(row))
            .filter_map(|row| {
                let band: Option<Band> = row
                    .get("band")
                    .and_then(|band| serde_json::from_value(band.clone()).ok());
                Some(LiveModel {
                    id: row.get("id")?.as_str()?.to_string(),
                    provider: row.get("provider")?.as_str()?.to_string(),
                    band,
                    rungs: band
                        .and_then(|_| serde_json::from_value(row.get("rungs")?.clone()).ok())
                        .unwrap_or_default(),
                    tier_from: row
                        .get("tierFrom")
                        .and_then(|from| serde_json::from_value(from.clone()).ok()),
                    tier_confidence_permille: row
                        .get("tierConfidencePermille")
                        .and_then(Value::as_u64)
                        .and_then(|permille| u16::try_from(permille).ok()),
                    efforts: row
                        .get("efforts")
                        .and_then(|efforts| {
                            serde_json::from_value::<Vec<String>>(efforts.clone()).ok()
                        })
                        .filter(|efforts| !efforts.is_empty()),
                    builtin: row.get("builtin").and_then(Value::as_bool).unwrap_or(false),
                    unlisted: row
                        .get("unlistedSince")
                        .is_some_and(|since| !since.is_null()),
                    superseded_by: row
                        .get("supersededBy")
                        .and_then(Value::as_str)
                        .filter(|successor| !successor.is_empty())
                        .map(str::to_string),
                })
            })
            .collect();
        if models.is_empty() {
            return None;
        }
        let aliases = catalog
            .get("aliases")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|row| {
                let canonical = row.get("canonical")?.as_str()?;
                models.iter().any(|model| model.id == canonical).then(|| {
                    Some((
                        row.get("alias")?.as_str()?.to_string(),
                        canonical.to_string(),
                    ))
                })?
            })
            .collect();
        Some(Self { models, aliases })
    }

    /// The id `model` names: itself, or what its alias resolves to.
    #[must_use]
    pub fn canonical<'a>(&'a self, model: &'a str) -> &'a str {
        self.aliases.get(model).map_or(model, String::as_str)
    }

    #[must_use]
    pub fn find(&self, model: &str) -> Option<&LiveModel> {
        let id = self.canonical(model);
        self.models.iter().find(|held| held.id == id)
    }

    /// Listed today and not folded.
    #[must_use]
    pub fn live(&self, model: &str) -> bool {
        self.find(model).is_some_and(|held| !held.folded())
    }

    /// The release that stands for `model` today, when the classifier
    /// superseded it (t-46882): the newer release of its own line the row
    /// names, followed while that one is superseded too, and only when the
    /// release it ends on is live here. `None` for a current model, a model
    /// this lineup does not list, a twin of the same release, and an older
    /// release whose successor this lineup does not hold — nothing is
    /// guessed.
    #[must_use]
    pub fn current_release(&self, model: &str) -> Option<&str> {
        let mut held = self.find(model)?;
        if held.band != Some(Band::Superseded) {
            return None;
        }
        // A release followed past every row the lineup holds has met itself
        // again: a catalog that says so is answered with nothing.
        for _ in 0..self.models.len() {
            held = self.find(held.superseded_by.as_deref()?)?;
            if held.band != Some(Band::Superseded) {
                return (!held.folded()).then_some(held.id.as_str());
            }
        }
        None
    }

    /// Every live model, the newly arrived marked — what a person picks a
    /// written row's model from.
    #[must_use]
    pub fn picks(&self, agent: &str, seen: Option<&Seen>, now_ms: i64) -> Vec<Pick> {
        self.models
            .iter()
            .filter(|model| !model.folded())
            .map(|model| Pick {
                id: model.id.clone(),
                fresh: seen.is_some_and(|seen| seen.fresh(agent, &model.id, now_ms)),
            })
            .collect()
    }
}

/// One model a person can pick for a written row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Pick {
    pub id: String,
    pub fresh: bool,
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
    /// The first look at an agent takes what the binary shipped as known;
    /// anything discovered since is news.
    pub fn observe(&mut self, agent: &str, lineup: &Lineup, now_ms: i64) -> Change {
        let first_look = !self.agents.contains_key(agent);
        let book = self.agents.entry(agent.to_string()).or_default();
        let mut change = Change::default();
        for model in &lineup.models {
            let folded = model.folded();
            if let Some(held) = book.get_mut(&model.id) {
                if folded && !held.folded {
                    change.folded.push(model.id.clone());
                }
                held.folded = folded;
                continue;
            }
            let known = first_look && model.builtin;
            book.insert(
                model.id.clone(),
                SeenModel {
                    first_ms: if known { 0 } else { now_ms },
                    folded,
                },
            );
            if !known && !folded {
                change.entered.push(model.id.clone());
            }
        }
        for (id, held) in book.iter_mut() {
            if !held.folded && !lineup.models.iter().any(|model| model.id == *id) {
                held.folded = true;
                change.folded.push(id.clone());
            }
        }
        change
    }

    /// Whether `model` arrived within [`NEW_FOR_MS`] of `now_ms`.
    #[must_use]
    pub fn fresh(&self, agent: &str, model: &str, now_ms: i64) -> bool {
        self.agents
            .get(agent)
            .and_then(|book| book.get(model))
            .is_some_and(|held| held.first_ms > 0 && now_ms - held.first_ms < NEW_FOR_MS)
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

/// Why a row runs at its effort — the line a person reads beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EffortRule {
    /// The agent's ladder effort for the difficulty, inside the model's
    /// range: the table has no row for this agent.
    Ladder,
    /// The person wrote it.
    Written,
    /// The shipped table's word for the difficulty, inside the model's range.
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
    /// Where the provider's classifier ranks the row's model, when the
    /// lineup lists it — half of why this model runs here.
    pub band: Option<Band>,
    /// Who placed the model in that rank (t-45961), when the lineup says.
    pub tier_from: Option<TierFrom>,
    /// How sure the judgment that placed it was, in thousandths.
    pub tier_confidence_permille: Option<u16>,
    /// And why it runs at this effort.
    pub effort_rule: EffortRule,
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
    root: &Value,
    defaults: &Value,
    agent: &str,
    lineup: Option<&Lineup>,
    seen: Option<&Seen>,
    now_ms: i64,
) -> Result<Vec<Row>, String> {
    build(root, defaults, agent, lineup, seen, now_ms, None)
}

/// The one difficulty `difficulty` of [`rows`]; a person's row at another
/// difficulty that cannot launch does not refuse this one.
///
/// # Errors
/// As [`rows`], for this difficulty's row alone.
pub fn row_at(
    root: &Value,
    agent: &str,
    difficulty: &str,
    lineup: Option<&Lineup>,
    seen: Option<&Seen>,
    now_ms: i64,
) -> Result<Option<Row>, String> {
    let defaults: Value =
        serde_json::from_str(super::DEFAULT_PROFILES).map_err(|e| e.to_string())?;
    Ok(build(
        root,
        &defaults,
        agent,
        lineup,
        seen,
        now_ms,
        Some(difficulty),
    )?
    .pop())
}

fn build(
    root: &Value,
    defaults: &Value,
    agent: &str,
    lineup: Option<&Lineup>,
    seen: Option<&Seen>,
    now_ms: i64,
    only: Option<&str>,
) -> Result<Vec<Row>, String> {
    let written = root
        .get(SMART_SETTINGS_KEY)
        .and_then(|smart| smart.get(PROFILES_SETTING))
        .and_then(|table| table.get(agent));
    let read = |table: Option<&Value>, difficulty: &str| -> Result<Option<Profile>, String> {
        table
            .and_then(|rows| rows.get(difficulty))
            .map(|row| serde_json::from_value::<Profile>(row.clone()).map_err(|e| e.to_string()))
            .transpose()
    };
    // An agent of every provider (zo) falls back to the provider of the
    // model its own settings run, when its lineup lists that model.
    let home = root
        .get("model")
        .and_then(Value::as_str)
        .and_then(|model| lineup?.find(model))
        .map(|model| model.provider.clone());
    let mut rows = Vec::new();
    for (difficulty, _, _) in LADDER {
        if only.is_some_and(|only| only != difficulty) {
            continue;
        }
        let shipped = read(defaults.get(agent), difficulty)?;
        let person = read(written, difficulty)?.filter(|row| Some(row) != shipped.as_ref());
        // The default effort for the difficulty: the shipped table's word —
        // data, overridden by a person's row — and the agent's own ladder
        // (the measured launch table's word, ceiling included) only where the
        // table has no row for this agent.
        let ladder = crate::orchestration::difficulty_effort(agent, difficulty);
        // An agent that takes no effort at all (no ladder) has none.
        let default_effort = ladder.map(|ladder| {
            shipped.as_ref().map_or_else(
                || (ladder.to_string(), EffortRule::Ladder),
                |row| (row.effort.clone(), EffortRule::Table),
            )
        });
        let (model, effort, from, effort_rule, effort_clamped) = if let Some(person) = person {
            super::launchable(agent, &person)?;
            let clamped = lineup
                .and_then(|held| held.find(&person.model))
                .and_then(|held| held.clamp(&person.effort))
                .map(|to| Clamp {
                    from: person.effort.clone(),
                    to,
                });
            (
                person.model,
                person.effort,
                Source::Person,
                EffortRule::Written,
                clamped,
            )
        } else if let Some((pick, (wanted, rule))) =
            lineup
                .zip(default_effort.clone())
                .and_then(|(held, wanted)| {
                    let rung = Rung::of(difficulty)?;
                    let by_rung = held
                        .models
                        .iter()
                        .filter(|model| !model.folded() && model.rungs.contains(&rung))
                        .min_by_key(|model| home.as_ref() != Some(&model.provider));
                    // No model for the rung: the table's model stands while it is
                    // live here; only a folded one gives way to what
                    // the lineup offers at this difficulty.
                    let table_stands = shipped.as_ref().is_some_and(|row| held.live(&row.model));
                    let pick = by_rung.or_else(|| {
                        (!table_stands)
                            .then(|| offered(held, difficulty).next())
                            .flatten()
                    })?;
                    Some((pick, wanted))
                })
        {
            let (effort, rule, clamped) = pick.effort_at(&wanted, rule);
            (pick.id.clone(), effort, Source::Lineup, rule, clamped)
        } else if let Some(table) =
            shipped.filter(|row| lineup.is_none_or(|held| held.live(&row.model)))
        {
            let clamped = lineup
                .and_then(|held| held.find(&table.model))
                .and_then(|held| held.clamp(&table.effort));
            let effort = clamped.clone().unwrap_or_else(|| table.effort.clone());
            (
                table.model,
                effort,
                Source::Table,
                EffortRule::Table,
                clamped.map(|to| Clamp {
                    from: table.effort,
                    to,
                }),
            )
        } else {
            continue;
        };
        let candidates = match (lineup, default_effort.as_ref()) {
            (Some(held), Some((wanted, rule))) => offered(held, difficulty)
                .filter(|candidate| candidate.id != held.canonical(&model))
                .map(|candidate| Candidate {
                    model: candidate.id.clone(),
                    effort: candidate.effort_at(wanted, *rule).0,
                    fresh: seen.is_some_and(|seen| seen.fresh(agent, &candidate.id, now_ms)),
                })
                .collect(),
            _ => Vec::new(),
        };
        let held = lineup.and_then(|held| held.find(&model));
        rows.push(Row {
            difficulty,
            band: held.and_then(|held| held.band),
            tier_from: held.and_then(|held| held.tier_from),
            tier_confidence_permille: held.and_then(|held| held.tier_confidence_permille),
            model,
            effort,
            from,
            effort_rule,
            effort_clamped,
            candidates,
        });
    }
    Ok(rows)
}

/// The live models `lineup` offers at `difficulty`, the ones whose rung it
/// is first.
fn offered<'a>(
    lineup: &'a Lineup,
    difficulty: &'a str,
) -> impl Iterator<Item = &'a LiveModel> + 'a {
    let rung = Rung::of(difficulty);
    let exact = move |model: &&LiveModel| rung.is_some_and(|rung| model.rungs.contains(&rung));
    let at = move |model: &&LiveModel| model.difficulties().contains(difficulty);
    lineup
        .models
        .iter()
        .filter(move |model| at(model) && exact(model))
        .chain(
            lineup
                .models
                .iter()
                .filter(move |model| at(model) && !exact(model)),
        )
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

    /// An agent that runs any provider's model (zo) takes every provider's
    /// rows and every usable custom provider's models — the person's own, so
    /// never news — and may be launched on another CLI's model.
    #[test]
    fn an_agent_of_every_provider_reads_every_row_and_the_usable_custom_ones() {
        let catalog = json!({
            "models": [
                {"provider": "claude", "id": "model-a", "builtin": true, "band": "second", "rungs": ["hard"]},
                {"provider": "openai", "id": "model-o", "builtin": false, "band": "rest", "rungs": ["easy"]},
                {"provider": "openai", "id": "model-p", "builtin": true, "band": "second", "rungs": ["hard"]},
            ],
            "customProviders": [
                {"name": "custom-a", "models": ["custom-a/model-x"], "usable": true},
                {"name": "custom-b", "models": ["custom-b/model-y"], "usable": false},
            ],
        });
        let all = Lineup::from_catalog_all(&catalog).unwrap();
        let ids: Vec<&str> = all.models.iter().map(|model| model.id.as_str()).collect();
        assert_eq!(ids, ["model-a", "model-o", "model-p", "custom-a/model-x"]);
        let mut seen = Seen::default();
        assert_eq!(
            seen.observe("zo", &all, NOW).entered,
            ["model-o"],
            "a custom model is not news"
        );
        assert!(Lineup::from_catalog_all(&json!({"models": []})).is_none());
        let rows =
            rows_with_defaults(&Value::Null, &json!({}), "zo", Some(&all), None, NOW).unwrap();
        assert_eq!(
            row(&rows, LOW).model,
            "model-o",
            "another provider's easy rung"
        );
        assert_eq!(
            row(&rows, HIGH).model,
            "model-a",
            "no settings: the lineup's order"
        );
        let runs_openai = json!({"model": "model-o"});
        let rows =
            rows_with_defaults(&runs_openai, &json!({}), "zo", Some(&all), None, NOW).unwrap();
        assert_eq!(
            row(&rows, HIGH).model,
            "model-p",
            "the provider zo's own settings run comes first"
        );
        let across = Profile {
            model: "claude-model".into(),
            effort: "high".into(),
        };
        assert!(super::super::launchable("zo", &across).is_ok());
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

    /// t-74181: a default row runs at the shipped table's effort for its
    /// difficulty — data, inside the model's range — and no rule in code
    /// raises it. Before, easy and ordinary work ran at the model's highest
    /// effort, so every default row of every agent read `max`. An agent the
    /// table has no row for runs its ladder's effort.
    #[test]
    fn a_default_row_runs_at_the_shipped_tables_effort_and_never_at_a_coded_highest() {
        let mut easy = m("model-b", "rest", &["easy", "medium"]);
        easy.efforts = Some(&["low", "medium", "high", "xhigh", "max"]);
        let mut hard = m("model-a", "second", &["hard"]);
        hard.efforts = Some(&["low", "medium", "high", "xhigh", "max"]);
        let mut other = m("model-c", "rest", &[]);
        other.efforts = Some(&["high", "low"]);
        let today = lineup(&[easy, hard, other]);
        let rows = read(&Value::Null, Some(&today), None);
        for (difficulty, effort) in [(LOW, "medium"), (MID, "high")] {
            let at = row(&rows, difficulty);
            assert_eq!(
                (at.model.as_str(), at.effort.as_str(), at.effort_rule),
                ("model-b", effort, EffortRule::Table),
                "{difficulty}"
            );
            assert_eq!(
                at.band,
                Some(Band::Rest),
                "the row says where its model stands"
            );
        }
        assert_eq!(
            row(&rows, LOW)
                .candidates
                .iter()
                .find(|c| c.model == "model-c")
                .map(|c| c.effort.as_str()),
            Some("low"),
            "a candidate at the table's effort, brought inside what it takes"
        );
        let high = row(&rows, HIGH);
        assert_eq!(
            (high.model.as_str(), high.effort.as_str(), high.effort_rule),
            ("model-a", "max", EffortRule::Table)
        );
        // Every default row would have read max under the coded rule; the
        // table's three words stand instead.
        let efforts: Vec<&str> = rows.iter().map(|row| row.effort.as_str()).collect();
        assert_eq!(efforts, ["medium", "high", "max"]);
        // An agent with no row in the table: its ladder.
        let bare = rows_with_defaults(&Value::Null, &json!({}), "claude", Some(&today), None, NOW)
            .unwrap();
        assert!(
            bare.iter().all(|row| row.effort_rule == EffortRule::Ladder),
            "{bare:?}"
        );
    }

    /// t-74181: no effort is decided by a rule written in code — the default
    /// effort is the table's word or the agent's ladder, brought inside the
    /// model's range, and nothing else in `effort_at` reads the difficulty.
    #[test]
    fn no_rule_in_code_raises_a_default_rows_effort() {
        let source = include_str!("lineup.rs");
        let body = source
            .split("fn effort_at(")
            .nth(1)
            .and_then(|rest| rest.split("\n    }\n").next())
            .expect("effort_at");
        for coded in ["CHALLENGED", "highest_effort", "difficulty", "\"max\""] {
            assert!(!body.contains(coded), "effort_at reads `{coded}`:\n{body}");
        }
        let rules = source
            .split("pub enum EffortRule {")
            .nth(1)
            .and_then(|rest| rest.split('}').next())
            .expect("EffortRule");
        assert!(
            !rules.contains("Highest"),
            "a coded highest rule is back:\n{rules}"
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

    /// t-46882: the release that stands for an older one is the newer
    /// release of its line the classifier named — through an alias, across a
    /// second release, and only onto a live one. A current model, a twin of
    /// the same release (the classifier names it nothing), a release whose
    /// successor is not listed or no longer listed, and a model the lineup
    /// does not hold have none.
    #[test]
    fn a_superseded_release_resolves_to_the_live_release_of_its_line() {
        let catalog = json!({
            "models": [
                {"provider": "claude", "id": "model-a-5", "band": "superseded", "supersededBy": "model-a-5-1"},
                {"provider": "claude", "id": "model-a-5-1", "band": "top", "rungs": []},
                {"provider": "claude", "id": "model-a-4", "band": "superseded", "supersededBy": "model-a-5"},
                {"provider": "claude", "id": "model-twin-green", "band": "superseded"},
                {"provider": "claude", "id": "model-twin-blue", "band": "rest", "rungs": ["easy"]},
                {"provider": "claude", "id": "model-b-1", "band": "superseded", "supersededBy": "model-b-2"},
                {"provider": "claude", "id": "model-c-1", "band": "superseded", "supersededBy": "model-c-2"},
                {"provider": "claude", "id": "model-c-2", "band": "rest", "rungs": ["easy"], "unlistedSince": 1},
            ],
            "aliases": [{"alias": "model-a-old", "canonical": "model-a-5", "provider": "anthropic"}],
        });
        let held = Lineup::from_catalog(&catalog, "claude").expect("a lineup");
        assert_eq!(held.current_release("model-a-5"), Some("model-a-5-1"));
        assert_eq!(
            held.current_release("model-a-old"),
            Some("model-a-5-1"),
            "through its alias"
        );
        assert_eq!(
            held.current_release("model-a-4"),
            Some("model-a-5-1"),
            "two releases on"
        );
        for none in [
            "model-a-5-1",
            "model-twin-green",
            "model-twin-blue",
            "model-b-1",
            "model-c-1",
            "model-nobody",
        ] {
            assert_eq!(held.current_release(none), None, "{none}");
        }
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

    /// An effort below `low` is ranked like every other: a model that
    /// accepts it is brought down to it, never launched at an effort it does
    /// not accept.
    #[test]
    fn an_effort_below_low_is_ranked_like_every_other() {
        let mut two = m("model-a", "second", &["hard"]);
        two.efforts = Some(&["minimal", "low"]);
        assert_eq!(two.into_live().effort_for("medium"), "low");
        let mut one = m("model-b", "second", &["hard"]);
        one.efforts = Some(&["minimal"]);
        let one = one.into_live();
        assert_eq!(
            one.effort_for("low"),
            "minimal",
            "the one effort it accepts"
        );
        assert_eq!(one.highest_effort(), Some("minimal"));
    }

    /// The measurement a person reads (t-14437 criterion 8): today's rows
    /// of every agent that takes a model at a difficulty, read against a
    /// `zo models --json` answer this machine gave — named, never fetched
    /// here.
    #[test]
    #[ignore = "live measurement: reads the zo models --json answer named by ZEROCODE_LINEUP_CATALOG"]
    fn measure_todays_rows() {
        let path = std::env::var("ZEROCODE_LINEUP_CATALOG").expect("a catalog path");
        let catalog: Value =
            serde_json::from_str(&std::fs::read_to_string(path).expect("the catalog")).unwrap();
        for spec in crate::agent::AGENT_SPECS {
            if crate::orchestration::difficulty_effort(spec.id, LOW).is_none() {
                continue;
            }
            let lineup = match crate::agent::agent_voice(spec.id).models_provider {
                Some(provider) => Lineup::from_catalog(&catalog, provider),
                None => Lineup::from_catalog_all(&catalog),
            };
            let rows = rows(&Value::Null, spec.id, lineup.as_ref(), None, NOW).unwrap();
            println!(
                "{}",
                json!({"agent": spec.id, "lineup": lineup.map(|held| held.models.len()), "rows": rows})
            );
        }
    }

    impl M {
        fn into_live(self) -> LiveModel {
            let mut held = lineup(&[self]).models;
            assert_eq!(held.len(), 1, "one row, one model");
            held.remove(0)
        }
    }
}
