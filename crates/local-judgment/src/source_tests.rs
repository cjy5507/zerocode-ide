use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::json;

use crate::Error;
use crate::catalog::EngineKind;
use crate::source::{self, Cancelled, Progress, SourceInput};

fn callbacks() -> (Cancelled, Progress) {
    (Arc::new(|| false), Arc::new(|_| {}))
}

fn weights() -> Vec<u8> {
    let mut header = br#"{"weight":{"dtype":"F32","shape":[1],"data_offsets":[0,4]}}"#.to_vec();
    while !header.len().is_multiple_of(8) {
        header.push(b' ');
    }
    let mut bytes = (header.len() as u64).to_le_bytes().to_vec();
    bytes.extend(header);
    bytes.extend(1f32.to_le_bytes());
    bytes
}

pub(crate) fn local_fixture(directory: &Path) {
    fs::create_dir_all(directory.join("encoder")).unwrap();
    fs::create_dir_all(directory.join("tokenizer")).unwrap();
    fs::write(
        directory.join("rl_agent_config.json"),
        br#"{"encoder":"acme/encoder","head_layers":2,"max_len":1024}"#,
    )
    .unwrap();
    fs::write(
        directory.join("encoder/config.json"),
        br#"{"model_type":"modernbert"}"#,
    )
    .unwrap();
    fs::write(directory.join("tokenizer/tokenizer_config.json"), b"{}").unwrap();
    fs::write(directory.join("tokenizer/tokenizer.json"), b"{}").unwrap();
    fs::write(directory.join("model.safetensors"), weights()).unwrap();
}

#[test]
fn hf_sources_resolve_repo_urls_revision_and_subfolder_without_other_hosts() {
    let source = source::hub_source(
        "https://huggingface.co/acme/native/tree/pinned/checkpoint",
        None,
        None,
    )
    .unwrap();
    assert_eq!(source.repository, "acme/native");
    assert_eq!(source.revision, "pinned");
    assert_eq!(source.subfolder.as_deref(), Some("checkpoint"));
    for address in [
        "https://example.com/acme/native",
        "https://user:secret@example.com/acme/native",
        "https://huggingface.co/acme/native?token=fixture",
        "acme/../native",
        "https://huggingface.co/acme/native/tree/main/../other",
    ] {
        assert!(
            source::hub_source(address, None, None).is_err(),
            "{address}"
        );
    }
    assert!(
        source::hub_source(
            "https://huggingface.co/acme/native/tree/main",
            Some("other"),
            None
        )
        .is_err()
    );
}

#[test]
fn a_generic_chat_config_is_not_a_native_decision_model() {
    let mut configs = BTreeMap::new();
    configs.insert(
        "config.json".to_owned(),
        json!({"model_type":"qwen3_5","architectures":["ForConditionalGeneration"]}),
    );
    assert_eq!(source::schema(&configs), Err(Error::SourceNotNative));
}

#[test]
fn native_format_detection_requires_the_reviewed_data_schema() {
    let mut configs = BTreeMap::new();
    configs.insert("config.json".to_owned(), json!({"model_type":"qwen3_5"}));
    configs.insert(
        "decision_config.json".to_owned(),
        json!({"format_version":1,"format_id":"d3-code-readout-v1"}),
    );
    assert_eq!(
        source::schema(&configs).unwrap().0,
        EngineKind::TransformersSystemOne
    );
    configs.get_mut("decision_config.json").unwrap()["format_id"] = json!("unknown-native-format");
    assert_eq!(source::schema(&configs), Err(Error::ModelFormatUnsupported));
}

#[test]
fn local_inspection_builds_stable_content_pins_without_changing_originals() {
    let home = tempfile::tempdir().unwrap();
    local_fixture(home.path());
    let original = fs::read(home.path().join("model.safetensors")).unwrap();
    let (cancelled, progress) = callbacks();
    let first = source::inspect_local(home.path(), &cancelled, &progress).unwrap();
    let second = source::inspect_local(home.path(), &cancelled, &progress).unwrap();
    assert_eq!(
        first.model.fingerprint().unwrap(),
        second.model.fingerprint().unwrap()
    );
    assert_eq!(first.model.engine, EngineKind::Laya);
    assert!(first.model.source.starts_with("local://"));
    assert_eq!(first.model.files.len(), 5);
    assert_eq!(
        fs::read(home.path().join("model.safetensors")).unwrap(),
        original
    );
    assert!(!home.path().join("receipt.json").exists());
}

