//! A person's own decisions, asked for the agents in this window's panes
//! (t-106308): the decisions that ask before a shell command and at a turn's
//! end, asked from the moments a pane's hooks carry as zo asks them at its
//! own points — the same door, the same hours, the same ledger and the same
//! row ([`custom::answered`]), with `from` naming the pane's agent.
//!
//! Nothing waits and nothing is decided: each ask runs on a thread of its
//! own after the hook was answered, and a decision never blocks a command,
//! changes a tool's input or output, or answers a permission. All one may do
//! is hand its one line ([`custom::line`]), and only while it stands on and
//! its answer earns it. A line waits for the pane's next turn's brief — and,
//! from a decision before a command, for the pane's next tool boundary
//! first ([`take_for_the_tool_boundary`]) — and is taken once, by the launch
//! that earned it.
//!
//! What asks is read off a snapshot ([`Decisions`]) the window refreshes
//! when zo's settings file, the decisions' store or its index moves, so a
//! pane's event reads nothing for a decision while none asks at its point.
//! A refusal the door would give again until something outside the pane
//! changes — an hour or a day found full, no key, a folder not consented, a
//! definition that breaks a rule — makes the decision quiet ([`Quiet`]): it
//! writes one row saying so, and until its span ends, the settings or the
//! store are read again, or its definition changes, a command costs it
//! nothing more. A one-off failure — a wall, a malformed reply — writes its
//! row each time, as the request it counts did leave.

use std::collections::HashMap;
use std::sync::Arc;

use zerocode_core::jev::custom::{self, DecisionRow, Definition, Lent, Point, store};
use zerocode_core::jev::{CUSTOM_DECISIONS, CUSTOM_EVIDENCE_BYTE_CAP, count, door};

use super::*;
use crate::systemone::DecisionHour;

/// One decision as the snapshot keeps it: its row in the index, its
/// definition, and where it stands ([`custom::standing`]).
#[derive(Debug, Clone)]
pub(crate) struct Kept {
    pub entry: store::Entry,
    pub definition: Definition,
    pub mode: JevMode,
}

/// The decisions that ask, as zo's settings and the store were last read:
/// the switch over every decision, and each decision that stands anything
/// but off, with its definition.
#[derive(Debug, Clone, Default)]
pub(crate) struct Decisions {
    pub switch: JevMode,
    pub kept: Vec<Kept>,
}

impl Decisions {
    /// Read the switch from `root`, zo's settings, and — only while it asks
    /// and Jev itself is on — every decision of the store under `home` that
    /// stands anything but off. A store that does not read asks nothing, and
    /// with Jev off a pane's decisions start no thread and write no row; the
    /// settings read again when it is turned back on.
    pub(crate) fn read(root: &Value, home: Option<&Path>) -> Self {
        let switch = CUSTOM_DECISIONS.mode_in(root);
        let jev_on = door::JevSettings::from_root(root).enabled;
        let Some(home) = home.filter(|_| switch.asks() && jev_on) else {
            return Self {
                switch,
                kept: Vec::new(),
            };
        };
        let kept = store::list(home)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|entry| {
                let mode = custom::standing(switch, entry.mode);
                if !mode.asks() {
                    return None;
                }
                let definition = store::definition(home, &entry.definition).ok()?;
                (definition.id == entry.id).then_some(Kept {
                    entry,
                    definition,
                    mode,
                })
            })
            .collect();
        Self { switch, kept }
    }

    /// The decisions that ask at `point`.
    pub(crate) fn at(&self, point: Point) -> impl Iterator<Item = &Kept> {
        self.kept
            .iter()
            .filter(move |kept| kept.definition.at == point)
    }

    /// Whether a decision asks at `point`.
    pub(crate) fn asks_at(&self, point: Point) -> bool {
        self.at(point).next().is_some()
    }

    /// Whether the decision `id` stands on at the version `version`, the one
    /// that answered — zo's rule for a line still waiting to be handed.
    fn stands_on(&self, id: &str, version: &str) -> bool {
        self.switch == JevMode::On
            && self.kept.iter().any(|kept| {
                kept.entry.id == id && kept.entry.definition == version && kept.mode == JevMode::On
            })
    }
}

