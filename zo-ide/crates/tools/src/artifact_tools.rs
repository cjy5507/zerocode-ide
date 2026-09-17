//! Publish audience-facing HTML through the pane's window or the shared headless format.
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use runtime::permission_enforcer::PermissionEnforcer;
use runtime::PermissionMode;
use serde::Deserialize;
use serde_json::{json, Value};
use zerocode_core::artifact_publish::{self as publication, ExportInput, PublishInput, ARTIFACT_MAX_BYTES, ARTIFACT_SHIM_TIMEOUT_SECS, SHIM};

use crate::{from_value, maybe_enforce_permission_check, to_pretty_json, ToolContext, ToolError, ToolSpec};

pub(crate) const DESIGN_SKILLS: &[&str] = &["artifact-design", "dataviz", "artifact-diagramming"];

const ENDPOINT_ENV: &str = "ZEROCODE_HOOK_ENDPOINT";
const OUTPUT_CAP: u64 = ARTIFACT_MAX_BYTES * 2;

pub(crate) fn tool_specs() -> Vec<ToolSpec> {
    vec![ToolSpec {
        name: "Artifact",
        description: "Publish self-contained HTML; list/read pages; export a version to a new file. Before publish, read artifact-design, dataviz or artifact-diagramming with Skill this turn. publish needs file_path; read needs id; export needs id and out (optional version, latest by default). Republishing keeps the id and local URL. Export copies one file; it does not host it or bundle external assets.",
        input_schema: json!({
            "type": "object", "properties": {
                "action": {"type": "string", "enum": ["publish", "list", "read", "export"]},
                "file_path": {"type": "string"}, "title": {"type": "string"},
                "description": {"type": "string"}, "favicon": {"type": "string"},
                "label": {"type": "string"}, "id": {"type": "string"},
                "out": {"type": "string"}, "version": {"type": "integer", "minimum": 1}
            }, "required": ["action"], "additionalProperties": false
        }),
        required_permission: PermissionMode::WorkspaceWrite,
    }]
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ArtifactInput {
    action: String,
    file_path: Option<PathBuf>,
    title: Option<String>,
    description: Option<String>,
    favicon: Option<String>,
    label: Option<String>,
    id: Option<String>,
    out: Option<PathBuf>,
    version: Option<u32>,
}

fn prepare(input: &Value, ctx: &ToolContext, enforcer: Option<&PermissionEnforcer>) -> Result<Value, ToolError> {
    let parsed: ArtifactInput = from_value(input)?;
    match parsed.action.as_str() {
        "publish" => {
            let path = parsed.file_path.ok_or_else(|| ToolError::InvalidInput("publish needs file_path".into()))?;
            let path = destination(path, ctx, enforcer)?;
            let request = PublishInput { file_path: path, title: parsed.title, description: parsed.description,
                favicon: parsed.favicon, label: parsed.label };
            publication::validate(&request).map_err(ToolError::InvalidInput)?;
            let mut value = serde_json::to_value(request)?;
            value["action"] = json!("publish");
            Ok(value)
        }
        "export" => {
            let out = parsed.out.ok_or_else(|| ToolError::InvalidInput("export needs out".into()))?;
            let request = ExportInput {
                id: parsed.id.ok_or_else(|| ToolError::InvalidInput("export needs id".into()))?,
                version: parsed.version,
                out: destination(out, ctx, enforcer)?,
            };
            publication::validate_export(&request).map_err(ToolError::InvalidInput)?;
            let mut value = serde_json::to_value(request)?;
            value["action"] = json!("export");
            Ok(value)
        }
        "read" if parsed.id.as_ref().is_some_and(|id| !id.is_empty()) => Ok(json!({"action":"read", "id":parsed.id})),
        "list" => Ok(json!({"action":"list"})),
        _ => Err(ToolError::InvalidInput("Artifact needs publish, list, read with id, or export with id and out".into())),
    }
}

fn destination(path: PathBuf, ctx: &ToolContext, enforcer: Option<&PermissionEnforcer>) -> Result<PathBuf, ToolError> {
    let path = if path.is_absolute() { path } else {
        let cwd = ctx.cwd.clone().or_else(|| ctx.workspace_root.clone()).map_or_else(std::env::current_dir, Ok)?;
        cwd.join(path)
    };
    Ok(crate::file_tools::enforce_workspace_boundary(enforcer, ctx.session_permission_mode(),
        &path.to_string_lossy(), ctx.workspace_root.as_deref())?.unwrap_or(path))
}

pub(crate) fn dispatch(ctx: &ToolContext, enforcer: Option<&PermissionEnforcer>, name: &str, input: &Value) -> Option<Result<String, ToolError>> {
    if name != "Artifact" { return None; }
    Some((|| {
        maybe_enforce_permission_check(enforcer, name, input)?;
        if input.get("action").and_then(Value::as_str) == Some("publish") && !ctx.artifact_design_read() {
            return Err(ToolError::InvalidInput("Before Artifact publish, read artifact-design (or dataviz/artifact-diagramming) with Skill in this turn.".into()));
        }
        let request = prepare(input, ctx, enforcer)?;
        let result = if std::env::var_os(ENDPOINT_ENV).is_some_and(|v| !v.is_empty()) {
            let program = shim_on_path().ok_or_else(|| ToolError::Execution(format!("{SHIM} is missing from this pane's PATH")))?;
            call_shim(&program, &request, ctx.cwd.as_deref())?
        } else {
            let root = core_types::paths::default_config_home().join("artifacts");
            headless(&root, &request)?
        };
        to_pretty_json(&result)
    })())
}

fn shim_on_path() -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path).map(|dir| dir.join(if cfg!(windows) { "zerocode-artifact.cmd" } else { SHIM }))
        .find(|file| file.is_file())
}

