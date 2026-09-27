//! Reads the model asked the shell for, done by the tool built for them.
//!
//! The prompt already says "prefer the dedicated tools" and the shell tool's
//! description repeats it; a day of transcripts still showed one bash call in
//! eight being `cat`, `sed -n`, `grep` or `find` (54 of 436, 2026-09-03).
//! Claude Code only nudges. Here the runtime does the routing itself: a shell
//! command that IS a plain read is run as `read_file` / `grep_search` /
//! `glob_search` — permission-scoped, output-capped, and counted in the
//! harness's file-state tracking — and the result says so, so the model
//! learns the door without paying a retry.
//!
//! Exact equivalents only. A command with any shell metacharacter (pipes,
//! redirects, globs, substitutions, chaining), an unknown flag, or a shape the
//! dedicated tool cannot reproduce byte-for-byte (`ls -la`, `tail`, `grep -F`,
//! `-A/-B/-C` context) runs in the shell as written. Being too strict costs one
//! shell run; being too loose changes what the model asked for.

/// What a shell read becomes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DedicatedRead {
    ReadFile {
        path: String,
        offset: Option<usize>,
        limit: Option<usize>,
    },
    Grep {
        pattern: String,
        path: Option<String>,
        case_insensitive: bool,
    },
    Glob {
        pattern: String,
        path: String,
    },
}

impl DedicatedRead {
    /// The tool the read is done by — the name the permission check and the
    /// model-facing note both use.
    pub(crate) const fn tool_name(&self) -> &'static str {
        match self {
            Self::ReadFile { .. } => crate::file_tools::READ_FILE_TOOL_NAME,
            Self::Grep { .. } => "grep_search",
            Self::Glob { .. } => "glob_search",
        }
    }

    /// The dedicated tool's input, in the shape its own dispatch reads.
    pub(crate) fn input(&self) -> serde_json::Value {
        match self {
            Self::ReadFile {
                path,
                offset,
                limit,
            } => {
                let mut input = serde_json::json!({ "path": path });
                if let Some(offset) = offset {
                    input["offset"] = serde_json::json!(offset);
                }
                if let Some(limit) = limit {
                    input["limit"] = serde_json::json!(limit);
                }
                input
            }
            Self::Grep {
                pattern,
                path,
                case_insensitive,
            } => {
                let mut input = serde_json::json!({ "pattern": pattern, "-n": true });
                if let Some(path) = path {
                    input["path"] = serde_json::json!(path);
                }
                if *case_insensitive {
                    input["-i"] = serde_json::json!(true);
                }
                input
            }
            Self::Glob { pattern, path } => serde_json::json!({ "pattern": pattern, "path": path }),
        }
    }
}

/// The line put before a redirected result, so the model sees which door its
/// shell read went through — and that the door exists.
pub(crate) fn redirect_note(read: &DedicatedRead, command: &str) -> String {
    format!(
        "[zo] `{}` ran as {} — the shell is for what the dedicated tools cannot do; call {} directly next time.",
        command.trim(),
        read.tool_name(),
        read.tool_name(),
    )
}

/// Characters that make a command more than one program and its arguments.
const SHELL_METACHARACTERS: &[char] = &[
    '|', '&', ';', '>', '<', '$', '`', '(', ')', '{', '}', '*', '?', '[', ']', '~', '\n', '\\',
];

/// Split a metacharacter-free command into words, honouring single and double
/// quotes. `None` when a quote is left open.
fn words(command: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut in_word = false;
    for ch in command.chars() {
        match quote {
            Some(open) if ch == open => quote = None,
            Some(_) => current.push(ch),
            None if ch == '\'' || ch == '"' => {
                quote = Some(ch);
                in_word = true;
            }
            None if ch.is_whitespace() => {
                if in_word {
                    out.push(std::mem::take(&mut current));
                    in_word = false;
                }
            }
            None => {
                current.push(ch);
                in_word = true;
            }
        }
    }
    if quote.is_some() {
        return None;
    }
    if in_word {
        out.push(current);
    }
    Some(out)
}

