//! 세션에 붙는 지속 목표와 bounded autonomous continuation 원장.
//!
//! 목표는 대화 메시지가 아니다. 트랜스크립트 옆 `<session>.goal.json` 에 두어
//! 압축이 지울 수 없게 하면서, 프로젝트 공용 `.zo/` 에 놓아 다른 세션까지
//! 번지게 하지 않는다. 목표 문자열 자체는 런타임의 `Session::session_goal` 에도
//! 미러링된다. 그 필드는 트랜스크립트 헤더의 정본이고 TurnEnd/trace가 읽는다;
//! 이 sidecar는 자율 진행률과 네 한도처럼 대화 세션 형식에 속하지 않는 host
//! 상태만 가진다.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use commands::{GoalCommand, GoalOptions, SlashCommand};
use core_types::session::ContentBlock;
use serde::{Deserialize, Serialize};

use crate::autonomy::limits::AutonomyLimits;
use crate::autonomy::runner::{is_transient_error, provider_reset_at};
use crate::autonomy::scheduler::Trigger;

pub use crate::autonomy::limits::{
    DEFAULT_MAX_ASSISTANT_TURNS, DEFAULT_MAX_CONTINUATIONS, DEFAULT_MAX_OUTPUT_TOKENS,
    DEFAULT_MAX_WALL_CLOCK_SECS,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalPhase {
    Saved,
    Running,
    Paused,
    Completed,
}

/// The compact, human-facing state that belongs in the persistent footer.
///
/// This deliberately records only the state, not the objective: the footer is
/// a status indicator, while `/goal status` remains the place to read the
/// objective and its limits.  The strings for these variants live in
/// `status_format`, alongside the other shared status presentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalFooterStatus {
    Pursuing,
    Paused,
    Stalled,
    /// zo has four autonomous limits, not only Codex's usage limit.  Its
    /// wording is therefore a deliberately minimal extension of Codex's
    /// `Goal hit usage limits` form.
    HitAutonomousLimits,
    Unmet,
    Achieved,
}

/// Recover the footer state from the status snapshot already cached by the
/// TUI.  During a turn the session is owned by the turn task, so the UI cannot
/// borrow [`GoalController`] directly; these two fields are the stable status
/// facts it has on every draw.
#[must_use]
pub fn footer_status_from_snapshot(goal: &str, autonomous: &str) -> Option<GoalFooterStatus> {
    let phase = goal.rsplit_once('(')?.1.strip_suffix(')')?;
    match phase {
        "running" => Some(GoalFooterStatus::Pursuing),
        "completed" => Some(GoalFooterStatus::Achieved),
        // A saved objective is intentionally not advertised as paused: bare
        // `/goal <objective>` has not armed autonomous execution yet.
        "saved" => Some(GoalFooterStatus::Unmet),
        "paused" => {
            if autonomous.contains("workspace unchanged after repair") {
                Some(GoalFooterStatus::Stalled)
            } else if autonomous.contains("limit reached") {
                Some(GoalFooterStatus::HitAutonomousLimits)
            } else {
                Some(GoalFooterStatus::Paused)
            }
        }
        _ => None,
    }
}

impl GoalPhase {
    const fn label(self) -> &'static str {
        match self {
            Self::Saved => "saved",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Completed => "completed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GoalTurnKind {
    Plan,
    Action,
    Gate { index: usize },
    Repair { index: usize },
    Condition,
}

impl GoalTurnKind {
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Action => "action",
            Self::Gate { .. } => "gate",
            Self::Repair { .. } => "repair",
            Self::Condition => "condition",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct FailedGate {
    index: usize,
    workspace_fingerprint: String,
    output: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "the persisted policy snapshot keeps independent plan, edit, permission, and clock facts"
)]
struct GoalState {
    objective: String,
    phase: GoalPhase,
    checks: Vec<String>,
    #[serde(default)]
    until: Option<String>,
    #[serde(default)]
    every_secs: Option<u64>,
    #[serde(default)]
    plan_enabled: bool,
    #[serde(default)]
    planned: bool,
    #[serde(default)]
    max_plan_items: u32,
    allow_writes: bool,
    max_continuations: u32,
    max_assistant_turns: u32,
    max_output_tokens: u64,
    max_wall_clock_secs: u64,
    continuations: u32,
    assistant_turns: u32,
    output_tokens: u64,
    active_millis: u64,
    active_since_unix_ms: Option<u64>,
    dispatched_turns: u32,
    #[serde(default)]
    action_turns: u32,
    #[serde(default)]
    gates_passed: u32,
    #[serde(default)]
    todo_completed: u32,
    #[serde(default)]
    stalled_turns: u32,
    #[serde(default)]
    transient_failures: usize,
    #[serde(default)]
    next_at_unix_ms: Option<u64>,
    #[serde(default)]
    scheduled_trigger: Option<Trigger>,
    #[serde(default)]
    objective_edited: bool,
    #[serde(default)]
    last_receipt: Option<String>,
    pending: Option<GoalTurnKind>,
    last_failed_gate: Option<FailedGate>,
    pause_reason: Option<String>,
}

impl GoalState {
    fn new(
        objective: String,
        options: &GoalOptions,
        autonomous: bool,
        limits: &AutonomyLimits,
    ) -> Self {
        let checks = options
            .checks
            .iter()
            .map(|check| gate_command(check))
            .collect::<Vec<_>>();
        let until = options.until.as_deref().map(gate_command);
        let can_run = autonomous && (!checks.is_empty() || until.is_some());
        let pause_reason = (autonomous && checks.is_empty() && until.is_none())
            .then(|| "autonomous start needs a --check gate or --until condition".to_string());
        let plan_enabled = !options.no_plan;
        Self {
            objective,
            phase: if can_run {
                GoalPhase::Running
            } else if pause_reason.is_some() {
                GoalPhase::Paused
            } else {
                GoalPhase::Saved
            },
            checks,
            until,
            every_secs: options.every.as_ref().map(|every| {
                every.duration.as_secs().clamp(
                    limits.min_interval_secs,
                    limits.max_interval_secs,
                )
            }),
            plan_enabled,
            planned: !plan_enabled,
            max_plan_items: limits.max_plan_items,
            allow_writes: options.allow_writes,
            max_continuations: limits.max_continuations,
            max_assistant_turns: options
                .max_turns
                .unwrap_or(limits.max_assistant_turns),
            // A token budget is optional at the command surface, but autonomous
            // execution never gets an unbounded interpretation of `None`.
            max_output_tokens: options
                .token_budget
                .unwrap_or(limits.max_output_tokens),
            max_wall_clock_secs: limits.max_wall_clock_secs,
            continuations: 0,
            assistant_turns: 0,
            output_tokens: 0,
            active_millis: 0,
            active_since_unix_ms: can_run.then(now_unix_ms),
            dispatched_turns: 0,
            action_turns: 0,
            gates_passed: 0,
            todo_completed: 0,
            stalled_turns: 0,
            transient_failures: 0,
            next_at_unix_ms: None,
            scheduled_trigger: can_run.then_some(Trigger::Immediate),
            objective_edited: false,
            last_receipt: None,
            pending: can_run.then_some(if plan_enabled {
                GoalTurnKind::Plan
            } else {
                GoalTurnKind::Action
            }),
            last_failed_gate: None,
            pause_reason,
        }
    }

    fn elapsed_millis(&self, now: u64) -> u64 {
        self.active_millis.saturating_add(
            self.active_since_unix_ms
                .map_or(0, |since| now.saturating_sub(since)),
        )
    }

