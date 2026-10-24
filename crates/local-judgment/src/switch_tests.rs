use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use judgment_provider::{Protocol, Provider, ProviderKind};
use serde_json::json;

use crate::engine::Engine;
use crate::manager::{EngineFactory, test_support};
use crate::{Binding, Error, Manager, Model};

struct Fixture {
    home: tempfile::TempDir,
    manager: Arc<Manager>,
    previous: Model,
    target: Model,
    old_engine: Arc<Engine>,
    old_server: String,
    provider: Provider,
    settings: PathBuf,
}

async fn fixture(factory: EngineFactory) -> Fixture {
    let home = tempfile::tempdir().unwrap();
    let settings = home.path().join("settings.json");
    fs::write(&settings, b"{}").unwrap();
    let mut previous = crate::tests::fixture_model();
    previous.id = "fixture-a".to_owned();
    let mut target = previous.clone();
    target.id = "fixture-b".to_owned();
    let manager = test_support::switching_manager(
        &home.path().join("data"),
        &settings,
        previous.clone(),
        target.clone(),
        factory,
    );
    let old_engine = crate::tests::running_fixture_engine();
    let (old_server, provider) = test_support::running(&manager, old_engine.clone(), true).await;
    let binding = Binding {
        model_id: previous.id.clone(),
        revision: previous.revision.clone(),
        provider_identity: provider.identity(),
    };
    fs::write(
        &settings,
        serde_json::to_vec(
            &json!({"smart":{"judgmentProvider":provider,(crate::MANAGED_MODEL_KEY):binding}}),
        )
        .unwrap(),
    )
    .unwrap();
    Fixture {
        home,
        manager,
        previous,
        target,
        old_engine,
        old_server,
        provider,
        settings,
    }
}

fn save(path: &Path, provider: &Provider, expected: &str, binding: &Binding) -> crate::Result<()> {
    if Provider::read(path)
        .map_err(|_| Error::SettingsChanged)?
        .identity()
        != expected
    {
        return Err(Error::SettingsChanged);
    }
    fs::write(
        path,
        serde_json::to_vec(
            &json!({"smart":{"judgmentProvider":provider,(crate::MANAGED_MODEL_KEY):binding}}),
        )
        .unwrap(),
    )
    .map_err(|_| Error::Storage)
}

fn begin(
    fixture: &Fixture,
    apply: impl FnOnce(&Provider, &str, &Binding) -> crate::Result<()> + Send + 'static,
) -> crate::Snapshot {
    fixture
        .manager
        .switch_model(
            &fixture.target.id,
            &fixture.target.revision,
            &fixture.old_server,
            &fixture.provider.identity(),
            &fixture.manager.snapshot().instance_id,
            apply,
        )
        .unwrap()
}

#[tokio::test]
async fn explicit_switch_reaps_a_before_loading_b_and_applies_only_checked_b() {
    let engines = Arc::new(Mutex::new(Vec::<Arc<Engine>>::new()));
    let observed = engines.clone();
    let factory: EngineFactory = Arc::new(move |_| {
        assert!(
            observed
                .lock()
                .unwrap()
                .iter()
                .all(|engine| !engine.alive())
        );
        let engine = crate::tests::ready_fixture_engine();
        observed.lock().unwrap().push(engine.clone());
        Ok(engine)
    });
    let fixture = fixture(factory).await;
    engines.lock().unwrap().push(fixture.old_engine.clone());
    let settings = fixture.settings.clone();
    begin(&fixture, move |provider, expected, binding| {
        save(&settings, provider, expected, binding)
    });
    let switched = crate::tests::settled_snapshot(&fixture.manager).await;
    assert_eq!(switched.operation.unwrap().state, "done");
    assert_eq!(switched.applied_model_id.as_deref(), Some("fixture-b"));
    assert!(switched.server.checked);
    crate::manager::native_check(switched.server.provider.as_ref().unwrap())
        .await
        .unwrap();
    assert_eq!(switched.model_switch.unwrap().state, "applied");
    assert!(!fixture.old_engine.alive());
    assert_eq!(
        Provider::read(&fixture.settings).unwrap().model,
        "fixture-b"
    );
    fixture.manager.shutdown();
}

