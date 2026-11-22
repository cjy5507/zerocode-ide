//! Phase 0 recordings for the native terminal comparison.
//!
//! A recording is the frames a ZeroCode terminal hands its screen, written
//! down once so the DOM renderer and the GPUI renderer are fed the very same
//! frames. Bytes go into `zerocode_pty::Terminal` as a pty read would put them,
//! and after every step the frames are taken the way the window's `term_pull`
//! takes them: `FrameReaders::pull` for one reader. The first pull is that
//! reader's floor (a full snapshot); every later one is the grid's delta since
//! the one before, so a recording is exactly what one window would be handed.
//!
//! Per case `<name>`, in the output folder:
//!
//! - `<name>.deltas.jsonl` — one frame a line, the `serde_json` of a
//!   `GridDelta`: the bytes `term_pull` puts in an answer's `frames`.
//! - `<name>.reference.jsonl` — `{"frame":k,"screen":<GridDelta>}` at each
//!   checkpoint: the grid's own screen after frame `k`, read with
//!   `TerminalGrid::snapshot` (read-only), never rebuilt from the frames.
//! - `<name>.ansi` — every byte fed, in order.
//! - `<name>.meta.json` — the size, which step made which frame, and the
//!   checkpoints.
//!
//! Nothing here reads a clock into a frame: a synchronized-output block
//! (`CSI ?2026h`) is opened and closed inside one step, because the grid
//! gives up on an unclosed one after a wall-clock timeout.

use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde_json::json;
use zerocode_pty::{TERMINAL_VERSION, Terminal};

/// The one reader a recording takes frames for, as a window label would.
const READER: &str = "phase0";

/// The pane the window's own terminal measurements use (`ui/tests/terminal-frames.mjs`, `REAL_PANE`).
const ROWS: usize = 60;
const COLS: usize = 220;

/// What happens before one pull.
enum Step {
    /// Bytes the program wrote.
    Feed(Vec<u8>),
    /// A person scrolled the view through history (`term_scroll`): positive is back.
    ViewScroll(isize),
    /// The view snapped back to the live screen.
    ViewToBottom,
    /// The window's last pull failed and it asks for every screen whole (`term_pull` with `resync`).
    Resync,
    /// The pane changed size.
    Resize(usize, usize),
}

impl Step {
    const fn kind(&self) -> &'static str {
        match self {
            Self::Feed(_) => "feed",
            Self::ViewScroll(_) => "view-scroll",
            Self::ViewToBottom => "view-to-bottom",
            Self::Resync => "resync",
            Self::Resize(..) => "resize",
        }
    }
}

struct Case {
    name: String,
    about: &'static str,
    rows: usize,
    cols: usize,
    steps: Vec<Step>,
}

/// A small fixed-seed generator, so every case is the same bytes on every machine.
struct Dice(u64);

impl Dice {
    fn next(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, bound: usize) -> usize {
        let bound = u64::try_from(bound.max(1)).unwrap_or(u64::MAX);
        usize::try_from(self.next() % bound).unwrap_or(0)
    }
}

fn cup(row: usize, col: usize) -> String {
    format!("\x1b[{};{}H", row + 1, col + 1)
}

fn feed(text: impl Into<String>) -> Step {
    Step::Feed(text.into().into_bytes())
}

/// A build log streaming past: three short lines a frame, two colour runs each.
fn sparse_log(rows: usize, cols: usize) -> Case {
    const VERBS: [&str; 5] = ["Compiling", "Checking", "Finished", "Running", "Fresh"];
    let mut steps = vec![feed("\x1b[H\x1b[2J")];
    let mut line = 0;
    for _ in 0..240 {
        let mut text = String::new();
        for _ in 0..3 {
            let verb = VERBS[line % VERBS.len()];
            let _ = write!(
                text,
                "   \x1b[1;32m{verb}\x1b[0m zerocode-shell v1.1.165 \x1b[2m(/workspace/crates/zerocode-shell)\x1b[0m #{line}\r\n"
            );
            line += 1;
        }
        steps.push(feed(text));
    }
    Case {
        name: "sparse-log".into(),
        about: "a build log scrolling three lines a frame; most cells blank",
        rows,
        cols,
        steps,
    }
}

