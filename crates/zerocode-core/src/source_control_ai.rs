//! The source-control AI actions beyond the commit message.
//!
//! `commit_message` built the first of these; this is the rest of the family,
//! measured from Orca **1.4.164**. Orca splits them into two kinds
//! (`SOURCE_CONTROL_TEXT_ACTION_IDS` / `SOURCE_CONTROL_LAUNCH_ACTION_IDS`,
//! I18nProvider-4EBrmTGg.js:22863-22980) and the split is the design:
//!
//! - **TEXT** — `commitMessage`, `pullRequest`, `branchName`. A one-shot runs,
//!   its output is parsed, and the WINDOW puts the result in a field. Nothing
//!   is edited and nothing is run.
//! - **LAUNCH** — `fixCommitFailure`, `fixPushFailure`, `fixChecks`,
//!   `resolveConflicts`, `resolveComments`. A prompt is handed to a real agent
//!   session, which then works.
//!
//! What is here is the TEXT half's protocol: the prompts (verbatim — they are
//! instructions to a model, and a paraphrase is an unmeasured change to what
//! comes back), the parsing of what comes back, and the tables that say which
//! action is which. The git conversation and the subprocess are the shell's;
//! the fields are the window's.
//!
//! The parsing is the part with teeth. A model asked for compact JSON returns
//! it wrapped in a code fence about a third of the time, so the fence comes off
//! first (`stripJsonFence`, out/main/index.js:106881) — and then the text is
//! measured for STRUCTURE before it is parsed
//! (`assertJsonTextStructureWithinLimits`, I18nProvider:22187), because a
//! serde recursion on deeply nested input is a stack overflow and this input
//! came from a model.

use std::collections::BTreeMap;

/// One text-generating action.
///
/// The ids are Orca's own strings: they key the per-action command templates
/// and recipes a person can save, so they are protocol rather than names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextAction {
    CommitMessage,
    PullRequest,
    BranchName,
}

impl TextAction {
    /// `SOURCE_CONTROL_TEXT_ACTION_IDS`, in Orca's order.
    pub const ALL: &'static [TextAction] = &[
        TextAction::CommitMessage,
        TextAction::PullRequest,
        TextAction::BranchName,
    ];

    pub fn id(self) -> &'static str {
        match self {
            TextAction::CommitMessage => "commitMessage",
            TextAction::PullRequest => "pullRequest",
            TextAction::BranchName => "branchName",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|one| one.id() == id)
    }
}

/// One action that hands its prompt to a real agent session.
///
/// Carried as a table rather than built as the window needs them, because the
/// ids key the same saved recipes the text actions do — a person who set
/// "resolveConflicts runs codex with these arguments" is naming this string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchAction {
    FixCommitFailure,
    FixPushFailure,
    FixChecks,
    ResolveConflicts,
    ResolveComments,
}

impl LaunchAction {
    /// `SOURCE_CONTROL_LAUNCH_ACTION_IDS`, in Orca's order.
    pub const ALL: &'static [LaunchAction] = &[
        LaunchAction::FixCommitFailure,
        LaunchAction::FixPushFailure,
        LaunchAction::FixChecks,
        LaunchAction::ResolveConflicts,
        LaunchAction::ResolveComments,
    ];

    pub fn id(self) -> &'static str {
        match self {
            LaunchAction::FixCommitFailure => "fixCommitFailure",
            LaunchAction::FixPushFailure => "fixPushFailure",
            LaunchAction::FixChecks => "fixChecks",
            LaunchAction::ResolveConflicts => "resolveConflicts",
            LaunchAction::ResolveComments => "resolveComments",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|one| one.id() == id)
    }
}

/// Every action's default command template.
///
/// `DEFAULT_SOURCE_CONTROL_ACTION_COMMAND_TEMPLATES` maps all eight to
/// `"{basePrompt}"` — the built prompt, unwrapped. The table exists so a person
/// can override ONE action ("wrap the conflict prompt in my own preamble")
/// without inheriting a wrapper on the other seven.
pub const DEFAULT_COMMAND_TEMPLATE: &str = "{basePrompt}";

/// Render a command template.
///
/// Both spellings, because Orca accepts both
/// (`renderSourceControlActionCommandTemplate`, I18nProvider:22960): `{name}`
/// and `{{name}}`. A variable the caller did not supply is LEFT AS IT IS rather
/// than emptied — a template naming a variable this action does not have is a
/// mistake the person can see in the preview, and silently deleting it hides it.
pub fn render_command_template(template: &str, variables: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(at) = rest.find('{') {
        out.push_str(&rest[..at]);
        let after = &rest[at..];
        // `{{name}}` first: it starts with `{name…` too, so testing the short
        // form first would eat one brace and leave the other.
        let braced = after.strip_prefix("{{").and_then(|tail| {
            tail.find("}}")
                .map(|end| (&tail[..end], &tail[end + 2..], true))
        });
        let single = after.strip_prefix('{').and_then(|tail| {
            tail.find('}')
                .map(|end| (&tail[..end], &tail[end + 1..], false))
        });
        let Some((name, tail, doubled)) = braced.or(single) else {
            // A `{` with no closing brace is text.
            out.push('{');
            rest = &after[1..];
            continue;
        };
        match variables.iter().find(|(key, _)| *key == name) {
            Some((_, value)) => out.push_str(value),
            None if doubled => out.push_str(&format!("{{{{{name}}}}}")),
            None => out.push_str(&format!("{{{name}}}")),
        }
        rest = tail;
    }
    out.push_str(rest);
    out
}

/// Cut a prompt section to `max_chars`, saying what it cost.
///
/// `limitSection` (out/main/index.js:106812) verbatim, marker included. The
/// marker is the point: a model handed a silently truncated diff describes the
/// part it was given as if it were the whole change.
pub fn limit_section(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    // By characters rather than bytes: the source counts UTF-16 units and a
    // byte cut would split a multi-byte character. Characters is the closest
    // honest reading, and the marker's number is what it actually dropped.
    let kept: String = value.chars().take(max_chars).collect();
    let omitted = value.chars().count() - max_chars;
    format!("{kept}\n\n[truncated: {omitted} characters omitted]")
}

