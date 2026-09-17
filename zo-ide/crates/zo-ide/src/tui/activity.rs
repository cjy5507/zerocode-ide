//! Compact observable activity shared by the TUI and external status roads.

use runtime::message_stream::ToolPreview;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Activity {
    pub tool: String,
    pub target: Option<String>,
}

impl Activity {
    #[must_use]
    pub fn new(tool: &str, target: Option<&str>) -> Self {
        let tool = display_tool(tool);
        let target = target.and_then(|target| compact_target(&tool, target));
        Self { tool, target }
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

    /// The compact fact a tool START puts on the wire.
    ///
    /// `session_status.activity` and the `PreToolUse` payload share this one
    /// shape; at a start the clock reads zero by definition. Later seconds
    /// are the channel's to count — the hook road never repeats a start.
    #[must_use]
    pub fn started_card(&self) -> crate::ide::channel::state::ActivityCard {
        crate::ide::channel::state::ActivityCard {
            verb: self.wire_verb(),
            target: self.target.clone(),
            phase: super::strings::ACTIVITY_PHASE_STARTED.to_string(),
            elapsed_secs: 0,
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
            "WebSearch" => "web".to_string(),
            "Agent" | "SpawnMultiAgent" | "Workflow" | "Task" => "task".to_string(),
            other => other.to_string(),
        }
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

fn compact_target(tool: &str, target: &str) -> Option<String> {
    let cleaned = crate::util::ansi::sanitize_inline(target.trim());
    if cleaned.is_empty() {
        return None;
    }
    if tool.eq_ignore_ascii_case("bash") {
        return cleaned.split_whitespace().next().map(str::to_string);
    }
    if matches!(
        tool.to_ascii_lowercase().as_str(),
        "read" | "write" | "edit" | "glob"
    ) {
        return Some(path_tail(&cleaned));
    }
    Some(cleaned)
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
}
