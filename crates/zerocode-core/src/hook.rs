//! What an agent CLI reports back to ZeroCode.
//!
//! Agent CLIs already have a hook mechanism (a script invoked on lifecycle
//! events, fed JSON on stdin). ZeroCode installs one small script per agent that
//! forwards that payload to a loopback bridge, which is how a PTY-hosted agent
//! reports "I finished", "I need permission", "I changed the title" without
//! ZeroCode having to scrape its terminal output.
//!
//! `payload` stays an opaque string on purpose: it is the agent's own schema,
//! it differs per vendor, and it changes without notice. Parsing it is the
//! bridge's problem, not the envelope's.

use serde::{Deserialize, Serialize};

/// The environment variable every ZeroCode pane carries naming its own pane
/// (`term-<n>`). One spelling: the hook bridge's launch env, the browser shim
/// and the window's pane-key parser all read this constant.
pub const PANE_KEY_ENV: &str = "ZEROCODE_PANE_KEY";

use crate::agent::AgentKind;
use crate::payload::HookPayload;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookEnvelope {
    /// Which agent sent this. Taken from the URL path segment, not from the
    /// body, so a payload cannot claim to be another agent.
    pub agent: AgentKind,
    /// Durable pane identity, `"<tab_id>/<leaf_id>"`.
    pub pane_key: String,
    #[serde(default)]
    pub tab_id: String,
    /// Nonce minted when the lane launched. A late hook from a previous
    /// incarnation of the same pane carries a stale token and is dropped.
    #[serde(default)]
    pub launch_token: String,
    #[serde(default)]
    pub worktree_id: String,
    /// Which environment the agent believes it is running in (free-form vendor
    /// string).
    #[serde(default)]
    pub env: String,
    /// Hook script contract version, so an old script left on disk by a previous
    /// install is recognizable.
    #[serde(default)]
    pub version: String,
    /// The event's name when the agent's own payload does not carry it.
    ///
    /// Most agents put the event name inside the payload JSON; Copilot and
    /// Antigravity run ONE script for many events and tell it which through an
    /// environment variable, so the script forwards the name beside the payload
    /// (Orca's `hookEventName` form field does the same job).
    #[serde(default)]
    pub hook_event_name: String,
    /// The agent's own hook payload, verbatim.
    pub payload: String,
}

/// What a hook event says about the pane, reduced to the facts the window can
/// draw.
///
/// Orca runs a normalizer per agent, hundreds of lines each, because it also
/// extracts prompts, subagent rosters and session identities. This mapping is
/// deliberately only the STATE — which is the part a person can see on a tab —
/// and it is written against the event names this window actually installs
/// (docs/reverse/orca-ui-inventory.md 1-ba). Everything else in the payload
/// stays unread until a feature needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HookState {
    /// The agent took a prompt or a tool and is doing something.
    Working,
    /// The agent stopped to ask, and the question is sitting in a pane that
    /// may be behind another tab — the whole reason this bridge exists.
    NeedsAttention,
    /// The agent finished its turn.
    Done,
    /// There is no agent in this pane any more.
    ///
    /// **The one state no vendor ever sends** — [`hook_state`] cannot produce
    /// it, and that is deliberate: it is not a thing an agent says, it is a
    /// thing this window OBSERVES once the agent has stopped being able to say
    /// anything. A pane whose shell outlived its agent (somebody quit `codex`
    /// back to their prompt) has no `Stop` coming and never will, so without a
    /// word for "gone" its last `Working` stood forever and the sidebar kept
    /// reporting an agent that had left.
    ///
    /// Distinct from [`Self::Done`] on purpose, and the distinction is a
    /// notification: `Done` means "the turn ended, come read the answer" and
    /// earns a ring ([`crate::notify::ring_of`]); this means "nothing is
    /// running here", which is not news to the person who just quit it.
    Idle,
}

/// A subagent's own lifecycle, which is NOT its pane's state.
///
/// An agent that spawns background helpers does not call `tmux` and does not
/// get a split pane — Orca is the same, and shows those helpers as ROWS under
/// the agent that started them. So the five `@arch`/`@shell`/`@ui` workers a
/// person can see in their transcript had nowhere at all to appear here: their
/// events arrived, and `hook_state` correctly answered `None` for them,
/// because a helper starting must not repaint its parent's pane as anything.
///
/// This is the other question about the same event: not "what is the pane
/// doing" but "who is running underneath it".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubagentStep {
    Start,
    Stop,
}

/// One event name reduced to the word both tables below are keyed on:
/// lowercased, with every separator dropped.
///
/// The same fact arrives spelled `PreToolUse`, `preToolUse` and `pre_tool_use`
/// depending on the vendor, so the spelling has to stop mattering exactly
/// once. It matters that this is ONE function and not two: the two tables
/// hang on the near-miss between `stop` and `subagentstop`, and two copies of
/// this rule are two chances for one of them to start folding a separator the
/// other keeps.
fn normalized_event(event_name: &str) -> String {
    event_name
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase()
}

/// Whether this event is a subagent starting or stopping.
///
/// Deliberately separate from [`hook_state`] and deliberately overlapping with
/// nothing it answers: `SubagentStop` normalises to `subagentstop`, which is
/// not `stop`, so neither of these events has ever reached the pane's state
/// and neither should. The lowercase `subagentStart` spelling is one Claude
/// actually writes (see `zerocode-hookd`'s installer), and
/// `normalized_event` is what makes both arrive at the same answer.
#[must_use]
pub fn subagent_step(event_name: &str) -> Option<SubagentStep> {
    match normalized_event(event_name).as_str() {
        "subagentstart" => Some(SubagentStep::Start),
        "subagentstop" => Some(SubagentStep::Stop),
        _ => None,
    }
}

/// What the helper is called, and which helper it is.
///
/// Vendors spell both several ways and none of them is guaranteed, so the
/// candidates are tried in order and the first non-empty one wins. The id
/// falls back to the NAME because a stop has to be able to find the row a
/// start made: a vendor that reports neither would otherwise leave a row
/// standing forever, and matching on the name is the weaker answer that still
/// clears it.
#[must_use]
pub fn subagent_in_payload(payload: &str) -> (Option<String>, Option<String>) {
    subagent_in_parsed(&HookPayload::of(payload))
}

/// [`subagent_in_payload`], for a caller that already paid the parse.
///
/// The payload is carried as TEXT through the bridge on purpose — it is the
/// vendor's own schema — so every reader still interprets it for itself; what
/// the per-event path stopped repeating is the parse
/// ([`crate::payload::HookPayload`]). A payload that is not an object simply
/// answers nothing.
#[must_use]
pub fn subagent_in_parsed(payload: &HookPayload<'_>) -> (Option<String>, Option<String>) {
    let parsed = payload.tree_or_null();
    let read = |keys: &[&str]| -> Option<String> {
        for key in keys {
            let found = parsed.get(*key).and_then(serde_json::Value::as_str);
            if let Some(text) = found {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
        None
    };
    let name = read(&[
        "subagent_type",
        "subagentType",
        "agent_type",
        "agentType",
        "subagent_name",
        "subagentName",
        "name",
    ]);
    let id = read(&[
        "subagent_id",
        "subagentId",
        "agent_id",
        "agentId",
        "task_id",
        "taskId",
    ])
    .or_else(|| name.clone());
    (name, id)
}

/// One agent-typed entry of the `background_tasks` array Claude attaches to
/// its Stop and SubagentStop payloads.
///
/// This array is the only road a BACKGROUND Agent-tool helper's existence is
/// ever reported on: launching one fires no `SubagentStart` at all — measured
/// live 2026-08-18, where a synthetic start through the installed hook drew a
/// sidebar row while two real background scouts drew none. Orca reads the
/// same array (`claude-background-task-inventory.ts`) and folds it into the
/// roster its explicit lifecycle events feed; this is that reader, to the
/// letter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundAgentTask {
    pub id: String,
    /// The vendor's word for what KIND of helper — `Explore`,
    /// `general-purpose` — which is also the best on-screen name it offers.
    pub agent_type: Option<String>,
    pub description: Option<String>,
    /// Still going, judged from `status`: absent or unrecognised counts as
    /// running (fail-active, Orca's own rule — only an explicit terminal
    /// word may retire work), the terminal set below ends it.
    pub running: bool,
    /// A `type: "teammate"` entry — ids that never match a lifecycle stop and
    /// report running forever, so folds must not treat them as helpers.
    pub teammate: bool,
}

/// What one payload said about the background tasks, when it said anything at
/// all. `None` from the reader means the FIELD was absent — an older CLI —
/// and a caller must keep whatever roster it has rather than clearing it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundTasksReading {
    pub tasks: Vec<BackgroundAgentTask>,
    /// The list could not prove absence — an entry was malformed, unnamed, or
    /// the cap was passed — so a tracked row this list does not name must be
    /// KEPT, never deleted on the strength of an incomplete roll call.
    pub truncated: bool,
}

/// The words that mean a background task has ENDED — Orca's
/// `CLAUDE_TERMINAL_BACKGROUND_TASK_STATUSES`, verbatim.
const BACKGROUND_TASK_TERMINAL: [&str; 19] = [
    "idle",
    "done",
    "success",
    "succeeded",
    "complete",
    "completed",
    "finished",
    "failed",
    "error",
    "terminated",
    "exited",
    "aborted",
    "expired",
    "skipped",
    "crashed",
    "killed",
    "cancelled",
    "canceled",
    "timed_out",
];

/// The most helpers one payload may name — Orca's
/// `AGENT_STATUS_MAX_SUBAGENTS`. Entries past it are dropped and the reading
/// marked truncated, exactly as theirs is.
pub const MAX_BACKGROUND_SUBAGENTS: usize = 32;

/// Read the agent-typed entries of a payload's `background_tasks` field.
#[must_use]
pub fn background_agent_tasks(payload: &str) -> Option<BackgroundTasksReading> {
    background_agent_tasks_parsed(&HookPayload::of(payload))
}

/// [`background_agent_tasks`], for a caller that already paid the parse.
#[must_use]
pub fn background_agent_tasks_parsed(payload: &HookPayload<'_>) -> Option<BackgroundTasksReading> {
    let raw = payload.tree()?.get("background_tasks")?.as_array()?;
    let mut tasks: Vec<BackgroundAgentTask> = Vec::new();
    let mut truncated = false;
    let word = |value: Option<&serde_json::Value>| -> Option<String> {
        let text = value?.as_str()?.trim();
        (!text.is_empty()).then(|| text.to_string())
    };
    for item in raw {
        let Some(entry) = item.as_object() else {
            truncated = true;
            continue;
        };
        let Some(kind) = word(entry.get("type")).map(|kind| kind.to_ascii_lowercase()) else {
            truncated = true;
            continue;
        };
        if kind != "subagent" && kind != "teammate" {
            continue;
        }
        let Some(id) = word(entry.get("id")) else {
            truncated = true;
            continue;
        };
        if tasks.len() >= MAX_BACKGROUND_SUBAGENTS {
            truncated = true;
            continue;
        }
        let status = word(entry.get("status")).map(|status| status.to_ascii_lowercase());
        let running = !status
            .as_deref()
            .is_some_and(|status| BACKGROUND_TASK_TERMINAL.contains(&status));
        tasks.push(BackgroundAgentTask {
            id,
            agent_type: word(entry.get("agent_type")),
            description: word(entry.get("description")),
            running,
            teammate: kind == "teammate",
        });
    }
    Some(BackgroundTasksReading { tasks, truncated })
}

/// A tool whose "use" is a question to a PERSON — Orca's
/// `isAskUserQuestionTool`: fold away everything but letters and digits and
/// match the two spellings vendors use for the structured option prompt.
///
/// This answers "is this event a question?"; what the question SAYS stays
/// [`crate::ask::prompt_in_payload`]'s job, which reads the shape and not the
/// name. Two different questions, one speller each.
#[must_use]
pub fn is_ask_user_question_tool(tool_name: &str) -> bool {
    let folded: String = tool_name
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase();
    folded == "askuserquestion" || folded == "requestuserinput"
}

