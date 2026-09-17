//! Shared-working-tree command guard (track 4-1).
//!
//! When several agents share one working tree, a *global* mutating command
//! issued by one of them silently tangles the others' uncommitted work:
//! `cargo fmt` reformats files another agent is mid-edit on, `git add -A` /
//! `git commit -a` stages a sibling's changes, and `git reset --hard` /
//! `git checkout .` / `git clean -f` destroy uncommitted work outright. These
//! all have a safe alternative (a non-mutating `--check`, or direct
//! `rustfmt --config skip_children=true <files>`) that avoids unrelated files.
//!
//! This guard detects those whole-tree forms and returns a synthetic
//! [`BashCommandOutput`] that blocks the command up front (mirroring
//! [`crate::preflight::workspace_test_branch_preflight`]), naming the scoped
//! command to use instead. Detection is a pure function of the command string
//! ([`first_workspace_scope_violation`]) so it is fully unit-testable.
//!
//! It is **opt-in** (`ZO_WORKSPACE_GUARD=1`) and only consulted on the
//! shared process tree (a worktree-isolated agent, whose `cwd` is pinned, has
//! its own tree, so its global commands are already scoped — see
//! [`crate::bash_tools::run_bash`]). Default-off keeps the common solo workflow
//! — where broad mutating commands are entirely legitimate — unchanged.

use runtime::bash_validation::split_command_segments;
use runtime::BashCommandOutput;

/// Environment variable that turns the guard on. Default-off: absent, empty, or
/// any value other than the truthy set below leaves every command unguarded.
const WORKSPACE_GUARD_ENV: &str = "ZO_WORKSPACE_GUARD";

/// One whole-tree command form the guard refuses, with the reason and the
/// scoped alternative. All `&'static str` — the taxonomy is fixed, so a match
/// returns one of the consts below rather than allocating per call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WorkspaceScopeViolation {
    /// The offending command family, e.g. `"cargo fmt"` or `"git add -A"`.
    pub kind: &'static str,
    /// Why running it in a shared tree is unsafe.
    pub risk: &'static str,
    /// The scoped command to use instead.
    pub suggestion: &'static str,
}

const CARGO_FMT_GLOBAL: WorkspaceScopeViolation = WorkspaceScopeViolation {
    kind: "cargo fmt (broad reformat)",
    risk: "reformats files outside this agent's explicit changeset in the shared working tree, clobbering edits another agent has in flight",
    suggestion: "use `cargo fmt --check` to only verify, or format explicit files with `rustfmt --config skip_children=true <files>`",
};

const RUSTFMT_BROAD: WorkspaceScopeViolation = WorkspaceScopeViolation {
    kind: "rustfmt (recursive reformat)",
    risk: "a rustfmt invocation on a module root can recursively reformat child modules outside this agent's explicit changeset",
    suggestion: "use `rustfmt --check` to verify, or add `--config skip_children=true` when formatting explicit files",
};

const GIT_ADD_ALL: WorkspaceScopeViolation = WorkspaceScopeViolation {
    kind: "git add -A / git add .",
    risk: "stages every change in the shared tree, including files another agent is editing",
    suggestion:
        "stage explicit paths — `git add <path>...` — for only the files this agent changed",
};

const GIT_COMMIT_ALL: WorkspaceScopeViolation = WorkspaceScopeViolation {
    kind: "git commit -a",
    risk: "auto-stages and commits every modified tracked file in the shared tree, not just this agent's work",
    suggestion: "stage explicit paths first, then `git commit` (without -a) to commit only what you staged",
};

const GIT_RESET_HARD: WorkspaceScopeViolation = WorkspaceScopeViolation {
    kind: "git reset --hard",
    risk: "discards all uncommitted changes in the shared tree, destroying work other agents have not committed",
    suggestion: "scope the discard — `git restore <path>` for specific files — or `git stash` to set work aside reversibly",
};

