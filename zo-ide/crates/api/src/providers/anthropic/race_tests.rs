//! Two holders of one rotating Claude login, raced against a mock token
//! endpoint (t-11045) — the measurement behind the change, kept as its
//! regression.
//!
//! A window pane of zo reads its Claude login while the pane's own CLI — and
//! every other pane of the account — renews the same login. The endpoint
//! accepts a refresh token once and answers `invalid_grant` for it ever after.
//! Two orders, each run `TRIALS` times on a fresh login:
//!
//! * the CLI renews first: zo must still find a usable login — before
//!   t-11045 it read the window's copy, spent the copy's superseded refresh
//!   token and ended with "Claude auth unavailable";
//! * zo needs the login first: the CLI must still hold a live login
//!   afterwards — before t-11045 zo spent the refresh token itself and wrote
//!   the next one to the copy, so the CLI's own next renewal was refused.
//!
//! Nothing here reaches a real endpoint or a real store: the endpoint is a
//! thread on the loopback, the stores are temp folders, the keychain is off.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::{Arc, Mutex};

use super::now_unix_timestamp;

/// How many fresh logins each order is raced on.
const TRIALS: usize = 20;

/// The access tokens the endpoint mints live this long, in seconds.
const MINTED_LIFETIME_SECS: u64 = 3_600;

/// A token endpoint that rotates: each family's current refresh token is
/// accepted once, and any other is `invalid_grant`.
struct RotatingEndpoint {
    url: String,
    current: Arc<Mutex<HashMap<String, (String, u32)>>>,
    refused: Arc<Mutex<usize>>,
}

impl RotatingEndpoint {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the endpoint");
        let url = format!("http://{}/v1/oauth/token", listener.local_addr().expect("addr"));
        let current: Arc<Mutex<HashMap<String, (String, u32)>>> = Arc::default();
        let refused: Arc<Mutex<usize>> = Arc::default();
        let (held, count) = (Arc::clone(&current), Arc::clone(&refused));
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                answer(stream, &held, &count);
            }
        });
        Self { url, current, refused }
    }

    /// A new login: `family`'s first refresh token.
    fn seed(&self, family: &str) -> String {
        let token = format!("{family}-r0");
        self.current
            .lock()
            .expect("families")
            .insert(family.to_string(), (token.clone(), 0));
        token
    }

    fn refused(&self) -> usize {
        *self.refused.lock().expect("count")
    }
}

/// One request, answered: rotate a current refresh token, refuse any other.
fn answer(mut stream: TcpStream, current: &Mutex<HashMap<String, (String, u32)>>, refused: &Mutex<usize>) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
    let mut length = 0;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
            break;
        }
        if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0; length];
    let _ = reader.read_exact(&mut body);
    let body = String::from_utf8_lossy(&body).into_owned();
    let spent = refresh_token_in(&body).unwrap_or_default();
    let family = spent.rsplit_once("-r").map(|(family, _)| family.to_string()).unwrap_or_default();
    let mut held = current.lock().expect("families");
    let (status, reply) = match held.get(&family) {
        Some((token, generation)) if *token == spent => {
            let next = generation + 1;
            let refresh = format!("{family}-r{next}");
            held.insert(family.clone(), (refresh.clone(), next));
            (
                "200 OK",
                serde_json::json!({
                    "access_token": format!("{family}-a{next}"),
                    "refresh_token": refresh,
                    "expires_in": MINTED_LIFETIME_SECS,
                    "scope": "user:inference user:profile",
                })
                .to_string(),
            )
        }
        _ => {
            *refused.lock().expect("count") += 1;
            (
                "400 Bad Request",
                r#"{"error": "invalid_grant", "error_description": "Refresh token not found or invalid"}"#.to_string(),
            )
        }
    };
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
        reply.len()
    );
}

/// `refresh_token` out of a form or JSON body.
fn refresh_token_in(body: &str) -> Option<String> {
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(body) {
        return json.get("refresh_token").and_then(|token| token.as_str()).map(str::to_string);
    }
    body.split('&').find_map(|pair| {
        pair.strip_prefix("refresh_token=").map(|token| token.replace("%2D", "-"))
    })
}

/// A login blob as the CLI keeps it.
fn blob(access: &str, refresh: &str, expires_at_ms: u64) -> String {
    serde_json::json!({"claudeAiOauth": {
        "accessToken": access,
        "refreshToken": refresh,
        "expiresAt": expires_at_ms,
        "scopes": ["user:inference", "user:profile"],
        "subscriptionType": "max",
    }})
    .to_string()
}

fn store_of(folder: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(folder.join(".credentials.json")).expect("store"))
        .expect("a document")
}

/// What the Claude Code CLI does with its own store, as measured on 2.1.283:
/// under its lock, read the store again; renew only a token within five
/// minutes of expiry; write the new pair back. `Err` is the endpoint refusing.
fn cli_renews(folder: &Path, endpoint: &RotatingEndpoint, lock: &Mutex<()>) -> Result<(), String> {
    let _held = lock.lock().expect("the CLI's refresh lock");
    let oauth = store_of(folder)["claudeAiOauth"].clone();
    let expires_at = oauth["expiresAt"].as_u64().unwrap_or(0);
    if now_unix_timestamp() * 1_000 + 300_000 < expires_at {
        return Ok(());
    }
    let refresh = oauth["refreshToken"].as_str().unwrap_or_default().to_string();
    let form = format!("grant_type=refresh_token&refresh_token={refresh}");
    let url = endpoint.url.trim_start_matches("http://");
    let (host, _) = url.split_once('/').expect("a path");
    let mut stream = TcpStream::connect(host).expect("reach the endpoint");
    write!(
        stream,
        "POST /v1/oauth/token HTTP/1.1\r\nhost: {host}\r\ncontent-type: application/x-www-form-urlencoded\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{form}",
        form.len()
    )
    .expect("send");
    let mut reply = String::new();
    stream.read_to_string(&mut reply).expect("read");
    let (head, body) = reply.split_once("\r\n\r\n").expect("an answer");
    if !head.starts_with("HTTP/1.1 200") {
        return Err(body.to_string());
    }
    let minted: serde_json::Value = serde_json::from_str(body).expect("tokens");
    std::fs::write(
        folder.join(".credentials.json"),
        blob(
            minted["access_token"].as_str().expect("access"),
            minted["refresh_token"].as_str().expect("refresh"),
            (now_unix_timestamp() + MINTED_LIFETIME_SECS) * 1_000,
        ),
    )
    .expect("the CLI writes its store");
    Ok(())
}

