//! The coding model a person picks from zo's own list (t-75762): what the
//! settings file holds after the pick, and — against the installed zo — what
//! the next zo session's first request carries.

use super::*;
use std::io::{Read, Write};

/// A zo list as `zo models --json` answers it: three providers' login
/// models, one of them superseded, and two routers. Names and ids are the
/// fixture's own; the window reads whatever zo says.
fn listed() -> Value {
    json!({
        "models": [
            {"id":"fixture-claude-top","provider":"claude","displayName":"Fixture Top","band":"top"},
            {"id":"fixture-claude-old","provider":"claude","displayName":"Fixture Old","band":"superseded","supersededBy":"fixture-claude-top"},
            {"id":"fixture-openai-one","provider":"openai","displayName":"Fixture One","band":"second"},
            {"id":"fixture-google-one","provider":"google","displayName":"Fixture Gem","band":null}
        ],
        "customProviders": [
            {"name":"Fixture","models":["Fixture/vendor/coding-model","Fixture/second-model"],"usable":true,"credentialEnvVars":[]},
            {"name":"Elsewhere","models":["Elsewhere/env-only-model"],"usable":true,"credentialEnvVars":[]}
        ]
    })
}

fn settings_with_router(path: &Path, model: &str) -> Value {
    let root = json!({
        "theme": "dark",
        "model": model,
        "providers": [{
            "name": "Fixture", "base_url": "https://gateway.example.test/v1",
            "models": ["vendor/coding-model", "second-model"], "requires_auth": false,
            "connection": {"id": "fixture-connection", "generation": 2}
        }]
    });
    std::fs::write(path, root.to_string()).expect("settings");
    root
}

fn read(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).expect("read")).expect("json")
}

#[test]
fn a_login_model_zo_lists_becomes_the_coding_model_without_a_connection() {
    let home = tempfile::tempdir().expect("home");
    let path = home.path().join("settings.json");
    settings_with_router(&path, "Fixture/vendor/coding-model");
    apply_coding(
        &path,
        &Reference {
            id: "fixture-connection".to_string(),
            generation: 2,
            model: "vendor/coding-model".to_string(),
            protocol: judgment_provider::Protocol::OpenaiJson,
        },
    )
    .expect("router coding");
    apply_coding_model(
        &path,
        "fixture-claude-top",
        Some("Fixture/vendor/coding-model"),
        &listed(),
    )
    .expect("login coding model");
    let root = read(&path);
    assert_eq!(root["model"], "fixture-claude-top");
    assert!(
        root["smart"]
            .get(connections::CODING_CONNECTION_KEY)
            .is_none(),
        "a login model kept the router's coding connection"
    );
    assert_eq!(
        root["smart"][connections::PREVIOUS_CODING_KEY],
        "Fixture/vendor/coding-model"
    );
    assert_eq!(root["theme"], "dark", "the pick touched another setting");
    let catalog = catalog(&path, &api_routers::HeldKeys::default()).expect("catalog");
    assert_eq!(catalog.coding_model.as_deref(), Some("fixture-claude-top"));
    assert!(catalog.coding.is_none());
}

#[test]
fn a_superseded_login_model_is_still_a_model_zo_lists() {
    let home = tempfile::tempdir().expect("home");
    let path = home.path().join("settings.json");
    settings_with_router(&path, "fixture-claude-top");
    apply_coding_model(
        &path,
        "fixture-claude-old",
        Some("fixture-claude-top"),
        &listed(),
    )
    .expect("an older generation is a choice, not a hidden one");
    assert_eq!(read(&path)["model"], "fixture-claude-old");
}

#[test]
fn a_router_model_zo_lists_goes_through_its_saved_connection() {
    let home = tempfile::tempdir().expect("home");
    let path = home.path().join("settings.json");
    settings_with_router(&path, "fixture-claude-top");
    apply_coding_model(
        &path,
        "Fixture/second-model",
        Some("fixture-claude-top"),
        &listed(),
    )
    .expect("router model");
    let root = read(&path);
    assert_eq!(root["model"], "Fixture/second-model");
    assert_eq!(
        root["smart"][connections::CODING_CONNECTION_KEY]["id"],
        "fixture-connection"
    );
    assert_eq!(
        root["smart"][connections::CODING_CONNECTION_KEY]["model"],
        "second-model"
    );
    assert_eq!(
        root["smart"][connections::PREVIOUS_CODING_KEY],
        "fixture-claude-top"
    );
}

