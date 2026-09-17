//! Metadata describing the latest compaction that summarized a session.

use std::collections::BTreeMap;

use crate::json::JsonValue;

use super::json_field::{i64_from_usize, required_string, required_u32, required_usize};
use super::SessionError;

/// Typed, accumulating summary of everything compacted out of a session — the
/// single source of truth for the model-facing continuation message.
///
/// LAVA P1 replaces the old prose "summary of a summary" (which re-parsed and
/// re-truncated the prior round's text every compaction, eroding identifiers
/// over a long session) with this typed anchor. Each round folds in only the
/// *new* delta; the accumulating sections (`concepts`/`files`/
/// `errors_and_fixes`/`problem_solving`/`user_messages`) carry prior entries
/// forward **verbatim** and merely append/dedup, so a fact captured in round 1
/// is byte-identical in round 50. The snapshot sections (`intent`/
/// `pending_tasks`/`current_work`) reflect the latest known state.
///
/// All fields are plain values with no ordering significance beyond insertion;
/// the eight fields mirror the eight sections of `COMPACTION_SYSTEM_PROMPT`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AnchorSummary {
    /// Section 1 — Primary Request and Intent (stable; set once, refreshed only
    /// when a later round supplies a non-empty value).
    pub intent: String,
    /// Section 2 — Key Technical Concepts (union, deduplicated).
    pub concepts: Vec<String>,
    /// Section 3 — Files and Code Sections (union; an entry naming a path seen
    /// before supersedes the older entry for that path).
    pub files: Vec<String>,
    /// Section 4 — Errors and Fixes (append, deduplicated).
    pub errors_and_fixes: Vec<String>,
    /// Section 5 — Problem Solving (append history).
    pub problem_solving: Vec<String>,
    /// Section 6 — All User Messages (append; never dropped, so intent and
    /// corrections survive every round).
    pub user_messages: Vec<String>,
    /// Section 7 — Pending Tasks (snapshot: replaced by the latest round's).
    pub pending_tasks: Vec<String>,
    /// Section 8 — Current Work and Next Step (snapshot: replaced each round).
    pub current_work: String,
    /// LAVA P1 — the Raw Vault seq spans this anchor's rounds sealed, one
    /// inclusive `(lo, hi)` per compaction round (accumulating, in round order).
    /// Not summary content: it names WHERE the lossless originals live so the
    /// continuation message can advertise the exact `session_recall` range.
    /// Deliberately excluded from [`Self::is_empty`] (an anchor with only ranges
    /// carries no recoverable prose and must still be treated as empty, to
    /// preserve `prepare_compaction`'s `filter(!is_empty)` semantics). Serialized
    /// as `"vault_ranges": [[lo, hi], ...]`; absent in pre-P1 records (loads as
    /// an empty vec). A binary predating this field silently drops the key when
    /// it re-serializes an anchor — the recall spans are lost, but every prose
    /// entry is preserved, so recovery degrades to the pre-range behavior.
    pub vault_ranges: Vec<(u32, u32)>,
}

impl AnchorSummary {
    /// `true` when the anchor carries no content (a fresh or empty anchor).
    ///
    /// `vault_ranges` is intentionally NOT consulted: it is location metadata,
    /// not recoverable content, so an anchor holding only ranges must still read
    /// as empty. `prepare_compaction` filters an empty anchor out before folding
    /// (falling back to the rendered continuation), and treating a ranges-only
    /// anchor as non-empty there would resurrect a contentless anchor.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.intent.is_empty()
            && self.concepts.is_empty()
            && self.files.is_empty()
            && self.errors_and_fixes.is_empty()
            && self.problem_solving.is_empty()
            && self.user_messages.is_empty()
            && self.pending_tasks.is_empty()
            && self.current_work.is_empty()
    }

    fn to_json(&self) -> JsonValue {
        let mut object = BTreeMap::new();
        if !self.intent.is_empty() {
            object.insert("intent".to_string(), JsonValue::String(self.intent.clone()));
        }
        insert_string_array(&mut object, "concepts", &self.concepts);
        insert_string_array(&mut object, "files", &self.files);
        insert_string_array(&mut object, "errors_and_fixes", &self.errors_and_fixes);
        insert_string_array(&mut object, "problem_solving", &self.problem_solving);
        insert_string_array(&mut object, "user_messages", &self.user_messages);
        insert_string_array(&mut object, "pending_tasks", &self.pending_tasks);
        if !self.current_work.is_empty() {
            object.insert(
                "current_work".to_string(),
                JsonValue::String(self.current_work.clone()),
            );
        }
        if !self.vault_ranges.is_empty() {
            object.insert(
                "vault_ranges".to_string(),
                JsonValue::Array(
                    self.vault_ranges
                        .iter()
                        .map(|(lo, hi)| {
                            JsonValue::Array(vec![
                                JsonValue::Number(i64::from(*lo)),
                                JsonValue::Number(i64::from(*hi)),
                            ])
                        })
                        .collect(),
                ),
            );
        }
        JsonValue::Object(object)
    }

    fn from_json(value: &JsonValue) -> Self {
        let Some(object) = value.as_object() else {
            return Self::default();
        };
        let string_field = |key: &str| {
            object
                .get(key)
                .and_then(JsonValue::as_str)
                .unwrap_or_default()
                .to_string()
        };
        Self {
            intent: string_field("intent"),
            concepts: string_array(object, "concepts"),
            files: string_array(object, "files"),
            errors_and_fixes: string_array(object, "errors_and_fixes"),
            problem_solving: string_array(object, "problem_solving"),
            user_messages: string_array(object, "user_messages"),
            pending_tasks: string_array(object, "pending_tasks"),
            current_work: string_field("current_work"),
            vault_ranges: vault_ranges_from_json(object),
        }
    }
}

