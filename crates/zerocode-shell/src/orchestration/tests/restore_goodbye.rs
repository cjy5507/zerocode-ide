//! t-9091: a window's goodbye is the last thing it does to its workers.
//!
//! The beat keeps running for as long as the way out takes — a close took
//! 1.2 and 2.0 s on 2026-09-25 (19:04:36 → 19:04:37.6, 19:15:10 →
//! 19:15:12.5) — and that beat measured the grace from the CLOSING window's
//! boot, 72.2 and 10.4 minutes back, and ended every worker its goodbye had
//! just put to sleep: `sleeping worker … was not resumed within the grace`
//! 0.36 to 1.19 s after `… terminals go to sleep with the window`, then
//! `event loop exited`. The next windows booted with nobody to bring back.
//! The Claude workers of both restarts were `--retry-of … --inherit-checkout`
//! attempts, mid-turn, their conversations recorded and on disk; the one
//! worker that lived (Codex w-9055) was last in that loop when the process
//! ended.
//!
//! Driven through a private window's production doors, as `restore.rs` is:
//! the goodbye, the closing window's own beat, the next boot's sweep and its
//! coordinator coming back. A CLI counted here is one a real entry point cut.

use super::restore::{a_run, a_worker, census_without_commands, deaths_of, row, say};
use super::restore_door::{coordinator_back, the_next_boot};
use super::restore_seams::{WithTable, with_table};
use super::*;

/// The Claude launch the day's workers had.
const OPUS: &str = "--agent claude --model opus --effort xhigh";

/// A retry of `dispatch` in the checkout its first attempt left — the
/// `--retry-of … --inherit-checkout` the coordinator summoned after the
/// first loss — with the conversation it reported. Answers the worker, its
/// dispatch and the terminal its pane holds.
fn a_retry(
    host: &WithTable,
    team: &str,
    task: &str,
    dispatch: &str,
    session: &str,
) -> (String, String, u32) {
    let started: serde_json::Value = serde_json::from_str(&say(
        host,
        team,
        &format!("worker-start {OPUS} --task {task} --retry-of {dispatch} --inherit-checkout"),
    ))
    .expect("a retry");
    let worker = started["workerId"]
        .as_str()
        .expect("a worker id")
        .to_string();
    let term = host.inner.cuts().last().expect("the retry's pane").0;
    let held = super::super::runtime().expect("the private runtime");
    assert!(
        held.actor
            .worker_session_reported(
                term,
                zerocode_core::ProviderSession {
                    key: zerocode_core::SessionKey::SessionId,
                    id: session.to_string(),
                    transcript_path: None,
                },
                clock(),
            )
            .expect("the session lands")
            .0,
        "{worker}'s session was not written"
    );
    let dispatch = row(&worker).dispatch.expect("the retry's dispatch");
    (worker, dispatch, term)
}

/// The closing window's own beat, its boot `RESEAT_GRACE_MS` and more
/// behind it — twice, one second apart, as a two-second close beats.
fn the_closing_window_beats(host: &WithTable) {
    let _old = BootedHere::at(clock() - zerocode_core::orchestration::RESEAT_GRACE_MS - 1);
    super::super::tick(host, &[], clock());
    super::super::tick(host, &[], clock() + 1_000);
}

/// A pane of the closing window ends: its process with it, and the pane
/// table's word that it holds a conversation.
fn the_pane_goes(host: &WithTable, term: u32, session: &str) {
    super::super::terminal_gone(term, clock());
    crate::agent_teams::forget_term(term);
    host.standing.lock().unwrap().remove(session);
}

