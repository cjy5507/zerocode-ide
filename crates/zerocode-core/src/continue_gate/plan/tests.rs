use super::*;
use crate::continue_gate::{Code, Reason, StepBook};

const NOW: i64 = 1_800_000_000_000;
const SECOND: i64 = 1_000;

/// A judgment at `verdict`, with the one reason that earns it.
fn judged(verdict: Verdict) -> Judgement {
    let code = match verdict {
        Verdict::Continue => None,
        Verdict::Checkpoint => Some(Code::CheckpointDue),
        Verdict::Pause => Some(Code::ReworkLoop),
        Verdict::Stop => Some(Code::TaskBudgetStop),
    };
    Judgement {
        verdict,
        reasons: code
            .map(|code| Reason {
                code,
                value: 1.0,
                limit: 1.0,
            })
            .into_iter()
            .collect(),
        metrics: StepBook::default().metrics(),
    }
}

#[test]
fn nothing_is_planned_while_the_gate_is_off() {
    let mut standing = Standing::default();
    for verdict in [Verdict::Checkpoint, Verdict::Pause, Verdict::Stop] {
        assert_eq!(standing.plan(&judged(verdict), Mode::Off, NOW), vec![]);
    }
}

#[test]
fn a_calm_judgment_plans_nothing() {
    let mut standing = Standing::default();
    for beat in 0..30 {
        assert_eq!(
            standing.plan(&judged(Verdict::Continue), Mode::Stop, NOW + beat * SECOND),
            vec![]
        );
    }
}

#[test]
fn a_checkpoint_is_saved_once_per_gap_however_long_it_is_asked_for() {
    let mut standing = Standing::default();
    let checkpoint = judged(Verdict::Checkpoint);
    assert_eq!(
        standing.plan(&checkpoint, Mode::Notify, NOW),
        vec![Act::Snapshot]
    );
    for beat in 1..10 {
        assert_eq!(
            standing.plan(&checkpoint, Mode::Notify, NOW + beat * SECOND),
            vec![]
        );
    }
    assert_eq!(
        standing.plan(&checkpoint, Mode::Notify, NOW + CHECKPOINT_MIN_GAP_MS - 1),
        vec![]
    );
    assert_eq!(
        standing.plan(&checkpoint, Mode::Notify, NOW + CHECKPOINT_MIN_GAP_MS),
        vec![Act::Snapshot]
    );
}

#[test]
fn a_pause_is_told_once_and_a_stop_when_it_rises_to_one() {
    let mut standing = Standing::default();
    assert_eq!(
        standing.plan(&judged(Verdict::Pause), Mode::Notify, NOW),
        vec![Act::Snapshot, Act::Tell(Verdict::Pause)]
    );
    for beat in 1..10 {
        assert_eq!(
            standing.plan(&judged(Verdict::Pause), Mode::Notify, NOW + beat * SECOND),
            vec![]
        );
    }
    assert_eq!(
        standing.plan(&judged(Verdict::Stop), Mode::Notify, NOW + 10 * SECOND),
        vec![Act::Tell(Verdict::Stop)],
        "the snapshot of ten seconds ago stands, and a notifying gate ends nobody"
    );
    assert_eq!(
        standing.plan(&judged(Verdict::Stop), Mode::Notify, NOW + 11 * SECOND),
        vec![]
    );
}

#[test]
fn a_gate_that_only_tells_never_stops_anyone() {
    let mut standing = Standing::default();
    for beat in 0..200 {
        let acts = standing.plan(&judged(Verdict::Stop), Mode::Notify, NOW + beat * SECOND);
        assert!(!acts.contains(&Act::Stop), "beat {beat}: {acts:?}");
    }
}

#[test]
fn in_stop_mode_a_stop_follows_its_snapshot_and_its_word_and_is_asked_again_only_after_its_time() {
    let mut standing = Standing::default();
    let stop = judged(Verdict::Stop);
    assert_eq!(
        standing.plan(&stop, Mode::Stop, NOW),
        vec![Act::Snapshot, Act::Tell(Verdict::Stop), Act::Stop],
        "saved, told, then ended — in that order"
    );
    assert_eq!(standing.plan(&stop, Mode::Stop, NOW + SECOND), vec![]);
    assert_eq!(
        standing.plan(&stop, Mode::Stop, NOW + STOP_RETRY_MS - 1),
        vec![]
    );
    assert_eq!(
        standing.plan(&stop, Mode::Stop, NOW + STOP_RETRY_MS),
        vec![Act::Snapshot, Act::Stop],
        "the stop did not land: asked again, the word not repeated"
    );
}

#[test]
fn a_stop_that_follows_a_fresh_snapshot_does_not_take_another() {
    let mut standing = Standing::default();
    assert_eq!(
        standing.plan(&judged(Verdict::Checkpoint), Mode::Stop, NOW),
        vec![Act::Snapshot]
    );
    assert_eq!(
        standing.plan(
            &judged(Verdict::Stop),
            Mode::Stop,
            NOW + STOP_SNAPSHOT_MAX_AGE_MS - 1
        ),
        vec![Act::Tell(Verdict::Stop), Act::Stop]
    );
}

#[test]
fn a_flapping_attempt_is_one_episode_and_a_calm_one_is_told_about_afresh() {
    let mut standing = Standing::default();
    let calm = judged(Verdict::Continue);
    let pause = judged(Verdict::Pause);
    assert_eq!(
        standing.plan(&pause, Mode::Notify, NOW),
        vec![Act::Snapshot, Act::Tell(Verdict::Pause)]
    );
    // It flaps: calm for a second, wrong again — one episode.
    standing.plan(&calm, Mode::Notify, NOW + SECOND);
    assert_eq!(
        standing.plan(&pause, Mode::Notify, NOW + 2 * SECOND),
        vec![]
    );
    // Calm, but not for the whole of the time: wrong again, still one episode.
    let calm_from = NOW + 3 * SECOND;
    standing.plan(&calm, Mode::Notify, calm_from);
    standing.plan(&calm, Mode::Notify, calm_from + REARM_MS - 1);
    assert_eq!(
        standing.plan(&pause, Mode::Notify, calm_from + REARM_MS - 1),
        vec![]
    );
    // Calm for the whole of it: the next time it goes wrong is a new episode.
    let calm_again = calm_from + REARM_MS;
    standing.plan(&calm, Mode::Notify, calm_again);
    standing.plan(&calm, Mode::Notify, calm_again + REARM_MS);
    assert_eq!(
        standing.plan(&pause, Mode::Notify, calm_again + REARM_MS + SECOND),
        vec![Act::Snapshot, Act::Tell(Verdict::Pause)],
        "calm for ten minutes and wrong again is told afresh"
    );
}
