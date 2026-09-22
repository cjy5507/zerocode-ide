//! What the table promises every reader of it.

use serde_json::json;

use super::*;
use crate::jev::summary;

#[test]
fn screen_press_confidence_is_distinct_from_the_promotion_answer_rate() {
    for seat in [&BROWSER, &DESKTOP, &EMULATOR] {
        let floor = seat.press_floor_permille.unwrap();
        assert!(floor <= 1_000);
        let boundary = f64::from(floor) / 1_000.0;
        for (confidence, permitted) in [
            (boundary, true),
            (boundary - f64::EPSILON, false),
            (0.29, false),
            (1.0, true),
            (1.01, false),
            (f64::NAN, false),
            (f64::INFINITY, false),
            (-0.1, false),
        ] {
            assert_eq!(
                seat.permits_press(confidence),
                permitted,
                "{} {confidence}",
                seat.id
            );
        }
    }
    assert!(!ROUTING.permits_press(1.0));
}

#[test]
fn mobile_screen_judgment_has_its_own_seat_and_consent_setting() {
    let mobile = JEV_USES
        .iter()
        .find(|row| row.id == "emulator")
        .expect("mobile seat");
    assert_ne!(mobile.setting, BROWSER.setting);
    assert_ne!(mobile.setting, DESKTOP.setting);
    assert_ne!(mobile.ledger, BROWSER.ledger);
    assert_ne!(mobile.ledger, DESKTOP.ledger);
    assert_eq!(
        mobile.mode_in(&json!({"smart": {"browserAction": "on", "desktopAction": "on"}})),
        JevMode::Off
    );
}

/// The three screen seats' `auto` rises, on the lines
/// docs/design/jev-seats-accuracy-wave-20260921.md §4 wrote down — pinned
/// here because they are what a walk presses on, and a number that drifted
/// out of the design would move that with nobody reading it.
#[test]
fn every_screen_seat_rises_on_the_lines_the_wave_wrote_down() {
    const {
        assert!(BROWSER.promotes && DESKTOP.promotes && EMULATOR.promotes);
    }
    for seat in [&BROWSER, &DESKTOP, &EMULATOR] {
        assert_eq!(
            seat.answer_floor_permille,
            Some(SCREEN_ANSWER_FLOOR_PERMILLE),
            "{}",
            seat.id
        );
        assert_eq!(
            seat.agreement_floor_permille,
            Some(SCREEN_AGREEMENT_FLOOR_PERMILLE),
            "{}",
            seat.id
        );
        assert_eq!(
            seat.apply_deadline_ms,
            Some(SCREEN_APPLY_DEADLINE_MS),
            "{}",
            seat.id
        );
        // Pressing is still the press floor's to allow, whoever raised the
        // seat: a promoted `auto` presses at the same confidence `on` does.
        assert_eq!(
            seat.press_floor_permille,
            Some(SCREEN_PRESS_FLOOR_PERMILLE),
            "{}",
            seat.id
        );
    }
    assert_eq!(SCREEN_ANSWER_FLOOR_PERMILLE, 900);
    assert_eq!(SCREEN_AGREEMENT_FLOOR_PERMILLE, 800);
    assert_eq!(SCREEN_APPLY_DEADLINE_MS, 1_500);
    // Nine in ten is a line thirty-five walks can clear — the routing seat's
    // 73 is its own floor's window, not this one's — and the agreement is
    // read over the judgment's own cadence of twenty comparisons.
    assert_eq!(
        summary::rows_that_can_clear(SCREEN_ANSWER_FLOOR_PERMILLE),
        35
    );
    assert!(summary::rows_that_can_clear(SCREEN_AGREEMENT_FLOOR_PERMILLE) < 35);
}

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

/// Recall has an apply stage (t-4676): `on` reorders what a turn reads. Its
/// `auto` records until the judge raises it on the seat's own labels
/// (t-5806), on the skill seat's lines read from there.
#[test]
fn recall_acts_when_a_person_says_on_or_its_labels_raised_it() {
    assert_eq!(RECALL.mode_of(Some(&json!("on"))), JevMode::On);
    assert!(RECALL.mode_of(Some(&json!("on"))).applies());
    assert_eq!(RECALL.mode_of(Some(&json!("auto"))), JevMode::Auto);
    assert!(!RECALL.mode_of(Some(&json!("auto"))).applies());
    assert!(RECALL.mode_of(Some(&json!("auto"))).applies_with(true));
    let row = jev_use(RECALL.id).expect("the recall row");
    assert!(row.promotes, "recall rises on its labels now");
    assert_eq!(row.answer_floor_permille, Some(SKILL_ANSWER_FLOOR_PERMILLE));
    assert_eq!(
        row.agreement_floor_permille,
        Some(SKILL_AGREEMENT_FLOOR_PERMILLE)
    );
    assert_eq!(
        row.apply_deadline_ms,
        Some(ROUTING_APPLY_DEADLINE_MS),
        "the judge times recall against the wall its apply road waits inside"
    );
}

/// Read with no judgment to hand, `auto` records; read with one, it is
/// whatever the judge last decided. A person's three words are theirs in
/// both readings.
#[test]
fn auto_records_until_its_own_judge_says_otherwise() {
    assert!(!JevMode::Off.asks() && !JevMode::Off.applies());
    assert!(JevMode::Shadow.asks() && !JevMode::Shadow.applies());
    assert!(JevMode::Auto.asks() && !JevMode::Auto.applies());
    assert!(JevMode::On.asks() && JevMode::On.applies());
    for raised in [false, true] {
        assert!(!JevMode::Off.applies_with(raised));
        assert!(!JevMode::Shadow.applies_with(raised));
        assert!(JevMode::On.applies_with(raised));
        assert_eq!(JevMode::Auto.applies_with(raised), raised);
    }
}

/// Promotion rises from `auto` to acting, so a use that promotes offers both.
#[test]
fn a_use_names_a_rise_line_exactly_when_auto_may_rise_for_it() {
    // A floor on a seat that never rises is a number nobody reads; a rising
    // seat with no floor is a promotion with nothing to pass. §4 gives the
    // line to the use because the line is the use's own.
    for row in JEV_USES {
        assert_eq!(
            row.promotes,
            row.answer_floor_permille.is_some(),
            "{} promotes={} floor={:?}",
            row.id,
            row.promotes,
            row.answer_floor_permille
        );
        assert_eq!(
            row.promotes,
            row.agreement_floor_permille.is_some(),
            "{} promotes={} agreement floor={:?}: a seat that rises with no labels needs its route-change budget",
            row.id,
            row.promotes,
            row.agreement_floor_permille
        );
        for floor in [row.answer_floor_permille, row.agreement_floor_permille]
            .into_iter()
            .flatten()
        {
            // Under a thousand: a floor of a thousand has no window that
            // clears it (`summary::rows_that_can_clear`), and a line no
            // evidence can reach is `shadow` under another name.
            assert!(
                (500..1000).contains(&floor),
                "{} asks for a share of {floor} per thousand",
                row.id
            );
            assert!(
                summary::rows_that_can_clear(floor) < usize::MAX,
                "{} names a floor no window clears",
                row.id
            );
        }
    }
}