/// The dedicated read a shell command is exactly, or `None` to run the shell.
#[must_use]
pub(crate) fn dedicated_read_for(command: &str) -> Option<DedicatedRead> {
    if command.contains(SHELL_METACHARACTERS) {
        return None;
    }
    let parts = words(command)?;
    let (program, args) = parts.split_first()?;
    let program = std::path::Path::new(program)
        .file_name()?
        .to_str()?;
    match program {
        "cat" => cat(args),
        "head" => head(args),
        "sed" => sed(args),
        "grep" | "rg" => grep(args),
        "find" => find(args),
        _ => None,
    }
}

/// `cat FILE`, and nothing else — flags change the bytes.
fn cat(args: &[String]) -> Option<DedicatedRead> {
    match args {
        [path] if !path.starts_with('-') => Some(DedicatedRead::ReadFile {
            path: path.clone(),
            offset: None,
            limit: None,
        }),
        _ => None,
    }
}

/// `head -n N FILE` / `head -N FILE`.
fn head(args: &[String]) -> Option<DedicatedRead> {
    let (count, path) = match args {
        [flag, count, path] if flag == "-n" => (count.parse::<usize>().ok()?, path),
        [flag, path] => (flag.strip_prefix('-')?.parse::<usize>().ok()?, path),
        _ => return None,
    };
    (count > 0 && !path.starts_with('-')).then(|| DedicatedRead::ReadFile {
        path: path.clone(),
        offset: None,
        limit: Some(count),
    })
}

/// `sed -n 'A,Bp' FILE` / `sed -n Ap FILE` — the one sed a read is.
fn sed(args: &[String]) -> Option<DedicatedRead> {
    let [flag, script, path] = args else {
        return None;
    };
    if flag != "-n" || path.starts_with('-') {
        return None;
    }
    let range = script.strip_suffix('p')?;
    let (start, end) = if let Some((start, end)) = range.split_once(',') {
        (start.parse::<usize>().ok()?, end.parse::<usize>().ok()?)
    } else {
        let line = range.parse::<usize>().ok()?;
        (line, line)
    };
    (start >= 1 && end >= start).then(|| DedicatedRead::ReadFile {
        path: path.clone(),
        offset: Some(start),
        limit: Some(end - start + 1),
    })
}

/// `grep [-rnRiEHh…] PATTERN [PATH]` and `rg` alike. Flags that change the
/// output shape or the pattern's meaning (`-F`, `-w`, `-l`, `-c`, `-o`, `-v`,
/// context) are not a `grep_search`, so the shell keeps them.
fn grep(args: &[String]) -> Option<DedicatedRead> {
    let mut case_insensitive = false;
    let mut rest = args.iter();
    let mut pattern = None;
    for arg in rest.by_ref() {
        if let Some(flags) = arg.strip_prefix('-') {
            if flags.is_empty() || flags.starts_with('-') {
                return None;
            }
            for flag in flags.chars() {
                match flag {
                    'r' | 'R' | 'n' | 'E' | 'H' | 'h' | 's' => {}
                    'i' => case_insensitive = true,
                    _ => return None,
                }
            }
        } else {
            pattern = Some(arg.clone());
            break;
        }
    }
    let pattern = pattern.filter(|pattern| !pattern.is_empty())?;
    let path = match rest.as_slice() {
        [] => None,
        [path] if !path.starts_with('-') => Some(path.clone()),
        _ => return None,
    };
    Some(DedicatedRead::Grep {
        pattern,
        path,
        case_insensitive,
    })
}

/// `find PATH -name NAME` / `find PATH -type f -name NAME` — the file listing
/// `glob_search` answers under `PATH` with `**/NAME`.
fn find(args: &[String]) -> Option<DedicatedRead> {
    let (path, rest) = args.split_first()?;
    if path.starts_with('-') {
        return None;
    }
    let name = match rest {
        [flag, name] if flag == "-name" => name,
        [kind, file, flag, name] if kind == "-type" && file == "f" && flag == "-name" => name,
        _ => return None,
    };
    Some(DedicatedRead::Glob {
        pattern: format!("**/{name}"),
        path: path.clone(),
    })
}