#[test]
fn local_inspection_refuses_pickle_files_and_invalid_safetensors_headers() {
    let home = tempfile::tempdir().unwrap();
    local_fixture(home.path());
    fs::write(home.path().join("model.safetensors"), b"pickle-shaped-data").unwrap();
    let (cancelled, progress) = callbacks();
    assert!(matches!(
        source::inspect_local(home.path(), &cancelled, &progress),
        Err(Error::WeightsFormat)
    ));
}

#[test]
fn local_inspection_observes_cancellation_and_changes_before_install() {
    let home = tempfile::tempdir().unwrap();
    local_fixture(home.path());
    let (cancelled, progress) = callbacks();
    let inspected = source::inspect_local(home.path(), &cancelled, &progress).unwrap();
    fs::write(home.path().join("model.safetensors"), b"changed").unwrap();
    assert_eq!(inspected.check_source(), Err(Error::SourceChanged));
    let cancelled: Cancelled = Arc::new(|| true);
    assert!(matches!(
        source::inspect_local(home.path(), &cancelled, &progress),
        Err(Error::Cancelled)
    ));
}

#[cfg(unix)]
#[test]
fn local_inspection_never_follows_a_model_file_symlink() {
    let home = tempfile::tempdir().unwrap();
    let source = home.path().join("source");
    local_fixture(&source);
    let outside = home.path().join("outside.safetensors");
    fs::write(&outside, weights()).unwrap();
    fs::remove_file(source.join("model.safetensors")).unwrap();
    std::os::unix::fs::symlink(outside, source.join("model.safetensors")).unwrap();
    let (cancelled, progress) = callbacks();
    assert!(matches!(
        source::inspect_local(&source, &cancelled, &progress),
        Err(Error::UnsafePath)
    ));
}

#[test]
fn changing_a_file_during_hashing_never_produces_a_usable_source_token() {
    let home = tempfile::tempdir().unwrap();
    local_fixture(home.path());
    let path = home.path().join("model.safetensors");
    let changed = Arc::new(AtomicBool::new(false));
    let observed = changed.clone();
    let progress: Progress = Arc::new(move |_| {
        if !observed.swap(true, Ordering::SeqCst) {
            fs::write(&path, b"changed").unwrap();
        }
    });
    let cancelled: Cancelled = Arc::new(|| false);
    assert!(source::inspect_local(home.path(), &cancelled, &progress).is_err());
}

#[tokio::test]
async fn source_install_registers_its_manifest_and_preserves_original_data() {
    let home = tempfile::tempdir().unwrap();
    let source = home.path().join("source");
    local_fixture(&source);
    let original = fs::read(source.join("model.safetensors")).unwrap();
    let data = home.path().join("data");
    let settings = home.path().join("settings.json");
    fs::write(&settings, b"{}").unwrap();
    let manager =
        crate::manager::test_support::manager(&data, &settings, super::tests::fixture_model());
    let inspection = manager
        .inspect_source(SourceInput::LocalDirectory {
            directory: source.to_string_lossy().into_owned(),
        })
        .await
        .unwrap();
    assert_eq!(inspection.download_bytes, 0);
    assert!(inspection.copy_bytes > 0);
    assert!(matches!(
        manager.install_source(
            &inspection.inspection_id,
            "old-instance",
            &inspection.catalog_fingerprint,
            None
        ),
        Err(Error::InstanceChanged)
    ));
    manager
        .install_source(
            &inspection.inspection_id,
            &inspection.instance_id,
            &inspection.catalog_fingerprint,
            None,
        )
        .unwrap();
    let installed = super::tests::settled_snapshot(&manager).await;
    assert!(
        installed
            .models
            .iter()
            .any(|model| model.id == inspection.model.id && model.installed)
    );
    assert_eq!(
        fs::read(source.join("model.safetensors")).unwrap(),
        original
    );
    assert_eq!(fs::read(&settings).unwrap(), b"{}");
    assert!(matches!(
        manager.install_source(
            &inspection.inspection_id,
            &inspection.instance_id,
            &inspection.catalog_fingerprint,
            None
        ),
        Err(Error::InspectionChanged)
    ));
    manager.shutdown();
}

#[tokio::test]
async fn stale_catalog_and_changed_source_refuse_install_before_writing() {
    let home = tempfile::tempdir().unwrap();
    let source = home.path().join("source");
    local_fixture(&source);
    let manager = crate::manager::test_support::manager(
        &home.path().join("data"),
        &home.path().join("settings.json"),
        super::tests::fixture_model(),
    );
    let inspection = manager
        .inspect_source(SourceInput::LocalDirectory {
            directory: source.to_string_lossy().into_owned(),
        })
        .await
        .unwrap();
    assert!(matches!(
        manager.install_source(
            &inspection.inspection_id,
            &inspection.instance_id,
            "old-catalog",
            None
        ),
        Err(Error::InspectionChanged)
    ));
    fs::write(source.join("model.safetensors"), b"changed").unwrap();
    assert!(matches!(
        manager.install_source(
            &inspection.inspection_id,
            &inspection.instance_id,
            &inspection.catalog_fingerprint,
            None
        ),
        Err(Error::SourceChanged)
    ));
    assert!(
        !manager
            .snapshot()
            .models
            .iter()
            .any(|model| model.id == inspection.model.id)
    );
    manager.shutdown();
}

