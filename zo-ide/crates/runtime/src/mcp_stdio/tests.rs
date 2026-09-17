use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;
use tokio::runtime::Builder;

use crate::config::{
    ConfigSource, McpRemoteServerConfig, McpSdkServerConfig, McpServerConfig, McpStdioServerConfig,
    McpWebSocketServerConfig, ScopedMcpServerConfig,
};
use crate::mcp::mcp_tool_name;
use crate::mcp_client::McpClientBootstrap;

fn mark_test_script_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        let mut permissions = fs::metadata(path).expect("metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).expect("chmod");
    }
    #[cfg(windows)]
    {
        // Windows has no 0755 analogue. Script execution is selected by the
        // command host/PATHEXT; retain a real-file assertion so setup is not a
        // silent cfg-only no-op.
        assert!(fs::metadata(path).expect("metadata").is_file());
    }
}

use super::{
    InboundEvent, JsonRpcId, JsonRpcRequest, JsonRpcResponse, McpDiscoveryClass,
    McpInitializeClientInfo, McpInitializeParams, McpInitializeResult, McpInitializeServerInfo,
    McpListToolsResult, McpReadResourceParams, McpReadResourceResult, McpServerManager,
    McpServerManagerError, McpStdioProcess, McpTool, McpToolCallParams, spawn_mcp_stdio_process,
    unsupported_server_failed_server,
};
use crate::McpLifecyclePhase;

fn temp_dir() -> PathBuf {
    static NEXT_TEMP_DIR_ID: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be after epoch")
        .as_nanos();
    let unique_id = NEXT_TEMP_DIR_ID.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("runtime-mcp-stdio-{nanos}-{unique_id}"))
}

fn write_echo_script() -> PathBuf {
    let root = temp_dir();
    fs::create_dir_all(&root).expect("temp dir");
    let script_path = root.join("echo-mcp.sh");
    fs::write(
            &script_path,
            "#!/bin/sh\nprintf 'READY:%s\\n' \"$MCP_TEST_TOKEN\"\nIFS= read -r line\nprintf 'ECHO:%s\\n' \"$line\"\n",
        )
        .expect("write script");
    mark_test_script_executable(&script_path);
    script_path
}

fn write_audit_env_script() -> PathBuf {
    let root = temp_dir();
    fs::create_dir_all(&root).expect("temp dir");
    let script_path = root.join("audit-env-mcp.sh");
    fs::write(
            &script_path,
            "#!/bin/sh\nprintf 'AUDIT:%s|%s|%s|%s|%s|%s|%s\\n' \"$MCP_TEST_TOKEN\" \"$ZO_MCP_SERVER_NAME\" \"$ZO_MCP_SERVER_NORMALIZED_NAME\" \"$ZO_MCP_TOOL_PREFIX\" \"$ZO_MCP_TRANSPORT\" \"$ZO_MCP_PROJECT_SCOPED\" \"$ZO_MCP_CWD\"\n",
        )
        .expect("write script");
    mark_test_script_executable(&script_path);
    script_path
}

fn write_jsonrpc_script() -> PathBuf {
    let root = temp_dir();
    fs::create_dir_all(&root).expect("temp dir");
    let script_path = root.join("jsonrpc-mcp.py");
    let script = [
            "#!/usr/bin/env python3",
            "import json, os, sys",
            "CRLF_OUTPUT = os.environ.get('MCP_CRLF_OUTPUT') == '1'",
            "MISMATCHED_RESPONSE_ID = os.environ.get('MCP_MISMATCHED_RESPONSE_ID') == '1'",
            "INTERLEAVED_NOTIFICATION = os.environ.get('MCP_INTERLEAVED_NOTIFICATION') == '1'",
            "INTERLEAVED_TOOLS_CHANGED = os.environ.get('MCP_INTERLEAVED_TOOLS_CHANGED') == '1'",
            "line = sys.stdin.readline()",
            "if not line.strip():",
            "    raise SystemExit(1)",
            "request = json.loads(line)",
            r"assert request['jsonrpc'] == '2.0'",
            r"assert request['method'] == 'initialize'",
            "response_id = 'wrong-id' if MISMATCHED_RESPONSE_ID else request['id']",
            r"terminator = b'\r\n' if CRLF_OUTPUT else b'\n'",
            "def emit(message):",
            "    sys.stdout.buffer.write(json.dumps(message).encode() + terminator)",
            r"response = {",
            r"    'jsonrpc': '2.0',",
            r"    'id': response_id,",
            r"    'result': {",
            r"        'protocolVersion': request['params']['protocolVersion'],",
            r"        'capabilities': {'tools': {}},",
            r"        'serverInfo': {'name': 'fake-mcp', 'version': '0.1.0'}",
            r"    }",
            r"}",
            r"if INTERLEAVED_NOTIFICATION:",
            r"    emit({'jsonrpc': '2.0', 'method': 'notifications/progress'})",
            r"if INTERLEAVED_TOOLS_CHANGED:",
            r"    for _ in range(2):",
            r"        emit({'jsonrpc': '2.0', 'method': 'notifications/tools/list_changed'})",
            "emit(response)",
            "sys.stdout.buffer.flush()",
            "",
        ]
        .join("\n");
    fs::write(&script_path, script).expect("write script");
    mark_test_script_executable(&script_path);
    script_path
}

#[allow(clippy::too_many_lines)]
fn write_mcp_server_script() -> PathBuf {
    let root = temp_dir();
    fs::create_dir_all(&root).expect("temp dir");
    let script_path = root.join("fake-mcp-server.py");
    let script = [
            "#!/usr/bin/env python3",
            "import json, os, sys, time",
            "TOOL_CALL_DELAY_MS = int(os.environ.get('MCP_TOOL_CALL_DELAY_MS', '0'))",
            "INVALID_TOOL_CALL_RESPONSE = os.environ.get('MCP_INVALID_TOOL_CALL_RESPONSE') == '1'",
            "",
            "def read_message():",
            "    while True:",
            "        line = sys.stdin.readline()",
            "        if not line:",
            "            return None",
            "        if line.strip():",
            "            return json.loads(line)",
            "",
            "def send_message(message):",
            r"    sys.stdout.buffer.write(json.dumps(message).encode() + b'\n')",
            "    sys.stdout.buffer.flush()",
            "",
            "while True:",
            "    request = read_message()",
            "    if request is None:",
            "        break",
            "    if 'id' not in request:",
            "        continue",
            "    method = request['method']",
            "    if method == 'initialize':",
            "        send_message({",
            "            'jsonrpc': '2.0',",
            "            'id': request['id'],",
            "            'result': {",
            "                'protocolVersion': request['params']['protocolVersion'],",
            "                'capabilities': {'tools': {}, 'resources': {}},",
            "                'serverInfo': {'name': 'fake-mcp', 'version': '0.2.0'}",
            "            }",
            "        })",
            "    elif method == 'tools/list':",
            "        send_message({",
            "            'jsonrpc': '2.0',",
            "            'id': request['id'],",
            "            'result': {",
            "                'tools': [",
            "                    {",
            "                        'name': 'echo',",
            "                        'description': 'Echoes text',",
            "                        'inputSchema': {",
            "                            'type': 'object',",
            "                            'properties': {'text': {'type': 'string'}},",
            "                            'required': ['text']",
            "                        }",
            "                    }",
            "                ]",
            "            }",
            "        })",
            "    elif method == 'tools/call':",
            "        if INVALID_TOOL_CALL_RESPONSE:",
            "            sys.stdout.buffer.write(b'nope!\\n')",
            "            sys.stdout.buffer.flush()",
            "            continue",
            "        if TOOL_CALL_DELAY_MS:",
            "            time.sleep(TOOL_CALL_DELAY_MS / 1000)",
            "        args = request['params'].get('arguments') or {}",
            "        if request['params']['name'] == 'fail':",
            "            send_message({",
            "                'jsonrpc': '2.0',",
            "                'id': request['id'],",
            "                'error': {'code': -32001, 'message': 'tool failed'},",
            "            })",
            "        else:",
            "            text = args.get('text', '')",
            "            send_message({",
            "                'jsonrpc': '2.0',",
            "                'id': request['id'],",
            "                'result': {",
            "                    'content': [{'type': 'text', 'text': f'echo:{text}'}],",
            "                    'structuredContent': {'echoed': text},",
            "                    'isError': False",
            "                }",
            "            })",
            "    elif method == 'resources/list':",
            "        send_message({",
            "            'jsonrpc': '2.0',",
            "            'id': request['id'],",
            "            'result': {",
            "                'resources': [",
            "                    {",
            "                        'uri': 'file://guide.txt',",
            "                        'name': 'guide',",
            "                        'description': 'Guide text',",
            "                        'mimeType': 'text/plain'",
            "                    }",
            "                ]",
            "            }",
            "        })",
            "    elif method == 'prompts/list':",
            "        cursor = (request.get('params') or {}).get('cursor')",
            "        if cursor == 'page2':",
            "            send_message({",
            "                'jsonrpc': '2.0',",
            "                'id': request['id'],",
            "                'result': {",
            "                    'prompts': [",
            "                        {'name': 'plain', 'description': 'No-arg prompt'}",
            "                    ]",
            "                }",
            "            })",
            "        else:",
            "            send_message({",
            "                'jsonrpc': '2.0',",
            "                'id': request['id'],",
            "                'result': {",
            "                    'prompts': [",
            "                        {",
            "                            'name': 'review',",
            "                            'title': 'Code review',",
            "                            'description': 'Review the given file',",
            "                            'arguments': [",
            "                                {'name': 'path', 'required': True},",
            "                                {'name': 'focus'}",
            "                            ]",
            "                        }",
            "                    ],",
            "                    'nextCursor': 'page2'",
            "                }",
            "            })",
            "    elif method == 'prompts/get':",
            "        args = request['params'].get('arguments') or {}",
            "        name = request['params']['name']",
            "        path = args.get('path', '?')",
            "        send_message({",
            "            'jsonrpc': '2.0',",
            "            'id': request['id'],",
            "            'result': {",
            "                'description': f'resolved {name}',",
            "                'messages': [",
            "                    {",
            "                        'role': 'user',",
            "                        'content': {'type': 'text', 'text': f'Review {path}'}",
            "                    }",
            "                ]",
            "            }",
            "        })",
            "    elif method == 'resources/read':",
            "        uri = request['params']['uri']",
            "        send_message({",
            "            'jsonrpc': '2.0',",
            "            'id': request['id'],",
            "            'result': {",
            "                'contents': [",
            "                    {",
            "                        'uri': uri,",
            "                        'mimeType': 'text/plain',",
            "                        'text': f'contents for {uri}'",
            "                    }",
            "                ]",
            "            }",
            "        })",
            "    else:",
            "        send_message({",
            "            'jsonrpc': '2.0',",
            "            'id': request['id'],",
            "            'error': {'code': -32601, 'message': f'unknown method: {method}'},",
            "        })",
            "",
        ]
        .join("\n");
    fs::write(&script_path, script).expect("write script");
    mark_test_script_executable(&script_path);
    script_path
}

