//! Scheduled work: what an automation is, and when it is next due.
//!
//! An automation is "at this time, in this workspace, hand this prompt to an
//! agent". The prompt half is the delivery engine in `zerocode-pty`; this is
//! the *when*.
//!
//! **The schedule is a cron expression, and the presets are sugar over it.**
//! Orca offers five cadences and turns four of them into cron
//! (`buildAutomationCronSchedule`, AutomationsPage-BiVKYudv.js:479-486); the
//! fifth *is* cron, typed by hand. Keeping one representation means the thing
//! that decides "is it due" never has to know which door the schedule came
//! through — and a person who outgrows the presets is not asked to give up
//! their existing automation to say so.
//!
//! Time is minute-resolution and local. Cron has no concept of seconds and a
//! developer's "run this at 09:00" means their own nine o'clock, not UTC's.
//! Everything here works in *minutes since the epoch, local* so that no
//! timezone library is needed to answer the only question asked: is the
//! current minute one this expression names?

use serde::{Deserialize, Serialize};

use crate::launch::split_command_line;

/// The cadences the editor offers, and how each becomes cron.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cadence {
    /// Every hour, at this minute past.
    Hourly,
    /// Every day, at this time.
    Daily,
    /// Monday to Friday, at this time.
    Weekdays,
    /// One day a week, at this time.
    Weekly,
    /// Whatever the person typed.
    Custom,
}

/// Where an automation does its work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceMode {
    /// A checkout that already exists. May reuse the agent session already
    /// running there.
    Existing,
    /// A fresh worktree per run, cut from the base branch.
    NewPerRun,
}

/// The clock face of a schedule, before it becomes cron.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schedule {
    pub cadence: Cadence,
    /// `HH:MM`, 24-hour. Ignored by [`Cadence::Hourly`] except for its minute.
    pub time: String,
    /// 0 = Sunday … 6 = Saturday, the cron numbering. Only [`Cadence::Weekly`]
    /// reads it.
    pub day_of_week: u8,
    /// The expression, for [`Cadence::Custom`].
    pub custom: String,
}

impl Default for Schedule {
    fn default() -> Self {
        Self {
            cadence: Cadence::Daily,
            time: "09:00".to_string(),
            day_of_week: 1,
            custom: String::new(),
        }
    }
}

impl Schedule {
    /// The cron expression this schedule means.
    ///
    /// The four preset arms are Orca's, verbatim in structure: hourly pins
    /// only the minute, weekdays pins `1-5`, weekly pins the chosen day, and
    /// daily — the fallback — pins the time on every day. Out-of-range values
    /// are clamped rather than refused, because they arrive from a stored file
    /// as often as from the editor, and a schedule that will not parse is a
    /// schedule that silently never runs.
    #[must_use]
    pub fn to_cron(&self) -> String {
        if self.cadence == Cadence::Custom {
            return self.custom.trim().to_string();
        }
        let (hour, minute) = parse_time(&self.time);
        match self.cadence {
            Cadence::Hourly => format!("{minute} * * * *"),
            Cadence::Weekdays => format!("{minute} {hour} * * 1-5"),
            Cadence::Weekly => format!("{minute} {hour} * * {}", self.day_of_week.min(6)),
            _ => format!("{minute} {hour} * * *"),
        }
    }
}

/// `HH:MM` as numbers, clamped. Anything unreadable is midnight — a stored
/// file can say anything, and refusing to parse would take the automation out
/// of the schedule without saying so.
#[must_use]
pub fn parse_time(time: &str) -> (u32, u32) {
    let mut parts = time.split(':');
    let hour = parts
        .next()
        .and_then(|value| value.trim().parse::<u32>().ok())
        .unwrap_or(0)
        .min(23);
    let minute = parts
        .next()
        .and_then(|value| value.trim().parse::<u32>().ok())
        .unwrap_or(0)
        .min(59);
    (hour, minute)
}

/// Whether a job's runs leave evidence, and who decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidencePolicy {
    /// The prompt decides — see [`evidence_requested`].
    #[default]
    Prompt,
    Always,
    Never,
}

/// The words a prompt uses when it wants proof. Lower-cased substrings, so
/// "스크린샷을 남겨" and "Screenshots please" both count.
const EVIDENCE_WORDS: &[&str] = &[
    "증거",
    "증빙",
    "스크린샷",
    "스크린 샷",
    "캡처",
    "캡쳐",
    "화면 기록",
    "녹화",
    "evidence",
    "screenshot",
    "screen shot",
    "capture",
    "proof",
    "recording",
];

/// The phrasings that take it back.
const NO_EVIDENCE_PHRASES: &[&str] = &[
    "증거 없이",
    "증거는 없이",
    "스크린샷 없이",
    "캡처 없이",
    "남기지 마",
    "남기지 말",
    "찍지 마",
    "찍지 말",
    "without evidence",
    "without screenshots",
    "without screenshot",
    "no evidence",
    "no screenshots",
    "no screenshot",
    "don't screenshot",
    "do not screenshot",
];

/// Does this prompt ask for proof? A word from `EVIDENCE_WORDS` anywhere,
/// unless the prompt also says not to. Deliberately a reading of the
/// person's words rather than a setting: "매일 QA 하고 스크린샷 남겨" should
/// simply work, and "그냥 돌려" should simply run.
#[must_use]
pub fn evidence_requested(prompt: &str) -> bool {
    let lower = prompt.to_lowercase();
    if NO_EVIDENCE_PHRASES
        .iter()
        .any(|phrase| lower.contains(phrase))
    {
        return false;
    }
    EVIDENCE_WORDS.iter().any(|word| lower.contains(word))
}

/// Whether this job's next run leaves evidence.
#[must_use]
pub fn leaves_evidence(automation: &Automation) -> bool {
    match automation.evidence {
        EvidencePolicy::Always => true,
        EvidencePolicy::Never => false,
        EvidencePolicy::Prompt => evidence_requested(&automation.prompt),
    }
}

/// One scheduled job.
///
/// `workspace` is a path rather than an opaque id: this product addresses
/// checkouts by path everywhere else, and an id would need a second table to
/// mean anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Automation {
    pub id: String,
    pub name: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    pub workspace: String,
    #[serde(default = "existing")]
    pub workspace_mode: WorkspaceMode,
    /// The branch a per-run worktree is cut from. `None` means the project's
    /// default.
    #[serde(default)]
    pub base_branch: Option<String>,
    /// Which agent to wake, as [`crate::agent_spec`] ids it. `None` means
    /// whatever the `command` below starts, which is how a schedule can drive
    /// something the registry has never heard of.
    ///
    /// Orca's own record carries this (`agentId`), and it is what decides the
    /// readiness signal the prompt is delivered on — a composer glyph, a shown
    /// cursor, or silence. Guessing it wrong costs eight seconds of waiting on
    /// something that will never appear.
    #[serde(default)]
    pub agent: Option<String>,
    /// What is handed to the agent once it is listening.
    pub prompt: String,
    /// The command that starts the agent. Empty means the window's configured
    /// terminal command — the same one `⌘T` runs.
    #[serde(default)]
    pub command: String,
    /// Reuse the agent already running in that checkout instead of starting
    /// one. Only meaningful for [`WorkspaceMode::Existing`].
    #[serde(default)]
    pub reuse_session: bool,
    #[serde(default)]
    pub schedule: Schedule,
    /// A command that must succeed before the prompt is sent. Empty is no
    /// precheck.
    #[serde(default)]
    pub precheck: String,
    #[serde(default = "precheck_seconds")]
    pub precheck_timeout_seconds: u32,
    /// How late a missed run may be and still be worth running. A machine that
    /// was asleep for two days should not wake up and fire two days of
    /// backlog.
    #[serde(default = "grace_minutes")]
    pub missed_run_grace_minutes: u32,
    /// Whether a run leaves evidence — a folder the window fills with a
    /// frame and a step line after every surface action. `Prompt` (the
    /// default) lets the prompt decide: one that asks for screenshots,
    /// captures or proof gets it, one that just runs does not.
    #[serde(default)]
    pub evidence: EvidencePolicy,
    /// Retire the run's shell once the agent reports its turn ended — the
    /// daily job that otherwise leaves yesterday's pane listening. Off unless
    /// the person asks: an open pane is also how a run is supervised.
    #[serde(default)]
    pub close_when_done: bool,
    /// Local minutes-since-epoch of the last run, or `None` if never.
    #[serde(default)]
    pub last_run_at: Option<i64>,
}

const fn yes() -> bool {
    true
}
const fn existing() -> WorkspaceMode {
    WorkspaceMode::Existing
}
const fn precheck_seconds() -> u32 {
    60
}
const fn grace_minutes() -> u32 {
    60
}

impl Automation {
    /// Refuse a schedule that cannot run where and how it says it will.
    ///
    /// Validation belongs beside the record rather than in one renderer: the
    /// Tauri command, migrations and future headless callers all write the
    /// same type. A custom command is argv, not a shell fragment, so the same
    /// quote-aware tokenizer used by agent launch settings is its grammar.
    pub fn validate_for_save(&self) -> Result<(), String> {
        if self.workspace.trim().is_empty() {
            return Err("워크스페이스를 선택해 주세요".to_string());
        }
        if self.prompt.trim().is_empty() {
            return Err("프롬프트를 입력해 주세요".to_string());
        }
        if !self.command.trim().is_empty() {
            let words = split_command_line(&self.command)
                .map_err(|error| format!("실행할 명령을 읽을 수 없습니다: {error}"))?;
            if words.is_empty() {
                return Err("실행할 명령을 입력해 주세요".to_string());
            }
        }
        Ok(())
    }

