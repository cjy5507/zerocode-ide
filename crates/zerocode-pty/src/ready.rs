//! When an agent's TUI is ready to be handed a prompt — and the handing.
//!
//! Writing a prompt into an agent the moment its process exists does not work:
//! the program has not drawn its input line yet, and the bytes land in a
//! buffer nobody reads, or worse, in whatever it *was* showing. So the prompt
//! waits for the program to say it is listening — and this is what listens for
//! that, plus the [`PromptDelivery`] driver that does the sending once it has.
//!
//! **The handshake is bracketed paste.** A TUI that means to read keys turns
//! it on (`ESC[?2004h`), and that is the first thing worth waiting for: it is
//! the program stating what it is, rather than us guessing from a title or a
//! delay. Nothing is ready before it arrives, whatever else the screen does —
//! with ONE measured exception: a glyph drawn inside the alternate screen,
//! whose anchor is the screen switch itself. The shell that runs the launch
//! command speaks bracketed paste too, so 2004 cannot tell grok's `❯` from
//! starship's; `?1049h` can, because the shell's prompt never lives there
//! (Orca's `grok-composer-prompt` anchor, draft-paste-ready-scanner.ts:49-63).
//!
//! **After the handshake, what "ready" means depends on the program.** One
//! shows its cursor when its input line is back; another prints a glyph on
//! that line; a third says nothing at all, and for that one the honest signal
//! is that it has stopped talking. Orca measures the same signals
//! (`createDraftPasteReadyScanner`, draft-paste-ready-scanner.ts:36-70).
//!
//! The signals are **edges, not levels** — "showed its cursor", not "cursor is
//! visible". Orca gets that for free by searching the bytes that arrive after
//! the handshake; a level would go off on leftovers. A cursor that has been
//! visible since boot has not *said* anything, and a marker glyph still on
//! screen from the previous run is scrollback, not readiness. Hence the
//! [`TerminalGrid::cursor_shows`](crate::TerminalGrid::cursor_shows) counter
//! and
//! [`TerminalGrid::take_glyph_drawn`](crate::TerminalGrid::take_glyph_drawn),
//! which asks "was the glyph among the cells drawn this round" — both survive
//! chunk boundaries that would split a byte search, since the stream is
//! already parsed into a grid on this side and a glyph in a cell cannot be
//! split at all.

use std::time::{Duration, Instant};

use crate::input::encode_paste;

/// How long a silent program must stay silent before it counts as ready.
///
/// Orca's `BRACKETED_PASTE_QUIET_MS`. It is a guess by nature — that is the
/// point of the other two signals — but it is a guess with a floor: the
/// program has already said it reads keys, so the only question left is
/// whether it has finished drawing.
pub const QUIET: Duration = Duration::from_millis(1500);

/// How long a program may stay SILENT before a launch wait gives up on it.
/// Orca's `READINESS_TIMEOUT_MS` — Orca measures it from the launch, this
/// window from the last write (see `Readiness::settle_if_late`).
pub const TIMEOUT: Duration = Duration::from_millis(8_000);

/// The most a launch wait lasts whatever the program does — the cap for one
/// that never rests. A session resumed from a long transcript replays it for
/// well over the shared deadline; a spinner that redraws forever must still
/// be given up on so the caller is answered.
pub const PATIENCE: Duration = Duration::from_secs(120);

/// The pause between the paste and its Enter. Orca sleeps exactly this long
/// before writing the `\r` (`sendBracketedPasteToAgent`,
/// agent-paste-draft-C1MiUIC6.js:343-355) — a TUI that has just swallowed a
/// paste may still be reflowing, and an Enter glued to the last byte of the
/// envelope can land inside that redraw.
pub const SUBMIT_GAP: Duration = Duration::from_millis(50);

/// Time reserved after delivery for a provider's prompt-submit hook.
///
/// Kept below the bridge's three-second grace: the native waiter must always
/// finish first, while still leaving enough room for one quiet-window check
/// and one Enter retry on a busy TUI.
pub const SUBMIT_ACK_TIMEOUT: Duration = Duration::from_secs(2);

/// Ctrl+U — an agent TUI clears toward the start of its input buffer.
pub const CLEAR_INPUT_LINE: u8 = 0x15;

/// Ctrl+K — and toward the end.
pub const CLEAR_INPUT_FORWARD: u8 = 0x0b;

/// Headroom over the lines this window knows about. What it injected is a
/// LOWER BOUND on what the buffer holds — the person can type straight into
/// the TUI line — so the count is biased upward. Overshoot is free: Orca
/// measured 41 Ctrl+U against a one-line buffer perfectly clean on both
/// agents, and an undershoot is what leaves residue to glue onto the next
/// message (`AGENT_TUI_CLEAR_LINE_SLACK`, agent-tui-input-clear.ts).
pub const CLEAR_LINE_SLACK: usize = 8;

/// Bounds the burst so a pathological draft cannot emit an unbounded write.
pub const CLEAR_MAX_LINES: usize = 40;

/// The bytes that empty an agent TUI's input buffer of up to `line_count`
/// logical lines, from ANY cursor position — Orca's 2N−1 law
/// (`buildAgentTuiClearInput`): from the worst seat, line N of N, it takes
/// N−1 joins to walk up plus N−1 more to eat the joined lines, plus one for
/// the line under the cursor; then the same toward the end.
#[must_use]
pub fn clear_input(line_count: usize) -> Vec<u8> {
    let lines = line_count.clamp(1, CLEAR_MAX_LINES);
    let repetitions = 2 * lines - 1;
    let mut bytes = vec![CLEAR_INPUT_LINE; repetitions];
    bytes.extend(std::iter::repeat_n(CLEAR_INPUT_FORWARD, repetitions));
    bytes
}

/// Logical lines in `text`. Visual wrapping is irrelevant to the clear cost.
#[must_use]
pub fn count_input_lines(text: &str) -> usize {
    text.split(['\n', '\r']).count() - text.matches("\r\n").count()
}

/// Clear bytes for a buffer believed to hold `text`, with slack for the
/// edits a person made in the TUI itself (`buildAgentTuiClearInputForText`).
#[must_use]
pub fn clear_input_for_text(text: &str) -> Vec<u8> {
    clear_input(count_input_lines(text) + CLEAR_LINE_SLACK)
}

/// What counts as "ready" for a given program, once it has shaken hands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadySignal {
    /// It shows its cursor when the input line is back (`?25l` … draw …
    /// `?25h`).
    CursorShown,
    /// It prints this glyph on its input line.
    Prompt(char),
    /// It prints this glyph too — but the glyph is only its own inside the
    /// ALTERNATE SCREEN, because in the normal buffer the same character is
    /// the default prompt of popular shells (starship, pure), including the
    /// very shell that ran the launch command. The glyph door therefore
    /// bypasses the handshake gate — its anchor is the screen switch — and
    /// silence after the handshake stays on as the floor, because the
    /// program can be told to render inline (grok `--no-alt-screen`,
    /// `[ui] screen_mode = "minimal"`) where the switch never comes and its
    /// shimmering startup logo means the quiet window is all that is left
    /// (Orca's `grok-composer-prompt`, draft-paste-ready-scanner.ts:49-63).
    AltScreenPrompt(char),
    /// It says nothing in particular, so silence is the signal.
    Quiet,
    /// It already holds the pty and has drawn its composer — the send road's
    /// signal, not a launch signal. Any sign of rest says go: its own glyph
    /// freshly drawn, the cursor shown again, or the stream settling into
    /// silence, whichever comes first. Waiting for a launch announcement here
    /// starves — a TUI at rest never announces itself again (live report
    /// 2026-08-14: an idle codex took no send for the whole eight-second
    /// budget).
    Rest(Option<char>),
}

impl ReadySignal {
    /// The glyph this signal waits to see printed, when it waits for one.
    ///
    /// The pump has to ask the grid about a *particular* character —
    /// [`TerminalGrid::take_glyph_drawn`](crate::TerminalGrid::take_glyph_drawn)
    /// notices one, and the signal is the only thing that knows which. `None`
    /// for the other two: they are read off state the terminal keeps anyway
    /// and cost the paint path nothing.
    #[must_use]
    pub const fn marker(&self) -> Option<char> {
        match self {
            Self::Prompt(glyph) | Self::AltScreenPrompt(glyph) => Some(*glyph),
            Self::Rest(glyph) => *glyph,
            Self::CursorShown | Self::Quiet => None,
        }
    }
}