fn argv(request: &Value) -> Vec<String> {
    let mut words = vec![request["action"].as_str().unwrap_or_default().to_owned()];
    for (key, flag) in [("file_path", "--file-path"), ("title", "--title"), ("description", "--description"),
        ("favicon", "--favicon"), ("label", "--label"), ("id", "--id"), ("out", "--out")] {
        if let Some(value) = request[key].as_str() { words.extend([flag.to_owned(), value.to_owned()]); }
    }
    if let Some(version) = request["version"].as_u64() {
        words.extend(["--version".into(), version.to_string()]);
    }
    words
}

fn call_shim(program: &Path, request: &Value, cwd: Option<&Path>) -> Result<Value, ToolError> {
    crate::http_bridge::run_http(async {
        use tokio::io::AsyncReadExt as _;
        let mut command = tokio::process::Command::new(program);
        command.args(argv(request)).kill_on_drop(true).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        if let Some(cwd) = cwd { command.current_dir(cwd); }
        let mut child = command.spawn()?;
        let stdout = child.stdout.take().ok_or_else(|| ToolError::Execution("shim stdout missing".into()))?;
        let stderr = child.stderr.take().ok_or_else(|| ToolError::Execution("shim stderr missing".into()))?;
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut stdout = stdout.take(OUTPUT_CAP + 1);
        let mut stderr = stderr.take(OUTPUT_CAP + 1);
        let result = tokio::time::timeout(Duration::from_secs(ARTIFACT_SHIM_TIMEOUT_SECS), async {
            tokio::join!(child.wait(), stdout.read_to_end(&mut out), stderr.read_to_end(&mut err))
        }).await.map_err(|_| ToolError::Execution(format!("{SHIM} timed out")))?;
        let status = result.0?;
        result.1?;
        result.2?;
        if out.len() as u64 > OUTPUT_CAP || err.len() as u64 > OUTPUT_CAP { return Err(ToolError::Execution("artifact shim output exceeds limit".into())); }
        if !status.success() { return Err(ToolError::Execution(String::from_utf8_lossy(&err).into_owned())); }
        serde_json::from_slice(&out).map_err(ToolError::from)
    })
}

