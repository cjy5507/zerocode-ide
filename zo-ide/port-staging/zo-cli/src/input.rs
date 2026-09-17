#![allow(dead_code)]

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::io::{self, Write};

use commands::public_slash_command_specs;
use rustyline::completion::{Completer, Pair};
use rustyline::highlight::{CmdKind, Highlighter};
use rustyline::hint::{Hint, Hinter};
use rustyline::history::DefaultHistory;
use rustyline::validate::Validator;
use rustyline::{
    Cmd, CompletionType, Config, Context, EditMode, Editor, Helper, KeyCode, KeyEvent, Modifiers,
};

/// Maximum number of suggestions to render in the inline dropdown.
const MAX_HINT_ROWS: usize = 5;

/// Custom hint type that cleanly separates what's displayed from what's inserted
/// on accept (right-arrow / end-of-line). Without this split, rustyline would
/// treat the description text as completion payload and stuff it into the input
/// buffer the moment the user pressed right-arrow or space.
#[derive(Debug, Clone)]
pub(crate) struct SlashHint {
    display: String,
    completion: String,
}

impl Hint for SlashHint {
    fn display(&self) -> &str {
        &self.display
    }

    fn completion(&self) -> Option<&str> {
        if self.completion.is_empty() {
            None
        } else {
            Some(&self.completion)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadOutcome {
    Submit(String),
    Cancel,
    Exit,
}

struct SlashCommandHelper {
    completions: Vec<String>,
    descriptions: HashMap<String, String>,
    current_line: RefCell<String>,
}

impl SlashCommandHelper {
    fn new(completions: Vec<String>) -> Self {
        Self {
            completions: normalize_completions(completions),
            descriptions: build_description_map(),
            current_line: RefCell::new(String::new()),
        }
    }

    fn description_for(&self, candidate: &str) -> Option<&str> {
        // Try the full candidate first (e.g. "/model"), then the first token
        // (so "/model opus" falls back to "/model"'s description).
        if let Some(desc) = self.descriptions.get(candidate) {
            return Some(desc.as_str());
        }
        let head = candidate.split_whitespace().next()?;
        self.descriptions.get(head).map(String::as_str)
    }

    fn reset_current_line(&self) {
        self.current_line.borrow_mut().clear();
    }

    fn current_line(&self) -> String {
        self.current_line.borrow().clone()
    }

    fn set_current_line(&self, line: &str) {
        let mut current = self.current_line.borrow_mut();
        current.clear();
        current.push_str(line);
    }

    fn set_completions(&mut self, completions: Vec<String>) {
        self.completions = normalize_completions(completions);
    }
}

impl Completer for SlashCommandHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Self::Candidate>)> {
        let Some(prefix) = slash_command_prefix(line, pos) else {
            return Ok((0, Vec::new()));
        };

        let matches = self
            .completions
            .iter()
            .filter(|candidate| candidate.starts_with(prefix))
            .map(|candidate| Pair {
                display: candidate.clone(),
                replacement: candidate.clone(),
            })
            .collect();

        Ok((0, matches))
    }
}

impl Hinter for SlashCommandHelper {
    type Hint = SlashHint;

