//! Which project a run belongs to, and what a pane of another project may do
//! with it (t-65286).
//!
//! Every project a window opens shares one ledger. A run is one project's
//! work — its coordinator, its workers' checkouts and its mail live in one
//! repository — and nothing said so: any pane could answer any run with
//! `--run <id>`, sit in its empty seat, and be handed its mail pointer. On
//! 2026-10-16 two coordinator panes of one project sent seven letters into
//! another project's run that way, and on 2026-10-10 one pane held the seats
//! of thirteen runs across two projects.
//!
//! The project is the git directory a repository's worktrees share
//! ([`crate::git_dir::project_root`]), so every worktree of one repository is
//! one project and two runs of one project still write to each other. A run
//! made from now on is told its project by the pane that made it. A run made
//! before is INFERRED one from its own rows. That boundary already holds;
//! its live coordinator can confirm it from a pane of the same project.
//! An unscoped legacy run is visible only to its own seated participants.
//! Each project carries
//! how it was learned, when, and from which seat, so "why is this run that
//! project's" has an answer in the ledger itself.

use super::{
    BOOL_FLAGS, Ledger, QUESTION_CLOSE, Run, WORKTREE_EVIDENCE_VERB, run_holding, split_words,
    standing_in,
};

/// The longest project root a run may hold. A root is a path, and a platform
/// bounds its paths well inside this.
pub const PROJECT_ROOT_MAX: usize = 4096;

/// The verbs that answer for the whole window rather than in one run. What
/// they read or change is every run's, so no single run's project bounds
/// them; the window holds them to the caller's project on its own side.
pub const WINDOW_VERBS: &[&str] = &[
    "help",
    "run-create",
    "run-list",
    "retention",
    "agent-list",
    "inbox",
    "reset",
];

/// How a run's project was learned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectSource {
    /// Said by the pane that made the run, at `run-create`.
    Created,
    /// Read off the checkouts most of the run's workers were seated in.
    Workers,
    /// Read off the pane the run's coordinator sits in.
    Seat,
    /// Read off the run's rows, then confirmed by its live coordinator with a
    /// verb typed in its own pane — a pane of this project.
    Confirmed,
}

impl ProjectSource {
    /// Whether the project was read off the run's rows rather than confirmed.
    #[must_use]
    pub const fn inferred(self) -> bool {
        matches!(self, Self::Workers | Self::Seat)
    }

    /// The word the ledger stores and a reader is shown.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Workers => "workers",
            Self::Seat => "seat",
            Self::Confirmed => "confirmed",
        }
    }
}

/// The project a run belongs to, and how that was learned.
#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunProject {
    /// The project's root, as [`crate::git_dir::project_root`] answers for the
    /// folder it was read from.
    pub root: String,
    pub source: ProjectSource,
    /// When it was written down: made, read off, or confirmed.
    pub at_ms: i64,
    /// The seat (`team/pane`) that made the run, confirmed its project, or
    /// whose pane it was read off — none for a project read off the workers'
    /// checkouts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seat: Option<String>,
}

/// Shapes, never the path or the seat: a project root names somebody's
/// folder, and a `dbg!` of a run must not print it.
impl std::fmt::Debug for RunProject {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RunProject")
            .field("root_bytes", &self.root.len())
            .field("source", &self.source)
            .field("at_ms", &self.at_ms)
            .field("named_seat", &self.seat.is_some())
            .finish()
    }
}

impl RunProject {
    /// Whether this project was only read off the run's rows.
    #[must_use]
    pub const fn inferred(&self) -> bool {
        self.source.inferred()
    }

    /// The folder a person knows this project by ([`label`]).
    #[must_use]
    pub fn label(&self) -> String {
        label(&self.root)
    }

    /// The project as `run-show` says it: where, how it was learned, when
    /// and from which seat — the answer to "why is this run that project's".
    #[must_use]
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "folder": self.label(),
            "root": self.root,
            "source": self.source.as_str(),
            "atMs": self.at_ms,
            "seat": self.seat,
        })
    }
}

