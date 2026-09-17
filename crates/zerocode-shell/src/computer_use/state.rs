//! The operator's memory of its own hands (docs/design/
//! computer-use-full-operator.md §7.2, §7.4): what it last did, whether it
//! worked, how many times in a row it did not, and whether the same action
//! keeps producing the same screen — written as `state.json` beside the
//! evidence so a model whose context was cut can pick the thread up, and
//! read back by `status`.

use std::path::Path;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use zerocode_core::computer_use::COMPUTER_STUCK_REPEATS;

pub const STATE_FILE: &str = "state.json";
/// How many recent action fingerprints the state keeps.
pub const RECENT_ACTIONS: usize = 6;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperatorState {
    pub actions: u64,
    #[serde(rename = "lastVerb")]
    pub last_verb: Option<String>,
    #[serde(rename = "lastOk")]
    pub last_ok: Option<bool>,
    #[serde(rename = "lastError")]
    pub last_error: Option<String>,
    #[serde(rename = "lastAtMs")]
    pub last_at_ms: i64,
    #[serde(rename = "consecutiveFailures")]
    pub consecutive_failures: u32,
    /// Fingerprints (verb + arguments) of the most recent actions, newest last.
    pub recent: Vec<String>,
    /// Looks in a row that saw nothing change.
    #[serde(rename = "unchangedLooks")]
    pub unchanged_looks: u32,
    /// The same action, repeated, with nothing changing: a loop, not progress.
    pub stuck: bool,
}

impl OperatorState {
    /// One action answered.
    pub fn note_action(
        &mut self,
        verb: &str,
        fingerprint: &str,
        ok: bool,
        error: Option<&str>,
        at_ms: i64,
    ) {
        self.actions += 1;
        self.last_verb = Some(verb.to_string());
        self.last_ok = Some(ok);
        self.last_error = if ok { None } else { error.map(str::to_string) };
        self.last_at_ms = at_ms;
        self.consecutive_failures = if ok { 0 } else { self.consecutive_failures + 1 };
        self.recent.push(fingerprint.to_string());
        if self.recent.len() > RECENT_ACTIONS {
            self.recent.remove(0);
        }
        // A new action is a new chance; the look after it decides.
        self.stuck = false;
    }

    /// One look answered: whether anything changed since the last look.
    /// The same action `COMPUTER_STUCK_REPEATS` times with nothing changing
    /// in between marks the operator stuck.
    pub fn note_look(&mut self, changed: bool) {
        if changed {
            self.unchanged_looks = 0;
            self.stuck = false;
            return;
        }
        self.unchanged_looks += 1;
        let repeats = COMPUTER_STUCK_REPEATS as usize;
        let same_action_repeated = self.recent.len() >= repeats
            && self.recent[self.recent.len() - repeats..]
                .windows(2)
                .all(|pair| pair[0] == pair[1]);
        self.stuck = same_action_repeated && self.unchanged_looks >= COMPUTER_STUCK_REPEATS;
    }

    /// Write `state.json` into the evidence folder. Best effort.
    pub fn write(&self, dir: &Path) {
        if let Ok(bytes) = serde_json::to_vec_pretty(self) {
            let _ = std::fs::write(dir.join(STATE_FILE), bytes);
        }
    }
}

static STATE: Mutex<OperatorState> = Mutex::new(OperatorState {
    actions: 0,
    last_verb: None,
    last_ok: None,
    last_error: None,
    last_at_ms: 0,
    consecutive_failures: 0,
    recent: Vec::new(),
    unchanged_looks: 0,
    stuck: false,
});

/// A verb and its arguments as one word — what "the same action" means —
/// through the one redactor the step log uses: state.json sits in the same
/// evidence folder, so a typed secret must not reach it either (what was
/// typed is kept as its length).
#[must_use]
pub fn fingerprint(argv: &[String]) -> String {
    crate::run_evidence::redacted("computer", argv)
        .into_iter()
        .filter(|word| word != "--json")
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn note_action(
    verb: &str,
    argv: &[String],
    ok: bool,
    error: Option<&str>,
    at_ms: i64,
    dir: Option<&Path>,
) {
    let mut held = STATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    held.note_action(verb, &fingerprint(argv), ok, error, at_ms);
    if let Some(dir) = dir {
        held.write(dir);
    }
}

pub fn note_look(changed: bool, dir: Option<&Path>) -> bool {
    let mut held = STATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    held.note_look(changed);
    if let Some(dir) = dir {
        held.write(dir);
    }
    held.stuck
}

#[must_use]
pub fn snapshot() -> Value {
    serde_json::to_value(
        STATE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone(),
    )
    .unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_state_counts_failures_in_a_row_and_a_repeated_action_with_no_change_is_stuck() {
        let mut state = OperatorState::default();
        state.note_action(
            "mouse-click",
            "mouse-click --x 1 --y 1",
            false,
            Some("stopped"),
            10,
        );
        state.note_action(
            "mouse-click",
            "mouse-click --x 1 --y 1",
            false,
            Some("stopped"),
            20,
        );
        assert_eq!(state.consecutive_failures, 2);
        assert_eq!(state.last_error.as_deref(), Some("stopped"));
        state.note_action("type", "type --text hi", true, None, 30);
        assert_eq!(
            (
                state.consecutive_failures,
                state.last_ok,
                state.last_error.is_none()
            ),
            (0, Some(true), true)
        );
        assert_eq!(state.actions, 3);

        // The same click twice, and two looks that saw nothing: stuck.
        state.note_action("mouse-click", "mouse-click --x 5 --y 5", true, None, 40);
        state.note_look(false);
        assert!(!state.stuck, "one look is not a loop");
        state.note_action("mouse-click", "mouse-click --x 5 --y 5", true, None, 50);
        state.note_look(false);
        assert!(state.stuck, "the same action, twice, nothing changed");
        state.note_look(true);
        assert!(
            !state.stuck && state.unchanged_looks == 0,
            "a change clears it"
        );
        assert_eq!(
            fingerprint(&[
                "type".into(),
                "--text".into(),
                "hunter2".into(),
                "--json".into()
            ]),
            "type --text [7 chars]",
            "what was typed is kept as its length, as the step log keeps it"
        );
        assert_eq!(
            fingerprint(&[
                "set-value".into(),
                "--app".into(),
                "x".into(),
                "--value".into(),
                "hunter2".into()
            ]),
            "set-value --app x --value [7 chars]"
        );
        assert_eq!(
            fingerprint(&[
                "mouse-click".into(),
                "--x".into(),
                "1".into(),
                "--y".into(),
                "2".into()
            ]),
            "mouse-click --x 1 --y 2",
            "a place is not a secret"
        );
        let dir = tempfile::tempdir().unwrap();
        state.note_action(
            "type",
            &fingerprint(&["type".into(), "--text".into(), "hunter2".into()]),
            true,
            None,
            9,
        );
        state.write(dir.path());
        let written = std::fs::read_to_string(dir.path().join(STATE_FILE)).unwrap();
        assert!(
            !written.contains("hunter2"),
            "no typed secret in state.json: {written}"
        );
        let back: OperatorState = serde_json::from_str(&written).unwrap();
        assert_eq!(back.actions, state.actions);
    }
}
