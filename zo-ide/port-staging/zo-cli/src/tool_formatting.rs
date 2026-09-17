const DISPLAY_TRUNCATION_NOTICE: &str =
    "\x1b[2m… output truncated for display; full result preserved in session.\x1b[0m";
const READ_DISPLAY_MAX_LINES: usize = 80;
const READ_DISPLAY_MAX_CHARS: usize = 6_000;
const TOOL_OUTPUT_DISPLAY_MAX_LINES: usize = 60;
const TOOL_OUTPUT_DISPLAY_MAX_CHARS: usize = 4_000;

pub(crate) fn format_tool_call_start(name: &str, input: &str) -> String {
    let parsed: serde_json::Value =
        serde_json::from_str(input).unwrap_or(serde_json::Value::String(input.to_string()));

    let detail = match name {
        "bash" | "Bash" => format_bash_call(&parsed),
        "read_file" | "Read" => {
            let path = extract_tool_path(&parsed);
            format!("\x1b[2m📄 Reading {path}…\x1b[0m")
        }
        "write_file" | "Write" => {
            let path = extract_tool_path(&parsed);
            let lines = parsed
                .get("content")
                .and_then(|value| value.as_str())
                .map_or(0, |content| content.lines().count());
            format!("\x1b[1;32m✏️ Writing {path}\x1b[0m \x1b[2m({lines} lines)\x1b[0m")
        }
        "edit_file" | "Edit" => {
            let path = extract_tool_path(&parsed);
            let old_value = parsed
                .get("old_string")
                .or_else(|| parsed.get("oldString"))
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            let new_value = parsed
                .get("new_string")
                .or_else(|| parsed.get("newString"))
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            format!(
                "\x1b[1;33m📝 Editing {path}\x1b[0m{}",
                format_patch_preview(old_value, new_value)
                    .map(|preview| format!("\n{preview}"))
                    .unwrap_or_default()
            )
        }
        "glob_search" | "Glob" => format_search_start("🔎 Glob", &parsed),
        "grep_search" | "Grep" => format_search_start("🔎 Grep", &parsed),
        "web_search" | "WebSearch" => parsed
            .get("query")
            .and_then(|value| value.as_str())
            .unwrap_or("?")
            .to_string(),
        _ => summarize_tool_payload(input),
    };

    tool_card(name, &detail)
}

/// The bordered card frame shared by both tool-call-start formatters.
fn tool_card(name: &str, detail: &str) -> String {
    let border = "─".repeat(name.len() + 8);
    format!(
        "\x1b[38;5;245m╭─ \x1b[1;36m{name}\x1b[0;38;5;245m ─╮\x1b[0m\n\x1b[38;5;245m│\x1b[0m {detail}\n\x1b[38;5;245m╰{border}╯\x1b[0m"
    )
}

/// [`format_tool_call_start`] for the streaming block contract, which carries
/// a structured [`ToolPreview`](runtime::message_stream::ToolPreview) instead
/// of the raw input JSON (render blocks deliberately never ship full tool
/// payloads). Same card frame and per-tool detail shapes; where the preview
/// summarizes what the raw input carried verbatim, the summarized form
/// renders instead (write shows a byte count, not a line count; edit shows a
/// hunk count, not a patch preview — the applied diff still prints with the
/// tool result).
pub(crate) fn format_tool_call_start_from_preview(
    name: &str,
    preview: &runtime::message_stream::ToolPreview,
    summary: &str,
) -> String {
    use runtime::message_stream::ToolPreview;
    let detail = match preview {
        ToolPreview::Bash { command } => {
            if command.is_empty() {
                String::new()
            } else {
                format!(
                    "\x1b[48;5;236;38;5;255m $ {} \x1b[0m",
                    truncate_for_summary(command, 160)
                )
            }
        }
        ToolPreview::Read { path, .. } => format!("\x1b[2m📄 Reading {path}…\x1b[0m"),
        ToolPreview::Write { path, byte_count } => format!(
            "\x1b[1;32m✏️ Writing {path}\x1b[0m \x1b[2m({byte_count} bytes)\x1b[0m"
        ),
        ToolPreview::Edit { path, hunk_count } => format!(
            "\x1b[1;33m📝 Editing {path}\x1b[0m \x1b[2m({hunk_count} hunk{})\x1b[0m",
            if *hunk_count == 1 { "" } else { "s" }
        ),
        ToolPreview::Glob { pattern } => format!("🔎 Glob {pattern}"),
        ToolPreview::Grep { pattern, path } => format!(
            "🔎 Grep {pattern}\n\x1b[2min {}\x1b[0m",
            path.as_deref().unwrap_or(".")
        ),
        ToolPreview::Search { query } => query.clone(),
        ToolPreview::Generic { input_summary, .. } => {
            if input_summary.is_empty() {
                summary.to_string()
            } else {
                input_summary.clone()
            }
        }
    };
    tool_card(name, &detail)
}

