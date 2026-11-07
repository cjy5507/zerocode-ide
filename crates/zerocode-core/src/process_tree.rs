//! A child and every process it started, ended together (t-98207).
//!
//! `Child::kill` ends one process, and a CLI npm installs is two or three
//! deep: on Windows its `.cmd`, which std starts through `cmd.exe`, runs
//! node, and node runs the CLI's own binary; on unix the node script runs
//! that binary. Ending the child alone left the rest running, holding the
//! pipe a reader waits on. So a child that may have to be ended leads a
//! tree of its own ([`lead_own_tree`]) and [`end_tree`] ends the tree: on
//! unix its process group, on Windows `taskkill /T`, which follows
//! parentage — the way ZO's build runner ends a step's `cmd` (`end_tree`).
//!
//! For the children of a console program, such as the mirror in a person's
//! terminal: `taskkill` here shares that console. The window has no console,
//! so it starts its own `taskkill` through its quiet door (`proc.rs`), with
//! the same [`TASKKILL_TREE`] words.

use std::process::{Child, Command};

/// `taskkill`'s words for one pid and every process it started, forced: the
/// pid follows them. Spelled once for every Windows caller that ends a tree.
pub const TASKKILL_TREE: [&str; 3] = ["/T", "/F", "/PID"];

/// How long [`end_tree`] waits for `taskkill` before it lets go of it. It
/// answers in a moment; one that does not must not hold the caller longer
/// than this. Chosen, not measured: there is no Windows machine here.
#[cfg(windows)]
const TASKKILL_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// How often [`end_tree`] looks whether `taskkill` has ended.
#[cfg(windows)]
const TASKKILL_LOOK_EVERY: std::time::Duration = std::time::Duration::from_millis(10);

/// Make the child `command` starts the head of a tree [`end_tree`] can end:
/// on unix the leader of a process group of its own, which every process it
/// starts joins unless it leaves; on Windows nothing, since `taskkill /T`
/// follows parentage. A group of its own is not the terminal's foreground
/// one, so this is for a child that never reads the terminal.
pub fn lead_own_tree(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    #[cfg(not(unix))]
    {
        let _ = command;
    }
}

/// End `child`, started with [`lead_own_tree`], and every process it
/// started, then collect it. Its pid cannot have been taken by anyone else:
/// the child is collected only at the end.
pub fn end_tree(child: &mut Child) {
    #[cfg(unix)]
    {
        if let Ok(group) = i32::try_from(child.id()) {
            let _ = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(group),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
    }
    #[cfg(windows)]
    end_tree_by_taskkill(child.id());
    let _ = child.kill();
    let _ = child.wait();
}

/// `taskkill /T /F /PID <pid>`, waited for [`TASKKILL_WAIT`] at most.
#[cfg(windows)]
fn end_tree_by_taskkill(pid: u32) {
    use std::process::Stdio;
    let Ok(mut taskkill) = Command::new("taskkill")
        .args(TASKKILL_TREE)
        .arg(pid.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return;
    };
    let until = std::time::Instant::now() + TASKKILL_WAIT;
    while matches!(taskkill.try_wait(), Ok(None)) && std::time::Instant::now() < until {
        std::thread::sleep(TASKKILL_LOOK_EVERY);
    }
    let _ = taskkill.kill();
    let _ = taskkill.wait();
}

/// Unix only: the Windows tree is ended by `taskkill`, which this machine
/// cannot run, so there is nothing here for a Windows build to compile.
#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// What a child started leaves running is ended with it: the shape of
    /// npm's codex, a script whose binary runs under it. The kernel is asked
    /// about the grandchild itself, by its pid.
    #[test]
    fn a_tree_ends_with_what_its_head_started() {
        use std::io::BufRead as _;
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "sleep 30 & echo $!; wait"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped());
        lead_own_tree(&mut command);
        let mut child = command.spawn().expect("a head with a grandchild");
        let mut line = String::new();
        std::io::BufReader::new(child.stdout.take().expect("its output"))
            .read_line(&mut line)
            .expect("the grandchild's pid");
        let grandchild: u32 = line.trim().parse().expect("a pid");
        assert!(crate::settings_file_lock::process_alive(grandchild));

        end_tree(&mut child);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while crate::settings_file_lock::process_alive(grandchild)
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            !crate::settings_file_lock::process_alive(grandchild),
            "the grandchild {grandchild} outlived its tree"
        );
    }
}
