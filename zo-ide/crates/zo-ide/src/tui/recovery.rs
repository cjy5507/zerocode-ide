//! Keyboard recovery uses the same revision-bound actions as the GUI dialog.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::Value;
use zerocode_core::conversation_control::{Action, RestoreScope};
use super::ansi::{Line, Span, Style};
use super::{palette, wrap::wrap_line};

pub(crate) enum Outcome {
    Handled,
    Close,
    Action(Action),
    Changes(u32),
}

pub(crate) struct View {
    pub(crate) session: String,
    turns: Vec<(u32, String)>,
    selected: usize,
    preview: Option<Value>,
    scope: usize,
    pub(crate) error: Option<String>,
}

const SCOPES: [(RestoreScope, &str, &str); 3] = [
    (RestoreScope::Both, "Conversation and files", "Restore the conversation and tracked file edits."),
    (RestoreScope::Conversation, "Conversation only", "Keep current files and restart from this prompt."),
    (RestoreScope::Files, "Files only", "Keep the conversation and restore tracked file edits."),
];

impl View {
    pub(crate) fn new(session: String, mut turns: Vec<(u32, String)>) -> Self {
        turns.reverse();
        Self { session, turns, selected: 0, preview: None, scope: 0, error: None }
    }

    pub(crate) fn set_preview(&mut self, preview: Value) {
        self.preview = Some(preview);
        self.error = None;
        self.scope = (0..SCOPES.len()).find(|&at| self.enabled(at)).unwrap_or(0);
    }

    fn enabled(&self, at: usize) -> bool {
        let Some(preview) = &self.preview else { return false; };
        let chat = preview["conversation_available"].as_bool() == Some(true);
        let files = preview["files_available"].as_bool() == Some(true);
        match SCOPES[at].0 {
            RestoreScope::Both => chat && files,
            RestoreScope::Conversation => chat,
            RestoreScope::Files => files,
        }
    }

    pub(crate) fn key(&mut self, key: KeyEvent) -> Outcome {
        if key.code == KeyCode::Esc {
            if self.preview.take().is_some() { self.error = None; return Outcome::Handled; }
            return Outcome::Close;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Outcome::Close;
        }
        if let Some(preview) = &self.preview {
            let index = preview["message_index"].as_u64().and_then(|index| u32::try_from(index).ok()).unwrap_or(u32::MAX);
            let revision = preview["revision"].as_str().unwrap_or_default().to_string();
            match key.code {
                KeyCode::Up => self.scope = self.scope.saturating_sub(1),
                KeyCode::Down => self.scope = (self.scope + 1).min(SCOPES.len() - 1),
                KeyCode::Char(ch @ '1'..='3') => self.scope = match ch { '1' => 0, '2' => 1, _ => 2 },
                KeyCode::Char('d') => return Outcome::Changes(index),
                KeyCode::Char('f') if self.error.is_none() => return Outcome::Action(Action::Fork { message_index: index, revision }),
                KeyCode::Enter if self.enabled(self.scope) && self.error.is_none() => return Outcome::Action(Action::Restore { message_index: index, revision, scope: SCOPES[self.scope].0 }),
                _ => {}
            }
        } else {
            match key.code {
                KeyCode::Up => self.selected = self.selected.saturating_sub(1),
                KeyCode::Down => self.selected = (self.selected + 1).min(self.turns.len().saturating_sub(1)),
                KeyCode::Enter => if let Some((index, _)) = self.turns.get(self.selected) {
                    return Outcome::Action(Action::Preview { message_index: *index });
                },
                _ => {}
            }
        }
        Outcome::Handled
    }

    pub(crate) fn lines(&self, width: usize, height: usize) -> Vec<Line> {
        let mut lines = vec![Line::empty(), Line::new(vec![Span::bold("  Return to an earlier turn")]), Line::empty()];
        if let Some(error) = &self.error { lines.push(Line::from_text(format!("  {error} · esc to choose again"))); }
        let hint;
        if let Some(preview) = &self.preview {
            let prompt = preview["prompt"].as_str().unwrap_or_default();
            lines.extend(wrap_line(&Line::from_text(format!("  {prompt}")), width, &Span::raw("  ")).into_iter().take(if height < 20 { 1 } else { 3 }));
            lines.push(Line::empty());
            for (at, (_, label, detail)) in SCOPES.iter().enumerate() {
                let style = if !self.enabled(at) { Style::new().dim() } else if at == self.scope { Style::new().bold().fg(palette::COMMAND_TOKEN) } else { Style::new() };
                lines.push(Line::new(vec![Span::new(format!("  {} {}. {label}", if at == self.scope { "›" } else { " " }, at + 1), style)]));
                if height >= 20 { lines.push(Line::new(vec![Span::dim(format!("       {detail}"))])); }
            }
            lines.push(Line::empty());
            let paths = preview.pointer("/files/paths").and_then(Value::as_array);
            lines.push(Line::from_text(format!("  Tracked files: {}", paths.map_or(0, Vec::len))));
            if let Some(paths) = paths {
                for path in paths.iter().take(3).filter_map(Value::as_str) { lines.push(Line::from_text(format!("    {path}"))); }
            }
            if let Some(problem) = preview["file_problem"].as_str() { lines.push(Line::from_text(format!("  {problem}"))); }
            if preview.pointer("/files/incomplete_range").and_then(Value::as_bool) == Some(true) {
                lines.push(Line::from_text("  Untracked shell changes are not included."));
            }
            hint = "  ↑↓ / 1–3 choose · enter restore · d changes · f fork · esc back";
        } else {
            if self.turns.is_empty() { lines.push(Line::from_text("  No recoverable prompts in the current context.")); }
            let room = height.saturating_sub(7).max(1);
            let from = self.selected.saturating_sub(room / 2);
            for (at, (_, prompt)) in self.turns.iter().enumerate().skip(from).take(room) {
                let style = if at == self.selected { Style::new().bold().fg(palette::COMMAND_TOKEN) } else { Style::new() };
                lines.push(Line::new(vec![Span::new(format!("  {} {}", if at == self.selected { "›" } else { " " }, prompt.split_whitespace().collect::<Vec<_>>().join(" ")), style)]));
            }
            hint = "  ↑↓ choose a prompt · enter preview · esc close";
        }
        lines.truncate(height.saturating_sub(2));
        while lines.len() < height.saturating_sub(1) { lines.push(Line::empty()); }
        lines.push(Line::new(vec![Span::dim(hint)]));
        lines.into_iter().map(|line| line.truncated(width)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(code: KeyCode) -> KeyEvent { KeyEvent::new(code, KeyModifiers::NONE) }

    #[test]
    fn preview_is_required_and_escape_never_restores() {
        let mut view = View::new("s".into(), vec![(12, "첫 요청".into()), (27, "최근 요청".into())]);
        assert!(matches!(view.key(key(KeyCode::Enter)), Outcome::Action(Action::Preview { message_index: 27 })));
        view.set_preview(serde_json::json!({"message_index":27,"revision":"v1","conversation_available":true,"files_available":false,"prompt":"최근 요청"}));
        assert_eq!(view.scope, 1);
        assert!(matches!(view.key(key(KeyCode::Enter)), Outcome::Action(Action::Restore { message_index:27, scope:RestoreScope::Conversation, .. })));
        assert!(matches!(view.key(key(KeyCode::Esc)), Outcome::Handled));
        assert!(matches!(view.key(key(KeyCode::Esc)), Outcome::Close));
        for width in [20, 40, 80] {
            assert!(view.lines(width, 12).iter().all(|line| line.width() <= width));
        }
    }
}
