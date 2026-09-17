use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::manifest_io::{MANIFEST_FILE_NAME, MANIFEST_RELATIVE_PATH};
use super::*;

fn temp_dir(label: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time should be after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("plugins-{label}-{nanos}"))
}

fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent dir");
    }
    fs::write(path, contents).expect("write file");
}

fn write_loader_plugin(root: &Path) {
    write_file(
        root.join("hooks").join("pre.sh").as_path(),
        "#!/bin/sh\nprintf 'pre'\n",
    );
    write_file(
        root.join("tools").join("echo-tool.sh").as_path(),
        "#!/bin/sh\ncat\n",
    );
    write_file(
        root.join("commands").join("sync.sh").as_path(),
        "#!/bin/sh\nprintf 'sync'\n",
    );
    write_file(
        root.join(MANIFEST_FILE_NAME).as_path(),
        r#"{
  "name": "loader-demo",
  "version": "1.2.3",
  "description": "Manifest loader test plugin",
  "permissions": ["read", "write"],
  "hooks": {
"PreToolUse": ["./hooks/pre.sh"]
  },
  "tools": [
{
  "name": "echo_tool",
  "description": "Echoes JSON input",
  "inputSchema": {
    "type": "object"
  },
  "command": "./tools/echo-tool.sh",
  "requiredPermission": "workspace-write"
}
  ],
  "commands": [
{
  "name": "sync",
  "description": "Sync command",
  "command": "./commands/sync.sh"
}
  ]
}"#,
    );
}

fn write_external_plugin(root: &Path, name: &str, version: &str) {
    write_file(
        root.join("hooks").join("pre.sh").as_path(),
        "#!/bin/sh\nprintf 'pre'\n",
    );
    write_file(
        root.join("hooks").join("post.sh").as_path(),
        "#!/bin/sh\nprintf 'post'\n",
    );
    write_file(
        root.join(MANIFEST_RELATIVE_PATH).as_path(),
        format!(
            "{{\n  \"name\": \"{name}\",\n  \"version\": \"{version}\",\n  \"description\": \"test plugin\",\n  \"hooks\": {{\n    \"PreToolUse\": [\"./hooks/pre.sh\"],\n    \"PostToolUse\": [\"./hooks/post.sh\"]\n  }}\n}}"
        )
        .as_str(),
    );
}

fn write_broken_plugin(root: &Path, name: &str) {
    write_file(
        root.join(MANIFEST_RELATIVE_PATH).as_path(),
        format!(
            "{{\n  \"name\": \"{name}\",\n  \"version\": \"1.0.0\",\n  \"description\": \"broken plugin\",\n  \"hooks\": {{\n    \"PreToolUse\": [\"./hooks/missing.sh\"]\n  }}\n}}"
        )
        .as_str(),
    );
}

fn write_directory_path_plugin(root: &Path, name: &str) {
    fs::create_dir_all(root.join("hooks").join("pre-dir")).expect("hook dir");
    fs::create_dir_all(root.join("tools").join("tool-dir")).expect("tool dir");
    fs::create_dir_all(root.join("commands").join("sync-dir")).expect("command dir");
    fs::create_dir_all(root.join("lifecycle").join("init-dir")).expect("lifecycle dir");
    write_file(
        root.join(MANIFEST_FILE_NAME).as_path(),
        format!(
            "{{\n  \"name\": \"{name}\",\n  \"version\": \"1.0.0\",\n  \"description\": \"directory path plugin\",\n  \"hooks\": {{\n    \"PreToolUse\": [\"./hooks/pre-dir\"]\n  }},\n  \"lifecycle\": {{\n    \"Init\": [\"./lifecycle/init-dir\"]\n  }},\n  \"tools\": [\n    {{\n      \"name\": \"dir_tool\",\n      \"description\": \"Directory tool\",\n      \"inputSchema\": {{\"type\": \"object\"}},\n      \"command\": \"./tools/tool-dir\"\n    }}\n  ],\n  \"commands\": [\n    {{\n      \"name\": \"sync\",\n      \"description\": \"Directory command\",\n      \"command\": \"./commands/sync-dir\"\n    }}\n  ]\n}}"
        )
        .as_str(),
    );
}

fn write_literal_command_plugin(root: &Path, name: &str) {
    write_file(
        root.join(MANIFEST_FILE_NAME).as_path(),
        format!(
            "{{\n  \"name\": \"{name}\",\n  \"version\": \"1.0.0\",\n  \"description\": \"literal command plugin\",\n  \"hooks\": {{\n    \"PreToolUse\": [\"echo pre\"]\n  }},\n  \"lifecycle\": {{\n    \"Init\": [\"echo init\"]\n  }},\n  \"tools\": [\n    {{\n      \"name\": \"literal_tool\",\n      \"description\": \"Literal tool\",\n      \"inputSchema\": {{\"type\": \"object\"}},\n      \"command\": \"echo tool\"\n    }}\n  ],\n  \"commands\": [\n    {{\n      \"name\": \"sync\",\n      \"description\": \"Literal command\",\n      \"command\": \"echo slash\"\n    }}\n  ]\n}}"
        )
        .as_str(),
    );
}

fn write_broken_failure_hook_plugin(root: &Path, name: &str) {
    write_file(
        root.join(MANIFEST_RELATIVE_PATH).as_path(),
        format!(
            "{{\n  \"name\": \"{name}\",\n  \"version\": \"1.0.0\",\n  \"description\": \"broken plugin\",\n  \"hooks\": {{\n    \"PostToolUseFailure\": [\"./hooks/missing-failure.sh\"]\n  }}\n}}"
        )
        .as_str(),
    );
}

fn write_lifecycle_plugin(root: &Path, name: &str, version: &str) -> PathBuf {
    let log_path = root.join("lifecycle.log");
    write_file(
        root.join("lifecycle").join("init.sh").as_path(),
        "#!/bin/sh\nprintf 'init\\n' >> lifecycle.log\n",
    );
    write_file(
        root.join("lifecycle").join("shutdown.sh").as_path(),
        "#!/bin/sh\nprintf 'shutdown\\n' >> lifecycle.log\n",
    );
    write_file(
        root.join(MANIFEST_RELATIVE_PATH).as_path(),
        format!(
            "{{\n  \"name\": \"{name}\",\n  \"version\": \"{version}\",\n  \"description\": \"lifecycle plugin\",\n  \"lifecycle\": {{\n    \"Init\": [\"./lifecycle/init.sh\"],\n    \"Shutdown\": [\"./lifecycle/shutdown.sh\"]\n  }}\n}}"
        )
        .as_str(),
    );
    log_path
}

fn write_tool_plugin(root: &Path, name: &str, version: &str) {
    write_tool_plugin_with_name(root, name, version, "plugin_echo");
}