/// The tool a hook event names, when it names one. Both spellings, measured.
fn tool_in_parsed(payload: &HookPayload<'_>) -> Option<String> {
    let parsed = payload.tree()?;
    for key in ["tool_name", "toolName"] {
        if let Some(text) = parsed.get(key).and_then(serde_json::Value::as_str) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

/// Claude's `SessionStart`, reduced to whether it may land as a session
/// boundary — an idle `done` row that is nobody's completion.
///
/// The only signal a resumed or idle session emits before its first prompt
/// (Orca STA-3386, agent-hook-listener.ts:2876-2903): without it a reopened
/// claude wore a phantom "working" spinner until somebody typed. Fail
/// closed, all three ways Orca does — only claude (devin/grok/droid DROP
/// their SessionStart, because landing it as working painted the spinner
/// this exists to avoid), only the three idle sources (a compact restart
/// fires mid-turn), and never child-attributed (a helper's start must not
/// flip the lead's live turn to idle).
#[must_use]
pub fn session_boundary(agent: AgentKind, event_name: &str, payload: &str) -> bool {
    session_boundary_parsed(agent, event_name, &HookPayload::of(payload))
}

/// [`session_boundary`], for a caller that already paid the parse.
#[must_use]
pub fn session_boundary_parsed(
    agent: AgentKind,
    event_name: &str,
    payload: &HookPayload<'_>,
) -> bool {
    // Claude's contract, and this project's own harness, which speaks it
    // natively: a `SessionStart` from `zo` is the boundary that lets its
    // later `Stop`/`SessionEnd` read as the turn ending rather than as a
    // stranger's report.
    if !matches!(agent, AgentKind::Claude | AgentKind::Zo)
        || normalized_event(event_name) != "sessionstart"
    {
        return false;
    }
    let Some(parsed) = payload.tree() else {
        return false;
    };
    if child_attributed(parsed) {
        return false;
    }
    matches!(
        parsed.get("source").and_then(serde_json::Value::as_str),
        Some("startup" | "resume" | "clear")
    )
}

/// Whether a lead's `done` is held at `working` by what is still running under
/// it — and the asymmetry that an interrupt introduces.
///
/// Orca's whole answer is one expression (`resolveClaudePaneState`,
/// `agent-hook-listener.ts:2641-2655`):
///
/// ```text
/// claudeRosterHasWorkingSubagent(roster) ||
///   (!lead.interrupted && (runningNonAgentTask || activeSessionCron))
/// ```
///
/// The two halves answer to an interrupt differently, and that is the contract
/// rather than an accident:
///
/// - **A helper is unconditional.** Ctrl+C reaches the lead's foreground, not a
///   child running under it, so a live child keeps the pane at `working`
///   whether or not somebody pressed a key.
/// - **The inventory bends.** Background tasks and session crons are the LEAD's
///   own account of what it had going, and an interrupted lead's account is
///   stale the moment it is interrupted. Orca says this twice for safety — it
///   also deletes the registry entry when the flag is set
///   (`updateClaudeRunningNonAgentTask`, `:2628-2638`).
///
/// The decision is here, pure, for the reason [`crate::notify::LaneBell`] gives
/// in its own words: a rule that only a text gate can check is a rule that
/// survives being re-spelled into something else.
#[must_use]
pub fn done_is_held(roster_busy: bool, inventory_busy: bool, interrupted: bool) -> bool {
    roster_busy || (!interrupted && inventory_busy)
}

/// Whether a stop gesture leaves running work it could not have stopped, so no
/// interrupt may be inferred from it.
///
/// The SAME two facts [`done_is_held`] weighs, and the answer is different —
/// which is exactly why this is a second function and not a call to that one.
/// There, the inventory bends on the flag: an interrupted lead's account of
/// what it had going is stale the moment it is interrupted. **Here there is no
/// flag to bend on, because this is the judgement that would set it.** So both
/// halves are unconditional (`server.ts:954`, `:958-963`), and Orca states them
/// as two separate refusals rather than one expression:
///
/// - a non-idle child. Ctrl+C reaches the lead's foreground, not a child under
///   it, so synthesizing a `done` would retire live rows.
/// - the lead's own background work or session cron. "Escape/Ctrl+C at Claude's
///   idle prompt does not stop provider-owned shells or session crons."
///
/// That doc-comment on `done_is_held` already warned the next reader off
/// reusing it here, and it named one reason — the breadth of our roster proxy.
/// This is the other, and it is the load-bearing one: the two gates ask the
/// same inputs at different moments of the same story.
///
/// Our roster proxy asks "is the roster non-empty" where Orca asks "is any
/// child non-idle", and here that breadth errs SAFE: a gesture refused because
/// only idle children remain is an interrupt not inferred, never a `done`
/// invented.
#[must_use]
pub fn gesture_leaves_work_running(roster_busy: bool, inventory_busy: bool) -> bool {
    roster_busy || inventory_busy
}

/// What a pane was before a HELPER's wait displaced it.
///
/// The two facts a restored word needs in order to be the same word again: the
/// state, and whether that state was a person's doing. Orca's stash carries a
/// third — `turnCompletedAt` (`agent-hook-listener.ts:3110-3117`), the end time
/// it keeps so a later drain still belongs to the turn that ended — and ours
/// does not, because nothing in this window computes a per-turn end time yet
/// (`:3128-3134`). The cost is narrow and belongs here rather than in a plan: a
/// `done` restored through this stash comes back without the moment it
/// finished, so it dates from the restore instead. The day that time exists,
/// this struct is where it lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BeforeWait {
    pub state: HookState,
    /// Carried, never re-derived. The flag is already clamped to `done` on the
    /// row it came from ([`done_provenance`]), and asking the payload again
    /// would be asking about a different event than the one that earned it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub interrupted: bool,
}

/// What a pane should stash when this event lands on it, if anything.
///
/// A helper's wait DISPLACES the lead. The pane reads `needs-attention` because
/// a child stopped to ask for something, and the lead — which may have finished
/// its turn already — loses its own word to the child's. Nothing gives it back:
/// the lead is not going to send another event just because somebody answered
/// its helper. So the displaced word is kept at the moment it is displaced and
/// put back when the wait clears (`stateBeforeWait`,
/// `agent-hook-listener.ts:3105-3117`).
///
/// Three conditions, all of them Orca's: the incoming state is the wait
/// (`isWaitingInducing`), the event is a child's (`eventAgentId`), and there is
/// something to displace (`previousLead`). **A lead's own wait stashes
/// nothing**, and that is not an omission — an agent that stops to ask its own
/// question has been displaced by nobody, so there is nothing behind its
/// `needs-attention` except the asking. That absence is the whole reason the
/// restore has a default.
///
/// A SECOND child's wait carries the FIRST one's stash instead of stashing the
/// intermediate `needs-attention` (`:3107-3108`). Stashing the wait would make
/// the restore restore a wait, and the pane would have no way back out.
///
/// Answering `None` is the CLEARING, not an abstention: the caller rebuilds the
/// row from this answer, so any event that is not a helper's wait drops a stash
/// the row was holding — which is right, because that event gave the pane a
/// word of its own again.
#[must_use]
pub fn wait_stash(
    incoming: HookState,
    by_helper: bool,
    prior: Option<BeforeWait>,
    prior_stash: Option<BeforeWait>,
) -> Option<BeforeWait> {
    if incoming != HookState::NeedsAttention || !by_helper {
        return None;
    }
    let prior = prior?;
    if prior.state == HookState::NeedsAttention {
        return prior_stash;
    }
    Some(prior)
}

/// What a pane goes back to when its wait is ANSWERED
/// (`clearClaudeAnsweredQuestionWait`, `agent-hook-listener.ts:2853-2857`).
///
/// Answering an `AskUserQuestion` emits no hook event — the agent simply
/// resumes — so the submit keystroke is the only signal there will ever be,
/// and something has to say what the pane becomes. Not `done`: the turn did
/// not end, the person unblocked it.
///
/// **`Working` is the FALLBACK, not the answer.** Two of the three branches
/// reach it — no stash, or a pane that was not waiting — and reading
/// `?? { state: 'working' }` as the outcome loses the third: a lead that had
/// already FINISHED when a helper's wait displaced its word
/// ([`wait_stash`]) comes back finished, carrying whether a person had ended
/// it. Collapse that and answering a helper's permission prompt resurrects a
/// turn that is over, and nothing will correct it — the lead that would have
/// sent the correcting event is the one that already stopped.
///
/// The word this returns is not yet the word the pane SHOWS: Orca runs it back
/// through the same hold gate a `done` normally passes (`:2871`), which is
/// [`done_is_held`] here. That belongs to the caller, which holds the roster
/// and the inventory this one does not.
///
/// **Orca asks "was it waiting?" once more inside this expression and we do
/// not, because that question has two answers there and one here.** Its caller
/// gates on the emitted status (`server.ts:1017`) and this line re-asks the
/// listener's own lead record — two records that can disagree. Ours is one
/// row, so the re-ask would compare a fact with itself. The caller's gate is
/// therefore the only gate, and this answers only what a stash restores to.
#[must_use]
pub fn answered_restore(stash: Option<BeforeWait>) -> BeforeWait {
    stash.unwrap_or(BeforeWait {
        state: HookState::Working,
        interrupted: false,
    })
}

/// A done-provenance flag, refused on any state that is not `Done`.
///
/// Both `session_boundary` and `interrupted` say the same kind of thing — "this
/// `done` is not a completion" — so neither means anything on a row that is not
/// done, and Orca clamps at exactly this point
/// (`agent-status-types.ts:425`: `interrupted === true && state === 'done'`).
///
/// Applied on the WRITE and on the READ, because a file on disk can carry a
/// contradiction a live event never could: an older build, a hand-edited
/// ledger, or bytes half-written before a kill.
#[must_use]
pub fn done_provenance(state: HookState, flag: bool) -> bool {
    flag && state == HookState::Done
}

/// Whether this event is a HELPER's rather than the lead's.
///
/// Orca's whole vocabulary for this is one field read two ways
/// (`eventAgentId`), and three different facts answer to a child-attributed
/// event for three different reasons — a helper's `SessionStart` must not flip
/// the lead's live turn to idle, a helper's `Stop` must not declare the lead's
/// turn interrupted, and a helper's WAIT displaces a lead word that has to be
/// kept ([`wait_stash`]). One reader, so the three never disagree about what a
/// child is.
fn child_attributed(parsed: &serde_json::Value) -> bool {
    ["agent_id", "agentId"].iter().any(|key| {
        parsed
            .get(key)
            .and_then(serde_json::Value::as_str)
            .is_some_and(|id| !id.trim().is_empty())
    })
}

/// `child_attributed`, asked of a payload nobody has parsed yet.
///
/// This is a door and not a second opinion. The two facts inside this file that
/// need the answer already hold a parsed document and keep asking it directly;
/// the reader one layer out holds the envelope's bytes, like every other
/// `*_in_payload` here. A payload that will not parse is not a helper's — the
/// same silence every reader in this module answers with.
#[must_use]
pub fn helper_attributed(payload: &str) -> bool {
    helper_attributed_parsed(&HookPayload::of(payload))
}

/// [`helper_attributed`], for a caller that already paid the parse.
#[must_use]
pub fn helper_attributed_parsed(payload: &HookPayload<'_>) -> bool {
    payload.tree().is_some_and(child_attributed)
}

/// Whether this event declares the turn INTERRUPTED — the person stopped the
/// agent rather than the agent finishing.
///
/// `carried` is the flag the pane was already holding. Only a turn boundary may
/// declare an interrupt OR carry a prior one forward; every other event starts
/// a fresh turn and DROPS it (`agent-hook-listener.ts:2960-2966`, and its
/// comment says exactly that). So a caller passes what it holds and stores what
/// it gets back — an event that is not a boundary answers `false` and the flag
/// is gone, which is the clearing rule rather than a missing one.
///
/// Five derivations, enumerated rather than generalised, because the vendors
/// genuinely disagree and a shared shape would be a fiction:
///
/// | vendor | declares on | carries |
/// |---|---|---|
/// | claude (`:2961-2966`) | `Stop`\|`StopFailure`, lead only, strict `is_interrupt == true` | yes |
/// | devin (`:3254`) | `Stop` + `is_interrupt == true` | no |
/// | kimi (`:3316`) | `Stop` + `is_interrupt == true` | no |
/// | amp (`:3495`) | `agent.end` + `status == "cancelled"` | no |
/// | cursor (`:3979`) | `stop` + a `status` string that is not `"completed"` | no |
///
/// Codex says nothing at the lead level, so it never declares here.
///
/// Claude is the only one that reads the child attribution and the only one
/// that accepts `StopFailure`, and it is the only one whose flag survives the
/// turn — which is what lets a helper's later stop re-emit a pane status that
/// still remembers the person pressed the key.
///
/// The DONE-ONLY clamp is not here. It belongs where the state and the flag
/// meet (`agent-status-types.ts:425`: `interrupted === true && state === 'done'`),
/// because this answers about the EVENT and that answers about the row.
#[must_use]
pub fn interrupt_declared(
    agent: AgentKind,
    event_name: &str,
    payload: &str,
    carried: bool,
) -> bool {
    // Names in their NORMALISED spelling — [`normalized_event`] drops
    // everything that is not alphanumeric, so amp's `agent.end` arrives here as
    // `agentend`. Written the way it arrives rather than the way the vendor
    // sends it, because the readable spelling silently never matches and a test
    // is the only thing that would ever say so.
    let event = normalized_event(event_name);
    let boundary = match agent {
        AgentKind::Claude => event == "stop" || event == "stopfailure",
        AgentKind::Devin | AgentKind::Kimi => event == "stop",
        AgentKind::Amp => event == "agentend",
        AgentKind::Cursor => event == "stop",
        _ => false,
    };
    if !boundary {
        return false;
    }
    // Claude alone carries a prior interrupt across the boundary; for everyone
    // else the payload is the only witness there is.
    if agent == AgentKind::Claude && carried {
        return true;
    }
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(payload) else {
        return false;
    };
    // STRICT `true`, the way Orca writes it (`=== true`): a truthy `1` or the
    // string `"true"` is a payload we do not understand, and guessing that it
    // means the person pressed a key would synthesize a stop nobody asked for.
    let says_interrupt = parsed
        .get("is_interrupt")
        .and_then(serde_json::Value::as_bool)
        == Some(true);
    match agent {
        AgentKind::Claude => !child_attributed(&parsed) && says_interrupt,
        AgentKind::Devin | AgentKind::Kimi => says_interrupt,
        AgentKind::Amp => {
            parsed.get("status").and_then(serde_json::Value::as_str) == Some("cancelled")
        }
        // Anything other than `completed` is a cancellation here — but it must
        // BE a string. A missing status is not an interrupt, and Orca's own
        // `typeof … === 'string'` is what says so.
        AgentKind::Cursor => parsed
            .get("status")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|status| status != "completed"),
        _ => false,
    }
}

/// Whether a turn-ending payload itself says work is still running.
///
/// Claude's `Stop` carries its inventories along — `background_tasks` and
/// `session_crons` — and a non-empty `background_tasks` means the turn ended
/// for the LEAD but not for the pane: a shell it started is still running
/// under it, and the roster fold draws that shell as a row. Orca reads both
/// inventories before letting `done` land (`resolveClaudePaneState`; the pane
/// stays working "only because background inventory is still registered",
/// its comment says).
///
/// **A divergence that stays, named.** A session cron is a SCHEDULE, not
/// running work: between two fires the agent sits at its prompt, nothing of
/// its runs, and nothing ever replays the parked all-clear — a cron has no
/// stop of its own. Held behind it, the ring spun for as long as the loop
/// lived while the person looked at an idle prompt ("멈춰있는데 프로그래스
/// 도는것도", 2026-09-02). The fire itself reports as any prompt does, so a
/// cron that is actually working still shows as working. Absent keys answer
/// `false`, which is every other vendor's payload.
#[must_use]
pub fn payload_names_live_background_work(payload: &str) -> bool {
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(payload) else {
        return false;
    };
    parsed
        .get("background_tasks")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|list| !list.is_empty())
}