#[allow(clippy::too_many_lines)]
fn write_manager_mcp_server_script() -> PathBuf {
    let root = temp_dir();
    fs::create_dir_all(&root).expect("temp dir");
    let script_path = root.join("manager-mcp-server.py");
    let script = [
            "#!/usr/bin/env python3",
            "import json, os, sys, time",
            "",
            "LABEL = os.environ.get('MCP_SERVER_LABEL', 'server')",
            "LOG_PATH = os.environ.get('MCP_LOG_PATH')",
            "EXIT_AFTER_TOOLS_LIST = os.environ.get('MCP_EXIT_AFTER_TOOLS_LIST') == '1'",
            "EXIT_MARKER = os.environ.get('MCP_EXIT_MARKER')",
            "FAIL_ONCE_MODE = os.environ.get('MCP_FAIL_ONCE_MODE')",
            "FAIL_ONCE_MARKER = os.environ.get('MCP_FAIL_ONCE_MARKER')",
            "GROW_TOOLS = os.environ.get('MCP_GROW_TOOLS') == '1'",
            "EMIT_TOOLS_CHANGED = os.environ.get('MCP_EMIT_TOOLS_CHANGED') == '1'",
            "STUCK_CURSOR = os.environ.get('MCP_STUCK_CURSOR') == '1'",
            "LIST_TOOLS_DELAY_MS = int(os.environ.get('MCP_LIST_TOOLS_DELAY_MS', '0'))",
            "INITIALIZE_HANG_ALWAYS = os.environ.get('MCP_INITIALIZE_HANG_ALWAYS') == '1'",
            "SPAWN_LOG = os.environ.get('MCP_SPAWN_LOG')",
            "if SPAWN_LOG:",
            "    with open(SPAWN_LOG, 'a', encoding='utf-8') as handle:",
            "        handle.write('spawn\\n')",
            "initialize_count = 0",
            "tools_list_count = 0",
            "",
            "def log(method):",
            "    if LOG_PATH:",
            "        with open(LOG_PATH, 'a', encoding='utf-8') as handle:",
            "            handle.write(f'{method}\\n')",
            "",
            "def mark_exit():",
            "    if EXIT_MARKER:",
            "        with open(EXIT_MARKER, 'w', encoding='utf-8') as handle:",
            "            handle.write('exiting')",
            "",
            "def should_fail_once():",
            "    if not FAIL_ONCE_MODE or not FAIL_ONCE_MARKER:",
            "        return False",
            "    if os.path.exists(FAIL_ONCE_MARKER):",
            "        return False",
            "    with open(FAIL_ONCE_MARKER, 'w', encoding='utf-8') as handle:",
            "        handle.write(FAIL_ONCE_MODE)",
            "    return True",
            "",
            "def read_message():",
            "    while True:",
            "        line = sys.stdin.readline()",
            "        if not line:",
            "            return None",
            "        if line.strip():",
            "            return json.loads(line)",
            "",
            "def send_message(message):",
            r"    sys.stdout.buffer.write(json.dumps(message).encode() + b'\n')",
            "    sys.stdout.buffer.flush()",
            "",
            "while True:",
            "    request = read_message()",
            "    if request is None:",
            "        break",
            "    if 'id' not in request:",
            "        continue",
            "    method = request['method']",
            "    log(method)",
            "    if method == 'initialize':",
            "        if INITIALIZE_HANG_ALWAYS:",
            "            log('initialize-hang')",
            "            while True:",
            "                time.sleep(1)",
            "        if FAIL_ONCE_MODE == 'initialize_hang' and should_fail_once():",
            "            log('initialize-hang')",
            "            while True:",
            "                time.sleep(1)",
            "        initialize_count += 1",
            "        send_message({",
            "            'jsonrpc': '2.0',",
            "            'id': request['id'],",
            "            'result': {",
            "                'protocolVersion': request['params']['protocolVersion'],",
            "                'capabilities': {'tools': {'listChanged': True}},",
            "                'serverInfo': {'name': LABEL, 'version': '1.0.0'}",
            "            }",
            "        })",
            "        if EMIT_TOOLS_CHANGED:",
            "            send_message({'jsonrpc': '2.0', 'method': 'notifications/tools/list_changed'})",
            "    elif method == 'tools/list':",
            "        tools_list_count += 1",
            "        if LIST_TOOLS_DELAY_MS:",
            "            time.sleep(LIST_TOOLS_DELAY_MS / 1000)",
            "        tools = [",
            "            {",
            "                'name': 'echo',",
            "                'description': f'Echo tool for {LABEL}',",
            "                'inputSchema': {",
            "                    'type': 'object',",
            "                    'properties': {'text': {'type': 'string'}},",
            "                    'required': ['text']",
            "                }",
            "            }",
            "        ]",
            "        if GROW_TOOLS and tools_list_count >= 2:",
            "            tools.append({",
            "                'name': 'echo2',",
            "                'description': f'Second echo tool for {LABEL}',",
            "                'inputSchema': {'type': 'object', 'properties': {}}",
            "            })",
            "        result = {'tools': tools}",
            "        if STUCK_CURSOR:",
            "            result['nextCursor'] = 'stuck'",
            "        send_message({",
            "            'jsonrpc': '2.0',",
            "            'id': request['id'],",
            "            'result': result",
            "        })",
            "        if EXIT_AFTER_TOOLS_LIST:",
            "            mark_exit()",
            "            raise SystemExit(0)",
            "    elif method == 'tools/call':",
            "        if FAIL_ONCE_MODE == 'tool_call_disconnect' and should_fail_once():",
            "            log('tools/call-disconnect')",
            "            raise SystemExit(0)",
            "        args = request['params'].get('arguments') or {}",
            "        text = args.get('text', '')",
            "        send_message({",
            "            'jsonrpc': '2.0',",
            "            'id': request['id'],",
            "            'result': {",
            "                'content': [{'type': 'text', 'text': f'{LABEL}:{text}'}],",
            "                'structuredContent': {",
            "                    'server': LABEL,",
            "                    'echoed': text,",
            "                    'initializeCount': initialize_count",
            "                },",
            "                'isError': False",
            "            }",
            "        })",
            "    else:",
            "        send_message({",
            "            'jsonrpc': '2.0',",
            "            'id': request['id'],",
            "            'error': {'code': -32601, 'message': f'unknown method: {method}'},",
            "        })",
            "",
        ]
        .join("\n");
    fs::write(&script_path, script).expect("write script");
    mark_test_script_executable(&script_path);
    script_path
}

fn sample_bootstrap(script_path: &Path) -> McpClientBootstrap {
    let config = ScopedMcpServerConfig {
        scope: ConfigSource::Local,
        config: McpServerConfig::Stdio(McpStdioServerConfig {
            command: "/bin/sh".to_string(),
            args: vec![script_path.to_string_lossy().into_owned()],
            env: BTreeMap::from([("MCP_TEST_TOKEN".to_string(), "secret-value".to_string())]),
            tool_call_timeout_ms: None,
        }),
    };
    McpClientBootstrap::from_scoped_config("stdio server", &config)
}

fn script_transport(script_path: &Path) -> crate::mcp_client::McpStdioTransport {
    script_transport_with_env(script_path, BTreeMap::new())
}

fn script_transport_with_env(
    script_path: &Path,
    env: BTreeMap<String, String>,
) -> crate::mcp_client::McpStdioTransport {
    crate::mcp_client::McpStdioTransport {
        command: "python3".to_string(),
        args: vec![script_path.to_string_lossy().into_owned()],
        env,
        tool_call_timeout_ms: None,
    }
}

fn cleanup_script(script_path: &Path) {
    if let Err(error) = fs::remove_file(script_path) {
        assert_eq!(
            error.kind(),
            std::io::ErrorKind::NotFound,
            "cleanup script: {error}"
        );
    }
    if let Err(error) = fs::remove_dir_all(script_path.parent().expect("script parent")) {
        assert_eq!(
            error.kind(),
            std::io::ErrorKind::NotFound,
            "cleanup dir: {error}"
        );
    }
}

#[test]
fn remote_bridge_stdio_gets_longer_initialize_timeout() {
    let servers = BTreeMap::from([(
        "atlassian".to_string(),
        ScopedMcpServerConfig {
            scope: ConfigSource::User,
            config: McpServerConfig::Stdio(McpStdioServerConfig {
                command: "npx".to_string(),
                args: vec![
                    "-y".to_string(),
                    "mcp-remote".to_string(),
                    "https://mcp.atlassian.com/v1/sse".to_string(),
                ],
                env: BTreeMap::new(),
                tool_call_timeout_ms: None,
            }),
        },
    )]);
    let manager = McpServerManager::from_servers(&servers);

    assert_eq!(
        manager
            .initialize_timeout_ms("atlassian")
            .expect("timeout for atlassian"),
        super::MCP_REMOTE_BRIDGE_INITIALIZE_TIMEOUT_MS
    );
}

#[test]
fn ordinary_stdio_keeps_default_initialize_timeout() {
    let servers = BTreeMap::from([(
        "local".to_string(),
        ScopedMcpServerConfig {
            scope: ConfigSource::User,
            config: McpServerConfig::Stdio(McpStdioServerConfig {
                command: "uvx".to_string(),
                args: vec!["local-mcp".to_string()],
                env: BTreeMap::new(),
                tool_call_timeout_ms: None,
            }),
        },
    )]);
    let manager = McpServerManager::from_servers(&servers);

    assert_eq!(
        manager
            .initialize_timeout_ms("local")
            .expect("timeout for local"),
        super::MCP_INITIALIZE_TIMEOUT_MS
    );
}

#[test]
fn npx_cold_start_stdio_gets_longer_initialize_timeout() {
    // A plain `npx <pkg>` stdio server (not an `mcp-remote` bridge) still pays
    // the npm cold-start download on first boot, so it must get the wider
    // remote-bridge window — the 15s default times out before the package
    // installs and surfaces a healthy server (e.g. Chrome DevTools MCP) as
    // failed.
    let servers = BTreeMap::from([(
        "chrome-devtools".to_string(),
        ScopedMcpServerConfig {
            scope: ConfigSource::User,
            config: McpServerConfig::Stdio(McpStdioServerConfig {
                command: "npx".to_string(),
                args: vec![
                    "chrome-devtools-mcp@latest".to_string(),
                    "--isolated=true".to_string(),
                ],
                env: BTreeMap::new(),
                tool_call_timeout_ms: None,
            }),
        },
    )]);
    let manager = McpServerManager::from_servers(&servers);

    assert_eq!(
        manager
            .initialize_timeout_ms("chrome-devtools")
            .expect("timeout for chrome-devtools"),
        super::MCP_REMOTE_BRIDGE_INITIALIZE_TIMEOUT_MS
    );
}

#[test]
fn discovery_timeout_wraps_initialize_retry_and_tools_list() {
    let servers = BTreeMap::from([
        (
            "atlassian".to_string(),
            ScopedMcpServerConfig {
                scope: ConfigSource::User,
                config: McpServerConfig::Stdio(McpStdioServerConfig {
                    command: "npx".to_string(),
                    args: vec![
                        "-y".to_string(),
                        "mcp-remote".to_string(),
                        "https://mcp.atlassian.com/v1/sse".to_string(),
                    ],
                    env: BTreeMap::new(),
                    tool_call_timeout_ms: None,
                }),
            },
        ),
        (
            "local".to_string(),
            ScopedMcpServerConfig {
                scope: ConfigSource::User,
                config: McpServerConfig::Stdio(McpStdioServerConfig {
                    command: "uvx".to_string(),
                    args: vec!["local-mcp".to_string()],
                    env: BTreeMap::new(),
                    tool_call_timeout_ms: None,
                }),
            },
        ),
    ]);
    let manager = McpServerManager::from_servers(&servers);

    assert_eq!(
        manager
            .discovery_timeout_ms("atlassian")
            .expect("discovery timeout for remote bridge"),
        super::MCP_REMOTE_BRIDGE_INITIALIZE_TIMEOUT_MS * 2
            + super::MCP_LIST_TOOLS_TIMEOUT_MS
            + super::MCP_DISCOVERY_MARGIN_MS
    );
    assert_eq!(
        manager
            .discovery_timeout_ms("local")
            .expect("discovery timeout for local stdio"),
        super::MCP_INITIALIZE_TIMEOUT_MS * 2
            + super::MCP_LIST_TOOLS_TIMEOUT_MS
            + super::MCP_DISCOVERY_MARGIN_MS
    );
}

fn manager_server_config(
    script_path: &Path,
    label: &str,
    log_path: &Path,
) -> ScopedMcpServerConfig {
    manager_server_config_with_env(script_path, label, log_path, BTreeMap::new())
}

fn manager_server_config_with_env(
    script_path: &Path,
    label: &str,
    log_path: &Path,
    extra_env: BTreeMap<String, String>,
) -> ScopedMcpServerConfig {
    let mut env = BTreeMap::from([
        ("MCP_SERVER_LABEL".to_string(), label.to_string()),
        (
            "MCP_LOG_PATH".to_string(),
            log_path.to_string_lossy().into_owned(),
        ),
    ]);
    env.extend(extra_env);
    ScopedMcpServerConfig {
        scope: ConfigSource::Local,
        config: McpServerConfig::Stdio(McpStdioServerConfig {
            command: "python3".to_string(),
            args: vec![script_path.to_string_lossy().into_owned()],
            env,
            tool_call_timeout_ms: None,
        }),
    }
}

#[test]
fn spawns_stdio_process_and_round_trips_io() {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_echo_script();
        let bootstrap = sample_bootstrap(&script_path);
        let mut process = spawn_mcp_stdio_process(&bootstrap).expect("spawn stdio process");

        let ready = process.read_line().await.expect("read ready");
        assert_eq!(ready, "READY:secret-value\n");

        process
            .write_line("ping from client")
            .await
            .expect("write line");

        let echoed = process.read_line().await.expect("read echo");
        assert_eq!(echoed, "ECHO:ping from client\n");

        let status = process.wait().await.expect("wait for exit");
        assert!(status.success());

        cleanup_script(&script_path);
    });
}

