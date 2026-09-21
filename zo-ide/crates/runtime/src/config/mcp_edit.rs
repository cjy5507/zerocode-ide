//! Writing the `mcpServers` section of one settings document, and the record
//! of the project-scoped servers an operator consented to.
//!
//! The inverse of `parsers::parse_mcp_server_config`, spelled with
//! `parsers::mcp_keys` — the very table the parser matches on — so a
//! server `zo mcp add` writes is a server [`ConfigLoader`](super::ConfigLoader)
//! reads back. A second copy of those spellings here would be a file the
//! writer is happy with and the loader cannot see.
//!
//! The shape follows [`persist_allow_always_rules`](super::persist_allow_always_rules):
//! parse the existing
//! document with the loader's own parser, edit the tree, render, and read the
//! candidate back **through the parser a future session uses** before any byte
//! reaches disk. Every other key in the document — the person's `model`,
//! `smart.*`, `hooks`, the other servers — is carried through untouched.

use std::collections::BTreeMap;
use std::path::Path;

use core_types::paths::{write_private_file, ParentDirPolicy};

use crate::json::JsonValue;

use super::parsers::{
    infer_mcp_server_type, mcp_keys, mcp_server_context, parse_json_object_contents,
    parse_mcp_server_config, parse_trusted_mcp_server_names,
};
use super::{
    ConfigError, McpManagedProxyServerConfig, McpOAuthConfig, McpRemoteServerConfig,
    McpSdkServerConfig, McpServerConfig, McpStdioServerConfig, McpWebSocketServerConfig,
};

/// What one edit did to the document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpEdit {
    /// The name was not in this document before.
    Added,
    /// The name was there and now holds a different definition.
    Replaced,
    /// The name was there and holds exactly what was asked for.
    Unchanged,
    /// The name was there and is gone.
    Removed,
    /// The name was not in this document at all.
    Absent,
}

/// Put `config` under `mcpServers.<name>` in the settings document at `path`,
/// leaving every other key alone.
///
/// # Errors
///
/// The document is not a JSON object, its `mcpServers` section is not one, the
/// edited document does not survive the loader's own parser, or the write
/// fails.
pub fn write_mcp_server(
    path: &Path,
    name: &str,
    config: &McpServerConfig,
) -> Result<McpEdit, ConfigError> {
    let mut root = read_document(path)?;
    let written = mcp_server_to_json(config);
    let servers = servers_section(&mut root, path)?;
    let edit = match servers.get(name) {
        Some(existing) if *existing == written => McpEdit::Unchanged,
        Some(_) => McpEdit::Replaced,
        None => McpEdit::Added,
    };
    if edit == McpEdit::Unchanged {
        return Ok(edit);
    }
    servers.insert(name.to_string(), written);
    commit(path, root, name, Some(config))?;
    Ok(edit)
}

/// Take `mcpServers.<name>` out of the settings document at `path`.
///
/// # Errors
///
/// As [`write_mcp_server`].
pub fn remove_mcp_server(path: &Path, name: &str) -> Result<McpEdit, ConfigError> {
    let mut root = read_document(path)?;
    let servers = servers_section(&mut root, path)?;
    if servers.remove(name).is_none() {
        return Ok(McpEdit::Absent);
    }
    // An empty section is noise in a document a person reads; the loader treats
    // a missing `mcpServers` and an empty one identically.
    if servers.is_empty() {
        root.remove(mcp_keys::SERVERS);
    }
    commit(path, root, name, None)?;
    Ok(McpEdit::Removed)
}

/// Record `name` in the trusted-MCP-servers file at `path`, keeping the names
/// already there in the order the person wrote them.
///
/// A project document lives in the repository, so writing a server into it is
/// not consent to RUN it: the server loads only once its name stands in this
/// record. Whether a record may be BELIEVED at all is
/// [`ConfigLoader`](super::ConfigLoader)'s gate — a git-tracked, symlinked or
/// nested-`.zo` file fails closed there — which is why recording a name says
/// nothing about the next load on its own.
///
/// # Errors
///
/// The record is not a JSON array of server names, it does not survive the
/// round-trip through the parser a future session uses, or the write fails.
pub fn trust_mcp_server(path: &Path, name: &str) -> Result<McpEdit, ConfigError> {
    let mut names = parse_trusted_mcp_server_names(path, &read_file_or_empty(path)?)?;
    if names.iter().any(|trusted| trusted == name) {
        return Ok(McpEdit::Unchanged);
    }
    names.push(name.to_string());
    let rendered =
        JsonValue::Array(names.iter().cloned().map(JsonValue::String).collect()).render_pretty();
    // The read-back `commit` does for a settings document: a record our
    // renderer mangles is caught before it becomes the file on disk.
    if parse_trusted_mcp_server_names(path, &rendered)? != names {
        return Err(ConfigError::Parse(format!(
            "{}: the trusted MCP servers did not survive the JSON round-trip",
            path.display()
        )));
    }
    create_parent_directory(path)?;
    // Owner-only like every document this module writes: a name another local
    // account could append is a server this one would run.
    write_private_file(path, rendered.as_bytes(), &ParentDirPolicy::LeaveParent)
        .map_err(ConfigError::Io)?;
    Ok(McpEdit::Added)
}