/// A full-screen program repainting every cell, the colour changing every four columns.
fn dense_color(rows: usize, cols: usize) -> Case {
    const INK: [char; 24] = [
        'a', 'b', 'c', 'd', 'e', 'f', '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', '▏', '▎',
        '▍', '▌', '█', '─', '│', '┼',
    ];
    let mut steps = vec![feed("\x1b[?1049h\x1b[?25l\x1b[H\x1b[2J")];
    let frames = 90;
    for frame in 0..frames {
        let mut text = String::from("\x1b[?2026h");
        for row in 0..rows {
            text.push_str(&cup(row, 0));
            let mut col = 0;
            while col < cols {
                let run = 4.min(cols - col);
                let fg = (frame * 7 + row * 3 + col / 4) % 256;
                let _ = write!(text, "\x1b[38;5;{fg}m");
                if (col / 4 + row) % 3 == 0 {
                    let _ = write!(text, "\x1b[48;5;{}m", (fg + 128) % 256);
                }
                if (col / 4 + frame) % 11 == 0 {
                    text.push_str("\x1b[1m");
                }
                for at in 0..run {
                    text.push(INK[(frame + row + col + at) % INK.len()]);
                }
                text.push_str("\x1b[0m");
                col += run;
            }
        }
        if frame + 1 == frames {
            text.push_str(&cup(rows / 2, cols / 2));
            text.push_str("\x1b[?25h");
        }
        text.push_str("\x1b[?2026l");
        steps.push(feed(text));
    }
    Case {
        name: "dense-color".into(),
        about: "alternate screen, every row repainted each frame, a colour run every four columns",
        rows,
        cols,
        steps,
    }
}

/// Lines longer than the pane, wrapping; a wide glyph that meets the last column wraps early.
fn wrap(rows: usize, cols: usize) -> Case {
    let mut dice = Dice(0x5eed_0003);
    let mut steps = vec![feed("\x1b[H\x1b[2J")];
    for frame in 0..100 {
        let mut text = String::new();
        // A long line of words with an address straddling the wrap.
        let mut words = String::new();
        while words.chars().count() < cols - 30 + frame % 7 {
            let _ = write!(words, "word{} ", dice.below(1000));
        }
        let _ = write!(
            text,
            "{words}https://example.com/phase0/{frame}/a/long/path/that/wraps?x={}\r\n",
            dice.below(100_000)
        );
        // A wide glyph landing on the last column (or not), then more Korean.
        let lead = cols - 1 - frame % 3;
        let _ = write!(
            text,
            "\x1b[36m{}\x1b[0m가나다라마바사아자차카타파하 끝{frame}\r\n",
            "x".repeat(lead)
        );
        // Every fifth frame, an OSC 8 link across the edge.
        if frame % 5 == 0 {
            let _ = write!(
                text,
                "{}\x1b]8;;https://example.com/osc8/{frame}\x1b\\linked text over the edge\x1b]8;;\x1b\\\r\n",
                "-".repeat(cols - 10)
            );
        }
        steps.push(feed(text));
    }
    Case {
        name: "wrap".into(),
        about: "lines longer than the pane; a wide glyph at the last column wraps early; links across the edge",
        rows,
        cols,
        steps,
    }
}

/// Korean prose: precomposed and decomposed syllables, bold and colour, wrapping at the edge.
fn hangul(rows: usize, cols: usize) -> Case {
    const SENTENCES: [&str; 6] = [
        "터미널은 격자이고, 한글 한 글자는 두 칸을 차지한다.",
        "섞어 쓰기: mixed 한글 text 123 그리고 기호 ·・「」",
        "조합형 자모: \u{1112}\u{1161}\u{11ab}\u{1100}\u{1173}\u{11af} 은 한 칸에 모인다",
        "받침 있는 글자들 각낚닦랅맒밟삵앎잚찷칹탍팖핥",
        "옛한글 \u{1100}\u{119e}\u{11a8} 과 현대 한글 가",
        "문장 끝에 공백과 마침표.   ",
    ];
    let mut steps = vec![feed("\x1b[H\x1b[2J")];
    for frame in 0..120 {
        let mut text = String::new();
        let sentence = SENTENCES[frame % SENTENCES.len()];
        let color = 31 + frame % 7;
        let _ = write!(
            text,
            "\x1b[1;{color}m[{frame:03}]\x1b[0m {sentence} \x1b[4m밑줄\x1b[0m\r\n"
        );
        if frame % 4 == 0 {
            // Korean that runs into the right edge.
            let mut long = String::new();
            while long.chars().count() * 2 < cols + 30 {
                long.push_str("가나다라 ");
            }
            let _ = write!(text, "{long}\r\n");
        }
        steps.push(feed(text));
    }
    Case {
        name: "hangul".into(),
        about: "Korean prose with bold, colour and underline; decomposed jamo; Korean wrapping at the edge",
        rows,
        cols,
        steps,
    }
}

