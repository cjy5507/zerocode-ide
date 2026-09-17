//! One probe against a saved SSH target: can the system `ssh` reach it?
//!
//! This is the Test button's whole backend, and deliberately NOT a
//! connection. Orca's testConnection piggybacks on a live lifecycle when one
//! exists and otherwise runs a bare probe that changes no state; we have no
//! lifecycle yet, so this module is that bare probe alone — `-S none` keeps
//! it from minting a control master a later Connect would inherit by
//! accident, and nothing here is remembered past the reply.
//!
//! Recorded deviation: Orca answers interactive auth (passphrase dialogs)
//! during a test. This probe runs `BatchMode=yes` — a question is a failure —
//! because passphrases are memory-only and belong to the connection slice,
//! not to a probe that would have to carry them through IPC first.

use std::process::Stdio;

use crate::ssh_store::{DEFAULT_PORT, SshTarget};

/// TCP handshake ceiling, shared with the connection dial. Orca's connect
/// path leans on its library default; a subprocess `ssh` has no such default
/// worth waiting for — an unreachable address otherwise hangs a minute and
/// more on macOS while the card says "연결 중…".
pub(crate) const CONNECT_TIMEOUT_SECONDS: u32 = 10;
/// After the dial: two missed five-second heartbeats end a server that
/// accepted the socket and then went quiet, so the probe always comes home.
const ALIVE_INTERVAL_SECONDS: u32 = 5;
const ALIVE_COUNT_MAX: u32 = 2;
/// A card line, not a transcript. Character-safe cap on whatever stderr line
/// becomes the reason.
const MAX_REASON_CHARS: usize = 200;

/// The argv after `ssh`, in the order a person would type it. Ends with
/// `exit`: the only thing the remote shell is asked to do is leave.
pub(crate) fn probe_args(target: &SshTarget) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-T".into(),
        "-S".into(),
        "none".into(),
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        format!("ConnectTimeout={CONNECT_TIMEOUT_SECONDS}"),
        "-o".into(),
        format!("ServerAliveInterval={ALIVE_INTERVAL_SECONDS}"),
        "-o".into(),
        format!("ServerAliveCountMax={ALIVE_COUNT_MAX}"),
    ];
    args.extend(destination_args(target));
    args.push("exit".into());
    args
}

/// The half of the argv that says WHERE, shared with the connection this
/// probe is a rehearsal for. A `~/.ssh/config` alias travels ALONE — the
/// user's config outranks every field beside it (measured rule) — and a
/// manual target carries exactly the fields somebody filled in.
pub(crate) fn destination_args(target: &SshTarget) -> Vec<String> {
    if !target.config_host.is_empty() {
        return vec![target.config_host.clone()];
    }
    let mut args: Vec<String> = Vec::new();
    if target.port != DEFAULT_PORT {
        args.push("-p".into());
        args.push(target.port.to_string());
    }
    if !target.identity_file.is_empty() {
        args.push("-i".into());
        args.push(target.identity_file.clone());
    }
    if !target.jump_host.is_empty() {
        args.push("-J".into());
        args.push(target.jump_host.clone());
    }
    if !target.proxy_command.is_empty() {
        args.push("-o".into());
        args.push(format!("ProxyCommand={}", target.proxy_command));
    }
    if target.username.is_empty() {
        args.push(target.host.clone());
    } else {
        args.push(format!("{}@{}", target.username, target.host));
    }
    args
}

/// Whether stderr (already lowercased) reads as the server turning the KEY
/// away, as opposed to the road being closed. The connection lifecycle keys
/// its `auth-failed` state off this same judgement.
pub(crate) fn is_auth_refusal(spoken_lower: &str) -> bool {
    spoken_lower.contains("permission denied")
        || spoken_lower.contains("too many authentication failures")
}

/// Turn an exit and its stderr into the card's one sentence.
///
/// The known shapes of an `ssh` refusal each get a sentence a person can act
/// on; anything else answers with the last line ssh itself said. Nothing in
/// ssh's stderr is a secret — key PATHS at most — so the line may travel.
pub(crate) fn classify_probe(ok: bool, stderr: &str) -> Result<(), String> {
    if ok {
        return Ok(());
    }
    let spoken = stderr.to_lowercase();
    if is_auth_refusal(&spoken) {
        return Err("인증에 실패했습니다 — 사용자 이름과 키를 확인하세요".into());
    }
    if spoken.contains("timed out") {
        return Err("응답이 없습니다 — 주소와 네트워크를 확인하세요".into());
    }
    if spoken.contains("could not resolve hostname") {
        return Err("호스트 이름을 찾을 수 없습니다".into());
    }
    if spoken.contains("connection refused") {
        return Err("연결이 거부되었습니다 — 포트를 확인하세요".into());
    }
    if spoken.contains("host key verification failed") {
        return Err("호스트 키 확인이 필요합니다 — 터미널에서 한 번 접속해 승인하세요".into());
    }
    let reason = stderr
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("연결에 실패했습니다");
    Err(reason.chars().take(MAX_REASON_CHARS).collect())
}

