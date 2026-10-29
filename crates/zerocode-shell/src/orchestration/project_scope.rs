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
//! - **Made here, confirmed, or inferred** — another project's panes cannot
//!   read, write, receive mail, take the chair, or open a worker's checkout.
//!   This is the default; no settings file enables or disables the boundary.
//! - **None yet** — only the run's existing participants can use it. Its
//!   coordinator can establish the project from its own placed pane. A pane
//!   with no known project cannot enter a run whose project is known.
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
    /// Runs the caller must not see, including unscoped runs it does not own.
    hidden: Vec<String>,
}

/// The window's half of a verb from the pane `team/pane`, asked before the
/// ledger plans it: the pane's project, or a refusal to cross into another.
///
/// Refusals name the run, pane, projects and verb in the window log. A verb the run's live
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
    let caller = term.and_then(|term| pane_root(host, term));
    let Some(held) = runtime() else {
        return Ok(Gate {
            root: caller,
            ..Gate::default()
        });
    };
    let reach = held
        .actor
        .project_reach(
            argv,
            team,
            pane,
            actor,
            caller.as_deref().unwrap_or_default(),
        )
        .map_err(|_| "the ledger's project scope could not be checked; try again".to_string())?;
    let verb = argv.first().map_or("", String::as_str);
    let seat = format!("{team}/{pane}");
    if verb == "reset" {
        reset_gate(&reach.hidden, caller.as_deref(), &seat, term)?;
    }
    if let Some((run_id, Some(project))) = &reach.run {
        let caller = caller
            .as_deref()
            .ok_or_else(|| unidentified(verb, run_id))?;
        if project.inferred() && project.root == caller && reach.coordinator {
            confirm(&held, run_id, project, caller, &seat, term, now_ms);
        } else if project.root != caller {
            verb_crossing(verb, run_id, project, caller, &seat, term)?;
        }
    } else if let Some((run_id, None)) = &reach.run {
        if !reach.participant {
            return Err(unidentified(verb, run_id));
        }
        if let Some(caller) = caller.as_deref() {
            if !reach.coordinator {
                return Err(unidentified(verb, run_id));
            }
            let (moved, _) = held
                .actor
                .note_run_project(
                    run_id,
                    caller,
                    ProjectSource::Seat,
                    Some(seat.clone()),
                    now_ms,
                )
                .map_err(|_| unidentified(verb, run_id))?;
            rang(moved);
        }
    }
    if let Some((run_id, project)) = &reach.reply_run {
        match (caller.as_deref(), project.as_ref()) {
            (Some(caller), Some(project)) if caller != project.root => {
                verb_crossing(verb, run_id, project, caller, &seat, term)?
            }
            (None, Some(_)) | (Some(_), None) => return Err(unidentified(verb, run_id)),
            _ => {}
        }
    }
    for recipient in &reach.recipient_seats {
        recipient_gate(host, verb, caller.as_deref(), recipient)?;
    }
    Ok(Gate {
        root: caller,
        hidden: reach.hidden,
    })
}

fn unidentified(verb: &str, run: &str) -> String {
    format!(
        "the project of this pane or {run} is not identified — `{verb}` is not answered across an unknown project. Ask from the run's own project pane."
    )
}

/// A federation run has no coordinator. Its explicit host team supplies the
/// initial project; an older run must first agree with its recorded workers.
/// The actor only fills an absent project, so a later host cannot move it.
pub(super) fn place_federation_run(
    host: &dyn Host,
    held: &LiveRuntime,
    run: &str,
    team: &str,
    pane: &str,
    now_ms: i64,
) -> Result<(), String> {
    let image = held.actor.view().map_err(|_| unidentified("attach", run))?;
    let current = image
        .ledger()
        .run(run)
        .ok_or_else(|| unidentified("attach", run))?;
    if current.project.is_some() {
        return Ok(());
    }
    let workers = &current.workers;
    let (root, source, seat) = if workers.is_empty() {
        let term = crate::agent_teams::teams()
            .get(team)
            .and_then(|held| held.term_of(pane));
        let root = term
            .and_then(|term| pane_root(host, term))
            .ok_or_else(|| unidentified("attach", run))?;
        (root, ProjectSource::Seat, Some(format!("{team}/{pane}")))
    } else {
        let mut roots = workers.iter().map(|worker| {
            worker
                .checkout
                .as_deref()
                .and_then(|path| root_of(Path::new(path)))
        });
        let root = roots
            .next()
            .flatten()
            .ok_or_else(|| unidentified("attach", run))?;
        if roots.any(|other| other.as_deref() != Some(root.as_str())) {
            return Err(unidentified("attach", run));
        }
        (root, ProjectSource::Workers, None)
    };
    // The ledger goes back before the write: an image still held here would
    // have the actor copy the whole ledger to write past it (t-81273).
    drop(image);
    let (moved, _) = held
        .actor
        .note_run_project(run, root, source, seat, now_ms)
        .map_err(|_| unidentified("attach", run))?;
    rang(moved);
    Ok(())
}

