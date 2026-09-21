//! The writer is only correct if the loader reads back what it wrote. Every
//! test here ends at [`ConfigLoader`] or at [`parse_mcp_server_config`] — never
//! at the bytes, which are an implementation detail of the renderer.

use std::collections::BTreeMap;

use tempfile::TempDir;

use super::*;
use crate::config::ConfigLoader;

fn stdio(command: &str, args: &[&str], env: &[(&str, &str)]) -> McpServerConfig {
    McpServerConfig::Stdio(McpStdioServerConfig {
        command: command.to_string(),
        args: args.iter().map(|arg| (*arg).to_string()).collect(),
        env: env
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect(),
        tool_call_timeout_ms: None,
    })
}

fn http(url: &str) -> McpServerConfig {
    McpServerConfig::Http(McpRemoteServerConfig {
        url: url.to_string(),
        headers: BTreeMap::new(),
        headers_helper: None,
        oauth: None,
    })
}

/// A settings document written by the writer and read by the loader, in a
/// config home of its own — no environment, no developer's real `~/.zo`.
///
/// The guard is the one `config::tests` measured into existence: `load`
/// reads the process-wide `--settings` cell, and a test that loads while
/// another is setting it sees a document nobody wrote.
fn load_at(home: &TempDir, cwd: &TempDir) -> crate::RuntimeConfig {
    let _stable = crate::config::tests::overrides_stable();
    ConfigLoader::new(cwd.path(), home.path())
        .load()
        .expect("load")
}

#[test]
fn every_transport_survives_a_write_and_a_parse() {
    let dir = tempfile::tempdir().expect("temporary settings dir");
    let path = dir.path().join("settings.json");
    let cases = [
        ("stdio", stdio("npx", &["-y", "server"], &[("TOKEN", "t")])),
        ("http", http("https://example.test/mcp")),
        (
            "sse",
            McpServerConfig::Sse(McpRemoteServerConfig {
                url: "https://example.test/sse".to_string(),
                headers: [("X-Key".to_string(), "v".to_string())].into_iter().collect(),
                headers_helper: Some("helper.sh".to_string()),
                oauth: Some(McpOAuthConfig {
                    client_id: Some("client".to_string()),
                    callback_port: Some(9_999),
                    auth_server_metadata_url: Some("https://auth.test/.well-known".to_string()),
                    xaa: Some(true),
                }),
            }),
        ),
        (
            "ws",
            McpServerConfig::Ws(McpWebSocketServerConfig {
                url: "wss://example.test/ws".to_string(),
                headers: BTreeMap::new(),
                headers_helper: None,
            }),
        ),
        (
            "sdk",
            McpServerConfig::Sdk(McpSdkServerConfig {
                name: "in-process".to_string(),
            }),
        ),
        (
            "proxy",
            McpServerConfig::ManagedProxy(McpManagedProxyServerConfig {
                url: "https://proxy.test".to_string(),
                id: "abc".to_string(),
            }),
        ),
        (
            "timeout",
            McpServerConfig::Stdio(McpStdioServerConfig {
                command: "uvx".to_string(),
                args: Vec::new(),
                env: BTreeMap::new(),
                tool_call_timeout_ms: Some(30_000),
            }),
        ),
    ];
    for (name, config) in &cases {
        assert_eq!(
            write_mcp_server(&path, name, config).expect("write"),
            McpEdit::Added,
            "{name} was not added"
        );
    }
    // `commit` already verified each one against the parser; read the whole
    // document once more so the LAST write is proved not to have disturbed the
    // first.
    let contents = std::fs::read_to_string(&path).expect("settings");
    let root = parse_json_object_contents(&path, &contents).expect("parse");
    let servers = root
        .get(mcp_keys::SERVERS)
        .and_then(JsonValue::as_object)
        .expect("mcpServers");
    assert_eq!(servers.len(), cases.len());
    for (name, config) in &cases {
        let parsed = parse_mcp_server_config(
            name,
            servers.get(*name).expect("server"),
            &mcp_server_context(&path, name),
        )
        .expect("parse server");
        assert_eq!(parsed, *config, "{name} came back different");
    }
}

