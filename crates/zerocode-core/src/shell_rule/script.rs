//! A shell command read the way the shell will run it (t-80602): its simple
//! commands, the pipelines that join them, and — inside every word — the
//! command substitutions the shell runs before the command itself. A backtick
//! example inside a double-quoted argument is a command, and so is a `$( )`
//! in an unquoted here-document; a single-quoted span, a quoted
//! here-document and a `$'…'` string are only letters.
//!
//! Not a full shell grammar: `case` patterns, arithmetic and brace expansion
//! are read as plain letters. What a reader needs from it is which words sit
//! in command position and which text the shell will run as a command; both
//! are read conservatively — a grouping it cannot place still ends a command
//! rather than gluing two together.

/// How deep substitutions may nest before the rest is no longer read; a
/// command that deep is read as unreadable ([`Script::overflowed`]). Chosen,
/// not measured: a hand-written command rarely nests past three, and the
/// bound keeps the reader's recursion off the stack's edge for a hostile one.
const MAX_DEPTH: usize = 32;

/// Words that open or close a compound command in command position; the
/// command proper starts after them (`then pkill …`, `! kill …`).
const RESERVED_LEADERS: &[&str] = &[
    "if", "then", "else", "elif", "do", "while", "until", "!", "{", "}", "time",
];

/// A whole command line: its pipelines in order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Script {
    pub(crate) pipelines: Vec<Pipeline>,
    /// Substitutions nested past [`MAX_DEPTH`] were left unread.
    pub(crate) overflowed: bool,
}

/// Commands joined by `|`: each one's output is the next one's input.
pub(crate) type Pipeline = Vec<Command>;

/// One simple command: its words with the leading assignments and compound
/// keywords taken off, and the scripts the shell runs on its behalf — process
/// substitutions, here-document substitutions, substitutions in a redirection
/// target or an assignment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Command {
    pub(crate) words: Vec<Word>,
    pub(crate) nested: Vec<Script>,
    /// What it reads on its input from here-documents and here-strings,
    /// its letters only — a shell handed it runs it (`bash <<'EOF'`).
    pub(crate) here: Vec<String>,
}

/// One word as the shell hands it to a command.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Word {
    /// Its letters with the quoting taken off and every expansion left out.
    pub(crate) text: String,
    /// The parameters it expands (`!` for `$!`, `PID` for `$PID`/`${PID}`).
    pub(crate) params: Vec<String>,
    /// The command substitutions it holds, each read as a script.
    pub(crate) subs: Vec<Script>,
    /// It holds an arithmetic expansion.
    pub(crate) arithmetic: bool,
}

impl Word {
    /// Whether the shell fills any of it in when it runs.
    pub(crate) fn expands(&self) -> bool {
        !self.params.is_empty() || !self.subs.is_empty() || self.arithmetic
    }

    /// Its letters, when nothing of it is filled in at run time.
    pub(crate) fn literal(&self) -> Option<&str> {
        (!self.expands()).then_some(self.text.as_str())
    }
}

impl Command {
    /// Every script the shell runs for this command: what it nests and the
    /// substitutions in its words.
    pub(crate) fn scripts(&self) -> impl Iterator<Item = &Script> {
        self.nested
            .iter()
            .chain(self.words.iter().flat_map(|word| word.subs.iter()))
    }
}

/// Read `command` as the shell would split it.
pub(crate) fn read(command: &str) -> Script {
    let chars: Vec<char> = command.chars().collect();
    let mut reader = Reader {
        chars: &chars,
        at: 0,
        depth: 0,
        overflowed: false,
    };
    let mut script = reader.script(Until::End);
    script.overflowed |= reader.overflowed;
    script
}

/// Where a script being read ends.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Until {
    End,
    /// The `)` that closes a `$(` or a `<(`.
    CloseParen,
}

/// A here-document waiting for the end of its line.
struct Pending {
    delimiter: String,
    /// Quoted (`<<'EOF'`): its body is letters only.
    quoted: bool,
    /// `<<-`: leading tabs are taken off each line.
    strip_tabs: bool,
    /// Which command of the pipeline being read it belongs to.
    command: usize,
}