    fn hint(&self, line: &str, pos: usize, _ctx: &Context<'_>) -> Option<SlashHint> {
        let prefix = slash_command_prefix(line, pos)?;

        // Collect all prefix matches first; these rank above fuzzy fallbacks.
        let mut ranked: Vec<(i64, &String)> = self
            .completions
            .iter()
            .filter(|candidate| candidate.as_str() != prefix)
            .filter(|candidate| candidate.starts_with(prefix))
            .map(|candidate| {
                // Prefix matches beat fuzzy scores (which are small integers).
                // Shorter candidates sort first within the prefix bucket.
                let len = i64::try_from(candidate.len()).unwrap_or(i64::MAX);
                (1_000_000 - len, candidate)
            })
            .collect();

        // If we have no prefix matches and the user has typed enough, fall back to fuzzy.
        if ranked.is_empty() && prefix.len() >= 3 {
            ranked = self
                .completions
                .iter()
                .filter(|candidate| candidate.as_str() != prefix)
                .filter_map(|candidate| {
                    fuzzy_score(prefix, candidate).map(|score| (score, candidate))
                })
                .collect();
        }

        if ranked.is_empty() {
            return None;
        }

        // Best match wins the completion slot (what gets inserted on right-arrow).
        ranked.sort_by(|a, b| b.0.cmp(&a.0));
        let best = ranked[0].1.clone();
        let completion = best
            .strip_prefix(prefix)
            .map(str::to_string)
            .unwrap_or_default();

        // Display = inline ghost text for the best match + a stacked dropdown of
        // alternatives on subsequent lines. Lines after the first are pushed with
        // '\n' and styled dim; rustyline tracks the rendered row count and clears
        // these extra lines on the next redraw.
        let desc_best = self.description_for(&best).unwrap_or("");
        let mut display = if completion.is_empty() {
            format!(" → {best}")
        } else {
            completion.clone()
        };
        if !desc_best.is_empty() {
            display.push_str("  \x1b[2;3;38;5;244m— ");
            display.push_str(desc_best);
            display.push_str("\x1b[0m");
        }

        let rows_left = MAX_HINT_ROWS.saturating_sub(1);
        for (_, candidate) in ranked.iter().skip(1).take(rows_left) {
            let desc = self.description_for(candidate).unwrap_or("");
            display.push_str("\n\x1b[2;38;5;245m  ");
            display.push_str(candidate);
            if !desc.is_empty() {
                display.push_str("\x1b[0m\x1b[2;3;38;5;244m  — ");
                display.push_str(desc);
            }
            display.push_str("\x1b[0m");
        }

        Some(SlashHint {
            display,
            completion,
        })
    }
}

impl Highlighter for SlashCommandHelper {
    fn highlight<'l>(&self, line: &'l str, _pos: usize) -> Cow<'l, str> {
        self.set_current_line(line);
        Cow::Borrowed(line)
    }

    fn highlight_hint<'h>(&self, hint: &'h str) -> Cow<'h, str> {
        // The hint already contains per-row ANSI styling built in `Hinter::hint`,
        // so we only need to colorize the leading inline tail (up to the first
        // newline or ANSI escape). We keep the rest verbatim.
        let head_end = hint.find(['\n', '\u{1b}']).unwrap_or(hint.len());
        let (head, rest) = hint.split_at(head_end);
        Cow::Owned(format!("\x1b[2;38;5;245m{head}\x1b[0m{rest}"))
    }

    fn highlight_char(&self, line: &str, _pos: usize, _kind: CmdKind) -> bool {
        self.set_current_line(line);
        // Return true when there's a hint to display so rustyline refreshes
        line.starts_with('/')
    }
}

impl Validator for SlashCommandHelper {}
impl Helper for SlashCommandHelper {}

pub struct LineEditor {
    prompt: String,
    editor: Editor<SlashCommandHelper, DefaultHistory>,
}

impl LineEditor {
    #[must_use]
    pub fn new(prompt: impl Into<String>, completions: Vec<String>) -> Self {
        let config = Config::builder()
            .completion_type(CompletionType::List)
            .edit_mode(EditMode::Emacs)
            .build();
        let mut editor = Editor::<SlashCommandHelper, DefaultHistory>::with_config(config)
            .expect("rustyline editor should initialize");
        editor.set_helper(Some(SlashCommandHelper::new(completions)));
        editor.bind_sequence(KeyEvent(KeyCode::Char('J'), Modifiers::CTRL), Cmd::Newline);
        editor.bind_sequence(KeyEvent(KeyCode::Enter, Modifiers::SHIFT), Cmd::Newline);
        // Right-arrow at end of line accepts the current hint (fish-style).
        editor.bind_sequence(KeyEvent(KeyCode::Right, Modifiers::NONE), Cmd::CompleteHint);

        Self {
            prompt: prompt.into(),
            editor,
        }
    }

