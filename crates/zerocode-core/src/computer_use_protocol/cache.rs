//! The snapshot cache every provider keeps between an observation and the
//! action that names one of its element indexes.
//!
//! `rememberSnapshot`/`cachedSnapshot`/`pruneSnapshotCache` from the macOS
//! helper, generic over the snapshot type: the KEYS are the contract. A
//! snapshot is filed under the query the agent used, the app's name, its
//! bundle id and `pid:N`, each alone and each joined with the window id and
//! window index it was taken for, and every one of those again inside the
//! request's namespace (`session:…`/`worktree:…`) — so two agents working
//! the same app in different worktrees never validate against each other's
//! indexes, while an agent that named no namespace finds its own snapshot by
//! whichever spelling it used last.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::marks::{Pin, pin_broken};
use super::render::Rect;
use super::{ProviderError, error_code};

/// `ComputerSnapshotCachePolicy.maxEntries`.
pub const MAX_ENTRIES: usize = 32;
/// `ComputerSnapshotCachePolicy.maxAge`.
pub const MAX_AGE: Duration = Duration::from_secs(2 * 60);

/// A snapshot older than this is not worth validating against.
#[must_use]
pub fn is_expired(created_at: Instant, now: Instant) -> bool {
    now.saturating_duration_since(created_at) > MAX_AGE
}

/// The oldest entry goes when the cache is over its count or that entry is
/// over its age.
#[must_use]
pub fn should_prune(entry_count: usize, oldest_created_at: Instant, now: Instant) -> bool {
    entry_count > MAX_ENTRIES || is_expired(oldest_created_at, now)
}

/// `session:<id>` beats `worktree:<path>` beats `default`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Namespace(String);

impl Namespace {
    #[must_use]
    pub fn from_request(session: Option<&str>, worktree: Option<&str>) -> Self {
        if let Some(session) = session.filter(|value| !value.is_empty()) {
            return Self(format!("session:{session}"));
        }
        if let Some(worktree) = worktree.filter(|value| !value.is_empty()) {
            return Self(format!("worktree:{worktree}"));
        }
        Self("default".into())
    }

    #[must_use]
    pub fn is_explicit(&self) -> bool {
        self.0 != "default"
    }

    fn key(&self, key: &str) -> String {
        format!("{}:{}", self.0, key.to_lowercase())
    }
}

fn window_key(query: &str, window_id: u64) -> String {
    format!("{}#window:{window_id}", query.to_lowercase())
}

fn canonical_window_id_key(window_id: u64) -> String {
    format!("window-id:{window_id}")
}

fn window_index_key(query: &str, window_index: usize) -> String {
    format!("{}#windowIndex:{window_index}", query.to_lowercase())
}

fn canonical_window_index_key(window_index: usize) -> String {
    format!("window-index:{window_index}")
}

/// What a snapshot must say about itself to be filed and found.
pub trait CachedSnapshot: Clone {
    fn id(&self) -> &str;
    fn window_id(&self) -> u64;
    /// The signature of the element at `index`, if it was indexed.
    fn element_signature(&self, index: usize) -> Option<&str>;
    /// The display name of the element at `index` (a row's text).
    fn element_name(&self, index: usize) -> Option<&str>;
    /// The nearest named element above the one at `index`.
    fn element_context(&self, index: usize) -> Option<&str>;
    /// The window-local frame of the element at `index`, if it has one.
    fn element_frame(&self, index: usize) -> Option<Rect>;
}

struct Entry {
    snapshot_id: String,
    keys: Vec<String>,
    created_at: Instant,
}

/// The cache itself. One per provider session; not shared across threads.
pub struct SnapshotCache<S: CachedSnapshot> {
    snapshots: HashMap<String, S>,
    entries: Vec<Entry>,
}

impl<S: CachedSnapshot> Default for SnapshotCache<S> {
    fn default() -> Self {
        Self {
            snapshots: HashMap::new(),
            entries: Vec::new(),
        }
    }
}

