//! The runtime's refresh uses the same connection and changes the actual wire.
#[path = "support/custom_models.rs"]
mod fixture;

use api::custom_models::refresh_custom_model_capabilities;
use fixture::{OwnProviders, Server};
use serde_json::{json, Value};

#[test]
fn refresh_uses_connection_headers_and_the_wire_obeys_model_capabilities() {
    let _own = OwnProviders::new();
    let server = Server::new("200 OK", r#"{"data":[{"id":"vendor/model","context_length":32768,"max_output_tokens":4096,"input_modalities":["text","image"]},{"id":"vendor/text","input_modalities":["text"]}]}"#);
    let mut provider = server.provider("Fixture", json!(["vendor/model", "vendor/text"]));
    provider["user_agent"] = json!("fixture-agent");
    provider["headers"] = json!({"x-fixture": "present"});
    OwnProviders::install(&json!([provider]));
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        refresh_custom_model_capabilities("Fixture/vendor/model");
        refresh_custom_model_capabilities("Fixture/vendor/text");
        assert_eq!(api::context_window_for_model("Fixture/vendor/model"), 32_768);
        let capabilities = api::custom_models::custom_model_capabilities("Fixture/vendor/model").unwrap();
        assert_eq!((capabilities.context_window.value, capabilities.context_window.source),
            (32_768, api::HarnessValueSource::Read));
        assert_eq!((capabilities.max_output_tokens.value, capabilities.max_output_tokens.source),
            (4096, api::HarnessValueSource::Read));
        assert_eq!((capabilities.supports_vision.value, capabilities.supports_vision.source),
            (true, api::HarnessValueSource::Read));
        assert_eq!(server.requests.lock().unwrap().len(), 1);
        let client = api::ProviderClient::from_model("Fixture/vendor/model").unwrap();
        for model in ["Fixture/vendor/model", "Fixture/vendor/text"] {
            let request = serde_json::from_value(json!({
                "model": model,
                "max_tokens": api::max_tokens_for_model(model),
                "messages": [{"role":"user","content":[{
                    "type":"image","source":{"type":"base64","media_type":"image/png","data":"eA=="}
                }]}]
            })).unwrap();
            client.send_message(&request).await.unwrap();
        }
    });
    let requests = server.requests.lock().unwrap();
    assert!(requests[0].starts_with("GET /v1/models HTTP/1.1"));
    let head = requests[0].to_ascii_lowercase();
    assert!(head.contains("user-agent: fixture-agent"), "{head}");
    assert!(head.contains("x-fixture: present"), "{head}");
    let posted = |index: usize| -> Value {
        serde_json::from_str(requests[index].split_once("\r\n\r\n").unwrap().1).unwrap()
    };
    assert_eq!(posted(1)["max_tokens"], 4096);
    assert_eq!(posted(1)["messages"][0]["content"][0]["type"], "image_url");
    assert!(posted(2)["messages"][0]["content"].as_str().unwrap().contains("image omitted"));
    drop(requests);
    // The legacy authenticated listing path reads the same document too.
    assert_eq!(fixture::read(&server).unwrap(), ["vendor/model", "vendor/text"]);
}

