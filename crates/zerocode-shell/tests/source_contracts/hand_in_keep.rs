//! What a worker hands in is kept before its checkout goes (t-32798): one keeping
//! for every road that takes a checkout, one road in for every agent CLI, and one
//! table of words between the backend that says why a file was left out and the
//! board that says it to a person.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use super::quiet_children::shell_sources;
use super::support::block_after;

fn shell_source(name: &str) -> String {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    std::fs::read_to_string(src.join(name)).unwrap_or_else(|why| panic!("{name}: {why}"))
}

fn repo_source(name: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    std::fs::read_to_string(root.join(name)).unwrap_or_else(|why| panic!("{name}: {why}"))
}

/// Where `needle` first stands in `haystack`, or a failed assertion that says
/// what moved: a pin that cannot find its text fails as an assertion, never as a
/// panic from a helper.
fn at(haystack: &str, needle: &str) -> usize {
    let found = haystack.find(needle);
    assert!(found.is_some(), "`{needle}` is gone:\n{haystack}");
    found.unwrap_or_default()
}

/// The block `marker` opens, or a failed assertion that says it is gone.
fn block<'a>(source: &'a str, marker: &str) -> &'a str {
    assert!(source.contains(marker), "`{marker}` is gone");
    block_after(source, marker)
}

/// Every quoted word that follows a `=>` in `block`.
fn words_after_arrows(block: &str) -> BTreeSet<String> {
    block
        .lines()
        .filter_map(|line| {
            let (_, after) = line.split_once("=> \"")?;
            Some(after.split('"').next()?.to_string())
        })
        .collect()
}

/// Every key a JavaScript object literal's lines open with (`  word: {`).
fn keys_of(block: &str) -> BTreeSet<String> {
    block
        .lines()
        .skip(1)
        .filter_map(|line| {
            let line = line.trim_start();
            let (key, rest) = line.split_once(": {")?;
            (key.chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
                && rest.contains("key:"))
            .then(|| key.to_string())
        })
        .collect()
}

/// Every road that takes a checkout asks the keeping first, and the person's
/// removal keeps on the spot while the beat's sweep only starts it: the report
/// and the evidence are written in the very directory the removal ends.
#[test]
fn every_road_that_takes_a_checkout_asks_the_keeping_first() {
    let worktree = shell_source("cmd/worktree.rs");
    let remove = block(&worktree, "pub(crate) async fn remove_worktree(");
    let asked = at(remove, "hand_in_keep::before_cleanup(");
    assert!(
        remove[asked..].starts_with("hand_in_keep::before_cleanup(")
            && remove[asked..asked + 220].contains("hand_in_keep::Mode::Now"),
        "the person's removal does not keep on the spot:\n{remove}"
    );
    assert!(
        asked > at(remove, "CheckoutHeld::take(&chosen.path)"),
        "the keeping is asked outside the claim every road takes"
    );
    assert!(
        asked < at(remove, "archive_worktree(") && asked < at(remove, "Removal::Confirmed"),
        "the keeping is asked after the directory was already worked in"
    );

    let reclaim = shell_source("worktree_reclaim.rs");
    let shipped = reclaim
        .split_once("\n#[cfg(test)]\nmod tests")
        .map_or(reclaim.as_str(), |(before, _)| before);
    assert!(
        block(shipped, "struct Recheck<'a> {").contains("keeping: &'a dyn Fn(&Path) -> Clearance"),
        "the judgment's last questions lost the keeping"
    );
    assert!(
        block(shipped, "fn judge_and_act(").contains("before_cleanup(path, Mode::Wait)"),
        "the beat asks the keeping in a way that reads files on its thread"
    );
    let settle = block(shipped, "fn settle(");
    let keeping = at(settle, "(recheck.keeping)(&path)");
    assert!(
        keeping < at(settle, "let owner = match road")
            && keeping < at(settle, "look(&orchestrator"),
        "git is asked something before the keeping is:\n{settle}"
    );
    assert!(
        settle[keeping..].contains("Clearance::Keeping(_) => return Settled::Kept")
            && settle[keeping..].contains("Clearance::Held(because)")
            && settle[keeping..].contains("remember(candidate, Standing::Kept, &because"),
        "a held keeping is not written down once with its reason:\n{settle}"
    );
}

/// The roads that take a checkout ([`CheckoutHeld::take`]) and ask the keeping
/// first.
const ASKS_THE_KEEPING: &[&str] = &["cmd/worktree.rs", "worktree_reclaim.rs"];

/// The roads that take a folder and are not a worker's checkout, with why: a
/// hand-in is read from the worker's checkout and the temporary folders, so
/// neither of these can be holding one.
const NOT_A_WORKERS: &[(&str, &str)] = &[
    (
        "automation_runtime.rs",
        "an automation run's own isolated worktree, never a ledger worker's checkout",
    ),
    (
        "agent_tools_runtime.rs",
        "a worktree an agent's tool asked to be isolated in, outside the roots a hand-in is read from",
    ),
];

/// A file that only holds tests (`main_unit_tests.rs`, a `tests.rs`, a folder of
/// them) takes a checkout to prove a road, never to be one.
fn holds_only_tests(name: &str) -> bool {
    name.ends_with("tests.rs") || name.contains("/tests/")
}

