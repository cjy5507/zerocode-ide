//! Antigravity's plan figures over Google's Code Assist quota API.
//!
//! The only credential source is this machine's Google login, which zo and
//! this window share: [`crate::google_login`] owns the file, the key, the
//! OAuth client and the refresh, and this module is the quota reader on top
//! of it. A refreshed access token lives only for this scan — the login on
//! disk is written by the settings card and by zo, never by a gauge.
//!
//! The quota is asked for the project Google's answer names, and when it
//! names none, for the one zo resolved for the same login and keeps on file
//! (t-72144). The gauge never makes a project: a login with none to read gets
//! a sentence saying why and what makes one, never 「read failed — retry」.
//!
//! Both calls go out as the Antigravity IDE, exactly as zo's calls for the
//! same login do ([`AntigravityHeaders`]). Asked as this client instead,
//! Google named no project for hours and then refused the quota of the
//! project zo had recorded with a 403 (2026-10-27), while zo — same login,
//! same project — was served. The person chose zo's identity for these two
//! read-only calls once that refusal came back (2026-10-28).

use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use crate::google_login;
use crate::usage::{NamedWindow, ProviderUsage, UsageWindow};
use crate::usage_http;
use crate::vendor_cli::{CliCall, CliRunner as _, ProcessRunner, VendorCli};
use zerocode_core::civil::epoch_ms_of_iso;

const LOAD_CODE_ASSIST_URL: &str =
    "https://daily-cloudcode-pa.googleapis.com/v1internal:loadCodeAssist";
const RETRIEVE_QUOTA_URL: &str =
    "https://daily-cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota";
/// The two method names, as a refusal names the call it came back from.
const LOAD_CODE_ASSIST: &str = "loadCodeAssist";
const RETRIEVE_USER_QUOTA: &str = "retrieveUserQuota";
const BUCKET_WINDOW_MINUTES: u32 = 60;

/// The CLI version zo presents: this floor, raised by an installed `agy` CLI
/// and overridden by zo's own variable — zo's `ANTIGRAVITY_VERSION` and
/// `ANTIGRAVITY_VERSION_ENV`, which say why the floor moves.
const ANTIGRAVITY_VERSION: &str = "1.4.12";
const ANTIGRAVITY_VERSION_ENV: &str = "ZO_ANTIGRAVITY_VERSION";
const ANTIGRAVITY_CLI_BINARY: &str = "agy";
const ANTIGRAVITY_CLI_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
/// zo's `ANTIGRAVITY_API_CLIENT`.
const ANTIGRAVITY_API_CLIENT: &str = "google-cloud-sdk vscode_cloudshelleditor/0.1";

/// The headers of every Code Assist call: the Antigravity CLI identity zo's
/// `antigravity_headers` puts on every call it sends for this login. A test
/// holds each value to zo's source, so the two never drift apart.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AntigravityHeaders {
    user_agent: String,
    client_metadata: String,
}

impl AntigravityHeaders {
    fn new() -> Self {
        Self {
            user_agent: format!(
                "antigravity/cli/{} darwin/arm64",
                antigravity_version(
                    std::env::var(ANTIGRAVITY_VERSION_ENV).ok(),
                    installed_antigravity_version()
                )
            ),
            client_metadata: client_metadata(None).to_string(),
        }
    }

    fn pairs(&self) -> [(&str, &str); 3] {
        [
            ("User-Agent", &self.user_agent),
            ("X-Goog-Api-Client", ANTIGRAVITY_API_CLIENT),
            ("Client-Metadata", &self.client_metadata),
        ]
    }
}

/// zo's `client_metadata`: the IDE, this host's platform and the plugin, and
/// the configured project when there is one.
fn client_metadata(configured: Option<&str>) -> serde_json::Value {
    let mut metadata = serde_json::json!({
        "ideType": "ANTIGRAVITY",
        "platform": client_platform(),
        "pluginType": "GEMINI",
    });
    if let Some(project) = configured.filter(|project| !project.is_empty()) {
        metadata["duetProject"] = serde_json::Value::String(project.to_string());
    }
    metadata
}

/// zo's `client_platform`: the backend takes only its `{OS}_{ARCH}` enum
/// values, and the unspecified one where none fits.
fn client_platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "DARWIN_ARM64",
        ("macos", "x86_64") => "DARWIN_AMD64",
        ("linux", "aarch64") => "LINUX_ARM64",
        ("linux", "x86_64") => "LINUX_AMD64",
        ("windows", _) => "WINDOWS_AMD64",
        _ => "PLATFORM_UNSPECIFIED",
    }
}

/// The version zo's `antigravity_user_agent` presents: its variable when set
/// to something, else the newer of the installed `agy` CLI and the floor.
fn antigravity_version(overridden: Option<String>, installed: Option<String>) -> String {
    overridden
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| newest_version(installed, ANTIGRAVITY_VERSION))
}

/// zo's `newest_version`: compared numerically per dotted part, so `1.10.0`
/// is newer than `1.4.12`, and an older CLI does not lower the floor.
fn newest_version(installed: Option<String>, floor: &str) -> String {
    let parts = |version: &str| -> Vec<u64> {
        version
            .split('.')
            .map(|part| part.trim().parse::<u64>().unwrap_or(0))
            .collect()
    };
    match installed {
        Some(found) if parts(&found) > parts(floor) => found,
        _ => floor.to_string(),
    }
}

/// zo's `installed_antigravity_version`: the installed `agy` CLI version on
/// the hydrated login-shell `PATH`, cached once per hydrated `PATH` value.
fn installed_antigravity_version() -> Option<String> {
    type CachedCliVersion = (Option<String>, Option<String>);
    static INSTALLED_CLI_VERSION: Mutex<Option<CachedCliVersion>> = Mutex::new(None);

    let hydrated = crate::shell_path::hydrated();
    if let Ok(guard) = INSTALLED_CLI_VERSION.lock()
        && let Some((cached_path, cached_version)) = guard.as_ref()
        && *cached_path == hydrated
    {
        return cached_version.clone();
    }

    let args = ["--version".to_string()];
    let call = CliCall {
        cwd: None,
        args: &args,
        stdin: None,
        budget: Some(ANTIGRAVITY_CLI_PROBE_TIMEOUT),
    };
    let probed = ProcessRunner
        .run(VendorCli::new(ANTIGRAVITY_CLI_BINARY), &call)
        .ok()
        .filter(|output| output.success)
        .and_then(|output| cli_short_version(&output.stdout));

    if let Ok(mut guard) = INSTALLED_CLI_VERSION.lock() {
        *guard = Some((hydrated, probed.clone()));
    }
    probed
}

