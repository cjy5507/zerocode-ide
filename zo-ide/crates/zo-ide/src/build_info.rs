//! Build-time identity, including explicit unknowns for source archives.
use serde_json::{json, Value};

#[must_use]
pub fn current() -> Value {
    let known = |value: &'static str| (!value.is_empty()).then_some(value);
    json!({
        "version": env!("CARGO_PKG_VERSION"),
        "id": known(env!("ZO_BUILD_ID")),
        "git_sha": known(env!("ZO_BUILD_GIT_SHA")),
        "dirty": match env!("ZO_BUILD_DIRTY") { "true" => Some(true), "false" => Some(false), _ => None },
    })
}