/// Every road that takes a checkout is found by what it must hold
/// ([`CheckoutHeld::take`]), not by a list: a new road that deletes a folder
/// fails here until someone says whether it asks the keeping first or is not a
/// worker's checkout at all.
#[test]
fn every_road_that_takes_a_checkout_asks_the_keeping_or_says_here_why_it_need_not() {
    let mut takers: BTreeMap<String, bool> = BTreeMap::new();
    for (name, source) in shell_sources() {
        if holds_only_tests(&name) {
            continue;
        }
        let shipped = source
            .split_once("\n#[cfg(test)]\nmod tests")
            .map_or(source.as_str(), |(before, _)| before);
        if shipped.contains("CheckoutHeld::take(") {
            takers.insert(name, shipped.contains("before_cleanup("));
        }
    }
    let found: Vec<&str> = takers.keys().map(String::as_str).collect();
    let mut known: Vec<&str> = ASKS_THE_KEEPING
        .iter()
        .copied()
        .chain(NOT_A_WORKERS.iter().map(|(name, _)| *name))
        .collect();
    known.sort_unstable();
    assert_eq!(
        found, known,
        "a road takes a checkout that this test does not know: it asks `hand_in_keep::before_cleanup`, or it is listed with its reason as not a worker's"
    );
    for name in ASKS_THE_KEEPING {
        assert!(
            takers[*name],
            "{name} takes a checkout and does not ask the keeping first"
        );
    }
}

/// A hand-in reaches the keeping from three places and from no agent's own
/// door: the send the ledger accepted, the release of a seat, and the board's
/// beat that dresses rows. The file holds no branch on an agent, never reads
/// a file on the paint path, and writes the store through the one road that
/// masks.
#[test]
fn a_hand_in_is_kept_by_one_road_for_every_agent_cli() {
    let orchestration = shell_source("orchestration.rs");
    let shipped = orchestration
        .split_once("\n#[cfg(test)]\npub(crate) mod tests")
        .map_or(orchestration.as_str(), |(before, _)| before);
    assert!(
        shipped.contains("hand_in_keep::after_send(argv, &answered);")
            && !shipped.contains("fn note_worker_report("),
        "a send that names files is not handed to the keeping, or the old verbatim road is back"
    );
    let release = block(shipped, "Effect::WorkerTerminal {");
    assert!(
        at(release, "host.close(term);") < at(release, "hand_in_keep::note_release(&seat.worker)"),
        "a released seat does not hand what it filed to the keeping"
    );
    let refresh = block(shipped, "pub(crate) fn refresh_board_ledger() {");
    assert!(
        at(refresh, "hand_in_keep::dress(&mut next);") < at(refresh, "let next = Arc::new(next);"),
        "the board's beat does not dress its rows with what was kept"
    );

    let keeping = shell_source("orchestration/hand_in_keep.rs");
    let keeping = keeping
        .split_once("\n#[cfg(test)]\nmod tests;")
        .map_or(keeping.as_str(), |(before, _)| before);
    for forbidden in [
        "AgentKind",
        "\"claude\"",
        "\"codex\"",
        "Command::new(",
        "std::fs::copy(",
        "register_copy(",
        "register_report(",
        "std::thread::spawn",
    ] {
        assert!(
            !keeping.contains(forbidden),
            "the keeping grew a branch of its own (`{forbidden}`)"
        );
    }
    for (needle, why) in [
        (
            "register_kept(",
            "the one store road that masks, caps and names",
        ),
        ("libc::PRIO_DARWIN_BG", "the lowest priority the system has"),
        ("std::thread::Builder", "a thread of its own"),
        ("static ONE_AT_A_TIME", "one keeping at a time"),
        ("hand_in::named(", "the one parser of what a payload names"),
    ] {
        assert!(
            keeping.contains(needle),
            "the keeping lost {why} (`{needle}`)"
        );
    }
}

/// The words the backend says a file was left out for are the words the board
/// translates — one table between them, and no word on one side the other does
/// not know: a reason with no sentence would draw as its raw name.
#[test]
fn the_board_has_a_sentence_for_every_state_and_reason_the_backend_can_say() {
    let core = repo_source("crates/zerocode-core/src/hand_in.rs");
    let board = repo_source("ui/shell-board.js");
    let mut reasons = words_after_arrows(block(&core, "impl Why {"));
    reasons.extend(words_after_arrows(block(&core, "impl Fault {")));
    reasons.retain(|word| word.chars().all(|ch| ch.is_ascii_lowercase() || ch == '_'));
    let table = keys_of(block(&board, "const KEPT_REASONS = Object.freeze({"));
    assert_eq!(
        reasons, table,
        "the backend's reasons and the board's table differ — a word with no sentence draws raw"
    );
    let mut states = words_after_arrows(block(&core, "impl State {"));
    states.insert("keeping".to_string());
    assert_eq!(
        states,
        keys_of(block(&board, "const KEPT_STATES = Object.freeze({")),
        "the backend's states and the board's table differ"
    );
}

/// Every sentence the board writes about a keeping is in the four catalogs beside
/// the Korean one that stands where it is used.
#[test]
fn every_kept_sentence_is_in_the_four_catalogs() {
    let board = repo_source("ui/shell-board.js");
    let catalog = repo_source("ui/shell-i18n.js");
    let prefix = "\"board.desk.kept.";
    let mut keys = BTreeSet::new();
    for (at, _) in board.match_indices(prefix) {
        let key = board[at + 1..].split('"').next().expect("a closing quote");
        keys.insert(key.to_string());
    }
    assert!(
        keys.len() >= 20,
        "the board stopped naming its kept sentences: {keys:?}"
    );
    for key in keys {
        let held = catalog.matches(&format!("\"{key}\":")).count();
        assert_eq!(
            held, 4,
            "`{key}` is in {held} of the four catalogs (en, ja, zh, es)"
        );
    }
}