/// The dotted version printed by `agy --version`, when plausible.
fn cli_short_version(output: &str) -> Option<String> {
    output.split_whitespace().find_map(|token| {
        let version =
            token.trim_matches(|glyph: char| matches!(glyph, 'v' | 'V' | ',' | '(' | ')'));
        let plausible = version.contains('.')
            && !version.starts_with('.')
            && !version.ends_with('.')
            && version
                .chars()
                .all(|glyph| glyph.is_ascii_digit() || glyph == '.');
        plausible.then(|| version.to_string())
    })
}

/// The seam the tests replace: everything this reader says to a server.
///
/// A refresh is named here rather than called inline because the fake wire is
/// how the 60-second margin is measured without a network — the refresh itself
/// is [`google_login::refreshed_access_token`], which is also what the settings
/// card renews with.
trait Wire {
    fn refresh(&mut self, refresh_token: &str, now: i64) -> Option<String>;

    fn post_json(
        &mut self,
        url: &str,
        access_token: &str,
        headers: &AntigravityHeaders,
        body: &serde_json::Value,
        now: i64,
    ) -> Result<serde_json::Value, usage_http::Failure>;
}

struct LiveWire;

impl Wire for LiveWire {
    fn refresh(&mut self, refresh_token: &str, now: i64) -> Option<String> {
        google_login::refreshed_access_token(refresh_token, now)
    }

    fn post_json(
        &mut self,
        url: &str,
        access_token: &str,
        headers: &AntigravityHeaders,
        body: &serde_json::Value,
        now: i64,
    ) -> Result<serde_json::Value, usage_http::Failure> {
        usage_http::post_json_bearer(url, access_token, &headers.pairs(), body, now)
    }
}

/// The closed words of a refusal, for the window's log: the status Google
/// answered with and the call it refused. Never its body, never a token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Refusal {
    pub(crate) status: u16,
    pub(crate) call: &'static str,
}

/// Google refuses a call it will not serve with 400, 401 or 403. Anything else
/// is a failure with a sentence, not a refusal.
fn refusal_of(failure: &usage_http::Failure, call: &'static str) -> Option<Refusal> {
    match failure.http_status {
        Some(status @ (400 | 401 | 403)) => Some(Refusal { status, call }),
        _ => None,
    }
}

/// Where a read stopped short of its figures.
enum Stop {
    /// Google refused a call. The gauge says it could not verify the quota.
    Refused(Refusal),
    /// The call failed for another reason, and this is the answer that says so.
    Failed(Box<ProviderUsage>),
}

impl Stop {
    fn into_read(self, now: i64) -> (ProviderUsage, Option<Refusal>) {
        match self {
            Self::Refused(refusal) => (refused_answer(now), Some(refusal)),
            Self::Failed(usage) => (*usage, None),
        }
    }
}

/// Google refused a call this client made. The figures cannot be verified, and
/// no sentence names a fix a person can make, so the window does not ask again.
fn refused_answer(now: i64) -> ProviderUsage {
    answer(
        "refused",
        Some("Antigravity 사용량을 확인할 수 없습니다 — Google이 요청을 거절했습니다".to_string()),
        now,
    )
}

/// What Google says about this login's Code Assist project, asked the way
/// zo's `setup_user_for` asks it: about the configured project, with zo's
/// client metadata.
fn load_code_assist(
    wire: &mut impl Wire,
    access_token: &str,
    headers: &AntigravityHeaders,
    configured: Option<&str>,
    now: i64,
) -> Result<serde_json::Value, Stop> {
    wire.post_json(
        LOAD_CODE_ASSIST_URL,
        access_token,
        headers,
        &serde_json::json!({
            "cloudaicompanionProject": configured,
            "metadata": client_metadata(configured),
        }),
        now,
    )
    .map_err(|failure| match refusal_of(&failure, LOAD_CODE_ASSIST) {
        Some(refusal) => Stop::Refused(refusal),
        None => Stop::Failed(Box::new(answer(
            "error",
            Some(format!(
                "Antigravity 프로젝트 ID를 읽지 못했습니다 ({})",
                failure.message
            )),
            now,
        ))),
    })
}

/// The tier Google gives a personal login at no charge — the one zo takes for
/// a login with no tier yet without a project of its own.
const FREE_TIER: &str = "free-tier";

/// The project Google's answer names: the id itself, or an object carrying it
/// (`{"id": …}`). An empty id names none.
fn answered_project(load: &serde_json::Value) -> Option<String> {
    let named = load.get("cloudaicompanionProject")?;
    named
        .as_str()
        .or_else(|| named.get("id").and_then(serde_json::Value::as_str))
        .filter(|project| !project.is_empty())
        .map(str::to_string)
}

/// Whether Google's answer gives this login a tier now.
fn has_current_tier(load: &serde_json::Value) -> bool {
    load.get("currentTier").is_some_and(|tier| !tier.is_null())
}

/// The Code Assist project this login's quota is billed to, read and never
/// made: the project Google's answer names; else the one zo resolved for this
/// login under this configuration and keeps on file; else, for a login Google
/// gives a tier, the project the configuration names — zo's own answer for
/// such a login (`setup_user_from_load_response`). A login with no tier yet
/// takes one through `onboardUser`, a write that zo and Antigravity send and
/// this gauge never does.
fn project_of(
    load: &serde_json::Value,
    remembered: impl FnOnce() -> Option<String>,
    configured: Option<&str>,
) -> Result<String, NoProject> {
    answered_project(load)
        .or_else(remembered)
        .or_else(|| {
            configured
                .filter(|_| has_current_tier(load))
                .map(str::to_string)
        })
        .ok_or_else(|| NoProject::of(load, configured))
}

/// Why no project could be read, in the words of Google's own answer.
#[derive(Debug)]
enum NoProject {
    /// Google names no project — for a login it gives a tier, or with nothing
    /// to say at all — and zo has recorded none for this login either.
    Unnamed,
    /// No tier yet, and one on offer that zo would take for this login.
    NotOnboarded,
    /// No tier on offer, and Google's reasons against the ones it lists.
    NotEligible(String),
}

impl NoProject {
    /// Read off the answer the way zo decides what a login with no project
    /// needs (`setup_user_from_load_response`): a login with a tier lacks only
    /// its project; one without takes the default tier on offer when that is
    /// the free tier or a project is configured; otherwise Google's reasons,
    /// when it gives any, say why not.
    fn of(load: &serde_json::Value, configured: Option<&str>) -> Self {
        if has_current_tier(load) {
            return Self::Unnamed;
        }
        let takes_a_tier = default_allowed_tier(load).is_some_and(|tier| {
            configured.is_some()
                || tier.get("id").and_then(serde_json::Value::as_str) == Some(FREE_TIER)
        });
        if takes_a_tier {
            return Self::NotOnboarded;
        }
        ineligible_reasons(load).map_or(Self::Unnamed, Self::NotEligible)
    }

