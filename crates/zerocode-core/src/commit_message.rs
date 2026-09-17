//! A commit message drafted from what is staged, by the agent's one-shot mode.
//!
//! Orca's AI commit messages, at the layer that matters: the PROMPT and the
//! ARGV are protocol, measured verbatim, and the window only carries the
//! result into the commit box. Three sources, all in the extracted bundle:
//!
//! - `buildCommitMessagePrompt` (SourceControl-e46DLHZz.js:4236) — the rules
//!   block, the branch line, the staged file list capped at 6,000 chars, the
//!   patch in a ```diff fence;
//! - `truncateDiffForPrompt` + `allocateBudgetFairly` +
//!   `clipSectionOnLineBoundary` (I18nProvider-4EBrmTGg.js:28006-28078) — a
//!   200 KB budget shared FAIRLY across the diff's file sections, each clip
//!   landing on a line boundary with a marker saying what it cost;
//! - `COMMIT_MESSAGE_AGENT_SPECS.claude` (out/main/index.js:2438) — `claude
//!   -p --output-format text --model <alias> --permission-mode plan`, prompt
//!   on STDIN (a staged diff on argv would hit command-line limits), model as
//!   an ALIAS (`sonnet`), never a version id (a hardcoded id is rejected by
//!   Bedrock/Vertex accounts).
//!
//! The fair split is the part worth copying exactly: a naive head-truncation
//! spends the whole budget on whichever file happens to sort first, and the
//! message comes back describing one file of a forty-file change.

/// Orca's budget for the staged patch inside the prompt
/// (`STAGED_DIFF_BYTE_BUDGET`, I18nProvider:28006).
pub const STAGED_DIFF_BYTE_BUDGET: usize = 200_000;

/// How long the one-shot may run (`GENERATION_TIMEOUT_MS`,
/// out/main/index.js:107852).
pub const GENERATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// The most output the one-shot may print (`MAX_AGENT_OUTPUT_BYTES`,
/// :107853) — a generator that starts streaming a conversation instead of a
/// message is cut off, not buffered.
pub const MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;

/// The model the button uses — Orca's own default (`defaultModelId:
/// "sonnet"`, out/main/index.js:2478), and like every entry in its list an
/// alias rather than a version id.
pub const DEFAULT_MODEL: &str = "sonnet";

/// The one-shot's argv, after the program name.
///
/// Measured off `COMMIT_MESSAGE_AGENT_SPECS.claude.buildArgs`
/// (out/main/index.js:2446): `-p` reads the prompt from stdin when no
/// positional prompt is given, and `--permission-mode plan` is what makes
/// this a TEXT generator — a one-shot that could edit files while drafting a
/// commit message is a hazard, not a feature.
pub fn argv(model: &str) -> Vec<String> {
    [
        "-p",
        "--output-format",
        "text",
        "--model",
        model,
        "--permission-mode",
        "plan",
    ]
    .iter()
    .map(|word| (*word).to_string())
    .collect()
}

/// The prompt, verbatim from `buildCommitMessagePrompt` — every rule line is
/// Orca's own wording, because the rules are instructions to a model and a
/// paraphrase is an unmeasured change to what comes back.
pub fn prompt(branch: Option<&str>, staged_summary: &str, staged_patch: &str) -> String {
    let patch = if staged_patch.trim().is_empty() {
        "(diff omitted — too large to read; infer the change from the staged file list above)"
            .to_string()
    } else {
        truncate_diff_for_prompt(staged_patch, STAGED_DIFF_BYTE_BUDGET)
    };
    [
        "You are generating a single git commit message.",
        "Return only the commit message text. Do not include a preamble, quotes, or code fences.",
        "",
        "Rules:",
        "- First line: imperative mood, <= 72 chars, no trailing period.",
        "- Optional body: blank line, then short wrapped bullet points or prose explaining WHY.",
        "- Capture the primary user-visible or developer-visible change.",
        "- Use only the staged changes below as context.",
        "- Do not include \"Co-authored-by\" or other git trailers.",
        "",
        &format!("Branch: {}", branch.unwrap_or("(detached)")),
        "",
        "Staged files:",
        &limit_section(staged_summary, 6_000),
        "",
        "Staged patch:",
        "```diff",
        &patch,
        "```",
    ]
    .join("\n")
}

/// A section capped in characters, saying what it dropped (`limitSection`,
/// SourceControl:4227).
fn limit_section(value: &str, max_chars: usize) -> String {
    let count = value.chars().count();
    if count <= max_chars {
        return value.to_string();
    }
    let kept: String = value.chars().take(max_chars).collect();
    format!(
        "{kept}\n\n[truncated: {} characters omitted]",
        count - max_chars
    )
}

