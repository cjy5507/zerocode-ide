//! Which Claude account the window should run the NEXT launch as, and when a
//! pane standing at its quota wall should be seated again on another account
//! — one table and one pure judgment, read by the window's beat (t-7538).
//!
//! **What this is not.** It never reads a credential, never names an email,
//! and never decides that a working pane should be touched: a pane that is
//! working keeps its own login until the provider refuses it (the
//! two-witness wall, [`crate::orchestration::quota_wall_witness`]). The only
//! things it chooses are (1) the account new launches use — the window's
//! default — and (2) the account a walled pane continues on.
//!
//! **Why the thresholds are the summons table's.** The summons gate already
//! says what "at the wall" and "near it" mean for this window
//! (`QUOTA_POLICY.wall_percent` 97, `warn_percent` 90); a second pair of
//! numbers with the same meaning was the duplication the briefing forbade.
//! The default moves at the warn line ("한도 앞에서") and a pane moves at
//! the wall — both read off that one table.
//!
//! The person's own account ids are the only identity this module speaks
//! about, and they stay opaque strings; the organisation type is a word the
//! CLI reported (`claude_team`, `claude_max`) and is used only to break ties.

use crate::orchestration::{QUOTA_POLICY, QUOTA_WAIT_POLICY, minutes_up};

/// The person's answer to "may the window switch accounts by itself?" —
/// `accounts.claudeAutoSwitch`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AutoSwitchMode {
    /// Nothing happens without a hand: the picker in settings is the only
    /// road, and the beat only reads.
    Off,
    /// The beat proposes — one line and a button — and switches on a yes
    /// given for THAT proposal (source, target, generation). The default.
    #[default]
    Ask,
    /// The beat switches and leaves a receipt.
    Auto,
}

impl AutoSwitchMode {
    /// Every mode, in the order the settings control lists them.
    pub const ALL: [Self; 3] = [Self::Off, Self::Ask, Self::Auto];

    pub const fn word(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Ask => "ask",
            Self::Auto => "auto",
        }
    }

    /// The mode a word names; `None` for a word the table does not hold, so
    /// a caller decides its own fallback rather than this table inventing one.
    #[must_use]
    pub fn of(word: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|mode| mode.word() == word.trim().to_ascii_lowercase())
    }

    /// Whether the beat may act at all (propose or switch).
    pub const fn acts(self) -> bool {
        !matches!(self, Self::Off)
    }
}

/// The numbers the beat decides under. One table, and every number in it is
/// a reading of one the window already keeps — none is a new literal.
pub struct AutoSwitchPolicy {
    /// At or above this on any window that limits the launch, the default
    /// account is moved for new launches — and an account already this full
    /// is no place to move the default TO, or the next beat would move it
    /// straight back. `QUOTA_POLICY.warn_percent`: "한도 앞에서".
    pub default_moves_at_percent: u8,
    /// At or above this a window is BLOCKED and the account cannot be a
    /// candidate at all; a walled pane may still land on an account under
    /// it, because any room beats a refusal. `QUOTA_POLICY.wall_percent`.
    pub blocked_at_percent: u8,
    /// A gauge older than this is not a reading anybody may choose by.
    /// `QUOTA_POLICY.snapshot_max_age_ms`.
    pub gauge_max_age_ms: i64,
    /// A reset this close is waited out instead of switched around — the
    /// summons gate's own "ask again in N min". `QUOTA_POLICY.reset_soon_ms`.
    /// Not the wall's six-hour lifetime (`QUOTA_WAIT_POLICY.max_wait_ms`):
    /// that is how long a wall REASON stays valid, and read as "near" it
    /// would make every five-hour session wait instead of switching.
    pub near_reset_ms: i64,
    /// After a switch, how long the beat leaves the default where it is,
    /// whatever the numbers say. One usage re-read floor
    /// (`QUOTA_WAIT_POLICY.lift_read_ms`, the window's `MIN_REFETCH`): inside
    /// it the gauges cannot even have been read again, so a second move
    /// would be a move on the same numbers.
    pub cooldown_ms: i64,
    /// Which organisation type is spent first, first is best — read BEFORE
    /// room, because a percentage is a share of that plan's own window and
    /// a team seat's 20% and a Max plan's 20% are not the same amount of
    /// work (astra A3). Team before Max: the team seat is spent while it
    /// has room under the move line and the personal Max plan is kept as
    /// the reserve. Room decides within one type; the id breaks what is
    /// left, so two runs of the same numbers name the same account.
    pub org_type_order: &'static [&'static str],
}

/// The name every receipt written under this table carries, so a row can
/// be read against the rules that made it.
pub const POLICY_WORD: &str = "CLAUDE_ACCOUNT_AUTOSWITCH.v1";

/// The policy as it stands (2026-09-25, t-7538).
pub const CLAUDE_ACCOUNT_AUTOSWITCH: AutoSwitchPolicy = AutoSwitchPolicy {
    default_moves_at_percent: QUOTA_POLICY.warn_percent,
    blocked_at_percent: QUOTA_POLICY.wall_percent,
    gauge_max_age_ms: QUOTA_POLICY.snapshot_max_age_ms,
    near_reset_ms: QUOTA_POLICY.reset_soon_ms,
    cooldown_ms: QUOTA_WAIT_POLICY.lift_read_ms,
    org_type_order: &["claude_team", "claude_max"],
};