    /// What the bar's hover and the usage panel say: what is missing, and
    /// what makes it. zo records a login's project the first time it sends a
    /// Gemini request with that login, and the next read finds it.
    fn sentence(&self) -> String {
        match self {
            Self::Unnamed => "Google이 이 로그인의 Antigravity 프로젝트를 알려 주지 않았고 zo에도 기록이 없습니다 — zo에서 Gemini 모델을 한 번 쓰면 zo가 프로젝트를 기록합니다".to_string(),
            Self::NotOnboarded => "이 Google 로그인은 아직 Antigravity 프로젝트가 없습니다 — zo에서 Gemini 모델을 한 번 쓰면 만들어집니다".to_string(),
            Self::NotEligible(reasons) => {
                format!("Google이 이 계정에 Antigravity 프로젝트를 주지 않습니다 — {reasons}")
            }
        }
    }
}

/// The tier Google offers by default: the one marked default, else the first
/// (zo's `default_allowed_tier`).
fn default_allowed_tier(load: &serde_json::Value) -> Option<&serde_json::Value> {
    let tiers = load.get("allowedTiers")?.as_array()?;
    tiers
        .iter()
        .find(|tier| {
            tier.get("isDefault")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
        })
        .or_else(|| tiers.first())
}

/// Google's reasons against the tiers this login may not take, worded as zo
/// words them (`ineligible_or_project_required`): `tier: reason`, joined by
/// `; `. `None` when it lists none.
fn ineligible_reasons(load: &serde_json::Value) -> Option<String> {
    let said: Vec<String> = load
        .get("ineligibleTiers")?
        .as_array()?
        .iter()
        .map(|tier| {
            let name = tier
                .get("tierName")
                .or_else(|| tier.get("tierId"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("tier");
            let reason = tier
                .get("reasonMessage")
                .or_else(|| tier.get("validationErrorMessage"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("ineligible");
            format!("{name}: {reason}")
        })
        .collect();
    (!said.is_empty()).then(|| said.join("; "))
}

/// The status of a reading with no project to read. Its own word: the bar says
/// it as neither a failure nor a refusal.
const NO_PROJECT: &str = "no_project";

/// Google named no project and none can be read. Not a failure — the read went
/// through, and this is its answer — so it names no failure kind and asks for
/// no retry: the bar says the project is not set, and the sentence says why.
fn no_project(missing: &NoProject, now: i64) -> ProviderUsage {
    answer(NO_PROJECT, Some(missing.sentence()), now)
}

/// One quota bucket as the API speaks it, shape-checked the way Orca
/// shape-checks (`isQuotaBucket`) — a malformed row is dropped, not a crash.
struct QuotaBucket {
    remaining_fraction: f64,
    reset_time: String,
    model_id: String,
}

fn parse_quota_response(data: &serde_json::Value) -> Vec<QuotaBucket> {
    let rows = if let Some(rows) = data.as_array() {
        rows.as_slice()
    } else if let Some(rows) = data.get("buckets").and_then(serde_json::Value::as_array) {
        rows.as_slice()
    } else {
        &[]
    };
    rows.iter()
        .filter_map(|row| {
            let fraction = row.get("remainingFraction")?.as_f64()?;
            if !fraction.is_finite() {
                return None;
            }
            Some(QuotaBucket {
                remaining_fraction: fraction,
                reset_time: row.get("resetTime")?.as_str()?.to_string(),
                model_id: row.get("modelId")?.as_str()?.to_string(),
            })
        })
        .collect()
}

/* ---- bucket dressing ---------------------------------------------------- */

/// The model ids Orca names by hand; anything else is humanized from its id.
const MODEL_BUCKET_NAMES: &[(&str, &str)] = &[
    ("gemini-3.1-pro", "3.1 Pro"),
    ("gemini-3.1-flash", "3.1 Flash"),
    ("gemini-3.1-flash-lite", "3.1 Flash Lite"),
    ("gemini-3.0-pro", "3.0 Pro"),
    ("gemini-3.0-flash", "3.0 Flash"),
    ("gemini-2.5-pro", "Pro"),
    ("gemini-2.5-flash", "Flash"),
    ("gemini-2.5-flash-lite", "Flash Lite"),
    ("gemini-2.0-pro", "2.0 Pro"),
    ("gemini-2.0-flash", "2.0 Flash"),
    ("gemini-2.0-flash-lite", "2.0 Flash Lite"),
    ("gemini-1.5-pro", "1.5 Pro"),
    ("gemini-1.5-flash", "1.5 Flash"),
    ("gemini-exp", "Exp"),
    ("gemini-experimental", "Exp"),
];

fn named_bucket(model_id: &str) -> Option<&'static str> {
    MODEL_BUCKET_NAMES
        .iter()
        .find(|(id, _)| *id == model_id)
        .map(|(_, name)| *name)
}

/// `gemini-2.5-something-new` → `2.5 Something New`; tiered serving ids like
/// `gemini-4-argon-low` drop their tier suffix (`4 Argon`) so shared-quota
/// families do not carry a random rung label into the bar.
fn humanize_model_id(model_id: &str) -> String {
    let without = model_id
        .strip_prefix("gemini-")
        .or_else(|| model_id.strip_prefix("Gemini-"))
        .unwrap_or(model_id);
    let without = [
        "-extra-low",
        "-xhigh",
        "-max",
        "-low",
        "-medium",
        "-high",
        "-tiered",
    ]
    .into_iter()
    .find_map(|suffix| without.strip_suffix(suffix).filter(|stem| !stem.is_empty()))
    .unwrap_or(without);
    without
        .split('-')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn bucket_name(model_id: &str) -> String {
    named_bucket(model_id).map_or_else(|| humanize_model_id(model_id), str::to_string)
}

fn build_bucket(bucket: &QuotaBucket) -> NamedWindow {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let used_percent =
        (((1.0 - bucket.remaining_fraction) * 100.0).round()).clamp(0.0, 100.0) as u8;
    NamedWindow {
        name: bucket_name(&bucket.model_id),
        window: UsageWindow {
            used_percent,
            window_minutes: BUCKET_WINDOW_MINUTES,
            resets_at: epoch_ms_of_iso(&bucket.reset_time),
            reset_description: None,
        },
    }
}

/// Same percent, same reset: one row. The known-model name wins over the
/// derived one; between equals the shorter name stays (`deduplicateBuckets`).
fn deduplicate_buckets(buckets: Vec<(NamedWindow, String)>) -> Vec<NamedWindow> {
    let mut kept: Vec<(NamedWindow, String)> = Vec::new();
    for (window, model_id) in buckets {
        let key_of = |held: &NamedWindow| (held.window.used_percent, held.window.resets_at);
        let key = key_of(&window);
        match kept.iter_mut().find(|(held, _)| key_of(held) == key) {
            None => kept.push((window, model_id)),
            Some((held, held_model)) => {
                let held_known = named_bucket(held_model).is_some();
                let new_known = named_bucket(&model_id).is_some();
                if (new_known && !held_known)
                    || (new_known == held_known && window.name.len() < held.name.len())
                {
                    *held = window;
                    *held_model = model_id;
                }
            }
        }
    }
    kept.into_iter().map(|(window, _)| window).collect()
}

/// The segment's one figure: the most constrained bucket, nameless
/// (`deriveSessionSummary`).
fn session_summary(buckets: &[NamedWindow]) -> Option<UsageWindow> {
    buckets
        .iter()
        .max_by_key(|held| held.window.used_percent)
        .map(|held| held.window.clone())
}

/* ---- the fetch, assembled ----------------------------------------------- */

fn answer(status: &str, error: Option<String>, now: i64) -> ProviderUsage {
    ProviderUsage {
        provider: "antigravity".to_string(),
        session: None,
        weekly: None,
        fable_weekly: None,
        monthly: None,
        buckets: None,
        updated_at: now,
        error,
        status: status.to_string(),
        failure_kind: None,
        retry_at_ms: None,
        plan_type: None,
        reset_credits: None,
        account: None,
    }
}

fn fetch_quota(
    wire: &mut impl Wire,
    access_token: &str,
    project_id: &str,
    headers: &AntigravityHeaders,
    now: i64,
) -> Result<ProviderUsage, Stop> {
    match wire.post_json(
        RETRIEVE_QUOTA_URL,
        access_token,
        headers,
        &serde_json::json!({ "project": project_id }),
        now,
    ) {
        // The KIND rides back on the answer, so the bar words it without
        // reading a sentence. A refusal is not a kind: it ends the read.
        Err(failure) => match refusal_of(&failure, RETRIEVE_USER_QUOTA) {
            Some(refusal) => Err(Stop::Refused(refusal)),
            None => Ok(ProviderUsage {
                failure_kind: Some(failure.recovery.kind),
                retry_at_ms: failure.retry_at_ms,
                ..answer(
                    "error",
                    Some(format!("Quota fetch failed ({})", failure.message)),
                    now,
                )
            }),
        },
        Ok(body) => {
            let buckets = deduplicate_buckets(
                parse_quota_response(&body)
                    .iter()
                    .map(|bucket| (build_bucket(bucket), bucket.model_id.clone()))
                    .collect(),
            );
            Ok(ProviderUsage {
                provider: "antigravity".to_string(),
                session: session_summary(&buckets),
                weekly: None,
                fable_weekly: None,
                monthly: None,
                buckets: Some(buckets),
                updated_at: now,
                error: None,
                status: "ok".to_string(),
                failure_kind: None,
                retry_at_ms: None,
                plan_type: None,
                reset_credits: None,
                account: None,
            })
        }
    }
}

/// No login on file at all.
///
/// The sentence names the login that is missing and the surface that FIXES
/// it. It used to name a zo command in a terminal, which sent a person out of
/// the window for something the settings pane now does — and the status bar
/// shows this sentence as the segment's tooltip, so it is the only instruction
/// most people ever see. The login is Google's, not Antigravity's: the card
/// that makes it is the Google card, and the bar's word says the same
/// (t-70039).
fn signed_out(now: i64) -> ProviderUsage {
    answer(
        "signed_out",
        Some("Google 로그인 필요 — 설정 > AI 제공자 계정".to_string()),
        now,
    )
}

/// A token this scan can use, renewed through the wire when it is spent. The
/// margin itself is [`google_login::needs_refresh`] — shared with the card,
/// which renews the same login on its own road.
fn access_token(
    wire: &mut impl Wire,
    credentials: &google_login::Credentials,
    now: i64,
) -> Option<String> {
    if !google_login::needs_refresh(credentials, now) {
        return Some(credentials.access_token.clone());
    }
    wire.refresh(credentials.refresh_token.as_deref()?, now)
}

/// One read: the reading, and the refusal behind it when Google refused a call.
/// A refused login is not renewed and the refused call is not sent again.
/// `configured` is the project the configuration names, which also picks the
/// record of zo's that answers for it.
fn scan_with(
    path: Option<&Path>,
    configured: Option<&str>,
    now: i64,
    wire: &mut impl Wire,
) -> (ProviderUsage, Option<Refusal>) {
    let Some(path) = path else {
        return (signed_out(now), None);
    };
    let credentials = match google_login::read(path) {
        Ok(Some(credentials)) => credentials,
        Ok(None) => return (signed_out(now), None),
        Err(error) => return (answer("error", Some(error), now), None),
    };
    let Some(access_token) = access_token(wire, &credentials, now) else {
        return (
            answer(
                "error",
                Some("Antigravity Google 토큰을 갱신하지 못했습니다".to_string()),
                now,
            ),
            None,
        );
    };
    let headers = AntigravityHeaders::new();
    let load = match load_code_assist(wire, &access_token, &headers, configured, now) {
        Ok(load) => load,
        Err(stop) => return stop.into_read(now),
    };
    let remembered = || google_login::remembered_project(path, &credentials, configured);
    let project = match project_of(&load, remembered, configured) {
        Ok(project) => project,
        Err(missing) => return (no_project(&missing, now), None),
    };
    match fetch_quota(wire, &access_token, &project, &headers, now) {
        Ok(usage) => (usage, None),
        Err(stop) => stop.into_read(now),
    }
}

/// Read this machine's Google login and fetch Antigravity quota on the
/// caller's clock. A refused read names the call and the status it was refused
/// with, for the window's log and nothing else.
pub(crate) fn scan(now: i64) -> (ProviderUsage, Option<Refusal>) {
    let path = google_login::credentials_path();
    let configured = google_login::configured_project();
    scan_with(path.as_deref(), configured.as_deref(), now, &mut LiveWire)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zerocode_core::usage_limit;

    /// The bucket arithmetic, Orca's own: percent from the remaining
    /// fraction (rounded, clamped), an hour-wide window, the reset off the
    /// ISO stamp — and a stamp that does not parse keeps the row.
    #[test]
    fn a_bucket_is_dressed_from_the_remaining_fraction() {
        let dressed = build_bucket(&QuotaBucket {
            remaining_fraction: 0.25,
            reset_time: "2026-08-18T03:00:00Z".to_string(),
            model_id: "gemini-2.5-pro".to_string(),
        });
        assert_eq!(dressed.name, "Pro");
        assert_eq!(dressed.window.used_percent, 75);
        assert_eq!(dressed.window.window_minutes, 60);
        assert!(dressed.window.resets_at.is_some());

        let unstamped = build_bucket(&QuotaBucket {
            remaining_fraction: 1.0,
            reset_time: "sometime".to_string(),
            model_id: "gemini-9.9-new-thing".to_string(),
        });
        assert_eq!(unstamped.name, "9.9 New Thing", "the humanizer moved");
        assert_eq!(unstamped.window.used_percent, 0);
        assert_eq!(unstamped.window.resets_at, None);
    }

    /// Same percent and same reset are one row, the known-model name winning
    /// over the derived one, the shorter between equals — Orca's
    /// `deduplicateBuckets`, decision for decision.
    #[test]
    fn duplicate_buckets_collapse_and_the_known_name_wins() {
        let stamp = Some(1_755_500_000_000);
        let window = |name: &str| NamedWindow {
            name: name.to_string(),
            window: UsageWindow {
                used_percent: 40,
                window_minutes: 60,
                resets_at: stamp,
                reset_description: None,
            },
        };
        let kept = deduplicate_buckets(vec![
            (
                window("2.5 Preview Something"),
                "gemini-2.5-preview-something".to_string(),
            ),
            (window("Pro"), "gemini-2.5-pro".to_string()),
            (window("Flash"), "gemini-2.5-flash".to_string()),
        ]);
        assert_eq!(kept.len(), 1, "equal readings kept separate rows");
        // The known-name rule fires before the shorter-name rule ever could:
        // `Pro` replaced the derived row, and `Flash` (also known, longer)
        // then failed the equal-tier length test.
        assert_eq!(kept[0].name, "Pro");

        // Different resets are different rows even at the same percent.
        let mut other = window("Flash");
        other.window.resets_at = Some(1_755_500_000_001);
        let two = deduplicate_buckets(vec![
            (window("Pro"), "gemini-2.5-pro".to_string()),
            (other, "gemini-2.5-flash".to_string()),
        ]);
        assert_eq!(two.len(), 2);
    }

    /// The segment shows the most constrained bucket, nameless.
    #[test]
    fn the_session_summary_is_the_most_constrained_bucket() {
        let bucket = |name: &str, used: u8| NamedWindow {
            name: name.to_string(),
            window: UsageWindow {
                used_percent: used,
                window_minutes: 60,
                resets_at: None,
                reset_description: None,
            },
        };
        let summary =
            session_summary(&[bucket("Pro", 30), bucket("Flash", 80), bucket("Lite", 10)])
                .expect("summary");
        assert_eq!(summary.used_percent, 80);
        assert_eq!(session_summary(&[]), None);
    }

    struct PostedCall {
        url: String,
        access_token: String,
        headers: Vec<(String, String)>,
        body: serde_json::Value,
    }

    struct FakeWire {
        refreshes: usize,
        posts: Vec<PostedCall>,
        /// The call the server refuses, and the status it refuses with.
        refusal: Option<(&'static str, u16)>,
        /// What `loadCodeAssist` answers — a synthetic shape of Google's.
        load: serde_json::Value,
    }

    impl Wire for FakeWire {
        fn refresh(&mut self, _refresh_token: &str, _now: i64) -> Option<String> {
            self.refreshes += 1;
            Some("fresh-access".to_string())
        }

        fn post_json(
            &mut self,
            url: &str,
            access_token: &str,
            headers: &AntigravityHeaders,
            body: &serde_json::Value,
            _now: i64,
        ) -> Result<serde_json::Value, usage_http::Failure> {
            self.posts.push(PostedCall {
                url: url.to_string(),
                access_token: access_token.to_string(),
                headers: headers
                    .pairs()
                    .into_iter()
                    .map(|(name, value)| (name.to_string(), value.to_string()))
                    .collect(),
                body: body.clone(),
            });
            if let Some((_, status)) = self.refusal.filter(|(refused, _)| *refused == url) {
                return Err(usage_http::Failure {
                    recovery: usage_limit::classify_http(status, ""),
                    retry_at_ms: None,
                    message: format!("HTTP {status}"),
                    skip_cli_fallback: usage_limit::skips_pty_fallback(status),
                    http_status: Some(status),
                });
            }
            if url == LOAD_CODE_ASSIST_URL {
                Ok(self.load.clone())
            } else {
                Ok(serde_json::json!({ "buckets": [{
                    "remainingFraction": 0.5,
                    "resetTime": "2026-08-18T00:00:00Z",
                    "modelId": "gemini-2.5-pro"
                }] }))
            }
        }
    }

    fn fake_wire() -> FakeWire {
        FakeWire {
            refreshes: 0,
            posts: Vec::new(),
            refusal: None,
            load: serde_json::json!({ "cloudaicompanionProject": "project-a" }),
        }
    }

    /// A live login on file whose refresh token the tests can fingerprint,
    /// and the project record zo keeps beside it, when there is one.
    fn login_on_file(dir: &Path, record: Option<serde_json::Value>) -> std::path::PathBuf {
        let path = dir.join("credentials.json");
        let mut root = serde_json::json!({
            "google_code_assist_oauth": {
                "accessToken": "live-access",
                "refreshToken": "refresh-me",
                "expiresAt": 9_999_999_999_u64
            }
        });
        if let Some(record) = record {
            root["google_code_assist_project"] = record;
        }
        std::fs::write(&path, root.to_string()).expect("credentials");
        path
    }

    const NOW: i64 = 1_800_000_000_000;

    /// The quota call's project, when one went out.
    fn asked_project(wire: &FakeWire) -> Option<serde_json::Value> {
        wire.posts
            .iter()
            .find(|posted| posted.url == RETRIEVE_QUOTA_URL)
            .map(|posted| posted.body["project"].clone())
    }

    /// Google spells the project two ways — the id itself, or an object
    /// carrying it — and the quota is asked for that project either way
    /// (t-72144). An empty id, or an object with none, is no project.
    #[test]
    fn the_project_is_read_off_either_spelling_google_answers_with() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = login_on_file(dir.path(), None);
        let tier = serde_json::json!({ "id": "free-tier" });
        for (load, project) in [
            (
                serde_json::json!({ "currentTier": tier, "cloudaicompanionProject": "project-a" }),
                Some("project-a"),
            ),
            (
                serde_json::json!({
                    "currentTier": tier,
                    "cloudaicompanionProject": { "id": "project-b", "name": "synthetic" }
                }),
                Some("project-b"),
            ),
            (
                serde_json::json!({ "currentTier": tier, "cloudaicompanionProject": "" }),
                None,
            ),
            (
                serde_json::json!({
                    "currentTier": tier,
                    "cloudaicompanionProject": { "name": "synthetic" }
                }),
                None,
            ),
        ] {
            let mut wire = FakeWire {
                load: load.clone(),
                ..fake_wire()
            };
            let (held, refusal) = scan_with(Some(&path), None, NOW, &mut wire);
            assert_eq!(refusal, None);
            match project {
                Some(project) => {
                    assert_eq!(held.status, "ok", "{load} read as {:?}", held.error);
                    assert_eq!(asked_project(&wire), Some(serde_json::json!(project)));
                }
                None => {
                    assert_eq!(held.status, "no_project", "{load} read as {:?}", held.error);
                    assert_eq!(asked_project(&wire), None, "{load} asked a quota");
                }
            }
        }
    }

    /// When Google's answer names no project, the gauge reads the one zo
    /// resolved for this same login under this same configuration — zo's
    /// project record, matched by grant and configured project the way zo
    /// matches it (t-72144). Another login's record is not this login's, and
    /// neither is one made under another configuration: there, the configured
    /// project stands in, as it does in zo for a login with a tier.
    #[test]
    fn a_login_google_names_no_project_for_reads_the_one_zo_recorded_for_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let grant = google_login::grant_fingerprint("refresh-me");
        let unnamed = serde_json::json!({
            "currentTier": { "id": "free-tier" },
            "allowedTiers": [{ "id": "free-tier", "isDefault": true }]
        });
        for (record, configured, project) in [
            (
                serde_json::json!({ "grant": grant, "configured_project": null, "project": "zo-project" }),
                None,
                Some("zo-project"),
            ),
            (
                serde_json::json!({ "grant": grant, "configured_project": "configured-project", "project": "zo-project" }),
                Some("configured-project"),
                Some("zo-project"),
            ),
            (
                serde_json::json!({ "grant": grant, "configured_project": null, "project": "zo-project" }),
                Some("configured-project"),
                Some("configured-project"),
            ),
            (
                serde_json::json!({ "grant": "0123456789abcdef", "configured_project": null, "project": "theirs" }),
                None,
                None,
            ),
        ] {
            let path = login_on_file(dir.path(), Some(record.clone()));
            let mut wire = FakeWire {
                load: unnamed.clone(),
                ..fake_wire()
            };
            let (held, _) = scan_with(Some(&path), configured, NOW, &mut wire);
            match project {
                Some(project) => {
                    assert_eq!(held.status, "ok", "{record} read as {:?}", held.error);
                    assert_eq!(asked_project(&wire), Some(serde_json::json!(project)));
                }
                None => {
                    assert_eq!(
                        held.status, "no_project",
                        "{record} read as {:?}",
                        held.error
                    );
                    assert_eq!(asked_project(&wire), None, "{record} asked a quota");
                }
            }
        }
    }

    /// A project the configuration names stands in only where zo's own order
    /// takes it: for a login Google gives a tier and names no project for. A
    /// login with no tier yet is not given one by naming a project — that is
    /// onboarding, a write this gauge never makes.
    #[test]
    fn a_configured_project_stands_in_only_for_a_login_with_a_tier() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = login_on_file(dir.path(), None);
        let mut tiered = FakeWire {
            load: serde_json::json!({ "currentTier": { "id": "standard-tier" } }),
            ..fake_wire()
        };
        let (held, _) = scan_with(Some(&path), Some("configured-project"), NOW, &mut tiered);
        assert_eq!(held.status, "ok", "{:?}", held.error);
        assert_eq!(
            asked_project(&tiered),
            Some(serde_json::json!("configured-project"))
        );

        let mut untiered = FakeWire {
            load: serde_json::json!({ "allowedTiers": [{ "id": "standard-tier", "isDefault": true }] }),
            ..fake_wire()
        };
        let (held, _) = scan_with(Some(&path), Some("configured-project"), NOW, &mut untiered);
        assert_eq!(held.status, "no_project", "{:?}", held.error);
        assert_eq!(asked_project(&untiered), None);
    }

    /// With no project to read anywhere, the gauge says why in the words of
    /// Google's own answer, and asks nothing more: no quota call, no renewed
    /// login, no failure kind, and never 「read failed — retry」 (t-72144). A
    /// login with no tier yet, one Google gives a tier but no project, and one
    /// Google lists reasons against are three different sentences.
    #[test]
    fn with_no_project_to_read_the_gauge_says_why_and_asks_nothing_more() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = login_on_file(dir.path(), None);
        let reason = "synthetic: not offered to this account";
        let mut said = Vec::new();
        for load in [
            serde_json::json!({ "currentTier": { "id": "standard-tier" } }),
            serde_json::json!({}),
            serde_json::json!({ "allowedTiers": [{ "id": "free-tier", "isDefault": true }] }),
            serde_json::json!({
                "allowedTiers": [{ "id": "standard-tier", "isDefault": true }],
                "ineligibleTiers": [{ "tierName": "Synthetic Tier", "reasonMessage": reason }]
            }),
        ] {
            let mut wire = FakeWire {
                load: load.clone(),
                ..fake_wire()
            };
            let (held, refusal) = scan_with(Some(&path), None, NOW, &mut wire);
            assert_eq!(refusal, None);
            assert_eq!(held.status, "no_project", "{load} read as {:?}", held.error);
            assert_eq!(held.failure_kind, None, "{load} named a failure kind");
            assert_eq!(wire.posts.len(), 1, "{load} asked more than loadCodeAssist");
            assert_eq!(wire.refreshes, 0);
            let sentence = held.error.clone().expect("a no-project read says why");
            for secret in ["live-access", "refresh-me"] {
                assert!(
                    !sentence.contains(secret),
                    "the sentence carries `{secret}`"
                );
            }
            let line = crate::usage_runtime::usage_read_line(
                "antigravity",
                crate::usage_runtime::UsageRoad::Api,
                12,
                Some(&held),
                false,
            );
            assert_eq!(line, "usage antigravity road=api ms=12 status=no_project");
            said.push(sentence);
        }
        assert_eq!(said[0], said[1], "two answers naming no project read apart");
        assert_ne!(
            said[0], said[2],
            "a login with no tier yet read as one with a tier"
        );
        assert!(
            said[3].contains(&format!("Synthetic Tier: {reason}")),
            "Google's reason is not in the sentence: {}",
            said[3]
        );
    }

    #[test]
    fn missing_zo_google_login_is_signed_out() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut wire = fake_wire();
        let (held, _) = scan_with(
            Some(&dir.path().join("credentials.json")),
            None,
            7,
            &mut wire,
        );
        assert_eq!(held.provider, "antigravity");
        assert_eq!(held.status, "signed_out");
        // And the sentence points at the surface that fixes it, because this
        // is what the status bar shows on hover.
        assert_eq!(
            held.error.as_deref(),
            Some("Google 로그인 필요 — 설정 > AI 제공자 계정")
        );
        assert_eq!(wire.refreshes, 0);
        assert!(wire.posts.is_empty());
    }

    #[test]
    fn an_expiring_token_refreshes_and_asks_as_antigravity_without_writing_zo() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("credentials.json");
        let original = r#"{
  "kept": true,
  "google_code_assist_oauth": {
    "accessToken": "old-access",
    "refreshToken": "refresh-me",
    "expiresAt": 1800000030
  }
}"#;
        std::fs::write(&path, original).expect("credentials");
        let mut wire = fake_wire();

        let (held, _) = scan_with(Some(&path), None, 1_800_000_000_000, &mut wire);

        assert_eq!(held.status, "ok", "{:?}", held.error);
        assert_eq!(wire.refreshes, 1, "the 60-second refresh margin moved");
        assert_eq!(wire.posts.len(), 2);
        assert!(
            wire.posts
                .iter()
                .all(|request| request.access_token == "fresh-access")
        );
        // zo's identity on both calls: the IDE's User-Agent, its API client and
        // its client metadata, and nothing that names this window.
        for request in &wire.posts {
            let header = |name: &str| {
                request
                    .headers
                    .iter()
                    .find(|(held, _)| held == name)
                    .map(|(_, value)| value.clone())
                    .unwrap_or_else(|| panic!("{} went out without {name}", request.url))
            };
            let agent = header("User-Agent");
            assert!(
                agent.starts_with("antigravity/cli/") && agent.ends_with(" darwin/arm64"),
                "{} went out as {agent}",
                request.url
            );
            assert_eq!(header("X-Goog-Api-Client"), ANTIGRAVITY_API_CLIENT);
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&header("Client-Metadata"))
                    .expect("the client metadata is JSON"),
                serde_json::json!({
                    "ideType": "ANTIGRAVITY",
                    "platform": client_platform(),
                    "pluginType": "GEMINI"
                })
            );
            assert_eq!(request.headers.len(), 3, "{:?}", request.headers);
        }
        assert_eq!(wire.posts[0].url, LOAD_CODE_ASSIST_URL);
        assert_eq!(wire.posts[1].url, RETRIEVE_QUOTA_URL);
        assert_eq!(
            wire.posts[0].body,
            serde_json::json!({
                "cloudaicompanionProject": null,
                "metadata": {
                    "ideType": "ANTIGRAVITY",
                    "platform": client_platform(),
                    "pluginType": "GEMINI"
                }
            }),
            "the loadCodeAssist body is not the one zo sends"
        );
        assert_eq!(
            std::fs::read_to_string(path).expect("credentials after scan"),
            original,
            "the window wrote its refreshed access token into zo's store"
        );
    }

    /// A configured project rides the loadCodeAssist body the way zo's
    /// `setup_user_for` sends it: named in the request and in the metadata.
    #[test]
    fn a_configured_project_is_asked_about_the_way_zo_asks() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = login_on_file(dir.path(), None);
        let mut wire = fake_wire();

        scan_with(Some(&path), Some("acme-project"), NOW, &mut wire);

        let load = &wire.posts[0];
        assert_eq!(load.url, LOAD_CODE_ASSIST_URL);
        assert_eq!(load.body["cloudaicompanionProject"], "acme-project");
        assert_eq!(load.body["metadata"]["duetProject"], "acme-project");
        assert_eq!(
            load.headers
                .iter()
                .find(|(name, _)| name == "Client-Metadata")
                .map(|(_, value)| value.contains("duetProject")),
            Some(false),
            "the header's metadata names a project; zo's never does"
        );
    }

    /// The gauge's identity is zo's, value for value: the version floor and its
    /// variable, the API client, the User-Agent's shape, the three headers, the
    /// metadata and the platform table all stand in zo's own source. A zo that
    /// moves any of them turns this red until the gauge moves with it.
    #[test]
    fn the_gauge_asks_as_the_identity_zo_sends_for_the_same_login() {
        let zo = include_str!("../../../zo-ide/crates/api/src/providers/gemini_code_assist.rs");
        // Both calls go to the host zo calls.
        let host_of = |url: &'static str| url.rsplit_once('/').map(|(host, _)| host);
        let host = host_of(LOAD_CODE_ASSIST_URL).expect("the load URL's host");
        assert_eq!(host_of(RETRIEVE_QUOTA_URL), Some(host));
        let platform_rows = [
            r#"("macos", "aarch64") => "DARWIN_ARM64","#,
            r#"("macos", "x86_64") => "DARWIN_AMD64","#,
            r#"("linux", "aarch64") => "LINUX_ARM64","#,
            r#"("linux", "x86_64") => "LINUX_AMD64","#,
            r#"("windows", _) => "WINDOWS_AMD64","#,
            r#"_ => "PLATFORM_UNSPECIFIED","#,
        ];
        let pinned = [
            format!(r#"const ANTIGRAVITY_VERSION: &str = "{ANTIGRAVITY_VERSION}";"#),
            format!(r#"const ANTIGRAVITY_VERSION_ENV: &str = "{ANTIGRAVITY_VERSION_ENV}";"#),
            format!(r#"const ANTIGRAVITY_CLI_BINARY: &str = "{ANTIGRAVITY_CLI_BINARY}";"#),
            format!(r#"const ANTIGRAVITY_API_CLIENT: &str = "{ANTIGRAVITY_API_CLIENT}";"#),
            format!(r#"const DEFAULT_CODE_ASSIST_ENDPOINT: &str = "{host}";"#),
            r#"format!("antigravity/cli/{version} darwin/arm64")"#.to_string(),
            r#".header("User-Agent", antigravity_user_agent())"#.to_string(),
            r#".header("X-Goog-Api-Client", ANTIGRAVITY_API_CLIENT)"#.to_string(),
            r#".header("Client-Metadata", client_metadata(None).to_string())"#.to_string(),
            r#""ideType": "ANTIGRAVITY","#.to_string(),
            r#""platform": client_platform(),"#.to_string(),
            r#""pluginType": "GEMINI","#.to_string(),
            r#"metadata["duetProject"] = Value::String(project.to_string());"#.to_string(),
            r#""cloudaicompanionProject": project_id,"#.to_string(),
            r#"fn cli_short_version(output: &str) -> Option<String>"#.to_string(),
        ]
        .into_iter()
        .chain(platform_rows.iter().map(|row| (*row).to_string()));
        for line in pinned {
            assert!(
                zo.contains(&line),
                "zo no longer sends `{line}` — move the gauge's identity with it"
            );
        }
        for (os, arch) in [
            ("macos", "aarch64"),
            ("macos", "x86_64"),
            ("linux", "aarch64"),
            ("linux", "x86_64"),
        ] {
            assert!(
                platform_rows
                    .iter()
                    .any(|row| row.starts_with(&format!(r#"("{os}", "{arch}")"#))),
                "the pinned table lost {os}/{arch}"
            );
        }
    }

    /// The version zo presents: its variable when set to something, else the
    /// newer of the installed `agy` CLI and the floor, numerically per part.
    #[test]
    fn the_version_is_zos_variable_else_the_newer_of_the_cli_and_the_floor() {
        let some = |text: &str| Some(text.to_string());
        for (overridden, installed, presented) in [
            (some(" 1.9.0 "), some("1.6.0"), "1.9.0"),
            (some("   "), some("1.6.0"), "1.6.0"),
            (None, some("1.10.0"), "1.10.0"),
            (None, some("1.3.9"), ANTIGRAVITY_VERSION),
            (None, None, ANTIGRAVITY_VERSION),
        ] {
            assert_eq!(
                antigravity_version(overridden.clone(), installed.clone()),
                presented,
                "{overridden:?} over {installed:?}"
            );
        }
        assert_eq!(cli_short_version("1.4.12\n").as_deref(), Some("1.4.12"));
        assert_eq!(
            cli_short_version("agy version v1.5.2 (darwin/arm64)").as_deref(),
            Some("1.5.2")
        );
        assert_eq!(cli_short_version("1.5-beta"), None);
        assert_eq!(cli_short_version(""), None);
        assert_eq!(humanize_model_id("gemini-4-argon-low"), "4 Argon");
        assert_eq!(humanize_model_id("gemini-4-argon-high"), "4 Argon");
    }

    /// A refusal from Google is an answer, not a reason to ask again. The gauge
    /// says it could not verify the quota, the refused request is not sent a
    /// second time in this scan, and the login is not renewed to try again. The
    /// old retry-once road did both, so every poll of a refused login renewed it.
    #[test]
    fn a_refused_read_is_unverified_and_is_not_asked_again() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("credentials.json");
        std::fs::write(
            &path,
            r#"{
  "google_code_assist_oauth": {
    "accessToken": "live-access",
    "refreshToken": "refresh-me",
    "expiresAt": 9999999999
  }
}"#,
        )
        .expect("credentials");
        let now = 1_800_000_000_000;
        for (refused, call, status, asked) in [
            (LOAD_CODE_ASSIST_URL, LOAD_CODE_ASSIST, 400, 1),
            (LOAD_CODE_ASSIST_URL, LOAD_CODE_ASSIST, 401, 1),
            (LOAD_CODE_ASSIST_URL, LOAD_CODE_ASSIST, 403, 1),
            (RETRIEVE_QUOTA_URL, RETRIEVE_USER_QUOTA, 401, 2),
            (RETRIEVE_QUOTA_URL, RETRIEVE_USER_QUOTA, 403, 2),
        ] {
            let mut wire = FakeWire {
                refusal: Some((refused, status)),
                ..fake_wire()
            };
            let (held, refusal) = scan_with(Some(&path), None, now, &mut wire);
            assert_eq!(
                refusal,
                Some(Refusal { status, call }),
                "HTTP {status} from {refused} was not named as the refusal it is"
            );
            assert_eq!(
                held.status, "refused",
                "HTTP {status} from {refused} was read as {}",
                held.status
            );
            assert_eq!(
                held.failure_kind, None,
                "a refusal named a failure kind, and the bar words that kind as a sign-in prompt"
            );
            assert_eq!(
                wire.refreshes, 0,
                "a refused login was renewed to ask again"
            );
            assert_eq!(
                wire.posts.len(),
                asked,
                "HTTP {status} from {refused} was sent again within one scan"
            );
        }
    }

    /// A refusal reaches the window's log as one line of closed words: the road,
    /// the HTTP status and the call that was refused, then the time and the
    /// answer. The body, the account and the token are not in it.
    #[test]
    fn a_refusal_is_one_log_line_of_its_status_and_call() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("credentials.json");
        std::fs::write(
            &path,
            r#"{
  "google_code_assist_oauth": {
    "accessToken": "live-access",
    "refreshToken": "refresh-me",
    "expiresAt": 9999999999
  }
}"#,
        )
        .expect("credentials");
        let mut wire = FakeWire {
            refusal: Some((LOAD_CODE_ASSIST_URL, 401)),
            ..fake_wire()
        };
        let (held, refusal) = scan_with(Some(&path), None, 1_800_000_000_000, &mut wire);
        let refusal = refusal.expect("the refusal was not named");
        let line = crate::usage_runtime::usage_read_line(
            "antigravity",
            crate::usage_runtime::UsageRoad::Refused {
                status: refusal.status,
                call: refusal.call,
            },
            12,
            Some(&held),
            false,
        );
        assert_eq!(
            line, "usage antigravity road=api http=401 call=loadCodeAssist ms=12 status=refused",
            "a refusal wrote more than its status and its call to the log"
        );
        for secret in ["live-access", "refresh-me", "account"] {
            assert!(
                !line.contains(secret),
                "the log line carries `{secret}`: {line}"
            );
        }
    }

    /// The quota response reader takes both shapes the API answers with and
    /// drops malformed rows instead of failing the batch.
    #[test]
    fn the_quota_reader_takes_both_shapes_and_drops_broken_rows() {
        let wrapped: serde_json::Value = serde_json::json!({ "buckets": [
            { "remainingFraction": 0.5, "resetTime": "2026-08-18T00:00:00Z", "modelId": "gemini-2.5-pro" },
            { "remainingFraction": "not-a-number", "resetTime": "x", "modelId": "y" },
        ] });
        assert_eq!(parse_quota_response(&wrapped).len(), 1);
        let bare: serde_json::Value = serde_json::json!([
            { "remainingFraction": 0.1, "resetTime": "2026-08-18T00:00:00Z", "modelId": "gemini-2.5-flash" },
        ]);
        assert_eq!(parse_quota_response(&bare).len(), 1);
        assert_eq!(parse_quota_response(&serde_json::json!({})).len(), 0);
    }
}
