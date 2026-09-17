//! `zerocode` — the shell IDE's entry point.
//!
//! This is the process that actually wires the two channels the architecture
//! describes, and it is deliberately a terminal front-end for now: the window
//! (Tauri) comes later, but nothing about the wiring changes when it does.
//!
//! ```text
//! zerocode status            what is on this machine, and what is on the port
//! zerocode lane [<session>]   host `zo attach` in a pty and render its screen
//! zerocode watch <session>    follow the same session as structured frames
//! zerocode vault-lint         the second brain's lint table, exit 1 on findings
//! ```
//!
//! `status` is the one that proves the independence contract: on a machine with
//! no `zo` it reports that plainly and exits 0, because an IDE that cannot find
//! an agent is still an IDE.

use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use zerocode_core::SessionLabel;
use zerocode_harness::{Client, Incoming, ServeErrorKind};
use zerocode_lane::{
    DEFAULT_SERVE_READY_TIMEOUT, LaneError, LaneSupervisor, LocalPty, PlatformTokenError,
    PtyHandle, PtyTransport, ServeProbe, ServeState, TOKEN_ENV, default_bind_for,
    probe_session_server, project_root, validate_loopback_bind,
};
use zerocode_pty::ZoBinary;

mod vault_lint;

const USAGE: &str = "\
zerocode — shell IDE for parallel agent lanes

    zerocode status              report the agent harness and session server
    zerocode lane [<session>]    host `zo attach` in a pty and render it
    zerocode watch <session>     follow a session as structured frames
    zerocode vault-lint          lint the second-brain vault (exit 1 on findings)

Options:
    --bind <addr>   session server address (loopback only). Defaults to a port
                    derived from this project, so two checkouts never share a
                    session pool by accident.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let project = project_root(&cwd);
    let mut bind = default_bind_for(&project);
    let mut positional: Vec<String> = Vec::new();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--bind" => match iter.next() {
                Some(value) => bind = value,
                None => return fail("--bind needs an address"),
            },
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            _ => positional.push(arg),
        }
    }

    let command = positional.first().map(String::as_str).unwrap_or("status");
    let argument = positional.get(1).cloned();

    match command {
        // Its own arguments, its own usage: the vault has no bind address.
        "vault-lint" => vault_lint::run(&positional[1..]),
        "status" => run_status(&bind, &project),
        "lane" => run_lane(&bind, &project, argument.as_deref()),
        "watch" => match argument {
            Some(session) => run_watch(&bind, &project, &session),
            None => fail("watch needs a session id"),
        },
        other => fail(&format!("unknown command `{other}`\n\n{USAGE}")),
    }
}

/// Build the supervisor, or explain why there is none.
///
/// Token order: the environment when the user already exported one (so we join
/// their server), else the project's **persisted** token — the same file the
/// window reads, which is what lets `zerocode lane` and the window share one
/// server across restarts. If persistence is unavailable, startup stops with
/// an actionable error instead of creating a second identity that the
/// detached server will reject. An explicitly exported token remains the one
/// supported stateless escape hatch.
fn supervisor(bind: &str, project: &Path) -> Result<Option<LaneSupervisor>, LaneError> {
    let Some(zo) = ZoBinary::discover() else {
        return Ok(None);
    };
    let token = project_session_token(project)?;
    LaneSupervisor::new(zo, bind, Some(token)).map(Some)
}

fn project_session_token(project: &Path) -> Result<String, LaneError> {
    session_token_with(std::env::var(TOKEN_ENV).ok(), || {
        zerocode_lane::project_token_for_platform(project)
    })
}

fn session_token_with(
    explicit: Option<String>,
    persisted: impl FnOnce() -> Result<String, PlatformTokenError>,
) -> Result<String, LaneError> {
    match explicit {
        Some(token) if !token.is_empty() => Ok(token),
        _ => require_persisted_supervisor_token(persisted()),
    }
}

