//! The bundle's resources are said in three files and merged by tauri-cli in
//! order — `tauri.conf.json`, then `tauri.macos.conf.json`, then every
//! `--config` (`tauri.release.conf.json` on the release lane) — with
//! `json_patch::merge`: an object merges key by key, anything else replaces
//! the whole value. A release list therefore erased the macOS map and shipped
//! v1.3.0/1.3.1 without `ZeroCode Computer Use.app`. This models that merge
//! and pins what the macOS release bundle must carry.

use serde_json::Value;
use std::path::{Path, PathBuf};

fn shell_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn conf(name: &str) -> Value {
    let path = shell_dir().join(name);
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|why| panic!("{}: {why}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|why| panic!("{}: {why}", path.display()))
}

/// `json_patch::merge` as tauri-cli applies it (RFC 7396 merge patch).
fn merge(doc: &mut Value, patch: &Value) {
    let Value::Object(patch) = patch else {
        *doc = patch.clone();
        return;
    };
    if !doc.is_object() {
        *doc = Value::Object(serde_json::Map::new());
    }
    let map = doc.as_object_mut().expect("an object");
    for (key, value) in patch {
        if value.is_null() {
            map.remove(key);
        } else {
            merge(map.entry(key.clone()).or_insert(Value::Null), value);
        }
    }
}

/// Every resource target the merged macOS release config names.
fn macos_release_targets() -> Vec<String> {
    let mut merged = conf("tauri.conf.json");
    merge(&mut merged, &conf("tauri.macos.conf.json"));
    merge(&mut merged, &conf("tauri.release.conf.json"));
    match &merged["bundle"]["resources"] {
        Value::Object(map) => map
            .values()
            .map(|v| v.as_str().expect("a target path").to_owned())
            .collect(),
        Value::Array(list) => list
            .iter()
            .map(|v| v.as_str().expect("a source path").to_owned())
            .collect(),
        other => panic!("bundle.resources: {other}"),
    }
}

/// The macOS release bundle carries the Computer Use helper beside zo and
/// the emulator helper's licences — none of the three files may drop another's.
#[test]
fn the_macos_release_bundle_carries_the_computer_use_helper_zo_and_the_licences() {
    let targets = macos_release_targets();
    for wanted in [
        "ZeroCode Computer Use.app",
        "bin/zo",
        "native/ios-emulator-helper/LICENSE-APACHE-2.0",
        "native/ios-emulator-helper/LICENSE-IDB-MIT",
        "native/ios-emulator-helper/NOTICE.md",
    ] {
        assert!(
            targets.iter().any(|t| t == wanted),
            "{wanted} missing from the merged macOS release resources: {targets:?}"
        );
    }
}

/// A platform or `--config` file that names `bundle.resources` must use the
/// map form: a list replaces the maps merged before it instead of joining them.
#[test]
fn every_overlay_names_resources_as_a_map() {
    for name in [
        "tauri.macos.conf.json",
        "tauri.release.conf.json",
        "tauri.windows.conf.json",
    ] {
        let resources = &conf(name)["bundle"]["resources"];
        assert!(
            resources.is_null() || resources.is_object(),
            "{name}: bundle.resources must be a map, got {resources}"
        );
    }
}
