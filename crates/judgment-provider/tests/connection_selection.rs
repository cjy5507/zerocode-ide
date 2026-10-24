use judgment_provider::{Provider, Refusal};
use serde_json::{Value, json};

fn selected(id: &str, key_ref: &str) -> Value {
    json!({
        "providers": [{
            "name": "Fixture connection", "base_url": "https://gateway.example.com/api",
            "auth_env": key_ref, "requires_auth": true, "models": ["fixture-model"],
            "connection": {"id": id, "generation": 2}
        }],
        "smart": {
            "judgmentProvider": {"kind":"remote", "protocol":"openai_json",
                "baseUrl":"https://gateway.example.com/api", "model":"fixture-model", "requiresAuth":true},
            "judgmentConnection": {"id":id, "generation":2, "model":"fixture-model", "protocol":"openai_json"}
        }
    })
}

#[test]
fn a_linked_judgment_reads_only_its_connections_key_reference() {
    let provider = Provider::from_root(&selected("connection-a", "ZEROCODE_ROUTER_FIXTURE_A_KEY"))
        .expect("selected");
    assert_eq!(provider.key_env(), "ZEROCODE_ROUTER_FIXTURE_A_KEY");
}

#[test]
fn two_accounts_at_one_endpoint_have_distinct_judgment_evidence() {
    let a =
        Provider::from_root(&selected("connection-a", "ZEROCODE_ROUTER_FIXTURE_A_KEY")).expect("a");
    let b =
        Provider::from_root(&selected("connection-b", "ZEROCODE_ROUTER_FIXTURE_B_KEY")).expect("b");
    assert_ne!(
        a.identity(),
        b.identity(),
        "account changes must invalidate answers and late checks"
    );
    assert_ne!(a.route_fingerprint(), b.route_fingerprint());
}

#[test]
fn a_linked_judgment_uses_the_connections_exact_api_base() {
    let provider = Provider::from_root(&selected("connection-a", "ZEROCODE_ROUTER_FIXTURE_A_KEY"))
        .expect("selected");
    assert_eq!(
        provider.request_url(),
        "https://gateway.example.com/api/chat/completions"
    );
}

#[test]
fn a_missing_or_changed_connection_never_falls_back_to_the_legacy_key() {
    let mut root = selected("connection-a", "ZEROCODE_ROUTER_FIXTURE_A_KEY");
    root["providers"][0]["connection"]["generation"] = json!(3);
    assert_eq!(Provider::from_root(&root), Err(Refusal::SettingsChanged));
    root["providers"] = json!([]);
    assert_eq!(Provider::from_root(&root), Err(Refusal::SettingsChanged));
}

#[test]
fn a_connection_reference_cannot_redirect_the_selected_credentials() {
    let mut root = selected("connection-a", "ZEROCODE_ROUTER_FIXTURE_A_KEY");
    root["providers"][0]["base_url"] = json!("https://different.example.com/api");
    assert_eq!(Provider::from_root(&root), Err(Refusal::SettingsChanged));
}

#[test]
fn a_linked_provider_keeps_the_five_field_legacy_projection() {
    let root = selected("connection-a", "ZEROCODE_ROUTER_FIXTURE_A_KEY");
    let provider = Provider::from_root(&root).expect("selected");
    assert_eq!(
        serde_json::to_value(provider).expect("serialized"),
        root["smart"]["judgmentProvider"]
    );
}