/// The folder a person knows a project root by: the repository's own folder
/// for a shared git directory, the folder itself for one no checkout holds.
#[must_use]
pub fn label(root: &str) -> String {
    let path = std::path::Path::new(root);
    match (path.file_name(), path.parent()) {
        (Some(name), Some(parent)) if name == std::ffi::OsStr::new(".git") => {
            parent.display().to_string()
        }
        _ => root.to_string(),
    }
}

/// Where a run stands to a pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope<'a> {
    /// Nothing to compare: the run has no project written down, or the pane
    /// sits in no folder the window recorded.
    Unknown,
    /// The pane works in the run's project.
    Same,
    /// The pane works in another project than the run's — this one.
    Crossed(&'a RunProject),
}

impl Scope<'_> {
    /// A different project is a boundary even before its coordinator confirms
    /// an inferred project. Missing confirmation is not permission to cross.
    #[must_use]
    pub fn holds(&self) -> bool {
        matches!(self, Self::Crossed(_))
    }
}

impl Run {
    /// Where this run stands to a pane working in the project `root`
    /// ([`crate::git_dir::project_root`]) — `None` for a pane in no folder
    /// the window knows.
    #[must_use]
    pub fn scope(&self, root: Option<&str>) -> Scope<'_> {
        match (self.project.as_ref(), root) {
            (Some(project), Some(root)) if project.root == root => Scope::Same,
            (Some(project), Some(_)) => Scope::Crossed(project),
            _ => Scope::Unknown,
        }
    }
}

/// Whether `root` could be a project root at all.
fn a_root(root: &str) -> bool {
    !root.is_empty() && root.len() <= PROJECT_ROOT_MAX
}

impl Ledger {
    /// Write down the project the pane that made `run` works in, from its
    /// seat — for a run that holds none. Answers whether the run moved.
    pub fn state_run_project(&mut self, run: &str, root: &str, seat: &str, now_ms: i64) -> bool {
        if !a_root(root) {
            return false;
        }
        let Some(run) = self.run_mut(run) else {
            return false;
        };
        if run.project.is_some() {
            return false;
        }
        run.project = Some(RunProject {
            root: root.to_string(),
            source: ProjectSource::Created,
            at_ms: now_ms,
            seat: Some(seat.to_string()),
        });
        true
    }

    /// Write down the project a run made before projects were recorded was
    /// read off as — only for a run that holds none, and only as one of the
    /// two readings ([`ProjectSource::inferred`]). Answers whether the run
    /// moved.
    pub fn infer_run_project(
        &mut self,
        run: &str,
        root: &str,
        source: ProjectSource,
        seat: Option<&str>,
        now_ms: i64,
    ) -> bool {
        if !a_root(root) || !source.inferred() {
            return false;
        }
        let Some(run) = self.run_mut(run) else {
            return false;
        };
        if run.project.is_some() {
            return false;
        }
        run.project = Some(RunProject {
            root: root.to_string(),
            source,
            at_ms: now_ms,
            seat: seat.map(str::to_string),
        });
        true
    }

    /// Confirm an inferred project: the run's LIVE coordinator, in `seat`,
    /// typed a verb from a pane of the project `root` — and it is the project
    /// the run was read off as. Any other pane, a worker's included, and any
    /// other project confirms nothing. Answers whether the run moved.
    pub fn confirm_run_project(&mut self, run: &str, root: &str, seat: &str, now_ms: i64) -> bool {
        let Some(run) = self.run_mut(run) else {
            return false;
        };
        let inferred_here = run
            .project
            .as_ref()
            .is_some_and(|project| project.inferred() && project.root == root);
        if !inferred_here || run.seat_is_coordinator(seat) != Some(true) {
            return false;
        }
        run.project = Some(RunProject {
            root: root.to_string(),
            source: ProjectSource::Confirmed,
            at_ms: now_ms,
            seat: Some(seat.to_string()),
        });
        true
    }
}