    /// Whether this automation should fire at `minute`.
    ///
    /// Three questions, in the order that makes the cheap ones count first: is
    /// it switched on, does the expression name this minute, and has it
    /// already run in it.
    ///
    /// The last is what stops a job firing repeatedly through the minute it is
    /// due — the scheduler is asked several times a second, and "the cron
    /// matches" is true for all sixty of those seconds.
    #[must_use]
    pub fn is_due(&self, minute: LocalMinute) -> bool {
        if !self.enabled {
            return false;
        }
        if self.last_run_at == Some(minute.epoch_minutes) {
            return false;
        }
        Cron::parse(&self.schedule.to_cron()).is_some_and(|cron| cron.matches(minute))
    }

    /// A run that was missed while the machine was away, if one is worth
    /// making up.
    ///
    /// Returns the most recent missed minute within the grace window, or
    /// `None`. Only the most recent: a laptop shut for a week owes its owner
    /// one run of the nightly job, not seven. Orca models the same idea as
    /// `missedRunGraceMinutes`.
    #[must_use]
    pub fn missed_run(&self, now: LocalMinute) -> Option<LocalMinute> {
        if !self.enabled {
            return None;
        }
        let cron = Cron::parse(&self.schedule.to_cron())?;
        let last = self.last_run_at.unwrap_or(i64::MIN);
        let grace = i64::from(self.missed_run_grace_minutes);
        // Walk back through the grace window, newest first, and stop at the
        // first minute the expression names. Starting at `now - 1`: the
        // current minute is `is_due`'s business, not a missed run.
        (1..=grace)
            .map(|back| LocalMinute {
                epoch_minutes: now.epoch_minutes - back,
            })
            .filter(|candidate| candidate.epoch_minutes > last)
            .find(|candidate| cron.matches(*candidate))
    }

    /// The next occurrence that has not already been consumed.
    ///
    /// [`Cron::next_after`] deliberately includes its starting minute. That is
    /// right before a due run, but wrong immediately after one: listing a job
    /// in the same minute it fired must not call the just-spent occurrence its
    /// next run. Starting one minute beyond the last consumed minute keeps the
    /// scheduler and the list on one definition.
    #[must_use]
    pub fn next_unconsumed_run(&self, now: LocalMinute) -> Option<LocalMinute> {
        if !self.enabled {
            return None;
        }
        let floor = self.last_run_at.map_or(now.epoch_minutes, |last| {
            now.epoch_minutes.max(last.saturating_add(1))
        });
        Cron::parse(&self.schedule.to_cron())?.next_after(LocalMinute {
            epoch_minutes: floor,
        })
    }
}

/// Whether a save may name this agent.
///
/// `stored` is the record already on file under the same id, or `None` when
/// the job is being written for the first time.
///
/// **A new job may only name an agent the registry knows.** The id decides
/// which readiness signal the prompt is delivered on, so one nobody can
/// resolve buys eight seconds of waiting on a mark that will never appear —
/// and the editor is the one moment a person is looking at the thing that is
/// wrong.
///
/// **An edit keeps the unrecognised id it was already carrying, and only that
/// one.** A record written by a newer build, or by hand, must not lose its
/// agent because somebody fixed a typo in its name: refusing there would make
/// every unrecognised job unfixable, and the only way out would be re-pointing
/// it at a different agent, which is a change nobody asked for. Swapping one
/// unknown id for another is not keeping — it is the new choice this rule
/// refuses, made under cover of an edit.
///
/// Orca draws the same asymmetry from the other side: its form keeps the
/// draft's own agent in the list even when it is disabled or unknown
/// (`visibleAgents`), while its save refuses an agent outside the active set
/// only when nothing is being edited.
///
/// `None` — no agent named, so whatever `command` starts decides — is always
/// allowed.
#[must_use]
pub fn agent_may_be_saved(chosen: Option<&str>, stored: Option<&Automation>) -> bool {
    let Some(id) = chosen else {
        return true;
    };
    if crate::agent_spec(id).is_some() {
        return true;
    }
    stored.is_some_and(|held| held.agent.as_deref() == Some(id))
}

/// Which door a firing came through.
///
/// The record has never carried this, because until now nothing downstream
/// asked. The precheck asks it FIRST — Orca's guard opens
/// `if (run.trigger !== "scheduled" || !automation.precheck) return null;`
/// (`runLocalPrecheck`) — so the trigger is not bookkeeping, it is the gate's
/// first question, and it has to arrive as an argument rather than be guessed
/// from the record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunTrigger {
    /// The clock fired it, and nobody is looking.
    Scheduled,
    /// A person pressed 지금 실행 while watching.
    Manual,
}

/// Whether this firing has to pass its precheck before anything is started.
///
/// **Only a scheduled run is gated, and only when a command was written.**
/// The asymmetry is the point rather than an omission: a precheck exists to
/// stop a job from waking an agent at 3am against a tree that is not worth
/// waking one for — a red build, a branch that moved, a machine on battery.
/// Somebody who presses 지금 실행 has already made that judgement with their
/// eyes on the screen, and a window that answers "no, your test suite is red"
/// to a deliberate press is a window arguing with its owner.
///
/// The command is trimmed, so a field holding a stray newline is no precheck
/// at all rather than a shell run of nothing that exits 0 and passes.
#[must_use]
pub fn should_run_precheck(trigger: RunTrigger, precheck: &str) -> bool {
    trigger == RunTrigger::Scheduled && !precheck.trim().is_empty()
}

/// How a precheck ended, reduced to the three facts the verdict needs.
///
/// Deliberately not the shell's whole story — the output and the duration are
/// for the person reading the history later ([`PrecheckRecord`]); these three
/// are what decides whether an agent is woken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrecheckOutcome {
    /// `None` when the process was killed or never ran, which is why a
    /// missing code can never be mistaken for zero.
    pub exit_code: Option<i32>,
    /// It was still running when its budget ran out, and was killed.
    pub timed_out: bool,
    /// The shell could not be started, or could not be waited on.
    pub errored: bool,
}

impl PrecheckOutcome {
    /// Whether the run may go ahead. Orca's `didAutomationPrecheckPass`:
    /// `Boolean(result && !result.timedOut && !result.error && result.exitCode
    /// === 0)`.
    ///
    /// **Every uncertainty reads as a refusal.** A precheck that hung, a shell
    /// that would not start, an exit code nobody could read — none of those is
    /// evidence that the tree is in the state the job asked for, and the whole
    /// value of the gate is that it fails closed. The alternative is a machine
    /// where deleting the precheck command and breaking it are the same thing.
    #[must_use]
    pub const fn passed(&self) -> bool {
        !self.timed_out && !self.errored && matches!(self.exit_code, Some(0))
    }
}

/// Whether a job may continue the agent session already living in its
/// checkout, instead of starting another one.
///
/// **`NewPerRun` is always false, whatever the record says.** A checkout that
/// was cut seconds ago has never had an agent in it, so there is no session to
/// reuse — the flag can only mean "start one and call it reuse", which is the
/// same shell with a misleading name on it. Orca normalises the same way, at
/// both doors it can be written through: `reuseSession = workspaceMode ===
/// "existing" && reuseSession === true`.
///
/// Normalising rather than refusing, because the pair is reachable honestly: a
/// person sets up an `Existing` job with reuse on, then changes the mode.
/// Losing the reuse flag is what they asked for; being refused a save is not.
#[must_use]
pub fn reuse_normalized(mode: WorkspaceMode, reuse_session: bool) -> bool {
    matches!(mode, WorkspaceMode::Existing) && reuse_session
}

/// The precheck that stopped a run, kept so the history can say why.
///
/// Orca's own record carries command, exit code, timed-out, duration and both
/// pipes (truncated) — the same shape, minus the started/completed pair, which
/// is [`AutomationRun::at_epoch_ms`] plus this duration and would be a second
/// way to say the same instant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrecheckRecord {
    /// What was asked. A person reading last week's skip needs the question,
    /// not only the answer — the command has almost certainly been edited
    /// since.
    pub command: String,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub duration_ms: i64,
    /// Both pipes, head and tail, cut by
    /// [`crate::commit_failure::truncate_prompt_text`].
    pub output: String,
}

/// Where a firing stopped before it could open a usable terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationFailureStage {
    Workspace,
    Command,
    LaunchPlan,
    Spawn,
}

/// A failed occurrence is first-class run history, not a transient toast.
///
/// `scheduled_for_epoch_minutes` is the scheduler occurrence identity. It is
/// kept in the run as well as `Automation::last_run_at` so a successfully
/// written history row still fences a retry if the schedule document could
/// not be updated afterwards. A worktree cleanup is deliberately best effort;
/// when it fails, the error and the created path remain durable provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationFailureRecord {
    pub stage: AutomationFailureStage,
    pub scheduled_for_epoch_minutes: i64,
    pub error: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_cleanup_error: Option<String>,
}