fn write_tool_plugin_with_name(root: &Path, name: &str, version: &str, tool_name: &str) {
    let script_path = root.join("tools").join("echo-json.sh");
    write_file(
        &script_path,
        "#!/bin/sh\nINPUT=$(cat)\nprintf '{\"plugin\":\"%s\",\"tool\":\"%s\",\"input\":%s}\\n' \"$ZO_PLUGIN_ID\" \"$ZO_TOOL_NAME\" \"$INPUT\"\n",
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = fs::metadata(&script_path).expect("metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script_path, permissions).expect("chmod");
    }
    write_file(
        root.join(MANIFEST_RELATIVE_PATH).as_path(),
        format!(
            "{{\n  \"name\": \"{name}\",\n  \"version\": \"{version}\",\n  \"description\": \"tool plugin\",\n  \"tools\": [\n    {{\n      \"name\": \"{tool_name}\",\n      \"description\": \"Echo JSON input\",\n      \"inputSchema\": {{\"type\": \"object\", \"properties\": {{\"message\": {{\"type\": \"string\"}}}}, \"required\": [\"message\"], \"additionalProperties\": false}},\n      \"command\": \"./tools/echo-json.sh\",\n      \"requiredPermission\": \"workspace-write\"\n    }}\n  ]\n}}"
        )
        .as_str(),
    );
}

fn write_bundled_plugin(root: &Path, name: &str, version: &str, default_enabled: bool) {
    write_file(
        root.join(MANIFEST_RELATIVE_PATH).as_path(),
        format!(
            "{{\n  \"name\": \"{name}\",\n  \"version\": \"{version}\",\n  \"description\": \"bundled plugin\",\n  \"defaultEnabled\": {}\n}}",
            if default_enabled { "true" } else { "false" }
        )
        .as_str(),
    );
}

fn load_enabled_plugins(path: &Path) -> BTreeMap<String, bool> {
    let contents = fs::read_to_string(path).expect("settings should exist");
    let root: Value = serde_json::from_str(&contents).expect("settings json");
    root.get("enabledPlugins")
        .and_then(Value::as_object)
        .map(|enabled_plugins| {
            enabled_plugins
                .iter()
                .map(|(plugin_id, value)| {
                    (
                        plugin_id.clone(),
                        value.as_bool().expect("plugin state should be a bool"),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn load_plugin_from_directory_validates_required_fields() {
    let root = temp_dir("manifest-required");
    write_file(
        root.join(MANIFEST_FILE_NAME).as_path(),
        r#"{"name":"","version":"1.0.0","description":"desc"}"#,
    );

    let error = load_plugin_from_directory(&root).expect_err("empty name should fail");
    assert!(error.to_string().contains("name cannot be empty"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn load_plugin_from_directory_reads_root_manifest_and_validates_entries() {
    let root = temp_dir("manifest-root");
    write_loader_plugin(&root);

    let manifest = load_plugin_from_directory(&root).expect("manifest should load");
    assert_eq!(manifest.name, "loader-demo");
    assert_eq!(manifest.version, "1.2.3");
    assert_eq!(
        manifest
            .permissions
            .iter()
            .map(|permission| permission.as_str())
            .collect::<Vec<_>>(),
        vec!["read", "write"]
    );
    assert_eq!(manifest.hooks.pre_tool_use, vec!["./hooks/pre.sh"]);
    assert_eq!(manifest.tools.len(), 1);
    assert_eq!(manifest.tools[0].name, "echo_tool");
    assert_eq!(
        manifest.tools[0].required_permission,
        PluginToolPermission::WorkspaceWrite
    );
    assert_eq!(manifest.commands.len(), 1);
    assert_eq!(manifest.commands[0].name, "sync");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn load_plugin_from_directory_supports_packaged_manifest_path() {
    let root = temp_dir("manifest-packaged");
    write_external_plugin(&root, "packaged-demo", "1.0.0");

    let manifest = load_plugin_from_directory(&root).expect("packaged manifest should load");
    assert_eq!(manifest.name, "packaged-demo");
    assert!(manifest.tools.is_empty());
    assert!(manifest.commands.is_empty());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn load_plugin_from_directory_defaults_optional_fields() {
    let root = temp_dir("manifest-defaults");
    write_file(
        root.join(MANIFEST_FILE_NAME).as_path(),
        r#"{
  "name": "minimal",
  "version": "0.1.0",
  "description": "Minimal manifest"
}"#,
    );

    let manifest = load_plugin_from_directory(&root).expect("minimal manifest should load");
    assert!(manifest.permissions.is_empty());
    assert!(manifest.hooks.is_empty());
    assert!(manifest.tools.is_empty());
    assert!(manifest.commands.is_empty());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn load_plugin_from_directory_rejects_duplicate_permissions_and_commands() {
    let root = temp_dir("manifest-duplicates");
    write_file(
        root.join("commands").join("sync.sh").as_path(),
        "#!/bin/sh\nprintf 'sync'\n",
    );
    write_file(
        root.join(MANIFEST_FILE_NAME).as_path(),
        r#"{
  "name": "duplicate-manifest",
  "version": "1.0.0",
  "description": "Duplicate validation",
  "permissions": ["read", "read"],
  "commands": [
{"name": "sync", "description": "Sync one", "command": "./commands/sync.sh"},
{"name": "sync", "description": "Sync two", "command": "./commands/sync.sh"}
  ]
}"#,
    );

    let error = load_plugin_from_directory(&root).expect_err("duplicates should fail");
    match error {
        PluginError::ManifestValidation(errors) => {
            assert!(errors.iter().any(|error| matches!(
                error,
                PluginManifestValidationError::DuplicatePermission { permission }
                if permission == "read"
            )));
            assert!(errors.iter().any(|error| matches!(
                error,
                PluginManifestValidationError::DuplicateEntry { kind, name }
                if *kind == "command" && name == "sync"
            )));
        }
        other => panic!("expected manifest validation errors, got {other}"),
    }

    let _ = fs::remove_dir_all(root);
}

#[test]
fn load_plugin_from_directory_rejects_missing_tool_or_command_paths() {
    let root = temp_dir("manifest-paths");
    write_file(
        root.join(MANIFEST_FILE_NAME).as_path(),
        r#"{
  "name": "missing-paths",
  "version": "1.0.0",
  "description": "Missing path validation",
  "tools": [
{
  "name": "tool_one",
  "description": "Missing tool script",
  "inputSchema": {"type": "object"},
  "command": "./tools/missing.sh"
}
  ]
}"#,
    );

    let error = load_plugin_from_directory(&root).expect_err("missing paths should fail");
    assert!(error.to_string().contains("does not exist"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn load_plugin_from_directory_rejects_missing_lifecycle_paths() {
    // given
    let root = temp_dir("manifest-lifecycle-paths");
    write_file(
        root.join(MANIFEST_FILE_NAME).as_path(),
        r#"{
  "name": "missing-lifecycle-paths",
  "version": "1.0.0",
  "description": "Missing lifecycle path validation",
  "lifecycle": {
"Init": ["./lifecycle/init.sh"],
"Shutdown": ["./lifecycle/shutdown.sh"]
  }
}"#,
    );

    // when
    let error = load_plugin_from_directory(&root).expect_err("missing lifecycle paths should fail");

    // then
    match error {
        PluginError::ManifestValidation(errors) => {
            assert!(errors.iter().any(|error| matches!(
                error,
                PluginManifestValidationError::MissingPath { kind, path }
                if *kind == "lifecycle command"
                    && path.ends_with(Path::new("lifecycle/init.sh"))
            )));
            assert!(errors.iter().any(|error| matches!(
                error,
                PluginManifestValidationError::MissingPath { kind, path }
                if *kind == "lifecycle command"
                    && path.ends_with(Path::new("lifecycle/shutdown.sh"))
            )));
        }
        other => panic!("expected manifest validation errors, got {other}"),
    }

    let _ = fs::remove_dir_all(root);
}

#[test]
fn load_plugin_from_directory_rejects_directory_command_paths() {
    // given
    let root = temp_dir("manifest-directory-paths");
    write_directory_path_plugin(&root, "directory-paths");

    // when
    let error = load_plugin_from_directory(&root).expect_err("directory command paths should fail");

    // then
    match error {
        PluginError::ManifestValidation(errors) => {
            assert!(errors.iter().any(|error| matches!(
                error,
                PluginManifestValidationError::PathIsDirectory { kind, path }
                if *kind == "hook" && path.ends_with(Path::new("hooks/pre-dir"))
            )));
            assert!(errors.iter().any(|error| matches!(
                error,
                PluginManifestValidationError::PathIsDirectory { kind, path }
                if *kind == "lifecycle command"
                    && path.ends_with(Path::new("lifecycle/init-dir"))
            )));
            assert!(errors.iter().any(|error| matches!(
                error,
                PluginManifestValidationError::PathIsDirectory { kind, path }
                if *kind == "tool" && path.ends_with(Path::new("tools/tool-dir"))
            )));
            assert!(errors.iter().any(|error| matches!(
                error,
                PluginManifestValidationError::PathIsDirectory { kind, path }
                if *kind == "command" && path.ends_with(Path::new("commands/sync-dir"))
            )));
        }
        other => panic!("expected manifest validation errors, got {other}"),
    }

    let _ = fs::remove_dir_all(root);
}

#[test]
fn load_plugin_from_directory_rejects_literal_shell_commands() {
    // given
    let root = temp_dir("manifest-literal-commands");
    write_literal_command_plugin(&root, "literal-commands");

    // when
    let error = load_plugin_from_directory(&root).expect_err("literal commands should fail");

    // then
    match error {
        PluginError::ManifestValidation(errors) => {
            assert!(errors.iter().any(|error| matches!(
                error,
                PluginManifestValidationError::ShellCommandNotAllowed { kind, command }
                if *kind == "hook" && command == "echo pre"
            )));
            assert!(errors.iter().any(|error| matches!(
                error,
                PluginManifestValidationError::ShellCommandNotAllowed { kind, command }
                if *kind == "lifecycle command" && command == "echo init"
            )));
            assert!(errors.iter().any(|error| matches!(
                error,
                PluginManifestValidationError::ShellCommandNotAllowed { kind, command }
                if *kind == "tool" && command == "echo tool"
            )));
            assert!(errors.iter().any(|error| matches!(
                error,
                PluginManifestValidationError::ShellCommandNotAllowed { kind, command }
                if *kind == "command" && command == "echo slash"
            )));
        }
        other => panic!("expected manifest validation errors, got {other}"),
    }

    let _ = fs::remove_dir_all(root);
}

#[test]
fn load_plugin_from_directory_rejects_invalid_permissions() {
    let root = temp_dir("manifest-invalid-permissions");
    write_file(
        root.join(MANIFEST_FILE_NAME).as_path(),
        r#"{
  "name": "invalid-permissions",
  "version": "1.0.0",
  "description": "Invalid permission validation",
  "permissions": ["admin"]
}"#,
    );

    let error = load_plugin_from_directory(&root).expect_err("invalid permissions should fail");
    match error {
        PluginError::ManifestValidation(errors) => {
            assert!(errors.iter().any(|error| matches!(
                error,
                PluginManifestValidationError::InvalidPermission { permission }
                if permission == "admin"
            )));
        }
        other => panic!("expected manifest validation errors, got {other}"),
    }

    let _ = fs::remove_dir_all(root);
}

#[test]
fn load_plugin_from_directory_rejects_invalid_tool_required_permission() {
    let root = temp_dir("manifest-invalid-tool-permission");
    write_file(
        root.join("tools").join("echo.sh").as_path(),
        "#!/bin/sh\ncat\n",
    );
    write_file(
        root.join(MANIFEST_FILE_NAME).as_path(),
        r#"{
  "name": "invalid-tool-permission",
  "version": "1.0.0",
  "description": "Invalid tool permission validation",
  "tools": [
{
  "name": "echo_tool",
  "description": "Echo tool",
  "inputSchema": {"type": "object"},
  "command": "./tools/echo.sh",
  "requiredPermission": "admin"
}
  ]
}"#,
    );

    let error = load_plugin_from_directory(&root).expect_err("invalid tool permission should fail");
    match error {
        PluginError::ManifestValidation(errors) => {
            assert!(errors.iter().any(|error| matches!(
                error,
                PluginManifestValidationError::InvalidToolRequiredPermission {
                    tool_name,
                    permission
                } if tool_name == "echo_tool" && permission == "admin"
            )));
        }
        other => panic!("expected manifest validation errors, got {other}"),
    }

    let _ = fs::remove_dir_all(root);
}

#[test]
fn load_plugin_from_directory_accumulates_multiple_validation_errors() {
    let root = temp_dir("manifest-multi-error");
    write_file(
        root.join(MANIFEST_FILE_NAME).as_path(),
        r#"{
  "name": "",
  "version": "1.0.0",
  "description": "",
  "permissions": ["admin"],
  "commands": [
{"name": "", "description": "", "command": "./commands/missing.sh"}
  ]
}"#,
    );

    let error =
        load_plugin_from_directory(&root).expect_err("multiple manifest errors should fail");
    match error {
        PluginError::ManifestValidation(errors) => {
            assert!(errors.len() >= 4);
            assert!(errors.iter().any(|error| matches!(
                error,
                PluginManifestValidationError::EmptyField { field } if *field == "name"
            )));
            assert!(errors.iter().any(|error| matches!(
                error,
                PluginManifestValidationError::EmptyField { field }
                if *field == "description"
            )));
            assert!(errors.iter().any(|error| matches!(
                error,
                PluginManifestValidationError::InvalidPermission { permission }
                if permission == "admin"
            )));
        }
        other => panic!("expected manifest validation errors, got {other}"),
    }

    let _ = fs::remove_dir_all(root);
}

#[test]
fn discovers_builtin_and_bundled_plugins() {
    let manager = PluginManager::new(PluginManagerConfig::new(temp_dir("discover")));
    let plugins = manager.list_plugins().expect("plugins should list");
    assert!(plugins
        .iter()
        .any(|plugin| plugin.metadata.kind == PluginKind::Builtin));
    assert!(plugins
        .iter()
        .any(|plugin| plugin.metadata.kind == PluginKind::Bundled));
}

#[test]
fn installs_enables_updates_and_uninstalls_external_plugins() {
    let config_home = temp_dir("home");
    let source_root = temp_dir("source");
    write_external_plugin(&source_root, "demo", "1.0.0");

    let mut manager = PluginManager::new(PluginManagerConfig::new(&config_home));
    let install = manager
        .install(source_root.to_str().expect("utf8 path"))
        .expect("install should succeed");
    assert_eq!(install.plugin_id, "demo@external");
    assert!(manager
        .list_plugins()
        .expect("list plugins")
        .iter()
        .any(|plugin| plugin.metadata.id == "demo@external" && plugin.enabled));

    let hooks = manager.aggregated_hooks().expect("hooks should aggregate");
    assert_eq!(hooks.pre_tool_use.len(), 1);
    assert!(hooks.pre_tool_use[0].contains("pre.sh"));

    manager
        .disable("demo@external")
        .expect("disable should work");
    assert!(manager
        .aggregated_hooks()
        .expect("hooks after disable")
        .is_empty());
    manager.enable("demo@external").expect("enable should work");

    write_external_plugin(&source_root, "demo", "2.0.0");
    let update = manager.update("demo@external").expect("update should work");
    assert_eq!(update.old_version, "1.0.0");
    assert_eq!(update.new_version, "2.0.0");

    manager
        .uninstall("demo@external")
        .expect("uninstall should work");
    assert!(!manager
        .list_plugins()
        .expect("list plugins")
        .iter()
        .any(|plugin| plugin.metadata.id == "demo@external"));

    let _ = fs::remove_dir_all(config_home);
    let _ = fs::remove_dir_all(source_root);
}

#[test]
fn install_records_checksum_and_rejects_on_disk_tampering() {
    let config_home = temp_dir("checksum-home");
    let source_root = temp_dir("checksum-source");
    write_external_plugin(&source_root, "guard", "1.0.0");

    let mut manager = PluginManager::new(PluginManagerConfig::new(&config_home));
    let install = manager
        .install(source_root.to_str().expect("utf8 path"))
        .expect("install should succeed");

    // Install records an integrity digest over the materialised copy.
    let registry = manager.load_registry().expect("load registry");
    let record = registry
        .plugins
        .get("guard@external")
        .expect("installed record present");
    assert!(
        record.content_sha256.is_some(),
        "install should record a content digest"
    );

    // A healthy installed tree loads normally.
    assert!(manager
        .list_installed_plugins()
        .expect("list installed")
        .iter()
        .any(|plugin| plugin.metadata.id == "guard@external"));

    // Tamper with the installed tree, then load through a fresh (uncached)
    // manager. The digest mismatch surfaces as a load failure rather than a
    // silently-trusted plugin.
    write_file(
        install.install_path.join("injected.sh").as_path(),
        "#!/bin/sh\necho pwned\n",
    );
    let fresh = PluginManager::new(PluginManagerConfig::new(&config_home));
    let report = fresh
        .installed_plugin_registry_report()
        .expect("report tolerates a tampered plugin");
    assert!(
        !report.registry().contains("guard@external"),
        "a tampered plugin must not load",
    );
    assert!(
        report
            .failures()
            .iter()
            .any(|failure| matches!(failure.error(), PluginError::IntegrityMismatch(_))),
        "the tamper must surface as an integrity failure",
    );

    let _ = fs::remove_dir_all(config_home);
    let _ = fs::remove_dir_all(source_root);
}

#[cfg(unix)]
#[test]
fn plugin_registry_runs_contributed_slash_commands() {
    use std::os::unix::fs::PermissionsExt;

    let root = temp_dir("slash-plugin");
    let script = root.join("commands").join("greet.sh");
    write_file(
        &script,
        "#!/bin/sh\nprintf 'greet:%s' \"$ZO_SLASH_ARGS\"\n",
    );
    let mut perms = fs::metadata(&script)
        .expect("script metadata")
        .permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script, perms).expect("chmod script");
    write_file(
        root.join(MANIFEST_FILE_NAME).as_path(),
        r#"{
  "name": "greeter",
  "version": "1.0.0",
  "description": "Greeter plugin",
  "commands": [
    { "name": "greet", "description": "Greet someone", "command": "./commands/greet.sh" }
  ]
}"#,
    );

    let definition = super::builtin::load_plugin_definition(
        &root,
        PluginKind::External,
        "test".to_string(),
        "external",
    )
    .expect("plugin loads");
    let registry = PluginRegistry::new(vec![RegisteredPlugin::new(definition, true)]);

    // The command is discoverable (leading `/` optional) and listed in specs.
    assert!(registry.find_slash_command("greet").is_some());
    assert!(registry.find_slash_command("/greet").is_some());
    assert!(registry
        .slash_command_specs()
        .iter()
        .any(|(_, name, _)| name == "greet"));

    // Execution runs the script with ZO_SLASH_ARGS and returns its stdout.
    let output = registry.run_slash_command("greet", "world").expect("runs");
    assert_eq!(output, "greet:world");

    // An unknown command is a NotFound error, not a silent success.
    assert!(matches!(
        registry.run_slash_command("nope", ""),
        Err(PluginError::NotFound(_))
    ));

    let _ = fs::remove_dir_all(&root);
}

#[cfg(unix)]
#[test]
fn disabled_plugins_do_not_contribute_slash_commands() {
    let root = temp_dir("slash-disabled");
    write_file(
        root.join("commands").join("noop.sh").as_path(),
        "#!/bin/sh\ntrue\n",
    );
    write_file(
        root.join(MANIFEST_FILE_NAME).as_path(),
        r#"{
  "name": "disabled-greeter",
  "version": "1.0.0",
  "description": "Disabled plugin",
  "commands": [
    { "name": "greet", "description": "Greet", "command": "./commands/noop.sh" }
  ]
}"#,
    );

    let definition = super::builtin::load_plugin_definition(
        &root,
        PluginKind::External,
        "test".to_string(),
        "external",
    )
    .expect("plugin loads");
    // Registered but disabled.
    let registry = PluginRegistry::new(vec![RegisteredPlugin::new(definition, false)]);

    assert!(registry.find_slash_command("greet").is_none());
    assert!(registry.slash_command_specs().is_empty());

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn auto_installs_bundled_plugins_into_the_registry() {
    let config_home = temp_dir("bundled-home");
    let bundled_root = temp_dir("bundled-root");
    write_bundled_plugin(&bundled_root.join("starter"), "starter", "0.1.0", false);

    let mut config = PluginManagerConfig::new(&config_home);
    config.bundled_root = Some(bundled_root.clone());
    let manager = PluginManager::new(config);

    let installed = manager
        .list_installed_plugins()
        .expect("bundled plugins should auto-install");
    assert!(installed.iter().any(|plugin| {
        plugin.metadata.id == "starter@bundled"
            && plugin.metadata.kind == PluginKind::Bundled
            && !plugin.enabled
    }));

    let registry = manager.load_registry().expect("registry should exist");
    let record = registry
        .plugins
        .get("starter@bundled")
        .expect("bundled plugin should be recorded");
    assert_eq!(record.kind, PluginKind::Bundled);
    assert!(record.install_path.exists());

    let _ = fs::remove_dir_all(config_home);
    let _ = fs::remove_dir_all(bundled_root);
}

#[test]
fn install_invalidates_cached_plugin_registry() {
    let config_home = temp_dir("install-cache-home");
    let source_root = temp_dir("install-cache-source");
    write_external_plugin(&source_root, "cache-demo", "1.0.0");

    let mut manager = PluginManager::new(PluginManagerConfig::new(&config_home));
    let baseline = manager.list_plugins().expect("baseline plugin list");
    assert!(!baseline
        .iter()
        .any(|plugin| plugin.metadata.id == "cache-demo@external"));

    manager
        .install(source_root.to_str().expect("utf8 path"))
        .expect("install should succeed");

    let refreshed = manager.list_plugins().expect("refreshed plugin list");
    assert!(refreshed
        .iter()
        .any(|plugin| plugin.metadata.id == "cache-demo@external" && plugin.enabled));

    let _ = fs::remove_dir_all(config_home);
    let _ = fs::remove_dir_all(source_root);
}

#[test]
fn install_invalidates_cache_primed_by_registry_report() {
    let config_home = temp_dir("report-cache-home");
    let source_root = temp_dir("report-cache-source");
    write_external_plugin(&source_root, "cache-report-demo", "1.0.0");

    let mut manager = PluginManager::new(PluginManagerConfig::new(&config_home));
    let baseline = manager
        .plugin_registry_report()
        .expect("baseline registry report");
    assert!(!baseline
        .registry()
        .summaries()
        .iter()
        .any(|plugin| plugin.metadata.id == "cache-report-demo@external"));

    manager
        .install(source_root.to_str().expect("utf8 path"))
        .expect("install should succeed");

    let refreshed = manager
        .plugin_registry_report()
        .expect("refreshed registry report");
    assert!(refreshed
        .registry()
        .summaries()
        .iter()
        .any(|plugin| plugin.metadata.id == "cache-report-demo@external" && plugin.enabled));

    let _ = fs::remove_dir_all(config_home);
    let _ = fs::remove_dir_all(source_root);
}

#[test]
fn default_bundled_root_loads_repo_bundles_as_installed_plugins() {
    let config_home = temp_dir("default-bundled-home");
    let manager = PluginManager::new(PluginManagerConfig::new(&config_home));

    let installed = manager
        .list_installed_plugins()
        .expect("default bundled plugins should auto-install");
    assert!(installed
        .iter()
        .any(|plugin| plugin.metadata.id == "example-bundled@bundled"));
    assert!(installed
        .iter()
        .any(|plugin| plugin.metadata.id == "sample-hooks@bundled"));

    let _ = fs::remove_dir_all(config_home);
}

#[test]
fn bundled_sync_prunes_removed_bundled_registry_entries() {
    let config_home = temp_dir("bundled-prune-home");
    let bundled_root = temp_dir("bundled-prune-root");
    let stale_install_path = config_home
        .join("plugins")
        .join("installed")
        .join("stale-bundled-external");
    write_bundled_plugin(&bundled_root.join("active"), "active", "0.1.0", false);
    write_file(
        stale_install_path.join(MANIFEST_RELATIVE_PATH).as_path(),
        r#"{
  "name": "stale",
  "version": "0.1.0",
  "description": "stale bundled plugin"
}"#,
    );

    let mut config = PluginManagerConfig::new(&config_home);
    config.bundled_root = Some(bundled_root.clone());
    config.install_root = Some(config_home.join("plugins").join("installed"));
    let manager = PluginManager::new(config);

    let mut registry = InstalledPluginRegistry::default();
    registry.plugins.insert(
        "stale@bundled".to_string(),
        InstalledPluginRecord {
            kind: PluginKind::Bundled,
            id: "stale@bundled".to_string(),
            name: "stale".to_string(),
            version: "0.1.0".to_string(),
            description: "stale bundled plugin".to_string(),
            install_path: stale_install_path.clone(),
            source: PluginInstallSource::LocalPath {
                path: bundled_root.join("stale"),
            },
            installed_at_unix_ms: 1,
            updated_at_unix_ms: 1,
            resolved_commit: None,
            content_sha256: None,
        },
    );
    manager.store_registry(&registry).expect("store registry");
    manager
        .write_enabled_state("stale@bundled", Some(true))
        .expect("seed bundled enabled state");

    let installed = manager
        .list_installed_plugins()
        .expect("bundled sync should succeed");
    assert!(installed
        .iter()
        .any(|plugin| plugin.metadata.id == "active@bundled"));
    assert!(!installed
        .iter()
        .any(|plugin| plugin.metadata.id == "stale@bundled"));

    let registry = manager.load_registry().expect("load registry");
    assert!(!registry.plugins.contains_key("stale@bundled"));
    assert!(!stale_install_path.exists());

    let _ = fs::remove_dir_all(config_home);
    let _ = fs::remove_dir_all(bundled_root);
}

#[test]
fn installed_plugin_discovery_rejects_registry_entries_outside_install_root() {
    let config_home = temp_dir("registry-fallback-home");
    let bundled_root = temp_dir("registry-fallback-bundled");
    let install_root = config_home.join("plugins").join("installed");
    let external_install_path = temp_dir("registry-fallback-external");
    write_file(
        external_install_path.join(MANIFEST_FILE_NAME).as_path(),
        r#"{
  "name": "registry-fallback",
  "version": "1.0.0",
  "description": "Registry fallback plugin"
}"#,
    );
    fs::create_dir_all(&install_root).expect("install root");

    let mut config = PluginManagerConfig::new(&config_home);
    config.bundled_root = Some(bundled_root.clone());
    config.install_root = Some(install_root.clone());
    let manager = PluginManager::new(config);

    let mut registry = InstalledPluginRegistry::default();
    registry.plugins.insert(
        "registry-fallback@external".to_string(),
        InstalledPluginRecord {
            kind: PluginKind::External,
            id: "registry-fallback@external".to_string(),
            name: "registry-fallback".to_string(),
            version: "1.0.0".to_string(),
            description: "Registry fallback plugin".to_string(),
            install_path: external_install_path.clone(),
            source: PluginInstallSource::LocalPath {
                path: external_install_path.clone(),
            },
            installed_at_unix_ms: 1,
            updated_at_unix_ms: 1,
            resolved_commit: None,
            content_sha256: None,
        },
    );
    manager.store_registry(&registry).expect("store registry");
    manager
        .write_enabled_state("stale-external@external", Some(true))
        .expect("seed stale external enabled state");

    let report = manager
        .installed_plugin_registry_report()
        .expect("outside registry entry should be reported, not loaded");
    assert!(!report.registry().contains("registry-fallback@external"));
    assert!(report.failures().iter().any(|failure| {
        failure
            .error()
            .to_string()
            .contains("resolves outside configured install root")
    }));

    let _ = fs::remove_dir_all(config_home);
    let _ = fs::remove_dir_all(bundled_root);
    let _ = fs::remove_dir_all(external_install_path);
}

#[test]
fn installed_plugin_discovery_prunes_stale_registry_entries() {
    let config_home = temp_dir("registry-prune-home");
    let bundled_root = temp_dir("registry-prune-bundled");
    let install_root = config_home.join("plugins").join("installed");
    let missing_install_path = temp_dir("registry-prune-missing");

    let mut config = PluginManagerConfig::new(&config_home);
    config.bundled_root = Some(bundled_root.clone());
    config.install_root = Some(install_root);
    let manager = PluginManager::new(config);

    let mut registry = InstalledPluginRegistry::default();
    registry.plugins.insert(
        "stale-external@external".to_string(),
        InstalledPluginRecord {
            kind: PluginKind::External,
            id: "stale-external@external".to_string(),
            name: "stale-external".to_string(),
            version: "1.0.0".to_string(),
            description: "stale external plugin".to_string(),
            install_path: missing_install_path.clone(),
            source: PluginInstallSource::LocalPath {
                path: missing_install_path.clone(),
            },
            installed_at_unix_ms: 1,
            updated_at_unix_ms: 1,
            resolved_commit: None,
            content_sha256: None,
        },
    );
    manager.store_registry(&registry).expect("store registry");

    let installed = manager
        .list_installed_plugins()
        .expect("stale registry entries should be pruned");
    assert!(!installed
        .iter()
        .any(|plugin| plugin.metadata.id == "stale-external@external"));

    let registry = manager.load_registry().expect("load registry");
    assert!(!registry.plugins.contains_key("stale-external@external"));

    let _ = fs::remove_dir_all(config_home);
    let _ = fs::remove_dir_all(bundled_root);
}

#[test]
fn persists_bundled_plugin_enable_state_across_reloads() {
    let config_home = temp_dir("bundled-state-home");
    let bundled_root = temp_dir("bundled-state-root");
    write_bundled_plugin(&bundled_root.join("starter"), "starter", "0.1.0", false);

    let mut config = PluginManagerConfig::new(&config_home);
    config.bundled_root = Some(bundled_root.clone());
    let mut manager = PluginManager::new(config.clone());

    manager
        .enable("starter@bundled")
        .expect("enable bundled plugin should succeed");
    assert_eq!(
        load_enabled_plugins(&manager.settings_path()).get("starter@bundled"),
        Some(&true)
    );

    let mut reloaded_config = PluginManagerConfig::new(&config_home);
    reloaded_config.bundled_root = Some(bundled_root.clone());
    reloaded_config.enabled_plugins = load_enabled_plugins(&manager.settings_path());
    let reloaded_manager = PluginManager::new(reloaded_config);
    let reloaded = reloaded_manager
        .list_installed_plugins()
        .expect("bundled plugins should still be listed");
    assert!(reloaded
        .iter()
        .any(|plugin| { plugin.metadata.id == "starter@bundled" && plugin.enabled }));

    let _ = fs::remove_dir_all(config_home);
    let _ = fs::remove_dir_all(bundled_root);
}

#[test]
fn persists_bundled_plugin_disable_state_across_reloads() {
    let config_home = temp_dir("bundled-disabled-home");
    let bundled_root = temp_dir("bundled-disabled-root");
    write_bundled_plugin(&bundled_root.join("starter"), "starter", "0.1.0", true);

    let mut config = PluginManagerConfig::new(&config_home);
    config.bundled_root = Some(bundled_root.clone());
    let mut manager = PluginManager::new(config);

    manager
        .disable("starter@bundled")
        .expect("disable bundled plugin should succeed");
    assert_eq!(
        load_enabled_plugins(&manager.settings_path()).get("starter@bundled"),
        Some(&false)
    );

    let mut reloaded_config = PluginManagerConfig::new(&config_home);
    reloaded_config.bundled_root = Some(bundled_root.clone());
    reloaded_config.enabled_plugins = load_enabled_plugins(&manager.settings_path());
    let reloaded_manager = PluginManager::new(reloaded_config);
    let reloaded = reloaded_manager
        .list_installed_plugins()
        .expect("bundled plugins should still be listed");
    assert!(reloaded
        .iter()
        .any(|plugin| { plugin.metadata.id == "starter@bundled" && !plugin.enabled }));

    let _ = fs::remove_dir_all(config_home);
    let _ = fs::remove_dir_all(bundled_root);
}

#[test]
fn validates_plugin_source_before_install() {
    let config_home = temp_dir("validate-home");
    let source_root = temp_dir("validate-source");
    write_external_plugin(&source_root, "validator", "1.0.0");
    let manager = PluginManager::new(PluginManagerConfig::new(&config_home));
    let manifest = manager
        .validate_plugin_source(source_root.to_str().expect("utf8 path"))
        .expect("manifest should validate");
    assert_eq!(manifest.name, "validator");
    let _ = fs::remove_dir_all(config_home);
    let _ = fs::remove_dir_all(source_root);
}

#[test]
fn plugin_registry_tracks_enabled_state_and_lookup() {
    let config_home = temp_dir("registry-home");
    let source_root = temp_dir("registry-source");
    write_external_plugin(&source_root, "registry-demo", "1.0.0");

    let mut manager = PluginManager::new(PluginManagerConfig::new(&config_home));
    manager
        .install(source_root.to_str().expect("utf8 path"))
        .expect("install should succeed");
    manager
        .disable("registry-demo@external")
        .expect("disable should succeed");

    let registry = manager.plugin_registry().expect("registry should build");
    let plugin = registry
        .get("registry-demo@external")
        .expect("installed plugin should be discoverable");
    assert_eq!(plugin.metadata().name, "registry-demo");
    assert!(!plugin.is_enabled());
    assert!(registry.contains("registry-demo@external"));
    assert!(!registry.contains("missing@external"));

    let _ = fs::remove_dir_all(config_home);
    let _ = fs::remove_dir_all(source_root);
}

#[test]
fn plugin_registry_report_collects_load_failures_without_dropping_valid_plugins() {
    // given
    let config_home = temp_dir("report-home");
    let external_root = temp_dir("report-external");
    write_external_plugin(&external_root.join("valid"), "valid-report", "1.0.0");
    write_broken_plugin(&external_root.join("broken"), "broken-report");

    let mut config = PluginManagerConfig::new(&config_home);
    config.external_dirs = vec![external_root.clone()];
    let manager = PluginManager::new(config);

    // when
    let report = manager
        .plugin_registry_report()
        .expect("report should tolerate invalid external plugins");

    // then
    assert!(report.registry().contains("valid-report@external"));
    assert_eq!(report.failures().len(), 1);
    assert_eq!(report.failures()[0].kind, PluginKind::External);
    assert!(report.failures()[0]
        .plugin_root
        .ends_with(Path::new("broken")));
    assert!(report.failures()[0]
        .error()
        .to_string()
        .contains("does not exist"));

    let error = manager
        .plugin_registry()
        .expect_err("strict registry should surface load failures");
    match error {
        PluginError::LoadFailures(failures) => {
            assert_eq!(failures.len(), 1);
            assert!(failures[0].plugin_root.ends_with(Path::new("broken")));
        }
        other => panic!("expected load failures, got {other}"),
    }

    let _ = fs::remove_dir_all(config_home);
    let _ = fs::remove_dir_all(external_root);
}

#[test]
fn installed_plugin_registry_report_collects_load_failures_from_install_root() {
    // given
    let config_home = temp_dir("installed-report-home");
    let bundled_root = temp_dir("installed-report-bundled");
    let install_root = config_home.join("plugins").join("installed");
    write_external_plugin(&install_root.join("valid"), "installed-valid", "1.0.0");
    write_broken_plugin(&install_root.join("broken"), "installed-broken");

    let mut config = PluginManagerConfig::new(&config_home);
    config.bundled_root = Some(bundled_root.clone());
    config.install_root = Some(install_root);
    let manager = PluginManager::new(config);

    // when
    let report = manager
        .installed_plugin_registry_report()
        .expect("installed report should tolerate invalid installed plugins");

    // then
    assert!(report.registry().contains("installed-valid@external"));
    assert_eq!(report.failures().len(), 1);
    assert!(report.failures()[0]
        .plugin_root
        .ends_with(Path::new("broken")));

    let _ = fs::remove_dir_all(config_home);
    let _ = fs::remove_dir_all(bundled_root);
}

#[test]
fn rejects_plugin_sources_with_missing_hook_paths() {
    // given
    let config_home = temp_dir("broken-home");
    let source_root = temp_dir("broken-source");
    write_broken_plugin(&source_root, "broken");

    let manager = PluginManager::new(PluginManagerConfig::new(&config_home));

    // when
    let error = manager
        .validate_plugin_source(source_root.to_str().expect("utf8 path"))
        .expect_err("missing hook file should fail validation");

    // then
    assert!(error.to_string().contains("does not exist"));

    let mut manager = PluginManager::new(PluginManagerConfig::new(&config_home));
    let install_error = manager
        .install(source_root.to_str().expect("utf8 path"))
        .expect_err("install should reject invalid hook paths");
    assert!(install_error.to_string().contains("does not exist"));

    let _ = fs::remove_dir_all(config_home);
    let _ = fs::remove_dir_all(source_root);
}

#[test]
fn rejects_plugin_sources_with_missing_failure_hook_paths() {
    // given
    let config_home = temp_dir("broken-failure-home");
    let source_root = temp_dir("broken-failure-source");
    write_broken_failure_hook_plugin(&source_root, "broken-failure");

    let manager = PluginManager::new(PluginManagerConfig::new(&config_home));

    // when
    let error = manager
        .validate_plugin_source(source_root.to_str().expect("utf8 path"))
        .expect_err("missing failure hook file should fail validation");

    // then
    assert!(error.to_string().contains("does not exist"));

    let mut manager = PluginManager::new(PluginManagerConfig::new(&config_home));
    let install_error = manager
        .install(source_root.to_str().expect("utf8 path"))
        .expect_err("install should reject invalid failure hook paths");
    assert!(install_error.to_string().contains("does not exist"));

    let _ = fs::remove_dir_all(config_home);
    let _ = fs::remove_dir_all(source_root);
}

#[test]
fn plugin_registry_runs_initialize_and_shutdown_for_enabled_plugins() {
    let config_home = temp_dir("lifecycle-home");
    let source_root = temp_dir("lifecycle-source");
    let _ = write_lifecycle_plugin(&source_root, "lifecycle-demo", "1.0.0");

    let mut manager = PluginManager::new(PluginManagerConfig::new(&config_home));
    let install = manager
        .install(source_root.to_str().expect("utf8 path"))
        .expect("install should succeed");
    let log_path = install.install_path.join("lifecycle.log");

    let registry = manager.plugin_registry().expect("registry should build");
    registry.initialize().expect("init should succeed");
    registry.shutdown().expect("shutdown should succeed");

    let log = fs::read_to_string(&log_path).expect("lifecycle log should exist");
    assert_eq!(log, "init\nshutdown\n");

    let _ = fs::remove_dir_all(config_home);
    let _ = fs::remove_dir_all(source_root);
}

#[cfg(unix)]
#[test]
fn plugin_lifecycle_commands_receive_audit_environment() {
    let config_home = temp_dir("lifecycle-env-home");
    let source_root = temp_dir("lifecycle-env-source");
    write_file(
        source_root.join("lifecycle").join("init.sh").as_path(),
        "#!/bin/sh\nprintf '%s|%s|%s|%s|%s\\n' \"$ZO_PLUGIN_ID\" \"$ZO_PLUGIN_NAME\" \"$ZO_PLUGIN_LIFECYCLE_PHASE\" \"$ZO_PLUGIN_ROOT\" \"$ZO_PLUGIN_LIFECYCLE_COMMAND\" > lifecycle-env.log\n",
    );
    write_file(
        source_root.join(MANIFEST_RELATIVE_PATH).as_path(),
        r#"{
  "name": "lifecycle-env",
  "version": "1.0.0",
  "description": "lifecycle env plugin",
  "lifecycle": {
    "Init": ["./lifecycle/init.sh"]
  }
}"#,
    );

    let mut manager = PluginManager::new(PluginManagerConfig::new(&config_home));
    let install = manager
        .install(source_root.to_str().expect("utf8 path"))
        .expect("install should succeed");
    let registry = manager.plugin_registry().expect("registry should build");
    registry.initialize().expect("init should succeed");

    let log = fs::read_to_string(install.install_path.join("lifecycle-env.log"))
        .expect("lifecycle env log");
    let fields = log.trim().split('|').collect::<Vec<_>>();
    assert_eq!(fields[0], "lifecycle-env@external");
    assert_eq!(fields[1], "lifecycle-env");
    assert_eq!(fields[2], "init");
    assert_eq!(fields[3], install.install_path.display().to_string());
    assert!(fields[4].ends_with("lifecycle/init.sh"));

    let _ = fs::remove_dir_all(config_home);
    let _ = fs::remove_dir_all(source_root);
}

#[test]
fn aggregates_and_executes_plugin_tools() {
    let config_home = temp_dir("tool-home");
    let source_root = temp_dir("tool-source");
    write_tool_plugin(&source_root, "tool-demo", "1.0.0");

    let mut manager = PluginManager::new(PluginManagerConfig::new(&config_home));
    manager
        .install(source_root.to_str().expect("utf8 path"))
        .expect("install should succeed");

    let tools = manager.aggregated_tools().expect("tools should aggregate");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].definition().name, "plugin_echo");
    assert_eq!(tools[0].required_permission(), "workspace-write");

    let output = tools[0]
        .execute(&serde_json::json!({ "message": "hello" }))
        .expect("plugin tool should execute");
    let payload: Value = serde_json::from_str(&output).expect("valid json");
    assert_eq!(payload["plugin"], "tool-demo@external");
    assert_eq!(payload["tool"], "plugin_echo");
    assert_eq!(payload["input"]["message"], "hello");

    let _ = fs::remove_dir_all(config_home);
    let _ = fs::remove_dir_all(source_root);
}

#[test]
fn list_installed_plugins_scans_install_root_without_registry_entries() {
    let config_home = temp_dir("installed-scan-home");
    let bundled_root = temp_dir("installed-scan-bundled");
    let install_root = config_home.join("plugins").join("installed");
    let installed_plugin_root = install_root.join("scan-demo");
    write_file(
        installed_plugin_root.join(MANIFEST_FILE_NAME).as_path(),
        r#"{
  "name": "scan-demo",
  "version": "1.0.0",
  "description": "Scanned from install root"
}"#,
    );

    let mut config = PluginManagerConfig::new(&config_home);
    config.bundled_root = Some(bundled_root.clone());
    config.install_root = Some(install_root);
    let manager = PluginManager::new(config);

    let installed = manager
        .list_installed_plugins()
        .expect("installed plugins should scan directories");
    assert!(installed
        .iter()
        .any(|plugin| plugin.metadata.id == "scan-demo@external"));

    let _ = fs::remove_dir_all(config_home);
    let _ = fs::remove_dir_all(bundled_root);
}

#[test]
fn list_installed_plugins_scans_packaged_manifests_in_install_root() {
    let config_home = temp_dir("installed-packaged-scan-home");
    let bundled_root = temp_dir("installed-packaged-scan-bundled");
    let install_root = config_home.join("plugins").join("installed");
    let installed_plugin_root = install_root.join("scan-packaged");
    write_file(
        installed_plugin_root.join(MANIFEST_RELATIVE_PATH).as_path(),
        r#"{
  "name": "scan-packaged",
  "version": "1.0.0",
  "description": "Packaged manifest in install root"
}"#,
    );

    let mut config = PluginManagerConfig::new(&config_home);
    config.bundled_root = Some(bundled_root.clone());
    config.install_root = Some(install_root);
    let manager = PluginManager::new(config);

    let installed = manager
        .list_installed_plugins()
        .expect("installed plugins should scan packaged manifests");
    assert!(installed
        .iter()
        .any(|plugin| plugin.metadata.id == "scan-packaged@external"));

    let _ = fs::remove_dir_all(config_home);
    let _ = fs::remove_dir_all(bundled_root);
}