#[tokio::test]
async fn target_startup_failure_restores_a_on_its_original_selected_address() {
    let factory: EngineFactory = Arc::new(|model| {
        if model.id == "fixture-b" {
            Err(Error::RuntimeFailed)
        } else {
            Ok(crate::tests::ready_fixture_engine())
        }
    });
    let fixture = fixture(factory).await;
    let original = fs::read(&fixture.settings).unwrap();
    begin(&fixture, |_, _, _| panic!("failed startup must not apply"));
    let restored = crate::tests::settled_snapshot(&fixture.manager).await;
    assert_eq!(
        restored.operation.unwrap().error,
        Some(Error::RuntimeFailed.token())
    );
    assert_eq!(
        restored.server.provider.as_ref().unwrap().identity(),
        fixture.provider.identity()
    );
    assert_eq!(restored.applied_model_id.as_deref(), Some("fixture-a"));
    assert_eq!(restored.model_switch.unwrap().state, "restored");
    assert_eq!(fs::read(&fixture.settings).unwrap(), original);
    crate::manager::native_check(restored.server.provider.as_ref().unwrap())
        .await
        .unwrap();
    fixture.manager.shutdown();
}

#[tokio::test]
async fn switching_back_reuses_as_owned_port_and_releases_deleted_model_listeners() {
    let engines = Arc::new(Mutex::new(Vec::<Arc<Engine>>::new()));
    let observed = engines.clone();
    let fixture = fixture(Arc::new(move |_| {
        assert!(
            observed
                .lock()
                .unwrap()
                .iter()
                .all(|engine| !engine.alive())
        );
        let engine = crate::tests::ready_fixture_engine();
        observed.lock().unwrap().push(engine.clone());
        Ok(engine)
    }))
    .await;
    engines.lock().unwrap().push(fixture.old_engine.clone());
    let settings = fixture.settings.clone();
    begin(&fixture, move |provider, expected, binding| {
        save(&settings, provider, expected, binding)
    });
    let target = crate::tests::settled_snapshot(&fixture.manager).await;
    let target_provider = target.server.provider.unwrap();
    crate::manager::native_check(&target_provider)
        .await
        .unwrap();
    assert_eq!(test_support::held_listener_count(&fixture.manager), 1);
    let settings = fixture.settings.clone();
    fixture
        .manager
        .switch_model(
            &fixture.previous.id,
            &fixture.previous.revision,
            target.server.id.as_deref().unwrap(),
            &target_provider.identity(),
            &target.instance_id,
            move |provider, expected, binding| save(&settings, provider, expected, binding),
        )
        .unwrap();
    let previous = crate::tests::settled_snapshot(&fixture.manager).await;
    let previous_provider = previous.server.provider.unwrap();
    assert_eq!(previous_provider.identity(), fixture.provider.identity());
    crate::manager::native_check(&previous_provider)
        .await
        .unwrap();
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    let models: serde_json::Value = client
        .get(format!("{}/v1/models", previous_provider.base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(models["data"][0]["id"], "fixture-a");
    assert_eq!(test_support::held_listener_count(&fixture.manager), 1);
    fixture
        .manager
        .remove(&fixture.target.id, &fixture.target.revision)
        .unwrap();
    assert_eq!(test_support::held_listener_count(&fixture.manager), 0);
    fixture.manager.shutdown();
    assert_eq!(test_support::held_listener_count(&fixture.manager), 0);
    assert!(engines.lock().unwrap().iter().all(|engine| !engine.alive()));
}

#[tokio::test]
async fn invalid_native_answers_never_apply_b_and_restore_a() {
    let factory: EngineFactory = Arc::new(|model| {
        if model.id == "fixture-b" {
            Ok(Engine::from_command(std::process::Command::new("python3").args(["-I", "-u", "-c", "import sys\nprint('{\"ready\":true}',flush=True)\nfor line in sys.stdin: print('{\"choices\":[{\"message\":{\"content\":\"0.9\"}}]}',flush=True)\n"])))
        } else {
            Ok(crate::tests::ready_fixture_engine())
        }
    });
    let fixture = fixture(factory).await;
    let original = fs::read(&fixture.settings).unwrap();
    begin(&fixture, |_, _, _| panic!("chat output must not apply"));
    let restored = crate::tests::settled_snapshot(&fixture.manager).await;
    assert_eq!(
        restored.operation.unwrap().error,
        Some(Error::Protocol.token())
    );
    assert_eq!(restored.applied_model_id.as_deref(), Some("fixture-a"));
    assert_eq!(fs::read(&fixture.settings).unwrap(), original);
    fixture.manager.shutdown();
}

#[tokio::test]
async fn stale_switch_state_never_stops_the_current_server() {
    let fixture = fixture(Arc::new(|_| Ok(crate::tests::ready_fixture_engine()))).await;
    let instance = fixture.manager.snapshot().instance_id;
    assert!(matches!(
        fixture.manager.switch_model(
            &fixture.target.id,
            &fixture.target.revision,
            &fixture.old_server,
            &fixture.provider.identity(),
            "old-instance",
            |_, _, _| panic!("stale")
        ),
        Err(Error::InstanceChanged)
    ));
    assert!(matches!(
        fixture.manager.switch_model(
            &fixture.target.id,
            &fixture.target.revision,
            "old-server",
            &fixture.provider.identity(),
            &instance,
            |_, _, _| panic!("stale")
        ),
        Err(Error::ServerChanged)
    ));
    assert!(matches!(
        fixture.manager.switch_model(
            &fixture.target.id,
            &fixture.target.revision,
            &fixture.old_server,
            "old-provider",
            &instance,
            |_, _, _| panic!("stale")
        ),
        Err(Error::SettingsChanged)
    ));
    assert!(fixture.old_engine.alive());
    assert!(fixture.manager.snapshot().operation.is_none());
    fixture.manager.shutdown();
}

#[tokio::test]
async fn target_data_corruption_fails_before_a_is_stopped() {
    let starts = Arc::new(AtomicUsize::new(0));
    let observed = starts.clone();
    let fixture = fixture(Arc::new(move |_| {
        observed.fetch_add(1, Ordering::SeqCst);
        Ok(crate::tests::ready_fixture_engine())
    }))
    .await;
    fs::write(
        fixture
            .home
            .path()
            .join("data/managed-judgment/models/fixture-b/model.safetensors"),
        b"changed",
    )
    .unwrap();
    begin(&fixture, |_, _, _| panic!("corrupt data must not apply"));
    let failed = crate::tests::settled_snapshot(&fixture.manager).await;
    assert_eq!(
        failed.operation.unwrap().error,
        Some(Error::Integrity.token())
    );
    assert!(fixture.old_engine.alive());
    assert_eq!(starts.load(Ordering::SeqCst), 0);
    assert_eq!(failed.applied_model_id.as_deref(), Some("fixture-a"));
    fixture.manager.shutdown();
}

#[tokio::test]
async fn cancellation_before_cutover_keeps_the_original_model_running() {
    let fixture = fixture(Arc::new(|_| Ok(crate::tests::ready_fixture_engine()))).await;
    let snapshot = begin(&fixture, |_, _, _| {
        panic!("cancelled switch must not apply")
    });
    fixture
        .manager
        .cancel(&snapshot.operation.unwrap().id)
        .unwrap();
    let cancelled = crate::tests::settled_snapshot(&fixture.manager).await;
    assert_eq!(cancelled.operation.unwrap().state, "cancelled");
    assert!(fixture.old_engine.alive());
    assert_eq!(cancelled.applied_model_id.as_deref(), Some("fixture-a"));
    fixture.manager.shutdown();
}

#[tokio::test]
async fn cancellation_after_cutover_reaps_b_and_restores_a() {
    let loading = Arc::new(AtomicBool::new(false));
    let observed = loading.clone();
    let factory: EngineFactory = Arc::new(move |model| {
        if model.id == "fixture-b" {
            observed.store(true, Ordering::SeqCst);
            Ok(Engine::from_command(
                std::process::Command::new("sh")
                    .args(["-c", "while IFS= read -r line; do :; done"]),
            ))
        } else {
            Ok(crate::tests::ready_fixture_engine())
        }
    });
    let fixture = fixture(factory).await;
    let snapshot = begin(&fixture, |_, _, _| {
        panic!("cancelled switch must not apply")
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while !loading.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let address = url::Url::parse(&fixture.provider.base_url).unwrap();
    assert!(
        std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, address.port().unwrap()))
            .is_err()
    );
    fixture
        .manager
        .cancel(&snapshot.operation.unwrap().id)
        .unwrap();
    let cancelled = crate::tests::settled_snapshot(&fixture.manager).await;
    assert_eq!(cancelled.operation.unwrap().state, "cancelled");
    assert_eq!(cancelled.model_switch.unwrap().state, "restored");
    assert_eq!(
        cancelled.server.provider.as_ref().unwrap().identity(),
        fixture.provider.identity()
    );
    assert_eq!(cancelled.applied_model_id.as_deref(), Some("fixture-a"));
    crate::manager::native_check(cancelled.server.provider.as_ref().unwrap())
        .await
        .unwrap();
    fixture.manager.shutdown();
}

#[tokio::test]
async fn another_windows_new_provider_choice_is_never_replaced_by_rollback() {
    let fixture = fixture(Arc::new(|_| Ok(crate::tests::ready_fixture_engine()))).await;
    let settings = fixture.settings.clone();
    let chosen = Provider {
        kind: ProviderKind::Remote,
        protocol: Protocol::SystemOne,
        base_url: "https://example.invalid".to_owned(),
        model: "other-native".to_owned(),
        requires_auth: false,
        connection: None,
    };
    let expected = chosen.identity();
    begin(&fixture, move |_, _, _| {
        fs::write(
            &settings,
            serde_json::to_vec(&json!({"smart":{"judgmentProvider":chosen}})).unwrap(),
        )
        .unwrap();
        Err(Error::SettingsChanged)
    });
    let failed = crate::tests::settled_snapshot(&fixture.manager).await;
    assert_eq!(
        failed.operation.unwrap().error,
        Some(Error::SettingsChanged.token())
    );
    assert_eq!(
        Provider::read(&fixture.settings).unwrap().identity(),
        expected
    );
    assert_eq!(failed.server.state, "stopped");
    assert_eq!(failed.model_switch.unwrap().state, "selection_changed");
    fixture.manager.shutdown();
}

#[tokio::test]
async fn a_target_selected_by_another_window_is_kept_alive_after_cas_refusal() {
    let fixture = fixture(Arc::new(|_| Ok(crate::tests::ready_fixture_engine()))).await;
    let settings = fixture.settings.clone();
    begin(&fixture, move |provider, _, _| {
        fs::write(
            &settings,
            serde_json::to_vec(&json!({"smart":{"judgmentProvider":provider}})).unwrap(),
        )
        .unwrap();
        Err(Error::SettingsChanged)
    });
    let failed = crate::tests::settled_snapshot(&fixture.manager).await;
    assert_eq!(
        failed.operation.unwrap().error,
        Some(Error::SettingsChanged.token())
    );
    assert_eq!(failed.server.state, "ready");
    assert!(failed.server.checked);
    assert_eq!(failed.applied_model_id.as_deref(), Some("fixture-b"));
    assert_eq!(failed.model_switch.unwrap().state, "selection_changed");
    fixture.manager.shutdown();
}

#[tokio::test]
async fn restore_failure_is_explicit_and_keeps_the_last_selected_settings() {
    let fixture = fixture(Arc::new(|_| Err(Error::RuntimeFailed))).await;
    let original = fs::read(&fixture.settings).unwrap();
    begin(&fixture, |_, _, _| panic!("failed startup must not apply"));
    let failed = crate::tests::settled_snapshot(&fixture.manager).await;
    let operation = failed.operation.as_ref().unwrap();
    assert_eq!(operation.state, "failed");
    assert_eq!(operation.error, Some(Error::RestoreFailed.token()));
    assert_eq!(failed.server.error, Some(Error::RestoreFailed.token()));
    let recovery = failed.model_switch.unwrap();
    assert_eq!(recovery.state, "restore_failed");
    assert_eq!(recovery.restore_error, Some(Error::RuntimeFailed.token()));
    assert_eq!(fs::read(&fixture.settings).unwrap(), original);
    let address = url::Url::parse(&fixture.provider.base_url).unwrap();
    assert!(
        std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, address.port().unwrap()))
            .is_err()
    );
    assert_eq!(
        failed.configured_model_id.as_deref(),
        Some(fixture.previous.id.as_str())
    );
    test_support::replace_engine_factory(
        &fixture.manager,
        Arc::new(|_| Ok(crate::tests::ready_fixture_engine())),
    );
    fixture
        .manager
        .start(&fixture.previous.id, &fixture.previous.revision)
        .unwrap();
    let restarted = crate::tests::settled_snapshot(&fixture.manager).await;
    let server = restarted.server.id.unwrap();
    fixture.manager.check(&server).unwrap();
    crate::tests::settled_snapshot(&fixture.manager).await;
    fixture
        .manager
        .apply(
            &server,
            &fixture.provider.identity(),
            |provider, expected, binding| save(&fixture.settings, provider, expected, binding),
        )
        .unwrap();
    assert_eq!(
        fixture.manager.snapshot().applied_model_id.as_deref(),
        Some("fixture-a")
    );
    fixture.manager.shutdown();
}

