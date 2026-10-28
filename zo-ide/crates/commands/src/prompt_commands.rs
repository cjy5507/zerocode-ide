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

    /// Expand the command body into the prompt that is queued as the next turn.
    ///
    /// In addition to `$ARGUMENTS`/`$N` substitution this resolves the two
    /// dynamic-content forms Claude Code supports: `` !`cmd` `` (and a line that
    /// begins with `!`) inlines the command's stdout, and `@path` embeds the
    /// referenced file. Relative paths and the shell working directory resolve
    /// against the live process cwd; a failed command or missing file degrades
    /// to an inline notice rather than aborting the render. A `!` command runs
    /// only when the file's `allowed-tools` grants it Bash.
    #[must_use]
    pub fn render_prompt(&self, args: &str) -> String {
        let base_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        self.render_prompt_in(args, &base_dir)
    }

    /// [`Self::render_prompt`] against `base_dir` rather than the process cwd:
    /// the session's working folder, where `!cmd` runs and `@path` resolves.
    /// The caller vouches for the folder; a session renders through
    /// [`Self::render_prompt_as`].
    #[must_use]
    pub fn render_prompt_in(&self, args: &str, base_dir: &Path) -> String {
        let no_rules = runtime::RuntimePermissionRuleConfig::default();
        let gate = InlineShellGate {
            read_only: false,
            workspace: WorkspaceTrust::Trusted,
            rules: Ok(&no_rules),
        };
        self.render_prompt_as(args, base_dir, gate)
    }

    /// [`Self::render_prompt_in`] under what `gate` lets `!` commands run:
    /// none while read-only, and none from a workspace file in a folder the
    /// person has not trusted, whatever `allowed-tools` grants.
    #[must_use]
    pub fn render_prompt_as(&self, args: &str, base_dir: &Path, gate: InlineShellGate) -> String {
        self.render_prompt_traced(args, base_dir, gate).text
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
/// it, nearest first. A file whose name is taken is not callable; the one
/// that took it lists it in [`PromptCommandDef::shadows`].
#[must_use]
pub fn discover_prompt_commands(cwd: &Path) -> Vec<PromptCommandDef> {
    let personal_roots = personal_command_roots();
    // The walk up from a project below the person's home passes zo's home
    // too; that folder was already read as the person's.
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
    let workspace_roots = cwd
        .ancestors()
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
                output.push_str(&run_inline_command(&command, args, base_dir, shell, trace));
                at_line_start = false;
                prev_is_boundary = false;
            }
            '!' if at_line_start && peek_is_command_start(&mut chars) => {
                let command = collect_line(&mut chars);
                output.push_str(&run_inline_command(&command, args, base_dir, shell, trace));
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
                    "not run — in a pipe nobody answers a trust prompt; give zo --permission-mode to let a repository's command file run its ! lines"
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
        match policy.authorize(BASH_TOOL, &input, None) {
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
        timeout: None,
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
            trace.push(format!("! {command} → {}", how_it_ended(&output)));
            output.stdout.trim_end_matches('\n').to_string()
        }
        Err(error) => {
            trace.push(format!("! {command} → {error}"));
            format!("[!{command}: {error}]")
        }
    }
}

/// `exit 0`, `exit 2`, or the bash tool's own word for an end that has no
/// code (`timeout`, `interrupted`).
fn how_it_ended(output: &runtime::BashCommandOutput) -> String {
    if output.interrupted {
        return "interrupted".to_string();
    }
    match output.return_code_interpretation.as_deref() {
        None => "exit 0".to_string(),
        Some(word) => word
            .strip_prefix("exit_code:")
            .map_or_else(|| word.to_string(), |code| format!("exit {code}")),
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
