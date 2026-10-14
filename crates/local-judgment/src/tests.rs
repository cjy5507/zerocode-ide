use std::fs;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

#[cfg(unix)]
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::catalog::{self, Artifact, Catalog, EngineKind, Model};
use crate::manager::test_support;
use crate::storage::Store;
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

pub(crate) fn fixture_model() -> Model {
    model()
}

pub(crate) async fn settled_snapshot(manager: &Manager) -> crate::Snapshot {
    settled(manager).await
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

fn registration_document(mut models: Vec<Model>) -> String {
    for model in &mut models {
        model.label = "Fixture native model".to_owned();
        model.source = "https://example.com/native-model".to_owned();
    }
    serde_json::to_string(&Catalog { version: 1, models }).unwrap()
}

#[test]
fn catalog_exposes_native_models_from_distinct_engine_families() {
    let catalog = catalog::catalog().unwrap();
    let engines: Vec<_> = catalog.models.iter().map(|model| model.engine).collect();
    assert!(engines.contains(&EngineKind::Laya));
    assert!(engines.contains(&EngineKind::TransformersSystemOne));
    assert_eq!(catalog.models[1].weights_bytes(), 1_707_072_248);
}

#[test]
fn additive_metadata_preserves_the_existing_model_fingerprint() {
    #[derive(serde::Deserialize, serde::Serialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct LegacyArtifact {
        path: String,
        bytes: u64,
        sha256: String,
        url: String,
        #[serde(default)]
        package: Option<String>,
        #[serde(default)]
        version: Option<String>,
        #[serde(default)]
        license: Option<String>,
    }

    #[derive(serde::Deserialize, serde::Serialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct LegacyModel {
        id: String,
        label: String,
        engine: String,
        protocol: String,
        revision: String,
        license: String,
        source: String,
        quality: String,
        files: Vec<LegacyArtifact>,
    }

    let original: serde_json::Value =
        serde_json::from_str(include_str!("../manifests/models.json")).unwrap();
    let legacy = serde_json::from_value::<LegacyModel>(original["models"][0].clone()).unwrap();
    let current = serde_json::from_value::<Model>(original["models"][0].clone()).unwrap();
    let legacy_bytes = serde_json::to_vec(&legacy).unwrap();
    assert_eq!(
        serde_json::to_vec(&current).unwrap(),
        legacy_bytes,
        "optional discovery fields must not change installed model receipts"
    );
    assert_eq!(
        current.fingerprint().unwrap(),
        format!("{:x}", Sha256::digest(&legacy_bytes)),
    );
    let mut changed = current.clone();
    changed.weights_bytes = Some(current.download_bytes());
    assert_ne!(
        changed.fingerprint().unwrap(),
        current.fingerprint().unwrap()
    );
}

#[test]
fn registration_accepts_multiple_native_models_without_runtime_code() {
    let mut server = model();
    server.id = "fixture-system-one".to_owned();
    server.engine = EngineKind::SystemOneServer;
    server.server_model_id = Some("acme/native-decision".to_owned());
    server.files.clear();
    let parsed = Catalog::parse(&registration_document(vec![model(), server])).unwrap();
    assert_eq!(parsed.models.len(), 2);
    assert_ne!(parsed.models[0].engine, parsed.models[1].engine);
}

#[test]
fn registration_rejects_duplicate_ids_and_nonhex_revisions() {
    assert_eq!(
        Catalog::parse(&registration_document(vec![model(), model()])).unwrap_err(),
        Error::CatalogConflict
    );
    let mut invalid = model();
    invalid.revision = "z".repeat(40);
    assert_eq!(
        Catalog::parse(&registration_document(vec![invalid])).unwrap_err(),
        Error::Manifest
    );
}

#[test]
fn registration_rejects_executable_and_receipt_artifacts() {
    for path in [
        "model.py",
        "weights.pt",
        "runtime.whl",
        "receipt.json",
        "config.json/weights.safetensors",
    ] {
        let mut candidate = model();
        candidate.files.push(artifact(path, b"untrusted"));
        if path.contains('/') {
            candidate.files.push(artifact("config.json", b"{}"));
        }
        assert_eq!(
            Catalog::parse(&registration_document(vec![candidate])).unwrap_err(),
            Error::Manifest,
            "{path}"
        );
    }
}