#[tokio::test]
async fn cancellation_never_hides_a_failed_model_restoration() {
    let loading = Arc::new(AtomicBool::new(false));
    let observed = loading.clone();
    let factory: EngineFactory = Arc::new(move |model| {
        if model.id == "fixture-b" {
            observed.store(true, Ordering::SeqCst);
            Ok(Engine::from_command(
                std::process::Command::new("sh")
                    .args(["-c", "while IFS= read -r line; do :; done"]),
            ))
        } else {
            Err(Error::RuntimeFailed)
        }
    });
    let fixture = fixture(factory).await;
    let original = fs::read(&fixture.settings).unwrap();
    let snapshot = begin(&fixture, |_, _, _| {
        panic!("cancelled switch must not apply")
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while !loading.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    fixture
        .manager
        .cancel(&snapshot.operation.unwrap().id)
        .unwrap();
    let failed = crate::tests::settled_snapshot(&fixture.manager).await;
    let operation = failed.operation.unwrap();
    assert_eq!(operation.state, "failed");
    assert_eq!(operation.error, Some(Error::RestoreFailed.token()));
    assert_eq!(failed.server.error, Some(Error::RestoreFailed.token()));
    let recovery = failed.model_switch.unwrap();
    assert_eq!(recovery.state, "restore_failed");
    assert_eq!(recovery.error, Some(Error::Cancelled.token()));
    assert_eq!(recovery.restore_error, Some(Error::RuntimeFailed.token()));
    assert_eq!(failed.applied_model_id, None);
    assert_eq!(fs::read(&fixture.settings).unwrap(), original);
    fixture.manager.shutdown();
}

#[tokio::test]
async fn guarded_cleanup_preserves_b_selected_before_the_guarded_read() {
    let fixture = fixture(Arc::new(|_| Ok(crate::tests::ready_fixture_engine()))).await;
    let checks = Arc::new(AtomicUsize::new(0));
    let observed = checks.clone();
    let manager = fixture.manager.clone();
    let settings = fixture.settings.clone();
    fixture
        .manager
        .switch_model_guarded(
            &fixture.target.id,
            &fixture.target.revision,
            &fixture.old_server,
            &fixture.provider.identity(),
            &fixture.manager.snapshot().instance_id,
            (
                |_, _, _| Err(Error::SettingsChanged),
                move |action: &mut dyn FnMut() -> crate::Result<()>| {
                    if observed.fetch_add(1, Ordering::SeqCst) == 1 {
                        let provider = manager.snapshot().server.provider.unwrap();
                        fs::write(
                            &settings,
                            serde_json::to_vec(&json!({"smart":{"judgmentProvider":provider}}))
                                .unwrap(),
                        )
                        .unwrap();
                    }
                    action()
                },
            ),
        )
        .unwrap();
    let preserved = crate::tests::settled_snapshot(&fixture.manager).await;
    assert_eq!(checks.load(Ordering::SeqCst), 2);
    assert_eq!(preserved.applied_model_id.as_deref(), Some("fixture-b"));
    assert_eq!(preserved.model_switch.unwrap().state, "selection_changed");
    crate::manager::native_check(preserved.server.provider.as_ref().unwrap())
        .await
        .unwrap();
    fixture.manager.shutdown();
}

#[tokio::test(flavor = "current_thread")]
async fn cancelling_runtime_preparation_never_stops_the_running_model() {
    let home = tempfile::tempdir().unwrap();
    let settings = home.path().join("settings.json");
    fs::write(&settings, b"{}").unwrap();
    let model = crate::tests::fixture_model();
    let manager = test_support::manager(&home.path().join("data"), &settings, model);
    let engine = crate::tests::running_fixture_engine();
    let (server_id, _) = test_support::running(&manager, engine.clone(), true).await;
    let preparing = manager.prepare_runtime().unwrap();
    assert_eq!(preparing.server.id.as_deref(), Some(server_id.as_str()));
    manager.cancel(&preparing.operation.unwrap().id).unwrap();
    let cancelled = crate::tests::settled_snapshot(&manager).await;
    assert_eq!(cancelled.operation.unwrap().state, "cancelled");
    assert_eq!(cancelled.server.id.as_deref(), Some(server_id.as_str()));
    assert!(engine.alive());
    manager.shutdown();
}
