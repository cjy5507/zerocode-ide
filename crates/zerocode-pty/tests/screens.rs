//! The terminal read path called by a host with no window toolkit at all.
//!
//! Everything here goes through the crate's public API only, from a binary
//! that cannot link one (this crate depends on no toolkit): the host is a map
//! of shells, each under its own lock — the shape of the window's pool and
//! nothing more — and the notices land in a list. The pump's part of the read
//! path ([`pump_round`]) is the window pump's own lines: the declaration
//! inverted once, each shell's readers brought in line and looked at under
//! that shell's lock, then one notice a screen with no lock held.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use zerocode_pty::grid::{GridDelta, Terminal};
use zerocode_pty::{
    DirtyNotices, Look, PulledScreen, PulledScreens, TermId, TerminalPool, WatchedTerms, tell_dirty,
};

mod consumer;
use consumer::{Consumer, assert_same, truth};

/// How long a scenario may take before it is called a deadlock. Generous for
/// a loaded machine, and spent only on failure.
const DEADLOCK: Duration = Duration::from_secs(60);

const MAIN: &str = "main";
const POPOUT: &str = "board-popout";
const NATIVE: &str = "native";

/// A host's shells: the map lock held only to clone a shell's handle, the
/// work done under that shell's own lock.
#[derive(Default)]
struct Pool {
    shells: Mutex<HashMap<TermId, Arc<Mutex<Terminal>>>>,
}

impl Pool {
    fn holding(terms: &[TermId], rows: usize, cols: usize) -> Self {
        let pool = Self::default();
        for term in terms {
            pool.open(*term, rows, cols);
        }
        pool
    }

    fn open(&self, term: TermId, rows: usize, cols: usize) {
        self.shells
            .lock()
            .expect("the pool's map")
            .insert(term, Arc::new(Mutex::new(Terminal::new(rows, cols))));
    }

    fn close(&self, term: TermId) {
        self.shells.lock().expect("the pool's map").remove(&term);
    }

    fn terms(&self) -> Vec<TermId> {
        let mut terms: Vec<TermId> = self
            .shells
            .lock()
            .expect("the pool's map")
            .keys()
            .copied()
            .collect();
        terms.sort_unstable();
        terms
    }

    fn feed(&self, term: TermId, bytes: &[u8]) {
        self.with_terminal(term, |terminal| terminal.feed(bytes))
            .expect("a shell to feed");
    }

    fn screen(&self, term: TermId) -> Vec<String> {
        self.with_terminal(term, |terminal| truth(terminal))
            .expect("a shell to read")
    }

    /// Whether the shell holds `label` as one of its readers right now.
    fn reads(&self, term: TermId, label: &str) -> bool {
        self.with_terminal(term, |terminal| {
            terminal.readers_mut().0.share_of(label).is_some()
        })
        .expect("a shell to ask")
    }
}

impl TerminalPool for Pool {
    fn with_terminal<R>(&self, term: TermId, work: impl FnOnce(&mut Terminal) -> R) -> Option<R> {
        let held = self
            .shells
            .lock()
            .expect("the pool's map")
            .get(&term)
            .cloned()?;
        let mut terminal = held.lock().expect("a shell's lock");
        Some(work(&mut terminal))
    }
}

/// Notices, in the order they went out.
#[derive(Default)]
struct Told(Mutex<Vec<String>>);

impl Told {
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock().expect("the notices"))
    }
}

impl DirtyNotices for Told {
    fn dirty(&self, label: &str) {
        self.0.lock().expect("the notices").push(label.to_string());
    }
}

/// One pump round over the read path, with the declaration read now.
fn pump_round(pool: &Pool, watched: &WatchedTerms, notices: &impl DirtyNotices) {
    pump_round_reading(pool, &watched.readers_by_term(), notices);
}

