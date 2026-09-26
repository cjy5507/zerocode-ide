//! Session-scoped in-process agent overview.
//!
//! This module owns only picker material and its tiny navigation state. The
//! shared [`Picker`] and freeform
//! [`Question`] remain the one renderer for model,
//! resume, permissions, questions, and agents; no second overlay grammar is
//! introduced here.

use crate::session::subagent_progress::SubagentProgress;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use runtime::PermissionMode;

use super::ansi::Style;
use super::composer::Composer;
use super::shimmer::fmt_elapsed_compact;
use super::view::{Picker, PickerRow, Question, Tip};

const OUTPUT_LINES: usize = 24;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Stage {
    List,
    Detail(String),
    Message(String),
    Stop(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Action {
    None,
    Close,
    Send { agent_id: String },
    Stop { agent_id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NoticeLevel {
    Info,
    Warn,
    Error,
}

pub(crate) struct Effect {
    pub(crate) close: bool,
    pub(crate) remove_agent: Option<String>,
    pub(crate) notice: Option<(NoticeLevel, String)>,
}

impl Effect {
    fn none() -> Self {
        Self {
            close: false,
            remove_agent: None,
            notice: None,
        }
    }
}

/// One open Alt+A overview. Snapshots are owned values refreshed by the
/// existing asynchronous watcher while a turn runs.
pub(crate) struct Overview {
    agents: Vec<SubagentProgress>,
    stage: Stage,
    view: Picker,
    message: Option<Question>,
    composer: Composer,
    /// The session's agent registry: the store every stop and send below
    /// resolves the picked id in (t-2511).
    registry: std::sync::Arc<tools::AgentRegistry>,
}

impl Overview {
    pub(crate) fn new(
        agents: Vec<SubagentProgress>,
        registry: std::sync::Arc<tools::AgentRegistry>,
    ) -> Self {
        let view = list_view(&agents, 0);
        Self {
            agents,
            stage: Stage::List,
            view,
            message: None,
            composer: Composer::new(),
            registry,
        }
    }

    pub(crate) fn picker(&self) -> Option<&Picker> {
        (!matches!(self.stage, Stage::Message(_))).then_some(&self.view)
    }

    pub(crate) fn question(&self) -> Option<&Question> {
        self.message.as_ref()
    }

    pub(crate) fn composer(&self) -> Option<&Composer> {
        self.is_message().then_some(&self.composer)
    }

    fn is_message(&self) -> bool {
        matches!(self.stage, Stage::Message(_))
    }

    /// Paste into the message composer, returning whether the overview owned
    /// the event. Agent messages are deliberately one line and never turn a
    /// pasted image path into an attachment.
    pub(crate) fn paste(&mut self, text: &str) -> bool {
        if !self.is_message() {
            return false;
        }
        self.composer.insert_str(&one_line(text));
        true
    }

    pub(crate) fn key(
        &mut self,
        key: KeyEvent,
        session_id: &str,
        permission_mode: PermissionMode,
    ) -> Effect {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        if is_open_key(&key) {
            return Effect {
                close: true,
                ..Effect::none()
            };
        }
        if self.is_message() {
            return self.message_key(key, session_id, permission_mode);
        }

        let mut action = Action::None;
        match key.code {
            KeyCode::Up | KeyCode::Char('k') if !control && !alt => self.up(),
            KeyCode::Down | KeyCode::Char('j') if !control && !alt => self.down(),
            KeyCode::Char(ch) if ch.is_ascii_digit() && !control && !alt => {
                let digit = ch.to_digit(10).unwrap_or(0) as usize;
                self.select_number(digit);
            }
            KeyCode::Enter => action = self.enter(),
            KeyCode::Char('m') if !control && !alt => self.begin_message(),
            KeyCode::Char('s') if !control && !alt => self.begin_stop(),
            KeyCode::Esc => {
                if self.back() {
                    action = Action::Close;
                }
            }
            KeyCode::Char('c') if control => {
                if self.back() {
                    action = Action::Close;
                }
            }
            _ => {}
        }
        self.handle_action(action, session_id)
    }

    fn message_key(
        &mut self,
        key: KeyEvent,
        session_id: &str,
        permission_mode: PermissionMode,
    ) -> Effect {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Esc => {
                self.back();
                Effect::none()
            }
            KeyCode::Char('c') if control => {
                self.back();
                Effect::none()
            }
            KeyCode::Enter => self.send_message(session_id, permission_mode),
            KeyCode::Char('u') if control => {
                self.composer.kill_to_start();
                Effect::none()
            }
            KeyCode::Char('k') if control => {
                self.composer.kill_to_end();
                Effect::none()
            }
            KeyCode::Char('w') if control => {
                self.composer.kill_word_left();
                Effect::none()
            }
            KeyCode::Char('a') if control => {
                self.composer.home();
                Effect::none()
            }
            KeyCode::Char('e') if control => {
                self.composer.end();
                Effect::none()
            }
            KeyCode::Char(ch) if !control && !alt => {
                self.composer.insert_char(ch);
                Effect::none()
            }
            KeyCode::Backspace => {
                self.composer.backspace();
                Effect::none()
            }
            KeyCode::Delete => {
                self.composer.delete();
                Effect::none()
            }
            KeyCode::Left if control => {
                self.composer.word_left();
                Effect::none()
            }
            KeyCode::Right if control => {
                self.composer.word_right();
                Effect::none()
            }
            KeyCode::Left => {
                self.composer.left();
                Effect::none()
            }
            KeyCode::Right => {
                self.composer.right();
                Effect::none()
            }
            KeyCode::Home => {
                self.composer.home();
                Effect::none()
            }
            KeyCode::End => {
                self.composer.end();
                Effect::none()
            }
            KeyCode::Up => {
                self.composer.history_prev();
                Effect::none()
            }
            KeyCode::Down => {
                self.composer.history_next();
                Effect::none()
            }
            _ => Effect::none(),
        }
    }

    fn send_message(&mut self, session_id: &str, permission_mode: PermissionMode) -> Effect {
        let message = one_line(&self.composer.submit().text);
        if message.is_empty() {
            return Effect::none();
        }
        let Stage::Message(agent_id) = self.stage.clone() else {
            return Effect::none();
        };
        let outcome = tools::send_agent_message_for_session(
            &self.registry,
            &agent_id,
            &message,
            session_id,
            Some(permission_mode),
        );
        self.return_to_detail(&agent_id);
        let (level, text) = match outcome {
            tools::AgentSendOutcome::Steered { name } => (
                NoticeLevel::Info,
                format!("message sent to {name} · reply arrives with its result"),
            ),
            tools::AgentSendOutcome::Resumed { name } => (
                NoticeLevel::Info,
                format!("{name} finished before delivery · resumed with its context"),
            ),
            tools::AgentSendOutcome::NotFound => (
                NoticeLevel::Warn,
                "agent is no longer available".to_string(),
            ),
            tools::AgentSendOutcome::NotOwned { name } => (
                NoticeLevel::Error,
                format!("refused to message {name}: it is not owned by this session"),
            ),
            tools::AgentSendOutcome::Unreachable { name } => (
                NoticeLevel::Warn,
                format!("{name} is running but its steering queue is unreachable"),
            ),
            tools::AgentSendOutcome::Failed { name, error } => (
                NoticeLevel::Error,
                format!("could not message {name}: {error}"),
            ),
        };
        Effect {
            notice: Some((level, text)),
            ..Effect::none()
        }
    }

    fn handle_action(&mut self, action: Action, session_id: &str) -> Effect {
        match action {
            Action::None | Action::Send { .. } => Effect::none(),
            Action::Close => Effect {
                close: true,
                ..Effect::none()
            },
            Action::Stop { agent_id } => {
                let outcome = tools::stop_agent_for_session(
                    &self.registry,
                    &agent_id,
                    session_id,
                    "stopped by user from agents overview",
                );
                let (level, text, removed) = match outcome {
                    tools::AgentStopOutcome::Stopped { name } => {
                        (NoticeLevel::Info, format!("stopped {name}"), true)
                    }
                    // An idle pane teammate took the door over its channel.
                    tools::AgentStopOutcome::Closed { name } => {
                        (NoticeLevel::Info, format!("closed {name}"), true)
                    }
                    tools::AgentStopOutcome::AlreadyFinished { name, status } => (
                        NoticeLevel::Info,
                        format!("{name} already finished ({status})"),
                        true,
                    ),
                    tools::AgentStopOutcome::NotFound => (
                        NoticeLevel::Warn,
                        "agent is no longer available".to_string(),
                        true,
                    ),
                    tools::AgentStopOutcome::NotOwned { name } => (
                        NoticeLevel::Error,
                        format!("refused to stop {name}: ownership could not be proven"),
                        false,
                    ),
                    tools::AgentStopOutcome::Unreachable { name } => (
                        NoticeLevel::Warn,
                        format!("refused to stop {name}: its live generation is unreachable"),
                        false,
                    ),
                    tools::AgentStopOutcome::Failed { name, error } => (
                        NoticeLevel::Error,
                        format!("could not stop {name}: {error}"),
                        false,
                    ),
                };
                if removed {
                    self.agents.retain(|agent| agent.agent_id != agent_id);
                    self.show_list();
                }
                Effect {
                    remove_agent: removed.then_some(agent_id),
                    notice: Some((level, text)),
                    ..Effect::none()
                }
            }
        }
    }

    pub(crate) fn refresh(&mut self, agents: Vec<SubagentProgress>) {
        let selected_id = match &self.stage {
            Stage::List => self
                .agents
                .get(self.view.selected)
                .map(|agent| agent.agent_id.clone()),
            Stage::Detail(id) | Stage::Message(id) | Stage::Stop(id) => Some(id.clone()),
        };
        self.agents = agents;
        match self.stage.clone() {
            Stage::List => {
                let selected = selected_id
                    .as_deref()
                    .and_then(|id| self.index_of(id))
                    .unwrap_or(0);
                self.view = list_view(&self.agents, selected);
            }
            Stage::Detail(id) => {
                if let Some(agent) = self.agent(&id) {
                    self.view = detail_view(agent, self.view.selected);
                } else {
                    self.show_list();
                }
            }
            Stage::Stop(id) => {
                if self.agent(&id).is_none() {
                    self.show_list();
                }
            }
            // A message may race completion. Keep the target: the engine's
            // existing SendMessage contract resumes a just-finished agent with
            // its transcript intact.
            Stage::Message(_) => {}
        }
    }

    fn up(&mut self) {
        self.view.up();
    }

    fn down(&mut self) {
        self.view.down();
    }

    fn select_number(&mut self, digit: usize) -> bool {
        self.view.select_number(digit)
    }

    fn enter(&mut self) -> Action {
        match self.stage.clone() {
            Stage::List => {
                let Some(agent) = self.agents.get(self.view.selected) else {
                    return Action::None;
                };
                let id = agent.agent_id.clone();
                self.view = detail_view(agent, 0);
                self.stage = Stage::Detail(id);
                Action::None
            }
            Stage::Detail(_) => Action::None,
            Stage::Message(agent_id) => Action::Send { agent_id },
            Stage::Stop(agent_id) => {
                if self.view.selected == 1 {
                    self.return_to_detail(&agent_id);
                    Action::Stop { agent_id }
                } else {
                    self.return_to_detail(&agent_id);
                    Action::None
                }
            }
        }
    }

    fn begin_message(&mut self) {
        let Stage::Detail(agent_id) = self.stage.clone() else {
            return;
        };
        let Some(agent) = self.agent(&agent_id) else {
            self.show_list();
            return;
        };
        let label = agent.label.clone();
        self.composer.clear();
        self.message = Some(message_question(&label));
        self.stage = Stage::Message(agent_id);
    }

    fn begin_stop(&mut self) {
        let Stage::Detail(agent_id) = self.stage.clone() else {
            return;
        };
        let Some(agent) = self.agent(&agent_id) else {
            self.show_list();
            return;
        };
        self.view = stop_view(&agent.label);
        self.stage = Stage::Stop(agent_id);
    }

    /// Move one level back. Returns `true` when the whole overview should close.
    fn back(&mut self) -> bool {
        match self.stage.clone() {
            Stage::List => true,
            Stage::Detail(_) => {
                self.show_list();
                false
            }
            Stage::Message(agent_id) | Stage::Stop(agent_id) => {
                self.return_to_detail(&agent_id);
                false
            }
        }
    }

    fn return_to_detail(&mut self, agent_id: &str) {
        self.composer.clear();
        self.message = None;
        if let Some(agent) = self.agent(agent_id) {
            self.view = detail_view(agent, 0);
            self.stage = Stage::Detail(agent_id.to_string());
        } else {
            self.show_list();
        }
    }

    fn show_list(&mut self) {
        self.composer.clear();
        self.message = None;
        self.stage = Stage::List;
        self.view = list_view(&self.agents, 0);
    }

    fn agent(&self, id: &str) -> Option<&SubagentProgress> {
        self.agents.iter().find(|agent| agent.agent_id == id)
    }

    fn index_of(&self, id: &str) -> Option<usize> {
        self.agents.iter().position(|agent| agent.agent_id == id)
    }
}

/// How the footers and the `?` card write the `is_open_key` chord.
pub const OPEN_KEY_LABEL: &str = "alt+a";

/// Codex's fixed Open Agents binding (`alt+a`). Control is explicitly excluded
/// so a combined chord cannot steal Ctrl+A from composer home navigation.
pub(crate) fn is_open_key(key: &KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::ALT)
        && !key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char(ch) if ch.eq_ignore_ascii_case(&'a'))
}

fn list_view(agents: &[SubagentProgress], selected: usize) -> Picker {
    Picker {
        title: "Running agents".to_string(),
        title_style: Style::new().bold(),
        note: if agents.is_empty() {
            "No running agents in this session.".to_string()
        } else {
            format!("{} running in this session", agents.len())
        },
        rows: agents
            .iter()
            .map(|agent| PickerRow {
                label: clean(&agent.label),
                description: format!(
                    "{} · {}",
                    clean(&agent.activity),
                    fmt_elapsed_compact(agent.elapsed.as_secs())
                ),
                dim: false,
            })
            .collect(),
        selected: selected.min(agents.len().saturating_sub(1)),
        footer: format!("enter details · {OPEN_KEY_LABEL}/esc close"),
    }
}

fn detail_view(agent: &SubagentProgress, selected: usize) -> Picker {
    let mut rows = vec![PickerRow {
        label: "activity".to_string(),
        description: clean(&agent.activity),
        dim: false,
    }];
    // What the last message to this helper came to (t-2513 §2.3) — the same
    // word the `SendMessage` result and the `subagents` frame carry.
    if let Some(record) = agent.last_receipt.as_ref() {
        rows.push(PickerRow {
            label: "message".to_string(),
            description: match record.reason.as_deref() {
                Some(reason) => format!("{} · {}", record.receipt.as_str(), clean(reason)),
                None => record.receipt.as_str().to_string(),
            },
            dim: false,
        });
    }
    rows.extend(agent.recent_tools.iter().map(|tool| PickerRow {
        label: "tool".to_string(),
        description: clean(tool),
        dim: false,
    }));
    let output: Vec<&str> = agent.output_tail.lines().collect();
    if output.is_empty() {
        rows.push(PickerRow {
            label: "output".to_string(),
            description: "No streamed text yet.".to_string(),
            dim: false,
        });
    } else {
        let start = output.len().saturating_sub(OUTPUT_LINES);
        rows.extend(output[start..].iter().enumerate().map(|(index, line)| {
            PickerRow {
                label: if index == 0 { "output" } else { "" }.to_string(),
                description: clean(line),
                dim: false,
            }
        }));
    }
    let model = agent
        .model
        .as_deref()
        .map_or_else(|| "inherited model".to_string(), clean);
    Picker {
        title: clean(&agent.label),
        title_style: Style::new().bold(),
        note: format!(
            "{model} · {} · {}",
            fmt_elapsed_compact(agent.elapsed.as_secs()),
            clean(&agent.agent_id)
        ),
        selected: selected.min(rows.len().saturating_sub(1)),
        rows,
        footer: "m message · s stop · up/down scroll · esc agents".to_string(),
    }
}

fn stop_view(label: &str) -> Picker {
    Picker {
        title: format!("Stop {}?", clean(label)),
        title_style: Style::new().bold(),
        note: "Only a live agent owned by this session and zo process can be stopped.".to_string(),
        rows: vec![
            PickerRow {
                label: "Keep running".to_string(),
                description: "Return without changing the agent.".to_string(),
                dim: false,
            },
            PickerRow {
                label: "Stop agent".to_string(),
                description: "Cooperatively cancel this agent only.".to_string(),
                dim: false,
            },
        ],
        selected: 0,
        footer: "enter confirm · esc details".to_string(),
    }
}

fn message_question(label: &str) -> Question {
    Question {
        progress: format!("Message to {}", clean(label)),
        question: "Send one line to this agent's current context.".to_string(),
        rows: Vec::new(),
        selected: 0,
        checked: Vec::new(),
        other_index: None,
        other_input: false,
        tips: vec![
            Tip::highlighted("enter to send"),
            Tip::new("esc to return"),
        ],
    }
}

fn clean(text: &str) -> String {
    crate::util::ansi::sanitize_inline(text)
}

/// Collapse pasted/newline-rich text to the viewer's promised one-line send.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    fn test_registry() -> std::sync::Arc<tools::AgentRegistry> {
        tools::AgentRegistry::at_root_for_tests("session-test", std::path::Path::new("/tmp"))
    }

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use runtime::PermissionMode;

    use super::{detail_view, list_view, one_line, stop_view, Action, Overview};
    use crate::session::subagent_progress::SubagentProgress;

    fn agent(id: &str, label: &str) -> SubagentProgress {
        SubagentProgress {
            agent_id: id.to_string(),
            tool_call_id: None,
            label: label.to_string(),
            model: Some("gpt-5.6-sol".to_string()),
            activity: "read_file · src/main.rs".to_string(),
            recent_tools: vec!["grep_search · auth".to_string()],
            tool_calls: 0,
            output_tail: "first finding\nsecond finding".to_string(),
            started_epoch: 1_000,
            elapsed: Duration::from_secs(65),
            no_new_output_for: None,
            transcript_path: None,
            pane: None,
            last_receipt: None,
        }
    }

    #[test]
    fn list_opens_detail_and_reuses_picker_rows_for_live_output() {
        let mut overview = Overview::new(vec![agent("agent-1", "scout")], test_registry());
        assert_eq!(overview.picker().expect("list").title, "Running agents");
        assert_eq!(overview.enter(), Action::None);
        let detail = overview.picker().expect("detail");
        assert_eq!(detail.title, "scout");
        assert!(detail.rows.iter().any(|row| row.label == "activity"));
        assert!(detail.rows.iter().any(|row| row.description == "second finding"));
    }

    #[test]
    fn message_and_stop_are_explicit_detail_actions() {
        let mut overview = Overview::new(vec![agent("agent-1", "scout")], test_registry());
        overview.enter();
        overview.begin_message();
        assert!(overview.picker().is_none());
        assert!(overview.question().is_some());
        assert_eq!(
            overview.enter(),
            Action::Send {
                agent_id: "agent-1".to_string(),
            }
        );
        overview.return_to_detail("agent-1");
        overview.begin_stop();
        assert_eq!(overview.enter(), Action::None, "safe default keeps running");
        overview.begin_stop();
        overview.down();
        assert_eq!(
            overview.enter(),
            Action::Stop {
                agent_id: "agent-1".to_string(),
            }
        );
    }

    #[test]
    fn human_message_is_one_line() {
        assert_eq!(one_line("  focus here\nthen answer  "), "focus here then answer");
    }

    #[test]
    fn picker_titles_are_bold_without_a_cyan_foreground() {
        let progress = agent("agent-1", "scout");
        let styles = [
            list_view(std::slice::from_ref(&progress), 0).title_style,
            detail_view(&progress, 0).title_style,
            stop_view("scout").title_style,
        ];

        assert!(styles.iter().all(|style| style.bold));
        assert!(styles.iter().all(|style| style.fg.is_none()));
    }

    #[test]
    fn keyboard_path_opens_detail_and_borrows_its_own_message_composer() {
        let mut overview = Overview::new(vec![agent("agent-1", "scout")], test_registry());
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);

        overview.key(
            key(KeyCode::Enter),
            "session-a",
            PermissionMode::WorkspaceWrite,
        );
        overview.key(
            key(KeyCode::Char('m')),
            "session-a",
            PermissionMode::WorkspaceWrite,
        );
        overview.key(
            key(KeyCode::Char('안')),
            "session-a",
            PermissionMode::WorkspaceWrite,
        );
        assert_eq!(overview.composer().expect("message composer").text(), "안");

        overview.key(
            key(KeyCode::Esc),
            "session-a",
            PermissionMode::WorkspaceWrite,
        );
        assert!(overview.composer().is_none());
        assert_eq!(overview.picker().expect("back at detail").title, "scout");
    }
}