/// Where a quiet decision's refusal stops repeating: the local hour or day
/// it was refused in ([`door::Refused::repeats`]), or the version whose
/// definition broke a rule ([`custom::check`]).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Until {
    Hour(String),
    Day(String),
    Version(String),
}

/// The local hour and day a moment falls in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Now {
    hour: String,
    day: String,
}

impl Now {
    /// The local hour and day of `at`, in Unix milliseconds.
    pub(crate) fn at(at: u64) -> Self {
        let at = i64::try_from(at).unwrap_or_default();
        Self {
            hour: systemone::hour_at(at),
            day: systemone::day_at(at),
        }
    }

    fn of(&self, span: door::Span) -> Until {
        match span {
            door::Span::Hour => Until::Hour(self.hour.clone()),
            door::Span::Day => Until::Day(self.day.clone()),
        }
    }
}

/// What a quiet mark holds for: a decision everywhere, or a decision in one
/// folder — the folder a pane works in, resolved — for a refusal that folder
/// owns ([`door::Refused::of_the_folder`]).
type Marked = (String, Option<PathBuf>);

/// The decisions whose refusal would repeat, each with where it stops: for
/// the machine, or for the one folder whose refusal it is — a folder not
/// consented quiets the decision there alone. Cleared whenever the snapshot
/// is read again — zo's settings or the decisions' store moved — so a key
/// put in, consent given, a bound raised or a definition mended asks at
/// once.
#[derive(Debug, Default)]
pub(crate) struct Quiet {
    marks: Mutex<HashMap<Marked, Until>>,
}

impl Quiet {
    /// Whether the decision `id`, at the version `version`, is quiet `now`:
    /// everywhere, or — for a pane whose folder is known — in `folder`.
    pub(crate) fn is(&self, id: &str, version: &str, folder: Option<&Path>, now: &Now) -> bool {
        let marks = self.marks.lock().unwrap_or_else(PoisonError::into_inner);
        let holds = |until: &Until| match until {
            Until::Hour(hour) => *hour == now.hour,
            Until::Day(day) => *day == now.day,
            Until::Version(quiet) => quiet == version,
        };
        marks.get(&(id.to_string(), None)).is_some_and(holds)
            || folder.is_some_and(|folder| {
                marks
                    .get(&(id.to_string(), Some(folder.to_path_buf())))
                    .is_some_and(holds)
            })
    }

    /// Mark `id` quiet until `until` — in `folder` alone, or everywhere:
    /// whether this mark is new, which is what lets exactly one row say so.
    fn mark(&self, id: &str, folder: Option<&Path>, until: Until) -> bool {
        let mut marks = self.marks.lock().unwrap_or_else(PoisonError::into_inner);
        let key = (id.to_string(), folder.map(Path::to_path_buf));
        if marks.get(&key) == Some(&until) {
            return false;
        }
        marks.insert(key, until);
        true
    }
}

/// What the decisions read now: the snapshot and the hours it found full.
#[derive(Debug, Clone, Default)]
pub(crate) struct Decided {
    pub decisions: Arc<Decisions>,
    pub quiet: Arc<Quiet>,
}

impl Decided {
    /// A fresh reading of `root` and `home`, with no hour found full yet.
    pub(crate) fn read(root: &Value, home: Option<&Path>) -> Self {
        Self {
            decisions: Arc::new(Decisions::read(root, home)),
            quiet: Arc::new(Quiet::default()),
        }
    }

    /// The decisions at `point` that are not quiet `now` — and, once the
    /// pane's folder is known resolved ([`PaneBook::folder`]), that ask in
    /// it and are not quiet there. A paths' comparison, no file read: the
    /// hook loop asks it.
    fn asking_at(&self, point: Point, now: &Now, folder: Option<&Path>) -> Vec<Kept> {
        self.decisions
            .at(point)
            .filter(|kept| folder.is_none_or(|folder| custom::asks_in(&kept.definition, folder)))
            .filter(|kept| {
                !self
                    .quiet
                    .is(&kept.entry.id, &kept.entry.definition, folder, now)
            })
            .cloned()
            .collect()
    }
}

