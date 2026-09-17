//! Running one sub-agent as a PANE instead of as a thread.
//!
//! ## The one seam
//!
//! Everything a sub-agent is — its manifest, its id, its live row in the HUD,
//! the JSON its parent's tool call returns — is built by
//! [`super::execute_agent_with_spawn_and_parent_model_and_hooks`], which ends
//! by handing an [`AgentJob`] to a spawn function. In-process, that function
//! is [`super::spawn::spawn_agent_job`], a thread. Here it is a pane: a
//! `tmux split-window` running `zo --teammate <dir>`, a `result.json` waited
//! for, and the same [`AgentCompletion`] published at the end.
//!
//! Choosing between them is therefore the ONLY difference between the two
//! worlds, and it is one `match` in one function. Nothing downstream — not
//! `wait_for_agent_completions`, not the tool-result JSON, not the agents
//! viewer — can tell which one ran, which is the point: a person turning
//! panes off must get the behavior they had before, not a second
//! implementation of it.
//!
//! ## What a pane child is, since t-2513
//!
//! The SAME agent its parent would have run inline (`docs/design/
//! zo-teammate-lifecycle-contract.md`). The brief carries the harness the
//! parent resolved (`AgentJob::harness`), so the child never re-derives one;
//! the child stays alive after its first turn and takes the parent's next
//! words over its own events channel (`session.steer`), writing
//! `result-<n>.json` per turn; and a resume of a closed pane re-cuts one on
//! the same transcript. Grandchildren are still refused (`--no-spawn`).

use std::path::{Path, PathBuf};

use runtime::subagent_panes::{
    channel_method, Brief, CHANNEL_FILE, ChannelCoordinates, Exit, Limits, PaneOutcome,
    PROTOCOL_VERSION, SplitSpec, SteerOutcome, TeammateResult, Tmux, steer_over_channel,
    teammate_program, wait_for_turn_result,
};

use super::completion::AgentCompletion;
use super::spawn::{effort_word, AgentJob};
use crate::ToolError;

/// Start one sub-agent in a pane of the leader's own.
///
/// Returns as soon as the pane exists, exactly like the thread spawner: the
/// waiting happens on a thread of this process, and the completion is
/// published through the same channel, so a blocking `Agent` call and a
/// detached one both behave the way they already do.
pub(super) fn spawn_pane_job(job: AgentJob) -> Result<(), ToolError> {
    spawn_pane_job_with(&Tmux::on_path(), job)
}

/// [`spawn_pane_job`] against a named multiplexer.
///
/// The test seam, in the shape this crate already uses for spawning
/// (`execute_agent_with_spawn`): production resolves `tmux` off `PATH`, where
/// the window's shim is, and a test hands over a script that records what it
/// was asked and answers the way that shim would.
fn spawn_pane_job_with(tmux: &Tmux, job: AgentJob) -> Result<(), ToolError> {
    cut_pane_for(tmux, job, None)
}

/// Cut a pane for `job` and sit with it. `first_turn` is `None` for a fresh
/// child (turn 1) and the parent's next turn number for a child re-cut to
/// continue a transcript (`resume_pane_job`).
fn cut_pane_for(tmux: &Tmux, job: AgentJob, first_turn: Option<u32>) -> Result<(), ToolError> {
    let directory = child_directory(&job.manifest)?;
    std::fs::create_dir_all(&directory).map_err(|error| {
        ToolError::Execution(format!(
            "could not make the teammate's directory {}: {error}",
            directory.display()
        ))
    })?;
    let turn = first_turn.unwrap_or(1);
    // A re-cut child starts clean: the closing document and channel file of
    // the child that left would otherwise be read as this one's.
    if first_turn.is_some() {
        let _ = std::fs::remove_file(directory.join(runtime::subagent_panes::RESULT_FINAL_FILE));
        let _ = std::fs::remove_file(directory.join(CHANNEL_FILE));
    }
    let mut brief = brief_for(&job);
    brief.first_turn = first_turn;
    brief.write(&directory).map_err(|error| {
        ToolError::Execution(format!("could not write the teammate's brief: {error}"))
    })?;

    let program = teammate_program();
    let pane = tmux
        .split(&SplitSpec {
            directory: &directory,
            // The same id the `SubagentStart` hook already named: the window
            // folds the hook's helper row and this pane's row into one by it.
            agent_id: &job.manifest.agent_id,
            program: &program,
            model: job.manifest.resolved_model.as_deref(),
            effort: job.route_effort.map(effort_word),
            wave_index: wave_index_of(&job),
            resume_transcript: brief.resume_transcript.as_deref(),
        })
        .map_err(|error| {
            // Loud rather than quietly inline. Running the child in-process
            // after promising a pane would hide the cost of the retry AND
            // leave the person looking at a screen that never splits.
            ToolError::Execution(format!(
                "could not open a pane for this sub-agent: {error}"
            ))
        })?;
    super::manifest::stamp_agent_pane(&job.manifest, &pane);
    super::manifest::stamp_agent_execution(&job.manifest, super::EXECUTION_PANE);

    let thread_name = format!("zo-pane-{}", job.manifest.agent_id);
    let tmux = tmux.clone();
    std::thread::Builder::new()
        .name(thread_name)
        .spawn(move || watch_pane(&job, &tmux, &directory, &pane, turn))
        .map(|_| ())
        .map_err(|error| ToolError::Execution(error.to_string()))
}

/// Continue a pane child on its next turn (t-2513 contract 4).
///
/// The child is still in its pane, idle, if its channel answers: the
/// follow-up goes in through `session.steer` and opens its next turn, and
/// this process watches for that turn's result the way it watched the first.
/// If the pane is gone, a new one is cut on the SAME transcript
/// (`--resume-transcript`), so the second turn has the first in its context
/// either way. A thread is never substituted for a pane here — the mode is
/// the manifest's, and switching it is an explicit ask, not a fallback.
pub(super) fn resume_pane_job(job: AgentJob) -> Result<(), ToolError> {
    resume_pane_job_with(&Tmux::on_path(), job)
}

fn resume_pane_job_with(tmux: &Tmux, job: AgentJob) -> Result<(), ToolError> {
    let directory = child_directory(&job.manifest)?;
    let turn = turn_of_generation(job.manifest.run_generation);
    let limits = Limits::load();
    let channel_file = directory.join(CHANNEL_FILE);
    if channel_file.is_file() {
        let outcome = steer_over_channel(&channel_file, &job.prompt, limits.channel_timeout);
        record_receipt(&job.manifest, &outcome);
        if outcome.delivered() {
            let Some(pane) = job.manifest.pane.clone() else {
                return Err(ToolError::Execution(
                    "the pane child answered its channel but its manifest names no pane".to_string(),
                ));
            };
            let thread_name = format!("zo-pane-{}", job.manifest.agent_id);
            let tmux = tmux.clone();
            return std::thread::Builder::new()
                .name(thread_name)
                .spawn(move || watch_pane(&job, &tmux, &directory, &pane, turn))
                .map(|_| ())
                .map_err(|error| ToolError::Execution(error.to_string()));
        }
    }
    // Nobody answered: the pane closed (idle budget, parent restart, a
    // person). Cut a new one on the same transcript.
    let mut job = job;
    job.resume = true;
    if job.transcript_path.is_none() {
        return Err(ToolError::Execution(
            "the pane child is gone and its manifest names no transcript to continue".to_string(),
        ));
    }
    cut_pane_for(tmux, job, Some(turn))
}

/// The child's turn number for a manifest generation: the first run
/// (`AGENT_INITIAL_RUN_GENERATION`) wrote `result.json`, and every resume
/// advances the generation by one and the turn by one. One axis, so the
/// parent waits on the file the child actually writes.
fn turn_of_generation(run_generation: u64) -> u32 {
    let offset = run_generation.saturating_sub(super::AGENT_INITIAL_RUN_GENERATION);
    u32::try_from(offset).unwrap_or(u32::MAX - 1).saturating_add(1)
}

/// Write the receipt of a message to a pane child onto its manifest — the
/// same `lastReceipt` the inline observer stamps when a boundary reads.
fn record_receipt(manifest: &super::AgentOutput, outcome: &SteerOutcome) {
    super::manifest::stamp_agent_receipt(
        manifest,
        runtime::subagent_panes::SteerReceiptRecord::now(outcome),
    );
}