/// t-9091, the day's shape: an inherit-checkout Claude retry, mid-turn,
/// in a window up past the grace, and the window goes — by a close with
/// its panes still standing while it beats (19:04, 19:15), and by a quit
/// whose panes end before the embedded browser's stall, which the beat
/// outlives (the ⌘Q road, t-7812 D1). Either way the closing window ends
/// nobody and cuts nobody a pane; the next window's coordinator brings
/// the retry back once, as itself: the same worker, the same dispatch,
/// the same conversation, one CLI.
#[test]
fn a_closing_windows_beat_leaves_its_sleepers_to_the_next_window() {
    const OLD_LEADER: u32 = 290_910;
    const FIRST: u32 = 290_911;
    const NEW_LEADER: u32 = 290_930;
    for (at, shape) in ["close", "quit"].into_iter().enumerate() {
        let (_window, _store) = PrivateWindow::boot();
        let _beat = one_beat_at_a_time();
        let checkout = tempfile::tempdir().expect("the worker's checkout");
        let host = with_table(checkout.path(), FIRST, NEW_LEADER, OLD_LEADER);
        let (team, _run) = a_run(&host, OLD_LEADER, &format!("goodbye-{shape}"));
        // The first attempt ends — the day's first restart lost it — and
        // its retry inherits the checkout it left.
        let (task, first, first_term) = a_worker(
            &host.inner,
            &team,
            OPUS,
            Some(&format!("9091a0c1-0000-4000-8000-{at:012}")),
        );
        let first_dispatch = row(&first).dispatch.expect("the first dispatch");
        say(
            &host,
            &team,
            &format!("worker-stop --worker {first} --reason the-window-lost-it"),
        );
        crate::agent_teams::forget_term(first_term);
        let session = format!("9091b0c1-0000-4000-8000-{at:012}");
        let (worker, dispatch, term) = a_retry(&host, &team, &task, &first_dispatch, &session);
        super::super::pane_turn_began(term, clock());
        host.standing.lock().unwrap().insert(session.clone(), term);
        let failures = |rows: &zerocode_core::orchestration::LedgerProjectionV1| {
            rows.tasks
                .iter()
                .find(|held| held.id == task)
                .map(|held| held.failures)
        };
        let failures_before = failures(&the_rows());
        let cuts_before = host.inner.cuts().len();

        let road = if shape == "close" {
            crate::exit_runtime::ExitRoad::Close
        } else {
            crate::exit_runtime::ExitRoad::Terminate
        };
        super::super::window_exiting(clock(), road, &census_without_commands);
        assert_eq!(row(&worker).state, WorkerState::Sleeping, "{shape}");
        if shape == "quit" {
            the_pane_goes(&host, term, &session);
        }
        the_closing_window_beats(&host);
        assert_eq!(
            row(&worker).state,
            WorkerState::Sleeping,
            "{shape}: the closing window's beat moved the sleeper its goodbye made"
        );
        assert!(
            deaths_of(&worker).is_empty(),
            "{shape}: the closing window announced its own sleeper dead: {:?}",
            deaths_of(&worker)
        );
        assert_eq!(
            host.inner.cuts().len(),
            cuts_before,
            "{shape}: a CLI was cut in a window on its way out: {:?}",
            host.inner.cuts()
        );
        if shape == "close" {
            the_pane_goes(&host, term, &session);
        }

        // The next window: its boot, its coordinator back.
        the_next_boot(OLD_LEADER);
        let restored = coordinator_back(&host, &format!("goodbye-{shape}"), OLD_LEADER, NEW_LEADER);
        assert_eq!(restored, 1, "{shape}: the next window brought nobody back");
        let back = row(&worker);
        assert_eq!(back.state, WorkerState::Active, "{shape}");
        assert_eq!(
            back.dispatch.as_deref(),
            Some(dispatch.as_str()),
            "{shape}: a second attempt was minted"
        );
        assert_eq!(failures(&the_rows()), failures_before, "{shape}");
        assert!(deaths_of(&worker).is_empty(), "{shape}");
        let cuts = host.inner.cuts();
        let new: Vec<&(u32, String)> = cuts.iter().skip(cuts_before).collect();
        assert_eq!(new.len(), 1, "{shape}: one conversation, one CLI: {new:?}");
        for said in [
            "--resume",
            session.as_str(),
            "--model opus",
            "--effort xhigh",
            "--name",
        ] {
            assert!(
                new[0].1.contains(said),
                "{shape}: the pane came back without `{said}`: {}",
                new[0].1
            );
        }
        // And the next window's grace finds nobody left to end.
        {
            let _later =
                BootedHere::at(clock() - zerocode_core::orchestration::RESEAT_GRACE_MS - 1);
            super::super::tick(&host, &[], clock());
        }
        assert_eq!(row(&worker).state, WorkerState::Active, "{shape}");
        assert!(deaths_of(&worker).is_empty(), "{shape}");
        crate::agent_teams::forget_term(new[0].0);
        crate::agent_teams::forget_term(NEW_LEADER);
    }
}