#[test]
fn a_router_model_zo_reads_from_outside_the_settings_is_written_as_zo_names_it() {
    let home = tempfile::tempdir().expect("home");
    let path = home.path().join("settings.json");
    settings_with_router(&path, "fixture-claude-top");
    apply_coding_model(
        &path,
        "Elsewhere/env-only-model",
        Some("fixture-claude-top"),
        &listed(),
    )
    .expect("a router zo knows from its own environment");
    let root = read(&path);
    assert_eq!(root["model"], "Elsewhere/env-only-model");
    assert!(
        root["smart"]
            .get(connections::CODING_CONNECTION_KEY)
            .is_none()
    );
}

#[test]
fn a_model_zo_does_not_list_or_a_moved_selection_leaves_the_file_unchanged() {
    let home = tempfile::tempdir().expect("home");
    let path = home.path().join("settings.json");
    settings_with_router(&path, "fixture-claude-top");
    let before = std::fs::read(&path).expect("before");
    assert_eq!(
        apply_coding_model(
            &path,
            "not-a-listed-model",
            Some("fixture-claude-top"),
            &listed()
        )
        .unwrap_err(),
        "provider_model"
    );
    assert_eq!(
        apply_coding_model(&path, "fixture-openai-one", Some("someone-else"), &listed())
            .unwrap_err(),
        "settings_changed"
    );
    assert_eq!(
        apply_coding_model(
            &path,
            "fixture-openai-one",
            Some("fixture-claude-top"),
            &Value::Null
        )
        .unwrap_err(),
        "provider_model",
        "with no zo list there is nothing to check a pick against"
    );
    assert_eq!(std::fs::read(&path).expect("after"), before);
}

/// A loopback server that records the first request carrying a turn and
/// answers it 400, which zo does not retry. Nothing else listens: the zo it
/// talks to runs with a cleared environment, a temporary HOME and the
/// keychain switched off, so no setting, session or credential of the
/// person's is read, copied or written.
struct FirstRequest {
    addr: std::net::SocketAddr,
    seen: std::sync::mpsc::Receiver<String>,
}

impl FirstRequest {
    fn listen() -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let (tell, seen) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
                let mut bytes = Vec::new();
                let mut buffer = [0_u8; 65536];
                loop {
                    let Ok(read) = stream.read(&mut buffer) else {
                        break;
                    };
                    if read == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&buffer[..read]);
                    let text = String::from_utf8_lossy(&bytes);
                    if let Some((head, body)) = text.split_once("\r\n\r\n") {
                        let length = head
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().ok())?
                            })
                            .unwrap_or(0);
                        if body.len() >= length {
                            break;
                        }
                    }
                }
                // A model list a session asks for on its way up is answered
                // empty; the first request that carries a turn is the one
                // recorded.
                if bytes.starts_with(b"GET ") {
                    let empty = "{\"data\":[]}";
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{empty}",
                        empty.len()
                    );
                    continue;
                }
                let answer =
                    "{\"error\":{\"type\":\"invalid_request_error\",\"message\":\"fixture\"}}";
                let _ = write!(
                    stream,
                    "HTTP/1.1 400 Bad Request\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{answer}",
                    answer.len()
                );
                if tell
                    .send(String::from_utf8_lossy(&bytes).into_owned())
                    .is_err()
                {
                    return;
                }
            }
        });
        Self { addr, seen }
    }

    fn base(&self) -> String {
        format!("http://{}", self.addr)
    }
}

/// The zo this machine runs, when the suite is asked to reach it.
fn installed_zo() -> std::path::PathBuf {
    std::env::var_os("ZO_E2E_BIN")
        .map(std::path::PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".local/bin/zo")))
        .filter(|bin| bin.exists())
        .expect("ZO_E2E_BIN or ~/.local/bin/zo")
}