/// One pump round's worth of what the terminal did.
#[derive(Debug, Clone, Copy, Default)]
pub struct Observed {
    /// The child wrote something this round.
    pub wrote: bool,
    /// The child has bracketed paste on — the handshake.
    pub bracketed_paste: bool,
    /// Cumulative count of `?25h` emissions
    /// ([`TerminalGrid::cursor_shows`](crate::TerminalGrid::cursor_shows)).
    pub cursor_shows: u64,
    /// The ready glyph was among the cells drawn this round — the answer
    /// [`TerminalGrid::take_glyph_drawn`](crate::TerminalGrid::take_glyph_drawn)
    /// gives when it is asked about [`PromptDelivery::marker`].
    pub marker_written: bool,
    /// …and at least one of those draws went to the ALTERNATE screen, judged
    /// at pen time — the half of the answer [`AltScreenPrompt`] and the
    /// pre-handshake door read, because there the screen is what separates
    /// the agent's glyph from a shell prompt wearing the same character.
    ///
    /// [`AltScreenPrompt`]: ReadySignal::AltScreenPrompt
    pub marker_in_alt: bool,
    /// The alternate screen is up as this round ends. Leaving it is what
    /// REVOKES the pre-handshake memory: the screen was handed back to the
    /// shell, and a glyph after that is the shell's prompt (Orca revokes on
    /// `?1049l` the same way, draft-paste-ready-scanner.ts:51-55).
    pub alt_screen: bool,
}

/// Where the wait has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Still waiting, and there is still time.
    Waiting,
    /// The program is listening. Send the prompt.
    Ready,
    /// It never said so. The caller decides whether to send anyway — Orca
    /// gives one fallback check (is the expected process actually on the
    /// pty?) and aborts without sending when that fails too.
    TimedOut,
}

/// Which door a wait that has not reached [`State::Ready`] is still waiting
/// at — the sentence a failed launch owes whoever reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unmet {
    /// The program never turned bracketed paste on.
    Handshake,
    /// It shook hands and kept writing: the quiet window never opened.
    Quiet {
        /// How long ago it last wrote, as of the question.
        last_write: Duration,
    },
    /// It shook hands and never drew its composer glyph.
    Glyph(char),
    /// It shook hands and never showed its cursor again.
    Cursor,
}

impl std::fmt::Display for Unmet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Handshake => f.write_str(
                "the program never turned on bracketed paste, so nothing said it reads keys",
            ),
            Self::Quiet { last_write } => write!(
                f,
                "the program kept writing (its last output {} ms ago), so the quiet it is launched on never came",
                last_write.as_millis()
            ),
            Self::Glyph(glyph) => write!(f, "the program never drew its composer glyph {glyph}"),
            Self::Cursor => {
                f.write_str("the program never showed its cursor again after the handshake")
            }
        }
    }
}

/// The wait for one agent, on one shell.
#[derive(Debug, PartialEq, Eq)]
pub struct Readiness {
    signal: ReadySignal,
    quiet: Duration,
    timeout: Duration,
    started: Instant,
    /// When the handshake arrived, with the `cursor_shows` reading taken on
    /// that round — emissions up to and including the handshake round belong
    /// to the program's *previous* life, not to its answer.
    shook: Option<(Instant, u64)>,
    /// The last round the child wrote something.
    spoke: Instant,
    /// A [`Prompt`] glyph was drawn inside the alternate screen BEFORE the
    /// handshake, and the program has not left that screen since — Orca's
    /// `sawCodexPromptInAltScreen` (draft-paste-ready-scanner.ts:174-203).
    /// An idle composer drawn before `?2004h` never redraws after it, and a
    /// wait that only reads post-handshake rounds starves against it.
    ///
    /// [`Prompt`]: ReadySignal::Prompt
    saw_marker_in_alt: bool,
    settled: Option<State>,
}

impl Readiness {
    pub fn new(signal: ReadySignal, now: Instant) -> Self {
        Self::with_deadlines(signal, now, QUIET, TIMEOUT)
    }

    /// The same machine on faster clocks. For tests: nobody should wait eight
    /// real seconds to prove a timeout times out.
    pub fn with_deadlines(
        signal: ReadySignal,
        now: Instant,
        quiet: Duration,
        timeout: Duration,
    ) -> Self {
        Self {
            signal,
            quiet,
            timeout,
            started: now,
            shook: None,
            spoke: now,
            saw_marker_in_alt: false,
            settled: None,
        }
    }

    /// Feed one round of observation and say where the wait stands.
    ///
    /// Settled states stick: a caller that keeps pumping after `Ready` must
    /// not be told `TimedOut` a moment later, because the answer it acted on
    /// was the first one.
    pub fn observe(&mut self, seen: Observed, now: Instant) -> State {
        if let Some(settled) = self.settled {
            return settled;
        }
        if seen.wrote {
            self.spoke = now;
        }

        // The alternate-screen glyph outranks the handshake gate: its anchor
        // is the screen switch, not `?2004h`, because the shell that ran the
        // launch command speaks bracketed paste too and its prompt may be
        // this very character. The switch is the one line the shell cannot
        // cross, so a draw judged inside it is the program's own whenever it
        // happens (Orca's `grok-composer-prompt` marker path,
        // draft-paste-ready-scanner.ts:49-56,100-113).
        if matches!(&self.signal, ReadySignal::AltScreenPrompt(_)) && seen.marker_in_alt {
            self.settled = Some(State::Ready);
            return State::Ready;
        }

        // Nothing else is ready before the handshake — not a shown cursor,
        // not a glyph that happened to be on screen already, not silence. A
        // program that has not asked for bracketed paste has not said it
        // reads keys, and a prompt sent to it is a prompt sent to whatever
        // it *is* doing.
        let Some((shook_at, shows_at_handshake)) = self.shook else {
            // But the pre-handshake rounds are not unread: a `Prompt` glyph
            // drawn while the program holds the alternate screen is its
            // composer already mounted, and an idle composer never redraws
            // once `?2004h` finally arrives — waiting for the next draw
            // starves. The memory is alt-screen-anchored for the same
            // reason as above, and leaving the screen revokes it (Orca's
            // `scanCodexPreAnchorPrompt`, scanner:174-203,228-230).
            if matches!(&self.signal, ReadySignal::Prompt(_)) {
                if !seen.alt_screen {
                    self.saw_marker_in_alt = false;
                } else if seen.marker_in_alt {
                    self.saw_marker_in_alt = true;
                }
            }
            if seen.bracketed_paste {
                self.shook = Some((now, seen.cursor_shows));
                if self.saw_marker_in_alt {
                    self.settled = Some(State::Ready);
                    return State::Ready;
                }
            }
            // Otherwise deliberately not falling through on the round the
            // handshake lands. Orca only looks for its marker in what
            // arrives AFTER the handshake; anything this round carried
            // arrived in the same breath as the announcement, and a screen
            // already wearing the glyph would otherwise read as ready the
            // instant the program started.
            return self.settle_if_late(now);
        };

        let ready = match &self.signal {
            ReadySignal::CursorShown => seen.cursor_shows > shows_at_handshake,
            ReadySignal::Prompt(glyph) => {
                debug_assert!(!glyph.is_whitespace(), "a blank marker matches everything");
                seen.marker_written
            }
            // The glyph half already answered above; what remains here is
            // the quiet floor, on the same clock `Quiet` reads — Orca arms
            // grok's quiet window on `?2004h` exactly so an inline launch
            // (no screen switch, no glyph) still has a delivery path
            // (scanner:57-63).
            ReadySignal::AltScreenPrompt(_) => {
                now.duration_since(self.spoke.max(shook_at)) >= self.quiet
            }
            // The clock starts at the handshake, not at spawn: a program that
            // was quiet for a while *before* announcing itself has not yet
            // drawn anything, and that silence says nothing about being ready.
            ReadySignal::Quiet => now.duration_since(self.spoke.max(shook_at)) >= self.quiet,
            // Three doors, first one open wins. The glyph and the cursor are
            // edges measured from the handshake like their launch cousins;
            // the silence clock is the same one `Quiet` reads.
            ReadySignal::Rest(glyph) => {
                (glyph.is_some() && seen.marker_written)
                    || seen.cursor_shows > shows_at_handshake
                    || now.duration_since(self.spoke.max(shook_at)) >= self.quiet
            }
        };
        if ready {
            self.settled = Some(State::Ready);
            return State::Ready;
        }
        self.settle_if_late(now)
    }

    /// Late is SILENT past the deadline, not slow.
    ///
    /// A launch wait used to end at `started + timeout` whatever the program
    /// was doing, and a session resumed from a long transcript replays it for
    /// longer than eight seconds without a pause — so its briefing was dropped
    /// while it was visibly still arriving (live report 2026-08-30, "터미널
    /// 3의 에이전트가 프롬프트를 받지 못했습니다"). A program that keeps
    /// writing has not failed to be ready; it has not finished. So the
    /// deadline is measured from its last write, and [`PATIENCE`] caps the
    /// whole wait for the program that never rests.
    ///
    /// The send road keeps the shared clock from the start: a running agent
    /// asked to take a prompt while it works IS the case that deadline was
    /// written for, and the caller waiting on that answer must not wait out
    /// two minutes for it.
    fn settle_if_late(&mut self, now: Instant) -> State {
        let late = match self.signal {
            ReadySignal::Rest(_) => now.duration_since(self.started) >= self.timeout,
            _ => {
                now.duration_since(self.spoke) >= self.timeout
                    || now.duration_since(self.started) >= PATIENCE
            }
        };
        if late {
            self.settled = Some(State::TimedOut);
            return State::TimedOut;
        }
        State::Waiting
    }

