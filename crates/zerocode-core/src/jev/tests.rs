//! What the table promises every reader of it.

use serde_json::json;

use super::*;

#[test]
fn every_use_reads_its_own_words_in_any_case_and_anything_else_as_off() {
    for row in &JEV_USES {
        for mode in JevMode::ALL {
            let shouted = json!(format!("  {}  ", mode.key().to_ascii_uppercase()));
            let expected = if row.modes.contains(&mode) {
                mode
            } else {
                JevMode::Off
            };
            assert_eq!(row.mode_of(Some(&shouted)), expected, "{} {mode:?}", row.id);
        }
        for stranger in [
            json!("shadwo"),
            json!(""),
            json!(true),
            json!(1),
            json!(null),
        ] {
            assert_eq!(
                row.mode_of(Some(&stranger)),
                JevMode::Off,
                "{} {stranger}",
                row.id
            );
        }
        assert_eq!(row.mode_of(None), JevMode::Off, "{}", row.id);
    }
}

#[test]
fn every_use_is_off_until_a_person_says_otherwise() {
    assert_eq!(JevMode::default(), JevMode::Off);
    for row in &JEV_USES {
        assert_eq!(
            row.modes.first(),
            Some(&JevMode::Off),
            "{} lists off first",
            row.id
        );
        assert_eq!(row.mode_in(&json!({})), JevMode::Off, "{}", row.id);
        assert_eq!(
            row.mode_in(&json!({ SMART_SETTINGS_KEY: "fast" })),
            JevMode::Off,
            "a `smart` that is not an object says nothing about {}",
            row.id
        );
    }
    let set = json!({ SMART_SETTINGS_KEY: { ROUTING.setting: "shadow" } });
    assert_eq!(ROUTING.mode_in(&set), JevMode::Shadow);
    assert_eq!(
        RECALL.mode_in(&set),
        JevMode::Off,
        "one use's word is not another's"
    );
}

/// Recall has an apply stage now (t-4676): `on` reorders what a turn reads,
/// and `auto` still only records, because nothing promotes recall yet.
#[test]
fn recall_acts_only_when_a_person_says_on() {
    assert_eq!(RECALL.mode_of(Some(&json!("on"))), JevMode::On);
    assert!(RECALL.mode_of(Some(&json!("on"))).applies());
    assert_eq!(RECALL.mode_of(Some(&json!("auto"))), JevMode::Auto);
    assert!(!RECALL.mode_of(Some(&json!("auto"))).applies());
    assert!(
        !jev_use(RECALL.id).expect("the recall row").promotes,
        "only a person moves recall off recording"
    );
}

#[test]
fn auto_asks_and_records_but_acts_on_nothing_yet() {
    assert!(!JevMode::Off.asks() && !JevMode::Off.applies());
    assert!(JevMode::Shadow.asks() && !JevMode::Shadow.applies());
    assert!(JevMode::Auto.asks() && !JevMode::Auto.applies());
    assert!(JevMode::On.asks() && JevMode::On.applies());
}

/// Promotion rises from `auto` to acting, so a use that promotes offers both.
#[test]
fn a_use_that_promotes_has_somewhere_to_rise_from_and_to() {
    for row in JEV_USES.iter().filter(|row| row.promotes) {
        assert!(row.modes.contains(&JevMode::Auto), "{}", row.id);
        assert!(row.modes.iter().any(|mode| mode.applies()), "{}", row.id);
    }
}

#[test]
fn uses_are_told_apart_by_every_name_they_answer_to() {
    for (index, row) in JEV_USES.iter().enumerate() {
        for other in &JEV_USES[index + 1..] {
            assert_ne!(row.id, other.id);
            assert_ne!(row.setting, other.setting);
            assert_ne!(row.ledger, other.ledger);
        }
        assert_eq!(jev_use(row.id), Some(row));
        let mut words: Vec<&str> = row.modes.iter().map(|mode| mode.key()).collect();
        words.sort_unstable();
        words.dedup();
        assert_eq!(
            words.len(),
            row.modes.len(),
            "{} lists a mode twice",
            row.id
        );
    }
    assert_eq!(jev_use("notify"), None);
}

