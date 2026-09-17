//! Command intent classification — upstream `tools/BashTool/commandSemantics.ts`.
//!
//! Splits a command into segments, classifies each segment by its first word,
//! and returns the most dangerous [`CommandIntent`] so a safe prefix can't
//! mask a dangerous suffix. The shared command-class constants and the
//! validation rules live in the parent module.

use super::parse::{extract_first_command, split_command_segments, strip_command_wrappers};
use super::{
    find_uses_mutating_primary, CommandIntent, ALWAYS_DESTRUCTIVE_COMMANDS, AWK_COMMANDS,
    GIT_READ_ONLY_SUBCOMMANDS, WRITE_COMMANDS,
};

/// Commands that are read-only (no filesystem or state modification).
const SEMANTIC_READ_ONLY_COMMANDS: &[&str] = &[
    "ls",
    "cat",
    "head",
    "tail",
    "less",
    "more",
    "wc",
    "sort",
    "uniq",
    "grep",
    "egrep",
    "fgrep",
    "find",
    "which",
    "whereis",
    "whatis",
    "man",
    "info",
    "file",
    "stat",
    "du",
    "df",
    "free",
    "uptime",
    "uname",
    "hostname",
    "whoami",
    "id",
    "groups",
    "env",
    "printenv",
    "echo",
    "printf",
    "date",
    "cal",
    "bc",
    "expr",
    "test",
    "true",
    "false",
    "pwd",
    "tree",
    "diff",
    "cmp",
    "md5sum",
    "sha256sum",
    "sha1sum",
    "xxd",
    "od",
    "hexdump",
    "strings",
    "readlink",
    "realpath",
    "basename",
    "dirname",
    "seq",
    "yes",
    "tput",
    "column",
    "jq",
    "yq",
    "xargs",
    "tr",
    "cut",
    "paste",
    "sed",
];

/// Commands that perform network operations.
const NETWORK_COMMANDS: &[&str] = &[
    "curl",
    "wget",
    "ssh",
    "scp",
    "rsync",
    "ftp",
    "sftp",
    "nc",
    "ncat",
    "telnet",
    "ping",
    "traceroute",
    "dig",
    "nslookup",
    "host",
    "whois",
    "ifconfig",
    "ip",
    "netstat",
    "ss",
    "nmap",
];

/// Commands that manage processes.
const PROCESS_COMMANDS: &[&str] = &[
    "kill", "pkill", "killall", "ps", "top", "htop", "bg", "fg", "jobs", "nohup", "disown", "wait",
    "nice", "renice",
];

/// Commands that manage packages.
const PACKAGE_COMMANDS: &[&str] = &[
    "apt", "apt-get", "yum", "dnf", "pacman", "brew", "pip", "pip3", "npm", "yarn", "pnpm", "bun",
    "cargo", "gem", "go", "rustup", "snap", "flatpak",
];

/// Commands that require system administrator privileges.
const SYSTEM_ADMIN_COMMANDS: &[&str] = &[
    "sudo",
    "su",
    "chroot",
    "mount",
    "umount",
    "fdisk",
    "parted",
    "lsblk",
    "blkid",
    "systemctl",
    "service",
    "journalctl",
    "dmesg",
    "modprobe",
    "insmod",
    "rmmod",
    "iptables",
    "ufw",
    "firewall-cmd",
    "sysctl",
    "crontab",
    "at",
    "useradd",
    "userdel",
    "usermod",
    "groupadd",
    "groupdel",
    "passwd",
    "visudo",
];

/// Relative danger of a [`CommandIntent`]; higher wins when a compound
/// command mixes intents, so the most restrictive wins.
fn intent_rank(intent: CommandIntent) -> u8 {
    match intent {
        CommandIntent::ReadOnly => 0,
        CommandIntent::Unknown => 1,
        CommandIntent::ProcessManagement => 2,
        CommandIntent::Network => 3,
        CommandIntent::Write => 4,
        CommandIntent::PackageManagement => 5,
        CommandIntent::SystemAdmin => 6,
        CommandIntent::Destructive => 7,
    }
}

/// Classify the semantic intent of a bash command.
///
/// Corresponds to upstream `tools/BashTool/commandSemantics.ts`, extended
/// to be shell-aware: the command is split into segments on `&&`, `||`,
/// `;`, `|`, `&` and newlines, each segment is unwrapped (`timeout`,
/// `nice`, …) and classified, and the **most dangerous** intent wins.
/// This closes the "safe prefix masks dangerous suffix" gap where
/// `git status && git push` would otherwise classify as read-only.
#[must_use]
pub fn classify_command(command: &str) -> CommandIntent {
    split_command_segments(command)
        .into_iter()
        .map(|segment| {
            let inner = strip_command_wrappers(segment);
            classify_by_first_command(&extract_first_command(inner), inner)
        })
        .max_by_key(|&intent| intent_rank(intent))
        .unwrap_or(CommandIntent::Unknown)
}

fn classify_by_first_command(first: &str, command: &str) -> CommandIntent {
    let first_base = first.rsplit('/').next().unwrap_or(first);
    if AWK_COMMANDS.contains(&first_base) {
        return CommandIntent::Unknown;
    }

    if SEMANTIC_READ_ONLY_COMMANDS.contains(&first) {
        if first == "find" && find_uses_mutating_primary(command) {
            return CommandIntent::Write;
        }
        if first == "sed" && command.contains(" -i") {
            return CommandIntent::Write;
        }
        return CommandIntent::ReadOnly;
    }

    if ALWAYS_DESTRUCTIVE_COMMANDS.contains(&first) || first == "rm" {
        return CommandIntent::Destructive;
    }

    if WRITE_COMMANDS.contains(&first) {
        return CommandIntent::Write;
    }

    if NETWORK_COMMANDS.contains(&first) {
        return CommandIntent::Network;
    }

    if PROCESS_COMMANDS.contains(&first) {
        return CommandIntent::ProcessManagement;
    }

    if PACKAGE_COMMANDS.contains(&first) {
        return CommandIntent::PackageManagement;
    }

    if SYSTEM_ADMIN_COMMANDS.contains(&first) {
        return CommandIntent::SystemAdmin;
    }

    if first == "git" {
        return classify_git_command(command);
    }

    CommandIntent::Unknown
}

fn classify_git_command(command: &str) -> CommandIntent {
    let parts: Vec<&str> = command.split_whitespace().collect();
    let subcommand = parts.iter().skip(1).find(|p| !p.starts_with('-'));
    match subcommand {
        Some(&sub) if GIT_READ_ONLY_SUBCOMMANDS.contains(&sub) => CommandIntent::ReadOnly,
        _ => CommandIntent::Write,
    }
}
