//! A message may only send a person to a command this build has (t-11378).
//!
//! A login failure told a person to run `/login` and `zo login`, and neither
//! exists here — following the words did nothing. This reads every string a
//! person can be shown from the sources of `zo` and its provider layer, finds
//! each slash command and each `zo` verb the prose names, and asks the real
//! registries whether it is there: [`zo_ide::slash::Slash::from_word`] for the
//! composer's commands, and for verbs [`super::ANSWERED_BEFORE_A_SESSION`]
//! and the launch parser itself. No list of names is written here.

use std::path::{Path, PathBuf};

/// The crates whose words reach a person: `zo` itself and the provider layer
/// whose errors a turn prints.
const MESSAGE_SOURCES: &[&str] = &["src", "../api/src"];

#[test]
fn no_message_names_a_command_this_build_lacks() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    for source in MESSAGE_SOURCES {
        collect_sources(&root.join(source), &mut files);
    }
    assert!(files.len() > 50, "the sources were not found under {}", root.display());
    let mut missing = Vec::new();
    for file in &files {
        let text = std::fs::read_to_string(file).expect("a source file reads");
        for literal in string_literals(shipped_part(&text)) {
            for command in named_commands(&literal) {
                if !exists(&command) {
                    let shown: String = literal.chars().take(100).collect();
                    missing.push(format!("{}: {command:?} in {shown:?}", file.display()));
                }
            }
        }
    }
    assert!(
        missing.is_empty(),
        "messages name commands this build does not have:\n{}",
        missing.join("\n")
    );
}

/// The checker finds what it is meant to find, so a quiet pass above means
/// clean words and not a blind reader.
#[test]
fn the_checker_sees_a_missing_command_and_passes_a_real_one() {
    let found = |text: &str| named_commands(text);
    assert_eq!(found("run `/login google` first"), vec![Command::Slash("login".into())]);
    assert_eq!(found("    • shell:  zo login [provider]"), vec![Command::Verb("login".into())]);
    assert_eq!(found("Run `zo login openai` to reconnect."), vec![Command::Verb("login".into())]);
    assert!(!exists(&Command::Slash("login".into())));
    assert!(!exists(&Command::Verb("login".into())));
    assert!(exists(&Command::Slash("model".into())));
    assert!(exists(&Command::Verb("mcp".into())));
    assert!(exists(&Command::Verb("models".into())));
    // A path, a URL and prose about zo are not commands.
    assert!(found("wrote /tmp/x.json and https://example.com/login").is_empty());
    assert!(found("zo can read it").is_empty());
    let source = "let a = '\"'; let b = \"one /login two\"; // \"/exit now\"\n";
    assert_eq!(string_literals(source), vec!["one /login two".to_string()]);
}

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Slash(String),
    Verb(String),
}

fn exists(command: &Command) -> bool {
    match command {
        Command::Slash(word) => zo_ide::slash::Slash::from_word(word).is_some(),
        Command::Verb(word) => {
            super::ANSWERED_BEFORE_A_SESSION.iter().any(|(name, _)| name == word)
                || zo_ide::ide::args::parse(std::slice::from_ref(word)).is_ok()
        }
    }
}

fn collect_sources(dir: &Path, into: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if name != "tests" {
                collect_sources(&path, into);
            }
        } else if name.ends_with(".rs") && name != "tests.rs" && !name.ends_with("_tests.rs") {
            into.push(path);
        }
    }
}

/// The file up to its test module, which by this codebase's habit is last.
fn shipped_part(text: &str) -> &str {
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        if line.trim() == "#[cfg(test)]" {
            let rest = text[at + line.len()..].trim_start();
            if rest.starts_with("mod ") || rest.starts_with("pub mod ") {
                return &text[..at];
            }
        }
        at += line.len();
    }
    text
}

/// The contents of every string literal outside comments, as written.
fn string_literals(source: &str) -> Vec<String> {
    let chars: Vec<char> = source.chars().collect();
    let mut literals = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '/' if chars.get(i + 1) == Some(&'/') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                i += 2;
                while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                    i += 1;
                }
                i += 2;
            }
            // A char literal ('"', '\'') — not a lifetime, which has no close.
            '\'' if chars.get(i + 1) == Some(&'\\') || chars.get(i + 2) == Some(&'\'') => {
                i += 2;
                while i < chars.len() && chars[i] != '\'' {
                    i += 1;
                }
                i += 1;
            }
            'r' if matches!(chars.get(i + 1), Some('"' | '#')) && !ident_before(&chars, i) => {
                let hashes = chars[i + 1..].iter().take_while(|&&c| c == '#').count();
                let open = i + 1 + hashes;
                if chars.get(open) != Some(&'"') {
                    i += 1;
                    continue;
                }
                let mut end = open + 1;
                while end < chars.len()
                    && !(chars[end] == '"' && chars[end + 1..].iter().take(hashes).filter(|&&c| c == '#').count() == hashes)
                {
                    end += 1;
                }
                literals.push(chars[open + 1..end.min(chars.len())].iter().collect());
                i = end + 1 + hashes;
            }
            '"' => {
                let mut end = i + 1;
                while end < chars.len() && chars[end] != '"' {
                    end += if chars[end] == '\\' { 2 } else { 1 };
                }
                literals.push(chars[i + 1..end.min(chars.len())].iter().collect());
                i = end + 1;
            }
            _ => i += 1,
        }
    }
    literals
}

fn ident_before(chars: &[char], at: usize) -> bool {
    at > 0 && (chars[at - 1].is_alphanumeric() || chars[at - 1] == '_')
}

/// The commands a piece of prose tells a person to run. Only prose counts —
/// a literal with no space is a path, a key or a pointer, never a sentence.
///
/// A slash command is `/word` standing where a word starts; a following `/`
/// or `.` makes it a path. A verb is `zo word` in backticks, or in a usage
/// column (`zo word  …`, `zo word [arg]`), since plain prose says "zo can".
fn named_commands(text: &str) -> Vec<Command> {
    if !text.contains(' ') {
        return Vec::new();
    }
    let chars: Vec<char> = text.chars().collect();
    let word_at = |from: usize| -> String {
        chars[from..]
            .iter()
            .take_while(|c| c.is_ascii_lowercase() || **c == '-')
            .collect()
    };
    let mut commands = Vec::new();
    for i in 0..chars.len() {
        let starts_word = i == 0 || matches!(chars[i - 1], ' ' | '\t' | '\n' | '`' | '(' | '\'' | '"');
        if chars[i] == '/' && starts_word {
            let word = word_at(i + 1);
            let after = chars.get(i + 1 + word.chars().count());
            let continues = after.is_some_and(|c| c.is_alphanumeric() || matches!(c, '/' | '.' | '_'));
            if word.starts_with(|c: char| c.is_ascii_lowercase()) && !continues {
                commands.push(Command::Slash(word));
            }
        }
        if starts_word && chars[i..].starts_with(&['z', 'o', ' ']) {
            let word = word_at(i + 3);
            if word.is_empty() {
                continue;
            }
            let rest: String = chars[i + 3 + word.chars().count()..].iter().take(2).collect();
            let quoted = i > 0 && chars[i - 1] == '`';
            if quoted || rest == "  " || rest.starts_with(" [") {
                commands.push(Command::Verb(word));
            }
        }
    }
    commands
}
