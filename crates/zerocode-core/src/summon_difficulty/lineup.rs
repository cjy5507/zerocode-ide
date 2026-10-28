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
//!   `medium`, `hard`), with no effort of ours: the agent's CLI runs it at
//!   its own default unless the person wrote an effort (t-74181 — a coded
//!   "easy and ordinary work at the model's highest effort", then a coded
//!   ladder ending at `max` and a shipped table of efforts, put default rows
//!   at `max`; the person, 2026-10-28: no table, in code or in data). The
//!   table's model is only the road back when no lineup was read or it has
//!   no model for the rung, and a row says which road it took. Which pair a
//!   summons runs — the effort included — is the model seat's to judge
//!   ([`crate::summon_model`]); these rows are what runs when it abstains or
//!   does not act.
//! - A row the person wrote is theirs and never moves; a saved row equal to
//!   the shipped one is the shipped one.
//! - A model without a band is offered only at the lowest difficulty: nothing
//!   says it can carry more. A folded model — unlisted, or an older release
//!   of its own line — is nobody's choice.
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{LADDER, PROFILES_SETTING, Profile, Written};
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
            .position(|(key, _)| *key == difficulty)
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
    /// When the source first left it out, epoch seconds, when zo says.
    pub unlisted_since: Option<i64>,
    /// A model of one of the person's own connections (`customProviders`).
    pub connection: bool,
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
                unlisted_since: None,
                connection: true,
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
                    unlisted_since: row.get("unlistedSince").and_then(Value::as_i64),
                    connection: false,
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
    /// written row's model from, with what each takes and where it fits.
    #[must_use]
    pub fn picks(&self, agent: &str, seen: Option<&Seen>, now_ms: i64) -> Vec<Pick> {
        self.models
            .iter()
            .filter(|model| !model.folded())
            .map(|model| {
                let (efforts, efforts_known) = efforts_of(agent, model);
                Pick {
                    id: model.id.clone(),
                    fresh: seen.is_some_and(|seen| seen.fresh(agent, &model.id, now_ms)),
                    provider: model.provider.clone(),
                    connection: model.connection,
                    efforts,
                    efforts_known,
                    fits: model.difficulties().into_iter().collect(),
                }
            })
            .collect()
    }

    /// Where `model` stands in this lineup today.
    #[must_use]
    pub fn standing(&self, model: &str) -> Standing {
        match self.find(model) {
            None => Standing::Missing,
            Some(held) if held.unlisted => Standing::Unlisted {
                since: held.unlisted_since,
            },
            Some(held) if held.band == Some(Band::Superseded) => Standing::Superseded {
                by: held.superseded_by.clone(),
            },
            Some(_) => Standing::Listed,
        }
    }
}

/// The efforts `model` may run at for `agent`, lowest first, and whether
/// that is the model's own word: a model that names none (a connection's)
/// is offered every effort the agent's CLI takes.
fn efforts_of(agent: &str, model: &LiveModel) -> (Vec<String>, bool) {
    model.efforts.clone().map_or_else(
        || {
            (
                crate::orchestration::ladder_efforts(agent)
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
                false,
            )
        },
        |own| (own, true),
    )
}

/// One model a person can pick for a written row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Pick {
    pub id: String,
    pub fresh: bool,
    /// The provider key its row names, or the connection's name.
    pub provider: String,
    /// One of the person's own connections (`customProviders`), not a login.
    pub connection: bool,
    /// What a written effort may be for it, lowest first.
    pub efforts: Vec<String>,
    /// `efforts` is the model's own list; otherwise the agent's ladder.
    pub efforts_known: bool,
    /// The difficulties the lineup offers it at.
    pub fits: Vec<&'static str>,
}

/// Where a row's model stands in today's lineup — what a row the person
/// wrote is warned by when the provider moved under it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Standing {
    Listed,
    /// The source stopped listing it, since (epoch seconds) when zo says.
    Unlisted {
        since: Option<i64>,
    },
    /// An older release of its line; `by` is the release that stands.
    Superseded {
        by: Option<String>,
    },
    /// The lineup does not hold it at all.
    Missing,
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

