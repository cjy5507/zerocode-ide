//! `zo mcp …` — the configured MCP servers, from outside a session.
//!
//! Until this door existed the only way to add a server was to hand-edit
//! `~/.zo/settings.json` or `.zo/settings.json` and hope the next session
//! agreed with what you typed; the only way to see one was to count them in
//! `--doctor`. Codex answers both with verbs (`codex mcp list|get|add|remove|
//! login|logout`), and this is that surface, with Codex's own grammar
//! (docs/design/zo-vs-codex-gaps-20260921.md §18).
//!
//! Reading is [`McpConfigCollection`](runtime::McpConfigCollection) rendered —
//! including the project-scoped servers the supply-chain trust gate skipped, so
//! a deliberately gated server is a row that says why rather than a server that
//! silently is not there. Writing is [`runtime::write_mcp_server`], the inverse
//! of the loader's own parser. No session, no credentials, no workspace trust:
//! the same principle as `--doctor`.
//!
//! **A value is never printed.** An `env` block under `mcpServers` carries the
//! server's tokens and a `headers` block carries its bearer, so both are
//! rendered as their KEYS. The one place this rule could be broken silently is
//! the `--json` shape, so a test holds it.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use core_types::paths::ZO_DIR_NAME;
use runtime::mcp_oauth::{BrowserOpener, McpAuthResult};
use runtime::{
    ConfigLoader, ConfigSource, McpEdit, McpRemoteServerConfig, McpServerConfig,
    McpStdioServerConfig, McpTransport, McpWebSocketServerConfig, RuntimeConfig,
};
use serde_json::{json, Value};

pub const USAGE: &str = "\
zo mcp list [--json] [--cwd <dir>]
zo mcp get <name> [--json] [--cwd <dir>]
zo mcp add <name> (--url <url> | -- <command> [args…]) [--env K=V]… [--header K=V]…
       [--transport stdio|http|sse|ws] [--project [--trust]] [--cwd <dir>]
zo mcp remove <name> [--project] [--cwd <dir>]
zo mcp login <name> [--scopes <a,b>] [--cwd <dir>]
zo mcp logout <name> [--cwd <dir>]

  The MCP servers this workspace is configured with, without opening a
  session. `list` and `get` read what ConfigLoader merged — a project server
  the supply-chain gate skipped is shown as untrusted, with the document that
  declared it, rather than left out. `add` and `remove` write one server into
  ~/.zo/settings.json, or into <cwd>/.zo/settings.json with --project, and
  leave every other key of that document alone.

  A server is stdio unless --url names an endpoint; --transport picks one of
  zo's own transports explicitly. --env is the stdio server's environment and
  --header the remote one's headers; neither VALUE is ever printed back.

  A project server is behind the supply-chain gate: writing it into a document
  the repository carries is not consent to run it. --trust is that consent —
  it records this one name in the project's trust record, which counts only
  while it is your own uncommitted file — and without it `add` says in one
  line that the server is not loaded and how to trust it.

  `login` runs the OAuth flow for a remote server (inside the ZeroCode window
  the consent page opens in the window's own browser, where the person is
  already signed in) and `logout` forgets that server's token.
";

/// The name of the window's browser door, at the head of every pane's `PATH`.
/// Absent from a bare terminal, which is exactly how this CLI tells the two
/// apart — there is no window there to hold a signed-in tab.
const WINDOW_BROWSER: &str = "zerocode-browser";

/// The verbs, and whether each takes a server name first (Codex's grammar).
///
/// One table: the parser reads it to know what to consume AND to say which
/// word is not a verb. Asking for the name first meant `zo mcp frobnicate`
/// complained about a missing server name for a verb that does not exist.
const VERBS: [(&str, bool); 6] = [
    ("list", false),
    ("get", true),
    ("add", true),
    ("remove", true),
    ("login", true),
    ("logout", true),
];

#[derive(Debug, Clone, PartialEq, Eq)]
enum Verb {
    List,
    Get { name: String },
    Add { name: String, spec: AddSpec },
    Remove { name: String },
    Login { name: String, scopes: Vec<String> },
    Logout { name: String },
}

