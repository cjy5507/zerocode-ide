//! The panes that wait on the person (t-26595), and the inbox they stand in
//! (t-67483): which pane waits, and for what — a block, a question, or a
//! finish nobody has looked at.
//!
//! The decisions are `zerocode_core::notify::waiting_for`, and a finish is listed
//! only where the device's finish setting would ring it. What lives here is the
//! lock around the rows and the names a click needs. The locks are taken one at
//! a time, each only while its facts are copied: every row is read first, and
//! the names are read after. The inbox page reads these rows beside the
//! ledger's desk and counts them once (`orchestration::inbox`).
//!
//! Nothing here waits on the disk. The inbox is asked on the main thread after
//! every agent report, so the finish setting is the one this process last read
//! or wrote (`remembered_settings`, t-67102).

use zerocode_core::notify::{Look, Waiting, turn_ms, waiting_for};

use crate::*;

/// One pane that waits, as the inbox lists it.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct WaitingRow {
    /// The pane's id in the board's own form (`term:<n>`), which a click opens.
    pub(crate) pane: String,
    /// The agent's slug (`claude`, `codex`, …).
    pub(crate) agent: &'static str,
    /// The worktree the pane works in, as the ledger knows it.
    pub(crate) worktree: String,
    /// `blocked`, `question` or `finished`.
    pub(crate) waiting: &'static str,
    /// When the pane entered the state it waits in, in epoch milliseconds.
    pub(crate) since_ms: i64,
}

/// The rank a kind sorts at: a pane that cannot go on without the person first.
const fn rank(waiting: Waiting) -> u8 {
    match waiting {
        Waiting::Blocked => 0,
        Waiting::Question => 1,
        Waiting::Finished => 2,
    }
}

/// The word a row keeps for a kind.
const fn word(waiting: Waiting) -> &'static str {
    match waiting {
        Waiting::Blocked => "blocked",
        Waiting::Question => "question",
        Waiting::Finished => "finished",
    }
}

/// The inbox (t-67483): what waits on the person from every project, in one
/// list and one set of numbers — the ledger's desk and the panes together,
/// counted once (`orchestration::inbox::inbox_of`).
#[tauri::command]
pub(crate) fn board_inbox(state: State<'_, AppState>) -> crate::orchestration::inbox::Inbox {
    let _crumb = crate::crumbs::Command::enter("board_inbox");
    let panes = pane_waits(&state);
    crate::orchestration::inbox::inbox_of(
        &crate::orchestration::board_ledger_snapshot().desk,
        &panes,
    )
}

/// Every pane that waits on the person now. Blocks and questions come first,
/// then finishes; each kind lists its newest first.
pub(crate) fn pane_waits(state: &AppState) -> Vec<WaitingRow> {
    let mode = finish_mode(state.settings());
    let looks: Vec<(TermId, Look, i64)> = state
        .pane_states()
        .iter()
        // A worker's question its coordinator holds waits on the coordinator,
        // not on the person (t-82296).
        .filter(|(term, _)| !crate::worker_question::hidden(**term))
        .map(|(term, held)| {
            let look = Look {
                state: held.state,
                asking: held.ask.is_some() || held.ask_prompt.is_some(),
                mark: held.finish_mark && !held.interrupted && !held.session_boundary,
                turn_ms: turn_ms(held.turn_started_at, held.state_started_at),
            };
            (*term, look, held.state_started_at)
        })
        .collect();
    let mut rows: Vec<(Waiting, WaitingRow)> = looks
        .into_iter()
        .filter_map(|(term, look, since_ms)| {
            waiting_for(look, mode).map(|kind| (term, kind, since_ms))
        })
        .map(|(term, kind, since_ms)| {
            let agent = state.agent_terms().get(&term).copied().unwrap_or("");
            let seat = state.last_status_seats().get(&term).cloned();
            let worktree = seat
                .and_then(|seat| {
                    state
                        .last_statuses()
                        .get(&seat)
                        .map(|held| held.worktree.clone())
                })
                .unwrap_or_default();
            let row = WaitingRow {
                pane: format!("term:{term}"),
                agent,
                worktree,
                waiting: word(kind),
                since_ms,
            };
            (kind, row)
        })
        .collect();
    rows.sort_by_key(|(kind, row)| (rank(*kind), std::cmp::Reverse(row.since_ms)));
    rows.into_iter().map(|(_, row)| row).collect()
}

/// The device's finish setting, as the list reads it: from memory, so a
/// settings commit holding the repository through a slow fsync never holds
/// the main thread with it.
fn finish_mode(repository: &settings::SettingsRepository) -> zerocode_core::notify::FinishRing {
    remembered_settings(repository)
        .document
        .notifications
        .agent_completion
}

