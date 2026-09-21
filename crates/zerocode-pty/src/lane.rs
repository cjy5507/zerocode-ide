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

use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};

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
        let pair = native_pty_system()
            .openpty(size)
            .map_err(|error| PtyError::Open(error.to_string()))?;

        let mut command = CommandBuilder::new(program);
        for arg in args {
            command.arg(arg);
        }
        if let Some(cwd) = cwd {
            command.cwd(cwd);
        }
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
                command.env_remove(marker);
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
                command.env(key, value);
            }
        }
        for (key, value) in env {
            if value.is_empty() {
                command.env_remove(key);
            } else {
                command.env(key, value);
            }
        }

        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|error| PtyError::Spawn {
                program: program.to_string(),
                reason: error.to_string(),
            })?;
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

    #[cfg(test)]
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
    /// [`PtyError::Io`] if the signal could not be delivered — which includes
    /// the ordinary case of a child that already exited, so check
    /// [`Self::try_wait`] first when the two need telling apart.
    pub fn kill(&mut self) -> Result<(), PtyError> {
        self.child.kill().map_err(PtyError::Io)
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
    /// process group: `portable_pty` spawns through `setsid`, so the child
    /// leads the session and its group id equals its pid. Nothing needs to be
    /// recorded at spawn for this.
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
        let line: Vec<ProcessFacts> = holding
            .into_iter()
            .filter_map(|pid| i32::try_from(pid).ok())
            .map(process_facts)
            .collect();
        ordered_names(&line, std::env::consts::EXE_SUFFIX)
    }
}

/// What the OS says about one process: its image path and its argv.
type ProcessFacts = (Option<String>, Vec<String>);

fn process_facts(pid: i32) -> ProcessFacts {
    (executable_path(pid), argv_of(pid))
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
        let mut entry = PROCESSENTRY32W::default();
        entry.dwSize = u32::try_from(std::mem::size_of::<PROCESSENTRY32W>()).unwrap_or(0);
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
/// The result is ignored on purpose: a child that already exited is the common
/// case, and in a destructor there is nobody left to tell.
impl Drop for PtyLane {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

#[cfg(test)]
mod foreground_tests {
    use super::{argv_in, conpty_foreground, ordered_names, program_name};

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
