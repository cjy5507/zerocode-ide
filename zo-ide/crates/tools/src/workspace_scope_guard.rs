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

use runtime::BashCommandOutput;
// The table and its detection are the core crate's (t-10916), so the command
// guard's baseline reads the forms this guard refuses; what stays here is
// when it refuses them and what a refusal hands back.
pub(crate) use zerocode_core::shell_rule::scope::first_workspace_scope_violation;
use zerocode_core::shell_rule::scope::WorkspaceScopeViolation;
#[cfg(test)]
use zerocode_core::shell_rule::scope::{
    CARGO_FMT_GLOBAL, GIT_ADD_ALL, GIT_CHECKOUT_DOT, GIT_CLEAN_FORCE, GIT_COMMIT_ALL, GIT_RESET_HARD,
    GIT_RESTORE_DOT, RUSTFMT_BROAD,
};

/// Environment variable that turns the guard on. Default-off: absent, empty, or
/// any value other than the truthy set below leaves every command unguarded.
const WORKSPACE_GUARD_ENV: &str = "ZO_WORKSPACE_GUARD";

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
