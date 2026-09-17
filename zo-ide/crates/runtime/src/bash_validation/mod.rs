//! Bash command validation submodules.
//!
//! Ports the upstream `BashTool` validation pipeline:
//! - `readOnlyValidation` — block write-like commands in read-only mode
//! - `destructiveCommandWarning` — flag dangerous destructive commands
//! - `modeValidation` — enforce permission mode constraints on commands
//! - `sedValidation` — validate sed expressions before execution
//! - `pathValidation` — detect suspicious path patterns
//! - `commandSemantics` — classify command intent

use std::path::{Component, Path, PathBuf};

use crate::permissions::PermissionMode;

mod classify;
pub mod inspection;
mod parse;

pub use classify::classify_command;
pub use parse::split_command_segments;
use parse::{
    extract_first_command, extract_sudo_inner, strip_command_wrappers, strip_single_quoted,
};

/// Result of validating a bash command before execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationResult {
    /// Command is safe to execute.
    Allow,
    /// Command should be blocked with the given reason.
    Block { reason: String },
    /// Command requires user confirmation with the given warning.
    Warn { message: String },
}

impl ValidationResult {
    /// The advisory message for a [`ValidationResult::Warn`], if any.
    /// `Allow` and `Block` carry no non-blocking warning, so they return
    /// `None`.
    #[must_use]
    pub fn warning_message(&self) -> Option<&str> {
        match self {
            ValidationResult::Warn { message } => Some(message),
            ValidationResult::Allow | ValidationResult::Block { .. } => None,
        }
    }
}

/// Semantic classification of a bash command's intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandIntent {
    /// Read-only operations: ls, cat, grep, find, etc.
    ReadOnly,
    /// File system writes: cp, mv, mkdir, touch, tee, etc.
    Write,
    /// Destructive operations: rm, shred, truncate, etc.
    Destructive,
    /// Network operations: curl, wget, ssh, etc.
    Network,
    /// Process management: kill, pkill, etc.
    ProcessManagement,
    /// Package management: apt, brew, pip, npm, etc.
    PackageManagement,
    /// System administration: sudo, chmod, chown, mount, etc.
    SystemAdmin,
    /// Unknown or unclassifiable command.
    Unknown,
}

// ---------------------------------------------------------------------------
// readOnlyValidation
// ---------------------------------------------------------------------------

/// Commands that perform write operations and should be blocked in read-only mode.
const WRITE_COMMANDS: &[&str] = &[
    "cp", "mv", "rm", "mkdir", "rmdir", "touch", "chmod", "chown", "chgrp", "ln", "install", "tee",
    "truncate", "shred", "mkfifo", "mknod", "dd",
];

/// Commands that modify system state and should be blocked in read-only mode.
const STATE_MODIFYING_COMMANDS: &[&str] = &[
    "apt",
    "apt-get",
    "yum",
    "dnf",
    "pacman",
    "brew",
    "pip",
    "pip3",
    "npm",
    "yarn",
    "pnpm",
    "bun",
    "cargo",
    "gem",
    "go",
    "rustup",
    "docker",
    "systemctl",
    "service",
    "mount",
    "umount",
    "kill",
    "pkill",
    "killall",
    "reboot",
    "shutdown",
    "halt",
    "poweroff",
    "useradd",
    "userdel",
    "usermod",
    "groupadd",
    "groupdel",
    "crontab",
    "at",
];

/// Shell redirection operators that indicate writes.
const WRITE_REDIRECTIONS: &[&str] = &[">", ">>", ">&"];

/// Upper bound on the command length the static classifier will attempt to
/// prove read-only. Anything longer is never downgraded and always prompts —
/// padding a command past this point is the cheapest way to blunt a
/// substring-based scan, so it fails closed.
const MAX_ANALYZED_COMMAND_LEN: usize = 10_000;

/// Shells that can execute arbitrary commands. Running one in read-only
/// mode (`cat x | sh`, `sh -c '…'`) escapes the per-command classifier,
/// so a safe-looking first token still leads to arbitrary execution.
const SHELL_COMMANDS: &[&str] = &[
    "sh", "bash", "zsh", "dash", "ksh", "fish", "csh", "tcsh", "ash",
];

/// Interpreters that execute arbitrary code through an inline-eval flag.
const EVAL_INTERPRETERS: &[&str] = &["python", "python3", "node", "ruby", "perl", "php"];

/// Awk implementations execute a program supplied in their arguments and can
/// escape read-only mode through `system()`, output redirection, or program
/// files. Proving an arbitrary awk program safe would require a real parser,
/// so read-only mode fails closed for the whole family.
const AWK_COMMANDS: &[&str] = &["awk", "gawk", "mawk", "nawk"];

/// `find` actions that mutate files or execute another command. Match these as
/// exact shell tokens so safe predicates such as `-printf` are not confused
/// with file-writing actions such as `-fprintf`.
const FIND_MUTATING_PRIMARIES: &[&str] = &[
    "-delete", "-exec", "-execdir", "-ok", "-okdir", "-fls", "-fprint", "-fprint0",
    "-fprintf",
];

fn find_uses_mutating_primary(command: &str) -> bool {
    command.split_whitespace().any(|token| {
        let token = token.trim_matches(['\'', '"']);
        FIND_MUTATING_PRIMARIES.contains(&token)
    })
}

/// Inline-eval flags that turn an interpreter into an arbitrary-code runner.
const EVAL_FLAGS: &[&str] = &["-c", "-e", "-i", "-m", "--eval", "--command"];

