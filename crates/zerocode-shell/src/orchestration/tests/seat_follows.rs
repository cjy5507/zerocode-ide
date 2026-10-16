//! t-57705: the coordinator seat follows its conversation. The conversation
//! that holds a run's live seat goes on in a new leader pane — the person
//! closed the old pane at its quota wall and resumed the conversation in a
//! new one, on 2026-10-15 19:15 (terminal 9 → 20) and again on 2026-10-16
//! 07:29 (terminal 4 → 26) — and the pane it sat from is no longer that
//! conversation's pane in this window. The seat moves to the new pane by the
//! takeover road on the next beat, with nothing typed and no `run-takeover`
//! from the person. A twin still open in a tab, and a seat another
//! conversation holds, stay where they are, and the window's one line names
//! the pane that really holds the seat.

use super::*;
use std::collections::{HashMap, HashSet};
use zerocode_core::orchestration::MessageKind;

/// A window whose leader panes show the conversations a test sets: each
/// terminal shows the conversation named by the terminal it STARTED in
/// (`test_actor`), and `detached` holds the panes the person closed to the
/// background with their agent still running. Every keystroke is recorded.
struct Moving {
    shows: Mutex<HashMap<u32, u32>>,
    detached: Mutex<HashSet<u32>>,
    sends: Mutex<Vec<(u32, String)>>,
}

impl Moving {
    fn new(shows: &[(u32, u32)]) -> Self {
        Self {
            shows: Mutex::new(shows.iter().copied().collect()),
            detached: Mutex::new(HashSet::new()),
            sends: Mutex::new(Vec::new()),
        }
    }

    /// The pane at `term` now shows the conversation that began in
    /// `conversation`'s terminal.
    fn show(&self, term: u32, conversation: u32) {
        self.shows
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .insert(term, conversation);
    }

    /// The person closes the pane at `term`; its agent keeps running.
    fn detach(&self, term: u32) {
        self.detached
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .insert(term);
    }

    fn typed(&self) -> usize {
        self.sends
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .len()
    }
}

impl Host for Moving {
    fn split(
        &self,
        _team: &str,
        _leader_term: u32,
        _from_term: u32,
        _pane: &str,
        _direction: zerocode_core::agent_teams::Direction,
        _command: &str,
        _token: &str,
    ) -> Option<u32> {
        None
    }
    fn send(&self, term: u32, text: &str) -> bool {
        self.sends
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .push((term, text.to_string()));
        true
    }
    fn capture(&self, _term: u32) -> Option<String> {
        Some(String::new())
    }
    fn focus(&self, _term: u32) -> bool {
        true
    }
    fn close(&self, _term: u32) {}
    fn actor_for(&self, term: u32) -> Option<String> {
        self.shows
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .get(&term)
            .map(|conversation| test_actor(*conversation))
    }
    fn pane_detached(&self, term: u32) -> bool {
        self.detached
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .contains(&term)
    }
}

/// The seat of `run_id`, as the rows hold it.
fn seat_of(run_id: &str) -> zerocode_core::orchestration::CoordinatorSeat {
    the_rows()
        .runs
        .iter()
        .find(|run| run.id == run_id)
        .and_then(|run| run.coordinator.clone())
        .expect("the seat record")
}

/// A run opened from the leader pane of `team` at `term`, which shows its own
/// conversation: the pane the seat sits in.
fn a_run_sat_from(host: &Moving, team: &str, term: u32, name: &str) -> String {
    seat_a_team(team, term);
    host.show(term, term);
    let opened = run(
        host,
        Vec::new(),
        team,
        zerocode_core::agent_teams::LEADER_PANE,
        TEST_CAPABILITY,
        &words(&format!("run-create --name {name}")),
        clock(),
    );
    assert_eq!(opened.exit_code, 0, "{}", opened.stderr);
    let opened: serde_json::Value = serde_json::from_str(&opened.stdout).expect("a run");
    opened["runId"].as_str().expect("a run id").to_string()
}

/// One coordinator verb from the leader pane of `team`, as the shim sends it.
fn from_leader(host: &Moving, team: &str, line: &str) -> zerocode_hookd::TeamAnswer {
    run(
        host,
        Vec::new(),
        team,
        zerocode_core::agent_teams::LEADER_PANE,
        TEST_CAPABILITY,
        &words(line),
        clock(),
    )
}