#[test]
fn bootstrap_spawn_adds_mcp_audit_environment() {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_audit_env_script();
        let config = ScopedMcpServerConfig {
            scope: ConfigSource::Project,
            config: McpServerConfig::Stdio(McpStdioServerConfig {
                command: "/bin/sh".to_string(),
                args: vec![script_path.to_string_lossy().into_owned()],
                env: BTreeMap::from([("MCP_TEST_TOKEN".to_string(), "secret-value".to_string())]),
                tool_call_timeout_ms: None,
            }),
        };
        let bootstrap = McpClientBootstrap::from_scoped_config("audit server", &config);
        let mut process = spawn_mcp_stdio_process(&bootstrap).expect("spawn stdio process");

        let ready = process.read_line().await.expect("read audit env");
        let fields = ready
            .trim()
            .strip_prefix("AUDIT:")
            .expect("audit prefix")
            .split('|')
            .collect::<Vec<_>>();
        assert_eq!(fields[0], "secret-value");
        assert_eq!(fields[1], "audit server");
        assert_eq!(fields[2], "audit_server");
        assert_eq!(fields[3], "mcp__audit_server__");
        assert_eq!(fields[4], "stdio");
        assert_eq!(fields[5], "true");
        assert_eq!(
            fields[6],
            std::env::current_dir().expect("cwd").display().to_string()
        );

        let status = process.wait().await.expect("wait for exit");
        assert!(status.success());

        cleanup_script(&script_path);
    });
}

#[test]
fn rejects_non_stdio_bootstrap() {
    let config = ScopedMcpServerConfig {
        scope: ConfigSource::Local,
        config: McpServerConfig::Sdk(crate::config::McpSdkServerConfig {
            name: "sdk-server".to_string(),
        }),
    };
    let bootstrap = McpClientBootstrap::from_scoped_config("sdk server", &config);
    let error = spawn_mcp_stdio_process(&bootstrap).expect_err("non-stdio should fail");
    assert_eq!(error.kind(), ErrorKind::InvalidInput);
}

#[test]
fn round_trips_initialize_request_and_response_over_stdio_frames() {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_jsonrpc_script();
        let transport = script_transport(&script_path);
        let mut process = McpStdioProcess::spawn(&transport).expect("spawn transport directly");

        let response = process
            .initialize(
                JsonRpcId::Number(1),
                McpInitializeParams {
                    protocol_version: "2025-03-26".to_string(),
                    capabilities: json!({"roots": {}}),
                    client_info: McpInitializeClientInfo {
                        name: "runtime-tests".to_string(),
                        version: "0.1.0".to_string(),
                    },
                },
            )
            .await
            .expect("initialize roundtrip");

        assert_eq!(response.id, JsonRpcId::Number(1));
        assert_eq!(response.error, None);
        assert_eq!(
            response.result,
            Some(McpInitializeResult {
                protocol_version: "2025-03-26".to_string(),
                capabilities: json!({"tools": {}}),
                server_info: McpInitializeServerInfo {
                    name: "fake-mcp".to_string(),
                    version: "0.1.0".to_string(),
                },
            })
        );

        let status = process.wait().await.expect("wait for exit");
        assert!(status.success());

        cleanup_script(&script_path);
    });
}

#[test]
fn write_jsonrpc_request_emits_newline_delimited_frame() {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_jsonrpc_script();
        let transport = script_transport(&script_path);
        let mut process = McpStdioProcess::spawn(&transport).expect("spawn transport directly");
        let request = JsonRpcRequest::new(
            JsonRpcId::Number(7),
            "initialize",
            Some(json!({
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": {"name": "runtime-tests", "version": "0.1.0"}
            })),
        );

        process.send_request(&request).await.expect("send request");
        let response: JsonRpcResponse<serde_json::Value> =
            process.read_response().await.expect("read response");

        assert_eq!(response.id, JsonRpcId::Number(7));
        assert_eq!(response.jsonrpc, "2.0");

        let status = process.wait().await.expect("wait for exit");
        assert!(status.success());

        cleanup_script(&script_path);
    });
}

fn init_params() -> McpInitializeParams {
    McpInitializeParams {
        protocol_version: "2025-03-26".to_string(),
        capabilities: json!({ "roots": {} }),
        client_info: McpInitializeClientInfo {
            name: "runtime-tests".to_string(),
            version: "0.1.0".to_string(),
        },
    }
}

#[test]
fn captures_interleaved_tools_list_changed_and_drains_on_poll() {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_jsonrpc_script();
        let transport = script_transport_with_env(
            &script_path,
            BTreeMap::from([("MCP_INTERLEAVED_TOOLS_CHANGED".to_string(), "1".to_string())]),
        );
        let mut process = McpStdioProcess::spawn(&transport).expect("spawn");

        // The notification frames precede the initialize response, so the
        // request read loop captures them while waiting for its response.
        process
            .initialize(JsonRpcId::Number(1), init_params())
            .await
            .expect("initialize roundtrip");

        // Two identical notifications collapse to one (idempotent dedup).
        assert_eq!(process.poll_inbound(), vec![InboundEvent::ToolsListChanged]);
        // Draining is exhaustive.
        assert!(process.poll_inbound().is_empty());

        let _ = process.wait().await;
        cleanup_script(&script_path);
    });
}

#[test]
fn ignores_interleaved_progress_notification() {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_jsonrpc_script();
        let transport = script_transport_with_env(
            &script_path,
            BTreeMap::from([("MCP_INTERLEAVED_NOTIFICATION".to_string(), "1".to_string())]),
        );
        let mut process = McpStdioProcess::spawn(&transport).expect("spawn");

        process
            .initialize(JsonRpcId::Number(1), init_params())
            .await
            .expect("initialize roundtrip");

        // `notifications/progress` is read past but never buffered.
        assert!(process.poll_inbound().is_empty());

        let _ = process.wait().await;
        cleanup_script(&script_path);
    });
}

#[test]
fn refresh_server_tools_grows_one_server_and_preserves_others() {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_manager_mcp_server_script();
        let parent = script_path.parent().expect("script parent");
        let log_alpha = parent.join("alpha.log");
        let log_beta = parent.join("beta.log");

        let mut servers = BTreeMap::new();
        servers.insert(
            "alpha".to_string(),
            manager_server_config_with_env(
                &script_path,
                "alpha",
                &log_alpha,
                BTreeMap::from([("MCP_GROW_TOOLS".to_string(), "1".to_string())]),
            ),
        );
        servers.insert(
            "beta".to_string(),
            manager_server_config(&script_path, "beta", &log_beta),
        );
        let mut manager = McpServerManager::from_servers(&servers);

        let report = manager.discover_tools_best_effort().await;
        assert!(
            report.failed_servers.is_empty(),
            "{:?}",
            report.failed_servers
        );
        assert_eq!(report.tools.len(), 2); // alpha:echo + beta:echo

        // `tools/list_changed` on alpha → re-discover; grow flag adds echo2.
        let fresh = manager
            .refresh_server_tools("alpha")
            .await
            .expect("refresh alpha");
        assert_eq!(fresh.len(), 2);
        assert!(fresh.iter().any(|tool| tool.raw_name == "echo2"));

        // alpha's new tool routes through the updated index...
        let alpha_echo2 = mcp_tool_name("alpha", "echo2");
        manager
            .call_tool(&alpha_echo2, Some(json!({ "text": "hi" })))
            .await
            .expect("new tool routes");
        // ...and beta's pre-existing route survived (not nuked by refresh).
        let beta_echo = mcp_tool_name("beta", "echo");
        manager
            .call_tool(&beta_echo, Some(json!({ "text": "hi" })))
            .await
            .expect("beta still routes");

        manager.shutdown().await.expect("shutdown");
        cleanup_script(&script_path);
    });
}

#[test]
fn refresh_server_tools_relists_when_discover_tools_cache_is_warm() {
    // Regression for the bridge's `UnknownTool` retry (McpToolRegistry::
    // spawn_tool_call): on a missing route it re-discovers, then calls again.
    // It MUST use `refresh_server_tools` (a forced per-server re-list), NOT
    // `discover_tools`, which short-circuits on the warm `discovered_tools_cache`
    // and returns the same stale list — leaving the retry to fail forever with
    // `UnknownTool`. This proves the two methods diverge on a warm cache.
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_manager_mcp_server_script();
        let parent = script_path.parent().expect("script parent");
        let log_path = parent.join("grow.log");

        let servers = BTreeMap::from([(
            "alpha".to_string(),
            manager_server_config_with_env(
                &script_path,
                "alpha",
                &log_path,
                // GROW_TOOLS: the 1st `tools/list` returns only `echo`; `echo2`
                // appears from the 2nd `tools/list` onward.
                BTreeMap::from([("MCP_GROW_TOOLS".to_string(), "1".to_string())]),
            ),
        )]);
        let mut manager = McpServerManager::from_servers(&servers);

        // 1. Initial discovery warms the cache with a list that lacks `echo2`.
        let discovered = manager.discover_tools().await.expect("initial discover");
        assert_eq!(discovered.len(), 1);
        let echo2 = mcp_tool_name("alpha", "echo2");
        assert!(
            matches!(
                manager.call_tool(&echo2, Some(json!({}))).await,
                Err(McpServerManagerError::UnknownTool { .. })
            ),
            "echo2 must be unroutable before any re-list"
        );

        // 2. A plain `discover_tools` cannot recover: the warm cache short-circuits
        //    it (no fresh `tools/list`), so `echo2` stays unknown. This is exactly
        //    why the old bridge retry was a silent no-op.
        let cached = manager.discover_tools().await.expect("cached discover");
        assert_eq!(
            cached.len(),
            1,
            "discover_tools must short-circuit on the warm cache"
        );
        assert!(
            matches!(
                manager.call_tool(&echo2, Some(json!({}))).await,
                Err(McpServerManagerError::UnknownTool { .. })
            ),
            "discover_tools cannot pick up echo2 on a warm cache"
        );

        // 3. `refresh_server_tools` forces a fresh `tools/list` (2nd call → echo2),
        //    so the retry can finally route. This is what the bridge uses post-fix.
        let fresh = manager
            .refresh_server_tools("alpha")
            .await
            .expect("forced re-list");
        assert!(fresh.iter().any(|tool| tool.raw_name == "echo2"));
        manager
            .call_tool(&echo2, Some(json!({})))
            .await
            .expect("echo2 routes after a forced re-list");

        manager.shutdown().await.expect("shutdown");
        cleanup_script(&script_path);
    });
}

#[test]
fn discover_tools_best_effort_isolates_a_slow_server_and_keeps_fast_tools() {
    // Regression: one server stalling in `tools/list` must not starve
    // discovery for the others. The prior global timeout discarded *every*
    // server's tools when any one ran long, leaving healthy MCP tools
    // unroutable as "unknown MCP tool".
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_manager_mcp_server_script();
        let parent = script_path.parent().expect("script parent");
        let log_fast = parent.join("fast.log");
        let log_slow = parent.join("slow.log");

        let mut servers = BTreeMap::new();
        servers.insert(
            "fast".to_string(),
            manager_server_config(&script_path, "fast", &log_fast),
        );
        servers.insert(
            "slow".to_string(),
            manager_server_config_with_env(
                &script_path,
                "slow",
                &log_slow,
                BTreeMap::from([("MCP_LIST_TOOLS_DELAY_MS".to_string(), "5000".to_string())]),
            ),
        );
        let mut manager = McpServerManager::from_servers(&servers);
        manager.set_discover_server_timeout_ms(300);

        let report = manager.discover_tools_best_effort().await;

        // Fast server's tool survived and is routable...
        let fast_echo = mcp_tool_name("fast", "echo");
        assert!(
            report
                .tools
                .iter()
                .any(|tool| tool.qualified_name == fast_echo),
            "fast server tool missing from report: {:?}",
            report.tools
        );
        manager
            .call_tool(&fast_echo, Some(json!({ "text": "hi" })))
            .await
            .expect("fast tool routes after a slow neighbor timed out");

        // ...while the slow server is surfaced as degraded, not dropped.
        assert!(
            report
                .failed_servers
                .iter()
                .any(|failure| failure.server_name == "slow"),
            "slow server should be a discovery failure: {:?}",
            report.failed_servers
        );

        manager.shutdown().await.expect("shutdown");
        cleanup_script(&script_path);
    });
}

#[test]
fn discover_tools_best_effort_stops_at_total_budget_and_skips_remaining() {
    // Hardening: once the overall discovery budget is spent, remaining
    // servers are surfaced as degraded WITHOUT being probed (so startup
    // stays bounded) while tools discovered so far are retained.
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_manager_mcp_server_script();
        let parent = script_path.parent().expect("script parent");
        let log_a = parent.join("a-slow.log");
        let log_b = parent.join("b-never.log");

        // BTreeMap iterates sorted: "a-slow" first exhausts the budget,
        // so "b-never" must be skipped (never contacted).
        let mut servers = BTreeMap::new();
        servers.insert(
            "a-slow".to_string(),
            manager_server_config_with_env(
                &script_path,
                "a-slow",
                &log_a,
                BTreeMap::from([("MCP_LIST_TOOLS_DELAY_MS".to_string(), "5000".to_string())]),
            ),
        );
        servers.insert(
            "b-never".to_string(),
            manager_server_config(&script_path, "b-never", &log_b),
        );
        let mut manager = McpServerManager::from_servers(&servers);
        // Large per-server budget so the *total* budget governs. a-slow is
        // allowed to start because the remaining total can cover initialize,
        // but the in-flight probe is still capped by the remaining total rather
        // than the 5s server timeout.
        manager.set_discover_server_timeout_ms(5_000);
        manager.set_discover_total_timeout_ms(400);

        let report = manager.discover_tools_best_effort().await;

        // Both degraded: a-slow timed out, b-never skipped past the budget.
        assert!(
            report
                .failed_servers
                .iter()
                .any(|f| f.server_name == "a-slow"),
            "a-slow should be a discovery failure: {:?}",
            report.failed_servers
        );
        assert!(
            report
                .failed_servers
                .iter()
                .any(|f| f.server_name == "b-never"),
            "b-never should be degraded once the budget is spent: {:?}",
            report.failed_servers
        );
        // b-never was never probed — its mock never logged a method.
        let b_log = std::fs::read_to_string(&log_b).unwrap_or_default();
        assert!(
            b_log.is_empty(),
            "b-never must not be contacted after the budget is spent, got: {b_log:?}"
        );

        manager.shutdown().await.expect("shutdown");
        cleanup_script(&script_path);
    });
}