/// Emoji: presentation selectors, joined sequences, flags, skin tones, combining marks.
fn emoji(rows: usize, cols: usize) -> Case {
    const SAMPLES: [&str; 12] = [
        "😀 grin",
        "👨\u{200d}👩\u{200d}👧 family",
        "👍\u{1f3fd} tone",
        "\u{1f1f0}\u{1f1f7} flag",
        "\u{2602}\u{fe0f} umbrella-vs16",
        "\u{2602} umbrella-text",
        "1\u{fe0f}\u{20e3} keycap",
        "e\u{301} combining",
        "\u{1f3f3}\u{fe0f}\u{200d}\u{1f308} rainbow",
        "\u{1f9d1}\u{1f3fb}\u{200d}\u{1f4bb} technologist",
        "日本語 漢字 中文",
        "✓ ✗ ★ ☆ → ← ⚠\u{fe0f}",
    ];
    let mut steps = vec![feed("\x1b[H\x1b[2J")];
    for frame in 0..120 {
        let mut text = String::new();
        let _ = write!(text, "[{frame:03}] ");
        for at in 0..4 {
            let _ = write!(text, "{}  ", SAMPLES[(frame + at * 3) % SAMPLES.len()]);
        }
        text.push_str("\r\n");
        if frame % 4 == 0 {
            // An emoji meeting the last column.
            let _ = write!(text, "{}😀🚀\r\n", ".".repeat(cols - 1 - frame % 2));
        }
        steps.push(feed(text));
    }
    Case {
        name: "emoji".into(),
        about: "emoji with selectors, joiners, flags, skin tones, keycaps and combining marks; emoji at the edge",
        rows,
        cols,
        steps,
    }
}

/// The cursor moving over a still screen: small edits, erases, inserts, hides and shows.
fn cursor(rows: usize, cols: usize) -> Case {
    let mut dice = Dice(0x5eed_0006);
    let mut floor = String::from("\x1b[H\x1b[2J\x1b[2m");
    for row in 0..rows {
        floor.push_str(&cup(row, 0));
        floor.push_str(&"·".repeat(cols));
    }
    floor.push_str("\x1b[0m");
    floor.push_str(&cup(0, 0));
    let mut steps = vec![feed(floor)];
    const WORDS: [&str; 6] = ["ok", "한글", "x", "error", "😀", "→→"];
    for frame in 0..160 {
        let row = dice.below(rows);
        let col = dice.below(cols - 8);
        let word = WORDS[dice.below(WORDS.len())];
        let style = match dice.below(5) {
            0 => "\x1b[7m",
            1 => "\x1b[1;33m",
            2 => "\x1b[3;35m",
            3 => "\x1b[9m",
            _ => "",
        };
        let text = match frame % 9 {
            0 | 1 => format!("{}{style}{word}\x1b[0m", cup(row, col)),
            2 => format!("{}\x1b[K", cup(row, col)),
            3 => format!("{}\x1b[2@{style}{word}\x1b[0m", cup(row, col)),
            4 => format!("{}\x1b[3P", cup(row, col)),
            5 => format!(
                "\x1b7{}{style}{word}\x1b[0m\x1b8",
                cup(dice.below(rows), dice.below(cols - 8))
            ),
            6 => "\x1b[?25l".to_string(),
            7 => format!("\x1b[?25h{}", cup(row, col)),
            _ => cup(row, col),
        };
        steps.push(feed(text));
    }
    Case {
        name: "cursor".into(),
        about: "a still screen with small edits: moves, erases, inserts, deletes, save and restore, hide and show",
        rows,
        cols,
        steps,
    }
}

