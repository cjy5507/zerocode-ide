//! Per-worktree shell history.
//!
//! Every checkout is a different piece of work, and a shell that remembers all
//! of them in one file is a shell that cannot help with any of them: the
//! command you want is buried under four other worktrees', and a checkout you
//! deleted last week still answers `Ctrl-R`. Orca gives each worktree its own
//! history file and cleans it up with the worktree
//! (`main/terminal-history.ts:179`, `main/terminal-history-deletion.ts:206`).
//!
//! This module is the part of that with no I/O in it: which shell a spawn is
//! about to be, where that worktree's history belongs, and what to put in the
//! environment. Making the directory and writing the record are the caller's,
//! because they can fail and this cannot.
//!
//! # What the environment can and cannot do
//!
//! Measured on this machine (macOS 25.3), because the answer decides the whole
//! design:
//!
//! - **bash**: `HISTFILE` in the spawn environment survives an interactive
//!   shell. Isolation is one variable.
//! - **fish**: ignores `HISTFILE` entirely — history lives at
//!   `<data dir>/<name>_history` and the only knob is the session NAME, in
//!   `fish_history`. So the file necessarily lands in the user's fish data dir
//!   rather than under our root, which is why deleting it needs a name we
//!   recorded rather than a directory we own.
//! - **zsh**: `HISTFILE` in the environment is **destroyed**. `/etc/zshrc:16`
//!   assigns `HISTFILE=${ZDOTDIR:-$HOME}/.zsh_history` unconditionally, and it
//!   runs before any file a user owns. Measured: `HISTFILE=/tmp/probe` went in
//!   and `/Users/…/.zsh_history` came out. An env-only injection for zsh is a
//!   no-op that looks like a feature, so this module does not offer one — zsh
//!   is isolated by owning a `ZDOTDIR`, which is a different mechanism with a
//!   different door, and Orca needs the same thing for the same reason
//!   (`main/shell-templates.ts`, its `#11044`).
//!
//! # Inheritance
//!
//! These variables are EXPORTED, so a pane's children get them — including one
//! of our own windows started from a pane. Left alone, worktree B's panes would
//! all append into worktree A's history, which is the exact leak the feature
//! exists to close. So an existing value is honoured only when it is not one of
//! ours: a person who set `HISTFILE` deliberately keeps it, and a value with
//! our machine-minted shape is replaced by this worktree's. Orca fixes the same
//! two cases in `worktree-history-file-path.ts` and `fish-history-session.ts`.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// The directory our history tree lives in, under the caller's data root.
pub const HISTORY_DIR: &str = "terminal-history";

/// Session names we mint for fish, and the only ones we will ever drop.
const FISH_PREFIX: &str = "zerocode_";

/// Hex characters in a worktree's directory name. Long enough not to collide,
/// short enough to read in a path.
///
/// Public because it is part of the layout, not an implementation detail: the
/// shape [`ours`] recognises is written in terms of it, and so is anything that
/// later walks the tree looking for directories to sweep.
pub const HASH_LEN: usize = 16;

/// Which shell a spawn is about to be.
///
/// Resolved from the binary's name rather than a setting, because the name is
/// what the spawn actually runs. Prefix matching on the base name so a
/// versioned `bash-5.2` and a nix-store `/nix/store/…/bin/zsh` are recognised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    Zsh,
    Bash,
    Fish,
    /// PowerShell, `cmd`, or something we have never heard of. All three are
    /// the same fact here: no history knob we know how to set.
    Other,
}

/// Which shell this path runs.
#[must_use]
pub fn shell_of(path: &str) -> Shell {
    let name = path
        .replace('\\', "/")
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let name = name.strip_suffix(".exe").unwrap_or(&name);
    if name.starts_with("zsh") {
        Shell::Zsh
    } else if name.starts_with("bash") {
        Shell::Bash
    } else if name.starts_with("fish") {
        Shell::Fish
    } else {
        Shell::Other
    }
}

