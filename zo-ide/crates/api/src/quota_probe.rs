//! Ask each signed-in provider what quota the account has left.
//!
//! Every other quota signal in this crate is a by-product of traffic: the
//! Anthropic headers ride on a response, and the 429 cool-downs only exist once
//! the account is already throttled. Neither can answer the question a session
//! opens with — *how much room do I have right now* — because both require
//! having already spent something to find out.
//!
//! These endpoints answer it directly. They are the same ones the vendors' own
//! desktop clients poll, so the figures match what a user sees elsewhere rather
//! than being a second opinion derived from our traffic alone.
//!
//! Everything here fails soft. A probe that cannot authenticate, cannot reach
//! the network, or gets a shape it does not recognise records nothing and lets
//! [`crate::quota`] fall back to the signals it already had — a missing row
//! reads as "unknown", which is honest, whereas a zero would read as "empty".

use std::time::Duration;

use serde::Deserialize;

use crate::quota::{record_measured_quota, MeasuredQuota, QuotaWindow};
use crate::ProviderKind;

/// Ceiling on a probe. These run on a background cadence and nothing waits on
/// them, so a hung endpoint must not pin a task for minutes.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Refresh every provider we hold credentials for.
///
/// Probes run concurrently because they are independent and each one is a
/// network round trip; serially this would be three timeouts deep in the worst
/// case. Failures are per-provider — one signed-out account never suppresses
/// the others.
pub async fn refresh_measured_quotas() {
    let (anthropic, codex, google) =
        tokio::join!(probe_anthropic(), probe_codex(), probe_google());
    if let Some(quota) = anthropic {
        record_measured_quota(ProviderKind::Anthropic, quota);
    }
    if let Some(quota) = codex {
        record_measured_quota(ProviderKind::OpenAi, quota);
    }
    if let Some(quota) = google {
        record_measured_quota(ProviderKind::Google, quota);
    }
}

/// How often a background refresh may run.
///
/// Quota windows move over hours, so this is not about freshness — it is about
/// not turning a per-frame HUD rebuild into a per-frame HTTP request.
const REFRESH_CADENCE: Duration = Duration::from_secs(60);

/// Unix millis of the last refresh kick, so the cadence survives across the
/// many places a HUD rebuild can originate.
static LAST_REFRESH_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Kick a refresh in the background if one is due, and return immediately.
///
/// Safe to call from a render path: it never blocks, never awaits, and drops
/// the request entirely when no async runtime is running. The cadence gate is
/// claimed before spawning, so two threads racing here produce one probe.
pub fn refresh_measured_quotas_soon() {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| u64::try_from(since.as_millis()).unwrap_or(u64::MAX));
    let last = LAST_REFRESH_MS.load(std::sync::atomic::Ordering::Relaxed);
    let cadence_ms = u64::try_from(REFRESH_CADENCE.as_millis()).unwrap_or(u64::MAX);
    if last != 0 && now.saturating_sub(last) < cadence_ms {
        return;
    }
    // Claim the slot before spawning so a burst of rebuilds yields one probe.
    if LAST_REFRESH_MS
        .compare_exchange(
            last,
            now,
            std::sync::atomic::Ordering::Relaxed,
            std::sync::atomic::Ordering::Relaxed,
        )
        .is_err()
    {
        return;
    }
    handle.spawn(refresh_measured_quotas());
}

/// The subscription bearer this probe may speak with, plus the plan word to
/// label it — resolved through **the same chain the session itself uses**
/// ([`crate::providers::anthropic::resolve_claude_auth_fresh_detailed`]).
///
/// Reading the credential store directly was the earlier shape, and it went
/// blind exactly when it mattered: a Claude Code token lives about eight hours,
/// so a long session (the case this figure exists for) spends part of its life
/// holding an *expired* stored token. The session keeps working because the
/// chain refreshes it in place; a probe that re-reads the store and gives up on
/// expiry silently folds the usage card while the session it describes is
/// perfectly healthy. Measured 2026-08-27: card blank with a 122-minute-stale
/// token, full once the same chain had refreshed it.
///
/// An env API key is rejected here rather than sent: `/api/oauth/usage` is part
/// of the subscription OAuth surface and answers about a *subscription*, so a
/// metered key has nothing to ask it.
struct ProbeBearer {
    token: String,
    plan: Option<String>,
}

