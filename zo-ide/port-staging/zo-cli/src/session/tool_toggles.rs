use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ToolToggleConfig {
    disabled_tools: Vec<String>,
    disabled_mcp_tools: Vec<DisabledMcpTool>,
}

#[derive(Debug, Deserialize)]
struct DisabledMcpTool {
    #[serde(alias = "server")]
    server_id: String,
    #[serde(alias = "tool")]
    tool_name: String,
}

pub(crate) fn load_disabled_tool_names(
    cwd: &Path,
) -> Result<BTreeSet<String>, Box<dyn std::error::Error>> {
    let path = tool_toggles_path(cwd);
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        Err(error) => return Err(Box::new(error)),
    };

    let config: ToolToggleConfig = serde_json::from_str(&raw)?;
    let mut disabled = config.disabled_tools.into_iter().collect::<BTreeSet<_>>();
    for tool in config.disabled_mcp_tools {
        disabled.insert(runtime::mcp_tool_name(&tool.server_id, &tool.tool_name));
    }
    Ok(disabled)
}

pub(crate) fn save_disabled_tool_names(
    cwd: &Path,
    disabled: &BTreeSet<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let dir = cwd.join(".zo");
    fs::create_dir_all(&dir)?;
    let path = tool_toggles_path(cwd);
    let mut doc = match fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_else(|_| serde_json::json!({})),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
        Err(error) => return Err(Box::new(error)),
    };
    if !doc.is_object() {
        doc = serde_json::json!({});
    }
    if let Some(map) = doc.as_object_mut() {
        map.remove("disabled_mcp_tools");
        map.insert(
            "disabled_tools".to_string(),
            serde_json::Value::Array(
                disabled
                    .iter()
                    .map(|name| serde_json::Value::String(name.clone()))
                    .collect(),
            ),
        );
    }

    let serialized = serde_json::to_string_pretty(&doc)?;
    let tmp = dir.join("tool-toggles.json.tmp");
    fs::write(&tmp, serialized)?;
    fs::rename(tmp, path)?;
    Ok(())
}

fn tool_toggles_path(cwd: &Path) -> std::path::PathBuf {
    cwd.join(".zo").join("tool-toggles.json")
}

#[cfg(test)]
mod tests {
    use super::{load_disabled_tool_names, save_disabled_tool_names};
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("zo-tool-toggles-{label}-{unique}"))
    }

    #[test]
    fn loads_name_and_mcp_tuple_toggles() {
        let cwd = temp_dir("load");
        fs::create_dir_all(cwd.join(".zo")).expect("config dir");
        fs::write(
            cwd.join(".zo").join("tool-toggles.json"),
            r#"{
              "disabled_tools": ["WebSearch"],
              "disabled_mcp_tools": [{ "server_id": "alpha", "tool_name": "echo" }]
            }"#,
        )
        .expect("write toggles");

        let disabled = load_disabled_tool_names(&cwd).expect("load toggles");
        assert!(disabled.contains("WebSearch"));
        assert!(disabled.contains("mcp__alpha__echo"));

        fs::remove_dir_all(cwd).expect("cleanup");
    }

    #[test]
    fn saves_canonical_disabled_tools_and_clears_tuple_field() {
        let cwd = temp_dir("save");
        fs::create_dir_all(cwd.join(".zo")).expect("config dir");
        fs::write(
            cwd.join(".zo").join("tool-toggles.json"),
            r#"{
              "disabled_mcp_tools": [{ "server_id": "old", "tool_name": "stale" }],
              "other": true
            }"#,
        )
        .expect("write toggles");

        save_disabled_tool_names(
            &cwd,
            &BTreeSet::from(["WebSearch".to_string(), "mcp__alpha__echo".to_string()]),
        )
        .expect("save toggles");
        let raw = fs::read_to_string(cwd.join(".zo").join("tool-toggles.json"))
            .expect("read saved toggles");
        let doc: serde_json::Value = serde_json::from_str(&raw).expect("json");
        assert_eq!(doc["other"], true);
        assert!(doc.get("disabled_mcp_tools").is_none());
        assert_eq!(doc["disabled_tools"][0], "WebSearch");
        assert_eq!(doc["disabled_tools"][1], "mcp__alpha__echo");

        fs::remove_dir_all(cwd).expect("cleanup");
    }
}