/// `gh` subcommands that can mutate remote state (matches the historical
/// read-only allow-list). `gh api` is intentionally excluded — a bare API
/// read is permissible — and is gated by the usual escape patterns instead.
const GH_MUTATION_SUBCOMMANDS: &[&str] = &[
    "pr", "issue", "release", "repo", "workflow", "secret", "auth",
];

/// Detect arbitrary-execution escapes that the write/state/redirect checks
/// in [`validate_read_only`] would otherwise miss: a safe-looking first
/// token can still run arbitrary code via a shell, an interpreter eval
/// flag, command substitution, or a `find`/`xargs`/`gh` action. Operates on
/// a single already-split command segment and returns a block reason when
/// the segment is unsafe under read-only mode.
fn read_only_escape_reason(segment: &str) -> Option<String> {
    let unquoted = strip_single_quoted(segment);

    // Command substitution / backtick subshell — the inner command is
    // opaque to static classification, so it cannot be proven read-only.
    if unquoted.contains("$(") || unquoted.contains('`') {
        return Some(
            "Command substitution is not allowed in read-only mode (the inner command cannot be verified)"
                .to_string(),
        );
    }

    let first = extract_first_command(segment);
    let first_base = first.rsplit('/').next().unwrap_or(first.as_str());

    // A bare shell can run anything.
    if SHELL_COMMANDS.contains(&first_base) {
        return Some(format!(
            "Running a shell ('{first_base}') is not allowed in read-only mode"
        ));
    }

    // Awk programs can execute commands and write files without a shell-level
    // redirection. Static substring checks cannot prove arbitrary programs safe.
    if AWK_COMMANDS.contains(&first_base) {
        return Some(format!(
            "Awk interpreter '{first_base}' is not allowed in read-only mode"
        ));
    }

    // Interpreters with an inline-eval flag execute arbitrary code.
    if EVAL_INTERPRETERS.contains(&first_base) {
        let has_eval_flag = segment
            .split_whitespace()
            .any(|token| EVAL_FLAGS.contains(&token));
        if has_eval_flag {
            return Some(format!(
                "Interpreter '{first_base}' with an inline-eval flag is not allowed in read-only mode"
            ));
        }
    }

    // `find` can mutate files or execute arbitrary commands through actions
    // that do not use shell redirection.
    if first_base == "find" && find_uses_mutating_primary(segment) {
        return Some("`find` with a mutating action is not allowed in read-only mode".to_string());
    }

    // `xargs <write-command>` runs a mutating command per input line.
    if first_base == "xargs" {
        let sub = segment
            .strip_prefix("xargs")
            .unwrap_or(segment)
            .split_whitespace()
            .find(|token| !token.starts_with('-'))
            .unwrap_or("");
        if WRITE_COMMANDS.contains(&sub) || matches!(sub, "kill" | "rmdir" | "chmod" | "chown") {
            return Some(format!(
                "`xargs {sub}` runs a mutating command and is not allowed in read-only mode"
            ));
        }
    }

    // `gh` mutation subcommands change remote state.
    if first_base == "gh" {
        let sub = segment
            .strip_prefix("gh")
            .unwrap_or(segment)
            .split_whitespace()
            .next()
            .unwrap_or("");
        if GH_MUTATION_SUBCOMMANDS.contains(&sub) {
            return Some(format!(
                "`gh {sub}` may modify remote state and is not allowed in read-only mode"
            ));
        }
    }

    None
}

/// Validate that a command is allowed under read-only mode.
///
/// Corresponds to upstream `tools/BashTool/readOnlyValidation.ts`.
#[must_use]
pub fn validate_read_only(command: &str, mode: PermissionMode) -> ValidationResult {
    if mode != PermissionMode::ReadOnly {
        return ValidationResult::Allow;
    }

    // Arbitrary-execution escapes (shell, interpreter eval, command
    // substitution, find -delete/-exec, xargs write, gh mutation) that a
    // safe-looking first token would otherwise mask.
    if let Some(reason) = read_only_escape_reason(command) {
        return ValidationResult::Block { reason };
    }

    let first_command = extract_first_command(command);

    // Check for write commands.
    for &write_cmd in WRITE_COMMANDS {
        if first_command == write_cmd {
            return ValidationResult::Block {
                reason: format!(
                    "Command '{write_cmd}' modifies the filesystem and is not allowed in read-only mode"
                ),
            };
        }
    }

    // Check for state-modifying commands.
    for &state_cmd in STATE_MODIFYING_COMMANDS {
        if first_command == state_cmd {
            return ValidationResult::Block {
                reason: format!(
                    "Command '{state_cmd}' modifies system state and is not allowed in read-only mode"
                ),
            };
        }
    }

    // Check for sudo wrapping write commands.
    if first_command == "sudo" {
        let inner = extract_sudo_inner(command);
        if !inner.is_empty() {
            let inner_result = validate_read_only(inner, mode);
            if inner_result != ValidationResult::Allow {
                return inner_result;
            }
        }
    }

    // Check for write redirections.
    for &redir in WRITE_REDIRECTIONS {
        if command.contains(redir) {
            return ValidationResult::Block {
                reason: format!(
                    "Command contains write redirection '{redir}' which is not allowed in read-only mode"
                ),
            };
        }
    }

    // Check for git commands that modify state.
    if first_command == "git" {
        return validate_git_read_only(command);
    }

    ValidationResult::Allow
}

