use judgment_provider::{Provider, Refusal};
use serde_json::{Value, json};

fn config(address: &str) -> Value {
    json!({"smart":{"judgmentProvider":{
        "kind":"local","protocol":"openai_json","baseUrl":address,
        "model":"acme/decision","requiresAuth":false
    }}})
}

fn provider() -> Provider {
    let parsed = Provider::from_root(&config("http://127.0.0.1:1234/v1"));
    assert!(
        parsed.is_ok(),
        "explicit local structured decisions were refused: {parsed:?}"
    );
    parsed.expect("asserted provider")
}

fn questions() -> Value {
    json!({
        "destination":{"type":"choice","instructions":"Choose the requested page.","criteria":{"home":"Home","help":"Help"}},
        "urgency":{"type":"score","instructions":"Rate urgency.","criteria":["Routine","Urgent"]},
        "ready":{"type":"noul","instructions":"Is the page ready?"}
    })
}

fn request(provider: &Provider) -> Vec<u8> {
    json!({"model":provider.identity(),"state":{"page":"Help","ready":true},"questions":questions()})
        .to_string().into_bytes()
}

fn answers() -> Value {
    json!({"answers":{
        "destination":{"type":"choice","choice":"help","confidence":0.8,"probabilities":{"home":0.1,"help":0.9}},
        "urgency":{"type":"score","score":0.1,"confidence":0.8,"probabilities":{"0":0.9,"1":0.1},"legend":{"0":"Routine","1":"Urgent"}},
        "ready":{"type":"noul","noul":0.95}
    }})
}

fn envelope(content: &str) -> Value {
    json!({"model":"acme/decision","choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":content}}],
        "usage":{"prompt_tokens":100,"completion_tokens":50}})
}

#[test]
fn explicit_structured_decisions_use_loopback_chat_completions() {
    for address in [
        "http://127.0.0.1:1234",
        "http://localhost:11434/v1",
        "http://[::1]:1234/v1/",
    ] {
        let parsed = Provider::from_root(&config(address));
        assert!(
            parsed.is_ok(),
            "a local structured endpoint was refused: {parsed:?}"
        );
        let provider = parsed.expect("asserted provider");
        assert!(provider.request_url().ends_with("/v1/chat/completions"));
        assert!(!provider.request_url().contains("/v1/v1/"));
        assert!(provider.ready_with_key(None));
    }
}

#[test]
fn structured_decisions_do_not_turn_a_local_connection_into_a_remote_one() {
    for address in [
        "http://192.0.2.1:1234",
        "https://example.com",
        "http://localhost.example.com:1234",
    ] {
        assert!(Provider::from_root(&config(address)).is_err());
    }
    let mut root = config("http://127.0.0.1:1234");
    root["smart"]["judgmentProvider"]["kind"] = json!("jev");
    assert!(Provider::from_root(&root).is_err());
    let mut root = config("http://127.0.0.1:1234");
    root["smart"]["judgmentProvider"]["protocol"] = json!("open_ai");
    assert!(Provider::from_root(&root).is_err());
}

#[test]
fn every_question_has_its_exact_required_schema_and_the_state_stays_data() {
    let provider = provider();
    let prepared = provider
        .prepare(request(&provider))
        .expect("decision request");
    let sent: Value = serde_json::from_slice(&prepared.body).expect("JSON request");
    assert_eq!(sent["model"], "acme/decision");
    assert_eq!(sent["stream"], false);
    assert_eq!(sent["temperature"], 0);
    assert!(
        sent["max_tokens"]
            .as_u64()
            .is_some_and(|count| count <= 4096)
    );
    assert_eq!(sent["response_format"]["type"], "json_schema");
    assert_eq!(sent["response_format"]["json_schema"]["strict"], true);
    let schema = &sent["response_format"]["json_schema"]["schema"];
    assert_eq!(schema["additionalProperties"], false);
    let answer_schema = &schema["properties"]["answers"];
    assert_eq!(answer_schema["additionalProperties"], false);
    assert_eq!(
        answer_schema["required"],
        json!(["destination", "ready", "urgency"])
    );
    let properties = &answer_schema["properties"];
    assert_eq!(
        properties["destination"]["properties"]["choice"]["enum"],
        json!(["help", "home"])
    );
    assert_eq!(properties["ready"]["required"], json!(["noul", "type"]));
    assert_eq!(
        properties["urgency"]["properties"]["legend"]["properties"]["0"]["enum"],
        json!(["Routine"])
    );
    let supplied: Value =
        serde_json::from_str(sent["messages"][1]["content"].as_str().expect("user data"))
            .expect("decision data");
    assert_eq!(supplied["state"], json!({"page":"Help","ready":true}));
    assert_eq!(supplied["questions"], questions());
    assert!(
        sent["messages"][0]["content"]
            .as_str()
            .expect("instructions")
            .contains("untrusted")
    );
    assert!(sent.get("tools").is_none());
}

