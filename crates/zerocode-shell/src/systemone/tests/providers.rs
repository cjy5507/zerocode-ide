use super::*;
use judgment_provider::{Protocol, Provider, ProviderKind};
use zerocode_core::jev::JEV_USES;

fn provider(base: String) -> Provider {
    Provider {
        kind: ProviderKind::Local,
        protocol: Protocol::SystemOne,
        base_url: base,
        model: "fixture-decision".to_string(),
        requires_auth: false,
    }
}

fn questions() -> Value {
    json!({
        "destination":{"type":"choice","instructions":"Which destination was requested?","criteria":{"home":"Home","help":"Help"}},
        "urgency":{"type":"score","instructions":"How urgent is it?","criteria":["Routine","Urgent"]},
        "ready":{"type":"noul","instructions":"Is the page ready?"}
    })
}

fn answer() -> String {
    json!({"model":"fixture-version","answers":{
        "destination":{"type":"choice","choice":"help","confidence":0.8,"probabilities":{"home":0.1,"help":0.9}},
        "urgency":{"type":"score","score":0.1,"confidence":0.8,"probabilities":{"0":0.9,"1":0.1},"legend":{"0":"Routine","1":"Urgent"}},
        "ready":{"type":"noul","noul":0.99}
    },"usage":{"input_tokens":12,"output_tokens":0}}).to_string()
}

fn settings(provider: &Provider, workspace: &Path) -> Value {
    let mut root = json!({"smart":{
        "judgmentProvider":provider,
        "jev":{"enabled":true,"workspaces":[door::resolved_path(workspace)]}
    }});
    for seat in JEV_USES {
        let mode = seat
            .modes
            .iter()
            .find(|mode| mode.asks())
            .expect("asking mode");
        root["smart"][seat.setting] = json!(mode.key());
    }
    root
}

#[test]
fn every_registered_seat_uses_the_selected_local_provider_without_a_cloud_key() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let path = home.path().join("settings.json");
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", answer(), 0);
    let provider = provider(endpoint.base());
    std::fs::write(&path, settings(&provider, workspace.path()).to_string()).expect("settings");
    let wire = Wire::at("http://127.0.0.1:1", "", Some(path));
    assert!(wire.armed());
    for seat in JEV_USES {
        let asked = wire.ask(
            &seat,
            Some(workspace.path()),
            request_body(&json!("Open help; routine and ready."), &questions()),
            Duration::from_secs(5),
        );
        assert!(asked.answer.is_ok(), "{}: {:?}", seat.id, asked.answer);
        assert_eq!(
            asked.spent.model,
            Some(format!("{}:fixture-version", provider.identity()))
        );
        assert_eq!(asked.spent.requests, 1);
    }
    let sent = endpoint.asked();
    assert_eq!(sent.len(), JEV_USES.len());
    for request in sent {
        let (head, body) = request.split_once("\r\n\r\n").expect("request");
        assert!(head.starts_with("POST /v1/systemone "));
        assert!(!head.to_ascii_lowercase().contains("authorization:"));
        let body: Value = serde_json::from_str(body).expect("body");
        assert_eq!(body["model"], provider.model);
        assert!(body.get("messages").is_none());
    }
}

#[test]
fn selecting_local_does_not_bypass_an_off_switch_or_withdrawn_consent() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let path = home.path().join("settings.json");
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", answer(), 0);
    let provider = provider(endpoint.base());
    let mut root = settings(&provider, workspace.path());
    root["smart"]["jev"]["enabled"] = json!(false);
    std::fs::write(&path, root.to_string()).expect("settings");
    let wire = Wire::at("http://127.0.0.1:1", "", Some(path.clone()));
    let body = request_body(&json!("Open help; routine and ready."), &questions());
    for seat in JEV_USES {
        assert_eq!(
            wire.ask(
                &seat,
                Some(workspace.path()),
                body.clone(),
                Duration::from_secs(1)
            )
            .spent
            .requests,
            0
        );
    }
    root["smart"]["jev"]["enabled"] = json!(true);
    root["smart"]["jev"]["workspaces"] = json!([]);
    std::fs::write(&path, root.to_string()).expect("settings");
    for seat in JEV_USES {
        assert_eq!(
            wire.ask(
                &seat,
                Some(workspace.path()),
                body.clone(),
                Duration::from_secs(1)
            )
            .spent
            .requests,
            0
        );
    }
    assert!(endpoint.asked().is_empty());
}