fn require_persisted_supervisor_token(
    token: Result<String, PlatformTokenError>,
) -> Result<String, LaneError> {
    token.map_err(Into::into)
}

fn run_status(bind: &str, project: &Path) -> ExitCode {
    let supervisor = match supervisor(bind, project) {
        Ok(Some(supervisor)) => supervisor,
        Ok(None) => {
            // The independence contract: no agent, but the IDE is fine.
            println!("agent harness : not found on PATH");
            println!("session server: not applicable");
            println!();
            println!("Lanes are unavailable; every other surface still works.");
            println!("Install `zo` to enable them.");
            return ExitCode::SUCCESS;
        }
        Err(error) => return fail(&error.to_string()),
    };

    println!("agent harness : {}", supervisor.zo_path().display());
    let state = match supervisor.probe() {
        ServeProbe::Ready => "running, and it accepts our token",
        ServeProbe::Unauthorized => "running, but it does not share our token",
        ServeProbe::NotSessionServer => "a foreign process holds this port",
        ServeProbe::NotListening => "not running (a lane will start one)",
    };
    println!("session server: {state} @ {}", supervisor.bind_addr());
    ExitCode::SUCCESS
}

/// Host a lane: the terminal channel.
///
/// Everything here is blocking on purpose — a pty read blocks, and this command
/// owns the terminal anyway.
fn run_lane(bind: &str, project: &Path, session: Option<&str>) -> ExitCode {
    let supervisor = match supervisor(bind, project) {
        Ok(Some(supervisor)) => supervisor,
        Ok(None) => return fail("no `zo` on PATH — run `zerocode status` for details"),
        Err(error) => return fail(&error.to_string()),
    };

    match supervisor.ensure_serve(DEFAULT_SERVE_READY_TIMEOUT) {
        Ok(state) => announce_serve(state, supervisor.bind_addr()),
        Err(error) => return fail(&error.to_string()),
    }

    let (rows, cols) = terminal_size();
    // No id has to reach `zo attach` as an *absent* argument, not a placeholder:
    // the server creates a session only when none is named, so inventing one
    // gets a lane that renders and is owned by nothing.
    let lane = match supervisor.open_lane(Box::new(LocalPty), session, None, &[], rows, cols) {
        Ok(lane) => lane,
        Err(error) => return fail(&error.to_string()),
    };
    // The supervisor's token, not the environment's: when nothing exported one
    // we minted it above, and the server we just vouched for accepts only that.
    let label = session.map(|session| label_for(bind, supervisor.token(), session));
    announce_lane(label.as_ref());

    drive_lane(lane, session)
}

/// Say which server this lane joined.
///
/// "Started one" and "joined one that was already there" have different
/// consequences for the sessions a person is about to see, so they are not the
/// same sentence.
fn announce_serve(state: ServeState, addr: &str) {
    match state {
        ServeState::AlreadyRunning => eprintln!("zerocode: joined the session server on {addr}"),
        ServeState::Started => eprintln!("zerocode: started a session server on {addr}"),
    }
}

/// Name the lane on screen.
///
/// A session id is not a name — see [`SessionLabel`] — so the id only appears
/// alone when nothing has been asked in the session yet.
fn announce_lane(label: Option<&SessionLabel>) {
    match label {
        Some(label) if label.is_named() => eprintln!(
            "zerocode: lane open on “{}” ({}) — Ctrl-D to detach\n",
            label.name, label.id
        ),
        Some(label) => eprintln!(
            "zerocode: lane open on session `{}` — Ctrl-D to detach\n",
            label.id
        ),
        None => eprintln!("zerocode: lane open on a new session — Ctrl-D to detach\n"),
    }
}