/// The window log's lines about `run_id` that carry `needle`.
fn logged(run_id: &str, needle: &str) -> Vec<String> {
    let blackbox = super::super::BLACKBOX
        .get()
        .expect("the bench window's black box")
        .join("window-errors.log");
    std::fs::read_to_string(blackbox)
        .unwrap_or_default()
        .lines()
        .filter(|line| line.contains(run_id) && line.contains(needle))
        .map(str::to_string)
        .collect()
}

/// 2026-10-16 07:29, the case this task is named for: the coordinator's pane
/// (terminal 4) stood at its quota wall, the person closed it — its agent kept
/// running — and resumed the same conversation in a new pane (terminal 26).
/// The ledger kept the seat on the closed pane for 5 min 32 s, refused the
/// new pane's `task-update`, and moved only when the person typed
/// `run-takeover`. Now the next beat moves it: one generation up, nothing
/// typed, the closed pane told in its own inbox, and the new pane's
/// coordinator verbs taken at once.
#[test]
fn a_coordinator_conversation_resumed_in_a_new_pane_takes_its_seat_there() {
    const OLD: u32 = 57_704;
    const NEW: u32 = 57_726;
    let _window = the_window();
    let _turn = one_beat_at_a_time();
    let _young = BootedHere::at(clock());
    let host = Moving::new(&[]);
    let leader = zerocode_core::agent_teams::LEADER_PANE;
    let old_team = format!("team-seat-follows-old-{OLD}");
    let run_id = a_run_sat_from(&host, &old_team, OLD, "seat-follows");
    let before = seat_of(&run_id);
    assert_eq!(before.seat, format!("{old_team}/{leader}"));
    let task = from_leader(&host, &old_team, "task-create --spec follows");
    assert_eq!(task.exit_code, 0, "{}", task.stderr);
    let task: serde_json::Value = serde_json::from_str(&task.stdout).expect("a task");
    let task = task["taskId"].as_str().expect("a task id").to_string();

    // The person closes the old pane; its agent keeps running. The same
    // conversation goes on in a new pane, of a new team.
    host.detach(OLD);
    let new_team = format!("team-seat-follows-new-{NEW}");
    seat_a_team(&new_team, NEW);
    host.show(NEW, OLD);
    super::super::tick(&host, &[], clock());

    let after = seat_of(&run_id);
    assert_eq!(
        (after.seat.as_str(), after.generation, after.vacated_ms),
        (
            format!("{new_team}/{leader}").as_str(),
            before.generation + 1,
            None
        ),
        "the seat did not follow its conversation into the new pane"
    );
    assert_eq!(host.typed(), 0, "the seat was moved by typing into a pane");
    let told = the_rows()
        .messages
        .iter()
        .filter(|message| {
            message.run == run_id
                && message.to == format!("pane:{old_team}/{leader}")
                && message.kind == MessageKind::Handoff
        })
        .count();
    assert_eq!(
        told, 1,
        "the pane the seat left was not told in its own inbox"
    );
    let followed = logged(&run_id, "followed");
    assert!(
        followed.len() == 1 && followed[0].contains(&format!("terminal {NEW}")),
        "the window's log did not say once where the seat went: {followed:?}"
    );

    // The new pane is the coordinator now: its `task-update` is taken, and
    // a second beat moves nothing.
    let updated = from_leader(
        &host,
        &new_team,
        &format!("task-update --task {task} --status ready --retry-request t57705-follows"),
    );
    assert_eq!(
        updated.exit_code, 0,
        "the new pane's task-update was refused: {}",
        updated.stderr
    );
    super::super::tick(&host, &[], clock());
    assert_eq!(
        seat_of(&run_id).generation,
        after.generation,
        "a second beat moved the seat again"
    );
    crate::agent_teams::forget_term(NEW);
    crate::agent_teams::forget_term(OLD);
}