/// One pump round with the readers it read at its start — which may be older
/// than the declaration by the time a shell is looked at, as in the window's
/// pump, where a declaration that lands mid-round is read on the next one.
fn pump_round_reading(
    pool: &Pool,
    readers: &HashMap<TermId, Vec<String>>,
    notices: &impl DirtyNotices,
) {
    let now = Instant::now();
    let mut told = Vec::new();
    for term in pool.terms() {
        let labels = readers.get(&term).map(Vec::as_slice).unwrap_or_default();
        pool.with_terminal(term, |terminal| {
            let (frame_readers, grid) = terminal.readers_mut();
            frame_readers.reconcile(labels);
            frame_readers.look(grid, now, Look::Beat, &mut told);
        });
    }
    tell_dirty(told, notices);
}

/// A screen as a host keeps it: one model per shell, `None` until its floor.
#[derive(Default)]
struct Screen {
    models: HashMap<TermId, Consumer>,
}

impl Screen {
    fn apply(&mut self, answer: &PulledScreens) {
        for PulledScreen { term, frames } in &answer.screens {
            for frame in frames {
                match self.models.get_mut(term) {
                    Some(model) => model.apply(frame),
                    None => {
                        self.models.insert(*term, Consumer::from_full(frame));
                    }
                }
            }
        }
    }

    fn rows(&self, term: TermId) -> &[String] {
        &self
            .models
            .get(&term)
            .unwrap_or_else(|| panic!("no floor was ever pulled for shell {term}"))
            .rows
    }
}

fn terms_of(answer: &PulledScreens) -> Vec<TermId> {
    answer.screens.iter().map(|screen| screen.term).collect()
}

/// Run `work` on its own thread and wait for it at most [`DEADLOCK`].
fn within<T: Send + 'static>(what: &str, work: impl FnOnce() -> T + Send + 'static) -> T {
    let (done, finished) = mpsc::channel();
    thread::spawn(move || {
        let _ = done.send(work());
    });
    finished.recv_timeout(DEADLOCK).unwrap_or_else(|error| {
        panic!(
            "{what} did not finish ({error:?}): a lock is held across a call that takes it again"
        )
    })
}

/// Wait until every screen thread has pulled again since the last wait, so
/// the printing interleaves with the pulls however the machine schedules them.
fn wait_until_each_moved(progress: &[Arc<AtomicUsize>], seen: &mut [usize]) {
    for (counter, seen) in progress.iter().zip(seen.iter_mut()) {
        while counter.load(Ordering::Acquire) <= *seen {
            thread::yield_now();
        }
        *seen = counter.load(Ordering::Acquire);
    }
}

/// How many pieces of output go in between two waits for the screens.
const PIECES_A_WAIT: usize = 64;

/// Output a person would recognise from a build: numbered lines in colour,
/// a header redrawn now and then, and once in a while a cleared screen.
fn build_log(lines: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for line in 0..lines {
        if line.is_multiple_of(97) {
            out.extend_from_slice(
                format!("\x1b[s\x1b[H\x1b[7m header {line} \x1b[0m\x1b[u").as_bytes(),
            );
        }
        if line % 601 == 600 {
            out.extend_from_slice(b"\x1b[2J\x1b[H");
        }
        out.extend_from_slice(
            format!(
                "\x1b[3{}mstep {line}\x1b[0m {} 빌드 {}\r\n",
                line % 7,
                "abcdefghij".repeat(line % 6),
                line * 7 % 1000
            )
            .as_bytes(),
        );
    }
    out
}

/// Pieces of uneven size, cut anywhere — mid-escape and mid-character too, as
/// a pty read cuts them.
fn pieces(bytes: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut at = 0;
    let mut size = 1;
    while at < bytes.len() {
        let end = (at + size).min(bytes.len());
        out.push(&bytes[at..end]);
        at = end;
        size = size * 7 % 311 + 1;
    }
    out
}

