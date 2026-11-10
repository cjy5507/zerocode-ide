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
//!
//! One departure from the verbatim prompt, and only where a repository has a
//! history to read (2026-11-10): its latest commit messages ride the prompt as
//! style examples ([`prompt_in_style`]), the way Zed's generator has carried
//! them since zed-industries/zed#61528. Orca's rules know nothing of the
//! repository they are used in — a history of `fix(scope): …` subjects, long
//! prose ones or Korean ones got back an imperative English line of seventy-two
//! characters — and [`tidy`] then cut any longer subject at that length. A
//! repository with no commits yet gets Orca's prompt and Orca's tidy, to the
//! letter.

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

/// How many of the repository's latest commit messages ride the prompt as
/// style examples — Zed's number (zed-industries/zed#61528): enough to show a
/// convention, few enough that one odd commit does not become it.
pub const STYLE_EXAMPLES: usize = 3;

/// The most of one example the prompt carries. A body is kept so its shape
/// can be learned, not so a long one can be read whole.
const STYLE_EXAMPLE_CHARS: usize = 1_500;

/// Orca's cap on a subject, which its rules state and its tidy enforces.
const SUBJECT_CHARS: usize = 72;

/// The repository's latest commit messages as style examples, newest first,
/// from `git log --format=%B%x00`: each message trimmed, its trailers taken
/// off (the rules forbid them, and an example that ends in `Co-authored-by`
/// teaches one), empty ones dropped, at most [`STYLE_EXAMPLES`] kept.
#[must_use]
pub fn style_examples(log: &str) -> Vec<String> {
    log.split('\0')
        .map(|message| without_trailers(message.trim()).to_string())
        .filter(|message| !message.is_empty())
        .take(STYLE_EXAMPLES)
        .collect()
}

/// `message` without its trailer block: a last paragraph in which every line
/// is `Token: value`, as git reads one. A message that is nothing but such a
/// paragraph is its subject and stays.
fn without_trailers(message: &str) -> &str {
    let Some(cut) = message.rfind("\n\n") else {
        return message;
    };
    let last = &message[cut + 2..];
    let is_trailer = |line: &str| {
        line.split_once(": ").is_some_and(|(token, value)| {
            !token.is_empty()
                && !value.trim().is_empty()
                && token
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
        })
    };
    if last.lines().all(is_trailer) {
        message[..cut].trim_end()
    } else {
        message
    }
}

/// What the repository's own recent commits show a subject may be, where that
/// differs from Orca's defaults — so [`tidy_in_style`] does not cut a subject
/// the prompt asked for at the length the default rules name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    /// The longest a subject may run: Orca's 72, or the longest recent one.
    pub subject_chars: usize,
    /// Whether a subject keeps a closing period: only where every recent one
    /// ends in one.
    pub keeps_period: bool,
}

impl Style {
    /// Orca's own: what a repository with no history to read gets.
    pub const ORCA: Self = Self {
        subject_chars: SUBJECT_CHARS,
        keeps_period: false,
    };

    /// The style `examples` ([`style_examples`]) show.
    #[must_use]
    pub fn of(examples: &[String]) -> Self {
        let subjects: Vec<&str> = examples
            .iter()
            .filter_map(|message| message.lines().next())
            .map(str::trim)
            .filter(|subject| !subject.is_empty())
            .collect();
        if subjects.is_empty() {
            return Self::ORCA;
        }
        Self {
            subject_chars: subjects
                .iter()
                .map(|subject| subject.chars().count())
                .fold(SUBJECT_CHARS, usize::max),
            keeps_period: subjects.iter().all(|subject| subject.ends_with('.')),
        }
    }
}

/// The prompt, verbatim from `buildCommitMessagePrompt` — every rule line is
/// Orca's own wording, because the rules are instructions to a model and a
/// paraphrase is an unmeasured change to what comes back.
pub fn prompt(branch: Option<&str>, staged_summary: &str, staged_patch: &str) -> String {
    prompt_in_style(branch, staged_summary, staged_patch, &[])
}

