//! The transparent tee an agent's launch wears, so a nested run has a screen.
//!
//! `codex exec …` under Claude's Bash tool writes to a pipe Claude owns — the
//! bytes never reach a pty, so there was nothing for the window to draw and
//! the person watched a spinner over an invisible four-minute run (reported
//! three times, in the end as "창이 분할되면서 터미널로 보여야"). This binary
//! stands in front of the real one on `PATH` inside ZeroCode's terminals and
//! does exactly three things:
//!
//! - **A tty means hands off.** An interactive `codex` owns a real terminal
//!   already and must keep every TUI behaviour it has; the shim `exec`s the
//!   real binary and ceases to exist.
//! - **A pipe means tee.** stdout and stderr are spliced byte-for-byte to the
//!   parent that captured them AND appended to one file under
//!   `$ZEROCODE_MIRROR_DIR` — ANSI intact, because the reader on the other
//!   end is a real terminal grid that speaks it natively.
//! - **The exit code is the child's.** A wrapper that launders failures into
//!   its own exit code breaks every caller that checks one.
//! - **An orchestrator never gets a pipe-shaped worker.** Inside a ZeroCode
//!   orchestration seat, a non-interactive nested agent launch is refused with
//!   the exact ledger commands to use. Codex→Claude and Claude→Codex must both
//!   produce a real worker terminal; the mirror is not a degraded substitute
//!   when durable authority is temporarily unavailable.
//!
//! The window learns about the run through the same loopback bridge the hook
//! scripts use — a form POST with the pane's own coordinates from the
//! environment. Every bridge failure is swallowed: the mirror is a witness,
//! and a witness that can kill the run it watches is worse than no witness.

use std::io::{IsTerminal, Read, Write};
use std::sync::{Arc, Mutex};

fn main() {
    let mut args = std::env::args_os().skip(1);
    let Some(real) = args.next() else {
        eprintln!("zerocode-mirror: real binary path missing");
        std::process::exit(127);
    };
    let rest: Vec<std::ffi::OsString> = args.collect();

    // An interactive run is not ours to touch. The question is asked with
    // std's own portable spelling ("윈도우에서도 돌아가야함"); only the ANSWER
    // differs by platform — unix replaces this process outright, and a
    // platform with no exec runs the child with untouched stdio and wears
    // its exit code, which is the same transparency one fork later.
    if std::io::stdout().is_terminal() {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            let error = std::process::Command::new(&real).args(&rest).exec();
            eprintln!("zerocode-mirror: {}: {error}", real.to_string_lossy());
            std::process::exit(127);
        }
        #[cfg(not(unix))]
        {
            let status = std::process::Command::new(&real).args(&rest).status();
            match status {
                Ok(done) => std::process::exit(done.code().unwrap_or(1)),
                Err(error) => {
                    eprintln!("zerocode-mirror: {}: {error}", real.to_string_lossy());
                    std::process::exit(127);
                }
            }
        }
    }

    let vendor = std::path::Path::new(&real).file_name().map_or_else(
        || "agent".to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    let probe = is_probe(&rest);
    if refuses_pipe_worker(probe, orchestration_seat_from_env()) {
        eprintln!(
            "ZeroCode orchestration guard: `{vendor}` was not started as a raw nested command. \
Create the work with `zerocode-orc task-create --spec <work> --title <name>`, then run \
`zerocode-orc worker-start --agent {vendor} --task <task-id> --prompt <work>`. \
If durable authority is unavailable, stop and retry after recovery; do not fall back to \
`{vendor} exec` or a background pipe."
        );
        std::process::exit(2);
    }
    let log = if probe {
        None
    } else {
        mirror_log_path(&vendor)
    };

    let mut child = match std::process::Command::new(&real)
        .args(&rest)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            eprintln!("zerocode-mirror: {}: {error}", real.to_string_lossy());
            std::process::exit(127);
        }
    };

    if let Some(path) = &log {
        announce("ZerocodeMirrorStart", &vendor, path, child.id());
    }

    let sink = log.as_ref().and_then(|path| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()
            .map(|file| Arc::new(Mutex::new(file)))
    });

    let out = child.stdout.take().map(|pipe| {
        let sink = sink.clone();
        std::thread::spawn(move || splice(pipe, std::io::stdout(), sink))
    });
    let err = child.stderr.take().map(|pipe| {
        let sink = sink.clone();
        std::thread::spawn(move || splice(pipe, std::io::stderr(), sink))
    });
    if let Some(thread) = out {
        let _ = thread.join();
    }
    if let Some(thread) = err {
        let _ = thread.join();
    }
    let status = child.wait();
    if let Some(path) = &log {
        announce("ZerocodeMirrorEnd", &vendor, path, std::process::id());
    }
    std::process::exit(status.ok().and_then(|held| held.code()).unwrap_or(1));
}

