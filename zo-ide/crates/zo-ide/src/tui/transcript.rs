//! Ctrl+T — the session transcript, unfolded.
//!
//! Native scrollback already carries what the screen showed. This carries what
//! it did **not**: a tool cell commits folded (its body lives behind an OSC 7788
//! marker, collapsed), and compaction evicts whole turns out of the window
//! entirely. Neither is reachable by scrolling the terminal. `session_recall`
//! is the model's door back to those originals; this is the person's.
//!
//! Codex binds its transcript overlay to Ctrl+T and keeps it available during a
//! task (`is_open_key`); a long autonomous run is exactly when someone wants
//! to read what a tool actually printed twenty minutes ago without stopping the
//! turn.
//!
//! The source is [`ReplayItem`] — the same vocabulary `/resume` replays a
//! previous conversation from, so the overlay and the replay can never disagree
//! about what the session contained.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use runtime::message_stream::AgentResultStatus;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::ansi::{Line, Span, Style};
use super::cells::{prefixed, Prefix};
use super::markdown;
use super::palette;
use super::wrap::wrap_line;
use crate::session::plain_session::ReplayItem;

/// Rows the overlay spends on chrome — two blanks, title, note, blank, blank,
/// footer. Mirrors `Picker`'s budget so both overlays sit the same on screen.
const CHROME: usize = 7;

/// What a key did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// The overlay consumed it.
    Handled,
    /// Close and give the key back to nobody.
    Close,
}

/// One item in the live Ctrl+T transcript store.
#[derive(Debug, Clone)]
pub(crate) enum Entry {
    Replay(ReplayItem),
    Reasoning(String),
    Diff(runtime::message_stream::DiffView),
}

/// The transcript overlay: a pager over pre-rendered lines.
#[derive(Debug)]
pub(crate) struct Transcript {
    title: &'static str,
    /// Source items retained so a terminal resize can rebuild every wrap.
    items: Arc<Vec<Entry>>,
    recent: Arc<Vec<Entry>>,
    /// Every line of the session, already wrapped to the width it was built at.
    lines: Vec<Line>,
    /// Width used to build `lines`.
    width: usize,
    /// Index of the first visible line.
    offset: usize,
    /// Rows the last render could show — pages move by this, and it is only
    /// known at render time, so it is remembered rather than guessed.
    page: usize,
    /// Search the unwrapped source so a narrow terminal cannot hide a match.
    query: String,
    finding: bool,
    matches: Vec<usize>,
    selected_match: Option<usize>,
    entry_lines: Vec<usize>,
    turn_lines: Vec<usize>,
    source: Option<PathBuf>,
    source_index: Option<Arc<super::transcript_source::Index>>,
    source_job: Option<super::transcript_source::Job>,
    source_due: Option<Instant>,
    source_at: Option<usize>,
    source_next: usize,
    source_match: Option<usize>,
    source_note: Option<String>,
    pending_turn: Option<bool>,
}

impl Transcript {
    /// Build from the session's replay items at `width` columns.
    #[cfg(test)]
    pub(crate) fn new(items: &[ReplayItem], width: usize) -> Self {
        let items = items.iter().cloned().map(Entry::Replay).collect::<Vec<_>>();
        Self::from_entries(&items, width)
    }

    /// Build from the TUI's live transcript store, including reasoning-only
    /// entries that are not replayable assistant turns.
    pub(crate) fn from_entries(items: &[Entry], width: usize) -> Self {
        let width = width.max(8);
        let items = Arc::new(items.to_vec());
        let (lines, entry_lines, turn_lines) = render_items(&items, width);
        let mut transcript = Self {
            title: "Transcript",
            recent: Arc::clone(&items),
            items,
            lines,
            width,
            offset: 0,
            page: 1,
            query: String::new(),
            finding: false,
            matches: Vec::new(),
            selected_match: None,
            entry_lines,
            turn_lines,
            source: None,
            source_index: None,
            source_job: None,
            source_due: None,
            source_at: None,
            source_next: 0,
            source_match: None,
            source_note: None,
            pending_turn: None,
        };
        // Open at the end. The interesting part of a long run is what just
        // happened, and codex's transcript opens on the tail for the same
        // reason.
        transcript.offset = usize::MAX;
        transcript
    }