#[test]
fn installed_plugin_discovery_merges_lower_canonical_roots() {
    // A plugin installed under a lower-priority canonical home (e.g. `ZO_HOME`
    // or `$HOME/.zo`) must be discovered even though the primary root is empty.
    let primary_home = temp_dir("multiroot-primary-home");
    let secondary_home = temp_dir("multiroot-secondary-home");
    let bundled_root = temp_dir("multiroot-bundled");
    let primary_install = primary_home.join("plugins").join("installed");
    let secondary_install = secondary_home.join("plugins").join("installed");
    write_external_plugin(&secondary_install.join("lower-demo"), "lower-demo", "2.0.0");

    let mut config = PluginManagerConfig::new(&primary_home);
    config.bundled_root = Some(bundled_root.clone());
    config.install_root = Some(primary_install);
    config.discovery_install_roots = vec![secondary_install];
    let manager = PluginManager::new(config);

    let installed = manager
        .list_installed_plugins()
        .expect("lower canonical root plugin should load");
    let record = installed
        .iter()
        .find(|plugin| plugin.metadata.id == "lower-demo@external")
        .expect("plugin installed under lower canonical root should be discovered");
    assert_eq!(record.metadata.version, "2.0.0");

    let _ = fs::remove_dir_all(primary_home);
    let _ = fs::remove_dir_all(secondary_home);
    let _ = fs::remove_dir_all(bundled_root);
}

