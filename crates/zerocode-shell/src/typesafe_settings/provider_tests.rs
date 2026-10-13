use super::*;
use crate::api_routers::HeldKeys;
use crate::systemone::tests::Endpoint;
use judgment_provider::{Protocol, Provider, ProviderKind};
use serde_json::json;

fn local() -> Provider {
    Provider {
        kind: ProviderKind::Local,
        protocol: Protocol::SystemOne,
        base_url: "http://127.0.0.1:1234".to_string(),
        model: "acme/decision-model".to_string(),
        requires_auth: false,
    }
}

#[test]
fn switching_a_provider_preserves_every_feature_choice_and_cloud_pin() {
    let home = tempfile::tempdir().expect("home");
    let path = home.path().join("settings.json");
    let mut before = json!({"theme":"dark","smart":{
        "jevModel":"jev-held",
        "jev":{"enabled":true,"workspaces":["/work/acme"],"dailyRequests":12}
    }});
    for seat in JEV_USES {
        before["smart"][seat.setting] = json!(seat.modes[0].key());
    }
    std::fs::write(&path, before.to_string()).expect("settings");
    let keys = HeldKeys::default();
    let original = read_settings(&path, &keys, true).expect("original");
    set_provider(&path, local()).expect("select local");
    let current = read_settings(&path, &keys, true).expect("current");
    assert_eq!(current.switches, original.switches);
    assert_eq!(current.jev, original.jev);
    assert_eq!(current.jev_provider.model, "jev-held");
    let mut after: Value =
        serde_json::from_slice(&std::fs::read(&path).expect("read")).expect("JSON");
    after["smart"]
        .as_object_mut()
        .expect("smart")
        .remove(judgment_provider::SETTINGS_KEY);
    assert_eq!(after, before);
    set_provider(&path, current.jev_provider).expect("return to Jev");
    assert_eq!(
        selected_provider(&path).expect("provider").model,
        "jev-held"
    );
}

#[test]
fn a_local_provider_neither_reports_nor_removes_the_cloud_key() {
    let home = tempfile::tempdir().expect("home");
    let path = home.path().join("settings.json");
    let keys = HeldKeys::default();
    save_key("cloud-fixture", &keys).expect("cloud key");
    set_provider(&path, local()).expect("local");
    let state = read_settings(&path, &keys, true).expect("state");
    assert!(!state.key_saved);
    remove_selected_key(&path, Some(&state.provider_identity), &keys).expect("remove local key");
    assert_eq!(
        keys.held(&typesafe_keychain_service()).as_deref(),
        Some("cloud-fixture")
    );
    let text = serde_json::to_string(&state).expect("state JSON");
    assert!(!text.contains("cloud-fixture"));
}

#[test]
fn an_old_settings_pane_cannot_write_or_delete_the_new_providers_key() {
    let home = tempfile::tempdir().expect("home");
    let path = home.path().join("settings.json");
    let keys = HeldKeys::default();
    let mut provider = local();
    provider.requires_auth = true;
    set_provider(&path, provider.clone()).expect("local");
    save_selected_key(&path, Some(&provider.identity()), "local-fixture", &keys)
        .expect("local key");
    for expected in [None, Some("jev-latest")] {
        assert!(save_selected_key(&path, expected, "stale-fixture", &keys).is_err());
        assert!(remove_selected_key(&path, expected, &keys).is_err());
    }
    assert_eq!(
        keys.held(&provider_keychain_service(&provider)).as_deref(),
        Some("local-fixture")
    );
    assert_eq!(keys.held(&typesafe_keychain_service()), None);
}

#[test]
fn refusing_a_provider_leaves_the_settings_bytes_unchanged() {
    let home = tempfile::tempdir().expect("home");
    let path = home.path().join("settings.json");
    let before = "{\"smart\":{\"jevModel\":\"jev-held\"}}\n";
    std::fs::write(&path, before).expect("settings");
    let mut provider = local();
    provider.base_url = "https://example.com".to_string();
    assert!(set_provider(&path, provider).is_err());
    assert_eq!(std::fs::read_to_string(&path).expect("settings"), before);
}

