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

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use zerocode_core::orchestration::project_scope::{label, refusal};
use zerocode_core::orchestration::{
    Ledger, PANE_ADDRESS_PREFIX, ProjectSource, RUN_ADDRESS_PREFIX, Run, RunProject, Scope,
};

use crate::agent_teams::Host;

use super::{BLACKBOX, LiveRuntime, rang, runtime};

/// How many folders' projects the window keeps read. A window holds a few
/// dozen panes; past this the whole map is forgotten and read again.
const ROOTS_KEPT: usize = 512;

/// The project a folder belongs to ([`zerocode_core::git_dir::project_root`]),
/// read once per folder. The beat asks for every seated pane each second, and
/// a folder belongs to the project it belonged to a second ago. A folder that
/// is not there is not remembered: it may be a checkout still being cut.
pub(super) fn root_of(folder: &Path) -> Option<String> {
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
///
/// `for_run` is the window's own verb, never a pane's (t-94842): the release
/// sweep closes a finished worker of a run in that run's recorded project, read
/// off the ledger, rather than the folder its coordinator's pane happens to
/// stand in. It confirms nothing — a confirmation is the coordinator's own verb
/// from its own pane — and it reaches no run whose project is another.
pub(super) fn verb_gate(
    host: &dyn Host,
    team: &str,
    pane: &str,
    argv: &[String],
    actor: Option<&str>,
    for_run: Option<&str>,
    now_ms: i64,
) -> Result<Gate, String> {
    let term = crate::agent_teams::teams()
        .get(team)
        .and_then(|held| held.term_of(pane));
    let caller = match for_run {
        Some(root) => Some(root.to_string()),
        None => term.and_then(|term| pane_root(host, term)),
    };
    if let (None, Some(_), Some(conversation)) = (for_run, term, actor) {
        remember_seat_conversation(&format!("{team}/{pane}"), conversation);
    }
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
        if project.inferred() && project.root == caller && reach.coordinator && for_run.is_none() {
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
    /* A recipient whose folder the window has no record of — a pane a
     * restart brought back, its launch environment gone — is judged by the
     * project of the run the ledger seats it in (t-97078). The caller is
     * not: a verb is answered from a pane the window knows. */
    let filed_in = reach.run.as_ref().map(|(run, _)| run.as_str());
    for recipient in &reach.recipient_seats {
        recipient_gate(
            host,
            verb,
            caller.as_deref(),
            recipient,
            reach.seated_root(recipient),
            filed_in,
        )?;
    }
    if let Some((run_id, project)) = &reach.carried_run {
        match (caller.as_deref(), project.as_ref()) {
            (Some(caller), Some(project)) if caller != project.root => {
                verb_crossing(verb, run_id, project, caller, &seat, term)?
            }
            (None, Some(_)) => return Err(unidentified(verb, run_id)),
            // A run with no project written down is reached through its
            // recipients, each judged above against this pane's project: a
            // letter to nobody placed is not answered across the unknown.
            (Some(_), None) if reach.recipient_seats.is_empty() => {
                return Err(unidentified(verb, run_id));
            }
            _ => {}
        }
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
///
/// `seated` is the project of the run the recipient sits in, the judge for a
/// pane whose folder the window has no record of (t-97078); `filed_in` is the
/// run the letter would be filed in.
fn recipient_gate(
    host: &dyn Host,
    verb: &str,
    caller: Option<&str>,
    recipient: &str,
    seated: Option<&str>,
    filed_in: Option<&str>,
) -> Result<(), String> {
    let term = recipient.rsplit_once('/').and_then(|(team, pane)| {
        crate::agent_teams::teams()
            .get(team)
            .and_then(|held| held.term_of(pane))
    });
    let target = term
        .and_then(|term| pane_root(host, term))
        .or_else(|| seated.map(str::to_string));
    match (caller, target.as_deref()) {
        (Some(caller), Some(target)) if caller == target => Ok(()),
        // Legacy, unplaced participants are already restricted to their own
        // unscoped run above. They never enter a known project's run.
        (None, None) => Ok(()),
        // No pane holds the address and no run seats it: not a project the
        // window failed to identify, but a pane that is not here.
        (Some(_), None) if term.is_none() => Err(gone_recipient(host, verb, recipient, filed_in)),
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

/// The refusal for a letter to a pane address no pane in this window holds,
/// and no run seats.
///
/// A pane's address is minted with its pane: a pane the person closed has
/// none, and a conversation a restart brings back answers at a new one. Its
/// correspondents still hold the old one — the letter they answer is signed
/// with it — and the refusal used to say the recipient "works in an
/// unidentified project", which sent a coordinator looking for a project
/// problem (2026-11-08 21:41–22:50, four letters to two panes the restart had
/// re-minted or the person had closed). So it says what is so: that address
/// is no pane here now, and — when the window knows which conversation wrote
/// from it ([`remember_seat_conversation`]) and that conversation stands in a
/// pane again — where it can be written to now ([`written_to_at`]).
fn gone_recipient(host: &dyn Host, verb: &str, recipient: &str, filed_in: Option<&str>) -> String {
    // The pane table is let go before the ledger is asked: the actor takes
    // that table itself.
    let goes_on = conversation_of_seat(recipient).and_then(|conversation| {
        crate::agent_teams::teams().iter().find_map(|(id, team)| {
            team.pane(&team.leader_pane)?;
            (host.actor_for(team.leader_term).as_deref() == Some(conversation.as_str()))
                .then(|| (format!("{id}/{}", team.leader_pane), team.leader_term))
        })
    });
    let reason = match goes_on {
        Some((seat, term)) => {
            let at = format!(
                "the recipient {recipient} is no pane in this window now — the conversation \
                 that wrote from it goes on in terminal {term}, at pane:{seat}. `{verb}` was not \
                 sent"
            );
            match written_to_at(&seat, filed_in) {
                Some(road) => format!("{at}; {road}."),
                None => format!(
                    "{at}, and that pane has written nothing{} since — a pane is written to once \
                     it has written there. Answer once it writes, or ask the person to point it \
                     at this letter.",
                    filed_in.map(|run| format!(" in {run}")).unwrap_or_default()
                ),
            }
        }
        None => format!(
            "the recipient {recipient} is no pane in this window now — its pane was closed, or \
             the window restarted since it wrote and a conversation that comes back answers at \
             a new address. `{verb}` was not sent; write to the run it works in (run:<id>), or \
             answer once it writes again."
        ),
    };
    note(&reason);
    reason
}

/// Where the pane at `seat` can be written to now, as the words that say how:
/// the run's chair or the worker it sits as ([`Ledger::seat_reader`]), else
/// its own pane address once it answers in `filed_in` — it has written there,
/// or the ledger told it there that it goes on for a pane that had
/// ([`zerocode_core::orchestration::Run::answers_at`], the rule the ledger
/// routes by). `None` when neither holds yet.
fn written_to_at(seat: &str, filed_in: Option<&str>) -> Option<String> {
    let held = runtime()?;
    let image = held.actor.view().ok()?;
    let ledger = super::cached_ledger(&held, &image);
    if let Some((run, address)) = ledger.seat_reader(seat) {
        return Some(if address.starts_with(RUN_ADDRESS_PREFIX) {
            format!("send it to {address}")
        } else {
            format!("send it with --run {run} --to {address}")
        });
    }
    let pane = format!("{PANE_ADDRESS_PREFIX}{seat}");
    ledger
        .run(filed_in?)?
        .answers_at(&pane)
        .then(|| format!("send it to {pane}"))
}

/// How many panes' conversations the window keeps on disk. A window holds a
/// few dozen panes over a day; this holds weeks of them and bounds the file.
const SEAT_CONVERSATIONS_KEPT: usize = 256;

/// `(seat, conversation)` pairs, oldest first: the conversation each pane
/// address last sent a verb from.
type SeatConversations = VecDeque<(String, String)>;

fn seat_conversations_path() -> Option<PathBuf> {
    BLACKBOX
        .get()
        .map(|root| root.join("orchestration-seat-conversations.json"))
}

fn seat_conversations() -> &'static Mutex<Option<SeatConversations>> {
    static KNOWN: OnceLock<Mutex<Option<SeatConversations>>> = OnceLock::new();
    KNOWN.get_or_init(|| Mutex::new(None))
}

fn load_seat_conversations() -> SeatConversations {
    seat_conversations_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|raw| serde_json::from_str::<Vec<(String, String)>>(&raw).ok())
        .map(SeatConversations::from)
        .unwrap_or_default()
}

/// Write down that the pane at `seat` (`team/pane`) speaks for `conversation`
/// — its agent and provider session, as the verb road names its caller —
/// so a letter to that address can still find the conversation after the
/// address is gone ([`gone_recipient`]). Kept on disk, because the address
/// goes with the window that minted it. Written only when the pair is new: a
/// pane sends many verbs from one conversation.
fn remember_seat_conversation(seat: &str, conversation: &str) {
    let mut store = seat_conversations()
        .lock()
        .unwrap_or_else(|held| held.into_inner());
    let known = store.get_or_insert_with(load_seat_conversations);
    if known
        .iter()
        .any(|(held, spoke)| held == seat && spoke == conversation)
    {
        return;
    }
    known.retain(|(held, _)| held != seat);
    known.push_back((seat.to_string(), conversation.to_string()));
    save_seat_conversations(known);
}

/// Bound the book and write it whole.
fn save_seat_conversations(known: &mut SeatConversations) {
    while known.len() > SEAT_CONVERSATIONS_KEPT {
        known.pop_front();
    }
    let Some(path) = seat_conversations_path() else {
        return;
    };
    let rows: Vec<&(String, String)> = known.iter().collect();
    if let Ok(payload) = serde_json::to_vec(&rows) {
        let _ = zerocode_core::atomic_file::write_atomically(&path, &payload);
    }
}

/// A pane whose conversation the seat book knows from another pane no team
/// holds any more: a restart minted the pane again ([`continuing`]).
pub(super) struct Continuing {
    old: String,
    new: String,
    term: u32,
    conversation: String,
}

/// The panes in `tables` whose conversation the seat book knows from a seat
/// whose team is not in the table — the same conversation under the address a
/// restart gave it. Read under the pane table's lock, once a beat; it writes
/// nothing. A conversation shown in two live panes, or only in a pane the
/// person closed, is left alone: which pane goes on for it was not witnessed.
pub(super) fn continuing(
    host: &dyn Host,
    tables: &HashMap<String, zerocode_core::agent_teams::Team>,
) -> Vec<Continuing> {
    let mut shown: HashMap<String, Vec<(String, u32)>> = HashMap::new();
    for (id, team) in tables {
        if team.pane(&team.leader_pane).is_none() || host.pane_detached(team.leader_term) {
            continue;
        }
        if let Some(conversation) = host.actor_for(team.leader_term) {
            shown
                .entry(conversation)
                .or_default()
                .push((format!("{id}/{}", team.leader_pane), team.leader_term));
        }
    }
    if shown.is_empty() {
        return Vec::new();
    }
    let told = continued().lock().unwrap_or_else(|held| held.into_inner());
    let mut store = seat_conversations()
        .lock()
        .unwrap_or_else(|held| held.into_inner());
    store
        .get_or_insert_with(load_seat_conversations)
        .iter()
        .filter_map(|(old, conversation)| {
            let [(new, term)] = shown.get(conversation)?.as_slice() else {
                return None;
            };
            let gone = old
                .split_once('/')
                .is_some_and(|(team, _)| !tables.contains_key(team));
            (gone && old != new && !told.contains(&(old.clone(), new.clone()))).then(|| {
                Continuing {
                    old: old.clone(),
                    new: new.clone(),
                    term: *term,
                    conversation: conversation.clone(),
                }
            })
        })
        .collect()
}

/// The `(old, new)` pairs this process has already told the ledger about
/// ([`continue_seat`]), so the beat does not ask again every second. Process
/// memory: after the next restart both seats are gone, and both are told the
/// pane the conversation goes on in then.
fn continued() -> &'static Mutex<std::collections::HashSet<(String, String)>> {
    static TOLD: OnceLock<Mutex<std::collections::HashSet<(String, String)>>> = OnceLock::new();
    TOLD.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}

/// The ledger is told that `one`'s conversation goes on in its new pane
/// ([`zerocode_core::orchestration::Ledger::seat_continued`]), and the seat
/// book learns the new pane beside the old one — the old row still names the
/// conversation a letter to the old address belongs to ([`gone_recipient`]),
/// and the new one lets a second restart find it again, also for a pane that
/// only receives mail and never sends a verb of its own. Asked after the beat
/// let go of the pane table: the actor takes that table itself.
pub(super) fn continue_seat(held: &LiveRuntime, one: &Continuing, now_ms: i64) {
    let Ok((filed, _)) = held
        .actor
        .seat_continued(one.old.clone(), one.new.clone(), now_ms)
    else {
        return;
    };
    rang(filed);
    continued()
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .insert((one.old.clone(), one.new.clone()));
    {
        let mut store = seat_conversations()
            .lock()
            .unwrap_or_else(|held| held.into_inner());
        let known = store.get_or_insert_with(load_seat_conversations);
        if !known
            .iter()
            .any(|(seat, conversation)| *seat == one.new && *conversation == one.conversation)
        {
            known.retain(|(seat, _)| *seat != one.new);
            known.push_back((one.new.clone(), one.conversation.clone()));
            save_seat_conversations(known);
        }
    }
    if filed && let Some(root) = BLACKBOX.get() {
        crate::note_window_event(
            root,
            &format!(
                "orchestration: the conversation that wrote from {PANE_ADDRESS_PREFIX}{} goes on at \
                 {PANE_ADDRESS_PREFIX}{} (terminal {}) after the window restarted; it was told so \
                 where the old address spoke, and is answerable there",
                one.old, one.new, one.term
            ),
        );
    }
}

/// The conversation the pane at `seat` last sent a verb from, in this life of
/// the window or an earlier one.
fn conversation_of_seat(seat: &str) -> Option<String> {
    let mut store = seat_conversations()
        .lock()
        .unwrap_or_else(|held| held.into_inner());
    store
        .get_or_insert_with(load_seat_conversations)
        .iter()
        .rev()
        .find(|(held, _)| held == seat)
        .map(|(_, conversation)| conversation.clone())
}

/// Forget what this process read of the seat book, so the next read comes
/// from disk — what a window restart does, minus the window.
#[cfg(test)]
pub(super) fn forget_seat_conversations_for_tests() {
    *seat_conversations()
        .lock()
        .unwrap_or_else(|held| held.into_inner()) = None;
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
/// team table is taken one lookup at a time. Every reading is made before the
/// first write and the ledger handed back between them, so a write meets no
/// image of this window's to copy the ledger for (t-81273).
pub(super) fn infer_projects(host: &dyn Host, held: &LiveRuntime, rows: Arc<Ledger>, now_ms: i64) {
    static TRIED: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    let tried = TRIED.get_or_init(|| Mutex::new(HashMap::new()));
    let mut asked = 0;
    let mut readings = Vec::new();
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
            break;
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
        readings.push((run.id.clone(), root, source, seat));
    }
    drop(rows);
    for (run, root, source, seat) in readings {
        let Ok((moved, _)) = held
            .actor
            .note_run_project(&run, &root, source, seat, now_ms)
        else {
            continue;
        };
        rang(moved);
        if moved {
            note(&format!(
                "{run}'s project was read off {} as {}; the project boundary holds while \
                 its coordinator's confirmation is pending",
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
