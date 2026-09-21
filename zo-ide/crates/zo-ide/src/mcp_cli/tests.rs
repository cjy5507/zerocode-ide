//! Every test here drives `answer` against a config home and a workspace of
//! its own, with a token store it constructed — the person's `~/.zo` and their
//! credentials file are never read, and nothing is written outside the
//! temporary tree.

use std::cell::RefCell;

use tempfile::TempDir;

use super::*;

/// A settings tree nobody else shares: a config home for the `user` scope and
/// a workspace for the `project` one.
struct Scratch {
    home: TempDir,
    cwd: TempDir,
}

impl Scratch {
    fn new() -> Self {
        Self {
            home: tempfile::tempdir().expect("config home"),
            cwd: tempfile::tempdir().expect("workspace"),
        }
    }

    fn loader(&self) -> ConfigLoader {
        ConfigLoader::new(self.cwd.path(), self.home.path())
    }

    fn home_settings(&self) -> PathBuf {
        self.home.path().join("settings.json")
    }

    fn project_settings(&self) -> PathBuf {
        self.cwd.path().join(".zo").join("settings.json")
    }

    /// Run one command line, with no stored tokens and a browser nobody opens.
    fn run(&self, line: &[&str]) -> Result<String, String> {
        self.run_with(line, &Tokens::empty())
    }

    fn run_with(&self, line: &[&str], tokens: &Tokens) -> Result<String, String> {
        let args = line.iter().map(|word| (*word).to_string()).collect::<Vec<_>>();
        let request = parse(&args)?;
        answer(&request, &self.loader(), tokens, &RefusingBrowser).map(|report| report.text)
    }

    /// The operator-authored opt-in that takes the supply-chain gate off this
    /// workspace's project servers — written the way a person would, straight
    /// into the home document beside whatever `add` already put there.
    fn opt_the_project_in(&self) {
        let path = self.home_settings();
        let mut document: Value = serde_json::from_str(
            &std::fs::read_to_string(&path).expect("home settings"),
        )
        .expect("home settings are JSON");
        document["enableAllProjectMcpServers"] = json!(true);
        std::fs::write(&path, document.to_string()).expect("opt the project in");
    }

    /// `--json` goes where a person would put it — before any `--`, which
    /// hands the rest of the line to the server.
    fn json(&self, line: &[&str]) -> Value {
        serde_json::from_str(&self.run(line).expect("json answer")).expect("valid JSON")
    }
}

/// A token store in memory. `forget` records what it was asked to forget, so a
/// test can see the store was reached without a credentials file existing.
struct Tokens {
    held: RefCell<BTreeSet<String>>,
    forgotten: RefCell<Vec<String>>,
}

impl Tokens {
    fn empty() -> Self {
        Self {
            held: RefCell::new(BTreeSet::new()),
            forgotten: RefCell::new(Vec::new()),
        }
    }

    fn holding(names: &[&str]) -> Self {
        let store = Self::empty();
        *store.held.borrow_mut() = names.iter().map(|name| (*name).to_string()).collect();
        store
    }
}

impl TokenStore for Tokens {
    fn names(&self) -> Result<BTreeSet<String>, String> {
        Ok(self.held.borrow().clone())
    }

    fn forget(&self, name: &str) -> Result<(), String> {
        self.held.borrow_mut().remove(name);
        self.forgotten.borrow_mut().push(name.to_string());
        Ok(())
    }
}

/// A browser that fails if anything reaches it. No test here should open one:
/// a login that got as far as the consent page would be a test that needs the
/// network.
struct RefusingBrowser;

impl BrowserOpener for RefusingBrowser {
    fn open_url(&self, url: &str) -> io::Result<()> {
        panic!("a test opened a browser at {url}");
    }
}

#[test]
fn the_usage_names_every_verb_and_flag_the_parser_takes() {
    for word in [
        "list", "get", "add", "remove", "login", "logout", "--json", "--project", "--cwd",
        "--url", "--transport", "--env", "--header", "--scopes", "stdio", "http", "sse", "ws",
    ] {
        assert!(USAGE.contains(word), "usage says nothing of {word}");
    }
    assert_eq!(parse(&[]).unwrap_err(), USAGE);
    assert_eq!(parse(&["--help".to_string()]).unwrap_err(), USAGE);
}

