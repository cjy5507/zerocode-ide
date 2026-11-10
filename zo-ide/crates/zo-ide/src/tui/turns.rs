//! A turn's work, folded — the `turns` view of the conversation.
//!
//! The transcript is append-only: a row that reached the terminal's
//! scrollback cannot be taken back. So a turn that should stand as *the
//! person's words · what was done · the answer* has to decide before its rows
//! go in. In the `turns` view the finished tool cells are not committed as
//! they finish. They are kept here — as steps, and as a few live rows the
//! viewport shows above the working row — and one cell says them all when
//! something else has to be said: a text long enough to be an answer starts,
//! a note or the person's next words arrive, or the turn ends.
//!
//! ```text
//! ⏺ Worked for 8s · 7 steps                                  ctrl+o to expand
//!   ⎿ Read 3 files · searched 2 patterns
//!     Edited src/parser.rs  +1 -1
//!     Ran cargo test parser  3.0s
//! ```
//!
//! A short text between two tool calls — the model saying what it is about to
//! do — is work too: it is held ([`HELD_ROWS`]) until the next event says
//! which it was. A tool call after it makes it a step's preamble and it goes
//! with the fold; the turn's end makes it the answer and it is committed
//! under the fold; a text that outgrows the hold is an answer already, and
//! the fold goes in above it. A turn with a long explanation in its middle
//! therefore shows two folds, one on each side of it — nothing is reordered.
//!
//! What a fold leaves out is not lost: the session's transcript keeps every
//! call (`Ctrl+T`), and `ctrl+o` redraws the conversation a row a step.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use super::ansi::{Color, Line, Span, Style};
use super::palette;
use super::strings;

/// How many rows of a text block are held before it is taken for an answer.
pub const HELD_ROWS: usize = 4;
/// How many finished steps the viewport shows above the working row.
pub const LIVE_STEPS: usize = 3;
/// How many edited files and how many passed commands a fold names; the rest
/// are counted.
const NAMED_EDITS: usize = 4;
const NAMED_RUNS: usize = 2;

/// What a step did, as the fold groups it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepKind {
    Read,
    Search,
    List,
    /// An edit, a created file or a deleted one — `verb` says which.
    Edit,
    Run,
    /// Any other call: a helper spawned, an MCP tool, a plan update.
    Other,
}

/// One finished call.
#[derive(Debug, Clone)]
pub struct Step {
    pub kind: StepKind,
    /// The row's word for it: `Read`, `Edit`, `Run`, a tool's name.
    pub verb: String,
    /// What it was done to: a path, a pattern, a command's first line.
    pub target: String,
    /// Lines added and removed, for an edit that went through.
    pub change: Option<(usize, usize)>,
    pub ok: bool,
    pub declined: bool,
    /// `exit 101`, `failed`, `declined` — a failed step's own word.
    pub failure: Option<String>,
    pub started: Instant,
    pub took: Option<Duration>,
}

/// The work held since the last fold was committed.
#[derive(Debug, Default)]
pub struct TurnFold {
    steps: Vec<Step>,
    /// A short text block's rows, not in history yet.
    held: Vec<Line>,
    /// The open text block outgrew the hold: its rows go straight to history.
    passing: bool,
    /// The newest finished steps, for the viewport.
    live: VecDeque<Step>,
}

impl TurnFold {
    /// Whether anything waits to be said.
    #[must_use]
    pub fn is_pending(&self) -> bool {
        !self.steps.is_empty() || !self.held.is_empty()
    }

    #[must_use]
    pub fn has_steps(&self) -> bool {
        !self.steps.is_empty()
    }

    /// Finished calls join the fold.
    pub fn take_steps(&mut self, steps: Vec<Step>) {
        for step in steps {
            if self.live.len() == LIVE_STEPS {
                self.live.pop_front();
            }
            self.live.push_back(step.clone());
            self.steps.push(step);
        }
    }

