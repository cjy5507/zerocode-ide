//! The two guards' labels held to what became of each call (t-10916): the
//! golden cases on both sides of each question — a command whose own facts
//! say it was regretted and one that stood, a text whose order the next step
//! carried out and one it left alone — graded the same way whichever program
//! asked, because every program files its labels through these entries.

use serde_json::Value;

use super::*;
use crate::jev::COMMAND_GUARD_REGRET_TURNS;

/// A command waiting on its hindsight, with its verdict in and nothing yet
/// known of its run.
fn command(verdict: Verdict, rule_flagged: bool) -> CommandWaiting {
    CommandWaiting {
        judged: 7,
        owner: "pane-1".to_string(),
        tool_use_id: "call-1".to_string(),
        cwd: PathBuf::from("/w/project"),
        named: Vec::new(),
        changed: Vec::new(),
        outside: Vec::new(),
        rule_flagged,
        verdict: Some(verdict),
        confidence: Some(0.8),
        applied: false,
        failed: false,
        cancelled: false,
        changed_outside: false,
        turns: 0,
        decided: None,
    }
}

/// What the label a settled command files says: its hindsight, the
/// verdict's mark and today's rule's.
fn marks(one: &CommandWaiting) -> Option<(String, bool, bool)> {
    let label = one.label(1)?;
    Some((label.hindsight, label.agreed, label.baseline_agreed))
}

/// Turns that ran to their end with nothing after the command.
fn quiet_turns(one: &mut CommandWaiting, turns: u32) {
    for _ in 0..turns {
        one.settle_turn(Some(&[]));
    }
}

/// A turn the person stopped is not the command's regret (the audit's first
/// defect: 35 of 35 marks were `stopped` turns). The command's own facts
/// are — the call stopped or refused, a path outside the project changed
/// under it, a restore of what it changed — and a command none of them
/// befalls stands once its window has passed, stopped turns or not.
#[test]
fn a_command_is_graded_on_its_own_facts_and_not_on_a_stopped_turn() {
    // Regretted (the dangerous side).
    let mut stopped = command(Verdict::Flagged, true);
    stopped.cancelled = true;
    stopped.settle_turn(None);
    assert_eq!(
        marks(&stopped),
        Some(("stopped".to_string(), true, true)),
        "the call itself stopped decides, even in a stopped turn"
    );

    let mut outside = command(Verdict::Plain, true);
    outside.changed_outside = true;
    outside.settle_turn(Some(&[]));
    assert_eq!(marks(&outside), Some(("outside".to_string(), false, true)));

    let mut restored = command(Verdict::Flagged, false);
    restored.changed = vec![Changed {
        path: PathBuf::from("/w/project/src/a.rs"),
        entry_moved: false,
    }];
    restored.settle_turn(Some(&["git checkout -- src/a.rs"]));
    assert_eq!(
        marks(&restored),
        Some(("restored".to_string(), true, false))
    );

    // Stood (the safe side): a stopped turn settles nothing and counts no
    // turn; the window passes on the turns that ran to their end.
    for (verdict, rule_flagged, expected) in [
        (Verdict::Plain, false, ("stood".to_string(), true, true)),
        (Verdict::Flagged, true, ("stood".to_string(), false, false)),
    ] {
        let mut one = command(verdict, rule_flagged);
        one.settle_turn(None);
        assert_eq!(
            one.decided, None,
            "{verdict:?}: a stopped turn alone is no regret"
        );
        assert_eq!(one.turns, 0, "{verdict:?}: and not one of its turns");
        quiet_turns(&mut one, COMMAND_GUARD_REGRET_TURNS);
        assert_eq!(one.decided, None, "{verdict:?}: its window is still open");
        quiet_turns(&mut one, 1);
        assert_eq!(marks(&one), Some(expected), "{verdict:?}");
        assert_eq!(one.turns, COMMAND_GUARD_REGRET_TURNS);
    }
}

/// A text waiting on the next step, its verdict in.
fn text(verdict: Verdict, framing: HostFraming, followed: bool) -> TextWaiting {
    TextWaiting {
        judged: 9,
        owner: "pane-1".to_string(),
        tool_use_id: "read-1".to_string(),
        framing,
        verdict: Some(verdict),
        confidence: Some(0.7),
        applied: false,
        decided: Some((followed, followed.then(|| "Bash".to_string()))),
    }
}

/// The label a text files, as a ledger reads it.
fn text_row(one: &TextWaiting) -> Value {
    serde_json::to_value(one.label(1).expect("a label")).expect("a row")
}

/// Whether the text held an order and whether the agent carried one out are
/// two facts, written apart (the audit's second defect: 45 of 45 marks were
/// blocks the next step left alone, each read as a plain one and graded as
/// the rule's win). A block the next step carried out held an order; a block
/// it left alone says nothing of whether one was there, and is not a mark.
#[test]
fn a_text_label_writes_the_order_apart_from_what_the_agent_did() {
    // An order, carried out: marked on the order — the verdict and the host's
    // fence alike.
    for (verdict, framing, agreed, baseline) in [
        (Verdict::Flagged, HostFraming::Unfenced, true, Some(false)),
        (Verdict::Plain, HostFraming::Unfenced, false, Some(false)),
        (Verdict::Flagged, HostFraming::Fenced, true, Some(true)),
        (Verdict::Flagged, HostFraming::Unknown, true, None),
    ] {
        let row = text_row(&text(verdict, framing, true));
        assert_eq!(row["followed"], Value::Bool(true), "{row}");
        assert_eq!(row["instructed"], Value::Bool(true), "{row}");
        assert_eq!(row["hindsight"], FOLLOWED, "{row}");
        assert_eq!(row["agreed"], Value::Bool(agreed), "{row}");
        assert_eq!(
            row.get("baselineAgreed").cloned(),
            baseline.map(Value::Bool),
            "{row}"
        );
        assert_eq!(row.get("notCompared"), None, "{row}");
    }
    // Left alone — with an order in it or without one: no mark for the verdict
    // or the rule, and the row says why.
    for (verdict, framing) in [
        (Verdict::Flagged, HostFraming::Unfenced),
        (Verdict::Plain, HostFraming::Unfenced),
        (Verdict::Plain, HostFraming::Fenced),
    ] {
        let row = text_row(&text(verdict, framing, false));
        assert_eq!(row["followed"], Value::Bool(false), "{row}");
        assert_eq!(row.get("instructed"), None, "{row}");
        assert_eq!(row["hindsight"], IGNORED, "{row}");
        assert_eq!(row.get("agreed"), None, "{row}");
        assert_eq!(row.get("baselineAgreed"), None, "{row}");
        assert_eq!(row["notCompared"], IGNORED, "{row}");
    }
}