#[test]
fn installed_plugin_discovery_rejects_tampered_registered_lower_plugin() {
    let primary_home = temp_dir("multiroot-tampered-lower-primary");
    let secondary_home = temp_dir("multiroot-tampered-lower-secondary");
    let bundled_root = temp_dir("multiroot-tampered-lower-bundled");
    let source_root = temp_dir("multiroot-tampered-lower-source");
    write_external_plugin(&source_root, "tampered-lower", "1.0.0");

    let mut secondary_manager = PluginManager::new(PluginManagerConfig::new(&secondary_home));
    let install = secondary_manager
        .install(source_root.to_str().expect("utf8 path"))
        .expect("lower plugin should install");
    write_file(
        install.install_path.join("injected.sh").as_path(),
        "#!/bin/sh\\necho pwned\\n",
    );

    let mut config = PluginManagerConfig::new(&primary_home);
    config.bundled_root = Some(bundled_root.clone());
    config.discovery_install_roots = vec![secondary_home.join("plugins").join("installed")];
    let manager = PluginManager::new(config);

    let report = manager
        .installed_plugin_registry_report()
        .expect("tampered lower plugin should be reported, not loaded");
    assert!(!report.registry().contains("tampered-lower@external"));
    assert!(report
        .failures()
        .iter()
        .any(|failure| matches!(failure.error(), PluginError::IntegrityMismatch(_))));

    let _ = fs::remove_dir_all(primary_home);
    let _ = fs::remove_dir_all(secondary_home);
    let _ = fs::remove_dir_all(bundled_root);
    let _ = fs::remove_dir_all(source_root);
}

