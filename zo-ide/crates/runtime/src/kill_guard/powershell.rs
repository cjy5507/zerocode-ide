//! The calls a `PowerShell` command makes that end processes (t-84973), read
//! into the kill calls core judges ([`zerocode_core::shell_rule::kill`]), so
//! the `PowerShell` tool meets the guard the bash tool meets.
//!
//! Core reads `kill`, `pkill` and `killall` the way a POSIX shell runs them,
//! and the guard reads a `PowerShell` command with core too: an external
//! `pkill` or `kill` is the same call there. This reads what `PowerShell`
//! itself ends with — `Stop-Process` and its aliases `spps` and `kill`,
//! `taskkill`, `Get-Process … | Stop-Process`, and a process object's
//! `.Kill()` — each into the targets core knows. What it cannot name a
//! target for, it names [`KillTarget::Unknown`], which core asks a person
//! about; it never reads a call as signalling nothing.

use zerocode_core::shell_rule::kill::{KillCall, KillTarget, ProcessPattern};

/// One word of a statement, its quoting taken off. `listed` says it followed
/// a comma, so it is one more value of the word before it (`-Id 1,2`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Word {
    text: String,
    listed: bool,
}

/// One statement, and whether a pipe feeds it the one before.
#[derive(Debug, Default)]
struct Statement {
    words: Vec<Word>,
    piped: bool,
}

/// The calls `command` makes that end processes, as `PowerShell` runs it.
pub(crate) fn kill_calls(command: &str) -> Vec<KillCall> {
    let statements = statements(command);
    let mut calls = Vec::new();
    for (at, statement) in statements.iter().enumerate() {
        let Some(program) = statement.words.first() else {
            continue;
        };
        let args = &statement.words[1..];
        let targets = match program_name(&program.text).as_str() {
            "stop-process" | "spps" | "kill" => {
                let fed = (statement.piped && at > 0).then(|| &statements[at - 1]);
                stop_process_targets(args, fed)
            }
            "taskkill" => taskkill_targets(args),
            _ => continue,
        };
        calls.push(KillCall {
            written: written(statement),
            targets,
        });
    }
    // A process object's own `Kill()` — `(Get-Process -Id 5).Kill()`,
    // `[Diagnostics.Process]::GetProcessById(5).Kill()` — names its process
    // only when the script runs.
    if command.to_ascii_lowercase().contains(".kill(") {
        calls.push(KillCall {
            written: ".Kill()".to_string(),
            targets: vec![KillTarget::Unknown(".Kill()".to_string())],
        });
    }
    calls
}

/// A program's name as `PowerShell` resolves it: the last part of a path,
/// without `.exe`, in lower case.
fn program_name(word: &str) -> String {
    let name = word.rsplit(['/', '\\']).next().unwrap_or(word).to_ascii_lowercase();
    name.strip_suffix(".exe").map_or(name.clone(), str::to_string)
}