struct Reader<'a> {
    chars: &'a [char],
    at: usize,
    depth: usize,
    overflowed: bool,
}

impl Reader<'_> {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.at).copied()
    }

    fn peek_at(&self, ahead: usize) -> Option<char> {
        self.chars.get(self.at + ahead).copied()
    }

    /// A nested reader for `text` one level deeper — a backtick's body, an
    /// unquoted here-document, an `eval`'s words.
    fn nested_script(&mut self, text: &str) -> Script {
        if self.depth >= MAX_DEPTH {
            self.overflowed = true;
            return Script::default();
        }
        let chars: Vec<char> = text.chars().collect();
        let mut inner = Reader {
            chars: &chars,
            at: 0,
            depth: self.depth + 1,
            overflowed: false,
        };
        let script = inner.script(Until::End);
        self.overflowed |= inner.overflowed;
        script
    }

    fn script(&mut self, until: Until) -> Script {
        let mut script = Script::default();
        let mut pipeline: Pipeline = Vec::new();
        let mut command = Command::default();
        let mut pending: Vec<Pending> = Vec::new();
        // Subshell parentheses opened inside this script, so their `)` is not
        // taken for the one that closes a `$(`.
        let mut groups = 0usize;

        loop {
            self.skip_blanks();
            let Some(c) = self.peek() else { break };
            match c {
                ')' if until == Until::CloseParen && groups == 0 => {
                    self.at += 1;
                    break;
                }
                ')' => {
                    self.at += 1;
                    groups = groups.saturating_sub(1);
                    end_command(&mut pipeline, &mut command);
                }
                '(' => {
                    self.at += 1;
                    groups += 1;
                    end_command(&mut pipeline, &mut command);
                }
                '#' => self.skip_comment(),
                '\n' => {
                    self.at += 1;
                    end_command(&mut pipeline, &mut command);
                    self.read_heredocs(&mut pending, &mut pipeline);
                    end_pipeline(&mut script, &mut pipeline);
                }
                ';' => {
                    self.at += 1;
                    end_command(&mut pipeline, &mut command);
                    end_pipeline(&mut script, &mut pipeline);
                }
                '&' => {
                    self.at += 1;
                    if self.peek() == Some('>') {
                        // `&>file`: a redirection of both streams.
                        self.at += 1;
                        if self.peek() == Some('>') {
                            self.at += 1;
                        }
                        self.redirect_target(&mut command);
                        continue;
                    }
                    if self.peek() == Some('&') {
                        self.at += 1;
                    }
                    end_command(&mut pipeline, &mut command);
                    end_pipeline(&mut script, &mut pipeline);
                }
                '|' => {
                    self.at += 1;
                    let or = self.peek() == Some('|');
                    if or || self.peek() == Some('&') {
                        self.at += 1;
                    }
                    end_command(&mut pipeline, &mut command);
                    if or {
                        end_pipeline(&mut script, &mut pipeline);
                    }
                }
                '<' | '>' => self.redirection(&mut command, &mut pending, pipeline.len()),
                _ => {
                    let word = self.word();
                    if self.fd_before_redirection(&word) {
                        continue;
                    }
                    command.words.push(word);
                }
            }
        }
        // A here-document whose line never ended has no body to read.
        end_command(&mut pipeline, &mut command);
        end_pipeline(&mut script, &mut pipeline);
        script
    }

    fn skip_blanks(&mut self) {
        while let Some(c) = self.peek() {
            if c == ' ' || c == '\t' || c == '\r' {
                self.at += 1;
            } else if c == '\\' && self.peek_at(1) == Some('\n') {
                self.at += 2;
            } else {
                break;
            }
        }
    }

    fn skip_comment(&mut self) {
        while let Some(c) = self.peek() {
            if c == '\n' {
                break;
            }
            self.at += 1;
        }
    }

    /// `2>` and `3<`: digits glued to a redirection name a descriptor, never
    /// an argument.
    fn fd_before_redirection(&self, word: &Word) -> bool {
        matches!(self.peek(), Some('<' | '>'))
            && !word.expands()
            && !word.text.is_empty()
            && word.text.bytes().all(|b| b.is_ascii_digit())
            && self
                .chars
                .get(self.at.wrapping_sub(1))
                .is_some_and(char::is_ascii_digit)
    }

    /// `<`, `>`, `>>`, `>&`, `<&`, `<>`, `>|`, `<<<`, `<<`, `<<-`, and the
    /// process substitutions `<(` / `>(`.
    fn redirection(&mut self, command: &mut Command, pending: &mut Vec<Pending>, at: usize) {
        let first = self.peek().unwrap_or('>');
        self.at += 1;
        if self.peek() == Some('(') {
            // A process substitution runs its script beside the command.
            self.at += 1;
            let script = self.sub_script();
            command.nested.push(script);
            return;
        }
        if first == '<' && self.peek() == Some('<') {
            self.at += 1;
            if self.peek() == Some('<') {
                // A here-string: one word, read like any other.
                self.at += 1;
                if let Some(text) = self.redirect_target(command) {
                    command.here.push(text);
                }
                return;
            }
            let strip_tabs = self.peek() == Some('-');
            if strip_tabs {
                self.at += 1;
            }
            self.skip_blanks();
            let quoted = self.chars[self.at..]
                .iter()
                .take_while(|c| !is_word_end(**c))
                .any(|c| matches!(*c, '\'' | '"' | '\\'));
            let word = self.word();
            pending.push(Pending {
                delimiter: word.text,
                quoted,
                strip_tabs,
                command: at,
            });
            return;
        }
        if matches!(self.peek(), Some('>' | '&' | '|')) {
            self.at += 1;
        }
        self.redirect_target(command);
    }

    /// The word after a redirection: a file, not an argument — but what it
    /// substitutes still runs. Its letters come back.
    fn redirect_target(&mut self, command: &mut Command) -> Option<String> {
        self.skip_blanks();
        if self.peek().is_none_or(is_word_end) {
            return None;
        }
        let target = self.word();
        command.nested.extend(target.subs);
        Some(target.text)
    }

    /// Read the bodies of the here-documents opened on the line just ended.
    fn read_heredocs(&mut self, pending: &mut Vec<Pending>, pipeline: &mut Pipeline) {
        for doc in pending.drain(..) {
            let mut body = String::new();
            while self.peek().is_some() {
                let start = self.at;
                while self.peek().is_some_and(|c| c != '\n') {
                    self.at += 1;
                }
                let line: String = self.chars[start..self.at].iter().collect();
                if self.peek().is_some() {
                    self.at += 1;
                }
                let compared = if doc.strip_tabs {
                    line.trim_start_matches('\t')
                } else {
                    line.as_str()
                };
                if compared == doc.delimiter {
                    break;
                }
                body.push_str(&line);
                body.push('\n');
            }
            // An unquoted body expands like a double-quoted word, and what
            // it substitutes runs whichever command it was written for.
            let scripts = if doc.quoted {
                Vec::new()
            } else {
                self.expanded_text(&body)
            };
            let at = if doc.command < pipeline.len() {
                doc.command
            } else {
                pipeline.push(Command::default());
                pipeline.len() - 1
            };
            pipeline[at].nested.extend(scripts);
            pipeline[at].here.push(body);
        }
    }

    /// The substitutions in text the shell expands but does not split into
    /// words (an unquoted here-document's body).
    fn expanded_text(&mut self, text: &str) -> Vec<Script> {
        let chars: Vec<char> = text.chars().collect();
        let mut inner = Reader {
            chars: &chars,
            at: 0,
            depth: self.depth,
            overflowed: false,
        };
        let mut word = Word::default();
        while inner.peek().is_some() {
            inner.double_quoted_char(&mut word);
        }
        self.overflowed |= inner.overflowed;
        word.subs
    }

    /// One word, up to an unquoted blank or operator.
    fn word(&mut self) -> Word {
        let mut word = Word::default();
        while let Some(c) = self.peek() {
            if is_word_end(c) {
                break;
            }
            match c {
                '\\' => {
                    self.at += 1;
                    match self.peek() {
                        Some('\n') => self.at += 1,
                        Some(escaped) => {
                            word.text.push(escaped);
                            self.at += 1;
                        }
                        None => {}
                    }
                }
                '\'' => {
                    self.at += 1;
                    while let Some(q) = self.peek() {
                        self.at += 1;
                        if q == '\'' {
                            break;
                        }
                        word.text.push(q);
                    }
                }
                '"' => {
                    self.at += 1;
                    self.double_quoted(&mut word);
                }
                '`' => {
                    self.at += 1;
                    self.backtick(&mut word);
                }
                '$' => self.dollar(&mut word, false),
                _ => {
                    word.text.push(c);
                    self.at += 1;
                }
            }
        }
        word
    }

    /// The inside of `"…"`, its closing quote consumed.
    fn double_quoted(&mut self, word: &mut Word) {
        while let Some(c) = self.peek() {
            if c == '"' {
                self.at += 1;
                return;
            }
            self.double_quoted_char(word);
        }
    }

    /// One unit of double-quoted text: a letter, an escape, an expansion.
    fn double_quoted_char(&mut self, word: &mut Word) {
        let Some(c) = self.peek() else { return };
        match c {
            '\\' => {
                self.at += 1;
                match self.peek() {
                    Some('\n') => self.at += 1,
                    Some(escaped @ ('$' | '`' | '"' | '\\')) => {
                        word.text.push(escaped);
                        self.at += 1;
                    }
                    _ => word.text.push('\\'),
                }
            }
            '`' => {
                self.at += 1;
                self.backtick(word);
            }
            '$' => self.dollar(word, true),
            _ => {
                word.text.push(c);
                self.at += 1;
            }
        }
    }

    /// A backtick substitution, its opening backtick consumed: its body is
    /// read with the backslash escapes the shell takes off first, then run.
    fn backtick(&mut self, word: &mut Word) {
        let mut body = String::new();
        while let Some(c) = self.peek() {
            self.at += 1;
            match c {
                '`' => break,
                '\\' => match self.peek() {
                    Some(escaped @ ('`' | '\\' | '$')) => {
                        body.push(escaped);
                        self.at += 1;
                    }
                    _ => body.push('\\'),
                },
                _ => body.push(c),
            }
        }
        let script = self.nested_script(&body);
        word.subs.push(script);
    }

    /// A `$` expansion. `quoted` inside double quotes, where `$'` is letters.
    fn dollar(&mut self, word: &mut Word, quoted: bool) {
        self.at += 1;
        match self.peek() {
            Some('(') if self.peek_at(1) == Some('(') => {
                self.at += 2;
                self.skip_arithmetic();
                word.arithmetic = true;
            }
            Some('(') => {
                self.at += 1;
                let script = self.sub_script();
                word.subs.push(script);
            }
            Some('{') => {
                self.at += 1;
                // Its letters, and apart the expansions a default value can
                // hold (`${X:-$(pkill …)}`), which still run.
                let mut inside = Word::default();
                let mut depth = 1usize;
                while let Some(c) = self.peek() {
                    match c {
                        '$' => self.dollar(&mut inside, true),
                        '`' => {
                            self.at += 1;
                            self.backtick(&mut inside);
                        }
                        _ => {
                            self.at += 1;
                            match c {
                                '{' => depth += 1,
                                '}' => depth -= 1,
                                _ => {}
                            }
                            if depth == 0 {
                                break;
                            }
                            inside.text.push(c);
                        }
                    }
                }
                word.subs.append(&mut inside.subs);
                word.params.append(&mut inside.params);
                word.arithmetic |= inside.arithmetic;
                word.params.push(parameter_name(&inside.text));
            }
            Some('\'') if !quoted => {
                self.at += 1;
                self.ansi_c(word);
            }
            Some('"') if !quoted => {
                self.at += 1;
                self.double_quoted(word);
            }
            Some(c) if c.is_ascii_alphabetic() || c == '_' => {
                let mut name = String::new();
                while let Some(c) = self.peek() {
                    if c.is_ascii_alphanumeric() || c == '_' {
                        name.push(c);
                        self.at += 1;
                    } else {
                        break;
                    }
                }
                word.params.push(name);
            }
            Some(c) if c.is_ascii_digit() || "!$?#@*-".contains(c) => {
                self.at += 1;
                word.params.push(c.to_string());
            }
            _ => word.text.push('$'),
        }
    }

    /// `$'…'`: letters, with the escapes it takes.
    fn ansi_c(&mut self, word: &mut Word) {
        while let Some(c) = self.peek() {
            self.at += 1;
            match c {
                '\'' => return,
                '\\' => {
                    let Some(escaped) = self.peek() else { return };
                    self.at += 1;
                    word.text.push(match escaped {
                        'n' => '\n',
                        't' => '\t',
                        other => other,
                    });
                }
                _ => word.text.push(c),
            }
        }
    }

    /// The script of a `$(` or `<(`, its `)` consumed.
    fn sub_script(&mut self) -> Script {
        if self.depth >= MAX_DEPTH {
            self.overflowed = true;
            self.skip_balanced();
            return Script::default();
        }
        self.depth += 1;
        let script = self.script(Until::CloseParen);
        self.depth -= 1;
        script
    }

    /// Past a `)` that balances the `(` already consumed, unread.
    fn skip_balanced(&mut self) {
        let mut depth = 1usize;
        while let Some(c) = self.peek() {
            self.at += 1;
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return;
                    }
                }
                _ => {}
            }
        }
    }

    /// Past the `))` of an arithmetic expansion.
    fn skip_arithmetic(&mut self) {
        let mut depth = 2usize;
        while let Some(c) = self.peek() {
            self.at += 1;
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return;
                    }
                }
                _ => {}
            }
        }
    }
}

