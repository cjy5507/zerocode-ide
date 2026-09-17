//! Integration tests for the async [`runtime::permission`] seam (L3).
//!
//! These exercise [`ChannelPrompter`] end-to-end: request shape, happy
//! path, error paths, concurrency, and the provider-neutrality of
//! [`PermissionRequest`].

use std::sync::Arc;
use std::time::Duration;

use runtime::permission::{
    ChannelPrompter, OneshotResponder, PermissionChoice, PermissionDecision, PermissionError,
    PermissionPrompter, PermissionRequest, RiskLevel,
};
use tokio::sync::mpsc;
use tokio::time::timeout;

/// Build a canonical neutral request used by most tests.
fn sample_request(tool: &str) -> PermissionRequest {
    PermissionRequest {
        tool: tool.to_string(),
        input_summary: tool.to_string(),
        input_hash: "abc123".to_string(),
        reasoning: format!("agent wants to run {tool}"),
        choices: vec![
            PermissionChoice {
                key: 'y',
                label: "Allow once".to_string(),
                decision: PermissionDecision::AllowOnce,
            },
            PermissionChoice {
                key: 'a',
                label: "Allow session".to_string(),
                decision: PermissionDecision::Allow,
            },
            PermissionChoice {
                key: 'n',
                label: "Deny".to_string(),
                decision: PermissionDecision::Deny,
            },
        ],
        risk_level: RiskLevel::Medium,
    }
}

#[tokio::test]
async fn channel_prompter_happy_path_allow() {
    let (prompter, mut rx) = ChannelPrompter::new(4);

    let consumer = tokio::spawn(async move {
        let (req, responder) = rx.recv().await.expect("request delivered");
        assert_eq!(req.tool, "bash");
        responder
            .respond(PermissionDecision::Allow)
            .expect("receiver still live");
    });

    let decision = prompter
        .decide(sample_request("bash"))
        .await
        .expect("decision returned");
    assert_eq!(decision, PermissionDecision::Allow);
    consumer.await.unwrap();
}

#[tokio::test]
async fn channel_prompter_deny_path() {
    let (prompter, mut rx) = ChannelPrompter::new(1);

    tokio::spawn(async move {
        let (_req, responder) = rx.recv().await.unwrap();
        let _ = responder.respond(PermissionDecision::Deny);
    });

    let decision = prompter.decide(sample_request("write_file")).await.unwrap();
    assert_eq!(decision, PermissionDecision::Deny);
}

#[tokio::test]
async fn channel_prompter_dropped_responder_errors() {
    let (prompter, mut rx) = ChannelPrompter::new(1);

    tokio::spawn(async move {
        let (_req, responder) = rx.recv().await.unwrap();
        drop(responder); // never answer
    });

    let err = prompter.decide(sample_request("bash")).await.unwrap_err();
    assert!(matches!(err, PermissionError::ResponderDropped));
}

#[tokio::test]
async fn channel_prompter_closed_channel_errors() {
    let (prompter, rx) = ChannelPrompter::new(1);
    drop(rx); // consumer gone before any request

    let err = prompter.decide(sample_request("bash")).await.unwrap_err();
    assert!(matches!(err, PermissionError::ChannelClosed));
}

#[tokio::test]
async fn channel_prompter_concurrent_requests_do_not_interleave() {
    let (prompter, mut rx) = ChannelPrompter::new(8);
    let prompter = Arc::new(prompter);

    // Consumer maps tool name -> decision deterministically.
    let consumer = tokio::spawn(async move {
        while let Some((req, responder)) = rx.recv().await {
            let decision = match req.tool.as_str() {
                "bash" => PermissionDecision::Allow,
                "write_file" => PermissionDecision::AllowOnce,
                _ => PermissionDecision::Deny,
            };
            let _ = responder.respond(decision);
        }
    });

    let p1 = Arc::clone(&prompter);
    let p2 = Arc::clone(&prompter);
    let p3 = Arc::clone(&prompter);
    let (a, b, c) = tokio::join!(
        async move { p1.decide(sample_request("bash")).await },
        async move { p2.decide(sample_request("write_file")).await },
        async move { p3.decide(sample_request("rm")).await },
    );
    assert_eq!(a.unwrap(), PermissionDecision::Allow);
    assert_eq!(b.unwrap(), PermissionDecision::AllowOnce);
    assert_eq!(c.unwrap(), PermissionDecision::Deny);

    drop(prompter);
    consumer.await.unwrap();
}

#[tokio::test]
async fn permission_request_is_provider_neutral() {
    // Compile-time / structural assertion: the neutral request exposes
    // only tool/reasoning/choices/risk_level. No Anthropic-specific
    // field names leak across the seam (code-rules R1).
    let req = sample_request("bash");
    let PermissionRequest {
        tool,
        input_summary,
        input_hash,
        reasoning,
        choices,
        risk_level,
    } = req;
    assert_eq!(tool, "bash");
    assert_eq!(input_summary, "bash");
    assert_eq!(input_hash, "abc123");
    assert!(reasoning.contains("bash"));
    assert_eq!(choices.len(), 3);
    assert_eq!(risk_level, RiskLevel::Medium);
}

#[tokio::test]
async fn from_sender_bridges_existing_mpsc() {
    let (tx, mut rx) = mpsc::channel::<(PermissionRequest, OneshotResponder)>(2);
    let prompter = ChannelPrompter::from_sender(tx);

    tokio::spawn(async move {
        let (_req, responder) = rx.recv().await.unwrap();
        let _ = responder.respond(PermissionDecision::AllowOnce);
    });

    let out = timeout(
        Duration::from_secs(2),
        prompter.decide(sample_request("bash")),
    )
    .await
    .expect("no timeout")
    .expect("no error");
    assert_eq!(out, PermissionDecision::AllowOnce);
}
