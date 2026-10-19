//! The window's half of the ledger's project boundary (t-65286).
//!
//! The ledger writes down which project a run belongs to
//! ([`zerocode_core::orchestration::project_scope`]); the window is the one
//! that knows which project a PANE works in — the checkout it put the pane in
//! ([`Host::worktree_of`]), read for the git directory its repository's
//! worktrees share. So the window holds the boundary where a run reaches a
//! pane: the mail pointer it types, the empty chair it sits a returning
//! conversation in, the verb a pane sends, and the checkout a summoned worker
//! opens in.
//!
//! By the run's project:
//!
//! - **Made here, or confirmed** — no pointer is typed into a pane of another
//!   project, no chair is sat from one, and no worker's checkout is opened in
//!   one. A verb from one is written down as a verb the window would refuse,
//!   and refused once the person turns the boundary on ([`SWITCH_FILE`]): the
//!   first release only writes it down, so a wrong project costs a log line
//!   and never the letter that would have put it right.
//! - **Inferred** — every crossing is written down and nothing is held, until
//!   the run's live coordinator confirms the project with a verb typed in its
//!   own pane, in that project.
//! - **None yet** — as before, with one line saying none could be read.
//!
//! The window's own reads — the sidebar, the board, the mail triage seat —
//! are not a pane's verb and are not held here.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use zerocode_core::orchestration::project_scope::{label, refusal};
use zerocode_core::orchestration::{Ledger, ProjectSource, Run, RunProject, Scope};

use crate::agent_teams::Host;

use super::{BLACKBOX, LiveRuntime, rang, runtime};

/// The file a person turns the verb boundary on with, under the window's data
/// root: a JSON string, [`ENFORCE`]. Absent — the first release's default — or
/// holding anything else, a verb into another project's run is carried out
/// and written down as one the window would refuse, so the boundary is turned
/// on only after its lines show it refuses nothing a project should be given.
pub(crate) const SWITCH_FILE: &str = "orchestration-project-scope.json";

/// The word in [`SWITCH_FILE`] that turns the verb boundary on.
pub(crate) const ENFORCE: &str = "enforce";

/// Whether a verb into a run of another project is refused rather than only
/// written down. Read at each verb, so the person's switch reaches the next
/// verb without a restart; a verb is not the beat.
fn verbs_refused() -> bool {
    let Some(root) = BLACKBOX.get() else {
        return false;
    };
    std::fs::read_to_string(root.join(SWITCH_FILE))
        .ok()
        .and_then(|raw| serde_json::from_str::<String>(&raw).ok())
        .is_some_and(|word| word == ENFORCE)
}

/// How many folders' projects the window keeps read. A window holds a few
/// dozen panes; past this the whole map is forgotten and read again.
const ROOTS_KEPT: usize = 512;

/// The project a folder belongs to ([`zerocode_core::git_dir::project_root`]),
/// read once per folder. The beat asks for every seated pane each second, and
/// a folder belongs to the project it belonged to a second ago. A folder that
/// is not there is not remembered: it may be a checkout still being cut.
fn root_of(folder: &Path) -> Option<String> {
    static ROOTS: OnceLock<Mutex<HashMap<PathBuf, String>>> = OnceLock::new();
    let roots = ROOTS.get_or_init(|| Mutex::new(HashMap::new()));
    let known = roots
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .get(folder)
        .cloned();
    if known.is_some() {
        return known;
    }
    let root = zerocode_core::git_dir::project_root(folder)?
        .to_string_lossy()
        .into_owned();
    let mut held = roots.lock().unwrap_or_else(|held| held.into_inner());
    if held.len() >= ROOTS_KEPT {
        held.clear();
    }
    held.insert(folder.to_path_buf(), root.clone());
    Some(root)
}

/// The project the pane at `term` works in: the checkout the window put it in,
/// read for its project. `None` for a pane the window recorded no folder for.
pub(super) fn pane_root(host: &dyn Host, term: u32) -> Option<String> {
    host.worktree_of(term).as_deref().and_then(root_of)
}

/// The project a folder belongs to, for a road that knows the folder a
/// conversation comes back in rather than a pane.
pub(super) fn folder_root(folder: &Path) -> Option<String> {
    root_of(folder)
}

