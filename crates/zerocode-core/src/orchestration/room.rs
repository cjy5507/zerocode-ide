//! A project's room (2026-11-12): where the agent panes a person opened write
//! to each other, whatever CLI each one runs.
//!
//! Mail needs a run. A pane the person opened sits in none, so its `send` was
//! refused (「no run in use」), and naming another run with `--run` only let it
//! write to panes that had already spoken there. Claude panes could still talk
//! through Claude Code's own messaging; a Codex, Antigravity or zo pane could
//! not. The roads that tell a pane about its mail already exist for every
//! kind of pane — what was missing was a run they could all write in.
//!
//! A room is that run: one per project, opened by the first letter a pane no
//! run holds sends, and marked [`RoomMark`] so every rule below can ask.
//!
//! 1. **Mail and nothing else.** A room answers [`ROOM_VERBS`] only. A task,
//!    a worker, a gate or a handover needs a run somebody coordinates, so a
//!    verb that names a room for any of them is refused (`refused_here`).
//!    A room has no chair, and no chair must not read as "anybody may open
//!    work".
//! 2. **Nobody speaks as the room.** In a run with no coordinator record the
//!    ledger signs a leader-shaped pane as `run:` — the operator voice a
//!    worker obeys. A room has no coordinator record by design, so in a room
//!    every pane signs and reads as its own `pane:` address
//!    (`sender`), and a letter to the room's `run:` address is
//!    refused (`send_allowed`).
//! 3. **A worker's letter goes through its coordinator.** A worker works from
//!    one briefing, and a letter from a pane beside it is a request its
//!    coordinator never saw. So a room letter to a pane working as a worker
//!    is filed in that run's `run:` inbox, saying whom it was for
//!    (`worker_letter`), and refused when that run has no live coordinator
//!    to read it.
//! 4. **The person's switch.** The window says whether 「에이전트 판끼리 편지」
//!    is on ([`super::Launcher::peer_mail_on`]); off, a room letter is
//!    refused with the reason, and what is already in a room can still be
//!    read.
//! 5. **Waking is the window's to limit.** How often a room letter may wake a
//!    pane is counted where the pointer is handed over, per pane — the
//!    window's side, not this file's.

use super::{
    Draft, Launcher, Ledger, MessageKind, PANE_ADDRESS_PREFIX, RUN_ADDRESS_PREFIX,
    WORKER_ADDRESS_PREFIX, Words, standing_in,
};

/// The verb that lists the project's agent panes.
pub const PEERS_VERB: &str = "peers";

/// What a room answers. Everything else needs a run somebody coordinates.
pub const ROOM_VERBS: &[&str] = &[PEERS_VERB, "send", "ask", "reply", "check"];

/// The name a room run is made under, so `run-list` says what it is.
pub const ROOM_NAME: &str = "room";

/// The refusal while the person's switch is off — said in the ledger's words;
/// the window names its own setting.
pub const MAIL_OFF: &str = "agent-to-agent mail is off in this window's settings — nothing was \
     sent; the person turns it on, and a coordinator's run still carries letters";

/// The refusal for a pane whose project the window does not know: a room is
/// one per project, and a letter with no project to file it under would be
/// read by nobody.
pub const PROJECT_UNKNOWN: &str = "this pane's project is not known — the window has no folder \
     for it, and a room is one per project; nothing was sent";

/// The run is its project's room — when it was opened.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomMark {
    pub since_ms: i64,
}

/// One agent pane the window has open, as the window knows it — the half of
/// a `peers` row the ledger cannot see for itself.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerPane {
    /// `team/pane`, as the pane's own `pane:` address carries it.
    pub seat: String,
    /// The catalog id of the agent running there, when the window knows it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// The pane's title as the person sees it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The folder the pane works in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// `working`, `idle` or `waiting` (on the person).
    pub state: &'static str,
}

impl super::Run {
    /// Whether this run is its project's room.
    #[must_use]
    pub const fn is_room(&self) -> bool {
        self.room.is_some()
    }
}