/// Pump one pipe to its original destination and, when there is one, the
/// mirror file. The original write failing ends the pump — the parent hung
/// up, and a mirror of a stream nobody reads is a file growing for no one.
/// The mirror failing is ignored: the run must never notice its witness.
fn splice(mut from: impl Read, mut to: impl Write, sink: Option<Arc<Mutex<std::fs::File>>>) {
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let count = match from.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(count) => count,
        };
        if to.write_all(&buffer[..count]).is_err() {
            break;
        }
        let _ = to.flush();
        if let Some(file) = &sink
            && let Ok(mut file) = file.lock()
        {
            let _ = file.write_all(&buffer[..count]);
        }
    }
}

/// A version or help probe is not a run. A coordinator sniffs its agents
/// before launching them — `codex --version`, a `--help` sweep — and a
/// window that opens a pane for every probe fills with help dumps (reported
/// as "codex 화면이 재대로 동작안해"). A probe still runs through the tee
/// untouched; it just leaves no file and wakes no window. Only an argument
/// that IS one of the four flags counts — a prompt merely mentioning
/// `--help` is one quoted argument and matches nothing.
fn is_probe(rest: &[std::ffi::OsString]) -> bool {
    // The one definition, shared with the window's nested-run reader
    // (`zerocode_core::hook::nested_agent_of`): a probe is refused a mirror
    // file here by the same rule that refuses it a navigator row there.
    let args: Vec<String> = rest
        .iter()
        .map(|arg| arg.to_str().unwrap_or("").to_string())
        .collect();
    zerocode_core::hook::is_agent_probe_argv(&args)
}

/// Whether this process sits in a real orchestration seat.
///
/// All three values are required. A person's plain terminal may inherit one
/// unrelated variable and must keep normal CLI behaviour; a coordinator or
/// worker gets the id, pane and capability as one launch contract.
fn orchestration_seat_from_env() -> bool {
    use zerocode_core::agent_teams::{TEAM_ID_VAR, TEAM_PANE_VAR, TEAM_TOKEN_VAR};
    [TEAM_ID_VAR, TEAM_PANE_VAR, TEAM_TOKEN_VAR]
        .into_iter()
        .all(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
}

/// A probe is observation, not delegated work. Everything else launched with
/// piped output from an orchestration seat would create the asymmetric mirror
/// page this guard exists to remove.
const fn refuses_pipe_worker(probe: bool, orchestration_seat: bool) -> bool {
    orchestration_seat && !probe
}

/// Where this run's bytes are mirrored, or `None` when no window asked.
fn mirror_log_path(vendor: &str) -> Option<std::path::PathBuf> {
    let dir = std::path::PathBuf::from(std::env::var_os("ZEROCODE_MIRROR_DIR")?);
    if !dir.is_dir() {
        return None;
    }
    Some(dir.join(format!("{vendor}-{}.ansi", std::process::id())))
}

/// Tell the window, through the hook scripts' own bridge, with the pane's own
/// coordinates. Hand-rolled HTTP because the peer is our own loopback server
/// one connect away, and a dependency for that is a dependency.
fn announce(event: &str, vendor: &str, path: &std::path::Path, pid: u32) {
    let Some(port) = std::env::var(zerocode_hookd::env_var::PORT).ok() else {
        return;
    };
    let token = std::env::var(zerocode_hookd::env_var::TOKEN).unwrap_or_default();
    let read = |name: &str| std::env::var(name).unwrap_or_default();
    let payload = format!(
        r#"{{"vendor":{},"path":{},"pid":{pid}}}"#,
        json_text(vendor),
        json_text(&path.to_string_lossy()),
    );
    let body = [
        ("pane_key", read(zerocode_hookd::env_var::PANE_KEY)),
        ("tab_id", read(zerocode_hookd::env_var::TAB_ID)),
        ("launch_token", read(zerocode_hookd::env_var::LAUNCH_TOKEN)),
        ("worktree_id", read(zerocode_hookd::env_var::WORKTREE_ID)),
        ("env", read(zerocode_hookd::env_var::AGENT_ENV)),
        ("version", read(zerocode_hookd::env_var::VERSION)),
        ("hook_event_name", event.to_string()),
        ("payload", payload),
    ]
    .iter()
    .map(|(field, value)| format!("{field}={}", form_encode(value)))
    .collect::<Vec<_>>()
    .join("&");
    let request = format!(
        "POST /hook/claude HTTP/1.1\r\nHost: 127.0.0.1\r\n{}: {token}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        zerocode_hookd::HOOK_TOKEN_HEADER,
        body.len(),
    );
    let address = format!("127.0.0.1:{port}");
    let Ok(mut stream) = std::net::TcpStream::connect_timeout(
        &address
            .parse()
            .unwrap_or_else(|_| "127.0.0.1:0".parse().unwrap()),
        std::time::Duration::from_millis(500),
    ) else {
        return;
    };
    let _ = stream.set_write_timeout(Some(std::time::Duration::from_millis(500)));
    let _ = stream.write_all(request.as_bytes());
    // One read so the server finishes before the socket drops; the answer
    // itself is nothing the mirror can act on.
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(500)));
    let mut answer = [0_u8; 256];
    let _ = stream.read(&mut answer);
}