/// Minimal permission mode a bash command actually needs, derived from the
/// same classifier that gates execution: a command every segment of which
/// passes read-only validation (e.g. `git log`, `grep`) only needs
/// [`PermissionMode::ReadOnly`], so read-only sessions can run it without
/// escalating. Anything unprovable keeps the bash tool's static
/// `DangerFullAccess` requirement — writes are deliberately not downgraded
/// to `WorkspaceWrite`, because shell path containment cannot be proven
/// statically.
///
/// Mirrors [`validate_command`]'s per-segment structure (read-only + sed
/// checks on each `&&`/`;`/`|` segment): raw [`validate_read_only`] inspects
/// only the first command of the line, so calling it alone would prove
/// `git log && rm x` "read-only" and miss `sed -i` entirely.
#[must_use]
pub fn required_mode_for_command(command: &str) -> PermissionMode {
    // Very long commands are not worth proving safe: the static, substring-based
    // scanners degrade on pathological input and an attacker can pad a mutating
    // command past the point where a linear check stays reliable. Fail closed —
    // never downgrade, so an oversized command always prompts.
    if command.len() > MAX_ANALYZED_COMMAND_LEN {
        return PermissionMode::DangerFullAccess;
    }
    let provably_read_only = split_command_segments(command).into_iter().all(|segment| {
        let inner = strip_command_wrappers(segment);
        validate_read_only(inner, PermissionMode::ReadOnly) == ValidationResult::Allow
            && validate_sed(inner, PermissionMode::ReadOnly) == ValidationResult::Allow
    });
    if provably_read_only {
        PermissionMode::ReadOnly
    } else {
        PermissionMode::DangerFullAccess
    }
}

/// Git subcommands that are read-only safe.
const GIT_READ_ONLY_SUBCOMMANDS: &[&str] = &[
    "status",
    "log",
    "diff",
    "show",
    "branch",
    "tag",
    "stash",
    "remote",
    "fetch",
    "ls-files",
    "ls-tree",
    "cat-file",
    "rev-parse",
    "describe",
    "shortlog",
    "blame",
    "bisect",
    "reflog",
    "config",
];

fn validate_git_read_only(command: &str) -> ValidationResult {
    let parts: Vec<&str> = command.split_whitespace().collect();
    // Skip past "git" and any flags (e.g., "git -C /path")
    let subcommand = parts.iter().skip(1).find(|p| !p.starts_with('-'));

    match subcommand {
        Some(&sub) if GIT_READ_ONLY_SUBCOMMANDS.contains(&sub) => ValidationResult::Allow,
        Some(&sub) => ValidationResult::Block {
            reason: format!(
                "Git subcommand '{sub}' modifies repository state and is not allowed in read-only mode"
            ),
        },
        None => ValidationResult::Allow, // bare "git" is fine
    }
}

// ---------------------------------------------------------------------------
// destructiveCommandWarning
// ---------------------------------------------------------------------------

/// Patterns that indicate potentially destructive commands.
const DESTRUCTIVE_PATTERNS: &[(&str, &str)] = &[
    (
        "rm -rf /",
        "Recursive forced deletion at root — this will destroy the system",
    ),
    ("rm -rf ~", "Recursive forced deletion of home directory"),
    (
        "rm -rf *",
        "Recursive forced deletion of all files in current directory",
    ),
    ("rm -rf .", "Recursive forced deletion of current directory"),
    (
        "mkfs",
        "Filesystem creation will destroy existing data on the device",
    ),
    (
        "dd if=",
        "Direct disk write — can overwrite partitions or devices",
    ),
    ("> /dev/sd", "Writing to raw disk device"),
    (
        "chmod -R 777",
        "Recursively setting world-writable permissions",
    ),
    ("chmod -R 000", "Recursively removing all permissions"),
    (":(){ :|:& };:", "Fork bomb — will crash the system"),
];

/// Commands that are always destructive regardless of arguments.
const ALWAYS_DESTRUCTIVE_COMMANDS: &[&str] = &["shred", "wipefs"];

// ---------------------------------------------------------------------------
// Catastrophic (hard-block) detection — goal G8
// ---------------------------------------------------------------------------
//
// A small set of commands have no legitimate use inside an agent workflow and
// would destroy the host or wipe a disk irrecoverably. Unlike the advisory
// warnings above, these are *hard-blocked* in every permission mode —
// including `DangerFullAccess` — so a model-driven cleanup step can never run
// `rm -rf /` automatically. Matching is precise (whole-argument, not
// substring) so ordinary workspace paths such as `/tmp/build` are never
// caught.

/// Top-level paths whose recursive deletion or permission-stripping breaks the
/// OS or wipes all user data. Compared against a whole argument after a
/// trailing `/` or `/*` glob is stripped, so `/tmp/build` and
/// `/home/alice/project` never match — only the roots themselves.
const CRITICAL_ROOT_PATHS: &[&str] = &[
    "/", "/bin", "/boot", "/dev", "/etc", "/home", "/lib", "/lib64", "/opt", "/proc", "/root",
    "/sbin", "/srv", "/sys", "/usr", "/var",
];

/// `/dev` entries that are safe to write to (pseudo-devices). A write to any
/// other `/dev/...` path targets a raw block device and destroys its contents.
const SAFE_DEV_NODES: &[&str] = &[
    "/dev/null",
    "/dev/zero",
    "/dev/stdin",
    "/dev/stdout",
    "/dev/stderr",
    "/dev/tty",
    "/dev/full",
    "/dev/random",
    "/dev/urandom",
];

const FORK_BOMB_REASON: &str =
    "fork bomb — refusing to run (would exhaust process resources and crash the host)";
