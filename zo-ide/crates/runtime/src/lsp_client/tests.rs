use std::collections::BTreeMap;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::mcp_client::McpStdioTransport;

use super::*;

fn mark_test_script_executable(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        let mut permissions = fs::metadata(path).expect("metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).expect("chmod");
    }
    #[cfg(windows)]
    {
        // Windows has no executable permission bit: launchability comes from
        // the executable/script host rather than a 0755 mode. Still assert the
        // test artifact is a regular file instead of silently skipping setup.
        assert!(fs::metadata(path).expect("metadata").is_file());
    }
}

#[test]
fn registers_and_retrieves_server() {
    let registry = LspRegistry::new();
    registry.register(
        "rust",
        LspServerStatus::Connected,
        Some("/workspace"),
        vec!["hover".into(), "completion".into()],
    );

    let server = registry.get("rust").expect("should exist");
    assert_eq!(server.language, "rust");
    assert_eq!(server.status, LspServerStatus::Connected);
    assert_eq!(server.capabilities.len(), 2);
}

#[test]
fn finds_server_by_file_extension() {
    let registry = LspRegistry::new();
    registry.register("rust", LspServerStatus::Connected, None, vec![]);
    registry.register("typescript", LspServerStatus::Connected, None, vec![]);

    let rs_server = registry.find_server_for_path("src/main.rs").unwrap();
    assert_eq!(rs_server.language, "rust");

    let ts_server = registry.find_server_for_path("src/index.ts").unwrap();
    assert_eq!(ts_server.language, "typescript");

    assert!(registry.find_server_for_path("data.csv").is_none());
}

#[tokio::test]
async fn dispatches_diagnostics_action() {
    let registry = LspRegistry::new();
    registry.register("rust", LspServerStatus::Connected, None, vec![]);
    registry
        .add_diagnostics(
            "rust",
            vec![LspDiagnostic {
                path: "src/lib.rs".into(),
                line: 1,
                character: 0,
                severity: "warning".into(),
                message: "unused import".into(),
                source: None,
            }],
        )
        .unwrap();

    let result = registry
        .dispatch("diagnostics", Some("src/lib.rs"), None, None, None)
        .await
        .unwrap();
    assert_eq!(result["count"], 1);
}

#[test]
fn diagnostics_path_index_tracks_add_and_clear() {
    let registry = LspRegistry::new();
    registry.register("rust", LspServerStatus::Connected, None, vec![]);
    registry
        .add_diagnostics(
            "rust",
            vec![
                LspDiagnostic {
                    path: "src/lib.rs".into(),
                    line: 1,
                    character: 0,
                    severity: "warning".into(),
                    message: "unused import".into(),
                    source: None,
                },
                LspDiagnostic {
                    path: "src/main.rs".into(),
                    line: 2,
                    character: 4,
                    severity: "error".into(),
                    message: "missing item".into(),
                    source: None,
                },
            ],
        )
        .unwrap();

    assert_eq!(registry.get_diagnostics("src/lib.rs").len(), 1);
    assert_eq!(registry.get_diagnostics("src/main.rs").len(), 1);
    let server = registry.get("rust").expect("registered server");
    assert_eq!(server.diagnostics.len(), 2);

    registry.clear_diagnostics("rust").unwrap();
    let server = registry.get("rust").expect("registered server");
    assert!(server.diagnostics.is_empty());
    assert!(registry.get_diagnostics("src/lib.rs").is_empty());
}

#[test]
fn diagnostics_path_index_is_removed_on_disconnect() {
    let registry = LspRegistry::new();
    registry.register("rust", LspServerStatus::Connected, None, vec![]);
    registry
        .add_diagnostics(
            "rust",
            vec![LspDiagnostic {
                path: "src/lib.rs".into(),
                line: 1,
                character: 0,
                severity: "warning".into(),
                message: "unused import".into(),
                source: None,
            }],
        )
        .unwrap();
    assert_eq!(registry.get_diagnostics("src/lib.rs").len(), 1);

    let _ = registry.disconnect("rust");

    assert!(registry.get_diagnostics("src/lib.rs").is_empty());
}