/// Which launches one gauge window limits (t-7538, astra A3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limits {
    /// Every request the login makes counts against it.
    EveryModel,
    /// Only a model of this family — a word of its id (`claude-fable-5-1`,
    /// `fable`). A Fable-only week at 100% does not stop an Opus launch
    /// (vault: a Fable-only cap walled a worker while the session gauge
    /// read a quarter).
    Family(&'static str),
}

/// Every window the usage cache names, and what it limits. A window this
/// table does not name limits everything: a window the provider adds is a
/// limit until somebody writes down otherwise.
pub const WINDOW_LIMITS: &[(&str, Limits)] = &[
    ("session", Limits::EveryModel),
    ("weekly", Limits::EveryModel),
    ("fable_weekly", Limits::Family("fable")),
];

/// Whether `window` limits a launch of `model`. `None` is a launch whose
/// model nobody knows — the default serves every model the next summons
/// may name, and a pane whose model was never reported could be running
/// any — so every window limits it.
#[must_use]
pub fn limits(window: &str, model: Option<&str>) -> bool {
    match WINDOW_LIMITS
        .iter()
        .find(|(kind, _)| *kind == window)
        .map(|(_, held)| *held)
    {
        None | Some(Limits::EveryModel) => true,
        Some(Limits::Family(family)) => model.is_none_or(|model| {
            model
                .to_ascii_lowercase()
                .split(|glyph: char| !glyph.is_ascii_alphanumeric())
                .any(|word| word == family)
        }),
    }
}

/// One provider window of one account, as the window's usage cache holds it
/// — the percentage the provider reported and when that window resets.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GaugeWindow {
    /// `session`, `weekly`, `fable_weekly` — the cache's own names.
    pub kind: String,
    pub used_percent: u8,
    pub resets_at_ms: Option<i64>,
}

impl GaugeWindow {
    /// Whether this window is a number an account may be chosen by (astra
    /// R6): a percentage inside 0–100, and — for a window that has counted
    /// anything — the reset that frees it. A window at 0% names no reset
    /// because nothing has started it; one that has counted something and
    /// names none says nothing about when its room comes back, and is
    /// unknown rather than room.
    #[must_use]
    pub fn is_a_reading(&self) -> bool {
        self.used_percent <= 100 && (self.used_percent == 0 || self.resets_at_ms.is_some())
    }
}

/// One account's reading, id and numbers only.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AccountGauge {
    pub id: String,
    /// The CLI's `organizationType` word, when the store recorded one.
    pub org_type: Option<String>,
    /// Which login the row is — the store's account and organisation uuids,
    /// joined. Two rows naming one login are one quota counted twice, and
    /// "switching" between them moves nothing (astra A3). Compared here,
    /// never shown: it does not cross into any answer.
    #[serde(skip)]
    pub identity: Option<String>,
    /// Every window the last read reported. Empty for a read that failed —
    /// which is an UNKNOWN, never a 0%.
    pub windows: Vec<GaugeWindow>,
    /// When the read behind `windows` finished, epoch ms.
    pub observed_at_ms: i64,
    /// The read's own status word (`ok`, `error`, `signed_out`, `denied`,
    /// …). Only `ok` is a reading; anything else keeps the account out of
    /// the choice.
    pub status: String,
}

impl AccountGauge {
    /// The windows that limit a launch of `model`.
    fn limiting<'a>(&'a self, model: Option<&'a str>) -> impl Iterator<Item = &'a GaugeWindow> {
        self.windows
            .iter()
            .filter(move |window| limits(&window.kind, model))
    }

    fn same_login_as(&self, other: &Self) -> bool {
        self.identity.is_some() && self.identity == other.identity
    }
}

/// Why an account cannot be chosen right now, said as one word for a UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unfit {
    /// No successful read, or no window in it that limits the launch.
    Unknown,
    /// The read is older than the table allows, or a window in it has
    /// already reset — a number about nothing.
    Stale,
    /// The reading is from the future — a clock nobody trusts.
    FutureRead,
    /// At least one limiting window is at or past the blocked line.
    Blocked,
}

/// What one account's gauge says about choosing it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Fitness {
    pub id: String,
    /// Percent of room left on the FULLEST limiting window — the one that
    /// will refuse first. `None` when the reading is not one to choose by;
    /// `Some(0)` for a blocked account, which is a reading.
    pub room_percent: Option<u8>,
    pub unfit: Option<Unfit>,
    /// The earliest reset among the limiting windows at or past the
    /// default-moves line, when there is one.
    pub next_reset_ms: Option<i64>,
}