const RM_CRITICAL_ROOT_REASON: &str = "recursive deletion of a critical system path — refusing to run (would destroy the OS or all user data)";
const CHMOD_CRITICAL_ROOT_REASON: &str =
    "recursive `chmod 000` of a critical system path — refusing to run (would lock the OS out)";
const DD_BLOCK_DEVICE_REASON: &str =
    "`dd` writing to a raw block device — refusing to run (would overwrite the disk)";
const DEVICE_WIPE_REASON: &str =
    "filesystem/device wipe on a raw block device — refusing to run (irrecoverable data loss)";
const BLOCK_DEVICE_WRITE_REASON: &str =
    "redirection to a raw block device — refusing to run (would corrupt the disk)";
const CHMOD_WORLD_WRITABLE_REASON: &str = "recursive world-writable `chmod` of a critical system path — refusing to run (would let any user tamper with the OS)";
const CREDENTIAL_EXFIL_REASON: &str = "reading a credential file and piping it to a network client — refusing to run (credential exfiltration)";
const RECURSIVE_DELETE_OUTSIDE_WS_REASON: &str = "recursive deletion of a system directory outside the workspace — refusing to run (would damage the host)";

/// Local credential files whose contents must never be exfiltrated. Matched as
/// substrings, but only ever blocked in combination with an external
/// transmission ([`transmits_externally`]) so a plain local read stays allowed.
const CREDENTIAL_PATHS: &[&str] = &[
    "/etc/passwd",
    "/etc/shadow",
    "/.ssh/",
    "/.aws/",
    "/.gnupg/",
    "id_rsa",
    "id_ed25519",
    ".pem",
];

/// System directory prefixes whose recursive deletion damages the host. Matched
/// as a strict subpath (the exact roots are handled by [`is_critical_root_arg`]).
/// `/var`, `/opt`, `/home`, `/root`, `/srv`, `/tmp`, and `/usr/local` are
/// deliberately absent: they host legitimate scratch/app/user subtrees.
const SYSTEM_DELETE_SUBTREES: &[&str] = &[
    "/etc/", "/usr/", "/boot/", "/sys/", "/proc/", "/dev/", "/sbin/", "/bin/", "/lib/", "/lib64/",
];

/// Whole-argument test for a critical root path. Strips a trailing glob and
/// slash (`/etc`, `/etc/`, and `/etc/*` all reduce to `/etc`; `/` and `/*` to
/// `/`) and treats whole-home references (`~`, `$HOME`, `${HOME}`) as equally
/// catastrophic.
fn is_critical_root_arg(arg: &str) -> bool {
    let trimmed = arg.trim_matches(|c| c == '"' || c == '\'');
    let normalized = trimmed.trim_end_matches('*').trim_end_matches('/');
    if matches!(normalized, "~" | "$HOME" | "${HOME}") {
        return true;
    }
    let normalized = if normalized.is_empty() {
        "/"
    } else {
        normalized
    };
    CRITICAL_ROOT_PATHS.contains(&normalized)
}

/// Whether a `/dev/...` path is a raw block device (not a safe pseudo-device
/// and not a pty/fd node).
fn is_block_device_path(path: &str) -> bool {
    let path = path.trim_matches(|c| c == '"' || c == '\'');
    if !path.starts_with("/dev/") {
        return false;
    }
    if path.starts_with("/dev/pts/") || path.starts_with("/dev/fd/") {
        return false;
    }
    !SAFE_DEV_NODES.contains(&path)
}

/// Whether any token references a raw block device (used for `mkfs`, `wipefs`,
/// `shred`). Handles `of=`/`if=`-style `key=path` tokens.
fn references_block_device(command: &str) -> bool {
    command.split_whitespace().any(|tok| {
        let path = tok.rsplit('=').next().unwrap_or(tok);
        is_block_device_path(path)
    })
}

/// Whether a tokenized command carries a recursive flag (`-r`, `-R`,
/// `--recursive`, or a bundled short flag like `-rf`).
fn has_recursive_flag(tokens: &[&str]) -> bool {
    tokens.iter().any(|t| {
        *t == "--recursive"
            || (t.starts_with('-') && !t.starts_with("--") && (t.contains('r') || t.contains('R')))
    })
}

/// Classic fork-bomb shape, compared after removing whitespace so spacing
/// variants (`:(){ :|:& };:`, `:(){:|:&};:`) all collapse to the same form.
fn is_fork_bomb(command: &str) -> bool {
    let compact: String = command.chars().filter(|c| !c.is_whitespace()).collect();
    compact.contains(":(){:|:&};:")
}

/// Whether a chmod mode token grants write to "other" (world-writable): a
/// numeric mode whose last octal digit has the write bit (`777`, `0666`, `757`)
/// or an explicit symbolic `o+w`/`a+w`. A non-world mode (`750`, `o-w`) is not
/// flagged.
#[must_use]
fn is_world_writable_mode(token: &str) -> bool {
    let token = token.trim_matches(|c| c == '"' || c == '\'');
    if token.len() <= 4 && token.chars().all(|c| c.is_ascii_digit()) {
        if let Some(other) = token.chars().last().and_then(|c| c.to_digit(8)) {
            return other & 0o2 != 0;
        }
    }
    let lower = token.to_ascii_lowercase();
    lower.contains("o+w") || lower.contains("a+w")
}

