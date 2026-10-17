use serde_json::json;

use super::*;

fn local() -> Provider {
    Provider {
        kind: ProviderKind::Local,
        protocol: Protocol::SystemOne,
        base_url: "http://127.0.0.1:1234/v1".to_string(),
        model: "acme/small-judge".to_string(),
        requires_auth: false,
    }
}

fn questions() -> Value {
    json!({
        "route":{"type":"choice","instructions":"Choose the requested destination.","criteria":{"home":"Home page","help":"Help page"}},
        "urgency":{"type":"score","instructions":"Rate urgency.","criteria":["routine","urgent"]},
        "ready":{"type":"noul","instructions":"Is the page ready?"}
    })
}

fn request(provider: &Provider) -> Vec<u8> {
    json!({"model":provider.identity(),"state":"Open help; ready and routine.","questions":questions()}).to_string().into_bytes()
}

fn response() -> Value {
    json!({
        "model":"acme/small-judge",
        "answers": {
            "route":{"type":"choice","choice":"help","probabilities":{"help":0.9,"home":0.1},"confidence":0.7},
            "urgency":{"type":"score","score":0.1,"probabilities":{"0":0.9,"1":0.1},"legend":{"0":"routine","1":"urgent"},"confidence":0.7},
            "ready":{"type":"noul","noul":0.95}
        },
        "usage":{"input_tokens":100,"output_tokens":0}
    })
}

#[test]
fn an_existing_installation_keeps_jev_and_its_model_pin() {
    assert_eq!(
        Provider::from_root(&json!({"smart":{"jevModel":"jev-1.13.0"}})),
        Ok(Provider::jev("jev-1.13.0"))
    );
}

#[test]
fn a_malformed_custom_configuration_never_falls_back_to_cloud() {
    assert_eq!(
        Provider::from_root(&json!({"smart":{"judgmentProvider":{"kind":"unknown"}}})),
        Err(Refusal::Settings)
    );
}

#[test]
fn a_general_chat_protocol_is_not_a_decision_provider() {
    let mut config = serde_json::to_value(local()).expect("profile");
    config["protocol"] = json!("open_ai");
    assert_eq!(
        Provider::from_root(&json!({"smart":{"judgmentProvider":config}})),
        Err(Refusal::Settings)
    );
}

#[test]
fn a_local_provider_needs_no_jev_key() {
    let provider = local().validated().expect("local provider");
    assert!(provider.ready_with_key(None));
    assert_ne!(provider.key_env(), JEV_KEY_ENV);
}

#[test]
fn an_endpoint_can_be_configured_before_choosing_a_model() {
    let mut provider = local();
    provider.model.clear();
    assert!(provider.clone().endpoint_validated().is_ok());
    assert_eq!(provider.validated(), Err(Refusal::Model));
}

#[test]
fn malformed_usage_is_not_a_valid_decision_response() {
    let provider = local();
    let prepared = provider.prepare(request(&provider)).expect("request");
    for usage in [
        Value::Null,
        json!([]),
        json!({}),
        json!({"input_tokens":-1,"output_tokens":0}),
        json!({"input_tokens":1,"output_tokens":"0"}),
        json!({"input_tokens":1,"output_tokens":0,"truncated":"true"}),
    ] {
        let mut answer = response();
        answer["usage"] = usage;
        assert_eq!(
            provider.decode(&prepared, answer.to_string().into_bytes()),
            Err(Refusal::Response)
        );
    }
}

#[test]
fn a_truncated_question_cannot_hide_behind_an_untruncated_state() {
    let provider = local();
    let prepared = provider.prepare(request(&provider)).expect("request");
    let mut answer = response();
    answer["usage"]["truncated"] = json!(false);
    answer["usage"]["truncated_questions"] = json!(["route"]);
    assert_eq!(
        provider.decode(&prepared, answer.to_string().into_bytes()),
        Err(Refusal::Truncated)
    );
}