/// A host with no window toolkit declares, pulls, and reads Rust types: the
/// first pull is each shell's whole screen, ascending by number, a declared
/// shell the pool does not hold is named, and a shell that owes nothing is
/// absent rather than empty. The wire a webview reads is the same answer
/// serialised, field for field.
#[test]
fn a_host_with_no_window_toolkit_reads_the_answer_as_rust_types() {
    let pool = Pool::holding(&[3, 7, 12, 20, 31, 44], 5, 20);
    pool.feed(3, b"hello");
    pool.feed(7, b"world");
    let watched = WatchedTerms::default();

    // Declared out of order, with a shell nobody opened. Six shells, so an
    // answer that kept the set's own order would be in order by chance once
    // in 720.
    watched.declare(&pool, NATIVE, [44, 7, 9, 31, 3, 20, 12]);
    let answer: PulledScreens = watched.pull(&pool, NATIVE, false);
    assert_eq!(
        terms_of(&answer),
        [3, 7, 12, 20, 31, 44],
        "the shells are not in order"
    );
    assert_eq!(
        answer.missing,
        [9],
        "a declared shell that is not there is not named"
    );
    let floors: Vec<&GridDelta> = answer
        .screens
        .iter()
        .map(|screen| {
            assert_eq!(
                screen.frames.len(),
                1,
                "a floor came as more than one frame"
            );
            &screen.frames[0]
        })
        .collect();
    assert!(
        floors.iter().all(|frame| frame.full),
        "the first frame a screen gets is not whole"
    );
    let mut screen = Screen::default();
    screen.apply(&answer);
    assert_eq!(screen.rows(3)[0], "hello");
    assert_eq!(screen.rows(7)[0], "world");

    // The wire is this very answer, serialised: the names and their order
    // are what the webview parses.
    let wire = serde_json::to_string(&answer).expect("an answer serialises");
    assert!(
        wire.starts_with(r#"{"screens":[{"term":3,"frames":[{"#)
            && wire.ends_with(r#"}]}],"missing":[9]}"#),
        "the answer's wire shape moved: {wire}"
    );

    // Then only what changed, from the shell that changed.
    pool.feed(3, b" again");
    let answer = watched.pull(&pool, NATIVE, false);
    assert_eq!(
        terms_of(&answer),
        [3],
        "a shell that owed nothing came back anyway"
    );
    assert!(
        answer.screens[0].frames.iter().all(|frame| !frame.full),
        "a change after the floor came back as a whole screen"
    );
    screen.apply(&answer);
    assert_same("after the floor", screen.rows(3), &pool.screen(3));
    assert_eq!(answer.missing, [9]);

    // Nothing new: an answer with no screens at all.
    let answer = watched.pull(&pool, NATIVE, false);
    assert!(
        answer.screens.is_empty(),
        "a still terminal answered frames"
    );
    assert_eq!(
        serde_json::to_string(&answer).expect("an answer serialises"),
        r#"{"screens":[],"missing":[9]}"#
    );
}

/// Two screens, two declarations: each pulls its own shells, and a change is
/// told to the screens that read that shell and to no other.
#[test]
fn two_screens_keep_two_different_declarations() {
    let pool = Pool::holding(&[1, 2, 3], 4, 16);
    let watched = WatchedTerms::default();
    let told = Told::default();
    watched.declare(&pool, MAIN, [1, 2]);
    watched.declare(&pool, POPOUT, [2, 3]);

    assert_eq!(terms_of(&watched.pull(&pool, MAIN, false)), [1, 2]);
    assert_eq!(terms_of(&watched.pull(&pool, POPOUT, false)), [2, 3]);
    assert_eq!(watched.terms_of(MAIN), HashSet::from([1, 2]));
    assert_eq!(watched.terms_of(POPOUT), HashSet::from([2, 3]));
    assert_eq!(
        watched.readers_by_term().get(&2).map(Vec::as_slice),
        Some([POPOUT.to_string(), MAIN.to_string()].as_slice()),
        "the shell both screens read is not read by both, in order"
    );
    // Quiet again: an empty pull lets a screen go quiet.
    assert!(watched.pull(&pool, MAIN, false).screens.is_empty());
    assert!(watched.pull(&pool, POPOUT, false).screens.is_empty());

    pool.feed(1, b"one");
    pump_round(&pool, &watched, &told);
    assert_eq!(
        told.take(),
        [MAIN],
        "a change only main reads was told elsewhere"
    );
    assert_eq!(terms_of(&watched.pull(&pool, MAIN, false)), [1]);

    pool.feed(3, b"three");
    pump_round(&pool, &watched, &told);
    assert_eq!(
        told.take(),
        [POPOUT],
        "a change only the pop-out reads was told elsewhere"
    );
    assert_eq!(terms_of(&watched.pull(&pool, POPOUT, false)), [3]);

    pool.feed(2, b"two");
    pump_round(&pool, &watched, &told);
    assert_eq!(
        told.take(),
        [POPOUT, MAIN],
        "a change both read was not told to both, in order"
    );
    assert_eq!(terms_of(&watched.pull(&pool, MAIN, false)), [2]);
    assert_eq!(terms_of(&watched.pull(&pool, POPOUT, false)), [2]);
}

/// Two screens reading one shell at the same time — each on its own thread,
/// with the shell printing and the pump looking on threads of their own — each
/// rebuild the whole screen: every frame one of them took is still owed to the
/// other.
#[test]
fn two_screens_reading_one_shell_at_once_each_rebuild_the_whole_screen() {
    within("two screens pulling one shell", || {
        let pool = Arc::new(Pool::holding(&[4], 24, 80));
        let watched = Arc::new(WatchedTerms::default());
        watched.declare(&*pool, MAIN, [4]);
        watched.declare(&*pool, POPOUT, [4]);
        let printing = Arc::new(AtomicBool::new(true));

        let pump = {
            let (pool, watched, printing) = (pool.clone(), watched.clone(), printing.clone());
            thread::spawn(move || {
                let told = Told::default();
                while printing.load(Ordering::Acquire) {
                    pump_round(&pool, &watched, &told);
                    thread::yield_now();
                }
            })
        };
        let progress = [Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0))];
        let readers: Vec<_> = [MAIN, POPOUT]
            .into_iter()
            .zip(progress.clone())
            .map(|(label, pulled)| {
                let (pool, watched, printing) = (pool.clone(), watched.clone(), printing.clone());
                thread::spawn(move || {
                    let mut screen = Screen::default();
                    while printing.load(Ordering::Acquire) {
                        screen.apply(&watched.pull(&*pool, label, false));
                        pulled.fetch_add(1, Ordering::Release);
                        thread::yield_now();
                    }
                    (label, screen)
                })
            })
            .collect();

        let log = build_log(3000);
        let mut seen = [0, 0];
        for (n, piece) in pieces(&log).into_iter().enumerate() {
            pool.feed(4, piece);
            if n.is_multiple_of(PIECES_A_WAIT) {
                wait_until_each_moved(&progress, &mut seen);
            }
        }
        printing.store(false, Ordering::Release);
        pump.join().expect("the pump");
        for reader in readers {
            let (label, mut screen) = reader.join().expect("a screen");
            screen.apply(&watched.pull(&*pool, label, false));
            assert_same(label, screen.rows(4), &pool.screen(4));
        }
    });
}