/// What `add` was asked to write, before a scope turns it into a file.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct AddSpec {
    url: Option<String>,
    transport: Option<McpTransport>,
    command: Vec<String>,
    env: BTreeMap<String, String>,
    headers: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Request {
    verb: Verb,
    /// Which document a write lands in. Reads merge every scope regardless.
    project: bool,
    /// Whether the person consented to the project server actually running.
    /// Separate from `project` on purpose: the document is the repository's,
    /// the consent is theirs.
    trust: bool,
    json: bool,
    cwd: Option<PathBuf>,
}

/// What the command printed.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub text: String,
}

/// # Errors
///
/// The usage, what was wrong with the arguments, or what stopped the verb —
/// an unknown server name, a document that cannot be written, a refused login.
pub fn run(args: &[String], cwd: &Path) -> Result<Report, String> {
    let request = parse(args)?;
    let cwd = request.cwd.clone().unwrap_or_else(|| cwd.to_path_buf());
    let loader = ConfigLoader::default_for(&cwd);
    answer(&request, &loader, &CredentialFile, &ConsentBrowser)
}

/// The stored MCP OAuth tokens, by server name.
///
/// The only place this CLI touches the credentials file. A test hands it a set
/// of its own rather than reaching into the person's `~/.zo/credentials.json`.
trait TokenStore {
    fn names(&self) -> Result<BTreeSet<String>, String>;
    fn forget(&self, name: &str) -> Result<(), String>;
}

struct CredentialFile;

impl TokenStore for CredentialFile {
    fn names(&self) -> Result<BTreeSet<String>, String> {
        runtime::list_mcp_oauth_servers()
            .map(|names| names.into_iter().collect())
            .map_err(|error| format!("could not read the stored MCP tokens: {error}"))
    }

    fn forget(&self, name: &str) -> Result<(), String> {
        runtime::clear_mcp_oauth_token(name)
            .map_err(|error| format!("could not forget {name}'s token: {error}"))
    }
}

/// Opening the consent page. Inside the `ZeroCode` window the pane's `PATH`
/// carries `zerocode-browser`, whose tabs hold the person's live signed-in
/// sessions — the door the window's own agents already use. Outside it there
/// is no such tab, so the system opener is the only browser there is.
struct ConsentBrowser;

impl BrowserOpener for ConsentBrowser {
    fn open_url(&self, url: &str) -> io::Result<()> {
        let opened = Command::new(WINDOW_BROWSER)
            .arg("open")
            .arg(url)
            .output()
            .is_ok_and(|output| output.status.success());
        if opened {
            return Ok(());
        }
        runtime::open_browser(url)
    }
}

fn parse(args: &[String]) -> Result<Request, String> {
    let (verb_word, rest) = match args.split_first() {
        Some((word, rest)) => (word.as_str(), rest),
        None => return Err(USAGE.to_string()),
    };
    if matches!(verb_word, "-h" | "--help") {
        return Err(USAGE.to_string());
    }
    let takes_a_name = VERBS
        .iter()
        .find_map(|(verb, takes_a_name)| (*verb == verb_word).then_some(*takes_a_name))
        .ok_or_else(|| format!("unknown `zo mcp` verb `{verb_word}`\n\n{USAGE}"))?;
    let (name, flags) = take_name(verb_word, takes_a_name, rest)?;
    let mut spec = AddSpec::default();
    let mut scopes = Vec::new();
    let mut request = Request {
        // Replaced below once the flags are read; `add` is the only verb whose
        // payload the flags carry.
        verb: Verb::List,
        project: false,
        trust: false,
        json: false,
        cwd: None,
    };
    // Indexed rather than an iterator: `--` hands the whole REST of the line to
    // the server's own command, and a closure that reads the next value cannot
    // borrow the same iterator the rest is drained from. It is read before the
    // closure exists for that reason.
    let mut index = 0;
    while index < flags.len() {
        let flag = flags[index].as_str();
        index += 1;
        if flag == "--" {
            spec.command = flags[index..].to_vec();
            break;
        }
        let mut value = || next_value(flags, &mut index, flag);
        match flag {
            "--json" => request.json = true,
            "--project" => request.project = true,
            "--trust" => request.trust = true,
            "--cwd" => request.cwd = Some(PathBuf::from(value()?)),
            "--url" => spec.url = Some(value()?),
            "--transport" => spec.transport = Some(transport_from_label(&value()?)?),
            "--env" => insert_pair(&mut spec.env, flag, &value()?)?,
            "--header" => insert_pair(&mut spec.headers, flag, &value()?)?,
            "--scopes" => scopes = split_scopes(&value()?),
            other => return Err(format!("unknown argument '{other}'\n\n{USAGE}")),
        }
    }
    request.verb = build_verb(verb_word, name, spec, scopes)?;
    // The record gates project-scoped servers and nothing else, so the flag is
    // refused where it would quietly mean nothing.
    if request.trust && !(request.project && matches!(request.verb, Verb::Add { .. })) {
        return Err(format!(
            "--trust is the consent `zo mcp add --project` asks for, and belongs to no other command line\n\n{USAGE}"
        ));
    }
    Ok(request)
}