#[test]
fn tampered_primary_registered_plugin_reserves_id_against_lower_root() {
    let primary_home = temp_dir("multiroot-tampered-primary");
    let secondary_home = temp_dir("multiroot-tampered-shadow-lower");
    let bundled_root = temp_dir("multiroot-tampered-shadow-bundled");
    let primary_source = temp_dir("multiroot-tampered-shadow-primary-source");
    let secondary_source = temp_dir("multiroot-tampered-shadow-secondary-source");
    write_external_plugin(&primary_source, "shadowed", "9.9.9");
    write_external_plugin(&secondary_source, "shadowed", "1.0.0");

    let mut primary_manager = PluginManager::new(PluginManagerConfig::new(&primary_home));
    let primary_install = primary_manager
        .install(primary_source.to_str().expect("utf8 path"))
        .expect("primary plugin should install");
    let mut secondary_manager = PluginManager::new(PluginManagerConfig::new(&secondary_home));
    secondary_manager
        .install(secondary_source.to_str().expect("utf8 path"))
        .expect("lower plugin should install");
    write_file(
        primary_install.install_path.join("injected.sh").as_path(),
        "#!/bin/sh\\necho pwned\\n",
    );

    let mut config = PluginManagerConfig::new(&primary_home);
    config.bundled_root = Some(bundled_root.clone());
    config.discovery_install_roots = vec![secondary_home.join("plugins").join("installed")];
    let manager = PluginManager::new(config);

    let report = manager
        .installed_plugin_registry_report()
        .expect("tampered primary plugin should be reported");
    assert!(
        !report.registry().contains("shadowed@external"),
        "a lower-root copy must not replace a failed higher-priority plugin"
    );
    assert!(report
        .failures()
        .iter()
        .any(|failure| matches!(failure.error(), PluginError::IntegrityMismatch(_))));

    let _ = fs::remove_dir_all(primary_home);
    let _ = fs::remove_dir_all(secondary_home);
    let _ = fs::remove_dir_all(bundled_root);
    let _ = fs::remove_dir_all(primary_source);
    let _ = fs::remove_dir_all(secondary_source);
}