/// The folder `pane` works in, resolved, as an ask of its decisions left it
/// in its book — `None` until one has, or when the pane's hooks name another
/// folder now.
fn known_folder<'a>(book: &'a PaneBook, pane: &Pane) -> Option<&'a Path> {
    book.folder
        .as_ref()
        .filter(|(said, _)| *said == pane.worktree)
        .map(|(_, resolved)| resolved.as_path())
}

/// A line one decision earned for a pane, with what earned it: the decision
/// and the version that answered, and the launch it was earned in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Handed {
    decision: String,
    version: String,
    line: String,
}

/// What a turn's end offered the pane's next turn: the turn that ended at
/// `at`, and the lines it handed.
#[derive(Debug, Clone, Default)]
pub(crate) struct Offered {
    at: u64,
    handed: Vec<Handed>,
}

/// The lines waiting in one pane: those a command's decisions handed, for
/// the next tool boundary or turn's brief, and those a turn's end offered
/// the next turn — under the launch that earned them.
#[derive(Debug, Clone, Default)]
pub(crate) struct Waiting {
    launch: String,
    command: Vec<Handed>,
    offered: Option<Offered>,
}

/// The decisions a pane's moment asks, off the hook loop.
#[derive(Debug, Clone)]
pub(crate) struct Asked {
    point: Point,
    kept: Vec<Kept>,
    lent: Lending,
    at: u64,
}

/// What a point lends, as the hook loop holds it: what a command lends
/// whole, or a turn's end whose answer may still be in its transcript.
#[derive(Debug, Clone)]
enum Lending {
    Ready(Lent),
    TurnEnd {
        task: String,
        said: SaidAt,
        evidence: Vec<Evidence<String>>,
        edited: Vec<String>,
    },
}

/// A shell command is about to run in `pane`: the decisions asking before
/// one, unless the read-only rule clears it ([`asks_about`]), each whose hour
/// is not quiet. One call reported twice — once by an event that names no
/// call — is asked once.
pub(super) fn before_command(
    book: &mut PaneBook,
    pane: &Pane,
    decided: &Decided,
    call: &hook_guard::Call,
    command: &str,
    at: u64,
) -> Option<Asked> {
    if !asks_about(command) {
        return None;
    }
    let named = call.id.is_some();
    let seen = book
        .decided_calls
        .iter()
        .any(|(id, words)| match (&call.id, id) {
            (Some(id), Some(seen)) => id == seen,
            _ => words == command,
        })
        || (named
            && book
                .decided_calls
                .iter()
                .any(|(id, words)| id.is_none() && words == command));
    if seen {
        return None;
    }
    shelve(
        &mut book.decided_calls,
        (call.id.clone(), command.to_string()),
    );
    let kept = decided.asking_at(Point::BeforeCommand, &Now::at(at), known_folder(book, pane));
    if kept.is_empty() {
        return None;
    }
    let cwd = call.cwd.clone().unwrap_or_else(|| pane.worktree.clone());
    Some(Asked {
        point: Point::BeforeCommand,
        kept,
        lent: Lending::Ready(custom::lent_before_command(command, &cwd, &book.task)),
        at,
    })
}

/// A turn ended with its answer, not stopped: the decisions asking at a
/// turn's end, each whose hour is not quiet.
pub(super) fn at_turn_end(
    book: &PaneBook,
    pane: &Pane,
    decided: &Decided,
    said: &SaidAt,
    evidence: &[Evidence<String>],
    edited: &[String],
    at: u64,
) -> Option<Asked> {
    let kept = decided.asking_at(Point::TurnEnd, &Now::at(at), known_folder(book, pane));
    (!kept.is_empty()).then(|| Asked {
        point: Point::TurnEnd,
        kept,
        lent: Lending::TurnEnd {
            task: book.persons.clone(),
            said: said.clone(),
            evidence: evidence.to_vec(),
            edited: edited.to_vec(),
        },
        at,
    })
}

