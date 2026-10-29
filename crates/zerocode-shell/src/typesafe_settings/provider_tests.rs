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
        connection: None,
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
        connection: None,
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
fn an_entered_preflight_key_is_used_without_replacing_the_saved_key() {
    let home = tempfile::tempdir().expect("home");
    let path = home.path().join("settings.json");
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", preflight_answer().to_string(), 0);
    let mut provider = candidate(endpoint.base());
    provider.requires_auth = true;
    set_provider(&path, provider.clone()).expect("active provider");
    let before = std::fs::read(&path).expect("settings");
    let keys = HeldKeys::default();
    let service = provider_keychain_service(&provider);
    keys.write(&service, "kept-fixture").expect("saved key");
    let checked = preflight_provider_with_key(
        &path,
        provider.clone(),
        &provider.identity(),
        &keys,
        &settings_wire(&path),
        Some("entered-fixture"),
    )
    .expect("preflight");
    assert!(checked.answered, "{checked:?}");
    assert!(
        endpoint.asked()[0]
            .to_ascii_lowercase()
            .contains("authorization: bearer entered-fixture")
    );
    assert_eq!(keys.held(&service).as_deref(), Some("kept-fixture"));
    assert_eq!(std::fs::read(&path).expect("settings"), before);
}

#[test]
fn a_stale_connection_apply_leaves_its_saved_key_and_settings_untouched() {
    let home = tempfile::tempdir().expect("home");
    let path = home.path().join("settings.json");
    let (active, before) = selected_with_managed_model(&path);
    let mut provider = local();
    provider.requires_auth = true;
    let keys = HeldKeys::default();
    let service = provider_keychain_service(&provider);
    keys.write(&service, "kept-fixture").expect("saved key");
    let result = set_provider_with_key_if_identity(
        &path,
        provider,
        "stale-identity",
        "entered-fixture",
        &keys,
    );
    assert_eq!(result.expect_err("stale apply").message, "settings_changed");
    assert_eq!(keys.held(&service).as_deref(), Some("kept-fixture"));
    assert_eq!(std::fs::read_to_string(&path).expect("settings"), before);
    assert_eq!(selected_provider(&path).expect("provider"), active);
}

