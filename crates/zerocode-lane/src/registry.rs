//! Every lane the window is holding, and what changed about them.
//!
//! One terminal transport per lane is easy; eight of them feeding one window is where it
//! gets expensive, and this is the layer that decides what the view is allowed
//! to cost. Two rules do most of that work.
//!
//! **Only the focused lane produces cells.** The stage shows one lane at a time
//! — the rest are rows on the spine, and a row needs a state and a title, not a
//! screen. So background lanes are pumped (their scrollback must be current the
//! moment someone switches to them) but never serialized. That is the whole
//! reason [`GridDelta`] exists.
//!
//! **Switching focus starts from a snapshot.** The view has never seen the lane
//! it is switching to, so a delta describing "what changed" is meaningless
//! there — it gets the whole screen instead.
//!
//! ## What this layer can and cannot know
//!
//! A pty tells you two things honestly: bytes are flowing, and the child is
//! gone. So [`LaneState::Streaming`], [`LaneState::Idle`] and
//! [`LaneState::Exited`] are inferred here.
//! [`LaneState::AwaitingPermission`] and [`LaneState::Blocked`] are **not** —
//! they arrive on the structured channel as permission frames, and guessing
//! them from output would put the one state a person must never miss on a
//! heuristic. They are set by the caller that reads that channel.

use std::collections::HashMap;

use zerocode_core::{Lane, LaneId, LaneState};
use zerocode_pty::{GridDelta, MouseTracking};

use crate::pty_transport::{PtyHandle, PtyTransport, PtyTransportError};

/// Quiet pumps before a streaming lane is called idle again.
///
/// One quiet pump means nothing — output arrives in bursts and a 16ms frame
/// lands between them constantly, so flipping on the first gap would strobe the
/// rail. A few frames of silence is a pause a person would agree with.
pub const QUIET_PUMPS_BEFORE_IDLE: u8 = 8;

/// Something the view has to react to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaneEvent {
    /// The lane's rail-visible facts changed — its state, its title, or both.
    ///
    /// Emitted for background lanes as well, because the spine paints every
    /// lane whether or not it is on the stage. This is the cheap event: one
    /// small struct, no cells.
    Updated(Lane),
    /// Cells changed on the focused lane.
    ///
    /// Never emitted for a background lane. Obey the shift contract on
    /// [`GridDelta`] when applying it.
    Screen { lane: LaneId, delta: GridDelta },
    /// A program requested a system clipboard write with OSC 52.
    ///
    /// Emitted for every lane, focused or not. This is a side effect rather
    /// than screen state, so hiding cell deltas must never swallow it. The
    /// grid has already validated, bounded, decoded, and coalesced the text.
    ClipboardWrite { lane: LaneId, text: String },
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("no lane with id {0}")]
    UnknownLane(LaneId),
    #[error("lane {0} is already held")]
    DuplicateLane(LaneId),
    #[error(transparent)]
    Pty(#[from] PtyTransportError),
}

/// What closing a lane left behind.
#[derive(Debug)]
pub struct Removed {
    pub lane: Lane,
    /// The closed lane was on the stage, so this is the **whole screen** of the
    /// one that replaced it.
    ///
    /// It is in the return type rather than left for the caller to remember,
    /// because every focus change starts from a snapshot — and a caller that
    /// missed this one would keep applying deltas against the buffer of a lane
    /// that no longer exists.
    pub refocused: Option<LaneEvent>,
    /// The child was alive and would not die. The registry has let go of it
    /// either way, so a process nobody is showing is still out there — which a
    /// caller has to be able to say out loud rather than discover in Activity
    /// Monitor.
    pub orphaned: bool,
}

/// What the pty saw, as a fact rather than a conclusion.
///
/// Kept separate from [`LaneState`] on purpose. A lane stopped on a permission
/// gate keeps redrawing — the prompt blinks, the TUI repaints — so "bytes are
/// arriving" and "the agent is making progress" are different claims. Collapse
/// them and a redraw erases `AwaitingPermission` exactly when a person needs to
/// see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Activity {
    /// Bytes arrived recently.
    Streaming,
    /// Nothing has arrived for [`QUIET_PUMPS_BEFORE_IDLE`] pumps.
    Quiet,
    /// The child is gone.
    Ended,
}

/// Folds a stream of pump observations into an [`Activity`].
///
/// Its own type so the quiet threshold can be tested by calling a function
/// rather than by spawning a process and waiting out eight real frames.
#[derive(Debug, Clone, Copy)]
struct ActivityTracker {
    current: Activity,
    quiet_pumps: u8,
}