/// Ask a pane child to leave the polite way: stop its turn, then close it.
///
/// Both over its channel; `kill-pane` is the caller's last resort after
/// this, never the first move (design §2.2). A child with no channel file
/// yet — it has not booted — has nothing to be asked, and the caller's kill
/// is the only road. Answers whether the child took the door: `false` for a
/// missing or stale channel, so a caller can say "unreachable" instead of
/// "closed". `reason` is the word the child's closing document keeps.
pub(super) fn close_pane_child(
    directory: &Path,
    limits: &Limits,
    reason: runtime::subagent_panes::CloseReason,
) -> bool {
    let Ok(coordinates) = ChannelCoordinates::read(&directory.join(CHANNEL_FILE)) else {
        return false;
    };
    let _ = coordinates.call(
        channel_method::CANCEL_TURN,
        serde_json::json!({}),
        limits.channel_timeout,
    );
    let closed = coordinates.call(
        channel_method::TEAMMATE_CLOSE,
        serde_json::json!({ "reason": reason.as_str() }),
        limits.channel_timeout,
    );
    if closed.is_err() {
        return false;
    }
    // Give the child its moment to write `result-final.json` before the
    // pane goes; a child that ignored the door is killed all the same.
    let deadline = std::time::Instant::now() + limits.close_grace;
    while std::time::Instant::now() < deadline {
        if TeammateResult::read_final(directory).is_some() {
            break;
        }
        std::thread::sleep(limits.result_poll.min(std::time::Duration::from_millis(50)));
    }
    true
}

/// The parent's release of an IDLE pane child — `StopAgent` on a teammate
/// that finished its turn and waits in its pane for the next word.
///
/// Its manifest reads `completed`, which is terminal for an inline child, so
/// the stop used to answer "already finished" and the pane stood until the
/// idle budget ran out ("다시 필요 없음 닫는 기능까지", 2026-09-06). A child
/// whose channel file is there and whose `result-final.json` is not is asked
/// to leave over that channel: `Some(true)` when it answered, `Some(false)`
/// when the channel is stale, `None` when this is not an idle pane child at
/// all — an inline agent, or a pane that already left.
pub(super) fn release_idle_pane_child(manifest: &super::AgentOutput) -> Option<bool> {
    if manifest.lifecycle.execution.as_deref() != Some(super::EXECUTION_PANE) {
        return None;
    }
    let directory = child_directory(manifest).ok()?;
    if !directory.join(CHANNEL_FILE).is_file() || TeammateResult::read_final(&directory).is_some() {
        return None;
    }
    Some(close_pane_child(
        &directory,
        &Limits::load(),
        runtime::subagent_panes::CloseReason::ClosedByParent,
    ))
}

/// Where one child's `brief.json`, `harness.json`, `channel.addr` and
/// `result*.json` live.
///
/// A directory named for the agent, beside the manifests rather than inside
/// one: the store's readers keep only `*.json` FILES, so a directory here is
/// invisible to the HUD scan and to the orphan reaper. Inline children get
/// the directory too, for their stored harness.
pub(super) fn child_directory(child: &super::AgentOutput) -> Result<PathBuf, ToolError> {
    child_directory_in(&child_store(child)?, &child.agent_id)
}

/// The store a child's manifest was written into — its own path's parent,
/// which is the session registry's root (or an adopted mirror), never a
/// directory re-derived from the process cwd.
fn child_store(child: &super::AgentOutput) -> Result<PathBuf, ToolError> {
    Path::new(&child.manifest_file)
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            ToolError::Execution(format!(
                "`{}` is not a manifest path a store can be read from",
                child.manifest_file
            ))
        })
}

fn child_directory_in(store: &Path, agent_id: &str) -> Result<PathBuf, ToolError> {
    // The id is minted by `make_agent_id` (`agent-<nanos>`), never by a model.
    // Refuse anything that could climb out of the store all the same — this is
    // the path a process is about to be started in.
    if agent_id.is_empty()
        || !agent_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return Err(ToolError::Execution(format!(
            "`{agent_id}` is not an agent id a directory can be named for"
        )));
    }
    Ok(store.join(agent_id))
}

/// The brief a child reads: the v1 fields as before, and — generation 2 —
/// the harness the parent resolved, verbatim. The v1 fields that the harness
/// also carries (`model`, `effort`, `permission_mode`, `cwd`) are spelled
/// from the same harness, so the two can never disagree.
pub(super) fn brief_for(job: &AgentJob) -> Brief {
    let harness = job.harness();
    let limits = Limits::load();
    Brief {
        version: PROTOCOL_VERSION,
        agent_id: job.manifest.agent_id.clone(),
        prompt: job.prompt.clone(),
        description: job.manifest.description.clone(),
        subagent_type: job.manifest.subagent_type.clone(),
        name: Some(job.manifest.name.clone()),
        model: harness.model.effective.clone(),
        effort: harness.effort.clone(),
        permission_mode: harness.permission_mode.clone(),
        cwd: harness.cwd.clone(),
        parent_session: job.manifest.parent_session_id.clone(),
        tool_call_id: job.manifest.tool_call_id.clone(),
        wave_index: wave_index_of(job),
        registry_locator: harness.registry_locator.clone(),
        // A resume carries the transcript to continue; a fresh spawn none.
        resume_transcript: job.resume.then(|| job.transcript_path.clone()).flatten(),
        parent_channel: runtime::subagent_panes::parent_channel(),
        idle_budget_ms: Some(u64::try_from(limits.idle_budget.as_millis()).unwrap_or(u64::MAX)),
        first_turn: None,
        harness_stale: false,
        harness: Some(harness),
    }
}

/// Which child of the current wave this is, counted by the siblings already
/// running for this parent session.
///
/// The first cuts the leader side by side and the rest stack under it, which
/// is the layout a person recognizes — one leader, a column of helpers. The
/// count is of LIVE siblings rather than of everything ever spawned, so a
/// second wave after the first finished starts over at `-h` instead of
/// stacking onto a column that is no longer there.
///
/// Counted as "siblings that started BEFORE me", never as "siblings alive
/// right now", and the difference is the whole correction. The `Agent` calls
/// of one message are spawned side by side, so a live count is symmetric: each
/// child sees the other and both answer the same number. Measured twice in the
/// live window — first both children answered 0 and cut the leader twice
/// (their panes were not stamped yet), then both answered 1. An agent id is
/// `agent-<nanos>`, minted in spawn order, so ordering by it is the one answer
/// that does not depend on which thread got scheduled first.
fn wave_index_of(job: &AgentJob) -> usize {
    let stores = job.registry.as_ref().map_or_else(
        || child_store(&job.manifest).into_iter().collect(),
        |registry| registry.stores(),
    );
    siblings_started_before(&stores, job.manifest.parent_session_id.as_deref(), &job.manifest.agent_id)
}

fn siblings_started_before(stores: &[PathBuf], parent_session_id: Option<&str>, mine: &str) -> usize {
    /// `agent-<nanos>` compared as a NUMBER would be: a longer id is a later
    /// one, and only then does the text decide. Plain `<` on the strings would
    /// put `agent-9…` after `agent-10…` the day the clock gains a digit.
    fn order(id: &str) -> (usize, &str) {
        (id.len(), id)
    }

    stores
        .iter()
        .filter_map(|store| std::fs::read_dir(store).ok())
        .flatten()
        .flatten()
        .filter(|entry| {
            entry.path().extension().and_then(std::ffi::OsStr::to_str) == Some("json")
                && entry.file_type().is_ok_and(|kind| kind.is_file())
        })
        .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
        .filter_map(|text| serde_json::from_str::<super::AgentOutput>(&text).ok())
        .filter(|manifest| {
            manifest.status == "running"
                && order(&manifest.agent_id) < order(mine)
                && manifest.parent_session_id.as_deref() == parent_session_id
        })
        .count()
}

