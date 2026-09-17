//! The single table of finite autonomy budgets, polling intervals, and clamps.

use serde_json::Value;

pub const DEFAULT_MAX_CONTINUATIONS: u32 = 8;
pub const DEFAULT_MAX_ASSISTANT_TURNS: u32 = 12;
pub const DEFAULT_MAX_OUTPUT_TOKENS: u64 = 32_000;
pub const DEFAULT_MAX_WALL_CLOCK_SECS: u64 = 30 * 60;
pub const DEFAULT_MAX_LOOP_RUNS: u32 = 50;
pub const DEFAULT_MODEL_WAKEUP_SECS: u64 = 20 * 60;
pub const MIN_MODEL_WAKEUP_SECS: u64 = 60;
pub const MAX_MODEL_WAKEUP_SECS: u64 = 60 * 60;
pub const DEFAULT_MAX_CONSECUTIVE_NOOPS: u32 = 24;
pub const DEFAULT_POLL_INTERVAL_SECS: u64 = 2;
pub const DEFAULT_MIN_INTERVAL_SECS: u64 = 1;
pub const DEFAULT_MAX_INTERVAL_SECS: u64 = 24 * 60 * 60;
pub const DEFAULT_BACKOFF_SECS: [u64; 3] = [30, 2 * 60, 8 * 60];
pub const DEFAULT_MAX_PLAN_ITEMS: u32 = 7;
pub const DEFAULT_MAX_STALLED_TURNS: u32 = 2;
pub const DEFAULT_MAX_FAILURE_OUTPUT_CHARS: u64 = 12_000;
pub const DEFAULT_STREAM_PHASE_AFTER_SECS: u64 = 15;
pub const DEFAULT_QUIET_AFTER_SECS: u64 = 60;

/// Process exit vocabulary for a bounded headless loop.
pub const HEADLESS_LOOP_EXIT_DONE: u8 = 0;
pub const HEADLESS_LOOP_EXIT_LIMIT: u8 = 2;
pub const HEADLESS_LOOP_EXIT_INTERRUPTED: u8 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutonomyLimits {
    pub max_continuations: u32,
    pub max_assistant_turns: u32,
    pub max_output_tokens: u64,
    pub max_wall_clock_secs: u64,
    pub max_loop_runs: u32,
    pub default_model_wakeup_secs: u64,
    pub min_model_wakeup_secs: u64,
    pub max_model_wakeup_secs: u64,
    pub max_consecutive_noops: u32,
    pub poll_interval_secs: u64,
    pub min_interval_secs: u64,
    pub max_interval_secs: u64,
    pub backoff_secs: [u64; 3],
    pub max_plan_items: u32,
    pub max_stalled_turns: u32,
    pub max_failure_output_chars: u64,
    pub stream_phase_after_secs: u64,
    pub quiet_after_secs: u64,
}

impl Default for AutonomyLimits {
    fn default() -> Self {
        let mut resolved = Self {
            max_continuations: DEFAULT_MAX_CONTINUATIONS,
            max_assistant_turns: DEFAULT_MAX_ASSISTANT_TURNS,
            max_output_tokens: DEFAULT_MAX_OUTPUT_TOKENS,
            max_wall_clock_secs: DEFAULT_MAX_WALL_CLOCK_SECS,
            max_loop_runs: DEFAULT_MAX_LOOP_RUNS,
            default_model_wakeup_secs: DEFAULT_MODEL_WAKEUP_SECS,
            min_model_wakeup_secs: MIN_MODEL_WAKEUP_SECS,
            max_model_wakeup_secs: MAX_MODEL_WAKEUP_SECS,
            max_consecutive_noops: DEFAULT_MAX_CONSECUTIVE_NOOPS,
            poll_interval_secs: DEFAULT_POLL_INTERVAL_SECS,
            min_interval_secs: DEFAULT_MIN_INTERVAL_SECS,
            max_interval_secs: DEFAULT_MAX_INTERVAL_SECS,
            backoff_secs: DEFAULT_BACKOFF_SECS,
            max_plan_items: DEFAULT_MAX_PLAN_ITEMS,
            max_stalled_turns: DEFAULT_MAX_STALLED_TURNS,
            max_failure_output_chars: DEFAULT_MAX_FAILURE_OUTPUT_CHARS,
            stream_phase_after_secs: DEFAULT_STREAM_PHASE_AFTER_SECS,
            quiet_after_secs: DEFAULT_QUIET_AFTER_SECS,
        };
        resolved.max_model_wakeup_secs = resolved
            .max_model_wakeup_secs
            .max(resolved.min_model_wakeup_secs);
        resolved.default_model_wakeup_secs = resolved.default_model_wakeup_secs.clamp(
            resolved.min_model_wakeup_secs,
            resolved.max_model_wakeup_secs,
        );
        resolved.max_interval_secs = resolved.max_interval_secs.max(resolved.min_interval_secs);
        resolved
    }
}