/// The person looked at a pane, so its finish mark is released. True when a
/// mark stood and was released; a pane id that names no terminal releases nothing.
#[tauri::command]
pub(crate) fn clear_finish_mark(state: State<'_, AppState>, pane: String) -> bool {
    let _crumb = crate::crumbs::Command::enter("clear_finish_mark");
    let Some(term) = pane
        .strip_prefix("term:")
        .and_then(|id| id.parse::<TermId>().ok())
    else {
        return false;
    };
    let mut states = state.pane_states();
    match states.get_mut(&term) {
        Some(held) if held.finish_mark => {
            held.finish_mark = false;
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// How long the list may spend on the finish setting: the main thread
    /// owes the next frame, and t-67102 bars a stall past 100 ms.
    const READ_BUDGET: Duration = Duration::from_millis(100);

    /// How long the stand-in commit holds the repository. The commit of
    /// 2026-10-21 05:32 held it 20 s; three tell a wait from none.
    const HELD_FOR: Duration = Duration::from_secs(3);

    /// 2026-10-21 05:32: a build saturated the disk, a settings commit held
    /// the repository through its fsync, and `board_inbox` — asked on the
    /// main thread after every agent report — read the finish setting
    /// behind it while the window stood for 20 255 ms. A commit of any
    /// settings document holds the one lock every settings read waits on;
    /// this one holds it on purpose while the list reads.
    #[test]
    fn the_finish_setting_is_read_without_waiting_on_a_settings_commit() {
        let directory = tempfile::tempdir().expect("settings sandbox");
        let repository = settings::SettingsRepository::new(directory.path());
        // Boot reads the window's settings before any command can be asked.
        let boot = load_settings_for_boot(&repository);
        let (held_tx, held_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        let committing = repository.clone();
        let writer = std::thread::spawn(move || {
            committing
                .mutate_json(
                    "slow-commit.json",
                    serde_json::Value::default,
                    |_: &mut serde_json::Value| {
                        held_tx.send(()).expect("the reader waits for the hold");
                        let _ = done_rx.recv_timeout(HELD_FOR);
                    },
                )
                .expect("the stand-in commit");
        });
        held_rx.recv().expect("the commit holds the repository");
        let started = Instant::now();
        let mode = finish_mode(&repository);
        let waited = started.elapsed();
        let _ = done_tx.send(());
        writer.join().expect("the stand-in commit ends");
        eprintln!(
            "finish_setting_read_ms={} commit_held_ms={}",
            waited.as_millis(),
            HELD_FOR.as_millis()
        );
        assert!(
            waited < READ_BUDGET,
            "the finish setting waited {} ms behind a settings commit",
            waited.as_millis()
        );
        assert_eq!(mode, boot.document.notifications.agent_completion);
    }

    /// Whatever road the list's read takes, it answers the last value saved:
    /// the read after a change of the finish setting answers the change.
    #[test]
    fn the_finish_setting_the_list_reads_is_the_last_one_saved() {
        use zerocode_core::notify::FinishRing;
        let directory = tempfile::tempdir().expect("settings sandbox");
        let repository = settings::SettingsRepository::new(directory.path());
        let boot = load_settings_for_boot(&repository);
        assert_eq!(
            finish_mode(&repository),
            boot.document.notifications.agent_completion
        );
        for mode in FinishRing::ALL {
            mutate_settings(&repository, |document| {
                document.notifications.agent_completion = mode;
                Ok(())
            })
            .expect("the finish setting is saved");
            assert_eq!(finish_mode(&repository), mode);
        }
    }

    /// The list's refresh beat, read off the inbox's own constant.
    #[cfg(unix)]
    fn list_beat() -> Duration {
        let ms = crate::ui_source::window_source()
            .split("const INBOX_REFRESH_MS = ")
            .nth(1)
            .and_then(|rest| rest.split(';').next())
            .and_then(|ms| ms.trim().parse().ok())
            .expect("the inbox's INBOX_REFRESH_MS");
        Duration::from_millis(ms)
    }

    /// The process's own CPU clock, user and system, in microseconds.
    #[cfg(unix)]
    fn cpu_us() -> u128 {
        // SAFETY: an all-zero rusage is a valid value for getrusage to fill.
        let mut used: libc::rusage = unsafe { std::mem::zeroed() };
        // SAFETY: getrusage writes only through the one pointer it is handed.
        unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut used) };
        let micros = |t: libc::timeval| t.tv_sec as u128 * 1_000_000 + t.tv_usec as u128;
        micros(used.ru_utime) + micros(used.ru_stime)
    }

    /// The share of a sorted run at `percent`.
    #[cfg(unix)]
    fn at(sorted: &[Duration], percent: usize) -> Duration {
        sorted[(sorted.len() * percent / 100).min(sorted.len() - 1)]
    }

    /// The list's read of the finish setting as the window makes it. First a
    /// minute of reads at the list's busiest, one per settled beat of agent
    /// reports, on a quiet disk: the cost of a read and the process CPU, and
    /// beside them the cost of a settings load. Then the same reads on the
    /// beat while writers saturate the disk and a settings commit fsyncs in
    /// a loop beside the list, the shape of 2026-10-21 05:32: the longest
    /// read is the stall the main thread would stand. Run it under
    /// `taskpolicy -b` for the low-spec profile. Everything is written in a
    /// private sandbox, at most `WRITERS * WRITER_FILE` bytes at once.
    // getrusage is the process's own CPU clock on unix; the Windows build has
    // none to read, and the cost this measures is the same code everywhere.
    #[cfg(unix)]
    #[test]
    #[ignore = "a measurement: saturates the disk; run on purpose through the build line"]
    fn measure_the_finish_setting_read_on_a_quiet_and_a_saturated_disk() {
        use std::io::{Seek, Write};
        use std::sync::atomic::{AtomicBool, Ordering};
        /// How long the disk stays saturated: the stall of 05:32 lasted 20 s.
        const SATURATED_FOR: Duration = Duration::from_secs(20);
        /// The writers that saturate the disk, and what each writes at once
        /// and in all before it starts over.
        const WRITERS: usize = 2;
        const BLOCK: usize = 16 << 20;
        const WRITER_FILE: u64 = 256 << 20;
        /// Settings loads timed on the quiet disk.
        const LOADS: usize = 100;

        let directory = tempfile::tempdir().expect("settings sandbox");
        let repository = settings::SettingsRepository::new(directory.path());
        load_settings_for_boot(&repository);
        let beat = list_beat();
        let per_minute = (Duration::from_secs(60).as_millis() / beat.as_millis()) as usize;

        let cpu_before = cpu_us();
        let mut quiet: Vec<Duration> = (0..per_minute)
            .map(|_| {
                let one = Instant::now();
                std::hint::black_box(finish_mode(&repository));
                one.elapsed()
            })
            .collect();
        let quiet_cpu_ms = (cpu_us() - cpu_before) / 1000;
        quiet.sort();
        let mut loads: Vec<Duration> = (0..LOADS)
            .map(|_| {
                let one = Instant::now();
                std::hint::black_box(load_settings_for_boot(&repository));
                one.elapsed()
            })
            .collect();
        loads.sort();

        let stop = Arc::new(AtomicBool::new(false));
        let writers: Vec<_> = (0..WRITERS)
            .map(|n| {
                let stop = Arc::clone(&stop);
                let path = directory.path().join(format!("saturate-{n}.bin"));
                std::thread::spawn(move || {
                    let block = vec![0xa5_u8; BLOCK];
                    let mut file = std::fs::File::create(&path).expect("a saturating file");
                    let mut written = 0;
                    while !stop.load(Ordering::Relaxed) {
                        if written >= WRITER_FILE {
                            file.rewind().expect("the file starts over");
                            written = 0;
                        }
                        file.write_all(&block).expect("a saturating block");
                        file.sync_all().expect("a saturating fsync");
                        written += BLOCK as u64;
                    }
                })
            })
            .collect();
        let committer = {
            let stop = Arc::clone(&stop);
            let repository = repository.clone();
            std::thread::spawn(move || {
                let mut commits = Vec::new();
                let mut dark = false;
                while !stop.load(Ordering::Relaxed) {
                    dark = !dark;
                    let one = Instant::now();
                    mutate_settings(&repository, |document| {
                        document.theme = if dark { "dark" } else { "light" }.into();
                        Ok(())
                    })
                    .expect("a settings commit");
                    commits.push(one.elapsed());
                }
                commits
            })
        };
        let mut saturated = Vec::new();
        let until = Instant::now() + SATURATED_FOR;
        while Instant::now() < until {
            let one = Instant::now();
            std::hint::black_box(finish_mode(&repository));
            saturated.push(one.elapsed());
            std::thread::sleep(beat);
        }
        stop.store(true, Ordering::Relaxed);
        for writer in writers {
            writer.join().expect("a writer ends");
        }
        let mut commits = committer.join().expect("the committer ends");
        saturated.sort();
        commits.sort();

        eprintln!(
            "quiet disk: {} reads (a minute at a {} ms beat) p50={}us p95={}us max={}us \
             cpu={quiet_cpu_ms}ms; settings load x{LOADS} p50={}us p95={}us",
            quiet.len(),
            beat.as_millis(),
            at(&quiet, 50).as_micros(),
            at(&quiet, 95).as_micros(),
            at(&quiet, 100).as_micros(),
            at(&loads, 50).as_micros(),
            at(&loads, 95).as_micros(),
        );
        eprintln!(
            "saturated disk {} s: {} reads p50={}us p95={}us max={}ms; \
             settings commits beside them: {} p50={}ms max={}ms",
            SATURATED_FOR.as_secs(),
            saturated.len(),
            at(&saturated, 50).as_micros(),
            at(&saturated, 95).as_micros(),
            at(&saturated, 100).as_millis(),
            commits.len(),
            at(&commits, 50).as_millis(),
            at(&commits, 100).as_millis(),
        );
    }
}
