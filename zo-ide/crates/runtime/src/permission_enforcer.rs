//! Permission enforcement layer that gates tool execution based on the
//! active `PermissionPolicy`.

use crate::permissions::{PermissionMode, PermissionOutcome, PermissionPolicy};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome")]
pub enum EnforcementResult {
    /// Tool execution is allowed.
    Allowed,
    /// Tool execution was denied due to insufficient permissions.
    Denied {
        tool: String,
        active_mode: String,
        required_mode: String,
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct PermissionEnforcer {
    policy: PermissionPolicy,
}

impl PermissionEnforcer {
    #[must_use]
    pub fn new(policy: PermissionPolicy) -> Self {
        Self { policy }
    }

    /// Check whether a tool can be executed under the current permission policy.
    /// Auto-denies when prompting is required but no prompter is provided.
    #[must_use]
    pub fn check(&self, tool_name: &str, input: &str) -> EnforcementResult {
        // In Prompt mode, allow/ask escalation is deferred to the caller's
        // interactive prompt flow (the enforcer has no prompter). But an explicit
        // `deny` rule means "never — don't even ask", so it must still hard-deny
        // regardless of mode; otherwise a registry-layer gate on a path with no
        // conversation-layer prompter (sub-agent / headless) would silently allow
        // a tool the operator explicitly denied.
        if self.policy.active_mode() == PermissionMode::Prompt {
            return match self.policy.deny_reason(tool_name, input) {
                Some(reason) => EnforcementResult::Denied {
                    tool: tool_name.to_owned(),
                    active_mode: self.policy.active_mode().as_str().to_owned(),
                    required_mode: self
                        .policy
                        .required_mode_for_input(tool_name, input)
                        .as_str()
                        .to_owned(),
                    reason,
                },
                None => EnforcementResult::Allowed,
            };
        }

        let outcome = self.policy.authorize(tool_name, input, None);

        match outcome {
            PermissionOutcome::Allow => EnforcementResult::Allowed,
            PermissionOutcome::Deny { reason } => {
                let active_mode = self.policy.active_mode();
                let required_mode = self.policy.required_mode_for_input(tool_name, input);
                EnforcementResult::Denied {
                    tool: tool_name.to_owned(),
                    active_mode: active_mode.as_str().to_owned(),
                    required_mode: required_mode.as_str().to_owned(),
                    reason,
                }
            }
        }
    }

    #[must_use]
    pub fn is_allowed(&self, tool_name: &str, input: &str) -> bool {
        matches!(self.check(tool_name, input), EnforcementResult::Allowed)
    }

    #[must_use]
    pub fn active_mode(&self) -> PermissionMode {
        self.policy.active_mode()
    }

    #[must_use]
    pub fn with_active_mode(mut self, mode: PermissionMode) -> Self {
        self.policy.set_active_mode(mode);
        self
    }

    #[must_use]
    pub fn with_tool_requirement(
        mut self,
        tool_name: impl Into<String>,
        required_mode: PermissionMode,
    ) -> Self {
        self.policy
            .set_tool_requirements([(tool_name.into(), required_mode)]);
        self
    }

    /// Classify a file operation against workspace boundaries.
    #[must_use]
    pub fn check_file_write(&self, path: &str, workspace_root: &str) -> EnforcementResult {
        let mode = self.policy.active_mode();

        match mode {
            PermissionMode::ReadOnly => EnforcementResult::Denied {
                tool: "write_file".to_owned(),
                active_mode: mode.as_str().to_owned(),
                required_mode: PermissionMode::WorkspaceWrite.as_str().to_owned(),
                reason: format!("file writes are not allowed in '{}' mode", mode.as_str()),
            },
            PermissionMode::WorkspaceWrite => {
                if is_within_workspace(path, workspace_root) {
                    EnforcementResult::Allowed
                } else {
                    EnforcementResult::Denied {
                        tool: "write_file".to_owned(),
                        active_mode: mode.as_str().to_owned(),
                        required_mode: PermissionMode::DangerFullAccess.as_str().to_owned(),
                        reason: format!(
                            "path '{path}' is outside workspace root '{workspace_root}'"
                        ),
                    }
                }
            }
            // Allow and DangerFullAccess permit all writes
            PermissionMode::Allow | PermissionMode::DangerFullAccess => EnforcementResult::Allowed,
            PermissionMode::Prompt => EnforcementResult::Denied {
                tool: "write_file".to_owned(),
                active_mode: mode.as_str().to_owned(),
                required_mode: PermissionMode::WorkspaceWrite.as_str().to_owned(),
                reason: "file write requires confirmation in prompt mode".to_owned(),
            },
        }
    }

    /// Classify a bash command against the active mode and deny the ones
    /// that the shared command-intent pipeline marks as forbidden.
    ///
    /// Delegates to [`crate::bash_validation`] — the command classifier
    /// ported from upstream Claude Code's `BashTool` — so read-only and
    /// destructive gating live in exactly one place instead of being
    /// duplicated by a second heuristic here. A `Block` verdict (for
    /// example a write command or `sed -i` under read-only mode) denies;
    /// `Warn` and `Allow` both permit execution. Destructive *warnings*
    /// are non-blocking and surfaced on the command result rather than
    /// hard-denied at this layer, matching upstream behaviour.
    #[must_use]
    pub fn check_bash(&self, command: &str) -> EnforcementResult {
        let mode = self.policy.active_mode();
        let workspace = std::env::current_dir().unwrap_or_default();

        match crate::bash_validation::validate_command(command, mode, &workspace) {
            crate::bash_validation::ValidationResult::Block { reason } => {
                EnforcementResult::Denied {
                    tool: "bash".to_owned(),
                    active_mode: mode.as_str().to_owned(),
                    required_mode: PermissionMode::WorkspaceWrite.as_str().to_owned(),
                    reason,
                }
            }
            crate::bash_validation::ValidationResult::Warn { .. }
            | crate::bash_validation::ValidationResult::Allow => EnforcementResult::Allowed,
        }
    }
}

/// Workspace boundary check.
///
/// Resolves `.`/`..` segments *lexically* (no filesystem access, so it works
/// for paths that don't exist yet and for the synthetic roots used in tests)
/// before the prefix comparison. Without this, a traversal such as
/// `/workspace/../etc/passwd` string-starts-with `/workspace/` and would be
/// wrongly accepted. Mirrors the filesystem-touching check in
/// `tools::file_tools::resolve_for_boundary_check`, keeping the policy layer
/// consistent with the tool layer.
fn is_within_workspace(path: &str, workspace_root: &str) -> bool {
    if is_within_root(path, workspace_root) {
        return true;
    }
    // `--add-dir` roots count as workspace too — the same single list every
    // boundary check consults (see `file_ops::additional_workspace_roots`),
    // so an added directory cannot be writable here but unreadable elsewhere.
    crate::file_ops::additional_workspace_roots()
        .iter()
        .any(|root| is_within_root(path, &root.to_string_lossy()))
}

fn is_within_root(path: &str, workspace_root: &str) -> bool {
    let joined = if path.starts_with('/') {
        path.to_owned()
    } else {
        format!("{workspace_root}/{path}")
    };
    let normalized = lexically_normalize(&joined);
    let root = lexically_normalize(workspace_root);
    let root = root.trim_end_matches('/');

    let root_prefix = format!("{root}/");
    normalized.starts_with(&root_prefix) || normalized == root
}

/// Collapse `.` and `..` segments without touching the filesystem. A `..` that
/// would escape the path's anchor is clamped at the root rather than allowed to
/// climb above it, so the result can never reach outside the rooted prefix.
fn lexically_normalize(path: &str) -> String {
    use std::path::{Component, Path, PathBuf};

    let mut out = PathBuf::new();
    for component in Path::new(path).components() {
        match component {
            // `pop` returns false at the root — clamp instead of escaping.
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_enforcer(mode: PermissionMode) -> PermissionEnforcer {
        let policy = PermissionPolicy::new(mode);
        PermissionEnforcer::new(policy)
    }

    #[test]
    fn allow_mode_permits_everything() {
        let enforcer = make_enforcer(PermissionMode::Allow);
        assert!(enforcer.is_allowed("bash", ""));
        assert!(enforcer.is_allowed("write_file", ""));
        assert!(enforcer.is_allowed("edit_file", ""));
        assert_eq!(
            enforcer.check_file_write("/outside/path", "/workspace"),
            EnforcementResult::Allowed
        );
        // A non-catastrophic command is permitted even in Allow mode…
        assert_eq!(
            enforcer.check_bash("rm -rf build"),
            EnforcementResult::Allowed
        );
        // …but the catastrophic set (goal G8) is the one exception: hard-blocked
        // regardless of mode.
        assert!(matches!(
            enforcer.check_bash("rm -rf /"),
            EnforcementResult::Denied { .. }
        ));
    }

    #[test]
    fn read_only_denies_writes() {
        let policy = PermissionPolicy::new(PermissionMode::ReadOnly)
            .with_tool_requirement("read_file", PermissionMode::ReadOnly)
            .with_tool_requirement("grep_search", PermissionMode::ReadOnly)
            .with_tool_requirement("write_file", PermissionMode::WorkspaceWrite);

        let enforcer = PermissionEnforcer::new(policy);
        assert!(enforcer.is_allowed("read_file", ""));
        assert!(enforcer.is_allowed("grep_search", ""));

        // write_file requires WorkspaceWrite but we're in ReadOnly
        let result = enforcer.check("write_file", "");
        assert!(matches!(result, EnforcementResult::Denied { .. }));

        let result = enforcer.check_file_write("/workspace/file.rs", "/workspace");
        assert!(matches!(result, EnforcementResult::Denied { .. }));
    }

    #[test]
    fn read_only_allows_read_commands() {
        let enforcer = make_enforcer(PermissionMode::ReadOnly);
        assert_eq!(
            enforcer.check_bash("cat src/main.rs"),
            EnforcementResult::Allowed
        );
        assert_eq!(
            enforcer.check_bash("grep -r 'pattern' ."),
            EnforcementResult::Allowed
        );
        assert_eq!(enforcer.check_bash("ls -la"), EnforcementResult::Allowed);
    }

    #[test]
    fn read_only_denies_write_commands() {
        let enforcer = make_enforcer(PermissionMode::ReadOnly);
        let result = enforcer.check_bash("rm file.txt");
        assert!(matches!(result, EnforcementResult::Denied { .. }));
    }

    #[test]
    fn workspace_write_allows_within_workspace() {
        let enforcer = make_enforcer(PermissionMode::WorkspaceWrite);
        let result = enforcer.check_file_write("/workspace/src/main.rs", "/workspace");
        assert_eq!(result, EnforcementResult::Allowed);
    }

    #[test]
    fn workspace_write_denies_outside_workspace() {
        let enforcer = make_enforcer(PermissionMode::WorkspaceWrite);
        let result = enforcer.check_file_write("/etc/passwd", "/workspace");
        assert!(matches!(result, EnforcementResult::Denied { .. }));
    }

    #[test]
    fn prompt_mode_check_bash_defers_benign_command() {
        // The command-intent gate only *blocks* commands the active mode
        // forbids; a benign command under prompt mode is left to the
        // interactive permission layer (allowed here), while file writes
        // still deny outright.
        let enforcer = make_enforcer(PermissionMode::Prompt);
        assert_eq!(enforcer.check_bash("echo test"), EnforcementResult::Allowed);

        let result = enforcer.check_file_write("/workspace/file.rs", "/workspace");
        assert!(matches!(result, EnforcementResult::Denied { .. }));
    }

    #[test]
    fn prompt_mode_still_enforces_explicit_deny_rules() {
        use crate::config::RuntimePermissionRuleConfig;
        // A `deny` rule means "never — don't even prompt". Prompt mode defers
        // allow/ask escalation to the interactive prompter, but an explicitly
        // denied tool must STILL be hard-denied here — otherwise a registry-layer
        // gate on a prompterless path (sub-agent / headless) would silently allow
        // what the operator forbade. Signature: new(allow, deny, ask).
        let rules = RuntimePermissionRuleConfig::new(
            Vec::new(),
            vec!["bash(rm -rf:*)".to_string()],
            Vec::new(),
        );
        let policy = PermissionPolicy::new(PermissionMode::Prompt).with_permission_rules(&rules);
        let enforcer = PermissionEnforcer::new(policy);

        // The explicitly denied command is hard-denied even in Prompt mode.
        assert!(matches!(
            enforcer.check("bash", r#"{"command":"rm -rf /tmp/x"}"#),
            EnforcementResult::Denied { .. }
        ));
        // A command with no matching deny rule still defers to the prompter.
        assert!(enforcer.is_allowed("bash", r#"{"command":"git status"}"#));
        // An unrelated tool is likewise deferred (allowed here).
        assert!(enforcer.is_allowed("read_file", r#"{"file_path":"src/main.rs"}"#));
    }

    #[test]
    fn workspace_boundary_check() {
        assert!(is_within_workspace("/workspace/src/main.rs", "/workspace"));
        assert!(is_within_workspace("/workspace", "/workspace"));
        assert!(!is_within_workspace("/etc/passwd", "/workspace"));
        assert!(!is_within_workspace("/workspacex/hack", "/workspace"));
    }

    #[test]
    fn workspace_boundary_rejects_dotdot_traversal() {
        // A `..` segment must not let a path that string-starts-with the root
        // escape it — these all resolve outside `/workspace`.
        assert!(!is_within_workspace(
            "/workspace/../etc/passwd",
            "/workspace"
        ));
        assert!(!is_within_workspace(
            "/workspace/sub/../../etc",
            "/workspace"
        ));
        assert!(!is_within_workspace("../secret", "/workspace"));
        assert!(!is_within_workspace(
            "/workspace/../workspacex/hack",
            "/workspace"
        ));
        // Interior `..` that stays inside the root is still allowed.
        assert!(is_within_workspace(
            "/workspace/src/../lib/x.rs",
            "/workspace"
        ));
        assert!(is_within_workspace("src/../main.rs", "/workspace"));
        // A trailing-slash root must behave identically.
        assert!(!is_within_workspace("/workspace/../etc", "/workspace/"));
    }

    #[test]
    fn active_mode_returns_policy_mode() {
        // given
        let modes = [
            PermissionMode::ReadOnly,
            PermissionMode::WorkspaceWrite,
            PermissionMode::DangerFullAccess,
            PermissionMode::Prompt,
            PermissionMode::Allow,
        ];

        // when
        let active_modes: Vec<_> = modes
            .into_iter()
            .map(|mode| make_enforcer(mode).active_mode())
            .collect();

        // then
        assert_eq!(active_modes, modes);
    }

    #[test]
    fn danger_full_access_permits_file_writes_and_bash() {
        // given
        let enforcer = make_enforcer(PermissionMode::DangerFullAccess);

        // when
        let file_result = enforcer.check_file_write("/outside/workspace/file.txt", "/workspace");
        let bash_result = enforcer.check_bash("rm -rf /tmp/scratch");

        // then
        assert_eq!(file_result, EnforcementResult::Allowed);
        assert_eq!(bash_result, EnforcementResult::Allowed);
    }

    #[test]
    fn check_denied_payload_contains_tool_and_modes() {
        // given
        let policy = PermissionPolicy::new(PermissionMode::ReadOnly)
            .with_tool_requirement("write_file", PermissionMode::WorkspaceWrite);
        let enforcer = PermissionEnforcer::new(policy);

        // when
        let result = enforcer.check("write_file", "{}");

        // then
        match result {
            EnforcementResult::Denied {
                tool,
                active_mode,
                required_mode,
                reason,
            } => {
                assert_eq!(tool, "write_file");
                assert_eq!(active_mode, "read-only");
                assert_eq!(required_mode, "workspace-write");
                assert!(reason.contains("requires workspace-write permission"));
            }
            other @ EnforcementResult::Allowed => panic!("expected denied result, got {other:?}"),
        }
    }

    #[test]
    fn workspace_write_relative_path_resolved() {
        // given
        let enforcer = make_enforcer(PermissionMode::WorkspaceWrite);

        // when
        let result = enforcer.check_file_write("src/main.rs", "/workspace");

        // then
        assert_eq!(result, EnforcementResult::Allowed);
    }

    #[test]
    fn workspace_root_with_trailing_slash() {
        // given
        let enforcer = make_enforcer(PermissionMode::WorkspaceWrite);

        // when
        let result = enforcer.check_file_write("/workspace/src/main.rs", "/workspace/");

        // then
        assert_eq!(result, EnforcementResult::Allowed);
    }

    #[test]
    fn workspace_root_equality() {
        // given
        let root = "/workspace/";

        // when
        let equal_to_root = is_within_workspace("/workspace", root);

        // then
        assert!(equal_to_root);
    }

    #[test]
    fn read_only_blocks_redirects() {
        // Write redirections (both space-padded and bare) escape read-only
        // mode and are blocked by the shared validation pipeline.
        let enforcer = make_enforcer(PermissionMode::ReadOnly);
        assert!(matches!(
            enforcer.check_bash("cat Cargo.toml > out.txt"),
            EnforcementResult::Denied { .. }
        ));
        assert!(matches!(
            enforcer.check_bash("echo test >> out.txt"),
            EnforcementResult::Denied { .. }
        ));
    }

    #[test]
    fn read_only_blocks_interpreter_and_sed_in_place() {
        let enforcer = make_enforcer(PermissionMode::ReadOnly);
        assert!(matches!(
            enforcer.check_bash("python -i script.py"),
            EnforcementResult::Denied { .. }
        ));
        assert!(matches!(
            enforcer.check_bash("sed --in-place 's/a/b/' file.txt"),
            EnforcementResult::Denied { .. }
        ));
    }

    #[test]
    fn read_only_blocks_pipe_to_shell() {
        // Reported in code-review-2026-05: `cat foo | sh` bypassed the
        // first-token allow-list.
        let enforcer = make_enforcer(PermissionMode::ReadOnly);
        assert!(matches!(
            enforcer.check_bash("cat /etc/passwd | sh"),
            EnforcementResult::Denied { .. }
        ));
    }

    #[test]
    fn read_only_blocks_find_delete_and_xargs_rm() {
        let enforcer = make_enforcer(PermissionMode::ReadOnly);
        assert!(matches!(
            enforcer.check_bash("find . -delete"),
            EnforcementResult::Denied { .. }
        ));
        assert!(matches!(
            enforcer.check_bash("find . -name '*.tmp' | xargs rm"),
            EnforcementResult::Denied { .. }
        ));
    }

    #[test]
    fn read_only_blocks_git_push_and_gh_merge() {
        let enforcer = make_enforcer(PermissionMode::ReadOnly);
        assert!(matches!(
            enforcer.check_bash("git push origin main"),
            EnforcementResult::Denied { .. }
        ));
        assert!(matches!(
            enforcer.check_bash("gh pr merge 42"),
            EnforcementResult::Denied { .. }
        ));
    }

    #[test]
    fn read_only_blocks_interpreter_eval_flags() {
        let enforcer = make_enforcer(PermissionMode::ReadOnly);
        assert!(matches!(
            enforcer.check_bash("python -c \"import os; os.unlink('/tmp/x')\""),
            EnforcementResult::Denied { .. }
        ));
        assert!(matches!(
            enforcer.check_bash("node -e \"require('fs').unlinkSync('/tmp/x')\""),
            EnforcementResult::Denied { .. }
        ));
    }

    #[test]
    fn read_only_blocks_redirect_without_spaces() {
        // Original heuristic only caught ` > ` and ` >> ` (space-padded).
        let enforcer = make_enforcer(PermissionMode::ReadOnly);
        assert!(matches!(
            enforcer.check_bash("echo x>file"),
            EnforcementResult::Denied { .. }
        ));
    }

    #[test]
    fn read_only_blocks_command_substitution() {
        let enforcer = make_enforcer(PermissionMode::ReadOnly);
        assert!(matches!(
            enforcer.check_bash("echo $(rm /tmp/leak)"),
            EnforcementResult::Denied { .. }
        ));
        assert!(matches!(
            enforcer.check_bash("echo `rm /tmp/leak`"),
            EnforcementResult::Denied { .. }
        ));
    }

    #[test]
    fn read_only_still_allows_safe_git_subcommands() {
        let enforcer = make_enforcer(PermissionMode::ReadOnly);
        for cmd in [
            "git log --oneline",
            "git status",
            "git diff",
            "git show HEAD",
        ] {
            assert_eq!(
                enforcer.check_bash(cmd),
                EnforcementResult::Allowed,
                "expected allowed: {cmd}"
            );
        }
    }

    #[test]
    fn read_only_allows_quoted_pipes_in_args() {
        // `grep -F '|' file` ships a literal pipe inside single quotes —
        // it is an argument, not a shell pipeline.
        let enforcer = make_enforcer(PermissionMode::ReadOnly);
        assert_eq!(
            enforcer.check_bash("grep -F '|' Cargo.toml"),
            EnforcementResult::Allowed
        );
    }

    #[test]
    fn read_only_check_file_write_denied_payload() {
        // given
        let enforcer = make_enforcer(PermissionMode::ReadOnly);

        // when
        let result = enforcer.check_file_write("/workspace/file.txt", "/workspace");

        // then
        match result {
            EnforcementResult::Denied {
                tool,
                active_mode,
                required_mode,
                reason,
            } => {
                assert_eq!(tool, "write_file");
                assert_eq!(active_mode, "read-only");
                assert_eq!(required_mode, "workspace-write");
                assert!(reason.contains("file writes are not allowed"));
            }
            other @ EnforcementResult::Allowed => panic!("expected denied result, got {other:?}"),
        }
    }
}
