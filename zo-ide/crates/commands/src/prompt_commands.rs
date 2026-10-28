use std::collections::BTreeSet;
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
}

impl PromptCommandDef {
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
    #[must_use]
    pub fn render_prompt_in(&self, args: &str, base_dir: &Path) -> String {
        self.render_prompt_as(args, base_dir, false)
    }

    /// [`Self::render_prompt_in`] for a session whose permission mode is
    /// read-only when `read_only` is set: then no `!` command runs, whatever
    /// `allowed-tools` grants.
    #[must_use]
    pub fn render_prompt_as(&self, args: &str, base_dir: &Path, read_only: bool) -> String {
        let shell = InlineShell {
            allowed_tools: &self.allowed_tools,
            read_only,
        };
        expand_command_template(&self.body, args, base_dir, &shell)
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

#[must_use]
pub fn discover_prompt_commands(cwd: &Path) -> Vec<PromptCommandDef> {
    let mut directories = Vec::new();
    let mut cursor = Some(cwd);
    while let Some(dir) = cursor {
        directories.push(dir.to_path_buf());
        cursor = dir.parent();
    }

    let mut seen = BTreeSet::new();
    let mut commands = Vec::new();
    for dir in directories {
        let root = dir.join(".zo").join("commands");
        push_prompt_commands_from_root(&mut commands, &mut seen, &root);
        if commands.len() >= MAX_PROMPT_COMMANDS {
            commands.truncate(MAX_PROMPT_COMMANDS);
            return commands;
        }
    }

    // Personal commands live under the user's home directory and are available
    // in every project. Project commands are discovered first, so a project
    // command with the same name keeps priority via the `seen` set.
    for root in personal_command_roots() {
        push_prompt_commands_from_root(&mut commands, &mut seen, &root);
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

fn push_prompt_commands_from_root(
    commands: &mut Vec<PromptCommandDef>,
    seen: &mut BTreeSet<String>,
    root: &Path,
) {
    let mut files = Vec::new();
    collect_markdown_files(root, &mut files);
    files.sort();

    for path in files {
        let Some(name) = command_name_from_path(root, &path) else {
            continue;
        };
        let key = name.to_ascii_lowercase();
        if seen.contains(&key) {
            continue;
        }
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        let Some(command) = parse_prompt_command_file(name, path, &content) else {
            continue;
        };
        seen.insert(key);
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
                output.push_str(&run_inline_command(&command, args, base_dir, shell));
                at_line_start = false;
                prev_is_boundary = false;
            }
            '!' if at_line_start && peek_is_command_start(&mut chars) => {
                let command = collect_line(&mut chars);
                output.push_str(&run_inline_command(&command, args, base_dir, shell));
                // The line marker is fully consumed; the next char (if any) is a
                // newline, so the output stays on its own line.
                at_line_start = false;
                prev_is_boundary = false;
            }
            '@' if prev_is_boundary && peek_is_path_start(&mut chars) => {
                let token = collect_path_token(&mut chars);
                output.push_str(&embed_file(&token, args, base_dir));
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
/// grants Bash, and nothing while the session is read-only.
struct InlineShell<'a> {
    allowed_tools: &'a [String],
    read_only: bool,
}

impl InlineShell<'_> {
    /// The notice that stands in for `command` when it may not run.
    fn refusal(&self, command: &str) -> Option<String> {
        if self.read_only {
            return Some(format!("[!{command}: not run in read-only mode]"));
        }
        if self
            .allowed_tools
            .iter()
            .any(|spec| bash_rule_grants(spec, command))
        {
            return None;
        }
        Some(format!(
            "[!{command}: not run — allowed-tools does not grant Bash for it]"
        ))
    }
}

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
/// one-line notice rather than panicking.
fn run_inline_command(
    template: &str,
    args: &str,
    base_dir: &Path,
    shell: &InlineShell<'_>,
) -> String {
    let command = expand_prompt_arguments(template, args);
    let command = command.trim();
    if command.is_empty() {
        return String::new();
    }
    if let Some(notice) = shell.refusal(command) {
        return notice;
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
        Ok(output) => output.stdout.trim_end_matches('\n').to_string(),
        Err(error) => format!("[!{command}: {error}]"),
    }
}

/// Embed `@path` file contents. Relative paths resolve against `base_dir`; a
/// missing or unreadable file degrades to a one-line notice.
fn embed_file(token: &str, args: &str, base_dir: &Path) -> String {
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
    match fs::read_to_string(&resolved) {
        Ok(contents) => contents,
        Err(error) => format!("[@{raw}: {error}]"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// [`expand_command_template`] for a file whose `allowed-tools` is `Bash`.
    fn expand(body: &str, args: &str, base_dir: &Path) -> String {
        let allowed_tools = ["Bash".to_string()];
        let shell = InlineShell {
            allowed_tools: &allowed_tools,
            read_only: false,
        };
        expand_command_template(body, args, base_dir, &shell)
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

        let commands = discover_prompt_commands(&cwd);
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

        let commands = discover_prompt_commands(&cwd);
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

    #[test]
    fn discovers_personal_commands_in_home() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let home = temp_dir("home");
        let personal = home.join(".zo").join("commands");
        fs::create_dir_all(&personal).expect("personal commands dir");
        fs::write(personal.join("greet.md"), "Hello from home.").expect("write personal command");

        // A project directory with no commands of its own.
        let cwd = temp_dir("project");
        fs::create_dir_all(&cwd).expect("project dir");

        let previous_home = std::env::var_os("HOME");
        std::env::set_var("HOME", &home);
        let commands = discover_prompt_commands(&cwd);
        match previous_home {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }

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

        let commands = discover_prompt_commands(&cwd);
        assert!(find_prompt_command(&commands, "ignored-.other-tool").is_none());
        assert!(find_prompt_command(&commands, "ignored-.codex").is_none());

        let _ = fs::remove_dir_all(root);
    }

    /// Serializes the tests that mutate `HOME` (process-global state).
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
}