/// Forward stdin lines on their own thread.
///
/// Reading stdin blocks, and doing it inline would freeze the screen for as
/// long as a person takes to finish a line.
fn forward_stdin() -> std::sync::mpsc::Receiver<String> {
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let Ok(mut line) = line else { break };
            line.push('\n');
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

/// Render the lane and feed it what the person types, until either side ends.
fn drive_lane(mut lane: PtyHandle, session: Option<&str>) -> ExitCode {
    let input = forward_stdin();
    let mut painted = String::new();

    loop {
        let pumped = lane.pump();
        paint_if_changed(&mut lane, &mut painted);

        if pumped.ended {
            eprintln!("\nzerocode: the attach client exited; the session stays on the server");
            return ExitCode::SUCCESS;
        }
        match input.try_recv() {
            Ok(line) => {
                if let Err(error) = lane.write_input(line.as_bytes()) {
                    return fail(&error.to_string());
                }
            }
            // Ctrl-D: detach, do not kill the session.
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                announce_detach(session);
                return ExitCode::SUCCESS;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
        std::thread::sleep(Duration::from_millis(16));
    }
}

/// Print the screen, but only when it actually changed.
///
/// Detaching leaves the session on the server, so what is on screen is a view
/// rather than the record — reprinting it is always safe, just noisy.
fn paint_if_changed(lane: &mut PtyHandle, painted: &mut String) {
    if !lane.terminal_mut().grid_mut().take_dirty() {
        return;
    }
    let screen = lane.terminal().grid().visible_text();
    if screen == *painted {
        return;
    }
    println!("{screen}");
    let _ = std::io::stdout().flush();
    *painted = screen;
}

/// Say how to come back, which differs by whether the session has a name yet.
fn announce_detach(session: Option<&str>) {
    match session {
        Some(session) => {
            eprintln!("\nzerocode: detached — reattach with `zerocode lane {session}`");
        }
        None => eprintln!(
            "\nzerocode: detached — the session stays on the server; \
             `zerocode watch <id>` to find it"
        ),
    }
}

/// Ask the session server what to call this session.
///
/// `token` is the credential the caller already vouched the server with, passed
/// in rather than read from the environment. Re-reading the environment here
/// would silently skip the token this process **minted** when nothing exported
/// one — which is the default path — so the subscribe would be refused and
/// every lane would fall back to showing its id.
///
/// Never fails the caller: a name is what the screen shows, not what it
/// addresses, so an unreachable server or an unfamiliar history shape falls
/// back to the id rather than stopping a lane from opening.
fn label_for(bind: &str, token: Option<&str>, session: &str) -> SessionLabel {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return SessionLabel::new(session, None);
    };
    let token = token.map(str::to_string);
    let question = runtime.block_on(async {
        let mut client = Client::connect(bind, token).await.ok()?;
        client.session_name(session).await.ok().flatten()
    });
    SessionLabel::new(session, question.as_deref())
}

/// Follow a session as typed frames: the structured channel.
fn run_watch(bind: &str, project: &Path, session: &str) -> ExitCode {
    let (address, token) = match protected_watch_identity(bind, || project_session_token(project)) {
        Ok(identity) => identity,
        Err(error) => return fail(&error.to_string()),
    };
    match probe_session_server(address, &token) {
        ServeProbe::Ready => {}
        ServeProbe::Unauthorized => {
            return fail(&format!(
                "the session server rejected our token; export {TOKEN_ENV} to match it"
            ));
        }
        ServeProbe::NotSessionServer => {
            return fail("the loopback address belongs to a different service");
        }
        ServeProbe::NotListening => return fail("the session server is not listening"),
    }
    let bind = address.to_string();
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => return fail(&error.to_string()),
    };

    runtime.block_on(async move {
        let mut client = match Client::connect(&bind, Some(token)).await {
            Ok(client) => client,
            Err(error) => return fail(&format!("cannot reach the session server: {error}")),
        };
        // The hydration snapshot is also what names the session, so watching
        // shows what was asked rather than an id nobody can tell apart.
        let hydrated = match client.subscribe(session, true).await {
            Ok(hydrated) => hydrated,
            Err(error) => {
                if let zerocode_harness::HarnessError::Rpc { kind, .. } = &error
                    && *kind == ServeErrorKind::Unauthorized
                {
                    return fail(&format!(
                        "the session server rejected our token; export {TOKEN_ENV} to match it"
                    ));
                }
                return fail(&error.to_string());
            }
        };
        let label = SessionLabel::new(
            session,
            hydrated
                .get("history")
                .and_then(zerocode_harness::first_question_in)
                .as_deref(),
        );
        if label.is_named() {
            eprintln!(
                "zerocode: watching “{}” ({}) — frames follow\n",
                label.name, label.id
            );
        } else {
            eprintln!("zerocode: watching `{}` — frames follow\n", label.id);
        }

        loop {
            match client.next_incoming().await {
                Ok(Some(Incoming::Frame(frame))) => {
                    let kind = frame
                        .get("type")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("frame");
                    println!("[{kind}] {frame}");
                }
                Ok(Some(Incoming::Response { id, .. })) => {
                    println!("[response #{id}]");
                }
                Ok(None) => {
                    eprintln!("zerocode: the server closed the stream");
                    return ExitCode::SUCCESS;
                }
                Err(error) => return fail(&error.to_string()),
            }
        }
    })
}