/// The two columns that say what a seat's window forgives and how much
/// comparison its route-change budget is read over belong to the same rule
/// as the floors: a seat that rises names both, a seat that does not names
/// neither, and neither may ask for a window no evidence reaches.
#[test]
fn a_use_names_what_its_window_forgives_and_what_it_compares_exactly_when_it_rises() {
    for row in JEV_USES {
        assert_eq!(
            row.promotes,
            row.window_forgives.is_some(),
            "{} promotes={} forgives={:?}",
            row.id,
            row.promotes,
            row.window_forgives
        );
        assert_eq!(
            row.promotes,
            row.agreement_rows_wanted.is_some(),
            "{} promotes={} comparisons wanted={:?}",
            row.id,
            row.promotes,
            row.agreement_rows_wanted
        );
        let (Some(floor), Some(forgives)) = (row.answer_floor_permille, row.window_forgives) else {
            continue;
        };
        // A window is a window: forgiving more than the floor's own miss
        // budget would be a line the floor no longer means.
        assert!(
            forgives <= FORGIVES_A_BAD_MINUTE,
            "{} forgives {forgives} rows",
            row.id
        );
        let wanted = summary::rows_that_can_clear_forgiving(floor, forgives);
        assert!(
            wanted < usize::MAX,
            "{} names a window nothing fills",
            row.id
        );
        assert!(
            wanted >= summary::rows_that_can_clear(floor),
            "{} forgives a miss on a window no wider than the perfect one",
            row.id
        );
        // What a forgiving window buys, in the units the seat is judged in:
        // one miss inside it is not the end of the climb.
        let one_short = summary::wilson_lower(wanted - forgives, wanted, summary::WILSON_Z_95);
        assert!(
            crate::jev::promote::permille(one_short) >= floor,
            "{} cannot clear {floor}‰ over {wanted} rows with {forgives} forgiven",
            row.id
        );
    }
}

/// A seat is judged only once its window can be full, and then every
/// [`summary::JUDGED_EVERY_ROWS`] requests — the countdown the screen draws
/// and the cadence the judge runs on are one function.
#[test]
fn a_seats_first_judgment_waits_for_a_window_it_can_fill() {
    use crate::jev::promote::{rows_to_next_judgment, window_wanted_for};
    for seat in JEV_USES.iter().filter(|row| row.promotes) {
        let wanted = window_wanted_for(seat).expect("a rising seat names a window");
        assert_eq!(rows_to_next_judgment(seat, 0), Some(wanted));
        assert_eq!(rows_to_next_judgment(seat, wanted - 1), Some(1));
        assert_eq!(
            rows_to_next_judgment(seat, wanted),
            Some(summary::JUDGED_EVERY_ROWS)
        );
        assert_eq!(
            rows_to_next_judgment(seat, wanted + 1),
            Some(summary::JUDGED_EVERY_ROWS - 1)
        );
    }
}

/// The seats with no reader to be compared against are the ones whose own
/// mark is a later fact — the person's move, the turn's read, the re-read
/// after a drop. The table names them for a reader's sake, and the judge
/// holds them to the same sample floor as everyone else: no seat rises on
/// answer rate, latency and shape alone (t-6155 F1).
#[test]
fn every_seat_waits_for_a_window_of_marks_whatever_kind_they_are() {
    // The counter and both writers ask one judge; the sample floor is the
    // named table field, and the judge reads no kind beside it.
    let judge = include_str!("promote.rs");
    assert!(judge.contains("agreement.compared < evidence.agreement_rows_wanted"));
    assert!(
        !judge.contains("AgreementKind"),
        "the judge reads no kind: the floor holds every seat"
    );

    let hindsight: Vec<&str> = JEV_USES
        .iter()
        .filter(|row| row.agreement_kind == AgreementKind::Hindsight)
        .map(|row| row.id)
        .collect();
    assert_eq!(hindsight, vec![RECALL.id, PLACEMENT.id, COMPACTION.id]);
    for row in JEV_USES.iter().filter(|row| row.promotes) {
        assert_eq!(
            row.agreement_rows_wanted,
            Some(A_WINDOW_OF_COMPARISONS),
            "{}",
            row.id
        );
    }
}

/// The placement seat's answer floor is its own, and under the line the other
/// orchestration seats share: the cheapest negative in the table pays the
/// least for it (the vault's "a judgment seat pays where its negatives are
/// few, not where its ceiling is high").
#[test]
fn the_placement_seats_line_sits_where_its_negatives_are() {
    assert_eq!(
        PLACEMENT.answer_floor_permille,
        Some(PLACEMENT_ANSWER_FLOOR_PERMILLE)
    );
    const { assert!(PLACEMENT_ANSWER_FLOOR_PERMILLE < ORCHESTRATION_ANSWER_FLOOR_PERMILLE) };
    for row in [STALL, SUMMON, STEP_EFFORT] {
        assert_eq!(
            row.answer_floor_permille,
            Some(ORCHESTRATION_ANSWER_FLOOR_PERMILLE),
            "{}",
            row.id
        );
    }
    // And it buys a window the seat's own ledger can fill: 25 rows against
    // the 53 the nine-in-ten line asks for once a miss is forgiven, on a
    // ledger 38 rows long (2026-09-22).
    assert_eq!(
        summary::rows_that_can_clear_forgiving(
            PLACEMENT_ANSWER_FLOOR_PERMILLE,
            FORGIVES_A_BAD_MINUTE
        ),
        25
    );
    assert_eq!(
        summary::rows_that_can_clear_forgiving(
            ORCHESTRATION_ANSWER_FLOOR_PERMILLE,
            FORGIVES_A_BAD_MINUTE
        ),
        53
    );
}

/// Promotion rises from `auto` to acting, so a use that promotes offers
/// `auto` — and a word that acts once the judge has raised it, which `auto`
/// itself is.
#[test]
fn a_use_that_promotes_has_somewhere_to_rise_from_and_to() {
    for row in JEV_USES.iter().filter(|row| row.promotes) {
        assert!(row.modes.contains(&JevMode::Auto), "{}", row.id);
        assert!(
            row.modes.iter().any(|mode| mode.applies_with(true)),
            "{}",
            row.id
        );
    }
}

