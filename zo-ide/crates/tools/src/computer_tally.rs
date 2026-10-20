//! Per-project Computer call counts, without page or request text.

use runtime::ToolExecutionResult;
use serde_json::Value;

use super::{ToolContext, ToolError};

pub(crate) fn run(
    _input: &Value,
    _ctx: &ToolContext,
    execute: impl FnOnce() -> Result<ToolExecutionResult, ToolError>,
) -> Result<ToolExecutionResult, ToolError> {
    execute()
}

pub(crate) fn receipt(_answer: &Value) {}

#[cfg(test)]
mod tests;