/// One firing of a scheduled job, as it is remembered afterwards.
///
/// Orca loads a target's runs beside its automations — one `Promise.all` over
/// `listAutomationsForTarget` and `listAutomationRunsForTarget`
/// (AutomationsPage-CJka0o7M.js:216447) — so the history is first-class data,
/// not a detail hanging off the schedule.
///
/// What is deliberately absent is the token and cost roll-up
/// (`summarizeAutomationRunUsage`, :19344). That reads a session's usage, and
/// this window has no per-run usage plumbing yet; a record with a `usage`
/// field nothing can fill is a promise the screen would have to keep printing
/// blanks about. Counts, times and destinations are what this carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationRun {
    /// Unique across the ledger. Composed by the caller rather than random:
    /// the file outlives the window, and a run is already identified by the
    /// instant it started and the shell it opened.
    pub id: String,
    /// Which schedule fired. The only key the history is ever sliced by.
    pub automation_id: String,
    /// The local scheduler minute this row consumed.
    ///
    /// This is distinct from [`Self::at_epoch_ms`]: a grace-window run can be
    /// started at 09:03 while consuming the 09:00 occurrence.  Keeping the
    /// occurrence on every kind of row (started, skipped, or failed) lets the
    /// ledger fence a retry even if writing `Automation::last_run_at` fails.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduled_for_epoch_minutes: Option<i64>,
    /// Milliseconds since the epoch, UTC. Milliseconds rather than the
    /// scheduler's local minutes because this is a moment being *reported* to
    /// a reader, and the reader's own locale turns it into words.
    pub at_epoch_ms: i64,
    /// The checkout the run actually happened in.
    pub root: String,
    /// The shell it opened, so a run still on screen can be jumped to.
    pub term: u32,
    /// The checkout this run CUT, when its mode said to cut one.
    ///
    /// This field is the provenance. Orca stores it on the worktree record
    /// (`automationProvenance.kind === "created-by-automation"`,
    /// worktree-activation-3qRw45tK.js:59302); we keep one source of truth
    /// instead, because a second store would be a second thing to get wrong
    /// about the same fact — and the run that cut the checkout is already
    /// being written down.
    pub made_worktree: Option<String>,
    /// The precheck that stopped this firing, when one did. Its PRESENCE is
    /// the skip — there is no separate flag to disagree with it.
    ///
    /// **A gated run is still written down.** The alternative is a schedule
    /// whose history is silent on the nights it did not run, which reads
    /// exactly like a scheduler that is broken: the one question somebody
    /// opens this list to answer is "why did nothing happen last night", and a
    /// missing row cannot answer it. `#[serde(default)]` so ledgers written
    /// before this field existed still load — every one of their runs did
    /// start.
    #[serde(default)]
    pub skipped: Option<PrecheckRecord>,
    /// Why this firing could not launch, when it could not. Its presence is
    /// terminal in the same way `skipped` is; no shell exists to finish it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<AutomationFailureRecord>,
    /// When the shell this run opened was SEEN to finish.
    ///
    /// **Absence is "no ending was witnessed", not "still running".** The
    /// window learns of an ending by watching the pty, so a run whose window
    /// was quit while it worked keeps an open row forever — and that is the
    /// honest record of what was observed. Inventing an end for it, at load or
    /// at the next start, would put a time in the history that nothing ever
    /// measured.
    ///
    /// `#[serde(default)]` says that out loud, though serde would already
    /// answer a missing `Option` with `None` — measured, not assumed: taking
    /// the attribute off changes nothing a test can see. What keeps a ledger
    /// written before this field loading is the type; the attribute is the
    /// intent, and it is what holds if this ever stops being an `Option`. The
    /// behaviour that has to be true either way — an old row reading as
    /// UNFINISHED rather than as finished at the epoch — is pinned by a test.
    #[serde(default)]
    pub ended_at_epoch_ms: Option<i64>,
    /// What the shell exited with, when a code could be read.
    ///
    /// **`None` is "no code could be read", never zero.** The child is reaped
    /// as a separate question from its output side closing, and the two can
    /// answer a moment apart. Reading a missing code as success is the mistake
    /// [`PrecheckOutcome::passed`] exists to refuse.
    ///
    /// It reports the SHELL, the only ending this window can witness: an agent
    /// that gave up and exited cleanly exited 0, and the row says what
    /// happened rather than guessing at what it meant.
    #[serde(default)]
    pub exit_code: Option<i32>,
    /// When the AGENT said its turn ended — a `done` hook report from the
    /// run's shell — as distinct from the shell exiting. An interactive agent
    /// that finished and sits at its prompt is completed but not ended; a
    /// second turn in the same shell moves this to its own ending.
    #[serde(default)]
    pub completed_at_epoch_ms: Option<i64>,
    /// What the agent last said when it completed — the run's result, clipped
    /// to a few thousand characters; the transcript keeps the whole answer.
    #[serde(default)]
    pub result: Option<String>,
    /// The folder the run was told to leave its evidence in (screenshots,
    /// logs). Made before the shell was opened, named to the agent through
    /// `ZEROCODE_RUN_EVIDENCE_DIR`, listed on the run's row afterwards.
    #[serde(default)]
    pub evidence_dir: Option<String>,
    /// The launch the run's shell was born with (the window's launch
    /// fingerprint), so a completion or an ending is matched to THIS shell and
    /// never to a later one wearing the same terminal id. Terminal ids come
    /// back around within a window's life; a fingerprint does not. `None` on
    /// rows written before the field existed — such a row takes no further
    /// stamp and is closed by the boot sweep instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch: Option<u64>,
    /// The ending was not witnessed: the window came back and the shell was
    /// simply not there (a restart ends every pane's child), so the row was
    /// closed by the boot sweep rather than by the exit it never saw.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unwitnessed: bool,
}

/// Write the agent's completion onto the row that opened `term` — the row
/// whose shell carries `launch`, and no other.
///
/// Matched by `run_of_shell`: a terminal id names a series of shells over a
/// window's life, and a `done` from this afternoon's occupant of id 13 must not
/// land on the row a job opened on id 13 three days ago (measured 2026-09-05:
/// an Explore agent's report stamped onto an Obsidian run, then the job's
/// `close_when_done` shut the agent's pane). A skipped or failed firing never
/// had a shell. Unlike an ending, a completion may be written again — a shell
/// that ran a second turn completed again, and the row's result is what the
/// agent said last; a row already ended takes none, its shell is gone.
/// Answers which run completed, so the caller can announce it and consult
/// the job's own settings, or `None` when the shell was nobody's run.
pub fn mark_completed(
    runs: &mut [AutomationRun],
    term: u32,
    launch: Option<u64>,
    completed_at: i64,
    result: Option<String>,
) -> Option<(String, String)> {
    let run = run_of_shell(runs, term, launch)?;
    if run.ended_at_epoch_ms.is_some() {
        return None;
    }
    run.completed_at_epoch_ms = Some(completed_at);
    if result.is_some() {
        run.result = result;
    }
    Some((run.automation_id.clone(), run.id.clone()))
}

/// How many FINAL runs of ONE job the ledger keeps.
pub const RUNS_KEPT_PER_AUTOMATION: usize = 100;

fn run_is_final(run: &AutomationRun) -> bool {
    run.skipped.is_some() || run.failure.is_some() || run.ended_at_epoch_ms.is_some()
}

/// Keep the newest `cap` final rows per automation without pruning live runs.
///
/// A row whose terminal has not been seen to finish is evidence about a live
/// or unwitnessed process.  Retention may forget old history; it must never be
/// the action that makes such a process disappear from the ledger.
///
/// The rows forgotten are handed back: a run's evidence folder outlives its
/// row otherwise, and the caller holding the disk is the one to sweep it.
pub fn prune_final_runs(runs: &mut Vec<AutomationRun>, cap: usize) -> Vec<AutomationRun> {
    let mut jobs: Vec<String> = Vec::new();
    for run in runs.iter() {
        if !jobs.iter().any(|job| job == &run.automation_id) {
            jobs.push(run.automation_id.clone());
        }
    }
    let mut forgotten = Vec::new();
    for job in jobs {
        while runs
            .iter()
            .filter(|run| run.automation_id == job && run_is_final(run))
            .count()
            > cap
        {
            let Some(oldest) = runs
                .iter()
                .position(|run| run.automation_id == job && run_is_final(run))
            else {
                break;
            };
            forgotten.push(runs.remove(oldest));
        }
    }
    forgotten
}

/// Write a run into the ledger, and forget this job's oldest final row if it
/// now has more than `cap` final rows. Unfinished rows are never pruned.
///
/// **The cap is per automation, not per file.** A `NewPerRun` job firing
/// hourly would otherwise push every other job's entire history out of the
/// file within a day — the busiest schedule would be the only one with any
/// past, which is exactly backwards from what a person opening a quiet
/// weekly job wants to see.
///
/// **Oldest means earliest in the ledger, not lowest timestamp.** The file is
/// an append-only log, so its order already is its chronology; reading the
/// clock instead would let a machine whose time jumped backwards evict the run
/// that just happened, which is the one run somebody is definitely about to
/// look for.
pub fn record_run(
    runs: &mut Vec<AutomationRun>,
    run: AutomationRun,
    cap: usize,
) -> Vec<AutomationRun> {
    runs.push(run);
    prune_final_runs(runs, cap)
}

/// Write the end of a run onto the row that opened `term`.
///
/// Answers whether anything was stamped, so a caller holding a file can tell
/// an ending it recorded from a shell that was never a run's — most terminals
/// in this window are somebody's own, and rewriting the ledger for each of
/// them would be a write per closed tab.
///
/// **The NEWEST row naming that terminal is the only candidate.** Terminal ids
/// are handed back out, so one number names a series of shells over a window's
/// life, and the run that just ended is the last one to have claimed it. An
/// older row for the same id is never reached — it stayed open because nobody
/// saw it stop, and giving it this exit would date last month's run by this
/// afternoon's, which is a lie no reader could ever catch.
///
/// **An ending is written once.** If that newest row already carries one, the
/// shell that just exited was not the run's — it is the second shell to wear
/// the id — and nothing is written. Overwriting would replace a measured
/// moment with a coincidence.
///
/// The row whose shell is the one at `term` right now.
///
/// A shell is named by its terminal id AND the launch fingerprint the window
/// minted when it spawned; the id is reused, the fingerprint is not. A row
/// written before fingerprints were recorded (`launch: None`) matches nothing:
/// the shell it opened cannot be told from the shell that inherited its id,
/// and guessing is how a stale row gets somebody else's verdict. A shell the
/// window holds no fingerprint for (`launch: None` here) is nobody's run.
fn run_of_shell(
    runs: &mut [AutomationRun],
    term: u32,
    launch: Option<u64>,
) -> Option<&mut AutomationRun> {
    let launch = launch?;
    runs.iter_mut().rev().find(|run| {
        run.term == term
            && run.launch == Some(launch)
            && run.skipped.is_none()
            && run.failure.is_none()
    })
}