/// Seconds a `sleep` must ask for before it counts as a wait on the world
/// rather than a pause in a script.
const WAIT_SLEEP_SECS: u64 = 60;

/// Is this command a wait on other agents — `zerocode-orc check --wait`, or a
/// long `sleep`? Such a command blocks the whole turn for as long as the others
/// take; run in the background its completion arrives as a task notification
/// and the turn stays free to talk and work.
#[must_use]
pub(crate) fn is_agent_wait(command: &str) -> bool {
    let Some(parts) = words(command) else {
        return false;
    };
    let Some((program, args)) = parts.split_first() else {
        return false;
    };
    let program = std::path::Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    match program {
        "zerocode-orc" => {
            args.first().is_some_and(|verb| verb == "check")
                && args.iter().any(|arg| arg == "--wait")
        }
        "sleep" => args
            .first()
            .and_then(|secs| secs.trim_end_matches('s').parse::<u64>().ok())
            .is_some_and(|secs| secs >= WAIT_SLEEP_SECS),
        _ => false,
    }
}

/// Is this compound command a wait — a poll whose sleeps add up to
/// [`WAIT_SLEEP_SECS`] or more? The loops a main turn held the person's words
/// behind for five hours on 2026-09-27 (t-11354) were of this shape:
/// `for i in $(seq 1 12); do <read a state file>; sleep 29; done`, and
/// `sleep 90; tail …`. A loop's body counts once per iteration its list
/// names (`$(seq a b)`, `{a..b}`, words); a `while`/`until` loop that sleeps
/// counts without end. Work with a pause in it — three builds a second apart
/// — does not add up to a wait. Asked only for a turn a person attends
/// (see `run_bash`).
#[must_use]
pub(crate) fn is_polling_wait(command: &str) -> bool {
    sleep_budget(command).is_some_and(|secs| secs >= WAIT_SLEEP_SECS)
}

/// One shell token: a word (its quotes and substitutions kept inside it), a
/// break between commands (`;` `&&` `||` `|` `&` newline), or a subshell's
/// parenthesis.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Word(String),
    Break,
    Open,
    Close,
}

/// Split a command into [`Token`]s. `None` when a quote or a substitution is
/// left open — a command the shell itself would refuse.
fn tokens(command: &str) -> Option<Vec<Token>> {
    let chars: Vec<char> = command.chars().collect();
    let mut out = Vec::new();
    let mut word = String::new();
    let mut at = 0;
    while at < chars.len() {
        let next = match chars[at] {
            '\'' => closing_single_quote(&chars, at + 1)? + 1,
            '"' => closing_double_quote(&chars, at + 1)? + 1,
            '`' => chars[at + 1..].iter().position(|&ch| ch == '`')? + at + 2,
            '\\' => (at + 2).min(chars.len()),
            '$' if chars.get(at + 1) == Some(&'(') => closing_paren(&chars, at + 2)? + 1,
            ch @ (';' | '\n' | '|' | '&' | '(' | ')') => {
                if !word.is_empty() {
                    out.push(Token::Word(std::mem::take(&mut word)));
                }
                let token = match ch {
                    '(' => Token::Open,
                    ')' => Token::Close,
                    _ => Token::Break,
                };
                if token != Token::Break || out.last() != Some(&Token::Break) {
                    out.push(token);
                }
                at += 1;
                continue;
            }
            ch if ch.is_whitespace() => {
                if !word.is_empty() {
                    out.push(Token::Word(std::mem::take(&mut word)));
                }
                at += 1;
                continue;
            }
            _ => at + 1,
        };
        word.extend(&chars[at..next]);
        at = next;
    }
    if !word.is_empty() {
        out.push(Token::Word(word));
    }
    Some(out)
}