/// Whether the segment references a known local credential file. Loose on its
/// own — only meaningful paired with [`transmits_externally`].
#[must_use]
fn references_credential_path(command: &str) -> bool {
    CREDENTIAL_PATHS.iter().any(|path| command.contains(path))
}

/// Whether the segment sends data off the host: a bash `/dev/tcp`/`/dev/udp`
/// redirect, or a pipe into a network client (`| curl`, `| nc`, …). A bare
/// `curl https://x` (no pipe feeding it local data) is not flagged.
#[must_use]
fn transmits_externally(command: &str) -> bool {
    if command.contains("/dev/tcp/") || command.contains("/dev/udp/") {
        return true;
    }
    command.split('|').skip(1).any(|after| {
        let first = after.split_whitespace().next().unwrap_or("");
        let base = first.rsplit('/').next().unwrap_or(first);
        matches!(
            base,
            "curl" | "wget" | "nc" | "ncat" | "netcat" | "telnet" | "socat"
        )
    })
}

/// Whether an argument is a strict subpath of a system directory whose recursive
/// deletion damages the host (`/etc/nginx`, `/usr/lib/x`). Excludes the
/// user-writable `/usr/local` subtree. Exact roots are handled by
/// [`is_critical_root_arg`]; scratch/user roots are intentionally not listed.
#[must_use]
fn targets_system_delete_subtree(arg: &str) -> bool {
    let normalized = arg
        .trim_matches(|c| c == '"' || c == '\'')
        .trim_end_matches('*')
        .trim_end_matches('/');
    if normalized == "/usr/local" || normalized.starts_with("/usr/local/") {
        return false;
    }
    SYSTEM_DELETE_SUBTREES
        .iter()
        .any(|prefix| normalized.starts_with(prefix))
}

/// Blank out single/double-quoted spans, replacing each with whitespace so byte
/// offsets and word boundaries are preserved. Used so a `>` that lives *inside a
/// string literal* (e.g. `grep "dd of=/dev/sda" notes.md`) is not mistaken for a
/// shell redirection operator.
fn mask_quoted_spans(command: &str) -> String {
    let mut out = String::with_capacity(command.len());
    let mut quote: Option<char> = None;
    for ch in command.chars() {
        match quote {
            Some(q) => {
                if ch == q {
                    quote = None;
                }
                out.push(' ');
            }
            None if ch == '\'' || ch == '"' => {
                quote = Some(ch);
                out.push(' ');
            }
            None => out.push(ch),
        }
    }
    out
}

/// A `>`/`>>` redirect whose target is a raw block device (`echo 1 > /dev/sda`).
/// Quoted spans are masked first, so a device path merely *mentioned inside a
/// string* (common in research/example text) is not a false positive — the
/// guard targets actual disk-destroying redirects, not text about them (WI-F).
fn redirects_to_block_device(command: &str) -> bool {
    mask_quoted_spans(command).split('>').skip(1).any(|after| {
        // A leading `&` is the fd-duplication marker of `>&file` / `&>file`
        // (redirect stdout+stderr), not part of the path — strip it so
        // `echo x >&/dev/sda` still resolves the device target. A bare fd like
        // `2>&1` reduces to `1`, which is not a device path.
        let after = after.trim_start().trim_start_matches('&').trim_start();
        after
            .split_whitespace()
            .next()
            .map(redirect_target_head)
            .is_some_and(is_block_device_path)
    })
}

/// The path part of a redirect target token.
///
/// A target ends at the next shell metacharacter, not merely at whitespace:
/// `2>/dev/null;` and `2>/dev/null|wc` are `/dev/null` followed by an operator.
/// Leaving the operator attached made every entry in [`SAFE_DEV_NODES`] miss the
/// allowlist by exact comparison and read as a raw disk, so the guard rejected
/// the single most common redirect in shell whenever no space preceded the
/// operator. Trimming here keeps real device targets intact: `>/dev/sda;` still
/// resolves to `/dev/sda`.
fn redirect_target_head(token: &str) -> &str {
    token
        .split([';', '|', '&', '(', ')', '<', '>'])
        .next()
        .unwrap_or(token)
}

