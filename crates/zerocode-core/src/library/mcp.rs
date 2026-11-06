//! One MCP server, the way every agent spells it.
//!
//! Each agent keeps its user list in its own file and its own spelling — and
//! the spellings below are what each CLI WROTE when asked to add a server in a
//! throwaway home on 2026-11-06 (claude 2.1.318, codex-cli 0.166.0,
//! gemini 0.58.0, qwen, zo 1.1.126, agy, grok, opencode), not what their
//! documents say: a remote address is `url` to Claude, Gemini 0.58, zo and
//! Codex, `httpUrl` to Qwen and `serverUrl` to Antigravity; OpenCode keeps a
//! local server's command and arguments as one array under `mcp.servers`.
//!
//! The canonical shape ([`McpDef`]) is what the store keeps and what two
//! agents are compared by. Fields only one agent understands (a working
//! folder, a timeout, a trust flag) are not part of it: the store carries what
//! every agent can be told, and a comparison that counted a timeout would call
//! the same server two servers.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// One server, as every agent can be told it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum McpDef {
    /// A process the agent starts and talks to over its standard streams.
    Stdio {
        command: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<String>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        env: BTreeMap<String, String>,
    },
    /// A remote server over streamable HTTP.
    Http {
        url: String,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        headers: BTreeMap<String, String>,
    },
    /// A remote server over server-sent events.
    Sse {
        url: String,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        headers: BTreeMap<String, String>,
    },
}

/// A server as one list holds it: what it is, and whether that list has it
/// switched off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpEntry {
    pub def: McpDef,
    pub disabled: bool,
}

/// The ten spellings, one per file the window reads or writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Format {
    /// The store, `~/.agents/mcp.json`: Claude's spelling plus `disabled`.
    Store,
    /// `.claude.json` (`mcpServers`).
    Claude,
    /// `~/.gemini/settings.json` (`mcpServers`).
    Gemini,
    /// `~/.qwen/settings.json` (`mcpServers`, `httpUrl`).
    Qwen,
    /// `~/.zo/settings.json` (`mcpServers`).
    Zo,
    /// `~/.gemini/config/mcp_config.json` (`mcpServers`, `serverUrl`).
    Antigravity,
    /// `~/.codex/config.toml` (`[mcp_servers.<name>]`).
    Codex,
    /// `~/.grok/config.toml` (`[mcp_servers.<name>]`).
    Grok,
    /// `~/.config/opencode/opencode.json` (`mcp.servers`).
    OpenCode,
    /// `~/.kiro/settings/mcp.json` (`mcpServers`).
    Kiro,
    /// `~/.cursor/mcp.json` (`mcpServers`).
    Cursor,
}

impl Format {
    /// Whether the file is TOML rather than JSON.
    #[must_use]
    pub fn is_toml(self) -> bool {
        matches!(self, Self::Codex | Self::Grok)
    }
}

