//! The one door an answer prepared from a card takes into a pane.
//!
//! A card is drawn from what a pane's agent said it asked, and answered some
//! moments later by typing into the pane. In between the question may have
//! been answered at the keyboard, resolved by the agent, or replaced by the
//! next one, and a key typed then lands in whatever the screen shows now — a
//! digit in somebody else's menu, an Escape that stops a running turn.
//!
//! So every road that types an answer a card prepared (`answer_ask`,
//! `answer_approval`) — and the lines the window types by itself that a menu
//! would take for an answer (the hand-over's exit command, a CLI's way back
//! to an earlier message) — goes through here, and here the screen is read
//! again under the pane's own terminal lock, in the same hold as the write:
//! [`type_if_up`] writes only if the screen shows what the caller needs. The pump needs that lock to move
//! output into the grid, so the screen read and the key are not separated by a
//! redraw this window parsed.
//!
//! One answer at a time per pane ([`answer_lease`]), on the lease a
//! `zerocode-ssh send` already takes — the same pane, the same "two writers
//! must not cross" rule.
//!
//! The reading itself is `zerocode_core::screen_menu`'s. This module owns the
//! terminal, its lock and its clock; core owns what a menu is.
//!
//! **What does not come through here, and why.** A person's own keys, paste
//! and wheel (`term_key`, `term_paste`, `term_scroll`): the person is looking
//! at the screen that takes them. A queued prompt (`PromptDelivery`): it has
//! its own guard on the composer line it writes to. An agent team's
//! `send-keys`, the restart nudge and the usage probes: nothing was prepared
//! from an earlier read of the screen, so there is nothing here to be late.

use super::*;
use zerocode_core::agent::turn_refusal as refused;
use zerocode_core::ask::{AskPrompt, AskQuestion, AskSelection};
use zerocode_core::screen_menu::{self, ScreenMenu};
use zerocode_core::secret_prompt::SecretPrompt;

use crate::terminal_registry::HeldTerminal;

/// The word a refused answer carries to the page when the question it was
/// prepared for is no longer the one on the screen. The page maps it to its
/// own sentence and shows the pane's question as it is now.
pub(crate) const QUESTION_CHANGED: &str = "question-changed";

/// The word a refused answer carries when another answer to the same pane is
/// still being typed or has not yet been shown to land.
pub(crate) const ANSWER_IN_FLIGHT: &str = "answer-in-flight";

/// The word a refused answer carries when a numbered menu was answered with
/// only words: a menu takes a row.
pub(crate) const MENU_NEEDS_A_ROW: &str = "menu-needs-a-row";

/// The word a refused secret carries when its value cannot be typed as one
/// line of visible text (`zerocode_core::secret_prompt::value_is_typable`).
pub(crate) const SECRET_INVALID: &str = "secret-invalid";

/// How long a pane stays "being answered" after the last key of an answer.
///
/// The child needs a redraw to show it took the key. A second answer inside
/// that redraw would verify against the screen the first one already
/// answered and type again — the double answer the lease exists to prevent.
/// Short enough that a person answering one question and then the next one it
/// raises never meets it.
pub(super) const ANSWER_SETTLE: Duration = Duration::from_millis(400);

/// Why an answer was not typed.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Refusal {
    /// The screen no longer shows the question the answer was prepared for,
    /// or the selection is not where the answer needs it.
    Changed,
    /// Another answer to this pane is still on its way in.
    InFlight,
    /// The pty would not take the key.
    Pty(String),
}

impl Refusal {
    /// The error string a command returns.
    pub(super) fn into_message(self) -> String {
        match self {
            Self::Changed => QUESTION_CHANGED.to_string(),
            Self::InFlight => ANSWER_IN_FLIGHT.to_string(),
            Self::Pty(error) => error,
        }
    }
}

/// What the screen has to show for a prepared answer to be typed.
pub(super) struct Expect<'a> {
    question: Option<&'a AskQuestion>,
    row: Option<usize>,
    /// No numbered menu at all — for words the window types by itself.
    absent: bool,
    /// And this agent's input line shown empty (`composer_empty::observed`)
    /// — for a line that must stand alone on it (t-84850 r2).
    empty_line: Option<&'a str>,
}

impl<'a> Expect<'a> {
    /// A live numbered menu of any question — a permission card has no rows
    /// of its own to compare, only that it is a menu that asks.
    pub(super) const fn any_menu() -> Self {
        Self {
            question: None,
            row: None,
            absent: false,
            empty_line: None,
        }
    }

    /// No numbered menu on the screen: the pane is at a prompt that takes
    /// words. A line the window types by itself — the hand-over's exit command
    /// — is guidance, and Enter on a menu takes the highlighted row, which
    /// may be a permission nobody gave.
    pub(super) const fn no_menu() -> Self {
        Self {
            question: None,
            row: None,
            absent: true,
            empty_line: None,
        }
    }

    /// This question, as the screen's menu.
    pub(super) const fn question(question: &'a AskQuestion) -> Self {
        Self {
            question: Some(question),
            row: None,
            absent: false,
            empty_line: None,
        }
    }

    /// And the selection standing on this row.
    pub(super) const fn on_row(mut self, row: usize) -> Self {
        self.row = Some(row);
        self
    }

    /// And `agent`'s input line shown empty: words the window types by itself
    /// that must not join a draft standing there.
    pub(super) const fn with_empty_line(mut self, agent: &'a str) -> Self {
        self.empty_line = Some(agent);
        self
    }
}

/// The visible rows of a pane's screen and where its cursor stands — what
/// `screen_menu` reads. Copied out so the lock is held for the copy and not
/// for the reading.
fn screen_of(pty: &PtyHandle) -> (Vec<String>, Option<usize>) {
    let grid = pty.terminal().grid();
    let rows = (0..grid.screen_rows()).map(|row| grid.line(row)).collect();
    (rows, Some(grid.cursor().0))
}

/// Whether the screen shows what `expect` says.
fn shows(pty: &PtyHandle, expect: &Expect<'_>) -> bool {
    if let Some(agent) = expect.empty_line
        && !crate::composer_empty::observed(agent, pty.terminal().grid())
    {
        return false;
    }
    let (rows, cursor) = screen_of(pty);
    if expect.absent {
        return screen_menu::read_menu(&rows, cursor).is_none();
    }
    let on_row = |menu: &ScreenMenu| expect.row.is_none_or(|row| menu.selected == row);
    match expect.question {
        // A question with no rows to compare: its words on the last rows.
        Some(question) if question.options.is_empty() => {
            screen_menu::words_are_up(&rows, &question.question)
        }
        Some(question) => screen_menu::read_menu(&rows, cursor)
            .is_some_and(|menu| menu.shows(question) && on_row(&menu)),
        None => screen_menu::read_menu(&rows, cursor).is_some_and(|menu| on_row(&menu)),
    }
}

/// Whether the pane's screen still shows `question`, read and nothing typed
/// — what a worker's question the window wrote down before a restart is held
/// to before it is carried again (t-82296).
pub(super) fn question_is_up(held: &HeldTerminal, question: &AskQuestion) -> bool {
    shows(&lock_pty(held), &Expect::question(question))
}

/// Type `bytes` into the pane if — and only if — the screen shows what
/// `expect` says, read and written in one hold of the pane's lock.
///
/// `Ok(false)` is a refusal: nothing was typed.
pub(super) fn type_if_up(
    held: &HeldTerminal,
    expect: &Expect<'_>,
    bytes: &[u8],
) -> Result<bool, PtyTransportError> {
    let mut pty = lock_pty(held);
    if !shows(&pty, expect) {
        return Ok(false);
    }
    pty.write_input(bytes)?;
    Ok(true)
}

/// Why the window would not type a CLI's way back to an earlier message
/// (draft C10, t-82196) — the words the page turns into its sentence. The
/// pane's agent is not between turns (the hook's word).
pub(crate) const REWIND_BUSY: &str = "rewind-busy";

/// The pane's screen shows a menu, or words on its input line: the way back's
/// Enter would take the menu's highlighted row, or send those words with it.
pub(crate) const REWIND_SCREEN: &str = "rewind-screen";

/// A run's worker or coordinator sits in the pane: winding its conversation
/// back is the run's business, and the ledger would not hear of it.
pub(crate) const REWIND_SEAT: &str = "rewind-seat";

/// The ledger cannot be read now, so the pane may be a run's coordinator's.
pub(crate) const REWIND_LEDGER_UNREAD: &str = "rewind-ledger-unread";

/// What the window knows of a pane before it types a CLI's way back into it.
pub(super) struct RewindPane<'a> {
    /// The ledger's reason the pane is a run's
    /// (`cmd::wire::pane_the_ledger_drives`), already the page's word.
    pub(super) seat: Option<&'static str>,
    /// Its agent ended its turn and asks nothing (the hook's word), and its output has gone quiet
    /// ([`wait_for_rest`]): a message the CLI held in its own queue begins its turn with no hook, and
    /// the way back typed on that turn would open over a turn that is running (t-86909 r4).
    pub(super) between_turns: bool,
    /// The agent whose input line the screen is read for.
    pub(super) agent: &'a str,
}

/// The page's word for the ledger's reason a pane is a run's: `unread` when
/// the ledger could not be read, `seat` for a worker's or a coordinator's seat.
/// One reading for every door that asks the ledger first (the way back, the
/// queue's three).
fn seat_word(
    reason: Option<&str>,
    unread: &'static str,
    seat: &'static str,
) -> Option<&'static str> {
    reason.map(|said| {
        if said == crate::cmd::wire::LEDGER_UNREAD_PANE_STAYS {
            unread
        } else {
            seat
        }
    })
}

/// [`seat_word`] as the way back says it.
pub(crate) fn rewind_seat_word(reason: Option<&str>) -> Option<&'static str> {
    seat_word(reason, REWIND_LEDGER_UNREAD, REWIND_SEAT)
}

/// The first key group of a CLI's way back — Claude Code's `/rewind` — typed
/// into the pane only when the pane takes it (draft C10, t-82196): no run
/// sits in it, nor may (a ledger that cannot be read), its agent is between
/// turns, and — read in the same hold as the write — its screen shows its
/// input line empty and no menu, whose Enter would take the highlighted row:
/// a permission nobody gave. The hand-over's exit command takes the same last
/// look (t-84850). A refusal types nothing and carries the word the page says
/// it with.
pub(super) fn type_rewind(
    held: &HeldTerminal,
    pane: &RewindPane<'_>,
    first: &[u8],
) -> Result<(), String> {
    if let Some(word) = pane.seat {
        return Err(word.to_string());
    }
    if !pane.between_turns {
        return Err(REWIND_BUSY.to_string());
    }
    match type_on_a_free_line(held, pane.agent, first) {
        Ok(true) => Ok(()),
        Ok(false) => Err(REWIND_SCREEN.to_string()),
        Err(error) => Err(error),
    }
}

/// [`type_if_up`] for a line the window types by itself: only while the
/// screen shows no menu — whose highlighted row an Enter would take — and
/// `agent`'s input line shown empty, so nothing joins words a person left
/// there. `Ok(false)` is a refusal: nothing was typed. The look every door
/// that types at a pane on the person's behalf takes first (the way back, the
/// queue's steer and stop).
fn type_on_a_free_line(held: &HeldTerminal, agent: &str, bytes: &[u8]) -> Result<bool, String> {
    type_if_up(held, &Expect::no_menu().with_empty_line(agent), bytes)
        .map_err(|error| error.to_string())
}

/// [`seat_word`] as the turn doors say it: a seat, or a ledger that cannot be
/// read.
pub(crate) fn turn_seat_word(reason: Option<&str>) -> Option<&'static str> {
    seat_word(reason, refused::LEDGER_UNREAD, refused::SEAT)
}

/// What the window knows of a pane before it types at the turn running in it.
pub(super) struct TurnPane<'a> {
    /// The ledger's reason the pane is a run's, already the page's word
    /// ([`turn_seat_word`]).
    pub(super) seat: Option<&'static str>,
    /// The hook's word is that a turn runs — working, not at rest and not
    /// asking.
    pub(super) working: bool,
    /// The agent whose input line the screen is read for.
    pub(super) agent: &'a str,
    /// The pane's CLI has shown it reads its input (`human_input::reads_input`).
    /// A pane restored a moment ago may stand "working" by a hook word that
    /// outlived the restart while its CLI is still loading — and keys typed at
    /// it go to whatever reads the terminal then.
    pub(super) reads_input: bool,
}

/// What is asked of a pane before its screen is looked at (t-86909 r2): that
/// no run's seat holds it, that the hook's word is the one the words are for —
/// a turn that runs for a steer or a stop, one that is over for the words of a
/// send now — and that its CLI has shown it reads its input.
fn before_the_screen(pane: &TurnPane<'_>, moment: Moment) -> Result<(), String> {
    if let Some(word) = pane.seat {
        return Err(word.to_string());
    }
    match (moment, pane.working) {
        (Moment::During, false) => return Err(refused::IDLE.to_string()),
        (Moment::After, true) => return Err(refused::RUNNING.to_string()),
        _ => {}
    }
    if !pane.reads_input {
        return Err(refused::NOT_READING.to_string());
    }
    Ok(())
}