impl Ledger {
    /// The room of the project rooted at `root`, if one was opened.
    #[must_use]
    pub fn room_of(&self, root: &str) -> Option<&str> {
        self.runs()
            .iter()
            .find(|run| {
                run.is_room()
                    && run
                        .project
                        .as_ref()
                        .is_some_and(|project| project.root == root)
            })
            .map(|run| run.id.as_str())
    }

    /// Open `root`'s room, said to be opened by `seat` — never for a root
    /// that could not be a project's, so no room stands without one.
    fn open_room(&mut self, root: &str, seat: &str, now_ms: i64) -> Option<String> {
        if root.is_empty() || root.len() > super::project_scope::PROJECT_ROOT_MAX {
            return None;
        }
        let id = self.create_run(ROOM_NAME, now_ms);
        let placed = self.state_run_project(&id, root, seat, now_ms);
        let run = self.run_mut(&id)?;
        run.room = Some(RoomMark { since_ms: now_ms });
        placed.then_some(id)
    }

    fn is_room(&self, run_id: &str) -> bool {
        self.run(run_id).is_some_and(super::Run::is_room)
    }
}

/// Whether `run_id` is a room — for the signing rule in [`super::sender`].
pub(super) fn signs_as_pane(ledger: &Ledger, run_id: &str) -> bool {
    ledger.is_room(run_id)
}

/// The run a mail verb from a pane no run holds acts in, for the window's
/// project door ([`super::project_scope::reach`]): its project's room, when
/// one is open. The plan names the same room ([`route`]), so the door judges
/// the run the letter is filed in.
pub(super) fn acting_room<'a>(ledger: &'a Ledger, argv: &[String], root: &str) -> Option<&'a str> {
    let verb = argv.first()?.as_str();
    (verb != PEERS_VERB && ROOM_VERBS.contains(&verb) && !root.is_empty())
        .then(|| ledger.room_of(root))
        .flatten()
}

/// The pane a room letter names when no room is open yet — the first letter
/// opens it — so the window's door still holds that pane to the caller's
/// project before anything is filed.
pub(super) fn first_letter_seat(argv: &[String]) -> Option<String> {
    let (verb, rest) = argv.split_first()?;
    if !writes(verb) {
        return None;
    }
    let words = super::split_words(rest, super::BOOL_FLAGS);
    words
        .value("--to")?
        .strip_prefix(PANE_ADDRESS_PREFIX)
        .map(str::to_string)
}

/// The verbs that write a room letter, and so may open the room.
fn writes(verb: &str) -> bool {
    matches!(verb, "send" | "ask")
}

/// Where a pane no run holds writes: its project's room, named onto `words`
/// as `--run` before anything reads them, so a retry is fingerprinted with
/// the room it went to. Answers the room it named.
///
/// Only for [`ROOM_VERBS`], only when the caller named no run and none holds
/// it. `send` and `ask` open the room when there is none yet — refused while
/// the person's switch is off, and for a pane whose project the window does
/// not know; a look at a room nobody opened is the old 「no run in use」,
/// because there is nothing to read. A room this opened for a letter that is
/// then refused is taken back ([`take_back`]).
///
/// And first, the refusal: a verb outside [`ROOM_VERBS`] that names a room,
/// or that a room would answer for its caller.
#[allow(clippy::too_many_arguments)]
pub(super) fn route(
    ledger: &mut Ledger,
    launcher: &dyn Launcher,
    verb: &str,
    words: &mut Words,
    caller: &str,
    seat: &str,
    project: Option<&str>,
    now_ms: i64,
) -> Result<Option<String>, String> {
    let standing = standing_in(ledger, caller, seat);
    refused_here(ledger, verb, words, standing.as_deref())?;
    if verb == PEERS_VERB
        || !ROOM_VERBS.contains(&verb)
        || words.value("--run").is_some()
        || standing.is_some()
    {
        return Ok(None);
    }
    if writes(verb) && !launcher.peer_mail_on() {
        return Err(MAIL_OFF.to_string());
    }
    let root = project.filter(|root| !root.is_empty());
    let room = match root.and_then(|root| ledger.room_of(root)) {
        Some(room) => room.to_string(),
        None if !writes(verb) => return Ok(None),
        None => root
            .and_then(|root| ledger.open_room(root, seat, now_ms))
            .ok_or_else(|| PROJECT_UNKNOWN.to_string())?,
    };
    words.values.push(("--run".to_string(), room.clone()));
    Ok(Some(room))
}

