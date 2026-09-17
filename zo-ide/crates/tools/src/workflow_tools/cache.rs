//! Workflow resume cache (roadmap step 7).
//!
//! A workflow is keyed by a **stable** hash of its (spec, input) pair, so the
//! same workflow re-run resumes from `.zo/workflows/<run_id>.json` instead of
//! re-spawning the phases it already finished. Two design points matter:
//!
//! * **Stable run id.** `std::hash::DefaultHasher` is explicitly *not* stable
//!   across processes/builds, which would make it a broken cache key. We use a
//!   tiny in-crate FNV-1a over the pair's *canonical* JSON. `serde_json::Map`
//!   is a `BTreeMap` (no `preserve_order` feature in this workspace), so
//!   `to_string` emits sorted keys — the same spec hashes identically no matter
//!   how the model ordered its keys (guarded by a test).
//! * **Forward/backward-compatible cache format.** Unlike the strict spec
//!   ([`super::spec`]), the cache is *our own* output read back later: it must
//!   tolerate format evolution. So [`CachedRun`] is **not**
//!   `deny_unknown_fields` (a newer field written by a newer zo is ignored by
//!   an older one) and every field is `#[serde(default)]` (an older cache
//!   missing a field still loads). A corrupt/unparseable cache is ignored, never
//!   a panic — the workflow simply re-runs.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::engine::{PassReceipt, PhaseReport, SemanticCache, SemanticCacheKey, WorkflowCache};

/// Env override for the cache directory. Mirrors `agent_store_dir`'s
/// `ZO_AGENT_STORE`: the seam a `-p` sandbox (or a test) uses to redirect
/// cache writes out of the repository.
const WORKFLOW_STORE_ENV: &str = "ZO_WORKFLOW_STORE";

/// The on-disk cache document: the run id it belongs to plus the completed
/// phase prefix. `run_id` is re-checked on load so a stale file that somehow
/// shares a path can never be mistaken for this run's cache.
#[derive(Debug, Default, Serialize, Deserialize)]
struct CachedRun {
    #[serde(default)]
    run_id: String,
    #[serde(default)]
    phases: Vec<PhaseReport>,
}

/// File-backed [`WorkflowCache`]. Owns the `run_id` scoping so the engine only
/// ever deals in [`PhaseReport`]s.
pub(crate) struct FileCache {
    run_id: String,
    path: PathBuf,
}

impl FileCache {
    /// Resolve the cache file for `run_id`, or `None` when no store directory is
    /// available (e.g. the cwd cannot be determined) — caching then silently
    /// disables rather than failing the run.
    pub(crate) fn resolve(run_id: String) -> Option<Self> {
        let dir = workflow_store_dir()?;
        let path = dir.join(format!("{run_id}.json"));
        Some(Self { run_id, path })
    }
}

impl WorkflowCache for FileCache {
    fn load(&self) -> Option<Vec<PhaseReport>> {
        let text = std::fs::read_to_string(&self.path).ok()?;
        // A corrupt or older-incompatible cache parses to `Err` → ignored.
        let cached: CachedRun = serde_json::from_str(&text).ok()?;
        (cached.run_id == self.run_id).then_some(cached.phases)
    }

