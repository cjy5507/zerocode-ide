//! The shared-working-tree table (zo's track 4-1 guard, t-10916 moved it
//! here): the whole-tree mutating forms — `cargo fmt`, `git add -A`,
//! `git reset --hard`, `git clean -f`, … — that tangle or destroy work another
//! agent has in flight in the same tree, each with its scoped alternative.
//!
//! Detection is a pure function of the command string
//! ([`first_workspace_scope_violation`]). What refuses a command on it — and
//! when, since that guard is opt-in — is zo's (`tools::workspace_scope_guard`);
//! the command guard's baseline reads the same table
//! (`todays_rule`).

use super::split_command_segments;

/// One whole-tree command form the guard refuses, with the reason and the
/// scoped alternative. All `&'static str` — the taxonomy is fixed, so a match
/// returns one of the consts below rather than allocating per call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkspaceScopeViolation {
    /// The offending command family, e.g. `"cargo fmt"` or `"git add -A"`.
    pub kind: &'static str,
    /// Why running it in a shared tree is unsafe.
    pub risk: &'static str,
    /// The scoped command to use instead.
    pub suggestion: &'static str,
}

pub const CARGO_FMT_GLOBAL: WorkspaceScopeViolation = WorkspaceScopeViolation {
    kind: "cargo fmt (broad reformat)",
    risk: "reformats files outside this agent's explicit changeset in the shared working tree, clobbering edits another agent has in flight",
    suggestion: "use `cargo fmt --check` to only verify, or format explicit files with `rustfmt --config skip_children=true <files>`",
};

pub const RUSTFMT_BROAD: WorkspaceScopeViolation = WorkspaceScopeViolation {
    kind: "rustfmt (recursive reformat)",
    risk: "a rustfmt invocation on a module root can recursively reformat child modules outside this agent's explicit changeset",
    suggestion: "use `rustfmt --check` to verify, or add `--config skip_children=true` when formatting explicit files",
};

pub const GIT_ADD_ALL: WorkspaceScopeViolation = WorkspaceScopeViolation {
    kind: "git add -A / git add .",
    risk: "stages every change in the shared tree, including files another agent is editing",
    suggestion: "stage explicit paths — `git add <path>...` — for only the files this agent changed",
};

pub const GIT_COMMIT_ALL: WorkspaceScopeViolation = WorkspaceScopeViolation {
    kind: "git commit -a",
    risk: "auto-stages and commits every modified tracked file in the shared tree, not just this agent's work",
    suggestion: "stage explicit paths first, then `git commit` (without -a) to commit only what you staged",
};

pub const GIT_RESET_HARD: WorkspaceScopeViolation = WorkspaceScopeViolation {
    kind: "git reset --hard",
    risk: "discards all uncommitted changes in the shared tree, destroying work other agents have not committed",
    suggestion: "scope the discard — `git restore <path>` for specific files — or `git stash` to set work aside reversibly",
};

pub const GIT_CHECKOUT_DOT: WorkspaceScopeViolation = WorkspaceScopeViolation {
    kind: "git checkout .",
    risk: "overwrites every modified file in the shared tree with HEAD, destroying other agents' uncommitted work",
    suggestion: "scope it — `git checkout -- <path>` (or `git restore <path>`) for only this agent's files",
};

pub const GIT_RESTORE_DOT: WorkspaceScopeViolation = WorkspaceScopeViolation {
    kind: "git restore .",
    risk: "reverts every modified file in the shared tree, destroying other agents' uncommitted work",
    suggestion: "scope it — `git restore <path>` for only the files this agent changed",
};

pub const GIT_CLEAN_FORCE: WorkspaceScopeViolation = WorkspaceScopeViolation {
    kind: "git clean -f",
    risk: "deletes untracked files across the shared tree, including new files another agent just created",
    suggestion: "preview first with `git clean -n`, then remove explicit paths — `git clean -f -- <path>`",
};

/// The first whole-tree violation across the command's operator-separated
/// segments (so `foo && git add -A` is caught), or `None`. Pure — the unit
/// tests drive this directly without touching the environment.
#[must_use]
pub fn first_workspace_scope_violation(command: &str) -> Option<WorkspaceScopeViolation> {
    split_command_segments(command)
        .into_iter()
        .find_map(segment_violation)
}

/// Inspect one already-split segment. Strips a leading `sudo` (and its flags)
/// and any `KEY=val` env assignments, then dispatches on the real command.
fn segment_violation(segment: &str) -> Option<WorkspaceScopeViolation> {
    let tokens: Vec<&str> = segment.split_whitespace().collect();
    let mut idx = 0;

    if tokens.get(idx) == Some(&"sudo") {
        idx += 1;
        while tokens.get(idx).is_some_and(|tok| tok.starts_with('-')) {
            idx += 1;
        }
    }
    while tokens.get(idx).is_some_and(|tok| is_env_assignment(tok)) {
        idx += 1;
    }

    let command = *tokens.get(idx)?;
    let args = &tokens[idx + 1..];
    match command {
        "cargo" => cargo_violation(args),
        "rustfmt" => rustfmt_violation(args),
        "git" => git_violation(args),
        _ => None,
    }
}