const GIT_CHECKOUT_DOT: WorkspaceScopeViolation = WorkspaceScopeViolation {
    kind: "git checkout .",
    risk: "overwrites every modified file in the shared tree with HEAD, destroying other agents' uncommitted work",
    suggestion: "scope it — `git checkout -- <path>` (or `git restore <path>`) for only this agent's files",
};

const GIT_RESTORE_DOT: WorkspaceScopeViolation = WorkspaceScopeViolation {
    kind: "git restore .",
    risk:
        "reverts every modified file in the shared tree, destroying other agents' uncommitted work",
    suggestion: "scope it — `git restore <path>` for only the files this agent changed",
};

const GIT_CLEAN_FORCE: WorkspaceScopeViolation = WorkspaceScopeViolation {
    kind: "git clean -f",
    risk: "deletes untracked files across the shared tree, including new files another agent just created",
    suggestion: "preview first with `git clean -n`, then remove explicit paths — `git clean -f -- <path>`",
};

/// Whether the guard is enabled. Default-off so non-multi-agent workflows are
/// unaffected.
///
/// Two doors, and the second is why this is not env-only. The guard shipped
/// reachable ONLY through `ZO_WORKSPACE_GUARD`, which appears in no `--help`
/// text and no settings schema — so it activated zero times, while 365 of 395
/// real fan-outs qualified for isolation and 106 whole-tree mutators
/// (`git add -A`, `git reset --hard`, `git clean -f`, …) ran against a shared
/// tree unguarded. `settings.env` could not fix that by hand either: it is
/// applied to CHILD processes, never `set_var` into zo's own, so an operator
/// could configure it correctly and still get nothing.
///
/// The env var stays the operator override and wins when set — including
/// `ZO_WORKSPACE_GUARD=0`, which must be able to turn the guard OFF for a
/// session whose settings enable it. Otherwise the merged settings decide, so
/// project and `--settings` overlays participate like every other key.
#[must_use]
pub(crate) fn workspace_guard_enabled() -> bool {
    if let Ok(value) = std::env::var(WORKSPACE_GUARD_ENV) {
        return truthy(value.trim());
    }
    workspace_guard_setting()
}

fn truthy(value: &str) -> bool {
    matches!(value, "1" | "true" | "on" | "yes")
}

/// `workspaceGuard.enabled` from merged settings (global, project, `--settings`).
/// A loader failure leaves the guard off — this is a command BLOCKER, so an
/// unreadable config must not start refusing the user's commands.
fn workspace_guard_setting() -> bool {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let Ok(config) = runtime::ConfigLoader::default_for(&cwd).load() else {
        return false;
    };
    let Ok(root) = serde_json::from_str::<serde_json::Value>(&config.as_json().render()) else {
        return false;
    };
    workspace_guard_from_root(&root)
}