    pub(crate) fn with_title(mut self, title: &'static str) -> Self { self.title = title; self }

    pub(crate) fn find(&mut self, query: &str) {
        self.query = query.to_string();
        self.finding = query.is_empty();
        self.find_matches();
    }

    pub(crate) fn paste(&mut self, text: &str) {
        if self.finding { self.query.push_str(text); self.find_matches(); }
    }

    pub(crate) fn with_source(mut self, path: Option<PathBuf>) -> Self {
        if let Some(path) = &path {
            self.source_job = Some(super::transcript_source::Job::scan(path.clone(), String::new()));
        }
        self.source = path;
        self
    }

    pub(crate) fn needs_poll(&self) -> bool {
        self.source_job.is_some() || self.source_due.is_some()
    }

    fn load_source_page(&mut self, start: usize) {
        if let Some(index) = &self.source_index {
            self.source_job = Some(super::transcript_source::Job::page(Arc::clone(index), start, self.query.clone()));
            self.source_note = None;
        }
    }

    fn poll_source(&mut self) {
        use super::transcript_source::{Job, Reply};
        if self.source_due.is_some_and(|due| Instant::now() >= due) {
            self.source_due = None;
            if let Some(path) = &self.source {
                self.source_job = Some(Job::scan(path.clone(), self.query.clone()));
            }
        }
        let Some(result) = self.source_job.as_ref().and_then(Job::poll) else { return; };
        self.source_job = None;
        match result {
            Err(error) => self.source_note = Some(error),
            Ok(Reply::Index(index)) => {
                let last = index.matches.len().checked_sub(1);
                self.source_index = Some(Arc::clone(&index));
                if let Some(previous) = self.pending_turn.take() {
                    self.next_turn(previous);
                } else if let Some(at) = last {
                    self.source_match = Some(at);
                    self.load_source_page(index.matches[at]);
                }
            }
            Ok(Reply::Page { entries, start, next }) => {
                self.items = Arc::new(entries);
                self.source_at = Some(start);
                self.source_next = next;
                (self.lines, self.entry_lines, self.turn_lines) = render_items(&self.items, self.width);
                self.offset = 0;
                self.find_local_matches();
            }
        }
    }

    fn scroll_to(&mut self, target: usize, visible: usize) {
        self.offset = target.min(self.lines.len().saturating_sub(visible));
    }

    pub(crate) fn key(&mut self, key: KeyEvent) -> Outcome {
        let page = self.page.max(1);
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        if is_open_key(&key) {
            return Outcome::Close;
        }
        if control && matches!(key.code, KeyCode::Char('f' | 'F')) {
            self.finding = true;
            return Outcome::Handled;
        }
        if self.finding {
            match key.code {
                KeyCode::Esc | KeyCode::Enter => self.finding = false,
                KeyCode::Backspace => {
                    self.query.pop();
                    self.find_matches();
                }
                KeyCode::Char(ch) if !control && !key.modifiers.contains(KeyModifiers::ALT) => {
                    self.query.push(ch);
                    self.find_matches();
                }
                _ => {}
            }
            return Outcome::Handled;
        }
        match key.code {
            // Three ways out, all of them codex's: the binding that opened
            // it, the universal Esc, and the pager `q`.
            KeyCode::Esc | KeyCode::Char('q') => return Outcome::Close,
            KeyCode::Char('t' | 'T') if control => return Outcome::Close,
            KeyCode::Up | KeyCode::Char('k') => {
                self.offset = self.offset.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let next = self.offset.saturating_add(1);
                self.scroll_to(next, page);
            }
            KeyCode::PageUp | KeyCode::Char('b') => {
                if self.offset == 0 && self.source_at.is_some_and(|at| at > 0) {
                    self.load_source_page(self.source_at.unwrap_or(0).saturating_sub(32));
                } else { self.offset = self.offset.saturating_sub(page); }
            }
            KeyCode::PageDown | KeyCode::Char(' ' | 'f') => {
                if self.source_at.is_some() && self.offset >= self.lines.len().saturating_sub(page)
                    && self.source_index.as_ref().is_some_and(|index| self.source_next < index.len()) {
                    self.load_source_page(self.source_next);
                } else {
                    let next = self.offset.saturating_add(page);
                    self.scroll_to(next, page);
                }
            }
            KeyCode::Home | KeyCode::Char('g') => self.offset = 0,
            KeyCode::End | KeyCode::Char('G') => self.scroll_to(usize::MAX, page),
            KeyCode::Char('/') => self.finding = true,
            KeyCode::Char('n') => self.next_match(false),
            KeyCode::Char('N') => self.next_match(true),
            KeyCode::Char('[' | '{') => self.next_turn(true),
            KeyCode::Char(']' | '}') => self.next_turn(false),
            _ => {}
        }
        Outcome::Handled
    }

    fn find_matches(&mut self) {
        self.source_job = None;
        self.source_index = None;
        self.source_at = None;
        self.source_match = None;
        self.source_note = None;
        if !Arc::ptr_eq(&self.items, &self.recent) {
            self.items = Arc::clone(&self.recent);
            (self.lines, self.entry_lines, self.turn_lines) = render_items(&self.items, self.width);
        }
        if self.source.is_some() {
            self.source_due = Some(Instant::now() + Duration::from_millis(150));
        }
        self.find_local_matches();
    }

    fn find_local_matches(&mut self) {
        let needle = self.query.to_lowercase();
        self.matches = if needle.is_empty() {
            Vec::new()
        } else {
            self.items.iter().enumerate().filter_map(|(index, item)| {
                entry_matches(item, &needle).then_some(index)
            }).collect()
        };
        self.selected_match = (!self.matches.is_empty()).then_some(0);
        self.reveal_match();
    }

    fn reveal_match(&mut self) {
        let Some(index) = self.selected_match.and_then(|at| self.matches.get(at)).copied() else { return; };
        let from = self.entry_lines[index];
        let to = self.entry_lines.get(index + 1).copied().unwrap_or(self.lines.len());
        let needle = self.query.to_lowercase();
        let line = (from..to).find(|&at| self.lines[at].plain().to_lowercase().contains(&needle)).unwrap_or(from);
        self.scroll_to(line, self.page);
    }

    fn next_match(&mut self, previous: bool) {
        if let Some(index) = &self.source_index {
            if !index.matches.is_empty() {
                let at = self.source_match.unwrap_or(0);
                let next = if previous { (at + index.matches.len() - 1) % index.matches.len() } else { (at + 1) % index.matches.len() };
                let record = index.matches[next];
                self.source_match = Some(next);
                self.load_source_page(record);
                return;
            }
        }
        if self.matches.is_empty() { return; }
        let at = self.selected_match.unwrap_or(0);
        self.selected_match = Some(if previous {
            (at + self.matches.len() - 1) % self.matches.len()
        } else {
            (at + 1) % self.matches.len()
        });
        self.reveal_match();
    }

    fn next_turn(&mut self, previous: bool) {
        if let Some(index) = &self.source_index {
            let from = self.source_at.unwrap_or(index.len());
            let found = if previous { index.turns.iter().rev().find(|&&at| at < from) } else { index.turns.iter().find(|&&at| at > from) };
            if let Some(&at) = found { self.load_source_page(at); }
            return;
        }
        if self.source_job.is_some() { self.pending_turn = Some(previous); }
        let at = self.offset.min(self.lines.len().saturating_sub(self.page));
        let target = if previous {
            self.turn_lines.iter().rev().find(|&&line| line < at)
        } else {
            self.turn_lines.iter().find(|&&line| line > at)
        };
        if let Some(&line) = target { self.scroll_to(line, self.page); }
    }

    /// Render into at most `max_rows` rows.
    ///
    /// Takes `&mut self` because the window size is what a page key moves by
    /// and only the renderer knows it — remembering it here is what keeps
    /// `PageDown` honest after a resize.
    pub(crate) fn lines(&mut self, width: usize, max_rows: usize) -> Vec<Line> {
        self.poll_source();
        self.rewrap(width);
        let visible = max_rows.saturating_sub(CHROME).max(1);
        self.page = visible;
        // Clamp now: the offset may have been parked past the end by `new`
        // (open at the tail) or by a resize that grew the window.
        self.offset = self.offset.min(self.lines.len().saturating_sub(visible));

        let mut out = vec![Line::empty(), Line::empty()];
        out.push(
            Line::new(vec![
                Span::raw("  "),
                Span::new(
                    self.title.to_string(),
                    Style::new().bold().fg(palette::COMMAND_TOKEN),
                ),
            ])
            .truncated(width),
        );
        let last = (self.offset + visible).min(self.lines.len());
        out.push(
            Line::new(vec![
                Span::raw("  "),
                Span::dim(if let Some(note) = &self.source_note {
                    note.clone()
                } else if self.needs_poll() {
                    format!("/{} · reading saved history…", self.query)
                } else if let Some(index) = &self.source_index {
                    if self.finding || !self.query.is_empty() {
                        format!("/{}{} · {}/{} saved matches{}", self.query, if self.finding { "▏" } else { "" },
                            self.source_match.map_or(0, |at| at + 1), index.matches.len(), if index.limited { " · some records skipped" } else { "" })
                    } else {
                        format!("Saved history · {} turns · [/] navigate{}", index.turns.len(), if index.limited { " · some records skipped" } else { "" })
                    }
                } else if self.finding || !self.query.is_empty() {
                    format!("/{}{} · {}/{} matches", self.query, if self.finding { "▏" } else { "" },
                        self.selected_match.map_or(0, |at| at + 1), self.matches.len())
                } else { format!(
                    "{}-{} of {} lines · tool output is unfolded here",
                    self.offset + 1,
                    last,
                    self.lines.len()
                ) }),
            ])
            .truncated(width),
        );
        out.push(Line::empty());
        for line in self.lines.iter().skip(self.offset).take(visible) {
            out.push(line.clone().truncated(width));
        }
        // Pad so the footer sits at the bottom even on a short transcript —
        // a footer that floats up mid-screen reads as a torn frame.
        for _ in out.len()..(max_rows.saturating_sub(2)) {
            out.push(Line::empty());
        }
        out.push(Line::empty());
        out.push(
            Line::new(vec![
                Span::raw("  "),
                Span::dim(if self.finding {
                    "type to find · enter/esc finish · ctrl+t close"
                } else {
                    "/ find · n/N matches · [/] turns · ↑↓ scroll · ctrl+t/esc close"
                }),
            ])
            .truncated(width),
        );
        out
    }

    fn rewrap(&mut self, width: usize) {
        let width = width.max(8);
        if width == self.width {
            return;
        }
        let was_at_tail = self.offset == usize::MAX
            || self.offset >= self.lines.len().saturating_sub(self.page.max(1));
        (self.lines, self.entry_lines, self.turn_lines) = render_items(&self.items, width);
        self.width = width;
        if self.selected_match.is_some() {
            self.reveal_match();
        } else if was_at_tail {
            self.offset = usize::MAX;
        }
    }
}

/// Codex's fixed transcript binding (`ctrl+t`), available during a task.
pub(crate) fn is_open_key(key: &KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL)
        && !key.modifiers.contains(KeyModifiers::ALT)
        && matches!(key.code, KeyCode::Char(ch) if ch.eq_ignore_ascii_case(&'t'))
}

