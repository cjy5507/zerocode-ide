use std::fs;
use std::sync::atomic::AtomicBool;

use local_judgment::{Error, Result};

#[path = "../src/gguf.rs"]
mod gguf;

fn string(value: &str) -> Vec<u8> {
    let mut bytes = (value.len() as u64).to_le_bytes().to_vec();
    bytes.extend(value.as_bytes());
    bytes
}

fn field(name: &str, kind: u32, value: Vec<u8>) -> Vec<u8> {
    let mut bytes = string(name);
    bytes.extend(kind.to_le_bytes());
    bytes.extend(value);
    bytes
}

fn fixture(extra: Vec<Vec<u8>>) -> Vec<u8> {
    let mut bytes = b"GGUF".to_vec();
    bytes.extend(3_u32.to_le_bytes());
    bytes.extend(1_u64.to_le_bytes());
    bytes.extend((1 + extra.len() as u64).to_le_bytes());
    bytes.extend(field("general.architecture", 8, string("fixture")));
    for value in extra {
        bytes.extend(value);
    }
    bytes.extend(string("weight"));
    bytes.extend(1_u32.to_le_bytes());
    bytes.extend(32_u64.to_le_bytes());
    bytes.extend(0_u32.to_le_bytes());
    bytes.extend(0_u64.to_le_bytes());
    bytes.resize(bytes.len().div_ceil(32) * 32, 0);
    bytes.extend([0_u8; 128]);
    bytes
}

fn inspect(bytes: &[u8]) -> Result<gguf::Info> {
    let home = tempfile::tempdir().expect("owned input");
    let file = home.path().join("model.gguf");
    fs::write(&file, bytes).expect("GGUF fixture");
    gguf::inspect(&file, &AtomicBool::new(false))
}

#[test]
fn bounded_gguf_metadata_is_read_without_loading_weights_or_model_code() {
    let bytes = fixture(Vec::new());
    let parsed = inspect(&bytes);
    assert!(
        parsed.is_ok(),
        "a structurally bounded GGUF was rejected: {parsed:?}"
    );
    let parsed = parsed.expect("asserted GGUF");
    assert_eq!(parsed.architecture, "fixture");
    assert_eq!(parsed.tensors, 1);
    assert_eq!(parsed.file_bytes, bytes.len() as u64);
    assert_eq!(parsed.template, None);
}

#[test]
fn an_embedded_template_is_only_metadata_for_the_pinned_native_detector() {
    let template = "an unknown template must never be executed";
    let parsed = inspect(&fixture(vec![field(
        "tokenizer.chat_template",
        8,
        string(template),
    )]));
    assert!(
        parsed.is_ok(),
        "bounded template metadata was rejected: {parsed:?}"
    );
    assert_eq!(
        parsed.expect("asserted metadata").template.as_deref(),
        Some(template)
    );
}

#[test]
fn tokenizer_arrays_are_bounded_and_skipped_without_allocating_a_token_table() {
    let mut tokens = 8_u32.to_le_bytes().to_vec();
    tokens.extend(2_u64.to_le_bytes());
    tokens.extend(string("first"));
    tokens.extend(string("second"));
    let parsed = inspect(&fixture(vec![field("tokenizer.ggml.tokens", 9, tokens)]));
    assert!(
        parsed.is_ok(),
        "bounded token array was rejected: {parsed:?}"
    );
}

#[test]
fn truncated_or_forged_gguf_headers_are_refused() {
    let valid = fixture(Vec::new());
    for size in [0, 4, 8, 16, 23, 28, 40, valid.len() - 129] {
        assert!(inspect(&valid[..size]).is_err(), "accepted {size} bytes");
    }
    let mut wrong_magic = valid.clone();
    wrong_magic[..4].copy_from_slice(b"NOPE");
    assert!(inspect(&wrong_magic).is_err());
    let mut wrong_version = valid;
    wrong_version[4..8].copy_from_slice(&99_u32.to_le_bytes());
    assert!(inspect(&wrong_version).is_err());
}

#[test]
fn malformed_array_lengths_and_nested_arrays_are_refused() {
    for (kind, count) in [(8_u32, u64::MAX), (9, 1), (255, 1)] {
        let mut array = kind.to_le_bytes().to_vec();
        array.extend(count.to_le_bytes());
        assert!(inspect(&fixture(vec![field("tokenizer.ggml.tokens", 9, array)])).is_err());
    }
}

#[test]
fn split_models_and_duplicate_metadata_are_not_silently_accepted() {
    assert!(
        inspect(&fixture(vec![field(
            "split.count",
            2,
            2_u16.to_le_bytes().to_vec()
        )]))
        .is_err()
    );
    assert!(
        inspect(&fixture(vec![field(
            "general.architecture",
            8,
            string("other")
        )]))
        .is_err()
    );
}

#[test]
fn oversized_declared_strings_and_tensor_counts_are_refused() {
    let mut long_string = fixture(Vec::new());
    long_string[24..32].copy_from_slice(&u64::MAX.to_le_bytes());
    assert!(inspect(&long_string).is_err());
    let mut many_tensors = fixture(Vec::new());
    many_tensors[8..16].copy_from_slice(&u64::MAX.to_le_bytes());
    assert!(inspect(&many_tensors).is_err());
}

#[test]
fn cancellation_is_checked_before_opening_a_model() {
    assert!(matches!(
        gguf::inspect(std::path::Path::new("absent.gguf"), &AtomicBool::new(true)),
        Err(Error::Cancelled)
    ));
}

#[cfg(unix)]
#[test]
fn a_model_symlink_is_not_followed() {
    let home = tempfile::tempdir().expect("owned input");
    let target = home.path().join("original.gguf");
    let link = home.path().join("link.gguf");
    fs::write(&target, fixture(Vec::new())).expect("GGUF");
    std::os::unix::fs::symlink(&target, &link).expect("symlink");
    assert!(gguf::inspect(&link, &AtomicBool::new(false)).is_err());
}