/// What the PR generator is told about the branch it is describing.
#[derive(Debug, Clone, Default)]
pub struct PullRequestContext {
    /// `None` on a detached HEAD, and the prompt says so in those words.
    pub branch: Option<String>,
    pub base: String,
    pub current_title: String,
    pub current_body: String,
    pub current_draft: bool,
    /// `git log --pretty=format:- %s --max-count=50 <merge-base>..HEAD`.
    pub commit_summary: String,
    /// `git diff --name-status <merge-base>..HEAD`.
    pub change_summary: String,
    /// `git diff --patch --minimal --no-color --no-ext-diff <merge-base>..HEAD`,
    /// already through the fair-split truncator.
    pub patch: String,
}

/// Orca's caps on the two summary sections and on a person's own prompt.
const SUMMARY_LIMIT: usize = 8_000;
const CUSTOM_PROMPT_LIMIT: usize = 4_000;

/// The pull-request prompt, verbatim from `buildPullRequestFieldsPrompt`
/// (out/main/index.js:106818).
///
/// Two rules in it are worth naming because they are the difference between a
/// useful draft and one a reviewer has to undo: the TEMPLATE rule — if the
/// current description is a PR template, its headings, required sections and
/// checklists are preserved and filled rather than replaced — and the TODO
/// rule, which says to leave genuinely unknown template items as TODO or
/// unchecked instead of deleting them. A generator that quietly deletes a
/// repository's checklist is worse than no generator.
pub fn pull_request_prompt(context: &PullRequestContext, custom_prompt: &str) -> String {
    let base = [
        "You are generating pull request details.",
        "Return ONLY compact JSON with this exact shape:",
        r#"{"base":"branch-name","title":"short title","body":"markdown description","draft":false}"#,
        "",
        "Rules:",
        "- Use the branch diff and commits below as source of truth.",
        "- Keep the base branch as the current base unless the diff clearly targets a different branch.",
        "- Title: concise, specific, no trailing period.",
        "- Body: useful Markdown summary for reviewers. Include testing notes only when evidence exists.",
        "- If Current description contains a pull request or merge request template, preserve its headings, required sections, and checklists while filling relevant sections from the branch changes.",
        "- Leave genuinely unknown template items as TODO or unchecked instead of deleting them.",
        "- draft: true only when the changes clearly look unfinished, WIP, or unsafe to review.",
        "- Do not include labels, reviewers, code fences, prose, or any keys beyond base/title/body/draft.",
        "",
    ]
    .join("\n");
    let head = format!(
        "Head branch: {}\nCurrent base: {}\nCurrent title: {}\nCurrent description: {}\nCurrent draft: {}",
        context.branch.as_deref().unwrap_or("(detached)"),
        context.base,
        if context.current_title.is_empty() {
            "(empty)"
        } else {
            &context.current_title
        },
        if context.current_body.is_empty() {
            "(empty)"
        } else {
            &context.current_body
        },
        if context.current_draft {
            "true"
        } else {
            "false"
        },
    );
    let body = format!(
        "{base}{head}\n\nCommits:\n{}\n\nChanged files:\n{}\n\nPatch:\n```diff\n{}\n```",
        limit_section(
            if context.commit_summary.is_empty() {
                "(none)"
            } else {
                &context.commit_summary
            },
            SUMMARY_LIMIT
        ),
        limit_section(
            if context.change_summary.is_empty() {
                "(none)"
            } else {
                &context.change_summary
            },
            SUMMARY_LIMIT
        ),
        context.patch,
    );
    let tail = "Final output requirement:\nReturn compact JSON only with keys base, title, body, and draft. No prose or code fences.";
    match custom_prompt.trim() {
        "" => format!("{body}\n\n{tail}"),
        said => format!(
            "{body}\n\nAdditional user prompt:\n{}\n\n{tail}",
            limit_section(said, CUSTOM_PROMPT_LIMIT)
        ),
    }
}

/// Orca's structure ceiling for a generated pull request
/// (`GENERATED_PULL_REQUEST_JSON_STRUCTURE_LIMITS`, :106806).
const PR_STRUCTURAL_TOKENS: usize = 64;
const PR_NESTING_DEPTH: usize = 8;

/// Is this text structurally small enough to hand to a parser?
///
/// `assertJsonTextStructureWithinLimits` (I18nProvider:22187) verbatim: count
/// `{}[],:` outside strings, refuse past the token ceiling, and refuse past the
/// nesting ceiling. Asked BEFORE parsing, which is the whole point — this input
/// came from a model, and "the parser will handle it" is how a recursive
/// descent meets a thousand open braces.
fn structure_within_limits(text: &str, tokens: usize, depth: usize) -> bool {
    let mut seen = 0usize;
    let mut deep = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for character in text.chars() {
        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }
        if character == '"' {
            in_string = true;
            continue;
        }
        if !matches!(character, '{' | '}' | '[' | ']' | ',' | ':') {
            continue;
        }
        seen += 1;
        if seen > tokens {
            return false;
        }
        if matches!(character, '{' | '[') {
            deep += 1;
            if deep > depth {
                return false;
            }
        } else if matches!(character, '}' | ']') {
            deep = deep.saturating_sub(1);
        }
    }
    true
}

/// Take a model's JSON out of whatever it wrapped it in.
///
/// `stripJsonFence` (:106881): an opening fence with or without a `json` tag
/// comes off when the text ends in one, and then the span from the first `{` to
/// the last `}` is taken. The second half is what saves the common failure —
/// a model that says "Here is the JSON:" before the object.
pub fn strip_json_fence(raw: &str) -> String {
    let mut text = raw.trim();
    if let Some(inner) = fenced_body(text) {
        text = inner.trim();
    }
    let start = text.find('{');
    let end = text.rfind('}');
    match (start, end) {
        (Some(start), Some(end)) if end > start => text[start..=end].to_string(),
        _ => text.to_string(),
    }
}