/// A turn ended, not stopped: what an earlier turn's end offered was for the
/// turn that just ended, so it is replaced now — by nothing, until this
/// turn's own decisions answer ([`place`]) — whether or not a decision asks
/// here or every one is quiet. A stopped turn replaces nothing, as zo's
/// stopped turn asks nothing. On the hook loop: the book alone.
pub(super) fn turn_ended(book: &mut PaneBook, pane: &Pane, at: u64) {
    if book.waiting.launch != pane.launch {
        book.waiting = Waiting {
            launch: pane.launch.clone(),
            ..Waiting::default()
        };
    }
    if book
        .waiting
        .offered
        .as_ref()
        .is_some_and(|later| later.at > at)
    {
        return;
    }
    book.waiting.offered = Some(Offered {
        at,
        handed: Vec::new(),
    });
}

/// Whether a line a decision at `point` earns can reach `agent` at all: the
/// next turn's brief, where its prompt hook takes context
/// ([`hook_guard::turn_start_road`]) — or, before a command, its next tool
/// boundary first. With neither, its answer is recorded and never handed.
fn has_a_road(agent: AgentKind, point: Point) -> bool {
    hook_guard::turn_start_road(agent).yes()
        || (point == Point::BeforeCommand
            && zerocode_core::hook_continuation::tool_boundary_context(agent).is_some())
}

/// Ask what `asked` names for `pane`, write each decision's row in the
/// ledger zo keeps for the pane's folder, and leave the lines the acting
/// ones earned waiting in the pane's book.
pub(super) fn ask(
    guards: &'static Mutex<Guards>,
    wire: &Wire,
    pane: &Pane,
    decided: &Decided,
    asked: Asked,
) {
    let root = pane.root();
    keep_the_folder(guards, pane, &root);
    let (Some(home), Some(ledger)) = (
        wire.config_home(),
        systemone::project_ledger_of(wire, &CUSTOM_DECISIONS, &root),
    ) else {
        // No zo home or provider to file a row under: a refusal that repeats
        // as no key does, with no ledger to say it in — quiet for the hour,
        // or until the settings are read again.
        let now = Now::at(asked.at);
        for kept in &asked.kept {
            decided
                .quiet
                .mark(&kept.entry.id, None, now.of(door::Span::Hour));
        }
        return;
    };
    let workspace = PathBuf::from(door::resolved_path(&root));
    let kept: Vec<Kept> = asked
        .kept
        .into_iter()
        .filter(|kept| custom::asks_in(&kept.definition, &workspace))
        .collect();
    if kept.is_empty() {
        return;
    }
    let lent = match asked.lent {
        Lending::Ready(lent) => lent,
        Lending::TurnEnd {
            task,
            said,
            evidence,
            edited,
        } => turn_lent(&root, task, said, &evidence, &edited),
    };
    let from = pane.agent.slug();
    let road = has_a_road(pane.agent, asked.point);
    let answered: Vec<(Option<DecisionRow>, Option<Handed>)> = std::thread::scope(|scope| {
        let asks: Vec<_> = kept
            .iter()
            .map(|one| {
                let lent = &lent;
                let workspace = root.as_path();
                scope.spawn(move || {
                    ask_one(
                        wire,
                        (home, workspace),
                        decided,
                        one,
                        lent,
                        (from, road),
                        (asked.point, asked.at),
                    )
                })
            })
            .collect();
        asks.into_iter()
            .map(|handle| handle.join().unwrap_or((None, None)))
            .collect()
    });
    let (mut rows, handed): (Vec<Option<DecisionRow>>, Vec<Option<Handed>>) =
        answered.into_iter().unzip();
    let handed: Vec<Handed> = handed.into_iter().flatten().collect();
    let placed = place(guards, pane, asked.point, asked.at, handed);
    for row in rows.iter_mut().flatten() {
        row.applied = row.applied && placed;
    }
    let rows: Vec<Value> = rows
        .into_iter()
        .flatten()
        .filter_map(|row| serde_json::to_value(row).ok())
        .collect();
    systemone::record_bounded_rows(
        &CUSTOM_DECISIONS,
        &ledger,
        &rows,
        crate::usage_runtime::epoch_ms_now(),
    );
}

