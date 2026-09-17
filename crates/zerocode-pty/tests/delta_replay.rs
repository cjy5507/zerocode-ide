//! The screen a consumer rebuilds from the frames the pump sends must equal
//! the grid's own screen — for a terminal that drew while nobody watched,
//! was then snapshotted, and then drew some more.
//!
//! The consumer model here is `ui/shell-term.js`'s: a full frame replaces
//! every row; `scrolled_lines` splices that many rows off the top and pads
//! the bottom; `view_shift` moves the rows as a block, up toward live and
//! down into history; then `rows` land by index.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use zerocode_pty::grid::{GridDelta, Terminal};
use zerocode_pty::{Look, READER_SHARE_FRAMES, READER_SHARE_SCREENS, RENOTIFY_AFTER};

fn text(cells: &[zerocode_pty::grid::Cell]) -> String {
    cells
        .iter()
        .map(|cell| cell.ch)
        .collect::<String>()
        .trim_end()
        .to_string()
}

struct Consumer {
    rows: Vec<String>,
}

impl Consumer {
    fn from_full(delta: &GridDelta) -> Self {
        assert!(delta.full, "the first frame a consumer gets is a snapshot");
        let mut rows = vec![String::new(); delta.size.0];
        for row in &delta.rows {
            rows[row.index] = text(&row.cells);
        }
        Self { rows }
    }

    fn apply(&mut self, delta: &GridDelta) {
        if delta.full {
            for row in &mut self.rows {
                row.clear();
            }
        } else if delta.scrolled_lines > 0 {
            let n = delta.scrolled_lines.min(self.rows.len());
            self.rows.drain(..n);
            while self.rows.len() < delta.size.0 {
                self.rows.push(String::new());
            }
        } else if delta.view_shift > 0 {
            let n = delta.view_shift.unsigned_abs().min(self.rows.len());
            self.rows.drain(..n);
            while self.rows.len() < delta.size.0 {
                self.rows.push(String::new());
            }
        } else if delta.view_shift < 0 {
            let n = delta.view_shift.unsigned_abs().min(self.rows.len());
            self.rows.truncate(self.rows.len() - n);
            while self.rows.len() < delta.size.0 {
                self.rows.insert(0, String::new());
            }
        }
        for row in &delta.rows {
            if row.index < self.rows.len() {
                self.rows[row.index] = text(&row.cells);
            }
        }
    }
}

fn truth(terminal: &Terminal) -> Vec<String> {
    let snapshot = terminal.grid().snapshot();
    let mut rows = vec![String::new(); snapshot.size.0];
    for row in &snapshot.rows {
        rows[row.index] = text(&row.cells);
    }
    rows
}

/// Feed `bytes` in `chunk` sized pieces: the first `unwatched` chunks with
/// nobody watching (title and bell only), then a snapshot, then every later
/// chunk as a delta. Answer the consumer's rows and the grid's rows.
fn replay(
    bytes: &[u8],
    rows: usize,
    cols: usize,
    chunk: usize,
    unwatched: usize,
) -> (Vec<String>, Vec<String>) {
    let mut terminal = Terminal::new(rows, cols);
    let mut consumer: Option<Consumer> = None;
    for (n, piece) in bytes.chunks(chunk).enumerate() {
        terminal.feed(piece);
        match consumer.as_mut() {
            None if n + 1 < unwatched => {
                let _ = terminal.grid_mut().take_news();
            }
            None => {
                let snapshot = terminal.grid_mut().take_snapshot();
                consumer = Some(Consumer::from_full(&snapshot));
            }
            Some(consumer) => {
                if let Some(delta) = terminal.grid_mut().take_delta() {
                    consumer.apply(&delta);
                }
            }
        }
    }
    let consumer = consumer.expect("at least one chunk");
    (consumer.rows, truth(&terminal))
}

fn assert_same(label: &str, got: &[String], want: &[String]) {
    let diffs: Vec<String> = got
        .iter()
        .zip(want)
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .map(|(i, (a, b))| format!("row {i}: consumer {a:?} / grid {b:?}"))
        .collect();
    assert!(
        diffs.is_empty(),
        "{label}: {} rows differ:\n{}",
        diffs.len(),
        diffs.join("\n")
    );
}