#[test]
fn a_stdio_server_is_written_without_a_transport_tag_and_a_remote_one_with_it() {
    let dir = tempfile::tempdir().expect("temporary settings dir");
    let path = dir.path().join("settings.json");
    write_mcp_server(&path, "local", &stdio("npx", &[], &[])).expect("write stdio");
    write_mcp_server(
        &path,
        "remote",
        &McpServerConfig::Sse(McpRemoteServerConfig {
            url: "https://example.test/sse".to_string(),
            headers: BTreeMap::new(),
            headers_helper: None,
            oauth: None,
        }),
    )
    .expect("write sse");
    let contents = std::fs::read_to_string(&path).expect("settings");
    let root = parse_json_object_contents(&path, &contents).expect("parse");
    let servers = root
        .get(mcp_keys::SERVERS)
        .and_then(JsonValue::as_object)
        .expect("mcpServers");
    // The parser infers stdio from the absent `url`, so the tag would be noise.
    assert!(!servers["local"]
        .as_object()
        .expect("object")
        .contains_key(mcp_keys::TYPE));
    // It infers `http` from a `url`, so `sse` has to say so.
    assert_eq!(
        servers["remote"].as_object().expect("object")[mcp_keys::TYPE].as_str(),
        Some(mcp_keys::SSE)
    );
}

#[test]
fn an_edit_keeps_every_other_key_in_the_document() {
    let dir = tempfile::tempdir().expect("temporary settings dir");
    let path = dir.path().join("settings.json");
    std::fs::write(
        &path,
        r#"{"model":"fable","smart":{"autoClassifier":"off"},"mcpServers":{"kept":{"command":"kept"}}}"#,
    )
    .expect("seed settings");
    write_mcp_server(&path, "added", &http("https://example.test/mcp")).expect("write");
    let contents = std::fs::read_to_string(&path).expect("settings");
    let root = parse_json_object_contents(&path, &contents).expect("parse");
    assert_eq!(root["model"].as_str(), Some("fable"));
    assert_eq!(
        root["smart"].as_object().expect("smart")["autoClassifier"].as_str(),
        Some("off")
    );
    let servers = root[mcp_keys::SERVERS].as_object().expect("mcpServers");
    assert_eq!(servers.len(), 2);
    assert!(servers.contains_key("kept"));
}

#[test]
fn writing_the_same_definition_twice_reports_no_change_and_a_different_one_replaces() {
    let dir = tempfile::tempdir().expect("temporary settings dir");
    let path = dir.path().join("settings.json");
    let first = stdio("npx", &["one"], &[]);
    assert_eq!(
        write_mcp_server(&path, "server", &first).expect("write"),
        McpEdit::Added
    );
    assert_eq!(
        write_mcp_server(&path, "server", &first).expect("write"),
        McpEdit::Unchanged
    );
    assert_eq!(
        write_mcp_server(&path, "server", &stdio("npx", &["two"], &[])).expect("write"),
        McpEdit::Replaced
    );
}

#[test]
fn removing_a_name_nobody_wrote_is_absent_and_the_last_removal_takes_the_section() {
    let dir = tempfile::tempdir().expect("temporary settings dir");
    let path = dir.path().join("settings.json");
    assert_eq!(
        remove_mcp_server(&path, "ghost").expect("remove"),
        McpEdit::Absent
    );
    write_mcp_server(&path, "only", &stdio("npx", &[], &[])).expect("write");
    assert_eq!(
        remove_mcp_server(&path, "ghost").expect("remove"),
        McpEdit::Absent
    );
    assert_eq!(
        remove_mcp_server(&path, "only").expect("remove"),
        McpEdit::Removed
    );
    let contents = std::fs::read_to_string(&path).expect("settings");
    let root = parse_json_object_contents(&path, &contents).expect("parse");
    assert!(!root.contains_key(mcp_keys::SERVERS));
}

