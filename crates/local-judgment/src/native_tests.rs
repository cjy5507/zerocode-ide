//! Integration boundaries use synthetic files and an owned fake pipe, never a model.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use judgment_provider::Protocol;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::catalog::{self, Artifact, EngineKind};
use crate::engine::{Engine, Limits};
use crate::storage::Store;
use crate::{Manager, SourceInput};

fn artifact(path: &str, bytes: &[u8]) -> Artifact {
    Artifact {
        path: path.to_owned(),
        bytes: bytes.len() as u64,
        sha256: format!("{:x}", Sha256::digest(bytes)),
        url: "https://example.invalid/runtime.tar.gz".to_owned(),
        package: None,
        version: None,
        license: None,
    }
}

fn gguf() -> Vec<u8> {
    fn text(bytes: &mut Vec<u8>, value: &str) {
        bytes.extend((value.len() as u64).to_le_bytes());
        bytes.extend(value.as_bytes());
    }
    let mut bytes = b"GGUF".to_vec();
    bytes.extend(3_u32.to_le_bytes());
    bytes.extend(1_u64.to_le_bytes());
    bytes.extend(2_u64.to_le_bytes());
    for (name, value) in [
        ("general.architecture", "fixture"),
        ("tokenizer.chat_template", "chatml"),
    ] {
        text(&mut bytes, name);
        bytes.extend(8_u32.to_le_bytes());
        text(&mut bytes, value);
    }
    text(&mut bytes, "weight");
    bytes.extend(1_u32.to_le_bytes());
    bytes.extend(32_u64.to_le_bytes());
    bytes.extend(0_u32.to_le_bytes());
    bytes.extend(0_u64.to_le_bytes());
    bytes.resize(bytes.len().div_ceil(32) * 32 + 128, 0);
    bytes
}

async fn inspected() -> (tempfile::TempDir, Arc<Manager>, crate::SourceInspection) {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("selected.gguf");
    fs::write(&file, gguf()).unwrap();
    let manager = Manager::new(root.path(), &root.path().join("settings.json"), |_| {}).unwrap();
    let inspection = manager
        .inspect_source(SourceInput::LocalFile {
            file: file.to_string_lossy().into_owned(),
        })
        .await
        .unwrap();
    (root, manager, inspection)
}

#[tokio::test]
async fn native_inspection_reports_a_file_instead_of_a_directory() {
    let (_root, manager, inspection) = inspected().await;
    manager.shutdown();
    assert_eq!(
        inspection.source_kind, "local_file",
        "native_source_kind_was_lost"
    );
}

#[tokio::test]
async fn native_inspection_reports_the_detected_template_without_claiming_validation() {
    let (_root, manager, inspection) = inspected().await;
    let value = serde_json::to_value(inspection).unwrap();
    manager.shutdown();
    assert_eq!(
        value["inputTemplate"], "chatml",
        "native_input_template_was_lost"
    );
    assert_eq!(value["local"]["supported"], false);
    assert_eq!(value["local"]["downloadOnly"], true);
    assert_eq!(value["local"]["reason"], "engine_unverified");
}

#[tokio::test]
async fn an_installed_native_template_is_kept_without_reading_weights_for_status() {
    let (_root, manager, inspection) = inspected().await;
    let id = inspection.model.id.clone();
    manager
        .install_source(
            &inspection.inspection_id,
            &inspection.instance_id,
            &inspection.catalog_fingerprint,
            None,
        )
        .unwrap();
    let settled = crate::tests::settled_snapshot(&manager).await;
    assert_eq!(settled.operation.as_ref().unwrap().state, "done");
    let model = settled
        .models
        .into_iter()
        .find(|model| model.id == id)
        .unwrap();
    let directory = std::path::PathBuf::from(model.installed_directory.unwrap());
    let gguf = fs::read_dir(&directory)
        .unwrap()
        .flatten()
        .find(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "gguf")
        })
        .unwrap()
        .path();
    fs::set_permissions(&gguf, fs::Permissions::from_mode(0o000)).unwrap();
    let snapshot = manager.snapshot();
    fs::set_permissions(gguf, fs::Permissions::from_mode(0o600)).unwrap();
    manager.shutdown();
    let model = snapshot
        .models
        .into_iter()
        .find(|model| model.id == id)
        .unwrap();
    assert_eq!(
        serde_json::to_value(model).unwrap()["inputTemplate"],
        "chatml",
        "installed_native_template_was_not_kept"
    );
}

#[test]
fn a_native_runtime_receipt_does_not_replace_its_pinned_files() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let profile = catalog::runtime_profiles()
        .unwrap()
        .into_iter()
        .find(|profile| profile.engine == EngineKind::LlamaCpp)
        .unwrap();
    let mut stage = store.stage().unwrap();
    let python = stage.path().join(&profile.executable);
    fs::create_dir_all(python.parent().unwrap()).unwrap();
    fs::write(python, b"synthetic interpreter").unwrap();
    store.commit_runtime(&mut stage, &profile).unwrap();
    assert!(
        !store.runtime_ready(&profile),
        "missing_native_files_were_reported_ready"
    );
}