#[test]
fn an_old_companion_cannot_report_a_cloud_answer_as_a_local_connection() {
    let provider = local();
    let mut check = TypeSafeCheck {
        answered: true,
        model: Some("jev-old".to_string()),
        failure: None,
        elapsed_ms: 12,
    };
    assert_eq!(
        checked_provider(&provider, check.clone()),
        Err("provider_client_outdated".to_string())
    );
    check.model = Some(format!("{}:actual-model", provider.identity()));
    assert_eq!(checked_provider(&provider, check.clone()), Ok(check));
}

#[test]
fn unreadable_provider_settings_never_select_the_default_cloud_service() {
    let home = tempfile::tempdir().expect("home");
    let path = home.path().join("settings.json");
    assert_eq!(
        Provider::read(&path).expect("existing default"),
        Provider::jev(DEFAULT_MODEL)
    );
    for contents in ["{", "null", "[]", "{\"smart\":{\"judgmentProvider\":null}}"] {
        std::fs::write(&path, contents).expect("settings");
        assert!(Provider::read(&path).is_err(), "{contents}");
    }
}

#[test]
fn a_custom_provider_check_requires_a_companion_that_understands_the_provider() {
    let provider = local();
    let args = key_check_args(&provider);
    assert_eq!(args[..3], ZO_KEY_CHECK_ARGS);
    assert_eq!(args[3], ZO_CHECK_PROVIDER_FLAG);
    assert_eq!(args[4], provider.identity());
    assert_eq!(
        key_check_args(&Provider::jev(DEFAULT_MODEL)),
        ZO_KEY_CHECK_ARGS
    );
}

fn candidate(base: String) -> Provider {
    Provider {
        kind: ProviderKind::Local,
        protocol: Protocol::SystemOne,
        base_url: base,
        model: "acme/decision-model:q4".to_string(),
        requires_auth: false,
    }
}

fn preflight_answer() -> Value {
    json!({"model":"fixture-version","answers":{
        (PREFLIGHT_CHOICE):{"type":"choice","choice":"even","confidence":0.9,"probabilities":{"even":0.95,"odd":0.05}},
        (PREFLIGHT_SCORE):{"type":"score","score":0.1,"confidence":0.8,"probabilities":{"0":0.9,"1":0.1},"legend":{"0":PREFLIGHT_LEVELS[0],"1":PREFLIGHT_LEVELS[1]}},
        (PREFLIGHT_NOUL):{"type":"noul","noul":0.97}
    },"usage":{"input_tokens":40,"output_tokens":0}})
}

fn selected_with_managed_model(path: &Path) -> (Provider, String) {
    let mut selected = local();
    selected.base_url = "http://127.0.0.1:9".to_string();
    let text = json!({"theme":"dark","smart":{
        (judgment_provider::SETTINGS_KEY):selected,
        (local_judgment::MANAGED_MODEL_KEY):{
            "modelId":selected.model,
            "revision":"0".repeat(40),
            "providerIdentity":selected.identity()
        }
    }})
    .to_string();
    std::fs::write(path, &text).expect("settings");
    (selected, text)
}

fn requests_today(path: &Path) -> u64 {
    count::sent(&count::requests_path(
        path.parent().expect("home"),
        &crate::systemone::today(),
    ))
}

fn settings_wire(path: &Path) -> crate::systemone::Wire {
    crate::systemone::Wire::at("http://127.0.0.1:1", "", Some(path.to_path_buf()))
}

