//! The narrow local shell for inspection agents. Parse, validate, then quote
//! argv again: glob expansion and shell syntax never turn data into commands.

use super::{FIND_MUTATING_PRIMARIES, MAX_ANALYZED_COMMAND_LEN};

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
const BRANCH_OPTIONS: &[&str] = &["--list", "-l", "-a", "--all", "-r", "--remotes", "-v", "-vv", "--verbose", "--show-current", "--no-color", "--contains", "--no-contains", "--merged", "--no-merged", "--format", "--sort"];
// Positive option spellings are deliberate: Git accepts abbreviations such
// as --out for --output, so a blacklist of write flags would not be a guard.
const GIT_READ_OPTIONS: &[&str] = &[
    "--short", "--branch", "--porcelain", "--ignored", "--untracked-files",
    "--oneline", "--format", "--pretty", "--max-count", "--since", "--until",
    "--after", "--before", "--author", "--committer", "--grep", "--all",
    "--branches", "--tags", "--remotes", "--graph", "--decorate", "--date",
    "--no-walk", "--reverse", "--first-parent", "--merges", "--no-merges",
    "--name-only", "--name-status", "--stat", "--numstat", "--shortstat",
    "--summary", "--raw", "--patch", "--no-patch", "--no-color", "--color",
    "--check", "--exit-code", "--quiet", "--cached", "--staged", "--no-index",
    "--diff-filter", "--unified", "--ignore-all-space", "--ignore-space-change",
];
const GIT_READ_SHORT_OPTIONS: &[&str] = &["-s", "-b", "-sb", "-bs", "-z", "-p", "-u", "-n", "-w", "-M", "-C", "-R", "-r", "-m", "-c"];

fn git_read_options(args: &[String]) -> bool {
    let mut paths = false;
    args.iter().all(|arg| {
        if arg == "--" { paths = true; }
        if paths || !arg.starts_with('-') { return true; }
        GIT_READ_OPTIONS.contains(&arg.split('=').next().unwrap_or_default())
            || GIT_READ_SHORT_OPTIONS.contains(&arg.as_str())
            || arg.strip_prefix('-').is_some_and(|count| !count.is_empty() && count.chars().all(|ch| ch.is_ascii_digit()))
            || ["-n", "-U"].iter().any(|prefix| arg.strip_prefix(prefix).is_some_and(|count| !count.is_empty() && count.chars().all(|ch| ch.is_ascii_digit())))
    })
}

const DENIAL: &str = "Inspection Bash permits one local read-only query; writes, network, shell syntax and unlisted commands/options are refused. Use the file tools or do the work in the parent.";

/// Validate a local query and return shell-safe argv with Git's external
/// helpers, lazy network fetching, pager and optional index writes disabled.
/// This is a host constraint; no tool input can switch it off.
pub fn command(source: &str) -> Result<String, String> {
    if source.len() > MAX_ANALYZED_COMMAND_LEN
        || source.contains(['$', '`', '\\', '\n', '\r', ';', '|', '&', '>', '<', '(', ')', '{', '}'])
    {
        return Err(DENIAL.to_string());
    }
    let words = shlex::split(source).ok_or_else(|| DENIAL.to_string())?;
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
        Some("branch") if !args.iter().all(|arg| BRANCH_OPTIONS.contains(&arg.split('=').next().unwrap_or_default())) => return Err(DENIAL.to_string()),
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

fn remote_query(args: &[String]) -> bool {
    match args {
        [] => true,
        [flag] if matches!(flag.as_str(), "-v" | "--verbose") => true,
        [verb, rest @ ..] if verb == "get-url" => rest.iter().filter(|arg| !arg.starts_with('-')).count() == 1
            && rest.iter().all(|arg| !arg.starts_with('-') || matches!(arg.as_str(), "--all" | "--push")),
        _ => false,
    }
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