#[test]
fn a_delayed_local_reply_cannot_survive_a_provider_switch() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let path = home.path().join("settings.json");
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", answer(), 100);
    let provider = provider(endpoint.base());
    let root = settings(&provider, workspace.path());
    std::fs::write(&path, root.to_string()).expect("settings");
    let wire = Wire::at("http://127.0.0.1:1", "", Some(path.clone()));
    thread::scope(|scope| {
        let waiting = scope.spawn(|| {
            wire.ask(
                &zerocode_core::jev::STALL,
                Some(workspace.path()),
                request_body(&json!("Open help; routine and ready."), &questions()),
                Duration::from_secs(5),
            )
        });
        let deadline = Instant::now() + Duration::from_secs(3);
        while endpoint.asked().is_empty() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(endpoint.asked().len(), 1);
        let mut next = root.clone();
        next["smart"]["judgmentProvider"]["model"] = json!("next-decision-model");
        std::fs::write(&path, next.to_string()).expect("new settings");
        let asked = waiting.join().expect("join");
        assert_eq!(asked.answer, Err("settings_changed".to_string()));
        assert_eq!(asked.spent.requests, 1);
    });
}

#[test]
fn a_local_model_cannot_inherit_a_cloud_models_promoted_standing() {
    use zerocode_core::jev::promote::{ROSE, Stand};
    use zerocode_core::jev::summary::{RUBRIC_VERSIONS, TRANSITION};

    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let path = home.path().join("settings.json");
    let seat = zerocode_core::jev::ROUTING;
    assert_eq!(seat.auto_starts, Stand::Recording);
    let cloud = json!({"smart":{"jev":{"enabled":true,"workspaces":[door::resolved_path(workspace.path())]},(seat.setting):"auto"}});
    std::fs::write(&path, cloud.to_string()).expect("settings");
    let wire = Wire::at("http://127.0.0.1:1", "", Some(path.clone()));
    let cloud_ledger = ledger_of(&wire, &seat).expect("cloud ledger");
    append_rows(
        &cloud_ledger,
        &[json!({(TRANSITION.canonical):ROSE,(RUBRIC_VERSIONS.canonical):[seat.rubric_version]})],
    );
    assert!(applies_once_risen(&wire, &seat));
    let local = provider("http://127.0.0.1:1234".to_string());
    let mut root = settings(&local, workspace.path());
    root["smart"][seat.setting] = json!("auto");
    std::fs::write(&path, root.to_string()).expect("local settings");
    assert_ne!(ledger_of(&wire, &seat), Some(cloud_ledger.clone()));
    assert!(!applies_once_risen(&wire, &seat));
    assert!(!applies(&wire, &seat));
    std::fs::write(&path, cloud.to_string()).expect("return to Jev");
    assert_eq!(ledger_of(&wire, &seat), Some(cloud_ledger));
    assert!(applies_once_risen(&wire, &seat));
}

#[test]
#[ignore = "requires an explicitly selected loopback decision server"]
fn a_real_local_provider_uses_the_window_wire() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let path = home.path().join("settings.json");
    let mut local =
        provider(std::env::var("ZEROCODE_JUDGMENT_TEST_URL").expect("loopback server URL"));
    local.model = std::env::var("ZEROCODE_JUDGMENT_TEST_MODEL").expect("exact model ID");
    let local = local.validated().expect("local decision provider");
    std::fs::write(&path, settings(&local, workspace.path()).to_string()).expect("settings");
    let wire = Wire::at("http://127.0.0.1:1", "", Some(path));
    let mut samples = Vec::new();
    for ordinal in 0..13 {
        let body = request_body(
            &json!("Open help; the page is ready and this is routine, not urgent."),
            &questions(),
        );
        let asked = wire.ask(
            &zerocode_core::jev::BROWSER,
            Some(workspace.path()),
            body,
            Duration::from_secs(12),
        );
        let mut sample = json!({"ordinal":ordinal,"elapsedMs":asked.waited.as_secs_f64()*1000.0});
        asked.spent.stamp(&mut sample);
        match asked.answer {
            Ok(body) => {
                sample["response"] = serde_json::from_str(&body).expect("native response");
                sample["transportPassed"] = json!(true);
                sample["destinationCorrect"] =
                    json!(sample["response"]["answers"]["destination"]["choice"] == "help");
            }
            Err(reason) => {
                sample["failure"] = json!(reason);
                sample["transportPassed"] = json!(false);
            }
        }
        samples.push(sample);
    }
    let report = json!({"provider":local,"boundary":"native window wire only; not a completed Computer Use task","samples":samples});
    let output = std::env::var("ZEROCODE_JUDGMENT_TEST_OUT").expect("evidence output path");
    std::fs::write(output, serde_json::to_vec_pretty(&report).expect("report"))
        .expect("save report");
    assert!(
        samples
            .iter()
            .all(|sample| sample["transportPassed"] == true),
        "native wire failed"
    );
}