#[test]
fn a_preflight_answer_leaves_the_selected_provider_and_its_managed_model_unchanged() {
    let home = tempfile::tempdir().expect("home");
    let path = home.path().join("settings.json");
    let (selected, before) = selected_with_managed_model(&path);
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", preflight_answer().to_string(), 0);
    let keys = HeldKeys::default();
    let probed = candidate(endpoint.base());
    let preflight = preflight_provider(
        &path,
        probed.clone(),
        &selected.identity(),
        &keys,
        &settings_wire(&path),
    )
    .expect("preflight");
    assert_eq!(
        preflight,
        ProviderPreflight {
            candidate_identity: probed.identity(),
            active_provider_identity: selected.identity(),
            answered: true,
            model: Some(format!("{}:fixture-version", probed.identity())),
            failure: None,
            elapsed_ms: preflight.elapsed_ms,
        }
    );
    assert_eq!(std::fs::read_to_string(&path).expect("settings"), before);
    assert_eq!(selected_provider(&path).expect("selected"), selected);
    assert_eq!(keys.held(&provider_keychain_service(&probed)), None);
    assert_eq!(requests_today(&path), 1);
    let asked = endpoint.asked();
    assert_eq!(asked.len(), 1);
    let (head, body) = asked[0].split_once("\r\n\r\n").expect("request");
    assert!(head.starts_with("POST /v1/systemone "));
    assert!(!head.to_ascii_lowercase().contains("authorization:"));
    let body: Value = serde_json::from_str(body).expect("body");
    assert_eq!(body["model"], probed.model);
    assert_eq!(body["state"], preflight_body()["state"]);
}

#[test]
fn a_custom_candidate_keeps_its_model_id_and_endpoint_through_preflight_and_apply() {
    let home = tempfile::tempdir().expect("home");
    let path = home.path().join("settings.json");
    let (selected, _) = selected_with_managed_model(&path);
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", preflight_answer().to_string(), 0);
    let typed = candidate(format!("{}/v1/", endpoint.base()));
    let preflight = preflight_provider(
        &path,
        typed.clone(),
        &selected.identity(),
        &HeldKeys::default(),
        &settings_wire(&path),
    )
    .expect("preflight");
    assert!(preflight.answered, "{preflight:?}");
    let asked = endpoint.asked();
    assert!(asked[0].starts_with("POST /v1/systemone "), "{}", asked[0]);
    assert!(asked[0].contains("\"model\":\"acme/decision-model:q4\""));
    set_provider_if_identity(&path, typed, &preflight.active_provider_identity, None)
        .expect("apply");
    let applied = selected_provider(&path).expect("applied");
    assert_eq!(applied.identity(), preflight.candidate_identity);
    assert_eq!(applied.model, "acme/decision-model:q4");
    assert_eq!(applied.base_url, format!("{}/v1", endpoint.base()));
    let root: Value = serde_json::from_slice(&std::fs::read(&path).expect("read")).expect("JSON");
    assert!(
        root["smart"]
            .get(local_judgment::MANAGED_MODEL_KEY)
            .is_none()
    );
}

#[test]
fn a_bare_success_or_a_wrong_shape_never_passes_for_an_answer() {
    let home = tempfile::tempdir().expect("home");
    let path = home.path().join("settings.json");
    let (selected, before) = selected_with_managed_model(&path);
    let keys = HeldKeys::default();
    save_key("cloud-fixture", &keys).expect("cloud key");
    let mut unoffered = preflight_answer();
    unoffered["answers"][PREFLIGHT_CHOICE]["choice"] = json!("seven");
    let mut off_the_mean = preflight_answer();
    off_the_mean["answers"][PREFLIGHT_SCORE]["score"] = json!(0.9);
    let mut unsure = preflight_answer();
    unsure["answers"][PREFLIGHT_NOUL]["noul"] = json!(1.5);
    let mut missing = preflight_answer();
    missing["answers"]
        .as_object_mut()
        .expect("answers")
        .remove(PREFLIGHT_NOUL);
    let mut retagged = preflight_answer();
    retagged["answers"][PREFLIGHT_SCORE]["type"] = json!("choice");
    let bodies = [
        String::new(),
        "OK".to_string(),
        "{}".to_string(),
        json!({"data":[{"id":"acme/decision-model:q4"}]}).to_string(),
        json!({"model":"fixture-version","choices":[{"message":{"role":"assistant","content":"even"}}],
            "usage":{"input_tokens":40,"output_tokens":1}})
        .to_string(),
        unoffered.to_string(),
        off_the_mean.to_string(),
        unsure.to_string(),
        missing.to_string(),
        retagged.to_string(),
    ];
    for body in bodies {
        let endpoint = Endpoint::serving("HTTP/1.1 200 OK", body.clone(), 0);
        let candidates = [candidate(endpoint.base()), Provider::jev(DEFAULT_MODEL)];
        for probed in candidates {
            let wire = crate::systemone::Wire::at(&endpoint.base(), "", Some(path.clone()));
            let preflight =
                preflight_provider(&path, probed.clone(), &selected.identity(), &keys, &wire)
                    .expect("preflight");
            let failure = preflight.failure.clone().unwrap_or_default();
            assert!(!preflight.answered, "{body} passed for {:?}", probed.kind);
            assert_eq!(preflight.model, None, "{body}");
            assert!(
                promote::names_a_schema_failure(&failure),
                "{body} for {:?}: {failure}",
                probed.kind
            );
        }
        assert_eq!(endpoint.asked().len(), 2, "{body}");
    }
    assert_eq!(std::fs::read_to_string(&path).expect("settings"), before);
}

