//! How long the conversation's prompt cache lives, and how many times an idle
//! conversation touches it to keep it — chosen per model from this machine's
//! own request ledger and the model's list prices, never written down.
//!
//! Anthropic offers a marker two lifetimes, five minutes and an hour, and
//! bills writing at 1.25× and 2× input (the price table's
//! `cache_write_5m_rate` / `cache_write_1h_rate`). A read refreshes the entry
//! at the read price, the row's own multiple of input — 0.05× on Opus 5.5 and
//! Sonnet 5.5, 0.025× on Fable 5.1, the table's 0.1× on a row that names no
//! multiple of its own — and the lifetime runs from the start of the
//! request that last wrote or read it (platform.claude.com prompt-caching and
//! pricing pages, read 2026-10-28). So there are three ways to carry a
//! conversation across a person's pause, and which is cheapest depends on two
//! things only the machine knows:
//!
//! * the pauses themselves — how often a turn waits five minutes, twenty, an
//!   hour, or for ever (the conversation ended), and how large the prefix
//!   waiting is;
//! * how much a request writes that any lifetime would write, because the
//!   hour's premium lands on every write, not only on the ones a pause
//!   follows.
//!
//! The three: let a five-minute entry go (a pause past it rewrites the whole
//! prefix); write every entry for an hour (the premium on every write); or
//! keep the five-minute entry and, while the conversation is idle, re-send
//! its last request with `max_tokens: 0` just before the entry would expire
//! — a *refresh*, billed as a cache read of the prefix and nothing else.
//! [`cheapest_plan`] prices every lifetime with every refresh count against
//! the recorded pauses and keeps the cheapest. On this machine's fourteen
//! days to 2026-10-28 (53,192 foreground Anthropic requests, Opus 5.5 rates)
//! the lifetime-dependent part of a request's input cost came to 7,409
//! input-token equivalents with no refresh, 12,307 with the hour, and 2,727
//! with five minutes and up to ten refreshes. Another machine's pauses or
//! another model's prices can choose differently; nothing here assumes the
//! answer.

use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::types::CacheControl;

/// A lifetime Anthropic's prompt cache offers a `cache_control` marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum CacheTtl {
    /// The provider's default, written at the cheaper rate.
    #[default]
    FiveMinutes,
    /// Survives a longer pause, written at the dearer rate.
    OneHour,
}

impl CacheTtl {
    /// Every lifetime the provider offers, shortest first — the order a tie
    /// is broken in.
    pub const ALL: [Self; 2] = [Self::FiveMinutes, Self::OneHour];

    /// The provider's `ttl` spelling, which a setting and the ledger's `ttl`
    /// column use too.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::FiveMinutes => "5m",
            Self::OneHour => "1h",
        }
    }

    /// The lifetime a label names, `None` for anything else.
    #[must_use]
    pub fn from_label(label: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|ttl| ttl.label().eq_ignore_ascii_case(label.trim()))
    }

    /// How long an entry lives after the start of the request that last
    /// wrote or read it.
    #[must_use]
    pub const fn lifetime(self) -> Duration {
        match self {
            Self::FiveMinutes => Duration::from_secs(5 * 60),
            Self::OneHour => Duration::from_secs(60 * 60),
        }
    }

    /// The marker that asks for this lifetime.
    #[must_use]
    pub fn cache_control(self) -> CacheControl {
        match self {
            Self::FiveMinutes => CacheControl::ephemeral(),
            Self::OneHour => CacheControl::ephemeral_1h(),
        }
    }
}

/// How long before an entry would expire a refresh is sent.
///
/// The lifetime runs from the start of the request the provider received, and
/// the refresh is timed from the moment this client sent the last request —
/// a little earlier — so the margin only has to cover the refresh's own way
/// to the provider: connection setup and one slow round trip. Thirty seconds
/// is a tenth of the shorter lifetime: an early refresh costs that tenth more
/// refreshes, a late one costs the whole prefix rewritten.
pub const REFRESH_LEAD: Duration = Duration::from_secs(30);

/// One model's cache prices, each a multiple of its input rate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CacheRates {
    pub read: f64,
    pub five_minute_write: f64,
    pub one_hour_write: f64,
}