    /// Whether the handshake has arrived, for a caller that wants to say so.
    #[must_use]
    pub const fn shook_hands(&self) -> bool {
        self.shook.is_some()
    }

    /// The door this wait is still at, or `None` once it was answered.
    ///
    /// Read, never fed: asking does not move the wait. A wait that already
    /// timed out still names what it lacked — that is the moment the answer
    /// is wanted.
    #[must_use]
    pub fn unmet(&self, now: Instant) -> Option<Unmet> {
        if self.settled == Some(State::Ready) {
            return None;
        }
        if self.shook.is_none() {
            return Some(Unmet::Handshake);
        }
        Some(match self.signal {
            ReadySignal::Prompt(glyph) => Unmet::Glyph(glyph),
            ReadySignal::CursorShown => Unmet::Cursor,
            // Every signal with a silence door reports the silence: of its
            // doors it is the one a program can visibly hold shut.
            ReadySignal::Quiet | ReadySignal::AltScreenPrompt(_) | ReadySignal::Rest(_) => {
                Unmet::Quiet {
                    last_write: now.saturating_duration_since(self.spoke),
                }
            }
        })
    }

    /// The glyph this wait is listening for. See [`ReadySignal::marker`].
    #[must_use]
    pub const fn marker(&self) -> Option<char> {
        self.signal.marker()
    }
}

/// How a finished delivery ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The prompt — and its Enter, when one was asked for — went to the child.
    Delivered,
    /// The program never said it was ready. Nothing was sent.
    TimedOut,
    /// The line was not this delivery's to write on when the paste came due.
    /// Nothing was sent, and the words are still the caller's to place.
    Refused(Refusal),
    /// The words went in and the Enter did not: between the two, the line
    /// stopped being this delivery's to submit. They sit in the composer as a
    /// draft — a thing a person can read and send or clear.
    Unsubmitted(Refusal),
}

impl Outcome {
    /// Whether the words reached the child at all, whatever became of the
    /// Enter.
    #[must_use]
    pub const fn pasted(self) -> bool {
        matches!(self, Self::Delivered | Self::Unsubmitted(_))
    }
}

/// What the driver wants done this round.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Nothing this round; keep pumping.
    Waiting,
    /// Write these bytes to the child now: the clear keys, if any, and the
    /// paste envelope.
    Write(Vec<u8>),
    /// Write these bytes to the child now: the Enter that submits the paste.
    /// Its own step because it is its own decision — a caller that has to
    /// know an Enter reached the pane (the human-input tracker, a receipt)
    /// reads it here rather than sniffing the bytes.
    Submit(Vec<u8>),
    /// Finished. The caller reports the outcome and drops the delivery.
    Done(Outcome),
}

/// What the window knows about the LINE a delivery is addressed to, fed to
/// the delivery every round beside [`Observed`].
///
/// [`Observed`] is what the grid did; this is who else has a hand on the
/// composer. Both are facts the pump holds and the delivery cannot see for
/// itself, and both are read at the moment a write is due rather than when
/// the delivery was registered — a person can start typing, a question can
/// be put, a pane can be relaunched, all while a paste waits for its
/// composer to settle.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Line {
    /// Somebody typed here and nothing has reported taking what they typed.
    pub draft: bool,
    /// How many times a hand has reached this line, ever — `None` for a line
    /// nobody has touched. Compared across the paste-to-Enter gap, because
    /// the worst case leaves no draft behind: a person's own Enter closing
    /// over half-pasted words empties the line and only the count remembers
    /// they were there.
    pub hand: Option<u64>,
    /// A question or an approval is on screen, waiting for a person.
    pub parked: bool,
    /// Which launch the pane holds right now, as the window fingerprints it
    /// — `None` where the window recorded none.
    pub launch: Option<u64>,
}

/// What a delivery refuses to write over.
///
/// The default refuses nothing, which is what a launch briefing wants: the
/// composer it is addressed to is new, empty and the window's own. A door
/// that types at a pane on somebody else's behalf turns the refusals on,
/// and they are then decided at the write itself — not at registration,
/// where the answer can go stale in the readiness wait.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Guard {
    /// Refuse the paste while the line holds a person's unsent words, and
    /// refuse the Enter when a hand reached the line after the paste.
    pub hands: bool,
    /// Refuse both writes while a question or approval is parked: answering
    /// it belongs to the person it was put to.
    pub parked: bool,
    /// The launch this delivery was addressed to. A pane holding a different
    /// one is a different program, and words meant for the last occupant
    /// are not typed at the next.
    pub launch: Option<u64>,
}

impl Guard {
    /// A delivery that yields to everything a person owns on this line.
    #[must_use]
    pub const fn for_somebody_elses_line(launch: Option<u64>) -> Self {
        Self {
            hands: true,
            parked: true,
            launch,
        }
    }

    /// A delivery that owns its line and yields only to a relaunch.
    #[must_use]
    pub const fn for_its_own_line(launch: Option<u64>) -> Self {
        Self {
            hands: false,
            parked: false,
            launch,
        }
    }

    /// What, if anything, refuses the PASTE right now.
    #[must_use]
    pub fn against_paste(self, line: Line) -> Option<Refusal> {
        if self.launch.is_some() && self.launch != line.launch {
            return Some(Refusal::LaunchChanged);
        }
        if self.parked && line.parked {
            return Some(Refusal::Parked);
        }
        if self.hands && line.draft {
            return Some(Refusal::HoldsADraft);
        }
        None
    }

    /// What, if anything, refuses the ENTER right now, given the hand count
    /// read when the paste was written.
    #[must_use]
    pub fn against_submit(self, line: Line, hand_at_paste: Option<u64>) -> Option<Refusal> {
        if self.launch.is_some() && self.launch != line.launch {
            return Some(Refusal::LaunchChanged);
        }
        if self.parked && line.parked {
            return Some(Refusal::Parked);
        }
        if self.hands && line.hand != hand_at_paste {
            return Some(Refusal::HandReached);
        }
        None
    }
}

/// Why a guarded delivery withheld a write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The line holds words somebody typed and nothing has reported sending.
    HoldsADraft,
    /// A hand reached the line between the paste and the Enter.
    HandReached,
    /// A question or an approval is parked on the pane.
    Parked,
    /// The pane holds a different launch from the one addressed.
    LaunchChanged,
    /// The terminal's input queue rejected this entire write.
    InputRejected,
    /// The pane's zo refused its exact launch contract (t-2773): the program
    /// there will not run these words, so they were withheld rather than typed.
    LaunchRefused,
}

impl Refusal {
    /// Every refusal, for a table that must word each one.
    pub const ALL: [Self; 6] = [
        Self::HoldsADraft,
        Self::HandReached,
        Self::Parked,
        Self::LaunchChanged,
        Self::InputRejected,
        Self::LaunchRefused,
    ];

    /// The token the window's notice names this refusal by (t-10159). What
    /// it means to a person is the window's own table (`TERM_WITHHELD`), in
    /// the person's language; [`Self::says`] stays the receipt's and the
    /// black box's.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::HoldsADraft => "holds_a_draft",
            Self::HandReached => "hand_reached",
            Self::Parked => "parked",
            Self::LaunchChanged => "launch_changed",
            Self::InputRejected => "input_rejected",
            Self::LaunchRefused => "launch_refused",
        }
    }

    /// The refusal, in the words a receipt carries.
    #[must_use]
    pub const fn says(self) -> &'static str {
        match self {
            Self::HoldsADraft => {
                "the line is holding words somebody typed and nothing has reported \
                 sending; a paste would be appended to them and submitted with them"
            }
            Self::HandReached => {
                "somebody reached the line while the words were landing; they were \
                 pasted and left unsubmitted rather than sent with whatever else is \
                 now on it"
            }
            Self::Parked => {
                "the pane is holding a question or an approval; answering it belongs \
                 to the person it was put to"
            }
            Self::LaunchChanged => {
                "the pane no longer holds the program these words were addressed to"
            }
            Self::InputRejected => {
                "the terminal did not accept the input; delivery was not completed"
            }
            Self::LaunchRefused => {
                "the pane's zo refused its exact launch contract; the briefing was \
                 withheld rather than typed at a program that will not run it"
            }
        }
    }
}