/// Index of the `'` closing a single-quoted string whose body starts at `at`.
fn closing_single_quote(chars: &[char], at: usize) -> Option<usize> {
    chars.get(at..)?.iter().position(|&ch| ch == '\'').map(|end| end + at)
}

/// Index of the `"` closing a double-quoted string whose body starts at `at`.
fn closing_double_quote(chars: &[char], mut at: usize) -> Option<usize> {
    while at < chars.len() {
        match chars[at] {
            '\\' => at += 2,
            '"' => return Some(at),
            '$' if chars.get(at + 1) == Some(&'(') => at = closing_paren(chars, at + 2)? + 1,
            _ => at += 1,
        }
    }
    None
}

/// Index of the `)` closing a `$(` whose body starts at `at`.
fn closing_paren(chars: &[char], mut at: usize) -> Option<usize> {
    let mut depth = 1_usize;
    while at < chars.len() {
        match chars[at] {
            '\'' => at = closing_single_quote(chars, at + 1)? + 1,
            '"' => at = closing_double_quote(chars, at + 1)? + 1,
            '\\' => at += 2,
            '(' => {
                depth += 1;
                at += 1;
            }
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(at);
                }
                at += 1;
            }
            _ => at += 1,
        }
    }
    None
}

/// Seconds a command can sleep in the foreground, all its sleeps added up —
/// a loop's body times its iterations, a `while`/`until` loop that sleeps
/// without end, the longest branch of an `if`. `None` when the command is not
/// one this reads (an arithmetic `for ((…))`, a `case`, an unbalanced one).
fn sleep_budget(command: &str) -> Option<u64> {
    let tokens = tokens(command)?;
    let mut reader = Reader {
        tokens: &tokens,
        at: 0,
    };
    let budget = reader.list(&[])?;
    (reader.at == tokens.len()).then_some(budget)
}

/// Reads [`Token`]s as the shell's commands, adding up their sleeps.
struct Reader<'a> {
    tokens: &'a [Token],
    at: usize,
}

