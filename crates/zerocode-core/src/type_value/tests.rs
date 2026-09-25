//! What the seat's table promises every reader of it — the product, the probe
//! that measured it, and whoever reads it next.

use std::path::{Path, PathBuf};

use super::*;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

/// The fingerprint of the words as they stand. Bump [`Question::rubric_version`]
/// with it: a reading taken under one wording says nothing about another.
const RUBRIC_V1: &str = "af0edebd59fe41c0";

#[test]
fn the_version_is_pinned_to_the_words() {
    assert_eq!(asked().rubric_version, 1, "the version moved");
    assert_eq!(
        crate::jev::rubric_fingerprint(rubric_words),
        RUBRIC_V1,
        "the question's words changed without a version bump — a reading taken \
         under the old wording is not evidence about the new one"
    );
}

#[test]
fn the_state_is_rendered_in_the_tables_order_under_the_tables_labels() {
    let said = render(&FieldLook {
        goal: "Find a flight from Zurich to London",
        label: "From",
        placeholder: "City or airport",
        near: "Departure airport",
    });
    let lines: Vec<&str> = said.lines().collect();
    assert_eq!(lines.len(), asked().state.len() + 1, "{said}");
    for (line, key) in lines.iter().zip(&asked().state) {
        assert!(line.starts_with(&format!("{}: ", key.label)), "{line}");
    }
    assert_eq!(*lines.last().expect("asking line"), asked().value_line);
    assert!(said.contains("goal: Find a flight from Zurich to London"));
    assert!(said.contains("field label: From"));
}

/// A look that saw nothing beside the box still renders every key: an absent
/// label is an empty line, never a missing one, or the state's shape would
/// depend on the page.
#[test]
fn a_look_that_saw_nothing_still_renders_every_key() {
    let said = render(&FieldLook {
        goal: "g",
        label: "",
        placeholder: "",
        near: "",
    });
    assert_eq!(said.lines().count(), asked().state.len() + 1, "{said}");
}

#[test]
fn one_line_inside_the_cap_is_a_value_and_nothing_else_is() {
    assert_eq!(read("Zurich").as_deref(), Ok("Zurich"));
    assert_eq!(read("  Zurich \n").as_deref(), Ok("Zurich"));
    assert_eq!(read("\"Zurich\"").as_deref(), Ok("Zurich"));
    assert_eq!(read("'Zurich'").as_deref(), Ok("Zurich"));
    // One pair only: a value that is itself quoted keeps its inner quotes.
    assert_eq!(read("\"\"Zurich\"\"").as_deref(), Ok("\"Zurich\""));
    // A model that explained itself answered another question.
    assert_eq!(
        read("Zurich\nThat is the departure city."),
        Err(ValueRefusal::NotOneLine)
    );
    assert_eq!(read("   "), Err(ValueRefusal::Empty));
    assert_eq!(read("\"\""), Err(ValueRefusal::Empty));
    assert_eq!(
        read(&"x".repeat(asked().value_char_cap + 1)),
        Err(ValueRefusal::TooLong)
    );
    assert_eq!(
        read(&"x".repeat(asked().value_char_cap)).map(|v| v.len()),
        Ok(asked().value_char_cap)
    );
    assert_eq!(read("Zur\u{7}ich"), Err(ValueRefusal::Untypable));
}

/// The cap counts CHARACTERS, not bytes: a Korean value inside the cap is a
/// value, and a byte cap would have refused it at a third of its length.
#[test]
fn the_cap_counts_characters_not_bytes() {
    let korean = "가".repeat(asked().value_char_cap);
    assert!(korean.len() > asked().value_char_cap, "the two differ");
    assert!(read(&korean).is_ok());
}

#[test]
fn the_chosen_row_is_one_this_table_holds() {
    let Some(id) = seat().chosen.as_deref() else {
        panic!("no row chosen — say so in the table's `chosen`, or name one");
    };
    let chosen = chosen().expect("`chosen` names a row this table does not hold");
    assert_eq!(chosen.id, id);
    assert!(
        chosen.measured.is_some(),
        "the chosen row was never measured"
    );
}

