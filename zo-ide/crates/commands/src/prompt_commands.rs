use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const MAX_PROMPT_COMMANDS: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptCommandDef {
    pub name: String,
    pub description: Option<String>,
    pub argument_hint: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// `allowed-tools` frontmatter, split into individual tool specs (e.g.
    /// `Bash(git diff:*)`, `Read`). Empty when the command does not scope the
    /// turn; the dispatcher converts this into the active allow-list.
    pub allowed_tools: Vec<String>,
    pub body: String,
    pub path: PathBuf,
    /// Where the file was found, which decides whether its `!` commands
    /// also need the person's word for the folder that holds it.
    pub scope: PromptCommandScope,
    /// The `commands` folder the file was found under (`<folder>/.zo/commands`
    /// for a workspace file).
    pub root: PathBuf,
    /// Files further down the search order with the same name, which this one
    /// keeps from running (`/name` always means this file).
    pub shadows: Vec<PathBuf>,
}

/// A command body expanded, and one line per `!` command and `@path` it met
/// saying what came of it (`! git status → exit 0`, `@notes.md → read`), for
/// the screen to show before the turn goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedPrompt {
    pub text: String,
    pub trace: Vec<String>,
}

/// Where a command file was found.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PromptCommandScope {
    /// `.zo/commands` in the working folder or a folder above it: whoever
    /// wrote the repository wrote the file.
    #[default]
    Workspace,
    /// `commands` in zo's home: the person's own file.
    ZoHome,
}

/// What the session lets a command file's `!` commands run, decided once by
/// the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InlineShellGate<'a> {
    /// The session is read-only: no `!` command runs.
    pub read_only: bool,
    /// What the person has said for the folder a workspace file is in
    /// ([`PromptCommandDef::workspace_folder`]). A file in zo's home does
    /// not ask.
    pub workspace: WorkspaceTrust,
    /// The person's permission rules, the ones the session's Bash answers
    /// to: every `!` command a deny or an ask rule matches stays unrun.
    /// `Err` carries why they could not be read, and then none runs.
    pub rules: Result<&'a runtime::RuntimePermissionRuleConfig, &'a str>,
    /// How long one `!` line may run before it is stopped, in milliseconds;
    /// `None` leaves it to the Bash tool's own limit (120 s). A screen that
    /// expands a command on its own loop is held still for as long as a line
    /// runs — keys, repaint and Esc wait — so it names a short one; a pipe has
    /// no screen to hold and keeps the Bash tool's (t-81313 P3-6).
    pub line_limit_ms: Option<u64>,
}

/// The person's word for the folder that holds a workspace command file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceTrust {
    /// The person trusted the folder, or named the session's mode: its `!`
    /// commands run what `allowed-tools` grants.
    Trusted,
    /// Nothing the person said covers the folder: no `!` command runs.
    Untrusted,
    /// A pipe whose mode the person did not name: nobody is there to answer
    /// for the folder, so no `!` command runs, whatever the trust store says.
    /// The careful side, on purpose (t-81313 P3-4, m-81282): a record of trust
    /// was given at another time, to what the folder held then, and a run with
    /// no head has nobody present to see what it runs now. The person who
    /// wants a repository's `!` lines in a pipe names the mode.
    UnnamedPipe,
}

impl PromptCommandDef {
    /// The folder whose `.zo/commands` holds a workspace file — the one the
    /// person's trust is asked for. `None` for a file in zo's home.
    #[must_use]
    pub fn workspace_folder(&self) -> Option<&Path> {
        match self.scope {
            PromptCommandScope::Workspace => self.root.parent()?.parent(),
            PromptCommandScope::ZoHome => None,
        }
    }

    #[must_use]
    pub fn summary(&self) -> String {
        self.description
            .clone()
            .unwrap_or_else(|| format!("Prompt command from {}", self.path.display()))
    }

    /// Expand the command body into the prompt that is queued as the next turn,
    /// against `base_dir` — the session's working folder, where `!cmd` runs and
    /// `@path` resolves — under what `gate` lets `!` commands run: none while
    /// read-only, and none from a workspace file in a folder the person has not
    /// trusted, whatever `allowed-tools` grants.
    ///
    /// In addition to `$ARGUMENTS`/`$N` substitution this resolves the two
    /// dynamic-content forms Claude Code supports: `` !`cmd` `` (and a line that
    /// begins with `!`) inlines the command's stdout, and `@path` embeds the
    /// referenced file. A failed command or missing file degrades to an inline
    /// notice rather than aborting the render. A `!` command runs only when the
    /// file's `allowed-tools` grants it Bash.
    #[must_use]
    pub fn render_prompt_as(&self, args: &str, base_dir: &Path, gate: InlineShellGate) -> String {
        self.render_prompt_traced(args, base_dir, gate).text
    }

    /// [`Self::render_prompt_in`] against the process's working folder. This
    /// crate's tests only (t-81313 P3-2): no session calls it, and it passes
    /// none of the doors a `!` line answers to.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn render_prompt(&self, args: &str) -> String {
        let base_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        self.render_prompt_in(args, &base_dir)
    }

    /// [`Self::render_prompt_as`] with every door open: the folder trusted, the
    /// session not read-only, no permission rule. It is the render of a file
    /// whose `!` lines answer to `allowed-tools` alone, which no session's
    /// is — a session renders through [`Self::render_prompt_as`] with the gate
    /// it built (t-81313 P3-2). This crate's tests only.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn render_prompt_in(&self, args: &str, base_dir: &Path) -> String {
        let no_rules = runtime::RuntimePermissionRuleConfig::default();
        let gate = InlineShellGate {
            read_only: false,
            workspace: WorkspaceTrust::Trusted,
            rules: Ok(&no_rules),
            line_limit_ms: None,
        };
        self.render_prompt_as(args, base_dir, gate)
    }

    /// [`Self::render_prompt_as`] with the line for each `!` command and
    /// `@path` the body met ([`RenderedPrompt::trace`]).
    #[must_use]
    pub fn render_prompt_traced(
        &self,
        args: &str,
        base_dir: &Path,
        gate: InlineShellGate,
    ) -> RenderedPrompt {
        let workspace = self.scope == PromptCommandScope::Workspace;
        let folder = self.workspace_folder().unwrap_or(base_dir);
        let shell = InlineShell {
            allowed_tools: &self.allowed_tools,
            read_only: gate.read_only,
            workspace: if workspace { gate.workspace } else { WorkspaceTrust::Trusted },
            folder,
            rules: gate.rules,
            confined: workspace,
            line_limit_ms: gate.line_limit_ms,
        };
        let mut trace = Vec::new();
        let text = expand_command_template(&self.body, args, base_dir, &shell, &mut trace);
        RenderedPrompt { text, trace }
    }
}

#[must_use]
pub fn find_prompt_command<'a>(
    commands: &'a [PromptCommandDef],
    name: &str,
) -> Option<&'a PromptCommandDef> {
    let needle = name.trim().trim_start_matches('/');
    commands
        .iter()
        .find(|command| command.name.eq_ignore_ascii_case(needle))
}

