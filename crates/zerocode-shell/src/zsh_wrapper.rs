//! The one directory zsh needs before a worktree can keep its own history.
//!
//! Every other shell we isolate takes a variable. zsh does not: `/etc/zshrc`
//! assigns `HISTFILE=${ZDOTDIR:-$HOME}/.zsh_history` with no check, and it runs
//! before any file a person owns — so a variable would read as a feature and do
//! nothing. The lever is `ZDOTDIR`, because `$ZDOTDIR/.zshenv` is the first
//! thing zsh reads and the only file we can own.
//!
//! One directory serves every pane. The contents say nothing about any
//! worktree — the path rides in the environment — so this is written once and
//! read by every spawn, rather than a directory per checkout.
//!
//! The whole text lives in [`zerocode_core::shell_history::zsh_wrapper_zshenv`],
//! next to the rules about it. This module only does what can fail.
//!
//! Measured end to end against a real zsh (see the test below): with the
//! wrapper in front, an interactive shell ends up with the worktree's history
//! path, the person's own `.zshenv` AND `.zshrc` both loaded, `ZDOTDIR` back to
//! theirs, and neither of our two variables left in the environment for a child
//! to inherit.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use zerocode_core::shell_history::{ZSH_WRAPPER_MARKER, zsh_wrapper_zshenv};

use crate::durable_file;

/// Where the wrapper sits under the data root.
const WRAPPER_DIR: &str = "shell-wrapper";

/// What the marker file says, for whoever finds it.
const MARKER_TEXT: &str = "\
# ZeroCode-generated zsh startup wrapper directory.
#
# Its presence is how ZeroCode recognises its own wrapper instead of treating
# this as somebody's ZDOTDIR — without it, a pane started from a pane could
# hand this directory back to itself as a config root. Do not edit; the whole
# directory is rewritten on launch.
";

/// The wrapper directory, built once for this process.
///
/// Once because the contents depend on nothing that varies between panes, and
/// rewriting it per spawn would put two file writes on the road that opens a
/// terminal. `None` when it cannot be written — which is not a terminal we
/// refuse to open: zsh then keeps the history it had, which is where it was
/// before any of this existed.
pub fn wrapper_dir(data_root: &Path) -> Option<&'static Path> {
    static HELD: OnceLock<Option<PathBuf>> = OnceLock::new();
    HELD.get_or_init(|| {
        let dir = data_root.join(WRAPPER_DIR).join("zsh");
        write_wrapper(&dir).ok().map(|()| dir)
    })
    .as_deref()
}