#[test]
fn discover_tools_best_effort_skips_remote_when_total_cannot_cover_initialize_and_continues() {
    // Regression: a remote/OAuth bridge can need a longer initialize timeout
    // than the eager total budget. Such a server must not consume the whole
    // budget and starve a later healthy local server.
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_manager_mcp_server_script();
        let parent = script_path.parent().expect("script parent");
        let log_remote = parent.join("a-remote.log");
        let log_local = parent.join("b-local.log");

        let mut servers = BTreeMap::new();
        servers.insert(
            "a-remote".to_string(),
            ScopedMcpServerConfig {
                scope: ConfigSource::User,
                config: McpServerConfig::Stdio(McpStdioServerConfig {
                    command: "python3".to_string(),
                    args: vec![
                        script_path.to_string_lossy().to_string(),
                        "mcp-remote".to_string(),
                    ],
                    env: BTreeMap::from([
                        ("MCP_SERVER_LABEL".to_string(), "a-remote".to_string()),
                        (
                            "MCP_LOG_PATH".to_string(),
                            log_remote.to_string_lossy().to_string(),
                        ),
                    ]),
                    tool_call_timeout_ms: None,
                }),
            },
        );
        servers.insert(
            "b-local".to_string(),
            manager_server_config(&script_path, "b-local", &log_local),
        );
        let mut manager = McpServerManager::from_servers(&servers);
        manager.set_discover_server_timeout_ms(950);
        manager.set_discover_total_timeout_ms(950);

        let report = manager.discover_tools_best_effort().await;

        assert!(
            report
                .failed_servers
                .iter()
                .any(|failure| failure.server_name == "a-remote"),
            "remote server should be degraded when total budget cannot cover initialize: {:?}",
            report.failed_servers
        );
        let local_echo = mcp_tool_name("b-local", "echo");
        assert!(
            report
                .tools
                .iter()
                .any(|tool| tool.qualified_name == local_echo),
            "healthy local server should still be discovered: {:?}",
            report.tools
        );
        assert!(
            std::fs::read_to_string(&log_remote)
                .unwrap_or_default()
                .is_empty(),
            "remote server should not be started when its initialize budget cannot fit"
        );
        assert!(
            std::fs::read_to_string(&log_local)
                .unwrap_or_default()
                .contains("tools/list"),
            "local server should be probed"
        );

        manager.shutdown().await.expect("shutdown");
        cleanup_script(&script_path);
    });
}

#[test]
fn discover_tools_best_effort_attempts_local_with_partial_remaining_budget() {
    // A local stdio server should be attempted even when the remaining total
    // budget is below the conservative initialize timeout; fast local servers
    // often complete well inside that remainder, and skipping them would hide
    // healthy tools unnecessarily.
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_manager_mcp_server_script();
        let parent = script_path.parent().expect("script parent");
        let log_b = parent.join("b-fast.log");

        let mut servers = BTreeMap::new();
        servers.insert(
            "b-fast".to_string(),
            manager_server_config(&script_path, "b-fast", &log_b),
        );
        let mut manager = McpServerManager::from_servers(&servers);
        manager
            .ensure_server_ready("b-fast")
            .await
            .expect("pre-initialize fast server");
        // Keep the live process so this integration test measures the scheduler
        // decision rather than process startup. The injected total is already
        // below the server's conservative initialize allowance, so a scheduler
        // that pre-skips local stdio under a partial budget still fails here.
        let partial_budget_ms = super::MCP_INITIALIZE_TIMEOUT_MS - 10;
        assert!(partial_budget_ms < manager.initialize_timeout_ms_for("b-fast"));
        manager.set_discover_total_timeout_ms(partial_budget_ms);

        let report = manager.discover_tools_best_effort().await;
        let fast_echo = mcp_tool_name("b-fast", "echo");
        assert!(
            report
                .tools
                .iter()
                .any(|tool| tool.qualified_name == fast_echo),
            "fast local server should be discovered with partial remaining budget: tools={:?} failures={:?}",
            report.tools,
            report.failed_servers
        );
        assert!(
            std::fs::read_to_string(&log_b)
                .unwrap_or_default()
                .contains("tools/list"),
            "b-fast should be attempted rather than pre-skipped"
        );

        manager.shutdown().await.expect("shutdown");
        cleanup_script(&script_path);
    });
}

#[test]
fn discover_tools_best_effort_attempts_remote_when_budget_covers_initialize() {
    // Regression for production eager discovery: the default total budget must
    // be large enough for a remote/OAuth bridge to get an initialize attempt,
    // while shorter local servers are still discovered first.
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_manager_mcp_server_script();
        let parent = script_path.parent().expect("script parent");
        let log_remote = parent.join("z-remote.log");
        let log_local = parent.join("a-local.log");

        let mut servers = BTreeMap::new();
        servers.insert(
            "z-remote".to_string(),
            ScopedMcpServerConfig {
                scope: ConfigSource::User,
                config: McpServerConfig::Stdio(McpStdioServerConfig {
                    command: "python3".to_string(),
                    args: vec![
                        script_path.to_string_lossy().to_string(),
                        "mcp-remote".to_string(),
                    ],
                    env: BTreeMap::from([
                        ("MCP_SERVER_LABEL".to_string(), "z-remote".to_string()),
                        (
                            "MCP_LOG_PATH".to_string(),
                            log_remote.to_string_lossy().to_string(),
                        ),
                    ]),
                    tool_call_timeout_ms: None,
                }),
            },
        );
        servers.insert(
            "a-local".to_string(),
            manager_server_config(&script_path, "a-local", &log_local),
        );
        let mut manager = McpServerManager::from_servers(&servers);

        let report = manager.discover_tools_best_effort().await;
        for server in ["a-local", "z-remote"] {
            let echo = mcp_tool_name(server, "echo");
            assert!(
                report.tools.iter().any(|tool| tool.qualified_name == echo),
                "{server} should be discovered: tools={:?} failures={:?}",
                report.tools,
                report.failed_servers
            );
        }
        assert!(
            std::fs::read_to_string(&log_remote)
                .unwrap_or_default()
                .contains("initialize"),
            "remote bridge should receive an initialize attempt"
        );
        assert!(
            std::fs::read_to_string(&log_local)
                .unwrap_or_default()
                .contains("tools/list"),
            "local server should still be discovered"
        );

        manager.shutdown().await.expect("shutdown");
        cleanup_script(&script_path);
    });
}

#[test]
fn list_changed_notification_then_refresh_exposes_the_new_tool() {
    // End-to-end consumer chain against a live process: the server announces
    // `tools/list_changed`, we detect it via `poll_all_inbound`, refresh that
    // server in response, and the freshly-grown tool becomes routable.
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_manager_mcp_server_script();
        let parent = script_path.parent().expect("script parent");
        let log = parent.join("grow-emit.log");

        let mut servers = BTreeMap::new();
        servers.insert(
            "grower".to_string(),
            manager_server_config_with_env(
                &script_path,
                "grower",
                &log,
                BTreeMap::from([
                    ("MCP_EMIT_TOOLS_CHANGED".to_string(), "1".to_string()),
                    ("MCP_GROW_TOOLS".to_string(), "1".to_string()),
                ]),
            ),
        );
        let mut manager = McpServerManager::from_servers(&servers);

        // First discovery sees only `echo` and buffers the notification.
        let report = manager.discover_tools_best_effort().await;
        assert!(
            report.failed_servers.is_empty(),
            "{:?}",
            report.failed_servers
        );
        assert_eq!(report.tools.len(), 1);

        // The turn-boundary consumer drains the notification...
        let changed = manager.poll_all_inbound();
        assert_eq!(
            changed,
            vec![("grower".to_string(), InboundEvent::ToolsListChanged)]
        );

        // ...and refreshes exactly that server, picking up the grown tool.
        let fresh = manager
            .refresh_server_tools("grower")
            .await
            .expect("refresh grower");
        assert!(fresh.iter().any(|tool| tool.raw_name == "echo2"));

        // The new tool is now routable through the updated index.
        let echo2 = mcp_tool_name("grower", "echo2");
        manager
            .call_tool(&echo2, Some(json!({ "text": "hi" })))
            .await
            .expect("grown tool routes");

        manager.shutdown().await.expect("shutdown");
        cleanup_script(&script_path);
    });
}

#[test]
fn poll_all_inbound_tags_events_with_their_server() {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_manager_mcp_server_script();
        let parent = script_path.parent().expect("script parent");
        let log = parent.join("emitter.log");

        let mut servers = BTreeMap::new();
        servers.insert(
            "emitter".to_string(),
            manager_server_config_with_env(
                &script_path,
                "emitter",
                &log,
                BTreeMap::from([("MCP_EMIT_TOOLS_CHANGED".to_string(), "1".to_string())]),
            ),
        );
        let mut manager = McpServerManager::from_servers(&servers);

        // Discovery's tools/list read loop captures the notification the
        // server emitted right after initialize.
        let report = manager.discover_tools_best_effort().await;
        assert!(
            report.failed_servers.is_empty(),
            "{:?}",
            report.failed_servers
        );

        assert_eq!(
            manager.poll_all_inbound(),
            vec![("emitter".to_string(), InboundEvent::ToolsListChanged)]
        );
        // Drained.
        assert!(manager.poll_all_inbound().is_empty());

        manager.shutdown().await.expect("shutdown");
        cleanup_script(&script_path);
    });
}

#[test]
fn given_crlf_terminated_output_when_initialize_then_response_parses() {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_jsonrpc_script();
        let transport = script_transport_with_env(
            &script_path,
            BTreeMap::from([("MCP_CRLF_OUTPUT".to_string(), "1".to_string())]),
        );
        let mut process = McpStdioProcess::spawn(&transport).expect("spawn transport directly");

        let response = process
            .initialize(
                JsonRpcId::Number(8),
                McpInitializeParams {
                    protocol_version: "2025-03-26".to_string(),
                    capabilities: json!({"roots": {}}),
                    client_info: McpInitializeClientInfo {
                        name: "runtime-tests".to_string(),
                        version: "0.1.0".to_string(),
                    },
                },
            )
            .await
            .expect("initialize roundtrip");

        assert_eq!(response.id, JsonRpcId::Number(8));
        assert_eq!(response.error, None);
        assert!(response.result.is_some());

        let status = process.wait().await.expect("wait for exit");
        assert!(status.success());

        cleanup_script(&script_path);
    });
}

#[test]
fn given_only_mismatched_id_response_when_initialize_then_does_not_accept_it() {
    // 동작 변경(C1): 잘못된 id 응답은 즉시 실패시키지 않고 skip 한 뒤 매칭
    // 응답을 기다린다. 서버가 wrong-id 응답만 보내고 종료하면, 클라이언트는
    // 그 응답을 결과로 수락하지 않고 매칭 응답을 못 받아 스트림 종료(EOF)
    // 에러로 끝난다 — 즉 엉뚱한 id 를 결과로 반환하지 않는다.
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_jsonrpc_script();
        let transport = script_transport_with_env(
            &script_path,
            BTreeMap::from([("MCP_MISMATCHED_RESPONSE_ID".to_string(), "1".to_string())]),
        );
        let mut process = McpStdioProcess::spawn(&transport).expect("spawn transport directly");

        let error = process
            .initialize(
                JsonRpcId::Number(9),
                McpInitializeParams {
                    protocol_version: "2025-03-26".to_string(),
                    capabilities: json!({"roots": {}}),
                    client_info: McpInitializeClientInfo {
                        name: "runtime-tests".to_string(),
                        version: "0.1.0".to_string(),
                    },
                },
            )
            .await
            .expect_err("wrong-id response must not be accepted as the result");

        // 매칭 응답 없이 스트림이 닫혀 EOF 로 끝난다(엉뚱한 id 수락 안 함).
        assert_eq!(error.kind(), ErrorKind::UnexpectedEof);

        let status = process.wait().await.expect("wait for exit");
        assert!(status.success());

        cleanup_script(&script_path);
    });
}