/// Every command file `cwd` can call. The person's own, in zo's home, come
/// first and keep their names: a repository cannot take over a name the
/// person already types. Then `.zo/commands` in `cwd` and each folder above
/// it, nearest first, up to the top of the repository or short of the
/// person's home (`command_folders`). A file whose name is taken is not
/// callable; the one that took it lists it in [`PromptCommandDef::shadows`].
#[must_use]
pub fn discover_prompt_commands(cwd: &Path) -> Vec<PromptCommandDef> {
    let personal_roots = personal_command_roots();
    // The walk stops short of the person's home, but zo's home can stand
    // elsewhere in the tree (`ZO_CONFIG_HOME`); a folder that is zo's home was
    // already read as the person's.
    let personal_canonical = personal_roots
        .iter()
        .filter_map(|root| root.canonicalize().ok())
        .collect::<Vec<_>>();
    let is_personal = |root: &Path| {
        personal_roots.iter().any(|personal| personal == root)
            || root
                .canonicalize()
                .is_ok_and(|canonical| personal_canonical.contains(&canonical))
    };
    let workspace_roots = command_folders(cwd, person_home().as_deref())
        .into_iter()
        .map(|dir| dir.join(".zo").join("commands"))
        .filter(|root| !is_personal(root));
    let roots = personal_roots
        .iter()
        .map(|root| (root.clone(), PromptCommandScope::ZoHome))
        .chain(workspace_roots.map(|root| (root, PromptCommandScope::Workspace)));

    let mut seen = BTreeMap::new();
    let mut commands = Vec::new();
    for (root, scope) in roots {
        push_prompt_commands_from_root(&mut commands, &mut seen, &root, scope);
        if commands.len() >= MAX_PROMPT_COMMANDS {
            commands.truncate(MAX_PROMPT_COMMANDS);
            return commands;
        }
    }
    commands
}

/// The folders whose `.zo/commands` a call from `cwd` reads: `cwd` and each
/// folder above it, nearest first, up to the top of the repository — the
/// folder that holds `.git`, which is read — or short of `home`, whichever
/// comes first. A folder further up is not this project's: whoever wrote the
/// repository did not write its files, and the ones above the person's home
/// are every other user's of the machine (t-81313 P3-8). With neither in
/// sight, nothing says where the project ends, and only `cwd` is read.
fn command_folders<'a>(cwd: &'a Path, home: Option<&Path>) -> Vec<&'a Path> {
    let home_canonical = home.and_then(|home| home.canonicalize().ok());
    let is_home = |dir: &Path| {
        home.is_some_and(|home| dir == home)
            || home_canonical
                .as_deref()
                .is_some_and(|home| dir.canonicalize().is_ok_and(|real| real.as_path() == home))
    };
    let mut folders = Vec::new();
    for dir in cwd.ancestors() {
        if is_home(dir) {
            return folders;
        }
        folders.push(dir);
        if dir.join(".git").exists() {
            return folders;
        }
    }
    folders.truncate(1);
    folders
}

/// The person's home folder, the parent of their conventional `~/.zo`.
fn person_home() -> Option<PathBuf> {
    core_types::paths::conventional_config_home().and_then(|zo| zo.parent().map(Path::to_path_buf))
}

/// Zo's per-user command roots, using the canonical global-home precedence.
fn personal_command_roots() -> Vec<PathBuf> {
    core_types::paths::zo_global_config_roots()
        .into_iter()
        .map(|root| root.join("commands"))
        .collect()
}

/// Push the command files under `root`. `seen` maps each name taken so far
/// (lowercase) to its command's index in `commands`.
fn push_prompt_commands_from_root(
    commands: &mut Vec<PromptCommandDef>,
    seen: &mut BTreeMap<String, usize>,
    root: &Path,
    scope: PromptCommandScope,
) {
    let mut files = Vec::new();
    collect_markdown_files(root, &mut files);
    files.sort();

    for path in files {
        let Some(name) = command_name_from_path(root, &path) else {
            continue;
        };
        let key = name.to_ascii_lowercase();
        if let Some(&taken) = seen.get(&key) {
            commands[taken].shadows.push(path);
            continue;
        }
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        let Some(mut command) = parse_prompt_command_file(name, path, &content) else {
            continue;
        };
        command.scope = scope;
        command.root = root.to_path_buf();
        seen.insert(key, commands.len());
        commands.push(command);
    }
}

