//! Turn-time input queues and their Codex-style preview.
//!
//! Enter submits a steer to the running turn, while Tab queues a fresh
//! follow-up turn. Neither action is history yet: this module keeps the input
//! in the bottom pane until the runtime acknowledges the steer or the queued
//! turn actually starts.

use std::collections::VecDeque;

use super::ansi::{Line, Span, Style};
use super::composer::{Composer, Submission};
use super::wrap::wrap_line;

const PREVIEW_LINE_LIMIT: usize = 3;

/// Inputs which have not become committed user turns yet.
#[derive(Debug)]
pub struct PendingInputs {
    pending_steers: VecDeque<String>,
    rejected_steers: VecDeque<String>,
    queued_messages: VecDeque<Submission>,
    submit_steers_after_interrupt: bool,
    interrupt_binding: String,
    edit_binding: String,
}

impl Default for PendingInputs {
    fn default() -> Self {
        Self {
            pending_steers: VecDeque::new(),
            rejected_steers: VecDeque::new(),
            queued_messages: VecDeque::new(),
            submit_steers_after_interrupt: false,
            interrupt_binding: "esc".to_string(),
            edit_binding: "⌥ + ↑".to_string(),
        }
    }
}

impl PendingInputs {
    pub fn clear(&mut self) {
        self.pending_steers.clear();
        self.rejected_steers.clear();
        self.queued_messages.clear();
        self.submit_steers_after_interrupt = false;
    }

    pub fn push_steer(&mut self, text: String) {
        self.pending_steers.push_back(text);
    }

    /// A runtime steering-echo block is the acknowledgement that this input
    /// crossed a model boundary and may now become history.
    pub fn acknowledge_steer(&mut self, text: &str) {
        if let Some(index) = self.pending_steers.iter().position(|pending| pending == text) {
            self.pending_steers.remove(index);
        }
        if self.pending_steers.is_empty() {
            self.submit_steers_after_interrupt = false;
        }
    }

    #[must_use]
    pub fn has_pending_steers(&self) -> bool {
        !self.pending_steers.is_empty()
    }

    pub fn interrupt_and_submit_steers(&mut self) {
        self.submit_steers_after_interrupt = true;
    }

    /// Take the still-unacknowledged steers after an interrupt. An empty
    /// result means the runtime consumed them before cancellation won the
    /// race, so no fresh turn should be manufactured.
    pub fn take_interrupted_steers(&mut self) -> Option<Vec<String>> {
        if !std::mem::take(&mut self.submit_steers_after_interrupt) {
            return None;
        }
        let steers = self.pending_steers.drain(..).collect::<Vec<_>>();
        (!steers.is_empty()).then_some(steers)
    }

    /// A turn can fail before core commits a submitted steer. Codex retries
    /// those before ordinary Tab-queued follow-ups on the next idle boundary.
    pub fn reject_unacknowledged_steers(&mut self) -> Vec<String> {
        let rejected = self.pending_steers.drain(..).collect::<Vec<_>>();
        self.rejected_steers.extend(rejected.iter().cloned());
        rejected
    }

    pub fn push_queued(&mut self, submission: Submission) {
        self.queued_messages.push_back(submission);
    }

    pub fn set_interrupt_binding(&mut self, binding: impl Into<String>) {
        let binding = binding.into();
        if !binding.trim().is_empty() {
            self.interrupt_binding = binding;
        }
    }

    pub fn set_edit_binding(&mut self, binding: impl Into<String>) {
        let binding = binding.into();
        if !binding.trim().is_empty() {
            self.edit_binding = binding;
        }
    }

    /// Restore the newest Tab-queued draft, replacing the current composer in
    /// the same way Codex's `edit_queued_message` binding does.
    pub fn edit_latest_queued(&mut self, composer: &mut Composer) -> bool {
        let Some(submission) = self.queued_messages.pop_back() else {
            return false;
        };
        composer.restore_submission(submission);
        true
    }

    /// Rejected steers are merged into one fresh turn, then ordinary queued
    /// messages drain FIFO one turn at a time.
    pub fn pop_next_turn(&mut self) -> Option<Submission> {
        if !self.rejected_steers.is_empty() {
            let text = self.rejected_steers.drain(..).collect::<Vec<_>>().join("\n\n");
            return Some(Submission {
                text,
                image_paths: Vec::new(),
            });
        }
        self.queued_messages.pop_front()
    }

    #[must_use]
    pub fn lines(&self, width: usize) -> Vec<Line> {
        if width < 4 || self.is_empty() {
            return Vec::new();
        }

        let mut lines = Vec::new();
        if !self.pending_steers.is_empty() {
            push_header(
                &mut lines,
                width,
                "Messages to be submitted after next tool call",
                Some(&self.interrupt_binding),
            );
            for steer in &self.pending_steers {
                push_preview(&mut lines, steer, width, false);
            }
        }

        if !self.rejected_steers.is_empty() {
            separate(&mut lines);
            push_header(
                &mut lines,
                width,
                "Messages to be submitted at end of turn",
                None,
            );
            for steer in &self.rejected_steers {
                push_preview(&mut lines, steer, width, false);
            }
        }

        if !self.queued_messages.is_empty() {
            separate(&mut lines);
            push_header(&mut lines, width, "Queued follow-up inputs", None);
            for message in &self.queued_messages {
                push_preview(&mut lines, &message.text, width, true);
            }
            lines.push(Line::new(vec![
                Span::dim("    "),
                Span::dim(self.edit_binding.clone()),
                Span::dim(" edit last queued message"),
            ]));
        }
        lines
    }