/// Inspect one command segment for a catastrophic operation, unwrapping a
/// leading wrapper (`timeout`, `env`, …) and `sudo` first so
/// `sudo timeout 5 rm -rf /` is still caught. Returns a static block reason.
fn segment_catastrophe(segment: &str) -> Option<&'static str> {
    let inner = strip_command_wrappers(segment);

    let first = extract_first_command(inner);
    if first == "sudo" {
        let sudo_inner = extract_sudo_inner(inner);
        return if sudo_inner.is_empty() {
            None
        } else {
            segment_catastrophe(sudo_inner)
        };
    }

    // A raw block-device write via redirection is catastrophic regardless of
    // the leading command.
    if redirects_to_block_device(inner) {
        return Some(BLOCK_DEVICE_WRITE_REASON);
    }

    let base = first.rsplit('/').next().unwrap_or(first.as_str());
    let tokens: Vec<&str> = inner.split_whitespace().collect();

    match base {
        "rm" if has_recursive_flag(&tokens)
            && tokens.iter().skip(1).copied().any(is_critical_root_arg) =>
        {
            Some(RM_CRITICAL_ROOT_REASON)
        }
        "rm" if has_recursive_flag(&tokens)
            && tokens
                .iter()
                .skip(1)
                .copied()
                .any(targets_system_delete_subtree) =>
        {
            Some(RECURSIVE_DELETE_OUTSIDE_WS_REASON)
        }
        "chmod"
            if has_recursive_flag(&tokens)
                && tokens.iter().any(|t| matches!(*t, "000" | "0000"))
                && tokens.iter().skip(1).copied().any(is_critical_root_arg) =>
        {
            Some(CHMOD_CRITICAL_ROOT_REASON)
        }
        "chmod"
            if has_recursive_flag(&tokens)
                && tokens.iter().any(|t| is_world_writable_mode(t))
                && tokens.iter().skip(1).copied().any(is_critical_root_arg) =>
        {
            Some(CHMOD_WORLD_WRITABLE_REASON)
        }
        "dd" if tokens
            .iter()
            .any(|t| t.strip_prefix("of=").is_some_and(is_block_device_path)) =>
        {
            Some(DD_BLOCK_DEVICE_REASON)
        }
        "mkfs" | "wipefs" | "shred" if references_block_device(inner) => Some(DEVICE_WIPE_REASON),
        other if other.starts_with("mkfs.") && references_block_device(inner) => {
            Some(DEVICE_WIPE_REASON)
        }
        // `tee /dev/sda` writes a raw block device just like `> /dev/sda`, but as
        // a command argument rather than a redirection, so it slips past
        // `redirects_to_block_device`.
        "tee" if tokens.iter().skip(1).copied().any(is_block_device_path) => {
            Some(BLOCK_DEVICE_WRITE_REASON)
        }
        // `find /etc … -delete` (or `find /`) recursively deletes a system
        // subtree without ever invoking `rm`, so the `rm` arms above miss it.
        "find"
            if tokens.contains(&"-delete")
                && tokens
                    .iter()
                    .skip(1)
                    .copied()
                    .any(|t| is_critical_root_arg(t) || targets_system_delete_subtree(t)) =>
        {
            Some(RECURSIVE_DELETE_OUTSIDE_WS_REASON)
        }
        _ => None,
    }
}

/// Reason a command is catastrophic and must be hard-blocked, or `None`.
fn catastrophic_block_reason(command: &str) -> Option<&'static str> {
    if is_fork_bomb(command) {
        return Some(FORK_BOMB_REASON);
    }
    // Whole-command (pre-split): credential exfiltration spans a pipe, with the
    // secret read and the network sink in different segments
    // (`cat ~/.ssh/id_rsa | curl …`), so it cannot be seen one segment at a time.
    if references_credential_path(command) && transmits_externally(command) {
        return Some(CREDENTIAL_EXFIL_REASON);
    }
    // Whole-command block-device redirect: the `&` in an `>&`/`&>` fd-duplication
    // is a control operator to the segment splitter, so `echo x >&/dev/sda`
    // splits into `echo x >` + `/dev/sda` and the device target is hidden from
    // the per-segment scan below. Scanning the intact line catches it.
    if redirects_to_block_device(command) {
        return Some(BLOCK_DEVICE_WRITE_REASON);
    }
    split_command_segments(command)
        .into_iter()
        .find_map(segment_catastrophe)
}

/// Flag destructive commands: hard-block the catastrophic ones, warn on the
/// merely risky ones.
///
/// Corresponds to upstream `tools/BashTool/destructiveCommandWarning.ts`,
/// extended with the catastrophic hard-block (goal G8): commands that would
/// destroy the host or wipe a disk (`rm -rf /`, `mkfs`/`dd` on a device, a
/// fork bomb, …) return [`ValidationResult::Block`] in *every* permission
/// mode, while non-catastrophic destructive commands stay a non-blocking
/// [`ValidationResult::Warn`] as before.
#[must_use]
pub fn check_destructive(command: &str) -> ValidationResult {
    // Hard-block catastrophic commands first — no legitimate agent workflow
    // needs them, so they are refused regardless of permission mode.
    if let Some(reason) = catastrophic_block_reason(command) {
        return ValidationResult::Block {
            reason: reason.to_owned(),
        };
    }

    // Check known destructive patterns.
    for &(pattern, warning) in DESTRUCTIVE_PATTERNS {
        if command.contains(pattern) {
            return ValidationResult::Warn {
                message: format!("Destructive command detected: {warning}"),
            };
        }
    }

    // Check always-destructive commands.
    let first = extract_first_command(command);
    for &cmd in ALWAYS_DESTRUCTIVE_COMMANDS {
        if first == cmd {
            return ValidationResult::Warn {
                message: format!(
                    "Command '{cmd}' is inherently destructive and may cause data loss"
                ),
            };
        }
    }

    // Check for a genuine recursive-force `rm` (any segment of a chain).
    // The catastrophic targets above are already hard-blocked; flag any other
    // real `rm -rf` as an advisory warning.
    if command_has_recursive_force_rm(command) {
        return ValidationResult::Warn {
            message: "Recursive forced deletion detected — verify the target path is correct"
                .to_string(),
        };
    }

    ValidationResult::Allow
}

/// Whether any command segment is a true recursive-force `rm` invocation.
///
/// Replaces the old `command.contains("rm ") && contains("-r") && contains("-f")`
/// substring test, which both **false-fired** and **missed** real cases:
///
/// * False positive: `rm -f a kibana/application-logger-reqid-…` was flagged —
///   the bare substring `-r` matched inside the filename `-reqid`, and a
///   harmless non-recursive `rm -f` was wrongly reported as a recursive force
///   deletion. Likewise `grep -rn x && rm -f y` matched because `-r` came from
///   `grep` and `-f` from a *different* command.
/// * False negative: `rm -rf build` does NOT contain the substring `-f`
///   (its flags are the single token `-rf`), so a genuine recursive force was
///   never flagged.
///
/// This parses each operator-separated segment: confirm the (wrapper-stripped)
/// command is actually `rm`, then scan only its `-`-prefixed flag tokens for a
/// recursive flag (`-r`/`-R`/`--recursive`, including bundled `-rf`) AND a force
/// flag (`-f`/`--force`, including bundled `-rf`). A `--` end-of-options marker
/// stops flag scanning so a later path argument can never masquerade as a flag.
fn command_has_recursive_force_rm(command: &str) -> bool {
    split_command_segments(command)
        .into_iter()
        .any(segment_is_recursive_force_rm)
}