/// Who set a row's effort — the line a person reads beside it. Never a rule
/// of ours: the person's word, or none, and the agent's CLI decides
/// (t-74181). The model seat's pair, when it runs, says `jev` in a summons'
/// dials instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EffortRule {
    /// The person wrote it.
    Person,
    /// Nobody chose one: no effort goes on the command line, and the agent's
    /// CLI runs at its own default.
    CliDefault,
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
    /// `None`: it would run at its CLI's default.
    pub effort: Option<String>,
    pub fresh: bool,
}

/// One difficulty of one agent: what runs, where it came from, and what else
/// today's lineup offers there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    pub difficulty: &'static str,
    pub model: String,
    /// `None`: no effort goes on the command line ([`EffortRule::CliDefault`]).
    pub effort: Option<String>,
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
    /// For a person's effort outside what the model accepts: the value
    /// stands, and this says where the model would bring it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort_clamped: Option<Clamp>,
    pub candidates: Vec<Candidate>,
    /// What runs here once the person's row is cleared — the lineup's or
    /// the table's pair; `None` when neither has a model for it.
    pub auto_model: Option<String>,
    pub auto_effort: Option<String>,
    /// The fields the person wrote; `None` where the lineup fills it.
    pub written_model: Option<String>,
    pub written_effort: Option<String>,
    /// Where the row's model stands in today's lineup; `None` when no
    /// lineup was read.
    pub standing: Option<Standing>,
    /// The words a summons puts on the agent's command line for this row
    /// (`--model …`, and the effort the way this agent's CLI takes it).
    pub launch: Vec<String>,
}

