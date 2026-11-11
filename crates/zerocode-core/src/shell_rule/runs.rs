//! The programs a shell command runs, reached the way the shell will run
//! them (t-80602): through `&&`, `;` and `|`, leading assignments, wrappers
//! (`sudo`, `nice`, `env`, `xargs`, …), a path's last part, the string of an
//! `sh -c` or an `eval`, a shell fed a here-document or an `echo` pipe, and
//! every command substitution. The kill guard reads its calls through this
//! walk ([`super::kill::kill_calls`]) and zo's desk guard the programs it
//! names ([`programs_run`], t-104888); neither splits a command line its own
//! way.

use super::SHELL_COMMANDS;
use super::parse::WRAPPER_COMMANDS;
use super::script::{self, Command, Script, Word};

/// How many times an `sh -c` or `eval` string is read inside another before
/// the rest is called unreadable. Chosen, not measured: the incident nested
/// none, a hand-written script rarely goes past two (`ssh … "bash -c '…'"`),
/// and each level is read again in full, so the bound keeps a hostile string
/// from costing the hook its 800 ms.
const MAX_STRING_DEPTH: usize = 8;

/// Wrappers that run the command after them and are not in
/// [`WRAPPER_COMMANDS`], which the intent classifier reads.
const RUNS_THE_NEXT_COMMAND: &[&str] = &["sudo", "command", "exec", "builtin"];

/// What a walk does at each step: which text it reads, and what it makes of
/// each program the shell will run.
pub(super) trait Walker {
    /// Whether a string the shell is handed as a script whose letters are
    /// filled in at run time is read anyway, its letters only. When not, it
    /// is [`Walker::unread`].
    const READS_FILLED_IN: bool;

    /// Whether `text` — the whole command, or a string handed to a shell —
    /// is worth reading at all; one that is not is passed over unread.
    fn worth_reading(&self, text: &str) -> bool;

    /// A string handed to a shell that the walk does not read: filled in at
    /// run time (unless [`Walker::READS_FILLED_IN`]), or nested past
    /// `MAX_STRING_DEPTH`.
    fn unread(&mut self, text: &str);

    /// Substitutions nested too deep for the reader were left unread.
    fn overflowed(&mut self);

    /// A program the shell will run: `program` is its name
    /// ([`program_name`]), `words` start at its command word, `command` is
    /// the simple command it sits in, `upstream` the commands before it in
    /// its pipeline, and `fed` says an `xargs` feeds it their output.
    fn program(
        &mut self,
        program: &str,
        words: &[Word],
        command: &Command,
        upstream: &[Command],
        fed: bool,
    );
}

/// Walk `command` with `walker`, when it is worth reading.
pub(super) fn walk(command: &str, walker: &mut impl Walker) {
    if walker.worth_reading(command) {
        walk_script(&script::read(command), walker, 0);
    }
}

fn walk_script<W: Walker>(script: &Script, walker: &mut W, depth: usize) {
    if script.overflowed {
        walker.overflowed();
    }
    for pipeline in &script.pipelines {
        for (at, command) in pipeline.iter().enumerate() {
            for nested in command.scripts() {
                walk_script(nested, walker, depth);
            }
            step(command, &pipeline[..at], walker, depth);
        }
    }
}

/// The program `command` runs, through its wrappers, an `xargs` that feeds
/// it the output of `upstream`, a shell's `-c` string or input, an `eval`.
fn step<W: Walker>(command: &Command, upstream: &[Command], walker: &mut W, depth: usize) {
    let mut words = command.words.as_slice();
    let mut fed = false;
    loop {
        let Some(program) = words.first().and_then(Word::literal).map(program_name) else {
            return;
        };
        let args = &words[1..];
        if let Some(rest) = after_wrapper(&program, args) {
            fed |= program == "xargs";
            words = rest;
            continue;
        }
        if SHELL_COMMANDS.contains(&program.as_str()) {
            match shell_string(args) {
                Some(word) => run_text(&word.text, word.expands(), walker, depth),
                None if args
                    .iter()
                    .all(|word| word.literal().is_some_and(|text| text.starts_with('-'))) =>
                {
                    // A shell with no script file runs what it reads.
                    for text in &command.here {
                        run_text(text, false, walker, depth);
                    }
                    if command.here.is_empty()
                        && !fed
                        && let Some((text, expands)) = printed_by(upstream)
                    {
                        run_text(&text, expands, walker, depth);
                    }
                }
                None => {}
            }
            return;
        }
        if program == "eval" {
            let text = args
                .iter()
                .map(|word| word.text.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            run_text(&text, args.iter().any(Word::expands), walker, depth);
            return;
        }
        walker.program(&program, words, command, upstream, fed);
        return;
    }
}

/// A string the shell is handed as a script: read in its turn, or — when
/// part of it is filled in at run time, or it is nested too deep — left to
/// the walker as unread.
fn run_text<W: Walker>(text: &str, expands: bool, walker: &mut W, depth: usize) {
    if !walker.worth_reading(text) {
        return;
    }
    if (expands && !W::READS_FILLED_IN) || depth >= MAX_STRING_DEPTH {
        walker.unread(text);
        return;
    }
    walk_script(&script::read(text), walker, depth + 1);
}

/// One program a shell command runs ([`programs_run`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramRun {
    /// Its name: the command word's last path part, ASCII case folded.
    pub program: String,
    /// Its arguments' letters, the quoting taken off and every expansion
    /// left out.
    pub args: Vec<String>,
    /// What it reads on its input from here-documents and here-strings.
    pub input: Vec<String>,
}