/// The round trip the whole unit stands on, at the scope a person picks: what
/// the writer put in `settings.json` is what `ConfigLoader` hands a session.
#[test]
fn what_the_writer_wrote_is_what_the_loader_loads_in_the_user_scope() {
    let home = tempfile::tempdir().expect("config home");
    let cwd = tempfile::tempdir().expect("workspace");
    let path = home.path().join("settings.json");
    let expected = stdio("npx", &["-y", "@modelcontextprotocol/server-everything"], &[
        ("EXAMPLE_TOKEN", "fixture-value"),
    ]);
    write_mcp_server(&path, "everything", &expected).expect("write");
    write_mcp_server(&path, "remote", &http("https://example.test/mcp")).expect("write");

    let config = load_at(&home, &cwd);
    let servers = config.mcp().servers();
    assert_eq!(servers["everything"].config, expected);
    assert_eq!(servers["everything"].scope, crate::ConfigSource::User);
    assert_eq!(servers["remote"].config, http("https://example.test/mcp"));

    remove_mcp_server(&path, "everything").expect("remove");
    let config = load_at(&home, &cwd);
    assert!(!config.mcp().servers().contains_key("everything"));
    assert!(config.mcp().servers().contains_key("remote"));
}

/// A project-scoped server is behind the supply-chain gate, and stays there
/// until an operator-authored document opts the project in. Both sides of that
/// gate are the writer's business: the name it wrote has to be findable either
/// way, or `zo mcp add --project` writes into a hole.
#[test]
fn a_project_scoped_server_is_gated_until_the_operator_opts_the_project_in() {
    let home = tempfile::tempdir().expect("config home");
    let cwd = tempfile::tempdir().expect("workspace");
    let path = cwd.path().join(".zo").join("settings.json");
    let expected = http("https://example.test/mcp");
    write_mcp_server(&path, "repo-server", &expected).expect("write");

    let config = load_at(&home, &cwd);
    assert!(!config.mcp().servers().contains_key("repo-server"));
    let untrusted = config.mcp().untrusted_project_servers();
    assert_eq!(untrusted.len(), 1);
    assert_eq!(untrusted[0].name, "repo-server");
    assert_eq!(untrusted[0].path, path);

    std::fs::write(
        home.path().join("settings.json"),
        r#"{"enableAllProjectMcpServers":true}"#,
    )
    .expect("opt in");
    let config = load_at(&home, &cwd);
    assert_eq!(config.mcp().servers()["repo-server"].config, expected);
    assert_eq!(
        config.mcp().servers()["repo-server"].scope,
        crate::ConfigSource::Project
    );
}

#[test]
fn a_document_that_is_not_an_object_is_refused_before_anything_is_written() {
    let dir = tempfile::tempdir().expect("temporary settings dir");
    let path = dir.path().join("settings.json");
    std::fs::write(&path, "[]").expect("seed settings");
    let error = write_mcp_server(&path, "server", &stdio("npx", &[], &[])).expect_err("refusal");
    assert!(
        error.to_string().contains("must be a JSON object"),
        "{error}"
    );
    assert_eq!(std::fs::read_to_string(&path).expect("settings"), "[]");
}