/// `KEY=value` shell env-assignment prefix (`FOO=bar cargo …`). Mirrors the
/// name rule the validation parser uses: non-empty, alphanumeric + underscore.
fn is_env_assignment(token: &str) -> bool {
    token.split_once('=').is_some_and(|(key, _)| {
        !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// Direct `rustfmt` can recursively walk child modules when invoked on a module
/// root (for example `lib.rs`), causing the same unrelated-file noise as broad
/// `cargo fmt`. Read-only `--check` is safe, and mutating explicit-file formats
/// are safe only when recursion is disabled with `skip_children=true`.
fn rustfmt_violation(args: &[&str]) -> Option<WorkspaceScopeViolation> {
    if args.contains(&"--check") {
        return None;
    }
    let has_skip_children_true = args.windows(2).any(|window| {
        window[0] == "--config"
            && window[1]
                .split(',')
                .any(|setting| setting.trim() == "skip_children=true")
    }) || args.iter().any(|arg| {
        arg.strip_prefix("--config=").is_some_and(|settings| {
            settings
                .split(',')
                .any(|setting| setting.trim() == "skip_children=true")
        })
    });
    (!has_skip_children_true).then_some(RUSTFMT_BROAD)
}

/// Any mutating `cargo fmt` is a violation in a shared worktree. `-p`/
/// `--package` is still too broad, and `cargo fmt -- <file>` is not a reliable
/// file pathspec: Cargo still formats package targets and forwards arguments to
/// rustfmt. A leading `+toolchain` selector (`cargo +nightly fmt`) is skipped.
/// Read-only `--check` is safe.
fn cargo_violation(args: &[&str]) -> Option<WorkspaceScopeViolation> {
    let mut idx = 0;
    if args.get(idx).is_some_and(|tok| tok.starts_with('+')) {
        idx += 1;
    }
    if args.get(idx) != Some(&"fmt") {
        return None;
    }
    let fmt_args = &args[idx + 1..];
    (!fmt_args.contains(&"--check")).then_some(CARGO_FMT_GLOBAL)
}

/// Dispatch a `git` subcommand, skipping leading global options
/// (`git -C <dir> …`, `git -c k=v …`) to reach the real subcommand.
fn git_violation(args: &[&str]) -> Option<WorkspaceScopeViolation> {
    let mut idx = 0;
    while let Some(tok) = args.get(idx) {
        if tok.starts_with('-') {
            // `-C`/`-c` take a separate value; other globals are self-contained.
            idx += if matches!(*tok, "-C" | "-c") { 2 } else { 1 };
        } else {
            break;
        }
    }
    let subcommand = *args.get(idx)?;
    let rest = &args[idx + 1..];
    match subcommand {
        "add" => git_add_violation(rest),
        "commit" => git_commit_violation(rest),
        "reset" => rest.contains(&"--hard").then_some(GIT_RESET_HARD),
        "checkout" => rest.contains(&".").then_some(GIT_CHECKOUT_DOT),
        "restore" => rest.contains(&".").then_some(GIT_RESTORE_DOT),
        "clean" => git_clean_violation(rest),
        _ => None,
    }
}

/// `git add` that stages the whole tree (`-A`, `--all`, `.`, `-u`,
/// `--update`, `:/`). An explicit pathspec (`git add src/x.rs`) is safe.
fn git_add_violation(args: &[&str]) -> Option<WorkspaceScopeViolation> {
    args.iter()
        .any(|tok| matches!(*tok, "-A" | "--all" | "." | "-u" | "--update" | ":/"))
        .then_some(GIT_ADD_ALL)
}

/// `git commit -a`/`--all` (incl. bundled short flags like `-am`) auto-stages
/// every tracked modification. A bare `git commit`/`-m` (commits only the
/// already-staged set) is safe; `--amend` is not an auto-stage-all flag.
fn git_commit_violation(args: &[&str]) -> Option<WorkspaceScopeViolation> {
    args.iter()
        .any(|tok| *tok == "--all" || is_short_flag_with(tok, 'a'))
        .then_some(GIT_COMMIT_ALL)
}

/// `git clean` with a force flag (`-f`, `-fd`, `-xf`, `--force`) deletes
/// untracked files. A dry-run (`-n`/`--dry-run`, no force) is safe.
fn git_clean_violation(args: &[&str]) -> Option<WorkspaceScopeViolation> {
    args.iter()
        .any(|tok| *tok == "--force" || is_short_flag_with(tok, 'f'))
        .then_some(GIT_CLEAN_FORCE)
}

/// Whether `token` is a bundled *short* flag group (`-am`, `-fd`) that contains
/// the option letter `letter`. Long flags (`--all`) are handled by the caller's
/// explicit comparison, so a `--`-prefixed token never matches here.
fn is_short_flag_with(token: &str, letter: char) -> bool {
    token.starts_with('-') && !token.starts_with("--") && token.contains(letter)
}