#[test]
fn registration_rejects_unknown_runtime_and_loader_fields() {
    let mut document: serde_json::Value =
        serde_json::from_str(&registration_document(vec![model()])).unwrap();
    document["runtime"] = serde_json::json!({"command":"untrusted"});
    assert_eq!(
        Catalog::parse(&document.to_string()).unwrap_err(),
        Error::Manifest
    );
    document.as_object_mut().unwrap().remove("runtime");
    document["models"][0]["loader"] = serde_json::json!("untrusted.module");
    assert_eq!(
        Catalog::parse(&document.to_string()).unwrap_err(),
        Error::Manifest
    );
}

#[test]
fn model_weight_metadata_must_match_the_pinned_artifact_sizes() {
    let mut model = model();
    model.weights_bytes = Some(1);
    assert_eq!(
        Catalog::parse(&registration_document(vec![model])).unwrap_err(),
        Error::Manifest
    );
}

#[test]
fn registration_enforces_catalog_size_and_model_count_limits() {
    assert_eq!(
        Catalog::parse(&" ".repeat(catalog::MAX_CATALOG_BYTES + 1)).unwrap_err(),
        Error::TooLarge
    );
    let models = (0..=catalog::MAX_MODELS)
        .map(|index| {
            let mut model = model();
            model.id = format!("fixture-{index}");
            model
        })
        .collect();
    assert_eq!(
        Catalog::parse(&registration_document(models)).unwrap_err(),
        Error::Manifest
    );
}

#[test]
fn registered_sources_survive_restart_without_changing_provider_settings() {
    let home = tempfile::tempdir().unwrap();
    let settings = home.path().join("settings.json");
    fs::write(&settings, b"{}").unwrap();
    let manager = Manager::new(home.path(), &settings, |_| {}).unwrap();
    let before = manager.snapshot();
    let registered = manager
        .register_catalog(
            &before.instance_id,
            &before.catalog_fingerprint,
            &registration_document(vec![model()]),
        )
        .unwrap();
    assert!(
        registered
            .models
            .iter()
            .any(|model| model.id == "fixture-model" && model.origin == "registered")
    );
    assert_eq!(fs::read(&settings).unwrap(), b"{}");
    drop(manager);
    let restored = Manager::new(home.path(), &settings, |_| {})
        .unwrap()
        .snapshot();
    assert_eq!(restored.catalog_fingerprint, registered.catalog_fingerprint);
}

#[test]
fn registration_refuses_stale_instances_and_changed_catalogs() {
    let home = tempfile::tempdir().unwrap();
    let manager = Manager::new(home.path(), &home.path().join("settings.json"), |_| {}).unwrap();
    let before = manager.snapshot();
    let document = registration_document(vec![model()]);
    assert!(matches!(
        manager.register_catalog("old-instance", &before.catalog_fingerprint, &document),
        Err(Error::InstanceChanged)
    ));
    manager
        .register_catalog(&before.instance_id, &before.catalog_fingerprint, &document)
        .unwrap();
    assert!(matches!(
        manager.register_catalog(&before.instance_id, &before.catalog_fingerprint, &document),
        Err(Error::RevisionChanged)
    ));
}

#[test]
fn catalog_identity_does_not_depend_on_registration_directory_order() {
    let home = tempfile::tempdir().unwrap();
    let settings = home.path().join("settings.json");
    let manager = Manager::new(home.path(), &settings, |_| {}).unwrap();
    for id in ["fixture-z", "fixture-a"] {
        let mut model = model();
        model.id = id.to_owned();
        let before = manager.snapshot();
        manager
            .register_catalog(
                &before.instance_id,
                &before.catalog_fingerprint,
                &registration_document(vec![model]),
            )
            .unwrap();
    }
    let expected = manager.snapshot().catalog_fingerprint;
    drop(manager);
    let restored = Manager::new(home.path(), &settings, |_| {}).unwrap();
    assert_eq!(restored.snapshot().catalog_fingerprint, expected);
}