/// The state a hook event puts its pane in, or `None` when the event says
/// nothing about state (a subagent starting, a compaction, a session ending).
///
/// Matched on `normalized_event`, because the same fact arrives spelled
/// `PreToolUse`, `preToolUse` and `pre_tool_use` depending on the vendor —
/// and on the payload's `tool_name` for the ONE fact the event name cannot
/// carry: a blocked question.
///
/// [`HookState::Idle`] is deliberately unreachable from here: no vendor reports
/// its own absence, and an event that could claim it would let a straggler from
/// a dead agent switch a live one off.
#[must_use]
pub fn hook_state(event_name: &str, payload: &str) -> Option<HookState> {
    hook_state_parsed(event_name, &HookPayload::of(payload))
}

/// [`hook_state`], for a caller that already paid the parse.
#[must_use]
pub fn hook_state_parsed(event_name: &str, payload: &HookPayload<'_>) -> Option<HookState> {
    // Claude emits PreToolUse while `AskUserQuestion` is BLOCKED — the tool
    // never runs; it stands waiting for a person. Newer builds spell the
    // same moment `PermissionRequest` (Orca's own comment,
    // agent-hook-listener.ts:2924-2928). Folding it into Working left the
    // pane spinning, gated the ask parser off downstream, and the question
    // card could never appear — the map's P0-3.
    if matches!(
        normalized_event(event_name).as_str(),
        "pretooluse" | "permissionrequest"
    ) && tool_in_parsed(payload).is_some_and(|tool| is_ask_user_question_tool(&tool))
    {
        return Some(HookState::NeedsAttention);
    }
    match normalized_event(event_name).as_str() {
        // The agent was handed a prompt or reached for a tool.
        "userpromptsubmit"
        | "beforesubmitprompt"
        | "pretooluse"
        | "posttooluse"
        | "posttoolusefailure"
        | "preinvocation"
        | "beforeshellexecution"
        | "beforemcpexecution" => Some(HookState::Working),
        // The plugin agents' own spelling of the same three facts. Amp reports
        // `agent.start` / `tool.call` / `tool.result` / `agent.end`; OpenCode's
        // family reports `SessionBusy` / `SessionIdle`. Reduced here rather than
        // inside each plugin, so ONE table answers "what does this mean" for
        // every road an event arrives by.
        "agentstart" | "toolcall" | "toolresult" | "sessionbusy" => Some(HookState::Working),
        // The agent stopped to ask a person.
        "permissionrequest" => Some(HookState::NeedsAttention),
        // A notification is CHATTER until its content says otherwise. Mapping
        // the bare event to attention ambered every routine banner grok and
        // droid print — the map's Notification misclassification.
        "notification" => notification_state(payload),
        // The turn ended — including the ways it ends badly, which a person
        // still needs to come look at, but the agent is no longer running.
        //
        // `TeammateIdle` is deliberately NOT here: it is a roster fact — one
        // teammate of a claude team parking itself — and folding it into
        // Done dropped the LEAD's card to the completed column (with a
        // completion ring) every time one helper of a working team went
        // quiet: the map's P0-4. Orca parks the teammate's row and re-emits
        // the lead's cached state (agent-hook-listener.ts:2870-2874); until
        // roster rows carry states of their own, saying nothing is the
        // truthful reduction.
        "stop" | "stopfailure" | "afteragentresponse" | "postinvocation" | "erroroccurred"
        | "agentend" | "sessionidle" => Some(HookState::Done),
        _ => None,
    }
}

/// Words in a notification's message that mean an agent is waiting on a
/// PERSON. The union of droid's set (`isDroidPermissionNotification`,
/// agent-hook-listener.ts:2144-2151) and grok's
/// (`isGrokPermissionNotification`, :2424-2441) — minus `confirm`, whose
/// exclusion droid states the reason for: it false-positives on benign
/// messages like "task confirmed". One table serves every road here, so the
/// word that is safe for one vendor and wrong for another stays out; a grok
/// "Confirm tool use?" without any of the other nine words falls to chatter,
/// and a missed amber is the recoverable half of that trade.
const NOTIFICATION_ASKING_WORDS: [&str; 9] = [
    "permission",
    "approve",
    "approval",
    "allow",
    "needs your",
    "requires your",
    "feedback",
    "clarify",
    "question",
];

/// Words that mean the agent has parked itself at its own prompt — droid's
/// idle set (`:2153-2159`, the only end-of-turn signal it emits on a user
/// interrupt) plus grok's (`:2464-2475`, its composer's own hint strings).
const NOTIFICATION_PARKED_WORDS: [&str; 6] = [
    "waiting for your input",
    "waiting for input",
    "type your message",
    "enter send",
    "shift-tab normal",
    "ask a side question",
];

/// What a `Notification` event's CONTENT says about the pane, when it says
/// anything (`normalizeDroidEvent` :4144-4149, `normalizeGrokEvent`
/// :4251-4267).
///
/// The default is the whole point: **`None`, not attention.** Grok emits a
/// notification before every tool even under bypassPermissions, droid narrates
/// status — and the original discards everything its ladders do not claim.
/// Claude's branch goes further and never reads notifications for state at
/// all (`normalizeClaudeEvent` :2987-2998 has no Notification row); ours turns
/// amber on claude's TYPED permission notification too, which is the same
/// screen one event sooner — the `PermissionRequest` that sets it in the
/// original arrives in the same breath.
fn notification_state(payload: &HookPayload<'_>) -> Option<HookState> {
    let parsed = payload.tree_or_null();
    let first_of = |keys: &[&str]| {
        keys.iter().find_map(|key| {
            parsed
                .get(*key)
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
        })
    };
    let kind = first_of(&["notification_type", "notificationType", "type"])
        .map(normalized_event)
        .unwrap_or_default();
    let message = first_of(&["message", "body", "text", "title"]).unwrap_or_default();
    let lowered = message.to_lowercase();

    // Grok's routine pre-tool banner: typed permission_prompt, that exact
    // sentence, info level or none. Fired before EACH tool; PreToolUse
    // already covers the progress it narrates (`:2451-2462`).
    if kind == "permissionprompt"
        && lowered == "tool permission requested"
        && first_of(&["level"]).is_none_or(|level| level.eq_ignore_ascii_case("info"))
    {
        return None;
    }
    // A vendor that TYPED the notification has answered the question itself
    // (claude's two spellings, `:2082-2083`).
    if kind == "permissionprompt" || kind == "elicitationdialog" {
        return Some(HookState::NeedsAttention);
    }
    // Asking outranks parked, as in both originals: a message carrying both
    // ("waiting for your input to approve…") is a question, not a rest.
    if NOTIFICATION_ASKING_WORDS
        .iter()
        .any(|word| lowered.contains(word))
    {
        return Some(HookState::NeedsAttention);
    }
    if NOTIFICATION_PARKED_WORDS
        .iter()
        .any(|word| lowered.contains(word))
    {
        return Some(HookState::Done);
    }
    None
}

// ------------------------------------------------------------------ activity

/// How much of a tool's target survives onto a row.
///
/// Half of what a card's own two lines get ([`crate::transcript::SUMMARY_CHARS`]
/// is 240), because this is not a sentence: it is a path, a command or a query,
/// and twenty of them are held per agent at once. A command that runs past this
/// is a command whose first hundred characters already said which one it is.
pub const ACTIVITY_TARGET_CHARS: usize = 120;

/// The two events that are not a tool at all, wearing a verb of their own.
///
/// A prompt and a turn's end belong in the same stream as the tools between
/// them — that stream is "what happened, in order", and a reader that has to
/// merge two lists to get it has been handed the merge as homework. They name
/// themselves rather than borrowing a tool's name, and no vendor tool
/// normalises to either word, so nothing collides with them.
const PROMPT_VERB: &str = "prompt";
const STOP_VERB: &str = "stop";

/// Which tool an agent reached for, reduced to the seven a row can draw.
///
/// The vendors' own names, mapped like [`hook_state`] maps their event names
/// and for the same reason: `Read`, `read_file` and `ReadFile` are one fact,
/// and a window that drew three of them would be a window that learned to
/// spell instead of a window that reads. What does not map keeps the vendor's
/// spelling in [`Tool::Other`] — an MCP tool nobody here has heard of is still
/// worth naming on the row, and inventing a category for it would be this
/// table pretending to know something it does not.
///
/// Serialized as a **plain string**, not as a tagged enum: the webview draws a
/// word per verb and falls through to the raw name for everything else, and
/// `{"other":"…"}` would make the window unwrap a shape to learn what it
/// already knows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tool {
    Read,
    Edit,
    Write,
    Bash,
    Grep,
    Task,
    Web,
    Other(String),
}

impl Tool {
    /// The word that travels, which is also the key the window localises on.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Tool::Read => "read",
            Tool::Edit => "edit",
            Tool::Write => "write",
            Tool::Bash => "bash",
            Tool::Grep => "grep",
            Tool::Task => "task",
            Tool::Web => "web",
            Tool::Other(name) => name,
        }
    }

    /// One vendor's tool name, reduced. `None` for a name that is not a name —
    /// a blank field is the vendor saying nothing, not a tool called nothing.
    #[must_use]
    pub fn named(name: &str) -> Option<Self> {
        let known = match normalized_event(name).as_str() {
            "read" | "readfile" | "readmanyfiles" | "view" | "viewfile" | "opendocument" => {
                Some(Tool::Read)
            }
            "edit" | "editfile" | "multiedit" | "strreplace" | "strreplaceeditor"
            | "applypatch" | "patch" | "replace" | "notebookedit" => Some(Tool::Edit),
            "write" | "writefile" | "createfile" | "savefile" | "newfile" => Some(Tool::Write),
            "bash" | "shell" | "localshell" | "runshellcommand" | "runcommand"
            | "runterminalcommand" | "shellcommand" | "terminal" | "exec" | "execute"
            | "executecommand" => Some(Tool::Bash),
            "grep" | "glob" | "search" | "searchfiles" | "findfiles" | "ripgrep"
            | "codebasesearch" | "filesearch" | "grepsearch" => Some(Tool::Grep),
            "task" | "agent" | "subagent" | "spawnagent" | "dispatchagent" | "delegate" => {
                Some(Tool::Task)
            }
            "websearch" | "webfetch" | "fetch" | "browse" | "browser" | "urlfetch"
            | "googlewebsearch" | "webread" | "openurl" => Some(Tool::Web),
            _ => None,
        };
        known.or_else(|| activity_text(name).map(Tool::Other))
    }
}

impl Serialize for Tool {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Tool {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        // A name that is only whitespace comes back as itself rather than as
        // nothing: this is the wire, and dropping a field on the way in would
        // make a round trip lose what it was handed.
        Ok(Tool::named(&name).unwrap_or(Tool::Other(name)))
    }
}

/// Where in its life the tool call was when this event fired.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Started,
    Finished,
    Failed,
    /// A person handed the agent something to do.
    Prompted,
    /// The turn ended, however it ended.
    Stopped,
}

/// One thing an agent did, out of the same envelope its state was read from.
///
/// The detail the bridge used to throw away. [`hook_state`] reduced an event to
/// one of three words and the rest of the payload — WHICH tool, on WHAT — was
/// dropped where it was classified, so a person supervising five agents could
/// see that all five were "working" and nothing about what any of them was
/// doing. This is that missing half, and it is deliberately three fields: a
/// verb, a target and a phase are what a row can draw at a glance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Activity {
    pub verb: Tool,
    /// The file, the command or the query — the first argument that says WHICH
    /// one, scrubbed of credentials and clamped. `None` when the payload
    /// carried no argument worth a row (an approval with no input, a turn
    /// ending), which is not a failure to parse: a verb on its own is still
    /// the truth about what is happening.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub phase: Phase,
}

/// What one event says a tool call is doing, or `None` when it is not about a
/// tool at all.
///
/// The spellings are the ones this window's own installers put on disk
/// (`zerocode-hookd`): Claude's family writes `PreToolUse`/`PostToolUse`/
/// `PostToolUseFailure`, Cursor adds
/// `beforeShellExecution`/`beforeMCPExecution`, and Amp's plugin reports
/// `tool.call`/`tool.result`.
fn tool_phase(word: &str) -> Option<Phase> {
    match word {
        "pretooluse" | "toolcall" | "beforeshellexecution" | "beforemcpexecution" => {
            Some(Phase::Started)
        }
        "posttooluse" | "toolresult" => Some(Phase::Finished),
        "posttoolusefailure" => Some(Phase::Failed),
        _ => None,
    }
}

/// The keys a vendor puts its tool's arguments under. Tried in order, first
/// present wins — the same defensive shape [`subagent_in_payload`] uses,
/// because the payload is the vendor's own schema and changes without notice.
const INPUT_KEYS: &[&str] = &[
    "tool_input",
    "toolInput",
    "input",
    "args",
    "arguments",
    "parameters",
    "params",
];

/// And the one argument inside them that says WHICH file, command or query.
///
/// Orca's `summarizeApprovalInput` picks from `command`/`file_path`/`path`/
/// `url`/`pattern` (out/main/index.js:9011) and this is that list widened by
/// the spellings the other vendors use.
///
/// Two orderings are deliberate. `pattern` and `query` come BEFORE the paths:
/// a search carries both (`{pattern, path}`) and the useful half is what is
/// being looked for, not the directory it is being looked for in — Orca's own
/// order draws every search in a repository as "crates". And `description`
/// sits ahead of `prompt`, because a delegated task carries both and the
/// description is the sentence somebody wrote to name the job while the prompt
/// is the whole briefing.
const TARGET_KEYS: &[&str] = &[
    "command",
    "cmd",
    "pattern",
    "query",
    "file_path",
    "filePath",
    "path",
    "absolute_path",
    "absolutePath",
    "notebook_path",
    "url",
    "description",
    "prompt",
];