/// The run a verb from the pane `team/pane` answers in, by the rule the plan
/// reads it with: `--run`, the id `run-use` and `run-takeover` name, or the
/// run the caller is bound to. `None` for a verb that answers for the whole
/// window ([`WINDOW_VERBS`]), for the bare `worktree-evidence` (it reads the
/// caller's own checkout, which is no run's), and for a run this ledger does
/// not hold — the plan refuses that one in its own words.
#[must_use]
pub fn acting_run(
    ledger: &Ledger,
    argv: &[String],
    team: &str,
    pane: &str,
    actor: Option<&str>,
) -> Option<String> {
    let (verb, rest) = argv.split_first()?;
    if WINDOW_VERBS.contains(&verb.as_str()) {
        return None;
    }
    let words = split_words(rest, BOOL_FLAGS);
    if verb == WORKTREE_EVIDENCE_VERB && words.value("--worker").is_none() {
        return None;
    }
    let named = match verb.as_str() {
        "run-use" | "run-takeover" => words
            .positional
            .first()
            .map(String::as_str)
            .or_else(|| words.value("--run")),
        _ => words.value("--run"),
    };
    if let Some(id) = named {
        return ledger.run(id).map(|run| run.id.clone());
    }
    if verb == "run-use" {
        return None;
    }
    let seat = format!("{team}/{pane}");
    let caller = actor
        .filter(|actor| !actor.is_empty())
        .map_or_else(|| seat.clone(), str::to_string);
    let standing = standing_in(ledger, &caller, &seat);
    // A question is closed where it was filed, which for the asking side is
    // another run (t-83289): the plan finds it by id, and so does the check.
    match words.value("--message") {
        Some(id) if verb == QUESTION_CLOSE => {
            run_holding(ledger, standing.as_deref(), id).or(standing)
        }
        _ => standing,
    }
}

/// Where a verb reaches, against the project its pane works in — read in one
/// place, by the one holder of the ledger, so a verb pays for a lookup and not
/// for a copy of the ledger.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reach {
    /// The run the verb answers in ([`acting_run`]), with its project when one
    /// is written down.
    pub run: Option<(String, Option<RunProject>)>,
    /// Whether the caller sits in that run's live coordinator seat.
    pub coordinator: bool,
    /// The caller is a live coordinator or worker of this run. This is the
    /// visibility boundary for legacy rows whose project is not known yet.
    pub participant: bool,
    /// The runs of another project than the caller's that a window verb
    /// reaches — every run `reset` clears, `run-list` lists and `inbox`
    /// reads — each with its project.
    pub elsewhere: Vec<(String, RunProject)>,
    /// Rows a window-wide answer must omit, including unscoped legacy runs
    /// in which the caller has no seat.
    pub hidden: Vec<String>,
    /// Actual recipients of send/ask/reply, resolved by the ledger's own
    /// address rules. The window checks each pane's present project.
    pub recipient_seats: Vec<String>,
    /// The other run a letter is filed in or copied into, with its project:
    /// the writer's run for a reply, the run a `send` to another run's
    /// `run:` address files in, and the run a `pane:` recipient reads its
    /// mail in (t-97078).
    pub carried_run: Option<(String, Option<RunProject>)>,
    /// The project of the run each recipient sits in ([`Ledger::seated_root`]),
    /// for a recipient whose folder the window has no record of, as a restart
    /// leaves one (t-97078). A pane no run seats is not here.
    pub seated_roots: Vec<(String, String)>,
}

impl Reach {
    /// The project of the run `seat` sits in, as [`Self::seated_roots`] holds it.
    #[must_use]
    pub fn seated_root(&self, seat: &str) -> Option<&str> {
        self.seated_roots
            .iter()
            .find(|(held, _)| held == seat)
            .map(|(_, root)| root.as_str())
    }
}

fn participant(run: &Run, team: &str, pane: &str) -> bool {
    run.seat_is_coordinator(&format!("{team}/{pane}")) == Some(true)
        || run.worker_in_pane(team, pane).is_some()
}

/// Scope a list before its limit or cursor is applied. A page of another
/// project's rows must not hide this project's older messages or runs.
#[must_use]
pub fn visible_to(run: &Run, root: Option<&str>, team: &str, pane: &str) -> bool {
    match (run.project.as_ref(), root) {
        // Window-owned/internal reads have no pane context. Pane commands
        // always carry Some(root), or Some("") for an unidentified project.
        (_, None) => true,
        (Some(project), Some(root)) => !root.is_empty() && project.root == root,
        (None, _) => participant(run, team, pane),
    }
}

