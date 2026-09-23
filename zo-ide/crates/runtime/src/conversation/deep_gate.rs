//! Deep-lane gate: a live `plan → implement → verify → retry` turn orchestrator.
//!
//! This wires the pure decision brain in [`decision_core::deep_lane`] (already
//! unit-tested and shared with the benchmark harness) into the interactive
//! streaming loop. It is **not** a second copy of that policy —
//! [`validate_plan`], [`parse_lens_verifier`] and [`fold_verification_attempt`]
//! are called directly, so the accept/retry rules can never drift
//! from the benchmark. Only the *live IO* lives here, because it genuinely
//! differs from the benchmark's subprocess path:
//!
//! - each phase is one streaming sub-turn (`run_turn_streaming_with_images`)
//!   instead of a spawned `zo -p`;
//! - the objective gate is the project's own check command, run through the
//!   shared [`crate::execute_bash`] chokepoint (the same green source the
//!   workflow `command_green` check converges on);
//! - the PLAN and VERIFY sub-turns run under [`PermissionMode::ReadOnly`] so the
//!   model cannot edit before a valid plan exists and the verifier inspects but
//!   never mutates.
//!
//! See `docs/design-deep-lane-live-wiring.md` for the full rationale and the
//! crate-dependency constraints that make a focused live gate (rather than a
//! full extraction of the benchmark loop) the right shape.

use std::collections::{BTreeSet, HashSet};
use std::fmt::Write as _;
use std::io::Cursor;
use std::sync::Arc;

use base64::Engine as _;
use serde_json::json;
use tokio::sync::mpsc;

use decision_core::deep_lane::{
    DeepDecision, MAX_SUMMARY_CHARS, PlanVerdict, VerificationAttempt, VerifierParse,
    VerifierVerdict, fold_verification_attempt, parse_lens_verifier, validate_plan,
};

use crate::hooks::HookEvent;
use crate::message_stream::types::{BlockIdGen, RenderBlock, SystemLevel};
use crate::model_router::{RouteTaskComplexity, RouteTaskIntent, RouteTaskRisk};
use crate::permission::PermissionPrompter as AsyncPermissionPrompter;
use crate::session::{ContentBlock, ConversationMessage, MessageRole};
use crate::usage::TokenUsage;
use crate::{BashCommandInput, PermissionMode, execute_bash};
use crate::permissions::TemporaryAllowGrant;

use super::{build_turn_end_hook_context, changed_files_snapshot_async};

use super::{
    ApiClient, AsyncApiClient, AutoCompactionEvent, BudgetExhausted, ConversationRuntime,
    PromptCacheEvent, StreamingTurnError, ToolExecutor, TurnSummary,
};

/// How the gate structures a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DeepMode {
    /// Reactive (default): run the turn normally with full tools, then — only if
    /// it actually edited files — auto-verify the diff and retry on failure. No
    /// read-only planning phase, so there is no permission friction; chat and
    /// analysis turns pass straight through with zero overhead.
    #[default]
    Reactive,
    /// Plan-first: force a structured read-only PLAN before any edit, then
    /// implement → verify → retry. Stronger guarantee for hard tasks at the cost
    /// of a read-only planning phase (bash is blocked there).
    PlanFirst,
}

/// How many implementer attempts the Architect contract runs before
/// escalating the EXEC leg back to the native (reserved) model. Mirrors the
/// router's `implementation_route_model_allowed` escape (`prior_failures >=
/// 2`): two real implementer failures are the contract's own escalation
/// signal, so the third attempt runs on the session's premium model.
const ARCHITECT_IMPL_ATTEMPTS: u32 = 2;
const CHECK_OUTPUT_TAIL_BYTES: usize = 4_000;
const VERIFY_EDITED_PATHS_BYTES: usize = 2_000;
const VERIFY_ASSISTANT_CLAIM_BYTES: usize = 4_000;
// Post-edit file contents attached to a SingleLens verify prompt so the
// common leg is a one-call verdict instead of read_file-then-verdict
// (measured: the leg's only tool call was re-reading files whose content the
// diff or the conversation already carried — a full API round trip per leg).
// Proportionate to the 6KB diff bound; oversized/binary files get a skip
// note directing the verifier to read_file them instead.
const VERIFY_FILE_ATTACH_PER_FILE_BYTES: usize = 12_000;
const VERIFY_FILE_ATTACH_TOTAL_BYTES: usize = 32_000;
const EXEC_PRIOR_DIFF_BYTES: usize = 6_000;
const EXEC_PRIOR_EDITED_PATHS_BYTES: usize = 2_000;
// Desktop, mobile, and one intermediate/current frame cover the common visual
// checks without multiplying the verifier packet by every screenshot an EXEC
// leg took while iterating.
const VERIFY_IMAGE_MAX_COUNT: usize = 3;
const VERIFY_VISUAL_EVIDENCE_BLOCK: &str =
    "\n\nAttached images are direct visual evidence produced by the EXEC leg's tools. \
     Inspect them yourself. Concrete visual defects are in scope even when they cannot \
     be inferred from the text diff.";
// Two paths still covers one focused edit plus a directly coupled companion file.
const FILES_TRIVIAL_MAX: usize = 2;
// Twenty-four changed lines keeps skip eligibility to genuinely tiny patches.
const CHURN_TRIVIAL_MAX: usize = 24;
// Four paths bounds spec-only review to a small, locally auditable change.
const FILES_SMALL_MAX: usize = 4;
// One hundred sixty changed lines is a conservative ceiling for modest churn.
const CHURN_SMALL_MAX: usize = 160;
const SECURITY_PATH_MARKERS: &[&str] = &[
    "auth",
    "secret",
    "credential",
    "token",
    "crypto",
    "password",
];
const TEST_PATH_MARKERS: &[&str] = &["test", "_test.", ".test.", "spec"];

/// Per-turn Architect execution contract (`smart.policy=architect`): the
/// metadata for an implementation-shaped turn whose session main model is
/// reserved for plan/orchestrate/verify duty
/// ([`crate::is_reserved_orchestrator_model`]).
///
/// Installed by the host on every turn entry via
/// [`ConversationRuntime::set_exec_contract`] — set-or-cleared, mirroring
/// `set_deep_verify_candidates`, so it can never outlive its turn. `None`
/// keeps the pre-contract behavior. The optional implementer client gates only
/// the EXEC swap (`smart.execSwap`); plan-first promotion can remain active
/// while EXEC runs on the native client, but the foreground edit gate arms only
/// for a live swap. PLAN and VERIFY use their independent deep-lane clients.
#[derive(Clone)]
pub struct ExecContract {
    /// The implementer client EXEC legs swap to (attempts
    /// `1..=ARCHITECT_IMPL_ATTEMPTS`). `None` means the configured policy did
    /// not arm a swap for this turn's difficulty.
    pub impl_client: Option<Arc<dyn AsyncApiClient>>,
    /// The implementer's model id, for narration and telemetry.
    pub impl_model: String,
    /// Run the read-only PLAN phase before the first EXEC even when the gate
    /// mode is Reactive (complex/multi-scope implementation turns).
    pub plan_first: bool,
}

impl ExecContract {
    /// Whether this contract swaps EXEC legs away from the session client.
    #[must_use]
    pub fn exec_swap_enabled(&self) -> bool {
        self.impl_client.is_some()
    }
}

impl std::fmt::Debug for ExecContract {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecContract")
            .field("impl_model", &self.impl_model)
            .field("plan_first", &self.plan_first)
            .field("exec_swap_enabled", &self.exec_swap_enabled())
            .finish_non_exhaustive()
    }
}

/// Which upstream client a deep-gate sub-turn (leg) runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubturnClient {
    /// The session's native client (reserved PLAN or EXEC without a contract).
    Native,
    /// The Architect deep-tier PLAN client; falls back to Native only when the
    /// session model is itself reserved deep-tier.
    Plan,
    /// A native-client Architect EXEC leg. It remains exempt from the edit
    /// gate without pretending that an implementer client was swapped in.
    NativeExec,
    /// The ranked cross-model verifier candidate selected by `verify_subturn`
    /// (`deep_verify_candidate_idx`).
    Verify,
    /// The Architect contract's implementer ([`ExecContract::impl_client`]).
    /// Falls back to Native when no contract is installed.
    Implementer,
}

/// Semantic phase of one deep-gate leg. Client and permission mode cannot
/// identify this reliably: PLAN and VERIFY are both read-only, while EXEC may
/// run on the native, native-exec, or implementer client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeepSubturnPhase {
    Plan,
    Exec,
    Verify,
}

#[derive(Debug)]
struct DeepSubturnResult {
    summary: TurnSummary,
    /// Guarded tool-produced images harvested from this leg. Always empty for
    /// PLAN and VERIFY, even if either phase called an image-bearing tool.
    verify_images: Vec<(String, String)>,
}

/// Configuration for the deep-lane gate. Installed on a [`ConversationRuntime`]
/// via [`ConversationRuntime::set_deep_gate`]; absent (`None`) means a turn runs
/// the ordinary single-pass loop with no verification.
#[derive(Debug, Clone)]
pub struct DeepGateConfig {
    /// How the turn is structured (reactive vs plan-first).
    pub mode: DeepMode,
    /// Project check command whose exit 0 is treated as objectively green. When
    /// `None`, there is no objective gate and the adversarial verifier alone
    /// decides acceptance. The host can fill this from [`detect_check_command`].
    pub check_command: Option<String>,
    /// Upper bound on the implement→verify retries (and, in plan-first mode, the
    /// plan re-tries).
    pub max_attempts: u32,
}

impl Default for DeepGateConfig {
    fn default() -> Self {
        Self {
            mode: DeepMode::Reactive,
            check_command: None,
            max_attempts: 2,
        }
    }
}

/// Best-effort detection of the project's check command from the working
/// directory, for the reactive objective gate. Returns `None` when no known
/// project marker is present (the verifier then decides alone). First match in a
/// fixed order wins, so a mixed repo gets a deterministic choice.
///
/// This is wired as the opt-in per-coding-turn reactive gate (installed only
/// when `ZO_AUTO_VERIFY=1` — see `install_reactive_verify_gate_if_coding`), so
/// the default must be a *cheap* objective signal, not a full multi-minute test
/// run on a large repo. For Rust the default is therefore `cargo build --tests`: it
/// compiles every test target (catching the same build/type errors a
/// `cargo test` build would surface) without paying for the test *execution*.
/// The heavier full `cargo test` stays available where a turn explicitly asks
/// for it; the reactive auto default favors a fast green-build gate.
#[must_use]
pub fn detect_check_command() -> Option<String> {
    let cwd = std::env::current_dir().ok()?;
    let has = |name: &str| cwd.join(name).is_file();
    if has("Cargo.toml") {
        return Some("cargo build --tests".to_string());
    }
    if has("package.json") {
        return Some("npm test".to_string());
    }
    if has("pyproject.toml") || has("pytest.ini") || has("setup.cfg") {
        return Some("pytest".to_string());
    }
    if has("go.mod") {
        return Some("go test ./...".to_string());
    }
    if let Ok(entries) = std::fs::read_dir(&cwd) {
        for entry in entries.flatten() {
            if let Some(ext) = entry.path().extension() {
                if ext == "sln" || ext == "csproj" {
                    return Some("dotnet test".to_string());
                }
            }
        }
    }
    None
}

/// The structured result of a deep turn, for telemetry and the final note.
#[derive(Debug, Clone)]
pub struct DeepOutcome {
    pub decision: DeepDecision,
    pub attempts: u32,
    pub plan_valid: bool,
    pub plan_missing: Vec<String>,
    /// The adversarial verifier's semantic verdict on the change, surfaced for
    /// goal-completion gating (anti "optimistic stop"). `Some(true)` = the
    /// VERIFY phase accepted an edit-making turn, `Some(false)` = it rejected
    /// or the gate gave up, `None` = no semantic judgment was made this turn
    /// (a chat/analysis turn that changed nothing, or a trivial green edit whose
    /// proportional depth skipped VERIFY). Distinct from `decision`,
    /// which also reports `Accept` for a no-edit turn that was never verified.
    pub verification: Option<bool>,
    /// The concrete problems the adversarial verifier raised on the FINAL
    /// attempt (the unresolved rejection that ended the inner loop). Empty when
    /// the change was accepted or no verify ran. Surfaced so the goal-level
    /// repair prompt can tell the model *what specifically to fix* instead of a
    /// generic "rejected, try again" — the inner loop already feeds these back
    /// (`failure_summary`); this carries the same signal to the outer loop.
    pub issues: Vec<String>,
    /// How the most recent VERIFY sub-turn's verdict was recovered (Phase 4
    /// verdict-channel seam — NOT consumed by any accept/retry/stall policy,
    /// which stays entirely in `decision-core`). `None` when no VERIFY
    /// sub-turn ran this turn (a no-edit chat/analysis turn or proportional
    /// VERIFY skip). `Some(
    /// VerifierParse::Json | VerifierParse::Salvaged)` means the verifier
    /// actually produced a usable verdict — `verification` reflects it and a
    /// verdict-outcome recorder may safely record it. `Some(VerifierParse::
    /// Empty | Unparseable | Timeout)` means a VERIFY sub-turn ran but
    /// recovered no usable signal (`verification` is still gated
    /// conservatively for goal-completion purposes) — a verdict-outcome
    /// recorder MUST treat this as "no signal, do not record" per the
    /// ambiguous-verdicts-are-never-recorded doctrine.
    pub verifier_parse: Option<VerifierParse>,
    /// The cross-model verifier's model id when [`ConversationRuntime::
    /// set_deep_verify_client`] installed one for this turn's VERIFY leg
    /// (`None` when the leg ran on the turn's own native model, or when no
    /// VERIFY leg ran this turn — same absence condition as `verifier_parse`).
    /// Surfaced so a verdict-outcome recorder can credit/blame the verifier's
    /// OWN run under its real model, distinct from the main turn's model.
    pub verifier_model: Option<String>,
}

// ── Pure helpers (no IO; unit-tested below) ──────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum VerifyDepth {
    Skip,
    SingleLens,
    Full,
}

/// The band is a pre-execution guess (probe/verb match); the diff arguments
/// are post-execution facts. For `Medium` complexity the facts outrank the
/// guess — a verb like "fix" bands casual one-line turns as `Medium`, and the
/// spec lens covers a measured-small green change regardless of what the verb
/// promised. `Large`/`Unknown` complexity and `High`/`Critical`/`Unknown`
/// risk stay `Full` no matter how small the diff: when the guess says "big or
/// unreadable", a small measured diff is evidence the change may be
/// INCOMPLETE, not that it is safe. Every objective guard (unscoped or
/// oversized diff, security or test paths, red check) forces `Full`
/// regardless of band.
fn verify_depth(
    complexity: RouteTaskComplexity,
    risk: RouteTaskRisk,
    files_changed: usize,
    line_churn: usize,
    objective_ok: bool,
    touches_security: bool,
    touches_tests: bool,
) -> VerifyDepth {
    if files_changed == 0
        || matches!(
            complexity,
            RouteTaskComplexity::Large | RouteTaskComplexity::Unknown
        )
        || matches!(
            risk,
            RouteTaskRisk::High | RouteTaskRisk::Critical | RouteTaskRisk::Unknown
        )
        || !objective_ok
        || files_changed > FILES_SMALL_MAX
        || line_churn > CHURN_SMALL_MAX
        || touches_security
        || touches_tests
    {
        return VerifyDepth::Full;
    }

    // Reachable here: {Trivial, Small, Medium} complexity × {Low, Medium}
    // risk, with a small green diff outside security/test paths. Depth must
    // stay monotone in the band from this point — Trivial may never verify
    // deeper than Small — so everything that does not qualify for the
    // tiny-Trivial skip gets exactly one spec lens.
    if complexity == RouteTaskComplexity::Trivial
        && risk == RouteTaskRisk::Low
        && files_changed <= FILES_TRIVIAL_MAX
        && line_churn <= CHURN_TRIVIAL_MAX
    {
        return VerifyDepth::Skip;
    }

    VerifyDepth::SingleLens
}

fn verify_depth_for_band(
    band: Option<(RouteTaskComplexity, RouteTaskRisk)>,
    files_changed: usize,
    line_churn: usize,
    objective_ok: bool,
    touches_security: bool,
    touches_tests: bool,
) -> VerifyDepth {
    let Some((complexity, risk)) = band else {
        return VerifyDepth::Full;
    };
    verify_depth(
        complexity,
        risk,
        files_changed,
        line_churn,
        objective_ok,
        touches_security,
        touches_tests,
    )
}

/// The verification depth this turn's probed intent is entitled to, as a FLOOR
/// only ([`VerifyDepth::Skip`] is the identity under `max`).
///
/// A `Design` turn never skips verification outright: its deliverable is a
/// look, so "small green diff" says nothing about whether the result is any
/// good — the one signal the objective check cannot carry is exactly the one
/// the design lens exists to read. But it deliberately does NOT force
/// [`VerifyDepth::Full`]: proportionality still comes from the band, the file
/// count, and the churn, so a two-line color tweak gets one lens, not three.
const fn intent_verify_floor(intent: RouteTaskIntent) -> VerifyDepth {
    match intent {
        RouteTaskIntent::Design => VerifyDepth::SingleLens,
        RouteTaskIntent::Implementation | RouteTaskIntent::Analysis | RouteTaskIntent::Other => {
            VerifyDepth::Skip
        }
    }
}

fn paths_touch_security(paths: &[String]) -> bool {
    paths.iter().any(|path| {
        let path = path.to_ascii_lowercase();
        SECURITY_PATH_MARKERS
            .iter()
            .any(|marker| path.contains(marker))
    })
}

fn paths_touch_tests(paths: &[String]) -> bool {
    paths.iter().any(|path| {
        let path = path.to_ascii_lowercase();
        TEST_PATH_MARKERS
            .iter()
            .any(|marker| path.contains(marker))
    })
}

/// Map a bash `return_code_interpretation` to "green" (exit 0). `None` means the
/// command exited 0 and produced the expected output; any other interpretation
/// (`exit_code:N`, `timeout`, …) is not green. Mirrors the workflow
/// `command_green` reader so both paths agree on what green means.
fn interpret_green(interpretation: Option<&str>) -> bool {
    match interpretation {
        None => true,
        Some(code) => {
            code.strip_prefix("exit_code:")
                .and_then(|n| n.parse::<i32>().ok())
                == Some(0)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CheckObservation {
    green: bool,
    output_tail: String,
    /// The tree the check actually ran in, captured AT RUN TIME — the diff
    /// may span a sibling worktree while the check runs in this process's
    /// cwd, and `EnterWorktree` can move that cwd between the check and the
    /// prompt assembly, so re-sampling later would label the wrong tree.
    ran_in: Option<std::path::PathBuf>,
}

/// Run the objective check command and report whether it was green. A command
/// that fails to even start is treated as not-green, never a panic. Runs in the
/// live process cwd (the working tree) via the shared [`crate::execute_bash`]
/// chokepoint.
///
/// `execute_bash` is a *blocking* call (it drives a child process to completion
/// via `block_in_place`). The reactive/deep gate runs inside the host's live
/// streaming turn, whose `select!` polls the turn future on the same task; a
/// `block_in_place` there suspends the whole task, freezing the TUI's input,
/// mouse and spinner for the entire (potentially multi-minute) check run. So
/// offload it to a dedicated blocking thread via `spawn_blocking` and `await`
/// the result — the turn task yields, keeping the event loop live throughout.
async fn run_check_command(command: &str) -> CheckObservation {
    let command = command.to_string();
    let Ok(observation) = tokio::task::spawn_blocking(move || {
        let ran_in = std::env::current_dir().ok();
        let Ok(input) = serde_json::from_value::<BashCommandInput>(json!({ "command": command }))
        else {
            return CheckObservation {
                green: false,
                output_tail: "check command could not be parsed".to_string(),
                ran_in,
            };
        };
        match execute_bash(input) {
            Ok(out) => CheckObservation {
                green: interpret_green(out.return_code_interpretation.as_deref()),
                output_tail: bounded_check_output_tail(&out.stdout, &out.stderr),
                ran_in,
            },
            Err(error) => CheckObservation {
                green: false,
                output_tail: format!("check command failed to start: {error}"),
                ran_in,
            },
        }
    })
    .await
    else {
        // The blocking task panicked or was cancelled — treat as not-green
        // rather than propagating a panic into the turn loop.
        return CheckObservation {
            green: false,
            output_tail: "check command runner was cancelled".to_string(),
            ran_in: None,
        };
    };
    observation
}

/// `" (ran in <tree>)"` qualifier for a check label, from the observation's
/// own run-time record.
fn check_ran_in_label(observation: &CheckObservation) -> String {
    observation
        .ran_in
        .as_ref()
        .map(|dir| format!(" (ran in {})", dir.display()))
        .unwrap_or_default()
}

async fn command_is_green(command: &str) -> bool {
    run_check_command(command).await.green
}

fn bounded_check_output_tail(stdout: &str, stderr: &str) -> String {
    let mut output = String::new();
    if !stdout.trim().is_empty() {
        let _ = writeln!(output, "stdout:\n{}", stdout.trim_end());
    }
    if !stderr.trim().is_empty() {
        let _ = writeln!(output, "stderr:\n{}", stderr.trim_end());
    }
    if output.is_empty() {
        return "(no output)".to_string();
    }
    truncate_to_tail_on_boundary(&mut output, CHECK_OUTPUT_TAIL_BYTES);
    output
}

/// A bounded `git diff` of the relevant working-tree paths for the verifier
/// prompt. It asks git for only those pathspecs so unrelated pre-existing
/// dirt cannot crowd the actual attempt out of the bounded verifier prompt.
/// Read-only; an unavailable git or oversized diff degrades to a truncated best
/// effort.
///
/// Offloaded to a blocking thread for the same reason as [`command_is_green`]:
/// even a scoped `git diff` can walk the index and must not freeze the live TUI
/// event loop while it runs.
async fn bounded_git_diff_for_paths(
    paths: Vec<String>,
    max: usize,
    own_root: Option<std::path::PathBuf>,
) -> (String, usize) {
    let mut diff =
        tokio::task::spawn_blocking(move || scoped_git_diff(&paths, own_root.as_deref()))
            .await
            .unwrap_or_default();
    let line_churn = diff_line_churn(&diff);
    truncate_on_boundary(&mut diff, max);
    (diff, line_churn)
}

/// This process's own repository root, resolved off the event loop (it spawns
/// `git rev-parse`). Computed once per attempt and threaded to every path
/// normalization/grouping consumer, instead of each re-spawning git.
async fn own_repo_root_async() -> Option<std::path::PathBuf> {
    tokio::task::spawn_blocking(|| repo_root_for(std::path::Path::new(".")))
        .await
        .unwrap_or_default()
}

fn diff_line_churn(diff: &str) -> usize {
    diff.lines()
        .filter(|line| {
            (line.starts_with('+') && !line.starts_with("+++ "))
                || (line.starts_with('-') && !line.starts_with("--- "))
        })
        .count()
}

fn scoped_git_diff(paths: &[String], own_root: Option<&std::path::Path>) -> String {
    if paths.is_empty() {
        // NEVER fall back to the full working-tree diff here. On a tree that
        // was already dirty before the attempt (another task's edits, another
        // worktree's half-done work), the full diff attributes every one of
        // those pre-existing changes to THIS attempt — observed in the field
        // as a verifier rejecting a plan-writing turn because "the attempt
        // modified four production files" that the attempt never touched.
        return "(this attempt reported no file edits; pre-existing \
                working-tree changes are deliberately not shown)"
            .to_string();
    }

    // Render every group FIRST (a group whose diff is empty — edits already
    // committed, nothing changed — contributes nothing, not even its label:
    // a bare label with no hunks read as the whole diff and swallowed the
    // "(no git diff …)" fallback), then decide labeling from what actually
    // rendered.
    let mut rendered: Vec<(Option<std::path::PathBuf>, String)> = Vec::new();
    for (root, group) in group_paths_by_repo_root(paths, own_root) {
        let mut part = run_git_diff(root.as_deref(), &group);
        append_untracked_file_diffs(&mut part, root.as_deref(), &group);
        if part.trim().is_empty() {
            continue;
        }
        rendered.push((root, part));
    }
    // Label EVERY rendered group whenever the diff spans more than this
    // process's own repository. Labeling only the foreign group left the own
    // group's hunks dangling under the foreign header when sort order put the
    // foreign root first — the verifier read one repo's edits as the other's,
    // the exact misattribution this change exists to remove.
    let label_groups = rendered.len() > 1
        || rendered
            .first()
            .is_some_and(|(root, _)| root.as_deref() != own_root);
    let mut diff = String::new();
    for (root, part) in rendered {
        if !diff.is_empty() && !diff.ends_with('\n') {
            diff.push('\n');
        }
        if label_groups {
            match root.as_deref() {
                Some(root) => {
                    let _ = writeln!(diff, "(attempt paths in repository: {})", root.display());
                }
                None => {
                    let _ = writeln!(diff, "(attempt paths outside any git repository)");
                }
            }
        }
        diff.push_str(&part);
    }
    if diff.trim().is_empty() {
        let _ = writeln!(
            diff,
            "(no git diff for scoped attempt paths: {})",
            paths.join(", ")
        );
    }
    diff
}

/// Group attempt paths by the git repository that OWNS them, resolved from
/// each path's own location rather than this process's cwd. Edits into a
/// sibling worktree (the isolation worktrees agents routinely use) probed as
/// "outside the repository" from here: `git diff` returned nothing and
/// `ls-files --error-unmatch` failed, so a real modification reached the
/// verifier as an empty diff or misrendered as a brand-new file. `None`
/// groups paths in no repository at all (rendered via `--no-index`).
///
/// Spawn discipline: `git rev-parse` is memoized per nearest existing
/// ancestor directory, so the cost scales with the number of DIRECTORIES
/// touched, not files. Deliberately NO "under `own_root` ⇒ own repo"
/// prefix shortcut: `EnterWorktree` creates its isolation worktrees INSIDE
/// the repository (a relative path joined onto the cwd), and a prefix check
/// classified those — and any nested repo — as the outer repository,
/// resurrecting the exact new-file misrendering this grouping exists to fix.
/// Only the probe's own answer decides the group.
fn group_paths_by_repo_root(
    paths: &[String],
    own_root: Option<&std::path::Path>,
) -> Vec<(Option<std::path::PathBuf>, Vec<String>)> {
    let mut probe_memo: std::collections::BTreeMap<
        std::path::PathBuf,
        Option<std::path::PathBuf>,
    > = std::collections::BTreeMap::new();
    let mut groups: std::collections::BTreeMap<
        Option<std::path::PathBuf>,
        Vec<String>,
    > = std::collections::BTreeMap::new();
    for path in paths {
        // Same anchoring rule as `attempt_path_key`: a RELATIVE spelling here
        // is a `changed_files_snapshot` product and therefore repo-root
        // relative — anchoring it at the process cwd instead silently pointed
        // the pathspec at `<cwd>/<rel>` whenever zo runs in a subdirectory of
        // the repository, and that file's diff went missing. EVERY downstream
        // consumer gets this one spelling — the untracked probe and the
        // no-index rendering read different files when they anchored
        // differently, and the packet carried the wrong file's contents.
        let absolute = attempt_path_key(path, own_root);
        let root = match nearest_existing_dir(&absolute) {
            Some(probe) => probe_memo
                .entry(probe)
                .or_insert_with_key(|probe| repo_root_of_dir(probe))
                .clone(),
            None => None,
        };
        groups
            .entry(root)
            .or_default()
            .push(absolute.to_string_lossy().into_owned());
    }
    groups.into_iter().collect()
}

/// The repository root owning `path`, from its nearest EXISTING ancestor (a
/// deleted file must still resolve to the tree it was deleted from). `None`
/// when no ancestor is inside a git repository.
fn repo_root_for(path: &std::path::Path) -> Option<std::path::PathBuf> {
    let absolute = std::path::absolute(path).ok()?;
    repo_root_of_dir(&nearest_existing_dir(&absolute)?)
}

/// Nearest existing DIRECTORY at-or-above `path` (the path itself may be a
/// deleted file, or sit under a directory the attempt removed).
fn nearest_existing_dir(path: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut probe = path;
    loop {
        if probe.is_dir() {
            return Some(probe.to_path_buf());
        }
        probe = probe.parent()?;
    }
}

fn repo_root_of_dir(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let output = std::process::Command::new("git")
        .args(["--no-optional-locks", "rev-parse", "--show-toplevel"])
        .current_dir(dir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if root.is_empty() {
        None
    } else {
        Some(std::path::PathBuf::from(root))
    }
}

fn run_git_diff(repo_root: Option<&std::path::Path>, paths: &[String]) -> String {
    let mut command = std::process::Command::new("git");
    if let Some(root) = repo_root {
        command.current_dir(root);
    }
    command.args(["--no-optional-locks"]);
    if !paths.is_empty() {
        command.args(["--literal-pathspecs"]);
    }
    command.args(["diff"]);
    if !paths.is_empty() {
        command.arg("--");
        command.args(paths);
    }
    match command.output() {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout).into(),
        _ => String::new(),
    }
}

fn append_untracked_file_diffs(
    diff: &mut String,
    repo_root: Option<&std::path::Path>,
    paths: &[String],
) {
    for path in paths {
        if !path_is_untracked_file(repo_root, path) {
            continue;
        }
        if !diff.is_empty() && !diff.ends_with('\n') {
            diff.push('\n');
        }
        diff.push_str(&run_no_index_new_file_diff(path));
    }
}

fn path_is_untracked_file(repo_root: Option<&std::path::Path>, path: &str) -> bool {
    if !std::path::Path::new(path).is_file() {
        return false;
    }
    let mut command = std::process::Command::new("git");
    if let Some(root) = repo_root {
        command.current_dir(root);
    }
    let output = command
        .args([
            "--no-optional-locks",
            "--literal-pathspecs",
            "ls-files",
            "--error-unmatch",
            "--",
            path,
        ])
        .output();
    matches!(output, Ok(output) if !output.status.success())
}

fn run_no_index_new_file_diff(path: &str) -> String {
    let empty = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let output = std::process::Command::new("git")
        .args([
            "--no-optional-locks",
            "diff",
            "--no-index",
            "--",
            empty,
            path,
        ])
        .output();
    match output {
        // `git diff --no-index` returns 1 when files differ; stdout is still the
        // useful patch. Treat any stdout as best-effort diff content.
        Ok(output) if !output.stdout.is_empty() => String::from_utf8_lossy(&output.stdout).into(),
        _ => String::new(),
    }
}

fn edited_file_paths(summary: &TurnSummary) -> Vec<String> {
    let mut paths = BTreeSet::new();
    for message in &summary.tool_results {
        for block in &message.blocks {
            let ContentBlock::ToolResult {
                tool_name,
                output,
                is_error,
                ..
            } = block
            else {
                continue;
            };
            if *is_error || !super::is_edit_or_write_tool(tool_name) {
                continue;
            }
            if let Some(path) = tool_result_path(output) {
                paths.insert(path);
            }
        }
    }
    paths.into_iter().collect()
}

pub(super) fn tool_result_path(output: &str) -> Option<String> {
    let value = crate::compact::result_envelope(output)?;
    ["filePath", "path", "file_path"]
        .into_iter()
        .find_map(|key| value.get(key).and_then(serde_json::Value::as_str))
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(ToOwned::to_owned)
}

/// One canonical comparison key for an attempt path, whatever spelling it
/// arrived in. The two path sources spell the SAME file differently — edit
/// tools report canonicalized absolute paths, `changed_files_snapshot` emits
/// repo-root-relative ones — and comparing them as raw strings silently never
/// matched: the attempt's own edit survived the baseline subtraction and was
/// handed to the verifier as "pre-existing, do NOT judge" (a rubber stamp for
/// exactly the file the turn changed). Relative spellings are anchored at
/// `own_root` (the snapshot's own frame); absolute ones pass through
/// `std::path::absolute` for lexical normalization.
fn attempt_path_key(path: &str, own_root: Option<&std::path::Path>) -> std::path::PathBuf {
    let trimmed = path.trim();
    let raw = std::path::Path::new(trimmed);
    let anchored = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        match own_root {
            Some(root) => root.join(raw),
            None => raw.to_path_buf(),
        }
    };
    let absolute = std::path::absolute(&anchored).unwrap_or(anchored);
    // `std::path::absolute` keeps `..` components, and built-in edit tools are
    // the only writers guaranteed to report canonicalized paths — MCP/plugin
    // edit tools pass their `filePath` through verbatim, so `a/x/../b.rs` and
    // `a/b.rs` keyed as different files and the attempt's own edit slipped
    // back into the do-not-judge list. Resolve them lexically.
    let mut key = std::path::PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !key.pop() {
                    key.push(component.as_os_str());
                }
            }
            other => key.push(other.as_os_str()),
        }
    }
    key
}

/// Working-tree changes that predate the attempt: the baseline snapshot minus
/// everything the attempt itself touched. Fed to the verifier as an explicit
/// "not yours to judge" list — see [`VerifyPromptContext::preexisting`].
/// Subtraction compares [`attempt_path_key`]s, not raw strings (the baseline
/// is repo-relative while `diff_paths` carries absolute tool spellings); the
/// surviving entries keep their original repo-relative spelling for the
/// prompt.
fn preexisting_dirty_paths(
    baseline_files: &[String],
    diff_paths: &[String],
    own_root: Option<&std::path::Path>,
) -> Vec<String> {
    let attempt: BTreeSet<std::path::PathBuf> = diff_paths
        .iter()
        .map(|path| attempt_path_key(path, own_root))
        .collect();
    baseline_files
        .iter()
        .filter(|path| !attempt.contains(&attempt_path_key(path, own_root)))
        .cloned()
        .collect()
}

fn attempt_diff_paths(
    baseline_files: &[String],
    after_files: &[String],
    edited_paths: &[String],
    own_root: Option<&std::path::Path>,
) -> Vec<String> {
    let baseline: BTreeSet<std::path::PathBuf> = baseline_files
        .iter()
        .map(|path| attempt_path_key(path, own_root))
        .collect();
    let mut seen: BTreeSet<std::path::PathBuf> = BTreeSet::new();
    let mut paths: Vec<String> = Vec::new();
    for path in edited_paths {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            continue;
        }
        if seen.insert(attempt_path_key(trimmed, own_root)) {
            paths.push(trimmed.to_owned());
        }
    }
    // Baseline-clean files that changed during the attempt but were reported
    // by no edit tool (a build step, a formatter). Key-based dedupe: the same
    // file typically also arrives via `edited_paths` under its absolute
    // spelling, and double-listing it inflated the verifier's path list and
    // the depth heuristic's file count.
    for path in after_files {
        let key = attempt_path_key(path, own_root);
        if baseline.contains(&key) || !seen.insert(key) {
            continue;
        }
        paths.push(path.clone());
    }
    paths.sort();
    paths
}

/// Truncate `s` to at most `max` bytes, never splitting a UTF-8 char.
pub(super) fn truncate_on_boundary(s: &mut String, max: usize) {
    if s.len() <= max {
        return;
    }
    let mut n = max;
    while n > 0 && !s.is_char_boundary(n) {
        n -= 1;
    }
    s.truncate(n);
}

fn truncate_to_tail_on_boundary(s: &mut String, max: usize) {
    if s.len() <= max {
        return;
    }
    let mut start = s.len().saturating_sub(max);
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    s.drain(..start);
}

/// Build the retry repair contract from the failing attempt, bounded to
/// [`MAX_SUMMARY_CHARS`] so a retry's added model cost stays small.
fn failure_summary(objective_ok: bool, verifier: &VerifierVerdict) -> String {
    let mut s =
        String::from("Your previous attempt did NOT pass. Treat this as the repair contract:\n");
    if !objective_ok {
        let _ = writeln!(
            s,
            "- The objective check is RED. Make it pass without weakening, modifying, or deleting tests."
        );
    }
    if verifier.issues.is_empty() {
        if !verifier.accepted {
            let _ = writeln!(
                s,
                "- The verifier rejected the change (no specific issues were itemized)."
            );
        }
    } else {
        let _ = writeln!(s, "- The verifier raised these issues:");
        for issue in &verifier.issues {
            let _ = writeln!(s, "  - {issue}");
        }
    }
    let _ = writeln!(
        s,
        "Use the current working tree (your prior edits are still applied) to find the exact code to fix or narrow. Do not keep verifier-rejected behavior just because tests are green."
    );
    s.push_str("\nMandatory repair checklist:\n");
    s.push_str("- Make the objective check pass first; do not stop on a red check.\n");
    s.push_str(
        "- For every verifier finding above, change the code until that exact defect is gone.\n",
    );
    s.push_str("- If a finding names a stale symbol, wrong receiver, or missed call site, search intended files and fix every occurrence.\n");
    s.push_str("- If the task threads options or a new argument, audit wrappers and cache paths for stale or mixed-mode results.\n");
    s.push_str(
        "- Re-run the exact failing check after edits and inspect any remaining failure before stopping.\n",
    );
    truncate_on_boundary(&mut s, MAX_SUMMARY_CHARS);
    s
}