#[test]
fn an_mcp_servers_section_that_is_not_an_object_is_refused() {
    let dir = tempfile::tempdir().expect("temporary settings dir");
    let path = dir.path().join("settings.json");
    std::fs::write(&path, r#"{"mcpServers":[]}"#).expect("seed settings");
    let error = write_mcp_server(&path, "server", &stdio("npx", &[], &[])).expect_err("refusal");
    assert!(error.to_string().contains(mcp_keys::SERVERS), "{error}");
}

/// The file may hold an `env` block, so it is the owner's alone.
#[cfg(unix)]
#[test]
fn the_written_document_is_owner_only() {
    use std::os::unix::fs::PermissionsExt as _;

    let dir = tempfile::tempdir().expect("temporary settings dir");
    let path = dir.path().join("nested").join("settings.json");
    write_mcp_server(&path, "server", &stdio("npx", &[], &[("TOKEN", "t")])).expect("write");
    let mode = std::fs::metadata(&path).expect("metadata").permissions().mode();
    assert_eq!(mode & 0o777, 0o600, "mode was {:o}", mode & 0o777);
}

// --- the project's trust record ----------------------------------------

/// The other half of `zo mcp add --project --trust`: a name in the record is
/// consent for exactly that server. Its sibling in the same document stays
/// gated, and the writer lands on the file the loader reads rather than
/// joining a `.zo` of its own.
#[test]
fn a_trusted_name_un_gates_exactly_that_project_server() {
    let home = tempfile::tempdir().expect("config home");
    let cwd = tempfile::tempdir().expect("workspace");
    let settings = cwd.path().join(".zo").join("settings.json");
    let expected = http("https://example.test/mcp");
    write_mcp_server(&settings, "consented", &expected).expect("write");
    write_mcp_server(&settings, "sibling", &stdio("npx", &[], &[])).expect("write");

    let record = ConfigLoader::new(cwd.path(), home.path()).trusted_mcp_servers_path();
    assert_eq!(
        trust_mcp_server(&record, "consented").expect("trust"),
        McpEdit::Added
    );

    let config = load_at(&home, &cwd);
    assert_eq!(config.mcp().servers()["consented"].config, expected);
    assert!(!config.mcp().servers().contains_key("sibling"));
    let untrusted = config.mcp().untrusted_project_servers();
    assert_eq!(untrusted.len(), 1);
    assert_eq!(untrusted[0].name, "sibling");
}

/// Consent is a set of names. The record is created where nothing stood, the
/// names a person put there stay in the order they wrote them, and recording
/// one twice is not a second entry.
#[test]
fn trusting_a_name_twice_is_unchanged_and_the_names_already_there_survive() {
    let dir = tempfile::tempdir().expect("temporary project dir");
    let record = dir
        .path()
        .join(".zo")
        .join(mcp_keys::TRUSTED_SERVERS_FILE);
    assert_eq!(
        trust_mcp_server(&record, "first").expect("trust"),
        McpEdit::Added,
        "the record is written even where no .zo directory stood"
    );
    std::fs::write(&record, "[\"first\",\"handwritten\"]").expect("a person edits the record");
    assert_eq!(
        trust_mcp_server(&record, "second").expect("trust"),
        McpEdit::Added
    );
    assert_eq!(
        trust_mcp_server(&record, "second").expect("trust"),
        McpEdit::Unchanged
    );
    let names = parse_trusted_mcp_server_names(
        &record,
        &std::fs::read_to_string(&record).expect("trust record"),
    )
    .expect("parse");
    assert_eq!(names, ["first", "handwritten", "second"]);
}

#[test]
fn a_trust_record_that_is_not_an_array_of_names_is_refused_before_anything_is_written() {
    let dir = tempfile::tempdir().expect("temporary project dir");
    let record = dir.path().join(mcp_keys::TRUSTED_SERVERS_FILE);
    for (seeded, complaint) in [
        (r#"{"mcpServers":{}}"#, "JSON array"),
        ("[1]", "must be strings"),
    ] {
        std::fs::write(&record, seeded).expect("seed record");
        let error = trust_mcp_server(&record, "name").expect_err("refusal");
        assert!(error.to_string().contains(complaint), "{error}");
        assert_eq!(
            std::fs::read_to_string(&record).expect("trust record"),
            seeded
        );
    }
}

/// A name another local account could append is a server this one would run.
#[cfg(unix)]
#[test]
fn the_trust_record_is_owner_only() {
    use std::os::unix::fs::PermissionsExt as _;

    let dir = tempfile::tempdir().expect("temporary project dir");
    let record = dir
        .path()
        .join(".zo")
        .join(mcp_keys::TRUSTED_SERVERS_FILE);
    trust_mcp_server(&record, "consented").expect("trust");
    let mode = std::fs::metadata(&record)
        .expect("metadata")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600, "mode was {:o}", mode & 0o777);
}