#[test]
fn a_key_is_scoped_to_its_server_not_the_model_selected_there() {
    let provider = local();
    let mut changed = provider.clone();
    changed.model = "acme/next-model".to_string();
    assert_eq!(provider.key_env(), changed.key_env());
    changed.base_url = "http://127.0.0.1:4321/v1".to_string();
    assert_ne!(provider.key_env(), changed.key_env());
}

#[test]
fn learned_evidence_is_scoped_to_each_decision_model_and_server() {
    let base = Path::new("evidence");
    assert_eq!(Provider::jev(JEV_DEFAULT_MODEL).evidence_dir(base), base);
    let provider = local();
    let held = provider.evidence_dir(base);
    assert!(held.starts_with(base.join("providers")));
    let mut changed = provider.clone();
    changed.model = "next-decision-model".to_string();
    assert_ne!(changed.evidence_dir(base), held);
    changed.model = provider.model;
    changed.base_url = "http://127.0.0.1:4321".to_string();
    assert_ne!(changed.evidence_dir(base), held);
}

#[test]
fn jev_still_requires_its_own_key() {
    assert!(!Provider::jev(JEV_DEFAULT_MODEL).ready_with_key(None));
}

#[test]
fn local_auth_is_an_explicit_requirement_not_a_dummy_key() {
    let mut provider = local();
    provider.requires_auth = true;
    assert!(!provider.ready_with_key(Some("  ")));
}

#[test]
fn local_addresses_cannot_name_an_external_server() {
    for address in [
        "https://example.com/v1",
        "http://localhost.example.com/v1",
        "http://192.0.2.1:8000/v1",
    ] {
        let mut provider = local();
        provider.base_url = address.to_string();
        assert_eq!(provider.validated(), Err(Refusal::LocalAddress));
    }
}

#[test]
fn addresses_have_a_byte_ceiling_and_cannot_hide_control_characters() {
    for address in [
        format!("http://127.0.0.1/{}", "x".repeat(MAX_ADDRESS_BYTES)),
        "http://local\nhost:1234".to_string(),
    ] {
        let mut provider = local();
        provider.base_url = address;
        assert_eq!(provider.validated(), Err(Refusal::Address));
    }
}

#[test]
fn both_loopback_address_families_are_supported() {
    for address in [
        "http://127.0.0.1:1234",
        "http://[::1]:1234",
        "http://localhost:1234",
    ] {
        let mut provider = local();
        provider.base_url = address.to_string();
        assert!(provider.validated().is_ok(), "{address}");
    }
}

#[test]
fn credentials_queries_and_fragments_are_not_endpoint_configuration() {
    for address in [
        "http://dev:fixture@localhost:1234/v1",
        "http://localhost:1234/v1?secret=fixture",
        "http://localhost:1234/v1#fixture",
        "file:///tmp/model",
    ] {
        let mut provider = local();
        provider.base_url = address.to_string();
        assert_eq!(provider.validated(), Err(Refusal::Address));
    }
}

#[test]
fn a_remote_provider_requires_tls() {
    let mut provider = local();
    provider.kind = ProviderKind::Remote;
    provider.base_url = "http://example.com/v1".to_string();
    assert_eq!(provider.validated(), Err(Refusal::RemoteAddress));
}

#[test]
fn an_api_root_does_not_repeat_its_version_component() {
    assert_eq!(local().request_url(), "http://127.0.0.1:1234/v1/systemone");
}

#[test]
fn a_reverse_proxy_prefix_is_preserved() {
    let mut provider = local();
    provider.base_url = "http://127.0.0.1:1234/decision".to_string();
    assert_eq!(
        provider.request_url(),
        "http://127.0.0.1:1234/decision/v1/systemone"
    );
}

#[test]
fn switching_a_model_or_endpoint_changes_the_evidence_identity() {
    let provider = local();
    let mut different = provider.clone();
    different.model = "acme/another-judge".to_string();
    assert_ne!(provider.identity(), different.identity());
    different = provider.clone();
    different.base_url = "http://127.0.0.1:4321/v1".to_string();
    assert_ne!(provider.identity(), different.identity());
}