/// Leave `root`, the folder `pane` works in resolved, in its book: the hook
/// loop holds the pane's next moments against it without reading a file —
/// the decisions that do not ask there, or are quiet there, start no thread.
fn keep_the_folder(guards: &'static Mutex<Guards>, pane: &Pane, root: &Path) {
    let mut held = guards.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(book) = held.panes.get_mut(&pane.term)
        && book.owner == pane.owner()
    {
        book.folder = Some((pane.worktree.clone(), root.to_path_buf()));
    }
}

/// What a turn's end lends, read now: the answer from its transcript when
/// the payload did not carry it, scrubbed as the payload's own words are;
/// the turn's newest evidence within its cap; the files its edits wrote, as
/// paths in the folder.
fn turn_lent(
    root: &Path,
    task: String,
    said: SaidAt,
    evidence: &[Evidence<String>],
    edited: &[String],
) -> Lent {
    let final_text = match said {
        SaidAt::Words(words) => words,
        SaidAt::Transcript(path) => zerocode_core::transcript::last_assistant_words(&path)
            .map(|words| zerocode_core::clone::scrub_credentials(&words))
            .unwrap_or_default(),
    };
    let evidence = door::newest_within(
        &evidence
            .iter()
            .map(|evidence| evidence.output.as_str())
            .collect::<Vec<_>>(),
        CUSTOM_EVIDENCE_BYTE_CAP,
    );
    let edited = edited
        .iter()
        .filter_map(|path| zerocode_core::jev::project_rules::store::edit_path(root, path).ok())
        .collect();
    custom::lent_at_turn_end(task, final_text, evidence, edited)
}

/// Ask one decision through the door inside its point's wall and its two
/// hours, the moment `at` its point came counted in: its row — none for a
/// refusal that repeats and another ask already wrote ([`Quiet`]) — and the
/// line it hands, when it stands on, its answer earns the line, the switch
/// is still on, the door lets the folder act now and the pane's agent has a
/// road for it.
fn ask_one(
    wire: &Wire,
    (home, workspace): (&Path, &Path),
    decided: &Decided,
    kept: &Kept,
    lent: &Lent,
    (from, road): (&str, bool),
    (point, at): (Point, u64),
) -> (Option<DecisionRow>, Option<Handed>) {
    let id = kept.entry.id.as_str();
    let version = kept.entry.definition.as_str();
    let mut row = DecisionRow::of(
        at,
        from,
        &kept.entry,
        &kept.definition,
        kept.mode,
        ANSWERED_OUTCOME,
    );
    let now = Now::at(at);
    if decided.quiet.is(id, version, Some(workspace), &now) {
        return (None, None);
    }
    if let Some(refusal) = custom::check(&kept.definition).first() {
        if !decided
            .quiet
            .mark(id, None, Until::Version(version.to_string()))
        {
            return (None, None);
        }
        refusal.word().clone_into(&mut row.outcome);
        return (Some(row), None);
    }
    let hour = now.hour.clone();
    let (own, every) = (
        count::hour_path(home, id, &hour),
        count::hour_path(home, custom::EVERY_DECISION, &hour),
    );
    let hours = DecisionHour {
        decision: &own,
        decision_most: kept.definition.max_per_hour,
        every: &every,
    };
    let answered = wire.ask_within_the_hour(
        &CUSTOM_DECISIONS,
        Some(workspace),
        custom::body_of(&kept.definition, lent),
        custom::deadline_at(point),
        hours,
    );
    row.elapsed_ms = u64::try_from(answered.waited.as_millis()).unwrap_or(u64::MAX);
    row.requests = answered.spent.requests;
    row.redacted_lines = u32::try_from(answered.spent.redacted_lines).unwrap_or(u32::MAX);
    row.input_tokens = answered.spent.input_tokens;
    row.model.clone_from(&answered.spent.model);
    let body = match answered.answer {
        Ok(body) => body,
        Err(token) => {
            // A refusal that repeats is written once a span. An hourly one
            // is one only when an hour is truly full: a lock that stayed
            // busy is a moment's.
            let refused = door::Refused::from_token(&token);
            let repeats = refused.and_then(door::Refused::repeats).filter(|_| {
                token != door::Refused::HourlyBudget.token() || wire.hour_is_full(hours)
            });
            let folder = refused
                .is_some_and(door::Refused::of_the_folder)
                .then_some(workspace);
            if let Some(span) = repeats
                && !decided.quiet.mark(id, folder, now.of(span))
            {
                return (None, None);
            }
            row.outcome = token;
            return (Some(row), None);
        }
    };
    let parsed: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let reading = match custom::read(
        &kept.definition,
        parsed.get("answers").and_then(|answers| answers.get(id)),
    ) {
        Ok(reading) => reading,
        Err(rule) => {
            systemone::SCHEMA.clone_into(&mut row.outcome);
            row.rejected = Some(rule);
            return (Some(row), None);
        }
    };
    row.chosen = Some(reading.chosen.clone());
    row.confidence = Some(reading.confidence);
    let hands = road
        && kept.mode.applies()
        && custom::earns_its_line(&kept.definition, &reading)
        && CUSTOM_DECISIONS.mode_in(&wire.settings_root()) == JevMode::On
        && wire.permits_workspace_now(Some(workspace));
    row.applied = hands;
    let handed = hands.then(|| Handed {
        decision: kept.entry.id.clone(),
        version: kept.entry.definition.clone(),
        line: custom::line(&kept.definition.name, &reading.shown, reading.confidence),
    });
    (Some(row), handed)
}