fn anthropic_probe_bearer() -> Option<ProbeBearer> {
    let resolved = crate::providers::anthropic::resolve_claude_auth_fresh_detailed()?;
    let token = match resolved.auth {
        crate::providers::anthropic::AuthSource::BearerToken(token)
        | crate::providers::anthropic::AuthSource::ApiKeyAndBearer {
            bearer_token: token,
            ..
        } => token,
        crate::providers::anthropic::AuthSource::ApiKey(_)
        | crate::providers::anthropic::AuthSource::None => return None,
    };
    // The plan word lives in the Claude Code credential blob and nowhere else;
    // when another rung of the chain answered, the card simply has no plan to
    // print. This read is cached and shares the in-flight lock with the
    // resolution above, so it costs no extra `security(1)` fork.
    let plan = crate::providers::anthropic::keychain::read_claude_code_keychain_session()
        .and_then(|session| session.plan);
    Some(ProbeBearer { token, plan })
}

/// Anthropic's OAuth usage endpoint.
///
/// The beta header and the Claude Code user agent are both load-bearing: the
/// endpoint is part of the subscription OAuth surface, not the public API, and
/// it refuses the request without them.
async fn probe_anthropic() -> Option<MeasuredQuota> {
    // Off the async worker: this may fork a `security` process to reach the
    // keychain and may refresh a token over the network, and blocking a
    // current-thread runtime here would stall the very UI the figure is for.
    let bearer = tokio::task::spawn_blocking(anthropic_probe_bearer)
        .await
        .ok()
        .flatten()?;
    let response = crate::providers::shared_http_client()
        .get("https://api.anthropic.com/api/oauth/usage")
        .bearer_auth(&bearer.token)
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("User-Agent", "claude-code/2.1.0")
        .timeout(PROBE_TIMEOUT)
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let usage: AnthropicUsage = response.json().await.ok()?;
    Some(MeasuredQuota {
        windows: usage.windows(),
        // The usage response identifies the quota, never the person: no email,
        // no plan. The credential that opened the session is the only thing
        // that knows either, and it knows the plan.
        account: account_label(None, bearer.plan.as_deref()),
    })
}

/// The ChatGPT backend's usage endpoint, which Codex plans bill against.
///
/// Resolved through [`crate::oauth_store::load_openai_oauth`] — the same
/// credential the request path speaks with — so this row can only ever describe
/// the account zo is actually spending. Reading `$CODEX_HOME/auth.json` on its
/// own is what let `/status` report a full plan while every turn 429'd on a
/// different, exhausted one.
async fn probe_codex() -> Option<MeasuredQuota> {
    // Same reasoning as the Anthropic probe: a filesystem read is short but it
    // is still blocking, and the cost of being wrong is a frozen frame.
    let auth = tokio::task::spawn_blocking(|| {
        crate::oauth_store::load_openai_oauth().ok().flatten()
    })
    .await
    .ok()
    .flatten()?;
    let mut request = crate::providers::shared_http_client()
        .get("https://chatgpt.com/backend-api/wham/usage")
        .bearer_auth(&auth.access_token)
        .header("User-Agent", "codex-cli")
        .header("OpenAI-Beta", "codex-1")
        .header("originator", "Codex Desktop")
        .timeout(PROBE_TIMEOUT);
    if let Some(account_id) = &auth.account_id {
        request = request.header("ChatGPT-Account-Id", account_id);
    }
    let response = request.send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let usage: CodexUsage = response.json().await.ok()?;
    Some(MeasuredQuota {
        windows: usage.windows(),
        account: account_label(usage.email.as_deref(), usage.plan_type.as_deref()),
    })
}