/// The identities a snapshot is filed under besides the raw query.
#[derive(Debug, Clone, Default)]
pub struct AppKeys {
    pub name: String,
    pub bundle_id: Option<String>,
    pub pid: u32,
}

impl<S: CachedSnapshot> SnapshotCache<S> {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// File a snapshot the way the helper does, then prune.
    pub fn remember(
        &mut self,
        query: &str,
        app: &AppKeys,
        namespace: &Namespace,
        window_index: Option<usize>,
        snapshot: S,
        now: Instant,
    ) {
        let keys: Vec<String> = [
            query.to_string(),
            app.name.clone(),
            app.bundle_id.clone().unwrap_or_default(),
            format!("pid:{}", app.pid),
        ]
        .into_iter()
        .filter(|key| !key.is_empty())
        .map(|key| key.to_lowercase())
        .collect();
        let explicit = namespace.is_explicit();
        let window_id = snapshot.window_id();
        let mut stored = Vec::new();
        let mut file = |key: String, snapshots: &mut HashMap<String, S>| {
            snapshots.insert(key.clone(), snapshot.clone());
            stored.push(key);
        };
        let canonical_window = canonical_window_id_key(window_id);
        if !explicit {
            file(canonical_window.to_lowercase(), &mut self.snapshots);
        }
        file(namespace.key(&canonical_window), &mut self.snapshots);
        if let Some(index) = window_index {
            let canonical_index = canonical_window_index_key(index);
            if !explicit {
                file(canonical_index.to_lowercase(), &mut self.snapshots);
            }
            file(namespace.key(&canonical_index), &mut self.snapshots);
        }
        for key in &keys {
            if !explicit {
                file(key.clone(), &mut self.snapshots);
                file(window_key(key, window_id), &mut self.snapshots);
                if let Some(index) = window_index {
                    file(window_index_key(key, index), &mut self.snapshots);
                }
            }
            file(namespace.key(key), &mut self.snapshots);
            file(
                namespace.key(&window_key(key, window_id)),
                &mut self.snapshots,
            );
            if let Some(index) = window_index {
                file(
                    namespace.key(&window_index_key(key, index)),
                    &mut self.snapshots,
                );
            }
        }
        self.entries.push(Entry {
            snapshot_id: snapshot.id().to_string(),
            keys: stored,
            created_at: now,
        });
        self.prune(now);
    }

    /// Drop the oldest entries while the policy says so. An entry's keys are
    /// removed only while they still point at that entry's snapshot: a newer
    /// snapshot filed under the same key stays.
    pub fn prune(&mut self, now: Instant) {
        while let Some(oldest) = self.entries.first()
            && should_prune(self.entries.len(), oldest.created_at, now)
        {
            let expired = self.entries.remove(0);
            for key in &expired.keys {
                if self
                    .snapshots
                    .get(key)
                    .is_some_and(|held| held.id() == expired.snapshot_id)
                {
                    self.snapshots.remove(key);
                }
            }
        }
    }

    /// The snapshot a follow-up action validates its indexes against.
    #[must_use]
    pub fn lookup(
        &self,
        query: &str,
        namespace: &Namespace,
        window_id: Option<u64>,
        window_index: Option<usize>,
    ) -> Option<&S> {
        if query.is_empty() {
            return None;
        }
        let explicit = namespace.is_explicit();
        let lowered = query.to_lowercase();
        if let Some(window_id) = window_id {
            let canonical = canonical_window_id_key(window_id);
            if let Some(cached) = self.snapshots.get(&namespace.key(&canonical)) {
                return Some(cached);
            }
            if !explicit && let Some(cached) = self.snapshots.get(&canonical.to_lowercase()) {
                return Some(cached);
            }
            let by_window = window_key(&lowered, window_id);
            if let Some(cached) = self.snapshots.get(&namespace.key(&by_window)) {
                return Some(cached);
            }
            if !explicit && let Some(cached) = self.snapshots.get(&by_window) {
                return Some(cached);
            }
            return None;
        }
        if let Some(window_index) = window_index {
            let canonical = canonical_window_index_key(window_index);
            if let Some(cached) = self.snapshots.get(&namespace.key(&canonical)) {
                return Some(cached);
            }
            if !explicit && let Some(cached) = self.snapshots.get(&canonical.to_lowercase()) {
                return Some(cached);
            }
            let by_index = window_index_key(&lowered, window_index);
            if let Some(cached) = self.snapshots.get(&namespace.key(&by_index)) {
                return Some(cached);
            }
            if !explicit && let Some(cached) = self.snapshots.get(&by_index) {
                return Some(cached);
            }
            return None;
        }
        self.snapshots
            .get(&namespace.key(&lowered))
            .or_else(|| (!explicit).then(|| self.snapshots.get(&lowered)).flatten())
    }
}