/// One road's answer for a row: its pair, and where each half came from.
#[derive(Clone)]
struct Picked {
    model: String,
    effort: Option<String>,
    from: Source,
    effort_rule: EffortRule,
    effort_clamped: Option<Clamp>,
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
    let read_shipped = |difficulty: &str| -> Result<Option<Written>, String> {
        defaults
            .get(agent)
            .and_then(|rows| rows.get(difficulty))
            .map(|row| serde_json::from_value::<Written>(row.clone()).map_err(|e| e.to_string()))
            .transpose()
    };
    let read_written = |difficulty: &str| -> Result<Option<Written>, String> {
        written
            .and_then(|rows| rows.get(difficulty))
            .map(|row| serde_json::from_value::<Written>(row.clone()).map_err(|e| e.to_string()))
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
    // An agent whose command line takes no effort is tuned at no difficulty
    // (the measured launch table): it has no rows.
    if !crate::orchestration::takes_effort(agent) {
        return Ok(rows);
    }
    for (difficulty, _) in LADDER {
        if only.is_some_and(|only| only != difficulty) {
            continue;
        }
        // The shipped row names a model only: no effort is ours to ship.
        let shipped = read_shipped(difficulty)?.and_then(|row| {
            Some(Profile {
                model: row.model?,
                effort: None,
            })
        });
        // A saved row equal to the shipped one is the shipped one.
        let person = read_written(difficulty)?
            .filter(|row| !row.is_empty() && (row.whole().is_none() || row.whole() != shipped));
        // What runs here when nobody wrote a row: the lineup's model for the
        // rung, else the table's while it is live.
        let auto = if let Some(pick) = lineup.and_then(|held| {
            let rung = Rung::of(difficulty)?;
            let by_rung = held
                .models
                .iter()
                .filter(|model| !model.folded() && model.rungs.contains(&rung))
                .min_by_key(|model| home.as_ref() != Some(&model.provider));
            // No model for the rung: the table's model stands while it is
            // live here; only a folded one gives way to what the lineup
            // offers at this difficulty.
            let table_stands = shipped.as_ref().is_some_and(|row| held.live(&row.model));
            by_rung.or_else(|| {
                (!table_stands)
                    .then(|| offered(held, difficulty).next())
                    .flatten()
            })
        }) {
            Some(Picked {
                model: pick.id.clone(),
                effort: None,
                from: Source::Lineup,
                effort_rule: EffortRule::CliDefault,
                effort_clamped: None,
            })
        } else {
            shipped
                .clone()
                .filter(|row| lineup.is_none_or(|held| held.live(&row.model)))
                .map(|table| Picked {
                    model: table.model,
                    effort: None,
                    from: Source::Table,
                    effort_rule: EffortRule::CliDefault,
                    effort_clamped: None,
                })
        };
        let picked = match person.as_ref() {
            Some(person) => {
                super::written_launchable(agent, person)?;
                match written_pick(person, auto.as_ref(), lineup) {
                    Some(picked) => picked,
                    // No model to run: an effort written alone with no
                    // model here to ride.
                    None => continue,
                }
            }
            None => match auto.clone() {
                Some(auto) => auto,
                None => continue,
            },
        };
        let Picked {
            model,
            effort,
            from,
            effort_rule,
            effort_clamped,
        } = picked;
        let candidates = lineup.map_or_else(Vec::new, |held| {
            offered(held, difficulty)
                .filter(|candidate| candidate.id != held.canonical(&model))
                .map(|candidate| Candidate {
                    model: candidate.id.clone(),
                    effort: None,
                    fresh: seen.is_some_and(|seen| seen.fresh(agent, &candidate.id, now_ms)),
                })
                .collect()
        });
        let held = lineup.and_then(|held| held.find(&model));
        rows.push(Row {
            difficulty,
            band: held.and_then(|held| held.band),
            tier_from: held.and_then(|held| held.tier_from),
            tier_confidence_permille: held.and_then(|held| held.tier_confidence_permille),
            standing: lineup.map(|held| held.standing(&model)),
            launch: crate::orchestration::launch_words(agent, &model, effort.as_deref())
                .unwrap_or_default(),
            auto_model: auto.as_ref().map(|auto| auto.model.clone()),
            auto_effort: auto.and_then(|auto| auto.effort),
            written_model: person.as_ref().and_then(|person| person.model.clone()),
            written_effort: person.and_then(|person| person.effort),
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

/// The pair a person's row runs at: each field as written, the model the
/// lineup's where none is written, and no effort where none is written — the
/// agent's CLI runs its own default. `None` when there is no model: an
/// effort written alone where no model runs here.
fn written_pick(
    person: &Written,
    auto: Option<&Picked>,
    lineup: Option<&Lineup>,
) -> Option<Picked> {
    let (model, from) = match &person.model {
        Some(model) => (model.clone(), Source::Person),
        None => {
            let auto = auto?;
            (auto.model.clone(), auto.from)
        }
    };
    let Some(effort) = person.effort.clone() else {
        return Some(Picked {
            model,
            effort: None,
            from,
            effort_rule: EffortRule::CliDefault,
            effort_clamped: None,
        });
    };
    let clamped = lineup
        .and_then(|held| held.find(&model))
        .and_then(|held| held.clamp(&effort));
    Some(Picked {
        // A person's model and effort stand as written; an effort written
        // alone rides the lineup's model, inside that model's range: the
        // model may move under it, a launch outside may not.
        effort: Some(match (&clamped, &person.model) {
            (Some(to), None) => to.clone(),
            _ => effort.clone(),
        }),
        model,
        from,
        effort_rule: EffortRule::Person,
        effort_clamped: clamped.map(|to| Clamp { from: effort, to }),
    })
}

/// Why a row a person is about to save is refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Refusal {
    /// A model the agent's lineup does not offer today.
    NotListed { model: String },
    /// An effort the row's model does not take; `takes` is what it does.
    #[serde(rename_all = "camelCase")]
    EffortOutside {
        effort: String,
        model: String,
        takes: Vec<String>,
    },
    /// What no lineup could launch ([`super::written_launchable`]).
    Unlaunchable { reason: String },
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotListed { model } => write!(f, "{model} is not in today's lineup"),
            Self::EffortOutside {
                effort,
                model,
                takes,
            } => write!(f, "{model} takes {} — not {effort}", takes.join(", ")),
            Self::Unlaunchable { reason } => f.write_str(reason),
        }
    }
}

/// Check the row `written` a person is about to save for `agent` at
/// `difficulty` into the settings `root`, against today's lineup. A field
/// already written may stay as it is — a model the provider since withdrew
/// keeps its row until the person moves it — but a field that changes must
/// be a model the lineup offers and an effort that model takes. An empty row
/// (back to the lineup) always passes; with no lineup read, only what could
/// launch at all is checked.
///
/// # Errors
/// The [`Refusal`] that names what is wrong.
pub fn check_written(
    root: &Value,
    agent: &str,
    difficulty: &str,
    written: &Written,
    lineup: Option<&Lineup>,
) -> Result<(), Refusal> {
    if written.is_empty() {
        return Ok(());
    }
    super::written_launchable(agent, written).map_err(|reason| Refusal::Unlaunchable { reason })?;
    let Some(held) = lineup else {
        return Ok(());
    };
    let before = row_at(root, agent, difficulty, Some(held), None, 0)
        .ok()
        .flatten();
    let kept_model = before.as_ref().and_then(|row| row.written_model.clone());
    let kept_effort = before.as_ref().and_then(|row| row.written_effort.clone());
    if let Some(model) = &written.model
        && kept_model.as_ref() != Some(model)
        && !held.live(model)
    {
        return Err(Refusal::NotListed {
            model: model.clone(),
        });
    }
    let unchanged = written.model == kept_model && written.effort == kept_effort;
    if let Some(effort) = &written.effort
        && !unchanged
    {
        // The model the effort rides: the one written, else what runs here
        // when nobody wrote a model.
        let model = written
            .model
            .clone()
            .or_else(|| before.as_ref().and_then(|row| row.auto_model.clone()));
        if let Some(model) = model
            && let Some(found) = held.find(&model)
        {
            let (takes, _) = efforts_of(agent, found);
            if !takes.contains(effort) {
                return Err(Refusal::EffortOutside {
                    effort: effort.clone(),
                    model,
                    takes,
                });
            }
        }
    }
    Ok(())
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
            LOW: {"model": "model-old"},
            MID: {"model": "model-alias"},
            HIGH: {"model": "model-old"},
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
            effort: Some("high".into()),
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

    /// t-74181: a default row — nobody wrote one — carries no effort of
    /// ours: no effort goes on the command line and the agent's CLI runs at
    /// its own default, whatever the difficulty, the model's range or the
    /// agent. Before, easy and ordinary work ran at the model's highest
    /// effort, then at a shipped table's word and a coded ladder ending at
    /// `max` (zo ran three workers at opus `max` on 2026-10-28); the person:
    /// no table, in code or in data — the effort is the model seat's pair or
    /// the person's word.
    #[test]
    fn a_default_row_carries_no_effort_and_its_cli_decides() {
        let mut easy = m("model-b", "rest", &["easy", "medium"]);
        easy.efforts = Some(&["low", "medium", "high", "xhigh", "max"]);
        let mut hard = m("model-a", "second", &["hard"]);
        hard.efforts = Some(&["low", "medium", "high", "xhigh", "max"]);
        let mut other = m("model-c", "rest", &[]);
        other.efforts = Some(&["high", "low"]);
        let today = lineup(&[easy, hard, other]);
        for agent in ["claude", "zo"] {
            let bare = rows_with_defaults(&Value::Null, &json!({}), agent, Some(&today), None, NOW)
                .unwrap();
            let table =
                rows_with_defaults(&Value::Null, &shipped(), agent, Some(&today), None, NOW)
                    .unwrap();
            let none =
                rows_with_defaults(&Value::Null, &shipped(), agent, None, None, NOW).unwrap();
            for at in bare.iter().chain(&table).chain(&none) {
                assert_eq!(
                    (
                        at.effort.as_deref(),
                        at.effort_rule,
                        at.auto_effort.as_deref()
                    ),
                    (None, EffortRule::CliDefault, None),
                    "{agent} {at:?}"
                );
                assert!(
                    !at.launch.iter().any(|word| word.contains("effort")),
                    "{agent}: an effort nobody chose went on the command line: {at:?}"
                );
                assert!(at.candidates.iter().all(|c| c.effort.is_none()), "{at:?}");
            }
            assert_eq!(bare.len(), LADDER.len(), "{agent}: {bare:?}");
        }
        let rows = read(&Value::Null, Some(&today), None);
        assert_eq!(
            row(&rows, HIGH).model,
            "model-a",
            "the difficulty still picks the model"
        );
        assert_eq!(row(&rows, LOW).model, "model-b");
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
        assert_eq!(written.effort.as_deref(), Some("high"));
        assert_eq!(written.effort_rule, EffortRule::Person);
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

    /// A person's effort outside what the row's model takes: written with
    /// the model it stands and the row says so; written alone, it rides the
    /// lineup's model and comes down inside what that model takes.
    #[test]
    fn the_effort_comes_down_inside_what_the_model_accepts() {
        let mut capped = m("model-a", "second", &["hard"]);
        capped.efforts = Some(&["low", "medium", "high"]);
        let root = settings(json!({"claude": {HIGH: {"effort": "max"}}}));
        let rows = read(&root, Some(&lineup(&[capped])), None);
        let high = row(&rows, HIGH);
        assert_eq!(
            high.effort.as_deref(),
            Some("high"),
            "max is not accepted; the nearest below is"
        );
        assert_eq!(
            high.effort_clamped,
            Some(Clamp {
                from: "max".into(),
                to: "high".into()
            })
        );
        // A person's effort beside their model stands; the row only says it
        // is outside.
        let mut person = shipped()["claude"].clone();
        person[HIGH] = json!({"model": "model-a", "effort": "max"});
        let mut capped = m("model-a", "second", &["hard"]);
        capped.efforts = Some(&["low", "medium", "high"]);
        let root = settings(json!({"claude": person}));
        let rows = read(&root, Some(&lineup(&[capped])), None);
        assert_eq!(row(&rows, HIGH).effort.as_deref(), Some("max"));
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

    /// What a person picks from (the settings screen's lists): every live
    /// model with the efforts it takes — a connection's model, which names
    /// none, the agent's ladder — where it fits, and whose it is.
    #[test]
    fn a_pick_carries_what_it_takes_where_it_fits_and_whose_it_is() {
        let catalog = json!({
            "models": [
                {"provider": "claude", "id": "model-a", "band": "second", "rungs": ["hard"], "efforts": ["low", "high"]},
                {"provider": "claude", "id": "model-gone", "band": "rest", "rungs": ["easy"], "unlistedSince": 1_790_000_000},
            ],
            "customProviders": [{"name": "custom-a", "models": ["custom-a/model-x"], "usable": true}],
        });
        let all = Lineup::from_catalog_all(&catalog).unwrap();
        let picks = all.picks("zo", None, NOW);
        let ids: Vec<&str> = picks.iter().map(|pick| pick.id.as_str()).collect();
        assert_eq!(
            ids,
            ["model-a", "custom-a/model-x"],
            "a folded model is nobody's pick"
        );
        assert_eq!(picks[0].efforts, ["low", "high"]);
        assert!(picks[0].efforts_known);
        assert_eq!(picks[0].fits, [HIGH]);
        assert!(!picks[0].connection);
        assert_eq!(picks[1].provider, "custom-a");
        assert!(picks[1].connection);
        assert!(!picks[1].efforts_known);
        assert_eq!(
            picks[1].efforts,
            crate::orchestration::ladder_efforts("zo"),
            "a connection's model is offered at the agent's ladder"
        );
        assert_eq!(
            all.standing("model-gone"),
            Standing::Unlisted {
                since: Some(1_790_000_000)
            }
        );
        assert_eq!(all.standing("model-a"), Standing::Listed);
        assert_eq!(all.standing("model-none"), Standing::Missing);
    }

    /// A row may leave either field to the lineup (the settings screen's
    /// 「자동」): a model written alone runs with no effort of ours — its CLI's
    /// default — an effort written alone rides the lineup's model inside what
    /// that takes, and every row says what 「자동」 would run.
    #[test]
    fn a_row_written_by_half_takes_the_other_half_from_the_lineup() {
        let mut hard = m("model-a", "second", &["hard"]);
        hard.efforts = Some(&["low", "medium", "high"]);
        let mut easy = m("model-b", "rest", &["easy"]);
        easy.efforts = Some(&["low", "medium", "high", "max"]);
        let held = lineup(&[hard, easy]);
        let rows = read(
            &settings(json!({"claude": {
                HIGH: {"effort": "low"},
                LOW: {"model": "model-a"},
            }})),
            Some(&held),
            None,
        );
        let high = row(&rows, HIGH);
        assert_eq!(
            (high.model.as_str(), high.effort.as_deref()),
            ("model-a", Some("low"))
        );
        assert_eq!(high.from, Source::Lineup, "the model is still the lineup's");
        assert_eq!(high.effort_rule, EffortRule::Person);
        assert_eq!(high.written_effort.as_deref(), Some("low"));
        assert_eq!(high.written_model, None);
        assert_eq!(high.auto_model.as_deref(), Some("model-a"));
        assert_eq!(high.auto_effort, None, "「자동」 is the CLI's own default");
        let low = row(&rows, LOW);
        assert_eq!(
            (low.model.as_str(), low.effort.as_deref(), low.effort_rule),
            ("model-a", None, EffortRule::CliDefault),
            "no effort of ours beside the person's model"
        );
        assert_eq!(low.from, Source::Person);
        assert_eq!(low.auto_model.as_deref(), Some("model-b"));
        assert_eq!(low.launch, ["--model", "model-a"]);
        assert_eq!(low.standing, Some(Standing::Listed));
        // An effort written alone is brought inside the model it rides.
        let rows = read(
            &settings(json!({"claude": {HIGH: {"effort": "max"}}})),
            Some(&held),
            None,
        );
        let high = row(&rows, HIGH);
        assert_eq!(high.effort.as_deref(), Some("high"));
        assert_eq!(
            high.effort_clamped,
            Some(Clamp {
                from: "max".into(),
                to: "high".into()
            })
        );
    }

    /// The save door's check (the settings screen writes one row at a time):
    /// a model the lineup does not offer, or an effort the row's model does
    /// not take, is refused by name; a field already written may stay; an
    /// empty row always passes; with no lineup only launching is checked.
    #[test]
    fn a_row_is_checked_against_the_lineup_before_it_is_saved() {
        let mut hard = m("model-a", "second", &["hard"]);
        hard.efforts = Some(&["low", "medium", "high"]);
        let mut gone = m("model-gone", "rest", &["easy"]);
        gone.unlisted = true;
        let held = lineup(&[hard, gone]);
        let cell = |model: Option<&str>, effort: Option<&str>| Written {
            model: model.map(str::to_string),
            effort: effort.map(str::to_string),
        };
        let check = |root: &Value, written: &Written| {
            check_written(root, "claude", HIGH, written, Some(&held))
        };
        let none = Value::Null;
        assert_eq!(check(&none, &cell(Some("model-a"), Some("high"))), Ok(()));
        assert_eq!(
            check(&none, &cell(Some("model-z"), None)),
            Err(Refusal::NotListed {
                model: "model-z".into()
            })
        );
        assert_eq!(
            check(&none, &cell(Some("model-gone"), None)),
            Err(Refusal::NotListed {
                model: "model-gone".into()
            }),
            "a withdrawn model is nobody's new choice"
        );
        let outside = Err(Refusal::EffortOutside {
            effort: "max".into(),
            model: "model-a".into(),
            takes: vec!["low".into(), "medium".into(), "high".into()],
        });
        assert_eq!(check(&none, &cell(Some("model-a"), Some("max"))), outside);
        assert_eq!(
            check(&none, &cell(None, Some("max"))),
            outside,
            "an effort alone is checked against the lineup's model"
        );
        assert_eq!(check(&none, &Written::default()), Ok(()));
        assert!(matches!(
            check_written(&none, "claude", HIGH, &cell(Some("two words"), None), None),
            Err(Refusal::Unlaunchable { .. })
        ));
        assert_eq!(
            check_written(
                &none,
                "claude",
                HIGH,
                &cell(Some("model-z"), Some("ultra")),
                None
            ),
            Ok(()),
            "no lineup: nothing to check against"
        );
        let kept = settings(json!({"claude": {HIGH: {"model": "model-gone", "effort": "high"}}}));
        assert_eq!(
            check(&kept, &cell(Some("model-gone"), Some("medium"))),
            Ok(()),
            "a model already written stays while its effort moves"
        );
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
            if !crate::orchestration::takes_effort(spec.id) {
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