/// Read the document through the loader's own parser. A file that is not there
/// yet is an empty document — `zo mcp add` is often the first thing to write a
/// settings file at all.
fn read_document(path: &Path) -> Result<BTreeMap<String, JsonValue>, ConfigError> {
    parse_json_object_contents(path, &read_file_or_empty(path)?)
}

/// The file as it stands, or nothing at all — the same thing to a writer whose
/// job is to add one entry.
fn read_file_or_empty(path: &Path) -> Result<String, ConfigError> {
    match std::fs::read_to_string(path) {
        Ok(contents) => Ok(contents),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(ConfigError::Io(error)),
    }
}

/// Create the `.zo` (or config home) directory when it is missing, but never
/// re-permission one that already exists — it may be a shared, pre-existing
/// directory this process does not own.
fn create_parent_directory(path: &Path) -> Result<(), ConfigError> {
    let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) else {
        return Ok(());
    };
    std::fs::create_dir_all(parent).map_err(ConfigError::Io)
}

/// The document's `mcpServers` object, created when absent.
fn servers_section<'a>(
    root: &'a mut BTreeMap<String, JsonValue>,
    path: &Path,
) -> Result<&'a mut BTreeMap<String, JsonValue>, ConfigError> {
    let section = root
        .entry(mcp_keys::SERVERS.to_string())
        .or_insert_with(|| JsonValue::Object(BTreeMap::new()));
    match section {
        JsonValue::Object(servers) => Ok(servers),
        _ => Err(ConfigError::Parse(format!(
            "{}: {} must be a JSON object",
            path.display(),
            mcp_keys::SERVERS
        ))),
    }
}

/// Render the edited document, read it back through the parser a future
/// session uses, and only then write it.
///
/// `expected` is the definition the caller asked for, or `None` for a removal.
/// A document our renderer mangles, or a server that does not come back as it
/// went in, fails here — before it becomes the file on disk.
fn commit(
    path: &Path,
    root: BTreeMap<String, JsonValue>,
    name: &str,
    expected: Option<&McpServerConfig>,
) -> Result<(), ConfigError> {
    let rendered = JsonValue::Object(root).render_pretty();
    let roundtrip = parse_json_object_contents(path, &rendered)?;
    let read_back = roundtrip
        .get(mcp_keys::SERVERS)
        .and_then(JsonValue::as_object)
        .and_then(|servers| servers.get(name));
    match (expected, read_back) {
        (Some(expected), Some(value)) => {
            let parsed = parse_mcp_server_config(name, value, &mcp_server_context(path, name))?;
            if parsed != *expected {
                return Err(ConfigError::Parse(format!(
                    "{}: MCP server {name} did not survive the JSON round-trip",
                    path.display()
                )));
            }
        }
        (None, None) => {}
        (Some(_), None) | (None, Some(_)) => {
            return Err(ConfigError::Parse(format!(
                "{}: MCP server {name} did not survive the JSON round-trip",
                path.display()
            )));
        }
    }
    create_parent_directory(path)?;
    // The file is owner-only: an `env` block under `mcpServers` carries the
    // server's tokens.
    write_private_file(path, rendered.as_bytes(), &ParentDirPolicy::LeaveParent)
        .map_err(ConfigError::Io)
}

/// One server, as the settings document spells it.
///
/// The transport tag is written only when `infer_mcp_server_type` — the
/// parser's own inference — would not reach this variant on its own, so a
/// stdio server stays the two-key object people write by hand and an `sse`,
/// `ws` or `sdk` server carries the tag it needs.
fn mcp_server_to_json(config: &McpServerConfig) -> JsonValue {
    let (tag, mut object) = match config {
        McpServerConfig::Stdio(stdio) => (mcp_keys::STDIO, stdio_to_json(stdio)),
        McpServerConfig::Sse(remote) => (mcp_keys::SSE, remote_to_json(remote)),
        McpServerConfig::Http(remote) => (mcp_keys::HTTP, remote_to_json(remote)),
        McpServerConfig::Ws(ws) => (mcp_keys::WS, websocket_to_json(ws)),
        McpServerConfig::Sdk(sdk) => (mcp_keys::SDK, sdk_to_json(sdk)),
        McpServerConfig::ManagedProxy(proxy) => {
            (mcp_keys::MANAGED_PROXY, managed_proxy_to_json(proxy))
        }
    };
    if infer_mcp_server_type(&object) != tag {
        object.insert(
            mcp_keys::TYPE.to_string(),
            JsonValue::String(tag.to_string()),
        );
    }
    JsonValue::Object(object)
}

