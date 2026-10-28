//! A PTY-hosted lane: one child process, one terminal grid.
//!
//! This is the terminal surface of the shell IDE. The child is usually
//! `zo attach <session>`, but nothing here knows that — it hosts any command,
//! which is also why the IDE still has a working terminal on a machine with no
//! `zo` installed.
//!
//! Output is read on a dedicated thread because PTY reads block, and handed to
//! the caller through a channel. Input LEAVES on a thread of its own for the
//! same reason — a pty write blocks while the child is not reading, and the
//! caller here is the window's own thread. The grid is only ever touched from
//! whoever calls [`PtyLane::pump`], so there is no lock around screen state.

use std::io::{ErrorKind, Read, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel, sync_channel};
use std::time::Instant;

#[cfg(not(target_os = "macos"))]
use portable_pty::CommandBuilder;
use portable_pty::{MasterPty, PtySize, native_pty_system};

use crate::answer::Unanswered;
use crate::echo_shapes::ExpectedEchoes;
use crate::grid::{TERMINAL_NAME, TERMINAL_VERSION, Terminal};

#[derive(Debug, thiserror::Error)]
pub enum PtyError {
    #[error("could not open a pty: {0}")]
    Open(String),
    // The field is `reason`, not `source`: thiserror treats a field named
    // `source` as the error cause and requires it to implement `Error`.
    #[error("could not start {program}: {reason}")]
    Spawn { program: String, reason: String },
    #[error("pty io: {0}")]
    Io(#[from] std::io::Error),
}

/// Maximum output bytes a single pump moves into a grid.
///
/// A pump runs on the thread that draws, so an unbounded drain hands one
/// flooding lane the whole round: a `yes` in a pane nobody is looking at
/// stops the window answering anybody. Nothing is dropped, only deferred —
/// what is left waits in the channel, and the delta this pump did produce
/// keeps the round from counting as quiet, so the next frame comes for it
/// straight away.
///
/// One number for local and remote lanes alike
/// (`zerocode_ssh::PTY_OUTPUT_BYTE_BUDGET` is this one, re-exported), because
/// "how much output may one pump move" has the same answer either side of a
/// network. Remote lanes additionally spend it as the memory their decode
/// queue may own, which is the same bound seen from the producing end.
pub const PTY_OUTPUT_BYTE_BUDGET: usize = 256 * 1024;

/// One PTY read, and therefore one queued output chunk, is at most this many
/// bytes — the reader thread's own buffer.
pub const PTY_READ_CHUNK_BYTES: usize = 8 * 1024;

/// How many whole pump budgets the local output queue will hold for a pump
/// that has not come yet.
///
/// Two, not an arbitrary depth: one budget so a pump can always drain a full
/// round without the reader being the bottleneck, and a second for the reader
/// to refill while the grid parses the first — double buffering, and nothing
/// past it. More would only let a flooding child own more of this process's
/// memory for output nobody has looked at.
const LOCAL_OUTPUT_QUEUE_BUDGETS: usize = 2;

/// Capacity of the local output queue, in chunks.
///
/// The queue used to be unbounded, and that was a memory promise made to
/// every flooding child: a `yes` in a pane nobody pumps quickly enough grew
/// the queue without limit. Bounded, the reader thread blocks once the queue
/// holds [`LOCAL_OUTPUT_QUEUE_BUDGETS`] pump budgets, the kernel's own pty
/// buffer fills behind it, and the child's next write waits — the same
/// backpressure a real terminal applies, and nothing is dropped, only
/// deferred. EOF still arrives: the reader keeps reading the moment the
/// queue drains, and the closed pty ends the thread whose exit is what
/// [`PtyLane::pump`] reports as `ended`.
const LOCAL_OUTPUT_QUEUE_CHUNKS: usize =
    LOCAL_OUTPUT_QUEUE_BUDGETS * PTY_OUTPUT_BYTE_BUDGET / PTY_READ_CHUNK_BYTES;

/// What [`PtyLane::pump`] observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pumped {
    /// Bytes moved from the child into the grid this call.
    pub bytes: usize,
    /// The child closed its side. No more output will arrive.
    pub ended: bool,
    /// This call parsed the child's answer to a write: the first output that
    /// arrived after it ([`crate::answer`]). A transport that cannot tell says
    /// `false`.
    pub answered: bool,
    /// The earliest write the child has still said nothing after, if one is
    /// waiting. `None` from a transport that does not keep count.
    pub unanswered_since: Option<Instant>,
}

/// What a parent agent stamps on its own process, and must not hand down.
///
/// This window is very often started FROM an agent — that is most of what it
/// is for — and a pty inherits the environment of whatever spawned it. So
/// every shell this app opens was being handed the marks of the session that
/// opened the app, and the agent inside it read them as its own: the reported
/// symptom was `inherited CLAUDE_CODE_CHILD_SESSION marker`, but the same
/// inheritance also points a child at the parent's IPC endpoints and tells it
/// it is running inside a Claude Code session it has nothing to do with.
///
/// Three groups, and each is a different way to be wrong:
///
/// - **Session stamps.** Orca's own list, verbatim
///   (`CLAUDE_CHILD_SESSION_STAMP_ENV_KEYS`, out/main/index.js:41721-41725).
///   A child wearing these claims to be a continuation of a conversation it
///   was never part of.
/// - **"You are inside Claude Code".** A shell that is not one says it is, and
///   tools that branch on it take the wrong branch.
/// - **The parent's own sockets and paths.** The worst of the three: a child
///   that dials these is talking to a DIFFERENT process about work it is not
///   doing.
///
/// `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS` is here for a reason of ours: with
/// it set and no shim on PATH, a launched agent execs the machine's real
/// `tmux` (1-el). When this window means it, it sets it — and setting it is
/// exactly what protects it below.
///
/// NOT here: `CLAUDE_CODE_OAUTH_TOKEN` and the other credentials, which the
/// account switch already clears deliberately per launch and which are not
/// session identity; and `TMUX`/`TMUX_PANE`, because somebody running this
/// window from inside tmux is a person doing an ordinary thing.
pub const INHERITED_SESSION_MARKERS: &[&str] = &[
    // Session stamps — Orca's three.
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_BRIDGE_SESSION_ID",
    // "This process is a Claude Code session."
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS",
    // The parent's own channels back to itself.
    "CLAUDE_CODE_SSE_PORT",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_EXECPATH",
];

/// What this terminal answers to, when the launch does not say otherwise.
///
/// One place, because there is one terminal: every pane and every background
/// scan comes through this door, and the answer is a property of the emulator
/// on the other side of it rather than of any particular caller.
///
/// `TERM_PROGRAM` is part of that answer, not a detail: programs branch on it.
/// Inherited from a window opened out of Terminal.app it said `Apple_Terminal`
/// in every pane (measured 2026-09-17), and Claude Code 2.1.273 in fullscreen
/// then polled `CSI ? 6 n` every 200 ms for a Cmd+K this window never sends,
/// skipped its synchronized-output probe, and dropped strikethrough; zsh ran
/// `/etc/zshrc_Apple_Terminal` for a Terminal.app session that was not there.
/// The name and version are the ones the grid gives when asked (XTVERSION).
pub const DECLARED_TERMINAL: &[(&str, &str)] = &[
    ("TERM", "xterm-256color"),
    ("COLORTERM", "truecolor"),
    ("TERM_PROGRAM", TERMINAL_NAME),
    ("TERM_PROGRAM_VERSION", TERMINAL_VERSION),
];

/// How the terminal that opened this window names its own session, and must
/// not name a pane's.
///
/// `TERM_PROGRAM` above is overwritten; these have no value this terminal
/// could truthfully give, so an inherited one is taken out. Each is read by
/// something that runs in a pane: `TERM_SESSION_ID` by macOS's
/// `/etc/zshrc_Apple_Terminal` (the per-session history file), and the rest by
/// Claude Code 2.1.273's terminal detection — which also reads
/// `LC_TERMINAL=iTerm2` as a second way to decide it is inside iTerm2.
///
/// Stripped only when INHERITED, by the same rule as the lists around it.
/// `TMUX` and `STY` are not here: a multiplexer the person runs this window
/// inside is a real layer between the pane and the screen.
pub const INHERITED_TERMINAL_IDENTITY: &[&str] = &[
    // Terminal.app and iTerm2.
    "TERM_SESSION_ID",
    "ITERM_SESSION_ID",
    "LC_TERMINAL",
    "LC_TERMINAL_VERSION",
    // Other emulators and IDE terminals, as the agents detect them.
    "KITTY_WINDOW_ID",
    "ALACRITTY_LOG",
    "GHOSTTY_RESOURCES_DIR",
    "KONSOLE_VERSION",
    "VTE_VERSION",
    "WT_SESSION",
    "TERMINAL_EMULATOR",
    "ZED_TERM",
    "CURSOR_TRACE_ID",
];

/// What a program is told about colour is THIS terminal's to say.
///
/// A pane is a real terminal on a real pty, and it announces itself as one:
/// `TERM=xterm-256color` and `COLORTERM=truecolor` go out with every launch. But
/// these four are the overrides that beat both, and a window inherits whatever
/// set them — so whoever happened to start this app got to decide, for every
/// pane, forever.
///
/// That is not hypothetical. This window was started from a tool session that
/// exports `NO_COLOR=1`, and every agent in it went monochrome: the saved
/// scrollback held not one SGR sequence, only the OSC 8 hyperlinks the same
/// programs kept emitting, because a well-behaved CLI honours `NO_COLOR`
/// absolutely. On screen it read as our own renderer losing the palette
/// ("터미널 색상이 이상해") while the emulator, the wire and the palette were
/// all correct and simply had nothing to colour.
///
/// Stripped only when INHERITED — the same rule, and for the same reason, as
/// the session markers above: a caller with a reason to force one of these
/// still wins, because the removals land before the caller's own entries.
///
/// Windows carries these too. They are conventions of the program, not of the
/// platform, so this list is not `cfg`-gated.
pub const INHERITED_COLOUR_OVERRIDES: &[&str] = &[
    // The standard (no-color.org): present and non-empty disables colour.
    "NO_COLOR",
    // npm/`supports-color`'s counterpart. `FORCE_COLOR=0` disables just as
    // firmly as `NO_COLOR`, so it cannot be kept just because it says "force".
    "FORCE_COLOR",
    // The BSD/macOS pair much of coreutils and Go tooling reads.
    "CLICOLOR",
    "CLICOLOR_FORCE",
];

/// How much unanswered input a lane will hold for a child that has stopped
/// reading, before it says so instead of growing.
///
/// Generous on purpose: this is not a rate limit, it is the line past which a
/// child is not "slow" but gone. Four megabytes is far more than a person can
/// type or paste and far less than a window can afford to keep per lane.
const PENDING_INPUT_BUDGET: usize = 4 * 1024 * 1024;

/// A running child plus the screen it is drawing on.
pub struct PtyLane {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    input: Sender<Vec<u8>>,
    /// Bytes handed to the writer thread and not yet written. Shared with that
    /// thread, which is the half that knows when they leave.
    pending: Arc<AtomicUsize>,
    /// What the child wrote, each chunk with when the reader thread took it
    /// off the pty — the clock a write's answer is judged by
    /// ([`crate::answer`]), which a parse that comes later cannot keep.
    output: Receiver<(Instant, Vec<u8>)>,
    /// The answers this terminal has given that the tty may yet hand back.
    /// Empty is the state a lane spends almost all of its life in, and an
    /// empty one costs a read nothing at all.
    echoes: ExpectedEchoes,
    terminal: Terminal,
    ended: bool,
    /// When the child last wrote anything — the one fact that tells a shell
    /// still at work from one that has stopped, for a caller whose wait on
    /// it ran out.
    last_output_at: Option<Instant>,
    /// The same moment on the wall clock, in epoch milliseconds — for a
    /// reader that keys a silence by its start and must read the same
    /// number on every beat. Deriving it later as `now − elapsed` does not
    /// give that: measured 2026-09-21, eight beats a second apart read eight
    /// starts spread over 21 ms, and the ledger told the same silence eight
    /// times. Written in the same breath as `last_output_at`, read as is.
    last_output_epoch_ms: Option<i64>,
    /// The writes the child has not answered yet ([`crate::answer`]).
    unanswered: Unanswered,
    /// Every test-owned write, including automatic terminal replies.
    #[cfg(test)]
    input_tape: Vec<Vec<u8>>,
}

impl PtyLane {
    /// Spawn `program` with `args` in a PTY of `rows` × `cols`.
    ///
    /// `env` entries are applied on top of the inherited environment, with an
    /// EMPTY value meaning "remove this one" rather than "set it to nothing".
    /// The distinction matters to anything that tests presence, and it is what
    /// lets a caller clear a variable it must not merely blank. Which is how a
    /// lane carries its pane key and hook token to the child, and how an
    /// account switch clears the credentials that would override it.
    pub fn spawn(
        program: &str,
        args: &[String],
        cwd: Option<&Path>,
        env: &[(String, String)],
        rows: u16,
        cols: u16,
    ) -> Result<Self, PtyError> {
        let size = PtySize {
            rows: rows.max(1),
            cols: cols.max(1),
            pixel_width: 0,
            pixel_height: 0,
        };
        let pair = open_pty(|| native_pty_system().openpty(size))?;

        // What changes in the inherited environment, in order: a value, or
        // `None` to take the name out. Applied by whichever start the
        // platform uses (see `start_child`).
        let mut edits: Vec<(&str, Option<&str>)> = Vec::new();
        // An EMPTY value means "take this one out of the child's environment",
        // not "set it to nothing". The two differ for anything that tests
        // presence rather than truthiness — and the caller that needs this is
        // the account switch, which has to clear the credential variables that
        // would otherwise override the config directory it just named. Setting
        // them empty would leave them present.
        // The marks of whatever session started this window go no further —
        // but only the ones this launch is not itself setting.
        //
        // That condition is the whole rule, and it is Orca's
        // (`getInheritedClaudeSessionStampEnvKeysToDelete`,
        // out/main/index.js:42153-42156): strip what was INHERITED, never what
        // was chosen. A blind delete list would take the team marker back out
        // of the one launch that means it, and would fight every future caller
        // that has a reason to set one of these on purpose.
        //
        // Before the caller's own entries rather than after, so a launch that
        // sets one of these still wins — the removal cannot land on top of a
        // value that was asked for.
        //
        // The colour overrides and the other terminal's session identity ride
        // the same rule for the same reason: what a program is told about the
        // terminal belongs to the terminal it is running in, not to whatever
        // started the terminal.
        for marker in INHERITED_SESSION_MARKERS
            .iter()
            .chain(INHERITED_COLOUR_OVERRIDES)
            .chain(INHERITED_TERMINAL_IDENTITY)
        {
            if !env.iter().any(|(key, _)| key == marker) {
                edits.push((marker, None));
            }
        }
        // And what this terminal IS, said by the terminal.
        //
        // Inheriting these is the same mistake in the other direction: a window
        // opened from inside tmux handed every pane `TERM=screen`, so panes that
        // are not tmux introduced themselves as tmux and programs dropped to the
        // capabilities of a multiplexer nobody was running. The grid this feeds
        // is an xterm with a 256-colour palette and truecolor SGR (`grid.rs`
        // parses `38;2;r;g;b`), so that is what it says it is.
        //
        // Defaults, not decrees — set before the caller's entries, so a launch
        // with a reason to say something else still says it.
        for (key, value) in DECLARED_TERMINAL {
            if !env.iter().any(|(named, _)| named == key) {
                edits.push((key, Some(value)));
            }
        }
        for (key, value) in env {
            if value.is_empty() {
                edits.push((key, None));
            } else {
                edits.push((key, Some(value)));
            }
        }

        let child = start_child(&pair, program, args, cwd, &edits)?;
        // The slave handle must be dropped or the reader never sees EOF after
        // the child exits — the kernel keeps the pty open on our behalf.
        drop(pair.slave);

        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| PtyError::Open(error.to_string()))?;
        let mut writer = pair
            .master
            .take_writer()
            .map_err(|error| PtyError::Open(error.to_string()))?;

