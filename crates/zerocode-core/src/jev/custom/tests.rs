use std::collections::{BTreeMap, BTreeSet};

use serde_json::json;

use super::store::{self, Error};
use super::*;

/// 2026-10-17T05:00:00Z: when the store's stamps are written.
pub(super) const NOW: i64 = 1_792_213_200_000;

/// A yes-or-no decision asking at a turn's end, keeping every rule.
pub(super) fn yes_no(id: &str, name: &str) -> Definition {
    Definition::new(
        id.to_string(),
        name.to_string(),
        "Did this turn change what a person using the product sees?".to_string(),
        Answer::YesNo {
            yes: "The evidence shows a visible change.".to_string(),
            no: "Nothing visible changed, or the evidence cannot tell.".to_string(),
        },
        Point::TurnEnd,
    )
}

fn choice(options: &[(&str, &str)]) -> Answer {
    Answer::Choice(
        options
            .iter()
            .map(|(name, meaning)| ((*name).to_string(), (*meaning).to_string()))
            .collect(),
    )
}

fn options(count: usize) -> Answer {
    Answer::Choice(
        (0..count)
            .map(|n| (format!("option {n}"), format!("Option {n} fits.")))
            .collect::<BTreeMap<_, _>>(),
    )
}

fn levels(count: usize) -> Answer {
    Answer::Score((0..count).map(|n| format!("Level {n}.")).collect())
}

/// What the check says of the fixture after `change`.
fn refused(change: fn(&mut Definition)) -> Vec<Refusal> {
    let mut definition = yes_no("d-0000000a", "Release notes");
    change(&mut definition);
    check(&definition)
}

/// Every line a person writes is read by the detector the door clears a
/// request with (`crate::credential::may_carry_a_credential`). A line that may
/// carry a credential is refused and pointed at — its field, and its number
/// counted from one — and the store keeps nothing.
#[test]
fn a_question_with_a_secret_line_is_refused_with_its_line() {
    let mut definition = yes_no("d-0000000a", "Release notes");
    assert!(
        check(&definition).is_empty(),
        "the fixture keeps every rule"
    );
    definition.question = "Did this turn change what a person sees?\nRead the diff first.\n\
                           Authorization: Bearer abc\nThen answer."
        .to_string();
    let third = [Refusal::SecretLine {
        field: Field::Question,
        line: 3,
    }];
    assert_eq!(check(&definition), third);
    let home = tempfile::tempdir().unwrap();
    assert!(
        matches!(
            store::create(home.path(), &definition, JevMode::Shadow, NOW),
            Err(Error::Refused(refused)) if refused == third
        ),
        "the store keeps nothing the check refuses",
    );
    assert!(store::list(home.path()).unwrap().is_empty());

    let mut named = yes_no("d-0000000b", "Rotate the deploy password");
    named.answer = Answer::YesNo {
        yes: "It rotated.".to_string(),
        no: "Nothing rotated.\nAsk with Bearer abc.".to_string(),
    };
    assert_eq!(
        check(&named),
        [
            Refusal::SecretLine {
                field: Field::Name,
                line: 1,
            },
            Refusal::SecretLine {
                field: Field::No,
                line: 2,
            },
        ],
    );

    let mut chose = yes_no("d-0000000c", "Ship or wait");
    chose.answer = choice(&[
        ("ship", "Ship it.\nexport TOKEN=1"),
        ("password", "Ask for one."),
    ]);
    assert_eq!(
        check(&chose),
        [
            Refusal::SecretLine {
                field: Field::OptionName("password".to_string()),
                line: 1,
            },
            Refusal::SecretLine {
                field: Field::Meaning("ship".to_string()),
                line: 2,
            },
        ],
    );

    let mut scored = yes_no("d-0000000d", "Risk");
    scored.answer = Answer::Score(vec![
        "Nothing to do.".to_string(),
        "Someone runs mysql -phunter2 by hand.".to_string(),
        "Stop.".to_string(),
    ]);
    assert_eq!(
        check(&scored),
        [Refusal::SecretLine {
            field: Field::Level(1),
            line: 1,
        }],
    );
}