#[test]
fn native_template_detection_keeps_distinct_model_families_and_does_not_evaluate_jinja() {
    for (template, expected) in [
        ("chatml", "chatml"),
        ("<|im_start|> {{ untrusted_expression() }}", "chatml"),
        ("<|im_start|> <|im_sep|>", "phi4"),
        ("<|im_start|> <end_of_utterance>", "smolvlm"),
        ("<|start_header_id|> <|end_header_id|>", "llama3"),
        ("<start_of_turn>", "gemma"),
        ("<|assistant|> <|end|>", "phi3"),
        ("[INST] [SYSTEM_PROMPT]", "mistral-v7"),
        (" [INST] [AVAILABLE_TOOLS]", "mistral-v1"),
        ("[INST] <<SYS>>", "llama2-sys"),
    ] {
        assert_eq!(
            gguf::builtin_template(template),
            Some(expected),
            "{template}"
        );
    }
}

#[test]
fn unknown_template_code_is_rejected_instead_of_guessed_as_chatml() {
    for template in [
        "",
        "unknown prompt layout",
        "{{ arbitrary_function() }}",
        "{%- for message in messages -%}{{ message }}{%- endfor -%}",
    ] {
        assert_eq!(gguf::builtin_template(template), None, "{template}");
    }
}

#[tokio::test]
async fn a_selected_local_gguf_is_inspected_without_changing_the_active_provider() {
    let home = tempfile::tempdir().expect("owned store");
    let input = tempfile::tempdir().expect("owned input");
    let file = input.path().join("selected.gguf");
    let bytes = fixture(vec![field("tokenizer.chat_template", 8, string("chatml"))]);
    fs::write(&file, &bytes).expect("model");
    let manager =
        local_judgment::Manager::new(home.path(), &home.path().join("settings.json"), |_| {})
            .expect("manager");
    let before = manager.snapshot();
    let source = serde_json::from_value::<local_judgment::SourceInput>(
        serde_json::json!({"kind":"local_file","file":file}),
    );
    assert!(source.is_ok(), "a GGUF file source was refused: {source:?}");
    let inspected = manager
        .inspect_source(source.expect("asserted source"))
        .await;
    assert!(
        inspected.is_ok(),
        "a selected GGUF was not inspected: {:?}",
        inspected.as_ref().err()
    );
    let inspected = inspected.expect("asserted inspection");
    let shown = serde_json::to_value(&inspected).expect("inspection");
    assert_eq!(shown["model"]["engine"], "llama_cpp");
    assert_eq!(shown["model"]["protocol"], "openai_json");
    assert_eq!(shown["model"]["nativeFormat"], "gguf-v3");
    assert_eq!(shown["model"]["downloadBytes"], bytes.len() as u64);
    let after = manager.snapshot();
    assert_eq!(after.provider_identity, before.provider_identity);
    let operation = after.operation.as_ref().expect("inspection record");
    assert_eq!(
        (operation.kind, operation.state, operation.error),
        ("inspect", "done", None),
        "inspection_did_not_finish: kind={} state={} error={:?}",
        operation.kind,
        operation.state,
        operation.error
    );
    assert_eq!(after.models.len(), before.models.len());
    assert!(
        after.models.iter().all(|model| !model.installed),
        "inspection_installed_model_data"
    );
    assert_eq!(
        after.runtime.state, before.runtime.state,
        "inspection_prepared_a_runtime"
    );
    assert_eq!(
        serde_json::to_value(&after.server).unwrap(),
        serde_json::to_value(&before.server).unwrap(),
        "inspection_started_a_server"
    );
    assert_eq!(fs::read(&file).expect("original"), bytes);
    manager.shutdown();
}

#[tokio::test]
async fn a_local_gguf_with_an_unknown_template_never_reaches_an_install_operation() {
    let home = tempfile::tempdir().expect("owned store");
    let file = home.path().join("selected.gguf");
    fs::write(
        &file,
        fixture(vec![field(
            "tokenizer.chat_template",
            8,
            string("unknown template"),
        )]),
    )
    .expect("model");
    let manager =
        local_judgment::Manager::new(home.path(), &home.path().join("settings.json"), |_| {})
            .expect("manager");
    let before = manager.snapshot();
    let source = serde_json::from_value::<local_judgment::SourceInput>(
        serde_json::json!({"kind":"local_file","file":file}),
    )
    .expect("file source");
    assert!(matches!(
        manager.inspect_source(source).await,
        Err(Error::ModelFormatUnsupported)
    ));
    let after = manager.snapshot();
    let operation = after.operation.as_ref().expect("refused inspection record");
    assert_eq!(
        (operation.kind, operation.state, operation.error),
        (
            "inspect",
            "failed",
            Some(Error::ModelFormatUnsupported.token())
        ),
        "unsupported_template_was_not_refused: kind={} state={} error={:?}",
        operation.kind,
        operation.state,
        operation.error
    );
    assert_eq!(
        after.provider_identity, before.provider_identity,
        "unsupported_template_changed_the_provider"
    );
    assert_eq!(after.models.len(), before.models.len());
    assert!(
        after.models.iter().all(|model| !model.installed),
        "unsupported_template_installed_model_data"
    );
    assert!(after.source_inspection.is_none());
    assert_eq!(
        after.runtime.state, before.runtime.state,
        "unsupported_template_prepared_a_runtime"
    );
    assert_eq!(
        serde_json::to_value(&after.server).unwrap(),
        serde_json::to_value(&before.server).unwrap(),
        "unsupported_template_started_a_server"
    );
    manager.shutdown();
}
