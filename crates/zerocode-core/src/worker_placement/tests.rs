//! What the placement question promises the caller that asks it.

use serde_json::json;

use super::*;

fn look(may_split: bool, in_front: InFront, same_workspace: bool) -> PlacementLook<'static> {
    PlacementLook {
        brief: "이 화면 버그 좀 봐줘.",
        started_by: StartedBy::Person,
        in_front,
        same_workspace,
        panes: 2,
        may_split,
    }
}

#[test]
fn the_version_is_pinned_to_the_words() {
    // Changing a word of the question without bumping the version turns this
    // red: a judgment read under one wording is not evidence about another.
    // Version 2 is version 1's words under a label that grades an answer only
    // on a pane that tried its room (t-9427), so the fingerprint stands.
    assert_eq!(WORKER_PLACEMENT_RUBRIC_VERSION, 2);
    assert_eq!(
        crate::jev::rubric_fingerprint(rubric_words),
        "da5b36e59acebce8"
    );
}

/// A closed choice is only closed if every option the caller could be told to
/// take is one it can carry out. `split` is the only room with a condition.
#[test]
fn split_is_offered_only_where_the_window_could_carry_it_out() {
    assert_eq!(
        ask(&look(true, InFront::Terminal, true)).options(),
        ["tab", "split", "background"],
        "room, a terminal in front, the same checkout"
    );
    for barred in [
        look(false, InFront::Terminal, true),
        look(true, InFront::Page, true),
        look(true, InFront::Nothing, true),
        look(true, InFront::Terminal, false),
    ] {
        assert_eq!(
            ask(&barred).options(),
            ["tab", "background"],
            "{barred:?} cannot be split into"
        );
    }
}

/// The reader judges against the set THIS question offered, not against every
/// word the table knows — so a room that was barred cannot be chosen by an
/// answer that names it anyway.
#[test]
fn an_answer_is_judged_against_the_set_that_was_asked() {
    let asked = ask(&look(false, InFront::Terminal, true));
    let answers = json!({
        "placement": {
            "type": "choice",
            "choice": "split",
            "probabilities": { "tab": 0.5, "background": 0.5 },
            "confidence": 0.9,
        }
    });
    assert_eq!(
        asked.read(&answers),
        Err(crate::jev::choice::ChoiceRefusal::UnknownOption),
        "split was not offered to this window"
    );

    let honest = json!({
        "placement": {
            "type": "choice",
            "choice": "background",
            "probabilities": { "tab": 0.25, "background": 0.75 },
            "confidence": 0.75,
        }
    });
    let read = asked.read(&honest).expect("an offered room reads");
    assert_eq!(read.chosen, Placement::Background);
    assert!((read.confidence - 0.75).abs() < 1e-9);
}

/// The state recorded beside an answer is the state that was asked about, so
/// the cut happens here as well as at the door. A character cap cuts
/// silently — the mark is the byte caps' — so what is recorded is exactly the
/// head the question carried.
#[test]
fn the_brief_is_cut_to_the_tables_cap() {
    let long = "가".repeat(PLACEMENT_BRIEF_CHAR_CAP + 50);
    let mut look = look(true, InFront::Terminal, true);
    look.brief = &long;
    let brief = ask(&look).state["brief"].as_str().unwrap().to_string();
    assert_eq!(brief.chars().count(), PLACEMENT_BRIEF_CHAR_CAP);
    assert!(!brief.ends_with(crate::jev::CUT_MARK));
    assert_eq!(
        ask(&look).state["brief"],
        json!(
            long.chars()
                .take(PLACEMENT_BRIEF_CHAR_CAP)
                .collect::<String>()
        )
    );
}

/// One spelling of the three rooms: the table's words and this file's values
/// are the same set, so a room added to one and not the other is red.
#[test]
fn the_rooms_are_the_tables_three() {
    assert_eq!(
        every_room(),
        [Placement::Tab, Placement::Split, Placement::Background]
    );
    assert_eq!(every_room().len(), PLACEMENT_OPTIONS.len());
    for room in every_room() {
        assert!(PLACEMENT_OPTIONS.contains(&room.key()), "{room:?}");
    }
    assert_eq!(Placement::of("beside"), None);
}