/// The seat contract's third rule (corrected after t-6155 F7): the words a
/// seat offers are read off whether anything labels it. A labeled seat —
/// one that promotes — offers all four, `on` being a person's explicit
/// override and `auto` what its own evidence raises; a seat nothing labels
/// offers `off | shadow | on`, because an `auto` there could never rise and
/// would be `shadow` under a name that promises otherwise. One rule for
/// every row, so a new seat cannot pick a third set: the notify and
/// branching seats had (t-6155 F7).
#[test]
fn a_seats_mode_set_is_read_off_whether_anything_labels_it() {
    let labeled: &[JevMode] = &JevMode::ALL;
    let unlabeled: &[JevMode] = &[JevMode::Off, JevMode::Shadow, JevMode::On];
    for row in JEV_USES.iter() {
        assert_eq!(
            row.promotes,
            row.agreement_rows_wanted.is_some(),
            "{}: a labeled seat names its label sample floor",
            row.id
        );
        let expected = if row.promotes { labeled } else { unlabeled };
        assert_eq!(row.modes, expected, "{}", row.id);
    }
    assert!(
        JEV_USES.iter().any(|row| !row.promotes),
        "the rule is exercised on both kinds of seat"
    );
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
    assert_eq!(jev_use("a seat the table does not name"), None);
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
        crate::screen_action::MAX_ACTION_CANDIDATES,
        SCREEN_CANDIDATE_CAP
    );
    let caps = |row: &JevUse| -> Vec<Cap> { row.sends.iter().map(|sent| sent.cap).collect() };
    assert!(caps(&ROUTING).contains(&Cap::Chars(ROUTING_TASK_CHAR_CAP)));
    assert!(caps(&RECALL).contains(&Cap::Chars(RECALL_REQUEST_CHAR_CAP)));
    assert!(caps(&RECALL).contains(&Cap::Items(RECALL_NOTE_CAP)));
    assert!(caps(&RECALL).contains(&Cap::Bytes(RECALL_SUMMARY_BYTE_CAP)));
    assert!(caps(&BROWSER).contains(&Cap::Items(SCREEN_CANDIDATE_CAP)));
    assert!(caps(&STALL).contains(&Cap::Bytes(STALL_SCREEN_BYTE_CAP)));
    assert!(caps(&STALL).contains(&Cap::Bytes(STALL_TRANSCRIPT_BYTE_CAP)));
    assert!(caps(&PLACEMENT).contains(&Cap::Chars(PLACEMENT_BRIEF_CHAR_CAP)));
    assert!(caps(&SUMMON).contains(&Cap::Chars(SUMMON_BRIEF_CHAR_CAP)));
    assert!(caps(&SKILLS).contains(&Cap::Chars(SKILL_TASK_CHAR_CAP)));
    assert!(caps(&SKILLS).contains(&Cap::Items(SKILL_SHARD_TARGET)));
    assert!(caps(&SKILLS).contains(&Cap::Chars(SKILL_DESCRIPTION_CHAR_CAP)));
    assert!(caps(&ZO_STEP_EFFORT).contains(&Cap::Chars(ROUTING_TASK_CHAR_CAP)));
    assert!(caps(&BROWSER_READ).contains(&Cap::Chars(BROWSER_READ_TITLE_CHAR_CAP)));
    assert!(caps(&BROWSER_READ).contains(&Cap::Items(BROWSER_READ_BLOCK_CAP)));
    assert!(caps(&BROWSER_READ).contains(&Cap::Chars(BROWSER_READ_PATH_CHAR_CAP)));
    assert!(caps(&BROWSER_READ).contains(&Cap::Chars(BROWSER_READ_HEAD_CHAR_CAP)));
}

/// The browser-read seat folds only on an answer over its own line, sends a
/// page's title and each block's path and head and never a block's body,
/// waits one wall for every shard, and rises on the screen seats' answer
/// line with a stricter route-change budget than theirs — a fold the agent
/// reached back into costs a whole page, not one press.
#[test]
fn the_browser_read_seat_folds_on_its_own_line_and_sends_no_body() {
    assert_eq!(BROWSER_READ.id, "browser_read");
    assert_eq!(BROWSER_READ.setting, "jevBrowserRead");
    assert_eq!(BROWSER_READ.ledger, "browser-read.jsonl");
    assert_eq!(BROWSER_READ.modes, &JevMode::ALL[..]);
    const {
        assert!(BROWSER_READ.promotes);
        assert!(
            BROWSER_READ_FOLD_FLOOR_PERMILLE > SCREEN_PRESS_FLOOR_PERMILLE,
            "a dropped block costs more than a press"
        );
        assert!(BROWSER_READ_AGREEMENT_FLOOR_PERMILLE > SCREEN_AGREEMENT_FLOOR_PERMILLE);
        assert!(BROWSER_READ_SHARD_TARGET * 4 == BROWSER_READ_BLOCK_CAP);
    }
    assert_eq!(
        BROWSER_READ.press_floor_permille,
        Some(BROWSER_READ_FOLD_FLOOR_PERMILLE)
    );
    assert!(BROWSER_READ.permits_press(0.7) && !BROWSER_READ.permits_press(0.69));
    assert_eq!(
        BROWSER_READ.answer_floor_permille,
        Some(BROWSER_READ_ANSWER_FLOOR_PERMILLE)
    );
    assert_eq!(
        BROWSER_READ.agreement_floor_permille,
        Some(BROWSER_READ_AGREEMENT_FLOOR_PERMILLE)
    );
    assert_eq!(
        BROWSER_READ.apply_deadline_ms,
        Some(BROWSER_READ_APPLY_DEADLINE_MS)
    );
    assert_eq!(BROWSER_READ.agreement_kind, AgreementKind::Comparison);
    let sent: Vec<&str> = BROWSER_READ.sends.iter().map(|sent| sent.at).collect();
    assert_eq!(
        sent,
        vec![
            "/state/title",
            "/state/blocks",
            "/state/blocks/*/path",
            "/state/blocks/*/head"
        ]
    );
    assert!(
        !sent.iter().any(|at| at.ends_with("/text")),
        "a block's body never leaves"
    );
    assert_eq!(BROWSER_READ_OPTIONS, ["content", "chrome"]);
    assert_eq!(BROWSER_READ_CHROME, "chrome");
    // The block roots are selectors, each spelled once.
    let mut roots = BROWSER_READ_BLOCK_ROOTS.to_vec();
    roots.sort_unstable();
    roots.dedup();
    assert_eq!(roots.len(), BROWSER_READ_BLOCK_ROOTS.len());
    for root in BROWSER_READ_BLOCK_ROOTS {
        assert!(!root.contains(',') && !root.contains(' '), "{root}");
    }
    // The card and the harness read the whole table: the seat is on it.
    assert!(JEV_USES.contains(&BROWSER_READ));
    assert_eq!(
        BROWSER_READ.mode_in(&json!({"smart": {"browserAction": "on"}})),
        JevMode::Off,
        "the walk's consent is not the read's"
    );
}

/// The skill seat sends a name and a line about each skill, and never a
/// skill's body: the body is read off this machine's own disk and handed to
/// the model as a tool result, so what leaves is a list and what the turn
/// reads is a document.
#[test]
fn the_skill_seat_sends_names_and_descriptions_and_never_a_body() {
    let sent: Vec<&str> = SKILLS.sends.iter().map(|sent| sent.at).collect();
    assert_eq!(
        sent,
        vec![
            "/state/task",
            "/state/skills",
            "/state/skills/*/name",
            "/state/skills/*/description",
        ]
    );
    assert!(
        !sent
            .iter()
            .any(|at| at.contains("body") || at.contains("prompt")),
        "a skill's body never leaves: {sent:?}"
    );
}