fn fitness(gauge: &AccountGauge, model: Option<&str>, now_ms: i64) -> Fitness {
    let policy = &CLAUDE_ACCOUNT_AUTOSWITCH;
    let limiting: Vec<&GaugeWindow> = gauge.limiting(model).collect();
    let unfit = if gauge.status != "ok"
        || limiting.is_empty()
        || limiting.iter().any(|window| !window.is_a_reading())
    {
        Some(Unfit::Unknown)
    } else if gauge.observed_at_ms > now_ms {
        Some(Unfit::FutureRead)
    } else if now_ms.saturating_sub(gauge.observed_at_ms) > policy.gauge_max_age_ms
        || limiting
            .iter()
            .any(|window| window.resets_at_ms.is_some_and(|at| at <= now_ms))
    {
        Some(Unfit::Stale)
    } else if limiting
        .iter()
        .any(|window| window.used_percent >= policy.blocked_at_percent)
    {
        Some(Unfit::Blocked)
    } else {
        None
    };
    let fullest = limiting
        .iter()
        .map(|window| window.used_percent.min(100))
        .max();
    let next_reset_ms = limiting
        .iter()
        .filter(|window| window.used_percent >= policy.default_moves_at_percent)
        .filter_map(|window| window.resets_at_ms)
        .filter(|at| *at > now_ms)
        .min();
    Fitness {
        id: gauge.id.clone(),
        room_percent: match unfit {
            Some(Unfit::Unknown | Unfit::Stale | Unfit::FutureRead) => None,
            Some(Unfit::Blocked) => Some(0),
            None => fullest.map(|used| 100 - used),
        },
        unfit,
        next_reset_ms,
    }
}

/// Why the beat is switching, in the receipt's own words.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SwitchReason {
    /// The source's fullest window crossed the default-moves line.
    NearLimit { window: String, used_percent: u8 },
    /// A pane on the source stands at its two-witness wall.
    Walled,
}

impl SwitchReason {
    /// The receipt's reason word.
    #[must_use]
    pub const fn word(&self) -> &'static str {
        match self {
            Self::NearLimit { .. } => "near_limit",
            Self::Walled => "walled",
        }
    }
}

/// What the beat should do about one account's launches.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Decision {
    /// Nothing. `why` is one word the status bar says in its own language:
    /// `room` (the source is fine), `alone` (one account), `no_candidate`
    /// (nobody fit to move to), `cooldown` (a switch was just made),
    /// `unknown` (the source is not a managed account), `unread` (the
    /// source's own reading cannot be trusted), `off` (the person said no),
    /// `unattributed` (a walled pane this window cannot name the account of
    /// — never presumed to be the selected one, astra A4).
    Stay { why: &'static str },
    /// Every window that crossed the line resets within the table's near
    /// window: wait for it rather than move.
    Wait { until_ms: i64, minutes: u64 },
    /// Move from `from` to `to`.
    Switch {
        from: String,
        to: String,
        reason: SwitchReason,
    },
}

/// What the beat asks about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question<'a> {
    /// The account the question is about: the window's default, or the
    /// account a walled pane runs as.
    pub source: &'a str,
    /// Every managed account's latest reading, the source's included.
    pub gauges: &'a [AccountGauge],
    /// The model the moved launches run: `None` for the default, which
    /// serves every model (so every window limits it).
    pub model: Option<&'a str>,
    /// When the default last moved, if it did. The cooldown is the
    /// default's; a walled pane is refused NOW and asks with `None`.
    pub last_switch_ms: Option<i64>,
    /// Whether a pane on `source` stands at its two-witness wall — the
    /// second trigger, which does not wait for the source's own gauge to
    /// say so (the wall's witness already did).
    pub walled: bool,
    pub now_ms: i64,
}

/// The candidate with the most room under `ceiling` on every window that
/// limits `model`, by the table's order; `None` when nobody fit exists
/// other than the source (and other than the source's own login under
/// another row).
#[must_use]
pub fn best_candidate(
    source: &str,
    gauges: &[AccountGauge],
    model: Option<&str>,
    ceiling: u8,
    now_ms: i64,
) -> Option<Fitness> {
    let order = CLAUDE_ACCOUNT_AUTOSWITCH.org_type_order;
    let rank = |gauge: &AccountGauge| {
        let org = gauge.org_type.as_deref().unwrap_or_default();
        order
            .iter()
            .position(|held| *held == org)
            .unwrap_or(order.len())
    };
    let source_gauge = gauges.iter().find(|gauge| gauge.id == source);
    gauges
        .iter()
        .filter(|gauge| gauge.id != source)
        .filter(|gauge| source_gauge.is_none_or(|held| !gauge.same_login_as(held)))
        .map(|gauge| (gauge, fitness(gauge, model, now_ms)))
        .filter(|(_, fit)| fit.unfit.is_none())
        .filter(|(_, fit)| {
            fit.room_percent
                .is_some_and(|room| 100 - u16::from(room) < u16::from(ceiling))
        })
        // The organisation's turn first, then room within it, then the id.
        .min_by(|(left, left_fit), (right, right_fit)| {
            rank(left)
                .cmp(&rank(right))
                .then_with(|| right_fit.room_percent.cmp(&left_fit.room_percent))
                .then_with(|| left.id.cmp(&right.id))
        })
        .map(|(_, fit)| fit)
}