    fn stop_clock(&mut self) {
        let now = now_unix_ms();
        self.active_millis = self.elapsed_millis(now);
        self.active_since_unix_ms = None;
    }

    fn start_clock(&mut self) {
        if self.active_since_unix_ms.is_none() {
            self.active_since_unix_ms = Some(now_unix_ms());
        }
    }

    fn pause(&mut self, reason: impl Into<String>) {
        self.stop_clock();
        self.phase = GoalPhase::Paused;
        self.pending = None;
        self.next_at_unix_ms = None;
        self.scheduled_trigger = None;
        self.pause_reason = Some(reason.into());
    }

    fn complete(&mut self) {
        self.stop_clock();
        self.phase = GoalPhase::Completed;
        self.pending = None;
        self.next_at_unix_ms = None;
        self.scheduled_trigger = None;
        self.pause_reason = None;
    }

    fn exhaustion(&self) -> Option<String> {
        if self.assistant_turns >= self.max_assistant_turns {
            return Some(format!(
                "assistant-turn limit reached ({}/{})",
                self.assistant_turns, self.max_assistant_turns
            ));
        }
        if self.output_tokens >= self.max_output_tokens {
            return Some(format!(
                "output-token limit reached ({}/{})",
                self.output_tokens, self.max_output_tokens
            ));
        }
        if self.elapsed_millis(now_unix_ms())
            >= self.max_wall_clock_secs.saturating_mul(1_000)
        {
            return Some(format!(
                "wall-clock limit reached ({}s)",
                self.max_wall_clock_secs
            ));
        }
        None
    }
}

/// 실행 직전에 원장에서 꺼내는 한 턴. `allow_writes=false` 면 host가 런타임의
/// 활성 권한을 그 턴 동안 read-only로 낮춘다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalTurn {
    pub kind: GoalTurnKind,
    pub prompt: String,
    pub allow_writes: bool,
    objective: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalCommandResult {
    pub notice: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalTurnReport {
    pub output_tokens: u64,
    pub active_millis: u64,
    pub workspace_before: String,
    pub workspace_fingerprint: String,
    pub gate: Option<GateObservation>,
    pub permission_blocked: bool,
    pub question_blocked: bool,
    pub cancelled: bool,
    pub error: Option<String>,
    pub todo_completed_delta: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateObservation {
    pub passed: bool,
    pub output: String,
}

/// 세션 목표 host 상태. 모든 변경은 곧바로 sidecar에 반영된다.
pub struct GoalController {
    path: PathBuf,
    state: Option<GoalState>,
    limits: AutonomyLimits,
}

impl GoalController {
    #[must_use]
    pub fn load(session_path: &Path, session_goal: Option<&str>, resumed: bool) -> Self {
        let path = sidecar_path(session_path);
        let limits = AutonomyLimits::load();
        let state = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<GoalState>(&bytes).ok())
            .filter(|state| Some(state.objective.as_str()) == session_goal)
            .or_else(|| {
                session_goal.map(|objective| GoalState {
                    objective: objective.to_string(),
                    phase: GoalPhase::Saved,
                    checks: Vec::new(),
                    until: None,
                    every_secs: None,
                    plan_enabled: false,
                    planned: true,
                    max_plan_items: limits.max_plan_items,
                    allow_writes: false,
                    max_continuations: limits.max_continuations,
                    max_assistant_turns: limits.max_assistant_turns,
                    max_output_tokens: limits.max_output_tokens,
                    max_wall_clock_secs: limits.max_wall_clock_secs,
                    continuations: 0,
                    assistant_turns: 0,
                    output_tokens: 0,
                    active_millis: 0,
                    active_since_unix_ms: None,
                    dispatched_turns: 0,
                    action_turns: 0,
                    gates_passed: 0,
                    todo_completed: 0,
                    stalled_turns: 0,
                    transient_failures: 0,
                    next_at_unix_ms: None,
                    scheduled_trigger: None,
                    objective_edited: false,
                    last_receipt: None,
                    pending: None,
                    last_failed_gate: None,
                    pause_reason: None,
                })
            });
        let mut controller = Self {
            path,
            state,
            limits,
        };
        // A prior explicit start does not silently relaunch work merely because
        // a process resumed the transcript. The state survives; continuation
        // requires the equally explicit `/goal resume`.
        if resumed {
            if let Some(state) = controller
                .state
                .as_mut()
                .filter(|state| state.phase == GoalPhase::Running)
            {
                state.stop_clock();
                state.phase = GoalPhase::Paused;
                state.pause_reason = Some(
                    "session resumed; use /goal resume to continue autonomously".to_string(),
                );
                let _ = controller.persist();
            }
        }
        controller
    }

    #[must_use]
    pub fn objective(&self) -> Option<&str> {
        self.state.as_ref().map(|state| state.objective.as_str())
    }

    #[must_use]
    pub fn reminder_state(&self) -> Option<(&str, GoalPhase)> {
        self.state
            .as_ref()
            .map(|state| (state.objective.as_str(), state.phase))
    }

    #[must_use]
    pub fn is_running(&self) -> bool {
        self.state
            .as_ref()
            .is_some_and(|state| state.phase == GoalPhase::Running && state.pending.is_some())
    }

    /// `/goal`의 raw remainder를 읽는다. `start`만 자율 실행을 켠다; bare goal은
    /// 지속 목표를 저장하기만 하므로 기본값은 언제나 off다.
    pub fn apply(
        &mut self,
        raw: &str,
        workspace_fingerprint: Option<&str>,
    ) -> Result<GoalCommandResult, String> {
        let trimmed = raw.trim();
        let first = trimmed.split_whitespace().next().unwrap_or_default();
        if matches!(first, "complete" | "done") && trimmed.split_whitespace().count() == 1 {
            let Some(state) = self.state.as_mut() else {
                return Ok(GoalCommandResult {
                    notice: "goal: none".to_string(),
                });
            };
            state.complete();
            self.persist().map_err(|error| error.to_string())?;
            return Ok(GoalCommandResult {
                notice: "goal: completed explicitly".to_string(),
            });
        }
        // commands' broad compatibility parser treats `stop` as clear. zo's
        // requested contract distinguishes stop (pause) from clear (forget).
        let parsed = if first == "stop" && trimmed.split_whitespace().count() == 1 {
            GoalCommand::Pause
        } else {
            let input = if trimmed.is_empty() {
                "/goal".to_string()
            } else {
                format!("/goal {trimmed}")
            };
            match SlashCommand::parse(&input).map_err(|error| error.to_string())? {
                Some(SlashCommand::Goal { command }) => command,
                _ => return Err("goal command did not parse as /goal".to_string()),
            }
        };
        let explicitly_autonomous = first == "start";
        self.apply_parsed(parsed, explicitly_autonomous, workspace_fingerprint)
    }

    #[allow(clippy::too_many_lines)] // `/goal`의 여덟 명령이 각 한 arm인 정본 dispatch.
    fn apply_parsed(
        &mut self,
        command: GoalCommand,
        explicitly_autonomous: bool,
        workspace_fingerprint: Option<&str>,
    ) -> Result<GoalCommandResult, String> {
        let notice = match command {
            GoalCommand::Status | GoalCommand::History => self.status_report(),
            GoalCommand::Start { goal, options } => {
                self.state = Some(GoalState::new(
                    goal,
                    &options,
                    explicitly_autonomous,
                    &self.limits,
                ));
                let state = self.state.as_ref().expect("goal was just installed");
                if state.phase == GoalPhase::Running {
                    let completion = state.until.as_ref().map_or_else(
                        || format!("{} gate(s)", state.checks.len()),
                        |_| "until condition".to_string(),
                    );
                    format!("goal: running · {completion} · four limits armed")
                } else if state.phase == GoalPhase::Paused {
                    format!(
                        "goal: saved · autonomous off ({})",
                        state.pause_reason.as_deref().unwrap_or("paused")
                    )
                } else {
                    "goal: saved · autonomous off (use /goal start <goal> --check <command>)"
                        .to_string()
                }
            }
            GoalCommand::Verify => {
                let Some(state) = self.state.as_mut() else {
                    return Ok(GoalCommandResult {
                        notice: "goal: none".to_string(),
                    });
                };
                if state.checks.is_empty() && state.until.is_none() {
                    "goal: cannot verify without a --check gate or --until condition".to_string()
                } else if state.last_failed_gate.as_ref().is_some_and(|failed| {
                    workspace_fingerprint
                        .is_some_and(|fingerprint| fingerprint == failed.workspace_fingerprint)
                }) {
                    "goal: gate not rerun — workspace is unchanged since the same failure"
                        .to_string()
                } else {
                    state.phase = GoalPhase::Running;
                    state.pause_reason = None;
                    state.pending = Some(if state.until.is_some() {
                        GoalTurnKind::Condition
                    } else {
                        let index = state
                            .last_failed_gate
                            .as_ref()
                            .map_or(0, |failed| failed.index);
                        GoalTurnKind::Gate { index }
                    });
                    state.next_at_unix_ms = None;
                    state.scheduled_trigger = Some(Trigger::Immediate);
                    state.start_clock();
                    "goal: verification queued".to_string()
                }
            }
            GoalCommand::Pause => {
                let Some(state) = self.state.as_mut() else {
                    return Ok(GoalCommandResult {
                        notice: "goal: none".to_string(),
                    });
                };
                state.pause("stopped explicitly");
                "goal: stopped · objective preserved".to_string()
            }
            GoalCommand::Resume => {
                let Some(state) = self.state.as_mut() else {
                    return Ok(GoalCommandResult {
                        notice: "goal: none".to_string(),
                    });
                };
                if state.phase == GoalPhase::Completed {
                    "goal: already completed; edit or clear it before another run".to_string()
                } else if state.checks.is_empty() && state.until.is_none() {
                    state.pause("resume needs a --check gate or --until condition");
                    "goal: autonomous off — no completion policy is configured".to_string()
                } else if let Some(exhausted) = state.exhaustion() {
                    state.pause(exhausted.clone());
                    format!("goal: cannot resume — {exhausted}")
                } else {
                    state.phase = GoalPhase::Running;
                    state.pause_reason = None;
                    state.pending.get_or_insert(GoalTurnKind::Action);
                    state.next_at_unix_ms = None;
                    state.scheduled_trigger = Some(Trigger::Immediate);
                    state.start_clock();
                    "goal: autonomous continuation resumed".to_string()
                }
            }
            GoalCommand::Clear => {
                self.state = None;
                match std::fs::remove_file(&self.path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.to_string()),
                }
                return Ok(GoalCommandResult {
                    notice: "goal: cleared".to_string(),
                });
            }
            GoalCommand::Edit { goal } => {
                let Some(state) = self.state.as_mut() else {
                    self.state = Some(GoalState::new(
                        goal,
                        &GoalOptions::default(),
                        false,
                        &self.limits,
                    ));
                    self.persist().map_err(|error| error.to_string())?;
                    return Ok(GoalCommandResult {
                        notice: "goal: saved · autonomous off".to_string(),
                    });
                };
                let was_running = state.phase == GoalPhase::Running;
                state.objective = goal;
                state.phase = if was_running {
                    GoalPhase::Running
                } else {
                    GoalPhase::Saved
                };
                state.pending = was_running.then_some(GoalTurnKind::Action);
                state.pause_reason = None;
                state.objective_edited = true;
                state.next_at_unix_ms = None;
                state.scheduled_trigger = was_running.then_some(Trigger::Immediate);
                if was_running {
                    "goal: edited · next autonomous prompt will announce the change".to_string()
                } else {
                    state.stop_clock();
                    "goal: edited · autonomous off".to_string()
                }
            }
        };
        self.persist().map_err(|error| error.to_string())?;
        Ok(GoalCommandResult { notice })
    }

    /// 다음 자동 턴을 디스패치한다. 첫 action 뒤의 모든 턴은 continuation으로
    /// 세며, 한도에 닿으면 prompt를 만들기 전에 멈춘다.
    pub fn dispatch_next(&mut self) -> Result<Option<GoalTurn>, String> {
        let Some(state) = self.state.as_mut() else {
            return Ok(None);
        };
        if state.phase != GoalPhase::Running {
            return Ok(None);
        }
        if state
            .next_at_unix_ms
            .is_some_and(|next_at| next_at > now_unix_ms())
        {
            return Ok(None);
        }
        state.next_at_unix_ms = None;
        state.scheduled_trigger = None;
        state.start_clock();
        if let Some(exhausted) = state.exhaustion() {
            state.pause(exhausted);
            self.persist().map_err(|error| error.to_string())?;
            return Ok(None);
        }
        if state.dispatched_turns > 0 {
            if state.continuations >= state.max_continuations {
                state.pause(format!(
                    "continuation limit reached ({}/{})",
                    state.continuations, state.max_continuations
                ));
                self.persist().map_err(|error| error.to_string())?;
                return Ok(None);
            }
            state.continuations = state.continuations.saturating_add(1);
        }
        let Some(kind) = state.pending.clone() else {
            return Ok(None);
        };
        state.dispatched_turns = state.dispatched_turns.saturating_add(1);
        let prompt = prompt_for(state, &kind);
        state.objective_edited = false;
        let allow_writes = state.allow_writes
            && matches!(kind, GoalTurnKind::Action | GoalTurnKind::Repair { .. });
        let objective = state.objective.clone();
        self.persist().map_err(|error| error.to_string())?;
        Ok(Some(GoalTurn {
            kind,
            prompt,
            allow_writes,
            objective,
        }))
    }

    /// 한 자동 턴의 결과를 기록하고 다음 action/gate/repair를 결정한다.
    #[expect(
        clippy::too_many_lines,
        reason = "the policy transition table keeps every goal turn and blocking outcome exhaustive in one place"
    )]
    pub fn record_turn(
        &mut self,
        turn: &GoalTurn,
        report: GoalTurnReport,
    ) -> Result<String, String> {
        let now = now_unix_ms();
        let backoff_secs = self.limits.backoff_secs;
        let max_stalled_turns = self.limits.max_stalled_turns;
        let max_failure_output_chars = self.limits.max_failure_output_chars;
        let Some(state) = self.state.as_mut() else {
            return Ok("goal: result ignored because the goal was cleared".to_string());
        };
        if state.objective != turn.objective {
            return Ok("goal: result ignored because the goal changed during the turn".to_string());
        }
        if state.phase != GoalPhase::Running {
            return Ok(format!("goal: {}", state.phase.label()));
        }
        let GoalTurnReport {
            output_tokens,
            active_millis: _,
            workspace_before,
            workspace_fingerprint,
            gate,
            permission_blocked,
            question_blocked,
            cancelled,
            error,
            todo_completed_delta,
        } = report;
        state.assistant_turns = state.assistant_turns.saturating_add(1);
        state.output_tokens = state.output_tokens.saturating_add(output_tokens);
        let workspace_label = if workspace_before == workspace_fingerprint {
            "workspace unchanged"
        } else {
            "workspace changed"
        };
        state.last_receipt = Some(format!(
            "{workspace_label} · output {output_tokens} · todo +{todo_completed_delta}"
        ));
        state.todo_completed = state
            .todo_completed
            .saturating_add(todo_completed_delta);
        if permission_blocked {
            state.pause(
                "approval required during an unattended turn; stopped without bypassing it",
            );
        } else if question_blocked {
            state.pause("user input required during an unattended turn; stopped for the user");
        } else if cancelled {
            state.pause("autonomous turn interrupted");
        } else if let Some(error) = error {
            if let Some(reset_at) = provider_reset_at(&error, now) {
                state.pending = Some(turn.kind.clone());
                state.next_at_unix_ms = Some(reset_at);
                state.scheduled_trigger = Some(Trigger::ProviderReset {
                    at_unix_ms: reset_at,
                });
                state.pause_reason = Some(format!("provider limit; resumes at {reset_at}"));
                state.stop_clock();
            } else if is_transient_error(&error) && state.transient_failures < backoff_secs.len() {
                let attempt = state.transient_failures;
                let next_at = now.saturating_add(backoff_secs[attempt].saturating_mul(1_000));
                state.transient_failures = state.transient_failures.saturating_add(1);
                state.pending = Some(turn.kind.clone());
                state.next_at_unix_ms = Some(next_at);
                state.scheduled_trigger = Some(Trigger::Backoff { n: attempt });
                state.pause_reason = Some(format!(
                    "transient failure; retry {}/{} scheduled",
                    state.transient_failures,
                    backoff_secs.len()
                ));
                state.stop_clock();
            } else {
                state.pause(format!("autonomous turn failed: {error}"));
            }
        } else if let Some(exhausted) = state.exhaustion() {
            state.pause(exhausted);
        } else {
            state.transient_failures = 0;
            state.pause_reason = None;
            let progressed = turn.kind == GoalTurnKind::Plan
                || workspace_before != workspace_fingerprint
                || todo_completed_delta > 0
                || gate.is_some();
            state.stalled_turns = if progressed {
                0
            } else {
                state.stalled_turns.saturating_add(1)
            };
            if state.stalled_turns >= max_stalled_turns {
                state.pause("two unattended turns made no workspace, gate, or todo progress");
            } else {
                match turn.kind {
                GoalTurnKind::Plan => {
                    state.planned = true;
                    state.pending = Some(GoalTurnKind::Action);
                }
                GoalTurnKind::Action => {
                    state.action_turns = state.action_turns.saturating_add(1);
                    state.pending = Some(if state.until.is_some() {
                        GoalTurnKind::Condition
                    } else {
                        GoalTurnKind::Gate { index: 0 }
                    });
                }
                GoalTurnKind::Gate { index } => {
                    let observation = gate.unwrap_or_else(|| GateObservation {
                        passed: false,
                        output: "the exact gate command was not observed in Bash tool results"
                            .to_string(),
                    });
                    if observation.passed {
                        state.last_failed_gate = None;
                        state.gates_passed = state.gates_passed.saturating_add(1);
                        if index + 1 < state.checks.len() {
                            state.pending = Some(GoalTurnKind::Gate { index: index + 1 });
                        } else {
                            state.complete();
                        }
                    } else {
                        state.last_failed_gate = Some(FailedGate {
                            index,
                            workspace_fingerprint,
                            output: bounded_output(
                                &observation.output,
                                max_failure_output_chars,
                            ),
                        });
                        state.pending = Some(GoalTurnKind::Repair { index });
                    }
                }
                GoalTurnKind::Repair { index } => {
                    let unchanged = state.last_failed_gate.as_ref().is_some_and(|failed| {
                        failed.index == index
                            && failed.workspace_fingerprint == workspace_fingerprint
                    });
                    if unchanged {
                        state.pause(
                            "workspace unchanged after repair; the same gate was not rerun",
                        );
                    } else {
                        state.pending = Some(GoalTurnKind::Gate { index });
                    }
                }
                GoalTurnKind::Condition => {
                    let passed = gate.is_some_and(|observation| observation.passed);
                    if passed {
                        state.gates_passed = 1;
                        state.complete();
                    } else {
                        state.pending = Some(GoalTurnKind::Action);
                        if let Some(seconds) = state.every_secs {
                            let next_at = now.saturating_add(seconds.saturating_mul(1_000));
                            state.next_at_unix_ms = Some(next_at);
                            state.scheduled_trigger = Some(Trigger::Every { seconds });
                            state.stop_clock();
                        } else {
                            state.scheduled_trigger = Some(Trigger::Immediate);
                        }
                    }
                }
                }
            }
        }
        let notice = self.status_report();
        self.persist().map_err(|error| error.to_string())?;
        Ok(notice)
    }

    #[must_use]
    pub fn expected_gate(&self, turn: &GoalTurn) -> Option<&str> {
        match turn.kind {
            GoalTurnKind::Gate { index } => self
                .state
                .as_ref()
                .and_then(|state| state.checks.get(index))
                .map(String::as_str),
            GoalTurnKind::Condition => self
                .state
                .as_ref()
                .and_then(|state| state.until.as_deref()),
            GoalTurnKind::Plan | GoalTurnKind::Action | GoalTurnKind::Repair { .. } => None,
        }
    }

    #[must_use]
    pub fn next_wakeup(&self) -> Option<u64> {
        self.state.as_ref().and_then(|state| {
            (state.phase == GoalPhase::Running)
                .then_some(state.next_at_unix_ms)
                .flatten()
        })
    }

    #[must_use]
    pub fn autonomy_status(&self) -> Option<crate::autonomy::GoalStatus> {
        self.state.as_ref().map(|state| crate::autonomy::GoalStatus {
            phase: state.phase.label().to_string(),
            next_at: state.next_at_unix_ms,
            gates_passed: state.gates_passed,
            gates_total: if state.until.is_some() {
                1
            } else {
                u32::try_from(state.checks.len()).unwrap_or(u32::MAX)
            },
            action_turns: state.action_turns,
            stalled_turns: state.stalled_turns,
            pause_reason: state.pause_reason.clone(),
        })
    }

    #[must_use]
    pub fn status_fields(&self) -> (String, String) {
        let Some(state) = self.state.as_ref() else {
            return ("none".to_string(), "off".to_string());
        };
        let goal = format!("{} ({})", one_line(&state.objective, 72), state.phase.label());
        let elapsed = state.elapsed_millis(now_unix_ms()) / 1_000;
        let gates_total = if state.until.is_some() {
            1
        } else {
            u32::try_from(state.checks.len()).unwrap_or(u32::MAX)
        };
        let autonomous = if state.phase == GoalPhase::Running {
            let next = state
                .next_at_unix_ms
                .map(|at| format!(" · next {at}"))
                .unwrap_or_default();
            format!(
                "on · action {} · gates {}/{} · stalled {} · todo {} · continuations {}/{} · turns {}/{} · tokens {}/{} · time {}s/{}s{}",
                state.action_turns,
                state.gates_passed,
                gates_total,
                state.stalled_turns,
                state.todo_completed,
                state.continuations,
                state.max_continuations,
                state.assistant_turns,
                state.max_assistant_turns,
                state.output_tokens,
                state.max_output_tokens,
                elapsed,
                state.max_wall_clock_secs,
                next
            )
        } else {
            let reason = state
                .pause_reason
                .as_deref()
                .map(|reason| format!(" · {}", one_line(reason, 60)))
                .unwrap_or_default();
            format!("off{reason}")
        };
        let last = state
            .last_receipt
            .as_deref()
            .map(|receipt| format!(" · last {receipt}"))
            .unwrap_or_default();
        (goal, format!("{autonomous}{last}"))
    }

    #[must_use]
    pub fn status_report(&self) -> String {
        let (goal, autonomous) = self.status_fields();
        format!("goal: {goal} · autonomous: {autonomous}")
    }

    pub fn pause_for_session_limit(&mut self, reason: &str) -> Result<(), String> {
        if let Some(state) = self
            .state
            .as_mut()
            .filter(|state| state.phase == GoalPhase::Running)
        {
            state.pause(reason);
            self.persist().map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    fn persist(&self) -> io::Result<()> {
        let Some(state) = self.state.as_ref() else {
            return Ok(());
        };
        let payload = serde_json::to_vec_pretty(state).map_err(io::Error::other)?;
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

/// Gate 턴의 assistant/tool 결과에서 **정확히 요청한 Bash command**의 결과만
/// 고른다. 모델이 말로 "통과"했다고 주장하거나 다른 검사를 돌린 것은 신호가
/// 아니다.
#[must_use]
pub fn observe_gate(summary: &runtime::TurnSummary, expected: &str) -> GateObservation {
    let mut call_ids = Vec::new();
    for message in &summary.assistant_messages {
        for block in &message.blocks {
            let ContentBlock::ToolUse { id, name, input } = block else {
                continue;
            };
            if !name.eq_ignore_ascii_case("bash") {
                continue;
            }
            let command = serde_json::from_str::<serde_json::Value>(input)
                .ok()
                .and_then(|value| value.get("command").and_then(|value| value.as_str()).map(str::to_string))
                .unwrap_or_else(|| input.clone());
            if command.trim() == expected.trim() {
                call_ids.push(id.as_str());
            }
        }
    }
    if call_ids.len() != 1 {
        return GateObservation {
            passed: false,
            output: format!(
                "gate must run exactly once (observed {} exact calls): {expected}",
                call_ids.len()
            ),
        };
    }
    for message in &summary.tool_results {
        for block in &message.blocks {
            let ContentBlock::ToolResult {
                tool_use_id,
                output,
                is_error,
                ..
            } = block
            else {
                continue;
            };
            if call_ids.contains(&tool_use_id.as_str()) {
                // The Bash tool answers a failing command without an error
                // flag: the gate passed only if the command exited 0.
                return GateObservation {
                    passed: !is_error && runtime::bash_result_exited_zero(output),
                    output: output.clone(),
                };
            }
        }
    }
    GateObservation {
        passed: false,
        output: format!("gate was not executed exactly once: {expected}"),
    }
}

/// Record what the gate's exit settled, as an OBJECTIVE verdict about the
/// turn's own attempt.
///
/// This is the row the accuracy report's "model-only verdicts" share moves
/// off 100% with: every verdict the ledger held until now was an LLM saying
/// the work looked right, and a completion loop that drives to a model-only
/// green is the deepest form of appearance-over-results the design warns
/// about. A gate command's exit is the opposite kind of evidence, and it was
/// being observed and then thrown away.
///
/// Nothing is invented: with no checker there is no row (the same rule
/// `objective_verdict_from_exit` holds), and with no wire model on the turn —
/// a turn that never reached a provider — there is no route to credit, so
/// again no row. The model is read off the assistant message the provider
/// actually answered with, never guessed from settings.
pub fn record_gate_verdict(
    cwd: &std::path::Path,
    summary: &runtime::TurnSummary,
    attempt: &str,
    observation: &GateObservation,
) {
    if attempt.trim().is_empty() {
        return;
    }
    let Some(model) = summary
        .assistant_messages
        .iter()
        .rev()
        .find_map(|message| message.model.as_deref())
        .map(str::trim)
        .filter(|model| !model.is_empty())
    else {
        return;
    };
    let record = runtime::RouteOutcomeRecord::new(
        "main",
        "turn",
        model,
        if observation.passed {
            runtime::OUTCOME_COMPLETED
        } else {
            runtime::OUTCOME_FAILED
        },
    )
    .with_signal("verdict")
    .with_decision(runtime::DecisionKind::Verify)
    .with_verdict_basis(runtime::VerdictBasis::Objective)
    // The shape the turn ran under, as the host deposited it when it decided.
    .with_shape_label(api::plan_shape_for_attempt(attempt).unwrap_or_default())
    .with_attempt_key(attempt);
    let _ = runtime::record_route_outcome(cwd, &record);
}

fn prompt_for(state: &GoalState, kind: &GoalTurnKind) -> String {
    match *kind {
        GoalTurnKind::Plan => format!(
            "[zo:goal-plan]\nPersistent goal: {}\n\nPlan only: use TodoWrite once to split this goal into at most {} concrete items. Do not edit the workspace in this turn.",
            state.objective, state.max_plan_items
        ),
        GoalTurnKind::Action => {
            let changed = if state.objective_edited {
                "\n\nThe persistent goal changed since the prior turn. Reconcile the plan and act on the new objective."
            } else {
                ""
            };
            format!(
                "[zo:goal-action]\nPersistent goal: {}{changed}\n\nTake the next concrete actions toward this goal in this bounded turn. Preserve prior progress, do not ask the absent user a question, and do not declare the goal complete: the controller decides only from its explicit completion policy.",
                state.objective
            )
        }
        GoalTurnKind::Gate { index } => {
            let check = &state.checks[index];
            format!(
                "[zo:goal-gate]\nPersistent goal: {}\n\nRun this exact command once with the Bash tool and do not edit files in this turn:\n\n{}\n\nReport its real output. Do not replace it with another command and do not claim success without the tool result.",
                state.objective, check
            )
        }
        GoalTurnKind::Repair { index } => {
            let check = &state.checks[index];
            let output = state
                .last_failed_gate
                .as_ref()
                .map_or("gate failed without output", |failed| failed.output.as_str());
            format!(
                "[zo:goal-repair]\nPersistent goal: {}\n\nThe gate failed. Fix the root cause in the workspace in this bounded turn. Do not rerun the gate yourself; the controller will rerun it only if the workspace changed.\n\nGate command:\n{}\n\nFailure output:\n{}",
                state.objective, check, output
            )
        }
        GoalTurnKind::Condition => format!(
            "[zo:goal-condition]\nPersistent goal: {}\n\nRun this exact completion condition once with the Bash tool and do not edit files:\n\n{}",
            state.objective,
            state.until.as_deref().unwrap_or_default()
        ),
    }
}

fn sidecar_path(session_path: &Path) -> PathBuf {
    session_path.with_extension("goal.json")
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}

fn one_line(text: &str, max_chars: usize) -> String {
    let folded = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if folded.chars().count() <= max_chars {
        return folded;
    }
    let mut out = folded
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    out.push('…');
    out
}

fn bounded_output(output: &str, max_chars: u64) -> String {
    one_line(output, usize::try_from(max_chars).unwrap_or(usize::MAX))
}

fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// How long a screen check waits for what it names: the gate is the check,
/// not the wait — a window or a line the goal's last turn left on the screen
/// is there within ten seconds.
const SCREEN_GATE_TIMEOUT_MS: u64 = 10_000;

/// The command a `--check` value's gate runs (`commands::GoalCheck`). A screen
/// check asks the window's Computer Use shim, whose exit code is the verdict
/// (a refusal or a timeout exits 1).
pub(crate) fn gate_command(check: &str) -> String {
    use commands::GoalCheck;
    let screen = |what: &str| {
        format!(
            "{} wait-for {what} --timeout-ms {SCREEN_GATE_TIMEOUT_MS} --json",
            tools::COMPUTER_SHIM
        )
    };
    match GoalCheck::parse(check) {
        Ok(GoalCheck::CargoFmt) => "cargo fmt --all --check".to_string(),
        Ok(GoalCheck::CargoCheck) => "cargo check --workspace".to_string(),
        Ok(GoalCheck::CargoTest) => "cargo test --workspace".to_string(),
        Ok(GoalCheck::CargoClippy) => {
            "cargo clippy --workspace --all-targets --no-deps -- -D warnings".to_string()
        }
        Ok(GoalCheck::GitDiff) => "git diff --quiet".to_string(),
        Ok(GoalCheck::GitDiffCheck) => "git diff --check".to_string(),
        Ok(GoalCheck::Grep(pattern)) => format!("rg --quiet -- {} .", shell_quote(&pattern)),
        Ok(GoalCheck::ScreenWindow(title)) => screen(&format!("--window {}", shell_quote(&title))),
        Ok(GoalCheck::ScreenText(text)) => screen(&format!("--text {} --ocr", shell_quote(&text))),
        Ok(GoalCheck::Command(command)) => command,
        Err(_) => check.trim().to_string(),
    }
}

/// 워크스페이스 내용 지문. Git 트리에서는 HEAD + tracked diff + staged diff +
/// untracked contents를 재고, Git 밖 테스트/임시 디렉터리에서는 제한된 재귀
/// 해시로 떨어진다. 파일명 목록만 재면 이미 dirty인 파일을 다시 고친 변화가
/// 보이지 않으므로 실제 바이트를 센다.
pub fn workspace_fingerprint(cwd: &Path) -> io::Result<String> {
    let mut hasher = DefaultHasher::new();
    let head = Command::new("git")
        .args(["rev-parse", "--verify", "HEAD"])
        .current_dir(cwd)
        .output();
    if let Ok(head) = head {
        if head.status.success() {
            head.stdout.hash(&mut hasher);
            for args in [
                &["diff", "--binary", "--no-ext-diff", "HEAD"][..],
                &["diff", "--cached", "--binary", "--no-ext-diff"][..],
            ] {
                let output = Command::new("git").args(args).current_dir(cwd).output()?;
                output.status.code().hash(&mut hasher);
                output.stdout.hash(&mut hasher);
                output.stderr.hash(&mut hasher);
            }
            let untracked = Command::new("git")
                .args(["ls-files", "--others", "--exclude-standard", "-z"])
                .current_dir(cwd)
                .output()?;
            for relative in untracked.stdout.split(|byte| *byte == 0) {
                if relative.is_empty() {
                    continue;
                }
                relative.hash(&mut hasher);
                hash_file(&cwd.join(relative_path(relative)), &mut hasher)?;
            }
            return Ok(format!("{:016x}", hasher.finish()));
        }
    }
    hash_tree(cwd, cwd, &mut hasher)?;
    Ok(format!("{:016x}", hasher.finish()))
}

#[cfg(unix)]
fn relative_path(bytes: &[u8]) -> PathBuf {
    use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
    PathBuf::from(OsStr::from_bytes(bytes))
}

#[cfg(not(unix))]
fn relative_path(bytes: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
}

fn hash_file(path: &Path, hasher: &mut DefaultHasher) -> io::Result<()> {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let mut buffer = vec![0u8; 64 * 1024].into_boxed_slice();
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        buffer[..read].hash(hasher);
    }
    Ok(())
}

fn hash_tree(root: &Path, directory: &Path, hasher: &mut DefaultHasher) -> io::Result<()> {
    let mut entries = std::fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let file_name = entry.file_name();
        if matches!(file_name.to_str(), Some(".git" | "target" | "node_modules")) {
            continue;
        }
        let path = entry.path();
        path.strip_prefix(root).unwrap_or(&path).hash(hasher);
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            hash_tree(root, &path, hasher)?;
        } else if file_type.is_file() {
            hash_file(&path, hasher)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        workspace_fingerprint, GateObservation, GoalController, GoalPhase, GoalTurnKind,
        GoalTurnReport,
    };

    #[test]
    fn footer_snapshot_distinguishes_goal_terminal_and_paused_states() {
        use super::{footer_status_from_snapshot, GoalFooterStatus};

        assert_eq!(
            footer_status_from_snapshot("ship (running)", "on · turns 1/12"),
            Some(GoalFooterStatus::Pursuing)
        );
        assert_eq!(
            footer_status_from_snapshot("ship (saved)", "off"),
            Some(GoalFooterStatus::Unmet)
        );
        assert_eq!(
            footer_status_from_snapshot("ship (paused)", "off · stopped explicitly"),
            Some(GoalFooterStatus::Paused)
        );
        assert_eq!(
            footer_status_from_snapshot(
                "ship (paused)",
                "off · workspace unchanged after repair; the same gate was not rerun",
            ),
            Some(GoalFooterStatus::Stalled)
        );
        assert_eq!(
            footer_status_from_snapshot("ship (paused)", "off · output-token limit reached (8/8)"),
            Some(GoalFooterStatus::HitAutonomousLimits)
        );
        assert_eq!(
            footer_status_from_snapshot("ship (completed)", "off"),
            Some(GoalFooterStatus::Achieved)
        );
        assert_eq!(footer_status_from_snapshot("none", "off"), None);
    }
    use std::fs;

    /// A gate turn that ran `command` once, as the Bash tool answered it.
    fn gate_turn(command: &str, answer: &serde_json::Value, is_error: bool) -> runtime::TurnSummary {
        use core_types::session::{ContentBlock, ConversationMessage};
        runtime::TurnSummary {
            assistant_messages: vec![ConversationMessage::assistant(vec![ContentBlock::ToolUse {
                id: "gate-1".into(),
                name: "bash".into(),
                input: serde_json::json!({ "command": command }).to_string(),
            }])],
            tool_results: vec![ConversationMessage::tool_result("gate-1", "bash", answer.to_string(), is_error)],
            prompt_cache_events: Vec::new(),
            iterations: 1,
            usage: runtime::TokenUsage::default(),
            turn_output_tokens: 0,
            auto_compaction: None,
            microcompact: None,
            deep_verification: None,
            verification_issues: Vec::new(),
            deep_verifier_parse: None,
            deep_verifier_model: None,
            budget_exhausted: None,
        }
    }

    /// The Bash tool answers a failing command without an error flag — its
    /// exit code is in the answer. A gate is passed only by a zero exit: a
    /// failing `cargo test` gate must never complete the goal.
    #[test]
    fn a_gate_passes_on_its_commands_real_exit_code_not_on_the_tool_answering() {
        use super::observe_gate;
        let gate = "cargo test --workspace";
        let failed = gate_turn(
            gate,
            &serde_json::json!({ "stdout": "test result: FAILED", "stderr": "", "interrupted": false,
                "returnCodeInterpretation": "exit_code:101" }),
            false,
        );
        assert!(!observe_gate(&failed, gate).passed, "exit 101 is a failed gate");
        let killed = gate_turn(
            gate,
            &serde_json::json!({ "stdout": "", "stderr": "", "interrupted": false, "returnCodeInterpretation": "signal" }),
            false,
        );
        assert!(!observe_gate(&killed, gate).passed, "a killed gate did not pass");
        let green = gate_turn(
            gate,
            &serde_json::json!({ "stdout": "test result: ok", "stderr": "", "interrupted": false, "returnCodeInterpretation": null }),
            false,
        );
        let observed = observe_gate(&green, gate);
        assert!(observed.passed, "{}", observed.output);
        let refused = gate_turn(gate, &serde_json::json!({ "stdout": "", "interrupted": false }), true);
        assert!(!observe_gate(&refused, gate).passed, "a refused call ran nothing");
    }

    /// A screen check is the window's Computer Use shim waiting for what it
    /// names, bounded: its exit code is the gate's verdict.
    #[test]
    fn a_screen_check_waits_through_the_computer_shim_and_its_exit_is_the_verdict() {
        use super::{gate_command, SCREEN_GATE_TIMEOUT_MS};
        assert_eq!(
            gate_command("screen:window:Order #12"),
            format!("{} wait-for --window 'Order #12' --timeout-ms {SCREEN_GATE_TIMEOUT_MS} --json", tools::COMPUTER_SHIM)
        );
        assert_eq!(
            gate_command("screen:text:Payment received"),
            format!(
                "{} wait-for --text 'Payment received' --ocr --timeout-ms {SCREEN_GATE_TIMEOUT_MS} --json",
                tools::COMPUTER_SHIM
            )
        );
        assert_eq!(gate_command("cargo:test"), "cargo test --workspace");
        assert_eq!(gate_command("grep:TODO"), "rg --quiet -- 'TODO' .");
        assert_eq!(gate_command("just verify"), "just verify");
        // The window's CLI takes the line the gate runs.
        let words = shell_words(&gate_command("screen:window:Order #12"));
        assert!(zerocode_core_parses(&words[1..]), "{words:?}");
    }

    /// Split a gate line the way the shell does for the quoting it uses.
    fn shell_words(line: &str) -> Vec<String> {
        let mut words = Vec::new();
        let mut word = String::new();
        let mut quoted = false;
        for character in line.chars() {
            match character {
                '\'' => quoted = !quoted,
                ' ' if !quoted => {
                    if !word.is_empty() {
                        words.push(std::mem::take(&mut word));
                    }
                }
                other => word.push(other),
            }
        }
        if !word.is_empty() {
            words.push(word);
        }
        words
    }

    fn zerocode_core_parses(words: &[String]) -> bool {
        zerocode_core::computer_use::parse_command(words).is_ok()
    }

    fn controller() -> (tempfile::TempDir, GoalController) {
        let temp = tempfile::tempdir().expect("tempdir");
        let transcript = temp.path().join("session.jsonl");
        fs::write(&transcript, "").expect("transcript");
        (temp, GoalController::load(&transcript, None, false))
    }

    #[test]
    fn bare_goal_is_saved_but_only_start_enables_autonomy() {
        let (_temp, mut controller) = controller();
        controller
            .apply("ship it --check \"just verify\"", None)
            .expect("save");
        assert!(!controller.is_running());
        assert_eq!(controller.objective(), Some("ship it"));
        controller
            .apply("start ship it --check \"just verify\" --max-turns 3 --no-plan", None)
            .expect("start");
        assert!(controller.is_running());
        let turn = controller.dispatch_next().expect("dispatch").expect("turn");
        assert_eq!(turn.kind, GoalTurnKind::Action);
    }

    #[test]
    fn planned_goal_dispatches_a_plan_before_action() {
        let (_temp, mut controller) = controller();
        controller
            .apply("start ship it --check 'true'", None)
            .expect("start");

        let turn = controller.dispatch_next().expect("dispatch").expect("turn");

        assert_eq!(turn.kind, GoalTurnKind::Plan);
    }

    #[test]
    fn provider_rate_limit_schedules_resume_without_pausing_the_goal() {
        let (_temp, mut controller) = controller();
        controller
            .apply("start ship --check 'true' --no-plan", None)
            .expect("start");
        let turn = controller.dispatch_next().expect("dispatch").expect("turn");
        controller
            .record_turn(
                &turn,
                GoalTurnReport {
                    output_tokens: 0,
                    active_millis: 1,
                    workspace_before: "a".to_string(),
                    workspace_fingerprint: "a".to_string(),
                    gate: None,
                    permission_blocked: false,
                    question_blocked: false,
                    cancelled: false,
                    error: Some("429 rate limit; retry-after: 90".to_string()),
                    todo_completed_delta: 0,
                },
            )
            .expect("record");

        let state = controller.state.as_ref().expect("state");
        assert_eq!(state.phase, GoalPhase::Running);
        assert!(matches!(
            state.scheduled_trigger,
            Some(crate::autonomy::scheduler::Trigger::ProviderReset { .. })
        ));
        assert!(controller.next_wakeup().is_some());
    }

    #[test]
    fn autonomy_status_carries_pause_reason_and_clear_removes_it() {
        let (_temp, mut controller) = controller();
        controller.apply("start ship --check 'true' --no-plan", None).unwrap();
        controller.apply("pause", None).unwrap();
        let state = controller.state.as_ref().unwrap();
        let status = controller.autonomy_status().unwrap();
        assert_eq!(status.phase, "paused");
        assert_eq!(status.pause_reason, state.pause_reason);
        assert!(status.pause_reason.is_some());
        let wire = serde_json::to_value(&status).unwrap();
        assert_eq!(wire["pause_reason"], status.pause_reason.unwrap());
        controller.apply("clear", None).unwrap();
        assert!(controller.autonomy_status().is_none());
    }

    #[test]
    fn transient_failures_use_three_backoffs_then_pause() {
        let (_temp, mut controller) = controller();
        controller
            .apply("start ship --check 'true' --no-plan", None)
            .expect("start");
        let turn = controller.dispatch_next().expect("dispatch").expect("turn");

        for _ in 0..4 {
            controller
                .record_turn(
                    &turn,
                    GoalTurnReport {
                        output_tokens: 0,
                        active_millis: 1,
                        workspace_before: "a".to_string(),
                        workspace_fingerprint: "a".to_string(),
                        gate: None,
                        permission_blocked: false,
                        question_blocked: false,
                        cancelled: false,
                        error: Some("network timeout".to_string()),
                        todo_completed_delta: 0,
                    },
                )
                .expect("record");
        }

        assert_eq!(
            controller.state.as_ref().map(|state| state.phase),
            Some(GoalPhase::Paused)
        );
    }

    #[test]
    fn until_condition_completes_only_from_its_observed_command() {
        let (_temp, mut controller) = controller();
        controller
            .apply("start deploy --until \"test -f ready\" --no-plan", None)
            .expect("start");
        let action = controller.dispatch_next().expect("dispatch").expect("action");
        controller
            .record_turn(
                &action,
                GoalTurnReport {
                    output_tokens: 1,
                    active_millis: 1,
                    workspace_before: "a".to_string(),
                    workspace_fingerprint: "a".to_string(),
                    gate: None,
                    permission_blocked: false,
                    question_blocked: false,
                    cancelled: false,
                    error: None,
                    todo_completed_delta: 0,
                },
            )
            .expect("action");
        let condition = controller
            .dispatch_next()
            .expect("dispatch")
            .expect("condition");
        assert_eq!(condition.kind, GoalTurnKind::Condition);
        assert_eq!(controller.expected_gate(&condition), Some("test -f ready"));
        controller
            .record_turn(
                &condition,
                GoalTurnReport {
                    output_tokens: 1,
                    active_millis: 1,
                    workspace_before: "a".to_string(),
                    workspace_fingerprint: "a".to_string(),
                    gate: Some(GateObservation {
                        passed: true,
                        output: String::new(),
                    }),
                    permission_blocked: false,
                    question_blocked: false,
                    cancelled: false,
                    error: None,
                    todo_completed_delta: 0,
                },
            )
            .expect("condition");

        assert!(controller.status_report().contains("completed"));
    }

    #[test]
    fn unchanged_repair_pauses_before_the_same_gate_can_run_again() {
        let (_temp, mut controller) = controller();
        controller
            .apply("start fix it --check \"just verify\" --allow-writes --no-plan", None)
            .expect("start");
        let action = controller.dispatch_next().expect("dispatch").expect("action");
        controller
            .record_turn(
                &action,
                GoalTurnReport {
                    output_tokens: 10,
                    active_millis: 1,
                    workspace_before: "dirty-a".to_string(),
                    workspace_fingerprint: "dirty-a".to_string(),
                    gate: None,
                    permission_blocked: false,
                    question_blocked: false,
                    cancelled: false,
                    error: None,
                    todo_completed_delta: 0,
                },
            )
            .expect("record action");
        let gate = controller.dispatch_next().expect("dispatch").expect("gate");
        controller
            .record_turn(
                &gate,
                GoalTurnReport {
                    output_tokens: 5,
                    active_millis: 1,
                    workspace_before: "dirty-a".to_string(),
                    workspace_fingerprint: "dirty-a".to_string(),
                    gate: Some(GateObservation {
                        passed: false,
                        output: "red".to_string(),
                    }),
                    permission_blocked: false,
                    question_blocked: false,
                    cancelled: false,
                    error: None,
                    todo_completed_delta: 0,
                },
            )
            .expect("record gate");
        let repair = controller.dispatch_next().expect("dispatch").expect("repair");
        assert_eq!(repair.kind, GoalTurnKind::Repair { index: 0 });
        controller
            .record_turn(
                &repair,
                GoalTurnReport {
                    output_tokens: 5,
                    active_millis: 1,
                    workspace_before: "dirty-a".to_string(),
                    workspace_fingerprint: "dirty-a".to_string(),
                    gate: None,
                    permission_blocked: false,
                    question_blocked: false,
                    cancelled: false,
                    error: None,
                    todo_completed_delta: 0,
                },
            )
            .expect("record repair");
        assert!(!controller.is_running());
        assert!(controller.status_report().contains("workspace unchanged"));
        assert!(controller.dispatch_next().expect("dispatch").is_none());
    }

    #[test]
    fn approval_need_pauses_instead_of_being_bypassed() {
        let (_temp, mut controller) = controller();
        controller
            .apply("start fix it --check \"just verify\" --allow-writes --no-plan", None)
            .expect("start");
        let action = controller.dispatch_next().expect("dispatch").expect("action");
        controller
            .record_turn(
                &action,
                GoalTurnReport {
                    output_tokens: 1,
                    active_millis: 1,
                    workspace_before: "x".to_string(),
                    workspace_fingerprint: "x".to_string(),
                    gate: None,
                    permission_blocked: true,
                    question_blocked: false,
                    cancelled: false,
                    error: None,
                    todo_completed_delta: 0,
                },
            )
            .expect("record");
        assert!(controller.status_report().contains("approval required"));
        assert!(!controller.is_running());
    }

    #[test]
    fn a_green_explicit_gate_completes_but_does_not_forget_the_goal() {
        let (_temp, mut controller) = controller();
        controller
            .apply("start ship --check \"just verify\" --allow-writes --no-plan", None)
            .expect("start");
        let action = controller.dispatch_next().expect("dispatch").expect("action");
        controller
            .record_turn(
                &action,
                GoalTurnReport {
                    output_tokens: 10,
                    active_millis: 1,
                    workspace_before: "before".to_string(),
                    workspace_fingerprint: "changed".to_string(),
                    gate: None,
                    permission_blocked: false,
                    question_blocked: false,
                    cancelled: false,
                    error: None,
                    todo_completed_delta: 0,
                },
            )
            .expect("action");
        let gate = controller.dispatch_next().expect("dispatch").expect("gate");
        controller
            .record_turn(
                &gate,
                GoalTurnReport {
                    output_tokens: 2,
                    active_millis: 1,
                    workspace_before: "changed".to_string(),
                    workspace_fingerprint: "changed".to_string(),
                    gate: Some(GateObservation {
                        passed: true,
                        output: "green".to_string(),
                    }),
                    permission_blocked: false,
                    question_blocked: false,
                    cancelled: false,
                    error: None,
                    todo_completed_delta: 0,
                },
            )
            .expect("gate");
        assert_eq!(controller.objective(), Some("ship"));
        assert!(controller.status_report().contains("completed"));
        assert!(!controller.is_running());
    }

    #[test]
    fn every_autonomous_axis_has_a_finite_hard_stop() {
        let (_temp, mut limits) = controller();
        limits
            .apply("start fix --check \"just verify\" --no-plan", None)
            .expect("start");
        let state = limits.state.as_ref().expect("state");
        assert!(state.max_continuations > 0);
        assert!(state.max_assistant_turns > 0);
        assert!(state.max_output_tokens > 0);
        assert!(state.max_wall_clock_secs > 0);

        for axis in ["continuations", "assistant turns", "tokens", "wall clock"] {
            let (_temp, mut bounded) = controller();
            bounded
                .apply("start fix --check \"just verify\" --no-plan", None)
                .expect("start");
            let state = bounded.state.as_mut().expect("state");
            match axis {
                "continuations" => {
                    state.dispatched_turns = 1;
                    state.continuations = state.max_continuations;
                }
                "assistant turns" => state.assistant_turns = state.max_assistant_turns,
                "tokens" => state.output_tokens = state.max_output_tokens,
                "wall clock" => {
                    state.active_millis = state.max_wall_clock_secs * 1_000;
                    state.active_since_unix_ms = None;
                }
                _ => unreachable!(),
            }
            assert!(bounded.dispatch_next().expect("dispatch").is_none(), "{axis}");
            assert!(!bounded.is_running(), "{axis}");
        }
    }

    #[test]
    fn completed_goal_survives_reload_and_clear_is_the_only_forget_path() {
        let (temp, mut controller) = controller();
        let transcript = temp.path().join("session.jsonl");
        controller
            .apply("start ship --check \"just verify\" --no-plan", None)
            .expect("start");
        controller.apply("complete", None).expect("complete");
        assert_eq!(controller.state.as_ref().map(|state| state.phase), Some(GoalPhase::Completed));
        let mut loaded = GoalController::load(&transcript, Some("ship"), true);
        assert_eq!(loaded.objective(), Some("ship"));
        assert!(loaded.status_report().contains("completed"));
        loaded.apply("clear", None).expect("clear");
        assert!(loaded.objective().is_none());
    }

    #[test]
    fn conversation_compaction_cannot_erase_the_persistent_goal() {
        let mut session = core_types::Session::new();
        session.session_goal = Some("ship the refactor".to_string());
        for index in 0..8 {
            session
                .push_user_text(format!("long message {index} {}", "x".repeat(200)))
                .expect("message");
        }
        let compacted = runtime::compact_session(
            &session,
            runtime::CompactionConfig {
                preserve_recent_messages: 1,
                max_estimated_tokens: 1,
            },
        );
        assert!(compacted.removed_message_count > 0);
        assert_eq!(
            compacted.compacted_session.session_goal.as_deref(),
            Some("ship the refactor")
        );
    }

    #[test]
    fn workspace_fingerprint_changes_with_contents_not_just_file_names() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("a.txt");
        fs::write(&path, "one").expect("write one");
        let one = workspace_fingerprint(temp.path()).expect("fingerprint one");
        fs::write(&path, "two").expect("write two");
        let two = workspace_fingerprint(temp.path()).expect("fingerprint two");
        assert_ne!(one, two);
    }
}