#[test]
fn applying_a_checked_connection_commits_its_key_without_putting_it_in_settings() {
    let home = tempfile::tempdir().expect("home");
    let path = home.path().join("settings.json");
    let (active, _) = selected_with_managed_model(&path);
    let mut provider = local();
    provider.requires_auth = true;
    let keys = HeldKeys::default();
    set_provider_with_key_if_identity(
        &path,
        provider.clone(),
        &active.identity(),
        "entered-fixture",
        &keys,
    )
    .expect("checked apply");
    assert_eq!(selected_provider(&path).expect("provider"), provider);
    assert_eq!(
        keys.held(&provider_keychain_service(&provider)).as_deref(),
        Some("entered-fixture")
    );
    let saved = std::fs::read_to_string(&path).expect("settings");
    assert!(!saved.contains("entered-fixture"));
    let root: Value = serde_json::from_str(&saved).expect("JSON");
    assert_eq!(root["theme"], "dark");
    assert!(
        root["smart"]
            .get(local_judgment::MANAGED_MODEL_KEY)
            .is_none()
    );
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
    remote.base_url = "http://gateway.example.com/v1".to_string();
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
fn a_remote_loopback_candidate_can_be_checked_then_applied() {
    let home = tempfile::tempdir().expect("home");
    let path = home.path().join("settings.json");
    let (selected, before) = selected_with_managed_model(&path);
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", preflight_answer().to_string(), 0);
    let mut remote = candidate(endpoint.base());
    remote.kind = ProviderKind::Remote;
    let check = preflight_provider(
        &path,
        remote.clone(),
        &selected.identity(),
        &HeldKeys::default(),
        &settings_wire(&path),
    )
    .expect("an explicit loopback HTTP connection");
    assert!(check.answered, "{check:?}");
    assert_eq!(endpoint.count(), 1);
    assert_eq!(std::fs::read_to_string(&path).expect("settings"), before);
    set_provider_if_identity(&path, remote.clone(), &selected.identity(), None).expect("apply");
    assert_eq!(selected_provider(&path).expect("provider"), remote);
}

/// A preflight of the very provider the settings select sends a real request
/// to the client a feature would ask, and is still no feature's use of it
/// (t-53792): a probe files no feature record, so the reading of that
/// client's real use says none was observed.
#[test]
fn a_preflight_of_the_selected_provider_is_not_read_as_its_use() {
    use crate::systemone::latest::{self, State};

    let home = tempfile::tempdir().expect("home");
    let path = home.path().join("settings.json");
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", preflight_answer().to_string(), 0);
    let selected = candidate(endpoint.base());
    set_provider(&path, selected.clone()).expect("select the served provider");
    let wire = settings_wire(&path);
    let preflight = preflight_provider(
        &path,
        selected.clone(),
        &selected.identity(),
        &HeldKeys::default(),
        &wire,
    )
    .expect("preflight");
    assert!(preflight.answered, "{preflight:?}");
    assert_eq!(endpoint.count(), 1);
    assert_eq!(requests_today(&path), 1, "the request left and was counted");
    let reading = latest::read(&wire, 1);
    assert_eq!(reading.provider_identity, Some(selected.identity()));
    assert_eq!(reading.state, State::NoneObserved, "{reading:?}");
    assert_eq!(reading.last_request, None);
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

#[test]
fn a_shared_connection_preflight_uses_its_own_key_headers_and_exact_route() {
    use judgment_provider::connections::{self, Reference};
    let home = tempfile::tempdir().expect("home");
    let path = home.path().join("settings.json");
    let (selected, _) = selected_with_managed_model(&path);
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", preflight_answer().to_string(), 0);
    let mut root: Value =
        serde_json::from_slice(&std::fs::read(&path).expect("read")).expect("json");
    root["providers"] = json!([{
        "name":"Fixture", "base_url":format!("{}/custom", endpoint.base()),
        "auth_env":"ZEROCODE_ROUTER_LINKED_FIXTURE_KEY", "requires_auth":true, "models":["fixture-model"],
        "headers":{"X-Fixture-Route":"shared"}, "user_agent":"fixture-client/1",
        "connection":{"id":"fixture-connection", "generation":3,"auth":{"header":"X-Fixture-Key", "scheme":null}}
    }]);
    std::fs::write(&path, root.to_string()).expect("write");
    let before = std::fs::read(&path).expect("before");
    let reference = Reference {
        id: "fixture-connection".to_string(),
        generation: 3,
        model: "fixture-model".to_string(),
        protocol: Protocol::SystemOne,
    };
    let candidate = connections::candidate(&root, &reference).expect("candidate");
    let keys = HeldKeys::default();
    keys.write(
        &provider_keychain_service(&candidate),
        "linked-fixture-value",
    )
    .expect("key");
    let mut legacy = candidate.clone();
    legacy.connection = None;
    keys.write(
        &provider_keychain_service(&legacy),
        "legacy-must-not-be-sent",
    )
    .expect("old key");
    let result = crate::model_connection_store::check(
        &path,
        &reference,
        &selected.identity(),
        &keys,
        &settings_wire(&path),
    )
    .expect("check");
    assert!(result.preflight.answered, "{:?}", result.preflight);
    assert!(result.legacy_key_present);
    let request = endpoint.asked()[0].to_ascii_lowercase();
    assert!(
        request.contains("/custom/systemone "),
        "exact router API base"
    );
    assert!(request.contains("x-fixture-key: linked-fixture-value"));
    assert!(request.contains("x-fixture-route: shared"));
    assert!(request.contains("user-agent: fixture-client/1"));
    assert!(!request.contains("legacy-must-not-be-sent"));
    assert!(!request.contains("authorization:"));
    assert_eq!(std::fs::read(&path).expect("unchanged"), before);
}

/// t-75762 ⑥, as the machine showed it: a connection whose key variable has
/// no window prefix is read from the window's own environment, which a
/// window started from the Dock never has — so its judgment check ended
/// `no_key` while the old judgment setting still kept a key for the same
/// address. The catalog says so before any request, and one press moves that
/// key into the connection, after which the check finds it.
#[test]
fn a_connection_takes_the_old_judgment_key_for_its_address_with_one_press() {
    use judgment_provider::connections::{self, Reference};
    let home = tempfile::tempdir().expect("home");
    let path = home.path().join("settings.json");
    let (selected, _) = selected_with_managed_model(&path);
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", preflight_answer().to_string(), 0);
    let mut root: Value =
        serde_json::from_slice(&std::fs::read(&path).expect("read")).expect("json");
    let foreign = "FIXTURE_T75762_FOREIGN_LLM_KEY";
    assert!(
        std::env::var_os(foreign).is_none(),
        "the fixture variable is set"
    );
    root["providers"] = json!([{
        "name":"Fixture gateway", "base_url":format!("{}/v1", endpoint.base()),
        "auth_env":foreign, "requires_auth":true, "models":["fixture-model"],
        "connection":{"id":"fixture-foreign", "generation":1}
    }]);
    std::fs::write(&path, root.to_string()).expect("write");
    let reference = Reference {
        id: "fixture-foreign".to_string(),
        generation: 1,
        model: "fixture-model".to_string(),
        protocol: Protocol::SystemOne,
    };
    let mut legacy = connections::candidate(&root, &reference).expect("candidate");
    legacy.connection = None;
    let keys = HeldKeys::default();
    keys.write(&provider_keychain_service(&legacy), "[합성-이전-판단-키]")
        .expect("old key");

    let before = crate::model_connection_store::catalog(&path, &keys).expect("catalog");
    let row = &before.connections[0];
    assert!(
        !row.key_available,
        "an unreadable variable was called a key"
    );
    assert!(
        row.legacy_key,
        "the old key for this address was not offered"
    );
    let refused = crate::model_connection_store::check(
        &path,
        &reference,
        &selected.identity(),
        &keys,
        &settings_wire(&path),
    )
    .expect("check");
    assert_eq!(refused.preflight.failure.as_deref(), Some("no_key"));
    assert!(
        endpoint.asked().is_empty(),
        "a keyless check reached the server"
    );

    crate::model_connection_store::adopt_legacy_judgment_key(&path, "fixture-foreign", 1, &keys)
        .expect("one press moves the key");
    let after = crate::model_connection_store::catalog(&path, &keys).expect("catalog");
    let row = &after.connections[0];
    assert!(row.key_available);
    assert!(!row.legacy_key, "the moved key is still offered");
    assert!(
        crate::api_routers::is_router_env(&row.provider.auth_env),
        "the row still names a variable the window cannot read"
    );
    assert!(
        !std::fs::read_to_string(&path)
            .expect("settings")
            .contains("[합성-이전-판단-키]"),
        "the key was written into the settings file"
    );
    let reference = Reference {
        generation: row.generation,
        ..reference
    };
    let answered = crate::model_connection_store::check(
        &path,
        &reference,
        &selected.identity(),
        &keys,
        &settings_wire(&path),
    )
    .expect("check");
    assert!(answered.preflight.answered, "{:?}", answered.preflight);
    assert!(endpoint.asked()[0].contains("[합성-이전-판단-키]"));
}

/// t-75762 시험 3: a connection model applied for judgment is the model
/// field of the next judgment request, which goes to that connection.
#[test]
fn an_applied_judgment_connection_model_is_the_next_requests_model() {
    use judgment_provider::connections::Reference;
    use zerocode_core::jev::JEV_USES;
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let path = home.path().join("settings.json");
    let (selected, _) = selected_with_managed_model(&path);
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", preflight_answer().to_string(), 0);
    let mut root: Value =
        serde_json::from_slice(&std::fs::read(&path).expect("read")).expect("json");
    root["providers"] = json!([{
        "name":"Fixture judge", "base_url":format!("{}/route", endpoint.base()),
        "requires_auth":false, "models":["fixture-other","fixture-judge-model"],
        "connection":{"id":"fixture-judge", "generation":4}
    }]);
    root["smart"]["jev"] =
        json!({"enabled":true,"workspaces":[door::resolved_path(workspace.path())]});
    let seat = &JEV_USES[0];
    root["smart"][seat.setting] = json!(
        seat.modes
            .iter()
            .find(|mode| mode.asks())
            .expect("asking mode")
            .key()
    );
    std::fs::write(&path, root.to_string()).expect("write");
    let reference = Reference {
        id: "fixture-judge".to_string(),
        generation: 4,
        model: "fixture-judge-model".to_string(),
        protocol: Protocol::SystemOne,
    };
    crate::model_connection_store::apply_judgment(
        &path,
        &reference,
        &selected.identity(),
        &HeldKeys::default(),
        &settings_wire(&path),
    )
    .expect("apply");
    let applied = endpoint.count();
    let _ = settings_wire(&path).ask(
        seat,
        Some(workspace.path()),
        crate::systemone::request_body(
            &json!("Ready"),
            &json!({"ready":{"type":"noul","instructions":"Ready?"}}),
        ),
        std::time::Duration::from_secs(5),
    );
    let asked = endpoint.asked();
    assert_eq!(
        asked.len(),
        applied + 1,
        "the next judgment did not reach the connection"
    );
    let (head, body) = asked[applied].split_once("\r\n\r\n").expect("request");
    assert!(head.starts_with("POST /route/systemone "), "{head}");
    let body: Value = serde_json::from_str(body).expect("JSON");
    assert_eq!(body["model"], "fixture-judge-model");
}

#[test]
fn returning_from_a_connection_restores_the_previous_managed_selection() {
    use judgment_provider::connections::{self, Reference};
    let home = tempfile::tempdir().expect("home");
    let path = home.path().join("settings.json");
    let (selected, _) = selected_with_managed_model(&path);
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", preflight_answer().to_string(), 0);
    let mut root: Value =
        serde_json::from_slice(&std::fs::read(&path).expect("read")).expect("json");
    let before_smart = root["smart"].clone();
    root["providers"] = json!([{"name":"Fixture", "base_url":endpoint.base(),"auth_env":"ZEROCODE_ROUTER_RESTORE_FIXTURE_KEY","requires_auth":false,"models":["fixture-model"],"connection":{"id":"restore-fixture","generation":0}}]);
    std::fs::write(&path, root.to_string()).expect("write");
    let reference = Reference {
        id: "restore-fixture".to_string(),
        generation: 0,
        model: "fixture-model".to_string(),
        protocol: Protocol::SystemOne,
    };
    crate::model_connection_store::apply_judgment(
        &path,
        &reference,
        &selected.identity(),
        &HeldKeys::default(),
        &settings_wire(&path),
    )
    .expect("apply");
    let applied = selected_provider(&path).expect("linked");
    assert!(applied.connection.is_some());
    let after: Value = serde_json::from_slice(&std::fs::read(&path).expect("read")).expect("json");
    // The old parser sees only its original five-field projection.
    let old: Provider =
        serde_json::from_value(after["smart"][judgment_provider::SETTINGS_KEY].clone())
            .expect("legacy parser");
    assert!(old.connection.is_none());
    assert_eq!(
        after["smart"][judgment_provider::SETTINGS_KEY]
            .as_object()
            .expect("projection")
            .len(),
        5
    );
    crate::model_connection_store::clear(
        &path,
        crate::model_connection_store::Role::Judgment,
        Some(&reference),
    )
    .expect("restore");
    let restored: Value =
        serde_json::from_slice(&std::fs::read(&path).expect("read")).expect("json");
    // The selection is restored as it was; the connection's own format is
    // no part of it and stays, for the next time it is picked (t-76211).
    let mut selection = restored["smart"].clone();
    let remembered = selection
        .as_object_mut()
        .expect("smart")
        .remove("judgmentFormats");
    assert_eq!(selection, before_smart);
    assert_eq!(remembered, Some(json!({"restore-fixture": "system_one"})));
    assert_eq!(selected_provider(&path).expect("managed again"), selected);
    assert!(
        restored["smart"]
            .get(connections::JUDGMENT_CONNECTION_KEY)
            .is_none()
    );
}

/// t-76211: a connection's judgment format is kept with the settings it was
/// applied in, by the connection. Judgment applied on A in System One, then
/// moved to B: a fresh read of the settings — what a restarted window sees —
/// still says A was used in System One, while a connection never applied to
/// judgment says nothing. Before, only the connection judgment named now
/// carried a format, and the page asked A in OpenAI JSON after a restart.
#[test]
fn a_connections_judgment_format_outlives_judgment_moving_elsewhere_and_a_restart() {
    use judgment_provider::connections::Reference;
    let home = tempfile::tempdir().expect("home");
    let path = home.path().join("settings.json");
    let (selected, _) = selected_with_managed_model(&path);
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", preflight_answer().to_string(), 0);
    let mut root: Value =
        serde_json::from_slice(&std::fs::read(&path).expect("read")).expect("json");
    let row = |name: &str, id: &str| json!({"name":name,"base_url":endpoint.base(),"auth_env":"ZEROCODE_ROUTER_FORMAT_FIXTURE_KEY","requires_auth":false,"models":["fixture-model"],"connection":{"id":id,"generation":0}});
    root["providers"] = json!([
        row("First", "format-a"),
        row("Second", "format-b"),
        row("Third", "format-c")
    ]);
    std::fs::write(&path, root.to_string()).expect("write");
    let reference = |id: &str| Reference {
        id: id.to_string(),
        generation: 0,
        model: "fixture-model".to_string(),
        protocol: Protocol::SystemOne,
    };
    crate::model_connection_store::apply_judgment(
        &path,
        &reference("format-a"),
        &selected.identity(),
        &HeldKeys::default(),
        &settings_wire(&path),
    )
    .expect("apply A");
    let on_a = selected_provider(&path).expect("A judges");
    crate::model_connection_store::apply_judgment(
        &path,
        &reference("format-b"),
        &on_a.identity(),
        &HeldKeys::default(),
        &settings_wire(&path),
    )
    .expect("apply B");

    let catalog =
        crate::model_connection_store::catalog(&path, &HeldKeys::default()).expect("catalog");
    let format = |id: &str| {
        catalog
            .connections
            .iter()
            .find(|row| row.id == id)
            .map(|row| row.judgment_format)
            .expect("the row")
    };
    assert_eq!(
        format("format-a"),
        Some(Protocol::SystemOne),
        "A's format was forgotten when judgment moved to B"
    );
    assert_eq!(format("format-b"), Some(Protocol::SystemOne));
    assert_eq!(
        format("format-c"),
        None,
        "a connection never applied has no format"
    );
}

#[test]
fn a_linked_key_is_captured_under_the_settings_lock_and_never_for_a_stale_route() {
    use judgment_provider::connections::{self, Reference};
    struct LockedKeys<'a> {
        path: &'a std::path::Path,
        reads: std::cell::Cell<usize>,
    }
    impl RouterKeys for LockedKeys<'_> {
        fn read(&self, _service: &str) -> Result<Option<String>, RouterRefusal> {
            assert!(
                self.path.with_extension("json.lock").exists(),
                "the endpoint and credential must be one locked snapshot"
            );
            self.reads.set(self.reads.get() + 1);
            Ok(Some("fixture-bound-key".to_string()))
        }
        fn write(&self, _service: &str, _value: &str) -> Result<(), RouterRefusal> {
            panic!("read-only capture");
        }
        fn delete(&self, _service: &str) -> Result<(), RouterRefusal> {
            panic!("read-only capture");
        }
    }
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("settings.json");
    let mut root = json!({"providers":[{"name":"Fixture","base_url":"https://gateway.example.com/v1","auth_env":"ZEROCODE_ROUTER_CAPTURE_KEY","requires_auth":true,"models":["fixture-model"],"connection":{"id":"capture","generation":1}}]});
    std::fs::write(&path, root.to_string()).unwrap();
    let reference = Reference {
        id: "capture".to_string(),
        generation: 1,
        model: "fixture-model".to_string(),
        protocol: Protocol::SystemOne,
    };
    let provider = connections::candidate(&root, &reference).unwrap();
    let keys = LockedKeys {
        path: &path,
        reads: std::cell::Cell::new(0),
    };
    assert_eq!(
        provider_key_at(&path, &provider, &keys).unwrap().as_deref(),
        Some("fixture-bound-key")
    );
    root["providers"][0]["base_url"] = json!("https://different.example.com/v1");
    std::fs::write(&path, root.to_string()).unwrap();
    assert_eq!(
        provider_key_at(&path, &provider, &keys)
            .unwrap_err()
            .message,
        "settings_changed"
    );
    assert_eq!(
        keys.reads.get(),
        1,
        "a stale destination reached the credential store"
    );
}