    /// Rows of the open text block. `Some` gives back what must go to history
    /// now — the block outgrew the hold, so it is an answer — and the caller
    /// commits the fold above it first.
    pub fn take_text(&mut self, rows: Vec<Line>) -> Option<Vec<Line>> {
        if self.passing {
            return Some(rows);
        }
        self.held.extend(rows);
        let said = self.held.iter().filter(|row| !row.spans.is_empty()).count();
        if said > HELD_ROWS {
            self.passing = true;
            return Some(std::mem::take(&mut self.held));
        }
        None
    }

    /// The open text block closed.
    pub fn text_closed(&mut self) {
        self.passing = false;
    }

    /// A tool call follows the held text: it was the step's preamble, and
    /// goes with the fold.
    pub fn drop_preamble(&mut self) {
        self.held.clear();
    }

    /// The held text, for the viewport while it waits.
    #[must_use]
    pub fn held(&self) -> &[Line] {
        &self.held
    }

    /// The fold's cell and the held text, in the order they are said; the
    /// fold is empty afterwards. `now` closes the fold's clock.
    pub fn flush(&mut self, width: usize, now: Instant) -> (Vec<Line>, Vec<Line>) {
        let cell = cell(&self.steps, width, now);
        self.steps.clear();
        self.live.clear();
        (cell, std::mem::take(&mut self.held))
    }

    /// Forget everything — the conversation was swapped or redrawn.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// How many steps the fold holds: the working row counts them
    /// ([`steps_word`]) with the calls still live.
    #[must_use]
    pub fn step_count(&self) -> usize {
        self.steps.len()
    }

    /// The viewport's rows for the work so far: the newest finished steps,
    /// dim, hung under the look's mark. How many there are in all is the
    /// working row's to say.
    #[must_use]
    pub fn live_rows(&self, width: usize) -> Vec<Line> {
        let glyphs = super::style::glyphs();
        let mut rows = Vec::with_capacity(self.live.len());
        for (index, step) in self.live.iter().enumerate() {
            let lead = if index == 0 { glyphs.result_first } else { glyphs.result_rest };
            let mut spans = vec![Span::dim(lead), Span::dim(format!("{} {}", step.verb, step.target))];
            spans.extend(step_tail(step, true));
            rows.push(Line::new(spans).truncated(width));
        }
        rows
    }
}

/// `1 step`, `7 steps` — a fold's count, in its cell and on the working row.
#[must_use]
pub fn steps_word(count: usize) -> String {
    counted(count, "step", "steps")
}

/// What follows a step's target on its row: an edit's counts, a command's
/// seconds, a failure's word.
fn step_tail(step: &Step, quiet: bool) -> Vec<Span> {
    let tint = |color: Color| if quiet { Style::new().fg(color).dim() } else { Style::new().fg(color) };
    if !step.ok {
        let word = step.failure.clone().unwrap_or_else(|| strings::TOOL_FAILED_RESULT.to_string());
        let style = if step.declined { Style::new().dim() } else { tint(palette::TOOL_FAIL) };
        return vec![Span::raw("  "), Span::new(word, style)];
    }
    if let Some((added, removed)) = step.change {
        return vec![
            Span::raw("  "),
            Span::new(format!("+{added}"), tint(Color::GREEN)),
            Span::raw(" "),
            Span::new(format!("-{removed}"), tint(Color::RED)),
        ];
    }
    match step.took.filter(|took| step.kind == StepKind::Run && took.as_secs() > 0) {
        Some(took) => vec![Span::dim(format!("  {:.1}s", took.as_secs_f32()))],
        None => Vec::new(),
    }
}

