use super::*;

#[test]
fn failed_discovery_backs_off_persists_and_recovers_without_repeating_the_notice() {
    let dir = tempfile::tempdir().unwrap();
    let flaky = checkout(dir.path(), "flaky", Some("flaky"));
    let topic = checkout(dir.path(), "topic", Some("topic"));
    let roots = [flaky.clone(), topic.clone()];
    let limits = Limits::default();
    let mut observer = Observer::default();
    let start = 1_000_000;
    tick(
        &mut observer,
        &roots,
        github_losing_flaky,
        &limits,
        start,
        dir.path(),
    );
    tick(
        &mut observer,
        &roots,
        github_losing_flaky,
        &limits,
        start + 30_000,
        dir.path(),
    );
    let waiting = tick(
        &mut observer,
        &roots,
        github_losing_flaky,
        &limits,
        start + 60_000,
        dir.path(),
    );
    assert!(
        waiting
            .iter()
            .all(|call| call.cwd.as_deref() == Some(topic.as_path()))
    );
    assert!(!waiting.is_empty());
    assert_eq!(logged_discovery_refusals(dir.path()), 1);

    let path = dir.path().join("health.json");
    std::fs::write(&path, serde_json::to_vec(&observer.book).unwrap()).unwrap();
    let mut restored = Observer::default();
    restored.load(&path).unwrap();
    assert!(!restored.book.health.due(
        &flaky.to_string_lossy(),
        Operation::Discovery,
        &checkout_token(&flaky).unwrap(),
        start + 60_000
    ));
    restored
        .request_retry(&flaky, start + 70_000, &mut Accepted)
        .unwrap();
    let before = restored
        .book
        .health
        .rows()
        .into_iter()
        .find(|row| row.root == flaky.to_string_lossy())
        .unwrap();
    assert_eq!(before.state.failure, Some(core::FailureKind::Timeout));
    assert_eq!(before.state.last_success_ms, None);
    let recovered = tick(
        &mut restored,
        &roots,
        github,
        &limits,
        start + 70_000,
        dir.path(),
    );
    assert!(
        recovered
            .iter()
            .any(|call| call.cwd.as_deref() == Some(flaky.as_path()))
    );
    assert!(!restored.retry_roots.contains(&flaky));
    let after = restored
        .book
        .health
        .rows()
        .into_iter()
        .find(|row| row.root == flaky.to_string_lossy() && row.operation == Operation::Discovery)
        .unwrap();
    assert_eq!(after.state.failure, None);
    assert_eq!(after.state.last_success_ms, Some(start + 70_000));
    assert_eq!(after.state.consecutive_failures, 0);
    assert_eq!(logged_discovery_refusals(dir.path()), 1);
}

fn github_without_reviews(call: &FakeCall) -> Result<CliOutput, CliError> {
    if is_graphql(call) {
        Err(CliError::TimedOut)
    } else {
        github(call)
    }
}

#[test]
fn a_review_backoff_does_not_stop_ci_or_reuse_an_unacknowledged_cache() {
    let dir = tempfile::tempdir().unwrap();
    let topic = checkout(dir.path(), "topic", Some("topic"));
    let roots = [topic.clone()];
    let limits = Limits::default();
    let mut observer = Observer::default();
    tick(
        &mut observer,
        &roots,
        github_without_reviews,
        &limits,
        1_000_000,
        dir.path(),
    );
    tick(
        &mut observer,
        &roots,
        github_without_reviews,
        &limits,
        1_030_000,
        dir.path(),
    );
    let calls = tick(
        &mut observer,
        &roots,
        github_without_reviews,
        &limits,
        1_060_000,
        dir.path(),
    );
    assert!(calls.iter().all(|call| !is_graphql(call)));
    assert!(
        calls
            .iter()
            .any(|call| call.args.iter().any(|arg| arg.contains("/check-runs?")))
    );
    let rows = observer.book.health.rows();
    assert_eq!(
        rows.iter()
            .find(|row| row.operation == Operation::Checks)
            .unwrap()
            .state
            .last_success_ms,
        Some(1_060_000)
    );
    assert_eq!(
        rows.iter()
            .find(|row| row.operation == Operation::Reviews)
            .unwrap()
            .state
            .consecutive_failures,
        2
    );
    assert!(observer.clients.is_empty());
}

#[test]
fn spending_the_tick_budget_does_not_invent_a_remote_failure() {
    let dir = tempfile::tempdir().unwrap();
    let topic = checkout(dir.path(), "topic", Some("topic"));
    let limits = Limits {
        gh_calls_max: 0,
        ..Limits::default()
    };
    let mut observer = Observer::default();
    assert!(
        tick(
            &mut observer,
            &[topic],
            github,
            &limits,
            1_000_000,
            dir.path()
        )
        .is_empty()
    );
    assert!(observer.book.health.rows().is_empty());
}
