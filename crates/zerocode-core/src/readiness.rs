//! The agent readiness snapshot: what this machine last saw of whether an
//! agent can be started — its binary and its login — as ONE observation
//! with its time on it, read by purpose.
//!
//! Modelled on agent-orchestrator's `AgentReadinessProvider`
//! (`EnsureAgentReadiness(agent, purpose)`, `Invalidate…`, `Recheck`) and
//! its `domain.AgentReadinessSnapshot`. The shape is the design; the checks
//! behind it are the ones the window's account modules already keep, and
//! this module runs none of them — it holds what a probe found, says how old
//! that is, and decides whether a purpose may still read it.
//!
//! Two purposes, two freshness windows. A row on screen may show a
//! five-minute-old verdict: the cost of a stale 「로그인 필요」 is a glance.
//! A worker summons may not: a briefing pasted into a login screen is the
//! P0 this snapshot exists to keep from happening, so a launch re-checks
//! anything older than half a minute. A display-fresh snapshot therefore
//! never stands in for a launch check, and the numbers are one table with
//! its settings overlay ([`Limits`]) rather than two constants in two
//! callers.
//!
//! Three login states, not two. `Unknown` is "no witness answered" — an
//! agent with no local witness at all, a home this window may not read, a
//! probe that could not run — and it is never folded into `Unauthorized`:
//! the row that word marks says 「로그인 필요」, which is a diagnosis, and the
//! same discipline `accounts::login_alive` states for the CLI probe holds
//! here ("could not read" must not mark anybody).

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

/// What the local witnesses last said about the agent's login.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthState {
    /// A witness names a login.
    Authorized,
    /// Every witness that answered says there is none.
    Unauthorized,
    /// No witness answered, or the agent has none.
    Unknown,
}

impl AuthState {
    /// The wire word, for a sentence that carries the state.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Authorized => "authorized",
            Self::Unauthorized => "unauthorized",
            Self::Unknown => "unknown",
        }
    }
}

/// Whether the agent's binary was on the launch PATH when the probe looked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum BinaryState {
    /// Resolved to this file on the PATH a launch would use.
    Present {
        path: String,
    },
    Missing,
}

/// Who is reading the snapshot, which decides how old it may be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// A picker row, a settings row, `agent-list`: a look.
    Display,
    /// A worker summons: the briefing is about to be pasted.
    Launch,
}

/// Every number the readiness snapshot has, in one place.
///
/// A settings overlay (`readiness.<field>`) is the only road to a different
/// value; see [`Limits::overlaid`]. The floor exists because a zero would
/// re-run the witnesses — a keychain read, `gh auth status` — on every
/// paint; the overlay may shorten a window, never remove the cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Limits {
    /// How old a snapshot may be for a look, in milliseconds.
    pub display_fresh_ms: u64,
    /// How old a snapshot may be for a launch, in milliseconds.
    pub launch_fresh_ms: u64,
    /// The least either window may be set to.
    pub fresh_ms_min: u64,
    /// Characters the evidence line keeps — a short reason, never a dump.
    pub evidence_chars: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            display_fresh_ms: 5 * 60 * 1_000,
            launch_fresh_ms: 30 * 1_000,
            fresh_ms_min: 1_000,
            evidence_chars: 160,
        }
    }
}

/// The overlay's key prefix; `readiness.launch_fresh_ms` names
/// [`Limits::launch_fresh_ms`].
pub const OVERLAY_PREFIX: &str = "readiness.";

impl Limits {
    /// Lay a settings overlay over the table. Unknown keys are ignored, the
    /// windows are floored, and every other field takes the value as given —
    /// the table is the authority on what exists, the overlay only on what
    /// it says (the contract `checks::Limits` keeps).
    #[must_use]
    pub fn overlaid(mut self, overlay: &BTreeMap<String, u64>) -> Self {
        for (key, value) in overlay {
            let Some(field) = key.strip_prefix(OVERLAY_PREFIX) else {
                continue;
            };
            let value = *value;
            match field {
                "display_fresh_ms" => self.display_fresh_ms = value.max(self.fresh_ms_min),
                "launch_fresh_ms" => self.launch_fresh_ms = value.max(self.fresh_ms_min),
                "evidence_chars" => {
                    self.evidence_chars = usize::try_from(value).unwrap_or(usize::MAX);
                }
                _ => {}
            }
        }
        self
    }

