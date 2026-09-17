use serde::{Deserialize, Serialize};

/// Result of comparing multiple candidate answers for the same task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CouncilOutcome {
    /// One candidate clearly wins, usually by self-consistency majority or a
    /// later judge verdict.
    BestOf {
        winner_index: usize,
        supporting_indices: Vec<usize>,
        rationale: String,
    },
    /// No honest winner can be selected.
    Tie { reason: String },
}
