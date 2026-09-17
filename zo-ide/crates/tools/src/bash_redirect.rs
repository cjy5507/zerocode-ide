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
            Self::ReadFile { .. } => "read_file",
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

/// The note put before a wait the runtime backgrounded on the model's behalf.
pub(crate) fn auto_background_note(command: &str) -> String {
    format!(
        "[zo] `{}` waits on other agents, so it was started in the background: its output arrives as a task notification when it finishes. Keep working — and keep answering the user — meanwhile.",
        command.trim()
    )
}

#[cfg(test)]
mod tests {
    use super::{dedicated_read_for, is_agent_wait, DedicatedRead};

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
}
