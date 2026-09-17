//! The PATH a GUI app does not have.
//!
//! A macOS app launched from Finder or the Dock inherits launchd's PATH —
//! `/usr/bin:/bin:/usr/sbin:/sbin` — not the user's shell PATH. Every agent this
//! window cares about lives outside that list: homebrew's `/opt/homebrew/bin`,
//! npm's globals, nvm's shims, `~/.local/bin`. So a window that reads
//! `std::env::var_os("PATH")` detects nothing and launches nothing, **exactly and
//! only when it was started the way apps are normally started.** Started from a
//! terminal it inherits everything and looks fine — which is why this window
//! shipped broken: every test run and every `cargo run` came from a terminal.
//!
//! The fix is Orca's, measured from `managed-agent-hook-controls-Hy3KYvcp.js`
//! (**1.4.169**): ask the user's own shell what PATH it builds.
//!
//! - `pickShell` (:7777-7782): `$SHELL`, else `/bin/zsh` on macOS, `/bin/bash`
//!   elsewhere; nothing on Windows (GUI apps there inherit a real PATH).
//! - `spawnShellAndReadPath` (:7793-7860): `shell -ilc 'printf …$PATH…'` between
//!   two delimiter strings, stdout piped, stderr ignored, **5s timeout** then
//!   SIGKILL. Interactive AND login (`-ilc`), because nvm and friends initialise
//!   in `~/.zshrc`, which a plain login shell never reads.
//! - `parseCapturedPath` (:7783-7792): strip ANSI, take what sits between the
//!   two delimiters — a shell that echoes a banner or colours its prompt must
//!   not have that banner read as a directory.
//! - `mergePathSegments` (:7878-7890): the shell's segments first, then the
//!   process's own that the shell did not name.
//! - Hydration runs before every detection (`hydrateShellPathForAgentDetection`,
//!   index.js:93910) and the refresh button forces a re-read (:94100-94117) —
//!   its tooltip is literally "Re-read your shell PATH".
//!
//! **One deliberate difference.** Orca writes the merged PATH back into
//! `process.env.PATH`, and every later spawn inherits it. In Rust 2024
//! `std::env::set_var` is unsafe precisely because other threads may be reading
//! the environment, and this process is full of threads. So the merged PATH
//! lives in a static here, and the two places that need it ask for it: agent
//! detection reads [`hydrate`], and every PTY child is handed `PATH` explicitly
//! through `hooks::pty_env`. Same effect, no global mutation.

use std::sync::Mutex;
use std::time::Duration;

/// Ours, not Orca's string — a third-party sentinel would be a trademark in a
/// process listing. Same shape: unlikely to appear in any shell's own output.
pub const DELIMITER: &str = "__ZEROCODE_SHELL_PATH__";

/// Orca's `SPAWN_TIMEOUT_MS = 5e3`. A shell that takes longer than this to
/// start is not going to answer, and the window must not hang on it.
pub const SPAWN_TIMEOUT: Duration = Duration::from_secs(5);

/// Which shell to ask (`pickShell`, :7777-7782).
///
/// Takes the variable as a parameter rather than reading it, so the tests do
/// not mutate a process-global — the same lesson the vault's `roots` learned.
pub fn pick_shell(shell_var: Option<&str>, os: &str) -> Option<String> {
    if os == "windows" {
        return None;
    }
    if let Some(shell) = shell_var.map(str::trim).filter(|value| !value.is_empty()) {
        return Some(shell.to_string());
    }
    Some(
        if os == "macos" {
            "/bin/zsh"
        } else {
            "/bin/bash"
        }
        .to_string(),
    )
}

/// Strip ANSI escape sequences (`ANSI_RE`, :7775: `\x1b\[[0-9;?]*[A-Za-z]`).
///
/// A zsh with a themed prompt colours everything it prints, including our
/// delimiters — read raw, the escape bytes land inside a "directory".
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            out.push(ch);
            continue;
        }
        if chars.peek() != Some(&'[') {
            continue;
        }
        chars.next();
        // Parameter bytes, then one final alphabetic byte, exactly as the
        // regex reads it.
        while let Some(&next) = chars.peek() {
            chars.next();
            if next.is_ascii_alphabetic() {
                break;
            }
            if !(next.is_ascii_digit() || next == ';' || next == '?') {
                break;
            }
        }
    }
    out
}

/// The PATH between the two delimiters, split and deduped
/// (`parseCapturedPath`, :7783-7792).
///
/// Everything outside the delimiters is the shell's own noise — banners, rc
/// echoes, "last login" lines — and is discarded without being read.
pub fn parse_captured(stdout: &str) -> Vec<String> {
    let cleaned = strip_ansi(stdout);
    let Some(first) = cleaned.find(DELIMITER) else {
        return Vec::new();
    };
    let value_at = first + DELIMITER.len();
    let Some(second) = cleaned[value_at..].find(DELIMITER) else {
        return Vec::new();
    };
    let value = cleaned[value_at..value_at + second].trim();
    if value.is_empty() {
        return Vec::new();
    }
    let mut seen = std::collections::BTreeSet::new();
    value
        .split(':')
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .filter(|segment| seen.insert(segment.to_string()))
        .map(str::to_string)
        .collect()
}