/// One replay item to wrapped lines, marked the way the live cell marks it so
/// the overlay reads as the same session and not as a second UI.
fn render_item(item: &Entry, width: usize, out: &mut Vec<Line>) {
    match item {
        Entry::Diff(view) => {
            out.push(Line::new(vec![Span::bold(view.new_path.as_deref().or(view.old_path.as_deref()).unwrap_or("Recorded change"))]).truncated(width));
            out.extend(super::diff::hunk_lines(view, width));
        }
        Entry::Replay(ReplayItem::User(text)) => {
            let start = out.len();
            push_body(
                out,
                Span::new("› ", palette::user_marker()),
                text,
                width,
                Style::new(),
            );
            let style = super::cells::user_message_style();
            for line in &mut out[start..] {
                line.style = style;
            }
        }
        Entry::Replay(ReplayItem::Assistant(text)) => {
            push_body(
                out,
                Span::new("• ", palette::cell_marker()),
                text,
                width,
                Style::new(),
            );
        }
        Entry::Replay(ReplayItem::AgentResult {
            label,
            status,
            summary,
            body,
        }) => {
            let head = super::tools::agent_result_header(
                label,
                matches!(status, AgentResultStatus::Completed),
                summary.as_deref(),
            );
            out.extend(wrap_line(&head, width, &Span::raw("  ")));
            let body = super::tools::strip_agent_result_harness(body);
            if !body.trim().is_empty() {
                // Ctrl+T is the unfolded transcript, so keep the complete
                // report here; only the compact history card spends a tail
                // budget. The host-only wrappers stay hidden in both views.
                push_body(out, Span::dim("  └ "), body, width, Style::new());
            }
        }
        Entry::Reasoning(text) => {
            let style = Style::new().dim().italic();
            let lines = markdown::render(text)
                .into_iter()
                .map(|line| line.patched(style))
                .collect::<Vec<_>>();
            out.extend(prefixed(&lines, width, &Prefix::bullet(), true));
        }
        Entry::Replay(ReplayItem::ToolCall {
            name,
            input,
            output,
            is_error,
        }) => {
            let head = Line::new(vec![
                Span::new("• ", palette::cell_marker()),
                Span::new(name.clone(), Style::new().bold()),
                Span::dim(format!(" {}", one_line(input))),
            ]);
            out.extend(wrap_line(&head, width, &Span::raw("  ")));
            // The reason this overlay exists: the committed cell folds this
            // body away, so print it whole.
            if let Some(body) = output {
                let marker = if *is_error { "  └ error " } else { "  └ " };
                push_body(out, Span::dim(marker), body, width, Style::new().dim());
            }
        }
    }
    out.push(Line::empty());
}