/// The skill search's own floor is a line under one skill's relevance, and
/// the seat's rise line is a line under how often the seat answers at all.
/// Two different questions, kept apart by their units as well as their names.
#[test]
fn a_skills_relevance_floor_is_not_the_seats_answer_rate() {
    // 1.4 of a top level of 2 — the reference build's line, in this table's
    // own per-thousand units.
    let top = SKILL_LEVELS.len() - 1;
    assert_eq!(top, 2);
    assert_eq!(SKILL_RELEVANCE_FLOOR_PERMILLE, 700);
    assert_eq!(
        f64::from(SKILL_RELEVANCE_FLOOR_PERMILLE) / 1_000.0 * 2.0,
        1.4
    );
    assert_eq!(
        SKILLS.answer_floor_permille,
        Some(SKILL_ANSWER_FLOOR_PERMILLE)
    );
    assert_ne!(
        SKILLS.answer_floor_permille,
        Some(SKILL_RELEVANCE_FLOOR_PERMILLE),
        "the seat's rise line is not a skill's relevance line"
    );
    // A level is judged on its own against the state, so each one describes a
    // situation rather than a degree — no level names its own number or its
    // neighbour.
    for level in SKILL_LEVELS {
        assert!(!level.is_empty());
        assert!(
            !level.contains("more") && !level.contains("less"),
            "a level describes a situation, not a degree: {level}"
        );
    }
}

/// The skill seat rises on its own evidence, waits a wall a person sits
/// through, and asks in shards no larger than one request should carry.
#[test]
fn the_skill_seat_rises_on_its_own_lines() {
    const { assert!(SKILLS.promotes) };
    assert!(SKILLS.modes.contains(&JevMode::On));
    assert_eq!(
        SKILLS.agreement_floor_permille,
        Some(SKILL_AGREEMENT_FLOOR_PERMILLE)
    );
    assert_eq!(
        SKILLS.apply_deadline_ms,
        Some(SKILL_SEARCH_APPLY_DEADLINE_MS)
    );
    assert_eq!(
        SKILLS.press_floor_permille, None,
        "a search presses nothing"
    );
    // The window it is judged over is one a real day of searches can fill.
    assert!(summary::rows_that_can_clear(SKILL_ANSWER_FLOOR_PERMILLE) <= 40);
    // Every shard fits the cap the table says one request carries.
    for total in [1_usize, 50, 51, 101, 137] {
        for shard in shard::even_shards(total, SKILL_SHARD_TARGET) {
            assert!(shard.len() <= SKILL_SHARD_TARGET, "{total}: {shard:?}");
        }
    }
}

/// The step governor's seat sends the same head of a turn the routing seat
/// sends, rises on its own progress marks, and offers every word — a seat
/// whose answer moves a request field is one a person can switch on and one
/// evidence can raise (docs/design/zo-step-effort-governor-20260921.md §5).
#[test]
fn the_step_effort_seat_reads_the_turn_like_routing_and_rises_on_its_own_marks() {
    assert_eq!(jev_use("step_effort"), Some(&ZO_STEP_EFFORT));
    assert_eq!(ZO_STEP_EFFORT.sends, ROUTING.sends);
    const { assert!(ZO_STEP_EFFORT.promotes) };
    assert_eq!(
        ZO_STEP_EFFORT.answer_floor_permille,
        ROUTING.answer_floor_permille
    );
    assert_eq!(
        ZO_STEP_EFFORT.agreement_floor_permille,
        ROUTING.agreement_floor_permille
    );
    assert_eq!(
        ZO_STEP_EFFORT.apply_deadline_ms,
        Some(ZO_STEP_EFFORT_APPLY_DEADLINE_MS)
    );
    assert!(ZO_STEP_EFFORT.mode_of(Some(&json!("on"))).applies());
    assert!(
        !ZO_STEP_EFFORT
            .mode_of(Some(&json!("auto")))
            .applies_with(false)
    );
    assert!(
        ZO_STEP_EFFORT
            .mode_of(Some(&json!("auto")))
            .applies_with(true)
    );
    assert!(
        !ZO_STEP_EFFORT.permits_press(1.0),
        "a step judgment presses nothing"
    );
}

/// A tool result about to be summarized away is kept or dropped on the
/// remaining work, not on its age: the seat sends the goal, the newest words
/// and each block's head, answers a closed keep/drop, drops only past its
/// own lean, and rises on the hindsight of the turns after — a dropped block
/// read again inside the window is the regret its label records.
#[test]
fn the_compaction_seat_drops_by_relevance_and_rises_on_its_regret_marks() {
    assert_eq!(jev_use("compaction"), Some(&COMPACTION));
    let sent: Vec<&str> = COMPACTION.sends.iter().map(|sent| sent.at).collect();
    assert_eq!(
        sent,
        [
            "/state/goal",
            "/state/recent",
            "/state/blocks",
            "/state/blocks/*/tool",
            "/state/blocks/*/input",
            "/state/blocks/*/head",
        ],
        "heads only: a body never leaves, and the tail is not asked about"
    );
    let caps = |row: &JevUse| -> Vec<Cap> { row.sends.iter().map(|sent| sent.cap).collect() };
    assert!(caps(&COMPACTION).contains(&Cap::Chars(COMPACTION_GOAL_CHAR_CAP)));
    assert!(caps(&COMPACTION).contains(&Cap::Items(COMPACTION_SHARD_TARGET)));
    assert!(caps(&COMPACTION).contains(&Cap::Bytes(COMPACTION_BLOCK_HEAD_BYTE_CAP)));
    assert!(caps(&COMPACTION).contains(&Cap::Chars(COMPACTION_INPUT_CHAR_CAP)));

    const { assert!(COMPACTION.promotes) };
    assert_eq!(COMPACTION.agreement_kind, AgreementKind::Hindsight);
    assert_eq!(
        COMPACTION.answer_floor_permille,
        Some(COMPACTION_ANSWER_FLOOR_PERMILLE)
    );
    assert_eq!(
        COMPACTION.agreement_floor_permille,
        Some(COMPACTION_AGREEMENT_FLOOR_PERMILLE)
    );
    assert_eq!(
        COMPACTION.apply_deadline_ms,
        Some(COMPACTION_APPLY_DEADLINE_MS),
        "the one wall the whole batch waits is the row's"
    );
    // A miss costs nothing, so the seat waits longer than a turn's route
    // and shorter than a summons: it sits beside a summary round-trip.
    const { assert!(COMPACTION_APPLY_DEADLINE_MS > ROUTING_APPLY_DEADLINE_MS) };
    const { assert!(COMPACTION_APPLY_DEADLINE_MS < SUMMON_APPLY_DEADLINE_MS) };

    assert_eq!(COMPACTION_OPTIONS, [COMPACTION_KEEP, COMPACTION_DROP]);
    assert_ne!(COMPACTION_KEEP, COMPACTION_DROP);
    // The drop lean is a lean: over one half, under certainty.
    const { assert!(COMPACTION_DROP_FLOOR_PERMILLE > 500 && COMPACTION_DROP_FLOOR_PERMILLE < 1_000) };
    const { assert!(COMPACTION_REGRET_TURNS > 0) };

    assert!(COMPACTION.mode_of(Some(&json!("on"))).applies());
    assert!(!COMPACTION.mode_of(Some(&json!("auto"))).applies_with(false));
    assert!(COMPACTION.mode_of(Some(&json!("auto"))).applies_with(true));
    assert!(
        !COMPACTION
            .mode_of(Some(&json!("shadow")))
            .applies_with(true)
    );
    assert!(
        !COMPACTION.permits_press(1.0),
        "a compaction judgment presses nothing"
    );
}

