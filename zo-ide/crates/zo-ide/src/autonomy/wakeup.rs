//! Turn-scoped dynamic loop scheduling tool.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use super::limits::AutonomyLimits;

pub const TOOL_NAME: &str = "loop_schedule";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopScheduleRequest {
    pub delay_secs: u64,
    #[serde(default)]
    pub noop: bool,
    #[serde(default)]
    pub stop: bool,
    pub reason: Option<String>,
}

pub struct LoopToolScope;

#[derive(Deserialize)]
struct WireRequest {
    delay_secs: Option<u64>,
    #[serde(default)]
    noop: bool,
    #[serde(default)]
    stop: bool,
    reason: Option<String>,
}

static ACTIVE_SCOPES: AtomicUsize = AtomicUsize::new(0);
static REQUEST: OnceLock<Mutex<Option<LoopScheduleRequest>>> = OnceLock::new();

fn request_slot() -> &'static Mutex<Option<LoopScheduleRequest>> {
    REQUEST.get_or_init(|| Mutex::new(None))
}

impl Drop for LoopToolScope {
    fn drop(&mut self) {
        ACTIVE_SCOPES.fetch_sub(1, Ordering::AcqRel);
    }
}

#[must_use]
pub fn begin_scope() -> LoopToolScope {
    if ACTIVE_SCOPES.fetch_add(1, Ordering::AcqRel) == 0 {
        *request_slot()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }
    LoopToolScope
}

#[must_use]
pub fn scope_active() -> bool {
    ACTIVE_SCOPES.load(Ordering::Acquire) > 0
}

#[must_use]
pub fn tool_definition_if_active() -> Option<api::ToolDefinition> {
    if !scope_active() {
        return None;
    }
    let limits = AutonomyLimits::load();
    Some(api::ToolDefinition {
        name: TOOL_NAME.to_string(),
        description: Some(
            "Schedule this loop's next iteration, mark a quiet iteration, or stop it. Available only during a dynamic loop iteration."
                .to_string(),
        ),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "delay_secs": {
                    "type": "integer",
                    "minimum": limits.min_model_wakeup_secs,
                    "maximum": limits.max_model_wakeup_secs
                },
                "noop": {"type": "boolean", "default": false},
                "stop": {"type": "boolean", "default": false},
                "reason": {"type": "string"}
            },
            "additionalProperties": false
        }),
    })
}

pub fn execute(input: serde_json::Value) -> Result<String, runtime::ToolError> {
    if !scope_active() {
        return Err(runtime::ToolError::new(
            "loop_schedule is available only during a dynamic loop iteration",
        ));
    }
    let wire: WireRequest = serde_json::from_value(input)
        .map_err(|error| runtime::ToolError::new(format!("invalid loop_schedule input: {error}")))?;
    let limits = AutonomyLimits::load();
    let delay_secs = wire
        .delay_secs
        .unwrap_or(limits.default_model_wakeup_secs)
        .clamp(
            limits.min_model_wakeup_secs,
            limits.max_model_wakeup_secs,
        );
    let request = LoopScheduleRequest {
        delay_secs,
        noop: wire.noop,
        stop: wire.stop,
        reason: wire.reason.filter(|reason| !reason.trim().is_empty()),
    };
    *request_slot()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(request.clone());
    serde_json::to_string(&request).map_err(|error| runtime::ToolError::new(error.to_string()))
}

#[must_use]
pub fn take_request() -> Option<LoopScheduleRequest> {
    request_slot()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
}

#[cfg(test)]
mod tests {
    use super::{begin_scope, execute, take_request, tool_definition_if_active, TOOL_NAME};
    use crate::autonomy::limits::AutonomyLimits;

    #[test]
    fn tool_is_advertised_only_inside_a_loop_scope() {
        let _lock = crate::test_env_lock();
        assert!(tool_definition_if_active().is_none());
        let _scope = begin_scope();
        assert_eq!(
            tool_definition_if_active().map(|definition| definition.name),
            Some(TOOL_NAME.to_string())
        );
    }

    #[test]
    fn execution_clamps_delay_and_records_the_response() {
        let _lock = crate::test_env_lock();
        let limits = AutonomyLimits::default();
        let _scope = begin_scope();
        execute(serde_json::json!({
            "delay_secs": 1,
            "noop": true,
            "reason": "nothing changed"
        }))
        .expect("execute");

        let request = take_request().expect("request");
        assert_eq!(request.delay_secs, limits.min_model_wakeup_secs);
        assert!(request.noop);
    }
}