/// The run is not the whole destination: an explicit pane or a reply can
/// name a participant whose pane has since moved to another project.
fn recipient_gate(
    host: &dyn Host,
    verb: &str,
    caller: Option<&str>,
    recipient: &str,
) -> Result<(), String> {
    let term = recipient.rsplit_once('/').and_then(|(team, pane)| {
        crate::agent_teams::teams()
            .get(team)
            .and_then(|held| held.term_of(pane))
    });
    let target = term.and_then(|term| pane_root(host, term));
    match (caller, target.as_deref()) {
        (Some(caller), Some(target)) if caller == target => Ok(()),
        // Legacy, unplaced participants are already restricted to their own
        // unscoped run above. They never enter a known project's run.
        (None, None) => Ok(()),
        _ => {
            let reason = format!(
                "the recipient {recipient} works in {}, and this pane works in {} — `{verb}` was not sent across different or unidentified projects. Use a recipient in this project.",
                target
                    .as_deref()
                    .map(label)
                    .unwrap_or_else(|| "an unidentified project".into()),
                caller
                    .map(label)
                    .unwrap_or_else(|| "an unidentified project".into())
            );
            note(&reason);
            Err(reason)
        }
    }
}

/// A verb into another project's run is refused and recorded.
fn verb_crossing(
    verb: &str,
    run: &str,
    project: &RunProject,
    caller: &str,
    seat: &str,
    term: Option<u32>,
) -> Result<(), String> {
    if let Some(count) = worth_a_line(format!("verb\u{1f}{verb}\u{1f}{run}\u{1f}{seat}")) {
        note(&format!(
            "`{verb}` from {}, working in {}, into {run} of the project at {} ({}): refused \
             (met {count} time(s))",
            pane_named(seat, term),
            label(caller),
            project.label(),
            learned(project),
        ));
    }
    Err(refusal(verb, run, project, caller))
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
/// another project at once. It is refused while any row lies outside the
/// caller's scope, including unidentified runs it does not own.
fn reset_gate(
    hidden: &[String],
    caller: Option<&str>,
    seat: &str,
    term: Option<u32>,
) -> Result<(), String> {
    if hidden.is_empty() {
        return Ok(());
    }
    if let Some(count) = worth_a_line(format!("reset\u{1f}{seat}")) {
        note(&format!(
            "`reset` from {}, working in {}, reaches {} run(s) outside its project: refused \
             (met {count} time(s))",
            pane_named(seat, term),
            caller
                .map(label)
                .unwrap_or_else(|| "an unidentified project".into()),
            hidden.len(),
        ));
    }
    Err("`reset` clears every run in this window, including other or unidentified projects — it is not answered across projects. Use this project's run-specific commands.".into())
}

/// A final guard over list answers. The core already scopes before paging;
/// this also holds a row that was outside the scope read before planning.
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
    let stated: std::collections::HashSet<&str> = gate.hidden.iter().map(String::as_str).collect();
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
    let seat = format!("{team}/{pane}");
    if let Some(count) = worth_a_line(format!("listing\u{1f}{verb}\u{1f}{seat}")) {
        note(&format!(
            "`{verb}` from {seat}, working in {}, showed {hidden} row(s) of runs of other \
            projects: left out (met {count} time(s))",
            gate.root
                .as_deref()
                .map(label)
                .unwrap_or_else(|| "an unidentified project".into()),
        ));
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
    /// Both sides are legacy, unplaced participants. They remain in their
    /// own unscoped run, never a known project's run.
    Noted,
    /// A placed pane must not receive a run whose project is still unknown.
    Unknown,
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
        return match run.project.as_ref() {
            Some(project) => PaneScope::Held(project, "an unidentified project".into()),
            None => PaneScope::Noted,
        };
    };
    match run.scope(Some(root.as_str())) {
        Scope::Same => PaneScope::Pass,
        Scope::Unknown => {
            note_unknown(&run.id, address, term);
            PaneScope::Unknown
        }
        Scope::Crossed(project) => PaneScope::Held(project, root),
    }
}

fn note_unknown(run: &str, address: &str, term: u32) {
    if met(format!(
        "pointer-unknown\u{1f}{run}\u{1f}{address}\u{1f}{term}"
    )) == 1
    {
        note(&format!(
            "mail for {address} in {run} is not pointed at terminal {term}: the run's project is not identified"
        ));
    }
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
/// worker works in its run's project, as the reservation found it. Inferred
/// projects hold the same boundary; only an entirely unplaced legacy run
/// and checkout retain their existing participant-only road.
pub(super) fn worker_checkout(
    project: Option<&RunProject>,
    run: &str,
    worker: &str,
    checkout: Option<&Path>,
) -> Result<(), String> {
    let (project, root) = match (project, checkout.and_then(root_of)) {
        (Some(project), Some(root)) => (project, root),
        (None, None) => return Ok(()),
        _ => {
            return Err(format!(
                "the project of {run} or worker {worker}'s checkout is not identified; the worker was not opened"
            ));
        }
    };
    if project.root == root {
        return Ok(());
    }
    let where_ = label(&root);
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
                     seat; only its existing unplaced participants may use it until its project is identified",
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
                "{}'s project was read off {} as {}; the project boundary holds while \
                 its coordinator's confirmation is pending",
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