        // Bounded (see [`LOCAL_OUTPUT_QUEUE_CHUNKS`]): a full queue parks
        // this thread in `send` rather than growing, which walks the flood
        // back through the kernel's pty buffer to the child's own `write`.
        // A receiver that went away — the lane was dropped mid-flood — fails
        // the send and ends the thread, exactly as the closed pty does.
        let (tx, output) = sync_channel::<(Instant, Vec<u8>)>(LOCAL_OUTPUT_QUEUE_CHUNKS);
        std::thread::spawn(move || {
            let mut buffer = [0u8; PTY_READ_CHUNK_BYTES];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(count) => {
                        if tx.send((Instant::now(), buffer[..count].to_vec())).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        // Writes leave on a thread because a pty write BLOCKS while the child
        // is not reading its input, and the thread that calls `write_input` is
        // the one drawing the window. Measured live, twice in one hour: the
        // main thread's stack ended in `write` inside `PtyLane::write_input`
        // for the whole of a two-second sample while every pane sat frozen —
        // eight lanes taken down because one child paused between reads.
        //
        // Order is the channel's, so a keystroke cannot overtake the paste
        // before it, and the answer to a query cannot overtake either.
        let (input, queued) = channel::<Vec<u8>>();
        let pending = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&pending);
        std::thread::spawn(move || {
            while let Ok(bytes) = queued.recv() {
                let wrote = writer.write_all(&bytes).and_then(|()| writer.flush());
                counted.fetch_sub(bytes.len(), Ordering::Relaxed);
                if wrote.is_err() {
                    break;
                }
            }
        });

        Ok(Self {
            master: pair.master,
            child,
            input,
            pending,
            output,
            echoes: ExpectedEchoes::default(),
            terminal: Terminal::new(rows as usize, cols as usize),
            ended: false,
            last_output_at: None,
            last_output_epoch_ms: None,
            unanswered: Unanswered::default(),
            #[cfg(test)]
            input_tape: Vec::new(),
        })
    }

    /// Drain whatever the child has produced into the grid.
    ///
    /// Non-blocking: the caller decides the cadence, which is what keeps eight
    /// streaming lanes from each pinning a thread on the render path.
    ///
    /// Bounded by [`PTY_OUTPUT_BYTE_BUDGET`] as well, since "non-blocking" is
    /// not the same promise as "brief" while a child is producing faster than
    /// a grid can take it. Overshoot is a single chunk, and a chunk is the
    /// reader thread's buffer.
    pub fn pump(&mut self) -> Pumped {
        self.pump_with_budget(PTY_OUTPUT_BYTE_BUDGET)
    }

    /// [`Self::pump`] with the caller's own byte budget — the same single
    /// loop, which `pump` delegates to whole. Exists so a test can turn the
    /// drain in small deterministic steps; production callers use `pump`
    /// and the one measured budget.
    pub fn pump_with_budget(&mut self, budget: usize) -> Pumped {
        let mut bytes = 0;
        let mut answered = false;
        while bytes < budget {
            match self.output.try_recv() {
                Ok((arrived, chunk)) => {
                    bytes += chunk.len();
                    answered |= self.unanswered.arrived(arrived);
                    // What the child wrote, with this terminal's own answers
                    // taken back out of it: a line discipline in ECHO, or a
                    // line editor redrawing its input, hands them straight
                    // back as ordinary output. See [`crate::echo_shapes`].
                    let shown = self.echoes.sift(&chunk);
                    self.terminal.feed(&shown);
                    self.last_output_at = Some(Instant::now());
                    self.last_output_epoch_ms = Some(epoch_ms_now());
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.ended = true;
                    break;
                }
            }
        }
        // Some sequences are questions, and the answers have to reach the
        // child or it waits until its own timeout — which is what "the UI
        // never appeared" and "the app feels slow" both look like. Taken
        // here rather than at parse time because the grid has no handle on
        // the pty, and taken after feeding so a query in this chunk is
        // answered in the same pump.
        let replies = self.terminal.grid_mut().take_replies();
        if !replies.is_empty() {
            // Watched for before it is written, because nothing says the echo
            // cannot be read back before the write call has returned. And
            // queued rather than written as input: nobody waits for the
            // program to answer the terminal's own answer.
            self.echoes.expect(&replies);
            let _ = self.queue_input(&replies);
        }
        Pumped {
            bytes,
            ended: self.ended,
            answered,
            unanswered_since: self.unanswered.since(),
        }
    }

    /// Send keystrokes (or pasted text) to the child.
    ///
    /// Handed to this lane's writer thread rather than written here, and so it
    /// returns at the speed of a channel send whatever the child is doing. The
    /// bytes are still delivered, in order, the moment the child reads again —
    /// a child that pauses now costs its own pane a pause and no other pane
    /// anything at all.
    ///
    /// The write then waits for the child's answer: the clock is read before
    /// the bytes leave, so nothing the child says in reply can look older than
    /// the write it answers ([`Pumped::unanswered_since`]).
    ///
    /// # Errors
    ///
    /// [`PtyError::Io`] once the child has stopped taking input: either the
    /// write failed, which is what a child that exited looks like from here,
    /// or it has ignored `PENDING_INPUT_BUDGET` bytes and the queue will not
    /// grow further to spare it. One write can still be accepted after the
    /// child is gone — the failure is discovered by the thread that writes —
    /// and every write after that reports it.
    pub fn write_input(&mut self, bytes: &[u8]) -> Result<(), PtyError> {
        let sent = Instant::now();
        self.queue_input(bytes)?;
        self.unanswered.sent(sent);
        Ok(())
    }

    /// Hand bytes to the writer thread: a person's write
    /// ([`Self::write_input`]) and the terminal's own replies both leave here,
    /// and only the first waits for an answer.
    fn queue_input(&mut self, bytes: &[u8]) -> Result<(), PtyError> {
        #[cfg(test)]
        self.input_tape.push(bytes.to_vec());
        let waiting = self.pending.load(Ordering::Relaxed);
        if waiting.saturating_add(bytes.len()) > PENDING_INPUT_BUDGET {
            return Err(PtyError::Io(std::io::Error::new(
                ErrorKind::WouldBlock,
                "the child has stopped reading its input",
            )));
        }
        // Counted BEFORE the send, so the writer thread can never subtract for
        // bytes this side has not added yet.
        self.pending.fetch_add(bytes.len(), Ordering::Relaxed);
        self.input.send(bytes.to_vec()).map_err(|_| {
            self.pending.fetch_sub(bytes.len(), Ordering::Relaxed);
            PtyError::Io(std::io::Error::new(
                ErrorKind::BrokenPipe,
                "the child is no longer taking input",
            ))
        })
    }

    #[cfg(all(test, unix))]
    fn input_tape(&self) -> &[Vec<u8>] {
        &self.input_tape
    }

    /// Resize both the grid and the child's notion of the window. Both halves
    /// matter: skip the grid and the screen wraps wrong, skip the pty and the
    /// child never receives `SIGWINCH` and keeps drawing at the old width.
    ///
    /// Zero is clamped to one rather than refused: a window dragged to nothing
    /// is a transient state, not a caller mistake.
    ///
    /// A resize to the size this lane already has does nothing at all, and
    /// says so by returning before the syscall. The window asks whenever a
    /// screen could have changed shape — a stage repaint, a divider, a tab
    /// coming forward — and most of those move nothing: switching tabs asked
    /// for the same grid every time. The grid below already refused an
    /// unchanged size; the `ioctl` above it did not, and it is the half that
    /// leaves this process to reach the kernel.
    ///
    /// # Errors
    ///
    /// [`PtyError::Open`] if the pty rejects the new size. The grid is left
    /// untouched in that case, so the two never disagree.
    pub fn resize(&mut self, rows: u16, cols: u16) -> Result<(), PtyError> {
        let rows = rows.max(1);
        let cols = cols.max(1);
        let grid = self.terminal.grid();
        if grid.rows() == rows as usize && grid.cols() == cols as usize {
            return Ok(());
        }
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| PtyError::Open(error.to_string()))?;
        self.terminal
            .grid_mut()
            .resize(rows as usize, cols as usize);
        Ok(())
    }

    #[must_use]
    pub const fn terminal(&self) -> &Terminal {
        &self.terminal
    }

    pub const fn terminal_mut(&mut self) -> &mut Terminal {
        &mut self.terminal
    }

    /// True once the child's output side has closed.
    #[must_use]
    pub const fn has_ended(&self) -> bool {
        self.ended
    }

    /// Exit status if the child has finished, `None` while it runs.
    ///
    /// A different question from [`Self::has_ended`], which reports that the
    /// *output* side closed — a child can stop writing long before it exits.
    ///
    /// # Errors
    ///
    /// [`PtyError::Io`] if the operating system cannot be asked about the
    /// child, which is distinct from "it is still running".
    pub fn try_wait(&mut self) -> Result<Option<u32>, PtyError> {
        match self.child.try_wait() {
            Ok(Some(status)) => Ok(Some(status.exit_code())),
            Ok(None) => Ok(None),
            Err(error) => Err(PtyError::Io(error)),
        }
    }

    /// Terminate the child. Closing a lane must not leave an agent running.
    ///
    /// # Errors
    ///
    /// [`PtyError::Io`] if the child's status could not be read or a signal
    /// could not be delivered. An already-reaped child is a no-op: its PID
    /// may have been reused and no longer grants permission to send a signal.
    pub fn kill(&mut self) -> Result<(), PtyError> {
        kill_known_child(self.child.as_mut()).map_err(PtyError::Io)
    }

    /// When the child last wrote, if it ever has.
    #[must_use]
    pub const fn last_output_at(&self) -> Option<Instant> {
        self.last_output_at
    }

    /// The same moment as [`Self::last_output_at`] on the wall clock, in
    /// epoch milliseconds — the number a silence is keyed by, stable across
    /// reads because it was taken once, when the bytes arrived.
    #[must_use]
    pub const fn last_output_epoch_ms(&self) -> Option<i64> {
        self.last_output_epoch_ms
    }

    /// The child's process id while it runs.
    ///
    /// For diagnostics, and for asking the operating system whether the process
    /// is really gone — [`Self::has_ended`] only reports that the *output* side
    /// closed, which is a different question.
    #[must_use]
    pub fn pid(&self) -> Option<u32> {
        self.child.process_id()
    }

    /// Whether the program holding the terminal right now is our own child.
    ///
    /// The kernel keeps one process group per terminal that is allowed to read
    /// it and to take its signals — the *foreground* group — and a job-control
    /// shell hands that group away when it runs a command and takes it back
    /// when the command ends. So this one question, asked of the pty rather
    /// than of anything the program says about itself, is what tells a shell
    /// sitting at its prompt from a shell with something running in it.
    ///
    /// It is asked of the operating system on purpose. The alternative — the
    /// terminal's TITLE, which is what Orca classifies to decide the same thing
    /// — is a string the running program chooses, so it is absent for every
    /// program that sets none, wrong for every program that sets a stale one,
    /// and unavailable exactly when it matters most: a tool that died without
    /// restoring the title leaves the last word it wrote standing forever.
    ///
    /// The comparison is against the CHILD's pid because that pid *is* its
    /// process group: the child is started as a new session (`setsid`, by
    /// `portable_pty` or by `posix_spawn` on macOS), so it leads the session
    /// and its group id equals its pid. Nothing needs to be recorded at spawn
    /// for this.
    ///
    /// `None` when the platform will not say — a closed or reaped descriptor
    /// answers nothing, and a platform with neither a foreground process
    /// group nor a process tree to read has no opinion. A caller must read
    /// that as "no opinion", never as "the shell is back". Windows has no
    /// foreground group on a ConPTY, so it reads the process tree instead:
    /// the command the shell is running — its direct child, as a job
    /// leader is — holds the terminal, never that command's own
    /// subprocesses ([`conpty_foreground`]). The tree cannot tell a shell
    /// from an agent spawned straight into the pty; the window's transport
    /// (`zerocode-lane`'s `PtyTransport for PtyLane`) states that rule for
    /// Windows with `zerocode_core::child_is_the_agent`.
    #[must_use]
    pub fn foreground_is_child(&self) -> Option<bool> {
        #[cfg(any(unix, windows))]
        {
            let holding = self.foreground_process_id()?;
            let child = self.child.process_id()?;
            Some(holding == child)
        }

        #[cfg(not(any(unix, windows)))]
        {
            None
        }
    }

    /// Process id leading the foreground process group on this terminal.
    ///
    /// This is the child a job-control shell put in front, not necessarily the
    /// shell process that [`Self::pid`] returns. `None` means the platform or a
    /// closed descriptor cannot provide that identity.
    #[must_use]
    pub fn foreground_process_id(&self) -> Option<u32> {
        #[cfg(unix)]
        {
            self.master
                .process_group_leader()
                .and_then(|pid| u32::try_from(pid).ok())
        }

        #[cfg(windows)]
        {
            self.conpty().map(|seen| seen.holder)
        }

        #[cfg(not(any(unix, windows)))]
        {
            None
        }
    }