#[test]
fn given_interleaved_notification_before_response_then_skips_and_succeeds() {
    // C1 회귀: 서버가 응답 전에 id 없는 notification(notifications/progress
    // 등)을 끼워 보내도, 클라이언트는 그것을 skip 하고 매칭 응답을 반환해야
    // 한다. 구버전은 첫 프레임(notification)을 JsonRpcResponse 로 파싱하려다
    // 실패 → InvalidData → 서버 재시작 루프였다.
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_jsonrpc_script();
        let transport = script_transport_with_env(
            &script_path,
            BTreeMap::from([("MCP_INTERLEAVED_NOTIFICATION".to_string(), "1".to_string())]),
        );
        let mut process = McpStdioProcess::spawn(&transport).expect("spawn transport directly");

        let response = process
            .initialize(
                JsonRpcId::Number(11),
                McpInitializeParams {
                    protocol_version: "2025-03-26".to_string(),
                    capabilities: json!({"roots": {}}),
                    client_info: McpInitializeClientInfo {
                        name: "runtime-tests".to_string(),
                        version: "0.1.0".to_string(),
                    },
                },
            )
            .await
            .expect("notification before response should be skipped, then initialize succeeds");

        assert_eq!(response.id, JsonRpcId::Number(11));
        assert!(response.result.is_some());

        let status = process.wait().await.expect("wait for exit");
        assert!(status.success());

        cleanup_script(&script_path);
    });
}

#[test]
fn direct_spawn_uses_transport_env() {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_echo_script();
        let transport = crate::mcp_client::McpStdioTransport {
            command: "/bin/sh".to_string(),
            args: vec![script_path.to_string_lossy().into_owned()],
            env: BTreeMap::from([("MCP_TEST_TOKEN".to_string(), "direct-secret".to_string())]),
            tool_call_timeout_ms: None,
        };
        let mut process = McpStdioProcess::spawn(&transport).expect("spawn transport directly");
        let ready = process.read_available().await.expect("read ready");
        assert_eq!(String::from_utf8_lossy(&ready), "READY:direct-secret\n");
        process.terminate().await.expect("terminate child");
        let _ = process.wait().await.expect("wait after kill");

        cleanup_script(&script_path);
    });
}

#[test]
fn lists_tools_calls_tool_and_reads_resources_over_jsonrpc() {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_mcp_server_script();
        let transport = script_transport(&script_path);
        let mut process = McpStdioProcess::spawn(&transport).expect("spawn fake mcp server");

        let tools = process
            .list_tools(JsonRpcId::Number(2), None)
            .await
            .expect("list tools");
        assert_eq!(tools.error, None);
        assert_eq!(tools.id, JsonRpcId::Number(2));
        assert_eq!(
            tools.result,
            Some(McpListToolsResult {
                tools: vec![McpTool {
                    name: "echo".to_string(),
                    description: Some("Echoes text".to_string()),
                    input_schema: Some(json!({
                        "type": "object",
                        "properties": {"text": {"type": "string"}},
                        "required": ["text"]
                    })),
                    annotations: None,
                    meta: None,
                }],
                next_cursor: None,
            })
        );

        let call = process
            .call_tool(
                JsonRpcId::String("call-1".to_string()),
                McpToolCallParams {
                    name: "echo".to_string(),
                    arguments: Some(json!({"text": "hello"})),
                    meta: None,
                },
            )
            .await
            .expect("call tool");
        assert_eq!(call.error, None);
        let call_result = call.result.expect("tool result");
        assert_eq!(call_result.is_error, Some(false));
        assert_eq!(
            call_result.structured_content,
            Some(json!({"echoed": "hello"}))
        );
        assert_eq!(call_result.content.len(), 1);
        assert_eq!(call_result.content[0].kind, "text");
        assert_eq!(
            call_result.content[0].data.get("text"),
            Some(&json!("echo:hello"))
        );

        let resources = process
            .list_resources(JsonRpcId::Number(3), None)
            .await
            .expect("list resources");
        let resources_result = resources.result.expect("resources result");
        assert_eq!(resources_result.resources.len(), 1);
        assert_eq!(resources_result.resources[0].uri, "file://guide.txt");
        assert_eq!(
            resources_result.resources[0].mime_type.as_deref(),
            Some("text/plain")
        );

        let read = process
            .read_resource(
                JsonRpcId::Number(4),
                McpReadResourceParams {
                    uri: "file://guide.txt".to_string(),
                },
            )
            .await
            .expect("read resource");
        assert_eq!(
            read.result,
            Some(McpReadResourceResult {
                contents: vec![super::McpResourceContents {
                    uri: "file://guide.txt".to_string(),
                    mime_type: Some("text/plain".to_string()),
                    text: Some("contents for file://guide.txt".to_string()),
                    blob: None,
                    meta: None,
                }],
            })
        );

        process.terminate().await.expect("terminate child");
        let _ = process.wait().await.expect("wait after kill");
        cleanup_script(&script_path);
    });
}

#[test]
fn surfaces_jsonrpc_errors_from_tool_calls() {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_mcp_server_script();
        let transport = script_transport(&script_path);
        let mut process = McpStdioProcess::spawn(&transport).expect("spawn fake mcp server");

        let response = process
            .call_tool(
                JsonRpcId::Number(9),
                McpToolCallParams {
                    name: "fail".to_string(),
                    arguments: None,
                    meta: None,
                },
            )
            .await
            .expect("call tool with error response");

        assert_eq!(response.id, JsonRpcId::Number(9));
        assert!(response.result.is_none());
        assert_eq!(response.error.as_ref().map(|e| e.code), Some(-32001));
        assert_eq!(
            response.error.as_ref().map(|e| e.message.as_str()),
            Some("tool failed")
        );

        process.terminate().await.expect("terminate child");
        let _ = process.wait().await.expect("wait after kill");
        cleanup_script(&script_path);
    });
}

#[test]
fn manager_discovers_tools_from_stdio_config() {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_manager_mcp_server_script();
        let root = script_path.parent().expect("script parent");
        let log_path = root.join("alpha.log");
        let servers = BTreeMap::from([(
            "alpha".to_string(),
            manager_server_config(&script_path, "alpha", &log_path),
        )]);
        let mut manager = McpServerManager::from_servers(&servers);

        let tools = manager.discover_tools().await.expect("discover tools");

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].server_name, "alpha");
        assert_eq!(tools[0].raw_name, "echo");
        assert_eq!(tools[0].qualified_name, mcp_tool_name("alpha", "echo"));
        assert_eq!(tools[0].tool.name, "echo");
        assert!(manager.unsupported_servers().is_empty());

        manager.shutdown().await.expect("shutdown");
        cleanup_script(&script_path);
    });
}

#[test]
fn manager_reuses_discovered_tool_cache_on_second_discovery() {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_manager_mcp_server_script();
        let root = script_path.parent().expect("script parent");
        let log_path = root.join("alpha.log");
        let servers = BTreeMap::from([(
            "alpha".to_string(),
            manager_server_config(&script_path, "alpha", &log_path),
        )]);
        let mut manager = McpServerManager::from_servers(&servers);

        let first = manager.discover_tools().await.expect("first discover");
        let second = manager.discover_tools().await.expect("second discover");

        assert_eq!(first.len(), 1);
        assert_eq!(second.len(), 1);
        assert_eq!(first[0].qualified_name, second[0].qualified_name);

        let log = fs::read_to_string(&log_path).expect("read log");
        assert_eq!(
            log.lines().collect::<Vec<_>>(),
            vec!["initialize", "tools/list"]
        );

        manager.shutdown().await.expect("shutdown");
        cleanup_script(&script_path);
    });
}

#[test]
fn manager_routes_tool_calls_to_correct_server() {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_manager_mcp_server_script();
        let root = script_path.parent().expect("script parent");
        let alpha_log = root.join("alpha.log");
        let beta_log = root.join("beta.log");
        let servers = BTreeMap::from([
            (
                "alpha".to_string(),
                manager_server_config(&script_path, "alpha", &alpha_log),
            ),
            (
                "beta".to_string(),
                manager_server_config(&script_path, "beta", &beta_log),
            ),
        ]);
        let mut manager = McpServerManager::from_servers(&servers);

        let tools = manager.discover_tools().await.expect("discover tools");
        assert_eq!(tools.len(), 2);

        let alpha = manager
            .call_tool(
                &mcp_tool_name("alpha", "echo"),
                Some(json!({"text": "hello"})),
            )
            .await
            .expect("call alpha tool");
        let beta = manager
            .call_tool(
                &mcp_tool_name("beta", "echo"),
                Some(json!({"text": "world"})),
            )
            .await
            .expect("call beta tool");

        assert_eq!(
            alpha
                .result
                .as_ref()
                .and_then(|result| result.structured_content.as_ref())
                .and_then(|value| value.get("server")),
            Some(&json!("alpha"))
        );
        assert_eq!(
            beta.result
                .as_ref()
                .and_then(|result| result.structured_content.as_ref())
                .and_then(|value| value.get("server")),
            Some(&json!("beta"))
        );

        manager.shutdown().await.expect("shutdown");
        cleanup_script(&script_path);
    });
}

#[test]
fn manager_times_out_slow_tool_calls() {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_mcp_server_script();
        let root = script_path.parent().expect("script parent");
        let log_path = root.join("timeout.log");
        let servers = BTreeMap::from([(
            "slow".to_string(),
            ScopedMcpServerConfig {
                scope: ConfigSource::Local,
                config: McpServerConfig::Stdio(McpStdioServerConfig {
                    command: "python3".to_string(),
                    args: vec![script_path.to_string_lossy().into_owned()],
                    env: BTreeMap::from([(
                        "MCP_TOOL_CALL_DELAY_MS".to_string(),
                        "200".to_string(),
                    )]),
                    tool_call_timeout_ms: Some(25),
                }),
            },
        )]);
        let mut manager = McpServerManager::from_servers(&servers);

        manager.discover_tools().await.expect("discover tools");
        let error = manager
            .call_tool(
                &mcp_tool_name("slow", "echo"),
                Some(json!({"text": "slow"})),
            )
            .await
            .expect_err("slow tool call should time out");

        match error {
            McpServerManagerError::Timeout {
                server_name,
                method,
                timeout_ms,
            } => {
                assert_eq!(server_name, "slow");
                assert_eq!(method, "tools/call");
                assert_eq!(timeout_ms, 25);
            }
            other => panic!("expected timeout error, got {other:?}"),
        }

        manager.shutdown().await.expect("shutdown");
        cleanup_script(&script_path);
        let _ = fs::remove_file(log_path);
    });
}

#[test]
fn manager_surfaces_parse_errors_from_tool_calls() {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_mcp_server_script();
        let servers = BTreeMap::from([(
            "broken".to_string(),
            ScopedMcpServerConfig {
                scope: ConfigSource::Local,
                config: McpServerConfig::Stdio(McpStdioServerConfig {
                    command: "python3".to_string(),
                    args: vec![script_path.to_string_lossy().into_owned()],
                    env: BTreeMap::from([(
                        "MCP_INVALID_TOOL_CALL_RESPONSE".to_string(),
                        "1".to_string(),
                    )]),
                    tool_call_timeout_ms: Some(1_000),
                }),
            },
        )]);
        let mut manager = McpServerManager::from_servers(&servers);

        manager.discover_tools().await.expect("discover tools");
        let error = manager
            .call_tool(
                &mcp_tool_name("broken", "echo"),
                Some(json!({"text": "invalid-json"})),
            )
            .await
            .expect_err("invalid json should fail");

        match error {
            McpServerManagerError::InvalidResponse {
                server_name,
                method,
                details,
            } => {
                assert_eq!(server_name, "broken");
                assert_eq!(method, "tools/call");
                assert!(details.contains("expected ident") || details.contains("expected value"));
            }
            other => panic!("expected invalid response error, got {other:?}"),
        }

        manager.shutdown().await.expect("shutdown");
        cleanup_script(&script_path);
    });
}

