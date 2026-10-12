use std::fs;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

#[cfg(unix)]
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::catalog::{self, Artifact, Model};
use crate::manager::test_support;
use crate::storage::{self, Store};
use crate::{Error, Manager};

fn artifact(path: &str, bytes: &[u8]) -> Artifact {
    Artifact {
        path: path.to_owned(),
        bytes: bytes.len() as u64,
        sha256: format!("{:x}", Sha256::digest(bytes)),
        url: "https://example.com/model".to_owned(),
        package: None,
        version: None,
        license: None,
    }
}

fn model() -> Model {
    let mut model = catalog::catalog().unwrap().models.remove(0);
    model.id = "fixture-model".to_owned();
    model.files = vec![artifact("model.safetensors", b"weights")];
    model
}

async fn settled(manager: &Manager) -> crate::Snapshot {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let snapshot = manager.snapshot();
            if snapshot
                .operation
                .as_ref()
                .is_some_and(|operation| !matches!(operation.state, "running" | "cancelling"))
            {
                return snapshot;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("operation did not settle")
}

#[test]
fn public_manifests_are_pinned_and_have_an_exact_download_total() {
    let catalog = catalog::catalog().unwrap();
    assert_eq!(catalog.models[0].download_bytes(), 678_201_636);
    let profile = catalog::runtime().unwrap();
    if let Some(profile) = profile {
        assert!(profile.python.sha256.len() == 64 && profile.download_bytes() > 100_000_000);
    }
}

#[test]
fn portable_paths_reject_traversal_absolute_paths_devices_and_ads() {
    for path in [
        "../other",
        "/tmp/other",
        "C:/other",
        "a\\other",
        "a/../other",
        "a//b",
        "a/./b",
        "NUL",
        "file:stream",
        "a. ",
        "a/COM1.txt",
    ] {
        assert_eq!(catalog::relative(path), Err(Error::UnsafePath), "{path}");
    }
    assert_eq!(
        catalog::relative("tokenizer/config.json").unwrap(),
        Path::new("tokenizer/config.json")
    );
}

#[test]
fn download_sources_require_https_without_embedded_credentials() {
    for source in [
        "http://example.com/model",
        "https://user:secret@example.com/model",
        "file:///tmp/model",
        "https://example.com/model#fragment",
    ] {
        assert!(!catalog::https(source));
    }
    assert!(catalog::https("https://example.com/model?revision=pinned"));
}

#[test]
fn import_rejects_same_size_corruption() {
    let source = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    fs::write(source.path().join("model.safetensors"), b"changed").unwrap();
    let result = test_support::copy_import(
        source.path(),
        target.path(),
        &artifact("model.safetensors", b"weights"),
        &AtomicBool::new(false),
        |_| {},
    );
    assert_eq!(
        result,
        Err(Error::Integrity),
        "same-size corrupt weights must never become installed"
    );
}

#[test]
fn import_refuses_a_size_mismatch_before_copying() {
    let source = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    fs::write(source.path().join("model.safetensors"), b"short").unwrap();
    assert_eq!(
        test_support::copy_import(
            source.path(),
            target.path(),
            &artifact("model.safetensors", b"weights"),
            &AtomicBool::new(false),
            |_| {}
        ),
        Err(Error::Integrity)
    );
    assert!(!target.path().join("model.safetensors").exists());
}

#[test]
fn import_cancellation_does_not_create_an_install_receipt() {
    let source = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    fs::write(source.path().join("model.safetensors"), b"weights").unwrap();
    assert_eq!(
        test_support::copy_import(
            source.path(),
            target.path(),
            &artifact("model.safetensors", b"weights"),
            &AtomicBool::new(true),
            |_| {}
        ),
        Err(Error::Cancelled)
    );
    assert!(!target.path().join("receipt.json").exists());
}

#[test]
fn a_restart_removes_only_owned_partial_files() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::open(home.path()).unwrap();
    let stage = store.stage().unwrap();
    let partial = stage.path().to_owned();
    fs::write(partial.join("partial.safetensors"), b"partial").unwrap();
    std::mem::forget(stage);
    fs::write(home.path().join("user-model.safetensors"), b"original").unwrap();
    drop(store);
    let _ = Store::open(home.path()).unwrap();
    assert!(!partial.exists());
    assert_eq!(
        fs::read(home.path().join("user-model.safetensors")).unwrap(),
        b"original"
    );
}