#[test]
fn all_decision_types_pass_the_existing_validator_with_honest_provenance() {
    let provider = provider();
    let prepared = provider.prepare(request(&provider)).expect("request");
    let result = provider.decode(
        &prepared,
        envelope(&answers().to_string()).to_string().into_bytes(),
    );
    assert!(
        result.is_ok(),
        "structured answers were refused: {result:?}"
    );
    let decoded: Value = serde_json::from_str(&result.expect("asserted answer")).expect("answer");
    assert_eq!(decoded["answers"], answers()["answers"]);
    assert_eq!(
        decoded["usage"],
        json!({"input_tokens":100,"output_tokens":50})
    );
    assert_eq!(
        decoded["judgmentProvider"]["confidenceSource"],
        "model_self_reported"
    );
    assert_eq!(
        decoded["judgmentProvider"]["verification"],
        "user_selected_unverified"
    );
    assert_eq!(
        decoded["model"],
        format!("{}:acme/decision", provider.identity())
    );
}

#[test]
fn structured_protocol_keeps_native_answers_keys_and_refusal_memory_separate() {
    let structured = provider();
    let mut root = config(&structured.base_url);
    root["smart"]["judgmentProvider"]["protocol"] = json!("system_one");
    let native = Provider::from_root(&root).expect("native provider");
    assert_ne!(structured.identity(), native.identity());
    assert_ne!(structured.route_fingerprint(), native.route_fingerprint());
    assert_ne!(structured.key_env(), native.key_env());
    assert_ne!(
        structured.evidence_dir(std::path::Path::new("fixture")),
        native.evidence_dir(std::path::Path::new("fixture"))
    );
    assert!(native.request_url().ends_with("/systemone"));
    let native_request: Value = serde_json::from_slice(
        &native
            .prepare(request(&native))
            .expect("native request")
            .body,
    )
    .expect("JSON");
    assert!(native_request.get("messages").is_none());
}

#[test]
fn invalid_decisions_are_never_repaired_or_normalized() {
    let provider = provider();
    let prepared = provider.prepare(request(&provider)).expect("request");
    for (path, replacement) in [
        ("/answers/ready/noul", json!(1.01)),
        ("/answers/destination/confidence", json!(-0.1)),
        ("/answers/destination/choice", json!("home")),
        ("/answers/destination/probabilities/help", json!(0.5)),
        ("/answers/urgency/score", json!(0.9)),
        ("/answers/urgency/legend/0", json!(false)),
    ] {
        let mut invalid = answers();
        *invalid.pointer_mut(path).expect("fixture field") = replacement;
        assert_eq!(
            provider.decode(
                &prepared,
                envelope(&invalid.to_string()).to_string().into_bytes()
            ),
            Err(Refusal::Response),
            "{path}"
        );
    }
    for content in [
        "plain answer",
        "```json\n{}\n```",
        "{\"answers\":{}}",
        "{\"answers\":{},\"answers\":{}}",
    ] {
        assert_eq!(
            provider.decode(&prepared, envelope(content).to_string().into_bytes()),
            Err(Refusal::Response),
            "{content}"
        );
    }
}

#[test]
fn missing_extra_or_duplicate_answer_fields_are_refused() {
    let provider = provider();
    let prepared = provider.prepare(request(&provider)).expect("request");
    let mut missing = answers();
    missing["answers"]
        .as_object_mut()
        .expect("answers")
        .remove("ready");
    let mut extra = answers();
    extra["answers"]["unasked"] = json!({"type":"noul","noul":0.5});
    let mut extra_field = answers();
    extra_field["answers"]["ready"]["unasked"] = json!(true);
    let duplicate = answers()
        .to_string()
        .replace("\"noul\":0.95", "\"noul\":0.95,\"noul\":0.95");
    for content in [
        missing.to_string(),
        extra.to_string(),
        extra_field.to_string(),
        duplicate,
    ] {
        assert_eq!(
            provider.decode(&prepared, envelope(&content).to_string().into_bytes()),
            Err(Refusal::Response)
        );
    }
}