/// Every row carries what its road needs to be reached, and nothing carries a
/// key: the table names where a key LIVES, never what it is.
#[test]
fn every_row_carries_its_road_and_no_key() {
    for row in rows() {
        match row.road {
            Road::OpenaiCompat => {
                assert!(row.base_url.is_some(), "{} has no endpoint", row.id);
                assert!(
                    row.credential_key.is_some() || row.keychain_service.is_some(),
                    "{} names no key store",
                    row.id
                );
            }
            Road::CodeAssist => assert!(row.host.is_some(), "{} has no host", row.id),
            Road::Anthropic => {
                assert!(row.base_url.is_none(), "{} pins an endpoint", row.id);
                // A person's own key, by its name in the window's key store:
                // this road is never a subscription's (t-6720).
                assert!(
                    row.credential_key.is_some(),
                    "{} names no key store",
                    row.id
                );
            }
        }
        assert!(
            !row.reach.trim().is_empty(),
            "{} says who can reach it",
            row.id
        );
        let table = MODELS_JSON;
        for mark in ["sk-", "Bearer ", "ya29."] {
            assert!(
                !table.contains(mark),
                "the table carries something shaped like a key ({mark})"
            );
        }
    }
}

#[test]
fn every_id_is_its_own() {
    let mut seen: Vec<&str> = rows().iter().map(|row| row.id.as_str()).collect();
    let before = seen.len();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), before, "two rows share an id");
}

/// A reading says what it is: its percentiles rise, it accepted no more
/// answers than it made calls, and it carries the load it was read under.
#[test]
fn every_reading_says_what_it_was_read_under() {
    for row in rows() {
        let Some(read) = &row.measured else { continue };
        let id = &row.id;
        assert_eq!(read.at.len(), 10, "{id} reading has no day");
        assert!(!read.runs.is_empty(), "{id} measured nothing");
        for run in &read.runs {
            let note = &run.note;
            assert!(run.n > 0, "{id} {note} counted nothing");
            assert!(run.p10_ms <= run.p50_ms, "{id} {note} p10 above p50");
            assert!(run.p50_ms <= run.p90_ms, "{id} {note} p50 above p90");
            assert!(
                run.accepted <= run.n,
                "{id} {note} accepted more than it asked"
            );
            assert!(
                run.thought_first <= run.n,
                "{id} {note} narrated more than it asked"
            );
            assert!(
                run.load1_min <= run.load1_max,
                "{id} {note} load range inverted"
            );
            assert!(run.load1_min >= 0.0, "{id} {note} has no load");
        }
    }
}

/// No row is chosen on ONE reading. The first reading of this table would have
/// picked the wrong row (`Measured`'s note), so a row a seat names has to have
/// been read more than once — that is the whole discipline, held by a test
/// rather than by whoever edits the table next.
#[test]
fn the_chosen_row_was_read_more_than_once() {
    let Some(chosen) = chosen() else { return };
    let read = chosen
        .measured
        .as_ref()
        .expect("the chosen row was measured");
    assert!(
        read.runs.len() >= 2,
        "{} was chosen on one reading",
        chosen.id
    );
    let (asked, accepted) = read.asked_and_accepted();
    assert_eq!(
        accepted, asked,
        "{} was chosen while it answered {accepted} of {asked} — a fast wrong \
         answer is not an answer this seat can type",
        chosen.id
    );
    let worst = read.p50_worst_ms().expect("a median");
    for row in rows() {
        let Some(other) = &row.measured else { continue };
        if row.id == chosen.id || other.runs.len() < read.runs.len() {
            continue;
        }
        let (asked, accepted) = other.asked_and_accepted();
        if accepted < asked {
            continue;
        }
        let Some(rival) = other.p50_worst_ms() else {
            continue;
        };
        assert!(
            rival >= worst || !first_party(row),
            "{} is steadier than the chosen {} ({rival} ms against {worst} ms) and \
             is a first party's too",
            row.id,
            chosen.id
        );
    }
}

/// Whether a row's road is a model maker's own API — one key or one
/// sign-in with the maker — rather than a gateway or a private endpoint in
/// front of someone else's model. Every row now asks for something a person
/// set up (t-6720); a gateway's row is one the seat may prefer, a first
/// party's the one it defaults to.
fn first_party(row: &ValueRow) -> bool {
    matches!(row.road, Road::Anthropic | Road::CodeAssist)
}

