use super::*;
use crate::api_routers::HeldKeys;
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