/// The staged patch under its budget: whole when it fits, otherwise split at
/// file boundaries and clipped under a FAIRLY shared budget
/// (`truncateDiffForPrompt`, I18nProvider:28064).
pub fn truncate_diff_for_prompt(diff: &str, budget: usize) -> String {
    if diff.len() <= budget {
        return diff.to_string();
    }
    let sections = split_into_file_sections(diff);
    if sections.len() <= 1 {
        return clip_on_line_boundary(diff, budget);
    }
    let allocations = allocate_budget_fairly(
        &sections.iter().map(|s| s.len()).collect::<Vec<_>>(),
        budget,
    );
    sections
        .iter()
        .zip(allocations)
        .map(|(section, allowed)| clip_on_line_boundary(section, allowed))
        .collect()
}

/// One slice per `diff --git` header (`splitDiffIntoFileSections`,
/// I18nProvider:28008). The boundary carries its leading newline so joining
/// the sections back reproduces the diff byte for byte.
fn split_into_file_sections(diff: &str) -> Vec<&str> {
    const BOUNDARY: &str = "\ndiff --git ";
    let mut sections = Vec::new();
    let mut start = 0usize;
    let mut from = 0usize;
    while let Some(found) = diff[from..].find(BOUNDARY) {
        let at = from + found;
        sections.push(&diff[start..=at]);
        start = at + 1;
        from = at + 1;
    }
    sections.push(&diff[start..]);
    sections
}

/// Round-robin shares until every section fits or the budget is spent
/// (`allocateBudgetFairly`, I18nProvider:28036). Small files take what they
/// need; the giants split what remains — which is what stops one generated
/// lockfile from eating the whole budget.
fn allocate_budget_fairly(sizes: &[usize], budget: usize) -> Vec<usize> {
    let mut alloc = vec![0usize; sizes.len()];
    let mut active: Vec<usize> = (0..sizes.len()).collect();
    let mut remaining = budget;
    while !active.is_empty() && remaining > 0 {
        let share = remaining / active.len();
        if share == 0 {
            break;
        }
        let mut still = Vec::new();
        for at in active {
            let need = sizes[at] - alloc[at];
            let grant = need.min(share);
            alloc[at] += grant;
            remaining -= grant;
            if grant < need {
                still.push(at);
            }
        }
        active = still;
    }
    alloc
}

/// A section cut on a line break, wearing a marker that says what the cut
/// cost (`clipSectionOnLineBoundary`, I18nProvider:28016). The marker's own
/// length is budgeted too — a clip that overshoots by the size of its
/// apology has not clipped.
fn clip_on_line_boundary(section: &str, limit: usize) -> String {
    if section.len() <= limit {
        return section.to_string();
    }
    if limit == 0 {
        return String::new();
    }
    let marker_for = |omitted: usize| format!("\n...(diff truncated, {omitted} bytes omitted)\n");
    let mut marker = marker_for(section.len());
    if marker.len() >= limit {
        marker.truncate(limit);
        return marker;
    }
    let target = limit - marker.len();
    let boundary = floor_char(section, target);
    let cut = match section[..boundary].rfind('\n') {
        Some(line_break) if line_break > target / 2 => line_break,
        _ => boundary,
    };
    marker = marker_for(section.len() - cut);
    let end = floor_char(section, cut.min(limit.saturating_sub(marker.len())));
    format!("{}{marker}", &section[..end])
}

/// The largest char boundary at or under `at` — the JS slices by UTF-16 units
/// and never notices; a Rust slice through a multibyte char panics.
fn floor_char(text: &str, at: usize) -> usize {
    let mut boundary = at.min(text.len());
    while !text.is_char_boundary(boundary) {
        boundary -= 1;
    }
    boundary
}

/// What the model printed, reduced to the message a commit box can take.
///
/// Orca's two passes, combined (`cleanGeneratedCommitMessage`,
/// out/main/index.js:21388, and `splitGeneratedCommitMessage`, :106791): drop
/// a `Generating…`/`Thinking…` first line, unwrap a fence the model added
/// despite the rules, strip a leading bullet, then cap the subject at 72
/// chars with no trailing period. An empty answer becomes Orca's own
/// fallback subject rather than an empty commit box that looks like a
/// success.
pub fn tidy(raw: &str) -> String {
    let mut text = raw.replace("\r\n", "\n").trim().to_string();
    if let Some((first, rest)) = text.split_once('\n') {
        let lead = first.trim().to_ascii_lowercase();
        let noise = lead.starts_with("generating")
            || lead.starts_with("thinking")
            || (!lead.is_empty() && lead.chars().all(|c| c == '.' || c == '…'));
        if noise {
            text = rest.trim().to_string();
        }
    }
    if let Some(inner) = fence_body(&text) {
        text = inner.trim().to_string();
    }
    for lead in ["- ", "* ", "• ", "● "] {
        if let Some(rest) = text.strip_prefix(lead) {
            text = rest.trim_start().to_string();
            break;
        }
    }
    let (subject_line, body) = match text.split_once('\n') {
        Some((subject, rest)) => (subject, rest.trim()),
        None => (text.as_str(), ""),
    };
    let subject: String = subject_line
        .trim()
        .trim_end_matches('.')
        .chars()
        .take(72)
        .collect();
    let subject = subject.trim_end();
    let subject = if subject.is_empty() {
        "Update project files"
    } else {
        subject
    };
    if body.is_empty() {
        subject.to_string()
    } else {
        format!("{subject}\n\n{body}")
    }
}