/// The notify seat (t-6043) judges one ring — attention, completion or a
/// push — at the one point today's rule table decides "ring or not", and
/// rises on the person's own reaction: a pane they turned to within
/// [`NOTIFY_LABEL_WINDOW_MS`] was one worth the interruption.
#[test]
fn the_notify_seat_judges_one_ring_and_rises_on_the_persons_reaction() {
    assert_eq!(jev_use("notify"), Some(&NOTIFY));
    assert_eq!(NOTIFY.setting, "jevNotify");
    // A labeled seat offers the four words every labeled seat offers (the
    // seat contract's third rule, as corrected after t-6155 F7): `on` is a
    // person's explicit override, `auto` is what its own evidence raises.
    assert_eq!(NOTIFY.modes, &JevMode::ALL[..]);
    assert_eq!(NOTIFY.offered("on"), Some(JevMode::On));
    let sent: Vec<&str> = NOTIFY.sends.iter().map(|sent| sent.at).collect();
    assert_eq!(
        sent,
        ["/state/pane", "/state/words", "/state/recent"],
        "the pane's name, one card line of words, and the pane's last rings — nothing else a person wrote"
    );
    let caps = |row: &JevUse| -> Vec<Cap> { row.sends.iter().map(|sent| sent.cap).collect() };
    assert!(caps(&NOTIFY).contains(&Cap::Chars(NOTIFY_WORDS_CHAR_CAP)));
    assert!(caps(&NOTIFY).contains(&Cap::Items(NOTIFY_RECENT_CAP)));
    assert_eq!(
        NOTIFY_WORDS_CHAR_CAP,
        crate::transcript::SUMMARY_CHARS,
        "one card line"
    );

    const { assert!(NOTIFY.promotes) };
    assert_eq!(NOTIFY.agreement_kind, AgreementKind::Comparison);
    assert_eq!(
        NOTIFY.answer_floor_permille,
        Some(NOTIFY_ANSWER_FLOOR_PERMILLE)
    );
    assert_eq!(
        NOTIFY.agreement_floor_permille,
        Some(NOTIFY_AGREEMENT_FLOOR_PERMILLE)
    );
    assert_eq!(NOTIFY.apply_deadline_ms, Some(NOTIFY_APPLY_DEADLINE_MS));
    // The wall an attention ring may be held is the wait every completion
    // ring already sits through before it may ring at all.
    assert_eq!(
        NOTIFY_APPLY_DEADLINE_MS,
        crate::notify::DONE_QUIET_MS.unsigned_abs(),
        "the seat's wall is the completion's own quiet"
    );
    assert_eq!(
        NOTIFY_OPTIONS,
        [NOTIFY_INTERRUPT, NOTIFY_BATCH, NOTIFY_IGNORE]
    );
    const { assert!(NOTIFY_LABEL_WINDOW_MS == 60 * 1_000) };
    const { assert!(NOTIFY_ATTENDANCE_WINDOW_MS > NOTIFY_LABEL_WINDOW_MS) };
    const { assert!(NOTIFY_RECENT_CAP > 0) };

    assert!(
        NOTIFY.mode_of(Some(&json!("on"))).applies_with(false),
        "`on` is a person's override: it acts without a rise"
    );
    assert!(!NOTIFY.mode_of(Some(&json!("auto"))).applies_with(false));
    assert!(NOTIFY.mode_of(Some(&json!("auto"))).applies_with(true));
    assert!(!NOTIFY.mode_of(Some(&json!("shadow"))).applies_with(true));
    assert!(
        !NOTIFY.permits_press(1.0),
        "a notify judgment presses nothing"
    );
}

/// A fuzzy page is reordered by what the person is writing, never by a
/// body: the seat sends the sentence in progress, the token typed and one
/// page of candidate names with the head each row already shows, asks one
/// closed choice over them, and rises on the comparison the person makes
/// with every pick — the row they took against the row the judgment put
/// first. The wall is the routing seat's, because nothing waits on it: a
/// late answer is dropped, and the page the person already sees stands.
#[test]
fn the_mention_seat_reranks_a_fuzzy_page_and_rises_on_the_persons_pick() {
    assert_eq!(jev_use("mention_rerank"), Some(&MENTION_RERANK));
    assert_eq!(MENTION_RERANK.setting, "jevMentionRerank");
    let sent: Vec<&str> = MENTION_RERANK.sends.iter().map(|sent| sent.at).collect();
    assert_eq!(
        sent,
        [
            "/state/intent",
            "/state/query",
            "/state/candidates",
            "/state/candidates/*/name",
            "/state/candidates/*/head",
        ],
        "names and heads only: no file body, no page body, no transcript"
    );
    let caps: Vec<Cap> = MENTION_RERANK.sends.iter().map(|sent| sent.cap).collect();
    assert_eq!(caps[0], Cap::Chars(MENTION_INTENT_CHAR_CAP));
    assert_eq!(
        caps[1],
        Cap::Chars(MENTION_INTENT_CHAR_CAP),
        "the token is part of the same sentence"
    );
    assert_eq!(caps[2], Cap::Items(MENTION_CANDIDATE_CAP));
    assert_eq!(caps[4], Cap::Bytes(MENTION_HEAD_BYTE_CAP));
    // One page, and one page only: the popup's window (codex
    // `MAX_POPUP_ROWS`), which zo's own view pins to the same number.
    const { assert!(MENTION_CANDIDATE_CAP == 8) };
    assert_eq!(
        MENTION_INTENT_CHAR_CAP, RECALL_REQUEST_CHAR_CAP,
        "the same kind of text, the same cap"
    );
    assert_eq!(MENTION_HEAD_BYTE_CAP, RECALL_SUMMARY_BYTE_CAP);

    const { assert!(MENTION_RERANK.promotes) };
    assert_eq!(MENTION_RERANK.agreement_kind, AgreementKind::Comparison);
    assert_eq!(
        MENTION_RERANK.answer_floor_permille,
        Some(MENTION_ANSWER_FLOOR_PERMILLE)
    );
    assert_eq!(
        MENTION_RERANK.agreement_floor_permille,
        Some(MENTION_AGREEMENT_FLOOR_PERMILLE)
    );
    assert_eq!(
        MENTION_RERANK.apply_deadline_ms,
        Some(MENTION_APPLY_DEADLINE_MS),
        "the one wall a late answer is dropped past is the row's"
    );
    assert_eq!(MENTION_RERANK.window_forgives, Some(FORGIVES_A_BAD_MINUTE));
    assert_eq!(
        MENTION_RERANK.agreement_rows_wanted,
        Some(A_WINDOW_OF_COMPARISONS)
    );
    assert_eq!(
        MENTION_RERANK.modes,
        &[JevMode::Off, JevMode::Shadow, JevMode::On, JevMode::Auto],
        "the recall seat's words: a person's on, and an auto that rises on its own labels"
    );
    assert!(MENTION_RERANK.mode_of(Some(&json!("on"))).applies());
    assert!(
        !MENTION_RERANK
            .mode_of(Some(&json!("shadow")))
            .applies_with(true)
    );
    assert!(
        MENTION_RERANK
            .mode_of(Some(&json!("auto")))
            .applies_with(true)
    );
    assert!(
        !MENTION_RERANK
            .mode_of(Some(&json!("auto")))
            .applies_with(false)
    );
    assert!(
        !MENTION_RERANK.permits_press(1.0),
        "a rerank presses nothing"
    );
}