/// zo's own way of adding history above an inline head: a scroll region over
/// the rows above the head, the cursor parked on its last row, one `\r\n` per
/// line — and, when there is room under the head, a reverse-index push of the
/// head downward first. Both inside a synchronized update.
fn zo_like_stream(rows: usize) -> Vec<u8> {
    let mut out = Vec::new();
    // The head: a two-row composer at the floor.
    let top = rows - 2;
    out.extend_from_slice(format!("\x1b[?2026h\x1b[{};1H\x1b[0m\x1b[K> composer\x1b[{};1H\x1b[0m\x1b[K? for shortcuts\x1b[?2026l", top + 1, top + 2).as_bytes());
    // Forty history lines scrolled into the region above the head, ten per frame.
    for frame in 0..4 {
        out.extend_from_slice(b"\x1b[?2026h");
        out.extend_from_slice(format!("\x1b[1;{}r\x1b[{};1H", top, top).as_bytes());
        for line in 0..10 {
            out.extend_from_slice(format!("\r\n\x1b[Kh{}", frame * 10 + line).as_bytes());
        }
        out.extend_from_slice(b"\x1b[r");
        // The head repainted in place, as every frame repaints it.
        out.extend_from_slice(
            format!(
                "\x1b[{};1H\x1b[0m\x1b[K> composer {frame}\x1b[?2026l",
                top + 1
            )
            .as_bytes(),
        );
    }
    out
}

#[test]
fn a_consumer_that_watched_from_the_start_matches_the_grid() {
    let bytes = zo_like_stream(24);
    for chunk in [1usize, 7, 64, 4096] {
        let (got, want) = replay(&bytes, 24, 80, chunk, 1);
        assert_same(&format!("chunk {chunk}"), &got, &want);
    }
}

#[test]
fn a_consumer_that_started_watching_late_matches_the_grid() {
    let bytes = zo_like_stream(24);
    for (chunk, unwatched) in [(64usize, 3usize), (7, 40), (1, 500), (4096, 1)] {
        let (got, want) = replay(&bytes, 24, 80, chunk, unwatched);
        assert_same(&format!("chunk {chunk} unwatched {unwatched}"), &got, &want);
    }
}

/// A reader who scrolls back and comes home — quietly, in a fling longer than
/// the screen, beside output, and after output landed while they were back —
/// sees the live screen again, not the window they were reading. A home frame
/// that named only the rows output touched (or none) left the history window
/// on screen under a scrollbar that said live: the pane that "would not come
/// back down" when scrolled up and down (2026-09-17).
#[test]
fn a_consumer_that_scrolls_back_and_comes_home_matches_the_grid() {
    fn step(terminal: &mut Terminal, consumer: &mut Consumer, output: &[u8], lines: isize) {
        terminal.feed(output);
        terminal.grid_mut().scroll_view(lines);
        if let Some(delta) = terminal.grid_mut().take_delta() {
            consumer.apply(&delta);
        }
    }
    let mut terminal = Terminal::new(6, 8);
    for n in 0..16 {
        terminal.feed(format!("L{n}\r\n").as_bytes());
    }
    let mut consumer = Consumer::from_full(&terminal.grid_mut().take_snapshot());

    step(&mut terminal, &mut consumer, b"", 3);
    step(&mut terminal, &mut consumer, b"", -3);
    assert_same("quiet home", &consumer.rows, &truth(&terminal));

    step(&mut terminal, &mut consumer, b"", 8);
    step(&mut terminal, &mut consumer, b"", -8);
    assert_same("fling home", &consumer.rows, &truth(&terminal));

    step(&mut terminal, &mut consumer, b"", 2);
    step(&mut terminal, &mut consumer, b"L16\r\n", -3);
    assert_same("home beside output", &consumer.rows, &truth(&terminal));

    step(&mut terminal, &mut consumer, b"", 4);
    step(&mut terminal, &mut consumer, b"L17\r\nL18\r\n", 0);
    assert_eq!(
        terminal.grid().view_offset(),
        6,
        "output deepened the reader's place"
    );
    step(&mut terminal, &mut consumer, b"", -6);
    assert_same(
        "home after output landed while back",
        &consumer.rows,
        &truth(&terminal),
    );
}