/// The parameter a `${…}` names: `!` for `${!}`, `PID` for `${PID:-1}`, and
/// the whole inside for anything else (`${!ref}` is an indirection, never
/// the last background job).
fn parameter_name(inside: &str) -> String {
    let identifier: String = inside
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    if identifier.is_empty() {
        inside.to_string()
    } else {
        identifier
    }
}

/// Whether `c`, unquoted, ends a word.
fn is_word_end(c: char) -> bool {
    matches!(
        c,
        ' ' | '\t' | '\r' | '\n' | ';' | '&' | '|' | '<' | '>' | '(' | ')'
    )
}

/// Close the command being read into the pipeline, its leading assignments
/// and compound keywords taken off. Their substitutions still run, so they
/// stay with the command as scripts it nests.
fn end_command(pipeline: &mut Pipeline, command: &mut Command) {
    let mut taken = std::mem::take(command);
    let lead = taken
        .words
        .iter()
        .take_while(|word| {
            is_assignment(word)
                || word
                    .literal()
                    .is_some_and(|text| RESERVED_LEADERS.contains(&text))
        })
        .count();
    for word in taken.words.drain(..lead) {
        taken.nested.extend(word.subs);
    }
    if !taken.words.is_empty() || !taken.nested.is_empty() || !taken.here.is_empty() {
        pipeline.push(taken);
    }
}

fn end_pipeline(script: &mut Script, pipeline: &mut Pipeline) {
    if !pipeline.is_empty() {
        script.pipelines.push(std::mem::take(pipeline));
    }
}

/// `NAME=value` before a command: an assignment, not the command.
pub(crate) fn is_assignment(word: &Word) -> bool {
    word.text.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && !name.starts_with(|c: char| c.is_ascii_digit())
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}
