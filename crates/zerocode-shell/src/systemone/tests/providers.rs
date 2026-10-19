use super::*;
use crate::systemone::latest;
use judgment_provider::{Protocol, Provider, ProviderKind, Refusal};
use zerocode_core::jev::{JEV_USES, STALL};

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

/// A reply that lands after the person selected another model names the
/// client it was put to, not the one selected when its row is filed — and
/// filed where the new model's records are, it is no use of the new model
/// (t-53792): the folder a row lands in is read after the reply, the client
/// before the request.
#[test]
fn a_reply_after_a_provider_switch_names_the_client_asked_and_is_no_use_of_the_next() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let path = home.path().join("settings.json");
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", answer(), 100);
    let provider = provider(endpoint.base());
    let root = settings(&provider, workspace.path());
    std::fs::write(&path, root.to_string()).expect("settings");
    let wire = Wire::at("http://127.0.0.1:1", "", Some(path.clone()));
    let asked_route = route_of(&wire);
    let mut next = provider.clone();
    next.model = "next-decision-model".to_string();
    let asked = thread::scope(|scope| {
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
        let mut switched = root.clone();
        switched["smart"]["judgmentProvider"] = json!(next);
        std::fs::write(&path, switched.to_string()).expect("new settings");
        waiting.join().expect("join")
    });
    assert_eq!(asked.answer, Err("settings_changed".to_string()));
    assert_eq!(asked.spent.requests, 1);
    assert!(asked_route.is_some());
    assert_eq!(asked.spent.route, asked_route, "the client it was put to");
    assert_ne!(
        route_of(&wire),
        asked_route,
        "not the one selected after it"
    );

    let mut row = json!({"at": 1_000, "outcome": "settings_changed"});
    asked.spent.stamp(&mut row);
    let filed = ledger_of(&wire, &zerocode_core::jev::STALL).expect("the next model's folder");
    append_rows(&filed, &[row]);
    let reading = latest::read(&wire, 2_000);
    assert_eq!(reading.provider_identity, Some(next.identity()));
    assert_eq!(reading.state, latest::State::NoneObserved, "{reading:?}");
    assert_eq!(reading.last_request, None);
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

#[test]
fn local_requests_carry_the_remaining_deadline_and_jev_requests_do_not() {
    for kind in [ProviderKind::Local, ProviderKind::Jev] {
        let home = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let path = home.path().join("settings.json");
        let endpoint = Endpoint::serving("HTTP/1.1 200 OK", answer(), 0);
        let configured = provider(endpoint.base());
        let mut root = settings(&configured, workspace.path());
        if kind == ProviderKind::Jev {
            root["smart"]
                .as_object_mut()
                .unwrap()
                .remove("judgmentProvider");
        }
        std::fs::write(&path, root.to_string()).unwrap();
        let wire = Wire::at(&endpoint.base(), "test-key", Some(path));
        let mut spent = Vec::new();
        for budget in [Duration::from_secs(5), Duration::from_secs(10)] {
            let asked = wire.ask(
                &zerocode_core::jev::BROWSER,
                Some(workspace.path()),
                request_body(&json!("Open help; routine and ready."), &questions()),
                budget,
            );
            assert!(asked.answer.is_ok(), "{:?}", asked.answer);
            spent.push((budget, asked.waited));
        }
        let requests = endpoint.asked();
        assert_eq!(requests.len(), spent.len());
        let mut previous_body = None;
        for (request, (budget, elapsed)) in requests.iter().zip(spent) {
            let (head, body) = request.split_once("\r\n\r\n").unwrap();
            let header = head.lines().find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case(judgment_provider::DEADLINE_HEADER)
                    .then(|| value.trim().parse::<u128>().ok())
                    .flatten()
            });
            if kind == ProviderKind::Local {
                assert!(
                    header.is_some(),
                    "a local request omitted its remaining deadline header"
                );
                let remaining = header.unwrap();
                assert!(remaining > 0 && remaining < budget.as_millis());
                assert!(remaining >= budget.saturating_sub(elapsed).as_millis());
            } else {
                assert!(
                    header.is_none(),
                    "a Jev request carried local deadline metadata"
                );
            }
            let body: Value = serde_json::from_str(body).unwrap();
            if let Some(previous) = &previous_body {
                assert_eq!(
                    &body, previous,
                    "the deadline changed the model's request body"
                );
            }
            previous_body = Some(body);
        }
    }
}

/// The words judgment-provider names its refusals by (`Refusal::token`), read
/// off its source, so a refusal it gains is one these cases ask about.
fn refusal_words() -> Vec<&'static str> {
    include_str!("../../../../judgment-provider/src/lib.rs")
        .split_once("pub const fn token(self)")
        .and_then(|(_, rest)| rest.split_once("\n    }"))
        .map_or("", |(table, _)| table)
        .lines()
        .filter_map(|line| line.split_once("=> \""))
        .filter_map(|(_, rest)| rest.split('"').next())
        .collect()
}

/// What local-judgment answers a question it refused with (`engine.rs`): its
/// protocol error and the refusal's word, and beside the deadline's refusal
/// the largest usage it can name.
fn local_refusal(word: &str) -> String {
    let mut said = json!({"error": local_judgment::Error::Protocol.token(), "refusal": word});
    if word == Refusal::DeadlineBudget.token() {
        said["usage"] = json!({
            "model_calls": u64::MAX,
            "deadline_budget_ms": u64::MAX,
            "estimated_ms": f64::MAX,
            "deadline_budget_reason": "unmeasured",
        });
    }
    said.to_string()
}