#[test]
fn given_child_exits_after_discovery_when_calling_twice_then_second_call_succeeds_after_reset() {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_manager_mcp_server_script();
        let root = script_path.parent().expect("script parent");
        let log_path = root.join("dropping.log");
        let exit_marker = root.join("dropping.exit");
        let servers = BTreeMap::from([(
            "alpha".to_string(),
            manager_server_config_with_env(
                &script_path,
                "alpha",
                &log_path,
                BTreeMap::from([
                    ("MCP_EXIT_AFTER_TOOLS_LIST".to_string(), "1".to_string()),
                    (
                        "MCP_EXIT_MARKER".to_string(),
                        exit_marker.to_string_lossy().into_owned(),
                    ),
                ]),
            ),
        )]);
        let mut manager = McpServerManager::from_servers(&servers);

        manager.discover_tools().await.expect("discover tools");
        for _ in 0..20 {
            if exit_marker.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            exit_marker.exists(),
            "manager server should record exit marker"
        );

        let response = match manager
            .call_tool(
                &mcp_tool_name("alpha", "echo"),
                Some(json!({"text": "reconnect"})),
            )
            .await
        {
            Ok(response) => response,
            Err(McpServerManagerError::Transport {
                server_name,
                method,
                source,
            }) => {
                assert_eq!(server_name, "alpha");
                assert_eq!(method, "tools/call");
                assert!(matches!(
                    source.kind(),
                    ErrorKind::UnexpectedEof | ErrorKind::BrokenPipe
                ));
                manager
                    .call_tool(
                        &mcp_tool_name("alpha", "echo"),
                        Some(json!({"text": "reconnect"})),
                    )
                    .await
                    .expect("second tool call should succeed after reset")
            }
            Err(other) => panic!("expected transport reset path, got {other:?}"),
        };

        assert_eq!(
            response
                .result
                .as_ref()
                .and_then(|result| result.structured_content.as_ref())
                .and_then(|value| value.get("server")),
            Some(&json!("alpha"))
        );
        assert_eq!(
            response
                .result
                .as_ref()
                .and_then(|result| result.structured_content.as_ref())
                .and_then(|value| value.get("initializeCount")),
            Some(&json!(1))
        );
        let log = fs::read_to_string(&log_path).expect("read log");
        assert_eq!(
            log.lines().collect::<Vec<_>>(),
            vec!["initialize", "tools/list", "initialize", "tools/call"]
        );

        manager.shutdown().await.expect("shutdown");
        cleanup_script(&script_path);
    });
}

#[test]
fn given_initialize_hangs_once_when_discover_tools_then_manager_retries_and_succeeds() {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_manager_mcp_server_script();
        let root = script_path.parent().expect("script parent");
        let log_path = root.join("initialize-hang.log");
        let marker_path = root.join("initialize-hang.marker");
        let servers = BTreeMap::from([(
            "alpha".to_string(),
            manager_server_config_with_env(
                &script_path,
                "alpha",
                &log_path,
                BTreeMap::from([
                    (
                        "MCP_FAIL_ONCE_MODE".to_string(),
                        "initialize_hang".to_string(),
                    ),
                    (
                        "MCP_FAIL_ONCE_MARKER".to_string(),
                        marker_path.to_string_lossy().into_owned(),
                    ),
                ]),
            ),
        )]);
        let mut manager = McpServerManager::from_servers(&servers);

        let tools = manager
            .discover_tools()
            .await
            .expect("discover tools after retry");

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].qualified_name, mcp_tool_name("alpha", "echo"));
        let log = fs::read_to_string(&log_path).expect("read log");
        assert_eq!(
            log.lines().collect::<Vec<_>>(),
            vec!["initialize", "initialize-hang", "initialize", "tools/list"]
        );

        manager.shutdown().await.expect("shutdown");
        cleanup_script(&script_path);
    });
}

#[test]
fn given_initialize_always_times_out_when_rediscovered_then_respawns_are_bounded() {
    // Regression: a server whose handshake never completes within the timeout
    // (the interactive `mcp-remote` OAuth case — the user has not finished
    // authenticating in the browser) used to be killed and respawned on *every*
    // timeout. Each respawn relaunches the OAuth browser tab, so the user could
    // never finish. The manager now bounds consecutive timeout-induced respawns
    // of a still-alive child, so repeated discovery passes spawn the child a
    // small bounded number of times instead of once per pass forever.
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_manager_mcp_server_script();
        let root = script_path.parent().expect("script parent");
        let log_path = root.join("oauth-hang.log");
        let spawn_log = root.join("oauth-hang.spawns");
        let servers = BTreeMap::from([(
            "atlassian".to_string(),
            manager_server_config_with_env(
                &script_path,
                "atlassian",
                &log_path,
                BTreeMap::from([
                    ("MCP_INITIALIZE_HANG_ALWAYS".to_string(), "1".to_string()),
                    (
                        "MCP_SPAWN_LOG".to_string(),
                        spawn_log.to_string_lossy().into_owned(),
                    ),
                ]),
            ),
        )]);
        let mut manager = McpServerManager::from_servers(&servers);

        // Several discovery passes, as the background discovery + per-turn
        // inbound refresh would drive over a session. Every pass fails (the
        // handshake never completes), but the respawns must not grow without
        // bound.
        for _ in 0..6 {
            let _ = manager.discover_tools().await;
        }

        let spawn_count = fs::read_to_string(&spawn_log)
            .map(|log| log.lines().filter(|line| *line == "spawn").count())
            .unwrap_or(0);
        assert!(
            spawn_count <= 4,
            "a never-authenticating OAuth server must not respawn (relaunch its browser) \
             once per discovery pass forever; got {spawn_count} spawns across 6 passes"
        );

        manager.shutdown().await.expect("shutdown");
        cleanup_script(&script_path);
    });
}

#[test]
fn given_oauth_bridge_initialize_times_out_then_child_is_never_respawned() {
    // Regression for the "three browser windows" report: an `mcp-remote`
    // bridge whose `initialize` blocks on browser OAuth was killed and
    // respawned by the discovery pass's timeout retry (twice — the respawn
    // budget only kicked in afterwards), then once more by the first
    // on-demand refresh. Each spawn opened another auth tab, and only the
    // last one still had a live callback server. A known interactive OAuth
    // bridge must keep its single live child (and its single tab) from the
    // very first timeout: across discovery + a later refresh, exactly one
    // spawn.
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_manager_mcp_server_script();
        let root = script_path.parent().expect("script parent");
        let log_path = root.join("oauth-bridge-hang.log");
        let spawn_log = root.join("oauth-bridge-hang.spawns");
        let env = BTreeMap::from([
            ("MCP_SERVER_LABEL".to_string(), "atlassian".to_string()),
            (
                "MCP_LOG_PATH".to_string(),
                log_path.to_string_lossy().into_owned(),
            ),
            ("MCP_INITIALIZE_HANG_ALWAYS".to_string(), "1".to_string()),
            (
                "MCP_SPAWN_LOG".to_string(),
                spawn_log.to_string_lossy().into_owned(),
            ),
        ]);
        let servers = BTreeMap::from([(
            "atlassian".to_string(),
            ScopedMcpServerConfig {
                scope: ConfigSource::Local,
                config: McpServerConfig::Stdio(McpStdioServerConfig {
                    command: "python3".to_string(),
                    // The trailing `mcp-remote` arg marks this config as an
                    // interactive OAuth bridge (`is_stdio_remote_bridge`
                    // matches any arg basename); the fixture script ignores
                    // extra argv, so behavior is unchanged.
                    args: vec![
                        script_path.to_string_lossy().into_owned(),
                        "mcp-remote".to_string(),
                    ],
                    env,
                    tool_call_timeout_ms: None,
                }),
            },
        )]);
        let mut manager = McpServerManager::from_servers(&servers);

        // Startup discovery (with its internal timeout retry), then the
        // on-demand refresh a model tool call would drive — the exact
        // sequence that used to stack three browser windows.
        let _ = manager.discover_tools().await;
        let _ = manager.refresh_server_tools("atlassian").await;

        let spawn_count = fs::read_to_string(&spawn_log)
            .map(|log| log.lines().filter(|line| *line == "spawn").count())
            .unwrap_or(0);
        assert_eq!(
            spawn_count, 1,
            "an interactive OAuth bridge must keep its single live child \
             (one auth tab) across discovery retries and on-demand refreshes"
        );

        manager.shutdown().await.expect("shutdown");
        cleanup_script(&script_path);
    });
}

#[test]
fn given_tool_call_disconnects_once_when_calling_tool_then_manager_retries_and_call_succeeds() {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_manager_mcp_server_script();
        let root = script_path.parent().expect("script parent");
        let log_path = root.join("tool-call-disconnect.log");
        let marker_path = root.join("tool-call-disconnect.marker");
        let servers = BTreeMap::from([(
            "alpha".to_string(),
            manager_server_config_with_env(
                &script_path,
                "alpha",
                &log_path,
                BTreeMap::from([
                    (
                        "MCP_FAIL_ONCE_MODE".to_string(),
                        "tool_call_disconnect".to_string(),
                    ),
                    (
                        "MCP_FAIL_ONCE_MARKER".to_string(),
                        marker_path.to_string_lossy().into_owned(),
                    ),
                ]),
            ),
        )]);
        let mut manager = McpServerManager::from_servers(&servers);

        manager.discover_tools().await.expect("discover tools");
        let response = manager
            .call_tool(
                &mcp_tool_name("alpha", "echo"),
                Some(json!({"text": "retried"})),
            )
            .await
            .expect("tool call should retry once after transport drop");

        assert_eq!(
            response
                .result
                .as_ref()
                .and_then(|result| result.structured_content.as_ref())
                .and_then(|value| value.get("echoed")),
            Some(&json!("retried"))
        );
        let log = fs::read_to_string(&log_path).expect("read log");
        assert_eq!(
            log.lines().collect::<Vec<_>>(),
            vec![
                "initialize",
                "tools/list",
                "tools/call",
                "tools/call-disconnect",
                "initialize",
                "tools/call",
            ]
        );

        manager.shutdown().await.expect("shutdown");
        cleanup_script(&script_path);
    });
}

#[test]
fn manager_rejects_duplicate_qualified_tool_routes() {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_manager_mcp_server_script();
        let root = script_path.parent().expect("script parent");
        let first_log = root.join("collision-first.log");
        let second_log = root.join("collision-second.log");
        let servers = BTreeMap::from([
            (
                "a.b".to_string(),
                manager_server_config(&script_path, "a.b", &first_log),
            ),
            (
                "a_b".to_string(),
                manager_server_config(&script_path, "a_b", &second_log),
            ),
        ]);
        let mut manager = McpServerManager::from_servers(&servers);

        let error = manager
            .discover_tools()
            .await
            .expect_err("normalized duplicate route should fail discovery");
        match error {
            McpServerManagerError::DuplicateToolRoute {
                qualified_name,
                existing_server,
                existing_raw_name,
                new_server,
                new_raw_name,
            } => {
                assert_eq!(qualified_name, mcp_tool_name("a_b", "echo"));
                assert_eq!(existing_server, "a.b");
                assert_eq!(existing_raw_name, "echo");
                assert_eq!(new_server, "a_b");
                assert_eq!(new_raw_name, "echo");
            }
            other => panic!("expected duplicate route error, got {other:?}"),
        }

        manager.shutdown().await.expect("shutdown");
        cleanup_script(&script_path);
    });
}

#[test]
fn manager_lists_and_reads_resources_from_stdio_servers() {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_mcp_server_script();
        let root = script_path.parent().expect("script parent");
        let log_path = root.join("resources.log");
        let servers = BTreeMap::from([(
            "alpha".to_string(),
            manager_server_config(&script_path, "alpha", &log_path),
        )]);
        let mut manager = McpServerManager::from_servers(&servers);

        let listed = manager
            .list_resources("alpha")
            .await
            .expect("list resources");
        assert_eq!(listed.resources.len(), 1);
        assert_eq!(listed.resources[0].uri, "file://guide.txt");

        let read = manager
            .read_resource("alpha", "file://guide.txt")
            .await
            .expect("read resource");
        assert_eq!(read.contents.len(), 1);
        assert_eq!(
            read.contents[0].text.as_deref(),
            Some("contents for file://guide.txt")
        );

        manager.shutdown().await.expect("shutdown");
        cleanup_script(&script_path);
    });
}

#[test]
fn manager_lists_and_gets_prompts_from_stdio_servers() {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_mcp_server_script();
        let root = script_path.parent().expect("script parent");
        let log_path = root.join("prompts.log");
        let servers = BTreeMap::from([(
            "alpha".to_string(),
            manager_server_config(&script_path, "alpha", &log_path),
        )]);
        let mut manager = McpServerManager::from_servers(&servers);

        // Pagination: page 1 carries `review` + nextCursor, page 2 `plain`.
        let prompts = manager.list_prompts("alpha").await.expect("list prompts");
        assert_eq!(
            prompts
                .iter()
                .map(|prompt| prompt.name.as_str())
                .collect::<Vec<_>>(),
            vec!["review", "plain"],
            "paginated prompts/list pages are concatenated in order"
        );
        assert_eq!(prompts[0].arguments.len(), 2);
        assert_eq!(prompts[0].arguments[0].name, "path");
        assert_eq!(prompts[0].arguments[0].required, Some(true));
        assert_eq!(prompts[1].arguments.len(), 0, "missing arguments → empty");

        let resolved = manager
            .get_prompt(
                "alpha",
                "review",
                Some(serde_json::json!({ "path": "src/main.rs" })),
            )
            .await
            .expect("get prompt");
        assert_eq!(resolved.description.as_deref(), Some("resolved review"));
        assert_eq!(resolved.messages.len(), 1);
        assert_eq!(resolved.messages[0].role, "user");
        assert_eq!(
            resolved.messages[0]
                .content
                .get("text")
                .and_then(serde_json::Value::as_str),
            Some("Review src/main.rs"),
            "server-side argument substitution flows back"
        );

        manager.shutdown().await.expect("shutdown");
        cleanup_script(&script_path);
    });
}

