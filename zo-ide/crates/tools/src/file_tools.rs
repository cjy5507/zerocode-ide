use std::path::Path;

use serde::Deserialize;
use serde_json::{json, Value};

use super::{
    from_value, maybe_enforce_permission_check, to_pretty_json, ToolContext, ToolError, ToolSpec,
};
use crate::context::{DebugHypothesis, HypothesisStatus, Probe};
use runtime::{
    edit_file,
    file_ops::{multi_edit_file, validate_workspace_boundary},
    glob_search, grep_search,
    permission_enforcer::PermissionEnforcer, read_file, write_file, FileFreshness,
    FileReadRegistry, GrepSearchInput, PermissionMode,
};

#[derive(Debug, Deserialize)]
pub(crate) struct ReadFileInput {
    pub path: String,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ReadImageInput {
    pub path: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WriteFileInput {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct EditFileInput {
    pub path: String,
    pub old_string: String,
    pub new_string: String,
    pub replace_all: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct MultiEditInput {
    pub path: String,
    pub edits: Vec<MultiEditReplacement>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct MultiEditReplacement {
    pub old_string: String,
    pub new_string: String,
    pub replace_all: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct GlobSearchInputValue {
    /// Optional with a `**/*` default: models routinely call glob with only a
    /// `path`, meaning "list what's here" — a hard missing-field error on that
    /// was the single most common tool-call failure in session logs.
    pub pattern: Option<String>,
    pub path: Option<String>,
}

impl GlobSearchInputValue {
    /// Every file under the base path when the model omitted `pattern`.
    pub(crate) fn pattern_or_default(&self) -> &str {
        self.pattern.as_deref().unwrap_or("**/*")
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct InstrumentLogInput {
    pub path: String,
    /// Unique existing text to attach the probe after (the insertion anchor).
    pub anchor: String,
    /// The probe statement to inject (e.g. a log/print line). Inserted on its
    /// own line right after `anchor`, prefixed with a removable marker.
    pub statement: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DebugHypothesisInput {
    /// The root-cause hypothesis being tracked. Restated on each update.
    pub hypothesis: String,
    /// Current verdict: open (untested), confirmed, or refuted.
    pub status: HypothesisStatus,
    /// Evidence behind the status (test output, probe result, reasoning).
    #[serde(default)]
    pub evidence: Option<String>,
    /// Stable id to UPDATE an existing hypothesis; omit to record a new one
    /// (an `h<n>` id is assigned and returned).
    #[serde(default)]
    pub id: Option<String>,
}

#[allow(clippy::too_many_lines)] // a flat spec table, clearer unsplit
pub(crate) fn tool_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "read_file",
            description: "Read workspace text, PDF pages, or notebook cells and outputs. Read one generous range and batch independent files. Do not reread after a successful edit unless disk may have changed. A whole code-file read ends with [neighbours].",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "offset": { "type": "integer", "minimum": 0 },
                    "limit": { "type": "integer", "minimum": 1 }
                },
                "required": ["path"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::ReadOnly,
        },
        ToolSpec {
            name: "read_image",
            description: "Read an image file (PNG/JPEG/GIF/WEBP) so you can see it — e.g. to visually verify a screenshot or a generated image.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" }
                },
                "required": ["path"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::ReadOnly,
        },
        ToolSpec {
            name: "write_file",
            description: "Create or replace a workspace text file; read existing files first. Prefer edit_file for partial changes. Create only required files.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "content": { "type": "string" }
                },
                "required": ["path", "content"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::WorkspaceWrite,
        },
        ToolSpec {
            name: "edit_file",
            description: "Replace exact, previously read workspace text. old_string must be unique unless replace_all. Success means applied; do not reread only to verify.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "old_string": { "type": "string" },
                    "new_string": { "type": "string" },
                    "replace_all": { "type": "boolean" }
                },
                "required": ["path", "old_string", "new_string"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::WorkspaceWrite,
        },
        ToolSpec {
            name: "MultiEdit",
            description: "Atomically apply ordered replacements to one previously read file. Each old_string must be unique unless replace_all; any failure writes nothing and names its index.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "edits": {
                        "type": "array",
                        "minItems": 1,
                        "items": {
                            "type": "object",
                            "properties": {
                                "old_string": { "type": "string" },
                                "new_string": { "type": "string" },
                                "replace_all": { "type": "boolean" }
                            },
                            "required": ["old_string", "new_string"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["path", "edits"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::WorkspaceWrite,
        },
        ToolSpec {
            name: "InstrumentLog",
            description: "Debug mode: insert a TEMPORARY, auto-reverted probe (a log/print statement tagged with a `/*ZO_PROBE*/` marker) on its own line right after a unique anchor line. Every probe is removed automatically when the debugging run ends, so instrumentation never leaks into the final diff. Prefer this over `edit_file` for throwaway tracing while diagnosing a bug.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "anchor": {
                        "type": "string",
                        "description": "Unique existing line/text to insert the probe right after."
                    },
                    "statement": {
                        "type": "string",
                        "description": "The probe statement to inject (use a comment style valid for the file's language)."
                    }
                },
                "required": ["path", "anchor", "statement"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::WorkspaceWrite,
        },
        ToolSpec {
            name: "DebugHypothesis",
            description: "Debug mode: record or update a root-cause hypothesis while diagnosing a bug. Track each guess with a status (open → confirmed/refuted) and the evidence behind it; the tool returns the FULL hypothesis ledger every call, so your reasoning persists across iterations and you never re-test a theory you already refuted. Pass `id` to update an existing hypothesis, or omit it to add a new one.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "hypothesis": { "type": "string", "description": "The root-cause hypothesis being tracked." },
                    "status": { "type": "string", "enum": ["open", "confirmed", "refuted"] },
                    "evidence": { "type": "string", "description": "Evidence behind the status (test output, probe result, reasoning)." },
                    "id": { "type": "string", "description": "Id of an existing hypothesis to update; omit to record a new one." }
                },
                "required": ["hypothesis", "status"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::ReadOnly,
        },
        ToolSpec {
            name: "glob_search",
            description: "Find files by glob; pattern defaults to `**/*`.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string" },
                    "path": { "type": "string" }
                },
                "additionalProperties": false
            }),
            required_permission: PermissionMode::ReadOnly,
        },
        ToolSpec {
            name: "grep_search",
            description: "Search workspace files by regex. Batch independent searches.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string" },
                    "path": { "type": "string" },
                    "glob": { "type": "string" },
                    "output_mode": { "type": "string" },
                    "-B": { "type": "integer", "minimum": 0 },
                    "-A": { "type": "integer", "minimum": 0 },
                    "-C": { "type": "integer", "minimum": 0 },
                    "context": { "type": "integer", "minimum": 0 },
                    "-n": { "type": "boolean" },
                    "-i": { "type": "boolean" },
                    "type": { "type": "string" },
                    "head_limit": { "type": "integer", "minimum": 1 },
                    "offset": { "type": "integer", "minimum": 0 },
                    "multiline": { "type": "boolean" }
                },
                "required": ["pattern"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::ReadOnly,
        },
    ]
}

pub(crate) fn dispatch(
    ctx: &ToolContext,
    enforcer: Option<&PermissionEnforcer>,
    name: &str,
    input: &Value,
) -> Option<Result<String, ToolError>> {
    match name {
        "read_file" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<ReadFileInput>(input).and_then(|inp| {
                    run_read_file(
                        &inp,
                        enforcer,
                        ctx.session_permission_mode(),
                        ctx.workspace_root.as_deref(),
                        ctx.cwd.as_deref(),
                        &ctx.file_reads,
                    )
                })
            }),
        ),
        "read_image" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<ReadImageInput>(input).and_then(|inp| run_read_image(&inp, ctx))
            }),
        ),
        "write_file" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<WriteFileInput>(input).and_then(|inp| {
                    enforce_write_lease(ctx, &inp.path)?;
                    run_write_file(
                        &inp,
                        enforcer,
                        ctx,
                    )
                })
            }),
        ),
        "edit_file" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<EditFileInput>(input).and_then(|inp| {
                    enforce_write_lease(ctx, &inp.path)?;
                    run_edit_file(
                        &inp,
                        enforcer,
                        ctx,
                    )
                })
            }),
        ),
        "MultiEdit" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<MultiEditInput>(input).and_then(|inp| {
                    enforce_write_lease(ctx, &inp.path)?;
                    run_multi_edit(&inp, enforcer, ctx)
                })
            }),
        ),
        "InstrumentLog" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<InstrumentLogInput>(input)
                    .and_then(|inp| run_instrument_log(&inp, enforcer, ctx))
            }),
        ),
        "DebugHypothesis" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<DebugHypothesisInput>(input).map(|inp| run_debug_hypothesis(inp, ctx))
            }),
        ),
        "glob_search" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<GlobSearchInputValue>(input).and_then(|input| {
                    run_glob_search(
                        &input,
                        enforcer,
                        ctx.session_permission_mode(),
                        ctx.workspace_root.as_deref(),
                        ctx.cwd.as_deref(),
                    )
                })
            }),
        ),
        "grep_search" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                // Content search has no sane default pattern, and a bare
                // serde "missing field" gave the model nothing to correct
                // with — it kept re-issuing path-only calls. Say what the
                // field means and name the tool that does what it wanted.
                if input.get("pattern").is_none() {
                    return Err(ToolError::InvalidInput(
                        "missing field `pattern` — the regex to search file contents for, \
                         e.g. {\"pattern\": \"fn main\", \"path\": \"src/\"}. To list files \
                         under a directory instead, use glob_search (its pattern defaults \
                         to `**/*`)."
                            .to_string(),
                    ));
                }
                from_value::<GrepSearchInput>(input).and_then(|input| {
                    run_grep_search(
                        &input,
                        enforcer,
                        ctx.session_permission_mode(),
                        ctx.workspace_root.as_deref(),
                        ctx.cwd.as_deref(),
                    )
                })
            }),
        ),
        _ => None,
    }
}

/// Acquire the cross-process write lease for a `write_file`/`edit_file`/`MultiEdit` target
/// before the write runs (track 4-2). A no-op unless the context carries a
/// `lease_owner` (a spawned agent / identified session) **and** the workspace
/// guard is opt-in enabled — so solo, single-process editing is never gated.
///
/// On a live conflict the write is refused with the holder's identity so the
/// agent can coordinate instead of silently clobbering a sibling's in-flight
/// edit. The lease is keyed on the same absolute path the write resolves to.
fn enforce_write_lease(ctx: &ToolContext, path: &str) -> Result<(), ToolError> {
    let Some(owner) = ctx.lease_owner.as_deref() else {
        return Ok(());
    };
    if !crate::workspace_guard_enabled() {
        return Ok(());
    }
    // Resolve the same target the write will touch: an absolute path as-is,
    // otherwise against the per-agent cwd (falling back to the literal path when
    // no cwd is pinned, matching the write path's own resolution).
    let resolved = if Path::new(path).is_absolute() {
        std::path::PathBuf::from(path)
    } else {
        resolve_against_cwd(path, ctx.cwd.as_deref())
            .unwrap_or_else(|| std::path::PathBuf::from(path))
    };
    // The lease registry partitions by workspace slug, so any directory inside
    // the tree resolves to the same lease store; use the pinned cwd when present
    // and the process cwd otherwise.
    let cwd = ctx
        .cwd
        .clone()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    match crate::acquire_write_lease(&resolved, owner, &cwd) {
        crate::LeaseOutcome::Acquired => Ok(()),
        crate::LeaseOutcome::Conflict(holder) => Err(ToolError::PermissionDenied {
            tool: "write_file/edit_file/MultiEdit".to_string(),
            reason: format!(
                "file `{}` is being edited by another agent (owner `{}`, pid {}); \
                 coordinate or wait for it to finish instead of overwriting its in-flight changes \
                 (lease guard is opt-in via ZO_WORKSPACE_GUARD).",
                holder.path, holder.owner, holder.pid
            ),
        }),
    }
}

/// Whether a permission mode authorizes unrestricted file access, so the
/// workspace boundary is relaxed. `DangerFullAccess` and `Allow` both render as
/// "full-access" in the HUD and both already permit any write at the policy
/// layer (`PermissionEnforcer::check_file_write`), so reads/writes outside the
/// workspace are intended too.
fn mode_grants_full_access(mode: PermissionMode) -> bool {
    matches!(
        mode,
        PermissionMode::DangerFullAccess | PermissionMode::Allow
    )
}