/// A new zo session started in `home` with nothing but the settings the
/// window wrote: its first request, as the server read it.
fn first_request_of_a_new_session(
    home: &Path,
    extra: &[(&str, String)],
    server: &FirstRequest,
) -> (String, Value) {
    // The child's whole environment is this list: the window's own door for
    // a child with an environment of its own (`proc::quiet_command_in_env`).
    let mut env: Vec<(String, String)> = vec![
        ("HOME".to_string(), home.to_string_lossy().into_owned()),
        ("TERM".to_string(), "dumb".to_string()),
        ("ZO_DISABLE_KEYCHAIN".to_string(), "1".to_string()),
    ];
    env.extend(
        extra
            .iter()
            .map(|(name, value)| ((*name).to_string(), value.clone())),
    );
    let mut command = crate::proc::quiet_command_in_env(installed_zo(), &env);
    command
        .arg("-p")
        .current_dir(home)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let mut child = command.spawn().expect("zo starts");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(b"say hi\n")
        .expect("prompt");
    let request = server
        .seen
        .recv_timeout(std::time::Duration::from_secs(60))
        .expect("the new session sent a request");
    let _ = child.kill();
    let _ = child.wait();
    let (head, body) = request.split_once("\r\n\r\n").expect("request");
    (
        head.lines().next().unwrap_or_default().to_string(),
        serde_json::from_str(body).expect("JSON body"),
    )
}

/// t-75762 시험 1: a router model picked in the window is the model field of
/// the next new zo session's first request, sent to that router.
#[test]
#[ignore = "runs the installed zo; `cargo test -- --ignored the_next_zo_session`"]
fn the_next_zo_session_sends_a_picked_router_model_to_its_router() {
    let server = FirstRequest::listen();
    let home = tempfile::tempdir().expect("home");
    std::fs::create_dir_all(home.path().join(".zo")).expect(".zo");
    let path = home.path().join(".zo/settings.json");
    std::fs::write(
        &path,
        json!({"model":"fixture-claude-top","providers":[{
            "name":"Fixture","base_url":format!("{}/v1", server.base()),
            "models":["vendor/coding-model","second-model"],"requires_auth":false,
            "connection":{"id":"fixture-connection","generation":2}
        }]})
        .to_string(),
    )
    .expect("settings");
    apply_coding_model(
        &path,
        "Fixture/second-model",
        Some("fixture-claude-top"),
        &listed(),
    )
    .expect("pick");
    let (line, body) = first_request_of_a_new_session(home.path(), &[], &server);
    assert!(line.starts_with("POST /v1/chat/completions "), "{line}");
    assert_eq!(body["model"], "second-model");
}

/// t-75762 시험 2: a login model picked in the window is the model field of
/// the next new zo session's first request — caught before it leaves the
/// machine by pointing the provider's own address at the fixture.
#[test]
#[ignore = "runs the installed zo; `cargo test -- --ignored the_next_zo_session`"]
fn the_next_zo_session_sends_a_picked_login_model() {
    let server = FirstRequest::listen();
    let home = tempfile::tempdir().expect("home");
    std::fs::create_dir_all(home.path().join(".zo")).expect(".zo");
    let path = home.path().join(".zo/settings.json");
    std::fs::write(&path, json!({"model":"Fixture/second-model"}).to_string()).expect("settings");
    let mut zo_list = listed();
    zo_list["models"]
        .as_array_mut()
        .expect("models")
        .push(json!({"id":"claude-opus-5-5","provider":"claude","displayName":"Fixture Opus","band":"second"}));
    apply_coding_model(
        &path,
        "claude-opus-5-5",
        Some("Fixture/second-model"),
        &zo_list,
    )
    .expect("pick");
    let (line, body) = first_request_of_a_new_session(
        home.path(),
        &[
            ("ANTHROPIC_BASE_URL", server.base()),
            ("ANTHROPIC_API_KEY", "[합성-키]".to_string()),
        ],
        &server,
    );
    assert!(line.starts_with("POST /v1/messages"), "{line}");
    assert_eq!(body["model"], "claude-opus-5-5");
}

/* ---- a removed managed model and the selection judgment returns to (t-88166) */

const REMOVED_MODEL: &str = "fixture-removed-model";