fn fenced_body(text: &str) -> Option<&str> {
    if !text.ends_with("```") {
        return None;
    }
    // ```json first, then a bare fence — the tag is case-insensitive in the
    // source, and a model writes ```JSON often enough to matter.
    let after_open = if text.len() >= 7 && text[..7].eq_ignore_ascii_case("```json") {
        line_break_end(text, 7).or_else(|| line_break_end(text, 3))?
    } else {
        line_break_end(text, 3)?
    };
    let close = text.len() - 3;
    if close <= after_open {
        return None;
    }
    // The newline in front of the closing fence belongs to the fence, not to
    // the body.
    let mut end = close;
    if text[..end].ends_with('\n') {
        end -= 1;
        if text[..end].ends_with('\r') {
            end -= 1;
        }
    } else if text[..end].ends_with('\r') {
        end -= 1;
    } else {
        return None;
    }
    (end >= after_open).then(|| &text[after_open..end])
}

fn line_break_end(text: &str, index: usize) -> Option<usize> {
    match text.as_bytes().get(index)? {
        b'\n' => Some(index + 1),
        b'\r' => Some(if text.as_bytes().get(index + 1) == Some(&b'\n') {
            index + 2
        } else {
            index + 1
        }),
        _ => None,
    }
}

/// What a generated pull request came back as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestFields {
    pub base: String,
    pub title: String,
    pub body: String,
    pub draft: bool,
}

/// Read the model's answer, field by field, falling back to what was already
/// in the dialog.
///
/// `parseGeneratedPullRequestFields` (:106919) verbatim, and every fallback is
/// per-FIELD rather than all-or-nothing: an answer with a good title and a
/// missing draft flag is mostly right, and throwing it away for the one absent
/// key would waste a minute of somebody's model time.
///
/// # Errors
///
/// The text is not JSON, is not an object, or is structurally larger than a
/// pull request has any reason to be.
pub fn parse_pull_request_fields(
    raw: &str,
    fallback: &PullRequestContext,
) -> Result<PullRequestFields, String> {
    let content = strip_json_fence(raw);
    if !structure_within_limits(&content, PR_STRUCTURAL_TOKENS, PR_NESTING_DEPTH) {
        return Err("생성된 JSON이 너무 복잡합니다".into());
    }
    let parsed: serde_json::Value =
        serde_json::from_str(&content).map_err(|_| "JSON을 읽지 못했습니다".to_string())?;
    let Some(record) = parsed.as_object() else {
        return Err("JSON 객체가 아닙니다".into());
    };
    let said = |key: &str| record.get(key).and_then(|one| one.as_str());
    let base = said("base")
        .map(|one| one.trim().to_string())
        .unwrap_or_else(|| fallback.base.clone());
    let title = said("title")
        .map(str::trim)
        .filter(|one| !one.is_empty())
        .map(|one| one.trim_end_matches('.').to_string())
        .unwrap_or_else(|| fallback.current_title.trim().to_string());
    let body = said("body")
        .map(|one| one.trim_end().to_string())
        .unwrap_or_else(|| fallback.current_body.clone());
    let draft = record
        .get("draft")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(fallback.current_draft);
    Ok(PullRequestFields {
        base: if base.is_empty() {
            fallback.base.clone()
        } else {
            base
        },
        // Orca's own last resort, and it is the same words the commit-message
        // splitter falls back to.
        title: if title.is_empty() {
            "Update project files".to_string()
        } else {
            title
        },
        body,
        draft,
    })
}

/// What the branch-name generator is told.
///
/// The work, not the diff. Orca names a branch from the FIRST PROMPT of the
/// session and the agent's opening answer (`buildBranchNamePrompt`,
/// branch-name-from-work-Ddry6EUD.js:208) — because the branch is named when
/// the work starts, before there is a diff to read.
#[derive(Debug, Clone, Default)]
pub struct BranchNameContext {
    pub first_prompt: String,
    pub assistant_message: Option<String>,
}

/// The branch-name prompt, verbatim.
///
/// The one-word difference between the two openings is Orca's own: with a
/// person's extra instruction the word "short" comes out, because their
/// instruction may be exactly a request for something longer.
pub fn branch_name_prompt(context: &BranchNameContext, custom_prompt: &str) -> String {
    let mut sections: Vec<String> = Vec::new();
    let said = custom_prompt.trim();
    if !said.is_empty() {
        sections.push(said.to_string());
        sections.push(String::new());
    }
    sections.push(
        if said.is_empty() {
            "Generate a short git branch name that summarizes the coding task described below."
        } else {
            "Generate a git branch name that summarizes the coding task described below."
        }
        .to_string(),
    );
    sections.push("Output ONLY the branch name on a single line, nothing else.".to_string());
    sections.push(String::new());
    sections.push("User request:".to_string());
    sections.push(context.first_prompt.trim().to_string());
    if let Some(assistant) = context
        .assistant_message
        .as_deref()
        .map(str::trim)
        .filter(|one| !one.is_empty())
    {
        sections.push(String::new());
        sections.push("Agent's initial response:".to_string());
        sections.push(assistant.to_string());
    }
    sections.join("\n")
}

/// `MAX_BRANCH_NAME_WORDS` (out/main/index.js:107549).
pub const MAX_BRANCH_NAME_WORDS: usize = 4;