impl Reader<'_> {
    fn word(&self) -> Option<&str> {
        match self.tokens.get(self.at) {
            Some(Token::Word(word)) => Some(word),
            _ => None,
        }
    }

    fn take(&mut self, token: &Token) -> Option<()> {
        if self.tokens.get(self.at) == Some(token) {
            self.at += 1;
            Some(())
        } else {
            None
        }
    }

    fn take_word(&mut self, keyword: &str) -> Option<()> {
        if self.word() == Some(keyword) {
            self.at += 1;
            Some(())
        } else {
            None
        }
    }

    fn skip_breaks(&mut self) {
        while self.tokens.get(self.at) == Some(&Token::Break) {
            self.at += 1;
        }
    }

    /// Commands up to one of `ends` (a keyword, left unread) or a `)`.
    fn list(&mut self, ends: &[&str]) -> Option<u64> {
        let mut budget = 0_u64;
        loop {
            self.skip_breaks();
            let ended = match self.tokens.get(self.at) {
                None | Some(Token::Close) => true,
                Some(Token::Word(word)) => ends.contains(&word.as_str()),
                Some(Token::Open | Token::Break) => false,
            };
            if ended {
                return Some(budget);
            }
            budget = budget.saturating_add(self.command()?);
        }
    }

    fn command(&mut self) -> Option<u64> {
        let budget = match self.tokens.get(self.at)? {
            Token::Open => {
                self.at += 1;
                let inner = self.list(&[])?;
                self.take(&Token::Close)?;
                inner
            }
            Token::Close | Token::Break => return None,
            Token::Word(word) => match word.as_str() {
                "for" => self.for_loop()?,
                "while" | "until" => self.condition_loop()?,
                "if" => self.if_clause()?,
                "{" => {
                    self.at += 1;
                    let inner = self.list(&["}"])?;
                    self.take_word("}")?;
                    inner
                }
                "case" => return None,
                _ => return self.simple(),
            },
        };
        // A compound command's redirections (`done < files.txt`).
        while self.word().is_some() {
            self.at += 1;
        }
        Some(budget)
    }

    fn for_loop(&mut self) -> Option<u64> {
        self.at += 1;
        self.word()?;
        self.at += 1;
        let mut iterations = 1_u64;
        if self.take_word("in").is_some() {
            iterations = 0;
            while let Some(word) = self.word().filter(|word| *word != "do") {
                iterations = iterations.saturating_add(list_count(word));
                self.at += 1;
            }
        }
        self.skip_breaks();
        self.take_word("do")?;
        let body = self.list(&["done"])?;
        self.take_word("done")?;
        Some(iterations.saturating_mul(body))
    }

    fn condition_loop(&mut self) -> Option<u64> {
        self.at += 1;
        let condition = self.list(&["do"])?;
        self.take_word("do")?;
        let body = self.list(&["done"])?;
        self.take_word("done")?;
        Some(if condition.saturating_add(body) > 0 {
            u64::MAX
        } else {
            0
        })
    }

    fn if_clause(&mut self) -> Option<u64> {
        self.at += 1;
        let mut conditions = self.list(&["then"])?;
        self.take_word("then")?;
        let mut longest = 0_u64;
        loop {
            longest = longest.max(self.list(&["elif", "else", "fi"])?);
            match self.word()? {
                "fi" => break,
                "elif" => {
                    self.at += 1;
                    conditions = conditions.saturating_add(self.list(&["then"])?);
                    self.take_word("then")?;
                }
                _ => self.at += 1,
            }
        }
        self.take_word("fi")?;
        Some(conditions.saturating_add(longest))
    }

    /// A simple command, up to the next break: a `sleep`'s seconds, plus what
    /// its substitutions sleep. A function definition (`name() { … }`) runs
    /// nothing where it stands.
    fn simple(&mut self) -> Option<u64> {
        let start = self.at;
        while self.word().is_some() {
            self.at += 1;
        }
        if self.at == start + 1
            && self.tokens.get(self.at) == Some(&Token::Open)
            && self.tokens.get(self.at + 1) == Some(&Token::Close)
        {
            self.at += 2;
            self.command()?;
            return Some(0);
        }
        let parts: Vec<&str> = self.tokens[start..self.at]
            .iter()
            .filter_map(|token| match token {
                Token::Word(part) => Some(part.as_str()),
                _ => None,
            })
            .collect();
        let mut budget = 0_u64;
        for part in &parts {
            for inner in substitutions_in(part) {
                budget = budget.saturating_add(sleep_budget(&inner)?);
            }
        }
        let mut program = parts.iter().skip_while(|part| part.contains('='));
        if program
            .next()
            .is_some_and(|name| name.rsplit('/').next() == Some("sleep"))
        {
            for arg in program {
                budget = budget.saturating_add(sleep_seconds(arg)?);
            }
        }
        Some(budget)
    }
}

/// The bodies of a word's `$(…)` substitutions, bare or double-quoted — the
/// commands the shell runs to build the word.
fn substitutions_in(word: &str) -> Vec<String> {
    let chars: Vec<char> = word.chars().collect();
    let mut out = Vec::new();
    let mut at = 0;
    while at < chars.len() {
        match chars[at] {
            '\'' => {
                at = closing_single_quote(&chars, at + 1).map_or(chars.len(), |end| end + 1);
            }
            '\\' => at += 2,
            '$' if chars.get(at + 1) == Some(&'(') && chars.get(at + 2) != Some(&'(') => {
                let Some(end) = closing_paren(&chars, at + 2) else {
                    break;
                };
                out.push(chars[at + 2..end].iter().collect());
                at = end + 1;
            }
            _ => at += 1,
        }
    }
    out
}