/// The statement as written, for a reason.
fn written(statement: &Statement) -> String {
    statement
        .words
        .iter()
        .map(|word| word.text.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

/// A parameter word (`-Id`, `-Name:foo`): its name in lower case and the
/// value joined to it by a colon.
fn parameter(word: &Word) -> Option<(String, Option<String>)> {
    let rest = word.text.strip_prefix('-')?;
    if rest.is_empty() || rest.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    Some(match rest.split_once(':') {
        Some((name, value)) => (name.to_ascii_lowercase(), Some(value.to_string())),
        None => (rest.to_ascii_lowercase(), None),
    })
}

/// Whether `name` is `full` or a prefix of it `PowerShell` would take.
fn names(name: &str, full: &str) -> bool {
    !name.is_empty() && full.starts_with(name)
}

/// A parameter's values: the one joined by a colon, or the word after it
/// and every word listed after that one.
fn values(args: &[Word], at: &mut usize, joined: Option<String>) -> Vec<String> {
    if let Some(value) = joined {
        return vec![value];
    }
    let mut values = Vec::new();
    if let Some(first) = args.get(*at) {
        values.push(first.text.clone());
        *at += 1;
        while let Some(next) = args.get(*at).filter(|word| word.listed) {
            values.push(next.text.clone());
            *at += 1;
        }
    }
    values
}

/// What one `-Id` value of `Stop-Process` ends.
fn id_target(value: &str) -> KillTarget {
    if value.eq_ignore_ascii_case("$pid") {
        // PowerShell's own process: the shell the tool started.
        return KillTarget::OwnShell;
    }
    match value.parse::<u32>() {
        Ok(pid) => KillTarget::Pid(pid),
        Err(_) => KillTarget::Unknown(value.to_string()),
    }
}

/// What one `-Name` value ends: the processes of that name, any case, its
/// wildcards (`*`, `?`) read as `PowerShell` reads them.
fn name_target(value: &str) -> KillTarget {
    if value.starts_with('$') {
        return KillTarget::Unknown(value.to_string());
    }
    let wildcard = value.contains(['*', '?', '[']);
    let text = if wildcard {
        let mut expression = String::new();
        for c in value.chars() {
            match c {
                '*' => expression.push_str(".*"),
                '?' => expression.push('.'),
                '[' | ']' => expression.push(c),
                other => expression.push_str(&regex_escape(other)),
            }
        }
        expression
    } else {
        value.to_string()
    };
    KillTarget::Matching(ProcessPattern {
        text,
        full: false,
        exact: true,
        ignore_case: true,
        regex: wildcard,
    })
}

fn regex_escape(c: char) -> String {
    if "\\.+()|{}^$".contains(c) {
        format!("\\{c}")
    } else {
        c.to_string()
    }
}

/// `Stop-Process`'s targets. `fed` is the statement a pipe feeds it from:
/// with no target of its own, it ends what that one lists.
fn stop_process_targets(args: &[Word], fed: Option<&Statement>) -> Vec<KillTarget> {
    let mut targets = Vec::new();
    let mut at = 0;
    let mut own = false;
    while let Some(word) = args.get(at) {
        at += 1;
        match parameter(word) {
            Some((name, joined)) if name == "id" || name == "pid" => {
                own = true;
                targets.extend(values(args, &mut at, joined).iter().map(|value| id_target(value)));
            }
            Some((name, joined)) if names(&name, "name") || name == "processname" => {
                own = true;
                targets.extend(values(args, &mut at, joined).iter().map(|value| name_target(value)));
            }
            Some((name, joined)) if names(&name, "inputobject") && name.len() > 1 => {
                own = true;
                let values = values(args, &mut at, joined);
                targets.push(KillTarget::Unknown(values.join(",")));
            }
            // A switch: -Force, -PassThru, -WhatIf, -Confirm.
            Some(_) => {}
            // The first word that is no parameter is the `-Id`.
            None => {
                own = true;
                at -= 1;
                targets.extend(values(args, &mut at, None).iter().map(|value| id_target(value)));
            }
        }
    }
    if own {
        return targets;
    }
    match fed {
        Some(fed) => fed_targets(fed),
        None => vec![KillTarget::Unknown("Stop-Process".to_string())],
    }
}

/// What a statement piped into `Stop-Process` lists: `Get-Process` (`gps`,
/// `ps`) by name or id, or every process when it names none. Anything else
/// feeds what only running it tells.
fn fed_targets(fed: &Statement) -> Vec<KillTarget> {
    let lists = fed
        .words
        .first()
        .is_some_and(|program| matches!(program_name(&program.text).as_str(), "get-process" | "gps" | "ps"));
    if !lists {
        return vec![KillTarget::Unknown(written(fed))];
    }
    let args = &fed.words[1..];
    let mut targets = Vec::new();
    let mut at = 0;
    while let Some(word) = args.get(at) {
        at += 1;
        match parameter(word) {
            Some((name, joined)) if name == "id" || name == "pid" => {
                targets.extend(values(args, &mut at, joined).iter().map(|value| id_target(value)));
            }
            Some((name, joined)) if names(&name, "name") || name == "processname" => {
                targets.extend(values(args, &mut at, joined).iter().map(|value| name_target(value)));
            }
            Some(_) => return vec![KillTarget::Unknown(written(fed))],
            // `Get-Process`'s first positional parameter is `-Name`.
            None => {
                at -= 1;
                targets.extend(values(args, &mut at, None).iter().map(|value| name_target(value)));
            }
        }
    }
    if targets.is_empty() {
        targets.push(KillTarget::Everything);
    }
    targets
}

/// `taskkill`'s targets: `/PID`, `/IM` (an image name, `*` allowed), and
/// with `/T` the children of each pid too. A `/FI` filter selects what only
/// running it tells.
fn taskkill_targets(args: &[Word]) -> Vec<KillTarget> {
    let mut targets = Vec::new();
    let mut tree = false;
    let mut at = 0;
    while let Some(word) = args.get(at) {
        at += 1;
        let option = word
            .text
            .strip_prefix(['/', '-'])
            .map(str::to_ascii_lowercase);
        match option.as_deref() {
            Some("pid") => {
                if let Some(value) = args.get(at) {
                    at += 1;
                    targets.push(id_target(&value.text));
                }
            }
            Some("im") => {
                if let Some(value) = args.get(at) {
                    at += 1;
                    let image = value.text.strip_suffix(".exe").unwrap_or(&value.text);
                    targets.push(name_target(image));
                }
            }
            Some("t") => tree = true,
            Some("fi") => {
                let filter = args.get(at).map_or_else(String::new, |value| value.text.clone());
                at += 1;
                targets.push(KillTarget::Unknown(filter));
            }
            Some("s" | "u" | "p") => at += 1,
            _ => {}
        }
    }
    if tree {
        let children: Vec<KillTarget> = targets
            .iter()
            .filter_map(|target| match target {
                KillTarget::Pid(pid) => Some(KillTarget::ChildrenOf(*pid)),
                _ => None,
            })
            .collect();
        targets.extend(children);
    }
    if targets.is_empty() {
        targets.push(KillTarget::Unknown("taskkill".to_string()));
    }
    targets
}

/// `command` cut into statements, as `PowerShell` reads it closely enough to
/// find each statement's program: `;`, a newline, `|`, `&&`, `||`, the call
/// operator `&`, and the brackets of a script block or a sub-expression end
/// one; a `$( … )` inside a double-quoted string runs, so it is read as
/// statements of its own; quotes, the backtick escape and comments are taken
/// off.
fn statements(command: &str) -> Vec<Statement> {
    let mut reader = Reader {
        chars: command.chars().collect(),
        at: 0,
        statements: vec![Statement::default()],
        word: None,
        listed: false,
    };
    reader.read(None);
    reader.end_word();
    reader.statements.retain(|statement| !statement.words.is_empty());
    reader.statements
}

struct Reader {
    chars: Vec<char>,
    at: usize,
    statements: Vec<Statement>,
    /// The word being read.
    word: Option<String>,
    /// Whether a comma came before the next word.
    listed: bool,
}

impl Reader {
    fn peek(&self, ahead: usize) -> Option<char> {
        self.chars.get(self.at + ahead).copied()
    }

    fn end_word(&mut self) {
        if let Some(text) = self.word.take() {
            let listed = std::mem::take(&mut self.listed);
            if let Some(statement) = self.statements.last_mut() {
                statement.words.push(Word { text, listed });
            }
        }
    }

    /// End the statement being read. An empty one stays the one to read
    /// into, keeping a pipe that came before it: a line may break after `|`.
    fn end_statement(&mut self, piped: bool) {
        self.end_word();
        self.listed = false;
        match self.statements.last_mut() {
            Some(last) if last.words.is_empty() => last.piped |= piped,
            _ => self.statements.push(Statement {
                words: Vec::new(),
                piped,
            }),
        }
    }

    fn push(&mut self, c: char) {
        self.word.get_or_insert_with(String::new).push(c);
    }

    /// Read until `close` (a sub-expression's `)`) or the end.
    fn read(&mut self, close: Option<char>) {
        let mut depth = 0usize;
        while let Some(c) = self.peek(0) {
            self.at += 1;
            match c {
                c if close == Some(c) && depth == 0 => return,
                '(' if close.is_some() => {
                    depth += 1;
                    self.end_statement(false);
                }
                ')' if close.is_some() => {
                    depth = depth.saturating_sub(1);
                    self.end_statement(false);
                }
                '\'' => self.single_quoted(),
                '"' => self.double_quoted(),
                '`' => {
                    if let Some(next) = self.peek(0) {
                        self.at += 1;
                        if next != '\n' {
                            self.push(next);
                        }
                    }
                }
                '#' if self.word.is_none() => self.skip_comment(),
                '<' if self.peek(0) == Some('#') => {
                    self.at += 1;
                    self.skip_block_comment();
                }
                '|' => {
                    if self.peek(0) == Some('|') {
                        self.at += 1;
                        self.end_statement(false);
                    } else {
                        self.end_statement(true);
                    }
                }
                '&' => {
                    if self.peek(0) == Some('&') {
                        self.at += 1;
                    }
                    self.end_statement(false);
                }
                ';' | '\n' | '\r' | '{' | '}' | '(' | ')' => self.end_statement(false),
                ',' => {
                    self.end_word();
                    self.listed = true;
                }
                '$' if self.peek(0) == Some('(') => {
                    self.at += 1;
                    self.end_statement(false);
                    self.read(Some(')'));
                    self.end_statement(false);
                }
                '@' if self.peek(0) == Some('(') => {
                    self.at += 1;
                    self.end_statement(false);
                }
                c if c.is_whitespace() => self.end_word(),
                c => self.push(c),
            }
        }
    }

    fn single_quoted(&mut self) {
        self.word.get_or_insert_with(String::new);
        while let Some(c) = self.peek(0) {
            self.at += 1;
            if c == '\'' {
                if self.peek(0) == Some('\'') {
                    self.at += 1;
                    self.push('\'');
                    continue;
                }
                return;
            }
            self.push(c);
        }
    }

    /// A double-quoted string: its text is one word, and each `$( … )` in it
    /// is read as statements of its own after the one the string is in.
    fn double_quoted(&mut self) {
        self.word.get_or_insert_with(String::new);
        let mut inner = Vec::new();
        while let Some(c) = self.peek(0) {
            self.at += 1;
            match c {
                '"' if self.peek(0) == Some('"') => {
                    self.at += 1;
                    self.push('"');
                }
                '"' => break,
                '`' => {
                    if let Some(next) = self.peek(0) {
                        self.at += 1;
                        self.push(next);
                    }
                }
                '$' if self.peek(0) == Some('(') => {
                    self.at += 1;
                    let start = self.at;
                    let mut depth = 1usize;
                    while let Some(c) = self.peek(0) {
                        self.at += 1;
                        match c {
                            '(' => depth += 1,
                            ')' => {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                    let end = self.at.saturating_sub(1).max(start);
                    inner.push(self.chars[start..end].iter().collect::<String>());
                }
                c => self.push(c),
            }
        }
        for text in inner {
            for statement in statements(&text) {
                self.statements.insert(self.statements.len() - 1, statement);
            }
        }
    }

    fn skip_comment(&mut self) {
        while let Some(c) = self.peek(0) {
            if c == '\n' {
                return;
            }
            self.at += 1;
        }
    }

    fn skip_block_comment(&mut self) {
        while let Some(c) = self.peek(0) {
            self.at += 1;
            if c == '#' && self.peek(0) == Some('>') {
                self.at += 1;
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::kill_calls;
    use zerocode_core::shell_rule::kill::{KillTarget, ProcessPattern};

    fn targets(command: &str) -> Vec<KillTarget> {
        kill_calls(command).into_iter().flat_map(|call| call.targets).collect()
    }

    fn name(text: &str, regex: bool) -> KillTarget {
        KillTarget::Matching(ProcessPattern {
            text: text.to_string(),
            full: false,
            exact: true,
            ignore_case: true,
            regex,
        })
    }

    #[test]
    fn powershell_kill_calls_read_stop_process_its_aliases_and_taskkill() {
        assert_eq!(targets("Stop-Process -Id 500"), [KillTarget::Pid(500)]);
        assert_eq!(targets("stop-process 500,600 -Force"), [KillTarget::Pid(500), KillTarget::Pid(600)]);
        assert_eq!(targets("spps -Id:500 -WhatIf"), [KillTarget::Pid(500)]);
        assert_eq!(targets("kill -Name zerocode-shell"), [name("zerocode-shell", false)]);
        assert_eq!(targets("Stop-Process -n 'zero*'"), [name("zero.*", true)]);
        assert_eq!(targets("Stop-Process -Id $PID"), [KillTarget::OwnShell]);
        assert_eq!(targets("Stop-Process -Id $target"), [KillTarget::Unknown("$target".to_string())]);
        assert_eq!(
            targets("C:\\Windows\\System32\\taskkill.exe /F /T /PID 500"),
            [KillTarget::Pid(500), KillTarget::ChildrenOf(500)]
        );
        assert_eq!(targets("taskkill /IM zerocode-shell.exe"), [name("zerocode-shell", false)]);
    }

    #[test]
    fn powershell_kill_calls_read_a_pipe_a_sub_expression_and_a_process_objects_kill() {
        assert_eq!(targets("Get-Process -Name zo | Stop-Process"), [name("zo", false)]);
        assert_eq!(targets("gps -Id 500 | spps"), [KillTarget::Pid(500)]);
        assert_eq!(targets("Get-Process | Stop-Process -Force"), [KillTarget::Everything]);
        assert!(matches!(
            targets("Get-Process | Where-Object { $_.CPU -gt 9 } | Stop-Process")[..],
            [KillTarget::Unknown(_)]
        ));
        assert_eq!(targets("Write-Host \"now $(Stop-Process -Id 500)\""), [KillTarget::Pid(500)]);
        assert_eq!(targets("if ($true) { Stop-Process -Id 500 }; echo done"), [KillTarget::Pid(500)]);
        assert_eq!(targets("& 'Stop-Process' -Id 500"), [KillTarget::Pid(500)]);
        assert!(matches!(targets("(Get-Process -Id 500).Kill()")[..], [KillTarget::Unknown(_)]));
    }

    #[test]
    fn powershell_kill_calls_leave_words_and_comments_alone() {
        assert!(targets("Write-Host 'Stop-Process -Id 500'").is_empty());
        assert!(targets("Get-Process -Name zo # | Stop-Process").is_empty());
        assert!(targets("<# Stop-Process -Id 500 #> Get-Date").is_empty());
        assert!(targets("Get-ChildItem; echo stop-process").is_empty());
        assert_eq!(targets("Get-Process -Name zo |\n  Stop-Process"), [name("zo", false)]);
    }
}