pub(crate) fn format_tool_result(name: &str, output: &str, is_error: bool) -> String {
    let icon = if is_error {
        "\x1b[1;31m✗\x1b[0m"
    } else {
        "\x1b[1;32m✓\x1b[0m"
    };
    if is_error {
        let summary = truncate_for_summary(output.trim(), 160);
        return if summary.is_empty() {
            format!("{icon} \x1b[38;5;245m{name}\x1b[0m")
        } else {
            format!("{icon} \x1b[38;5;245m{name}\x1b[0m\n\x1b[38;5;203m{summary}\x1b[0m")
        };
    }

    let parsed: serde_json::Value =
        serde_json::from_str(output).unwrap_or(serde_json::Value::String(output.to_string()));
    match name {
        "bash" | "Bash" => format_bash_result(icon, &parsed),
        "read_file" | "Read" => format_read_result(icon, &parsed),
        "write_file" | "Write" => format_write_result(icon, &parsed),
        "edit_file" | "Edit" => format_edit_result(icon, &parsed),
        "glob_search" | "Glob" => format_glob_result(icon, &parsed),
        "grep_search" | "Grep" => format_grep_result(icon, &parsed),
        "TodoWrite" => format_todo_result(icon, &parsed),
        "Skill" => format_skill_result(icon, &parsed),
        _ => format_generic_tool_result(icon, name, &parsed),
    }
}

