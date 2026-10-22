use judgment_provider::{Protocol, Provider, ProviderKind, Refusal};

fn remote(base_url: String, protocol: Protocol) -> Provider {
    Provider {
        kind: ProviderKind::Remote,
        protocol,
        base_url,
        model: "acme/decision".into(),
        requires_auth: true,
    }
}

#[test]
fn remote_structured_decisions_use_the_selected_https_endpoint() {
    let provider = remote(
        "https://gateway.example.com/v1/".into(),
        Protocol::OpenaiJson,
    )
    .validated()
    .expect("an explicitly selected structured decision server");
    assert_eq!(
        provider.request_url(),
        "https://gateway.example.com/v1/chat/completions"
    );
    assert!(!provider.ready_with_key(None));
    assert_ne!(provider.key_env(), Provider::jev("jev-latest").key_env());
}

#[test]
fn an_explicit_private_network_connection_accepts_http_for_both_protocols() {
    // Construct documentation fixtures for the RFC 1918 ranges; no machine's address.
    for address in [
        std::net::Ipv4Addr::new(10, 0, 0, 1),
        std::net::Ipv4Addr::new(172, 16, 0, 1),
        std::net::Ipv4Addr::new(192, 168, 0, 1),
    ] {
        for protocol in [Protocol::SystemOne, Protocol::OpenaiJson] {
            let provider = remote(format!("http://{address}:4000/v1"), protocol);
            assert!(
                provider.validated().is_ok(),
                "private connection refused: {protocol:?}"
            );
        }
    }
}

#[test]
fn a_remote_public_address_still_requires_https() {
    for address in [
        "http://192.0.2.1:4000/v1",
        "http://gateway.example.com/v1",
        "http://localhost.example.com/v1",
    ] {
        for protocol in [Protocol::SystemOne, Protocol::OpenaiJson] {
            assert_eq!(
                remote(address.into(), protocol).validated(),
                Err(Refusal::RemoteAddress)
            );
        }
    }
}

#[test]
fn remote_structured_decisions_preserve_strict_response_validation() {
    let provider = remote(
        "https://gateway.example.com/v1".into(),
        Protocol::OpenaiJson,
    )
    .validated()
    .expect("structured decision server");
    let request = serde_json::json!({
        "model": provider.identity(), "state": {"ready": true},
        "questions": {"ready": {"type": "noul", "instructions": "Is the page ready?"}}
    })
    .to_string()
    .into_bytes();
    let prepared = provider.prepare(request).expect("decision request");
    let sent: serde_json::Value = serde_json::from_slice(&prepared.body).expect("request JSON");
    assert_eq!(sent["response_format"]["json_schema"]["strict"], true);
    assert_eq!(sent["model"], "acme/decision");
}