/// [`prompt`], with the repository's latest commit messages as style examples
/// ([`style_examples`]). With none it is Orca's prompt to the letter. With
/// some, one rule is added that puts the repository's convention above the
/// default rules where the two differ, and the examples stand between the
/// branch and the staged files, fenced in tags and said to be style only —
/// the wording of Zed's section (zed-industries/zed#61528).
pub fn prompt_in_style(
    branch: Option<&str>,
    staged_summary: &str,
    staged_patch: &str,
    examples: &[String],
) -> String {
    let patch = if staged_patch.trim().is_empty() {
        "(diff omitted — too large to read; infer the change from the staged file list above)"
            .to_string()
    } else {
        truncate_diff_for_prompt(staged_patch, STAGED_DIFF_BYTE_BUDGET)
    };
    let mut lines = vec![
        "You are generating a single git commit message.".to_string(),
        "Return only the commit message text. Do not include a preamble, quotes, or code fences."
            .to_string(),
        String::new(),
        "Rules:".to_string(),
        "- First line: imperative mood, <= 72 chars, no trailing period.".to_string(),
        "- Optional body: blank line, then short wrapped bullet points or prose explaining WHY."
            .to_string(),
        "- Capture the primary user-visible or developer-visible change.".to_string(),
        "- Use only the staged changes below as context.".to_string(),
        "- Do not include \"Co-authored-by\" or other git trailers.".to_string(),
    ];
    if !examples.is_empty() {
        lines.push(
            "- Where this repository's recent commit messages below follow another convention — a prefix or scope, the form or length of the first line, the language, how a body is laid out — follow the repository's convention instead of the defaults above."
                .to_string(),
        );
    }
    lines.push(String::new());
    lines.push(format!("Branch: {}", branch.unwrap_or("(detached)")));
    lines.push(String::new());
    if !examples.is_empty() {
        lines.push(
            "Use the following recent commit messages only as style examples. Keep the generated message consistent with them; do not reuse what they say:"
                .to_string(),
        );
        lines.push("<recent_commit_messages>".to_string());
        for example in examples.iter().take(STYLE_EXAMPLES) {
            lines.push("<commit_message>".to_string());
            lines.push(limit_section(example, STYLE_EXAMPLE_CHARS));
            lines.push("</commit_message>".to_string());
        }
        lines.push("</recent_commit_messages>".to_string());
        lines.push(String::new());
    }
    lines.push("Staged files:".to_string());
    lines.push(limit_section(staged_summary, 6_000));
    lines.push(String::new());
    lines.push("Staged patch:".to_string());
    lines.push("```diff".to_string());
    lines.push(patch);
    lines.push("```".to_string());
    lines.join("\n")
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
    clip_text_on_line_boundary(section, limit, "diff")
}