    /// How old a snapshot may be for this purpose.
    #[must_use]
    pub const fn fresh_ms(&self, purpose: Purpose) -> u64 {
        match purpose {
            Purpose::Display => self.display_fresh_ms,
            Purpose::Launch => self.launch_fresh_ms,
        }
    }
}

/// One agent, as this machine last observed it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentReadinessSnapshot {
    pub agent: String,
    pub binary: BinaryState,
    pub auth: AuthState,
    /// When the witnesses were asked, epoch milliseconds.
    pub observed_at_ms: i64,
    /// Which witnesses answered and what they said — a short reason with
    /// no secret in it (a file's presence, a keychain item's state, a gh
    /// host), so it can ride a refusal and a tooltip.
    pub evidence: String,
}

impl AgentReadinessSnapshot {
    /// The snapshot of an agent nobody could observe, with the reason.
    #[must_use]
    pub fn unknown(agent: &str, evidence: &str, observed_at_ms: i64) -> Self {
        Self {
            agent: agent.to_string(),
            binary: BinaryState::Missing,
            auth: AuthState::Unknown,
            observed_at_ms,
            evidence: evidence.to_string(),
        }
    }

    /// How long ago the witnesses were asked; never negative.
    #[must_use]
    pub fn age_ms(&self, now_ms: i64) -> u64 {
        u64::try_from(now_ms.saturating_sub(self.observed_at_ms)).unwrap_or(0)
    }

    /// Whether this purpose may still read the snapshot.
    #[must_use]
    pub fn is_fresh(&self, purpose: Purpose, now_ms: i64, limits: &Limits) -> bool {
        self.age_ms(now_ms) <= limits.fresh_ms(purpose)
    }

    /// The evidence, cut to the table's length.
    #[must_use]
    pub fn trimmed(mut self, limits: &Limits) -> Self {
        if self.evidence.chars().count() > limits.evidence_chars {
            self.evidence = self.evidence.chars().take(limits.evidence_chars).collect();
        }
        self
    }
}

/// The snapshots this process holds, one per agent.
///
/// The cache decides nothing about an agent: it answers "is what I hold
/// still fresh for you", hands the caller what it holds, and takes what a
/// probe found. Probing is the caller's — so the caller can run the
/// witnesses outside whatever lock guards this table.
#[derive(Debug, Default)]
pub struct ReadinessCache {
    rows: HashMap<String, AgentReadinessSnapshot>,
}

impl ReadinessCache {
    /// The held snapshot when it is fresh for this purpose; `None` is "probe
    /// again", whether because nothing is held or because it is too old.
    #[must_use]
    pub fn fresh(
        &self,
        agent: &str,
        purpose: Purpose,
        now_ms: i64,
        limits: &Limits,
    ) -> Option<&AgentReadinessSnapshot> {
        self.rows
            .get(agent)
            .filter(|held| held.is_fresh(purpose, now_ms, limits))
    }

    /// The held snapshot, whatever its age.
    #[must_use]
    pub fn peek(&self, agent: &str) -> Option<&AgentReadinessSnapshot> {
        self.rows.get(agent)
    }

    /// Take what a probe found. The newest observation wins; a probe that
    /// finished after a later one started is not allowed to roll the table
    /// back.
    pub fn insert(&mut self, snapshot: AgentReadinessSnapshot) {
        match self.rows.get(&snapshot.agent) {
            Some(held) if held.observed_at_ms > snapshot.observed_at_ms => {}
            _ => {
                self.rows.insert(snapshot.agent.clone(), snapshot);
            }
        }
    }