#[test]
fn a_captured_window_key_cannot_follow_a_reused_reference_to_another_endpoint() {
    use judgment_provider::connections::{self, Reference};
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("settings.json");
    let mut root = json!({"providers":[{"name":"Fixture","base_url":"https://gateway.example.com/v1","auth_env":"ZEROCODE_ROUTER_PINNED_KEY","requires_auth":true,"models":["fixture-model"],"connection":{"id":"pinned","generation":1}}]});
    let reference = Reference {
        id: "pinned".to_string(),
        generation: 1,
        model: "fixture-model".to_string(),
        protocol: Protocol::SystemOne,
    };
    let provider = connections::candidate(&root, &reference).unwrap();
    root["smart"] = json!({"judgmentProvider":provider,"judgmentConnection":reference});
    std::fs::write(&path, root.to_string()).unwrap();
    let keys = HeldKeys::default();
    keys.write(
        &provider_keychain_service(&provider),
        "captured-fixture-key",
    )
    .unwrap();
    let wire = crate::systemone::Wire::reading(&keys, Some(path.clone()));
    assert!(wire.armed());
    root["providers"][0]["base_url"] = json!("https://different.example.com/v1");
    root["smart"]["judgmentProvider"]["baseUrl"] = json!("https://different.example.com/v1");
    std::fs::write(&path, root.to_string()).unwrap();
    assert!(
        !wire.armed(),
        "a captured key was reused for the changed endpoint"
    );
}