/// Seconds one `sleep` argument asks for: `90`, `1.5` (whole seconds), `2m`.
fn sleep_seconds(arg: &str) -> Option<u64> {
    let (number, unit) = arg
        .find(|ch: char| ch.is_ascii_alphabetic())
        .map_or((arg, "s"), |at| arg.split_at(at));
    let scale = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 60 * 60,
        "d" => 24 * 60 * 60,
        _ => return None,
    };
    let whole = number.split('.').next()?;
    let whole = if whole.is_empty() {
        0
    } else {
        whole.parse::<u64>().ok()?
    };
    Some(whole.saturating_mul(scale))
}

/// How many values one word of a `for … in` list stands for: `$(seq …)` and
/// `{a..b}` their count, any other word one.
fn list_count(word: &str) -> u64 {
    if let Some(inner) = word.strip_prefix("$(").and_then(|rest| rest.strip_suffix(')')) {
        let args: Vec<&str> = inner.split_whitespace().collect();
        if args.first() != Some(&"seq") {
            return 1;
        }
        let numbers: Option<Vec<i64>> = args[1..].iter().map(|arg| arg.parse().ok()).collect();
        let count = match numbers.as_deref() {
            Some([last]) => *last,
            Some([first, last]) => last - first + 1,
            Some([first, step, last]) if *step != 0 => (last - first) / step + 1,
            _ => 1,
        };
        return u64::try_from(count).unwrap_or(0);
    }
    word.strip_prefix('{')
        .and_then(|rest| rest.strip_suffix('}'))
        .and_then(|range| range.split_once(".."))
        .and_then(|(first, last)| Some((first.parse::<i64>().ok()?, last.parse::<i64>().ok()?)))
        .map_or(1, |(first, last)| first.abs_diff(last) + 1)
}

/// The note put before a wait the runtime backgrounded on the model's behalf.
pub(crate) fn auto_background_note(command: &str) -> String {
    format!(
        "[zo] `{}` waits on other agents, so it was started in the background: its output arrives as a task notification when it finishes. Keep working — and keep answering the user — meanwhile.",
        command.trim()
    )
}

#[cfg(test)]
mod tests {
    use super::{dedicated_read_for, is_agent_wait, is_polling_wait, DedicatedRead};

    #[test]
    fn plain_reads_become_the_tool_built_for_them() {
        assert_eq!(
            dedicated_read_for("cat src/main.rs"),
            Some(DedicatedRead::ReadFile {
                path: "src/main.rs".into(),
                offset: None,
                limit: None
            })
        );
        assert_eq!(
            dedicated_read_for("sed -n '120,160p' ui/shell.js"),
            Some(DedicatedRead::ReadFile {
                path: "ui/shell.js".into(),
                offset: Some(120),
                limit: Some(41)
            })
        );
        assert_eq!(
            dedicated_read_for("head -n 40 Cargo.toml"),
            Some(DedicatedRead::ReadFile {
                path: "Cargo.toml".into(),
                offset: None,
                limit: Some(40)
            })
        );
        assert_eq!(
            dedicated_read_for("grep -rn \"fn main\" crates/"),
            Some(DedicatedRead::Grep {
                pattern: "fn main".into(),
                path: Some("crates/".into()),
                case_insensitive: false
            })
        );
        assert_eq!(
            dedicated_read_for("rg -i todo"),
            Some(DedicatedRead::Grep {
                pattern: "todo".into(),
                path: None,
                case_insensitive: true
            })
        );
        assert_eq!(
            dedicated_read_for("find ui -type f -name '*.css'"),
            None,
            "a glob in the name is a metacharacter to the shell — the shell keeps it"
        );
        assert_eq!(
            dedicated_read_for("find crates -name Cargo.toml"),
            Some(DedicatedRead::Glob {
                pattern: "**/Cargo.toml".into(),
                path: "crates".into()
            })
        );
    }