/// The pane the seat sat from shows another conversation now (a `/clear`, a
/// resume of something else): it is not this conversation's pane any more,
/// and the seat follows the conversation even though that pane is in a tab.
#[test]
fn the_seat_follows_its_conversation_out_of_a_pane_that_shows_another_one() {
    const OLD: u32 = 57_734;
    const NEW: u32 = 57_735;
    const ELSEWHERE: u32 = 57_736;
    let _window = the_window();
    let _turn = one_beat_at_a_time();
    let _young = BootedHere::at(clock());
    let host = Moving::new(&[]);
    let leader = zerocode_core::agent_teams::LEADER_PANE;
    let old_team = format!("team-seat-follows-cleared-{OLD}");
    let run_id = a_run_sat_from(&host, &old_team, OLD, "seat-follows-cleared");
    let before = seat_of(&run_id);

    host.show(OLD, ELSEWHERE);
    let new_team = format!("team-seat-follows-cleared-new-{NEW}");
    seat_a_team(&new_team, NEW);
    host.show(NEW, OLD);
    super::super::tick(&host, &[], clock());

    let after = seat_of(&run_id);
    assert_eq!(
        (after.seat.as_str(), after.generation),
        (
            format!("{new_team}/{leader}").as_str(),
            before.generation + 1
        ),
        "the seat stayed on a pane that shows another conversation"
    );
    crate::agent_teams::forget_term(NEW);
    crate::agent_teams::forget_term(OLD);
}

/// The pane the seat names is not in this window's table at all — its team
/// went with no word to the ledger. The conversation bound to the run, back in
/// a pane here, takes the seat instead of reading its own inbox for hours.
#[test]
fn the_seat_follows_its_conversation_out_of_a_pane_this_window_does_not_hold() {
    const OLD: u32 = 57_744;
    const NEW: u32 = 57_745;
    let _window = the_window();
    let _turn = one_beat_at_a_time();
    let _young = BootedHere::at(clock());
    let host = Moving::new(&[]);
    let leader = zerocode_core::agent_teams::LEADER_PANE;
    let old_team = format!("team-seat-follows-gone-{OLD}");
    let run_id = a_run_sat_from(&host, &old_team, OLD, "seat-follows-gone");
    let before = seat_of(&run_id);

    // The table loses the old team and the ledger is never told.
    crate::agent_teams::forget_term(OLD);
    let new_team = format!("team-seat-follows-gone-new-{NEW}");
    seat_a_team(&new_team, NEW);
    host.show(NEW, OLD);
    super::super::tick(&host, &[], clock());

    let after = seat_of(&run_id);
    assert_eq!(
        (after.seat.as_str(), after.generation),
        (
            format!("{new_team}/{leader}").as_str(),
            before.generation + 1
        ),
        "the seat stayed on a pane this window does not hold"
    );
    crate::agent_teams::forget_term(NEW);
}

/// Another conversation holds the seat: a second agent bound to the run takes
/// nothing, even with the holder's pane closed to the background — replacing
/// somebody else's seat is the person's road (`run-takeover`), not the
/// window's.
#[test]
fn a_seat_another_conversation_holds_stays_with_it() {
    const HOLDER: u32 = 57_754;
    const OTHER: u32 = 57_755;
    let _window = the_window();
    let _turn = one_beat_at_a_time();
    let _young = BootedHere::at(clock());
    let host = Moving::new(&[]);
    let holder_team = format!("team-seat-follows-holder-{HOLDER}");
    let run_id = a_run_sat_from(&host, &holder_team, HOLDER, "seat-follows-holder");
    let before = seat_of(&run_id);

    let other_team = format!("team-seat-follows-other-{OTHER}");
    seat_a_team(&other_team, OTHER);
    host.show(OTHER, OTHER);
    let bound = from_leader(
        &host,
        &other_team,
        &format!("run-use {run_id} --retry-request t57705-other-binds"),
    );
    assert_eq!(bound.exit_code, 0, "{}", bound.stderr);
    host.detach(HOLDER);
    super::super::tick(&host, &[], clock());

    let after = seat_of(&run_id);
    assert_eq!(
        (after.seat.as_str(), after.generation),
        (before.seat.as_str(), before.generation),
        "the window took a seat another conversation holds"
    );
    crate::agent_teams::forget_term(OTHER);
    crate::agent_teams::forget_term(HOLDER);
}