impl AutonomyLimits {
    #[must_use]
    #[expect(
        clippy::too_many_lines,
        reason = "the single limits table intentionally resolves every autonomy setting in one auditable place"
    )]
    pub fn from_sources(
        settings: Option<&Value>,
        env: impl Fn(&str) -> Option<String>,
    ) -> Self {
        let defaults = Self::default();
        let setting = |key: &str| {
            settings
                .and_then(|root| root.get("autonomy"))
                .and_then(|autonomy| autonomy.get(key))
                .and_then(Value::as_u64)
                .filter(|value| *value > 0)
        };
        let number = |key: &str, env_key: &str, default: u64| {
            env(env_key)
                .and_then(|raw| raw.trim().parse::<u64>().ok())
                .filter(|value| *value > 0)
                .or_else(|| setting(key))
                .unwrap_or(default)
        };
        let as_u32 = |value: u64, default: u32| u32::try_from(value).unwrap_or(default);
        let backoff = settings
            .and_then(|root| root.get("autonomy"))
            .and_then(|autonomy| autonomy.get("backoffSecs"))
            .and_then(Value::as_array);
        let backoff_at = |index: usize, env_key: &str| {
            env(env_key)
                .and_then(|raw| raw.trim().parse::<u64>().ok())
                .filter(|value| *value > 0)
                .or_else(|| {
                    backoff
                        .and_then(|values| values.get(index))
                        .and_then(Value::as_u64)
                        .filter(|value| *value > 0)
                })
                .unwrap_or(defaults.backoff_secs[index])
        };

        let mut resolved = Self {
            max_continuations: as_u32(
                number(
                    "maxContinuations",
                    "ZO_AUTONOMY_MAX_CONTINUATIONS",
                    u64::from(defaults.max_continuations),
                ),
                defaults.max_continuations,
            ),
            max_assistant_turns: as_u32(
                number(
                    "maxAssistantTurns",
                    "ZO_AUTONOMY_MAX_ASSISTANT_TURNS",
                    u64::from(defaults.max_assistant_turns),
                ),
                defaults.max_assistant_turns,
            ),
            max_output_tokens: number(
                "maxOutputTokens",
                "ZO_AUTONOMY_MAX_OUTPUT_TOKENS",
                defaults.max_output_tokens,
            ),
            max_wall_clock_secs: number(
                "maxWallClockSecs",
                "ZO_AUTONOMY_MAX_WALL_CLOCK_SECS",
                defaults.max_wall_clock_secs,
            ),
            max_loop_runs: as_u32(
                number(
                    "maxLoopRuns",
                    "ZO_AUTONOMY_MAX_LOOP_RUNS",
                    u64::from(defaults.max_loop_runs),
                ),
                defaults.max_loop_runs,
            ),
            default_model_wakeup_secs: number(
                "defaultModelWakeupSecs",
                "ZO_AUTONOMY_DEFAULT_MODEL_WAKEUP_SECS",
                defaults.default_model_wakeup_secs,
            ),
            min_model_wakeup_secs: number(
                "minModelWakeupSecs",
                "ZO_AUTONOMY_MIN_MODEL_WAKEUP_SECS",
                defaults.min_model_wakeup_secs,
            ),
            max_model_wakeup_secs: number(
                "maxModelWakeupSecs",
                "ZO_AUTONOMY_MAX_MODEL_WAKEUP_SECS",
                defaults.max_model_wakeup_secs,
            ),
            max_consecutive_noops: as_u32(
                number(
                    "maxConsecutiveNoops",
                    "ZO_AUTONOMY_MAX_CONSECUTIVE_NOOPS",
                    u64::from(defaults.max_consecutive_noops),
                ),
                defaults.max_consecutive_noops,
            ),
            poll_interval_secs: number(
                "pollIntervalSecs",
                "ZO_AUTONOMY_POLL_INTERVAL_SECS",
                defaults.poll_interval_secs,
            ),
            min_interval_secs: number(
                "minIntervalSecs",
                "ZO_AUTONOMY_MIN_INTERVAL_SECS",
                defaults.min_interval_secs,
            ),
            max_interval_secs: number(
                "maxIntervalSecs",
                "ZO_AUTONOMY_MAX_INTERVAL_SECS",
                defaults.max_interval_secs,
            ),
            backoff_secs: [
                backoff_at(0, "ZO_AUTONOMY_BACKOFF_1_SECS"),
                backoff_at(1, "ZO_AUTONOMY_BACKOFF_2_SECS"),
                backoff_at(2, "ZO_AUTONOMY_BACKOFF_3_SECS"),
            ],
            max_plan_items: as_u32(
                number(
                    "maxPlanItems",
                    "ZO_AUTONOMY_MAX_PLAN_ITEMS",
                    u64::from(defaults.max_plan_items),
                ),
                defaults.max_plan_items,
            ),
            max_stalled_turns: as_u32(
                number(
                    "maxStalledTurns",
                    "ZO_AUTONOMY_MAX_STALLED_TURNS",
                    u64::from(defaults.max_stalled_turns),
                ),
                defaults.max_stalled_turns,
            ),
            max_failure_output_chars: number(
                "maxFailureOutputChars",
                "ZO_AUTONOMY_MAX_FAILURE_OUTPUT_CHARS",
                defaults.max_failure_output_chars,
            ),
            stream_phase_after_secs: number(
                "streamPhaseAfterSecs",
                "ZO_AUTONOMY_STREAM_PHASE_AFTER_SECS",
                defaults.stream_phase_after_secs,
            ),
            quiet_after_secs: number(
                "quietAfterSecs",
                "ZO_AUTONOMY_QUIET_AFTER_SECS",
                defaults.quiet_after_secs,
            ),
        };
        resolved.max_model_wakeup_secs = resolved
            .max_model_wakeup_secs
            .max(resolved.min_model_wakeup_secs);
        resolved.default_model_wakeup_secs = resolved.default_model_wakeup_secs.clamp(
            resolved.min_model_wakeup_secs,
            resolved.max_model_wakeup_secs,
        );
        resolved.max_interval_secs = resolved.max_interval_secs.max(resolved.min_interval_secs);
        resolved
    }

    #[must_use]
    /// The table as this machine configures it: `autonomy.*` from zo's own
    /// `settings.json` (the file every other zo preference lives in), under
    /// `ZO_AUTONOMY_*` environment overrides.
    pub fn load() -> Self {
        let settings = std::fs::read(crate::preferences::preferences_path())
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
        Self::from_sources(settings.as_ref(), |name| std::env::var(name).ok())
    }
}