/// The same cut for any text. `what` is the word the marker calls the text by
/// (`diff`, `conversation`): the model that reads a marker is told what was
/// cut, and a conversation that says "(diff truncated)" is told wrongly.
pub(crate) fn clip_text_on_line_boundary(section: &str, limit: usize, what: &str) -> String {
    if section.len() <= limit {
        return section.to_string();
    }
    if limit == 0 {
        return String::new();
    }
    let marker_for = |omitted: usize| format!("\n...({what} truncated, {omitted} bytes omitted)\n");
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
pub(crate) fn floor_char(text: &str, at: usize) -> usize {
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
    tidy_in_style(raw, Style::ORCA)
}

/// [`tidy`], with the subject held to what the repository's recent commits
/// show ([`Style`]) rather than to Orca's seventy-two characters and no
/// period — the prompt asked for the repository's convention
/// ([`prompt_in_style`]), and a tidy that then cut the subject it got at the
/// default length would hand the commit box half a sentence.
pub fn tidy_in_style(raw: &str, style: Style) -> String {
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
    let subject_line = subject_line.trim();
    let subject_line = if style.keeps_period {
        subject_line
    } else {
        subject_line.trim_end_matches('.')
    };
    let subject: String = subject_line.chars().take(style.subject_chars).collect();
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
pub(crate) fn fence_body(text: &str) -> Option<&str> {
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

    /// A repository with a history lends its latest messages as style
    /// examples: at most three, newest first, trailers off, each capped — and
    /// one rule that puts its convention above the defaults. A repository
    /// with none gets Orca's prompt to the letter.
    #[test]
    fn a_repositorys_recent_messages_ride_the_prompt_as_style_examples_only() {
        let log = [
            "perf(pty): a screen row keeps how far it was written\n\nMost of a row is blank.\n\nCo-Authored-By: A Person <a@example.com>\nSigned-off-by: A Person <a@example.com>\n",
            "\nfix(quota): the wall that names its window\n",
            "   \n",
            "release: v1.1.138\n\nCo-Authored-By: A Person <a@example.com>\n",
            "an older one\n",
        ]
        .join("\0\n");
        let examples = style_examples(&log);
        assert_eq!(
            examples,
            [
                "perf(pty): a screen row keeps how far it was written\n\nMost of a row is blank.",
                "fix(quota): the wall that names its window",
                "release: v1.1.138",
            ]
        );
        // A message that is only a trailer-shaped line is its own subject.
        assert_eq!(style_examples("Fixes: the build\0"), ["Fixes: the build"]);
        assert!(style_examples("").is_empty());

        let plain = prompt(Some("main"), "M src/a.rs", "diff --git a/a b/a\n+x\n");
        assert_eq!(
            prompt_in_style(Some("main"), "M src/a.rs", "diff --git a/a b/a\n+x\n", &[]),
            plain
        );
        assert!(!plain.contains("recent_commit_messages") && !plain.contains("convention"));

        let styled = prompt_in_style(
            Some("main"),
            "M src/a.rs",
            "diff --git a/a b/a\n+x\n",
            &examples,
        );
        let at = |needle: &str| styled.find(needle).unwrap_or_else(|| panic!("{needle}"));
        assert!(at("- Do not include") < at("follow the repository's convention"));
        assert!(at("follow the repository's convention") < at("Branch: main"));
        assert!(at("Branch: main") < at("only as style examples"));
        assert!(at("<recent_commit_messages>") < at("perf(pty): a screen row"));
        assert!(at("release: v1.1.138") < at("</recent_commit_messages>"));
        assert!(at("</recent_commit_messages>") < at("Staged files:"));
        assert_eq!(styled.matches("<commit_message>").count(), 3);
        assert!(!styled.contains("Co-Authored-By") && !styled.contains("Signed-off-by"));
        // Every line of Orca's prompt is still there, in its order.
        let mut rest = styled.as_str();
        for line in plain.lines().filter(|line| !line.is_empty()) {
            let found = rest.find(line).unwrap_or_else(|| panic!("lost: {line}"));
            rest = &rest[found + line.len()..];
        }

        let long = vec![format!("subject\n\n{}", "b".repeat(5_000))];
        let capped = prompt_in_style(None, "M a", "", &long);
        assert!(
            capped.contains("[truncated: 3509 characters omitted]"),
            "an example over its cap"
        );
    }

    /// The tidy holds a subject to what the repository's recent subjects
    /// show, so one the prompt asked to be long is not cut at seventy-two —
    /// and to Orca's own where there is nothing to read or nothing longer.
    #[test]
    fn a_subject_is_held_to_the_repositorys_own_length_and_period() {
        let long_subject = format!("perf(pty): {}", "word ".repeat(30).trim_end());
        assert!(long_subject.chars().count() > 72);
        let style = Style::of(&[format!("{long_subject}\n\nbody"), "fix: short".to_string()]);
        assert_eq!(style.subject_chars, long_subject.chars().count());
        assert!(!style.keeps_period);
        let answer = format!("fix(vault): {}.\n\nWhy.", "w".repeat(100));
        assert_eq!(
            tidy_in_style(&answer, style),
            format!("fix(vault): {}\n\nWhy.", "w".repeat(100))
        );
        assert_eq!(
            tidy(&answer)
                .lines()
                .next()
                .map(|line| line.chars().count()),
            Some(72)
        );
        // Still capped, at the repository's own longest.
        let over = tidy_in_style(&"x".repeat(500), style);
        assert_eq!(over.chars().count(), style.subject_chars);

        assert_eq!(Style::of(&[]), Style::ORCA);
        assert_eq!(Style::of(&["Add a reader".to_string()]), Style::ORCA);
        let dotted = Style::of(&["Add a reader.".to_string(), "Fix a writer.".to_string()]);
        assert!(dotted.keeps_period);
        assert_eq!(tidy_in_style("Add a thing.", dotted), "Add a thing.");
        let mixed = Style::of(&["Add a reader.".to_string(), "Fix a writer".to_string()]);
        assert!(!mixed.keeps_period);
        assert_eq!(tidy_in_style("Add a thing.", mixed), "Add a thing");
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
