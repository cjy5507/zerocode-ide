//! Compact observable activity shared by the TUI and external status roads.

use runtime::message_stream::ToolPreview;

const CARD_COMMAND_CHARS: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Activity {
    pub tool: String,
    pub target: Option<String>,
    pub full_target: Option<String>,
}

impl Activity {
    #[must_use]
    pub fn new(tool: &str, target: Option<&str>) -> Self {
        let tool = display_tool(tool);
        let full_target = target.and_then(clean_target);
        let target = full_target.as_deref().and_then(|full| compact_target(&tool, full));
        Self {
            tool,
            target,
            full_target,
        }
    }

    #[must_use]
    pub fn from_preview(announced_name: &str, preview: &ToolPreview) -> Self {
        match preview {
            ToolPreview::Bash { command } => Self::new("Bash", Some(command)),
            ToolPreview::Read { path, .. } => Self::new("Read", Some(path)),
            ToolPreview::Glob { pattern } => Self::new("Glob", Some(pattern)),
            ToolPreview::Grep { pattern, .. } => Self::new("Grep", Some(pattern)),
            ToolPreview::Write { path, .. } => Self::new("Write", Some(path)),
            ToolPreview::Edit { path, .. } => Self::new("Edit", Some(path)),
            ToolPreview::Search { query } => Self::new("WebSearch", Some(query)),
            ToolPreview::Generic {
                name,
                input_summary,
            } => Self::new(name, Some(input_summary)),
        }
        .with_fallback_tool(announced_name)
    }

    #[must_use]
    pub fn from_said(said: &str) -> Self {
        let (tool, target) = said
            .split_once(core_types::helper_run::FACT_SEPARATOR)
            .map_or((said, None), |(tool, target)| (tool, Some(target)));
        Self::new(tool, target)
    }

    fn with_fallback_tool(mut self, announced_name: &str) -> Self {
        if self.tool.is_empty() {
            self.tool = crate::util::ansi::sanitize_inline(announced_name.trim());
        }
        self
    }

    #[must_use]
    pub fn said(&self) -> String {
        match self.target.as_deref() {
            Some(target) => format!("{} · {target}", self.tool),
            None => self.tool.clone(),
        }
    }

    /// The fact a tool START puts on the wire.
    ///
    /// `session_status.activity` and the `PreToolUse` payload share this one
    /// shape; at a start the clock reads zero by definition. Later seconds
    /// are the channel's to count — the hook road never repeats a start.
    #[must_use]
    pub fn started_card(&self) -> crate::ide::channel::state::ActivityCard {
        crate::ide::channel::state::ActivityCard {
            verb: self.wire_verb(),
            target: self.card_target(),
            phase: super::strings::ACTIVITY_PHASE_STARTED.to_string(),
            elapsed_secs: 0,
        }
    }

    /// A command's whole text up to the card's character budget; else the compact target.
    #[must_use]
    pub fn card_target(&self) -> Option<String> {
        match self.full_target.as_deref() {
            Some(full) if self.tool.eq_ignore_ascii_case("bash") => {
                Some(full.chars().take(CARD_COMMAND_CHARS).collect())
            }
            _ => self.target.clone(),
        }
    }

    /// What the window's activity row is handed as the target: the compact
    /// target the Working line shows, or the announced summary when the
    /// preview named none — a row with a verb and no object says less than
    /// the cell under it.
    #[must_use]
    pub fn hook_input<'a>(&'a self, summary: &'a str) -> &'a str {
        self.target.as_deref().unwrap_or(summary)
    }

    #[must_use]
    pub fn wire_verb(&self) -> String {
        match self.tool.as_str() {
            "Read" => "read".to_string(),
            "Edit" => "edit".to_string(),
            "Write" => "write".to_string(),
            "Bash" => "bash".to_string(),
            "Grep" | "Glob" => "grep".to_string(),
            "WebSearch" => "websearch".to_string(),
            "Agent" | "SpawnMultiAgent" | "Workflow" | "Task" => "task".to_string(),
            other => other.to_string(),
        }
    }
}

/// The file a call names, as the model wrote it — what the window's file tree
/// resolves, which the compact target the working line shows (the last two
/// path components) cannot be. A read, a write and an edit name one; no other
/// call does.
#[must_use]
pub fn file_of(preview: &ToolPreview) -> Option<&str> {
    match preview {
        ToolPreview::Read { path, .. }
        | ToolPreview::Write { path, .. }
        | ToolPreview::Edit { path, .. } => Some(path.as_str()).filter(|path| !path.is_empty()),
        _ => None,
    }
}

fn display_tool(tool: &str) -> String {
    let clean = crate::util::ansi::sanitize_inline(tool.trim());
    match clean.to_ascii_lowercase().as_str() {
        "bash" => "Bash".to_string(),
        "read" | "read_file" => "Read".to_string(),
        "write" | "write_file" => "Write".to_string(),
        "edit" | "edit_file" => "Edit".to_string(),
        "grep" => "Grep".to_string(),
        "glob" => "Glob".to_string(),
        "websearch" | "web_search" => "WebSearch".to_string(),
        _ => clean,
    }
}