/// Every room the question offers carries its own words, and no room it did
/// not offer does — the criteria and the answer space are one thing.
#[test]
fn every_offered_room_carries_its_own_words() {
    let asked = ask(&look(false, InFront::Terminal, true));
    let criteria = asked.questions["placement"]["criteria"]
        .as_object()
        .expect("criteria");
    assert_eq!(criteria.len(), asked.offered().len());
    for room in asked.offered() {
        assert!(criteria.contains_key(room.key()), "{room:?}");
    }
    assert!(!criteria.contains_key("split"));
}

/// A pane nobody was in front of says nothing about the room it was put in
/// (t-6342): all thirty marks this machine's ledger held said the pane was
/// left where it stood, as any answer's would have — the label could not
/// tell "nobody looked" from "looked and kept it". So a pane nobody moved is
/// graded only once somebody could have seen it, and otherwise names why it
/// carries no mark.
#[test]
fn a_pane_nobody_was_present_for_leaves_no_placement_mark() {
    for chosen in every_room() {
        assert_eq!(
            mark(chosen, stood_in(chosen, true), false, false),
            Err(UNSEEN),
            "{}",
            chosen.key()
        );
        assert_eq!(
            baseline_mark(chosen, stood_in(chosen, true), false, false),
            None,
            "{}",
            chosen.key()
        );
    }
    // Seen and left alone: the room it stood in is the person's answer.
    assert_eq!(
        mark(Placement::Tab, stood_in(Placement::Tab, true), false, true),
        Ok(true)
    );
    // A move is always seen, and the person's room grades every answer.
    assert_eq!(
        mark(Placement::Split, Placement::Background, true, true),
        Ok(false)
    );
    assert_eq!(
        mark(Placement::Split, Placement::Split, true, true),
        Ok(true)
    );
}

/// A recording seat's answer never seated anything: the window put the
/// worker in today's room, its own tab, and that is where a pane nobody moved
/// stood. Version 1 of the label wrote the answer's room there (eleven of the
/// thirty marks of 2026-09-23 were recorded splits written down as splits a
/// person had left alone), then graded the answer against the tab — all 22
/// seen recorded splits of 2026-09-26 marked wrong for a room nobody tried.
/// A pane nobody moved grades only the answer that named the room it stood
/// in (t-9427).
#[test]
fn a_pane_nobody_moved_grades_only_the_answer_whose_room_it_stood_in() {
    assert_eq!(Placement::TODAYS, Placement::Tab);
    assert_eq!(stood_in(Placement::Split, false), Placement::Tab);
    assert_eq!(stood_in(Placement::Background, false), Placement::Tab);
    assert_eq!(stood_in(Placement::Split, true), Placement::Split);
    for chosen in [Placement::Split, Placement::Background] {
        assert_eq!(
            mark(chosen, stood_in(chosen, false), false, true),
            Err(NOT_CARRIED),
            "a recorded {} its pane never tried",
            chosen.key()
        );
        assert_eq!(
            baseline_mark(chosen, stood_in(chosen, false), false, true),
            None,
            "and the tab is not graded on a pane the answer is not"
        );
        // Seated by the seat and left alone, the room was tried.
        assert_eq!(mark(chosen, stood_in(chosen, true), false, true), Ok(true));
    }
    assert_eq!(
        mark(Placement::Tab, stood_in(Placement::Tab, false), false, true),
        Ok(true)
    );
    assert_eq!(NOT_CARRIED, crate::step_effort::NOT_CARRIED);
}

/// Today's room is graded on the panes the answer is — the two readers held
/// to the same marks — and against the room the pane ended in: a move grades
/// both on the room the person chose.
#[test]
fn the_baseline_is_graded_on_the_answers_panes_against_where_the_pane_ended() {
    let recorded_tab = stood_in(Placement::Tab, false);
    assert_eq!(
        baseline_mark(Placement::Tab, recorded_tab, false, true),
        Some(true)
    );
    assert_eq!(
        baseline_mark(Placement::Split, Placement::Background, true, true),
        Some(false)
    );
    assert_eq!(
        baseline_mark(Placement::Split, Placement::Tab, true, true),
        Some(true),
        "a split the person moved back to a tab"
    );
    for (chosen, ended_in, moved) in [
        (Placement::Tab, recorded_tab, false),
        (Placement::Split, Placement::Background, true),
        (Placement::Split, stood_in(Placement::Split, false), false),
        (Placement::Split, stood_in(Placement::Split, true), false),
    ] {
        assert_eq!(
            baseline_mark(chosen, ended_in, moved, true).is_some(),
            mark(chosen, ended_in, moved, true).is_ok(),
            "{} in {}",
            chosen.key(),
            ended_in.key()
        );
    }
}