impl McpDef {
    /// The same server always gives the same string, and two servers that
    /// differ in anything an agent is told give two. Computed over the
    /// canonical JSON: every map is ordered, so the bytes do not depend on
    /// how the file that held it was written.
    #[must_use]
    pub fn fingerprint(&self) -> String {
        use sha2::{Digest, Sha256};
        let canonical = serde_json::to_string(self).unwrap_or_default();
        Sha256::digest(canonical.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    /// Whether the server is reached over the network.
    #[must_use]
    pub fn is_remote(&self) -> bool {
        !matches!(self, Self::Stdio { .. })
    }

    /// The names of the values the server is handed — never the values.
    #[must_use]
    pub fn secret_names(&self) -> Vec<String> {
        match self {
            Self::Stdio { env, .. } => env.keys().cloned().collect(),
            Self::Http { headers, .. } | Self::Sse { headers, .. } => {
                headers.keys().cloned().collect()
            }
        }
    }

    /// One line a person can read, with nothing a credential could be in:
    /// the command and its arguments through the core's credential mask
    /// ([`crate::credential::mask_named_values`] — by name, so a package
    /// like `@upstash/context7-mcp` is not taken for a token by its shape), or
    /// the address without its query.
    #[must_use]
    pub fn summary(&self) -> String {
        match self {
            Self::Stdio { command, args, .. } => {
                let line = std::iter::once(command.as_str())
                    .chain(args.iter().map(String::as_str))
                    .collect::<Vec<_>>()
                    .join(" ");
                crate::credential::mask_named_values(&line)
            }
            Self::Http { url, .. } | Self::Sse { url, .. } => {
                let bare = url.split(['?', '#']).next().unwrap_or(url);
                crate::credential::mask_named_values(bare)
            }
        }
    }

    /// A server that only works for the agent that wrote it: a command named
    /// by a relative path (resolved against that agent's own folder) or one
    /// inside an application bundle. Codex keeps two such — its Computer Use
    /// client and the ChatGPT app's `node_repl` — and handing either to Gemini
    /// would hand it a command that does not run there.
    ///
    /// A whole path runs from any agent: `/…` and `~/…`, and on Windows a
    /// drive (`C:\…`, `C:/…`) or a share (`\\host\…`). The rule reads the
    /// text, not the machine it runs on — a list written on Windows is
    /// judged the same on a Mac (the review of v1.1.128, P2-2).
    #[must_use]
    pub fn bound_to_its_agent(&self) -> bool {
        let Self::Stdio { command, .. } = self else {
            return false;
        };
        let bytes = command.as_bytes();
        let drive = bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'\\' | b'/');
        let whole = command.starts_with('/')
            || command.starts_with('~')
            || command.starts_with(r"\\")
            || drive;
        let relative = !whole && (command.contains('/') || command.contains('\\'));
        relative || command.contains(".app/Contents/")
    }
}

fn string_map(value: Option<&Value>) -> BTreeMap<String, String> {
    value
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .filter_map(|(key, value)| match value {
                    Value::String(text) => Some((key.clone(), text.clone())),
                    Value::Number(number) => Some((key.clone(), number.to_string())),
                    Value::Bool(flag) => Some((key.clone(), flag.to_string())),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn string_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| match item {
                    Value::String(text) => Some(text.clone()),
                    Value::Number(number) => Some(number.to_string()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|found| !found.is_empty())
}

/// One server out of a JSON list, in `format`'s spelling.
fn entry_from_json(format: Format, value: &Value) -> Option<McpEntry> {
    if !value.is_object() {
        return None;
    }
    let disabled = value.get("disabled").and_then(Value::as_bool) == Some(true)
        || value.get("enabled").and_then(Value::as_bool) == Some(false);
    if format == Format::OpenCode {
        let kind = text(value, "type").unwrap_or("local");
        let def = if kind == "remote" {
            McpDef::Http {
                url: text(value, "url")?.to_string(),
                headers: string_map(value.get("headers")),
            }
        } else {
            let mut command = string_list(value.get("command"));
            if command.is_empty() {
                return None;
            }
            let program = command.remove(0);
            McpDef::Stdio {
                command: program,
                args: command,
                env: string_map(value.get("environment")),
            }
        };
        return Some(McpEntry { def, disabled });
    }
    let kind = text(value, "type")
        .or_else(|| text(value, "transport"))
        .map(str::to_ascii_lowercase);
    let address = text(value, "url")
        .or_else(|| text(value, "httpUrl"))
        .or_else(|| text(value, "serverUrl"));
    let def = match (address, text(value, "command")) {
        (Some(url), _) => {
            let headers = string_map(value.get("headers"));
            if kind.as_deref() == Some("sse") {
                McpDef::Sse {
                    url: url.to_string(),
                    headers,
                }
            } else {
                McpDef::Http {
                    url: url.to_string(),
                    headers,
                }
            }
        }
        (None, Some(command)) => McpDef::Stdio {
            command: command.to_string(),
            args: string_list(value.get("args")),
            env: string_map(value.get("env")),
        },
        (None, None) => return None,
    };
    Some(McpEntry { def, disabled })
}

/// The object a JSON document keeps its servers in, for `format`.
fn json_list(format: Format, doc: &Value) -> Option<&Map<String, Value>> {
    match format {
        Format::OpenCode => {
            let mcp = doc.get("mcp")?.as_object()?;
            // The current spelling nests the servers one level down; an older
            // file keeps them straight under `mcp`.
            match mcp.get("servers").and_then(Value::as_object) {
                Some(servers) => Some(servers),
                None => Some(mcp),
            }
        }
        _ => doc.get("mcpServers")?.as_object(),
    }
}

/// Every server a JSON document lists, by name.
#[must_use]
pub fn read_json(format: Format, doc: &Value) -> BTreeMap<String, McpEntry> {
    let Some(list) = json_list(format, doc) else {
        return BTreeMap::new();
    };
    list.iter()
        .filter(|(name, _)| format != Format::OpenCode || name.as_str() != "servers")
        .filter_map(|(name, value)| Some((name.clone(), entry_from_json(format, value)?)))
        .collect()
}

fn toml_to_json(value: &toml::Value) -> Value {
    match value {
        toml::Value::String(text) => Value::String(text.clone()),
        toml::Value::Integer(number) => Value::from(*number),
        toml::Value::Float(number) => serde_json::Number::from_f64(*number)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        toml::Value::Boolean(flag) => Value::Bool(*flag),
        toml::Value::Datetime(when) => Value::String(when.to_string()),
        toml::Value::Array(items) => Value::Array(items.iter().map(toml_to_json).collect()),
        toml::Value::Table(table) => Value::Object(
            table
                .iter()
                .map(|(key, value)| (key.clone(), toml_to_json(value)))
                .collect(),
        ),
    }
}

/// Every server a TOML document lists (`[mcp_servers.<name>]`), by name.
#[must_use]
pub fn read_toml(doc: &toml::Table) -> BTreeMap<String, McpEntry> {
    let Some(list) = doc.get("mcp_servers").and_then(toml::Value::as_table) else {
        return BTreeMap::new();
    };
    list.iter()
        .filter_map(|(name, value)| {
            let mut json = toml_to_json(value);
            // Codex spells a remote server's headers `http_headers`.
            if let Some(object) = json.as_object_mut()
                && !object.contains_key("headers")
                && let Some(headers) = object.remove("http_headers")
            {
                object.insert("headers".to_string(), headers);
            }
            Some((name.clone(), entry_from_json(Format::Codex, &json)?))
        })
        .collect()
}

/// Every server a file's text lists, or `None` when the text is not a
/// document of that kind — a file that cannot be read is never taken for an
/// empty list, or the store would read every server as gone from it.
#[must_use]
pub fn read_text(format: Format, text: &str) -> Option<BTreeMap<String, McpEntry>> {
    if text.trim().is_empty() {
        return Some(BTreeMap::new());
    }
    if format.is_toml() {
        let doc: toml::Table = toml::from_str(text).ok()?;
        return Some(read_toml(&doc));
    }
    let doc: Value = serde_json::from_str(text).ok()?;
    doc.is_object().then(|| read_json(format, &doc))
}

/// The server as `format` writes it into a JSON file.
#[must_use]
pub fn to_json(format: Format, entry: &McpEntry) -> Value {
    let mut out = Map::new();
    match (&entry.def, format) {
        (McpDef::Stdio { command, args, env }, Format::OpenCode) => {
            out.insert("type".into(), "local".into());
            let mut line = vec![Value::String(command.clone())];
            line.extend(args.iter().cloned().map(Value::String));
            out.insert("command".into(), Value::Array(line));
            if !env.is_empty() {
                out.insert("environment".into(), json_map(env));
            }
        }
        (McpDef::Http { url, headers } | McpDef::Sse { url, headers }, Format::OpenCode) => {
            out.insert("type".into(), "remote".into());
            out.insert("url".into(), url.clone().into());
            if !headers.is_empty() {
                out.insert("headers".into(), json_map(headers));
            }
        }
        (McpDef::Stdio { command, args, env }, _) => {
            if matches!(format, Format::Store | Format::Claude) {
                out.insert("type".into(), "stdio".into());
            }
            out.insert("command".into(), command.clone().into());
            if !args.is_empty() {
                out.insert(
                    "args".into(),
                    Value::Array(args.iter().cloned().map(Value::String).collect()),
                );
            }
            if !env.is_empty() {
                out.insert("env".into(), json_map(env));
            }
        }
        (McpDef::Http { url, headers } | McpDef::Sse { url, headers }, _) => {
            let kind = if matches!(entry.def, McpDef::Sse { .. }) {
                "sse"
            } else {
                "http"
            };
            if matches!(format, Format::Store | Format::Claude | Format::Gemini) {
                out.insert("type".into(), kind.into());
            }
            let key = match format {
                Format::Qwen => "httpUrl",
                Format::Antigravity => "serverUrl",
                _ => "url",
            };
            out.insert(key.into(), url.clone().into());
            if !headers.is_empty() {
                out.insert("headers".into(), json_map(headers));
            }
        }
    }
    match format {
        Format::Store | Format::Kiro | Format::Antigravity if entry.disabled => {
            out.insert("disabled".into(), true.into());
        }
        Format::Kiro | Format::Antigravity => {
            out.insert("disabled".into(), false.into());
        }
        Format::OpenCode if entry.disabled => {
            out.insert("enabled".into(), false.into());
        }
        _ => {}
    }
    Value::Object(out)
}

fn json_map(map: &BTreeMap<String, String>) -> Value {
    Value::Object(
        map.iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect(),
    )
}

/// Put one server into a JSON document, or take it out (`None`), leaving
/// every other key as it was. Answers whether the document changed.
pub fn set_json(doc: &mut Value, format: Format, name: &str, entry: Option<&McpEntry>) -> bool {
    if !doc.is_object() {
        *doc = Value::Object(Map::new());
    }
    let Some(root) = doc.as_object_mut() else {
        return false;
    };
    let list = match format {
        Format::OpenCode => {
            let mcp = root
                .entry("mcp")
                .or_insert_with(|| Value::Object(Map::new()));
            if !mcp.is_object() {
                *mcp = Value::Object(Map::new());
            }
            let Some(mcp) = mcp.as_object_mut() else {
                return false;
            };
            // A server a former spelling kept straight under `mcp` goes too.
            let mut changed = false;
            if entry.is_none() && mcp.get(name).is_some_and(Value::is_object) && name != "servers" {
                mcp.remove(name);
                changed = true;
            }
            let servers = mcp
                .entry("servers")
                .or_insert_with(|| Value::Object(Map::new()));
            if !servers.is_object() {
                *servers = Value::Object(Map::new());
            }
            let Some(servers) = servers.as_object_mut() else {
                return changed;
            };
            return set_in(servers, format, name, entry) || changed;
        }
        _ => root
            .entry("mcpServers")
            .or_insert_with(|| Value::Object(Map::new())),
    };
    if !list.is_object() {
        *list = Value::Object(Map::new());
    }
    let Some(list) = list.as_object_mut() else {
        return false;
    };
    set_in(list, format, name, entry)
}

fn set_in(
    list: &mut Map<String, Value>,
    format: Format,
    name: &str,
    entry: Option<&McpEntry>,
) -> bool {
    match entry {
        Some(entry) => {
            let next = to_json(format, entry);
            if list.get(name) == Some(&next) {
                return false;
            }
            list.insert(name.to_string(), next);
            true
        }
        None => list.remove(name).is_some(),
    }
}

/// How the window writes one agent's list: through the agent's own command,
/// or straight into the file where the agent has no command that can.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Write {
    /// Run the agent's command with these arguments.
    Command(Vec<String>),
    /// Edit the file (JSON only).
    File,
    /// Not something this agent can hold — a transport it does not speak.
    Unsupported(&'static str),
}

/// The arguments that add `name` with `def` through the agent's own command.
///
/// Each road was run against a throwaway home and its file read back
/// (2026-11-06); the comments say where the order is the command's rule
/// rather than a choice.
#[must_use]
pub fn add_command(format: Format, name: &str, def: &McpDef) -> Write {
    let mut argv: Vec<String> = vec!["mcp".into()];
    let push_all = |argv: &mut Vec<String>, flag: &str, pairs: Vec<String>| {
        for pair in pairs {
            argv.push(flag.to_string());
            argv.push(pair);
        }
    };
    let env_pairs = |env: &BTreeMap<String, String>| -> Vec<String> {
        env.iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect()
    };
    let colon_headers = |headers: &BTreeMap<String, String>| -> Vec<String> {
        headers
            .iter()
            .map(|(key, value)| format!("{key}: {value}"))
            .collect()
    };
    let equals_headers = |headers: &BTreeMap<String, String>| -> Vec<String> {
        headers
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect()
    };
    match format {
        Format::Claude => {
            argv.extend(["add-json".into(), "-s".into(), "user".into(), name.into()]);
            argv.push(
                to_json(
                    Format::Claude,
                    &McpEntry {
                        def: def.clone(),
                        disabled: false,
                    },
                )
                .to_string(),
            );
        }
        Format::Codex => match def {
            McpDef::Stdio { command, args, env } => {
                argv.extend(["add".into(), name.into()]);
                push_all(&mut argv, "--env", env_pairs(env));
                argv.push("--".into());
                argv.push(command.clone());
                argv.extend(args.iter().cloned());
            }
            // Codex speaks streamable HTTP only; its headers are not a flag of
            // `codex mcp add` and are written beside it ([`codex_headers`]).
            McpDef::Http { url, .. } => {
                argv.extend(["add".into(), name.into(), "--url".into(), url.clone()]);
            }
            McpDef::Sse { .. } => return Write::Unsupported("sse"),
        },
        Format::Zo => match def {
            McpDef::Stdio { command, args, env } => {
                argv.extend(["add".into(), name.into()]);
                push_all(&mut argv, "--env", env_pairs(env));
                argv.push("--".into());
                argv.push(command.clone());
                argv.extend(args.iter().cloned());
            }
            McpDef::Http { url, headers } | McpDef::Sse { url, headers } => {
                argv.extend(["add".into(), name.into(), "--url".into(), url.clone()]);
                push_all(&mut argv, "--header", equals_headers(headers));
                if matches!(def, McpDef::Sse { .. }) {
                    argv.extend(["--transport".into(), "sse".into()]);
                }
            }
        },
        // yargs: the scope and the array flags go before the two positionals,
        // and a `-y` meant for the server goes after `--`.
        Format::Gemini | Format::Qwen | Format::Grok => {
            argv.extend(["add".into(), "-s".into(), "user".into()]);
            match def {
                McpDef::Stdio { command, args, env } => {
                    push_all(&mut argv, "-e", env_pairs(env));
                    argv.push(name.into());
                    argv.push(command.clone());
                    if !args.is_empty() {
                        argv.push("--".into());
                        argv.extend(args.iter().cloned());
                    }
                }
                McpDef::Http { url, headers } | McpDef::Sse { url, headers } => {
                    let kind = if matches!(def, McpDef::Sse { .. }) {
                        "sse"
                    } else {
                        "http"
                    };
                    argv.extend(["-t".into(), kind.into()]);
                    push_all(&mut argv, "-H", colon_headers(headers));
                    argv.push(name.into());
                    argv.push(url.clone());
                }
            }
        }
        // `agy`: every flag before the name, or it is refused.
        Format::Antigravity => {
            argv.push("add".into());
            match def {
                McpDef::Stdio { command, args, env } => {
                    push_all(&mut argv, "--env", env_pairs(env));
                    argv.push(name.into());
                    argv.push("--".into());
                    argv.push(command.clone());
                    argv.extend(args.iter().cloned());
                }
                McpDef::Http { url, headers } => {
                    push_all(&mut argv, "--header", colon_headers(headers));
                    argv.extend(["--type".into(), "http".into(), name.into(), url.clone()]);
                }
                McpDef::Sse { .. } => return Write::Unsupported("sse"),
            }
        }
        Format::OpenCode => {
            argv.extend(["add".into(), "--global".into()]);
            match def {
                McpDef::Stdio { command, args, env } => {
                    push_all(&mut argv, "--env", env_pairs(env));
                    argv.push(name.into());
                    argv.push("--".into());
                    argv.push(command.clone());
                    argv.extend(args.iter().cloned());
                }
                McpDef::Http { url, headers } | McpDef::Sse { url, headers } => {
                    argv.extend(["--url".into(), url.clone()]);
                    push_all(&mut argv, "--header", equals_headers(headers));
                    argv.push(name.into());
                }
            }
        }
        Format::Kiro | Format::Cursor | Format::Store => return Write::File,
    }
    Write::Command(argv)
}

/// The arguments that remove `name` through the agent's own command.
#[must_use]
pub fn remove_command(format: Format, name: &str) -> Write {
    let argv: Vec<String> = match format {
        Format::Claude | Format::Gemini | Format::Qwen | Format::Grok => {
            vec![
                "mcp".into(),
                "remove".into(),
                "-s".into(),
                "user".into(),
                name.into(),
            ]
        }
        Format::Codex | Format::Zo | Format::Antigravity => {
            vec!["mcp".into(), "remove".into(), name.into()]
        }
        // OpenCode has no `mcp remove`; Kiro's asks for a login first.
        Format::OpenCode | Format::Kiro | Format::Cursor | Format::Store => return Write::File,
    };
    Write::Command(argv)
}

/// The table Codex reads a remote server's headers from, as TOML text to put
/// after the table `codex mcp add` wrote — the command has no flag for them.
#[must_use]
pub fn codex_headers(name: &str, def: &McpDef) -> Option<String> {
    let McpDef::Http { headers, .. } = def else {
        return None;
    };
    if headers.is_empty() {
        return None;
    }
    let mut out = format!("\n[mcp_servers.{}.http_headers]\n", toml_key(name));
    for (key, value) in headers {
        out.push_str(&format!("{} = {}\n", toml_key(key), toml_string(value)));
    }
    Some(out)
}

/// A TOML key: bare when it can be, quoted otherwise.
fn toml_key(key: &str) -> String {
    if !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        key.to_string()
    } else {
        toml_string(key)
    }
}

/// A TOML basic string.
fn toml_string(value: &str) -> String {
    let mut out = String::from("\"");
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\u{:04X}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The servers of a pasted document — the `{"mcpServers": {…}}` block an MCP
/// server's README hands out, or a bare map of names. Nothing else is read.
#[must_use]
pub fn read_pasted(text: &str) -> Option<BTreeMap<String, McpEntry>> {
    let doc: Value = serde_json::from_str(text.trim()).ok()?;
    let object = doc.as_object()?;
    for key in ["mcpServers", "servers"] {
        if let Some(list) = object.get(key).and_then(Value::as_object) {
            let wrapped = serde_json::json!({ "mcpServers": list });
            return Some(read_json(Format::Claude, &wrapped));
        }
    }
    if let Some(mcp) = object.get("mcp").and_then(Value::as_object) {
        let wrapped = serde_json::json!({ "mcp": mcp });
        return Some(read_json(Format::OpenCode, &wrapped));
    }
    let wrapped = serde_json::json!({ "mcpServers": object });
    let found = read_json(Format::Claude, &wrapped);
    (!found.is_empty()).then_some(found)
}

/// Whether `name` is one a server may carry in every agent's list: the names
/// the agents' own commands take (letters, digits, `-`, `_`, `.`, `@`, `/`).
#[must_use]
pub fn is_server_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@' | '/'))
        && !name.contains("..")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stdio(command: &str, args: &[&str], env: &[(&str, &str)]) -> McpDef {
        McpDef::Stdio {
            command: command.into(),
            args: args.iter().map(|a| (*a).to_string()).collect(),
            env: env
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
        }
    }

    fn http(url: &str, headers: &[(&str, &str)]) -> McpDef {
        McpDef::Http {
            url: url.into(),
            headers: headers
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
        }
    }

    /// The exact documents each CLI wrote into a throwaway home (2026-11-06),
    /// read back to the same server.
    #[test]
    fn every_agents_own_spelling_reads_back_to_one_server() {
        let local = stdio("echo", &["-y", "hi"], &[("ZZ_KEY", "v1")]);
        let remote = http("https://example.com/mcp", &[("X-Key", "h1")]);
        let cases: Vec<(Format, &str, &McpDef)> = vec![
            (
                Format::Claude,
                r#"{"mcpServers":{"s":{"type":"stdio","command":"echo","args":["-y","hi"],"env":{"ZZ_KEY":"v1"}}}}"#,
                &local,
            ),
            (
                Format::Gemini,
                r#"{"mcpServers":{"s":{"command":"echo","args":["-y","hi"],"env":{"ZZ_KEY":"v1"}}}}"#,
                &local,
            ),
            (
                Format::Zo,
                r#"{"mcpServers":{"s":{"args":["-y","hi"],"command":"echo","env":{"ZZ_KEY":"v1"}}}}"#,
                &local,
            ),
            (
                Format::Antigravity,
                r#"{"mcpServers":{"s":{"args":["-y","hi"],"command":"echo","disabled":false,"env":{"ZZ_KEY":"v1"}}}}"#,
                &local,
            ),
            (
                Format::OpenCode,
                r#"{"mcp":{"servers":{"s":{"type":"local","command":["echo","-y","hi"],"environment":{"ZZ_KEY":"v1"}}}}}"#,
                &local,
            ),
            (
                Format::Claude,
                r#"{"mcpServers":{"s":{"type":"http","url":"https://example.com/mcp","headers":{"X-Key":"h1"}}}}"#,
                &remote,
            ),
            (
                Format::Gemini,
                r#"{"mcpServers":{"s":{"url":"https://example.com/mcp","type":"http","headers":{"X-Key":"h1"}}}}"#,
                &remote,
            ),
            (
                Format::Qwen,
                r#"{"mcpServers":{"s":{"httpUrl":"https://example.com/mcp","headers":{"X-Key":"h1"}}}}"#,
                &remote,
            ),
            (
                Format::Zo,
                r#"{"mcpServers":{"s":{"headers":{"X-Key":"h1"},"url":"https://example.com/mcp"}}}"#,
                &remote,
            ),
            (
                Format::Antigravity,
                r#"{"mcpServers":{"s":{"disabled":false,"headers":{"X-Key":"h1"},"serverUrl":"https://example.com/mcp"}}}"#,
                &remote,
            ),
            (
                Format::OpenCode,
                r#"{"mcp":{"servers":{"s":{"type":"remote","url":"https://example.com/mcp","headers":{"X-Key":"h1"}}}}}"#,
                &remote,
            ),
        ];
        for (format, text, want) in cases {
            let read = read_text(format, text).expect("a document");
            assert_eq!(
                read.get("s").map(|e| &e.def),
                Some(want),
                "{format:?}: {text}"
            );
        }
        let codex = "[mcp_servers.s]\ncommand = \"echo\"\nargs = [\"-y\", \"hi\"]\n\n[mcp_servers.s.env]\nZZ_KEY = \"v1\"\n";
        assert_eq!(read_text(Format::Codex, codex).unwrap()["s"].def, local);
        let grok = "[mcp_servers.s]\ncommand = \"echo\"\nargs = [\n    \"-y\",\n    \"hi\",\n]\nenabled = true\n\n[mcp_servers.s.env]\nZZ_KEY = \"v1\"\n";
        assert_eq!(read_text(Format::Grok, grok).unwrap()["s"].def, local);
        let codex_remote = "[mcp_servers.s]\nurl = \"https://example.com/mcp\"\n\n[mcp_servers.s.http_headers]\nX-Key = \"h1\"\n";
        assert_eq!(
            read_text(Format::Codex, codex_remote).unwrap()["s"].def,
            remote
        );
    }

    #[test]
    fn an_unreadable_file_is_not_an_empty_list() {
        assert_eq!(read_text(Format::Claude, "{ not json"), None);
        assert_eq!(read_text(Format::Codex, "[[[ nope"), None);
        assert_eq!(read_text(Format::Claude, "  "), Some(BTreeMap::new()));
        assert_eq!(read_text(Format::Claude, "{}"), Some(BTreeMap::new()));
    }

    #[test]
    fn a_switched_off_server_says_so_in_every_spelling() {
        let off = read_text(
            Format::Kiro,
            r#"{"mcpServers":{"s":{"command":"x","disabled":true}}}"#,
        )
        .unwrap();
        assert!(off["s"].disabled);
        let off = read_text(
            Format::Grok,
            "[mcp_servers.s]\ncommand = \"x\"\nenabled = false\n",
        )
        .unwrap();
        assert!(off["s"].disabled);
        let on = read_text(
            Format::Antigravity,
            r#"{"mcpServers":{"s":{"command":"x","disabled":false}}}"#,
        )
        .unwrap();
        assert!(!on["s"].disabled);
    }

    #[test]
    fn the_fingerprint_is_the_server_not_the_file() {
        let a = read_text(
            Format::Gemini,
            r#"{"mcpServers":{"s":{"env":{"B":"2","A":"1"},"command":"x","args":["1"]}}}"#,
        )
        .unwrap();
        let b = read_text(
            Format::Zo,
            r#"{"mcpServers":{"s":{"command":"x","args":["1"],"env":{"A":"1","B":"2"}}}}"#,
        )
        .unwrap();
        assert_eq!(a["s"].def.fingerprint(), b["s"].def.fingerprint());
        let c = stdio("x", &["2"], &[("A", "1"), ("B", "2")]);
        assert_ne!(a["s"].def.fingerprint(), c.fingerprint());
        assert_ne!(
            http("https://a.test/mcp", &[]).fingerprint(),
            McpDef::Sse {
                url: "https://a.test/mcp".into(),
                headers: BTreeMap::new()
            }
            .fingerprint()
        );
    }

    #[test]
    fn the_summary_never_carries_a_value() {
        let local = stdio(
            "npx",
            &["-y", "@acme/mcp", "--token=sk-live-abcdefghijklmnop"],
            &[("API_KEY", "s3cret")],
        );
        let line = local.summary();
        assert!(line.starts_with("npx -y @acme/mcp"), "{line}");
        assert!(
            !line.contains("s3cret") && !line.contains("sk-live-abcdefghijklmnop"),
            "{line}"
        );
        assert_eq!(local.secret_names(), vec!["API_KEY".to_string()]);
        let remote = http(
            "https://mcp.example.com/mcp?key=abc#x",
            &[("Authorization", "Bearer x")],
        );
        assert_eq!(remote.summary(), "https://mcp.example.com/mcp");
        assert_eq!(remote.secret_names(), vec!["Authorization".to_string()]);
    }

    #[test]
    fn a_helper_inside_an_agents_app_is_bound_to_that_agent() {
        assert!(
            stdio(
                "./Codex Computer Use.app/Contents/SharedSupport/X.app/Contents/MacOS/X",
                &["mcp"],
                &[]
            )
            .bound_to_its_agent()
        );
        assert!(
            stdio(
                "/Applications/ChatGPT.app/Contents/Resources/cua_node/bin/node_repl",
                &[],
                &[]
            )
            .bound_to_its_agent()
        );
        assert!(stdio("bin/run", &[], &[]).bound_to_its_agent());
        assert!(!stdio("npx", &["-y", "@acme/mcp"], &[]).bound_to_its_agent());
        assert!(!stdio("/opt/homebrew/bin/rea", &["mcp"], &[]).bound_to_its_agent());
        assert!(!stdio("~/bin/rea", &[], &[]).bound_to_its_agent());
        assert!(!http("https://a.test", &[]).bound_to_its_agent());
    }

    /// The review of v1.1.128 (P2-2): a Windows command named by its whole
    /// path — a drive letter or a share — runs from any agent, so it is
    /// shared like any other; only a path relative to the agent's own folder
    /// is bound to it.
    #[test]
    fn a_windows_absolute_command_is_shared_like_any_other() {
        for absolute in [
            r"C:\Program Files\nodejs\npx.cmd",
            "D:/tools/rea.exe",
            r"\\fileserver\tools\rea.exe",
        ] {
            assert!(
                !stdio(absolute, &[], &[]).bound_to_its_agent(),
                "{absolute} was kept with its agent"
            );
        }
        for relative in [r"bin\run.exe", r".\tools\rea.exe"] {
            assert!(
                stdio(relative, &[], &[]).bound_to_its_agent(),
                "{relative} was shared"
            );
        }
    }

    /// The argument lists below are the ones run against the real CLIs in a
    /// throwaway home; each wrote the server above back out.
    #[test]
    fn each_agents_own_command_is_asked_in_its_own_grammar() {
        let local = stdio("echo", &["-y", "hi"], &[("ZZ_KEY", "v1")]);
        let remote = http("https://example.com/mcp", &[("X-Key", "h1")]);
        let argv = |w: Write| match w {
            Write::Command(argv) => argv.join(" "),
            other => format!("{other:?}"),
        };
        assert_eq!(
            argv(add_command(Format::Codex, "s", &local)),
            "mcp add s --env ZZ_KEY=v1 -- echo -y hi"
        );
        assert_eq!(
            argv(add_command(Format::Zo, "s", &local)),
            "mcp add s --env ZZ_KEY=v1 -- echo -y hi"
        );
        assert_eq!(
            argv(add_command(Format::Gemini, "s", &local)),
            "mcp add -s user -e ZZ_KEY=v1 s echo -- -y hi"
        );
        assert_eq!(
            argv(add_command(Format::Qwen, "s", &local)),
            "mcp add -s user -e ZZ_KEY=v1 s echo -- -y hi"
        );
        assert_eq!(
            argv(add_command(Format::Grok, "s", &local)),
            "mcp add -s user -e ZZ_KEY=v1 s echo -- -y hi"
        );
        assert_eq!(
            argv(add_command(Format::Antigravity, "s", &local)),
            "mcp add --env ZZ_KEY=v1 s -- echo -y hi"
        );
        assert_eq!(
            argv(add_command(Format::OpenCode, "s", &local)),
            "mcp add --global --env ZZ_KEY=v1 s -- echo -y hi"
        );
        // Claude's road carries the server as one JSON argument; its key
        // order is the JSON library's, so it is compared as JSON.
        let Write::Command(claude) = add_command(Format::Claude, "s", &local) else {
            panic!("Claude is written through its command");
        };
        assert_eq!(claude[..5].join(" "), "mcp add-json -s user s");
        let sent: Value = serde_json::from_str(&claude[5]).unwrap();
        assert_eq!(
            sent,
            serde_json::json!({"type":"stdio","command":"echo","args":["-y","hi"],"env":{"ZZ_KEY":"v1"}})
        );
        assert_eq!(
            argv(add_command(Format::Codex, "s", &remote)),
            "mcp add s --url https://example.com/mcp"
        );
        assert_eq!(
            argv(add_command(Format::Zo, "s", &remote)),
            "mcp add s --url https://example.com/mcp --header X-Key=h1"
        );
        assert_eq!(
            argv(add_command(Format::Gemini, "s", &remote)),
            "mcp add -s user -t http -H X-Key: h1 s https://example.com/mcp"
        );
        assert_eq!(
            argv(add_command(Format::Antigravity, "s", &remote)),
            "mcp add --header X-Key: h1 --type http s https://example.com/mcp"
        );
        assert_eq!(
            argv(add_command(Format::OpenCode, "s", &remote)),
            "mcp add --global --url https://example.com/mcp --header X-Key=h1 s"
        );
        assert_eq!(add_command(Format::Kiro, "s", &local), Write::File);
        assert_eq!(add_command(Format::Cursor, "s", &local), Write::File);
        let sse = McpDef::Sse {
            url: "https://a.test/sse".into(),
            headers: BTreeMap::new(),
        };
        assert_eq!(
            add_command(Format::Codex, "s", &sse),
            Write::Unsupported("sse")
        );
        assert_eq!(
            add_command(Format::Antigravity, "s", &sse),
            Write::Unsupported("sse")
        );
        assert_eq!(
            argv(add_command(Format::Zo, "s", &sse)),
            "mcp add s --url https://a.test/sse --transport sse"
        );
        // No args: no `--` for a yargs command, whose positional would be empty.
        let bare = stdio("rea", &[], &[]);
        assert_eq!(
            argv(add_command(Format::Gemini, "s", &bare)),
            "mcp add -s user s rea"
        );
    }

    #[test]
    fn each_agents_own_command_removes_in_its_own_grammar() {
        let argv = |w: Write| match w {
            Write::Command(argv) => argv.join(" "),
            other => format!("{other:?}"),
        };
        assert_eq!(
            argv(remove_command(Format::Claude, "s")),
            "mcp remove -s user s"
        );
        assert_eq!(
            argv(remove_command(Format::Gemini, "s")),
            "mcp remove -s user s"
        );
        assert_eq!(
            argv(remove_command(Format::Grok, "s")),
            "mcp remove -s user s"
        );
        assert_eq!(argv(remove_command(Format::Codex, "s")), "mcp remove s");
        assert_eq!(argv(remove_command(Format::Zo, "s")), "mcp remove s");
        assert_eq!(
            argv(remove_command(Format::Antigravity, "s")),
            "mcp remove s"
        );
        assert_eq!(remove_command(Format::OpenCode, "s"), Write::File);
        assert_eq!(remove_command(Format::Kiro, "s"), Write::File);
    }

    #[test]
    fn a_file_edit_keeps_every_other_key() {
        let mut doc: Value =
            serde_json::from_str(r#"{"theme":"dark","mcpServers":{"keep":{"command":"k"}}}"#)
                .unwrap();
        let entry = McpEntry {
            def: stdio("x", &["1"], &[]),
            disabled: false,
        };
        assert!(set_json(&mut doc, Format::Kiro, "s", Some(&entry)));
        assert!(
            !set_json(&mut doc, Format::Kiro, "s", Some(&entry)),
            "the same write twice changed it"
        );
        assert_eq!(doc["theme"], "dark");
        assert_eq!(doc["mcpServers"]["keep"]["command"], "k");
        assert_eq!(doc["mcpServers"]["s"]["disabled"], false);
        assert_eq!(read_json(Format::Kiro, &doc)["s"].def, entry.def);
        assert!(set_json(&mut doc, Format::Kiro, "s", None));
        assert!(doc["mcpServers"].get("s").is_none());
        assert!(!set_json(&mut doc, Format::Kiro, "s", None));

        let mut open: Value = serde_json::from_str(
            r#"{"$schema":"x","mcp":{"old":{"type":"local","command":["o"]}}}"#,
        )
        .unwrap();
        assert!(set_json(&mut open, Format::OpenCode, "s", Some(&entry)));
        assert_eq!(
            open["mcp"]["servers"]["s"]["command"],
            serde_json::json!(["x", "1"])
        );
        assert!(
            set_json(&mut open, Format::OpenCode, "old", None),
            "the former spelling's server stayed"
        );
        assert!(open["mcp"].get("old").is_none());
        assert_eq!(open["$schema"], "x");
    }

    #[test]
    fn the_store_keeps_a_switched_off_server_with_its_flag() {
        let entry = McpEntry {
            def: http("https://a.test/mcp", &[]),
            disabled: true,
        };
        let mut doc = Value::Null;
        set_json(&mut doc, Format::Store, "s", Some(&entry));
        assert_eq!(doc["mcpServers"]["s"]["type"], "http");
        assert_eq!(doc["mcpServers"]["s"]["disabled"], true);
        let back = read_json(Format::Store, &doc);
        assert_eq!(back["s"], entry);
    }

    #[test]
    fn codexs_headers_are_a_table_beside_its_server() {
        let remote = http(
            "https://example.com/mcp",
            &[("X-Key", "h\"1"), ("Authorization", "Bearer a")],
        );
        let table = codex_headers("my server", &remote).unwrap();
        let doc =
            format!("[mcp_servers.\"my server\"]\nurl = \"https://example.com/mcp\"\n{table}");
        let read = read_text(Format::Codex, &doc).unwrap();
        assert_eq!(read["my server"].def, remote);
        assert_eq!(codex_headers("s", &http("https://a.test", &[])), None);
    }

    #[test]
    fn a_pasted_readme_block_reads_as_its_servers() {
        let readme = r#"{ "mcpServers": { "github": { "command": "npx", "args": ["-y", "@modelcontextprotocol/server-github"], "env": { "GITHUB_TOKEN": "x" } }, "sentry": { "url": "https://mcp.sentry.dev/mcp" } } }"#;
        let read = read_pasted(readme).unwrap();
        assert_eq!(read.len(), 2);
        assert!(matches!(read["sentry"].def, McpDef::Http { .. }));
        let bare = read_pasted(r#"{"fs": {"command": "npx", "args": ["fs"]}}"#).unwrap();
        assert!(bare.contains_key("fs"));
        assert_eq!(read_pasted("not json"), None);
        assert_eq!(read_pasted("{}"), None);
    }

    #[test]
    fn a_server_name_is_one_every_command_takes() {
        for good in [
            "rea",
            "context7",
            "codebase-memory-mcp",
            "@hypothesi/tauri-mcp-server",
            "node_repl",
            "a.b",
        ] {
            assert!(is_server_name(good), "{good}");
        }
        for bad in ["", "-x", "a b", "a;b", "../x", "x\n", &"a".repeat(129)] {
            assert!(!is_server_name(bad), "{bad}");
        }
    }
}