/// Sit with one pane until it answers turn `turn`, then publish what it said.
///
/// While waiting, the child's channel file is copied onto the manifest the
/// moment it appears, so a `SendMessage` mid-turn can reach the child and a
/// window can find its channel — the child boots after the pane is cut, so
/// this is the first place the parent can learn where it listens.
fn watch_pane(job: &AgentJob, tmux: &Tmux, directory: &std::path::Path, pane: &str, turn: u32) {
    let _registration = super::spawn::pane_worker_registration(job);
    let limits = Limits::load();
    let budget = job.time_budget.unwrap_or(limits.pane_budget);
    let channel_seen = std::cell::Cell::new(job.manifest.lifecycle.channel.is_some());
    let cancelled = || {
        if !channel_seen.get() && directory.join(CHANNEL_FILE).is_file() {
            channel_seen.set(true);
            let channel = directory.join(CHANNEL_FILE);
            super::manifest::stamp_agent_lifecycle(&job.manifest, |lifecycle| {
                lifecycle.channel = Some(channel);
            });
        }
        job.cancel_signal.is_aborted()
    };
    let close = || {
        close_pane_child(
            directory,
            &limits,
            runtime::subagent_panes::CloseReason::ClosedByParent,
        );
    };
    let outcome = wait_for_turn_result(tmux, directory, pane, turn, budget, &cancelled, &close);
    // A lane's answer is the last thing asked of it: once it is read, the
    // pane is released — after the completion is published, so the parent's
    // collection never waits on the door (`close_grace`).
    let lane_done = job.one_shot && matches!(outcome, PaneOutcome::Finished(_));
    let transcript = directory.join(runtime::subagent_panes::result_file_for_turn(turn));
    let completion = match outcome {
        PaneOutcome::Finished(result) => finished(job, &result),
        // The child left before answering: a stopped run, with the child's
        // own reason. Its transcript is still on the manifest for a resume.
        PaneOutcome::Closed(result) => settled(
            job,
            "stopped",
            None,
            Some(format!(
                "the sub-agent's pane closed before answering ({})",
                result
                    .reason
                    .map_or("no reason given", runtime::subagent_panes::CloseReason::as_str)
            )),
        ),
        PaneOutcome::Cancelled => settled(
            job,
            "stopped",
            None,
            Some("the parent's turn was cancelled, so this pane was closed".to_string()),
        ),
        PaneOutcome::TimedOut => settled(
            job,
            "failed",
            None,
            Some(format!(
                "the sub-agent's pane wrote no result within {} seconds; its pane was closed",
                budget.as_secs()
            )),
        ),
        // The one case that has to name a path. A child that died without
        // writing is a bug in something, and the only place left to look is
        // its own transcript — which is on screen too, in a pane the window
        // keeps until somebody closes it.
        PaneOutcome::Vanished => settled(
            job,
            "failed",
            None,
            Some(format!(
                "the sub-agent's pane closed without writing a result; its transcript is beside {}",
                transcript.display()
            )),
        ),
    };
    super::spawn::publish_pane_completion(job, completion);
    if lane_done {
        close_pane_child(
            directory,
            &limits,
            runtime::subagent_panes::CloseReason::LaneDone,
        );
    }
}

fn finished(job: &AgentJob, result: &TeammateResult) -> AgentCompletion {
    let (status, error) = match result.exit {
        Exit::Ok => ("completed", None),
        Exit::Error => (
            "failed",
            Some(
                result
                    .error
                    .clone()
                    .unwrap_or_else(|| "the sub-agent ended with an error".to_string()),
            ),
        ),
        Exit::Cancelled | Exit::Closed => (
            "stopped",
            Some(
                result
                    .error
                    .clone()
                    .unwrap_or_else(|| "the sub-agent was stopped in its own pane".to_string()),
            ),
        ),
    };
    // The child's own transcript goes on the manifest FIRST, so the terminal
    // write below and every later resume see it (design §2.4: `transcript`
    // is copied from the result, and `transcript_path_for` prefers it).
    if let Some(transcript) = result.transcript.clone() {
        super::manifest::stamp_agent_lifecycle(&job.manifest, |lifecycle| {
            lifecycle.transcript = Some(transcript);
        });
    }
    let mut completion = settled(
        job,
        status,
        (!result.final_message.trim().is_empty()).then(|| result.final_message.clone()),
        error,
    );
    completion.run.output_tokens = result.usage.output_tokens;
    completion.run.tool_calls = result.usage.tool_calls;
    completion
}

