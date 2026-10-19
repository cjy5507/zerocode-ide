use super::*;

fn configured(base: &str) -> SystemOneConfig {
    let candidate = SystemOneConfig::from_root(&serde_json::json!({"smart":{"judgmentProvider":{
        "kind":"local","protocol":"openai_json","baseUrl":base,
        "model":"acme/decision","requiresAuth":false
    }}}));
    assert!(candidate.is_ok(), "structured provider was refused: {candidate:?}");
    candidate.expect("asserted candidate")
}

fn request(config: &SystemOneConfig) -> Vec<u8> {
    serde_json::json!({"model":config.provider.identity(),"state":"Ready",
        "questions":{"ready":{"type":"noul","instructions":"Is it ready?"}}}).to_string().into_bytes()
}

fn response() -> String {
    serde_json::json!({"model":"acme/decision","choices":[{"index":0,"finish_reason":"stop",
        "message":{"role":"assistant","content":"{\"answers\":{\"ready\":{\"type\":\"noul\",\"noul\":0.95}}}"}}],
        "usage":{"prompt_tokens":10,"completion_tokens":8}}).to_string()
}

#[tokio::test]
async fn structured_decisions_use_the_same_cli_client_without_a_cloud_key() {
    let mock = MockSystemOne::serving(vec![Reply::now(200, response())]).await;
    let config = configured(&mock.base_url);
    let body = request(&config);
    assert_eq!(config.key_source(), None);
    let call = config.into_client().decide_body_once(body, UNHURRIED).await;
    assert!(call.outcome.is_ok(), "{:?}", call.outcome);
    assert_eq!(call.requests, 1);
    let seen = mock.seen();
    assert_eq!(seen.len(), 1);
    assert!(seen[0].head.starts_with("POST /v1/chat/completions "));
    assert!(!seen[0].head.to_ascii_lowercase().contains("authorization:"));
    let sent: serde_json::Value = serde_json::from_str(&seen[0].body).expect("JSON");
    assert_eq!(sent["response_format"]["type"], "json_schema");
}

#[tokio::test]
async fn a_schema_unsupported_server_is_not_retried_as_free_text() {
    let mock = MockSystemOne::serving(vec![Reply::now(400, "{\"error\":\"unsupported response_format\"}")]).await;
    let config = configured(&mock.base_url);
    let body = request(&config);
    let call = config.into_client().decide_body_once(body, UNHURRIED).await;
    assert!(call.outcome.is_err());
    assert_eq!(call.requests, 1);
    assert_eq!(mock.seen().len(), 1);
}

#[tokio::test]
async fn structured_json_that_is_not_a_decision_remains_a_schema_refusal() {
    let mock = MockSystemOne::serving(vec![Reply::now(200, "{\"choices\":[]}")]).await;
    let config = configured(&mock.base_url);
    let body = request(&config);
    let call = config.into_client().decide_body_once(body, UNHURRIED).await;
    assert_eq!(call.outcome, Err(SystemOneFailure::Schema));
    assert_eq!(call.requests, 1);
}
