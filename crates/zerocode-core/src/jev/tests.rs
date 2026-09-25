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
                seat.permits_press(confidence, crate::guarded::ControlKind::Plain),
                permitted,
                "{} {confidence}",
                seat.id
            );
        }
    }
    assert!(!ROUTING.permits_press(1.0, crate::guarded::ControlKind::Plain));
}

/// A control a press cannot take back asks nine in ten of every screen seat,
/// whatever its own press floor (t-6187); a plain one asks that floor. A
/// seat that presses nothing presses neither.
#[test]
fn a_destructive_control_asks_nine_in_ten_of_every_screen_seat() {
    use crate::guarded::ControlKind::{Destructive, Plain};
    let destructive = f64::from(SCREEN_DESTRUCTIVE_PRESS_FLOOR_PERMILLE) / 1_000.0;
    for seat in [&BROWSER, &DESKTOP, &EMULATOR, &BRANCHING] {
        let plain = f64::from(seat.press_floor_permille.expect("a screen seat presses")) / 1_000.0;
        assert!(plain < destructive, "{}", seat.id);
        assert!(seat.permits_press(plain, Plain), "{}", seat.id);
        assert!(!seat.permits_press(plain, Destructive), "{}", seat.id);
        assert!(
            !seat.permits_press(destructive - ANSWER_STEP, Destructive),
            "{}",
            seat.id
        );
        assert!(seat.permits_press(destructive, Destructive), "{}", seat.id);
    }
    assert!(!ROUTING.permits_press(1.0, Destructive));
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

/// What a use stands at when a person turned Jev on and chose nothing seat by
/// seat (2026-09-23, §6.1): one of the use's own modes — `auto` wherever the
/// use offers it, and the agent's own tool, which has nothing to rise on,
/// answers the agent.
///
/// `shadow` for the one seat that only ever records — the vault pair review,
/// whose proposals a person reads at the weekly review and which never rises.
/// `off` only for a seat stopped on its own evidence until it is redesigned
/// (docs/design/jev-engineering-review-20260923.md §7, t-6342): the patch
/// review and the window's worker effort. Anywhere else a switch turned on
/// that left a use off would say one thing and do another.
#[test]
fn every_use_recommends_one_of_its_own_modes_and_off_only_where_stopped() {
    let stopped = [PATCH_REVIEW.id, STEP_EFFORT.id];
    let recording = [VAULT_PAIRS.id, REFLEX_DECIDE.id];
    for row in &JEV_USES {
        assert!(
            row.modes.contains(&row.recommended),
            "{} recommends {:?}, which it does not offer",
            row.id,
            row.recommended
        );
        let expected = if stopped.contains(&row.id) {
            JevMode::Off
        } else if recording.contains(&row.id) {
            JevMode::Shadow
        } else if row.modes.contains(&JevMode::Auto) {
            JevMode::Auto
        } else {
            JevMode::On
        };
        assert_eq!(row.recommended, expected, "{}", row.id);
    }
    assert_eq!(AGENT_TOOL.recommended, JevMode::On);
}

/// A repeated run moves only the rows that say so (t-6385): the judgment
/// cache, left `auto` by word or by the switch, stands `on` when the run
/// walks what was walked before; any other word its person wrote stands; an
/// unwritten use is off in every run while Jev is switched off; and every
/// other use reads as it always did.
#[test]
fn a_repeated_run_moves_only_the_rows_that_say_so() {
    for row in &JEV_USES {
        if let Some(repeated) = row.repeat {
            assert!(
                row.modes.contains(&repeated),
                "{} repeats as {repeated:?}",
                row.id
            );
            assert_ne!(repeated, JevMode::Off, "{} turns off in a repeat", row.id);
        }
    }
    assert_eq!(
        JEV_USES
            .iter()
            .filter(|row| row.repeat.is_some())
            .map(|row| row.id)
            .collect::<Vec<_>>(),
        [JUDGMENT_CACHE.id]
    );
    let on = json!({ door::ENABLED_SETTING: true });
    let off = json!({ door::ENABLED_SETTING: false });
    let root = |jev: &Value, word: Option<&str>| {
        let mut root = json!({ SMART_SETTINGS_KEY: { door::JEV_SETTINGS_KEY: jev } });
        if let Some(word) = word {
            root[SMART_SETTINGS_KEY][JUDGMENT_CACHE.setting] = json!(word);
        }
        root
    };
    for (jev, word, fresh, repeated) in [
        (&on, None, JevMode::Auto, JevMode::On),
        (&on, Some("auto"), JevMode::Auto, JevMode::On),
        (&off, Some("auto"), JevMode::Auto, JevMode::On),
        (&on, Some("shadow"), JevMode::Shadow, JevMode::Shadow),
        (&on, Some("off"), JevMode::Off, JevMode::Off),
        (&on, Some("on"), JevMode::On, JevMode::On),
        (&off, None, JevMode::Off, JevMode::Off),
    ] {
        let root = root(jev, word);
        assert_eq!(
            JUDGMENT_CACHE.mode_in_run(&root, Run::Fresh),
            fresh,
            "{root}"
        );
        assert_eq!(
            JUDGMENT_CACHE.mode_in_run(&root, Run::Repeated),
            repeated,
            "{root}"
        );
    }
    let switched = json!({ SMART_SETTINGS_KEY: { door::JEV_SETTINGS_KEY: on } });
    for row in &JEV_USES {
        if row.repeat.is_none() {
            assert_eq!(
                row.mode_in_run(&switched, Run::Repeated),
                row.mode_in(&switched),
                "{}",
                row.id
            );
        }
    }
    assert_eq!(Run::default(), Run::Fresh);
    assert_eq!(
        (Run::Fresh.key(), Run::Repeated.key()),
        ("fresh", "repeated")
    );
}

/// The switch decides a use nobody wrote a word for: its recommendation
/// while Jev is switched on, `off` while it is off or was never touched —
/// a machine that has not met Jev asks nothing. A word a person did write is
/// theirs whatever the switch says, and a word the use does not offer is
/// `off`, as it always was.
#[test]
fn a_use_with_no_word_of_its_own_follows_the_switch() {
    let on =
        json!({ SMART_SETTINGS_KEY: { door::JEV_SETTINGS_KEY: { door::ENABLED_SETTING: true } } });
    for row in &JEV_USES {
        assert_eq!(
            row.mode_in(&on),
            row.recommended,
            "{} under the switch",
            row.id
        );
        for untouched in [
            json!({}),
            json!({ SMART_SETTINGS_KEY: {} }),
            json!({ SMART_SETTINGS_KEY: { door::JEV_SETTINGS_KEY: { door::WORKSPACES_SETTING: ["/work/app"] } } }),
            json!({ SMART_SETTINGS_KEY: { door::JEV_SETTINGS_KEY: { door::ENABLED_SETTING: false } } }),
            json!({ SMART_SETTINGS_KEY: { door::JEV_SETTINGS_KEY: { door::ENABLED_SETTING: "yes" } } }),
            json!({ SMART_SETTINGS_KEY: { door::JEV_SETTINGS_KEY: true } }),
        ] {
            assert_eq!(
                row.mode_in(&untouched),
                JevMode::Off,
                "{} in {untouched}",
                row.id
            );
        }
        // A person's own word outranks the switch in both directions.
        let written = |jev: Value, word: &str| json!({ SMART_SETTINGS_KEY: { door::JEV_SETTINGS_KEY: jev, row.setting: word } });
        assert_eq!(
            row.mode_in(&written(
                json!({ door::ENABLED_SETTING: true }),
                JevMode::Off.key()
            )),
            JevMode::Off,
            "{}: a written off stays off under the switch",
            row.id
        );
        assert_eq!(
            row.mode_in(&written(json!({ door::ENABLED_SETTING: true }), "shadwo")),
            JevMode::Off,
            "{}: a slip is off, never the recommendation",
            row.id
        );
        assert_eq!(
            row.mode_in(&written(json!({}), JevMode::Shadow.key())),
            JevMode::Shadow,
            "{}: a written word stands with the switch untouched",
            row.id
        );
    }
}

/// The skill suggestion keeps the word its person wrote for the skill
/// search until they write one of its own (t-6877 round 3, the
/// coordinator's migration contract m-8181): it read `smart.skillSearch`
/// until it was a seat of its own, so a file that says `off` there asks no
/// suggestion after the update, and `shadow`, `on` and `auto` stand as
/// written — a slip reads as `off` for both, as it did; with neither word
/// written it stands where it stood before the split, the search's reading,
/// whatever the switch says; and a word written for the suggestion is its
/// own, whatever the search's says. The search never reads the
/// suggestion's word.
#[test]
fn the_skill_suggestion_keeps_the_word_written_for_the_skill_search() {
    let document = |switch: Option<bool>, search: Option<&str>, suggestion: Option<&str>| {
        let mut smart = serde_json::Map::new();
        if let Some(on) = switch {
            smart.insert(
                door::JEV_SETTINGS_KEY.to_string(),
                json!({ door::ENABLED_SETTING: on }),
            );
        }
        if let Some(word) = search {
            smart.insert(SKILLS.setting.to_string(), json!(word));
        }
        if let Some(word) = suggestion {
            smart.insert(SKILL_SUGGESTION.setting.to_string(), json!(word));
        }
        json!({ SMART_SETTINGS_KEY: smart })
    };
    // The contract's three cases, under the switch.
    assert_eq!(
        SKILL_SUGGESTION.mode_in(&document(Some(true), Some("off"), None)),
        JevMode::Off,
        "a search turned off by hand keeps the suggestion off after the update"
    );
    assert_eq!(
        SKILL_SUGGESTION.mode_in(&document(Some(true), Some("off"), Some("auto"))),
        JevMode::Auto,
        "a word of the suggestion's own is its own"
    );
    assert_eq!(
        SKILL_SUGGESTION.mode_in(&document(Some(true), None, None)),
        SKILLS.mode_in(&document(Some(true), None, None)),
        "with neither word written, where it stood before the split"
    );
    for switch in [Some(true), Some(false), None] {
        let neither = document(switch, None, None);
        assert_eq!(
            SKILL_SUGGESTION.mode_in(&neither),
            SKILLS.mode_in(&neither),
            "{switch:?}: neither word written reads as the search did"
        );
        for search in ["off", "shadow", "on", "auto", "shadwo"] {
            let before = document(switch, Some(search), None);
            assert_eq!(
                SKILL_SUGGESTION.mode_in(&before),
                SKILLS.mode_in(&before),
                "{switch:?} {search}: the search's word stands for the suggestion"
            );
            for own in ["off", "shadow", "on", "auto"] {
                let split = document(switch, Some(search), Some(own));
                assert_eq!(
                    SKILL_SUGGESTION.mode_in(&split),
                    SKILL_SUGGESTION.mode_of(Some(&json!(own))),
                    "{switch:?} {search} {own}: the suggestion's own word"
                );
                assert_eq!(
                    SKILLS.mode_in(&split),
                    SKILLS.mode_in(&before),
                    "{switch:?} {search} {own}: the search reads its own word alone"
                );
            }
        }
        let only_the_suggestion = document(switch, None, Some("off"));
        assert_eq!(
            SKILLS.mode_in(&only_the_suggestion),
            SKILLS.mode_in(&neither),
            "{switch:?}: the search never follows the suggestion"
        );
    }
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
        Some(RECALL_APPLY_DEADLINE_MS),
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
    assert_eq!(
        hindsight,
        vec![
            RECALL.id,
            PLACEMENT.id,
            COMPACTION.id,
            PATCH_REVIEW.id,
            CLAIM.id,
            VAULT_PAIRS.id,
            FILE_PICK.id,
            COMMAND_GUARD.id,
            TOOL_TEXT_GUARD.id,
        ]
    );
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
///
/// One named exception, for the same reason turned the other way: a seat
/// whose answer nothing in the product carries out yet — the reflex decision,
/// whose apply stage is a later version's (t-9205) — offers `off | shadow`,
/// because an `on` it cannot carry out would be `shadow` under a name that
/// promises otherwise.
#[test]
fn a_seats_mode_set_is_read_off_whether_anything_labels_it() {
    let labeled: &[JevMode] = &JevMode::ALL;
    let unlabeled: &[JevMode] = &[JevMode::Off, JevMode::Shadow, JevMode::On];
    let recorded: &[JevMode] = &[JevMode::Off, JevMode::Shadow];
    let nothing_applies = [REFLEX_DECIDE.id];
    for row in JEV_USES.iter() {
        assert_eq!(
            row.promotes,
            row.agreement_rows_wanted.is_some(),
            "{}: a labeled seat names its label sample floor",
            row.id
        );
        let expected = if row.promotes {
            labeled
        } else if nothing_applies.contains(&row.id) {
            recorded
        } else {
            unlabeled
        };
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
    assert!(caps(&SKILLS).contains(&Cap::Items(SKILL_SUGGESTION_CATALOG_CAP)));
    assert!(caps(&SKILLS).contains(&Cap::Items(SKILL_SUGGESTION_SHORTLIST)));
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
    assert!(
        BROWSER_READ.permits_press(0.7, crate::guarded::ControlKind::Plain)
            && !BROWSER_READ.permits_press(0.69, crate::guarded::ControlKind::Plain)
    );
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

/// The second pass sends a bounded instruction excerpt for three candidates.
#[test]
fn the_skill_seat_discloses_the_bounded_second_pass() {
    let sent: Vec<&str> = SKILLS.sends.iter().map(|sent| sent.at).collect();
    assert_eq!(
        sent,
        vec![
            "/state/task",
            "/state/skills",
            "/state/skills/*/name",
            "/state/skills/*/description",
            "/state/candidates",
            "/state/candidates/*/excerpt",
            "/state/candidates/*/description",
            "/questions/which/criteria/*",
            "/questions/*/instructions",
        ]
    );
    assert!(!sent.iter().any(|at| at.contains("prompt")));
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

/// The routing seat sends the head of the task under its cap, by the
/// pointer the catalog's own state key makes, and the facts code wrote whole
/// (t-6346) — the door cuts a string only where a pointer names it.
#[test]
fn the_routing_seat_sends_the_task_under_its_cap_and_its_facts_whole() {
    use crate::jev::questions::{ROUTING_STATE_FACTS, ROUTING_STATE_TASK};
    let sent: Vec<(&str, Cap)> = ROUTING
        .sends
        .iter()
        .map(|sent| (sent.at, sent.cap))
        .collect();
    let task = format!("/state/{ROUTING_STATE_TASK}");
    let facts = format!("/state/{ROUTING_STATE_FACTS}");
    assert_eq!(
        sent,
        vec![
            (task.as_str(), Cap::Chars(ROUTING_TASK_CHAR_CAP)),
            (facts.as_str(), Cap::Uncut),
        ]
    );
}

/// The step governor's seat sends the same head of a turn the routing seat
/// sends, rises on its own progress marks, and offers every word — a seat
/// whose answer moves a request field is one a person can switch on and one
/// evidence can raise (docs/design/zo-step-effort-governor-20260921.md §5).
#[test]
fn the_step_effort_seat_reads_the_turn_like_routing_and_rises_on_its_own_marks() {
    assert_eq!(jev_use("step_effort"), Some(&ZO_STEP_EFFORT));
    // The same head of the turn under the same cap — as the probe rubric's
    // plain string, where the routing seat's second version sends it inside
    // an object beside its facts (t-6346).
    assert_eq!(
        ZO_STEP_EFFORT.sends,
        &[Sent {
            at: "/state",
            cap: Cap::Chars(ROUTING_TASK_CHAR_CAP),
        }]
    );
    assert!(
        ROUTING
            .sends
            .iter()
            .any(|sent| sent.cap == Cap::Chars(ROUTING_TASK_CHAR_CAP))
    );
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
        !ZO_STEP_EFFORT.permits_press(1.0, crate::guarded::ControlKind::Plain),
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
        !COMPACTION.permits_press(1.0, crate::guarded::ControlKind::Plain),
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
        !NOTIFY.permits_press(1.0, crate::guarded::ControlKind::Plain),
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
        !MENTION_RERANK.permits_press(1.0, crate::guarded::ControlKind::Plain),
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

/// Every word but `off` asks the routing seat; only the probing word calls
/// the chat probe (t-4727, t-6346).
///
/// This is the fact the card has to say out loud, so it is held to zo's
/// source: the decision shadow is fired from `probe_and_shadow` alone, the
/// spawn road reads a task whenever automatic routing runs and the seat asks
/// (`spawn_is_read`), and the probe inside that road waits on `Probed`. If zo
/// ever gates the judgment on the probe again, the card's notice becomes a lie
/// and this goes red first.
#[test]
fn the_routing_seat_is_asked_under_every_word_but_off() {
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
    let reads = &product(apply)[product(apply)
        .find("fn spawn_is_read(")
        .expect("the spawn road's one gate")..];
    let reads = &reads[..reads.find("\n}\n").expect("the gate closes")];
    assert!(
        reads.contains("RouteAutoClassifierMode::Off") && reads.contains("asks_here()"),
        "the spawn road reads a task for the seat under a word other than `off`:\n{reads}"
    );
    for entry in ["route_probe_assessment(", "route_probe_assessments("] {
        for at in product(apply).match_indices(entry).map(|(at, _)| at) {
            let before = &product(apply)[..at];
            let gate = before
                .rfind("spawn_is_read(")
                .expect("a spawn read with no gate above it");
            assert!(
                before.len() - gate < 800,
                "a `{entry}` call is not under the spawn road's gate"
            );
            let admitted = &product(apply)[at..];
            let admitted = &admitted[..admitted
                .find(')')
                .map_or(admitted.len(), |close| close + 200)
                .min(admitted.len())];
            assert!(
                admitted.contains("Admitted::spawn(probes)"),
                "a spawn read lets the probe run without the probing word"
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
    for mode in ClassifierMode::ALL {
        assert_eq!(
            mode.reaches(),
            mode.runs(),
            "`{}`: the seat is asked exactly where routing runs",
            mode.key()
        );
    }
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
    assert_eq!(JEV_USES.len(), 27);
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
    assert!(
        BRANCHING.permits_press(0.5, crate::guarded::ControlKind::Plain)
            && !BRANCHING.permits_press(0.49, crate::guarded::ControlKind::Plain)
    );
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
        !JUDGMENT_CACHE.permits_press(1.0, crate::guarded::ControlKind::Plain),
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

/// The patch review seat (t-6203) asks four Noul questions of a patch an edit
/// has just written — its hunks, the person's words and the evidence the edit
/// followed, never the file around them — and rises on hindsight: whether the
/// same lines were edited again inside its window.
#[test]
fn the_patch_review_seat_sends_a_patch_and_its_evidence_and_rises_on_hindsight() {
    assert_eq!(jev_use("patch_review"), Some(&PATCH_REVIEW));
    assert_eq!(PATCH_REVIEW.setting, "jevPatchReview");
    assert_eq!(PATCH_REVIEW.ledger, "patch-review.jsonl");
    assert_eq!(
        PATCH_REVIEW.modes,
        &[JevMode::Off, JevMode::Shadow, JevMode::On, JevMode::Auto],
        "a labeled seat offers all four words (seat contract, second correction)"
    );
    let sent: Vec<(&str, Cap)> = PATCH_REVIEW
        .sends
        .iter()
        .map(|sent| (sent.at, sent.cap))
        .collect();
    assert_eq!(
        sent,
        [
            ("/state/task", Cap::Chars(PATCH_REVIEW_TASK_CHAR_CAP)),
            ("/state/patch", Cap::Bytes(PATCH_REVIEW_PATCH_BYTE_CAP)),
            (
                "/state/evidence",
                Cap::Bytes(PATCH_REVIEW_EVIDENCE_BYTE_CAP)
            ),
            ("/state/path", Cap::Uncut),
        ],
        "the hunks and the evidence's tail: no file body, and the path only as a fingerprint"
    );
    assert_eq!(PATCH_REVIEW_TASK_CHAR_CAP, ROUTING_TASK_CHAR_CAP);

    const { assert!(PATCH_REVIEW.promotes) };
    assert_eq!(PATCH_REVIEW.agreement_kind, AgreementKind::Hindsight);
    assert_eq!(
        PATCH_REVIEW.answer_floor_permille,
        Some(PATCH_REVIEW_ANSWER_FLOOR_PERMILLE)
    );
    assert_eq!(PATCH_REVIEW_ANSWER_FLOOR_PERMILLE, 900);
    assert_eq!(
        PATCH_REVIEW.agreement_floor_permille,
        Some(PATCH_REVIEW_AGREEMENT_FLOOR_PERMILLE)
    );
    assert_eq!(
        PATCH_REVIEW.apply_deadline_ms,
        Some(PATCH_REVIEW_APPLY_DEADLINE_MS)
    );
    assert_eq!(PATCH_REVIEW.window_forgives, Some(FORGIVES_A_BAD_MINUTE));
    assert_eq!(
        PATCH_REVIEW.agreement_rows_wanted,
        Some(A_WINDOW_OF_COMPARISONS)
    );
    // The permit line is the reference harness's own 0.8, and it is not the
    // seat's answer rate: one says how sure one review must be, the other how
    // often the seat must answer at all.
    assert_eq!(PATCH_REVIEW_PERMIT_FLOOR_PERMILLE, 800);
    const {
        assert!(
            PATCH_REVIEW_PERMIT_FLOOR_PERMILLE > 500 && PATCH_REVIEW_PERMIT_FLOOR_PERMILLE < 1_000
        )
    };
    const { assert!(PATCH_REVIEW_REGRET_TURNS == 5) };
    assert!(
        !PATCH_REVIEW.permits_press(1.0, crate::guarded::ControlKind::Plain),
        "a review presses nothing and blocks nothing"
    );

    assert!(PATCH_REVIEW.mode_of(Some(&json!("on"))).applies());
    assert!(
        !PATCH_REVIEW
            .mode_of(Some(&json!("shadow")))
            .applies_with(true)
    );
    assert!(
        !PATCH_REVIEW
            .mode_of(Some(&json!("auto")))
            .applies_with(false)
    );
    assert!(
        PATCH_REVIEW
            .mode_of(Some(&json!("auto")))
            .applies_with(true)
    );
    assert_eq!(PATCH_REVIEW.mode_in(&json!({})), JevMode::Off);
    // The patch review remains before claim, the vault-pair seat, the
    // file-pick seat, the two guards and the reflex decision.
    assert_eq!(JEV_USES[JEV_USES.len() - 7], PATCH_REVIEW);
    assert_eq!(JEV_USES[JEV_USES.len() - 8], CHALLENGER);
}

#[test]
fn vault_pairs_are_recorded_for_review_without_automatic_promotion() {
    let row = jev_use("vault_pairs").expect("vault-pair seat");
    assert_eq!(row, &VAULT_PAIRS);
    assert_eq!(row.recommended, JevMode::Shadow);
    assert_eq!(row.modes, &[JevMode::Off, JevMode::Shadow, JevMode::On]);
    assert!(!row.promotes);
    assert_eq!(row.baseline, Baseline::AlwaysSame("none"));
    assert!(
        row.sends
            .iter()
            .any(|sent| sent.at == "/state/page_a/summary"
                && sent.cap == Cap::Bytes(VAULT_PAIR_SUMMARY_BYTE_CAP))
    );
}

#[test]
fn the_file_pick_seat_rises_only_by_the_judge_and_compares_with_recent_edits() {
    use crate::jev::{FILE_PICK, FILE_PICK_ANSWER_FLOOR_PERMILLE, FILE_PICK_MATCH_FLOOR_PERMILLE};

    assert_eq!(jev_use("file_pick"), Some(&FILE_PICK));
    assert_eq!(FILE_PICK.setting, "jevFilePick");
    assert_eq!(FILE_PICK.ledger, "file-pick.jsonl");
    assert_eq!(FILE_PICK.recommended, JevMode::Auto);
    assert!(jev_use("file_pick").is_some_and(|row| row.promotes));
    assert_eq!(FILE_PICK.repeat, None);
    assert_eq!(FILE_PICK.baseline, Baseline::TodaysRule);
    assert_eq!(FILE_PICK.negatives_wanted, Some(NEGATIVES_WANTED));
    assert_eq!(
        FILE_PICK.answer_floor_permille,
        Some(FILE_PICK_ANSWER_FLOOR_PERMILLE)
    );
    assert_eq!(FILE_PICK.agreement_floor_permille, Some(600));
    assert_eq!(
        FILE_PICK.confidence_bands,
        Some(ConfidenceBands::on_a_noul(
            700,
            FILE_PICK_MATCH_FLOOR_PERMILLE
        ))
    );
    assert_eq!(FILE_PICK.sends.len(), 4);
    assert_eq!(FILE_PICK.sends[0].at, "/state/request");
    assert_eq!(FILE_PICK.sends[0].cap, Cap::Chars(2_000));
    assert_eq!(FILE_PICK.sends[1].at, "/state/files");
    assert_eq!(FILE_PICK.sends[1].cap, Cap::Items(30));
    assert_eq!(FILE_PICK.sends[2].at, "/state/files/*/path");
    assert_eq!(FILE_PICK.sends[2].cap, Cap::Uncut);
    assert_eq!(FILE_PICK.sends[3].at, "/state/files/*/about");
    assert_eq!(FILE_PICK.sends[3].cap, Cap::Bytes(200));
    assert_eq!(JEV_USES.len(), 27);
    assert_eq!(JEV_USES.get(JEV_USES.len() - 4), Some(&FILE_PICK));
}

#[test]
fn completion_claims_are_a_recording_hindsight_seat_with_bounded_evidence() {
    assert_eq!(jev_use("claim"), Some(&CLAIM));
    assert_eq!(CLAIM.setting, "jevClaimCheck");
    assert_eq!(CLAIM.recommended, JevMode::Auto);
    assert_eq!(CLAIM.ledger, "claim-check.jsonl");
    assert_eq!(CLAIM.agreement_kind, AgreementKind::Hindsight);
    assert_eq!(CLAIM.baseline, Baseline::AlwaysSame(CLAIM_CRITERIA[0].0));
    assert_eq!(CLAIM.negatives_wanted, Some(NEGATIVES_WANTED));
    assert_eq!(CLAIM.sends[0].cap, Cap::Items(CLAIM_LIMIT));
    assert_eq!(CLAIM.sends[1].cap, Cap::Chars(CLAIM_TEXT_CHAR_CAP));
    assert_eq!(CLAIM.sends[2].cap, Cap::Bytes(CLAIM_EVIDENCE_BYTE_CAP));
    assert_eq!(JEV_USES.get(JEV_USES.len() - 6), Some(&CLAIM));
}

/// The command guard (t-6348): right before zo runs a shell command, the
/// command, the folder it runs in and the first line of the person's newest
/// words are put to two Nouls in one request — cannot it be undone, does it
/// change something outside the project. It records first, is graded by
/// what became of the command, and is held to today's rule: the destructive
/// and path tables zo already warns from, and the Computer Use table of
/// words a control that cannot be taken back carries.
#[test]
fn the_command_guard_sends_a_command_its_folder_and_a_task_line_and_rises_on_hindsight() {
    use crate::jev::questions::COMMAND_GUARD_STATE_KEYS;

    assert_eq!(jev_use("command_guard"), Some(&COMMAND_GUARD));
    assert_eq!(COMMAND_GUARD.setting, "jevCommandGuard");
    assert_eq!(COMMAND_GUARD.ledger, "command-guard.jsonl");
    assert_eq!(COMMAND_GUARD.modes, &JevMode::ALL[..]);
    assert_eq!(COMMAND_GUARD.recommended, JevMode::Auto);
    assert_eq!(COMMAND_GUARD.repeat, None);
    assert!(jev_use("command_guard").is_some_and(|row| row.promotes));
    assert_eq!(COMMAND_GUARD.agreement_kind, AgreementKind::Hindsight);
    assert_eq!(COMMAND_GUARD.baseline, Baseline::TodaysRule);
    assert_eq!(COMMAND_GUARD.negatives_wanted, Some(NEGATIVES_WANTED));
    assert_eq!(COMMAND_GUARD.press_floor_permille, None);
    assert_eq!(
        COMMAND_GUARD.answer_floor_permille,
        Some(COMMAND_GUARD_ANSWER_FLOOR_PERMILLE)
    );
    assert_eq!(
        COMMAND_GUARD.agreement_floor_permille,
        Some(COMMAND_GUARD_AGREEMENT_FLOOR_PERMILLE)
    );
    assert_eq!(
        COMMAND_GUARD.apply_deadline_ms,
        Some(COMMAND_GUARD_APPLY_DEADLINE_MS)
    );
    assert_eq!(
        COMMAND_GUARD.confidence_bands,
        Some(ConfidenceBands::on_a_noul(
            NOUL_UNCERTAIN_TO_PERMILLE,
            COMMAND_GUARD_FLAG_FLOOR_PERMILLE
        ))
    );
    let sent: Vec<(&str, Cap)> = COMMAND_GUARD
        .sends
        .iter()
        .map(|sent| (sent.at, sent.cap))
        .collect();
    assert_eq!(
        sent,
        vec![
            ("/state/command", Cap::Chars(COMMAND_GUARD_COMMAND_CHAR_CAP)),
            ("/state/cwd", Cap::Uncut),
            ("/state/task", Cap::Chars(COMMAND_GUARD_TASK_CHAR_CAP)),
        ]
    );
    // The pointers are the catalog's own state keys, in its order.
    for (sent, key) in COMMAND_GUARD.sends.iter().zip(COMMAND_GUARD_STATE_KEYS) {
        assert_eq!(sent.at, format!("/state/{key}"));
    }
    // A thousand characters holds 91.0% of this machine's 2,318 commands of a
    // week whole; the task line is a goal's sentence or two.
    assert_eq!(COMMAND_GUARD_COMMAND_CHAR_CAP, RECALL_REQUEST_CHAR_CAP);
    assert_eq!(COMMAND_GUARD_TASK_CHAR_CAP, GOAL_CHAR_CAP);
    assert_eq!(COMMAND_GUARD.mode_in(&json!({})), JevMode::Off);
    assert_eq!(JEV_USES.get(JEV_USES.len() - 3), Some(&COMMAND_GUARD));
}

/// The tool text guard (t-6348): the screen's instructions guard asked of
/// every block a file read, a web fetch, the window's browser or an MCP tool
/// hands back — the head of the block and the kind of tool, one Noul, the
/// screen's own line. It records first, is graded by whether the agent's next
/// call carried out what the block said, and is held to today's rule: the
/// fence the window already puts around the words it did not write.
#[test]
fn the_tool_text_guard_sends_a_block_head_and_its_source_and_rises_on_hindsight() {
    use crate::jev::questions::TOOL_TEXT_GUARD_STATE_KEYS;

    assert_eq!(jev_use("tool_text_guard"), Some(&TOOL_TEXT_GUARD));
    assert_eq!(TOOL_TEXT_GUARD.setting, "jevToolTextGuard");
    assert_eq!(TOOL_TEXT_GUARD.ledger, "tool-text-guard.jsonl");
    assert_eq!(TOOL_TEXT_GUARD.modes, &JevMode::ALL[..]);
    assert_eq!(TOOL_TEXT_GUARD.recommended, JevMode::Auto);
    assert_eq!(TOOL_TEXT_GUARD.repeat, None);
    assert!(jev_use("tool_text_guard").is_some_and(|row| row.promotes));
    assert_eq!(TOOL_TEXT_GUARD.agreement_kind, AgreementKind::Hindsight);
    assert_eq!(TOOL_TEXT_GUARD.baseline, Baseline::TodaysRule);
    assert_eq!(TOOL_TEXT_GUARD.negatives_wanted, Some(NEGATIVES_WANTED));
    assert_eq!(TOOL_TEXT_GUARD.press_floor_permille, None);
    assert_eq!(
        TOOL_TEXT_GUARD.apply_deadline_ms,
        Some(TOOL_TEXT_GUARD_APPLY_DEADLINE_MS)
    );
    // The screen's question asked of another text is held to the screen's line.
    assert_eq!(
        TOOL_TEXT_INSTRUCTED_FLOOR_PERMILLE,
        SCREEN_INSTRUCTED_FLOOR_PERMILLE
    );
    assert_eq!(
        TOOL_TEXT_GUARD.confidence_bands,
        Some(ConfidenceBands::on_a_noul(
            NOUL_UNCERTAIN_TO_PERMILLE,
            TOOL_TEXT_INSTRUCTED_FLOOR_PERMILLE
        ))
    );
    let sent: Vec<(&str, Cap)> = TOOL_TEXT_GUARD
        .sends
        .iter()
        .map(|sent| (sent.at, sent.cap))
        .collect();
    assert_eq!(
        sent,
        vec![
            ("/state/source", Cap::Uncut),
            ("/state/text", Cap::Chars(TOOL_TEXT_GUARD_TEXT_CHAR_CAP)),
        ]
    );
    for (sent, key) in TOOL_TEXT_GUARD.sends.iter().zip(TOOL_TEXT_GUARD_STATE_KEYS) {
        assert_eq!(sent.at, format!("/state/{key}"));
    }
    assert_eq!(TOOL_TEXT_GUARD_TEXT_CHAR_CAP, ROUTING_TASK_CHAR_CAP);
    assert_eq!(TOOL_TEXT_GUARD.mode_in(&json!({})), JevMode::Off);
    assert_eq!(JEV_USES.get(JEV_USES.len() - 2), Some(&TOOL_TEXT_GUARD));
}

/// The reflex decision (t-9205): a live reflex run's typed state — each
/// detector's newest sighting and how its actions ended — put to one closed
/// choice, `continue`, `pause` or `replan`. It records the teacher's answer
/// and nothing else: `shadow` is the most it offers, it never promotes, names
/// no floor, wall or band, and its wire waits one lease.
#[test]
fn the_reflex_decision_records_teacher_labels_and_never_rises() {
    use crate::computer_use_protocol::reflex::LIMITS;
    use crate::jev::questions::{REFLEX_DECIDE_OPTIONS, REFLEX_DECIDE_STATE_KEYS};

    assert_eq!(jev_use("reflex_decide"), Some(&REFLEX_DECIDE));
    assert_eq!(REFLEX_DECIDE.setting, "jevReflexDecide");
    assert_eq!(REFLEX_DECIDE.ledger, "reflex-decide.jsonl");
    assert_eq!(REFLEX_DECIDE.modes, &[JevMode::Off, JevMode::Shadow]);
    assert_eq!(REFLEX_DECIDE.recommended, JevMode::Shadow);
    assert_eq!(REFLEX_DECIDE.repeat, None);
    const { assert!(!REFLEX_DECIDE.promotes) };
    assert!(
        REFLEX_DECIDE
            .modes
            .iter()
            .all(|mode| !mode.applies_with(true)),
        "no mode it offers acts"
    );
    assert_eq!(REFLEX_DECIDE.apply_deadline_ms, None);
    assert_eq!(REFLEX_DECIDE.confidence_bands, None);
    assert_eq!(
        REFLEX_DECIDE_DEADLINE_MS * 1_000_000,
        LIMITS.max_lease_ns,
        "one lease on the wire"
    );
    assert_eq!(
        REFLEX_DECIDE_OPTIONS.map(|(word, _)| word),
        ["continue", "pause", "replan"]
    );
    for key in REFLEX_DECIDE_STATE_KEYS {
        assert!(
            REFLEX_DECIDE
                .sends
                .iter()
                .any(|sent| sent.at == format!("/state/{key}")),
            "{key}"
        );
    }
    assert_eq!(JEV_USES.last(), Some(&REFLEX_DECIDE));
}

/// The desktop's consent — its word, whatever it says — never switches the
/// reflex decision on, and neither does any other seat's: only its own word
/// or the one switch does, and even then it only records. A word it does not
/// offer (`on`, `auto`) reads as off.
#[test]
fn desktop_consent_never_enables_reflex_decide() {
    for row in JEV_USES.iter().filter(|row| row.id != REFLEX_DECIDE.id) {
        for word in ["on", "auto", "shadow"] {
            let root = json!({ SMART_SETTINGS_KEY: { row.setting: word } });
            assert_eq!(
                REFLEX_DECIDE.mode_in(&root),
                JevMode::Off,
                "{} {word}",
                row.id
            );
        }
    }
    let desktop = json!({ SMART_SETTINGS_KEY: { DESKTOP.setting: "on", EMULATOR.setting: "on" } });
    assert_eq!(DESKTOP.mode_in(&desktop), JevMode::On);
    assert_eq!(
        REFLEX_DECIDE.mode_in(&desktop),
        JevMode::Off,
        "the desktop's `on` is the desktop's"
    );
    for (word, mode) in [
        ("shadow", JevMode::Shadow),
        ("on", JevMode::Off),
        ("auto", JevMode::Off),
        ("off", JevMode::Off),
    ] {
        let root = json!({ SMART_SETTINGS_KEY: { REFLEX_DECIDE.setting: word } });
        assert_eq!(REFLEX_DECIDE.mode_in(&root), mode, "{word}");
    }
    // The one switch stands it at its recommendation, which only records.
    let on =
        json!({ SMART_SETTINGS_KEY: { door::JEV_SETTINGS_KEY: { door::ENABLED_SETTING: true } } });
    assert_eq!(REFLEX_DECIDE.mode_in(&on), JevMode::Shadow);
    assert!(!REFLEX_DECIDE.mode_in(&on).applies_with(true));
}

/// An answer is read by the option's word — whatever place that option stood
/// in when it was asked — and a position, a letter or any label the question
/// never offered is refused, never mapped onto an option: reordering or
/// relabelling the options cannot point an answer at another action.
#[test]
fn choice_label_permutation_cannot_retarget_an_action() {
    use crate::jev::questions::{
        REFLEX_DECIDE_ASKS, REFLEX_DECIDE_OPTIONS, REFLEX_DECIDE_QUESTION,
    };
    use std::collections::BTreeSet;

    let offered: BTreeSet<String> = REFLEX_DECIDE_OPTIONS
        .iter()
        .map(|(word, _)| (*word).to_string())
        .collect();
    let orders: [[usize; 3]; 6] = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];
    for order in orders {
        let criteria: serde_json::Map<String, Value> = order
            .iter()
            .map(|at| {
                let (word, covers) = REFLEX_DECIDE_OPTIONS[*at];
                (word.to_string(), Value::from(covers))
            })
            .collect();
        let asked = choice::asked(REFLEX_DECIDE_QUESTION, REFLEX_DECIDE_ASKS, criteria);
        let named: BTreeSet<String> = asked[REFLEX_DECIDE_QUESTION]["criteria"]
            .as_object()
            .expect("criteria")
            .keys()
            .cloned()
            .collect();
        assert_eq!(named, offered, "{order:?}: every option asked by its word");
        for (word, _) in REFLEX_DECIDE_OPTIONS {
            let probabilities: serde_json::Map<String, Value> = order
                .iter()
                .map(|at| {
                    let (other, _) = REFLEX_DECIDE_OPTIONS[*at];
                    (
                        other.to_string(),
                        json!(if other == word { 0.8 } else { 0.1 }),
                    )
                })
                .collect();
            let answer = json!({ REFLEX_DECIDE_QUESTION: {
                "type": "choice", "choice": word, "probabilities": probabilities, "confidence": 0.7
            } });
            let read = choice::read(&answer, REFLEX_DECIDE_QUESTION, &offered)
                .expect("a well-formed answer");
            assert_eq!(read.chosen, word, "{order:?}");
        }
    }
    for label in ["0", "1", "2", "A", "B", "C", "option_1", "Continue", ""] {
        let answer = json!({ REFLEX_DECIDE_QUESTION: {
            "type": "choice", "choice": label,
            "probabilities": { "continue": 0.8, "pause": 0.1, "replan": 0.1 }, "confidence": 0.7
        } });
        assert_eq!(
            choice::read(&answer, REFLEX_DECIDE_QUESTION, &offered),
            Err(choice::ChoiceRefusal::UnknownOption),
            "{label:?} names no option"
        );
    }
}

/// What a reflex decision sends is the run's typed state and nothing else:
/// the table declares only the sightings and the outcome counts, a detector's
/// name is cut at the plan's own id bound and the list at the plan's detector
/// bound — the door's own clearing — and no pointer reaches a pixel, a
/// screen's words or an app.
#[test]
fn reflex_decide_sends_no_pixels_text_or_app_names() {
    use crate::computer_use_protocol::reflex::{LIMITS, MAX_IDENTIFIER_BYTES};

    let pointers: Vec<&str> = REFLEX_DECIDE.sends.iter().map(|sent| sent.at).collect();
    assert_eq!(
        pointers,
        [
            "/state/sightings",
            "/state/sightings/*/detector",
            "/state/sightings/*/unknown",
            "/state/outcomes",
        ]
    );
    for sent in REFLEX_DECIDE.sends {
        for word in [
            "pixel", "image", "png", "text", "title", "app", "window", "scope", "target", "screen",
            "label",
        ] {
            assert!(!sent.at.contains(word), "{} names {word}", sent.at);
        }
    }
    assert_eq!(
        REFLEX_DECIDE.sends[0].cap,
        Cap::Items(LIMITS.max_detectors as usize)
    );
    assert_eq!(REFLEX_DECIDE.sends[1].cap, Cap::Bytes(MAX_IDENTIFIER_BYTES));
    // Through the real door: a list past the plan's detectors and a name past
    // the id bound are cut where the table says.
    let settings = door::JevSettings::from_root(&json!({ SMART_SETTINGS_KEY: {
        door::JEV_SETTINGS_KEY: { door::ENABLED_SETTING: true, door::WORKSPACES_SETTING: [door::EVERY_WORKSPACE] }
    } }));
    let long = "d".repeat(MAX_IDENTIFIER_BYTES * 3);
    let sightings: Vec<Value> = (0..LIMITS.max_detectors + 4)
        .map(|_| json!({ "detector": long, "value": 1, "unknown": null, "track": 5, "age_ms": 3 }))
        .collect();
    let body = json!({ "state": { "sightings": sightings, "outcomes": { "done": 12 } } });
    let asking = door::Asking {
        key: true,
        settings: &settings,
        workspace: Some("/any"),
        sent_today: 0,
    };
    let cleared = door::may_send(&REFLEX_DECIDE, &asking, body).expect("the door lets it through");
    let sent: Value = serde_json::from_slice(cleared.bytes()).expect("json");
    let kept = sent["state"]["sightings"].as_array().expect("sightings");
    assert_eq!(kept.len() as u64, LIMITS.max_detectors);
    let name = kept[0]["detector"].as_str().expect("a name");
    assert!(
        name.len() <= MAX_IDENTIFIER_BYTES + CUT_MARK.len(),
        "{} bytes",
        name.len()
    );
}

/// A request's receipt is the whole SHA-256 of the seat, the rubric version,
/// the model asked for and the cleared bytes — each part framed by its
/// length, so no two different requests share one digest by sliding bytes
/// from one part into the next.
#[test]
fn a_request_digest_vouches_for_the_seat_the_rubric_the_model_and_the_bytes() {
    let digest = digest_of("patch_review", 1, "jev-1.13.0", br#"{"state":{}}"#);
    assert_eq!(digest.len(), 64, "the whole SHA-256: {digest}");
    assert!(
        digest
            .chars()
            .all(|glyph| glyph.is_ascii_hexdigit() && !glyph.is_ascii_uppercase())
    );
    assert_eq!(
        digest,
        digest_of("patch_review", 1, "jev-1.13.0", br#"{"state":{}}"#),
        "the same request, the same receipt"
    );
    for other in [
        digest_of("compaction", 1, "jev-1.13.0", br#"{"state":{}}"#),
        digest_of("patch_review", 2, "jev-1.13.0", br#"{"state":{}}"#),
        digest_of("patch_review", 1, "jev-latest", br#"{"state":{}}"#),
        digest_of("patch_review", 1, "jev-1.13.0", br#"{"state":{"a":1}}"#),
    ] {
        assert_ne!(digest, other, "every part is in the receipt");
    }
    // Framed: a byte moved from one part into the next is another request.
    assert_ne!(digest_of("ab", 1, "c", b"d"), digest_of("a", 1, "bc", b"d"));
    assert_ne!(digest_of("a", 1, "b", b"cd"), digest_of("a", 1, "bc", b"d"));
    // Pinned, so an offline replay that frames the parts as the doc says
    // reproduces it byte for byte.
    assert_eq!(
        digest_of("", 0, "", b""),
        "353e4a6a2987c8ad2380e1971e961cfe482d00e5e599e3e1a296ea28a5374ddc"
    );
    // The fingerprint beside it is the same hasher, cut to sixteen digits.
    assert_eq!(fingerprint_of("").len(), 16);
    assert_eq!(
        fingerprint_of(""),
        "e3b0c44298fc1c14",
        "SHA-256 of nothing, cut"
    );
}

/// A seat with comparison labels names the cheapest reader, the negative
/// sample requirement, and its confidence bands. Vault pairs records those
/// facts for weekly review even though it never promotes automatically.
#[test]
fn every_promoting_row_names_a_baseline_and_an_abstain_band() {
    for row in &JEV_USES {
        let compared = row.promotes || row.id == VAULT_PAIRS.id;
        assert_eq!(
            compared,
            row.negatives_wanted.is_some(),
            "{} promotes={} negatives={:?}",
            row.id,
            row.promotes,
            row.negatives_wanted
        );
        assert_eq!(
            compared,
            row.confidence_bands.is_some(),
            "{} promotes={} bands={:?}",
            row.id,
            row.promotes,
            row.confidence_bands
        );
        if let Some(bands) = row.confidence_bands {
            assert!(
                bands.abstain_below_permille <= bands.act_from_permille
                    && bands.act_from_permille <= 1_000,
                "{} {bands:?}",
                row.id
            );
        }
        if let Some(wanted) = row.negatives_wanted {
            assert_eq!(wanted, NEGATIVES_WANTED, "{}", row.id);
        }
        if !compared {
            assert_eq!(row.baseline, Baseline::None, "{}", row.id);
        }
        if let Baseline::AlwaysSame(word) = row.baseline {
            assert!(!word.trim().is_empty(), "{}", row.id);
        }
    }
    // The two constant answers are words their own seats write.
    assert_eq!(
        STALL.baseline,
        Baseline::AlwaysSame(crate::stall_cause::Cause::LongRunningTool.word())
    );
    assert_eq!(
        PATCH_REVIEW.baseline,
        Baseline::AlwaysSame(PATCH_REVIEW_PERMIT)
    );
    // A seat whose marks grade only its own act has no cheaper reader.
    for seat in [
        &BROWSER,
        &DESKTOP,
        &EMULATOR,
        &BROWSER_READ,
        &JUDGMENT_CACHE,
    ] {
        assert_eq!(seat.baseline, Baseline::None, "{}", seat.id);
    }
    assert_eq!(Baseline::TodaysRule.kind(), "todays_rule");
    assert!(!Baseline::None.binds());
    // What `NEGATIVES_WANTED` says it costs: forty marks at the 800‰ line.
    assert_eq!(promote::marks_that_can_clear(&PLACEMENT), Some(40));
    assert_eq!(promote::marks_that_can_clear(&AGENT_TOOL), None);
}

/// A seat that presses acts from its press floor and has nothing between:
/// inside a walk there is nobody to confirm a press with, so the band that
/// acts is exactly the answers the press gate lets through (t-6342).
#[test]
fn a_pressing_seat_acts_from_its_press_floor_with_nothing_between() {
    for row in JEV_USES
        .iter()
        .filter(|row| row.press_floor_permille.is_some())
    {
        let floor = row.press_floor_permille.expect("filtered");
        assert_eq!(
            row.confidence_bands,
            Some(ConfidenceBands::pressing(floor)),
            "{}",
            row.id
        );
        for confidence in [0.0, 0.29, 0.499_999, 0.5, 0.699_999, 0.7, 0.95, 1.0] {
            assert_eq!(
                row.band_of(confidence) == Some(Band::Act),
                row.permits_press(confidence, crate::guarded::ControlKind::Plain),
                "{} at {confidence}",
                row.id
            );
            assert_ne!(row.band_of(confidence), Some(Band::Confirm), "{}", row.id);
        }
    }
}

/// A seat's act line moves what its stage acts on and nothing else
/// (t-9468). With no line every seat acts exactly as it did: a band is its
/// bands', a press is its press floor's, and a seat with no press floor acts
/// on whatever it answered. With a line, an answer acts from the line — a
/// band's act line moves and its abstain line never rises over it, a plain
/// press asks the line and a destructive one still asks nine in ten, and a
/// seat with no press floor still presses nothing. Only a seat that promotes
/// and names bands has a stage that reads a line.
#[test]
fn an_act_line_moves_what_a_stage_acts_on_and_nothing_else() {
    use crate::guarded::ControlKind::{Destructive, Plain};
    let confidences = [
        0.0, 0.29, 0.3, 0.499_999, 0.5, 0.6, 0.699_999, 0.7, 0.85, 0.9, 1.0,
    ];
    for row in &JEV_USES {
        for confidence in confidences {
            assert_eq!(
                row.band_at(confidence, None),
                row.band_of(confidence),
                "{}",
                row.id
            );
            assert_eq!(
                row.acts_on(confidence, None),
                row.press_floor_permille.is_none() || row.permits_press(confidence, Plain),
                "{} at {confidence}: today's gate",
                row.id
            );
            for kind in [Plain, Destructive] {
                assert_eq!(
                    row.permits_press_at(confidence, kind, None),
                    row.permits_press(confidence, kind),
                    "{}",
                    row.id
                );
            }
            let line = 300;
            assert_eq!(
                row.acts_on(confidence, Some(line)),
                reaches(confidence, line),
                "{}",
                row.id
            );
            assert_eq!(
                row.permits_press_at(confidence, Plain, Some(line)),
                row.press_floor_permille.is_some() && reaches(confidence, line),
                "{}: a line grants no press",
                row.id
            );
            assert_eq!(
                row.permits_press_at(confidence, Destructive, Some(line)),
                row.press_floor_permille.is_some()
                    && reaches(confidence, SCREEN_DESTRUCTIVE_PRESS_FLOOR_PERMILLE),
                "{}: a press that cannot be taken back asks nine in ten",
                row.id
            );
            if let Some(bands) = row.confidence_bands {
                let band = row.band_at(confidence, Some(line));
                assert_eq!(
                    band == Some(Band::Act),
                    reaches(confidence, line),
                    "{}",
                    row.id
                );
                assert_eq!(
                    band == Some(Band::Abstain),
                    !reaches(confidence, line.min(bands.abstain_below_permille)),
                    "{} at {confidence}",
                    row.id
                );
            }
        }
        if row.reads_act_line {
            assert!(
                row.promotes && row.confidence_bands.is_some(),
                "{}: a stage that reads a line has one to read",
                row.id
            );
        }
    }
    assert!(!reaches(f64::NAN, 0) && !reaches(1.01, 0) && reaches(1.0, 1_000));
}

/// One answer's confidence falls in exactly one band: under the first line
/// it abstains, from the second it acts, and between the two it wants a
/// confirmation — the three bands of TypeSafe's confidence-routing pattern
/// at the pattern's own lines (t-6342). A reading outside 0..=1 is none.
#[test]
fn an_answer_falls_in_one_band_by_its_confidence() {
    let routed = ConfidenceBands::ROUTED;
    assert_eq!(
        (routed.abstain_below_permille, routed.act_from_permille),
        (600, 850)
    );
    for (confidence, band) in [
        (0.0, Band::Abstain),
        (0.599, Band::Abstain),
        (0.6, Band::Confirm),
        (0.849, Band::Confirm),
        (0.85, Band::Act),
        (1.0, Band::Act),
    ] {
        assert_eq!(routed.band_of(confidence), Some(band), "{confidence}");
    }
    for stray in [-0.1, 1.01, f64::NAN, f64::INFINITY] {
        assert_eq!(routed.band_of(stray), None, "{stray}");
    }
    // A seat whose wrong act the person undoes in one move acts from the
    // same floor with nothing between.
    let low = ConfidenceBands::LOW_STAKES;
    assert_eq!(low.band_of(0.6), Some(Band::Act));
    assert_eq!(low.band_of(0.599), Some(Band::Abstain));
    assert_eq!(Band::ALL.map(Band::word), ["abstain", "confirm", "act"]);
    // A seat that decides on Nouls reads a Noul's lean |2p − 1|: the
    // cookbook's uncertain 0.30–0.70 is a lean under 400‰, and the patch
    // review's 800‰ permit line a lean of 600‰.
    assert_eq!(
        PATCH_REVIEW.confidence_bands,
        Some(ConfidenceBands {
            abstain_below_permille: 400,
            act_from_permille: 600
        })
    );
    assert_eq!(
        ConfidenceBands::on_a_noul(300, 200),
        ConfidenceBands::on_a_noul(
            NOUL_UNCERTAIN_TO_PERMILLE,
            PATCH_REVIEW_PERMIT_FLOOR_PERMILLE
        ),
        "a no leans as far as the yes it mirrors"
    );
    // A seat that never rises reads no band.
    assert_eq!(AGENT_TOOL.band_of(0.99), None);
}

/// The language column (t-6324 §6-1, t-6346): how much of a request's
/// letters are Hangul, per thousand — counted by code, so a seat's agreement
/// can be read apart by language without keeping a word of the text.
#[test]
fn a_requests_hangul_share_is_counted_over_its_letters() {
    assert_eq!(
        hangul_share_permille("이 함수의 버그를 수정해줘"),
        Some(1_000)
    );
    assert_eq!(hangul_share_permille("fix the bug"), Some(0));
    assert_eq!(
        hangul_share_permille("fix 버그"),
        Some(400),
        "two of five letters"
    );
    assert_eq!(
        hangul_share_permille("ㄱㄴ ab"),
        Some(500),
        "jamo are Hangul too"
    );
    assert_eq!(
        hangul_share_permille("123 !? -"),
        None,
        "no letters, no share"
    );
    assert_eq!(hangul_share_permille(""), None);
}

/// A seat that never rises names no apply wall (the column's own contract,
/// [`JevUse::apply_deadline_ms`]): the wall is the latency line a rising seat
/// is judged against, and a number nobody is judged on only tells a screen
/// there is a stage to time. zo's summary already held every row to it; the
/// vault-pair seat broke it on arrival (t-6345) and no core test said so.
#[test]
fn a_seat_that_never_rises_names_no_apply_wall() {
    for row in &JEV_USES {
        assert_eq!(
            row.apply_deadline_ms.is_some(),
            row.promotes,
            "{} promotes={} wall={:?}",
            row.id,
            row.promotes,
            row.apply_deadline_ms
        );
    }
}

/// Every seat's row names the words it asks now and how its labels name a
/// request (t-6877): one rubric version — a version, never zero — and
/// naming keys that are plain words. The judge reads a seat's ledger as
/// one rubric's series by these two columns; a seat is one question, so no
/// two seats write one ledger, and the skills seat's two questions are two
/// rows, each pointing at the constant its writer stamps.
#[test]
fn every_seat_names_its_rubric_and_how_its_labels_name_a_request() {
    use crate::jev::questions::UNVERSIONED_RUBRIC;
    assert_eq!(
        UNVERSIONED_RUBRIC, 1,
        "a row that names no version is the first rubric's"
    );
    for row in &JEV_USES {
        assert!(
            row.rubric_version >= UNVERSIONED_RUBRIC,
            "{}: zero is not a version",
            row.id
        );
        for key in row.request_name {
            assert!(
                !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric()),
                "{}: {key:?} is not a key a request row carries",
                row.id
            );
        }
    }
    assert_eq!(
        SKILLS.rubric_version,
        questions::SKILL_SEARCH_RUBRIC_VERSION
    );
    assert_eq!(
        SKILL_SUGGESTION.rubric_version,
        questions::SKILL_SUGGESTION_RUBRIC_VERSION
    );
    assert_ne!(
        SKILLS.rubric_version, SKILL_SUGGESTION.rubric_version,
        "two questions, two rubrics"
    );
    assert_ne!(SKILLS.ledger, SKILL_SUGGESTION.ledger, "and two ledgers");
    assert_eq!(SKILLS.sends, SKILL_SUGGESTION.sends, "cut at one door");
    assert_eq!(SKILLS.request_name, &["task", "catalog"]);
    assert_eq!(SKILL_SUGGESTION.request_name, SKILLS.request_name);
    assert_eq!(
        TOOL_TEXT_GUARD.rubric_version,
        questions::TOOL_TEXT_GUARD_RUBRIC_VERSION
    );
    assert_eq!(TOOL_TEXT_GUARD.request_name, &["judged"]);
    assert_eq!(ROUTING.rubric_version, questions::ROUTING_RUBRIC_VERSION);
    assert_eq!(
        CHALLENGER.rubric_version,
        questions::CHALLENGER_RUBRIC_VERSION,
        "the number the arm stamps on every row that asked"
    );
    assert_eq!(RECALL.request_name, &["query", "notes"]);
    assert_eq!(
        ZO_STEP_EFFORT.request_name,
        &["attempt", "step"],
        "a progress mark names the judgment it grades by the turn and the step it was asked at"
    );
}

/// What a request's name picks out is the table's to say, seat by seat
/// (t-6877 round 3, astra R1b): the routing seat's attempt names a turn of
/// several judgments; the recall, mention and both skills seats name the
/// words asked, which the same words asked again carry again — so their
/// labels name the time of the asking they grade; every other seat's name
/// is an id its writer made for one request. A label grades a part of its
/// request only where the request has several — the compaction seat's
/// dropped blocks — and a part's keys are keys a label row carries.
#[test]
fn every_seat_says_what_its_request_name_picks_out() {
    for row in &JEV_USES {
        let expected = match row.id {
            id if id == ROUTING.id => Naming::Turn,
            id if [RECALL.id, MENTION_RERANK.id, SKILLS.id, SKILL_SUGGESTION.id].contains(&id) => {
                Naming::Words
            }
            _ => Naming::Request,
        };
        assert_eq!(row.names, expected, "{}", row.id);
        if row.names != Naming::Request {
            assert!(
                !row.request_name.is_empty(),
                "{}: a name that picks out words or a turn is carried under some key",
                row.id
            );
        }
        for key in row.label_part {
            assert!(
                !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric()),
                "{}: {key:?} is not a key a label row carries",
                row.id
            );
        }
        let parts: &[&str] = if row.id == COMPACTION.id {
            &["block"]
        } else {
            &[]
        };
        assert_eq!(row.label_part, parts, "{}", row.id);
    }
}

/// A seat split off another follows a word written for the other, and only
/// a seat that was (t-6877 round 3, m-8181): the skill suggestion follows
/// the search's setting — another row's, never its own — reads every word
/// the search offers as the search reads it, and the search follows
/// nothing, so a word is followed one step and never round a circle.
#[test]
fn a_seat_follows_only_the_seat_it_was_split_from() {
    for row in &JEV_USES {
        let Some(followed) = row.follows else {
            continue;
        };
        assert_eq!(
            (row.id, followed),
            (SKILL_SUGGESTION.id, SKILLS.setting),
            "only the skill suggestion was split from another seat"
        );
        let from = JEV_USES
            .iter()
            .find(|other| other.setting == followed)
            .expect("the followed setting is a row's");
        assert_ne!(from.id, row.id, "a seat does not follow itself");
        assert_eq!(
            from.follows, None,
            "{}: a followed seat follows nothing",
            from.id
        );
        for mode in from.modes {
            assert!(
                row.modes.contains(mode),
                "{}: {} is a word {} offers and this seat would read as off",
                row.id,
                mode.key(),
                from.id
            );
        }
    }
    assert_eq!(SKILL_SUGGESTION.follows, Some(SKILLS.setting));
}