/// One prompt on its way into one shell, driven by the pump.
///
/// The shape is a hand-rolled future on purpose: the pump loop is the one
/// place that already turns every terminal at the display rate, and a second
/// thread parked on a channel per pending prompt would be a second cadence to
/// keep honest. Feed it once per round; do what the [`Step`] says.
#[derive(Debug, PartialEq, Eq)]
pub struct PromptDelivery {
    text: String,
    submit: bool,
    clearing: bool,
    guard: Guard,
    /// The hand count read the round the paste was written, for the Enter
    /// to compare against. `None` until then, and `None` for a line nobody
    /// had touched.
    hand_at_paste: Option<u64>,
    wait: Readiness,
    phase: Phase,
}

#[derive(Debug, PartialEq, Eq)]
enum Phase {
    Waiting,
    /// The paste has gone; the Enter goes once the gap has passed.
    Submitting {
        pasted_at: Instant,
    },
    Done(Outcome),
}

impl PromptDelivery {
    pub fn new(text: String, submit: bool, signal: ReadySignal, now: Instant) -> Self {
        Self {
            text,
            submit,
            clearing: true,
            guard: Guard::default(),
            hand_at_paste: None,
            wait: Readiness::new(signal, now),
            phase: Phase::Waiting,
        }
    }

    /// The same delivery with the agent's own hard deadline. Orca resolves
    /// the timeout per agent (`resolveDraftPasteReadyTimeoutMs`: an override,
    /// else the agent's `draftPasteReadyTimeoutMs`, else 8000) — codex needs
    /// twenty seconds where everyone else settles under eight, and one
    /// shared deadline made every slow codex start a dropped prompt. The
    /// quiet window stays shared: no agent overrides it.
    pub fn with_timeout(
        text: String,
        submit: bool,
        signal: ReadySignal,
        now: Instant,
        timeout: Duration,
    ) -> Self {
        Self::with_deadlines(text, submit, signal, now, QUIET, timeout)
    }

    /// The same delivery with explicit readiness timing.
    ///
    /// Tests use short clocks; launchers use it for the small number of
    /// providers whose startup gate keeps painting after their composer first
    /// appears. Running sends stay on [`Self::new`] and the shared timing.
    pub fn with_deadlines(
        text: String,
        submit: bool,
        signal: ReadySignal,
        now: Instant,
        quiet: Duration,
        timeout: Duration,
    ) -> Self {
        Self {
            text,
            submit,
            clearing: true,
            guard: Guard::default(),
            hand_at_paste: None,
            wait: Readiness::with_deadlines(signal, now, quiet, timeout),
            phase: Phase::Waiting,
        }
    }

    /// Choose whether this delivery sends composer edit keys before its paste.
    ///
    /// The default remains `true` for backwards compatibility. Launch callers
    /// opt out because their newly mounted composer is empty; running sends
    /// take the answer from their agent catalog.
    #[must_use]
    pub const fn clearing(mut self, clearing: bool) -> Self {
        self.clearing = clearing;
        self
    }

    /// Choose what this delivery refuses to write over. See [`Guard`].
    #[must_use]
    pub const fn guarded(mut self, guard: Guard) -> Self {
        self.guard = guard;
        self
    }

    /// What this delivery refuses to write over.
    #[must_use]
    pub const fn guard(&self) -> Guard {
        self.guard
    }

    /// The words this delivery carries — for the caller that has to hand
    /// them to a person once the child could not be made to take them.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Settle after the terminal rejected an entire write before enqueueing
    /// it. A rejected Enter leaves an accepted paste in place; neither case
    /// may advance to Delivered or retry the write on a later pump round.
    pub fn reject_write(&mut self, paste_accepted: bool) -> Outcome {
        let outcome = if paste_accepted {
            Outcome::Unsubmitted(Refusal::InputRejected)
        } else {
            Outcome::Refused(Refusal::InputRejected)
        };
        self.phase = Phase::Done(outcome);
        outcome
    }

    /// Feed one pump round about a line nobody else has a hand on; do what
    /// the returned [`Step`] says.
    ///
    /// [`Self::poll_line`] with an untouched [`Line`]: right for a delivery
    /// with no guard, and for a driver that has no line facts to give. A
    /// guarded delivery fed this way sees no draft, no hand and no parked
    /// question — so a driver that means to protect any of those feeds the
    /// real line instead.
    pub fn poll(&mut self, seen: Observed, now: Instant) -> Step {
        self.poll_line(seen, Line::default(), now)
    }

    /// Feed one pump round; do what the returned [`Step`] says.
    ///
    /// `line` is read at the two moments that matter and nowhere else: the
    /// round the paste comes due, and the round the Enter does. Whatever the
    /// guard refuses settles the delivery on the spot with an [`Outcome`]
    /// that says how far it got — [`Outcome::Refused`] wrote nothing,
    /// [`Outcome::Unsubmitted`] wrote the words and withheld the Enter — and
    /// a settled delivery writes nothing further, however many rounds it is
    /// fed.
    pub fn poll_line(&mut self, seen: Observed, line: Line, now: Instant) -> Step {
        match self.phase {
            Phase::Waiting => match self.wait.observe(seen, now) {
                State::Waiting => Step::Waiting,
                State::TimedOut => {
                    self.phase = Phase::Done(Outcome::TimedOut);
                    Step::Done(Outcome::TimedOut)
                }
                State::Ready => {
                    // The line, looked at as the words are about to land —
                    // not when they were registered, which may have been a
                    // whole readiness wait ago.
                    if let Some(why) = self.guard.against_paste(line) {
                        self.phase = Phase::Done(Outcome::Refused(why));
                        return Step::Done(Outcome::Refused(why));
                    }
                    self.hand_at_paste = line.hand;
                    // A running composer is emptied first only when its agent
                    // consumes these edit keys. Launch composers are new and
                    // empty; several TUIs insert Ctrl+U/Ctrl+K as literal
                    // text. When present, the clear rides OUTSIDE the envelope:
                    // inside brackets a Ctrl+U is always paste data.
                    let mut bytes = if self.clearing {
                        clear_input_for_text("")
                    } else {
                        Vec::new()
                    };
                    // Always bracketed: the handshake this wait is gated on IS
                    // the program turning bracketed paste on, so by the time
                    // anything is written the envelope is wanted. Sanitisation
                    // lives inside `encode_paste`.
                    bytes.extend(encode_paste(&self.text, true));
                    if self.submit {
                        self.phase = Phase::Submitting { pasted_at: now };
                    } else {
                        self.phase = Phase::Done(Outcome::Delivered);
                    }
                    Step::Write(bytes)
                }
            },
            Phase::Submitting { pasted_at } => {
                if now.duration_since(pasted_at) >= SUBMIT_GAP {
                    // The line, looked at AGAIN, immediately before the
                    // Enter. The words have been sitting there for the gap,
                    // and a pane is not frozen while they do.
                    if let Some(why) = self.guard.against_submit(line, self.hand_at_paste) {
                        self.phase = Phase::Done(Outcome::Unsubmitted(why));
                        return Step::Done(Outcome::Unsubmitted(why));
                    }
                    self.phase = Phase::Done(Outcome::Delivered);
                    Step::Submit(b"\r".to_vec())
                } else {
                    Step::Waiting
                }
            }
            Phase::Done(outcome) => Step::Done(outcome),
        }
    }

    /// The glyph this delivery is waiting to see printed, if its agent
    /// announces itself with one.
    ///
    /// What the pump hands the grid each round so the paint path knows which
    /// character to notice. A delivery waiting on a caret or on silence asks
    /// for nothing, and nothing is watched for.
    #[must_use]
    pub const fn marker(&self) -> Option<char> {
        self.wait.marker()
    }

    /// What this delivery is still waiting for, while it waits. A delivery
    /// past its paste waits for nothing the program has to say.
    #[must_use]
    pub fn unmet(&self, now: Instant) -> Option<Unmet> {
        match self.phase {
            Phase::Waiting => self.wait.unmet(now),
            Phase::Submitting { .. } | Phase::Done(_) => None,
        }
    }