    /// The fresh snapshot, or the probe's answer, remembered. The
    /// convenience road for a caller that is not holding a lock across it.
    pub fn ensure(
        &mut self,
        agent: &str,
        purpose: Purpose,
        now_ms: i64,
        limits: &Limits,
        probe: impl FnOnce() -> AgentReadinessSnapshot,
    ) -> AgentReadinessSnapshot {
        if let Some(held) = self.fresh(agent, purpose, now_ms, limits) {
            return held.clone();
        }
        let found = probe().trimmed(limits);
        self.insert(found.clone());
        found
    }

    /// Forget one agent's snapshot: the world it described changed.
    pub fn invalidate(&mut self, agent: &str) -> bool {
        self.rows.remove(agent).is_some()
    }

    /// Forget every snapshot the predicate names — an account switch touches
    /// every agent on that provider, a PATH re-read touches them all.
    pub fn invalidate_where(&mut self, mut stale: impl FnMut(&str) -> bool) -> usize {
        let before = self.rows.len();
        self.rows.retain(|agent, _| !stale(agent));
        before - self.rows.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(agent: &str, auth: AuthState, at: i64) -> AgentReadinessSnapshot {
        AgentReadinessSnapshot {
            agent: agent.to_string(),
            binary: BinaryState::Present {
                path: format!("/opt/bin/{agent}"),
            },
            auth,
            observed_at_ms: at,
            evidence: format!("fixture: {auth:?}"),
        }
    }

    /// The three states and the two binary states have their wire words,
    /// and a snapshot round-trips as one object.
    #[test]
    fn the_three_login_states_and_the_binary_have_wire_words() {
        let words = |auth: AuthState| serde_json::to_value(auth).expect("json");
        assert_eq!(words(AuthState::Authorized), "authorized");
        assert_eq!(words(AuthState::Unauthorized), "unauthorized");
        assert_eq!(words(AuthState::Unknown), "unknown");
        let snapshot = seen("claude", AuthState::Authorized, 1_000);
        let json = serde_json::to_value(&snapshot).expect("json");
        assert_eq!(
            json,
            serde_json::json!({
                "agent": "claude",
                "binary": { "state": "present", "path": "/opt/bin/claude" },
                "auth": "authorized",
                "observedAtMs": 1_000,
                "evidence": "fixture: Authorized",
            })
        );
        assert_eq!(
            serde_json::from_value::<AgentReadinessSnapshot>(json).expect("back"),
            snapshot
        );
        assert_eq!(
            serde_json::to_value(BinaryState::Missing).expect("json"),
            serde_json::json!({ "state": "missing" })
        );
        let unknown = AgentReadinessSnapshot::unknown("kimi", "no local witness", 5);
        assert_eq!(unknown.auth, AuthState::Unknown);
        assert_eq!(unknown.binary, BinaryState::Missing);
        assert_eq!(unknown.evidence, "no local witness");
    }

    /// A display-fresh snapshot never stands in for a launch check: the
    /// same two-minute-old row is served to a look and re-probed for a
    /// summons.
    #[test]
    fn a_stale_display_snapshot_does_not_answer_a_launch() {
        let limits = Limits::default();
        let mut cache = ReadinessCache::default();
        let now = 10 * 60 * 1_000;
        let two_minutes_ago = now - 2 * 60 * 1_000;
        cache.insert(seen("claude", AuthState::Authorized, two_minutes_ago));

        let mut probed = 0;
        let looked = cache.ensure("claude", Purpose::Display, now, &limits, || {
            probed += 1;
            seen("claude", AuthState::Unauthorized, now)
        });
        assert_eq!(probed, 0, "a look re-ran the witnesses on a fresh row");
        assert_eq!(looked.auth, AuthState::Authorized);

        let launched = cache.ensure("claude", Purpose::Launch, now, &limits, || {
            probed += 1;
            seen("claude", AuthState::Unauthorized, now)
        });
        assert_eq!(probed, 1, "a launch read a two-minute-old verdict");
        assert_eq!(launched.auth, AuthState::Unauthorized);
        // And the launch's fresh answer is what the next look reads.
        assert_eq!(
            cache
                .fresh("claude", Purpose::Display, now, &limits)
                .map(|held| held.auth),
            Some(AuthState::Unauthorized)
        );
        // The boundary is inclusive on both windows.
        let at_edge = seen(
            "codex",
            AuthState::Authorized,
            now - limits.launch_fresh_ms as i64,
        );
        cache.insert(at_edge);
        assert!(
            cache
                .fresh("codex", Purpose::Launch, now, &limits)
                .is_some()
        );
        assert!(
            cache
                .fresh("codex", Purpose::Launch, now + 1, &limits)
                .is_none()
        );
    }

    /// Invalidation is what an account switch, a login and an install do
    /// to the table: the next reader of any purpose probes again, and only
    /// the named rows go.
    #[test]
    fn invalidated_rows_are_probed_again_and_the_rest_stand() {
        let limits = Limits::default();
        let mut cache = ReadinessCache::default();
        let now = 1_000;
        cache.insert(seen("claude", AuthState::Authorized, now));
        cache.insert(seen("codex", AuthState::Authorized, now));
        cache.insert(seen("zo", AuthState::Authorized, now));
        assert!(cache.invalidate("claude"));
        assert!(
            !cache.invalidate("claude"),
            "a second invalidation found a row"
        );
        let mut probed = 0;
        let again = cache.ensure("claude", Purpose::Display, now, &limits, || {
            probed += 1;
            seen("claude", AuthState::Unauthorized, now)
        });
        assert_eq!((probed, again.auth), (1, AuthState::Unauthorized));
        assert!(
            cache
                .fresh("codex", Purpose::Display, now, &limits)
                .is_some()
        );
        // By predicate: the provider's agents, and nobody else's.
        assert_eq!(cache.invalidate_where(|agent| agent != "codex"), 2);
        assert!(cache.peek("codex").is_some());
        assert!(cache.peek("zo").is_none() && cache.peek("claude").is_none());
    }

    /// The newest observation wins, whichever probe finished last.
    #[test]
    fn an_older_observation_cannot_roll_the_table_back() {
        let mut cache = ReadinessCache::default();
        cache.insert(seen("claude", AuthState::Unauthorized, 2_000));
        cache.insert(seen("claude", AuthState::Authorized, 1_000));
        assert_eq!(
            cache.peek("claude").map(|held| held.auth),
            Some(AuthState::Unauthorized)
        );
        cache.insert(seen("claude", AuthState::Authorized, 3_000));
        assert_eq!(
            cache.peek("claude").map(|held| held.auth),
            Some(AuthState::Authorized)
        );
    }

    /// The numbers are the table's, an overlay may move them but not below
    /// the floor, and an unknown key changes nothing.
    #[test]
    fn the_overlay_moves_the_windows_and_the_floor_holds() {
        let table = Limits::default();
        assert_eq!(table.fresh_ms(Purpose::Display), 300_000);
        assert_eq!(table.fresh_ms(Purpose::Launch), 30_000);
        let overlay: BTreeMap<String, u64> = [
            ("readiness.display_fresh_ms".to_string(), 60_000),
            ("readiness.launch_fresh_ms".to_string(), 0),
            ("readiness.evidence_chars".to_string(), 12),
            ("readiness.not_a_field".to_string(), 7),
            ("checks.poll_ms".to_string(), 1),
        ]
        .into_iter()
        .collect();
        let moved = table.overlaid(&overlay);
        assert_eq!(moved.display_fresh_ms, 60_000);
        assert_eq!(
            moved.launch_fresh_ms, table.fresh_ms_min,
            "the floor gave way"
        );
        assert_eq!(moved.evidence_chars, 12);
        assert_eq!(
            seen("claude", AuthState::Authorized, 0)
                .trimmed(&moved)
                .evidence,
            "fixture: Aut"
        );
        assert_eq!(Limits::default().overlaid(&BTreeMap::new()), table);
    }

    /// Age never goes negative: a clock that stepped back reads as "just
    /// now", not as a snapshot from the future that is fresh forever.
    #[test]
    fn age_is_never_negative() {
        let held = seen("claude", AuthState::Authorized, 5_000);
        assert_eq!(held.age_ms(4_000), 0);
        assert_eq!(held.age_ms(5_250), 250);
    }
}