/// `zo --help` is where a person looks for a verb; a door only the subcommand
/// admits to is a door nobody finds (the trap that kept `jev` unfindable).
#[test]
fn the_top_level_usage_names_the_verb() {
    assert!(crate::ide::args::USAGE.contains("zo mcp "));
    for verb in ["list", "get", "add", "remove", "login", "logout"] {
        assert!(
            crate::ide::args::USAGE.contains(verb),
            "zo --help says nothing of `mcp {verb}`"
        );
    }
}

#[test]
fn a_verb_that_needs_a_name_refuses_without_one() {
    for verb in ["get", "add", "remove", "login", "logout"] {
        let error = parse(&[verb.to_string()]).expect_err("refusal");
        assert!(error.contains("requires a server name"), "{verb}: {error}");
    }
    assert!(parse(&["list".to_string()]).is_ok());
}

#[test]
fn an_unknown_verb_and_an_unknown_flag_are_refused_with_the_usage() {
    for line in [vec!["frobnicate"], vec!["frobnicate", "name"]] {
        let args = line.iter().map(|word| (*word).to_string()).collect::<Vec<_>>();
        let error = parse(&args).expect_err("refusal");
        // Not "requires a server name": the word is not a verb at all, and
        // saying so is what tells a person they misspelled one.
        assert!(error.contains("unknown `zo mcp` verb"), "{line:?}: {error}");
    }
    let error = parse(&["list".to_string(), "--bogus".to_string()]).expect_err("refusal");
    assert!(error.contains("unknown argument"), "{error}");
}

#[test]
fn everything_after_the_double_dash_is_the_servers_own_command_line() {
    let args = ["add", "s", "--env", "K=V", "--", "npx", "-y", "--json", "pkg"]
        .iter()
        .map(|word| (*word).to_string())
        .collect::<Vec<_>>();
    let request = parse(&args).expect("parse");
    let Verb::Add { spec, .. } = request.verb else {
        panic!("not an add");
    };
    // `--json` after the `--` belongs to the server, not to zo.
    assert!(!request.json);
    assert_eq!(spec.command, ["npx", "-y", "--json", "pkg"]);
    assert_eq!(spec.env["K"], "V");
}

#[test]
fn a_pair_flag_needs_a_key_and_a_value() {
    for pair in ["NOEQUALS", "=value"] {
        let args = ["add", "s", "--env", pair, "--", "npx"]
            .iter()
            .map(|word| (*word).to_string())
            .collect::<Vec<_>>();
        let error = parse(&args).expect_err("refusal");
        assert!(error.contains("KEY=VALUE"), "{pair}: {error}");
    }
}

// --- the round trip ----------------------------------------------------