/// Recheck the run inside the atomic ledger command. A legacy run can acquire
/// its project between the window's preflight and the command being applied.
pub fn require_project(
    ledger: &Ledger,
    argv: &[String],
    team: &str,
    pane: &str,
    actor: Option<&str>,
    root: Option<&str>,
) -> Result<(), String> {
    // Internal ledger operations have no pane scope. The window explicitly
    // marks an unidentified pane with an empty scope, never None.
    let Some(root) = root else {
        return Ok(());
    };
    let Some(run) = acting_run(ledger, argv, team, pane, actor).and_then(|id| ledger.run(&id))
    else {
        return Ok(());
    };
    if let Some(project) = run.project.as_ref().filter(|project| project.root != root) {
        return Err(refused_across(argv, run, project, root));
    }
    require_carried_project(ledger, argv, run, root)
}

/// [`refusal`] for a verb `argv` from a pane working in `root` — the empty
/// root the window gives a pane in no folder it knows said as such.
fn refused_across(argv: &[String], run: &Run, project: &RunProject, root: &str) -> String {
    refusal(
        argv.first().map_or("", String::as_str),
        &run.id,
        project,
        Some(root)
            .filter(|root| !root.is_empty())
            .unwrap_or("an unidentified project"),
    )
}

/// [`require_project`] for the run a letter is filed in or copied into
/// ([`carried_into`]), asked again inside the atomic ledger command (t-98182
/// L2, review t-97475 P3-4): the window's [`reach`] read it a moment before,
/// and that run can learn another project — or the pane a `pane:` letter
/// follows can sit down in another project's run — before the letter is
/// written. A run of another project is refused as the window refuses it; a
/// run with no project written down is left to the window, which judged its
/// recipients' own folders.
fn require_carried_project(
    ledger: &Ledger,
    argv: &[String],
    run: &Run,
    root: &str,
) -> Result<(), String> {
    let Some(there) = carried_into(ledger, argv, run) else {
        return Ok(());
    };
    match there.project.as_ref() {
        Some(project) if project.root != root => Err(refused_across(argv, there, project, root)),
        _ => Ok(()),
    }
}

/// The other run a letter `argv` writes from `run` is filed in or copied into
/// (t-97078): the writer's run for a `reply`, the run a `send` to another
/// run's `run:` address files in, the run a `pane:` recipient reads its
/// mail in, and the run that asked a question `question-close` closes.
/// Read here once, for the window's look ([`reach`]) and the ledger's write
/// ([`require_project`]) alike.
fn carried_into<'a>(ledger: &'a Ledger, argv: &[String], run: &Run) -> Option<&'a Run> {
    let (verb, rest) = argv.split_first()?;
    let words = split_words(rest, BOOL_FLAGS);
    match verb.as_str() {
        "reply" => words
            .value("--to-message")
            .and_then(|id| run.message(id))
            .and_then(|answered| ledger.asker_run(answered, &run.id))
            .and_then(|id| ledger.run(&id)),
        "send" => {
            let target = words.value("--to")?;
            let there = match target.strip_prefix(super::PANE_ADDRESS_PREFIX) {
                Some(seat) => ledger.seat_reader(seat).map(|(there, _)| there)?,
                None => target.strip_prefix(super::RUN_ADDRESS_PREFIX)?.to_string(),
            };
            ledger.run(&there).filter(|there| there.id != run.id)
        }
        // The close's line is copied into the run that asked (t-98182 r3,
        // review t-99153 P3-1): the closer's reason travels in it.
        QUESTION_CLOSE => words
            .value("--message")
            .and_then(|id| run.message(id))
            .and_then(|question| ledger.asker_run(question, &run.id))
            .and_then(|id| ledger.run(&id)),
        _ => None,
    }
}

