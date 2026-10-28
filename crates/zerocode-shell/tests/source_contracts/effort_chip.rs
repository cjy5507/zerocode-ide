//! t-81532: the composer's effort chip. A press reaches a pane only through
//! the window's one command, from the main window, off its threads; it walks
//! the step-effort seat's own picker and never types the effort word that
//! saves the account's default; the beat never reaches it; the renderer asks
//! the person once more before a move that rewrites the cache; and every
//! reason a press can be refused has words in the window.

use std::path::Path;

fn shell_source(name: &str) -> String {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    std::fs::read_to_string(src.join(name)).unwrap_or_else(|why| panic!("{name}: {why}"))
}

fn core_source(name: &str) -> String {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../zerocode-core/src");
    std::fs::read_to_string(src.join(name)).unwrap_or_else(|why| panic!("{name}: {why}"))
}

#[test]
fn a_press_proves_the_main_window_and_runs_off_its_threads_and_the_beat_never_reaches_it() {
    let terminal = shell_source("cmd/terminal.rs");
    let body = super::support::block_after(&terminal, "pub(crate) async fn pane_effort_press(");
    let main_only = body
        .find("crate::from_the_main_webview(&webview)?")
        .unwrap_or_else(|| panic!("the press is open beyond the main window:\n{body}"));
    let pressed = body.find("step_effort::press(").expect("the press");
    assert!(
        main_only < pressed && body.contains("spawn_blocking"),
        "the press goes before it proves the main window, or on the window's thread:\n{body}"
    );
    let orchestration = shell_source("orchestration.rs");
    let tick = super::support::block_after(&orchestration, "pub(crate) fn tick(");
    assert!(
        !tick.contains("pane_effort") && !tick.contains("step_effort::press"),
        "the beat reaches a person's press:\n{tick}"
    );
    let seat = shell_source("orchestration/step_effort.rs");
    let sweep = super::support::block_after(&seat, "pub(super) fn sweep(");
    assert!(
        !sweep.contains("press(") && sweep.contains(".pressed"),
        "the seat's sweep presses, or does not ask which panes a person moved:\n{sweep}"
    );
}

#[test]
fn a_press_drives_the_bare_picker_and_never_types_the_word_that_saves_the_default() {
    let chip = core_source("effort_chip.rs");
    let press = super::support::block_after(&chip, "pub fn press(");
    assert!(
        press.contains("keys.session_only")
            && press.contains("keys.raise")
            && press.contains("keys.lower"),
        "the press does not drive the picker's own keys:\n{press}"
    );
    assert!(
        !press.contains("format!"),
        "the press builds a line of its own — `/effort <level>` saves the account's default:\n{press}"
    );
    let seat = shell_source("orchestration/step_effort.rs");
    let carry = super::support::block_after(&seat, "fn carry(");
    let delivered = carry
        .find("host\n        .point(")
        .or_else(|| carry.find("host.point("))
        .expect("the guarded door");
    let legend = carry
        .find("keys.legend")
        .expect("the legend licenses the keys");
    let sent = carry.find("host.send(").expect("the keys");
    assert!(
        delivered < legend && legend < sent,
        "a key goes before the command is delivered or before the legend shows:\n{carry}"
    );
}

#[test]
fn the_renderer_asks_once_more_before_a_move_that_rewrites_the_cache() {
    let window = super::support::window_source();
    assert_eq!(
        window.matches("\"pane_effort_press\"").count(),
        1,
        "the press is invoked from more than its one door"
    );
    let moving = super::support::block_after(window, "async function movePaneEffort(");
    assert!(
        moving.contains("\"pane_effort_press\""),
        "the press lost its door:\n{moving}"
    );
    let pressing = super::support::block_after(window, "async function pressPaneEffort(");
    let asks = pressing
        .find("said.rewritesCache")
        .expect("the cost is asked about");
    let confirm = pressing.find("composer.effort.confirm\"").expect("the yes");
    let moved = pressing.rfind("movePaneEffort(").expect("the move");
    assert!(
        asks < confirm && pressing[..confirm].contains("effortCacheWords(run)"),
        "the yes stands without the cost in front of it:\n{pressing}"
    );
    assert!(moved > confirm, "{pressing}");
    assert_eq!(
        window.matches("invoke(\"pane_effort\"").count(),
        1,
        "the chip's word is asked from more than its one reader"
    );
}

/// The serde code of one variant line of a `rename_all = "snake_case"` enum.
fn snake(name: &str) -> String {
    let mut code = String::new();
    for (at, letter) in name.chars().enumerate() {
        if letter.is_ascii_uppercase() {
            if at > 0 {
                code.push('_');
            }
            code.push(letter.to_ascii_lowercase());
        } else {
            code.push(letter);
        }
    }
    code
}

#[test]
fn every_reason_a_press_is_refused_has_words_in_the_window() {
    let chip = core_source("effort_chip.rs");
    let stuck = super::support::block_after(&chip, "pub enum Stuck {");
    let codes: Vec<String> = stuck
        .lines()
        .filter_map(|line| line.strip_prefix("    "))
        .filter(|line| !line.starts_with(['/', '#', ' ']))
        .filter_map(|line| line.strip_suffix(','))
        .map(snake)
        .collect();
    assert!(codes.len() >= 10, "{codes:?}");
    let window = super::support::window_source();
    let words = super::support::block_after(window, "function effortStuckWords(");
    for code in &codes {
        assert!(
            words.contains(&format!("case \"{code}\":")),
            "the window has no words for `{code}`:\n{words}"
        );
    }
    let refusal = super::support::block_after(window, "function effortRefusalWords(");
    for door in ["withheld", "picker_missing"] {
        assert!(
            refusal.contains(&format!("\"{door}\"")),
            "no words for `{door}`:\n{refusal}"
        );
    }
}