    fn store(&self, phases: &[PhaseReport]) {
        let cached = CachedRun {
            run_id: self.run_id.clone(),
            phases: phases.to_vec(),
        };
        let Ok(text) = serde_json::to_string(&cached) else {
            return;
        };
        // Best-effort: a cache write that fails must never break the workflow.
        let _ = write_atomic(&self.path, &text);
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SemanticPassCacheDoc {
    #[serde(default)]
    passes: BTreeMap<String, PassReceipt>,
}

/// File-backed item-level verifier cache. Shared across workflow runs in the
/// project workflow store; keys include phase id, item text, schema
/// fingerprint, and rendered verifier semantics so edited verifiers do not reuse
/// mixed-mode receipts.
///
/// Cross-run reuse is only sound when the cache key also pins the source state a
/// pass was proven against — otherwise a later run could replay a stale "pass"
/// for code that changed underneath it (the verifier prompt/schema/command can
/// be byte-identical while the working tree moved). `scope` carries that pin: a
/// per-run fingerprint of the parent model plus the git source state (HEAD +
/// tracked working-tree diff), prepended to every stored/looked-up key. Any
/// source or model change yields a different scope, so passes never cross that
/// boundary. When the source state cannot be fingerprinted (not a git work tree,
/// or git is unavailable), [`resolve`](Self::resolve) returns `None` and
/// cross-run reuse is disabled rather than risk a stale carry.
pub(crate) struct FileSemanticCache {
    path: PathBuf,
    scope: String,
}

impl FileSemanticCache {
    pub(crate) fn resolve(parent_model: Option<&str>) -> Option<Self> {
        let path = workflow_store_dir()?.join("semantic-passes.json");
        // No robust source fingerprint → do not reuse across runs.
        let source = workspace_source_fingerprint()?;
        Some(Self {
            path,
            scope: format!("model={}|src={source}", parent_model.unwrap_or("")),
        })
    }

    /// Scope-qualified storage key: the per-run source/model `scope` plus the
    /// engine's verifier key, so a pass is only ever replayed against the exact
    /// source + model it was proven on.
    fn scoped_key(&self, key: &SemanticCacheKey) -> String {
        format!("{}|{}", self.scope, key.stable_key())
    }

    fn load_doc(&self) -> SemanticPassCacheDoc {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }
}

impl SemanticCache for FileSemanticCache {
    fn load_pass(&self, key: &SemanticCacheKey) -> Option<PassReceipt> {
        self.load_doc().passes.get(&self.scoped_key(key)).cloned()
    }

    fn store_pass(&self, key: &SemanticCacheKey, receipt: &PassReceipt) {
        let mut doc = self.load_doc();
        doc.passes.insert(self.scoped_key(key), receipt.clone());
        if let Ok(text) = serde_json::to_string(&doc) {
            let _ = write_atomic(&self.path, &text);
        }
    }
}

/// Fingerprint of the current git source state for the cross-run semantic cache
/// scope: `HEAD` commit plus an FNV hash of the tracked working-tree diff
/// (`git diff HEAD`, covering staged and unstaged edits to tracked files).
/// `None` when this is not a git work tree or git is unavailable — the caller
/// then disables cross-run reuse rather than carry a pass it cannot pin to a
/// source state. Untracked files are intentionally out of scope: a semantic
/// `pass` is still `command_green`-gated, and over-invalidation is the safe
/// direction here.
fn workspace_source_fingerprint() -> Option<String> {
    let head = Command::new("git").args(["rev-parse", "HEAD"]).output().ok()?;
    if !head.status.success() {
        return None;
    }
    let head = String::from_utf8_lossy(&head.stdout).trim().to_string();
    let diff = Command::new("git").args(["diff", "HEAD"]).output().ok()?;
    if !diff.status.success() {
        return None;
    }
    Some(format!("{head}:{:016x}", fnv1a_64(&diff.stdout)))
}

/// Directory for workflow resume caches. Honors the [`WORKFLOW_STORE_ENV`]
/// override, else Zo's global per-project state directory so workflow resume
/// cache writes do not dirty the workspace. Shared with [`super::progress`] so
/// the live-progress snapshot lands in the same store as the resume cache.
pub(super) fn workflow_store_dir() -> Option<PathBuf> {
    if let Ok(path) = std::env::var(WORKFLOW_STORE_ENV) {
        return Some(PathBuf::from(path));
    }
    let cwd = std::env::current_dir().ok()?;
    Some(runtime::zo_project_state_dir(&cwd).join("workflows"))
}

/// Write `contents` to `path` atomically (temp file + rename) so a crash
/// mid-write can never leave a half-written, unparseable cache behind. Shared
/// with [`super::progress`] for the same crash-safety on the progress snapshot.
pub(super) fn write_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    // BUG-R7: a fixed `<name>.json.tmp` lets two processes writing the same
    // (spec,input) cache rename over each other's temp file. A pid + per-process
    // nonce makes the temp path unique so concurrent writers cannot race; the
    // final atomic rename onto `path` still gives last-writer-wins on the target.
    static NONCE: AtomicU64 = AtomicU64::new(0);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let nonce = NONCE.fetch_add(1, Ordering::Relaxed);
    let tmp = path.with_extension(format!("json.tmp.{}.{nonce}", std::process::id()));
    std::fs::write(&tmp, contents)?;
    // On failure, clean up the temp file rather than leaking it.
    if let Err(error) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(error);
    }
    Ok(())
}

