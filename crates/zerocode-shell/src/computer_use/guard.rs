//! The one hand on the operator, on the window's side (docs/design/
//! computer-use-full-operator.md §1.3): a stop flag the CLI verbs read
//! before any action goes near the helper, and the road to the helper's own
//! switch that does not wait behind an in-flight request — a signal to its
//! pid. `resume` lifts both; `status` reads both.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;
use zerocode_core::computer_use_protocol::error_code;

use super::ComputerUseError;

/// The window's stop: why it refuses actions, if it does, and its revision —
/// a count that moves whenever a stop is made or lifted (t-59372), so a
/// report and a resume name the very stop they speak of.
struct Stop {
    reason: Option<String>,
    revision: u64,
    /// An agent's stop made while a helper request was in flight, so the
    /// window could not hear whether the person's chord came just before it
    /// (t-62469): the stop may be the person's. An agent's `resume` leaves
    /// it standing; the band's resume (the person) lifts it. An agent's stop
    /// over it keeps the mark (B1); only the band's resume and the person's
    /// chord taken in ([`adopt`]) clear it.
    unconfirmed: bool,
}

static STOPPED: Mutex<Stop> = Mutex::new(Stop {
    reason: None,
    revision: 0,
    unconfirmed: false,
});
static STOP_GENERATION: AtomicU64 = AtomicU64::new(0);

fn stopped() -> std::sync::MutexGuard<'static, Stop> {
    STOPPED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(super) fn stop_generation() -> u64 {
    STOP_GENERATION.load(Ordering::Acquire)
}
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
    stopped().reason.clone()
}

/// The stop and its revision, read together: a report never pairs one
/// moment's stop with another moment's revision.
#[must_use]
pub fn snapshot() -> (Option<String>, u64) {
    let held = stopped();
    (held.reason.clone(), held.revision)
}