/// Settings as a connection left them: judgment on a saved connection, with
/// the selection it replaced kept for 「연결 해제」. That selection names
/// `model` by a managed binding, unless `model` is `None`: a server of the
/// person's own, which no binding names.
fn settings_returning_to(path: &Path, model: Option<&str>) -> Value {
    let previous_provider = json!({
        "kind": "local", "protocol": "system_one", "baseUrl": "http://127.0.0.1:9",
        "model": model.unwrap_or("persons-own-server-model"), "requiresAuth": false,
    });
    let binding = model.map_or(Value::Null, |model| {
        json!({"modelId": model, "revision": "0".repeat(40), "providerIdentity": "fixture-identity"})
    });
    let root = json!({
        "theme": "dark",
        "providers": [{
            "name": "Fixture", "base_url": "https://gateway.example.test/v1", "models": ["judge"],
            "requires_auth": true, "auth_env": "ZEROCODE_ROUTER_FIXTURE_KEY",
            "connection": {"id": "fixture-connection", "generation": 2}
        }],
        "smart": {
            (judgment_provider::SETTINGS_KEY): {
                "kind": "remote", "protocol": "system_one", "baseUrl": "https://gateway.example.test/v1",
                "model": "judge", "requiresAuth": true,
            },
            (connections::JUDGMENT_CONNECTION_KEY): {
                "id": "fixture-connection", "generation": 2, "model": "judge", "protocol": "system_one",
            },
            (connections::PREVIOUS_JUDGMENT_KEY): {
                (judgment_provider::SETTINGS_KEY): previous_provider,
                "jevModel": "fixture-jev-pin",
                (local_judgment::MANAGED_MODEL_KEY): binding,
            },
        }
    });
    std::fs::write(path, root.to_string()).expect("settings");
    root
}

fn previous_of(root: &Value) -> &Value {
    &root["smart"][connections::PREVIOUS_JUDGMENT_KEY]
}

#[test]
fn removing_a_model_takes_it_out_of_the_selection_judgment_returns_to() {
    let home = tempfile::tempdir().expect("home");
    let path = home.path().join("settings.json");
    let before = settings_returning_to(&path, Some(REMOVED_MODEL));
    let identity = judgment_provider::Provider::from_root(&before)
        .expect("provider")
        .identity();
    removing_managed_model(&path, REMOVED_MODEL, || Ok(())).expect("removed");
    let after = read(&path);
    let previous = previous_of(&after);
    assert!(
        previous
            .get(judgment_provider::SETTINGS_KEY)
            .is_none_or(Value::is_null),
        "the removed model's provider stayed: {previous}"
    );
    assert!(
        previous
            .get(local_judgment::MANAGED_MODEL_KEY)
            .is_none_or(Value::is_null),
        "the removed model's binding stayed: {previous}"
    );
    // Everything else is as it stood: above all what the evidence folders are
    // named from, and the connection's own row and key.
    assert_eq!(previous["jevModel"], "fixture-jev-pin");
    assert_eq!(after["providers"], before["providers"]);
    assert_eq!(after["theme"], before["theme"]);
    for key in [
        judgment_provider::SETTINGS_KEY,
        connections::JUDGMENT_CONNECTION_KEY,
    ] {
        assert_eq!(after["smart"][key], before["smart"][key], "{key}");
    }
    assert_eq!(
        judgment_provider::Provider::from_root(&after)
            .expect("provider")
            .identity(),
        identity
    );
}

#[test]
fn a_selection_that_names_another_model_or_a_persons_own_server_is_left_unwritten() {
    for (held, removed) in [
        (Some("fixture-other-model"), REMOVED_MODEL),
        (None, "persons-own-server-model"),
    ] {
        let home = tempfile::tempdir().expect("home");
        let path = home.path().join("settings.json");
        settings_returning_to(&path, held);
        let bytes = std::fs::read(&path).expect("read");
        removing_managed_model(&path, removed, || Ok(())).expect("removed");
        assert_eq!(
            std::fs::read(&path).expect("read"),
            bytes,
            "{held:?}: a selection this removal does not touch was written"
        );
    }
}