/// The contract the unit stands on: what `add` wrote, `ConfigLoader` reads —
/// for a stdio server and a `--url` one, in the user scope and the project one.
#[test]
fn what_add_wrote_the_loader_reads_back_in_both_scopes_and_both_transports() {
    let scratch = Scratch::new();
    scratch
        .run(&["add", "local", "--env", "TOKEN=fixture", "--", "npx", "-y", "pkg"])
        .expect("add stdio");
    scratch
        .run(&["add", "remote", "--url", "https://example.test/mcp"])
        .expect("add remote");
    // The project scope is behind the supply-chain gate, so the operator's own
    // document opts this workspace in — the same door `list` reports on.
    scratch
        .run(&["add", "repo", "--project", "--", "uvx", "repo-server"])
        .expect("add project");

    let expected_local = McpServerConfig::Stdio(McpStdioServerConfig {
        command: "npx".to_string(),
        args: vec!["-y".to_string(), "pkg".to_string()],
        env: [("TOKEN".to_string(), "fixture".to_string())]
            .into_iter()
            .collect(),
        tool_call_timeout_ms: None,
    });
    let expected_remote = McpServerConfig::Http(McpRemoteServerConfig {
        url: "https://example.test/mcp".to_string(),
        headers: BTreeMap::new(),
        headers_helper: None,
        oauth: None,
    });

    let config = scratch.loader().load().expect("load");
    let servers = config.mcp().servers();
    assert_eq!(servers["local"].config, expected_local);
    assert_eq!(servers["local"].scope, ConfigSource::User);
    assert_eq!(servers["remote"].config, expected_remote);
    // Gated until trusted, and named as such rather than missing.
    assert!(!servers.contains_key("repo"));
    assert_eq!(config.mcp().untrusted_project_servers()[0].name, "repo");

    scratch.opt_the_project_in();
    let config = scratch.loader().load().expect("load");
    assert_eq!(
        config.mcp().servers()["repo"].config,
        McpServerConfig::Stdio(McpStdioServerConfig {
            command: "uvx".to_string(),
            args: vec!["repo-server".to_string()],
            env: BTreeMap::new(),
            tool_call_timeout_ms: None,
        })
    );
    assert_eq!(config.mcp().servers()["repo"].scope, ConfigSource::Project);
}

#[test]
fn each_scope_writes_its_own_document() {
    let scratch = Scratch::new();
    scratch.run(&["add", "u", "--", "npx"]).expect("add user");
    scratch
        .run(&["add", "p", "--project", "--", "npx"])
        .expect("add project");
    let home = std::fs::read_to_string(scratch.home_settings()).expect("home settings");
    let project = std::fs::read_to_string(scratch.project_settings()).expect("project settings");
    assert!(home.contains("\"u\"") && !home.contains("\"p\""), "{home}");
    assert!(project.contains("\"p\"") && !project.contains("\"u\""), "{project}");
}

#[test]
fn removing_a_name_nobody_configured_exits_with_a_refusal() {
    let scratch = Scratch::new();
    let error = scratch.run(&["remove", "ghost"]).expect_err("refusal");
    assert!(error.contains("no MCP server named 'ghost'"), "{error}");

    scratch.run(&["add", "here", "--", "npx"]).expect("add");
    // The scope is part of the question: the same name in the other document
    // is not this document's server.
    let error = scratch
        .run(&["remove", "here", "--project"])
        .expect_err("refusal");
    assert!(error.contains("no MCP server named 'here'"), "{error}");
    assert!(scratch.run(&["remove", "here"]).is_ok());
    assert!(scratch.loader().load().expect("load").mcp().servers().is_empty());
}

#[test]
fn getting_a_name_nobody_configured_exits_with_a_refusal_that_names_the_rest() {
    let scratch = Scratch::new();
    let error = scratch.run(&["get", "ghost"]).expect_err("refusal");
    assert!(error.contains("no MCP server named 'ghost'"), "{error}");
    scratch.run(&["add", "real", "--", "npx"]).expect("add");
    let error = scratch.run(&["get", "ghost"]).expect_err("refusal");
    assert!(error.contains("configured: real"), "{error}");
}

// --- what the reports say ----------------------------------------------

#[test]
fn list_json_is_one_row_per_server_with_the_gated_one_marked_and_its_reason() {
    let scratch = Scratch::new();
    scratch
        .run(&["add", "local", "--env", "TOKEN=fixture", "--", "npx", "-y", "pkg"])
        .expect("add");
    scratch
        .run(&["add", "repo", "--project", "--", "uvx", "repo-server"])
        .expect("add project");

    let listed = scratch.json(&["list", "--json"]);
    let servers = listed["servers"].as_array().expect("servers");
    assert_eq!(servers.len(), 2);

    let local = &servers[0];
    assert_eq!(local["name"], "local");
    assert_eq!(local["transport"], "stdio");
    assert_eq!(local["scope"], "user");
    assert_eq!(local["trusted"], true);
    assert_eq!(local["authenticated"], false);
    assert_eq!(local["endpoint"], "npx -y pkg");
    assert_eq!(local["envKeys"], json!(["TOKEN"]));

    let repo = &servers[1];
    assert_eq!(repo["name"], "repo");
    assert_eq!(repo["trusted"], false);
    assert_eq!(
        repo["gatedBy"],
        json!(scratch.project_settings().display().to_string())
    );
}