impl CacheRates {
    /// The price table's rates for an Anthropic model, `None` for a model no
    /// row prices — nothing is planned for a price nobody wrote down.
    #[must_use]
    pub fn for_anthropic_model(model: &str) -> Option<Self> {
        let rate = model_prices::anthropic_rate(model)?;
        let (five_minute_write, one_hour_write) = model_prices::anthropic_cache_write_rates();
        Some(Self {
            read: rate.cache_read,
            five_minute_write,
            one_hour_write,
        })
    }

    /// Writing an entry that lives `ttl`.
    #[must_use]
    pub fn write(self, ttl: CacheTtl) -> f64 {
        match ttl {
            CacheTtl::FiveMinutes => self.five_minute_write,
            CacheTtl::OneHour => self.one_hour_write,
        }
    }
}

/// One recorded request of a conversation, and the pause before the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdleGap {
    /// Start to start, or `None` when no request followed: the conversation
    /// ended, so whatever kept its cache alive was spent for nothing.
    pub until_next: Option<Duration>,
    /// The prefix the next request could read back: this request's input.
    pub prefix_tokens: u64,
    /// What this request wrote that any lifetime would have written — its
    /// new content, not a rewrite an expiry forced.
    pub written_tokens: u64,
}

/// The recorded pauses, arranged so the expected cost of a plan is a handful
/// of lookups rather than a pass over every request.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct IdleProfile {
    requests: u64,
    written_tokens: u64,
    /// Prefix waiting on a conversation that never came back.
    ended_prefix_tokens: u64,
    /// The pauses that did end, longest first, with the running sum of their
    /// prefixes: `returned[i].1` is the prefix of every pause at least as
    /// long as `returned[i].0`.
    returned: Vec<(Duration, u64)>,
}

impl IdleProfile {
    #[must_use]
    pub fn new(gaps: impl IntoIterator<Item = IdleGap>) -> Self {
        let mut profile = Self::default();
        let mut returned = Vec::new();
        for gap in gaps {
            profile.requests += 1;
            profile.written_tokens += gap.written_tokens;
            match gap.until_next {
                Some(pause) => returned.push((pause, gap.prefix_tokens)),
                None => profile.ended_prefix_tokens += gap.prefix_tokens,
            }
        }
        returned.sort_unstable_by(|left, right| right.0.cmp(&left.0));
        let mut running = 0;
        for entry in &mut returned {
            running += entry.1;
            entry.1 = running;
        }
        profile.returned = returned;
        profile
    }

    #[must_use]
    pub fn requests(&self) -> u64 {
        self.requests
    }

    /// The prefix of every returned pause longer than `elapsed`.
    fn returned_beyond(&self, elapsed: Duration) -> u64 {
        let longer = self.returned.partition_point(|(pause, _)| *pause > elapsed);
        longer.checked_sub(1).map_or(0, |last| self.returned[last].1)
    }

    /// The longest pause anyone came back from.
    fn longest_return(&self) -> Duration {
        self.returned.first().map_or(Duration::ZERO, |(pause, _)| *pause)
    }
}

/// What the conversation's cache does: the lifetime its markers ask for, and
/// how many refreshes an idle conversation gets before its entry is let go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CachePlan {
    pub ttl: CacheTtl,
    pub refreshes: u32,
}

impl CachePlan {
    /// The pause between one touch of the entry and the refresh after it.
    #[must_use]
    pub fn refresh_every(self) -> Duration {
        refresh_every(self.ttl)
    }
}

fn refresh_every(ttl: CacheTtl) -> Duration {
    ttl.lifetime().saturating_sub(REFRESH_LEAD)
}

/// The expected lifetime-dependent input cost of one request under `ttl` with
/// up to `refreshes` refreshes, in input-token equivalents: its writes at the
/// lifetime's rate, the refreshes a pause outlasts, and the rewrite past the
/// last one.
fn plan_cost(rates: CacheRates, profile: &IdleProfile, ttl: CacheTtl, refreshes: u32) -> f64 {
    let every = refresh_every(ttl);
    let mut reads = 0_u64;
    for refresh in 1..=refreshes {
        reads += profile.returned_beyond(every * refresh) + profile.ended_prefix_tokens;
    }
    let expired = profile.returned_beyond(every * refreshes + ttl.lifetime());
    let total = rates.write(ttl) * as_f64(profile.written_tokens)
        + rates.read * as_f64(reads)
        + (rates.write(ttl) - rates.read) * as_f64(expired);
    total / as_f64(profile.requests.max(1))
}