#[cfg(test)]
mod tests {
    use super::AutonomyLimits;

    #[test]
    fn environment_overrides_settings_and_defaults() {
        let settings = serde_json::json!({
            "autonomy": {
                "maxContinuations": 5,
                "pollIntervalSecs": 9,
                "streamPhaseAfterSecs": 17,
                "quietAfterSecs": 70
            }
        });
        let limits = AutonomyLimits::from_sources(Some(&settings), |name| {
            match name {
                "ZO_AUTONOMY_MAX_CONTINUATIONS" => Some("3".to_string()),
                "ZO_AUTONOMY_QUIET_AFTER_SECS" => Some("65".to_string()),
                _ => None,
            }
        });

        assert_eq!(limits.max_continuations, 3);
        assert_eq!(limits.poll_interval_secs, 9);
        assert_eq!(limits.stream_phase_after_secs, 17);
        assert_eq!(limits.quiet_after_secs, 65);
    }

    #[test]
    fn invalid_or_zero_overrides_keep_finite_defaults() {
        let settings = serde_json::json!({"autonomy": {"maxLoopRuns": 0}});
        let limits = AutonomyLimits::from_sources(Some(&settings), |name| {
            (name == "ZO_AUTONOMY_MAX_OUTPUT_TOKENS").then(|| "nope".to_string())
        });

        assert!(limits.max_loop_runs > 0);
        assert!(limits.max_output_tokens > 0);
    }
}