impl Default for ActivityTracker {
    fn default() -> Self {
        Self {
            current: Activity::Quiet,
            quiet_pumps: 0,
        }
    }
}

impl ActivityTracker {
    fn observe(&mut self, produced_output: bool, ended: bool) -> Activity {
        self.current = if ended {
            // A closed child stays closed; `pump` keeps reporting it.
            Activity::Ended
        } else if produced_output {
            self.quiet_pumps = 0;
            Activity::Streaming
        } else if self.current == Activity::Streaming {
            self.quiet_pumps = self.quiet_pumps.saturating_add(1);
            if self.quiet_pumps >= QUIET_PUMPS_BEFORE_IDLE {
                Activity::Quiet
            } else {
                Activity::Streaming
            }
        } else {
            self.current
        };
        self.current
    }

    const fn current(self) -> Activity {
        self.current
    }
}

/// Which state a lane is in, given what its output shows and what the
/// structured channel asserted.
///
/// Pure and total on purpose. The precedence *is* the policy, and keeping it as
/// one table — rather than a rule threaded through code that also talks to a
/// pty — is what makes it both readable and testable without a process.
///
/// A dead child outranks everything, because there is nothing left to permit.
/// An open gate outranks output. Only with no gate does activity decide.
const fn resolve_state(activity: Activity, gate: Option<LaneState>) -> LaneState {
    match (activity, gate) {
        (Activity::Ended, _) => LaneState::Exited,
        (_, Some(gate)) => gate,
        (Activity::Streaming, None) => LaneState::Streaming,
        (Activity::Quiet, None) => LaneState::Idle,
    }
}

/// Kill a child unless it already finished, reporting whether it survived.
///
/// Asking first is what keeps "it was already gone" and "it refused to die"
/// different answers. Treating every kill error as success is how a lane gets
/// reported closed while its process keeps running.
fn kill_if_running(pty: &mut dyn PtyTransport) -> bool {
    if matches!(pty.try_wait(), Ok(Some(_))) {
        return false;
    }
    pty.kill().is_err()
}

struct Entry {
    lane: Lane,
    pty: PtyHandle,
    activity: ActivityTracker,
    /// A state the structured channel asserted, which outranks anything output
    /// activity implies. `None` means output decides.
    gate: Option<LaneState>,
}

/// The lanes a window is holding.
///
/// Deliberately synchronous. A pty read blocks and a grid is only ever touched
/// by whoever pumps it, so there is no lock here and no runtime — the caller
/// owns the cadence, which is what lets one thread drive eight lanes.
pub struct LaneRegistry {
    /// Insertion order is the rail's order, so a `Vec` of ids rather than a map
    /// iteration whose order would shuffle under the user.
    order: Vec<LaneId>,
    entries: HashMap<LaneId, Entry>,
    focused: Option<LaneId>,
}

