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
    assert_eq!(WORKER_PLACEMENT_RUBRIC_VERSION, 1);
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