/// The shell's segments first, then the process's own that the shell did not
/// name (`mergePathSegments`, :7878-7890).
///
/// The shell first is the point: an agent installed twice — an old copy in a
/// system directory, the real one under nvm — must resolve to the one the
/// user's terminal would run.
pub fn merge(current: Option<&str>, segments: &[String]) -> Option<String> {
    if segments.is_empty() {
        return None;
    }
    let mut shell: Vec<&str> = Vec::new();
    for segment in segments {
        if !shell.contains(&segment.as_str()) {
            shell.push(segment);
        }
    }
    let named: std::collections::BTreeSet<&str> = shell.iter().copied().collect();
    let mut merged: Vec<&str> = shell.clone();
    for segment in current
        .unwrap_or_default()
        .split(':')
        .filter(|s| !s.is_empty())
    {
        if !named.contains(segment) && !merged.contains(&segment) {
            merged.push(segment);
        }
    }
    Some(merged.join(":"))
}

/// Ask one shell for its PATH, bounded (`spawnShellAndReadPath`, :7793-7860).
///
/// `-i` and `-l` together, or nvm-installed agents stay invisible; stderr to
/// null because an interactive shell prints prompts there; killed at the
/// timeout because a shell stuck in an rc file must not hang the window.
fn read_login_shell_path(shell: &str, timeout: Duration) -> Vec<String> {
    let command =
        format!("printf '%s' '{DELIMITER}'; printf '%s' \"$PATH\"; printf '%s' '{DELIMITER}'");
    let Ok(mut child) = crate::proc::quiet_command(shell)
        .args(["-ilc", &command])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return Vec::new();
    };

    // The read happens on its own thread because a child that never exits also
    // never closes its stdout; the timeout below must not depend on it.
    let stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        use std::io::Read;
        let mut text = String::new();
        if let Some(mut stdout) = stdout {
            let _ = stdout.read_to_string(&mut text);
        }
        text
    });

    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            _ => {
                // Timed out or unreadable: kill, and report nothing rather than
                // whatever half a PATH made it out. The reader thread is NOT
                // joined here — a grandchild the shell left behind (an `sleep`
                // in somebody's rc file) inherits the pipe and holds it open
                // long after the shell itself is dead, and joining would wait
                // on it for exactly as long as the timeout existed to avoid.
                // The thread parks on a dead pipe and costs nothing.
                let _ = child.kill();
                let _ = child.wait();
                return Vec::new();
            }
        }
    }
    reader
        .join()
        .map(|text| parse_captured(&text))
        .unwrap_or_default()
}

/// The merged PATH, hydrated once and kept.
///
/// `None` inside means "not asked yet or nothing learned" — the callers fall
/// back to the process PATH, which is today's behaviour, so a machine where the
/// shell will not answer is degraded, not broken.
static HYDRATED: Mutex<Option<Option<String>>> = Mutex::new(None);

/// The merged PATH, asking the shell on the first call (or again on `force`).
///
/// Blocks up to [`SPAWN_TIMEOUT`]; call it off the UI thread. `force` is the
/// refresh button — Orca's re-detect re-reads the shell PATH first, because the
/// user pressing it has usually just installed something and *changed* it.
pub fn hydrate(force: bool) -> Option<String> {
    {
        let held = HYDRATED.lock().expect("shell path lock");
        if !force && let Some(known) = held.as_ref() {
            return known.clone();
        }
    }
    let shell = pick_shell(std::env::var("SHELL").ok().as_deref(), std::env::consts::OS);
    let merged = shell.and_then(|shell| {
        let segments = read_login_shell_path(&shell, SPAWN_TIMEOUT);
        merge(std::env::var("PATH").ok().as_deref(), &segments)
    });
    let mut held = HYDRATED.lock().expect("shell path lock");
    *held = Some(merged.clone());
    merged
}

/// The merged PATH if hydration has finished, without waiting.
///
/// This is what a PTY spawn asks — a terminal opening must never wait five
/// seconds on a stuck shell, and a launch before hydration lands simply
/// inherits the process PATH, which is what it would have done anyway.
/// The PATH a launch resolves a binary against: the hydrated shell PATH
/// when it has answered, the process's own when it has not — the same list
/// `hooks::pty_env` hands a child, so a verdict about a binary and the
/// launch that follows resolve the same name against the same list. Never
/// waits: this is read under the ledger and on the readiness probe.
pub fn launch_path() -> Option<std::ffi::OsString> {
    hydrated()
        .map(std::ffi::OsString::from)
        .or_else(|| std::env::var_os("PATH"))
}

pub fn hydrated() -> Option<String> {
    HYDRATED.lock().expect("shell path lock").clone().flatten()
}