fn collect_markdown_files(root: &Path, files: &mut Vec<PathBuf>) {
    let Ok(children) = fs::read_dir(root) else {
        return;
    };

    for child in children.filter_map(Result::ok) {
        let path = child.path();
        if path.is_dir() {
            collect_markdown_files(&path, files);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("md") {
            files.push(path);
        }
    }
}

fn command_name_from_path(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    let without_ext = relative.with_extension("");
    let parts = without_ext
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect::<Vec<_>>();
    let name = parts.join("/");
    if name.is_empty() || name.chars().any(char::is_whitespace) {
        None
    } else {
        Some(name)
    }
}

fn parse_prompt_command_file(
    name: String,
    path: PathBuf,
    content: &str,
) -> Option<PromptCommandDef> {
    let (frontmatter, body) = split_frontmatter(content);
    let body = body.trim_start_matches(['\n', '\r']).to_string();
    if body.trim().is_empty() {
        return None;
    }
    let allowed_tools = frontmatter_field(&frontmatter, "allowed-tools")
        .or_else(|| frontmatter_field(&frontmatter, "allowed_tools"))
        .map(|raw| split_allowed_tools(&raw))
        .unwrap_or_default();
    Some(PromptCommandDef {
        name,
        description: frontmatter_field(&frontmatter, "description"),
        argument_hint: frontmatter_field(&frontmatter, "argument-hint")
            .or_else(|| frontmatter_field(&frontmatter, "argument_hint")),
        model: frontmatter_field(&frontmatter, "model"),
        effort: frontmatter_field(&frontmatter, "effort"),
        allowed_tools,
        body,
        path,
        scope: PromptCommandScope::default(),
        root: PathBuf::new(),
        shadows: Vec::new(),
    })
}

/// Split an `allowed-tools` scalar into individual specs. Claude Code uses a
/// comma-separated list (e.g. `Bash(git diff:*), Read`); a `Bash(...)` argument
/// pattern can itself contain commas, so splitting respects parentheses.
fn split_allowed_tools(raw: &str) -> Vec<String> {
    let mut specs = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    for ch in raw.chars() {
        match ch {
            '(' => {
                depth += 1;
                current.push(ch);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                current.push(ch);
            }
            ',' if depth == 0 => {
                push_allowed_tool(&mut specs, &mut current);
            }
            _ => current.push(ch),
        }
    }
    push_allowed_tool(&mut specs, &mut current);
    specs
}

fn push_allowed_tool(specs: &mut Vec<String>, current: &mut String) {
    let spec = current.trim();
    if !spec.is_empty() {
        specs.push(spec.to_string());
    }
    current.clear();
}

fn split_frontmatter(content: &str) -> (Vec<(String, String)>, &str) {
    let Some(rest) = content.strip_prefix("---") else {
        return (Vec::new(), content);
    };
    let Some(rest) = rest
        .strip_prefix('\n')
        .or_else(|| rest.strip_prefix("\r\n"))
    else {
        return (Vec::new(), content);
    };

    let mut offset = content.len() - rest.len();
    let mut fields = Vec::new();
    for line in rest.split_inclusive('\n') {
        let trimmed = line.trim();
        offset += line.len();
        if trimmed == "---" {
            return (fields, &content[offset..]);
        }
        if let Some((key, value)) = trimmed.split_once(':') {
            fields.push((
                key.trim().to_ascii_lowercase(),
                trim_frontmatter_scalar(value.trim()).to_string(),
            ));
        }
    }
    (Vec::new(), content)
}

fn frontmatter_field(fields: &[(String, String)], key: &str) -> Option<String> {
    fields
        .iter()
        .find(|(field, _)| field == key)
        .map(|(_, value)| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn trim_frontmatter_scalar(value: &str) -> &str {
    value.trim().trim_matches('"').trim_matches('\'').trim()
}

fn expand_prompt_arguments(body: &str, args: &str) -> String {
    let positionals = args.split_whitespace().collect::<Vec<_>>();
    let mut output = String::with_capacity(body.len() + args.len());
    let mut chars = body.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '$' {
            output.push(ch);
            continue;
        }
        if consume_literal(&mut chars, "ARGUMENTS") {
            output.push_str(args);
            continue;
        }
        let mut digits = String::new();
        while let Some(next) = chars.peek() {
            if next.is_ascii_digit() {
                digits.push(*next);
                let _ = chars.next();
            } else {
                break;
            }
        }
        if digits.is_empty() {
            output.push('$');
        } else if let Ok(index) = digits.parse::<usize>() {
            if let Some(value) = index.checked_sub(1).and_then(|idx| positionals.get(idx)) {
                output.push_str(value);
            }
        }
    }
    output
}

fn consume_literal<I>(chars: &mut std::iter::Peekable<I>, literal: &str) -> bool
where
    I: Iterator<Item = char> + Clone,
{
    let mut clone = chars.clone();
    for expected in literal.chars() {
        match clone.next() {
            Some(actual) if actual == expected => {}
            _ => return false,
        }
    }
    for _ in literal.chars() {
        let _ = chars.next();
    }
    true
}

/// Expand a command body: `$ARGUMENTS`/`$N`, `` !`cmd` ``/`!cmd` bash inlining,
/// and `@path` file embedding, in a single forward pass.
///
/// The `!` and `@` markers are only recognized in the static template text —
/// arguments injected via `$ARGUMENTS`/`$N` are substituted into the captured
/// command/path token but are never re-scanned, so a user argument cannot
/// smuggle in a shell invocation. Failures (a non-zero command, an unreadable
/// file) and commands `shell` does not grant are inlined as short notices
/// instead of aborting the render.
fn expand_command_template(
    body: &str,
    args: &str,
    base_dir: &Path,
    shell: &InlineShell<'_>,
    trace: &mut Vec<String>,
) -> String {
    let mut output = String::with_capacity(body.len() + args.len());
    // Tracks line position for the `!cmd` line form and the `@path` word
    // boundary: `at_line_start` until non-whitespace appears on the line,
    // `prev_is_boundary` after whitespace or a line break.
    let mut at_line_start = true;
    let mut prev_is_boundary = true;
    let mut chars = body.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '$' => {
                expand_dollar(&mut output, &mut chars, args);
                at_line_start = false;
                prev_is_boundary = false;
            }
            '!' if chars.peek() == Some(&'`') => {
                let _ = chars.next();
                let command = collect_until(&mut chars, '`');
                output.push_str(&runtime::kill_guard::for_one_command(None, || run_inline_command(&command, args, base_dir, shell, trace)));
                at_line_start = false;
                prev_is_boundary = false;
            }
            '!' if at_line_start && peek_is_command_start(&mut chars) => {
                let command = collect_line(&mut chars);
                output.push_str(&runtime::kill_guard::for_one_command(None, || run_inline_command(&command, args, base_dir, shell, trace)));
                // The line marker is fully consumed; the next char (if any) is a
                // newline, so the output stays on its own line.
                at_line_start = false;
                prev_is_boundary = false;
            }
            '@' if prev_is_boundary && peek_is_path_start(&mut chars) => {
                let token = collect_path_token(&mut chars);
                output.push_str(&embed_file(&token, args, base_dir, shell.confined, trace));
                at_line_start = false;
                prev_is_boundary = false;
            }
            '\n' => {
                output.push('\n');
                at_line_start = true;
                prev_is_boundary = true;
            }
            other => {
                output.push(other);
                let ws = other.is_whitespace();
                at_line_start = at_line_start && ws;
                prev_is_boundary = ws;
            }
        }
    }
    output
}

/// Handle a `$` already consumed by the caller: `$ARGUMENTS` / `$N` / literal.
fn expand_dollar<I>(output: &mut String, chars: &mut std::iter::Peekable<I>, args: &str)
where
    I: Iterator<Item = char> + Clone,
{
    if consume_literal(chars, "ARGUMENTS") {
        output.push_str(args);
        return;
    }
    let mut digits = String::new();
    while let Some(next) = chars.peek() {
        if next.is_ascii_digit() {
            digits.push(*next);
            let _ = chars.next();
        } else {
            break;
        }
    }
    if digits.is_empty() {
        output.push('$');
    } else if let Ok(index) = digits.parse::<usize>() {
        let positionals = args.split_whitespace().collect::<Vec<_>>();
        if let Some(value) = index.checked_sub(1).and_then(|idx| positionals.get(idx)) {
            output.push_str(value);
        }
    }
}

/// True when the next char can begin a `!cmd` line marker (rules out `!=`, the
/// markdown image `![`, and a bare `!` followed by whitespace).
fn peek_is_command_start<I>(chars: &mut std::iter::Peekable<I>) -> bool
where
    I: Iterator<Item = char>,
{
    matches!(chars.peek(), Some(&c) if !c.is_whitespace() && c != '=' && c != '[')
}

/// True when the next char can begin an `@path` token (rules out `@ ` and `@@`).
fn peek_is_path_start<I>(chars: &mut std::iter::Peekable<I>) -> bool
where
    I: Iterator<Item = char>,
{
    matches!(chars.peek(), Some(&c) if !c.is_whitespace() && c != '@')
}

fn collect_until<I>(chars: &mut std::iter::Peekable<I>, terminator: char) -> String
where
    I: Iterator<Item = char>,
{
    let mut buffer = String::new();
    for ch in chars.by_ref() {
        if ch == terminator {
            break;
        }
        buffer.push(ch);
    }
    buffer
}

fn collect_line<I>(chars: &mut std::iter::Peekable<I>) -> String
where
    I: Iterator<Item = char>,
{
    let mut buffer = String::new();
    while let Some(&ch) = chars.peek() {
        if ch == '\n' {
            break;
        }
        buffer.push(ch);
        let _ = chars.next();
    }
    buffer.trim_end_matches('\r').to_string()
}

fn collect_path_token<I>(chars: &mut std::iter::Peekable<I>) -> String
where
    I: Iterator<Item = char>,
{
    let mut buffer = String::new();
    while let Some(&ch) = chars.peek() {
        if ch.is_whitespace() {
            break;
        }
        buffer.push(ch);
        let _ = chars.next();
    }
    // Trailing sentence punctuation is almost never part of the path.
    buffer.trim_end_matches([',', '.', ';', ':', ')']).to_string()
}

/// What a command file's `!` commands may run: what its `allowed-tools`
/// grants Bash, and nothing while the session is read-only or while the file
/// is a workspace one the person has not given their word for (`workspace`,
/// about `folder`). A workspace file is also `confined`: its `@path` reads
/// only inside the working folder.
struct InlineShell<'a> {
    allowed_tools: &'a [String],
    read_only: bool,
    workspace: WorkspaceTrust,
    folder: &'a Path,
    rules: Result<&'a runtime::RuntimePermissionRuleConfig, &'a str>,
    confined: bool,
    /// The most a `!` line may run, when the caller names one
    /// ([`InlineShellGate::line_limit_ms`]).
    line_limit_ms: Option<u64>,
}