/// The first key group typed at a pane whose turn RUNS (t-86909): the words a
/// queued message steers into it, or the key that stops it. Typed only when no
/// run sits in the pane — nor may, when the ledger cannot be read — the
/// agent's turn runs, its CLI has shown it reads its input, and — read in the
/// same hold as the write — the screen shows its input line empty and no menu:
/// never onto a permission menu, where an Escape would refuse it and an Enter
/// take its highlighted row, nor after words a person left on the line. A
/// refusal types nothing and carries the word the page says it with.
pub(super) fn type_at_a_turn(
    held: &HeldTerminal,
    pane: &TurnPane<'_>,
    first: &[u8],
) -> Result<(), String> {
    before_the_screen(pane, Moment::During)?;
    match type_on_a_free_line(held, pane.agent, first) {
        Ok(true) => Ok(()),
        Ok(false) => Err(refused::SCREEN.to_string()),
        Err(error) => Err(error),
    }
}

/// The Enter that sends words [`type_at_a_turn`] put on the line, typed
/// through the same door. The screen is asked again — a menu that came up in
/// the breath between would take this Enter for its highlighted row — and so
/// is the person's hand: one that reached the line since (`hand`, the count
/// read before the words went in) may have added to the words or sent them,
/// and no Enter of the window's follows it. Any refusal here is
/// [`refused::LANDED`]: the words are on the pane's line already.
pub(super) fn type_enter_after_words(
    held: &HeldTerminal,
    term: u32,
    hand: Option<u64>,
    enter: &[u8],
) -> Result<(), String> {
    if crate::human_input::line_of(term).1 != hand {
        return Err(refused::LANDED.to_string());
    }
    match type_if_up(held, &Expect::no_menu(), enter) {
        Ok(true) => Ok(()),
        Ok(false) | Err(_) => Err(refused::LANDED.to_string()),
    }
}

/// Which side of its turn a line the queue types is for (t-86909 r2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Moment {
    /// Into the turn that runs — the 끼워 넣기's words: the hook says a turn runs.
    During,
    /// After the stop that cleared the way, the turn over — the 바로 보내기's own
    /// words: the hook no longer says a turn runs.
    After,
}

/// How long the queue's typing waits (t-86909 r2).
#[derive(Clone, Copy, Debug)]
pub(super) struct Timing {
    /// The breath between the words and their Enter (`line_keys`).
    pub(super) step: Duration,
    /// How long a screen that does not yet show the empty line is given to —
    /// the redraw after a stop may trail the hook's word.
    pub(super) wait: Duration,
}

/// How often the screen is looked at while the door waits for the empty line.
const SCREEN_LOOK_EVERY: Duration = Duration::from_millis(50);

/// How long a pane's output must have been silent before the pane counts as at rest (t-86909 r4).
///
/// The hook's word that a turn is over is not enough: Claude Code sends no hook when a message it
/// held in its own queue begins its turn, so the word says rest, and the input field is empty again,
/// while that turn runs. Measured on 2.1.316: a turn draws a spinner 18 times in 2 s, and a CLI at rest
/// draws nothing in 5 s. The slowest redraw of a reduced spinner is its seconds counter, once a second,
/// so this is half a second over it.
pub(super) const REST_QUIET: Duration = Duration::from_millis(1_500);
/// How long a pane that is not yet quiet is waited for before it is called busy (t-86909 r4): a turn
/// that has just ended is still drawing its last frames, and one that has started again keeps drawing.
pub(super) const REST_WAIT: Duration = Duration::from_secs(3);

/// Whether a pane that last wrote at `last` has been silent for `quiet` by `now`. A transport that
/// does not say when it last wrote is read as quiet, as [`secret_prompt`] reads it: nothing shows
/// otherwise, and the pane's other facts (the hook's word, the screen) still have to hold.
fn silent_for(last: Option<Instant>, now: Instant, quiet: Duration) -> bool {
    last.is_none_or(|at| now.saturating_duration_since(at) >= quiet)
}

/// [`silent_for`] for the pane `pty` as it stands now — a look the caller makes under the lock it
/// already holds, so it can stand in one hold with the other facts of a pane.
pub(super) fn output_quiet(pty: &PtyHandle, quiet: Duration) -> bool {
    silent_for(pty.last_output_at(), Instant::now(), quiet)
}

/// Look every `poll` until `quiet` answers yes, for at most `wait`: what the last look found. A pane
/// that went quiet at the very end counts as quiet.
fn wait_until(mut quiet: impl FnMut() -> bool, wait: Duration, poll: Duration) -> bool {
    let deadline = Instant::now() + wait;
    loop {
        if quiet() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(poll);
    }
}

/// Wait up to `wait` for the output of the pane `held` to have been silent for `quiet`, looking every
/// `poll`; whether it is quiet at the end. The pane's lock is taken for each look and not kept across
/// the wait, so the pump that moves its output goes on.
pub(super) fn wait_for_quiet(
    held: &HeldTerminal,
    quiet: Duration,
    wait: Duration,
    poll: Duration,
) -> bool {
    wait_until(|| output_quiet(&lock_pty(held), quiet), wait, poll)
}

/// [`wait_for_quiet`] with the window's own bounds ([`REST_QUIET`], [`REST_WAIT`]): the one question
/// the hand-over's second look and the way back ask of a pane whose hook says its turn is over. The
/// caller reads [`output_quiet`] again in the hold it reads its other facts in — this wait only gives
/// a pane that is about to be quiet the time to be.
pub(super) fn wait_for_rest(held: &HeldTerminal) -> bool {
    wait_for_quiet(held, REST_QUIET, REST_WAIT, SCREEN_LOOK_EVERY)
}

/// [`type_at_a_turn`] for the words of a send now, after its stop (t-86909 r2):
/// the hook says the turn is over, and the screen is given up to `wait` to show
/// the input line empty and no menu — the redraw after a stop can trail the
/// hook's word. Words the stop took back onto the line (a steer the CLI had not
/// taken yet) stay there and refuse these as [`refused::LINE`]: pasted behind
/// them, an Enter would send both as one message.
///
/// The hook's word is read once, by [`before_the_screen`], and not again while the
/// screen is waited for (t-86909 r3, P3-6: left as it is). A turn that starts in
/// those `wait` seconds — a steered row the CLI began on its own after the stop, say —
/// is not seen, and the words go onto the line of a pane that is working again: its own
/// queue takes them and folds them in at its next step. Nothing is lost and nothing is
/// sent twice; only the order is the CLI's. Reading the hook's word at each look would
/// refuse such words as [`refused::RUNNING`] instead, and the page would ask again.
fn type_after_a_turn(
    held: &HeldTerminal,
    pane: &TurnPane<'_>,
    first: &[u8],
    wait: Duration,
) -> Result<(), String> {
    before_the_screen(pane, Moment::After)?;
    let deadline = std::time::Instant::now() + wait;
    loop {
        match type_on_a_free_line(held, pane.agent, first) {
            Ok(true) => return Ok(()),
            Ok(false) if std::time::Instant::now() < deadline => {
                std::thread::sleep(SCREEN_LOOK_EVERY);
            }
            Ok(false) => return Err(refused::LINE.to_string()),
            Err(error) => return Err(error),
        }
    }
}

/// A line the queue types at a pane — the words as one bracketed paste, a
/// breath, their Enter — every key through the door (t-86909 r2).
///
/// The words are ALWAYS a bracketed paste, a single line too: bare, a Tab was
/// the CLI's Tab and a `^C` its Ctrl+C. The door reads the screen first and
/// types onto an empty field with no menu only, which for the two CLIs it can
/// read means bracketed paste is on. `Moment::During` is a steer into a running
/// turn; `Moment::After` the words of a send now, whose Enter is a gesture the
/// provider's prompt report answers, so the tracker is told what the window's
/// ordinary delivery tells it (`prompt_transaction::human_write`).
///
/// A person's hand that reaches the line between the words and the Enter ends
/// the Enter ([`type_enter_after_words`]): [`refused::LANDED`].
pub(super) fn type_queued_line(
    held: &HeldTerminal,
    term: u32,
    pane: &TurnPane<'_>,
    moment: Moment,
    text: &str,
    timing: Timing,
) -> Result<(), String> {
    let words = zerocode_pty::encode_paste(text, true);
    let enter = zerocode_core::ask::line_keys(text)
        .0
        .iter()
        .map(crate::cmd::terminal::key_group_bytes)
        .next_back()
        .ok_or_else(|| refused::UNSUPPORTED.to_string())?;
    // The hand count before the words go in: one that reached the line by the time the Enter is
    // due may have added to them or sent them.
    let mut hand = crate::human_input::line_of(term).1;
    match moment {
        Moment::During => type_at_a_turn(held, pane, &words)?,
        Moment::After => {
            type_after_a_turn(held, pane, &words, timing.wait)?;
            // The words are on the line: a draft until their Enter closes over them. The mark
            // moves the count, so the Enter is held to the count after it.
            crate::human_input::typed(term);
            hand = crate::human_input::line_of(term).1;
        }
    }
    std::thread::sleep(timing.step);
    type_enter_after_words(held, term, hand, &enter)?;
    if moment == Moment::After {
        crate::human_input::entered(term);
    }
    Ok(())
}

/// What typing the hand-over's probe at a pane showed (t-84850 r2).
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Probe {
    /// It showed on the field and was erased: the CLI reads its input.
    Read,
    /// Typed, and not shown within the wait: the CLI is not reading yet. The
    /// probe may still show later, for [`erase_probe`] to take away.
    Deaf,
    /// Not typed: the screen stood on a menu, or its field was not empty.
    NotTyped,
    /// Not typed: an earlier probe at this pane has not shown yet and is
    /// still looked for — a second space would join it and outlive both.
    StillOut,
    /// Typed, and a person's hand reached the line before it showed: it is
    /// left where it is, ahead of their words.
    Touched,
}

/// The panes an unshown probe is still out on, while [`probe_pane`]'s late
/// look runs (t-84850 r3).
static PROBES_OUT: std::sync::Mutex<std::collections::BTreeSet<u32>> =
    std::sync::Mutex::new(std::collections::BTreeSet::new());

fn probes_out() -> std::sync::MutexGuard<'static, std::collections::BTreeSet<u32>> {
    PROBES_OUT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// [`probe_reading`] for pane `term`, at most one probe out at a time. One
/// that does not show within `wait` is looked for up to `late` more, on a
/// thread of its own, and taken away when it shows — unless a person's hand
/// reaches the line first, whose words then follow it, or the pane goes.
/// While that look runs, the pane is not probed again ([`Probe::StillOut`]).
pub(super) fn probe_pane(
    held: &HeldTerminal,
    term: u32,
    agent: &str,
    wait: Duration,
    late: Duration,
    poll: Duration,
) -> Result<Probe, PtyTransportError> {
    if !probes_out().insert(term) {
        return Ok(Probe::StillOut);
    }
    let (probed, hand) = match probe_reading(held, term, agent, wait, poll) {
        Ok((Probe::Deaf, hand)) => (Probe::Deaf, hand),
        other => {
            probes_out().remove(&term);
            return other.map(|(probed, _)| probed);
        }
    };
    let held = Arc::clone(held);
    let agent = agent.to_string();
    let looked = std::thread::Builder::new()
        .name(format!("hand-over-probe-{term}"))
        .spawn(move || {
            let began = std::time::Instant::now();
            while began.elapsed() < late && Arc::strong_count(&held) > 1 {
                match erase_probe(&held, term, &agent, hand) {
                    // Drawn late and taken away: the CLI reads now, and the
                    // next press is not probed again (t-84850 r3).
                    Ok(Erased::Done) => {
                        crate::human_input::saw_reading(term);
                        break;
                    }
                    Ok(Erased::NotShown) => std::thread::sleep(poll),
                    Ok(Erased::Touched) | Err(_) => break,
                }
            }
            probes_out().remove(&term);
        });
    if looked.is_err() {
        probes_out().remove(&term);
    }
    Ok(probed)
}

/// The probe: one space, harmless on an empty field and nothing a CLI reads as
/// a command.
const PROBE_KEY: &[u8] = b" ";
/// The key that takes it away again.
const PROBE_ERASE: &[u8] = b"\x7f";

/// Whether the pane's CLI reads what is typed at it, asked by typing one space
/// at its empty field — through this door, so never onto a menu or a draft —
/// and looking up to `wait` (every `poll`) for the field to show it; once it
/// shows, it is erased in the same hold of the pane's lock it was seen in.
/// A person's hand that reaches the line meanwhile stops the look with
/// nothing erased ([`Probe::Touched`]). Also answers the hand count read
/// before the probe was typed, for a later look to hold its erasing to.
pub(super) fn probe_reading(
    held: &HeldTerminal,
    term: u32,
    agent: &str,
    wait: Duration,
    poll: Duration,
) -> Result<(Probe, Option<u64>), PtyTransportError> {
    let hand = crate::human_input::line_of(term).1;
    if !type_if_up(held, &Expect::no_menu().with_empty_line(agent), PROBE_KEY)? {
        return Ok((Probe::NotTyped, hand));
    }
    let began = std::time::Instant::now();
    loop {
        match erase_probe(held, term, agent, hand)? {
            Erased::Done => return Ok((Probe::Read, hand)),
            Erased::Touched => return Ok((Probe::Touched, hand)),
            Erased::NotShown if began.elapsed() >= wait => return Ok((Probe::Deaf, hand)),
            Erased::NotShown => std::thread::sleep(poll),
        }
    }
}

/// What [`erase_probe`] did.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Erased {
    /// The field showed exactly the probe, and it was taken away.
    Done,
    /// The field does not show it (yet): nothing typed.
    NotShown,
    /// A person's hand reached the line since the probe was typed: nothing
    /// typed, and nothing ever will be — the backspace could take their key.
    Touched,
}