    /// Whether the terminal echoes what is typed into it, as its flags say now.
    ///
    /// `None` when the platform will not say: a ConPTY has no line discipline to
    /// ask, and a descriptor that is closed or not a terminal answers nothing.
    /// A caller reads `None` as "cannot tell", never as "does not echo".
    #[must_use]
    pub fn echo_on(&self) -> Option<bool> {
        #[cfg(unix)]
        {
            let fd = self.master.as_raw_fd()?;
            let mut flags = std::mem::MaybeUninit::<libc::termios>::uninit();
            // SAFETY: `tcgetattr` writes one `termios` through the pointer it is
            // given and returns 0 only when it has; `fd` is the master we hold.
            let read = unsafe { libc::tcgetattr(fd, flags.as_mut_ptr()) };
            if read != 0 {
                return None;
            }
            // SAFETY: the call above returned 0, so `flags` is initialised.
            let flags = unsafe { flags.assume_init() };
            Some(flags.c_lflag & libc::ECHO != 0)
        }

        #[cfg(not(unix))]
        {
            None
        }
    }

    /// Who holds this ConPTY, from one process snapshot.
    #[cfg(windows)]
    fn conpty(&self) -> Option<ConptyForeground> {
        let shell = self.child.process_id()?;
        Some(conpty_foreground(
            shell,
            &windows_processes::process_table(),
            windows_processes::born,
        ))
    }

    /// The names of the pty's OWN child, by the same roads
    /// [`Self::foreground_programs`] takes, best first — the first is the
    /// name it is called by. For the window's transport, which must know
    /// whether the child is itself an agent on a platform whose terminal
    /// cannot say so. Empty when the process is gone or unreadable.
    #[must_use]
    pub fn child_programs(&self) -> Vec<String> {
        let line: Vec<ProcessFacts> = self
            .child
            .process_id()
            .and_then(|pid| i32::try_from(pid).ok())
            .map(process_facts)
            .into_iter()
            .collect();
        ordered_names(&line, std::env::consts::EXE_SUFFIX)
    }

    /// The NAME of the program holding the terminal right now.
    ///
    /// The foreground process group leader's executable, asked of the OS the
    /// same way [`Self::foreground_is_child`] asks its question — never of
    /// anything the program says about itself. This is the send menu's third
    /// road (1-g7): a `claude` somebody typed into a plain shell has no
    /// launch record and may run no hooks, but the kernel still knows who
    /// holds the pty.
    ///
    /// Empty when the platform will not say, and a caller reads that as "no
    /// opinion". Windows keeps no foreground notion on a ConPTY, so the
    /// names come off the process tree ([`conpty_foreground`]): the
    /// command the shell runs first, then each process under it down to
    /// the deepest descendant, since a launcher often has the real program
    /// below it. Their roads are the executable's name and path only — the
    /// argv roads need the PEB and stay empty there.
    ///
    /// SEVERAL names, best first, because launchers bury the real one
    /// (live reports 2026-08-14): Claude Code's binary is
    /// `~/.local/share/claude/versions/2.1.232` — a version number — with
    /// the name in a parent DIRECTORY; codex is `#!/usr/bin/env node
    /// …/codex.js`, so the process is `node` and the name is `argv[1]`'s. The
    /// roads, in order of trust: the executable's own name, `argv[0]`, the
    /// interpreted script's name (with and without its extension), then the
    /// executable path's directories. The first two are names a program is
    /// called by (`program_name`): a Windows image's `.exe` comes off, or
    /// `claude.exe` would never meet the agent table's `claude`.
    #[must_use]
    pub fn foreground_programs(&self) -> Vec<String> {
        self.foreground_programs_read().0
    }

    /// [`Self::foreground_programs`], and whether every process on the line
    /// said all it can say of itself (t-76489 r6): its image, and on unix
    /// its argv too. A name missing because a road would not answer is not
    /// a name the program lacks — a ConPTY's `cmd` read while the Claude
    /// below it would not say, a `node` whose script would not be read —
    /// so only a line read whole can say no agent holds the terminal.
    #[must_use]
    pub fn foreground_programs_read(&self) -> (Vec<String>, bool) {
        // `MasterPty::process_group_leader` exists on unix only (a ConPTY
        // has no foreground process group); Windows names the line the
        // process tree gives; anywhere else falls through to "no opinion" —
        // the empty answer this doc promises.
        #[cfg(unix)]
        let holding: Vec<u32> = self
            .master
            .process_group_leader()
            .and_then(|pid| u32::try_from(pid).ok())
            .into_iter()
            .collect();
        #[cfg(windows)]
        let holding: Vec<u32> = self.conpty().map(|seen| seen.named).unwrap_or_default();
        #[cfg(not(any(unix, windows)))]
        let holding: Vec<u32> = Vec::new();
        let line: Vec<Option<ProcessFacts>> = holding
            .into_iter()
            .map(|pid| i32::try_from(pid).ok().map(process_facts))
            .collect();
        named_line(line, std::env::consts::EXE_SUFFIX, ARGV_ROAD)
    }

    /// The names of every OTHER process in the terminal's foreground group,
    /// one list per process, each best first by the roads
    /// [`Self::foreground_programs`] takes — and whether they are all of
    /// them ([`GroupPrograms`]).
    ///
    /// A launcher that does not `exec` its program leads the group while the
    /// program runs below it: a shell script running `claude "$@"` and
    /// waiting for its status names `sh` alone to
    /// [`Self::foreground_programs`], and the agent it waits on is one of
    /// these (t-76489 r4). A non-interactive shell does no job control, so
    /// what it starts stays in its group.
    ///
    /// The group's list is pids alone, and a pid read after it may be
    /// another process by then (t-76489 r5): a member that ended, its pid
    /// taken by a program of another group, or of another person. A member
    /// is named only while it is the same process — the same start — of
    /// this group, this terminal's session and this user, before and after
    /// its names are read (`group_programs`). One that is not, one that
    /// will not say, and one past `FOREGROUND_GROUP_NAMED` leave the answer
    /// [`GroupPrograms::Partial`]: what was not read is not known absent.
    ///
    /// Windows names the process line down from the holder in
    /// [`Self::foreground_programs`] already, so it has nothing more to
    /// give here: [`GroupPrograms::Whole`], and empty. Whether that line
    /// was read whole is [`Self::foreground_programs_read`]'s to say.
    #[must_use]
    pub fn foreground_group_programs(&self) -> GroupPrograms {
        #[cfg(unix)]
        {
            let Some(leader) = self.master.process_group_leader() else {
                return GroupPrograms::Unread;
            };
            // The child leads a session of its own, so its pid is the
            // session of every process on this terminal.
            let Some(session) = self
                .child
                .process_id()
                .and_then(|pid| i32::try_from(pid).ok())
            else {
                return GroupPrograms::Unread;
            };
            // SAFETY: `getuid` takes nothing and cannot fail.
            let user = unsafe { libc::getuid() };
            group_programs(
                (leader, session, user),
                group_members(leader),
                &process_identity,
                &|pid| {
                    let facts = process_facts(pid);
                    facts_whole(&facts, ARGV_ROAD)
                        .then(|| ordered_names(&[facts], std::env::consts::EXE_SUFFIX))
                },
            )
        }

        #[cfg(windows)]
        {
            GroupPrograms::Whole(Vec::new())
        }

        #[cfg(not(any(unix, windows)))]
        {
            GroupPrograms::Unread
        }
    }
}

/// What [`PtyLane::foreground_group_programs`] could say of the foreground
/// group's other processes (t-76489 r5). No agent among them means the
/// group runs none only when the answer is [`Self::Whole`]; otherwise the
/// one not read may be it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupPrograms {
    /// Every other process of the group, named.
    Whole(Vec<Vec<String>>),
    /// These were named, and at least one more process was not: the group
    /// was listed in part, a process would not say its names or who it
    /// is, it was not the same process of this group before and after its
    /// names were read, or it came past the bound.
    Partial(Vec<Vec<String>>),
    /// The group could not be listed, or the platform will not say.
    Unread,
}

impl GroupPrograms {
    /// The names read, one list per process; none when nothing was.
    #[must_use]
    pub fn named(&self) -> &[Vec<String>] {
        match self {
            Self::Whole(named) | Self::Partial(named) => named,
            Self::Unread => &[],
        }
    }
}

/// How many of a foreground group's other processes
/// [`PtyLane::foreground_group_programs`] reads: a launcher's program is
/// born right after it, and a build leading a group of hundreds is not read
/// whole.
#[cfg(unix)]
const FOREGROUND_GROUP_NAMED: usize = 32;

/// Who one process is, as the OS says it now: its group, its session, its
/// user, and when it started — a pid reused is another start.
#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProcessIdentity {
    group: i32,
    session: i32,
    user: u32,
    started: u64,
}

/// [`PtyLane::foreground_group_programs`] off the platform's answers:
/// `ours` the group, session and user a member must be of, `listed` the
/// group's pids and whether that list is whole (`None` when it could not be
/// read), `identity` who a pid is now and `names` what it is called —
/// `None` when a road would not answer ([`facts_whole`]). The group's
/// leader, whose pid is the group's, is not among the members.
///
/// A member's names count only when it is ours and the same process before
/// and after they are read; any member they do not count for leaves the
/// answer partial.
#[cfg(unix)]
fn group_programs(
    ours: (i32, i32, u32),
    listed: Option<(Vec<i32>, bool)>,
    identity: &dyn Fn(i32) -> Option<ProcessIdentity>,
    names: &dyn Fn(i32) -> Option<Vec<String>>,
) -> GroupPrograms {
    let (group, session, user) = ours;
    let Some((pids, mut whole)) = listed else {
        return GroupPrograms::Unread;
    };
    let mut named = Vec::new();
    for (read, pid) in pids.into_iter().filter(|pid| *pid != group).enumerate() {
        if read == FOREGROUND_GROUP_NAMED {
            whole = false;
            break;
        }
        let Some(before) = identity(pid)
            .filter(|seen| seen.group == group && seen.session == session && seen.user == user)
        else {
            whole = false;
            continue;
        };
        let Some(called) = names(pid).filter(|called| !called.is_empty()) else {
            whole = false;
            continue;
        };
        if identity(pid) != Some(before) {
            whole = false;
            continue;
        }
        named.push(called);
    }
    if whole {
        GroupPrograms::Whole(named)
    } else {
        GroupPrograms::Partial(named)
    }
}

/// The pids in process group `group`, lowest first, and whether that is all
/// of them; `None` when the platform will not say.
#[cfg(target_os = "macos")]
fn group_members(group: i32) -> Option<(Vec<i32>, bool)> {
    // `proc_listpids` out of libproc, declared here as `proc_pidpath` is;
    // PROC_PGRP_ONLY is libproc's own selector for "the members of one
    // process group". Asked once for the size, then for the pids, with room
    // for a few born in between; a list that fills the room may have left
    // some out.
    unsafe extern "C" {
        fn proc_listpids(kind: u32, typeinfo: u32, buffer: *mut i32, buffersize: i32) -> i32;
    }
    const PROC_PGRP_ONLY: u32 = 2;
    let typeinfo = u32::try_from(group).ok()?;
    let wanted = unsafe { proc_listpids(PROC_PGRP_ONLY, typeinfo, std::ptr::null_mut(), 0) };
    let wanted = usize::try_from(wanted).ok().filter(|wanted| *wanted > 0)?;
    let mut pids = vec![0i32; wanted / std::mem::size_of::<i32>() + 16];
    let room = i32::try_from(pids.len() * std::mem::size_of::<i32>()).ok()?;
    let filled = unsafe { proc_listpids(PROC_PGRP_ONLY, typeinfo, pids.as_mut_ptr(), room) };
    listed_pids(filled, room, pids)
}

/// The pids `proc_listpids` put in `pids`, lowest first, off the bytes it
/// answered it `filled` of the `room` it was given, and whether that is all
/// of them; `None` for an answer that lists nothing (t-76489 r6).
///
/// libproc fails with 0 as well as with a negative — asked with one byte of
/// room on 2026-10-27 it answered 0 and `ENOMEM` — and a group whose
/// terminal names it has a member to list, so no answer of 0 is an empty
/// group read whole. A list that fills the room may have left some out.
#[cfg(target_os = "macos")]
fn listed_pids(filled: i32, room: i32, mut pids: Vec<i32>) -> Option<(Vec<i32>, bool)> {
    if filled <= 0 {
        return None;
    }
    let whole = filled < room;
    let filled = usize::try_from(filled).ok()?;
    pids.truncate((filled / std::mem::size_of::<i32>()).min(pids.len()));
    pids.retain(|pid| *pid > 0);
    pids.sort_unstable();
    pids.dedup();
    Some((pids, whole))
}

/// The pids in process group `group`, lowest first, off `/proc` — the
/// group is the fifth field of each `stat`, the first after the command's
/// closing parenthesis but two — and whether that is all of them: a `stat`
/// that will not be read or parsed may be a member. `None` where there is
/// no `/proc`.
#[cfg(all(unix, not(target_os = "macos")))]
fn group_members(group: i32) -> Option<(Vec<i32>, bool)> {
    let entries = std::fs::read_dir("/proc").ok()?;
    let mut whole = true;
    let mut pids = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else {
            whole = false;
            continue;
        };
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<i32>().ok())
        else {
            continue;
        };
        match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(stat) => match stat_fields(&stat).and_then(|fields| fields.get(2)?.parse().ok()) {
                Some(of) if of == group => pids.push(pid),
                Some(_) => {}
                None => whole = false,
            },
            // A process that ended since the folder was read is no member.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => whole = false,
        }
    }
    pids.sort_unstable();
    Some((pids, whole))
}

/// The fields of a `/proc/<pid>/stat` after the command's closing
/// parenthesis: the state first, then the parent, the group and the
/// session, and the start the twentieth.
#[cfg(all(unix, not(target_os = "macos")))]
fn stat_fields(stat: &str) -> Option<Vec<&str>> {
    let (_, after) = stat.rsplit_once(')')?;
    Some(after.split_whitespace().collect())
}