#[test]
fn manager_maps_prompts_method_not_found_to_empty_list() {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        // The manager-focused script answers any unknown method (including
        // `prompts/list`) with JSON-RPC -32601 — exactly the shape a server
        // without the prompts capability produces.
        let script_path = write_manager_mcp_server_script();
        let root = script_path.parent().expect("script parent");
        let log_path = root.join("no-prompts.log");
        let servers = BTreeMap::from([(
            "alpha".to_string(),
            manager_server_config(&script_path, "alpha", &log_path),
        )]);
        let mut manager = McpServerManager::from_servers(&servers);

        let prompts = manager
            .list_prompts("alpha")
            .await
            .expect("method-not-found must not surface as an error");
        assert!(
            prompts.is_empty(),
            "-32601 maps to an empty prompt list, got {prompts:?}"
        );

        manager.shutdown().await.expect("shutdown");
        cleanup_script(&script_path);
    });
}

#[test]
fn manager_discovery_report_keeps_healthy_servers_when_one_server_fails() {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_manager_mcp_server_script();
        let root = script_path.parent().expect("script parent");
        let alpha_log = root.join("alpha.log");
        let servers = BTreeMap::from([
            (
                "alpha".to_string(),
                manager_server_config(&script_path, "alpha", &alpha_log),
            ),
            (
                "broken".to_string(),
                ScopedMcpServerConfig {
                    scope: ConfigSource::Local,
                    config: McpServerConfig::Stdio(McpStdioServerConfig {
                        command: "python3".to_string(),
                        args: vec!["-c".to_string(), "import sys; sys.exit(0)".to_string()],
                        env: BTreeMap::new(),
                        tool_call_timeout_ms: None,
                    }),
                },
            ),
        ]);
        let mut manager = McpServerManager::from_servers(&servers);

        let report = manager.discover_tools_best_effort().await;

        assert_eq!(report.tools.len(), 1);
        assert_eq!(
            report.tools[0].qualified_name,
            mcp_tool_name("alpha", "echo")
        );
        assert_eq!(report.failed_servers.len(), 1);
        assert_eq!(report.failed_servers[0].server_name, "broken");
        assert_eq!(
            report.failed_servers[0].phase,
            McpLifecyclePhase::InitializeHandshake
        );
        assert!(!report.failed_servers[0].recoverable);
        assert_eq!(
            report.failed_servers[0]
                .context
                .get("method")
                .map(String::as_str),
            Some("initialize")
        );
        assert!(report.failed_servers[0].error.contains("initialize"));
        let degraded = report
            .degraded_startup
            .as_ref()
            .expect("partial startup should surface degraded report");
        assert_eq!(degraded.working_servers, vec!["alpha".to_string()]);
        assert_eq!(degraded.failed_servers.len(), 1);
        assert_eq!(degraded.failed_servers[0].server_name, "broken");
        assert_eq!(
            degraded.failed_servers[0].phase,
            McpLifecyclePhase::InitializeHandshake
        );
        assert_eq!(
            degraded.available_tools,
            vec![mcp_tool_name("alpha", "echo")]
        );
        assert!(degraded.missing_tools.is_empty());

        let response = manager
            .call_tool(&mcp_tool_name("alpha", "echo"), Some(json!({"text": "ok"})))
            .await
            .expect("healthy server should remain callable");
        assert_eq!(
            response
                .result
                .as_ref()
                .and_then(|result| result.structured_content.as_ref())
                .and_then(|value| value.get("echoed")),
            Some(&json!("ok"))
        );

        manager.shutdown().await.expect("shutdown");
        cleanup_script(&script_path);
    });
}

#[test]
fn manager_records_only_truly_unsupported_servers_without_panicking() {
    let servers = BTreeMap::from([
        (
            "http".to_string(),
            ScopedMcpServerConfig {
                scope: ConfigSource::Local,
                config: McpServerConfig::Http(McpRemoteServerConfig {
                    url: "https://example.test/mcp".to_string(),
                    headers: BTreeMap::new(),
                    headers_helper: None,
                    oauth: None,
                }),
            },
        ),
        (
            "sdk".to_string(),
            ScopedMcpServerConfig {
                scope: ConfigSource::Local,
                config: McpServerConfig::Sdk(McpSdkServerConfig {
                    name: "sdk-server".to_string(),
                }),
            },
        ),
        (
            "ws".to_string(),
            ScopedMcpServerConfig {
                scope: ConfigSource::Local,
                config: McpServerConfig::Ws(McpWebSocketServerConfig {
                    url: "wss://example.test/mcp".to_string(),
                    headers: BTreeMap::new(),
                    headers_helper: None,
                }),
            },
        ),
    ]);

    let manager = McpServerManager::from_servers(&servers);
    let unsupported = manager.unsupported_servers();

    assert_eq!(unsupported.len(), 1);
    assert_eq!(unsupported[0].server_name, "sdk");
    assert_eq!(
        unsupported_server_failed_server(&unsupported[0]).phase,
        McpLifecyclePhase::ServerRegistration
    );
}

#[test]
fn manager_shutdown_terminates_spawned_children_and_is_idempotent() {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_manager_mcp_server_script();
        let root = script_path.parent().expect("script parent");
        let log_path = root.join("alpha.log");
        let servers = BTreeMap::from([(
            "alpha".to_string(),
            manager_server_config(&script_path, "alpha", &log_path),
        )]);
        let mut manager = McpServerManager::from_servers(&servers);

        manager.discover_tools().await.expect("discover tools");
        manager.shutdown().await.expect("first shutdown");
        manager.shutdown().await.expect("second shutdown");

        cleanup_script(&script_path);
    });
}

#[test]
fn manager_reuses_spawned_server_between_discovery_and_call() {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_manager_mcp_server_script();
        let root = script_path.parent().expect("script parent");
        let log_path = root.join("alpha.log");
        let servers = BTreeMap::from([(
            "alpha".to_string(),
            manager_server_config(&script_path, "alpha", &log_path),
        )]);
        let mut manager = McpServerManager::from_servers(&servers);

        manager.discover_tools().await.expect("discover tools");
        let response = manager
            .call_tool(
                &mcp_tool_name("alpha", "echo"),
                Some(json!({"text": "reuse"})),
            )
            .await
            .expect("call tool");

        assert_eq!(
            response
                .result
                .as_ref()
                .and_then(|result| result.structured_content.as_ref())
                .and_then(|value| value.get("initializeCount")),
            Some(&json!(1))
        );

        let log = fs::read_to_string(&log_path).expect("read log");
        assert_eq!(log.lines().filter(|line| *line == "initialize").count(), 1);
        assert_eq!(
            log.lines().collect::<Vec<_>>(),
            vec!["initialize", "tools/list", "tools/call"]
        );

        manager.shutdown().await.expect("shutdown");
        cleanup_script(&script_path);
    });
}

#[test]
fn manager_reports_unknown_qualified_tool_name() {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_manager_mcp_server_script();
        let root = script_path.parent().expect("script parent");
        let log_path = root.join("alpha.log");
        let servers = BTreeMap::from([(
            "alpha".to_string(),
            manager_server_config(&script_path, "alpha", &log_path),
        )]);
        let mut manager = McpServerManager::from_servers(&servers);

        let error = manager
            .call_tool(
                &mcp_tool_name("alpha", "missing"),
                Some(json!({"text": "nope"})),
            )
            .await
            .expect_err("unknown qualified tool should fail");

        match error {
            McpServerManagerError::UnknownTool { qualified_name } => {
                assert_eq!(qualified_name, mcp_tool_name("alpha", "missing"));
            }
            other => panic!("expected unknown tool error, got {other:?}"),
        }

        cleanup_script(&script_path);
    });
}

#[test]
fn oauth_server_name_surfaces_the_name_for_remote_servers() {
    use crate::config::McpOAuthConfig;
    use crate::mcp_client::McpClientTransport;

    fn sse_bootstrap(name: &str, oauth: Option<McpOAuthConfig>) -> McpClientBootstrap {
        McpClientBootstrap::from_scoped_config(
            name,
            &ScopedMcpServerConfig {
                scope: ConfigSource::User,
                config: McpServerConfig::Sse(McpRemoteServerConfig {
                    url: "https://mcp.example/sse".to_string(),
                    headers: BTreeMap::new(),
                    headers_helper: None,
                    oauth,
                }),
            },
        )
    }

    // Explicit OAuth: the server name is surfaced as the bearer token's key.
    let paid = sse_bootstrap(
        "paid-server",
        Some(McpOAuthConfig {
            client_id: None,
            callback_port: None,
            auth_server_metadata_url: None,
            xaa: None,
        }),
    );
    assert!(matches!(&paid.transport, McpClientTransport::Sse(_)));
    assert_eq!(
        super::oauth_server_name(&paid),
        "paid-server",
        "an explicit-OAuth server passes its name so the bearer token can be attached",
    );

    // No explicit `oauth`: still surface the name so a token obtained via native
    // discovery (stored under the server name) is injected on later requests.
    let free = sse_bootstrap("free-server", None);
    assert!(matches!(&free.transport, McpClientTransport::Sse(_)));
    assert_eq!(
        super::oauth_server_name(&free),
        "free-server",
        "a discovery-capable server surfaces its name so a cached token is injected",
    );
}

#[test]
fn paginated_listing_aborts_when_the_cursor_never_advances() {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_manager_mcp_server_script();
        let parent = script_path.parent().expect("script parent");
        let log = parent.join("stuck.log");

        let mut servers = BTreeMap::new();
        servers.insert(
            "stuck".to_string(),
            manager_server_config_with_env(
                &script_path,
                "stuck",
                &log,
                BTreeMap::from([("MCP_STUCK_CURSOR".to_string(), "1".to_string())]),
            ),
        );
        let mut manager = McpServerManager::from_servers(&servers);

        // The server hands back the same `nextCursor` forever. Discovery must
        // detect the lack of progress and error out rather than paging — and
        // allocating — without end.
        let error = manager
            .refresh_server_tools("stuck")
            .await
            .expect_err("a non-advancing cursor must abort, not loop");
        match error {
            McpServerManagerError::InvalidResponse { details, .. } => assert!(
                details.contains("did not advance"),
                "expected a cursor-progress error, got: {details}"
            ),
            other => panic!("expected InvalidResponse, got {other:?}"),
        }

        cleanup_script(&script_path);
    });
}

fn remote_http_server_config(url: &str) -> ScopedMcpServerConfig {
    ScopedMcpServerConfig {
        scope: ConfigSource::User,
        config: McpServerConfig::Http(McpRemoteServerConfig {
            url: url.to_string(),
            headers: BTreeMap::new(),
            headers_helper: None,
            oauth: None,
        }),
    }
}

#[test]
fn discovery_class_splits_stdio_bridge_from_remote() {
    // stdio (even an `npx mcp-remote` OAuth bridge) is a local child → Stdio
    // class; only true network transports are Remote. This drives the per-class
    // concurrency cap (stdio 3 / remote 20).
    let servers = BTreeMap::from([
        (
            "local".to_string(),
            manager_server_config(&PathBuf::from("/bin/true"), "local", &PathBuf::from("/dev/null")),
        ),
        (
            "bridge".to_string(),
            ScopedMcpServerConfig {
                scope: ConfigSource::User,
                config: McpServerConfig::Stdio(McpStdioServerConfig {
                    command: "npx".to_string(),
                    args: vec!["-y".to_string(), "mcp-remote".to_string()],
                    env: BTreeMap::new(),
                    tool_call_timeout_ms: None,
                }),
            },
        ),
        (
            "remote".to_string(),
            remote_http_server_config("https://mcp.context7.com/mcp"),
        ),
    ]);
    let manager = McpServerManager::from_servers(&servers);

    assert_eq!(manager.discovery_class_for("local"), McpDiscoveryClass::Stdio);
    assert_eq!(manager.discovery_class_for("bridge"), McpDiscoveryClass::Stdio);
    assert_eq!(manager.discovery_class_for("remote"), McpDiscoveryClass::Remote);
    // An unknown server defaults to the tighter Stdio class.
    assert_eq!(manager.discovery_class_for("missing"), McpDiscoveryClass::Stdio);
}