fn exec_retry_context(
    repair: &str,
    diff: &str,
    edited_paths: &[String],
    check: Option<(&str, &CheckObservation)>,
) -> String {
    let mut paths = if edited_paths.is_empty() {
        "(none reported)".to_string()
    } else {
        edited_paths
            .iter()
            .map(|path| format!("- {path}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    truncate_on_boundary(&mut paths, EXEC_PRIOR_EDITED_PATHS_BYTES);

    let mut diff = if diff.trim().is_empty() {
        "(no scoped diff available)".to_string()
    } else {
        diff.to_string()
    };
    truncate_on_boundary(&mut diff, EXEC_PRIOR_DIFF_BYTES);

    let check = match check {
        Some((command, observation)) if !observation.green => {
            let mut output_tail = observation.output_tail.clone();
            truncate_to_tail_on_boundary(&mut output_tail, CHECK_OUTPUT_TAIL_BYTES);
            format!(
                "Latest check `{command}`{}: FAIL\nLatest failing output (bounded tail):\n{output_tail}",
                check_ran_in_label(observation)
            )
        }
        Some((command, observation)) => format!(
            "Latest check `{command}`{}: PASS",
            check_ran_in_label(observation)
        ),
        None => "No objective check command was configured for this turn.".to_string(),
    };

    format!(
        "{repair}\n\nPrior attempt edited paths (bounded):\n{paths}\n\n\
         {check}\n\nPrior attempt scoped diff (bounded):\n{diff}"
    )
}

/// Convert only typed provider transport failures into a bounded deep EXEC
/// retry contract. Authentication, context, permission, and tool-protocol
/// failures need to surface rather than repeatedly replaying an unchanged
/// request. The original typed error remains the terminal result at the
/// attempt cap.
fn exec_transport_retry_context(error: &StreamingTurnError) -> Option<String> {
    let signature = match error.provider_error_class()? {
        api::ProviderErrorClass::RateLimit { .. } => "provider_rate_limit",
        api::ProviderErrorClass::Transient => "provider_transient",
        _ => return None,
    };
    Some(format!(
        "The previous EXEC attempt failed before it completed ({signature}). This is an infrastructure failure, not a verifier rejection. Retry the same task from the current working tree; preserve valid prior edits and do not claim completion without verifying the result."
    ))
}

fn verification_outcome_note(
    scope: &str,
    decision: DeepDecision,
    attempt: u32,
    max_attempts: u32,
    objective_ok: bool,
    verifier: &VerifierVerdict,
) -> String {
    let objective = if objective_ok {
        "objective ok"
    } else {
        "objective red"
    };
    // Codex-style work citation: show WHAT the verifier checked next to its
    // verdict, so one glance at the note replaces a second verification round.
    let evidence = verifier
        .evidence
        .as_deref()
        .map(|cited| format!(" · checked: {cited}"))
        .unwrap_or_default();
    let verifier = verifier_display_summary(verifier);
    match decision {
        DeepDecision::Accept => {
            format!("{scope}: accepted — verification passed ({objective}; {verifier}){evidence}")
        }
        DeepDecision::Retry => format!(
            "{scope}: retrying — {verifier} ({objective}; attempt {attempt}/{max_attempts}){evidence}"
        ),
        DeepDecision::GiveUp => {
            format!("{scope}: stopped — out of attempts ({objective}; {verifier}){evidence}")
        }
    }
}

fn verifier_display_summary(verifier: &VerifierVerdict) -> String {
    let mode = verifier_mode_label(verifier.parse);
    if verifier.accepted {
        return format!("{mode} accepted");
    }
    if !verifier.issues.is_empty() {
        return format!("{mode} found {}", issue_count_label(verifier.issues.len()));
    }
    match verifier.parse {
        VerifierParse::Json | VerifierParse::Salvaged => format!("{mode} rejected"),
        VerifierParse::Empty => "verifier returned no output".to_string(),
        VerifierParse::Unparseable => "verifier returned no usable verdict".to_string(),
        VerifierParse::Timeout => "verifier timed out".to_string(),
        VerifierParse::BudgetExhausted => {
            "verifier hit its inspection budget; objective check gated".to_string()
        }
    }
}

const fn verifier_mode_label(parse: VerifierParse) -> &'static str {
    match parse {
        VerifierParse::Json => "strict verifier",
        VerifierParse::Salvaged => "salvaged verifier",
        VerifierParse::Empty
        | VerifierParse::Unparseable
        | VerifierParse::Timeout
        | VerifierParse::BudgetExhausted => "verifier",
    }
}

fn issue_count_label(count: usize) -> String {
    if count == 1 {
        "1 issue".to_string()
    } else {
        format!("{count} issues")
    }
}

/// One VERIFY fold's (objective, verifier) pairing, buffered in-process for
/// the host to drain via [`take_verifier_calibration_events`].
#[derive(Debug, Clone)]
pub struct VerifierCalibrationEvent {
    /// Unix milliseconds at fold time.
    pub ts_ms: u64,
    /// Session the VERIFY leg folded in.
    pub session_id: String,
    /// What the objective check observed (green = true).
    pub objective_ok: bool,
    /// What the verifier claimed.
    pub accepted: bool,
    /// Verdict parse class (`VerifierParse::as_str`).
    pub parse: &'static str,
}

/// Never-drained backstop: a host that installs no persist guard must not
/// grow the buffer for the life of the process.
const VERIFIER_CALIBRATION_BUFFER_CAP: usize = 4096;

fn verifier_calibration_buffer() -> &'static std::sync::Mutex<Vec<VerifierCalibrationEvent>> {
    static BUFFER: std::sync::OnceLock<std::sync::Mutex<Vec<VerifierCalibrationEvent>>> =
        std::sync::OnceLock::new();
    BUFFER.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

/// Drain every buffered VERIFY calibration event, oldest first. The host owns
/// persistence (zo-cli appends to `~/.zo/evidence/verifier-calibration.jsonl`
/// behind the same guard that persists harness attest); a process that never
/// drains simply discards the buffer at exit.
#[must_use]
pub fn take_verifier_calibration_events() -> Vec<VerifierCalibrationEvent> {
    std::mem::take(
        &mut *verifier_calibration_buffer()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    )
}

/// Verifier-calibration evidence: one event per VERIFY fold pairing what the
/// OBJECTIVE check observed with what the VERIFIER claimed. The matrix this
/// accumulates is the false-accept measurement the pillar map called missing:
/// `objectiveOk:false, accepted:true` is a verifier rubber-stamp the gate
/// caught — a measured LOWER BOUND on the false-accept rate (accepts of
/// semantically-wrong-but-tests-green changes stay invisible, and the event
/// says so by construction, not by claim). Kept out of the route-outcome
/// ledger on purpose: those records feed routing feedback, and calibration
/// counts would warp what they tune (the one-predicate-two-questions class).
///
/// Buffered in-process instead of appended straight to disk, mirroring the
/// harness-attest ledger: only a host that installs a persist guard writes
/// the file, so a dependent crate's test suite driving deep folds against
/// mock providers never pollutes the real ledger. (The direct-append version
/// wrote ~30 mock rows per `cargo test -p zo-cli` run — a runtime-side
/// `cfg(test)` seam cannot see a dependent crate's tests, which compile this
/// crate without `cfg(test)`.)
fn record_verifier_calibration(session_id: &str, objective_ok: bool, verdict: &VerifierVerdict) {
    let ts_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0);
    let mut buffer = verifier_calibration_buffer()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if buffer.len() >= VERIFIER_CALIBRATION_BUFFER_CAP {
        return;
    }
    buffer.push(VerifierCalibrationEvent {
        ts_ms,
        session_id: session_id.to_string(),
        objective_ok,
        accepted: verdict.accepted,
        parse: verdict.parse.as_str(),
    });
}

/// Iteration budget for one VERIFY leg. Production distribution (223 legs,
/// 2026-08-07): p50 3, p90 7, p99 19, max 51 — the cap leaves the typical
/// inspection untouched and kills the runaway tail (a 51-iteration leg spends
/// dozens of model calls re-reading a diff it was seeded with). Hitting the
/// cap is graceful: the loop ends `BudgetExhausted::Iterations` well-formed,
/// and the caller folds it into [`verify_budget_exhausted_verdict`] instead
/// of parsing the synthetic closer as a verdict.
const VERIFY_LEG_MAX_ITERATIONS: usize = 12;

/// The verdict for a VERIFY leg that hit [`VERIFY_LEG_MAX_ITERATIONS`] before
/// emitting one. Distinct from [`verify_leg_failed_verdict`] (transient
/// failure, where a retry can plausibly verify next time): a budget spiral is
/// a property of the diff-vs-inspection pairing, so the same retry re-spirals
/// — `fold_verification_attempt` lets this class defer to the objective gate
/// rather than buying a 325-422k retry of the same walk.
fn verify_budget_exhausted_verdict() -> VerifierVerdict {
    VerifierVerdict {
        accepted: false,
        issues: Vec::new(),
        parse: VerifierParse::BudgetExhausted,
        evidence: None,
    }
}

/// The conservative verdict for a VERIFY sub-turn that did not produce a usable
/// verdict because the sub-turn itself failed (a transient streaming error). A
/// non-accept tagged `Timeout` so [`fold_verification_attempt`] retries (or
/// gives up at the cap) rather than the `?` it used to take throwing away the
/// EXEC edits that are already applied to the work tree. `Timeout` keeps the
/// display honest ("verifier timed out") and never gate-accepts.
fn verify_leg_failed_verdict() -> VerifierVerdict {
    VerifierVerdict {
        accepted: false,
        issues: Vec::new(),
        parse: VerifierParse::Timeout,
        evidence: None,
    }
}

/// Parse the VERIFY leg's final text into a verdict, refusing to read an
/// instruction ECHO as one.
///
/// The VERIFY prompt embeds PARSEABLE example verdict objects (deliberately —
/// the examples pin the exact contract `parse_lens_verifier` reads, see
/// `verify_prompt_examples_are_strict_parseable_json`). That makes an echo
/// self-certifying: a verifier that parrots its instructions back reproduces
/// the accept example, and `last_complete_lens`'s embedded-object fallback
/// reads that EXAMPLE as a full three-lens accept. A live cross-model
/// verifier did exactly this and rubber-stamped the turn as "strict verifier
/// accepted". The contract response is a single-line JSON object, which can
/// never contain the leg's own `[deep:VERIFY]` marker — so matching the
/// marker here can only ever catch echoes, and an echo is judged
/// conservatively as not-accepted with an explicit issue the retry loop can
/// show.
fn parse_verify_leg_text(text: &str) -> VerifierVerdict {
    if text.contains(DEEP_VERIFY_MARKER) {
        return VerifierVerdict {
            accepted: false,
            issues: vec![
                "verifier echoed its instructions instead of judging the change; no verdict was produced".to_string(),
            ],
            parse: VerifierParse::Unparseable,
            evidence: None,
        };
    }
    parse_lens_verifier(text)
}

/// Whether a completed turn actually edited the workspace — true when any
/// non-error tool result came from a write-class tool. Reactive verification
/// only engages when this holds, so chat/analysis turns are never taxed.
fn made_edits(summary: &TurnSummary) -> bool {
    summary.tool_results.iter().any(|message| {
        message.blocks.iter().any(|block| {
            matches!(
                block,
                ContentBlock::ToolResult { tool_name, is_error, .. }
                    if !is_error && super::is_edit_or_write_tool(tool_name)
            )
        })
    })
}

/// Command substrings that mark a bash call as a build/test/run check whose
/// green exit is worth reporting to the verifier. Aligned with
/// [`detect_check_command`]'s per-ecosystem vocabulary, plus the direct
/// script-run shapes implementers actually type (`python3 test_x.py`,
/// `cargo run` as an output check).
const EXEC_CHECK_COMMAND_MARKERS: &[&str] = &[
    "cargo build",
    "cargo check",
    "cargo clippy",
    "cargo test",
    "cargo run",
    "npm test",
    "npm run",
    "yarn test",
    "pnpm test",
    "pytest",
    "python -m",
    "python3 -m",
    "python test",
    "python3 test",
    "python tests",
    "python3 tests",
    "go build",
    "go test",
    "go vet",
    "go run",
    "dotnet build",
    "dotnet test",
    "make test",
    "make check",
    "make build",
    "tsc",
    "./gradlew",
    "mvn test",
];

pub(super) fn command_is_check_shaped(command: &str) -> bool {
    EXEC_CHECK_COMMAND_MARKERS
        .iter()
        .any(|marker| command.contains(marker))
        || command_is_assert_heredoc(command)
}

/// The other shape a self-verification takes: an inline interpreter heredoc
/// whose body asserts something (`python3 - <<'EOF' … assert … EOF`).
///
/// Measured on the deep lanes this is the DOMINANT check shape — the model
/// writes an ad-hoc assert script per stage rather than a `pytest` run — and
/// the marker list above cannot name it because the command line carries no
/// tool name, just `python3 -`. Without this arm the verified-state ledger
/// records nothing in exactly the sessions it was built for (observed live:
/// ten turns, an empty ledger, every injection declined `no_green_checks`).
///
/// The `assert`/`raise` requirement keeps data-transform heredocs (scripts
/// that only compute or write) out: a green transform proves nothing worth
/// carrying, and recording one would let a tree-mutating script masquerade as
/// a still-true check. A script that both asserts and writes files can still
/// slip in — a known limit, mitigated by the observation's own wording (the
/// harness never claims coverage, it lists edits and lets the model judge).
fn command_is_assert_heredoc(command: &str) -> bool {
    let Some(heredoc) = command.find("<<") else {
        return false;
    };
    let opener = &command[..heredoc];
    let opener_is_interpreter = ["python3", "python"].iter().any(|interpreter| {
        opener
            .split_whitespace()
            .any(|word| word == *interpreter || word.ends_with(&format!("/{interpreter}")))
    });
    if !opener_is_interpreter {
        return false;
    }
    let body = &command[heredoc..];
    body.contains("assert") || body.contains("raise ")
}

/// `true` when a non-error bash tool result records a foreground command that
/// ran to completion with exit 0. The bash tool encodes a non-zero exit as
/// `returnCodeInterpretation: "exit_code:N"` (absent on success), a timeout as
/// `interrupted: true`, and a backgrounded launch as `backgroundTaskId` — a
/// background start proves nothing about the command's outcome. The `stdout`
/// key doubles as a shape check so arbitrary JSON from another tool can never
/// read as a green check.
pub fn bash_result_exited_zero(output: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(output) else {
        return false;
    };
    value.get("stdout").is_some_and(serde_json::Value::is_string)
        && value
            .get("returnCodeInterpretation")
            .is_none_or(serde_json::Value::is_null)
        && value.get("interrupted").and_then(serde_json::Value::as_bool) != Some(true)
        && value
            .get("backgroundTaskId")
            .is_none_or(serde_json::Value::is_null)
}

/// Check-shaped bash commands this attempt's EXEC leg ran to completion with
/// exit 0 AFTER its last successful edit — runtime-observed IO facts (the
/// executor recorded the exit), NOT model claims. Injected into the VERIFY
/// prompt so the verifier can cite the implementer's own green run instead of
/// attempting a build/test itself: the verify phase's bash grant is read-only,
/// so such an attempt burns a model round trip only to be denied (observed on
/// the bench: the verifier tried `cargo run --quiet`, was denied, and the
/// implementer's green `cargo build && cargo run` sat unreported in the same
/// transcript). A check that ran BEFORE the last edit is stale evidence and is
/// dropped.
/// Ready-to-embed section carrying the CURRENT post-edit content of each
/// changed file, read from disk by the harness at leg-build time — the same
/// IO-fact pattern as [`exec_green_checks`]. Injected only into `SingleLens`
/// prompts so the common small-green-diff leg can verdict in ONE call instead
/// of spending a round trip on `read_file`s whose bytes the diff or the
/// conversation already carried (measured: that read was the leg's only tool
/// call in both long-lane iterative trials). Oversized, unreadable, or
/// non-UTF-8 files get a skip note pointing the verifier at `read_file`;
/// an empty `diff_paths` yields an empty string, leaving the prompt
/// byte-identical.
fn verify_file_attachments(diff_paths: &[String], own_root: Option<&std::path::Path>) -> String {
    use std::fmt::Write as _;
    if diff_paths.is_empty() {
        return String::new();
    }
    let mut blocks = String::new();
    let mut attached_total = 0usize;
    for path in diff_paths {
        let resolved = attempt_path_key(path, own_root);
        let note = match std::fs::read(&resolved) {
            Ok(bytes) if bytes.len() > VERIFY_FILE_ATTACH_PER_FILE_BYTES => {
                Some("too large to attach — read_file it if you need it")
            }
            Ok(bytes) if attached_total + bytes.len() > VERIFY_FILE_ATTACH_TOTAL_BYTES => {
                Some("attachment budget spent — read_file it if you need it")
            }
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(content) => {
                    attached_total += content.len();
                    let _ = write!(
                        blocks,
                        "──── FILE: {path} (attached in full, {len} bytes) ────\n{content}\n──── END FILE: {path} ────\n",
                        len = content.len(),
                    );
                    None
                }
                Err(_) => Some("binary or non-UTF-8 — read_file it if you need it"),
            },
            Err(_) => Some("unreadable or deleted"),
        };
        if let Some(note) = note {
            let _ = writeln!(blocks, "──── FILE: {path} (not attached: {note}) ────");
        }
    }
    format!(
        "\n\nCurrent post-edit content of every changed file, read from disk by the harness \
         AFTER the last edit (runtime-recorded IO facts, byte-identical to what `read_file` \
         would return — do NOT re-read attached files):\n{blocks}\
         If these contents plus the observations above already settle every requirement, \
         deliver your verdict JSON in THIS response without any tool calls."
    )
}

/// Files the EXEC leg consulted this turn: successful `read_file` targets
/// plus every edited/written path. The verifier's remaining round trip was
/// cross-checking the change against context the implementer had consulted
/// but this attempt did not touch (measured: a decision-carry verifier
/// `read_file`ing the stage-1 `DECISIONS.md`) — so the attachment set must
/// cover what the implementer SAW, not just what it changed.
fn exec_consulted_paths(summary: &TurnSummary) -> Vec<String> {
    let mut read_targets: std::collections::HashMap<&str, String> =
        std::collections::HashMap::new();
    for message in &summary.assistant_messages {
        for block in &message.blocks {
            if let ContentBlock::ToolUse { id, name, input } = block {
                if name == "read_file" {
                    if let Some(path) = serde_json::from_str::<serde_json::Value>(input)
                        .ok()
                        .as_ref()
                        .and_then(|value| value.get("path"))
                        .and_then(serde_json::Value::as_str)
                        .map(str::trim)
                        .filter(|path| !path.is_empty())
                    {
                        read_targets.insert(id.as_str(), path.to_string());
                    }
                }
            }
        }
    }
    let mut paths: Vec<String> = Vec::new();
    for message in &summary.tool_results {
        for block in &message.blocks {
            let ContentBlock::ToolResult {
                tool_use_id,
                is_error,
                ..
            } = block
            else {
                continue;
            };
            if *is_error {
                continue;
            }
            if let Some(path) = read_targets.get(tool_use_id.as_str()) {
                if !paths.contains(path) {
                    paths.push(path.clone());
                }
            }
        }
    }
    for path in edited_file_paths(summary) {
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    paths
}

/// The attachment set for a `SingleLens` leg: this attempt's diff paths first
/// (they own the byte budget), then the other files EXEC consulted, deduped
/// by resolved location so one file under two spellings attaches once.
fn verify_attachment_paths(
    diff_paths: &[String],
    consulted: &[String],
    own_root: Option<&std::path::Path>,
) -> Vec<String> {
    let mut seen: BTreeSet<std::path::PathBuf> = BTreeSet::new();
    let mut ordered: Vec<String> = Vec::new();
    for path in diff_paths.iter().chain(consulted.iter()) {
        if seen.insert(attempt_path_key(path, own_root)) {
            ordered.push(path.clone());
        }
    }
    ordered
}

fn exec_green_checks(summary: &TurnSummary) -> Vec<String> {
    let mut commands: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    for message in &summary.assistant_messages {
        for block in &message.blocks {
            if let ContentBlock::ToolUse { id, name, input } = block {
                if name == "bash" {
                    commands.insert(id.as_str(), input.as_str());
                }
            }
        }
    }
    let mut checks: Vec<String> = Vec::new();
    for message in &summary.tool_results {
        for block in &message.blocks {
            let ContentBlock::ToolResult {
                tool_use_id,
                tool_name,
                output,
                is_error,
                ..
            } = block
            else {
                continue;
            };
            if *is_error {
                continue;
            }
            // Evidence must postdate the last mutation: a green check that ran
            // before a later edit validated a different tree.
            if super::is_edit_or_write_tool(tool_name) {
                checks.clear();
                continue;
            }
            if tool_name != "bash" || !bash_result_exited_zero(output) {
                continue;
            }
            let Some(command) = commands
                .get(tool_use_id.as_str())
                .and_then(|input| serde_json::from_str::<serde_json::Value>(input).ok())
                .and_then(|value| {
                    value
                        .get("command")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                })
            else {
                continue;
            };
            if !command_is_check_shaped(&command) {
                continue;
            }
            let mut command = command;
            truncate_on_boundary(&mut command, EXEC_CHECK_COMMAND_BYTES);
            if !checks.contains(&command) {
                checks.push(command);
            }
        }
    }
    // Newest evidence wins the cap: the last commands run are the closest to
    // the final tree state.
    if checks.len() > EXEC_CHECK_MAX_COMMANDS {
        checks.drain(..checks.len() - EXEC_CHECK_MAX_COMMANDS);
    }
    checks
}

/// Display cap for one harvested check command inside the VERIFY prompt, and
/// the storage cap the session ledger applies to the same commands
/// (`conversation::verified_state`) so both surfaces truncate identically.
pub(super) const EXEC_CHECK_COMMAND_BYTES: usize = 240;
/// At most this many observed commands are reported (newest kept).
const EXEC_CHECK_MAX_COMMANDS: usize = 3;

/// The green-check evidence one VERIFY leg may cite, by provenance.
///
/// Two sources, kept apart all the way to the prompt because they answer
/// different questions and a verifier that conflated them would over-credit the
/// attempt: `this_turn` is what THIS attempt ran after its own last edit, while
/// `session` is what earlier turns ran and the ledger still holds as
/// uninvalidated. Bundling them also keeps the prompt builders' argument lists
/// from growing a second bare `&[String]` that a caller could swap by mistake —
/// the two are indistinguishable to the type system, and getting them backwards
/// would silently relabel this attempt's evidence as an earlier turn's.
#[derive(Debug, Clone, Copy, Default)]
struct GreenCheckEvidence<'a> {
    /// [`exec_green_checks`] for this attempt: post-last-edit, exit 0.
    this_turn: &'a [String],
    /// Session-ledger checks with nothing recorded as edited since
    /// (`ConversationRuntime::verify_leg_session_checks`).
    session: &'a [String],
}

fn task_with_retry_context(task: &str, retry: Option<&str>) -> String {
    match retry {
        Some(retry) if !retry.trim().is_empty() => {
            format!("{task}\n\nLatest repair/update context:\n{retry}")
        }
        _ => task.to_string(),
    }
}

/// Leg-prompt markers — the single authority BOTH sides bind to: the prompt
/// builders below prefix their instructions with these, and every consumer
/// that must RECOGNIZE a leg prompt (the echo guard `parse_verify_leg_text`,
/// zo-cli's transcript-seed visibility filter) matches the same constants.
/// As free literals, a marker edited at its prompt site silently orphaned
/// the recognizers — a guard/filter kept matching a string no prompt
/// produced any more. The VALUES are load-bearing beyond this build: saved
/// transcripts carry them, and the resume-time visibility filter reads those
/// back — so changing one is a compatibility decision, pinned by literal
/// (not constant-referencing) tests.
pub const DEEP_PLAN_MARKER: &str = "[deep:PLAN]";
pub const DEEP_EXEC_MARKER: &str = "[deep:EXEC]";
pub const DEEP_VERIFY_MARKER: &str = "[deep:VERIFY]";

// See `ConversationRuntime::set_quota_preflight_clean_for_this_thread`.
#[cfg(test)]
thread_local! {
    static QUOTA_PREFLIGHT_CLEAN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
pub const AUTO_RETRY_MARKER: &str = "[auto:RETRY]";

/// Delegation tools a bounded PLAN/VERIFY leg may not call — the single
/// authority for both sides of that rule: the leg prompts name these tools in
/// prose, and [`PermissionPolicy::begin_phase_tool_block`] enforces the same
/// list at authorize time.
///
/// It is a list of names rather than a permission mode because these tools only
/// require `ReadOnly`, so the phase's read-only clamp does not reach them. It is
/// enforced at execution rather than by narrowing the leg's advertised tools
/// because the advertised set is part of the cached prompt prefix: dropping four
/// definitions for one leg and restoring them on the next turn re-bills the
/// entire conversation behind them, twice per leg.
pub const DEEP_LEG_DELEGATION_TOOLS: &[&str] =
    &["Agent", "SpawnMultiAgent", "Workflow", "SendMessage"];

/// The reactive retry prompt: restate the failure repair contract and the
/// current request context so the next attempt fixes the rejected change.
fn reactive_retry_prompt(task: &str, repair: &str) -> String {
    format!(
        "{AUTO_RETRY_MARKER} Your previous change did not pass verification.\n\n{repair}\n\n\
         Fix every issue above before stopping; treat the objective check and verifier findings as blocking. Current request context:\n{task}\n\n\
         Edit only what the fix requires; do not weaken, modify, or delete tests to force a pass."
    )
}

/// The read-only `bash` inspection commands permitted during a write-capable
/// session's PLAN/VERIFY phases (see `deep_subturn`). Single owner of the
/// allowlist. Each entry is an OpenCode-style `tool(<glob>)` allow rule matched
/// against the tool's permission subject — `bash(<glob>)` against the command,
/// and `Cargo(<verb>)` against the typed `action` so the shell-free `Cargo`
/// tool gets the same read-only inspection relaxation as `bash "cargo …"`.
/// Restricted to inspection verbs (build, status, search, read); destructive
/// `bash` outside these stays gated by `bash`'s `DangerFullAccess` requirement.
/// A glob `*` can still span shell chaining, so this is a pragmatic relaxation
/// for a user-initiated full-access session's read-only phases, not a sandbox.
#[must_use]
pub fn read_only_bash_allow_rules() -> &'static [&'static str] {
    &[
        "bash(cargo check*)",
        "bash(cargo test*)",
        "bash(cargo clippy*)",
        "bash(cargo fmt*)",
        "bash(cargo build*)",
        "bash(cargo metadata*)",
        "bash(git status*)",
        "bash(git diff*)",
        "bash(git log*)",
        "bash(git show*)",
        "bash(git branch*)",
        "bash(pwd)",
        "bash(diff *)",
        "bash(ls *)",
        "bash(ls)",
        "bash(cat *)",
        "bash(rg *)",
        "bash(grep *)",
        "bash(find *)",
        "bash(head *)",
        "bash(tail *)",
        "bash(wc *)",
        "bash(echo *)",
        // Typed-action equivalents of the read-only `cargo` verbs above. `Cargo`
        // requires WorkspaceWrite (it writes `target/`), so under a downgraded
        // ReadOnly phase it is denied unless explicitly allowed here — the same
        // relaxation the `bash(cargo …)` rules grant the shell form. Subjects
        // are the discrete `action` verb (see `extract_permission_subject`), so
        // each rule names one verb. Inspection verbs only: `run`/`build` (which
        // execute arbitrary or heavier writes) stay gated. `Git` is already
        // ReadOnly, so it needs no scoped grant.
        "Cargo(check)",
        "Cargo(test)",
        "Cargo(clippy)",
        "Cargo(fmt)",
    ]
}

/// Harvest the newest distinct, valid tool-produced images from one EXEC leg.
///
/// The caller supplies only the message suffix created by that leg, excluding
/// both earlier session history and the user's original image attachments.
/// Images remain newest-first in the returned verifier packet.
fn harvest_exec_tool_images(messages: &[ConversationMessage]) -> Vec<(String, String)> {
    let mut images = Vec::new();
    let mut seen_payloads = HashSet::new();
    for message in messages.iter().rev() {
        for block in message.blocks.iter().rev() {
            let ContentBlock::ToolResult {
                images: tool_images,
                ..
            } = block
            else {
                continue;
            };
            for (media_type, data) in tool_images.iter().rev() {
                if !seen_payloads.insert(data.as_str()) {
                    continue;
                }
                let Some(image) = guard_verify_image(media_type, data) else {
                    continue;
                };
                images.push(image);
                if images.len() == VERIFY_IMAGE_MAX_COUNT {
                    return images;
                }
            }
        }
    }
    images
}

fn harvest_subturn_images(
    phase: DeepSubturnPhase,
    messages: &[ConversationMessage],
) -> Vec<(String, String)> {
    if phase == DeepSubturnPhase::Exec {
        harvest_exec_tool_images(messages)
    } else {
        Vec::new()
    }
}

/// Decode, validate, and apply the ordinary raw-byte dimension guard before an
/// EXEC image is copied into a VERIFY user packet.
fn guard_verify_image(media_type: &str, data: &str) -> Option<(String, String)> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data.as_bytes())
        .ok()?;
    match crate::image_guard::guard_image_bytes(&bytes) {
        crate::image_guard::ImageGuardOutcome::Keep => {
            // `guard_image_bytes` deliberately keeps unreadable input because
            // the ordinary history path must not destroy an unproven payload.
            // Forwarding is stricter: VERIFY needs real visual evidence, so a
            // malformed/non-image payload is not useful and must be dropped.
            image::ImageReader::new(Cursor::new(bytes.as_slice()))
                .with_guessed_format()
                .ok()?
                .into_dimensions()
                .ok()?;
            Some((media_type.to_string(), data.to_string()))
        }
        crate::image_guard::ImageGuardOutcome::Rescaled { media_type, bytes } => Some((
            media_type,
            base64::engine::general_purpose::STANDARD.encode(bytes),
        )),
        crate::image_guard::ImageGuardOutcome::DropOversized { .. } => None,
    }
}

struct DeepSubturnPermissionGuard<'a, C, T> {
    runtime: &'a mut ConversationRuntime<C, T>,
    saved_mode: PermissionMode,
    bash_grant: Option<TemporaryAllowGrant>,
    /// Prior conversation messages while a cross-model VERIFY or swapped EXEC
    /// runs on an isolated packet. Drop appends the leg's messages back so the
    /// existing parsing and rendering seams remain unchanged.
    saved_isolated_messages: Option<Arc<Vec<ConversationMessage>>>,
    /// `Some(prior)` when this sub-turn swapped a leg client (verify or
    /// implementer) into `async_api_client`; Drop restores `prior`, so a
    /// cancelled or errored leg can never leak its client into a later leg
    /// or turn.
    #[allow(
        clippy::option_option,
        reason = "tri-state: None = no swap performed, Some(prior) = restore prior (which may itself be None)"
    )]
    saved_async_client: Option<Option<Arc<dyn AsyncApiClient>>>,
    /// Which leg flag the swap set, cleared by Drop in lockstep with the client
    /// restore.
    swapped: Option<SubturnClient>,
    /// `true` for a native-client Architect EXEC leg; cleared on Drop even
    /// though no client swap occurred.
    native_exec_leg: bool,
    phase: DeepSubturnPhase,
    /// Message index immediately before this leg's user packet is appended.
    /// EXEC harvesting scans only this suffix while the guard still owns the
    /// pre-splice session.
    message_start: usize,
    /// Stashed before Drop restores/splices the session, then moved into
    /// [`DeepSubturnResult`].
    verify_images: Vec<(String, String)>,
    /// `Some(prior_list)` when this phase installed the delegation block
    /// ([`DEEP_LEG_DELEGATION_TOOLS`]); Drop restores the prior list so a
    /// nested phase cannot clear an outer one's block.
    saved_phase_tool_block: Option<Vec<String>>,
    /// `Some(prior)` when a VERIFY leg clamped the runtime's iteration budget
    /// to [`VERIFY_LEG_MAX_ITERATIONS`]; Drop restores it so the main turn
    /// (and later legs) keep their full budget.
    saved_max_iterations: Option<usize>,
}

impl<'a, C, T> DeepSubturnPermissionGuard<'a, C, T> {
    fn new(
        runtime: &'a mut ConversationRuntime<C, T>,
        mode: PermissionMode,
        client: SubturnClient,
        phase: DeepSubturnPhase,
    ) -> Self {
        runtime.deep_subturn_depth += 1;
        // `begin_phase_clamp` (vs plain `set_active_mode`) records the
        // stronger base mode so a mutating-tool denial during PLAN/VERIFY
        // names the phase clamp instead of telling the model to ask the user
        // for a permission the session already has.
        let saved_mode = runtime.permission_policy.begin_phase_clamp(mode);
        // When this phase downgrades a write-capable base mode to ReadOnly (the
        // PLAN/VERIFY phases of a full-access `/goal`/`/loop` turn), grant a
        // small read-only `bash` allowlist so inspection commands (cargo / git
        // status / rg ...) are not denied with the confusing "requires
        // danger-full-access; current mode is read-only". Removed by Drop, so a
        // cancelled sub-turn cannot leak the transient allowlist.
        let bash_grant = (mode == PermissionMode::ReadOnly
            && saved_mode.satisfies(PermissionMode::WorkspaceWrite))
        .then(|| {
            runtime
                .permission_policy
                .add_temporary_allow_rules(read_only_bash_allow_rules())
        });
        // Leg-scoped client swap. Marking the leg flag makes the
        // quota-fallback override defer to the swapped client for the
        // duration of this sub-turn (restored in Drop alongside the client
        // swap, so the two stay in lockstep).
        let native_exec_leg = client == SubturnClient::NativeExec;
        if native_exec_leg {
            runtime.exec_native_leg_active = true;
        }
        let swap: Option<(Arc<dyn AsyncApiClient>, String)> = match client {
            SubturnClient::Native | SubturnClient::NativeExec => None,
            SubturnClient::Plan => runtime.deep_plan_client.clone(),
            // `verify_subturn` picks which ranked candidate this leg runs on
            // by setting `deep_verify_candidate_idx` first; swap that one in.
            SubturnClient::Verify => runtime
                .deep_verify_candidates
                .get(runtime.deep_verify_candidate_idx)
                .cloned(),
            SubturnClient::Implementer => runtime.exec_contract.as_ref().and_then(|contract| {
                contract
                    .impl_client
                    .clone()
                    .map(|impl_client| (impl_client, contract.impl_model.clone()))
            }),
        };
        // Only VERIFY runs isolated. It has to judge the diff on its own terms,
        // so the conversation that produced the diff is exactly what it must not
        // see.
        //
        // The Implementer used to be isolated too, which meant the leg that
        // writes the user-visible artifact ran with the entire conversation
        // replaced by an empty vec: in reactive mode its prompt is the raw user
        // sentence, so "now make the hero darker" reached the writer with no
        // referent for either "the hero" or "darker". Isolation is right for a
        // judge and wrong for an author.
        let saved_isolated_messages = (client == SubturnClient::Verify)
            .then(|| std::mem::replace(&mut runtime.session.messages, Arc::new(Vec::new())));
        let message_start = runtime.session.messages.len();
        let swapped = swap.is_some().then_some(client);
        // A leg on its own client is a model switch the router never saw:
        // tell the switch observer, AFTER the isolation above, so the context
        // it prices is what the leg actually sends (a VERIFY leg sends its
        // prompt alone). `plan_shadow` files the row; the runtime keeps none.
        if let Some((_, leg_model)) = &swap {
            let trigger = match client {
                SubturnClient::Plan => Some(crate::SwitchTrigger::PlanLeg),
                SubturnClient::Verify => Some(crate::SwitchTrigger::VerifyLeg),
                SubturnClient::Implementer => Some(crate::SwitchTrigger::ExecLeg),
                SubturnClient::Native | SubturnClient::NativeExec => None,
            };
            if let Some(trigger) = trigger {
                let main = runtime.context_model.clone().unwrap_or_default();
                runtime.note_model_switch(trigger, &main, leg_model);
            }
        }
        let saved_async_client = swap.map(|(client_arc, _)| {
            match client {
                SubturnClient::Plan => runtime.deep_plan_leg_active = true,
                SubturnClient::Verify => runtime.deep_verify_leg_active = true,
                SubturnClient::Implementer => runtime.exec_impl_leg_active = true,
                SubturnClient::Native | SubturnClient::NativeExec => {}
            }
            runtime.async_api_client.replace(client_arc)
        });
        // PLAN and VERIFY must reason for themselves; EXEC is the leg that is
        // allowed to fan work out. Enforced here, at authorize time, rather than
        // by hiding the tools from the leg's advertised set: see
        // `begin_phase_tool_block` for what the hidden-tools version cost in
        // prompt cache. Both dispatch paths (sync and streaming) authorize
        // through the same policy, so one install covers both.
        let saved_phase_tool_block = matches!(
            phase,
            DeepSubturnPhase::Plan | DeepSubturnPhase::Verify
        )
        .then(|| {
            runtime
                .permission_policy
                .begin_phase_tool_block(DEEP_LEG_DELEGATION_TOOLS)
        });
        let saved_max_iterations = matches!(phase, DeepSubturnPhase::Verify).then(|| {
            let saved = runtime.max_iterations;
            runtime.max_iterations = saved.min(VERIFY_LEG_MAX_ITERATIONS);
            saved
        });
        Self {
            runtime,
            saved_mode,
            bash_grant,
            saved_isolated_messages,
            saved_async_client,
            swapped,
            native_exec_leg,
            phase,
            message_start,
            verify_images: Vec::new(),
            saved_phase_tool_block,
            saved_max_iterations,
        }
    }
}

impl<C, T> DeepSubturnPermissionGuard<'_, C, T>
where
    C: ApiClient,
    T: ToolExecutor,
{
    async fn run(
        mut self,
        prompt: String,
        images: Vec<(String, String)>,
        render_tx: mpsc::Sender<RenderBlock>,
        prompter: Arc<dyn AsyncPermissionPrompter>,
    ) -> Result<DeepSubturnResult, StreamingTurnError> {
        // A VERIFY leg is an internal judge: on resume its whole turn is
        // policy-hidden (`seed_user_visibility` → `HideTurn`), but the live
        // stream used to disagree and dump the judge's reasoning, tool rows,
        // and raw verdict JSON into the transcript. Live now matches the
        // resume policy — the leg streams into a drain, while the gate's own
        // `auto: verifying…` notices narrate progress and permission prompts
        // ride the separate prompter channel. PLAN/EXEC keep streaming: their
        // output is user-facing work, and resume hides only their prompts.
        let render_tx = if matches!(self.phase, DeepSubturnPhase::Verify) {
            let (drain_tx, mut drain_rx) = mpsc::channel(64);
            tokio::spawn(async move { while drain_rx.recv().await.is_some() {} });
            drain_tx
        } else {
            render_tx
        };
        let summary = self
            .runtime
            .run_internal_subturn_streaming_with_images(prompt, images, render_tx, prompter)
            .await?;
        let leg_messages = self
            .runtime
            .session
            .messages
            .get(self.message_start..)
            .unwrap_or_default();
        self.verify_images = harvest_subturn_images(self.phase, leg_messages);
        Ok(DeepSubturnResult {
            summary,
            verify_images: std::mem::take(&mut self.verify_images),
        })
    }
}

