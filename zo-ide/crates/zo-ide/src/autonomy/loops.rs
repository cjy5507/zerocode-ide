//! Persistent session loops and their bounded iteration/gate state machine.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use commands::LoopCommand;
use serde::{Deserialize, Serialize};
use tools::wakeup_store::WakeupRecord;

use super::limits::AutonomyLimits;
use super::runner::{BlockedReason, TurnReceipt};
use super::scheduler::Trigger;
use super::wakeup::LoopScheduleRequest;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopPhase {
    Running,
    Paused,
    Completed,
    Stopped,
}

impl LoopPhase {
    const fn label(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Completed => "completed",
            Self::Stopped => "stopped",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopTurnKind {
    Iteration,
    Gate,
    Repair,
    RepairedGate,
}

impl LoopTurnKind {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Iteration => "loop",
            Self::Gate | Self::RepairedGate => "loop gate",
            Self::Repair => "loop repair",
        }
    }
}

/// Who armed a loop. The two share every state transition; the source decides
/// the label and what a silent iteration means (see `finish_iteration`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopSource {
    /// `/loop …`, or the headless `--loop-*` flags.
    #[default]
    Command,
    /// The model's `ScheduleWakeup` tool — the persistent, cross-turn face of
    /// `loop_schedule`. A session keeps one such loop; every schedule record
    /// re-arms it and a stop record ends it.
    Wakeup,
}