/// Every limit is the person's, and holds at its edge: the most passes, and
/// one more is refused.
#[test]
fn limits_hold() {
    assert_eq!(
        (
            NAME_MAX_CHARS,
            QUESTION_MAX_CHARS,
            OPTIONS,
            MEANING_MAX_CHARS,
            DECISIONS_MAX,
            HOURLY_REQUESTS,
            HOURLY_DEFAULT,
            ALL_HOURLY_DEFAULT,
        ),
        (40, 1_000, 2..=10, 600, 20, 1..=120, 20, 60),
    );
    assert!(refused(|_| {}).is_empty(), "the fixture keeps every rule");

    assert!(refused(|d| d.name = "가".repeat(NAME_MAX_CHARS)).is_empty());
    assert_eq!(
        refused(|d| d.name = "가".repeat(NAME_MAX_CHARS + 1)),
        [Refusal::NameLength],
        "a name is counted in characters",
    );
    assert_eq!(refused(|d| d.name = " ".to_string()), [Refusal::NameLength]);
    assert!(refused(|d| d.question = "가".repeat(QUESTION_MAX_CHARS)).is_empty());
    assert_eq!(
        refused(|d| d.question = "가".repeat(QUESTION_MAX_CHARS + 1)),
        [Refusal::QuestionLength],
    );

    let mut definition = yes_no("d-0000000a", "Release notes");
    for count in [*OPTIONS.start(), *OPTIONS.end()] {
        definition.answer = options(count);
        assert!(check(&definition).is_empty(), "{count} options");
        definition.answer = levels(count);
        assert!(check(&definition).is_empty(), "{count} levels");
    }
    for count in [*OPTIONS.start() - 1, *OPTIONS.end() + 1] {
        definition.answer = options(count);
        assert_eq!(
            check(&definition),
            [Refusal::OptionCount],
            "{count} options"
        );
        definition.answer = levels(count);
        assert_eq!(check(&definition), [Refusal::OptionCount], "{count} levels");
    }
    definition.answer = choice(&[("ship", "Ship it."), (NOT_APPLICABLE, "Neither.")]);
    assert_eq!(
        check(&definition),
        [Refusal::OptionName(NOT_APPLICABLE.to_string())],
        "every choice offers it already",
    );
    let long = "가".repeat(NAME_MAX_CHARS + 1);
    definition.answer = choice(&[("ship", "Ship it."), (long.as_str(), "Long.")]);
    assert_eq!(check(&definition), [Refusal::OptionName(long)]);

    let most = "가".repeat(MEANING_MAX_CHARS);
    let over = "가".repeat(MEANING_MAX_CHARS + 1);
    definition.answer = Answer::YesNo {
        yes: most.clone(),
        no: over.clone(),
    };
    assert_eq!(check(&definition), [Refusal::MeaningLength(Field::No)]);
    definition.answer = choice(&[("ship", most.as_str()), ("wait", over.as_str())]);
    assert_eq!(
        check(&definition),
        [Refusal::MeaningLength(Field::Meaning("wait".to_string()))],
    );
    definition.answer = Answer::Score(vec![most.clone(), over.clone()]);
    assert_eq!(
        check(&definition),
        [Refusal::MeaningLength(Field::Level(1))]
    );
    definition.answer = Answer::YesNo {
        yes: String::new(),
        no: most,
    };
    assert_eq!(check(&definition), [Refusal::MeaningLength(Field::Yes)]);

    assert!(refused(|d| d.max_per_hour = *HOURLY_REQUESTS.start()).is_empty());
    assert!(refused(|d| d.max_per_hour = *HOURLY_REQUESTS.end()).is_empty());
    assert_eq!(
        refused(|d| d.max_per_hour = *HOURLY_REQUESTS.start() - 1),
        [Refusal::MaxPerHour],
    );
    assert_eq!(
        refused(|d| d.max_per_hour = *HOURLY_REQUESTS.end() + 1),
        [Refusal::MaxPerHour],
    );
    assert_eq!(
        yes_no("d-0000000a", "New").max_per_hour,
        HOURLY_DEFAULT,
        "a new decision starts at the person's bound",
    );
    assert!(refused(|d| d.act_from = *ACT_FROM.start()).is_empty());
    assert!(refused(|d| d.act_from = *ACT_FROM.end()).is_empty());
    assert_eq!(refused(|d| d.act_from = 0.49), [Refusal::ActFrom]);
    assert_eq!(refused(|d| d.act_from = 0.96), [Refusal::ActFrom]);
    assert_eq!(refused(|d| d.act_from = f64::NAN), [Refusal::ActFrom]);

    assert_eq!(refused(|d| d.reads.clear()), [Refusal::Reads]);
    assert_eq!(
        refused(|d| {
            d.reads.insert(Input::Command);
        }),
        [Refusal::Reads],
        "a turn's end offers no command",
    );
    assert_eq!(
        refused(|d| d.folders = Folders::Chosen(BTreeSet::new())),
        [Refusal::Folders],
    );
    assert_eq!(
        refused(|d| d.folders = Folders::Chosen(BTreeSet::from(["work/product".to_string()]))),
        [Refusal::Folders],
    );
    assert!(
        refused(|d| d.folders = Folders::Chosen(BTreeSet::from(["/work/product".to_string()])))
            .is_empty()
    );
    for id in [
        "d-0000000A",
        "skill_suggest",
        "..",
        "/",
        "/work/product",
        "ａ",
        "\0",
    ] {
        let mut invalid = yes_no("d-0000000a", "Release notes");
        invalid.id = id.to_string();
        assert_eq!(check(&invalid), [Refusal::Id], "invalid id: {id:?}");
    }
    assert!(check(&yes_no(&new_id(), "Fresh")).is_empty());
    assert_ne!(new_id(), new_id());

    let home = tempfile::tempdir().unwrap();
    for n in 0..DECISIONS_MAX {
        let decision = yes_no(&format!("d-{n:08x}"), &format!("Decision {n}"));
        store::create(home.path(), &decision, JevMode::Shadow, NOW).unwrap();
    }
    assert!(
        matches!(
            store::create(home.path(), &yes_no("d-000000ff", "One more"), JevMode::Shadow, NOW),
            Err(Error::Refused(refused)) if refused == [Refusal::TooMany]
        ),
        "the twenty-first decision is refused",
    );

    let home = tempfile::tempdir().unwrap();
    let kept = yes_no("d-0000000a", "Release notes");
    store::create(home.path(), &kept, JevMode::Shadow, NOW).unwrap();
    let refused_by_the_store = |definition: &Definition, mode: JevMode| match store::create(
        home.path(),
        definition,
        mode,
        NOW,
    ) {
        Err(Error::Refused(refused)) => refused,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        refused_by_the_store(&yes_no("d-0000000b", " Release notes "), JevMode::Shadow),
        [Refusal::NameTaken],
    );
    assert_eq!(
        refused_by_the_store(&yes_no(&kept.id, "Another"), JevMode::Shadow),
        [Refusal::IdTaken],
    );
    assert_eq!(
        refused_by_the_store(&yes_no("d-0000000c", "Acting alone"), JevMode::Auto),
        [Refusal::Mode],
    );
    let mut large = yes_no("d-0000000d", "Large");
    large.folders = Folders::Chosen(
        (0..store::DEFINITION_MAX_BYTES / 64)
            .map(|n| format!("/work/{n:058}"))
            .collect(),
    );
    assert_eq!(
        refused_by_the_store(&large, JevMode::Shadow),
        [Refusal::TooLarge]
    );
    assert_eq!(
        store::list(home.path()).unwrap().len(),
        1,
        "nothing refused was kept"
    );
}