/// t-9091 ③, the 19:15 restart as it stood: three workers mid-turn — two
/// Claude retries (w-9049, w-9052) and a Codex reviewer (w-9055) — in a
/// window up 10.4 minutes, closed with its panes standing, beating twice on
/// its way out. Counts the panes the next window brings back and times the
/// roads, printed before anything is asserted so a red run prints its own.
#[test]
fn the_three_workers_of_the_19_15_restart_come_back_as_themselves() {
    const OLD_LEADER: u32 = 290_940;
    const FIRST: u32 = 290_941;
    const NEW_LEADER: u32 = 290_960;
    let (_window, _store) = PrivateWindow::boot();
    let _beat = one_beat_at_a_time();
    let checkout = tempfile::tempdir().expect("the workers' checkout");
    let host = with_table(checkout.path(), FIRST, NEW_LEADER, OLD_LEADER);
    let (team, _run) = a_run(&host, OLD_LEADER, "goodbye-1915");
    let launches = [
        (OPUS, "9091c0c1-0000-4000-8000-000000009049"),
        (OPUS, "9091c0c1-0000-4000-8000-000000009052"),
        (
            "--agent codex --model gpt-6-astra --effort xhigh",
            "session-t9091-w9055",
        ),
    ];
    let mut workers = Vec::new();
    for (launch, session) in launches {
        let (_task, worker, term) = a_worker(&host.inner, &team, launch, Some(session));
        super::super::pane_turn_began(term, clock());
        host.standing
            .lock()
            .unwrap()
            .insert(session.to_string(), term);
        let dispatch = row(&worker).dispatch;
        workers.push((worker, term, session, dispatch));
    }
    let cuts_before = host.inner.cuts().len();

    let exit = std::time::Instant::now();
    super::super::window_exiting(
        clock(),
        crate::exit_runtime::ExitRoad::Close,
        &census_without_commands,
    );
    the_closing_window_beats(&host);
    let ended_on_the_way_out = workers
        .iter()
        .filter(|(worker, ..)| !deaths_of(worker).is_empty())
        .count();
    for (_, term, session, _) in &workers {
        the_pane_goes(&host, *term, session);
    }
    let way_out = exit.elapsed();
    let boot = std::time::Instant::now();
    the_next_boot(OLD_LEADER);
    let restored = coordinator_back(&host, "goodbye-1915", OLD_LEADER, NEW_LEADER);
    let back_in = boot.elapsed();
    let revived = workers
        .iter()
        .filter(|(worker, _, _, dispatch)| {
            let now = row(worker);
            now.state == WorkerState::Active && &now.dispatch == dispatch
        })
        .count();
    let cut_after = host.inner.cuts().len() - cuts_before;
    eprintln!(
        "t-9091 ③ 19:15 shape: workers {} · ended on the way out {ended_on_the_way_out} · \
         revived {revived} (reseat answered {restored}) · CLIs cut {cut_after} · way out \
         (goodbye + 2 beats + panes) {:.1} ms · next boot → all seated {:.1} ms",
        workers.len(),
        way_out.as_secs_f64() * 1_000.0,
        back_in.as_secs_f64() * 1_000.0,
    );
    assert_eq!(ended_on_the_way_out, 0);
    assert_eq!(revived, workers.len());
    assert_eq!(cut_after, workers.len(), "one CLI per conversation");
    for (term, _) in host.inner.cuts().iter().skip(cuts_before) {
        crate::agent_teams::forget_term(*term);
    }
    crate::agent_teams::forget_term(NEW_LEADER);
}