impl Default for LaneRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl LaneRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self {
            order: Vec::new(),
            entries: HashMap::new(),
            focused: None,
        }
    }

    /// Take ownership of a lane and the process drawing it.
    ///
    /// The first lane inserted takes focus, because a window with lanes and no
    /// stage is a window showing nothing.
    ///
    /// An id already held is refused rather than replaced. Replacing would drop
    /// a running child without killing it *and* leave the rail holding the id
    /// twice, so `len()` and `lanes()` would start disagreeing with what the
    /// registry actually has — a bookkeeping bug that surfaces much later, as a
    /// duplicated row nobody can close.
    ///
    /// # Errors
    ///
    /// [`RegistryError::DuplicateLane`] when `lane.id` is already held. The
    /// refused transport is dropped. A transport owns its hosted program and
    /// must clean it up on drop; the local [`zerocode_pty::PtyLane`] does so by
    /// killing its child.
    pub fn insert(&mut self, lane: Lane, pty: PtyHandle) -> Result<LaneId, RegistryError> {
        let id = lane.id;
        if self.entries.contains_key(&id) {
            return Err(RegistryError::DuplicateLane(id));
        }
        self.order.push(id);
        self.entries.insert(
            id,
            Entry {
                lane,
                pty,
                activity: ActivityTracker::default(),
                gate: None,
            },
        );
        if self.focused.is_none() {
            self.focused = Some(id);
        }
        Ok(id)
    }

    /// Close a lane, kill the process behind it, and move the stage if it was
    /// on it.
    ///
    /// Killing is the point: closing a pane must not leave an agent running,
    /// spending tokens for a screen nobody will ever look at.
    ///
    /// The child is asked whether it already exited **before** being killed, so
    /// that "it was already gone" and "it refused to die" stay different
    /// answers. Treating every kill error as success is how a lane gets
    /// reported closed while its process keeps running.
    ///
    /// Moving the stage goes through the same path as any other focus change —
    /// see [`Removed::refocused`].
    pub fn remove(&mut self, id: LaneId) -> Option<Removed> {
        let mut entry = self.entries.remove(&id)?;
        let orphaned = kill_if_running(&mut entry.pty);
        self.order.retain(|held| *held != id);
        let refocused = self.refocus_after_removing(id);

        Some(Removed {
            lane: entry.lane,
            refocused,
            orphaned,
        })
    }

    /// Move the stage off a lane that has just gone away.
    ///
    /// `None` when nothing moved — the closed lane was not on the stage — or
    /// when it was the last one and there is no stage left to hand back.
    fn refocus_after_removing(&mut self, removed: LaneId) -> Option<LaneEvent> {
        if self.focused != Some(removed) {
            return None;
        }
        let Some(next) = self.order.first().copied() else {
            self.focused = None;
            return None;
        };
        self.focus(next).ok()
    }

    #[must_use]
    pub fn focused(&self) -> Option<LaneId> {
        self.focused
    }

    /// Rail order.
    pub fn lanes(&self) -> impl Iterator<Item = &Lane> {
        self.order
            .iter()
            .filter_map(|id| self.entries.get(id).map(|entry| &entry.lane))
    }

    #[must_use]
    pub fn lane(&self, id: LaneId) -> Option<&Lane> {
        self.entries.get(&id).map(|entry| &entry.lane)
    }

    /// Local process id owned by this lane, when its transport has one.
    ///
    /// Resource accounting asks this without exposing the transport itself;
    /// remote lanes deliberately answer `None` rather than a host-relative id
    /// that would refer to an unrelated process on this machine.
    #[must_use]
    pub fn pid(&self, id: LaneId) -> Option<u32> {
        self.entries.get(&id).and_then(|entry| entry.pty.pid())
    }

    /// Whether the lane's program has bracketed paste enabled right now.
    ///
    /// The paste path needs this at the moment of pasting: the flag is the
    /// program's, not the lane's, and it flips whenever a TUI starts or stops.
    #[must_use]
    pub fn bracketed_paste(&self, id: LaneId) -> Option<bool> {
        self.entries
            .get(&id)
            .map(|entry| entry.pty.terminal().grid().bracketed_paste())
    }

    /// How much of the mouse the program in this lane asked to hear about,
    /// and in which encoding. `None` when there is no such lane.
    #[must_use]
    pub fn mouse_modes(&self, id: LaneId) -> Option<(MouseTracking, bool)> {
        self.entries.get(&id).map(|entry| {
            let grid = entry.pty.terminal().grid();
            (grid.mouse_tracking(), grid.mouse_sgr())
        })
    }

    /// The text between two points of the lane's history and screen, for the
    /// window's selection to copy — [`zerocode_pty::TerminalGrid::text_between`]'s rules,
    /// which is the one place they live. `None` when there is no such lane.
    #[must_use]
    pub fn text_between(
        &self,
        id: LaneId,
        from: (usize, usize),
        to: (usize, usize),
    ) -> Option<String> {
        self.entries
            .get(&id)
            .map(|entry| entry.pty.terminal().grid().text_between(from, to))
    }

    /// The lane's screen as plain text, trailing blank rows trimmed.
    ///
    /// The cheap way to see a lane without serializing cells — what a terminal
    /// front-end prints, and what a test compares a replayed [`GridDelta`]
    /// stream against.
    #[must_use]
    pub fn screen_text(&self, id: LaneId) -> Option<String> {
        self.entries
            .get(&id)
            .map(|entry| entry.pty.terminal().grid().visible_text())
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.order.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// Put a lane on the stage, and hand back the whole screen for it.
    ///
    /// The returned event is always a full frame. A background lane has been
    /// accumulating changes nobody collected, so
    /// [`zerocode_pty::TerminalGrid::take_snapshot`] hands over the current screen and advances the pending-delta baseline in
    /// one operation. The next delta therefore describes only changes made
    /// after the snapshot.
    ///
    /// # Errors
    ///
    /// [`RegistryError::UnknownLane`] if no lane holds `id`. The stage does not
    /// move in that case.
    pub fn focus(&mut self, id: LaneId) -> Result<LaneEvent, RegistryError> {
        let entry = self
            .entries
            .get_mut(&id)
            .ok_or(RegistryError::UnknownLane(id))?;
        self.focused = Some(id);

        let delta = entry.pty.terminal_mut().grid_mut().take_snapshot();
        Ok(LaneEvent::Screen { lane: id, delta })
    }

    /// Move every lane forward and report what changed.
    ///
    /// Background lanes are pumped so their scrollback is current the moment
    /// someone focuses them — they simply do not pay to be serialized.
    pub fn pump(&mut self) -> Vec<LaneEvent> {
        let mut events = Vec::new();
        let focused = self.focused;
        for id in &self.order {
            let Some(entry) = self.entries.get_mut(id) else {
                continue;
            };
            let pumped = entry.pty.pump();
            entry.activity.observe(pumped.bytes > 0, pumped.ended);
            if let Some(event) = entry.refresh() {
                events.push(event);
            }
            if let Some(text) = entry
                .pty
                .terminal_mut()
                .grid_mut()
                .take_osc52_clipboard_write()
            {
                events.push(LaneEvent::ClipboardWrite { lane: *id, text });
            }

            if focused == Some(*id)
                && let Some(delta) = entry.pty.terminal_mut().grid_mut().take_delta()
            {
                events.push(LaneEvent::Screen { lane: *id, delta });
            }
        }
        events
    }

    /// Set a state this layer cannot observe — the permission and blocked
    /// states that arrive on the structured channel.
    ///
    /// Returns the event when it actually changed something, so a caller can
    /// forward it without comparing states itself.
    ///
    /// # Errors
    ///
    /// [`RegistryError::UnknownLane`] if no lane holds `id`.
    pub fn set_state(
        &mut self,
        id: LaneId,
        state: LaneState,
    ) -> Result<Option<LaneEvent>, RegistryError> {
        let entry = self
            .entries
            .get_mut(&id)
            .ok_or(RegistryError::UnknownLane(id))?;
        // An attention state is a gate: it stays up until this same channel
        // takes it down, because the pty cannot see it lift. Anything else
        // hands the lane back to what its output shows.
        entry.gate = state.needs_attention().then_some(state);
        Ok(entry.refresh())
    }

    /// Send keystrokes or pasted text to a lane's child.
    ///
    /// Goes to the named lane rather than the focused one: a lane can be typed
    /// at from a command palette or a script while a different one is on stage.
    ///
    /// # Errors
    ///
    /// [`RegistryError::UnknownLane`] if no lane holds `id`, or
    /// [`RegistryError::Pty`] if the child's input side has closed.
    pub fn write_input(&mut self, id: LaneId, bytes: &[u8]) -> Result<(), RegistryError> {
        let entry = self
            .entries
            .get_mut(&id)
            .ok_or(RegistryError::UnknownLane(id))?;
        // Typing is aimed at the program, and the program is at the bottom —
        // the same rule the terminal road applies at its input doors. Here
        // rather than in each caller, so a door added later cannot forget it.
        entry.pty.terminal_mut().grid_mut().view_to_bottom();
        entry.pty.write_input(bytes)?;
        Ok(())
    }

    /// Move a lane's view through its history — positive lines go back
    /// toward older output. The decisions live in the grid (`scroll_view`):
    /// clamped to the scrollback, refused on the alternate screen, anchored
    /// to content while the program keeps printing.
    ///
    /// # Errors
    ///
    /// [`RegistryError::UnknownLane`] if no lane holds `id`.
    pub fn scroll_view(&mut self, id: LaneId, lines: isize) -> Result<(), RegistryError> {
        let entry = self
            .entries
            .get_mut(&id)
            .ok_or(RegistryError::UnknownLane(id))?;
        entry.pty.terminal_mut().grid_mut().scroll_view(lines);
        Ok(())
    }

    /// Open or close one fold region in this lane's history; `Ok(false)`
    /// names an unknown region.
    pub fn set_fold_collapsed(
        &mut self,
        id: LaneId,
        fold: u32,
        collapsed: bool,
    ) -> Result<bool, RegistryError> {
        let entry = self
            .entries
            .get_mut(&id)
            .ok_or(RegistryError::UnknownLane(id))?;
        Ok(entry
            .pty
            .terminal_mut()
            .grid_mut()
            .set_fold_collapsed(fold, collapsed))
    }

    /// Whether this lane's program holds the alternate screen — the caller's
    /// cue to scroll by meaning (arrow keys) instead of by history.
    #[must_use]
    pub fn alt_screen(&self, id: LaneId) -> Option<bool> {
        self.entries
            .get(&id)
            .map(|entry| entry.pty.terminal().grid().alt_screen())
    }

    /// Resize a lane's screen and tell its child about it.
    ///
    /// Both halves matter, and [`PtyTransport::resize`] does both: resize only the
    /// grid and text wraps at a width the program is not using; resize only the
    /// pty and the program never hears `SIGWINCH`.
    ///
    /// # Errors
    ///
    /// [`RegistryError::UnknownLane`] if no lane holds `id`, or
    /// [`RegistryError::Pty`] if the pty rejects the new size.
    pub fn resize(&mut self, id: LaneId, rows: u16, cols: u16) -> Result<(), RegistryError> {
        let entry = self
            .entries
            .get_mut(&id)
            .ok_or(RegistryError::UnknownLane(id))?;
        entry.pty.resize(rows, cols)?;
        Ok(())
    }
}