/// Close every run whose shell this window is not holding, as an ending nobody
/// witnessed.
///
/// A restart ends every pane's child, and the exit that would have stamped the
/// row happened while no window was listening — so the row would read
/// "running" for the rest of its life (measured 2026-09-05: a run of the day
/// before, still "실행 중" the next evening). Rows whose terminal `is_live`
/// says this process holds are left alone: a run started before the first
/// sweep is a live run. Returns how many rows were closed.
pub fn close_unwitnessed(
    runs: &mut [AutomationRun],
    now: i64,
    is_live: impl Fn(u32) -> bool,
) -> usize {
    let mut closed = 0;
    for run in runs.iter_mut() {
        if run.skipped.is_some()
            || run.failure.is_some()
            || run.ended_at_epoch_ms.is_some()
            || is_live(run.term)
        {
            continue;
        }
        run.ended_at_epoch_ms = Some(now);
        run.exit_code = None;
        run.unwitnessed = true;
        closed += 1;
    }
    closed
}

/// A skipped firing is passed over entirely: its `term` is a placeholder for a
/// shell that was never opened, and the row is already final on its own terms.
pub fn mark_ended(
    runs: &mut [AutomationRun],
    term: u32,
    launch: Option<u64>,
    ended_at: i64,
    exit_code: Option<i32>,
) -> bool {
    let Some(run) = run_of_shell(runs, term, launch) else {
        return false;
    };
    if run.ended_at_epoch_ms.is_some() {
        return false;
    }
    run.ended_at_epoch_ms = Some(ended_at);
    run.exit_code = exit_code;
    true
}

/// Every checkout an automation cut, each named once.
///
/// The ledger IS the provenance, so this is the whole answer to "was this
/// workspace born of a schedule". Deduplicated because one path can be
/// reported by more than one run — a job re-cutting the same branch name, or
/// a ledger merged from two windows — and a caller building a hide-set would
/// otherwise pay for the repeats. Ledger order is preserved: the oldest
/// checkout is named first.
#[must_use]
pub fn born_worktrees(runs: &[AutomationRun]) -> Vec<String> {
    let mut named: Vec<String> = Vec::new();
    for run in runs {
        let Some(path) = run.made_worktree.as_deref() else {
            continue;
        };
        if !named.iter().any(|held| held == path) {
            named.push(path.to_string());
        }
    }
    named
}

/// Whether a history row already consumed this scheduled occurrence.
///
/// Normally `last_run_at` is the faster answer. This ledger fence is the
/// recovery answer when a started, skipped, or failed row reached disk but the
/// schedule stamp did not; without it the grace sweep retries the same
/// occurrence every minute.
#[must_use]
pub fn occurrence_recorded(
    runs: &[AutomationRun],
    automation_id: &str,
    minute: LocalMinute,
) -> bool {
    runs.iter().any(|run| {
        run.automation_id == automation_id
            && match run.scheduled_for_epoch_minutes {
                Some(scheduled) => scheduled == minute.epoch_minutes,
                // Compatibility with failure rows written by the brief build
                // that carried the occurrence only inside the failure. Once
                // the run-level value exists it is authoritative, so a
                // contradictory hand-edited row cannot consume two minutes.
                None => run.failure.as_ref().is_some_and(|failure| {
                    failure.scheduled_for_epoch_minutes == minute.epoch_minutes
                }),
            }
    })
}

/// Compatibility name for callers interested only in failed rows.
#[must_use]
pub fn failed_occurrence_recorded(
    runs: &[AutomationRun],
    automation_id: &str,
    minute: LocalMinute,
) -> bool {
    runs.iter().any(|run| {
        run.automation_id == automation_id
            && run.failure.is_some()
            && occurrence_recorded(std::slice::from_ref(run), automation_id, minute)
    })
}

/// A wall-clock minute, in the local timezone, as minutes since the epoch.
///
/// A plain number rather than a date type: the scheduler only ever asks "which
/// minute is this, and what are its calendar fields", and carrying those five
/// fields alongside costs nothing while a timezone-aware date library costs a
/// dependency and a class of bugs at every daylight-saving boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalMinute {
    pub epoch_minutes: i64,
}

/// The calendar fields a cron expression matches against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wall {
    pub minute: u32,
    pub hour: u32,
    pub day_of_month: u32,
    pub month: u32,
    /// 0 = Sunday.
    pub day_of_week: u32,
}

impl LocalMinute {
    /// The calendar fields of this minute, by the civil-from-days algorithm
    /// (Howard Hinnant's), which is exact for every proleptic Gregorian date
    /// and needs no table.
    #[must_use]
    pub const fn wall(self) -> Wall {
        let total = self.epoch_minutes;
        // Floor division, so minutes before the epoch land on the right day.
        let days = total.div_euclid(1440);
        let minute_of_day = total.rem_euclid(1440);

        // 1970-01-01 was a Thursday, which is 4 in a Sunday-first week.
        let day_of_week = (days + 4).rem_euclid(7);

        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let _year = if m <= 2 { y + 1 } else { y };

        Wall {
            minute: (minute_of_day % 60) as u32,
            hour: (minute_of_day / 60) as u32,
            day_of_month: d as u32,
            month: m as u32,
            day_of_week: day_of_week as u32,
        }
    }
}

/// A five-field cron expression, parsed.
///
/// Standard vocabulary and nothing beyond it: `*`, a number, `a-b`, `a-b/n`,
/// `*/n`, and comma-separated lists of those. Names (`MON`, `JAN`) and the
/// non-standard extensions (`@daily`, `L`, `?`) are **refused**, not guessed —
/// the same rule the terminal parser follows on the way out. A schedule that
/// silently means something other than what it says is worse than one the
/// editor rejects while the person is still looking at it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cron {
    minute: Field,
    hour: Field,
    day_of_month: Field,
    month: Field,
    day_of_week: Field,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Field {
    /// Every value this field allows, sorted. `any` is kept alongside because
    /// day-of-month and day-of-week interact through it (see [`Cron::matches`]).
    allowed: Vec<u32>,
    any: bool,
}

impl Field {
    fn parse(text: &str, min: u32, max: u32) -> Option<Self> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        let mut allowed = Vec::new();
        for part in text.split(',') {
            let (spec, step) = match part.split_once('/') {
                Some((spec, step)) => (spec, step.trim().parse::<u32>().ok().filter(|n| *n > 0)?),
                None => (part, 1),
            };
            let spec = spec.trim();
            let (lo, hi) = if spec == "*" {
                (min, max)
            } else if let Some((lo, hi)) = spec.split_once('-') {
                (
                    lo.trim().parse::<u32>().ok()?,
                    hi.trim().parse::<u32>().ok()?,
                )
            } else {
                let value = spec.parse::<u32>().ok()?;
                // A bare number with a step means "from here to the end",
                // which is how `0/15` is read everywhere.
                if step > 1 {
                    (value, max)
                } else {
                    (value, value)
                }
            };
            if lo < min || hi > max || lo > hi {
                return None;
            }
            allowed.extend((lo..=hi).step_by(step as usize));
        }
        allowed.sort_unstable();
        allowed.dedup();
        Some(Self {
            allowed,
            any: text == "*",
        })
    }

    fn matches(&self, value: u32) -> bool {
        self.allowed.binary_search(&value).is_ok()
    }
}