/// True when one already-split segment is `rm` (after wrapper/sudo stripping)
/// carrying both a recursive and a force flag among its option tokens.
fn segment_is_recursive_force_rm(segment: &str) -> bool {
    let inner = strip_command_wrappers(segment);
    // Unwrap a leading `sudo` so `sudo rm -rf x` is still parsed as `rm`.
    let inner = if extract_first_command(inner) == "sudo" {
        extract_sudo_inner(inner)
    } else {
        inner
    };
    if extract_first_command(inner) != "rm" {
        return false;
    }

    let mut recursive = false;
    let mut force = false;
    for tok in inner.split_whitespace().skip(1) {
        // `--` ends option parsing; everything after it is a path argument and
        // must never be treated as a flag (so `rm -- -rf` deletes a file named
        // `-rf` without being flagged as recursive force).
        if tok == "--" {
            break;
        }
        if let Some(long) = tok.strip_prefix("--") {
            match long {
                "recursive" => recursive = true,
                "force" => force = true,
                _ => {}
            }
        } else if let Some(short) = tok.strip_prefix('-') {
            // A bundled short-flag cluster (`-rf`, `-fr`, `-vrf`): each char is
            // an independent flag, so scan the whole cluster.
            if short.contains(['r', 'R']) {
                recursive = true;
            }
            if short.contains('f') {
                force = true;
            }
        }
        if recursive && force {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Worktree isolation containment
// ---------------------------------------------------------------------------
//
// A subagent confined to an isolated git worktree must not redirect git at the
// shared main checkout. Its bash child already runs pinned to the worktree cwd
// with `GIT_DIR`/`GIT_WORK_TREE` stripped from the inherited environment, but a
// command can still name another repository explicitly via `git -C <dir>`,
// `--git-dir`/`--work-tree`, or an *inline* `GIT_DIR=`/`GIT_WORK_TREE=`
// assignment. These are refused when the resolved target escapes the worktree.

/// Human-readable block reason naming the git redirect that escaped the
/// worktree.
fn git_escape_reason(target: &str) -> String {
    format!(
        "`{target}` points outside the isolated worktree — refusing to run (an isolated agent must not reach the shared checkout)"
    )
}

/// Reason a bash command escapes its isolated worktree `root` by redirecting
/// git at a directory outside it, or `None` when every git target stays inside.
/// Purely lexical: relative paths resolve against `root` and `..` is folded, so
/// no filesystem access or symlink resolution is involved.
#[must_use]
pub fn git_worktree_escape_reason(command: &str, root: &Path) -> Option<String> {
    split_command_segments(command)
        .into_iter()
        .find_map(|segment| segment_git_escape(segment, root))
}

fn segment_git_escape(segment: &str, root: &Path) -> Option<String> {
    // Inline `GIT_DIR=…` / `GIT_WORK_TREE=…` assignments sit before the command
    // and set the redirect for that one invocation, so scan the raw segment.
    for tok in segment.split_whitespace() {
        for var in ["GIT_DIR", "GIT_WORK_TREE"] {
            if let Some(value) = tok.strip_prefix(var).and_then(|rest| rest.strip_prefix('=')) {
                if !path_within_root(root, value) {
                    return Some(git_escape_reason(var));
                }
            }
        }
    }

    let stripped = strip_command_wrappers(segment);
    let leading = extract_first_command(stripped);
    let (first, inner) = if leading == "sudo" {
        let sudo_inner = extract_sudo_inner(stripped);
        (extract_first_command(sudo_inner), sudo_inner)
    } else {
        (leading, stripped)
    };
    if first.rsplit('/').next().unwrap_or(first.as_str()) != "git" {
        return None;
    }

    let tokens: Vec<&str> = inner.split_whitespace().collect();
    let mut idx = 1;
    while idx < tokens.len() {
        let token = tokens[idx];
        // `-C <dir>`: the directory git treats as the repository root.
        if token == "-C" {
            if tokens.get(idx + 1).is_some_and(|dir| !path_within_root(root, dir)) {
                return Some(git_escape_reason("git -C"));
            }
            idx += 2;
            continue;
        }
        // `--git-dir[=]<path>` / `--work-tree[=]<path>`, both spellings.
        for flag in ["--git-dir", "--work-tree"] {
            if let Some(rest) = token.strip_prefix(flag) {
                let value = if let Some(inline) = rest.strip_prefix('=') {
                    Some(inline)
                } else if rest.is_empty() {
                    tokens.get(idx + 1).copied()
                } else {
                    None
                };
                if value.is_some_and(|path| !path_within_root(root, path)) {
                    return Some(git_escape_reason(flag));
                }
            }
        }
        idx += 1;
    }
    None
}

/// Whether `candidate` (resolved against `root` when relative) stays within
/// `root` after lexically folding `.`/`..`. No filesystem access.
fn path_within_root(root: &Path, candidate: &str) -> bool {
    let candidate = candidate.trim_matches(|c| c == '"' || c == '\'');
    if candidate.is_empty() {
        return true;
    }
    let joined = if Path::new(candidate).is_absolute() {
        PathBuf::from(candidate)
    } else {
        root.join(candidate)
    };
    normalize_lexically(&joined).starts_with(normalize_lexically(root))
}

/// Fold `.`/`..` components without touching the filesystem. `..` pops only a
/// preceding normal segment, so it can ascend above the root prefix — which is
/// exactly what marks an escape.
fn normalize_lexically(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if result
                    .components()
                    .next_back()
                    .is_some_and(|c| matches!(c, Component::Normal(_)))
                {
                    result.pop();
                } else {
                    result.push("..");
                }
            }
            other => result.push(other.as_os_str()),
        }
    }
    result
}