#[test]
fn a_stage_is_removed_when_integrity_validation_fails() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::open(home.path()).unwrap();
    let path;
    {
        let stage = store.stage().unwrap();
        path = stage.path().to_owned();
        fs::write(path.join("bad"), b"bad").unwrap();
    }
    assert!(!path.exists());
}

#[test]
fn archive_links_must_stay_inside_the_python_distribution() {
    assert_eq!(
        crate::archive::resolve_test_link("python/bin/python3", "python3.12").unwrap(),
        Path::new("python/bin/python3.12")
    );
    for target in [
        "../../../other",
        "/tmp/other",
        "C:\\other",
        "../../other",
        "../NUL",
    ] {
        assert_eq!(
            crate::archive::resolve_test_link("python/bin/python3", target),
            Err(Error::UnsafePath)
        );
    }
}

#[cfg(unix)]
#[test]
fn a_managed_directory_link_cannot_redirect_delete_or_install() {
    let home = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let store = Store::open(home.path()).unwrap();
    let model = model();
    fs::write(outside.path().join("original"), b"keep").unwrap();
    std::os::unix::fs::symlink(
        outside.path(),
        home.path().join("managed-judgment/models").join(&model.id),
    )
    .unwrap();
    assert_eq!(store.remove(&model), Err(Error::UnsafePath));
    assert_eq!(fs::read(outside.path().join("original")).unwrap(), b"keep");
}

#[tokio::test]
async fn importing_and_removing_preserve_the_users_original() {
    let home = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    let settings = home.path().join("settings.json");
    let model = model();
    fs::write(source.path().join("model.safetensors"), b"weights").unwrap();
    let manager = test_support::manager(home.path(), &settings, model.clone());
    let initial = manager.snapshot();
    manager
        .install(&model.id, &model.revision, Some(source.path().to_owned()))
        .unwrap();
    let installed = settled(&manager).await;
    assert_eq!(installed.operation.as_ref().unwrap().state, "done");
    assert!(installed.models[0].installed);
    assert!(installed.revision > initial.revision);
    assert!(!settings.exists());
    manager.remove(&model.id, &model.revision).unwrap();
    assert!(!manager.snapshot().models[0].installed);
    assert_eq!(
        fs::read(source.path().join("model.safetensors")).unwrap(),
        b"weights"
    );
}