impl Shell {
    /// The file this shell keeps its history in, inside the worktree's own
    /// directory — `None` for the shells whose history is not a file we can
    /// name.
    ///
    /// fish is `None` on purpose and is not the same `None` as an unknown
    /// shell's: it has isolation, through [`fish_session`], just not through a
    /// path — [`fish_session`] is the whole of fish's isolation.
    #[must_use]
    pub fn history_file(self) -> Option<&'static str> {
        match self {
            Shell::Zsh => Some("zsh_history"),
            Shell::Bash => Some("bash_history"),
            Shell::Fish | Shell::Other => None,
        }
    }
}

/// The directory name a worktree's history lives under: the first
/// [`HASH_LEN`] hex characters of the SHA-256 of its path.
///
/// A hash rather than the path itself because the path contains separators and
/// arbitrary characters, and this becomes one directory name on three
/// platforms.
#[must_use]
pub fn worktree_hash(worktree_id: &str) -> String {
    let digest = Sha256::digest(worktree_id.as_bytes());
    let mut out = String::with_capacity(HASH_LEN);
    for byte in digest.iter() {
        if out.len() >= HASH_LEN {
            break;
        }
        out.push_str(&format!("{byte:02x}"));
    }
    out.truncate(HASH_LEN);
    out
}

/// Where this worktree's history belongs, under `root`.
#[must_use]
pub fn history_dir(root: &Path, worktree_id: &str) -> PathBuf {
    root.join(HISTORY_DIR).join(worktree_hash(worktree_id))
}

/// Whether this `HISTFILE` is one we minted.
///
/// Shape only, and deliberately: the same answer has to be reachable without
/// knowing which data root the value came from — a window with a different root
/// than the pane that exported it is exactly the nesting case this guards.
///
/// The shape is machine-minted and not one a person types: an absolute path
/// whose last three segments are [`HISTORY_DIR`], a [`HASH_LEN`]-character
/// lowercase hex directory, and one of the history file names. If it were ever
/// matched wrongly the cost is bounded — one spawn's variable is replaced by
/// this worktree's, and nothing on disk is read, written or removed.
#[must_use]
pub fn ours(value: &str) -> bool {
    let path = value.replace('\\', "/");
    if !is_rooted(&path) {
        return false;
    }
    let mut walk = path.rsplit('/');
    let (Some(file), Some(hash), Some(dir)) = (walk.next(), walk.next(), walk.next()) else {
        return false;
    };
    dir == HISTORY_DIR
        && is_hash(hash)
        && [Shell::Zsh, Shell::Bash]
            .iter()
            .any(|shell| shell.history_file() == Some(file))
}

/// A relative path of our shape is the person's, not ours — theirs is the only
/// kind that can be relative, because everything we mint is joined onto a root.
fn is_rooted(path: &str) -> bool {
    path.starts_with('/')
        || path
            .split_once(":/")
            .is_some_and(|(drive, _)| drive.len() == 1 && drive.starts_with(char::is_alphabetic))
}

/// Lowercase only, because lowercase is all [`worktree_hash`] ever writes.
///
/// Accepting uppercase would mean calling `…/AAAAAAAAAAAAAAAA/bash_history` one
/// of ours and overwriting it — a file only a person could have made, and the
/// one thing this predicate exists to protect.
fn is_hash(value: &str) -> bool {
    value.len() == HASH_LEN && is_lower_hex(value)
}