    pub fn push_history(&mut self, entry: impl Into<String>) {
        let entry = entry.into();
        if entry.trim().is_empty() {
            return;
        }

        let _ = self.editor.add_history_entry(entry);
    }

    pub fn set_completions(&mut self, completions: Vec<String>) {
        if let Some(helper) = self.editor.helper_mut() {
            helper.set_completions(completions);
        }
    }

    fn current_line(&self) -> String {
        self.editor
            .helper()
            .map_or_else(String::new, SlashCommandHelper::current_line)
    }

    fn finish_interrupted_read(&mut self) -> io::Result<()> {
        if let Some(helper) = self.editor.helper_mut() {
            helper.reset_current_line();
        }
        let mut stdout = io::stdout();
        writeln!(stdout)
    }

    fn read_line_fallback(&self) -> io::Result<ReadOutcome> {
        let mut stdout = io::stdout();
        write!(stdout, "{}", self.prompt)?;
        stdout.flush()?;

        let mut buffer = String::new();
        let bytes_read = io::stdin().read_line(&mut buffer)?;
        if bytes_read == 0 {
            return Ok(ReadOutcome::Exit);
        }

        while matches!(buffer.chars().last(), Some('\n' | '\r')) {
            buffer.pop();
        }
        Ok(ReadOutcome::Submit(buffer))
    }
}

fn slash_command_prefix(line: &str, pos: usize) -> Option<&str> {
    if pos != line.len() {
        return None;
    }

    let prefix = &line[..pos];
    if !prefix.starts_with('/') {
        return None;
    }

    Some(prefix)
}

/// Probe the current terminal width; falls back to 120 columns if unavailable.
fn terminal_width() -> usize {
    crossterm::terminal::size()
        .map(|(cols, _)| cols as usize)
        .unwrap_or(120)
        .max(20)
}

/// Build a dim horizontal rule. When `is_top` is true and `label` is provided,
/// the label is right-anchored into the rule (" ─── slash-hints ──").
fn horizontal_rule(width: usize, label: Option<&str>, is_top: bool) -> String {
    const DIM_GREY: &str = "\x1b[2;38;5;240m";
    const RESET: &str = "\x1b[0m";
    let ch = '─';
    if let (true, Some(label)) = (is_top, label.filter(|value| !value.trim().is_empty())) {
        // "──────── label ──"
        let label = format!(" {} ", label.trim());
        let trailing = 2usize; // "──" on the right
        let label_len = label.chars().count();
        let leading = width.saturating_sub(label_len + trailing).max(2);
        let mut out = String::with_capacity(width + DIM_GREY.len() + RESET.len());
        out.push_str(DIM_GREY);
        for _ in 0..leading {
            out.push(ch);
        }
        out.push_str(&label);
        for _ in 0..trailing {
            out.push(ch);
        }
        out.push_str(RESET);
        out
    } else {
        let mut out = String::with_capacity(width + DIM_GREY.len() + RESET.len());
        out.push_str(DIM_GREY);
        for _ in 0..width {
            out.push(ch);
        }
        out.push_str(RESET);
        out
    }
}

fn build_description_map() -> HashMap<String, String> {
    let mut map = HashMap::new();
    for spec in public_slash_command_specs() {
        let key = format!("/{}", spec.name);
        map.insert(key, spec.summary.to_string());
        for alias in spec.aliases {
            map.insert(format!("/{alias}"), spec.summary.to_string());
        }
    }
    map
}