/// Turn whatever the model said into a branch name.
///
/// `sanitizeBranchSlug` verbatim: lowercase, every run of non-alphanumerics
/// becomes one `-`, empties dropped, and at most `max_words` words kept. The
/// word cap is what stops a model's helpful sentence becoming a branch nobody
/// can type.
pub fn sanitize_branch_slug(raw: &str, max_words: usize) -> String {
    raw.to_lowercase()
        .chars()
        .map(|one| {
            if one.is_ascii_alphanumeric() {
                one
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|word| !word.is_empty())
        .take(max_words)
        .collect::<Vec<_>>()
        .join("-")
}

/// Take a configured prefix back off a slug.
///
/// `stripConfiguredBranchPrefix`: a person whose branches are all `joe/…` has
/// that prefix added by the branch machinery, and a model that helpfully
/// included it too would produce `joe/joe-fix-login`. A slug that is ONLY the
/// prefix becomes empty, which the caller reads as "the model said nothing".
pub fn strip_configured_branch_prefix(slug: &str, prefix: &str) -> String {
    let prefix_slug = sanitize_branch_slug(prefix.trim(), usize::MAX);
    if prefix_slug.is_empty() {
        return slug.to_string();
    }
    if slug == prefix_slug {
        return String::new();
    }
    slug.strip_prefix(&format!("{prefix_slug}-"))
        .unwrap_or(slug)
        .to_string()
}

/// A slug as a sentence, for a title beside the branch (`humanizeBranchSlug`).
pub fn humanize_branch_slug(slug: &str) -> String {
    let joined = slug
        .split('-')
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let mut chars = joined.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// One saved way of launching an action — Orca's action recipe
/// (`normalizeCompleteRecipe`, source-control-ai-recipe-save-DBaL-85b.js:876):
/// which agent, what wraps the prompt, and what extra arguments ride the CLI.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchRecipe {
    /// `None` follows the default-agent chain rather than naming one.
    #[serde(default)]
    pub agent: Option<String>,
    /// The command-input template, `{basePrompt}` by default.
    #[serde(default = "default_template")]
    pub template: String,
    /// Extra CLI arguments, whitespace-separated.
    #[serde(default)]
    pub args: String,
}

fn default_template() -> String {
    DEFAULT_COMMAND_TEMPLATE.to_string()
}

impl Default for LaunchRecipe {
    fn default() -> Self {
        Self {
            agent: None,
            template: default_template(),
            args: String::new(),
        }
    }
}

/// Every action a recipe can be filed against — both halves of the family.
///
/// TEXT and LAUNCH ids key the SAME saved recipes in Orca (the note on
/// [`LaunchAction`] says so, and it is the reason both tables carry their ids
/// as protocol rather than as names). A door that knows only one half turns
/// "which action is this" into "which kind of action is this", and the answer
/// somebody saved for `commitMessage` has nowhere to live.
#[must_use]
pub fn is_recipe_action(id: &str) -> bool {
    LaunchAction::from_id(id).is_some() || TextAction::from_id(id).is_some()
}

/// The recipe an action runs, given what each scope had to say about it.
///
/// Orca files a recipe either globally or against one repository, and the
/// repository's answer WINS — that is the whole of the override
/// (`resolveSourceControlActionRecipe`, recipe-save:458-481). Nothing saved
/// anywhere runs the default.
///
/// The distinction with teeth is between a repository that says "the plain
/// default here" and a repository that says nothing at all: `Some(default)`
/// still beats a global recipe, `None` inherits it. A resolver that treated an
/// entry equal to the default as absent would silently drop the one override
/// somebody writes most often — getting their global wrapper OUT of the one
/// repository where it does not belong.
#[must_use]
pub fn resolve_recipe(global: Option<&LaunchRecipe>, repo: Option<&LaunchRecipe>) -> LaunchRecipe {
    repo.or(global).cloned().unwrap_or_default()
}

/// Every recipe saved on this machine, in Orca's two scopes.
///
/// The repository key is a repository, not a checkout: a recipe saved from one
/// worktree is the same repository's recipe in all of them, which is what
/// makes the scope "this repository" rather than "this directory".
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RecipeBook {
    /// Action id → recipe, for every repository that does not override it.
    pub global: BTreeMap<String, LaunchRecipe>,
    /// Repository → (action id → recipe).
    pub by_repo: BTreeMap<String, BTreeMap<String, LaunchRecipe>>,
}

impl RecipeBook {
    /// Read the saved file, promoting the shape written before scopes existed.
    ///
    /// The first file this product wrote was a FLAT map of action id → recipe,
    /// because there was one scope and it needed no name. Read as a book that
    /// file has neither key, and serde — which ignores what it does not know —
    /// would answer "no recipes at all": every recipe somebody saved, gone
    /// without a word on the first launch after an update. So the shape is
    /// decided by looking, and a flat file becomes the global scope.
    ///
    /// The two shapes cannot be confused: no action id is `global` or
    /// `byRepo`, and [`is_recipe_action`] is the list.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
            return Self::default();
        };
        let scoped = value
            .as_object()
            .is_some_and(|held| held.contains_key("global") || held.contains_key("byRepo"));
        if scoped {
            return serde_json::from_value(value).unwrap_or_default();
        }
        Self {
            global: serde_json::from_value(value).unwrap_or_default(),
            by_repo: BTreeMap::new(),
        }
    }

    /// What one action runs in one scope — `None` is the global scope.
    #[must_use]
    pub fn effective(&self, action: &str, scope: Option<&str>) -> LaunchRecipe {
        resolve_recipe(self.global.get(action), self.repo_entry(action, scope))
    }

    fn repo_entry(&self, action: &str, scope: Option<&str>) -> Option<&LaunchRecipe> {
        self.by_repo.get(scope?)?.get(action)
    }

    /// Every action this scope has an answer for, each already resolved.
    ///
    /// Both key sets, because a repository's recipe for an action nobody saved
    /// globally is still an answer this scope has.
    #[must_use]
    pub fn effective_all(&self, scope: Option<&str>) -> BTreeMap<String, LaunchRecipe> {
        let repo = scope.and_then(|repo| self.by_repo.get(repo));
        self.global
            .keys()
            .chain(repo.into_iter().flat_map(BTreeMap::keys))
            .map(|action| (action.clone(), self.effective(action, scope)))
            .collect()
    }

    /// File one action's recipe in one scope — or clear it, when it says
    /// nothing this scope did not already inherit.
    ///
    /// Inherited rather than default, because the two are the same thing at
    /// global scope and are not at repository scope. A repository saving the
    /// plain default while the global recipe wraps every prompt is saying
    /// something — "not here" — and storing it is the only way to say it. What
    /// gets dropped is the entry that changes nothing: a file of those is a
    /// file of noise, and every reader has to decide again that they mean
    /// nothing.
    pub fn set(&mut self, action: &str, recipe: &LaunchRecipe, scope: Option<&str>) {
        let inherited = match scope {
            None => LaunchRecipe::default(),
            Some(_) => self.global.get(action).cloned().unwrap_or_default(),
        };
        let Some(repo) = scope else {
            if *recipe == inherited {
                self.global.remove(action);
            } else {
                self.global.insert(action.to_string(), recipe.clone());
            }
            return;
        };
        if *recipe == inherited {
            if let Some(held) = self.by_repo.get_mut(repo) {
                held.remove(action);
                // An empty repository is a repository with no recipes, which
                // is what an absent key already says.
                if held.is_empty() {
                    self.by_repo.remove(repo);
                }
            }
            return;
        }
        self.by_repo
            .entry(repo.to_string())
            .or_default()
            .insert(action.to_string(), recipe.clone());
    }
}