/// A token count as a float. Counts stay far below 2^52, where a float holds
/// every integer exactly, so nothing is lost.
#[allow(clippy::cast_precision_loss)]
fn as_f64(count: u64) -> f64 {
    count as f64
}

/// The fewest refreshes at `ttl` that cost the least over `profile`. No
/// refresh past the longest pause anyone came back from can save anything,
/// so the search stops there.
#[must_use]
pub fn cheapest_refreshes(rates: CacheRates, profile: &IdleProfile, ttl: CacheTtl) -> u32 {
    cheapest_refreshes_with_cost(rates, profile, ttl).0
}

fn cheapest_refreshes_with_cost(rates: CacheRates, profile: &IdleProfile, ttl: CacheTtl) -> (u32, f64) {
    let every = refresh_every(ttl);
    let reach = profile.longest_return().saturating_sub(ttl.lifetime());
    let most = u32::try_from(reach.as_nanos().div_ceil(every.as_nanos().max(1))).unwrap_or(u32::MAX);
    let mut best = (0, plan_cost(rates, profile, ttl, 0));
    for refreshes in 1..=most {
        let cost = plan_cost(rates, profile, ttl, refreshes);
        if cost < best.1 {
            best = (refreshes, cost);
        }
    }
    best
}

/// The cheapest plan over `profile` among `ttls`, with refreshes when
/// `refresh` allows them. A tie keeps the earlier lifetime and the fewer
/// refreshes, so an empty profile plans the provider's default and no
/// refresh at all.
#[must_use]
pub fn cheapest_plan(rates: CacheRates, profile: &IdleProfile, ttls: &[CacheTtl], refresh: bool) -> CachePlan {
    let mut best: Option<(CachePlan, f64)> = None;
    for &ttl in ttls {
        let (refreshes, cost) = if refresh {
            cheapest_refreshes_with_cost(rates, profile, ttl)
        } else {
            (0, plan_cost(rates, profile, ttl, 0))
        };
        if best.is_none_or(|(_, least)| cost < least) {
            best = Some((CachePlan { ttl, refreshes }, cost));
        }
    }
    best.map(|(plan, _)| plan).unwrap_or_default()
}

/// How the host asked the conversation's lifetime to be chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TtlChoice {
    /// By [`cheapest_plan`] over the recorded pauses.
    #[default]
    Planned,
    /// A lifetime the person named.
    Fixed(CacheTtl),
}

impl TtlChoice {
    /// The word a setting spells for planning.
    pub const PLANNED_LABEL: &'static str = "auto";

    /// `auto`, or a lifetime's label; `None` for anything else.
    #[must_use]
    pub fn from_label(label: &str) -> Option<Self> {
        if label.trim().eq_ignore_ascii_case(Self::PLANNED_LABEL) {
            return Some(Self::Planned);
        }
        CacheTtl::from_label(label).map(Self::Fixed)
    }

    fn candidates(self) -> Vec<CacheTtl> {
        match self {
            Self::Planned => CacheTtl::ALL.to_vec(),
            Self::Fixed(ttl) => vec![ttl],
        }
    }
}

/// The host's cache settings: how the lifetime is chosen, and whether an idle
/// conversation may be refreshed at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheSettings {
    pub ttl: TtlChoice,
    pub refresh: bool,
}

impl Default for CacheSettings {
    fn default() -> Self {
        Self {
            ttl: TtlChoice::Planned,
            refresh: true,
        }
    }
}

static DECLARED_SETTINGS: std::sync::Mutex<Option<CacheSettings>> = std::sync::Mutex::new(None);

/// Declare the cache settings for this process. The host calls this on the
/// way into every turn, from the same settings roots it reads everything
/// else from; a host that never does gets [`CacheSettings::default`].
pub fn declare_cache_settings(settings: CacheSettings) {
    *DECLARED_SETTINGS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(settings);
}

fn declared_cache_settings() -> CacheSettings {
    DECLARED_SETTINGS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .unwrap_or_default()
}

/// Whether the provider has refused a refresh in this process. A refusal is
/// a fact about the account or the endpoint (a gateway that does not take
/// `max_tokens: 0`, say), so asking again would only be refused again.
static REFRESH_REFUSED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn note_refresh_refused() {
    REFRESH_REFUSED.store(true, std::sync::atomic::Ordering::Relaxed);
}