/// The alphabet a hash of ours is written in.
fn is_lower_hex(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// The fish session name for a worktree's hash.
#[must_use]
pub fn fish_session(worktree_hash: &str) -> String {
    format!("{FISH_PREFIX}{worktree_hash}")
}

/// Whether this `fish_history` is one we minted — the same argument as
/// [`ours`], for the name-shaped half of the problem.
#[must_use]
pub fn our_fish_session(value: &str) -> bool {
    value
        .strip_prefix(FISH_PREFIX)
        .is_some_and(|rest| !rest.is_empty() && rest.len() <= 64 && is_lower_hex(rest))
}

/// Basename of the file every wrapper directory of ours is stamped with.
///
/// Its presence is how both sides recognise our own wrapper instead of treating
/// it as somebody's `ZDOTDIR` — the shell-side check reads this name, and so
/// does anything of ours that has to refuse to hand a wrapper back as a config
/// root. One name, said once.
pub const ZSH_WRAPPER_MARKER: &str = ".zerocode-shell-wrapper";

/// The worktree's history path, for the wrapper to re-apply after the person's
/// own configuration has finished loading.
pub const ZSH_HISTFILE_VAR: &str = "ZEROCODE_HISTFILE";

/// The `ZDOTDIR` the wrapper hands back before it does anything else.
pub const ZSH_ORIG_ZDOTDIR_VAR: &str = "ZEROCODE_ORIG_ZDOTDIR";

/// The one file a zsh wrapper directory needs.
///
/// zsh reads `$ZDOTDIR/.zshenv` first and everything else — `/etc/zshrc`, the
/// person's `.zprofile`, `.zshrc`, `.zlogin` — through whatever `ZDOTDIR` says
/// by then. So this file gives `ZDOTDIR` back **before anything else**, and
/// from that instant zsh reads every remaining startup file out of the person's
/// own directory, exactly as it would with no wrapper at all. Nothing of theirs
/// is replaced, shadowed, or reordered; `/etc/zshrc` even derives their own
/// `.zsh_history` path rather than one inside this directory.
///
/// Then three things, in this order and for these reasons:
///
/// 1. The history path is captured into a variable that **cannot be inherited**
///    and the exported one is destroyed — here, not in the hook below. A
///    configuration that replaces `precmd_functions` wholesale drops the hook,
///    and an exported value nothing will ever consume is then inherited by every
///    child of this pane, including one of our own windows. Orca learned this
///    one twice (`#11146`).
/// 2. The person's own `.zshenv` is sourced at top level, so their exports,
///    functions, `fpath` and options land in the scope they expect. Wrapped in
///    `{ } always { }` because the whole compound command is parsed before any
///    of it runs — so the registration is already parsed as zsh even if their
///    `.zshenv` switches the shell into `sh` emulation.
/// 3. A `precmd` hook re-applies the history path once, after every file they
///    own has loaded, and then takes itself out of the list.
///
/// `builtin` on every command, because a person's own function named `source`,
/// `unset` or `typeset` must not be able to stand in the way — and
/// `emulate -L zsh` first in the hook, because its body runs after their
/// configuration and would otherwise inherit whatever options that left set
/// (under `NO_UNSET` an unset `precmd_functions` is fatal).
#[must_use]
pub fn zsh_wrapper_zshenv() -> String {
    format!(
        r#"# ZeroCode-generated zsh startup wrapper. Do not edit — regenerated on launch.
#
# This file exists to give one worktree its own shell history. It hands ZDOTDIR
# back to you on its first line, so every other startup file is read from your
# own directory exactly as it would be without it.

builtin typeset -g _zerocode_histfile="${{{histfile}:-}}"
builtin unset {histfile}

__zerocode_usable_zdotdir() {{
  [[ -n "${{1:-}}" ]] || return 1
  # Never one of ours: that self-loop is the whole reason for the marker file.
  [[ ! -f "$1/{marker}" ]] || return 1
  # A directory holding no zsh startup file is not a config root, whoever wrote
  # it — and a stale value naming one would stop zsh from ever reading the real
  # .zshenv.
  local _zerocode_startup
  for _zerocode_startup in .zshenv .zshrc .zprofile .zlogin; do
    [[ -r "$1/$_zerocode_startup" ]] && return 0
  done
  return 1
}}
if __zerocode_usable_zdotdir "${{{orig}:-}}"; then
  builtin export ZDOTDIR="${orig}"
else
  builtin unset ZDOTDIR
fi
builtin unset {orig}
builtin unfunction __zerocode_usable_zdotdir

{{
  builtin typeset _zerocode_user_zshenv="${{ZDOTDIR-$HOME}}/.zshenv"
  [[ ! -r "$_zerocode_user_zshenv" ]] || builtin source -- "$_zerocode_user_zshenv"
}} always {{
  builtin unset _zerocode_user_zshenv
  builtin typeset -ag precmd_functions
  (( ${{precmd_functions[(Ie)__zerocode_deferred_init]}} )) || precmd_functions+=(__zerocode_deferred_init)
}}

__zerocode_deferred_init() {{
  # First, because this body runs after your configuration and would otherwise
  # inherit whatever options it left set.
  builtin emulate -L zsh
  (( $+_zerocode_deferred_init_done )) && return 0
  builtin typeset -g _zerocode_deferred_init_done=1
  builtin typeset -g precmd_functions
  precmd_functions=(${{precmd_functions:#__zerocode_deferred_init}})
  if [[ -n "${{_zerocode_histfile:-}}" ]]; then
    HISTFILE="$_zerocode_histfile"
  fi
  builtin unset _zerocode_histfile
  builtin unfunction __zerocode_deferred_init
}}
"#,
        histfile = ZSH_HISTFILE_VAR,
        orig = ZSH_ORIG_ZDOTDIR_VAR,
        marker = ZSH_WRAPPER_MARKER,
    )
}

/// What to put in a zsh spawn's environment, given the wrapper directory the
/// caller has built and this worktree's history directory.
///
/// Empty when the person set their own `HISTFILE`: the wrapper exists only to
/// carry history, so there is nothing for it to carry and no reason to stand in
/// their shell's startup. A value of ours that arrived by inheritance is not
/// their choice and does not count — see the module's note.
///
/// Their `ZDOTDIR` is passed along to be handed back, unless it is the very
/// wrapper we are about to point at — the self-loop, and the one form of it this
/// side can see without touching a disk. A wrapper from some OTHER install can
/// arrive the same way, and that one is refused by the shell side reading
/// [`ZSH_WRAPPER_MARKER`], which is where the question belongs: it is asked at
/// the moment it matters, about the directory actually in hand, and it keeps a
/// filesystem call off the road that opens a terminal.
#[must_use]
pub fn zsh_history_env(
    wrapper: &Path,
    dir: &Path,
    look: &impl Fn(&str) -> Option<String>,
) -> Vec<(String, String)> {
    if look("HISTFILE").is_some_and(|held| !held.is_empty() && !ours(&held)) {
        return Vec::new();
    }
    let Some(file) = Shell::Zsh.history_file() else {
        return Vec::new();
    };
    let mut env = vec![
        (
            "ZDOTDIR".to_string(),
            wrapper.to_string_lossy().into_owned(),
        ),
        (
            ZSH_HISTFILE_VAR.to_string(),
            dir.join(file).to_string_lossy().into_owned(),
        ),
    ];
    if let Some(theirs) =
        look("ZDOTDIR").filter(|held| !held.is_empty() && Path::new(held) != wrapper)
    {
        env.push((ZSH_ORIG_ZDOTDIR_VAR.to_string(), theirs));
    }
    env
}

/// The file fish writes a session's history into, inside [`fish_data_dir`].
///
/// Its own layout, not ours: fish appends `_history` to the session name. Said
/// here because the only way to take a worktree's fish history away is to name
/// that file, and a guess would either miss it or reach for someone else's.
#[must_use]
pub fn fish_history_file(session: &str) -> String {
    format!("{session}_history")
}

/// The directory fish will write its history into, for a given environment.
///
/// Resolved from the SPAWN environment and not this process's: a window can be
/// launched with a different `XDG_DATA_HOME` or `HOME` than the shells it
/// starts, and fish follows its own. Recorded at spawn time because it is the
/// only moment it is knowable — fish's history file lives outside our tree, so
/// a later cleanup can only find it by what was written down here.
///
/// `XDG_DATA_HOME` is deliberately not redirected: that would move every other
/// tool's data for the session, not just fish's history.
#[must_use]
pub fn fish_data_dir(look: &impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let xdg = look("XDG_DATA_HOME")
        .map(|held| held.trim().to_string())
        .filter(|held| Path::new(held).is_absolute());
    let data_home = match xdg {
        Some(held) => PathBuf::from(held),
        None => {
            let home = look("HOME").map(|held| held.trim().to_string())?;
            if home.is_empty() {
                return None;
            }
            PathBuf::from(home).join(".local").join("share")
        }
    };
    Some(data_home.join("fish"))
}

/// EVERY knob we own gets this worktree's value — not only the one the shell
/// being spawned happens to read.
///
/// Because they are exported. A bash typed inside a fish pane inherits
/// `HISTFILE`, so setting only `fish_history` leaves that bash appending to
/// whatever worktree the environment came from: the leak this closes. The same
/// in reverse for a fish started inside bash.
///
/// One exception, and it is zsh's: `HISTFILE` is deliberately NOT set for a zsh
/// spawn. Measured — a value that ARRIVES in the environment stays exported,
/// and `/etc/zshrc` then overwrites it, so `HISTFILE=/tmp/probe` went in and a
/// child of that shell saw `/Users/…/.zsh_history` come out. A bash typed in
/// that pane would append to the person's zsh history, which is a new problem
/// rather than the old one. Left out of the environment, zsh's own `HISTFILE`
/// is a plain parameter no child ever sees, and the wrapper sets it there.
///
/// Each knob is checked on its own, the way Ghostty, Kitty and VS Code check
/// `HISTFILE` — except that a value we minted ourselves is not a person's
/// choice, it is the inheritance.
#[must_use]
pub fn history_env(
    dir: &Path,
    worktree_hash: &str,
    shell: Shell,
    look: &impl Fn(&str) -> Option<String>,
) -> Vec<(String, String)> {
    let mut env = Vec::new();
    let theirs = |name: &str, ours_already: fn(&str) -> bool| {
        look(name).is_some_and(|held| !held.is_empty() && !ours_already(&held))
    };
    if shell != Shell::Zsh
        && let Some(file) = Shell::Bash.history_file()
        && !theirs("HISTFILE", ours)
    {
        env.push((
            "HISTFILE".to_string(),
            dir.join(file).to_string_lossy().into_owned(),
        ));
    }
    // fish's knob is safe for every spawn: nothing but fish reads it, and no
    // configuration file assigns it.
    if !theirs("fish_history", our_fish_session) {
        env.push(("fish_history".to_string(), fish_session(worktree_hash)));
    }
    env
}

#[cfg(test)]
mod tests {
    use super::{
        HISTORY_DIR, Shell, ZSH_HISTFILE_VAR, ZSH_ORIG_ZDOTDIR_VAR, ZSH_WRAPPER_MARKER,
        fish_data_dir, fish_history_file, fish_session, history_dir, history_env, our_fish_session,
        ours, shell_of, worktree_hash, zsh_history_env, zsh_wrapper_zshenv,
    };
    use std::path::{Path, PathBuf};

    fn nothing(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn a_shell_is_read_off_the_binary_it_runs() {
        for (path, want) in [
            ("/bin/zsh", Shell::Zsh),
            ("/nix/store/abc-zsh-5.9/bin/zsh", Shell::Zsh),
            ("/usr/local/bin/bash-5.2", Shell::Bash),
            ("/opt/homebrew/bin/fish", Shell::Fish),
            ("C:\\Windows\\System32\\cmd.exe", Shell::Other),
            ("/usr/bin/pwsh", Shell::Other),
            ("", Shell::Other),
        ] {
            assert_eq!(shell_of(path), want, "{path}");
        }
    }

    #[test]
    fn zsh_is_not_offered_an_environment_that_would_not_work() {
        // Measured, not assumed, twice over. /etc/zshrc assigns HISTFILE
        // unconditionally and runs before any file a person owns — and a
        // HISTFILE that ARRIVED in the environment stays exported, so the value
        // that file writes reaches a bash typed inside the pane. So a zsh spawn
        // is handed no HISTFILE at all.
        let zsh = history_env(
            Path::new("/data"),
            &worktree_hash("/w"),
            Shell::Zsh,
            &nothing,
        );
        assert!(
            !zsh.iter().any(|(name, _)| name == "HISTFILE"),
            "a zsh spawn was handed a HISTFILE: {zsh:?}"
        );
        // fish's knob still travels — a fish started inside that pane is a fish.
        assert!(zsh.iter().any(|(name, _)| name == "fish_history"));
        // It still knows the file's name, because the ZDOTDIR road wants it.
        assert_eq!(Shell::Zsh.history_file(), Some("zsh_history"));
        // And that road answers instead.
        assert!(!zsh_history_env(Path::new("/w/wrap"), Path::new("/d"), &nothing).is_empty());
    }

    #[test]
    fn bash_gets_this_worktrees_file_and_fish_gets_its_own_name() {
        let hash = worktree_hash("/Users/dev/2026/zerocode");
        let dir = history_dir(Path::new("/data"), "/Users/dev/2026/zerocode");
        assert_eq!(dir, Path::new("/data").join(HISTORY_DIR).join(&hash));

        // Every knob, for every spawn: these variables are EXPORTED, so a bash
        // typed inside a fish pane inherits HISTFILE and would otherwise append
        // to whichever worktree the environment came from.
        let both = vec![
            (
                "HISTFILE".to_string(),
                format!("/data/{HISTORY_DIR}/{hash}/bash_history"),
            ),
            ("fish_history".to_string(), fish_session(&hash)),
        ];
        assert_eq!(history_env(&dir, &hash, Shell::Bash, &nothing), both);
        assert_eq!(history_env(&dir, &hash, Shell::Fish, &nothing), both);
        // Including a shell we have no knob of our own for: whatever it starts
        // is still in this worktree.
        assert_eq!(history_env(&dir, &hash, Shell::Other, &nothing), both);
    }

    #[test]
    fn two_worktrees_never_share_a_directory() {
        assert_ne!(worktree_hash("/a/one"), worktree_hash("/a/two"));
        assert_eq!(worktree_hash("/a/one").len(), 16);
        assert!(
            worktree_hash("/a/one")
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        );
    }

    #[test]
    fn a_histfile_the_person_set_is_left_alone_and_only_that_knob() {
        let look = |name: &str| (name == "HISTFILE").then(|| "/Users/dev/.my_history".to_string());
        let env = history_env(
            Path::new("/data/terminal-history/00112233aabbccdd"),
            "00112233aabbccdd",
            Shell::Bash,
            &look,
        );
        // Their choice stands, and it says nothing about the other knob: each
        // is asked about on its own.
        assert!(!env.iter().any(|(name, _)| name == "HISTFILE"), "{env:?}");
        assert!(
            env.iter().any(|(name, _)| name == "fish_history"),
            "{env:?}"
        );
    }

    #[test]
    fn a_shape_only_a_person_could_have_written_is_never_called_ours() {
        // Uppercase is not an alphabet we mint in. Calling it ours would mean
        // overwriting a file only a person could have made — the exact thing
        // these two predicates exist to prevent.
        assert!(!ours(
            "/data/terminal-history/AAAAAAAAAAAAAAAA/bash_history"
        ));
        assert!(!ours(
            "/data/terminal-history/00112233AABBCCDD/bash_history"
        ));
        assert!(!our_fish_session("zerocode_DEADBEEF"));
        assert!(our_fish_session("zerocode_deadbeef"));
    }

    #[test]
    fn a_histfile_we_minted_ourselves_is_replaced_and_not_honoured() {
        // The nesting case: this window was started from one of our own panes,
        // so worktree A's path arrived in the environment. Honouring it would
        // put every pane of this window into A's history.
        let inherited = "/elsewhere/terminal-history/ffffffffffffffff/bash_history";
        assert!(ours(inherited));
        let look = |name: &str| (name == "HISTFILE").then(|| inherited.to_string());
        let dir = Path::new("/data/terminal-history/00112233aabbccdd");
        assert_eq!(
            history_env(dir, "00112233aabbccdd", Shell::Bash, &look)
                .into_iter()
                .find(|(name, _)| name == "HISTFILE"),
            Some((
                "HISTFILE".to_string(),
                "/data/terminal-history/00112233aabbccdd/bash_history".to_string()
            ))
        );
    }

    #[test]
    fn only_the_machine_minted_shape_is_called_ours() {
        assert!(ours("/data/terminal-history/00112233aabbccdd/zsh_history"));
        assert!(ours(
            "C:/Users/dev/terminal-history/00112233aabbccdd/bash_history"
        ));
        for theirs in [
            // Relative — everything we mint is joined onto a root.
            "terminal-history/00112233aabbccdd/zsh_history",
            // Not our directory.
            "/data/history/00112233aabbccdd/zsh_history",
            // Not a hash.
            "/data/terminal-history/my-project/zsh_history",
            // Right length, not hex.
            "/data/terminal-history/zzzzzzzzzzzzzzzz/zsh_history",
            // Not a history file we name.
            "/data/terminal-history/00112233aabbccdd/notes.txt",
            // A person's own, which happens to sit near the words.
            "/Users/dev/.zsh_history",
            "",
        ] {
            assert!(!ours(theirs), "{theirs}");
        }
    }

    #[test]
    fn a_zsh_spawn_is_pointed_at_the_wrapper_and_told_its_history() {
        let env = zsh_history_env(
            Path::new("/data/shell-wrapper/zsh"),
            Path::new("/data/terminal-history/00112233aabbccdd"),
            &nothing,
        );
        assert_eq!(
            env,
            vec![
                ("ZDOTDIR".to_string(), "/data/shell-wrapper/zsh".to_string()),
                (
                    ZSH_HISTFILE_VAR.to_string(),
                    "/data/terminal-history/00112233aabbccdd/zsh_history".to_string()
                ),
            ]
        );
    }

    #[test]
    fn a_zdotdir_of_their_own_is_handed_back_and_one_of_ours_never_is() {
        let wrapper = Path::new("/data/shell-wrapper/zsh");
        let theirs = |name: &str| (name == "ZDOTDIR").then(|| "/Users/dev/.config/zsh".to_string());
        assert_eq!(
            zsh_history_env(wrapper, Path::new("/d"), &theirs)
                .into_iter()
                .find(|(name, _)| name == ZSH_ORIG_ZDOTDIR_VAR)
                .map(|(_, value)| value),
            Some("/Users/dev/.config/zsh".to_string())
        );
        // Inherited from one of our own panes: handing this back would point
        // ZDOTDIR at the wrapper it came from, which is the self-loop.
        let ours_already =
            |name: &str| (name == "ZDOTDIR").then(|| "/data/shell-wrapper/zsh".to_string());
        assert!(
            !zsh_history_env(wrapper, Path::new("/d"), &ours_already)
                .iter()
                .any(|(name, _)| name == ZSH_ORIG_ZDOTDIR_VAR)
        );
    }

    #[test]
    fn a_histfile_they_set_keeps_the_wrapper_out_of_their_startup() {
        // The wrapper carries history and nothing else, so with their own
        // choice standing there is nothing to carry — and no reason at all to
        // be in the way of their shell starting up.
        let look = |name: &str| (name == "HISTFILE").then(|| "/Users/dev/.my_history".to_string());
        assert!(zsh_history_env(Path::new("/w"), Path::new("/d"), &look).is_empty());
        // A value of OURS is not their choice: it arrived by inheritance.
        let inherited = |name: &str| {
            (name == "HISTFILE")
                .then(|| "/elsewhere/terminal-history/ffffffffffffffff/zsh_history".to_string())
        };
        assert!(!zsh_history_env(Path::new("/w"), Path::new("/d"), &inherited).is_empty());
    }

    #[test]
    fn the_wrapper_hands_zdotdir_back_before_it_reads_anything() {
        let text = zsh_wrapper_zshenv();
        let handback = text
            .find(r#"builtin export ZDOTDIR="$ZEROCODE_ORIG_ZDOTDIR""#)
            .expect("the wrapper stopped handing ZDOTDIR back");
        let reading = text
            .find(r#"builtin source -- "$_zerocode_user_zshenv""#)
            .expect("the wrapper stopped reading the person's own .zshenv");
        let hook = text
            .find("__zerocode_deferred_init() {")
            .expect("the wrapper lost its deferred hook");
        // Order is the whole design: everything after the handback — /etc/zshrc
        // included — is read from their own directory.
        assert!(
            handback < reading && reading < hook,
            "the wrapper's three steps are out of order:\n{text}"
        );
        // Both exported names are consumed, so nothing this pane spawns can
        // inherit either of them.
        for consumed in [
            &format!("builtin unset {ZSH_HISTFILE_VAR}"),
            &format!("builtin unset {ZSH_ORIG_ZDOTDIR_VAR}"),
        ] {
            assert!(text.contains(consumed.as_str()), "missing `{consumed}`");
        }
        // The hook runs after their configuration, so it cannot inherit the
        // options that configuration left set.
        let body = &text[hook..];
        assert!(
            body.contains("builtin emulate -L zsh"),
            "the deferred hook runs under whatever options were left set:\n{body}"
        );
        // And it recognises our own wrapper by the marker, not by a path shape.
        assert!(text.contains(ZSH_WRAPPER_MARKER), "the marker went missing");
    }

    #[test]
    fn a_fish_sessions_file_is_named_the_way_fish_names_it() {
        assert_eq!(
            fish_history_file(&fish_session("00112233aabbccdd")),
            "zerocode_00112233aabbccdd_history"
        );
    }

    #[test]
    fn fishs_own_data_dir_is_read_from_the_spawns_environment() {
        let xdg = |name: &str| match name {
            "XDG_DATA_HOME" => Some("/opt/share".to_string()),
            "HOME" => Some("/Users/dev".to_string()),
            _ => None,
        };
        assert_eq!(fish_data_dir(&xdg), Some(PathBuf::from("/opt/share/fish")));
        // A relative XDG_DATA_HOME is not one fish would follow either.
        let relative = |name: &str| match name {
            "XDG_DATA_HOME" => Some("share".to_string()),
            "HOME" => Some("/Users/dev".to_string()),
            _ => None,
        };
        assert_eq!(
            fish_data_dir(&relative),
            Some(PathBuf::from("/Users/dev/.local/share/fish"))
        );
        // Nowhere to guess from is answered with nothing, not with a guess.
        assert_eq!(fish_data_dir(&|_: &str| None), None);
    }

    #[test]
    fn a_fish_session_of_ours_is_recognised_and_a_persons_is_not() {
        assert!(our_fish_session(&fish_session("00112233aabbccdd")));
        for theirs in ["", "zerocode_", "zerocode_not-hex", "work", "fish"] {
            assert!(!our_fish_session(theirs), "{theirs}");
        }
        // And a person's own name survives the spawn.
        let look = |name: &str| (name == "fish_history").then(|| "work".to_string());
        let env = history_env(Path::new("/d"), "00112233aabbccdd", Shell::Fish, &look);
        assert!(
            !env.iter().any(|(name, _)| name == "fish_history"),
            "{env:?}"
        );
    }
}