/// One line, scrubbed and clamped, or nothing.
///
/// Credentials go first and for the reason [`crate::clone::scrub_credentials`]
/// exists: a `git clone https://user:token@host/…` typed at an agent arrives
/// here verbatim, and a row is a thing people screen-share. Whitespace is
/// collapsed because a heredoc in a shell command would otherwise draw a row
/// forty lines tall.
fn activity_text(text: &str) -> Option<String> {
    let collapsed = crate::clone::scrub_credentials(text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if collapsed.is_empty() {
        return None;
    }
    if collapsed.chars().count() <= ACTIVITY_TARGET_CHARS {
        return Some(collapsed);
    }
    let mut cut: String = collapsed.chars().take(ACTIVITY_TARGET_CHARS).collect();
    cut.push('…');
    Some(cut)
}

/// What separates a verb from its target in a composed activity line.
///
/// The window writes it (`activityLine`, ui/shell.js: `${word} · ${target}`)
/// and a vendor that composes the line itself writes the same one, so this is
/// where the sentence is taken back apart.
pub const ACTIVITY_SEPARATOR: &str = " · ";

/// A vendor's already-composed activity line, read back into an [`Activity`].
///
/// Most vendors say what they are doing as a tool EVENT and the fields are
/// read out of its payload ([`activity_of_parsed`]). zo says it as one line in
/// its `subagents` frame — `Read · src/x.rs` — because that frame is a
/// snapshot of helpers, not a stream of events. Both roads have to end in the
/// same three fields or the window would need a second way to draw the same
/// sentence.
///
/// The phase is always `Started`: a snapshot says what is happening NOW, and
/// nothing in it claims a call has finished. An unrecognised verb is kept as
/// itself ([`Tool::named`] falls through to `Other`), so a vendor's own word
/// survives to the row rather than being dropped for not being on a list.
#[must_use]
pub fn activity_said(line: &str) -> Option<Activity> {
    let line = line.trim();
    let (verb, target) = line
        .split_once(ACTIVITY_SEPARATOR)
        .map_or((line, None), |(verb, target)| (verb.trim(), Some(target)));
    Some(Activity {
        verb: Tool::named(verb)?,
        target: target.and_then(activity_text),
        phase: Phase::Started,
    })
}

/// A value's words, whether the vendor wrote a string or an argv array.
///
/// Codex and the shell tools of several others pass `["bash","-lc","cargo
/// test"]`; a reader that only understood strings would call that no target at
/// all and draw a bare verb for every command in the run.
fn value_text(value: &serde_json::Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return activity_text(text);
    }
    let parts = value.as_array()?;
    let joined = parts
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect::<Vec<_>>()
        .join(" ");
    activity_text(&joined)
}

/// The first of `keys` this record answers to.
fn first_text(record: &serde_json::Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| record.get(*key).and_then(value_text))
}

/// Whether a finished tool actually failed.
///
/// Amp has no failure EVENT — its `tool.result` carries the verdict in the
/// payload (`status`, `error`), so a stream that read only event names would
/// draw every one of its failures as a success.
fn failed_in(parsed: &serde_json::Value) -> bool {
    if parsed
        .get("error")
        .is_some_and(|error| !error.is_null() && value_text(error).is_some())
    {
        return true;
    }
    if parsed.get("success").and_then(serde_json::Value::as_bool) == Some(false) {
        return true;
    }
    ["status", "tool_status", "toolStatus"].iter().any(|key| {
        parsed
            .get(*key)
            .and_then(serde_json::Value::as_str)
            .map(str::to_ascii_lowercase)
            .is_some_and(|status| matches!(status.as_str(), "error" | "failed" | "failure"))
    })
}

/// What an agent is DOING, out of the event that said so — or `None` when the
/// event is not about doing anything.
///
/// The third question about the same envelope, beside [`hook_state`] ("what is
/// the pane") and [`subagent_step`] ("who is running underneath it"). It is a
/// third function for the reason those two are two: the answers have different
/// shapes and different lifetimes, and an event that has one of them usually
/// has nothing to say about the others.
///
/// Silent, on purpose, for: a helper starting or stopping (the roster's fact,
/// drawn as a row), a session starting, a compaction, and a permission request
/// — the ask is already a card of its own with the tool's name on it, and
/// putting it in this stream too would be the same event drawn twice.
#[must_use]
pub fn activity_of(event_name: &str, payload: &str) -> Option<Activity> {
    activity_of_parsed(event_name, &HookPayload::of(payload))
}

/// [`activity_of`], for a caller that already paid the parse.
#[must_use]
pub fn activity_of_parsed(event_name: &str, payload: &HookPayload<'_>) -> Option<Activity> {
    // A helper's lifecycle is not a tool call. Asked first because
    // `SubagentStop` would otherwise fall through to the turn-ending table
    // below and report the PANE's turn as over.
    if subagent_step(event_name).is_some() {
        return None;
    }
    let word = normalized_event(event_name);
    let parsed = payload.tree_or_null();
    if let Some(phase) = tool_phase(&word) {
        // The tool's name, however the vendor spells the field — Amp's plugin
        // writes `tool`, everyone else writes `tool_name`.
        let named = ["tool_name", "toolName", "tool", "name"]
            .iter()
            .find_map(|key| parsed.get(*key).and_then(serde_json::Value::as_str))
            .and_then(Tool::named);
        // Cursor's shell event names no tool at all — the EVENT is the name,
        // and its payload carries the command at the top level.
        let verb = named.or_else(|| (word == "beforeshellexecution").then_some(Tool::Bash))?;
        let input = INPUT_KEYS.iter().find_map(|key| parsed.get(*key));
        let target = input
            .and_then(value_text)
            .or_else(|| input.and_then(|held| first_text(held, TARGET_KEYS)))
            .or_else(|| first_text(parsed, TARGET_KEYS));
        let phase = if phase == Phase::Finished && failed_in(parsed) {
            Phase::Failed
        } else {
            phase
        };
        return Some(Activity {
            verb,
            target,
            phase,
        });
    }
    if word == "userpromptsubmit" || word == "beforesubmitprompt" {
        return Some(Activity {
            verb: Tool::Other(PROMPT_VERB.to_string()),
            target: first_text(parsed, &["prompt", "message", "user_prompt", "userPrompt"]),
            phase: Phase::Prompted,
        });
    }
    // Every way a turn ends, borrowed from the one table that already knows
    // them all rather than copied into a second list that would drift from it.
    if hook_state(event_name, "{}") == Some(HookState::Done) {
        return Some(Activity {
            verb: Tool::Other(STOP_VERB.to_string()),
            target: None,
            phase: Phase::Stopped,
        });
    }
    None
}

/// The agent CLI a shell command launches, or `None` for an ordinary command.
///
/// `codex exec "…"` under Claude's Bash tool is an agent RUNNING an agent,
/// and no hook will ever say so — codex was started as a *command*, not as a
/// helper, so there is no `SubagentStart` to draw a row from and the person
/// watching the board sees a pane "working" with a whole second agent
/// invisible inside it (reported exactly so). The command line is the only
/// witness.
///
/// Each pipeline segment's head token is read — a launch hides behind `env`,
/// assignments and a `cd … &&` often enough that skipping wrappers is the
/// difference between seeing most launches and seeing few — and the first
/// head that is a known agent binary names the worker. `zo` is excluded: it
/// is this product's own multiplexer, not an agent.
///
/// The segments are cut by a reader that knows the shell's grammar well
/// enough not to be fooled by it ([`shell_segments`]): a `|` inside quotes is
/// a regex alternation, not a pipe (`grep -E 'a|codex'` drew a Codex row —
/// navigator audit t-2607); a heredoc body is a document, not commands (a
/// markdown table `| codex | gpt-5.6-sol |` drew one too); and a `#` comment
/// is nobody's command. And a launch that only ASKS the binary about itself
/// — `claude --help`, `codex --version` — is a probe, not a run
/// ([`is_agent_probe`]): the coordinator's four probes were the four
/// "finished Claude" rows in the client's screenshot.
#[must_use]
pub fn nested_agent_of(command: &str) -> Option<crate::agent::AgentKind> {
    shell_segments(command).into_iter().find_map(|segment| {
        let mut words = segment.into_iter().skip_while(|word| {
            !word.quoted
                && (word.text.contains('=')
                    || matches!(
                        word.text.as_str(),
                        "env" | "exec" | "command" | "nohup" | "time" | "sudo"
                    ))
        });
        let head = words.next()?;
        if head.quoted {
            return None;
        }
        let name = head.text.rsplit('/').next().unwrap_or(&head.text);
        // EVERY name that means this agent on PATH, not only the primary
        // one: Cursor's CLI is `agent` now and `cursor-agent` is the legacy
        // link its installer still makes, so a person's muscle memory names
        // the same agent (`AgentSpec::detect_names`, t-6120).
        let kind = crate::agent::ALL_AGENTS
            .into_iter()
            .filter(|kind| *kind != crate::agent::AgentKind::Zo)
            .find(|kind| {
                crate::agent::agent_spec(kind.slug())
                    .is_some_and(|spec| spec.detect_names().any(|said| said == name))
            })?;
        let rest: Vec<ShellWord> = words.collect();
        if is_agent_probe(&rest) {
            return None;
        }
        Some(kind)
    })
}

/// Whether an agent invocation only asks the binary about itself.
///
/// One definition for both readers of a command line — the nested-run row
/// here and the mirror shim on the launch road (`zerocode-mirror`'s
/// `is_probe`) — so a probe is refused a row by the same rule that refuses
/// it a page and a mirror file. Only an argument that IS one of the four
/// flags counts: a prompt that merely mentions `--help` is one quoted
/// argument and matches nothing, and everything after a bare `--` is data
/// whatever it is spelled like.
#[must_use]
pub fn is_agent_probe(args: &[ShellWord]) -> bool {
    args.iter()
        .take_while(|word| word.quoted || word.text != "--")
        .any(|word| {
            !word.quoted && matches!(word.text.as_str(), "--version" | "-V" | "--help" | "-h")
        })
}

/// [`is_agent_probe`] for an argv that has already been split by a shell —
/// the mirror shim's own arguments — where every word is bare and quoting
/// is already spent.
#[must_use]
pub fn is_agent_probe_argv<S: AsRef<str>>(args: &[S]) -> bool {
    let words: Vec<ShellWord> = args
        .iter()
        .map(|arg| ShellWord {
            text: arg.as_ref().to_string(),
            quoted: false,
        })
        .collect();
    is_agent_probe(&words)
}

/// One word of a shell command, with whether any of it was quoted.
///
/// Quoting is what tells a flag from a mention: `--help` is a probe and
/// `'--help'` is a prompt about one. Kept on the word rather than resolved
/// away, because the reader that decides is not the reader that splits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellWord {
    pub text: String,
    pub quoted: bool,
}

/// A command line cut into the commands a shell would actually run, each a
/// list of words.
///
/// Enough of the shell's grammar to not be fooled by it, and no more: single
/// and double quotes and backslash escapes keep their contents inside one
/// word; `|`, `||`, `;`, `&&`, `&` and a newline end a command; `#` at the
/// start of a word begins a comment to the end of the line; and a heredoc
/// (`<<TAG`, `<<-TAG`, `<<'TAG'`, `<<"TAG"`) swallows the lines up to its
/// tag. Redirections and subshell parentheses are left as words — they are
/// never an agent's name, and reading them any further would be a parser
/// nobody asked for.
#[must_use]
pub fn shell_segments(command: &str) -> Vec<Vec<ShellWord>> {
    let mut segments: Vec<Vec<ShellWord>> = Vec::new();
    let mut words: Vec<ShellWord> = Vec::new();
    let mut text = String::new();
    let mut quoted = false;
    let mut in_word = false;
    // Heredoc tags announced on the current line, to be skipped once the
    // line ends — in the order they were announced, as the shell reads them.
    let mut heredocs: Vec<String> = Vec::new();
    let chars: Vec<char> = command.chars().collect();
    let mut at = 0;

    fn close_word(
        words: &mut Vec<ShellWord>,
        text: &mut String,
        quoted: &mut bool,
        in_word: &mut bool,
    ) {
        if *in_word {
            words.push(ShellWord {
                text: std::mem::take(text),
                quoted: *quoted,
            });
        }
        *quoted = false;
        *in_word = false;
    }
    fn close_segment(segments: &mut Vec<Vec<ShellWord>>, words: &mut Vec<ShellWord>) {
        if !words.is_empty() {
            segments.push(std::mem::take(words));
        }
    }

    while at < chars.len() {
        let c = chars[at];
        match c {
            '\'' => {
                in_word = true;
                quoted = true;
                at += 1;
                while at < chars.len() && chars[at] != '\'' {
                    text.push(chars[at]);
                    at += 1;
                }
                at += 1;
            }
            '"' => {
                in_word = true;
                quoted = true;
                at += 1;
                while at < chars.len() && chars[at] != '"' {
                    if chars[at] == '\\' && at + 1 < chars.len() {
                        at += 1;
                    }
                    text.push(chars[at]);
                    at += 1;
                }
                at += 1;
            }
            '\\' => {
                in_word = true;
                quoted = true;
                if at + 1 < chars.len() {
                    if chars[at + 1] != '\n' {
                        text.push(chars[at + 1]);
                    }
                    at += 2;
                } else {
                    at += 1;
                }
            }
            '#' if !in_word => {
                while at < chars.len() && chars[at] != '\n' {
                    at += 1;
                }
            }
            '<' if !in_word && at + 1 < chars.len() && chars[at + 1] == '<' => {
                // A heredoc: read the tag, remember it, and let the line end
                // take the body.
                at += 2;
                if at < chars.len() && chars[at] == '-' {
                    at += 1;
                }
                while at < chars.len() && chars[at] == ' ' {
                    at += 1;
                }
                let mut tag = String::new();
                while at < chars.len()
                    && !chars[at].is_whitespace()
                    && !matches!(chars[at], '|' | ';' | '&')
                {
                    if !matches!(chars[at], '\'' | '"' | '\\') {
                        tag.push(chars[at]);
                    }
                    at += 1;
                }
                if !tag.is_empty() {
                    heredocs.push(tag);
                }
            }
            '\n' => {
                close_word(&mut words, &mut text, &mut quoted, &mut in_word);
                close_segment(&mut segments, &mut words);
                at += 1;
                // The bodies of every heredoc this line announced, in order.
                for tag in heredocs.drain(..) {
                    loop {
                        let mut line = String::new();
                        while at < chars.len() && chars[at] != '\n' {
                            line.push(chars[at]);
                            at += 1;
                        }
                        let ended = at >= chars.len();
                        at += 1;
                        if line.trim_start_matches('\t') == tag || ended {
                            break;
                        }
                    }
                }
            }
            '|' | ';' | '&' => {
                close_word(&mut words, &mut text, &mut quoted, &mut in_word);
                close_segment(&mut segments, &mut words);
                at += 1;
                // `||`, `&&`, `|&`: the second character is the same cut.
                if at < chars.len() && matches!(chars[at], '|' | '&') {
                    at += 1;
                }
            }
            c if c.is_whitespace() => {
                close_word(&mut words, &mut text, &mut quoted, &mut in_word);
                at += 1;
            }
            _ => {
                in_word = true;
                text.push(c);
                at += 1;
            }
        }
    }
    close_word(&mut words, &mut text, &mut quoted, &mut in_word);
    close_segment(&mut segments, &mut words);
    segments
}

