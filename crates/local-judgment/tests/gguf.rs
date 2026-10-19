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