/// `1 file`, `3 files`.
fn counted(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// The committed cell: the gap every cell opens with, the head row, and what
/// the steps came to. Empty when there were no steps.
#[must_use]
pub fn cell(steps: &[Step], width: usize, now: Instant) -> Vec<Line> {
    if steps.is_empty() {
        return Vec::new();
    }
    let width = super::style::current().reading_width(width);
    let glyphs = super::style::glyphs();
    let mut body: Vec<Vec<Span>> = Vec::new();
    body.extend(looked_row(steps));
    body.extend(edit_rows(steps));
    body.extend(run_rows(steps));
    body.extend(other_row(steps));
    // What did not go through is never folded away.
    for step in steps.iter().filter(|step| !step.ok) {
        let mut row = vec![Span::dim(format!("{} ", step.verb)), Span::raw(step.target.clone())];
        row.extend(step_tail(step, false));
        body.push(row);
    }

    let mut out = vec![Line::empty(), Line::new(head_row(steps, width, now)).truncated(width)];
    for (index, mut row) in body.into_iter().enumerate() {
        let lead = if index == 0 { glyphs.result_first } else { glyphs.result_rest };
        row.insert(0, Span::dim(lead));
        out.push(Line::new(row).truncated(width));
    }
    out
}

/// The cell's first row: how long the steps took and how many there were,
/// and, where it fits, the key that shows them all.
fn head_row(steps: &[Step], width: usize, now: Instant) -> Vec<Span> {
    let failed = steps.iter().any(|step| !step.ok && !step.declined);
    let glyph_style = Style::new().fg(if failed { palette::TOOL_FAIL } else { palette::TOOL_OK }).bold();
    let seconds = steps
        .iter()
        .map(|step| step.started)
        .min()
        .map_or(0, |began| now.saturating_duration_since(began).as_secs());
    let worked = if seconds > 0 {
        format!("{} {}", strings::WORKED_FOR, super::shimmer::fmt_elapsed_compact(seconds))
    } else {
        strings::WORKED.to_string()
    };
    let mut head = vec![
        Span::new(super::style::glyphs().tool_done.to_string(), glyph_style),
        Span::raw(" "),
        Span::bold(worked),
        Span::dim(format!(" · {}", steps_word(steps.len()))),
    ];
    let hint = [Span::bold(strings::KEY_VIEW), Span::dim(strings::TO_EXPAND)];
    let used: usize = head.iter().map(Span::width).sum();
    let hint_width: usize = hint.iter().map(Span::width).sum();
    if used + 2 + hint_width <= width {
        head.push(Span::raw(" ".repeat(width - used - hint_width)));
        head.extend(hint);
    }
    head
}

/// Looking around, in one row: `Read 3 files · searched 2 patterns`.
fn looked_row(steps: &[Step]) -> Option<Vec<Span>> {
    let count = |kind: StepKind| steps.iter().filter(|step| step.ok && step.kind == kind).count();
    let mut looked = Vec::new();
    for (kind, word, one, many) in [
        (StepKind::Read, "read", "file", "files"),
        (StepKind::Search, "searched", "pattern", "patterns"),
        (StepKind::List, "listed", "folder", "folders"),
    ] {
        let seen = count(kind);
        if seen > 0 {
            looked.push(format!("{word} {}", counted(seen, one, many)));
        }
    }
    if looked.is_empty() {
        return None;
    }
    let mut text = looked.join(" · ");
    if let Some(first) = text.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    Some(vec![Span::dim(text)])
}

/// What changed, by file, in the order the files were first touched; the
/// files past [`NAMED_EDITS`] are counted in one row.
fn edit_rows(steps: &[Step]) -> Vec<Vec<Span>> {
    let mut edits: Vec<(String, &str, usize, usize)> = Vec::new();
    for step in steps.iter().filter(|step| step.ok && step.kind == StepKind::Edit) {
        let (added, removed) = step.change.unwrap_or_default();
        match edits.iter_mut().find(|(path, ..)| *path == step.target) {
            Some(edit) => {
                edit.2 += added;
                edit.3 += removed;
            }
            None => edits.push((step.target.clone(), past_tense(&step.verb), added, removed)),
        }
    }
    let mut rows: Vec<Vec<Span>> = edits
        .iter()
        .take(NAMED_EDITS)
        .map(|(path, verb, added, removed)| {
            vec![
                Span::dim(format!("{verb} ")),
                Span::raw(path.clone()),
                Span::raw("  "),
                Span::new(format!("+{added}"), Style::new().fg(Color::GREEN)),
                Span::raw(" "),
                Span::new(format!("-{removed}"), Style::new().fg(Color::RED)),
            ]
        })
        .collect();
    if edits.len() > NAMED_EDITS {
        let rest = &edits[NAMED_EDITS..];
        let (added, removed) = rest.iter().fold((0, 0), |sum, edit| (sum.0 + edit.2, sum.1 + edit.3));
        rows.push(vec![Span::dim(format!(
            "… +{} +{added} -{removed}",
            counted(rest.len(), "more file", "more files")
        ))]);
    }
    rows
}

/// What ran: the newest [`NAMED_RUNS`] that passed, by name, under a count
/// of the ones before them.
fn run_rows(steps: &[Step]) -> Vec<Vec<Span>> {
    let runs: Vec<&Step> = steps.iter().filter(|step| step.ok && step.kind == StepKind::Run).collect();
    let mut rows = Vec::new();
    if runs.len() > NAMED_RUNS {
        rows.push(vec![Span::dim(format!(
            "… +{}",
            counted(runs.len() - NAMED_RUNS, "earlier command", "earlier commands")
        ))]);
    }
    for step in runs.iter().skip(runs.len().saturating_sub(NAMED_RUNS)) {
        let mut row = vec![Span::dim(format!("{} ", strings::RAN)), Span::raw(step.target.clone())];
        row.extend(step_tail(step, false));
        rows.push(row);
    }
    rows
}

/// Everything else that went through, counted by its word.
fn other_row(steps: &[Step]) -> Option<Vec<Span>> {
    let mut others: Vec<(&str, usize)> = Vec::new();
    for step in steps.iter().filter(|step| step.ok && step.kind == StepKind::Other) {
        match others.iter_mut().find(|(verb, _)| *verb == step.verb) {
            Some(other) => other.1 += 1,
            None => others.push((step.verb.as_str(), 1)),
        }
    }
    if others.is_empty() {
        return None;
    }
    let text = others
        .iter()
        .map(|(verb, count)| if *count == 1 { (*verb).to_string() } else { format!("{verb} ×{count}") })
        .collect::<Vec<_>>()
        .join(" · ");
    Some(vec![Span::dim(text)])
}

/// A row's verb as the fold says it happened: `Edit` → `Edited`.
fn past_tense(verb: &str) -> &'static str {
    match verb {
        strings::TOOL_CREATE => strings::CREATED,
        strings::TOOL_DELETE => strings::DELETED,
        _ => strings::EDITED,
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{cell, steps_word, Step, StepKind, TurnFold, HELD_ROWS, LIVE_STEPS};
    use crate::tui::ansi::Line;

    fn step(kind: StepKind, verb: &str, target: &str) -> Step {
        Step {
            kind,
            verb: verb.to_string(),
            target: target.to_string(),
            change: None,
            ok: true,
            declined: false,
            failure: None,
            started: Instant::now(),
            took: None,
        }
    }

    fn plain(rows: &[Line]) -> Vec<String> {
        rows.iter().map(Line::plain).collect()
    }

    #[test]
    fn a_fold_says_what_was_read_changed_and_run() {
        let began = Instant::now();
        let mut steps = vec![
            step(StepKind::Read, "Read", "src/lib.rs"),
            step(StepKind::Read, "Read", "src/parser.rs"),
            step(StepKind::Search, "Search", "break|continue in src"),
            Step { change: Some((1, 1)), ..step(StepKind::Edit, "Edit", "src/parser.rs") },
            Step { took: Some(Duration::from_millis(3_040)), ..step(StepKind::Run, "Run", "cargo test parser") },
        ];
        for step in &mut steps {
            step.started = began;
        }
        let rows = plain(&cell(&steps, 100, began + Duration::from_secs(8)));
        assert_eq!(rows[0], "", "the gap every cell opens with");
        assert!(rows[1].starts_with("✓ Worked for 8s · 5 steps"), "{}", rows[1]);
        assert!(rows[1].ends_with("ctrl+o to expand"), "{}", rows[1]);
        assert_eq!(rows[2], "  └ Read 2 files · searched 1 pattern");
        assert_eq!(rows[3], "    Edited src/parser.rs  +1 -1");
        assert_eq!(rows[4], "    Ran cargo test parser  3.0s");
        assert_eq!(rows.len(), 5);
    }

    #[test]
    fn a_step_that_failed_is_named_and_a_fold_with_no_clock_says_no_seconds() {
        let now = Instant::now();
        let steps = vec![
            Step { started: now, ..step(StepKind::Read, "Read", "a.rs") },
            Step {
                started: now,
                ok: false,
                failure: Some("exit 101".to_string()),
                ..step(StepKind::Run, "Run", "cargo test")
            },
        ];
        let rows = plain(&cell(&steps, 80, now));
        assert!(rows[1].starts_with("✓ Worked · 2 steps"), "a replayed fold has no seconds: {}", rows[1]);
        assert_eq!(rows[2], "  └ Read 1 file");
        assert_eq!(rows[3], "    Run cargo test  exit 101");
        assert!(cell(&[], 80, now).is_empty(), "no steps, no cell");
    }

    #[test]
    fn edits_of_one_file_add_up_and_the_files_past_the_fourth_are_counted() {
        let now = Instant::now();
        let mut steps: Vec<Step> = (0..6)
            .map(|n| Step { change: Some((2, 1)), ..step(StepKind::Edit, "Edit", &format!("src/f{n}.rs")) })
            .collect();
        steps.push(Step { change: Some((5, 0)), ..step(StepKind::Edit, "Edit", "src/f0.rs") });
        let rows = plain(&cell(&steps, 80, now));
        assert_eq!(rows[2], "  └ Edited src/f0.rs  +7 -1");
        assert_eq!(rows[5], "    Edited src/f3.rs  +2 -1");
        assert_eq!(rows[6], "    … +2 more files +4 -2");
    }

    #[test]
    fn a_short_text_is_held_and_a_long_one_passes() {
        let mut fold = TurnFold::default();
        let rows = |count: usize| (0..count).map(|n| Line::from_text(format!("row {n}"))).collect::<Vec<_>>();
        assert!(fold.take_text(rows(HELD_ROWS)).is_none(), "short enough to be a preamble");
        assert_eq!(fold.held().len(), HELD_ROWS);
        fold.text_closed();
        fold.drop_preamble();
        assert!(!fold.is_pending(), "a tool call after it: the text went with the fold");

        assert!(fold.take_text(rows(2)).is_none());
        let out = fold.take_text(rows(HELD_ROWS)).expect("it outgrew the hold");
        assert_eq!(out.len(), HELD_ROWS + 2, "everything held comes out, in order");
        assert_eq!(fold.take_text(rows(1)).map(|rows| rows.len()), Some(1), "and the rest passes");
        fold.text_closed();
        assert!(fold.take_text(rows(1)).is_none(), "the next block is held again");
    }

    #[test]
    fn the_viewport_shows_the_newest_steps_and_counts_them_all() {
        let mut fold = TurnFold::default();
        assert!(fold.live_rows(80).is_empty() && fold.step_count() == 0);
        fold.take_steps((0..5).map(|n| step(StepKind::Read, "Read", &format!("f{n}.rs"))).collect());
        let rows = plain(&fold.live_rows(80));
        assert_eq!(rows.len(), LIVE_STEPS);
        assert_eq!(rows[0], "  └ Read f2.rs");
        assert_eq!(rows[2], "    Read f4.rs");
        assert_eq!(fold.step_count(), 5);
        assert_eq!((steps_word(1), steps_word(5)), ("1 step".to_string(), "5 steps".to_string()));
        let (cell, held) = fold.flush(80, Instant::now());
        assert!(cell[1].plain().contains("5 steps") && held.is_empty());
        assert!(fold.live_rows(80).is_empty() && !fold.is_pending());
    }
}