/// A twin still open in a tab: the same conversation in two panes the person
/// can see. The seat stays where it is (t-21908), and the one line the window
/// writes names the pane that HOLDS the seat — it named the twin's own pane
/// until now, so the `run-takeover --from` it suggested pointed at the twin.
#[test]
fn a_twin_in_view_keeps_the_seat_and_the_line_names_its_holder() {
    const LEADER: u32 = 57_764;
    const TWIN: u32 = 57_765;
    let _window = the_window();
    let _turn = one_beat_at_a_time();
    let _young = BootedHere::at(clock());
    let host = Moving::new(&[]);
    let leader = zerocode_core::agent_teams::LEADER_PANE;
    let team = format!("team-seat-follows-held-{LEADER}");
    let run_id = a_run_sat_from(&host, &team, LEADER, "seat-follows-twin");
    let before = seat_of(&run_id);

    let twin_team = format!("team-seat-follows-twin-{TWIN}");
    seat_a_team(&twin_team, TWIN);
    host.show(TWIN, LEADER);
    super::super::tick(&host, &[], clock());
    super::super::tick(&host, &[], clock());

    let after = seat_of(&run_id);
    assert_eq!(
        (after.seat.as_str(), after.generation),
        (before.seat.as_str(), before.generation),
        "a twin still in view took the seat"
    );
    let told = logged(&run_id, &format!("terminal {TWIN}"));
    assert_eq!(told.len(), 1, "the twin was not told once: {told:?}");
    assert!(
        told[0].contains(&format!("{team}/{leader}"))
            && !told[0].contains(&format!("{twin_team}/{leader}")),
        "the line does not name the pane that holds the seat: {}",
        told[0]
    );
    crate::agent_teams::forget_term(TWIN);
    crate::agent_teams::forget_term(LEADER);
}

/// t-57705, the numbers: the 2026-10-16 07:29 scene in the fake window, read
/// the same way on any build. The coordinator's pane is closed with its agent
/// running and the conversation goes on in a new pane; then five beats of one
/// second each, and after the first of them the new pane tries two
/// `task-update`s — the two the field refused. It prints how many beats the
/// seat took to reach the new pane (`never` within five), how many of the two
/// updates were refused, and whether a person still has to type
/// `run-takeover`. It asserts no number: the numbers are the measurement,
/// before the fix and after it.
#[test]
fn t57705_measure_the_case_c_scene_counts_the_seatless_beats_and_the_refusals() {
    const OLD: u32 = 57_774;
    const NEW: u32 = 57_775;
    const BEATS: usize = 5;
    let _window = the_window();
    let _turn = one_beat_at_a_time();
    let _young = BootedHere::at(clock());
    let host = Moving::new(&[]);
    let leader = zerocode_core::agent_teams::LEADER_PANE;
    let old_team = format!("team-seat-follows-measure-{OLD}");
    let run_id = a_run_sat_from(&host, &old_team, OLD, "seat-follows-measure");
    let task = from_leader(&host, &old_team, "task-create --spec measure");
    let task: serde_json::Value = serde_json::from_str(&task.stdout).expect("a task");
    let task = task["taskId"].as_str().expect("a task id").to_string();

    host.detach(OLD);
    let new_team = format!("team-seat-follows-measure-new-{NEW}");
    seat_a_team(&new_team, NEW);
    host.show(NEW, OLD);
    let here = format!("{new_team}/{leader}");
    let mut seated_after = None;
    let mut refused = 0;
    for beat in 1..=BEATS {
        super::super::tick(&host, &[], clock());
        if seated_after.is_none() && seat_of(&run_id).seat == here {
            seated_after = Some(beat);
        }
        if beat == 1 {
            for _ in 0..2 {
                let updated = from_leader(
                    &host,
                    &new_team,
                    &format!("task-update --task {task} --status ready"),
                );
                if updated.exit_code != 0 {
                    refused += 1;
                }
            }
        }
    }
    let takeover_needed = usize::from(seat_of(&run_id).seat != here);
    println!(
        "t-57705 measure case C: seat reached the new pane after {} (one beat = 1 s); \
         task-update from the new pane refused {refused}/2; run-takeover a person must type \
         {takeover_needed}; typed into panes {}",
        seated_after.map_or_else(
            || format!("never in {BEATS} beats"),
            |beat| format!("{beat} beat(s)")
        ),
        host.typed()
    );
    crate::agent_teams::forget_term(NEW);
    crate::agent_teams::forget_term(OLD);
}