/// Scrolling: a scroll region under a fixed header, floods past a screen, reverse index, and the view moving through history.
fn scroll(rows: usize, cols: usize) -> Case {
    let mut steps = Vec::new();
    let mut head = String::from("\x1b[H\x1b[2J");
    let _ = write!(
        head,
        "{}\x1b[44;97m HEADER fixed above the region \x1b[K\x1b[0m",
        cup(0, 0)
    );
    let _ = write!(
        head,
        "{}\x1b[42;30m STATUS fixed below the region \x1b[K\x1b[0m",
        cup(rows - 1, 0)
    );
    let _ = write!(head, "\x1b[3;{}r{}", rows - 1, cup(rows - 2, 0));
    steps.push(feed(head));
    let mut line = 0;
    for _ in 0..60 {
        let mut text = String::new();
        for _ in 0..2 {
            let _ = write!(
                text,
                "\r\nregion line {line} \x1b[33m{}\x1b[0m",
                "=".repeat(line % 40)
            );
            line += 1;
        }
        steps.push(feed(text));
    }
    steps.push(feed(format!("\x1b[r{}", cup(rows - 1, 0))));
    for burst in [rows / 2, rows / 2, rows * 2, rows * 2, rows / 2] {
        for _ in 0..6 {
            let mut text = String::new();
            for _ in 0..burst {
                let _ = writeln!(text, "\rflood {line} {}", "#".repeat(line % 60));
                line += 1;
            }
            steps.push(feed(text));
        }
    }
    for _ in 0..5 {
        steps.push(feed(format!(
            "{}\x1bM\x1bM\x1b[35mreverse index line {line}\x1b[0m",
            cup(0, 0)
        )));
        line += 1;
    }
    steps.push(feed(cup(rows - 1, 0)));
    for lines in [5, 5, 5, 5, 5, 5, -3, -3, -3, -3, 40] {
        steps.push(Step::ViewScroll(lines));
    }
    steps.push(Step::ViewToBottom);
    for _ in 0..10 {
        steps.push(feed(format!("\r\nafter history {line}")));
        line += 1;
    }
    Case {
        name: "scroll".into(),
        about: "a scroll region under a fixed header, floods of half and two screens a frame, reverse index, the view moving back and forth through history",
        rows,
        cols,
        steps,
    }
}

/// A full frame mid-stream (the window asking again after a failed pull) and two size changes.
fn resync_resize(rows: usize, cols: usize) -> Case {
    let mut steps = vec![feed("\x1b[H\x1b[2J")];
    let mut line = 0;
    let mut lines = |count: usize, steps: &mut Vec<Step>, width: usize| {
        for _ in 0..count {
            steps.push(feed(format!(
                "\x1b[32mline {line}\x1b[0m {}\r\n",
                "abcdefghij".repeat((line % 7) + width / 40)
            )));
            line += 1;
        }
    };
    lines(20, &mut steps, cols);
    steps.push(Step::Resync);
    lines(10, &mut steps, cols);
    let (small_rows, small_cols) = (
        rows.saturating_sub(10).max(4),
        cols.saturating_sub(20).max(10),
    );
    steps.push(Step::Resize(small_rows, small_cols));
    lines(10, &mut steps, small_cols);
    steps.push(Step::Resize(rows, cols));
    lines(10, &mut steps, cols);
    Case {
        name: "resync-resize".into(),
        about: "a full frame mid-stream from a resync, a shrink and a grow back",
        rows,
        cols,
        steps,
    }
}

fn builtin(rows: usize, cols: usize) -> Vec<Case> {
    vec![
        sparse_log(rows, cols),
        dense_color(rows, cols),
        wrap(rows, cols),
        hangul(rows, cols),
        emoji(rows, cols),
        cursor(rows, cols),
        scroll(rows, cols),
        resync_resize(rows, cols),
    ]
}

/// A pty recording from elsewhere, fed in fixed-size chunks: one pull after each.
fn from_bytes(name: &str, bytes: &[u8], rows: usize, cols: usize, chunk: usize) -> Case {
    Case {
        name: name.to_string(),
        about: "raw pty bytes from a file, one pull per chunk",
        rows,
        cols,
        steps: bytes
            .chunks(chunk.max(1))
            .map(|part| Step::Feed(part.to_vec()))
            .collect(),
    }
}

/// One reference line: the grid's own screen after frame `frame`.
fn write_screen(out: &mut impl Write, frame: usize, terminal: &Terminal) -> io::Result<()> {
    let screen = terminal.grid().snapshot();
    serde_json::to_writer(&mut *out, &json!({ "frame": frame, "screen": screen }))?;
    out.write_all(b"\n")
}

