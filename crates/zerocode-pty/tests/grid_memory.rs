//! What a pane's grid holds in memory (t-54317).
//!
//! Two things live here. The contract — a history line costs about what it
//! shows, not a full screen cell per written column — is an ordinary test
//! that counts live bytes with the shared counting allocator. The census is an
//! ignored test that builds the window's scenarios in this process (1, 10 and
//! 30 panes, a 5,000-line and a 50,000-line history) and holds each one while
//! `tools/memory-census/grid.py` reads the process from outside: the heap's
//! size histogram and the physical footprint, never the resident size alone
//! (a terminal grid's memory is read from the histogram, not from RSS). It
//! runs only when that script names a folder in `ZEROCODE_GRID_CENSUS`.

mod support;

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use support::live_bytes;
use zerocode_pty::{Cell, DEFAULT_SCROLLBACK_LINES, MAX_SCROLLBACK_LINES, Terminal};

/// The pane a person actually opens — the window's own fixtures measure the
/// same size (`REAL_PANE`, ui/tests/terminal-frames.mjs).
const ROWS: usize = 60;
const COLS: usize = 220;

/// Words the transcript is made of. Every line is synthetic.
const WORDS: [&str; 16] = [
    "the",
    "grid",
    "keeps",
    "history",
    "rows",
    "while",
    "panes",
    "stream",
    "agent",
    "output",
    "into",
    "scrollback",
    "and",
    "each",
    "cell",
    "costs",
];

/// One line of an inline agent's transcript (zo and codex draw no alternate
/// screen, so every finished line of theirs scrolls into the pane's history),
/// in the shapes that transcript is made of: prose with a bold lead and an
/// inline code span, a diff's added and removed lines with their background on
/// the text, a tool call's header, a highlighted code line, and the blank line
/// between blocks.
fn transcript_line(n: usize) -> String {
    let words = |count: usize| {
        (0..count)
            .map(|at| WORDS[(n * 7 + at * 3) % WORDS.len()])
            .collect::<Vec<_>>()
            .join(" ")
    };
    match n % 10 {
        0..=4 => format!(
            "\x1b[1m{}\x1b[22m {} \x1b[36m`value_{n}`\x1b[39m {}",
            words(2),
            words(10),
            words(4 + n % 9)
        ),
        5 => format!(
            "\x1b[38;5;245m{:>4} \x1b[32;48;5;22m+ let value_{n} = compute({n}, \"{}\");\x1b[0m",
            n % 1000,
            words(3)
        ),
        6 => format!(
            "\x1b[38;5;245m{:>4} \x1b[31;48;5;52m- let value_{n} = compute({});\x1b[0m",
            n % 1000,
            n / 2
        ),
        7 => format!(
            "\x1b[32m\u{25cf}\x1b[39m \x1b[1mBash\x1b[22m(cargo test -p zerocode-pty --test grid_memory -- case_{n})"
        ),
        8 => format!(
            "    \x1b[35mfn\x1b[39m \x1b[33mpaint_{n}\x1b[39m(list: &mut List) {{ list.push(row_of({n})); }}"
        ),
        _ => String::new(),
    }
}

/// `lines` transcript lines, each ended the way a line leaves the bottom row.
fn transcript(lines: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    for n in 0..lines {
        bytes.extend_from_slice(transcript_line(n).as_bytes());
        bytes.extend_from_slice(b"\r\n");
    }
    bytes
}

/// An inline agent's pane with `lines` of transcript behind it, the history
/// kept to `cap`, and its screen taken once, as the window's pump takes it.
fn inline_pane(lines: usize, cap: usize) -> Terminal {
    let mut terminal = Terminal::new(ROWS, COLS);
    terminal.grid_mut().set_scrollback_cap(cap);
    terminal.feed(&transcript(lines));
    let _ = terminal.grid_mut().take_delta();
    terminal
}

/// A full-screen agent's pane (Claude Code draws on the alternate screen): a
/// few shell lines parked on the primary screen and a whole painted frame on
/// the alternate one, taken once.
fn fullscreen_pane() -> Terminal {
    let mut terminal = Terminal::new(ROWS, COLS);
    terminal.feed(&transcript(12));
    terminal.feed(b"\x1b[?1049h\x1b[H\x1b[2J");
    for row in 0..ROWS {
        let line = transcript_line(row);
        terminal.feed(format!("\x1b[{};1H{line}", row + 1).as_bytes());
    }
    let _ = terminal.grid_mut().take_delta();
    terminal
}