/// When every limiting window at or past the move line resets inside the
/// near window, the moment the last of them does — the wait that frees the
/// source. `None` when one of them never names a reset or resets later.
fn near_reset(gauge: &AccountGauge, model: Option<&str>, now_ms: i64) -> Option<i64> {
    let policy = &CLAUDE_ACCOUNT_AUTOSWITCH;
    let crossed: Vec<&GaugeWindow> = gauge
        .limiting(model)
        .filter(|window| window.used_percent >= policy.default_moves_at_percent)
        .collect();
    if crossed.is_empty() {
        return None;
    }
    let latest = crossed
        .iter()
        .map(|window| window.resets_at_ms)
        .collect::<Option<Vec<i64>>>()?
        .into_iter()
        .max()?;
    (latest > now_ms && latest.saturating_sub(now_ms) <= policy.near_reset_ms).then_some(latest)
}

/// The judgment: whether `source` should give way, and to whom.
#[must_use]
pub fn decide(question: &Question<'_>) -> Decision {
    let policy = &CLAUDE_ACCOUNT_AUTOSWITCH;
    let now_ms = question.now_ms;
    if question.gauges.len() < 2 {
        return Decision::Stay { why: "alone" };
    }
    if question
        .last_switch_ms
        .is_some_and(|at| now_ms.saturating_sub(at) < policy.cooldown_ms)
    {
        return Decision::Stay { why: "cooldown" };
    }
    let Some(source) = question
        .gauges
        .iter()
        .find(|gauge| gauge.id == question.source)
    else {
        return Decision::Stay { why: "unknown" };
    };
    let fit = fitness(source, question.model, now_ms);
    let trusted = fit.unfit.is_none() || fit.unfit == Some(Unfit::Blocked);
    // The trigger: the wall's witness, or the source's own fullest limiting
    // window at the default-moves line — read only off a reading the table
    // trusts.
    let crossed = trusted
        .then(|| {
            source
                .limiting(question.model)
                .filter(|window| window.used_percent >= policy.default_moves_at_percent)
                .max_by_key(|window| window.used_percent)
        })
        .flatten();
    let reason = if question.walled {
        SwitchReason::Walled
    } else if let Some(window) = crossed {
        SwitchReason::NearLimit {
            window: window.kind.clone(),
            used_percent: window.used_percent,
        }
    } else if trusted {
        return Decision::Stay { why: "room" };
    } else {
        return Decision::Stay { why: "unread" };
    };
    // Near a reset, the answer is patience — but only when EVERY window that
    // crossed the line resets inside the near window. A session that resets
    // in a minute does not excuse a week that is at 100% for six days. A
    // wall waits too when its gauge names the reset that lifts it: moving a
    // conversation costs its warm cache on the new login, and five minutes
    // do not repay that.
    if let Some(at) = near_reset(source, question.model, now_ms) {
        return Decision::Wait {
            until_ms: at,
            minutes: minutes_up(at.saturating_sub(now_ms)),
        };
    }
    // Where the default may go must itself be under the move line, or the
    // next beat moves it straight back; a walled pane takes any room.
    let ceiling = if question.walled {
        policy.blocked_at_percent
    } else {
        policy.default_moves_at_percent
    };
    match best_candidate(
        question.source,
        question.gauges,
        question.model,
        ceiling,
        now_ms,
    ) {
        Some(candidate) => Decision::Switch {
            from: question.source.to_string(),
            to: candidate.id,
            reason,
        },
        None => Decision::Stay {
            why: "no_candidate",
        },
    }
}

/// Where ONE walled pane goes (t-7538): to `landing` — the account new
/// launches run as once the default's own decision is made, because a pane
/// is seated again through the ordinary launch and a launch runs as the
/// default — when the landing is another login with room for the pane's
/// model; after its own reset when that is near; nowhere otherwise.
#[must_use]
pub fn judge_pane(
    pane_account: &str,
    pane_model: Option<&str>,
    landing: &str,
    gauges: &[AccountGauge],
    now_ms: i64,
) -> Decision {
    let own = gauges.iter().find(|gauge| gauge.id == pane_account);
    let Some(target) = gauges.iter().find(|gauge| gauge.id == landing) else {
        return Decision::Stay { why: "unknown" };
    };
    // The pane's own reset first: when it is near, the answer is the same
    // whether or not there is somewhere to go.
    if let Some(at) = own.and_then(|own| near_reset(own, pane_model, now_ms)) {
        return Decision::Wait {
            until_ms: at,
            minutes: minutes_up(at.saturating_sub(now_ms)),
        };
    }
    if landing == pane_account || own.is_some_and(|own| own.same_login_as(target)) {
        return Decision::Stay {
            why: "no_candidate",
        };
    }
    // Blocked is unfit: any room under the blocked line beats a refusal.
    if fitness(target, pane_model, now_ms).unfit.is_some() {
        return Decision::Stay {
            why: "no_candidate",
        };
    }
    Decision::Switch {
        from: pane_account.to_string(),
        to: landing.to_string(),
        reason: SwitchReason::Walled,
    }
}

