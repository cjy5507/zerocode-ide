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
