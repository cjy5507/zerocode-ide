//! The ears' wait (docs/design/computer-use-full-operator.md §7.1): the
//! window asks the helper what it heard, the way `wait-for` looks, until a
//! sound it waits for is heard or the table's budget passes. The helper
//! answers one request at a time, so a wait held inside it would hold every
//! click behind it — the window polls instead, at the table's pace.

use std::time::{Duration, Instant};

use serde_json::{Value, json};
use zerocode_core::computer_use::{
    SOUND_MIN_CONFIDENCE, SOUND_WAIT_POLL_MS, desktop_wait_for_ms, sound_event_matches,
    sound_wait_labels,
};
use zerocode_core::computer_use_protocol::error_code;

use super::ComputerUseError;

/// Wait for a sound. `heard(after)` answers what the helper heard after the
/// event numbered `after` (`soundRead`); the wait starts from the newest
/// event unless the caller names its own cursor (`--after`), so a sound heard
/// before the wait began does not end it.
pub fn sound_wait(
    params: &Value,
    heard: impl Fn(u64) -> Result<Value, ComputerUseError>,
) -> Result<Value, ComputerUseError> {
    let labels = sound_wait_labels(params.get("label").and_then(Value::as_str));
    let min_confidence = params
        .get("minConfidence")
        .and_then(Value::as_f64)
        .unwrap_or(SOUND_MIN_CONFIDENCE);
    let (budget_ms, capped) = desktop_wait_for_ms(params.get("timeoutMs").and_then(Value::as_u64));
    let started = Instant::now();
    let budget = Duration::from_millis(budget_ms);
    let poll = Duration::from_millis(SOUND_WAIT_POLL_MS);
    let mut after = match params.get("after").and_then(Value::as_u64) {
        Some(after) => after,
        None => heard(0)?.get("latest").and_then(Value::as_u64).unwrap_or(0),
    };
    let mut polls = 0_u32;
    loop {
        polls += 1;
        let answer = heard(after)?;
        for event in answer
            .get("events")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(seq) = event.get("seq").and_then(Value::as_u64) {
                after = after.max(seq);
            }
            if sound_event_matches(event, &labels, min_confidence) {
                return Ok(json!({
                    "satisfied": true,
                    "event": event,
                    "after": after,
                    "polls": polls,
                    "elapsedMs": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                    "budgetMs": budget_ms,
                    "budgetCapped": capped,
                }));
            }
        }
        if answer.get("listening").and_then(Value::as_bool) == Some(false) {
            let why = answer
                .get("failure")
                .and_then(Value::as_str)
                .unwrap_or("the listener stopped");
            return Err(ComputerUseError::new(error_code::NOT_LISTENING, why));
        }
        if started.elapsed() + poll >= budget {
            return Err(ComputerUseError::new(
                error_code::TIMEOUT,
                format!(
                    "heard nothing waited for in {} ms ({polls} asks, last event {after})",
                    started.elapsed().as_millis()
                ),
            ));
        }
        std::thread::sleep(poll);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A helper that answers from a script: each ask takes the next answer.
    fn scripted(answers: Vec<Value>) -> impl Fn(u64) -> Result<Value, ComputerUseError> {
        let answers = RefCell::new(answers.into_iter());
        let asked = RefCell::new(Vec::new());
        move |after| {
            asked.borrow_mut().push(after);
            Ok(answers
                .borrow_mut()
                .next()
                .unwrap_or_else(|| json!({ "listening": true, "latest": after, "events": [] })))
        }
    }

    fn heard(seq: u64, label: &str, confidence: f64) -> Value {
        json!({ "seq": seq, "label": label, "confidence": confidence, "at": 0 })
    }

    #[test]
    fn a_wait_starts_after_what_was_already_heard_and_ends_on_the_sound_named() {
        let helper = scripted(vec![
            // Before the wait: a siren already heard does not end it.
            json!({ "listening": true, "latest": 4, "events": [heard(4, "siren", 0.9)] }),
            json!({ "listening": true, "latest": 5, "events": [heard(5, "speech", 0.9)] }),
            json!({ "listening": true, "latest": 7, "events": [heard(6, "siren", 0.3), heard(7, "siren", 0.95)] }),
        ]);
        let answer =
            sound_wait(&json!({ "label": "Siren", "timeoutMs": 5000 }), helper).expect("heard");
        assert_eq!(
            answer["event"]["seq"], 7,
            "not the siren before the wait, not the unsure one"
        );
        assert_eq!(answer["after"], 7);
        assert_eq!(answer["polls"], 2);
    }

    #[test]
    fn a_wait_without_a_label_ends_on_any_sound_and_honours_a_cursor() {
        let helper = scripted(vec![
            json!({ "listening": true, "latest": 3, "events": [heard(3, "knock", 0.6)] }),
        ]);
        let answer = sound_wait(&json!({ "after": 2 }), helper).expect("heard");
        assert_eq!(answer["event"]["label"], "knock");
    }

    #[test]
    fn a_stopped_listener_and_a_spent_budget_are_said_by_code() {
        let stopped = scripted(vec![
            json!({ "listening": true, "latest": 0, "events": [] }),
            json!({ "listening": false, "latest": 0, "events": [], "failure": "the sound stream stopped" }),
        ]);
        let error = sound_wait(&json!({}), stopped).expect_err("not listening");
        assert_eq!(error.code, error_code::NOT_LISTENING);
        assert!(error.message.contains("stream stopped"));
        let quiet = scripted(vec![]);
        let error = sound_wait(&json!({ "timeoutMs": 1 }), quiet).expect_err("nothing heard");
        assert_eq!(error.code, error_code::TIMEOUT);
    }
}