/// What the agent choice came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentChoice {
    /// Launch this one.
    Agent(String),
    /// The SAVED agent is not on this machine — a hard stop, never a
    /// fallback. Orca refuses here too
    /// (`copy.savedAgentUnavailable`, SourceControl-e46DLHZz.js:5807-5809):
    /// somebody chose that agent for this action on purpose, and quietly
    /// running a different one substitutes a decision nobody made.
    SavedUnavailable(String),
    /// Nothing is installed at all.
    NoneInstalled,
}

/// Pick the agent a launch action runs — `pickSourceControlLaunchAgent`
/// (recipe-save:609-618) plus the direct road's hard block (:5807).
///
/// The order: a SAVED agent wins or blocks — no fallback past an explicit
/// choice. With nothing saved, the default agent when installed, else the
/// first installed one, else nothing.
pub fn pick_launch_agent(
    saved: Option<&str>,
    default_agent: Option<&str>,
    installed: &[String],
) -> AgentChoice {
    if let Some(saved) = saved {
        return if installed.iter().any(|one| one == saved) {
            AgentChoice::Agent(saved.to_string())
        } else {
            AgentChoice::SavedUnavailable(saved.to_string())
        };
    }
    if let Some(default) = default_agent
        && installed.iter().any(|one| one == default)
    {
        return AgentChoice::Agent(default.to_string());
    }
    match installed.first() {
        Some(first) => AgentChoice::Agent(first.clone()),
        None => AgentChoice::NoneInstalled,
    }
}

