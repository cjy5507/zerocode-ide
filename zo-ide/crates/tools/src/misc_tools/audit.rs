//! `Audit` — read the tool-invocation ledger (read-only).
//!
//! Surfaces the `ToolGateway` shadow ledger that `record_tool_invocation` fills on
//! every dispatch but that previously had no reader (WI-E2): a rollup of how many
//! tools ran, how many the policy allowed / denied / failed, and the reason for
//! every denial. A pure read of the current `ToolContext` ledger — it records
//! nothing and mutates nothing, so it never appears in its own summary as a
//! write. Takes no arguments.

use super::{to_pretty_json, ToolContext, ToolError};

pub(crate) fn run_audit(ctx: &ToolContext) -> Result<String, ToolError> {
    to_pretty_json(ctx.audit_summary())
}
