//! Who reads a terminal's frames, and what each of them is still owed.
//!
//! The screen used to be PUSHED: the window's pump took a delta off every
//! watched grid every round and emitted it, whether or not the webview had
//! finished the last one. On a machine where one webview frame costs more than
//! a pump round those emits queued, the delay grew with every second of
//! output, and every stale frame was painted in turn
//! (`docs/design/terminal-display-paced-frames-20260917.md`, H1). Now the
//! screen PULLS: the pump only parses, and a delta is taken when a screen
//! comes for it. The grid already folds every change since the last take into
//! one delta (the shift contract on [`GridDelta`]), so taking later IS the
//! merge — nothing here composes frames.
//!
//! This module is the bookkeeping and nothing else: no lock, no window, no
//! event. It lives in [`crate::grid::Terminal`], under the same terminal lock
//! as the grid, so a take and the shares it fills are one step.
//!
//! - **Shares.** Every reader — a webview, by label — has a share: frames
//!   taken for somebody else's pull that this reader has not come for yet. A
//!   terminal with one reader (the usual case) never holds anything, because
//!   that reader's pull takes and returns in one breath.
//! - **Debts.** A reader that has just started reading, or whose share has
//!   grown past what one snapshot would cost, is owed a snapshot instead. The
//!   debt is paid on that reader's pull, in one order: first the grid's delta
//!   goes to every OTHER reader, and only then is the snapshot taken — so the
//!   snapshot's baseline is exactly where the others' next delta begins, and
//!   no reader loses the change in between. Never while the program is inside
//!   a frame it asked to be shown whole (`CSI ?2026h`): the grid hands no delta
//!   over then, so a snapshot would be both the half-drawn screen and a
//!   baseline the others never got the change to. The debt waits for the
//!   frame to close, and is told then.
//! - **Notices.** A reader nobody expects to come is told to, once
//!   ([`FrameReaders::look`]). A pull that comes back with frames keeps it
//!   expected — it will be back on its next display frame without being told
//!   — and an empty pull lets it go quiet, so a still terminal costs nothing.
//!   A reader told and silent for [`RENOTIFY_AFTER`] is told again: a notice
//!   is a hint, and a lost one must cost a moment, not a screen.
//! - **Chases.** The one exception to "a reader on its way is not told": the
//!   terminal has just answered a person's write ([`Look::Chase`], the first
//!   output that arrived after it — [`crate::answer`]). That answer — the echo
//!   of a key — is told to the reader that is coming back on its own display
//!   frame too, so its pull is now and the echo lands on the next frame rather
//!   than the one after. A reader already told is not told twice; its pull is
//!   on its way.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::grid::{GridDelta, TerminalGrid};

/// The most frames one reader's share may hold.
///
/// The same bound the webview's reveal buffer carried before this module
/// existed — four seconds of display-rate frames — for the same reason: a
/// reader that does not come (a hidden window, a stalled page) must cost a
/// bounded amount, and past this it costs one snapshot instead.
pub const READER_SHARE_FRAMES: usize = 240;

/// How much screen one reader's share may hold, in whole screens of cells.
///
/// The bound that matters in practice. A share is only ever worth keeping
/// while it is SMALLER than the snapshot that could replace it: once the
/// queued rows add up to a screen, a snapshot is cheaper to hold, cheaper to
/// send and cheaper to paint, and it is fresher. So the share folds into a
/// debt at one screen — a 60×220 pane holds at most one screen of cells per
/// reader however long its window stays away.
pub const READER_SHARE_SCREENS: usize = 1;

/// How long a reader that was told to come may stay silent before it is told
/// again.
///
/// A notice carries nothing and may be lost; the reader that missed one
/// would otherwise wait for the next change to be told. A quarter second is
/// under the pause a person reads as "stuck" and, for a window that is simply
/// hidden, four empty notices a second is nothing.
pub const RENOTIFY_AFTER: Duration = Duration::from_millis(250);