impl<C, T> Drop for DeepSubturnPermissionGuard<'_, C, T> {
    fn drop(&mut self) {
        self.runtime.deep_subturn_depth = self.runtime.deep_subturn_depth.saturating_sub(1);
        if let Some(mut prior) = self.saved_isolated_messages.take() {
            let leg_messages =
                std::mem::replace(&mut self.runtime.session.messages, Arc::new(Vec::new()));
            Arc::make_mut(&mut prior).extend(leg_messages.iter().cloned());
            self.runtime.session.messages = prior;
        }
        if let Some(grant) = self.bash_grant.take() {
            self.runtime
                .permission_policy
                .remove_temporary_allow_rules(grant);
        }
        if let Some(prior) = self.saved_async_client.take() {
            self.runtime.async_api_client = prior;
            match self.swapped {
                Some(SubturnClient::Plan) => self.runtime.deep_plan_leg_active = false,
                Some(SubturnClient::Verify) => self.runtime.deep_verify_leg_active = false,
                Some(SubturnClient::Implementer) => self.runtime.exec_impl_leg_active = false,
                Some(SubturnClient::Native | SubturnClient::NativeExec) | None => {}
            }
        }
        if self.native_exec_leg {
            self.runtime.exec_native_leg_active = false;
        }
        if let Some(saved) = self.saved_phase_tool_block.take() {
            self.runtime
                .permission_policy
                .end_phase_tool_block(saved);
        }
        if let Some(saved) = self.saved_max_iterations.take() {
            self.runtime.max_iterations = saved;
        }
        self.runtime
            .permission_policy
            .end_phase_clamp(self.saved_mode);
    }
}

/// The PLAN-phase prompt. Forces a structured plan whose four headers match
/// [`decision_core::deep_lane::REQUIRED_PLAN_SECTIONS`], so [`validate_plan`]
/// can confirm it before any edit is allowed.
fn plan_prompt(task: &str, baseline: Option<&str>, missing: &[String]) -> String {
    let mut s = format!(
        "{DEEP_PLAN_MARKER} You are in the PLANNING phase of a deliberate change. Do NOT edit any files \
         yet — use read-only tools (read, grep, list) to inspect the repository, then write a \
         concrete plan. You MUST do this planning YOURSELF, inline. Do NOT spawn sub-agents, \
         delegate, or call Agent, SpawnMultiAgent, Workflow, or SendMessage.\n\n",
    );
    let _ = write!(s, "Task:\n{task}\n\n");
    if let Some(baseline) = baseline {
        let _ = write!(s, "{baseline}\n\n");
    }
    if !missing.is_empty() {
        let _ = write!(
            s,
            "Your previous plan had missing, empty, or placeholder-only required sections: {}. Add concrete, non-placeholder content for each one.\n\n",
            missing.join(", ")
        );
    }
    s.push_str(
        "Produce a short markdown plan with EXACTLY these four section headers, in order. Each section must contain concrete, non-placeholder content; empty/TODO/TBD/N/A/none-only sections are invalid.\n\n\
         ## Target files\n\
         For each file you will change, say what changes. Treat this as a contract across files: a \
         field/type/signature introduced in one file must be threaded through every file and test \
         that consumes it.\n\n\
         ## Invariants\n\
         Behavior that must NOT change; public APIs/signatures to preserve.\n\n\
         ## Expected tests\n\
         Which tests must pass — and any test you must NOT modify or delete.\n\n\
         ## Risks\n\
         Edge cases, hidden invariants, and failure modes to watch.\n\n\
         Output ONLY the plan. No code, no edits.\n",
    );
    s
}

/// The IMPLEMENT-phase prompt, carrying the validated plan and (on a retry) the
/// failure repair contract.
fn exec_prompt(task: &str, plan: &str, retry: Option<&str>) -> String {
    let mut s = format!(
        "{DEEP_EXEC_MARKER} You are in the IMPLEMENTATION phase. Apply the change now, following the \
         plan.\n\n\
         Task:\n{task}\n\n\
         Plan (from the planning phase):\n{plan}\n"
    );
    if let Some(extra) = retry {
        let _ = write!(s, "\n{extra}\n");
        s.push_str(
            "\nRetry rules:\n\
             - Treat every failing check line and every verifier finding as blocking; do not stop while any remains true.\n\
             - If an Immediate mechanical edits section is present, apply those exact edits first, then rerun the failing check before broader rewrites.\n\
             - If repair hints list exact receiver replacements, apply those replacements unless the candidate is truly not in scope.\n\
             - Search intended files for stale symbols, wrong receiver/type names, and missed call sites when the task renames or threads an API.\n\
             - If the task threads options or a new argument, audit wrappers and cache paths for stale or mixed-mode results.\n",
        );
    }
    s.push_str(
        "\nRules:\n\
         - Edit only the files the plan targets.\n\
         - Preserve call receivers during renames: `thing.oldName(...)` should become `thing.newName(...)`, not `TypeName.newName(...)`, unless the task explicitly asks for a static/type call.\n\
         - Before stopping, any new identifier used as a call receiver must be imported, defined, or passed in that file.\n\
         - Do NOT modify, weaken, or delete tests to make them pass.\n\
         - If you are FIXING A BUG: first add a test (or assertion) that REPRODUCES it — one that FAILS on the current, unfixed code — then make it pass. A fix with no failing-first reproduction is how a plausible-but-wrong fix slips through.\n\
         - If a recent change already touched what you are now being asked to fix again, treat that prior change as SUSPECT, not ground truth — reproduce the symptom and confirm the real root cause before re-patching.\n\
         - Do NOT leave stray or scratch files in the repository.\n",
    );
    s
}

/// The VERIFY-phase prompt examples consumed by [`parse_lens_verifier`].
const VERIFY_JSON_ACCEPT_EXAMPLE: &str = r#"{"spec": true, "regression": true, "security": true, "issues": [], "evidence": "read diff + both call sites; ran `cargo test -p core` (ok, 42 passed)"}"#;
const VERIFY_JSON_REJECT_EXAMPLE: &str = r#"{"spec": false, "regression": true, "security": true, "issues": ["specific problem"], "evidence": "read diff; ran `cargo test -p core` (2 failed: parse_roundtrip)"}"#;
const VERIFY_SCALAR_ACCEPT_EXAMPLE: &str = r#"{"accepted": true, "issues": [], "evidence": "read the scoped diff and checked every task requirement"}"#;
const VERIFY_SCALAR_REJECT_EXAMPLE: &str = r#"{"accepted": false, "issues": ["specific unmet task requirement"], "evidence": "read the scoped diff and found the missing requirement"}"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VerifyLensMode {
    SpecOnly,
    Full,
}

fn latest_assistant_text(messages: &[ConversationMessage]) -> String {
    messages
        .iter()
        .rev()
        .find(|message| message.role == MessageRole::Assistant)
        .map(|message| {
            let mut out = String::new();
            for block in &message.blocks {
                if let ContentBlock::Text { text } = block {
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(text);
                }
            }
            out
        })
        .unwrap_or_default()
}

/// The visual-quality criteria the VERIFY leg applies on a turn the probe read
/// as [`RouteTaskIntent::Design`]. Shared by both depths so the single-lens and
/// three-lens forms grade against the identical rubric.
///
/// Phrased throughout as criteria for the ONE verifier already running this
/// sub-turn — never as an instruction to convene, assemble, or staff anything.
/// A VERIFY prompt that once asked for a "PANEL of three" made the verifier
/// literally spawn three sub-agents; the deep legs stopped advertising
/// Agent/Spawn/Workflow because of it, and this text must not read like a
/// roster and re-open that door.
///
/// The vocabulary deliberately mirrors [`crate::build_design_guidance_reminder`]
/// (token plan, the named AI-default clusters, the quality floor) so the
/// verifier grades against the exact contract the implementer was handed. The
/// nav-parity and Korean-line-breaking checks mirror the reminder's Korean-web
/// block for the same reason: blind judges kept finding both defects in shipped
/// zo output, so the grader must be able to name them.
const DESIGN_LENS_CRITERIA: &str = "\
- Token plan: if the turn stated one (named hex palette, typeface roles, a signature element), check the \
result against those exact values and flag every drift.\n\
- AI defaults: flag the result if it lands on one of these unless the user explicitly asked for it — warm \
cream (#F4F1EA) + serif display + terracotta accent; near-black with a lone acid-green or vermilion pop; \
purple-to-blue gradient on white; Inter or the system font for everything; emoji as section markers; \
rounded cards with an accent rail.\n\
- Hierarchy: one clear focal point, and type sizes/weights that actually separate levels instead of a flat \
wall.\n\
- Contrast: body text, labels, and interactive states stay legible on the backgrounds they actually sit \
on.\n\
- Spacing rhythm: a consistent scale with aligned edges — flag arbitrary one-off gaps, crowding, and \
orphaned elements.\n\
- Quality floor, where the artifact type makes it applicable: responsive down to small viewports, visible \
keyboard focus, reduced motion respected.\n\
- Small-viewport nav parity: every navigation destination reachable at desktop width must still be \
reachable at mobile width. Reject a mobile rule that hides a link with `display: none` (or equivalent) \
without reflowing it into a wrap, stack, or disclosure — that deletes the destination.\n\
- Korean line-breaking discipline: if the copy is Korean, `word-break: keep-all` must survive — reject a \
blanket `overflow-wrap: anywhere` / `word-break: break-all` that silently undoes it, and reject mid-word \
breaks in tight cells, buttons, and table headers.\n\
- Judge what the diff actually produces, not what the assistant claims about it, and name the file, \
selector, or token you checked.\n";

/// The design lens as it rides the three-lens form: an extra rubric for the
/// EXISTING `spec` key, never a fourth verdict channel. A design defect is
/// itemized in `issues` and rejects `spec`, so severity reaches the retry loop
/// through exactly the same path a correctness finding does.
fn full_lens_design_block(design: bool) -> String {
    if !design {
        return String::new();
    }
    format!(
        "\nThis turn's request was read as DESIGN work, so judge \"spec\" against the visual result too. \
         A design defect below is a spec rejection: itemize it in issues like any other unmet \
         requirement.\n{DESIGN_LENS_CRITERIA}"
    )
}

/// The bounded context every VERIFY form restates before its lens criteria,
/// built once so the forms below differ only in their rubric and verdict
/// contract.
struct VerifyPromptContext<'a> {
    task: &'a str,
    diff: &'a str,
    objective: String,
    paths: String,
    /// Ready-to-embed paragraph naming working-tree changes that predate the
    /// attempt (empty when the tree was clean). The verifier may run
    /// `git status`/`git diff` itself, and on a pre-dirty tree those commands
    /// cannot distinguish the attempt's edits from another task's leftovers —
    /// only the harness holds the baseline snapshot, so it must say which is
    /// which or the verifier judges someone else's work (observed: a
    /// plan-writing turn rejected over four files another worktree's task had
    /// left modified).
    preexisting: String,
    assistant_claim: String,
    /// Ready-to-embed post-edit file contents ([`verify_file_attachments`]);
    /// empty for Full-lens prompts, which stay byte-identical.
    attached_files: &'a str,
    visual_evidence: &'static str,
}

impl<'a> VerifyPromptContext<'a> {
    #[allow(clippy::too_many_arguments)] // bounded context facts, built in one place
    fn new(
        task: &'a str,
        diff: &'a str,
        check: Option<(&str, &CheckObservation)>,
        checks: GreenCheckEvidence<'_>,
        edited_paths: &[String],
        preexisting_dirty: &[String],
        assistant_claim: &str,
        attached_files: &'a str,
        has_images: bool,
    ) -> Self {
        let mut objective = match check {
            // Name the tree the check ran in: the diff may span a sibling
            // worktree while the check runs in this process's cwd, and an
            // unqualified "PASS" next to a foreign-tree diff reads as if the
            // check validated that tree.
            Some((cmd, observation)) => format!(
                "Objective check `{cmd}`{}: {}\nLatest check output (bounded tail):\n{}",
                check_ran_in_label(observation),
                if observation.green { "PASS" } else { "FAIL" },
                observation.output_tail
            ),
            None => "No objective check command was configured for this turn.".to_string(),
        };
        // The implementer's own green runs are objective evidence too: the
        // harness (not the model) recorded each exit 0. Reporting them stops
        // the verifier from spending a round trip attempting a build/test the
        // read-only phase will deny.
        if !checks.this_turn.is_empty() {
            objective.push_str(
                "\n\nCommands the implementer ran in this workspace AFTER its last edit, each \
                 observed by the harness to exit 0 (runtime-recorded facts, not model claims):\n",
            );
            for command in checks.this_turn {
                objective.push_str("- `");
                objective.push_str(command);
                objective.push_str("`\n");
            }
            objective.push_str(
                "Treat these as this attempt's execution evidence. Do NOT re-run build/test \
                 commands yourself — bash in this phase is read-only and such commands are \
                 denied; judge from the diff, the files you inspect, and these observations.",
            );
        }
        // Earlier turns' greens come SECOND and stay labelled as such: they are
        // weaker evidence about this diff (they predate it), and the ordering is
        // what stops a verifier reading them as this attempt's own run. Only
        // still-fresh ones reach here — the ledger withholds any command it has
        // recorded an edit after — so every line is a fact the leg can cite
        // without qualifying it.
        if !checks.session.is_empty() {
            objective.push_str(
                "\n\nAlso on record from earlier turns in this session, and still green — the \
                 harness observed each exit 0 and has recorded no edit since (runtime-recorded \
                 facts, not model claims):\n",
            );
            for command in checks.session {
                objective.push_str("- `");
                objective.push_str(command);
                objective.push_str("`\n");
            }
            objective.push_str(
                "These ran before this attempt, so they are standing evidence about the parts of \
                 the tree it did not touch — not coverage of this diff.",
            );
            if checks.this_turn.is_empty() {
                objective.push_str(
                    " Do NOT re-run build/test commands yourself — bash in this phase is \
                     read-only and such commands are denied; judge from the diff, the files you \
                     inspect, and these observations.",
                );
            }
        }
        let mut paths = if edited_paths.is_empty() {
            "(none reported)".to_string()
        } else {
            edited_paths
                .iter()
                .map(|path| format!("- {path}"))
                .collect::<Vec<_>>()
                .join("\n")
        };
        truncate_on_boundary(&mut paths, VERIFY_EDITED_PATHS_BYTES);
        let preexisting = if preexisting_dirty.is_empty() {
            String::new()
        } else {
            let mut listing = preexisting_dirty
                .iter()
                .map(|path| format!("- {path}"))
                .collect::<Vec<_>>()
                .join("\n");
            truncate_on_boundary(&mut listing, VERIFY_EDITED_PATHS_BYTES);
            format!(
                "Pre-existing working-tree changes NOT made by this attempt — `git status`/`git \
                 diff` will show them, but they belong to other work; do NOT judge them or \
                 attribute them to this attempt:\n{listing}\n\n"
            )
        };
        let mut assistant_claim = if assistant_claim.trim().is_empty() {
            "(no final assistant claim was produced)".to_string()
        } else {
            assistant_claim.to_string()
        };
        truncate_on_boundary(&mut assistant_claim, VERIFY_ASSISTANT_CLAIM_BYTES);
        Self {
            task,
            diff,
            objective,
            paths,
            preexisting,
            assistant_claim,
            attached_files,
            visual_evidence: if has_images {
                VERIFY_VISUAL_EVIDENCE_BLOCK
            } else {
                ""
            },
        }
    }
}

/// Build the VERIFY sub-turn instruction for this attempt's depth and intent.
///
/// `intent` selects the rubric, never the verdict contract: a
/// [`RouteTaskIntent::Design`] turn swaps the single lens for the design lens
/// and adds the design criteria to the three-lens `spec` key, while every other
/// intent produces byte-for-byte the pre-design-axis prompt.
#[allow(
    clippy::too_many_arguments,
    reason = "the verifier packet keeps each independently bounded evidence field explicit"
)]
fn verify_prompt(
    task: &str,
    diff: &str,
    check: Option<(&str, &CheckObservation)>,
    checks: GreenCheckEvidence<'_>,
    edited_paths: &[String],
    preexisting_dirty: &[String],
    assistant_claim: &str,
    attached_files: &str,
    lens_mode: VerifyLensMode,
    intent: RouteTaskIntent,
    has_images: bool,
) -> String {
    // Attachments are a SingleLens-only optimization; the Full-lens prompt
    // stays byte-identical regardless of what the caller computed.
    let attached_files = if lens_mode == VerifyLensMode::Full {
        ""
    } else {
        attached_files
    };
    let context = VerifyPromptContext::new(
        task,
        diff,
        check,
        checks,
        edited_paths,
        preexisting_dirty,
        assistant_claim,
        attached_files,
        has_images,
    );
    let design = intent == RouteTaskIntent::Design;
    match lens_mode {
        VerifyLensMode::SpecOnly if design => design_lens_verify_prompt(&context),
        VerifyLensMode::SpecOnly => spec_only_verify_prompt(&context),
        VerifyLensMode::Full => full_lens_verify_prompt(&context, design),
    }
}

/// The single-lens form on a design turn: the one lens IS the design lens, on
/// the same scalar verdict contract [`parse_lens_verifier`] already reads.
fn design_lens_verify_prompt(context: &VerifyPromptContext<'_>) -> String {
    let VerifyPromptContext {
        task,
        diff,
        objective,
        paths,
        preexisting,
        assistant_claim,
        attached_files,
        visual_evidence,
    } = context;
    format!(
        "{DEEP_VERIFY_MARKER} You are ONE strict, adversarial verifier. This turn's request was read as \
         DESIGN work, so you MUST judge ONLY the design/visual-quality dimension YOURSELF, inline, \
         in this sub-turn. Do NOT spawn sub-agents, delegate, or call Agent, SpawnMultiAgent, \
         Workflow, or SendMessage. You may use read-only tools to inspect further — prefer \
         read_file/grep; if you run bash, use ONE simple command from the current directory (no \
         `cd`, no `&&`/`;` chaining — compound commands are denied in this read-only phase).\n\n\
         Task:\n{task}\n\n\
         {objective}\n\n\
         Paths changed this attempt:\n{paths}\n\n\
         {preexisting}Assistant's final claim for this attempt (bounded):\n{assistant_claim}\n\n\
         Diff (scoped git diff, bounded):\n{diff}{attached_files}{visual_evidence}\n\n\
         Judge the design lens (true = the result holds up, false = it does not):\n\
         {DESIGN_LENS_CRITERIA}\n\
         A design defect is a real defect — reject and itemize it exactly as a correctness verifier \
         would. Do not reject on personal taste alone: name the concrete drift, default, or floor \
         violation you found.\n\n\
         Respond with ONLY a single-line JSON object and NOTHING else — no prose, no markdown \
         code fences, no extra keys, and no text before or after it. Use exactly these scalar \
         verdict keys (accepted, issues, evidence). List each concrete design defect in issues, and \
         cite what you actually inspected in evidence — at most two short sentences naming the \
         decisive facts, never a narration of your process (issues may be as detailed as a \
         rejection needs):\n\
         {VERIFY_SCALAR_ACCEPT_EXAMPLE}\n\
         or\n\
         {VERIFY_SCALAR_REJECT_EXAMPLE}\n"
    )
}

fn spec_only_verify_prompt(context: &VerifyPromptContext<'_>) -> String {
    let VerifyPromptContext {
        task,
        diff,
        objective,
        paths,
        preexisting,
        assistant_claim,
        attached_files,
        visual_evidence,
    } = context;
    format!(
        "{DEEP_VERIFY_MARKER} You are ONE strict, adversarial verifier. You MUST judge ONLY the \
             spec/task-compliance dimension YOURSELF, inline, in this sub-turn. Do NOT spawn \
             sub-agents, delegate, or call Agent, SpawnMultiAgent, Workflow, or SendMessage. You \
             may use read-only tools to inspect further — prefer read_file/grep; if you run bash, \
             use ONE simple command from the current directory (no `cd`, no `&&`/`;` chaining — \
             compound commands are denied in this read-only phase).\n\n\
             Task:\n{task}\n\n\
             {objective}\n\n\
             Paths changed this attempt:\n{paths}\n\n\
             {preexisting}Assistant's final claim for this attempt (bounded):\n{assistant_claim}\n\n\
             Diff (scoped git diff, bounded):\n{diff}{attached_files}{visual_evidence}\n\n\
             Does the change FULLY and CORRECTLY satisfy the task, including every requirement, \
             correct error handling, and edge cases? If the change FIXES A BUG, prefer a test that \
             fails on the unfixed code and passes now; reject a bug fix that lacks such a test ONLY \
             where a failing-first reproduction is feasible. If reproduction is genuinely \
             impractical (a timing/heisenbug fix, a TUI/rendering glitch, a config or dependency \
             bump, or a change whose only feasible repro is manual), accept when the diff documents \
             WHY reproduction is impractical AND the objective check is green. If a checked-in test \
             explicitly requires behavior not spelled out in the task, treat that test as part of \
             the contract. Do not emit a partial lens object.\n\n\
             Respond with ONLY a single-line JSON object and NOTHING else — no prose, no markdown \
             code fences, no extra keys, and no text before or after it. Use exactly these scalar \
             verdict keys (accepted, issues, evidence). List each concrete unmet task requirement \
             in issues, and cite what you actually inspected in evidence — at most two short \
             sentences naming the decisive facts, never a narration of your process (issues may \
             be as detailed as a rejection needs):\n\
             {VERIFY_SCALAR_ACCEPT_EXAMPLE}\n\
             or\n\
             {VERIFY_SCALAR_REJECT_EXAMPLE}\n"
    )
}

/// The three-lens form. `design` adds the design rubric to the `spec` lens and
/// is otherwise a no-op, so a non-design turn's instruction is byte-identical
/// to the pre-design-axis prompt.
fn full_lens_verify_prompt(context: &VerifyPromptContext<'_>, design: bool) -> String {
    let VerifyPromptContext {
        task,
        diff,
        objective,
        paths,
        preexisting,
        assistant_claim,
        attached_files: _,
        visual_evidence,
    } = context;
    let design_lens = full_lens_design_block(design);
    format!(
        "{DEEP_VERIFY_MARKER} You are ONE strict, adversarial verifier. You MUST judge all three lenses \
         YOURSELF, inline, in this sub-turn, assessing each lens independently. Do NOT spawn \
         sub-agents, delegate, or call Agent, SpawnMultiAgent, Workflow, or SendMessage. The whole change is \
         REJECTED if ANY lens finds a real defect in its dimension (a single credible objection \
         blocks acceptance). You may use read-only tools to inspect further — prefer \
         read_file/grep; if you run bash, use ONE simple command from the current directory (no \
         `cd`, no `&&`/`;` chaining — compound commands are denied in this read-only phase).\n\n\
         Task:\n{task}\n\n\
         {objective}\n\n\
         Paths changed this attempt:\n{paths}\n\n\
         {preexisting}Assistant's final claim for this attempt (bounded):\n{assistant_claim}\n\n\
         Diff (scoped git diff, bounded):\n{diff}{visual_evidence}\n\n\
         Judge each lens (true = accept, false = reject):\n\
         - \"spec\": does the change FULLY and CORRECTLY satisfy the task? Every requirement \
         implemented; correct error handling and edge cases. If the change FIXES A BUG, prefer a \
         test that fails on the unfixed code and passes now; reject a bug fix that lacks such a test \
         ONLY where a failing-first reproduction is feasible — that is exactly how a \
         plausible-but-wrong fix slips through. If a reproduction is genuinely impractical (a \
         timing/heisenbug fix, a TUI/rendering glitch, a config or dependency bump, a change whose \
         only feasible repro is manual), accept when the diff documents WHY a reproduction is \
         impractical AND the objective check is green. If a checked-in test explicitly requires \
         behavior not spelled out in the task, treat that test as part of the contract instead of \
         rejecting the behavior as scope creep.\n\
         - \"regression\": does the change avoid breaking anything? Only the intended files changed \
         (no unrelated edits, no leaked artifacts); NO test modified or deleted to force a pass; no \
         existing behavior silently broken.\n\
         - \"security\": does the change avoid introducing a vulnerability (injection, unsafe input \
         handling, secret/credential leak, path traversal) in THIS diff? If the change is not \
         security-relevant, accept this lens (true).\n\
         {design_lens}\n\
         Respond with ONLY a single-line JSON object and NOTHING else — no prose, no markdown code \
         fences, no extra keys, and no text before or after it. Use exactly these keys (spec, \
         regression, security, issues, evidence). List every rejecting lens's concrete problem in \
         issues, and cite your work in evidence — ONE line naming what you actually inspected and \
         every command you ran with its observed result. A verdict without evidence is not \
         auditable and wastes the reviewer's trust:\n\
         {VERIFY_JSON_ACCEPT_EXAMPLE}\n\
         or\n\
         {VERIFY_JSON_REJECT_EXAMPLE}\n"
    )
}

/// Accumulates per-phase [`TurnSummary`]s into one combined summary for the
/// whole deep turn (iterations summed, message vectors concatenated, usage
/// folded field-by-field).
#[derive(Default)]
struct DeepSummaryAcc {
    assistant_messages: Vec<ConversationMessage>,
    tool_results: Vec<ConversationMessage>,
    prompt_cache_events: Vec<PromptCacheEvent>,
    iterations: usize,
    usage: TokenUsage,
    turn_output_tokens: u32,
    auto_compaction: Option<AutoCompactionEvent>,
    microcompact: Option<crate::MicrocompactEvent>,
    budget_exhausted: Option<BudgetExhausted>,
}

impl DeepSummaryAcc {
    fn fold(&mut self, summary: TurnSummary) {
        self.assistant_messages.extend(summary.assistant_messages);
        self.tool_results.extend(summary.tool_results);
        self.prompt_cache_events.extend(summary.prompt_cache_events);
        self.iterations += summary.iterations;
        // Each sub-turn's usage is the *cumulative* session usage at that point
        // (both `TurnSummary.usage` assignment sites use `cumulative_usage()`), so
        // the deep turn's usage is the LATEST sub-turn's cumulative — NOT the sum
        // of the snapshots. Summing multiplied the total by the sub-turn count and,
        // downstream, inflated the goal token budget and tripped auto-compaction
        // early. Sub-turns run in sequence so the last fold carries the highest
        // cumulative.
        self.usage = summary.usage;
        // `turn_output_tokens`, by contrast, is each sub-turn's OWN in-turn delta,
        // so the deep turn's output is their SUM (the goal budget charges the whole
        // multi-sub-turn deep turn, not just the last leg).
        self.turn_output_tokens = self
            .turn_output_tokens
            .saturating_add(summary.turn_output_tokens);
        if summary.auto_compaction.is_some() {
            self.auto_compaction = summary.auto_compaction;
        }
        if summary.microcompact.is_some() {
            self.microcompact = summary.microcompact;
        }
        // A budget stop in ANY leg marks the whole deep turn budget-stopped:
        // dropping it here silently disarmed every downstream consumer (the
        // `/loop` budget-pause and the grind-escalation streak) whenever the
        // deep gate wrapped the turn. Sub-turns run in sequence, so a later
        // leg's stop simply overwrites an earlier one.
        if summary.budget_exhausted.is_some() {
            self.budget_exhausted = summary.budget_exhausted;
        }
    }

    fn into_summary(self) -> TurnSummary {
        TurnSummary {
            assistant_messages: self.assistant_messages,
            tool_results: self.tool_results,
            prompt_cache_events: self.prompt_cache_events,
            iterations: self.iterations,
            usage: self.usage,
            turn_output_tokens: self.turn_output_tokens,
            auto_compaction: self.auto_compaction,
            microcompact: self.microcompact,
            // The deep methods return `(TurnSummary, DeepOutcome)` separately;
            // the verdict is stamped onto the summary at the wrapper seam
            // (`run_turn_streaming_maybe_deep`), so the accumulator leaves it
            // `None` here.
            deep_verification: None,
            verification_issues: Vec::new(),
            deep_verifier_parse: None,
            deep_verifier_model: None,
            budget_exhausted: self.budget_exhausted,
        }
    }
}

/// The goal-facing verification scalar exported on `summary.deep_verification`.
///
/// This must mean "the deep loop as a whole accepted", NOT merely "the verifier
/// gate accepted". `gate_accepted` can be true on an objective-RED turn (a strict
/// JSON accept is trusted for the deep loop's own retry/stall policy), but a goal
/// must never be marked succeeded while its objective check is red. `decision` is
/// `Accept` only when `objective_ok && gate_accepted` (see `decide_with_progress`),
/// so gating the export on it is the correct, conservative goal-facing signal.
fn goal_facing_accept(folded: &VerificationAttempt) -> bool {
    folded.decision == DeepDecision::Accept
}

/// The longest single wait the shared rate-limit backoff ladder will ever take.
///
/// [`api::quota::rate_limit_backoff_ms`] doubles per attempt and clamps at its
/// own ceiling, so feeding it a saturating attempt yields that ceiling. A park
/// longer than this cannot be ridden out by retrying — every attempt's wait
/// expires before the window clears — which makes it the point where skipping
/// the candidate finally beats waiting for it. Derived from the ladder instead
/// of restated as a literal so the two can never drift apart.
fn verifier_park_ride_out_ms() -> u64 {
    api::quota::rate_limit_backoff_ms(u32::MAX)
}

/// Whether the ranked walk should skip a verifier candidate before any request
/// is sent, given its provider's rate-limit cooldown (`parked_ms`) and whether
/// the provider is otherwise throttle-hot (`headroom_low`).
///
/// Both signals matter because they cover different windows: `parked_ms` is an
/// active cooldown, while `headroom_low` also stays true through the
/// recent-429 window *after* a cooldown lapses — the state that hands out a
/// fresh 429 on the next request. Non-Anthropic providers publish no
/// remaining-headroom header at all, so this recent-throttle window is the only
/// pre-flight signal they have.
///
/// Skipping is a pure win when `alternative_usable` — another eligible candidate
/// is clear, so the walk still lands a real cross-model verifier instead of
/// burning the stream's retry budget rediscovering a 429 the quota registry
/// already knows about. With no usable alternative the trade inverts: falling
/// through to the native client means the model that just wrote the diff also
/// verifies it, so a merely hot provider is still worth attempting and only a
/// park longer than the backoff ladder could ever clear
/// ([`verifier_park_ride_out_ms`]) justifies that degradation.
///
/// `exhausted_ms` overrides all of that. The other two signals are inferences
/// from recent behaviour, but this one is the provider's own statement of when
/// its window resets, so attempting is a guaranteed 429 — worse than degrading.
/// It is also the only signal that outlives the cool-down's two-minute ceiling,
/// which is why a session used to re-earn the same 429 every two minutes.
fn skip_parked_verifier(
    parked_ms: u64,
    headroom_low: bool,
    alternative_usable: bool,
    exhausted_ms: u64,
) -> bool {
    if exhausted_ms > 0 {
        return true;
    }
    if parked_ms == 0 && !headroom_low {
        return false;
    }
    alternative_usable || parked_ms > verifier_park_ride_out_ms()
}

/// Emit a non-critical deep-phase progress note into the render stream. A closed
/// channel just means the turn is unwinding, so the error is ignored.
/// Spec-literal self-verify gate for the streaming dispatcher's terminal arm,
/// parity with the sync `run_turn`: an edit that reproduced a task-specified
/// backticked literal with the wrong case is patched deterministically (a
/// model repair only fixes the casing ~50% of the time, measured). Offloaded
/// through `spawn_blocking` because the gate probes git when the prompt
/// carries a candidate literal, and this dispatcher also drives TUI turns on
/// the render reactor. The visible note keeps the file mutation honest
/// instead of silent.
async fn run_spec_literal_gate(original: &str, render_tx: &mpsc::Sender<RenderBlock>) {
    let spec_original = original.to_string();
    let patched = tokio::task::spawn_blocking(move || {
        super::turn_end::spec_literal_autopatch(&spec_original)
    })
    .await
    .unwrap_or(false);
    if patched {
        deep_note(
            render_tx,
            &BlockIdGen::default(),
            "spec-literal gate: auto-patched exact-case literal(s)",
        )
        .await;
    }
}

async fn deep_note(
    render_tx: &mpsc::Sender<RenderBlock>,
    ids: &BlockIdGen,
    text: impl Into<String>,
) {
    let _ = render_tx
        .send(RenderBlock::System {
            id: ids.next(),
            level: SystemLevel::Info,
            text: text.into(),
        })
        .await;
}

