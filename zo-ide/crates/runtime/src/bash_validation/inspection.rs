//! The narrow local shell for inspection agents. Parse, validate, then quote
//! argv again: glob expansion and shell syntax never turn data into commands.

use super::{FIND_MUTATING_PRIMARIES, MAX_ANALYZED_COMMAND_LEN};
// Git's read options are the core's read-only list's (t-132606): one list
// for what a read-only session may run and what this shell may.
pub(crate) use zerocode_core::shell_rule::{branch_query, git_read_options, remote_query};

struct Query {
    program: &'static str,
    subcommand: Option<&'static str>,
    denied_options: &'static [&'static str],
}

// One allowlist. Git's ambiguous branch/remote verbs have argument rules below.
const QUERIES: &[Query] = &[
    Query { program: "git", subcommand: Some("status"), denied_options: &[] },
    Query { program: "git", subcommand: Some("log"), denied_options: &[] },
    Query { program: "git", subcommand: Some("remote"), denied_options: &[] },
    Query { program: "git", subcommand: Some("branch"), denied_options: &[] },
    Query { program: "git", subcommand: Some("diff"), denied_options: &[] },
    Query { program: "git", subcommand: Some("show"), denied_options: &[] },
    Query { program: "ls", subcommand: None, denied_options: &[] },
    Query { program: "cat", subcommand: None, denied_options: &[] },
    Query { program: "head", subcommand: None, denied_options: &[] },
    Query { program: "tail", subcommand: None, denied_options: &[] },
    Query { program: "wc", subcommand: None, denied_options: &[] },
    Query { program: "find", subcommand: None, denied_options: FIND_MUTATING_PRIMARIES },
    Query { program: "stat", subcommand: None, denied_options: &[] },
];
const DENIAL: &str = "Inspection Bash permits one local read-only query; writes, network, shell syntax and unlisted commands/options are refused. Use the file tools or do the work in the parent.";

/// Validate a local query and return shell-safe argv with Git's external
/// helpers, lazy network fetching, pager and optional index writes disabled.
/// This is a host constraint; no tool input can switch it off.
pub fn command(source: &str) -> Result<String, String> {
    let words = literal_argv(source).ok_or_else(|| DENIAL.to_string())?;
    command_words(&words)
}

#[must_use]
pub fn literal_argv(source: &str) -> Option<Vec<String>> {
    if source.len() > MAX_ANALYZED_COMMAND_LEN
        || source.contains(['$', '`', '\\', '\n', '\r', ';', '|', '&', '>', '<', '(', ')', '{', '}'])
    {
        return None;
    }
    shlex::split(source)
}

fn command_words(words: &[String]) -> Result<String, String> {
    let Some(program) = words.first() else { return Err(DENIAL.to_string()); };
    let mut offset = 1;
    // -C is the only accepted global Git option, and its operand is quoted.
    if program == "git" && words.get(offset).is_some_and(|arg| arg == "-C") {
        offset += 2;
    }
    let query = QUERIES.iter().find(|query| query.program == program
        && query.subcommand.is_none_or(|sub| words.get(offset).is_some_and(|word| word == sub)))
        .ok_or_else(|| DENIAL.to_string())?;
    let args = &words[(offset + usize::from(query.subcommand.is_some())).min(words.len())..];
    if args.iter().any(|arg| query.denied_options.iter().any(|denied| arg.split('=').next() == Some(denied))) {
        return Err(DENIAL.to_string());
    }
    match query.subcommand {
        Some("branch") if !branch_query(args) => return Err(DENIAL.to_string()),
        Some("remote") if !remote_query(args) => return Err(DENIAL.to_string()),
        Some("status" | "log" | "diff" | "show") if !git_read_options(args) => return Err(DENIAL.to_string()),
        _ => {}
    }
    let quote = |arg: &str| shlex::try_quote(arg).map(std::borrow::Cow::into_owned).map_err(|_| DENIAL.to_string());
    let mut command_words = Vec::new();
    if program == "git" {
        command_words.extend(["env", "GIT_NO_LAZY_FETCH=1", "GIT_TERMINAL_PROMPT=0", "git", "--no-pager", "--no-optional-locks", "-c", "core.fsmonitor=false", "-c", "core.hooksPath=/dev/null"].into_iter().map(str::to_string));
        command_words.extend(words[1..offset].iter().map(|arg| quote(arg)).collect::<Result<Vec<_>, _>>()?);
        command_words.push(quote(query.subcommand.unwrap_or_default())?);
        if matches!(query.subcommand, Some("log" | "diff" | "show")) {
            command_words.extend(["--no-ext-diff".to_string(), "--no-textconv".to_string()]);
        }
        command_words.extend(args.iter().map(|arg| quote(arg)).collect::<Result<Vec<_>, _>>()?);
    } else {
        command_words.extend(words.iter().map(|arg| quote(arg)).collect::<Result<Vec<_>, _>>()?);
    }
    Ok(command_words.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_queries_and_git_helpers_are_constrained() {
        for source in ["git status --short", "git -C '/a path' remote -v", "git log -5 --oneline", "git branch --show-current", "git remote get-url origin", "git diff HEAD", "git show HEAD:README.md", "ls -la", "cat 'a file'", "head -n 4 a", "tail -n 4 a", "wc -l a", "find . -name '*.rs'", "stat a"] {
            assert!(command(source).is_ok(), "{source}");
        }
        for source in ["git fetch", "git branch new", "git branch -D old", "git remote update", "git remote show origin", "git remote add x y", "git diff --output=x", "git diff --out=x", "git diff --ext-dif", "git log --no-no-ext-diff", "git log --textconv", "git -c alias.status=evil status", "find . -exec sh x", "find . '-delete'", "find . -fprint out", "curl x", "cat $(id)", "cat a > b", "ls; touch x", "env git status", "bash -c ls"] {
            assert!(command(source).is_err(), "{source}");
        }
        let git = command("git diff HEAD").unwrap();
        assert!(git.contains("GIT_NO_LAZY_FETCH=1") && git.contains("--no-ext-diff") && git.contains("--no-optional-locks"));
    }
}