impl InlineShell<'_> {
    /// Why `command` may not run, when it may not: the words of the notice
    /// that stands in for it (`[!<command>: <why>]`).
    fn refusal(&self, command: &str) -> Option<String> {
        if self.read_only {
            return Some("not run in read-only mode".to_string());
        }
        match self.workspace {
            WorkspaceTrust::Trusted => {}
            WorkspaceTrust::Untrusted => {
                return Some(format!(
                    "not run — {} is not trusted; a repository's command file runs its ! lines only in a folder you trust, or under a --permission-mode you name",
                    self.folder.display()
                ));
            }
            WorkspaceTrust::UnnamedPipe => {
                return Some(
                    "not run — a pipe has nobody there to answer for the folder, so it takes the more careful side and does not go by an earlier trust record; give zo --permission-mode to let a repository's command file run its ! lines"
                        .to_string(),
                );
            }
        }
        if !self
            .allowed_tools
            .iter()
            .any(|spec| bash_rule_grants(spec, command))
        {
            return Some("not run — allowed-tools does not grant Bash for it".to_string());
        }
        self.rules_refusal(command)
    }

    /// Why the person's permission rules stop `command`, when they do: what
    /// the model's Bash would meet, with no prompt to ask at.
    fn rules_refusal(&self, command: &str) -> Option<String> {
        let rules = match self.rules {
            Ok(rules) => rules,
            Err(why) => {
                return Some(format!("not run — your permission settings did not load: {why}"));
            }
        };
        // The mode is the session's to apply; this asks the rules alone, so
        // the policy stands in full access and only a rule can say no.
        let policy = runtime::PermissionPolicy::new(runtime::PermissionMode::DangerFullAccess)
            .with_permission_rules(rules);
        let input = serde_json::json!({ "command": command }).to_string();
        if policy.deny_reason(BASH_TOOL, &input).is_some() {
            return Some("not run — a deny rule in your permission settings forbids it".to_string());
        }
        // A command file asks nobody, whatever the mode or the rules: the kill
        // guard's refusal says what it refused and what to do instead, as a
        // sub-agent's bash says it (t-82854). What it lets through is not
        // judged again; the rules decide the rest (t-81315).
        let call = match runtime::kill_guard::judge_where_nobody_can_ask(command) {
            Ok(call) => call,
            Err(refused) => return Some(format!("not run — {refused}")),
        };
        match policy.authorize_let_through(&call, None) {
            runtime::PermissionOutcome::Allow => None,
            runtime::PermissionOutcome::Deny { .. } => Some(
                "not run — an ask rule in your permission settings wants your approval, and a command file cannot ask; run it yourself if you approve"
                    .to_string(),
            ),
        }
    }
}

/// The name the session's Bash tool is registered and ruled under.
const BASH_TOOL: &str = "bash";

/// Characters that let one shell line chain, substitute or redirect beyond its
/// first command.
const SHELL_CONTROL: &[char] = &[';', '&', '|', '<', '>', '`', '$', '(', ')', '\n', '\r'];

/// Whether one `allowed-tools` spec grants `command`: bare `Bash` grants every
/// command, `Bash(<prefix>:*)` a command that is `<prefix>` or `<prefix>`
/// followed by whitespace, and `Bash(<command>)` that command exactly. A prefix
/// rule never grants a command carrying [`SHELL_CONTROL`] — `echo hi; touch x`
/// is not an `echo` — so neither the file nor an argument chains one on.
fn bash_rule_grants(spec: &str, command: &str) -> bool {
    let spec = spec.trim();
    if spec == "Bash" {
        return true;
    }
    let Some(rule) = spec
        .strip_prefix("Bash(")
        .and_then(|rest| rest.strip_suffix(')'))
    else {
        return false;
    };
    let rule = rule.trim();
    match rule.strip_suffix(":*") {
        Some(prefix) => {
            let prefix = prefix.trim_end();
            !prefix.is_empty()
                && !command.contains(SHELL_CONTROL)
                && command
                    .strip_prefix(prefix)
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
        }
        None => command == rule,
    }
}

/// Run a `!` command and return its stdout (trailing newline trimmed). A
/// command `shell` does not grant, or a failed or refused one, degrades to a
/// one-line notice rather than panicking. Either way one line goes on
/// `trace`: the exit code, or why it did not run.
fn run_inline_command(
    template: &str,
    args: &str,
    base_dir: &Path,
    shell: &InlineShell<'_>,
    trace: &mut Vec<String>,
) -> String {
    let command = expand_prompt_arguments(template, args);
    let command = command.trim();
    if command.is_empty() {
        return String::new();
    }
    if let Some(why) = shell.refusal(command) {
        trace.push(format!("! {command} → {why}"));
        return format!("[!{command}: {why}]");
    }
    let input = runtime::BashCommandInput {
        command: command.to_string(),
        timeout: shell.line_limit_ms,
        description: None,
        run_in_background: None,
        dangerously_disable_sandbox: None,
        namespace_restrictions: None,
        isolate_network: None,
        filesystem_mode: None,
        allowed_mounts: None,
        cwd: Some(base_dir.to_path_buf()),
    };
    match runtime::execute_bash(input) {
        Ok(output) => {
            trace.push(format!("! {command} → {}", how_it_ended(&output, shell.line_limit_ms)));
            inline_output(&output, command, shell.line_limit_ms)
        }
        Err(error) => {
            trace.push(format!("! {command} → {error}"));
            format!("[!{command}: {error}]")
        }
    }
}

/// `exit 0`, `exit 2`, or the bash tool's own word for an end that has no
/// code (`timeout`, `interrupted`). A `timeout` under a limit a screen put on
/// the line says what it did ([`slow_line_notice`]).
fn how_it_ended(output: &runtime::BashCommandOutput, line_limit_ms: Option<u64>) -> String {
    let ending = output.return_code_interpretation.as_deref().map(runtime::CommandEnding::read);
    if ending == Some(runtime::CommandEnding::TimedOut) {
        return line_limit_ms.map_or_else(runtime::CommandEnding::timed_out_word, slow_line_notice);
    }
    if output.interrupted {
        return "interrupted".to_string();
    }
    match ending {
        None => "exit 0".to_string(),
        Some(runtime::CommandEnding::Exit(code)) => format!("exit {code}"),
        Some(runtime::CommandEnding::TimedOut) => runtime::CommandEnding::timed_out_word(),
        Some(runtime::CommandEnding::Other(word)) => word.to_string(),
    }
}

/// What a `!` line that outran the limit a screen put on it says, in the line
/// the screen shows and in the prompt: it held the window for the whole limit
/// and was stopped (t-81313 P3-6).
fn slow_line_notice(limit_ms: u64) -> String {
    format!(
        "timeout — a slow ! line held the window for {} s and was stopped",
        limit_ms.div_ceil(1000)
    )
}

/// What a `!` line puts in the prompt: its stdout, the trailing newline
/// trimmed — or, for a line a screen's limit stopped, the notice that stands
/// in for the output that never came.
fn inline_output(output: &runtime::BashCommandOutput, command: &str, line_limit_ms: Option<u64>) -> String {
    match line_limit_ms {
        Some(limit)
            if output.return_code_interpretation.as_deref().map(runtime::CommandEnding::read)
                == Some(runtime::CommandEnding::TimedOut) =>
        {
            format!("[!{command}: {}]", slow_line_notice(limit))
        }
        _ => output.stdout.trim_end_matches('\n').to_string(),
    }
}