/// A screen whose pull was lost asks again with `resync`, and every shell it
/// reads is paid whole — the quiet one too — while the screen beside it, which
/// lost nothing, goes on with deltas and loses nothing either.
#[test]
fn a_screen_that_lost_a_pull_is_paid_every_screen_whole_on_resync() {
    let pool = Pool::holding(&[5, 6], 6, 30);
    let watched = WatchedTerms::default();
    watched.declare(&pool, MAIN, [5, 6]);
    watched.declare(&pool, POPOUT, [5]);
    let mut main = Screen::default();
    let mut popout = Screen::default();
    main.apply(&watched.pull(&pool, MAIN, false));
    popout.apply(&watched.pull(&pool, POPOUT, false));

    pool.feed(5, b"first line\r\n");
    let lost = watched.pull(&pool, MAIN, false);
    assert_eq!(terms_of(&lost), [5], "nothing was there to lose");
    pool.feed(5, b"second line\r\n");
    popout.apply(&watched.pull(&pool, POPOUT, false));

    let paid = watched.pull(&pool, MAIN, true);
    assert_eq!(
        terms_of(&paid),
        [5, 6],
        "a resync left a shell it reads unpaid"
    );
    assert!(
        paid.screens
            .iter()
            .all(|screen| screen.frames.len() == 1 && screen.frames[0].full),
        "a resync was paid with something other than whole screens"
    );
    main.apply(&paid);
    assert_same("main after its resync", main.rows(5), &pool.screen(5));
    assert_same("main's quiet shell", main.rows(6), &pool.screen(6));

    // The other screen's next frame is a delta measured from where its own
    // model stands: the resync moved nobody else's baseline.
    pool.feed(5, b"third line\r\n");
    let next = watched.pull(&pool, POPOUT, false);
    assert!(
        next.screens[0].frames.iter().all(|frame| !frame.full),
        "the screen that lost nothing was made to start over"
    );
    popout.apply(&next);
    assert_same(
        "the pop-out beside the resync",
        popout.rows(5),
        &pool.screen(5),
    );
    main.apply(&watched.pull(&pool, MAIN, false));
    assert_same(
        "main after its resync and more",
        main.rows(5),
        &pool.screen(5),
    );
}