#[tokio::test]
async fn a_new_inspection_invalidates_the_previous_source_token() {
    let home = tempfile::tempdir().unwrap();
    let source = home.path().join("source");
    local_fixture(&source);
    let manager = crate::manager::test_support::manager(
        &home.path().join("data"),
        &home.path().join("settings.json"),
        super::tests::fixture_model(),
    );
    let input = SourceInput::LocalDirectory {
        directory: source.to_string_lossy().into_owned(),
    };
    let first = manager.inspect_source(input.clone()).await.unwrap();
    let second = manager.inspect_source(input).await.unwrap();
    assert_ne!(first.inspection_id, second.inspection_id);
    assert_eq!(
        first.model.manifest_fingerprint,
        second.model.manifest_fingerprint
    );
    assert!(matches!(
        manager.install_source(
            &first.inspection_id,
            &first.instance_id,
            &first.catalog_fingerprint,
            None
        ),
        Err(Error::InspectionChanged)
    ));
    assert!(
        manager
            .snapshot()
            .operation
            .as_ref()
            .is_some_and(|operation| operation.kind == "inspect" && operation.state == "done")
    );
    manager.shutdown();
}

#[tokio::test]
async fn an_existing_generated_id_cannot_replace_different_registered_data() {
    let home = tempfile::tempdir().unwrap();
    let source = home.path().join("source");
    local_fixture(&source);
    let (cancelled, progress) = callbacks();
    let inspected = source::inspect_local(&source, &cancelled, &progress).unwrap();
    let mut conflict = inspected.model;
    conflict.files[0].sha256 = "a".repeat(64);
    let manager = crate::manager::test_support::manager(
        &home.path().join("data"),
        &home.path().join("settings.json"),
        super::tests::fixture_model(),
    );
    let before = manager.snapshot();
    let document = serde_json::to_string(&crate::Catalog {
        version: 1,
        models: vec![conflict],
    })
    .unwrap();
    manager
        .register_catalog(&before.instance_id, &before.catalog_fingerprint, &document)
        .unwrap();
    let inspection = manager
        .inspect_source(SourceInput::LocalDirectory {
            directory: source.to_string_lossy().into_owned(),
        })
        .await
        .unwrap();
    assert!(matches!(
        manager.install_source(
            &inspection.inspection_id,
            &inspection.instance_id,
            &inspection.catalog_fingerprint,
            None
        ),
        Err(Error::CatalogConflict)
    ));
    assert!(
        manager
            .snapshot()
            .models
            .iter()
            .all(|model| !model.installed)
    );
    manager.shutdown();
}

#[test]
fn unrelated_hub_wrapper_code_is_not_part_of_a_laya_data_import() {
    let home = tempfile::tempdir().unwrap();
    local_fixture(home.path());
    fs::write(
        home.path().join("config.json"),
        br#"{"auto_map":{"AutoModel":"untrusted.Loader"}}"#,
    )
    .unwrap();
    fs::write(home.path().join("untrusted.py"), b"untrusted").unwrap();
    let (cancelled, progress) = callbacks();
    let inspected = source::inspect_local(home.path(), &cancelled, &progress).unwrap();
    assert!(
        inspected
            .model
            .files
            .iter()
            .all(|file| file.path != "config.json" && !file.path.ends_with(".py"))
    );
}