/// Resolve whether the active session is full-access, from *either* an explicit
/// registry [`PermissionEnforcer`] (sub-agents, tests) *or* the foreground
/// session mode carried on the [`ToolContext`] (`session_mode`).
///
/// The foreground `tool_registry` carries no enforcer — gating happens at the
/// runtime layer — so without the `session_mode` fallback a danger-full-access
/// user is wrongly denied an outside `read_file`/`write_file`/`edit_file` with
/// "escapes workspace boundary", even though `bash cat` / `read_image` reach the
/// same path. See `ToolContext::session_permission_mode`.
fn boundary_is_full_access(
    enforcer: Option<&PermissionEnforcer>,
    session_mode: Option<PermissionMode>,
) -> bool {
    enforcer
        .map(PermissionEnforcer::active_mode)
        .is_some_and(mode_grants_full_access)
        || session_mode.is_some_and(mode_grants_full_access)
}

/// Reject paths that escape the workspace root.
///
/// Layered on top of `maybe_enforce_permission_check` (which already
/// gated the high-level mode): this resolves the *actual* on-disk path
/// so `../` traversal and platform symlink aliases (e.g. `/tmp` →
/// `/private/tmp` on macOS) are normalised before comparing against
/// the canonical workspace root. Skipped when no workspace root is
/// configured (tests, harness runs).
pub(crate) fn enforce_workspace_boundary(
    enforcer: Option<&PermissionEnforcer>,
    session_mode: Option<PermissionMode>,
    path: &str,
    workspace_root: Option<&Path>,
) -> Result<Option<std::path::PathBuf>, ToolError> {
    let Some(root) = workspace_root else {
        return Ok(None);
    };
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let candidate = if Path::new(path).is_absolute() {
        std::path::PathBuf::from(path)
    } else {
        canonical_root.join(path)
    };
    let resolved = resolve_for_boundary_check(&candidate);
    // Danger-full-access means the user explicitly authorized unrestricted
    // writes — the enforcer already permits every write in this mode — so the
    // workspace boundary is relaxed: the agent may write to sibling/outside
    // directories too (e.g. a new `../zo-ide` project). Otherwise a path
    // that "escapes" the workspace is rejected even though the granted mode would
    // allow it, the surprising denial a full-access user hits writing next door
    // (CC parity with `--dangerously-skip-permissions`). Read-only and
    // workspace-write keep the boundary so `../` traversal stays confined there.
    //
    // `Allow` relaxes it too: the policy layer already treats Allow as
    // permit-all for writes (`PermissionEnforcer::check_file_write`), the HUD
    // labels both modes "full-access", and `bash` in Allow mode can already
    // write the same outside paths — keeping only `write_file` confined was an
    // inconsistent denial, not protection (the `bench_*.sql` incident: bash
    // redirect succeeded, write_file "escapes workspace boundary" failed).
    //
    // The mode is read from the registry enforcer *or* the foreground session
    // mode on the context: the foreground `tool_registry` carries no enforcer
    // (gating is done at the runtime layer), so without the session-mode
    // fallback a full-access user is wrongly denied an outside write.
    if !boundary_is_full_access(enforcer, session_mode) {
        validate_workspace_boundary(&resolved, &canonical_root).map_err(|e| {
            ToolError::PermissionDenied {
                tool: "write_file".to_owned(),
                reason: e.to_string(),
            }
        })?;
    }
    // Hand the *validated* absolute path back so the caller writes to exactly
    // what was boundary-checked. Otherwise the IO layer
    // (`write_file`/`edit_file`) re-resolves the relative path against the
    // live process cwd, which diverges from `workspace_root` once
    // `EnterWorktree` calls `set_current_dir` — a write could pass the check
    // against the root yet land in a different directory.
    Ok(Some(resolved))
}

/// Resolve `path` to the exact location `fs::write` will touch, so the workspace
/// boundary check sees the true destination.
///
/// `fs::write` follows a symlink at the leaf to its target and resolves every
/// symlink and `..` in the parent directories through the OS. So we (1) follow the
/// whole leaf-symlink chain (bounded against cycles), then (2) `canonicalize` the
/// deepest existing ancestor — which resolves intermediate symlinked directories
/// and `..` segments soundly (lexical `..` collapsing is UNSOUND across a symlink:
/// `ws/d/..` is `/` when `ws/d -> /outside`, not `ws`) — and re-append the missing
/// tail (the file being created). This closes the dangling-leaf, whole-chain,
/// symlinked-directory-in-target, and `..`-through-a-symlink workspace escapes.
pub(crate) fn resolve_for_boundary_check(path: &Path) -> std::path::PathBuf {
    // Matches the Linux `MAXSYMLINKS` chain bound; ample for any real link graph.
    const MAX_SYMLINK_HOPS: usize = 40;
    let mut current = path.to_path_buf();
    for _ in 0..MAX_SYMLINK_HOPS {
        match current.symlink_metadata() {
            // A symlink leaf: `fs::write` follows it, so follow it too. A relative
            // target is joined against the link's parent; the NEXT iteration
            // re-lstats the composed path, so an intermediate symlinked directory
            // in the target is resolved on the following pass rather than trusted
            // by its in-workspace lexical name.
            Ok(meta) if meta.file_type().is_symlink() => match std::fs::read_link(&current) {
                Ok(target) if target.is_absolute() => current = target,
                Ok(target) => {
                    let base = current.parent().unwrap_or_else(|| Path::new(""));
                    current = base.join(target);
                }
                Err(_) => break,
            },
            // A real file/dir, or a missing leaf `fs::write` would create at this
            // exact location — stop following.
            _ => break,
        }
    }
    // The existing ancestor is canonicalized (symlink-free); collapse any `.`/`..`
    // left in the not-yet-created tail against it. This is sound precisely because
    // the resolved path has no symlink for a `..` to cross — the unsound case
    // (`..` past a symlinked dir) was already resolved by `canonicalize` above.
    lexically_normalize(&resolve_existing_ancestor(&current))
}

/// Collapse `.`/`..` segments in an already symlink-resolved path without
/// touching the filesystem — used only to tidy the not-yet-created tail after
/// [`resolve_existing_ancestor`] has canonicalized the existing prefix. Applying
/// it to a path whose prefix is canonical is sound (no symlink for `..` to cross).
fn lexically_normalize(path: &Path) -> std::path::PathBuf {
    use std::path::{Component, PathBuf};
    let mut lexical = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                if !lexical.pop() {
                    lexical.push(Component::ParentDir);
                }
            }
            Component::CurDir => {}
            other => lexical.push(other.as_os_str()),
        }
    }
    lexical
}

/// Canonicalize the deepest existing ancestor of `path` (resolving symlinks and
/// `..` in the existing prefix the way the OS does), then re-append the missing
/// tail. `symlink_metadata` (lstat) marks the boundary between the existing prefix
/// and the not-yet-created tail so a dangling leaf is not mistaken for a plain
/// missing component.
fn resolve_existing_ancestor(path: &Path) -> std::path::PathBuf {
    let mut current = path.to_path_buf();
    let mut suffix: Vec<std::ffi::OsString> = Vec::new();
    while current.symlink_metadata().is_err() {
        match current.file_name() {
            Some(name) => {
                suffix.push(name.to_owned());
                if !current.pop() {
                    break;
                }
            }
            None => break,
        }
    }
    if current.as_os_str().is_empty() {
        return path.to_path_buf();
    }
    // `canonicalize` resolves intermediate symlinked dirs and `..` soundly; on an
    // unresolvable prefix (e.g. a symlink cycle → `ELOOP`) fall back to the lstat'd
    // prefix, which stays put rather than inventing an out-of-tree path.
    let mut resolved = current.canonicalize().unwrap_or(current);
    for name in suffix.into_iter().rev() {
        resolved.push(name);
    }
    resolved
}

/// Resolve a relative tool path against the per-agent working directory.
///
/// Returns `None` when there is nothing to rebase — an absolute path, or no
/// `cwd` set — so callers fall back to the historical process-cwd behavior.
/// Only consulted when no workspace root already resolved the path (e.g. a
/// harness run with `cwd` but no boundary), so it never overrides the
/// boundary-checked absolute path a configured workspace produces.
fn resolve_against_cwd(path: &str, cwd: Option<&Path>) -> Option<std::path::PathBuf> {
    let cwd = cwd?;
    let candidate = Path::new(path);
    (!candidate.is_absolute()).then(|| cwd.join(candidate))
}

pub(crate) fn run_read_file(
    input: &ReadFileInput,
    enforcer: Option<&PermissionEnforcer>,
    session_mode: Option<PermissionMode>,
    workspace_root: Option<&Path>,
    cwd: Option<&Path>,
    file_reads: &std::sync::Mutex<FileReadRegistry>,
) -> Result<String, ToolError> {
    // Read-only boundary check: symlink-escape and `../` traversal can
    // exfiltrate files outside the workspace even in ReadOnly mode.
    // DangerFullAccess relaxes it (see `enforce_read_boundary`).
    let resolved = enforce_read_boundary(enforcer, session_mode, &input.path, workspace_root)?;
    let target = resolved
        .or_else(|| resolve_against_cwd(&input.path, cwd))
        .map_or_else(|| input.path.clone(), |p| p.to_string_lossy().into_owned());
    let output = read_file(&target, input.offset, input.limit)?;
    // 성공한 읽기를 대화 스코프 레지스트리에 등재 — 이후 edit/write 가드의
    // 기준 상태. 부분 읽기(offset/limit)여도 파일 전체 스냅샷을 기록한다
    // (CC 패리티: 부분 Read도 "읽음"으로 친다).
    record_file_observation(file_reads, &output.file.file_path);
    to_pretty_json(output)
}

/// Read an image file and stage it (`media_type`, base64) in the context's image
/// sink so the conversation loop can attach it to this tool's result — the
/// model then *sees* the image. Returns a short text summary as the textual
/// tool output. Unlike `read_file`, this is intentionally *not* confined to the
/// workspace boundary — OCR/screenshot images commonly live outside it (e.g.
/// `/tmp`), so any readable path may be staged (still read-only, model-visible).
pub(crate) fn run_read_image(
    input: &ReadImageInput,
    ctx: &ToolContext,
) -> Result<String, ToolError> {
    use base64::Engine as _;
    use runtime::image_guard::{guard_image_bytes, ImageGuardOutcome};

    // read_image is read-only multimodal staging and, unlike `read_file`, is
    // deliberately *not* confined to the workspace boundary: OCR/screenshot
    // images routinely live in `/tmp` and other scratch dirs, so any readable
    // path must be stageable for the model to see.
    let target = resolve_against_cwd(&input.path, ctx.cwd.as_deref())
        .map_or_else(|| input.path.clone(), |p| p.to_string_lossy().into_owned());
    let bytes = std::fs::read(&target)
        .map_err(|error| ToolError::Execution(format!("read_image: {target}: {error}")))?;
    let media_type = sniff_image_mime(&bytes).ok_or_else(|| {
        ToolError::InvalidInput(
            "read_image: unsupported image format (expected PNG, JPEG, GIF, or WEBP)".to_owned(),
        )
    })?;

    // Dimension-guard on ingest so an oversized image (e.g. a full-page browser
    // screenshot taller than 8000px) never enters conversation history: baked
    // into a stored tool_result it would 400 *every* subsequent turn and wedge
    // the session (Anthropic rejects any dimension > 8000px). The wire-lowering
    // guard in `convert_messages` is the backstop for images already stored and
    // for the paste / MCP staging paths that bypass this tool.
    let (staged_media_type, encoded, staged_bytes, downscaled) = match guard_image_bytes(&bytes) {
        ImageGuardOutcome::Keep => {
            let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
            (media_type.to_owned(), encoded, bytes.len(), false)
        }
        ImageGuardOutcome::Rescaled {
            media_type,
            bytes: rescaled,
        } => {
            let encoded = base64::engine::general_purpose::STANDARD.encode(&rescaled);
            (media_type, encoded, rescaled.len(), true)
        }
        ImageGuardOutcome::DropOversized { width, height } => {
            // Either cap can land here — dimensions or encoded size — so the
            // message names both rather than asserting the one that did not fire.
            return Err(ToolError::Execution(format!(
                "read_image: {target}: image is {width}x{height}px ({} bytes on the \
                 wire) and could not be brought under the provider limits \
                 ({}px per dimension, {} bytes base64). Re-export it smaller \
                 (crop or scale down) and read that instead.",
                runtime::image_guard::base64_len(bytes.len()),
                runtime::image_guard::IMAGE_CLAMP_DIMENSION,
                runtime::image_guard::MAX_IMAGE_BASE64_BYTES,
            )));
        }
    };
    ctx.image_sink
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push((staged_media_type.clone(), encoded));
    to_pretty_json(json!({
        "staged": true,
        "media_type": staged_media_type,
        "bytes": staged_bytes,
        "downscaled": downscaled,
    }))
}