/// How many times each crossing was met, by its key — process memory, like
/// the pointer's own marks.
fn met(key: String) -> u64 {
    const KEPT: usize = 4096;
    static MET: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();
    let mut held = MET
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|held| held.into_inner());
    if held.len() >= KEPT && !held.contains_key(&key) {
        held.clear();
    }
    let count = held.entry(key).or_insert(0);
    *count += 1;
    *count
}

/// Whether a crossing earns a line now: the first time, then one time in
/// fifty — a coordinator polling across a boundary writes a few lines a day,
/// not one a minute — with how many times it was met.
fn worth_a_line(key: String) -> Option<u64> {
    let count = met(key);
    (count == 1 || count.is_multiple_of(50)).then_some(count)
}

/// One line in the window's black box.
fn note(line: &str) {
    if let Some(root) = BLACKBOX.get() {
        crate::note_window_event(root, &format!("orchestration: project boundary — {line}"));
    }
}

/// A pane, as a line names it.
fn pane_named(seat: &str, term: Option<u32>) -> String {
    term.map_or_else(
        || seat.to_string(),
        |term| format!("terminal {term} ({seat})"),
    )
}

/// How a run's project was learned, as a line says it.
fn learned(project: &RunProject) -> &'static str {
    match project.source {
        ProjectSource::Created => "made there",
        ProjectSource::Workers => "inferred from its workers' checkouts",
        ProjectSource::Seat => "inferred from its coordinator's pane",
        ProjectSource::Confirmed => "confirmed by its coordinator",
    }
}

/* ---- the verb a pane sends --------------------------------------------- */

/// What the window's door read about a verb before the ledger planned it.
#[derive(Debug, Default)]
pub(super) struct Gate {
    /// The project the caller's pane works in, which a `run-create` writes on
    /// the run it makes.
    pub(super) root: Option<String>,
    /// The runs of another project the verb reaches — for `run-list` and
    /// `inbox`, whose answers are held to the caller's project afterwards.
    elsewhere: Vec<(String, RunProject)>,
}

/// The window's half of a verb from the pane `team/pane`, asked before the
/// ledger plans it: the project the pane works in, or the refusal a verb into
/// a run of another project gets once the boundary is on.
///
/// Every crossing is written down whatever is decided — the run, the pane,
/// both projects and the verb — so the person can read what turning the
/// boundary on would refuse before it refuses anything. A verb the run's live
/// coordinator types from a pane of the project the run was inferred to be in
/// confirms that project. One question to the actor that holds the ledger
/// ([`zerocode_core::orchestration::project_scope::reach`]); nothing is
/// copied for it.
pub(super) fn verb_gate(
    host: &dyn Host,
    team: &str,
    pane: &str,
    argv: &[String],
    actor: Option<&str>,
    now_ms: i64,
) -> Result<Gate, String> {
    let term = crate::agent_teams::teams()
        .get(team)
        .and_then(|held| held.term_of(pane));
    let Some(caller) = term.and_then(|term| pane_root(host, term)) else {
        return Ok(Gate::default());
    };
    let Some(held) = runtime() else {
        return Ok(Gate {
            root: Some(caller),
            ..Gate::default()
        });
    };
    let reach = held
        .actor
        .project_reach(argv, team, pane, actor, &caller)
        .unwrap_or_default();
    let verb = argv.first().map_or("", String::as_str);
    let seat = format!("{team}/{pane}");
    if verb == "reset" {
        reset_gate(&reach.elsewhere, &caller, &seat, term)?;
    }
    if let Some((run_id, Some(project))) = &reach.run {
        if project.inferred() && project.root == caller && reach.coordinator {
            confirm(&held, run_id, project, &caller, &seat, term, now_ms);
        } else if project.root != caller {
            verb_crossing(verb, run_id, project, &caller, &seat, term)?;
        }
    }
    Ok(Gate {
        root: Some(caller),
        elsewhere: reach.elsewhere,
    })
}

/// A verb from a pane of another project into `run`: written down, and
/// refused once the boundary is on and the run's project was not merely
/// inferred.
fn verb_crossing(
    verb: &str,
    run: &str,
    project: &RunProject,
    caller: &str,
    seat: &str,
    term: Option<u32>,
) -> Result<(), String> {
    let refused = !project.inferred() && verbs_refused();
    if let Some(count) = worth_a_line(format!("verb\u{1f}{verb}\u{1f}{run}\u{1f}{seat}")) {
        let what = match (refused, project.inferred()) {
            (true, _) => "refused".to_string(),
            (false, true) => format!(
                "carried out and only written down — the run's project is {}, not stated",
                learned(project)
            ),
            (false, false) => {
                format!("carried out; it would be refused once {SWITCH_FILE} holds \"{ENFORCE}\"")
            }
        };
        note(&format!(
            "`{verb}` from {}, working in {}, into {run} of the project at {} ({}): {what} \
             (met {count} time(s))",
            pane_named(seat, term),
            label(caller),
            project.label(),
            learned(project),
        ));
    }
    match refused {
        true => Err(refusal(verb, run, project, caller)),
        false => Ok(()),
    }
}