    /// Whether the paste itself has been written, whatever remains — or
    /// became of the Enter.
    #[must_use]
    pub const fn pasted(&self) -> bool {
        matches!(
            self.phase,
            Phase::Submitting { .. } | Phase::Done(Outcome::Delivered | Outcome::Unsubmitted(_))
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A composer glyph for the machine to wait on. Which agent wears which
    /// is its catalog row's fact; this crate only knows a glyph is a glyph.
    const COMPOSER_PROMPT: char = '\u{203a}';

    fn seen(wrote: bool, bracketed: bool, shows: u64, marker: bool) -> Observed {
        Observed {
            marker_in_alt: false,
            alt_screen: false,
            wrote,
            bracketed_paste: bracketed,
            cursor_shows: shows,
            marker_written: marker,
        }
    }

    /// A wait that failed says which door stayed shut.
    ///
    /// Measured 2026-09-17: three worker launches ended "did not accept its
    /// briefing: Err(Timeout)" and nothing said why. The panes had shaken
    /// hands and were answering a cursor poll five times a second — a wait
    /// that could name "kept writing, last 200 ms ago" would have pointed
    /// there in one line instead of an evening.
    #[test]
    fn an_unready_wait_names_the_door_that_stayed_shut() {
        let start = Instant::now();
        let mut never_shook = Readiness::new(ReadySignal::Quiet, start);
        never_shook.observe(seen(true, false, 0, false), start);
        assert_eq!(never_shook.unmet(start + QUIET), Some(Unmet::Handshake));

        let mut kept_writing = Readiness::new(ReadySignal::Quiet, start);
        kept_writing.observe(seen(true, true, 0, false), start);
        let last_write = start + QUIET * 2;
        kept_writing.observe(seen(true, true, 0, false), last_write);
        let asked = last_write + QUIET / 10;
        assert_eq!(
            kept_writing.observe(seen(false, true, 0, false), asked),
            State::Waiting
        );
        assert_eq!(
            kept_writing.unmet(asked),
            Some(Unmet::Quiet {
                last_write: QUIET / 10
            })
        );

        let mut no_glyph = Readiness::new(ReadySignal::Prompt(COMPOSER_PROMPT), start);
        no_glyph.observe(seen(true, true, 0, false), start);
        assert_eq!(no_glyph.unmet(start), Some(Unmet::Glyph(COMPOSER_PROMPT)));

        let mut no_cursor = Readiness::new(ReadySignal::CursorShown, start);
        no_cursor.observe(seen(true, true, 1, false), start);
        assert_eq!(no_cursor.unmet(start), Some(Unmet::Cursor));

        let mut ready = Readiness::new(ReadySignal::Quiet, start);
        ready.observe(seen(true, true, 0, false), start);
        assert_eq!(
            ready.observe(seen(false, true, 0, false), start + QUIET),
            State::Ready
        );
        assert_eq!(
            ready.unmet(start + QUIET),
            None,
            "a wait that was answered owes no reason"
        );

        let delivery = PromptDelivery::new("brief".to_string(), true, ReadySignal::Quiet, start);
        assert_eq!(delivery.unmet(start), Some(Unmet::Handshake));
        assert!(
            Unmet::Quiet {
                last_write: Duration::from_millis(180)
            }
            .to_string()
            .contains("180 ms")
        );
    }

    /// Nothing is ready until the program says it reads keys.
    ///
    /// A shell sitting at its prompt shows a cursor and is perfectly quiet,
    /// and it is NOT an agent waiting for a task. Without the handshake gate,
    /// every one of these would read as ready and the prompt would be typed
    /// at a shell.
    #[test]
    fn nothing_is_ready_before_the_handshake() {
        let start = Instant::now();
        for signal in [
            ReadySignal::CursorShown,
            ReadySignal::Prompt(COMPOSER_PROMPT),
            // The glyph is on screen — but in the NORMAL buffer, where it is
            // starship's prompt, not grok's composer. `marker_in_alt` stays
            // false and the alt-anchored door must stay shut.
            ReadySignal::AltScreenPrompt('❯'),
            ReadySignal::Quiet,
            ReadySignal::Rest(Some(COMPOSER_PROMPT)),
        ] {
            let mut wait = Readiness::new(signal.clone(), start);
            let long_after = start + QUIET * 3;
            assert_eq!(
                wait.observe(seen(false, false, 5, true), long_after),
                State::Waiting,
                "{signal:?} read a shell as an agent"
            );
            assert!(!wait.shook_hands());
        }
    }

    /// The round the handshake lands is not the round anything is ready.
    ///
    /// Orca only looks for its marker in what arrives after the handshake. A
    /// screen already carrying the glyph — scrollback from the last run, say —
    /// would otherwise satisfy the wait the instant the program announced
    /// itself, before it had drawn its own input line.
    #[test]
    fn the_handshake_round_is_not_the_ready_round() {
        let start = Instant::now();
        let mut wait = Readiness::new(ReadySignal::Prompt(COMPOSER_PROMPT), start);
        assert_eq!(
            wait.observe(seen(true, true, 0, true), start),
            State::Waiting,
            "the glyph was accepted on the handshake round itself"
        );
        assert!(wait.shook_hands());
        assert_eq!(
            wait.observe(seen(true, true, 0, true), start + QUIET),
            State::Ready
        );
    }

    /// A cursor that was already visible has not *said* anything.
    ///
    /// The signal is the emission — the program hid its cursor to redraw and
    /// showed it when the input line was back. The count at the handshake is
    /// the baseline; only a show after that answers. A level read here would
    /// fire on every program on earth, since terminals boot with the cursor
    /// on.
    #[test]
    fn only_a_show_after_the_handshake_answers() {
        let start = Instant::now();
        let mut wait = Readiness::new(ReadySignal::CursorShown, start);
        // Two shows happened before/with the handshake — previous life.
        assert_eq!(
            wait.observe(seen(true, true, 2, false), start),
            State::Waiting
        );
        assert_eq!(
            wait.observe(seen(true, true, 2, false), start + QUIET / 4),
            State::Waiting,
            "no new show, yet it fired"
        );
        assert_eq!(
            wait.observe(seen(true, true, 3, false), start + QUIET / 2),
            State::Ready
        );
    }

    /// A send to a running terminal lands when the stream settles.
    ///
    /// The launch glyph is exactly what an idle TUI never draws again — a
    /// delivery waiting on it starved for the whole timeout while codex sat
    /// at its composer saying nothing. Rest reads the same silence clock as
    /// `Quiet`, so nothing new needs to happen for it to fire.
    #[test]
    fn a_running_send_lands_on_silence() {
        let start = Instant::now();
        let mut wait = Readiness::new(ReadySignal::Rest(Some(COMPOSER_PROMPT)), start);
        // The running TUI's bracketed paste is latched on — the handshake
        // registers on the first look, and that round never fires.
        assert_eq!(
            wait.observe(seen(false, true, 5, false), start),
            State::Waiting
        );
        assert_eq!(
            wait.observe(
                seen(false, true, 5, false),
                start + QUIET + Duration::from_millis(1)
            ),
            State::Ready
        );
    }

    /// A send to a busy terminal takes the composer's own word early.
    ///
    /// Mid-stream there is no silence to wait for, but the program that
    /// redraws its input line — the glyph for codex, a shown cursor for the
    /// rest — has said it is listening, and the send must not sit out the
    /// quiet window it will never get.
    #[test]
    fn a_running_send_takes_an_announcement_early() {
        let start = Instant::now();
        let mut wait = Readiness::new(ReadySignal::Rest(Some(COMPOSER_PROMPT)), start);
        assert_eq!(
            wait.observe(seen(true, true, 5, false), start),
            State::Waiting
        );
        assert_eq!(
            wait.observe(seen(true, true, 5, true), start + QUIET / 4),
            State::Ready,
            "the glyph landed and the send still waited"
        );
        let mut cursor = Readiness::new(ReadySignal::Rest(None), start);
        assert_eq!(
            cursor.observe(seen(true, true, 5, false), start),
            State::Waiting
        );
        assert_eq!(
            cursor.observe(seen(true, true, 6, false), start + QUIET / 4),
            State::Ready,
            "the cursor came back and the send still waited"
        );
    }

    /// A silent program is ready when it has been silent since the handshake.
    ///
    /// Counted from the handshake and not from the last byte before it: a
    /// program that took a second to start has been quiet that whole time
    /// without having drawn anything, and that silence is not readiness.
    #[test]
    fn silence_is_measured_from_the_handshake() {
        let start = Instant::now();
        let mut wait = Readiness::new(ReadySignal::Quiet, start);
        let shook_at = start + QUIET * 2;
        assert_eq!(
            wait.observe(seen(false, true, 0, false), shook_at),
            State::Waiting
        );
        assert_eq!(
            wait.observe(seen(false, true, 0, false), shook_at + QUIET / 2),
            State::Waiting,
            "half the quiet window was enough"
        );
        assert_eq!(
            wait.observe(seen(false, true, 0, false), shook_at + QUIET),
            State::Ready
        );
    }

    /// Output restarts the silence.
    #[test]
    fn a_program_that_speaks_again_is_not_quiet() {
        let start = Instant::now();
        let mut wait = Readiness::new(ReadySignal::Quiet, start);
        wait.observe(seen(true, true, 0, false), start);
        let nearly = start + QUIET;
        assert_eq!(
            wait.observe(seen(true, true, 0, false), nearly),
            State::Waiting
        );
        assert_eq!(
            wait.observe(seen(false, true, 0, false), nearly + QUIET / 2),
            State::Waiting
        );
        assert_eq!(
            wait.observe(seen(false, true, 0, false), nearly + QUIET),
            State::Ready
        );
    }

    /// A program that never says it is ready stops being waited for.
    ///
    /// And the answer sticks. A caller that pumps once more after acting on
    /// `Ready` must not be handed `TimedOut` for the same wait — the decision
    /// was made on the first answer and the second would contradict it.
    #[test]
    fn the_wait_ends_and_its_answer_sticks() {
        let start = Instant::now();
        let mut wait = Readiness::new(ReadySignal::CursorShown, start);
        // It shook hands and then fell silent for the whole deadline — the
        // program that never says it is ready. (One still writing is not
        // late: `a_launch_wait_is_not_late_while_the_program_still_draws`.)
        assert_eq!(
            wait.observe(seen(true, true, 0, false), start),
            State::Waiting
        );
        assert_eq!(
            wait.observe(seen(false, true, 0, false), start + TIMEOUT),
            State::TimedOut
        );
        assert_eq!(
            wait.observe(seen(true, true, 9, false), start + TIMEOUT + QUIET),
            State::TimedOut,
            "a settled wait changed its mind"
        );

        let mut settled = Readiness::new(ReadySignal::CursorShown, start);
        settled.observe(seen(true, true, 0, false), start);
        assert_eq!(
            settled.observe(seen(false, true, 1, false), start + QUIET),
            State::Ready
        );
        assert_eq!(
            settled.observe(seen(false, true, 0, false), start + TIMEOUT * 2),
            State::Ready,
            "a wait that had already said Ready went back on it"
        );
    }

    /// The delivery pastes on Ready, waits the gap, then presses Enter.
    ///
    /// Two writes, never one: Orca sleeps 50ms between the envelope and the
    /// `\r` because a TUI that has just swallowed a paste may still be
    /// reflowing. Gluing the Enter to the envelope is how a prompt gets
    /// submitted into a half-drawn input line.
    #[test]
    fn the_delivery_pastes_waits_the_gap_then_presses_enter() {
        let start = Instant::now();
        let mut delivery =
            PromptDelivery::new("do the task".into(), true, ReadySignal::CursorShown, start);
        // Handshake round.
        assert_eq!(
            delivery.poll(seen(true, true, 0, false), start),
            Step::Waiting
        );
        // The show arrives: the clear burst first — 2·(1+8)−1 of each control
        // byte, the slack law against an unknown composer line — then the
        // envelope, in ONE write so nothing can land between them.
        let shown = start + SUBMIT_GAP;
        let Step::Write(paste) = delivery.poll(seen(true, true, 1, false), shown) else {
            panic!("the show did not produce the paste");
        };
        let burst = clear_input_for_text("");
        assert_eq!(burst.len(), 34, "the 2N−1 slack law moved");
        assert!(
            paste.starts_with(&burst) && paste[burst.len()..].starts_with(b"\x1b[200~"),
            "the paste no longer clears the composer line it lands on"
        );
        assert!(paste.ends_with(b"\x1b[201~"));
        assert!(
            !paste.ends_with(b"\r"),
            "the Enter was glued to the envelope"
        );
        assert!(delivery.pasted());
        // Inside the gap: nothing.
        assert_eq!(
            delivery.poll(seen(false, true, 1, false), shown + SUBMIT_GAP / 2),
            Step::Waiting
        );
        // Past it: the Enter, then done — and done stays done.
        assert_eq!(
            delivery.poll(seen(false, true, 1, false), shown + SUBMIT_GAP),
            Step::Submit(b"\r".to_vec())
        );
        assert_eq!(
            delivery.poll(seen(false, true, 1, false), shown + SUBMIT_GAP * 2),
            Step::Done(Outcome::Delivered)
        );
    }

    /// A launch owns a newly mounted, empty composer. Clearing it is both
    /// needless and unsafe: some TUIs insert Ctrl+U/Ctrl+K as literal text.
    #[test]
    fn a_launch_delivery_pastes_into_an_empty_composer_without_clearing() {
        let start = Instant::now();
        let mut delivery = PromptDelivery::new(
            "You are a worker".into(),
            true,
            ReadySignal::CursorShown,
            start,
        )
        .clearing(false);
        assert_eq!(
            delivery.poll(seen(true, true, 0, false), start),
            Step::Waiting
        );
        let Step::Write(bytes) = delivery.poll(seen(true, true, 1, false), start + SUBMIT_GAP)
        else {
            panic!("the ready composer did not receive its briefing");
        };
        assert!(
            bytes.starts_with(b"\x1b[200~You are a worker"),
            "a launch briefing did not begin with the paste envelope: {bytes:?}"
        );
        assert!(!bytes.contains(&0x15) && !bytes.contains(&0x0b));
    }

    /// The delivery obeys its caller's catalog decision: a running composer
    /// is cleared only for an agent whose editor consumes those keys.
    #[test]
    fn a_send_clears_only_where_the_catalog_says() {
        let bytes_for = |clearing| {
            let start = Instant::now();
            let mut delivery =
                PromptDelivery::new("next task".into(), true, ReadySignal::CursorShown, start)
                    .clearing(clearing);
            assert_eq!(
                delivery.poll(seen(true, true, 0, false), start),
                Step::Waiting
            );
            let Step::Write(bytes) = delivery.poll(seen(true, true, 1, false), start + SUBMIT_GAP)
            else {
                panic!("the ready composer did not receive the send");
            };
            bytes
        };

        assert_eq!(bytes_for(true).first(), Some(&0x15));
        assert!(bytes_for(false).starts_with(b"\x1b[200~next task"));
    }

    /// The clear law, exactly Orca's (`buildAgentTuiClearInput`): 2N−1 of
    /// each control byte, floor one line, ceiling forty, logical lines only.
    #[test]
    fn the_clear_burst_follows_the_two_n_minus_one_law() {
        assert_eq!(clear_input(1), [vec![0x15], vec![0x0b]].concat());
        assert_eq!(clear_input(0), clear_input(1), "zero lines under-floored");
        let nine = clear_input(9);
        assert_eq!(nine.len(), 34);
        assert!(nine[..17].iter().all(|byte| *byte == CLEAR_INPUT_LINE));
        assert!(nine[17..].iter().all(|byte| *byte == CLEAR_INPUT_FORWARD));
        assert_eq!(
            clear_input(100),
            clear_input(CLEAR_MAX_LINES),
            "a pathological draft emitted an unbounded burst"
        );
        for (text, lines) in [
            ("", 1),
            ("one", 1),
            ("a\nb", 2),
            ("a\r\nb", 2),
            ("a\rb", 2),
            ("a\r\n", 2),
            ("a\nb\r\nc\rd", 4),
        ] {
            assert_eq!(count_input_lines(text), lines, "{text:?}");
        }
        assert_eq!(clear_input_for_text(""), clear_input(1 + CLEAR_LINE_SLACK));
    }

    /// Without `submit`, the paste is the whole delivery.
    #[test]
    fn a_draft_without_submit_stops_at_the_paste() {
        let start = Instant::now();
        let mut delivery = PromptDelivery::new("draft".into(), false, ReadySignal::Quiet, start);
        delivery.poll(seen(true, true, 0, false), start);
        let quiet_after = start + QUIET + Duration::from_millis(1);
        let Step::Write(_) = delivery.poll(seen(false, true, 0, false), quiet_after) else {
            panic!("quiet did not produce the paste");
        };
        assert_eq!(
            delivery.poll(seen(false, true, 0, false), quiet_after + SUBMIT_GAP * 2),
            Step::Done(Outcome::Delivered),
            "an Enter was sent for a draft that asked for none"
        );
    }

    /// A timed-out delivery writes nothing at all.
    ///
    /// The measured shape: on timeout Orca checks once whether the expected
    /// process is really on the pty and otherwise aborts without sending
    /// (`pasteDraftWhenAgentReady`, agent-paste-draft-C1MiUIC6.js:291-315).
    /// We do not have the process check yet, so the conservative branch is
    /// the whole behaviour — reporting over blind typing.
    #[test]
    fn a_timed_out_delivery_writes_nothing() {
        let start = Instant::now();
        let mut delivery = PromptDelivery::with_deadlines(
            "never".into(),
            true,
            ReadySignal::CursorShown,
            start,
            QUIET,
            Duration::from_millis(100),
        );
        assert_eq!(
            delivery.poll(seen(true, false, 0, false), start),
            Step::Waiting
        );
        // Silent since: a program still writing is not late (see
        // `a_launch_wait_is_not_late_while_the_program_still_draws`).
        assert_eq!(
            delivery.poll(
                seen(false, false, 0, false),
                start + Duration::from_millis(100)
            ),
            Step::Done(Outcome::TimedOut)
        );
        assert!(!delivery.pasted());
    }

    /// grok's glyph is its own wherever the ALTERNATE SCREEN took the draw —
    /// before the handshake included, because the marker's anchor is the
    /// screen switch, not `?2004h` (the launch shell speaks 2004 too, and
    /// its prompt may be this very character).
    #[test]
    fn a_glyph_drawn_in_the_alt_screen_fires_with_or_without_the_handshake() {
        let start = Instant::now();
        let mut wait = Readiness::new(ReadySignal::AltScreenPrompt('❯'), start);
        // The shell's own `❯`, normal buffer: not grok's.
        assert_eq!(
            wait.observe(seen(true, false, 0, true), start),
            State::Waiting,
            "a shell prompt in the normal buffer fired the alt-anchored door"
        );
        // grok mounts its composer inside the alternate screen at ~0.6s —
        // Orca measures no handshake requirement on this path, and the logo
        // shimmer means quiet would never have settled.
        let drawn = Observed {
            marker_in_alt: true,
            alt_screen: true,
            ..seen(true, false, 0, true)
        };
        assert_eq!(
            wait.observe(drawn, start + QUIET / 2),
            State::Ready,
            "the composer glyph inside the alt screen did not fire pre-handshake"
        );

        // And after the handshake it is the same door, same answer.
        let mut shaken = Readiness::new(ReadySignal::AltScreenPrompt('❯'), start);
        shaken.observe(seen(true, true, 0, false), start);
        assert_eq!(
            shaken.observe(
                Observed {
                    marker_in_alt: true,
                    alt_screen: true,
                    ..seen(true, true, 0, true)
                },
                start + QUIET / 4
            ),
            State::Ready
        );
    }

    /// An inline grok (`--no-alt-screen`) never switches screens, so the
    /// glyph door never opens — the 2004-anchored quiet window is the floor
    /// that keeps that launch on a delivery path at all (Orca's why for the
    /// signal's TWO anchors, draft-paste-ready-scanner.ts:57-63).
    #[test]
    fn an_inline_launch_falls_to_the_quiet_floor() {
        let start = Instant::now();
        let mut wait = Readiness::new(ReadySignal::AltScreenPrompt('❯'), start);
        assert_eq!(
            wait.observe(seen(true, true, 0, false), start),
            State::Waiting
        );
        assert_eq!(
            wait.observe(seen(false, true, 0, false), start + QUIET / 2),
            State::Waiting,
            "half the quiet window was enough"
        );
        assert_eq!(
            wait.observe(seen(false, true, 0, false), start + QUIET),
            State::Ready
        );
    }

    /// A composer drawn BEFORE the handshake is remembered — if it was drawn
    /// inside the alternate screen and the program has not left it since.
    ///
    /// Orca's `scanCodexPreAnchorPrompt`: an idle codex that mounted its
    /// composer before enabling `?2004h` never redraws it after, and a wait
    /// that only reads post-handshake rounds starves for the whole budget.
    /// Leaving the screen revokes the memory: the terminal went back to the
    /// shell, and a `›` after that is anybody's.
    #[test]
    fn a_composer_drawn_before_the_handshake_is_remembered_until_the_screen_is_left() {
        let start = Instant::now();
        let mut wait = Readiness::new(ReadySignal::Prompt(COMPOSER_PROMPT), start);
        let mounted = Observed {
            marker_in_alt: true,
            alt_screen: true,
            ..seen(true, false, 0, true)
        };
        assert_eq!(wait.observe(mounted, start), State::Waiting);
        // The handshake arrives with nothing else in it: the remembered
        // mount answers NOW — this is the one round the plain glyph rule
        // deliberately refuses, and the alt-screen anchor is what makes
        // accepting it safe (scrollback lives in the normal buffer).
        assert_eq!(
            wait.observe(
                Observed {
                    alt_screen: true,
                    ..seen(false, true, 0, false)
                },
                start + QUIET / 4
            ),
            State::Ready,
            "the pre-handshake mount was forgotten and the wait starves"
        );

        // Same mount, but the program leaves the alternate screen before the
        // handshake: memory revoked, the handshake round stays a waiting one.
        let mut left = Readiness::new(ReadySignal::Prompt(COMPOSER_PROMPT), start);
        left.observe(mounted, start);
        left.observe(seen(true, false, 0, false), start + QUIET / 8);
        assert_eq!(
            left.observe(
                Observed {
                    alt_screen: true,
                    ..seen(false, true, 0, false)
                },
                start + QUIET / 4
            ),
            State::Waiting,
            "a revoked mount still answered for the handshake round"
        );
    }

    /// The hard deadline is the agent's own — Orca's
    /// `resolveDraftPasteReadyTimeoutMs` gives codex 20s where the shared
    /// A program still drawing has not failed to be ready — it has not
    /// finished. A session resumed from a long transcript replays it for
    /// longer than the shared deadline without a pause, and the launch wait
    /// used to end at `started + timeout` regardless, dropping the briefing
    /// while it was visibly still arriving (live report 2026-08-30, "터미널
    /// 3의 에이전트가 프롬프트를 받지 못했습니다"). The deadline is on
    /// silence: measured from the last write.
    #[test]
    fn a_launch_wait_is_not_late_while_the_program_still_draws() {
        let start = Instant::now();
        let timeout = Duration::from_millis(100);
        let quiet = Duration::from_millis(30);
        let mut wait = Readiness::with_deadlines(ReadySignal::Quiet, start, quiet, timeout);
        // The handshake, then a write every 20 ms for three deadlines' worth.
        assert_eq!(
            wait.observe(seen(true, true, 0, false), start),
            State::Waiting
        );
        let mut now = start;
        while now < start + timeout * 3 {
            now += Duration::from_millis(20);
            assert_eq!(
                wait.observe(seen(true, true, 0, false), now),
                State::Waiting,
                "a program that wrote {:?} ago was given up on",
                Duration::from_millis(20)
            );
        }
        // Then it rests, and the rest is what readiness was waiting for.
        assert_eq!(
            wait.observe(seen(false, true, 0, false), now + quiet),
            State::Ready
        );
    }

    /// …but not forever. A program that never rests — a TUI whose spinner
    /// redraws until the end of time — is given up on at [`PATIENCE`], so a
    /// caller waiting on the answer is answered.
    #[test]
    fn a_launch_wait_still_gives_up_on_a_program_that_never_rests() {
        let start = Instant::now();
        let mut wait = Readiness::new(ReadySignal::Quiet, start);
        assert_eq!(
            wait.observe(seen(true, true, 0, false), start),
            State::Waiting
        );
        assert_eq!(
            wait.observe(seen(true, true, 0, false), start + TIMEOUT * 4),
            State::Waiting,
            "the shared deadline ended a wait on a program that was still drawing"
        );
        assert_eq!(
            wait.observe(seen(true, true, 0, false), start + PATIENCE),
            State::TimedOut
        );
    }

    /// The send road keeps the shared clock. A running agent asked to take
    /// a prompt while it works IS the case that deadline was written for,
    /// and the caller waiting on the answer must not wait out `PATIENCE`.
    #[test]
    fn a_send_to_a_working_agent_keeps_the_shared_deadline() {
        let start = Instant::now();
        let mut wait = Readiness::new(ReadySignal::Rest(None), start);
        assert_eq!(
            wait.observe(seen(true, true, 0, false), start),
            State::Waiting
        );
        assert_eq!(
            wait.observe(seen(true, true, 0, false), start + TIMEOUT),
            State::TimedOut
        );
    }

    /// default is 8 — and a delivery built with it must survive the shared
    /// deadline and still end at its own.
    #[test]
    fn the_deadline_is_the_agents_own_not_the_shared_eight_seconds() {
        let start = Instant::now();
        let twenty = Duration::from_millis(20_000);
        let mut delivery = PromptDelivery::with_timeout(
            "cold codex start".into(),
            true,
            ReadySignal::Prompt(COMPOSER_PROMPT),
            start,
            twenty,
        );
        assert_eq!(
            delivery.poll(seen(true, true, 0, false), start),
            Step::Waiting
        );
        assert_eq!(
            delivery.poll(seen(false, true, 0, false), start + TIMEOUT + QUIET),
            Step::Waiting,
            "the shared eight seconds still ended a wait the agent was given twenty for"
        );
        assert_eq!(
            delivery.poll(seen(false, true, 0, false), start + twenty),
            Step::Done(Outcome::TimedOut)
        );
    }

    /// The guard is decided at the WRITE, not at registration.
    ///
    /// A delivery registered against an empty line waits out its readiness;
    /// by the time the paste comes due, a person has typed. The words must
    /// not land on theirs, so nothing is written and the delivery settles
    /// as refused — with the words still the caller's to place.
    #[test]
    fn a_draft_on_the_line_when_the_paste_comes_due_refuses_the_whole_delivery() {
        let start = Instant::now();
        let mut delivery = PromptDelivery::new(
            "mail is waiting".into(),
            true,
            ReadySignal::CursorShown,
            start,
        )
        .clearing(false)
        .guarded(Guard::for_somebody_elses_line(None));
        // Registered against an untouched line…
        assert_eq!(
            delivery.poll_line(seen(true, true, 0, false), Line::default(), start),
            Step::Waiting
        );
        // …and by the ready round somebody has typed.
        let typed = Line {
            draft: true,
            hand: Some(1),
            ..Line::default()
        };
        assert_eq!(
            delivery.poll_line(seen(true, true, 1, false), typed, start + SUBMIT_GAP),
            Step::Done(Outcome::Refused(Refusal::HoldsADraft))
        );
        assert!(!delivery.pasted());
        assert_eq!(delivery.text(), "mail is waiting");
        // And it stays refused: no later round writes anything.
        assert_eq!(
            delivery.poll_line(
                seen(true, true, 2, false),
                Line::default(),
                start + QUIET * 2
            ),
            Step::Done(Outcome::Refused(Refusal::HoldsADraft))
        );
        assert!(!Outcome::Refused(Refusal::HoldsADraft).pasted());
    }

    /// A hand between the paste and the Enter withholds the Enter — in BOTH
    /// shapes, including the one that leaves no draft behind: the person's
    /// own Enter closing over the half-pasted words empties the line, and
    /// only the hand count remembers they were there.
    #[test]
    fn a_hand_after_the_paste_withholds_the_enter_and_writes_no_cr() {
        for (name, after) in [
            (
                "typing",
                Line {
                    draft: true,
                    hand: Some(8),
                    ..Line::default()
                },
            ),
            (
                "typed and sent",
                Line {
                    draft: false,
                    hand: Some(9),
                    ..Line::default()
                },
            ),
        ] {
            let start = Instant::now();
            let mut delivery =
                PromptDelivery::new("check".into(), true, ReadySignal::CursorShown, start)
                    .clearing(false)
                    .guarded(Guard::for_somebody_elses_line(None));
            let before = Line {
                hand: Some(7),
                ..Line::default()
            };
            delivery.poll_line(seen(true, true, 0, false), before, start);
            let Step::Write(paste) =
                delivery.poll_line(seen(true, true, 1, false), before, start + SUBMIT_GAP)
            else {
                panic!("{name}: the paste did not go in");
            };
            assert!(!paste.contains(&b'\r'), "{name}: an Enter rode the paste");
            assert!(delivery.pasted());
            assert_eq!(
                delivery.poll_line(seen(false, true, 1, false), after, start + SUBMIT_GAP * 2),
                Step::Done(Outcome::Unsubmitted(Refusal::HandReached)),
                "{name}"
            );
            assert!(Outcome::Unsubmitted(Refusal::HandReached).pasted());
        }
        // The same hand count either side is nobody's hand: the Enter goes.
        let start = Instant::now();
        let mut delivery =
            PromptDelivery::new("check".into(), true, ReadySignal::CursorShown, start)
                .clearing(false)
                .guarded(Guard::for_somebody_elses_line(None));
        let still = Line {
            hand: Some(7),
            ..Line::default()
        };
        delivery.poll_line(seen(true, true, 0, false), still, start);
        delivery.poll_line(seen(true, true, 1, false), still, start + SUBMIT_GAP);
        assert_eq!(
            delivery.poll_line(seen(false, true, 1, false), still, start + SUBMIT_GAP * 2),
            Step::Submit(b"\r".to_vec())
        );
    }

    /// A question or an approval parked on the pane refuses either write —
    /// and, between the two, leaves the words as a draft rather than
    /// answering the question with them.
    #[test]
    fn a_parked_question_refuses_the_paste_and_withholds_the_enter() {
        let start = Instant::now();
        let parked = Line {
            parked: true,
            ..Line::default()
        };
        let mut before = PromptDelivery::new("go".into(), true, ReadySignal::CursorShown, start)
            .guarded(Guard::for_somebody_elses_line(None));
        before.poll_line(seen(true, true, 0, false), Line::default(), start);
        assert_eq!(
            before.poll_line(seen(true, true, 1, false), parked, start + SUBMIT_GAP),
            Step::Done(Outcome::Refused(Refusal::Parked))
        );

        let mut between = PromptDelivery::new("go".into(), true, ReadySignal::CursorShown, start)
            .guarded(Guard::for_somebody_elses_line(None));
        between.poll_line(seen(true, true, 0, false), Line::default(), start);
        assert!(matches!(
            between.poll_line(
                seen(true, true, 1, false),
                Line::default(),
                start + SUBMIT_GAP
            ),
            Step::Write(_)
        ));
        assert_eq!(
            between.poll_line(seen(false, true, 1, false), parked, start + SUBMIT_GAP * 2),
            Step::Done(Outcome::Unsubmitted(Refusal::Parked))
        );
    }

    /// The launch the words were addressed to is the launch they are typed
    /// at. A pane relaunched under the delivery — the words meant for the
    /// last occupant — refuses at whichever write finds the change.
    #[test]
    fn a_relaunched_pane_is_not_typed_at_by_a_delivery_addressed_to_its_last_occupant() {
        let start = Instant::now();
        let first = Line {
            launch: Some(41),
            ..Line::default()
        };
        let second = Line {
            launch: Some(42),
            ..Line::default()
        };
        let mut delivery =
            PromptDelivery::new("resume".into(), true, ReadySignal::CursorShown, start)
                .guarded(Guard::for_its_own_line(Some(41)));
        delivery.poll_line(seen(true, true, 0, false), first, start);
        assert_eq!(
            delivery.poll_line(seen(true, true, 1, false), second, start + SUBMIT_GAP),
            Step::Done(Outcome::Refused(Refusal::LaunchChanged))
        );
        // Its own line means hands and parked questions are not consulted:
        // the launch briefing's composer is the window's own.
        let mut own = PromptDelivery::new("brief".into(), true, ReadySignal::CursorShown, start)
            .guarded(Guard::for_its_own_line(Some(41)));
        let busy = Line {
            draft: true,
            hand: Some(1),
            parked: true,
            launch: Some(41),
        };
        own.poll_line(seen(true, true, 0, false), busy, start);
        assert!(matches!(
            own.poll_line(seen(true, true, 1, false), busy, start + SUBMIT_GAP),
            Step::Write(_)
        ));
        let moved = Line {
            hand: Some(2),
            ..busy
        };
        assert_eq!(
            own.poll_line(seen(false, true, 1, false), moved, start + SUBMIT_GAP * 2),
            Step::Submit(b"\r".to_vec())
        );
    }

    /// The default guard refuses nothing, so a driver with no line facts —
    /// `poll` — keeps every delivery it always had.
    #[test]
    fn an_unguarded_delivery_writes_whatever_the_line_holds() {
        let start = Instant::now();
        let mut delivery =
            PromptDelivery::new("next".into(), true, ReadySignal::CursorShown, start);
        assert_eq!(delivery.guard(), Guard::default());
        let busy = Line {
            draft: true,
            hand: Some(1),
            parked: true,
            launch: Some(9),
        };
        delivery.poll_line(seen(true, true, 0, false), busy, start);
        assert!(matches!(
            delivery.poll_line(seen(true, true, 1, false), busy, start + SUBMIT_GAP),
            Step::Write(_)
        ));
        assert_eq!(
            delivery.poll_line(
                seen(false, true, 1, false),
                Line {
                    hand: Some(2),
                    ..busy
                },
                start + SUBMIT_GAP * 2
            ),
            Step::Submit(b"\r".to_vec())
        );
    }

    /// Every refusal says why, in words a receipt can carry.
    #[test]
    fn every_refusal_says_what_it_protected() {
        for refusal in Refusal::ALL {
            assert!(!refusal.says().trim().is_empty(), "{refusal:?}");
        }
        assert!(Refusal::HoldsADraft.says().contains("appended to them"));
        assert!(Refusal::HandReached.says().contains("left unsubmitted"));
    }

    /// Every refusal has one token of its own, and a token is a name — the
    /// window's table is keyed by it, and a sentence there would be a second
    /// copy of what the receipt already says (t-10159).
    #[test]
    fn every_refusal_has_one_token_the_window_can_key() {
        let tokens = Refusal::ALL.map(Refusal::token);
        for (refusal, token) in Refusal::ALL.into_iter().zip(tokens) {
            assert!(
                !token.is_empty()
                    && token
                        .bytes()
                        .all(|byte| byte.is_ascii_lowercase() || byte == b'_'),
                "{refusal:?} is named by something other than a token: {token}"
            );
            assert_ne!(token, refusal.says(), "{refusal:?}");
        }
        let distinct: std::collections::HashSet<&str> = tokens.into_iter().collect();
        assert_eq!(distinct.len(), tokens.len(), "two refusals share a token");
        assert_eq!(Refusal::HoldsADraft.token(), "holds_a_draft");
    }
}