/// The label a wakeup loop carries where a command loop carries its trigger.
pub const WAKEUP_LABEL: &str = "wakeup";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopTurn {
    pub id: String,
    pub kind: LoopTurnKind,
    pub prompt: String,
    pub allow_writes: bool,
    pub dynamic: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopBudget {
    pub max_runs: u32,
    pub token_budget: u64,
    pub runs: u32,
    pub output_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopSpec {
    pub id: String,
    pub trigger: Trigger,
    pub prompt: String,
    pub check: Option<String>,
    pub budget: LoopBudget,
    pub phase: LoopPhase,
    pub next_at_unix_ms: Option<u64>,
    pub quiet_streak: u32,
    pub successes: u32,
    pub failures: u32,
    pub pause_reason: Option<String>,
    pending: Option<LoopTurnKind>,
    in_flight: bool,
    #[serde(default)]
    allow_writes: bool,
    #[serde(default)]
    pending_schedule: Option<LoopScheduleRequest>,
    #[serde(default)]
    until_satisfied: bool,
    #[serde(default)]
    last_receipt: Option<String>,
    #[serde(default)]
    source: LoopSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct LoopLedger {
    next_id: u64,
    loops: Vec<LoopSpec>,
}

impl Default for LoopLedger {
    fn default() -> Self {
        Self {
            next_id: 1,
            loops: Vec::new(),
        }
    }
}

pub struct LoopRegistry {
    path: PathBuf,
    limits: AutonomyLimits,
    ledger: LoopLedger,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadlessLoopState {
    Running,
    Done,
    Limit,
    Interrupted,
}

impl HeadlessLoopState {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Running => "continue",
            Self::Done => "done",
            Self::Limit => "limit",
            Self::Interrupted => "interrupted",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopIterationStatus {
    pub iteration: u32,
    pub of: u32,
    pub trigger: String,
    pub outcome: HeadlessLoopState,
}

impl LoopRegistry {
    #[must_use]
    pub fn load(session_path: &Path, resumed: bool, limits: AutonomyLimits) -> Self {
        let path = session_path.with_extension("loops.json");
        let mut ledger = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<LoopLedger>(&bytes).ok())
            .unwrap_or_default();
        if resumed {
            for spec in ledger
                .loops
                .iter_mut()
                .filter(|spec| spec.phase == LoopPhase::Running)
            {
                spec.phase = LoopPhase::Paused;
                spec.in_flight = false;
                spec.pause_reason = Some(
                    "session resumed; use /loop resume to continue autonomously".to_string(),
                );
            }
        }
        let registry = Self {
            path,
            limits,
            ledger,
        };
        if resumed {
            let _ = registry.persist();
        }
        registry
    }

    #[expect(
        clippy::too_many_lines,
        reason = "the slash-command state transition table keeps every loop command exhaustive in one match"
    )]
    pub fn apply(
        &mut self,
        command: LoopCommand,
        now_unix_ms: u64,
        cwd: &Path,
    ) -> Result<String, String> {
        let notice = match command {
            LoopCommand::List | LoopCommand::Status { id: None } => self.status_report(None),
            LoopCommand::Status { id: Some(id) } => self.status_report(Some(&id)),
            LoopCommand::StartFixedCount { count, prompt } => {
                if count > self.limits.max_loop_runs {
                    return Err(format!(
                        "loop count {count} exceeds autonomy.maxLoopRuns {}",
                        self.limits.max_loop_runs
                    ));
                }
                let (options, prompt) = parse_options(&prompt, &self.limits)?;
                self.start(
                    Trigger::Count { left: count },
                    prompt,
                    options,
                    Some(now_unix_ms),
                    LoopSource::Command,
                )?
            }
            LoopCommand::StartDynamic { prompt } => {
                let (options, prompt) = parse_options(&prompt, &self.limits)?;
                self.start(Trigger::Immediate, prompt, options, Some(now_unix_ms), LoopSource::Command)?
            }
            LoopCommand::StartInterval { every, prompt } => {
                let (options, prompt) = parse_options(&prompt, &self.limits)?;
                self.start(
                    Trigger::Every {
                        seconds: every.duration.as_secs().clamp(
                            self.limits.min_interval_secs,
                            self.limits.max_interval_secs,
                        ),
                    },
                    prompt,
                    options,
                    Some(now_unix_ms),
                    LoopSource::Command,
                )?
            }
            LoopCommand::StartWatch { glob, prompt } => {
                let (options, prompt) = parse_options(&prompt, &self.limits)?;
                let seen = watch_fingerprint(cwd, &glob);
                self.start(
                    Trigger::Watch {
                        glob,
                        last_seen: seen,
                    },
                    prompt,
                    options,
                    Some(now_unix_ms.saturating_add(
                        self.limits.poll_interval_secs.saturating_mul(1_000),
                    )),
                    LoopSource::Command,
                )?
            }
            LoopCommand::StartUntil { check, prompt } => {
                let (options, prompt) = parse_options(&prompt, &self.limits)?;
                self.start(
                    Trigger::Until { cmd: check },
                    prompt,
                    options,
                    Some(now_unix_ms),
                    LoopSource::Command,
                )?
            }
            LoopCommand::RunNow { id } => {
                let spec = self.selected_mut(id.as_deref())?;
                if matches!(spec.phase, LoopPhase::Completed | LoopPhase::Stopped) {
                    return Err(format!("{} is {}", spec.id, spec.phase.label()));
                }
                spec.phase = LoopPhase::Running;
                spec.pending = Some(LoopTurnKind::Iteration);
                spec.next_at_unix_ms = Some(now_unix_ms);
                spec.pause_reason = None;
                format!("loop: {} queued now", spec.id)
            }
            LoopCommand::Pause { id } => {
                let spec = self.selected_mut(id.as_deref())?;
                spec.phase = LoopPhase::Paused;
                spec.next_at_unix_ms = None;
                spec.pause_reason = Some("paused explicitly".to_string());
                format!("loop: {} paused", spec.id)
            }
            LoopCommand::Resume { id } => {
                let spec = self.selected_mut(id.as_deref())?;
                if matches!(spec.phase, LoopPhase::Completed | LoopPhase::Stopped) {
                    return Err(format!("{} is {}", spec.id, spec.phase.label()));
                }
                spec.phase = LoopPhase::Running;
                spec.pending.get_or_insert(LoopTurnKind::Iteration);
                spec.next_at_unix_ms = Some(now_unix_ms);
                spec.pause_reason = None;
                format!("loop: {} resumed", spec.id)
            }
            LoopCommand::Stop { id, all } => {
                if all {
                    let mut stopped = 0_u32;
                    for spec in &mut self.ledger.loops {
                        if matches!(spec.phase, LoopPhase::Running | LoopPhase::Paused) {
                            spec.phase = LoopPhase::Stopped;
                            spec.next_at_unix_ms = None;
                            spec.in_flight = false;
                            stopped = stopped.saturating_add(1);
                        }
                    }
                    format!("loop: stopped {stopped}")
                } else {
                    let spec = self.selected_mut(id.as_deref())?;
                    spec.phase = LoopPhase::Stopped;
                    spec.next_at_unix_ms = None;
                    spec.in_flight = false;
                    format!("loop: {} stopped", spec.id)
                }
            }
            LoopCommand::Clear => {
                self.ledger
                    .loops
                    .retain(|spec| matches!(spec.phase, LoopPhase::Running | LoopPhase::Paused));
                "loop: cleared finished entries".to_string()
            }
        };
        self.persist().map_err(|error| error.to_string())?;
        Ok(notice)
    }

    /// Seed the same persistent [`LoopSpec`] used by `/loop`, but from the
    /// bounded headless CLI. The caller supplies the first stdin prompt; all
    /// later dispatch, receipt, permission and budget paths remain shared.
    pub fn start_headless(
        &mut self,
        trigger: Trigger,
        prompt: String,
        max_runs: Option<u32>,
        now_unix_ms: u64,
    ) -> Result<String, String> {
        let max_runs = max_runs.unwrap_or(self.limits.max_loop_runs);
        if max_runs > self.limits.max_loop_runs {
            return Err(format!(
                "loop max {max_runs} exceeds autonomy.maxLoopRuns {}",
                self.limits.max_loop_runs
            ));
        }
        let trigger = match trigger {
            Trigger::Every { seconds } => Trigger::Every {
                seconds: seconds.clamp(
                    self.limits.min_interval_secs,
                    self.limits.max_interval_secs,
                ),
            },
            other => other,
        };
        let id = format!("loop-{}", self.ledger.next_id);
        self.start(
            trigger,
            prompt,
            ParsedOptions {
                check: None,
                max_runs,
                token_budget: self.limits.max_output_tokens,
                allow_writes: false,
            },
            Some(now_unix_ms),
            LoopSource::Command,
        )?;
        self.persist().map_err(|error| error.to_string())?;
        Ok(id)
    }

    pub fn dispatch_due(
        &mut self,
        now_unix_ms: u64,
        cwd: &Path,
    ) -> Result<Option<LoopTurn>, String> {
        let poll_interval_secs = self.limits.poll_interval_secs;
        let Some(spec) = self.ledger.loops.iter_mut().find(|spec| {
            spec.phase == LoopPhase::Running
                && !spec.in_flight
                && spec.next_at_unix_ms.is_some_and(|at| at <= now_unix_ms)
        }) else {
            return Ok(None);
        };
        if let Trigger::Watch { glob, last_seen } = &mut spec.trigger {
            let seen = watch_fingerprint(cwd, glob);
            if seen == *last_seen {
                spec.next_at_unix_ms = Some(
                    now_unix_ms.saturating_add(poll_interval_secs.saturating_mul(1_000)),
                );
                self.persist().map_err(|error| error.to_string())?;
                return Ok(None);
            }
            *last_seen = seen;
            spec.pending = Some(LoopTurnKind::Iteration);
        }
        if let Trigger::Until { cmd } = &spec.trigger {
            if !matches!(
                runtime::bash_validation::validate_command(
                    cmd,
                    runtime::PermissionMode::ReadOnly,
                    cwd,
                ),
                runtime::bash_validation::ValidationResult::Allow
            ) {
                spec.phase = LoopPhase::Paused;
                spec.next_at_unix_ms = None;
                spec.pause_reason = Some(
                    "until condition is not provably read-only; polling stopped".to_string(),
                );
                self.persist().map_err(|error| error.to_string())?;
                return Ok(None);
            }
            spec.until_satisfied = std::process::Command::new("sh")
                .arg("-lc")
                .arg(cmd)
                .current_dir(cwd)
                .status()
                .is_ok_and(|status| status.success());
            spec.pending = Some(LoopTurnKind::Iteration);
        }
        let kind = spec.pending.unwrap_or(LoopTurnKind::Iteration);
        if kind == LoopTurnKind::Iteration && spec.budget.runs >= spec.budget.max_runs {
            spec.phase = LoopPhase::Completed;
            spec.next_at_unix_ms = None;
            self.persist().map_err(|error| error.to_string())?;
            return Ok(None);
        }
        spec.in_flight = true;
        let turn = LoopTurn {
            id: spec.id.clone(),
            kind,
            prompt: prompt_for(spec, kind),
            allow_writes: spec.allow_writes,
            dynamic: matches!(spec.trigger, Trigger::Immediate | Trigger::ModelWakeup { .. }),
        };
        self.persist().map_err(|error| error.to_string())?;
        Ok(Some(turn))
    }

    pub fn record_turn(
        &mut self,
        turn: &LoopTurn,
        receipt: TurnReceipt,
        now_unix_ms: u64,
    ) -> Result<String, String> {
        let limits = self.limits.clone();
        let Some(spec) = self.ledger.loops.iter_mut().find(|spec| spec.id == turn.id) else {
            return Ok(format!("loop: {} result ignored because it was cleared", turn.id));
        };
        spec.in_flight = false;
        spec.budget.output_tokens = spec
            .budget
            .output_tokens
            .saturating_add(receipt.output_tokens);
        spec.last_receipt = Some(receipt.summary());
        if let Some(blocked) = receipt.blocked {
            spec.phase = LoopPhase::Paused;
            spec.next_at_unix_ms = None;
            spec.pause_reason = Some(
                match blocked {
                    BlockedReason::Permission => "approval required during an unattended turn",
                    BlockedReason::Question => "user input required during an unattended turn",
                    BlockedReason::Interrupted => "autonomous turn interrupted",
                    BlockedReason::Failed(ref error) => error,
                }
                .to_string(),
            );
        } else if spec.budget.output_tokens >= spec.budget.token_budget {
            spec.phase = LoopPhase::Paused;
            spec.next_at_unix_ms = None;
            spec.pause_reason = Some(format!(
                "output-token limit reached ({}/{})",
                spec.budget.output_tokens, spec.budget.token_budget
            ));
        } else {
            match turn.kind {
                LoopTurnKind::Iteration => {
                    spec.budget.runs = spec.budget.runs.saturating_add(1);
                    spec.pending_schedule = receipt.loop_schedule;
                    if spec.check.is_some() {
                        spec.pending = Some(LoopTurnKind::Gate);
                        spec.next_at_unix_ms = Some(now_unix_ms);
                    } else {
                        finish_iteration(spec, now_unix_ms, &limits);
                    }
                }
                LoopTurnKind::Gate | LoopTurnKind::RepairedGate => {
                    if receipt.gate.is_some_and(|gate| gate.passed) {
                        spec.successes = spec.successes.saturating_add(1);
                        finish_iteration(spec, now_unix_ms, &limits);
                    } else if turn.kind == LoopTurnKind::Gate {
                        spec.pending = Some(LoopTurnKind::Repair);
                        spec.next_at_unix_ms = Some(now_unix_ms);
                    } else {
                        spec.failures = spec.failures.saturating_add(1);
                        finish_iteration(spec, now_unix_ms, &limits);
                    }
                }
                LoopTurnKind::Repair => {
                    spec.pending = Some(LoopTurnKind::RepairedGate);
                    spec.next_at_unix_ms = Some(now_unix_ms);
                }
            }
        }
        let notice = format_spec(spec);
        self.persist().map_err(|error| error.to_string())?;
        Ok(notice)
    }

    #[must_use]
    pub fn expected_gate<'a>(&'a self, turn: &'a LoopTurn) -> Option<&'a str> {
        matches!(turn.kind, LoopTurnKind::Gate | LoopTurnKind::RepairedGate)
            .then(|| {
                self.ledger
                    .loops
                    .iter()
                    .find(|spec| spec.id == turn.id)
                    .and_then(|spec| spec.check.as_deref())
            })
            .flatten()
    }

    #[must_use]
    pub fn next_wakeup(&self) -> Option<u64> {
        self.ledger
            .loops
            .iter()
            .filter(|spec| spec.phase == LoopPhase::Running && !spec.in_flight)
            .filter_map(|spec| spec.next_at_unix_ms)
            .min()
    }

    #[must_use]
    pub fn phase(&self, id: &str) -> Option<LoopPhase> {
        self.ledger
            .loops
            .iter()
            .find(|spec| spec.id == id)
            .map(|spec| spec.phase)
    }

    #[must_use]
    pub fn headless_state(&self, id: &str) -> Option<HeadlessLoopState> {
        let spec = self.ledger.loops.iter().find(|spec| spec.id == id)?;
        Some(match spec.phase {
            LoopPhase::Running => HeadlessLoopState::Running,
            LoopPhase::Completed => {
                if matches!(spec.trigger, Trigger::Until { .. }) && !spec.until_satisfied {
                    HeadlessLoopState::Limit
                } else {
                    HeadlessLoopState::Done
                }
            }
            LoopPhase::Paused
                if spec.pause_reason.as_deref() == Some("autonomous turn interrupted") =>
            {
                HeadlessLoopState::Interrupted
            }
            LoopPhase::Paused => HeadlessLoopState::Limit,
            LoopPhase::Stopped => HeadlessLoopState::Interrupted,
        })
    }

    #[must_use]
    pub fn iteration_status(&self, id: &str) -> Option<LoopIterationStatus> {
        let spec = self.ledger.loops.iter().find(|spec| spec.id == id)?;
        Some(LoopIterationStatus {
            iteration: spec.budget.runs,
            of: spec.budget.max_runs,
            trigger: spec_label(spec),
            outcome: self.headless_state(id)?,
        })
    }

    #[must_use]
    pub fn quiet_streak(&self, id: &str) -> Option<u32> {
        self.ledger
            .loops
            .iter()
            .find(|spec| spec.id == id)
            .map(|spec| spec.quiet_streak)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ledger.loops.is_empty()
    }

    #[must_use]
    pub fn status_report(&self, id: Option<&str>) -> String {
        let matching = self
            .ledger
            .loops
            .iter()
            .filter(|spec| id.is_none_or(|id| spec.id == id))
            .map(format_spec)
            .collect::<Vec<_>>();
        if matching.is_empty() {
            "loop: none".to_string()
        } else {
            matching.join("\n")
        }
    }

    #[must_use]
    pub fn autonomy_status(&self) -> Vec<crate::autonomy::LoopStatus> {
        self.ledger
            .loops
            .iter()
            .map(|spec| crate::autonomy::LoopStatus {
                id: spec.id.clone(),
                phase: spec.phase.label().to_string(),
                trigger: spec_label(spec),
                next_at: spec.next_at_unix_ms,
                runs: spec.budget.runs,
                max_runs: spec.budget.max_runs,
                quiet: spec.quiet_streak,
                reason: spec.pause_reason.clone(),
            })
            .collect()
    }

    #[must_use]
    pub fn footer_text(&self) -> Option<String> {
        self.ledger
            .loops
            .iter()
            .find(|spec| matches!(spec.phase, LoopPhase::Running | LoopPhase::Paused))
            .map(|spec| {
                let next = spec
                    .next_at_unix_ms
                    .map(|at| {
                        let now = super::scheduler::now_unix_ms();
                        let clock = crate::status_format::format_reset(at / 1_000, now / 1_000)
                            .replacen("resets ", "", 1);
                        format!(" · next {clock}")
                    })
                    .unwrap_or_default();
                let quiet = if spec.quiet_streak > 0 {
                    format!(" · quiet ×{}", spec.quiet_streak)
                } else {
                    String::new()
                };
                format!("loop: {}{next}{quiet}", spec_label(spec))
            })
    }

    pub fn pause_for_session_limit(&mut self, reason: &str) -> Result<(), String> {
        let mut changed = false;
        for spec in self
            .ledger
            .loops
            .iter_mut()
            .filter(|spec| spec.phase == LoopPhase::Running)
        {
            spec.phase = LoopPhase::Paused;
            spec.next_at_unix_ms = None;
            spec.in_flight = false;
            spec.pause_reason = Some(reason.to_string());
            changed = true;
        }
        if changed {
            self.persist().map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    fn start(
        &mut self,
        trigger: Trigger,
        prompt: String,
        options: ParsedOptions,
        next_at_unix_ms: Option<u64>,
        source: LoopSource,
    ) -> Result<String, String> {
        if prompt.trim().is_empty() {
            return Err("loop prompt cannot be empty".to_string());
        }
        let id = format!("loop-{}", self.ledger.next_id);
        self.ledger.next_id = self.ledger.next_id.saturating_add(1);
        self.ledger.loops.push(LoopSpec {
            id: id.clone(),
            trigger,
            prompt,
            check: options.check.map(|check| crate::goal::gate_command(&check)),
            budget: LoopBudget {
                max_runs: options.max_runs,
                token_budget: options.token_budget,
                runs: 0,
                output_tokens: 0,
            },
            phase: LoopPhase::Running,
            next_at_unix_ms,
            quiet_streak: 0,
            successes: 0,
            failures: 0,
            pause_reason: None,
            pending: Some(LoopTurnKind::Iteration),
            in_flight: false,
            allow_writes: options.allow_writes,
            pending_schedule: None,
            until_satisfied: false,
            last_receipt: None,
            source,
        });
        Ok(format!("loop: {id} started"))
    }

    /// Fold the `ScheduleWakeup` records this session's tools wrote into the
    /// registry, in the order they were written. A schedule record arms the
    /// session's one wakeup loop — or re-arms it with the new prompt and
    /// delay — and a stop record ends it. Returns one notice per record.
    pub fn adopt_wakeups(
        &mut self,
        records: &[WakeupRecord],
        now_unix_ms: u64,
    ) -> Result<Vec<String>, String> {
        let mut notices = Vec::with_capacity(records.len());
        for record in records {
            notices.push(if record.stop {
                self.stop_wakeup_loop(record)
            } else {
                self.arm_wakeup_loop(record, now_unix_ms)?
            });
        }
        if !records.is_empty() {
            self.persist().map_err(|error| error.to_string())?;
        }
        Ok(notices)
    }

    /// The records a wakeup loop's own iteration wrote are that iteration's
    /// schedule, exactly as a `loop_schedule` call would be: the last schedule
    /// record sets the delay, the quiet mark and the next prompt; any stop
    /// record wins. Returns `false` — and leaves the receipt alone — when the
    /// turn is not the wakeup loop's, so the caller adopts the records instead.
    pub fn fold_wakeups_into_receipt(
        &mut self,
        turn: &LoopTurn,
        receipt: &mut TurnReceipt,
        records: &[WakeupRecord],
    ) -> bool {
        let limits = self.limits.clone();
        let Some(spec) = self
            .ledger
            .loops
            .iter_mut()
            .find(|spec| spec.id == turn.id && spec.source == LoopSource::Wakeup)
        else {
            return false;
        };
        if turn.kind != LoopTurnKind::Iteration {
            return false;
        }
        if records.is_empty() {
            return true;
        }
        if let Some(stop) = records.iter().rev().find(|record| record.stop) {
            receipt.loop_schedule = Some(LoopScheduleRequest {
                delay_secs: 0,
                noop: stop.noop,
                stop: true,
                reason: reason_of(stop),
            });
            return true;
        }
        if let Some(record) = records.iter().rev().find(|record| !record.stop) {
            spec.prompt.clone_from(&record.prompt);
            receipt.loop_schedule = Some(LoopScheduleRequest {
                delay_secs: clamp_wakeup_delay(record, &limits),
                noop: record.noop,
                stop: false,
                reason: reason_of(record),
            });
        }
        true
    }

    fn wakeup_loop_mut(&mut self) -> Option<&mut LoopSpec> {
        self.ledger.loops.iter_mut().find(|spec| {
            spec.source == LoopSource::Wakeup
                && matches!(spec.phase, LoopPhase::Running | LoopPhase::Paused)
        })
    }

    fn stop_wakeup_loop(&mut self, record: &WakeupRecord) -> String {
        match self.wakeup_loop_mut() {
            Some(spec) => {
                spec.phase = LoopPhase::Stopped;
                spec.pending = None;
                spec.next_at_unix_ms = None;
                spec.in_flight = false;
                spec.pause_reason = reason_of(record);
                format!("{WAKEUP_LABEL}: {} stopped", spec.id)
            }
            None => format!("{WAKEUP_LABEL}: nothing scheduled to stop"),
        }
    }

    fn arm_wakeup_loop(&mut self, record: &WakeupRecord, now_unix_ms: u64) -> Result<String, String> {
        let limits = self.limits.clone();
        let asked = record.delay_secs();
        let delay = clamp_wakeup_delay(record, &limits);
        // The tool stamps whole seconds; a record with no readable stamp
        // counts from now rather than from the epoch.
        let base = match record.scheduled_at_secs() {
            0 => now_unix_ms,
            secs => secs.saturating_mul(1_000),
        };
        let at = base.saturating_add(delay.saturating_mul(1_000));
        let clamped = if delay == asked {
            String::new()
        } else {
            format!(" (clamped from {asked}s)")
        };
        let quiet = u32::from(record.noop);
        if let Some(spec) = self.wakeup_loop_mut() {
            spec.phase = LoopPhase::Running;
            spec.trigger = Trigger::ModelWakeup { at_unix_ms: at };
            spec.next_at_unix_ms = Some(at);
            spec.prompt.clone_from(&record.prompt);
            spec.pending = Some(LoopTurnKind::Iteration);
            spec.in_flight = false;
            spec.quiet_streak = if record.noop {
                spec.quiet_streak.saturating_add(1)
            } else {
                0
            };
            spec.pause_reason = reason_of(record);
            return Ok(format!("{WAKEUP_LABEL}: {} armed · in {delay}s{clamped}", spec.id));
        }
        self.start(
            Trigger::ModelWakeup { at_unix_ms: at },
            record.prompt.clone(),
            ParsedOptions {
                check: None,
                max_runs: limits.max_loop_runs,
                token_budget: limits.max_output_tokens,
                // The model scheduled this from a turn that already ran under
                // the session's permission mode; the wakeup turn inherits it
                // the same way, and an approval it would need pauses it.
                allow_writes: true,
            },
            Some(at),
            LoopSource::Wakeup,
        )?;
        let spec = self.ledger.loops.last_mut().ok_or_else(|| "wakeup loop vanished".to_string())?;
        spec.quiet_streak = quiet;
        spec.pause_reason = reason_of(record);
        Ok(format!("{WAKEUP_LABEL}: {} armed · in {delay}s{clamped}", spec.id))
    }

    fn selected_mut(&mut self, id: Option<&str>) -> Result<&mut LoopSpec, String> {
        let selected = match id {
            Some(id) => self.ledger.loops.iter_mut().find(|spec| spec.id == id),
            None => self.ledger.loops.last_mut(),
        };
        selected.ok_or_else(|| {
            id.map_or_else(
                || "loop: none".to_string(),
                |id| format!("loop: unknown id {id}"),
            )
        })
    }

    fn persist(&self) -> io::Result<()> {
        let payload = serde_json::to_vec_pretty(&self.ledger).map_err(io::Error::other)?;
        if self.path.exists() {
            crate::write_atomic(&self.path, &payload)
        } else {
            core_types::paths::write_private_file(
                &self.path,
                &payload,
                &core_types::paths::ParentDirPolicy::LeaveParent,
            )
        }
    }
}

#[derive(Debug)]
struct ParsedOptions {
    check: Option<String>,
    max_runs: u32,
    token_budget: u64,
    allow_writes: bool,
}

fn parse_options(prompt: &str, limits: &AutonomyLimits) -> Result<(ParsedOptions, String), String> {
    let mut rest = prompt.trim_start();
    let mut options = ParsedOptions {
        check: None,
        max_runs: limits.max_loop_runs,
        token_budget: limits.max_output_tokens,
        allow_writes: false,
    };
    loop {
        let (flag, after_flag) = first_token(rest);
        match flag.as_str() {
            "--allow-writes" => {
                options.allow_writes = true;
                rest = after_flag;
            }
            "--check" | "--max-runs" | "--token-budget" => {
                let (value, after_value) = first_token(after_flag);
                if value.is_empty() {
                    return Err(format!("{flag} needs a value"));
                }
                match flag.as_str() {
                    "--check" => options.check = Some(value),
                    "--max-runs" => {
                        options.max_runs = positive_u32(&value, &flag)?.min(limits.max_loop_runs);
                    }
                    "--token-budget" => options.token_budget = positive_u64(&value, &flag)?,
                    _ => unreachable!(),
                }
                rest = after_value;
            }
            _ if flag.starts_with("--check=") => {
                options.check = flag.split_once('=').map(|(_, value)| value.to_string());
                rest = after_flag;
            }
            _ if flag.starts_with("--max-runs=") => {
                let value = flag.split_once('=').map_or("", |(_, value)| value);
                options.max_runs = positive_u32(value, "--max-runs")?.min(limits.max_loop_runs);
                rest = after_flag;
            }
            _ if flag.starts_with("--token-budget=") => {
                let value = flag.split_once('=').map_or("", |(_, value)| value);
                options.token_budget = positive_u64(value, "--token-budget")?;
                rest = after_flag;
            }
            _ => break,
        }
    }
    let prompt = unquote(rest.trim()).to_string();
    Ok((options, prompt))
}

fn positive_u32(value: &str, flag: &str) -> Result<u32, String> {
    value
        .parse::<u32>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| format!("{flag} needs a positive integer"))
}

fn positive_u64(value: &str, flag: &str) -> Result<u64, String> {
    value
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| format!("{flag} needs a positive integer"))
}

fn first_token(input: &str) -> (String, &str) {
    let input = input.trim_start();
    let Some(first) = input.chars().next() else {
        return (String::new(), "");
    };
    if matches!(first, '\'' | '"') {
        for (index, character) in input.char_indices().skip(1) {
            if character == first {
                return (
                    input[1..index].to_string(),
                    input[index + character.len_utf8()..].trim_start(),
                );
            }
        }
        return (input[1..].to_string(), "");
    }
    match input.find(char::is_whitespace) {
        Some(index) => (input[..index].to_string(), input[index..].trim_start()),
        None => (input.to_string(), ""),
    }
}

fn unquote(input: &str) -> &str {
    if input.len() >= 2 {
        let first = input.as_bytes()[0];
        let last = input.as_bytes()[input.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return &input[1..input.len() - 1];
        }
    }
    input
}

fn reason_of(record: &WakeupRecord) -> Option<String> {
    let reason = record.reason.trim();
    (!reason.is_empty()).then(|| reason.to_string())
}

/// The delay a wakeup record asked for, bounded by the limits table the way
/// `loop_schedule` bounds its own.
fn clamp_wakeup_delay(record: &WakeupRecord, limits: &AutonomyLimits) -> u64 {
    record
        .delay_secs()
        .clamp(limits.min_model_wakeup_secs, limits.max_model_wakeup_secs)
}

fn prompt_for(spec: &LoopSpec, kind: LoopTurnKind) -> String {
    match kind {
        LoopTurnKind::Iteration if spec.source == LoopSource::Wakeup => format!(
            "[zo:wakeup id={}]\n{}\n\nThis is the turn you scheduled with ScheduleWakeup. Call ScheduleWakeup again to continue (stop: true to end); a turn that schedules nothing ends the loop. Do not ask the absent user a question.",
            spec.id, spec.prompt
        ),
        LoopTurnKind::Iteration => format!(
            "[zo:loop-iteration id={}]\n{}\n\nRun one bounded iteration. Do not ask the absent user a question.",
            spec.id, spec.prompt
        ),
        LoopTurnKind::Gate | LoopTurnKind::RepairedGate => format!(
            "[zo:loop-gate id={}]\nRun this exact command once with the Bash tool and do not edit files:\n\n{}",
            spec.id,
            spec.check.as_deref().unwrap_or_default()
        ),
        LoopTurnKind::Repair => format!(
            "[zo:loop-repair id={}]\nThe iteration gate failed. Fix the root cause once; the controller will rerun the gate.",
            spec.id
        ),
    }
}

fn finish_iteration(spec: &mut LoopSpec, now_unix_ms: u64, limits: &AutonomyLimits) {
    if spec.budget.runs >= spec.budget.max_runs {
        spec.phase = LoopPhase::Completed;
        spec.pending = None;
        spec.next_at_unix_ms = None;
        return;
    }
    if matches!(spec.trigger, Trigger::Immediate | Trigger::ModelWakeup { .. }) {
        // A `/loop` that says nothing keeps its default cadence; a wakeup the
        // model asked for once fires once — silence after it ends the loop
        // rather than inventing a twenty-minute schedule nobody asked for.
        if spec.source == LoopSource::Wakeup && spec.pending_schedule.is_none() {
            spec.phase = LoopPhase::Completed;
            spec.pending = None;
            spec.next_at_unix_ms = None;
            spec.pause_reason = Some("no further wakeup scheduled".to_string());
            return;
        }
        let schedule = spec.pending_schedule.take().unwrap_or(LoopScheduleRequest {
            delay_secs: limits.default_model_wakeup_secs,
            noop: false,
            stop: false,
            reason: None,
        });
        if schedule.noop {
            spec.quiet_streak = spec.quiet_streak.saturating_add(1);
        } else {
            spec.quiet_streak = 0;
        }
        spec.pause_reason = schedule.reason;
        if schedule.stop {
            spec.phase = LoopPhase::Completed;
            spec.pending = None;
            spec.next_at_unix_ms = None;
        } else if spec.quiet_streak >= limits.max_consecutive_noops {
            spec.phase = LoopPhase::Paused;
            spec.pending = None;
            spec.next_at_unix_ms = None;
            spec.pause_reason = Some("quiet limit reached".to_string());
        } else {
            let at = now_unix_ms.saturating_add(schedule.delay_secs.saturating_mul(1_000));
            spec.trigger = Trigger::ModelWakeup { at_unix_ms: at };
            spec.pending = Some(LoopTurnKind::Iteration);
            spec.next_at_unix_ms = Some(at);
        }
        return;
    }
    spec.pending = Some(LoopTurnKind::Iteration);
    match &mut spec.trigger {
        Trigger::Count { left } => {
            *left = left.saturating_sub(1);
            if *left == 0 || spec.budget.runs >= spec.budget.max_runs {
                spec.phase = LoopPhase::Completed;
                spec.pending = None;
                spec.next_at_unix_ms = None;
            } else {
                spec.next_at_unix_ms = Some(now_unix_ms);
            }
        }
        Trigger::Every { seconds } => {
            spec.next_at_unix_ms = Some(now_unix_ms.saturating_add(
                (*seconds)
                    .max(limits.min_interval_secs)
                    .saturating_mul(1_000),
            ));
        }
        Trigger::Watch { .. } => {
            spec.next_at_unix_ms = Some(
                now_unix_ms.saturating_add(limits.poll_interval_secs.saturating_mul(1_000)),
            );
        }
        Trigger::Until { .. } => {
            if spec.until_satisfied {
                spec.phase = LoopPhase::Completed;
                spec.pending = None;
                spec.next_at_unix_ms = None;
            } else {
                spec.next_at_unix_ms = Some(
                    now_unix_ms.saturating_add(limits.poll_interval_secs.saturating_mul(1_000)),
                );
            }
        }
        Trigger::Immediate
        | Trigger::ModelWakeup { .. }
        | Trigger::ProviderReset { .. }
        | Trigger::Backoff { .. } => {
            spec.next_at_unix_ms = Some(now_unix_ms);
        }
    }
}

fn format_spec(spec: &LoopSpec) -> String {
    let reason = spec
        .pause_reason
        .as_deref()
        .map(|reason| format!(" · {reason}"))
        .unwrap_or_default();
    let quiet = if spec.quiet_streak > 0 {
        format!(" · quiet ×{}", spec.quiet_streak)
    } else {
        String::new()
    };
    let next = spec
        .next_at_unix_ms
        .map(|at| format!(" · next {at}"))
        .unwrap_or_default();
    let last = spec
        .last_receipt
        .as_deref()
        .map(|receipt| format!(" · last {receipt}"))
        .unwrap_or_default();
    format!(
        "loop: {} · {} · {} · runs {}/{} · tokens {}/{} · gates {}/{}{}{}{}{}",
        spec.id,
        spec.phase.label(),
        spec_label(spec),
        spec.budget.runs,
        spec.budget.max_runs,
        spec.budget.output_tokens,
        spec.budget.token_budget,
        spec.successes,
        spec.successes.saturating_add(spec.failures),
        next,
        quiet,
        reason,
        last
    )
}

/// What a loop is called on screen: a wakeup loop by its source, a command
/// loop by its trigger.
fn spec_label(spec: &LoopSpec) -> String {
    if spec.source == LoopSource::Wakeup {
        WAKEUP_LABEL.to_string()
    } else {
        trigger_label(&spec.trigger)
    }
}

fn trigger_label(trigger: &Trigger) -> String {
    match trigger {
        Trigger::Immediate | Trigger::ModelWakeup { .. } => "dynamic".to_string(),
        Trigger::Every { seconds } => format!("every {seconds}s"),
        Trigger::Count { left } => format!("count · {left} left"),
        Trigger::Watch { glob, .. } => format!("watch {glob}"),
        Trigger::Until { cmd } => format!("until {cmd}"),
        Trigger::ProviderReset { .. } => "provider reset".to_string(),
        Trigger::Backoff { n } => format!("backoff {}", n.saturating_add(1)),
    }
}

fn watch_fingerprint(cwd: &Path, glob: &str) -> Option<u64> {
    let mut hasher = DefaultHasher::new();
    hash_matching_tree(cwd, cwd, glob, &mut hasher).ok()?;
    Some(hasher.finish())
}

fn hash_matching_tree(
    root: &Path,
    directory: &Path,
    pattern: &str,
    hasher: &mut DefaultHasher,
) -> io::Result<()> {
    let mut entries = std::fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let relative = path.strip_prefix(root).unwrap_or(&path);
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            if !matches!(entry.file_name().to_str(), Some(".git" | "target" | "node_modules")) {
                hash_matching_tree(root, &path, pattern, hasher)?;
            }
        } else if file_type.is_file() && wildcard_match(pattern.as_bytes(), relative.to_string_lossy().as_bytes()) {
            relative.hash(hasher);
            entry
                .metadata()?
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_nanos())
                .hash(hasher);
        }
    }
    Ok(())
}

