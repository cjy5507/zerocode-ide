//! What a full disk still lets a session run.
//!
//! The disk floor refuses every shell run below [`super::HARD_MIN_DISK_BYTES`],
//! and its own message names what to reclaim — "reclaim Rust target/ build
//! dirs, temp scratch, or orphaned worktrees". But the refusal is at the
//! shared chokepoint every run funnels through, so it takes away the hands
//! that would do the reclaiming: a session told to free space cannot run
//! `df` to see where, cannot run `du` to find the big directory, and cannot
//! run `rm` on it. It is advised to act and forbidden to act, and it dies
//! there.
//!
//! Observed on this machine: an agent at 126 MB was refused `lsof +L1 ; df -h`
//! — a pure read — and its turn then ended on `No space left on device`.
//!
//! So below the floor a run is refused unless it can only look or only free.
//! Two rules decide that, and both are deliberately blunt, because the input
//! is an arbitrary shell string and a parser that is clever about it is a
//! parser that is wrong about it:
//!
//! 1. **It must be one simple command.** Any shell metacharacter that could
//!    chain, expand or redirect into something else — `; & | ` $ ( ) < > { }
//!    newline — disqualifies it. `rm -rf big && cargo build` frees nothing and
//!    then does the very thing the floor exists to stop.
//! 2. **Its program must be in [`RECLAIM`]**, and where the program alone is
//!    not enough, its first argument too. `cargo` can `clean`; `cargo` cannot
//!    `build`.
//!
//! Anything this pair is unsure about stays refused. The cost of that is a
//! session that has to be told again, in simpler words; the cost of the
//! opposite is the floor not holding.

/// A program a full disk still admits, and the first argument it needs when
/// the program alone does not settle it.
///
/// Read only, or frees space. Nothing here writes a file it did not already
/// own — `truncate` is the one that writes, and it writes a shorter file.
const RECLAIM: [(&str, Option<&str>); 14] = [
    // Look.
    ("df", None),
    ("du", None),
    ("ls", None),
    ("find", None),
    ("stat", None),
    ("lsof", None),
    ("pwd", None),
    ("echo", None),
    // Free.
    ("rm", None),
    ("rmdir", None),
    ("truncate", None),
    ("cargo", Some("clean")),
    ("git", Some("worktree")),
    ("git", Some("gc")),
];

/// Shell punctuation that can turn one command into another. A command
/// carrying any of it is not the one simple command this door admits.
const CHAINS: [char; 13] = [
    ';', '&', '|', '`', '$', '(', ')', '<', '>', '{', '}', '\n', '\r',
];

/// Whether `command` may still run on a filesystem under the floor.
///
/// The name says the test: not "is this safe" — the floor already decided
/// nothing is safe — but "can this only look, or only free".
#[must_use]
pub fn frees_or_looks(command: &str) -> bool {
    let command = command.trim();
    if command.is_empty() || command.contains(CHAINS) {
        return false;
    }
    let mut words = command.split_whitespace();
    let Some(program) = words.next() else {
        return false;
    };
    // A path is allowed to name the program — `/bin/rm` is `rm` — but a
    // relative one is not, because what it resolves to depends on a cwd this
    // door cannot see.
    let program = match program.rsplit_once('/') {
        Some((dir, name)) if dir.starts_with('/') => name,
        Some(_) => return false,
        None => program,
    };
    let first = words.next();
    RECLAIM.iter().any(|(name, needs)| {
        *name == program && needs.is_none_or(|needs| first == Some(needs))
    })
}

#[cfg(test)]
mod tests;
