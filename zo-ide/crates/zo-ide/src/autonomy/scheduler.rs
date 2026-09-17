//! Pure trigger-to-wakeup decisions plus bounded polling helpers.

use serde::{Deserialize, Serialize};

use super::limits::AutonomyLimits;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Trigger {
    Immediate,
    Every { seconds: u64 },
    Count { left: u32 },
    Watch { glob: String, last_seen: Option<u64> },
    Until { cmd: String },
    ModelWakeup { at_unix_ms: u64 },
    ProviderReset { at_unix_ms: u64 },
    Backoff { n: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wakeup {
    pub due: bool,
    pub next_at_unix_ms: Option<u64>,
}

pub struct Scheduler;

#[must_use]
pub fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}

impl Scheduler {
    #[must_use]
    pub fn evaluate(trigger: &Trigger, now_unix_ms: u64, limits: &AutonomyLimits) -> Wakeup {
        Self::evaluate_with(trigger, now_unix_ms, limits, TriggerObservation::Waiting)
    }

    #[must_use]
    pub fn evaluate_with(
        trigger: &Trigger,
        now_unix_ms: u64,
        limits: &AutonomyLimits,
        observation: TriggerObservation,
    ) -> Wakeup {
        let after = |seconds: u64| now_unix_ms.saturating_add(seconds.saturating_mul(1_000));
        match trigger {
            Trigger::Immediate | Trigger::Count { left: 1.. } => Wakeup {
                due: true,
                next_at_unix_ms: Some(now_unix_ms),
            },
            Trigger::Count { left: 0 } => Wakeup {
                due: false,
                next_at_unix_ms: None,
            },
            Trigger::Every { seconds } => Wakeup {
                due: false,
                next_at_unix_ms: Some(after((*seconds).max(limits.min_interval_secs))),
            },
            Trigger::Watch { .. } => Wakeup {
                due: observation == TriggerObservation::Changed,
                next_at_unix_ms: Some(if observation == TriggerObservation::Changed {
                    now_unix_ms
                } else {
                    after(limits.poll_interval_secs)
                }),
            },
            Trigger::Until { .. } => Wakeup {
                due: observation == TriggerObservation::Satisfied,
                next_at_unix_ms: Some(if observation == TriggerObservation::Satisfied {
                    now_unix_ms
                } else {
                    after(limits.poll_interval_secs)
                }),
            },
            Trigger::ModelWakeup { at_unix_ms } => {
                if *at_unix_ms <= now_unix_ms {
                    Wakeup {
                        due: true,
                        next_at_unix_ms: Some(now_unix_ms),
                    }
                } else {
                    let minimum = after(limits.min_model_wakeup_secs);
                    let maximum = after(limits.max_model_wakeup_secs);
                    Wakeup {
                        due: false,
                        next_at_unix_ms: Some((*at_unix_ms).clamp(minimum, maximum)),
                    }
                }
            }
            Trigger::ProviderReset { at_unix_ms } => Wakeup {
                due: *at_unix_ms <= now_unix_ms,
                next_at_unix_ms: Some((*at_unix_ms).max(now_unix_ms)),
            },
            Trigger::Backoff { n } => {
                let index = (*n).min(limits.backoff_secs.len().saturating_sub(1));
                Wakeup {
                    due: false,
                    next_at_unix_ms: Some(after(limits.backoff_secs[index])),
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerObservation {
    Waiting,
    Changed,
    Satisfied,
}

#[cfg(test)]
mod tests {
    use super::{Scheduler, Trigger, Wakeup};
    use crate::autonomy::limits::AutonomyLimits;

    #[test]
    fn immediate_and_count_are_due_now() {
        let limits = AutonomyLimits::default();
        for trigger in [Trigger::Immediate, Trigger::Count { left: 2 }] {
            assert_eq!(
                Scheduler::evaluate(&trigger, 10_000, &limits),
                Wakeup { due: true, next_at_unix_ms: Some(10_000) }
            );
        }
    }

    #[test]
    fn model_wakeup_is_clamped_from_now() {
        let limits = AutonomyLimits::default();
        assert_eq!(
            Scheduler::evaluate(&Trigger::ModelWakeup { at_unix_ms: 10_001 }, 10_000, &limits),
            Wakeup {
                due: false,
                next_at_unix_ms: Some(10_000 + limits.min_model_wakeup_secs * 1_000),
            }
        );
    }

    #[test]
    fn provider_reset_preserves_the_provider_clock() {
        let limits = AutonomyLimits::default();
        assert_eq!(
            Scheduler::evaluate(&Trigger::ProviderReset { at_unix_ms: 42_000 }, 10_000, &limits),
            Wakeup { due: false, next_at_unix_ms: Some(42_000) }
        );
    }

    #[test]
    fn backoff_uses_the_bounded_table() {
        let limits = AutonomyLimits::default();
        assert_eq!(
            Scheduler::evaluate(&Trigger::Backoff { n: usize::MAX }, 10_000, &limits),
            Wakeup {
                due: false,
                next_at_unix_ms: Some(10_000 + limits.backoff_secs[2] * 1_000),
            }
        );
    }
}