/// The value after a flag, consumed from the same cursor the loop walks.
fn next_value(flags: &[String], index: &mut usize, flag: &str) -> Result<String, String> {
    let taken = flags.get(*index).cloned();
    *index += 1;
    taken.ok_or_else(|| format!("{flag} requires a value"))
}

/// The server name a verb takes before its flags.
fn take_name<'a>(
    verb: &str,
    takes_a_name: bool,
    rest: &'a [String],
) -> Result<(String, &'a [String]), String> {
    if !takes_a_name {
        return Ok((String::new(), rest));
    }
    match rest.split_first() {
        Some((name, flags)) if !name.starts_with('-') => Ok((name.clone(), flags)),
        _ => Err(format!("`zo mcp {verb}` requires a server name\n\n{USAGE}")),
    }
}

/// The verb word, now that its arguments are read. `VERBS` already refused
/// anything that is not one of these, so the last arm cannot be reached by a
/// command line — it is here so a row added to the table without a case here
/// is a compile-time hole nobody can walk through silently.
fn build_verb(verb: &str, name: String, spec: AddSpec, scopes: Vec<String>) -> Result<Verb, String> {
    Ok(match verb {
        "list" => Verb::List,
        "get" => Verb::Get { name },
        "add" => Verb::Add { name, spec },
        "remove" => Verb::Remove { name },
        "login" => Verb::Login { name, scopes },
        "logout" => Verb::Logout { name },
        other => return Err(format!("unknown `zo mcp` verb `{other}`\n\n{USAGE}")),
    })
}

/// `--transport` names one of zo's own transports. The labels are the settings
/// document's own tags, so this accepts exactly what the loader parses.
fn transport_from_label(label: &str) -> Result<McpTransport, String> {
    Ok(match label {
        "stdio" => McpTransport::Stdio,
        "http" => McpTransport::Http,
        "sse" => McpTransport::Sse,
        "ws" => McpTransport::Ws,
        other => {
            return Err(format!(
                "unknown transport '{other}' — one of stdio, http, sse, ws"
            ))
        }
    })
}

fn insert_pair(
    into: &mut BTreeMap<String, String>,
    flag: &str,
    pair: &str,
) -> Result<(), String> {
    let (key, value) = pair
        .split_once('=')
        .ok_or_else(|| format!("{flag} takes KEY=VALUE, not '{pair}'"))?;
    if key.is_empty() {
        return Err(format!("{flag} takes KEY=VALUE, not '{pair}'"));
    }
    into.insert(key.to_string(), value.to_string());
    Ok(())
}