impl Entry {
    /// Recompute the lane's rail-visible facts, and report an event only when
    /// one of them actually moved.
    ///
    /// The precedence is the whole policy: a dead child outranks everything
    /// (there is nothing left to be permitted), an open gate outranks output,
    /// and only with no gate does activity decide.
    fn refresh(&mut self) -> Option<LaneEvent> {
        let before = self.lane.state;
        self.lane.state = resolve_state(self.activity.current(), self.gate);
        let title_moved = self.sync_title();

        (self.lane.state != before || title_moved).then(|| LaneEvent::Updated(self.lane.clone()))
    }

    /// Adopt the title the program set for itself (`OSC 0`/`2`), reporting
    /// whether it moved.
    ///
    /// It wins over whatever the task was called, because it is what the agent
    /// says it is doing *now*.
    ///
    /// Reporting the change rather than letting the caller compare is what
    /// keeps a `String` clone out of the pump loop — this runs for every lane
    /// on every frame.
    fn sync_title(&mut self) -> bool {
        let Some(title) = self.pty.terminal().grid().title() else {
            return false;
        };
        if title == self.lane.title {
            return false;
        }
        self.lane.title = title.to_string();
        true
    }
}

#[cfg(test)]
mod resolve_state {
    use super::*;

    #[test]
    fn output_decides_when_no_gate_is_up() {
        assert_eq!(
            resolve_state(Activity::Streaming, None),
            LaneState::Streaming
        );
    }

