use serde::{Deserialize, Serialize};

pub const MAX_SURFACE_ID_BYTES: usize = 512;
pub const SURFACE_EXECUTION_CACHE_LIMIT: usize = 128;
pub const EXECUTION_ENVELOPE_ENV: &str = "ZEROCODE_EXECUTION_ENVELOPE";
pub const EXECUTION_ENVELOPE_HEADER: &str = "x-zerocode-execution";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionEnvelope {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface_execution: Option<SurfaceExecution>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SurfaceExecution {
    pub surface_id: String,
    pub state_revision: u64,
    pub effect: SurfaceEffect,
}

impl SurfaceExecution {
    #[must_use]
    pub fn is_valid(&self) -> bool {
        !self.surface_id.is_empty()
            && self.surface_id.len() <= MAX_SURFACE_ID_BYTES
            && match self.effect {
                SurfaceEffect::ConfirmedChange { previous_revision } => {
                    previous_revision < self.state_revision
                }
                _ => true,
            }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum SurfaceEffect {
    Observation,
    Acknowledged,
    ConfirmedChange {
        #[serde(rename = "previousRevision")]
        previous_revision: u64,
    },
    Uncertain,
}
