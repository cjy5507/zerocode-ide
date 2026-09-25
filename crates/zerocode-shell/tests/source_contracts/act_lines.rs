//! A seat that says its stage reads an act line has a stage that reads it
//! (t-9468): `JevUse::reads_act_line` is what lets the judge read a seat's
//! marks from the line its labels drew, so a seat that declared it and a
//! stage that still acted on every answer would rise on its confident part
//! and act on all of it. One row per declaring seat, naming where its
//! shipped stage reads the line; a seat that starts declaring it is a red
//! here until its row says where.

use zerocode_core::jev::JEV_USES;

/// A source file's shipped part — everything before the test module it
/// holds inline. A module declared for tests and kept in a file of its own
/// (`mod tests;`) holds nothing here, and what follows it ships.
fn shipped(text: &'static str) -> &'static str {
    text.match_indices("\n#[cfg(test)]\nmod ")
        .find(|(at, opens)| {
            text[at + opens.len()..]
                .lines()
                .next()
                .is_some_and(|line| line.trim_end().ends_with('{'))
        })
        .map_or(text, |(at, _)| &text[..at])
}

const NOTIFY: &str = include_str!("../../src/notify_call.rs");
const PLACEMENT: &str = include_str!("../../src/cmd/worker_room.rs");
const STALL: &str = include_str!("../../src/orchestration/stall_cause.rs");
const SUMMON: &str = include_str!("../../src/orchestration/summon_choice.rs");
const BROWSER_READ: &str = include_str!("../../src/browser_read.rs");
const WALKS: &str = include_str!("../../src/agent_tools_runtime.rs");
const ERRAND: &str = include_str!("../../src/computer_use/errand.rs");
const BRANCH: &str = include_str!("../../src/computer_use/errand/branch.rs");
const ROUTING_STAGE: &str =
    include_str!("../../../../zo-ide/crates/tools/src/misc_tools/smart_router/decision_shadow.rs");
const ROUTING_BAND: &str =
    include_str!("../../../../zo-ide/crates/runtime/src/model_router/decision.rs");

/// Each declaring seat, and where its stage reads the line: the file, and
/// what it says there.
const READS: &[(&str, &[(&str, &str)])] = &[
    (
        "routing",
        &[
            (ROUTING_STAGE, "threshold::line_beside(&ROUTING, &ledger)"),
            (ROUTING_STAGE, "reading.assessment_at(line)"),
            (
                ROUTING_BAND,
                "ROUTING.band_at(self.band_confidence(), line)",
            ),
        ],
    ),
    (
        "browser",
        &[
            (
                WALKS,
                "act_line: crate::systemone::act_line(judge.wire(), seat)",
            ),
            (ERRAND, "policy.permits_press_at(confidence, kind, line)"),
        ],
    ),
    (
        "desktop",
        &[
            (
                WALKS,
                "act_line: crate::systemone::act_line(judge.wire(), seat)",
            ),
            (ERRAND, "policy.permits_press_at(confidence, kind, line)"),
        ],
    ),
    (
        "emulator",
        &[
            (
                WALKS,
                "act_line: crate::systemone::act_line(judge.wire(), seat)",
            ),
            (ERRAND, "policy.permits_press_at(confidence, kind, line)"),
        ],
    ),
    (
        "stall",
        &[
            (STALL, "crate::systemone::act_line(&wire, &STALL)"),
            (STALL, "STALL.acts_on("),
        ],
    ),
    (
        "placement",
        &[
            (PLACEMENT, "crate::systemone::act_line(wire, &PLACEMENT)"),
            (PLACEMENT, "PLACEMENT.acts_on("),
        ],
    ),
    (
        "summon",
        &[
            (SUMMON, "crate::systemone::act_line(&wire, &SUMMON)"),
            (SUMMON, "SUMMON.acts_on(pick.confidence, line)"),
        ],
    ),
    (
        "browser_read",
        &[(
            BROWSER_READ,
            "crate::systemone::act_line(wire, &BROWSER_READ)",
        )],
    ),
    (
        "notify",
        &[
            (NOTIFY, "crate::systemone::act_line(&wire, &NOTIFY)"),
            (NOTIFY, "NOTIFY.acts_on(confidence, line)"),
        ],
    ),
    (
        "branching",
        &[
            (
                WALKS,
                "act_line: crate::systemone::act_line(judge.wire(), forks)",
            ),
            (BRANCH, "press_rule(&BRANCHING, line, screen"),
        ],
    ),
];

#[test]
fn a_seat_that_reads_an_act_line_has_a_stage_that_reads_it() {
    let mut declared: Vec<&str> = JEV_USES
        .iter()
        .filter(|seat| seat.reads_act_line)
        .map(|seat| seat.id)
        .collect();
    let mut named: Vec<&str> = READS.iter().map(|(id, _)| *id).collect();
    declared.sort_unstable();
    named.sort_unstable();
    assert_eq!(
        declared, named,
        "the seats that declare an act line and the stages named here"
    );
    for (id, reads) in READS {
        for (source, needle) in *reads {
            assert!(
                shipped(source).contains(needle),
                "{id}: its stage no longer reads its act line — `{needle}` is gone"
            );
        }
    }
}