/// What a nested agent's viewer shows of the CALL: the run's id when the
/// vendor names one, and the whole command — scrubbed, newlines kept, capped
/// far above the activity row's 120 chars because this is a page, not a row.
///
/// The id keys the viewer tab, so a Started and its Finished meet at the same
/// surface; a payload without one falls back to the caller's own naming.
pub const WORKER_COMMAND_CHARS: usize = 8 * 1024;

/// And of the RESULT: whatever text the vendor's `tool_response` carried,
/// scrubbed and capped, with "was it cut" said separately — a truncated
/// answer that does not say so reads as the whole answer.
pub const WORKER_OUTPUT_CHARS: usize = 256 * 1024;

/// The model a hook event says its agent is on — `"model": "gpt-5.6-sol"`,
/// `"model": "claude-opus-5"`. Orca reads the same field and caps it the same
/// (`normalizeOptionalField(hookPayload["model"], 120)`, main index.js:10385);
/// the row it feeds is 120 chars of somebody's config, so control bytes are
/// refused the way `read_transcript` refuses them — this string reaches a DOM
/// row, not a shell, but a name that needs a control byte is not a name.
#[must_use]
pub fn model_in_payload(payload: &str) -> Option<String> {
    model_in_parsed(&HookPayload::of(payload))
}

/// [`model_in_payload`], for a caller that already paid the parse.
#[must_use]
pub fn model_in_parsed(payload: &HookPayload<'_>) -> Option<String> {
    let raw = payload.tree()?.get("model")?.as_str()?.trim();
    if raw.is_empty()
        || raw.chars().count() > 120
        || raw.chars().any(|c| (c as u32) <= 31 || c as u32 == 127)
    {
        return None;
    }
    Some(raw.to_string())
}

/// The run id a tool event carries, however the vendor spells it.
#[must_use]
pub fn worker_call_id(payload: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(payload).ok()?;
    [
        "tool_use_id",
        "toolUseId",
        "tool_call_id",
        "toolCallId",
        "call_id",
        "callId",
    ]
    .iter()
    .find_map(|key| parsed.get(*key).and_then(serde_json::Value::as_str))
    .filter(|id| !id.trim().is_empty())
    .map(str::to_string)
}

/// One page of text out of a value that may be a string, an argv array, or an
/// object holding the text under a vendor's key — shared by the command and
/// the output readers, because both face the same schema drift.
fn worker_text(value: &serde_json::Value, keys: &[&str], cap: usize) -> Option<(String, bool)> {
    let raw = match value {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(parts) => parts
            .iter()
            .filter_map(serde_json::Value::as_str)
            .collect::<Vec<_>>()
            .join(" "),
        serde_json::Value::Object(_) => {
            let inner = keys.iter().find_map(|key| value.get(*key))?;
            return worker_text(inner, keys, cap);
        }
        _ => return None,
    };
    let scrubbed = crate::clone::scrub_credentials(raw.trim());
    if scrubbed.is_empty() {
        return None;
    }
    if scrubbed.chars().count() <= cap {
        return Some((scrubbed, false));
    }
    Some((scrubbed.chars().take(cap).collect(), true))
}

/// The whole command a tool call named, for the viewer's own page.
#[must_use]
pub fn worker_command(payload: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(payload).ok()?;
    let input = INPUT_KEYS.iter().find_map(|key| parsed.get(*key))?;
    let held = input
        .get("command")
        .or_else(|| input.get("cmd"))
        .unwrap_or(input);
    worker_text(held, &["command", "cmd"], WORKER_COMMAND_CHARS).map(|(text, _)| text)
}

/// The text a finished tool call brought back, and whether the cap cut it.
#[must_use]
pub fn worker_output(payload: &str) -> Option<(String, bool)> {
    let parsed: serde_json::Value = serde_json::from_str(payload).ok()?;
    let held = [
        "tool_response",
        "toolResponse",
        "tool_result",
        "toolResult",
        "output",
        "result",
        "response",
    ]
    .iter()
    .find_map(|key| parsed.get(*key))?;
    worker_text(
        held,
        &["stdout", "output", "text", "content", "result", "stderr"],
        WORKER_OUTPUT_CHARS,
    )
}

/// The event's name out of an envelope: the side-channel field when the script
/// carried one, otherwise the payload's own `hook_event_name` — the spelling
/// Claude, Droid, Grok, Devin and Kimi all write.
pub fn envelope_event_name(envelope: &HookEnvelope) -> Option<String> {
    envelope_event_name_parsed(envelope, &HookPayload::of(&envelope.payload))
}

/// [`envelope_event_name`], for a caller that already paid the parse.
#[must_use]
pub fn envelope_event_name_parsed(
    envelope: &HookEnvelope,
    payload: &HookPayload<'_>,
) -> Option<String> {
    if !envelope.hook_event_name.trim().is_empty() {
        return Some(envelope.hook_event_name.trim().to_string());
    }
    event_name_in_record(payload.tree()?.as_object()?)
}

fn event_name_in_record(record: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
    for key in ["hook_event_name", "hookEventName", "event", "eventName"] {
        if let Some(name) = record.get(key).and_then(serde_json::Value::as_str)
            && !name.trim().is_empty()
        {
            return Some(name.trim().to_string());
        }
    }
    None
}