/* ---- two screens, one terminal, frames pulled ----------------------------
 *
 * The screen pulls its frames now (`zerocode_pty::readers`): the pump only
 * parses, a quiet reader is told to come, and a delta is taken when a reader
 * comes for it — with a copy for every other reader of the same terminal. The
 * invariants the design names (docs/design/terminal-display-paced-frames-
 * 20260917.md §4.2) are asserted against the grid itself, at every pull:
 *
 * - I1 each reader's model, after applying what one pull brought, is the
 *   grid's screen at that moment — its last FINISHED screen, when the pull
 *   lands inside a frame the program asked to be shown whole (`?2026h`);
 * - I2 that holds for two readers of one terminal, through debts paid;
 * - I3 no share holds more than its bound between pulls;
 * - I7 the first frame any reader applies is whole. */

/// One unit of output, and whether it ends inside a synchronized frame.
///
/// Most units are whole. One kind stops halfway through a frame the program
/// asked to be shown whole, so the pump's round and the pulls land between its
/// halves — Claude Code draws this way since the terminal answers XTVERSION.
/// There the grid hands no delta over, a reader is shown the last finished
/// screen and never the half-drawn one, and a debt waits for the close.
struct Unit {
    bytes: Vec<u8>,
    mid_frame: bool,
}

fn units_of_work(rows: usize) -> Vec<Unit> {
    let whole = |bytes: Vec<u8>| Unit {
        bytes,
        mid_frame: false,
    };
    let mut units = Vec::new();
    let mut line = 0usize;
    for round in 0..240usize {
        match round % 7 {
            // A build log: a few lines to more than a screen at once.
            0..=2 => {
                let count = 1 + (round * 13) % (rows + 8);
                let mut unit = Vec::new();
                for _ in 0..count {
                    unit.extend_from_slice(
                        format!("   Compiling crate-{line} v0.{round}.0\r\n").as_bytes(),
                    );
                    line += 1;
                }
                units.push(whole(unit));
            }
            // A TUI repainting its whole screen inside one atomic frame.
            3 => {
                let mut unit = b"\x1b[?2026h\x1b[H".to_vec();
                for row in 0..rows {
                    unit.extend_from_slice(
                        format!("\x1b[{};1H\x1b[2Kstatus {round} row {row}", row + 1).as_bytes(),
                    );
                }
                unit.extend_from_slice(b"\x1b[?2026l");
                units.push(whole(unit));
            }
            // The same repaint with a round landing between its halves.
            4 => {
                let mut first = b"\x1b[?2026h\x1b[H".to_vec();
                let mut second = Vec::new();
                for row in 0..rows {
                    let half = if row < rows / 2 {
                        &mut first
                    } else {
                        &mut second
                    };
                    half.extend_from_slice(
                        format!("\x1b[{};1H\x1b[2Kstatus {round} row {row}", row + 1).as_bytes(),
                    );
                }
                second.extend_from_slice(b"\x1b[?2026l");
                units.push(Unit {
                    bytes: first,
                    mid_frame: true,
                });
                units.push(whole(second));
            }
            // Into the alternate screen and out again: full frames both ways,
            // with a bell and a title that must not disturb the screen.
            5 => units.push(whole(
                format!("\x1b[?1049h\x1b[Hvim {round}\x07\x1b]2;editing {round}\x07").into_bytes(),
            )),
            _ => units.push(whole(b"\x1b[?1049l".to_vec())),
        }
    }
    units
}

/// A deterministic coin, so a failure names the same interleaving every run.
struct Coin(u64);

impl Coin {
    fn flip(&mut self, percent: u64) -> bool {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) % 100 < percent
    }
}

/// A screen reading through the readers: `None` until its first frame.
struct Screen {
    label: &'static str,
    model: Option<Consumer>,
    pulls: usize,
}

impl Screen {
    fn pull(&mut self, terminal: &mut Terminal, now: Instant) {
        let (readers, grid) = terminal.readers_mut();
        for frame in readers.pull(self.label, grid, now) {
            match self.model.as_mut() {
                None => self.model = Some(Consumer::from_full(&frame)),
                Some(model) => model.apply(&frame),
            }
        }
        self.pulls += 1;
    }
}