/// [`format_tool_result`] for the streaming block contract, which carries a
/// structured [`ToolResultBody`](runtime::message_stream::ToolResultBody)
/// instead of the raw output JSON. Same ✓/✗ visual language and display
/// truncation limits as the raw formatter; where the body summarizes what the
/// raw JSON carried verbatim (read results lose their line-range header, bash
/// loses the return-code interpretation string), the summarized form renders
/// instead.
pub(crate) fn format_tool_result_from_body(
    name: &str,
    is_error: bool,
    body: &runtime::message_stream::ToolResultBody,
) -> String {
    use runtime::message_stream::{DiffLineKind, ToolResultBody};

    let icon = if is_error {
        "\x1b[1;31m✗\x1b[0m"
    } else {
        "\x1b[1;32m✓\x1b[0m"
    };
    if is_error {
        let content = match body {
            ToolResultBody::Text { content, .. } | ToolResultBody::Generic { content, .. } => {
                content.as_str()
            }
            ToolResultBody::Bash(result) => result.stderr.as_str(),
            _ => "",
        };
        let summary = truncate_for_summary(content.trim(), 160);
        return if summary.is_empty() {
            format!("{icon} \x1b[38;5;245m{name}\x1b[0m")
        } else {
            format!("{icon} \x1b[38;5;245m{name}\x1b[0m\n\x1b[38;5;203m{summary}\x1b[0m")
        };
    }

    match body {
        ToolResultBody::Bash(result) => format_bash_body(icon, name, result),
        ToolResultBody::Read { path, content, .. } => format!(
            "{icon} \x1b[2m📄 Read {path}\x1b[0m\n{}",
            truncate_output_for_display(content, READ_DISPLAY_MAX_LINES, READ_DISPLAY_MAX_CHARS)
        ),
        ToolResultBody::Diff(diff) => {
            let path = diff
                .new_path
                .as_deref()
                .or(diff.old_path.as_deref())
                .unwrap_or("?");
            let mut preview = Vec::new();
            for hunk in diff.hunks.iter().take(2) {
                for line in hunk.lines.iter().take(6) {
                    match line.kind {
                        DiffLineKind::Added => {
                            preview.push(format!("\x1b[38;5;70m+{}\x1b[0m", line.text));
                        }
                        DiffLineKind::Removed => {
                            preview.push(format!("\x1b[38;5;203m-{}\x1b[0m", line.text));
                        }
                        DiffLineKind::Context => preview.push(format!(" {}", line.text)),
                    }
                }
            }
            if preview.is_empty() {
                format!("{icon} \x1b[1;33m📝 Edited {path}\x1b[0m")
            } else {
                format!(
                    "{icon} \x1b[1;33m📝 Edited {path}\x1b[0m\n{}",
                    preview.join("\n")
                )
            }
        }
        ToolResultBody::Listing { entries, truncated } => {
            let shown = entries
                .iter()
                .take(8)
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join("\n");
            let count_note = if *truncated {
                format!("{}+ entries", entries.len())
            } else {
                format!("{} entries", entries.len())
            };
            if shown.is_empty() {
                format!("{icon} \x1b[38;5;245m{name}\x1b[0m {count_note}")
            } else {
                format!("{icon} \x1b[38;5;245m{name}\x1b[0m {count_note}\n{shown}")
            }
        }
        ToolResultBody::Todos(items) => format_todos_body(icon, name, items),
        ToolResultBody::Text { content, .. } | ToolResultBody::Generic { content, .. } => {
            let preview = truncate_output_for_display(
                content,
                TOOL_OUTPUT_DISPLAY_MAX_LINES,
                TOOL_OUTPUT_DISPLAY_MAX_CHARS,
            );
            if preview.is_empty() {
                format!("{icon} \x1b[38;5;245m{name}\x1b[0m")
            } else if preview.contains('\n') {
                format!("{icon} \x1b[38;5;245m{name}\x1b[0m\n{preview}")
            } else {
                format!("{icon} \x1b[38;5;245m{name}:\x1b[0m {preview}")
            }
        }
    }
}

fn format_bash_body(
    icon: &str,
    name: &str,
    result: &runtime::message_stream::BashResult,
) -> String {
    let mut lines = vec![if result.exit_code == 0 {
        format!("{icon} \x1b[38;5;245m{name}\x1b[0m")
    } else {
        format!(
            "{icon} \x1b[38;5;245m{name}\x1b[0m \x1b[38;5;203m(exit {})\x1b[0m",
            result.exit_code
        )
    }];
    if !result.stdout.trim().is_empty() {
        lines.push(truncate_output_for_display(
            &result.stdout,
            TOOL_OUTPUT_DISPLAY_MAX_LINES,
            TOOL_OUTPUT_DISPLAY_MAX_CHARS,
        ));
    }
    if !result.stderr.trim().is_empty() {
        lines.push(format!(
            "\x1b[38;5;203m{}\x1b[0m",
            truncate_output_for_display(
                &result.stderr,
                TOOL_OUTPUT_DISPLAY_MAX_LINES,
                TOOL_OUTPUT_DISPLAY_MAX_CHARS,
            )
        ));
    }
    lines.join("\n")
}