fn clean_target(target: &str) -> Option<String> {
    let cleaned = crate::util::ansi::sanitize_inline(target.trim());
    let trimmed = cleaned.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn compact_target(tool: &str, cleaned: &str) -> Option<String> {
    if tool.eq_ignore_ascii_case("bash") {
        return command_word_source(cleaned)
            .split_whitespace()
            .next()
            .map(str::to_string);
    }
    if matches!(
        tool.to_ascii_lowercase().as_str(),
        "read" | "write" | "edit" | "glob"
    ) {
        return Some(path_tail(cleaned));
    }
    Some(cleaned.to_string())
}

// The window's leading-cd rule, so the Working line names the word the window does.
fn command_word_source(command: &str) -> &str {
    let mut rest = command;
    while let Some(after) = cd_lead(rest) {
        if after.is_empty() {
            break;
        }
        rest = after;
    }
    rest
}

fn cd_lead(command: &str) -> Option<&str> {
    let after_cd = command.trim_start().strip_prefix("cd")?;
    if !after_cd.starts_with(char::is_whitespace) {
        return None;
    }
    let arg = after_cd.trim_start();
    let arg_len = match arg.chars().next()? {
        quote @ ('"' | '\'') => arg[1..].find(quote)? + 2,
        _ => unquoted_arg_len(arg)?,
    };
    let rest = arg[arg_len..].trim_start();
    let after_separator = rest.strip_prefix("&&").or_else(|| rest.strip_prefix(';'))?;
    Some(after_separator.trim_start())
}

fn unquoted_arg_len(arg: &str) -> Option<usize> {
    let mut len = 0;
    let mut chars = arg.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => len += ch.len_utf8() + chars.next()?.len_utf8(),
            other if other.is_whitespace() || matches!(other, ';' | '&' | '|' | '"' | '\'') => {
                break;
            }
            other => len += other.len_utf8(),
        }
    }
    (len > 0).then_some(len)
}

fn path_tail(path: &str) -> String {
    let mut parts = path
        .split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .rev();
    let Some(last) = parts.next() else {
        return path.to_string();
    };
    parts
        .next()
        .map_or_else(|| last.to_string(), |parent| format!("{parent}/{last}"))
}

#[cfg(test)]
mod tests {
    use super::Activity;
    use crate::tui::view::StatusActivity;
    use std::time::Duration;

    #[test]
    fn file_targets_keep_only_two_trailing_components() {
        assert_eq!(
            Activity::new("Read", Some("/repo/crates/zo-ide/src/tui/view.rs")).target,
            Some("tui/view.rs".to_string())
        );
    }

    #[test]
    fn commands_keep_only_the_first_word() {
        assert_eq!(
            Activity::new("Bash", Some("cargo test -p zo-ide")).target,
            Some("cargo".to_string())
        );
    }

    #[test]
    fn bash_target_skips_leading_cd_leads() {
        let target = |command: &str| Activity::new("Bash", Some(command)).target;
        assert_eq!(target("cd /a/b && make test"), Some("make".to_string()));
        assert_eq!(target("cd \"/a b\" ; cargo test"), Some("cargo".to_string()));
        assert_eq!(target("cd '/a b' && cd c;make"), Some("make".to_string()));
        assert_eq!(target("cd my\\ dir && make"), Some("make".to_string()));
        assert_eq!(target("cd /a/b"), Some("cd".to_string()));
        assert_eq!(target("cd /a/b &&"), Some("cd".to_string()));
    }

    #[test]
    fn hook_input_after_a_cd_lead_is_the_command_word() {
        let make = Activity::new("Bash", Some("cd /a/b && make test"));
        assert_eq!(make.hook_input("cd /a/b && make test"), "make");
        let codex = Activity::new("Bash", Some("cd /a/b && codex exec x"));
        assert_eq!(codex.hook_input("cd /a/b && codex exec x"), "codex");
    }

    #[test]
    fn bash_card_carries_the_whole_command() {
        let activity = Activity::new("Bash", Some("cd /a/b && cargo test -p zo-ide"));
        let card = activity.started_card();
        assert_eq!(card.verb, "bash");
        assert_eq!(card.target.as_deref(), Some("cd /a/b && cargo test -p zo-ide"));
    }

    #[test]
    fn bash_card_is_cut_to_200_characters_at_a_character_boundary() {
        let command = format!("echo {}", "가".repeat(300));
        let expected: String = command.chars().take(200).collect();
        assert_eq!(expected.chars().count(), 200);
        let activity = Activity::new("Bash", Some(command.as_str()));
        assert_eq!(activity.started_card().target.as_deref(), Some(expected.as_str()));
    }

    #[test]
    fn status_tool_target_is_the_whole_command_and_its_line_the_command_word() {
        let status = StatusActivity::Tool {
            activity: Activity::new("Bash", Some("cd /a/b && make test")),
            elapsed: Duration::ZERO,
        };
        assert_eq!(status.target().as_deref(), Some("cd /a/b && make test"));
        assert!(status.line().contains("make"));
        assert!(!status.line().contains("cd /a/b"));
    }

    #[test]
    fn websearch_goes_on_the_wire_as_websearch() {
        let search = Activity::new("WebSearch", Some("rust async"));
        assert_eq!(search.wire_verb(), "websearch");
        let snake = Activity::new("web_search", Some("rust async"));
        assert_eq!(snake.wire_verb(), "websearch");
    }

    #[test]
    fn web_fetch_and_unknown_tools_keep_their_names() {
        let fetch = Activity::new("WebFetch", Some("https://example.com"));
        assert_eq!(fetch.wire_verb(), "WebFetch");
        assert_eq!(Activity::new("mcp__docs__search", None).wire_verb(), "mcp__docs__search");
    }
}