#[test]
fn a_removal_the_store_refused_leaves_the_selection_as_it_stood() {
    let home = tempfile::tempdir().expect("home");
    let path = home.path().join("settings.json");
    settings_returning_to(&path, Some(REMOVED_MODEL));
    let bytes = std::fs::read(&path).expect("read");
    assert_eq!(
        removing_managed_model(&path, REMOVED_MODEL, || Err("model_configured".to_string())),
        Err("model_configured".to_string())
    );
    assert_eq!(std::fs::read(&path).expect("read"), bytes);
}

#[test]
fn clearing_judgment_does_not_bring_back_a_model_this_computer_no_longer_holds() {
    let home = tempfile::tempdir().expect("home");
    let path = home.path().join("settings.json");
    let before = settings_returning_to(&path, Some(REMOVED_MODEL));
    let reference: Reference =
        serde_json::from_value(before["smart"][connections::JUDGMENT_CONNECTION_KEY].clone())
            .expect("reference");
    let mut asked = std::cell::RefCell::new(Vec::new());
    let holds = |binding: &local_judgment::Binding| {
        asked.borrow_mut().push(binding.model_id.clone());
        false
    };
    clear(&path, Role::Judgment, Some(&reference), &holds).expect("cleared");
    assert_eq!(asked.get_mut().as_slice(), [REMOVED_MODEL.to_string()]);
    let after = read(&path);
    let smart = &after["smart"];
    assert!(
        smart.get(judgment_provider::SETTINGS_KEY).is_none(),
        "a model with no files came back as judgment's provider: {smart}"
    );
    assert!(smart.get(local_judgment::MANAGED_MODEL_KEY).is_none());
    assert_eq!(
        smart["jevModel"], "fixture-jev-pin",
        "the pin is no part of the model"
    );
    assert!(smart.get(connections::JUDGMENT_CONNECTION_KEY).is_none());
    assert!(smart.get(connections::PREVIOUS_JUDGMENT_KEY).is_none());
    assert_eq!(
        after["providers"], before["providers"],
        "the connection and its key name stay"
    );
    assert_eq!(
        judgment_provider::Provider::from_root(&after)
            .expect("the default")
            .kind,
        judgment_provider::ProviderKind::Jev,
        "judgment goes back to the default"
    );
}

#[test]
fn clearing_judgment_brings_back_a_model_still_held_and_a_persons_own_server() {
    for (model, reads_holds) in [(Some(REMOVED_MODEL), true), (None, false)] {
        let home = tempfile::tempdir().expect("home");
        let path = home.path().join("settings.json");
        let before = settings_returning_to(&path, model);
        let reference: Reference =
            serde_json::from_value(before["smart"][connections::JUDGMENT_CONNECTION_KEY].clone())
                .expect("reference");
        let read_holds = std::cell::Cell::new(false);
        let holds = |_: &local_judgment::Binding| {
            read_holds.set(true);
            true
        };
        clear(&path, Role::Judgment, Some(&reference), &holds).expect("cleared");
        assert_eq!(read_holds.get(), reads_holds, "{model:?}");
        let after = read(&path);
        assert_eq!(
            after["smart"][judgment_provider::SETTINGS_KEY],
            previous_of(&before)[judgment_provider::SETTINGS_KEY],
            "{model:?}: the selection did not come back"
        );
        assert_eq!(
            after["smart"]
                .get(local_judgment::MANAGED_MODEL_KEY)
                .cloned()
                .unwrap_or(Value::Null),
            previous_of(&before)[local_judgment::MANAGED_MODEL_KEY],
            "{model:?}"
        );
    }
}

#[test]
fn clearing_after_the_removal_cleaned_the_selection_goes_back_to_the_default() {
    let home = tempfile::tempdir().expect("home");
    let path = home.path().join("settings.json");
    let before = settings_returning_to(&path, Some(REMOVED_MODEL));
    let reference: Reference =
        serde_json::from_value(before["smart"][connections::JUDGMENT_CONNECTION_KEY].clone())
            .expect("reference");
    removing_managed_model(&path, REMOVED_MODEL, || Ok(())).expect("removed");
    clear(&path, Role::Judgment, Some(&reference), &|_| true).expect("cleared");
    assert_eq!(
        judgment_provider::Provider::from_root(&read(&path))
            .expect("the default")
            .kind,
        judgment_provider::ProviderKind::Jev
    );
}