/// A definition's version is the hash of its bytes: the same words, the same
/// version; one character more, another. A mode is not one of its words.
#[test]
fn a_changed_word_makes_a_new_version() {
    let definition = yes_no("d-0000000a", "Release notes");
    assert_eq!(
        definition.version().unwrap(),
        definition.clone().version().unwrap()
    );
    let mut changed = definition.clone();
    changed.question.push('?');
    assert_ne!(
        definition.version().unwrap(),
        changed.version().unwrap(),
        "one character more is another version",
    );
    let mut fewer = definition.clone();
    fewer.reads.remove(&Input::Evidence);
    assert_ne!(
        definition.version().unwrap(),
        fewer.version().unwrap(),
        "and so is one input less",
    );

    let home = tempfile::tempdir().unwrap();
    let first = store::create(home.path(), &definition, JevMode::Shadow, NOW).unwrap();
    assert_eq!(first, definition.version().unwrap());
    store::set_mode(home.path(), &definition.id, JevMode::On, NOW + 1).unwrap();
    let listed = store::list(home.path()).unwrap();
    assert_eq!(
        (listed[0].mode, listed[0].definition.as_str()),
        (JevMode::On, first.as_str()),
        "a mode leaves the version where it was",
    );
    assert!(
        matches!(
            store::set_mode(home.path(), &definition.id, JevMode::Auto, NOW + 1),
            Err(Error::Refused(refused)) if refused == [Refusal::Mode]
        ),
        "and no decision offers auto",
    );

    let second = store::edit(home.path(), &changed, NOW + 2).unwrap();
    assert_eq!(second, changed.version().unwrap());
    let listed = store::list(home.path()).unwrap();
    assert_eq!(
        (listed[0].mode, listed[0].definition.as_str()),
        (JevMode::On, second.as_str())
    );
    assert_eq!(
        store::definition(home.path(), &first).unwrap(),
        definition,
        "the old version stays",
    );
    assert_eq!(store::definition(home.path(), &second).unwrap(), changed);
}

