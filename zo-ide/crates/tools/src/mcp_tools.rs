use serde::Deserialize;
use serde_json::{json, Value};

use super::{
    from_value, maybe_enforce_permission_check, to_pretty_json, ToolContext, ToolError, ToolSpec,
};
use runtime::{lsp_client::LspRegistry, PermissionMode};

#[derive(Debug, Deserialize)]
pub(crate) struct LspInput {
    pub action: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub line: Option<u32>,
    #[serde(default)]
    pub character: Option<u32>,
    #[serde(default)]
    pub query: Option<String>,
}

pub(crate) fn tool_specs() -> Vec<ToolSpec> {
    vec![ToolSpec {
        name: "LSP",
        description: "Query connected Language Server Protocol backends for code intelligence (hover, definition, references, symbols, diagnostics).",
        input_schema: json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["symbols", "references", "diagnostics", "definition", "hover"] },
                "path": { "type": "string" },
                "line": { "type": "integer", "minimum": 0 },
                "character": { "type": "integer", "minimum": 0 },
                "query": { "type": "string" }
            },
            "required": ["action"],
            "additionalProperties": false
        }),
        required_permission: PermissionMode::ReadOnly,
    }]
}

pub(crate) fn dispatch(
    ctx: &ToolContext,
    enforcer: Option<&runtime::permission_enforcer::PermissionEnforcer>,
    name: &str,
    input: &Value,
) -> Option<Result<String, ToolError>> {
    match name {
        "LSP" => Some(
            maybe_enforce_permission_check(enforcer, name, input)
                .and_then(|()| from_value::<LspInput>(input).and_then(|i| run_lsp(&ctx.lsp, &i))),
        ),
        _ => None,
    }
}

fn run_lsp(registry: &LspRegistry, input: &LspInput) -> Result<String, ToolError> {
    let action = &input.action;
    let path = input.path.as_deref();
    let line = input.line;
    let character = input.character;
    let query = input.query.as_deref();

    let result =
        api::sync_bridge::run_blocking(registry.dispatch(action, path, line, character, query));

    match result {
        Ok(result) => to_pretty_json(result),
        Err(e) => to_pretty_json(json!({
            "action": action,
            "error": e,
            "status": "error"
        })),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use runtime::lsp_client::{LspAction, LspServerStatus, LspTransport};

    #[derive(Debug)]
    struct MockLspTransport {
        response: Value,
    }

    impl LspTransport for MockLspTransport {
        fn dispatch(
            &self,
            _action: LspAction,
            _path: &str,
            _line: u32,
            _character: u32,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, String>> + Send>>
        {
            let response = Ok(self.response.clone());
            Box::pin(async move { response })
        }
    }

    #[test]
    fn lsp_tool_renders_transport_success_as_json() {
        let registry = LspRegistry::new();
        registry.register_with_transport(
            "rust",
            LspServerStatus::Connected,
            None,
            vec!["hover".into()],
            Some(Arc::new(MockLspTransport {
                response: serde_json::json!({"hover": "docs"}),
            })),
        );

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let _guard = runtime.enter();

        let output = run_lsp(
            &registry,
            &LspInput {
                action: "hover".to_string(),
                path: Some("src/main.rs".to_string()),
                line: Some(3),
                character: Some(7),
                query: None,
            },
        )
        .expect("lsp tool should serialize success");

        let output_json: Value = serde_json::from_str(&output).expect("json");
        assert_eq!(output_json["hover"], "docs");
    }

    #[test]
    fn lsp_tool_wraps_runtime_errors_in_status_error_json() {
        let registry = LspRegistry::new();
        registry.register(
            "rust",
            LspServerStatus::Disconnected,
            None,
            vec!["hover".into()],
        );

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let _guard = runtime.enter();

        let output = run_lsp(
            &registry,
            &LspInput {
                action: "hover".to_string(),
                path: Some("src/main.rs".to_string()),
                line: Some(1),
                character: Some(0),
                query: None,
            },
        )
        .expect("lsp tool should serialize error");

        let output_json: Value = serde_json::from_str(&output).expect("json");
        assert_eq!(output_json["action"], "hover");
        assert_eq!(output_json["status"], "error");
        assert!(output_json["error"]
            .as_str()
            .expect("error string")
            .contains("is not connected"));
    }

    #[test]
    fn lsp_tool_wraps_missing_capability_errors_in_status_error_json() {
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

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let _guard = runtime.enter();

        let output = run_lsp(
            &registry,
            &LspInput {
                action: "definition".to_string(),
                path: Some("src/main.rs".to_string()),
                line: Some(1),
                character: Some(0),
                query: None,
            },
        )
        .expect("lsp tool should serialize unsupported-capability error");

        let output_json: Value = serde_json::from_str(&output).expect("json");
        assert_eq!(output_json["action"], "definition");
        assert_eq!(output_json["status"], "error");
        assert!(output_json["error"]
            .as_str()
            .expect("error string")
            .contains("does not advertise support for definition"));
    }

    #[test]
    fn lsp_tool_runs_without_preexisting_tokio_runtime() {
        let registry = LspRegistry::new();
        registry.register_with_transport(
            "rust",
            LspServerStatus::Connected,
            None,
            vec!["hover".into()],
            Some(Arc::new(MockLspTransport {
                response: serde_json::json!({"hover": "docs"}),
            })),
        );

        let output = run_lsp(
            &registry,
            &LspInput {
                action: "hover".to_string(),
                path: Some("src/main.rs".to_string()),
                line: Some(3),
                character: Some(7),
                query: None,
            },
        )
        .expect("lsp tool should create a local runtime when none exists");

        let output_json: Value = serde_json::from_str(&output).expect("json");
        assert_eq!(output_json["hover"], "docs");
    }
}