#[test]
fn abandoning_an_external_stage_removes_only_the_owned_temporary_child() {
    let home = tempfile::tempdir().unwrap();
    let destination = home.path().join("chosen");
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("user-file.txt"), b"keep").unwrap();
    let store = crate::storage::Store::open(&home.path().join("data")).unwrap();
    let model = super::tests::fixture_model();
    let stage = store.model_stage(&model, Some(&destination)).unwrap();
    let partial = stage.path().to_owned();
    assert!(partial.join("ownership.json").is_file());
    drop(stage);
    assert!(!partial.exists());
    assert_eq!(
        fs::read(destination.join("user-file.txt")).unwrap(),
        b"keep"
    );
    assert_eq!(
        fs::read_dir(home.path().join("data/managed-judgment/external-partial"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn restart_cleans_owned_external_partial_data_without_adopting_the_parent() {
    let home = tempfile::tempdir().unwrap();
    let destination = home.path().join("chosen");
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("user-file.txt"), b"keep").unwrap();
    let data = home.path().join("data");
    let store = crate::storage::Store::open(&data).unwrap();
    let stage = store
        .model_stage(&super::tests::fixture_model(), Some(&destination))
        .unwrap();
    let partial = stage.path().to_owned();
    std::mem::forget(stage);
    drop(store);
    crate::storage::Store::open(&data).unwrap();
    assert!(!partial.exists());
    assert_eq!(
        fs::read(destination.join("user-file.txt")).unwrap(),
        b"keep"
    );
}

#[test]
fn tampered_external_ownership_cannot_remove_any_installed_data() {
    let home = tempfile::tempdir().unwrap();
    let destination = home.path().join("chosen");
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("user-file.txt"), b"keep").unwrap();
    let store = crate::storage::Store::open(&home.path().join("data")).unwrap();
    let model = super::tests::fixture_model();
    let mut stage = store.model_stage(&model, Some(&destination)).unwrap();
    fs::write(stage.path().join("model.safetensors"), b"weights").unwrap();
    store.commit_model(&mut stage, &model).unwrap();
    let installed = store.model_path(&model).unwrap();
    fs::write(
        installed.join("ownership.json"),
        br#"{"fingerprint":"changed","nonce":"changed"}"#,
    )
    .unwrap();
    assert_eq!(store.remove(&model), Err(Error::Integrity));
    assert!(installed.join("model.safetensors").is_file());
    assert_eq!(
        fs::read(destination.join("user-file.txt")).unwrap(),
        b"keep"
    );
}

#[test]
fn a_changed_private_location_record_cannot_redirect_removal() {
    let home = tempfile::tempdir().unwrap();
    let destination = home.path().join("chosen");
    fs::create_dir(&destination).unwrap();
    let data = home.path().join("data");
    let store = crate::storage::Store::open(&data).unwrap();
    let model = super::tests::fixture_model();
    let mut stage = store.model_stage(&model, Some(&destination)).unwrap();
    fs::write(stage.path().join("model.safetensors"), b"weights").unwrap();
    store.commit_model(&mut stage, &model).unwrap();
    let installed = store.model_path(&model).unwrap();
    let record = data
        .join("managed-judgment/locations")
        .join(&model.id)
        .join("location.json");
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&record).unwrap()).unwrap();
    value["nonce"] = json!(uuid::Uuid::new_v4().to_string());
    fs::write(record, serde_json::to_vec(&value).unwrap()).unwrap();
    assert_eq!(store.remove(&model), Err(Error::Integrity));
    assert!(installed.join("model.safetensors").is_file());
}

#[tokio::test]
async fn custom_destination_owns_only_its_created_child_and_survives_restart() {
    let home = tempfile::tempdir().unwrap();
    let source = home.path().join("source");
    local_fixture(&source);
    let destination = home.path().join("chosen");
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("user-file.txt"), b"keep").unwrap();
    let data = home.path().join("data");
    let settings = home.path().join("settings.json");
    fs::write(&settings, b"{}").unwrap();
    let manager =
        crate::manager::test_support::manager(&data, &settings, super::tests::fixture_model());
    let inspection = manager
        .inspect_source(SourceInput::LocalDirectory {
            directory: source.to_string_lossy().into_owned(),
        })
        .await
        .unwrap();
    manager
        .install_source(
            &inspection.inspection_id,
            &inspection.instance_id,
            &inspection.catalog_fingerprint,
            Some(destination.clone()),
        )
        .unwrap();
    let snapshot = super::tests::settled_snapshot(&manager).await;
    let installed = snapshot
        .models
        .iter()
        .find(|model| model.id == inspection.model.id)
        .unwrap();
    let child = Path::new(installed.installed_directory.as_deref().unwrap());
    let canonical_destination = fs::canonicalize(&destination).unwrap();
    assert_eq!(child.parent(), Some(canonical_destination.as_path()));
    assert!(child.join("ownership.json").is_file());
    manager.shutdown();
    drop(manager);
    let restored =
        crate::manager::test_support::manager(&data, &settings, super::tests::fixture_model());
    assert!(
        restored
            .snapshot()
            .models
            .iter()
            .any(|model| model.id == inspection.model.id && model.installed)
    );
    restored
        .remove(&inspection.model.id, &inspection.model.revision)
        .unwrap();
    assert!(!child.exists());
    assert_eq!(
        fs::read(destination.join("user-file.txt")).unwrap(),
        b"keep"
    );
    assert!(source.join("model.safetensors").is_file());
}