/// A screen that stopped reading a shell, or closed, is not told about that
/// shell again from the first round that read the declaration after it — and
/// its pulls come back empty rather than bringing it back as a reader.
#[test]
fn a_screen_that_left_or_closed_is_not_told_from_the_round_that_read_it_gone() {
    let pool = Pool::holding(&[8], 4, 20);
    let watched = WatchedTerms::default();
    let told = Told::default();
    watched.declare(&pool, MAIN, [8]);
    watched.declare(&pool, POPOUT, [8]);
    for label in [MAIN, POPOUT] {
        assert_eq!(terms_of(&watched.pull(&pool, label, false)), [8]);
        assert!(watched.pull(&pool, label, false).screens.is_empty());
    }

    // The pop-out stops reading the shell: it leaves as a reader at once.
    watched.declare(&pool, POPOUT, []);
    assert!(
        !pool.reads(8, POPOUT),
        "a screen that left is still a reader"
    );
    for round in 0..3 {
        pool.feed(8, format!("left {round}\r\n").as_bytes());
        pump_round(&pool, &watched, &told);
        assert_eq!(
            told.take(),
            [MAIN],
            "round {round} told the screen that left"
        );
        assert_eq!(terms_of(&watched.pull(&pool, MAIN, false)), [8]);
        assert!(watched.pull(&pool, MAIN, false).screens.is_empty());
        let pulled = watched.pull(&pool, POPOUT, false);
        assert!(
            pulled.screens.is_empty() && pulled.missing.is_empty(),
            "a screen that left was answered for the shell it left"
        );
        assert!(
            !pool.reads(8, POPOUT),
            "a pull brought back a screen that left"
        );
    }

    // Back again: owed the whole screen, as any new reader is.
    watched.declare(&pool, POPOUT, [8]);
    let back = watched.pull(&pool, POPOUT, false);
    assert!(back.screens.len() == 1 && back.screens[0].frames[0].full);

    // Closed: the declaration goes at once, the reader on the next round.
    watched.remove(POPOUT);
    assert!(watched.terms_of(POPOUT).is_empty());
    for round in 0..3 {
        pool.feed(8, format!("closed {round}\r\n").as_bytes());
        pump_round(&pool, &watched, &told);
        assert_eq!(
            told.take(),
            [MAIN],
            "round {round} told the screen that closed"
        );
        assert!(
            !pool.reads(8, POPOUT),
            "a closed screen is still a reader after a round"
        );
        let pulled = watched.pull(&pool, POPOUT, false);
        assert!(pulled.screens.is_empty() && pulled.missing.is_empty());
        assert_eq!(terms_of(&watched.pull(&pool, MAIN, false)), [8]);
        assert!(watched.pull(&pool, MAIN, false).screens.is_empty());
    }
}