fn assert_bounded(terminal: &mut Terminal, labels: &[&str], at: &str) {
    let (rows, cols) = {
        let grid = terminal.grid();
        (grid.rows(), grid.cols())
    };
    let (readers, _) = terminal.readers_mut();
    for label in labels {
        let share = readers
            .share_of(label)
            .expect("a declared reader is a reader");
        assert!(
            share.frames <= READER_SHARE_FRAMES
                && share.cells <= rows * cols * READER_SHARE_SCREENS,
            "{at}: {label} holds {share:?}, past its bound"
        );
    }
}

#[test]
fn two_screens_pulling_one_terminal_each_match_the_grid_at_their_own_pull() {
    const ROWS: usize = 24;
    const COLS: usize = 80;
    let labels = ["main", "board-popout"];
    let mut terminal = Terminal::new(ROWS, COLS);
    terminal.readers_mut().0.reconcile(&labels);
    let mut main = Screen {
        label: labels[0],
        model: None,
        pulls: 0,
    };
    let mut popout = Screen {
        label: labels[1],
        model: None,
        pulls: 0,
    };
    let mut coin = Coin(0x5eed);
    // Some rounds are chase rounds — a person just wrote — and those tell a
    // flowing reader too, with the take made for it. Every model must stay
    // whole just the same. A coin of its own, so the pulls interleave exactly
    // as they did before chase rounds existed.
    let mut chased = Coin(0x00c4_a5ed);
    let mut chase_looks = 0usize;
    let mut now = Instant::now();
    let mut told: Vec<Arc<str>> = Vec::new();
    let mut debts_paid = 0usize;
    let mut mid_frame_pulls = 0usize;
    let mut debts_waited = 0usize;
    // Every screen the program finished, the last one last.
    let mut finished = vec![truth(&terminal)];

    for (n, unit) in units_of_work(ROWS).iter().enumerate() {
        terminal.feed(&unit.bytes);
        if !unit.mid_frame {
            finished.push(truth(&terminal));
        }
        now += Duration::from_millis(16);
        // The pump's round: whoever is due is told.
        told.clear();
        {
            let look = if chased.flip(25) {
                chase_looks += 1;
                Look::Chase
            } else {
                Look::Beat
            };
            let (readers, grid) = terminal.readers_mut();
            readers.look(grid, now, look, &mut told);
        }
        assert_bounded(&mut terminal, &labels, &format!("unit {n} after the look"));

        // The main window reads nearly every frame. The pop-out reads rarely,
        // and not at all for a long stretch — the hidden window whose share
        // must fold into a debt rather than grow.
        let popout_away = (60..140).contains(&n);
        let main_pulls = coin.flip(80);
        let popout_pulls = !popout_away && coin.flip(30);
        for (screen, pulls) in [(&mut main, main_pulls), (&mut popout, popout_pulls)] {
            if !pulls {
                continue;
            }
            let label = screen.label;
            let owes = |terminal: &mut Terminal| {
                terminal
                    .readers_mut()
                    .0
                    .share_of(label)
                    .expect("declared")
                    .owes_snapshot
            };
            let owed = owes(&mut terminal);
            let first = screen.model.is_none();
            screen.pull(&mut terminal, now);
            let still_owed = owes(&mut terminal);
            if owed && !still_owed && !first {
                debts_paid += 1;
            }
            mid_frame_pulls += usize::from(unit.mid_frame);
            if still_owed {
                // Only an open frame keeps a debt from being paid, and then the
                // reader keeps the older screen it had — never the half-drawn
                // one — until the frame closes and it is told.
                assert!(
                    unit.mid_frame,
                    "unit {n}: {}'s pull left its debt unpaid with no frame open",
                    screen.label
                );
                debts_waited += 1;
                continue;
            }
            let model = screen
                .model
                .as_ref()
                .expect("a paid debt always brings the first screen");
            if unit.mid_frame {
                // The grid hands nothing over while a frame is open — not even
                // what changed before it opened — so the reader stands on the
                // screen as it was at its last take: a finished one, never the
                // half-drawn one.
                assert!(
                    finished.contains(&model.rows),
                    "unit {n}: {} shows a screen the program never finished:\n{}",
                    screen.label,
                    model.rows.join("\n")
                );
            } else {
                assert_same(
                    &format!(
                        "unit {n}: {} after its pull #{}",
                        screen.label, screen.pulls
                    ),
                    &model.rows,
                    finished.last().expect("the first screen is finished"),
                );
            }
            assert_bounded(
                &mut terminal,
                &labels,
                &format!("unit {n} after {}'s pull", screen.label),
            );
        }
    }
    // The long absence was long enough to cost a debt, and the debt was paid
    // with a screen that matched.
    assert!(
        debts_paid > 0,
        "the pop-out never outgrew its share, so no debt was exercised"
    );
    assert!(
        chase_looks > 0,
        "no round was a chase round, so a flowing reader was never told"
    );
    assert!(
        mid_frame_pulls > 0 && debts_waited > 0,
        "no pull (or no debt) landed inside an open frame, so the hold was never \
         exercised: {mid_frame_pulls} pulls, {debts_waited} debts"
    );
    // And a silent reader is told again, never left waiting on a lost notice.
    now += RENOTIFY_AFTER;
    terminal.feed(b"one more line\r\n");
    told.clear();
    let (readers, grid) = terminal.readers_mut();
    readers.look(grid, now, Look::Beat, &mut told);
    assert!(
        told.iter().any(|label| &**label == "board-popout"),
        "a reader silent past the renotify interval was not told again: {told:?}"
    );
}

