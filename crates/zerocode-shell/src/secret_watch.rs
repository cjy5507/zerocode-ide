//! Tells the window when a pane starts or stops waiting for a secret (herdr 3,
//! t-26596), without the window asking.
//!
//! A thread looks at the panes every [`SECRET_WATCH_TICK`]. A pane is read only
//! once its output has stopped for the answer door's quiet spell, and only once
//! for each stretch of output, so an idle window reads nothing and sends
//! nothing. When a pane's question appears, or leaves, the window hears it as a
//! `term:secret` event; the window never has to call the backend to find out.

use super::*;

use std::collections::HashMap;
use std::time::Duration;

use serde::Serialize;

use crate::answer_door;
use zerocode_core::secret_prompt::SecretKind;

/// How often the watcher looks at the panes. Short enough that a question is
/// said within a fraction of a second of its pane going quiet; each look reads
/// nothing for a pane that has not printed since its last read.
pub(crate) const SECRET_WATCH_TICK: Duration = Duration::from_millis(150);

/// What the watcher remembers about one pane between two looks.
#[derive(Debug, Default)]
pub(crate) struct PaneWatch {
    /// The output time the last look saw, so a pane printing again is new.
    output_at: Option<i64>,
    /// Whether this stretch of output has already been read.
    read: bool,
    /// The question the window was last told about, if it was told one.
    said: Option<SecretQuestion>,
}

/// One question as the window hears it: the kind, the question's line, and the
/// output time it was read at. The value is in none of these.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SecretQuestion {
    pub(crate) kind: SecretKind,
    pub(crate) line: String,
    pub(crate) since: i64,
}

/// What the watcher tells the window about one pane this look.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Said {
    Nothing,
    Question(SecretQuestion),
    Gone,
}

/// What the window hears on `term:secret`: a pane's question, or `null` when the
/// pane no longer asks one.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SecretNotice {
    pub(crate) term: TermId,
    pub(crate) question: Option<SecretQuestion>,
}

/// Decide what one look at a pane says. `output_at` is the pane's last output
/// time, `None` when its transport does not say. `read` reads the pane's
/// question, and runs at most once for each stretch of output, and only once the
/// pane has been quiet for the spell.
pub(crate) fn judge(
    watch: &mut PaneWatch,
    output_at: Option<i64>,
    now_ms: i64,
    read: impl FnOnce() -> Option<SecretQuestion>,
) -> Said {
    if watch.output_at != output_at {
        watch.output_at = output_at;
        watch.read = false;
    }
    // A transport that does not say when it last wrote cannot be waited on, so
    // its stretch is read at once, and only once.
    let quiet = output_at.is_none_or(|at| now_ms.saturating_sub(at) >= quiet_ms());
    if watch.read || !quiet {
        return Said::Nothing;
    }
    watch.read = true;
    match read() {
        Some(question) if watch.said.as_ref() == Some(&question) => Said::Nothing,
        Some(question) => {
            watch.said = Some(question.clone());
            Said::Question(question)
        }
        None => match watch.said.take() {
            Some(_) => Said::Gone,
            None => Said::Nothing,
        },
    }
}

/// The quiet spell a pane must keep before its question is read.
fn quiet_ms() -> i64 {
    i64::try_from(answer_door::SECRET_QUIET.as_millis()).unwrap_or(i64::MAX)
}

/// Start the watcher: one thread for the whole process, one look at every pane
/// each [`SECRET_WATCH_TICK`].
pub(crate) fn spawn(app: AppHandle) {
    let _ = std::thread::Builder::new()
        .name("secret-watch".to_string())
        .spawn(move || {
            let mut watches: HashMap<TermId, PaneWatch> = HashMap::new();
            loop {
                std::thread::sleep(SECRET_WATCH_TICK);
                look_once(&app, &mut watches);
            }
        });
}

/// One look at every pane this process holds, and the changes said on.
fn look_once(app: &AppHandle, watches: &mut HashMap<TermId, PaneWatch>) {
    let state = app.state::<AppState>();
    let living = state.terminals().terms();
    watches.retain(|term, _| living.contains(term));
    for term in living {
        let Some(held) = state.terminals().handle(term) else {
            continue;
        };
        // A pane being parsed this instant is looked at on the next look.
        let Some(output_at) = answer_door::output_clock(&held) else {
            continue;
        };
        // Read before the pane's lock is taken: the agent map is never held inside it.
        let agent_pane = state.agent_terms().contains_key(&term);
        let said = judge(
            watches.entry(term).or_default(),
            output_at,
            epoch_ms_now(),
            || {
                // A question is said only where a typed value may reach it.
                let prompt = answer_door::secret_prompt(&held, agent_pane)?;
                Some(SecretQuestion {
                    kind: prompt.kind,
                    line: prompt.line,
                    since: answer_door::quiet_since(&held),
                })
            },
        );
        let question = match said {
            Said::Nothing => continue,
            Said::Question(question) => Some(question),
            Said::Gone => None,
        };
        let _ = app.emit("term:secret", SecretNotice { term, question });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn question(line: &str, since: i64) -> SecretQuestion {
        SecretQuestion {
            kind: SecretKind::Password,
            line: line.to_string(),
            since,
        }
    }

    #[test]
    fn a_question_is_said_once_its_pane_has_been_quiet() {
        let mut watch = PaneWatch::default();
        let printed = 1_000;
        assert_eq!(
            judge(&mut watch, Some(printed), printed + 10, || Some(question(
                "Password:",
                printed
            ))),
            Said::Nothing,
            "a pane that printed a moment ago is not read yet"
        );
        let quiet_at = printed + quiet_ms();
        assert_eq!(
            judge(&mut watch, Some(printed), quiet_at, || Some(question(
                "Password:",
                printed
            ))),
            Said::Question(question("Password:", printed)),
            "a pane quiet for the spell is read and its question said"
        );
        assert_eq!(
            judge(&mut watch, Some(printed), quiet_at + 150, || Some(
                question("Password:", printed)
            )),
            Said::Nothing,
            "the same question is said once, not on every look"
        );
    }

    #[test]
    fn a_pane_still_printing_is_not_read() {
        let mut watch = PaneWatch::default();
        let mut reads = 0;
        for tick in 0..10 {
            let printed = 1_000 + tick * 50;
            judge(&mut watch, Some(printed), printed + 10, || {
                reads += 1;
                None
            });
        }
        assert_eq!(reads, 0, "no look reads a pane that keeps printing");
    }

    #[test]
    fn a_question_that_leaves_is_said_gone_once() {
        let mut watch = PaneWatch::default();
        let quiet_at = 1_000 + quiet_ms();
        judge(&mut watch, Some(1_000), quiet_at, || {
            Some(question("Password:", 1_000))
        });
        assert_eq!(
            judge(&mut watch, Some(2_000), 2_000 + quiet_ms(), || None),
            Said::Gone,
            "the pane printed again and asks nothing now"
        );
        assert_eq!(
            judge(&mut watch, Some(2_000), 2_000 + quiet_ms() + 150, || None),
            Said::Nothing,
            "gone is said once"
        );
    }
}