#[test]
fn registration_failure_does_not_publish_partial_catalog_changes() {
    let home = tempfile::tempdir().unwrap();
    let manager = Manager::new(home.path(), &home.path().join("settings.json"), |_| {}).unwrap();
    let before = manager.snapshot();
    let document = registration_document(vec![model()]);
    let addition = Catalog::parse(&document).unwrap();
    let collision = home
        .path()
        .join("managed-judgment/sources")
        .join(addition.fingerprint().unwrap());
    fs::create_dir(&collision).unwrap();
    assert!(matches!(
        manager.register_catalog(&before.instance_id, &before.catalog_fingerprint, &document),
        Err(Error::Storage)
    ));
    assert_eq!(
        manager.snapshot().catalog_fingerprint,
        before.catalog_fingerprint
    );
    assert_eq!(
        fs::read_dir(home.path().join("managed-judgment/partial"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn source_receipts_detect_changed_registration_bytes() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::open(home.path()).unwrap();
    let catalog = Catalog::parse(&registration_document(vec![model()])).unwrap();
    store.register_catalog(&catalog).unwrap();
    let path = home
        .path()
        .join("managed-judgment/sources")
        .join(catalog.fingerprint().unwrap())
        .join("catalog.json");
    let mut tampered = catalog;
    tampered.models[0].files[0].sha256 = "a".repeat(64);
    fs::write(path, serde_json::to_vec(&tampered).unwrap()).unwrap();
    assert_eq!(store.registered_catalogs().unwrap_err(), Error::Integrity);
}

#[test]
fn registration_waits_for_an_active_operation_to_finish() {
    let home = tempfile::tempdir().unwrap();
    let manager = Manager::new(home.path(), &home.path().join("settings.json"), |_| {}).unwrap();
    let before = manager.snapshot();
    test_support::begin_registration_blocker(&manager);
    assert!(matches!(
        manager.register_catalog(
            &before.instance_id,
            &before.catalog_fingerprint,
            &registration_document(vec![model()])
        ),
        Err(Error::Busy)
    ));
    manager.shutdown();
}

#[test]
fn candidate_engines_remain_disabled_before_platform_validation() {
    let home = tempfile::tempdir().unwrap();
    let manager = Manager::new(home.path(), &home.path().join("settings.json"), |_| {}).unwrap();
    let snapshot = manager.snapshot();
    let model = snapshot
        .models
        .iter()
        .find(|model| model.engine == EngineKind::TransformersSystemOne)
        .unwrap();
    assert!(!model.local.supported);
    assert!(model.existing_server.supported);
    assert_eq!(model.server_model_id.as_deref(), Some("vllm-sr/d3-lite"));
    assert!(matches!(
        manager.install(&model.id, &model.revision, None),
        Err(Error::EngineUnverified | Error::Unsupported)
    ));
    assert!(matches!(
        manager.start(&model.id, &model.revision),
        Err(Error::EngineUnverified | Error::Unsupported)
    ));
    assert!(manager.snapshot().operation.is_none());
}

#[test]
fn model_data_rejects_dynamic_loader_configuration_before_install() {
    let home = tempfile::tempdir().unwrap();
    let bytes = br#"{"auto_map":{"AutoModel":"untrusted.Loader"}}"#;
    let mut model = model();
    model.files = vec![artifact("config.json", bytes)];
    fs::write(home.path().join("config.json"), bytes).unwrap();
    assert_eq!(
        crate::adapter::verify_model(home.path(), &model, &AtomicBool::new(false)),
        Err(Error::ExecutableArtifact)
    );
}

#[test]
fn safetensors_indexes_cannot_read_shards_outside_the_registered_data() {
    let home = tempfile::tempdir().unwrap();
    let bytes = br#"{"weight_map":{"weight":"../outside.safetensors"}}"#;
    let mut model = model();
    model.files = vec![artifact("model.safetensors.index.json", bytes)];
    fs::write(home.path().join("model.safetensors.index.json"), bytes).unwrap();
    assert_eq!(
        crate::adapter::verify_model(home.path(), &model, &AtomicBool::new(false)),
        Err(Error::UnsafePath)
    );
}

#[test]
fn engine_selection_uses_the_manifest_engine_instead_of_the_model_name() {
    let mut model = model();
    model.id = "unfamiliar-native-model".to_owned();
    let profile = catalog::runtime().unwrap();
    if let Some(profile) = profile {
        assert!(crate::adapter::local_profile(&model, Some(&profile), false).is_ok());
        model.engine = EngineKind::TransformersSystemOne;
        model.native_format = Some("d3-code-readout-v1".to_owned());
        assert!(matches!(
            crate::adapter::local_profile(&model, Some(&profile), false),
            Err(Error::Unsupported)
        ));
    }
}

#[test]
fn new_runtime_profiles_preserve_the_existing_runtime_receipt_shape() {
    let profiles = catalog::runtime_profiles().unwrap();
    if let Some(original) = catalog::runtime().unwrap() {
        assert_eq!(
            profiles[0].fingerprint().unwrap(),
            original.fingerprint().unwrap()
        );
        let candidate = profiles
            .iter()
            .find(|profile| profile.engine == EngineKind::TransformersSystemOne)
            .unwrap();
        assert!(!candidate.validated);
        let mut validated = candidate.clone();
        validated.validated = true;
        assert_eq!(
            candidate.fingerprint().unwrap(),
            validated.fingerprint().unwrap()
        );
        assert!(
            candidate
                .runtime_files
                .iter()
                .any(|file| file.path == "d3_runtime.py")
        );
        assert!(
            !candidate
                .wheels
                .iter()
                .any(|file| file.package.as_deref() == Some("laya"))
        );
    }
}

#[test]
fn reviewed_runtime_module_tampering_is_rejected_before_spawning() {
    let home = tempfile::tempdir().unwrap();
    let profiles = catalog::runtime_profiles().unwrap();
    if let Some(mut profile) = profiles
        .into_iter()
        .find(|profile| profile.engine == EngineKind::TransformersSystemOne)
    {
        profile.site_packages = "packages".to_owned();
        profile.runtime_files = vec![artifact("d3_runtime.py", b"reviewed")];
        fs::create_dir(home.path().join("packages")).unwrap();
        fs::write(home.path().join("packages/d3_runtime.py"), b"tampered").unwrap();
        assert!(matches!(
            crate::engine::Engine::spawn(home.path(), &profile, None),
            Err(Error::Integrity)
        ));
    }
}

#[cfg(unix)]
#[test]
fn production_bridge_rejects_untrusted_execution_paths_without_loading_a_model() {
    let output = std::process::Command::new("python3")
        .args([
            "-I",
            "-u",
            "-c",
            include_str!("../tests/bridge_boundary.py"),
        ])
        .arg(serde_json::to_string(include_str!("bridge.py")).unwrap())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(feature = "runtime-validation")]
#[test]
fn candidate_validation_requires_a_new_directory_and_owned_settings_path() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("candidate");
    let foreign_settings = home.path().join("settings.json");
    fs::write(&foreign_settings, b"unchanged").unwrap();
    assert!(matches!(
        Manager::for_validation(&root, &foreign_settings, |_| {}),
        Err(Error::UnsafePath)
    ));
    let settings = root.join("settings.json");
    let manager = Manager::for_validation(&root, &settings, |_| {}).unwrap();
    drop(manager);
    assert!(matches!(
        Manager::for_validation(&root, &settings, |_| {}),
        Err(Error::Storage)
    ));
    assert_eq!(fs::read(&foreign_settings).unwrap(), b"unchanged");
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

fn python_archive(path: &Path, links: &[(&str, &str, bool)]) {
    let file = fs::File::create(path).unwrap();
    let gzip = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
    let mut tar = tar::Builder::new(gzip);
    let bytes = b"executable fixture";
    let mut header = tar::Header::new_gnu();
    header.set_mode(0o755);
    header.set_size(bytes.len() as u64);
    header.set_cksum();
    tar.append_data(&mut header, "python/bin/python3.12", &bytes[..])
        .unwrap();
    for (name, target, hard) in links {
        let mut header = tar::Header::new_gnu();
        header.set_mode(0o755);
        header.set_size(0);
        header.set_entry_type(if *hard {
            tar::EntryType::Link
        } else {
            tar::EntryType::Symlink
        });
        tar.append_link(&mut header, name, target).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap();
}

#[test]
fn python_archives_reject_symlink_cycles_and_hardlink_escape() {
    for links in [
        vec![("python/bin/a", "b", false), ("python/bin/b", "a", false)],
        vec![("python/bin/a", "../outside", true)],
        vec![("python/bin/a", "../../outside", false)],
    ] {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let archive = source.path().join("python.tar.gz");
        python_archive(&archive, &links);
        assert_eq!(
            crate::archive::python(
                &archive,
                destination.path(),
                &mut crate::archive::Budget::new(),
                &AtomicBool::new(false)
            ),
            Err(Error::UnsafePath)
        );
    }
}

#[test]
fn python_archives_accept_contained_relative_link_chains() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let archive = source.path().join("python.tar.gz");
    python_archive(
        &archive,
        &[
            ("python/bin/python", "python3", false),
            ("python/bin/python3", "python3.12", false),
        ],
    );
    crate::archive::python(
        &archive,
        destination.path(),
        &mut crate::archive::Budget::new(),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(
        fs::read(destination.path().join("python/bin/python")).unwrap(),
        b"executable fixture"
    );
}

#[test]
fn wheel_extraction_rejects_parent_paths_and_windows_streams() {
    use std::io::Write;
    for name in ["../outside.py", "module.py:stream", "a/CON.txt"] {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let path = source.path().join("fixture.whl");
        let mut zip = zip::ZipWriter::new(fs::File::create(&path).unwrap());
        zip.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"unsafe").unwrap();
        zip.finish().unwrap();
        assert_eq!(
            crate::archive::wheel(
                &path,
                destination.path(),
                "python/site",
                &mut crate::archive::Budget::new(),
                &AtomicBool::new(false)
            ),
            Err(Error::UnsafePath)
        );
    }
}

#[test]
fn wheel_installation_keeps_data_and_omits_executable_startup_hooks() {
    use std::io::Write;
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let path = source.path().join("fixture.whl");
    let mut zip = zip::ZipWriter::new(fs::File::create(&path).unwrap());
    for (name, bytes) in [
        ("module/__init__.py", b"pass".as_slice()),
        ("startup.pth", b"import unwanted".as_slice()),
        ("fixture.data/scripts/run", b"unwanted".as_slice()),
    ] {
        zip.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap();
    crate::archive::wheel(
        &path,
        destination.path(),
        "python/site",
        &mut crate::archive::Budget::new(),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(
        fs::read(destination.path().join("python/site/module/__init__.py")).unwrap(),
        b"pass"
    );
    assert!(!destination.path().join("python/site/startup.pth").exists());
    assert!(!destination.path().join("python/bin/run").exists());
}

async fn stream_server(
    body: Vec<u8>,
    declared: usize,
    pause: bool,
) -> (String, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0; 2048];
        let request_bytes = stream.read(&mut request).await.unwrap();
        assert!(request_bytes > 0);
        stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {declared}\r\nConnection: close\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        stream.write_all(&body).await.unwrap();
        if pause {
            let mut next = [0; 1];
            let closed = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut next))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(closed, 0, "cancelled response must close its stream");
        }
    });
    (address, task)
}