/// Embed `@path` file contents. Relative paths resolve against `base_dir`; a
/// missing or unreadable file degrades to a one-line notice, and so does a
/// path that lands outside `base_dir` when `confined`.
fn embed_file(
    token: &str,
    args: &str,
    base_dir: &Path,
    confined: bool,
    trace: &mut Vec<String>,
) -> String {
    let raw = expand_prompt_arguments(token, args);
    let raw = raw.trim();
    if raw.is_empty() {
        return "@".to_string();
    }
    let path = Path::new(raw);
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base_dir.join(path)
    };
    if confined {
        // Where the path really lands, links and `..` resolved, must be
        // inside the working folder.
        let inside = match (resolved.canonicalize(), base_dir.canonicalize()) {
            (Ok(real), Ok(base)) => real.starts_with(base),
            (Err(error), _) | (_, Err(error)) => {
                trace.push(format!("@{raw} → {error}"));
                return format!("[@{raw}: {error}]");
            }
        };
        if !inside {
            let why = "not read — a repository's command file reads only inside the working folder";
            trace.push(format!("@{raw} → {why}"));
            return format!("[@{raw}: {why}]");
        }
    }
    match fs::read_to_string(&resolved) {
        Ok(contents) => {
            trace.push(format!("@{raw} → read"));
            contents
        }
        Err(error) => {
            trace.push(format!("@{raw} → {error}"));
            format!("[@{raw}: {error}]")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// [`expand_command_template`] for a file whose `allowed-tools` is `Bash`.
    fn expand(body: &str, args: &str, base_dir: &Path) -> String {
        let allowed_tools = ["Bash".to_string()];
        let no_rules = runtime::RuntimePermissionRuleConfig::default();
        let shell = InlineShell {
            allowed_tools: &allowed_tools,
            read_only: false,
            workspace: WorkspaceTrust::Trusted,
            folder: base_dir,
            rules: Ok(&no_rules),
            confined: true,
            line_limit_ms: None,
        };
        expand_command_template(body, args, base_dir, &shell, &mut Vec::new())
    }

    fn temp_dir(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("zo-prompt-commands-{label}-{unique}"))
    }

    #[test]
    fn discovers_prompt_commands_with_frontmatter_and_nested_names() {
        let root = temp_dir("discover");
        let cwd = root.join("app");
        let commands_dir = cwd.join(".zo").join("commands").join("git");
        fs::create_dir_all(&commands_dir).expect("commands dir");
        fs::write(
            commands_dir.join("review.md"),
            "---\ndescription: Review changes\nargument-hint: <scope>\nmodel: opus\neffort: high\n---\nReview $ARGUMENTS with $1.\n",
        )
        .expect("write command");

        let commands = discover_with_home(&cwd, &root.join("home"));
        let command = find_prompt_command(&commands, "git/review").expect("command discovered");

        assert_eq!(command.description.as_deref(), Some("Review changes"));
        assert_eq!(command.argument_hint.as_deref(), Some("<scope>"));
        assert_eq!(command.model.as_deref(), Some("opus"));
        assert_eq!(command.effort.as_deref(), Some("high"));
        assert_eq!(
            command.render_prompt("src/main.rs extra"),
            "Review src/main.rs extra with src/main.rs.\n"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn nearest_prompt_command_wins_duplicate_name() {
        let root = temp_dir("nearest");
        let cwd = root.join("app").join("nested");
        fs::create_dir_all(root.join(".zo").join("commands")).expect("root commands");
        fs::create_dir_all(cwd.join(".zo").join("commands")).expect("nested commands");
        fs::write(root.join(".zo").join("commands").join("plan.md"), "root")
            .expect("write root command");
        fs::write(
            cwd.join(".zo").join("commands").join("plan.md"),
            "nested",
        )
        .expect("write nested command");

        let commands = discover_with_home(&cwd, &root.join("home"));
        let command = find_prompt_command(&commands, "plan").expect("command discovered");

        assert_eq!(command.body, "nested");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn missing_positionals_expand_to_empty() {
        assert_eq!(
            expand_prompt_arguments("$1|$2|$ARGUMENTS", "one"),
            "one||one"
        );
    }

    #[test]
    fn bash_marker_inlines_stdout() {
        let base = std::env::temp_dir();
        // Inline `!`cmd`` and the line `!cmd` form both inline stdout; the line
        // form receives a positional argument.
        let rendered = expand(
            "before !`echo inline` mid\n!echo $1\nafter",
            "lined",
            &base,
        );
        assert_eq!(rendered, "before inline mid\nlined\nafter");
    }

    #[test]
    fn bash_marker_is_not_re_scanned_from_arguments() {
        // A `!`…`` smuggled in through $ARGUMENTS must stay literal, never run.
        let rendered = expand("$ARGUMENTS", "!`echo pwned`", &std::env::temp_dir());
        assert_eq!(rendered, "!`echo pwned`");
    }

    #[test]
    fn file_marker_embeds_contents_and_degrades() {
        let root = temp_dir("embed");
        fs::create_dir_all(&root).expect("base dir");
        fs::write(root.join("notes.md"), "FILE BODY").expect("write file");

        let rendered = expand("see @notes.md done", "", &root);
        assert_eq!(rendered, "see FILE BODY done");

        // A missing file degrades to an inline notice instead of panicking.
        let missing = expand("@nope.md", "", &root);
        assert!(missing.starts_with("[@nope.md:"), "got: {missing}");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn at_mention_inside_word_stays_literal() {
        let rendered = expand("ping user@host now", "", &std::env::temp_dir());
        assert_eq!(rendered, "ping user@host now");
    }

    #[test]
    fn allowed_tools_frontmatter_is_parsed() {
        let command = parse_prompt_command_file(
            "scoped".to_string(),
            PathBuf::from("scoped.md"),
            "---\nallowed-tools: Bash(git diff:*), Read, Glob\n---\nBody.\n",
        )
        .expect("command parsed");
        assert_eq!(
            command.allowed_tools,
            vec![
                "Bash(git diff:*)".to_string(),
                "Read".to_string(),
                "Glob".to_string(),
            ]
        );
    }

    /// A command file that does not grant Bash in `allowed-tools` cannot run
    /// its `!` lines: typing `/name` must not run a shell command the file
    /// carries. Each one leaves a one-line notice in the prompt instead.
    #[test]
    fn bang_lines_without_allowed_tools_do_not_run() {
        let root = temp_dir("bang-ungranted");
        fs::create_dir_all(&root).expect("base dir");
        let line_marker = root.join("line-ran");
        let inline_marker = root.join("inline-ran");
        let line = format!("touch {}", line_marker.display());
        let inline = format!("touch {}", inline_marker.display());
        let command = parse_prompt_command_file(
            "ungranted".to_string(),
            PathBuf::from("ungranted.md"),
            &format!("---\ndescription: no tools\n---\nbefore\n!{line}\nmid !`{inline}` end\n"),
        )
        .expect("command parsed");

        let rendered = command.render_prompt_in("", &root);

        assert!(!line_marker.exists(), "the `!cmd` line ran without allowed-tools");
        assert!(!inline_marker.exists(), "the inline `!`cmd`` ran without allowed-tools");
        assert_eq!(
            rendered,
            format!(
                "before\n[!{line}: not run — allowed-tools does not grant Bash for it]\n\
                 mid [!{inline}: not run — allowed-tools does not grant Bash for it] end\n"
            )
        );
        let _ = fs::remove_dir_all(root);
    }

    /// `Bash(echo:*)` grants `echo` and nothing else: `touch` stays unrun, and
    /// so does an `echo` that chains a second command after it.
    #[test]
    fn bang_lines_run_only_what_allowed_tools_grants() {
        let root = temp_dir("bang-granted");
        fs::create_dir_all(&root).expect("base dir");
        let touch_marker = root.join("touch-ran");
        let chained_marker = root.join("chained-ran");
        let touch = format!("touch {}", touch_marker.display());
        let chained = format!("echo hi; touch {}", chained_marker.display());
        let command = parse_prompt_command_file(
            "granted".to_string(),
            PathBuf::from("granted.md"),
            &format!("---\nallowed-tools: Bash(echo:*)\n---\n!echo hi\n!{touch}\n!{chained}\n"),
        )
        .expect("command parsed");

        let rendered = command.render_prompt_in("", &root);

        assert!(!touch_marker.exists(), "Bash(echo:*) let `touch` run");
        assert!(!chained_marker.exists(), "Bash(echo:*) let a chained `touch` run");
        assert_eq!(
            rendered,
            format!(
                "hi\n[!{touch}: not run — allowed-tools does not grant Bash for it]\n\
                 [!{chained}: not run — allowed-tools does not grant Bash for it]\n"
            )
        );

        // An argument cannot chain a command onto a granted prefix either.
        let argued_marker = root.join("argued-ran");
        let argued = parse_prompt_command_file(
            "argued".to_string(),
            PathBuf::from("argued.md"),
            "---\nallowed-tools: Bash(echo:*)\n---\n!echo $ARGUMENTS\n",
        )
        .expect("command parsed");
        let args = format!("hi; touch {}", argued_marker.display());
        let rendered = argued.render_prompt_in(&args, &root);
        assert!(!argued_marker.exists(), "an argument chained `touch` onto `echo`");
        assert_eq!(
            rendered,
            format!("[!echo {args}: not run — allowed-tools does not grant Bash for it]\n")
        );
        let _ = fs::remove_dir_all(root);
    }

    /// A `!` line answers to the person's deny rule as the model's bash
    /// does, however the rule spells the tool: Claude Code's `Bash(rm:*)`
    /// and a shouted `BASH(rm:*)` both keep `rm` from running (t-81312).
    #[test]
    fn a_deny_rule_naming_bash_in_any_case_stops_a_bang_line() {
        let root = temp_dir("bang-rule-case");
        fs::create_dir_all(&root).expect("base dir");
        let victim = root.join("victim");
        let line = format!("rm {}", victim.display());
        let command = parse_prompt_command_file(
            "remove".to_string(),
            PathBuf::from("remove.md"),
            &format!("---\nallowed-tools: Bash\n---\n!{line}\n"),
        )
        .expect("command parsed");

        for deny in ["Bash(rm:*)", "BASH(rm:*)"] {
            fs::write(&victim, "keep").expect("victim file");
            let rules =
                runtime::RuntimePermissionRuleConfig::new(Vec::new(), vec![deny.to_string()], Vec::new());
            let gate = InlineShellGate {
                read_only: false,
                workspace: WorkspaceTrust::Trusted,
                rules: Ok(&rules),
                line_limit_ms: None,
            };
            let rendered = command.render_prompt_as("", &root, gate);
            assert!(victim.exists(), "deny {deny} let the `!` line remove the file");
            assert_eq!(
                rendered,
                format!("[!{line}: not run — a deny rule in your permission settings forbids it]\n"),
                "deny {deny}"
            );
        }
        let _ = fs::remove_dir_all(root);
    }

    /// A `!` line the kill guard refuses says why in the guard's own words —
    /// a signal no person said yes to — not that an ask rule wants approval
    /// (t-82721 P3-3). The pid is known only when the line would run, so
    /// nothing is aimed at.
    #[test]
    fn a_bang_line_the_kill_guard_refuses_says_why_in_the_guards_words() {
        let root = temp_dir("bang-kill");
        fs::create_dir_all(&root).expect("base dir");
        let line = "kill \"$TARGET\"";
        let command = parse_prompt_command_file(
            "stop".to_string(),
            PathBuf::from("stop.md"),
            &format!("---\nallowed-tools: Bash\n---\n!{line}\n"),
        )
        .expect("command parsed");
        let rules = runtime::RuntimePermissionRuleConfig::new(Vec::new(), Vec::new(), Vec::new());
        let gate = InlineShellGate {
            read_only: false,
            workspace: WorkspaceTrust::Trusted,
            rules: Ok(&rules),
            line_limit_ms: None,
        };
        let rendered = command.render_prompt_as("", &root, gate);
        let refused = format!("[!{line}: not run — refused, since no person said yes to it");
        assert!(rendered.starts_with(&refused), "{rendered}");
        assert!(!rendered.contains("an ask rule"), "{rendered}");
        let _ = fs::remove_dir_all(root);
    }

    /// A `!` line is judged on one reading of the processes: the guard's
    /// refusal check and the bash chokepoint each read them before (t-84911).
    /// A `ps` first on `PATH` notes each run and gives a listing taken once
    /// before, so the test does not wait on `ps`; the line sends a running
    /// `sleep` a signal it ignores (`kill -CONT`), to this process's own child.
    #[cfg(unix)]
    #[test]
    fn a_bang_line_reads_the_processes_once_for_a_kill_call() {
        use std::os::unix::fs::PermissionsExt as _;

        let _guard = ENV_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let root = temp_dir("bang-one-reading");
        fs::create_dir_all(&root).expect("base dir");
        let path = std::env::var_os("PATH");
        let real = path
            .iter()
            .flat_map(std::env::split_paths)
            .map(|dir| dir.join("ps"))
            .find(|ps| ps.is_file())
            .expect("a ps on PATH");
        let log = root.join("log");
        let mut own = std::process::Command::new("sleep").arg("120").spawn().expect("a child to signal");
        // The listing is taken once, now that the child runs, with the real
        // `ps` and the guard's own arguments: the guard then reads it without
        // waiting on `ps`, whose 800 ms wall a loaded machine overran.
        let listing = std::process::Command::new(&real)
            .args(["-ww", "-eo", "pid=,ppid=,pgid=,command="])
            .env("LC_ALL", "C")
            .output()
            .expect("the real ps lists the processes");
        assert!(listing.status.success(), "the real ps lists the processes");
        let kept = root.join("listing");
        fs::write(&kept, &listing.stdout).expect("the listing is kept");
        let script = format!("#!/bin/sh\necho read >> '{}'\nexec cat '{}'\n", log.display(), kept.display());
        fs::write(root.join("ps"), script).expect("the noting ps is written");
        fs::set_permissions(root.join("ps"), fs::Permissions::from_mode(0o755)).expect("the noting ps runs");
        // macOS looks a new executable over the first time it runs — a tenth
        // of a second on a quiet machine, more on a loaded one — and the guard
        // gives `ps` 800 ms. The first run happens here, outside that wall,
        // and what it noted is cleared, as tools' `NotingPs` does.
        let warmed = std::process::Command::new(root.join("ps"))
            .stdout(std::process::Stdio::null())
            .status()
            .expect("the noting ps runs once");
        assert!(warmed.success(), "the noting ps runs once");
        fs::remove_file(&log).expect("the warming run is forgotten");
        let mut dirs = vec![root.clone()];
        dirs.extend(path.iter().flat_map(std::env::split_paths));
        std::env::set_var("PATH", std::env::join_paths(dirs).expect("a PATH"));
        let rendered = expand(&format!("!kill -CONT {}\n", own.id()), "", &root);
        match path {
            Some(path) => std::env::set_var("PATH", path),
            None => std::env::remove_var("PATH"),
        }
        let reads = fs::read_to_string(&log).unwrap_or_default().lines().count();
        let still_running = own.try_wait().expect("the child can be asked").is_none();
        let _ = own.kill();
        let _ = own.wait();
        let _ = fs::remove_dir_all(&root);

        assert!(!rendered.contains("not run"), "the guard lets zo signal its own child: {rendered}");
        assert!(still_running, "`kill -CONT` ends nothing");
        assert_eq!(reads, 1, "the processes were read {reads} times for one line");
    }

    /// A command file asks nobody, whatever the mode or the rules: a `!` line
    /// the kill guard refuses says what to do instead as a sub-agent's bash
    /// says it, and never that the session runs calls without asking — under
    /// no rule, an allow rule and an ask rule alike (t-82854, review w-83819
    /// P3-1).
    #[test]
    fn a_bang_lines_kill_refusal_reads_as_check_bashs_under_any_rule() {
        let root = temp_dir("bang-kill-rules");
        fs::create_dir_all(&root).expect("base dir");
        let line = "kill \"$TARGET\"";
        let command = parse_prompt_command_file(
            "stop".to_string(),
            PathBuf::from("stop.md"),
            &format!("---\nallowed-tools: Bash\n---\n!{line}\n"),
        )
        .expect("command parsed");
        let check_bash = runtime::permission_enforcer::PermissionEnforcer::new(runtime::PermissionPolicy::new(
            runtime::PermissionMode::DangerFullAccess,
        ))
        .check_bash(line);
        let runtime::permission_enforcer::EnforcementResult::Denied { reason, .. } = check_bash else {
            panic!("check_bash ran a kill no person said yes to");
        };
        let kill_rule = || vec!["bash(kill:*)".to_string()];
        for (name, rules) in [
            ("no rule", runtime::RuntimePermissionRuleConfig::new(Vec::new(), Vec::new(), Vec::new())),
            ("an allow rule", runtime::RuntimePermissionRuleConfig::new(kill_rule(), Vec::new(), Vec::new())),
            ("an ask rule", runtime::RuntimePermissionRuleConfig::new(Vec::new(), Vec::new(), kill_rule())),
        ] {
            let gate = InlineShellGate {
                read_only: false,
                workspace: WorkspaceTrust::Trusted,
                rules: Ok(&rules),
                line_limit_ms: None,
            };
            let rendered = command.render_prompt_as("", &root, gate);
            assert!(!rendered.contains("runs calls without asking"), "{name}: {rendered}");
            assert_eq!(rendered, format!("[!{line}: not run — {reason}]\n"), "{name}");
        }
        let _ = fs::remove_dir_all(root);
    }

    /// A repository's command file reads `@path` only inside the working
    /// folder: an absolute path, a `..` that climbs out, and a link inside
    /// that points out each leave one notice line. The same file in zo's
    /// home reads where it points.
    #[test]
    fn workspace_at_paths_stay_inside_the_working_folder() {
        let root = temp_dir("at-confined");
        let work = root.join("work");
        let outside = root.join("outside");
        fs::create_dir_all(&work).expect("working folder");
        fs::create_dir_all(&outside).expect("outside folder");
        fs::write(work.join("inside.md"), "INSIDE").expect("inside file");
        fs::write(outside.join("secret.md"), "SECRET").expect("outside file");
        let absolute_outside = outside.join("secret.md").display().to_string();
        let absolute_inside = work.join("inside.md").display().to_string();
        let body = format!(
            "@inside.md\n@../outside/secret.md\n@{absolute_outside}\n@{absolute_inside}\n"
        );
        let no_rules = runtime::RuntimePermissionRuleConfig::default();
        let trusted = InlineShellGate {
            read_only: false,
            workspace: WorkspaceTrust::Trusted,
            rules: Ok(&no_rules),
            line_limit_ms: None,
        };
        let refused = |raw: &str| {
            format!("[@{raw}: not read — a repository's command file reads only inside the working folder]")
        };

        let mut command = parse_prompt_command_file("at".to_string(), PathBuf::from("at.md"), &body)
            .expect("command parsed");
        assert_eq!(
            command.render_prompt_as("", &work, trusted),
            format!(
                "INSIDE\n{}\n{}\nINSIDE\n",
                refused("../outside/secret.md"),
                refused(&absolute_outside)
            )
        );

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.join("secret.md"), work.join("link.md"))
                .expect("a link that points out");
            let linked = parse_prompt_command_file("link".to_string(), PathBuf::from("link.md"), "@link.md\n")
                .expect("command parsed");
            assert_eq!(linked.render_prompt_as("", &work, trusted), format!("{}\n", refused("link.md")));
        }

        command.scope = PromptCommandScope::ZoHome;
        assert_eq!(
            command.render_prompt_as("", &work, trusted),
            "INSIDE\nSECRET\nSECRET\nINSIDE\n"
        );
        let _ = fs::remove_dir_all(root);
    }

    /// The person's zo-home command keeps its name when a repository file
    /// has the same one: a repository cannot take over a name the person
    /// already types.
    #[test]
    fn a_zo_home_command_wins_the_name_a_repository_file_also_has() {
        let root = temp_dir("home-wins");
        let zo_home = root.join("home").join(".zo");
        let cwd = root.join("project");
        fs::create_dir_all(zo_home.join("commands")).expect("personal commands dir");
        fs::create_dir_all(cwd.join(".zo").join("commands")).expect("project commands dir");
        fs::write(zo_home.join("commands").join("plan.md"), "home").expect("personal command");
        fs::write(cwd.join(".zo").join("commands").join("plan.md"), "repo").expect("repo command");

        let commands = discover_with_home(&cwd, &root.join("home"));
        let command = find_prompt_command(&commands, "plan").expect("command discovered");
        assert_eq!(command.body, "home");
        assert_eq!(command.scope, PromptCommandScope::ZoHome);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn discovers_personal_commands_in_home() {
        let home = temp_dir("home");
        let personal = home.join(".zo").join("commands");
        fs::create_dir_all(&personal).expect("personal commands dir");
        fs::write(personal.join("greet.md"), "Hello from home.").expect("write personal command");

        // A project directory with no commands of its own.
        let cwd = temp_dir("project");
        fs::create_dir_all(&cwd).expect("project dir");

        let commands = discover_with_home(&cwd, &home);
        let command = find_prompt_command(&commands, "greet").expect("personal command discovered");
        assert_eq!(command.body, "Hello from home.");

        let _ = fs::remove_dir_all(home);
        let _ = fs::remove_dir_all(cwd);
    }

    #[test]
    fn ignores_non_zo_prompt_command_roots() {
        let root = temp_dir("zo-only");
        let cwd = root.join("app");
        for hidden_root in [".other-tool", ".codex"] {
            let commands = cwd.join(hidden_root).join("commands");
            fs::create_dir_all(&commands).expect("non-Zo commands dir");
            fs::write(
                commands.join(format!("ignored-{hidden_root}.md")),
                "must not load",
            )
            .expect("write non-Zo command");
        }

        let commands = discover_with_home(&cwd, &root.join("home"));
        assert!(find_prompt_command(&commands, "ignored-.other-tool").is_none());
        assert!(find_prompt_command(&commands, "ignored-.codex").is_none());

        let _ = fs::remove_dir_all(root);
    }

    /// A command folder above the repository is not the repository's: the
    /// walk up from the working folder reads each `.zo/commands` it passes,
    /// nearest first, and stops at the top of the repository — the folder that
    /// holds `.git`, which is read. What sits beyond is another project's or
    /// nobody's (t-81313 P3-8).
    #[test]
    fn discovery_stops_at_the_top_of_the_repository() {
        let root = temp_dir("repo-top");
        let repo = root.join("repo");
        let cwd = repo.join("src").join("deep");
        let above = root.join(".zo").join("commands");
        let top = repo.join(".zo").join("commands");
        let between = repo.join("src").join(".zo").join("commands");
        for dir in [&above, &top, &between, &repo.join(".git"), &cwd] {
            fs::create_dir_all(dir).expect("the tree");
        }
        fs::write(above.join("outside.md"), "above the repository").expect("write outside");
        fs::write(top.join("top.md"), "the top of the repository").expect("write top");
        fs::write(between.join("between.md"), "a folder inside").expect("write between");

        let commands = discover_with_home(&cwd, &root.join("home"));

        assert!(find_prompt_command(&commands, "between").is_some(), "a folder inside the repository is read");
        assert!(find_prompt_command(&commands, "top").is_some(), "the top of the repository is read");
        assert!(
            find_prompt_command(&commands, "outside").is_none(),
            "a folder above the repository was read"
        );
        let _ = fs::remove_dir_all(root);
    }

    /// The walk also stops short of the person's home: a project below it
    /// reads the folders up to the home, and neither the home itself (its
    /// `.zo` is read as the person's own, by name) nor anything above it.
    #[test]
    fn discovery_stops_short_of_the_persons_home() {
        let root = temp_dir("home-stop");
        let home = root.join("home");
        let project = home.join("project");
        let cwd = project.join("src");
        let above_home = root.join(".zo").join("commands");
        let project_commands = project.join(".zo").join("commands");
        for dir in [&above_home, &project_commands, &cwd] {
            fs::create_dir_all(dir).expect("the tree");
        }
        fs::write(above_home.join("elsewhere.md"), "above the home").expect("write elsewhere");
        fs::write(project_commands.join("project.md"), "the project's").expect("write project");

        let commands = discover_with_home(&cwd, &home);

        assert!(find_prompt_command(&commands, "project").is_some(), "the project below the home is read");
        assert!(
            find_prompt_command(&commands, "elsewhere").is_none(),
            "a folder above the person's home was read"
        );
        let _ = fs::remove_dir_all(root);
    }

    /// With neither the top of a repository nor the person's home above the
    /// working folder, nothing says where the project ends, so only the
    /// working folder is read.
    #[test]
    fn with_no_top_in_sight_only_the_working_folder_is_read() {
        let root = temp_dir("no-top");
        // A temp folder inside a repository has a top in sight; the case
        // needs one that does not.
        if root.ancestors().any(|dir| dir.join(".git").exists()) {
            eprintln!("skipped: {} stands inside a repository", root.display());
            return;
        }
        let cwd = root.join("app").join("nested");
        let far = root.join(".zo").join("commands");
        let near = cwd.join(".zo").join("commands");
        for dir in [&far, &near] {
            fs::create_dir_all(dir).expect("the tree");
        }
        fs::write(far.join("far.md"), "far above").expect("write far");
        fs::write(near.join("near.md"), "the working folder's").expect("write near");

        let commands = discover_with_home(&cwd, &root.join("home"));

        assert!(find_prompt_command(&commands, "near").is_some(), "the working folder is read");
        assert!(find_prompt_command(&commands, "far").is_none(), "a folder far above was read");
        let _ = fs::remove_dir_all(root);
    }

    /// A `!` line that outruns the limit a screen put on it is stopped, and the
    /// prompt and the line the screen shows say a slow line held the window
    /// (t-81313 P3-6, decided in m-94038): the TUI expands a command on its own
    /// loop, so keys, repaint and Esc wait for as long as a line runs. The line
    /// would sleep twenty seconds and then write a marker; the limit is one.
    #[test]
    fn a_bang_line_past_its_limit_is_stopped_and_says_a_slow_line_held_the_window() {
        let root = temp_dir("bang-slow");
        fs::create_dir_all(&root).expect("base dir");
        let marker = root.join("slow-ran");
        // `!` lines run in the Bash tool's POSIX shell — Git's `sh` on
        // Windows, where a backslash is an escape.
        let path = marker.display().to_string();
        let path = if cfg!(windows) { path.replace('\\', "/") } else { path };
        let command = parse_prompt_command_file(
            "slow".to_string(),
            PathBuf::from("slow.md"),
            &format!("---\nallowed-tools: Bash\n---\n!sleep 20; touch {path}\nDone.\n"),
        )
        .expect("command parsed");
        let no_rules = runtime::RuntimePermissionRuleConfig::default();
        let limited = InlineShellGate {
            read_only: false,
            workspace: WorkspaceTrust::Trusted,
            rules: Ok(&no_rules),
            line_limit_ms: Some(1_000),
        };

        let begun = std::time::Instant::now();
        let rendered = command.render_prompt_traced("", &root, limited);
        let took = begun.elapsed();

        let notice = "a slow ! line held the window for 1 s and was stopped";
        assert!(
            took < std::time::Duration::from_secs(15),
            "the line ran {took:?}; its prompt came out as {:?} and its trace as {:?}",
            rendered.text,
            rendered.trace
        );
        assert!(!marker.exists(), "the line was not stopped");
        assert_eq!(
            rendered.text,
            format!("[!sleep 20; touch {path}: timeout — {notice}]\nDone.\n"),
            "the prompt keeps the rest of the file and says what happened to the line"
        );
        assert_eq!(rendered.trace, vec![format!("! sleep 20; touch {path} → timeout — {notice}")]);

        // With no limit of the screen's (a pipe) a quick line renders as it
        // always did.
        let unlimited = InlineShellGate {
            read_only: false,
            workspace: WorkspaceTrust::Trusted,
            rules: Ok(&no_rules),
            line_limit_ms: None,
        };
        let quick = parse_prompt_command_file(
            "quick".to_string(),
            PathBuf::from("quick.md"),
            "---\nallowed-tools: Bash\n---\n!echo fast\nDone.\n",
        )
        .expect("command parsed");
        let rendered = quick.render_prompt_traced("", &root, unlimited);
        assert_eq!(rendered.text, "fast\nDone.\n");
        assert_eq!(rendered.trace, vec!["! echo fast → exit 0".to_string()]);
        let _ = fs::remove_dir_all(root);
    }

    /// Serializes the tests that mutate `HOME` (process-global state).
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// [`discover_prompt_commands`] with `home` as the person's home and zo's
    /// home in it (`<home>/.zo`): the person's own commands come first and
    /// keep their names, so a test that let the real home in would answer
    /// with whatever that home holds.
    fn discover_with_home(cwd: &Path, home: &Path) -> Vec<PromptCommandDef> {
        let _guard = ENV_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let saved = ["HOME", "ZO_CONFIG_HOME", "ZO_HOME"].map(|key| (key, std::env::var_os(key)));
        std::env::set_var("HOME", home);
        std::env::set_var("ZO_CONFIG_HOME", home.join(".zo"));
        std::env::remove_var("ZO_HOME");
        let commands = discover_prompt_commands(cwd);
        for (key, value) in saved {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
        commands
    }
}
