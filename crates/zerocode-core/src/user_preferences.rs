use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const FILE_NAME: &str = "user-preferences.json";
pub const DIRECTORY_NAME: &str = "memory";
pub const MAX_BYTES: usize = 262_144;
pub const MAX_ENTRIES: usize = 128;
pub const MAX_PER_SCOPE: usize = 8;
pub const MAX_TEXT_BYTES: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PreferenceScope {
    Personal,
    Project { key: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FeedbackOrigin {
    pub artifact_id: String,
    pub version: u32,
    pub sha256: Option<String>,
    pub feedback_key: String,
    #[serde(default)]
    pub followed_link: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SavedPreference {
    pub id: String,
    pub text: String,
    pub scope: PreferenceScope,
    pub origin: FeedbackOrigin,
    pub created_ms: i64,
    pub updated_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PreferenceError {
    InvalidDocument,
    InvalidText,
    InvalidOrigin,
    ScopeFull,
    StoreFull,
}

impl PreferenceError {
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::InvalidDocument => "saved preference state is invalid",
            Self::InvalidText => "a preference must be one nonempty line of at most 1024 bytes",
            Self::InvalidOrigin => "preference provenance is invalid",
            Self::ScopeFull => "this preference scope is full",
            Self::StoreFull => "the preference store is full",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreferenceBook {
    version: u32,
    generation: u64,
    entries: Vec<SavedPreference>,
}

impl Default for PreferenceBook {
    fn default() -> Self {
        Self {
            version: 1,
            generation: 0,
            entries: Vec::new(),
        }
    }
}

fn valid_text(text: &str) -> bool {
    !text.trim().is_empty()
        && text == text.trim()
        && text.len() <= MAX_TEXT_BYTES
        && !text
            .chars()
            .any(|character| character.is_control() || matches!(character, '\u{2028}' | '\u{2029}'))
}

fn digest_word(word: &str) -> bool {
    word.len() == 64
        && word
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_scope(scope: &PreferenceScope) -> bool {
    match scope {
        PreferenceScope::Personal => true,
        PreferenceScope::Project { key } => digest_word(key),
    }
}

fn valid_origin(origin: &FeedbackOrigin) -> bool {
    !origin.artifact_id.is_empty()
        && origin.artifact_id.len() <= 256
        && !origin.artifact_id.chars().any(char::is_control)
        && digest_word(&origin.feedback_key)
        && origin.sha256.as_deref().is_none_or(digest_word)
}

#[must_use]
pub fn content_key(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[must_use]
pub fn project_key(cwd: &Path) -> Option<String> {
    let local = cwd
        .ancestors()
        .find(|root| root.join(".git").exists())
        .unwrap_or(cwd);
    let root = crate::git_dir::owning_checkout_of(cwd).or_else(|| local.canonicalize().ok())?;
    Some(content_key(root.as_os_str().as_encoded_bytes()))
}

fn preference_id(text: &str, scope: &PreferenceScope) -> String {
    let mut hash = Sha256::new();
    match scope {
        PreferenceScope::Personal => hash.update(b"personal\0"),
        PreferenceScope::Project { key } => {
            hash.update(b"project\0");
            hash.update(key.as_bytes());
            hash.update(b"\0");
        }
    }
    hash.update(text.as_bytes());
    format!("{:x}", hash.finalize())
}

impl PreferenceBook {
    pub fn parse(bytes: &[u8]) -> Result<Self, PreferenceError> {
        if bytes.len() > MAX_BYTES {
            return Err(PreferenceError::InvalidDocument);
        }
        let book: Self =
            serde_json::from_slice(bytes).map_err(|_| PreferenceError::InvalidDocument)?;
        book.validate()?;
        Ok(book)
    }

    pub fn encode(&self) -> Result<Vec<u8>, PreferenceError> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|_| PreferenceError::InvalidDocument)?;
        if bytes.len() > MAX_BYTES {
            return Err(PreferenceError::StoreFull);
        }
        Ok(bytes)
    }

    fn validate(&self) -> Result<(), PreferenceError> {
        if self.version != 1 || self.entries.len() > MAX_ENTRIES {
            return Err(PreferenceError::InvalidDocument);
        }
        let mut ids = HashSet::new();
        for entry in &self.entries {
            if !valid_text(&entry.text)
                || !valid_scope(&entry.scope)
                || !valid_origin(&entry.origin)
                || entry.created_ms < 0
                || entry.updated_ms < entry.created_ms
                || entry.id != preference_id(&entry.text, &entry.scope)
                || !ids.insert(&entry.id)
                || self
                    .entries
                    .iter()
                    .filter(|other| other.scope == entry.scope)
                    .count()
                    > MAX_PER_SCOPE
            {
                return Err(PreferenceError::InvalidDocument);
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn entries(&self) -> &[SavedPreference] {
        &self.entries
    }

    #[must_use]
    pub fn active_for(&self, project: Option<&str>) -> Vec<&SavedPreference> {
        let mut entries: Vec<_> = self
            .entries
            .iter()
            .filter(|entry| match &entry.scope {
                PreferenceScope::Personal => true,
                PreferenceScope::Project { key } => Some(key.as_str()) == project,
            })
            .collect();
        entries.sort_by_key(|entry| {
            (
                matches!(entry.scope, PreferenceScope::Project { .. }),
                entry.updated_ms,
            )
        });
        entries
    }

    pub fn save(
        &mut self,
        text: &str,
        scope: PreferenceScope,
        origin: FeedbackOrigin,
        now_ms: i64,
    ) -> Result<String, PreferenceError> {
        let text = text.trim();
        if !valid_text(text) {
            return Err(PreferenceError::InvalidText);
        }
        if !valid_scope(&scope) || !valid_origin(&origin) || now_ms < 0 {
            return Err(PreferenceError::InvalidOrigin);
        }
        let id = preference_id(text, &scope);
        let mut staged = self.clone();
        if let Some(held) = staged.entries.iter_mut().find(|entry| entry.id == id) {
            if held.origin == origin {
                return Ok(id);
            }
            held.origin = origin;
            held.updated_ms = now_ms.max(held.updated_ms);
        } else {
            if staged
                .entries
                .iter()
                .filter(|entry| entry.scope == scope)
                .count()
                >= MAX_PER_SCOPE
            {
                return Err(PreferenceError::ScopeFull);
            }
            if staged.entries.len() >= MAX_ENTRIES {
                return Err(PreferenceError::StoreFull);
            }
            staged.entries.push(SavedPreference {
                id: id.clone(),
                text: text.into(),
                scope,
                origin,
                created_ms: now_ms,
                updated_ms: now_ms,
            });
        }
        staged.generation = staged
            .generation
            .checked_add(1)
            .ok_or(PreferenceError::InvalidDocument)?;
        staged.encode()?;
        *self = staged;
        Ok(id)
    }

    pub fn revoke(&mut self, id: &str) -> Result<bool, PreferenceError> {
        if !digest_word(id) {
            return Err(PreferenceError::InvalidDocument);
        }
        if !self.entries.iter().any(|entry| entry.id == id) {
            return Ok(false);
        }
        let next = self
            .generation
            .checked_add(1)
            .ok_or(PreferenceError::InvalidDocument)?;
        self.entries.retain(|entry| entry.id != id);
        self.generation = next;
        Ok(true)
    }
}

#[cfg(test)]
mod tests;