fn entry_matches(item: &Entry, needle: &str) -> bool {
    let has = |text: &str| text.to_lowercase().contains(needle);
    match item {
        Entry::Diff(view) => view.old_path.as_deref().is_some_and(has) || view.new_path.as_deref().is_some_and(has)
            || view.hunks.iter().flat_map(|hunk| &hunk.lines).any(|line| has(&line.text)),
        Entry::Reasoning(text) | Entry::Replay(ReplayItem::User(text) | ReplayItem::Assistant(text)) => has(text),
        Entry::Replay(ReplayItem::ToolCall { name, input, output, .. }) => has(name) || has(input) || output.as_deref().is_some_and(has),
        Entry::Replay(ReplayItem::AgentResult { label, summary, body, .. }) => has(label) || summary.as_deref().is_some_and(has) || has(body),
    }
}

fn render_items(items: &[Entry], width: usize) -> (Vec<Line>, Vec<usize>, Vec<usize>) {
    let mut lines = Vec::new();
    let mut entries = Vec::with_capacity(items.len());
    let mut turns = Vec::new();
    for item in items {
        entries.push(lines.len());
        if matches!(item, Entry::Replay(ReplayItem::User(_))) { turns.push(lines.len()); }
        render_item(item, width, &mut lines);
    }
    if lines.is_empty() {
        lines.push(Line::new(vec![Span::dim("This session has no turns yet.")]));
    }
    (lines, entries, turns)
}