impl<C, T> ConversationRuntime<C, T>
where
    C: ApiClient,
    T: ToolExecutor,
{
    /// Install (or clear) the deep-lane gate. With a config set, the host should
    /// drive turns through [`Self::run_deep_turn_streaming`] instead of
    /// [`Self::run_turn_streaming_with_images`].
    pub fn set_deep_gate(&mut self, config: Option<DeepGateConfig>) {
        self.deep_gate = config;
    }

    /// The installed deep-lane config, if any.
    #[must_use]
    pub fn deep_gate(&self) -> Option<&DeepGateConfig> {
        self.deep_gate.as_ref()
    }

    /// Set the stable logical workspace directory used to root durable external
    /// traces (`.zo/dream/`, `.zo/turns/`). Hosts whose process cwd can
    /// diverge from the workspace — the interactive TUI (where `EnterWorktree`
    /// chdirs) and `zo serve` (many sessions, one process) — must call this so
    /// trace producers and the auto-dream consumer agree on one `.zo/`.
    pub fn set_workspace_cwd(&mut self, cwd: std::path::PathBuf) {
        self.workspace_cwd = Some(cwd);
    }

    /// Resolve the directory to root durable traces at: the configured stable
    /// workspace if set, else the live process cwd. Centralizes the producer
    /// rule so every trace site (deep-gate accept, turn completion) agrees.
    pub(crate) fn trace_cwd(&self) -> Option<std::path::PathBuf> {
        if let Some(root) = std::env::var_os("ZO_TRACE_ROOT") {
            return Some(std::path::PathBuf::from(root));
        }
        match &self.workspace_cwd {
            Some(cwd) => Some(cwd.clone()),
            None => std::env::current_dir().ok(),
        }
    }

    /// Record a green-verified acceptance as a Dreamer candidate lesson.
    ///
    /// Called from both accept paths (reactive and plan-first) when a change is
    /// accepted *and* the objective check ran green. Best-effort and silent: it
    /// appends one candidate to `.zo/dream/` rooted at the session's stable
    /// workspace ([`Self::trace_cwd`]), which a later between-sessions
    /// [`crate::maybe_auto_dream`] pass may promote once the same project check
    /// has been green-verified across enough distinct sessions. A turn is never
    /// failed or slowed by a recording problem.
    /// Record what the project's own check command settled about this
    /// attempt — an OBJECTIVE verdict, the one kind the ledger had none of.
    ///
    /// The deep gate already RAN the check and already knew the answer; it
    /// simply never wrote it down, so every verdict on disk was an LLM's
    /// opinion and the accuracy report read 100% model-only by construction.
    /// Nothing is invented here: with no attempt declared, or no wire model
    /// to credit, there is no row.
    fn record_objective_check_verdict(&self, green: bool) {
        if self.attempt().is_empty() {
            return;
        }
        let Some((model, _)) = self.wire_model() else {
            return;
        };
        let Some(cwd) = self.trace_cwd() else {
            return;
        };
        let record = crate::RouteOutcomeRecord::new(
            "main",
            "turn",
            model,
            if green {
                crate::OUTCOME_COMPLETED
            } else {
                crate::OUTCOME_FAILED
            },
        )
        .with_signal("verdict")
        .with_decision(crate::DecisionKind::Verify)
        .with_verdict_basis(crate::VerdictBasis::Objective)
        // The shape the turn ran under, as the host deposited it when it
        // decided the prelude.
        .with_shape_label(::api::plan_shape_for_attempt(self.attempt()).unwrap_or_default())
        .with_attempt_key(self.attempt());
        let _ = crate::record_route_outcome(&cwd, &record);
    }

    fn record_verified_accept(&self, objective_ok: bool) {
        if !objective_ok {
            return;
        }
        let Some(check) = self
            .deep_gate
            .as_ref()
            .and_then(|c| c.check_command.clone())
        else {
            return;
        };
        if let Some(cwd) = self.trace_cwd() {
            let _ = crate::record_verified_check(&cwd, &self.session.session_id, Some(&check));
            let _ = crate::memory::record_self_improve_pulse_if_enabled(
                self.dream_automation_enabled,
                &cwd,
                decision_core::dreamer::CandidateKind::VerifiedAccept,
                &self.session.session_id,
                "deep_gate",
                "deep gate accepted after objective check",
                &check,
                true,
            );
        }
    }

    /// Drop-in streaming entry point for hosts: routes to the deep gate when one
    /// is installed (discarding the structured [`DeepOutcome`], which is already
    /// narrated into the render stream), otherwise runs the ordinary turn. Keeps
    /// the caller's `select!`/render loop on a single `Result<TurnSummary, _>`
    /// shape regardless of mode.
    ///
    /// Honors the `TurnEnd` (Stop) hook exactly like the synchronous
    /// [`Self::run_turn`] loop: a hook returning a `followupMessage` (or the
    /// Claude Code `decision: "block"` shape) re-enters the turn with that
    /// message as the next user input, bounded by `max_stop_loops`. This used
    /// to exist only on the sync path, which made Stop-hook gates (e.g. a
    /// session-goal "keep working until done" check) dead in the interactive
    /// TUI — the one place users actually run them.
    ///
    /// # Errors
    /// Propagates any [`StreamingTurnError`] from the underlying turn.
    pub async fn run_turn_streaming_maybe_deep(
        &mut self,
        user_input: impl Into<String>,
        images: Vec<(String, String)>,
        render_tx: mpsc::Sender<RenderBlock>,
        prompter: Arc<dyn AsyncPermissionPrompter>,
    ) -> Result<TurnSummary, StreamingTurnError> {
        let mut input: String = user_input.into();
        // Keep the original request verbatim for the spec-literal self-verify
        // gate in the terminal arm — the Stop-loop rewrites `input` with each
        // followup (mirrors the sync `run_turn`).
        let original = input.clone();
        // Follow-up rounds are text-only; the original images belong to the
        // first round (mirrors the sync Stop-loop).
        let mut images = images;
        let mut loop_count = 0usize;
        // Baseline for the WHOLE turn's output delta across Stop-loop legs — each
        // leg's summary carries only its own delta, but a multi-leg (TurnEnd
        // followup) turn must charge the goal budget the sum. Cumulative is
        // monotonic within this runtime instance, so re-derive it at return.
        let turn_base_output = self.usage_tracker.cumulative_usage().output_tokens;
        let result = loop {
            let round_images = std::mem::take(&mut images);
            let deep_mode = self.deep_gate.as_ref().map(|cfg| cfg.mode);
            if deep_mode.is_some() {
                if let Err(error) = self.run_user_prompt_submit_for_streaming_user_entry(&input) {
                    break Err(error);
                }
            }
            // A per-turn Architect contract can promote a Reactive gate to the
            // plan-first driver for THIS turn only (complex implementation
            // turns get a read-only PLAN by the reserved model before the
            // implementer edits); the installed gate config itself is untouched.
            let plan_first_contract = self
                .exec_contract
                .as_ref()
                .is_some_and(|contract| contract.plan_first);
            let mut summary = match deep_mode {
                Some(DeepMode::Reactive) if !plan_first_contract => {
                    let (mut summary, outcome) = match self
                        .run_auto_turn_streaming(
                            input.clone(),
                            round_images,
                            render_tx.clone(),
                            Arc::clone(&prompter),
                        )
                        .await
                    {
                        Ok(value) => value,
                        Err(error) => break Err(error),
                    };
                    // Surface the adversarial verifier's verdict to the host so
                    // the goal controller can gate completion on it.
                    summary.deep_verification = outcome.verification;
                    summary.verification_issues = outcome.issues;
                    summary.deep_verifier_parse = outcome.verifier_parse;
                    summary.deep_verifier_model = outcome.verifier_model;
                    summary
                }
                Some(DeepMode::PlanFirst | DeepMode::Reactive) => {
                    let (mut summary, outcome) = match self
                        .run_deep_turn_streaming(
                            input.clone(),
                            round_images,
                            render_tx.clone(),
                            Arc::clone(&prompter),
                        )
                        .await
                    {
                        Ok(value) => value,
                        Err(error) => break Err(error),
                    };
                    summary.deep_verification = outcome.verification;
                    summary.verification_issues = outcome.issues;
                    summary.deep_verifier_parse = outcome.verifier_parse;
                    summary.deep_verifier_model = outcome.verifier_model;
                    summary
                }
                None => {
                    match self
                        .run_turn_streaming_with_images(
                            input.clone(),
                            round_images,
                            render_tx.clone(),
                            Arc::clone(&prompter),
                        )
                        .await
                    {
                        Ok(summary) => summary,
                        Err(error) => break Err(error),
                    }
                }
            };
            // TurnEnd (Stop) hook — same contract and bound as the sync loop
            // (`run_turn`). Hook commands run synchronously with the shared 5s
            // timeout; with no TurnEnd rules configured this is a no-op, so the
            // render loop only ever pauses when the user opted into a gate.
            let files_changed = changed_files_snapshot_async().await;
            let context = build_turn_end_hook_context(
                &summary,
                loop_count,
                &files_changed,
                self.session.session_goal.as_deref(),
            );
            let outcome = self.run_lifecycle_hook(HookEvent::TurnEnd, &context);
            match outcome.followup().map(str::to_owned) {
                Some(followup) if loop_count < self.max_stop_loops => {
                    loop_count += 1;
                    input = followup;
                }
                _ => {
                    run_spec_literal_gate(&original, &render_tx).await;
                    summary.turn_output_tokens = self
                        .usage_tracker
                        .cumulative_usage()
                        .output_tokens
                        .saturating_sub(turn_base_output);
                    break Ok(summary);
                }
            }
        };
        self.settle_team_inbox_turn_for_result(&result);
        result
    }

    /// Reactive auto-verify turn — the default [`DeepMode::Reactive`]. Runs the
    /// user's request as an ordinary turn (full tools, no read-only phase), then
    /// **only if it edited files** selects proportional VERIFY depth for the diff
    /// and retries verified failures, bounded by `max_attempts`. A chat/analysis
    /// turn that changes nothing returns immediately with zero verification
    /// overhead and no permission friction.
    ///
    /// This is the Reactive phase driver behind
    /// [`Self::run_turn_streaming_maybe_deep`]: it drives internal sub-turns
    /// only and deliberately skips the per-turn host lifecycle — the
    /// `UserPromptSubmit` hook and the `TeamInbox` digest injection/settle run
    /// in that outer loop. Calling this directly bypasses those policies.
    ///
    /// # Errors
    /// Propagates any [`StreamingTurnError`] from a sub-turn.
    #[allow(clippy::too_many_lines)]
    pub async fn run_auto_turn_streaming(
        &mut self,
        user_input: impl Into<String>,
        images: Vec<(String, String)>,
        render_tx: mpsc::Sender<RenderBlock>,
        prompter: Arc<dyn AsyncPermissionPrompter>,
    ) -> Result<(TurnSummary, DeepOutcome), StreamingTurnError> {
        let cfg = self.deep_gate.clone().unwrap_or_default();
        let max = cfg.max_attempts.max(1);
        let task = user_input.into();
        let ids = BlockIdGen::default();
        let mut acc = DeepSummaryAcc::default();
        let mut pending_images = Some(images);
        let mut extra: Option<String> = None;
        let mut exec_retry: Option<String> = None;
        let mut exec_transport_escalated = false;
        let mut decision = DeepDecision::Accept;
        let mut attempts = 0u32;
        // The verifier's semantic verdict for goal-completion gating. `None`
        // until an edit-making turn is actually verified, so a no-edit turn or
        // proportional trivial-change skip reports no semantic judgment rather
        // than a spurious verifier accept.
        let mut verification: Option<bool> = None;
        // The final attempt's verifier issues, surfaced on the outcome so the
        // goal-level repair prompt can name the concrete defects to fix.
        let mut verifier_issues: Vec<String> = Vec::new();
        // Phase 4 verdict-channel seam: the final attempt's raw verifier parse
        // confidence and the verifier model, surfaced on the outcome alongside
        // `verification`/`verifier_issues`. See `DeepOutcome::verifier_parse`.
        let mut verifier_parse: Option<VerifierParse> = None;
        let mut verifier_model: Option<String> = None;
        // The previous attempt's verifier issues, for the ALP §3 "no more
        // progress" stop condition: if a retry fails for the same reason, give
        // up early instead of burning the remaining attempt budget.
        let mut prev_issues: Vec<String> = Vec::new();
        let mut verify_depth_floor = VerifyDepth::Skip;
        // Do not run the objective command before the first model stream. In the
        // interactive default this command can be a heavyweight project test (for
        // example `cargo test`), and running it here delays the first token even
        // for a no-edit chat turn. Reactive mode verifies only after an edit is
        // observed below; no-edit turns keep the zero-overhead contract.

        for attempt in 1..=max {
            attempts = attempt;
            // Attempt 1 is the user's request verbatim — behaves exactly like an
            // ordinary turn. A retry restates it with the failure repair contract.
            let prompt = match exec_retry.as_deref() {
                None => task.clone(),
                Some(repair) => reactive_retry_prompt(&task, repair),
            };
            let baseline_files = changed_files_snapshot_async().await;
            // Sampled HERE, next to the baseline it anchors: EXEC may move
            // the process cwd (`EnterWorktree`), and an own_root sampled
            // after EXEC would key `baseline_files` in a different frame
            // than the snapshot they came from.
            let own_root = own_repo_root_async().await;
            let phase_images = pending_images.take().unwrap_or_default();
            // Runtime effort escalation, mirroring the plan-first EXEC leg
            // (`run_deep_turn_streaming`): the first attempt runs at the
            // configured effort, but a retry means that effort did not solve the
            // task, so power up every retry to at least `Xhigh` (a floor, never a
            // downgrade). Cleared immediately after the EXEC sub-turn — before `?`
            // and before VERIFY — so the override never leaks into the read-only
            // verify turn or a later turn on error.
            if attempt > 1 {
                self.set_effort_override(Some(super::ESCALATION_EFFORT_BUDGET));
                deep_note(
                    &render_tx,
                    &ids,
                    "auto: escalating reasoning effort (xhigh) for retry…",
                )
                .await;
            }
            if exec_transport_escalated {
                self.reserved_edit_gate = false;
                if attempt == 2 {
                    if let Some(note) = self.exec_transport_escalation_note() {
                        deep_note(&render_tx, &ids, note).await;
                    }
                }
            } else if let Some(note) = self.exec_leg_note(attempt) {
                deep_note(&render_tx, &ids, note).await;
            }
            if self.exec_swap_enabled() && attempt > ARCHITECT_IMPL_ATTEMPTS {
                // Failure escalation: the native (reserved) model implements
                // from here on, so the edit gate stands down for this turn.
                self.reserved_edit_gate = false;
            }
            // The EXEC leg keeps the caller's permission mode (a no-op
            // set/restore); the guard exists so an Architect contract can swap
            // the leg onto the implementer client — without one this is
            // byte-identical to the old direct sub-turn call.
            let base_mode = self.permission_policy.active_mode();
            let exec_client = if exec_transport_escalated {
                SubturnClient::Native
            } else {
                self.exec_leg_client(attempt)
            };
            let exec_was_implementer = exec_client == SubturnClient::Implementer;
            let exec_result = self
                .deep_subturn(
                    prompt,
                    phase_images,
                    base_mode,
                    exec_client,
                    DeepSubturnPhase::Exec,
                    &render_tx,
                    &prompter,
                )
                .await;
            self.set_effort_override(None);
            let exec_result = match exec_result {
                Ok(result) => result,
                Err(error) if attempt < max => {
                    if let Some(retry_context) = exec_transport_retry_context(&error) {
                        if exec_was_implementer {
                            exec_transport_escalated = true;
                        }
                        exec_retry = Some(retry_context);
                        deep_note(
                            &render_tx,
                            &ids,
                            "auto: EXEC transport failure — retrying…",
                        )
                        .await;
                        continue;
                    }
                    return Err(error);
                }
                Err(error) => return Err(error),
            };
            let verify_images = exec_result.verify_images;
            let summary = exec_result.summary;
            let edited = made_edits(&summary);
            let edited_paths = edited_file_paths(&summary);
            let assistant_claim = latest_assistant_text(&summary.assistant_messages);
            let green_checks = exec_green_checks(&summary);
            // Snapshot before the fold consumes `summary`: the SingleLens
            // attachment set needs what EXEC consulted this attempt.
            let consulted_paths = exec_consulted_paths(&summary);
            acc.fold(summary);

            // No edits ⇒ a question/analysis/chat turn. Done — never tax a turn
            // that changed nothing.
            if !edited {
                decision = DeepDecision::Accept;
                break;
            }

            let check_observation = match cfg.check_command.as_deref() {
                Some(cmd) => {
                    let observation = run_check_command(cmd).await;
                    self.record_objective_check_verdict(observation.green);
                    deep_note(
                        &render_tx,
                        &ids,
                        format!(
                            "auto: check `{cmd}` → {}",
                            if observation.green {
                                "green ✓"
                            } else {
                                "red ✗"
                            }
                        ),
                    )
                    .await;
                    Some(observation)
                }
                None => None,
            };
            let objective_ok = check_observation.as_ref().is_none_or(|check| check.green);

            let after_files = changed_files_snapshot_async().await;
            let diff_paths = attempt_diff_paths(
                &baseline_files,
                &after_files,
                &edited_paths,
                own_root.as_deref(),
            );
            let (diff, line_churn) =
                bounded_git_diff_for_paths(diff_paths.clone(), 6000, own_root.clone()).await;
            let selected_depth = verify_depth_for_band(
                self.verify_band,
                diff_paths.len(),
                line_churn,
                objective_ok,
                paths_touch_security(&diff_paths),
                paths_touch_tests(&diff_paths),
            );
            // Two floors, both monotone: the retry floor (a later attempt never
            // verifies more shallowly than an earlier one) and this turn's
            // probed intent (a design turn never skips verification outright).
            // Neither can force `Full` on its own.
            let intent = self.verify_intent;
            let depth = selected_depth
                .max(verify_depth_floor)
                .max(intent_verify_floor(intent));
            verify_depth_floor = depth;
            if depth == VerifyDepth::Skip {
                deep_note(
                    &render_tx,
                    &ids,
                    "auto: trivial green change — skipping deep verify",
                )
                .await;
                decision = DeepDecision::Accept;
                break;
            }
            let verify_note = match self.deep_verify_primary_model_label() {
                Some(model) => format!(
                    "auto: verifying the change with {model} (cross-model, attempt {attempt}/{max})…"
                ),
                None => format!("auto: verifying the change (attempt {attempt}/{max})…"),
            };
            deep_note(&render_tx, &ids, verify_note).await;
            // VERIFY runs read-only just like plan-first: an adversarial verifier
            // inspects the diff but must never edit or delete files. `deep_subturn`
            // downgrades a write-capable session to ReadOnly (with the scoped
            // read-only `bash` grant) and always restores the prior mode. When a
            // cross-model verifier is installed the leg runs on it (native
            // fallback inside `verify_subturn`).
            // The leg deliberately inherits the session's thinking/effort
            // config unchanged: any per-leg config delta (e.g. disabling
            // thinking for a SingleLens verdict) invalidates the provider's
            // message-level prompt cache and re-bills the whole conversation
            // prefix as cache-write — measured at 14-25k tokens per leg,
            // dwarfing the few hundred thinking tokens it would save.
            // Post-edit file contents ride the SingleLens prompt so the
            // common leg verdicts in one call; Full legs skip the read (the
            // prompt drops it anyway, and large/red changes read files
            // selectively themselves).
            let attached_files = if depth == VerifyDepth::SingleLens {
                verify_file_attachments(
                    &verify_attachment_paths(&diff_paths, &consulted_paths, own_root.as_deref()),
                    own_root.as_deref(),
                )
            } else {
                String::new()
            };
            // Read the session ledger HERE — after every gate that can skip the
            // leg — so the attest counts legs that were actually built. Computed
            // earlier it would record a decline for turns that never opened a
            // verifier, and a counter diluted by non-events cannot answer the
            // only question it exists for: how often the join had something to
            // say when a verifier was listening.
            //
            // The ledger is current because the tool-result seam kept it so
            // THROUGH the EXEC leg above: any earlier check those edits
            // invalidated is already excluded, rather than being handed to the
            // verifier as a still-true fact.
            let session_checks = self.verify_leg_session_checks(&green_checks);
            let verify_result = self
                .verify_subturn(
                    verify_prompt(
                        &task_with_retry_context(&task, extra.as_deref()),
                        &diff,
                        cfg.check_command
                            .as_deref()
                            .zip(check_observation.as_ref()),
                        GreenCheckEvidence {
                            this_turn: &green_checks,
                            session: &session_checks,
                        },
                        &diff_paths,
                        &preexisting_dirty_paths(
                            &baseline_files,
                            &diff_paths,
                            own_root.as_deref(),
                        ),
                        &assistant_claim,
                        &attached_files,
                        if depth == VerifyDepth::SingleLens {
                            VerifyLensMode::SpecOnly
                        } else {
                            VerifyLensMode::Full
                        },
                        intent,
                        !verify_images.is_empty(),
                    ),
                    verify_images,
                    &render_tx,
                    &ids,
                    &prompter,
                )
                .await;
            // A failed VERIFY leg (transient streaming error) must NOT throw away
            // the EXEC edits already applied this attempt. Fold a conservative
            // non-accept (Timeout) so the loop retries or gives up at the cap,
            // preserving the completed implementation in the work tree.
            let verifier = match verify_result {
                Ok(mut verify_summary) => {
                    // The leg's own iteration cap is verify-local: folding it
                    // unstripped would mark the WHOLE deep turn budget-stopped
                    // (the flag that drives /loop pause and grind escalation),
                    // and parsing the synthetic closer would misread the stop
                    // as a rejection. It becomes the verdict instead.
                    let verify_budget_stopped = verify_summary.budget_exhausted.take().is_some();
                    acc.fold(verify_summary);
                    if verify_budget_stopped {
                        deep_note(
                            &render_tx,
                            &ids,
                            format!(
                                "verifier hit its inspection budget ({VERIFY_LEG_MAX_ITERATIONS} rounds) without a verdict — the objective check gates this attempt"
                            ),
                        )
                        .await;
                        verify_budget_exhausted_verdict()
                    } else {
                        parse_verify_leg_text(&self.last_assistant_text())
                    }
                }
                Err(_) => verify_leg_failed_verdict(),
            };
            // Keep accept/retry/stall policy in decision-core; this runtime only
            // supplies observed IO facts from the live VERIFY sub-turn. Reactive
            // mode intentionally does not run a pre-model baseline command, so an
            // objective-red post-edit check remains blocking rather than delaying
            // the first token to classify it as a pre-existing failure.
            let gating_objective_ok = objective_ok;
            record_verifier_calibration(&self.session.session_id, gating_objective_ok, &verifier);
            let folded = fold_verification_attempt(
                attempt,
                max,
                gating_objective_ok,
                &verifier,
                &prev_issues,
            );

            // Record the goal-facing verdict for goal-completion gating: export
            // "the deep loop accepted overall" (decision == Accept), NOT the raw
            // verifier gate — an objective-red turn must never read as accepted by
            // a downstream goal that has no objective validators of its own.
            verification = Some(goal_facing_accept(&folded));
            verifier_issues = verifier.issues.clone();
            verifier_parse = Some(verifier.parse);
            verifier_model = self
                .deep_verify_succeeded_model_label()
                .map(str::to_string);
            decision = folded.decision;
            deep_note(
                &render_tx,
                &ids,
                verification_outcome_note("auto", decision, attempt, max, objective_ok, &verifier),
            )
            .await;

            match decision {
                DeepDecision::Accept | DeepDecision::GiveUp => {
                    if decision == DeepDecision::Accept {
                        self.record_verified_accept(objective_ok);
                    }
                    // Retry-conversion evidence: only a turn that actually
                    // spent a retry carries signal. fired = the retry
                    // converted to Accept; failed = the budget went for
                    // nothing — the fired:failed ratio is what lets a future
                    // max-attempts change cite measurement instead of taste.
                    if attempt > 1 {
                        if decision == DeepDecision::Accept {
                            telemetry::attest_fired(telemetry::HarnessFeature::DeepRetryConversion);
                        } else {
                            telemetry::attest_failed(
                                telemetry::HarnessFeature::DeepRetryConversion,
                                "gave_up",
                            );
                        }
                    }
                    break;
                }
                DeepDecision::Retry => {
                    let repair = failure_summary(objective_ok, &verifier);
                    exec_retry = Some(exec_retry_context(
                        &repair,
                        &diff,
                        &diff_paths,
                        cfg.check_command
                            .as_deref()
                            .zip(check_observation.as_ref()),
                    ));
                    extra = Some(repair);
                    prev_issues = verifier.issues.clone();
                }
            }
        }

        let outcome = DeepOutcome {
            decision,
            attempts,
            plan_valid: true,
            plan_missing: Vec::new(),
            verification,
            issues: verifier_issues,
            verifier_parse,
            verifier_model,
        };
        Ok((acc.into_summary(), outcome))
    }

    /// Concatenated text of the most recent assistant message (the phase output
    /// the gate inspects). Empty when the last turn produced no assistant text.
    fn last_assistant_text(&self) -> String {
        latest_assistant_text(&self.session.messages)
    }

    /// Run one phase sub-turn under `mode`, always restoring the prior
    /// permission mode (even on error) so a `ReadOnly` PLAN/VERIFY never leaks
    /// past its phase.
    #[allow(
        clippy::too_many_arguments,
        reason = "phase must stay explicit because permission mode and client do not identify EXEC"
    )]
    async fn deep_subturn(
        &mut self,
        prompt: String,
        images: Vec<(String, String)>,
        mode: PermissionMode,
        client: SubturnClient,
        phase: DeepSubturnPhase,
        render_tx: &mpsc::Sender<RenderBlock>,
        prompter: &Arc<dyn AsyncPermissionPrompter>,
    ) -> Result<DeepSubturnResult, StreamingTurnError> {
        let guard = DeepSubturnPermissionGuard::new(self, mode, client, phase);
        guard
            .run(prompt, images, render_tx.clone(), Arc::clone(prompter))
            .await
    }

    fn exec_swap_enabled(&self) -> bool {
        self.exec_contract
            .as_ref()
            .is_some_and(ExecContract::exec_swap_enabled)
    }

    fn native_model_is_deep_tier(&self) -> bool {
        self.context_model
            .as_deref()
            .is_some_and(|model| crate::is_deep_tier_model(model, &self.deep_tier_models))
    }

    fn plan_leg_client(&self) -> Result<SubturnClient, StreamingTurnError> {
        if self.deep_plan_client.as_ref().is_some_and(|(_, model)| {
            !self.deep_tier_only || crate::is_deep_tier_model(model, &self.deep_tier_models)
        }) {
            return Ok(SubturnClient::Plan);
        }
        if !self.deep_tier_only || self.native_model_is_deep_tier() {
            return Ok(SubturnClient::Native);
        }
        Err(StreamingTurnError::runtime(
            "architect PLAN requires an available configured deep-tier client",
        ))
    }

    /// Which client the EXEC leg of `attempt` runs on. When `smart.execSwap`
    /// did not arm for this turn, every attempt remains native while retaining
    /// the Architect EXEC edit-gate exemption. When armed, two implementer
    /// failures escalate to the native model, mirroring the router escape.
    fn exec_leg_client(&self, attempt: u32) -> SubturnClient {
        if let Some(contract) = &self.exec_contract {
            if !contract.exec_swap_enabled() {
                return SubturnClient::NativeExec;
            }
            if attempt <= ARCHITECT_IMPL_ATTEMPTS {
                return SubturnClient::Implementer;
            }
        }
        SubturnClient::Native
    }

    /// One-line narration for the EXEC leg's contract state, `None` when the
    /// leg runs native without a contract (nothing to announce).
    fn exec_leg_note(&self, attempt: u32) -> Option<String> {
        let contract = self.exec_contract.as_ref()?;
        if !contract.exec_swap_enabled() {
            return None;
        }
        let native = self.context_model.as_deref().unwrap_or("the main model");
        if attempt <= ARCHITECT_IMPL_ATTEMPTS {
            (attempt == 1).then(|| {
                format!(
                    "architect: implementing with {} — {native} stays on plan/verify",
                    contract.impl_model
                )
            })
        } else if attempt == ARCHITECT_IMPL_ATTEMPTS + 1 {
            Some(format!(
                "architect: {ARCHITECT_IMPL_ATTEMPTS} implementer attempts failed — escalating implementation to {native}"
            ))
        } else {
            None
        }
    }

    /// Narration for a typed implementer transport failure that moves the next
    /// EXEC attempt to the native model immediately, rather than spending a
    /// second attempt against the same exhausted provider.
    fn exec_transport_escalation_note(&self) -> Option<String> {
        let contract = self.exec_contract.as_ref()?;
        if !contract.exec_swap_enabled() {
            return None;
        }
        let native = self.context_model.as_deref().unwrap_or("the main model");
        Some(format!(
            "architect: implementer transport failure — escalating implementation to {native}"
        ))
    }

    /// One VERIFY sub-turn (read-only). When cross-model verifier candidates are
    /// installed ([`Self::set_deep_verify_candidates`]), the leg walks them
    /// top-ranked first: a hard `RateLimit` skips every remaining candidate on
    /// that provider and tries the next different-provider candidate, while any
    /// other stream error advances to the next candidate. The walk is bounded
    /// by the candidate count. Exhausting an installed ranked list falls back
    /// to the native main client once unless the Architect deep-tier invariant
    /// forbids that non-deep fallback.
    async fn verify_subturn(
        &mut self,
        prompt: String,
        images: Vec<(String, String)>,
        render_tx: &mpsc::Sender<RenderBlock>,
        ids: &BlockIdGen,
        prompter: &Arc<dyn AsyncPermissionPrompter>,
    ) -> Result<TurnSummary, StreamingTurnError> {
        self.deep_verify_succeeded_model = None;
        let candidate_count = self.deep_verify_candidates.len();
        if candidate_count > 0 {
            let mut rate_limited_providers: Vec<api::ProviderKind> = Vec::new();
            let alternative_usable = self.verifier_alternative_usable();
            for idx in 0..candidate_count {
                let model = self.deep_verify_candidates[idx].1.clone();
                if !self.verifier_candidate_eligible(&model) {
                    continue;
                }
                let provider = api::detect_provider_kind(&model);
                if rate_limited_providers.contains(&provider) {
                    continue;
                }
                if Self::verifier_parked_skip_note(
                    &model,
                    provider,
                    alternative_usable,
                    render_tx,
                    ids,
                )
                .await
                {
                    rate_limited_providers.push(provider);
                    continue;
                }
                self.deep_verify_candidate_idx = idx;
                match self
                    .deep_subturn(
                        prompt.clone(),
                        images.clone(),
                        PermissionMode::ReadOnly,
                        SubturnClient::Verify,
                        DeepSubturnPhase::Verify,
                        render_tx,
                        prompter,
                    )
                    .await
                {
                    Ok(result) => {
                        self.deep_verify_succeeded_model = Some(model);
                        return Ok(result.summary);
                    }
                    Err(err) => {
                        if matches!(
                            err.provider_error_class(),
                            Some(api::ProviderErrorClass::RateLimit { .. })
                        ) {
                            rate_limited_providers.push(provider);
                            deep_note(
                                render_tx,
                                ids,
                                format!(
                                    "auto: verifier {model} rate-limited — trying the next-ranked provider…"
                                ),
                            )
                            .await;
                        } else {
                            deep_note(
                                render_tx,
                                ids,
                                format!(
                                    "auto: verifier {model} unavailable — trying the next-ranked candidate…"
                                ),
                            )
                            .await;
                        }
                    }
                }
            }
            deep_note(
                render_tx,
                ids,
                if self.deep_tier_only && !self.native_model_is_deep_tier() {
                    "auto: no deep-tier verifier available — VERIFY continues on the main model (same-model check, not cross-model)"
                } else {
                    "auto: all ranked verifier candidates unavailable — retrying with the main model…"
                },
            )
            .await;
        }
        self.deep_subturn(
            prompt,
            images,
            PermissionMode::ReadOnly,
            SubturnClient::Native,
            DeepSubturnPhase::Verify,
            render_tx,
            prompter,
        )
        .await
        .map(|result| result.summary)
    }

    /// Rate-limit cooldown `provider` is currently parked in, in milliseconds.
    /// Non-zero means a request would be throttled on arrival. The registry is
    /// cross-process, so this also sees a 429 another zo process took on the
    /// same account — the single source of truth both the pre-attempt label and
    /// the candidate walk read, so they can never disagree about who is usable.
    fn verifier_provider_parked_ms(provider: api::ProviderKind) -> u64 {
        api::quota::rate_limit_cooldown_remaining_ms(provider)
    }

    /// Whether a ranked verifier candidate passes the walk's deep-tier gate.
    /// Read by both the walk and the pre-attempt label so the two can never
    /// filter differently.
    fn verifier_candidate_eligible(&self, model: &str) -> bool {
        !self.deep_tier_only || crate::is_deep_tier_model(model, &self.deep_tier_models)
    }

    /// Test-only: make every quota pre-flight read report a clean registry on
    /// THIS thread. The ranked-walk tests assert walk semantics driven by
    /// client outcomes, but the pre-flight consults process-global quota
    /// state that ANY parallel test streaming a 429-shaped error repollutes
    /// (via `streaming_turn`'s capacity-stall marking) — an unwinnable
    /// whack-a-mole. The parked-skip POLICY keeps its own coverage through
    /// the pure `skip_parked_verifier` unit tests. Thread-local is sound
    /// here because `#[tokio::test]` runs a current-thread runtime: the
    /// whole walk executes on the test's thread.
    #[cfg(test)]
    fn set_quota_preflight_clean_for_this_thread() {
        QUOTA_PREFLIGHT_CLEAN.set(true);
    }

    fn quota_preflight_clean() -> bool {
        #[cfg(test)]
        {
            QUOTA_PREFLIGHT_CLEAN.get()
        }
        #[cfg(not(test))]
        {
            false
        }
    }

    fn preflight_parked_ms(provider: api::ProviderKind) -> u64 {
        if Self::quota_preflight_clean() {
            return 0;
        }
        Self::verifier_provider_parked_ms(provider)
    }

    fn preflight_headroom_low(provider: api::ProviderKind) -> bool {
        !Self::quota_preflight_clean() && api::quota::rate_limit_headroom_low(provider)
    }

    fn preflight_exhausted_ms(provider: api::ProviderKind) -> u64 {
        if Self::quota_preflight_clean() {
            return 0;
        }
        api::quota::provider_quota_exhausted_remaining_ms(provider)
    }

    /// Whether some eligible verifier candidate has quota headroom right now.
    /// When one does, a parked candidate is strictly worse and is skipped
    /// outright; when none does, skipping every parked candidate would degrade
    /// VERIFY onto the model that just wrote the diff, so a short park is ridden
    /// out instead. See [`skip_parked_verifier`].
    fn verifier_alternative_usable(&self) -> bool {
        self.deep_verify_candidates.iter().any(|(_, model)| {
            let provider = api::detect_provider_kind(model);
            self.verifier_candidate_eligible(model)
                && Self::preflight_parked_ms(provider) == 0
                && !Self::preflight_headroom_low(provider)
        })
    }

    /// How many capacity (429) retries the VERIFY leg's stream may spend before
    /// the error propagates so [`Self::verify_subturn`]'s ranked walk can move
    /// on. `None` outside a verify leg — this answer is only about the leg, and
    /// the stream falls through to `main_turn_rate_limit_retry_cap`, which asks
    /// the main turn's own question (has this wall anywhere to go?).
    ///
    /// Inside the leg the escape route decides the cap. A later
    /// different-provider candidate is a real cross-model verifier, so the first
    /// 429 should hand over at once (`Some(0)`) rather than burn the backoff
    /// ladder rediscovering a wall the walk already routes around. With no such
    /// candidate the only escape is the terminal native fallback — which lets
    /// the model that just wrote the diff verify it — so absorb one short burst
    /// retry first (`Some(1)`) instead of degrading on a blip. When the
    /// deep-tier invariant bars even that fallback there is nowhere to land, so
    /// the cap widens slightly (`Some(2)`) to ride out a genuine burst — but it
    /// stays a cap. No amount of backoff resolves an exhausted quota window, and
    /// the walk ends the leg either way, so spending the full wall-clock budget
    /// would only hold the user's turn for minutes to reach the same stop.
    pub(super) fn verifier_rate_limit_retry_cap(&self) -> Option<u32> {
        if !self.deep_verify_leg_active {
            return None;
        }
        let (_, current_model) = self
            .deep_verify_candidates
            .get(self.deep_verify_candidate_idx)?;
        let current_provider = api::detect_provider_kind(current_model);
        let cross_provider_candidate_left = self
            .deep_verify_candidates
            .iter()
            .skip(self.deep_verify_candidate_idx + 1)
            .any(|(_, candidate)| api::detect_provider_kind(candidate) != current_provider);
        if cross_provider_candidate_left {
            return Some(0);
        }
        // Never hand the verify leg the full wall-clock budget. VERIFY is
        // optional work behind the outer attempt loop, and no amount of backoff
        // resolves an exhausted quota window — riding the ladder out just spends
        // minutes to reach the same failure. A native fallback still gets the
        // shorter cap because it has somewhere to land; without one the leg only
        // needs enough retries to ride out a genuine burst.
        let native_fallback_available = !self.deep_tier_only || self.native_model_is_deep_tier();
        Some(if native_fallback_available { 1 } else { 2 })
    }

    /// Pre-flight one ranked verifier candidate against the quota registry,
    /// emitting the skip note when its provider is parked. `true` means the walk
    /// moves on without spending a sub-turn: a live 429 is only discovered
    /// *after* the stream's rate-limit retry budget is burned (ten backoffs —
    /// minutes of stall) even though the registry already knows.
    /// Takes no `&self`: holding a shared borrow of the runtime across this
    /// `await` would make the whole turn future require `ConversationRuntime:
    /// Sync`, which it is not (it owns a `Box<dyn HookProgressReporter + Send>`),
    /// and every host spawn site would stop compiling.
    async fn verifier_parked_skip_note(
        model: &str,
        provider: api::ProviderKind,
        alternative_usable: bool,
        render_tx: &mpsc::Sender<RenderBlock>,
        ids: &BlockIdGen,
    ) -> bool {
        let parked_ms = Self::preflight_parked_ms(provider);
        let headroom_low = Self::preflight_headroom_low(provider);
        let exhausted_ms = Self::preflight_exhausted_ms(provider);
        if !skip_parked_verifier(parked_ms, headroom_low, alternative_usable, exhausted_ms) {
            return false;
        }
        // A lapsed cooldown carries no seconds figure, so name the state rather
        // than print a bare `0s`. A reported reset window gets minutes: it is
        // routinely hours, where a seconds figure is unreadable.
        let reason = if exhausted_ms > 0 {
            format!(
                "quota exhausted (resets in ~{}m)",
                exhausted_ms.div_ceil(60_000)
            )
        } else if parked_ms > 0 {
            format!("rate-limited ({}s cooldown)", parked_ms.div_ceil(1_000))
        } else {
            "recently rate-limited".to_string()
        };
        deep_note(
            render_tx,
            ids,
            format!("auto: verifier {model} {reason} — skipping to the next-ranked provider…"),
        )
        .await;
        true
    }

    /// The verifier the ranked walk will attempt first under the current quota
    /// state, shown before a VERIFY attempt starts. Applies the walk's own
    /// eligibility gate and [`skip_parked_verifier`] policy, so the note can
    /// never promise a candidate the walk passes over. `None` when every
    /// eligible candidate is skipped and VERIFY falls through to the native
    /// client, which renders the honest non-cross-model note instead.
    fn deep_verify_primary_model_label(&self) -> Option<&str> {
        let alternative_usable = self.verifier_alternative_usable();
        self.deep_verify_candidates
            .iter()
            .map(|(_, model)| model.as_str())
            .find(|model| {
                let provider = api::detect_provider_kind(model);
                self.verifier_candidate_eligible(model)
                    && !skip_parked_verifier(
                        Self::preflight_parked_ms(provider),
                        Self::preflight_headroom_low(provider),
                        alternative_usable,
                        Self::preflight_exhausted_ms(provider),
                    )
            })
    }

    /// Verifier model that actually produced the current attempt's verdict.
    fn deep_verify_succeeded_model_label(&self) -> Option<&str> {
        self.deep_verify_succeeded_model.as_deref()
    }

    /// Run a deliberate turn: PLAN (read-only, re-tried until structurally
    /// valid) → IMPLEMENT → objective check → VERIFY (read-only) → decide, with
    /// bounded retries fed the failure contract. `Accept` and `GiveUp` both end
    /// the turn honestly; `GiveUp` leaves the work tree in its last state rather
    /// than pretending success. Returns the combined summary of every phase.
    ///
    /// This is the `PlanFirst` phase driver behind
    /// [`Self::run_turn_streaming_maybe_deep`]: it drives internal sub-turns
    /// only and deliberately skips the per-turn host lifecycle — the
    /// `UserPromptSubmit` hook and the `TeamInbox` digest injection/settle run
    /// in that outer loop. Calling this directly bypasses those policies.
    ///
    /// # Errors
    /// Propagates any [`StreamingTurnError`] from a phase sub-turn.
    #[allow(clippy::too_many_lines)]
    pub async fn run_deep_turn_streaming(
        &mut self,
        user_input: impl Into<String>,
        images: Vec<(String, String)>,
        render_tx: mpsc::Sender<RenderBlock>,
        prompter: Arc<dyn AsyncPermissionPrompter>,
    ) -> Result<(TurnSummary, DeepOutcome), StreamingTurnError> {
        let cfg = self.deep_gate.clone().unwrap_or_default();
        let max = cfg.max_attempts.max(1);
        let task = user_input.into();
        let ids = BlockIdGen::default();
        let base_mode = self.permission_policy.active_mode();
        let mut acc = DeepSummaryAcc::default();
        // Fresh deep turn: forget any verifier model recorded by a previous
        // turn so the summary reports this turn's successful verifier (or none).
        self.deep_verify_succeeded_model = None;

        // Baseline: show the check's starting state to the planner (cheap, and
        // it tells the model whether it is starting red or must keep green). The
        // check itself is offloaded to a blocking thread inside `command_is_green`
        // so it never freezes the host event loop while it runs.
        let baseline = match cfg.check_command.as_deref() {
            Some(cmd) => {
                let green = command_is_green(cmd).await;
                Some(if green {
                    format!("Baseline check `{cmd}` currently PASSES; keep it green.")
                } else {
                    format!("Baseline check `{cmd}` currently FAILS; this is the red state to fix.")
                })
            }
            None => None,
        };

        // ── PLAN phase (ReadOnly): re-plan until structurally valid. ──
        deep_note(&render_tx, &ids, "deep: PLAN phase (read-only)…").await;
        let mut plan_md = String::new();
        let mut plan_verdict = PlanVerdict {
            valid: false,
            missing: Vec::new(),
        };
        let mut pending_images = Some(images);
        let mut missing: Vec<String> = Vec::new();
        let plan_client = self.plan_leg_client()?;
        for _ in 1..=max {
            let prompt = plan_prompt(&task, baseline.as_deref(), &missing);
            let phase_images = pending_images.take().unwrap_or_default();
            let summary = self
                .deep_subturn(
                    prompt,
                    phase_images,
                    PermissionMode::ReadOnly,
                    plan_client,
                    DeepSubturnPhase::Plan,
                    &render_tx,
                    &prompter,
                )
                .await?;
            acc.fold(summary.summary);
            plan_md = self.last_assistant_text();
            plan_verdict = validate_plan(&plan_md);
            if plan_verdict.valid {
                deep_note(&render_tx, &ids, "deep: plan valid ✓").await;
                break;
            }
            missing = plan_verdict.missing.clone();
            deep_note(
                &render_tx,
                &ids,
                format!("deep: plan missing [{}] — re-planning", missing.join(", ")),
            )
            .await;
        }
        // If still invalid after `max` tries, proceed honestly: plan validity is
        // surfaced in the outcome, and the verifier still gates acceptance.

        // ── IMPLEMENT → check → VERIFY → decide, bounded retries. ──
        let mut extra: Option<String> = None;
        let mut exec_retry: Option<String> = None;
        let mut exec_transport_escalated = false;
        let mut decision = DeepDecision::GiveUp;
        let mut attempts = 0u32;
        // Previous attempt's verifier issues, for the ALP §3 "no more progress"
        // stop condition (same as the reactive loop).
        let mut prev_issues: Vec<String> = Vec::new();
        let mut verify_depth_floor = VerifyDepth::Skip;
        let mut verification: Option<bool> = None;
        // Final attempt's verifier issues, surfaced on the outcome for the
        // goal-level repair prompt (same as the reactive loop).
        let mut verifier_issues: Vec<String> = Vec::new();
        // Phase 4 verdict-channel seam (same as the reactive loop): the final
        // attempt's raw verifier parse confidence and the verifier model.
        let mut verifier_parse: Option<VerifierParse> = None;
        let mut verifier_model: Option<String> = None;
        // Pre-edit objective baseline (see `run_auto_turn_streaming`): a check that
        // is already red before this deep turn edits anything is an out-of-scope
        // baseline failure, so it does not force the retry loop; an edit-introduced
        // green→red regression still gates.
        let baseline_objective_green = match cfg.check_command.as_deref() {
            Some(cmd) => command_is_green(cmd).await,
            None => true,
        };
        for attempt in 1..=max {
            attempts = attempt;

            // Runtime effort escalation (the mechanism `auto_effort_for_prompt`'s
            // doc delegates to): the first attempt runs at the configured effort,
            // but a retry means that effort did not solve the task — the bench
            // shows hard tasks `High` cannot pass do pass at `Xhigh`. So power up
            // every retry to at least `Xhigh` (a floor, so a task already above
            // it is not lowered). Cleared after the loop.
            if attempt > 1 {
                self.set_effort_override(Some(super::ESCALATION_EFFORT_BUDGET));
                deep_note(
                    &render_tx,
                    &ids,
                    "deep: escalating reasoning effort (xhigh) for retry…",
                )
                .await;
            }

            deep_note(
                &render_tx,
                &ids,
                format!("deep: EXEC attempt {attempt}/{max}…"),
            )
            .await;
            let baseline_files = changed_files_snapshot_async().await;
            // Sampled HERE, next to the baseline it anchors: EXEC may move
            // the process cwd (`EnterWorktree`), and an own_root sampled
            // after EXEC would key `baseline_files` in a different frame
            // than the snapshot they came from.
            let own_root = own_repo_root_async().await;
            if exec_transport_escalated {
                self.reserved_edit_gate = false;
                if attempt == 2 {
                    if let Some(note) = self.exec_transport_escalation_note() {
                        deep_note(&render_tx, &ids, note).await;
                    }
                }
            } else if let Some(note) = self.exec_leg_note(attempt) {
                deep_note(&render_tx, &ids, note).await;
            }
            if self.exec_swap_enabled() && attempt > ARCHITECT_IMPL_ATTEMPTS {
                // Failure escalation: the native (reserved) model implements
                // from here on, so the edit gate stands down for this turn.
                self.reserved_edit_gate = false;
            }
            let exec_client = if exec_transport_escalated {
                SubturnClient::Native
            } else {
                self.exec_leg_client(attempt)
            };
            let exec_was_implementer = exec_client == SubturnClient::Implementer;
            let exec_result = self
                .deep_subturn(
                    exec_prompt(&task, &plan_md, exec_retry.as_deref()),
                    Vec::new(),
                    base_mode,
                    exec_client,
                    DeepSubturnPhase::Exec,
                    &render_tx,
                    &prompter,
                )
                .await;
            // Clear the escalation floor immediately after the (possibly
            // escalated) EXEC sub-turn — before retry handling and VERIFY — so
            // it never leaks into the read-only verify turn or a later turn on
            // error. Idempotent when no escalation was set.
            self.set_effort_override(None);
            let exec_result = match exec_result {
                Ok(result) => result,
                Err(error) if attempt < max => {
                    if let Some(retry_context) = exec_transport_retry_context(&error) {
                        if exec_was_implementer {
                            exec_transport_escalated = true;
                        }
                        exec_retry = Some(retry_context);
                        deep_note(
                            &render_tx,
                            &ids,
                            "deep: EXEC transport failure — retrying…",
                        )
                        .await;
                        continue;
                    }
                    return Err(error);
                }
                Err(error) => return Err(error),
            };
            let verify_images = exec_result.verify_images;
            let summary = exec_result.summary;
            let edited_paths = edited_file_paths(&summary);
            let assistant_claim = latest_assistant_text(&summary.assistant_messages);
            let green_checks = exec_green_checks(&summary);
            // Snapshot before the fold consumes `summary`: the SingleLens
            // attachment set needs what EXEC consulted this attempt.
            let consulted_paths = exec_consulted_paths(&summary);
            acc.fold(summary);

            // Objective gate: the project's own check command, when configured.
            let check_observation = match cfg.check_command.as_deref() {
                Some(cmd) => {
                    let observation = run_check_command(cmd).await;
                    self.record_objective_check_verdict(observation.green);
                    deep_note(
                        &render_tx,
                        &ids,
                        format!(
                            "deep: check `{cmd}` → {}",
                            if observation.green {
                                "green ✓"
                            } else {
                                "red ✗"
                            }
                        ),
                    )
                    .await;
                    Some(observation)
                }
                None => None,
            };
            let objective_ok = check_observation.as_ref().is_none_or(|check| check.green);

            let after_files = changed_files_snapshot_async().await;
            let diff_paths = attempt_diff_paths(
                &baseline_files,
                &after_files,
                &edited_paths,
                own_root.as_deref(),
            );
            let (diff, line_churn) =
                bounded_git_diff_for_paths(diff_paths.clone(), 6000, own_root.clone()).await;
            let selected_depth = verify_depth_for_band(
                self.verify_band,
                diff_paths.len(),
                line_churn,
                objective_ok,
                paths_touch_security(&diff_paths),
                paths_touch_tests(&diff_paths),
            );
            // Two floors, both monotone: the retry floor (a later attempt never
            // verifies more shallowly than an earlier one) and this turn's
            // probed intent (a design turn never skips verification outright).
            // Neither can force `Full` on its own.
            let intent = self.verify_intent;
            let depth = selected_depth
                .max(verify_depth_floor)
                .max(intent_verify_floor(intent));
            verify_depth_floor = depth;
            if depth == VerifyDepth::Skip {
                deep_note(
                    &render_tx,
                    &ids,
                    "deep: trivial green change — skipping deep verify",
                )
                .await;
                decision = DeepDecision::Accept;
                break;
            }
            let verify_note = match self.deep_verify_primary_model_label() {
                Some(model) => format!("deep: VERIFY phase (read-only, cross-model {model})…"),
                None => "deep: VERIFY phase (read-only)…".to_string(),
            };
            deep_note(&render_tx, &ids, verify_note).await;
            // The leg deliberately inherits the session's thinking/effort
            // config unchanged: any per-leg config delta (e.g. disabling
            // thinking for a SingleLens verdict) invalidates the provider's
            // message-level prompt cache and re-bills the whole conversation
            // prefix as cache-write — measured at 14-25k tokens per leg,
            // dwarfing the few hundred thinking tokens it would save.
            // Post-edit file contents ride the SingleLens prompt so the
            // common leg verdicts in one call; Full legs skip the read (the
            // prompt drops it anyway, and large/red changes read files
            // selectively themselves).
            let attached_files = if depth == VerifyDepth::SingleLens {
                verify_file_attachments(
                    &verify_attachment_paths(&diff_paths, &consulted_paths, own_root.as_deref()),
                    own_root.as_deref(),
                )
            } else {
                String::new()
            };
            // Read the session ledger HERE — after every gate that can skip the
            // leg — so the attest counts legs that were actually built. Computed
            // earlier it would record a decline for turns that never opened a
            // verifier, and a counter diluted by non-events cannot answer the
            // only question it exists for: how often the join had something to
            // say when a verifier was listening.
            //
            // The ledger is current because the tool-result seam kept it so
            // THROUGH the EXEC leg above: any earlier check those edits
            // invalidated is already excluded, rather than being handed to the
            // verifier as a still-true fact.
            let session_checks = self.verify_leg_session_checks(&green_checks);
            let verify_result = self
                .verify_subturn(
                    verify_prompt(
                        &task_with_retry_context(&task, extra.as_deref()),
                        &diff,
                        cfg.check_command
                            .as_deref()
                            .zip(check_observation.as_ref()),
                        GreenCheckEvidence {
                            this_turn: &green_checks,
                            session: &session_checks,
                        },
                        &diff_paths,
                        &preexisting_dirty_paths(
                            &baseline_files,
                            &diff_paths,
                            own_root.as_deref(),
                        ),
                        &assistant_claim,
                        &attached_files,
                        if depth == VerifyDepth::SingleLens {
                            VerifyLensMode::SpecOnly
                        } else {
                            VerifyLensMode::Full
                        },
                        intent,
                        !verify_images.is_empty(),
                    ),
                    verify_images,
                    &render_tx,
                    &ids,
                    &prompter,
                )
                .await;
            // A failed VERIFY leg (transient streaming error) must NOT throw away
            // the EXEC edits already applied this attempt via `?`. Fold a
            // conservative non-accept (Timeout) so the loop retries or gives up at
            // the cap, preserving the completed implementation in the work tree.
            let verifier = match verify_result {
                Ok(mut summary) => {
                    // Same stripping as the reactive path: the leg-local cap
                    // must neither budget-stop the whole turn nor read as a
                    // rejection.
                    let verify_budget_stopped = summary.budget_exhausted.take().is_some();
                    acc.fold(summary);
                    if verify_budget_stopped {
                        deep_note(
                            &render_tx,
                            &ids,
                            format!(
                                "verifier hit its inspection budget ({VERIFY_LEG_MAX_ITERATIONS} rounds) without a verdict — the objective check gates this attempt"
                            ),
                        )
                        .await;
                        verify_budget_exhausted_verdict()
                    } else {
                        parse_verify_leg_text(&self.last_assistant_text())
                    }
                }
                Err(_) => verify_leg_failed_verdict(),
            };
            // A still-red baseline failure is out of scope: only an edit-introduced
            // regression gates the deep loop. The verifier still sees raw objective.
            let gating_objective_ok = objective_ok || !baseline_objective_green;
            record_verifier_calibration(&self.session.session_id, gating_objective_ok, &verifier);
            // Keep accept/retry/stall policy in decision-core; this runtime only
            // supplies observed IO facts from the live VERIFY sub-turn.
            let folded = fold_verification_attempt(
                attempt,
                max,
                gating_objective_ok,
                &verifier,
                &prev_issues,
            );

            verification = Some(goal_facing_accept(&folded));
            verifier_issues = verifier.issues.clone();
            verifier_parse = Some(verifier.parse);
            verifier_model = self
                .deep_verify_succeeded_model_label()
                .map(str::to_string);
            decision = folded.decision;
            deep_note(
                &render_tx,
                &ids,
                verification_outcome_note("deep", decision, attempt, max, objective_ok, &verifier),
            )
            .await;

            match decision {
                DeepDecision::Accept | DeepDecision::GiveUp => {
                    if decision == DeepDecision::Accept {
                        self.record_verified_accept(objective_ok);
                    }
                    // Retry-conversion evidence: only a turn that actually
                    // spent a retry carries signal. fired = the retry
                    // converted to Accept; failed = the budget went for
                    // nothing — the fired:failed ratio is what lets a future
                    // max-attempts change cite measurement instead of taste.
                    if attempt > 1 {
                        if decision == DeepDecision::Accept {
                            telemetry::attest_fired(telemetry::HarnessFeature::DeepRetryConversion);
                        } else {
                            telemetry::attest_failed(
                                telemetry::HarnessFeature::DeepRetryConversion,
                                "gave_up",
                            );
                        }
                    }
                    break;
                }
                DeepDecision::Retry => {
                    let repair = failure_summary(objective_ok, &verifier);
                    exec_retry = Some(exec_retry_context(
                        &repair,
                        &diff,
                        &diff_paths,
                        cfg.check_command
                            .as_deref()
                            .zip(check_observation.as_ref()),
                    ));
                    extra = Some(repair);
                    prev_issues = verifier.issues.clone();
                }
            }
        }

        let outcome = DeepOutcome {
            decision,
            attempts,
            plan_valid: plan_verdict.valid,
            plan_missing: plan_verdict.missing,
            // Preserve the semantic verifier gate observed when VERIFY ran;
            // proportional trivial-change skips leave it as `None`.
            verification,
            issues: verifier_issues,
            verifier_parse,
            verifier_model,
        };
        deep_note(
            &render_tx,
            &ids,
            format!(
                "deep: {} after {attempts} attempt(s) · plan {}",
                decision.as_str(),
                if outcome.plan_valid {
                    "valid"
                } else {
                    "invalid"
                }
            ),
        )
        .await;

        Ok((acc.into_summary(), outcome))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_assert_heredoc_is_check_shaped() {
        // The dominant deep-lane self-check shape, verbatim from a bench
        // transcript: no tool name on the command line, just `python3 -`.
        let command = "python3 - <<'EOF'\nimport cardparse\nassert cardparse.parse('# a\\n')[0]['name'] == 'a'\nEOF";
        assert!(command_is_check_shaped(command));
        // Path-qualified interpreter and a pipeline prefix still count.
        assert!(command_is_check_shaped(
            "cd /tmp && /usr/bin/python3 - <<'PY'\nraise SystemExit(0)\nPY"
        ));
    }

    #[test]
    fn a_transform_heredoc_is_not_check_shaped() {
        // No assertion — a green transform proves nothing worth carrying, and
        // recording it would let a tree-mutating script read as a check.
        assert!(!command_is_check_shaped(
            "python3 - <<'EOF'\nopen('out.txt','w').write('data')\nEOF"
        ));
        // Assert-y body under a non-Python interpreter stays out.
        assert!(!command_is_check_shaped(
            "node - <<'EOF'\nconsole.assert(1 == 1)\nEOF"
        ));
        // A bare heredoc with no interpreter at all stays out.
        assert!(!command_is_check_shaped("cat <<'EOF'\nassert docs\nEOF"));
    }

    fn test_png_bytes(width: u32, height: u32) -> Vec<u8> {
        use image::{DynamicImage, ImageFormat, RgbImage};

        let mut png = Cursor::new(Vec::new());
        DynamicImage::ImageRgb8(RgbImage::new(width, height))
            .write_to(&mut png, ImageFormat::Png)
            .expect("encode PNG fixture");
        png.into_inner()
    }

    fn test_png_base64(width: u32, height: u32) -> String {
        base64::engine::general_purpose::STANDARD.encode(test_png_bytes(width, height))
    }

    fn tool_image_message(id: usize, images: Vec<(String, String)>) -> ConversationMessage {
        ConversationMessage::tool_result_with_images(
            format!("tool-{id}"),
            "screenshot",
            "captured",
            false,
            images,
        )
    }

    #[test]
    fn exec_image_harvest_is_newest_first_and_capped_at_three() {
        let payloads = (1..=5)
            .map(|width| test_png_base64(width, 1))
            .collect::<Vec<_>>();
        let messages = payloads
            .iter()
            .enumerate()
            .map(|(id, data)| {
                tool_image_message(id, vec![("image/png".to_string(), data.clone())])
            })
            .collect::<Vec<_>>();

        let harvested = harvest_exec_tool_images(&messages);
        assert_eq!(harvested.len(), VERIFY_IMAGE_MAX_COUNT);
        assert_eq!(
            harvested
                .iter()
                .map(|(_, data)| data)
                .collect::<Vec<_>>(),
            vec![&payloads[4], &payloads[3], &payloads[2]]
        );
    }

    #[test]
    fn exec_image_harvest_deduplicates_identical_payloads() {
        let first = test_png_base64(1, 1);
        let second = test_png_base64(2, 1);
        let messages = vec![tool_image_message(
            0,
            vec![
                ("image/png".to_string(), first.clone()),
                ("image/png".to_string(), second.clone()),
                ("image/png".to_string(), first.clone()),
            ],
        )];

        let harvested = harvest_exec_tool_images(&messages);
        assert_eq!(
            harvested,
            vec![
                ("image/png".to_string(), first),
                ("image/png".to_string(), second),
            ]
        );
    }

    #[test]
    fn exec_image_harvest_guards_every_forwarded_payload() {
        let valid = test_png_base64(4, 4);
        let malformed = base64::engine::general_purpose::STANDARD.encode(b"not an image");
        let mut rejected_bytes =
            test_png_bytes(crate::image_guard::IMAGE_CLAMP_DIMENSION + 1, 1);
        let idat = rejected_bytes
            .windows(4)
            .position(|window| window == b"IDAT")
            .expect("PNG fixture has IDAT");
        rejected_bytes[idat + 4] ^= 0xff;
        assert!(matches!(
            crate::image_guard::guard_image_bytes(&rejected_bytes),
            crate::image_guard::ImageGuardOutcome::DropOversized { .. }
        ));
        let rejected = base64::engine::general_purpose::STANDARD.encode(rejected_bytes);
        let messages = vec![tool_image_message(
            0,
            vec![
                ("image/png".to_string(), valid.clone()),
                ("image/png".to_string(), malformed),
                ("image/png".to_string(), rejected),
                ("image/png".to_string(), "@@ invalid base64 @@".to_string()),
            ],
        )];

        assert_eq!(
            harvest_exec_tool_images(&messages),
            vec![("image/png".to_string(), valid)]
        );

        let oversized = test_png_base64(crate::image_guard::IMAGE_CLAMP_DIMENSION + 1, 1);
        let resized = harvest_exec_tool_images(&[tool_image_message(
            1,
            vec![("image/png".to_string(), oversized.clone())],
        )]);
        assert_eq!(resized.len(), 1, "recoverable oversize is guarded by downscaling");
        assert_ne!(resized[0].1, oversized);
        let resized_bytes = base64::engine::general_purpose::STANDARD
            .decode(resized[0].1.as_bytes())
            .expect("guard emits base64");
        assert_eq!(
            crate::image_guard::guard_image_bytes(&resized_bytes),
            crate::image_guard::ImageGuardOutcome::Keep
        );
    }

    #[test]
    fn only_exec_legs_contribute_tool_images() {
        let image = ("image/png".to_string(), test_png_base64(2, 2));
        let messages = vec![
            ConversationMessage::user_with_images("original", vec![image.clone()]),
            tool_image_message(0, vec![image.clone()]),
        ];

        assert_eq!(
            harvest_subturn_images(DeepSubturnPhase::Exec, &messages),
            vec![image]
        );
        assert!(
            harvest_subturn_images(DeepSubturnPhase::Plan, &messages).is_empty(),
            "PLAN tool images must not seed VERIFY"
        );
        assert!(
            harvest_subturn_images(DeepSubturnPhase::Verify, &messages).is_empty(),
            "VERIFY tool images must not seed a later VERIFY"
        );
        assert!(
            harvest_subturn_images(
                DeepSubturnPhase::Exec,
                &[ConversationMessage::user_with_images(
                    "original only",
                    vec![("image/png".to_string(), test_png_base64(3, 3))],
                )],
            )
            .is_empty(),
            "the user's original attachments are not EXEC tool output"
        );
        assert!(
            harvest_subturn_images(
                DeepSubturnPhase::Exec,
                &[ConversationMessage::tool_result(
                    "text-only",
                    "bash",
                    "ok",
                    false,
                )],
            )
            .is_empty(),
            "an EXEC leg with no tool images preserves the empty VERIFY packet"
        );
    }

    /// The parked-verifier skip policy must not trade cross-model verification
    /// for self-verification: skipping is right only when another candidate can
    /// actually run, or when the park is longer than the stream backoff could
    /// ride out anyway.
    #[test]
    fn skip_parked_verifier_prefers_alternatives_but_rides_out_short_parks() {
        // Boundaries come from the ladder itself, never a restated literal.
        let ride_out = verifier_park_ride_out_ms();
        let clearable_park = ride_out / 2;
        // A clear provider is never skipped, with or without an alternative.
        assert!(!skip_parked_verifier(0, false, true, 0));
        assert!(!skip_parked_verifier(0, false, false, 0));
        // With a clear alternative, any park is worth skipping — the walk still
        // lands a real cross-model verifier.
        assert!(skip_parked_verifier(1, false, true, 0));
        assert!(skip_parked_verifier(clearable_park, false, true, 0));
        // With no alternative, skipping degrades VERIFY onto the model that
        // wrote the diff, so a park the ladder can still clear is waited through.
        assert!(!skip_parked_verifier(clearable_park, false, false, 0));
        assert!(!skip_parked_verifier(ride_out, false, false, 0));
        // Only a park past the ladder's ceiling justifies that degradation.
        assert!(skip_parked_verifier(ride_out + 1, false, false, 0));
        // A lapsed cooldown that is still inside the recent-429 window counts:
        // the next request would just collect a fresh 429, so prefer a clear
        // alternative. Without one it is still worth attempting rather than
        // falling through to self-verification.
        assert!(skip_parked_verifier(0, true, true, 0));
        assert!(!skip_parked_verifier(0, true, false, 0));
    }

    /// A reported reset window is the provider's own statement rather than an
    /// inference, so it decides on its own — and it is the only signal that
    /// outlives the cool-down ceiling, which is what stopped a new session from
    /// re-earning the same 429 every two minutes.
    #[test]
    fn skip_parked_verifier_trusts_a_reported_reset_window_over_every_inference() {
        let ride_out = verifier_park_ride_out_ms();
        // Skips even with no alternative: attempting buys a guaranteed 429,
        // which is a worse trade than degrading to self-verification.
        assert!(skip_parked_verifier(0, false, false, 1));
        assert!(skip_parked_verifier(0, false, true, 1));
        // Outlives the ladder: a clear cooldown and clear headroom — the exact
        // state a fresh process starts in — still skip while the window holds.
        assert!(skip_parked_verifier(0, false, false, ride_out + 1));
        // A lapsed window decides nothing, leaving the other signals in charge.
        assert!(!skip_parked_verifier(0, false, false, 0));
    }

    #[test]
    fn detect_check_command_finds_cargo_in_this_crate() {
        // `cargo test -p runtime` runs with the crate root as cwd, which has a
        // Cargo.toml, so detection must pick the Rust check command.
        assert_eq!(
            detect_check_command().as_deref(),
            Some("cargo build --tests")
        );
    }

    #[test]
    fn detect_check_command_reactive_default_is_a_cheap_build_not_a_full_test_run() {
        // The detected command is auto-wired as the reactive per-coding-turn gate
        // (it runs after *every* edited turn — see
        // `install_reactive_verify_gate_if_coding`). On a large repo a full
        // `cargo test` would force a multi-minute test *build + run* after each
        // edit, freezing the loop on the objective check. The Rust auto default
        // must therefore compile the test targets without running them: a green
        // build is a real objective signal at a fraction of the cost.
        let cmd =
            detect_check_command().expect("this crate has a Cargo.toml, so a command is detected");

        // It must still drive the test targets through the compiler (so it catches
        // the same build/type errors `cargo test` would surface)…
        assert!(
            cmd.starts_with("cargo build") && cmd.contains("--tests"),
            "Rust reactive default must build the test targets, got {cmd:?}"
        );
        // …but it must NOT be the heavy full `cargo test` run that this finding
        // replaces.
        assert_ne!(
            cmd, "cargo test",
            "the reactive auto default must not be a full multi-minute test run"
        );
    }

    #[test]
    fn interpret_green_reads_exit_codes() {
        assert!(interpret_green(None));
        assert!(interpret_green(Some("exit_code:0")));
        assert!(!interpret_green(Some("exit_code:1")));
        assert!(!interpret_green(Some("exit_code:137")));
        assert!(!interpret_green(Some("timeout")));
        assert!(!interpret_green(Some("garbage")));
    }

    #[test]
    #[allow(clippy::too_many_lines)] // exhaustive band × diff-fact matrix
    fn verify_depth_is_conservative_across_band_and_change_matrix() {
        assert_eq!(
            verify_depth(
                RouteTaskComplexity::Trivial,
                RouteTaskRisk::Low,
                1,
                CHURN_TRIVIAL_MAX,
                true,
                false,
                false,
            ),
            VerifyDepth::Skip
        );
        for risk in [RouteTaskRisk::Low, RouteTaskRisk::Medium] {
            assert_eq!(
                verify_depth(
                    RouteTaskComplexity::Small,
                    risk,
                    FILES_SMALL_MAX,
                    CHURN_SMALL_MAX,
                    true,
                    false,
                    false,
                ),
                VerifyDepth::SingleLens
            );
        }
        // A measured-small green diff demotes the Medium band guess to one
        // lens, and keeps the ladder monotone: Trivial at Medium risk (or
        // above the tiny-skip caps) must never verify deeper than Small.
        assert_eq!(
            verify_depth(
                RouteTaskComplexity::Medium,
                RouteTaskRisk::Low,
                1,
                1,
                true,
                false,
                false,
            ),
            VerifyDepth::SingleLens
        );
        assert_eq!(
            verify_depth(
                RouteTaskComplexity::Medium,
                RouteTaskRisk::Medium,
                FILES_SMALL_MAX,
                CHURN_SMALL_MAX,
                true,
                false,
                false,
            ),
            VerifyDepth::SingleLens
        );
        assert_eq!(
            verify_depth(
                RouteTaskComplexity::Trivial,
                RouteTaskRisk::Medium,
                1,
                1,
                true,
                false,
                false,
            ),
            VerifyDepth::SingleLens
        );
        assert_eq!(
            verify_depth(
                RouteTaskComplexity::Trivial,
                RouteTaskRisk::Low,
                1,
                CHURN_TRIVIAL_MAX + 1,
                true,
                false,
                false,
            ),
            VerifyDepth::SingleLens
        );
        // The facts only rescue Medium while they stay small: an oversized
        // churn or file count drops it right back to Full.
        assert_eq!(
            verify_depth(
                RouteTaskComplexity::Medium,
                RouteTaskRisk::Low,
                1,
                CHURN_SMALL_MAX + 1,
                true,
                false,
                false,
            ),
            VerifyDepth::Full
        );
        assert_eq!(
            verify_depth(
                RouteTaskComplexity::Medium,
                RouteTaskRisk::Low,
                FILES_SMALL_MAX + 1,
                1,
                true,
                false,
                false,
            ),
            VerifyDepth::Full
        );
        for complexity in [RouteTaskComplexity::Large, RouteTaskComplexity::Unknown] {
            assert_eq!(
                verify_depth(complexity, RouteTaskRisk::Low, 1, 1, true, false, false),
                VerifyDepth::Full
            );
        }
        for risk in [RouteTaskRisk::High, RouteTaskRisk::Critical] {
            assert_eq!(
                verify_depth(
                    RouteTaskComplexity::Trivial,
                    risk,
                    1,
                    1,
                    true,
                    false,
                    false,
                ),
                VerifyDepth::Full
            );
        }
        assert_eq!(
            verify_depth(
                RouteTaskComplexity::Small,
                RouteTaskRisk::Low,
                FILES_SMALL_MAX + 1,
                1,
                true,
                false,
                false,
            ),
            VerifyDepth::Full
        );
        assert_eq!(
            verify_depth(
                RouteTaskComplexity::Small,
                RouteTaskRisk::Low,
                1,
                CHURN_SMALL_MAX + 1,
                true,
                false,
                false,
            ),
            VerifyDepth::Full
        );
    }

    #[test]
    fn verify_depth_forces_full_on_failed_sensitive_or_unknown_changes() {
        assert_eq!(
            verify_depth(
                RouteTaskComplexity::Trivial,
                RouteTaskRisk::Low,
                1,
                1,
                false,
                false,
                false,
            ),
            VerifyDepth::Full
        );
        assert_eq!(
            verify_depth(
                RouteTaskComplexity::Trivial,
                RouteTaskRisk::Low,
                0,
                0,
                true,
                false,
                false,
            ),
            VerifyDepth::Full,
            "an unscoped edit must not bypass verification"
        );

        let security_paths = vec!["src/Auth/session.rs".to_string()];
        assert_eq!(
            verify_depth(
                RouteTaskComplexity::Trivial,
                RouteTaskRisk::Low,
                1,
                1,
                true,
                paths_touch_security(&security_paths),
                false,
            ),
            VerifyDepth::Full
        );
        let test_paths = vec!["tests/parser_spec.rs".to_string()];
        assert_eq!(
            verify_depth(
                RouteTaskComplexity::Trivial,
                RouteTaskRisk::Low,
                1,
                1,
                true,
                false,
                paths_touch_tests(&test_paths),
            ),
            VerifyDepth::Full
        );

        assert_eq!(
            verify_depth(
                RouteTaskComplexity::Unknown,
                RouteTaskRisk::Low,
                1,
                1,
                true,
                false,
                false,
            ),
            VerifyDepth::Full
        );
        assert_eq!(
            verify_depth(
                RouteTaskComplexity::Trivial,
                RouteTaskRisk::Unknown,
                1,
                1,
                true,
                false,
                false,
            ),
            VerifyDepth::Full
        );
        assert_eq!(
            verify_depth_for_band(None, 1, 1, true, false, false),
            VerifyDepth::Full,
            "an absent host band must preserve full verification"
        );
    }

    #[test]
    fn verify_depth_retry_floor_never_downgrades() {
        assert_eq!(
            VerifyDepth::SingleLens.max(VerifyDepth::Full),
            VerifyDepth::Full
        );
        assert_eq!(
            VerifyDepth::Skip.max(VerifyDepth::SingleLens),
            VerifyDepth::SingleLens
        );
    }

    #[test]
    fn diff_line_churn_excludes_patch_headers() {
        let diff =
            "--- a/src/lib.rs\n+++ b/src/lib.rs\n-old\n+new\n----deleted\n++++added\n context\n";
        assert_eq!(diff_line_churn(diff), 4);
    }

    /// The PLAN/VERIFY delegation ban is an execution gate, not a wire edit.
    ///
    /// The four tools it names only require `ReadOnly`, so the phase's read-only
    /// clamp does NOT reach them — a bounded leg could call `Agent` and really
    /// spawn a sub-agent, which is what the old advertise-time exclusion existed
    /// to prevent. This pins the replacement: denied while the phase owns the
    /// policy, allowed again the moment it does not, and never at the cost of a
    /// tool definition moving on the wire.
    #[test]
    fn deep_leg_phase_blocks_delegation_tools_without_touching_the_wire() {
        use crate::permissions::PermissionOutcome;

        let mut policy = crate::PermissionPolicy::new(crate::PermissionMode::ReadOnly)
            .with_tool_requirement("Agent", crate::PermissionMode::ReadOnly)
            .with_tool_requirement("SpawnMultiAgent", crate::PermissionMode::ReadOnly)
            .with_tool_requirement("Workflow", crate::PermissionMode::ReadOnly)
            .with_tool_requirement("read_file", crate::PermissionMode::ReadOnly);

        // Premise: the read-only clamp alone lets all three through, so the
        // block is doing work no mode could do.
        for tool in ["Agent", "SpawnMultiAgent", "Workflow"] {
            assert_eq!(
                policy.authorize(tool, "{}", None),
                PermissionOutcome::Allow,
                "premise: {tool} needs only read-only"
            );
        }

        let saved = policy.begin_phase_tool_block(DEEP_LEG_DELEGATION_TOOLS);
        for tool in DEEP_LEG_DELEGATION_TOOLS {
            let outcome = policy.authorize(tool, "{}", None);
            let PermissionOutcome::Deny { reason } = outcome else {
                panic!("{tool} must be denied during a bounded leg, got {outcome:?}");
            };
            assert!(
                reason.contains("architect phase") && reason.contains("must not delegate"),
                "the denial must name the phase, not the permission mode: {reason}"
            );
        }
        // Case cannot slip the block, and the enforcement layer's unconditional
        // path sees it too (otherwise Prompt mode would defer to the prompter).
        assert!(matches!(
            policy.authorize("agent", "{}", None),
            PermissionOutcome::Deny { .. }
        ));
        assert!(policy.deny_reason("Workflow", "{}").is_some());
        // Unrelated tools are untouched — this is a named list, not a clamp.
        assert_eq!(policy.authorize("read_file", "{}", None), PermissionOutcome::Allow);

        policy.end_phase_tool_block(saved);
        for tool in DEEP_LEG_DELEGATION_TOOLS.iter().take(3) {
            assert_eq!(
                policy.authorize(tool, "{}", None),
                PermissionOutcome::Allow,
                "{tool} must be available again once the phase ends"
            );
        }
        assert!(policy.deny_reason("Workflow", "{}").is_none());
    }

    /// Both sides of the delegation ban must name the same tools. The prompts
    /// tell the model what not to call, in prose; the phase block enforces it.
    /// A tool added to the enforced list but not to the prompts would be denied
    /// mid-leg with no warning, which reads to the model as a broken tool — so
    /// the list is pinned against the prompts that must mention it.
    #[test]
    fn every_blocked_delegation_tool_is_named_in_the_leg_prompts() {
        let plan = plan_prompt("do the thing", None, &[]);
        for tool in DEEP_LEG_DELEGATION_TOOLS {
            assert!(
                plan.contains(tool),
                "the PLAN prompt must tell the model {tool} is off limits"
            );
        }
    }

    /// Nested phases must restore, not clear: a VERIFY leg opened inside another
    /// blocked phase used to be able to hand delegation back to the outer one.
    #[test]
    fn a_nested_phase_block_restores_the_outer_block() {
        use crate::permissions::PermissionOutcome;

        let mut policy = crate::PermissionPolicy::new(crate::PermissionMode::ReadOnly)
            .with_tool_requirement("Agent", crate::PermissionMode::ReadOnly);

        let outer = policy.begin_phase_tool_block(DEEP_LEG_DELEGATION_TOOLS);
        let inner = policy.begin_phase_tool_block(DEEP_LEG_DELEGATION_TOOLS);
        policy.end_phase_tool_block(inner);
        assert!(
            matches!(
                policy.authorize("Agent", "{}", None),
                PermissionOutcome::Deny { .. }
            ),
            "the outer phase still forbids delegation"
        );
        policy.end_phase_tool_block(outer);
        assert_eq!(policy.authorize("Agent", "{}", None), PermissionOutcome::Allow);
    }

    #[test]
    fn read_only_allow_rules_unblock_typed_cargo_inspection_not_writes() {
        // End-to-end: the exact rule set `deep_subturn` injects must let the
        // shell-free `Cargo` typed tool run inspection verbs under a downgraded
        // ReadOnly phase (the deep VERIFY denial), while `run`/`build` and other
        // write tools stay gated — matching the `bash(cargo …)` relaxation.
        use crate::permissions::PermissionOutcome;

        let mut policy = crate::PermissionPolicy::new(crate::PermissionMode::ReadOnly)
            .with_tool_requirement("Cargo", crate::PermissionMode::WorkspaceWrite)
            .with_tool_requirement("bash", crate::PermissionMode::DangerFullAccess)
            .with_tool_requirement("write_file", crate::PermissionMode::WorkspaceWrite);

        let grant = policy.add_temporary_allow_rules(read_only_bash_allow_rules());

        // Typed inspection verbs are now allowed…
        for verb in ["check", "test", "clippy", "fmt"] {
            let input = format!(r#"{{"action":"{verb}"}}"#);
            assert_eq!(
                policy.authorize("Cargo", &input, None),
                PermissionOutcome::Allow,
                "Cargo({verb}) should be permitted by the scoped read-only grant"
            );
        }
        // …and so is the equivalent shell form.
        assert_eq!(
            policy.authorize("bash", r#"{"command":"cargo test --all"}"#, None),
            PermissionOutcome::Allow
        );

        // But heavier/arbitrary `Cargo` verbs and unrelated writes stay denied.
        for verb in ["run", "build"] {
            let input = format!(r#"{{"action":"{verb}"}}"#);
            assert!(
                matches!(
                    policy.authorize("Cargo", &input, None),
                    PermissionOutcome::Deny { .. }
                ),
                "Cargo({verb}) must remain gated"
            );
        }
        assert!(matches!(
            policy.authorize("write_file", r#"{"path":"a.rs","content":"x"}"#, None),
            PermissionOutcome::Deny { .. }
        ));

        // No leak once the phase restores.
        policy.remove_temporary_allow_rules(grant);
        assert!(matches!(
            policy.authorize("Cargo", r#"{"action":"test"}"#, None),
            PermissionOutcome::Deny { .. }
        ));
    }

    #[test]
    fn plan_prompt_headers_satisfy_validate_plan() {
        // The PLAN prompt instructs four headers; a plan literally echoing them
        // must validate, proving the prompt and the policy agree.
        let echoed = "## Target files\nx\n## Invariants\ny\n## Expected tests\nz\n## Risks\nw";
        assert!(validate_plan(echoed).valid);
        // And the prompt itself names the canonical sections.
        let prompt = plan_prompt("do a thing", None, &[]);
        assert!(prompt.contains("[deep:PLAN]"));
        for header in [
            "## Target files",
            "## Invariants",
            "## Expected tests",
            "## Risks",
        ] {
            assert!(prompt.contains(header), "missing {header}");
        }
        assert!(
            prompt.contains("Do NOT spawn sub-agents, delegate, or call Agent, SpawnMultiAgent, Workflow, or SendMessage"),
            "bounded PLAN must explicitly forbid delegation"
        );
    }

    #[test]
    fn plan_prompt_carries_baseline_and_missing_feedback() {
        let prompt = plan_prompt(
            "t",
            Some("Baseline check `ct` currently FAILS"),
            &["tests".into(), "risks".into()],
        );
        assert!(prompt.contains("Baseline check `ct` currently FAILS"));
        assert!(
            prompt.contains("missing, empty, or placeholder-only required sections: tests, risks")
        );
        assert!(prompt.contains("concrete, non-placeholder content"));
    }

    #[test]
    fn exec_prompt_includes_retry_only_when_present() {
        assert!(!exec_prompt("t", "p", None).contains("repair contract"));
        let retry = exec_prompt("t", "p", Some("Your previous attempt did NOT pass."));
        assert!(retry.contains("[deep:EXEC]"));
        assert!(retry.contains("Your previous attempt did NOT pass."));
        assert!(retry.contains("Immediate mechanical edits"));
        assert!(retry.contains("exact receiver replacements"));
        assert!(retry.contains("Preserve call receivers during renames"));
    }

    #[test]
    fn exec_demands_reproduction_first_and_distrusts_a_prior_fix() {
        // The two disciplines that beat zo's repeated surface fixes, taught to
        // the implementer as rules (no keyword gate): write a failing-first
        // reproduction before fixing a bug, and treat a recent change to the same
        // code as suspect rather than ground truth.
        let exec = exec_prompt("the streaming stutter is still there", "plan", None);
        assert!(
            exec.to_lowercase().contains("reproduces"),
            "exec must require a failing-first reproduction for a bug fix"
        );
        assert!(
            exec.contains("SUSPECT"),
            "exec must distrust a recent prior fix to the same code"
        );
    }

    #[test]
    fn verify_rejects_a_bug_fix_without_a_reproduction_test() {
        // The verifier closes the hole the prior failed commit fell through: a
        // plausible bug-fix diff that passes the pre-existing (toothless) suite.
        let check = CheckObservation {
            green: true,
            output_tail: "42 passed".to_string(),
            ran_in: None,
        };
        let verify = verify_prompt(
            "fix the bug",
            "diff",
            Some(("cargo test", &check)),
            GreenCheckEvidence::default(),
            &[],
            &[],
            "implemented the fix",
            "",
            VerifyLensMode::Full,
            RouteTaskIntent::Other,
            false,
        );
        assert!(
            verify.contains("fails on the unfixed code"),
            "verifier must require a test that reproduces the bug (RED before, green after)"
        );
        assert!(
            verify.contains("ONLY where a failing-first reproduction is feasible"),
            "verifier must carve out genuinely-untestable fixes (heisenbug/TUI/config) \
             instead of hard-rejecting them and burning retries"
        );
    }

    #[test]
    fn retry_context_is_visible_to_retry_and_verify_prompts() {
        let repair = "Mandatory repair checklist:\n- Fix every issue above\n- Also handle the MCP ToolSearch path";
        let retry = reactive_retry_prompt("why did it stop?", repair);
        assert!(retry.contains("Current request context:"));
        assert!(retry.contains("why did it stop?"));
        assert_eq!(
            retry.matches("Also handle the MCP ToolSearch path").count(),
            1,
            "auto:RETRY should not duplicate the repair contract inside the task"
        );

        let task = task_with_retry_context("why did it stop?", Some(repair));
        let verify = verify_prompt(
            &task,
            "diff",
            None,
            GreenCheckEvidence::default(),
            &[],
            &[],
            "updated the implementation",
            "",
            VerifyLensMode::Full,
            RouteTaskIntent::Other,
            false,
        );
        assert!(verify.contains("Task:\nwhy did it stop?"));
        assert!(verify.contains("Latest repair/update context:"));
        assert!(verify.contains("Also handle the MCP ToolSearch path"));
    }

    #[test]
    fn verify_prompt_is_strict_json_and_states_objective() {
        let check = CheckObservation {
            green: false,
            output_tail: "CHECK_OUTPUT_TAIL_MARKER".to_string(),
            ran_in: Some(std::path::PathBuf::from("/work/checkout")),
        };
        let with = verify_prompt(
            "t",
            "diff",
            Some(("cargo test", &check)),
            GreenCheckEvidence::default(),
            &["src/changed.rs".to_string()],
            &[],
            "ASSISTANT_CLAIM_MARKER",
            "",
            VerifyLensMode::Full,
            RouteTaskIntent::Other,
            false,
        );
        assert!(with.contains("[deep:VERIFY]"));
        // The check line names the tree it ran in (recorded at run time), so
        // a PASS next to a sibling-worktree diff cannot read as validating
        // that other tree.
        assert!(with.contains("Objective check `cargo test` (ran in /work/checkout): FAIL"));
        assert!(with.contains("CHECK_OUTPUT_TAIL_MARKER"));
        assert!(with.contains("src/changed.rs"));
        assert!(with.contains("ASSISTANT_CLAIM_MARKER"));
        assert!(with.contains("scoped git diff"));
        assert!(with.contains(VERIFY_JSON_ACCEPT_EXAMPLE));
        assert!(with.contains(VERIFY_JSON_REJECT_EXAMPLE));
        assert!(
            with.contains("Do NOT spawn sub-agents, delegate, or call Agent, SpawnMultiAgent, Workflow, or SendMessage"),
            "VERIFY must imperatively require one inline verifier"
        );
        let without = verify_prompt(
            "t",
            "diff",
            None,
            GreenCheckEvidence::default(),
            &[],
            &[],
            "claim",
            "",
            VerifyLensMode::Full,
            RouteTaskIntent::Other,
            false,
        );
        assert!(without.contains("No objective check command was configured"));
    }

    #[test]
    fn verify_visual_evidence_prompt_is_strictly_conditional_and_additive() {
        for (lens_mode, intent) in [
            (VerifyLensMode::SpecOnly, RouteTaskIntent::Design),
            (VerifyLensMode::SpecOnly, RouteTaskIntent::Other),
            (VerifyLensMode::Full, RouteTaskIntent::Design),
        ] {
            let without = verify_prompt(
                "task",
                "diff",
                None,
                GreenCheckEvidence::default(),
                &[],
                &[],
                "claim",
                "",
                lens_mode,
                intent,
                false,
            );
            let with = verify_prompt(
                "task",
                "diff",
                None,
                GreenCheckEvidence::default(),
                &[],
                &[],
                "claim",
                "",
                lens_mode,
                intent,
                true,
            );

            assert!(!without.contains(VERIFY_VISUAL_EVIDENCE_BLOCK));
            assert_eq!(
                with.replacen(VERIFY_VISUAL_EVIDENCE_BLOCK, "", 1),
                without,
                "the no-image prompt must remain byte-identical"
            );
            assert!(with.contains("Inspect them yourself"));
            assert!(with.contains("visual defects are in scope"));
        }
    }

    #[test]
    fn verify_prompt_examples_are_strict_parseable_json() {
        // The examples are the per-lens contract; parse_lens_verifier folds them
        // under AnyReject. The all-true example accepts; the one-false (spec)
        // example rejects on that single lens objection.
        let accept = parse_lens_verifier(VERIFY_JSON_ACCEPT_EXAMPLE);
        assert_eq!(accept.parse, VerifierParse::Json);
        assert!(accept.accepted);

        let reject = parse_lens_verifier(VERIFY_JSON_REJECT_EXAMPLE);
        assert_eq!(reject.parse, VerifierParse::Json);
        assert!(!reject.accepted, "any single lens reject blocks acceptance");
        assert_eq!(reject.issues.len(), 1);
    }

    #[test]
    fn verify_prompt_uses_scalar_spec_contract_only_for_single_lens() {
        let single = verify_prompt(
            "task",
            "diff",
            None,
            GreenCheckEvidence::default(),
            &[],
            &[],
            "claim",
            "",
            VerifyLensMode::SpecOnly,
            RouteTaskIntent::Other,
            false,
        );
        assert!(single.contains("ONLY the spec/task-compliance dimension"));
        assert!(single.contains(VERIFY_SCALAR_ACCEPT_EXAMPLE));
        assert!(single.contains(VERIFY_SCALAR_REJECT_EXAMPLE));
        assert!(!single.contains(VERIFY_JSON_ACCEPT_EXAMPLE));
        assert!(!single.contains("\"regression\""));
        assert!(!single.contains("\"security\""));

        let scalar_accept = parse_lens_verifier(VERIFY_SCALAR_ACCEPT_EXAMPLE);
        assert_eq!(scalar_accept.parse, VerifierParse::Json);
        assert!(scalar_accept.accepted);
        let scalar_reject = parse_lens_verifier(VERIFY_SCALAR_REJECT_EXAMPLE);
        assert_eq!(scalar_reject.parse, VerifierParse::Json);
        assert!(!scalar_reject.accepted);

        let full = verify_prompt(
            "task",
            "diff",
            None,
            GreenCheckEvidence::default(),
            &[],
            &[],
            "claim",
            "",
            VerifyLensMode::Full,
            RouteTaskIntent::Other,
            false,
        );
        assert!(full.contains(VERIFY_JSON_ACCEPT_EXAMPLE));
        assert!(full.contains(VERIFY_JSON_REJECT_EXAMPLE));
        assert!(full.contains("\"regression\": does"));
    }

    /// Every VERIFY form for a fixed set of inputs, so the design assertions
    /// below compare exactly one variable.
    fn verify_prompt_for(lens_mode: VerifyLensMode, intent: RouteTaskIntent) -> String {
        let check = CheckObservation {
            green: false,
            output_tail: "TAIL".to_string(),
            ran_in: None,
        };
        verify_prompt(
            "TASK",
            "DIFF",
            Some(("cargo test", &check)),
            GreenCheckEvidence::default(),
            &["src/a.rs".to_string()],
            &[],
            "CLAIM",
            "",
            lens_mode,
            intent,
            false,
        )
    }

    /// Markers that must appear ONLY on a design turn's VERIFY instruction.
    /// Drawn from all three parts of the lens (its `spec` framing, the criteria
    /// body, and the single-lens framing) so a leak in any one of them fails.
    const DESIGN_LENS_MARKERS: &[&str] = &[
        "read as DESIGN work",
        "Token plan:",
        "AI defaults:",
        "Spacing rhythm:",
        "reduced motion respected",
        // The bench-measured Korean-web defects: both must reach the grader on a
        // design turn and neither may leak onto any other intent.
        "Small-viewport nav parity:",
        "Korean line-breaking discipline:",
    ];

    #[test]
    fn design_intent_floors_verify_depth_at_single_lens_without_forcing_full() {
        // The floor is exactly one rung: a design turn stops skipping, it does
        // not start paying for three lenses it did not earn.
        assert_eq!(
            intent_verify_floor(RouteTaskIntent::Design),
            VerifyDepth::SingleLens
        );
        assert_eq!(
            VerifyDepth::Skip.max(intent_verify_floor(RouteTaskIntent::Design)),
            VerifyDepth::SingleLens,
            "a design turn never skips verification outright"
        );
        assert_eq!(
            VerifyDepth::Full.max(intent_verify_floor(RouteTaskIntent::Design)),
            VerifyDepth::Full,
            "the floor must never downgrade a Full-depth design turn"
        );
        assert_eq!(
            VerifyDepth::SingleLens.max(intent_verify_floor(RouteTaskIntent::Design)),
            VerifyDepth::SingleLens
        );

        // Every other intent — including the one every unprobed path carries —
        // leaves proportional depth exactly where the band and the diff put it.
        for intent in [
            RouteTaskIntent::Implementation,
            RouteTaskIntent::Analysis,
            RouteTaskIntent::Other,
        ] {
            assert_eq!(intent_verify_floor(intent), VerifyDepth::Skip, "{intent:?}");
            for depth in [VerifyDepth::Skip, VerifyDepth::SingleLens, VerifyDepth::Full] {
                assert_eq!(depth.max(intent_verify_floor(intent)), depth, "{intent:?}");
            }
        }
    }

    #[test]
    fn design_turns_get_the_design_lens_at_both_depths() {
        // SingleLens: the one lens IS the design lens — same scalar verdict
        // contract, so severity reaches the retry loop through the existing
        // channel rather than a new one.
        let single = verify_prompt_for(VerifyLensMode::SpecOnly, RouteTaskIntent::Design);
        for marker in DESIGN_LENS_MARKERS {
            assert!(single.contains(marker), "single lens missing {marker:?}");
        }
        assert!(single.contains("ONLY the design/visual-quality dimension"));
        assert!(single.contains(VERIFY_SCALAR_ACCEPT_EXAMPLE));
        assert!(single.contains(VERIFY_SCALAR_REJECT_EXAMPLE));
        assert!(!single.contains("ONLY the spec/task-compliance dimension"));
        assert!(!single.contains(VERIFY_JSON_ACCEPT_EXAMPLE));

        // Full: the design criteria are appended to the existing three lenses
        // and fold into `spec`; no fourth verdict key is introduced.
        let full = verify_prompt_for(VerifyLensMode::Full, RouteTaskIntent::Design);
        for marker in DESIGN_LENS_MARKERS {
            assert!(full.contains(marker), "full lens missing {marker:?}");
        }
        assert!(full.contains("\"regression\": does"));
        assert!(full.contains("\"security\": does"));
        assert!(full.contains(VERIFY_JSON_ACCEPT_EXAMPLE));
        assert!(
            full.contains("A design defect below is a spec rejection"),
            "design severity must ride the existing spec lens, not a new key"
        );
        assert!(
            !full.contains("\"design\":"),
            "a fourth verdict key would break the strict-JSON contract"
        );

        // The shipped landmine: a VERIFY prompt that reads like a roster made
        // the verifier spawn sub-agents. The lens must stay criteria for the one
        // reviewer this sub-turn already has.
        for prompt in [&single, &full] {
            assert!(prompt.contains("You are ONE strict, adversarial verifier"));
            assert!(prompt.contains(
                "Do NOT spawn sub-agents, delegate, or call Agent, SpawnMultiAgent, Workflow, or SendMessage"
            ));
            for banned in ["panel", "convene", "assemble", "reviewers"] {
                assert!(
                    !prompt.to_lowercase().contains(banned),
                    "design lens must not read like a roster: {banned:?}"
                );
            }
        }
    }

    #[test]
    fn non_design_turns_never_see_the_design_lens() {
        // The pin: the lens cannot leak. Every non-design intent — the value
        // every unprobed path, every fail-open, and a kill-switched design turn
        // all carry — must produce the pre-design-axis instruction verbatim.
        for lens_mode in [VerifyLensMode::SpecOnly, VerifyLensMode::Full] {
            let baseline = verify_prompt_for(lens_mode, RouteTaskIntent::Other);
            for marker in DESIGN_LENS_MARKERS {
                assert!(
                    !baseline.contains(marker),
                    "{lens_mode:?} leaked {marker:?} onto a non-design turn"
                );
            }
            for intent in [RouteTaskIntent::Implementation, RouteTaskIntent::Analysis] {
                assert_eq!(
                    verify_prompt_for(lens_mode, intent),
                    baseline,
                    "{intent:?} must be byte-identical to the pre-intent prompt"
                );
            }
            assert_ne!(
                verify_prompt_for(lens_mode, RouteTaskIntent::Design),
                baseline
            );
        }
    }

    #[test]
    fn lens_verify_rejects_when_only_security_lens_objects() {
        // The whole change is rejected if ANY lens objects, even when spec and
        // regression accept — the multi-lens rigor BB1-lite adds over a single
        // holistic verdict.
        let verdict = parse_lens_verifier(
            r#"{"spec": true, "regression": true, "security": false, "issues": ["logs a secret"]}"#,
        );
        assert!(!verdict.accepted);
        assert_eq!(verdict.parse, VerifierParse::Json);
        assert_eq!(verdict.issues, vec!["logs a secret".to_string()]);
    }

    #[test]
    fn lens_verify_falls_back_to_single_verdict_contract() {
        // A model that ignores the per-lens rubric and returns the old
        // {accepted,issues} shape still resolves correctly via the fallback.
        let accept = parse_lens_verifier(r#"{"accepted": true, "issues": []}"#);
        assert!(accept.accepted);
        // An unusable response is a conservative non-accept, never a silent pass.
        assert!(!parse_lens_verifier("not json at all").accepted);
    }

    #[test]
    fn goal_facing_accept_requires_deep_accept_not_just_verifier_gate() {
        // Objective check is RED but the verifier JSON-accepted. The verifier gate
        // still accepts (that is the deep loop's own retry/stall policy), and the
        // deep decision is GiveUp at the attempt cap — so the goal-facing scalar
        // must NOT export accept. Otherwise a goal with no objective validators
        // could be marked Succeeded on an objective-red turn (silent false success).
        let verifier = VerifierVerdict {
            accepted: true,
            issues: Vec::new(),
            parse: VerifierParse::Json,
            evidence: None,
        };
        let folded = fold_verification_attempt(2, 2, false, &verifier, &[]);

        assert!(folded.gate_accepted);
        assert_eq!(folded.decision, DeepDecision::GiveUp);
        assert!(!goal_facing_accept(&folded));
    }

    #[test]
    fn edited_file_paths_extracts_successful_write_targets() {
        let summary = TurnSummary {
            assistant_messages: Vec::new(),
            tool_results: vec![
                ConversationMessage::tool_result(
                    "edit-1",
                    "edit_file",
                    r#"{"filePath":"crates/runtime/src/lib.rs"}"#,
                    false,
                ),
                ConversationMessage::tool_result(
                    "write-1",
                    "write_file",
                    r#"{"path":"crates/runtime/src/new.rs"}"#,
                    false,
                ),
                ConversationMessage::tool_result(
                    "read-ignored",
                    "read_file",
                    r#"{"filePath":"README.md"}"#,
                    false,
                ),
                ConversationMessage::tool_result(
                    "failed-ignored",
                    "edit_file",
                    r#"{"filePath":"crates/runtime/src/failed.rs"}"#,
                    true,
                ),
            ],
            prompt_cache_events: Vec::new(),
            iterations: 1,
            usage: TokenUsage::default(),
            turn_output_tokens: 0,
            auto_compaction: None,
            microcompact: None,
            deep_verification: None,
            verification_issues: Vec::new(),
            deep_verifier_parse: None,
            deep_verifier_model: None,
            budget_exhausted: None,
        };

        assert_eq!(
            edited_file_paths(&summary),
            vec![
                "crates/runtime/src/lib.rs".to_string(),
                "crates/runtime/src/new.rs".to_string()
            ]
        );
    }

    /// 첨부 경로 합집합: diff 경로가 예산 우선권을 갖고, EXEC가 읽은(성공한
    /// `read_file`) 파일이 뒤따르며, 같은 파일의 두 표기는 한 번만 실린다.
    #[test]
    fn verify_attachment_paths_unions_diff_and_consulted_reads() {
        let dir = std::env::temp_dir().join(format!(
            "zo-attach-union-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join("changed.rs"), "x").expect("seed");
        std::fs::write(dir.join("DECISIONS.md"), "d").expect("seed");

        let read_use = |id: &str, path: &str| ContentBlock::ToolUse {
            id: id.to_string(),
            name: "read_file".to_string(),
            input: serde_json::json!({ "path": path }).to_string(),
        };
        let summary = TurnSummary {
            assistant_messages: vec![ConversationMessage::assistant(vec![
                read_use("r1", "DECISIONS.md"),
                // diff 경로와 같은 파일의 다른 표기 — 중복 첨부 금지.
                read_use("r2", "./changed.rs"),
                read_use("r3", "failed.txt"),
            ])],
            tool_results: vec![
                ConversationMessage::tool_result("r1", "read_file", "content", false),
                ConversationMessage::tool_result("r2", "read_file", "content", false),
                ConversationMessage::tool_result("r3", "read_file", "denied", true),
            ],
            prompt_cache_events: Vec::new(),
            iterations: 1,
            usage: TokenUsage::default(),
            turn_output_tokens: 0,
            auto_compaction: None,
            microcompact: None,
            deep_verification: None,
            verification_issues: Vec::new(),
            deep_verifier_parse: None,
            deep_verifier_model: None,
            budget_exhausted: None,
        };
        let consulted = exec_consulted_paths(&summary);
        let union = verify_attachment_paths(
            &["changed.rs".to_string()],
            &consulted,
            Some(&dir),
        );
        assert_eq!(union[0], "changed.rs", "diff paths own the byte budget");
        assert!(union.contains(&"DECISIONS.md".to_string()), "consulted read attaches");
        assert_eq!(
            union.iter().filter(|p| p.contains("changed")).count(),
            1,
            "two spellings of one file attach once"
        );
        assert!(
            !union.iter().any(|p| p.contains("failed")),
            "errored reads must not attach"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 첨부 빌더 계약: 작은 UTF-8 파일은 전문 첨부·초과/비UTF-8은 스킵
    /// 노트(`read_file` 안내)·빈 `diff_paths`는 빈 문자열(프롬프트 바이트 불변).
    #[test]
    fn verify_file_attachments_caps_and_skip_notes() {
        assert_eq!(verify_file_attachments(&[], None), "");

        let dir = std::env::temp_dir().join(format!(
            "zo-verify-attach-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join("small.py"), "def f():\n    return 1\n").expect("seed");
        std::fs::write(
            dir.join("big.py"),
            "x".repeat(VERIFY_FILE_ATTACH_PER_FILE_BYTES + 1),
        )
        .expect("seed");
        std::fs::write(dir.join("bin.dat"), [0xFF, 0xFE, 0x00, 0x9F]).expect("seed");

        let section = verify_file_attachments(
            &[
                "small.py".to_string(),
                "big.py".to_string(),
                "bin.dat".to_string(),
                "gone.py".to_string(),
            ],
            Some(&dir),
        );
        assert!(section.contains("FILE: small.py (attached in full"));
        assert!(section.contains("def f():"));
        assert!(section.contains("FILE: big.py (not attached: too large"));
        assert!(!section.contains(&"x".repeat(64)), "oversized content must not leak");
        assert!(section.contains("FILE: bin.dat (not attached: binary or non-UTF-8"));
        assert!(section.contains("FILE: gone.py (not attached: unreadable or deleted"));
        assert!(section.contains("deliver your verdict JSON in THIS response"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 첨부는 `SingleLens` 전용: `Full` 렌즈 프롬프트는 첨부를 건네도 바이트
    /// 불변이고, `SpecOnly`는 첨부가 diff 뒤에 그대로 실린다. 빈 첨부의
    /// `SpecOnly`도 종전과 바이트 동일(무발화 시 완전 무변화 핀).
    #[test]
    fn verify_prompt_attaches_files_only_on_single_lens() {
        let build = |attached: &str, lens: VerifyLensMode| {
            verify_prompt(
                "task",
                "diff",
                None,
                GreenCheckEvidence::default(),
                &[],
                &[],
                "claim",
                attached,
                lens,
                RouteTaskIntent::Other,
                false,
            )
        };
        let attachment = "\n\nATTACHED-SENTINEL";
        let spec_with = build(attachment, VerifyLensMode::SpecOnly);
        let spec_without = build("", VerifyLensMode::SpecOnly);
        assert!(spec_with.contains("ATTACHED-SENTINEL"));
        assert_eq!(spec_with.replacen(attachment, "", 1), spec_without);

        let full_with = build(attachment, VerifyLensMode::Full);
        let full_without = build("", VerifyLensMode::Full);
        assert!(!full_with.contains("ATTACHED-SENTINEL"));
        assert_eq!(full_with, full_without);
    }

    #[test]
    fn exec_green_checks_reports_only_post_edit_foreground_zero_exits() {
        let bash_use = |id: &str, command: &str| {
            ContentBlock::ToolUse {
                id: id.to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({ "command": command }).to_string(),
            }
        };
        let summary = TurnSummary {
            assistant_messages: vec![ConversationMessage::assistant(vec![
                bash_use("pre-edit", "cargo build --tests"),
                bash_use("green", "cargo build 2>&1 | tail -5 && cargo run --quiet"),
                bash_use("red", "cargo test broken"),
                bash_use("bg", "cargo test --workspace"),
                bash_use("not-a-check", "ls -la"),
            ])],
            tool_results: vec![
                // Green check BEFORE the edit: stale evidence, must be dropped.
                ConversationMessage::tool_result(
                    "pre-edit",
                    "bash",
                    r#"{"stdout":"ok","stderr":""}"#,
                    false,
                ),
                ConversationMessage::tool_result(
                    "edit-1",
                    "edit_file",
                    r#"{"filePath":"src/main.rs"}"#,
                    false,
                ),
                // Post-edit green check: the evidence this axis exists for.
                ConversationMessage::tool_result(
                    "green",
                    "bash",
                    r#"{"stdout":"15\n","stderr":""}"#,
                    false,
                ),
                // Non-zero exit is not green.
                ConversationMessage::tool_result(
                    "red",
                    "bash",
                    r#"{"stdout":"","stderr":"boom","returnCodeInterpretation":"exit_code:101"}"#,
                    false,
                ),
                // A backgrounded start proves nothing about the outcome.
                ConversationMessage::tool_result(
                    "bg",
                    "bash",
                    r#"{"stdout":"Started background task","stderr":"","backgroundTaskId":"t1"}"#,
                    false,
                ),
                // Green but not check-shaped: noise, not evidence.
                ConversationMessage::tool_result(
                    "not-a-check",
                    "bash",
                    r#"{"stdout":"total 8","stderr":""}"#,
                    false,
                ),
            ],
            prompt_cache_events: Vec::new(),
            iterations: 1,
            usage: TokenUsage::default(),
            turn_output_tokens: 0,
            auto_compaction: None,
            microcompact: None,
            deep_verification: None,
            verification_issues: Vec::new(),
            deep_verifier_parse: None,
            deep_verifier_model: None,
            budget_exhausted: None,
        };

        assert_eq!(
            exec_green_checks(&summary),
            vec!["cargo build 2>&1 | tail -5 && cargo run --quiet".to_string()]
        );
    }

    #[test]
    fn verify_prompt_reports_exec_observed_checks_and_forbids_reruns() {
        let checks = vec!["cargo build --tests && cargo run --quiet".to_string()];
        let with = verify_prompt(
            "fix the crate",
            "diff",
            None,
            GreenCheckEvidence { this_turn: &checks, session: &[] },
            &[],
            &[],
            "claim",
            "",
            VerifyLensMode::Full,
            RouteTaskIntent::Other,
            false,
        );
        assert!(
            with.contains("cargo build --tests && cargo run --quiet"),
            "the observed command must be quoted verbatim"
        );
        assert!(
            with.contains("observed by the harness to exit 0"),
            "the prompt must attribute the observation to the harness, not the model"
        );
        assert!(
            with.contains("Do NOT re-run build/test commands yourself"),
            "the prompt must forbid the denied-anyway re-run attempt"
        );
        // Without observations the prompt stays byte-identical to the old form.
        let without = verify_prompt(
            "fix the crate",
            "diff",
            None,
            GreenCheckEvidence::default(),
            &[],
            &[],
            "claim",
            "",
            VerifyLensMode::Full,
            RouteTaskIntent::Other,
            false,
        );
        assert!(!without.contains("observed by the harness"));
    }

    /// Every VERIFY prompt for one fixed input except the check evidence, so the
    /// assertions below compare exactly one variable.
    fn verify_prompt_with_checks(checks: GreenCheckEvidence<'_>) -> String {
        verify_prompt(
            "fix the crate",
            "diff",
            None,
            checks,
            &[],
            &[],
            "claim",
            "",
            VerifyLensMode::Full,
            RouteTaskIntent::Other,
            false,
        )
    }

    /// Session-ledger checks join the SAME objective section, after this
    /// attempt's own and labelled by provenance — and an empty session set
    /// leaves the prompt byte-identical to the pre-join form.
    #[test]
    fn verify_prompt_joins_session_checks_after_this_attempts_own() {
        let this_turn = vec!["cargo test -p runtime".to_string()];
        let session = vec!["cargo clippy -p runtime".to_string()];

        let exec_only = verify_prompt_with_checks(GreenCheckEvidence {
            this_turn: &this_turn,
            session: &[],
        });
        let both = verify_prompt_with_checks(GreenCheckEvidence {
            this_turn: &this_turn,
            session: &session,
        });

        assert!(both.contains("cargo clippy -p runtime"), "{both}");
        assert!(
            both.contains("still green — the harness observed each exit 0 and has recorded no \
                           edit since"),
            "the session lines must carry their own provenance: {both}"
        );
        assert!(
            both.contains("not coverage of this diff"),
            "the leg must be told these predate the attempt: {both}"
        );
        // Ordering: this attempt's evidence is what the verifier weighs first.
        assert!(
            both.find("AFTER its last edit").expect("exec heading")
                < both.find("earlier turns in this session").expect("session heading"),
            "this attempt's observations must come first: {both}"
        );
        // Additive only: the exec-only prompt is a strict prefix-preserving
        // subset, so a leg with nothing to join is byte-identical to before.
        assert_eq!(
            verify_prompt_with_checks(GreenCheckEvidence {
                this_turn: &this_turn,
                session: &[],
            }),
            exec_only
        );
        assert!(
            !exec_only.contains("earlier turns in this session"),
            "an empty session set must add nothing at all: {exec_only}"
        );
    }

    /// With no observation of its own, a leg carrying only session checks still
    /// gets the read-only warning — otherwise the one prompt shape that has
    /// evidence but no re-run ban is the one most likely to attempt a build.
    #[test]
    fn a_session_only_prompt_still_forbids_reruns() {
        let session = vec!["cargo test -p runtime".to_string()];
        let session_only = verify_prompt_with_checks(GreenCheckEvidence {
            this_turn: &[],
            session: &session,
        });
        assert!(
            session_only.contains("Do NOT re-run build/test commands yourself"),
            "{session_only}"
        );
        // And it is stated exactly once when BOTH sources are present.
        let both = verify_prompt_with_checks(GreenCheckEvidence {
            this_turn: &["cargo build".to_string()],
            session: &session,
        });
        assert_eq!(
            both.matches("Do NOT re-run build/test commands yourself").count(),
            1,
            "{both}"
        );
    }

    /// The whole objective section stays byte-identical to the pre-feature form
    /// when neither source observed anything — the neutrality pin.
    #[test]
    fn no_observations_leaves_the_verify_prompt_byte_identical() {
        let none = verify_prompt_with_checks(GreenCheckEvidence::default());
        assert!(!none.contains("observed by the harness"));
        assert!(!none.contains("earlier turns in this session"));
        assert!(!none.contains("Do NOT re-run build/test commands yourself"));
        assert!(none.contains("No objective check command was configured for this turn."));
    }

    #[test]
    fn deep_summary_fold_last_wins_cumulative_but_sums_turn_output() {
        // `usage` is the *cumulative* session usage, so a folded deep turn's usage
        // must be the LATEST snapshot, never the sum — summing multiplied the total
        // by the sub-turn count and inflated the goal budget / tripped compaction.
        // `turn_output_tokens` is each sub-turn's OWN delta, so it DOES sum (the
        // goal budget charges the whole multi-sub-turn deep turn). Iterations sum.
        let sub_turn = |iterations: usize, cumulative_output: u32, turn_delta: u32| TurnSummary {
            assistant_messages: Vec::new(),
            tool_results: Vec::new(),
            prompt_cache_events: Vec::new(),
            iterations,
            usage: TokenUsage {
                output_tokens: cumulative_output,
                ..Default::default()
            },
            turn_output_tokens: turn_delta,
            auto_compaction: None,
            microcompact: None,
            deep_verification: None,
            verification_issues: Vec::new(),
            deep_verifier_parse: None,
            deep_verifier_model: None,
            budget_exhausted: None,
        };
        let mut acc = DeepSummaryAcc::default();
        acc.fold(sub_turn(2, 100, 100)); // cumulative 100, this leg produced 100
        acc.fold(sub_turn(3, 250, 150)); // cumulative 250, this leg produced 150
        let summary = acc.into_summary();
        assert_eq!(
            summary.usage.output_tokens, 250,
            "usage is the latest cumulative snapshot, not 100 + 250"
        );
        assert_eq!(
            summary.turn_output_tokens, 250,
            "turn_output is the SUM of per-leg deltas (100 + 150)"
        );
        assert_eq!(
            summary.iterations, 5,
            "iterations accumulate across sub-turns"
        );
    }

    #[test]
    fn deep_summary_fold_preserves_a_sub_turn_budget_stop() {
        // A budget stop in ANY leg must survive into the composed summary:
        // dropping it silently disarmed the `/loop` budget-pause and the
        // grind-escalation streak whenever the deep gate wrapped the turn.
        let sub_turn = |budget_exhausted: Option<BudgetExhausted>| TurnSummary {
            assistant_messages: Vec::new(),
            tool_results: Vec::new(),
            prompt_cache_events: Vec::new(),
            iterations: 1,
            usage: TokenUsage::default(),
            turn_output_tokens: 0,
            auto_compaction: None,
            microcompact: None,
            deep_verification: None,
            verification_issues: Vec::new(),
            deep_verifier_parse: None,
            deep_verifier_model: None,
            budget_exhausted,
        };
        let mut acc = DeepSummaryAcc::default();
        acc.fold(sub_turn(None));
        acc.fold(sub_turn(Some(BudgetExhausted::Deadline)));
        // A later clean leg must not erase the earlier stop.
        acc.fold(sub_turn(None));
        assert_eq!(
            acc.into_summary().budget_exhausted,
            Some(BudgetExhausted::Deadline),
            "a sub-turn budget stop must survive the deep-turn fold"
        );
    }

    #[test]
    fn attempt_diff_paths_excludes_preexisting_unedited_dirty_files() {
        let baseline = vec!["crates/api/src/client.rs".to_string()];
        let after = vec![
            "crates/api/src/client.rs".to_string(),
            "crates/runtime/src/conversation/deep_gate.rs".to_string(),
        ];
        let edited =
            vec!["crates/zo-cli/src/session/slash_dispatch/helpers_tui.rs".to_string()];

        assert_eq!(
            attempt_diff_paths(&baseline, &after, &edited, None),
            vec![
                "crates/runtime/src/conversation/deep_gate.rs".to_string(),
                "crates/zo-cli/src/session/slash_dispatch/helpers_tui.rs".to_string(),
            ]
        );
    }

    #[test]
    fn attempt_diff_paths_keeps_preexisting_file_when_attempt_edited_it() {
        let baseline = vec!["crates/runtime/src/conversation/deep_gate.rs".to_string()];
        let after = baseline.clone();
        let edited = vec!["crates/runtime/src/conversation/deep_gate.rs".to_string()];

        assert_eq!(attempt_diff_paths(&baseline, &after, &edited, None), edited);
    }

    /// **철자 불일치 고무도장 회귀 핀** — 편집 도구는 절대경로를, baseline
    /// 스냅샷은 리포루트 상대경로를 말한다. 문자열 비교는 그 둘을 영원히
    /// 다른 파일로 봐서 (a) 같은 파일이 diff 경로에 두 번 들어가고 (b) 시도가
    /// 편집한 바로 그 파일이 "선재 dirty — 판정하지 말라" 목록에 실렸다.
    #[test]
    fn spelling_mismatch_never_double_lists_nor_rubber_stamps_the_attempts_own_edit() {
        let own_root = std::path::PathBuf::from("/repo");
        let baseline = vec!["crates/runtime/src/foo.rs".to_string()];
        let after = baseline.clone();
        let edited = vec!["/repo/crates/runtime/src/foo.rs".to_string()];

        let diff_paths =
            attempt_diff_paths(&baseline, &after, &edited, Some(own_root.as_path()));
        assert_eq!(
            diff_paths, edited,
            "the same file in two spellings must resolve to ONE diff path"
        );
        assert!(
            preexisting_dirty_paths(&baseline, &diff_paths, Some(own_root.as_path()))
                .is_empty(),
            "the attempt's own edit must never reach the do-not-judge list"
        );
    }

    /// The dedupe is key-based, not order-based: a baseline-clean file that
    /// changed without an edit-tool report (build output, formatter) still
    /// joins under its snapshot spelling.
    #[test]
    fn attempt_diff_paths_still_admits_unreported_changes_under_either_spelling() {
        let own_root = std::path::PathBuf::from("/repo");
        let baseline: Vec<String> = Vec::new();
        let after = vec!["generated/schema.rs".to_string()];
        let edited = vec!["/repo/src/lib.rs".to_string()];

        assert_eq!(
            attempt_diff_paths(&baseline, &after, &edited, Some(own_root.as_path())),
            vec!["/repo/src/lib.rs".to_string(), "generated/schema.rs".to_string()],
        );
    }

    /// **선재 dirty 오귀속 회귀 핀** — 편집 경로가 비었을 때 전체 워킹트리
    /// diff로 폴백하면, 시도 전부터 dirty였던 남의 파일들(다른 워크트리
    /// 작업 잔재)이 통째로 "이 시도의 diff"로 verifier에 들어간다. 실제로
    /// 계획-작성 턴이 "운영 파일 4개를 수정했다"며 거부당했다.
    #[test]
    fn empty_attempt_paths_never_fall_back_to_the_full_tree_diff() {
        let diff = scoped_git_diff(&[], None);
        assert!(
            diff.contains("no file edits") && !diff.contains("diff --git"),
            "an empty attempt must yield an explicit note, not the tree's dirt: {diff:?}"
        );
    }

    /// A throwaway git repository OUTSIDE this process's cwd, with one tracked
    /// file. Staged (not committed) so the fixture needs no user identity.
    fn sibling_repo_with_tracked_file() -> (tempfile::TempDir, std::path::PathBuf) {
        let repo = tempfile::TempDir::new().expect("mkdir sibling repo");
        let run = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .args(args)
                .current_dir(repo.path())
                .output()
                .expect("run git in fixture repo");
            assert!(output.status.success(), "git {args:?} failed in fixture");
        };
        run(&["init", "-q"]);
        let file = repo.path().join("lib.rs");
        std::fs::write(&file, "fn old() {}\n").expect("seed file");
        run(&["add", "lib.rs"]);
        (repo, file)
    }

    /// **형제 워크트리 오귀속 회귀 핀** — git이 zo의 cwd에서 돌던 시절, 다른
    /// 리포/워크트리의 절대경로 편집은 `git diff`엔 리포 밖이라 빈 결과,
    /// `ls-files --error-unmatch`엔 fatal이라 "untracked"로 오판돼 실제
    /// 수정이 통째 신규 파일 추가로 렌더됐다. 경로 소유 리포에서 실행하면
    /// 진짜 수정 diff가 나와야 한다.
    #[test]
    fn sibling_repo_edit_yields_a_real_modification_diff() {
        let (repo, file) = sibling_repo_with_tracked_file();
        std::fs::write(&file, "fn new() {}\n").expect("modify file");

        let diff = scoped_git_diff(&[file.to_string_lossy().into_owned()], None);
        assert!(
            diff.contains("diff --git") && diff.contains("-fn old") && diff.contains("+fn new"),
            "a sibling-repo edit must surface as a modification diff: {diff:?}"
        );
        assert!(
            !diff.contains("/dev/null"),
            "a tracked modification must not be misrendered as a brand-new file: {diff:?}"
        );
        assert!(
            diff.contains("(attempt paths in repository:"),
            "an out-of-cwd repository must be named for the verifier: {diff:?}"
        );
        drop(repo);
    }

    /// An untracked file in a sibling repository still renders as a new-file
    /// diff — now because that repository's own `ls-files` says untracked,
    /// not because the probe ran in the wrong tree and errored.
    #[test]
    fn sibling_repo_untracked_file_renders_as_new_file_diff() {
        let (repo, _) = sibling_repo_with_tracked_file();
        let fresh = repo.path().join("fresh.rs");
        std::fs::write(&fresh, "fn fresh() {}\n").expect("untracked file");

        let diff = scoped_git_diff(&[fresh.to_string_lossy().into_owned()], None);
        assert!(
            diff.contains("/dev/null") && diff.contains("+fn fresh"),
            "an untracked sibling-repo file must render as a new-file diff: {diff:?}"
        );
        drop(repo);
    }

    /// A path in no repository at all keeps the pre-existing no-index
    /// rendering (the `None` group preserves the old behavior).
    #[test]
    fn path_outside_any_repository_still_renders_no_index_diff() {
        let outside = tempfile::TempDir::new().expect("mkdir plain dir");
        let file = outside.path().join("note.txt");
        std::fs::write(&file, "plain contents\n").expect("plain file");

        let diff = scoped_git_diff(&[file.to_string_lossy().into_owned()], None);
        assert!(
            diff.contains("/dev/null") && diff.contains("+plain contents"),
            "a repo-less path must keep the no-index rendering: {diff:?}"
        );
    }

    /// **혼합 그룹 오귀속 회귀 핀 (F1)** — 라벨을 외부 그룹에만 붙이면
    /// 정렬상 외부 그룹이 앞설 때 own 리포의 hunk들이 외부 라벨 아래 그대로
    /// 이어져, 검증자에게 "같은 파일이 모순된 내용으로 두 번"으로 읽힌다.
    /// diff가 여러 리포에 걸치면 모든 그룹에 라벨이 붙어야 한다.
    #[test]
    fn mixed_repo_diff_labels_every_group_including_own() {
        let (own, own_file) = sibling_repo_with_tracked_file();
        let (foreign, foreign_file) = sibling_repo_with_tracked_file();
        std::fs::write(&own_file, "fn own_edit() {}\n").expect("edit own");
        std::fs::write(&foreign_file, "fn foreign_edit() {}\n").expect("edit foreign");

        let diff = scoped_git_diff(
            &[
                own_file.to_string_lossy().into_owned(),
                foreign_file.to_string_lossy().into_owned(),
            ],
            repo_root_for(own.path()).as_deref(),
        );
        let own_label = format!(
            "(attempt paths in repository: {})",
            repo_root_for(own.path()).expect("own root").display()
        );
        let foreign_label = format!(
            "(attempt paths in repository: {})",
            repo_root_for(foreign.path()).expect("foreign root").display()
        );
        assert!(
            diff.contains(&own_label) && diff.contains(&foreign_label),
            "a multi-repo diff must label EVERY group, own included: {diff:?}"
        );
        let own_edit = diff.find("+fn own_edit").expect("own hunk");
        let own_label_at = diff.find(&own_label).expect("own label");
        let foreign_label_at = diff.find(&foreign_label).expect("foreign label");
        let own_section_end = if foreign_label_at > own_label_at {
            foreign_label_at
        } else {
            diff.len()
        };
        assert!(
            own_edit > own_label_at && own_edit < own_section_end,
            "the own repo's hunks must sit under the own repo's label: {diff:?}"
        );
    }

    /// **빈 외부 그룹 회귀 핀 (F2)** — 라벨을 본문 확인 전에 쓰면, 편집이
    /// 이미 커밋돼 diff가 빈 그룹이 라벨 한 줄만 남기고 "(no git diff …)"
    /// 폴백까지 삼킨다. 검증자가 받는 diff 필드 전체가 괄호 한 줄이었다.
    #[test]
    fn clean_sibling_group_still_yields_the_no_diff_fallback() {
        let (repo, file) = sibling_repo_with_tracked_file();
        // Staged and unmodified: the group renders nothing.
        let diff = scoped_git_diff(&[file.to_string_lossy().into_owned()], None);
        assert!(
            diff.contains("(no git diff for scoped attempt paths:"),
            "an all-empty rendering must keep the explicit fallback note: {diff:?}"
        );
        assert!(
            !diff.contains("(attempt paths in repository:"),
            "an empty group must not leave a dangling repository label: {diff:?}"
        );
        drop(repo);
    }

    /// **중첩 워크트리 회귀 핀 (F4)** — `EnterWorktree`는 상대 경로를 cwd에
    /// join하므로 격리 워크트리가 리포 *안*에 생기는 것이 일급 흐름이다.
    /// `own_root` 프리픽스 단락은 그 워크트리 파일을 바깥 리포 그룹으로
    /// 오분류해, 바깥 리포의 git이 "untracked"라 답하고 실수정이 신규 파일
    /// 추가로 왜곡됐다 — 이 변경이 없애려던 바로 그 형태. 그룹은 오직
    /// 프로브(rev-parse)의 답으로만 정해져야 한다.
    #[test]
    fn nested_worktree_edit_is_grouped_by_its_own_tree_not_the_outer_repo() {
        let (outer, _outer_file) = sibling_repo_with_tracked_file();
        let run = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .args(args)
                .current_dir(outer.path())
                .output()
                .expect("run git in outer repo");
            assert!(
                output.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        // A worktree needs a commit; inline identity keeps the fixture
        // machine-independent.
        run(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "seed",
        ]);
        run(&["worktree", "add", "-q", "wt-x"]);
        let worktree_file = outer.path().join("wt-x").join("lib.rs");
        assert!(worktree_file.is_file(), "worktree checkout must exist");
        std::fs::write(&worktree_file, "fn worktree_edit() {}\n").expect("edit in worktree");

        // Production condition: own_root and the edited path are both
        // physical spellings (rev-parse / canonicalize), so a prefix
        // shortcut WOULD fire here if one existed.
        let own_root = repo_root_for(outer.path()).expect("outer root");
        let edited = worktree_file
            .canonicalize()
            .expect("canonicalize edit")
            .to_string_lossy()
            .into_owned();
        let diff = scoped_git_diff(&[edited], Some(own_root.as_path()));
        assert!(
            diff.contains("-fn old") && diff.contains("+fn worktree_edit"),
            "a nested-worktree edit must surface as a modification diff: {diff:?}"
        );
        assert!(
            !diff.contains("/dev/null"),
            "a tracked worktree file must not be misrendered as brand-new: {diff:?}"
        );
        assert!(
            diff.contains("(attempt paths in repository:") && diff.contains("wt-x"),
            "the diff must be attributed to the worktree, not the outer repo: {diff:?}"
        );
        // Cleanup so the outer TempDir can drop cleanly on every platform.
        run(&["worktree", "remove", "--force", "wt-x"]);
    }

    /// `..` components arrive verbatim from MCP/plugin edit tools (only the
    /// built-in tools canonicalize), and an unresolved `..` keyed the same
    /// file two ways — putting the attempt's own edit back on the
    /// do-not-judge list.
    #[test]
    fn attempt_path_key_resolves_parent_components() {
        let own_root = std::path::PathBuf::from("/repo");
        let baseline = vec!["crates/runtime/src/foo.rs".to_string()];
        let diff_paths = vec!["/repo/crates/x/../runtime/src/foo.rs".to_string()];
        assert!(
            preexisting_dirty_paths(&baseline, &diff_paths, Some(own_root.as_path()))
                .is_empty(),
            "a `..` spelling of the attempt's own edit must still subtract"
        );
    }

    #[test]
    fn preexisting_dirty_paths_subtracts_the_attempts_own_files() {
        let baseline = vec![
            "Business/Card/CardBusiness.cs".to_string(),
            "Common/Acme/AcmeApi.cs".to_string(),
            "src/touched_by_attempt.rs".to_string(),
        ];
        let diff_paths = vec!["src/touched_by_attempt.rs".to_string()];
        assert_eq!(
            preexisting_dirty_paths(&baseline, &diff_paths, None),
            vec![
                "Business/Card/CardBusiness.cs".to_string(),
                "Common/Acme/AcmeApi.cs".to_string(),
            ]
        );
        assert!(preexisting_dirty_paths(&[], &diff_paths, None).is_empty());
    }

    /// The marker VALUES are transcript compatibility, not free strings:
    /// saved sessions carry them, and the resume-time visibility filter and
    /// the deep-leg tool gate match them back. Pinned as literals on purpose
    /// — a constant-referencing assert would silently follow a rename and
    /// orphan every existing transcript.
    #[test]
    fn leg_marker_values_are_pinned_for_saved_transcripts() {
        assert_eq!(DEEP_PLAN_MARKER, "[deep:PLAN]");
        assert_eq!(DEEP_EXEC_MARKER, "[deep:EXEC]");
        assert_eq!(DEEP_VERIFY_MARKER, "[deep:VERIFY]");
        assert_eq!(AUTO_RETRY_MARKER, "[auto:RETRY]");
    }

    /// 실측 회귀 핀(2026-08-02, /Users/dev/mobile 세션): 크로스모델 verifier가
    /// verdict 대신 "👤: [deep:VERIFY] …" 지시문 전문을 에코했는데, 프롬프트
    /// 꼬리의 accept 예시 JSON을 `last_complete_lens`가 verdict로 읽어
    /// "strict verifier accepted" 고무도장이 찍혔다. 에코(자기 마커 포함)는
    /// 판정 불능 — 보수적 non-accept여야 한다.
    #[test]
    fn instruction_echo_never_parses_as_an_accept() {
        // The echo the live verifier produced: the leg's own prompt, verbatim
        // — built by the same prompt fn so the embedded accept example stays
        // byte-identical to whatever the contract evolves into.
        let echoed_prompt = verify_prompt(
            "다른 추가 개선 가능 한부분이있는지 분석만 해줘",
            "(this attempt reported no file edits)",
            None,
            GreenCheckEvidence::default(),
            &[],
            &["Business/Card/CardBusiness.cs".to_string()],
            "분석 완료 — 파일 변경 없음",
            "",
            VerifyLensMode::Full,
            RouteTaskIntent::Other,
            false,
        );
        let echo = format!("👤: {echoed_prompt}");
        let verdict = parse_verify_leg_text(&echo);
        assert!(
            !verdict.accepted,
            "an instruction echo must never rubber-stamp the change"
        );
        assert_eq!(verdict.parse, VerifierParse::Unparseable);
        assert!(
            verdict.issues.iter().any(|issue| issue.contains("echoed")),
            "the retry loop must see WHY the leg produced no verdict: {:?}",
            verdict.issues
        );
        // Control: the guard is echo-specific — a genuine single-line verdict
        // (which can never contain the leg marker) still parses as before.
        let genuine = parse_verify_leg_text(
            r#"{"spec": true, "regression": true, "security": true, "issues": [], "evidence": "checked"}"#,
        );
        assert!(genuine.accepted);
        assert_eq!(genuine.parse, VerifierParse::Json);
    }

    #[test]
    fn verify_prompt_marks_preexisting_dirt_as_not_this_attempts_work() {
        let with_dirt = verify_prompt(
            "write a fix plan",
            "(this attempt reported no file edits)",
            None,
            GreenCheckEvidence::default(),
            &[],
            &["Business/Card/CardBusiness.cs".to_string()],
            "wrote the plan",
            "",
            VerifyLensMode::Full,
            RouteTaskIntent::Other,
            false,
        );
        assert!(
            with_dirt.contains("NOT made by this attempt")
                && with_dirt.contains("- Business/Card/CardBusiness.cs"),
            "the verifier must be told which dirt predates the attempt: {with_dirt:?}"
        );

        let clean = verify_prompt(
            "write a fix plan",
            "diff",
            None,
            GreenCheckEvidence::default(),
            &[],
            &[],
            "wrote the plan",
            "",
            VerifyLensMode::Full,
            RouteTaskIntent::Other,
            false,
        );
        assert!(
            !clean.contains("NOT made by this attempt"),
            "a clean tree adds no block, keeping the prompt byte-stable"
        );
    }

    #[test]
    fn verification_outcome_note_summarizes_without_wire_parse_tokens() {
        let verifier = VerifierVerdict {
            accepted: false,
            issues: vec!["leaked settings file".into()],
            parse: VerifierParse::Json,
            evidence: None,
        };
        let note = verification_outcome_note("auto", DeepDecision::GiveUp, 2, 2, true, &verifier);

        assert_eq!(
            note,
            "auto: stopped — out of attempts (objective ok; strict verifier found 1 issue)"
        );
        assert!(!note.contains("verifier json"));
        assert!(!note.contains("accepted\":"));
    }

    #[test]
    fn verification_outcome_note_handles_retry_and_missing_verdict() {
        let verifier = VerifierVerdict {
            accepted: false,
            issues: Vec::new(),
            parse: VerifierParse::Unparseable,
            evidence: None,
        };
        let note = verification_outcome_note("deep", DeepDecision::Retry, 1, 3, false, &verifier);

        assert_eq!(
            note,
            "deep: retrying — verifier returned no usable verdict (objective red; attempt 1/3)"
        );
    }

    #[test]
    fn failure_summary_is_bounded_and_lists_issues() {
        let verifier = VerifierVerdict {
            accepted: false,
            issues: vec!["off-by-one".into(), "missing null check".into()],
            parse: decision_core::deep_lane::VerifierParse::Json,
            evidence: None,
        };
        let summary = failure_summary(false, &verifier);
        assert!(summary.contains("objective check is RED"));
        assert!(summary.contains("off-by-one"));
        assert!(summary.contains("Mandatory repair checklist"));
        assert!(summary.contains("stale symbol"));
        assert!(summary.contains("cache path"));
        assert!(summary.len() <= MAX_SUMMARY_CHARS);

        // A huge issue list is truncated on a char boundary, never panicking.
        let big = VerifierVerdict {
            accepted: false,
            issues: vec!["x".repeat(5000)],
            parse: decision_core::deep_lane::VerifierParse::Json,
            evidence: None,
        };
        assert!(failure_summary(true, &big).len() <= MAX_SUMMARY_CHARS);
    }

    #[test]
    fn exec_retry_context_carries_bounded_prior_attempt_evidence() {
        let check = CheckObservation {
            green: false,
            output_tail: format!("{}CHECK_TAIL", "é".repeat(CHECK_OUTPUT_TAIL_BYTES)),
            ran_in: None,
        };
        let context = exec_retry_context(
            "repair every verifier issue",
            &"d".repeat(EXEC_PRIOR_DIFF_BYTES * 2),
            &[format!(
                "src/{}path.rs",
                "é".repeat(EXEC_PRIOR_EDITED_PATHS_BYTES)
            )],
            Some(("cargo test -p runtime", &check)),
        );

        assert!(context.contains("repair every verifier issue"));
        assert!(context.contains("cargo test -p runtime"));
        assert!(context.contains("CHECK_TAIL"));
        assert!(!context.contains(&"d".repeat(EXEC_PRIOR_DIFF_BYTES + 1)));
        assert!(
            context.len()
                <= "repair every verifier issue".len()
                    + EXEC_PRIOR_DIFF_BYTES
                    + EXEC_PRIOR_EDITED_PATHS_BYTES
                    + CHECK_OUTPUT_TAIL_BYTES
                    + 512
        );
    }

    #[test]
    fn truncate_on_boundary_respects_utf8() {
        let mut s = "héllo wörld".to_string();
        truncate_on_boundary(&mut s, 2); // byte 2 splits 'é' (2 bytes from 'h')
        assert!(s.is_char_boundary(s.len()));
        assert!(s.len() <= 2);

        let mut tail = format!("{}TAIL", "é".repeat(CHECK_OUTPUT_TAIL_BYTES));
        truncate_to_tail_on_boundary(&mut tail, CHECK_OUTPUT_TAIL_BYTES);
        assert!(tail.is_char_boundary(tail.len()));
        assert!(tail.len() <= CHECK_OUTPUT_TAIL_BYTES);
        assert!(tail.ends_with("TAIL"));
    }

    #[test]
    fn verify_leg_failure_folds_to_conservative_non_accept_not_abort() {
        // A failed VERIFY sub-turn (transient streaming error) used to abort the
        // whole deep turn via `?`, discarding the EXEC edits already applied this
        // attempt. The Err-fold path now folds `verify_leg_failed_verdict()`
        // instead. That verdict must be a non-accept tagged `Timeout` so the gate
        // never accepts on a failed verify, and the loop continues honestly.
        let verdict = verify_leg_failed_verdict();
        assert!(!verdict.accepted, "a failed verify leg must never accept");
        assert_eq!(verdict.parse, VerifierParse::Timeout);
        assert!(verdict.issues.is_empty());

        // Mid-loop (attempts remain): fold ⇒ Retry, preserving the applied edits
        // rather than throwing them away. Never Accept.
        let mid = fold_verification_attempt(1, 2, true, &verdict, &[]);
        assert!(!mid.gate_accepted, "Timeout is not a salvage accept");
        assert_eq!(mid.decision, DeepDecision::Retry);

        // Last attempt: fold ⇒ GiveUp, which still ends the turn honestly and
        // leaves the completed implementation in the work tree (no `?` unwind).
        let last = fold_verification_attempt(2, 2, true, &verdict, &[]);
        assert!(!last.gate_accepted);
        assert_eq!(last.decision, DeepDecision::GiveUp);
        // The goal-facing scalar must NOT read as accepted on a failed verify.
        assert!(!goal_facing_accept(&last));
    }

    #[test]
    fn verify_leg_failure_display_is_honest_timeout() {
        // The conservative verdict drives an honest "verifier timed out" note in
        // both reactive and plan-first paths (display already handles Timeout).
        let verdict = verify_leg_failed_verdict();
        assert_eq!(verifier_display_summary(&verdict), "verifier timed out");
    }

    #[test]
    fn verify_leg_budget_verdict_is_distinct_from_timeout_and_displays_honestly() {
        let verdict = verify_budget_exhausted_verdict();
        assert!(!verdict.accepted);
        assert_eq!(verdict.parse, VerifierParse::BudgetExhausted);
        assert_eq!(
            verifier_display_summary(&verdict),
            "verifier hit its inspection budget; objective check gated"
        );
    }

    #[test]
    fn verify_leg_clamps_the_iteration_budget_and_drop_restores_it() {
        use crate::conversation::StaticToolExecutor;
        use crate::session::Session;

        struct NoopApiClient;
        impl crate::conversation::ApiClient for NoopApiClient {
            fn stream(
                &mut self,
                _request: crate::conversation::ApiRequest,
            ) -> Result<Vec<crate::conversation::AssistantEvent>, crate::conversation::RuntimeError>
            {
                unreachable!("guard construction must not call the client")
            }
        }

        let mut runtime = ConversationRuntime::new(
            Session::new(),
            NoopApiClient,
            StaticToolExecutor::new(),
            crate::PermissionPolicy::new(PermissionMode::WorkspaceWrite),
            vec!["system".to_string()],
        );
        // Set explicitly rather than trusting the default: the default reads
        // ZO_MAX_ITERATIONS, and an env-reading assertion is a parallel-test
        // landmine.
        runtime.max_iterations = 40;
        {
            let guard = DeepSubturnPermissionGuard::new(
                &mut runtime,
                PermissionMode::ReadOnly,
                SubturnClient::Native,
                DeepSubturnPhase::Verify,
            );
            assert_eq!(
                guard.runtime.max_iterations, VERIFY_LEG_MAX_ITERATIONS,
                "a VERIFY leg runs under the inspection budget"
            );
        }
        assert_eq!(runtime.max_iterations, 40, "Drop must restore the full budget");
        {
            let guard = DeepSubturnPermissionGuard::new(
                &mut runtime,
                PermissionMode::ReadOnly,
                SubturnClient::Native,
                DeepSubturnPhase::Plan,
            );
            assert_eq!(
                guard.runtime.max_iterations, 40,
                "the cap is verify-local; PLAN keeps the full budget"
            );
        }
        assert_eq!(runtime.max_iterations, 40);
        // A budget SMALLER than the cap is never raised by the clamp.
        runtime.max_iterations = 5;
        {
            let guard = DeepSubturnPermissionGuard::new(
                &mut runtime,
                PermissionMode::ReadOnly,
                SubturnClient::Native,
                DeepSubturnPhase::Verify,
            );
            assert_eq!(guard.runtime.max_iterations, 5);
        }
        assert_eq!(runtime.max_iterations, 5);
    }

    /// A leg on its own client is a model switch the router never saw, and
    /// the guard tells the switch observer: which leg, from the main model to
    /// the leg's model, with the context the leg actually sends — nothing for
    /// an isolated VERIFY leg, the conversation for a PLAN leg — and the
    /// turn's attempt. A native leg swaps nothing and says nothing.
    #[test]
    fn a_leg_on_its_own_client_reaches_the_switch_observer_with_what_it_sends() {
        use std::future::Future;
        use std::pin::Pin;
        use std::sync::Mutex;

        use crate::conversation::{ApiRequest, AssistantEvent, RuntimeError, StaticToolExecutor};
        use crate::session::Session;
        use crate::{ModelSwitch, SwitchTrigger};

        struct NoopApiClient;
        impl crate::conversation::ApiClient for NoopApiClient {
            fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
                unreachable!("guard construction must not call the client")
            }
        }
        struct StubAsyncClient;
        impl AsyncApiClient for StubAsyncClient {
            fn stream_async<'a>(
                &'a self,
                _request: ApiRequest,
                _render_tx: mpsc::Sender<RenderBlock>,
                _text_block_id: crate::message_stream::types::BlockId,
            ) -> Pin<Box<dyn Future<Output = Result<Vec<AssistantEvent>, RuntimeError>> + Send + 'a>>
            {
                Box::pin(async { unreachable!("guard construction must not stream") })
            }
        }

        let seen: Arc<Mutex<Vec<ModelSwitch>>> = Arc::new(Mutex::new(Vec::new()));
        let mut runtime = ConversationRuntime::new(
            Session::new(),
            NoopApiClient,
            StaticToolExecutor::new(),
            crate::PermissionPolicy::new(PermissionMode::WorkspaceWrite),
            vec!["system".to_string()],
        );
        runtime.set_context_model("main-model");
        runtime
            .session
            .push_user_text("a conversation the planner reads and the verifier must not".to_string())
            .unwrap();
        let context = u64::try_from(runtime.estimated_tokens()).unwrap();
        assert!(context > 0);
        runtime.set_deep_verify_candidates(vec![(Arc::new(StubAsyncClient), "verifier-x".to_string())]);
        runtime.set_deep_plan_client(Some((Arc::new(StubAsyncClient), "planner-y".to_string())));
        {
            let seen = Arc::clone(&seen);
            runtime.set_switch_observer(Some(Arc::new(move |switch: &ModelSwitch| {
                seen.lock().unwrap().push(switch.clone());
            })));
        }
        {
            let _guard = DeepSubturnPermissionGuard::new(
                &mut runtime,
                PermissionMode::ReadOnly,
                SubturnClient::Verify,
                DeepSubturnPhase::Verify,
            );
        }
        {
            let _guard = DeepSubturnPermissionGuard::new(
                &mut runtime,
                PermissionMode::ReadOnly,
                SubturnClient::Plan,
                DeepSubturnPhase::Plan,
            );
        }
        {
            let _guard = DeepSubturnPermissionGuard::new(
                &mut runtime,
                PermissionMode::ReadOnly,
                SubturnClient::Native,
                DeepSubturnPhase::Plan,
            );
        }
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2, "two swapped legs, one native: {seen:?}");
        assert_eq!(
            (seen[0].trigger, seen[0].from.as_str(), seen[0].to.as_str(), seen[0].context_tokens),
            (SwitchTrigger::VerifyLeg, "main-model", "verifier-x", 0),
            "VERIFY is isolated: it sends its prompt alone"
        );
        assert_eq!(
            (seen[1].trigger, seen[1].from.as_str(), seen[1].to.as_str(), seen[1].context_tokens),
            (SwitchTrigger::PlanLeg, "main-model", "planner-y", context),
            "PLAN reads the conversation"
        );
        // The guards restored the conversation for the next leg.
        assert_eq!(runtime.estimated_tokens(), usize::try_from(context).unwrap());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn deep_subturn_restores_permission_mode_when_dropped_mid_stream() {
        use std::future::{Future, pending};
        use std::pin::Pin;

        use crate::conversation::{
            ApiRequest, AssistantEvent, AsyncApiClient, RuntimeError, StaticToolExecutor,
        };
        use crate::permission::{
            PermissionDecision as AsyncPermissionDecision, PermissionError,
            PermissionRequest as AsyncPermissionRequest,
        };
        use crate::session::Session;

        struct NoopApiClient;

        impl ApiClient for NoopApiClient {
            fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
                Ok(vec![AssistantEvent::MessageStop])
            }
        }

        struct PendingAsyncClient {
            entered: Arc<tokio::sync::Notify>,
        }

        impl AsyncApiClient for PendingAsyncClient {
            fn stream_async<'a>(
                &'a self,
                _request: ApiRequest,
                _render_tx: mpsc::Sender<RenderBlock>,
                _text_block_id: crate::message_stream::types::BlockId,
            ) -> Pin<
                Box<
                    dyn Future<Output = Result<Vec<AssistantEvent>, RuntimeError>> + Send + 'a,
                >,
            > {
                let entered = Arc::clone(&self.entered);
                Box::pin(async move {
                    entered.notify_one();
                    pending().await
                })
            }
        }

        struct DenyAsyncPrompter;

        impl AsyncPermissionPrompter for DenyAsyncPrompter {
            fn decide<'a>(
                &'a self,
                _request: AsyncPermissionRequest,
            ) -> Pin<
                Box<
                    dyn Future<Output = Result<AsyncPermissionDecision, PermissionError>>
                        + Send
                        + 'a,
                >,
            > {
                Box::pin(async { Ok(AsyncPermissionDecision::Deny) })
            }
        }

        let entered_stream = Arc::new(tokio::sync::Notify::new());
        let mut runtime = ConversationRuntime::new(
            Session::new(),
            NoopApiClient,
            StaticToolExecutor::new(),
            crate::PermissionPolicy::new(PermissionMode::WorkspaceWrite)
                .with_tool_requirement("bash", PermissionMode::DangerFullAccess),
            vec!["system".to_string()],
        )
        .with_async_api_client(Arc::new(PendingAsyncClient {
            entered: Arc::clone(&entered_stream),
        }));
        let (render_tx, mut render_rx) = mpsc::channel(64);
        let _drain = tokio::spawn(async move { while render_rx.recv().await.is_some() {} });
        let prompter: Arc<dyn AsyncPermissionPrompter> = Arc::new(DenyAsyncPrompter);

        let mut subturn = Box::pin(runtime.deep_subturn(
            "inspect before editing".to_string(),
            Vec::new(),
            PermissionMode::ReadOnly,
            SubturnClient::Native,
            DeepSubturnPhase::Plan,
            &render_tx,
            &prompter,
        ));
        tokio::select! {
            result = subturn.as_mut() => panic!("pending stream unexpectedly completed: {result:?}"),
            () = entered_stream.notified() => {}
        }

        drop(subturn);

        assert_eq!(
            runtime.permission_policy.active_mode(),
            PermissionMode::WorkspaceWrite,
            "dropping a PLAN/VERIFY sub-turn future must restore the previous permission mode"
        );
        assert!(
            matches!(
                runtime.permission_policy.authorize(
                    "bash",
                    r#"{"command":"cargo test -p runtime"}"#,
                    None,
                ),
                crate::PermissionOutcome::Deny { .. }
            ),
            "dropping the sub-turn future must remove the temporary read-only bash/Cargo allow grant"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[allow(clippy::too_many_lines)] // stub-heavy async harness, mirrors the drop test above
    async fn verify_subturn_sends_focused_packet_and_restores_native_after_drop() {
        use std::future::{Future, pending};
        use std::pin::Pin;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Mutex;

        use crate::conversation::{
            ApiRequest, AssistantEvent, AsyncApiClient, RuntimeError, StaticToolExecutor,
        };
        use crate::permission::{
            PermissionDecision as AsyncPermissionDecision, PermissionError,
            PermissionRequest as AsyncPermissionRequest,
        };
        use crate::session::Session;

        struct NoopApiClient;
        impl ApiClient for NoopApiClient {
            fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
                Ok(vec![AssistantEvent::MessageStop])
            }
        }

        /// Counts entries and then hangs, so the test can observe *which*
        /// client a leg streamed on and cancel it mid-flight.
        struct CountingPendingClient {
            calls: Arc<AtomicUsize>,
            entered: Arc<tokio::sync::Notify>,
            captured: Option<Arc<Mutex<Option<ApiRequest>>>>,
        }
        impl AsyncApiClient for CountingPendingClient {
            fn stream_async<'a>(
                &'a self,
                request: ApiRequest,
                _render_tx: mpsc::Sender<RenderBlock>,
                _text_block_id: crate::message_stream::types::BlockId,
            ) -> Pin<Box<dyn Future<Output = Result<Vec<AssistantEvent>, RuntimeError>> + Send + 'a>>
            {
                self.calls.fetch_add(1, Ordering::SeqCst);
                if let Some(captured) = &self.captured {
                    *captured.lock().expect("request lock") = Some(request);
                }
                let entered = Arc::clone(&self.entered);
                Box::pin(async move {
                    entered.notify_one();
                    pending().await
                })
            }
        }

        struct DenyAsyncPrompter;
        impl AsyncPermissionPrompter for DenyAsyncPrompter {
            fn decide<'a>(
                &'a self,
                _request: AsyncPermissionRequest,
            ) -> Pin<
                Box<dyn Future<Output = Result<AsyncPermissionDecision, PermissionError>> + Send + 'a>,
            > {
                Box::pin(async { Ok(AsyncPermissionDecision::Deny) })
            }
        }

        let native_calls = Arc::new(AtomicUsize::new(0));
        let native_entered = Arc::new(tokio::sync::Notify::new());
        let cross_calls = Arc::new(AtomicUsize::new(0));
        let cross_entered = Arc::new(tokio::sync::Notify::new());
        let captured = Arc::new(Mutex::new(None));
        let prior_marker = "PRIOR_SESSION_MARKER_MUST_NOT_REACH_VERIFY";
        let mut session = Session::new();
        session
            .push_user_text(prior_marker)
            .expect("seed prior conversation");

        let mut runtime = ConversationRuntime::new(
            session,
            NoopApiClient,
            StaticToolExecutor::new(),
            crate::PermissionPolicy::new(PermissionMode::WorkspaceWrite),
            vec!["system".to_string()],
        )
        .with_async_api_client(Arc::new(CountingPendingClient {
            calls: Arc::clone(&native_calls),
            entered: Arc::clone(&native_entered),
            captured: None,
        }));
        runtime.set_deep_verify_client(Some((
            Arc::new(CountingPendingClient {
                calls: Arc::clone(&cross_calls),
                entered: Arc::clone(&cross_entered),
                captured: Some(Arc::clone(&captured)),
            }),
            "cross-verifier-model".to_string(),
        )));

        let (render_tx, mut render_rx) = mpsc::channel(64);
        let _drain = tokio::spawn(async move { while render_rx.recv().await.is_some() {} });
        let prompter: Arc<dyn AsyncPermissionPrompter> = Arc::new(DenyAsyncPrompter);
        let ids = BlockIdGen::default();
        let check = CheckObservation {
            green: true,
            output_tail: "FOCUSED_CHECK_OUTPUT_MARKER".to_string(),
            ran_in: None,
        };
        let verify_image = ("image/png".to_string(), test_png_base64(3, 2));
        let packet = verify_prompt(
            "FOCUSED_TASK_MARKER",
            "FOCUSED_DIFF_MARKER",
            Some(("cargo test -p runtime", &check)),
            GreenCheckEvidence::default(),
            &["src/focused.rs".to_string()],
            &[],
            "FOCUSED_ASSISTANT_CLAIM_MARKER",
            "",
            VerifyLensMode::Full,
            RouteTaskIntent::Other,
            true,
        );

        // The VERIFY leg must stream on the cross-model client, not the native one.
        let mut subturn = Box::pin(runtime.verify_subturn(
            packet,
            vec![verify_image.clone()],
            &render_tx,
            &ids,
            &prompter,
        ));
        tokio::select! {
            result = subturn.as_mut() => panic!("pending cross stream unexpectedly completed: {result:?}"),
            () = cross_entered.notified() => {}
        }
        let request = captured
            .lock()
            .expect("request lock")
            .clone()
            .expect("captured verifier request");
        let request_text = request
            .messages
            .iter()
            .flat_map(|message| &message.blocks)
            .filter_map(|block| match block {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(request.messages.len(), 1);
        assert!(request_text.contains("FOCUSED_TASK_MARKER"));
        assert!(request_text.contains("FOCUSED_DIFF_MARKER"));
        assert!(request_text.contains("FOCUSED_CHECK_OUTPUT_MARKER"));
        assert!(request_text.contains("src/focused.rs"));
        assert!(request_text.contains("FOCUSED_ASSISTANT_CLAIM_MARKER"));
        assert!(request_text.contains("Attached images are direct visual evidence"));
        assert!(!request_text.contains(prior_marker));
        assert!(request.messages.iter().any(|message| {
            message.blocks.iter().any(
                |block| matches!(
                    block,
                    ContentBlock::Image { media_type, data }
                        if media_type == &verify_image.0 && data == &verify_image.1
                ),
            )
        }));
        drop(subturn);
        assert!(runtime.session.messages.iter().any(|message| {
            message.blocks.iter().any(
                |block| matches!(block, ContentBlock::Text { text } if text == prior_marker),
            )
        }));
        assert_eq!(cross_calls.load(Ordering::SeqCst), 1, "verify leg must use the cross client");
        assert_eq!(
            native_calls.load(Ordering::SeqCst),
            0,
            "the native client must not stream during a cross verify leg"
        );

        // Dropping the leg mid-stream must restore the native client for
        // subsequent (non-verify) sub-turns.
        let mut ordinary = Box::pin(runtime.deep_subturn(
            "plan next".to_string(),
            Vec::new(),
            PermissionMode::ReadOnly,
            SubturnClient::Native,
            DeepSubturnPhase::Plan,
            &render_tx,
            &prompter,
        ));
        tokio::select! {
            result = ordinary.as_mut() => panic!("pending native stream unexpectedly completed: {result:?}"),
            () = native_entered.notified() => {}
        }
        drop(ordinary);
        assert_eq!(
            native_calls.load(Ordering::SeqCst),
            1,
            "after the cancelled verify leg, ordinary sub-turns must be back on the native client"
        );
        assert_eq!(cross_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[allow(clippy::too_many_lines)] // end-to-end EXEC tool drain → VERIFY request harness
    async fn exec_tool_images_reach_the_following_verify_subturn() {
        use std::future::Future;
        use std::pin::Pin;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Mutex;

        use crate::conversation::{
            ApiRequest, AssistantEvent, AsyncApiClient, RuntimeError, ToolError, ToolExecutor,
        };
        use crate::permission::{
            PermissionDecision as AsyncPermissionDecision, PermissionError,
            PermissionRequest as AsyncPermissionRequest,
        };
        use crate::session::Session;

        struct NoopApiClient;
        impl ApiClient for NoopApiClient {
            fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
                Ok(vec![AssistantEvent::MessageStop])
            }
        }

        struct ImageToolExecutor {
            image: (String, String),
            pending: Vec<(String, String)>,
        }
        impl ToolExecutor for ImageToolExecutor {
            fn execute(&mut self, tool_name: &str, _input: &str) -> Result<String, ToolError> {
                assert_eq!(tool_name, "write_file");
                self.pending.push(self.image.clone());
                Ok(r#"{"filePath":"src/visual.rs"}"#.to_string())
            }

            fn take_pending_images(&mut self) -> Vec<(String, String)> {
                std::mem::take(&mut self.pending)
            }
        }

        struct ExecThenVerifyClient {
            calls: Arc<AtomicUsize>,
            verify_request: Arc<Mutex<Option<ApiRequest>>>,
        }
        impl AsyncApiClient for ExecThenVerifyClient {
            fn stream_async<'a>(
                &'a self,
                request: ApiRequest,
                _render_tx: mpsc::Sender<RenderBlock>,
                _text_block_id: crate::message_stream::types::BlockId,
            ) -> Pin<Box<dyn Future<Output = Result<Vec<AssistantEvent>, RuntimeError>> + Send + 'a>>
            {
                let call = self.calls.fetch_add(1, Ordering::SeqCst);
                if call == 2 {
                    *self.verify_request.lock().expect("request lock") = Some(request);
                }
                Box::pin(async move {
                    Ok(match call {
                        0 => vec![
                            AssistantEvent::ToolUse {
                                id: "write-visual".to_string(),
                                name: "write_file".to_string(),
                                input: r#"{"path":"src/visual.rs","content":"visual"}"#.to_string(),
                            },
                            AssistantEvent::MessageStop,
                        ],
                        1 => vec![
                            AssistantEvent::TextDelta("implemented".to_string()),
                            AssistantEvent::MessageStop,
                        ],
                        2 => vec![
                            AssistantEvent::TextDelta(
                                r#"{"accepted":true,"issues":[],"evidence":"inspected attached screenshot"}"#
                                    .to_string(),
                            ),
                            AssistantEvent::MessageStop,
                        ],
                        _ => panic!("unexpected extra model request"),
                    })
                })
            }
        }

        struct DenyAsyncPrompter;
        impl AsyncPermissionPrompter for DenyAsyncPrompter {
            fn decide<'a>(
                &'a self,
                _request: AsyncPermissionRequest,
            ) -> Pin<
                Box<dyn Future<Output = Result<AsyncPermissionDecision, PermissionError>> + Send + 'a>,
            > {
                Box::pin(async { Ok(AsyncPermissionDecision::Deny) })
            }
        }

        let image = ("image/png".to_string(), test_png_base64(4, 4));
        let calls = Arc::new(AtomicUsize::new(0));
        let verify_request = Arc::new(Mutex::new(None));
        let mut runtime = ConversationRuntime::new(
            Session::new(),
            NoopApiClient,
            ImageToolExecutor {
                image: image.clone(),
                pending: Vec::new(),
            },
            crate::PermissionPolicy::new(PermissionMode::WorkspaceWrite)
                .with_tool_requirement("write_file", PermissionMode::WorkspaceWrite),
            vec!["system".to_string()],
        )
        .with_async_api_client(Arc::new(ExecThenVerifyClient {
            calls: Arc::clone(&calls),
            verify_request: Arc::clone(&verify_request),
        }));
        runtime.set_deep_gate(Some(DeepGateConfig {
            mode: DeepMode::Reactive,
            check_command: None,
            max_attempts: 1,
        }));
        runtime.set_verify_intent(RouteTaskIntent::Design);

        let (render_tx, mut render_rx) = mpsc::channel(64);
        let _drain = tokio::spawn(async move { while render_rx.recv().await.is_some() {} });
        let prompter: Arc<dyn AsyncPermissionPrompter> = Arc::new(DenyAsyncPrompter);
        runtime
            .run_auto_turn_streaming("build the visual", Vec::new(), render_tx, prompter)
            .await
            .expect("deep turn");

        let request = verify_request
            .lock()
            .expect("request lock")
            .clone()
            .expect("captured VERIFY request");
        let attached = request
            .messages
            .iter()
            .flat_map(|message| &message.blocks)
            .filter_map(|block| match block {
                ContentBlock::Image { media_type, data } => {
                    Some((media_type.clone(), data.clone()))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(attached, vec![image]);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test(flavor = "current_thread")]
    // The serialization guard below is held across awaits BY DESIGN (the
    // whole test is the critical section against sibling registry writers),
    // and it deliberately precedes the test's local item definitions.
    #[allow(clippy::await_holding_lock, clippy::items_after_statements)]
    async fn deep_exec_retries_typed_transport_failure_into_next_attempt() {
        use std::future::Future;
        use std::pin::Pin;
        use std::sync::atomic::{AtomicUsize, Ordering};

        use crate::conversation::{
            ApiRequest, AssistantEvent, AsyncApiClient, RuntimeError, StaticToolExecutor,
        };
        use crate::message_stream::types::{BlockId, RenderBlock};
        use crate::permission::{
            PermissionDecision as AsyncPermissionDecision, PermissionError,
            PermissionPrompter as AsyncPermissionPrompter,
            PermissionRequest as AsyncPermissionRequest,
        };
        use crate::session::Session;

        let _quota_serial = api::quota::rate_limit_test_guard();

        struct NoopApiClient;
        impl ApiClient for NoopApiClient {
            fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
                Ok(vec![AssistantEvent::MessageStop])
            }
        }

        struct FailThenSuccess {
            calls: Arc<AtomicUsize>,
        }
        impl AsyncApiClient for FailThenSuccess {
            fn stream_async<'a>(
                &'a self,
                _request: ApiRequest,
                _render_tx: mpsc::Sender<RenderBlock>,
                _text_block_id: BlockId,
            ) -> Pin<Box<dyn Future<Output = Result<Vec<AssistantEvent>, RuntimeError>> + Send + 'a>>
            {
                let call = self.calls.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move {
                    if call == 0 {
                        return Err(RuntimeError::with_provider_error_class(
                            "api failed after 6 attempts: api returned 429 Too Many Requests",
                            api::ProviderErrorClass::account_rate_limit(None),
                        ));
                    }
                    Ok(vec![
                        AssistantEvent::TextDelta("recovered".to_string()),
                        AssistantEvent::MessageStop,
                    ])
                })
            }
        }

        struct AllowAsyncPrompter;
        impl AsyncPermissionPrompter for AllowAsyncPrompter {
            fn decide<'a>(
                &'a self,
                _request: AsyncPermissionRequest,
            ) -> Pin<
                Box<dyn Future<Output = Result<AsyncPermissionDecision, PermissionError>> + Send + 'a>,
            > {
                Box::pin(async { Ok(AsyncPermissionDecision::Allow) })
            }
        }

        let calls = Arc::new(AtomicUsize::new(0));
        let mut runtime = ConversationRuntime::new(
            Session::new(),
            NoopApiClient,
            StaticToolExecutor::new(),
            crate::PermissionPolicy::new(PermissionMode::WorkspaceWrite),
            vec!["system".to_string()],
        )
        .with_async_api_client(Arc::new(FailThenSuccess {
            calls: Arc::clone(&calls),
        }));
        runtime.set_deep_gate(Some(DeepGateConfig {
            mode: DeepMode::Reactive,
            check_command: None,
            max_attempts: 2,
        }));

        let (render_tx, mut render_rx) = mpsc::channel(64);
        let _drain = tokio::spawn(async move { while render_rx.recv().await.is_some() {} });
        let prompter: Arc<dyn AsyncPermissionPrompter> = Arc::new(AllowAsyncPrompter);
        let (_summary, outcome) = runtime
            .run_auto_turn_streaming("retry the task", Vec::new(), render_tx, prompter)
            .await
            .expect("typed transport failure should use the remaining EXEC attempt");

        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(outcome.attempts, 2);
        assert_eq!(outcome.decision, DeepDecision::Accept);
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test(flavor = "current_thread")]
    // The serialization guard below is held across awaits BY DESIGN (the
    // whole test is the critical section against sibling registry writers),
    // and it deliberately precedes the test's local item definitions.
    #[allow(clippy::await_holding_lock, clippy::items_after_statements)]
    async fn deep_exec_implementer_transport_failure_escalates_to_native() {
        use std::future::Future;
        use std::pin::Pin;
        use std::sync::atomic::{AtomicUsize, Ordering};

        use crate::conversation::{
            ApiRequest, AssistantEvent, AsyncApiClient, ExecContract, RuntimeError,
            StaticToolExecutor,
        };
        use crate::message_stream::types::{BlockId, RenderBlock};
        use crate::permission::{
            PermissionDecision as AsyncPermissionDecision, PermissionError,
            PermissionPrompter as AsyncPermissionPrompter,
            PermissionRequest as AsyncPermissionRequest,
        };
        use crate::session::Session;
        let _quota_serial = api::quota::rate_limit_test_guard();

        struct NoopApiClient;
        impl ApiClient for NoopApiClient {
            fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
                Ok(vec![AssistantEvent::MessageStop])
            }
        }

        struct NativeSuccess {
            calls: Arc<AtomicUsize>,
        }
        impl AsyncApiClient for NativeSuccess {
            fn stream_async<'a>(
                &'a self,
                _request: ApiRequest,
                _render_tx: mpsc::Sender<RenderBlock>,
                _text_block_id: BlockId,
            ) -> Pin<Box<dyn Future<Output = Result<Vec<AssistantEvent>, RuntimeError>> + Send + 'a>>
            {
                self.calls.fetch_add(1, Ordering::SeqCst);
                Box::pin(async {
                    Ok(vec![
                        AssistantEvent::TextDelta("native recovery".to_string()),
                        AssistantEvent::MessageStop,
                    ])
                })
            }
        }

        struct ImplementerRateLimited {
            calls: Arc<AtomicUsize>,
        }
        impl AsyncApiClient for ImplementerRateLimited {
            fn stream_async<'a>(
                &'a self,
                _request: ApiRequest,
                _render_tx: mpsc::Sender<RenderBlock>,
                _text_block_id: BlockId,
            ) -> Pin<Box<dyn Future<Output = Result<Vec<AssistantEvent>, RuntimeError>> + Send + 'a>>
            {
                self.calls.fetch_add(1, Ordering::SeqCst);
                Box::pin(async {
                    Err(RuntimeError::with_provider_error_class(
                        "api failed after 6 attempts: api returned 429 Too Many Requests",
                        api::ProviderErrorClass::account_rate_limit(None),
                    ))
                })
            }
        }

        struct AllowAsyncPrompter;
        impl AsyncPermissionPrompter for AllowAsyncPrompter {
            fn decide<'a>(
                &'a self,
                _request: AsyncPermissionRequest,
            ) -> Pin<
                Box<dyn Future<Output = Result<AsyncPermissionDecision, PermissionError>> + Send + 'a>,
            > {
                Box::pin(async { Ok(AsyncPermissionDecision::Allow) })
            }
        }

        let native_calls = Arc::new(AtomicUsize::new(0));
        let implementer_calls = Arc::new(AtomicUsize::new(0));
        let implementer: Arc<dyn AsyncApiClient> = Arc::new(ImplementerRateLimited {
            calls: Arc::clone(&implementer_calls),
        });
        let mut runtime = ConversationRuntime::new(
            Session::new(),
            NoopApiClient,
            StaticToolExecutor::new(),
            crate::PermissionPolicy::new(PermissionMode::WorkspaceWrite),
            vec!["system".to_string()],
        )
        .with_async_api_client(Arc::new(NativeSuccess {
            calls: Arc::clone(&native_calls),
        }));
        runtime.set_context_model("claude-opus-4-8");
        runtime.set_exec_contract(Some(ExecContract {
            impl_client: Some(implementer),
            impl_model: "gpt-5.6-sol".to_string(),
            plan_first: false,
        }));
        runtime.set_deep_gate(Some(DeepGateConfig {
            mode: DeepMode::Reactive,
            check_command: None,
            max_attempts: 2,
        }));

        let (render_tx, mut render_rx) = mpsc::channel(64);
        let _drain = tokio::spawn(async move { while render_rx.recv().await.is_some() {} });
        let prompter: Arc<dyn AsyncPermissionPrompter> = Arc::new(AllowAsyncPrompter);
        let (_summary, outcome) = runtime
            .run_auto_turn_streaming("recover through the main model", Vec::new(), render_tx, prompter)
            .await
            .expect("native EXEC should recover an implementer transport failure");

        assert_eq!(implementer_calls.load(Ordering::SeqCst), 1);
        assert_eq!(native_calls.load(Ordering::SeqCst), 1);
        assert_eq!(outcome.attempts, 2);
        assert_eq!(outcome.decision, DeepDecision::Accept);
    }

    #[test]
    fn exec_leg_client_swaps_for_two_attempts_then_escalates_native() {
        use crate::conversation::{ApiRequest, AssistantEvent, RuntimeError, StaticToolExecutor};
        use crate::session::Session;

        struct NoopApiClient;
        impl ApiClient for NoopApiClient {
            fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
                Ok(vec![AssistantEvent::MessageStop])
            }
        }
        struct NeverAsyncClient;
        impl AsyncApiClient for NeverAsyncClient {
            fn stream_async<'a>(
                &'a self,
                _request: ApiRequest,
                _render_tx: mpsc::Sender<RenderBlock>,
                _text_block_id: crate::message_stream::types::BlockId,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<Output = Result<Vec<AssistantEvent>, RuntimeError>>
                        + Send
                        + 'a,
                >,
            > {
                Box::pin(async { Ok(vec![AssistantEvent::MessageStop]) })
            }
        }

        let mut runtime = ConversationRuntime::new(
            Session::new(),
            NoopApiClient,
            StaticToolExecutor::new(),
            crate::PermissionPolicy::new(PermissionMode::WorkspaceWrite),
            vec!["system".to_string()],
        );
        // No contract: every attempt runs native.
        assert_eq!(runtime.exec_leg_client(1), SubturnClient::Native);
        assert_eq!(runtime.exec_leg_client(3), SubturnClient::Native);
        assert!(runtime.exec_leg_note(1).is_none(), "no contract, nothing to announce");

        runtime.set_exec_contract(Some(ExecContract {
            impl_client: None,
            impl_model: "gpt-5.6-terra".to_string(),
            plan_first: true,
        }));
        // Unarmed contract (default medium/hard, or `never`): plan-first
        // metadata stays installed, but every EXEC attempt uses the native
        // client and nothing is announced.
        assert!(runtime.exec_contract().is_some_and(|contract| contract.plan_first));
        assert_eq!(runtime.exec_leg_client(1), SubturnClient::NativeExec);
        assert_eq!(runtime.exec_leg_client(ARCHITECT_IMPL_ATTEMPTS + 1), SubturnClient::NativeExec);
        assert!(runtime.exec_leg_note(1).is_none());

        runtime.set_exec_contract(Some(ExecContract {
            impl_client: Some(Arc::new(NeverAsyncClient)),
            impl_model: "gpt-5.6-terra".to_string(),
            plan_first: false,
        }));
        // Contract: the first ARCHITECT_IMPL_ATTEMPTS run on the implementer,
        // then implementation escalates back to the native (reserved) model —
        // the same "two real failures" rule as the router's premium gate.
        assert_eq!(runtime.exec_leg_client(1), SubturnClient::Implementer);
        assert_eq!(runtime.exec_leg_client(ARCHITECT_IMPL_ATTEMPTS), SubturnClient::Implementer);
        assert_eq!(
            runtime.exec_leg_client(ARCHITECT_IMPL_ATTEMPTS + 1),
            SubturnClient::Native
        );
        let first = runtime.exec_leg_note(1).expect("attempt 1 announces the contract");
        assert!(first.contains("gpt-5.6-terra"), "{first}");
        assert!(runtime.exec_leg_note(2).is_none(), "no re-announcement mid-loop");
        let escalated = runtime
            .exec_leg_note(ARCHITECT_IMPL_ATTEMPTS + 1)
            .expect("escalation announces the native takeover");
        assert!(escalated.contains("escalating"), "{escalated}");

        // Clearing the contract (the host does this every turn entry) restores
        // native legs.
        runtime.set_exec_contract(None);
        assert_eq!(runtime.exec_leg_client(1), SubturnClient::Native);
    }

    #[test]
    fn architect_plan_uses_a_deep_client_or_reserved_native_only() {
        use std::future::Future;
        use std::pin::Pin;

        use crate::conversation::{ApiRequest, AssistantEvent, RuntimeError, StaticToolExecutor};
        use crate::session::Session;

        struct NoopApiClient;
        impl ApiClient for NoopApiClient {
            fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
                Ok(vec![AssistantEvent::MessageStop])
            }
        }
        struct NeverAsyncClient;
        impl AsyncApiClient for NeverAsyncClient {
            fn stream_async<'a>(
                &'a self,
                _request: ApiRequest,
                _render_tx: mpsc::Sender<RenderBlock>,
                _text_block_id: crate::message_stream::types::BlockId,
            ) -> Pin<
                Box<
                    dyn Future<Output = Result<Vec<AssistantEvent>, RuntimeError>> + Send + 'a,
                >,
            > {
                Box::pin(std::future::pending())
            }
        }

        let mut runtime = ConversationRuntime::new(
            Session::new(),
            NoopApiClient,
            StaticToolExecutor::new(),
            crate::PermissionPolicy::new(PermissionMode::WorkspaceWrite),
            vec!["system".to_string()],
        );
        runtime.set_context_model("gpt-5.6-terra");
        runtime.set_deep_tier_only(true);
        runtime.set_deep_plan_client(Some((
            Arc::new(NeverAsyncClient),
            "claude-fable-5-1".to_string(),
        )));
        assert_eq!(runtime.plan_leg_client().unwrap(), SubturnClient::Plan);

        runtime.set_deep_plan_client(None);
        assert!(
            runtime.plan_leg_client().is_err(),
            "an implementer-tier native model must not inherit PLAN"
        );
        runtime.set_context_model("gpt-5.6-sol");
        assert_eq!(runtime.plan_leg_client().unwrap(), SubturnClient::Native);

        runtime.set_deep_tier_models(vec!["claude-opus-5".to_string()]);
        runtime.set_context_model("opus-5");
        assert_eq!(runtime.plan_leg_client().unwrap(), SubturnClient::Native);
        runtime.set_context_model("claude-fable-5-1");
        assert!(
            runtime.plan_leg_client().is_err(),
            "an explicit pool replaces the built-in membership"
        );
    }

    #[test]
    fn architect_edit_gate_denies_reserved_foreground_edits_until_exempt() {
        use crate::conversation::{ApiRequest, AssistantEvent, RuntimeError, StaticToolExecutor};
        use crate::session::Session;

        struct NoopApiClient;
        impl ApiClient for NoopApiClient {
            fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
                Ok(vec![AssistantEvent::MessageStop])
            }
        }

        let mut runtime = ConversationRuntime::new(
            Session::new(),
            NoopApiClient,
            StaticToolExecutor::new(),
            crate::PermissionPolicy::new(PermissionMode::WorkspaceWrite),
            vec!["system".to_string()],
        );
        // Not armed (the default; sub-agent/headless runtimes stay here):
        // nothing is denied.
        assert!(runtime.architect_edit_gate_denial("edit_file").is_none());

        runtime.set_reserved_edit_gate(true);
        let denial = runtime
            .architect_edit_gate_denial("edit_file")
            .expect("armed gate must deny a foreground edit");
        assert!(denial.contains("swapped implementer EXEC leg"), "{denial}");
        assert!(
            runtime.architect_edit_gate_denial("Write").is_some(),
            "every edit-result tool is gated"
        );
        assert!(
            runtime.architect_edit_gate_denial("read_file").is_none(),
            "read tools pass"
        );
        assert!(
            runtime.architect_edit_gate_denial("bash").is_none(),
            "non-edit tools pass (bash has its own permission ladder)"
        );

        // An EXEC leg on the implementer client is the contract being honored.
        runtime.exec_impl_leg_active = true;
        assert!(runtime.architect_edit_gate_denial("edit_file").is_none());
        runtime.exec_impl_leg_active = false;

        // A scoped native EXEC leg remains exempt if an armed contract reaches
        // its native escalation attempt.
        runtime.exec_native_leg_active = true;
        assert!(runtime.architect_edit_gate_denial("edit_file").is_none());
        runtime.exec_native_leg_active = false;
        assert!(runtime.architect_edit_gate_denial("edit_file").is_some());

        // A ReadOnly phase already denies writes with mode messaging.
        let prior = runtime.permission_policy.set_active_mode(PermissionMode::ReadOnly);
        assert!(runtime.architect_edit_gate_denial("edit_file").is_none());
        runtime.permission_policy.set_active_mode(prior);

        // Disarming (host does this every turn entry; the deep gate does it on
        // failure escalation) restores ordinary behavior.
        runtime.set_reserved_edit_gate(false);
        assert!(runtime.architect_edit_gate_denial("edit_file").is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[allow(clippy::too_many_lines)] // stub-heavy async harness, mirrors the verify swap test above
    async fn exec_impl_leg_sends_focused_packet_and_native_escalation_keeps_history() {
        use std::future::{Future, pending};
        use std::pin::Pin;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Mutex;

        use crate::conversation::{
            ApiRequest, AssistantEvent, AsyncApiClient, RuntimeError, StaticToolExecutor,
        };
        use crate::permission::{
            PermissionDecision as AsyncPermissionDecision, PermissionError,
            PermissionRequest as AsyncPermissionRequest,
        };
        use crate::session::Session;

        struct NoopApiClient;
        impl ApiClient for NoopApiClient {
            fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
                Ok(vec![AssistantEvent::MessageStop])
            }
        }
        struct CountingPendingClient {
            calls: Arc<AtomicUsize>,
            entered: Arc<tokio::sync::Notify>,
            captured: Arc<Mutex<Option<ApiRequest>>>,
        }
        impl AsyncApiClient for CountingPendingClient {
            fn stream_async<'a>(
                &'a self,
                request: ApiRequest,
                _render_tx: mpsc::Sender<RenderBlock>,
                _text_block_id: crate::message_stream::types::BlockId,
            ) -> Pin<Box<dyn Future<Output = Result<Vec<AssistantEvent>, RuntimeError>> + Send + 'a>>
            {
                self.calls.fetch_add(1, Ordering::SeqCst);
                *self.captured.lock().expect("request lock") = Some(request);
                let entered = Arc::clone(&self.entered);
                Box::pin(async move {
                    entered.notify_one();
                    pending().await
                })
            }
        }
        struct DenyAsyncPrompter;
        impl AsyncPermissionPrompter for DenyAsyncPrompter {
            fn decide<'a>(
                &'a self,
                _request: AsyncPermissionRequest,
            ) -> Pin<
                Box<dyn Future<Output = Result<AsyncPermissionDecision, PermissionError>> + Send + 'a>,
            > {
                Box::pin(async { Ok(AsyncPermissionDecision::Deny) })
            }
        }

        let native_calls = Arc::new(AtomicUsize::new(0));
        let native_entered = Arc::new(tokio::sync::Notify::new());
        let impl_calls = Arc::new(AtomicUsize::new(0));
        let impl_entered = Arc::new(tokio::sync::Notify::new());
        let native_request = Arc::new(Mutex::new(None));
        let impl_request = Arc::new(Mutex::new(None));
        let prior_marker = "PRIOR_SESSION_MARKER_MUST_REACH_IMPLEMENTER";
        let mut session = Session::new();
        session
            .push_user_text(prior_marker)
            .expect("seed prior conversation");

        let mut runtime = ConversationRuntime::new(
            session,
            NoopApiClient,
            StaticToolExecutor::new(),
            crate::PermissionPolicy::new(PermissionMode::WorkspaceWrite),
            vec!["system".to_string()],
        )
        .with_async_api_client(Arc::new(CountingPendingClient {
            calls: Arc::clone(&native_calls),
            entered: Arc::clone(&native_entered),
            captured: Arc::clone(&native_request),
        }));
        runtime.set_exec_contract(Some(ExecContract {
            impl_client: Some(Arc::new(CountingPendingClient {
                calls: Arc::clone(&impl_calls),
                entered: Arc::clone(&impl_entered),
                captured: Arc::clone(&impl_request),
            })),
            impl_model: "gpt-5.6-terra".to_string(),
            plan_first: true,
        }));

        let (render_tx, mut render_rx) = mpsc::channel(64);
        let _drain = tokio::spawn(async move { while render_rx.recv().await.is_some() {} });
        let prompter: Arc<dyn AsyncPermissionPrompter> = Arc::new(DenyAsyncPrompter);

        let check = CheckObservation {
            green: false,
            output_tail: "FOCUSED_FAILING_CHECK_OUTPUT".to_string(),
            ran_in: None,
        };
        let retry = exec_retry_context(
            "FOCUSED_REPAIR_CONTEXT",
            "FOCUSED_PRIOR_DIFF",
            &["src/focused.rs".to_string()],
            Some(("cargo test -p runtime", &check)),
        );
        let packet = exec_prompt("FOCUSED_TASK", "FOCUSED_PLAN", Some(&retry));

        // A later EXEC attempt still runs on the implementer client, but sends
        // only its self-contained packet. Cancelling it mid-stream must restore
        // the prior transcript and native client.
        let implementer_client = runtime.exec_leg_client(ARCHITECT_IMPL_ATTEMPTS);
        let mut exec = Box::pin(runtime.deep_subturn(
            packet,
            Vec::new(),
            PermissionMode::WorkspaceWrite,
            implementer_client,
            DeepSubturnPhase::Exec,
            &render_tx,
            &prompter,
        ));
        tokio::select! {
            result = exec.as_mut() => panic!("pending impl stream unexpectedly completed: {result:?}"),
            () = impl_entered.notified() => {}
        }
        let request = impl_request
            .lock()
            .expect("request lock")
            .clone()
            .expect("captured implementer request");
        let request_text = request
            .messages
            .iter()
            .flat_map(|message| &message.blocks)
            .filter_map(|block| match block {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        // The self-contained packet is still the last message, and still carries
        // everything the leg needs to work from.
        assert!(request_text.contains("FOCUSED_TASK"));
        assert!(request_text.contains("FOCUSED_PLAN"));
        assert!(request_text.contains("FOCUSED_REPAIR_CONTEXT"));
        assert!(request_text.contains("FOCUSED_PRIOR_DIFF"));
        assert!(request_text.contains("src/focused.rs"));
        assert!(request_text.contains("cargo test -p runtime"));
        assert!(request_text.contains("FOCUSED_FAILING_CHECK_OUTPUT"));
        // ...and the conversation reaches it. The implementer writes the
        // user-visible artifact, so a follow-up like "now make it darker" needs
        // the turns that establish what "it" is. Only VERIFY runs isolated,
        // because a judge must weigh the diff rather than the chat that argued
        // for it.
        assert!(
            request.messages.len() > 1,
            "implementer must see the conversation, not just its packet"
        );
        assert!(request_text.contains(prior_marker));
        drop(exec);
        assert!(runtime.session.messages.iter().any(|message| {
            message.blocks.iter().any(
                |block| matches!(block, ContentBlock::Text { text } if text == prior_marker),
            )
        }));
        assert_eq!(impl_calls.load(Ordering::SeqCst), 1, "EXEC leg must use the implementer client");
        assert_eq!(native_calls.load(Ordering::SeqCst), 0);
        assert!(
            !runtime.exec_impl_leg_active,
            "dropping the EXEC leg must clear the implementer-leg flag"
        );

        // The post-failure escalation is a native EXEC leg and keeps the full,
        // restored conversation in the session model's cache namespace.
        let native_client = runtime.exec_leg_client(ARCHITECT_IMPL_ATTEMPTS + 1);
        let mut ordinary = Box::pin(runtime.deep_subturn(
            exec_prompt("NATIVE_TASK", "NATIVE_PLAN", Some(&retry)),
            Vec::new(),
            PermissionMode::WorkspaceWrite,
            native_client,
            DeepSubturnPhase::Exec,
            &render_tx,
            &prompter,
        ));
        tokio::select! {
            result = ordinary.as_mut() => panic!("pending native stream unexpectedly completed: {result:?}"),
            () = native_entered.notified() => {}
        }
        let request = native_request
            .lock()
            .expect("request lock")
            .clone()
            .expect("captured native escalation request");
        let request_text = request
            .messages
            .iter()
            .flat_map(|message| &message.blocks)
            .filter_map(|block| match block {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(request_text.contains(prior_marker));
        assert!(request_text.contains("NATIVE_TASK"));
        drop(ordinary);
        assert_eq!(native_calls.load(Ordering::SeqCst), 1);
        assert_eq!(impl_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[allow(clippy::too_many_lines)] // two end-to-end candidate-walk scenarios
    // The serialization guard below is held across awaits BY DESIGN (the
    // whole test is the critical section against sibling registry writers),
    // and it deliberately precedes the test's local item definitions.
    #[allow(clippy::await_holding_lock, clippy::items_after_statements)]
    async fn verify_subturn_uses_ranked_candidates_then_native_fallback() {
        use std::future::Future;
        use std::pin::Pin;
        use std::sync::atomic::{AtomicUsize, Ordering};

        use crate::conversation::{
            ApiRequest, AssistantEvent, AsyncApiClient, RuntimeError, StaticToolExecutor,
        };
        use crate::permission::{
            PermissionDecision as AsyncPermissionDecision, PermissionError,
            PermissionRequest as AsyncPermissionRequest,
        };
        use crate::session::Session;
        let _quota_serial = api::quota::rate_limit_test_guard();

        struct NoopApiClient;
        impl ApiClient for NoopApiClient {
            fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
                Ok(vec![AssistantEvent::MessageStop])
            }
        }

        #[derive(Clone, Copy)]
        enum Outcome {
            Stop,
            RateLimit,
        }

        struct CountingAsyncClient {
            calls: Arc<AtomicUsize>,
            outcome: Outcome,
        }
        impl AsyncApiClient for CountingAsyncClient {
            fn stream_async<'a>(
                &'a self,
                _request: ApiRequest,
                _render_tx: mpsc::Sender<RenderBlock>,
                _text_block_id: crate::message_stream::types::BlockId,
            ) -> Pin<Box<dyn Future<Output = Result<Vec<AssistantEvent>, RuntimeError>> + Send + 'a>>
            {
                self.calls.fetch_add(1, Ordering::SeqCst);
                let outcome = self.outcome;
                Box::pin(async move {
                    match outcome {
                        Outcome::Stop => Ok(vec![
                            AssistantEvent::TextDelta(
                                r#"{"accepted":true,"issues":[]}"#.to_string(),
                            ),
                            AssistantEvent::MessageStop,
                        ]),
                        Outcome::RateLimit => Err(RuntimeError::with_provider_error_class(
                            "verifier rate-limited",
                            api::ProviderErrorClass::account_rate_limit(None),
                        )),
                    }
                })
            }
        }

        struct DenyAsyncPrompter;
        impl AsyncPermissionPrompter for DenyAsyncPrompter {
            fn decide<'a>(
                &'a self,
                _request: AsyncPermissionRequest,
            ) -> Pin<
                Box<dyn Future<Output = Result<AsyncPermissionDecision, PermissionError>> + Send + 'a>,
            > {
                Box::pin(async { Ok(AsyncPermissionDecision::Deny) })
            }
        }

        let prompter: Arc<dyn AsyncPermissionPrompter> = Arc::new(DenyAsyncPrompter);

        // The ranked walk pre-flights every candidate against the process- and
        // account-global quota registry, so without this the verdict depends on
        // whatever a sibling test recorded (`retry::` marks an Anthropic
        // cool-down) or on a real `zo` throttling the same account through the
        // cross-process file. Both made this test fail on a clean checkout.
        api::quota::isolate_rate_limit_state_for_tests();
        // Walk semantics are driven by CLIENT outcomes below; pin the quota
        // pre-flight to a clean registry so no parallel test's capacity-stall
        // marking can inject a skip (policy coverage lives in the pure
        // `skip_parked_verifier` tests).
        ConversationRuntime::<NoopApiClient, StaticToolExecutor>::set_quota_preflight_clean_for_this_thread();

        // A 429 on the first provider skips lower-ranked models on that same
        // provider and uses the next different-provider candidate. It must not
        // call either the native client or the main-turn quota fallback.
        let native_calls = Arc::new(AtomicUsize::new(0));
        let quota_fallback_calls = Arc::new(AtomicUsize::new(0));
        let first_calls = Arc::new(AtomicUsize::new(0));
        let same_provider_calls = Arc::new(AtomicUsize::new(0));
        let next_provider_calls = Arc::new(AtomicUsize::new(0));
        let mut runtime = ConversationRuntime::new(
            Session::new(),
            NoopApiClient,
            StaticToolExecutor::new(),
            crate::PermissionPolicy::new(PermissionMode::WorkspaceWrite),
            vec!["system".to_string()],
        )
        .with_async_api_client(Arc::new(CountingAsyncClient {
            calls: Arc::clone(&native_calls),
            outcome: Outcome::Stop,
        }));
        runtime.set_quota_wait_band(std::time::Duration::ZERO);
        runtime.set_quota_fallback_client(Some((
            Arc::new(CountingAsyncClient {
                calls: Arc::clone(&quota_fallback_calls),
                outcome: Outcome::Stop,
            }),
            "gemini-3.5-flash".to_string(),
        )));
        runtime.set_deep_verify_candidates(vec![
            (
                Arc::new(CountingAsyncClient {
                    calls: Arc::clone(&first_calls),
                    outcome: Outcome::RateLimit,
                }),
                "claude-fable-4-5".to_string(),
            ),
            (
                Arc::new(CountingAsyncClient {
                    calls: Arc::clone(&same_provider_calls),
                    outcome: Outcome::Stop,
                }),
                "claude-opus-4-8".to_string(),
            ),
            (
                Arc::new(CountingAsyncClient {
                    calls: Arc::clone(&next_provider_calls),
                    outcome: Outcome::Stop,
                }),
                "gpt-5.6-sol".to_string(),
            ),
        ]);
        let (render_tx, mut render_rx) = mpsc::channel(64);
        let _drain = tokio::spawn(async move { while render_rx.recv().await.is_some() {} });
        let result = runtime
            .verify_subturn(
                "judge the diff".to_string(),
                Vec::new(),
                &render_tx,
                &BlockIdGen::default(),
                &prompter,
            )
            .await;
        assert!(result.is_ok(), "next-ranked verify must succeed: {result:?}");
        assert_eq!(first_calls.load(Ordering::SeqCst), 1);
        assert_eq!(same_provider_calls.load(Ordering::SeqCst), 0);
        assert_eq!(next_provider_calls.load(Ordering::SeqCst), 1);
        assert_eq!(native_calls.load(Ordering::SeqCst), 0);
        assert_eq!(quota_fallback_calls.load(Ordering::SeqCst), 0);
        assert!(runtime.active_cross_fallback.is_none());
        assert!(runtime.quota_dry_until.is_none());
        assert!(parse_lens_verifier(&runtime.last_assistant_text()).accepted);
        assert_eq!(
            runtime.deep_verify_succeeded_model_label(),
            Some("gpt-5.6-sol")
        );
        assert!(!runtime.deep_verify_leg_active);

        // If every ranked provider is rate-limited, the same-model native
        // verifier remains the final safety net and its real verdict is used.
        //
        // Re-isolate: the section above drove a real `RateLimit` through the
        // stream path, whose `on_error` hook marks this provider's cool-down in
        // the same process-global registry the walk pre-flights against.
        api::quota::isolate_rate_limit_state_for_tests();
        let native_calls = Arc::new(AtomicUsize::new(0));
        let first_calls = Arc::new(AtomicUsize::new(0));
        let second_calls = Arc::new(AtomicUsize::new(0));
        let mut runtime = ConversationRuntime::new(
            Session::new(),
            NoopApiClient,
            StaticToolExecutor::new(),
            crate::PermissionPolicy::new(PermissionMode::WorkspaceWrite),
            vec!["system".to_string()],
        )
        .with_async_api_client(Arc::new(CountingAsyncClient {
            calls: Arc::clone(&native_calls),
            outcome: Outcome::Stop,
        }));
        runtime.set_deep_verify_candidates(vec![
            (
                Arc::new(CountingAsyncClient {
                    calls: Arc::clone(&first_calls),
                    outcome: Outcome::RateLimit,
                }),
                "claude-fable-4-5".to_string(),
            ),
            (
                Arc::new(CountingAsyncClient {
                    calls: Arc::clone(&second_calls),
                    outcome: Outcome::RateLimit,
                }),
                "gpt-5.6-sol".to_string(),
            ),
        ]);
        let (render_tx, mut render_rx) = mpsc::channel(64);
        let _drain = tokio::spawn(async move { while render_rx.recv().await.is_some() {} });
        let result = runtime
            .verify_subturn(
                "judge the diff".to_string(),
                Vec::new(),
                &render_tx,
                &BlockIdGen::default(),
                &prompter,
            )
            .await;
        assert!(result.is_ok(), "native fallback must produce a verdict: {result:?}");
        assert_eq!(first_calls.load(Ordering::SeqCst), 1);
        assert_eq!(second_calls.load(Ordering::SeqCst), 1);
        assert_eq!(native_calls.load(Ordering::SeqCst), 1);
        assert!(parse_lens_verifier(&runtime.last_assistant_text()).accepted);
        assert_eq!(runtime.deep_verify_succeeded_model_label(), None);
        assert!(!runtime.deep_verify_leg_active);

        // Architect + implementer-tier session: exhausting the deep pool now
        // continues on the main model instead of failing closed. A same-model
        // check is weaker than cross-model, but with every deep-tier verifier
        // rate-limited the alternative was no verification at all — the leg used
        // to stop with `objective ok; verifier timed out`. The note names which
        // one ran so the verdict is never mistaken for a cross-model pass.
        //
        // Re-isolate for the same reason as the section above.
        api::quota::isolate_rate_limit_state_for_tests();
        let native_calls = Arc::new(AtomicUsize::new(0));
        let deep_calls = Arc::new(AtomicUsize::new(0));
        let mut runtime = ConversationRuntime::new(
            Session::new(),
            NoopApiClient,
            StaticToolExecutor::new(),
            crate::PermissionPolicy::new(PermissionMode::WorkspaceWrite),
            vec!["system".to_string()],
        )
        .with_async_api_client(Arc::new(CountingAsyncClient {
            calls: Arc::clone(&native_calls),
            outcome: Outcome::Stop,
        }));
        runtime.set_context_model("gpt-5.6-terra");
        runtime.set_deep_tier_only(true);
        runtime.set_deep_verify_candidates(vec![(
            Arc::new(CountingAsyncClient {
                calls: Arc::clone(&deep_calls),
                outcome: Outcome::RateLimit,
            }),
            "claude-fable-5-1".to_string(),
        )]);
        let (render_tx, mut render_rx) = mpsc::channel(64);
        let _drain = tokio::spawn(async move { while render_rx.recv().await.is_some() {} });
        let result = runtime
            .verify_subturn(
                "judge the diff".to_string(),
                Vec::new(),
                &render_tx,
                &BlockIdGen::default(),
                &prompter,
            )
            .await;
        assert!(
            result.is_ok(),
            "an exhausted deep pool must still produce a verdict: {result:?}"
        );
        assert_eq!(
            deep_calls.load(Ordering::SeqCst),
            1,
            "the deep-tier candidate is still attempted first"
        );
        assert_eq!(
            native_calls.load(Ordering::SeqCst),
            1,
            "the native fallback carries VERIFY once the deep pool is exhausted"
        );
    }

    #[test]
    fn reactive_verify_uses_same_read_only_allowlist_as_plan_first() {
        // Fix A routes the reactive VERIFY through `deep_subturn(.., ReadOnly, ..)`
        // exactly like plan-first, so the verifier inspects but never mutates.
        // The end-to-end ReadOnly downgrade is exercised by the live integration
        // test; here we pin the invariant that downgrade relies on — the scoped
        // grant `deep_subturn` injects unblocks read-only inspection while every
        // write-class tool stays denied under ReadOnly.
        use crate::permissions::PermissionOutcome;

        let mut policy = crate::PermissionPolicy::new(crate::PermissionMode::ReadOnly)
            .with_tool_requirement("bash", crate::PermissionMode::DangerFullAccess)
            .with_tool_requirement("write_file", crate::PermissionMode::WorkspaceWrite)
            .with_tool_requirement("edit_file", crate::PermissionMode::WorkspaceWrite);
        let grant = policy.add_temporary_allow_rules(read_only_bash_allow_rules());

        // Read-only inspection (git diff) the verifier needs is permitted by the
        // scoped grant `deep_subturn` injects…
        assert_eq!(
            policy.authorize("bash", r#"{"command":"git diff"}"#, None),
            PermissionOutcome::Allow
        );
        // …but the verifier can never edit or delete files.
        for tool in ["write_file", "edit_file"] {
            assert!(
                matches!(
                    policy.authorize(tool, r#"{"path":"a.rs","content":"x"}"#, None),
                    PermissionOutcome::Deny { .. }
                ),
                "{tool} must stay denied during the read-only VERIFY phase"
            );
        }
        policy.remove_temporary_allow_rules(grant);
    }

    /// The objective check must run off the async task (via `spawn_blocking`) so
    /// it never freezes the host's `select!` event loop. We can't easily assert
    /// loop-liveness in this shared unit binary (other tests mutate the global
    /// cwd, racing the subprocess), so the end-to-end non-starvation property is
    /// covered by the isolated integration test
    /// `tests/deep_gate_live.rs::reactive_check_does_not_starve_the_render_loop`.
    /// Here we only confirm the helper still computes the right verdict when run
    /// on the blocking pool.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn command_is_green_runs_on_blocking_pool() {
        assert!(command_is_green("true").await, "`true` exits 0 ⇒ green");
        assert!(
            !command_is_green("false").await,
            "`false` exits 1 ⇒ not green"
        );
        let observed = run_check_command("printf check-output-marker").await;
        assert!(observed.green);
        assert!(observed.output_tail.contains("check-output-marker"));
    }

    /// The deep-gate computes `changed_files_snapshot` on every edit-making
    /// attempt (baseline + after, plus the `TurnEnd` hook context) — twice per
    /// attempt, regardless of whether an objective `check_command` is set. It
    /// spawns a blocking `git diff`, so before the fix it ran synchronously on
    /// the host `select!` task and froze the spinner/stream mid-turn on a large
    /// or index-locked working tree (the reported "도구 사용 중 멈춤"). The async
    /// wrapper must run it off-thread via `spawn_blocking` so the await yields
    /// and the event loop stays live, exactly like `command_is_green` above.
    ///
    /// We can't assert loop-liveness deterministically in this shared unit
    /// binary (other tests mutate the global cwd, racing the subprocess), and a
    /// PATH-shimmed slow `git` would require mutating process-global env in a
    /// multi-threaded test binary — exactly the flakiness this file avoids. The
    /// off-thread guarantee instead rests on two robust facts: (1) every
    /// deep-gate call site now awaits this async wrapper (no sync
    /// `changed_files_snapshot()` remains on an async path — see the call sites
    /// in `run_auto_turn_streaming` / `run_deep_turn_streaming` / the Stop loop),
    /// and (2) the wrapper delegates to `tokio::task::spawn_blocking`, the same
    /// offload `command_is_green` uses (covered live by
    /// `tests/deep_gate_live.rs::reactive_check_does_not_starve_the_render_loop`).
    /// Here we confirm the helper still returns and never panics when driven on
    /// the runtime (a panicking/cancelled blocking task degrades to an empty
    /// snapshot rather than propagating into the turn loop).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn changed_files_snapshot_async_runs_on_blocking_pool() {
        // Whatever the ambient repo state, the call must resolve to a Vec (never
        // hang, never panic) when awaited from the async context.
        let _snapshot: Vec<String> = changed_files_snapshot_async().await;
    }
}