/// A stall's answer is a row beside what the coordinator did, never an act:
/// the use offers nothing that applies, and so nothing it could rise to.
#[test]
fn a_stall_question_acts_under_on_and_under_auto_once_its_evidence_stands() {
    // 2026-09-20: "전부 자동 기록하며 실제 적용되어야" — a seat that only
    // ever records is `shadow` under another name. `on` acts on a person's
    // word; `auto` records until the judge raises it (§4) and acts after.
    assert!(STALL.modes.contains(&JevMode::On));
    const { assert!(STALL.promotes) };
    assert_eq!(STALL.mode_of(Some(&json!("on"))), JevMode::On);
    assert_eq!(STALL.mode_of(Some(&json!("auto"))), JevMode::Auto);
    assert!(STALL.mode_of(Some(&json!("on"))).applies());
    assert!(!STALL.mode_of(Some(&json!("auto"))).applies_with(false));
    assert!(STALL.mode_of(Some(&json!("auto"))).applies_with(true));
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
fn a_placement_question_acts_under_on_and_under_auto_once_its_evidence_stands() {
    // 2026-09-20: "전부 자동 기록하며 실제 적용되어야" — a seat that only
    // ever records is `shadow` under another name. `on` acts on a person's
    // word; `auto` records until the judge raises it (§4) and acts after.
    assert!(PLACEMENT.modes.contains(&JevMode::On));
    const { assert!(PLACEMENT.promotes) };
    assert_eq!(PLACEMENT.mode_of(Some(&json!("on"))), JevMode::On);
    assert_eq!(PLACEMENT.mode_of(Some(&json!("auto"))), JevMode::Auto);
    assert!(PLACEMENT.mode_of(Some(&json!("on"))).applies());
    assert!(!PLACEMENT.mode_of(Some(&json!("auto"))).applies_with(false));
    assert!(PLACEMENT.mode_of(Some(&json!("auto"))).applies_with(true));
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

/// Which agent carries a summons is a judgment about the work, and the
/// coordinator's three typed words are what it is written down beside. The
/// use offers nothing that applies: a worker is summoned by what the caller
/// asked for, and the row is evidence, not a substitution.
#[test]
fn a_summon_question_acts_under_on_and_under_auto_once_its_evidence_stands() {
    // 2026-09-20: "전부 자동 기록하며 실제 적용되어야" — a seat that only
    // ever records is `shadow` under another name. `on` acts on a person's
    // word; `auto` records until the judge raises it (§4) and acts after.
    assert!(SUMMON.modes.contains(&JevMode::On));
    const { assert!(SUMMON.promotes) };
    assert_eq!(SUMMON.mode_of(Some(&json!("on"))), JevMode::On);
    assert_eq!(SUMMON.mode_of(Some(&json!("auto"))), JevMode::Auto);
    assert!(SUMMON.mode_of(Some(&json!("on"))).applies());
    assert!(!SUMMON.mode_of(Some(&json!("auto"))).applies_with(false));
    assert!(SUMMON.mode_of(Some(&json!("auto"))).applies_with(true));
    assert_eq!(jev_use("summon"), Some(&SUMMON));
}

/// A worker's between-turn effort move is a judgment about its last turn,
/// and the door is the agent's row: the seat acts under `on` and under an
/// `auto` its own evidence raised, records under `shadow`, and its one text
/// is the repeated call's card line.
#[test]
fn a_step_effort_question_acts_under_on_and_under_auto_once_its_evidence_stands() {
    assert!(STEP_EFFORT.modes.contains(&JevMode::On));
    const { assert!(STEP_EFFORT.promotes) };
    assert_eq!(STEP_EFFORT.mode_of(Some(&json!("on"))), JevMode::On);
    assert_eq!(STEP_EFFORT.mode_of(Some(&json!("auto"))), JevMode::Auto);
    assert!(STEP_EFFORT.mode_of(Some(&json!("on"))).applies());
    assert!(
        !STEP_EFFORT
            .mode_of(Some(&json!("auto")))
            .applies_with(false)
    );
    assert!(STEP_EFFORT.mode_of(Some(&json!("auto"))).applies_with(true));
    assert_eq!(jev_use("effort"), Some(&STEP_EFFORT));
    let sent: Vec<&str> = STEP_EFFORT.sends.iter().map(|sent| sent.at).collect();
    assert_eq!(sent, ["/state/repeated"]);
    assert_eq!(
        STEP_EFFORT.apply_deadline_ms,
        Some(STEP_EFFORT_APPLY_DEADLINE_MS),
        "the wall the beat waits is the row's"
    );
    // A move lands between two turns; it cannot wait the stall sweep's wall.
    const { assert!(STEP_EFFORT_APPLY_DEADLINE_MS < STALL_APPLY_DEADLINE_MS) };
}

/// The one text a summons' question carries is the head of the brief. The
/// agent the coordinator typed is not in the table's `sends` because it is
/// not in the state at all — a question that shows the answer somebody
/// already wrote down is not a second opinion.
#[test]
fn a_summon_question_sends_the_brief_and_nothing_else() {
    let sent: Vec<&str> = SUMMON.sends.iter().map(|sent| sent.at).collect();
    assert_eq!(sent, ["/state/brief"]);
}

/// The classifier's four words are zo's four words, and its absent-key answer
/// is zo's.
///
/// The words live here now because the window puts them on a card; zo's own
/// enum stays where it routes from. Both read one settings key, so this reads
/// zo's file and holds the two together — a fifth word, a renamed one, or a
/// changed default in zo is a red test here rather than a card offering a mode
/// zo would read as something else.
#[test]
fn the_classifier_words_are_the_ones_zo_routes_on() {
    let policy = include_str!("../../../../zo-ide/crates/runtime/src/model_router/policy.rs");
    let zo = &policy[policy
        .find("pub enum RouteAutoClassifierMode {")
        .expect("zo's classifier enum")..];
    let zo = &zo[..zo
        .find("\nimpl RouteAutoClassifierMode")
        .expect("the enum closes")];
    for mode in ClassifierMode::ALL {
        assert!(
            policy.contains(&format!("value == \"{}\"", mode.key()))
                || mode == ClassifierMode::Deterministic,
            "zo does not read `{}`",
            mode.key()
        );
    }
    // Deterministic is zo's else-branch rather than a compared word, so it is
    // named by the variant it falls through to.
    assert!(
        zo.matches("    Off,").count()
            + zo.matches("    Deterministic,").count()
            + zo.matches("    Assisted,").count()
            + zo.matches("    Probed,").count()
            == ClassifierMode::ALL.len(),
        "zo offers a different number of classifier modes:\n{zo}"
    );

    // Absent means probed — zo's settings reader, not its enum default.
    let settings =
        include_str!("../../../../zo-ide/crates/tools/src/misc_tools/smart_router/settings.rs");
    assert!(
        settings.contains("None => RouteAutoClassifierMode::Probed"),
        "zo no longer reads an absent `{CLASSIFIER_SETTING}` as probed"
    );
    assert_eq!(ClassifierMode::of(None), ClassifierMode::Probed);
    assert_eq!(
        ClassifierMode::of(Some(&json!(" PROBED "))),
        ClassifierMode::Probed
    );
    assert_eq!(
        ClassifierMode::of(Some(&json!("surprise"))),
        ClassifierMode::Deterministic,
        "a word nobody reads is the provider-free verdict, never the probing one"
    );
    assert_eq!(
        ClassifierMode::of(Some(&json!(true))),
        ClassifierMode::Deterministic
    );
    assert_eq!(
        ClassifierMode::offered("probed"),
        Some(ClassifierMode::Probed)
    );
    assert_eq!(
        ClassifierMode::offered("PROBED"),
        None,
        "a writer is held to the word"
    );
    assert_eq!(
        ClassifierMode::in_settings(&json!({ "smart": { "autoClassifier": "assisted" } })),
        ClassifierMode::Assisted
    );
}

/// Only the probing word asks the routing seat anything.
///
/// This is the fact the card has to say out loud, so it is held to zo's
/// source: the decision shadow is fired from `probe_and_shadow` alone, and the
/// probe is called only under `Probed`. If zo ever fires the shadow from
/// somewhere else, the card's notice becomes a lie and this goes red first.
#[test]
fn the_routing_seat_is_only_asked_under_the_probing_word() {
    let probe_exec =
        include_str!("../../../../zo-ide/crates/tools/src/misc_tools/smart_router/probe_exec.rs");
    let apply =
        include_str!("../../../../zo-ide/crates/tools/src/misc_tools/smart_router/apply.rs");
    let product = |source: &'static str| source.split("#[cfg(test)]").next().unwrap_or(source);
    assert_eq!(
        product(probe_exec)
            .matches("decision_shadow::fire(")
            .count(),
        1,
        "the shadow is fired from more than one place"
    );
    assert_eq!(
        product(probe_exec)
            .matches("decision_shadow::active_assessments(")
            .count(),
        1,
        "the shadow's active road has more than one entrance"
    );
    for entry in ["route_probe_assessment(", "route_probe_assessments("] {
        for at in product(apply).match_indices(entry).map(|(at, _)| at) {
            let before = &product(apply)[..at];
            let gate = before
                .rfind("RouteAutoClassifierMode::Probed")
                .expect("a probe call with no Probed gate above it");
            assert!(
                before.len() - gate < 800,
                "a `{entry}` call is not under a Probed gate"
            );
        }
    }
    assert!(
        ClassifierMode::ALL
            .iter()
            .filter(|mode| mode.probes())
            .count()
            == 1,
        "more than one word claims to probe"
    );
    assert!(ROUTING.modes.iter().copied().any(JevMode::applies));
}

#[test]
fn only_auto_changes_its_mind_when_the_judge_speaks() {
    // A person's word is theirs: `on` acts whatever a window says and `off`
    // and `shadow` stay put. `auto` is the one mode evidence moves.
    for mode in JevMode::ALL {
        let quiet = mode.applies_with(false);
        let raised = mode.applies_with(true);
        assert_eq!(
            quiet != raised,
            mode == JevMode::Auto,
            "{} moved={} ",
            mode.key(),
            quiet != raised
        );
        assert_eq!(
            mode.applies(),
            quiet,
            "{} reads as unraised without a judgment",
            mode.key()
        );
    }
    assert!(JevMode::On.applies_with(false));
    assert!(!JevMode::Shadow.applies_with(true));
}

/// The seat an agent asks on purpose (t-6040): a tool and a CLI, never a
/// stage of the product's own. It names the wire's own bounds for what one
/// question may carry, offers no `auto` — nothing labels it, so `auto` could
/// never rise and would be `shadow` under a name that promises otherwise —
/// and never promotes, so every rise line on its row is empty.
#[test]
fn the_agent_tool_seat_names_the_wires_bounds_and_never_rises() {
    let row = jev_use(AGENT_TOOL.id).expect("the agent tool row");
    assert_eq!(row.id, "agent_tool");
    assert_eq!(row.setting, "agentTool");
    assert_eq!(row.modes, &[JevMode::Off, JevMode::Shadow, JevMode::On]);
    assert!(!row.promotes);
    assert_eq!(row.answer_floor_permille, None);
    assert_eq!(row.agreement_floor_permille, None);
    assert_eq!(row.apply_deadline_ms, None);
    assert_eq!(row.window_forgives, None);
    assert_eq!(row.agreement_rows_wanted, None);
    assert_eq!(row.press_floor_permille, None);

    // What one question may carry: the caller's words, every one of them
    // cleared and cut where the door reads them.
    let at: Vec<&str> = row.sends.iter().map(|sent| sent.at).collect();
    assert_eq!(
        at,
        vec![
            "/state/question",
            "/state/context",
            "/state/items",
            "/state/items/*",
            "/questions/*/criteria/*",
        ]
    );
    for sent in row.sends {
        match sent.at {
            "/state/items" => assert_eq!(sent.cap, Cap::Items(SKILL_SHARD_TARGET)),
            _ => assert_eq!(
                sent.cap,
                Cap::Chars(AGENT_TOOL_TEXT_CHAR_CAP),
                "{}",
                sent.at
            ),
        }
    }
    assert_eq!(AGENT_TOOL_TEXT_CHAR_CAP, ROUTING_TASK_CHAR_CAP);
    // The wire's own ceilings (docs.typesafe.ai: a choice takes up to 255
    // options, a score between two and ten levels), spelled once here.
    assert_eq!(AGENT_TOOL_OPTION_CAP, 255);
    assert_eq!(AGENT_TOOL_LEVELS, 2..=10);
    assert_eq!(
        AGENT_TOOL_ITEM_CAP % SKILL_SHARD_TARGET,
        0,
        "items are even shards"
    );
    assert_eq!(AGENT_TOOL_DEADLINE_MS, SKILL_SEARCH_APPLY_DEADLINE_MS);
    assert_eq!(AGENT_TOOL_ASK_OPTIONS, ["yes", "no"]);
    assert_eq!(JEV_USES.len(), 18);
}

/// The branching seat (t-6044) forks one phone step — the emulator seat's
/// top candidates each tried on a saved device and the results compared —
/// and rises on the walk's own next step.
#[test]
fn the_branching_seat_forks_a_phone_step_and_rises_on_the_walks_next_step() {
    assert_eq!(jev_use("branching"), Some(&BRANCHING));
    assert_eq!(BRANCHING.setting, "jevBranching");
    assert_eq!(BRANCHING.ledger, "branching.jsonl");
    // A labeled seat offers the four words every labeled seat offers (the
    // seat contract's third rule, as corrected after t-6155 F7).
    assert_eq!(BRANCHING.modes, &JevMode::ALL[..]);
    assert_eq!(BRANCHING.offered("on"), Some(JevMode::On));
    assert!(
        BRANCHING.mode_of(Some(&json!("on"))).applies_with(false),
        "`on` is a person's override: it acts without a rise"
    );
    assert!(!BRANCHING.mode_of(Some(&json!("auto"))).applies_with(false));
    assert!(BRANCHING.mode_of(Some(&json!("auto"))).applies_with(true));
    assert_eq!(
        BRANCHING.mode_in(&json!({"smart": {"emulatorAction": "auto"}})),
        JevMode::Off,
        "the emulator seat's consent is not the fork's"
    );
    assert_ne!(BRANCHING.setting, EMULATOR.setting);
    assert_ne!(BRANCHING.ledger, EMULATOR.ledger);

    // What is sent: the goal, the phone's address, the screen before and each
    // candidate's action and the controls it led to — never a body.
    let sent: Vec<&str> = BRANCHING.sends.iter().map(|sent| sent.at).collect();
    assert_eq!(
        sent,
        [
            "/state/goal",
            "/state/where/platform",
            "/state/where/device",
            "/state/before",
            "/state/before/*",
            "/state/candidates",
            "/state/candidates/*/action",
            "/state/candidates/*/result/controls",
            "/state/candidates/*/result/controls/*",
            "/questions/*/criteria/*",
        ]
    );
    let caps = |row: &JevUse| -> Vec<Cap> { row.sends.iter().map(|sent| sent.cap).collect() };
    assert!(caps(&BRANCHING).contains(&Cap::Chars(GOAL_CHAR_CAP)));
    assert!(caps(&BRANCHING).contains(&Cap::Items(BRANCHING_K_CAP)));
    assert!(caps(&BRANCHING).contains(&Cap::Items(SCREEN_CANDIDATE_CAP)));

    // k is one number in the table, at least two and never past what the
    // question carries.
    const {
        assert!(BRANCHING_K == 2);
        assert!(BRANCHING_K_CAP == 3);
        assert!(BRANCHING_K <= BRANCHING_K_CAP);
        assert!(BRANCHING.promotes);
    }
    assert_eq!(BRANCHING.agreement_kind, AgreementKind::Comparison);
    assert_eq!(
        BRANCHING.answer_floor_permille,
        Some(BRANCHING_ANSWER_FLOOR_PERMILLE)
    );
    assert_eq!(
        BRANCHING.agreement_floor_permille,
        Some(BRANCHING_AGREEMENT_FLOOR_PERMILLE)
    );
    assert_eq!(
        BRANCHING.apply_deadline_ms,
        Some(BRANCHING_APPLY_DEADLINE_MS)
    );
    assert_eq!(BRANCHING.window_forgives, Some(FORGIVES_A_BAD_MINUTE));
    assert_eq!(
        BRANCHING.agreement_rows_wanted,
        Some(A_WINDOW_OF_COMPARISONS)
    );
    // The comparison's pick is pressed at the screen seats' own confidence
    // line; under it the first candidate stands, as today.
    assert_eq!(
        BRANCHING.press_floor_permille,
        Some(SCREEN_PRESS_FLOOR_PERMILLE)
    );
    assert!(BRANCHING.permits_press(0.5) && !BRANCHING.permits_press(0.49));
    assert_eq!(BRANCHING_APPLY_DEADLINE_MS, 1_500);
}

/// The judgment cache (t-6132) is a seat with no question of its own: it
/// sends nothing, presses nothing, and rises on the memo's own comparison —
/// the remembered choice against the fresh one the wire gave for the same
/// bytes. A labeled seat: all four words, `on` the person's own.
#[test]
fn the_judgment_cache_sends_nothing_and_rises_on_the_memos_own_comparison() {
    assert_eq!(jev_use("judgment_cache"), Some(&JUDGMENT_CACHE));
    assert_eq!(JUDGMENT_CACHE.setting, "jevJudgmentCache");
    assert_eq!(
        JUDGMENT_CACHE.modes,
        &[JevMode::Off, JevMode::Shadow, JevMode::On, JevMode::Auto]
    );
    assert_eq!(JUDGMENT_CACHE.offered("on"), Some(JevMode::On));
    assert!(JUDGMENT_CACHE.mode_of(Some(&json!("on"))).applies());
    assert!(
        JUDGMENT_CACHE.sends.is_empty(),
        "a memo hit leaves the machine no bytes; there is nothing to cap"
    );
    assert_eq!(JUDGMENT_CACHE.ledger, "judgment-cache.jsonl");
    const { assert!(JUDGMENT_CACHE.promotes) };
    assert_eq!(JUDGMENT_CACHE.agreement_kind, AgreementKind::Comparison);
    assert_eq!(
        JUDGMENT_CACHE.answer_floor_permille,
        Some(JUDGMENT_CACHE_ANSWER_FLOOR_PERMILLE)
    );
    assert_eq!(
        JUDGMENT_CACHE.agreement_floor_permille,
        Some(JUDGMENT_CACHE_AGREEMENT_FLOOR_PERMILLE)
    );
    // Held above the screen seats' line: the same reader against itself.
    const { assert!(JUDGMENT_CACHE_AGREEMENT_FLOOR_PERMILLE > SCREEN_AGREEMENT_FLOOR_PERMILLE) };
    assert_eq!(
        JUDGMENT_CACHE.apply_deadline_ms,
        Some(JUDGMENT_MEMO_DEADLINE_MS)
    );
    const { assert!(JUDGMENT_MEMO_DEADLINE_MS < SCREEN_APPLY_DEADLINE_MS) };
    assert_eq!(JUDGMENT_CACHE.window_forgives, Some(FORGIVES_NOTHING));
    assert!(
        !JUDGMENT_CACHE.permits_press(1.0),
        "the memo presses nothing; the screen seat's own rule reads the remembered confidence"
    );
    assert!(JUDGMENT_CACHE.mode_of(Some(&json!("on"))).asks());
    assert!(
        !JUDGMENT_CACHE
            .mode_of(Some(&json!("auto")))
            .applies_with(false)
    );
    assert!(
        JUDGMENT_CACHE
            .mode_of(Some(&json!("auto")))
            .applies_with(true)
    );
    // The door asks the memo after its own four questions and before it
    // counts — held as a statement order in the door's source.
    let door = include_str!("door.rs");
    let passing = &door[door
        .find("pub fn pass_remembering(")
        .expect("the memo road")..];
    let asked = passing
        .find("let cleared = ask(&asking)?;")
        .expect("the four questions");
    let looked = passing
        .find("memo::recall(memo.path, &key)")
        .expect("the lookup");
    let counted = passing
        .find("take_a_place(settings, requests)")
        .expect("the count");
    assert!(asked < looked && looked < counted, "{passing}");
}