#[test]
fn native_start_passes_the_verified_executable_to_the_supervisor() {
    let root = tempfile::tempdir().unwrap();
    let mut profile = catalog::runtime_profiles()
        .unwrap()
        .into_iter()
        .find(|profile| profile.engine == EngineKind::LlamaCpp)
        .unwrap();
    profile.executable = "fake-python".to_owned();
    let native = b"\xcf\xfa\xed\xfe\x0c\0\0\x01";
    profile.native = Some(crate::llama::Runtime {
        archive: artifact("native.tar.gz", b"synthetic archive"),
        executable: "package/llama-server".to_owned(),
        files: vec![artifact("package/llama-server", native)],
    });
    let server = root.path().join("native/package/llama-server");
    fs::create_dir_all(server.parent().unwrap()).unwrap();
    fs::write(&server, native).unwrap();
    // This interpreter records the supervisor's initial frame. It never runs native code.
    let python = root.path().join(&profile.executable);
    fs::write(&python, "#!/bin/sh\nexec /usr/bin/python3 -I -u -c 'import json,sys; r=json.loads(sys.stdin.readline()); open(\"initial.json\",\"w\").write(json.dumps(r)); print(json.dumps({\"ready\":True}),flush=True); sys.stdin.read()'\n").unwrap();
    fs::set_permissions(&python, fs::Permissions::from_mode(0o700)).unwrap();
    let engine = Engine::spawn(root.path(), &profile, None).unwrap();
    engine.ready().unwrap();
    engine.stop();
    let initial: Value =
        serde_json::from_slice(&fs::read(root.path().join("initial.json")).unwrap()).unwrap();
    assert_eq!(
        initial["nativeExecutable"],
        json!(server),
        "native_executable_was_not_passed"
    );
    assert_eq!(initial["engine"], "llama_cpp");
    assert_eq!(initial["mode"], "verify");
}

#[tokio::test]
async fn the_owned_native_http_route_keeps_the_model_protocol_and_checks_the_response() {
    let root = tempfile::tempdir().unwrap();
    let fake = r#"import json,sys
for line in sys.stdin:
 r=json.loads(line)
 content=json.loads(r['messages'][1]['content'])
 answer={'answers':{'answer':{'type':'noul','noul':2 if content['state']=='invalid' else 0.5}}}
 print(json.dumps({'model':r['model'],'choices':[{'index':0,'finish_reason':'stop','message':{'role':'assistant','content':json.dumps(answer)}}],'usage':{'prompt_tokens':1,'completion_tokens':1}}),flush=True)
"#;
    let engine = Engine::from_command(
        Command::new("python3")
            .args(["-I", "-u", "-c", fake])
            .current_dir(root.path()),
    );
    let mut model = crate::tests::fixture_model();
    model.engine = EngineKind::LlamaCpp;
    model.protocol = Protocol::OpenaiJson;
    model.native_format = Some("gguf-v3".to_owned());
    let started =
        crate::engine::serve_limited(&model, engine.clone(), Arc::new(|| {}), Limits::DEFAULT)
            .await;
    if started.is_err() {
        engine.stop();
    }
    assert!(started.is_ok(), "native_http_route_was_not_available");
    let running = started.unwrap();
    assert_eq!(
        running.provider.protocol,
        Protocol::OpenaiJson,
        "native_model_protocol_was_replaced"
    );
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    for (state, expected) in [("valid", 200), ("invalid", 502)] {
        let request = json!({"model":running.provider.identity(),"state":state,"questions":{"answer":{"type":"noul","instructions":"Is the page ready?"}}});
        let prepared = running
            .provider
            .prepare(serde_json::to_vec(&request).unwrap())
            .unwrap();
        let response = client
            .post(running.provider.request_url())
            .body(prepared.body)
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status().as_u16(),
            expected,
            "native_response_validation_was_skipped"
        );
    }
    let response = client
        .post(running.provider.request_url())
        .json(&json!({"model":model.id,"state":"not a prepared request","questions":{}}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status().as_u16(),
        400,
        "unprepared_native_request_was_forwarded"
    );
    running.retire().await;
}

#[test]
fn native_stop_reaps_the_owned_process_group_including_the_native_child() {
    let fake = r#"import json,subprocess,sys,time
child=subprocess.Popen(['sleep','30'],stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
sys.stdin.readline()
print(json.dumps({'child':child.pid}),flush=True)
time.sleep(30)
"#;
    let engine =
        Engine::from_native_command(Command::new("python3").args(["-I", "-u", "-c", fake]));
    let response: Value = serde_json::from_slice(&engine.exchange(b"{}").unwrap()).unwrap();
    let child = i32::try_from(response["child"].as_u64().unwrap()).unwrap();
    engine.stop();
    let until = Instant::now() + Duration::from_secs(2);
    // The PID came from this invocation's owned child, and remains a sleeping fixture.
    while unsafe { libc::kill(child, 0) } == 0 && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(10));
    }
    let remained = unsafe { libc::kill(child, 0) } == 0;
    if remained {
        unsafe { libc::kill(child, libc::SIGKILL) };
    }
    assert!(!remained, "native_child_survived_owner_stop");
}