/// History lines an inline pane of the panes scenarios carries: a working
/// session's worth, short of the cap.
const PANE_HISTORY_LINES: usize = 1_000;

/// `count` panes, half of them inline agents and half full-screen ones.
fn panes(count: usize) -> Vec<Terminal> {
    (0..count)
        .map(|at| {
            if at % 2 == 0 {
                inline_pane(PANE_HISTORY_LINES, DEFAULT_SCROLLBACK_LINES)
            } else {
                fullscreen_pane()
            }
        })
        .collect()
}

/// The counting allocator is one per process; the tests that read it take
/// turns, so one test's allocations never land in another's count.
static MEASURING: Mutex<()> = Mutex::new(());

fn measuring() -> MutexGuard<'static, ()> {
    MEASURING.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A history line's bill: at most this many bytes for each character it
/// shows, plus this many for the line itself (its place in the history and
/// the style runs that dress it).
const HISTORY_BYTES_PER_CHAR: usize = 8;
const HISTORY_BYTES_PER_LINE: usize = 160;

/// A full history costs its text, not a screen cell per written column.
///
/// An inline agent fills its pane's history with every line it finishes. Kept
/// as screen cells — 44 bytes a column — the default 5,000 lines of this
/// transcript held about 19 MiB for one pane, and a window runs ten such panes.
#[test]
fn a_full_history_costs_its_text_not_a_screen_cell_per_column() {
    let _turn = measuring();
    let mut terminal = Terminal::new(ROWS, COLS);
    terminal
        .grid_mut()
        .set_scrollback_cap(DEFAULT_SCROLLBACK_LINES);
    let before = live_bytes();
    terminal.feed(&transcript(DEFAULT_SCROLLBACK_LINES + ROWS));
    let held = live_bytes().saturating_sub(before);

    let grid = terminal.grid();
    let lines = grid.scrollback_len();
    assert_eq!(lines, DEFAULT_SCROLLBACK_LINES);
    let shown: usize = (0..lines)
        .map(|index| grid.scrollback_line(index).chars().count())
        .sum();
    let bill = HISTORY_BYTES_PER_CHAR * shown + HISTORY_BYTES_PER_LINE * lines;
    assert!(
        held <= bill,
        "{lines} history lines showing {shown} characters held {held} bytes \
         ({} per line), over the bill of {bill} ({HISTORY_BYTES_PER_CHAR} a character \
         and {HISTORY_BYTES_PER_LINE} a line)",
        held / lines.max(1)
    );
}

/// What the window's history budget gives back (t-80604): a pane at the
/// deepest history a person may choose, shed to the grid's default the way
/// `history_budget` sheds a quiet pane, returns all but the default's share,
/// and what it still holds is the default's bill. The numbers ride the
/// failure message and `--nocapture`, for the task's before/after table.
#[test]
fn a_deep_pane_shed_to_the_default_gives_back_all_but_the_defaults_share() {
    let _turn = measuring();
    let before = live_bytes();
    let mut terminal = inline_pane(MAX_SCROLLBACK_LINES + ROWS, MAX_SCROLLBACK_LINES);
    let deep = live_bytes().saturating_sub(before);
    let dropped = terminal.grid_mut().shed_history(DEFAULT_SCROLLBACK_LINES);
    let shed = live_bytes().saturating_sub(before);

    let grid = terminal.grid();
    assert_eq!(dropped, MAX_SCROLLBACK_LINES - DEFAULT_SCROLLBACK_LINES);
    assert_eq!(grid.scrollback_len(), DEFAULT_SCROLLBACK_LINES);
    assert_eq!(
        grid.scrollback_cap(),
        MAX_SCROLLBACK_LINES,
        "the person's depth moved"
    );
    let shown: usize = (0..DEFAULT_SCROLLBACK_LINES)
        .map(|index| grid.scrollback_line(index).chars().count())
        .sum();
    let screen = ROWS * COLS * size_of::<Cell>();
    let bill = HISTORY_BYTES_PER_CHAR * shown + HISTORY_BYTES_PER_LINE * DEFAULT_SCROLLBACK_LINES;
    let mib = |bytes: usize| bytes as f64 / (1024.0 * 1024.0);
    let said = format!(
        "{MAX_SCROLLBACK_LINES} lines held {:.1} MiB; shed to {DEFAULT_SCROLLBACK_LINES} lines \
         it holds {:.1} MiB (bill {:.1} MiB for the history plus screens of {:.1} MiB)",
        mib(deep),
        mib(shed),
        mib(bill),
        mib(screen * 3)
    );
    eprintln!("{said}");
    // The history's bill, plus the screens a pane holds whatever its history:
    // the live one, the sent copy and the queues' spare room.
    assert!(shed <= bill + screen * 3, "{said}");
    assert!(
        shed * 4 <= deep,
        "shedding gave back less than three quarters: {said}"
    );
}

