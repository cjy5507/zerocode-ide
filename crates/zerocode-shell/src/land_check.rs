//! 착지 전 점검 (t-34501 3단계, t-42447) — 창의 반쪽.
//!
//! RED STAGE: the names and shapes below are the ones the window will keep; the bodies are not
//! built yet, so every road panics or refuses until the green commit lands.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use zerocode_core::orchestration::land_check::{LandCheckAsk, LandCheckReceipt};

/// How long a check's first answer may take before the window says `started` and goes on alone.
pub(crate) const ANSWER_BUDGET: Duration = Duration::from_secs(7);

/// The limit a project gets when it names none for its check command.
pub(crate) const DEFAULT_TIMEOUT: Duration = Duration::from_secs(1800);

/// The one place a check's folders, prepared rows and logs live: `<orchestration data>/land-check`.
#[derive(Debug, Clone)]
pub(crate) struct Store {
    root: PathBuf,
}

impl Store {
    /// Make (or find) the store under the data root and answer its path.
    pub(crate) fn at(data_root: &Path) -> Result<Store, String> {
        // RED STAGE (t-42447): the path is named, but nothing is made under it yet.
        Ok(Store {
            root: data_root.join("land-check"),
        })
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }
}

/// How one check runs: the project's command (none means "merge only"), its limit, the compare ref
/// the project pins, and the shell that reads the command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Policy {
    pub(crate) command: Option<String>,
    pub(crate) timeout: Duration,
    pub(crate) pinned_base: Option<String>,
    pub(crate) shell: PathBuf,
}

impl Policy {
    /// The project's stored words, read the way the settings read them: a blank command is none, a
    /// missing or zero limit is [`DEFAULT_TIMEOUT`], a blank pin is none.
    pub(crate) fn from_settings(
        _command: Option<&str>,
        _timeout_secs: Option<u64>,
        _pinned_base: Option<&str>,
    ) -> Policy {
        // RED STAGE (t-42447): the stored words are not read yet; every project is merge-only.
        Policy {
            command: None,
            timeout: DEFAULT_TIMEOUT,
            pinned_base: None,
            shell: PathBuf::from("/bin/bash"),
        }
    }
}

/// Where the window hands each receipt of one check's evidence: the ledger's letter road in
/// production, a channel in tests.
pub(crate) type Sink = Arc<dyn Fn(LandCheckReceipt) + Send + Sync>;

/// Carry out one `land-check` ask. Answers the JSON the verb prints, or the refusal's words.
pub(crate) fn run(
    _store: &Store,
    _policy_for: &dyn Fn(&Path) -> Policy,
    _ask: &LandCheckAsk,
    _now_ms: i64,
    _sink: &Sink,
) -> Result<String, String> {
    // RED STAGE (t-42447): the check does not run yet. It answers a state no check can have, so
    // each test fails at its own assertion about the state it expected.
    Ok(serde_json::json!({ "state": "unbuilt" }).to_string())
}

#[cfg(test)]
mod tests;