fn split_scopes(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|scope| !scope.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn answer(
    request: &Request,
    loader: &ConfigLoader,
    tokens: &dyn TokenStore,
    browser: &dyn BrowserOpener,
) -> Result<Report, String> {
    match &request.verb {
        Verb::List => {
            let rows = rows(&load(loader)?, &tokens.names()?);
            Ok(render(request.json, &list_json(&rows), || list_text(&rows)))
        }
        Verb::Get { name } => {
            let config = load(loader)?;
            let row = rows(&config, &tokens.names()?)
                .into_iter()
                .find(|row| row.name == *name)
                .ok_or_else(|| unknown_server(name, &config))?;
            Ok(render(request.json, &row_json(&row), || row_text(&row)))
        }
        Verb::Add { name, spec } => add(request, loader, name, spec),
        Verb::Remove { name } => remove(request, loader, name),
        Verb::Login { name, scopes } => login(request, loader, name, scopes, browser),
        Verb::Logout { name } => logout(request, tokens, name),
    }
}

fn load(loader: &ConfigLoader) -> Result<RuntimeConfig, String> {
    loader
        .load()
        .map_err(|error| format!("settings could not be read: {error}"))
}

/// One answer in whichever form was asked for. The trailing newline belongs to
/// whoever prints the report, so `--json` and prose are the same shape here.
fn render(json: bool, value: &Value, text: impl FnOnce() -> String) -> Report {
    let rendered = if json { value.to_string() } else { text() };
    Report {
        text: rendered.trim_end().to_string(),
    }
}

fn unknown_server(name: &str, config: &RuntimeConfig) -> String {
    let known = config
        .mcp()
        .servers()
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    if known.is_empty() {
        return format!("no MCP server named '{name}' is configured");
    }
    format!(
        "no MCP server named '{name}' is configured — configured: {}",
        known.join(", ")
    )
}

// --- reading -----------------------------------------------------------

/// One server as this CLI reports it: what the loader knows, plus whether a
/// token is stored for it. Never a value out of `env` or `headers`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Row {
    name: String,
    scope: ConfigSource,
    /// `None` for a server the trust gate skipped: it was never parsed.
    config: Option<McpServerConfig>,
    /// The document that declared a gated server, and why it is not loaded.
    gated_by: Option<PathBuf>,
    authenticated: bool,
}

fn rows(config: &RuntimeConfig, authenticated: &BTreeSet<String>) -> Vec<Row> {
    let mut rows: Vec<Row> = config
        .mcp()
        .servers()
        .iter()
        .map(|(name, scoped)| Row {
            name: name.clone(),
            scope: scoped.scope,
            config: Some(scoped.config.clone()),
            gated_by: None,
            authenticated: authenticated.contains(name),
        })
        .collect();
    rows.extend(config.mcp().untrusted_project_servers().iter().map(|entry| {
        Row {
            name: entry.name.clone(),
            scope: ConfigSource::Project,
            config: None,
            gated_by: Some(entry.path.clone()),
            authenticated: authenticated.contains(&entry.name),
        }
    }));
    rows.sort_by(|left, right| left.name.cmp(&right.name));
    rows
}

const fn scope_label(scope: ConfigSource) -> &'static str {
    match scope {
        ConfigSource::User => "user",
        ConfigSource::Project => "project",
        ConfigSource::Local => "local",
    }
}

const fn transport_label(transport: McpTransport) -> &'static str {
    match transport {
        McpTransport::Stdio => "stdio",
        McpTransport::Sse => "sse",
        McpTransport::Http => "http",
        McpTransport::Ws => "ws",
        McpTransport::Sdk => "sdk",
        McpTransport::ManagedProxy => "claudeai-proxy",
    }
}

const fn transport_of(config: &McpServerConfig) -> McpTransport {
    match config {
        McpServerConfig::Stdio(_) => McpTransport::Stdio,
        McpServerConfig::Sse(_) => McpTransport::Sse,
        McpServerConfig::Http(_) => McpTransport::Http,
        McpServerConfig::Ws(_) => McpTransport::Ws,
        McpServerConfig::Sdk(_) => McpTransport::Sdk,
        McpServerConfig::ManagedProxy(_) => McpTransport::ManagedProxy,
    }
}

