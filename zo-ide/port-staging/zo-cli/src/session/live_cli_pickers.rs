use std::io;

fn choice_marker(current: &str, value: &str) -> &'static str {
    if current == value { "*" } else { " " }
}

/// Generic numbered picker used by `/model`, `/permissions`, etc.
pub(super) fn between_turn_choice_picker(
    title: &str,
    options: &[(&str, &str)],
    current: &str,
) -> io::Result<Option<String>> {
    use std::io::Write;

    let mut stdout = io::stdout();
    writeln!(stdout, "\n{title} (Enter to cancel)")?;
    for (idx, (label, value)) in options.iter().enumerate() {
        let marker = choice_marker(current, value);
        writeln!(stdout, "  {marker} {n}) {label}  [{value}]", n = idx + 1)?;
    }
    write!(stdout, "> ")?;
    stdout.flush()?;

    let mut line = String::new();
    let bytes = io::stdin().read_line(&mut line)?;
    if bytes == 0 {
        return Ok(None);
    }
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if let Ok(n) = trimmed.parse::<usize>() {
        if let Some((_, value)) = options.get(n.wrapping_sub(1)) {
            return Ok(Some((*value).to_string()));
        }
    }
    Ok(Some(trimmed.to_string()))
}

pub(super) fn prompt_model_picker(current: &str) -> io::Result<Option<String>> {
    let opus = api::resolve_model_alias(api::ANTHROPIC_OPUS_MODEL_ALIAS);
    let sonnet = api::resolve_model_alias("sonnet");
    let haiku = api::resolve_model_alias("haiku");
    between_turn_choice_picker(
        "Select a model",
        &[
            ("Opus (latest)", opus.as_str()),
            ("Sonnet (latest)", sonnet.as_str()),
            ("Haiku (latest)", haiku.as_str()),
        ],
        &crate::cli_args::resolve_model_alias(current),
    )
}

pub(super) fn prompt_permissions_picker(current: &str) -> io::Result<Option<String>> {
    between_turn_choice_picker(
        "Select a permission mode",
        &[
            ("Read-only - no writes, no shell", "read-only"),
            (
                "Workspace write - edits inside the workspace",
                "workspace-write",
            ),
            ("Danger full access - no guardrails", "danger-full-access"),
        ],
        current,
    )
}

#[cfg(test)]
mod tests {
    use super::choice_marker;

    #[test]
    fn canonical_model_alias_marks_the_current_picker_entry() {
        let current = api::resolve_model_alias("claude-opus-5");
        let opus = api::resolve_model_alias(api::ANTHROPIC_OPUS_MODEL_ALIAS);
        assert_eq!(current, opus);
        assert_eq!(choice_marker(&current, &opus), "*");
    }
}