/// Parse the `"vault_ranges": [[lo, hi], ...]` array back into inclusive spans.
/// Absent (pre-P1 records) or malformed entries yield an empty vec / are skipped
/// — tolerant, like the other anchor readers, so an old or partial record loads
/// rather than erroring.
fn vault_ranges_from_json(object: &BTreeMap<String, JsonValue>) -> Vec<(u32, u32)> {
    object
        .get("vault_ranges")
        .and_then(JsonValue::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let pair = item.as_array()?;
                    let lo = u32::try_from(pair.first()?.as_i64()?).ok()?;
                    let hi = u32::try_from(pair.get(1)?.as_i64()?).ok()?;
                    Some((lo, hi))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn insert_string_array(object: &mut BTreeMap<String, JsonValue>, key: &str, values: &[String]) {
    if values.is_empty() {
        return;
    }
    object.insert(
        key.to_string(),
        JsonValue::Array(values.iter().cloned().map(JsonValue::String).collect()),
    );
}

fn string_array(object: &BTreeMap<String, JsonValue>, key: &str) -> Vec<String> {
    object
        .get(key)
        .and_then(JsonValue::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(ToOwned::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// Metadata describing the latest compaction that summarized a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCompaction {
    pub count: u32,
    pub removed_message_count: usize,
    pub summary: String,
    pub first_kept_message_index: Option<u32>,
    /// Typed accumulating anchor — the single source of truth the model-facing
    /// continuation message is rendered from (LAVA P1). `None` for sessions
    /// compacted before P1, which fall back to prose recovery.
    pub anchor: Option<AnchorSummary>,
}

impl SessionCompaction {
    pub fn to_json(&self) -> Result<JsonValue, SessionError> {
        let mut object = BTreeMap::new();
        object.insert(
            "count".to_string(),
            JsonValue::Number(i64::from(self.count)),
        );
        object.insert(
            "removed_message_count".to_string(),
            JsonValue::Number(i64_from_usize(
                self.removed_message_count,
                "removed_message_count",
            )?),
        );
        object.insert(
            "summary".to_string(),
            JsonValue::String(self.summary.clone()),
        );
        if let Some(first_kept) = self.first_kept_message_index {
            object.insert(
                "first_kept_message_index".to_string(),
                JsonValue::Number(i64::from(first_kept)),
            );
        }
        if let Some(anchor) = &self.anchor {
            object.insert("anchor".to_string(), anchor.to_json());
        }
        Ok(JsonValue::Object(object))
    }

    pub fn to_jsonl_record(&self) -> Result<JsonValue, SessionError> {
        let mut object = BTreeMap::new();
        object.insert(
            "type".to_string(),
            JsonValue::String("compaction".to_string()),
        );
        object.insert(
            "count".to_string(),
            JsonValue::Number(i64::from(self.count)),
        );
        object.insert(
            "removed_message_count".to_string(),
            JsonValue::Number(i64_from_usize(
                self.removed_message_count,
                "removed_message_count",
            )?),
        );
        object.insert(
            "summary".to_string(),
            JsonValue::String(self.summary.clone()),
        );
        if let Some(first_kept) = self.first_kept_message_index {
            object.insert(
                "first_kept_message_index".to_string(),
                JsonValue::Number(i64::from(first_kept)),
            );
        }
        if let Some(anchor) = &self.anchor {
            object.insert("anchor".to_string(), anchor.to_json());
        }
        Ok(JsonValue::Object(object))
    }

    pub(super) fn from_json(value: &JsonValue) -> Result<Self, SessionError> {
        let object = value
            .as_object()
            .ok_or_else(|| SessionError::Format("compaction must be an object".to_string()))?;
        let first_kept_message_index = object
            .get("first_kept_message_index")
            .map(|value| {
                value
                    .as_i64()
                    .and_then(|raw| u32::try_from(raw).ok())
                    .ok_or_else(|| {
                        SessionError::Format("first_kept_message_index out of range".to_string())
                    })
            })
            .transpose()?;
        Ok(Self {
            count: required_u32(object, "count")?,
            removed_message_count: required_usize(object, "removed_message_count")?,
            summary: required_string(object, "summary")?,
            first_kept_message_index,
            anchor: object.get("anchor").map(AnchorSummary::from_json),
        })
    }
}