/// Who process `pid` is now, off libproc's BSD record and its session; `None`
/// when it will not say — a process gone, or not ours to ask about.
#[cfg(target_os = "macos")]
fn process_identity(pid: i32) -> Option<ProcessIdentity> {
    let size = i32::try_from(std::mem::size_of::<libc::proc_bsdinfo>()).ok()?;
    // SAFETY: an all-zero `proc_bsdinfo` is a valid value of a plain C struct.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    // SAFETY: the buffer is `size` bytes of the struct the flavour fills.
    let filled =
        unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size) };
    if filled != size {
        return None;
    }
    // SAFETY: `getsid` reads a pid and answers -1 for one it cannot.
    let session = unsafe { libc::getsid(pid) };
    if session < 0 {
        return None;
    }
    Some(ProcessIdentity {
        group: i32::try_from(info.pbi_pgid).ok()?,
        session,
        user: info.pbi_uid,
        started: info
            .pbi_start_tvsec
            .checked_mul(1_000_000)?
            .checked_add(info.pbi_start_tvusec)?,
    })
}

/// Who process `pid` is now, off `/proc/<pid>/stat` and the owner of its
/// folder; `None` when it will not say.
#[cfg(all(unix, not(target_os = "macos")))]
fn process_identity(pid: i32) -> Option<ProcessIdentity> {
    use std::os::unix::fs::MetadataExt;
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let fields = stat_fields(&stat)?;
    Some(ProcessIdentity {
        group: fields.get(2)?.parse().ok()?,
        session: fields.get(3)?.parse().ok()?,
        user: std::fs::metadata(format!("/proc/{pid}")).ok()?.uid(),
        started: fields.get(19)?.parse().ok()?,
    })
}

/// What the OS says about one process: its image path and its argv.
type ProcessFacts = (Option<String>, Vec<String>);

fn process_facts(pid: i32) -> ProcessFacts {
    (executable_path(pid), argv_of(pid))
}

/// Whether this platform reads a process's argv ([`argv_of`]): Windows
/// keeps it in the PEB and the name comes off the image alone.
const ARGV_ROAD: bool = cfg!(unix);

/// Whether `facts` hold everything the roads could say (t-76489 r6): the
/// image, and the argv where `argv_road` reads one. A process whose image
/// or argv would not be read may be named by the road that failed — a
/// `node` running `codex.js`, the Claude under a ConPTY's `cmd`.
fn facts_whole((exec, argv): &ProcessFacts, argv_road: bool) -> bool {
    exec.is_some() && (!argv_road || !argv.is_empty())
}

/// [`ordered_names`] of a line of processes, `None` for one that could not
/// be asked, and whether every one of them said everything
/// ([`facts_whole`]); an empty line said nothing.
fn named_line(
    line: Vec<Option<ProcessFacts>>,
    suffix: &str,
    argv_road: bool,
) -> (Vec<String>, bool) {
    let whole = !line.is_empty()
        && line.iter().all(|facts| {
            facts
                .as_ref()
                .is_some_and(|facts| facts_whole(facts, argv_road))
        });
    let line: Vec<ProcessFacts> = line.into_iter().flatten().collect();
    (ordered_names(&line, suffix), whole)
}

/// Every name a line of processes answers to, best first and without repeats
/// — the roads [`PtyLane::foreground_programs`] documents. Each process's own
/// names (its image, `argv[0]`, the interpreted script with and without its
/// extension) come for the WHOLE line before any process's image directories:
/// the sweep takes the first name the agent table knows, and a launcher's
/// install path (nvm under a user named `pi`) must not outrank the program it
/// launched. One process — the unix foreground leader — keeps the old order.
fn ordered_names(line: &[ProcessFacts], suffix: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut put = |name: &str| {
        let name = name.trim();
        if !name.is_empty() && !names.iter().any(|held| held == name) {
            names.push(name.to_string());
        }
    };
    for (exec, argv) in line {
        if let Some(path) = exec.as_deref() {
            put(program_name(path, suffix));
        }
        if let Some(first) = argv.first() {
            put(program_name(first, suffix));
        }
        if let Some(script) = argv.get(1) {
            let script_name = base_name(script);
            put(script_name);
            if let Some((stem, _)) = script_name.rsplit_once('.') {
                put(stem);
            }
        }
    }
    for (exec, _) in line {
        if let Some(path) = exec.as_deref() {
            for part in path.split(PATH_SEPARATORS).rev().skip(1) {
                put(part);
            }
        }
    }
    names
}

/// The name a program is called by: a path's last segment without the
/// platform's executable `suffix` (`std::env::consts::EXE_SUFFIX` — `.exe`
/// on Windows, empty on unix). The agent table names `claude` and `zo`, and
/// a Windows image path says `claude.exe` and `zo.exe`, so this is the one
/// place that suffix comes off. Case-blind, because the file system is; a
/// name that is only the suffix stays a name.
fn program_name<'a>(path: &'a str, suffix: &str) -> &'a str {
    let name = base_name(path);
    if suffix.is_empty() || name.len() <= suffix.len() {
        return name;
    }
    let cut = name.len() - suffix.len();
    match (name.get(..cut), name.get(cut..)) {
        (Some(stem), Some(tail)) if tail.eq_ignore_ascii_case(suffix) => stem,
        _ => name,
    }
}

/// A path's last segment.
fn base_name(path: &str) -> &str {
    path.rsplit(PATH_SEPARATORS).next().unwrap_or(path)
}

/// Both separators, because a Windows image path comes back with
/// backslashes and a script named on a unix command line with slashes.
const PATH_SEPARATORS: [char; 2] = ['/', '\\'];

/// Who holds a ConPTY, and which processes say what is running in it —
/// read off the process tree, because a ConPTY has no foreground process
/// group to ask.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConptyForeground {
    /// The process holding the terminal: the command the shell is running
    /// (its direct child, on the line down to the deepest descendant) —
    /// what the foreground group's leader is under a unix job-control shell
    /// — or the shell itself when nothing runs under it. This is the pid
    /// [`PtyLane::foreground_is_child`] compares, so a command's own
    /// subprocesses (an agent's tools) never take the terminal from it.
    pub holder: u32,
    /// The processes whose names say what runs, best first: the holder,
    /// then each process under it down to the deepest descendant. For
    /// naming only — a launcher (`cmd` running a `.cmd`, `node` running
    /// `codex.js`) often has the real program somewhere below it.
    pub named: Vec<u32>,
}

/// [`ConptyForeground`] from a `(pid, parent pid)` table.
///
/// The deepest descendant of `shell` is found breadth-first, the largest
/// pid among equals (a fresh child tends to be the newer one); the line
/// from the shell's direct child down to it is `named`, and its first
/// process is the `holder`. With nothing under the shell both are the
/// shell.
///
/// A Toolhelp parent pid is the pid a process was STARTED by, and Windows
/// reuses a pid once its process exits — so an orphan can name, as its
/// parent, a pid that now belongs to this pane's shell. A child is never
/// older than its parent, so a link that `born` (a process's creation
/// time) refutes is no link; a birth nobody could read refutes nothing.
/// `born` is asked only about the shell's candidates, never the whole
/// table. Bounded by the table's size, so a stale snapshot with a cycle
/// cannot spin. Pure, so the choice is testable without a live tree.
#[must_use]
pub fn conpty_foreground(
    shell: u32,
    table: &[(u32, u32)],
    born: impl Fn(u32) -> Option<u64>,
) -> ConptyForeground {
    let mut births: Vec<(u32, Option<u64>)> = Vec::new();
    let mut birth = |pid: u32| {
        if let Some((_, known)) = births.iter().find(|(asked, _)| *asked == pid) {
            return *known;
        }
        let known = born(pid);
        births.push((pid, known));
        known
    };
    // Every process reached, with the parent it was reached from.
    let mut reached: Vec<(u32, u32)> = Vec::new();
    let mut frontier = vec![shell];
    let mut leaf = shell;
    let mut rounds = 0;
    while !frontier.is_empty() && rounds <= table.len() {
        let mut next: Vec<u32> = Vec::new();
        for &parent in &frontier {
            let parent_born = birth(parent);
            for &(pid, of) in table {
                if of != parent || pid == shell || reached.iter().any(|(held, _)| *held == pid) {
                    continue;
                }
                if let (Some(child_born), Some(parent_born)) = (birth(pid), parent_born)
                    && child_born < parent_born
                {
                    continue;
                }
                reached.push((pid, parent));
                next.push(pid);
            }
        }
        if next.is_empty() {
            break;
        }
        leaf = next.iter().copied().max().unwrap_or(leaf);
        frontier = next;
        rounds += 1;
    }
    let mut named = vec![leaf];
    let mut at = leaf;
    while let Some(&(_, parent)) = reached.iter().find(|(held, _)| *held == at) {
        if parent == shell {
            break;
        }
        named.push(parent);
        at = parent;
    }
    named.reverse();
    ConptyForeground {
        holder: named.first().copied().unwrap_or(shell),
        named,
    }
}