/// The one rule the whole reading surface has: a value out of `env` or
/// `headers` is the server's token and never leaves the settings file.
#[test]
fn no_report_ever_prints_an_env_value_or_a_header_value() {
    let scratch = Scratch::new();
    scratch
        .run(&["add", "local", "--env", "TOKEN=must-not-be-printed", "--", "npx"])
        .expect("add");
    scratch
        .run(&[
            "add",
            "remote",
            "--url",
            "https://example.test/mcp",
            "--header",
            "Authorization=Bearer must-not-be-printed",
        ])
        .expect("add");
    // It reached the settings file, which is the point: the value is stored,
    // and only the reading surface withholds it.
    assert!(std::fs::read_to_string(scratch.home_settings())
        .expect("settings")
        .contains("must-not-be-printed"));
    for line in [
        vec!["list"],
        vec!["list", "--json"],
        vec!["get", "local"],
        vec!["get", "local", "--json"],
        vec!["get", "remote"],
        vec!["get", "remote", "--json"],
    ] {
        let printed = scratch.run(&line).expect("report");
        assert!(
            !printed.contains("must-not-be-printed"),
            "`zo mcp {}` printed a secret:\n{printed}",
            line.join(" ")
        );
    }
    // The KEY is the useful half and is not a secret: `get` names it so a
    // person can see which variable the server wants set.
    assert!(scratch.run(&["get", "local"]).expect("report").contains("TOKEN"));
    assert!(scratch
        .run(&["get", "remote"])
        .expect("report")
        .contains("Authorization"));
}

#[test]
fn an_empty_list_says_how_to_write_the_first_server() {
    let scratch = Scratch::new();
    let printed = scratch.run(&["list"]).expect("report");
    assert!(printed.contains("no MCP servers configured"), "{printed}");
    assert!(printed.contains("zo mcp add"), "{printed}");
    assert_eq!(scratch.json(&["list", "--json"])["servers"], json!([]));
}

#[test]
fn a_stored_token_shows_up_against_its_server() {
    let scratch = Scratch::new();
    scratch
        .run(&["add", "remote", "--url", "https://example.test/mcp"])
        .expect("add");
    let tokens = Tokens::holding(&["remote"]);
    let printed = scratch.run_with(&["get", "remote"], &tokens).expect("report");
    assert!(printed.contains("token stored"), "{printed}");
    let printed = scratch
        .run_with(&["get", "remote", "--json"], &tokens)
        .expect("report");
    assert!(printed.contains("\"authenticated\":true"), "{printed}");
}

// --- writing edge cases ------------------------------------------------

#[test]
fn a_second_add_of_the_same_name_replaces_it_and_says_so() {
    let scratch = Scratch::new();
    let add = |command: &str| scratch.json(&["add", "s", "--json", "--", command]);
    assert_eq!(add("one")["edit"], "added");
    assert_eq!(add("one")["edit"], "unchanged");
    assert_eq!(add("two")["edit"], "replaced");
    let config = scratch.loader().load().expect("load");
    assert_eq!(
        config.mcp().servers()["s"].config,
        McpServerConfig::Stdio(McpStdioServerConfig {
            command: "two".to_string(),
            args: Vec::new(),
            env: BTreeMap::new(),
            tool_call_timeout_ms: None,
        })
    );
}

#[test]
fn transport_chooses_among_zos_own_and_the_url_decides_by_default() {
    let scratch = Scratch::new();
    scratch
        .run(&["add", "sse", "--transport", "sse", "--url", "https://e.test/sse"])
        .expect("add sse");
    scratch
        .run(&["add", "ws", "--transport", "ws", "--url", "wss://e.test/ws"])
        .expect("add ws");
    scratch
        .run(&["add", "http", "--url", "https://e.test/mcp"])
        .expect("add http");
    let config = scratch.loader().load().expect("load");
    let servers = config.mcp().servers();
    assert!(matches!(servers["sse"].config, McpServerConfig::Sse(_)));
    assert!(matches!(servers["ws"].config, McpServerConfig::Ws(_)));
    assert!(matches!(servers["http"].config, McpServerConfig::Http(_)));
}