fn refresh_allowed(settings: CacheSettings) -> bool {
    settings.refresh && !REFRESH_REFUSED.load(std::sync::atomic::Ordering::Relaxed)
}

/// How far back the recorded pauses are read: two weeks of a person's
/// working rhythm, long enough that the rare long pause has samples (about
/// 1% of requests here wait past five minutes) and short enough to follow a
/// change in how they work.
pub const PROFILE_WINDOW: Duration = Duration::from_secs(14 * 24 * 60 * 60);

static PROFILE: OnceLock<IdleProfile> = OnceLock::new();
static PROFILE_LOAD: std::sync::Once = std::sync::Once::new();

/// This machine's recorded foreground pauses, read once per process off the
/// request path. `None` until the read has finished — the plan is then the
/// one an empty profile gives, the provider's default.
fn idle_profile() -> Option<&'static IdleProfile> {
    PROFILE_LOAD.call_once(|| {
        let spawned = std::thread::Builder::new()
            .name("zo-cache-profile".to_string())
            .spawn(|| {
                let since = SystemTime::now()
                    .checked_sub(PROFILE_WINDOW)
                    .unwrap_or(UNIX_EPOCH);
                let gaps = crate::prompt_cache::recorded_foreground_gaps(since);
                let _ = PROFILE.set(IdleProfile::new(gaps));
            });
        if spawned.is_err() {
            let _ = PROFILE.set(IdleProfile::default());
        }
    });
    PROFILE.get()
}

/// The plan for the foreground conversation on `model` under the declared
/// settings. A model the price table does not know keeps the provider's
/// default (or the named lifetime) and is never refreshed.
#[must_use]
pub fn conversation_cache_plan(model: &str) -> CachePlan {
    let settings = declared_cache_settings();
    let candidates = settings.ttl.candidates();
    let Some(rates) = CacheRates::for_anthropic_model(model) else {
        return CachePlan {
            ttl: candidates.first().copied().unwrap_or_default(),
            refreshes: 0,
        };
    };
    let empty = IdleProfile::default();
    let profile = idle_profile().unwrap_or(&empty);
    cheapest_plan(rates, profile, &candidates, refresh_allowed(settings))
}

/// How many refreshes an idle conversation on `model` gets when its markers
/// ask for `ttl` — the lifetime actually written, which a host's declaration
/// may have fixed.
#[must_use]
pub fn conversation_refreshes(model: &str, ttl: CacheTtl) -> u32 {
    let settings = declared_cache_settings();
    if !refresh_allowed(settings) {
        return 0;
    }
    let (Some(rates), Some(profile)) = (CacheRates::for_anthropic_model(model), idle_profile()) else {
        return 0;
    };
    cheapest_refreshes(rates, profile, ttl)
}

/// The recorded pauses of the conversations under `root`, from the request
/// rows of every session directory a sub-agent did not write: each provider
/// request of `provider` since `since`, the pause to the next one, and what
/// it wrote that any lifetime would have written. A refresh row is neither a
/// request nor a pause — it is what this module adds.
#[must_use]
pub fn idle_gaps_under(root: &Path, provider: &str, since: SystemTime, sub_agent_prefix: &str) -> Vec<IdleGap> {
    let since_ms = since
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| u64::try_from(since.as_millis()).unwrap_or(u64::MAX));
    let Ok(sessions) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut gaps = Vec::new();
    for session in sessions.flatten() {
        if session.file_name().to_string_lossy().starts_with(sub_agent_prefix) {
            continue;
        }
        let rows: Vec<_> = crate::prompt_cache::read_request_ledger(&session.path().join("requests.jsonl"))
            .into_iter()
            .filter(|row| !row.refresh && row.provider == provider && row.ts_unix_ms >= since_ms)
            .collect();
        gaps.extend(session_gaps(&rows));
    }
    gaps
}