/// One trial's two folders: the account's own (the CLI's store) and the
/// window's runtime home, holding the copy the window wrote at the switch.
struct Pane {
    account: tempfile::TempDir,
    runtime_home: tempfile::TempDir,
}

impl Pane {
    fn switched_to(family: &str, endpoint: &RotatingEndpoint) -> Self {
        let first = endpoint.seed(family);
        let expired = blob(&format!("{family}-a0"), &first, 1_000);
        let pane = Self {
            account: tempfile::tempdir().expect("account folder"),
            runtime_home: tempfile::tempdir().expect("runtime home"),
        };
        std::fs::write(pane.account.path().join(".credentials.json"), &expired).expect("the account");
        std::fs::write(pane.runtime_home.path().join(".credentials.json"), &expired).expect("the copy");
        pane
    }
}

/// The environment a window pane of zo runs in, over one trial's folders.
struct PaneEnv(Vec<(&'static str, Option<std::ffi::OsString>)>);

impl PaneEnv {
    fn enter(pane: &Pane) -> Self {
        let names = ["CLAUDE_CONFIG_DIR", "CLAUDE_SECURESTORAGE_CONFIG_DIR", "ZO_DISABLE_KEYCHAIN"];
        let saved = names.iter().map(|name| (*name, std::env::var_os(name))).collect();
        std::env::set_var("CLAUDE_CONFIG_DIR", pane.runtime_home.path());
        std::env::set_var("CLAUDE_SECURESTORAGE_CONFIG_DIR", pane.account.path());
        std::env::set_var("ZO_DISABLE_KEYCHAIN", "1");
        crate::managed_account::clear();
        super::keychain::invalidate_claude_code_keychain_cache();
        Self(saved)
    }
}

impl Drop for PaneEnv {
    fn drop(&mut self) {
        for (name, value) in &self.0 {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
        crate::managed_account::clear();
    }
}

/// The CLI renews first; zo then needs the login.
fn zo_after_the_cli(endpoint: &RotatingEndpoint, lock: &Arc<Mutex<()>>, trial: usize) -> bool {
    let pane = Pane::switched_to(&format!("cli-first-{trial}"), endpoint);
    cli_renews(pane.account.path(), endpoint, lock).expect("the CLI renews its own login");
    let _env = PaneEnv::enter(&pane);
    super::resolve_claude_auth_fresh_explained().is_ok()
}

/// zo needs the login first; the CLI then renews its own.
fn cli_after_zo(endpoint: &RotatingEndpoint, lock: &Arc<Mutex<()>>, trial: usize) -> bool {
    let pane = Pane::switched_to(&format!("zo-first-{trial}"), endpoint);
    {
        let _env = PaneEnv::enter(&pane);
        let _ = super::resolve_claude_auth_fresh_explained();
    }
    cli_renews(pane.account.path(), endpoint, lock).is_ok()
}

/// The renewer zo asks: the CLI's own renewal of the store it names.
fn install_the_cli_as_renewer(endpoint: &Arc<RotatingEndpoint>, lock: &Arc<Mutex<()>>) {
    let (endpoint, lock) = (Arc::clone(endpoint), Arc::clone(lock));
    super::keychain::install_claude_login_renewer(Some(Arc::new(move |renewal| {
        let folder = renewal.folder.clone().expect("a pane's login is scoped");
        match cli_renews(&folder, &endpoint, &lock) {
            Ok(()) | Err(_) => super::keychain::RenewalRun::Ran,
        }
    })));
}

#[test]
fn a_pane_and_its_cli_share_one_login_without_either_losing_it() {
    let _guard = crate::test_env_lock();
    let _isolation = crate::test_env::CredentialEnvIsolation::empty();
    let endpoint = Arc::new(RotatingEndpoint::start());
    let lock = Arc::new(Mutex::new(()));
    install_the_cli_as_renewer(&endpoint, &lock);

    let zo_lost = (0..TRIALS).filter(|trial| !zo_after_the_cli(&endpoint, &lock, *trial)).count();
    let cli_lost = (0..TRIALS).filter(|trial| !cli_after_zo(&endpoint, &lock, *trial)).count();
    super::keychain::install_claude_login_renewer(None);
    eprintln!(
        "[t-11045 race] trials={TRIALS} zo_auth_unavailable={zo_lost} cli_login_lost={cli_lost} endpoint_refused={}",
        endpoint.refused()
    );
    assert_eq!(zo_lost, 0, "zo lost the login the pane's CLI had renewed");
    assert_eq!(cli_lost, 0, "the pane's CLI lost the login zo had needed");
    assert_eq!(endpoint.refused(), 0, "a refresh token was spent twice");
}