#[test]
fn a_writer_takes_exactly_the_words_a_use_offers() {
    assert_eq!(ROUTING.offered("shadow"), Some(JevMode::Shadow));
    assert_eq!(ROUTING.offered("auto"), Some(JevMode::Auto));
    assert_eq!(ROUTING.offered("Shadow"), None, "a writer spells the word");
    assert_eq!(RECALL.offered("on"), Some(JevMode::On));
    assert_eq!(BROWSER.offered("actual"), None);
}

/// The caps the question builders cut at are the table's own numbers.
#[test]
fn the_builders_cut_at_the_tables_caps() {
    assert_eq!(
        crate::browser_action::MAX_ACTION_CANDIDATES,
        BROWSER_CANDIDATE_CAP
    );
    let caps = |row: &JevUse| -> Vec<Cap> { row.sends.iter().map(|sent| sent.cap).collect() };
    assert!(caps(&ROUTING).contains(&Cap::Chars(ROUTING_TASK_CHAR_CAP)));
    assert!(caps(&RECALL).contains(&Cap::Chars(RECALL_REQUEST_CHAR_CAP)));
    assert!(caps(&RECALL).contains(&Cap::Items(RECALL_NOTE_CAP)));
    assert!(caps(&RECALL).contains(&Cap::Bytes(RECALL_SUMMARY_BYTE_CAP)));
    assert!(caps(&BROWSER).contains(&Cap::Items(BROWSER_CANDIDATE_CAP)));
    assert!(caps(&STALL).contains(&Cap::Bytes(STALL_SCREEN_BYTE_CAP)));
    assert!(caps(&STALL).contains(&Cap::Bytes(STALL_TRANSCRIPT_BYTE_CAP)));
    assert!(caps(&PLACEMENT).contains(&Cap::Chars(PLACEMENT_BRIEF_CHAR_CAP)));
}

/// A stall's answer is a row beside what the coordinator did, never an act:
/// the use offers nothing that applies, and so nothing it could rise to.
#[test]
fn a_stall_question_offers_nothing_that_acts() {
    assert!(STALL.modes.iter().all(|mode| !mode.applies()));
    const { assert!(!STALL.promotes) };
    assert_eq!(STALL.mode_of(Some(&json!("on"))), JevMode::Off);
    assert_eq!(STALL.mode_of(Some(&json!("auto"))), JevMode::Auto);
    assert_eq!(jev_use("stall"), Some(&STALL));
}

/// Where a worker the window just started should stand is a judgment about
/// what the person is doing right now, not a fact the layout can be asked
/// for: the measured rule (`tilePlacement`, `ui/shell-term.js`) reads room
/// and worktree, and neither of those says whether this worker is the one
/// somebody wants to watch. So it is a row here — and a recording one, for
/// now. The rule still places every worker; this only writes down what Jev
/// would have chosen beside what the rule chose and what the person then did
/// with it.
#[test]
fn a_placement_question_offers_nothing_that_acts() {
    assert!(PLACEMENT.modes.iter().all(|mode| !mode.applies()));
    const { assert!(!PLACEMENT.promotes) };
    assert_eq!(PLACEMENT.mode_of(Some(&json!("on"))), JevMode::Off);
    assert_eq!(PLACEMENT.mode_of(Some(&json!("auto"))), JevMode::Auto);
    assert_eq!(jev_use("placement"), Some(&PLACEMENT));
}

/// The three places a worker can be put, spelled once. A fourth option would
/// be a rubric change, and the fingerprint is what makes that a red test
/// rather than a quiet drift.
#[test]
fn the_placement_options_are_the_three_the_window_can_actually_do() {
    assert_eq!(
        PLACEMENT_OPTIONS,
        ["tab", "split", "background"],
        "the window has exactly these roads: a tab of its own, a division of \
         what is in front, or no stage at all"
    );
}