/// One session's rows as pauses. A request that followed a pause longer than
/// the shortest lifetime counts only the prefix it added as written: the rest
/// was the expiry's rewrite, which the plan prices separately.
fn session_gaps(rows: &[crate::prompt_cache::RequestLedgerRow]) -> Vec<IdleGap> {
    let shortest = CacheTtl::ALL[0].lifetime();
    let prefix = |row: &crate::prompt_cache::RequestLedgerRow| {
        u64::from(row.cache_read) + u64::from(row.cache_creation) + u64::from(row.input_uncached)
    };
    let mut ordered: Vec<_> = rows.iter().collect();
    ordered.sort_by_key(|row| row.ts_unix_ms);
    ordered
        .iter()
        .enumerate()
        .map(|(at, row)| {
            let after_pause = at.checked_sub(1).map(|before| {
                (
                    Duration::from_millis(row.ts_unix_ms.saturating_sub(ordered[before].ts_unix_ms)),
                    prefix(ordered[before]),
                )
            });
            let written_tokens = match after_pause {
                Some((pause, before)) if pause > shortest => prefix(row).saturating_sub(before),
                _ => u64::from(row.cache_creation),
            };
            IdleGap {
                until_next: ordered
                    .get(at + 1)
                    .map(|next| Duration::from_millis(next.ts_unix_ms.saturating_sub(row.ts_unix_ms))),
                prefix_tokens: prefix(row),
                written_tokens,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{cheapest_plan, cheapest_refreshes, CacheRates, CachePlan, CacheTtl, IdleGap, IdleProfile, TtlChoice};
    use std::time::Duration;

    const OPUS_5_5: CacheRates = CacheRates {
        read: 0.05,
        five_minute_write: 1.25,
        one_hour_write: 2.0,
    };

    fn minutes(count: u64) -> Duration {
        Duration::from_secs(count * 60)
    }

    fn gap(until_next: Option<Duration>, prefix_tokens: u64, written_tokens: u64) -> IdleGap {
        IdleGap { until_next, prefix_tokens, written_tokens }
    }

    /// With nothing recorded, the plan is the provider's default and no
    /// refresh: nothing is spent on a guess.
    #[test]
    fn an_empty_record_plans_the_default_and_no_refresh() {
        let plan = cheapest_plan(OPUS_5_5, &IdleProfile::default(), &CacheTtl::ALL, true);
        assert_eq!(plan, CachePlan { ttl: CacheTtl::FiveMinutes, refreshes: 0 });
    }

    /// Requests that follow each other inside five minutes keep the entry
    /// warm by themselves; a refresh or an hour would only cost.
    #[test]
    fn a_conversation_that_never_pauses_needs_neither_the_hour_nor_a_refresh() {
        let profile = IdleProfile::new((0..100).map(|_| gap(Some(Duration::from_secs(40)), 200_000, 5_000)));
        assert_eq!(
            cheapest_plan(OPUS_5_5, &profile, &CacheTtl::ALL, true),
            CachePlan { ttl: CacheTtl::FiveMinutes, refreshes: 0 }
        );
    }

    /// A person who comes back after twenty minutes: refreshing the entry
    /// until then costs a few reads of the prefix, letting it go costs the
    /// prefix rewritten, and writing everything for an hour costs the premium
    /// on every write. The refreshes reach just past twenty minutes.
    #[test]
    fn a_twenty_minute_pause_is_bridged_by_refreshes_that_reach_past_it() {
        let mut gaps: Vec<_> = (0..95).map(|_| gap(Some(Duration::from_secs(30)), 200_000, 8_000)).collect();
        gaps.extend((0..5).map(|_| gap(Some(minutes(20)), 200_000, 8_000)));
        let profile = IdleProfile::new(gaps);
        let plan = cheapest_plan(OPUS_5_5, &profile, &CacheTtl::ALL, true);
        assert_eq!(plan.ttl, CacheTtl::FiveMinutes);
        let reach = plan.refresh_every() * plan.refreshes + plan.ttl.lifetime();
        assert!(reach >= minutes(20), "the last refresh keeps the entry to the person's return: {plan:?}");
        assert!(
            plan.refresh_every() * plan.refreshes.saturating_sub(1) + plan.ttl.lifetime() < minutes(20),
            "and not one refresh more: {plan:?}"
        );
    }

    /// A conversation nobody comes back to is never refreshed: every refresh
    /// would be spent for nothing.
    #[test]
    fn a_conversation_that_ends_is_not_refreshed() {
        let mut gaps: Vec<_> = (0..90).map(|_| gap(Some(Duration::from_secs(30)), 200_000, 8_000)).collect();
        gaps.extend((0..10).map(|_| gap(None, 200_000, 8_000)));
        assert_eq!(cheapest_refreshes(OPUS_5_5, &IdleProfile::new(gaps), CacheTtl::FiveMinutes), 0);
    }

    /// Where writes are small and every pause is long, the hour wins: a chat
    /// whose requests add little but whose person steps away for fifty
    /// minutes each time pays less premium on its writes than ten refreshes
    /// of its prefix, at a model's standard tenth-of-input read.
    #[test]
    fn small_writes_and_long_pauses_choose_the_hour() {
        let standard = CacheRates { read: 0.1, ..OPUS_5_5 };
        let profile = IdleProfile::new((0..100).map(|_| gap(Some(minutes(50)), 100_000, 500)));
        let plan = cheapest_plan(standard, &profile, &CacheTtl::ALL, true);
        assert_eq!(plan, CachePlan { ttl: CacheTtl::OneHour, refreshes: 0 });
    }

    /// A lifetime the person named is the only one considered, and with
    /// refreshes switched off the plan has none.
    #[test]
    fn a_named_lifetime_and_no_refresh_are_kept() {
        let mut gaps: Vec<_> = (0..95).map(|_| gap(Some(Duration::from_secs(30)), 200_000, 8_000)).collect();
        gaps.extend((0..5).map(|_| gap(Some(minutes(20)), 200_000, 8_000)));
        let profile = IdleProfile::new(gaps);
        let hour = cheapest_plan(OPUS_5_5, &profile, &[CacheTtl::OneHour], true);
        assert_eq!(hour.ttl, CacheTtl::OneHour);
        assert_eq!(cheapest_plan(OPUS_5_5, &profile, &CacheTtl::ALL, false).refreshes, 0);
    }

    #[test]
    fn a_setting_spells_auto_or_a_lifetime() {
        assert_eq!(TtlChoice::from_label("auto"), Some(TtlChoice::Planned));
        assert_eq!(TtlChoice::from_label(" 1H "), Some(TtlChoice::Fixed(CacheTtl::OneHour)));
        assert_eq!(TtlChoice::from_label("5m"), Some(TtlChoice::Fixed(CacheTtl::FiveMinutes)));
        assert_eq!(TtlChoice::from_label("10m"), None);
        assert_eq!(CacheTtl::from_label("1h").map(CacheTtl::lifetime), Some(minutes(60)));
    }

    /// The recorded pauses come from foreground sessions only, one pause per
    /// request of the named provider, a refresh row being neither; a request
    /// after a pause past the shortest lifetime counts only what it added.
    #[test]
    fn the_record_is_the_foreground_sessions_requests_and_their_pauses() {
        let root = std::env::temp_dir().join(format!(
            "zo-cache-policy-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_nanos())
        ));
        let row = |ts: u64, read: u32, written: u32, provider: &str, refresh: bool| {
            let row = crate::prompt_cache::RequestLedgerRow {
                ts_unix_ms: ts,
                provider: provider.to_string(),
                cache_read: read,
                cache_creation: written,
                refresh,
                ..Default::default()
            };
            serde_json::to_string(&row).expect("row") + "\n"
        };
        let write = |dir: &str, rows: &[String]| {
            std::fs::create_dir_all(root.join(dir)).expect("session dir");
            std::fs::write(root.join(dir).join("requests.jsonl"), rows.concat()).expect("rows");
        };
        let minute = 60_000;
        write(
            "session-1",
            &[
                row(1_000, 0, 10_000, "anthropic", false),
                row(1_000 + minute, 10_000, 2_000, "anthropic", false),
                row(1_000 + 5 * minute, 12_000, 0, "anthropic", true),
                row(1_000 + 21 * minute, 0, 15_000, "anthropic", false),
                row(1_000 + 22 * minute, 0, 9_000, "openai", false),
            ],
        );
        write("agent-7", &[row(1_000, 0, 50_000, "anthropic", false)]);

        let mut gaps = super::idle_gaps_under(&root, "anthropic", std::time::UNIX_EPOCH, "agent-");
        gaps.sort_by_key(|gap| gap.until_next.unwrap_or(Duration::MAX));
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(
            gaps,
            vec![
                gap(Some(minutes(1)), 10_000, 10_000),
                gap(Some(minutes(20)), 12_000, 2_000),
                gap(None, 15_000, 3_000),
            ]
        );
    }
}