/// A real zo session's bytes, when a capture is at hand
/// (`ZC_DELTA_REPLAY_BYTES=<file>`, 40 rows by 120 columns as the e2e
/// harness records them). Skipped otherwise.
#[test]
fn a_real_zo_capture_replays_the_same_through_snapshot_and_deltas() {
    let Some(path) = std::env::var_os("ZC_DELTA_REPLAY_BYTES").map(PathBuf::from) else {
        return;
    };
    let bytes = std::fs::read(&path).expect("read the capture");
    for (chunk, unwatched) in [(4096usize, 1usize), (256, 2), (64, 8), (16, 40)] {
        let (got, want) = replay(&bytes, 40, 120, chunk, unwatched);
        assert_same(
            &format!("real capture chunk {chunk} unwatched {unwatched}"),
            &got,
            &want,
        );
    }
}

/// Print what a ZeroCode pane would show for a capture: the grid's composed
/// live screen (collapsed fold bodies hidden, rows filled from history) next
/// to its raw rows, and the fold table. A probe, not an assertion:
/// `ZC_DELTA_REPLAY_BYTES=<file> cargo test -p zerocode-pty --test delta_replay -- --ignored --nocapture dump`.
#[test]
#[ignore]
fn dump_real_capture_screen() {
    let Some(path) = std::env::var_os("ZC_DELTA_REPLAY_BYTES").map(PathBuf::from) else {
        return;
    };
    let bytes = std::fs::read(&path).expect("read the capture");
    let mut terminal = Terminal::new(40, 120);
    terminal.feed(&bytes);
    let snapshot = terminal.grid().snapshot();
    use std::collections::HashMap;
    let collapsed: HashMap<u32, bool> =
        snapshot.folds.iter().map(|m| (m.id, m.collapsed)).collect();
    println!(
        "== composed live screen ({} rows sent, cursor {:?}, scrollback {})",
        snapshot.rows.len(),
        snapshot.cursor,
        snapshot.scrollback_len
    );
    for row in &snapshot.rows {
        let t = text(&row.cells);
        let shown: String = t.chars().take(80).collect();
        let tag = match &row.fold {
            Some(f) => {
                let c = collapsed.get(&f.id).copied().unwrap_or(false);
                // The frontend hides on the live screen: body+collapsed, or teaser+open.
                let hidden = (matches!(f.role, zerocode_pty::grid::FoldRole::Body) && c)
                    || (matches!(f.role, zerocode_pty::grid::FoldRole::Teaser) && !c);
                format!(
                    "[{:?} id{} {} {}]",
                    f.role,
                    f.id,
                    if c { "COLL" } else { "open" },
                    if hidden { "HIDE!" } else { "show" }
                )
            }
            None => "                ".to_string(),
        };
        println!("{:2}|{:32}|{}", row.index, tag, shown);
    }
    println!("== fold table ({})", snapshot.folds.len());
    for meta in &snapshot.folds {
        println!("{meta:?}");
    }
}