fn stdio_to_json(stdio: &McpStdioServerConfig) -> BTreeMap<String, JsonValue> {
    let mut object = BTreeMap::new();
    object.insert(
        mcp_keys::COMMAND.to_string(),
        JsonValue::String(stdio.command.clone()),
    );
    if !stdio.args.is_empty() {
        object.insert(mcp_keys::ARGS.to_string(), string_array(&stdio.args));
    }
    if !stdio.env.is_empty() {
        object.insert(mcp_keys::ENV.to_string(), string_map(&stdio.env));
    }
    if let Some(timeout) = stdio.tool_call_timeout_ms {
        object.insert(
            mcp_keys::TOOL_CALL_TIMEOUT_MS.to_string(),
            JsonValue::Number(i64::try_from(timeout).unwrap_or(i64::MAX)),
        );
    }
    object
}

fn remote_to_json(remote: &McpRemoteServerConfig) -> BTreeMap<String, JsonValue> {
    let mut object = BTreeMap::new();
    object.insert(
        mcp_keys::URL.to_string(),
        JsonValue::String(remote.url.clone()),
    );
    insert_header_keys(&mut object, &remote.headers, remote.headers_helper.as_ref());
    if let Some(oauth) = &remote.oauth {
        object.insert(mcp_keys::OAUTH.to_string(), oauth_to_json(oauth));
    }
    object
}

fn websocket_to_json(ws: &McpWebSocketServerConfig) -> BTreeMap<String, JsonValue> {
    let mut object = BTreeMap::new();
    object.insert(mcp_keys::URL.to_string(), JsonValue::String(ws.url.clone()));
    insert_header_keys(&mut object, &ws.headers, ws.headers_helper.as_ref());
    object
}

fn sdk_to_json(sdk: &McpSdkServerConfig) -> BTreeMap<String, JsonValue> {
    let mut object = BTreeMap::new();
    object.insert(
        mcp_keys::NAME.to_string(),
        JsonValue::String(sdk.name.clone()),
    );
    object
}

fn managed_proxy_to_json(proxy: &McpManagedProxyServerConfig) -> BTreeMap<String, JsonValue> {
    let mut object = BTreeMap::new();
    object.insert(
        mcp_keys::URL.to_string(),
        JsonValue::String(proxy.url.clone()),
    );
    object.insert(mcp_keys::ID.to_string(), JsonValue::String(proxy.id.clone()));
    object
}

fn oauth_to_json(oauth: &McpOAuthConfig) -> JsonValue {
    let mut object = BTreeMap::new();
    if let Some(client_id) = &oauth.client_id {
        object.insert(
            mcp_keys::CLIENT_ID.to_string(),
            JsonValue::String(client_id.clone()),
        );
    }
    if let Some(port) = oauth.callback_port {
        object.insert(
            mcp_keys::CALLBACK_PORT.to_string(),
            JsonValue::Number(i64::from(port)),
        );
    }
    if let Some(url) = &oauth.auth_server_metadata_url {
        object.insert(
            mcp_keys::AUTH_SERVER_METADATA_URL.to_string(),
            JsonValue::String(url.clone()),
        );
    }
    if let Some(xaa) = oauth.xaa {
        object.insert(mcp_keys::XAA.to_string(), JsonValue::Bool(xaa));
    }
    JsonValue::Object(object)
}

fn insert_header_keys(
    object: &mut BTreeMap<String, JsonValue>,
    headers: &BTreeMap<String, String>,
    headers_helper: Option<&String>,
) {
    if !headers.is_empty() {
        object.insert(mcp_keys::HEADERS.to_string(), string_map(headers));
    }
    if let Some(helper) = headers_helper {
        object.insert(
            mcp_keys::HEADERS_HELPER.to_string(),
            JsonValue::String(helper.clone()),
        );
    }
}

fn string_array(values: &[String]) -> JsonValue {
    JsonValue::Array(
        values
            .iter()
            .map(|value| JsonValue::String(value.clone()))
            .collect(),
    )
}

fn string_map(values: &BTreeMap<String, String>) -> JsonValue {
    JsonValue::Object(
        values
            .iter()
            .map(|(key, value)| (key.clone(), JsonValue::String(value.clone())))
            .collect(),
    )
}

#[cfg(test)]
mod tests;
