use thiserror::Error;

/// Structured error type for tool execution failures.
#[derive(Debug, Error)]
pub enum ToolError {
    /// Tool name not found in any dispatcher.
    #[error("unsupported tool: {0}")]
    NotFound(String),

    /// Permission denied by the enforcement layer.
    #[error("permission denied for `{tool}`: {reason}")]
    PermissionDenied { tool: String, reason: String },

    /// Invalid input (deserialization or validation failure).
    #[error("invalid input: {0}")]
    InvalidInput(String),

    /// Tool execution failed.
    #[error("execution error: {0}")]
    Execution(String),

    /// The dispatch policy declined the call — a small delegated task kept
    /// inline, an implementation model reserved for orchestration. Not a
    /// failure of anything: the verdict tells the model what to do instead,
    /// and the screen heads it as a verdict, not a crash.
    #[error("declined: {0}")]
    Declined(String),

    /// I/O error during tool execution.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON serialization/deserialization error.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// Plugin tool name conflict.
    #[error("plugin tool `{0}` conflicts with a built-in tool name")]
    PluginConflict(String),

    /// Duplicate tool name.
    #[error("duplicate tool name: `{0}`")]
    DuplicateName(String),
}

impl From<String> for ToolError {
    fn from(s: String) -> Self {
        Self::Execution(s)
    }
}

/// The runtime's error for a tool call, with the policy's verdict carried
/// as its kind rather than as words the screen would have to recognise.
impl From<ToolError> for runtime::ToolError {
    fn from(error: ToolError) -> Self {
        let words = error.to_string();
        match error {
            ToolError::Declined(_) => runtime::ToolError::declined(words),
            _ => runtime::ToolError::new(words),
        }
    }
}

#[cfg(test)]
mod declined_tests {
    use super::*;

    #[test]
    fn a_declined_verdict_crosses_to_the_runtime_as_a_declined_error() {
        let crossed = runtime::ToolError::from(ToolError::Declined("do it inline".to_owned()));
        assert!(crossed.is_declined());
        assert_eq!(crossed.to_string(), "declined: do it inline");
        let failed = runtime::ToolError::from(ToolError::Execution("boom".to_owned()));
        assert!(!failed.is_declined());
        assert_eq!(failed.to_string(), "execution error: boom");
    }
}