#[test]
fn initialize_timeout_ms_for_orders_fast_local_before_slow_bridge() {
    let servers = BTreeMap::from([
        (
            "local".to_string(),
            manager_server_config(&PathBuf::from("/bin/true"), "local", &PathBuf::from("/dev/null")),
        ),
        (
            "bridge".to_string(),
            ScopedMcpServerConfig {
                scope: ConfigSource::User,
                config: McpServerConfig::Stdio(McpStdioServerConfig {
                    command: "npx".to_string(),
                    args: vec!["-y".to_string(), "mcp-remote".to_string()],
                    env: BTreeMap::new(),
                    tool_call_timeout_ms: None,
                }),
            },
        ),
    ]);
    let manager = McpServerManager::from_servers(&servers);

    // Fast local stdio sorts before the slow OAuth bridge — the fast-first key.
    assert!(
        manager.initialize_timeout_ms_for("local") < manager.initialize_timeout_ms_for("bridge"),
        "local {} should be < bridge {}",
        manager.initialize_timeout_ms_for("local"),
        manager.initialize_timeout_ms_for("bridge"),
    );
    // Unknown servers sort last.
    assert_eq!(manager.initialize_timeout_ms_for("missing"), u64::MAX);
}

#[test]
fn detach_then_absorb_round_trips_discovery_routing() {
    // The off-lock concurrent path detaches a server into its own manager,
    // discovers there, then absorbs the live connection + routes back. After
    // absorb the parent manager must route the discovered tool exactly as the
    // serial `refresh_server_tools` path would.
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_manager_mcp_server_script();
        let parent = script_path.parent().expect("script parent");
        let servers = BTreeMap::from([(
            "alpha".to_string(),
            manager_server_config(&script_path, "alpha", &parent.join("alpha.log")),
        )]);
        let mut manager = McpServerManager::from_servers(&servers);

        let mut detached = manager
            .detach_for_discovery("alpha")
            .expect("detach alpha");
        // The parent no longer routes alpha while it is detached.
        assert!(manager.server_names().is_empty());

        let fresh = detached
            .refresh_server_tools("alpha")
            .await
            .expect("discover on detached unit");
        assert_eq!(fresh.len(), 1);

        manager
            .absorb_discovered("alpha", detached, &fresh)
            .expect("absorb alpha");
        assert_eq!(manager.server_names(), vec!["alpha".to_string()]);

        // The absorbed route is live: the tool call reaches the connection.
        let echo = mcp_tool_name("alpha", "echo");
        manager
            .call_tool(&echo, Some(json!({ "text": "hi" })))
            .await
            .expect("absorbed tool routes");

        manager.shutdown().await.expect("shutdown");
        cleanup_script(&script_path);
    });
}

#[test]
fn reattach_after_failed_discovery_leaves_routes_untouched() {
    // A detached unit whose discovery failed is reattached WITHOUT installing
    // routes — the same terminal state the serial path produced (the server
    // returns to the manager for a later retry, advertising nothing).
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_manager_mcp_server_script();
        let parent = script_path.parent().expect("script parent");
        let servers = BTreeMap::from([(
            "alpha".to_string(),
            manager_server_config(&script_path, "alpha", &parent.join("alpha.log")),
        )]);
        let mut manager = McpServerManager::from_servers(&servers);

        let detached = manager
            .detach_for_discovery("alpha")
            .expect("detach alpha");
        manager.reattach_detached("alpha", detached);

        // The entry is back (retryable) but advertises no routes.
        assert_eq!(manager.server_names(), vec!["alpha".to_string()]);
        assert!(manager.qualified_tool_names_for_server("alpha").is_empty());

        manager.shutdown().await.expect("shutdown");
        cleanup_script(&script_path);
    });
}

#[test]
fn concurrent_discovery_does_not_serialize_a_slow_server_behind_a_fast_one() {
    // Six independent 100ms tools/list delays take at least 600ms when run
    // serially. Detached ownership lets `buffer_unordered` overlap them while
    // keeping every request comfortably below the 300ms test RPC timeout.
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        use futures_util::stream::{self, StreamExt};

        let script_path = write_manager_mcp_server_script();
        let parent = script_path.parent().expect("script parent");
        let names = ["alpha", "beta", "gamma", "delta", "epsilon", "zeta"];

        let mut servers = BTreeMap::new();
        for name in names {
            servers.insert(
                name.to_string(),
                manager_server_config_with_env(
                    &script_path,
                    name,
                    &parent.join(format!("{name}.log")),
                    BTreeMap::from([(
                        "MCP_LIST_TOOLS_DELAY_MS".to_string(),
                        "100".to_string(),
                    )]),
                ),
            );
        }
        let mut manager = McpServerManager::from_servers(&servers);
        manager.set_discover_server_timeout_ms(1_000);

        let mut units = names
            .into_iter()
            .map(|name| {
                let unit = manager
                    .detach_for_discovery(name)
                    .unwrap_or_else(|| panic!("detach {name}"));
                (name, unit)
            })
            .collect::<Vec<_>>();

        let started = std::time::Instant::now();
        let results = stream::iter(units.drain(..).map(|(name, mut unit)| async move {
            let fresh = unit.refresh_server_tools(name).await;
            (name, unit, fresh)
        }))
        .buffer_unordered(names.len())
        .collect::<Vec<_>>()
        .await;
        let elapsed = started.elapsed();

        for (name, mut unit, fresh) in results {
            assert!(fresh.is_ok(), "{name} discovery failed: {fresh:?}");
            let _ = unit.shutdown().await;
        }

        assert!(
            elapsed < std::time::Duration::from_millis(500),
            "concurrent discovery took {elapsed:?}; expected it not to serialize six servers"
        );

        manager.shutdown().await.expect("shutdown");
        cleanup_script(&script_path);
    });
}

/// Regression: an interactive OAuth bridge (`mcp-remote`) that never completes
/// browser auth (initialize always hangs) is spawned exactly once. If its
/// process later exits, subsequent calls must NOT re-spawn — the original
/// browser tab is still open and a re-spawn would open another.
#[test]
fn given_oauth_bridge_never_initializes_then_respawn_blocked() {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_manager_mcp_server_script();
        let root = script_path.parent().expect("script parent");
        let spawn_log = root.join("oauth-never-init.spawns");
        let env = BTreeMap::from([
            ("MCP_SERVER_LABEL".to_string(), "atlassian".to_string()),
            ("MCP_INITIALIZE_HANG_ALWAYS".to_string(), "1".to_string()),
            (
                "MCP_SPAWN_LOG".to_string(),
                spawn_log.to_string_lossy().into_owned(),
            ),
        ]);
        let servers = BTreeMap::from([(
            "atlassian".to_string(),
            ScopedMcpServerConfig {
                scope: ConfigSource::Local,
                config: McpServerConfig::Stdio(McpStdioServerConfig {
                    command: "python3".to_string(),
                    args: vec![
                        script_path.to_string_lossy().into_owned(),
                        "mcp-remote".to_string(),
                    ],
                    env,
                    tool_call_timeout_ms: None,
                }),
            },
        )]);
        let mut manager = McpServerManager::from_servers(&servers);

        // First discovery spawns the OAuth bridge but initialize never
        // completes (auth still pending). The process hangs; discovery
        // eventually times out but the live process is preserved.
        let _ = manager.discover_tools().await;

        // Simulate the process dying (OS kill, crash, etc.) while auth is
        // still pending. Shutdown kills the child.
        manager.shutdown().await.expect("shutdown");

        // A subsequent attempt must NOT re-spawn. The browser tab from the
        // first spawn is still open; a second spawn would open another.
        let refresh_result = manager.refresh_server_tools("atlassian").await;
        match refresh_result {
            Err(McpServerManagerError::InvalidResponse { details, .. }) => {
                assert!(
                    details.contains("browser"),
                    "error should mention browser auth, got: {details}"
                );
            }
            other => {
                panic!(
                    "expected InvalidResponse about browser auth, got: {other:?}"
                );
            }
        }

        // Exactly one spawn across both attempts.
        let spawn_count = fs::read_to_string(&spawn_log)
            .map(|log| log.lines().filter(|line| *line == "spawn").count())
            .unwrap_or(0);
        assert_eq!(
            spawn_count, 1,
            "auth-pending OAuth bridge must not re-spawn — \
             a re-spawn opens another browser tab"
        );
    });
}

/// An alive-and-initialized OAuth bridge must remain fully usable after its
/// initial discovery: `refresh_server_tools` succeeds without a second spawn,
/// reusing the live process.
#[test]
fn given_oauth_bridge_stays_alive_then_refresh_succeeds() {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_manager_mcp_server_script();
        let root = script_path.parent().expect("script parent");
        let spawn_log = root.join("oauth-alive-refresh.spawns");
        let env = BTreeMap::from([
            ("MCP_SERVER_LABEL".to_string(), "atlassian".to_string()),
            (
                "MCP_SPAWN_LOG".to_string(),
                spawn_log.to_string_lossy().into_owned(),
            ),
        ]);
        let servers = BTreeMap::from([(
            "atlassian".to_string(),
            ScopedMcpServerConfig {
                scope: ConfigSource::Local,
                config: McpServerConfig::Stdio(McpStdioServerConfig {
                    command: "python3".to_string(),
                    args: vec![
                        script_path.to_string_lossy().into_owned(),
                        "mcp-remote".to_string(),
                    ],
                    env,
                    tool_call_timeout_ms: None,
                }),
            },
        )]);
        let mut manager = McpServerManager::from_servers(&servers);

        let first = manager
            .discover_tools()
            .await
            .expect("first discover_tools");
        assert!(
            first.iter().any(|t| t.raw_name == "echo"),
            "bridge should serve echo tool: {first:?}"
        );

        // Refresh while the process is still alive: must succeed without a
        // second spawn. The OAuth bridge guard must NOT fire because
        // needs_spawn is false (the live child is still running).
        let refreshed = manager
            .refresh_server_tools("atlassian")
            .await
            .expect("refresh of alive OAuth bridge should succeed");
        assert!(
            refreshed.iter().any(|t| t.raw_name == "echo"),
            "refresh should still serve echo: {refreshed:?}"
        );

        let spawn_count = fs::read_to_string(&spawn_log)
            .map(|log| log.lines().filter(|line| *line == "spawn").count())
            .unwrap_or(0);
        assert_eq!(
            spawn_count, 1,
            "alive OAuth bridge must not re-spawn on refresh; \
             got {spawn_count} spawns"
        );

        manager.shutdown().await.expect("shutdown");
        cleanup_script(&script_path);
    });
}

/// After a successful OAuth handshake (initialize completed), the bridge can
/// recover from a process exit by re-spawning normally. mcp-remote has cached
/// OAuth tokens at this point, so a re-spawn does NOT open a new browser tab.
#[test]
fn given_oauth_bridge_crashes_after_auth_then_recovery_respawns() {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_manager_mcp_server_script();
        let root = script_path.parent().expect("script parent");
        let exit_marker = root.join("oauth-recovery.exit");
        let spawn_log = root.join("oauth-recovery.spawns");
        let env = BTreeMap::from([
            ("MCP_SERVER_LABEL".to_string(), "atlassian".to_string()),
            ("MCP_EXIT_AFTER_TOOLS_LIST".to_string(), "1".to_string()),
            (
                "MCP_EXIT_MARKER".to_string(),
                exit_marker.to_string_lossy().into_owned(),
            ),
            (
                "MCP_SPAWN_LOG".to_string(),
                spawn_log.to_string_lossy().into_owned(),
            ),
        ]);
        let servers = BTreeMap::from([(
            "atlassian".to_string(),
            ScopedMcpServerConfig {
                scope: ConfigSource::Local,
                config: McpServerConfig::Stdio(McpStdioServerConfig {
                    command: "python3".to_string(),
                    args: vec![
                        script_path.to_string_lossy().into_owned(),
                        "mcp-remote".to_string(),
                    ],
                    env,
                    tool_call_timeout_ms: None,
                }),
            },
        )]);
        let mut manager = McpServerManager::from_servers(&servers);

        // First discovery: auth completes (initialize succeeds), tools are
        // listed, then the process exits. The flag is cleared on initialize
        // success, so a post-auth crash can recover.
        let first = manager
            .discover_tools()
            .await
            .expect("first discover_tools");
        assert!(
            first.iter().any(|t| t.raw_name == "echo"),
            "bridge should serve echo: {first:?}"
        );

        // Wait for the mock process to finish exiting.
        for _ in 0..50 {
            if exit_marker.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(exit_marker.exists(), "exit marker should be written");

        // Refresh after auth: re-spawn IS allowed because mcp-remote has
        // cached OAuth tokens and won't open another browser tab.
        let refreshed = manager
            .refresh_server_tools("atlassian")
            .await
            .expect("refresh after auth should succeed with re-spawn");
        assert!(
            refreshed.iter().any(|t| t.raw_name == "echo"),
            "recovery refresh should serve echo: {refreshed:?}"
        );

        // Two spawns: one for initial auth, one for recovery.
        let spawn_count = fs::read_to_string(&spawn_log)
            .map(|log| log.lines().filter(|line| *line == "spawn").count())
            .unwrap_or(0);
        assert_eq!(
            spawn_count, 2,
            "authenticated bridge should re-spawn once for recovery; \
             got {spawn_count} spawns (expected 2: initial + recovery)"
        );

        manager.shutdown().await.expect("shutdown");
        cleanup_script(&script_path);
    });
}