/// Detect a supported image media type from magic bytes. Returns `None` for
/// anything the Anthropic image API does not accept, so the tool fails loudly
/// rather than staging an unusable blob.
pub(crate) fn sniff_image_mime(bytes: &[u8]) -> Option<&'static str> {
    match bytes {
        [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, ..] => Some("image/png"),
        [0xFF, 0xD8, 0xFF, ..] => Some("image/jpeg"),
        [0x47, 0x49, 0x46, 0x38, ..] => Some("image/gif"),
        [0x52, 0x49, 0x46, 0x46, _, _, _, _, 0x57, 0x45, 0x42, 0x50, ..] => Some("image/webp"),
        _ => None,
    }
}

fn enforce_read_boundary(
    enforcer: Option<&PermissionEnforcer>,
    session_mode: Option<PermissionMode>,
    path: &str,
    workspace_root: Option<&Path>,
) -> Result<Option<std::path::PathBuf>, ToolError> {
    let Some(root) = workspace_root else {
        return Ok(None);
    };
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let candidate = if Path::new(path).is_absolute() {
        std::path::PathBuf::from(path)
    } else {
        canonical_root.join(path)
    };
    let resolved = resolve_for_boundary_check(&candidate);
    // Mirror `enforce_workspace_boundary`: danger-full-access (and Allow —
    // both render as "full-access" and both already permit any write) means
    // the user explicitly authorized unrestricted access, so reads outside
    // the workspace are allowed too. Keeping reads confined while writes are
    // not was pure friction with no protection — `bash`/`read_image` could
    // already read the same paths in these modes.
    //
    // The mode is read from the registry enforcer *or* the foreground session
    // mode on the context: the foreground `tool_registry` carries no enforcer
    // (gating is done at the runtime layer), so without the session-mode
    // fallback a full-access user is wrongly denied an outside `read_file`.
    if !boundary_is_full_access(enforcer, session_mode) {
        validate_workspace_boundary(&resolved, &canonical_root).map_err(|e| {
            ToolError::PermissionDenied {
                tool: "read_file".to_owned(),
                reason: e.to_string(),
            }
        })?;
    }
    // See `enforce_workspace_boundary`: read against the validated absolute
    // path so a relative read can't be re-resolved against a divergent cwd.
    Ok(Some(resolved))
}

pub(crate) fn run_write_file(
    input: &WriteFileInput,
    enforcer: Option<&PermissionEnforcer>,
    ctx: &ToolContext,
) -> Result<String, ToolError> {
    let (target, target_path) = prepare_guarded_file_write(ctx, enforcer, "write_file", &input.path)?;
    let output = write_file(&target, &input.content)?;
    ctx.record_workspace_checkpoint_write(&target_path);
    // 자기 자신이 만든 변경은 신선한 것으로 등재 — 연속 write/edit 허용.
    record_file_observation(&ctx.file_reads, &output.file_path);
    to_pretty_json(output)
}

pub(crate) fn run_edit_file(
    input: &EditFileInput,
    enforcer: Option<&PermissionEnforcer>,
    ctx: &ToolContext,
) -> Result<String, ToolError> {
    let (target, target_path) = prepare_guarded_file_write(ctx, enforcer, "edit_file", &input.path)?;
    let output = edit_file(
        &target,
        &input.old_string,
        &input.new_string,
        input.replace_all.unwrap_or(false),
    )?;
    ctx.record_workspace_checkpoint_write(&target_path);
    // 자기 자신이 만든 변경은 신선한 것으로 등재 — 연속 edit 허용.
    record_file_observation(&ctx.file_reads, &output.file_path);
    to_pretty_json(output)
}

pub(crate) fn run_multi_edit(
    input: &MultiEditInput,
    enforcer: Option<&PermissionEnforcer>,
    ctx: &ToolContext,
) -> Result<String, ToolError> {
    let (target, target_path) = prepare_guarded_file_write(ctx, enforcer, "MultiEdit", &input.path)?;
    let edits = input
        .edits
        .iter()
        .map(|edit| {
            (
                edit.old_string.as_str(),
                edit.new_string.as_str(),
                edit.replace_all.unwrap_or(false),
            )
        })
        .collect::<Vec<_>>();
    let output = multi_edit_file(&target, &edits)?;
    ctx.record_workspace_checkpoint_write(&target_path);
    record_file_observation(&ctx.file_reads, &output.file_path);
    to_pretty_json(json!({
        "filePath": output.file_path,
        "editsApplied": edits.len(),
        "structuredPatch": output.structured_patch,
    }))
}

fn prepare_guarded_file_write(
    ctx: &ToolContext,
    enforcer: Option<&PermissionEnforcer>,
    tool_name: &str,
    path: &str,
) -> Result<(String, std::path::PathBuf), ToolError> {
    let target_path = resolve_guarded_write_target(ctx, enforcer, path)?;
    let target = target_path.to_string_lossy().into_owned();
    enforce_read_before_write(&ctx.file_reads, tool_name, &target)?;
    ctx.record_workspace_checkpoint_before(&target_path)?;
    Ok((target, target_path))
}

fn resolve_guarded_write_target(
    ctx: &ToolContext,
    enforcer: Option<&PermissionEnforcer>,
    path: &str,
) -> Result<std::path::PathBuf, ToolError> {
    let resolved = enforce_workspace_boundary(
        enforcer,
        ctx.session_permission_mode(),
        path,
        ctx.workspace_root.as_deref(),
    )?;
    Ok(resolved
        .or_else(|| resolve_against_cwd(path, ctx.cwd.as_deref()))
        .unwrap_or_else(|| std::path::PathBuf::from(path)))
}

struct PendingWorkspaceRestore {
    path: std::path::PathBuf,
    desired: crate::WorkspaceFileSnapshot,
}

/// Whether `path` is an existing entry with more than one hard link. Writing
/// through such a path modifies every alias — including any outside the
/// workspace that the symlink-aware boundary check cannot see. A missing file
/// (or a non-Unix platform, where `nlink` is unavailable) reports `false`.
#[cfg(unix)]
fn path_has_hard_link_aliases(path: &std::path::Path) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.nlink() > 1)
}

#[cfg(not(unix))]
fn path_has_hard_link_aliases(_path: &std::path::Path) -> bool {
    false
}

#[allow(clippy::too_many_lines)]
pub(crate) fn restore_workspace_checkpoint(
    ctx: &ToolContext,
    enforcer: Option<&PermissionEnforcer>,
    target_turn_index: usize,
    force: bool,
) -> Result<crate::WorkspaceRestoreSummary, ToolError> {
    let plan = ctx
        .workspace_restore_plan(target_turn_index)
        .map_err(ToolError::InvalidInput)?;
    let mut summary = crate::WorkspaceRestoreSummary {
        target_turn_index: plan.target_turn_index,
        incomplete_range: plan.incomplete_range,
        ..crate::WorkspaceRestoreSummary::default()
    };
    let mut pending = Vec::new();
    for entry in plan.entries {
        if entry.desired.is_oversized() || entry.expected_current.is_oversized() {
            summary.skipped.push(crate::WorkspaceRestoreSkippedPath {
                path: entry.path,
                reason: "checkpoint snapshot exceeded the per-file size cap".to_string(),
            });
            continue;
        }
        let guarded_path = match resolve_guarded_write_target(
            ctx,
            enforcer,
            &entry.path.to_string_lossy(),
        ) {
            Ok(path) => path,
            Err(error) => {
                summary.skipped.push(crate::WorkspaceRestoreSkippedPath {
                    path: entry.path,
                    reason: error.to_string(),
                });
                continue;
            }
        };
        let current = match crate::workspace_checkpoint::capture_snapshot(&guarded_path) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                summary.skipped.push(crate::WorkspaceRestoreSkippedPath {
                    path: guarded_path,
                    reason: error.to_string(),
                });
                continue;
            }
        };
        if !force && current != entry.expected_current {
            summary.conflicted.push(guarded_path);
            continue;
        }
        // A write that would go *through* a hard link can modify a file outside
        // the workspace that shares the same inode — the path-boundary check
        // follows symlinks but is blind to hard links. Skip and report, as with
        // an out-of-bounds path. (A delete only unlinks this name, so it stays
        // safe and is not gated here.)
        if entry.desired.content.is_some() && path_has_hard_link_aliases(&guarded_path) {
            summary.skipped.push(crate::WorkspaceRestoreSkippedPath {
                path: guarded_path,
                reason: "target has hard-link aliases; skipped to avoid modifying a file outside the workspace".to_string(),
            });
            continue;
        }
        pending.push(PendingWorkspaceRestore {
            path: guarded_path,
            desired: entry.desired,
        });
    }

    if !pending.is_empty() {
        summary.restore_checkpoint_turn_index =
            Some(ctx.begin_workspace_checkpoint(plan.suggested_checkpoint_turn_index));
    }
    for restore in pending {
        if let Err(error) = ctx.record_workspace_checkpoint_before(&restore.path) {
            summary.skipped.push(crate::WorkspaceRestoreSkippedPath {
                path: restore.path,
                reason: error.to_string(),
            });
            continue;
        }
        let result = match restore.desired.content {
            Some(content) => {
                if let Some(parent) = restore.path.parent() {
                    std::fs::create_dir_all(parent).and_then(|()| std::fs::write(&restore.path, content))
                } else {
                    std::fs::write(&restore.path, content)
                }
                .map(|()| false)
            }
            None => match std::fs::remove_file(&restore.path) {
                Ok(()) => Ok(true),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
                Err(error) => Err(error),
            },
        };
        match result {
            Ok(deleted) => {
                ctx.record_workspace_checkpoint_write(&restore.path);
                record_file_observation(&ctx.file_reads, &restore.path.to_string_lossy());
                if deleted {
                    summary.deleted.push(restore.path);
                } else {
                    summary.restored.push(restore.path);
                }
            }
            Err(error) => summary.skipped.push(crate::WorkspaceRestoreSkippedPath {
                path: restore.path,
                reason: error.to_string(),
            }),
        }
    }
    if summary.restore_checkpoint_turn_index.is_some() {
        ctx.finish_workspace_checkpoint()?;
    }
    Ok(summary)
}

/// CC 패리티 read-before-edit 가드 — 모델 툴 경로(`edit_file`, 기존 파일을
/// 덮어쓰는 `write_file`) 전용. 이 대화에서 파일을 읽은 적 없거나(레지스트리
/// 미등재) 마지막 읽기 이후 디스크가 바뀐 경우(hash 불일치가 권위) 실행 전에
/// 거부한다. 에러는 짧고 actionable하게 — 변경된 최신 내용을 에러에 재주입하지
/// 않고 `read_file` 재호출을 유도한다(CC 방식). 내부 시스템 writer(훅, 프로브
/// revert, 세션 파일 등)는 `runtime::{edit_file, write_file}`를 직접 호출하므로
/// 이 가드를 타지 않는다.
fn enforce_read_before_write(
    file_reads: &std::sync::Mutex<FileReadRegistry>,
    tool: &str,
    target: &str,
) -> Result<(), ToolError> {
    let freshness = file_reads
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .check(Path::new(target));
    match freshness {
        FileFreshness::Missing | FileFreshness::Fresh => Ok(()),
        FileFreshness::NeverRead => Err(ToolError::InvalidInput(format!(
            "{tool}: {target} exists but has not been read in this conversation. \
             Read it with read_file first, then retry this change."
        ))),
        FileFreshness::ModifiedSinceRead => Err(ToolError::InvalidInput(format!(
            "{tool}: {target} has changed on disk since it was last read (modified \
             by the user or an external tool). Re-read it with read_file, then \
             re-apply your change against the current content."
        ))),
    }
}

/// 성공한 read/write/edit 후 현재 디스크 상태를 대화 스코프 레지스트리에
/// 기록한다. 디스패치 enrich 단계의 auto-format이 파일을 재작성하면
/// `crate::dispatch`가 같은 경로로 재기록해 stale 거부(라이브락)를 막는다.
pub(crate) fn record_file_observation(
    file_reads: &std::sync::Mutex<FileReadRegistry>,
    path: &str,
) {
    file_reads
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .record_from_disk(Path::new(path));
}

