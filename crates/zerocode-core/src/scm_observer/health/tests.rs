use super::*;

#[test]
fn renderer_fixture_is_the_serialized_core_health_contract() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../../../../../fixtures/scm-health.json")).unwrap();
    let mut health = HealthBook::default();
    health.record(
        fixture["root"].as_str().unwrap(),
        Operation::Discovery,
        "head",
        Err(FailureKind::Timeout.into()),
        10,
    );
    assert_eq!(
        serde_json::to_value(health.rows()).unwrap(),
        fixture["rows"]
    );
}

#[test]
fn repeated_failures_back_off_only_the_failed_operation_and_notify_once() {
    let mut health = HealthBook::default();
    for (at, expected_retry) in [(0, 30_000), (30_000, 90_000), (90_000, 210_000)] {
        let transition = health.record(
            "/repos/first",
            Operation::Discovery,
            "head",
            Err(FailureKind::Refused.into()),
            at,
        );
        assert_eq!(
            transition,
            (at == 0).then_some(HealthChange::Failed(FailureKind::Refused))
        );
        assert!(!health.due(
            "/repos/first",
            Operation::Discovery,
            "head",
            expected_retry - 1
        ));
        assert!(health.due("/repos/first", Operation::Discovery, "head", expected_retry));
        assert!(health.due("/repos/second", Operation::Discovery, "head", at));
        assert!(health.due("/repos/first", Operation::Checks, "head", at));
    }
    assert_eq!(health.rows()[0].state.consecutive_failures, 3);
}

#[test]
fn recovery_is_a_transition_and_preserves_the_last_success_during_a_failure() {
    let mut health = HealthBook::default();
    assert_eq!(
        health.record("/repo", Operation::Reviews, "head", Ok(()), 10),
        None
    );
    health.record(
        "/repo",
        Operation::Reviews,
        "head",
        Err(FailureKind::Timeout.into()),
        20,
    );
    assert_eq!(health.rows()[0].state.last_success_ms, Some(10));
    assert_eq!(
        health.record("/repo", Operation::Reviews, "head", Ok(()), 30),
        Some(HealthChange::Recovered)
    );
    assert_eq!(health.rows()[0].state.consecutive_failures, 0);
    assert_eq!(health.rows()[0].state.next_retry_ms, None);
    assert_eq!(
        health.record("/repo", Operation::Reviews, "head", Ok(()), 40),
        None
    );
}

#[test]
fn retry_deadlines_survive_restart_and_manual_retry_does_not_forge_success() {
    let mut health = HealthBook::default();
    health.record(
        "/repo",
        Operation::Checks,
        "head",
        Err(Failure {
            kind: FailureKind::RateLimited,
            retry_after_ms: Some(120_000),
        }),
        10,
    );
    let mut restored: HealthBook =
        serde_json::from_slice(&serde_json::to_vec(&health).unwrap()).unwrap();
    assert_eq!(restored, health);
    assert!(!restored.due("/repo", Operation::Checks, "head", 120_009));
    assert!(restored.retry("/repo", 100));
    assert!(restored.due("/repo", Operation::Checks, "head", 100));
    assert_eq!(restored.rows()[0].state.last_success_ms, None);
    assert_eq!(
        restored.rows()[0].state.failure,
        Some(FailureKind::RateLimited)
    );
}

#[test]
fn changed_checkout_and_clock_rollback_cannot_trap_an_observer_in_backoff() {
    let mut health = HealthBook::default();
    health.record(
        "/repo",
        Operation::Discovery,
        "old",
        Err(FailureKind::Forbidden.into()),
        100_000,
    );
    assert!(health.due("/repo", Operation::Discovery, "new", 100_001));
    assert!(health.due("/repo", Operation::Discovery, "old", 99_999));
    health.record(
        "/repo",
        Operation::Discovery,
        "new",
        Err(FailureKind::Forbidden.into()),
        100_001,
    );
    assert_eq!(health.rows()[0].state.consecutive_failures, 1);
}

#[test]
fn old_books_load_and_failed_state_writes_do_not_publish_a_new_health_state() {
    use crate::scm_observer::{Book, Effects, Subject};
    struct Refused;
    impl Effects for Refused {
        fn save(&mut self, _: &Book) -> Result<(), String> {
            Err("disk unavailable".into())
        }
        fn mail(&mut self, _: &Subject, _: &str, _: &str) -> Result<(), String> {
            unreachable!()
        }
    }
    let mut book: Book = serde_json::from_str(r#"{"subjects":{}}"#).unwrap();
    let mut health = book.health.clone();
    health.record(
        "/repo",
        Operation::Discovery,
        "head",
        Err(FailureKind::Refused.into()),
        0,
    );
    assert!(book.update_health(health, &mut Refused).is_err());
    assert!(book.health.rows().is_empty());
}