impl Cron {
    /// Parse five whitespace-separated fields, or `None`.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let fields: Vec<&str> = text.split_whitespace().collect();
        if fields.len() != 5 {
            return None;
        }
        Some(Self {
            minute: Field::parse(fields[0], 0, 59)?,
            hour: Field::parse(fields[1], 0, 23)?,
            day_of_month: Field::parse(fields[2], 1, 31)?,
            month: Field::parse(fields[3], 1, 12)?,
            // 7 is Sunday too, in every cron that has ever shipped.
            day_of_week: Field::parse(fields[4], 0, 7)?,
        })
    }

    /// Whether this expression names that minute.
    ///
    /// The day fields are an OR, not an AND, when both are restricted — cron's
    /// oldest and least obvious rule. `0 0 1 * 1` means "the first of the
    /// month, **and also** every Monday", not "Mondays that fall on the
    /// first". Implementing it as an AND turns a job someone expects weekly
    /// into one that fires a few times a year, and the mistake is invisible
    /// until the run does not happen.
    #[must_use]
    pub fn matches(&self, minute: LocalMinute) -> bool {
        let wall = minute.wall();
        if !self.minute.matches(wall.minute)
            || !self.hour.matches(wall.hour)
            || !self.month.matches(wall.month)
        {
            return false;
        }
        // Sunday is both 0 and 7.
        let dow = self.day_of_week.matches(wall.day_of_week)
            || (wall.day_of_week == 0 && self.day_of_week.matches(7));
        let dom = self.day_of_month.matches(wall.day_of_month);
        match (self.day_of_month.any, self.day_of_week.any) {
            (true, true) => true,
            (false, true) => dom,
            (true, false) => dow,
            (false, false) => dom || dow,
        }
    }

    /// The next minute at or after `from` that this expression names, searched
    /// up to a year ahead.
    ///
    /// A bounded scan rather than a calendar solver: a minute-by-minute walk
    /// over a year is half a million cheap comparisons, run once when a
    /// schedule changes, and it cannot disagree with [`Cron::matches`] the way
    /// a second implementation would.
    #[must_use]
    pub fn next_after(&self, from: LocalMinute) -> Option<LocalMinute> {
        const MINUTES_IN_A_YEAR: i64 = 366 * 24 * 60;
        (0..MINUTES_IN_A_YEAR)
            .map(|ahead| LocalMinute {
                epoch_minutes: from.epoch_minutes + ahead,
            })
            .find(|candidate| self.matches(*candidate))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minutes since the epoch for a local wall-clock time, for readable tests.
    fn at(year: i64, month: i64, day: i64, hour: i64, minute: i64) -> LocalMinute {
        // Days from civil, the inverse of the algorithm under test.
        let y = if month <= 2 { year - 1 } else { year };
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let mp = if month > 2 { month - 3 } else { month + 9 };
        let doy = (153 * mp + 2) / 5 + day - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        let days = era * 146_097 + doe - 719_468;
        LocalMinute {
            epoch_minutes: days * 1440 + hour * 60 + minute,
        }
    }

    /// The presets mean the cron Orca says they mean.
    ///
    /// Pinned as strings because this is the whole contract between the
    /// picker's five options and the thing that decides when a job runs. An
    /// hourly job pins only its minute; weekdays is `1-5`; weekly takes the
    /// chosen day; daily is the fallback.
    #[test]
    fn the_presets_are_the_cron_orca_builds() {
        let at_nine_thirty = |cadence, day| Schedule {
            cadence,
            time: "09:30".to_string(),
            day_of_week: day,
            custom: String::new(),
        };
        assert_eq!(at_nine_thirty(Cadence::Hourly, 1).to_cron(), "30 * * * *");
        assert_eq!(at_nine_thirty(Cadence::Daily, 1).to_cron(), "30 9 * * *");
        assert_eq!(
            at_nine_thirty(Cadence::Weekdays, 1).to_cron(),
            "30 9 * * 1-5"
        );
        assert_eq!(at_nine_thirty(Cadence::Weekly, 3).to_cron(), "30 9 * * 3");
        let custom = Schedule {
            cadence: Cadence::Custom,
            custom: "  */5 * * * *  ".to_string(),
            ..Schedule::default()
        };
        assert_eq!(custom.to_cron(), "*/5 * * * *");
    }

    /// A stored file can say anything, and it must not take a job off the
    /// schedule quietly.
    #[test]
    fn an_unreadable_time_is_midnight_not_a_refusal() {
        assert_eq!(parse_time("09:30"), (9, 30));
        assert_eq!(parse_time("9:5"), (9, 5));
        assert_eq!(
            parse_time("99:99"),
            (23, 59),
            "out of range was not clamped"
        );
        assert_eq!(parse_time("nonsense"), (0, 0));
        assert_eq!(parse_time(""), (0, 0));
    }

    /// Calendar fields come out right, including across the epoch and a leap
    /// day.
    #[test]
    fn a_minute_knows_its_own_calendar() {
        let new_years = at(2026, 1, 1, 0, 0).wall();
        assert_eq!((new_years.month, new_years.day_of_month), (1, 1));
        assert_eq!(new_years.day_of_week, 4, "2026-01-01 is a Thursday");

        let leap = at(2024, 2, 29, 13, 45).wall();
        assert_eq!((leap.month, leap.day_of_month), (2, 29));
        assert_eq!((leap.hour, leap.minute), (13, 45));
        assert_eq!(leap.day_of_week, 4, "2024-02-29 is a Thursday");

        let epoch = at(1970, 1, 1, 0, 0).wall();
        assert_eq!(epoch.day_of_week, 4, "the epoch was a Thursday");

        // Before the epoch: floor division, not truncation, or the day is off
        // by one and the weekday with it.
        let before = at(1969, 12, 31, 23, 59).wall();
        assert_eq!((before.month, before.day_of_month), (12, 31));
        assert_eq!(before.day_of_week, 3, "1969-12-31 was a Wednesday");
    }

    /// The cron vocabulary, and what it refuses.
    #[test]
    fn cron_parses_its_vocabulary_and_refuses_the_rest() {
        let every_five = Cron::parse("*/5 * * * *").expect("step");
        assert!(every_five.matches(at(2026, 3, 2, 10, 15)));
        assert!(!every_five.matches(at(2026, 3, 2, 10, 16)));

        let workday = Cron::parse("0 9 * * 1-5").expect("range");
        assert!(workday.matches(at(2026, 3, 2, 9, 0)), "Monday 09:00");
        assert!(
            !workday.matches(at(2026, 3, 1, 9, 0)),
            "Sunday is not a weekday"
        );

        let list = Cron::parse("0,30 8,17 * * *").expect("lists");
        assert!(list.matches(at(2026, 3, 2, 17, 30)));
        assert!(!list.matches(at(2026, 3, 2, 17, 15)));

        // Sunday answers to both 0 and 7.
        assert!(
            Cron::parse("0 0 * * 7")
                .expect("7")
                .matches(at(2026, 3, 1, 0, 0))
        );

        for refused in [
            "* * * *",     // four fields
            "* * * * * *", // six
            "60 * * * *",  // minute out of range
            "* 24 * * *",  // hour out of range
            "@daily",      // a shorthand we do not implement
            "0 9 * * MON", // a name we do not implement
            "0 9 * * ?",   // a Quartz extension
            "*/0 * * * *", // a step of zero
        ] {
            assert!(
                Cron::parse(refused).is_none(),
                "`{refused}` parsed, and a schedule nobody meant now runs"
            );
        }
    }

    /// Both day fields restricted means OR, not AND.
    ///
    /// Cron's oldest trap. `0 0 1 * 1` is "the first of the month, and also
    /// every Monday". Read as an AND it becomes "Mondays that fall on the
    /// first" — a job someone set up weekly that fires a few times a year,
    /// and nothing on screen says why.
    #[test]
    fn two_restricted_day_fields_are_an_or() {
        let both = Cron::parse("0 0 1 * 1").expect("parse");
        assert!(both.matches(at(2026, 4, 1, 0, 0)), "the 1st, a Wednesday");
        assert!(both.matches(at(2026, 4, 6, 0, 0)), "a Monday, not the 1st");
        assert!(
            !both.matches(at(2026, 4, 7, 0, 0)),
            "a Tuesday, not the 1st"
        );

        // With one of them unrestricted the other simply applies.
        let mondays = Cron::parse("0 0 * * 1").expect("parse");
        assert!(mondays.matches(at(2026, 4, 6, 0, 0)));
        assert!(!mondays.matches(at(2026, 4, 1, 0, 0)));
    }

    /// A schedule may name an agent, and the id it names has to be one the
    /// registry knows — otherwise the run waits eight seconds on a readiness
    /// signal chosen for a program that does not exist.
    #[test]
    fn a_scheduled_agent_is_one_the_registry_knows() {
        let job = nightly();
        let named = job.agent.as_deref().expect("the fixture names one");
        assert!(crate::agent_spec(named).is_some(), "{named}");
        // And absent is allowed: a schedule can start something the registry
        // has never heard of through its own `command`.
        let anonymous = Automation {
            agent: None,
            ..nightly()
        };
        assert_eq!(anonymous.agent, None);
    }

    /// An unknown agent may be kept, but never newly chosen.
    ///
    /// The asymmetry is the whole rule. A first save is refused while the
    /// person can still fix it; an edit of a record that already names an id
    /// nobody recognises keeps that id, so renaming the job does not quietly
    /// re-point it at something else. Anything that is a *new* unknown choice
    /// — a different unknown id, or one arriving on a record that named a real
    /// agent or none at all — is the case the first rule refuses, wearing an
    /// edit as a disguise.
    #[test]
    fn an_unknown_agent_may_be_kept_but_never_newly_chosen() {
        let known = nightly();
        let strange = Automation {
            agent: Some("ghostwriter".into()),
            ..nightly()
        };
        let anonymous = Automation {
            agent: None,
            ..nightly()
        };

        // Absence is always fine — the command decides.
        assert!(agent_may_be_saved(None, None));
        assert!(agent_may_be_saved(None, Some(&known)));
        assert!(agent_may_be_saved(None, Some(&strange)));

        // An id the registry knows is always fine, new or not.
        assert!(agent_may_be_saved(Some("claude"), None));
        assert!(agent_may_be_saved(Some("claude"), Some(&strange)));

        // New, and nobody knows it.
        assert!(
            !agent_may_be_saved(Some("ghostwriter"), None),
            "a job nothing can wake was written and stored"
        );

        // Stored under that very id: kept.
        assert!(
            agent_may_be_saved(Some("ghostwriter"), Some(&strange)),
            "editing the name of a job cost it the agent it already named"
        );

        // But an edit is not a licence to choose a different unknown one,
        for (chosen, held, why) in [
            (
                "typewriter",
                &strange,
                "one unknown id was swapped for another",
            ),
            (
                "ghostwriter",
                &known,
                "a real agent was traded for an unknown one",
            ),
            (
                "ghostwriter",
                &anonymous,
                "a job that named no agent acquired an unknown one",
            ),
        ] {
            assert!(!agent_may_be_saved(Some(chosen), Some(held)), "{why}");
        }
    }

    /// A record written before this field existed still loads, with no agent
    /// named — the schedules already on disk must not be lost to a new column.
    #[test]
    fn a_record_stored_before_the_agent_field_still_loads() {
        let mut json = serde_json::to_value(nightly()).expect("serialize");
        json.as_object_mut().expect("object").remove("agent");
        let back: Automation = serde_json::from_value(json).expect("an older record");
        assert_eq!(back.agent, None);
        assert_eq!(back.name, "nightly");
    }

    #[test]
    fn a_saved_automation_needs_a_workspace_prompt_and_readable_command() {
        let valid = nightly();
        assert_eq!(valid.validate_for_save(), Ok(()));

        let blank_workspace = Automation {
            workspace: "  \n".into(),
            ..nightly()
        };
        assert!(blank_workspace.validate_for_save().is_err());

        let blank_prompt = Automation {
            prompt: "\t".into(),
            ..nightly()
        };
        assert!(blank_prompt.validate_for_save().is_err());

        let quoted = Automation {
            command: r#"agent --project "/path with spaces""#.into(),
            ..nightly()
        };
        assert_eq!(quoted.validate_for_save(), Ok(()));

        let unclosed = Automation {
            command: r#"agent --project "/path with spaces"#.into(),
            ..nightly()
        };
        assert!(
            unclosed
                .validate_for_save()
                .expect_err("an unclosed quote was stored")
                .contains("Unclosed quote")
        );
    }

    fn nightly() -> Automation {
        Automation {
            id: "a1".into(),
            name: "nightly".into(),
            enabled: true,
            workspace: "/repo".into(),
            workspace_mode: WorkspaceMode::Existing,
            base_branch: None,
            agent: Some("codex".into()),
            prompt: "tidy up".into(),
            command: String::new(),
            reuse_session: false,
            evidence: EvidencePolicy::Prompt,
            close_when_done: false,
            schedule: Schedule {
                cadence: Cadence::Daily,
                time: "03:00".into(),
                day_of_week: 1,
                custom: String::new(),
            },
            precheck: String::new(),
            precheck_timeout_seconds: 60,
            missed_run_grace_minutes: 60,
            last_run_at: None,
        }
    }

    /// Due once in its minute, and never twice.
    ///
    /// The scheduler asks several times a second and the expression is true
    /// for every second of the minute it names. Without the last-run check a
    /// nightly tidy-up would fire sixty times, each one starting an agent.
    #[test]
    fn a_job_fires_once_in_the_minute_it_is_due() {
        let mut job = nightly();
        let due = at(2026, 5, 4, 3, 0);
        assert!(job.is_due(due));

        job.last_run_at = Some(due.epoch_minutes);
        assert!(!job.is_due(due), "it fired twice in the same minute");

        // The next day's run is a different minute, so it is due again.
        assert!(job.is_due(at(2026, 5, 5, 3, 0)));
        // And a minute it does not name is never due.
        assert!(!job.is_due(at(2026, 5, 5, 3, 1)));
    }

    /// Switched off is switched off, whatever the clock says.
    #[test]
    fn a_disabled_job_is_never_due() {
        let mut job = nightly();
        job.enabled = false;
        assert!(!job.is_due(at(2026, 5, 4, 3, 0)));
        assert!(job.missed_run(at(2026, 5, 4, 3, 30)).is_none());
    }

    /// A machine that was away owes one run, not a backlog.
    ///
    /// The grace window bounds how late is worth making up, and only the most
    /// recent missed minute is returned — a laptop shut for a week owes its
    /// owner one nightly run, not seven.
    #[test]
    fn a_missed_run_is_made_up_once_and_only_within_the_grace() {
        let job = nightly();
        // Woken 30 minutes after the 03:00 run, with an hour of grace.
        let missed = job
            .missed_run(at(2026, 5, 4, 3, 30))
            .expect("the run half an hour ago was inside the grace window");
        assert_eq!(missed, at(2026, 5, 4, 3, 0));

        // Woken two hours late: outside the hour of grace, so nothing is owed.
        assert!(job.missed_run(at(2026, 5, 4, 5, 0)).is_none());

        // An hourly job that was away for three hours owes exactly one run.
        let mut hourly = nightly();
        hourly.schedule = Schedule {
            cadence: Cadence::Hourly,
            time: "00:00".into(),
            day_of_week: 1,
            custom: String::new(),
        };
        hourly.missed_run_grace_minutes = 24 * 60;
        let owed = hourly.missed_run(at(2026, 5, 4, 3, 30)).expect("one owed");
        assert_eq!(
            owed,
            at(2026, 5, 4, 3, 0),
            "it owed an older run than the last"
        );
    }

    /// A run already recorded is not owed again.
    #[test]
    fn a_run_that_already_happened_is_not_missed() {
        let mut job = nightly();
        job.last_run_at = Some(at(2026, 5, 4, 3, 0).epoch_minutes);
        assert!(
            job.missed_run(at(2026, 5, 4, 3, 30)).is_none(),
            "the run it already made was offered again"
        );
    }

    /// The next occurrence agrees with the matcher that found it.
    #[test]
    fn the_next_run_is_a_minute_the_expression_names() {
        let workday = Cron::parse("0 9 * * 1-5").expect("parse");
        // Saturday afternoon: the next one is Monday morning.
        let next = workday
            .next_after(at(2026, 5, 2, 14, 0))
            .expect("within a year");
        assert_eq!(next, at(2026, 5, 4, 9, 0));
        assert!(workday.matches(next));

        // Asked at exactly the due minute, the answer is that minute.
        assert_eq!(workday.next_after(next), Some(next));

        // An expression that can never match answers nothing rather than
        // looping — 30 February.
        assert!(
            Cron::parse("0 0 30 2 *")
                .expect("parse")
                .next_after(next)
                .is_none()
        );
    }

    #[test]
    fn a_consumed_current_minute_is_not_reported_as_the_next_run() {
        let mut job = nightly();
        let due = at(2026, 5, 4, 3, 0);
        assert_eq!(job.next_unconsumed_run(due), Some(due));

        job.last_run_at = Some(due.epoch_minutes);
        assert_eq!(
            job.next_unconsumed_run(due),
            Some(at(2026, 5, 5, 3, 0)),
            "the occurrence that just ran was still advertised as next"
        );
    }

    /// One run of a job, numbered so a ledger reads in order.
    fn ran(job: &str, nth: i64) -> AutomationRun {
        AutomationRun {
            id: format!("{job}-{nth}"),
            automation_id: job.to_string(),
            scheduled_for_epoch_minutes: Some(nth),
            at_epoch_ms: 1_700_000_000_000 + nth,
            root: format!("/checkouts/{job}-{nth}"),
            term: nth as u32,
            made_worktree: None,
            skipped: None,
            failure: None,
            // Started and not yet seen to stop, which is what every row in a
            // ledger looks like for as long as its shell is open.
            ended_at_epoch_ms: None,
            exit_code: None,
            completed_at_epoch_ms: None,
            result: None,
            evidence_dir: None,
            // The shell's fingerprint: the fixture makes it from the row's
            // ordinal so two rows on one terminal id are two shells.
            launch: Some(1_000 + nth as u64),
            unwitnessed: false,
        }
    }

    fn finished(job: &str, nth: i64) -> AutomationRun {
        AutomationRun {
            ended_at_epoch_ms: Some(1_700_000_100_000 + nth),
            exit_code: Some(0),
            ..ran(job, nth)
        }
    }

    /// The cap belongs to one job, and forgetting starts at that job's oldest.
    ///
    /// The boundary is the whole test: filling the cap exactly must forget
    /// nothing, and the run after it must forget exactly one — the first. And
    /// a busy schedule must not evict a quiet one: a `NewPerRun` job firing
    /// hourly would otherwise be the only job in the window with any past,
    /// while the weekly job somebody actually opens shows nothing.
    #[test]
    fn the_cap_forgets_this_jobs_oldest_and_nobody_elses() {
        let mut ledger = vec![finished("weekly", 0)];
        for nth in 1..=3 {
            record_run(&mut ledger, finished("nightly", nth), 3);
        }
        assert_eq!(
            ledger.len(),
            4,
            "a cap filled exactly to the brim forgot something"
        );
        assert_eq!(
            ledger
                .iter()
                .filter(|run| run.automation_id == "nightly")
                .count(),
            3
        );

        record_run(&mut ledger, finished("nightly", 4), 3);
        let nightly: Vec<&str> = ledger
            .iter()
            .filter(|run| run.automation_id == "nightly")
            .map(|run| run.id.as_str())
            .collect();
        assert_eq!(
            nightly,
            ["nightly-2", "nightly-3", "nightly-4"],
            "one past the cap forgot something other than this job's oldest"
        );
        assert!(
            ledger.iter().any(|run| run.id == "weekly-0"),
            "a busy job pushed a quiet job's only run out of the ledger"
        );

        // And far past the cap, from a job that was never near it.
        for nth in 5..=12 {
            record_run(&mut ledger, finished("nightly", nth), 3);
        }
        assert_eq!(
            ledger.len(),
            4,
            "the ledger grew past one job's cap plus one"
        );
        assert!(ledger.iter().any(|run| run.id == "weekly-0"));
    }

    #[test]
    fn retention_keeps_every_unfinished_run_beside_the_newest_final_hundred() {
        let mut ledger = Vec::new();
        for nth in 0..=RUNS_KEPT_PER_AUTOMATION as i64 {
            record_run(
                &mut ledger,
                finished("nightly", nth),
                RUNS_KEPT_PER_AUTOMATION,
            );
        }
        let live = ran("nightly", 10_000);
        record_run(&mut ledger, live.clone(), RUNS_KEPT_PER_AUTOMATION);

        assert_eq!(
            ledger
                .iter()
                .filter(|run| run.automation_id == "nightly" && run_is_final(run))
                .count(),
            100
        );
        assert!(
            ledger.iter().any(|run| run.id == live.id),
            "retention pruned a run whose terminal ending was never witnessed"
        );
        assert!(ledger.iter().all(|run| run.id != "nightly-0"));
    }

    /// The ledger is the provenance, and it names each checkout once.
    #[test]
    fn the_born_checkouts_are_named_once_each() {
        let cut = |job: &str, nth: i64, path: &str| AutomationRun {
            made_worktree: Some(path.to_string()),
            ..ran(job, nth)
        };
        let ledger = vec![
            ran("nightly", 1),
            cut("nightly", 2, "/wt/a"),
            cut("weekly", 3, "/wt/b"),
            // The same checkout reported twice — a job re-cutting the same
            // branch name, or two windows' ledgers merged.
            cut("nightly", 4, "/wt/a"),
        ];
        assert_eq!(
            born_worktrees(&ledger),
            ["/wt/a", "/wt/b"],
            "a checkout was named twice, or a run that cut nothing was counted"
        );
        assert!(born_worktrees(&[ran("nightly", 1)]).is_empty());
    }

    /// A precheck passes on a clean zero and on nothing else.
    ///
    /// Orca's `didAutomationPrecheckPass` is one boolean with three clauses,
    /// and every clause is a way the answer can be *unknown* rather than good.
    /// The gate exists to fail closed: if breaking the precheck command let
    /// the job run anyway, the guard would protect exactly the machines where
    /// it is working and abandon the ones where it is not.
    #[test]
    fn a_precheck_passes_only_on_a_clean_zero() {
        let ended = |code: Option<i32>, timed_out: bool, errored: bool| PrecheckOutcome {
            exit_code: code,
            timed_out,
            errored,
        };
        assert!(ended(Some(0), false, false).passed());

        // A non-zero exit is the ordinary "no" — the tests are red, the
        // branch moved, the disk is full.
        assert!(!ended(Some(1), false, false).passed());
        assert!(!ended(Some(127), false, false).passed());
        // Signals arrive as a negative code on the platforms that report one.
        assert!(!ended(Some(-9), false, false).passed());

        // A run that was killed at its deadline says nothing about the tree,
        // and the code it left behind — if any — is not an answer.
        assert!(!ended(None, true, false).passed());
        assert!(
            !ended(Some(0), true, false).passed(),
            "a timeout that raced a zero exit read as a pass"
        );

        // Neither is a shell that never started: a typo in the command must
        // not read as permission.
        assert!(!ended(None, false, true).passed());
        assert!(
            !ended(Some(0), false, true).passed(),
            "a shell that could not be started read as a pass"
        );

        // And no code at all is not zero.
        assert!(!ended(None, false, false).passed());
    }

    /// Only a scheduled firing asks its precheck, and only when there is one.
    #[test]
    fn only_a_scheduled_run_asks_its_precheck() {
        assert!(should_run_precheck(RunTrigger::Scheduled, "just test"));

        // The by-hand door is never gated. Somebody pressing 지금 실행 has
        // already decided, with the screen in front of them.
        assert!(
            !should_run_precheck(RunTrigger::Manual, "just test"),
            "a by-hand run was refused by a guard written for 3am"
        );

        // No command is no gate, on either door — and a field holding only
        // whitespace is no command, rather than a shell run of nothing that
        // exits 0 and passes everything forever.
        assert!(!should_run_precheck(RunTrigger::Scheduled, ""));
        assert!(!should_run_precheck(RunTrigger::Scheduled, "  \n\t "));
        assert!(!should_run_precheck(RunTrigger::Manual, ""));
    }

    /// A checkout cut this minute has no session to continue.
    #[test]
    fn a_fresh_checkout_has_no_session_to_reuse() {
        assert!(reuse_normalized(WorkspaceMode::Existing, true));
        assert!(!reuse_normalized(WorkspaceMode::Existing, false));

        // The pair that has to be normalised away. A worktree cut seconds ago
        // has never held an agent, so "reuse" there can only mean starting one
        // under a misleading name.
        assert!(
            !reuse_normalized(WorkspaceMode::NewPerRun, true),
            "a per-run checkout claimed a session it cannot have"
        );
        assert!(!reuse_normalized(WorkspaceMode::NewPerRun, false));
    }

    /// A skipped run is a run in the ledger, and it loads beside older ones.
    ///
    /// The compatibility half matters as much as the shape: `automation-runs
    /// .json` on a machine that has been running this window for weeks has no
    /// `skipped` key on any row, and a ledger that stopped parsing would take
    /// every job's whole history with it.
    #[test]
    fn a_gated_run_is_written_down_and_an_older_ledger_still_reads() {
        let older = r#"[{"id":"1-7","automation_id":"nightly","at_epoch_ms":1,
            "root":"/wt/a","term":7,"made_worktree":null}]"#;
        let loaded: Vec<AutomationRun> = serde_json::from_str(older).expect("an older ledger");
        assert_eq!(loaded[0].scheduled_for_epoch_minutes, None);
        assert_eq!(loaded[0].skipped, None, "an older run read as skipped");
        assert_eq!(loaded[0].failure, None, "an older run read as failed");

        let gated = AutomationRun {
            skipped: Some(PrecheckRecord {
                command: "just test".to_string(),
                exit_code: Some(1),
                timed_out: false,
                duration_ms: 120,
                output: "2 failed".to_string(),
            }),
            ..ran("nightly", 2)
        };
        let mut ledger = loaded;
        record_run(&mut ledger, gated.clone(), RUNS_KEPT_PER_AUTOMATION);
        assert_eq!(ledger.len(), 2, "a gated run was not kept");

        // And it survives the round trip through the file it lives in.
        let text = serde_json::to_string(&ledger).expect("write");
        let back: Vec<AutomationRun> = serde_json::from_str(&text).expect("read");
        assert_eq!(back[1], gated);
        assert!(occurrence_recorded(
            &back,
            "nightly",
            LocalMinute { epoch_minutes: 2 },
        ));
        // A gated run cut nothing, so it is not provenance for any checkout.
        assert!(born_worktrees(&back).is_empty());
    }

    #[test]
    fn a_failed_run_fences_its_scheduled_occurrence_and_keeps_cleanup_evidence() {
        let due = at(2026, 5, 4, 3, 0);
        let failed = AutomationRun {
            scheduled_for_epoch_minutes: Some(due.epoch_minutes),
            made_worktree: Some("/wt/nightly".into()),
            failure: Some(AutomationFailureRecord {
                stage: AutomationFailureStage::Spawn,
                scheduled_for_epoch_minutes: due.epoch_minutes,
                error: "program not found".into(),
                worktree_cleanup_error: Some("worktree is no longer clean".into()),
            }),
            ended_at_epoch_ms: Some(1_700_000_000_100),
            ..ran("nightly", 2)
        };
        let ledger = vec![failed.clone()];
        assert!(occurrence_recorded(&ledger, "nightly", due));
        assert!(failed_occurrence_recorded(&ledger, "nightly", due));
        assert!(!failed_occurrence_recorded(
            &ledger,
            "nightly",
            LocalMinute {
                epoch_minutes: due.epoch_minutes + 1,
            }
        ));

        let text = serde_json::to_string(&ledger).expect("serialize failure");
        let back: Vec<AutomationRun> = serde_json::from_str(&text).expect("read failure");
        assert_eq!(back, ledger);
        assert_eq!(born_worktrees(&back), ["/wt/nightly"]);
    }

    /// A shell ending stamps the run that opened it, and nothing else.
    ///
    /// Terminal ids come back around: one number names a series of shells over
    /// a window's life, so "the run on term 7" is a question with several
    /// answers and only the last of them is the one that just exited. Dating
    /// last month's run by this afternoon's exit is the failure this orders
    /// against, and it is invisible in a ledger — the row simply reads wrong
    /// forever.
    #[test]
    fn the_end_lands_on_the_newest_run_of_that_terminal_and_only_once() {
        let on_term = |nth: i64, term: u32| AutomationRun {
            term,
            ..ran("nightly", nth)
        };
        let mut ledger = vec![on_term(1, 7), on_term(2, 9), on_term(3, 7)];

        // The exiting shell is named by its fingerprint as well as its id.
        assert!(mark_ended(
            &mut ledger,
            7,
            Some(1_003),
            1_800_000_000_000,
            Some(0)
        ));
        assert_eq!(
            ledger[2].ended_at_epoch_ms,
            Some(1_800_000_000_000),
            "the shell that just exited left its run open"
        );
        assert_eq!(ledger[2].exit_code, Some(0));
        assert_eq!(
            ledger[0].ended_at_epoch_ms, None,
            "an older run of the same terminal was dated by a later shell's exit"
        );
        assert_eq!(ledger[1].ended_at_epoch_ms, None, "another run was ended");

        // A row that already carries an ending is finished. A second exit on
        // the same id belongs to a second shell, and letting it through would
        // overwrite a measured moment with a coincidence.
        assert!(
            !mark_ended(&mut ledger, 7, Some(1_003), 1_900_000_000_000, Some(1)),
            "a run that had already ended was ended again"
        );
        assert_eq!(ledger[2].ended_at_epoch_ms, Some(1_800_000_000_000));
        assert_eq!(ledger[2].exit_code, Some(0));
        assert_eq!(
            ledger[0].ended_at_epoch_ms, None,
            "a second exit on a reused id fell through to a run that ended \
             before the window was last quit"
        );

        // A terminal nobody scheduled anything on: most shells in this window
        // are somebody's own, and the answer is what keeps the file unwritten.
        let mut ordinary = vec![on_term(4, 11)];
        assert!(
            !mark_ended(&mut ordinary, 12, Some(1_004), 1_800_000_000_000, Some(0)),
            "an ending was claimed for a terminal no run ever opened"
        );
        assert_eq!(ordinary[0].ended_at_epoch_ms, None);

        // A code that could not be read stays unread. Absent is not zero, the
        // same rule the precheck verdict is built on.
        let mut unread = vec![on_term(5, 21)];
        assert!(mark_ended(
            &mut unread,
            21,
            Some(1_005),
            1_800_000_000_001,
            None
        ));
        assert_eq!(unread[0].ended_at_epoch_ms, Some(1_800_000_000_001));
        assert_eq!(
            unread[0].exit_code, None,
            "an unreadable exit code was written down as a clean zero"
        );

        // And a firing the precheck stopped is never a candidate: it opened no
        // shell, its `term` is a placeholder, and it is already final.
        let mut stopped = vec![AutomationRun {
            term: 0,
            skipped: Some(PrecheckRecord {
                command: "just test".to_string(),
                exit_code: Some(1),
                timed_out: false,
                duration_ms: 40,
                output: String::new(),
            }),
            ..ran("nightly", 6)
        }];
        assert!(
            !mark_ended(&mut stopped, 0, Some(1_006), 1_800_000_000_000, Some(0)),
            "a stopped firing was given the ending of somebody else's shell"
        );
        assert_eq!(stopped[0].ended_at_epoch_ms, None);
    }

    /// A ledger written before runs had an ending still loads, whole.
    ///
    /// The rows on a machine that has been running this window for weeks have
    /// neither key, and a ledger that stopped parsing would take every job's
    /// entire history with it — the file is read with `.ok()` and an
    /// unreadable one answers empty, so the loss would be silent.
    ///
    /// The sharper half is WHAT an old row reads as. A helpful default — an
    /// ending at the epoch, a zero exit — would load every history ever
    /// written and quietly declare all of it finished, which no reader could
    /// tell from the truth.
    #[test]
    fn a_ledger_written_before_the_ending_was_recorded_still_loads() {
        let older = r#"[{"id":"1-7","automation_id":"nightly","at_epoch_ms":1,
            "root":"/wt/a","term":7,"made_worktree":null,"skipped":null}]"#;
        let mut loaded: Vec<AutomationRun> = serde_json::from_str(older).expect("an older ledger");
        assert_eq!(
            loaded[0].ended_at_epoch_ms, None,
            "a run from before the field read as ended"
        );
        assert_eq!(loaded[0].exit_code, None);

        assert_eq!(
            loaded[0].launch, None,
            "a run from before the field wears a fingerprint"
        );
        assert!(!loaded[0].unwitnessed);

        // Such a row names no shell any more: the one it opened cannot be told
        // from the one that inherited its id, so an exit on that id is not
        // its ending. What closes it is the boot sweep, and the file says so.
        assert!(
            !mark_ended(&mut loaded, 7, Some(7_001), 1_800_000_000_000, Some(2)),
            "a row without a fingerprint took a later shell's exit"
        );
        assert_eq!(
            close_unwitnessed(&mut loaded, 1_800_000_000_000, |_| false),
            1
        );
        let text = serde_json::to_string(&loaded).expect("write");
        let back: Vec<AutomationRun> = serde_json::from_str(&text).expect("read");
        assert_eq!(back[0].ended_at_epoch_ms, Some(1_800_000_000_000));
        assert_eq!(
            back[0].exit_code, None,
            "an unwitnessed ending invented a code"
        );
        assert!(back[0].unwitnessed);
    }

    /// A `done` from the run's shell completes the row wearing that shell's
    /// terminal AND fingerprint, keeps the last thing the agent said, and may
    /// happen again; a terminal nobody's run opened completes nothing.
    #[test]
    fn a_completion_lands_on_the_row_of_its_shell_and_may_repeat() {
        let mut runs = vec![
            AutomationRun {
                id: "old".into(),
                launch: Some(1),
                ..ran("job", 7)
            },
            AutomationRun {
                id: "new".into(),
                launch: Some(2),
                ..ran("job", 7)
            },
        ];
        assert_eq!(
            mark_completed(&mut runs, 7, Some(2), 100, Some("first answer".to_string())),
            Some(("job".to_string(), "new".to_string()))
        );
        assert_eq!(runs[1].completed_at_epoch_ms, Some(100));
        assert_eq!(runs[1].result.as_deref(), Some("first answer"));
        assert!(
            runs[0].completed_at_epoch_ms.is_none(),
            "the older row is not reached"
        );
        // A second turn completes again; a silent completion keeps the words.
        assert!(mark_completed(&mut runs, 7, Some(2), 200, None).is_some());
        assert_eq!(runs[1].completed_at_epoch_ms, Some(200));
        assert_eq!(runs[1].result.as_deref(), Some("first answer"));
        assert_eq!(
            mark_completed(&mut runs, 9, Some(2), 300, None),
            None,
            "nobody's shell"
        );
        assert_eq!(
            mark_completed(&mut runs, 7, None, 300, None),
            None,
            "a shell the window holds no fingerprint for is nobody's run"
        );
        // The older shell, still alive, completes its own row — not the newest.
        assert_eq!(
            mark_completed(&mut runs, 7, Some(1), 400, Some("late".to_string())),
            Some(("job".to_string(), "old".to_string()))
        );
        assert_eq!(runs[0].result.as_deref(), Some("late"));
    }

    /// The next occupant of a terminal id is not the run that opened it.
    ///
    /// Measured 2026-09-05 in the window's own ledger: an Obsidian job's run
    /// of 9월 3일 19:00 on term 13 was stamped "completed" the next afternoon
    /// with a Claude Explore agent's report ("Sending the report now. PART 1 —
    /// IDE side…"), because that agent's pane had been given id 13 after the
    /// job's shell closed and the row was the newest one naming the id; the
    /// job's `close_when_done` then shut the agent's pane. A row from 8월 8일
    /// on term 2 took a coordinator's words the same way. The fingerprint is
    /// what tells the shells apart.
    #[test]
    fn a_done_from_the_next_occupant_of_a_terminal_id_lands_on_no_older_run() {
        let mut runs = vec![AutomationRun {
            id: "1788429600033-13".into(),
            term: 13,
            launch: Some(13_001),
            ..ran("obsidian", 1)
        }];
        let later_shell = Some(13_777);
        assert_eq!(
            mark_completed(
                &mut runs,
                13,
                later_shell,
                1_788_501_791_883,
                Some("I have what I need. Sending the report now.".to_string()),
            ),
            None,
            "another pane's done landed on an older run of its terminal id"
        );
        assert_eq!(runs[0].completed_at_epoch_ms, None);
        assert_eq!(runs[0].result, None);
        assert!(
            !mark_ended(&mut runs, 13, later_shell, 1_788_501_800_000, Some(0)),
            "another pane's exit ended an older run of its terminal id"
        );
        assert_eq!(runs[0].ended_at_epoch_ms, None);
        // Its own shell still completes it.
        assert!(mark_completed(&mut runs, 13, Some(13_001), 1_788_430_000_000, None).is_some());
        // And once ended, no completion is written — the shell is gone.
        assert!(mark_ended(
            &mut runs,
            13,
            Some(13_001),
            1_788_430_100_000,
            Some(0)
        ));
        assert_eq!(
            mark_completed(
                &mut runs,
                13,
                Some(13_001),
                1_788_430_200_000,
                Some("late".into())
            ),
            None
        );
    }

    /// The boot sweep closes every run nobody saw end and spares the shells
    /// this window holds.
    ///
    /// A restart ends every pane's child while no window is listening, so a
    /// row left open by the last window would read "running" forever
    /// (measured 2026-09-05: a run of 9월 4일 19:11 still "실행 중" the next
    /// evening). The sweep is honest about what it knows: an ending at the
    /// sweep's clock, no exit code, and the `unwitnessed` mark.
    #[test]
    fn the_boot_sweep_closes_every_run_nobody_saw_end_and_spares_live_shells() {
        let mut runs = vec![
            AutomationRun {
                id: "stale".into(),
                term: 37,
                ..ran("obsidian", 1)
            },
            AutomationRun {
                id: "live".into(),
                term: 5,
                ..ran("obsidian", 2)
            },
            finished("obsidian", 3),
            AutomationRun {
                term: 0,
                skipped: Some(PrecheckRecord {
                    command: "just test".to_string(),
                    exit_code: Some(1),
                    timed_out: false,
                    duration_ms: 40,
                    output: String::new(),
                }),
                ..ran("obsidian", 4)
            },
            AutomationRun {
                id: "legacy".into(),
                term: 2,
                launch: None,
                ..ran("obsidian", 5)
            },
        ];
        let now = 1_788_600_000_000;
        assert_eq!(close_unwitnessed(&mut runs, now, |term| term == 5), 2);
        for id in ["stale", "legacy"] {
            let row = runs.iter().find(|run| run.id == id).expect(id);
            assert_eq!(row.ended_at_epoch_ms, Some(now), "{id} stayed open");
            assert_eq!(row.exit_code, None, "{id} was given a code nobody read");
            assert!(
                row.unwitnessed,
                "{id} does not say its ending was unwitnessed"
            );
        }
        let live = &runs[1];
        assert_eq!(
            live.ended_at_epoch_ms, None,
            "a shell this window holds was closed"
        );
        assert!(!live.unwitnessed);
        assert_eq!(runs[2].ended_at_epoch_ms, Some(1_700_000_100_003));
        assert!(!runs[2].unwitnessed, "a witnessed ending was rewritten");
        assert_eq!(
            runs[3].ended_at_epoch_ms, None,
            "a skipped firing was closed"
        );
        // Sweeping again finds nothing: the rows are final now, and the live
        // shell is still this window's.
        assert_eq!(close_unwitnessed(&mut runs, now + 1, |term| term == 5), 0);

        // The mark survives the file, and a row that never needed it does not
        // carry the key — an older window reading this ledger sees its own
        // shape for every witnessed row.
        let text = serde_json::to_string(&runs).expect("write");
        let back: Vec<AutomationRun> = serde_json::from_str(&text).expect("read");
        assert_eq!(back, runs);
        let witnessed_only = serde_json::to_string(&[finished("obsidian", 9)]).expect("write");
        assert!(!witnessed_only.contains("unwitnessed"));
    }

    /// The prompt decides evidence unless the job overrides it: words for
    /// proof arm it, a "without" disarms it, and a plain prompt just runs.
    #[test]
    fn the_prompt_decides_whether_a_run_leaves_evidence() {
        assert!(evidence_requested(
            "Wallet 앱 실행해서 송금까지 QA 테스트하고 스크린샷 남겨"
        ));
        assert!(evidence_requested(
            "Run the checkout flow and keep evidence of every step"
        ));
        assert!(evidence_requested("각 단계를 캡쳐해서 증빙으로"));
        assert!(!evidence_requested("송금 흐름 QA 테스트해 줘"));
        assert!(!evidence_requested("QA 테스트하되 스크린샷은 남기지 마"));
        assert!(!evidence_requested("test the flow without screenshots"));
        let mut job = nightly();
        job.prompt = "그냥 돌려".into();
        assert!(!leaves_evidence(&job));
        job.evidence = EvidencePolicy::Always;
        assert!(leaves_evidence(&job));
        job.prompt = "스크린샷 남겨".into();
        job.evidence = EvidencePolicy::Never;
        assert!(!leaves_evidence(&job));
        job.evidence = EvidencePolicy::Prompt;
        assert!(leaves_evidence(&job));
    }

    /// Pruning hands back the rows it forgot, so their evidence can go too.
    #[test]
    fn pruning_hands_back_the_rows_it_forgot() {
        let mut runs: Vec<AutomationRun> = (0..3).map(|nth| finished("job", nth)).collect();
        let forgotten = prune_final_runs(&mut runs, 2);
        assert_eq!(
            forgotten
                .iter()
                .map(|run| run.id.as_str())
                .collect::<Vec<_>>(),
            vec!["job-0"]
        );
        assert_eq!(runs.len(), 2);
    }
}