/// What a verb from the pane `team/pane`, working in the project `root`,
/// reaches ([`Reach`]).
#[must_use]
pub fn reach(
    ledger: &Ledger,
    argv: &[String],
    team: &str,
    pane: &str,
    actor: Option<&str>,
    root: &str,
) -> Reach {
    let lists = matches!(
        argv.first().map(String::as_str),
        Some("reset" | "run-list" | "inbox")
    );
    let elsewhere = if lists {
        ledger
            .runs()
            .iter()
            .filter_map(|run| match run.scope(Some(root)) {
                Scope::Crossed(project) => Some((run.id.clone(), project.clone())),
                Scope::Same | Scope::Unknown => None,
            })
            .collect()
    } else {
        Vec::new()
    };
    let hidden = if lists {
        ledger
            .runs()
            .iter()
            .filter(|run| !visible_to(run, Some(root), team, pane))
            .map(|run| run.id.clone())
            .collect()
    } else {
        Vec::new()
    };
    let Some(run) = acting_run(ledger, argv, team, pane, actor).and_then(|id| ledger.run(&id))
    else {
        return Reach {
            elsewhere,
            hidden,
            ..Reach::default()
        };
    };
    let words = split_words(&argv[1..], BOOL_FLAGS);
    let target = match argv.first().map(String::as_str) {
        Some("reply") => words
            .value("--to-message")
            .and_then(|id| run.message(id))
            .map(|message| message.from.clone()),
        Some("send") => words.value("--to").map(str::to_string),
        Some("ask") if words.value("--resume").is_none() => Some(
            words
                .value("--to")
                .map_or_else(|| run.address(), str::to_string),
        ),
        _ => None,
    };
    let carried_to = carried_into(ledger, argv, run);
    let carried_run = carried_to.map(|there| (there.id.clone(), there.project.clone()));
    let from = super::sender(
        ledger,
        crate::agent_teams::LEADER_PANE,
        &run.id,
        (team, pane),
    );
    let mut recipient_seats: Vec<String> = target
        .as_deref()
        .and_then(|target| ledger.resolve_address(&run.id, target, &from).ok())
        .unwrap_or_default()
        .iter()
        .filter_map(|address| {
            if let Some(seat) = address.strip_prefix(super::PANE_ADDRESS_PREFIX) {
                return Some(seat.to_string());
            }
            if let Some(id) = address.strip_prefix(super::WORKER_ADDRESS_PREFIX) {
                return run
                    .worker(id)
                    .map(|worker| format!("{}/{}", worker.team, worker.pane));
            }
            if address == &run.address() {
                return run.coordinator_live().map(|chair| chair.seat.clone());
            }
            None
        })
        .collect();
    /* A reply to a pane that sat in the asking run's chair and sits there no
     * more (t-83289): the answer that reaches anybody is the copy that run's
     * chair reads, so that chair is the recipient placed — not the gone pane,
     * which no window can place and which refused every answer to it. */
    if argv.first().is_some_and(|verb| verb == "reply")
        && let Some(asker) = carried_to
        && let Some(gone) = target
            .as_deref()
            .and_then(|target| target.strip_prefix(super::PANE_ADDRESS_PREFIX))
        && asker.seat_is_coordinator(gone) != Some(true)
    {
        recipient_seats.retain(|seat| seat != gone);
        recipient_seats.extend(asker.coordinator_live().map(|chair| chair.seat.clone()));
    }
    // Another run's address reaches whoever sits in its chair (t-97078).
    if let Some(there) = carried_to
        && target.as_deref() == Some(there.address().as_str())
    {
        recipient_seats.extend(there.coordinator_live().map(|chair| chair.seat.clone()));
    }
    let seated_roots = recipient_seats
        .iter()
        .filter_map(|seat| Some((seat.clone(), ledger.seated_root(seat)?)))
        .collect();
    Reach {
        run: Some((run.id.clone(), run.project.clone())),
        coordinator: run.seat_is_coordinator(&format!("{team}/{pane}")) == Some(true),
        participant: participant(run, team, pane),
        elsewhere,
        hidden,
        recipient_seats,
        carried_run,
        seated_roots,
    }
}

/// What a pane of another project is told when the boundary holds: whose run
/// it asked, and where the pane itself works.
#[must_use]
pub fn refusal(verb: &str, run: &str, project: &RunProject, caller_root: &str) -> String {
    format!(
        "{run} belongs to the project at {} and this pane works in {} — `{verb}` is not \
         answered across projects. Ask from a pane of that project, or work in a run of \
         this one (`run-list`, `run-create`).",
        project.label(),
        label(caller_root),
    )
}