/// Subsequence fuzzy score. Returns `None` if `needle` is not a subsequence of
/// `haystack` (both compared case-insensitively). Higher scores are better.
///
/// Scoring rewards (a) matches at the start of the haystack, (b) contiguous
/// runs of matched characters, and (c) matches on fewer total gaps. This is
/// deliberately minimal — we only use it as a fallback when prefix matching
/// fails, so a full nucleo-style ranker would be overkill.
fn fuzzy_score(needle: &str, haystack: &str) -> Option<i64> {
    let needle = needle
        .strip_prefix('/')
        .unwrap_or(needle)
        .to_ascii_lowercase();
    let haystack_lc = haystack
        .strip_prefix('/')
        .unwrap_or(haystack)
        .to_ascii_lowercase();
    if needle.is_empty() {
        return None;
    }

    let mut score: i64 = 0;
    let mut last_match: Option<usize> = None;
    let mut n_iter = needle.chars().peekable();
    let mut matched = 0usize;

    for (idx, h_ch) in haystack_lc.chars().enumerate() {
        let Some(&n_ch) = n_iter.peek() else { break };
        if h_ch == n_ch {
            matched += 1;
            n_iter.next();
            if idx == 0 {
                score += 10;
            }
            if let Some(prev) = last_match {
                if idx == prev + 1 {
                    score += 5;
                } else {
                    let gap = idx - prev - 1;
                    score -= i64::try_from(gap).unwrap_or(i64::MAX);
                }
            }
            last_match = Some(idx);
        }
    }

    if matched < needle.chars().count() {
        return None;
    }
    let len = i64::try_from(haystack_lc.chars().count()).unwrap_or(i64::MAX);
    score -= len / 4;
    Some(score)
}