    /// Anything the dedicated tool would not reproduce byte-for-byte stays a
    /// shell command: pipes, redirects, flags that change the output, or a
    /// shape with no equivalent.
    #[test]
    fn everything_else_stays_in_the_shell() {
        for command in [
            "cat a.rs b.rs",
            "cat -n a.rs",
            "cat a.rs | head",
            "grep -rn foo . > out.txt",
            "grep -F 'a.b' src",
            "grep -A3 foo src",
            "grep -l foo src",
            "ls -la ui",
            "tail -n 20 log.txt",
            "sed -i 's/a/b/' x",
            "sed -n 'p' x",
            "head x y",
            "cat \"unterminated",
            "cargo test",
            "find . -newer x -name y",
        ] {
            assert_eq!(dedicated_read_for(command), None, "{command}");
        }
    }

    #[test]
    fn a_wait_on_other_agents_is_recognised_and_a_pause_is_not() {
        assert!(is_agent_wait("zerocode-orc check --wait --timeout-ms 600000"));
        assert!(is_agent_wait("/usr/local/bin/zerocode-orc check --wait"));
        assert!(is_agent_wait("sleep 600"));
        assert!(!is_agent_wait("zerocode-orc check"));
        assert!(!is_agent_wait("sleep 2"));
        assert!(!is_agent_wait("cargo test"));
    }

    /// The polls a main turn ran for five hours on 2026-09-27 (t-11354), in
    /// the shape it wrote them: a loop of short sleeps around a status read,
    /// and long sleeps chained before a look. Each held the person's words
    /// until it ended; each is a wait. Work with a pause in it is not.
    #[test]
    fn a_poll_whose_sleeps_add_up_to_a_wait_is_a_wait() {
        let polls = [
            "A=/state/agents; L=/scratch/reflex-bench.lock\nst(){ node -e 'try{console.log(JSON.parse(require(\"fs\").readFileSync(process.argv[1],\"utf8\")).status)}catch{console.log(\"?\")}' \"$A/$1.json\"; }\nfor i in $(seq 1 9); do a=$(st agent-1); b=$(st agent-2); l=$([ -e \"$L\" ] && echo lock || echo free); echo \"$(date +%H:%M:%S) impl=$a review=$b $l\"; [ \"$a\" != running ] || [ \"$b\" != running ] || [ \"$l\" = free ] && break; sleep 30; done",
            "A=/state/agents\nst(){ node -e 'console.log(1)' \"$A/$1.json\"; }\nfor i in $(seq 1 12); do a=$(st agent-1); r=$(for f in $(ls -t /tmp/impl/run1[0-9].log 2>/dev/null); do printf \"%s \" \"$(basename $f .log)\"; done); [ $((i % 4)) -eq 1 ] && echo \"$(date +%H:%M:%S) impl=$a runs=[${r}]\"; if [ \"$a\" != running ]; then break; fi; sleep 29; done; ls -t /tmp/impl/ | head -4",
            "for i in $(seq 1 16); do grep -q \"bin exit=\" /tmp/cargo-bin.log && break; sleep 15; done; grep -E \"exit=|test result\" /tmp/cargo-bin.log | head -8",
            "sleep 90; tail -c 400 /tmp/gpu-after.log; echo; ls /tmp/gpu-before",
            "sleep 118; sleep 60; grep -c METRIC /tmp/gpu-after.log",
            "until [ -e /tmp/released ]; do sleep 2; done; echo free",
            "while true; do date; sleep 5; done",
            "for i in {1..30}; do sleep 2; done",
        ];
        for poll in polls {
            assert!(is_polling_wait(poll), "a wait: {poll}");
        }
        let work = [
            "for i in 1 2 3; do cargo build && break; sleep 1; done",
            "for f in a b c; do sleep 10; done",
            "sleep 2",
            "echo 'sleep 600'",
            "grep -rn sleep src/",
            "while read -r f; do rustfmt \"$f\"; done < files.txt",
            "cargo test -p tools",
            "printf 'for i in $(seq 1 99); do sleep 60; done'",
        ];
        for command in work {
            assert!(!is_polling_wait(command), "not a wait: {command}");
        }
    }
}