/// Why the pump is looking this round — and so who has to hear of a change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Look {
    /// The display beat. A reader on its way comes for the change on its own
    /// next display frame, and is not told.
    Beat,
    /// The look after the terminal's answer to a person's write was parsed
    /// ([`crate::lane::Pumped::answered`]). A change is told to a reader on
    /// its way as well — one that is flowing would otherwise take the echo on
    /// its next display frame and paint it on the frame after, a whole frame
    /// behind the key that asked for it (M2, `ui/tests/terminal-pacing.mjs`).
    Chase,
}

/// What a reader is expected to do next.
#[derive(Debug, Clone, Copy)]
enum Awaited {
    /// Nothing: the next change tells it.
    Quiet,
    /// Told to come, at this instant, and not come yet.
    Told(Instant),
    /// Back from a pull that brought frames, at this instant: it comes again
    /// on its own next display frame without being told.
    Flowing(Instant),
}

/// The reader whose pull a take is serving.
#[derive(Debug, Clone, Copy)]
struct Pulling {
    at: usize,
    /// The frame is the puller's answer (`true`), or it is paying a debt and
    /// the snapshot that follows is its answer (`false`).
    keeps: bool,
}

/// Every reader of one terminal. See the module documentation.
#[derive(Debug, Default)]
pub struct FrameReaders {
    readers: Vec<Reader>,
}

#[derive(Debug)]
struct Reader {
    label: Arc<str>,
    /// Frames taken for another reader's pull, oldest first.
    share: Vec<GridDelta>,
    /// Cells held in `share`, so the bound is a sum kept, not a sum walked.
    share_cells: usize,
    owes_snapshot: bool,
    awaited: Awaited,
}

impl Reader {
    fn owing(label: &str) -> Self {
        Self {
            label: Arc::from(label),
            share: Vec::new(),
            share_cells: 0,
            owes_snapshot: true,
            awaited: Awaited::Quiet,
        }
    }

    /// Replace whatever this reader holds with a debt. The share's memory goes
    /// with it — a debt is the promise that nothing needs to be held.
    fn owe_snapshot(&mut self) {
        self.share = Vec::new();
        self.share_cells = 0;
        self.owes_snapshot = true;
    }

    /// Whether this reader should be told to come: nobody expects it, it was
    /// expected and has been silent too long, or it is flowing and the look
    /// follows the answer to a person's write ([`Look::Chase`]).
    fn due(&self, now: Instant, look: Look) -> bool {
        let silent = |since: Instant| now.saturating_duration_since(since) >= RENOTIFY_AFTER;
        match self.awaited {
            Awaited::Quiet => true,
            Awaited::Told(since) => silent(since),
            Awaited::Flowing(since) => look == Look::Chase || silent(since),
        }
    }

    /// Whether a pull would bring this reader anything — a debt only once the
    /// program has finished the frame it is drawing (`holding`).
    fn has_something(&self, holding: bool) -> bool {
        (self.owes_snapshot && !holding) || !self.share.is_empty()
    }

    /// One more frame for this reader's share.
    fn receive(&mut self, delta: GridDelta, screen_cells: usize) {
        if delta.full {
            // A full frame replaces everything a consumer holds, so it
            // replaces every earlier frame here — and a debt with them: it is
            // the whole screen, with every later delta measured from it.
            self.share.clear();
            self.share_cells = 0;
            self.owes_snapshot = false;
        } else if self.owes_snapshot {
            // The snapshot this reader is owed already contains it.
            return;
        }
        let cells = delta.rows.iter().map(|row| row.cells.len()).sum::<usize>();
        if self.share.len() >= READER_SHARE_FRAMES
            || self.share_cells + cells > screen_cells.saturating_mul(READER_SHARE_SCREENS)
        {
            self.owe_snapshot();
            return;
        }
        self.share_cells += cells;
        self.share.push(delta);
    }
}