/// A refused verb changes nothing: a room the refused call opened — the runs
/// past `runs_before`, a room that holds no letter — is taken back. Its id is
/// not given again (`mint` only counts up), and nobody was told it.
pub(super) fn take_back(ledger: &mut Ledger, runs_before: usize) {
    while ledger.runs.len() > runs_before
        && ledger
            .runs
            .last()
            .is_some_and(|run| run.is_room() && run.messages().is_empty())
    {
        ledger.runs.pop();
    }
}

/// A verb outside [`ROOM_VERBS`] that names a room — by `--run`, or as the
/// run id it takes first (`run-use`, `run-show`) — or whose caller a room
/// holds, is refused. Nothing binds a pane to a room today; this keeps the
/// rule in one place if something ever does.
fn refused_here(
    ledger: &Ledger,
    verb: &str,
    words: &Words,
    standing: Option<&str>,
) -> Result<(), String> {
    if ROOM_VERBS.contains(&verb) {
        return Ok(());
    }
    let named = words
        .value("--run")
        .into_iter()
        .chain(words.positional.first().map(String::as_str))
        .chain(standing.filter(|_| words.value("--run").is_none()));
    for id in named {
        if ledger.is_room(id) {
            return Err(format!(
                "{id} is this project's room, where agent panes write to each other — it \
                 answers {} only; {verb} needs a run somebody coordinates (run-create first)",
                ROOM_VERBS.join(" · ")
            ));
        }
    }
    Ok(())
}

/// What a room letter may be: a status or a question, to one pane by its
/// `pane:` address. A completion, a dispatch, a gate or a handover is work a
/// coordinator's run carries; a group is a crowd nobody listed; and the
/// room's own `run:` address is the operator voice nobody holds here.
///
/// And the person's switch, wherever the room was named from: a pane that
/// writes `--run <room>` is held by it as much as one the room was found for.
pub(super) fn send_allowed(
    ledger: &Ledger,
    launcher: &dyn Launcher,
    run_id: &str,
    kind: MessageKind,
    to: Option<&str>,
) -> Result<(), String> {
    if !ledger.is_room(run_id) {
        return Ok(());
    }
    if !launcher.peer_mail_on() {
        return Err(MAIL_OFF.to_string());
    }
    if !matches!(kind, MessageKind::Status | MessageKind::Question) {
        return Err(format!(
            "a room letter is a status or a question — {} is work a coordinator's run \
             carries (run-create first)",
            kind.as_str()
        ));
    }
    one_pane(to)
}

/// What a room question may be: to one pane no run seats. A pane coordinating
/// or working in a run reads its mail there, so a question filed in the room
/// under its pane address would wait for an answer from nobody — `send` it,
/// and the letter reaches it where it reads.
pub(super) fn ask_allowed(
    ledger: &Ledger,
    launcher: &dyn Launcher,
    run_id: &str,
    to: Option<&str>,
) -> Result<(), String> {
    if !ledger.is_room(run_id) {
        return Ok(());
    }
    if !launcher.peer_mail_on() {
        return Err(MAIL_OFF.to_string());
    }
    one_pane(to)?;
    let seat = to.and_then(|to| to.strip_prefix(PANE_ADDRESS_PREFIX));
    if let Some((there, _)) = seat.and_then(|seat| ledger.seat_reader(seat)) {
        return Err(format!(
            "{} reads its mail in {there}, not in this room, so a question here would wait \
             on nobody — send it a letter (send --type question) and it reads it there",
            to.unwrap_or_default()
        ));
    }
    Ok(())
}

/// A room reply, held by the person's switch like any room letter: off, two
/// panes do not go on answering each other in a room.
pub(super) fn reply_allowed(
    ledger: &Ledger,
    launcher: &dyn Launcher,
    run_id: &str,
) -> Result<(), String> {
    match ledger.is_room(run_id) && !launcher.peer_mail_on() {
        true => Err(MAIL_OFF.to_string()),
        false => Ok(()),
    }
}