/// A block of text under one marker: the marker leads the first line, the rest
/// hang under it.
fn push_body(out: &mut Vec<Line>, marker: Span, text: &str, width: usize, style: Style) {
    let indent = Span::raw(" ".repeat(marker.text.chars().count()));
    let mut first = true;
    for source in text.lines() {
        let line = if first {
            first = false;
            Line::new(vec![marker.clone(), Span::new(source.to_string(), style)])
        } else {
            Line::new(vec![
                indent.clone(),
                Span::new(source.to_string(), style),
            ])
        };
        out.extend(wrap_line(&line, width, &indent));
    }
    if first {
        // Empty body — still show the marker, or a tool that printed nothing
        // silently disappears from the transcript.
        out.push(Line::new(vec![marker]));
    }
}

/// Collapse a tool's argument JSON to one readable line.
fn one_line(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= 60 {
        return flat;
    }
    let mut cut: String = flat.chars().take(59).collect();
    cut.push('…');
    cut
}

#[cfg(test)]
mod tests {
    use super::{is_open_key, Entry, Outcome, Transcript};
    use crate::session::plain_session::ReplayItem;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(ch: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(ch), KeyModifiers::CONTROL)
    }

    #[test]
    fn search_reaches_tool_output_and_survives_a_narrow_rewrap() {
        let mut transcript = Transcript::new(&[
            ReplayItem::User("검사해 줘".into()),
            tool("first_byte_after_restore ... ok"),
            tool("other FIRST_BYTE result"),
        ], 60);
        transcript.lines(60, 12);
        transcript.key(ctrl('f'));
        for ch in "first_byte".chars() { transcript.key(key(KeyCode::Char(ch))); }
        assert_eq!(transcript.matches.len(), 2);
        assert_eq!(transcript.selected_match, Some(0));
        assert_eq!(transcript.key(key(KeyCode::Esc)), Outcome::Handled);
        transcript.key(key(KeyCode::Char('n')));
        assert_eq!(transcript.selected_match, Some(1));
        let shown = plain_text(&transcript.lines(12, 18)).join("\n");
        assert!(shown.contains("bash"), "matched tool stays reachable: {shown}");
        assert_eq!(transcript.matches.len(), 2, "wrapping cannot change source matches");
        transcript.key(key(KeyCode::Char('N')));
        assert_eq!(transcript.selected_match, Some(0));
    }

    #[test]
    fn search_edits_korean_and_turn_navigation_never_changes_the_conversation() {
        let items = vec![ReplayItem::User("첫 부탁".into()), tool(&"결과\n".repeat(25)),
            ReplayItem::User("다음 부탁".into()), tool(&"검사\n".repeat(25))];
        let mut transcript = Transcript::new(&items, 50);
        transcript.lines(50, 12);
        transcript.key(key(KeyCode::Home));
        transcript.key(key(KeyCode::Char(']')));
        assert_eq!(transcript.offset, transcript.turn_lines[1]);
        transcript.key(key(KeyCode::Char('[')));
        assert_eq!(transcript.offset, 0);
        transcript.key(key(KeyCode::Char('/')));
        for ch in "없는말".chars() { transcript.key(key(KeyCode::Char(ch))); }
        assert!(transcript.matches.is_empty());
        for _ in 0..3 { transcript.key(key(KeyCode::Backspace)); }
        for ch in "첫 부탁".chars() { transcript.key(key(KeyCode::Char(ch))); }
        assert_eq!(transcript.matches, vec![0]);
        transcript.key(key(KeyCode::Enter));
        assert!(!transcript.finding);
        assert_eq!(transcript.items.len(), items.len());
    }

    fn plain_text(lines: &[super::Line]) -> Vec<String> {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.text.as_str())
                    .collect::<String>()
            })
            .collect()
    }

    fn tool(output: &str) -> ReplayItem {
        ReplayItem::ToolCall {
            name: "bash".to_string(),
            input: "{\"command\":\"ls\"}".to_string(),
            output: Some(output.to_string()),
            is_error: false,
        }
    }

    /// The whole point: a tool body the committed cell folds away is printed in
    /// full here. A transcript that also truncated would leave the person with
    /// no way to read it at all.
    #[test]
    fn a_folded_tool_body_is_printed_whole() {
        let body = (0..40)
            .map(|i| format!("output line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut transcript = Transcript::new(&[tool(&body)], 60);
        transcript.key(key(KeyCode::Home));
        let mut seen = Vec::new();
        // Walk the whole pager, a page at a time, collecting what it shows.
        loop {
            seen.extend(plain_text(&transcript.lines(60, 20)));
            let before = transcript.offset;
            transcript.key(key(KeyCode::PageDown));
            if transcript.offset == before {
                break;
            }
        }
        let joined = seen.join("\n");
        for i in 0..40 {
            assert!(
                joined.contains(&format!("output line {i}")),
                "line {i} must be reachable in the transcript"
            );
        }
    }

    /// Opening lands on the tail — a long run's interesting end, not its start.
    #[test]
    fn it_opens_on_the_last_page() {
        let body = (0..100)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut transcript = Transcript::new(&[tool(&body)], 60);
        let shown = plain_text(&transcript.lines(60, 20));
        assert!(
            shown.iter().any(|line| line.contains("line 99")),
            "the tail is on screen at open: {shown:?}"
        );
        transcript.key(key(KeyCode::Home));
        let top = plain_text(&transcript.lines(60, 20));
        assert!(
            top.iter().any(|line| line.contains("line 0")),
            "Home reaches the head: {top:?}"
        );
    }

    /// Scrolling can never leave the content — a pager that scrolls past its
    /// end shows a blank screen and reads as a crash.
    #[test]
    fn scrolling_stays_inside_the_transcript() {
        let mut transcript = Transcript::new(&[tool("one\ntwo\nthree")], 60);
        for _ in 0..50 {
            transcript.key(key(KeyCode::PageDown));
            transcript.key(key(KeyCode::Down));
        }
        let shown = plain_text(&transcript.lines(60, 20));
        assert!(
            shown.iter().any(|line| line.contains("three")),
            "the tail stays on screen however far down we push: {shown:?}"
        );
        for _ in 0..50 {
            transcript.key(key(KeyCode::PageUp));
            transcript.key(key(KeyCode::Up));
        }
        let shown = plain_text(&transcript.lines(60, 20));
        assert!(
            shown.iter().any(|line| line.contains("one")),
            "and the head stays reachable: {shown:?}"
        );
    }

    /// An empty session opens without panicking and says so.
    #[test]
    fn an_empty_session_still_opens() {
        let mut transcript = Transcript::new(&[], 60);
        let shown = plain_text(&transcript.lines(60, 20)).join("\n");
        assert!(shown.contains("no turns yet"), "{shown}");
    }

    /// Every turn kind reaches the page, marked the way its live cell marks it.
    #[test]
    fn each_turn_kind_carries_its_own_marker() {
        let items = vec![
            ReplayItem::User("do the thing".to_string()),
            ReplayItem::Assistant("here is the answer".to_string()),
            tool("tool said this"),
        ];
        let mut transcript = Transcript::new(&items, 60);
        transcript.key(key(KeyCode::Home));
        let shown = plain_text(&transcript.lines(60, 30));
        let joined = shown.join("\n");
        assert!(joined.contains("› do the thing"), "{joined}");
        assert!(joined.contains("• here is the answer"), "{joined}");
        assert!(joined.contains("• bash"), "{joined}");
        assert!(joined.contains("└ tool said this"), "{joined}");
    }

    #[test]
    fn transcript_only_reasoning_is_markdown_rendered_and_dimmed() {
        let entries = vec![Entry::Reasoning("**Important conclusion**".to_string())];
        let mut transcript = Transcript::from_entries(&entries, 60);
        transcript.key(key(KeyCode::Home));

        let shown = plain_text(&transcript.lines(60, 20)).join("\n");

        assert!(shown.contains("• Important conclusion"), "{shown}");
        assert!(!shown.contains("**"), "{shown}");
        let body = transcript
            .lines
            .iter()
            .flat_map(|line| &line.spans)
            .filter(|span| !span.text.trim().is_empty() && span.text != "• ")
            .collect::<Vec<_>>();
        assert!(!body.is_empty(), "reasoning body spans: {:?}", transcript.lines);
        assert!(body.iter().all(|span| span.style.dim && span.style.italic));
    }

    /// A tool that printed nothing must still appear — an invisible call is
    /// worse than an empty one.
    #[test]
    fn a_silent_tool_still_appears() {
        let items = vec![ReplayItem::ToolCall {
            name: "write_file".to_string(),
            input: "{\"path\":\"a.rs\"}".to_string(),
            output: Some(String::new()),
            is_error: false,
        }];
        let mut transcript = Transcript::new(&items, 60);
        let joined = plain_text(&transcript.lines(60, 20)).join("\n");
        assert!(joined.contains("• write_file"), "{joined}");
    }

    #[test]
    fn narrowing_rewraps_instead_of_truncating_session_text() {
        let text = "alpha bravo charlie delta echo foxtrot";
        let mut transcript = Transcript::new(&[ReplayItem::Assistant(text.to_string())], 60);
        transcript.key(key(KeyCode::Home));

        let shown = plain_text(&transcript.lines(18, 30)).join("\n");

        for word in text.split_whitespace() {
            assert!(shown.contains(word), "{word:?} was lost after narrowing: {shown:?}");
        }
    }

    #[test]
    fn widening_rewraps_a_narrow_transcript_back_onto_fewer_rows() {
        let text = "alpha bravo charlie delta echo foxtrot";
        let mut transcript = Transcript::new(&[ReplayItem::Assistant(text.to_string())], 18);
        transcript.key(key(KeyCode::Home));

        let shown = plain_text(&transcript.lines(60, 30));

        assert!(shown.iter().any(|line| line == &format!("• {text}")), "{shown:?}");
    }

    /// Ctrl+T opens and closes; Alt+T is somebody else's chord.
    #[test]
    fn the_open_key_is_codexs_ctrl_t() {
        assert!(is_open_key(&ctrl('t')));
        assert!(is_open_key(&ctrl('T')));
        assert!(!is_open_key(&key(KeyCode::Char('t'))));
        assert!(!is_open_key(&KeyEvent::new(
            KeyCode::Char('t'),
            KeyModifiers::CONTROL | KeyModifiers::ALT
        )));

        let mut transcript = Transcript::new(&[tool("body")], 60);
        assert_eq!(transcript.key(ctrl('t')), Outcome::Close);
        assert_eq!(transcript.key(key(KeyCode::Esc)), Outcome::Close);
        assert_eq!(transcript.key(key(KeyCode::Down)), Outcome::Handled);
    }
}
