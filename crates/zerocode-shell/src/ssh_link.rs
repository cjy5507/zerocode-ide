//! The connection half the probe rehearses for: a control master that
//! outlives its first command, and the state a card may claim about it.
//!
//! Orca keeps eight lifecycle states; this module stands seven — everything
//! but deploying-relay, which belongs to a relay nobody has built — and the
//! measured reconnection ladder that moves between them. What "connected" MEANS here is a live `ControlMaster`
//! socket: Orca's connected is a live relay process, and until we grow one,
//! the master the next remote terminal will ride is the honest equivalent.
//!
//! Recorded deviation: Orca wraps per-command reuse with `ControlPersist=300`.
//! Our master's lifetime is the target's own grace setting instead, because
//! grace IS the user's answer to "how long may a connection linger" — Orca
//! spends it on the relay's lifetime, we spend it on the master's.

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::process::Stdio;

use crate::ssh_probe::{CONNECT_TIMEOUT_SECONDS, destination_args, is_auth_refusal, probe_args};
use crate::ssh_store::SshTarget;

/// Measured from Orca's system-ssh transport (recon §C): the keep-alive pair
/// that notices a dead peer in under a minute without ever waking a healthy
/// one.
const ALIVE_INTERVAL_SECONDS: u32 = 15;
const ALIVE_COUNT_MAX: u32 = 3;
/// Half of Orca's measured ControlPath hash — sixteen hex characters keeps
/// the whole socket path far inside the POSIX `sun_path` ceiling.
const SOCKET_TAG_CHARS: usize = 16;

/// The measured reconnection ladder, in seconds: nine rungs and then the run
/// is over ("Max reconnection attempts reached" is Orca's own last word).
pub(crate) const RECONNECT_LADDER: [u64; 9] = [1, 2, 5, 5, 10, 10, 10, 30, 30];
/// Measured anti-flap: a NEW run's first wait never exceeds five seconds,
/// however high the carried ladder position says to climb.
const FIRST_RETRY_CAP_SECONDS: u64 = 5;
/// Measured stability: a connection that lived this long earns the ladder
/// back — the next fall starts from the bottom rung again.
pub(crate) const STABLE_AFTER_SECONDS: u64 = 60;

/// The states this slice can honestly claim, spelled exactly as Orca spells
/// them on the wire (`auth-failed`, `reconnection-failed`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum LinkState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
    ReconnectionFailed,
    AuthFailed,
    Error,
}

/// One card's whole connection story: the state, and the sentence when the
/// state is a refusal. This is both the store's record and the event payload.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct LinkReport {
    pub(crate) id: String,
    pub(crate) state: LinkState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
    /// Which reconnection attempt this is (1-based), and only while one is
    /// under way — the measured event carries the same fact.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) attempt: Option<u32>,
}

impl LinkReport {
    pub(crate) fn new(id: &str, state: LinkState) -> Self {
        Self {
            id: id.to_string(),
            state,
            error: None,
            attempt: None,
        }
    }
}

/// How a connection attempt fell, split the way the state machine needs it:
/// a turned-away key is `auth-failed`, every other closed road is `error`.
pub(crate) struct LinkRefusal {
    pub(crate) auth: bool,
    /// Whether the road may reopen on its own — the measured transient set.
    /// Only a transient fall is worth a ladder; auth and the rest are final.
    pub(crate) transient: bool,
    pub(crate) said: String,
}

/// Where this target's control socket lives. Hashed from the target id — not
/// the endpoint — so renaming a label or fixing a port never strands a live
/// socket under a name nobody will ask for again.
pub(crate) fn control_path(id: &str) -> PathBuf {
    let digest = Sha256::digest(id.as_bytes());
    let mut tag = String::with_capacity(SOCKET_TAG_CHARS);
    for byte in digest.iter().take(SOCKET_TAG_CHARS / 2) {
        tag.push_str(&format!("{byte:02x}"));
    }
    std::env::temp_dir().join(format!("zerocode-ssh-{tag}"))
}

/// The wait before reconnection attempt `rung` (0-based, carried across
/// short-lived runs — only sixty stable seconds reset it), or `None` when the
/// ladder is spent. The first wait of a fresh run is capped so a link that
/// flaps at the top of the ladder still answers quickly once.
pub(crate) fn reconnect_wait(rung: usize, first_of_run: bool) -> Option<u64> {
    let wait = RECONNECT_LADDER.get(rung).copied()?;
    Some(if first_of_run {
        wait.min(FIRST_RETRY_CAP_SECONDS)
    } else {
        wait
    })
}