#[cfg(unix)]
#[test]
fn installed_plugin_discovery_rejects_symlinked_plugin_directory() {
    use std::os::unix::fs::symlink;

    let config_home = temp_dir("symlinked-installed-home");
    let bundled_root = temp_dir("symlinked-installed-bundled");
    let outside_root = temp_dir("symlinked-installed-outside");
    let install_root = config_home.join("plugins").join("installed");
    write_external_plugin(&outside_root, "symlink-escape", "1.0.0");
    fs::create_dir_all(&install_root).expect("install root");
    symlink(&outside_root, install_root.join("escape")).expect("plugin symlink");

    let mut config = PluginManagerConfig::new(&config_home);
    config.bundled_root = Some(bundled_root.clone());
    config.install_root = Some(install_root);
    let manager = PluginManager::new(config);

    let report = manager
        .installed_plugin_registry_report()
        .expect("symlinked plugin should be reported, not loaded");
    assert!(!report.registry().contains("symlink-escape@external"));
    assert!(report.failures().iter().any(|failure| {
        failure
            .error()
            .to_string()
            .contains("must not be a symlink")
    }));

    let _ = fs::remove_dir_all(config_home);
    let _ = fs::remove_dir_all(bundled_root);
    let _ = fs::remove_dir_all(outside_root);
}