/// The probe and the product read the same two files. A probe that spelled the
/// question itself would be measuring a seat this product does not have, and
/// every number in the table would be about the wrong thing.
#[test]
fn the_probe_reads_this_table_rather_than_a_copy_of_it() {
    let probe = std::fs::read_to_string(repo_root().join("tools/type_value_latency.py"))
        .expect("the probe that measured this table");
    for file in ["question.json", "models.json"] {
        assert!(probe.contains(file), "the probe does not read {file}");
    }
    let words = &asked().instructions;
    assert!(
        !probe.contains(words.as_str()),
        "the probe spells the question itself — it must read it from question.json"
    );
    for row in rows() {
        assert!(
            !probe.contains(row.model.as_str()),
            "the probe spells {}'s model — it must read the roster",
            row.id
        );
    }
}

/// This seat's model lives in the table and nowhere else in the crate's own
/// source. A pane that spelled one would be a second truth about which model
/// answers, and the two would part on the next reading.
#[test]
fn no_source_of_this_crate_spells_this_seats_model() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();
    let mut pending = vec![src.clone()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            if path.extension().is_none_or(|ext| ext != "rs") {
                continue;
            }
            if path.file_name().is_some_and(|name| name == "tests.rs")
                || path.components().any(|part| part.as_os_str() == "tests")
            {
                continue;
            }
            let Ok(source) = std::fs::read_to_string(&path) else {
                continue;
            };
            for row in rows() {
                if source.contains(&format!("\"{}\"", row.model)) {
                    offenders.push(format!("{}: {}", path.display(), row.model));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "a model this seat's table already names is spelled in source too:\n  {}",
        offenders.join("\n  ")
    );
}

/// Every case the probe rotates through can be answered by the seat's own
/// reader: a case whose accepted value this module would refuse would score
/// every candidate zero.
#[test]
fn every_case_accepts_a_value_this_seat_would_take() {
    for case in &asked().cases {
        assert!(!case.accepts.is_empty(), "{} accepts nothing", case.id);
        for word in &case.accepts {
            assert!(
                read(word).is_ok(),
                "{} accepts {word:?}, which this seat refuses",
                case.id
            );
        }
        assert!(!case.goal.trim().is_empty(), "{} has no goal", case.id);
    }
}

/// A written value's identity is every part of its input under the chosen
/// row: the same input is the same identity, a change to any one part is
/// another, and a look that named no document has none — such a value is
/// written fresh every time (t-6720).
#[test]
fn a_values_identity_is_every_part_of_its_input_and_none_without_a_document() {
    let row = chosen().expect("a chosen row");
    let look = FieldLook {
        goal: "Search for London",
        label: "Destination",
        placeholder: "City",
        near: "Travel search",
    };
    let input = ValueInput {
        look,
        now: "",
        epoch: "doc-1",
        target: r##"["#destination","input","textbox"]"##,
    };
    let same = identity(&input, row).expect("a document names an identity");
    assert_eq!(identity(&input, row).as_deref(), Some(same.as_str()));
    let changed = [
        ValueInput {
            look: FieldLook {
                goal: "Search for Paris",
                ..look
            },
            ..input
        },
        ValueInput {
            look: FieldLook {
                label: "Origin",
                ..look
            },
            ..input
        },
        ValueInput {
            look: FieldLook {
                placeholder: "Airport",
                ..look
            },
            ..input
        },
        ValueInput {
            look: FieldLook {
                near: "Hotels",
                ..look
            },
            ..input
        },
        ValueInput {
            now: "Zur",
            ..input
        },
        ValueInput {
            epoch: "doc-2",
            ..input
        },
        ValueInput {
            target: r##"["#origin","input","textbox"]"##,
            ..input
        },
    ];
    for other in changed {
        assert_ne!(
            identity(&other, row).as_deref(),
            Some(same.as_str()),
            "{other:?}"
        );
    }
    assert_eq!(
        identity(
            &ValueInput {
                epoch: " ",
                ..input
            },
            row
        ),
        None
    );
    // The value a field holds is part of the identity only: the question a
    // model is asked never carries it.
    assert!(!render(&look).contains("Zur"));
}