/// The one line that says where a server actually is: a command line for
/// stdio, an endpoint for everything else.
fn endpoint(config: &McpServerConfig) -> String {
    match config {
        McpServerConfig::Stdio(stdio) => {
            std::iter::once(stdio.command.as_str())
                .chain(stdio.args.iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join(" ")
        }
        McpServerConfig::Sse(remote) | McpServerConfig::Http(remote) => remote.url.clone(),
        McpServerConfig::Ws(ws) => ws.url.clone(),
        McpServerConfig::Sdk(sdk) => sdk.name.clone(),
        McpServerConfig::ManagedProxy(proxy) => proxy.url.clone(),
    }
}

/// The names in a secret-bearing map. The values stay in the settings file.
fn keys(map: &BTreeMap<String, String>) -> Vec<&str> {
    map.keys().map(String::as_str).collect()
}

fn secret_keys(config: &McpServerConfig) -> (Vec<&str>, Vec<&str>) {
    match config {
        McpServerConfig::Stdio(stdio) => (keys(&stdio.env), Vec::new()),
        McpServerConfig::Sse(remote) | McpServerConfig::Http(remote) => {
            (Vec::new(), keys(&remote.headers))
        }
        McpServerConfig::Ws(ws) => (Vec::new(), keys(&ws.headers)),
        McpServerConfig::Sdk(_) | McpServerConfig::ManagedProxy(_) => (Vec::new(), Vec::new()),
    }
}

fn row_json(row: &Row) -> Value {
    let mut value = json!({
        "name": row.name,
        "scope": scope_label(row.scope),
        "trusted": row.config.is_some(),
        "authenticated": row.authenticated,
    });
    if let Some(path) = &row.gated_by {
        value["gatedBy"] = json!(path.display().to_string());
    }
    if let Some(config) = &row.config {
        let (env, headers) = secret_keys(config);
        value["transport"] = json!(transport_label(transport_of(config)));
        value["endpoint"] = json!(endpoint(config));
        // Keys only: the values are the server's tokens.
        value["envKeys"] = json!(env);
        value["headerKeys"] = json!(headers);
        if let McpServerConfig::Stdio(stdio) = config {
            value["command"] = json!(stdio.command);
            value["args"] = json!(stdio.args);
        }
    }
    value
}

fn list_json(rows: &[Row]) -> Value {
    json!({ "servers": rows.iter().map(row_json).collect::<Vec<Value>>() })
}

fn list_text(rows: &[Row]) -> String {
    if rows.is_empty() {
        return "no MCP servers configured — `zo mcp add <name> -- <command>` writes one\n"
            .to_string();
    }
    let mut out = String::new();
    let _ = writeln!(out, "{:<24} {:<14} {:<8} {:<8} WHERE", "NAME", "TRANSPORT", "SCOPE", "AUTH");
    for row in rows {
        let (transport, where_) = row.config.as_ref().map_or_else(
            || {
                (
                    "—".to_string(),
                    format!("gated — {}", gate_reason(row.gated_by.as_deref())),
                )
            },
            |config| {
                (
                    transport_label(transport_of(config)).to_string(),
                    endpoint(config),
                )
            },
        );
        let _ = writeln!(
            out,
            "{:<24} {:<14} {:<8} {:<8} {}",
            row.name,
            transport,
            scope_label(row.scope),
            if row.authenticated { "token" } else { "—" },
            where_
        );
    }
    out
}

/// Why a project-scoped server is not loaded, and the two ways to load it.
/// The record is named from the loader's own spelling of it, so a rename there
/// cannot leave this sentence pointing at a file nobody reads.
fn gate_reason(path: Option<&Path>) -> String {
    format!(
        "{} is not trusted ({ZO_DIR_NAME}/{}, or enableAllProjectMcpServers)",
        path.map_or_else(
            || format!("{ZO_DIR_NAME}/settings.json"),
            |path| path.display().to_string()
        ),
        runtime::TRUSTED_MCP_SERVERS_FILE,
    )
}

fn row_text(row: &Row) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{:<12} {}", "name", row.name);
    let _ = writeln!(out, "{:<12} {}", "scope", scope_label(row.scope));
    let Some(config) = &row.config else {
        let _ = writeln!(out, "{:<12} no", "trusted");
        let _ = writeln!(out, "{:<12} {}", "gated by", gate_reason(row.gated_by.as_deref()));
        return out;
    };
    let _ = writeln!(out, "{:<12} yes", "trusted");
    let _ = writeln!(
        out,
        "{:<12} {}",
        "transport",
        transport_label(transport_of(config))
    );
    let _ = writeln!(out, "{:<12} {}", "endpoint", endpoint(config));
    let _ = writeln!(
        out,
        "{:<12} {}",
        "auth",
        if row.authenticated {
            "token stored"
        } else {
            "no token stored"
        }
    );
    let (env, headers) = secret_keys(config);
    for (label, names) in [("env", env), ("headers", headers)] {
        if !names.is_empty() {
            // Keys, never values.
            let _ = writeln!(out, "{:<12} {} (values not shown)", label, names.join(", "));
        }
    }
    out
}

// --- writing -----------------------------------------------------------

/// Which document a write lands in — the scope the person picked, resolved to
/// the same path `ConfigLoader` discovers for it.
fn scope_path(request: &Request, loader: &ConfigLoader) -> PathBuf {
    if request.project {
        loader.cwd().join(".zo").join("settings.json")
    } else {
        loader.config_home().join("settings.json")
    }
}