/// Every account's fitness for the default (every window), for the accounts
/// pane's table and the status bar.
#[must_use]
pub fn fitness_table(gauges: &[AccountGauge], now_ms: i64) -> Vec<Fitness> {
    gauges
        .iter()
        .map(|gauge| fitness(gauge, None, now_ms))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_700_000_000_000;
    const MIN: i64 = 60_000;

    fn gauge(id: &str, org: &str, windows: &[(&str, u8, Option<i64>)]) -> AccountGauge {
        AccountGauge {
            id: id.to_string(),
            org_type: Some(org.to_string()),
            identity: Some(format!("login-of-{id}")),
            windows: windows
                .iter()
                .map(|(kind, used, resets)| GaugeWindow {
                    kind: (*kind).to_string(),
                    used_percent: *used,
                    resets_at_ms: *resets,
                })
                .collect(),
            observed_at_ms: NOW - MIN,
            status: "ok".to_string(),
        }
    }

    fn ask<'a>(source: &'a str, gauges: &'a [AccountGauge]) -> Question<'a> {
        Question {
            source,
            gauges,
            model: None,
            last_switch_ms: None,
            walled: false,
            now_ms: NOW,
        }
    }

    /// The reset of a session window an hour out; the week's six days out.
    const HOUR: Option<i64> = Some(NOW + 60 * MIN);
    const DAYS6: Option<i64> = Some(NOW + 6 * 24 * 60 * MIN);

    #[test]
    fn the_autoswitch_table_picks_the_account_with_the_most_room_and_waits_when_the_reset_is_near()
    {
        // Today's case: the active account's session at 95%, weekly 96%;
        // two others with room. Within one plan type the most room wins.
        let gauges = [
            gauge(
                "a",
                "claude_max",
                &[("session", 95, HOUR), ("weekly", 96, DAYS6)],
            ),
            gauge(
                "b",
                "claude_max",
                &[("session", 40, HOUR), ("weekly", 70, DAYS6)],
            ),
            gauge(
                "c",
                "claude_max",
                &[("session", 10, HOUR), ("weekly", 20, DAYS6)],
            ),
        ];
        assert_eq!(
            decide(&ask("a", &gauges)),
            Decision::Switch {
                from: "a".into(),
                to: "c".into(),
                reason: SwitchReason::NearLimit {
                    window: "weekly".into(),
                    used_percent: 96
                },
            }
        );
        // The same numbers with the session resetting in five minutes and
        // the WEEK still six days out: the week is the wall, so no wait.
        let soon = Some(NOW + 5 * MIN);
        let gauges = [
            gauge(
                "a",
                "claude_max",
                &[("session", 95, soon), ("weekly", 96, DAYS6)],
            ),
            gauge(
                "c",
                "claude_team",
                &[("session", 10, HOUR), ("weekly", 20, DAYS6)],
            ),
        ];
        assert!(matches!(
            decide(&ask("a", &gauges)),
            Decision::Switch { .. }
        ));
        // Only the session crossed, and it resets in five minutes: wait.
        let gauges = [
            gauge(
                "a",
                "claude_max",
                &[("session", 95, soon), ("weekly", 50, DAYS6)],
            ),
            gauge(
                "c",
                "claude_team",
                &[("session", 10, HOUR), ("weekly", 20, DAYS6)],
            ),
        ];
        assert_eq!(
            decide(&ask("a", &gauges)),
            Decision::Wait {
                until_ms: NOW + 5 * MIN,
                minutes: 5
            }
        );
        // A wall whose own gauge names the reset that lifts it five minutes
        // out waits too: a moved conversation pays its cache again on the
        // new login, and five minutes do not repay that.
        let mut walled = ask("a", &gauges);
        walled.walled = true;
        assert!(matches!(decide(&walled), Decision::Wait { minutes: 5, .. }));
        // A wall the gauge does not show (a cap the session and week do not
        // carry) names no reset: it moves.
        let quiet = [
            gauge(
                "a",
                "claude_max",
                &[("session", 25, HOUR), ("weekly", 50, DAYS6)],
            ),
            gauge(
                "c",
                "claude_team",
                &[("session", 10, HOUR), ("weekly", 20, DAYS6)],
            ),
        ];
        let mut walled = ask("a", &quiet);
        walled.walled = true;
        assert_eq!(
            decide(&walled),
            Decision::Switch {
                from: "a".into(),
                to: "c".into(),
                reason: SwitchReason::Walled,
            }
        );
    }

    /// A number an account may be chosen by (astra R6): a window outside
    /// 0–100, or one that has counted something and names no reset to free
    /// it, is unknown — never room; a window nothing has started (0%, no
    /// reset) is room. As a candidate the unknown one is passed over.
    #[test]
    fn a_window_out_of_range_or_counting_with_no_reset_is_unknown_never_room() {
        let fit = |windows: &[(&str, u8, Option<i64>)]| {
            fitness(&gauge("b", "claude_team", windows), None, NOW)
        };
        let unreset = fit(&[("session", 30, None), ("weekly", 20, DAYS6)]);
        assert_eq!(unreset.unfit, Some(Unfit::Unknown));
        assert_eq!(unreset.room_percent, None);
        let over = fit(&[("session", 101, HOUR), ("weekly", 20, DAYS6)]);
        assert_eq!(over.unfit, Some(Unfit::Unknown));
        assert_eq!(over.room_percent, None);
        let unstarted = fit(&[("session", 0, None), ("weekly", 20, DAYS6)]);
        assert_eq!(unstarted.unfit, None);
        assert_eq!(unstarted.room_percent, Some(80));
        let gauges = [
            gauge(
                "a",
                "claude_max",
                &[("session", 95, HOUR), ("weekly", 50, DAYS6)],
            ),
            gauge(
                "b",
                "claude_team",
                &[("session", 5, None), ("weekly", 10, DAYS6)],
            ),
        ];
        assert_eq!(
            decide(&ask("a", &gauges)),
            Decision::Stay {
                why: "no_candidate"
            }
        );
        assert_eq!(
            judge_pane("a", None, "b", &gauges, NOW),
            Decision::Stay {
                why: "no_candidate"
            }
        );
    }

    #[test]
    fn a_source_with_room_stays_and_a_lone_account_has_nobody_to_switch_to() {
        let gauges = [
            gauge(
                "a",
                "claude_max",
                &[("session", 40, HOUR), ("weekly", 60, DAYS6)],
            ),
            gauge(
                "b",
                "claude_max",
                &[("session", 0, HOUR), ("weekly", 0, DAYS6)],
            ),
        ];
        assert_eq!(decide(&ask("a", &gauges)), Decision::Stay { why: "room" });
        let alone = [gauge("a", "claude_max", &[("session", 99, HOUR)])];
        assert_eq!(decide(&ask("a", &alone)), Decision::Stay { why: "alone" });
        // The source itself is not a candidate, and an unknown source is
        // not judged.
        assert_eq!(
            best_candidate(
                "a",
                &alone,
                None,
                CLAUDE_ACCOUNT_AUTOSWITCH.blocked_at_percent,
                NOW
            ),
            None
        );
        assert_eq!(
            decide(&ask("zz", &gauges)),
            Decision::Stay { why: "unknown" }
        );
        // A source whose own reading cannot be trusted is not "room": the
        // bar says it could not read, and nothing moves on a guess.
        let mut unread = gauges.clone();
        unread[0].status = "error".to_string();
        assert_eq!(decide(&ask("a", &unread)), Decision::Stay { why: "unread" });
    }

    #[test]
    fn unknown_stale_future_and_blocked_readings_are_never_room() {
        let mut unknown = gauge("b", "claude_max", &[]);
        unknown.status = "error".to_string();
        let mut stale = gauge("c", "claude_max", &[("session", 0, HOUR)]);
        stale.observed_at_ms = NOW - CLAUDE_ACCOUNT_AUTOSWITCH.gauge_max_age_ms - 1;
        let mut future = gauge("d", "claude_max", &[("session", 0, HOUR)]);
        future.observed_at_ms = NOW + MIN;
        // Session wide open, week blocked: excluded, not "the most room".
        let blocked = gauge(
            "e",
            "claude_max",
            &[("session", 0, HOUR), ("weekly", 98, DAYS6)],
        );
        // A window that has already reset is a number about nothing.
        let reset = gauge("f", "claude_max", &[("session", 0, Some(NOW - 1))]);
        let mut denied = gauge("g", "claude_max", &[]);
        denied.status = "denied".to_string();
        let source = gauge("a", "claude_max", &[("session", 96, HOUR)]);
        let gauges = [source, unknown, stale, future, blocked, reset, denied];
        assert_eq!(
            decide(&ask("a", &gauges)),
            Decision::Stay {
                why: "no_candidate"
            }
        );
        let table = fitness_table(&gauges, NOW);
        let of = |id: &str| table.iter().find(|fit| fit.id == id).expect("a row");
        assert_eq!(of("b").unfit, Some(Unfit::Unknown));
        assert_eq!(of("c").unfit, Some(Unfit::Stale));
        assert_eq!(of("d").unfit, Some(Unfit::FutureRead));
        assert_eq!(of("e").unfit, Some(Unfit::Blocked));
        assert_eq!(of("e").room_percent, Some(0));
        assert_eq!(of("f").unfit, Some(Unfit::Stale));
        assert_eq!(of("g").unfit, Some(Unfit::Unknown));
        assert_eq!(of("g").room_percent, None, "a refusal is not 100% room");
        assert_eq!(of("a").room_percent, Some(4));
        assert_eq!(of("a").unfit, None);
    }

    #[test]
    fn a_tie_on_room_goes_to_the_table_org_order_then_the_id_and_a_cooldown_holds_still() {
        let gauges = [
            gauge("a", "claude_max", &[("session", 95, HOUR)]),
            gauge("y", "claude_max", &[("session", 30, HOUR)]),
            gauge("x", "claude_team", &[("session", 30, HOUR)]),
            gauge("w", "claude_max", &[("session", 30, HOUR)]),
        ];
        let ceiling = CLAUDE_ACCOUNT_AUTOSWITCH.default_moves_at_percent;
        let best = best_candidate("a", &gauges, None, ceiling, NOW).expect("a candidate");
        assert_eq!(best.id, "x", "team wins the tie");
        let gauges = [
            gauge("a", "claude_max", &[("session", 95, HOUR)]),
            gauge("y", "claude_max", &[("session", 30, HOUR)]),
            gauge("w", "claude_max", &[("session", 30, HOUR)]),
        ];
        assert_eq!(
            best_candidate("a", &gauges, None, ceiling, NOW)
                .expect("a candidate")
                .id,
            "w",
            "the same org: the lower id, every time"
        );
        let mut cooled = ask("a", &gauges);
        cooled.last_switch_ms = Some(NOW - CLAUDE_ACCOUNT_AUTOSWITCH.cooldown_ms + 1);
        assert_eq!(decide(&cooled), Decision::Stay { why: "cooldown" });
        cooled.last_switch_ms = Some(NOW - CLAUDE_ACCOUNT_AUTOSWITCH.cooldown_ms);
        assert!(matches!(decide(&cooled), Decision::Switch { .. }));
    }

    /// A percentage is a share of its own plan's window: a team seat with
    /// room under the move line is spent before a Max plan with more
    /// percent free, and room only orders accounts of one type (astra A3).
    #[test]
    fn the_plan_type_is_read_before_the_percentage() {
        let gauges = [
            gauge("a", "claude_max", &[("session", 95, HOUR)]),
            gauge("max", "claude_max", &[("session", 5, HOUR)]),
            gauge("team", "claude_team", &[("session", 80, HOUR)]),
        ];
        assert!(matches!(
            decide(&ask("a", &gauges)),
            Decision::Switch { ref to, .. } if to == "team"
        ));
        // A team seat past the move line is not a place to move the default
        // to; the Max plan is.
        let gauges = [
            gauge("a", "claude_max", &[("session", 95, HOUR)]),
            gauge("max", "claude_max", &[("session", 5, HOUR)]),
            gauge("team", "claude_team", &[("session", 91, HOUR)]),
        ];
        assert!(matches!(
            decide(&ask("a", &gauges)),
            Decision::Switch { ref to, .. } if to == "max"
        ));
    }

    /// The default must land UNDER the move line, or the next beat moves it
    /// straight back: A at 95% and B at 92% is no switch at all — while a
    /// walled pane takes any room under the blocked line (astra B1: the
    /// preemptive default move and the walled pane read one policy with two
    /// effects).
    #[test]
    fn a_candidate_already_past_the_move_line_is_no_place_for_the_default() {
        let gauges = [
            gauge("a", "claude_max", &[("session", 95, HOUR)]),
            gauge("b", "claude_max", &[("session", 92, HOUR)]),
        ];
        assert_eq!(
            decide(&ask("a", &gauges)),
            Decision::Stay {
                why: "no_candidate"
            }
        );
        let mut walled = ask("a", &gauges);
        walled.walled = true;
        assert!(matches!(
            decide(&walled),
            Decision::Switch { ref to, reason: SwitchReason::Walled, .. } if to == "b"
        ));
        // And back the other way: after the move B at 93% finds A at 95% —
        // past the line — and stays. No ping-pong.
        assert_eq!(
            decide(&ask("b", &gauges)),
            Decision::Stay {
                why: "no_candidate"
            }
        );
    }

    /// Two rows of the store that are one login are one quota: "switching"
    /// between them moves nothing (astra A3).
    #[test]
    fn two_rows_of_one_login_are_not_a_switch() {
        let mut twin = gauge("a2", "claude_max", &[("session", 0, HOUR)]);
        twin.identity = Some("login-of-a".to_string());
        let gauges = [gauge("a", "claude_max", &[("session", 95, HOUR)]), twin];
        assert_eq!(
            decide(&ask("a", &gauges)),
            Decision::Stay {
                why: "no_candidate"
            }
        );
        // Rows whose login nobody recorded are not assumed to be one.
        let mut unrecorded = gauges.clone();
        unrecorded[0].identity = None;
        unrecorded[1].identity = None;
        assert!(matches!(
            decide(&ask("a", &unrecorded)),
            Decision::Switch { .. }
        ));
    }

    /// A Fable-only week limits Fable and nothing else (astra A3): a pane
    /// running Opus is not walled by it and an account whose Fable week is
    /// spent is still room for it; the default serves every model, so every
    /// window counts for it.
    #[test]
    fn a_fable_only_week_limits_fable_and_nothing_else() {
        assert!(limits("session", Some("claude-opus-5-5")));
        assert!(limits("fable_weekly", Some("claude-fable-5-1")));
        assert!(limits("fable_weekly", Some("fable")));
        assert!(!limits("fable_weekly", Some("claude-opus-5-5")));
        assert!(!limits("fable_weekly", Some("opus[1m]")));
        assert!(limits("fable_weekly", None), "the default serves Fable too");
        assert!(limits(
            "a_window_nobody_wrote_down",
            Some("claude-opus-5-5")
        ));
        let gauges = [
            gauge(
                "a",
                "claude_max",
                &[("session", 20, HOUR), ("fable_weekly", 100, DAYS6)],
            ),
            gauge(
                "b",
                "claude_max",
                &[("session", 30, HOUR), ("fable_weekly", 10, DAYS6)],
            ),
        ];
        // The default: A's Fable week crossed, so new launches move.
        assert!(matches!(
            decide(&ask("a", &gauges)),
            Decision::Switch { reason: SwitchReason::NearLimit { ref window, .. }, .. }
                if window == "fable_weekly"
        ));
        // An Opus launch on A has room.
        let mut opus = ask("a", &gauges);
        opus.model = Some("claude-opus-5-5");
        assert_eq!(decide(&opus), Decision::Stay { why: "room" });
        // A walled Fable pane on B may land on A only if A's Fable week has
        // room — it does not; an Opus pane may.
        let walled_b = [
            gauge(
                "a",
                "claude_max",
                &[("session", 20, HOUR), ("fable_weekly", 100, DAYS6)],
            ),
            gauge(
                "b",
                "claude_max",
                &[("session", 99, HOUR), ("fable_weekly", 10, DAYS6)],
            ),
        ];
        assert_eq!(
            judge_pane("b", Some("claude-fable-5-1"), "a", &walled_b, NOW),
            Decision::Stay {
                why: "no_candidate"
            }
        );
        assert!(matches!(
            judge_pane("b", Some("claude-opus-5-5"), "a", &walled_b, NOW),
            Decision::Switch { ref to, .. } if to == "a"
        ));
    }

    /// One walled pane goes to the landing default and nowhere else: not to
    /// its own account, not to its own login under another row, not to a
    /// landing without room for it, and not before a reset that is near.
    #[test]
    fn a_walled_pane_goes_to_the_landing_only_when_it_has_room_and_the_reset_is_not_near() {
        let gauges = [
            gauge(
                "a",
                "claude_max",
                &[("session", 99, HOUR), ("weekly", 60, DAYS6)],
            ),
            gauge(
                "b",
                "claude_max",
                &[("session", 30, HOUR), ("weekly", 40, DAYS6)],
            ),
        ];
        assert_eq!(
            judge_pane("a", None, "b", &gauges, NOW),
            Decision::Switch {
                from: "a".into(),
                to: "b".into(),
                reason: SwitchReason::Walled,
            }
        );
        assert_eq!(
            judge_pane("a", None, "a", &gauges, NOW),
            Decision::Stay {
                why: "no_candidate"
            }
        );
        assert_eq!(
            judge_pane("a", None, "nobody", &gauges, NOW),
            Decision::Stay { why: "unknown" }
        );
        let mut twin = gauges.clone();
        twin[1].identity = twin[0].identity.clone();
        assert_eq!(
            judge_pane("a", None, "b", &twin, NOW),
            Decision::Stay {
                why: "no_candidate"
            }
        );
        let mut full = gauges.clone();
        full[1].windows[0].used_percent = 97;
        assert_eq!(
            judge_pane("a", None, "b", &full, NOW),
            Decision::Stay {
                why: "no_candidate"
            }
        );
        let mut soon = gauges.clone();
        soon[0].windows[0].resets_at_ms = Some(NOW + 3 * MIN);
        assert_eq!(
            judge_pane("a", None, "b", &soon, NOW),
            Decision::Wait {
                until_ms: NOW + 3 * MIN,
                minutes: 3
            }
        );
    }

    #[test]
    fn the_mode_words_are_the_three_the_setting_offers() {
        assert_eq!(AutoSwitchMode::of("auto"), Some(AutoSwitchMode::Auto));
        assert_eq!(AutoSwitchMode::of(" ASK "), Some(AutoSwitchMode::Ask));
        assert_eq!(AutoSwitchMode::of("off"), Some(AutoSwitchMode::Off));
        assert_eq!(AutoSwitchMode::of("maybe"), None);
        assert_eq!(AutoSwitchMode::default(), AutoSwitchMode::Ask);
        assert!(!AutoSwitchMode::Off.acts());
        assert!(AutoSwitchMode::Ask.acts());
        let json = serde_json::to_string(&AutoSwitchMode::Auto).unwrap();
        assert_eq!(json, "\"auto\"");
    }

    #[test]
    fn the_table_reads_the_summons_tables_numbers_and_invents_none() {
        let policy = &CLAUDE_ACCOUNT_AUTOSWITCH;
        assert_eq!(policy.default_moves_at_percent, QUOTA_POLICY.warn_percent);
        assert_eq!(policy.blocked_at_percent, QUOTA_POLICY.wall_percent);
        assert_eq!(policy.gauge_max_age_ms, QUOTA_POLICY.snapshot_max_age_ms);
        assert_eq!(policy.near_reset_ms, QUOTA_POLICY.reset_soon_ms);
        assert_eq!(policy.cooldown_ms, QUOTA_WAIT_POLICY.lift_read_ms);
        assert!(policy.default_moves_at_percent < policy.blocked_at_percent);
        assert!(
            policy.near_reset_ms < QUOTA_WAIT_POLICY.max_wait_ms,
            "near is not the wall's lifetime"
        );
        // The identity never crosses into an answer.
        let row = gauge("a", "claude_max", &[("session", 1, HOUR)]);
        let json = serde_json::to_string(&row).unwrap();
        assert!(!json.contains("login-of-a"), "{json}");
    }
}