/// The measured transient set, translated from errno names to the sentences
/// the system `ssh` actually prints for each one.
pub(crate) fn is_transient_fall(spoken_lower: &str) -> bool {
    [
        "timed out",                            // ETIMEDOUT
        "connection refused",                   // ECONNREFUSED
        "connection reset",                     // ECONNRESET
        "no route to host",                     // EHOSTUNREACH
        "network is unreachable",               // ENETUNREACH
        "temporary failure in name resolution", // EAI_AGAIN (glibc)
        "nodename nor servname provided",       // EAI_AGAIN (macOS)
    ]
    .iter()
    .any(|sign| spoken_lower.contains(sign))
}

/// How long the master may linger once its last session ends. The target's
/// grace period is exactly that question, and "keep alive until reset" is
/// ssh's own `yes`.
fn persist_word(target: &SshTarget) -> String {
    if target.relay_keep_alive_until_reset {
        "yes".to_string()
    } else {
        target.relay_grace_period_seconds.to_string()
    }
}

/// The argv that stands the master up: measured master options, then the same
/// destination the probe dialled, then `exit` — the session leaves, the
/// master stays.
pub(crate) fn link_open_args(target: &SshTarget, socket: &str) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-T".into(),
        "-o".into(),
        "ControlMaster=auto".into(),
        "-o".into(),
        format!("ControlPath={socket}"),
        "-o".into(),
        format!("ControlPersist={}", persist_word(target)),
        // The dial's own ceiling, the probe's same ten seconds. Without it a
        // dead address hangs on the OS default (a minute and more on macOS)
        // and the person watches "연결 중…" tell them nothing — measured live
        // on an unreachable intranet host, 2026-08-18.
        "-o".into(),
        format!("ConnectTimeout={CONNECT_TIMEOUT_SECONDS}"),
        "-o".into(),
        format!("ServerAliveInterval={ALIVE_INTERVAL_SECONDS}"),
        "-o".into(),
        format!("ServerAliveCountMax={ALIVE_COUNT_MAX}"),
        "-o".into(),
        "BatchMode=yes".into(),
    ];
    args.extend(destination_args(target));
    args.push("exit".into());
    args
}

/// `-O <word>` through the socket, aimed at the same destination for form's
/// sake: `exit` asks a standing master to leave, `check` asks if one stands.
fn socket_word_args(word: &str, target: &SshTarget, socket: &str) -> Vec<String> {
    let mut args: Vec<String> = vec!["-O".into(), word.into(), "-S".into(), socket.to_string()];
    args.extend(destination_args(target));
    args
}

pub(crate) fn link_close_args(target: &SshTarget, socket: &str) -> Vec<String> {
    socket_word_args("exit", target, socket)
}

pub(crate) fn link_check_args(target: &SshTarget, socket: &str) -> Vec<String> {
    socket_word_args("check", target, socket)
}

fn run_ssh(args: Vec<String>) -> Result<(bool, String), String> {
    let output = crate::proc::quiet_command("ssh")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                "ssh 명령을 찾을 수 없습니다".to_string()
            } else {
                format!("ssh를 실행하지 못했습니다: {err}")
            }
        })?;
    Ok((
        output.status.success(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    ))
}

/// Stand the connection up. With reuse on this mints the control master the
/// next remote terminal rides; with reuse off the target has asked for no
/// standing anything, so connected means "the road was just proven" and the
/// dial is the probe's own.
pub(crate) fn open_link(target: &SshTarget) -> Result<(), LinkRefusal> {
    let args = if target.system_ssh_connection_reuse {
        link_open_args(target, &control_path(&target.id).to_string_lossy())
    } else {
        probe_args(target)
    };
    let (ok, stderr) = run_ssh(args).map_err(|said| LinkRefusal {
        auth: false,
        transient: false,
        said,
    })?;
    if ok {
        return Ok(());
    }
    let spoken = stderr.to_lowercase();
    let said = crate::ssh_probe::classify_probe(false, &stderr)
        .expect_err("a failed dial always carries a sentence");
    Err(LinkRefusal {
        auth: is_auth_refusal(&spoken),
        transient: is_transient_fall(&spoken),
        said,
    })
}

/// Take the connection down. A master that already died is not an error —
/// the point of Disconnect is the state after it, not the errand.
pub(crate) fn close_link(target: &SshTarget) {
    if !target.system_ssh_connection_reuse {
        return;
    }
    let socket = control_path(&target.id);
    let _ = run_ssh(link_close_args(target, &socket.to_string_lossy()));
}

