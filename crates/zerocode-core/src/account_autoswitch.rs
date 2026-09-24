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
    /// At or above this on ANY window, the default account is moved for
    /// new launches. `QUOTA_POLICY.warn_percent`: "한도 앞에서".
    pub default_moves_at_percent: u8,
    /// At or above this a window is BLOCKED and the account cannot be a
    /// candidate; a pane on it at the two-witness wall is reseated.
    /// `QUOTA_POLICY.wall_percent`.
    pub blocked_at_percent: u8,
    /// A gauge older than this is not a reading anybody may choose by.
    /// `QUOTA_POLICY.snapshot_max_age_ms`.
    pub gauge_max_age_ms: i64,
    /// A reset this close is waited out instead of switched around — the
    /// summons gate's own "ask again in N min". `QUOTA_POLICY.reset_soon_ms`.
    pub near_reset_ms: i64,
    /// After a switch, how long the beat leaves the default where it is,
    /// whatever the numbers say. One usage re-read floor
    /// (`QUOTA_WAIT_POLICY.lift_read_ms`, the window's `MIN_REFETCH`): inside
    /// it the gauges cannot even have been read again, so a second move
    /// would be a move on the same numbers.
    pub cooldown_ms: i64,
    /// Which organisation type wins a tie on room, first is best. Team
    /// before Max before the rest: a team plan's window is the shared one
    /// and the one somebody else is also spending, so it is spent first
    /// while it has room and the personal Max plan is kept as the reserve.
    pub org_type_order: &'static [&'static str],
}

/// The policy as it stands (2026-09-24, t-7538).
pub const CLAUDE_ACCOUNT_AUTOSWITCH: AutoSwitchPolicy = AutoSwitchPolicy {
    default_moves_at_percent: QUOTA_POLICY.warn_percent,
    blocked_at_percent: QUOTA_POLICY.wall_percent,
    gauge_max_age_ms: QUOTA_POLICY.snapshot_max_age_ms,
    near_reset_ms: QUOTA_POLICY.reset_soon_ms,
    cooldown_ms: QUOTA_WAIT_POLICY.lift_read_ms,
    org_type_order: &["claude_team", "claude_max"],
};

/// One provider window of one account, as the window's usage cache holds it
/// — the percentage the provider reported and when that window resets.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GaugeWindow {
    /// `session`, `weekly`, `fable_weekly` — the cache's own names.
    pub kind: String,
    pub used_percent: u8,
    pub resets_at_ms: Option<i64>,
}

/// One account's reading, id and numbers only.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AccountGauge {
    pub id: String,
    /// The CLI's `organizationType` word, when the store recorded one.
    pub org_type: Option<String>,
    /// Every window the last read reported. Empty for a read that failed —
    /// which is an UNKNOWN, never a 0%.
    pub windows: Vec<GaugeWindow>,
    /// When the read behind `windows` finished, epoch ms.
    pub observed_at_ms: i64,
    /// The read's own status word (`ok`, `error`, `signed_out`, …). Only
    /// `ok` is a reading; anything else keeps the account out of the choice.
    pub status: String,
}

/// Why an account cannot be chosen right now, said as one word for a UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unfit {
    /// No successful read, or no window in it.
    Unknown,
    /// The read is older than the table allows.
    Stale,
    /// The reading is from the future — a clock nobody trusts.
    FutureRead,
    /// At least one window is at or past the blocked line.
    Blocked,
}

/// What one account's gauge says about choosing it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Fitness {
    pub id: String,
    /// Percent of room left on the FULLEST window — the one that will refuse
    /// first. `None` when the account is unfit.
    pub room_percent: Option<u8>,
    pub unfit: Option<Unfit>,
    /// The earliest reset among the windows at or past the default-moves
    /// line, when there is one.
    pub next_reset_ms: Option<i64>,
}