/// Write the wrapper's two files.
///
/// Rewritten rather than patched, because the text is ours and a half-old copy
/// is the one state that would be hard to reason about. The marker goes down
/// FIRST: it is what makes this directory recognisable as ours, and a `.zshenv`
/// that exists before it could be handed back to itself by a pane that started
/// in between.
fn write_wrapper(dir: &Path) -> std::io::Result<()> {
    durable_file::ensure_private_directory(dir)?;
    durable_file::replace_bytes(&dir.join(ZSH_WRAPPER_MARKER), MARKER_TEXT.as_bytes())?;
    durable_file::replace_bytes(&dir.join(".zshenv"), zsh_wrapper_zshenv().as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{MARKER_TEXT, write_wrapper};
    use zerocode_core::shell_history::{
        ZSH_HISTFILE_VAR, ZSH_ORIG_ZDOTDIR_VAR, ZSH_WRAPPER_MARKER,
    };

    fn scratch(name: &str) -> std::path::PathBuf {
        let at = std::env::temp_dir().join(format!("zc-zsh-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&at);
        std::fs::create_dir_all(&at).expect("scratch");
        at
    }

    #[test]
    fn the_marker_is_written_before_the_file_that_needs_it() {
        let at = scratch("order");
        let dir = at.join("wrap");
        write_wrapper(&dir).expect("wrapper");
        assert_eq!(
            std::fs::read_to_string(dir.join(ZSH_WRAPPER_MARKER)).expect("marker"),
            MARKER_TEXT
        );
        // And the text is the core's, not a second copy living here.
        assert_eq!(
            std::fs::read_to_string(dir.join(".zshenv")).expect("zshenv"),
            zerocode_core::shell_history::zsh_wrapper_zshenv()
        );
        let _ = std::fs::remove_dir_all(&at);
    }

    /// A real zsh, with the wrapper in front of it.
    ///
    /// Everything else about this feature can be asserted on text; this is the
    /// only test that can say whether it WORKS, and the reason it exists is that
    /// the obvious implementation — `HISTFILE` in the environment — measurably
    /// does not.
    ///
    /// Four things at once, because they are four ways for this to be wrong:
    /// the history path is the worktree's; the person's `.zshenv` ran; their
    /// `.zshrc` ran too (so the handback reached the files zsh finds LATER);
    /// and neither of our variables survives into a child.
    ///
    /// macOS only, and not skipped there: zsh is the platform's own shell, so a
    /// missing zsh is a broken machine rather than a reason to pass quietly.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_real_zsh_takes_the_worktrees_history_and_keeps_every_file_of_theirs() {
        use std::io::Write as _;
        use std::process::Stdio;

        let at = scratch("live");
        let theirs = at.join("theirs");
        let wrapper = at.join("wrap");
        let history = at.join("history");
        for dir in [&theirs, &history] {
            std::fs::create_dir_all(dir).expect("dir");
        }
        std::fs::write(theirs.join(".zshenv"), "export ZC_FROM_ZSHENV=yes\n")
            .expect("their zshenv");
        std::fs::write(theirs.join(".zshrc"), "export ZC_FROM_ZSHRC=yes\n").expect("their zshrc");
        write_wrapper(&wrapper).expect("wrapper");
        let histfile = history.join("zsh_history");

        // Piped stdin rather than `-i -c`: the hook runs on the first PROMPT,
        // and `zsh -i -c` never draws one. Measured — with `-c` the shell ends
        // up on the person's own path instead, which is the right way to
        // degrade but says nothing about the feature.
        //
        // In its own session: an interactive zsh with job control takes the
        // FOREGROUND of whatever controlling terminal it inherits, and when
        // this test runs inside an agent's pane (`cargo test` typed into zo,
        // 2026-09-02) that was the agent's own pty — its prompt landed in the
        // pane, its reads stole the keys, and the agent sat at 100% CPU on a
        // terminal it no longer had. A session of its own has no terminal to
        // take.
        let mut command = crate::proc::quiet_command("zsh");
        // The terminal a pane's zsh is told it runs in, not the one this test
        // happens to run in. Under Terminal.app the inherited
        // `TERM_PROGRAM=Apple_Terminal` made zsh source
        // `/etc/zshrc_Apple_Terminal`, which moves HISTFILE into
        // `~/.zsh_sessions/<TERM_SESSION_ID>.historynew` — measured 2026-09-17,
        // and exactly what every pane of a window opened from Terminal.app did
        // before the lane declared its own name.
        for name in zerocode_pty::INHERITED_TERMINAL_IDENTITY {
            command.env_remove(name);
        }
        for (name, value) in zerocode_pty::DECLARED_TERMINAL {
            command.env(name, value);
        }
        command
            .arg("-i")
            .env_remove("HISTFILE")
            .env("HOME", &theirs)
            .env("ZDOTDIR", &wrapper)
            .env(ZSH_HISTFILE_VAR, &histfile)
            .env(ZSH_ORIG_ZDOTDIR_VAR, &theirs)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        {
            use std::os::unix::process::CommandExt as _;
            // SAFETY: `setsid` is async-signal-safe and touches nothing but
            // this child's own session; it runs after fork, before exec.
            unsafe {
                command.pre_exec(|| {
                    if libc::setsid() == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }
        let mut child = command.spawn().expect("zsh is the platform shell here");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(
                b"print -r -- \"<<<$HISTFILE|$ZC_FROM_ZSHENV|$ZC_FROM_ZSHRC|\
                  ${ZEROCODE_HISTFILE:-gone}|${ZEROCODE_ORIG_ZDOTDIR:-gone}>>>\"\nexit\n",
            )
            .expect("write");
        let out = child.wait_with_output().expect("zsh finished");
        let said = String::from_utf8_lossy(&out.stdout);
        // The prompt writes escape codes around it, so the payload is fenced.
        let payload = said
            .split_once("<<<")
            .and_then(|(_, rest)| rest.split_once(">>>"))
            .map(|(inside, _)| inside)
            .unwrap_or_else(|| panic!("zsh said nothing we asked for:\n{said}"));
        let fields: Vec<&str> = payload.split('|').collect();
        assert_eq!(
            fields.first().copied(),
            Some(histfile.to_string_lossy().as_ref()),
            "zsh did not end up on this worktree's history:\n{payload}"
        );
        assert_eq!(
            (fields.get(1).copied(), fields.get(2).copied()),
            (Some("yes"), Some("yes")),
            "a startup file of theirs did not run:\n{payload}"
        );
        assert_eq!(
            (fields.get(3).copied(), fields.get(4).copied()),
            (Some("gone"), Some("gone")),
            "one of our variables is left for a child to inherit:\n{payload}"
        );
        let _ = std::fs::remove_dir_all(&at);
    }
}