/// Whether `message`, filed in `run`, is a room's: written in a room, or the
/// answer to a room letter's writer in the thread that letter began — a
/// coordinator's reply to the copy of a room letter is a room reply though it
/// was written in its own run. Only an answer TO the room letter's writer: a
/// worker's word filed under that thread with `--thread-id` and sent to its
/// coordinator is not a room's (review t-105587).
#[must_use]
pub fn of_a_room(run: &super::Run, message: &super::Message) -> bool {
    message.room.is_some()
        || message
            .thread
            .as_deref()
            .and_then(|root| run.message(root))
            .is_some_and(|root| root.room.is_some() && message.to == root.from)
}

fn one_pane(to: Option<&str>) -> Result<(), String> {
    match to {
        Some(to) if to.starts_with(PANE_ADDRESS_PREFIX) => Ok(()),
        Some(to) => Err(format!(
            "in a room a letter goes to one pane by its pane: address (`peers` lists \
             them) — {to} is not one"
        )),
        None => Err(
            "in a room a letter names its pane with --to pane:<team>/<pane> (`peers` lists \
             them) — the room has no coordinator to take it"
                .to_string(),
        ),
    }
}

/// A room letter to a pane that works as a worker in another run: filed in
/// that run's `run:` inbox instead, its subject saying whom it was for, so
/// the coordinator decides whether the worker hears it. `None` for any other
/// letter; refused when that run has no live coordinator to read it.
pub(super) fn worker_letter(
    ledger: &Ledger,
    run_id: &str,
    draft: &Draft,
) -> Option<Result<(String, Draft), String>> {
    if !ledger.is_room(run_id) {
        return None;
    }
    let seat = draft.to.strip_prefix(PANE_ADDRESS_PREFIX)?;
    let (there, address) = ledger.seat_reader(seat)?;
    if !address.starts_with(WORKER_ADDRESS_PREFIX) {
        return None;
    }
    let coordinated = ledger
        .run(&there)
        .is_some_and(|run| run.coordinator_live().is_some());
    if !coordinated {
        return Some(Err(format!(
            "{} works as {address} in {there}, and {there} has no coordinator sitting to \
             read a letter for it — nothing was sent; a worker hears from its coordinator",
            draft.to
        )));
    }
    let subject = match draft.subject.as_str() {
        "" => format!("for {address}"),
        said => format!("for {address}: {said}"),
    };
    Some(Ok((
        there.clone(),
        Draft {
            to: format!("{RUN_ADDRESS_PREFIX}{there}"),
            subject: subject.into(),
            task: None,
            dispatch: None,
            ..draft.clone()
        },
    )))
}

/// `peers`: the agent panes of the caller's project as the window has them,
/// with where the ledger knows each one sits, the room, and whether the
/// person's switch is on.
pub(super) fn peers(
    ledger: &Ledger,
    launcher: &dyn Launcher,
    project: Option<&str>,
    seat: &str,
) -> Result<serde_json::Value, String> {
    let panes = launcher.live_panes(project).ok_or(
        "this window did not say which panes it has open — peers is answered by the window",
    )?;
    let rows: Vec<serde_json::Value> = panes
        .iter()
        .map(|pane| {
            let mut row = serde_json::to_value(pane).unwrap_or_default();
            row["address"] = format!("{PANE_ADDRESS_PREFIX}{}", pane.seat).into();
            if pane.seat == seat {
                row["you"] = true.into();
            }
            if let Some((run, reads)) = ledger.seat_reader(&pane.seat) {
                let role = if reads.starts_with(WORKER_ADDRESS_PREFIX) {
                    "worker"
                } else {
                    "coordinator"
                };
                row["sits"] = serde_json::json!({ "runId": run, "as": role });
            }
            row
        })
        .collect();
    Ok(serde_json::json!({
        "room": project.and_then(|root| ledger.room_of(root)),
        "mailOn": launcher.peer_mail_on(),
        "peers": rows,
        "note": "a letter to a pane: address here goes through the room; one to a worker \
                 goes to its coordinator. Facts and findings go to the pane; a decision \
                 goes to a coordinator or the person.",
    }))
}
