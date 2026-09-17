//! Children this product starts and never reads again: the system opener, the
//! file manager, a detached server.
//!
//! A `std::process::Child` dropped without `wait` is not gone when it exits —
//! it is a **zombie**, a row in the process table that stands until somebody
//! collects its status, and nobody ever will: the drop threw away the only
//! handle. Five of them were found under a live session (each one a click on
//! a URL or a Finder reveal), and they last as long as the window does.
//!
//! So fire-and-forget is a door, not a pattern: spawn here, and a parked
//! thread collects the exit whenever it comes. One thread per child is the
//! entire cost — the opener exits in milliseconds and the thread with it; a
//! detached server parks one thread for the window's life, which also means a
//! server that CRASHES is collected instead of standing in the table as a
//! second kind of corpse.

use std::process::Command;

/// Spawn a child nobody will read, and collect it when it ends.
///
/// The pid comes back for the caller that wants to log it — and for the test
/// below, which watches the process table itself, because "the child was
/// reaped" is a fact about the kernel and only the kernel can attest it.
pub fn spawn_forgotten(mut command: Command) -> std::io::Result<u32> {
    let mut child = command.spawn()?;
    let pid = child.id();
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The kernel's own answer: after the child exits, its row LEAVES the
    /// process table — a zombie would stand there with state `Z` for the rest
    /// of this test process's life, which is exactly what a dropped handle
    /// produces and what this door exists to prevent.
    #[test]
    fn a_forgotten_child_is_collected_not_left_as_a_zombie() {
        let mut echo = Command::new("true");
        echo.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let pid = spawn_forgotten(echo).expect("true must spawn");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let row = Command::new("ps")
                .args(["-o", "stat=", "-p", &pid.to_string()])
                .output()
                .expect("ps must run");
            let stat = String::from_utf8_lossy(&row.stdout).trim().to_string();
            // Gone from the table is the pass; a lingering `Z` is the bug.
            if stat.is_empty() {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the child was never collected — the table still says {stat:?}"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}