/// The run's live coordinator typed a verb from a pane of the project the run
/// was inferred to be in: that is the confirmation, written with when and
/// from which seat, so the ledger can say why the run is that project's.
fn confirm(
    held: &LiveRuntime,
    run: &str,
    project: &RunProject,
    root: &str,
    seat: &str,
    term: Option<u32>,
    now_ms: i64,
) {
    let Ok((moved, _)) = held.actor.note_run_project(
        run,
        root,
        ProjectSource::Confirmed,
        Some(seat.to_string()),
        now_ms,
    ) else {
        return;
    };
    rang(moved);
    if moved {
        note(&format!(
            "{run}'s project at {}, {}, was confirmed by its coordinator's own verb from {}",
            label(root),
            learned(project),
            pane_named(seat, term),
        ));
    }
}

/// `reset` clears every run in the window, so it crosses into every run of
/// another project at once. Written down like any crossing, and refused once
/// the boundary is on and one of those runs' projects was not merely inferred.
fn reset_gate(
    crossed: &[(String, RunProject)],
    caller: &str,
    seat: &str,
    term: Option<u32>,
) -> Result<(), String> {
    if crossed.is_empty() {
        return Ok(());
    }
    let held: Vec<&str> = crossed
        .iter()
        .filter(|(_, project)| !project.inferred())
        .map(|(run, _)| run.as_str())
        .collect();
    let refused = !held.is_empty() && verbs_refused();
    let runs: Vec<&str> = crossed.iter().map(|(run, _)| run.as_str()).collect();
    if let Some(count) = worth_a_line(format!("reset\u{1f}{seat}")) {
        note(&format!(
            "`reset` from {}, working in {}, reaches {} run(s) of other projects ({}): {} \
             (met {count} time(s))",
            pane_named(seat, term),
            label(caller),
            runs.len(),
            runs.join(", "),
            if refused {
                "refused"
            } else {
                "carried out and only written down"
            },
        ));
    }
    if refused {
        return Err(format!(
            "`reset` clears every run in this window, and {} of them belong to other \
             projects ({}) — it is not answered across projects. Reset from a pane of each, \
             or leave theirs to them.",
            held.len(),
            held.join(", "),
        ));
    }
    Ok(())
}

/// `run-list` and `inbox` read every run in the window. Once the boundary is
/// on, a pane is shown its own project's runs and mail and nothing of a run
/// whose project was stated elsewhere; until then the answer is whole and the
/// window writes down how much of it belongs to other projects.
pub(super) fn listing(
    team: &str,
    pane: &str,
    argv: &[String],
    gate: &Gate,
    answered: &mut zerocode_hookd::TeamAnswer,
) {
    let verb = argv.first().map_or("", String::as_str);
    let rows = match verb {
        "run-list" => "runs",
        "inbox" => "messages",
        _ => return,
    };
    let Some(caller) = gate.root.as_deref() else {
        return;
    };
    let stated: std::collections::HashSet<&str> = gate
        .elsewhere
        .iter()
        .filter(|(_, project)| !project.inferred())
        .map(|(run, _)| run.as_str())
        .collect();
    if stated.is_empty() || answered.exit_code != 0 {
        return;
    }
    let Ok(mut said) = serde_json::from_str::<serde_json::Value>(answered.stdout.trim()) else {
        return;
    };
    let elsewhere = |row: &serde_json::Value| {
        row["runId"]
            .as_str()
            .is_some_and(|run| stated.contains(run))
    };
    let Some(held) = said[rows].as_array() else {
        return;
    };
    let hidden = held.iter().filter(|&row| elsewhere(row)).count();
    if hidden == 0 {
        return;
    }
    let refused = verbs_refused();
    let seat = format!("{team}/{pane}");
    if let Some(count) = worth_a_line(format!("listing\u{1f}{verb}\u{1f}{seat}")) {
        note(&format!(
            "`{verb}` from {seat}, working in {}, showed {hidden} row(s) of runs of other \
             projects: {} (met {count} time(s))",
            label(caller),
            if refused {
                "left out"
            } else {
                "shown, and left out once the boundary is on"
            },
        ));
    }
    if !refused {
        return;
    }
    if let Some(held) = said[rows].as_array_mut() {
        held.retain(|row| !elsewhere(row));
        let kept = held.len();
        if rows == "messages" {
            said["count"] = kept.into();
        }
    }
    answered.stdout = format!("{said}\n");
}