    #[test]
    fn silence_with_no_gate_is_idle() {
        assert_eq!(resolve_state(Activity::Quiet, None), LaneState::Idle);
    }

    /// The rule the redraw bug came from: a lane waiting on a human keeps
    /// repainting, and that output must not be read as progress.
    #[test]
    fn an_open_gate_outranks_output() {
        assert_eq!(
            resolve_state(Activity::Streaming, Some(LaneState::AwaitingPermission)),
            LaneState::AwaitingPermission
        );
    }

    #[test]
    fn a_gate_also_outranks_silence() {
        assert_eq!(
            resolve_state(Activity::Quiet, Some(LaneState::Blocked)),
            LaneState::Blocked
        );
    }

    /// Nothing is left to permit once the child is gone, so death outranks even
    /// a gate somebody forgot to lower.
    #[test]
    fn a_dead_child_outranks_an_open_gate() {
        assert_eq!(
            resolve_state(Activity::Ended, Some(LaneState::AwaitingPermission)),
            LaneState::Exited
        );
    }

    #[test]
    fn a_dead_child_with_no_gate_is_exited() {
        assert_eq!(resolve_state(Activity::Ended, None), LaneState::Exited);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    use zerocode_core::{ALL_AGENTS, PaneKey};
    use zerocode_pty::{Pumped, Terminal};

    struct FakeTransport {
        terminal: Terminal,
        pending_output: Option<Vec<u8>>,
        running: bool,
        killed: Arc<AtomicBool>,
    }

    impl FakeTransport {
        fn new(output: &[u8], killed: Arc<AtomicBool>) -> Self {
            Self {
                terminal: Terminal::new(10, 40),
                pending_output: Some(output.to_vec()),
                running: true,
                killed,
            }
        }
    }

    impl PtyTransport for FakeTransport {
        fn pump(&mut self) -> Pumped {
            let output = self.pending_output.take().unwrap_or_default();
            self.terminal.feed(&output);
            Pumped {
                bytes: output.len(),
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
            self.terminal.feed(bytes);
            Ok(())
        }

        fn resize(&mut self, rows: u16, cols: u16) -> Result<(), PtyTransportError> {
            self.terminal
                .grid_mut()
                .resize(rows.max(1) as usize, cols.max(1) as usize);
            Ok(())
        }

        fn try_wait(&mut self) -> Result<Option<u32>, PtyTransportError> {
            Ok((!self.running).then_some(0))
        }

        fn kill(&mut self) -> Result<(), PtyTransportError> {
            self.running = false;
            self.killed.store(true, Ordering::Relaxed);
            Ok(())
        }
    }

    #[test]
    fn the_registry_drives_a_type_erased_handle_without_a_local_child() {
        let lane = Lane::new(ALL_AGENTS[0], PaneKey::random());
        let id = lane.id;
        let killed = Arc::new(AtomicBool::new(false));
        let mut registry = LaneRegistry::new();

        let handle = PtyHandle::new(FakeTransport::new(
            b"remote screen\x1b]52;c;ZnJvbSByZW1vdGU=\x07",
            Arc::clone(&killed),
        ));
        registry.insert(lane, handle).expect("insert transport");
        let events = registry.pump();
        assert_eq!(registry.screen_text(id).as_deref(), Some("remote screen"));
        assert!(events.iter().any(|event| {
            matches!(event, LaneEvent::ClipboardWrite { lane, text }
                if *lane == id && text == "from remote")
        }));

        registry
            .write_input(id, b" through input")
            .expect("write input");
        assert_eq!(
            registry.screen_text(id).as_deref(),
            Some("remote screen through input")
        );
        registry.resize(id, 12, 60).expect("resize");
        let LaneEvent::Screen { delta, .. } = registry.focus(id).expect("focus") else {
            panic!("focus must return a screen");
        };
        assert_eq!(delta.size, (12, 60));

        let removed = registry.remove(id).expect("remove transport");
        assert!(!removed.orphaned);
        assert!(killed.load(Ordering::Relaxed));
    }

    /// The payoff of splitting activity out: the quiet threshold is a function
    /// call, not eight real frames of a real process.
    #[test]
    fn a_burst_stays_streaming_until_the_quiet_run_is_long_enough() {
        let mut tracker = ActivityTracker::default();
        assert_eq!(tracker.observe(true, false), Activity::Streaming);

        for pump in 1..QUIET_PUMPS_BEFORE_IDLE {
            assert_eq!(
                tracker.observe(false, false),
                Activity::Streaming,
                "one gap is a pause between bursts, not the end of a turn (pump {pump})"
            );
        }
        assert_eq!(tracker.observe(false, false), Activity::Quiet);
    }

    #[test]
    fn any_output_restarts_the_quiet_run() {
        let mut tracker = ActivityTracker::default();
        tracker.observe(true, false);
        for _ in 0..QUIET_PUMPS_BEFORE_IDLE - 1 {
            tracker.observe(false, false);
        }
        assert_eq!(tracker.observe(true, false), Activity::Streaming);
        // The count restarted, so the threshold is a full run away again.
        for _ in 0..QUIET_PUMPS_BEFORE_IDLE - 1 {
            assert_eq!(tracker.observe(false, false), Activity::Streaming);
        }
        assert_eq!(tracker.observe(false, false), Activity::Quiet);
    }

    #[test]
    fn a_lane_that_never_spoke_is_quiet_rather_than_streaming() {
        let mut tracker = ActivityTracker::default();
        assert_eq!(tracker.current(), Activity::Quiet);
        assert_eq!(tracker.observe(false, false), Activity::Quiet);
    }

    #[test]
    fn a_closed_child_stays_closed() {
        let mut tracker = ActivityTracker::default();
        assert_eq!(tracker.observe(true, false), Activity::Streaming);
        assert_eq!(tracker.observe(false, true), Activity::Ended);
        // `pump` keeps reporting the end; the tracker must not drift off it.
        assert_eq!(tracker.observe(false, true), Activity::Ended);
        assert_eq!(tracker.current(), Activity::Ended);
    }
}