fn normalize_completions(completions: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    completions
        .into_iter()
        .filter(|candidate| candidate.starts_with('/'))
        .filter(|candidate| seen.insert(candidate.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{slash_command_prefix, LineEditor, SlashCommandHelper};
    use rustyline::completion::Completer;
    use rustyline::highlight::Highlighter;
    use rustyline::history::{DefaultHistory, History};
    use rustyline::Context;

    #[test]
    fn extracts_terminal_slash_command_prefixes_with_arguments() {
        assert_eq!(slash_command_prefix("/he", 3), Some("/he"));
        assert_eq!(slash_command_prefix("/help me", 8), Some("/help me"));
        assert_eq!(
            slash_command_prefix("/session switch ses", 19),
            Some("/session switch ses")
        );
        assert_eq!(slash_command_prefix("hello", 5), None);
        assert_eq!(slash_command_prefix("/help", 2), None);
    }

    #[test]
    fn completes_matching_slash_commands() {
        let helper = SlashCommandHelper::new(vec![
            "/help".to_string(),
            "/hello".to_string(),
            "/status".to_string(),
        ]);
        let history = DefaultHistory::new();
        let ctx = Context::new(&history);
        let (start, matches) = helper
            .complete("/he", 3, &ctx)
            .expect("completion should work");

        assert_eq!(start, 0);
        assert_eq!(
            matches
                .into_iter()
                .map(|candidate| candidate.replacement)
                .collect::<Vec<_>>(),
            vec!["/help".to_string(), "/hello".to_string()]
        );
    }

    #[test]
    fn completes_matching_slash_command_arguments() {
        let helper = SlashCommandHelper::new(vec![
            "/model".to_string(),
            "/model opus".to_string(),
            "/model sonnet".to_string(),
            "/session switch alpha".to_string(),
        ]);
        let history = DefaultHistory::new();
        let ctx = Context::new(&history);
        let (start, matches) = helper
            .complete("/model o", 8, &ctx)
            .expect("completion should work");

        assert_eq!(start, 0);
        assert_eq!(
            matches
                .into_iter()
                .map(|candidate| candidate.replacement)
                .collect::<Vec<_>>(),
            vec!["/model opus".to_string()]
        );
    }

    #[test]
    fn ignores_non_slash_command_completion_requests() {
        let helper = SlashCommandHelper::new(vec!["/help".to_string()]);
        let history = DefaultHistory::new();
        let ctx = Context::new(&history);
        let (_, matches) = helper
            .complete("hello", 5, &ctx)
            .expect("completion should work");

        assert!(matches.is_empty());
    }

    #[test]
    fn hint_includes_description_for_prefix_match() {
        use rustyline::hint::{Hint, Hinter};
        let helper = SlashCommandHelper::new(vec!["/model".to_string()]);
        let history = DefaultHistory::new();
        let ctx = Context::new(&history);
        let hint = helper.hint("/mo", 3, &ctx).expect("hint present");
        assert_eq!(hint.completion(), Some("del"));
        assert!(
            hint.display().contains("Show or switch the active model"),
            "hint missing description: {}",
            hint.display()
        );
    }

    #[test]
    fn hint_fuzzy_recovers_from_typos() {
        use rustyline::hint::{Hint, Hinter};
        let helper = SlashCommandHelper::new(vec![
            "/model".to_string(),
            "/memory".to_string(),
            "/help".to_string(),
        ]);
        let history = DefaultHistory::new();
        let ctx = Context::new(&history);
        let hint = helper.hint("/modl", 5, &ctx).expect("fuzzy hint present");
        assert!(
            hint.display().contains("/model"),
            "fuzzy fallback should point to /model, got: {}",
            hint.display()
        );
    }

    #[test]
    fn hint_completion_excludes_description_text() {
        use rustyline::hint::{Hint, Hinter};
        let helper = SlashCommandHelper::new(vec!["/model".to_string()]);
        let history = DefaultHistory::new();
        let ctx = Context::new(&history);
        let hint = helper.hint("/mo", 3, &ctx).expect("hint present");
        // completion() is what rustyline inserts when the user accepts the hint
        // (right-arrow). It must NOT contain description text, otherwise the
        // ghost text leaks into the input buffer.
        let completion = hint.completion().expect("completion should exist");
        assert!(
            !completion.contains("Show or switch"),
            "description text leaked into completion: {completion}"
        );
        assert!(
            !completion.contains('—'),
            "em-dash separator leaked into completion: {completion}"
        );
    }

    #[test]
    fn hint_display_stacks_multiple_matches() {
        use rustyline::hint::{Hint, Hinter};
        let helper = SlashCommandHelper::new(vec![
            "/help".to_string(),
            "/hooks".to_string(),
            "/hello".to_string(),
        ]);
        let history = DefaultHistory::new();
        let ctx = Context::new(&history);
        let hint = helper.hint("/he", 3, &ctx).expect("hint present");
        let display = hint.display();
        // We expect multiple candidates stacked on newlines in the dropdown.
        assert!(
            display.contains('\n'),
            "expected multi-line dropdown, got: {display}"
        );
    }

    #[test]
    fn fuzzy_score_rejects_non_subsequence() {
        assert!(super::fuzzy_score("xyz", "/model").is_none());
    }

    #[test]
    fn tracks_current_buffer_through_highlighter() {
        let helper = SlashCommandHelper::new(Vec::new());
        let _ = helper.highlight("draft", 5);

        assert_eq!(helper.current_line(), "draft");
    }

    #[test]
    fn push_history_ignores_blank_entries() {
        let mut editor = LineEditor::new("> ", vec!["/help".to_string()]);
        editor.push_history("   ");
        editor.push_history("/help");

        assert_eq!(editor.editor.history().len(), 1);
    }

    #[test]
    fn set_completions_replaces_and_normalizes_candidates() {
        let mut editor = LineEditor::new("> ", vec!["/help".to_string()]);
        editor.set_completions(vec![
            "/model opus".to_string(),
            "/model opus".to_string(),
            "status".to_string(),
        ]);

        let helper = editor.editor.helper().expect("helper should exist");
        assert_eq!(helper.completions, vec!["/model opus".to_string()]);
    }
}