#[test]
fn a_route_fingerprint_follows_every_part_of_the_route_for_every_kind() {
    for kind in [ProviderKind::Jev, ProviderKind::Local, ProviderKind::Remote] {
        let mut provider = local();
        provider.kind = kind;
        let held = provider.route_fingerprint();
        assert_eq!(provider.clone().route_fingerprint(), held, "{kind:?}");
        let mut moved = provider.clone();
        moved.base_url = "http://127.0.0.1:4321/v1".to_string();
        assert_ne!(moved.route_fingerprint(), held, "{kind:?} address");
        moved = provider.clone();
        moved.model = "acme/another-judge".to_string();
        assert_ne!(moved.route_fingerprint(), held, "{kind:?} model");
        moved = provider.clone();
        moved.requires_auth = !provider.requires_auth;
        assert_ne!(moved.route_fingerprint(), held, "{kind:?} auth");
    }
    let mut remote = local();
    remote.kind = ProviderKind::Remote;
    assert_ne!(remote.route_fingerprint(), local().route_fingerprint());
}

#[test]
fn a_route_fingerprint_spells_no_address_model_or_identity() {
    let mut overridden = Provider::jev(JEV_DEFAULT_MODEL);
    overridden.base_url = "http://127.0.0.1:1234".to_string();
    for provider in [local(), Provider::jev(JEV_DEFAULT_MODEL), overridden] {
        let route = provider.route_fingerprint();
        let digits = route.strip_prefix("route-").expect("a route fingerprint");
        assert_eq!(digits.len(), 32, "{route}");
        assert!(
            digits
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        );
        assert!(!route.contains(&provider.base_url));
        assert!(!route.contains(&provider.model));
        assert_ne!(route, provider.identity());
    }
}

#[test]
fn the_identity_keeps_its_meaning_beside_the_route() {
    let mut overridden = Provider::jev("jev-1.13.0");
    assert_eq!(overridden.identity(), "jev-1.13.0");
    let held = overridden.route_fingerprint();
    overridden.base_url = "http://127.0.0.1:1234".to_string();
    assert_eq!(overridden.identity(), "jev-1.13.0");
    assert_ne!(overridden.route_fingerprint(), held);
    assert_eq!(
        Provider::jev(JEV_DEFAULT_MODEL).evidence_dir(Path::new("evidence")),
        Path::new("evidence")
    );
}

#[test]
fn changing_auth_requirements_cannot_reuse_the_old_judgment_identity() {
    let provider = local();
    let mut different = provider.clone();
    different.requires_auth = true;
    assert_ne!(provider.identity(), different.identity());
}

#[test]
fn discovery_uses_server_model_ids_not_a_product_model_table() {
    let expected = vec![Model {
        id: "acme/new-model".to_string(),
        limits: None,
    }];
    for body in [
        json!({"data":[{"id":"acme/new-model"}]}),
        json!({"models":[{"name":"acme/new-model"}]}),
        json!({"models":["acme/new-model"]}),
    ] {
        assert_eq!(
            read_models(body.to_string().as_bytes()),
            Ok(expected.clone())
        );
    }
}