/// Gemini Code Assist's quota endpoint, which the Antigravity sign-in bills
/// against.
///
/// Unlike the other two this reports headroom directly, per model bucket, as a
/// fraction. The worst bucket is the provider's standing: a model that is out
/// is out regardless of how much room its siblings have.
async fn probe_google() -> Option<MeasuredQuota> {
    // `load_fresh_oauth` may refresh against the network, so it is blocking.
    let tokens = tokio::task::spawn_blocking(
        crate::providers::gemini_code_assist::load_fresh_oauth,
    )
    .await
    .ok()
    .flatten()?;
    let response = crate::providers::shared_http_client()
        .post(crate::providers::gemini_code_assist::method_url("retrieveUserQuota"))
        .bearer_auth(&tokens.access_token)
        .json(&serde_json::json!({}))
        .timeout(PROBE_TIMEOUT)
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let quota: GoogleQuota = response.json().await.ok()?;
    let windows = quota.windows();
    if windows.is_empty() {
        return None;
    }
    Some(MeasuredQuota {
        windows,
        account: None,
    })
}

/// Gemini's quota payload: either a bare array of buckets or one wrapped in
/// `buckets`, which is why both shapes are accepted.
#[derive(Deserialize)]
#[serde(untagged)]
enum GoogleQuota {
    Wrapped { buckets: Vec<GoogleBucket> },
    Bare(Vec<GoogleBucket>),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoogleBucket {
    /// Headroom as a fraction of the allowance, already the direction we want.
    remaining_fraction: f64,
    reset_time: Option<String>,
}

impl GoogleQuota {
    fn windows(&self) -> Vec<QuotaWindow> {
        let (Self::Wrapped { buckets } | Self::Bare(buckets)) = self;
        // The tightest bucket is the account's real standing; listing every
        // model would bury it and none of them are separately actionable here.
        let Some(worst) = buckets
            .iter()
            .filter(|bucket| bucket.remaining_fraction.is_finite())
            .min_by(|left, right| {
                left.remaining_fraction
                    .partial_cmp(&right.remaining_fraction)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
        else {
            return Vec::new();
        };
        vec![QuotaWindow {
            label: "quota".to_string(),
            remaining_percent: remaining_from_utilization(
                100.0 - worst.remaining_fraction.clamp(0.0, 1.0) * 100.0,
            ),
            resets_at_unix: unix_from_rfc3339(worst.reset_time.as_deref()),
            // The worst bucket already stands for the account; naming its model
            // would promise a per-model list this fold deliberately collapsed.
            model: None,
        }]
    }
}

/// Anthropic's usage payload. Only the fields we render are named; the response
/// carries a dozen nullable per-model windows that are absent on most plans.
#[derive(Deserialize)]
struct AnthropicUsage {
    five_hour: Option<AnthropicWindow>,
    seven_day: Option<AnthropicWindow>,
    /// The normalized limit list the endpoint reports beside the two named
    /// windows: one row per live limit, each with its own percentage, reset and
    /// optional scope.
    ///
    /// Only the model-scoped rows are read from it. Its unscoped rows restate
    /// `five_hour`/`seven_day` (measured: `session` = 14 next to
    /// `five_hour.utilization` = 14.0), so reading those too would list every
    /// account window twice — while the scoped rows are the ONLY place a
    /// per-model allowance appears at all. The alternative source, the payload's
    /// `seven_day_opus`/`seven_day_sonnet`/`tangelo`/`iguana_necktie` sprawl,
    /// is a hardcoded roster of codenames that goes stale the day a model ships;
    /// this list names its own model.
    limits: Option<Vec<AnthropicLimit>>,
}

#[derive(Deserialize)]
struct AnthropicWindow {
    /// Percent of the window consumed.
    utilization: f64,
    /// RFC 3339 instant the window rolls over.
    resets_at: Option<String>,
}

/// One row of [`AnthropicUsage::limits`].
#[derive(Deserialize)]
struct AnthropicLimit {
    /// Which window family the row belongs to (`"session"`, `"weekly"`), which
    /// is what names it — `kind` splits the same family into `weekly_all` and
    /// `weekly_scoped`, a distinction `scope` already carries.
    group: Option<String>,
    /// Percent of the window consumed, matching `utilization` above.
    percent: Option<f64>,
    resets_at: Option<String>,
    /// Present when the row is an allowance for something narrower than the
    /// account. Only the model dimension is read; a surface-scoped row is not a
    /// quota a model runs into.
    scope: Option<AnthropicLimitScope>,
}

#[derive(Deserialize)]
struct AnthropicLimitScope {
    model: Option<AnthropicScopedModel>,
}

#[derive(Deserialize)]
struct AnthropicScopedModel {
    /// The name the vendor shows people (`"Fable"`), not the wire id — which is
    /// `null` on this payload anyway.
    display_name: Option<String>,
}

impl AnthropicUsage {
    fn windows(&self) -> Vec<QuotaWindow> {
        let account = [("5h", self.five_hour.as_ref()), ("7d", self.seven_day.as_ref())]
            .into_iter()
            .filter_map(|(label, window)| window.map(|window| window.to_quota(label)));
        let scoped = self
            .limits
            .iter()
            .flatten()
            .filter_map(AnthropicLimit::to_quota);
        account.chain(scoped).collect()
    }
}

impl AnthropicWindow {
    fn to_quota(&self, label: &str) -> QuotaWindow {
        QuotaWindow {
            label: label.to_string(),
            remaining_percent: remaining_from_utilization(self.utilization),
            resets_at_unix: unix_from_rfc3339(self.resets_at.as_deref()),
            model: None,
        }
    }
}

impl AnthropicLimit {
    /// A model-scoped row as a window. `None` for every other row — an unscoped
    /// one duplicates an account window we already have, and a row without a
    /// percentage has nothing to report.
    fn to_quota(&self) -> Option<QuotaWindow> {
        let model = self
            .scope
            .as_ref()?
            .model
            .as_ref()?
            .display_name
            .as_deref()
            .filter(|name| !name.is_empty())?;
        Some(QuotaWindow {
            label: anthropic_group_label(self.group.as_deref()),
            remaining_percent: remaining_from_utilization(self.percent?),
            resets_at_unix: unix_from_rfc3339(self.resets_at.as_deref()),
            model: Some(model.to_string()),
        })
    }
}

/// Anthropic's window-family word in the vocabulary the other rows use.
///
/// The two names it translates are the two the endpoint reports; anything else
/// travels through verbatim rather than being dropped or guessed at, so a family
/// added later still reaches the card wearing its own name.
fn anthropic_group_label(group: Option<&str>) -> String {
    match group {
        Some("session") => "5h".to_string(),
        Some("weekly") => "7d".to_string(),
        Some(other) if !other.is_empty() => other.to_string(),
        _ => "limit".to_string(),
    }
}

/// The ChatGPT backend's usage payload.
#[derive(Deserialize)]
struct CodexUsage {
    email: Option<String>,
    /// The subscription word the vendor's own client prints beside the address
    /// (`"pro"`). Measured on the live response, so no JWT has to be opened for
    /// it.
    plan_type: Option<String>,
    /// The account-wide allowance every request bills against.
    rate_limit: Option<CodexRateLimit>,
    /// Allowances scoped to one model, which sit BESIDE the account window
    /// rather than replacing it — the vendor's own `/status` card prints both
    /// (measured: an account weekly window at 5% left above a
    /// `GPT-5.3-Codex-Spark` pair at 93%/52%).
    ///
    /// `Option` rather than `#[serde(default)]`: the sibling
    /// `code_review_rate_limit` shows this payload writes explicit `null`s, and
    /// a `null` would fail to deserialize into a bare `Vec`, taking the whole
    /// probe down with it.
    additional_rate_limits: Option<Vec<CodexAdditionalLimit>>,
}

/// One entry of [`CodexUsage::additional_rate_limits`] — the same window pair
/// as the account limit, under a name.
#[derive(Deserialize)]
struct CodexAdditionalLimit {
    limit_name: Option<String>,
    rate_limit: Option<CodexRateLimit>,
}

#[derive(Deserialize)]
struct CodexRateLimit {
    primary_window: Option<CodexWindow>,
    secondary_window: Option<CodexWindow>,
}

#[derive(Deserialize)]
struct CodexWindow {
    used_percent: f64,
    /// Window length, which is what names the row — the payload has no label of
    /// its own and "primary"/"secondary" means nothing to a reader.
    limit_window_seconds: Option<u64>,
    /// Unix seconds, unlike Anthropic's RFC 3339 string.
    reset_at: Option<u64>,
}

impl CodexUsage {
    fn windows(&self) -> Vec<QuotaWindow> {
        let account = self
            .rate_limit
            .iter()
            .flat_map(|limit| limit.windows(None));
        let per_model = self
            .additional_rate_limits
            .iter()
            .flatten()
            .filter_map(CodexAdditionalLimit::windows)
            .flatten();
        account.chain(per_model).collect()
    }
}

impl CodexAdditionalLimit {
    /// The entry's windows under its model name. An entry that names nothing is
    /// dropped: an unattributed row is indistinguishable from the account
    /// window it sits next to, and would read as a second opinion about it.
    fn windows(&self) -> Option<Vec<QuotaWindow>> {
        let name = self
            .limit_name
            .as_deref()
            .filter(|name| !name.is_empty())?;
        Some(self.rate_limit.as_ref()?.windows(Some(name)))
    }
}

impl CodexRateLimit {
    /// The pair of windows this limit reports, attributed to `model` (`None`
    /// for the account-wide limit). One mapping serves both callers — the
    /// account limit and every per-model entry have the identical shape.
    fn windows(&self, model: Option<&str>) -> Vec<QuotaWindow> {
        [self.primary_window.as_ref(), self.secondary_window.as_ref()]
            .into_iter()
            .flatten()
            .map(|window| window.to_quota(model))
            .collect()
    }
}

impl CodexWindow {
    fn to_quota(&self, model: Option<&str>) -> QuotaWindow {
        QuotaWindow {
            label: self
                .limit_window_seconds
                .map_or_else(|| "limit".to_string(), window_label),
            remaining_percent: remaining_from_utilization(self.used_percent),
            resets_at_unix: self.reset_at,
            model: model.map(str::to_string),
        }
    }
}

/// One account label from the two things a provider might know about a sign-in:
/// who it is, and what plan it bills.
///
/// Either half can be missing and the label folds to whatever is present —
/// Anthropic's usage endpoint names no person, so its card says `Max`, while
/// ChatGPT names both and says `someone@example.com (Pro)`. Both providers pass
/// through here so the two cards cannot drift into two different spellings of
/// the same fact; `None` means the card folds the row rather than printing an
/// empty one.
fn account_label(email: Option<&str>, plan: Option<&str>) -> Option<String> {
    let email = email.map(str::trim).filter(|email| !email.is_empty());
    let plan = plan
        .map(str::trim)
        .filter(|plan| !plan.is_empty())
        .map(title_case);
    match (email, plan) {
        (Some(email), Some(plan)) => Some(format!("{email} ({plan})")),
        (Some(email), None) => Some(email.to_string()),
        (None, Some(plan)) => Some(plan),
        (None, None) => None,
    }
}

/// A vendor's lowercase plan word as people see it printed (`"pro"` → `"Pro"`).
///
/// Only the first character moves, which is the whole rule: a lookup table of
/// plan names would need editing every time a vendor invents a tier.
fn title_case(word: &str) -> String {
    let mut characters = word.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => String::new(),
    }
}

/// RFC 3339 instant as unix seconds, dropping anything unparseable or
/// pre-epoch. Shared by the two payloads that date their windows with a string.
fn unix_from_rfc3339(value: Option<&str>) -> Option<u64> {
    value
        .and_then(core_types::date::unix_secs_from_rfc3339)
        .and_then(|secs| u64::try_from(secs).ok())
}

/// Name a window by its length, the way the other quota rows read (`5h`, `7d`).
fn window_label(seconds: u64) -> String {
    let hours = seconds / 3_600;
    if hours >= 24 && hours.is_multiple_of(24) {
        return format!("{}d", hours / 24);
    }
    if hours > 0 {
        return format!("{hours}h");
    }
    format!("{}m", seconds / 60)
}

/// Convert a consumed percentage into remaining headroom.
///
/// Providers report utilization; every surface here reads headroom. Converting
/// once at the edge keeps the inversion out of the render path, where getting
/// it backwards would silently invert the meaning of every gauge.
fn remaining_from_utilization(utilization: f64) -> u8 {
    if !utilization.is_finite() {
        return 0;
    }
    let used = utilization.clamp(0.0, 100.0).round();
    // Safe: the clamp above bounds this to 0..=100.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let used = used as u8;
    100u8.saturating_sub(used)
}

#[cfg(test)]
mod tests {
    use super::{
        account_label, anthropic_group_label, remaining_from_utilization, window_label,
        AnthropicUsage, CodexUsage,
    };