fn fitness(gauge: &AccountGauge, now_ms: i64) -> Fitness {
    let policy = &CLAUDE_ACCOUNT_AUTOSWITCH;
    let unfit = if gauge.status != "ok" || gauge.windows.is_empty() {
        Some(Unfit::Unknown)
    } else if gauge.observed_at_ms > now_ms {
        Some(Unfit::FutureRead)
    } else if now_ms.saturating_sub(gauge.observed_at_ms) > policy.gauge_max_age_ms
        || gauge
            .windows
            .iter()
            .any(|window| window.resets_at_ms.is_some_and(|at| at <= now_ms))
    {
        Some(Unfit::Stale)
    } else if gauge
        .windows
        .iter()
        .any(|window| window.used_percent >= policy.blocked_at_percent)
    {
        Some(Unfit::Blocked)
    } else {
        None
    };
    let fullest = gauge
        .windows
        .iter()
        .map(|window| window.used_percent.min(100))
        .max();
    let next_reset_ms = gauge
        .windows
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

/// What the beat should do about the default account.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Decision {
    /// Nothing: the source is fine, or nobody better exists. `why` is one
    /// word the status bar can show (`room`, `alone`, `no_candidate`,
    /// `cooldown`, `unknown`).
    Stay { why: &'static str },
    /// Every window that crossed the line resets within the table's near
    /// window: wait for it rather than move.
    Wait { until_ms: i64, minutes: u64 },
    /// Move the default from `from` to `to`.
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
    /// When the default last moved, if the beat moved it.
    pub last_switch_ms: Option<i64>,
    /// Whether a pane on `source` stands at its two-witness wall — the
    /// second trigger, which does not wait for the source's own gauge to
    /// say so (the wall's witness already did).
    pub walled: bool,
    pub now_ms: i64,
}

/// The candidate with the most room, by the table's tie-breaks; `None` when
/// nobody fit exists other than the source.
#[must_use]
pub fn best_candidate(source: &str, gauges: &[AccountGauge], now_ms: i64) -> Option<Fitness> {
    let order = CLAUDE_ACCOUNT_AUTOSWITCH.org_type_order;
    let rank = |id: &str| {
        let org = gauges
            .iter()
            .find(|gauge| gauge.id == id)
            .and_then(|gauge| gauge.org_type.as_deref())
            .unwrap_or_default();
        order
            .iter()
            .position(|held| *held == org)
            .unwrap_or(order.len())
    };
    gauges
        .iter()
        .filter(|gauge| gauge.id != source)
        .map(|gauge| fitness(gauge, now_ms))
        .filter(|fit| fit.unfit.is_none())
        // Most room first; on a tie the table's organisation order; then the
        // id, so two runs of the same numbers name the same account.
        .max_by(|left, right| {
            left.room_percent
                .cmp(&right.room_percent)
                .then_with(|| rank(&right.id).cmp(&rank(&left.id)))
                .then_with(|| right.id.cmp(&left.id))
        })
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
    let fit = fitness(source, now_ms);
    // The trigger: the wall's witness, or the source's own fullest window at
    // the default-moves line — read only off a reading the table trusts.
    let crossed = (fit.unfit.is_none() || fit.unfit == Some(Unfit::Blocked))
        .then(|| {
            source
                .windows
                .iter()
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
    } else {
        return Decision::Stay { why: "room" };
    };
    // Near a reset, the answer is patience — but only when EVERY window that
    // crossed the line resets inside the near window. A session that resets
    // in a minute does not excuse a week that is at 100% for six days.
    let crossed_windows: Vec<&GaugeWindow> = source
        .windows
        .iter()
        .filter(|window| window.used_percent >= policy.default_moves_at_percent)
        .collect();
    if !question.walled && !crossed_windows.is_empty() {
        let latest = crossed_windows
            .iter()
            .map(|window| window.resets_at_ms)
            .collect::<Option<Vec<i64>>>()
            .and_then(|resets| resets.into_iter().max());
        if let Some(at) = latest
            && at > now_ms
            && at.saturating_sub(now_ms) <= policy.near_reset_ms
        {
            return Decision::Wait {
                until_ms: at,
                minutes: minutes_up(at.saturating_sub(now_ms)),
            };
        }
    }
    match best_candidate(question.source, question.gauges, now_ms) {
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

/// Every account's fitness, for the accounts pane's table.
#[must_use]
pub fn fitness_table(gauges: &[AccountGauge], now_ms: i64) -> Vec<Fitness> {
    gauges.iter().map(|gauge| fitness(gauge, now_ms)).collect()
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
        // two others with room. The one with the most room wins.
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
                "claude_team",
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
        // A wall's witness does not wait: the pane is already refused.
        let mut walled = ask("a", &gauges);
        walled.walled = true;
        assert!(matches!(
            decide(&walled),
            Decision::Switch {
                reason: SwitchReason::Walled,
                ..
            }
        ));
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
        assert_eq!(best_candidate("a", &alone, NOW), None);
        assert_eq!(
            decide(&ask("zz", &gauges)),
            Decision::Stay { why: "unknown" }
        );
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
        let source = gauge("a", "claude_max", &[("session", 96, HOUR)]);
        let gauges = [source, unknown, stale, future, blocked, reset];
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
        let best = best_candidate("a", &gauges, NOW).expect("a candidate");
        assert_eq!(best.id, "x", "team wins the tie");
        let gauges = [
            gauge("a", "claude_max", &[("session", 95, HOUR)]),
            gauge("y", "claude_max", &[("session", 30, HOUR)]),
            gauge("w", "claude_max", &[("session", 30, HOUR)]),
        ];
        assert_eq!(
            best_candidate("a", &gauges, NOW).expect("a candidate").id,
            "w",
            "the same org: the lower id, every time"
        );
        let mut cooled = ask("a", &gauges);
        cooled.last_switch_ms = Some(NOW - CLAUDE_ACCOUNT_AUTOSWITCH.cooldown_ms + 1);
        assert_eq!(decide(&cooled), Decision::Stay { why: "cooldown" });
        cooled.last_switch_ms = Some(NOW - CLAUDE_ACCOUNT_AUTOSWITCH.cooldown_ms);
        assert!(matches!(decide(&cooled), Decision::Switch { .. }));
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
    }
}