    fn is_empty(&self) -> bool {
        self.pending_steers.is_empty()
            && self.rejected_steers.is_empty()
            && self.queued_messages.is_empty()
    }
}

fn separate(lines: &mut Vec<Line>) {
    if !lines.is_empty() {
        lines.push(Line::empty());
    }
}

fn push_header(
    lines: &mut Vec<Line>,
    width: usize,
    title: &str,
    interrupt_binding: Option<&str>,
) {
    let mut spans = vec![Span::dim("• "), Span::raw(title)];
    if let Some(binding) = interrupt_binding {
        spans.push(Span::dim(" (press "));
        spans.push(Span::dim(binding.to_string()));
        spans.push(Span::dim(" to interrupt and send immediately)"));
    }
    let mut wrapped = wrap_line(
        &Line::new(spans),
        width,
        &Span::dim("  "),
    );
    if let Some(binding) = interrupt_binding {
        isolate_hint_span(&mut wrapped, binding);
    }
    lines.extend(wrapped);
}

fn isolate_hint_span(lines: &mut [Line], hint: &str) {
    for line in lines {
        let Some((index, start)) = line
            .spans
            .iter()
            .enumerate()
            .find_map(|(index, span)| span.text.find(hint).map(|start| (index, start)))
        else {
            continue;
        };
        let span = line.spans.remove(index);
        let end = start + hint.len();
        let mut replacement = Vec::with_capacity(3);
        if start > 0 {
            replacement.push(Span::new(span.text[..start].to_string(), span.style));
        }
        replacement.push(Span::new(hint.to_string(), span.style));
        if end < span.text.len() {
            replacement.push(Span::new(span.text[end..].to_string(), span.style));
        }
        line.spans.splice(index..index, replacement);
        return;
    }
}

fn push_preview(lines: &mut Vec<Line>, text: &str, width: usize, italic: bool) {
    let body_style = if italic {
        Style::new().dim().italic()
    } else {
        Style::new().dim()
    };
    let mut wrapped = Vec::new();
    for (index, source_line) in text.lines().take(PREVIEW_LINE_LIMIT + 1).enumerate() {
        let prefix = if index == 0 {
            Span::dim("  ↳ ")
        } else {
            Span::raw("    ")
        };
        wrapped.extend(wrap_line(
            &Line::new(vec![prefix, Span::new(source_line.to_string(), body_style)]),
            width,
            &Span::raw("    "),
        ));
    }
    let overflow = wrapped.len() > PREVIEW_LINE_LIMIT;
    lines.extend(wrapped.into_iter().take(PREVIEW_LINE_LIMIT));
    if overflow {
        lines.push(Line::new(vec![Span::new("    …", body_style)]));
    }
}

#[cfg(test)]
mod tests {
    use super::{Line, PendingInputs};
    use crate::tui::composer::{Composer, Submission};

    fn plain(input: &PendingInputs, width: usize) -> Vec<String> {
        input.lines(width).iter().map(Line::plain).collect()
    }

    #[test]
    fn multiline_steer_is_limited_to_three_preview_lines() {
        let mut input = PendingInputs::default();
        input.push_steer("first\nsecond\nthird\nfourth".to_string());
        assert_eq!(
            plain(&input, 120),
            [
                "• Messages to be submitted after next tool call (press esc to interrupt and send immediately)",
                "  ↳ first",
                "    second",
                "    third",
                "    …",
            ]
        );
    }

    #[test]
    fn queued_message_is_italic_and_advertises_edit_binding() {
        let mut input = PendingInputs::default();
        input.push_queued(Submission {
            text: "draft".to_string(),
            image_paths: Vec::new(),
        });
        let lines = input.lines(80);
        assert_eq!(
            lines.iter().map(Line::plain).collect::<Vec<_>>(),
            [
                "• Queued follow-up inputs",
                "  ↳ draft",
                "    ⌥ + ↑ edit last queued message",
            ]
        );
        assert!(lines[1].spans.last().is_some_and(|span| span.style.italic));
    }

    #[test]
    fn pending_input_hints_render_configured_bindings_as_spans() {
        let mut input = PendingInputs::default();
        input.set_interrupt_binding("f12");
        input.set_edit_binding("ctrl+up");
        input.push_steer("now".to_string());
        input.push_queued(Submission {
            text: "later".to_string(),
            image_paths: Vec::new(),
        });

        let lines = input.lines(100);

        assert!(lines.iter().flat_map(|line| &line.spans).any(|span| span.text == "f12"));
        assert!(
            lines
                .iter()
                .flat_map(|line| &line.spans)
                .any(|span| span.text == "ctrl+up")
        );
    }

    #[test]
    fn alt_up_restores_the_latest_draft() {
        let mut input = PendingInputs::default();
        input.push_queued(Submission {
            text: "first".to_string(),
            image_paths: Vec::new(),
        });
        input.push_queued(Submission {
            text: "second".to_string(),
            image_paths: Vec::new(),
        });
        let mut composer = Composer::new();
        assert!(input.edit_latest_queued(&mut composer));
        assert_eq!(composer.text(), "second");
        assert_eq!(plain(&input, 80)[1], "  ↳ first");
    }
}