/// Leave the lines `handed` waiting in `pane`'s book: a command's after the
/// ones already there, a turn's end's in place of what an earlier turn's end
/// offered — even when it hands nothing — unless a later turn's end has
/// already offered. Whether the book still is the pane's and the launch's
/// that asked.
fn place(
    guards: &'static Mutex<Guards>,
    pane: &Pane,
    point: Point,
    at: u64,
    handed: Vec<Handed>,
) -> bool {
    let mut held = guards.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(book) = held
        .panes
        .get_mut(&pane.term)
        .filter(|book| book.owner == pane.owner())
    else {
        return false;
    };
    if book.waiting.launch != pane.launch {
        book.waiting = Waiting {
            launch: pane.launch.clone(),
            ..Waiting::default()
        };
    }
    match point {
        Point::BeforeCommand => book.waiting.command.extend(handed),
        Point::TurnEnd => {
            if book
                .waiting
                .offered
                .as_ref()
                .is_some_and(|later| later.at > at)
            {
                return false;
            }
            book.waiting.offered = Some(Offered { at, handed });
        }
        Point::TurnStart | Point::AgentAsks => return false,
    }
    true
}

/// zo's settings were read again: drop every line waiting in a pane whose
/// folder the door no longer lets act — consent withdrawn, Jev turned off —
/// here, on the watcher's road, so taking a line on a hook's reply reads no
/// file ([`take`]). The folders are asked outside the books' lock.
pub(super) fn drop_lines_the_door_refuses(guards: &Mutex<Guards>, wire: &Wire) {
    let waiting: Vec<(u32, PathBuf)> = guards
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .panes
        .iter()
        .filter(|(_, book)| !book.waiting.command.is_empty() || book.waiting.offered.is_some())
        .filter_map(|(term, book)| Some((*term, book.folder.as_ref()?.1.clone())))
        .collect();
    let refused: Vec<(u32, PathBuf)> = waiting
        .into_iter()
        .filter(|(_, folder)| !wire.permits_workspace_now(Some(folder)))
        .collect();
    if refused.is_empty() {
        return;
    }
    let mut held = guards.lock().unwrap_or_else(PoisonError::into_inner);
    for (term, folder) in refused {
        if let Some(book) = held
            .panes
            .get_mut(&term)
            .filter(|book| book.folder.as_ref().is_some_and(|(_, own)| *own == folder))
        {
            book.waiting = Waiting {
                launch: book.waiting.launch.clone(),
                ..Waiting::default()
            };
        }
    }
}