#[tokio::test]
async fn duplicate_imports_share_one_operation_and_reject_stale_cancellation() {
    let home = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    let model = model();
    fs::write(source.path().join("model.safetensors"), b"weights").unwrap();
    let manager = test_support::manager(
        home.path(),
        &home.path().join("settings.json"),
        model.clone(),
    );
    let snapshot = manager
        .install(&model.id, &model.revision, Some(source.path().to_owned()))
        .unwrap();
    assert!(matches!(
        manager.install(&model.id, &model.revision, Some(source.path().to_owned())),
        Err(Error::Busy)
    ));
    assert!(matches!(
        manager.cancel("stale-operation"),
        Err(Error::RevisionChanged)
    ));
    manager.cancel(&snapshot.operation.unwrap().id).unwrap();
    let cancelled = settled(&manager).await;
    assert_eq!(cancelled.operation.unwrap().state, "cancelled");
    assert!(!cancelled.models[0].installed);
    assert_eq!(
        fs::read_dir(home.path().join("managed-judgment/partial"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn revision_mismatch_and_missing_runtime_leave_the_provider_unchanged() {
    let home = tempfile::tempdir().unwrap();
    let settings = home.path().join("settings.json");
    let original = b"{\"theme\":\"dark\"}";
    fs::write(&settings, original).unwrap();
    let model = model();
    let manager = test_support::manager(home.path(), &settings, model.clone());
    assert!(matches!(
        manager.start(&model.id, "stale"),
        Err(Error::RevisionChanged)
    ));
    assert!(matches!(
        manager.start(&model.id, &model.revision),
        Err(Error::RuntimeMissing)
    ));
    assert!(matches!(
        manager.apply("stale", "old", |_, _| panic!("must not write")),
        Err(Error::ServerStopped)
    ));
    assert_eq!(fs::read(settings).unwrap(), original);
}

#[cfg(unix)]
fn fake_engine() -> std::sync::Arc<crate::engine::Engine> {
    let source = r#"import json,sys
for line in sys.stdin:
 r=json.loads(line);a={}
 for k,q in r['questions'].items():
  if q['type']=='noul': a[k]={'type':'noul','noul':0.9}
  elif q['type']=='choice':
   keys=list(q['criteria']);p={x:(1.0 if i==0 else 0.0) for i,x in enumerate(keys)};a[k]={'type':'choice','choice':keys[0],'confidence':1.0,'probabilities':p}
  else:
   n=len(q['criteria']);p={str(i):(1.0 if i==0 else 0.0) for i in range(n)};a[k]={'type':'score','score':0.0,'confidence':1.0,'probabilities':p,'legend':{str(i):v for i,v in enumerate(q['criteria'])}}
 print(json.dumps({'model':r['model'],'answers':a,'usage':{'input_tokens':1,'output_tokens':0}}),flush=True)
"#;
    crate::engine::Engine::from_command(
        std::process::Command::new("python3").args(["-I", "-u", "-c", source]),
    )
}

#[cfg(unix)]
#[tokio::test]
async fn an_owned_server_checks_native_answers_and_rejects_browser_origin() {
    let home = tempfile::tempdir().unwrap();
    let manager = test_support::manager(home.path(), &home.path().join("settings.json"), model());
    let engine = fake_engine();
    let (id, provider) = test_support::running(&manager, engine.clone(), false).await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let response = client
        .get(provider.api_url("models"))
        .header("origin", "https://example.com")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);
    manager.check(&id).unwrap();
    let checked = settled(&manager).await;
    assert_eq!(checked.operation.as_ref().unwrap().state, "done");
    assert!(checked.server.checked);
    assert!(checked.applied_model_id.is_none());
    manager.stop(&id).unwrap();
    assert!(!engine.alive());
}

#[cfg(unix)]
#[tokio::test]
async fn managed_apply_requires_the_checked_owned_server_and_expected_provider() {
    let home = tempfile::tempdir().unwrap();
    let settings = home.path().join("settings.json");
    let manager = test_support::manager(home.path(), &settings, model());
    let engine = fake_engine();
    let (id, _) = test_support::running(&manager, engine, false).await;
    let expected = manager.snapshot().provider_identity.unwrap();
    assert!(matches!(
        manager.apply(&id, &expected, |_, _| panic!("unchecked must not write")),
        Err(Error::NotChecked)
    ));
    manager.check(&id).unwrap();
    assert_eq!(settled(&manager).await.operation.unwrap().state, "done");
    assert!(matches!(
        manager.apply("old-server", &expected, |_, _| panic!(
            "stale must not write"
        )),
        Err(Error::ServerChanged)
    ));
    assert!(matches!(
        manager.apply(&id, "old-provider", |_, _| panic!("changed must not write")),
        Err(Error::SettingsChanged)
    ));
    let applied = manager
        .apply(&id, &expected, |provider, expected| {
            assert_eq!(
                judgment_provider::Provider::read(&settings)
                    .unwrap()
                    .identity(),
                expected
            );
            fs::write(
                &settings,
                serde_json::to_vec(&json!({"smart":{"judgmentProvider":provider}})).unwrap(),
            )
            .unwrap();
            Ok(())
        })
        .unwrap();
    assert_eq!(applied.applied_model_id.as_deref(), Some("fixture-model"));
    manager.stop(&id).unwrap();
    assert_eq!(
        manager.snapshot().configured_model_id.as_deref(),
        Some("fixture-model")
    );
    assert!(manager.snapshot().applied_model_id.is_none());
    assert!(matches!(
        manager.remove("fixture-model", &model().revision),
        Err(Error::Configured)
    ));
    manager.shutdown();
    drop(manager);
    let restored = test_support::manager(home.path(), &settings, model());
    assert_eq!(
        restored.snapshot().configured_model_id.as_deref(),
        Some("fixture-model")
    );
    assert_eq!(restored.snapshot().server.state, "stopped");
    fs::write(&settings, b"{}").unwrap();
    assert!(restored.snapshot().configured_model_id.is_none());
}

#[cfg(unix)]
#[tokio::test]
async fn shutdown_reaps_the_owned_process_and_refuses_late_work() {
    let home = tempfile::tempdir().unwrap();
    let manager = test_support::manager(home.path(), &home.path().join("settings.json"), model());
    let engine = fake_engine();
    let (id, _) = test_support::running(&manager, engine.clone(), true).await;
    assert!(matches!(
        manager.stop("old-server"),
        Err(Error::ServerChanged)
    ));
    assert!(engine.alive());
    manager.shutdown();
    assert!(!engine.alive());
    assert!(matches!(manager.check(&id), Err(Error::ServerStopped)));
    assert!(matches!(
        manager.install("fixture-model", &model().revision, None),
        Err(Error::ServerStopped)
    ));
}