/// The argv a remote TERMINAL runs, program word first: `-t` forces the TTY,
/// the master options let it ride a standing master (or stand one for the
/// next tab), and there is deliberately no BatchMode — this PTY is the one
/// road where ssh's own password or passphrase question can reach the person
/// exactly as ssh asks it.
pub(crate) fn remote_term_args(target: &SshTarget) -> Vec<String> {
    let mut args: Vec<String> = vec!["ssh".into(), "-t".into()];
    if target.system_ssh_connection_reuse {
        let socket = control_path(&target.id);
        args.extend([
            "-o".into(),
            "ControlMaster=auto".into(),
            "-o".into(),
            format!("ControlPath={}", socket.to_string_lossy()),
            "-o".into(),
            format!("ControlPersist={}", persist_word(target)),
        ]);
    } else {
        // The target asked for no standing anything: every tab is its own
        // dial, and none of them may mint a master by accident.
        args.extend(["-S".into(), "none".into()]);
    }
    args.extend([
        "-o".into(),
        format!("ServerAliveInterval={ALIVE_INTERVAL_SECONDS}"),
        "-o".into(),
        format!("ServerAliveCountMax={ALIVE_COUNT_MAX}"),
    ]);
    args.extend(destination_args(target));
    args
}

/// Open the file subsystem through the same target and master as its terminal.
pub(crate) fn sftp_args(target: &SshTarget) -> Vec<String> {
    let mut args = vec![
        "-T".to_string(),
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        format!("ConnectTimeout={CONNECT_TIMEOUT_SECONDS}"),
    ];
    if target.system_ssh_connection_reuse {
        args.extend([
            "-S".into(),
            control_path(&target.id).to_string_lossy().into_owned(),
        ]);
    }
    args.push("-s".into());
    args.extend(destination_args(target));
    args.push("sftp".into());
    args
}