/// The setting's shape, apart from where the JSON came from — so the key path
/// is testable without touching the process environment or the filesystem.
fn workspace_guard_from_root(root: &serde_json::Value) -> bool {
    root.get("workspaceGuard")
        .and_then(|guard| guard.get("enabled"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// Block a whole-tree mutating command with a scoped-alternative message, or
/// `None` when the command is already scoped (or unrelated). The caller gates
/// this on [`workspace_guard_enabled`] and the shared-tree condition.
#[must_use]
pub(crate) fn workspace_scope_guard(command: &str) -> Option<BashCommandOutput> {
    first_workspace_scope_violation(command).map(|violation| blocked_output(&violation))
}

/// The first whole-tree violation across the command's operator-separated
/// segments (so `foo && git add -A` is caught), or `None`. Pure — the unit
/// tests drive this directly without touching the environment.
#[must_use]
pub(crate) fn first_workspace_scope_violation(command: &str) -> Option<WorkspaceScopeViolation> {
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

/// Build the synthetic blocking output. Mirrors the preflight block contract:
/// empty stdout, the guidance on stderr, and a `preflight_blocked:` return-code
/// interpretation so any block-aware surface treats it uniformly.
fn blocked_output(violation: &WorkspaceScopeViolation) -> BashCommandOutput {
    let stderr = format!(
        "workspace-scope guard blocked `{}` in the shared working tree: {}. {}. \
         (guard is opt-in via {WORKSPACE_GUARD_ENV}; unset it or set {WORKSPACE_GUARD_ENV}=0 to disable.)",
        violation.kind, violation.risk, violation.suggestion
    );
    BashCommandOutput {
        stdout: String::new(),
        stderr,
        raw_output_path: None,
        interrupted: false,
        is_image: None,
        background_task_id: None,
        backgrounded_by_user: None,
        assistant_auto_backgrounded: None,
        dangerously_disable_sandbox: None,
        return_code_interpretation: Some("preflight_blocked:workspace_scope".to_string()),
        no_output_expected: Some(false),
        structured_content: None,
        persisted_output_path: None,
        persisted_output_size: None,
        sandbox_status: None,
        safety_warning: None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        first_workspace_scope_violation, workspace_scope_guard, CARGO_FMT_GLOBAL, GIT_ADD_ALL,
        GIT_CHECKOUT_DOT, GIT_CLEAN_FORCE, GIT_COMMIT_ALL, GIT_RESET_HARD, GIT_RESTORE_DOT,
        RUSTFMT_BROAD,
    };

    /// The whole-tree forms the guard must catch.
    #[test]
    fn blocks_global_mutating_commands() {
        let cases = [
            ("cargo fmt", CARGO_FMT_GLOBAL),
            ("cargo fmt --all", CARGO_FMT_GLOBAL),
            ("cargo fmt -p tools", CARGO_FMT_GLOBAL),
            ("cargo fmt --package runtime", CARGO_FMT_GLOBAL),
            ("cargo fmt -p runtime -p tools", CARGO_FMT_GLOBAL),
            ("cargo fmt --", CARGO_FMT_GLOBAL),
            ("cargo fmt -- crates/tools/src/lib.rs", CARGO_FMT_GLOBAL),
            (
                "cargo fmt -p tools -- crates/tools/src/lib.rs",
                CARGO_FMT_GLOBAL,
            ),
            ("cargo +nightly fmt", CARGO_FMT_GLOBAL),
            ("rustfmt crates/tools/src/lib.rs", RUSTFMT_BROAD),
            (
                "rustfmt --edition 2021 crates/runtime/src/prompt/sections.rs",
                RUSTFMT_BROAD,
            ),
            ("git add -A", GIT_ADD_ALL),
            ("git add .", GIT_ADD_ALL),
            ("git add --all", GIT_ADD_ALL),
            ("git add -u", GIT_ADD_ALL),
            ("git commit -a", GIT_COMMIT_ALL),
            ("git commit -am \"wip\"", GIT_COMMIT_ALL),
            ("git commit --all", GIT_COMMIT_ALL),
            ("git reset --hard", GIT_RESET_HARD),
            ("git reset --hard origin/main", GIT_RESET_HARD),
            ("git checkout .", GIT_CHECKOUT_DOT),
            ("git checkout -- .", GIT_CHECKOUT_DOT),
            ("git restore .", GIT_RESTORE_DOT),
            ("git clean -f", GIT_CLEAN_FORCE),
            ("git clean -fdx", GIT_CLEAN_FORCE),
        ];
        for (command, expected) in cases {
            assert_eq!(
                first_workspace_scope_violation(command),
                Some(expected),
                "`{command}` must be guarded"
            );
        }
    }

    /// Scoped / read-only forms must pass untouched — the guard must not punish
    /// the correct, surgical command.
    #[test]
    fn allows_scoped_and_readonly_commands() {
        let allowed = [
            "cargo fmt --check",
            "cargo fmt --all --check",
            "cargo fmt -p tools --check",
            "cargo fmt --package runtime --check",
            "rustfmt --check crates/tools/src/lib.rs",
            "rustfmt --edition 2021 --config skip_children=true crates/runtime/src/prompt/sections.rs",
            "rustfmt --config=skip_children=true crates/tools/src/lib.rs",
            "cargo build",
            "cargo clippy -p tools",
            "git add crates/tools/src/lib.rs",
            "git add src/",
            "git commit -m \"scoped\"",
            "git commit --amend -m \"x\"",
            "git reset HEAD~1",        // soft reset, no --hard
            "git reset crates/x.rs",   // unstage a path
            "git checkout -b feature", // new branch
            "git checkout main",       // switch branch
            "git checkout -- crates/tools/src/lib.rs",
            "git restore crates/tools/src/lib.rs",
            "git clean -n", // dry run
            "git status",
            "git diff",
            "ls -A", // -A here is `ls`, not `git add`
        ];
        for command in allowed {
            assert_eq!(
                first_workspace_scope_violation(command),
                None,
                "`{command}` is scoped/read-only and must be allowed"
            );
        }
    }

    /// A violation in any chained segment is caught (it is not enough to check
    /// only the first command).
    #[test]
    fn catches_violation_in_a_chained_segment() {
        assert_eq!(
            first_workspace_scope_violation("cargo build && git add -A && echo done"),
            Some(GIT_ADD_ALL),
        );
        // sudo / env prefixes are stripped before matching.
        assert_eq!(
            first_workspace_scope_violation("FOO=bar cargo fmt"),
            Some(CARGO_FMT_GLOBAL),
        );
        assert_eq!(
            first_workspace_scope_violation("sudo git clean -fd"),
            Some(GIT_CLEAN_FORCE),
        );
    }

    /// The blocking output carries the scoped suggestion and the
    /// `preflight_blocked:` marker, and never produces stdout.
    #[test]
    fn blocked_output_is_actionable_and_marked() {
        let output = workspace_scope_guard("git add -A").expect("must block");
        assert!(output.stdout.is_empty());
        assert!(output.stderr.contains("git add <path>"));
        assert!(output.stderr.contains("ZO_WORKSPACE_GUARD"));
        assert_eq!(
            output.return_code_interpretation.as_deref(),
            Some("preflight_blocked:workspace_scope"),
        );
        assert!(workspace_scope_guard("git status").is_none());
    }

    /// The guard's second door must actually open. `workspaceGuard` is not a
    /// key the config parser models, so this pins that the merged settings JSON
    /// carries an unmodelled top-level key through to the reader — the exact
    /// property that decides whether this setting is reachable at all, and the
    /// kind of link whose absence made the guard env-only in the first place.
    #[test]
    fn merged_settings_carry_the_workspace_guard_key_through() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = dir.path().join(".zo");
        std::fs::create_dir_all(&project).expect("mkdir .zo");
        std::fs::write(
            project.join("settings.json"),
            r#"{"workspaceGuard":{"enabled":true}}"#,
        )
        .expect("write settings");
        let config = runtime::ConfigLoader::default_for(dir.path())
            .load()
            .expect("load");
        let root: serde_json::Value =
            serde_json::from_str(&config.as_json().render()).expect("render");
        assert!(
            super::workspace_guard_from_root(&root),
            "an unmodelled top-level settings key must survive the merge, else the \
             setting is a door that does not open: {root}"
        );
    }

    #[test]
    fn the_guard_setting_defaults_off_and_reads_only_its_own_key() {
        use serde_json::json;
        assert!(!super::workspace_guard_from_root(&json!({})));
        assert!(!super::workspace_guard_from_root(&json!({"workspaceGuard": {}})));
        assert!(!super::workspace_guard_from_root(
            &json!({"workspaceGuard": {"enabled": false}})
        ));
        assert!(super::workspace_guard_from_root(
            &json!({"workspaceGuard": {"enabled": true}})
        ));
        // The env override's truth set is the documented one.
        assert!(super::truthy("1") && super::truthy("true") && super::truthy("on"));
        assert!(!super::truthy("0") && !super::truthy("") && !super::truthy("maybe"));
    }
}