#[test]
fn a_jev_candidate_is_read_by_the_same_seat_readers_with_its_own_key() {
    let home = tempfile::tempdir().expect("home");
    let path = home.path().join("settings.json");
    let (selected, before) = selected_with_managed_model(&path);
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", preflight_answer().to_string(), 0);
    let wire = crate::systemone::Wire::at(&endpoint.base(), "", Some(path.clone()));
    let keys = HeldKeys::default();
    let probed = Provider::jev(DEFAULT_MODEL);
    let unkeyed = preflight_provider(&path, probed.clone(), &selected.identity(), &keys, &wire)
        .expect("preflight");
    assert_eq!(
        unkeyed.failure.as_deref(),
        Some(door::Refused::NoKey.token())
    );
    assert!(endpoint.asked().is_empty());
    save_key("cloud-fixture", &keys).expect("cloud key");
    let keyed =
        preflight_provider(&path, probed, &selected.identity(), &keys, &wire).expect("preflight");
    assert!(keyed.answered, "{keyed:?}");
    assert_eq!(keyed.model.as_deref(), Some("fixture-version"));
    assert_eq!(keyed.candidate_identity, DEFAULT_MODEL);
    let head = endpoint.asked()[0].to_ascii_lowercase();
    assert!(head.contains("authorization: bearer cloud-fixture"));
    assert_eq!(std::fs::read_to_string(&path).expect("settings"), before);
}

#[test]
fn a_preflight_sends_each_endpoint_only_the_key_that_endpoint_keeps() {
    let home = tempfile::tempdir().expect("home");
    let path = home.path().join("settings.json");
    let (selected, _) = selected_with_managed_model(&path);
    let first = Endpoint::serving("HTTP/1.1 200 OK", preflight_answer().to_string(), 0);
    let second = Endpoint::serving("HTTP/1.1 200 OK", preflight_answer().to_string(), 0);
    let mut keyed = candidate(first.base());
    keyed.requires_auth = true;
    let mut unkeyed = candidate(second.base());
    unkeyed.requires_auth = true;
    let keys = HeldKeys::default();
    save_key("cloud-fixture", &keys).expect("cloud key");
    save_key_at(
        &provider_keychain_service(&keyed),
        " first-endpoint-fixture ",
        "empty",
        &keys,
    )
    .expect("first key");
    let refused = preflight_provider(
        &path,
        unkeyed,
        &selected.identity(),
        &keys,
        &settings_wire(&path),
    )
    .expect("preflight");
    assert_eq!(
        refused.failure.as_deref(),
        Some(door::Refused::NoKey.token())
    );
    assert!(second.asked().is_empty());
    let answered = preflight_provider(
        &path,
        keyed,
        &selected.identity(),
        &keys,
        &settings_wire(&path),
    )
    .expect("preflight");
    assert!(answered.answered, "{answered:?}");
    let head = first.asked()[0].to_ascii_lowercase();
    assert!(head.contains("authorization: bearer first-endpoint-fixture\r\n"));
    assert!(!head.contains("cloud-fixture"));
    assert!(second.asked().is_empty());
}