#[test]
fn failed_refresh_preserves_a_declaration_and_the_undeclared_fallback() {
    let _own = OwnProviders::new();
    let server = Server::new("503 Service Unavailable", r#"{"error":{"message":"unavailable"}}"#);
    OwnProviders::install(&json!([server.provider("Fixture", json!([
        "vendor/model", {"id":"vendor/declared", "context_window":16384, "max_output_tokens":2048, "supports_vision":true}
    ]))]));
    refresh_custom_model_capabilities("Fixture/vendor/model");
    assert_eq!(api::context_window_for_model("Fixture/vendor/model"), 200_000);
    assert_eq!(api::max_tokens_for_model("Fixture/vendor/model"), 64_000);
    assert_eq!(api::context_window_for_model("Fixture/vendor/declared"), 16_384);
    assert_eq!(api::max_tokens_for_model("Fixture/vendor/declared"), 2048);
    assert_eq!(vision("Fixture/vendor/declared"), (true, api::HarnessValueSource::Declared));
    assert_eq!(server.requests.lock().unwrap().len(), 1);
}

#[test]
fn a_failed_read_is_not_waited_on_again_for_the_same_endpoint_and_model() {
    let _own = OwnProviders::new();
    let server = Server::new("503 Service Unavailable", r#"{"error":{"message":"unavailable"}}"#);
    OwnProviders::install(&json!([server.provider("Fixture", json!(["vendor/model", "vendor/other"]))]));
    for _ in 0..3 {
        refresh_custom_model_capabilities("Fixture/vendor/model");
    }
    assert_eq!(server.requests.lock().unwrap().len(), 1, "a rebuilt runtime asked the failed endpoint again");
    refresh_custom_model_capabilities("Fixture/vendor/other");
    assert_eq!(server.requests.lock().unwrap().len(), 2, "another model on that endpoint is its own read");
    assert_eq!(api::context_window_for_model("Fixture/vendor/model"), 200_000);
}

/// An endpoint that takes the connection and never answers holds the session
/// start for the read's two-second timeout, no longer.
#[test]
fn a_session_start_waits_at_most_the_read_timeout_for_an_endpoint_that_never_answers() {
    let _own = OwnProviders::new();
    let silent = std::net::TcpListener::bind("127.0.0.1:0").expect("loopback listener that never answers");
    let base = format!("http://{}/v1", silent.local_addr().unwrap());
    OwnProviders::install(&json!([{"name":"Fixture","base_url":base,"requires_auth":false,"models":["vendor/model"]}]));
    let started = std::time::Instant::now();
    refresh_custom_model_capabilities("Fixture/vendor/model");
    let waited = started.elapsed();
    assert!(waited < std::time::Duration::from_secs(4), "the session start waited {waited:?}");
    assert_eq!(api::context_window_for_model("Fixture/vendor/model"), 200_000);
}

/// Two providers list the same model id at different endpoints. The output
/// cap a request carries is the selected provider's reading, never the other
/// endpoint's: the runtime asks with the selected model, not the bare wire id.
#[test]
fn a_request_carries_its_own_providers_output_cap_when_another_lists_the_same_id() {
    let _own = OwnProviders::new();
    let first = Server::new("200 OK", r#"{"data":[{"id":"vendor/shared","max_output_tokens":2048}]}"#);
    let second = Server::new("200 OK", r#"{"data":[{"id":"vendor/shared","max_output_tokens":8192}]}"#);
    OwnProviders::install(&json!([
        first.provider("First", json!(["vendor/shared"])),
        second.provider("Second", json!(["vendor/shared"]))
    ]));
    refresh_custom_model_capabilities("First/vendor/shared");
    refresh_custom_model_capabilities("Second/vendor/shared");
    let registry = tools::GlobalToolRegistry::builtin();
    let request = empty_request();
    for (model, cap) in [("First/vendor/shared", 2048), ("Second/vendor/shared", 8192)] {
        let wire = zo_ide::session::runtime_bridge::build_message_request(
            &request, model, false, None, &registry, None, None, None,
        );
        assert_eq!(wire.model, "vendor/shared");
        assert_eq!(wire.max_tokens, cap, "{model} carried another endpoint's output cap");
    }
}

/// A `/models` row's output cap is the most the model can write; a request's
/// `max_tokens` is what one request asks for. A cap read above the default
/// leaves the request at the default, in both server shapes: only a smaller
/// one is taken. The window is still read.
#[test]
fn a_read_output_cap_above_the_default_does_not_raise_the_request() {
    let _own = OwnProviders::new();
    let server = Server::new("200 OK", r#"{"data":[
        {"id":"vendor/model","context_length":1048576,"max_output_tokens":393216},
        {"id":"vendor/routed","top_provider":{"max_completion_tokens":393216}}]}"#);
    OwnProviders::install(&json!([server.provider("Fixture", json!(["vendor/model", "vendor/routed"]))]));
    fixture::read(&server).unwrap();
    assert_eq!(api::context_window_for_model("Fixture/vendor/model"), 1_048_576);
    let registry = tools::GlobalToolRegistry::builtin();
    let request = empty_request();
    for model in ["Fixture/vendor/model", "Fixture/vendor/routed"] {
        let wire = zo_ide::session::runtime_bridge::build_message_request(
            &request, model, false, None, &registry, None, None, None,
        );
        assert_eq!(wire.max_tokens, 64_000, "{model}: a read ceiling raised the request");
        let cap = api::custom_models::custom_model_capabilities(model).expect("the person's model").max_output_tokens;
        assert_eq!((cap.value, cap.source), (64_000, api::HarnessValueSource::Default), "{model}");
    }
}

fn empty_request() -> runtime::ApiRequest {
    runtime::ApiRequest {
        system_prompt: std::sync::Arc::from(Vec::<String>::new()),
        wire_reminders: std::sync::Arc::from(Vec::<String>::new()),
        messages: std::sync::Arc::new(Vec::new()),
        tool_choice: None,
        effort_override: None,
        effort_step: None,
        model_override: None,
        picture_cap: None,
        compaction_summary: false,
    }
}

/// The runtime asks the endpoint before it sizes the session. A test cannot
/// open one with discovery on (it would ask every provider over the network),
/// so the order is pinned in the source instead.
#[test]
fn the_runtime_reads_the_selected_endpoint_before_sizing_the_session() {
    let source = include_str!("../src/runtime_support.rs");
    let read = source.find("api::custom_models::refresh_custom_model_capabilities(&selected_model);")
        .expect("the runtime reads the selected endpoint");
    let sized = source.find("let effective_context_window = api::context_window_for_model(&selected_model);")
        .expect("the runtime sizes the session from the selected model");
    assert!(read < sized, "the window is sized before the endpoint is read");
}

fn vision(model: &str) -> (bool, api::HarnessValueSource) {
    let vision = api::custom_models::custom_model_capabilities(model).expect("the person's model").supports_vision;
    (vision.value, vision.source)
}

#[test]
fn same_endpoint_uses_the_selected_providers_vision_declaration() {
    let _own = OwnProviders::new();
    let server = Server::new("200 OK", r#"{"data":[]}"#);
    let mut first = server.provider("First", json!(["vendor/model"]));
    first["supports_vision"] = json!(false);
    let mut second = server.provider("Second", json!(["vendor/model"]));
    second["supports_vision"] = json!(true);
    OwnProviders::install(&json!([first, second]));
    assert_eq!(vision("First/vendor/model"), (false, api::HarnessValueSource::Declared));
    assert_eq!(vision("Second/vendor/model"), (true, api::HarnessValueSource::Declared));
    let client = api::ProviderClient::from_model("Second/vendor/model").unwrap();
    let request = serde_json::from_value(json!({
        "model":"Second/vendor/model", "max_tokens":1024,
        "messages":[{"role":"user","content":[{
            "type":"image","source":{"type":"base64","media_type":"image/png","data":"eA=="}
        }]}]
    })).unwrap();
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
        .block_on(client.send_message(&request)).unwrap();
    let requests = server.requests.lock().unwrap();
    let body: Value = serde_json::from_str(requests[0].split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(body["messages"][0]["content"][0]["type"], "image_url",
        "the selected provider declared vision, independently of the first row");
}
