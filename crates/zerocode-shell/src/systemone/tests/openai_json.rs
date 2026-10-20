use super::*;
use zerocode_core::jev::JEV_USES;

fn provider(base: &str) -> Provider {
    let parsed = Provider::from_root(&json!({"smart":{"judgmentProvider":{
        "kind":"local","protocol":"openai_json","baseUrl":base,
        "model":"acme/decision","requiresAuth":false
    }}}));
    assert!(
        parsed.is_ok(),
        "local structured provider was refused: {parsed:?}"
    );
    parsed.expect("asserted provider")
}

fn questions() -> Value {
    json!({"ready":{"type":"noul","instructions":"Is the page ready?"}})
}

fn response() -> String {
    json!({"model":"acme/decision","choices":[{"index":0,"finish_reason":"stop",
        "message":{"role":"assistant","content":"{\"answers\":{\"ready\":{\"type\":\"noul\",\"noul\":0.95}}}"}}],
        "usage":{"prompt_tokens":10,"completion_tokens":8}}).to_string()
}

fn settings(provider: &Provider, workspace: &Path) -> Value {
    let mut root = json!({"smart":{"judgmentProvider":provider,
        "jev":{"enabled":true,"workspaces":[door::resolved_path(workspace)]}}});
    for seat in JEV_USES {
        root["smart"][seat.setting] = json!(
            seat.modes
                .iter()
                .find(|mode| mode.asks())
                .expect("asking mode")
                .key()
        );
    }
    root
}

#[test]
fn all_seats_send_structured_requests_through_the_existing_window_wire() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let path = home.path().join("settings.json");
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", response(), 0);
    let provider = provider(&endpoint.base());
    std::fs::write(&path, settings(&provider, workspace.path()).to_string()).expect("settings");
    let wire = Wire::at("http://127.0.0.1:1", "", Some(path));
    for seat in JEV_USES {
        let asked = wire.ask(
            &seat,
            Some(workspace.path()),
            request_body(&json!("Ready"), &questions()),
            Duration::from_secs(5),
        );
        assert!(asked.answer.is_ok(), "{}: {:?}", seat.id, asked.answer);
        assert_eq!(asked.spent.requests, 1);
        assert_eq!(
            asked.spent.model,
            Some(format!("{}:acme/decision", provider.identity()))
        );
    }
    let seen = endpoint.asked();
    assert_eq!(seen.len(), JEV_USES.len());
    for request in seen {
        let (head, body) = request.split_once("\r\n\r\n").expect("request");
        assert!(head.starts_with("POST /v1/chat/completions "));
        assert!(!head.to_lowercase().contains("authorization:"));
        let body: Value = serde_json::from_str(body).expect("JSON");
        assert_eq!(body["response_format"]["type"], "json_schema");
        assert_eq!(body["model"], provider.model);
    }
}

#[test]
fn structured_requests_preserve_the_off_and_consent_guards() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let path = home.path().join("settings.json");
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", response(), 0);
    let provider = provider(&endpoint.base());
    let mut root = settings(&provider, workspace.path());
    let wire = Wire::at("http://127.0.0.1:1", "", Some(path.clone()));
    for enabled in [false, true] {
        root["smart"]["jev"] = json!({"enabled":enabled,"workspaces":[]});
        std::fs::write(&path, root.to_string()).expect("settings");
        let asked = wire.ask(
            &zerocode_core::jev::BROWSER,
            Some(workspace.path()),
            request_body(&json!("Ready"), &questions()),
            Duration::from_secs(1),
        );
        assert!(asked.answer.is_err());
        assert_eq!(asked.spent.requests, 0);
    }
    assert!(endpoint.asked().is_empty());
}

#[test]
fn malformed_structured_answers_keep_schema_refusal_and_separate_evidence() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let path = home.path().join("settings.json");
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", "{\"choices\":[]}".to_string(), 0);
    let structured = provider(&endpoint.base());
    let mut root = settings(&structured, workspace.path());
    std::fs::write(&path, root.to_string()).expect("settings");
    let wire = Wire::at("http://127.0.0.1:1", "", Some(path.clone()));
    let seat = zerocode_core::jev::BROWSER;
    let ledger = ledger_of(&wire, &seat);
    let asked = wire.ask(
        &seat,
        Some(workspace.path()),
        request_body(&json!("Ready"), &questions()),
        Duration::from_secs(5),
    );
    assert_eq!(asked.answer, Err(SCHEMA.to_string()));
    assert_eq!(asked.spent.requests, 1);
    root["smart"]["judgmentProvider"]["protocol"] = json!("system_one");
    std::fs::write(&path, root.to_string()).expect("changed protocol");
    assert_ne!(ledger_of(&wire, &seat), ledger);
}

#[test]
fn a_changed_provider_discards_a_delayed_structured_reply() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let path = home.path().join("settings.json");
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", response(), 150);
    let provider = provider(&endpoint.base());
    let mut root = settings(&provider, workspace.path());
    std::fs::write(&path, root.to_string()).expect("settings");
    let wire = Wire::at("http://127.0.0.1:1", "", Some(path.clone()));
    thread::scope(|scope| {
        let pending = scope.spawn(|| {
            wire.ask(
                &zerocode_core::jev::BROWSER,
                Some(workspace.path()),
                request_body(&json!("Ready"), &questions()),
                Duration::from_secs(5),
            )
        });
        let until = Instant::now() + Duration::from_secs(3);
        while endpoint.asked().is_empty() && Instant::now() < until {
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(endpoint.asked().len(), 1);
        root["smart"]["judgmentProvider"]["model"] = json!("acme/next");
        std::fs::write(&path, root.to_string()).expect("changed provider");
        assert_eq!(
            pending.join().expect("request thread").answer,
            Err("settings_changed".to_string())
        );
    });
}
