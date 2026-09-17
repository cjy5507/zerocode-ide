//! Pane identity.
//!
//! Two identifiers, deliberately not merged: a **durable** key that survives
//! restart and appears in persisted layout plus every hook request, and a
//! **runtime handle** that is a small 1-based integer the pane manager and the
//! renderer can pass around cheaply. Collapsing them into one id is what forces
//! either UUIDs into hot render paths or unstable integers into persisted
//! state.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// First runtime handle. Handles are 1-based so `0` stays available as "no
/// pane" in the view layer without an extra Option in the wire payload.
pub const FIRST_PANE_HANDLE: PaneHandle = PaneHandle(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PaneHandle(pub u32);

impl PaneHandle {
    pub const fn next(self) -> Self {
        PaneHandle(self.0 + 1)
    }
}

impl fmt::Display for PaneHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Durable pane identity: which tab, and which leaf within that tab's split
/// tree. Serialized as the single string `"<tab_id>/<leaf_id>"` so it can be a
/// map key in persisted layout and a plain form field in a hook request.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PaneKey {
    pub tab_id: Uuid,
    pub leaf_id: Uuid,
}

impl PaneKey {
    pub fn new(tab_id: Uuid, leaf_id: Uuid) -> Self {
        Self { tab_id, leaf_id }
    }

    pub fn random() -> Self {
        Self {
            tab_id: Uuid::new_v4(),
            leaf_id: Uuid::new_v4(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneKeyParseError {
    /// Not exactly one `/` separator.
    Shape,
    /// One of the halves is not a UUID.
    NotAUuid,
}

impl fmt::Display for PaneKeyParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PaneKeyParseError::Shape => f.write_str("expected \"<tab_id>/<leaf_id>\""),
            PaneKeyParseError::NotAUuid => f.write_str("tab_id and leaf_id must be UUIDs"),
        }
    }
}

impl std::error::Error for PaneKeyParseError {}

impl fmt::Display for PaneKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.tab_id, self.leaf_id)
    }
}

impl FromStr for PaneKey {
    type Err = PaneKeyParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (tab, leaf) = value.split_once('/').ok_or(PaneKeyParseError::Shape)?;
        if leaf.contains('/') {
            return Err(PaneKeyParseError::Shape);
        }
        Ok(PaneKey {
            tab_id: Uuid::parse_str(tab).map_err(|_| PaneKeyParseError::NotAUuid)?,
            leaf_id: Uuid::parse_str(leaf).map_err(|_| PaneKeyParseError::NotAUuid)?,
        })
    }
}

impl Serialize for PaneKey {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for PaneKey {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        raw.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_start_at_one_and_advance() {
        assert_eq!(FIRST_PANE_HANDLE.0, 1);
        assert_eq!(FIRST_PANE_HANDLE.next(), PaneHandle(2));
    }

    #[test]
    fn pane_key_round_trips_through_its_string_form() {
        let key = PaneKey::random();
        let text = key.to_string();
        assert_eq!(text.parse::<PaneKey>().expect("parse"), key);

        let json = serde_json::to_string(&key).expect("serialize");
        assert_eq!(json, format!("\"{text}\""));
        assert_eq!(
            serde_json::from_str::<PaneKey>(&json).expect("deserialize"),
            key
        );
    }

    #[test]
    fn malformed_pane_keys_are_errors_not_silent_defaults() {
        assert_eq!("nope".parse::<PaneKey>(), Err(PaneKeyParseError::Shape));
        assert_eq!("a/b/c".parse::<PaneKey>(), Err(PaneKeyParseError::Shape));
        assert_eq!(
            "not-a-uuid/also-not".parse::<PaneKey>(),
            Err(PaneKeyParseError::NotAUuid)
        );
    }
}