fn record(case: &Case, out: &Path, every: usize) -> io::Result<serde_json::Value> {
    let path = |suffix: &str| out.join(format!("{}.{suffix}", case.name));
    let mut deltas = BufWriter::new(File::create(path("deltas.jsonl"))?);
    let mut reference = BufWriter::new(File::create(path("reference.jsonl"))?);
    let mut ansi = BufWriter::new(File::create(path("ansi"))?);
    let mut terminal = Terminal::new(case.rows, case.cols);
    let mut made = Vec::new();
    let mut checkpoints = Vec::new();
    let mut bytes = 0usize;
    let mut frame = 0usize;
    for (index, step) in case.steps.iter().enumerate() {
        match step {
            Step::Feed(chunk) => {
                terminal.feed(chunk);
                ansi.write_all(chunk)?;
                bytes += chunk.len();
            }
            Step::ViewScroll(lines) => terminal.grid_mut().scroll_view(*lines),
            Step::ViewToBottom => terminal.grid_mut().view_to_bottom(),
            Step::Resync => terminal.readers_mut().0.declare(READER),
            Step::Resize(rows, cols) => terminal.grid_mut().resize(*rows, *cols),
        }
        let pulled = {
            let (readers, grid) = terminal.readers_mut();
            readers.pull(READER, grid, Instant::now())
        };
        let count = pulled.len();
        for delta in pulled {
            serde_json::to_writer(&mut deltas, &delta)?;
            deltas.write_all(b"\n")?;
            made.push(json!({
                "frame": frame,
                "step": index,
                "kind": step.kind(),
                "full": delta.full,
                "rows": delta.rows.len(),
                "scrolled_lines": delta.scrolled_lines,
                "view_shift": delta.view_shift,
            }));
            frame += 1;
        }
        // The grid's own screen after the frame this step made: at every
        // `every`-th frame and after any step that was not output.
        let special = !matches!(step, Step::Feed(_));
        if count > 0 && ((frame - 1).is_multiple_of(every.max(1)) || special) {
            write_screen(&mut reference, frame - 1, &terminal)?;
            checkpoints.push(frame - 1);
        }
    }
    // And at the end. A step that made no frame changed nothing a reader can
    // see, so the screen now is the screen after the last frame.
    if frame > 0 && checkpoints.last() != Some(&(frame - 1)) {
        write_screen(&mut reference, frame - 1, &terminal)?;
        checkpoints.push(frame - 1);
    }
    deltas.flush()?;
    reference.flush()?;
    ansi.flush()?;
    let meta = json!({
        "format": "zerocode-phase0/1",
        "case": case.name,
        "about": case.about,
        "rows": case.rows,
        "cols": case.cols,
        "frames": frame,
        "steps": case.steps.len(),
        "bytes": bytes,
        "reader": READER,
        "zerocode_pty": TERMINAL_VERSION,
        "checkpoints": checkpoints,
        "made": made,
    });
    fs::write(path("meta.json"), serde_json::to_vec_pretty(&meta)?)?;
    Ok(
        json!({ "case": case.name, "frames": frame, "bytes": bytes, "checkpoints": meta["checkpoints"] }),
    )
}

struct Args {
    command: String,
    out: PathBuf,
    rows: usize,
    cols: usize,
    every: usize,
    names: Vec<String>,
    file: Option<PathBuf>,
    chunk: usize,
    name: Option<String>,
}

const USAGE: &str = "usage:
  phase0-record list
  phase0-record cases [--out DIR] [--rows N] [--cols N] [--every N] [NAME…]
  phase0-record bytes --file PTY_BYTES --name NAME [--out DIR] [--rows N] [--cols N] [--chunk BYTES] [--every N]";