/// Take the lines waiting in the pane `pane_key` names for the knock of
/// `launch` (and `session`, when it names one) — a command's decisions', and
/// with `offered_too` what the last turn's end offered: none for a knock from another
/// launch of the pane, which leaves them where they were. Those whose
/// decision no longer stands on at the version that answered are dropped;
/// of the rest, each that fits under `room` after the ones before it, one to
/// a line. The snapshot alone is read — no file: a folder the door stopped
/// letting act lost its lines when the settings were read again
/// ([`drop_lines_the_door_refuses`]). Taken lines are gone whether or not
/// the reply that carries them is read: at most once, not exactly once.
fn take(
    guards: &Mutex<Guards>,
    decided: &Decided,
    pane_key: &str,
    launch: &str,
    session: Option<&str>,
    offered_too: bool,
    room: usize,
) -> Option<String> {
    if decided.decisions.kept.is_empty() {
        return None;
    }
    let term = crate::hooks::term_of_pane_key(pane_key)?;
    let handed = {
        let mut held = guards.lock().unwrap_or_else(PoisonError::into_inner);
        let book = held.panes.get_mut(&term)?;
        let pane = book.pane.as_ref()?;
        let other_launch =
            !launch.is_empty() && !book.waiting.launch.is_empty() && launch != book.waiting.launch;
        let other_session = session.is_some_and(|session| {
            !session.is_empty() && pane.session.as_deref().is_some_and(|own| own != session)
        });
        if other_launch || other_session {
            return None;
        }
        let mut handed = std::mem::take(&mut book.waiting.command);
        if offered_too {
            handed.extend(
                book.waiting
                    .offered
                    .take()
                    .map(|offered| offered.handed)
                    .into_iter()
                    .flatten(),
            );
        }
        handed
    };
    let mut said = String::new();
    for one in handed
        .iter()
        .filter(|one| decided.decisions.stands_on(&one.decision, &one.version))
    {
        let gap = usize::from(!said.is_empty());
        if said.chars().count() + gap + one.line.chars().count() > room {
            continue;
        }
        if gap > 0 {
            said.push('\n');
        }
        said.push_str(&one.line);
    }
    (!said.is_empty()).then_some(said)
}

/// The decisions' voice in a turn's brief: every line waiting in the
/// asking pane — the person's own as well as a summoned worker's — that
/// fits the room the voices before it left.
pub(super) fn brief_line(ask: &zerocode_hookd::TurnBriefAsk, room: usize) -> Option<String> {
    brief_in(&GUARDS, &STANDING.decided(), ask, room)
}

pub(super) fn brief_in(
    guards: &Mutex<Guards>,
    decided: &Decided,
    ask: &zerocode_hookd::TurnBriefAsk,
    room: usize,
) -> Option<String> {
    take(
        guards,
        decided,
        &ask.pane_key,
        &ask.launch_token,
        ask.session_id.as_deref(),
        true,
        room,
    )
}

/// The lines a command's decisions handed the pane `pane_key` names, for
/// the reply to its agent's tool boundary
/// ([`zerocode_core::hook_continuation::tool_boundary_context`]): taken now,
/// never waited for, within the brief's cap. What a turn's end offered waits
/// for the next turn's brief.
pub(crate) fn take_for_the_tool_boundary(pane_key: &str, launch: &str) -> Option<String> {
    boundary_in(&GUARDS, &STANDING.decided(), pane_key, launch)
}

pub(super) fn boundary_in(
    guards: &Mutex<Guards>,
    decided: &Decided,
    pane_key: &str,
    launch: &str,
) -> Option<String> {
    take(
        guards,
        decided,
        pane_key,
        launch,
        None,
        false,
        zerocode_hookd::TURN_BRIEF_CHAR_CAP,
    )
}

#[cfg(test)]
mod tests;