#[tokio::test]
async fn diagnostics_are_bounded_per_server() {
    let registry = LspRegistry::new();
    registry.register("rust", LspServerStatus::Connected, None, vec![]);
    let diagnostics: Vec<_> = (0..(MAX_DIAGNOSTICS_PER_SERVER + 3))
        .map(|index| LspDiagnostic {
            path: format!("src/file_{index}.rs"),
            line: u32::try_from(index).expect("diagnostic index fits in u32"),
            character: 0,
            severity: "warning".into(),
            message: format!("diag {index}"),
            source: None,
        })
        .collect();
    registry.add_diagnostics("rust", diagnostics).unwrap();

    let result = registry
        .dispatch("diagnostics", None, None, None, None)
        .await
        .unwrap();

    assert_eq!(result["count"], MAX_DIAGNOSTICS_PER_SERVER);
    assert!(
        registry.get_diagnostics("src/file_0.rs").is_empty(),
        "oldest diagnostics should be evicted first"
    );
}

#[derive(Debug)]
struct MockLspTransport {
    response: JsonValue,
}

impl LspTransport for MockLspTransport {
    fn dispatch(
        &self,
        _action: LspAction,
        _path: &str,
        _line: u32,
        _character: u32,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<JsonValue, String>> + Send>>
    {
        let res = Ok(self.response.clone());
        Box::pin(async move { res })
    }
}

#[derive(Debug)]
struct PendingLspTransport;

impl LspTransport for PendingLspTransport {
    fn dispatch(
        &self,
        _action: LspAction,
        _path: &str,
        _line: u32,
        _character: u32,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<JsonValue, String>> + Send>>
    {
        Box::pin(async move { pending::<Result<JsonValue, String>>().await })
    }
}

#[tokio::test]
async fn dispatches_to_transport() {
    let registry = LspRegistry::new();
    let transport = Arc::new(MockLspTransport {
        response: serde_json::json!({"hover": "mocked"}),
    });
    registry.register_with_transport(
        "rust",
        LspServerStatus::Connected,
        None,
        vec!["hover".into()],
        Some(transport),
    );

    let result = registry
        .dispatch("hover", Some("test.rs"), Some(10), Some(5), None)
        .await
        .unwrap();
    assert_eq!(result["hover"], "mocked");
}

fn temp_dir(label: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("zo-lsp-{label}-{unique}"))
}

fn write_lsp_server_script() -> PathBuf {
    let root = temp_dir("stdio-server");
    fs::create_dir_all(&root).expect("temp dir");
    let script_path = root.join("fake-lsp-server.py");
    let script = [
        "#!/usr/bin/env python3",
        "import json, sys",
        "",
        "def read_message():",
        "    header = b''",
        r"    while not header.endswith(b'\r\n\r\n'):",
        "        chunk = sys.stdin.buffer.read(1)",
        "        if not chunk:",
        "            return None",
        "        header += chunk",
        "    length = 0",
        r"    for line in header.decode().split('\r\n'):",
        r"        if line.lower().startswith('content-length:'):",
        r"            length = int(line.split(':', 1)[1].strip())",
        "    payload = sys.stdin.buffer.read(length)",
        "    return json.loads(payload.decode())",
        "",
        "def send_message(message):",
        "    payload = json.dumps(message).encode()",
        r"    sys.stdout.buffer.write(f'Content-Length: {len(payload)}\r\n\r\n'.encode() + payload)",
        "    sys.stdout.buffer.flush()",
        "",
        "while True:",
        "    request = read_message()",
        "    if request is None:",
        "        break",
        "    method = request['method']",
        "    if 'id' not in request:",
        "        continue",
        "    if method == 'textDocument/hover':",
        "        send_message({",
        "            'jsonrpc': '2.0',",
        "            'id': request['id'],",
        "            'result': {'contents': {'kind': 'markdown', 'value': 'hover docs'}}",
        "        })",
        "    elif method == 'textDocument/definition':",
        "        send_message({",
        "            'jsonrpc': '2.0',",
        "            'id': request['id'],",
        "            'result': [{'uri': request['params']['textDocument']['uri'], 'range': {'start': {'line': 9, 'character': 1}, 'end': {'line': 9, 'character': 7}}}]",
        "        })",
        "    elif method == 'textDocument/references':",
        "        send_message({",
        "            'jsonrpc': '2.0',",
        "            'id': request['id'],",
        "            'result': [{'uri': request['params']['textDocument']['uri'], 'range': {'start': {'line': 12, 'character': 0}, 'end': {'line': 12, 'character': 3}}}]",
        "        })",
        "    elif method == 'textDocument/documentSymbol':",
        "        send_message({",
        "            'jsonrpc': '2.0',",
        "            'id': request['id'],",
        "            'result': [{'name': 'Widget', 'kind': 5, 'range': {'start': {'line': 2, 'character': 0}, 'end': {'line': 6, 'character': 0}}, 'selectionRange': {'start': {'line': 2, 'character': 0}, 'end': {'line': 2, 'character': 6}}}]",
        "        })",
        "    else:",
        "        send_message({",
        "            'jsonrpc': '2.0',",
        "            'id': request['id'],",
        "            'error': {'code': -32601, 'message': f'unsupported method: {method}'}",
        "        })",
        "",
    ]
    .join("\n");
    fs::write(&script_path, script).expect("write script");
    mark_test_script_executable(&script_path);
    script_path
}

#[tokio::test]
async fn dispatches_supported_roundtrips_for_non_diagnostic_actions() {
    let registry = LspRegistry::new();
    let cases = [
        (
            "hover",
            vec!["hover".to_string()],
            serde_json::json!({"hover": "docs"}),
            "hover",
        ),
        (
            "definition",
            vec!["goto_definition".to_string()],
            serde_json::json!({"definition": [{"path": "src/lib.rs", "line": 9}]}),
            "definition",
        ),
        (
            "references",
            vec!["find_references".to_string()],
            serde_json::json!({"references": [{"path": "src/lib.rs", "line": 12}]}),
            "references",
        ),
        (
            "symbols",
            vec!["document_symbols".to_string()],
            serde_json::json!({"symbols": [{"name": "Widget"}]}),
            "symbols",
        ),
    ];

    for (language, (action, capabilities, response, field)) in [
        ("rust", &cases[0]),
        ("python", &cases[1]),
        ("typescript", &cases[2]),
        ("javascript", &cases[3]),
    ] {
        registry.register_with_transport(
            language,
            LspServerStatus::Connected,
            None,
            capabilities.clone(),
            Some(Arc::new(MockLspTransport {
                response: response.clone(),
            })),
        );

        let path = match language {
            "rust" => "src/main.rs",
            "python" => "src/main.py",
            "typescript" => "src/main.ts",
            "javascript" => "src/main.js",
            _ => unreachable!("known language"),
        };

        let result = registry
            .dispatch(action, Some(path), Some(10), Some(2), None)
            .await
            .unwrap();
        assert_eq!(result[field], response[field]);
    }
}

#[tokio::test]
async fn stdio_transport_round_trips_real_jsonrpc_requests() {
    let script_path = write_lsp_server_script();
    let transport = LspStdioTransport::spawn(&McpStdioTransport {
        command: script_path.display().to_string(),
        args: Vec::new(),
        env: BTreeMap::new(),
        tool_call_timeout_ms: None,
    })
    .expect("spawn stdio transport");

    let registry = LspRegistry::new();
    registry.register_with_transport(
        "rust",
        LspServerStatus::Connected,
        None,
        vec![
            "hover".into(),
            "definition".into(),
            "references".into(),
            "symbols".into(),
        ],
        Some(Arc::new(transport.clone())),
    );

    // Real LSP shapes are normalized into the model-facing structs: hover
    // contents flattened to `content`, locations re-relativized + 1-indexed.
    let hover = registry
        .dispatch("hover", Some("src/main.rs"), Some(10), Some(2), None)
        .await
        .expect("hover roundtrip");
    assert_eq!(hover["content"], "hover docs");
    assert_eq!(hover["language"], "markdown");

    let definition = registry
        .dispatch("definition", Some("src/main.rs"), Some(10), Some(2), None)
        .await
        .expect("definition roundtrip");
    // Server reported 0-indexed line 9; normalized output is 1-indexed (10).
    assert_eq!(definition[0]["line"], 10);

    let references = registry
        .dispatch("references", Some("src/main.rs"), Some(10), Some(2), None)
        .await
        .expect("references roundtrip");
    assert_eq!(references[0]["line"], 13);

    let symbols = registry
        .dispatch("symbols", Some("src/main.rs"), None, None, None)
        .await
        .expect("symbols roundtrip");
    assert_eq!(symbols[0]["name"], "Widget");
    assert_eq!(symbols[0]["kind"], "class");

    transport.terminate().await.expect("terminate transport");
}

#[tokio::test]
async fn stdio_transport_bounds_open_document_bookkeeping() {
    let script_path = write_lsp_server_script();
    let transport = LspStdioTransport::spawn(&McpStdioTransport {
        command: script_path.display().to_string(),
        args: Vec::new(),
        env: BTreeMap::new(),
        tool_call_timeout_ms: None,
    })
    .expect("spawn stdio transport");

    for index in 0..(MAX_OPENED_DOCUMENTS_PER_TRANSPORT + 2) {
        transport
            .dispatch(LspAction::Hover, &format!("src/generated_{index}.rs"), 0, 0)
            .await
            .expect("hover should dispatch");
    }

    assert_eq!(
        transport.opened_document_count(),
        MAX_OPENED_DOCUMENTS_PER_TRANSPORT
    );
    transport.terminate().await.expect("terminate transport");
}

/// Mock LSP server that answers `initialize`, and—once the client opens a
/// document via `textDocument/didOpen`—pushes a `publishDiagnostics`
/// notification for that document's uri. Hover requests get a stub reply
/// so the test can use hover to drive the lazy didOpen.
fn write_diagnostics_lsp_server_script() -> PathBuf {
    let root = temp_dir("diag-server");
    fs::create_dir_all(&root).expect("temp dir");
    let script_path = root.join("fake-lsp-diag.py");
    let script = [
        "#!/usr/bin/env python3",
        "import json, sys",
        "",
        "def read_message():",
        "    header = b''",
        r"    while not header.endswith(b'\r\n\r\n'):",
        "        chunk = sys.stdin.buffer.read(1)",
        "        if not chunk:",
        "            return None",
        "        header += chunk",
        "    length = 0",
        r"    for line in header.decode().split('\r\n'):",
        r"        if line.lower().startswith('content-length:'):",
        r"            length = int(line.split(':', 1)[1].strip())",
        "    payload = sys.stdin.buffer.read(length)",
        "    return json.loads(payload.decode())",
        "",
        "def send_message(message):",
        "    payload = json.dumps(message).encode()",
        r"    sys.stdout.buffer.write(f'Content-Length: {len(payload)}\r\n\r\n'.encode() + payload)",
        "    sys.stdout.buffer.flush()",
        "",
        "while True:",
        "    request = read_message()",
        "    if request is None:",
        "        break",
        "    method = request['method']",
        "    if method == 'initialize':",
        "        send_message({",
        "            'jsonrpc': '2.0',",
        "            'id': request['id'],",
        "            'result': {'capabilities': {}, 'serverInfo': {'name': 'fake-diag', 'version': '0.1.0'}}",
        "        })",
        "    elif method == 'textDocument/didOpen':",
        "        uri = request['params']['textDocument']['uri']",
        "        send_message({",
        "            'jsonrpc': '2.0',",
        "            'method': 'textDocument/publishDiagnostics',",
        "            'params': {",
        "                'uri': uri,",
        "                'diagnostics': [",
        "                    {",
        "                        'range': {'start': {'line': 3, 'character': 5}, 'end': {'line': 3, 'character': 9}},",
        "                        'severity': 1,",
        "                        'message': 'cannot find value `foo`',",
        "                        'source': 'rustc'",
        "                    }",
        "                ]",
        "            }",
        "        })",
        "    elif method == 'textDocument/didChange':",
        "        uri = request['params']['textDocument']['uri']",
        "        send_message({",
        "            'jsonrpc': '2.0',",
        "            'method': 'textDocument/publishDiagnostics',",
        "            'params': {",
        "                'uri': uri,",
        "                'diagnostics': [",
        "                    {",
        "                        'range': {'start': {'line': 1, 'character': 2}, 'end': {'line': 1, 'character': 6}},",
        "                        'severity': 2,",
        "                        'message': 'changed document diagnostic',",
        "                        'source': 'fake-lsp'",
        "                    }",
        "                ]",
        "            }",
        "        })",
        "    elif 'id' in request:",
        "        send_message({",
        "            'jsonrpc': '2.0',",
        "            'id': request['id'],",
        "            'result': {'contents': {'kind': 'markdown', 'value': 'stub'}}",
        "        })",
        "",
    ]
    .join("\n");
    fs::write(&script_path, script).expect("write script");
    mark_test_script_executable(&script_path);
    script_path
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reader_task_publishes_diagnostics_into_registry() {
    let script_path = write_diagnostics_lsp_server_script();

    // spawn_initialized drives initialize + `initialized`, then the
    // reader task is live to receive notifications.
    let transport = LspStdioTransport::spawn_initialized(
        McpStdioTransport {
            command: script_path.display().to_string(),
            args: Vec::new(),
            env: BTreeMap::new(),
            tool_call_timeout_ms: None,
        },
        None,
    )
    .await
    .expect("spawn + initialize diagnostics server");

    let registry = LspRegistry::new();
    registry.register_with_transport(
        "rust",
        LspServerStatus::Connected,
        None,
        vec!["hover".into()],
        Some(Arc::new(transport.clone())),
    );

    let rel_path = "src/main.rs";
    // The diagnostics cache starts empty; the slash command would show 0.
    assert!(registry.get_diagnostics(rel_path).is_empty());

    // A non-diagnostic dispatch triggers the lazy `didOpen`, which makes
    // the server push publishDiagnostics for that uri.
    registry
        .dispatch("hover", Some(rel_path), Some(0), Some(0), None)
        .await
        .expect("hover dispatch should open the document");

    // Reader task processes the pushed notification asynchronously; poll
    // briefly until the registry reflects it.
    let mut diagnostics = Vec::new();
    for _ in 0..50 {
        diagnostics = registry.get_diagnostics(rel_path);
        if !diagnostics.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    assert!(
        !diagnostics.is_empty(),
        "reader task should have populated diagnostics for {rel_path}"
    );
    let diag = &diagnostics[0];
    assert_eq!(diag.line, 3);
    assert_eq!(diag.character, 5);
    assert_eq!(diag.severity, "error");
    assert_eq!(diag.message, "cannot find value `foo`");
    assert_eq!(diag.source.as_deref(), Some("rustc"));

    // The diagnostics dispatch action reads the same cache (count > 0).
    let result = registry
        .dispatch("diagnostics", Some(rel_path), None, None, None)
        .await
        .expect("diagnostics dispatch");
    assert_eq!(result["count"], 1);

    transport.terminate().await.expect("terminate transport");
    fs::remove_dir_all(script_path.parent().expect("script parent")).ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sync_and_collect_diagnostics_returns_did_change_diagnostics() {
    let script_path = write_diagnostics_lsp_server_script();
    let transport = LspStdioTransport::spawn_initialized(
        McpStdioTransport {
            command: script_path.display().to_string(),
            args: Vec::new(),
            env: BTreeMap::new(),
            tool_call_timeout_ms: None,
        },
        None,
    )
    .await
    .expect("spawn + initialize diagnostics server");

    let registry = LspRegistry::new();
    registry.register_with_transport(
        "rust",
        LspServerStatus::Connected,
        None,
        vec!["hover".into()],
        Some(Arc::new(transport.clone())),
    );

    let rel_path = "src/main.rs";
    let diagnostics = registry.sync_and_collect_diagnostics(
        rel_path,
        "fn main() { let changed = true; }",
        Duration::from_secs(2),
    );

    assert!(
        !diagnostics.is_empty(),
        "didChange should publish diagnostics for {rel_path}"
    );
    assert_eq!(diagnostics[0].message, "changed document diagnostic");

    transport.terminate().await.expect("terminate transport");
    fs::remove_dir_all(script_path.parent().expect("script parent")).ok();
}

#[tokio::test]
async fn references_normalize_to_relative_path_and_one_indexed_position() {
    // Build a `file://` URI under the current working directory so that
    // `uri_to_path` re-relativizes it back to a workspace-relative path.
    let cwd = std::env::current_dir().expect("cwd");
    let canonical = cwd.canonicalize().unwrap_or(cwd);
    let abs = canonical.join("src/widget.rs");
    let uri = format!("file://{}", abs.to_string_lossy());

    let registry = LspRegistry::new();
    registry.register_with_transport(
        "rust",
        LspServerStatus::Connected,
        None,
        vec!["references".into()],
        Some(Arc::new(MockLspTransport {
            // Real LSP `Location[]` shape with 0-indexed positions.
            response: serde_json::json!([{
                "uri": uri,
                "range": {
                    "start": { "line": 11, "character": 4 },
                    "end": { "line": 11, "character": 10 }
                }
            }]),
        })),
    );

    let result = registry
        .dispatch("references", Some("src/widget.rs"), Some(0), Some(0), None)
        .await
        .expect("references dispatch");

    let first = &result[0];
    // Path re-relativized off the absolute `file://` URI.
    assert_eq!(first["path"], "src/widget.rs");
    // 0-indexed line 11 / character 4 become 1-indexed 12 / 5.
    assert_eq!(first["line"], 12);
    assert_eq!(first["character"], 5);
    assert_eq!(first["end_line"], 12);
    assert_eq!(first["end_character"], 11);
    // The raw `range` object must not leak through after normalization.
    assert!(first.get("range").is_none(), "raw range should be reshaped");
}

#[tokio::test]
async fn hover_content_extracted_from_each_contents_shape() {
    // (response payload, expected content, expected language)
    let cases = [
        // 1) bare MarkupContent ({kind, value}).
        (
            serde_json::json!({ "contents": { "kind": "markdown", "value": "fn foo()" } }),
            "fn foo()",
            Some("markdown"),
        ),
        // 2) bare string MarkedString.
        (
            serde_json::json!({ "contents": "plain docs" }),
            "plain docs",
            None,
        ),
        // 3) array of MarkedStrings (string + {language, value}).
        (
            serde_json::json!({
                "contents": [
                    "summary line",
                    { "language": "rust", "value": "let x = 1;" }
                ]
            }),
            "summary line\nlet x = 1;",
            Some("rust"),
        ),
    ];

    for (response, expected_content, expected_language) in cases {
        let registry = LspRegistry::new();
        registry.register_with_transport(
            "rust",
            LspServerStatus::Connected,
            None,
            vec!["hover".into()],
            Some(Arc::new(MockLspTransport { response })),
        );

        let result = registry
            .dispatch("hover", Some("src/main.rs"), Some(0), Some(0), None)
            .await
            .expect("hover dispatch");

        assert_eq!(result["content"], expected_content);
        match expected_language {
            Some(language) => assert_eq!(result["language"], language),
            None => assert!(result["language"].is_null()),
        }
        // The original `contents` shape must not survive normalization.
        assert!(result.get("contents").is_none());
    }
}

#[tokio::test]
async fn non_lsp_shaped_results_pass_through_unchanged() {
    // A transport returning an arbitrary object that is not a recognized LSP
    // result must be forwarded verbatim (e.g. mocks / future providers).
    let registry = LspRegistry::new();
    registry.register_with_transport(
        "rust",
        LspServerStatus::Connected,
        None,
        vec!["hover".into()],
        Some(Arc::new(MockLspTransport {
            response: serde_json::json!({ "hover": "docs" }),
        })),
    );

    let result = registry
        .dispatch("hover", Some("src/main.rs"), Some(0), Some(0), None)
        .await
        .expect("hover dispatch");
    assert_eq!(result["hover"], "docs");
}

#[tokio::test]
async fn rejects_unknown_server_paths_for_non_diagnostic_actions() {
    let registry = LspRegistry::new();
    let error = registry
        .dispatch("hover", Some("src/notes.txt"), Some(1), Some(0), None)
        .await
        .expect_err("unsupported extension should fail");
    assert!(error.contains("no LSP server available"));
}

#[tokio::test]
async fn rejects_disconnected_servers() {
    let registry = LspRegistry::new();
    registry.register(
        "rust",
        LspServerStatus::Disconnected,
        None,
        vec!["hover".into()],
    );

    let error = registry
        .dispatch("hover", Some("src/main.rs"), Some(1), Some(0), None)
        .await
        .expect_err("disconnected server should fail");
    assert!(error.contains("is not connected"));
    assert!(error.contains("disconnected"));
}

#[tokio::test]
async fn rejects_servers_without_advertised_capability() {
    let registry = LspRegistry::new();
    registry.register_with_transport(
        "rust",
        LspServerStatus::Connected,
        None,
        vec!["hover".into()],
        Some(Arc::new(MockLspTransport {
            response: serde_json::json!({"definition": []}),
        })),
    );

    let error = registry
        .dispatch("definition", Some("src/main.rs"), Some(1), Some(0), None)
        .await
        .expect_err("unsupported capability should fail");
    assert!(error.contains("does not advertise support for definition"));
}

#[tokio::test]
async fn rejects_connected_servers_without_active_transport() {
    let registry = LspRegistry::new();
    registry.register(
        "rust",
        LspServerStatus::Connected,
        None,
        vec!["hover".into(), "definition".into()],
    );

    let error = registry
        .dispatch("hover", Some("src/main.rs"), Some(1), Some(0), None)
        .await
        .expect_err("missing transport should fail");
    assert!(error.contains("has no active transport"));
    assert!(error.contains("hover"));
}

#[tokio::test]
async fn times_out_stuck_transport_dispatches() {
    let registry = LspRegistry::new();
    registry.register_with_transport(
        "rust",
        LspServerStatus::Connected,
        None,
        vec!["hover".into()],
        Some(Arc::new(PendingLspTransport)),
    );

    let error = registry
        .dispatch("hover", Some("src/main.rs"), Some(1), Some(0), None)
        .await
        .expect_err("stuck transport should time out");
    assert!(error.contains("timed out"));
    assert!(error.contains("hover"));
}