/// Append one newline-terminated `line` to `path`, creating it and any parent
/// directories if needed. Best-effort companion to [`write_atomic`] for the
/// Phase-3 append-only event log ([`super::progress::EventLogSink`]): callers
/// swallow the error because the event log is advisory, never load-bearing.
pub(super) fn append_line(path: &Path, line: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{line}")
}

/// Stable run id for a (spec, input) pair: FNV-1a over their canonical JSON.
/// Stable across processes and builds, unlike `DefaultHasher`.
pub(crate) fn compute_run_id(spec: &Value, input: &Value) -> String {
    let canonical = format!("{}\u{0}{}", canonical_json(spec), canonical_json(input));
    format!("{:016x}", fnv1a_64(canonical.as_bytes()))
}

/// Serialize with object keys sorted recursively. `serde_json::Map` keeps
/// insertion order once any workspace member enables `preserve_order`
/// (feature unification — the ACP protocol crate does), so a plain
/// `to_string` is NOT canonical: two semantically identical specs could
/// hash to different run ids. Sorting here keeps the id order-independent
/// regardless of that feature.
fn canonical_json(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            entries.sort_by_key(|(key, _)| *key);
            let body = entries
                .iter()
                .map(|(key, value)| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(key).unwrap_or_default(),
                        canonical_json(value)
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            format!("{{{body}}}")
        }
        Value::Array(items) => {
            let body = items
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",");
            format!("[{body}]")
        }
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