#[tokio::test]
async fn cancellation_closes_the_actual_download_stream_and_cleans_partial_files() {
    let home = tempfile::tempdir().unwrap();
    let manager = test_support::manager(home.path(), &home.path().join("settings.json"), model());
    let (address, server) = stream_server(vec![b'a'; 16], 32, true).await;
    let operation = test_support::stream(
        &manager,
        artifact("model.safetensors", &[b'a'; 32]),
        address,
    )
    .operation
    .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while manager
            .snapshot()
            .operation
            .as_ref()
            .unwrap()
            .received_bytes
            == 0
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    manager.cancel(&operation.id).unwrap();
    let result = settled(&manager).await;
    assert_eq!(result.operation.unwrap().state, "cancelled");
    assert_eq!(
        fs::read_dir(home.path().join("managed-judgment/partial"))
            .unwrap()
            .count(),
        0
    );
    assert!(!result.models[0].installed);
    server.await.unwrap();
}

#[tokio::test]
async fn a_download_with_the_correct_length_and_wrong_digest_never_installs() {
    let home = tempfile::tempdir().unwrap();
    let manager = test_support::manager(home.path(), &home.path().join("settings.json"), model());
    let (address, server) = stream_server(vec![b'b'; 32], 32, false).await;
    test_support::stream(
        &manager,
        artifact("model.safetensors", &[b'a'; 32]),
        address,
    );
    let result = settled(&manager).await;
    assert_eq!(
        result.operation.unwrap().error,
        Some(Error::Integrity.token())
    );
    assert_eq!(
        fs::read_dir(home.path().join("managed-judgment/partial"))
            .unwrap()
            .count(),
        0
    );
    server.await.unwrap();
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
        manager.apply("stale", "old", |_, _, _| panic!("must not write")),
        Err(Error::ServerStopped)
    ));
    assert_eq!(fs::read(settings).unwrap(), original);
}