/// Take the probe away if — and only if — no person's hand has reached pane
/// `term`'s line since `hand` (the hand count read before the probe was
/// typed) and the field shows exactly the probe, both read and the key
/// written in one hold of the pane's lock.
pub(super) fn erase_probe(
    held: &HeldTerminal,
    term: u32,
    agent: &str,
    hand: Option<u64>,
) -> Result<Erased, PtyTransportError> {
    let mut pty = lock_pty(held);
    if crate::human_input::line_of(term).1 != hand {
        return Ok(Erased::Touched);
    }
    if !crate::composer_empty::holds_probe(agent, pty.terminal().grid()) {
        return Ok(Erased::NotShown);
    }
    pty.write_input(PROBE_ERASE)?;
    Ok(Erased::Done)
}

/// Type one key group of a walk, refusing if the question is gone.
fn press(
    held: &HeldTerminal,
    expect: &Expect<'_>,
    group: &zerocode_core::ask::KeyGroup,
    wake: &dyn Fn(),
) -> Result<(), Refusal> {
    let typed = type_if_up(held, expect, &crate::cmd::terminal::key_group_bytes(group))
        .map_err(|error| Refusal::Pty(error.to_string()))?;
    wake();
    if typed { Ok(()) } else { Err(Refusal::Changed) }
}

/// The one row a card's answer picked on a numbered menu, or `None` when the
/// answer is not one pick of one single-select question: words only, several
/// picks, a multi-select, nothing answered. A menu takes a row — not a
/// sentence and not a set.
pub(super) fn chosen_row(question: &AskQuestion, selections: &[AskSelection]) -> Option<usize> {
    match selections.first()?.indices.as_slice() {
        [row] if !question.multi_select => Some(*row),
        _ => None,
    }
}

/// Choose row `to` of the menu `question` is on a pane's screen the way a
/// hand would: walk the selection there, look that it landed, then take it.
///
/// The look is the point. A menu nobody measured may take an arrow key
/// differently, and a pace that outran it would otherwise Enter the wrong
/// row; here it ends as [`Refusal::Changed`] with no Enter typed.
///
/// `wake` tells the window's pump the child has been written to — after every
/// key, because the screen the look reads is the one the pump parses from the
/// child's answer.
pub(super) fn choose_row(
    held: &HeldTerminal,
    question: &AskQuestion,
    to: usize,
    step: Duration,
    settle: Duration,
    wake: &dyn Fn(),
) -> Result<(), Refusal> {
    let (rows, cursor) = screen_of(&lock_pty(held));
    let from = screen_menu::read_menu(&rows, cursor)
        .filter(|menu| menu.shows(question) && to < menu.options.len())
        .map(|menu| menu.selected)
        .ok_or(Refusal::Changed)?;
    let on_question = Expect::question(question);
    let walk = zerocode_core::ask::walk_to_row(from, to);
    for group in &walk {
        press(held, &on_question, group, wake)?;
        std::thread::sleep(step);
    }
    // Nothing moved when the selection was already on the row, so there is
    // nothing for the screen to settle after.
    if !walk.is_empty() {
        std::thread::sleep(settle);
    }
    // The look: the selection has to be on the chosen row, and it is checked
    // in the same hold as the Enter.
    let on_the_row = Expect::question(question).on_row(to);
    for group in zerocode_core::ask::press_enter() {
        press(held, &on_the_row, &group, wake)?;
    }
    Ok(())
}

/// The card a waiting pane's screen is drawn as, when the screen shows a
/// numbered menu — for a pane whose agent said only that it waits.
///
/// Never blocks: this is asked from the window's own thread, and a pane whose
/// terminal is busy being parsed is simply not read this time; the next ask
/// finds it.
pub(super) fn screen_card(held: &HeldTerminal) -> Option<AskPrompt> {
    let (rows, cursor) = {
        let pty = match held.try_lock() {
            Ok(pty) => pty,
            Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return None,
        };
        screen_of(&pty)
    };
    screen_menu::read_menu(&rows, cursor)?.card()
}

/// How long a pane's output must have been silent before its last line is
/// read as a question the pane waits on. A line still being printed is not
/// yet a question.
pub(super) const SECRET_QUIET: Duration = Duration::from_millis(300);

/// The secret the pane's question asks for, once the pane has been quiet for
/// [`SECRET_QUIET`] and a value may be typed into it ([`secret_route_open`]).
/// `None` while the pane is still printing, for any last line that is not such
/// a question, and for a question no value may reach.
///
/// Never blocks: a pane whose terminal is being parsed this instant is simply
/// not read this time, as a numbered menu's card is not either.
pub(super) fn secret_prompt(held: &HeldTerminal, agent_pane: bool) -> Option<SecretPrompt> {
    let pty = match held.try_lock() {
        Ok(pty) => pty,
        Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => return None,
    };
    // A transport that does not say when it last wrote is read as quiet; the
    // reading itself still needs the question on the cursor's last row.
    let quiet = pty
        .last_output_at()
        .is_none_or(|at| at.elapsed() >= SECRET_QUIET);
    if !quiet {
        return None;
    }
    let (rows, cursor) = screen_of(&pty);
    let prompt = zerocode_core::secret_prompt::read_secret_prompt(&rows, cursor?)?;
    secret_route_open(&pty, agent_pane).then_some(prompt)
}

/// Whether a typed secret may reach the pane's question now.
///
/// The terminal's foreground must be a job the pane's own shell started, not the
/// pane's child: a child that holds the terminal is the shell at its prompt, or
/// the agent the pane runs as, and the agent reads whatever is typed. A
/// foreground named as an agent is refused for the same reason. A terminal that
/// echoes is refused, because the value would stay on the screen and in
/// scrollback. An agent pane must also name the program in front, or it cannot
/// be told apart from its agent.
///
/// Every answer the terminal cannot give refuses, except the echo: a terminal
/// that cannot say whether it echoes is not refused, and its card promises only
/// what this window keeps.
pub(super) fn secret_route_open(pty: &PtyHandle, agent_pane: bool) -> bool {
    let (Some(child), Some(holder)) = (pty.pid(), pty.foreground_process_id()) else {
        return false;
    };
    if holder == child || pty.echo_on() == Some(true) {
        return false;
    }
    let names = pty.foreground_programs();
    if names
        .iter()
        .any(|name| zerocode_core::agent::agent_spec_by_process(name).is_some())
    {
        return false;
    }
    !(agent_pane && names.is_empty())
}

/// When the pane's output last moved, in epoch milliseconds — the key a
/// question is listed under, so the same question read again is the same
/// question. `0` when the transport does not say.
pub(super) fn quiet_since(held: &HeldTerminal) -> i64 {
    lock_pty(held).last_output_epoch_ms().unwrap_or(0)
}

/// When the pane's output last moved, as the watcher reads it, without waiting
/// for the pane's lock: `None` while the pane is being parsed this instant (the
/// next look reads it), and `Some(None)` for a transport that does not say.
pub(super) fn output_clock(held: &HeldTerminal) -> Option<Option<i64>> {
    let pty = match held.try_lock() {
        Ok(pty) => pty,
        Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => return None,
    };
    Some(pty.last_output_epoch_ms())
}

/// Type a secret the person gave a card, and the return that sends it, into the
/// pane — if and only if the route is still open ([`secret_route_open`]) and the
/// pane still shows `expect`'s question. The route and the screen are read, and
/// the value written, in one hold of the pane's lock, as [`type_if_up`] does.
/// `Ok(false)` is a refusal: nothing was typed.
///
/// The value and its return go out as one write, in a buffer that is wiped when
/// this returns, so the return is never a separate key a changed screen could
/// take.
pub(super) fn type_secret_if_up(
    held: &HeldTerminal,
    expect: &SecretPrompt,
    value: &[u8],
    agent_pane: bool,
) -> Result<bool, PtyTransportError> {
    let mut pty = lock_pty(held);
    if !secret_route_open(&pty, agent_pane) {
        return Ok(false);
    }
    let (rows, cursor) = screen_of(&pty);
    let shown = cursor.and_then(|row| zerocode_core::secret_prompt::read_secret_prompt(&rows, row));
    if shown.as_ref() != Some(expect) {
        return Ok(false);
    }
    let mut typed = zeroize::Zeroizing::new(Vec::with_capacity(value.len() + 1));
    typed.extend_from_slice(value);
    typed.push(b'\r');
    pty.write_input(&typed)?;
    Ok(true)
}

/// The road a secret card's answer takes into its pane, from the value the
/// person typed to the write. The value is checked before anything else, moved
/// into a buffer that is wiped when this returns, and typed through the door
/// into the pane the card was raised for. A refusal says its word and types
/// nothing. `wake` tells the window's pump that the child was written to.
pub(super) fn answer_secret_into(
    held: Option<HeldTerminal>,
    term: TermId,
    expect: SecretPrompt,
    value: String,
    agent_pane: bool,
    wake: &dyn Fn(),
) -> Result<(), String> {
    if !zerocode_core::secret_prompt::value_is_typable(value.as_bytes()) {
        return Err(SECRET_INVALID.to_string());
    }
    let bytes = zeroize::Zeroizing::new(value.into_bytes());
    let Some(held) = held else {
        return Err("터미널이 떠 있지 않습니다".to_string());
    };
    let lease = answer_lease(term).map_err(Refusal::into_message)?;
    let typed =
        type_secret_if_up(&held, &expect, &bytes, agent_pane).map_err(|error| error.to_string())?;
    wake();
    settle_in_background(lease);
    if typed {
        Ok(())
    } else {
        Err(QUESTION_CHANGED.to_string())
    }
}

/// Take the right to type one answer into `term`, or say why not.
pub(super) fn answer_lease(term: TermId) -> Result<tokio::sync::OwnedMutexGuard<()>, Refusal> {
    crate::ssh_send_guard::send_lease(term)
        .try_lock_owned()
        .map_err(|_| Refusal::InFlight)
}

/// Keep the pane "being answered" for [`ANSWER_SETTLE`] after its last key,
/// then let go. Blocks; a caller already on a thread of its own calls this.
pub(super) fn settle(lease: tokio::sync::OwnedMutexGuard<()>) {
    std::thread::sleep(ANSWER_SETTLE);
    drop(lease);
}

