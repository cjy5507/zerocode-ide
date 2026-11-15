//! Server metadata fills only undeclared capabilities, without changing prompts.
#[path = "support/custom_models.rs"]
mod fixture;
#[path = "support/profile_sources.rs"]
mod profile_sources;

use fixture::{OwnProviders, Server};
use serde_json::json;
use zo_ide::prompt_input::PromptInput;

fn report(model: &str) -> PromptInput {
    PromptInput::measure(model, "/w", &["# Zo\nfixed".to_string()], &[], &[])
}

#[test]
fn context_length_replaces_the_undeclared_window_and_reports_read_sources() {
    let _own = OwnProviders::new();
    let server = Server::new("200 OK", r#"{"data":[{"id":"vendor/model","context_length":32768,"max_output_tokens":4096,"input_modalities":["text","image"]}]}"#);
    OwnProviders::install(&json!([server.provider("Fixture", json!(["vendor/model"]))]));
    let before = report("Fixture/vendor/model");
    assert_eq!(api::context_window_for_model("Fixture/vendor/model"), 200_000);
    assert_ne!(200_000, 32_768);
    assert_eq!(fixture::read(&server).unwrap(), ["vendor/model"]);
    assert_eq!(api::context_window_for_model("Fixture/vendor/model"), 32_768);
    assert_eq!(api::max_tokens_for_model("Fixture/vendor/model"), 4096);
    assert!(report("Fixture/vendor/model").render().contains("true ("));
    let after = report("Fixture/vendor/model");
    assert_eq!(before.digest(), after.digest());
    let rendered = after.render();
    profile_sources::assert_sources(&rendered);
    for row in ["32768 tokens (read)", "4096 tokens (read)", "true (read)"] {
        assert!(rendered.contains(row), "missing {row}: {rendered}");
    }
    let requests = server.requests.lock().unwrap();
    assert!(requests[0].starts_with("GET /v1/models HTTP/1.1"));
    assert!(requests[0].to_ascii_lowercase().contains("authorization: bearer fixture-key"));
}

#[test]
fn max_model_len_and_top_provider_output_are_read_per_model() {
    let _own = OwnProviders::new();
    let server = Server::new("200 OK", r#"{"data":[{"id":"vendor/Big","max_model_len":98304,"top_provider":{"max_completion_tokens":8192},"architecture":{"input_modalities":["text","image"]}},{"id":"vendor/plain","max_model_len":16384,"max_completion_tokens":2048,"input_modalities":["text"]}]}"#);
    OwnProviders::install(&json!([server.provider("Fixture", json!(["vendor/Big", "vendor/plain"]))]));
    fixture::read(&server).unwrap();
    assert_eq!(api::context_window_for_model("fixture/vendor/bIG"), 98_304);
    assert_eq!(api::max_tokens_for_model("vendor/big"), 8192);
    assert!(report("vendor/big").render().contains("true ("));
    assert_eq!(api::context_window_for_model("vendor/plain"), 16_384);
    assert_eq!(api::max_tokens_for_model("vendor/plain"), 2048);
    assert!(report("vendor/plain").render().contains("false ("));
}

#[test]
fn declarations_win_per_field_including_explicit_false() {
    let _own = OwnProviders::new();
    let server = Server::new("200 OK", r#"{"data":[{"id":"vendor/model","context_length":32768,"max_output_tokens":4096,"input_modalities":["text","image"]},{"id":"vendor/mixed","context_length":65536,"max_output_tokens":8192,"input_modalities":["text"]}]}"#);
    let mut provider = server.provider("Fixture", json!([
        {"id":"vendor/model","context_window":16384,"supports_vision":false},
        {"id":"vendor/mixed","context_window":0,"max_output_tokens":2048,"supports_vision":true}
    ]));
    provider["max_output_tokens"] = json!(1024);
    OwnProviders::install(&json!([provider]));
    fixture::read(&server).unwrap();
    assert_eq!(api::context_window_for_model("vendor/model"), 16_384);
    assert_eq!(api::max_tokens_for_model("vendor/model"), 1024);
    assert!(report("vendor/model").render().contains("false ("));
    assert_eq!(api::context_window_for_model("vendor/mixed"), 65_536);
    assert_eq!(api::max_tokens_for_model("vendor/mixed"), 2048);
    assert!(report("vendor/mixed").render().contains("true ("));
    let rendered = report("vendor/mixed").render();
    for row in ["65536 tokens (read)", "2048 tokens (declared)", "true (declared)"] {
        assert!(rendered.contains(row), "missing {row}: {rendered}");
    }
    let mut provider = server.provider("Fixture", json!(["vendor/model"]));
    provider["context_window"] = json!(12000);
    provider["supports_vision"] = json!(false);
    OwnProviders::install(&json!([provider]));
    assert_eq!(api::context_window_for_model("vendor/model"), 12_000);
    assert!(report("vendor/model").render().contains("false ("));
}

#[test]
fn a_persons_catalog_declaration_and_a_shipped_row_outrank_the_endpoints_reading() {
    // Window, output cap and pictures alike: the server's answer fills only
    // what no declaration states.
    let _own = OwnProviders::new();
    let server = Server::new("200 OK", r#"{"data":[
        {"id":"vendor/model","context_length":32768,"max_output_tokens":4096,"input_modalities":["text","image"]},
        {"id":"gpt-5.5","context_length":32768,"max_output_tokens":4096},
        {"id":"gpt-5.3-codex-spark","input_modalities":["text","image"]}]}"#);
    OwnProviders::install(&json!([server.provider("Fixture", json!(["vendor/model", "gpt-5.5", "gpt-5.3-codex-spark"]))]));
    // What the person wrote in their model catalog reaches zo through this
    // variable; the shipped catalog is the other declaration.
    std::env::set_var(api::MODEL_CONTEXT_WINDOWS_ENV, json!({"models":[{"ids":["vendor/model"],
        "context_window":50000,"max_output_tokens":3000,"capabilities":["no_vision"]}]}).to_string());
    let shipped = (api::context_window_for_model("gpt-5.5"), api::max_tokens_for_model("gpt-5.5"));
    assert_ne!(shipped, (32_768, 4096));
    assert!(!api::model_supports_vision("gpt-5.3-codex-spark"), "the shipped row reads no pictures");
    fixture::read(&server).unwrap();
    assert_eq!(api::context_window_for_model("Fixture/vendor/model"), 50_000);
    assert_eq!(api::max_tokens_for_model("Fixture/vendor/model"), 3000);
    assert_eq!((api::context_window_for_model("Fixture/gpt-5.5"), api::max_tokens_for_model("Fixture/gpt-5.5")), shipped);
    // A picture the person attaches stays a placeholder, as before the list was read.
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    for model in ["Fixture/vendor/model", "Fixture/gpt-5.3-codex-spark"] {
        let client = api::ProviderClient::from_model(model).unwrap();
        let request = serde_json::from_value(json!({
            "model": model, "max_tokens": 1024,
            "messages": [{"role":"user","content":[{
                "type":"image","source":{"type":"base64","media_type":"image/png","data":"eA=="}
            }]}]
        })).unwrap();
        runtime.block_on(client.send_message(&request)).unwrap();
    }
    {
        let requests = server.requests.lock().unwrap();
        assert_eq!(requests.len(), 3, "one list read and two sends");
        for request in &requests[1..] {
            let body: serde_json::Value = serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
            assert!(body["messages"][0]["content"].as_str().is_some_and(|text| text.contains("image omitted")),
                "a catalog's no_vision lost to the reading: {body}");
        }
    }
    for model in ["Fixture/vendor/model", "Fixture/gpt-5.5", "Fixture/gpt-5.3-codex-spark"] {
        let rendered = report(model).render();
        profile_sources::assert_sources(&rendered);
        assert!(!rendered.contains("(read)"), "a declaration lost to the reading: {rendered}");
    }
    let rendered = report("Fixture/vendor/model").render();
    for row in ["50000 tokens (declared)", "3000 tokens (declared)", "false (declared)"] {
        assert!(rendered.contains(row), "missing {row}: {rendered}");
    }
    let rendered = report("Fixture/gpt-5.3-codex-spark").render();
    assert!(rendered.contains("false (declared)"), "{rendered}");
}

#[test]
fn readings_do_not_cross_endpoints_or_undeclared_models() {
    let _own = OwnProviders::new();
    let first = Server::new("200 OK", r#"{"data":[{"id":"vendor/shared","context_length":32768,"max_output_tokens":4096,"input_modalities":["text","image"]},{"id":"vendor/unlisted","context_length":999999}]}"#);
    let second = Server::new("200 OK", r#"{"data":[{"id":"vendor/shared","context_length":16384,"max_output_tokens":2048,"input_modalities":["text"]}]}"#);
    OwnProviders::install(&json!([
        first.provider("First", json!(["vendor/shared"])),
        second.provider("Second", json!(["vendor/shared"]))
    ]));
    fixture::read(&first).unwrap();
    assert_eq!(api::context_window_for_model("First/vendor/shared"), 32_768);
    assert_eq!(api::context_window_for_model("Second/vendor/shared"), 200_000);
    assert_eq!(api::max_tokens_for_model("Second/vendor/shared"), 64_000);
    assert!(report("Second/vendor/shared").render().contains("false ("));
    assert!(api::custom_provider_for_model("vendor/unlisted").is_none());
    fixture::read(&second).unwrap();
    assert_eq!(api::context_window_for_model("Second/vendor/shared"), 16_384);
    assert_eq!(api::max_tokens_for_model("Second/vendor/shared"), 2048);
    assert_eq!(api::context_window_for_model("First/vendor/shared"), 32_768);
}

#[test]
fn failed_or_uninformative_models_keep_existing_limits() {
    let _own = OwnProviders::new();
    for (status, body, fails) in [
        ("503 Service Unavailable", r#"{"error":{"message":"unavailable"}}"#, true),
        ("200 OK", "not json", true),
        ("200 OK", r#"{"data":[{"id":"vendor/model","context_length":0,"max_output_tokens":-1}]}"#, false),
        ("200 OK", r#"{"data":[{"id":"vendor/model","context_length":"32768","input_modalities":null}]}"#, false),
    ] {
        let server = Server::new(status, body);
        OwnProviders::install(&json!([server.provider("Fixture", json!(["vendor/model"]))]));
        assert_eq!(fixture::read(&server).is_err(), fails);
        assert_eq!(api::context_window_for_model("Fixture/vendor/model"), 200_000);
        assert_eq!(api::max_tokens_for_model("Fixture/vendor/model"), 64_000);
        let rendered = report("Fixture/vendor/model").render();
        assert!(rendered.contains("200000 tokens (default)"), "{rendered}");
        assert!(rendered.contains("64000 tokens (default)"), "{rendered}");
        assert!(rendered.contains("false (default)"), "{rendered}");
    }
}