fn add(
    request: &Request,
    loader: &ConfigLoader,
    name: &str,
    spec: &AddSpec,
) -> Result<Report, String> {
    let config = server_from_spec(name, spec)?;
    let path = scope_path(request, loader);
    let edit = runtime::write_mcp_server(&path, name, &config)
        .map_err(|error| format!("{name} could not be written: {error}"))?;
    // Consent is its own act, and its own document. A project settings file is
    // carried by the repository to everyone who clones it, so the name reaches
    // the gate's record only because this command line said `--trust`.
    let trusted = request.trust.then(|| trust(loader, name)).transpose()?;
    // A project document is behind the supply-chain gate, so "written" is not
    // yet "will load" — and neither is "recorded", since the record counts only
    // as the operator's own uncommitted file. Ask the loader rather than guess:
    // a person who just added a server wants to know whether the next session
    // will run it.
    let gated = request.project && !load(loader)?.mcp().servers().contains_key(name);
    let mut report = json!({
        "server": name,
        "edit": edit_label(edit),
        "path": path.display().to_string(),
        "transport": transport_label(transport_of(&config)),
        "gated": gated,
    });
    if let Some((record, trust_edit)) = &trusted {
        report["trustEdit"] = json!(edit_label(*trust_edit));
        report["trustPath"] = json!(record.display().to_string());
    }
    Ok(render(request.json, &report, || {
        let mut text = format!("{} {name} in {}", past_tense(edit), path.display());
        if let Some((record, trust_edit)) = &trusted {
            let _ = write!(
                text,
                "\ntrusted {name} — {} in {}",
                edit_label(*trust_edit),
                record.display()
            );
        }
        if gated {
            let _ = write!(
                text,
                "\n{}",
                still_gated(&path, trusted.as_ref().map(|(record, _)| record.as_path()))
            );
        }
        text
    }))
}

/// Record the name in the gate's own document — never in the settings one,
/// which is the file the repository carries.
fn trust(loader: &ConfigLoader, name: &str) -> Result<(PathBuf, McpEdit), String> {
    let path = loader.trusted_mcp_servers_path();
    let edit = runtime::trust_mcp_server(&path, name)
        .map_err(|error| format!("{name} could not be trusted: {error}"))?;
    Ok((path, edit))
}

/// The line a project `add` ends on while the server still will not load.
/// Without consent the fix is the flag. With it the record exists and the gate
/// did not believe it, which is a different sentence and a different fix — the
/// record authorizes only as the operator's own uncommitted file, so that a
/// repository cannot ship consent to run its own MCP command.
fn still_gated(document: &Path, record: Option<&Path>) -> String {
    match record {
        None => format!(
            "it will not load until it is trusted — {} (--trust records it)",
            gate_reason(Some(document))
        ),
        Some(record) => format!(
            "it still will not load: {} counts only as your own uncommitted file — a git-tracked or symlinked one, or a nested .zo repository, fails closed",
            record.display()
        ),
    }
}

fn remove(request: &Request, loader: &ConfigLoader, name: &str) -> Result<Report, String> {
    let path = scope_path(request, loader);
    let edit = runtime::remove_mcp_server(&path, name)
        .map_err(|error| format!("{name} could not be removed: {error}"))?;
    if edit == McpEdit::Absent {
        return Err(format!(
            "no MCP server named '{name}' in {}",
            path.display()
        ));
    }
    Ok(render(
        request.json,
        &json!({
            "server": name,
            "edit": edit_label(edit),
            "path": path.display().to_string(),
        }),
        || format!("removed {name} from {}", path.display()),
    ))
}

const fn edit_label(edit: McpEdit) -> &'static str {
    match edit {
        McpEdit::Added => "added",
        McpEdit::Replaced => "replaced",
        McpEdit::Unchanged => "unchanged",
        McpEdit::Removed => "removed",
        McpEdit::Absent => "absent",
    }
}

const fn past_tense(edit: McpEdit) -> &'static str {
    match edit {
        McpEdit::Added => "added",
        McpEdit::Replaced => "replaced",
        McpEdit::Unchanged => "left unchanged",
        McpEdit::Removed => "removed",
        McpEdit::Absent => "did not find",
    }
}