/// A string as a JSON text — quotes, backslashes and control bytes escaped.
fn json_text(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if (ch as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

/// application/x-www-form-urlencoded, the strict way: everything but the
/// unreserved four classes percent-escaped, space included.
fn form_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two encoders the bridge's parser meets: JSON that survives quotes
    /// and control bytes, and form bytes that survive everything.
    #[test]
    fn the_wire_encodings_hold_their_edges() {
        assert_eq!(json_text("a\"b\\c\nd"), r#""a\"b\\c\nd""#);
        assert_eq!(form_encode("a b+c&d=e%"), "a%20b%2Bc%26d%3De%25");
        assert_eq!(form_encode("한글"), "%ED%95%9C%EA%B8%80");
        assert_eq!(form_encode("safe-_.~AZ09"), "safe-_.~AZ09");
    }

    /// Probes are told apart from runs by exact flag equality: a `--help`
    /// anywhere in the argument list marks a probe, a prompt that merely
    /// mentions the word does not.
    #[test]
    fn a_probe_is_a_flag_not_a_word_in_a_prompt() {
        let args = |list: &[&str]| -> Vec<std::ffi::OsString> {
            list.iter().map(std::ffi::OsString::from).collect()
        };
        assert!(is_probe(&args(&["--version"])));
        assert!(is_probe(&args(&["-V"])));
        assert!(is_probe(&args(&["exec", "--help"])));
        assert!(is_probe(&args(&["-h"])));
        assert!(!is_probe(&args(&["exec", "explain what --help does"])));
        assert!(!is_probe(&args(&[
            "exec",
            "--skip-git-repo-check",
            "리뷰해줘"
        ])));
        assert!(!is_probe(&args(&[])));
    }

    /// The direction does not matter: a seated orchestrator gets a real
    /// worker for delegated work, never a mirror page. Harmless capability
    /// probes and a person's unseated shell keep the transparent path.
    #[test]
    fn an_orchestration_seat_refuses_pipe_workers_but_not_probes() {
        assert!(refuses_pipe_worker(false, true));
        assert!(!refuses_pipe_worker(true, true));
        assert!(!refuses_pipe_worker(false, false));
    }

    /// The splice is byte-faithful to both destinations and survives a dead
    /// mirror file without disturbing the main stream.
    #[test]
    fn a_spliced_stream_reaches_both_sides_byte_for_byte() {
        let source: &[u8] = b"\x1b[31mred\x1b[0m and \xff raw bytes";
        let mut through = Vec::new();
        let dir = std::env::temp_dir().join(format!("zc-mirror-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("out.ansi");
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .unwrap();
        splice(source, &mut through, Some(Arc::new(Mutex::new(file))));
        assert_eq!(through, source);
        assert_eq!(std::fs::read(&path).unwrap(), source);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