/// Write the terminal manifest and build the completion, in that order.
///
/// The manifest first because it is what every READER of the store believes —
/// the HUD's live rows, the orphan reaper, a later `GetAgentCompletion`. A
/// completion published against a manifest still saying `running` is the
/// signature of an agent that "finished" and stayed on screen forever.
fn settled(
    job: &AgentJob,
    status: &str,
    result: Option<String>,
    error: Option<String>,
) -> AgentCompletion {
    let _ = super::manifest::persist_agent_terminal_state(
        &job.manifest,
        status,
        result.as_deref(),
        error.clone(),
    );
    AgentCompletion {
        agent_id: job.manifest.agent_id.clone(),
        name: job.manifest.name.clone(),
        status: status.to_string(),
        result,
        structured: None,
        error,
        run: core_types::helper_run::HelperRun::default(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::misc_tools::agent_tools::{AgentInput, execute_agent_with_spawn_and_parent_model};
    use runtime::subagent_panes::{Brief, ChannelCoordinates, CHANNEL_FILE};

    /// A store nobody else is writing into, and the process env pointed at it.
    ///
    /// The store is process-global (`ZO_AGENT_STORE`), so the crate-wide env
    /// lock is what keeps two of these apart.
    struct Isolated {
        store: PathBuf,
        _lock: std::sync::MutexGuard<'static, ()>,
        prior: Option<std::ffi::OsString>,
        /// Whether this test pointed the custom-agent search at its own dir,
        /// and what the variable said before it did.
        defs_pointed: bool,
        prior_defs: Option<std::ffi::OsString>,
    }

    const CUSTOM_AGENT_DEFS_ENV: &str = "ZO_AGENT_DEFS_DIR";

    impl Isolated {
        fn new(name: &str) -> Self {
            let lock = crate::tests::env_lock()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let store = std::env::temp_dir().join(format!(
                "zo-panes-{name}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&store).expect("store");
            let prior = std::env::var_os(super::super::labels::AGENT_STORE_ENV);
            std::env::set_var(super::super::labels::AGENT_STORE_ENV, &store);
            Self {
                store,
                _lock: lock,
                prior,
                defs_pointed: false,
                prior_defs: None,
            }
        }

        /// Custom agent definitions this test alone can see.
        fn with_custom_agents(mut self, agents: &[(&str, &str)]) -> Self {
            let defs = self.store.join("defs");
            std::fs::create_dir_all(&defs).expect("defs dir");
            for (name, contents) in agents {
                std::fs::write(defs.join(format!("{name}.md")), contents).expect("write definition");
            }
            self.defs_pointed = true;
            self.prior_defs = std::env::var_os(CUSTOM_AGENT_DEFS_ENV);
            std::env::set_var(CUSTOM_AGENT_DEFS_ENV, &defs);
            self
        }

        /// The manifest as the store has it now.
        fn manifest(&self, agent_id: &str) -> super::super::AgentOutput {
            serde_json::from_str(
                &std::fs::read_to_string(self.store.join(format!("{agent_id}.json"))).expect("manifest"),
            )
            .expect("parse manifest")
        }

        /// Poll until the watcher has settled `agent_id` on generation
        /// `generation`, and answer the settled manifest.
        fn wait_settled(&self, agent_id: &str, generation: u64) -> super::super::AgentOutput {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                let stored = self.manifest(agent_id);
                if stored.run_generation == generation
                    && super::super::agent_output_status_is_terminal(&stored.status)
                {
                    return stored;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "the watcher never settled {agent_id} on generation {generation}: {stored:?}"
                );
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    }

    impl Drop for Isolated {
        fn drop(&mut self) {
            match self.prior.take() {
                Some(value) => std::env::set_var(super::super::labels::AGENT_STORE_ENV, value),
                None => std::env::remove_var(super::super::labels::AGENT_STORE_ENV),
            }
            if self.defs_pointed {
                match self.prior_defs.take() {
                    Some(value) => std::env::set_var(CUSTOM_AGENT_DEFS_ENV, value),
                    None => std::env::remove_var(CUSTOM_AGENT_DEFS_ENV),
                }
            }
            let _ = std::fs::remove_dir_all(&self.store);
        }
    }

    /// One MCP tool as the parent session advertises it.
    fn mcp_definition(name: &str) -> crate::registry::RuntimeToolDefinition {
        crate::registry::RuntimeToolDefinition {
            name: name.to_string(),
            description: Some(format!("{name} docs")),
            input_schema: serde_json::json!({"type": "object", "properties": {"q": {"type": "string"}}}),
            required_permission: runtime::PermissionMode::Prompt,
        }
    }

    /// Two accepts — the parent's `session.cancel_turn`, then
    /// `teammate.close` — answered the way an idle child answers them, and
    /// the child's `result-final.json` written as it leaves.
    fn closing_child_channel(
        channel_file: &Path,
        directory: PathBuf,
        agent_id: String,
    ) -> std::thread::JoinHandle<Vec<String>> {
        use std::io::{BufRead as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr").to_string();
        ChannelCoordinates {
            addr,
            token: Some("child-token".to_string()),
            session_id: "child-session".to_string(),
        }
        .write(channel_file)
        .expect("write channel file");
        // Bounded: a parent that never calls leaves the methods empty instead
        // of parking the test on `accept` forever.
        listener.set_nonblocking(true).expect("nonblocking");
        std::thread::spawn(move || {
            let mut methods = Vec::new();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            for _ in 0..2 {
                let stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break Some(stream),
                        Err(_) if std::time::Instant::now() < deadline => {
                            std::thread::sleep(std::time::Duration::from_millis(20));
                        }
                        Err(_) => break None,
                    }
                };
                let Some(stream) = stream else {
                    break;
                };
                stream.set_nonblocking(false).expect("blocking stream");
                let mut reader = std::io::BufReader::new(stream.try_clone().expect("clone"));
                let mut line = String::new();
                let _ = reader.read_line(&mut line);
                let request: serde_json::Value = serde_json::from_str(line.trim()).expect("json");
                let method = request["method"].as_str().unwrap_or_default().to_string();
                let result = if method == runtime::subagent_panes::channel_method::TEAMMATE_CLOSE {
                    // The closing document keeps the parent's word, as the
                    // real child's does (`close_reason_from`).
                    let reason = serde_json::from_value(request["params"]["reason"].clone())
                        .unwrap_or(runtime::subagent_panes::CloseReason::ClosedByParent);
                    TeammateResult::closed(&agent_id, reason)
                        .write_final(&directory)
                        .expect("write final");
                    serde_json::json!({ "closing": true })
                } else {
                    serde_json::json!({ "cancelled": false })
                };
                methods.push(method);
                let mut writer = stream;
                let response = serde_json::json!({
                    "jsonrpc": "2.0", "id": request["id"], "result": result
                });
                let _ = writeln!(writer, "{response}");
            }
            methods
        })
    }

    /// `StopAgent` on an idle pane child (t-2513 §2.2, the release the
    /// contract left to the parent): the child's manifest reads `completed`
    /// — its last turn's word — yet its pane stands with a live channel, so
    /// the stop asks it over that channel to leave (cancel, then close),
    /// waits for its `result-final.json`, and reports `closed` rather than
    /// "already finished". A pane that already left is not this road's.
    #[test]
    fn stopping_an_idle_pane_child_releases_it_over_its_channel() {
        let isolated = Isolated::new("release");
        let tmux = fake_tmux(&isolated.store, "'%1' '%4'");
        let manifest = execute_agent_with_spawn_and_parent_model(
            an_agent("first"),
            |job| spawn_pane_job_with(&tmux, job),
            None,
            None,
        )
        .expect("spawn");
        let directory = isolated.store.join(&manifest.agent_id);
        let mut said = TeammateResult::new(&manifest.agent_id, Exit::Ok);
        said.final_message = "first answer".to_string();
        said.write_turn(&directory, 1).expect("write turn 1");
        let settled = isolated.wait_settled(&manifest.agent_id, 1);
        assert_eq!(settled.status, "completed", "precondition: the first turn settled");
        assert_eq!(settled.lifecycle.execution.as_deref(), Some(super::super::EXECUTION_PANE));

        let served = closing_child_channel(
            &directory.join(CHANNEL_FILE),
            directory.clone(),
            manifest.agent_id.clone(),
        );
        let outcome = super::super::stop_agent_for_session_in(
            &isolated.store,
            &manifest.agent_id,
            "session-pane-test",
            "no longer needed",
            false,
        );
        assert!(
            matches!(outcome, super::super::AgentStopOutcome::Closed { .. }),
            "an idle pane child is released, not 'already finished': {outcome:?}"
        );
        let asked = served.join().expect("child channel");
        assert_eq!(
            asked,
            vec![
                runtime::subagent_panes::channel_method::CANCEL_TURN.to_string(),
                runtime::subagent_panes::channel_method::TEAMMATE_CLOSE.to_string(),
            ],
            "cancel first, then the door"
        );
        let closing = TeammateResult::read_final(&directory).expect("the child wrote its final result");
        assert_eq!(closing.reason, Some(runtime::subagent_panes::CloseReason::ClosedByParent));
        let stored = isolated.manifest(&manifest.agent_id);
        assert_eq!(stored.lifecycle.released.as_deref(), Some("no longer needed"));

        // The pane already left: nothing to release, and the stop answers as
        // it always did for a finished agent.
        let again = super::super::stop_agent_for_session_in(
            &isolated.store,
            &manifest.agent_id,
            "session-pane-test",
            "again",
            false,
        );
        assert!(
            matches!(again, super::super::AgentStopOutcome::AlreadyFinished { .. }),
            "{again:?}"
        );
        super::super::clear_background_agent(&manifest.agent_id);
    }

    /// A channel nobody should call: answers whether anyone connected within
    /// `quiet`. The teammate's control — a pane that must be left standing.
    fn silent_child_channel(
        channel_file: &Path,
        quiet: std::time::Duration,
    ) -> std::thread::JoinHandle<bool> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.set_nonblocking(true).expect("nonblocking");
        let addr = listener.local_addr().expect("addr").to_string();
        ChannelCoordinates {
            addr,
            token: Some("child-token".to_string()),
            session_id: "child-session".to_string(),
        }
        .write(channel_file)
        .expect("write channel file");
        std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + quiet;
            while std::time::Instant::now() < deadline {
                if listener.accept().is_ok() {
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            false
        })
    }

    /// A fan-out lane leaves the moment its answer is read (2026-09-07,
    /// "조사가 끝나면 자동으로 닫혔으면"): the parent asks it over its channel —
    /// cancel, then the door, with `lane_done` as the word — without any
    /// `StopAgent`, and the closing document keeps that word. A teammate
    /// (a lone `Agent`) spawned the same way is not asked anything: its
    /// pane stands idle for the parent's next word, as t-2513 promised.
    #[test]
    fn a_fanout_lane_leaves_when_its_answer_is_read_and_a_teammate_stays() {
        let isolated = Isolated::new("lane");
        let tmux = fake_tmux(&isolated.store, "'%1' '%4'");
        let mut lane = an_agent("lane one");
        lane.one_shot = true;
        let lane = execute_agent_with_spawn_and_parent_model(
            lane,
            |job| spawn_pane_job_with(&tmux, job),
            None,
            None,
        )
        .expect("spawn the lane");
        let teammate = execute_agent_with_spawn_and_parent_model(
            an_agent("stay with me"),
            |job| spawn_pane_job_with(&tmux, job),
            None,
            None,
        )
        .expect("spawn the teammate");
        let lane_dir = isolated.store.join(&lane.agent_id);
        let teammate_dir = isolated.store.join(&teammate.agent_id);
        // Both children's channels are up before they answer, as a real
        // child's is from boot.
        let lane_channel =
            closing_child_channel(&lane_dir.join(CHANNEL_FILE), lane_dir.clone(), lane.agent_id.clone());
        let teammate_channel = silent_child_channel(
            &teammate_dir.join(CHANNEL_FILE),
            std::time::Duration::from_millis(600),
        );
        for (agent, directory) in [(&lane, &lane_dir), (&teammate, &teammate_dir)] {
            let mut said = TeammateResult::new(&agent.agent_id, Exit::Ok);
            said.final_message = "my answer".to_string();
            said.write_turn(directory, 1).expect("write turn 1");
        }
        let lane_settled = isolated.wait_settled(&lane.agent_id, 1);
        assert_eq!(lane_settled.status, "completed", "the lane's answer settled first: {lane_settled:?}");

        let asked = lane_channel.join().expect("lane channel");
        assert_eq!(
            asked,
            vec![
                runtime::subagent_panes::channel_method::CANCEL_TURN.to_string(),
                runtime::subagent_panes::channel_method::TEAMMATE_CLOSE.to_string(),
            ],
            "the lane is asked to leave on the read of its answer: cancel, then the door"
        );
        let closing = TeammateResult::read_final(&lane_dir).expect("the lane wrote its closing document");
        assert_eq!(closing.reason, Some(runtime::subagent_panes::CloseReason::LaneDone));

        let teammate_settled = isolated.wait_settled(&teammate.agent_id, 1);
        assert_eq!(teammate_settled.status, "completed");
        assert!(
            !teammate_channel.join().expect("teammate channel"),
            "a teammate's pane is left standing for the parent's next word"
        );
        assert!(TeammateResult::read_final(&teammate_dir).is_none(), "the teammate wrote no closing document");
        super::super::clear_background_agent(&lane.agent_id);
        super::super::clear_background_agent(&teammate.agent_id);
    }

    /// One accept, one answer to `session.steer`, then gone — a child that is
    /// idle in its pane and opens its next turn with the words.
    fn idle_child_channel(channel_file: &Path) -> std::thread::JoinHandle<Vec<serde_json::Value>> {
        use std::io::{BufRead as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr").to_string();
        ChannelCoordinates {
            addr,
            token: Some("child-token".to_string()),
            session_id: "child-session".to_string(),
        }
        .write(channel_file)
        .expect("write channel file");
        std::thread::spawn(move || {
            let mut seen = Vec::new();
            let Ok((stream, _)) = listener.accept() else {
                return seen;
            };
            let mut reader = std::io::BufReader::new(stream.try_clone().expect("clone"));
            let mut line = String::new();
            let _ = reader.read_line(&mut line);
            let request: serde_json::Value = serde_json::from_str(line.trim()).expect("json");
            seen.push(request.clone());
            let mut writer = stream;
            let response = serde_json::json!({
                "jsonrpc": "2.0", "id": request["id"],
                "result": {"turn_id": null, "receipt": "queued"}
            });
            let _ = writeln!(writer, "{response}");
            seen
        })
    }

    /// The manifest before the executor, the resume files after (t-2902).
    ///
    /// What a crash between the child's first provider request and the
    /// materialisation of its resume files would leave behind: the manifest
    /// is on disk — and the registry reports it, `running`, already naming
    /// the harness by digest — from the instant the executor is handed the
    /// job, while `harness.json` and `<id>.resume.json` are not there yet.
    /// They land once the executor has returned, and the stored harness is
    /// the digest the manifest carries. On the axis-A lane (t-2877) the two
    /// files used to be written at +32 ms, ahead of the child's first
    /// request at +71 ms; they serve a later resume, not the child's start.
    #[test]
    fn the_manifest_is_reported_before_the_executor_runs_and_the_resume_files_land_after() {
        let isolated = Isolated::new("order");
        let store = isolated.store.clone();
        let mut seen_at_spawn = None;
        let manifest = execute_agent_with_spawn_and_parent_model(
            an_agent("what does the store hold right now?"),
            |job| {
                let registry = job
                    .registry
                    .as_ref()
                    .expect("the session registry rides the job");
                let directory = store.join(&job.manifest.agent_id);
                seen_at_spawn = Some((
                    registry.manifest_by_id(&job.manifest.agent_id),
                    runtime::subagent_panes::ResolvedHarness::path_in(&directory).exists(),
                    store
                        .join(format!("{}.resume.json", job.manifest.agent_id))
                        .exists(),
                    job.harness().digest(),
                ));
                Ok(())
            },
            None,
            None,
        )
        .expect("spawn");
        let (reported, harness_there, snapshot_there, digest) =
            seen_at_spawn.expect("the executor ran");
        let reported =
            reported.expect("the registry reports the child while the executor still runs");
        assert_eq!(reported.status, "running");
        assert_eq!(
            reported.lifecycle.harness_digest.as_deref(),
            Some(digest.as_str()),
            "the manifest names the harness before the harness file exists"
        );
        assert!(
            !harness_there,
            "harness.json is materialised only after the executor holds the job"
        );
        assert!(
            !snapshot_there,
            "the resume snapshot is materialised only after the executor holds the job"
        );
        // Once the spawn has returned, both files are there and agree with the
        // manifest — the resume/registry guarantees stand as before.
        let on_disk = runtime::subagent_panes::ResolvedHarness::read(
            &isolated.store.join(&manifest.agent_id),
        )
        .expect("harness.json lands after the executor");
        assert_eq!(on_disk.digest(), digest, "the stored harness is the digested one");
        assert!(
            isolated
                .store
                .join(format!("{}.resume.json", manifest.agent_id))
                .is_file(),
            "the resume snapshot lands after the executor"
        );
        assert_eq!(
            isolated.manifest(&manifest.agent_id).lifecycle.harness_digest.as_deref(),
            Some(digest.as_str())
        );
    }

    /// One brief, one harness (t-2513 contract 1, design test 1).
    ///
    /// For every role — a built-in read-only one, the general-purpose
    /// default, and a custom definition with permission rules and two MCP
    /// tools — the harness the inline executor runs from (`AgentJob::harness`,
    /// which `build_agent_runtime` reads) and the harness the pane brief
    /// carries are ONE digest, and the manifest and the stored `harness.json`
    /// name that same digest. The table this prints is the report's.
    #[test]
    fn the_inline_executor_and_the_pane_brief_are_fed_one_harness_for_every_role() {
        let isolated = Isolated::new("harness").with_custom_agents(&[(
            "triage",
            "---\nname: triage\ndescription: Triage the report\n\
             tools: read_file, grep_search, mcp__ctx7__query, mcp__ctx7__fetch\n\
             permission: bash(git *)=allow, bash(rm *)=deny\npermissionMode: read-only\n---\n\
             You triage reports and never edit.",
        )]);
        let passthrough = crate::registry::McpPassthrough::for_tests(
            vec![mcp_definition("mcp__ctx7__query"), mcp_definition("mcp__ctx7__fetch")],
            std::sync::Arc::new(|_, _| Ok(String::new())),
        );
        let mut digests = Vec::new();
        for role in ["Explore", "general-purpose", "triage"] {
            let mut input = an_agent("map the edges");
            input.subagent_type = Some(role.to_string());
            input.mcp_passthrough = Some(passthrough.clone());
            let mut captured = None;
            let manifest = execute_agent_with_spawn_and_parent_model(
                input,
                |job| {
                    captured = Some(job);
                    Ok(())
                },
                None,
                None,
            )
            .expect("spawn");
            let job = captured.expect("a job");
            let inline = job.harness();
            let brief = brief_for(&job);
            let carried = brief.harness.clone().expect("the brief carries the harness");
            assert_eq!(inline.digest(), carried.digest(), "{role}: inline and pane disagree");
            assert_eq!(brief.version, PROTOCOL_VERSION);
            // The v1 fields are spelled from the same harness, never apart.
            assert_eq!(brief.permission_mode, carried.permission_mode, "{role}");
            assert_eq!(brief.model, carried.model.effective, "{role}");
            assert_eq!(brief.cwd, carried.cwd, "{role}");
            // And the disk agrees: the manifest's digest and the stored copy.
            let stored = isolated.manifest(&manifest.agent_id);
            assert_eq!(stored.lifecycle.harness_digest.as_deref(), Some(inline.digest().as_str()), "{role}");
            let on_disk = runtime::subagent_panes::ResolvedHarness::read(&isolated.store.join(&manifest.agent_id))
                .expect("harness.json beside the brief");
            assert_eq!(on_disk.digest(), inline.digest(), "{role}: harness.json drifted");
            // The MCP schemas ride every role that may call them.
            let mcp = carried.mcp.as_ref().expect("the parent's MCP route rides the harness");
            let mut names: Vec<&str> = mcp.tools.iter().map(|tool| tool.name.as_str()).collect();
            names.sort_unstable();
            assert_eq!(names, ["mcp__ctx7__fetch", "mcp__ctx7__query"], "{role}");
            assert_eq!(mcp.method, runtime::subagent_panes::channel_method::MCP_CALL);
            if role == "triage" {
                assert!(
                    carried.system_prompt.iter().any(|section| section.contains("You triage reports")),
                    "the custom role prompt did not ride the harness"
                );
                let rules = carried.permission_rules.as_ref().expect("the custom rules ride the harness");
                assert_eq!(rules.allow, ["bash(git *)".to_string()]);
                assert_eq!(rules.deny, ["bash(rm *)".to_string()]);
                assert_eq!(carried.permission_mode.as_deref(), Some("read-only"));
                assert!(carried.allowed_tools.contains("read_file") && !carried.allowed_tools.contains("bash"));
            } else {
                assert!(carried.permission_rules.is_none(), "{role}: a built-in role has no rules");
            }
            println!("harness-equivalence | {role} | inline {} | pane {}", inline.digest(), carried.digest());
            digests.push(inline.digest());
            super::super::unregister_agent_steering(&manifest.agent_id, manifest.run_generation);
        }
        digests.sort();
        digests.dedup();
        assert_eq!(digests.len(), 3, "three roles are three harnesses");
    }

    /// A resume continues a pane child IN A PANE (t-2513 contract 4): through
    /// its live channel when it is idle there — no new pane, receipt
    /// `queued`, the next turn's result watched for — and in a re-cut pane on
    /// the SAME transcript when the pane is gone, the brief saying which turn
    /// comes next. Never as a thread.
    #[test]
    fn a_pane_child_resumes_through_its_channel_or_in_a_new_pane_on_the_same_transcript() {
        let isolated = Isolated::new("resume");
        let tmux = fake_tmux(&isolated.store, "'%1' '%4'");
        let manifest = execute_agent_with_spawn_and_parent_model(
            an_agent("first"),
            |job| spawn_pane_job_with(&tmux, job),
            None,
            None,
        )
        .expect("spawn");
        let directory = isolated.store.join(&manifest.agent_id);
        // The child answers its first turn, naming its own transcript.
        let transcript = isolated.store.join("child-own.session.jsonl");
        std::fs::write(&transcript, b"{\"type\":\"session_meta\"}\n").expect("write transcript");
        let mut said = TeammateResult::new(&manifest.agent_id, Exit::Ok);
        said.final_message = "first answer".to_string();
        said.transcript = Some(transcript.clone());
        said.write_turn(&directory, 1).expect("write turn 1");
        let settled = isolated.wait_settled(&manifest.agent_id, 1);
        assert_eq!(settled.status, "completed");
        assert_eq!(settled.lifecycle.execution.as_deref(), Some(super::super::EXECUTION_PANE));
        assert_eq!(settled.lifecycle.transcript.as_deref(), Some(transcript.as_path()));
        assert_eq!(settled.pane.as_deref(), Some("%4"));
        let registry = super::super::AgentRegistry::at_root_for_tests("session-pane-test", &isolated.store);

        // (a) The child is idle in its pane: its channel answers.
        let served = idle_child_channel(&directory.join(CHANNEL_FILE));
        let resumed = super::super::resume_agent_with_spawn(
            Some(std::sync::Arc::clone(&registry)),
            &settled,
            "second question",
            None,
            None,
            None,
            None,
            false,
            |job| resume_pane_job_with(&tmux, job),
        )
        .expect("resume through the channel");
        assert_eq!(resumed.manifest.run_generation, 2);
        assert_eq!(resumed.manifest.status, "running");
        let asked_child = served.join().expect("child channel");
        assert_eq!(asked_child[0]["method"], "session.steer");
        assert_eq!(asked_child[0]["params"]["text"], "second question");
        assert_eq!(asked_child[0]["token"], "child-token");
        let splits = asked(&isolated.store).into_iter().filter(|line| line.starts_with("split-window")).count();
        assert_eq!(splits, 1, "a live child got a new pane instead of its channel");
        let stored = isolated.manifest(&manifest.agent_id);
        assert_eq!(
            stored.lifecycle.last_receipt.as_ref().map(|record| record.receipt),
            Some(runtime::subagent_panes::SteerReceipt::Queued)
        );
        // Its second answer lands in `result-2.json`, and the watcher settles
        // generation 1 on it.
        let mut second = TeammateResult::new(&manifest.agent_id, Exit::Ok);
        second.final_message = "second answer".to_string();
        second.transcript = Some(transcript.clone());
        second.write_turn(&directory, 2).expect("write turn 2");
        let settled = isolated.wait_settled(&manifest.agent_id, 2);
        assert_eq!(settled.status, "completed");
        assert!(
            std::fs::read_to_string(&settled.output_file).unwrap().contains("second answer"),
            "the second turn's answer did not reach the output"
        );
        super::super::clear_background_agent(&manifest.agent_id);

        // (b) The pane is gone (idle budget, a person, a restart): the channel
        // file is not there, so a new pane is cut on the same transcript and
        // told which turn it is answering.
        let resumed = super::super::resume_agent_with_spawn(
            Some(std::sync::Arc::clone(&registry)),
            &settled,
            "third question",
            None,
            None,
            None,
            None,
            false,
            |job| resume_pane_job_with(&tmux, job),
        )
        .expect("resume in a new pane");
        assert_eq!(resumed.manifest.run_generation, 3);
        let splits: Vec<String> = asked(&isolated.store)
            .into_iter()
            .filter(|line| line.starts_with("split-window"))
            .collect();
        assert_eq!(splits.len(), 2, "the gone pane was not re-cut");
        assert!(
            splits[1].contains(&format!("--resume-transcript {}", transcript.display())),
            "the new pane does not continue the transcript: {}",
            splits[1]
        );
        let brief = Brief::read(&directory).expect("the re-cut brief");
        assert_eq!(brief.first_turn, Some(3));
        assert_eq!(brief.resume_transcript.as_deref(), Some(transcript.as_path()));
        assert_eq!(brief.prompt, "third question");
        assert!(brief.harness.is_some(), "the re-cut child still gets the harness");
        assert!(brief.registry_locator.is_some(), "the re-cut child still opens the parent registry");
        // The third answer is turn 3 — the same axis the parent counts on.
        let mut third = TeammateResult::new(&manifest.agent_id, Exit::Ok);
        third.final_message = "third answer".to_string();
        third.write_turn(&directory, 3).expect("write turn 3");
        let settled = isolated.wait_settled(&manifest.agent_id, 3);
        assert_eq!(settled.status, "completed");
        super::super::clear_background_agent(&manifest.agent_id);
    }

    /// A pane child that leaves instead of answering settles as stopped with
    /// its own reason, and its transcript stays on the manifest for a resume.
    #[test]
    fn a_pane_child_that_closes_before_answering_is_a_stop_with_the_childs_reason() {
        let isolated = Isolated::new("closed");
        let tmux = fake_tmux(&isolated.store, "'%1' '%4'");
        let manifest = execute_agent_with_spawn_and_parent_model(
            an_agent("first"),
            |job| spawn_pane_job_with(&tmux, job),
            None,
            None,
        )
        .expect("spawn");
        let directory = isolated.store.join(&manifest.agent_id);
        TeammateResult::closed(&manifest.agent_id, runtime::subagent_panes::CloseReason::ParentLost)
            .write_final(&directory)
            .expect("write final");
        let settled = isolated.wait_settled(&manifest.agent_id, 1);
        assert_eq!(settled.status, "stopped");
        assert!(
            settled.error.as_deref().is_some_and(|why| why.contains("parent_lost")),
            "the child's reason did not reach the manifest: {:?}",
            settled.error
        );
    }

    /// The window's shim, as three lines of shell: it records the argv and
    /// answers with a pane id, which is the whole contract.
    fn fake_tmux(directory: &Path, panes: &str) -> Tmux {
        let script = directory.join("tmux");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n\
                 printf '%s\\n' \"$*\" >> {log}\n\
                 case \"$1\" in\n\
                 split-window) echo '%4' ;;\n\
                 list-panes) printf '%s\\n' {panes} ;;\n\
                 esac\n",
                log = directory.join("tmux.log").display(),
                panes = panes,
            ),
        )
        .expect("write fake tmux");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
                .expect("chmod");
        }
        Tmux::at(script)
    }

    fn asked(directory: &Path) -> Vec<String> {
        std::fs::read_to_string(directory.join("tmux.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn an_agent(prompt: &str) -> AgentInput {
        AgentInput {
            route_probe_confidence: None,
            fork_source: None,
            description: "read the graph".to_string(),
            prompt: prompt.to_string(),
            subagent_type: Some("Explore".to_string()),
            name: Some("recon-graph".to_string()),
            addressable_name: None,
            model: None,
            allow_cross_provider: false,
            cwd: None,
            schema: None,
            background: Some(false),
            workflow_member: false,
            one_shot: false,
            plan_shape: None,
            route_tax: None,
            api_concurrency: None,
            parent_permission_mode: None,
            parent_session_id: Some("session-pane-test".to_string()),
            registry: None,
            tool_call_id: Some("toolu_pane".to_string()),
            mcp_passthrough: None,
            time_budget: None,
            prior_failures: 0,
            route_reason: None,
            route_model: None,
            route_fallback_models: Vec::new(),
            route_effort: None,
            route_role: None,
            route_complexity: None,
            route_risk: None,
            route_source: None,
            judged_agent: None,
            verify_loop: None,
            sees: Vec::new(),
        }
    }

    /// The whole spawn, through the real executor: brief on disk, argv at the
    /// multiplexer, pane id back on the manifest.
    #[test]
    fn a_pane_spawn_writes_a_brief_cuts_a_pane_and_writes_the_pane_down() {
        let isolated = Isolated::new("spawn");
        let tmux = fake_tmux(&isolated.store, "'%1' '%4'");
        let manifest = execute_agent_with_spawn_and_parent_model(
            an_agent("map the edges"),
            |job| spawn_pane_job_with(&tmux, job),
            None,
            None,
        )
        .expect("spawn");

        let directory = isolated.store.join(&manifest.agent_id);
        let brief = Brief::read(&directory).expect("brief");
        assert_eq!(brief.prompt, "map the edges");
        assert_eq!(brief.agent_id, manifest.agent_id);
        assert_eq!(brief.subagent_type.as_deref(), Some("Explore"));
        assert_eq!(brief.parent_session.as_deref(), Some("session-pane-test"));
        assert_eq!(brief.tool_call_id.as_deref(), Some("toolu_pane"));
        assert_eq!(brief.wave_index, 0, "the first child cuts the leader");

        let split = asked(&isolated.store)
            .into_iter()
            .find(|line| line.starts_with("split-window"))
            .expect("nothing was split");
        assert!(split.starts_with("split-window -h -d -P -F #{pane_id} -e ZO_AGENT_ID="));
        assert!(
            split.contains(&format!("-e ZO_AGENT_ID={} -- ", manifest.agent_id)),
            "the split does not name the child by the hook's id: {split}"
        );
        assert!(split.contains("--teammate"));
        assert!(
            split.contains(&directory.display().to_string()),
            "the pane was not pointed at this child's directory: {split}"
        );

        // And the manifest says which pane, which is what the sidebar and the
        // events frame read.
        let stored: super::super::AgentOutput = serde_json::from_str(
            &std::fs::read_to_string(isolated.store.join(format!("{}.json", manifest.agent_id)))
                .expect("manifest"),
        )
        .expect("parse");
        assert_eq!(stored.pane.as_deref(), Some("%4"));
    }

    /// A second child of the same wave stacks under the first.
    #[test]
    fn a_second_live_child_stacks_under_the_first_instead_of_halving_the_leader() {
        let isolated = Isolated::new("wave");
        let tmux = fake_tmux(&isolated.store, "'%1' '%4'");
        let first = execute_agent_with_spawn_and_parent_model(
            an_agent("first"),
            |job| spawn_pane_job_with(&tmux, job),
            None,
            None,
        )
        .expect("first spawn");
        // The first is still running with a pane, so the second is its sibling.
        let second = execute_agent_with_spawn_and_parent_model(
            an_agent("second"),
            |job| spawn_pane_job_with(&tmux, job),
            None,
            None,
        )
        .expect("second spawn");
        assert_ne!(first.agent_id, second.agent_id);
        let brief = Brief::read(&isolated.store.join(&second.agent_id)).expect("brief");
        assert_eq!(brief.wave_index, 1);
        let splits: Vec<String> = asked(&isolated.store)
            .into_iter()
            .filter(|line| line.starts_with("split-window"))
            .collect();
        assert_eq!(splits.len(), 2);
        assert!(splits[0].starts_with("split-window -h"));
        assert!(
            splits[1].starts_with("split-window -v"),
            "the second child halved the leader again: {}",
            splits[1]
        );
    }

    /// What a pane child's answer becomes, in the shape the model reads.
    ///
    /// The tool result is a pure function of (manifest, completion)
    /// (`finish_blocking_agent_call`), so a completion with the same fields
    /// and the same status vocabulary IS the same tool result — which is the
    /// promise this executor makes: nothing downstream can tell a pane child
    /// from a thread.
    #[test]
    fn a_pane_childs_answer_becomes_the_completion_an_inline_child_would_have() {
        let isolated = Isolated::new("completion");
        let mut captured = None;
        let manifest = execute_agent_with_spawn_and_parent_model(
            an_agent("map the edges"),
            |job| {
                captured = Some(job);
                Ok(())
            },
            None,
            None,
        )
        .expect("spawn");
        let job = captured.expect("a job");

        let mut said = TeammateResult::new(&manifest.agent_id, Exit::Ok);
        said.final_message = "three edges, all read-only".to_string();
        said.usage = runtime::subagent_panes::Usage {
            output_tokens: 812,
            tool_calls: 4,
        };
        let done = finished(&job, &said);
        assert_eq!(done.status, "completed");
        assert!(super::super::agent_output_status_is_terminal(&done.status));
        assert_eq!(done.result.as_deref(), Some("three edges, all read-only"));
        assert!(done.error.is_none());
        assert_eq!(done.agent_id, manifest.agent_id);
        assert_eq!(done.name, manifest.name);
        // The cost line the card and the background notification both spell.
        assert_eq!(done.run.output_tokens, 812);
        assert_eq!(done.run.tool_calls, 4);
        // And the store agrees the child is over — a completion published
        // against a manifest still saying `running` is the signature of a
        // helper that finished and stayed on screen forever.
        let stored: super::super::AgentOutput = serde_json::from_str(
            &std::fs::read_to_string(isolated.store.join(format!("{}.json", manifest.agent_id)))
                .expect("manifest"),
        )
        .expect("parse");
        assert_eq!(stored.status, "completed");

        // The child's own two failures keep the same vocabulary.
        let mut broke = TeammateResult::new(&manifest.agent_id, Exit::Error);
        broke.error = Some("the provider refused".to_string());
        let failed = finished(&job, &broke);
        assert_eq!(failed.status, "failed");
        assert_eq!(failed.error.as_deref(), Some("the provider refused"));
        let stopped = finished(&job, &TeammateResult::new(&manifest.agent_id, Exit::Cancelled));
        assert_eq!(stopped.status, "stopped");
        assert!(stopped.error.is_some(), "a stop with no reason says nothing");
    }

    /// A child that died without writing says where to look.
    #[test]
    fn a_pane_that_closed_with_no_result_is_an_error_that_names_the_transcript() {
        let isolated = Isolated::new("vanished");
        let tmux = fake_tmux(&isolated.store, "'%1'");
        let mut captured = None;
        let manifest = execute_agent_with_spawn_and_parent_model(
            an_agent("map the edges"),
            |job| {
                captured = Some(job.clone());
                spawn_pane_job_with(&tmux, job)
            },
            None,
            None,
        )
        .expect("spawn");
        let job = captured.expect("a job");
        // `%4` is not in the fake's pane list, so the watcher sees it gone.
        let directory = isolated.store.join(&manifest.agent_id);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let settled = loop {
            let stored: super::super::AgentOutput = serde_json::from_str(
                &std::fs::read_to_string(
                    isolated.store.join(format!("{}.json", manifest.agent_id)),
                )
                .expect("manifest"),
            )
            .expect("parse");
            if super::super::agent_output_status_is_terminal(&stored.status) {
                break stored;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the watcher never settled a pane that was already gone"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        };
        assert_eq!(settled.status, "failed");
        let said = settled.error.unwrap_or_default();
        assert!(said.contains("without writing a result"), "{said}");
        assert!(
            said.contains(&directory.display().to_string()),
            "the error does not say where to look: {said}"
        );
        drop(job);
    }

    /// A sibling whose pane id has not landed yet is still a sibling.
    ///
    /// The measured break: two `Agent` calls of one message spawn side by
    /// side, and the second asked its wave index while the first's `%13` was
    /// still one syscall from disk. Both answered 0, both asked for `-h`, and
    /// the column that should have formed did not.
    #[test]
    fn a_sibling_still_being_cut_already_counts_toward_the_wave() {
        let isolated = Isolated::new("race");
        let tmux = fake_tmux(&isolated.store, "'%1' '%4'");
        let mut captured = None;
        // A sibling that has reached `running` but whose split has not
        // answered yet — exactly the window the live run lost.
        execute_agent_with_spawn_and_parent_model(
            an_agent("first"),
            |job| {
                captured = Some(job);
                Ok(())
            },
            None,
            None,
        )
        .expect("first spawn");
        let held = captured.expect("a job");
        assert!(held.manifest.pane.is_none(), "the sibling is mid-split");

        let second = execute_agent_with_spawn_and_parent_model(
            an_agent("second"),
            |job| spawn_pane_job_with(&tmux, job),
            None,
            None,
        )
        .expect("second spawn");
        let brief = Brief::read(&isolated.store.join(&second.agent_id)).expect("brief");
        assert_eq!(brief.wave_index, 1);
        let split = asked(&isolated.store)
            .into_iter()
            .find(|line| line.starts_with("split-window"))
            .expect("nothing was split");
        assert!(
            split.starts_with("split-window -v"),
            "the second child halved the leader again: {split}"
        );

        // And the FIRST child, asked now — after its sibling exists — still
        // answers 0. A count of who is alive would answer 1 here and cut the
        // leader twice; only "who started before me" is stable whichever
        // thread asks first.
        assert_eq!(
            wave_index_of(&held),
            0,
            "the wave index moved under the child that opened it"
        );
    }

    /// A multiplexer that will not cut says so, loudly.
    #[test]
    fn a_pane_that_could_not_be_opened_fails_the_call_rather_than_running_inline() {
        let isolated = Isolated::new("refused");
        let refusing = Tmux::at("/nonexistent/tmux");
        let error = execute_agent_with_spawn_and_parent_model(
            an_agent("map the edges"),
            |job| spawn_pane_job_with(&refusing, job),
            None,
            None,
        )
        .expect_err("a refused split was accepted");
        let said = error.to_string();
        assert!(said.contains("could not open a pane"), "{said}");
        // And the agent is not left claiming to run.
        let stored: Vec<super::super::AgentOutput> = std::fs::read_dir(&isolated.store)
            .expect("store")
            .flatten()
            .filter(|entry| entry.path().extension().is_some_and(|kind| kind == "json"))
            .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
            .filter_map(|text| serde_json::from_str(&text).ok())
            .collect();
        assert!(
            stored.iter().all(|manifest| manifest.status != "running"),
            "a refused spawn left a running manifest"
        );
    }


    #[test]
    fn an_agent_id_that_could_climb_out_of_the_store_is_refused() {
        // The ids production mints are `agent-<nanos>`; this is the guard for
        // the day one is built from something a model said.
        for hostile in ["..", "../../etc", "a/b", "", "a b"] {
            assert!(
                child_directory_in(Path::new("/tmp/zo-pane-store"), hostile).is_err(),
                "`{hostile}` was accepted as a directory name"
            );
        }
        assert!(child_directory_in(Path::new("/tmp/zo-pane-store"), "agent-1757000000000").is_ok());
    }

    #[test]
    fn every_effort_tier_has_a_word_the_child_can_be_told() {
        for (tier, word) in [
            (api::EffortLevel::Low, "low"),
            (api::EffortLevel::Medium, "medium"),
            (api::EffortLevel::High, "high"),
            (api::EffortLevel::Xhigh, "xhigh"),
            (api::EffortLevel::Max, "max"),
            (api::EffortLevel::Ultra, "ultra"),
        ] {
            assert_eq!(effort_word(tier), word);
        }
    }

    /// A fork in a pane (t-2875) rides the resume road a re-cut child takes:
    /// the brief names the fork's OWN transcript — the parent's, copied — as
    /// `resume_transcript`, the split argv says `--resume-transcript`, and
    /// the carried harness is the parent's system prompt rather than a
    /// harness instruction of its own.
    #[test]
    fn a_fork_pane_child_is_cut_on_the_forked_parent_transcript() {
        use core_types::session::{ContentBlock, ConversationMessage, Session};

        let isolated = Isolated::new("fork");
        let tmux = fake_tmux(&isolated.store, "'%1' '%4'");
        let parent_transcript = isolated.store.join("parent-session.jsonl");
        let mut parent = Session::new().with_persistence_path(parent_transcript.clone());
        parent
            .push_message(ConversationMessage::user_text("Read the graph, then fork"))
            .expect("parent user turn");
        parent
            .push_message(ConversationMessage::assistant(vec![ContentBlock::ToolUse {
                id: "toolu_pane".to_string(),
                name: "Agent".to_string(),
                input: r#"{"subagent_type":"fork","prompt":"how many edges?"}"#.to_string(),
            }]))
            .expect("parent fork call");
        let mut input = an_agent("how many edges?");
        input.subagent_type = Some("fork".to_string());
        input.model = Some("claude-haiku-4-5".to_string());
        input.fork_source = Some(crate::ForkSource {
            transcript: parent_transcript,
            system_prompt: vec!["PARENT_PROMPT_CANARY".to_string()],
        });

        let manifest = execute_agent_with_spawn_and_parent_model(
            input,
            |job| spawn_pane_job_with(&tmux, job),
            Some("claude-fable-5-1"),
            None,
        )
        .expect("spawn a fork pane");
        assert_eq!(manifest.subagent_type.as_deref(), Some("fork"));
        assert_eq!(
            manifest.model.as_deref(),
            Some("claude-fable-5-1"),
            "a fork runs on the parent's model; the call's `model` is ignored"
        );

        let directory = isolated.store.join(&manifest.agent_id);
        let brief = Brief::read(&directory).expect("the fork's brief");
        // The transcript path is minted from the canonical store (the same
        // `/private/var` a resume reads), so compare against that.
        let own_transcript = std::fs::canonicalize(&isolated.store)
            .expect("canonical store")
            .join(format!("{}.session.jsonl", manifest.agent_id));
        assert_eq!(
            brief.resume_transcript.as_deref(),
            Some(own_transcript.as_path()),
            "the pane child must continue the fork's own copy of the parent transcript"
        );
        assert_eq!(brief.first_turn, None, "a fork is a fresh child on an inherited transcript");
        let harness = brief.harness.expect("the carried harness");
        assert_eq!(harness.system_prompt, vec!["PARENT_PROMPT_CANARY".to_string()]);
        assert_eq!(harness.model.effective.as_deref(), Some("claude-fable-5-1"));
        let copied = Session::load_from_path(&own_transcript).expect("the copy loads");
        assert_eq!(&copied.messages[..2], &parent.messages[..]);
        assert!(copied.messages[2..].iter().any(|message| {
            message.blocks.iter().any(|block| matches!(
                block,
                ContentBlock::ToolResult { tool_use_id, output, .. }
                    if tool_use_id == "toolu_pane" && output.starts_with("[fork]")
            ))
        }));
        let splits: Vec<String> = asked(&isolated.store)
            .into_iter()
            .filter(|line| line.starts_with("split-window"))
            .collect();
        assert_eq!(splits.len(), 1);
        assert!(
            splits[0].contains(&format!("--resume-transcript {}", own_transcript.display())),
            "the pane's argv does not continue the fork transcript: {}",
            splits[0]
        );
        // Let the watcher settle so no thread outlives the store.
        let mut said = TeammateResult::new(&manifest.agent_id, Exit::Ok);
        said.final_message = "three edges".to_string();
        said.write_turn(&directory, 1).expect("write turn 1");
        let settled = isolated.wait_settled(&manifest.agent_id, 1);
        assert_eq!(settled.status, "completed");
        super::super::clear_background_agent(&manifest.agent_id);
    }
}