/// Split a recipe's CLI arguments for an argv spawn.
///
/// Whitespace only, and QUOTES ARE REFUSED rather than parsed: this product
/// spawns the agent directly — argv, no shell — so there is no quoting layer,
/// and accepting `--message "hello world"` while splitting it into three
/// arguments would run something visibly different from what was typed. Orca
/// validates its suffix for the same reason (`planAgentCliArgsSuffix`).
pub fn split_agent_args(args: &str) -> Result<Vec<String>, String> {
    if args
        .chars()
        .any(|one| one == '"' || one == '\'' || one == '\\')
    {
        return Err(
            "인용부호는 지원하지 않습니다 — 인수는 공백으로 나뉘어 그대로 전달됩니다".to_string(),
        );
    }
    Ok(args.split_whitespace().map(str::to_string).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A saved agent that is gone BLOCKS — it never quietly becomes another
    /// agent, because somebody chose it for this action on purpose.
    #[test]
    fn a_saved_agent_that_is_gone_blocks_rather_than_becoming_another() {
        let installed = vec!["claude".to_string(), "codex".to_string()];
        assert_eq!(
            pick_launch_agent(Some("missing-agent"), Some("claude"), &installed),
            AgentChoice::SavedUnavailable("missing-agent".to_string()),
        );
        assert_eq!(
            pick_launch_agent(Some("codex"), Some("claude"), &installed),
            AgentChoice::Agent("codex".to_string()),
        );
        // Nothing saved: the default, then the first installed, then nothing.
        assert_eq!(
            pick_launch_agent(None, Some("codex"), &installed),
            AgentChoice::Agent("codex".to_string()),
        );
        assert_eq!(
            pick_launch_agent(None, Some("missing-agent"), &installed),
            AgentChoice::Agent("claude".to_string()),
        );
        assert_eq!(
            pick_launch_agent(None, None, &[]),
            AgentChoice::NoneInstalled,
        );
        // The block outranks an empty machine's answer too: the message must
        // name the missing agent, not shrug about agents in general.
        assert_eq!(
            pick_launch_agent(Some("claude"), None, &[]),
            AgentChoice::SavedUnavailable("claude".to_string()),
        );
    }

    /// The args ride an argv spawn — no shell, so no quoting layer exists,
    /// and pretending one does would run something different from what was
    /// typed.
    #[test]
    fn recipe_args_split_on_whitespace_and_refuse_quotes() {
        assert_eq!(
            split_agent_args("--model sonnet  --verbose").unwrap(),
            vec!["--model", "sonnet", "--verbose"],
        );
        assert_eq!(split_agent_args("").unwrap(), Vec::<String>::new());
        assert_eq!(split_agent_args("   ").unwrap(), Vec::<String>::new());
        assert!(split_agent_args(r#"--message "hello world""#).is_err());
        assert!(split_agent_args("--path 'a b'").is_err());
        assert!(split_agent_args(r"--path a\ b").is_err());
    }

    /// A recipe read from an old or hand-edited file settles to the defaults
    /// field by field, and the default template is the unwrapped prompt.
    #[test]
    fn a_recipe_defaults_to_the_unwrapped_prompt() {
        let bare: LaunchRecipe = serde_json::from_str("{}").expect("empty recipe");
        assert_eq!(bare, LaunchRecipe::default());
        assert_eq!(bare.template, "{basePrompt}");
        assert_eq!(bare.agent, None);

        let partial: LaunchRecipe =
            serde_json::from_str(r#"{"agent":"codex"}"#).expect("partial recipe");
        assert_eq!(partial.agent.as_deref(), Some("codex"));
        assert_eq!(partial.template, "{basePrompt}");
    }

    /// A recipe with something in it, for the scope tests below.
    fn wrapped(agent: &str) -> LaunchRecipe {
        LaunchRecipe {
            agent: Some(agent.to_string()),
            ..LaunchRecipe::default()
        }
    }

    /// The repository wins whenever it has an answer, and only then.
    ///
    /// The last case is the one a cleanup breaks: a repository entry that
    /// happens to equal the default is still an ANSWER. Reading it as absence
    /// would turn "the plain default here" back into whatever the global
    /// recipe says, which is the one thing the person who wrote it was trying
    /// to get away from.
    #[test]
    fn a_repository_recipe_beats_the_global_one_and_absence_inherits() {
        let global = wrapped("claude");
        let repo = wrapped("codex");
        assert_eq!(
            resolve_recipe(Some(&global), Some(&repo)).agent.as_deref(),
            Some("codex"),
        );
        assert_eq!(
            resolve_recipe(Some(&global), None).agent.as_deref(),
            Some("claude"),
        );
        assert_eq!(resolve_recipe(None, None), LaunchRecipe::default());
        assert_eq!(
            resolve_recipe(None, Some(&repo)).agent.as_deref(),
            Some("codex"),
        );
        // Explicitly back to the default is not the same as unset.
        let plain = LaunchRecipe::default();
        assert_eq!(resolve_recipe(Some(&global), Some(&plain)), plain);
    }

    /// The book stores what a scope says, including a repository saying the
    /// plain default — and drops only what changes nothing.
    #[test]
    fn a_repository_can_say_the_plain_default_and_be_heard() {
        let mut book = RecipeBook::default();
        book.set("fixChecks", &wrapped("claude"), None);
        book.set("fixChecks", &LaunchRecipe::default(), Some("/repo"));
        assert_eq!(
            book.effective("fixChecks", Some("/repo")),
            LaunchRecipe::default(),
            "the repository's explicit default lost to the global recipe",
        );
        assert_eq!(
            book.effective("fixChecks", None).agent.as_deref(),
            Some("claude"),
        );
        // Every other repository still inherits, and so does an action nobody
        // scoped.
        assert_eq!(
            book.effective("fixChecks", Some("/other")).agent.as_deref(),
            Some("claude"),
        );
        assert_eq!(
            book.effective("resolveConflicts", Some("/repo")),
            LaunchRecipe::default(),
        );
        // A scope's view carries both key sets, resolved.
        book.set("resolveConflicts", &wrapped("codex"), Some("/repo"));
        let seen = book.effective_all(Some("/repo"));
        assert_eq!(seen.len(), 2);
        assert_eq!(seen["fixChecks"], LaunchRecipe::default());
        assert_eq!(seen["resolveConflicts"].agent.as_deref(), Some("codex"));
        assert_eq!(
            book.effective_all(None)["fixChecks"].agent.as_deref(),
            Some("claude"),
        );
        // Saving what the scope already inherits removes the entry rather than
        // leaving a row that says nothing — and the repository goes with its
        // last recipe.
        book.set("resolveConflicts", &wrapped("claude"), None);
        book.set("resolveConflicts", &wrapped("claude"), Some("/repo"));
        assert!(book.by_repo.contains_key("/repo"));
        book.set("fixChecks", &wrapped("claude"), Some("/repo"));
        assert!(!book.by_repo.contains_key("/repo"));
        book.set("fixChecks", &LaunchRecipe::default(), None);
        assert!(!book.global.contains_key("fixChecks"));
    }

    /// The file written before scopes existed is read as the global scope.
    ///
    /// Serde ignores keys it does not know, so a book reading a flat file
    /// would report no recipes at all — every saved recipe gone, quietly, on
    /// the first launch after an update.
    #[test]
    fn a_file_written_before_the_second_scope_is_read_as_global() {
        let flat =
            r#"{"fixChecks":{"agent":"codex","template":"{basePrompt}","args":"--model gpt-5"}}"#;
        let book = RecipeBook::parse(flat);
        assert_eq!(
            book.effective("fixChecks", None).agent.as_deref(),
            Some("codex"),
            "the pre-scope file was read as an empty book",
        );
        assert_eq!(book.effective("fixChecks", None).args, "--model gpt-5");
        assert!(book.by_repo.is_empty());

        // And the scoped shape round-trips, repository and all.
        let mut written = RecipeBook::default();
        written.set("commitMessage", &wrapped("claude"), Some("/repo"));
        let text = serde_json::to_string(&written).expect("a book serializes");
        assert_eq!(RecipeBook::parse(&text), written);
        assert!(text.contains("byRepo"));

        // Nonsense is an empty book, not a panic: this file is hand-editable.
        assert_eq!(RecipeBook::parse("not json"), RecipeBook::default());
        assert_eq!(RecipeBook::parse("[]"), RecipeBook::default());
    }

    /// Both halves of the family can hold a recipe — Orca keys them by the
    /// same ids, so a save door that knows only the launch half has nowhere to
    /// put the answer somebody chose for `commitMessage`.
    #[test]
    fn every_action_in_the_family_can_hold_a_recipe() {
        for action in TextAction::ALL.iter().map(|one| one.id()) {
            assert!(is_recipe_action(action), "{action} cannot hold a recipe");
        }
        for action in LaunchAction::ALL.iter().map(|one| one.id()) {
            assert!(is_recipe_action(action), "{action} cannot hold a recipe");
        }
        assert!(!is_recipe_action("nope"));
        assert!(!is_recipe_action("global"));
        assert!(!is_recipe_action("byRepo"));
    }

    #[test]
    fn the_action_tables_are_orcas_own_strings() {
        // Protocol, not names: these key the per-action templates and the
        // recipes a person saves.
        assert_eq!(
            TextAction::ALL
                .iter()
                .map(|one| one.id())
                .collect::<Vec<_>>(),
            ["commitMessage", "pullRequest", "branchName"]
        );
        assert_eq!(
            LaunchAction::ALL
                .iter()
                .map(|one| one.id())
                .collect::<Vec<_>>(),
            [
                "fixCommitFailure",
                "fixPushFailure",
                "fixChecks",
                "resolveConflicts",
                "resolveComments"
            ]
        );
        assert_eq!(
            TextAction::from_id("pullRequest"),
            Some(TextAction::PullRequest)
        );
        assert_eq!(TextAction::from_id("nope"), None);
        assert_eq!(
            LaunchAction::from_id("resolveComments"),
            Some(LaunchAction::ResolveComments)
        );
    }

    #[test]
    fn a_template_takes_both_spellings_and_keeps_what_it_does_not_know() {
        let vars = [("basePrompt", "DO THE THING"), ("firstPrompt", "fix login")];
        assert_eq!(
            render_command_template("{basePrompt}", &vars),
            "DO THE THING"
        );
        assert_eq!(
            render_command_template("before {{basePrompt}} after", &vars),
            "before DO THE THING after"
        );
        assert_eq!(
            render_command_template("{firstPrompt} → {basePrompt}", &vars),
            "fix login → DO THE THING"
        );
        // An unknown variable survives in the spelling it was written in — the
        // person can see their mistake in the preview, which deleting it hides.
        assert_eq!(
            render_command_template("{nope} {{alsoNope}}", &vars),
            "{nope} {{alsoNope}}"
        );
        // A brace that opens nothing is text.
        assert_eq!(render_command_template("100% {", &vars), "100% {");
        assert_eq!(render_command_template("", &vars), "");
    }

    #[test]
    fn a_cut_section_says_what_it_cost() {
        assert_eq!(limit_section("short", 10), "short");
        let long = "x".repeat(30);
        let cut = limit_section(&long, 10);
        assert!(cut.starts_with(&"x".repeat(10)));
        assert!(
            cut.contains("[truncated: 20 characters omitted]"),
            "a silently cut section makes a model describe part of a change as \
             if it were the whole one: {cut}"
        );
        // Multi-byte text is cut on a character, never mid-character.
        let hangul = "가".repeat(30);
        let cut = limit_section(&hangul, 10);
        assert!(cut.starts_with(&"가".repeat(10)));
        assert!(cut.contains("20 characters omitted"));
    }

    fn context() -> PullRequestContext {
        PullRequestContext {
            branch: Some("feature/login".into()),
            base: "main".into(),
            current_title: "Draft title".into(),
            current_body: "## Checklist\n- [ ] tests".into(),
            current_draft: false,
            commit_summary: "- Add login".into(),
            change_summary: "src/login.rs | 4 ++".into(),
            patch: "diff --git a/src/login.rs".into(),
        }
    }

    #[test]
    fn the_pull_request_prompt_keeps_the_two_rules_that_protect_a_template() {
        let prompt = pull_request_prompt(&context(), "");
        // The rule that stops a repository's PR template being replaced, and
        // the one that stops its unknown items being deleted.
        assert!(prompt.contains(
            "preserve its headings, required sections, and checklists while \
             filling relevant sections from the branch changes"
        ));
        assert!(prompt.contains("Leave genuinely unknown template items as TODO or unchecked"));
        // The shape it must answer in, said twice — once up front and once as
        // the last thing the model reads.
        assert!(prompt.contains(r#"{"base":"branch-name","title":"short title""#));
        assert!(prompt.trim_end().ends_with("No prose or code fences."));
        assert!(prompt.contains("Head branch: feature/login"));
        assert!(prompt.contains("Current base: main"));
        assert!(prompt.contains("Current draft: false"));
        assert!(prompt.contains("```diff\ndiff --git a/src/login.rs\n```"));
    }

    #[test]
    fn an_empty_field_reads_as_empty_and_a_detached_head_says_so() {
        let mut bare = PullRequestContext {
            base: "main".into(),
            ..PullRequestContext::default()
        };
        let prompt = pull_request_prompt(&bare, "");
        assert!(prompt.contains("Current title: (empty)"));
        assert!(prompt.contains("Current description: (empty)"));
        assert!(prompt.contains("Commits:\n(none)"));
        assert!(prompt.contains("Changed files:\n(none)"));
        // A branch is optional and the prompt has a word for that rather than
        // a blank the model has to guess at.
        assert!(prompt.contains("Head branch: (detached)"));
        bare.branch = Some("here".into());
        assert!(pull_request_prompt(&bare, "").contains("Head branch: here"));
    }

    #[test]
    fn a_persons_own_instruction_rides_between_the_context_and_the_requirement() {
        let prompt = pull_request_prompt(&context(), "  mention the migration  ");
        let extra = prompt
            .find("Additional user prompt:")
            .expect("the instruction was dropped");
        let patch = prompt.find("Patch:").expect("no patch section");
        let tail = prompt
            .find("Final output requirement:")
            .expect("no requirement");
        assert!(patch < extra && extra < tail, "the instruction moved");
        assert!(prompt.contains("mention the migration"));
    }

    #[test]
    fn the_answer_comes_out_of_whatever_the_model_wrapped_it_in() {
        let want = r##"{"base":"main","title":"Add login","body":"# Summary","draft":false}"##;
        for wrapped in [
            want.to_string(),
            format!("```json\n{want}\n```"),
            format!("```JSON\n{want}\n```"),
            format!("```\n{want}\n```"),
            format!("```json\r\n{want}\r\n```"),
            // The common failure: a sentence in front of the object.
            format!("Here is the JSON:\n{want}"),
            format!("  {want}  "),
        ] {
            assert_eq!(strip_json_fence(&wrapped), want, "failed on: {wrapped}");
        }
        // Nothing object-shaped in it — handed on as it is, for the parser to
        // refuse with its own message.
        assert_eq!(strip_json_fence("no json here"), "no json here");
    }

    #[test]
    fn every_field_falls_back_on_its_own() {
        let held = context();
        // A whole answer.
        let full = parse_pull_request_fields(
            r##"{"base":"develop","title":"Add login.","body":"# Summary\n\ndone  ","draft":true}"##,
            &held,
        )
        .expect("parse");
        assert_eq!(full.base, "develop");
        // The trailing period comes off the title, as it does for a commit
        // subject.
        assert_eq!(full.title, "Add login");
        assert_eq!(full.body, "# Summary\n\ndone");
        assert!(full.draft);

        // A partial answer keeps what the dialog already had, field by field —
        // throwing the whole thing away for one absent key would waste a
        // minute of somebody's model time.
        let partial = parse_pull_request_fields(r#"{"title":"Only this"}"#, &held).expect("parse");
        assert_eq!(partial.base, "main");
        assert_eq!(partial.body, held.current_body);
        assert!(!partial.draft);
        assert_eq!(partial.title, "Only this");

        // An empty title falls back, and a fallback that is also empty gets
        // Orca's own last resort rather than a blank PR title.
        let blank = parse_pull_request_fields(r#"{"title":"   "}"#, &held).expect("parse");
        assert_eq!(blank.title, "Draft title");
        let nothing = PullRequestContext {
            base: "main".into(),
            ..PullRequestContext::default()
        };
        let rescued = parse_pull_request_fields(r#"{"title":""}"#, &nothing).expect("parse");
        assert_eq!(rescued.title, "Update project files");
    }

    #[test]
    fn a_shape_this_could_never_be_is_refused_before_it_is_parsed() {
        let held = context();
        assert!(parse_pull_request_fields("not json", &held).is_err());
        assert!(parse_pull_request_fields("[1,2,3]", &held).is_err());
        // Deeper than a pull request has any reason to be. Refused on
        // STRUCTURE, before a parser recurses through it — this text came from
        // a model, and "the parser will cope" is how a stack overflows.
        let deep = format!(
            "{}{}",
            "{\"a\":".repeat(40),
            "1".to_string() + &"}".repeat(40)
        );
        assert!(parse_pull_request_fields(&deep, &held).is_err());
        // And wider than one.
        let wide = format!(
            "{{{}}}",
            (0..40)
                .map(|at| format!("\"k{at}\":{at}"))
                .collect::<Vec<_>>()
                .join(",")
        );
        assert!(parse_pull_request_fields(&wide, &held).is_err());
        // A brace inside a STRING is not structure — the counter must not be
        // fooled by a body that talks about JSON.
        let talky =
            r#"{"base":"main","title":"t","body":"use {{a:{b:{c:1}}}} here","draft":false}"#;
        let read = parse_pull_request_fields(talky, &held).expect("a string is not structure");
        assert_eq!(read.body, "use {{a:{b:{c:1}}}} here");
    }

    #[test]
    fn a_branch_is_named_from_the_work_rather_than_from_a_diff() {
        let held = BranchNameContext {
            first_prompt: "  Fix the login redirect loop  ".into(),
            assistant_message: Some("I will look at the session cookie.".into()),
        };
        let prompt = branch_name_prompt(&held, "");
        assert!(prompt.starts_with("Generate a short git branch name"));
        assert!(prompt.contains("Output ONLY the branch name on a single line, nothing else."));
        assert!(prompt.contains("User request:\nFix the login redirect loop"));
        assert!(prompt.contains("Agent's initial response:\nI will look at the session cookie."));

        // With a person's own instruction the word "short" goes — their
        // instruction may be exactly a request for something longer.
        let asked = branch_name_prompt(&held, "use my initials");
        assert!(asked.starts_with("use my initials\n\nGenerate a git branch name"));
        assert!(!asked.contains("a short git branch name"));

        // No agent reply yet is the ordinary case: the branch is named when the
        // work starts.
        let alone = BranchNameContext {
            first_prompt: "Fix login".into(),
            assistant_message: None,
        };
        assert!(!branch_name_prompt(&alone, "").contains("Agent's initial response"));
    }

    #[test]
    fn a_slug_is_short_typable_and_never_doubles_the_prefix() {
        assert_eq!(
            sanitize_branch_slug("Fix the login redirect loop now", MAX_BRANCH_NAME_WORDS),
            "fix-the-login-redirect"
        );
        assert_eq!(sanitize_branch_slug("  Add  __LOGIN__  ", 9), "add-login");
        assert_eq!(sanitize_branch_slug("!!!", 4), "");
        // The prefix the branch machinery adds is taken back off, so a model
        // that helpfully included it does not produce `joe/joe-fix-login`.
        assert_eq!(
            strip_configured_branch_prefix("joe-fix-login", "joe/"),
            "fix-login"
        );
        assert_eq!(
            strip_configured_branch_prefix("fix-login", "joe/"),
            "fix-login"
        );
        assert_eq!(strip_configured_branch_prefix("fix-login", ""), "fix-login");
        // Only the prefix is nothing at all, which the caller reads as "the
        // model said nothing".
        assert_eq!(strip_configured_branch_prefix("joe", "joe/"), "");
        assert_eq!(humanize_branch_slug("fix-login-loop"), "Fix login loop");
        assert_eq!(humanize_branch_slug(""), "");
    }
}