/// Turn what the command line asked for into the config the loader parses.
///
/// The transport is inferred the way the settings document infers it — a `url`
/// means remote, its absence means stdio — unless `--transport` names one.
fn server_from_spec(name: &str, spec: &AddSpec) -> Result<McpServerConfig, String> {
    let transport = spec.transport.unwrap_or(if spec.url.is_some() {
        McpTransport::Http
    } else {
        McpTransport::Stdio
    });
    if matches!(transport, McpTransport::Stdio) {
        let (command, args) = spec
            .command
            .split_first()
            .ok_or_else(|| format!("`zo mcp add {name}` needs `-- <command> [args…]`\n\n{USAGE}"))?;
        if !spec.headers.is_empty() {
            return Err("--header belongs to a remote server; a stdio one takes --env".to_string());
        }
        return Ok(McpServerConfig::Stdio(McpStdioServerConfig {
            command: command.clone(),
            args: args.to_vec(),
            env: spec.env.clone(),
            tool_call_timeout_ms: None,
        }));
    }
    let url = spec
        .url
        .clone()
        .ok_or_else(|| format!("`zo mcp add {name} --transport {}` needs --url", transport_label(transport)))?;
    if !spec.env.is_empty() {
        return Err("--env belongs to a stdio server; a remote one takes --header".to_string());
    }
    if !spec.command.is_empty() {
        return Err("a server has either --url or a `-- <command>`, not both".to_string());
    }
    let remote = McpRemoteServerConfig {
        url,
        headers: spec.headers.clone(),
        headers_helper: None,
        oauth: None,
    };
    Ok(match transport {
        McpTransport::Sse => McpServerConfig::Sse(remote),
        McpTransport::Ws => McpServerConfig::Ws(McpWebSocketServerConfig {
            url: remote.url,
            headers: remote.headers,
            headers_helper: None,
        }),
        _ => McpServerConfig::Http(remote),
    })
}

// --- login / logout ----------------------------------------------------

fn login(
    request: &Request,
    loader: &ConfigLoader,
    name: &str,
    scopes: &[String],
    browser: &dyn BrowserOpener,
) -> Result<Report, String> {
    let config = load(loader)?;
    let configured = config
        .mcp()
        .get(name)
        .ok_or_else(|| unknown_server(name, &config))?;
    let remote = match &configured.config {
        McpServerConfig::Sse(remote) | McpServerConfig::Http(remote) => remote.clone(),
        other => {
            return Err(format!(
                "{name} is a {} server; OAuth is for an http or sse one",
                transport_label(transport_of(other))
            ))
        }
    };
    let result = runtime::mcp_oauth::authenticate_mcp_server_remote_with_scopes(
        name, &remote, scopes, browser,
    );
    let (token, detail) = auth_outcome(&result);
    if token == "failed" {
        return Err(format!("{name} could not be authenticated: {detail}"));
    }
    Ok(render(
        request.json,
        &json!({ "server": name, "result": token, "detail": detail }),
        || format!("{name}: {token}{}\n", suffix(&detail)),
    ))
}

/// The four ways an authentication attempt ends, as one token and one detail.
fn auth_outcome(result: &McpAuthResult) -> (&'static str, String) {
    match result {
        McpAuthResult::AlreadyAuthenticated { .. } => ("already-authenticated", String::new()),
        McpAuthResult::Authenticated { scopes, .. } => ("authenticated", scopes.join(" ")),
        McpAuthResult::Refreshed { .. } => ("refreshed", String::new()),
        McpAuthResult::Failed { reason, .. } => ("failed", reason.clone()),
    }
}

fn suffix(detail: &str) -> String {
    if detail.is_empty() {
        String::new()
    } else {
        format!(" ({detail})")
    }
}

fn logout(request: &Request, tokens: &dyn TokenStore, name: &str) -> Result<Report, String> {
    // A token can outlive the settings entry that produced it, so logout asks
    // the store, never the config: forgetting a server you already removed is
    // exactly when you need this.
    let held = tokens.names()?.contains(name);
    if held {
        tokens.forget(name)?;
    }
    Ok(render(
        request.json,
        &json!({ "server": name, "forgotten": held }),
        || {
            if held {
                format!("forgot {name}'s token\n")
            } else {
                format!("no token was stored for {name}\n")
            }
        },
    ))
}

#[cfg(test)]
mod tests;
