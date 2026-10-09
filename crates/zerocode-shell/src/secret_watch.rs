//! Tells the window when a pane starts or stops waiting for a secret (herdr 3,
//! t-26596), without the window asking.
//!
//! A thread looks at the panes every [`SECRET_WATCH_TICK`]. A pane is read only
//! once its output has stopped for the answer door's quiet spell, and only once
//! for each stretch of output, so an idle window reads nothing and sends
//! nothing. When a pane's question appears, or leaves, the window hears it as a
//! `term:secret` event; the window never has to call the backend to find out.
//!
//! Red stage: [`judge`] says nothing yet, so the tests below fail.

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

/// Decide what one look at a pane says. Red stage: says nothing.
pub(crate) fn judge(
    watch: &mut PaneWatch,
    output_at: Option<i64>,
    now_ms: i64,
    read: impl FnOnce() -> Option<SecretQuestion>,
) -> Said {
    let _ = (watch, output_at, now_ms, read);
    Said::Nothing
}

/// The quiet spell a pane must keep before its question is read.
fn quiet_ms() -> i64 {
    i64::try_from(answer_door::SECRET_QUIET.as_millis()).unwrap_or(i64::MAX)
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