impl FrameReaders {
    /// Whether any screen reads this terminal.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.readers.is_empty()
    }

    /// Bring the readers in line with the labels that declared this terminal.
    ///
    /// A label not yet here starts owing a snapshot; a reader whose label is
    /// gone leaves with its share. Asked every pump round, because the
    /// declaration is the truth and this is only its delivery state: a shell
    /// replaced under the same id starts with no readers at all, and a window
    /// that closed takes its declaration with it. Allocates only for a label
    /// that is new.
    pub fn reconcile<L: AsRef<str>>(&mut self, labels: &[L]) {
        self.readers
            .retain(|reader| labels.iter().any(|label| label.as_ref() == &*reader.label));
        for label in labels {
            let label = label.as_ref();
            if !self.readers.iter().any(|reader| &*reader.label == label) {
                self.readers.push(Reader::owing(label));
            }
        }
    }

    /// A screen has just started reading this terminal: whatever its model
    /// holds is from before, so it is owed a snapshot — even if a reader by
    /// that label was still here.
    pub fn declare(&mut self, label: &str) {
        match self
            .readers
            .iter_mut()
            .find(|reader| &*reader.label == label)
        {
            Some(reader) => {
                reader.owe_snapshot();
                reader.awaited = Awaited::Quiet;
            }
            None => self.readers.push(Reader::owing(label)),
        }
    }

    /// A screen has stopped reading this terminal.
    pub fn leave(&mut self, label: &str) {
        self.readers.retain(|reader| &*reader.label != label);
    }

    /// One pump round's look: who has to be told to come.
    ///
    /// The delta is taken here only for a reader nobody is expecting — the
    /// quiet one that must hear about the change, or the silent one that must
    /// hear again. A reader already on its way takes its own frame when it
    /// arrives, and the grid keeps folding changes until then, which is the
    /// whole of display pacing. The one exception is a chase look
    /// ([`Look::Chase`]), which tells a flowing reader too. Each label that
    /// must be told is pushed onto `told`, once.
    pub fn look(
        &mut self,
        grid: &mut TerminalGrid,
        now: Instant,
        look: Look,
        told: &mut Vec<Arc<str>>,
    ) {
        if self
            .readers
            .iter()
            .any(|reader| reader.due(now, look) && !reader.owes_snapshot)
        {
            let _ = self.take(grid, None);
        }
        let holding = grid.holding_frame();
        for reader in &mut self.readers {
            if reader.due(now, look) && reader.has_something(holding) {
                reader.awaited = Awaited::Told(now);
                told.push(Arc::clone(&reader.label));
            }
        }
    }

    /// One reader's pull: everything it is owed, oldest first.
    ///
    /// A label that is not a reader yet becomes one here and is paid its
    /// snapshot at once — the pull IS the first contact. An empty answer lets
    /// the reader go quiet; frames keep it expected. A debt that meets an
    /// unfinished atomic frame is answered with nothing and stays owed: quiet,
    /// the reader is told again by the look that sees the frame close.
    pub fn pull(&mut self, label: &str, grid: &mut TerminalGrid, now: Instant) -> Vec<GridDelta> {
        let at = match self
            .readers
            .iter()
            .position(|reader| &*reader.label == label)
        {
            Some(at) => at,
            None => {
                self.readers.push(Reader::owing(label));
                self.readers.len() - 1
            }
        };
        if self.readers[at].owes_snapshot && grid.holding_frame() {
            self.readers[at].awaited = Awaited::Quiet;
            return Vec::new();
        }
        let frames = if self.readers[at].owes_snapshot {
            // The others first: after this, the grid's baseline is the point
            // their shares end at, and the snapshot below starts there too.
            let _ = self.take(grid, Some(Pulling { at, keeps: false }));
            vec![grid.take_snapshot()]
        } else {
            // The fresh frame goes straight into the answer, never through
            // this reader's own share: the share's bound exists for frames
            // that WAIT, and folding the one being handed over into a debt
            // would answer the pull with nothing.
            let fresh = self.take(grid, Some(Pulling { at, keeps: true }));
            let mut frames = std::mem::take(&mut self.readers[at].share);
            frames.extend(fresh);
            frames
        };
        let reader = &mut self.readers[at];
        reader.share_cells = 0;
        reader.owes_snapshot = false;
        reader.awaited = if frames.is_empty() {
            Awaited::Quiet
        } else {
            Awaited::Flowing(now)
        };
        frames
    }

    /// Take the grid's delta: a copy into the share of every reader except
    /// the one pulling, and the frame itself for the puller when it keeps it.
    ///
    /// When nobody keeps the frame, the last share takes it and the others
    /// copy — with the one reader a terminal almost always has, no copy at
    /// all. When nobody would receive it (the only reader paying its debt),
    /// nothing is taken: the snapshot that follows settles the same baseline,
    /// and materialising rows only to drop them is the cost this skips.
    fn take(&mut self, grid: &mut TerminalGrid, pulling: Option<Pulling>) -> Option<GridDelta> {
        let others = self.readers.len() - usize::from(pulling.is_some());
        let keeps = pulling.is_some_and(|pulling| pulling.keeps);
        if others == 0 && !keeps {
            return None;
        }
        let mut delta = Some(grid.take_delta()?);
        let screen_cells = grid.rows().saturating_mul(grid.cols());
        let mut handed = 0;
        for (index, reader) in self.readers.iter_mut().enumerate() {
            if pulling.is_some_and(|pulling| pulling.at == index) {
                continue;
            }
            handed += 1;
            let frame = if handed == others && !keeps {
                delta.take()
            } else {
                delta.clone()
            };
            if let Some(frame) = frame {
                reader.receive(frame, screen_cells);
            }
        }
        if keeps { delta } else { None }
    }

    /// What the named reader holds right now — for the tests that pin the
    /// bounds from outside this module.
    #[must_use]
    pub fn share_of(&self, label: &str) -> Option<ShareOf> {
        self.readers
            .iter()
            .find(|reader| &*reader.label == label)
            .map(|reader| ShareOf {
                frames: reader.share.len(),
                cells: reader.share_cells,
                owes_snapshot: reader.owes_snapshot,
            })
    }
}