/// What a round that read the declaration BEFORE a screen left does: it brings
/// that screen's reader back for the rest of the round and tells it once. This
/// is how the window's pump has always behaved (a declaration that lands
/// mid-round is read on the next round), pinned here so the read path is seen
/// to keep it; the cost is one empty pull, and the next round lets the reader
/// go. Scored and filed in `reports/issues/ISSUES.md` (I-t115950-1).
#[test]
fn a_round_that_read_the_declaration_before_a_screen_left_tells_it_once_more() {
    let pool = Pool::holding(&[8], 4, 20);
    let watched = WatchedTerms::default();
    let told = Told::default();
    watched.declare(&pool, MAIN, [8]);
    watched.declare(&pool, POPOUT, [8]);
    for label in [MAIN, POPOUT] {
        let _ = watched.pull(&pool, label, false);
        let _ = watched.pull(&pool, label, false);
    }

    let read_before = watched.readers_by_term();
    watched.declare(&pool, POPOUT, []);
    pool.feed(8, b"output\r\n");
    pump_round_reading(&pool, &read_before, &told);
    assert_eq!(
        told.take(),
        [POPOUT, MAIN],
        "the stale round behaved differently"
    );
    let pulled = watched.pull(&pool, POPOUT, false);
    assert!(
        pulled.screens.is_empty() && pulled.missing.is_empty(),
        "the stale notice cost the screen that left more than an empty pull"
    );

    // The next round reads the declaration again and lets the reader go.
    assert_eq!(terms_of(&watched.pull(&pool, MAIN, false)), [8]);
    assert!(watched.pull(&pool, MAIN, false).screens.is_empty());
    pool.feed(8, b"more\r\n");
    pump_round(&pool, &watched, &told);
    assert_eq!(told.take(), [MAIN]);
    assert!(!pool.reads(8, POPOUT));
}

/// Notices go out once a screen, sorted, and with no lock of the read path
/// held — so a channel may pull and declare from inside a notice, as a native
/// screen that pulls on the spot does.
#[test]
fn notices_go_out_once_a_screen_with_no_lock_held() {
    let told = Told::default();
    tell_dirty(
        [POPOUT, MAIN, MAIN, POPOUT, MAIN]
            .into_iter()
            .map(Arc::<str>::from)
            .collect(),
        &told,
    );
    assert_eq!(
        told.take(),
        [POPOUT, MAIN],
        "a screen was told more than once a round"
    );

    /// A channel that pulls, and declares again, the moment it is told.
    struct PullsOnTheSpot {
        pool: Arc<Pool>,
        watched: Arc<WatchedTerms>,
        pulled: Mutex<Vec<(String, Vec<TermId>)>>,
    }
    impl DirtyNotices for PullsOnTheSpot {
        fn dirty(&self, label: &str) {
            let answer = self.watched.pull(&*self.pool, label, false);
            self.watched
                .declare(&*self.pool, label, self.watched.terms_of(label));
            self.pulled
                .lock()
                .expect("the pulls")
                .push((label.to_string(), terms_of(&answer)));
        }
    }

    let pulled = within("a channel pulling from inside a notice", || {
        let pool = Arc::new(Pool::holding(&[1, 2], 4, 16));
        let watched = Arc::new(WatchedTerms::default());
        watched.declare(&*pool, MAIN, [1, 2]);
        watched.declare(&*pool, NATIVE, [2]);
        pool.feed(1, b"a");
        pool.feed(2, b"b");
        let channel = PullsOnTheSpot {
            pool: pool.clone(),
            watched: watched.clone(),
            pulled: Mutex::default(),
        };
        pump_round(&pool, &watched, &channel);
        channel.pulled.into_inner().expect("the pulls")
    });
    assert_eq!(
        pulled,
        [
            (MAIN.to_string(), vec![1, 2]),
            (NATIVE.to_string(), vec![2])
        ],
        "a screen told on the spot did not get its floor"
    );
}