/// Whether this target's master still stands. `-O check` is local socket
/// talk, cheap enough to ask on a watch interval. Only meaningful with reuse
/// on — a reuse-off "connected" is a memory, and no probe here may demote it.
pub(crate) fn check_link(target: &SshTarget) -> bool {
    let socket = control_path(&target.id);
    matches!(
        run_ssh(link_check_args(target, &socket.to_string_lossy())),
        Ok((true, _))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manual() -> SshTarget {
        SshTarget {
            id: "ssh-abc123".into(),
            host: "box.example.com".into(),
            username: "kim".into(),
            ..SshTarget::default()
        }
    }

    #[test]
    fn sftp_reuses_the_targets_master_and_config_alias() {
        let target = SshTarget {
            id: "files-host".to_string(),
            config_host: "build-alias".to_string(),
            ..SshTarget::default()
        };
        let args = sftp_args(&target);
        assert!(
            args.windows(2)
                .any(|pair| pair == ["-S", &control_path(&target.id).to_string_lossy()])
        );
        assert!(args.ends_with(&["build-alias".to_string(), "sftp".to_string()]));
        assert!(args.contains(&"BatchMode=yes".to_string()));
    }

    #[test]
    fn the_socket_is_named_by_the_id_alone() {
        let one = control_path("ssh-abc123");
        let two = control_path("ssh-abc123");
        let other = control_path("ssh-zzz999");
        assert_eq!(one, two);
        assert_ne!(one, other);
        let name = one.file_name().unwrap().to_string_lossy().into_owned();
        let tag = name.strip_prefix("zerocode-ssh-").expect("prefix");
        assert_eq!(tag.len(), SOCKET_TAG_CHARS);
        assert!(tag.chars().all(|ch| ch.is_ascii_hexdigit()));
    }

    #[test]
    fn a_link_opens_as_a_master_and_lingers_for_grace() {
        let target = SshTarget {
            relay_keep_alive_until_reset: false,
            relay_grace_period_seconds: 3600,
            ..manual()
        };
        let args = link_open_args(&target, "/tmp/sock");
        assert!(args.contains(&"ControlMaster=auto".to_string()));
        assert!(args.contains(&"ControlPath=/tmp/sock".to_string()));
        assert!(args.contains(&"ControlPersist=3600".to_string()));
        assert!(args.contains(&"ServerAliveInterval=15".to_string()));
        assert!(args.contains(&"ServerAliveCountMax=3".to_string()));
        assert!(args.contains(&"BatchMode=yes".to_string()));
        // A dial must own a ceiling — the probe's measured ten seconds.
        assert!(args.contains(&"ConnectTimeout=10".to_string()));
        assert_eq!(args[args.len() - 2], "kim@box.example.com");
        assert_eq!(args.last().map(String::as_str), Some("exit"));
        // The probe's no-master flag has no place in a link.
        assert!(!args.contains(&"none".to_string()));
    }

    #[test]
    fn keep_alive_until_reset_is_ssh_yes() {
        let args = link_open_args(&manual(), "/tmp/sock");
        assert!(args.contains(&"ControlPersist=yes".to_string()));
    }

    #[test]
    fn an_alias_still_travels_alone_through_a_link() {
        let target = SshTarget {
            config_host: "bastion".into(),
            identity_file: "~/.ssh/id_ed25519".into(),
            ..manual()
        };
        let args = link_open_args(&target, "/tmp/sock");
        assert_eq!(args[args.len() - 2], "bastion");
        assert!(!args.contains(&"-i".to_string()));
    }

    #[test]
    fn disconnect_speaks_through_the_socket() {
        let args = link_close_args(&manual(), "/tmp/sock");
        assert_eq!(&args[..4], &["-O", "exit", "-S", "/tmp/sock"]);
        assert_eq!(args.last().map(String::as_str), Some("kim@box.example.com"));
    }

    #[test]
    fn the_states_spell_themselves_like_orca() {
        let spelled = |state: LinkState| serde_json::to_string(&state).unwrap();
        assert_eq!(spelled(LinkState::AuthFailed), "\"auth-failed\"");
        assert_eq!(spelled(LinkState::Disconnected), "\"disconnected\"");
        assert_eq!(
            spelled(LinkState::ReconnectionFailed),
            "\"reconnection-failed\""
        );
        let report = LinkReport::new("ssh-1", LinkState::Connected);
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains("\"connected\""));
        // A clean state drags neither a null error nor a null attempt along.
        assert!(!json.contains("error"));
        assert!(!json.contains("attempt"));
        let mid = LinkReport {
            attempt: Some(3),
            ..LinkReport::new("ssh-1", LinkState::Reconnecting)
        };
        let json = serde_json::to_string(&mid).unwrap();
        assert!(json.contains("\"reconnecting\"") && json.contains("\"attempt\":3"));
    }

    #[test]
    fn the_ladder_is_measured_and_the_first_wait_is_capped() {
        let climbed: Vec<u64> = (0..9)
            .map(|rung| reconnect_wait(rung, false).unwrap())
            .collect();
        // The measured values themselves, not the constant — a test that
        // compares the ladder to itself would bless any rewrite of it.
        assert_eq!(climbed, [1, 2, 5, 5, 10, 10, 10, 30, 30]);
        assert_eq!(reconnect_wait(9, false), None);
        assert_eq!(reconnect_wait(9, true), None);
        // Anti-flap: a fresh run answers within five seconds wherever the
        // carried position points, and a low rung keeps its own faster word.
        assert_eq!(reconnect_wait(7, true), Some(5));
        assert_eq!(reconnect_wait(4, true), Some(5));
        assert_eq!(reconnect_wait(0, true), Some(1));
    }

    #[test]
    fn only_the_measured_falls_are_transient() {
        for transient in [
            "connect to host box port 22: operation timed out",
            "connection refused",
            "connection reset by peer",
            "no route to host",
            "network is unreachable",
            "temporary failure in name resolution",
            "nodename nor servname provided, or not known",
        ] {
            assert!(is_transient_fall(transient), "{transient}");
        }
        for lasting in [
            "permission denied (publickey)",
            "host key verification failed",
            "too many authentication failures",
        ] {
            assert!(!is_transient_fall(lasting), "{lasting}");
        }
    }

    #[test]
    fn check_asks_through_the_socket() {
        let args = link_check_args(&manual(), "/tmp/sock");
        assert_eq!(&args[..4], &["-O", "check", "-S", "/tmp/sock"]);
        assert_eq!(args.last().map(String::as_str), Some("kim@box.example.com"));
    }

    #[test]
    fn a_remote_terminal_rides_the_master_and_may_be_asked_questions() {
        let args = remote_term_args(&manual());
        assert_eq!(&args[..2], &["ssh", "-t"]);
        assert!(args.contains(&"ControlMaster=auto".to_string()));
        assert!(args.iter().any(|arg| arg.starts_with("ControlPath=")));
        assert!(args.contains(&"ControlPersist=yes".to_string()));
        assert_eq!(args.last().map(String::as_str), Some("kim@box.example.com"));
        // A terminal is the one road ssh may ask on, and it runs a SHELL —
        // nothing here may close the door or hang up the session.
        assert!(!args.contains(&"BatchMode=yes".to_string()));
        assert!(!args.contains(&"exit".to_string()));
    }

    #[test]
    fn reuse_off_keeps_every_tab_its_own_dial() {
        let target = SshTarget {
            system_ssh_connection_reuse: false,
            ..manual()
        };
        let args = remote_term_args(&target);
        let at = args.iter().position(|arg| arg == "-S").expect("-S");
        assert_eq!(args[at + 1], "none");
        assert!(!args.contains(&"ControlMaster=auto".to_string()));
        assert!(!args.iter().any(|arg| arg.starts_with("ControlPersist=")));
    }
}