#[test]
fn a_refused_key_a_failed_server_and_the_persons_controls_are_failures_not_answers() {
    let home = tempfile::tempdir().expect("home");
    let path = home.path().join("settings.json");
    let (selected, _) = selected_with_managed_model(&path);
    let keys = HeldKeys::default();
    for (status, token) in [
        ("HTTP/1.1 401 Unauthorized", "unauthorized"),
        ("HTTP/1.1 302 Found", "http_302"),
        ("HTTP/1.1 500 Internal Server Error", "http_500"),
    ] {
        let endpoint = Endpoint::serving(status, preflight_answer().to_string(), 0);
        let preflight = preflight_provider(
            &path,
            candidate(endpoint.base()),
            &selected.identity(),
            &keys,
            &settings_wire(&path),
        )
        .expect("preflight");
        assert!(!preflight.answered);
        assert_eq!(preflight.failure.as_deref(), Some(token));
        assert_eq!(preflight.model, None);
    }
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", preflight_answer().to_string(), 0);
    let sent = requests_today(&path);
    for (jev, refused) in [
        (json!({"enabled":false}), door::Refused::Off),
        (json!({"dailyRequests":0}), door::Refused::Budget),
    ] {
        let mut root: Value =
            serde_json::from_slice(&std::fs::read(&path).expect("read")).expect("JSON");
        root["smart"]["jev"] = jev;
        std::fs::write(&path, root.to_string()).expect("settings");
        let preflight = preflight_provider(
            &path,
            candidate(endpoint.base()),
            &selected.identity(),
            &keys,
            &settings_wire(&path),
        )
        .expect("preflight");
        assert_eq!(preflight.failure.as_deref(), Some(refused.token()));
    }
    assert!(endpoint.asked().is_empty());
    assert_eq!(requests_today(&path), sent);
}

#[test]
fn a_candidate_an_apply_would_refuse_is_refused_before_anything_is_sent() {
    let home = tempfile::tempdir().expect("home");
    let path = home.path().join("settings.json");
    let (selected, before) = selected_with_managed_model(&path);
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", preflight_answer().to_string(), 0);
    let mut remote = candidate(endpoint.base());
    remote.kind = ProviderKind::Remote;
    let mut unpinned = Provider::jev(DEFAULT_MODEL);
    unpinned.model = "two words".to_string();
    for (refused, token) in [
        (remote, "provider_https_required"),
        (unpinned, "provider_model"),
    ] {
        assert_eq!(
            preflight_provider(
                &path,
                refused,
                &selected.identity(),
                &HeldKeys::default(),
                &settings_wire(&path)
            ),
            Err(token.to_string())
        );
    }
    assert!(endpoint.asked().is_empty());
    assert_eq!(std::fs::read_to_string(&path).expect("settings"), before);
}

#[test]
fn a_moved_selection_refuses_the_preflight_and_its_stale_apply() {
    let home = tempfile::tempdir().expect("home");
    let path = home.path().join("settings.json");
    let (selected, _) = selected_with_managed_model(&path);
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", preflight_answer().to_string(), 300);
    let probed = candidate(endpoint.base());
    assert_eq!(
        preflight_provider(
            &path,
            probed.clone(),
            "judgment-stale",
            &HeldKeys::default(),
            &settings_wire(&path)
        ),
        Err("settings_changed".to_string())
    );
    assert!(endpoint.asked().is_empty());
    assert_eq!(requests_today(&path), 0);

    let mut moved = local();
    moved.model = "acme/other-model".to_string();
    let shared = settings_wire(&path);
    std::thread::scope(|scope| {
        let waiting = scope.spawn(|| {
            preflight_provider(
                &path,
                probed.clone(),
                &selected.identity(),
                &HeldKeys::default(),
                &shared,
            )
        });
        let deadline = Instant::now() + Duration::from_secs(3);
        while endpoint.count() == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(endpoint.count(), 1);
        set_provider(&path, moved.clone()).expect("moved during the preflight");
        assert_eq!(
            waiting.join().expect("preflight"),
            Err("settings_changed".to_string())
        );
    });

    let before = std::fs::read_to_string(&path).expect("settings");
    assert_eq!(
        set_provider_if_identity(&path, probed.clone(), &selected.identity(), None),
        Err("settings_changed".to_string())
    );
    assert_eq!(std::fs::read_to_string(&path).expect("settings"), before);
    set_provider_if_identity(&path, probed.clone(), &moved.identity(), None)
        .expect("an apply that names the current selection");
    assert_eq!(selected_provider(&path).expect("applied"), probed);
}