/// The body of a fence that encloses the WHOLE text, or nothing.
fn fence_body(text: &str) -> Option<&str> {
    let rest = text.strip_prefix("```")?;
    let after_info = rest.split_once('\n')?.1;
    let inner = after_info.strip_suffix("```").or_else(|| {
        after_info
            .rfind("\n```")
            .and_then(|at| (after_info[at + 4..].trim().is_empty()).then(|| &after_info[..at]))
    })?;
    Some(inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fair split, on the shape it exists for: one giant section and two
    /// small ones. Head-truncation would describe only the giant.
    #[test]
    fn the_budget_is_shared_fairly_across_file_sections() {
        let small_one = format!("diff --git a/a b/a\n{}\n", "a".repeat(100));
        let giant = format!("diff --git a/big b/big\n{}\n", "b\n".repeat(3_000));
        let small_two = format!("diff --git a/c b/c\n{}\n", "c".repeat(100));
        let diff = format!("{small_one}{giant}{small_two}");
        let cut = truncate_diff_for_prompt(&diff, 1_000);
        assert!(cut.len() <= 1_000 + 50, "{} bytes", cut.len());
        // The small sections survive whole; the giant wears the marker.
        assert!(cut.contains(&"a".repeat(100)));
        assert!(cut.contains(&"c".repeat(100)));
        assert!(cut.contains("...(diff truncated,"));
        // And a diff under budget is untouched, byte for byte.
        assert_eq!(truncate_diff_for_prompt(&diff, diff.len()), diff);
    }

    /// Splitting carries the boundary newline so the join reproduces the
    /// diff exactly — a split that eats bytes truncates silently.
    #[test]
    fn sections_join_back_to_the_exact_diff() {
        let diff = "diff --git a/a b/a\n+one\ndiff --git a/b b/b\n+two\n";
        let sections = split_into_file_sections(diff);
        assert_eq!(sections.len(), 2);
        assert_eq!(sections.concat(), diff);
    }

    /// The prompt is Orca's, line for line where it matters: the untrusted
    /// rules block, the 6,000-char file list cap, the fenced patch, and the
    /// omitted-diff sentence when nothing was readable.
    #[test]
    fn the_prompt_carries_orcas_own_rules() {
        let words = prompt(Some("main"), "M src/a.rs", "diff --git a/a b/a\n+x\n");
        assert!(words.starts_with("You are generating a single git commit message."));
        assert!(words.contains("- First line: imperative mood, <= 72 chars, no trailing period."));
        assert!(words.contains("Branch: main"));
        assert!(
            words.contains("```diff\ndiff --git a/a b/a\n+x\n\n```") || words.contains("```diff")
        );
        let bare = prompt(None, "M src/a.rs", "   ");
        assert!(bare.contains("Branch: (detached)"));
        assert!(bare.contains("(diff omitted — too large to read"));
    }

    /// The tidy pass: fences unwrapped, noise lines dropped, the subject
    /// capped at 72 with no trailing period, and an empty answer named
    /// rather than passed through as success.
    #[test]
    fn the_answer_is_tidied_the_way_orca_tidies_it() {
        assert_eq!(
            tidy("```\nFix the flaky test.\n\nBecause the fixture was shared.\n```"),
            "Fix the flaky test\n\nBecause the fixture was shared."
        );
        assert_eq!(tidy("Thinking...\nAdd a reader"), "Add a reader");
        assert_eq!(tidy("- Add a reader"), "Add a reader");
        let long = tidy(&"x".repeat(100));
        assert_eq!(long.chars().count(), 72);
        assert_eq!(tidy("   "), "Update project files");
    }

    /// The argv is the measured spec: stdin prompt, text output, plan mode —
    /// a generator that could edit files is a hazard, not a feature.
    #[test]
    fn the_one_shot_cannot_edit_anything() {
        let words = argv(DEFAULT_MODEL);
        assert_eq!(
            words,
            [
                "-p",
                "--output-format",
                "text",
                "--model",
                "sonnet",
                "--permission-mode",
                "plan"
            ]
        );
    }
}