#[test]
fn installed_plugin_discovery_prefers_higher_root_on_id_collision() {
    // The same plugin id present in both the primary and a lower canonical root
    // must resolve to the primary (higher-priority) root's copy.
    let primary_home = temp_dir("multiroot-precedence-primary");
    let secondary_home = temp_dir("multiroot-precedence-secondary");
    let bundled_root = temp_dir("multiroot-precedence-bundled");
    let primary_install = primary_home.join("plugins").join("installed");
    let secondary_install = secondary_home.join("plugins").join("installed");
    write_external_plugin(&primary_install.join("dup-demo"), "dup-demo", "9.9.9");
    write_external_plugin(&secondary_install.join("dup-demo"), "dup-demo", "1.0.0");

    let mut config = PluginManagerConfig::new(&primary_home);
    config.bundled_root = Some(bundled_root.clone());
    config.install_root = Some(primary_install);
    config.discovery_install_roots = vec![secondary_install];
    let manager = PluginManager::new(config);

    let installed = manager
        .list_installed_plugins()
        .expect("colliding plugin ids should load");
    let matches: Vec<_> = installed
        .iter()
        .filter(|plugin| plugin.metadata.id == "dup-demo@external")
        .collect();
    assert_eq!(matches.len(), 1, "id collision should dedupe to one entry");
    assert_eq!(
        matches[0].metadata.version, "9.9.9",
        "primary (higher-priority) root must win"
    );

    let _ = fs::remove_dir_all(primary_home);
    let _ = fs::remove_dir_all(secondary_home);
    let _ = fs::remove_dir_all(bundled_root);
}