/// Start hydration in the background.
///
/// Called once at startup, before the window exists, so the answer is usually
/// ready before the first agent list or the first launch asks for it.
pub fn warm() {
    std::thread::spawn(|| {
        let _ = hydrate(false);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Which shell answers: the user's own wins, the platform default backs it
    /// up, and Windows is not asked at all.
    #[test]
    fn the_users_shell_is_asked_and_windows_is_not() {
        assert_eq!(
            pick_shell(Some("/opt/homebrew/bin/fish"), "macos").as_deref(),
            Some("/opt/homebrew/bin/fish")
        );
        assert_eq!(pick_shell(None, "macos").as_deref(), Some("/bin/zsh"));
        assert_eq!(pick_shell(None, "linux").as_deref(), Some("/bin/bash"));
        // Set-but-empty is not a shell.
        assert_eq!(pick_shell(Some("  "), "macos").as_deref(), Some("/bin/zsh"));
        assert_eq!(pick_shell(Some("/bin/zsh"), "windows"), None);
    }

    /// The delimiters are the whole protocol: everything outside them is shell
    /// noise, and colour codes inside them are stripped before splitting.
    #[test]
    fn a_noisy_shell_still_yields_only_the_path_between_the_delimiters() {
        let stdout = format!(
            "Last login: banner\n\u{1b}[32mprompt\u{1b}[0m{DELIMITER}/opt/homebrew/bin:\u{1b}[1m/usr/bin\u{1b}[0m:/opt/homebrew/bin{DELIMITER}\ntrailing rc echo"
        );
        assert_eq!(
            parse_captured(&stdout),
            vec!["/opt/homebrew/bin".to_string(), "/usr/bin".to_string()],
            "banner text or ANSI bytes were read as directories"
        );

        // Half an answer is no answer: a shell killed mid-print must not hand
        // back a truncated segment as a real directory.
        assert_eq!(
            parse_captured(&format!("{DELIMITER}/usr/bi")),
            Vec::<String>::new()
        );
        assert_eq!(parse_captured("no delimiters at all"), Vec::<String>::new());
        assert_eq!(
            parse_captured(&format!("{DELIMITER}{DELIMITER}")),
            Vec::<String>::new()
        );
        assert_eq!(
            parse_captured(&format!("{DELIMITER}   {DELIMITER}")),
            Vec::<String>::new()
        );
    }

    /// The shell's order wins, the process's own segments survive, and merging
    /// twice changes nothing.
    #[test]
    fn the_shells_directories_come_first_and_nothing_is_lost() {
        let shell = vec![
            "/opt/homebrew/bin".to_string(),
            "/Users/j/.local/bin".to_string(),
            "/usr/bin".to_string(),
        ];
        let merged = merge(Some("/usr/bin:/bin:/usr/sbin"), &shell).expect("merged");
        assert_eq!(
            merged,
            "/opt/homebrew/bin:/Users/j/.local/bin:/usr/bin:/bin:/usr/sbin"
        );
        // Idempotent: merging the merged answer reproduces it.
        assert_eq!(
            merge(Some(&merged), &shell).as_deref(),
            Some(merged.as_str())
        );
        // A shell that answered nothing changes nothing — the caller keeps the
        // process PATH rather than replacing it with an empty one.
        assert_eq!(merge(Some("/usr/bin"), &[]), None);
        assert_eq!(merge(None, &shell).expect("merged"), shell.join(":"));
    }

    /// The real spawn, against a fake shell — a script that ignores the flags
    /// and prints a decorated PATH — and against one that hangs.
    #[test]
    fn a_real_shell_is_read_and_a_stuck_one_is_killed() {
        let dir = tempfile::tempdir().expect("temp");
        let fake = dir.path().join("fake-shell");
        std::fs::write(
            &fake,
            format!(
                "#!/bin/sh\nprintf 'Last login noise\\n'\nprintf '%s' '{DELIMITER}'\nprintf '%s' '/fake/bin:/other/bin'\nprintf '%s' '{DELIMITER}'\n"
            ),
        )
        .expect("write");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        assert_eq!(
            read_login_shell_path(&fake.to_string_lossy(), SPAWN_TIMEOUT),
            vec!["/fake/bin".to_string(), "/other/bin".to_string()]
        );

        // A shell that never answers is killed at the deadline and reports
        // nothing — the window must not hang on somebody's broken rc file.
        let stuck = dir.path().join("stuck-shell");
        std::fs::write(&stuck, "#!/bin/sh\nsleep 60\n").expect("write");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&stuck, std::fs::Permissions::from_mode(0o755))
                .expect("chmod");
        }
        let started = std::time::Instant::now();
        assert_eq!(
            read_login_shell_path(&stuck.to_string_lossy(), Duration::from_millis(300)),
            Vec::<String>::new()
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the timeout did not bound the wait"
        );

        // A shell that does not exist is also simply no answer.
        assert_eq!(
            read_login_shell_path("/nonexistent/shell", Duration::from_millis(300)),
            Vec::<String>::new()
        );
    }
}