/// The refusal an action gets while stopped: a stop the person made is
/// theirs to lift, and so is one they may have made (t-62469 S2) — the
/// band's resume lifts it; any other is lifted by `resume`.
#[must_use]
pub fn refusal(reason: &str) -> ComputerUseError {
    let lifted = if zerocode_core::computer_use::persons_stop(reason) {
        "the person stopped it — only they lift it; report where you were and wait".to_string()
    } else if unconfirmed_stop_stands() {
        "the helper was busy when this stop came, so the person may have stopped it — only they lift it, from the band's resume; report where you were and wait".to_string()
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
/// stopped: nothing is signalled. The person's chord takes the place of a
/// stop that is not theirs (t-62872), as their band's stop does: heard over
/// an agent's stop, the stop is the person's from then on, and reaches the
/// work browsers too. Anything else over a standing stop moves nothing, so a
/// helper that answers every `status` with its stop never moves the revision
/// the band's resume names. Answers whether the window's stop moved.
pub fn adopt(reason: &str) -> bool {
    let mut held = stopped();
    let takes =
        held.reason.is_none() || (!persons(held.reason.as_deref()) && persons(Some(reason)));
    if takes {
        STOP_GENERATION.fetch_add(1, Ordering::AcqRel);
        held.reason = Some(reason.to_string());
        held.revision += 1;
        held.unconfirmed = false;
    }
    takes
}

/// Whether the standing stop may be the person's though it carries an
/// agent's reason (t-62469): see [`Stop::unconfirmed`].
#[must_use]
pub fn unconfirmed_stop_stands() -> bool {
    let held = stopped();
    held.reason.is_some() && !persons(held.reason.as_deref()) && held.unconfirmed
}

/// Whether the person's stop stands in the window — the stop the work
/// browsers' door reads (t-62872), never asking the helper.
#[must_use]
pub fn persons_stop_stands() -> bool {
    persons(stopped().reason.as_deref())
}

/// Let `admit` in unless the person's stop stands, under that stop's own lock
/// (t-62872): a stop the person makes meanwhile waits for `admit` to end, so
/// nothing `admit` lets in comes after their stop. `None`: the person's stop
/// stands, and `admit` never ran. `admit` reads no stop of its own — it runs
/// under this one's lock.
pub fn unless_persons_stop<T>(admit: impl FnOnce() -> T) -> Option<T> {
    let held = stopped();
    let admitted = (!persons(held.reason.as_deref())).then(admit);
    drop(held);
    admitted
}

/// Stop now: the window refuses every action from here on, and the helper is
/// told by signal so an action already in flight ends at its next event.
/// Every stop is a new revision, even over one standing: a resume that named
/// the older one does not lift it. One that is not the person's, over the
/// person's, moves nothing (t-62872): the stop stays theirs, or an agent's
/// stop and then its resume would lift it.
pub fn stop(reason: &str) -> Value {
    stop_with(reason, stop_helper)
}

/// [`stop`] with the signal to the helper handed in (t-62872): an agent's
/// stop signals the helper it heard first, and a test's helper keeps a stop
/// of its own.
pub fn stop_with(reason: &str, signal: impl FnOnce() -> bool) -> Value {
    stop_hearing(reason, || true, signal)
}

/// [`stop_with`] with the helper heard after the window's stop stands and
/// before the signal (t-62469): the stop is up at once, so nothing the door
/// lets in meanwhile starts after it, and a helper slow to answer delays only
/// the signal. `hear` takes in the person's chord the helper holds
/// ([`adopt`]) and answers whether it could read the helper; when it could
/// not, the stop is marked unconfirmed — it may be the person's — until the
/// band's resume lifts it. A stop made over a marked one keeps the mark
/// though the helper is heard then (B1): the first stop's signal may have
/// overwritten the person's chord in the helper, which now says only
/// `signal`.
pub fn stop_hearing(
    reason: &str,
    hear: impl FnOnce() -> bool,
    signal: impl FnOnce() -> bool,
) -> Value {
    let (reason, revision, made_here, marked_before) = {
        let mut held = stopped();
        let made = !persons(held.reason.as_deref()) || persons(Some(reason));
        let marked_before = held.reason.is_some() && held.unconfirmed;
        if made {
            STOP_GENERATION.fetch_add(1, Ordering::AcqRel);
            held.reason = Some(reason.to_string());
            held.revision += 1;
            held.unconfirmed = marked_before || !persons(Some(reason));
        }
        let standing = held.reason.clone().unwrap_or_default();
        (standing, held.revision, made, marked_before)
    };
    // The helper is heard whenever the stop was made here — it may hold the
    // person's chord to take in — and only an unmarked stop is confirmed.
    let heard = made_here && hear();
    if heard && !marked_before {
        let mut held = stopped();
        if held.revision == revision {
            held.unconfirmed = false;
        }
    }
    let signalled = signal();
    let (reason, revision) = snapshot_reason(&reason, revision);
    serde_json::json!({
        "stopped": true,
        "personsStop": persons(Some(reason.as_str())),
        "maybePersonsStop": unconfirmed_stop_stands(),
        "reason": reason,
        "revision": revision,
        "helperSignalled": signalled,
    })
}

/// The stop as it stands after the helper was heard: the person's chord taken
/// in meanwhile has moved the reason and the revision.
fn snapshot_reason(made: &str, made_revision: u64) -> (String, u64) {
    let held = stopped();
    match &held.reason {
        Some(standing) if held.revision >= made_revision => (standing.clone(), held.revision),
        _ => (made.to_string(), made_revision),
    }
}

/// Tell the remembered helper to stop, by signal; false when there is none.
pub fn stop_helper() -> bool {
    let pid = *HELPER_PID
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    pid.is_some_and(signal_helper)
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

/// Whether a standing stop is the person's — what the band takes to the work
/// browsers too (t-62872); no stop is nobody's.
fn persons(standing: Option<&str>) -> bool {
    standing.is_some_and(zerocode_core::computer_use::persons_stop)
}

/// What the page's band paints: stopped or not and the stop's revision, the
/// count, the last verb and when — `active` says an action just happened —
/// and whether the stop is the person's.
#[must_use]
pub fn activity_report(just_now: Option<&str>) -> Value {
    let (count, last, at) = ACTIONS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let (standing, revision) = snapshot();
    serde_json::json!({
        "active": just_now.is_some(),
        "verb": just_now.map(str::to_string).or(last),
        "actions": count,
        "at": at,
        "personsStop": persons(standing.as_deref()),
        "stopped": standing,
        "revision": revision,
        "hotkey": zerocode_core::computer_use::COMPUTER_STOP_HOTKEY,
    })
}

/// Who asks to lift the stop: the person, from the window's band, naming the
/// revision of the stop the band showed; or an agent's `resume`, which never
/// lifts a stop the person made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lifter {
    Person { revision: Option<u64> },
    Agent,
}

/// What one `resume` came to: whether the operator runs again, the stop
/// standing after it and its revision, and what the helper answered when it
/// was asked — or why it was not, while another resume was under way.
#[derive(Debug)]
pub struct Resumed {
    pub lifted: bool,
    pub stopped: Option<String>,
    pub revision: u64,
    pub helper: Option<Result<Value, ComputerUseError>>,
}

impl Resumed {
    /// The window's answer to its band.
    #[must_use]
    pub fn answer(&self) -> Value {
        let helper = match &self.helper {
            Some(Ok(said)) => said.clone(),
            Some(Err(error)) => serde_json::json!({ "error": error.code }),
            None => Value::Null,
        };
        serde_json::json!({
            "resumed": self.lifted,
            "revision": self.revision,
            "personsStop": persons(self.stopped.as_deref()),
            "stopped": self.stopped,
            "helper": helper,
        })
    }

    /// An agent's `resume`, answered: what the helper said once the stop is
    /// lifted, else the helper's own failure or the stop that still stands.
    pub fn agent_answer(self) -> Result<Value, ComputerUseError> {
        match self.helper {
            Some(Err(error)) => Err(error),
            helper if self.lifted => Ok(serde_json::json!({
                "resumed": true,
                "helper": helper.and_then(Result::ok).unwrap_or(Value::Null),
            })),
            _ => Err(refusal(self.stopped.as_deref().unwrap_or_default())),
        }
    }
}

/// The helper's own `resume`, when a session stands.
pub fn helper_resume(reset: bool) -> Result<Value, ComputerUseError> {
    if super::session_stands() {
        super::call("resume", serde_json::json!({ "resetBudget": reset }))
    } else {
        Ok(Value::String("idle".into()))
    }
}

/// Held through one resume, from the check of the stop it names to stopping
/// the helper again: a second resume inside that span would wake the helper
/// the first is about to stop again, leaving the window running beside a
/// stopped helper. A stop never takes it.
static RESUMING: Mutex<()> = Mutex::new(());

/// One resume, the window's and an agent's alike (t-59372). The person's
/// names the revision of the stop the band showed and lifts only that one;
/// an agent's never lifts the person's stop. The helper is asked outside the
/// lock, and the stop is lifted only when the helper resumed and nothing
/// moved meanwhile: a helper that failed leaves the stop standing, and a stop
/// made while it resumed wins — the helper that late resume woke is stopped
/// again. One resume runs at a time (`RESUMING`): one asked meanwhile is
/// refused before it reaches the helper.
pub fn resume(
    lifter: Lifter,
    resume_helper: impl FnOnce() -> Result<Value, ComputerUseError>,
    stop_helper_again: impl FnOnce() -> bool,
) -> Resumed {
    let _resuming = match RESUMING.try_lock() {
        Ok(held) => held,
        Err(std::sync::TryLockError::Poisoned(held)) => held.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => {
            let (reason, revision) = snapshot();
            return Resumed {
                lifted: false,
                stopped: reason,
                revision,
                helper: Some(Err(ComputerUseError::new(
                    error_code::STOPPED,
                    "another resume is under way; ask again once it ends",
                ))),
            };
        }
    };
    let (reason, before) = snapshot();
    let named = match lifter {
        Lifter::Person { revision } => revision == Some(before),
        Lifter::Agent => {
            !reason
                .as_deref()
                .is_some_and(zerocode_core::computer_use::persons_stop)
                && !unconfirmed_stop_stands()
        }
    };
    if !named {
        return Resumed {
            lifted: false,
            stopped: reason,
            revision: before,
            helper: None,
        };
    }
    let helper = resume_helper();
    let (standing, revision) = {
        let mut held = stopped();
        if helper.is_ok() && held.revision == before && held.reason.take().is_some() {
            held.revision += 1;
            held.unconfirmed = false;
        }
        (held.reason.clone(), held.revision)
    };
    if helper.is_ok() && standing.is_some() {
        stop_helper_again();
    }
    Resumed {
        lifted: helper.is_ok() && standing.is_none(),
        stopped: standing,
        revision,
        helper: Some(helper),
    }
}

/// Lift the window's stop outright — a new revision when one stood. Tests
/// start from no stop this way; the window and agents lift one only through
/// [`resume`].
#[cfg(test)]
pub fn lift() {
    let mut held = stopped();
    if held.reason.take().is_some() {
        held.revision += 1;
    }
    held.unconfirmed = false;
}

#[cfg(test)]
mod tests {
    use super::*;
    use zerocode_core::computer_use::{
        STOP_REASON_HOTKEY, STOP_REASON_REQUEST, STOP_REASON_SIGNAL, STOP_REASON_WINDOW,
    };

    fn one_hand() -> std::sync::MutexGuard<'static, ()> {
        crate::tests::computer_desktop_wait::ONE_HAND
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn revision() -> Option<u64> {
        activity_report(None)["revision"].as_u64()
    }

    #[test]
    fn a_resume_naming_an_older_stop_neither_lifts_the_newer_one_nor_asks_the_helper() {
        let _hand = one_hand();
        remember_helper(None);
        lift();
        stop(STOP_REASON_WINDOW);
        let seen = revision();
        stop(STOP_REASON_HOTKEY);
        let newer = revision();
        let mut asked = 0;
        let resumed = resume(
            Lifter::Person { revision: seen },
            || {
                asked += 1;
                Ok(Value::from("idle"))
            },
            || true,
        );
        let standing = stopped_reason();
        let after = revision();
        lift();
        assert!(
            seen.is_some() && newer > seen,
            "every stop moves the revision: {seen:?} → {newer:?}"
        );
        assert_eq!(asked, 0, "a stale resume never reaches the helper");
        assert!(!resumed.lifted, "{resumed:?}");
        assert_eq!(standing.as_deref(), Some(STOP_REASON_HOTKEY));
        assert_eq!(after, newer, "nothing moved");
    }

    #[test]
    fn a_resume_without_a_revision_lifts_nothing_and_asks_no_helper() {
        let _hand = one_hand();
        remember_helper(None);
        lift();
        stop(STOP_REASON_WINDOW);
        let mut asked = 0;
        let resumed = resume(
            Lifter::Person { revision: None },
            || {
                asked += 1;
                Ok(Value::from("idle"))
            },
            || true,
        );
        let standing = stopped_reason();
        lift();
        assert_eq!(asked, 0, "no revision is no authority");
        assert!(!resumed.lifted, "{resumed:?}");
        assert_eq!(standing.as_deref(), Some(STOP_REASON_WINDOW));
    }

    #[test]
    fn a_helper_that_fails_to_resume_leaves_the_persons_stop_standing() {
        let _hand = one_hand();
        remember_helper(None);
        lift();
        stop(STOP_REASON_WINDOW);
        let seen = revision();
        let resumed = resume(
            Lifter::Person { revision: seen },
            || {
                Err(ComputerUseError::new(
                    "helper_unreachable",
                    "the helper did not answer",
                ))
            },
            || true,
        );
        let answer = resumed.answer();
        let standing = stopped_reason();
        let after = revision();
        lift();
        assert!(seen.is_some(), "the stop carries a revision");
        assert_eq!(answer["resumed"], false, "{answer}");
        assert_eq!(standing.as_deref(), Some(STOP_REASON_WINDOW));
        assert_eq!(after, seen, "nothing moved");
    }

    #[test]
    fn a_stop_made_while_the_helper_resumes_wins_and_the_helper_is_stopped_again() {
        let _hand = one_hand();
        remember_helper(None);
        lift();
        stop(STOP_REASON_WINDOW);
        let seen = revision();
        let mut stopped_again = 0;
        let resumed = resume(
            Lifter::Person { revision: seen },
            || {
                stop(STOP_REASON_HOTKEY);
                Ok(Value::from("resumed"))
            },
            || {
                stopped_again += 1;
                true
            },
        );
        let answer = resumed.answer();
        let standing = stopped_reason();
        lift();
        assert_eq!(answer["resumed"], false, "{answer}");
        assert_eq!(
            stopped_again, 1,
            "the helper the late resume woke is stopped again"
        );
        assert_eq!(standing.as_deref(), Some(STOP_REASON_HOTKEY));
    }

    /// One resume at a time (t-59372): the person's resume of a newer stop,
    /// asked while the older resume is still stopping the helper it woke, is
    /// refused before the helper — else that helper is woken and then stopped
    /// by the older resume, and the window runs beside a stopped helper — and
    /// lifts the newer stop once the older resume ends.
    #[test]
    fn a_resume_asked_while_another_is_under_way_never_reaches_the_helper_until_it_ends() {
        let _hand = one_hand();
        remember_helper(None);
        lift();
        stop(STOP_REASON_WINDOW);
        let seen = revision();
        let mut inner = None;
        let mut inner_asked = 0;
        let first = resume(
            Lifter::Person { revision: seen },
            || {
                stop(STOP_REASON_HOTKEY);
                Ok(Value::from("resumed"))
            },
            || {
                let newer = revision();
                inner = Some(resume(
                    Lifter::Person { revision: newer },
                    || {
                        inner_asked += 1;
                        Ok(Value::from("resumed"))
                    },
                    || true,
                ));
                true
            },
        );
        let standing = stopped_reason();
        let newer = revision();
        let mut retry_asked = 0;
        let retry = resume(
            Lifter::Person { revision: newer },
            || {
                retry_asked += 1;
                Ok(Value::from("resumed"))
            },
            || true,
        );
        let after = stopped_reason();
        lift();
        let inner = inner.expect("the first resume stops the helper it woke again");
        assert_eq!(
            inner_asked, 0,
            "a resume asked inside another never reaches the helper"
        );
        assert!(!inner.lifted, "{inner:?}");
        assert_eq!(
            inner.stopped.as_deref(),
            Some(STOP_REASON_HOTKEY),
            "{inner:?}"
        );
        assert!(!first.lifted, "{first:?}");
        assert_eq!(
            standing.as_deref(),
            Some(STOP_REASON_HOTKEY),
            "the newer stop stands once the first resume ends"
        );
        assert_eq!(retry_asked, 1, "asked again after, it reaches the helper");
        assert!(retry.lifted, "{retry:?}");
        assert_eq!(after, None);
    }

    #[test]
    fn an_agents_resume_never_lifts_the_persons_stop_nor_asks_the_helper() {
        let _hand = one_hand();
        remember_helper(None);
        lift();
        stop(STOP_REASON_WINDOW);
        let mut asked = 0;
        let resumed = resume(
            Lifter::Agent,
            || {
                asked += 1;
                Ok(Value::from("idle"))
            },
            || true,
        );
        let standing = stopped_reason();
        let said = resumed.agent_answer();
        lift();
        assert_eq!(asked, 0, "the person's stop is checked before the helper");
        assert_eq!(standing.as_deref(), Some(STOP_REASON_WINDOW));
        assert!(
            said.as_ref()
                .is_err_and(|error| error.message.contains("only they lift it")),
            "{said:?}"
        );
    }

    #[test]
    fn an_agents_resume_cannot_outlast_a_persons_stop_made_while_the_helper_resumes() {
        let _hand = one_hand();
        remember_helper(None);
        lift();
        stop(STOP_REASON_REQUEST);
        let mut stopped_again = 0;
        let resumed = resume(
            Lifter::Agent,
            || {
                stop(STOP_REASON_WINDOW);
                Ok(Value::from("resumed"))
            },
            || {
                stopped_again += 1;
                true
            },
        );
        let standing = stopped_reason();
        let said = resumed.agent_answer();
        lift();
        assert_eq!(standing.as_deref(), Some(STOP_REASON_WINDOW));
        assert_eq!(
            stopped_again, 1,
            "the helper the late resume woke is stopped again"
        );
        assert!(
            said.as_ref()
                .is_err_and(|error| error.message.contains("only they lift it")),
            "{said:?}"
        );
    }

    #[test]
    fn a_resume_naming_the_standing_stop_with_a_willing_helper_lifts_it() {
        let _hand = one_hand();
        remember_helper(None);
        lift();
        stop(STOP_REASON_WINDOW);
        let seen = revision();
        let mut asked = 0;
        let mut stopped_again = 0;
        let resumed = resume(
            Lifter::Person { revision: seen },
            || {
                asked += 1;
                Ok(Value::from("resumed"))
            },
            || {
                stopped_again += 1;
                true
            },
        );
        let standing = stopped_reason();
        let after = revision();
        let answer = resumed.answer();
        lift();
        assert!(resumed.lifted, "{resumed:?}");
        assert_eq!(asked, 1);
        assert_eq!(stopped_again, 0);
        assert_eq!(standing, None);
        assert!(
            seen.is_some() && after > seen,
            "a lift moves the revision: {seen:?} → {after:?}"
        );
        assert_eq!(answer["resumed"], true, "{answer}");
        assert_eq!(
            answer["revision"].as_u64(),
            after,
            "the answer names the revision it left: {answer}"
        );
        assert!(answer["stopped"].is_null(), "{answer}");
    }

    #[test]
    fn a_stop_answers_the_revision_its_report_carries() {
        let _hand = one_hand();
        remember_helper(None);
        lift();
        let answer = stop(STOP_REASON_WINDOW);
        let reported = revision();
        lift();
        assert!(reported.is_some(), "the report carries a revision");
        assert_eq!(answer["revision"].as_u64(), reported, "{answer}");
    }

    /// A report is one moment: its stop and its revision are read together,
    /// so a stop is never shown with the revision of the lift beside it.
    #[test]
    fn the_report_never_pairs_a_stop_with_another_moments_revision() {
        const TURNS: usize = 2_000;
        let _hand = one_hand();
        remember_helper(None);
        lift();
        let start = revision();
        let reader = std::thread::spawn(|| {
            (0..TURNS)
                .map(|_| {
                    let report = activity_report(None);
                    (report["stopped"].is_string(), report["revision"].as_u64())
                })
                .collect::<Vec<_>>()
        });
        for _ in 0..TURNS {
            stop(STOP_REASON_WINDOW);
            lift();
        }
        let seen = reader.join().expect("the reader finishes");
        lift();
        let start = start.expect("the report carries a revision");
        for (stopped, revision) in seen {
            let moved = revision.expect("every report carries a revision") - start;
            assert_eq!(
                stopped,
                moved % 2 == 1,
                "stopped {stopped} beside revision {revision:?}, counted from {start}"
            );
        }
    }

    /// An agent's `stop` over the person's stop (t-62872): the stop stays the
    /// person's and nothing moves, so the agent's `resume` after it reaches no
    /// helper and lifts nothing — else stop-then-resume would be the agent's
    /// own way past the person's hand.
    #[test]
    fn an_agents_stop_over_the_persons_stop_leaves_it_theirs_and_its_resume_lifts_nothing() {
        let _hand = one_hand();
        remember_helper(None);
        lift();
        stop(STOP_REASON_WINDOW);
        let persons = revision();
        let answer = stop(STOP_REASON_REQUEST);
        let standing = stopped_reason();
        let after_agents_stop = revision();
        let mut asked = 0;
        let resumed = resume(
            Lifter::Agent,
            || {
                asked += 1;
                Ok(Value::from("resumed"))
            },
            || true,
        );
        let after = stopped_reason();
        lift();
        assert_eq!(
            standing.as_deref(),
            Some(STOP_REASON_WINDOW),
            "the person's stop stands over the agent's: {answer}"
        );
        assert_eq!(after_agents_stop, persons, "nothing moved: {answer}");
        assert_eq!(answer["reason"], STOP_REASON_WINDOW, "{answer}");
        assert_eq!(answer["revision"].as_u64(), persons, "{answer}");
        assert_eq!(asked, 0, "the agent's resume never reaches the helper");
        assert!(!resumed.lifted, "{resumed:?}");
        assert_eq!(after.as_deref(), Some(STOP_REASON_WINDOW));
    }

    /// Whose stop stands is the backend's word (t-62872): the person's — the
    /// band's own or the desktop chord — reaches the work browsers too, an
    /// agent's does not. The report, the stop's answer and the resume's answer
    /// say it beside the revision they name.
    #[test]
    fn the_report_the_stop_and_the_resume_say_whether_the_standing_stop_is_the_persons() {
        let _hand = one_hand();
        remember_helper(None);
        lift();
        let unstopped = activity_report(None);
        let agents = stop(STOP_REASON_REQUEST);
        let agents_report = activity_report(None);
        lift();
        let persons = stop(STOP_REASON_WINDOW);
        let persons_report = activity_report(None);
        let refused = resume(
            Lifter::Person { revision: None },
            || Ok(Value::from("idle")),
            || true,
        )
        .answer();
        lift();
        adopt(STOP_REASON_HOTKEY);
        let chord_report = activity_report(None);
        lift();
        assert_eq!(unstopped["personsStop"], false, "{unstopped}");
        assert_eq!(agents["personsStop"], false, "{agents}");
        assert_eq!(agents_report["personsStop"], false, "{agents_report}");
        assert_eq!(persons["personsStop"], true, "{persons}");
        assert_eq!(persons_report["personsStop"], true, "{persons_report}");
        assert_eq!(refused["personsStop"], true, "{refused}");
        assert_eq!(chord_report["personsStop"], true, "{chord_report}");
    }

    /// The person's chord the helper heard over an agent's own stop
    /// (t-62872): taken in, it is the person's stop from then on — the band
    /// reads `personsStop` and stops the work browsers too — while a word of
    /// the helper's that is not the person's, or any word over the person's
    /// stop, moves nothing: a helper that answers every `status` with its
    /// stop never moves the revision the band's resume names.
    #[test]
    fn the_persons_chord_taken_in_over_an_agents_stop_is_the_persons_stop() {
        let _hand = one_hand();
        remember_helper(None);
        lift();
        stop(STOP_REASON_REQUEST);
        let agents = revision();
        let signal = adopt(STOP_REASON_SIGNAL);
        let after_signal = revision();
        let chord = adopt(STOP_REASON_HOTKEY);
        let report = activity_report(None);
        let again = adopt(STOP_REASON_HOTKEY);
        let window = adopt(STOP_REASON_WINDOW);
        let after_again = revision();
        lift();
        assert!(
            !signal && after_signal == agents,
            "a word of the helper's that is not the person's moves nothing: {agents:?} → {after_signal:?}"
        );
        assert!(
            chord,
            "the person's chord over an agent's stop is taken in: {report}"
        );
        assert_eq!(report["personsStop"], true, "{report}");
        assert_eq!(report["stopped"], STOP_REASON_HOTKEY, "{report}");
        assert!(
            report["revision"].as_u64() > agents,
            "taking it in moves the revision: {report}"
        );
        assert!(
            !again && !window,
            "over the person's stop nothing more is taken in"
        );
        assert_eq!(
            after_again,
            report["revision"].as_u64(),
            "nothing moved over the person's stop"
        );
    }

    /// A stop that may be the person's (t-62469 S2) is refused in words that
    /// name the band's resume: an agent told that its `resume` lifts it would
    /// only be refused again. An agent's stop the window heard the helper
    /// for is still lifted by `resume`.
    #[test]
    fn a_stop_the_person_may_have_made_is_refused_in_words_that_name_the_bands_resume() {
        let _hand = one_hand();
        remember_helper(None);
        lift();
        stop_hearing(STOP_REASON_REQUEST, || false, || true);
        let maybe = refusal(STOP_REASON_REQUEST).message;
        lift();
        stop_hearing(STOP_REASON_REQUEST, || true, || true);
        let heard = refusal(STOP_REASON_REQUEST).message;
        lift();
        assert!(
            maybe.contains("the person may have stopped it")
                && maybe.contains("the band's resume")
                && !maybe.contains("resume` lifts it"),
            "a stop the person may have made names the band's resume: {maybe}"
        );
        assert!(
            heard.contains("resume` lifts it"),
            "an agent's stop the window heard the helper for names `resume`: {heard}"
        );
    }

    /// An agent's stop over one the person may have made (t-62469 B1): the
    /// helper heard now says nothing of the person's — the first stop's
    /// signal overwrote their chord there — so the stop stays one they may
    /// have made. Only the band's resume lifts it; an agent's never does.
    #[test]
    fn an_agents_stop_heard_over_a_stop_the_person_may_have_made_keeps_it_theirs_maybe() {
        let _hand = one_hand();
        remember_helper(None);
        lift();
        stop_hearing(STOP_REASON_REQUEST, || false, || true);
        let again = stop_hearing(STOP_REASON_REQUEST, || true, || true);
        let maybe = unconfirmed_stop_stands();
        let mut asked = 0;
        let agents = resume(
            Lifter::Agent,
            || {
                asked += 1;
                Ok(Value::from("resumed"))
            },
            || true,
        );
        let (_, revision) = snapshot();
        let band = resume(
            Lifter::Person {
                revision: Some(revision),
            },
            || Ok(Value::from("resumed")),
            || true,
        );
        let after = stopped_reason();
        lift();
        assert!(
            maybe,
            "the stop stays one the person may have made after the agent's second stop: {again}"
        );
        assert_eq!(again["maybePersonsStop"], true, "{again}");
        assert_eq!(asked, 0, "the agent's resume never reaches the helper");
        assert!(!agents.lifted, "{agents:?}");
        assert!(band.lifted, "the band's resume lifts it: {band:?}");
        assert_eq!(after, None);
    }

    #[test]
    fn an_agents_resume_lifts_the_stop_an_agent_asked_for() {
        let _hand = one_hand();
        remember_helper(None);
        lift();
        stop(STOP_REASON_REQUEST);
        let resumed = resume(Lifter::Agent, || Ok(Value::from("idle")), || true);
        let standing = stopped_reason();
        let said = resumed.agent_answer();
        lift();
        assert_eq!(standing, None);
        assert_eq!(
            said.ok().map(|answer| answer["resumed"].clone()),
            Some(Value::Bool(true))
        );
    }

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