fn format_todos_body(
    icon: &str,
    name: &str,
    items: &[runtime::message_stream::TodoResultItem],
) -> String {
    const MAX_ITEMS: usize = 10;
    let mut lines = vec![format!(
        "{icon} \x1b[38;5;245m{name}\x1b[0m \x1b[2m({} todos)\x1b[0m",
        items.len()
    )];
    for item in items.iter().take(MAX_ITEMS) {
        let marker = match item.status {
            runtime::message_stream::TodoResultStatus::Completed => "\x1b[1;32m☑\x1b[0m",
            runtime::message_stream::TodoResultStatus::InProgress => "\x1b[1;33m◐\x1b[0m",
            runtime::message_stream::TodoResultStatus::Pending => "☐",
        };
        lines.push(format!(
            "  {marker} {}",
            truncate_for_summary(&item.content, 72)
        ));
    }
    if items.len() > MAX_ITEMS {
        lines.push(format!("  \x1b[2m… {} more\x1b[0m", items.len() - MAX_ITEMS));
    }
    lines.join("\n")
}

/// Compact checklist for a todo update — the raw ledger JSON (a screenful per
/// call) reads as noise in a terminal transcript.
fn format_todo_result(icon: &str, parsed: &serde_json::Value) -> String {
    const MAX_ITEMS: usize = 10;
    let Some(todos) = parsed.get("newTodos").and_then(|value| value.as_array()) else {
        return format_generic_tool_result(icon, "TodoWrite", parsed);
    };
    let mut lines = vec![format!(
        "{icon} \x1b[38;5;245mTodoWrite\x1b[0m \x1b[2m({} todos)\x1b[0m",
        todos.len()
    )];
    for todo in todos.iter().take(MAX_ITEMS) {
        let status = todo.get("status").and_then(|value| value.as_str());
        let marker = match status {
            Some("completed") => "\x1b[1;32m☑\x1b[0m",
            Some("in_progress") => "\x1b[1;33m◐\x1b[0m",
            _ => "☐",
        };
        let content = todo
            .get("content")
            .and_then(|value| value.as_str())
            .unwrap_or("?");
        lines.push(format!("  {marker} {}", truncate_for_summary(content, 72)));
    }
    if todos.len() > MAX_ITEMS {
        lines.push(format!("  \x1b[2m… {} more\x1b[0m", todos.len() - MAX_ITEMS));
    }
    lines.join("\n")
}

/// One-line summary for a loaded skill — its result carries the whole SKILL.md
/// body, which belongs in the model's context, not the user's transcript.
fn format_skill_result(icon: &str, parsed: &serde_json::Value) -> String {
    let Some(skill) = parsed.get("skill").and_then(|value| value.as_str()) else {
        return format_generic_tool_result(icon, "Skill", parsed);
    };
    let body_lines = parsed
        .get("prompt")
        .and_then(|value| value.as_str())
        .map_or(0, |prompt| prompt.lines().count());
    let description = parsed
        .get("description")
        .and_then(|value| value.as_str())
        .map(|text| format!("\n\x1b[2m{}\x1b[0m", truncate_for_summary(text, 120)))
        .unwrap_or_default();
    format!(
        "{icon} \x1b[38;5;245mSkill\x1b[0m loaded \x1b[1;36m{skill}\x1b[0m \x1b[2m({body_lines} lines)\x1b[0m{description}"
    )
}

pub(crate) fn extract_tool_path(parsed: &serde_json::Value) -> String {
    parsed
        .get("file_path")
        .or_else(|| parsed.get("filePath"))
        .or_else(|| parsed.get("path"))
        .and_then(|value| value.as_str())
        .unwrap_or("?")
        .to_string()
}

fn format_search_start(label: &str, parsed: &serde_json::Value) -> String {
    let pattern = parsed
        .get("pattern")
        .and_then(|value| value.as_str())
        .unwrap_or("?");
    let scope = parsed
        .get("path")
        .and_then(|value| value.as_str())
        .unwrap_or(".");
    format!("{label} {pattern}\n\x1b[2min {scope}\x1b[0m")
}