/// Every program `command` runs, reached the way the kill guard reaches its
/// calls. `worth_reading` is asked first of the whole command and of every
/// string handed to a shell: text it says no to is not read, so a caller
/// that looks for a few names keeps every other command off the reader. A
/// string filled in at run time is read for its letters.
#[must_use]
pub fn programs_run(command: &str, worth_reading: impl Fn(&str) -> bool) -> Vec<ProgramRun> {
    struct Programs<F> {
        worth_reading: F,
        found: Vec<ProgramRun>,
    }
    impl<F: Fn(&str) -> bool> Walker for Programs<F> {
        const READS_FILLED_IN: bool = true;
        fn worth_reading(&self, text: &str) -> bool {
            (self.worth_reading)(text)
        }
        fn unread(&mut self, _text: &str) {}
        fn overflowed(&mut self) {}
        fn program(
            &mut self,
            program: &str,
            words: &[Word],
            command: &Command,
            _upstream: &[Command],
            _fed: bool,
        ) {
            self.found.push(ProgramRun {
                program: program.to_string(),
                args: words[1..].iter().map(|word| word.text.clone()).collect(),
                input: command.here.clone(),
            });
        }
    }
    let mut programs = Programs {
        worth_reading,
        found: Vec::new(),
    };
    walk(command, &mut programs);
    programs.found
}

/// The letters `echo` or `printf` hands a pipe, or the here-document a `cat`
/// without a file hands it (`cat <<'EOF' | bash`), when that is all upstream.
fn printed_by(upstream: &[Command]) -> Option<(String, bool)> {
    let [source] = upstream else { return None };
    let (program, args) = source.words.split_first()?;
    let program = program_name(program.literal()?);
    let args: Vec<&Word> = args
        .iter()
        .skip_while(|word| word.literal().is_some_and(|text| text.starts_with('-')))
        .collect();
    if program == "cat" {
        // A file operand is what `cat` prints instead; `-` is its input.
        return (args.is_empty() && !source.here.is_empty()).then(|| (source.here.concat(), false));
    }
    if program != "echo" && program != "printf" {
        return None;
    }
    let text = args
        .iter()
        .map(|word| word.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    Some((text, args.iter().any(|word| word.expands())))
}

/// The program a command word runs: its last path part, ASCII case folded —
/// a case-insensitive disk finds `PKILL` too.
pub(super) fn program_name(word: &str) -> String {
    word.rsplit('/').next().unwrap_or(word).to_ascii_lowercase()
}

/// The words after a wrapper's own options and arguments, when `program`
/// is a wrapper; `Some(&[])` for a wrapper that only looks a command up.
pub(super) fn after_wrapper<'w>(program: &str, args: &'w [Word]) -> Option<&'w [Word]> {
    if !WRAPPER_COMMANDS.contains(&program) && !RUNS_THE_NEXT_COMMAND.contains(&program) {
        return None;
    }
    let first = args.first().and_then(Word::literal);
    if program == "command" && matches!(first, Some("-v" | "-V")) {
        return Some(&[]);
    }
    let takes_value = wrapper_value_flags(program);
    let mut at = 0;
    while let Some(text) = args.get(at).and_then(Word::literal) {
        if text == "--" {
            at += 1;
            break;
        }
        if text.len() < 2 || !text.starts_with('-') {
            break;
        }
        at += 1;
        if takes_value.contains(&text) {
            at += 1;
        }
    }
    match program {
        "timeout" => {
            if args
                .get(at)
                .and_then(Word::literal)
                .is_some_and(|text| text.starts_with(|c: char| c.is_ascii_digit()))
            {
                at += 1;
            }
        }
        "env" => {
            while args.get(at).is_some_and(script::is_assignment) {
                at += 1;
            }
        }
        _ => {}
    }
    Some(args.get(at..).unwrap_or_default())
}

/// The options of a wrapper that take the next word as their value.
fn wrapper_value_flags(wrapper: &str) -> &'static [&'static str] {
    match wrapper {
        "sudo" => &[
            "-u", "-g", "-h", "-p", "-C", "-U", "-r", "-t", "-D", "-R", "-T",
        ],
        "doas" => &["-u", "-C"],
        "xargs" => &[
            "-I", "-J", "-L", "-n", "-P", "-R", "-S", "-s", "-E", "-d", "-a",
        ],
        "nice" | "ionice" => &["-n", "-c", "-p"],
        "timeout" => &["-s", "-k", "--signal", "--kill-after"],
        "stdbuf" => &["-i", "-o", "-e"],
        "env" => &["-u", "-C", "-P"],
        "exec" => &["-a"],
        _ => &[],
    }
}

/// The script word of `sh -c '…'`: the first operand after options, when
/// one of them carries `c`.
fn shell_string(args: &[Word]) -> Option<&Word> {
    let mut command_flag = false;
    for word in args {
        match word.literal() {
            Some(text) if text.starts_with('-') && !text.starts_with("--") && text.len() > 1 => {
                command_flag |= text.contains('c');
            }
            Some(text) if text.starts_with("--") => {}
            _ => return command_flag.then_some(word),
        }
    }
    None
}

#[cfg(test)]
mod tests;