/// The stall seat's one question, asked through a local provider at `base`.
fn ask_locally(base: String, home: &Path, workspace: &Path) -> (Wire, Asked) {
    let path = home.join("settings.json");
    std::fs::write(&path, settings(&provider(base), workspace).to_string()).expect("settings");
    let wire = Wire::at("http://127.0.0.1:1", "", Some(path));
    let asked = wire.ask(
        &STALL,
        Some(workspace),
        request_body(&json!("Open help; routine and ready."), &questions()),
        Duration::from_secs(5),
    );
    (wire, asked)
}

/// A local server that turns a question away with a refusal judgment-provider
/// names, in a `502` whose body is local-judgment's protocol error and that
/// refusal's word, is told by the word rather than by its status: the
/// question fails with the word, and the last request read back off the row
/// its seat writes says it.
#[test]
fn a_local_refusal_is_told_by_its_word_up_to_the_last_request() {
    let words = refusal_words();
    assert!(
        words.contains(&Refusal::DeadlineBudget.token()),
        "judgment-provider's refusal words were not read: {words:?}"
    );
    let mut lost = Vec::new();
    for word in words {
        let home = tempfile::tempdir().expect("a zo home");
        let workspace = tempfile::tempdir().expect("a workspace");
        let said = local_refusal(word);
        assert!(
            said.len() <= MAX_REFUSAL_BYTES,
            "`{word}`'s refusal is past the bound: {said}"
        );
        let endpoint = Endpoint::serving("HTTP/1.1 502 Bad Gateway", said, 0);
        let (wire, asked) = ask_locally(endpoint.base(), home.path(), workspace.path());
        let mut row = json!({"at": 42, "outcome": asked.answer.as_ref().err()});
        asked.spent.stamp(&mut row);
        record_rows(
            &STALL,
            &ledger_of(&wire, &STALL).expect("the record"),
            &[row],
            42,
        );
        let last = latest::read(&wire, 43)
            .last_request
            .map(|last| last.outcome);
        if asked.answer != Err(word.to_string()) || last.as_deref() != Some(word) {
            lost.push((word, asked.answer, last));
        }
    }
    assert!(
        lost.is_empty(),
        "a local refusal's word was lost before the last request: {lost:?}"
    );
}

/// A local `502` keeps its status word unless its body, whole and in time, is
/// local-judgment's protocol error naming a refusal judgment-provider names:
/// an unknown word, free text, the protocol error alone, another error, no
/// error, a page that is not JSON and a body past the bound all stay
/// `http_502`, as does a body that breaks off, which is not waited out.
#[test]
fn a_local_502_that_names_no_known_refusal_keeps_its_status() {
    let home = tempfile::tempdir().expect("a zo home");
    let workspace = tempfile::tempdir().expect("a workspace");
    let deadline = Refusal::DeadlineBudget.token();
    let protocol = local_judgment::Error::Protocol.token();
    let mut told = Vec::new();
    for said in [
        json!({"error": protocol, "refusal": format!("{deadline}_spent")}).to_string(),
        json!({"error": protocol, "refusal": "The model ran out of time."}).to_string(),
        json!({"error": protocol}).to_string(),
        json!({"error": local_judgment::Error::Busy.token(), "refusal": deadline}).to_string(),
        json!({"refusal": deadline}).to_string(),
        "<html><body>502 Bad Gateway</body></html>".to_string(),
        json!({"error": protocol, "refusal": deadline, "pad": "x".repeat(MAX_REFUSAL_BYTES)})
            .to_string(),
    ] {
        let endpoint = Endpoint::serving("HTTP/1.1 502 Bad Gateway", said.clone(), 0);
        let (_, asked) = ask_locally(endpoint.base(), home.path(), workspace.path());
        if asked.answer != Err(token_for(502)) {
            told.push((said, asked.answer));
        }
    }
    assert!(
        told.is_empty(),
        "a local 502 was told by a word it does not carry: {told:?}"
    );

    let held = MAX_REFUSAL_WAIT * 10;
    for (hold, within) in [(Duration::ZERO, None), (held, Some(held / 2))] {
        let started = std::time::Instant::now();
        let (_, asked) = ask_locally(
            refusing_brokenly("HTTP/1.1 502 Bad Gateway", local_refusal(deadline), hold),
            home.path(),
            workspace.path(),
        );
        assert_eq!(
            asked.answer,
            Err(token_for(502)),
            "a body held {hold:?} and broken off keeps its status"
        );
        if let Some(within) = within {
            assert!(
                started.elapsed() < within,
                "a refusal's body held {hold:?} is not waited out"
            );
        }
    }
}

/// The same refusal from the Jev endpoint keeps its status: only a local
/// server's refusal is told by its word.
#[test]
fn a_jev_502_keeps_its_status_whatever_its_body_names() {
    let home = tempfile::tempdir().expect("a zo home");
    let workspace = tempfile::tempdir().expect("a workspace");
    let path = home.path().join("settings.json");
    let endpoint = Endpoint::serving(
        "HTTP/1.1 502 Bad Gateway",
        local_refusal(Refusal::DeadlineBudget.token()),
        0,
    );
    let mut root = settings(&provider(endpoint.base()), workspace.path());
    root["smart"]
        .as_object_mut()
        .expect("the smart settings")
        .remove("judgmentProvider");
    std::fs::write(&path, root.to_string()).expect("settings");
    let asked = Wire::at(&endpoint.base(), "test-key", Some(path)).ask(
        &STALL,
        Some(workspace.path()),
        request_body(&json!("Open help; routine and ready."), &questions()),
        Duration::from_secs(5),
    );
    assert_eq!(asked.answer, Err(token_for(502)));
}
