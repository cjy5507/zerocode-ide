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

/// 워크스페이스 설정 디렉터리와 파일 이름 — 경로 조각은 여기 한 번만 적는다.
const ZO_DIR: &str = ".zo";
const TOOL_TOGGLES_FILE: &str = "tool-toggles.json";

fn tool_toggles_path(cwd: &Path) -> std::path::PathBuf {
    cwd.join(ZO_DIR).join(TOOL_TOGGLES_FILE)
}

#[cfg(test)]
mod tests {
    use super::load_disabled_tool_names;
    
    use std::fs;
    
    


    #[test]
    fn loads_name_and_mcp_tuple_toggles() {
        let cwd = crate::support::temp_dir("load");
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

}