/* ---- the mail pointer and the chair --------------------------------------- */

/// What the pointer does about one pane, by where the run stands to it.
pub(super) enum PaneScope<'a> {
    /// The pane is the run's project's, or there is nothing to compare.
    Pass,
    /// The pane is another project's and the run's project was only inferred:
    /// written down once, pointed at as before.
    Noted,
    /// The pane is another project's and the run's project was stated: not
    /// pointed at, not sat from.
    Held(&'a RunProject, String),
}

/// Where the run stands to the pane at `term`, whose project `roots` keeps
/// for the length of one beat.
pub(super) fn pane_scope<'a>(
    host: &dyn Host,
    roots: &mut HashMap<u32, Option<String>>,
    run: &'a Run,
    address: &str,
    term: u32,
) -> PaneScope<'a> {
    let root = roots
        .entry(term)
        .or_insert_with(|| pane_root(host, term))
        .clone();
    let Some(root) = root else {
        return PaneScope::Pass;
    };
    let Scope::Crossed(project) = run.scope(Some(root.as_str())) else {
        return PaneScope::Pass;
    };
    if project.inferred() {
        if met(format!(
            "pointer-noted\u{1f}{}\u{1f}{address}\u{1f}{term}",
            run.id
        )) == 1
        {
            note(&format!(
                "mail for {address} in {} would not be pointed at terminal {term}, which works \
                 in {}: the run's project at {} is {}, so it is pointed as before",
                run.id,
                label(&root),
                project.label(),
                learned(project),
            ));
        }
        return PaneScope::Noted;
    }
    PaneScope::Held(project, root)
}

/// Mail for `address` was not pointed at the pane at `term`: it works in
/// another project than the run's. Said once per pane and address.
pub(super) fn note_pointer_held(
    run: &str,
    address: &str,
    term: u32,
    project: &RunProject,
    root: &str,
) {
    if met(format!(
        "pointer-held\u{1f}{run}\u{1f}{address}\u{1f}{term}"
    )) == 1
    {
        note(&format!(
            "mail for {address} in {run} is not pointed at terminal {term}: that pane works in \
             {}, and {run} belongs to the project at {} ({}). `check` from a pane of that \
             project reads it",
            label(root),
            project.label(),
            learned(project),
        ));
    }
}

/// A run's empty chair was not sat from the pane at `term`: it works in
/// another project than the run's. Said once per pane.
pub(super) fn note_chair_held(run: &str, term: u32, project: &RunProject, root: &str) {
    if met(format!("chair-held\u{1f}{run}\u{1f}{term}")) == 1 {
        note(&format!(
            "{run}'s empty chair is not sat from terminal {term}: that pane works in {}, and \
             {run} belongs to the project at {} ({})",
            label(root),
            project.label(),
            learned(project),
        ));
    }
}

/* ---- the checkout a summoned worker opens in ------------------------------ */

/// Whether the worker `worker` of `run` may open its pane in `checkout`: a
/// worker works in its run's project, as the reservation found it. Refused
/// when that project was not merely inferred and the checkout is another
/// project's; written down and let through when it was only inferred; let
/// through as before when there is nothing to compare.
pub(super) fn worker_checkout(
    project: Option<&RunProject>,
    run: &str,
    worker: &str,
    checkout: Option<&Path>,
) -> Result<(), String> {
    let (Some(project), Some(root)) = (project, checkout.and_then(root_of)) else {
        return Ok(());
    };
    if project.root == root {
        return Ok(());
    }
    let where_ = label(&root);
    if project.inferred() {
        if met(format!("checkout-noted\u{1f}{run}\u{1f}{worker}")) == 1 {
            note(&format!(
                "worker {worker} of {run} opens in {where_}, outside the run's project at {} ({}) \
                 — let through, the project is not stated",
                project.label(),
                learned(project),
            ));
        }
        return Ok(());
    }
    note(&format!(
        "worker {worker} of {run} was not opened in {where_}: the run belongs to the project at \
         {} ({})",
        project.label(),
        learned(project),
    ));
    Err(format!(
        "{run} belongs to the project at {}, and this worker would open in {where_} — a \
         worker's checkout is opened only in its run's project. Summon it from a pane of that \
         project.",
        project.label(),
    ))
}

