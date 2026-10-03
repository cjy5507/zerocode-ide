//! The continue gate on the board and in the settings (t-26583): the words the
//! board draws from the judgement the window made, and the one road a person's
//! budget travels. The judgement itself — what is counted, what is read, when a
//! worker is told or ended — is core's and the beat's, and is tested there.

use zerocode_core::continue_gate::{Code, Verdict};

use std::path::Path;

use super::support::{
    assert_canonical_setting_round_trip, block_after, shipped_backend, strip_rust_comments,
    window_source,
};

const BOARD: &str = include_str!("../../../../ui/shell-board.js");
const MARKUP: &str = include_str!("../../../../ui/index.html");
const METER: &str = include_str!("../../src/orchestration/gate_meter.rs");

/// The board says what the gate said, in a person's words, for every verdict and
/// every fact the gate can give for it — and the words are looked up by the same
/// names the backend serializes, so a new reason that reached the board with no
/// line of its own would show as a raw code, which is a bug a person cannot read.
/// A cost that is not a number says why, for each way the window can fail to read
/// it.
#[test]
fn every_word_the_gate_can_say_has_a_line_on_the_board() {
    assert!(
        BOARD.contains("const GATE_VERDICTS = Object.freeze({")
            && BOARD.contains("const GATE_REASONS = Object.freeze({")
            && BOARD.contains("const GATE_COST_NOTES = Object.freeze({"),
        "the board has no table of the gate's words"
    );
    let verdicts = block_after(BOARD, "const GATE_VERDICTS = Object.freeze({");
    for verdict in Verdict::ALL {
        let held = format!("{}: {{", verdict.word());
        assert!(
            verdicts.contains(&held),
            "the board has no word for the verdict `{}`:\n{verdicts}",
            verdict.word()
        );
    }
    let reasons = block_after(BOARD, "const GATE_REASONS = Object.freeze({");
    for code in Code::ALL {
        let held = format!("{}: {{", code.word());
        assert!(
            reasons.contains(&held),
            "the board has no sentence for the reason `{}`:\n{reasons}",
            code.word()
        );
    }
    // The ways a cost can be unknown are the meter's (`CostNote`); `read` is the
    // one that is a number and needs no excuse.
    let notes = block_after(BOARD, "const GATE_COST_NOTES = Object.freeze({");
    let meter = block_after(METER, "pub(crate) enum CostNote {");
    for (variant, word) in [
        ("NoReader", "no_reader"),
        ("NoTranscript", "no_transcript"),
        ("Unreadable", "unreadable"),
    ] {
        assert!(
            meter.contains(variant) && notes.contains(&format!("{word}: {{")),
            "`{variant}` is a way the meter fails to read a cost and the board has no excuse for `{word}`:\n{notes}"
        );
    }
}

/// A worker row paints its gate line from the row it was given — the judgement
/// is the backend's, drawn once with the rest of the row — and the desk's slow
/// beat re-reads the rows: the gate's numbers (calls, spend) move while the
/// ledger does not, and a ledger that did not move tells the desk nothing.
#[test]
fn the_gate_line_is_painted_with_its_row_and_the_slow_beat_re_reads_the_rows() {
    let painting = block_after(BOARD, "function paintDeskWorkers(");
    assert!(
        painting.contains("paintDeskGate(")
            && painting.contains("row.gate")
            && painting.contains(".board-desk-worker-gate"),
        "a worker row does not paint its gate line:\n{painting}"
    );
    let row = block_after(BOARD, "function deskWorkerRow(");
    for part in [
        "board-desk-worker-gate",
        "board-desk-worker-gate-verdict",
        "board-desk-worker-gate-why",
        "board-desk-worker-gate-basis",
    ] {
        assert!(
            row.contains(part),
            "a worker row has no `{part}` to paint:\n{row}"
        );
    }
    let slow = block_after(BOARD, "function refreshDeskAmbient() {");
    assert!(
        slow.contains("refreshDeskLedger()"),
        "the gate's numbers go stale: nothing but the ledger moving re-reads the rows:\n{slow}"
    );
}

/// A person's budget and ceilings travel the one road every setting travels: the
/// command patches the shared document and answers with its snapshot, and the
/// window applies that snapshot — and both commands are registered, and the card
/// has the controls the wire is read from.
#[test]
fn a_persons_budget_travels_the_one_settings_road() {
    let backend = shipped_backend();
    let window = window_source();
    for command in ["set_harness_settings", "harness_status"] {
        assert!(
            backend.contains(&format!("            {command},")),
            "`{command}` is not registered, so the card fails on every touch"
        );
    }
    assert_canonical_setting_round_trip(
        backend,
        window,
        "set_harness_settings",
        "setting_key::HARNESS",
        "settings.harness = harness;",
        "function commitHarnessSettings(",
        "harness",
        "harnessSettings = snapshot.harness;",
    );
    let setter = block_after(backend, "fn set_harness_settings(");
    assert!(
        setter.contains("gate_book::set_settings(") && setter.contains("set_limits("),
        "a saved budget does not reach the beat and the launch ledger until a restart:\n{setter}"
    );
    for id in [
        "harness-gate-mode",
        "harness-task-usd",
        "harness-day-usd",
        "harness-concurrent",
        "harness-per-hour",
        "harness-per-day",
        "harness-status",
    ] {
        assert!(
            MARKUP.contains(&format!("id=\"{id}\"")),
            "the settings card has no `{id}`"
        );
    }
}

/// Every shipped `.rs` file under `directory`, as `(path relative to it, code)` — the
/// code with its test tail and its comments taken out, so a gate reads what runs.
fn shipped_sources(directory: &Path) -> Vec<(String, String)> {
    fn walk(root: &Path, directory: &Path, found: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(directory).expect("a source directory") {
            let path = entry.expect("a directory entry").path();
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            if path.is_dir() {
                if name != "tests" {
                    walk(root, &path, found);
                }
                continue;
            }
            if !name.ends_with(".rs") || name == "tests.rs" || name.ends_with("_tests.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("a source file");
            let shipped = text
                .split_once("#[cfg(test)]")
                .map_or(text.as_str(), |(head, _)| head);
            let relative = path
                .strip_prefix(root)
                .expect("under the root")
                .display()
                .to_string();
            found.push((relative, strip_rust_comments(shipped)));
        }
    }
    let mut found = Vec::new();
    walk(directory, directory, &mut found);
    found
}

/// The ledger counts every launch only if every launch goes in by its one door. A
/// road that called `run_once` itself would run an agent's CLI outside the
/// ceilings and outside a provider's rest, and would be found by nobody — so the
/// two places `run_once` is named are its definition and the door, and a third is
/// a failure of this test.
#[test]
fn no_road_runs_an_agents_cli_headless_except_through_the_launch_ledgers_door() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let sources = shipped_sources(&source);
    assert!(
        sources.len() > 100,
        "the walk found {} files: it is measuring nothing",
        sources.len()
    );
    let mut callers: Vec<(String, usize)> = sources
        .iter()
        .map(|(file, code)| (file.clone(), code.matches("run_once(").count()))
        .filter(|(_, count)| *count > 0)
        .collect();
    callers.sort();
    assert_eq!(
        callers,
        [
            ("launch_budget_runtime.rs".to_string(), 1),
            ("scm_runtime.rs".to_string(), 1),
        ],
        "`run_once(` is named in its definition and in the launch ledger's door and nowhere else; \
         every other road goes in by `run_budgeted`"
    );
}