/// A judgment provider bound to a saved connection is named by that
/// connection's own name (t-76598): the dashboard and the settings say
/// 「Fixture」 where they once said the provider's `judgment-…` identity.
#[test]
fn a_connection_bound_judgment_provider_carries_the_connections_name() {
    use judgment_provider::connections::{self, Reference};
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("settings.json");
    let mut root = json!({"providers":[{"name":"Fixture Router","base_url":"https://gateway.example.com/v1","auth_env":"ZEROCODE_ROUTER_NAMED_KEY","requires_auth":true,"models":["fixture-model"],"connection":{"id":"named","generation":1}}]});
    let reference = Reference {
        id: "named".to_string(),
        generation: 1,
        model: "fixture-model".to_string(),
        protocol: Protocol::OpenaiJson,
    };
    let provider = connections::candidate(&root, &reference).unwrap();
    root["smart"] = json!({"judgmentProvider":provider,"judgmentConnection":reference});
    std::fs::write(&path, root.to_string()).unwrap();
    let state = read_settings(&path, &HeldKeys::default(), true).expect("settings");
    assert_eq!(
        state.provider_connection_name.as_deref(),
        Some("Fixture Router")
    );
    assert!(state.provider_identity.starts_with("judgment-"));

    std::fs::write(&path, "{}").unwrap();
    let state = read_settings(&path, &HeldKeys::default(), true).expect("settings");
    assert_eq!(
        state.provider_connection_name, None,
        "Jev has no saved connection"
    );
}