/// A score asks the wire's own primitive: its tag, the person's question, and
/// the levels in order, lowest first, as a list — and nothing else.
#[test]
fn a_score_asks_its_levels_in_order() {
    let mut definition = yes_no("d-0000000a", "Risk");
    definition.answer = Answer::Score(vec![
        "Low.".to_string(),
        "Some.".to_string(),
        "High.".to_string(),
    ]);
    let questions = questions(&definition);
    assert_eq!(questions.as_object().map(Map::len), Some(1));
    let question = &questions[definition.id.as_str()];
    assert_eq!(question.as_object().map(Map::len), Some(3), "{question}");
    assert_eq!(question["type"], "score");
    assert_eq!(question["criteria"], json!(["Low.", "Some.", "High."]));
    let instructions = question["instructions"].as_str().unwrap();
    assert!(
        instructions.starts_with(&definition.question),
        "{instructions}"
    );
}

/// Every refusal says which rule it is, and every field where, with a word of
/// its own in snake case, and no word holds what it was built with: an
/// option's name, the field a line stands in, a level's number, or the line's
/// number. zo writes a refusal's word on a row for a stored definition that
/// fails the check again, and the drawer keys its fields on a field's word,
/// so the person's words reach neither.
#[test]
fn every_refusal_and_field_has_a_word_of_its_own_and_never_its_payload() {
    let written = "deploy";
    let fields = [
        Field::Name,
        Field::Question,
        Field::Yes,
        Field::No,
        Field::OptionName(written.to_string()),
        Field::Meaning(written.to_string()),
        Field::Level(7),
    ];
    let field_words: BTreeSet<&str> = fields.iter().map(Field::word).collect();
    assert_eq!(
        field_words.len(),
        fields.len(),
        "two fields share a word: {field_words:?}"
    );
    let one_of_each = [
        Refusal::Id,
        Refusal::IdTaken,
        Refusal::NameLength,
        Refusal::NameTaken,
        Refusal::QuestionLength,
        Refusal::OptionCount,
        Refusal::OptionName(written.to_string()),
        Refusal::MeaningLength(Field::Meaning(written.to_string())),
        Refusal::SecretLine {
            field: Field::OptionName(written.to_string()),
            line: 7,
        },
        Refusal::Reads,
        Refusal::Folders,
        Refusal::ActFrom,
        Refusal::MaxPerHour,
        Refusal::Mode,
        Refusal::TooMany,
        Refusal::TooLarge,
    ];
    let words: BTreeSet<&str> = one_of_each.iter().map(Refusal::word).collect();
    assert_eq!(
        words.len(),
        one_of_each.len(),
        "two rules share a word: {words:?}"
    );
    let carrying_more = [
        Refusal::MeaningLength(Field::Level(7)),
        Refusal::SecretLine {
            field: Field::Meaning(written.to_string()),
            line: 7,
        },
    ];
    let spoken = one_of_each
        .iter()
        .chain(&carrying_more)
        .map(|refusal| (refusal.word(), format!("{refusal:?}")))
        .chain(
            fields
                .iter()
                .map(|field| (field.word(), format!("{field:?}"))),
        );
    for (word, built) in spoken {
        let snake = !word.is_empty()
            && !word.starts_with('_')
            && !word.ends_with('_')
            && !word.contains("__")
            && word
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte == b'_');
        assert!(snake, "{word} is not snake case");
        assert!(
            !word.contains(written) && !word.contains('7'),
            "{word} holds what {built} was built with"
        );
    }
}

/// The line an agent is handed reads as DESIGN 2.4 writes it, the confidence
/// a whole percent.
#[test]
fn a_line_reads_as_the_design_writes_it() {
    assert_eq!(line("이름", "보기", 0.85), "「이름」 판단: 보기 (확신 85%)");
    assert_eq!(
        line("Release notes", "yes", 0.5),
        "「Release notes」 판단: yes (확신 50%)"
    );
}

/// The longest name and the longest option the check lets through, at the
/// surest answer, fit [`LINE_MAX_CHARS`]: the cap holds by construction, and
/// no line is cut when it is handed.
#[test]
fn the_longest_line_the_limits_allow_fits_the_cap() {
    let longest = "가".repeat(NAME_MAX_CHARS);
    let mut definition = yes_no("d-0000000a", &longest);
    definition.answer = choice(&[(longest.as_str(), "It fits."), ("other", "It does not.")]);
    assert_eq!(check(&definition), Vec::new());
    let handed = line(&definition.name, &longest, 1.0);
    let length = handed.chars().count();
    assert!(length <= LINE_MAX_CHARS, "{length} characters: {handed}");
    definition.name.push('가');
    assert!(check(&definition).contains(&Refusal::NameLength));
}

/// A score's level is shown counted from one over its levels, the way a
/// person numbers them; the most levels a score offers still read whole.
#[test]
fn a_level_is_shown_counted_from_one_over_the_levels() {
    assert_eq!(shown_level(0, 4), "1/4");
    assert_eq!(shown_level(2, 4), "3/4");
    let most = *OPTIONS.end();
    assert_eq!(shown_level(most - 1, most), format!("{most}/{most}"));
}