fn headless(root: &Path, request: &Value) -> Result<Value, ToolError> {
    let answer = (|| -> Result<Value, String> {
        match request["action"].as_str().unwrap_or_default() {
            "publish" => {
                let mut input = request.clone();
                input.as_object_mut().ok_or("expected object")?.remove("action");
                let input = serde_json::from_value(input).map_err(|e| e.to_string())?;
                serde_json::to_value(publication::publish_headless(root, &input)?).map_err(|e| e.to_string())
            }
            "export" => {
                let mut input = request.clone();
                input.as_object_mut().ok_or("expected object")?.remove("action");
                let input = serde_json::from_value(input).map_err(|e| e.to_string())?;
                serde_json::to_value(publication::export(root, &input)?).map_err(|e| e.to_string())
            }
            "list" => {
                let mut rows: Vec<_> = publication::catalog(root)?.into_values().collect();
                rows.sort_by_key(|row| std::cmp::Reverse(row.modified_ms));
                let total = rows.len();
                rows.truncate(zerocode_core::artifact::Limits::default().list_rows_max);
                Ok(json!({"truncated": rows.len() < total, "rows":rows, "total":total}))
            }
            "read" => {
                let id = request["id"].as_str().ok_or("read needs id")?;
                let meta = publication::read_meta(root, id)?;
                let artifact = meta.artifact(zerocode_core::artifact::Origin::default());
                let html = publication::read_page(root, &artifact)?;
                Ok(json!({"artifact":artifact, "html":html}))
            }
            _ => Err("unknown Artifact action".into()),
        }
    })();
    answer.map_err(ToolError::Execution)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_export_keeps_selected_version_bytes_and_refuses_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.html");
        let root = dir.path().join("artifacts");
        let out = dir.path().join("export # 한.html");
        std::fs::write(&source, "<main>first</main>").unwrap();
        let request = json!({"action":"publish", "file_path":source});
        let first = headless(&root, &request).unwrap();
        std::fs::write(&source, "<main>second</main>").unwrap();
        headless(&root, &request).unwrap();
        std::fs::remove_file(&source).unwrap();
        let export = json!({"action":"export", "id":first["id"], "version":1, "out":out});
        let answer = headless(&root, &export).unwrap();
        assert_eq!(answer["version"], 1);
        let expected = std::fs::read(root.join("pages").join(first["id"].as_str().unwrap()).join("v1/index.html")).unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), expected);
        assert!(headless(&root, &export).is_err());
        assert_eq!(std::fs::read(&out).unwrap(), expected);
        let latest = dir.path().join("latest.html");
        let answer = headless(&root, &json!({"action":"export", "id":first["id"], "out":latest})).unwrap();
        assert_eq!(answer["version"], 2);
        assert!(std::fs::read_to_string(latest).unwrap().contains("second"));
        let absent = dir.path().join("absent.html");
        assert!(headless(&root, &json!({"action":"export", "id":first["id"], "version":99, "out":absent})).is_err());
        assert!(!absent.exists());
    }

    #[test]
    fn artifact_export_prepares_absolute_destination_and_shim_arguments() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = ToolContext::new();
        ctx.cwd = Some(dir.path().to_path_buf());
        let prepared = prepare(&json!({"action":"export", "id":"p-test", "version":2, "out":"share # 한.html"}), &ctx, None).unwrap();
        assert_eq!(prepared["out"], json!(dir.path().join("share # 한.html")));
        let words = argv(&prepared);
        let decoded = publication::request_from_argv(&words).unwrap();
        assert_eq!(decoded, prepared);
        for request in [
            json!({"action":"export", "id":"p-test"}),
            json!({"action":"export", "out":"page.html"}),
            json!({"action":"export", "id":"../bad", "out":"page.html"}),
            json!({"action":"export", "id":"p-test", "version":0, "out":"page.html"}),
        ] { assert!(prepare(&request, &ctx, None).is_err(), "{request}"); }
    }

    #[test]
    fn artifact_publish_requires_a_successful_design_skill_in_this_turn() {
        let ctx = ToolContext::new();
        let input = json!({"action":"publish", "file_path":"unused.html"});
        let refused = dispatch(&ctx, None, "Artifact", &input).unwrap().unwrap_err().to_string();
        assert!(refused.contains("Skill") && refused.contains("artifact-design"), "{refused}");
        assert!(crate::misc_tools::dispatch(&ctx, None, "Skill", &json!({"skill":"t3465-nonexistent-skill"})).unwrap().is_err());
        assert!(!ctx.artifact_design_read());
        crate::misc_tools::dispatch(&ctx, None, "Skill", &json!({"skill":"artifact-design"})).unwrap().unwrap();
        assert!(ctx.artifact_design_read());
        let independent = ToolContext::new();
        assert!(!independent.artifact_design_read());
        ctx.begin_skill_turn();
        assert!(!ctx.artifact_design_read());
        ctx.note_artifact_skill_read(Path::new("/skills/unrelated/SKILL.md"));
        assert!(!ctx.artifact_design_read());
        ctx.note_artifact_skill_read(Path::new("/skills/dataviz/SKILL.md"));
        assert!(ctx.artifact_design_read());
    }

    #[test]
    fn artifact_is_deferred_and_discoverable() {
        let registry = crate::GlobalToolRegistry::builtin();
        assert!(!registry.definitions(None).iter().any(|tool| tool.name == "Artifact"));
        assert!(crate::deferred_tool_manifest_section().contains("Artifact ("));
    }

    #[test]
    fn artifact_headless_publish_list_read_and_republish_share_one_catalog() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("a.html");
        std::fs::write(&source, "<main>hello</main>").unwrap();
        let root = dir.path().join("artifacts");
        let request = json!({"action":"publish", "file_path":source,"title":"Hello World","favicon":"🌿"});
        let one = headless(&root, &request).unwrap();
        let two = headless(&root, &request).unwrap();
        assert_eq!(one["url"], two["url"]);
        assert_eq!(two["version"], 2);
        let rows = headless(&root, &json!({"action":"list"})).unwrap();
        assert_eq!(rows["total"], 1);
        assert_eq!(rows["rows"][0]["version"], 2);
        let read = headless(&root, &json!({"action":"read", "id":one["id"]})).unwrap();
        assert!(read["html"].as_str().unwrap().contains("hello"));
    }

    #[cfg(unix)]
    #[test]
    fn artifact_the_shim_road_is_answered_by_the_window_and_preserves_arguments() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let shim = dir.path().join(SHIM);
        std::fs::write(&shim, "#!/bin/sh\n[ \"$1\" = publish ] && [ \"$2\" = --file-path ] && [ \"$3\" = '/tmp/a # 한.html' ] || exit 2\nprintf '%s' '{\"id\":\"page\",\"version\":4,\"url\":\"file:///window/index.html\"}'\n").unwrap();
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
        let answer = call_shim(&shim, &json!({"action":"publish","file_path":"/tmp/a # 한.html"}), None).unwrap();
        assert_eq!(answer["version"], 4);
        std::fs::write(&shim, "#!/bin/sh\necho 'window refused' >&2\nexit 1\n").unwrap();
        assert!(call_shim(&shim, &json!({"action":"list"}), None).unwrap_err().to_string().contains("window refused"));
    }
}
