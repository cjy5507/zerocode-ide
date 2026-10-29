use super::*;
use crate::capabilities::{CLAUDE_EFFORT_PICKER, agent_capabilities};
use crate::transcript::effort_in;

fn moves_of(agent: &str) -> TurnMoves {
    agent_capabilities(agent).expect(agent).moves
}

/// One Claude Code assistant record at `effort`, as 2.1.278 writes it.
fn claude_turn(effort: &str) -> String {
    serde_json::json!({
        "type": "assistant",
        "effort": effort,
        "perTurnEffort": effort,
        "message": {"role": "assistant", "content": [{"type": "text", "text": "done"}]},
    })
    .to_string()
}

#[test]
fn the_sessions_own_record_outranks_the_summons_which_outranks_the_default() {
    let record = claude_turn("low");
    let turn = effort_in(&record);
    assert_eq!(turn.as_deref(), Some("low"));
    assert_eq!(
        shown(turn.as_deref(), Some("max")),
        (Some("low"), EffortWitness::Turn)
    );
    assert_eq!(
        shown(None, Some("max")),
        (Some("max"), EffortWitness::Summons)
    );
    assert_eq!(shown(None, Some("  ")), (None, EffortWitness::Default));
    assert_eq!(shown(None, None), (None, EffortWitness::Default));
}

#[test]
fn a_claude_pane_at_rest_on_a_rung_can_be_pressed_and_says_the_cache_cost() {
    let moves = moves_of("claude");
    let chip = chip(Some(&moves), Some("high"), Some("max"), None, Ok(()));
    assert_eq!(chip.current.as_deref(), Some("high"));
    assert_eq!(chip.witness, EffortWitness::Turn);
    assert_eq!(chip.stuck, None);
    assert_eq!(chip.door, Some("picker"));
    assert!(chip.rewrites_cache);
    assert_eq!(chip.ladder, &["low", "medium", "high", "xhigh", "max"]);
}

#[test]
fn an_agent_with_no_road_between_turns_is_shown_and_never_pressed() {
    for (agent, why) in [
        ("codex", Stuck::Relaunch),
        ("antigravity", Stuck::Relaunch),
        ("zo", Stuck::OnScreen),
    ] {
        let moves = moves_of(agent);
        let shown = chip(Some(&moves), Some("high"), None, None, Ok(()));
        assert_eq!(shown.current.as_deref(), Some("high"), "{agent}");
        assert_eq!(shown.stuck, Some(why), "{agent}");
        assert_eq!(press(&moves, "high", "low"), Err(why), "{agent}");
    }
    let none = chip(None, None, Some("medium"), None, Ok(()));
    assert_eq!(none.stuck, Some(Stuck::NoRoad));
    assert_eq!(none.current.as_deref(), Some("medium"));
    assert_eq!(none.witness, EffortWitness::Summons);
    let zo = chip(Some(&moves_of("zo")), None, None, None, Ok(()));
    assert_eq!(zo.command, Some("/model"));
}

#[test]
fn a_pane_mid_turn_or_with_the_persons_words_on_its_line_refuses_the_press() {
    let moves = moves_of("claude");
    for busy in [
        Stuck::TurnRunning,
        Stuck::Draft,
        Stuck::HeldByPerson,
        Stuck::NeverHeard,
        Stuck::ShellInFront,
        Stuck::SeatMoving,
        Stuck::PressMoving,
    ] {
        let chip = chip(Some(&moves), Some("high"), None, None, Err(busy));
        assert_eq!(chip.stuck, Some(busy));
        assert_eq!(chip.current.as_deref(), Some("high"));
    }
}

#[test]
fn the_road_outranks_the_moment_and_an_unknown_rung_outranks_the_moment() {
    let codex = moves_of("codex");
    let chip_codex = chip(
        Some(&codex),
        Some("xhigh"),
        None,
        None,
        Err(Stuck::TurnRunning),
    );
    assert_eq!(chip_codex.stuck, Some(Stuck::Relaunch));
    let claude = moves_of("claude");
    let fresh = chip(Some(&claude), None, None, None, Ok(()));
    assert_eq!(fresh.stuck, Some(Stuck::Unknown));
    assert_eq!(fresh.witness, EffortWitness::Default);
    let odd = chip(Some(&claude), Some("ultracode"), None, None, Ok(()));
    assert_eq!(odd.stuck, Some(Stuck::OffLadder));
}

#[test]
fn a_pressed_word_stands_until_the_sessions_record_catches_up() {
    let moves = moves_of("claude");
    let waiting = chip(Some(&moves), Some("high"), None, Some("max"), Ok(()));
    assert_eq!(waiting.pending.as_deref(), Some("max"));
    assert_eq!(waiting.current.as_deref(), Some("high"));
    let landed = chip(Some(&moves), Some("max"), None, Some("max"), Ok(()));
    assert_eq!(landed.pending, None);
    // A session that has not said where it stands still moves from the word
    // a press put it on.
    let pressed_fresh = chip(Some(&moves), None, None, Some("low"), Ok(()));
    assert_eq!(pressed_fresh.stuck, None);
}

#[test]
fn a_press_drives_the_picker_one_arrow_a_rung_then_this_session_only() {
    let moves = moves_of("claude");
    let up = press(&moves, "medium", "max").expect("moves");
    assert_eq!(
        up,
        Some(PressKeys::Picker {
            keys: CLAUDE_EFFORT_PICKER,
            after_legend: vec!["\x1b[C", "\x1b[C", "\x1b[C", "s"],
        })
    );
    let down = press(&moves, "high", "low").expect("moves");
    assert_eq!(
        down,
        Some(PressKeys::Picker {
            keys: CLAUDE_EFFORT_PICKER,
            after_legend: vec!["\x1b[D", "\x1b[D", "s"],
        })
    );
    assert_eq!(press(&moves, "high", "high"), Ok(None));
    assert_eq!(press(&moves, "high", "ultracode"), Err(Stuck::OffLadder));
}

#[test]
fn no_press_ever_types_the_effort_word_that_saves_the_accounts_default() {
    let moves = moves_of("claude");
    for from in moves.ladder {
        for to in moves.ladder {
            let Ok(Some(PressKeys::Picker { keys, after_legend })) = press(&moves, from, to) else {
                continue;
            };
            assert_eq!(keys.open, "/effort", "the picker opens bare");
            assert!(
                after_legend.iter().all(|key| !key.contains(to)),
                "{from}->{to}"
            );
        }
    }
}