/// `validateRequestedElements`: every index an action names must exist in
/// the cached snapshot AND in the fresh one, with the same signature.
pub fn validate_requested_elements<S: CachedSnapshot>(
    cached: Option<&S>,
    current: &S,
    requested: &[usize],
) -> Result<(), ProviderError> {
    if requested.is_empty() {
        return Ok(());
    }
    let Some(cached) = cached else {
        return Err(ProviderError::new(
            error_code::ELEMENT_NOT_FOUND,
            "element indexes require a fresh get-app-state snapshot for this app/window",
        ));
    };
    for index in requested {
        match (
            cached.element_signature(*index),
            current.element_signature(*index),
        ) {
            (Some(expected), Some(actual)) if expected == actual => {}
            (Some(_), Some(_)) => {
                return Err(ProviderError::new(
                    error_code::ELEMENT_NOT_FOUND,
                    format!(
                        "element {index} changed since the last snapshot; run get-app-state again and use a fresh element index"
                    ),
                ));
            }
            _ => {
                return Err(ProviderError::new(
                    error_code::ELEMENT_NOT_FOUND,
                    format!(
                        "element {index} is stale; run get-app-state again and use a fresh element index"
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// A click by a mark's number: the element at `index` must still be the one
/// the mark was drawn on. The pin is the proof, so no cached snapshot is
/// needed — the marked look may have been filed anywhere.
pub fn validate_pinned_element<S: CachedSnapshot>(
    current: &S,
    index: usize,
    pin: &Pin,
) -> Result<(), ProviderError> {
    if pin.holds(
        current.element_signature(index),
        current.element_name(index),
        current.element_context(index),
        current.element_frame(index),
    ) {
        Ok(())
    } else {
        Err(pin_broken(index))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    struct Snap {
        id: String,
        window: u64,
        signatures: Vec<(usize, &'static str)>,
        names: Vec<(usize, &'static str)>,
        frames: Vec<(usize, Rect)>,
    }

    impl CachedSnapshot for Snap {
        fn id(&self) -> &str {
            &self.id
        }

        fn window_id(&self) -> u64 {
            self.window
        }

        fn element_signature(&self, index: usize) -> Option<&str> {
            self.signatures
                .iter()
                .find(|(held, _)| *held == index)
                .map(|(_, signature)| *signature)
        }

        fn element_name(&self, index: usize) -> Option<&str> {
            self.names
                .iter()
                .find(|(held, _)| *held == index)
                .map(|(_, name)| *name)
        }

        fn element_frame(&self, index: usize) -> Option<Rect> {
            self.frames
                .iter()
                .find(|(held, _)| *held == index)
                .map(|(_, frame)| *frame)
        }

        fn element_context(&self, index: usize) -> Option<&str> {
            (index == 1).then_some("Toolbar")
        }
    }

    fn snap(id: &str, window: u64) -> Snap {
        Snap {
            id: id.into(),
            window,
            signatures: vec![(0, "window"), (1, "button:OK")],
            names: vec![(1, "OK")],
            frames: vec![(1, Rect::new(10.0, 20.0, 60.0, 24.0))],
        }
    }

    fn app() -> AppKeys {
        AppKeys {
            name: "Notes".into(),
            bundle_id: Some("com.apple.Notes".into()),
            pid: 42,
        }
    }

    // ---- ComputerSnapshotCachePolicyTests.swift ----

    #[test]
    fn fresh_snapshots_at_the_age_boundary_do_not_expire() {
        let created = Instant::now();
        assert!(!is_expired(created, created + MAX_AGE));
        assert!(is_expired(
            created,
            created + MAX_AGE + Duration::from_millis(1)
        ));
    }

    #[test]
    fn prunes_when_the_cache_exceeds_its_entry_limit() {
        let now = Instant::now();
        assert!(should_prune(MAX_ENTRIES + 1, now, now));
        assert!(!should_prune(MAX_ENTRIES, now, now));
    }

    // ---- rememberSnapshot / cachedSnapshot ----

    #[test]
    fn a_snapshot_is_found_by_every_spelling_the_agent_might_use_next() {
        let mut cache = SnapshotCache::new();
        let now = Instant::now();
        let default = Namespace::from_request(None, None);
        cache.remember("notes", &app(), &default, Some(0), snap("S1", 7), now);
        for query in ["notes", "Notes", "COM.APPLE.NOTES", "pid:42"] {
            assert_eq!(
                cache
                    .lookup(query, &default, None, None)
                    .map(|s| s.id.as_str()),
                Some("S1"),
                "{query}"
            );
            assert_eq!(
                cache
                    .lookup(query, &default, Some(7), None)
                    .map(|s| s.id.as_str()),
                Some("S1")
            );
            assert_eq!(
                cache
                    .lookup(query, &default, None, Some(0))
                    .map(|s| s.id.as_str()),
                Some("S1")
            );
        }
        // A different window id or index of the same app is not this snapshot.
        assert!(cache.lookup("notes", &default, Some(8), None).is_none());
        assert!(cache.lookup("notes", &default, None, Some(1)).is_none());
        // Any query with the right window id finds it through the canonical key.
        assert_eq!(
            cache
                .lookup("whatever", &default, Some(7), None)
                .map(|s| s.id.as_str()),
            Some("S1")
        );
        assert!(cache.lookup("", &default, None, None).is_none());
    }

    #[test]
    fn an_explicit_namespace_is_its_own_world() {
        let mut cache = SnapshotCache::new();
        let now = Instant::now();
        let session = Namespace::from_request(Some("s1"), Some("/w"));
        assert_eq!(session, Namespace("session:s1".into()));
        assert!(session.is_explicit());
        let worktree = Namespace::from_request(None, Some("/w"));
        assert_eq!(worktree, Namespace("worktree:/w".into()));
        let default = Namespace::from_request(Some(""), Some(""));
        assert!(!default.is_explicit());

        cache.remember("notes", &app(), &session, None, snap("S1", 7), now);
        assert_eq!(
            cache
                .lookup("notes", &session, None, None)
                .map(|s| s.id.as_str()),
            Some("S1")
        );
        assert!(
            cache.lookup("notes", &default, None, None).is_none(),
            "an explicit namespace files nothing globally"
        );
        assert!(cache.lookup("notes", &worktree, None, None).is_none());
        assert!(cache.lookup("notes", &session, Some(9), None).is_none());
        assert_eq!(
            cache
                .lookup("notes", &session, Some(7), None)
                .map(|s| s.id.as_str()),
            Some("S1")
        );

        cache.remember("notes", &app(), &default, None, snap("S2", 7), now);
        assert_eq!(
            cache
                .lookup("notes", &default, None, None)
                .map(|s| s.id.as_str()),
            Some("S2")
        );
        assert_eq!(
            cache
                .lookup("notes", &session, None, None)
                .map(|s| s.id.as_str()),
            Some("S1"),
            "the default write does not overwrite the session's"
        );
    }

    #[test]
    fn the_oldest_entry_leaves_at_the_count_limit_and_a_newer_key_owner_stays() {
        let mut cache = SnapshotCache::new();
        let start = Instant::now();
        let default = Namespace::from_request(None, None);
        for round in 0..MAX_ENTRIES {
            cache.remember(
                &format!("app{round}"),
                &AppKeys {
                    name: format!("App {round}"),
                    bundle_id: None,
                    pid: round as u32 + 1,
                },
                &default,
                None,
                snap(&format!("S{round}"), round as u64),
                start + Duration::from_millis(round as u64),
            );
        }
        assert_eq!(cache.len(), MAX_ENTRIES);
        assert!(cache.lookup("app0", &default, None, None).is_some());
        // The 33rd write evicts the first.
        cache.remember(
            "app-new",
            &app(),
            &default,
            None,
            snap("SN", 99),
            start + Duration::from_secs(1),
        );
        assert_eq!(cache.len(), MAX_ENTRIES);
        assert!(cache.lookup("app0", &default, None, None).is_none());
        assert!(cache.lookup("app1", &default, None, None).is_some());

        // A key re-filed by a newer snapshot survives the older entry's eviction.
        let mut cache = SnapshotCache::new();
        cache.remember("notes", &app(), &default, None, snap("OLD", 7), start);
        cache.remember(
            "notes",
            &app(),
            &default,
            None,
            snap("NEW", 7),
            start + Duration::from_secs(1),
        );
        cache.prune(start + MAX_AGE + Duration::from_millis(500));
        assert_eq!(
            cache
                .lookup("notes", &default, None, None)
                .map(|s| s.id.as_str()),
            Some("NEW")
        );
        cache.prune(start + Duration::from_secs(1) + MAX_AGE + Duration::from_millis(1));
        assert!(cache.lookup("notes", &default, None, None).is_none());
        assert!(cache.is_empty());
    }

    // ---- validateRequestedElements ----

    #[test]
    fn stale_or_changed_indexes_are_refused_with_the_helpers_words() {
        let cached = snap("S1", 7);
        let mut current = snap("S2", 7);
        assert!(validate_requested_elements(Some(&cached), &current, &[0, 1]).is_ok());
        assert!(validate_requested_elements(None, &current, &[]).is_ok());
        let missing = validate_requested_elements(None, &current, &[1]).unwrap_err();
        assert_eq!(missing.code, "element_not_found");
        assert!(missing.message.contains("fresh get-app-state"));
        let gone = validate_requested_elements(Some(&cached), &current, &[5]).unwrap_err();
        assert!(gone.message.contains("element 5 is stale"));
        current.signatures[1] = (1, "button:Cancel");
        let changed = validate_requested_elements(Some(&cached), &current, &[1]).unwrap_err();
        assert!(changed.message.contains("element 1 changed"));
    }

    #[test]
    fn a_pinned_element_needs_no_cached_snapshot_but_its_own_face() {
        let pin = Pin {
            signature: "button:OK".into(),
            name: "OK".into(),
            context: "Toolbar".into(),
            frame: Rect::new(10.0, 20.0, 60.0, 24.0),
            tolerance: 2.0,
        };
        let mut current = snap("S2", 7);
        assert!(validate_pinned_element(&current, 1, &pin).is_ok());
        current.frames[0] = (1, Rect::new(10.0, 44.0, 60.0, 24.0));
        let moved = validate_pinned_element(&current, 1, &pin).unwrap_err();
        assert_eq!(moved.code, "element_not_found");
        assert!(
            moved.message.contains("look again with --marks"),
            "{}",
            moved.message
        );
        let mut current = snap("S2", 7);
        current.signatures[1] = (1, "button:Cancel");
        assert!(validate_pinned_element(&current, 1, &pin).is_err());
        assert!(
            validate_pinned_element(&snap("S2", 7), 5, &pin).is_err(),
            "no element there"
        );
    }
}