fn format_patch_preview(old_value: &str, new_value: &str) -> Option<String> {
    if old_value.is_empty() && new_value.is_empty() {
        return None;
    }
    Some(format!(
        "\x1b[38;5;203m- {}\x1b[0m\n\x1b[38;5;70m+ {}\x1b[0m",
        truncate_for_summary(first_visible_line(old_value), 72),
        truncate_for_summary(first_visible_line(new_value), 72)
    ))
}

fn format_bash_call(parsed: &serde_json::Value) -> String {
    let command = parsed
        .get("command")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    if command.is_empty() {
        String::new()
    } else {
        format!(
            "\x1b[48;5;236;38;5;255m $ {} \x1b[0m",
            truncate_for_summary(command, 160)
        )
    }
}

pub(crate) fn first_visible_line(text: &str) -> &str {
    text.lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or(text)
}

fn format_bash_result(icon: &str, parsed: &serde_json::Value) -> String {
    use std::fmt::Write as _;

    let mut lines = vec![format!("{icon} \x1b[38;5;245mbash\x1b[0m")];
    if let Some(task_id) = parsed
        .get("backgroundTaskId")
        .and_then(|value| value.as_str())
    {
        write!(&mut lines[0], " backgrounded ({task_id})").expect("write to string");
    } else if let Some(status) = parsed
        .get("returnCodeInterpretation")
        .and_then(|value| value.as_str())
        .filter(|status| !status.is_empty())
    {
        write!(&mut lines[0], " {status}").expect("write to string");
    }

    if let Some(stdout) = parsed.get("stdout").and_then(|value| value.as_str()) {
        if !stdout.trim().is_empty() {
            lines.push(truncate_output_for_display(
                stdout,
                TOOL_OUTPUT_DISPLAY_MAX_LINES,
                TOOL_OUTPUT_DISPLAY_MAX_CHARS,
            ));
        }
    }
    if let Some(stderr) = parsed.get("stderr").and_then(|value| value.as_str()) {
        if !stderr.trim().is_empty() {
            lines.push(format!(
                "\x1b[38;5;203m{}\x1b[0m",
                truncate_output_for_display(
                    stderr,
                    TOOL_OUTPUT_DISPLAY_MAX_LINES,
                    TOOL_OUTPUT_DISPLAY_MAX_CHARS,
                )
            ));
        }
    }

    lines.join("\n\n")
}

fn format_read_result(icon: &str, parsed: &serde_json::Value) -> String {
    let file = parsed.get("file").unwrap_or(parsed);
    let path = extract_tool_path(file);
    let start_line = file
        .get("startLine")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(1);
    let num_lines = file
        .get("numLines")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let total_lines = file
        .get("totalLines")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(num_lines);
    let content = file
        .get("content")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let end_line = start_line.saturating_add(num_lines.saturating_sub(1));

    format!(
        "{icon} \x1b[2m📄 Read {path} (lines {}-{} of {})\x1b[0m\n{}",
        start_line,
        end_line.max(start_line),
        total_lines,
        truncate_output_for_display(content, READ_DISPLAY_MAX_LINES, READ_DISPLAY_MAX_CHARS)
    )
}

fn format_write_result(icon: &str, parsed: &serde_json::Value) -> String {
    let path = extract_tool_path(parsed);
    let kind = parsed
        .get("type")
        .and_then(|value| value.as_str())
        .unwrap_or("write");
    let line_count = parsed
        .get("content")
        .and_then(|value| value.as_str())
        .map_or(0, |content| content.lines().count());
    format!(
        "{icon} \x1b[1;32m✏️ {} {path}\x1b[0m \x1b[2m({line_count} lines)\x1b[0m",
        if kind == "create" { "Wrote" } else { "Updated" },
    )
}