fn protected_watch_identity(
    bind: &str,
    token: impl FnOnce() -> Result<String, LaneError>,
) -> Result<(std::net::SocketAddr, String), LaneError> {
    let address = validate_loopback_bind(bind)?;
    Ok((address, token()?))
}

/// Terminal size, or a sane default when we are not attached to one (a pipe,
/// CI). The lane still needs a grid to draw into.
fn terminal_size() -> (u16, u16) {
    if !std::io::stdout().is_terminal() {
        return (24, 80);
    }
    let rows = std::env::var("LINES").ok().and_then(|v| v.parse().ok());
    let cols = std::env::var("COLUMNS").ok().and_then(|v| v.parse().ok());
    (rows.unwrap_or(24), cols.unwrap_or(80))
}

fn fail(message: &str) -> ExitCode {
    eprintln!("zerocode: {message}");
    ExitCode::FAILURE
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::io;

    use super::*;

    #[test]
    fn every_unresolved_persistent_authority_refuses_a_second_supervisor_identity() {
        for error in [
            PlatformTokenError::PathUnavailable(io::Error::new(
                io::ErrorKind::NotFound,
                "no app path",
            )),
            PlatformTokenError::MigrationPending,
            PlatformTokenError::Storage(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "token is unreadable",
            )),
        ] {
            let result = require_persisted_supervisor_token(Err(error));

            assert!(matches!(result, Err(LaneError::TokenStorage(_))));
        }
    }

    #[test]
    fn an_explicit_token_bypasses_storage_and_an_absent_one_uses_it() {
        let storage_calls = Cell::new(0);
        let explicit = session_token_with(Some("shared".to_string()), || {
            storage_calls.set(storage_calls.get() + 1);
            Ok("stored".to_string())
        })
        .expect("explicit token");
        assert_eq!(explicit, "shared");
        assert_eq!(storage_calls.get(), 0);

        let stored = session_token_with(None, || {
            storage_calls.set(storage_calls.get() + 1);
            Ok("stored".to_string())
        })
        .expect("persisted token");
        assert_eq!(stored, "stored");
        assert_eq!(storage_calls.get(), 1);
    }

    #[test]
    fn watch_rejects_a_remote_bind_before_resolving_a_credential() {
        let token_read = Cell::new(false);
        let result = protected_watch_identity("203.0.113.9:4010", || {
            token_read.set(true);
            Ok("must-not-leave".to_string())
        });

        assert!(matches!(result, Err(LaneError::NotLoopback { .. })));
        assert!(!token_read.get());
    }

    #[test]
    fn watch_carries_the_exact_numeric_loopback_address_and_persisted_identity() {
        let (bind, token) =
            protected_watch_identity("127.0.0.1:4010", || Ok("persisted".to_string()))
                .expect("loopback identity");

        assert_eq!(bind.to_string(), "127.0.0.1:4010");
        assert_eq!(token, "persisted");
    }
}