fn wildcard_match(pattern: &[u8], text: &[u8]) -> bool {
    match pattern.split_first() {
        None => text.is_empty(),
        Some((&b'*', rest)) => {
            let rest = rest.strip_prefix(b"*").unwrap_or(rest);
            wildcard_match(rest, text)
                || (!text.is_empty() && wildcard_match(pattern, &text[1..]))
        }
        Some((&b'?', rest)) => !text.is_empty() && wildcard_match(rest, &text[1..]),
        Some((&expected, rest)) => {
            text.first().is_some_and(|actual| *actual == expected)
                && wildcard_match(rest, &text[1..])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{LoopPhase, LoopRegistry};
    use crate::autonomy::limits::AutonomyLimits;
    use crate::autonomy::runner::TurnReceipt;
    use commands::{LoopCommand, SlashCommand};
    use tools::wakeup_store::WakeupRecord;

    fn command(input: &str) -> LoopCommand {
        let parsed = SlashCommand::parse(input).expect("parse").expect("command");
        let SlashCommand::Loop { command } = parsed else {
            panic!("expected loop command");
        };
        command
    }

    fn receipt() -> TurnReceipt {
        TurnReceipt {
            workspace_before: "a".to_string(),
            workspace_after: "b".to_string(),
            gate: None,
            output_tokens: 1,
            active_millis: 1,
            blocked: None,
            todo_completed_delta: 0,
            loop_schedule: None,
        }
    }

    #[test]
    fn fixed_count_runs_exactly_the_requested_number_of_iterations() {
        let temp = tempfile::tempdir().expect("tempdir");
        let transcript = temp.path().join("s.jsonl");
        let mut loops = LoopRegistry::load(&transcript, false, AutonomyLimits::default());
        loops
            .apply(command("/loop 3 echo hi"), 1_000, temp.path())
            .expect("start");

        for _ in 0..3 {
            let turn = loops
                .dispatch_due(1_000, temp.path())
                .expect("dispatch")
                .expect("due");
            loops
                .record_turn(&turn, receipt(), 1_000)
                .expect("record");
        }

        assert!(
            loops
                .dispatch_due(1_000, temp.path())
                .expect("dispatch")
                .is_none()
        );
        assert_eq!(loops.phase("loop-1"), Some(LoopPhase::Completed));
    }

    #[test]
    fn resumed_sidecar_keeps_specs_but_pauses_running_loops() {
        let temp = tempfile::tempdir().expect("tempdir");
        let transcript = temp.path().join("s.jsonl");
        let mut loops = LoopRegistry::load(&transcript, false, AutonomyLimits::default());
        loops
            .apply(
                command("/loop every 10m inspect CI"),
                1_000,
                temp.path(),
            )
            .expect("start");

        let resumed = LoopRegistry::load(&transcript, true, AutonomyLimits::default());

        assert!(!resumed.is_empty());
        assert_eq!(resumed.phase("loop-1"), Some(LoopPhase::Paused));
    }

    #[test]
    fn consecutive_noops_pause_at_the_limit() {
        let temp = tempfile::tempdir().expect("tempdir");
        let transcript = temp.path().join("s.jsonl");
        let limits = AutonomyLimits {
            max_consecutive_noops: 2,
            ..AutonomyLimits::default()
        };
        let mut loops = LoopRegistry::load(&transcript, false, limits.clone());
        loops
            .apply(command("/loop monitor CI"), 1_000, temp.path())
            .expect("start");

        for now in [1_000, 1_000 + limits.min_model_wakeup_secs * 1_000] {
            let turn = loops
                .dispatch_due(now, temp.path())
                .expect("dispatch")
                .expect("due");
            let mut quiet = receipt();
            quiet.loop_schedule = Some(crate::autonomy::wakeup::LoopScheduleRequest {
                delay_secs: limits.min_model_wakeup_secs,
                noop: true,
                stop: false,
                reason: Some("still waiting".to_string()),
            });
            loops.record_turn(&turn, quiet, now).expect("record");
        }

        assert_eq!(loops.phase("loop-1"), Some(LoopPhase::Paused));
        assert!(loops.status_report(Some("loop-1")).contains("quiet ×2"));
    }

    #[test]
    fn satisfied_until_runs_one_final_iteration_then_stops() {
        let temp = tempfile::tempdir().expect("tempdir");
        let transcript = temp.path().join("s.jsonl");
        let mut loops = LoopRegistry::load(&transcript, false, AutonomyLimits::default());
        loops
            .apply(command("/loop until 'true' announce done"), 1_000, temp.path())
            .expect("start");

        let turn = loops
            .dispatch_due(1_000, temp.path())
            .expect("dispatch")
            .expect("final iteration");
        loops.record_turn(&turn, receipt(), 1_000).expect("record");

        assert_eq!(loops.phase("loop-1"), Some(LoopPhase::Completed));
    }

    #[test]
    fn until_poll_never_executes_a_write_command() {
        let temp = tempfile::tempdir().expect("tempdir");
        let transcript = temp.path().join("s.jsonl");
        let mut loops = LoopRegistry::load(&transcript, false, AutonomyLimits::default());
        loops
            .apply(
                command("/loop until 'touch escaped' announce done"),
                1_000,
                temp.path(),
            )
            .expect("start");

        assert!(
            loops
                .dispatch_due(1_000, temp.path())
                .expect("dispatch")
                .is_none()
        );
        assert!(!temp.path().join("escaped").exists());
        assert_eq!(loops.phase("loop-1"), Some(LoopPhase::Paused));
    }

    fn wakeup(delay_seconds: f64, prompt: &str, scheduled_at: u64) -> WakeupRecord {
        WakeupRecord {
            delay_seconds,
            reason: "poll".to_string(),
            prompt: prompt.to_string(),
            scheduled_at: scheduled_at.to_string(),
            session_id: Some("s".to_string()),
            noop: false,
            stop: false,
        }
    }

    fn stop_record() -> WakeupRecord {
        WakeupRecord {
            stop: true,
            prompt: String::new(),
            ..wakeup(0.0, "", 1)
        }
    }

    #[test]
    fn a_schedule_record_arms_one_wakeup_loop_and_a_second_re_arms_it() {
        let temp = tempfile::tempdir().expect("tempdir");
        let transcript = temp.path().join("s.jsonl");
        let limits = AutonomyLimits {
            min_model_wakeup_secs: 1,
            ..AutonomyLimits::default()
        };
        let mut loops = LoopRegistry::load(&transcript, false, limits);

        let notices = loops
            .adopt_wakeups(&[wakeup(45.0, "first prompt", 1_000)], 1_000_500)
            .expect("adopt");
        assert_eq!(notices, ["wakeup: loop-1 armed · in 45s"]);
        assert_eq!(loops.next_wakeup(), Some(1_045_000));
        assert!(loops.dispatch_due(1_044_999, temp.path()).expect("not yet").is_none());

        let notices = loops
            .adopt_wakeups(&[wakeup(10.0, "second prompt", 1_100)], 1_100_500)
            .expect("re-arm");
        assert_eq!(notices, ["wakeup: loop-1 armed · in 10s"]);
        assert_eq!(loops.next_wakeup(), Some(1_110_000), "the newer record owns the clock");
        let turn = loops
            .dispatch_due(1_110_000, temp.path())
            .expect("dispatch")
            .expect("due");
        assert!(turn.prompt.contains("[zo:wakeup id=loop-1]"));
        assert!(turn.prompt.contains("second prompt"));
        assert!(turn.dynamic);
        assert!(turn.allow_writes, "a wakeup turn inherits the session's permission mode");
        assert!(loops.status_report(Some("loop-1")).contains("· wakeup ·"));
    }

    #[test]
    fn the_delay_is_bounded_by_the_limits_table() {
        let temp = tempfile::tempdir().expect("tempdir");
        let transcript = temp.path().join("s.jsonl");
        let limits = AutonomyLimits::default();
        let mut loops = LoopRegistry::load(&transcript, false, limits.clone());

        let notices = loops
            .adopt_wakeups(&[wakeup(45.0, "too soon", 1_000)], 1_000_000)
            .expect("adopt");

        assert_eq!(
            notices,
            [format!(
                "wakeup: loop-1 armed · in {}s (clamped from 45s)",
                limits.min_model_wakeup_secs
            )]
        );
        assert_eq!(
            loops.next_wakeup(),
            Some((1_000 + limits.min_model_wakeup_secs) * 1_000)
        );
    }

    #[test]
    fn a_stop_record_ends_the_wakeup_loop_so_it_never_fires() {
        let temp = tempfile::tempdir().expect("tempdir");
        let transcript = temp.path().join("s.jsonl");
        let limits = AutonomyLimits {
            min_model_wakeup_secs: 1,
            ..AutonomyLimits::default()
        };
        let mut loops = LoopRegistry::load(&transcript, false, limits);

        let notices = loops
            .adopt_wakeups(&[wakeup(2.0, "never", 1_000), stop_record()], 1_000_000)
            .expect("adopt");

        assert_eq!(
            notices,
            ["wakeup: loop-1 armed · in 2s", "wakeup: loop-1 stopped"]
        );
        assert_eq!(loops.phase("loop-1"), Some(LoopPhase::Stopped));
        assert_eq!(loops.next_wakeup(), None);
        assert!(loops.dispatch_due(10_000_000, temp.path()).expect("dispatch").is_none());
        assert_eq!(
            loops.adopt_wakeups(&[stop_record()], 1_000_000).expect("stop again"),
            ["wakeup: nothing scheduled to stop"]
        );
    }

    #[test]
    fn a_wakeup_turn_that_schedules_nothing_ends_the_loop() {
        let temp = tempfile::tempdir().expect("tempdir");
        let transcript = temp.path().join("s.jsonl");
        let limits = AutonomyLimits {
            min_model_wakeup_secs: 1,
            ..AutonomyLimits::default()
        };
        let mut loops = LoopRegistry::load(&transcript, false, limits);
        loops
            .adopt_wakeups(&[wakeup(1.0, "once", 1_000)], 1_000_000)
            .expect("adopt");
        let turn = loops
            .dispatch_due(1_001_000, temp.path())
            .expect("dispatch")
            .expect("due");

        let mut receipt = receipt();
        assert!(loops.fold_wakeups_into_receipt(&turn, &mut receipt, &[]));
        assert!(receipt.loop_schedule.is_none());
        let notice = loops.record_turn(&turn, receipt, 1_001_500).expect("record");

        assert_eq!(loops.phase("loop-1"), Some(LoopPhase::Completed));
        assert!(notice.contains("no further wakeup scheduled"), "{notice}");
        assert_eq!(loops.next_wakeup(), None);
    }

    #[test]
    fn a_wakeup_turn_that_schedules_again_is_re_armed_with_the_new_prompt() {
        let temp = tempfile::tempdir().expect("tempdir");
        let transcript = temp.path().join("s.jsonl");
        let limits = AutonomyLimits {
            min_model_wakeup_secs: 1,
            ..AutonomyLimits::default()
        };
        let mut loops = LoopRegistry::load(&transcript, false, limits);
        loops
            .adopt_wakeups(&[wakeup(1.0, "first", 1_000)], 1_000_000)
            .expect("adopt");
        let turn = loops
            .dispatch_due(1_001_000, temp.path())
            .expect("dispatch")
            .expect("due");

        let mut receipt = receipt();
        let mut again = wakeup(30.0, "second", 1_001);
        again.noop = true;
        assert!(loops.fold_wakeups_into_receipt(&turn, &mut receipt, &[again]));
        assert_eq!(
            receipt.loop_schedule.as_ref().map(|schedule| (schedule.delay_secs, schedule.noop)),
            Some((30, true))
        );
        loops.record_turn(&turn, receipt, 1_001_500).expect("record");

        assert_eq!(loops.phase("loop-1"), Some(LoopPhase::Running));
        assert_eq!(loops.next_wakeup(), Some(1_031_500));
        assert_eq!(loops.quiet_streak("loop-1"), Some(1));
        let next = loops
            .dispatch_due(1_031_500, temp.path())
            .expect("dispatch")
            .expect("due");
        assert!(next.prompt.contains("second"));
        assert!(!next.prompt.contains("first"));
    }

    #[test]
    fn records_written_during_a_command_loop_turn_are_not_that_loop_s_schedule() {
        let temp = tempfile::tempdir().expect("tempdir");
        let transcript = temp.path().join("s.jsonl");
        let mut loops = LoopRegistry::load(&transcript, false, AutonomyLimits::default());
        loops
            .apply(command("/loop 3 echo hi"), 1_000, temp.path())
            .expect("start");
        let turn = loops
            .dispatch_due(1_000, temp.path())
            .expect("dispatch")
            .expect("due");

        let mut receipt = receipt();
        let folded = loops.fold_wakeups_into_receipt(&turn, &mut receipt, &[wakeup(60.0, "aside", 1)]);

        assert!(!folded, "a command loop's turn leaves the records to adoption");
        assert!(receipt.loop_schedule.is_none());
    }
}