    /// Verbatim from the live endpoint, so a schema drift shows up here rather
    /// than as a silently empty gauge.
    #[test]
    fn anthropic_payload_maps_to_remaining_headroom() {
        let usage: AnthropicUsage = serde_json::from_str(
            r#"{
                "five_hour": {"utilization": 31.0, "resets_at": "2026-08-03T09:20:00.447442+00:00"},
                "seven_day": {"utilization": 39.0, "resets_at": "2026-08-09T02:00:00.447469+00:00"},
                "seven_day_opus": null
            }"#,
        )
        .expect("the live shape parses");

        let windows = usage.windows();
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].label, "5h");
        // 31% spent is 69% left — the inversion happens once, here.
        assert_eq!(windows[0].remaining_percent, 69);
        assert_eq!(windows[0].resets_at_unix, Some(1_785_748_800));
        assert_eq!(windows[1].label, "7d");
        assert_eq!(windows[1].remaining_percent, 61);
        // No `limits` array at all is the older shape; it must still parse.
        assert!(windows.iter().all(|window| window.model.is_none()));
    }

    /// The `limits` array, verbatim from the live endpoint (2026-08-27): a
    /// session row and a weekly row that RESTATE the two named windows, and one
    /// model-scoped weekly row that exists nowhere else in the payload.
    ///
    /// This is the case the card exists for: the account has 22% of its week
    /// left while the model in hand has none, and a card that read only
    /// `seven_day` would promise headroom the next turn cannot spend.
    #[test]
    fn anthropic_scoped_limits_become_per_model_windows_without_doubling_the_account() {
        let usage: AnthropicUsage = serde_json::from_str(
            r#"{
                "five_hour": {"utilization": 14.0, "resets_at": "2026-08-27T11:30:00.344819+00:00"},
                "seven_day": {"utilization": 78.0, "resets_at": "2026-08-30T02:00:00.344838+00:00"},
                "seven_day_opus": null,
                "limits": [
                    {"kind": "session", "group": "session", "percent": 14,
                     "severity": "normal", "resets_at": "2026-08-27T11:30:00.344819+00:00",
                     "scope": null, "is_active": false},
                    {"kind": "weekly_all", "group": "weekly", "percent": 78,
                     "severity": "warning", "resets_at": "2026-08-30T02:00:00.344838+00:00",
                     "scope": null, "is_active": false},
                    {"kind": "weekly_scoped", "group": "weekly", "percent": 100,
                     "severity": "critical", "resets_at": "2026-08-30T02:00:00.345076+00:00",
                     "scope": {"model": {"id": null, "display_name": "Fable"}, "surface": null},
                     "is_active": true}
                ]
            }"#,
        )
        .expect("the live shape parses");

        let windows = usage.windows();
        // Two account windows + one scoped: the unscoped rows of `limits` are
        // the SAME two windows and must not appear a second time.
        assert_eq!(windows.len(), 3);
        assert_eq!(
            windows
                .iter()
                .filter(|window| window.model.is_none())
                .count(),
            2
        );
        let scoped = &windows[2];
        assert_eq!(scoped.model.as_deref(), Some("Fable"));
        assert_eq!(scoped.label, "7d");
        assert_eq!(scoped.remaining_percent, 0);
        assert_eq!(scoped.resets_at_unix, Some(1_788_055_200));
    }

    /// A scoped row with nothing to say is not a window. Inventing one would put
    /// a nameless 100%-left gauge on the card.
    #[test]
    fn anthropic_limits_without_a_model_or_a_percentage_are_dropped() {
        let usage: AnthropicUsage = serde_json::from_str(
            r#"{
                "five_hour": null,
                "seven_day": null,
                "limits": [
                    {"group": "weekly", "percent": 40, "scope": {"surface": "cowork"}},
                    {"group": "weekly", "percent": 40, "scope": {"model": {"display_name": ""}}},
                    {"group": "weekly", "percent": null,
                     "scope": {"model": {"display_name": "Fable"}}}
                ]
            }"#,
        )
        .expect("the live shape parses");

        assert!(usage.windows().is_empty());
    }

    /// An unknown window family keeps its own name rather than being dropped or
    /// guessed into `5h`.
    #[test]
    fn anthropic_group_labels_translate_the_two_known_families() {
        assert_eq!(anthropic_group_label(Some("session")), "5h");
        assert_eq!(anthropic_group_label(Some("weekly")), "7d");
        assert_eq!(anthropic_group_label(Some("monthly")), "monthly");
        assert_eq!(anthropic_group_label(None), "limit");
        assert_eq!(anthropic_group_label(Some("")), "limit");
    }

    /// Also verbatim, including the null secondary window most plans return.
    #[test]
    fn codex_payload_maps_windows_and_account() {
        let usage: CodexUsage = serde_json::from_str(
            r#"{
                "email": "someone@example.com",
                "plan_type": "pro",
                "rate_limit": {
                    "primary_window": {
                        "used_percent": 15,
                        "limit_window_seconds": 604800,
                        "reset_at": 1786169992
                    },
                    "secondary_window": null
                }
            }"#,
        )
        .expect("the live shape parses");

        assert_eq!(usage.email.as_deref(), Some("someone@example.com"));
        let windows = usage.windows();
        assert_eq!(windows.len(), 1);
        // 604800s is a week, and "7d" is what the other rows call that.
        assert_eq!(windows[0].label, "7d");
        assert_eq!(windows[0].remaining_percent, 85);
        assert_eq!(windows[0].resets_at_unix, Some(1_786_169_992));
        assert_eq!(windows[0].model, None);
    }

    /// `additional_rate_limits`, verbatim from the live endpoint (2026-08-27).
    /// The numbers are the ones the vendor's own `/status` card printed in
    /// `docs/captures/codex-tui-v0.150.1-status.bin`: a weekly account window
    /// at 5% left, then a named pair at 93% (5h) and 52% (weekly).
    #[test]
    fn codex_additional_limits_become_per_model_windows() {
        let usage: CodexUsage = serde_json::from_str(
            r#"{
                "email": "someone@example.com",
                "plan_type": "pro",
                "rate_limit": {
                    "primary_window": {
                        "used_percent": 95,
                        "limit_window_seconds": 604800,
                        "reset_after_seconds": 459188,
                        "reset_at": 1788278566
                    },
                    "secondary_window": null
                },
                "code_review_rate_limit": null,
                "additional_rate_limits": [
                    {
                        "limit_name": "GPT-5.3-Codex-Spark",
                        "metered_feature": "codex_bengalfox",
                        "rate_limit": {
                            "primary_window": {
                                "used_percent": 7,
                                "limit_window_seconds": 18000,
                                "reset_at": 1787829104
                            },
                            "secondary_window": {
                                "used_percent": 48,
                                "limit_window_seconds": 604800,
                                "reset_at": 1788261728
                            }
                        }
                    }
                ]
            }"#,
        )
        .expect("the live shape parses");

        let windows = usage.windows();
        assert_eq!(windows.len(), 3);
        // The account window comes first and stays account-wide.
        assert_eq!(windows[0].model, None);
        assert_eq!(windows[0].label, "7d");
        assert_eq!(windows[0].remaining_percent, 5);
        // Then the model's own pair, in the order the payload lists them.
        assert_eq!(windows[1].model.as_deref(), Some("GPT-5.3-Codex-Spark"));
        assert_eq!(windows[1].label, "5h");
        assert_eq!(windows[1].remaining_percent, 93);
        assert_eq!(windows[2].model.as_deref(), Some("GPT-5.3-Codex-Spark"));
        assert_eq!(windows[2].label, "7d");
        assert_eq!(windows[2].remaining_percent, 52);
    }

    /// An explicit `null` where the array would be must not take the probe down
    /// with it — the sibling `code_review_rate_limit` proves the payload writes
    /// those.
    #[test]
    fn a_null_additional_limits_list_still_yields_the_account_window() {
        let usage: CodexUsage = serde_json::from_str(
            r#"{
                "rate_limit": {
                    "primary_window": {"used_percent": 20, "limit_window_seconds": 18000},
                    "secondary_window": null
                },
                "additional_rate_limits": null
            }"#,
        )
        .expect("an explicit null parses");

        let windows = usage.windows();
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].label, "5h");
    }

    /// An entry that names no model is dropped: its rows would be
    /// indistinguishable from the account window they sit beside.
    #[test]
    fn an_unnamed_additional_limit_is_dropped() {
        let usage: CodexUsage = serde_json::from_str(
            r#"{
                "rate_limit": null,
                "additional_rate_limits": [
                    {"limit_name": null,
                     "rate_limit": {"primary_window": {"used_percent": 3}, "secondary_window": null}},
                    {"limit_name": "", "rate_limit": null}
                ]
            }"#,
        )
        .expect("the live shape parses");

        assert!(usage.windows().is_empty());
    }

    /// The two cards say the same thing about a sign-in in the same words, and
    /// say nothing when there is nothing to say.
    #[test]
    fn the_account_label_folds_to_whatever_the_provider_knows() {
        // ChatGPT: both halves — the shape the vendor's own card prints.
        assert_eq!(
            account_label(Some("someone@example.com"), Some("pro")).as_deref(),
            Some("someone@example.com (Pro)")
        );
        // Anthropic: the plan alone, because the usage response names no person.
        assert_eq!(account_label(None, Some("max")).as_deref(), Some("Max"));
        assert_eq!(
            account_label(Some("someone@example.com"), None).as_deref(),
            Some("someone@example.com")
        );
        // Nothing known, and blanks count as nothing — the row folds.
        assert_eq!(account_label(None, None), None);
        assert_eq!(account_label(Some("  "), Some("")), None);
    }

    #[test]
    fn window_labels_read_as_durations() {
        assert_eq!(window_label(604_800), "7d");
        assert_eq!(window_label(18_000), "5h");
        assert_eq!(window_label(900), "15m");
    }

    /// A provider that reports nonsense must not invent headroom out of it.
    #[test]
    fn utilization_outside_the_scale_clamps_rather_than_wrapping() {
        assert_eq!(remaining_from_utilization(0.0), 100);
        assert_eq!(remaining_from_utilization(100.0), 0);
        assert_eq!(remaining_from_utilization(140.0), 0);
        assert_eq!(remaining_from_utilization(-20.0), 100);
        assert_eq!(remaining_from_utilization(f64::NAN), 0);
    }
}