fn format_structured_patch_preview(parsed: &serde_json::Value) -> Option<String> {
    let hunks = parsed
        .get("structuredPatch")
        .or_else(|| parsed.get("structured_patch"))?
        .as_array()?;
    let mut preview = Vec::new();
    for hunk in hunks.iter().take(2) {
        let lines = hunk.get("lines")?.as_array()?;
        for line in lines.iter().filter_map(|value| value.as_str()).take(6) {
            match line.chars().next() {
                Some('+') => preview.push(format!("\x1b[38;5;70m{line}\x1b[0m")),
                Some('-') => preview.push(format!("\x1b[38;5;203m{line}\x1b[0m")),
                _ => preview.push(line.to_string()),
            }
        }
    }
    if preview.is_empty() {
        None
    } else {
        Some(preview.join("\n"))
    }
}

fn format_edit_result(icon: &str, parsed: &serde_json::Value) -> String {
    let path = extract_tool_path(parsed);
    let suffix = if parsed
        .get("replaceAll")
        .or_else(|| parsed.get("replace_all"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        " (replace all)"
    } else {
        ""
    };
    let preview = format_structured_patch_preview(parsed).or_else(|| {
        let old_value = parsed
            .get("oldString")
            .or_else(|| parsed.get("old_string"))
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        let new_value = parsed
            .get("newString")
            .or_else(|| parsed.get("new_string"))
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        format_patch_preview(old_value, new_value)
    });

    match preview {
        Some(preview) => format!("{icon} \x1b[1;33m📝 Edited {path}{suffix}\x1b[0m\n{preview}"),
        None => format!("{icon} \x1b[1;33m📝 Edited {path}{suffix}\x1b[0m"),
    }
}

fn format_glob_result(icon: &str, parsed: &serde_json::Value) -> String {
    let num_files = parsed
        .get("numFiles")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let filenames = parsed
        .get("filenames")
        .and_then(|value| value.as_array())
        .map(|files| {
            files
                .iter()
                .filter_map(|value| value.as_str())
                .take(8)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    if filenames.is_empty() {
        format!("{icon} \x1b[38;5;245mglob_search\x1b[0m matched {num_files} files")
    } else {
        format!("{icon} \x1b[38;5;245mglob_search\x1b[0m matched {num_files} files\n{filenames}")
    }
}

fn format_grep_result(icon: &str, parsed: &serde_json::Value) -> String {
    let num_matches = parsed
        .get("numMatches")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let num_files = parsed
        .get("numFiles")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let content = parsed
        .get("content")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let filenames = parsed
        .get("filenames")
        .and_then(|value| value.as_array())
        .map(|files| {
            files
                .iter()
                .filter_map(|value| value.as_str())
                .take(8)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    let summary = format!(
        "{icon} \x1b[38;5;245mgrep_search\x1b[0m {num_matches} matches across {num_files} files"
    );
    if !content.trim().is_empty() {
        format!(
            "{summary}\n{}",
            truncate_output_for_display(
                content,
                TOOL_OUTPUT_DISPLAY_MAX_LINES,
                TOOL_OUTPUT_DISPLAY_MAX_CHARS,
            )
        )
    } else if !filenames.is_empty() {
        format!("{summary}\n{filenames}")
    } else {
        summary
    }
}

fn format_generic_tool_result(icon: &str, name: &str, parsed: &serde_json::Value) -> String {
    let rendered_output = match parsed {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Null => String::new(),
        serde_json::Value::Object(_) | serde_json::Value::Array(_) => {
            serde_json::to_string_pretty(parsed).unwrap_or_else(|_| parsed.to_string())
        }
        _ => parsed.to_string(),
    };
    let preview = truncate_output_for_display(
        &rendered_output,
        TOOL_OUTPUT_DISPLAY_MAX_LINES,
        TOOL_OUTPUT_DISPLAY_MAX_CHARS,
    );

    if preview.is_empty() {
        format!("{icon} \x1b[38;5;245m{name}\x1b[0m")
    } else if preview.contains('\n') {
        format!("{icon} \x1b[38;5;245m{name}\x1b[0m\n{preview}")
    } else {
        format!("{icon} \x1b[38;5;245m{name}:\x1b[0m {preview}")
    }
}

pub(crate) fn summarize_tool_payload(payload: &str) -> String {
    let compact = match serde_json::from_str::<serde_json::Value>(payload) {
        Ok(value) => value.to_string(),
        Err(_) => payload.trim().to_string(),
    };
    truncate_for_summary(&compact, 96)
}

pub(crate) fn truncate_for_summary(value: &str, limit: usize) -> String {
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(limit).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
}

fn truncate_output_for_display(content: &str, max_lines: usize, max_chars: usize) -> String {
    let original = content.trim_end_matches('\n');
    if original.is_empty() {
        return String::new();
    }

    let mut preview_lines = Vec::new();
    let mut used_chars = 0usize;
    let mut truncated = false;

    for (index, line) in original.lines().enumerate() {
        if index >= max_lines {
            truncated = true;
            break;
        }

        let newline_cost = usize::from(!preview_lines.is_empty());
        let available = max_chars.saturating_sub(used_chars + newline_cost);
        if available == 0 {
            truncated = true;
            break;
        }

        let line_chars = line.chars().count();
        if line_chars > available {
            preview_lines.push(line.chars().take(available).collect::<String>());
            truncated = true;
            break;
        }

        preview_lines.push(line.to_string());
        used_chars += newline_cost + line_chars;
    }

    let mut preview = preview_lines.join("\n");
    if truncated {
        if !preview.is_empty() {
            preview.push('\n');
        }
        preview.push_str(DISPLAY_TRUNCATION_NOTICE);
    }
    preview
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn todo_result_renders_compact_checklist_not_raw_json() {
        let output = json!({
            "oldTodos": [],
            "newTodos": [
                {"content": "버그 재현", "activeForm": "재현 중", "status": "completed"},
                {"content": "수정 적용", "activeForm": "수정 중", "status": "in_progress"},
                {"content": "회귀 테스트", "activeForm": "테스트 중", "status": "pending"}
            ],
            "verificationNudgeNeeded": true
        })
        .to_string();

        let rendered = format_tool_result("TodoWrite", &output, false);
        assert!(rendered.contains("(3 todos)"));
        assert!(rendered.contains("버그 재현"));
        assert!(
            !rendered.contains("verificationNudgeNeeded") && !rendered.contains("activeForm"),
            "raw ledger JSON must not reach the transcript: {rendered}"
        );
    }

    #[test]
    fn skill_result_renders_one_line_summary_not_body() {
        let output = json!({
            "skill": "context-md-builder",
            "path": "/tmp/skill/SKILL.md",
            "args": null,
            "description": "Build or update CONTEXT.md",
            "prompt": "---\nname: context-md-builder\n---\n\n# Body\nlots of instructions\n"
        })
        .to_string();

        let rendered = format_tool_result("Skill", &output, false);
        assert!(rendered.contains("context-md-builder"));
        assert!(rendered.contains("lines)"));
        assert!(
            !rendered.contains("lots of instructions"),
            "skill body belongs to the model, not the transcript: {rendered}"
        );
    }

    #[test]
    fn edit_result_preview_accepts_snake_case_replace_payload() {
        let output = json!({
            "file_path": "src/lib.rs",
            "old_string": "old value",
            "new_string": "new value",
            "replace_all": true
        })
        .to_string();

        let rendered = format_tool_result("edit_file", &output, false);
        assert!(rendered.contains("Edited src/lib.rs (replace all)"));
        assert!(rendered.contains("- old value"));
        assert!(rendered.contains("+ new value"));
    }

    #[test]
    fn edit_result_preview_accepts_snake_case_structured_patch() {
        let output = json!({
            "file_path": "src/lib.rs",
            "structured_patch": [
                {
                    "lines": [" context", "-old", "+new"]
                }
            ]
        })
        .to_string();

        let rendered = format_tool_result("Edit", &output, false);
        assert!(rendered.contains("Edited src/lib.rs"));
        assert!(rendered.contains(" context"));
        assert!(rendered.contains("-old"));
        assert!(rendered.contains("+new"));
    }
}
