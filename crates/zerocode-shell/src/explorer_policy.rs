//! One numbers table, shared by the searcher, operation owner and window.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct ExplorerPolicy {
    pub hit_cap: usize,
    pub max_file_bytes: u64,
    pub max_hit_line_chars: usize,
    pub search_timeout_ms: u64,
    pub regex_size_limit: usize,
    pub history_cap: usize,
    pub move_cap: usize,
    pub hook_pending_cap: usize,
    pub hook_pending_ms: u64,
    pub name_debounce_ms: u64,
    pub text_debounce_ms: u64,
    pub toast_ms: u64,
}
impl Default for ExplorerPolicy {
    fn default() -> Self {
        serde_json::from_str(include_str!("explorer_policy.json"))
            .expect("the explorer numbers table is checked in and tested")
    }
}
impl ExplorerPolicy {
    /// Settings may tighten resource budgets; invalid or unbounded values
    /// leave the table's ceiling in place. Durations also stay positive.
    pub(crate) fn overlay(settings: &serde_json::Value) -> Self {
        let mut table = serde_json::to_value(Self::default()).unwrap_or_default();
        if let Some(table) = table.as_object_mut() {
            for (key, value) in table {
                if let Some(asked) = settings.get(key).and_then(serde_json::Value::as_u64)
                    && asked > 0
                    && asked <= value.as_u64().unwrap_or_default()
                {
                    *value = asked.into();
                }
            }
        }
        serde_json::from_value(table).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explorer_settings_overlay_only_tightens_the_single_table() {
        let policy = ExplorerPolicy::overlay(
            &serde_json::json!({"history_cap": 2, "hit_cap": 0, "move_cap": 99999}),
        );
        assert_eq!(policy.history_cap, 2);
        assert_eq!(policy.hit_cap, ExplorerPolicy::default().hit_cap);
        assert_eq!(policy.move_cap, ExplorerPolicy::default().move_cap);
    }
}