/// While one screen changes what it reads, reads, and closes, over and over,
/// the other screen reading the same shell — pulling and now and then
/// resyncing — loses no frame; and once the pump has read the declaration
/// again, the shell's readers are exactly the screens that declared it.
#[test]
fn a_screen_changing_its_set_or_closing_never_costs_another_screen_a_frame() {
    within("one screen churning beside another", || {
        let pool = Arc::new(Pool::holding(&[2], 24, 80));
        let watched = Arc::new(WatchedTerms::default());
        watched.declare(&*pool, MAIN, [2]);
        watched.declare(&*pool, POPOUT, [2]);
        let printing = Arc::new(AtomicBool::new(true));

        let pump = {
            let (pool, watched, printing) = (pool.clone(), watched.clone(), printing.clone());
            thread::spawn(move || {
                let told = Told::default();
                while printing.load(Ordering::Acquire) {
                    pump_round(&pool, &watched, &told);
                    thread::yield_now();
                }
            })
        };
        let progress = [Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0))];
        let churn = {
            let (pool, watched, printing) = (pool.clone(), watched.clone(), printing.clone());
            let turned = progress[0].clone();
            thread::spawn(move || {
                let mut turn = 0usize;
                while printing.load(Ordering::Acquire) {
                    match turn % 5 {
                        0 => watched.declare(&*pool, POPOUT, []),
                        1 => watched.declare(&*pool, POPOUT, [2]),
                        2 => {
                            let _ = watched.pull(&*pool, POPOUT, turn.is_multiple_of(3));
                        }
                        3 => watched.remove(POPOUT),
                        _ => watched.declare(&*pool, POPOUT, [2, 77]),
                    }
                    turn += 1;
                    turned.fetch_add(1, Ordering::Release);
                    thread::yield_now();
                }
                watched.remove(POPOUT);
            })
        };
        let reader = {
            let (pool, watched, printing) = (pool.clone(), watched.clone(), printing.clone());
            let pulled = progress[1].clone();
            thread::spawn(move || {
                let mut screen = Screen::default();
                let mut pulls = 0usize;
                while printing.load(Ordering::Acquire) {
                    screen.apply(&watched.pull(&*pool, MAIN, pulls % 13 == 12));
                    pulls += 1;
                    pulled.fetch_add(1, Ordering::Release);
                    thread::yield_now();
                }
                screen
            })
        };

        let log = build_log(3000);
        let mut seen = [0, 0];
        for (n, piece) in pieces(&log).into_iter().enumerate() {
            pool.feed(2, piece);
            if n.is_multiple_of(PIECES_A_WAIT) {
                wait_until_each_moved(&progress, &mut seen);
            }
        }
        printing.store(false, Ordering::Release);
        pump.join().expect("the pump");
        churn.join().expect("the churning screen");
        let mut screen = reader.join().expect("the reading screen");

        // The pump reads the declaration again, and the readers follow it.
        pump_round(&pool, &watched, &Told::default());
        assert!(pool.reads(2, MAIN), "the screen that stayed lost its place");
        assert!(
            !pool.reads(2, POPOUT),
            "the closed screen is still a reader"
        );
        screen.apply(&watched.pull(&*pool, MAIN, false));
        assert_same(
            "the screen beside the churn",
            screen.rows(2),
            &pool.screen(2),
        );
    });
}

/// A shell that leaves the pool while declared is named `missing` from the
/// next pull, and a shell opened again under that number starts its screens
/// from a floor.
#[test]
fn a_shell_that_leaves_the_pool_is_named_missing() {
    let pool = Pool::holding(&[1, 2], 4, 16);
    let watched = WatchedTerms::default();
    watched.declare(&pool, MAIN, [1, 2]);
    assert_eq!(terms_of(&watched.pull(&pool, MAIN, false)), [1, 2]);

    pool.close(2);
    let answer = watched.pull(&pool, MAIN, false);
    assert_eq!(
        answer.missing,
        [2],
        "a declared shell that left is not named"
    );

    pool.open(2, 4, 16);
    pool.feed(2, b"again");
    let answer = watched.pull(&pool, MAIN, false);
    assert!(answer.missing.is_empty());
    assert_eq!(terms_of(&answer), [2]);
    assert!(
        answer.screens[0].frames[0].full,
        "a shell opened under a number its screen knew was not paid whole"
    );
}