#[test]
fn refusal_tool_calls_and_incomplete_completions_are_not_answers() {
    let provider = provider();
    let prepared = provider.prepare(request(&provider)).expect("request");
    for reason in ["length", "content_filter", "tool_calls", "unknown"] {
        let mut response = envelope(&answers().to_string());
        response["choices"][0]["finish_reason"] = json!(reason);
        let expected = if reason == "length" {
            Refusal::Truncated
        } else {
            Refusal::Response
        };
        assert_eq!(
            provider.decode(&prepared, response.to_string().into_bytes()),
            Err(expected)
        );
    }
    for (field, value) in [
        ("refusal", json!("cannot answer")),
        ("tool_calls", json!([{}])),
        ("function_call", json!({"name":"unexpected"})),
    ] {
        let mut response = envelope(&answers().to_string());
        response["choices"][0]["message"][field] = value;
        assert_eq!(
            provider.decode(&prepared, response.to_string().into_bytes()),
            Err(Refusal::Response)
        );
    }
    let mut multiple = envelope(&answers().to_string());
    let repeated = multiple["choices"][0].clone();
    multiple["choices"]
        .as_array_mut()
        .expect("choices")
        .push(repeated);
    assert_eq!(
        provider.decode(&prepared, multiple.to_string().into_bytes()),
        Err(Refusal::Response)
    );
}

#[test]
fn absent_or_malformed_token_usage_is_not_invented() {
    let provider = provider();
    let prepared = provider.prepare(request(&provider)).expect("request");
    for usage in [
        Value::Null,
        json!({}),
        json!({"prompt_tokens":-1,"completion_tokens":2}),
        json!({"prompt_tokens":1,"completion_tokens":"2"}),
    ] {
        let mut response = envelope(&answers().to_string());
        response["usage"] = usage;
        assert_eq!(
            provider.decode(&prepared, response.to_string().into_bytes()),
            Err(Refusal::Response)
        );
    }
}

#[test]
fn unknown_media_or_stale_request_identity_reaches_no_chat_request() {
    let provider = provider();
    let mut body: Value = serde_json::from_slice(&request(&provider)).expect("request");
    body["images"] = json!(["unsupported"]);
    assert!(matches!(
        provider.prepare(body.to_string().into_bytes()),
        Err(Refusal::Request)
    ));
    body.as_object_mut().expect("request").remove("images");
    body["model"] = json!("another-provider");
    assert!(matches!(
        provider.prepare(body.to_string().into_bytes()),
        Err(Refusal::SettingsChanged)
    ));
}

#[test]
fn an_owned_server_accepts_only_the_schema_and_instructions_its_provider_prepared() {
    let provider = provider();
    let outgoing = provider
        .prepare(request(&provider))
        .expect("outgoing request");
    let served = provider.prepare_served(outgoing.body.clone());
    assert!(
        served.is_ok(),
        "the owned server rejected its own prepared request"
    );
    assert_eq!(served.expect("asserted request").body, outgoing.body);
    for (pointer, value) in [
        ("/messages/0/content", json!("different instructions")),
        ("/stream", json!(true)),
        (
            "/response_format/json_schema/schema/additionalProperties",
            json!(true),
        ),
        ("/max_tokens", json!(999_999)),
    ] {
        let mut tampered: Value = serde_json::from_slice(&outgoing.body).expect("request");
        *tampered.pointer_mut(pointer).expect("field") = value;
        assert!(
            matches!(
                provider.prepare_served(tampered.to_string().into_bytes()),
                Err(Refusal::Request)
            ),
            "{pointer}"
        );
    }
}

#[test]
fn the_owned_chat_route_refuses_ambiguous_user_data_and_preserves_context_refusals() {
    let provider = provider();
    let prepared = provider.prepare(request(&provider)).expect("request");
    let mut incoming: Value = serde_json::from_slice(&prepared.body).expect("request");
    incoming["messages"][1]["content"] = json!("{\"state\":1,\"state\":2,\"questions\":{}}");
    assert!(matches!(
        provider.prepare_served(incoming.to_string().into_bytes()),
        Err(Refusal::Request)
    ));
    for (flag, value, expected) in [
        ("truncated", json!(true), Refusal::Truncated),
        ("truncated", json!("unknown"), Refusal::Response),
        ("state_tokens_dropped", json!(1), Refusal::Truncated),
        ("truncated_questions", json!(["ready"]), Refusal::Truncated),
        (
            "deadline_budget_exceeded",
            json!(true),
            Refusal::DeadlineBudget,
        ),
    ] {
        let mut response = envelope(&answers().to_string());
        response["usage"][flag] = value;
        assert_eq!(
            provider.decode(&prepared, response.to_string().into_bytes()),
            Err(expected),
            "{flag}"
        );
    }
}
