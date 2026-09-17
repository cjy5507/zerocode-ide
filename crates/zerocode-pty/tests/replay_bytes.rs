//! Replay a captured byte stream through the window's grid and print what a
//! person would see — the two-sided check for "the TUI's bytes are right under
//! pyte; what does OUR emulator make of them?". Ignored: it reads a file named
//! by `ZC_REPLAY_BYTES` (and `ZC_REPLAY_ROWS`/`ZC_REPLAY_COLS`, default 40×180)
//! and asserts nothing — a diagnostic, run with `--ignored --nocapture`.
use zerocode_pty::Terminal;

#[test]
#[ignore = "diagnostic replay of a captured byte stream; set ZC_REPLAY_BYTES"]
fn replay_captured_bytes_and_print_rows() {
    let path = std::env::var("ZC_REPLAY_BYTES").expect("ZC_REPLAY_BYTES names the capture");
    let rows: usize = std::env::var("ZC_REPLAY_ROWS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(40);
    let cols: usize = std::env::var("ZC_REPLAY_COLS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(180);
    let bytes = std::fs::read(&path).expect("read the capture");
    let mut terminal = Terminal::new(rows, cols);
    terminal.feed(&bytes);
    let grid = terminal.grid();
    for row in 0..rows {
        let line = grid.line(row);
        if line.trim().is_empty() {
            continue;
        }
        let cells = grid.row_cells(row);
        let bars: Vec<usize> = cells
            .iter()
            .enumerate()
            .filter(|(_, cell)| cell.ch == '│')
            .map(|(index, _)| index)
            .collect();
        let hangul = cells
            .iter()
            .filter(|cell| ('\u{AC00}'..='\u{D7A3}').contains(&cell.ch))
            .count();
        let tail: String = cells
            .iter()
            .skip(cols.saturating_sub(10))
            .map(|cell| cell.ch)
            .collect();
        println!(
            "{row:02} bars={bars:?} hangul={hangul} tail={tail:?} head={:?}",
            line.chars().take(40).collect::<String>()
        );
    }
    let delta = grid.snapshot();
    println!("cursor {:?}", delta.cursor);
}