/// Debug mode: insert a temporary instrumentation probe and record it in the
/// context's probe ledger so it is auto-reverted when the run ends (see
/// [`ToolContext::revert_probes`]). The probe is `<anchor>` followed by a new
/// line `/*ZO_PROBE:<id>*/ <statement>`. Reuses `edit_file`'s unique-anchor
/// guard, so an ambiguous anchor is rejected rather than silently mis-placed.
/// Same `WorkspaceWrite` boundary as `edit_file`; deliberately *not*
/// auto-formatted (the dispatch enrich step keys on `edit_file`/`write_file`
/// only) so the recorded snippet stays byte-identical for an exact strip on
/// revert.
pub(crate) fn run_instrument_log(
    input: &InstrumentLogInput,
    enforcer: Option<&PermissionEnforcer>,
    ctx: &ToolContext,
) -> Result<String, ToolError> {
    let resolved = enforce_workspace_boundary(
        enforcer,
        ctx.session_permission_mode(),
        &input.path,
        ctx.workspace_root.as_deref(),
    )?;
    let target = resolved
        .or_else(|| resolve_against_cwd(&input.path, ctx.cwd.as_deref()))
        .map_or_else(|| input.path.clone(), |p| p.to_string_lossy().into_owned());
    let marker = format!("/*ZO_PROBE:{}*/", next_probe_id());
    // Exact bytes inserted, recorded verbatim for a byte-identical revert. Own
    // line, attached after the anchor.
    let snippet = format!("\n{marker} {}", input.statement);
    let output = edit_file(
        &target,
        &input.anchor,
        &format!("{}{snippet}", input.anchor),
        false,
    )?;
    ctx.probe_sink
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(Probe {
            path: std::path::PathBuf::from(output.file_path.clone()),
            snippet,
        });
    // 프로브 삽입도 모델이 인지한 파일 변경 — read-registry에 반영해 직후의
    // edit_file이 거짓 "외부 변경" 거부를 맞지 않게 한다. (가드 자체는
    // InstrumentLog에 걸지 않는다: 자동 revert되는 임시 프로브라 read-first
    // 강제 대상이 아니다.)
    record_file_observation(&ctx.file_reads, &output.file_path);
    to_pretty_json(json!({
        "instrumented": true,
        "path": output.file_path,
        "marker": marker,
    }))
}

/// Monotonic per-process id so each probe marker is unique and findable. Only
/// uniqueness matters (the full snippet is recorded for the literal revert), so
/// a relaxed counter is sufficient.
fn next_probe_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// Record or update a debugging hypothesis in the run's out-of-band ledger and
/// return the full ledger, so a debugger sub-agent's reasoning persists across
/// iterations (see [`ToolContext::hypothesis_sink`]). Also mirrors the ledger to
/// a per-run scratch file under the OS temp dir (never inside the repo) for
/// post-run inspection — best-effort, like probe revert.
pub(crate) fn run_debug_hypothesis(input: DebugHypothesisInput, ctx: &ToolContext) -> String {
    let rendered = {
        let mut ledger = ctx
            .hypothesis_sink
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let id = upsert_hypothesis(
            &mut ledger,
            input.id,
            input.hypothesis,
            input.status,
            input.evidence,
        );
        format!("{}\n\nUpdated `{id}`.", render_ledger(&ledger))
    };
    match mirror_ledger_to_scratch(&ctx.hypothesis_sink, &rendered) {
        Some(path) => format!("{rendered} Durable record: {path}"),
        None => rendered,
    }
}

/// Upsert a hypothesis into `ledger`: when `id` matches an existing entry,
/// replace its statement/status/evidence; otherwise append a new entry (under
/// the caller's `id`, or an auto-assigned `h<n>` when none is given). Returns the
/// id used. Pure over the vec so it is unit-testable without a [`ToolContext`].
fn upsert_hypothesis(
    ledger: &mut Vec<DebugHypothesis>,
    id: Option<String>,
    statement: String,
    status: HypothesisStatus,
    evidence: Option<String>,
) -> String {
    if let Some(id) = id {
        if let Some(existing) = ledger.iter_mut().find(|h| h.id == id) {
            existing.statement = statement;
            existing.status = status;
            existing.evidence = evidence;
            return id;
        }
        ledger.push(DebugHypothesis {
            id: id.clone(),
            statement,
            status,
            evidence,
        });
        return id;
    }
    let id = next_hypothesis_id(ledger);
    ledger.push(DebugHypothesis {
        id: id.clone(),
        statement,
        status,
        evidence,
    });
    id
}

