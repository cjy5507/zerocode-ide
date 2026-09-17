//! One hook payload, parsed once.
//!
//! A hook payload crosses the bridge as TEXT deliberately — it is the vendor's
//! own schema, and each reader keeps deciding for itself what the fields mean
//! ([`crate::hook`]'s own words). What the readers used to repeat was not the
//! reading but the PARSE: one `Stop` envelope carries the whole answer, and
//! the shell's report path handed that same text to serde a dozen times per
//! event — [`crate::hook`], [`crate::transcript`], [`crate::ask`] and
//! [`crate::provider_session`] each parsing privately. This type is the
//! boundary that pays the parse at most once; every reader borrows the tree
//! and keeps its own answer for text that is not JSON.
//!
//! The parse is LAZY. Half the readers sit behind cheap gates — an event-name
//! match, an agent check — and a payload nobody looks inside should cost what
//! it always cost: nothing.

use std::cell::OnceCell;

/// What [`HookPayload::tree_or_null`] answers for text that is not JSON —
/// shared, so the answer allocates nothing.
static NULL: serde_json::Value = serde_json::Value::Null;

/// A payload's JSON tree, parsed on first look and remembered.
///
/// The readers disagree about what unparseable text means — `None`, `false`,
/// a default enum arm, or "carry on with `Null`" — so this type does not
/// decide. [`HookPayload::tree`] serves the readers that stop,
/// [`HookPayload::tree_or_null`] the ones that carry on; each keeps the
/// failure semantics its `&str` twin always had.
pub struct HookPayload<'a> {
    text: &'a str,
    tree: OnceCell<Option<serde_json::Value>>,
}

impl<'a> HookPayload<'a> {
    /// Wrap the payload text. Nothing is parsed until a reader looks.
    #[must_use]
    pub fn of(text: &'a str) -> Self {
        Self {
            text,
            tree: OnceCell::new(),
        }
    }

    fn parsed(&self) -> &Option<serde_json::Value> {
        self.tree
            .get_or_init(|| serde_json::from_str(self.text).ok())
    }

    /// The tree, or `None` when the text was not JSON — for readers whose
    /// answer to a broken payload is "nothing".
    #[must_use]
    pub fn tree(&self) -> Option<&serde_json::Value> {
        self.parsed().as_ref()
    }

    /// The tree with `Null` standing in for text that was not JSON — for
    /// readers that treat both silences the same.
    #[must_use]
    pub fn tree_or_null(&self) -> &serde_json::Value {
        self.tree().unwrap_or(&NULL)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_once_and_answers_both_ways() {
        let payload = HookPayload::of(r#"{"tool_name":"Bash"}"#);
        assert_eq!(
            payload
                .tree()
                .and_then(|tree| tree.get("tool_name")?.as_str()),
            Some("Bash")
        );
        assert_eq!(
            payload
                .tree_or_null()
                .get("tool_name")
                .and_then(|v| v.as_str()),
            Some("Bash")
        );
    }

    #[test]
    fn text_that_is_not_json_answers_none_and_null() {
        let payload = HookPayload::of("not json at all");
        assert!(payload.tree().is_none());
        assert!(payload.tree_or_null().is_null());
    }
}