/// Event and session routing fields in one parse.
///
/// Prompt-submit payloads can be large. A bridge that needs both fields must
/// not call two independent JSON readers on the input path; ordinary callers
/// that need only the event keep the side-channel fast path above.
#[must_use]
pub fn envelope_event_and_session(envelope: &HookEnvelope) -> (Option<String>, Option<String>) {
    let parsed = serde_json::from_str::<serde_json::Value>(&envelope.payload).ok();
    let record = parsed.as_ref().and_then(serde_json::Value::as_object);
    let event = if envelope.hook_event_name.trim().is_empty() {
        record.and_then(event_name_in_record)
    } else {
        Some(envelope.hook_event_name.trim().to_string())
    };
    let session = record.and_then(|record| {
        ["session_id", "sessionId"]
            .into_iter()
            .find_map(|key| record.get(key).and_then(serde_json::Value::as_str))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    });
    (event, session)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The model field as vendors send it, and the shapes that are not a
    /// model name: absence, junk, a page-long string, control bytes.
    #[test]
    fn a_model_name_is_read_and_a_non_name_is_refused() {
        assert_eq!(
            model_in_payload(r#"{"model":"gpt-5.6-sol"}"#).as_deref(),
            Some("gpt-5.6-sol")
        );
        assert_eq!(
            model_in_payload(r#"{"model":"  claude-opus-5  "}"#).as_deref(),
            Some("claude-opus-5")
        );
        assert_eq!(model_in_payload(r#"{"model":""}"#), None);
        assert_eq!(model_in_payload(r#"{"state":"working"}"#), None);
        assert_eq!(model_in_payload("not json"), None);
        assert_eq!(model_in_payload("{\"model\":\"a\u{7}b\"}"), None);
        let long = format!(r#"{{"model":"{}"}}"#, "m".repeat(121));
        assert_eq!(model_in_payload(&long), None);
    }

    /// The launches an agent's shell actually wears: bare, behind env and
    /// assignments, behind a `cd … &&`, at the tail of a pipe, by absolute
    /// path. And the shapes that must NOT match: mentions that are arguments
    /// rather than commands, and this product's own multiplexer.
    #[test]
    fn a_nested_agent_is_read_from_the_command_that_launches_it() {
        use crate::agent::AgentKind;
        for (command, expected) in [
            (
                "codex exec --skip-git-repo-check \"분석해줘\"",
                Some(AgentKind::Codex),
            ),
            ("CODEX_HOME=/tmp env codex exec hi", Some(AgentKind::Codex)),
            ("cd /repo && claude -p '요약'", Some(AgentKind::Claude)),
            ("echo prompt | claude -p", Some(AgentKind::Claude)),
            ("cursor-agent run", Some(AgentKind::Cursor)),
            // The same agent under the name its docs now spell.
            ("agent run", Some(AgentKind::Cursor)),
            ("grep codex src/main.rs", None),
            ("git commit -m \"claude did this\"", None),
            ("cargo test codex", None),
            ("zo attach main", None),
            ("", None),
        ] {
            assert_eq!(nested_agent_of(command), expected, "command: {command}");
        }
    }

    /// The false positives the navigator audit (t-2607) traced to real rows:
    /// a `|` inside quotes is an alternation, not a pipe; a heredoc body is a
    /// document; a comment is nobody's command; and a launch that only asks
    /// the binary about itself is a probe, not a run. The shapes that still
    /// must match sit beside them, so the reader is held to both edges.
    #[test]
    fn quoted_pipes_heredocs_comments_and_probes_never_draw_an_agent_row() {
        use crate::agent::AgentKind;
        for (command, expected) in [
            // The audit's own rows.
            ("grep -E 'a|codex' f", None),
            ("rg 'claude|antigravity' src", None),
            (
                "ps -axo pid,command | rg 'ZeroCode|zerocode-shell|claude|antigravity|codex|zo( |$)'",
                None,
            ),
            ("ps aux | grep -E \"bin/codex|/codex \"", None),
            ("grep -E \"codex resume|codex exec\" log.txt", None),
            (
                "cat <<'EOF'\n| codex | gpt-5.6-sol | xhigh |\n| claude | opus | high |\nEOF\n",
                None,
            ),
            (
                "cat > doc.md <<EOF\nRun claude -p hi to test\nEOF\necho done",
                None,
            ),
            ("cat <<-TAG\n\tcodex exec hi\n\tTAG\nls", None),
            ("# codex exec would go here\nls", None),
            ("echo hi # claude -p later", None),
            ("claude --help", None),
            ("claude --help 2>&1 | head -80", None),
            ("which claude codex; claude --version 2>&1 | head -3", None),
            ("cd /tmp && claude agents --help", None),
            ("codex -V", None),
            ("agy -h", None),
            // What must still be seen: the audit's real runs and their kin.
            (
                "codex exec 'explain what --help does'",
                Some(AgentKind::Codex),
            ),
            ("claude -p '--help'", Some(AgentKind::Claude)),
            ("claude -p \"summarise\" -- --help", Some(AgentKind::Claude)),
            ("claude -- --version", Some(AgentKind::Claude)),
            ("echo 'a|b' | claude -p", Some(AgentKind::Claude)),
            (
                "cat <<'EOF' | claude -p\n| table |\nEOF\n",
                Some(AgentKind::Claude),
            ),
            ("ls # codex\ncodex exec hi", Some(AgentKind::Codex)),
            ("FOO=1 env claude -p hi &", Some(AgentKind::Claude)),
            // A subshell is still a launch: the parenthesis is a word the
            // reader leaves alone, and `codex` heads the segment after `&&`.
            ("(cd /repo && codex exec hi)", Some(AgentKind::Codex)),
        ] {
            assert_eq!(nested_agent_of(command), expected, "command: {command:?}");
        }
    }

    /// The shell reader itself: quotes keep a word whole and mark it, cuts
    /// fall where the shell would cut, and a heredoc's body is not commands.
    #[test]
    fn the_shell_reader_keeps_quoted_words_whole_and_marks_them() {
        let read = |command: &str| -> Vec<Vec<(String, bool)>> {
            shell_segments(command)
                .into_iter()
                .map(|segment| {
                    segment
                        .into_iter()
                        .map(|word| (word.text, word.quoted))
                        .collect()
                })
                .collect()
        };
        let word = |text: &str, quoted: bool| (text.to_string(), quoted);
        assert_eq!(
            read("grep -E 'a|b' f | wc -l"),
            vec![
                vec![
                    word("grep", false),
                    word("-E", false),
                    word("a|b", true),
                    word("f", false)
                ],
                vec![word("wc", false), word("-l", false)],
            ]
        );
        assert_eq!(
            read("echo \"a \\\" b\"; x=\"y z\" && ls"),
            vec![
                vec![word("echo", false), word("a \" b", true)],
                vec![word("x=y z", true)],
                vec![word("ls", false)],
            ]
        );
        assert_eq!(
            read("printf a\\ b"),
            vec![vec![word("printf", false), word("a b", true)]]
        );
        assert_eq!(
            read("cat <<EOF\nline one\nEOF\nls"),
            vec![vec![word("cat", false)], vec![word("ls", false)]]
        );
        assert!(is_agent_probe_argv(&["--version"]));
        assert!(is_agent_probe_argv(&["exec", "--help"]));
        assert!(!is_agent_probe_argv(&["exec", "explain what --help does"]));
        assert!(!is_agent_probe_argv(&["-p", "hi", "--", "--help"]));
        assert!(!is_agent_probe_argv::<&str>(&[]));
    }

    /// The viewer's three readers: the run id under any spelling, the whole
    /// command (scrubbed, uncut below the page cap), and the output text out
    /// of a string, an object, or a nested object — cut with the cut SAID.
    #[test]
    fn a_worker_page_reads_id_command_and_output_defensively() {
        let call = r#"{"tool_use_id":"toolu_01","tool_input":{"command":"codex exec 'https://user:token@host/repo 분석'"}}"#;
        assert_eq!(worker_call_id(call).as_deref(), Some("toolu_01"));
        let command = worker_command(call).unwrap();
        assert!(command.contains("codex exec"), "{command}");
        assert!(
            !command.contains("token"),
            "a credential survived: {command}"
        );
        // The whole command, not the activity row's 120-char clamp.
        let long = format!(
            r#"{{"tool_input":{{"command":{}}}}}"#,
            serde_json::to_string(&"x".repeat(4000)).unwrap()
        );
        assert_eq!(
            worker_command(&long).map(|text| text.chars().count()),
            Some(4000)
        );

        let done = r#"{"toolUseId":"toolu_01","tool_response":{"stdout":"분석 결과","stderr":""}}"#;
        assert_eq!(worker_call_id(done).as_deref(), Some("toolu_01"));
        assert_eq!(worker_output(done), Some(("분석 결과".to_string(), false)));
        // A plain-string response reads too, and the cap says when it cut.
        let huge = format!(
            r#"{{"output":{}}}"#,
            serde_json::to_string(&"y".repeat(WORKER_OUTPUT_CHARS + 5)).unwrap()
        );
        let (text, cut) = worker_output(&huge).unwrap();
        assert_eq!(text.chars().count(), WORKER_OUTPUT_CHARS);
        assert!(cut);
        // No response at all is no page, not an empty one.
        assert_eq!(worker_output(r#"{"tool_name":"Bash"}"#), None);
        assert_eq!(worker_call_id(r#"{"tool_use_id":"  "}"#), None);
    }

    /// The web view mirrors these names by hand. If this golden changes, the
    /// TypeScript mirror and the generated hook scripts change with it.
    #[test]
    fn envelope_field_names_are_frozen_snake_case() {
        let envelope = HookEnvelope {
            agent: AgentKind::Claude,
            pane_key: "0197c0d9-0000-7000-8000-000000000001/0197c0d9-0000-7000-8000-000000000002"
                .into(),
            tab_id: "tab-1".into(),
            launch_token: "lt-1".into(),
            worktree_id: "wt-1".into(),
            env: "cli".into(),
            version: "1".into(),
            hook_event_name: String::new(),
            payload: "{\"event\":\"stop\"}".into(),
        };
        let json = serde_json::to_string(&envelope).expect("serialize");
        assert_eq!(
            json,
            r#"{"agent":"claude","pane_key":"0197c0d9-0000-7000-8000-000000000001/0197c0d9-0000-7000-8000-000000000002","tab_id":"tab-1","launch_token":"lt-1","worktree_id":"wt-1","env":"cli","version":"1","hook_event_name":"","payload":"{\"event\":\"stop\"}"}"#
        );
    }

    /// The three states, from the spellings the installed events actually use.
    #[test]
    fn every_installed_event_name_maps_to_the_state_a_person_would_expect() {
        // One spelling per vendor family, because the same fact arrives as
        // `PreToolUse`, `preToolUse` and `pre_tool_use`.
        for working in ["UserPromptSubmit", "preToolUse", "pre_tool_use"] {
            assert_eq!(
                hook_state(working, "{}"),
                Some(HookState::Working),
                "{working}"
            );
        }
        assert_eq!(
            hook_state("PermissionRequest", "{}"),
            Some(HookState::NeedsAttention)
        );
        // A bare Notification is chatter — content decides, below.
        assert_eq!(hook_state("Notification", "{}"), None);
        for done in ["Stop", "stop", "StopFailure", "afterAgentResponse"] {
            assert_eq!(hook_state(done, "{}"), Some(HookState::Done), "{done}");
        }
        // Events that say nothing about state say nothing — a subagent
        // starting must not repaint the pane as anything.
        for silent in [
            "SubagentStart",
            "SessionStart",
            "PreCompact",
            "",
            "session.start",
        ] {
            assert_eq!(hook_state(silent, "{}"), None, "{silent:?}");
        }
        // The plugin agents' dotted spelling reaches the same three states. A
        // plugin whose events fell through to `None` would install cleanly,
        // report faithfully, and paint nothing.
        for working in ["agent.start", "tool.call", "tool.result", "SessionBusy"] {
            assert_eq!(
                hook_state(working, "{}"),
                Some(HookState::Working),
                "{working}"
            );
        }
        for done in ["agent.end", "SessionIdle"] {
            assert_eq!(hook_state(done, "{}"), Some(HookState::Done), "{done}");
        }
        // And `SessionIdle` proves the point of the next test: the one event
        // whose NAME says idle is a turn ending, not an agent leaving.
    }

    /// A notification is chatter until its content says otherwise.
    ///
    /// Grok prints one before EVERY tool even under bypassPermissions, and
    /// droid narrates status through them — mapping the bare event to
    /// attention ambered a working pane over and over (the map's
    /// Notification misclassification). The ladders are the originals':
    /// droid `:4144-4149`, grok `:4251-4267`.
    #[test]
    fn a_notification_is_read_for_what_it_says_not_that_it_spoke() {
        // Typed by the vendor: the two claude spellings are an answer.
        for typed in [
            r#"{"notification_type":"permission_prompt","message":"Bash needs approval"}"#,
            r#"{"notificationType":"elicitation_dialog"}"#,
        ] {
            assert_eq!(
                hook_state("Notification", typed),
                Some(HookState::NeedsAttention),
                "{typed}"
            );
        }
        // Grok's routine pre-tool banner: typed, that exact sentence, info
        // level or none — narration, not a question.
        for routine in [
            r#"{"type":"permission_prompt","message":"Tool permission requested"}"#,
            r#"{"type":"permission_prompt","message":"tool permission requested","level":"info"}"#,
        ] {
            assert_eq!(hook_state("Notification", routine), None, "{routine}");
        }
        // ...but the same type at a louder level is a real ask.
        assert_eq!(
            hook_state(
                "Notification",
                r#"{"type":"permission_prompt","message":"Tool permission requested","level":"warn"}"#
            ),
            Some(HookState::NeedsAttention)
        );
        // Untyped, the words decide — droid's permission phrasing amber,
        // its interrupt-idle phrasing done (its only end-of-turn signal).
        assert_eq!(
            hook_state(
                "Notification",
                r#"{"message":"Droid needs permission to run rm"}"#
            ),
            Some(HookState::NeedsAttention)
        );
        assert_eq!(
            hook_state("Notification", r#"{"message":"Waiting for your input"}"#),
            Some(HookState::Done)
        );
        // Grok's composer hints park the pane; its questions raise it.
        assert_eq!(
            hook_state("Notification", r#"{"message":"Type your message…"}"#),
            Some(HookState::Done)
        );
        assert_eq!(
            hook_state(
                "Notification",
                r#"{"message":"grok requires your feedback"}"#
            ),
            Some(HookState::NeedsAttention)
        );
        // A message carrying both is a question, not a rest.
        assert_eq!(
            hook_state(
                "Notification",
                r#"{"message":"Waiting for your input to approve this plan"}"#
            ),
            Some(HookState::NeedsAttention)
        );
        // Everything else — progress banners, greetings, "task confirmed"
        // (the word droid excludes, with its reason) — is chatter.
        for chatter in [
            r#"{"message":"Task confirmed and scheduled"}"#,
            r#"{"message":"Compacting conversation…"}"#,
            r#"{"message":""}"#,
            "{}",
            "not json",
        ] {
            assert_eq!(hook_state("Notification", chatter), None, "{chatter}");
        }
    }

    /// One teammate resting is not the lead's turn ending (the map's P0-4).
    ///
    /// `TeammateIdle` used to sit in the Done row, so a working team's card
    /// fell to the completed column — with a completion ring — every time a
    /// single helper went quiet.
    #[test]
    fn a_teammate_resting_says_nothing_about_the_pane() {
        assert_eq!(hook_state("TeammateIdle", "{}"), None);
        assert_eq!(hook_state("teammate_idle", "{}"), None);
    }

    /// Claude's `SessionStart` may land only as the idle boundary it is —
    /// gated on source, on attribution, and on the one vendor whose contract
    /// says land it at all (the map's P0-4).
    #[test]
    fn a_session_start_is_a_boundary_only_on_claudes_idle_sources() {
        for source in ["startup", "resume", "clear"] {
            let payload = format!(r#"{{"source":"{source}"}}"#);
            assert!(
                session_boundary(AgentKind::Claude, "SessionStart", &payload),
                "{source}"
            );
        }
        // A compact restart (or any unknown source) fires mid-turn: closed.
        assert!(!session_boundary(
            AgentKind::Claude,
            "SessionStart",
            r#"{"source":"compact"}"#
        ));
        assert!(!session_boundary(AgentKind::Claude, "SessionStart", "{}"));
        assert!(!session_boundary(
            AgentKind::Claude,
            "SessionStart",
            "not json"
        ));
        // A helper's start must not flip the lead's live turn to idle.
        assert!(!session_boundary(
            AgentKind::Claude,
            "SessionStart",
            r#"{"source":"resume","agent_id":"helper-1"}"#
        ));
        // Devin/grok/droid drop theirs — landing it painted the spinner
        // this whole door exists to avoid.
        assert!(!session_boundary(
            AgentKind::Devin,
            "SessionStart",
            r#"{"source":"resume"}"#
        ));
        // And no other event is a boundary, whatever its payload claims.
        assert!(!session_boundary(
            AgentKind::Claude,
            "Stop",
            r#"{"source":"resume"}"#
        ));
    }

    /// done을 붙잡는 두 절반은 인터럽트에 **다르게** 답한다.
    ///
    /// 아이는 사람이 누른 키와 무관하게 계속 돌고(Ctrl+C는 리드의 전경에
    /// 닿는다), 인벤토리는 리드 자신의 진술이라 리드가 끊긴 순간 낡는다.
    /// 이 비대칭이 없으면 인터럽트된 `Stop`이 낡은 인벤토리를 들고 영원히
    /// `working`에 주차한다 — 사람이 Ctrl+C를 눌렀는데 카드가 계속 돈다.
    #[test]
    fn a_child_holds_a_done_but_an_interrupted_leads_own_inventory_does_not() {
        // 아무것도 안 돌면 붙잡지 않는다.
        assert!(!done_is_held(false, false, false));
        assert!(!done_is_held(false, false, true));

        // 아이는 무조건 붙잡는다 — 인터럽트되어도.
        assert!(done_is_held(true, false, false));
        assert!(
            done_is_held(true, false, true),
            "Ctrl+C가 리드에 닿았다고 아이가 멈추는 것은 아니다"
        );

        // 인벤토리는 인터럽트에 굽는다 — 이 한 줄이 이 시험의 전부다.
        assert!(done_is_held(false, true, false));
        assert!(
            !done_is_held(false, true, true),
            "인터럽트된 리드의 낡은 인벤토리가 카드를 계속 돌린다"
        );

        // 둘 다면 아이가 이긴다(어느 쪽이든 붙잡히므로).
        assert!(done_is_held(true, true, true));
    }

    /// 같은 두 사실, 다른 답 — 그리고 그 이유는 **깃발이 아직 없다**는 것.
    ///
    /// done 게이트는 이미 정해진 인터럽트를 읽지만, 추론 관문은 **그 인터럽트를
    /// 정하는 중**이라 굽을 깃발이 없다. 한 함수로 묶으면 한쪽이 조용히 틀린다.
    #[test]
    fn the_inference_bends_on_nothing_because_it_has_no_flag_to_bend_on() {
        // 아무것도 안 돌면 제스처가 말이 된다.
        assert!(!gesture_leaves_work_running(false, false));
        // 어느 한쪽이라도 돌면 거절 — **둘 다 무조건**이다.
        assert!(gesture_leaves_work_running(true, false));
        assert!(gesture_leaves_work_running(false, true));
        assert!(gesture_leaves_work_running(true, true));

        // 그리고 두 판정이 **실제로 갈라지는 한 줄**을 못박는다: 인벤토리만
        // 돌고 인터럽트가 선언된 done은 통과하지만(리드의 진술이 낡았으므로),
        // 같은 인벤토리 위의 제스처는 추론을 얻지 못한다.
        assert!(!done_is_held(false, true, true));
        assert!(
            gesture_leaves_work_running(false, true),
            "두 판정이 같은 답을 하면 하나로 묶고 싶어지고, 묶는 순간 \
             인터럽트를 정하는 쪽이 있지도 않은 깃발을 읽는다"
        );
    }

    /// 도우미의 기다림이 리드의 말을 **밀어내는** 순간과, 그 말을 되돌릴 수
    /// 있게 챙기는 규칙(`stateBeforeWait`, `agent-hook-listener.ts:3105-3117`).
    ///
    /// 이 시험이 지키는 한 줄은 **두 번째 기다림**이다. 중간의
    /// `needs-attention`을 챙기면 되돌리기가 기다림을 되돌리고, 판은 거기서
    /// 나올 길이 없어진다.
    #[test]
    fn a_helpers_wait_displaces_a_lead_and_a_second_one_still_remembers_the_first() {
        let done = BeforeWait {
            state: HookState::Done,
            interrupted: true,
        };
        let working = BeforeWait {
            state: HookState::Working,
            interrupted: false,
        };
        let waiting = BeforeWait {
            state: HookState::NeedsAttention,
            interrupted: false,
        };

        // 이 함수가 있는 이유: 이미 끝낸 리드 위로 도우미의 기다림이 내려앉고,
        // 끝났다는 사실이 아이의 말에 먹히지 않는다.
        assert_eq!(
            wait_stash(HookState::NeedsAttention, true, Some(done), None),
            Some(done)
        );
        // 두 번째 도우미의 기다림은 **처음 것**을 들고 간다.
        assert_eq!(
            wait_stash(HookState::NeedsAttention, true, Some(waiting), Some(done)),
            Some(done),
            "중간의 기다림을 챙기면 되돌리기가 기다림을 되돌린다"
        );
        // 리드 자신의 질문은 아무도 밀어내지 않았다 — 챙길 것이 없고, 그것이
        // 되돌리기에 기본값이 있는 이유다.
        assert_eq!(
            wait_stash(HookState::NeedsAttention, false, Some(done), None),
            None
        );
        // 도우미의 것이어도 기다림이 아니면 밀어내는 것이 아니고,
        assert_eq!(wait_stash(HookState::Working, true, Some(done), None), None);
        // 앞선 말이 없는 판은 챙길 것이 없다.
        assert_eq!(
            wait_stash(HookState::NeedsAttention, true, None, None),
            None
        );
        // `None`은 기권이 아니라 **비움**이다: 판이 제 말을 되찾은 사건은
        // 들고 있던 기억을 떨어뜨린다.
        assert_eq!(
            wait_stash(HookState::Done, true, Some(waiting), Some(done)),
            None,
            "제 말을 되찾은 판이 낡은 기억을 계속 들고 있으면 안 된다"
        );
        // 그리고 밀려난 것이 `working`이면 `working`을 챙긴다 — 이 함수는
        // done만 다루지 않는다.
        assert_eq!(
            wait_stash(HookState::NeedsAttention, true, Some(working), None),
            Some(working)
        );
    }

    /// 답해진 기다림이 되돌리는 것 — 그리고 `working`이 **답이 아니라
    /// 기본값**이라는 것(`agent-hook-listener.ts:2853-2857`).
    ///
    /// 이 시험이 지키는 한 줄은 마지막 것이다: 도우미의 기다림이 덮기 전에
    /// 이미 끝나 있던 리드는 **끝난 채로** 돌아오고, 사람이 멈춘 것이었다는
    /// 사실도 함께 돌아온다.
    #[test]
    fn an_answered_wait_puts_back_what_was_there_and_working_is_only_the_fallback() {
        // 저장분이 없다 — 리드 자신의 질문이었고, 아무도 밀려나지 않았다.
        assert_eq!(
            answered_restore(None),
            BeforeWait {
                state: HookState::Working,
                interrupted: false
            }
        );
        // 그리고 이 함수가 존재하는 이유: 이미 끝나 있던 리드는 끝난 채로,
        // 출처까지 들고 돌아온다.
        let done = BeforeWait {
            state: HookState::Done,
            interrupted: true,
        };
        assert_eq!(
            answered_restore(Some(done)),
            done,
            "끝난 턴을 되살리면 그것을 고쳐 줄 이벤트는 오지 않는다"
        );
        // 그리고 돌아온 것이 done이면 **done의 게이트를 다시 지나야** 한다 —
        // 이 함수는 그 판정을 하지 않고, 하지 않는다는 것이 계약이다.
        assert!(done_is_held(true, false, done.interrupted));
    }

    /// 봉투의 바이트로 묻는 문 하나가 파싱한 문서로 묻는 것과 **같은 답**을
    /// 해야 한다 — 두 이름이 한 사실을 말하므로.
    #[test]
    fn the_helper_door_and_the_reader_behind_it_answer_the_same() {
        for payload in [
            r#"{"agent_id":"a-1"}"#,
            r#"{"agentId":"a-1"}"#,
            r#"{"agent_id":"  "}"#,
            r#"{"agent_id":""}"#,
            "{}",
            r#"{"agent_id":7}"#,
        ] {
            let parsed = serde_json::from_str::<serde_json::Value>(payload).expect("json");
            assert_eq!(
                helper_attributed(payload),
                child_attributed(&parsed),
                "{payload}"
            );
        }
        // 그리고 파싱되지 않는 바이트는 도우미의 것이 아니다.
        assert!(!helper_attributed("not json"));
    }

    /// 인터럽트 선언의 **다섯 파생 전수**, 그리고 claude만 특별한 세 가지:
    /// 자식 귀속을 읽는 것, `StopFailure`를 받는 것, 그리고 깃발이 턴을
    /// 넘어 **승계**되는 것(`agent-hook-listener.ts:2961-2966`,
    /// `:3254`, `:3316`, `:3495`, `:3979`).
    #[test]
    fn only_a_turn_boundary_declares_an_interrupt_and_only_claude_carries_one() {
        const SAYS: &str = r#"{"is_interrupt":true}"#;

        // claude: 두 경계 이름 모두, 그리고 리드의 것일 때만.
        for event in ["Stop", "StopFailure", "stop", "stopfailure"] {
            assert!(
                interrupt_declared(AgentKind::Claude, event, SAYS, false),
                "{event}"
            );
        }
        // 도우미의 정지는 리드의 턴을 인터럽트로 선언하지 못한다.
        assert!(!interrupt_declared(
            AgentKind::Claude,
            "Stop",
            r#"{"is_interrupt":true,"agent_id":"helper-1"}"#,
            false
        ));
        // 엄격한 `true`만이다 — 참 같은 것은 우리가 모르는 payload다.
        for shape in [
            r#"{"is_interrupt":1}"#,
            r#"{"is_interrupt":"true"}"#,
            r#"{}"#,
            "not json",
        ] {
            assert!(
                !interrupt_declared(AgentKind::Claude, "Stop", shape, false),
                "{shape}"
            );
        }

        // 승계: 경계에서는 payload가 침묵해도 들고 있던 깃발이 이긴다.
        assert!(interrupt_declared(AgentKind::Claude, "Stop", "{}", true));
        // 그리고 경계가 아닌 이벤트는 그 깃발을 **떨어뜨린다** — 이것이
        // 비우는 규칙이고, 부재가 아니다.
        assert!(!interrupt_declared(
            AgentKind::Claude,
            "PreToolUse",
            SAYS,
            true
        ));
        // 승계는 claude만의 것이다.
        assert!(!interrupt_declared(AgentKind::Devin, "Stop", "{}", true));
        assert!(!interrupt_declared(AgentKind::Kimi, "Stop", "{}", true));

        // devin·kimi: `Stop` 하나, 자식 귀속을 읽지 않는다.
        for agent in [AgentKind::Devin, AgentKind::Kimi] {
            assert!(interrupt_declared(agent, "Stop", SAYS, false));
            assert!(interrupt_declared(
                agent,
                "Stop",
                r#"{"is_interrupt":true,"agent_id":"helper-1"}"#,
                false
            ));
            // `StopFailure`는 이 둘의 경계가 아니다.
            assert!(!interrupt_declared(agent, "StopFailure", SAYS, false));
        }

        // amp: 다른 이벤트 이름, 다른 낱말. 그리고 벤더가 보내는 철자
        // (`agent.end`)와 정규화된 철자(`agentend`) **둘 다** 들어와야 한다 —
        // 이 시험이 처음 잡은 것이 정확히 그 갈림이었다: 읽기 좋은 철자로
        // 비교하면 조용히 영원히 안 맞는다.
        for name in ["agent.end", "agentend", "AGENT.END"] {
            assert!(
                interrupt_declared(AgentKind::Amp, name, r#"{"status":"cancelled"}"#, false),
                "{name}"
            );
        }
        assert!(!interrupt_declared(
            AgentKind::Amp,
            "agent.end",
            r#"{"status":"done"}"#,
            false
        ));
        assert!(!interrupt_declared(AgentKind::Amp, "Stop", SAYS, false));

        // cursor: `completed`가 아닌 모든 상태가 취소다 — 다만 **문자열이어야**
        // 한다. 없는 상태는 인터럽트가 아니다.
        for status in ["cancelled", "aborted", "error", ""] {
            let payload = format!(r#"{{"status":"{status}"}}"#);
            assert!(
                interrupt_declared(AgentKind::Cursor, "stop", &payload, false),
                "{status}"
            );
        }
        assert!(!interrupt_declared(
            AgentKind::Cursor,
            "stop",
            r#"{"status":"completed"}"#,
            false
        ));
        assert!(!interrupt_declared(AgentKind::Cursor, "stop", "{}", false));
        assert!(!interrupt_declared(
            AgentKind::Cursor,
            "stop",
            r#"{"status":7}"#,
            false
        ));

        // codex는 리드 층위에서 아무것도 보내지 않는다.
        assert!(!interrupt_declared(AgentKind::Codex, "Stop", SAYS, false));
        assert!(!interrupt_declared(AgentKind::Codex, "Stop", SAYS, true));
    }

    /// A stop that names live background work has not ended for the pane —
    /// and a schedule is not work: a cron between fires leaves the pane idle.
    #[test]
    fn a_payload_can_name_the_work_that_outlives_the_turn() {
        assert!(payload_names_live_background_work(
            r#"{"background_tasks":[{"id":"t1"}]}"#
        ));
        for silent in [
            "{}",
            r#"{"background_tasks":[]}"#,
            r#"{"session_crons":[]}"#,
            r#"{"session_crons":["0 * * * *"]}"#,
            r#"{"session_crons":[{"id":"three-waits","schedule":"*/5 * * * *"}]}"#,
            "not json",
        ] {
            assert!(!payload_names_live_background_work(silent), "{silent}");
        }
    }

    /// A blocked question is attention, not work (the map's P0-3).
    ///
    /// Claude emits `PreToolUse` while `AskUserQuestion` is blocked — newer
    /// builds spell the same moment `PermissionRequest` — and folding it into
    /// Working left the pane spinning with the ask parser gated off
    /// downstream, so the question card could never appear.
    #[test]
    fn a_blocked_question_is_attention_not_work() {
        let ask = r#"{"tool_name":"AskUserQuestion","tool_input":{"questions":[]}}"#;
        assert_eq!(
            hook_state("PreToolUse", ask),
            Some(HookState::NeedsAttention)
        );
        assert_eq!(
            hook_state("pre_tool_use", r#"{"toolName":"RequestUserInput"}"#),
            Some(HookState::NeedsAttention)
        );
        // The same event over any OTHER tool is work, exactly as before —
        // and a payload that is not an object answers nothing extra.
        assert_eq!(
            hook_state("PreToolUse", r#"{"tool_name":"Bash"}"#),
            Some(HookState::Working)
        );
        assert_eq!(
            hook_state("PreToolUse", "not json"),
            Some(HookState::Working)
        );
        // The tool's name folds the way event names do, and only the two
        // question spellings pass.
        assert!(is_ask_user_question_tool("ask_user_question"));
        assert!(is_ask_user_question_tool("RequestUserInput"));
        assert!(!is_ask_user_question_tool("Bash"));
        assert!(!is_ask_user_question_tool(""));
    }

    /// `Idle` is not on this table, and nothing may put it there.
    ///
    /// It is the state the window OBSERVES when an agent's process stops
    /// holding the terminal — the pane's shell outlived it and no `Stop` is
    /// coming. If an event name could produce it, a straggler from a dead
    /// agent could switch off the live one that replaced it in the same pane,
    /// which is exactly what the launch-token gate exists to prevent and would
    /// then be bypassable by spelling.
    #[test]
    fn no_event_a_vendor_can_send_puts_a_pane_in_the_state_only_we_observe() {
        for spelling in [
            "idle",
            "Idle",
            "AgentIdle",
            "agent.idle",
            "SessionIdle",
            "TeammateIdle",
            "exit",
            "Exit",
            "AgentExit",
            "ProcessExit",
            "foreground-returned",
        ] {
            assert_ne!(
                hook_state(spelling, "{}"),
                Some(HookState::Idle),
                "{spelling} reached the one state no vendor may claim"
            );
        }
    }

    /// The other question about the same events, and the one nothing asked.
    #[test]
    fn a_helper_is_read_off_the_events_the_pane_state_ignores() {
        for spelling in ["SubagentStart", "subagentStart", "subagent_start"] {
            assert_eq!(
                subagent_step(spelling),
                Some(SubagentStep::Start),
                "{spelling}"
            );
            // And the two answers never overlap: an event that names a helper
            // must not also repaint the pane, which is why these are two
            // functions and not one.
            assert_eq!(hook_state(spelling, "{}"), None, "{spelling}");
        }
        for spelling in ["SubagentStop", "subagent_stop"] {
            assert_eq!(
                subagent_step(spelling),
                Some(SubagentStep::Stop),
                "{spelling}"
            );
            assert_eq!(hook_state(spelling, "{}"), None, "{spelling}");
        }
        // `Stop` is the PANE's, and `SubagentStop` normalises to a different
        // word — the near-miss this whole split hangs on.
        for other in ["Stop", "stop", "PreToolUse", "", "subagent"] {
            assert_eq!(subagent_step(other), None, "{other:?}");
        }
    }

    #[test]
    fn a_helper_names_itself_however_its_vendor_spells_it() {
        let (name, id) = subagent_in_payload(r#"{"subagent_type":"arch","subagent_id":"task_01"}"#);
        assert_eq!(name.as_deref(), Some("arch"));
        assert_eq!(id.as_deref(), Some("task_01"));
        // camelCase, and the id under a different word again.
        let (name, id) = subagent_in_payload(r#"{"agentType":"shell","taskId":"t-9"}"#);
        assert_eq!(name.as_deref(), Some("shell"));
        assert_eq!(id.as_deref(), Some("t-9"));
        // No id at all: the NAME becomes it, because a start with no id can
        // still be cleared by a stop that names the same helper — and a row
        // that can never be cleared is worse than a row that is only probably
        // the right one.
        let (name, id) = subagent_in_payload(r#"{"subagent_name":"ui"}"#);
        assert_eq!(name.as_deref(), Some("ui"));
        assert_eq!(id.as_deref(), Some("ui"));
        // Blank is not an answer, and neither is a payload that is not an
        // object — the string is the vendor's own and may be anything at all.
        assert_eq!(
            subagent_in_payload(r#"{"subagent_type":"   "}"#),
            (None, None)
        );
        assert_eq!(subagent_in_payload("not json"), (None, None));
        assert_eq!(subagent_in_payload("[1,2]"), (None, None));
    }

    /// The event name is found wherever the vendor put it.
    #[test]
    fn an_event_name_is_read_from_the_side_channel_first_and_the_payload_second() {
        let mut envelope = HookEnvelope {
            agent: AgentKind::Claude,
            pane_key: "a/b".into(),
            tab_id: String::new(),
            launch_token: String::new(),
            worktree_id: String::new(),
            env: String::new(),
            version: String::new(),
            hook_event_name: String::new(),
            payload: r#"{"hook_event_name":"PreToolUse","tool_name":"Bash"}"#.into(),
        };
        assert_eq!(
            envelope_event_name(&envelope).as_deref(),
            Some("PreToolUse")
        );
        envelope.payload = r#"{"hook_event_name":"PreToolUse","session_id":"session-7"}"#.into();
        assert_eq!(
            envelope_event_and_session(&envelope),
            (Some("PreToolUse".into()), Some("session-7".into()))
        );
        // The side channel wins when a script carried one — Copilot's script
        // knows the event better than a payload that may not name it.
        envelope.hook_event_name = "Stop".into();
        assert_eq!(envelope_event_name(&envelope).as_deref(), Some("Stop"));
        // A payload that is not JSON is not an error, it is just nameless.
        envelope.hook_event_name = String::new();
        envelope.payload = "not json".into();
        assert_eq!(envelope_event_name(&envelope), None);
    }

    /// The detail the bridge used to drop, out of the shapes the vendors this
    /// window installs for actually write.
    ///
    /// One payload per FAMILY, because the families differ in the field names
    /// and not in the idea: Claude's `tool_name`/`tool_input` is also Droid's,
    /// Grok's, Devin's, Codex's and Command Code's; Cursor's shell event names
    /// no tool at all; Amp's plugin renames both.
    #[test]
    fn a_tool_call_says_which_tool_and_on_what_however_its_vendor_spells_it() {
        // Claude, the shape `zerocode-hookd` installs ten events for.
        let claude = activity_of(
            "PreToolUse",
            r#"{"session_id":"s-1","hook_event_name":"PreToolUse","tool_name":"Bash",
                "tool_input":{"command":"cargo test --workspace","description":"run the suite"}}"#,
        )
        .expect("a claude tool call said nothing");
        assert_eq!(claude.verb, Tool::Bash);
        assert_eq!(claude.target.as_deref(), Some("cargo test --workspace"));
        assert_eq!(claude.phase, Phase::Started);
        // The same family finishing, on a file rather than a command.
        let read = activity_of(
            "PostToolUse",
            r#"{"hook_event_name":"PostToolUse","tool_name":"Read",
                "tool_input":{"file_path":"/repo/ui/shell.js"},"tool_response":{"text":"…"}}"#,
        )
        .expect("a finished tool call said nothing");
        assert_eq!(read.verb, Tool::Read);
        assert_eq!(read.target.as_deref(), Some("/repo/ui/shell.js"));
        assert_eq!(read.phase, Phase::Finished);
        // And failing, which is its own event for this family.
        let failed = activity_of(
            "PostToolUseFailure",
            r#"{"hook_event_name":"PostToolUseFailure","tool_name":"Edit",
                "tool_input":{"file_path":"/repo/src/main.rs"},"error":"no such file"}"#,
        )
        .expect("a failed tool call said nothing");
        assert_eq!(failed.verb, Tool::Edit);
        assert_eq!(failed.phase, Phase::Failed);

        // Cursor's shell event, which names no tool: the EVENT is the name,
        // and the command sits at the top level rather than inside an input.
        let cursor = activity_of(
            "beforeShellExecution",
            r#"{"hook_event_name":"beforeShellExecution","command":"git status --short",
                "cwd":"/repo"}"#,
        )
        .expect("cursor's shell event said nothing");
        assert_eq!(cursor.verb, Tool::Bash);
        assert_eq!(cursor.target.as_deref(), Some("git status --short"));

        // Amp's plugin, which renames both fields and reports its verdict in
        // the payload — it has no failure event at all.
        let amp = activity_of(
            "tool.call",
            r#"{"hook_event_name":"tool.call","threadId":"T-1","toolUseId":"u-1",
                "tool":"Grep","input":{"pattern":"activity_of","path":"crates"}}"#,
        )
        .expect("an amp tool call said nothing");
        assert_eq!(amp.verb, Tool::Grep);
        assert_eq!(amp.target.as_deref(), Some("activity_of"));
        let amp_failed = activity_of(
            "tool.result",
            r#"{"hook_event_name":"tool.result","tool":"Bash","input":{"cmd":"pnpm build"},
                "status":"error","output":"exit 1"}"#,
        )
        .expect("an amp tool result said nothing");
        assert_eq!(amp_failed.phase, Phase::Failed);
        assert_eq!(amp_failed.target.as_deref(), Some("pnpm build"));

        // An argv array, which is how the shell tools of several vendors pass a
        // command. A reader that only understood strings drew a bare verb for
        // every command in the run.
        let argv = activity_of(
            "PreToolUse",
            r#"{"tool_name":"shell","tool_input":{"command":["bash","-lc","cargo fmt --check"]}}"#,
        )
        .expect("an argv command said nothing");
        assert_eq!(argv.target.as_deref(), Some("bash -lc cargo fmt --check"));

        // A tool nobody here has heard of keeps its own name rather than being
        // filed under a category this table invented for it.
        let mcp = activity_of(
            "PreToolUse",
            r#"{"tool_name":"mcp__linear__create_issue","tool_input":{"title":"fix the bridge"}}"#,
        )
        .expect("an unknown tool said nothing");
        assert_eq!(
            mcp.verb,
            Tool::Other("mcp__linear__create_issue".to_string())
        );
        // And says nothing about its target rather than dumping the whole input
        // object onto a row.
        assert_eq!(mcp.target, None);
    }

    /// The prompt and the turn's end ride the same stream as the tools between
    /// them, and everything else on the wire stays out of it.
    #[test]
    fn a_prompt_and_a_turn_ending_are_in_the_stream_and_the_rest_is_not() {
        let asked = activity_of(
            "UserPromptSubmit",
            r#"{"hook_event_name":"UserPromptSubmit","prompt":"훅 다리를 고쳐 줘"}"#,
        )
        .expect("a prompt said nothing");
        assert_eq!(asked.phase, Phase::Prompted);
        assert_eq!(asked.verb.as_str(), "prompt");
        assert_eq!(asked.target.as_deref(), Some("훅 다리를 고쳐 줘"));

        // Every way a turn ends, off the one table that already knows them.
        for ending in ["Stop", "StopFailure", "afterAgentResponse", "agent.end"] {
            let done = activity_of(ending, r#"{"hook_event_name":"Stop"}"#)
                .unwrap_or_else(|| panic!("{ending} said nothing"));
            assert_eq!(done.phase, Phase::Stopped, "{ending}");
            assert_eq!(done.verb.as_str(), "stop", "{ending}");
            assert_eq!(done.target, None, "{ending}");
        }

        // A helper's lifecycle is the roster's fact and is drawn as a row —
        // and `SubagentStop` must NOT reach the turn-ending table, or every
        // helper that finished would report its parent's turn as over.
        for quiet in [
            "SubagentStart",
            "SubagentStop",
            "subagent_stop",
            "SessionStart",
            "PreCompact",
            "Notification",
            // The ask is already a card with the tool's name on it; putting it
            // here too would draw one event twice.
            "PermissionRequest",
            "",
        ] {
            assert_eq!(
                activity_of(
                    quiet,
                    r#"{"tool_name":"Bash","tool_input":{"command":"ls"}}"#
                ),
                None,
                "{quiet:?}"
            );
        }
        // A tool event carrying no tool name and no command says nothing —
        // a row reading "…" is worse than no row.
        assert_eq!(activity_of("PreToolUse", "{}"), None);
        assert_eq!(activity_of("PreToolUse", "not json"), None);
        assert_eq!(
            activity_of("PreToolUse", r#"{"tool_name":"   "}"#),
            None,
            "a blank name is the vendor saying nothing, not a tool called nothing"
        );
    }

    /// A row is a thing people screen-share, and a ring is a thing that is
    /// held twenty deep per agent.
    #[test]
    fn a_target_is_scrubbed_of_credentials_and_clamped_to_a_row() {
        let secret = activity_of(
            "PreToolUse",
            r#"{"tool_name":"Bash",
                "tool_input":{"command":"git clone https://joe:ghp_secret@github.com/o/r.git"}}"#,
        )
        .expect("a clone said nothing");
        let target = secret.target.expect("the command vanished with its token");
        assert!(!target.contains("ghp_secret"), "{target}");
        assert!(
            target.contains("https://***@github.com/o/r.git"),
            "{target}"
        );

        // Long, and multi-line: a heredoc must not draw a row forty lines tall.
        let long = format!(
            r#"{{"tool_name":"Bash","tool_input":{{"command":"{}"}}}}"#,
            "echo one\\ntwo ".repeat(60)
        );
        let clipped = activity_of("PreToolUse", &long)
            .expect("a long command said nothing")
            .target
            .expect("a long command lost its target");
        assert!(
            clipped.chars().count() <= ACTIVITY_TARGET_CHARS + 1,
            "{} chars survived",
            clipped.chars().count()
        );
        assert!(clipped.ends_with('…'), "{clipped}");
        assert!(!clipped.contains('\n'), "{clipped}");
    }

    /// A composed line comes back apart into the same three fields a tool
    /// event does, so one drawing path serves both roads.
    #[test]
    fn a_composed_activity_line_reads_back_as_a_verb_and_a_target() {
        let read = activity_said("Read · src/x.rs").expect("a composed line said nothing");
        assert_eq!(
            (read.verb.as_str(), read.target.as_deref(), read.phase),
            ("read", Some("src/x.rs"), Phase::Started)
        );

        // A verb on its own is still the truth about what is happening, and a
        // word no vendor list knows is kept as itself rather than dropped.
        let bare = activity_said("working").expect("a bare verb said nothing");
        assert_eq!((bare.verb.as_str(), bare.target), ("working", None));

        // The target walks through the same scrub and the same clamp every
        // other target does — this line reaches a row people screen-share.
        let secret = activity_said("Bash · git clone https://joe:ghp_secret@github.com/o/r.git")
            .expect("a clone said nothing")
            .target
            .expect("the command vanished with its token");
        assert!(!secret.contains("ghp_secret"), "{secret}");
        let long = activity_said(&format!("Read · {}", "a".repeat(400)))
            .expect("a long line said nothing")
            .target
            .expect("a long line lost its target");
        assert!(long.chars().count() <= ACTIVITY_TARGET_CHARS + 1);

        // Nothing said is nothing drawn.
        assert!(activity_said("   ").is_none());
    }

    /// The verb travels as a plain word, because the window draws a word.
    #[test]
    fn an_activity_is_json_a_row_can_read_without_unwrapping_anything() {
        let activity = activity_of(
            "PreToolUse",
            r#"{"tool_name":"Bash","tool_input":{"command":"cargo test --workspace"}}"#,
        )
        .expect("a tool call said nothing");
        assert_eq!(
            serde_json::to_string(&activity).expect("serialize"),
            r#"{"verb":"bash","target":"cargo test --workspace","phase":"started"}"#
        );
        // A verb with no target leaves the field out entirely rather than
        // shipping a null the window has to test for.
        let stopped = activity_of("Stop", "{}").expect("a stop said nothing");
        assert_eq!(
            serde_json::to_string(&stopped).expect("serialize"),
            r#"{"verb":"stop","phase":"stopped"}"#
        );
        // And it comes back as what it was, including a vendor's own name.
        let back: Activity = serde_json::from_str(
            r#"{"verb":"mcp__linear__create_issue","target":"x","phase":"failed"}"#,
        )
        .expect("deserialize");
        assert_eq!(
            back.verb,
            Tool::Other("mcp__linear__create_issue".to_string())
        );
        assert_eq!(back.phase, Phase::Failed);
    }

    #[test]
    fn optional_metadata_may_be_absent_but_payload_may_not() {
        let minimal = r#"{"agent":"codex","pane_key":"a/b","payload":"{}"}"#;
        let envelope: HookEnvelope = serde_json::from_str(minimal).expect("deserialize");
        assert_eq!(envelope.agent, AgentKind::Codex);
        assert_eq!(envelope.worktree_id, "");

        let missing_payload = r#"{"agent":"codex","pane_key":"a/b"}"#;
        assert!(serde_json::from_str::<HookEnvelope>(missing_payload).is_err());
    }

    /// The background-tasks reader holds Orca's contract to the letter: an
    /// absent field is `None` (keep your roster), a malformed or unnamed
    /// entry marks the reading truncated (absence proves nothing), only an
    /// explicit terminal word retires work, and teammate-typed entries are
    /// carried but flagged so folds leave them alone.
    #[test]
    fn background_tasks_read_by_orcas_rules() {
        assert!(background_agent_tasks(r#"{"hook_event_name":"Stop"}"#).is_none());
        assert!(background_agent_tasks("not json").is_none());

        let read = background_agent_tasks(
            r#"{"background_tasks":[
                {"id":"a-1","type":"subagent","agent_type":"Explore","status":"running"},
                {"id":"a-2","type":"subagent","description":"map the vault","status":"completed"},
                {"id":"t-1","type":"teammate","agent_type":"lead"},
                {"id":"b-9","type":"bash","status":"running"},
                {"id":"","type":"subagent"},
                "garbage"
            ]}"#,
        )
        .expect("the field was present");
        assert!(
            read.truncated,
            "an unnamed entry and a non-object both stand"
        );
        assert_eq!(read.tasks.len(), 3, "bash tasks are not helper rows");
        assert!(read.tasks[0].running);
        assert_eq!(read.tasks[0].agent_type.as_deref(), Some("Explore"));
        assert!(!read.tasks[1].running, "completed is a terminal word");
        assert!(read.tasks[2].teammate);
        assert!(
            read.tasks[2].running,
            "no status counts as running — only an explicit terminal word ends work"
        );

        let clean = background_agent_tasks(
            r#"{"background_tasks":[{"id":"a-1","type":"subagent","status":"WEIRD"}]}"#,
        )
        .expect("present");
        assert!(!clean.truncated);
        assert!(
            clean.tasks[0].running,
            "an unrecognised status fails active"
        );

        // The cap: entry thirty-three is dropped and the reading says so.
        let many: Vec<String> = (0..33)
            .map(|at| format!(r#"{{"id":"m-{at}","type":"subagent"}}"#))
            .collect();
        let capped =
            background_agent_tasks(&format!(r#"{{"background_tasks":[{}]}}"#, many.join(",")))
                .expect("present");
        assert_eq!(capped.tasks.len(), MAX_BACKGROUND_SUBAGENTS);
        assert!(capped.truncated);
    }
}