/* ---- the runs made before projects were recorded --------------------------- */

/// How many runs one beat reads a project off. A window that comes back
/// holding sixty old runs reads them over a few seconds, not in one beat.
const INFERRED_PER_BEAT: usize = 8;

/// Read a project off each run made before projects were recorded and write
/// it down as inferred: the project most of its workers' checkouts belong to,
/// first — every worktree of one repository counts as one — and the pane its
/// coordinator sits in when no worker says. A run nothing can be read off yet
/// is said once, and read again when its seat or its workers change. Newest
/// first: the runs still at work are the ones whose mail is moving.
///
/// Called with no lock held: the ledger is written through the actor, and the
/// team table is taken one lookup at a time.
pub(super) fn infer_projects(host: &dyn Host, held: &LiveRuntime, rows: &Ledger, now_ms: i64) {
    static TRIED: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    let tried = TRIED.get_or_init(|| Mutex::new(HashMap::new()));
    let mut asked = 0;
    for run in rows.runs().iter().rev() {
        if run.project.is_some() {
            continue;
        }
        // What a reading rests on: the live seat, and the checkouts seated.
        let standing = format!(
            "{}\u{1f}{}",
            run.coordinator_live().map_or("", |seat| seat.seat.as_str()),
            run.workers
                .iter()
                .filter(|worker| worker.checkout.is_some())
                .count()
        );
        let seen = tried
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .get(&run.id)
            .is_some_and(|before| *before == standing);
        if seen {
            continue;
        }
        if asked >= INFERRED_PER_BEAT {
            return;
        }
        asked += 1;
        tried
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .insert(run.id.clone(), standing);
        let read = most_workers_root(run)
            .map(|root| (root, ProjectSource::Workers, None))
            .or_else(|| {
                seat_root(host, run).map(|(root, seat)| (root, ProjectSource::Seat, Some(seat)))
            });
        let Some((root, source, seat)) = read else {
            if met(format!("unread\u{1f}{}", run.id)) == 1 {
                note(&format!(
                    "{} holds no project, and none could be read off it yet — none of its \
                     workers' checkouts is a folder here and no pane of this window holds its \
                     seat; its mail and verbs are not bounded until one can be",
                    run.id
                ));
            }
            continue;
        };
        let Ok((moved, _)) = held
            .actor
            .note_run_project(&run.id, &root, source, seat, now_ms)
        else {
            continue;
        };
        rang(moved);
        if moved {
            note(&format!(
                "{}'s project was read off {} as {}; it is written down as inferred, and \
                 nothing is held for it until its coordinator confirms it",
                run.id,
                match source {
                    ProjectSource::Workers => "its workers' checkouts",
                    _ => "its coordinator's pane",
                },
                label(&root),
            ));
        }
    }
}

/// The project most of `run`'s workers' checkouts belong to — `None` when no
/// checkout is a folder here, and when two projects tie, which says nothing.
fn most_workers_root(run: &Run) -> Option<String> {
    let mut counted: HashMap<String, usize> = HashMap::new();
    for checkout in run
        .workers
        .iter()
        .filter_map(|worker| worker.checkout.as_deref())
    {
        if let Some(root) = root_of(Path::new(checkout)) {
            *counted.entry(root).or_default() += 1;
        }
    }
    let most = counted.values().copied().max()?;
    let mut leading = counted.into_iter().filter(|(_, count)| *count == most);
    let (root, _) = leading.next()?;
    leading.next().is_none().then_some(root)
}

/// The project of the pane `run`'s live coordinator sits in, with its seat,
/// when this window holds that pane.
fn seat_root(host: &dyn Host, run: &Run) -> Option<(String, String)> {
    let seat = run.coordinator_live()?.seat.clone();
    let (team, pane) = seat.split_once('/')?;
    let term = crate::agent_teams::teams()
        .get(team)
        .and_then(|held| held.term_of(pane))?;
    pane_root(host, term).map(|root| (root, seat))
}