#[cfg(unix)]
fn fake_engine() -> std::sync::Arc<crate::engine::Engine> {
    native_fixture_engine(false)
}

#[cfg(unix)]
pub(crate) fn ready_fixture_engine() -> std::sync::Arc<crate::engine::Engine> {
    native_fixture_engine(true)
}

#[cfg(unix)]
pub(crate) fn running_fixture_engine() -> std::sync::Arc<crate::engine::Engine> {
    fake_engine()
}

#[cfg(unix)]
fn native_fixture_engine(ready: bool) -> std::sync::Arc<crate::engine::Engine> {
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
    let source = if ready {
        "print('{\"ready\":true}',flush=True)\n".to_owned() + source
    } else {
        source.to_owned()
    };
    crate::engine::Engine::from_command(
        std::process::Command::new("python3").args(["-I", "-u", "-c", &source]),
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
async fn a_text_only_server_explicitly_rejects_media_before_native_inference() {
    let home = tempfile::tempdir().unwrap();
    let manager = test_support::manager(home.path(), &home.path().join("settings.json"), model());
    let (server_id, provider) = test_support::running(&manager, fake_engine(), false).await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    for input in ["images", "videos"] {
        let request = json!({"model":provider.model,"state":"fixture","questions":{"answer":{"type":"noul"}},(input):[]});
        let response = client
            .post(provider.request_url())
            .json(&request)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);
        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["error"], "input_not_supported");
    }
    manager.stop(&server_id).unwrap();
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
        manager.apply(&id, &expected, |_, _, _| panic!("unchecked must not write")),
        Err(Error::NotChecked)
    ));
    manager.check(&id).unwrap();
    assert_eq!(settled(&manager).await.operation.unwrap().state, "done");
    assert!(matches!(
        manager.apply("old-server", &expected, |_, _, _| panic!(
            "stale must not write"
        )),
        Err(Error::ServerChanged)
    ));
    assert!(matches!(
        manager.apply(&id, "old-provider", |_, _, _| panic!(
            "changed must not write"
        )),
        Err(Error::SettingsChanged)
    ));
    let applied = manager
        .apply(&id, &expected, |provider, expected, binding| {
            assert_eq!(
                judgment_provider::Provider::read(&settings)
                    .unwrap()
                    .identity(),
                expected
            );
            fs::write(
                &settings,
                serde_json::to_vec(&json!({"smart":{"judgmentProvider":provider,(crate::MANAGED_MODEL_KEY):binding}})).unwrap(),
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

#[cfg(unix)]
#[tokio::test]
async fn a_readiness_timeout_reaps_a_real_blocked_child() {
    let engine = crate::engine::Engine::from_command(
        std::process::Command::new("sh").args(["-c", "while IFS= read -r line; do :; done"]),
    );
    assert_eq!(
        test_support::ready_timeout(&engine).await,
        Err(Error::Timeout)
    );
    assert!(!engine.alive());
}

#[cfg(unix)]
#[tokio::test]
async fn the_production_parent_watcher_exits_when_its_pipe_reaches_eof() {
    use std::process::{Command, Stdio};
    let source = include_str!("bridge.py")
        .split("\ntry:\n    initial")
        .next()
        .unwrap()
        .to_owned()
        + "\nthreading.Event().wait()\n";
    let mut child = Command::new("python3")
        .args(["-I", "-u", "-c", &source])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    drop(child.stdin.take());
    let status = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                return status;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    if status.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    assert!(status.unwrap().success());
}

#[cfg(unix)]
#[tokio::test]
async fn chat_text_cannot_pass_the_native_connection_check() {
    let home = tempfile::tempdir().unwrap();
    let settings = home.path().join("settings.json");
    fs::write(&settings, b"{}").unwrap();
    let manager = test_support::manager(home.path(), &settings, model());
    let source = "import sys\nfor line in sys.stdin: print('{\"choices\":[{\"message\":{\"content\":\"0.9\"}}]}',flush=True)\n";
    let engine = crate::engine::Engine::from_command(
        std::process::Command::new("python3").args(["-I", "-u", "-c", source]),
    );
    let (id, _) = test_support::running(&manager, engine, false).await;
    manager.check(&id).unwrap();
    let failed = settled(&manager).await;
    assert_eq!(failed.operation.unwrap().state, "failed");
    assert!(!failed.server.checked);
    assert_eq!(fs::read(settings).unwrap(), b"{}");
    manager.shutdown();
}

#[cfg(unix)]
#[tokio::test]
async fn an_idle_child_exit_emits_failure_without_a_status_poll() {
    use std::sync::atomic::Ordering;
    let engine =
        crate::engine::Engine::from_command(std::process::Command::new("sleep").arg("0.05"));
    let notified = std::sync::Arc::new(AtomicBool::new(false));
    let event = notified.clone();
    let running = crate::engine::serve(
        &model(),
        engine.clone(),
        std::sync::Arc::new(move || {
            event.store(true, Ordering::Release);
        }),
    )
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while !notified.load(Ordering::Acquire) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(!engine.alive());
    drop(running);
}

#[test]
fn a_manual_provider_change_emits_the_new_identity_without_starting_a_server() {
    let home = tempfile::tempdir().unwrap();
    let settings = home.path().join("settings.json");
    let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let observed = events.clone();
    let manager = Manager::new(home.path(), &settings, move |snapshot| {
        observed.lock().unwrap().push(snapshot);
    })
    .unwrap();
    let before = manager.snapshot();
    let provider = judgment_provider::Provider {
        kind: judgment_provider::ProviderKind::Remote,
        protocol: judgment_provider::Protocol::SystemOne,
        base_url: "https://example.invalid".to_owned(),
        model: "fixture-native".to_owned(),
        requires_auth: false,
    };
    fs::write(
        &settings,
        serde_json::to_vec(&serde_json::json!({"smart":{"judgmentProvider":provider}})).unwrap(),
    )
    .unwrap();
    manager.notify_settings_changed();
    let event = events.lock().unwrap().pop().unwrap();
    assert_eq!(event.provider_identity, Some(provider.identity()));
    assert!(event.revision > before.revision);
    assert!(event.configured_model_id.is_none());
    assert_eq!(event.server.state, "stopped");
}

#[test]
fn concurrent_snapshots_cannot_give_an_old_provider_a_newer_revision() {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Barrier, Mutex};
    let home = tempfile::tempdir().unwrap();
    let settings = home.path().join("settings.json");
    let mut identities = BTreeMap::new();
    let mut documents = Vec::new();
    for index in 0..=100 {
        let provider = judgment_provider::Provider {
            kind: judgment_provider::ProviderKind::Remote,
            protocol: judgment_provider::Protocol::SystemOne,
            base_url: "https://example.invalid".to_owned(),
            model: format!("fixture-{index}"),
            requires_auth: false,
        };
        identities.insert(provider.identity(), index);
        documents.push(
            serde_json::to_vec(&serde_json::json!({"smart":{"judgmentProvider":provider}}))
                .unwrap(),
        );
    }
    fs::write(&settings, &documents[0]).unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let observed = events.clone();
    let manager = Manager::new(home.path(), &settings, move |snapshot| {
        observed
            .lock()
            .unwrap()
            .push((snapshot.revision, snapshot.provider_identity.unwrap()));
    })
    .unwrap();
    let start = Arc::new(Barrier::new(9));
    std::thread::scope(|scope| {
        for _ in 0..8 {
            let manager = manager.clone();
            let start = start.clone();
            scope.spawn(move || {
                start.wait();
                for _ in 0..200 {
                    manager.notify_settings_changed();
                }
            });
        }
        start.wait();
        for document in &documents[1..] {
            let temporary = home.path().join("settings-next.json");
            fs::write(&temporary, document).unwrap();
            fs::rename(temporary, &settings).unwrap();
            manager.notify_settings_changed();
        }
    });
    let mut events = events.lock().unwrap();
    events.sort_by_key(|(revision, _)| *revision);
    let versions: Vec<_> = events
        .iter()
        .map(|(_, identity)| identities[identity])
        .collect();
    assert_eq!(events.len(), 1700);
    assert!(
        versions.windows(2).all(|pair| pair[0] <= pair[1]),
        "a stale provider observation received a newer revision"
    );
    assert_eq!(versions.last(), Some(&100));
}

#[cfg(unix)]
#[test]
fn importing_a_named_pipe_refuses_it_before_opening_a_reader() {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::OpenOptionsExt;
    let source = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let pipe = source.path().join("model.safetensors");
    let name = std::ffi::CString::new(pipe.as_os_str().as_bytes()).unwrap();
    // SAFETY: name is terminated, and both directories belong to this test.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let source_path = source.path().to_owned();
    let target_path = target.path().to_owned();
    let (send, receive) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let result = test_support::copy_import(
            &source_path,
            &target_path,
            &artifact("model.safetensors", b"weights"),
            &AtomicBool::new(false),
            |_| {},
        );
        send.send(result).unwrap();
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let mut reader_opened = false;
    let result = loop {
        if let Ok(result) = receive.try_recv() {
            break result;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "named pipe import did not settle"
        );
        match fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&pipe)
        {
            Ok(writer) => {
                reader_opened = true;
                drop(writer);
            }
            Err(error) => assert_eq!(error.raw_os_error(), Some(libc::ENXIO)),
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    reader.join().unwrap();
    assert_eq!(result, Err(Error::Integrity));
    assert!(
        !reader_opened,
        "a non-file source must be refused before a blocking open"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn failed_apply_does_not_erase_the_previously_configured_model() {
    let home = tempfile::tempdir().unwrap();
    let settings = home.path().join("settings.json");
    let model = model();
    let original = judgment_provider::Provider {
        kind: judgment_provider::ProviderKind::Local,
        protocol: judgment_provider::Protocol::SystemOne,
        base_url: "http://127.0.0.1:43127".to_owned(),
        model: model.id.clone(),
        requires_auth: false,
    };
    let identity = original.identity();
    let binding = json!({"modelId":model.id,"revision":model.revision,"providerIdentity":identity});
    let document = serde_json::to_vec(
        &json!({"smart":{"judgmentProvider":original,"managedJudgmentModel":binding}}),
    )
    .unwrap();
    fs::write(&settings, &document).unwrap();
    let manager = test_support::manager(home.path(), &settings, model.clone());
    let mut history = vec![binding];
    for index in 1..16 {
        history.push(json!({"modelId":model.id,"revision":model.revision,"providerIdentity":format!("historical-{index}")}));
    }
    fs::write(
        home.path().join("managed-judgment/bindings.json"),
        serde_json::to_vec(&history).unwrap(),
    )
    .unwrap();
    assert_eq!(
        manager.snapshot().configured_model_id.as_deref(),
        Some(model.id.as_str())
    );
    let (server, provider) = test_support::running(&manager, fake_engine(), false).await;
    assert_ne!(provider.identity(), identity);
    manager.check(&server).unwrap();
    assert_eq!(settled(&manager).await.operation.unwrap().state, "done");
    let result = test_support::fail_provider_write(&manager, &server, &identity, &settings);
    manager.stop(&server).unwrap();
    assert!(matches!(result, Err(Error::Storage)));
    assert_eq!(fs::read(&settings).unwrap(), document);
    assert_eq!(
        manager.snapshot().configured_model_id.as_deref(),
        Some(model.id.as_str()),
        "a failed provider write must preserve the configured model"
    );
    assert!(matches!(
        manager.remove(&model.id, &model.revision),
        Err(Error::Configured)
    ));
}