/// First `h<n>` id not already taken in `ledger` (n starts at length + 1), so an
/// auto-assigned id never collides with a caller-chosen one.
fn next_hypothesis_id(ledger: &[DebugHypothesis]) -> String {
    let mut n = ledger.len() + 1;
    loop {
        let candidate = format!("h{n}");
        if ledger.iter().all(|h| h.id != candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// Render the ledger as a compact, model-readable summary: a count line by
/// status, then one line per hypothesis (in record order) with its evidence.
fn render_ledger(ledger: &[DebugHypothesis]) -> String {
    use std::fmt::Write as _;
    if ledger.is_empty() {
        return "Debug hypothesis ledger: (empty)".to_string();
    }
    let count = |status: HypothesisStatus| ledger.iter().filter(|h| h.status == status).count();
    let mut out = format!(
        "Debug hypothesis ledger ({} total: {} open, {} confirmed, {} refuted)",
        ledger.len(),
        count(HypothesisStatus::Open),
        count(HypothesisStatus::Confirmed),
        count(HypothesisStatus::Refuted),
    );
    for h in ledger {
        let _ = write!(out, "\n- {} [{}] {}", h.id, h.status.label(), h.statement);
        if let Some(evidence) = &h.evidence {
            let _ = write!(out, "\n    \u{21b3} evidence: {evidence}");
        }
    }
    out
}

/// Mirror `rendered` to a per-run scratch file under the OS temp dir, keyed by
/// process id + the ledger `Arc`'s address so concurrent runs never collide and
/// nothing lands inside the repo tree. Best-effort: returns the path on success,
/// `None` if the temp write fails (the in-memory ledger and tool result are
/// unaffected).
fn mirror_ledger_to_scratch(
    sink: &std::sync::Arc<std::sync::Mutex<Vec<DebugHypothesis>>>,
    rendered: &str,
) -> Option<String> {
    let dir = std::env::temp_dir().join("zo-debug-hypotheses");
    std::fs::create_dir_all(&dir).ok()?;
    let key = std::sync::Arc::as_ptr(sink) as usize;
    let path = dir.join(format!("{}-{key:x}.md", std::process::id()));
    std::fs::write(&path, format!("# Debug hypotheses\n\n{rendered}\n")).ok()?;
    Some(path.to_string_lossy().into_owned())
}

fn resolve_search_base_path(
    path: Option<&str>,
    enforcer: Option<&PermissionEnforcer>,
    session_mode: Option<PermissionMode>,
    workspace_root: Option<&Path>,
    cwd: Option<&Path>,
) -> Result<Option<String>, ToolError> {
    let resolved =
        enforce_read_boundary(enforcer, session_mode, path.unwrap_or("."), workspace_root)?;
    Ok(match resolved {
        Some(path) => Some(path.to_string_lossy().into_owned()),
        None => match path {
            Some(path) => Some(
                resolve_against_cwd(path, cwd)
                    .map_or_else(|| path.to_owned(), |path| path.to_string_lossy().into_owned()),
            ),
            None => cwd.map(|path| path.to_string_lossy().into_owned()),
        },
    })
}

pub(crate) fn run_glob_search(
    input: &GlobSearchInputValue,
    enforcer: Option<&PermissionEnforcer>,
    session_mode: Option<PermissionMode>,
    workspace_root: Option<&Path>,
    cwd: Option<&Path>,
) -> Result<String, ToolError> {
    let path = resolve_search_base_path(
        input.path.as_deref(),
        enforcer,
        session_mode,
        workspace_root,
        cwd,
    )?;
    to_pretty_json(glob_search(input.pattern_or_default(), path.as_deref())?)
}

pub(crate) fn run_grep_search(
    input: &GrepSearchInput,
    enforcer: Option<&PermissionEnforcer>,
    session_mode: Option<PermissionMode>,
    workspace_root: Option<&Path>,
    cwd: Option<&Path>,
) -> Result<String, ToolError> {
    let mut input = input.clone();
    input.path = resolve_search_base_path(
        input.path.as_deref(),
        enforcer,
        session_mode,
        workspace_root,
        cwd,
    )?;
    to_pretty_json(grep_search(&input)?)
}

#[cfg(test)]
mod instrument_log_tests {
    use super::{run_instrument_log, InstrumentLogInput};
    use crate::context::ToolContext;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_source(body: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        // No workspace_root in these tests → boundary off; absolute path with no
        // cwd mutation keeps them deterministic and race-free.
        let path = std::env::temp_dir().join(format!(
            "zo-g23-{}-{unique}-{counter}.rs",
            std::process::id()
        ));
        std::fs::write(&path, body).expect("write fixture");
        path
    }

    fn probe_count(ctx: &ToolContext) -> usize {
        ctx.probe_sink
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    fn instrument(ctx: &ToolContext, path: &std::path::Path, anchor: &str, statement: &str) {
        run_instrument_log(
            &InstrumentLogInput {
                path: path.to_string_lossy().into_owned(),
                anchor: anchor.to_string(),
                statement: statement.to_string(),
            },
            None,
            ctx,
        )
        .expect("instrument succeeds");
    }

    #[test]
    fn instrument_then_revert_restores_byte_identical() {
        let original = "fn main() {\n    let x = 1;\n    println!(\"{x}\");\n}\n";
        let path = temp_source(original);
        let ctx = ToolContext::new();

        instrument(&ctx, &path, "let x = 1;", "eprintln!(\"probe x={x:?}\");");

        // The probe is in the file and recorded once in the ledger.
        let after = std::fs::read_to_string(&path).expect("read after instrument");
        assert!(after.contains("ZO_PROBE"), "probe inserted: {after}");
        assert!(
            after.contains("eprintln!(\"probe x={x:?}\");"),
            "statement: {after}"
        );
        assert_eq!(probe_count(&ctx), 1);

        // Auto-revert restores the file exactly and drains the ledger.
        assert_eq!(ctx.revert_probes(), 1);
        let restored = std::fs::read_to_string(&path).expect("read after revert");
        assert_eq!(restored, original, "byte-identical restore");
        assert_eq!(probe_count(&ctx), 0, "ledger drained");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn revert_preserves_unrelated_edits_around_the_probe() {
        // A real fix the agent made elsewhere must survive revert; only the
        // probe line is stripped — that is the whole point of the marker.
        let original = "let a = 1;\nlet b = 2;\n";
        let path = temp_source(original);
        let ctx = ToolContext::new();
        instrument(&ctx, &path, "let a = 1;", "// trace a");

        // Simulate the agent fixing an unrelated line after instrumenting.
        let edited = std::fs::read_to_string(&path)
            .expect("read")
            .replace("let b = 2;", "let b = 3; // fixed");
        std::fs::write(&path, &edited).expect("write edit");

        assert_eq!(ctx.revert_probes(), 1);
        let restored = std::fs::read_to_string(&path).expect("read after revert");
        assert_eq!(restored, "let a = 1;\nlet b = 3; // fixed\n");
        assert!(!restored.contains("ZO_PROBE"), "marker gone: {restored}");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn ambiguous_anchor_is_rejected_and_records_no_probe() {
        // edit_file's unique-anchor guard surfaces: a probe must target one site,
        // and a failed insert must not leave a phantom ledger entry.
        let path = temp_source("dup\ndup\n");
        let ctx = ToolContext::new();
        let result = run_instrument_log(
            &InstrumentLogInput {
                path: path.to_string_lossy().into_owned(),
                anchor: "dup".to_string(),
                statement: "// x".to_string(),
            },
            None,
            &ctx,
        );
        assert!(result.is_err(), "ambiguous anchor must be rejected");
        assert_eq!(probe_count(&ctx), 0, "no probe recorded on failure");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn two_probes_revert_independently() {
        let original = "one\ntwo\n";
        let path = temp_source(original);
        let ctx = ToolContext::new();
        instrument(&ctx, &path, "one", "// p1");
        instrument(&ctx, &path, "two", "// p2");
        assert_eq!(probe_count(&ctx), 2);

        assert_eq!(ctx.revert_probes(), 2);
        let restored = std::fs::read_to_string(&path).expect("read after revert");
        assert_eq!(restored, original, "both probes stripped, byte-identical");
        let _ = std::fs::remove_file(&path);
    }
}

#[cfg(test)]
mod debug_hypothesis_tests {
    use super::{
        next_hypothesis_id, render_ledger, run_debug_hypothesis, upsert_hypothesis,
        DebugHypothesisInput,
    };
    use crate::context::{DebugHypothesis, HypothesisStatus, ToolContext};

    fn entry(id: &str, status: HypothesisStatus) -> DebugHypothesis {
        DebugHypothesis {
            id: id.to_string(),
            statement: format!("statement for {id}"),
            status,
            evidence: None,
        }
    }

    #[test]
    fn upsert_appends_new_and_auto_assigns_sequential_ids() {
        let mut ledger = Vec::new();
        let first = upsert_hypothesis(
            &mut ledger,
            None,
            "cache is stale".into(),
            HypothesisStatus::Open,
            None,
        );
        let second = upsert_hypothesis(
            &mut ledger,
            None,
            "race in init".into(),
            HypothesisStatus::Open,
            None,
        );
        assert_eq!((first.as_str(), second.as_str()), ("h1", "h2"));
        assert_eq!(ledger.len(), 2);
    }

    #[test]
    fn upsert_updates_existing_by_id_in_place() {
        let mut ledger = vec![entry("h1", HypothesisStatus::Open)];
        let id = upsert_hypothesis(
            &mut ledger,
            Some("h1".into()),
            "refined statement".into(),
            HypothesisStatus::Confirmed,
            Some("the test now passes".into()),
        );
        assert_eq!(id, "h1");
        assert_eq!(ledger.len(), 1, "update must not append a duplicate");
        assert_eq!(ledger[0].status, HypothesisStatus::Confirmed);
        assert_eq!(ledger[0].statement, "refined statement");
        assert_eq!(ledger[0].evidence.as_deref(), Some("the test now passes"));
    }

    #[test]
    fn upsert_creates_a_caller_named_id_when_absent() {
        let mut ledger = Vec::new();
        let id = upsert_hypothesis(
            &mut ledger,
            Some("cache-stale".into()),
            "named guess".into(),
            HypothesisStatus::Open,
            None,
        );
        assert_eq!(id, "cache-stale");
        assert_eq!(ledger.len(), 1);
    }

    #[test]
    fn auto_id_skips_a_caller_named_collision() {
        // A caller named its hypothesis "h2"; the next auto-id must not reuse it.
        let mut ledger = Vec::new();
        upsert_hypothesis(
            &mut ledger,
            Some("h2".into()),
            "named h2".into(),
            HypothesisStatus::Open,
            None,
        );
        let auto = upsert_hypothesis(
            &mut ledger,
            None,
            "auto".into(),
            HypothesisStatus::Open,
            None,
        );
        assert_eq!(auto, "h3", "auto-id steps past the taken h2");
        assert_eq!(next_hypothesis_id(&ledger), "h4");
    }

    #[test]
    fn render_summarizes_counts_and_evidence() {
        let ledger = vec![
            DebugHypothesis {
                id: "h1".into(),
                statement: "stale cache".into(),
                status: HypothesisStatus::Refuted,
                evidence: Some("cache key matched".into()),
            },
            entry("h2", HypothesisStatus::Confirmed),
            entry("h3", HypothesisStatus::Open),
        ];
        let rendered = render_ledger(&ledger);
        assert!(
            rendered.contains("3 total: 1 open, 1 confirmed, 1 refuted"),
            "count line: {rendered}"
        );
        assert!(rendered.contains("h1 [refuted] stale cache"), "{rendered}");
        assert!(
            rendered.contains("evidence: cache key matched"),
            "{rendered}"
        );
    }

    #[test]
    fn render_of_empty_ledger_is_explicit() {
        assert_eq!(render_ledger(&[]), "Debug hypothesis ledger: (empty)");
    }

    #[test]
    fn run_records_to_scratch_and_persists_across_calls() {
        // The same context shared across calls is the whole point: call two
        // updates the hypothesis call one recorded, proving the ledger persists
        // between a debugger sub-agent's iterations.
        let ctx = ToolContext::new();
        let first = run_debug_hypothesis(
            DebugHypothesisInput {
                hypothesis: "the flush is skipped on early return".into(),
                status: HypothesisStatus::Open,
                evidence: None,
                id: None,
            },
            &ctx,
        );
        assert!(first.contains("h1 [open] the flush is skipped"), "{first}");

        let second = run_debug_hypothesis(
            DebugHypothesisInput {
                hypothesis: "the flush is skipped on early return".into(),
                status: HypothesisStatus::Confirmed,
                evidence: Some("added a probe; flush never ran".into()),
                id: Some("h1".into()),
            },
            &ctx,
        );
        assert!(
            second.contains("1 total: 0 open, 1 confirmed, 0 refuted"),
            "ledger persisted and updated in place: {second}"
        );

        // The scratch mirror exists and holds the latest ledger; clean it up.
        let path = second
            .rsplit("Durable record: ")
            .next()
            .map(str::trim)
            .expect("durable record path in result");
        let on_disk = std::fs::read_to_string(path).expect("scratch file written");
        assert!(on_disk.contains("confirmed"), "scratch mirror: {on_disk}");
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod read_image_tests {
    use super::{run_read_image, sniff_image_mime, ReadImageInput};
    use crate::context::ToolContext;
    use runtime::session::{ContentBlock, ConversationMessage};
    use std::time::{SystemTime, UNIX_EPOCH};

    // Smallest valid 1x1 PNG: 8-byte signature + IHDR + IDAT + IEND.
    const PNG_1X1: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    fn temp_file(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("zo-g10-{name}-{unique}.bin"));
        std::fs::write(&path, bytes).expect("write fixture");
        path
    }

    fn oversized_rgba_png(width: u32, height: u32) -> Vec<u8> {
        assert!(width > runtime::image_guard::IMAGE_CLAMP_DIMENSION);
        assert_eq!(height, 1, "test helper keeps the zlib stream single-block");
        let raw_len = 1usize + usize::try_from(width).expect("width fits usize") * 4;
        assert!(u16::try_from(raw_len).is_ok(), "single uncompressed deflate block");

        let mut png = Vec::new();
        png.extend_from_slice(b"\x89PNG\r\n\x1a\n");

        let mut ihdr = Vec::with_capacity(13);
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.push(8); // bit depth
        ihdr.push(6); // RGBA
        ihdr.push(0); // compression
        ihdr.push(0); // filter
        ihdr.push(0); // interlace
        append_png_chunk(&mut png, *b"IHDR", &ihdr);

        // zlib stream using one uncompressed deflate block. The raw scanline is
        // all zeros: PNG filter byte 0 + transparent black RGBA pixels.
        let mut idat = Vec::new();
        idat.extend_from_slice(&[0x78, 0x01]); // zlib header, no compression
        idat.push(0x01); // final stored block
        let len = u16::try_from(raw_len).expect("raw len fits u16");
        idat.extend_from_slice(&len.to_le_bytes());
        idat.extend_from_slice(&(!len).to_le_bytes());
        idat.resize(idat.len() + raw_len, 0);
        let adler = adler32_zero_bytes(raw_len);
        idat.extend_from_slice(&adler.to_be_bytes());
        append_png_chunk(&mut png, *b"IDAT", &idat);
        append_png_chunk(&mut png, *b"IEND", &[]);
        png
    }

    fn append_png_chunk(out: &mut Vec<u8>, kind: [u8; 4], data: &[u8]) {
        out.extend_from_slice(&(u32::try_from(data.len()).expect("chunk len fits u32")).to_be_bytes());
        out.extend_from_slice(&kind);
        out.extend_from_slice(data);
        let crc = crc32_png(kind, data);
        out.extend_from_slice(&crc.to_be_bytes());
    }

    fn crc32_png(kind: [u8; 4], data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for byte in kind.iter().chain(data.iter()) {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
            }
        }
        !crc
    }

    fn adler32_zero_bytes(len: usize) -> u32 {
        // Adler-32 over `len` zero bytes: a stays 1; b accumulates a once per
        // byte, modulo BASE. This is enough for the all-zero scanline fixture.
        const BASE: u32 = 65_521;
        let b = u32::try_from(len % BASE as usize).expect("mod result fits u32");
        (b << 16) | 1
    }

    #[test]
    fn read_image_stages_one_image_into_the_sink() {
        let path = temp_file("pixel-png", PNG_1X1);
        // No workspace_root → no boundary enforcement; absolute path read works
        // with no cwd mutation (deterministic, no test races).
        let ctx = ToolContext::new();
        let out = run_read_image(
            &ReadImageInput {
                path: path.to_string_lossy().into_owned(),
            },
            &ctx,
        )
        .expect("read_image should succeed on a valid PNG");
        assert!(out.contains("image/png"), "summary: {out}");

        // The sink holds exactly one (media_type, base64) entry.
        let staged: Vec<(String, String)> = std::mem::take(
            &mut *ctx
                .image_sink
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        assert_eq!(staged.len(), 1);
        assert_eq!(staged[0].0, "image/png");
        assert!(!staged[0].1.is_empty(), "base64 payload present");

        // The drain point builds a ToolResult carrying the image, as the loop does.
        let message = ConversationMessage::tool_result_with_images(
            "tu-1",
            "read_image",
            "staged",
            false,
            staged,
        );
        let ContentBlock::ToolResult { images, .. } = &message.blocks[0] else {
            panic!("expected a ToolResult block");
        };
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].0, "image/png");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_image_downscales_oversized_image_before_staging() {
        use base64::Engine as _;

        let oversized = oversized_rgba_png(runtime::image_guard::IMAGE_CLAMP_DIMENSION + 1, 1);
        let path = temp_file("oversized-png", &oversized);
        let ctx = ToolContext::new();
        let out = run_read_image(
            &ReadImageInput {
                path: path.to_string_lossy().into_owned(),
            },
            &ctx,
        )
        .expect("read_image should downscale an oversized valid PNG");
        let summary: serde_json::Value = serde_json::from_str(&out).expect("json summary");
        assert_eq!(summary["staged"], true, "summary: {out}");
        assert_eq!(summary["media_type"], "image/png", "summary: {out}");
        assert_eq!(summary["downscaled"], true, "summary: {out}");

        let staged: Vec<(String, String)> = std::mem::take(
            &mut *ctx
                .image_sink
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        assert_eq!(staged.len(), 1);
        assert_eq!(staged[0].0, "image/png");
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(staged[0].1.as_bytes())
            .expect("staged base64 decodes");
        assert_eq!(
            summary["bytes"],
            serde_json::Value::from(decoded.len()),
            "JSON bytes must describe the staged payload, not the original"
        );
        assert_eq!(
            runtime::image_guard::guard_image_bytes(&decoded),
            runtime::image_guard::ImageGuardOutcome::Keep,
            "staged payload is now provider-safe"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_image_reads_outside_the_workspace_boundary() {
        // Unlike read_file, read_image must stage an image that lives *outside*
        // the configured workspace root (OCR/screenshot scratch in /tmp, etc.).
        let image = temp_file("outside-pixel-png", PNG_1X1);
        // A workspace root that deliberately does NOT contain `image`.
        let ws_root = std::env::temp_dir().join("zo-g10-ws-root-only");
        let ctx = ToolContext::new().with_workspace_root(ws_root);
        let out = run_read_image(
            &ReadImageInput {
                path: image.to_string_lossy().into_owned(),
            },
            &ctx,
        )
        .expect("read_image must read outside the workspace boundary");
        assert!(out.contains("image/png"), "summary: {out}");
        assert_eq!(
            ctx.image_sink
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len(),
            1,
            "exactly one image staged",
        );
        let _ = std::fs::remove_file(&image);
    }

    #[test]
    fn read_image_rejects_a_non_image_file() {
        let path = temp_file("not-image", b"this is plain text, not an image");
        let ctx = ToolContext::new();
        let result = run_read_image(
            &ReadImageInput {
                path: path.to_string_lossy().into_owned(),
            },
            &ctx,
        );
        assert!(result.is_err(), "non-image must be rejected, not staged");
        assert!(
            ctx.image_sink
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty(),
            "nothing staged on rejection"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn sniff_image_mime_detects_supported_formats() {
        assert_eq!(sniff_image_mime(PNG_1X1), Some("image/png"));
        assert_eq!(
            sniff_image_mime(&[0xFF, 0xD8, 0xFF, 0xE0, 0x00]),
            Some("image/jpeg")
        );
        assert_eq!(sniff_image_mime(b"GIF89a..."), Some("image/gif"));
        assert_eq!(
            sniff_image_mime(&[0x52, 0x49, 0x46, 0x46, 0, 0, 0, 0, 0x57, 0x45, 0x42, 0x50, 0]),
            Some("image/webp")
        );
        assert_eq!(sniff_image_mime(b"plain text"), None);
    }
}

#[cfg(test)]
mod boundary_tests {
    use super::{
        restore_workspace_checkpoint, run_edit_file, run_grep_search, run_read_file,
        run_write_file, EditFileInput, ReadFileInput, WriteFileInput,
    };
    use crate::{error::ToolError, ToolContext};
    use runtime::permission_enforcer::PermissionEnforcer;
    use runtime::{GrepSearchInput, PermissionMode, PermissionPolicy};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("zo-tools-{name}-{unique}"))
    }

    fn workspace_write_enforcer() -> PermissionEnforcer {
        PermissionEnforcer::new(PermissionPolicy::new(PermissionMode::WorkspaceWrite))
    }

    fn context(workspace_root: Option<&std::path::Path>) -> ToolContext {
        let mut context = ToolContext::new();
        context.workspace_root = workspace_root.map(std::path::Path::to_path_buf);
        context
    }

    fn reads() -> std::sync::Mutex<runtime::FileReadRegistry> {
        std::sync::Mutex::new(runtime::FileReadRegistry::new())
    }

    #[cfg(unix)]
    #[test]
    fn detects_hard_link_aliases() {
        let dir = temp_dir("hardlink-detect");
        std::fs::create_dir_all(&dir).expect("dir");
        let plain = dir.join("plain.txt");
        std::fs::write(&plain, "x").expect("write plain");
        assert!(
            !super::path_has_hard_link_aliases(&plain),
            "a single-link file has no aliases"
        );

        let target = dir.join("target.txt");
        std::fs::write(&target, "y").expect("write target");
        let alias = dir.join("alias.txt");
        std::fs::hard_link(&target, &alias).expect("hard link");
        assert!(super::path_has_hard_link_aliases(&target), "linked target");
        assert!(super::path_has_hard_link_aliases(&alias), "linked alias");

        assert!(
            !super::path_has_hard_link_aliases(&dir.join("missing.txt")),
            "a missing path has nothing to alias"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn rewind_skips_hard_linked_target_and_leaves_external_file_intact() {
        // A checkpoint restore must never write *through* a hard link: a tracked
        // path sharing an inode with a file outside the workspace would let the
        // restore modify that external file, which the symlink-aware boundary
        // check cannot detect.
        let workspace = temp_dir("hardlink-workspace");
        std::fs::create_dir_all(&workspace).expect("workspace dir");
        let external_dir = temp_dir("hardlink-external");
        std::fs::create_dir_all(&external_dir).expect("external dir");

        let file = workspace.join("tracked.txt");
        std::fs::write(&file, "A").expect("seed content A");

        let ctx = context(Some(&workspace));
        let enforcer = workspace_write_enforcer();

        // One checkpoint turn: before="A", after="B".
        let turn = ctx.begin_workspace_checkpoint(0);
        ctx.record_workspace_checkpoint_before(&file)
            .expect("record before");
        std::fs::write(&file, "B").expect("write content B");
        ctx.record_workspace_checkpoint_write(&file);
        ctx.finish_workspace_checkpoint().expect("finish checkpoint");

        // Turn the tracked path into a hard-link aliasing an external file whose
        // content matches the checkpoint's `after` (so the conflict check passes
        // and we reach the hard-link guard).
        let external = external_dir.join("secret.txt");
        std::fs::write(&external, "B").expect("external content B");
        std::fs::remove_file(&file).expect("remove tracked");
        std::fs::hard_link(&external, &file).expect("hard link tracked -> external");

        let summary = restore_workspace_checkpoint(&ctx, Some(&enforcer), turn, false)
            .expect("restore runs");

        // `guarded_path` is canonicalized (on macOS `/var` -> `/private/var`),
        // so match by file name rather than the raw path.
        assert!(
            summary.skipped.iter().any(|s| {
                s.path.file_name() == file.file_name()
                    && s.reason.contains("hard-link aliases")
            }),
            "hard-linked target must be skipped, got {:?}",
            summary.skipped
        );
        assert!(summary.restored.is_empty(), "nothing was restored");
        assert_eq!(
            std::fs::read_to_string(&external).expect("read external"),
            "B",
            "external file must be untouched (no write-through)"
        );

        let _ = std::fs::remove_dir_all(&workspace);
        let _ = std::fs::remove_dir_all(&external_dir);
    }

    #[test]
    fn write_inside_workspace_succeeds() {
        let workspace = temp_dir("inside-write");
        std::fs::create_dir_all(&workspace).expect("workspace dir");
        let path = workspace.join("hello.txt");
        let enforcer = workspace_write_enforcer();
        let result = run_write_file(
            &WriteFileInput {
                path: path.to_string_lossy().into_owned(),
                content: "ok".to_owned(),
            },
            Some(&enforcer),
            &context(Some(&workspace)),
        );
        assert!(result.is_ok(), "expected ok, got {result:?}");
    }

    #[test]
    fn write_outside_workspace_denied() {
        let workspace = temp_dir("outside-write-ws");
        std::fs::create_dir_all(&workspace).expect("workspace dir");
        let outside = temp_dir("outside-write-target.txt");
        let enforcer = workspace_write_enforcer();
        let result = run_write_file(
            &WriteFileInput {
                path: outside.to_string_lossy().into_owned(),
                content: "should be denied".to_owned(),
            },
            Some(&enforcer),
            &context(Some(&workspace)),
        );
        match result {
            Err(ToolError::PermissionDenied { reason, .. }) => {
                assert!(
                    reason.contains("outside workspace") || reason.contains("escapes workspace"),
                    "unexpected reason: {reason}"
                );
            }
            other => panic!("expected PermissionDenied, got {other:?}"),
        }
        // Sanity: the file must not exist after a denied write.
        assert!(!outside.exists(), "file should not have been created");
    }

    /// A DANGLING symlink leaf inside the workspace pointing OUT of it must be
    /// denied: `fs::write` follows the link and would create a file outside the
    /// boundary. Regression for the `Path::exists()`-follows-symlink hole where
    /// the link read as a "missing tail" and passed the in-workspace check.
    #[test]
    #[cfg(unix)]
    fn write_through_dangling_symlink_denied() {
        let workspace = temp_dir("dangling-symlink-ws");
        std::fs::create_dir_all(&workspace).expect("workspace dir");
        // Sibling of the workspace, deliberately not yet created (dangling target).
        let outside = temp_dir("dangling-symlink-target.txt");
        let link = workspace.join("evil");
        std::os::unix::fs::symlink(&outside, &link).expect("create dangling symlink");
        let enforcer = workspace_write_enforcer();
        let result = run_write_file(
            &WriteFileInput {
                path: link.to_string_lossy().into_owned(),
                content: "escapes via symlink".to_owned(),
            },
            Some(&enforcer),
            &context(Some(&workspace)),
        );
        match result {
            Err(ToolError::PermissionDenied { reason, .. }) => assert!(
                reason.contains("outside workspace") || reason.contains("escapes workspace"),
                "unexpected reason: {reason}"
            ),
            other => panic!("expected PermissionDenied, got {other:?}"),
        }
        assert!(
            !outside.exists(),
            "symlink target outside the workspace must not have been created"
        );
        let _ = std::fs::remove_dir_all(&workspace);
    }

    /// A 2-hop symlink chain must be resolved to its true out-of-tree destination:
    /// `ws/a -> ws/b -> /outside`, where `/outside` is dangling. The first fix only
    /// followed ONE hop, so `resolve_for_boundary_check` returned the in-workspace
    /// `ws/b`, the containment check passed, and `fs::write` followed the whole
    /// chain out. Regression: the resolver now walks the full chain.
    #[test]
    #[cfg(unix)]
    fn write_through_symlink_chain_denied() {
        let workspace = temp_dir("symlink-chain-ws");
        std::fs::create_dir_all(&workspace).expect("workspace dir");
        // Dangling final target outside the workspace (dir may or may not exist).
        let outside = temp_dir("symlink-chain-target.txt");
        // hop2 (in-workspace) -> outside; hop1 (in-workspace) -> hop2.
        let hop2 = workspace.join("b");
        let hop1 = workspace.join("a");
        std::os::unix::fs::symlink(&outside, &hop2).expect("create hop2 symlink");
        std::os::unix::fs::symlink(&hop2, &hop1).expect("create hop1 symlink");
        let enforcer = workspace_write_enforcer();
        let result = run_write_file(
            &WriteFileInput {
                path: hop1.to_string_lossy().into_owned(),
                content: "escapes via 2-hop chain".to_owned(),
            },
            Some(&enforcer),
            &context(Some(&workspace)),
        );
        match result {
            Err(ToolError::PermissionDenied { reason, .. }) => assert!(
                reason.contains("outside workspace") || reason.contains("escapes workspace"),
                "unexpected reason: {reason}"
            ),
            other => panic!("expected PermissionDenied, got {other:?}"),
        }
        assert!(
            !outside.exists(),
            "chain target outside the workspace must not have been created"
        );
        let _ = std::fs::remove_dir_all(&workspace);
    }

    /// A leaf symlink whose RELATIVE target routes through a symlinked DIRECTORY
    /// must resolve to its true out-of-tree location. `ws/a -> "d/e"` with
    /// `ws/d -> /outside` (an existing out-of-tree dir): a lexical resolver
    /// composes the in-workspace `ws/d/e` and passes containment, but `fs::write`
    /// follows `ws/d` out to `/outside/e`. The resolver must canonicalize the
    /// symlinked directory in the target.
    #[test]
    #[cfg(unix)]
    fn write_through_symlinked_directory_in_relative_target_denied() {
        let workspace = temp_dir("symlinked-dir-ws");
        std::fs::create_dir_all(&workspace).expect("workspace dir");
        // An existing out-of-tree directory the in-workspace `d` link points at.
        let outside_dir = temp_dir("symlinked-dir-outside");
        std::fs::create_dir_all(&outside_dir).expect("outside dir");
        // ws/d -> /outside_dir (symlinked directory); ws/a -> "d/e" (relative).
        std::os::unix::fs::symlink(&outside_dir, workspace.join("d")).expect("dir symlink");
        let link = workspace.join("a");
        std::os::unix::fs::symlink("d/e", &link).expect("relative leaf symlink");
        let escaped = outside_dir.join("e");
        let enforcer = workspace_write_enforcer();
        let result = run_write_file(
            &WriteFileInput {
                path: link.to_string_lossy().into_owned(),
                content: "escapes via symlinked dir in target".to_owned(),
            },
            Some(&enforcer),
            &context(Some(&workspace)),
        );
        match result {
            Err(ToolError::PermissionDenied { reason, .. }) => assert!(
                reason.contains("outside workspace") || reason.contains("escapes workspace"),
                "unexpected reason: {reason}"
            ),
            other => panic!("expected PermissionDenied, got {other:?}"),
        }
        assert!(
            !escaped.exists(),
            "the file must not have been created outside the workspace"
        );
        let _ = std::fs::remove_dir_all(&workspace);
        let _ = std::fs::remove_dir_all(&outside_dir);
    }

    fn danger_full_access_enforcer() -> PermissionEnforcer {
        PermissionEnforcer::new(PermissionPolicy::new(PermissionMode::DangerFullAccess))
    }

    /// Track 4-2: with the guard enabled, a second owner's `edit_file`/`write_file`
    /// on a path a first owner already holds is refused through the dispatch-level
    /// `enforce_write_lease` gate. Exercised end-to-end via `ToolContext` so the
    /// owner-threading and env-gating are covered, not just the lease primitive.
    #[test]
    fn write_lease_blocks_a_second_owner_through_the_context_gate() {
        use crate::ToolContext;
        let _guard = crate::tests::env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let workspace = temp_dir("lease-gate-ws");
        std::fs::create_dir_all(&workspace).expect("workspace dir");
        // Isolate the lease store into this test's own state dir, and enable the
        // opt-in guard for the duration of the test.
        std::env::set_var("ZO_STATE_DIR", workspace.join("state"));
        std::env::set_var("ZO_WORKSPACE_GUARD", "1");

        let target = workspace.join("shared.rs");
        let rel = target.to_string_lossy().into_owned();

        // Agent A acquires the lease on first write.
        let ctx_a = ToolContext::new()
            .with_cwd(workspace.clone())
            .with_lease_owner("agent-a");
        assert!(
            super::enforce_write_lease(&ctx_a, &rel).is_ok(),
            "first owner acquires the lease"
        );

        // Agent B (a different live owner) is refused on the same path.
        let ctx_b = ToolContext::new()
            .with_cwd(workspace.clone())
            .with_lease_owner("agent-b");
        match super::enforce_write_lease(&ctx_b, &rel) {
            Err(ToolError::PermissionDenied { reason, .. }) => {
                assert!(
                    reason.contains("another agent") && reason.contains("agent-a"),
                    "conflict names the holder: {reason}"
                );
            }
            other => panic!("expected a lease conflict, got {other:?}"),
        }

        // Agent A (the holder) may still write — its own lease renews, no conflict.
        assert!(
            super::enforce_write_lease(&ctx_a, &rel).is_ok(),
            "the holder is never blocked by its own lease"
        );

        // A context with no lease owner (solo default) is never gated, even with
        // the guard enabled.
        let ctx_solo = ToolContext::new().with_cwd(workspace.clone());
        assert!(
            super::enforce_write_lease(&ctx_solo, &rel).is_ok(),
            "no lease owner ⇒ coordination off"
        );

        std::env::remove_var("ZO_WORKSPACE_GUARD");
        std::env::remove_var("ZO_STATE_DIR");
    }

    /// The gate stays inert when the guard env is unset, even for an identified
    /// owner — so enabling lease coordination is strictly opt-in.
    #[test]
    fn write_lease_is_inert_without_the_guard_env() {
        use crate::ToolContext;
        let _guard = crate::tests::env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let workspace = temp_dir("lease-inert-ws");
        std::fs::create_dir_all(&workspace).expect("workspace dir");
        std::env::set_var("ZO_STATE_DIR", workspace.join("state"));
        std::env::remove_var("ZO_WORKSPACE_GUARD");

        let target = workspace.join("shared.rs");
        let rel = target.to_string_lossy().into_owned();

        let ctx_a = ToolContext::new()
            .with_cwd(workspace.clone())
            .with_lease_owner("agent-a");
        let ctx_b = ToolContext::new()
            .with_cwd(workspace.clone())
            .with_lease_owner("agent-b");
        // Both succeed: with the guard off no lease is even taken, so there is
        // nothing for a second owner to conflict with.
        assert!(super::enforce_write_lease(&ctx_a, &rel).is_ok());
        assert!(
            super::enforce_write_lease(&ctx_b, &rel).is_ok(),
            "guard off ⇒ no lease taken ⇒ no conflict"
        );

        std::env::remove_var("ZO_STATE_DIR");
    }

    #[test]
    fn write_outside_workspace_allowed_in_danger_full_access() {
        // Regression: a full-access user writing to a sibling project (e.g.
        // ../zo-ide/prd.md) was denied "escapes workspace boundary" even
        // though they explicitly granted danger-full-access. Full access relaxes
        // the boundary so the write lands instead of being refused.
        let workspace = temp_dir("fullaccess-write-ws");
        std::fs::create_dir_all(&workspace).expect("workspace dir");
        let sibling = temp_dir("fullaccess-write-sibling");
        std::fs::create_dir_all(&sibling).expect("sibling dir");
        let outside = sibling.join("prd.md");
        let enforcer = danger_full_access_enforcer();
        let result = run_write_file(
            &WriteFileInput {
                path: outside.to_string_lossy().into_owned(),
                content: "allowed under full access".to_owned(),
            },
            Some(&enforcer),
            &context(Some(&workspace)),
        );
        assert!(
            result.is_ok(),
            "danger-full-access must allow a sibling-dir write, got {result:?}"
        );
        assert!(outside.exists(), "the file should have been written");
        let _ = std::fs::remove_dir_all(&workspace);
        let _ = std::fs::remove_dir_all(&sibling);
    }

    #[test]
    fn write_outside_workspace_allowed_in_allow_mode() {
        // Regression (the `bench_*.sql` incident): in Allow mode — rendered as
        // "full-access" in the HUD, and already permit-all at the policy layer
        // (`check_file_write`) and for bash redirects — `write_file` to a path
        // outside the workspace was still denied "escapes workspace boundary".
        // Allow must relax the boundary exactly like danger-full-access.
        let workspace = temp_dir("allowmode-write-ws");
        std::fs::create_dir_all(&workspace).expect("workspace dir");
        let sibling = temp_dir("allowmode-write-sibling");
        std::fs::create_dir_all(&sibling).expect("sibling dir");
        let outside = sibling.join("bench_TS7498_RUN.sql");
        let enforcer = PermissionEnforcer::new(PermissionPolicy::new(PermissionMode::Allow));
        let result = run_write_file(
            &WriteFileInput {
                path: outside.to_string_lossy().into_owned(),
                content: "-- allowed under full access (allow mode)".to_owned(),
            },
            Some(&enforcer),
            &context(Some(&workspace)),
        );
        assert!(
            result.is_ok(),
            "allow mode must permit an outside-workspace write, got {result:?}"
        );
        assert!(outside.exists(), "the file should have been written");
        let _ = std::fs::remove_dir_all(&workspace);
        let _ = std::fs::remove_dir_all(&sibling);
    }

    #[test]
    fn read_outside_workspace_allowed_in_allow_mode() {
        // Reads mirror writes: Allow mode relaxes the read boundary too.
        let workspace = temp_dir("allowmode-read-ws");
        std::fs::create_dir_all(&workspace).expect("workspace dir");
        let sibling = temp_dir("allowmode-read-sibling");
        std::fs::create_dir_all(&sibling).expect("sibling dir");
        let outside = sibling.join("notes.md");
        std::fs::write(&outside, "readable in allow mode").expect("seed outside file");
        let enforcer = PermissionEnforcer::new(PermissionPolicy::new(PermissionMode::Allow));
        let result = run_read_file(
            &ReadFileInput {
                path: outside.to_string_lossy().into_owned(),
                offset: None,
                limit: None,
            },
            Some(&enforcer),
            None,
            Some(&workspace),
            None,
            &reads(),
        );
        assert!(
            result.is_ok(),
            "allow mode must permit an outside-workspace read, got {result:?}"
        );
        let _ = std::fs::remove_dir_all(&workspace);
        let _ = std::fs::remove_dir_all(&sibling);
    }

    #[test]
    fn capped_read_still_registers_whole_file_snapshot() {
        // A default-capped read returns only the first 2000 lines to the model,
        // but the read-registry must snapshot the WHOLE file from disk so a
        // follow-up edit is not falsely rejected as "modified since read" over
        // the lines beyond the cap (CC parity: a partial Read counts as read).
        let workspace = temp_dir("capped-read-registry-ws");
        std::fs::create_dir_all(&workspace).expect("workspace dir");
        let file = workspace.join("big.txt");
        let mut body = String::new();
        for n in 0..2_500 {
            use std::fmt::Write as _;
            let _ = writeln!(body, "line {n}");
        }
        std::fs::write(&file, &body).expect("seed big file");

        let registry = reads();
        let output = run_read_file(
            &ReadFileInput {
                path: file.to_string_lossy().into_owned(),
                offset: None,
                limit: None,
            },
            None,
            None,
            Some(&workspace),
            None,
            &registry,
        )
        .expect("read should succeed");

        // Model-facing output is capped and annotated...
        assert!(output.contains("showing lines 1-2000 of 2500"));
        assert!(output.contains("pass offset/limit to read more"));
        // ...yet the snapshot covers the full 2500-line file: the file reads as
        // Fresh (a partial snapshot would diverge and report ModifiedSinceRead).
        assert_eq!(
            registry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .check(&file),
            runtime::FileFreshness::Fresh
        );
        let _ = std::fs::remove_dir_all(&workspace);
    }

    #[test]
    fn read_outside_workspace_allowed_in_danger_full_access() {
        // Same relaxation as writes: full access was already allowed to write
        // outside the workspace (and `bash` could `cat` any file), yet
        // `read_file` still refused with "escapes workspace boundary" — pure
        // friction with no protective value in this mode.
        let workspace = temp_dir("fullaccess-read-ws");
        std::fs::create_dir_all(&workspace).expect("workspace dir");
        let sibling = temp_dir("fullaccess-read-sibling");
        std::fs::create_dir_all(&sibling).expect("sibling dir");
        let outside = sibling.join("notes.md");
        std::fs::write(&outside, "readable under full access").expect("seed outside file");
        let enforcer = danger_full_access_enforcer();
        let result = run_read_file(
            &ReadFileInput {
                path: outside.to_string_lossy().into_owned(),
                offset: None,
                limit: None,
            },
            Some(&enforcer),
            None,
            Some(&workspace),
            None,
            &reads(),
        );
        let output = result.expect("danger-full-access must allow an outside read");
        assert!(
            output.contains("readable under full access"),
            "got: {output}"
        );
        let _ = std::fs::remove_dir_all(&workspace);
        let _ = std::fs::remove_dir_all(&sibling);
    }

    #[test]
    fn read_outside_workspace_still_denied_below_full_access() {
        // The security invariant the relaxation must NOT weaken: in
        // workspace-write (and read-only) mode, reads stay confined.
        let workspace = temp_dir("ww-read-ws");
        std::fs::create_dir_all(&workspace).expect("workspace dir");
        let outside = temp_dir("ww-read-target.txt");
        std::fs::write(&outside, "secret").expect("seed outside file");
        let enforcer = workspace_write_enforcer();
        let result = run_read_file(
            &ReadFileInput {
                path: outside.to_string_lossy().into_owned(),
                offset: None,
                limit: None,
            },
            Some(&enforcer),
            None,
            Some(&workspace),
            None,
            &reads(),
        );
        assert!(
            matches!(result, Err(ToolError::PermissionDenied { .. })),
            "workspace-write must keep the read boundary, got {result:?}"
        );
        let _ = std::fs::remove_dir_all(&workspace);
        let _ = std::fs::remove_file(&outside);
    }

    #[test]
    fn grep_search_boundary_matches_read_file_modes() {
        let workspace = temp_dir("grep-boundary-ws");
        std::fs::create_dir_all(&workspace).expect("workspace dir");
        let sibling = temp_dir("grep-boundary-sibling");
        std::fs::create_dir_all(&sibling).expect("sibling dir");
        std::fs::write(workspace.join("inside.txt"), "needle inside").expect("seed inside file");
        std::fs::write(sibling.join("outside.txt"), "needle outside").expect("seed outside file");

        let input = |path: &std::path::Path| GrepSearchInput {
            pattern: "needle".to_owned(),
            path: Some(path.to_string_lossy().into_owned()),
            glob: None,
            output_mode: Some("content".to_owned()),
            before: None,
            after: None,
            context_short: None,
            context: None,
            line_numbers: None,
            case_insensitive: None,
            file_type: None,
            head_limit: None,
            offset: None,
            multiline: None,
        };

        let enforcer = workspace_write_enforcer();
        let denied = run_grep_search(
            &input(&sibling),
            Some(&enforcer),
            None,
            Some(&workspace),
            None,
        );
        assert!(
            matches!(denied, Err(ToolError::PermissionDenied { .. })),
            "workspace-write must reject outside grep_search paths, got {denied:?}"
        );

        let inside = run_grep_search(
            &input(&workspace),
            Some(&enforcer),
            None,
            Some(&workspace),
            None,
        )
        .expect("in-workspace grep_search succeeds");
        assert!(inside.contains("needle inside"), "got: {inside}");

        let full_access = danger_full_access_enforcer();
        let outside = run_grep_search(
            &input(&sibling),
            Some(&full_access),
            None,
            Some(&workspace),
            None,
        )
        .expect("danger-full-access allows outside grep_search");
        assert!(outside.contains("needle outside"), "got: {outside}");

        let _ = std::fs::remove_dir_all(&workspace);
        let _ = std::fs::remove_dir_all(&sibling);
    }

    #[test]
    fn edit_outside_workspace_denied() {
        let workspace = temp_dir("outside-edit-ws");
        std::fs::create_dir_all(&workspace).expect("workspace dir");
        let outside = temp_dir("outside-edit-target.txt");
        std::fs::write(&outside, "alpha").expect("seed outside file");
        let enforcer = workspace_write_enforcer();
        let result = run_edit_file(
            &EditFileInput {
                path: outside.to_string_lossy().into_owned(),
                old_string: "alpha".to_owned(),
                new_string: "omega".to_owned(),
                replace_all: Some(false),
            },
            Some(&enforcer),
            &context(Some(&workspace)),
        );
        assert!(
            matches!(result, Err(ToolError::PermissionDenied { .. })),
            "expected PermissionDenied, got {result:?}"
        );
        // File contents must remain unchanged.
        let contents = std::fs::read_to_string(&outside).expect("read seeded file");
        assert_eq!(contents, "alpha");
    }

    #[test]
    fn traversal_attempt_denied() {
        let workspace = temp_dir("traversal-ws");
        let nested = workspace.join("nested");
        std::fs::create_dir_all(&nested).expect("workspace dir");
        let enforcer = workspace_write_enforcer();
        // `../../../etc/zo-leak.txt` resolves above workspace root.
        let leak_path = "../../../etc/zo-leak-should-not-exist.txt";
        let result = run_write_file(
            &WriteFileInput {
                path: leak_path.to_owned(),
                content: "leaked".to_owned(),
            },
            Some(&enforcer),
            &context(Some(&nested)),
        );
        assert!(
            matches!(result, Err(ToolError::PermissionDenied { .. })),
            "expected PermissionDenied for traversal, got {result:?}"
        );
    }

    #[test]
    fn workspace_root_none_skips_boundary_check() {
        // Backwards compatibility for harness/test environments that do
        // not configure a workspace root.
        let path = temp_dir("no-root-write.txt");
        let result = run_write_file(
            &WriteFileInput {
                path: path.to_string_lossy().into_owned(),
                content: "ok".to_owned(),
            },
            None,
            &context(None),
        );
        assert!(result.is_ok(), "expected ok, got {result:?}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn cwd_resolves_relative_read_without_workspace_root() {
        use super::{run_read_file, ReadFileInput};
        // No workspace root (boundary disabled) but a per-agent cwd is set:
        // a relative read must resolve against that cwd rather than the live
        // process cwd. With `cwd: None` the historical process-cwd behavior is
        // preserved (covered by the other tests, which all pass `None`).
        let dir = temp_dir("cwd-read");
        std::fs::create_dir_all(&dir).expect("cwd dir");
        std::fs::write(dir.join("note.txt"), "hi from cwd").expect("seed file");
        let result = run_read_file(
            &ReadFileInput {
                path: "note.txt".to_owned(),
                offset: None,
                limit: None,
            },
            None,
            None,
            None,
            Some(&dir),
            &reads(),
        )
        .expect("relative read should resolve against cwd");
        assert!(result.contains("hi from cwd"), "got: {result}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod read_guard_tests {
    //! CC 패리티 read-before-edit 가드 시나리오 (브리핑 ①~⑤):
    //! ① read → 외부수정 → edit 거부(재읽기 후 허용)
    //! ② read → edit → 연속 edit 허용(자기 갱신)
    //! ③ read 없이 edit 거부
    //! ④ 신규 파일 write 허용(가드 면제) + 이어지는 edit 허용
    //! ⑤ 서브에이전트/별도 대화는 별도 레지스트리(스코프 격리)

    use super::{
        dispatch, run_edit_file, run_read_file, run_write_file, EditFileInput, ReadFileInput,
        WriteFileInput,
    };
    use crate::error::ToolError;
    use crate::ToolContext;
    use serde_json::json;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_file(name: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("zo-read-guard-{name}-{unique}.txt"))
    }

    fn read(ctx: &ToolContext, path: &std::path::Path) -> Result<String, ToolError> {
        run_read_file(
            &ReadFileInput {
                path: path.to_string_lossy().into_owned(),
                offset: None,
                limit: None,
            },
            None,
            None,
            None,
            None,
            &ctx.file_reads,
        )
    }

    fn edit(
        ctx: &ToolContext,
        path: &std::path::Path,
        old: &str,
        new: &str,
    ) -> Result<String, ToolError> {
        run_edit_file(
            &EditFileInput {
                path: path.to_string_lossy().into_owned(),
                old_string: old.to_owned(),
                new_string: new.to_owned(),
                replace_all: Some(false),
            },
            None,
            ctx,
        )
    }

    fn write(
        ctx: &ToolContext,
        path: &std::path::Path,
        content: &str,
    ) -> Result<String, ToolError> {
        run_write_file(
            &WriteFileInput {
                path: path.to_string_lossy().into_owned(),
                content: content.to_owned(),
            },
            None,
            ctx,
        )
    }

    /// ① 마지막 읽기 이후 외부(사용자/다른 도구) 변경 → edit 거부, 재읽기
    /// 후 최신 내용 기준으로만 편집 가능. 에러는 짧고 actionable해야 하며
    /// 최신 파일 내용을 재주입하지 않는다.
    #[test]
    fn edit_after_external_modification_is_rejected_until_reread() {
        let path = temp_file("external-modify");
        std::fs::write(&path, "v1 alpha\n").expect("seed");
        let ctx = ToolContext::new();
        read(&ctx, &path).expect("initial read");

        // 외부 변경 (길이·내용 모두 상이 — hash 권위 판정).
        std::fs::write(&path, "v2 externally changed alpha\n").expect("external modify");

        let err = edit(&ctx, &path, "alpha", "omega").expect_err("stale edit must be refused");
        let message = err.to_string();
        assert!(
            message.contains("changed on disk") && message.contains("read_file"),
            "error must say what changed and how to recover: {message}"
        );
        assert!(
            !message.contains("externally changed"),
            "error must NOT re-inject the file's new content: {message}"
        );
        // 거부 시 파일은 그대로.
        assert_eq!(
            std::fs::read_to_string(&path).expect("read back"),
            "v2 externally changed alpha\n"
        );

        // 재읽기 후에는 최신 내용 기준으로 편집이 통과한다.
        read(&ctx, &path).expect("re-read after external change");
        edit(&ctx, &path, "alpha", "omega").expect("edit after re-read");
        assert!(
            std::fs::read_to_string(&path)
                .expect("read back")
                .contains("omega")
        );
        let _ = std::fs::remove_file(&path);
    }

    /// ② 자기 자신의 edit은 레지스트리를 갱신하므로 재읽기 없이 연속 edit이
    /// 허용된다.
    #[test]
    fn consecutive_edits_after_one_read_are_allowed() {
        let path = temp_file("consecutive-edits");
        std::fs::write(&path, "one two three\n").expect("seed");
        let ctx = ToolContext::new();
        read(&ctx, &path).expect("read once");

        edit(&ctx, &path, "one", "ONE").expect("first edit");
        edit(&ctx, &path, "two", "TWO").expect("second edit without re-read");
        edit(&ctx, &path, "three", "THREE").expect("third edit without re-read");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read back"),
            "ONE TWO THREE\n"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// ③ 이 대화에서 읽은 적 없는 기존 파일의 edit은 거부 — `read_file`을
    /// 지시하는 actionable 에러.
    #[test]
    fn edit_without_prior_read_is_rejected() {
        let path = temp_file("never-read-edit");
        std::fs::write(&path, "alpha\n").expect("seed");
        let ctx = ToolContext::new();

        let err = edit(&ctx, &path, "alpha", "omega").expect_err("unread edit must be refused");
        let message = err.to_string();
        assert!(
            message.contains("has not been read") && message.contains("read_file"),
            "error must be actionable: {message}"
        );
        assert_eq!(std::fs::read_to_string(&path).expect("read back"), "alpha\n");
        let _ = std::fs::remove_file(&path);
    }

    /// ③-b 기존 파일을 통째로 덮어쓰는 `write_file`도 같은 가드를 받는다.
    #[test]
    fn overwrite_without_prior_read_is_rejected() {
        let path = temp_file("never-read-overwrite");
        std::fs::write(&path, "precious existing content\n").expect("seed");
        let ctx = ToolContext::new();

        let err = write(&ctx, &path, "clobbered").expect_err("unread overwrite must be refused");
        assert!(err.to_string().contains("has not been read"), "{err}");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read back"),
            "precious existing content\n",
            "refused overwrite must leave the file untouched"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// ④ 신규 파일 생성은 면제 — write 성공이 곧 관측이므로 이어지는 edit도
    /// 재읽기 없이 허용된다.
    #[test]
    fn new_file_write_is_exempt_and_seeds_the_registry() {
        let path = temp_file("new-file");
        assert!(!path.exists());
        let ctx = ToolContext::new();

        write(&ctx, &path, "fresh alpha\n").expect("creating a new file needs no prior read");
        edit(&ctx, &path, "alpha", "omega").expect("edit right after own write");
        // 자기 write 뒤의 재-write(덮어쓰기)도 신선 — 연속 작업 허용.
        write(&ctx, &path, "rewritten\n").expect("overwrite after own write");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read back"),
            "rewritten\n"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// ⑤ 레지스트리는 대화(ToolContext) 스코프 — 한 대화의 읽기가 다른
    /// 대화(서브에이전트의 fresh 컨텍스트)의 가드를 통과시키지 않는다.
    /// 디스패치 경로 전체(e2e)로 검증한다.
    #[test]
    fn read_registry_is_scoped_per_conversation_context() {
        let path = temp_file("scope-isolation");
        std::fs::write(&path, "shared alpha\n").expect("seed");
        let path_str = path.to_string_lossy().into_owned();

        let ctx_a = ToolContext::new();
        let ctx_b = ToolContext::new(); // 서브에이전트/별도 대화에 해당

        dispatch(&ctx_a, None, "read_file", &json!({ "path": path_str }))
            .expect("read_file dispatches")
            .expect("conversation A reads the file");

        // A의 읽기는 B의 가드를 만족시키지 못한다.
        let err = dispatch(
            &ctx_b,
            None,
            "edit_file",
            &json!({ "path": path_str, "old_string": "alpha", "new_string": "omega" }),
        )
        .expect("edit_file dispatches")
        .expect_err("conversation B must be told to read first");
        assert!(err.to_string().contains("has not been read"), "{err}");

        // A 자신은 편집 가능.
        dispatch(
            &ctx_a,
            None,
            "edit_file",
            &json!({ "path": path_str, "old_string": "alpha", "new_string": "omega" }),
        )
        .expect("edit_file dispatches")
        .expect("conversation A edits after its own read");
        assert!(
            std::fs::read_to_string(&path)
                .expect("read back")
                .contains("omega")
        );
        let _ = std::fs::remove_file(&path);
    }

    /// 부분 읽기(offset/limit)도 "읽음"으로 등재된다 — CC 패리티.
    #[test]
    fn windowed_read_registers_the_whole_file() {
        let path = temp_file("windowed-read");
        std::fs::write(&path, "l1\nl2\nl3\n").expect("seed");
        let ctx = ToolContext::new();
        run_read_file(
            &ReadFileInput {
                path: path.to_string_lossy().into_owned(),
                offset: Some(1),
                limit: Some(1),
            },
            None,
            None,
            None,
            None,
            &ctx.file_reads,
        )
        .expect("windowed read");
        edit(&ctx, &path, "l3", "L3").expect("edit outside the read window is still allowed");
        let _ = std::fs::remove_file(&path);
    }
}

#[cfg(test)]
mod search_input_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn glob_without_pattern_defaults_to_all_files() {
        let input: GlobSearchInputValue =
            from_value(&json!({"path": "src"})).expect("path-only glob input parses");
        assert_eq!(input.pattern_or_default(), "**/*");
    }

    #[test]
    fn glob_tolerates_extra_fields_models_send() {
        // Real failing shape from session logs: {"path": …, "type": "rs"}.
        // serde ignores unknown fields; only the missing pattern was fatal.
        let input: GlobSearchInputValue =
            from_value(&json!({"path": "src", "type": "rs"})).expect("parses");
        assert_eq!(input.pattern_or_default(), "**/*");
        assert_eq!(input.path.as_deref(), Some("src"));
    }

    #[test]
    fn glob_dispatch_with_path_only_lists_files() {
        // End-to-end through dispatch: this exact call shape used to fail
        // with `missing field \`pattern\`` (the most common tool-call error).
        let dir = std::env::temp_dir().join(format!(
            "zo-glob-default-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(dir.join("hello.txt"), "hi").expect("seed file");

        let ctx = ToolContext::new();
        let result = dispatch(
            &ctx,
            None,
            "glob_search",
            &json!({"path": dir.to_string_lossy()}),
        )
        .expect("glob_search is dispatched")
        .expect("path-only glob must succeed");

        assert!(result.contains("hello.txt"), "{result}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn grep_without_pattern_gets_corrective_error() {
        let ctx = ToolContext::new();
        let error = dispatch(&ctx, None, "grep_search", &json!({"path": "src"}))
            .expect("grep_search is dispatched")
            .expect_err("missing pattern must fail");
        let message = error.to_string();
        assert!(message.contains("`pattern`"), "{message}");
        assert!(
            message.contains("glob_search"),
            "correction must point at the listing tool: {message}"
        );
    }
}