/// One reader's holding, as [`FrameReaders::share_of`] reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShareOf {
    pub frames: usize,
    pub cells: usize,
    pub owes_snapshot: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::Terminal;

    const MAIN: &str = "main";
    const POPOUT: &str = "board-popout";

    fn later(base: Instant, by: Duration) -> Instant {
        base.checked_add(by).expect("a test instant stays in range")
    }

    fn fed(bytes: &[u8]) -> Terminal {
        let mut terminal = Terminal::new(4, 10);
        terminal.feed(bytes);
        terminal
    }

    fn held(readers: &FrameReaders, label: &str) -> Option<(usize, bool)> {
        readers
            .share_of(label)
            .map(|share| (share.frames, share.owes_snapshot))
    }

    fn look(readers: &mut FrameReaders, terminal: &mut Terminal, now: Instant) -> Vec<String> {
        look_for(readers, terminal, now, Look::Beat)
    }

    fn look_for(
        readers: &mut FrameReaders,
        terminal: &mut Terminal,
        now: Instant,
        why: Look,
    ) -> Vec<String> {
        let mut told = Vec::new();
        readers.look(terminal.grid_mut(), now, why, &mut told);
        told.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn a_new_reader_is_owed_a_snapshot_and_told_once() {
        let mut terminal = fed(b"hello");
        let mut readers = FrameReaders::default();
        let now = Instant::now();
        readers.reconcile(&[MAIN]);

        assert_eq!(look(&mut readers, &mut terminal, now), [MAIN]);
        assert!(
            look(&mut readers, &mut terminal, now).is_empty(),
            "a reader already told was told again before it could come"
        );

        let frames = readers.pull(MAIN, terminal.grid_mut(), now);
        assert_eq!(frames.len(), 1);
        assert!(
            frames[0].full,
            "the first frame a new reader gets is not whole"
        );
    }

    #[test]
    fn a_reader_on_its_way_is_not_told_and_the_pump_takes_nothing_for_it() {
        let mut terminal = fed(b"a");
        let mut readers = FrameReaders::default();
        let now = Instant::now();
        readers.reconcile(&[MAIN]);
        let _ = readers.pull(MAIN, terminal.grid_mut(), now);

        // The pull brought frames, so the reader is coming back on its own.
        terminal.feed(b"b");
        assert!(look(&mut readers, &mut terminal, now).is_empty());
        assert_eq!(held(&readers, MAIN), Some((0, false)));

        // And what it comes back for is everything since, as one frame.
        terminal.feed(b"c");
        let frames = readers.pull(MAIN, terminal.grid_mut(), now);
        assert_eq!(
            frames.len(),
            1,
            "the grid did not fold two writes into one take"
        );
    }

    #[test]
    fn a_reader_on_its_way_is_told_when_the_pump_chases_an_answer() {
        let mut terminal = fed(b"$ ");
        let mut readers = FrameReaders::default();
        let now = Instant::now();
        readers.reconcile(&[MAIN]);
        let _ = readers.pull(MAIN, terminal.grid_mut(), now);

        // Flowing: the screen comes back on its own next display frame, so the
        // display beat tells it nothing.
        terminal.feed(b"building\r\n");
        assert!(look(&mut readers, &mut terminal, now).is_empty());

        // A person typed, and the chase look finds what answers it: the reader
        // on its way is told now, with the change taken for it.
        terminal.feed(b"x");
        assert_eq!(
            look_for(&mut readers, &mut terminal, now, Look::Chase),
            [MAIN],
            "an echo over a flowing screen waits for that screen's next display frame"
        );
        assert_eq!(held(&readers, MAIN), Some((1, false)));

        // Told, and not come yet: a second chase look does not tell it twice.
        terminal.feed(b"y");
        assert!(look_for(&mut readers, &mut terminal, now, Look::Chase).is_empty());

        // And its pull brings everything since, the echo included.
        let frames = readers.pull(MAIN, terminal.grid_mut(), now);
        assert_eq!(frames.len(), 2);
    }

    #[test]
    fn a_chase_look_tells_a_quiet_reader_nothing_it_had_not_changed() {
        let mut terminal = fed(b"$ ");
        let mut readers = FrameReaders::default();
        let now = Instant::now();
        readers.reconcile(&[MAIN]);
        let _ = readers.pull(MAIN, terminal.grid_mut(), now);

        // On its way, and nothing has arrived yet: the chase keeps looking and
        // tells nobody — a still terminal costs nothing, chased or not.
        assert!(look_for(&mut readers, &mut terminal, now, Look::Chase).is_empty());
        assert!(readers.pull(MAIN, terminal.grid_mut(), now).is_empty());
        assert!(look_for(&mut readers, &mut terminal, now, Look::Chase).is_empty());
    }

    #[test]
    fn an_empty_pull_goes_quiet_and_the_next_change_tells_it() {
        let mut terminal = fed(b"a");
        let mut readers = FrameReaders::default();
        let now = Instant::now();
        readers.reconcile(&[MAIN]);
        let _ = readers.pull(MAIN, terminal.grid_mut(), now);
        assert!(readers.pull(MAIN, terminal.grid_mut(), now).is_empty());

        // Quiet: a still terminal costs nothing.
        assert!(look(&mut readers, &mut terminal, now).is_empty());

        terminal.feed(b"b");
        assert_eq!(look(&mut readers, &mut terminal, now), [MAIN]);
        // Taken at the notice, so the pull that answers it finds it waiting.
        assert_eq!(held(&readers, MAIN), Some((1, false)));
        assert_eq!(readers.pull(MAIN, terminal.grid_mut(), now).len(), 1);
    }

    #[test]
    fn a_reader_told_and_silent_is_told_again() {
        let mut terminal = fed(b"a");
        let mut readers = FrameReaders::default();
        let now = Instant::now();
        readers.reconcile(&[MAIN]);
        assert_eq!(look(&mut readers, &mut terminal, now), [MAIN]);

        let soon = later(now, RENOTIFY_AFTER / 2);
        assert!(look(&mut readers, &mut terminal, soon).is_empty());

        let lost = later(now, RENOTIFY_AFTER);
        assert_eq!(
            look(&mut readers, &mut terminal, lost),
            [MAIN],
            "a lost notice leaves the reader waiting for the next change"
        );
    }

    #[test]
    fn two_readers_share_one_take_and_each_pull_is_whole() {
        let mut terminal = fed(b"a");
        let mut readers = FrameReaders::default();
        let now = Instant::now();
        readers.reconcile(&[MAIN, POPOUT]);
        let _ = readers.pull(MAIN, terminal.grid_mut(), now);
        let _ = readers.pull(POPOUT, terminal.grid_mut(), now);

        terminal.feed(b"b");
        let main = readers.pull(MAIN, terminal.grid_mut(), now);
        assert_eq!(main.len(), 1);
        // The take happened once, and the other reader holds its copy.
        assert_eq!(held(&readers, POPOUT), Some((1, false)));
        terminal.feed(b"c");
        let popout = readers.pull(POPOUT, terminal.grid_mut(), now);
        assert_eq!(
            popout.len(),
            2,
            "the popout lost the frame the main window took"
        );
        assert_eq!(held(&readers, MAIN), Some((1, false)));
    }

    #[test]
    fn a_share_that_outgrows_a_snapshot_becomes_a_debt() {
        let mut terminal = Terminal::new(4, 10);
        let mut readers = FrameReaders::default();
        let now = Instant::now();
        readers.reconcile(&[MAIN, POPOUT]);
        let _ = readers.pull(MAIN, terminal.grid_mut(), now);
        let _ = readers.pull(POPOUT, terminal.grid_mut(), now);

        // The main window keeps pulling whole-screen redraws; the popout never
        // comes. One screen of cells fits, a second does not.
        for round in 0..3u8 {
            terminal.feed(format!("\x1b[H{round}{round}{round}\r\n\x1b[2K{round}\r\n\x1b[2K{round}\r\n\x1b[2K{round}").as_bytes());
            let _ = readers.pull(MAIN, terminal.grid_mut(), now);
        }
        assert_eq!(
            held(&readers, POPOUT),
            Some((0, true)),
            "a reader that never comes holds more than one screen"
        );

        let frames = readers.pull(POPOUT, terminal.grid_mut(), now);
        assert_eq!(frames.len(), 1);
        assert!(
            frames[0].full,
            "a debt was paid with something other than the screen"
        );
    }

    #[test]
    fn a_pull_brings_its_own_frame_even_when_its_share_is_full() {
        let mut terminal = Terminal::new(4, 10);
        let mut readers = FrameReaders::default();
        let now = Instant::now();
        readers.reconcile(&[MAIN, POPOUT]);
        let _ = readers.pull(MAIN, terminal.grid_mut(), now);
        let _ = readers.pull(POPOUT, terminal.grid_mut(), now);
        let redraw = |terminal: &mut Terminal, mark: char| {
            let row = mark.to_string().repeat(4);
            terminal.feed(format!("\x1b[H{row}\r\n{row}\r\n{row}\r\n{row}").as_bytes());
        };

        // The popout's pull leaves a whole screen in the main window's share.
        redraw(&mut terminal, 'a');
        let _ = readers.pull(POPOUT, terminal.grid_mut(), now);
        assert_eq!(held(&readers, MAIN), Some((1, false)));

        // A second screen arrives with the main window's own pull: the share
        // bound is for frames that wait, not for the one being handed over.
        redraw(&mut terminal, 'b');
        let frames = readers.pull(MAIN, terminal.grid_mut(), now);
        assert_eq!(frames.len(), 2, "the pull answered with less than it took");
        assert_eq!(held(&readers, MAIN), Some((0, false)));
    }

    #[test]
    fn a_full_frame_replaces_the_share_it_lands_in() {
        let mut terminal = fed(b"a");
        let mut readers = FrameReaders::default();
        let now = Instant::now();
        readers.reconcile(&[MAIN, POPOUT]);
        let _ = readers.pull(MAIN, terminal.grid_mut(), now);
        let _ = readers.pull(POPOUT, terminal.grid_mut(), now);

        terminal.feed(b"b");
        let _ = readers.pull(MAIN, terminal.grid_mut(), now);
        assert_eq!(held(&readers, POPOUT), Some((1, false)));
        // The alternate screen swaps in: a full frame.
        terminal.feed(b"\x1b[?1049hvim");
        let _ = readers.pull(MAIN, terminal.grid_mut(), now);
        assert_eq!(held(&readers, POPOUT), Some((1, false)));
        let frames = readers.pull(POPOUT, terminal.grid_mut(), now);
        assert!(frames.len() == 1 && frames[0].full);
    }

    #[test]
    fn reconciling_drops_the_gone_and_owes_the_new() {
        let mut terminal = fed(b"a");
        let mut readers = FrameReaders::default();
        let now = Instant::now();
        readers.reconcile(&[MAIN]);
        let _ = readers.pull(MAIN, terminal.grid_mut(), now);

        readers.reconcile(&[POPOUT]);
        assert_eq!(held(&readers, MAIN), None);
        assert_eq!(held(&readers, POPOUT), Some((0, true)));

        readers.reconcile::<&str>(&[]);
        assert!(readers.is_empty());
    }

    #[test]
    fn a_debt_is_not_paid_with_a_frame_the_program_has_not_finished() {
        let mut terminal = fed(b"ready");
        let mut readers = FrameReaders::default();
        let now = Instant::now();
        readers.reconcile(&[MAIN]);
        let _ = readers.pull(MAIN, terminal.grid_mut(), now);

        // The program opens an atomic frame (`CSI ?2026h`) and is halfway
        // through it when a second screen starts reading.
        terminal.feed(b"\x1b[?2026h\x1b[H\x1b[2Khalf");
        readers.declare(POPOUT);
        assert!(
            look(&mut readers, &mut terminal, now).is_empty(),
            "a reader was told to come for a frame the program has not finished"
        );
        assert!(
            readers.pull(POPOUT, terminal.grid_mut(), now).is_empty(),
            "a debt was paid with a half-drawn screen"
        );
        assert_eq!(held(&readers, POPOUT), Some((0, true)));

        // The frame closes: the debt is told, and paid with all of it — and
        // the screen that was already reading loses none of it.
        terminal.feed(b" done\x1b[?2026l");
        assert_eq!(look(&mut readers, &mut terminal, now), [POPOUT]);
        let frames = readers.pull(POPOUT, terminal.grid_mut(), now);
        let row = |delta: &GridDelta| {
            delta
                .rows
                .iter()
                .find(|row| row.index == 0)
                .map(|row| row.cells.iter().map(|cell| cell.ch).collect::<String>())
                .unwrap_or_default()
        };
        assert!(
            frames.len() == 1 && frames[0].full && row(&frames[0]).starts_with("half done"),
            "the debt was not paid with the finished frame"
        );
        let main = readers.pull(MAIN, terminal.grid_mut(), now);
        assert!(
            main.iter().any(|delta| row(delta).starts_with("half done")),
            "the screen already reading lost the frame the debt was paid around"
        );
    }

    #[test]
    fn declaring_again_owes_again() {
        let mut terminal = fed(b"a");
        let mut readers = FrameReaders::default();
        let now = Instant::now();
        readers.declare(MAIN);
        let _ = readers.pull(MAIN, terminal.grid_mut(), now);
        assert_eq!(held(&readers, MAIN), Some((0, false)));

        // Hidden and shown again: whatever the screen held is from before.
        readers.declare(MAIN);
        assert_eq!(held(&readers, MAIN), Some((0, true)));
        readers.leave(MAIN);
        assert!(readers.is_empty());
    }
}