/// A pane draws on one screen of cells; the other screens it keeps cost what
/// they show.
///
/// A full-screen agent (Claude Code) parks the primary screen under its
/// alternate one for its whole session, and every pane keeps a copy of each
/// row the window was last sent, to answer "did this row change?". Kept as
/// cells, each was another screen — 580 KB at 220×60 — so one full-screen pane
/// held three screens' worth, and a window runs fifteen of them.
#[test]
fn a_panes_kept_screens_cost_what_they_show() {
    let _turn = measuring();
    let before = live_bytes();
    let terminal = fullscreen_pane();
    let held = live_bytes().saturating_sub(before);
    let screen = ROWS * COLS * size_of::<Cell>();
    assert!(
        held <= screen + screen / 2,
        "a full-screen pane held {held} bytes, {:.2} screens of cells ({screen} bytes each): \
         the parked primary screen and the sent copy cost more than what they show",
        held as f64 / screen as f64
    );
    drop(terminal);
}

/// The window's scenarios, by the name the census script asks for.
fn scenario(name: &str) -> Option<Vec<Terminal>> {
    Some(match name {
        "empty" => Vec::new(),
        "panes-1" => panes(1),
        "panes-10" => panes(10),
        "panes-30" => panes(30),
        "scrollback-5000" => vec![inline_pane(
            DEFAULT_SCROLLBACK_LINES + ROWS,
            DEFAULT_SCROLLBACK_LINES,
        )],
        "scrollback-50000" => vec![inline_pane(
            MAX_SCROLLBACK_LINES + ROWS,
            MAX_SCROLLBACK_LINES,
        )],
        _ => return None,
    })
}

/// How long the census waits for the script to read one scenario.
const CENSUS_PATIENCE: Duration = Duration::from_secs(300);

/// Wait for `go`, the script's word that it has read this scenario.
fn wait_for(go: &Path) -> bool {
    let began = Instant::now();
    while !go.exists() {
        if began.elapsed() > CENSUS_PATIENCE {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    true
}

/// The census: each scenario named in `ZEROCODE_GRID_CENSUS_SCENARIOS` is
/// built, announced in `<folder>/<name>.ready` with this process's id and
/// what the counting allocator holds, and kept until `<folder>/<name>.go`
/// appears. Run by `tools/memory-census/grid.py`; a plain run skips it.
#[test]
#[ignore = "a measurement, run by tools/memory-census/grid.py"]
fn census() {
    let Some(folder) = std::env::var_os("ZEROCODE_GRID_CENSUS").map(PathBuf::from) else {
        return;
    };
    let _turn = measuring();
    let names = std::env::var("ZEROCODE_GRID_CENSUS_SCENARIOS").unwrap_or_default();
    let baseline = live_bytes();
    for name in names.split(',').filter(|name| !name.is_empty()) {
        let built = scenario(name).unwrap_or_else(|| panic!("no scenario named {name}"));
        let history: usize = built
            .iter()
            .map(|terminal| terminal.grid().scrollback_len())
            .sum();
        let ready = format!(
            "{{\"pid\":{},\"live_bytes\":{},\"panes\":{},\"history_lines\":{history}}}\n",
            std::process::id(),
            live_bytes().saturating_sub(baseline),
            built.len(),
        );
        std::fs::write(folder.join(format!("{name}.ready")), ready)
            .expect("the census folder takes the ready note");
        assert!(
            wait_for(&folder.join(format!("{name}.go"))),
            "the census script never read {name}"
        );
        drop(built);
    }
}