#[test]
fn malformed_model_lists_are_not_reported_as_empty_successes() {
    assert_eq!(
        read_models(br#"{"error":"unavailable"}"#),
        Err(Refusal::Response)
    );
}

#[test]
fn model_list_size_is_bounded() {
    let body = json!({"data":vec![json!({"id":"fixture"}); MAX_MODELS + 1]});
    assert_eq!(
        read_models(body.to_string().as_bytes()),
        Err(Refusal::TooLarge)
    );
}

fn declared(model: &str, tokens: Value) -> Value {
    json!({"model":model,"runtime":"runtime-fixture","generation":"server-fixture","tokens":tokens})
}

#[test]
fn discovery_keeps_the_limits_a_server_declared_as_data() {
    for tokens in [
        json!({"max_len":1024,"head_max_len":256}),
        json!({"max_len":2048,"head_max_len":300}),
        json!({"max_len":1024}),
    ] {
        let limits = declared("acme/judge", tokens);
        for body in [
            json!({"data":[{"id":"acme/judge","limits":limits}]}),
            json!({"models":[{"name":"acme/judge","limits":limits}]}),
        ] {
            let models = read_models(body.to_string().as_bytes()).expect("model list");
            assert_eq!(
                serde_json::to_value(&models).expect("JSON"),
                json!([{"id":"acme/judge","limits":limits}]),
                "the limits a server declared were not kept as data"
            );
        }
    }
}

#[test]
fn unusable_limits_declare_nothing() {
    let mut bodies = vec![
        json!({"data":[{"id":"acme/judge"}]}),
        json!({"models":["acme/judge"]}),
        json!({"data":[
            {"id":"acme/judge","limits":declared("acme/judge", json!({"max_len":1024}))},
            {"id":"acme/judge","limits":declared("acme/judge", json!({"max_len":2048}))},
        ]}),
    ];
    for limits in [
        declared("acme/judge", json!({"max_len":0,"head_max_len":256})),
        declared("acme/judge", json!({"max_len":1024,"head_max_len":0})),
        declared("acme/judge", json!({"max_len":-1024})),
        declared("acme/judge", json!({"max_len":"1024"})),
        declared("acme/judge", json!({"max_len":true})),
        declared("acme/judge", json!({"max_len":1024.5})),
        declared("acme/judge", json!({"head_max_len":256})),
        declared(
            "acme/other-judge",
            json!({"max_len":1024,"head_max_len":256}),
        ),
        json!({"model":"acme/judge","runtime":"","generation":"server-fixture","tokens":{"max_len":1024}}),
        json!({"model":"acme/judge","runtime":"runtime-fixture","generation":"","tokens":{"max_len":1024}}),
        json!({"model":"acme/judge","runtime":"runtime-fixture","tokens":{"max_len":1024}}),
        json!({"tokens":{"max_len":1024}}),
        json!(1024),
        Value::Null,
    ] {
        bodies.push(json!({"data":[{"id":"acme/judge","limits":limits}]}));
    }
    for body in bodies {
        let models = read_models(body.to_string().as_bytes()).expect("model list");
        assert_eq!(
            serde_json::to_value(&models).expect("JSON"),
            json!([{"id":"acme/judge"}]),
            "{body} declared a limit"
        );
    }
}

#[test]
fn existing_jev_requests_are_not_rewritten() {
    let provider = Provider::jev(JEV_DEFAULT_MODEL);
    let original = b"{\"model\":\"jev-latest\", \"state\":\"fixture\", \"questions\":{}}".to_vec();
    assert_eq!(
        provider
            .prepare(original.clone())
            .expect("existing wire")
            .body,
        original
    );
}

#[test]
fn all_three_decision_types_share_one_native_request() {
    let provider = local();
    let prepared = provider.prepare(request(&provider)).expect("prepared");
    let wire: Value = serde_json::from_slice(&prepared.body).expect("JSON");
    assert_eq!(wire["model"], provider.model);
    assert_eq!(wire["questions"], questions());
    assert!(wire.get("messages").is_none());
}

#[test]
fn a_selected_provider_change_refuses_an_old_prepared_goal() {
    let provider = local();
    let old = request(&provider);
    let mut changed = provider.clone();
    changed.model = "acme/new-judge".to_string();
    assert!(matches!(
        changed.prepare(old),
        Err(Refusal::SettingsChanged)
    ));
}

#[test]
fn native_probabilities_and_usage_are_preserved_not_manufactured() {
    let provider = local();
    let prepared = provider.prepare(request(&provider)).expect("prepared");
    let decoded = provider
        .decode(&prepared, response().to_string().into_bytes())
        .expect("typed response");
    let decoded: Value = serde_json::from_str(&decoded).expect("JSON");
    assert_eq!(decoded["answers"], response()["answers"]);
    assert_eq!(decoded["usage"], response()["usage"]);
    assert!(
        decoded["model"]
            .as_str()
            .expect("identity")
            .starts_with(&provider.identity())
    );
}

#[test]
fn a_model_cannot_add_an_unoffered_action() {
    let provider = local();
    let prepared = provider.prepare(request(&provider)).expect("prepared");
    let mut proposed = response();
    proposed["answers"]["route"]["choice"] = json!("run_shell");
    assert_eq!(
        provider.decode(&prepared, proposed.to_string().into_bytes()),
        Err(Refusal::Response)
    );
}

#[test]
fn partial_questions_are_not_a_success() {
    let provider = local();
    let prepared = provider.prepare(request(&provider)).expect("prepared");
    let mut proposed = response();
    proposed["answers"]
        .as_object_mut()
        .expect("answers")
        .remove("ready");
    assert_eq!(
        provider.decode(&prepared, proposed.to_string().into_bytes()),
        Err(Refusal::Response)
    );
}

#[test]
fn out_of_range_probabilities_are_refused() {
    let provider = local();
    let prepared = provider.prepare(request(&provider)).expect("prepared");
    let mut proposed = response();
    proposed["answers"]["ready"]["noul"] = json!(1.5);
    assert_eq!(
        provider.decode(&prepared, proposed.to_string().into_bytes()),
        Err(Refusal::Response)
    );
}

#[test]
fn missing_confidence_is_not_replaced_with_the_chosen_probability() {
    let provider = local();
    let prepared = provider.prepare(request(&provider)).expect("prepared");
    let mut proposed = response();
    proposed["answers"]["route"]
        .as_object_mut()
        .expect("choice")
        .remove("confidence");
    assert_eq!(
        provider.decode(&prepared, proposed.to_string().into_bytes()),
        Err(Refusal::Response)
    );
}

#[test]
fn a_distribution_that_does_not_sum_to_one_is_refused() {
    let provider = local();
    let prepared = provider.prepare(request(&provider)).expect("prepared");
    let mut proposed = response();
    proposed["answers"]["route"]["probabilities"]["home"] = json!(0.8);
    assert_eq!(
        provider.decode(&prepared, proposed.to_string().into_bytes()),
        Err(Refusal::Response)
    );
}

#[test]
fn a_score_must_agree_with_its_distribution() {
    let provider = local();
    let prepared = provider.prepare(request(&provider)).expect("prepared");
    let mut proposed = response();
    proposed["answers"]["urgency"]["score"] = json!(0.9);
    assert_eq!(
        provider.decode(&prepared, proposed.to_string().into_bytes()),
        Err(Refusal::Response)
    );
}

#[test]
fn silently_dropped_state_is_not_evidence_for_an_action() {
    let provider = local();
    let prepared = provider.prepare(request(&provider)).expect("prepared");
    let mut proposed = response();
    proposed["usage"]["truncated"] = json!(true);
    assert_eq!(
        provider.decode(&prepared, proposed.to_string().into_bytes()),
        Err(Refusal::Truncated)
    );
}

#[test]
fn plain_chat_output_cannot_masquerade_as_typed_decisions() {
    let provider = local();
    let prepared = provider.prepare(request(&provider)).expect("prepared");
    assert_eq!(
        provider.decode(
            &prepared,
            br#"{"choices":[{"message":{"content":"done"}}]}"#.to_vec()
        ),
        Err(Refusal::Response)
    );
}

#[test]
fn an_oversized_answer_is_rejected_before_json_parsing() {
    let provider = local();
    let prepared = provider.prepare(request(&provider)).expect("prepared");
    assert_eq!(
        provider.decode(&prepared, vec![b' '; MAX_RESPONSE_BYTES + 1]),
        Err(Refusal::TooLarge)
    );
}