/// Run the probe. Blocking on purpose — the caller wraps it in the same
/// blocking-task runner every other subprocess in this crate rides.
pub(crate) fn run_probe(target: &SshTarget) -> Result<(), String> {
    let output = crate::proc::quiet_command("ssh")
        .args(probe_args(target))
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
    classify_probe(
        output.status.success(),
        &String::from_utf8_lossy(&output.stderr),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manual() -> SshTarget {
        SshTarget {
            host: "box.example.com".into(),
            username: "kim".into(),
            ..SshTarget::default()
        }
    }

    #[test]
    fn an_alias_travels_alone() {
        let target = SshTarget {
            config_host: "bastion".into(),
            // Junk beside the alias, to prove none of it rides along.
            host: "ignored.example.com".into(),
            port: 2222,
            username: "kim".into(),
            identity_file: "~/.ssh/id_ed25519".into(),
            jump_host: "hop.example.com".into(),
            proxy_command: "nc %h %p".into(),
            ..SshTarget::default()
        };
        let args = probe_args(&target);
        assert_eq!(args.first().map(String::as_str), Some("-T"));
        assert_eq!(args[args.len() - 2], "bastion");
        assert_eq!(args.last().map(String::as_str), Some("exit"));
        for banned in ["-p", "-i", "-J"] {
            assert!(!args.contains(&banned.to_string()), "{banned} rode along");
        }
        assert!(!args.iter().any(|arg| arg.starts_with("ProxyCommand=")));
        assert!(!args.iter().any(|arg| arg.contains("ignored.example.com")));
    }

    #[test]
    fn a_manual_target_carries_its_fields() {
        let target = SshTarget {
            port: 2222,
            identity_file: "~/.ssh/id_ed25519".into(),
            jump_host: "hop.example.com".into(),
            proxy_command: "cloudflared access ssh --hostname %h".into(),
            ..manual()
        };
        let args = probe_args(&target);
        let at = |flag: &str| args.iter().position(|arg| arg == flag).expect(flag);
        assert_eq!(args[at("-p") + 1], "2222");
        assert_eq!(args[at("-i") + 1], "~/.ssh/id_ed25519");
        assert_eq!(args[at("-J") + 1], "hop.example.com");
        assert!(args.contains(&"ProxyCommand=cloudflared access ssh --hostname %h".to_string()));
        assert_eq!(args[args.len() - 2], "kim@box.example.com");
        assert_eq!(args.last().map(String::as_str), Some("exit"));
        // The probe never mints a control master.
        assert_eq!(args[at("-S") + 1], "none");
        assert!(args.contains(&"BatchMode=yes".to_string()));
    }

    #[test]
    fn port_22_and_empty_fields_stay_home() {
        let args = probe_args(&manual());
        assert!(!args.contains(&"-p".to_string()));
        assert!(!args.contains(&"-i".to_string()));
        assert!(!args.contains(&"-J".to_string()));
        assert!(!args.iter().any(|arg| arg.starts_with("ProxyCommand=")));
        assert_eq!(args[args.len() - 2], "kim@box.example.com");
        let bare = SshTarget {
            username: String::new(),
            ..manual()
        };
        let args = probe_args(&bare);
        assert_eq!(args[args.len() - 2], "box.example.com");
    }

    #[test]
    fn a_refusal_reads_as_its_reason() {
        assert_eq!(classify_probe(true, ""), Ok(()));
        let said = |stderr: &str| classify_probe(false, stderr).unwrap_err();
        assert!(said("kim@box: Permission denied (publickey).").contains("인증"));
        assert!(said("Too many authentication failures").contains("인증"));
        assert!(
            said("ssh: connect to host box port 22: Operation timed out")
                .contains("응답이 없습니다")
        );
        assert!(
            said("ssh: Could not resolve hostname box: nodename nor servname provided")
                .contains("호스트 이름")
        );
        assert!(said("ssh: connect to host box port 2222: Connection refused").contains("거부"));
        assert!(said("Host key verification failed.").contains("호스트 키"));
        // An unknown refusal answers with ssh's own last line, capped.
        assert_eq!(
            said("Warning: banner\nsomething strange happened\n"),
            "something strange happened"
        );
        let long = format!("x{}", "y".repeat(500));
        assert_eq!(said(&long).chars().count(), 200);
        assert_eq!(said(""), "연결에 실패했습니다");
    }
}