/// [`settle`] for a caller that has nothing more to do and must not wait.
pub(super) fn settle_in_background(lease: tokio::sync::OwnedMutexGuard<()>) {
    std::thread::spawn(move || settle(lease));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    use zerocode_core::ask::AskOption;
    use zerocode_core::secret_prompt::SecretKind;
    use zerocode_pty::{Pumped, Terminal};

    /// What a fake pane's terminal says about who holds it, and whether it echoes.
    #[derive(Clone)]
    struct Holder {
        child: Option<u32>,
        holder: Option<u32>,
        names: Vec<String>,
        echo: Option<bool>,
    }

    impl Holder {
        /// A job the pane's shell started holds the terminal, and it does not echo.
        fn job() -> Self {
            Self {
                child: Some(100),
                holder: Some(200),
                names: vec!["ssh".to_string()],
                echo: Some(false),
            }
        }
    }

    /// A synthetic program that asks for a secret the way sudo does: it draws
    /// its question and waits, and it says when its output last moved.
    struct SecretCli {
        terminal: Terminal,
        last_output: Option<Instant>,
        written: Arc<Mutex<Vec<Vec<u8>>>>,
        holder: Holder,
    }

    impl PtyTransport for SecretCli {
        fn pump(&mut self) -> Pumped {
            Pumped {
                bytes: 0,
                ended: false,
                answered: false,
                unanswered_since: None,
            }
        }

        fn terminal(&self) -> &Terminal {
            &self.terminal
        }

        fn terminal_mut(&mut self) -> &mut Terminal {
            &mut self.terminal
        }

        fn write_input(&mut self, bytes: &[u8]) -> Result<(), PtyTransportError> {
            self.written.lock().unwrap().push(bytes.to_vec());
            Ok(())
        }

        fn resize(&mut self, _rows: u16, _cols: u16) -> Result<(), PtyTransportError> {
            Ok(())
        }

        fn try_wait(&mut self) -> Result<Option<u32>, PtyTransportError> {
            Ok(None)
        }

        fn kill(&mut self) -> Result<(), PtyTransportError> {
            Ok(())
        }

        fn last_output_at(&self) -> Option<Instant> {
            self.last_output
        }

        fn pid(&self) -> Option<u32> {
            self.holder.child
        }

        fn foreground_process_id(&self) -> Option<u32> {
            self.holder.holder
        }

        fn foreground_programs(&self) -> Vec<String> {
            self.holder.names.clone()
        }

        fn echo_on(&self) -> Option<bool> {
            self.holder.echo
        }
    }

    struct SecretPane {
        held: HeldTerminal,
        written: Arc<Mutex<Vec<Vec<u8>>>>,
    }

    impl SecretPane {
        fn typed(&self) -> Vec<String> {
            self.written
                .lock()
                .unwrap()
                .iter()
                .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
                .collect()
        }
    }

    /// A pane whose screen shows `question` on its first row, with the cursor
    /// at its end, and whose output last moved `silent_for` ago. A job the
    /// shell started holds its terminal ([`Holder::job`]).
    fn secret_pane(question: &str, silent_for: Duration) -> SecretPane {
        secret_pane_with(question, silent_for, Holder::job())
    }

    fn secret_pane_with(question: &str, silent_for: Duration, holder: Holder) -> SecretPane {
        let written = Arc::new(Mutex::new(Vec::new()));
        let mut terminal = Terminal::new(24, 80);
        terminal.feed(format!("\x1b[2J\x1b[H{question}").as_bytes());
        let cli = SecretCli {
            terminal,
            last_output: Instant::now().checked_sub(silent_for),
            written: Arc::clone(&written),
            holder,
        };
        SecretPane {
            held: Arc::new(Mutex::new(PtyHandle::new(cli))),
            written,
        }
    }

    #[test]
    fn a_question_for_a_secret_is_read_only_once_its_pane_has_been_quiet() {
        let talking = secret_pane("[sudo] password for dev:", Duration::ZERO);
        assert_eq!(
            secret_prompt(&talking.held, false),
            None,
            "a line still being printed is not yet a question"
        );
        let waiting = secret_pane("[sudo] password for dev:", SECRET_QUIET * 2);
        assert_eq!(
            secret_prompt(&waiting.held, false),
            Some(SecretPrompt {
                kind: SecretKind::Password,
                line: "[sudo] password for dev:".to_string(),
            }),
        );
    }

    #[test]
    fn a_secret_is_typed_with_its_return_once_when_the_same_question_still_shows() {
        let pane = secret_pane("Password:", SECRET_QUIET * 2);
        let expect = SecretPrompt {
            kind: SecretKind::Password,
            line: "Password:".to_string(),
        };
        assert!(type_secret_if_up(&pane.held, &expect, b"SENTINEL-not-a-secret", false).unwrap());
        assert_eq!(pane.typed(), vec!["SENTINEL-not-a-secret\r".to_string()]);
    }

    #[test]
    #[ignore = "measurement: cargo test -p zerocode-shell --bin zerocode-shell answer_door::tests::measure_the_secret_door -- --ignored --nocapture"]
    fn measure_the_secret_door() {
        const ITERATIONS: u32 = 2_000;
        let pane = secret_pane("[sudo] password for dev:", SECRET_QUIET * 2);
        let started = Instant::now();
        let mut found = 0_u32;
        for _ in 0..ITERATIONS {
            found += u32::from(secret_prompt(&pane.held, false).is_some());
        }
        let per_look_us = started.elapsed().as_secs_f64() * 1e6 / f64::from(ITERATIONS);
        eprintln!(
            "MEASURE secret door: {per_look_us:.2} us per look at a quiet pane ({found} of {ITERATIONS} looks read the question)"
        );
    }

    #[test]
    fn a_secret_is_not_typed_into_a_pane_that_asks_something_else() {
        let pane = secret_pane("Enter passphrase:", SECRET_QUIET * 2);
        let expect = SecretPrompt {
            kind: SecretKind::Password,
            line: "Password:".to_string(),
        };
        assert!(matches!(
            type_secret_if_up(&pane.held, &expect, b"SENTINEL-not-a-secret", false),
            Ok(false)
        ));
        assert!(pane.typed().is_empty());
    }

    #[test]
    fn a_secret_is_not_offered_while_the_pane_itself_holds_its_terminal() {
        let mut holder = Holder::job();
        holder.holder = holder.child;
        let pane = secret_pane_with("Password:", SECRET_QUIET * 2, holder);
        assert_eq!(
            secret_prompt(&pane.held, false),
            None,
            "a shell at its own prompt, or an agent with no job in front, asked for nothing a value answers"
        );
    }

    #[test]
    fn a_secret_is_not_offered_while_an_agent_is_the_job_in_front() {
        let mut holder = Holder::job();
        holder.names = vec!["claude".to_string(), "bin".to_string()];
        let pane = secret_pane_with("Password:", SECRET_QUIET * 2, holder);
        assert_eq!(
            secret_prompt(&pane.held, false),
            None,
            "a tool's prompt on an agent's terminal is read by the agent, not by the tool"
        );
    }

    #[test]
    fn a_secret_is_not_typed_into_a_question_that_echoes() {
        let mut holder = Holder::job();
        holder.echo = Some(true);
        let pane = secret_pane_with("Password:", SECRET_QUIET * 2, holder);
        let expect = SecretPrompt {
            kind: SecretKind::Password,
            line: "Password:".to_string(),
        };
        assert!(matches!(
            type_secret_if_up(&pane.held, &expect, b"SENTINEL-not-a-secret", false),
            Ok(false)
        ));
        assert!(
            pane.typed().is_empty(),
            "an echoing terminal keeps the typed value on the screen and in scrollback"
        );
    }

    #[test]
    fn a_secret_is_not_typed_when_the_terminal_cannot_say_who_holds_it() {
        let mut holder = Holder::job();
        holder.holder = None;
        let pane = secret_pane_with("Password:", SECRET_QUIET * 2, holder);
        let expect = SecretPrompt {
            kind: SecretKind::Password,
            line: "Password:".to_string(),
        };
        assert!(matches!(
            type_secret_if_up(&pane.held, &expect, b"SENTINEL-not-a-secret", false),
            Ok(false)
        ));
        assert!(pane.typed().is_empty());
    }

    #[test]
    fn an_agent_pane_must_name_the_program_in_front_of_its_terminal() {
        let mut unnamed = Holder::job();
        unnamed.names = Vec::new();
        let silent = secret_pane_with("Password:", SECRET_QUIET * 2, unnamed);
        assert_eq!(
            secret_prompt(&silent.held, true),
            None,
            "an agent pane whose job has no name cannot be told apart from its agent"
        );
        let named = secret_pane("Password:", SECRET_QUIET * 2);
        assert!(
            secret_prompt(&named.held, true).is_some(),
            "an agent pane whose job is named is asked the question"
        );
    }

    #[test]
    fn a_secret_is_typed_into_a_job_in_front_of_an_agent_pane() {
        let pane = secret_pane("Password:", SECRET_QUIET * 2);
        let expect = SecretPrompt {
            kind: SecretKind::Password,
            line: "Password:".to_string(),
        };
        assert!(type_secret_if_up(&pane.held, &expect, b"SENTINEL-not-a-secret", true).unwrap());
        assert_eq!(pane.typed(), vec!["SENTINEL-not-a-secret\r".to_string()]);
    }

    #[test]
    fn a_terminal_that_cannot_say_whether_it_echoes_still_takes_the_secret() {
        let mut holder = Holder::job();
        holder.echo = None;
        let pane = secret_pane_with("Password:", SECRET_QUIET * 2, holder);
        let expect = SecretPrompt {
            kind: SecretKind::Password,
            line: "Password:".to_string(),
        };
        assert!(type_secret_if_up(&pane.held, &expect, b"SENTINEL-not-a-secret", false).unwrap());
        assert_eq!(pane.typed(), vec!["SENTINEL-not-a-secret\r".to_string()]);
    }

    /// A value made up for these tests: 32 characters, the shape a password takes,
    /// and no account's.
    const EVIDENCE_VALUE: &str = "Qv4Lk9Zt2Rw7Xb3Ns8Hy1Mc6Jd0Fg5Ua";

    /// The answer road, run in a process of its own so that everything the process
    /// prints, and every file under its home folder, can be searched. The test
    /// below runs it with the home folder pointed at a temporary one. It prints
    /// counts and refusal words, never the value.
    #[test]
    #[ignore = "run by secret_answer_keeps_no_copy_in_its_output_or_its_files"]
    fn secret_answer_road_as_a_child_process() {
        let question = || SecretPrompt {
            kind: SecretKind::Password,
            line: "Password:".to_string(),
        };
        let typed = secret_pane("Password:", SECRET_QUIET * 2);
        let answered = answer_secret_into(
            Some(Arc::clone(&typed.held)),
            1,
            question(),
            EVIDENCE_VALUE.to_string(),
            false,
            &|| {},
        );

        let busy = secret_pane("Password:", SECRET_QUIET * 2);
        let _lease = answer_lease(2).expect("nothing else answers pane 2 yet");
        let refused_busy = answer_secret_into(
            Some(Arc::clone(&busy.held)),
            2,
            question(),
            EVIDENCE_VALUE.to_string(),
            false,
            &|| {},
        );

        let changed = secret_pane("Enter passphrase:", SECRET_QUIET * 2);
        let refused_changed = answer_secret_into(
            Some(Arc::clone(&changed.held)),
            3,
            question(),
            EVIDENCE_VALUE.to_string(),
            false,
            &|| {},
        );

        let invalid = secret_pane("Password:", SECRET_QUIET * 2);
        let refused_invalid = answer_secret_into(
            Some(Arc::clone(&invalid.held)),
            4,
            question(),
            format!("{EVIDENCE_VALUE}\n"),
            false,
            &|| {},
        );

        let writes = |pane: &SecretPane| pane.written.lock().unwrap().clone();
        let refusals = [refused_busy, refused_changed, refused_invalid]
            .into_iter()
            .map(Result::err)
            .collect::<Vec<_>>();
        let hits_in_writes: usize = writes(&typed)
            .iter()
            .map(|bytes| count_occurrences(bytes, EVIDENCE_VALUE.as_bytes()))
            .sum();
        let refused_writes = writes(&busy).len() + writes(&changed).len() + writes(&invalid).len();
        println!(
            "SECRET_EVIDENCE answered={} writes={} hits_in_writes={} refused_writes={} refused={refusals:?}",
            answered.is_ok(),
            writes(&typed).len(),
            hits_in_writes,
            refused_writes,
        );
    }

    /// The answer road keeps no copy of the value where a person could find one:
    /// not in what its process prints, not in any file under its home folder
    /// (write-ahead logs and journals included). The value reaches its pane once,
    /// and no refused answer reaches a pane at all.
    #[test]
    fn secret_answer_keeps_no_copy_in_its_output_or_its_files() {
        let home = tempfile::tempdir().expect("a temporary home folder");
        let child = crate::proc::quiet_command(std::env::current_exe().expect("this test binary"))
            .args([
                "--exact",
                "answer_door::tests::secret_answer_road_as_a_child_process",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("HOME", home.path())
            .env("XDG_CONFIG_HOME", home.path().join(".config"))
            .env("XDG_DATA_HOME", home.path().join(".local/share"))
            .env("XDG_CACHE_HOME", home.path().join(".cache"))
            .env("XDG_STATE_HOME", home.path().join(".local/state"))
            .env("ZO_HOME", home.path().join(".zo"))
            .env("ZO_CONFIG_HOME", home.path().join(".zo"))
            .env("ZO_DISABLE_KEYCHAIN", "1")
            .output()
            .expect("the answer road ran in a process of its own");
        assert!(
            child.status.success(),
            "the answer road failed in its own process"
        );
        let printed = [child.stdout.as_slice(), child.stderr.as_slice()].concat();
        assert_eq!(
            count_occurrences(&printed, EVIDENCE_VALUE.as_bytes()),
            0,
            "the value reached what the process printed"
        );
        assert_eq!(
            count_files_holding(home.path(), EVIDENCE_VALUE.as_bytes()),
            0,
            "the value reached a file under the temporary home folder"
        );
        let said = String::from_utf8_lossy(&child.stdout);
        assert!(
            said.contains(
                "SECRET_EVIDENCE answered=true writes=1 hits_in_writes=1 refused_writes=0 \
                 refused=[Some(\"answer-in-flight\"), Some(\"question-changed\"), Some(\"secret-invalid\")]"
            ),
            "the road did not answer once, refuse three times and write nothing refused: {said}"
        );
    }

    /// The search above would also pass a road that writes nothing it can find, so
    /// the search is checked first on copies that were planted by hand.
    #[test]
    fn the_search_for_the_value_finds_a_planted_copy() {
        let folder = tempfile::tempdir().expect("a folder");
        std::fs::write(
            folder.path().join("journal-wal"),
            format!("x{EVIDENCE_VALUE}y"),
        )
        .expect("a file can be written");
        assert_eq!(
            count_files_holding(folder.path(), EVIDENCE_VALUE.as_bytes()),
            1,
            "a copy planted in a file is found"
        );
        assert_eq!(
            count_occurrences(
                format!("out {EVIDENCE_VALUE}").as_bytes(),
                EVIDENCE_VALUE.as_bytes()
            ),
            1,
            "a copy planted in output is found"
        );
    }

    /// How many times `needle` occurs in `haystack`, byte for byte.
    fn count_occurrences(haystack: &[u8], needle: &[u8]) -> usize {
        haystack
            .windows(needle.len())
            .filter(|window| *window == needle)
            .count()
    }

    /// How many regular files under `root`, at any depth and whatever their name,
    /// hold `needle`.
    fn count_files_holding(root: &std::path::Path, needle: &[u8]) -> usize {
        let mut found = 0;
        let mut pending = vec![root.to_path_buf()];
        while let Some(path) = pending.pop() {
            if path.is_dir() {
                for entry in std::fs::read_dir(&path).expect("a folder under the home can be read")
                {
                    pending.push(entry.expect("a folder entry can be read").path());
                }
            } else if path.is_file() {
                let bytes = std::fs::read(&path).expect("a file under the home can be read");
                found += usize::from(count_occurrences(&bytes, needle) > 0);
            }
        }
        found
    }

    /// A synthetic CLI that draws a numbered menu and takes arrow keys and
    /// Enter the way a menu does — the pane every test here types into.
    struct MenuCli {
        terminal: Terminal,
        title: String,
        options: Vec<String>,
        selected: usize,
        /// Whether an arrow key moves the selection. A CLI that does not is
        /// the one the walk's look has to catch.
        follows_arrows: bool,
        written: Arc<Mutex<Vec<Vec<u8>>>>,
        chosen: Arc<Mutex<Option<usize>>>,
    }

    impl MenuCli {
        fn redraw(&mut self) {
            let mut screen = String::from("\x1b[2J\x1b[H");
            screen.push_str(&self.title);
            screen.push_str("\r\n");
            for (i, option) in self.options.iter().enumerate() {
                let glyph = if i == self.selected { "❯" } else { " " };
                screen.push_str(&format!("{glyph} {}. {option}\r\n", i + 1));
            }
            self.terminal.feed(screen.as_bytes());
        }
    }

    impl PtyTransport for MenuCli {
        fn pump(&mut self) -> Pumped {
            Pumped {
                bytes: 0,
                ended: false,
                answered: false,
                unanswered_since: None,
            }
        }

        fn terminal(&self) -> &Terminal {
            &self.terminal
        }

        fn terminal_mut(&mut self) -> &mut Terminal {
            &mut self.terminal
        }

        fn write_input(&mut self, bytes: &[u8]) -> Result<(), PtyTransportError> {
            self.written.lock().unwrap().push(bytes.to_vec());
            match bytes {
                b"\x1b[B" if self.follows_arrows => {
                    self.selected = (self.selected + 1).min(self.options.len() - 1);
                    self.redraw();
                }
                b"\x1b[A" if self.follows_arrows => {
                    self.selected = self.selected.saturating_sub(1);
                    self.redraw();
                }
                b"\r" => {
                    *self.chosen.lock().unwrap() = Some(self.selected);
                    self.terminal.feed(b"\x1b[2J\x1b[Hchosen\r\n> ");
                }
                _ => {}
            }
            Ok(())
        }

        fn resize(&mut self, _rows: u16, _cols: u16) -> Result<(), PtyTransportError> {
            Ok(())
        }

        fn try_wait(&mut self) -> Result<Option<u32>, PtyTransportError> {
            Ok(None)
        }

        fn kill(&mut self) -> Result<(), PtyTransportError> {
            Ok(())
        }
    }

    struct Pane {
        held: HeldTerminal,
        written: Arc<Mutex<Vec<Vec<u8>>>>,
        chosen: Arc<Mutex<Option<usize>>>,
    }

    impl Pane {
        fn typed(&self) -> Vec<String> {
            self.written
                .lock()
                .unwrap()
                .iter()
                .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
                .collect()
        }

        /// What the child would draw next, as the child's own bytes.
        fn draw(&self, text: &str) {
            draw_on(&self.held, text);
        }
    }

    /// [`Pane::draw`] for a screen another thread holds.
    fn draw_on(held: &HeldTerminal, text: &str) {
        let mut pty = lock_pty(held);
        pty.terminal_mut().feed(b"\x1b[2J\x1b[H");
        pty.terminal_mut()
            .feed(text.replace('\n', "\r\n").as_bytes());
    }

    fn pane(title: &str, options: &[&str], follows_arrows: bool) -> Pane {
        let written = Arc::new(Mutex::new(Vec::new()));
        let chosen = Arc::new(Mutex::new(None));
        let mut cli = MenuCli {
            terminal: Terminal::new(24, 80),
            title: title.to_string(),
            options: options.iter().map(|option| (*option).to_string()).collect(),
            selected: 0,
            follows_arrows,
            written: Arc::clone(&written),
            chosen: Arc::clone(&chosen),
        };
        cli.redraw();
        Pane {
            held: Arc::new(Mutex::new(PtyHandle::new(cli))),
            written,
            chosen,
        }
    }

    fn asked(text: &str, options: &[&str]) -> AskQuestion {
        AskQuestion {
            question: text.to_string(),
            header: None,
            multi_select: false,
            options: options
                .iter()
                .map(|label| AskOption {
                    label: (*label).to_string(),
                    description: None,
                })
                .collect(),
        }
    }

    const NO_WAIT: Duration = Duration::ZERO;

    /// The incident: an answer prepared for one question, typed after the
    /// pane has moved on to another, is typed zero times — to the next
    /// question, to a composer, to nothing at all.
    #[test]
    fn a_late_answer_is_typed_zero_times_after_the_question_changed() {
        let pane = pane("Which library?", &["date-fns", "dayjs"], true);
        let question = asked("Which library?", &["date-fns", "dayjs"]);
        let expect = Expect::question(&question);
        assert_eq!(type_if_up(&pane.held, &expect, b"1").ok(), Some(true));
        assert_eq!(
            pane.typed(),
            vec!["1"],
            "the live question takes its answer"
        );

        // The agent moved on to another question.
        pane.draw("Which framework?\n❯ 1. react\n  2. vue\n");
        assert_eq!(type_if_up(&pane.held, &expect, b"1").ok(), Some(false));
        // The question was answered and the pane is at its composer.
        pane.draw("● Done.\n\n❯ \n");
        assert_eq!(type_if_up(&pane.held, &expect, b"1").ok(), Some(false));
        assert_eq!(
            pane.typed(),
            vec!["1"],
            "nothing was typed after the change"
        );
    }

    /// A permission card has no rows of its own: any live menu will do, and a
    /// screen with none — the agent resolved it itself — gets no key, least of
    /// all the Escape that would stop a running turn.
    #[test]
    fn a_permission_answer_needs_a_live_menu_and_never_types_an_escape_without_one() {
        let pane = pane("Do you want to proceed?", &["Yes", "No"], true);
        assert_eq!(
            type_if_up(&pane.held, &Expect::any_menu(), b"1").ok(),
            Some(true)
        );
        pane.draw("✻ Working… (3s)\n\n❯ \n");
        assert_eq!(
            type_if_up(&pane.held, &Expect::any_menu(), b"\x1b").ok(),
            Some(false)
        );
        assert_eq!(pane.typed(), vec!["1"]);
    }

    /// The selection has to be where the answer needs it when the answer
    /// is a key relative to it.
    #[test]
    fn an_answer_that_needs_the_selection_on_a_row_waits_for_it() {
        let pane = pane("Pick", &["a", "b", "c"], true);
        let question = asked("Pick", &["a", "b", "c"]);
        let on_second = Expect::question(&question).on_row(1);
        assert_eq!(type_if_up(&pane.held, &on_second, b"\r").ok(), Some(false));
        assert!(pane.typed().is_empty());
        let on_first = Expect::question(&question).on_row(0);
        assert_eq!(type_if_up(&pane.held, &on_first, b"\r").ok(), Some(true));
    }

    /// The pane's own redraw of the same question — a clock above it, the
    /// selection moved — is not a changed question.
    #[test]
    fn a_redraw_of_the_same_question_is_not_a_change() {
        let pane = pane("Which library?", &["date-fns", "dayjs"], true);
        let question = asked("Which library?", &["date-fns", "dayjs"]);
        let expect = Expect::question(&question);
        for tick in 0..20 {
            pane.draw(&format!(
                "✻ Worked for {tick}m {s}s · {tick}.2k tokens\n\nWhich library?\n  1. date-fns\n❯ 2. dayjs\n\nEnter to select\n",
                s = 7 + tick
            ));
            assert_eq!(
                type_if_up(&pane.held, &expect, b"2").ok(),
                Some(true),
                "refused a good answer on redraw {tick}"
            );
        }
    }

    /// Guidance the window types by itself is not typed into a pane standing on
    /// a menu: Enter there takes the highlighted row.
    #[test]
    fn a_line_is_not_typed_into_a_pane_standing_on_a_menu() {
        let pane = pane("Do you want to proceed?", &["Yes", "No"], true);
        assert_eq!(
            type_if_up(&pane.held, &Expect::no_menu(), b"/exit").ok(),
            Some(false)
        );
        assert!(pane.typed().is_empty(), "a line was typed into a menu");
        pane.draw("● Done.\n\n❯ \n");
        assert_eq!(
            type_if_up(&pane.held, &Expect::no_menu(), b"/exit").ok(),
            Some(true)
        );
        assert_eq!(pane.typed(), vec!["/exit"]);
    }

    /// A CLI's way back the window types by itself (draft C10, t-82196) goes
    /// into a pane only when no run sits in it — nor may, when the ledger
    /// cannot be read — its agent is between turns, and its screen shows its
    /// input line empty with no menu: never onto a permission menu, where its
    /// Enter would take the highlighted row, nor after words on the line.
    #[test]
    fn a_way_back_is_typed_only_into_a_pane_that_takes_it() {
        let rule = "─".repeat(40);
        let pane = pane("Do you want to proceed?", &["Yes", "No"], true);
        let free = RewindPane {
            seat: None,
            between_turns: true,
            agent: "claude",
        };
        assert_eq!(
            type_rewind(&pane.held, &free, b"/rewind"),
            Err(REWIND_SCREEN.to_string())
        );
        assert!(pane.typed().is_empty(), "a way back was typed into a menu");
        pane.draw(&format!(
            "\x1b[?2004h● Done.\n\n{rule}\n❯ fix login\n{rule}\x1b[4;12H"
        ));
        assert_eq!(
            type_rewind(&pane.held, &free, b"/rewind"),
            Err(REWIND_SCREEN.to_string())
        );
        assert!(
            pane.typed().is_empty(),
            "a way back joined words on the line"
        );
        pane.draw(&format!(
            "\x1b[?2004h● Done.\n\n{rule}\n❯ \n{rule}\x1b[4;3H"
        ));
        use crate::cmd::wire::pane_the_ledger_drives as drives;
        let unread = rewind_seat_word(drives(7, || Some(Vec::new()), || None));
        let unread_workers = rewind_seat_word(drives(7, || None, || Some(Vec::new())));
        let coordinator = rewind_seat_word(drives(
            7,
            || Some(Vec::new()),
            || {
                Some(vec![crate::orchestration::restart_census::KeyedPane {
                    key: "run-1".to_string(),
                    term: 7,
                }])
            },
        ));
        let worker = rewind_seat_word(drives(7, || Some(vec![7]), || Some(Vec::new())));
        for (refused, word) in [
            (
                RewindPane {
                    seat: unread,
                    ..free
                },
                REWIND_LEDGER_UNREAD,
            ),
            (
                RewindPane {
                    seat: unread_workers,
                    ..free
                },
                REWIND_LEDGER_UNREAD,
            ),
            (
                RewindPane {
                    seat: coordinator,
                    ..free
                },
                REWIND_SEAT,
            ),
            (
                RewindPane {
                    seat: worker,
                    ..free
                },
                REWIND_SEAT,
            ),
            (
                RewindPane {
                    between_turns: false,
                    ..free
                },
                REWIND_BUSY,
            ),
        ] {
            assert_eq!(
                type_rewind(&pane.held, &refused, b"/rewind"),
                Err(word.to_string())
            );
        }
        assert!(pane.typed().is_empty(), "a refused way back was typed");
        assert_eq!(type_rewind(&pane.held, &free, b"/rewind"), Ok(()));
        assert_eq!(pane.typed(), vec!["/rewind"]);
    }

    /// What the queue's 끼워 넣기 and 바로 보내기 type at a pane whose turn runs
    /// (t-86909) goes in only when no run sits in the pane — nor may, when the
    /// ledger cannot be read — the agent's turn runs, and the screen shows its
    /// input line empty with no menu. Never onto a permission menu, where the
    /// stop would refuse the permission and the steer's Enter would take the
    /// highlighted row; never after words a person left on the line; never at
    /// a pane that is at rest. Every refusal types nothing and says its word.
    #[test]
    fn words_and_the_stop_are_typed_only_at_a_pane_whose_turn_runs() {
        let rule = "─".repeat(40);
        let pane = pane("Do you want to proceed?", &["Yes", "No"], true);
        let running = TurnPane {
            seat: None,
            working: true,
            agent: "claude",
            reads_input: true,
        };
        assert_eq!(
            type_at_a_turn(&pane.held, &running, b"\x1b"),
            Err(refused::SCREEN.to_string())
        );
        assert!(pane.typed().is_empty(), "a stop was typed into a menu");
        pane.draw(&format!(
            "\x1b[?2004h✻ Working…\n\n{rule}\n❯ fix login\n{rule}\x1b[4;12H"
        ));
        assert_eq!(
            type_at_a_turn(&pane.held, &running, b"words"),
            Err(refused::SCREEN.to_string())
        );
        assert!(pane.typed().is_empty(), "words joined a person's words");
        pane.draw(&format!(
            "\x1b[?2004h✻ Working…\n\n{rule}\n❯ \n{rule}\x1b[4;3H"
        ));
        use crate::cmd::wire::pane_the_ledger_drives as drives;
        let unread = turn_seat_word(drives(7, || None, || Some(Vec::new())));
        let unread_seats = turn_seat_word(drives(7, || Some(Vec::new()), || None));
        let worker = turn_seat_word(drives(7, || Some(vec![7]), || Some(Vec::new())));
        let coordinator = turn_seat_word(drives(
            7,
            || Some(Vec::new()),
            || {
                Some(vec![crate::orchestration::restart_census::KeyedPane {
                    key: "run-1".to_string(),
                    term: 7,
                }])
            },
        ));
        assert_eq!(
            turn_seat_word(drives(7, || Some(Vec::new()), || Some(Vec::new()))),
            None
        );
        for (refused, word) in [
            (
                TurnPane {
                    seat: unread,
                    ..running
                },
                refused::LEDGER_UNREAD,
            ),
            (
                TurnPane {
                    seat: unread_seats,
                    ..running
                },
                refused::LEDGER_UNREAD,
            ),
            (
                TurnPane {
                    seat: worker,
                    ..running
                },
                refused::SEAT,
            ),
            (
                TurnPane {
                    seat: coordinator,
                    ..running
                },
                refused::SEAT,
            ),
            (
                TurnPane {
                    working: false,
                    ..running
                },
                refused::IDLE,
            ),
        ] {
            assert_eq!(
                type_at_a_turn(&pane.held, &refused, b"words"),
                Err(word.to_string())
            );
        }
        assert!(pane.typed().is_empty(), "a refused steer was typed");
        // An agent whose input line the window cannot read as empty is never
        // typed at, whatever the screen shows.
        let unread_agent = TurnPane {
            agent: "codex",
            ..running
        };
        assert_eq!(
            type_at_a_turn(&pane.held, &unread_agent, b"words"),
            Err(refused::SCREEN.to_string())
        );
        assert!(pane.typed().is_empty(), "an unreadable line was typed at");
        assert_eq!(type_at_a_turn(&pane.held, &running, b"words"), Ok(()));
        assert_eq!(pane.typed(), vec!["words"]);
    }

    /// The Enter that sends the steered words goes through the door as well
    /// (t-86909): not onto a menu that came up in the breath between the
    /// words and it, and not after a hand that reached the line since — a
    /// person's own Enter or words — whose pane the window no longer reads.
    /// Either way the words are already on the line, which the page is told
    /// with [`refused::LANDED`] so it does not queue them a second time.
    #[test]
    fn the_enter_after_steered_words_is_typed_only_onto_a_pane_without_a_menu_or_a_new_hand() {
        let pane = pane("Do you want to proceed?", &["Yes", "No"], true);
        let term = 6_100_001;
        let hand = crate::human_input::line_of(term).1;
        assert_eq!(
            type_enter_after_words(&pane.held, term, hand, b"\r"),
            Err(refused::LANDED.to_string()),
            "an Enter was typed onto a menu"
        );
        assert!(pane.typed().is_empty());
        pane.draw("● Working…\n\n❯ words\n");
        crate::human_input::keyed(term);
        assert_eq!(
            type_enter_after_words(&pane.held, term, hand, b"\r"),
            Err(refused::LANDED.to_string()),
            "an Enter followed a person's hand"
        );
        assert!(pane.typed().is_empty());
        let after = crate::human_input::line_of(term).1;
        assert_eq!(
            type_enter_after_words(&pane.held, term, after, b"\r"),
            Ok(())
        );
        assert_eq!(pane.typed(), vec!["\r"]);
        crate::human_input::forget_term(term);
    }

    /// Claude Code's live field, drawn as the existing tests draw it: bracketed paste on, the
    /// cursor shown, `line` after the `❯ ` and the cursor `cursor_col` columns in.
    fn field(pane: &Pane, line: &str, cursor_col: usize) {
        let rule = "─".repeat(40);
        pane.draw(&format!(
            "\x1b[?2004h\x1b[?25h✻ Working…\n\n{rule}\n❯ {line}\n{rule}\x1b[4;{}H",
            cursor_col + 1
        ));
    }

    /// A claude pane whose turn runs, and one whose turn is over (the hook's word).
    fn running() -> TurnPane<'static> {
        TurnPane {
            seat: None,
            working: true,
            agent: "claude",
            reads_input: true,
        }
    }

    fn over() -> TurnPane<'static> {
        TurnPane {
            working: false,
            ..running()
        }
    }

    /// No waiting: a breath of a millisecond between the words and the Enter, and a screen
    /// that must show the field at once.
    const QUICK: Timing = Timing {
        step: Duration::from_millis(1),
        wait: Duration::ZERO,
    };

    /// A pane whose CLI has not shown it reads its input — restored a moment ago, a hook word
    /// that outlived the restart — is typed at by no queue door (t-86909 r2, P3-3): the hand-over
    /// asks the same of a pane before it types its exit command, and the queue's three doors
    /// type just as much.
    #[test]
    fn a_pane_whose_cli_has_not_shown_it_reads_its_input_is_typed_at_by_no_queue_door() {
        let term = 6_200_001;
        let pane = pane("Do you want to proceed?", &["Yes", "No"], true);
        field(&pane, "", 2);
        let blind = TurnPane {
            reads_input: false,
            ..running()
        };
        let blind_over = TurnPane {
            reads_input: false,
            ..over()
        };
        let not_reading = Err(refused::NOT_READING.to_string());
        assert_eq!(type_at_a_turn(&pane.held, &blind, b"\x1b"), not_reading);
        for (what, result) in [
            (
                "a steer",
                type_queued_line(&pane.held, term, &blind, Moment::During, "words", QUICK),
            ),
            (
                "a send now's words",
                type_queued_line(&pane.held, term, &blind_over, Moment::After, "words", QUICK),
            ),
        ] {
            assert_eq!(result, not_reading, "{what}");
        }
        assert!(
            pane.typed().is_empty(),
            "a pane that was not reading was typed at"
        );
        // The ledger's seat is asked before the pane is: a worker's pane says so.
        let seat = TurnPane {
            seat: Some(refused::SEAT),
            reads_input: false,
            ..running()
        };
        assert_eq!(
            type_at_a_turn(&pane.held, &seat, b"\x1b"),
            Err(refused::SEAT.to_string())
        );
    }

    /// A line the queue types is one bracketed paste whatever it holds (t-86909 r2, P2-3): a
    /// single line used to go in as bare keys — a Tab as the CLI's Tab, a `^C` as its Ctrl+C —
    /// where a longer text went in as a paste. Then its Enter, as a key of its own.
    #[test]
    fn a_queued_line_is_one_bracketed_paste_and_then_its_enter_whatever_it_holds() {
        let term = 6_200_002;
        for (text, pasted) in [
            ("이름\t값", "\x1b[200~이름\t값\x1b[201~"),
            ("끝에 \x03 이 든 글", "\x1b[200~끝에 \x03 이 든 글\x1b[201~"),
            ("\x04와 \x7f", "\x1b[200~\x04와 \x7f\x1b[201~"),
            ("/exit", "\x1b[200~/exit\x1b[201~"),
            ("한 줄", "\x1b[200~한 줄\x1b[201~"),
            ("두 줄\n둘째", "\x1b[200~두 줄\r둘째\x1b[201~"),
            (
                "끝에서 닫는 \x1b[201~ 표",
                "\x1b[200~끝에서 닫는 ␛[201~ 표\x1b[201~",
            ),
        ] {
            let pane = pane("Do you want to proceed?", &["Yes", "No"], true);
            field(&pane, "", 2);
            assert_eq!(
                type_queued_line(&pane.held, term, &running(), Moment::During, text, QUICK),
                Ok(()),
                "{text:?}"
            );
            assert_eq!(pane.typed(), vec![pasted, "\r"], "{text:?}");
        }
    }

    /// The words of a 바로 보내기 are typed after the turn is over, onto an empty line — and
    /// never behind words the stop's Escape took back onto it (t-86909 r2, P2-2). The incident:
    /// words A are steered into the running turn and the CLI has not taken them yet; B is sent
    /// now; the Escape makes the CLI put A back on its line; B pasted after A and an Enter would
    /// send both as one message. The door looks again, at the moment B is due.
    #[test]
    fn the_words_of_a_send_now_never_join_words_the_stop_put_back_on_the_line() {
        let term = 6_200_003;
        let pane = pane("Do you want to proceed?", &["Yes", "No"], true);
        field(&pane, "", 2);
        let steered = "앞서 끼워 넣은 글";
        assert_eq!(
            type_queued_line(&pane.held, term, &running(), Moment::During, steered, QUICK),
            Ok(())
        );
        assert_eq!(
            pane.typed(),
            vec!["\x1b[200~앞서 끼워 넣은 글\x1b[201~", "\r"]
        );
        // The Escape the stop typed: the CLI takes A back onto its line.
        field(&pane, steered, 19);
        assert_eq!(
            type_queued_line(
                &pane.held,
                term,
                &over(),
                Moment::After,
                "바로 보낼 글",
                QUICK
            ),
            Err(refused::LINE.to_string()),
            "the send now's words were typed behind the words on the line"
        );
        assert_eq!(pane.typed().len(), 2, "something was typed behind A");
        // A turn that runs again is no time for them either; a menu is no place.
        field(&pane, "", 2);
        assert_eq!(
            type_queued_line(
                &pane.held,
                term,
                &running(),
                Moment::After,
                "바로 보낼 글",
                QUICK
            ),
            Err(refused::RUNNING.to_string())
        );
        pane.draw("Do you want to proceed?\n❯ 1. Yes\n  2. No\n");
        assert_eq!(
            type_queued_line(
                &pane.held,
                term,
                &over(),
                Moment::After,
                "바로 보낼 글",
                QUICK
            ),
            Err(refused::LINE.to_string())
        );
        // The steer is for a turn that runs, as before.
        field(&pane, "", 2);
        assert_eq!(
            type_queued_line(
                &pane.held,
                term,
                &over(),
                Moment::During,
                "끼워 넣을 글",
                QUICK
            ),
            Err(refused::IDLE.to_string())
        );
        assert_eq!(pane.typed().len(), 2, "a refusal typed something");
        // The person cleared the line: now the words go, as one paste and their Enter.
        assert_eq!(
            type_queued_line(
                &pane.held,
                term,
                &over(),
                Moment::After,
                "바로 보낼 글",
                QUICK
            ),
            Ok(())
        );
        assert_eq!(&pane.typed()[2..], ["\x1b[200~바로 보낼 글\x1b[201~", "\r"]);
        crate::human_input::forget_term(term);
    }

    /// The redraw after a stop may trail the hook's word (t-86909 r2): the door gives the screen
    /// its `wait` to show the empty field, types the moment it does, and refuses — typing nothing
    /// — when it does not within it.
    #[test]
    fn a_screen_that_shows_the_empty_line_a_moment_late_is_waited_for_and_one_that_never_does_is_not_typed_at()
     {
        let term = 6_200_004;
        let late = pane("Do you want to proceed?", &["Yes", "No"], true);
        late.draw("● Interrupted by the person\n");
        let held = Arc::clone(&late.held);
        let redraw = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(80));
            let rule = "─".repeat(40);
            draw_on(
                &held,
                &format!("\x1b[?2004h\x1b[?25h● Interrupted\n\n{rule}\n❯ \n{rule}\x1b[4;3H"),
            );
        });
        let patient = Timing {
            step: Duration::from_millis(1),
            wait: Duration::from_secs(20),
        };
        assert_eq!(
            type_queued_line(
                &late.held,
                term,
                &over(),
                Moment::After,
                "늦게 보이는 줄",
                patient
            ),
            Ok(())
        );
        redraw.join().unwrap();
        assert_eq!(late.typed(), vec!["\x1b[200~늦게 보이는 줄\x1b[201~", "\r"]);
        let stuck = pane("Do you want to proceed?", &["Yes", "No"], true);
        stuck.draw("● Interrupted by the person\n");
        let began = Instant::now();
        let brief = Timing {
            step: Duration::from_millis(1),
            wait: Duration::from_millis(150),
        };
        assert_eq!(
            type_queued_line(
                &stuck.held,
                term,
                &over(),
                Moment::After,
                "안 보이는 줄",
                brief
            ),
            Err(refused::LINE.to_string())
        );
        assert!(
            began.elapsed() >= Duration::from_millis(150),
            "the door gave up before its wait"
        );
        assert!(stuck.typed().is_empty());
        crate::human_input::forget_term(term);
    }

    /// A person's hand between the words and their Enter ends the window's Enter, as a menu
    /// that came up does (t-86909): the words are on the line already, and the answer says so.
    #[test]
    fn a_hand_that_reaches_the_line_between_the_words_and_their_enter_stops_the_enter() {
        let term = 6_200_005;
        let pane = pane("Do you want to proceed?", &["Yes", "No"], true);
        field(&pane, "", 2);
        let toucher = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            crate::human_input::keyed(term);
        });
        let slow = Timing {
            step: Duration::from_millis(300),
            wait: Duration::ZERO,
        };
        assert_eq!(
            type_queued_line(
                &pane.held,
                term,
                &running(),
                Moment::During,
                "손이 닿은 글",
                slow
            ),
            Err(refused::LANDED.to_string())
        );
        toucher.join().unwrap();
        assert_eq!(pane.typed(), vec!["\x1b[200~손이 닿은 글\x1b[201~"]);
        crate::human_input::forget_term(term);
    }

    /// The window's own line after a stop is a prompt the provider will report taking, so the
    /// tracker is told what the old road told it — the words typed, the Enter a gesture — and
    /// the report answers it (t-86909 r2). A steer is no new prompt (the CLI queues it) and
    /// marks nothing.
    #[test]
    fn a_send_nows_line_leaves_the_gesture_the_providers_prompt_report_answers() {
        let term = 6_200_006;
        crate::human_input::forget_term(term);
        let pane = pane("Do you want to proceed?", &["Yes", "No"], true);
        field(&pane, "", 2);
        assert_eq!(
            type_queued_line(
                &pane.held,
                term,
                &running(),
                Moment::During,
                "끼워 넣은 글",
                QUICK
            ),
            Ok(())
        );
        assert!(
            !crate::human_input::holds_a_draft(term),
            "a steer marked the line as a draft"
        );
        field(&pane, "", 2);
        assert_eq!(
            type_queued_line(
                &pane.held,
                term,
                &over(),
                Moment::After,
                "바로 보낸 글",
                QUICK
            ),
            Ok(())
        );
        assert!(
            crate::human_input::holds_a_draft(term),
            "the line the window sent is not a gesture the provider's report waits for"
        );
        crate::human_input::submitted(term);
        assert!(
            !crate::human_input::holds_a_draft(term),
            "the provider's report of the prompt left the line marked"
        );
        crate::human_input::forget_term(term);
    }

    /// The hand-over's exit command stands alone on its line (t-84850 r2):
    /// typed only while the screen shows Claude Code's input line empty —
    /// the cursor right after `❯`, between its two rules — never onto words
    /// a person left there, which it would send to the model with it.
    #[test]
    fn the_exit_line_is_typed_only_onto_an_empty_input_line() {
        let pane = pane("Do you want to proceed?", &["Yes", "No"], true);
        let expect = Expect::no_menu().with_empty_line("claude");
        let rule = "─".repeat(40);
        pane.draw(&format!(
            "\x1b[?2004h● Done.\n\n{rule}\n❯ abc\n{rule}\x1b[4;6H"
        ));
        assert_eq!(type_if_up(&pane.held, &expect, b"/exit").ok(), Some(false));
        assert!(
            pane.typed().is_empty(),
            "the exit line joined words on the line"
        );
        pane.draw(&format!(
            "\x1b[?2004h● Done.\n\n{rule}\n❯ \n{rule}\x1b[4;3H"
        ));
        assert_eq!(type_if_up(&pane.held, &expect, b"/exit").ok(), Some(true));
        assert_eq!(pane.typed(), vec!["/exit"]);
    }

    /// The hand-over's probe of a pane that has shown no reading (t-84850 r2):
    /// one space typed at the empty field; a CLI that draws it has it erased
    /// at once and counts as reading; one that does not is refused with the
    /// space typed and nothing more, and the space is erased when it shows
    /// late. Never typed onto a draft.
    #[test]
    fn the_probe_is_erased_when_it_shows_and_typed_once_where_it_does_not() {
        let rule = "─".repeat(40);
        let empty = format!("\x1b[?2004h● Done.\n\n{rule}\n❯ \n{rule}\x1b[4;3H");
        let probed = format!("\x1b[?2004h● Done.\n\n{rule}\n❯  \n{rule}\x1b[4;4H");
        let wait = Duration::from_millis(600);
        let poll = Duration::from_millis(20);
        // A reading CLI draws the space a moment after it is typed.
        let reading = pane("Do you want to proceed?", &["Yes", "No"], true);
        reading.draw(&empty);
        let drawn = {
            let held = Arc::clone(&reading.held);
            let probed = probed.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(100));
                let mut pty = lock_pty(&held);
                pty.terminal_mut().feed(b"\x1b[2J\x1b[H");
                pty.terminal_mut()
                    .feed(probed.replace('\n', "\r\n").as_bytes());
            })
        };
        assert_eq!(
            probe_reading(&reading.held, 91_101, "claude", wait, poll)
                .ok()
                .map(|(probed, _)| probed),
            Some(Probe::Read)
        );
        drawn.join().expect("the drawing thread");
        assert_eq!(
            reading.typed(),
            vec![" ", "\x7f"],
            "the probe was not taken away"
        );
        // A deaf CLI draws nothing: the space is typed once, nothing follows.
        let deaf = pane("Do you want to proceed?", &["Yes", "No"], true);
        deaf.draw(&empty);
        let (probed_deaf, hand) =
            probe_reading(&deaf.held, 91_102, "claude", wait, poll).expect("the probe");
        assert_eq!(probed_deaf, Probe::Deaf);
        assert_eq!(deaf.typed(), vec![" "]);
        assert_eq!(
            erase_probe(&deaf.held, 91_102, "claude", hand).ok(),
            Some(Erased::NotShown)
        );
        deaf.draw(&probed);
        assert_eq!(
            erase_probe(&deaf.held, 91_102, "claude", hand).ok(),
            Some(Erased::Done)
        );
        assert_eq!(
            deaf.typed(),
            vec![" ", "\x7f"],
            "a late probe was not taken away"
        );
        // A person's hand reached the line before the probe showed: the
        // backspace would take their key, so nothing is erased (r3 P3-1).
        let touched = pane("Do you want to proceed?", &["Yes", "No"], true);
        touched.draw(&empty);
        let (_, hand) = probe_reading(&touched.held, 91_103, "claude", Duration::ZERO, poll)
            .expect("the probe");
        crate::human_input::typed(91_103);
        touched.draw(&probed);
        assert_eq!(
            erase_probe(&touched.held, 91_103, "claude", hand).ok(),
            Some(Erased::Touched)
        );
        assert_eq!(
            touched.typed(),
            vec![" "],
            "a backspace raced a person's key"
        );
        crate::human_input::forget_term(91_103);
        // Words on the line: nothing is typed at all.
        let drafted = pane("Do you want to proceed?", &["Yes", "No"], true);
        drafted.draw(&format!(
            "\x1b[?2004h● Done.\n\n{rule}\n❯ abc\n{rule}\x1b[4;6H"
        ));
        assert_eq!(
            probe_reading(&drafted.held, 91_104, "claude", wait, poll)
                .ok()
                .map(|(probed, _)| probed),
            Some(Probe::NotTyped)
        );
        assert!(drafted.typed().is_empty());
    }

    /// A second probe is never typed while the first is still out (t-84850
    /// r3): it would join the first, and no look would know the two spaces.
    /// Once the first shows late and is taken away, the pane may be asked
    /// again.
    #[test]
    fn a_pane_is_not_probed_again_while_its_first_probe_is_out() {
        const TERM: u32 = 91_201;
        let rule = "─".repeat(40);
        let empty = format!("\x1b[?2004h● Done.\n\n{rule}\n❯ \n{rule}\x1b[4;3H");
        let probed = format!("\x1b[?2004h● Done.\n\n{rule}\n❯  \n{rule}\x1b[4;4H");
        let (wait, late, poll) = (
            Duration::from_millis(200),
            Duration::from_secs(5),
            Duration::from_millis(20),
        );
        let deaf = pane("Do you want to proceed?", &["Yes", "No"], true);
        deaf.draw(&empty);
        assert_eq!(
            probe_pane(&deaf.held, TERM, "claude", wait, late, poll).ok(),
            Some(Probe::Deaf)
        );
        assert_eq!(
            probe_pane(&deaf.held, TERM, "claude", wait, late, poll).ok(),
            Some(Probe::StillOut),
            "a second press probed a pane whose first probe was out"
        );
        assert_eq!(deaf.typed(), vec![" "]);
        // The CLI wakes and draws the first space: the late look takes it
        // away and lets the pane be asked again.
        deaf.draw(&probed);
        let began = std::time::Instant::now();
        while probes_out().contains(&TERM) && began.elapsed() < late {
            std::thread::sleep(poll);
        }
        assert_eq!(
            deaf.typed(),
            vec![" ", "\x7f"],
            "the late probe was not taken away"
        );
        assert!(
            !probes_out().contains(&TERM),
            "the pane stayed marked after its probe went"
        );
        assert!(
            crate::human_input::reads_input(TERM),
            "a probe taken away late did not count the pane as reading (r3 P3-6)"
        );
        crate::human_input::forget_term(TERM);
    }

    /// How long a fake CLI on a real pty takes to leave after the keys the
    /// hand-over types (`cmd::wire::hand_over_pane`): the exit command's first
    /// key through this door, the rest after the line's step. The CLI reads
    /// nothing for its first `deaf` seconds — a resumed Claude Code can draw
    /// its screen and read no input for up to a minute — and then reads lines
    /// until its exit command. `None` when it is still there after the
    /// hand-over's watch (`HAND_OVER_LEAVE_LIMIT`); `then` says when it left,
    /// looked for up to `after` more. A shell loop draws no input line, so
    /// the door here asks for no menu only; no agent runs.
    #[cfg(unix)]
    fn fake_cli_leaves(deaf: u64, after: Duration) -> (Option<Duration>, Option<Duration>) {
        use crate::cmd::wire::{HAND_OVER_EXIT_POLL, HAND_OVER_LEAVE_LIMIT};
        let exit = zerocode_core::agent::agent_voice("claude")
            .exit_command
            .expect("claude has an exit command");
        let script = format!(
            "sleep {deaf}; while IFS= read -r line; do [ \"$line\" = '{exit}' ] && exit 0; done"
        );
        let lane =
            zerocode_pty::PtyLane::spawn("/bin/sh", &["-c".into(), script], None, &[], 24, 80)
                .expect("the fake CLI starts");
        let held: HeldTerminal = Arc::new(Mutex::new(PtyHandle::new(lane)));
        let (groups, step) = zerocode_core::ask::line_keys(exit);
        let (first, rest) = groups.split_first().expect("a line has keys");
        let key = |group| crate::cmd::terminal::key_group_bytes(group);
        let started = Instant::now();
        assert_eq!(
            type_if_up(&held, &Expect::no_menu(), &key(first)).ok(),
            Some(true),
            "a resting screen takes the exit command"
        );
        for group in rest {
            std::thread::sleep(step);
            lock_pty(&held)
                .write_input(&key(group))
                .expect("the line's keys");
        }
        let left = |until: Duration| {
            while started.elapsed() < until {
                let mut pty = lock_pty(&held);
                pty.pump();
                if pty.try_wait().ok().flatten().is_some() {
                    return Some(started.elapsed());
                }
                drop(pty);
                std::thread::sleep(HAND_OVER_EXIT_POLL);
            }
            None
        };
        let within = left(HAND_OVER_LEAVE_LIMIT);
        let then = within.or_else(|| left(HAND_OVER_LEAVE_LIMIT + after));
        let _ = lock_pty(&held).kill();
        (within, then)
    }

    /// The hand-over's exit line ends a CLI resting at its prompt at once,
    /// and one that reads nothing for its first two seconds still within the
    /// hand-over's wait: the pty keeps the keys until the CLI reads (t-84850,
    /// the 「대화」 road's first turn on).
    #[cfg(unix)]
    #[test]
    fn a_hand_overs_exit_line_ends_a_resting_cli_and_one_that_reads_late() {
        let (resting, _) = fake_cli_leaves(0, Duration::ZERO);
        let resting = resting.expect("a resting CLI kept its screen after its exit line");
        assert!(
            resting < Duration::from_secs(2),
            "a resting CLI took {resting:?} to leave"
        );
        let (late, _) = fake_cli_leaves(2, Duration::ZERO);
        let late = late.expect("a CLI reading 2 s late kept its screen past the hand-over's wait");
        assert!(
            late >= Duration::from_millis(1_900),
            "a CLI that read nothing for 2 s left in {late:?}"
        );
        eprintln!("exit line: a resting CLI left in {resting:?}, one reading 2 s late in {late:?}");
    }

    /// A CLI that reads its line ten seconds late — past the 8 s a pane's CLI
    /// usually takes (`HAND_OVER_EXIT_WAIT`) — still leaves within the
    /// hand-over's watch (t-84850 r2 P2-5). Under r1 the window had refused
    /// and stopped the wire at 8 s, and the CLI left at 10.1 s: the
    /// conversation on no screen and no wire.
    #[cfg(unix)]
    #[test]
    fn a_cli_reading_its_line_ten_seconds_late_leaves_within_the_hand_overs_watch() {
        use crate::cmd::wire::HAND_OVER_EXIT_WAIT;
        let (within, _) = fake_cli_leaves(10, Duration::ZERO);
        let within = within.expect("a CLI reading 10 s late outlasted the hand-over's watch");
        assert!(
            within > HAND_OVER_EXIT_WAIT,
            "the CLI left in {within:?}, inside the usual wait — the test proves nothing"
        );
        eprintln!("deaf 10 s: left within the hand-over's watch at {within:?}");
    }

    /// A menu takes one row: the answer's single pick of a single-select
    /// question. Words only, several picks, a multi-select and an unanswered
    /// question are not a row.
    #[test]
    fn a_menu_takes_one_row_and_only_one() {
        let single = asked("Pick", &["a", "b", "c"]);
        let pick = |indices: &[usize]| AskSelection {
            indices: indices.to_vec(),
            other: String::new(),
        };
        assert_eq!(chosen_row(&single, &[pick(&[1])]), Some(1));
        assert_eq!(chosen_row(&single, &[pick(&[])]), None, "words only");
        assert_eq!(chosen_row(&single, &[pick(&[0, 2])]), None, "two picks");
        assert_eq!(chosen_row(&single, &[]), None, "nothing answered");
        let mut multi = asked("Pick any", &["a", "b"]);
        multi.multi_select = true;
        assert_eq!(chosen_row(&multi, &[pick(&[0])]), None, "a multi-select");
    }

    /// A CLI that is not Claude Code or Codex, waiting on a numbered menu:
    /// the card's choice reaches the pane — the selection walked to the row,
    /// looked at, and taken.
    #[test]
    fn a_choice_on_another_clis_menu_walks_the_selection_looks_and_takes_the_row() {
        let pane = pane("Select an approach", &["Rebase", "Merge", "Cancel"], true);
        let question = asked("Select an approach", &["Rebase", "Merge", "Cancel"]);
        assert_eq!(
            choose_row(&pane.held, &question, 2, NO_WAIT, NO_WAIT, &|| {}),
            Ok(())
        );
        assert_eq!(pane.typed(), vec!["\x1b[B", "\x1b[B", "\r"]);
        assert_eq!(*pane.chosen.lock().unwrap(), Some(2));
    }

    /// A CLI whose menu does not move on an arrow key: the look catches it
    /// and the Enter is never typed.
    #[test]
    fn a_menu_that_ignores_the_arrows_is_not_entered() {
        let pane = pane("Select an approach", &["Rebase", "Merge", "Cancel"], false);
        let question = asked("Select an approach", &["Rebase", "Merge", "Cancel"]);
        assert_eq!(
            choose_row(&pane.held, &question, 2, NO_WAIT, NO_WAIT, &|| {}),
            Err(Refusal::Changed)
        );
        assert!(
            !pane.typed().contains(&"\r".to_string()),
            "an Enter was typed on a menu whose selection never moved: {:?}",
            pane.typed()
        );
        assert_eq!(*pane.chosen.lock().unwrap(), None);
    }

    /// Choosing on a pane that is no longer on the question types nothing.
    #[test]
    fn a_choice_on_a_screen_that_moved_on_types_nothing() {
        let pane = pane("Select an approach", &["Rebase", "Merge", "Cancel"], true);
        let question = asked("Select an approach", &["Rebase", "Merge", "Cancel"]);
        pane.draw("● Rebased.\n\n❯ \n");
        assert_eq!(
            choose_row(&pane.held, &question, 1, NO_WAIT, NO_WAIT, &|| {}),
            Err(Refusal::Changed)
        );
        assert!(pane.typed().is_empty());
    }

    /// The card a pane's screen is drawn as, for a pane whose agent said only
    /// that it waits — and none for a screen with no menu on it.
    #[test]
    fn a_waiting_panes_numbered_menu_is_drawn_as_a_card() {
        let pane = pane("Select an approach", &["Rebase", "Merge", "Cancel"], true);
        let card = screen_card(&pane.held).map(|card| card.questions);
        let only = card.unwrap_or_default();
        assert_eq!(only.len(), 1);
        assert_eq!(only[0].question, "Select an approach");
        let labels: Vec<&str> = only[0]
            .options
            .iter()
            .map(|option| option.label.as_str())
            .collect();
        assert_eq!(labels, vec!["Rebase", "Merge", "Cancel"]);
        pane.draw("● Done.\n\n❯ \n");
        assert_eq!(screen_card(&pane.held), None);
    }

    /// A terminal being parsed is not waited for: the ask comes back empty
    /// and the next one finds the pane.
    #[test]
    fn a_pane_busy_in_its_parse_is_not_waited_for() {
        let pane = pane("Select an approach", &["Rebase", "Merge", "Cancel"], true);
        let held = Arc::clone(&pane.held);
        let busy = lock_pty(&held);
        assert_eq!(screen_card(&pane.held), None);
        drop(busy);
        assert!(screen_card(&pane.held).is_some());
    }

    /// One answer at a time per pane; another pane is another matter.
    #[test]
    fn a_second_answer_to_the_same_pane_is_refused_while_one_is_in_flight() {
        let first = answer_lease(910_001).expect("a free pane");
        assert_eq!(
            answer_lease(910_001).err(),
            Some(Refusal::InFlight),
            "two answers were allowed to cross"
        );
        assert!(answer_lease(910_002).is_ok(), "another pane is not blocked");
        drop(first);
        assert!(answer_lease(910_001).is_ok(), "the pane is free again");
    }

    /// MEASUREMENT (ignored): what the door costs a pane and what it keeps. The
    /// reading is paid when a pane that waits is listed for the board and when
    /// an answer is about to be typed — never per output chunk — so this is the
    /// whole bill: the card's screen read (lock, copy, reading), the typed
    /// answer's look-and-write, and the memory after a long run of both.
    /// Run on purpose, normally and under `taskpolicy -b`:
    /// `cargo test -p zerocode-shell --bin zerocode-shell answer_door::tests::measure -- --ignored --nocapture`.
    #[test]
    #[ignore = "a measurement, run on purpose: -- --ignored --nocapture"]
    fn measure_the_door_and_what_it_keeps() {
        use std::time::Instant;
        const ITERATIONS: u32 = 50_000;
        const LEASE_CYCLES: u32 = 20_000;
        // The loops run in this many equal parts and the resident size is read
        // after each, so a plateau and a leak look different.
        const PARTS: u32 = 5;
        let rss_kib = || -> u64 {
            let out = crate::proc::quiet_command("ps")
                .args(["-o", "rss=", "-p", &std::process::id().to_string()])
                .output()
                .expect("ps");
            String::from_utf8_lossy(&out.stdout)
                .trim()
                .parse()
                .unwrap_or(0)
        };
        let written = Arc::new(Mutex::new(Vec::new()));
        let mut cli = MenuCli {
            terminal: Terminal::new(40, 120),
            title: "Select an approach".to_string(),
            options: vec!["Rebase".into(), "Merge".into(), "Cancel".into()],
            selected: 0,
            follows_arrows: true,
            written: Arc::clone(&written),
            chosen: Arc::new(Mutex::new(None)),
        };
        cli.redraw();
        let held: HeldTerminal = Arc::new(Mutex::new(PtyHandle::new(cli)));
        let question = asked("Select an approach", &["Rebase", "Merge", "Cancel"]);
        let before = rss_kib();

        let mut cards = 0_u32;
        let mut card_time = Duration::ZERO;
        let mut card_rss = Vec::new();
        for _ in 0..PARTS {
            let began = Instant::now();
            for _ in 0..ITERATIONS / PARTS {
                cards += u32::from(screen_card(&held).is_some());
            }
            card_time += began.elapsed();
            card_rss.push(rss_kib());
        }
        let card_nanos = card_time.as_nanos() as f64 / f64::from(ITERATIONS);

        let mut typed = 0_u32;
        let mut type_time = Duration::ZERO;
        let mut type_rss = Vec::new();
        for _ in 0..PARTS {
            let began = Instant::now();
            for round in 0..ITERATIONS / PARTS {
                typed += u32::from(
                    type_if_up(&held, &Expect::question(&question), b"x").unwrap_or(false),
                );
                if round.is_multiple_of(1_000) {
                    written.lock().unwrap().clear();
                }
            }
            type_time += began.elapsed();
            type_rss.push(rss_kib());
        }
        let type_nanos = type_time.as_nanos() as f64 / f64::from(ITERATIONS);

        let mut lease_rss = Vec::new();
        for part in 0..PARTS {
            for term in 0..LEASE_CYCLES / PARTS {
                let pane = 920_000 + part * (LEASE_CYCLES / PARTS) + term;
                drop(answer_lease(pane).expect("a free pane"));
            }
            lease_rss.push(rss_kib());
        }
        println!(
            "MEASURE door: screen_card {:.1} us/call ({cards} cards of {ITERATIONS}), type_if_up {:.1} us/call ({typed} typed of {ITERATIONS}) — look + write under one hold of the pane's lock",
            card_nanos / 1_000.0,
            type_nanos / 1_000.0
        );
        println!(
            "MEASURE memory: rss {before} KiB at the start; after each fifth of {ITERATIONS} card reads {card_rss:?}; of {ITERATIONS} typed answers {type_rss:?}; of {LEASE_CYCLES} lease cycles on distinct panes {lease_rss:?} (a plateau is no growth)"
        );
    }

    /// The pane's screen swapped for another terminal's — a recording of the real CLI.
    fn show_on(pane: &Pane, screen: zerocode_pty::Terminal) {
        *lock_pty(&pane.held).terminal_mut() = screen;
    }

    /// The door types at the field of a real Claude Code 2.1.316 (t-86909 r3). That CLI hides the
    /// terminal's cursor for good and draws its caret as a reversed cell, so a door that asked for
    /// a SHOWN cursor refused every pane of it: the queue's 끼워 넣기, its stop and the words after
    /// it all came back `turn-screen`. Replayed from the recordings, the inline layout with its
    /// transcript above the field and the alternate-screen one, with the queued placeholder on
    /// the field too — the bracketed paste goes in, then its Enter.
    #[test]
    fn the_door_types_at_a_real_claude_code_2_1_316_field_that_hides_its_cursor() {
        use crate::composer_empty::fixtures::{RECORDED_MARKS, recorded};
        let mut term = 6_200_010;
        for (fixture, marks) in RECORDED_MARKS {
            for mark in marks {
                for (moment, turn) in [(Moment::During, running()), (Moment::After, over())] {
                    term += 1;
                    let pane = pane("Do you want to proceed?", &["Yes", "No"], true);
                    show_on(&pane, recorded(fixture, mark));
                    assert_eq!(
                        type_queued_line(&pane.held, term, &turn, moment, "words", QUICK),
                        Ok(()),
                        "{mark} {moment:?}: the door refused a field that is empty"
                    );
                    assert_eq!(
                        pane.typed(),
                        vec!["\x1b[200~words\x1b[201~", "\r"],
                        "{mark} {moment:?}"
                    );
                }
            }
        }
    }

    /// And it types behind nothing a person wrote there, under the same hidden cursor: the shapes
    /// where the caret stands on the field's first cell and the field looks like the queued
    /// placeholder (a reversed first letter, then nothing).
    #[test]
    fn the_door_refuses_a_draft_on_a_field_that_hides_its_cursor() {
        use crate::composer_empty::fixtures::{CARET, hidden};
        let mut term = 6_200_100;
        for (what, field, cursor_col) in [
            (
                "one letter and the caret after it",
                format!("❯ x{CARET}"),
                3,
            ),
            (
                "one letter and the caret moved to the start",
                "❯ \x1b[7mx\x1b[27m".to_string(),
                2,
            ),
            (
                "letters after the caret's letter",
                "❯ \x1b[7mx\x1b[27myz".to_string(),
                2,
            ),
        ] {
            term += 1;
            let pane = pane("Do you want to proceed?", &["Yes", "No"], true);
            show_on(&pane, hidden(&field, cursor_col));
            assert_eq!(
                type_queued_line(&pane.held, term, &running(), Moment::During, "words", QUICK),
                Err(refused::SCREEN.to_string()),
                "{what}"
            );
            assert_eq!(
                type_at_a_turn(&pane.held, &running(), b"\x1b"),
                Err(refused::SCREEN.to_string()),
                "{what}: the stop key was typed at a draft"
            );
            assert!(pane.typed().is_empty(), "{what}: something was typed");
        }
    }

    /// The hand-over's second look and the way back ask one question of a pane whose hook says its
    /// turn is over (t-86909 r4): has its output been silent for the quiet time? One that wrote a
    /// moment ago has not — a message the CLI held in its own queue began its turn with no hook — and
    /// one silent for the whole time has.
    #[test]
    fn a_pane_is_at_rest_only_once_its_output_has_been_silent_for_the_quiet_time() {
        let wrote = Instant::now();
        let quiet = REST_QUIET;
        assert!(
            !silent_for(Some(wrote), wrote, quiet),
            "it wrote this instant"
        );
        assert!(!silent_for(
            Some(wrote),
            wrote + Duration::from_millis(100),
            quiet
        ));
        assert!(
            !silent_for(Some(wrote), wrote + quiet - Duration::from_millis(1), quiet),
            "a millisecond short of the quiet time"
        );
        assert!(
            silent_for(Some(wrote), wrote + quiet, quiet),
            "the whole quiet time"
        );
        assert!(silent_for(
            Some(wrote),
            wrote + Duration::from_secs(5),
            quiet
        ));
        // A clock that reads earlier than the write does not make a pane quiet, nor fail.
        assert!(!silent_for(
            Some(wrote + Duration::from_secs(1)),
            wrote,
            quiet
        ));
        // A transport that does not say when it last wrote shows nothing against it.
        assert!(silent_for(None, wrote, quiet));
    }

    /// The wait is what its last look found: yes as soon as a look says so, no only once the wait
    /// is over — and a wait of nothing still looks once.
    #[test]
    fn the_wait_for_quiet_answers_what_its_last_look_found() {
        let mut looks = 0;
        assert!(wait_until(
            || {
                looks += 1;
                looks == 3
            },
            Duration::from_secs(5),
            Duration::from_millis(1)
        ));
        assert_eq!(looks, 3, "it looked again after a yes");
        let began = Instant::now();
        let mut never = 0;
        assert!(!wait_until(
            || {
                never += 1;
                false
            },
            Duration::from_millis(60),
            Duration::from_millis(5)
        ));
        assert!(
            began.elapsed() >= Duration::from_millis(60),
            "it gave up before the wait was over"
        );
        assert!(never > 1, "it looked once and gave up");
        let mut once = 0;
        assert!(!wait_until(
            || {
                once += 1;
                false
            },
            Duration::ZERO,
            Duration::from_millis(5)
        ));
        assert_eq!(once, 1);
    }

    /// The same question asked of a real pane: one whose last write is a moment short of the quiet
    /// time is waited for and found quiet; one that wrote just now and needs longer than the wait is
    /// called busy only once the wait is over; one already silent is quiet at once.
    #[test]
    fn a_pane_about_to_be_quiet_is_waited_for_and_a_busy_one_only_to_the_bound() {
        let quiet = Duration::from_millis(300);
        let look = Duration::from_millis(10);
        let settling = secret_pane("Done.", Duration::from_millis(200));
        let began = Instant::now();
        assert!(wait_for_quiet(
            &settling.held,
            quiet,
            Duration::from_secs(5),
            look
        ));
        assert!(
            began.elapsed() < Duration::from_secs(4),
            "it waited out the whole bound for a pane that needed 100 ms"
        );
        let busy = secret_pane("Working...", Duration::ZERO);
        let began = Instant::now();
        let bound = Duration::from_millis(150);
        assert!(!wait_for_quiet(
            &busy.held,
            Duration::from_secs(60),
            bound,
            look
        ));
        assert!(
            began.elapsed() >= bound,
            "it called the pane busy before the bound"
        );
        let resting = secret_pane("Done.", quiet * 2);
        assert!(wait_for_quiet(&resting.held, quiet, bound, look));
        assert!(output_quiet(&lock_pty(&resting.held), quiet));
        assert!(!output_quiet(
            &lock_pty(&busy.held),
            Duration::from_secs(60)
        ));
    }
}
