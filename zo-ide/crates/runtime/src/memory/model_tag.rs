//! "Which model learned this" — the single canonical form of that answer.
//!
//! Durable memory stays keyed by *project*, not by model: a fact about this
//! codebase is a fact whichever model wrote it down, and partitioning the store
//! per model would make every model relearn the same repository. What genuinely
//! varies by model is habit — which tool it keeps misusing, how it likes to be
//! told things — so each entry instead records the model that authored it and
//! recall prefers the current model's entries over another model's.
//!
//! Normalization lives here and nowhere else: no other module compares raw
//! provider model strings, and no module spells a model name out. Callers hand
//! over whatever the session calls its active model and get back either a tag
//! or `None`; everything downstream — the metadata line, its parser, the recall
//! ranking — speaks [`MemoryModelTag`].

/// Longest model id that may be stamped onto an entry. Real ids are well under
/// this; the cap exists so a pathological caller cannot grow a metadata line
/// (which recall reads back on every entry) without bound. An over-long id is
/// rejected rather than truncated — a truncated id is a *different* identity,
/// and quietly inventing one would boost the wrong entries.
const MAX_TAG_BYTES: usize = 128;

/// The model that authored a memory entry, normalized to one comparable form.
///
/// Construction is the only way to get one, so an instance is always writable
/// onto a metadata line and always comparable with `==`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryModelTag(String);

impl MemoryModelTag {
    /// Normalize a session's active model id into a tag, or `None` when it
    /// cannot be one.
    ///
    /// Rejected — deliberately leaving the entry untagged rather than tagged
    /// wrongly: an empty/blank id, an id past `MAX_TAG_BYTES`, and an id
    /// carrying a character that could not survive the round trip through the
    /// `- memory_metadata:` line (`;` separates its fields, and a control
    /// character — newline above all — would end the line early). An untagged
    /// entry is still recalled; a corrupted metadata line would cost the entry
    /// its whole classification.
    #[must_use]
    pub fn new(raw: &str) -> Option<Self> {
        let normalized = raw.trim().to_lowercase();
        let writable = !normalized.is_empty()
            && normalized.len() <= MAX_TAG_BYTES
            && !normalized
                .chars()
                .any(|ch| ch == ';' || ch.is_control());
        writable.then_some(Self(normalized))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for MemoryModelTag {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::{MemoryModelTag, MAX_TAG_BYTES};

    #[test]
    fn normalizes_case_and_surrounding_space_so_one_model_is_one_identity() {
        let tag = MemoryModelTag::new("  Claude-Opus-5  ").expect("a real model id is a tag");
        assert_eq!(tag.as_str(), "claude-opus-5");
        assert_eq!(tag, MemoryModelTag::new("claude-opus-5").expect("same id"));
        assert_ne!(tag, MemoryModelTag::new("gpt-5.6-sol").expect("other id"));
    }

    #[test]
    fn rejects_ids_that_could_not_round_trip_through_a_metadata_line() {
        assert_eq!(MemoryModelTag::new(""), None);
        assert_eq!(MemoryModelTag::new("   "), None);
        assert_eq!(MemoryModelTag::new("model;kind=preference"), None);
        assert_eq!(MemoryModelTag::new("model\nwritten_at=0"), None);
        assert_eq!(MemoryModelTag::new(&"m".repeat(MAX_TAG_BYTES + 1)), None);
        assert!(MemoryModelTag::new(&"m".repeat(MAX_TAG_BYTES)).is_some());
    }

    #[test]
    fn keeps_the_bracketed_variant_ids_zo_actually_ships() {
        // `claude-opus-5[1m]` and friends carry brackets and dots; neither
        // breaks the metadata line, so neither may cost an entry its tag.
        for id in ["claude-opus-5[1m]", "gpt-5.6-sol", "claude-haiku-4-5-20251001"] {
            assert_eq!(
                MemoryModelTag::new(id).map(|tag| tag.as_str().to_string()),
                Some(id.to_string()),
                "{id} must survive tagging verbatim"
            );
        }
    }
}