/// The executable PATH behind a pid, by the road each platform offers.
#[cfg(target_os = "macos")]
fn executable_path(pid: i32) -> Option<String> {
    // `proc_pidpath` out of libproc — the one call macOS offers for "what
    // binary is this pid", declared here rather than carrying a crate for a
    // single symbol. The buffer size is libproc's own PROC_PIDPATHINFO_MAXSIZE.
    unsafe extern "C" {
        fn proc_pidpath(pid: i32, buffer: *mut u8, buffersize: u32) -> i32;
    }
    let mut path = vec![0u8; 4096];
    let len = unsafe { proc_pidpath(pid, path.as_mut_ptr(), u32::try_from(path.len()).ok()?) };
    if len <= 0 {
        return None;
    }
    Some(String::from_utf8_lossy(&path[..usize::try_from(len).ok()?]).into_owned())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn executable_path(pid: i32) -> Option<String> {
    std::fs::read_link(format!("/proc/{pid}/exe"))
        .ok()
        .map(|path| path.to_string_lossy().into_owned())
}

#[cfg(windows)]
fn executable_path(pid: i32) -> Option<String> {
    windows_processes::image_path(u32::try_from(pid).ok()?)
}

#[cfg(not(any(unix, windows)))]
fn executable_path(_pid: i32) -> Option<String> {
    None
}

/// The first two argv words of the foreground program.
#[cfg(target_os = "macos")]
fn argv_of(pid: i32) -> Vec<String> {
    // KERN_PROCARGS2, measured layout: a C int argc, the executable path
    // NUL-terminated, NUL padding, then the argv words — declared here
    // rather than carrying a crate for one sysctl.
    unsafe extern "C" {
        fn sysctl(
            name: *mut i32,
            namelen: u32,
            oldp: *mut u8,
            oldlenp: *mut usize,
            newp: *mut u8,
            newlen: usize,
        ) -> i32;
    }
    const CTL_KERN: i32 = 1;
    const KERN_PROCARGS2: i32 = 49;
    let mut mib = [CTL_KERN, KERN_PROCARGS2, pid];
    let mut size: usize = 0;
    let asked = unsafe {
        sysctl(
            mib.as_mut_ptr(),
            3,
            std::ptr::null_mut(),
            &raw mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if asked != 0 || size == 0 {
        return Vec::new();
    }
    let mut held = vec![0u8; size];
    let read = unsafe {
        sysctl(
            mib.as_mut_ptr(),
            3,
            held.as_mut_ptr(),
            &raw mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if read != 0 {
        return Vec::new();
    }
    held.truncate(size);
    argv_in(&held)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn argv_of(pid: i32) -> Vec<String> {
    let Ok(cmdline) = std::fs::read(format!("/proc/{pid}/cmdline")) else {
        return Vec::new();
    };
    cmdline
        .split(|&byte| byte == 0)
        .take(2)
        .filter_map(|word| std::str::from_utf8(word).ok())
        .filter(|word| !word.is_empty())
        .map(str::to_string)
        .collect()
}

/// Windows keeps a process's command line in its PEB; reading another
/// process's PEB is a debugger's road, not this lane's, so the argv roads
/// stay empty there and the name comes from the image path alone.
#[cfg(not(unix))]
fn argv_of(_pid: i32) -> Vec<String> {
    Vec::new()
}

#[cfg(windows)]
mod windows_processes {
    //! The process tree and image names, asked of Win32 directly.

    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };

    /// When a pid's current process was created, as a FILETIME count —
    /// nothing when the process is gone or may not be queried.
    pub(super) fn born(pid: u32) -> Option<u64> {
        // SAFETY: opening with the least right that answers the times query.
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if process.is_null() {
            return None;
        }
        let mut created = FILETIME::default();
        let mut exited = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        // SAFETY: four live FILETIMEs for the four out-parameters.
        let ok =
            unsafe { GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user) };
        // SAFETY: closing the handle we opened.
        unsafe {
            CloseHandle(process);
        }
        (ok != 0)
            .then(|| (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
    }

    /// Every process as `(pid, parent pid)` — one snapshot, read once.
    pub(super) fn process_table() -> Vec<(u32, u32)> {
        let mut table = Vec::new();
        // SAFETY: a plain snapshot request; the handle is closed below.
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if snapshot == INVALID_HANDLE_VALUE || snapshot.is_null() {
            return table;
        }
        let mut entry = PROCESSENTRY32W {
            dwSize: u32::try_from(std::mem::size_of::<PROCESSENTRY32W>()).unwrap_or(0),
            ..Default::default()
        };
        // SAFETY: `entry` is a valid, sized PROCESSENTRY32W for the whole walk.
        let mut more = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
        while more {
            table.push((entry.th32ProcessID, entry.th32ParentProcessID));
            // SAFETY: same live snapshot and entry as above.
            more = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
        }
        // SAFETY: closing the snapshot handle we opened.
        unsafe {
            CloseHandle(snapshot);
        }
        table
    }

    /// The full image path of a pid, or nothing when the process is gone or
    /// belongs to someone this window may not query.
    pub(super) fn image_path(pid: u32) -> Option<String> {
        // SAFETY: opening with the least right that answers the name query.
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if process.is_null() {
            return None;
        }
        let mut buffer = vec![0_u16; 32 * 1024];
        let mut length = u32::try_from(buffer.len()).unwrap_or(u32::MAX);
        // SAFETY: the buffer is live and `length` is its capacity in UTF-16 units.
        let ok =
            unsafe { QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut length) };
        // SAFETY: closing the handle we opened.
        unsafe {
            CloseHandle(process);
        }
        if ok == 0 {
            return None;
        }
        let length = usize::try_from(length).ok()?.min(buffer.len());
        Some(String::from_utf16_lossy(&buffer[..length]))
    }
}

/// The first two argv words of a `KERN_PROCARGS2` buffer. Pure so the parse
/// is testable without a live process.
#[cfg(any(target_os = "macos", test))]
fn argv_in(buffer: &[u8]) -> Vec<String> {
    let Some(rest) = buffer.get(4..) else {
        return Vec::new();
    };
    let Some(exec_end) = rest.iter().position(|&byte| byte == 0) else {
        return Vec::new();
    };
    let mut at = exec_end;
    while rest.get(at) == Some(&0) {
        at += 1;
    }
    let mut words = Vec::new();
    let mut start = at;
    for here in at..rest.len() {
        if rest[here] == 0 {
            if let Ok(word) = std::str::from_utf8(&rest[start..here])
                && !word.is_empty()
            {
                words.push(word.to_string());
            }
            start = here + 1;
            if words.len() >= 2 {
                break;
            }
        }
    }
    words
}

/// Killing the child when the lane goes away is the whole ownership claim.
///
/// A `PtyLane` exists to draw a screen. Once it is dropped nobody can read that
/// screen, or the child's output, ever again — so a surviving process is pure
/// waste: an agent spending tokens for a window that closed.
///
/// Without this, every path that lets a lane fall out of scope leaks one.
/// Dropping a registry holding eight lanes leaked eight agents, and handing a
/// lane to something that then refused it leaked that one, because the caller
/// had already moved it and had no handle left to clean up with.
///
/// A destructor cannot return an error. Explicit close records the outcome
/// before forgetting a lane; this last best effort uses the same child guard.
impl Drop for PtyLane {
    fn drop(&mut self) {
        let _ = kill_known_child(self.child.as_mut());
    }
}

/// The most times a lane asks for a pty before the failure is its answer.
///
/// Each ask is a whole round of the kernel's own retry (see
/// [`PTY_OPEN_PASSING_ERRORS`]) — the open and its ten reopens — so five
/// rounds is fifty-five opens of `/dev/ptmx`: far past a race between a
/// handful of opens, and still a prompt answer for an error that does not
/// pass. A round the race never lets go of sleeps 0.4 s in the kernel
/// (the nth reopen from the sixth sleeps n × 10 ms: 60 + 70 + 80 + 90 +
/// 100 ms), so the worst a pane waits is about 2 s, against 0.4 s with one
/// ask.
const PTY_OPEN_ATTEMPTS: usize = 5;

/// XNU's in-kernel "redrive open" (`bsd/sys/errno.h`), which `libc` does
/// not name because it is not meant to leave the kernel. It does: see
/// [`PTY_OPEN_PASSING_ERRORS`].
#[cfg(unix)]
const EREDRIVEOPEN: i32 = -6;

/// The `errno`s that say a pty open lost a race and may be asked again,
/// rather than that there is no pty to be had.
///
/// `/dev/ptmx` picks a free minor number under one hold of `DEVFS_LOCK`
/// (`ptmx_clone`, `bsd/kern/tty_ptmx.c`) and claims it under another
/// (`ptcopen`), so two opens at once can be handed the same one; the
/// loser's `ptcopen` answers [`EREDRIVEOPEN`] (`bsd/kern/tty_dev.c`).
/// `vn_open_auth` reopens on it ten times, pausing from the sixth, and
/// only then lets it out — which is how gate l81e read `Os { code: -6 }`
/// from `openpty` while a second test binary opened ptys alongside it.
/// `ENXIO` is the same driver's answer when it cannot set the pair up
/// (also its answer to a full pty table, where five quick asks cost
/// nothing), and `EAGAIN` the replica's while the pair is not yet
/// unlocked. Anything else — `EMFILE`, `EACCES` — is the answer at once.
#[cfg(unix)]
const PTY_OPEN_PASSING_ERRORS: [i32; 3] = [EREDRIVEOPEN, libc::ENXIO, libc::EAGAIN];
/// ConPTY reports no `errno`; nothing is asked twice.
#[cfg(not(unix))]
const PTY_OPEN_PASSING_ERRORS: [i32; 0] = [];

/// Ask `open` for a pty pair, again while it fails with an error in
/// `PTY_OPEN_PASSING_ERRORS`, at most `PTY_OPEN_ATTEMPTS` times.
///
/// `errno_of` reads the `errno` out of whatever error `open` fails with:
/// the raw `io::Error::raw_os_error` for a caller that opens with
/// `libc::openpty` itself, the text of a `portable_pty` failure for a lane
/// (`open_pty`). The set of `errno`s and the bound are this one's, so no
/// caller restates them. The last error comes back as it was.
///
/// No pause between asks: the kernel already paused inside each failed
/// one, and a pause here would only hold up the window that is opening
/// the pane. A yield hands the processor to the open this one raced, which
/// is what clears it.
pub fn ask_for_pty<P, E>(
    mut open: impl FnMut() -> Result<P, E>,
    errno_of: impl Fn(&E) -> Option<i32>,
) -> Result<P, E> {
    let mut asked = 1;
    loop {
        let error = match open() {
            Ok(pair) => return Ok(pair),
            Err(error) => error,
        };
        let passing = errno_of(&error).is_some_and(|code| PTY_OPEN_PASSING_ERRORS.contains(&code));
        if !passing || asked == PTY_OPEN_ATTEMPTS {
            return Err(error);
        }
        asked += 1;
        std::thread::yield_now();
    }
}

/// [`ask_for_pty`] for the platform's own pty system in a lane, whose
/// failure carries its `errno` only as text, and for the stand-in that
/// fails the way a kernel does in the tests below.
fn open_pty<P, E: std::fmt::Display>(
    mut open: impl FnMut() -> Result<P, E>,
) -> Result<P, PtyError> {
    ask_for_pty(
        || open().map_err(|error| error.to_string()),
        |reason| os_error_code(reason),
    )
    .map_err(PtyError::Open)
}

/// The `errno` in a `portable_pty` open failure.
///
/// Its error carries the `io::Error` only as text — `bail!("failed to
/// openpty: {:?}", …)` — so the code is read back out of the `Debug` form,
/// `Os { code: -6, kind: …`. Text worded any other way gives `None`, which
/// is never asked again: a reworded dependency costs the retry, not the
/// error.
fn os_error_code(reason: &str) -> Option<i32> {
    const CODE: &str = "Os { code: ";
    let after = &reason[reason.find(CODE)? + CODE.len()..];
    after[..after.find(',')?].parse().ok()
}

/// Start the lane's child on the pty's slave.
///
/// On macOS without a fork ([`crate::posix_launch`]): a fork in a window
/// that runs CEF can stand forever before its exec holding a copy of every
/// descriptor the window has (t-73907). Elsewhere through `portable_pty`,
/// whose ConPTY start on Windows forks nothing.
#[cfg(target_os = "macos")]
fn start_child(
    pair: &portable_pty::PtyPair,
    program: &str,
    args: &[String],
    cwd: Option<&Path>,
    edits: &[(&str, Option<&str>)],
) -> Result<Box<dyn portable_pty::Child + Send + Sync>, PtyError> {
    let spawn_error = |reason: String| PtyError::Spawn {
        program: program.to_string(),
        reason,
    };
    let tty = pair
        .master
        .tty_name()
        .ok_or_else(|| PtyError::Open("the pty has no slave name".into()))?;
    let args: Vec<std::ffi::OsString> = args.iter().map(Into::into).collect();
    let edits: Vec<crate::posix_launch::EnvEdit> = edits
        .iter()
        .map(|(key, value)| ((*key).into(), value.map(Into::into)))
        .collect();
    crate::posix_launch::spawn(&tty, program.as_ref(), &args, cwd, &edits)
        .map(|child| Box::new(child) as Box<dyn portable_pty::Child + Send + Sync>)
        .map_err(|error| spawn_error(error.to_string()))
}

#[cfg(not(target_os = "macos"))]
fn start_child(
    pair: &portable_pty::PtyPair,
    program: &str,
    args: &[String],
    cwd: Option<&Path>,
    edits: &[(&str, Option<&str>)],
) -> Result<Box<dyn portable_pty::Child + Send + Sync>, PtyError> {
    let mut command = CommandBuilder::new(program);
    for arg in args {
        command.arg(arg);
    }
    if let Some(cwd) = cwd {
        command.cwd(cwd);
    }
    for (key, value) in edits {
        match value {
            Some(value) => command.env(key, value),
            None => command.env_remove(key),
        }
    }
    pair.slave
        .spawn_command(command)
        .map_err(|error| PtyError::Spawn {
            program: program.to_string(),
            reason: error.to_string(),
        })
}

fn kill_known_child(child: &mut (impl portable_pty::Child + ?Sized)) -> std::io::Result<()> {
    // portable-pty's Unix ChildKiller sends SIGHUP to Child::id() before
    // looking at its cached exit status. Never pass it a reaped child: that
    // number can name another process, even though we still own this handle.
    match child.try_wait()? {
        Some(_) => Ok(()),
        None => child.kill(),
    }
}

#[cfg(test)]
mod foreground_tests {
    use super::{argv_in, conpty_foreground, named_line, ordered_names, program_name};

    /// t-76489 r6, the codex review's second finding of r5: a line one of
    /// whose processes would not say everything is not read whole, though
    /// the names the others gave still stand. On Windows a ConPTY's `cmd`
    /// gives its image while the Claude below it does not; on unix a
    /// `node` gives its path while its argv — the script that names codex
    /// — does not. Before, the names read were all there was, and a line
    /// that named `cmd` alone said no agent held the terminal.
    #[test]
    fn a_line_a_process_of_which_would_not_say_everything_is_not_read_whole() {
        let cmd = || Some((Some(r"C:\Windows\System32\cmd.exe".to_string()), Vec::new()));
        let claude = || Some((Some(r"C:\Users\dev\claude.exe".to_string()), Vec::new()));
        let (names, whole) = named_line(vec![cmd(), Some((None, Vec::new()))], ".exe", false);
        assert!(names.iter().any(|name| name == "cmd"), "{names:?}");
        assert!(
            !whole,
            "the Claude whose image would not be read was known absent"
        );
        assert!(
            !named_line(vec![cmd(), None], ".exe", false).1,
            "a pid not asked"
        );
        let (names, whole) = named_line(vec![cmd(), claude()], ".exe", false);
        assert!(
            whole && names.iter().any(|name| name == "claude"),
            "{names:?}"
        );

        let node = || (Some("/usr/local/bin/node".to_string()), Vec::new());
        let (names, whole) = named_line(vec![Some(node())], "", true);
        assert_eq!(names.first().map(String::as_str), Some("node"));
        assert!(
            !whole,
            "the script whose argv would not be read was known absent"
        );
        let read = (
            Some("/usr/local/bin/node".to_string()),
            vec!["node".to_string(), "/opt/codex/bin/codex.js".to_string()],
        );
        let (names, whole) = named_line(vec![Some(read)], "", true);
        assert!(
            whole && names.iter().any(|name| name == "codex"),
            "{names:?}"
        );
        assert_eq!(named_line(Vec::new(), "", true), (Vec::new(), false));
    }

    /// No birth time is known: nothing refutes a Toolhelp parent link.
    fn unknown(_pid: u32) -> Option<u64> {
        None
    }

    /// Windows keeps no foreground process group, so who holds a ConPTY is
    /// read off the process tree the way a job-control shell would have
    /// said it. The HOLDER is the command the shell runs — its direct
    /// child, as a unix job leader is — however deep that command's own
    /// tools go, and the shell itself when nothing runs. An agent's tool
    /// taking over the holder is what declared a working agent gone
    /// (WIN16). The line below the holder, down to the deepest descendant
    /// (newest among equals), is named after it for display only: a
    /// launcher often has the real program under it. A stranger in the
    /// table is ignored, and a cycle in a stale snapshot cannot spin.
    #[test]
    fn a_conpty_is_held_by_the_command_its_shell_runs_and_named_down_that_line() {
        let shell = 100;
        // shell 100 → claude 200 → tool 300 → (310, 320); a stranger 900 → 910.
        let table = [
            (200, 100),
            (300, 200),
            (310, 300),
            (320, 300),
            (900, 1),
            (910, 900),
        ];
        let running = conpty_foreground(shell, &table, unknown);
        assert_eq!(
            running.holder, 200,
            "the command the shell runs, not the tool under it"
        );
        assert_eq!(running.named, [200, 300, 320]);

        let idle = conpty_foreground(shell, &[(900, 1)], unknown);
        assert_eq!(idle.holder, shell, "no descendant: the shell holds it");
        assert_eq!(idle.named, [shell]);

        let cycle = conpty_foreground(shell, &[(200, 100), (100, 200)], unknown);
        assert_eq!(cycle.holder, 200);
        assert_eq!(cycle.named, [200]);
    }

    /// Toolhelp reports the pid a process was STARTED by, and Windows
    /// reuses pids: an orphan whose exited launcher's pid now belongs to
    /// this pane's shell must not read as the shell's command. A child is
    /// never older than its parent, so a link the birth times refute is no
    /// link; a birth nobody could read refutes nothing.
    #[test]
    fn an_orphan_under_a_recycled_pid_is_not_the_shells_command() {
        let born = |pid| match pid {
            100 => Some(50),
            200 => Some(60),
            700 => Some(10),
            710 => Some(12),
            _ => None,
        };
        // 700 (born 10) was started by an exited pid 100, long before this
        // shell (born 50) was given the same pid.
        let orphaned = conpty_foreground(100, &[(700, 100), (710, 700)], born);
        assert_eq!(orphaned.holder, 100);
        assert_eq!(orphaned.named, [100]);
        // A real child beside the orphan is still the shell's command.
        let beside = conpty_foreground(100, &[(700, 100), (200, 100)], born);
        assert_eq!(beside.holder, 200);
        assert_eq!(beside.named, [200]);
        // Unreadable births keep the link.
        let unread = conpty_foreground(100, &[(800, 100)], born);
        assert_eq!(unread.holder, 800);
    }

    /// A Windows image path ends in the platform's executable suffix, and
    /// the agent table names programs without one (`claude`, `zo`). The
    /// name a program is called by drops that suffix — whatever its case,
    /// since the file system ignores case — and only that suffix, so a
    /// dotted name elsewhere and a unix name (suffix "") come back whole.
    #[test]
    fn a_program_is_called_by_its_name_without_the_executable_suffix() {
        assert_eq!(program_name("claude.exe", ".exe"), "claude");
        assert_eq!(program_name("ZO.EXE", ".exe"), "ZO");
        assert_eq!(
            program_name("C:\\Users\\x\\.local\\bin\\claude.exe", ".exe"),
            "claude"
        );
        assert_eq!(program_name("node.js", ".exe"), "node.js");
        assert_eq!(
            program_name(".exe", ".exe"),
            ".exe",
            "a bare suffix is a name"
        );
        assert_eq!(program_name("/usr/local/bin/claude", ""), "claude");
        assert_eq!(program_name("2.1.232", ""), "2.1.232");
    }

    /// A Windows pane's foreground is a LINE of processes (the shell's child
    /// down to the leaf). Every process's own names come before any process's
    /// directories: the sweep takes the first name the agent table knows, and
    /// a launcher's install path (nvm under a user named `pi`) must never
    /// outrank the program it launched (`codex`) — `pi` is an agent id too.
    #[test]
    fn every_process_is_named_before_any_directory_on_the_line() {
        let line = [
            (
                Some("C:\\Users\\pi\\AppData\\Roaming\\nvm\\v20.1.0\\node.exe".to_string()),
                vec![
                    "node".to_string(),
                    "C:\\Users\\pi\\AppData\\Roaming\\npm\\node_modules\\@openai\\codex\\bin\\codex.js"
                        .to_string(),
                ],
            ),
            (
                Some("C:\\Users\\pi\\AppData\\Local\\codex\\codex.exe".to_string()),
                vec!["codex".to_string()],
            ),
        ];
        let names = ordered_names(&line, ".exe");
        let first_dir = names
            .iter()
            .position(|name| name == "pi")
            .expect("the user dir is named");
        for own in ["node", "codex.js", "codex"] {
            let at = names.iter().position(|name| name == own).expect(own);
            assert!(at < first_dir, "{own} came after a directory: {names:?}");
        }
        // One process (the unix foreground leader) keeps its old order.
        let one = ordered_names(&line[1..], ".exe");
        assert_eq!(one.first().map(String::as_str), Some("codex"));
    }

    /// The measured KERN_PROCARGS2 shape round-trips: argc, exec path, NUL
    /// padding, then the argv words the detection roads live on — the
    /// interpreter case (`node …/codex.js`) keeps its script in argv[1].
    #[test]
    fn the_invocation_words_survive_the_procargs_layout() {
        let mut buffer = Vec::new();
        buffer.extend_from_slice(&3i32.to_ne_bytes());
        buffer.extend_from_slice(b"/opt/homebrew/bin/node\0\0\0");
        buffer.extend_from_slice(b"node\0/opt/homebrew/bin/codex\0--flag\0");
        assert_eq!(argv_in(&buffer), ["node", "/opt/homebrew/bin/codex"]);
        // Garbage answers nothing rather than something wrong.
        assert!(argv_in(b"").is_empty());
        assert!(argv_in(&[0, 0, 0, 0]).is_empty());
    }
}

#[cfg(test)]
mod queue_bound_tests {
    use super::{
        LOCAL_OUTPUT_QUEUE_BUDGETS, LOCAL_OUTPUT_QUEUE_CHUNKS, PTY_OUTPUT_BYTE_BUDGET,
        PTY_READ_CHUNK_BYTES,
    };

    /// The queue's memory bound is the named constants and nothing else: two
    /// pump budgets, counted in reader chunks, with no remainder lost to the
    /// division — so "how much output may a flood park in this process" has
    /// one derivable answer instead of an arbitrary number.
    #[test]
    fn the_local_output_queue_holds_exactly_two_pump_budgets() {
        assert_eq!(
            PTY_OUTPUT_BYTE_BUDGET % PTY_READ_CHUNK_BYTES,
            0,
            "a budget that does not divide into chunks silently rounds the bound"
        );
        assert_eq!(
            LOCAL_OUTPUT_QUEUE_CHUNKS * PTY_READ_CHUNK_BYTES,
            LOCAL_OUTPUT_QUEUE_BUDGETS * PTY_OUTPUT_BYTE_BUDGET,
            "the queue capacity drifted from the budgets it is derived from"
        );
    }
}

#[cfg(all(test, unix))]
mod queue_tests {
    use super::{PTY_OUTPUT_BYTE_BUDGET, PTY_READ_CHUNK_BYTES, PtyLane};
    use crate::{Rgb, TerminalColors};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn screen_text(lane: &PtyLane) -> String {
        lane.terminal()
            .grid()
            .snapshot()
            .rows
            .iter()
            .map(|row| row.cells.iter().map(|cell| cell.ch).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// A flood four times the queue's whole bound, produced while nothing
    /// pumps: the bounded queue must stall the child rather than grow, and
    /// then hand over EVERY byte once pumping starts — nothing dropped, no
    /// pump over its budget, and the EOF still arriving at the end. The
    /// trailing marker is the proof of completeness: it is the last thing
    /// the child writes, so it only reaches the screen if everything ahead
    /// of it did.
    #[test]
    fn a_flood_is_deferred_whole_and_ends_in_an_eof() {
        let mut lane = PtyLane::spawn(
            "/bin/sh",
            &[
                "-c".to_string(),
                // ~2MB of x — four times LOCAL_OUTPUT_QUEUE_CHUNKS ×
                // PTY_READ_CHUNK_BYTES — then the marker, then exit.
                "head -c 2000000 /dev/zero | tr '\\0' x; printf FLOODEND".to_string(),
            ],
            None,
            &[],
            24,
            80,
        )
        .expect("a shell to run");

        // Let the child get as far ahead as the queue and the kernel allow,
        // which is the state the bound exists for. The assertions below do
        // not depend on this pause; it only makes the scenario the hard one.
        std::thread::sleep(Duration::from_millis(300));

        let deadline = Instant::now() + Duration::from_secs(20);
        let mut moved = 0usize;
        let mut largest = 0usize;
        let mut ended = false;
        while Instant::now() < deadline {
            let pumped = lane.pump();
            moved += pumped.bytes;
            largest = largest.max(pumped.bytes);
            if pumped.ended {
                ended = true;
                break;
            }
            if pumped.bytes == 0 {
                std::thread::sleep(Duration::from_millis(5));
            }
        }

        assert!(ended, "the EOF never arrived through the bounded queue");
        assert!(
            largest <= PTY_OUTPUT_BYTE_BUDGET,
            "one pump moved {largest} bytes and every other pane waited for it"
        );
        assert!(
            moved >= 2_000_000 + "FLOODEND".len(),
            "bytes were dropped under backpressure: {moved} arrived"
        );
        assert!(
            screen_text(&lane).contains("FLOODEND"),
            "the child's last write never reached the screen, so something \
             ahead of it was lost"
        );
    }

    /// The delegation is real: `pump_with_budget` turns the same drain in
    /// caller-sized steps, overshooting by at most one reader chunk — the
    /// overshoot [`PtyLane::pump`]'s own documentation grants itself.
    #[test]
    fn a_caller_budget_turns_the_same_drain_in_small_steps() {
        let mut lane = PtyLane::spawn(
            "/bin/sh",
            &[
                "-c".to_string(),
                "head -c 100000 /dev/zero | tr '\\0' y; printf SMALLEND".to_string(),
            ],
            None,
            &[],
            24,
            80,
        )
        .expect("a shell to run");

        let deadline = Instant::now() + Duration::from_secs(20);
        let mut ended = false;
        while Instant::now() < deadline {
            let pumped = lane.pump_with_budget(PTY_READ_CHUNK_BYTES);
            assert!(
                pumped.bytes <= PTY_READ_CHUNK_BYTES * 2,
                "a {}-byte budget moved {} bytes — the overshoot grew past \
                 one chunk",
                PTY_READ_CHUNK_BYTES,
                pumped.bytes
            );
            if pumped.ended {
                ended = true;
                break;
            }
            if pumped.bytes == 0 {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        assert!(ended, "the EOF never arrived through the small budget");
        assert!(screen_text(&lane).contains("SMALLEND"));
    }

    /// Input keeps its order across the writer thread and the bounded output
    /// road: three lines go in one after another, and the child hands them
    /// back in the order they were typed. Echo is turned off so the only
    /// copy on screen is the child's own, and EOF ends the run so the whole
    /// exchange is read rather than a prefix.
    #[test]
    fn typed_lines_come_back_in_the_order_they_went_in() {
        let mut lane = PtyLane::spawn(
            "/bin/sh",
            &[
                "-c".to_string(),
                "stty -echo; cat; printf ORDEREND".to_string(),
            ],
            None,
            &[],
            24,
            80,
        )
        .expect("a shell to run");

        for line in ["first\n", "second\n", "third\n", "\x04"] {
            lane.write_input(line.as_bytes())
                .expect("the child takes input");
        }

        let deadline = Instant::now() + Duration::from_secs(20);
        let said = loop {
            let pumped = lane.pump();
            let said = screen_text(&lane);
            if said.contains("ORDEREND") || pumped.ended || Instant::now() >= deadline {
                break said;
            }
            std::thread::sleep(Duration::from_millis(5));
        };

        let first = said.find("first").expect("the first line came back");
        let second = said.find("second").expect("the second line came back");
        let third = said.find("third").expect("the third line came back");
        assert!(
            first < second && second < third,
            "typed lines came back out of order:\n{said}"
        );
    }

    /// The exact bytes the window writes while a fresh TUI asks its startup
    /// questions. The test child emits the same cursor-position and OSC 10/11
    /// probes measured from Zo; the tape proves the automatic road contains
    /// protocol replies, never Ctrl-C or Ctrl-D.
    #[test]
    fn a_fresh_tui_startup_tape_contains_no_quit_controls() {
        let mut lane = PtyLane::spawn(
            "/bin/sh",
            &[
                "-c".to_string(),
                "stty raw -echo; printf '\\033[6n\\033]10;?\\033\\\\\\033]11;?\\033\\\\'; sleep 2"
                    .to_string(),
            ],
            None,
            &[],
            24,
            96,
        )
        .expect("spawn startup probe fixture");
        lane.terminal_mut()
            .grid_mut()
            .set_colors(Arc::new(TerminalColors::with_standard_tail(
                Rgb(230, 230, 230),
                Rgb(20, 20, 20),
                Rgb(230, 230, 230),
                [Rgb(0, 0, 0); 16],
            )));

        let deadline = Instant::now() + Duration::from_secs(1);
        while lane.input_tape().len() < 2 && Instant::now() < deadline {
            lane.pump();
            std::thread::sleep(Duration::from_millis(5));
        }
        let tape = lane.input_tape().concat();

        assert!(
            tape.windows(b"\x1b[1;1R".len())
                .any(|window| window == b"\x1b[1;1R"),
            "the cursor-position reply never crossed the tape: {tape:?}"
        );
        assert!(
            tape.windows(b"\x1b]10;rgb:".len())
                .any(|window| window == b"\x1b]10;rgb:"),
            "the palette reply never crossed the tape: {tape:?}"
        );
        assert!(
            tape.iter().all(|byte| *byte >= 0x20 || *byte == 0x1b),
            "an automatic startup reply contains a non-protocol C0 byte: {tape:?}"
        );
    }
}

#[cfg(all(test, unix))]
mod answer_tests {
    use super::PtyLane;
    use std::time::{Duration, Instant};

    /// Room for a child's first words to leave it and reach the reader thread
    /// before anybody writes, on a machine busy building something else.
    const SETTLE: Duration = Duration::from_secs(1);
    /// How long a test waits for what it is sure will come.
    const PATIENCE: Duration = Duration::from_secs(20);

    fn spawned(script: &str) -> PtyLane {
        PtyLane::spawn(
            "/bin/sh",
            &["-c".to_string(), script.to_string()],
            None,
            &[],
            24,
            80,
        )
        .expect("a shell to run")
    }

    fn shows(lane: &PtyLane, text: &str) -> bool {
        lane.terminal().grid().snapshot().rows.iter().any(|row| {
            row.cells
                .iter()
                .map(|cell| cell.ch)
                .collect::<String>()
                .contains(text)
        })
    }

    /// A write's answer is the first output that ARRIVED after it — not the
    /// first parsed after it. The child prints, and only reads its input two
    /// seconds later; the write goes once the print has had time to arrive,
    /// so the pump right after the write parses output the child sent before
    /// anybody wrote. Then the child reads the key and hands it back, and that
    /// is the answer.
    #[test]
    fn output_that_arrived_before_a_write_is_not_its_answer_and_output_after_it_is() {
        let mut lane = spawned("stty raw -echo; printf EARLY; sleep 2; head -c 1; sleep 5");
        std::thread::sleep(SETTLE);
        lane.write_input(b"k").expect("the child takes input");
        let early = lane.pump();
        assert!(
            shows(&lane, "EARLY"),
            "the child's first words had not arrived a second after it started"
        );
        assert!(
            !early.answered,
            "output that arrived before the write was taken for its answer"
        );
        assert!(
            early.unanswered_since.is_some(),
            "the write stopped waiting before the child said anything after it"
        );

        let deadline = Instant::now() + PATIENCE;
        let mut answered = false;
        while !shows(&lane, "k") && Instant::now() < deadline {
            answered |= lane.pump().answered;
            std::thread::sleep(Duration::from_millis(5));
        }
        answered |= lane.pump().answered;
        assert!(shows(&lane, "k"), "the child never handed the key back");
        assert!(answered, "the child's answer to the write was never seen");
        assert_eq!(
            lane.pump().unanswered_since,
            None,
            "an answered write is still waiting"
        );
    }

    /// The terminal answers a program's own questions by writing to it — here
    /// a cursor report — and nobody is waiting for the program to say anything
    /// back. That write must not start a wait the window's pump would chase.
    #[test]
    fn a_reply_the_terminal_writes_for_the_child_waits_for_no_answer() {
        let mut lane = spawned("stty raw -echo; printf '\\033[6n'; sleep 5");
        let deadline = Instant::now() + PATIENCE;
        while lane.input_tape().is_empty() && Instant::now() < deadline {
            lane.pump();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            !lane.input_tape().is_empty(),
            "the cursor report was never answered"
        );
        assert_eq!(
            lane.pump().unanswered_since,
            None,
            "the terminal's own reply is waiting for the child to answer it"
        );
    }
}

#[cfg(all(test, unix))]
mod ending_tests {
    use super::PtyLane;
    use std::time::{Duration, Instant};

    /// What the output side is actually worth as a death notice.
    ///
    /// [`PtyLane::has_ended`] reports that the pty closed and
    /// [`PtyLane::try_wait`] reports that the child was reaped, and this file
    /// says in prose that they are different questions. The window's reaper
    /// leans on the first one, so how far apart they can drift is a fact worth
    /// holding still rather than assuming.
    ///
    /// The hardest shape for the output side is a shell that leaves something
    /// running behind it: the grandchild inherits the pty and goes on holding
    /// it after the shell has exited, so an end-of-file that waited for the
    /// LAST holder would not arrive for as long as the grandchild lived. It
    /// arrives anyway — the direct child is the terminal's session leader, and
    /// its exit is what the master is told about. MEASURED, not assumed: the
    /// opposite was assumed first, and this test is what said otherwise.
    ///
    /// So a pane left standing empty on this platform is NOT explained by a
    /// grandchild holding the pty, and looking there again is looking in a
    /// place this test has already been.
    #[test]
    fn the_output_side_closes_when_the_shell_exits_even_behind_a_grandchild() {
        let mut lane = PtyLane::spawn(
            "/bin/sh",
            &["-c".to_string(), "sleep 4 & exit 7".to_string()],
            None,
            &[],
            24,
            80,
        )
        .expect("a shell to run");

        let deadline = Instant::now() + Duration::from_secs(3);
        let mut code = None;
        let mut closed = false;
        while Instant::now() < deadline {
            closed |= lane.pump().ended;
            if let Ok(Some(status)) = lane.try_wait() {
                code = Some(status);
            }
            if closed && code.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        assert_eq!(code, Some(7), "the child's exit status never arrived");
        assert!(
            closed && lane.has_ended(),
            "the output side did NOT close while a grandchild held the pty —              which means the window's reaper can miss a death, and the second              question has to be asked of every quiet shell rather than only              after this one answers"
        );
    }

    /// A pane says what IT is about colour, and what started this window does
    /// not get a vote.
    ///
    /// Measured in the child's own environment rather than read off the source,
    /// because the whole defect was invisible in the source: `TERM` and
    /// `COLORTERM` were being set correctly on the launch path all along, and an
    /// inherited `NO_COLOR=1` silently beat both. Every agent went monochrome,
    /// the saved scrollback held not one SGR sequence, and it read as the
    /// renderer losing the palette while the renderer was innocent.
    ///
    /// The window is the parent here, so this sets the hostile variables on
    /// ITSELF first — that is the actual situation being tested, and asserting
    /// against an environment we did not poison would prove nothing.
    #[test]
    fn a_pane_decides_its_own_colour_and_says_what_terminal_it_is() {
        // SAFETY: single-threaded within this test's own process, and the
        // values are restored below. This is the only way to stage "the window
        // was started by something that disabled colour".
        unsafe {
            std::env::set_var("NO_COLOR", "1");
            std::env::set_var("FORCE_COLOR", "0");
            std::env::set_var("TERM", "screen");
        }

        let mut lane = PtyLane::spawn(
            "/bin/sh",
            &[
                "-c".to_string(),
                "printf 'T=%s C=%s N=[%s] F=[%s]\\n' \"$TERM\" \"$COLORTERM\" \"$NO_COLOR\" \"$FORCE_COLOR\"".to_string(),
            ],
            None,
            &[],
            24,
            80,
        )
        .expect("a shell to run");

        // Read off the SCREEN the pane actually draws — the same surface a
        // person would be looking at.
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut said = String::new();
        while Instant::now() < deadline {
            lane.pump();
            said = lane
                .terminal()
                .grid()
                .snapshot()
                .rows
                .iter()
                .map(|row| row.cells.iter().map(|cell| cell.ch).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n");
            if said.contains("T=") && said.contains("F=[") {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        unsafe {
            std::env::remove_var("NO_COLOR");
            std::env::remove_var("FORCE_COLOR");
            std::env::remove_var("TERM");
        }

        assert!(
            said.contains("N=[]") && said.contains("F=[]"),
            "an inherited colour override reached the child, so every program \
             in every pane draws without colour and the renderer takes the \
             blame: {said:?}"
        );
        assert!(
            said.contains("T=xterm-256color") && said.contains("C=truecolor"),
            "the pane introduced itself as something other than the emulator \
             behind it, so programs pick capabilities this grid does not \
             have — or refuse ones it does: {said:?}"
        );
    }
}

/// Now, in epoch milliseconds — one read, taken where the fact happens.
fn epoch_ms_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_millis()).unwrap_or(i64::MAX)
        })
}

#[cfg(test)]
mod child_termination_tests {
    use super::kill_known_child;
    use portable_pty::{Child, ChildKiller, ExitStatus};
    use std::io;

    #[derive(Debug, Clone, Copy)]
    enum Observation {
        Running,
        Reaped,
        Unknown,
    }

    #[derive(Debug, Clone)]
    struct Probe {
        observation: Observation,
        signals: usize,
        refuse: bool,
    }

    impl ChildKiller for Probe {
        fn kill(&mut self) -> io::Result<()> {
            self.signals += 1;
            if self.refuse {
                return Err(io::Error::from(io::ErrorKind::PermissionDenied));
            }
            self.observation = Observation::Reaped;
            Ok(())
        }
        fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
            Box::new(self.clone())
        }
    }
    impl Child for Probe {
        fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
            match self.observation {
                Observation::Running => Ok(None),
                Observation::Reaped => Ok(Some(ExitStatus::with_exit_code(0))),
                Observation::Unknown => Err(io::Error::from(io::ErrorKind::Other)),
            }
        }
        fn wait(&mut self) -> io::Result<ExitStatus> {
            Err(io::Error::from(io::ErrorKind::WouldBlock))
        }
        fn process_id(&self) -> Option<u32> {
            Some(4242)
        }
        #[cfg(windows)]
        fn as_raw_handle(&self) -> Option<std::os::windows::io::RawHandle> {
            None
        }
    }
    fn probe(observation: Observation) -> Probe {
        Probe {
            observation,
            signals: 0,
            refuse: false,
        }
    }

    #[test]
    fn a_reaped_child_is_never_signalled_again() {
        let mut child = probe(Observation::Reaped);
        kill_known_child(&mut child).unwrap();
        assert_eq!(
            child.signals, 0,
            "a reaped child's PID may belong to another process"
        );
    }

    #[test]
    fn an_unknown_child_status_cannot_authorize_a_pid_signal() {
        let mut child = probe(Observation::Unknown);
        assert!(kill_known_child(&mut child).is_err());
        assert_eq!(child.signals, 0);
    }

    #[test]
    fn a_live_childs_signal_refusal_reaches_the_caller() {
        let mut child = probe(Observation::Running);
        child.refuse = true;
        assert_eq!(
            kill_known_child(&mut child).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(child.signals, 1);
    }

    #[test]
    fn repeated_termination_after_exit_sends_only_one_signal() {
        let mut child = probe(Observation::Running);
        kill_known_child(&mut child).unwrap();
        kill_known_child(&mut child).unwrap();
        assert_eq!(child.signals, 1);
    }
}

/// What a lane's child gets from its start, held still while the start
/// itself changes (t-73907: on macOS it no longer goes through a fork).
#[cfg(all(test, unix))]
mod launch_shape_tests {
    use super::{GroupPrograms, PtyLane};
    use std::time::{Duration, Instant};

    /// How long a test waits for what it is sure will come.
    const PATIENCE: Duration = Duration::from_secs(20);

    fn spawned(script: &str) -> PtyLane {
        PtyLane::spawn(
            "/bin/sh",
            &["-c".to_string(), script.to_string()],
            None,
            &[],
            24,
            80,
        )
        .expect("a shell to run")
    }

    fn screen(lane: &PtyLane) -> String {
        lane.terminal()
            .grid()
            .snapshot()
            .rows
            .iter()
            .map(|row| row.cells.iter().map(|cell| cell.ch).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Pump until the screen shows `text`, and hand back the line it is on.
    fn line_with(lane: &mut PtyLane, text: &str) -> String {
        let deadline = Instant::now() + PATIENCE;
        loop {
            lane.pump();
            if let Some(line) = screen(lane).lines().find(|line| line.contains(text)) {
                return line.trim().to_string();
            }
            assert!(
                Instant::now() < deadline,
                "{text:?} never came:\n{}",
                screen(lane)
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// The child leads a session of its own and the pty is its controlling
    /// terminal, with its group in front: job control, `/dev/tty` and the
    /// window's own foreground question all stand on these.
    #[test]
    fn the_child_leads_its_own_session_on_its_controlling_terminal() {
        let mut lane = spawned(
            "set -- $(ps -o pid=,pgid=,tpgid= -p $$); \
             if (exec 3</dev/tty) 2>/dev/null; then t=open; else t=none; fi; \
             echo \"SHAPE $1 $2 $3 $t\"; sleep 5",
        );
        let line = line_with(&mut lane, "SHAPE ");
        let pid = lane.pid().expect("a child pid").to_string();
        assert_eq!(
            line,
            format!("SHAPE {pid} {pid} {pid} open"),
            "pid, group, terminal's group, /dev/tty"
        );
        assert_eq!(lane.foreground_is_child(), Some(true));
    }

    /// What a script runs without `exec` stays in the script's group and
    /// is named among the group's other processes, the script itself not
    /// (t-76489 r4), and the platform says who each is — the same process
    /// of this group, session and user before and after its names are read
    /// — so the answer is whole (t-76489 r5). The script says READY only
    /// once `ps` names its `sleep`, so one reading after READY finds it,
    /// waiting on no clock.
    #[test]
    fn a_program_a_script_runs_is_named_among_its_group() {
        let mut lane = spawned(
            "sleep 30 & until ps -o comm= -p $! | grep -q sleep; do :; done; echo READY; wait",
        );
        line_with(&mut lane, "READY");
        let members = lane.foreground_group_programs();
        let GroupPrograms::Whole(named) = &members else {
            panic!("the script's group was not read whole: {members:?}");
        };
        assert!(
            named
                .iter()
                .any(|names| names.iter().any(|name| name == "sleep")),
            "the script's sleep was not named: {members:?}"
        );
        let leader = lane.foreground_programs();
        assert!(
            !named.contains(&leader),
            "the group's leader {leader:?} named among its other processes: {members:?}"
        );
    }

    /// ^C typed into the pane reaches the child as SIGINT.
    #[test]
    fn an_interrupt_typed_reaches_the_child_as_sigint() {
        let mut lane =
            spawned("trap 'echo GOT-INT; exit 3' INT; echo READY; while :; do sleep 0.1; done");
        line_with(&mut lane, "READY");
        lane.write_input(b"\x03").unwrap();
        line_with(&mut lane, "GOT-INT");
    }

    /// The child starts at the pane's size, and a resize reaches it as
    /// SIGWINCH with the new size.
    #[test]
    fn the_window_size_reaches_the_child_and_a_resize_signals_it() {
        let mut lane = spawned(
            "trap 'echo WINCH $(stty size)' WINCH; echo SIZE $(stty size); \
             while :; do sleep 0.1; done",
        );
        assert_eq!(line_with(&mut lane, "SIZE "), "SIZE 24 80");
        lane.resize(30, 100).unwrap();
        assert_eq!(line_with(&mut lane, "WINCH "), "WINCH 30 100");
    }

    /// A descriptor the window has open without close-on-exec does not
    /// reach the child: nothing but the terminal on 0, 1 and 2.
    ///
    /// The probe sits at 100 or above. `/bin/sh` (bash) opens 10 and 11 of
    /// its own before it runs a word, so a probe that landed on one of them —
    /// which tests opening descriptors on other threads decide — read as a
    /// leak that was bash's own (FD-LEAKED once in a parallel run, 10-27).
    #[test]
    fn no_descriptor_but_the_terminal_reaches_the_child() {
        let directory = tempfile::tempdir().unwrap();
        let path =
            std::ffi::CString::new(directory.path().join("held").as_os_str().as_encoded_bytes())
                .unwrap();
        let opened = unsafe { libc::open(path.as_ptr(), libc::O_RDWR | libc::O_CREAT, 0o600) };
        assert!(opened > 2, "open: {}", std::io::Error::last_os_error());
        let held = unsafe { libc::fcntl(opened, libc::F_DUPFD, 100) };
        unsafe { libc::close(opened) };
        assert!(held >= 100, "dup: {}", std::io::Error::last_os_error());
        let flags = unsafe { libc::fcntl(held, libc::F_GETFD) };
        assert_eq!(flags & libc::FD_CLOEXEC, 0, "the probe must be inheritable");

        let mut lane = spawned(&format!(
            "if {{ : <&{held}; }} 2>/dev/null; then echo FD-LEAKED; else echo FD-CLEAN; fi; sleep 5"
        ));
        let line = line_with(&mut lane, "FD-");
        unsafe { libc::close(held) };
        assert_eq!(line, "FD-CLEAN");
    }
}

/// The lane's start on macOS runs no fork in this process (t-73907).
///
/// A fork in a window that runs CEF can stand forever in a libSystem fork
/// handler before its exec, holding a copy of every descriptor the window
/// has open. Counted with a fork handler of our own: `pthread_atfork`'s
/// prepare step runs in the thread that forks, before the fork, and
/// `posix_spawn` runs no fork handler at all. The count is per thread, so a
/// launch on another test's thread does not land in this one's.
#[cfg(all(test, target_os = "macos"))]
mod fork_free_tests {
    use super::PtyLane;
    use std::cell::Cell;
    use std::time::{Duration, Instant};

    unsafe extern "C" {
        fn pthread_atfork(
            prepare: Option<unsafe extern "C" fn()>,
            parent: Option<unsafe extern "C" fn()>,
            child: Option<unsafe extern "C" fn()>,
        ) -> libc::c_int;
    }

    thread_local! {
        static FORKS: Cell<u32> = const { Cell::new(0) };
    }

    unsafe extern "C" fn counted() {
        FORKS.with(|forks| forks.set(forks.get() + 1));
    }

    fn count_forks() {
        static REGISTERED: std::sync::Once = std::sync::Once::new();
        REGISTERED.call_once(|| {
            assert_eq!(unsafe { pthread_atfork(Some(counted), None, None) }, 0);
        });
    }

    /// The control: a `pre_exec` closure is what makes std fork, and the
    /// counter must see that fork, or the zero below proves nothing.
    #[test]
    fn the_counter_sees_a_fork() {
        use std::os::unix::process::CommandExt as _;

        count_forks();
        let before = FORKS.with(Cell::get);
        let mut command = std::process::Command::new("/usr/bin/true");
        unsafe { command.pre_exec(|| Ok(())) };
        assert!(command.status().unwrap().success());
        assert_eq!(FORKS.with(Cell::get), before + 1, "a fork went uncounted");
    }

    #[test]
    fn a_lane_starts_its_child_without_forking_this_process() {
        count_forks();
        let before = FORKS.with(Cell::get);
        let mut lane = PtyLane::spawn(
            "/bin/sh",
            &["-c".to_string(), "echo STARTED; sleep 5".to_string()],
            None,
            &[],
            24,
            80,
        )
        .expect("a shell to run");
        let forks = FORKS.with(Cell::get) - before;

        let deadline = Instant::now() + Duration::from_secs(20);
        while !lane.terminal().grid().snapshot().rows.iter().any(|row| {
            row.cells
                .iter()
                .map(|cell| cell.ch)
                .collect::<String>()
                .contains("STARTED")
        }) {
            assert!(Instant::now() < deadline, "the child never ran");
            lane.pump();
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(forks, 0, "the lane's start forked this process");
    }
}

/// Whose names a foreground group's reading counts (t-76489 r5): the
/// platform's answers, faked, in the orders a pid can change under it.
#[cfg(all(test, unix))]
mod group_tests {
    use std::cell::RefCell;
    use std::collections::{HashMap, VecDeque};

    use super::{FOREGROUND_GROUP_NAMED, GroupPrograms, ProcessIdentity, group_programs};

    const GROUP: i32 = 500;
    const SESSION: i32 = 400;
    const USER: u32 = 501;

    /// A process of the terminal's group, session and user, started at
    /// `started`.
    fn ours(started: u64) -> ProcessIdentity {
        ProcessIdentity {
            group: GROUP,
            session: SESSION,
            user: USER,
            started,
        }
    }

    /// A Claude Code another terminal runs: another group and session.
    fn afar(started: u64) -> ProcessIdentity {
        ProcessIdentity {
            group: 900,
            session: 800,
            user: USER,
            started,
        }
    }

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    fn claude() -> Vec<String> {
        names(&["2.1.232", "versions", "claude"])
    }

    /// The OS, faked: each pid answers who it is off its queue, one answer
    /// a question and the last for every question after, and what it is
    /// called off its row — none for a pid with no row.
    #[derive(Default)]
    struct Os {
        identities: RefCell<HashMap<i32, VecDeque<Option<ProcessIdentity>>>>,
        called: HashMap<i32, Vec<String>>,
        asked: RefCell<Vec<i32>>,
    }

    impl Os {
        fn process(
            mut self,
            pid: i32,
            answers: &[Option<ProcessIdentity>],
            called: Vec<String>,
        ) -> Self {
            self.identities
                .get_mut()
                .insert(pid, answers.iter().copied().collect());
            if !called.is_empty() {
                self.called.insert(pid, called);
            }
            self
        }

        fn identity(&self, pid: i32) -> Option<ProcessIdentity> {
            let mut all = self.identities.borrow_mut();
            let answers = all.get_mut(&pid)?;
            if answers.len() > 1 {
                answers.pop_front().flatten()
            } else {
                answers.front().copied().flatten()
            }
        }

        fn read(&self, listed: Option<(Vec<i32>, bool)>) -> GroupPrograms {
            group_programs(
                (GROUP, SESSION, USER),
                listed,
                &|pid| self.identity(pid),
                &|pid| {
                    self.asked.borrow_mut().push(pid);
                    self.called.get(&pid).cloned()
                },
            )
        }
    }

    /// A group read whole names every member but its leader, whose pid is
    /// the group's.
    #[test]
    fn a_group_read_whole_names_every_member_but_its_leader() {
        let os = Os::default()
            .process(GROUP, &[Some(ours(1))], names(&["sh"]))
            .process(501, &[Some(ours(2))], names(&["node"]))
            .process(502, &[Some(ours(3))], claude());
        assert_eq!(
            os.read(Some((vec![GROUP, 501, 502], true))),
            GroupPrograms::Whole(vec![names(&["node"]), claude()])
        );
        assert_eq!(
            os.read(Some((vec![GROUP], true))),
            GroupPrograms::Whole(Vec::new())
        );
    }

    /// A group that cannot be listed is unread, and one listed in part is
    /// partial: the member not listed may be the agent.
    #[test]
    fn a_group_listed_in_part_or_not_at_all_is_not_read_whole() {
        let os = Os::default().process(501, &[Some(ours(2))], names(&["node"]));
        assert_eq!(os.read(None), GroupPrograms::Unread);
        assert_eq!(GroupPrograms::Unread.named(), &[] as &[Vec<String>]);
        assert_eq!(
            os.read(Some((vec![GROUP, 501], false))),
            GroupPrograms::Partial(vec![names(&["node"])])
        );
    }

    /// A member that will not say who it is, or what it is called, leaves
    /// the answer partial: the others it names still count.
    #[test]
    fn a_member_that_will_not_say_leaves_the_answer_partial() {
        let os = Os::default()
            .process(501, &[Some(ours(2))], names(&["node"]))
            .process(502, &[None], claude())
            .process(503, &[Some(ours(4))], Vec::new())
            .process(504, &[Some(ours(5)), None], claude());
        assert_eq!(
            os.read(Some((vec![GROUP, 501, 502, 503, 504], true))),
            GroupPrograms::Partial(vec![names(&["node"])])
        );
    }

    /// t-76489 r6, the codex review's third finding of r5: libproc answers
    /// 0 when it fails as well as a negative, and a group its terminal
    /// names has a member to list — so 0 lists no group, and only a list
    /// that left room is all of it. Before, 0 was an empty group read
    /// whole, and a launcher's pane held nothing.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_listing_of_nothing_or_of_a_failure_is_no_group_read() {
        use super::listed_pids;
        let size = i32::try_from(std::mem::size_of::<i32>()).expect("an int's size");
        let room = 4 * size;
        assert_eq!(listed_pids(0, room, vec![0; 4]), None, "0 and ENOMEM");
        assert_eq!(listed_pids(-1, room, vec![0; 4]), None);
        assert_eq!(
            listed_pids(2 * size, room, vec![GROUP + 1, GROUP, 0, 0]),
            Some((vec![GROUP, GROUP + 1], true))
        );
        assert_eq!(
            listed_pids(room, room, vec![GROUP, GROUP + 1, GROUP + 2, GROUP + 3]),
            Some((vec![GROUP, GROUP + 1, GROUP + 2, GROUP + 3], false)),
            "a list that filled its room may have left some out"
        );
        let os = Os::default().process(GROUP, &[Some(ours(1))], names(&["sh"]));
        assert_eq!(
            os.read(listed_pids(0, room, vec![0; 4])),
            GroupPrograms::Unread
        );
    }

    /// The codex review's second finding of r4: a member listed in pane A's
    /// group ends, and its pid is taken by the Claude Code another terminal
    /// runs — before its names are read, or while they are. Its names are
    /// that Claude's, and they are not A's: the answer is partial and names
    /// no claude. A pid that came back in A's own group as another start is
    /// not the member listed either. Before, the names were read with no
    /// question of who answered them, and A held B's conversation.
    #[test]
    fn a_pid_reused_by_another_groups_claude_is_not_named() {
        for (answers, scene) in [
            (
                vec![Some(ours(2)), Some(afar(9))],
                "reused while its names were read",
            ),
            (vec![Some(afar(9))], "reused before its names were read"),
            (
                vec![Some(ours(2)), Some(ours(9))],
                "reused in the group by another start",
            ),
        ] {
            let os = Os::default()
                .process(501, &[Some(ours(1))], names(&["make"]))
                .process(502, &answers, claude());
            let read = os.read(Some((vec![GROUP, 501, 502], true)));
            assert_eq!(
                read,
                GroupPrograms::Partial(vec![names(&["make"])]),
                "{scene}"
            );
        }
    }

    /// A member that leaves the group while it is read, or that is another
    /// person's or another session's, is not named.
    #[test]
    fn a_member_of_another_group_user_or_session_is_not_named() {
        let moved = ProcessIdentity {
            group: 900,
            ..ours(2)
        };
        let root = ProcessIdentity { user: 0, ..ours(2) };
        let elsewhere = ProcessIdentity {
            session: 800,
            ..ours(2)
        };
        for (answers, scene) in [
            (
                vec![Some(ours(2)), Some(moved)],
                "left the group while read",
            ),
            (vec![Some(root)], "another user's"),
            (vec![Some(elsewhere)], "another session's"),
        ] {
            let os = Os::default().process(502, &answers, claude());
            assert_eq!(
                os.read(Some((vec![GROUP, 502], true))),
                GroupPrograms::Partial(Vec::new()),
                "{scene}"
            );
        }
    }

    /// The reading stops at its bound, and a group past it is read in part:
    /// the agent past the bound is neither asked nor known absent.
    #[test]
    fn a_group_past_the_bound_is_read_in_part() {
        let bound = i32::try_from(FOREGROUND_GROUP_NAMED).expect("a small bound");
        let mut os = Os::default();
        for pid in 1..=bound {
            os = os.process(
                GROUP + pid,
                &[Some(ours(u64::from(pid.unsigned_abs())))],
                names(&["cc"]),
            );
        }
        let within: Vec<i32> = (0..=bound).map(|pid| GROUP + pid).collect();
        assert_eq!(
            os.read(Some((within.clone(), true))),
            GroupPrograms::Whole(vec![names(&["cc"]); FOREGROUND_GROUP_NAMED])
        );
        let agent = GROUP + bound + 1;
        os = os.process(agent, &[Some(ours(99))], claude());
        os.asked.borrow_mut().clear();
        let past: Vec<i32> = within.into_iter().chain([agent]).collect();
        assert_eq!(
            os.read(Some((past, true))),
            GroupPrograms::Partial(vec![names(&["cc"]); FOREGROUND_GROUP_NAMED])
        );
        assert!(
            !os.asked.borrow().contains(&agent),
            "the member past the bound was read"
        );
    }
}

#[cfg(all(test, unix))]
mod open_tests {
    use super::{EREDRIVEOPEN, PTY_OPEN_ATTEMPTS, PtyError, open_pty};
    use std::cell::Cell;
    use std::io;
    use std::rc::Rc;

    /// `errno` worded exactly as `portable_pty` words a failed `openpty`
    /// (`src/unix.rs`, `bail!("failed to openpty: {:?}", …)`).
    fn worded(errno: i32) -> String {
        format!(
            "failed to openpty: {:?}",
            io::Error::from_raw_os_error(errno)
        )
    }

    /// A pty system whose `n`th ask (from 1) fails with `failure(n)`, and
    /// opens where that is `None`. It counts every ask.
    fn pty_system(
        failure: impl Fn(usize) -> Option<String>,
    ) -> (impl FnMut() -> Result<(), String>, Rc<Cell<usize>>) {
        let asked = Rc::new(Cell::new(0));
        let counted = Rc::clone(&asked);
        let open = move || {
            counted.set(counted.get() + 1);
            match failure(counted.get()) {
                Some(reason) => Err(reason),
                None => Ok(()),
            }
        };
        (open, asked)
    }

    /// A pty system that fails its first `failures` opens with `errno`, and
    /// opens after that.
    fn opens_after(
        failures: usize,
        errno: i32,
    ) -> (impl FnMut() -> Result<(), String>, Rc<Cell<usize>>) {
        pty_system(move |n| (n <= failures).then(|| worded(errno)))
    }

    /// A pty lost to another open at the same moment is asked for again,
    /// and the lane opens.
    #[test]
    fn a_pty_lost_to_a_concurrent_open_is_asked_for_again() {
        for errno in [EREDRIVEOPEN, libc::ENXIO, libc::EAGAIN] {
            let failures = PTY_OPEN_ATTEMPTS - 1;
            let (open, asked) = opens_after(failures, errno);
            assert!(
                open_pty(open).is_ok(),
                "errno {errno}: the pty never opened"
            );
            assert_eq!(asked.get(), failures + 1, "errno {errno}");
        }
    }

    /// An error that asking again will not change is the answer at once.
    #[test]
    fn an_error_that_will_not_pass_is_not_asked_again() {
        for errno in [libc::EMFILE, libc::EACCES] {
            let (open, asked) = opens_after(usize::MAX, errno);
            let Err(PtyError::Open(reason)) = open_pty(open) else {
                panic!("errno {errno}: opened, or failed as something else");
            };
            assert!(reason.contains(&format!("code: {errno},")), "{reason}");
            assert_eq!(asked.get(), 1, "errno {errno}");
        }
    }

    /// A race that never clears ends at the bound, with the kernel's own
    /// words.
    #[test]
    fn a_race_that_never_clears_ends_at_the_bound() {
        let (open, asked) = opens_after(usize::MAX, EREDRIVEOPEN);
        let Err(PtyError::Open(reason)) = open_pty(open) else {
            panic!("opened, or failed as something else");
        };
        assert!(
            reason.contains(&format!("code: {EREDRIVEOPEN},")),
            "{reason}"
        );
        assert_eq!(asked.get(), PTY_OPEN_ATTEMPTS);
    }

    /// A race that turns into an error asking will not change ends there,
    /// with that error.
    #[test]
    fn a_race_that_turns_into_a_lasting_error_ends_there() {
        let (open, asked) =
            pty_system(|n| Some(worded(if n <= 2 { EREDRIVEOPEN } else { libc::EMFILE })));
        let Err(PtyError::Open(reason)) = open_pty(open) else {
            panic!("opened, or failed as something else");
        };
        assert!(
            reason.contains(&format!("code: {},", libc::EMFILE)),
            "{reason}"
        );
        assert_eq!(asked.get(), 3);
    }

    /// A failure worded any other way — here the pretty `Debug` of the
    /// very errno a race answers — is not asked again and comes back as it
    /// was: a reworded dependency costs the retry, not the error.
    #[test]
    fn a_failure_worded_otherwise_is_not_asked_again() {
        let reworded = format!(
            "failed to openpty: {:#?}",
            io::Error::from_raw_os_error(EREDRIVEOPEN)
        );
        let (open, asked) = pty_system(|_| Some(reworded.clone()));
        let Err(PtyError::Open(reason)) = open_pty(open) else {
            panic!("opened, or failed as something else");
        };
        assert_eq!(reason, reworded);
        assert_eq!(asked.get(), 1);
    }
}