fn parse(mut raw: impl Iterator<Item = String>) -> Result<Args, String> {
    let command = raw.next().ok_or_else(|| USAGE.to_string())?;
    let mut args = Args {
        command,
        out: PathBuf::from("phase0-recordings"),
        rows: ROWS,
        cols: COLS,
        every: 20,
        names: Vec::new(),
        file: None,
        chunk: 4096,
        name: None,
    };
    let number = |value: Option<String>, flag: &str| -> Result<usize, String> {
        value
            .ok_or_else(|| format!("{flag} needs a value"))?
            .parse::<usize>()
            .map_err(|error| format!("{flag}: {error}"))
    };
    while let Some(arg) = raw.next() {
        match arg.as_str() {
            "--out" => args.out = PathBuf::from(raw.next().ok_or("--out needs a value")?),
            "--rows" => args.rows = number(raw.next(), "--rows")?,
            "--cols" => args.cols = number(raw.next(), "--cols")?,
            "--every" => args.every = number(raw.next(), "--every")?,
            "--chunk" => args.chunk = number(raw.next(), "--chunk")?,
            "--file" => args.file = Some(PathBuf::from(raw.next().ok_or("--file needs a value")?)),
            "--name" => args.name = Some(raw.next().ok_or("--name needs a value")?),
            flag if flag.starts_with("--") => return Err(format!("unknown flag {flag}\n{USAGE}")),
            name => args.names.push(name.to_string()),
        }
    }
    if args.rows < 4 || args.cols < 40 {
        return Err("the pane must be at least 4 rows by 40 columns".into());
    }
    Ok(args)
}

fn main() -> Result<(), String> {
    let args = parse(std::env::args().skip(1))?;
    let cases = match args.command.as_str() {
        "list" => {
            for case in builtin(args.rows, args.cols) {
                println!("{}\t{}", case.name, case.about);
            }
            return Ok(());
        }
        "cases" => {
            let all = builtin(args.rows, args.cols);
            if args.names.is_empty() {
                all
            } else {
                let mut picked = Vec::new();
                for name in &args.names {
                    let Some(at) = all.iter().position(|case| &case.name == name) else {
                        return Err(format!(
                            "no case named {name}; `phase0-record list` names them"
                        ));
                    };
                    picked.push(at);
                }
                all.into_iter()
                    .enumerate()
                    .filter(|(at, _)| picked.contains(at))
                    .map(|(_, case)| case)
                    .collect()
            }
        }
        "bytes" => {
            let file = args.file.as_ref().ok_or("bytes needs --file")?;
            let name = args.name.as_deref().ok_or("bytes needs --name")?;
            let bytes = fs::read(file).map_err(|error| format!("{}: {error}", file.display()))?;
            vec![from_bytes(name, &bytes, args.rows, args.cols, args.chunk)]
        }
        other => return Err(format!("unknown command {other}\n{USAGE}")),
    };
    fs::create_dir_all(&args.out).map_err(|error| format!("{}: {error}", args.out.display()))?;
    let mut summary = Vec::new();
    for case in &cases {
        summary.push(
            record(case, &args.out, args.every)
                .map_err(|error| format!("{}: {error}", case.name))?,
        );
    }
    println!("{}", json!({ "out": args.out, "cases": summary }));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_frame_is_the_readers_floor_and_the_rest_are_deltas() {
        let out = std::env::temp_dir().join(format!("phase0-record-test-{}", std::process::id()));
        fs::create_dir_all(&out).unwrap();
        let case = sparse_log(10, 60);
        record(&case, &out, 5).unwrap();
        let text = fs::read_to_string(out.join("sparse-log.deltas.jsonl")).unwrap();
        let frames: Vec<serde_json::Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(frames[0]["full"], true);
        assert!(frames[1..].iter().all(|frame| frame["full"] == false));
        assert!(
            frames
                .iter()
                .any(|frame| frame["scrolled_lines"].as_u64() > Some(0))
        );
        fs::remove_dir_all(&out).unwrap();
    }

    #[test]
    fn a_case_is_the_same_bytes_every_time() {
        let a = builtin(20, 80);
        let b = builtin(20, 80);
        for (left, right) in a.iter().zip(&b) {
            assert_eq!(left.steps.len(), right.steps.len());
            for (one, two) in left.steps.iter().zip(&right.steps) {
                if let (Step::Feed(x), Step::Feed(y)) = (one, two) {
                    assert_eq!(x, y);
                }
            }
        }
    }

    #[test]
    fn a_resync_and_a_resize_each_hand_over_a_full_frame() {
        let out = std::env::temp_dir().join(format!("phase0-record-resync-{}", std::process::id()));
        fs::create_dir_all(&out).unwrap();
        record(&resync_resize(20, 80), &out, 50).unwrap();
        let meta: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join("resync-resize.meta.json")).unwrap())
                .unwrap();
        let fulls: Vec<&str> = meta["made"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|made| made["full"] == true)
            .map(|made| made["kind"].as_str().unwrap())
            .collect();
        assert_eq!(fulls, ["feed", "resync", "resize", "resize"]);
        fs::remove_dir_all(&out).unwrap();
    }
}
