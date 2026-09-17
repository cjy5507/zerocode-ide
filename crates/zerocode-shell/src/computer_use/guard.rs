//! The one hand on the operator, on the window's side (docs/design/
//! computer-use-full-operator.md §1.3): a stop flag the CLI verbs read
//! before any action goes near the helper, and the road to the helper's own
//! switch that does not wait behind an in-flight request — a signal to its
//! pid. `resume` lifts both; `status` reads both.

use std::sync::Mutex;

use serde_json::Value;
use zerocode_core::computer_use_protocol::error_code;

use super::ComputerUseError;

/// Why the window refuses actions, if it does.
static STOPPED: Mutex<Option<String>> = Mutex::new(None);
/// The helper's pid, remembered when a session stood, so a stop can reach it
/// without the session's lock.
static HELPER_PID: Mutex<Option<i32>> = Mutex::new(None);

/// Remember the helper a session stood with (or forget it, with `None`).
pub(super) fn remember_helper(pid: Option<i32>) {
    *HELPER_PID
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = pid;
}

/// The reason the window is stopped, if it is.
#[must_use]
pub fn stopped_reason() -> Option<String> {
    STOPPED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// The refusal an action gets while stopped: a stop the person made is
/// theirs to lift; any other is lifted by `resume`.
#[must_use]
pub fn refusal(reason: &str) -> ComputerUseError {
    let lifted = if zerocode_core::computer_use::persons_stop(reason) {
        "the person stopped it — only they lift it; report where you were and wait".to_string()
    } else {
        format!(
            "`{} resume` lifts it",
            zerocode_core::computer_use::COMPUTER_CLI
        )
    };
    ComputerUseError::new(
        error_code::STOPPED,
        format!("the operator is stopped ({reason}); {lifted}"),
    )
}

/// Take in a stop the helper made on its own — the person's chord heard on
/// the desktop — so the window refuses at its door too and its band shows the
/// stop and the resume the person lifts it with. The helper is already
/// stopped: nothing is signalled.
pub fn adopt(reason: &str) {
    let mut held = STOPPED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if held.is_none() {
        *held = Some(reason.to_string());
    }
}

/// Stop now: the window refuses every action from here on, and the helper is
/// told by signal so an action already in flight ends at its next event.
pub fn stop(reason: &str) -> Value {
    *STOPPED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(reason.to_string());
    let pid = *HELPER_PID
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let signalled = pid.is_some_and(signal_helper);
    serde_json::json!({ "stopped": true, "reason": reason, "helperSignalled": signalled })
}

#[cfg(unix)]
fn signal_helper(pid: i32) -> bool {
    // SAFETY: a plain signal to a pid this window launched and still holds a
    // socket to; a stale pid answers ESRCH, which is reported, not raised.
    unsafe { libc::kill(pid, libc::SIGUSR1) == 0 }
}

#[cfg(not(unix))]
fn signal_helper(_pid: i32) -> bool {
    false
}

/// How many desktop actions this window answered, and the last one — what
/// the page's band shows.
static ACTIONS: Mutex<(u64, Option<String>, i64)> = Mutex::new((0, None, 0));

/// Count one answered action (the verb, when it acted) at `at_epoch_ms`.
pub fn note_action(verb: &str, at_epoch_ms: i64) {
    let mut held = ACTIONS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    held.0 += 1;
    held.1 = Some(verb.to_string());
    held.2 = at_epoch_ms;
}

/// What the page's band paints: stopped or not, the count, the last verb
/// and when — `active` says an action just happened.
#[must_use]
pub fn activity_report(just_now: Option<&str>) -> Value {
    let (count, last, at) = ACTIONS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    serde_json::json!({
        "active": just_now.is_some(),
        "verb": just_now.map(str::to_string).or(last),
        "actions": count,
        "at": at,
        "stopped": stopped_reason(),
        "hotkey": zerocode_core::computer_use::COMPUTER_STOP_HOTKEY,
    })
}

/// Lift the window's stop; the helper's is lifted by the caller's `resume`
/// request when a session stands.
pub fn lift() {
    *STOPPED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stop_is_a_reason_until_lifted_and_a_missing_helper_is_said_not_raised() {
        let _hand = crate::tests::computer_desktop_wait::ONE_HAND
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        remember_helper(None);
        lift();
        assert_eq!(stopped_reason(), None);
        let answer = stop("test");
        assert_eq!(answer["stopped"], true);
        assert_eq!(answer["helperSignalled"], false, "no helper to signal");
        assert_eq!(stopped_reason().as_deref(), Some("test"));
        assert_eq!(refusal("test").code, error_code::STOPPED);
        lift();
        assert_eq!(stopped_reason(), None);
    }
}