#[test]
fn a_server_is_either_a_command_or_a_url_and_the_flags_follow_the_transport() {
    let scratch = Scratch::new();
    let error = scratch.run(&["add", "s"]).expect_err("refusal");
    assert!(error.contains("needs `-- <command>"), "{error}");

    let error = scratch
        .run(&["add", "s", "--transport", "http"])
        .expect_err("refusal");
    assert!(error.contains("needs --url"), "{error}");

    let error = scratch
        .run(&["add", "s", "--url", "https://e.test", "--", "npx"])
        .expect_err("refusal");
    assert!(error.contains("not both"), "{error}");

    let error = scratch
        .run(&["add", "s", "--url", "https://e.test", "--env", "K=V"])
        .expect_err("refusal");
    assert!(error.contains("--env belongs to a stdio server"), "{error}");

    let error = scratch
        .run(&["add", "s", "--header", "K=V", "--", "npx"])
        .expect_err("refusal");
    assert!(error.contains("--header belongs to a remote server"), "{error}");

    let error = scratch
        .run(&["add", "s", "--transport", "carrier-pigeon", "--url", "x"])
        .expect_err("refusal");
    assert!(error.contains("unknown transport"), "{error}");
}

/// The document is somebody's: an edit that touched one key must leave the
/// rest of their settings exactly as they wrote them.
#[test]
fn an_add_leaves_the_rest_of_the_persons_settings_alone() {
    let scratch = Scratch::new();
    std::fs::write(
        scratch.home_settings(),
        r#"{"model":"fable","smart":{"autoClassifier":"off"}}"#,
    )
    .expect("seed settings");
    scratch.run(&["add", "s", "--", "npx"]).expect("add");
    let config = scratch.loader().load().expect("load");
    assert_eq!(config.model(), Some("fable"));
    assert!(config.mcp().servers().contains_key("s"));
    // And a person can still read it.
    let written = std::fs::read_to_string(scratch.home_settings()).expect("settings");
    assert!(written.lines().count() > 1, "written on one line:\n{written}");
}

// --- login / logout ----------------------------------------------------

#[test]
fn login_refuses_a_server_nobody_configured_and_one_that_has_no_oauth() {
    let scratch = Scratch::new();
    let error = scratch.run(&["login", "ghost"]).expect_err("refusal");
    assert!(error.contains("no MCP server named 'ghost'"), "{error}");

    scratch.run(&["add", "local", "--", "npx"]).expect("add");
    let error = scratch.run(&["login", "local"]).expect_err("refusal");
    assert!(error.contains("is a stdio server"), "{error}");
}

#[test]
fn the_scopes_flag_is_split_on_commas() {
    let args = ["login", "s", "--scopes", "read, write ,"]
        .iter()
        .map(|word| (*word).to_string())
        .collect::<Vec<_>>();
    let Verb::Login { scopes, .. } = parse(&args).expect("parse").verb else {
        panic!("not a login");
    };
    assert_eq!(scopes, ["read", "write"]);
}

/// Logout asks the token store, never the config: forgetting a server you
/// already removed is exactly when you need it.
#[test]
fn logout_forgets_a_stored_token_and_says_so_when_there_was_none() {
    let scratch = Scratch::new();
    let tokens = Tokens::holding(&["remote"]);
    let printed = scratch
        .run_with(&["logout", "remote"], &tokens)
        .expect("logout");
    assert!(printed.contains("forgot remote's token"), "{printed}");
    assert_eq!(tokens.forgotten.borrow().as_slice(), ["remote"]);

    let printed = scratch
        .run_with(&["logout", "remote"], &tokens)
        .expect("logout");
    assert!(printed.contains("no token was stored"), "{printed}");
    assert_eq!(tokens.forgotten.borrow().len(), 1);
}