#[test]
fn plugin_registry_writes_target_primary_root_only() {
    // Registry/enabled-state writes must land only in the primary root; the
    // lower canonical root used for discovery must stay untouched.
    let primary_home = temp_dir("multiroot-write-primary");
    let secondary_home = temp_dir("multiroot-write-secondary");
    let bundled_root = temp_dir("multiroot-write-bundled");
    let primary_install = primary_home.join("plugins").join("installed");
    let secondary_install = secondary_home.join("plugins").join("installed");
    write_external_plugin(&secondary_install.join("lower-demo"), "lower-demo", "2.0.0");

    let mut config = PluginManagerConfig::new(&primary_home);
    config.bundled_root = Some(bundled_root.clone());
    config.install_root = Some(primary_install);
    config.discovery_install_roots = vec![secondary_install.clone()];
    let manager = PluginManager::new(config);

    // Discovery reads the lower root, but toggling enabled state writes state.
    manager
        .write_enabled_state("lower-demo@external", Some(false))
        .expect("enabled-state write should succeed");
    let mut registry = InstalledPluginRegistry::default();
    registry.plugins.insert(
        "written@external".to_string(),
        InstalledPluginRecord {
            kind: PluginKind::External,
            id: "written@external".to_string(),
            name: "written".to_string(),
            version: "1.0.0".to_string(),
            description: "written".to_string(),
            install_path: primary_home.join("plugins").join("installed").join("written"),
            source: PluginInstallSource::LocalPath {
                path: primary_home.join("plugins").join("installed").join("written"),
            },
            installed_at_unix_ms: 1,
            updated_at_unix_ms: 1,
            resolved_commit: None,
            content_sha256: None,
        },
    );
    manager.store_registry(&registry).expect("store registry");

    // Every write landed under the primary home; the secondary home holds only
    // the plugin directory we seeded, with no registry/settings files created.
    let primary_paths: Vec<PathBuf> = walk_files(&primary_home);
    assert!(
        primary_paths.iter().any(|p| p.ends_with("installed.json")),
        "registry file must be written under primary home"
    );
    assert!(
        primary_paths.iter().any(|p| p.ends_with("settings.json")),
        "enabled-state file must be written under primary home"
    );

    let secondary_paths: Vec<PathBuf> = walk_files(&secondary_home);
    assert!(
        !secondary_paths
            .iter()
            .any(|p| p.ends_with("installed.json") || p.ends_with("settings.json")),
        "no registry/settings files may be written under the lower canonical root; found {secondary_paths:?}"
    );

    let _ = fs::remove_dir_all(primary_home);
    let _ = fs::remove_dir_all(secondary_home);
    let _ = fs::remove_dir_all(bundled_root);
}

fn walk_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(root) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk_files(&path));
        } else {
            out.push(path);
        }
    }
    out
}