// ---------------------------------------------------------------------------
// modeValidation
// ---------------------------------------------------------------------------

/// Validate that a command is consistent with the given permission mode.
///
/// Corresponds to upstream `tools/BashTool/modeValidation.ts`.
#[must_use]
pub fn validate_mode(command: &str, mode: PermissionMode) -> ValidationResult {
    match mode {
        PermissionMode::ReadOnly => validate_read_only(command, mode),
        PermissionMode::WorkspaceWrite => {
            // In workspace-write mode, check for system-level destructive
            // operations that go beyond workspace scope.
            if command_targets_outside_workspace(command) {
                return ValidationResult::Warn {
                    message:
                        "Command appears to target files outside the workspace — requires elevated permission"
                            .to_string(),
                };
            }
            ValidationResult::Allow
        }
        PermissionMode::DangerFullAccess | PermissionMode::Allow | PermissionMode::Prompt => {
            ValidationResult::Allow
        }
    }
}

/// Heuristic: does the command reference absolute paths outside typical workspace dirs?
fn command_targets_outside_workspace(command: &str) -> bool {
    let system_paths = [
        "/etc/", "/usr/", "/var/", "/boot/", "/sys/", "/proc/", "/dev/", "/sbin/", "/lib/", "/opt/",
    ];

    let first = extract_first_command(command);
    let is_write_cmd = WRITE_COMMANDS.contains(&first.as_str())
        || STATE_MODIFYING_COMMANDS.contains(&first.as_str());

    if !is_write_cmd {
        return false;
    }

    for sys_path in &system_paths {
        if command.contains(sys_path) {
            return true;
        }
    }

    false
}

// ---------------------------------------------------------------------------
// sedValidation
// ---------------------------------------------------------------------------

/// Validate sed expressions for safety.
///
/// Corresponds to upstream `tools/BashTool/sedValidation.ts`.
#[must_use]
pub fn validate_sed(command: &str, mode: PermissionMode) -> ValidationResult {
    let first = extract_first_command(command);
    if first != "sed" {
        return ValidationResult::Allow;
    }

    // In read-only mode, block sed in-place editing in both its short
    // (`-i`) and long (`--in-place`) forms.
    if mode == PermissionMode::ReadOnly
        && (command.contains(" -i") || command.contains("--in-place"))
    {
        return ValidationResult::Block {
            reason: "sed -i (in-place editing) is not allowed in read-only mode".to_string(),
        };
    }

    ValidationResult::Allow
}

// ---------------------------------------------------------------------------
// pathValidation
// ---------------------------------------------------------------------------

/// Validate that command paths don't include suspicious traversal patterns.
///
/// Corresponds to upstream `tools/BashTool/pathValidation.ts`.
#[must_use]
pub fn validate_paths(command: &str, workspace: &Path) -> ValidationResult {
    // Check for directory traversal attempts.
    if command.contains("../") {
        let workspace_str = workspace.to_string_lossy();
        // Allow traversal if it resolves within workspace (heuristic).
        if !command.contains(&*workspace_str) {
            return ValidationResult::Warn {
                message: "Command contains directory traversal pattern '../' — verify the target path resolves within the workspace".to_string(),
            };
        }
    }

    // Check for home directory references that could escape workspace.
    if command.contains("~/") || command.contains("$HOME") {
        return ValidationResult::Warn {
            message:
                "Command references home directory — verify it stays within the workspace scope"
                    .to_string(),
        };
    }

    ValidationResult::Allow
}

// ---------------------------------------------------------------------------
// Pipeline: run all validations
// ---------------------------------------------------------------------------

/// Run the full validation pipeline on a bash command.
///
/// Returns the first non-Allow result, or Allow if all validations pass.
///
/// Shell-aware: the per-command checks (mode/read-only and sed) run on
/// **each** segment of a compound command (`a && b`, `a | b`, `a; b`, …)
/// after wrapper-stripping, so a safe leading command can never smuggle
/// in a blocked trailing one. The substring-based destructive and path
/// checks run once over the whole line (they already match anywhere).
#[must_use]
pub fn validate_command(command: &str, mode: PermissionMode, workspace: &Path) -> ValidationResult {
    // 1 + 2. Per-segment mode and sed validation.
    for segment in split_command_segments(command) {
        let inner = strip_command_wrappers(segment);

        let result = validate_mode(inner, mode);
        if result != ValidationResult::Allow {
            return result;
        }

        let result = validate_sed(inner, mode);
        if result != ValidationResult::Allow {
            return result;
        }
    }

    // 3. Destructive command warnings (substring match over whole line).
    let result = check_destructive(command);
    if result != ValidationResult::Allow {
        return result;
    }

    // 4. Path validation (substring match over whole line).
    validate_paths(command, workspace)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
