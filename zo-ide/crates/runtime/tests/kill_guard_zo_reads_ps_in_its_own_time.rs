//! zo gives `ps` the guard's own time (t-81315, 4275 m-84899, review w-83819
//! P2-1).
//!
//! runtime's own tests give `ps` a minute, so a real-run answer never turns on
//! how fast `ps` is under a release lane's load. That minute must never reach
//! zo. An integration test builds runtime as zo does, without `cfg(test)`, so
//! the time it sees is the time zo gives: past 800 ms the table does not read,
//! and a kill aimed at a process zo itself started is a question. Were the
//! minute to reach zo, the table would read and the kill would be let through.
//!
//! This binary holds this one test, so the `ps` it puts first on `PATH` meets
//! no other test.

#![cfg(unix)]

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use zerocode_core::shell_rule::kill::KillVerdict;

/// How late the stand-in `ps` answers, in seconds: well past the 800 ms zo
/// gives `ps`, and well within the minute runtime's own tests give it.
const PS_DELAY_SECS: u32 = 2;

/// A `sleep` this test started, ended and reaped when dropped.
struct OwnChild(Child);

impl Drop for OwnChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A `ps` that answers [`PS_DELAY_SECS`] late, first on `PATH` until dropped.
struct SlowPs {
    dir: PathBuf,
    path: Option<std::ffi::OsString>,
}

impl SlowPs {
    fn start() -> Self {
        use std::os::unix::fs::PermissionsExt as _;

        let path = std::env::var_os("PATH");
        let real = path
            .iter()
            .flat_map(std::env::split_paths)
            .map(|dir| dir.join("ps"))
            .find(|ps| ps.is_file())
            .expect("a ps on PATH");
        let dir = std::env::temp_dir().join(format!("zo-kill-guard-zo-time-ps-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("the slow ps's folder");
        let ps = dir.join("ps");
        let script = format!(
            "#!/bin/sh\n/bin/sleep {PS_DELAY_SECS}\nexec '{}' \"$@\"\n",
            real.display()
        );
        std::fs::write(&ps, script).expect("the slow ps is written");
        std::fs::set_permissions(&ps, std::fs::Permissions::from_mode(0o755)).expect("the slow ps runs");
        let mut dirs = vec![dir.clone()];
        dirs.extend(path.iter().flat_map(std::env::split_paths));
        std::env::set_var("PATH", std::env::join_paths(dirs).expect("a PATH"));
        Self { dir, path }
    }
}

impl Drop for SlowPs {
    fn drop(&mut self) {
        match self.path.take() {
            Some(path) => std::env::set_var("PATH", path),
            None => std::env::remove_var("PATH"),
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn zo_gives_ps_the_guards_own_time_not_the_minute_runtimes_tests_give_it() {
    let child = OwnChild(
        Command::new("/bin/sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("a child of this test"),
    );
    let slow = SlowPs::start();
    let verdict = runtime::kill_guard::judge(&format!("kill {}", child.0.id()));
    drop(slow);
    match verdict {
        Some(KillVerdict::Ask { reason }) => assert!(
            reason.contains("could not be checked"),
            "the table did not read in zo's time, and the question says so: {reason}"
        ),
        other => panic!("zo read a 2 s ps as if it had a minute: {other:?}"),
    }
}