/// FNV-1a 64-bit. Deterministic for the same bytes on every platform/build.
fn fnv1a_64(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET_BASIS;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::engine::Risk;
    use serde_json::json;
    use std::sync::MutexGuard;

    /// Crate-wide env lock: this module mutates `ZO_STATE_DIR`, which other
    /// test modules also read/write — a module-local mutex cannot exclude them.
    fn env_lock() -> MutexGuard<'static, ()> {
        crate::tests::env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn phase(id: &str, result: &str) -> PhaseReport {
        PhaseReport {
            id: id.to_string(),
            rounds: 1,
            output_tokens: 0,
            relay_bytes: 0,
            carried_pass_count: 0,
            retried_finding_count: 0,
            skipped_count: 0,
            blocked_finding_count: 0,
            escalated_finding_count: 0,
            findings: Vec::new(),
            pass_receipts: Vec::new(),
            items: vec![crate::workflow_tools::engine::ItemResult {
                index: 0,
                input: "in".to_string(),
                agent_id: "a0".to_string(),
                status: "completed".to_string(),
                result: Some(result.to_string()),
                error: None,
                structured: None,
                output_tokens: 0,
                loaded_skills: Vec::new(),
                semantic_verdict: None,
                retry_key: None,
                carry_reason: None,
                carried: false,
            }],
        }
    }

    /// A fresh, unique cache directory under the OS temp dir. Env-free: the
    /// `FileCache` is built directly so these tests never touch (and so never
    /// race on) the process-global `ZO_WORKFLOW_STORE`.
    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zo-wf-cache-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create test dir");
        dir
    }

    fn file_cache_at(dir: &Path, run_id: &str) -> FileCache {
        FileCache {
            run_id: run_id.to_string(),
            path: dir.join(format!("{run_id}.json")),
        }
    }

    #[test]
    fn run_id_is_stable_and_order_independent() {
        // Same spec, keys in a different order → identical run id (canonical JSON).
        let a = compute_run_id(&json!({ "name": "x", "mode": "phases" }), &json!("input"));
        let b = compute_run_id(&json!({ "mode": "phases", "name": "x" }), &json!("input"));
        assert_eq!(a, b, "key order must not change the run id");
        assert_eq!(a.len(), 16, "16 hex chars from a u64");
    }

    #[test]
    fn run_id_changes_with_spec_or_input() {
        let base = compute_run_id(&json!({ "name": "x" }), &json!("in"));
        assert_ne!(base, compute_run_id(&json!({ "name": "y" }), &json!("in")));
        assert_ne!(base, compute_run_id(&json!({ "name": "x" }), &json!("in2")));
    }

    #[test]
    fn file_cache_round_trips_phases() {
        let dir = temp_dir("roundtrip");
        let cache = file_cache_at(&dir, "run-roundtrip");
        assert!(cache.load().is_none(), "empty before any store");

        cache.store(&[phase("p0", "r0")]);
        let loaded = cache.load().expect("load after store");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "p0");
        assert_eq!(loaded[0].items[0].result.as_deref(), Some("r0"));

        // A second store overwrites with the longer prefix.
        cache.store(&[phase("p0", "r0"), phase("p1", "r1")]);
        assert_eq!(cache.load().expect("load").len(), 2);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_cache_rejects_mismatched_run_id_body() {
        let dir = temp_dir("wrong-id");
        let path = dir.join("shared.json");
        // Writer stamps the body with run_id "A".
        let writer = FileCache {
            run_id: "A".to_string(),
            path: path.clone(),
        };
        writer.store(&[phase("p0", "r0")]);
        assert!(writer.load().is_some(), "matching run_id loads");
        // A reader at the *same path* but run_id "B" must reject the body.
        let reader = FileCache {
            run_id: "B".to_string(),
            path,
        };
        assert!(reader.load().is_none(), "run_id body mismatch → miss");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_cache_corrupt_json_is_a_graceful_miss() {
        let dir = temp_dir("corrupt");
        let cache = file_cache_at(&dir, "corrupt-run");
        std::fs::write(dir.join("corrupt-run.json"), b"{ not valid json").expect("write garbage");
        assert!(cache.load().is_none(), "corrupt cache → miss, no panic");

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn semantic_key(label: &str) -> SemanticCacheKey {
        SemanticCacheKey {
            phase_id: "verify".to_string(),
            item: "parser".to_string(),
            schema_fingerprint: "schema-v1".to_string(),
            verifier_fingerprint: label.to_string(),
        }
    }

    fn pass_receipt(label: &str) -> PassReceipt {
        PassReceipt {
            item_index: 0,
            receipt_key: format!("receipt-{label}"),
            coverage: format!("coverage-{label}"),
            risk: Risk::Local,
        }
    }

    #[test]
    fn file_semantic_cache_round_trips_with_string_keys() {
        let dir = temp_dir("semantic-roundtrip");
        let cache = FileSemanticCache {
            path: dir.join("semantic-passes.json"),
            scope: "model=test|src=tree-a".to_string(),
        };
        let key = semantic_key("prompt-v1");
        let receipt = pass_receipt("v1");

        assert!(cache.load_pass(&key).is_none(), "empty before store");
        cache.store_pass(&key, &receipt);

        let raw = std::fs::read_to_string(dir.join("semantic-passes.json")).expect("cache written");
        let json: serde_json::Value = serde_json::from_str(&raw).expect("valid json object");
        let passes = json
            .get("passes")
            .and_then(serde_json::Value::as_object)
            .expect("passes object with string keys");
        assert!(
            passes.keys().any(|stored_key| stored_key.contains("|v1:")),
            "semantic cache keys must be stable strings, not struct map keys: {raw}"
        );

        let loaded = cache.load_pass(&key).expect("load after store");
        assert_eq!(loaded.receipt_key, receipt.receipt_key);
        assert_eq!(loaded.coverage, receipt.coverage);
        assert!(
            cache.load_pass(&semantic_key("prompt-v2")).is_none(),
            "verifier fingerprint changes must miss"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_semantic_cache_scope_pins_source_and_model() {
        // A pass proven under one source/model scope must NOT be replayed under a
        // different scope, even when the engine verifier key is byte-identical.
        // This is the cross-run correctness guard: a code (or model) change moves
        // the scope, so a stale "pass" can never carry across that boundary.
        let dir = temp_dir("semantic-scope");
        let path = dir.join("semantic-passes.json");
        let key = semantic_key("prompt-v1");
        let receipt = pass_receipt("v1");

        let tree_a = FileSemanticCache {
            path: path.clone(),
            scope: "model=opus|src=HEAD-a:0000000000000001".to_string(),
        };
        tree_a.store_pass(&key, &receipt);
        assert!(
            tree_a.load_pass(&key).is_some(),
            "same scope + same key must hit"
        );

        let tree_b = FileSemanticCache {
            path: path.clone(),
            scope: "model=opus|src=HEAD-b:0000000000000002".to_string(),
        };
        assert!(
            tree_b.load_pass(&key).is_none(),
            "changed source fingerprint must miss the identical verifier key"
        );

        let other_model = FileSemanticCache {
            path,
            scope: "model=sonnet|src=HEAD-a:0000000000000001".to_string(),
        };
        assert!(
            other_model.load_pass(&key).is_none(),
            "changed model must miss the identical verifier key"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn store_dir_honors_env_override() {
        let _lock = env_lock();
        let prior_workflow = std::env::var_os(WORKFLOW_STORE_ENV);
        let prior_state = std::env::var_os("ZO_STATE_DIR");
        let dir = temp_dir("env-override");
        std::env::set_var(WORKFLOW_STORE_ENV, &dir);
        std::env::set_var("ZO_STATE_DIR", dir.join("state-root"));
        let resolved = workflow_store_dir().expect("dir resolves");
        match prior_workflow {
            Some(value) => std::env::set_var(WORKFLOW_STORE_ENV, value),
            None => std::env::remove_var(WORKFLOW_STORE_ENV),
        }
        match prior_state {
            Some(value) => std::env::set_var("ZO_STATE_DIR", value),
            None => std::env::remove_var("ZO_STATE_DIR"),
        }
        assert_eq!(resolved, dir);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn zo_state_dir_partitions_workflow_store_by_workspace() {
        let _lock = env_lock();
        let prior_workflow = std::env::var_os(WORKFLOW_STORE_ENV);
        let prior_state = std::env::var_os("ZO_STATE_DIR");
        let previous_cwd = std::env::current_dir().expect("cwd");
        let root = temp_dir("state-partition");
        let state_root = root.join("state-root");
        let first_workspace = root.join("workspace-a");
        let second_workspace = root.join("workspace-b");
        std::fs::create_dir_all(&first_workspace).expect("first workspace");
        std::fs::create_dir_all(&second_workspace).expect("second workspace");
        std::env::remove_var(WORKFLOW_STORE_ENV);
        std::env::set_var("ZO_STATE_DIR", &state_root);

        std::env::set_current_dir(&first_workspace).expect("cwd first");
        let first = workflow_store_dir().expect("first workflow store");
        std::env::set_current_dir(&second_workspace).expect("cwd second");
        let second = workflow_store_dir().expect("second workflow store");
        std::env::set_current_dir(previous_cwd).expect("restore cwd");
        match prior_workflow {
            Some(value) => std::env::set_var(WORKFLOW_STORE_ENV, value),
            None => std::env::remove_var(WORKFLOW_STORE_ENV),
        }
        match prior_state {
            Some(value) => std::env::set_var("ZO_STATE_DIR", value),
            None => std::env::remove_var("ZO_STATE_DIR"),
        }

        assert!(first.starts_with(&state_root));
        assert!(second.starts_with(&state_root));
        assert_ne!(
            first, second,
            "ZO_STATE_DIR must not collapse workspaces"
        );
        assert!(first.ends_with("workflows"));

        let _ = std::fs::remove_dir_all(&root);
    }
}
