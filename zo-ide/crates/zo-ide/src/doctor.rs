//! `zo --doctor` — a small, secret-safe environment diagnosis.
//!
//! Doctor is deliberately a read-only command. It reports credential presence
//! and expiry metadata, model alias resolution, MCP configuration loading, and
//! the hook reporter environment without opening a session, refreshing OAuth,
//! spawning an MCP process, or printing a token.

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

/// A single row in the doctor report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub label: String,
    pub status: Status,
    pub value: String,
}

/// Status attached to a diagnostic row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    Pass,
    Warn,
    Fail,
}

impl Status {
    const fn label(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Warn => "WARN",
            Self::Fail => "FAIL",
        }
    }

    const fn color(self) -> &'static str {
        match self {
            Self::Pass => "\x1b[32m",
            Self::Warn => "\x1b[33m",
            Self::Fail => "\x1b[31m",
        }
    }
}

/// The complete diagnosis, in stable display order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorReport {
    pub findings: Vec<Finding>,
}

impl DoctorReport {
    #[must_use]
    pub fn is_healthy(&self) -> bool {
        self.findings
            .iter()
            .all(|finding| finding.status == Status::Pass)
    }

    /// Render the pipe-friendly report. Labels are bold, status colors stay in
    /// the ordinary 30-series, and values never contain credential material.
    #[must_use]
    pub fn render(&self) -> String {
        self.findings
            .iter()
            .map(|finding| {
                format!(
                    "\x1b[1m{:<12}\x1b[0m {}{}\x1b[0m {}",
                    finding.label,
                    finding.status.color(),
                    finding.status.label(),
                    finding.value,
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Plain rendering is useful for logs and tests that do not want terminal
    /// control sequences; the CLI uses [`Self::render`] for the requested
    /// pipe-style output.
    #[must_use]
    pub fn render_plain(&self) -> String {
        self.findings
            .iter()
            .map(|finding| {
                format!(
                    "{:<12} {:<4} {}",
                    finding.label,
                    finding.status.label(),
                    finding.value
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Run all doctor checks for `cwd`.
#[must_use]
pub fn diagnose(cwd: &Path) -> DoctorReport {
    let (config, config_finding) = inspect_config(cwd);
    let model_finding = inspect_model(config.as_ref());
    let mcp_finding = inspect_mcp(config.as_ref(), config_finding.is_some());
    let second_brain_finding = config.as_ref().and_then(inspect_second_brain);
    DoctorReport {
        findings: vec![
            inspect_claude_auth(),
            inspect_codex_auth(),
            model_finding,
            config_finding.unwrap_or_else(|| Finding {
                label: "MCP config".to_string(),
                status: Status::Pass,
                value: "no settings files found".to_string(),
            }),
            mcp_finding,
            inspect_hook_env(),
            inspect_prompt_cache(),
            inspect_cache_anchor(),
        ]
        .into_iter()
        .chain(second_brain_finding)
        .chain(inspect_decision_shadow(cwd))
        .collect(),
    }
}

/// Where the second-brain vault is and how much of it recall can see.
///
/// Absent — not a row saying "off" — when nobody configured a vault: doctor is
/// a diagnosis of what is set up, and an every-install row for a feature almost
/// nobody has turned on is noise in a report people read top to bottom. A
/// configured path with no `wiki/` is a WARN rather than a FAIL: the session
/// still works, it just learns nothing.
fn inspect_second_brain(config: &runtime::RuntimeConfig) -> Option<Finding> {
    let status = runtime::second_brain::status(config)?;
    Some(Finding {
        label: "Second brain".to_string(),
        status: if status.ready { Status::Pass } else { Status::Warn },
        value: if status.ready {
            format!(
                "{} ({} pages{})",
                status.root,
                status.pages,
                if status.capped {
                    format!(", capped at {}", runtime::second_brain::corpus::MAX_INDEXED_PAGES)
                } else {
                    String::new()
                }
            )
        } else {
            format!("{} (no wiki/ directory — run the vault setup)", status.root)
        },
    })
}

/// The §2.3 headline, read from the ledger instead of from a shell pipeline.
///
/// The gate is a cache-read share of 96.5%; the ledger measured 90.7% before
/// the anchor fix. This row is what turns "we think it improved" into a number
/// anyone can print, and it is deliberately a WARN rather than a FAIL: a
/// machine's ledger accretes across every project and provider it ever ran,
/// so one low session should not make `--doctor` claim the install is broken.
fn inspect_prompt_cache() -> Finding {
    let summary = api::summarize_cache_ledger();
    let Some(basis_points) = summary.cache_read_basis_points() else {
        return Finding {
            label: "Prompt cache".to_string(),
            status: Status::Pass,
            value: "no tracked requests yet".to_string(),
        };
    };
    let heaviest = summary
        .by_axis
        .first()
        .map_or_else(String::new, |(axis, rows, tokens)| {
            format!(
                " · heaviest {axis} ({rows} breaks, {} tokens)",
                crate::prompt_input::thousands(*tokens)
            )
        });
    Finding {
        label: "Prompt cache".to_string(),
        status: if basis_points >= CACHE_READ_GATE_BASIS_POINTS {
            Status::Pass
        } else {
            Status::Warn
        },
        value: format!(
            "{}.{:02}% of input served from cache over {} sessions (gate {}.{:02}%){heaviest}",
            basis_points / 100,
            basis_points % 100,
            summary.sessions,
            CACHE_READ_GATE_BASIS_POINTS / 100,
            CACHE_READ_GATE_BASIS_POINTS % 100,
        ),
    }
}

/// Whether the breakpoint anchor is landing where a previous request wrote.
///
/// This is the property `mark_breakpoints_with_ttl`'s anchor+rolling shape
/// exists to hold, and until the marker positions were recorded there was no
/// way to observe it from outside a debugger — the 1,190-row defect that cost
/// 149.9M tokens was found by inference and would have had to be re-found the
/// same way to confirm the fix.
fn inspect_cache_anchor() -> Finding {
    let summary = api::summarize_cache_ledger();
    let Some(basis_points) = summary.anchor_reuse_basis_points() else {
        // "No verdict yet" is two different states and the reader has to be
        // able to tell them apart: a ledger where nothing has been RECORDED is
        // an instrument still waiting for its first row, while a ledger that
        // records markers and finds every break ineligible is an instrument
        // that is running. Printing their sum as "rows predate marker
        // recording" said the first when the truth was the second — measured
        // on the live store, 3 of 1,916 rows carried markers and were reported
        // as predating the field that is right there in them.
        return Finding {
            label: "Cache anchor".to_string(),
            status: Status::Pass,
            value: if summary.anchor_ineligible == 0 {
                format!(
                    "not observed yet ({} rows predate marker recording)",
                    summary.anchor_unknown
                )
            } else {
                format!(
                    "recording, no verdict yet ({} marked break(s), all with another cause; \
                     {} rows predate marker recording)",
                    summary.anchor_ineligible, summary.anchor_unknown
                )
            },
        };
    };
    // A handful of rows is not evidence either way, so say so rather than
    // print a percentage of three. The whole point of this row is to replace an
    // inference with a measurement; a measurement with no sample behind it is
    // just a differently-shaped inference.
    let observed = summary.anchor_reused + summary.anchor_missed;
    Finding {
        label: "Cache anchor".to_string(),
        status: if summary.anchor_missed == 0 || observed < ANCHOR_SAMPLE_FLOOR {
            Status::Pass
        } else {
            Status::Warn
        },
        value: format!(
            "{}.{:02}% of {observed} eligible breaks anchored on a written position \
             ({} reused, {} missed){}",
            basis_points / 100,
            basis_points % 100,
            summary.anchor_reused,
            summary.anchor_missed,
            if observed < ANCHOR_SAMPLE_FLOOR {
                " — too few to judge yet"
            } else {
                ""
            }
        ),
    }
}

/// The routing probe's typed twin (`smart.decisionShadow`), read from its
/// ledger — the only place it can be seen from another process, since the
/// attestation counters live inside the session that fired them.
///
/// Absent, like the second brain's row, when it was never turned on and wrote
/// nothing: most installs never will, and a report read top to bottom should
/// not carry a row per feature nobody chose.
fn inspect_decision_shadow(cwd: &Path) -> Vec<Finding> {
    let mode = tools::decision_shadow_mode_from(&runtime::ConfigLoader::default_for(cwd)).unwrap_or_default();
    let rows: Vec<tools::DecisionShadowRow> = tools::read_shadow_rows(&tools::decision_shadow_path(cwd));
    decision_shadow_findings(
        mode,
        api::SystemOneConfig::from_env().is_ok(),
        &tools::summarize_decision_shadow(&rows),
    )
}

/// The decision shadow's rows from what was read: the setting and what it
/// sends, the answers and failures, the calls' latency, how often the judgment
/// named the probe's token, and what it billed.
fn decision_shadow_findings(
    mode: tools::DecisionShadowMode,
    key_present: bool,
    summary: &tools::DecisionShadowSummary,
) -> Vec<Finding> {
    let enabled = mode.asks();
    if !enabled && summary.rows == 0 {
        return Vec::new();
    }
    let host = api::SYSTEMONE_BASE_URL
        .split_once("://")
        .map_or(api::SYSTEMONE_BASE_URL, |(_, host)| host);
    let setting = format!("smart.{} = {}", tools::DECISION_SHADOW_SETTING, mode.key());
    // `auto` records until something promotes it, so it reads as record only.
    let applied = mode.applies();
    let mut findings = vec![Finding {
        label: "Jev mode".to_string(),
        status: if enabled && !key_present { Status::Warn } else { Status::Pass },
        value: if !enabled {
            format!("off ({setting}); the ledger keeps {} earlier rows", summary.rows)
        } else if !key_present {
            format!(
                "{} ({setting}), but {} is not set: nothing is sent and routing falls back to the chat probe",
                if applied { "applied" } else { "record only" },
                api::SYSTEMONE_API_KEY_ENV,
            )
        } else if applied {
            format!(
                "applied ({setting}): validated judgments go to conservative routing fusion; failures fall back to the chat probe"
            )
        } else {
            format!(
                "record only ({setting}): each probed task's first {} characters go to {host}; routing stays unchanged",
                crate::prompt_input::thousands(u64::try_from(runtime::RUBRIC_TASK_CHAR_CAP).unwrap_or(u64::MAX)),
            )
        },
    }];
    if summary.rows > 0 {
        findings.push(decision_shadow_answers(summary));
    }
    if let (Some(p50), Some(p95)) = (summary.latency_p50_ms, summary.latency_p95_ms) {
        findings.push(Finding {
            label: "Jev latency".to_string(),
            status: Status::Pass,
            value: format!("p50 {p50} ms · p95 {p95} ms over {} calls (recalls excluded)", summary.called),
        });
    }
    if let Some(compared) = summary.agreement.first().map(|agreement| agreement.compared).filter(|n| *n > 0) {
        let shares = summary
            .agreement
            .iter()
            .map(|agreement| format!("{} {}", agreement.axis.name, percent(tools::basis_points(agreement.agreed, agreement.compared))))
            .collect::<Vec<_>>()
            .join(" · ");
        findings.push(Finding {
            label: "Jev vs probe".to_string(),
            status: Status::Pass,
            value: format!("same token: {shares} over {compared} rows where both answered"),
        });
    }
    if summary.input_tokens > 0 {
        let rate = api::systemone_rate(api::SYSTEMONE_MODEL).map_or_else(String::new, |rate| {
            format!(
                " at ${}/M input ({}, announced {:04}-{:02}-{:02})",
                rate.input,
                api::SYSTEMONE_MODEL,
                rate.announced.year,
                rate.announced.month,
                rate.announced.day
            )
        });
        let cost = summary.cost_usd.map_or_else(|| "unpriced".to_string(), |usd| format!("${usd:.4}"));
        let unpriced = if summary.unpriced_input_tokens > 0 {
            format!(" · {} tokens on models no price row names", crate::prompt_input::thousands(summary.unpriced_input_tokens))
        } else {
            String::new()
        };
        findings.push(Finding {
            label: "Jev cost".to_string(),
            status: Status::Pass,
            value: format!(
                "{} input tokens ≈ {cost}{rate}{unpriced}",
                crate::prompt_input::thousands(summary.input_tokens)
            ),
        });
    }
    findings
}

/// The judgment's answers over the rows that asked — a row the Jev door refused
/// asked nothing, so it is no answer missed and is listed apart — with the
/// failures by token. Warns when enough calls went out and none answered.
fn decision_shadow_answers(summary: &tools::DecisionShadowSummary) -> Finding {
    let listed = |counts: &[(String, usize)]| {
        counts.iter().map(|(token, count)| format!("{token} {count}")).collect::<Vec<_>>().join(", ")
    };
    let (failures, refused) = (listed(&summary.failures), listed(&summary.refused));
    let asked = summary.rows - summary.refused.iter().map(|(_, count)| count).sum::<usize>();
    Finding {
        label: "Jev answers".to_string(),
        status: if summary.called >= JUDGMENT_SAMPLE_FLOOR && summary.answered == 0 {
            Status::Warn
        } else {
            Status::Pass
        },
        value: format!(
            "{} of {} rows answered ({}){}{}",
            summary.answered,
            asked,
            percent(tools::basis_points(summary.answered, asked)),
            if failures.is_empty() { String::new() } else { format!(" · failed: {failures}") },
            if refused.is_empty() { String::new() } else { format!(" · refused: {refused}") },
        ),
    }
}

/// A share in basis points as the doctor prints one, or a dash for no
/// population.
fn percent(basis_points: Option<u64>) -> String {
    basis_points.map_or_else(
        || "—".to_string(),
        |points| format!("{}.{:02}%", points / 100, points % 100),
    )
}

/// Calls a ledger needs before "none of them answered" reads as a broken
/// endpoint rather than a slow start — the shape the routing probe had for
/// weeks while every call failed.
const JUDGMENT_SAMPLE_FLOOR: usize = 20;

/// Eligible breaks needed before the anchor share means anything. Rows whose
/// model, system prompt, or tool block changed are excluded upstream
/// (`CacheBreakLedgerRow::anchor_was_previously_written`), so this population
/// grows slowly — and one `/model` switch reading as "0.00%" is exactly the
/// misreport this floor exists to prevent.
const ANCHOR_SAMPLE_FLOOR: usize = 20;

/// `architecture-r15.md` §2.3: input served from cache, the release gate.
const CACHE_READ_GATE_BASIS_POINTS: u64 = 9_650;


/// Convenience entry point used by `main.rs`.
#[must_use]
pub fn run(cwd: &Path) -> String {
    diagnose(cwd).render()
}

fn inspect_config(cwd: &Path) -> (Option<runtime::RuntimeConfig>, Option<Finding>) {
    let loader = runtime::ConfigLoader::default_for(cwd);
    let discovered = loader
        .discover()
        .into_iter()
        .filter(|entry| entry.path.exists())
        .count();
    match loader.load() {
        Ok(config) => {
            let loaded_count = config.loaded_entries().len();
            (
                Some(config),
                Some(Finding {
                    label: "MCP config".to_string(),
                    status: Status::Pass,
                    value: format!("{loaded_count}/{discovered} settings file(s) parsed"),
                }),
            )
        }
        Err(_) => (
            None,
            Some(Finding {
                label: "MCP config".to_string(),
                status: Status::Fail,
                value: "settings file could not be parsed".to_string(),
            }),
        ),
    }
}

fn inspect_model(config: Option<&runtime::RuntimeConfig>) -> Finding {
    let raw = config
        .and_then(runtime::RuntimeConfig::model)
        .unwrap_or(crate::DEFAULT_MODEL);
    let resolved = api::resolve_model_alias(raw);
    let value = if raw == resolved {
        resolved
    } else {
        format!("{raw} -> {resolved}")
    };
    Finding {
        label: "Model".to_string(),
        status: Status::Pass,
        value,
    }
}

fn inspect_mcp(config: Option<&runtime::RuntimeConfig>, config_failed: bool) -> Finding {
    let Some(config) = config else {
        return Finding {
            label: "MCP".to_string(),
            status: if config_failed {
                Status::Warn
            } else {
                Status::Pass
            },
            value: if config_failed {
                "not checked because settings parsing failed".to_string()
            } else {
                "no configuration".to_string()
            },
        };
    };
    // Name them. A count answers "did the file parse"; a diagnosis of MCP has
    // to answer "is the server I just added there", which is the question a
    // person runs doctor with after `zo mcp add`.
    let names = config
        .mcp()
        .servers()
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let blocked = config
        .mcp()
        .untrusted_project_servers()
        .iter()
        .map(|entry| entry.name.as_str())
        .collect::<Vec<_>>();
    let mut value = format!("{} configured server(s)", names.len());
    if !names.is_empty() {
        let _ = write!(value, ": {}", names.join(", "));
    }
    if !blocked.is_empty() {
        let _ = write!(
            value,
            "; {} awaiting trust: {}",
            blocked.len(),
            blocked.join(", ")
        );
    }
    let blocked = blocked.len();
    Finding {
        label: "MCP".to_string(),
        status: if blocked == 0 {
            Status::Pass
        } else {
            Status::Warn
        },
        value,
    }
}

fn inspect_hook_env() -> Finding {
    let required = [
        crate::ide::reporter::ENV_PORT,
        crate::ide::reporter::ENV_TOKEN,
        crate::ide::reporter::ENV_PANE_KEY,
    ];
    let missing = required
        .iter()
        .filter(|name| !env_non_empty(name))
        .copied()
        .collect::<Vec<_>>();
    if missing.is_empty() {
        Finding {
            label: "Hook env".to_string(),
            status: Status::Pass,
            value: "reporter ready (3/3 required variables)".to_string(),
        }
    } else {
        Finding {
            label: "Hook env".to_string(),
            status: Status::Warn,
            value: format!("reporter inactive; missing {}", missing.join(", ")),
        }
    }
}

fn inspect_claude_auth() -> Finding {
    let managed_path = env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map(|dir| dir.join(".credentials.json"));

    if let Some(path) = managed_path.as_deref().filter(|path| path.is_file()) {
        return match read_json(path) {
            Ok(value) => credential_finding("Claude", "CLAUDE_CONFIG_DIR", &value),
            Err(_) => Finding {
                label: "Claude".to_string(),
                status: Status::Fail,
                value: "CLAUDE_CONFIG_DIR credentials file is unreadable".to_string(),
            },
        };
    }

    if env::var_os("ZO_DISABLE_KEYCHAIN").is_some() {
        return Finding {
            label: "Claude".to_string(),
            status: Status::Warn,
            value: "no managed credentials; keychain disabled".to_string(),
        };
    }

    match read_keychain_blob() {
        KeychainProbe::Present(value) => credential_finding("Claude", "keychain", &value),
        KeychainProbe::Missing => Finding {
            label: "Claude".to_string(),
            status: Status::Warn,
            value: "not logged in via CLAUDE_CONFIG_DIR or keychain".to_string(),
        },
        KeychainProbe::Unreadable => Finding {
            label: "Claude".to_string(),
            status: Status::Warn,
            value: "keychain entry exists but could not be inspected".to_string(),
        },
    }
}

fn inspect_codex_auth() -> Finding {
    let codex_home_set = env_non_empty(api::oauth_store::codex_auth::CODEX_HOME_ENV);
    let Some((path, source)) = api::oauth_store::codex_auth::auth_json_path_with_source() else {
        return Finding {
            label: "Codex".to_string(),
            status: Status::Warn,
            value: if codex_home_set {
                "CODEX_HOME/auth.json not found".to_string()
            } else {
                "no Codex login: CODEX_HOME is unset and no ZeroCode window is signed in".to_string()
            },
        };
    };
    match api::oauth_store::codex_auth::load_at(&path) {
        Ok(Some(tokens)) => expiry_finding(
            "Codex",
            codex_home_phrase(source),
            tokens.expires_at,
            tokens.refresh_token.is_some(),
        ),
        Ok(None) => Finding {
            label: "Codex".to_string(),
            status: Status::Warn,
            value: "auth.json has no logged-in access token".to_string(),
        },
        Err(_) => Finding {
            label: "Codex".to_string(),
            status: Status::Fail,
            value: "auth.json is unreadable or malformed".to_string(),
        },
    }
}

/// 이 로그인을 어디서 빌렸는지 한 줄로 — 해석 순서의 행 이름 그대로.
const fn codex_home_phrase(source: api::managed_account::CodexHomeSource) -> &'static str {
    match source {
        api::managed_account::CodexHomeSource::Channel => "the account the window switched to",
        api::managed_account::CodexHomeSource::Env => "CODEX_HOME/auth.json",
        api::managed_account::CodexHomeSource::IdeManaged => "the ZeroCode window's own codex home",
    }
}

fn credential_finding(label: &str, source: &str, root: &Value) -> Finding {
    let Some(oauth) = root.get("claudeAiOauth") else {
        return Finding {
            label: label.to_string(),
            status: Status::Warn,
            value: format!("{source} has no Claude OAuth entry"),
        };
    };
    let logged_in = oauth
        .get("accessToken")
        .and_then(Value::as_str)
        .is_some_and(|token| !token.is_empty());
    if !logged_in {
        return Finding {
            label: label.to_string(),
            status: Status::Warn,
            value: format!("{source} is not logged in"),
        };
    }
    let refreshable = oauth
        .get("refreshToken")
        .and_then(Value::as_str)
        .is_some_and(|token| !token.is_empty());
    let expires_at_ms = oauth.get("expiresAt").and_then(Value::as_u64);
    let expires_at = expires_at_ms.map(|value| value / 1_000);
    expiry_finding(label, source, expires_at, refreshable)
}

fn expiry_finding(
    label: &str,
    source: &str,
    expires_at: Option<u64>,
    refreshable: bool,
) -> Finding {
    let Some(expires_at) = expires_at else {
        return Finding {
            label: label.to_string(),
            status: Status::Pass,
            value: format!("logged in via {source}; expiry not recorded"),
        };
    };
    let now = unix_now();
    if expires_at <= now {
        let action = if refreshable {
            "refresh available"
        } else {
            "re-login required"
        };
        return Finding {
            label: label.to_string(),
            status: Status::Warn,
            value: format!("logged in via {source}; expired ({action})"),
        };
    }
    Finding {
        label: label.to_string(),
        status: Status::Pass,
        value: format!(
            "logged in via {source}; expires in {}",
            short_duration(expires_at - now)
        ),
    }
}

fn read_json(path: &Path) -> std::io::Result<Value> {
    let contents = fs::read_to_string(path)?;
    serde_json::from_str(&contents)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

enum KeychainProbe {
    Present(Value),
    Missing,
    Unreadable,
}

/// Read the keychain blob for diagnosis only. The `security` invocation uses
/// the read operation and never refreshes or writes credentials; the blob is
/// parsed in memory and no token field is included in the report.
fn read_keychain_blob() -> KeychainProbe {
    let Ok(output) = Command::new("security")
        .args(["find-generic-password", "-s", "Claude Code-credentials", "-w"])
        .output()
    else {
        return KeychainProbe::Missing;
    };
    if !output.status.success() {
        return KeychainProbe::Missing;
    }
    let Ok(raw) = String::from_utf8(output.stdout) else {
        return KeychainProbe::Unreadable;
    };
    match serde_json::from_str(raw.trim()) {
        Ok(value) => KeychainProbe::Present(value),
        Err(_) => KeychainProbe::Unreadable,
    }
}

fn env_non_empty(name: &str) -> bool {
    env::var_os(name).is_some_and(|value| !value.is_empty())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn short_duration(seconds: u64) -> String {
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3_599 => format!("{}m", (seconds + 30) / 60),
        3_600..=86_399 => format!("{}h", (seconds + 1_800) / 3_600),
        _ => format!("{}d", (seconds + 43_200) / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        credential_finding, decision_shadow_findings, expiry_finding, inspect_decision_shadow,
        inspect_second_brain, short_duration, Status, JUDGMENT_SAMPLE_FLOOR,
    };

    /// Redirect every home the config loader consults at once. `ZO_CONFIG_HOME`
    /// alone is not enough: the canonical root chain still appends `$HOME/.zo`
    /// and `$HOME/.forge`, so a test that left `HOME` alone would read the
    /// developer's own settings.
    fn with_temp_home<T>(label: &str, body: impl FnOnce(&std::path::Path) -> T) -> T {
        let _lock = crate::test_env_lock();
        let root = std::env::temp_dir().join(format!(
            "zo-ide-doctor-{label}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("zo-global")).expect("zo home");
        std::fs::create_dir_all(root.join("home")).expect("home");
        std::fs::create_dir_all(root.join("workspace")).expect("workspace");
        // Saved and restored around every body, including the ones a body sets
        // for itself (the state dir, the System One key), even on a panic.
        let keys = [
            "HOME",
            "ZO_CONFIG_HOME",
            "ZO_HOME",
            "ZEROCODE_SECOND_BRAIN",
            core_types::paths::ZO_STATE_DIR_ENV,
            api::SYSTEMONE_API_KEY_ENV,
        ];
        let previous: Vec<_> = keys
            .iter()
            .map(|key| (*key, std::env::var_os(key)))
            .collect();
        std::env::set_var("HOME", root.join("home"));
        std::env::set_var("ZO_CONFIG_HOME", root.join("zo-global"));
        std::env::set_var("ZO_HOME", root.join("absent"));
        std::env::remove_var("ZEROCODE_SECOND_BRAIN");
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| body(&root)));
        for (key, value) in previous {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
        let _ = std::fs::remove_dir_all(&root);
        match outcome {
            Ok(value) => value,
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }

    fn config_at(root: &std::path::Path) -> runtime::RuntimeConfig {
        runtime::ConfigLoader::default_for(root.join("workspace"))
            .load()
            .expect("settings should load")
    }

    #[test]
    fn no_configured_vault_leaves_the_second_brain_row_out_of_the_report() {
        with_temp_home("second-brain-absent", |root| {
            assert!(inspect_second_brain(&config_at(root)).is_none());
        });
    }

    #[test]
    fn the_second_brain_row_counts_pages_and_warns_on_a_vault_that_is_not_set_up() {
        with_temp_home("second-brain-row", |root| {
            let vault = root.join("vault");
            std::fs::create_dir_all(vault.join("wiki")).expect("wiki");
            std::fs::write(vault.join("wiki/one.md"), "one\n").expect("page");
            std::env::set_var("ZEROCODE_SECOND_BRAIN", &vault);
            let finding = inspect_second_brain(&config_at(root)).expect("a configured vault shows");
            assert_eq!(finding.label, "Second brain");
            assert_eq!(finding.status, Status::Pass);
            assert!(finding.value.contains(&vault.display().to_string()));
            assert!(finding.value.contains("1 pages"), "{}", finding.value);

            std::env::set_var("ZEROCODE_SECOND_BRAIN", root.join("never-created"));
            let finding = inspect_second_brain(&config_at(root)).expect("still configured");
            assert_eq!(finding.status, Status::Warn);
            assert!(finding.value.contains("no wiki/ directory"), "{}", finding.value);
        });
    }

    /// The shadow's rows come from its ledger file: a separate process sees
    /// what the sessions recorded.
    #[test]
    fn the_decision_shadow_rows_read_the_ledger_and_say_what_is_sent() {
        with_temp_home("decision-shadow", |root| {
            let workspace = root.join("workspace");
            std::env::set_var(core_types::paths::ZO_STATE_DIR_ENV, root.join("state"));
            std::env::set_var(api::SYSTEMONE_API_KEY_ENV, "test-key");

            // Never turned on, nothing written: no row at all.
            assert!(inspect_decision_shadow(&workspace).is_empty());

            std::fs::write(
                root.join("zo-global").join("settings.json"),
                serde_json::json!({"smart": {(tools::DECISION_SHADOW_SETTING): "shadow"}}).to_string(),
            )
            .expect("settings");
            let ledger = tools::decision_shadow_path(&workspace);
            std::fs::create_dir_all(ledger.parent().expect("a directory")).expect("ledger dir");
            let rows = [
                serde_json::json!({"at": 1, "task": "0000000000000001", "rubricVersion": 1, "model": "jev-latest",
                    "outcome": tools::OUTCOME_ANSWERED, "elapsedMs": 240, "retries": 0, "cached": false,
                    "inputTokens": 1_200,
                    "probe": {"complexity": "large", "risk": "high", "confidence": "high", "intent": "design"},
                    "jev": {
                        "complexity": {"choice": "large", "probabilities": {"trivial": 0.0, "small": 0.1, "medium": 0.2, "large": 0.7}, "confidence": 0.5},
                        "risk": {"choice": "medium", "probabilities": {"low": 0.1, "medium": 0.6, "high": 0.2, "critical": 0.1}, "confidence": 0.4},
                        "intent": {"choice": "design", "probabilities": {"design": 0.9, "implementation": 0.05, "analysis": 0.03, "other": 0.02}, "confidence": 0.8}}}),
                serde_json::json!({"at": 2, "task": "0000000000000002", "rubricVersion": 1, "outcome": "timeout",
                    "elapsedMs": 8_000, "retries": 0, "cached": false, "probe": "provider_failure"}),
            ];
            let text = rows.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n") + "\n";
            std::fs::write(&ledger, text).expect("ledger");

            let findings = inspect_decision_shadow(&workspace);
            let labels: Vec<&str> = findings.iter().map(|finding| finding.label.as_str()).collect();
            assert_eq!(labels, vec!["Jev mode", "Jev answers", "Jev latency", "Jev vs probe", "Jev cost"]);
            assert!(findings.iter().all(|finding| finding.status == Status::Pass), "{findings:?}");
            assert!(findings.iter().all(|finding| finding.label.len() <= 12), "labels fit the report's column");
            let value = |label: &str| findings.iter().find(|finding| finding.label == label).map(|finding| finding.value.clone()).unwrap_or_default();
            assert!(value("Jev mode").contains("first 2,000 characters go to api.typesafe.ai"), "{}", value("Jev mode"));
            assert!(value("Jev answers").starts_with("1 of 2 rows answered (50.00%)"), "{}", value("Jev answers"));
            assert!(value("Jev answers").contains("timeout 1"), "{}", value("Jev answers"));
            assert!(value("Jev latency").contains("p50 240 ms"), "{}", value("Jev latency"));
            assert!(value("Jev vs probe").contains("complexity 100.00%") && value("Jev vs probe").contains("risk 0.00%"), "{}", value("Jev vs probe"));
            assert!(value("Jev cost").starts_with("1,200 input tokens"), "{}", value("Jev cost"));
        });
    }

    #[test]
    fn a_shadow_without_a_key_or_with_only_failures_warns() {
        let quiet = tools::summarize_decision_shadow(&[]);
        assert!(decision_shadow_findings(tools::DecisionShadowMode::Off, true, &quiet).is_empty());
        let keyless = decision_shadow_findings(tools::DecisionShadowMode::Shadow, false, &quiet);
        assert_eq!(keyless.len(), 1);
        assert_eq!(keyless[0].status, Status::Warn);
        assert!(keyless[0].value.contains(api::SYSTEMONE_API_KEY_ENV), "{}", keyless[0].value);
        let active = decision_shadow_findings(tools::DecisionShadowMode::On, true, &quiet);
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].status, Status::Pass);
        assert!(active[0].value.contains("validated judgments"), "{}", active[0].value);
        let active_keyless = decision_shadow_findings(tools::DecisionShadowMode::On, false, &quiet);
        assert_eq!(active_keyless[0].status, Status::Warn);
        assert!(active_keyless[0].value.contains("falls back"), "{}", active_keyless[0].value);
        let automatic = decision_shadow_findings(tools::DecisionShadowMode::Auto, true, &quiet);
        assert_eq!(automatic[0].status, Status::Pass);
        assert!(
            automatic[0].value.starts_with("record only (smart.decisionShadow = auto)"),
            "auto records until something promotes it: {}",
            automatic[0].value
        );

        let failing: Vec<tools::DecisionShadowRow> = (0..JUDGMENT_SAMPLE_FLOOR)
            .map(|at| {
                serde_json::from_value(serde_json::json!({"at": at, "task": "0000000000000003", "rubricVersion": 1,
                    "outcome": "unauthorized", "elapsedMs": 90, "retries": 0, "cached": false, "probe": "timeout"}))
                .expect("a row")
            })
            .collect();
        let summary = tools::summarize_decision_shadow(&failing);
        let findings = decision_shadow_findings(tools::DecisionShadowMode::Shadow, true, &summary);
        let answers = findings.iter().find(|finding| finding.label == "Jev answers").expect("an answers row");
        assert_eq!(answers.status, Status::Warn, "every call failed: {}", answers.value);
        // One call short of the floor is a slow start, not a defect.
        let summary = tools::summarize_decision_shadow(&failing[1..]);
        let findings = decision_shadow_findings(tools::DecisionShadowMode::Shadow, true, &summary);
        assert!(findings.iter().all(|finding| finding.status == Status::Pass));
        // Switched off later, the ledger still shows what it holds.
        let findings = decision_shadow_findings(tools::DecisionShadowMode::Off, false, &summary);
        assert!(findings[0].value.starts_with("off ("), "{}", findings[0].value);
    }

    #[test]
    fn expired_credentials_report_refreshability_without_secrets() {
        let finding = credential_finding(
            "Claude",
            "managed file",
            &serde_json::json!({
                "claudeAiOauth": {
                    "accessToken": "do-not-print",
                    "refreshToken": "refresh-token-secret",
                    "expiresAt": 1
                }
            }),
        );
        assert_eq!(finding.status, Status::Warn);
        assert!(finding.value.contains("expired"));
        assert!(finding.value.contains("refresh available"));
        assert!(!finding.value.contains("do-not-print"));
        assert!(!finding.value.contains("refresh-token-secret"));
    }

    #[test]
    fn duration_labels_are_compact() {
        assert_eq!(short_duration(59), "59s");
        assert_eq!(short_duration(90), "2m");
        assert_eq!(short_duration(3_600), "1h");
        assert_eq!(short_duration(86_400), "1d");
    }

    #[test]
    fn unknown_expiry_is_not_called_expired() {
        let finding = expiry_finding("Codex", "auth.json", None, false);
        assert_eq!(finding.status, Status::Pass);
        assert!(finding.value.contains("expiry not recorded"));
    }
}
