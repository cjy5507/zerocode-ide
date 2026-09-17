use super::*;

/// The receipt clock and its log projection live in one small table. No call
/// site carries a second number that can drift from the timer it describes.
pub(super) const RESUME_NUDGE_RECEIPT_MS: u64 = 20_000;
const RESUME_LOG_SESSION_PREFIX_CHARS: usize = 8;
/// How much of a commit subject the worktree line carries — a line, not a
/// changelog.
const WORKTREE_SUBJECT_CHARS: usize = 72;

/// What a worker whose ledger seat came back with it is told beside the
/// restart nudge (t-3058). English for [`RESTART_NUDGE`]'s reason — it is
/// spoken to the agent — and it says the two things a restored worker most
/// often got wrong on its own: it re-oriented from scratch, and it re-ran
/// every gate it had already run.
pub(super) const RESEATED_NUDGE: &str = "Your ledger seat was restored with the same worker id, \
dispatch and task, so report through the same verbs as before. Continue from your last tool \
result; re-run only the gates for what changed after your last commit.";

/// One line of where the checkout stands, read at the wake so the resumed
/// agent does not spend its first turns re-discovering it (t-3058).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WorktreeState {
    pub(super) head: String,
    pub(super) subject: String,
    pub(super) uncommitted: usize,
    pub(super) last_commit_age_secs: u64,
}

impl WorktreeState {
    pub(super) fn line(&self) -> String {
        format!(
            "Worktree now: HEAD {} \"{}\" · {} uncommitted file(s) · last commit {}.",
            self.head,
            self.subject,
            self.uncommitted,
            age_words(self.last_commit_age_secs)
        )
    }
}

/// An age in the coarsest unit that still says something, for a line an
/// agent reads once.
fn age_words(secs: u64) -> String {
    match secs {
        0..=59 => "moments ago".to_string(),
        60..=3_599 => format!("{} min ago", secs / 60),
        3_600..=86_399 => format!("{} h ago", secs / 3_600),
        _ => format!("{} d ago", secs / 86_400),
    }
}

/// One git question through the host boundary — the window never spawns
/// `git` itself (source contract: the host boundary is never branched on
/// inline), so a checkout on a remote host answers the same way.
fn git_text(checkout: &Path, args: &[&str]) -> Option<String> {
    let host = zerocode_core::host::Host::for_workspace(checkout);
    host.vcs().text(checkout, args).ok()
}

/// Read the checkout's state through git — three questions, one line. A
/// directory git will not answer for (not a repository, no commit yet) is
/// `None`, and the nudge simply carries no line: the words must never say
/// something the checkout did not.
pub(super) fn worktree_state(checkout: &Path, now_secs: u64) -> Option<WorktreeState> {
    let head = git_text(checkout, &["rev-parse", "--short=7", "HEAD"])?;
    let head = head.trim().to_string();
    if head.is_empty() {
        return None;
    }
    let last = git_text(checkout, &["log", "-1", "--format=%s%x1f%ct"])?;
    let (subject, committed) = last.trim_end().split_once('\u{1f}')?;
    let committed: u64 = committed.trim().parse().ok()?;
    let subject: String = subject.chars().take(WORKTREE_SUBJECT_CHARS).collect();
    let status = git_text(
        checkout,
        &["status", "--porcelain", "--untracked-files=normal"],
    )?;
    Some(WorktreeState {
        head,
        subject,
        uncommitted: status
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count(),
        last_commit_age_secs: now_secs.saturating_sub(committed),
    })
}

/// The words a resumed pane is nudged with: the restart nudge, the checkout
/// line when git could give one, and — for a pane the ledger seated again
/// as its worker — the seat sentence. One line, so both delivery roads
/// (Claude's argv, Codex's composer paste) carry it the same.
pub(super) fn resume_nudge(reseated: bool, state: Option<&WorktreeState>) -> String {
    let mut words = RESTART_NUDGE.to_string();
    if let Some(state) = state {
        words.push(' ');
        words.push_str(&state.line());
    }
    if reseated {
        words.push(' ');
        words.push_str(RESEATED_NUDGE);
    }
    words
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PendingNudge {
    pub(super) agent: String,
    pub(super) session_id: String,
    pub(super) road: zerocode_core::NudgeRoad,
    /// The exact words this wake carries, so the composer fallback types
    /// what the argv road said and not a second, shorter nudge.
    pub(super) text: String,
    pub(super) started: Instant,
    /// When the one composer fallback was delivered, if it has been. The row
    /// is KEPT past that delivery so the replacement's own `working` hook can
    /// still close it — a fallback that lands is a `working` receipt, and a
    /// row removed at fallback time made that receipt land on nothing and the
    /// forensic line stand at `receipt=none` for a nudge the pane had taken.
    pub(super) fallback_at: Option<Instant>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Resolution {
    /// A `working` hook closed the wake — the nudge was taken. Carries the
    /// time from the resume to the hook, whichever delivery earned it.
    Working(PendingNudge, Duration),
    /// The first receipt window passed in silence: deliver the one composer
    /// fallback now. The row stays, waiting for this delivery's own receipt.
    FallbackDeliver(PendingNudge),
    /// A full window after the fallback and still no `working`: give up on
    /// this wake and file it `receipt=none`. The one fallback already went,
    /// so this resolution delivers nothing.
    GaveUp(PendingNudge),
}

#[derive(Debug, Default)]
pub(super) struct PendingNudges {
    rows: HashMap<TermId, PendingNudge>,
}

impl PendingNudges {
    pub(super) fn register(&mut self, term: TermId, pending: PendingNudge) {
        self.rows.insert(term, pending);
    }

    pub(super) fn working(&mut self, term: TermId, now: Instant) -> Option<Resolution> {
        let pending = self.rows.remove(&term)?;
        Some(Resolution::Working(
            pending.clone(),
            now.saturating_duration_since(pending.started),
        ))
    }

    /// One receipt window's verdict, read on the timer thread's beat.
    ///
    /// The first firing arms the single composer fallback and keeps the row;
    /// the second, a window after that fallback, gives up. A `working` hook
    /// arriving on either side of the fallback removes the row first, so this
    /// answers `None` and no line is filed twice.
    pub(super) fn timeout(&mut self, term: TermId, now: Instant) -> Option<Resolution> {
        let deadline = Duration::from_millis(RESUME_NUDGE_RECEIPT_MS);
        let pending = self.rows.get_mut(&term)?;
        match pending.fallback_at {
            None => {
                if now.saturating_duration_since(pending.started) < deadline {
                    return None;
                }
                pending.fallback_at = Some(now);
                Some(Resolution::FallbackDeliver(pending.clone()))
            }
            Some(fallback_at) => {
                if now.saturating_duration_since(fallback_at) < deadline {
                    return None;
                }
                self.rows.remove(&term).map(Resolution::GaveUp)
            }
        }
    }

    fn remove(&mut self, term: TermId) -> Option<PendingNudge> {
        self.rows.remove(&term)
    }
}

pub(super) fn log_line(
    term: TermId,
    agent: &str,
    session_id: &str,
    road: Option<zerocode_core::NudgeRoad>,
    receipt: Option<Duration>,
) -> String {
    let session: String = session_id
        .chars()
        .take(RESUME_LOG_SESSION_PREFIX_CHARS)
        .collect();
    let road = match road {
        Some(zerocode_core::NudgeRoad::Argv) => "argv",
        Some(zerocode_core::NudgeRoad::Composer) => "composer",
        None => "none",
    };
    let receipt = receipt.map_or_else(
        || "none".to_string(),
        |elapsed| format!("working@{}s", elapsed.as_secs()),
    );
    format!("term {term} resumed {agent} {session} nudge={road} receipt={receipt}")
}

/// The line for a wake that found nothing to re-enter: the record's
/// conversation was never written (`conversation_never_written`), so the pane
/// started the agent fresh instead of resuming into an exit.
pub(super) fn fresh_line(term: TermId, agent: &str, session_id: &str) -> String {
    let session: String = session_id
        .chars()
        .take(RESUME_LOG_SESSION_PREFIX_CHARS)
        .collect();
    format!("term {term} started {agent} fresh: {session} was never written")
}

fn note_resolution(state: &AppState, term: TermId, resolution: Resolution) {
    match resolution {
        Resolution::Working(pending, elapsed) => note_window_event(
            state.local_data_root(),
            &log_line(
                term,
                &pending.agent,
                &pending.session_id,
                Some(pending.road),
                Some(elapsed),
            ),
        ),
        // Deliver the one fallback and file nothing yet: its own `working`
        // hook is what the forensic line waits for. A row removed here — the
        // road this replaced — made a landed fallback's receipt land on
        // nothing and left the line reading `receipt=none` for a nudge the
        // pane had taken.
        Resolution::FallbackDeliver(pending) => {
            deliver_composer(state, term, &pending.agent, &pending.text, false);
        }
        // A full window after that fallback and still no receipt: the line is
        // filed `receipt=none`, and nothing is delivered a third time.
        Resolution::GaveUp(pending) => note_window_event(
            state.local_data_root(),
            &log_line(
                term,
                &pending.agent,
                &pending.session_id,
                Some(pending.road),
                None,
            ),
        ),
    }
}

fn deliver_composer(state: &AppState, term: TermId, agent: &str, text: &str, mounting: bool) {
    let readiness = if mounting {
        crate::cmd::terminal::PromptReadiness::Mounting
    } else {
        crate::cmd::terminal::PromptReadiness::Resting
    };
    let _ = crate::cmd::terminal::type_prompt_at_term(
        state,
        term,
        text.to_string(),
        true,
        Some(agent),
        readiness,
    );
}

/// The mark a restored pane wakes with: whether its conversation was cut
/// MID-TURN, so the wake continues it instead of opening an empty composer.
///
/// `stored` is the hook's word at the last persist (`WakeAgent::interrupted`,
/// written at the renderer's mid-turn edge). For a Codex pane it is the wrong
/// witness — Codex's hook says `idle` across a long tool call, so a worker cut
/// mid-`CommandExecution` woke unmarked and the nudge road returned before
/// typing (t-2874; rollout 01a07004, 2026-09-05). The rollout the record names
/// is the witness that outlives the process, and at wake time it is final, so
/// its word replaces the stored one BOTH ways: a turn it left open is
/// continued whatever the hook last said, and a turn it closed is not reopened
/// by a stale `working` mark. A record naming no file, or a file that is gone,
/// keeps the stored word. Every other agent keeps the hook-state rule as it is.
///
/// Read here rather than at persist time on purpose: Codex's `Stop` hook lands
/// 5 ms–3 s before `task_complete` reaches the file, and a pane at `idle` never
/// crosses the mid-turn edge again, so a persist-time reading is stale in both
/// directions (measured, docs/design/restart-nudge-delivery.md §4).
pub(super) fn wake_interrupted(agent: &str, stored: bool, transcript_path: Option<&str>) -> bool {
    // The witness is the row's: only an agent whose wake mark is its own
    // rollout file is read there; everyone else keeps the stored hook word.
    let reads_rollout = zerocode_core::agent_capabilities(agent).is_some_and(|caps| {
        caps.resume.wake_mark == zerocode_core::capabilities::WakeMark::Rollout
    });
    if !reads_rollout {
        return stored;
    }
    transcript_path
        .and_then(|path| zerocode_core::transcript::codex_turn_open(std::path::Path::new(path)))
        .unwrap_or(stored)
}

/// Register one restored pane and arm its one-shot receipt deadline.
/// Unmarked wakes are logged immediately; marked wakes resolve on the first
/// working hook or the one timeout thread below.
///
/// `nudge` is the exact text the wake carries ([`resume_nudge`]) — the argv
/// road already said it; the composer road types it here.
pub(super) fn register_wake(
    app: &AppHandle,
    term: TermId,
    agent: &str,
    session_id: &str,
    interrupted: bool,
    nudge: &str,
) {
    if !interrupted {
        note_window_event(
            app.state::<AppState>().local_data_root(),
            &log_line(term, agent, session_id, None, None),
        );
        return;
    }
    let Some(spec) = agent_spec(agent) else {
        note_window_event(
            app.state::<AppState>().local_data_root(),
            &log_line(term, agent, session_id, None, None),
        );
        return;
    };
    let pending = PendingNudge {
        agent: agent.to_string(),
        session_id: session_id.to_string(),
        road: spec.resume_nudge,
        text: nudge.to_string(),
        started: Instant::now(),
        fallback_at: None,
    };
    app.state::<AppState>()
        .pending_nudges()
        .register(term, pending);
    if spec.resume_nudge == zerocode_core::NudgeRoad::Composer {
        deliver_composer(&app.state::<AppState>(), term, agent, nudge, true);
    }
    let app = app.clone();
    std::thread::spawn(move || {
        // Two beats of one window. The first arms the single composer
        // fallback; the second gives up when even that never earned a
        // receipt. A `working` hook on either side removes the row, so a
        // beat that finds it gone files nothing and the line stands as the
        // hook already wrote it.
        for _ in 0..2 {
            std::thread::sleep(Duration::from_millis(RESUME_NUDGE_RECEIPT_MS));
            let resolution = app
                .state::<AppState>()
                .pending_nudges()
                .timeout(term, Instant::now());
            match resolution {
                Some(resolution) => note_resolution(&app.state::<AppState>(), term, resolution),
                None => break,
            }
        }
    });
}

/// The first working hook is the receipt for a marked wake.
pub(super) fn received_working(app: &AppHandle, term: TermId) {
    let resolution = app
        .state::<AppState>()
        .pending_nudges()
        .working(term, Instant::now());
    if let Some(resolution) = resolution {
        note_resolution(&app.state::<AppState>(), term, resolution);
    }
}

/// Resolve a pending wake when its terminal disappears before the timer.
pub(super) fn forgotten(state: &AppState, term: TermId) {
    let pending = state.pending_nudges().remove(term);
    if let Some(pending) = pending {
        note_window_event(
            state.local_data_root(),
            &log_line(
                term,
                &pending.agent,
                &pending.session_id,
                Some(pending.road),
                None,
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::thread;
    use zerocode_pty::DeliveryStep;

    const TEST_TERM_WITH_RECEIPT: TermId = 41;
    const TEST_TERM_WITHOUT_RECEIPT: TermId = 42;
    const TEST_ROWS: u16 = 24;
    const TEST_COLS: u16 = 80;
    const TEST_POLL_MS: u64 = 5;
    /// A ceiling on a fake Codex's whole resume — a shell script starting in a
    /// pty, its banner, the delivery's readiness wait, the paste and the
    /// Enter — never a measurement: what the tests assert is what reached the
    /// composer. Two seconds turned the v1.3.107 lane red (df529bf9,
    /// 2026-09-17, load 7) twice in the gate and once on the solo re-run, while
    /// the same run finishes in well under a second on a quiet machine.
    const TEST_DEADLINE_SECS: u64 = 10;

    fn pending(agent: &str, road: zerocode_core::NudgeRoad, started: Instant) -> PendingNudge {
        PendingNudge {
            agent: agent.to_string(),
            session_id: "01234567-session".to_string(),
            road,
            text: RESTART_NUDGE.to_string(),
            started,
            fallback_at: None,
        }
    }

    /// t-3058: the words a restored worker hears. The seat sentence names
    /// the two things a resumed worker got wrong by itself — re-orienting
    /// from scratch and re-running every gate — and rides only on a pane the
    /// ledger seated again; the checkout line rides on any resume that git
    /// could answer for. One line, whichever road delivers it.
    #[test]
    fn a_reseated_worker_is_told_its_seat_stands_and_where_the_checkout_is() {
        let state = WorktreeState {
            head: "1248312".to_string(),
            subject: "docs(product): base commit 78a67e5f".to_string(),
            uncommitted: 3,
            last_commit_age_secs: 12 * 60 + 7,
        };
        assert_eq!(
            state.line(),
            "Worktree now: HEAD 1248312 \"docs(product): base commit 78a67e5f\" · 3 uncommitted \
             file(s) · last commit 12 min ago."
        );
        let plain = resume_nudge(false, None);
        assert_eq!(plain, RESTART_NUDGE);
        let seated = resume_nudge(true, Some(&state));
        assert_eq!(
            seated,
            format!("{RESTART_NUDGE} {} {RESEATED_NUDGE}", state.line())
        );
        assert!(!seated.contains('\n'), "the nudge must stay one line");
        for words in [
            "seat was restored",
            "same worker id",
            "last tool result",
            "only the gates for what changed after your last commit",
        ] {
            assert!(RESEATED_NUDGE.contains(words), "{words}");
        }
        let unseated = resume_nudge(false, Some(&state));
        assert!(unseated.contains(&state.line()) && !unseated.contains(RESEATED_NUDGE));
        assert_eq!(age_words(30), "moments ago");
        assert_eq!(age_words(3 * 3_600 + 5), "3 h ago");
        assert_eq!(age_words(2 * 86_400 + 3_600), "2 d ago");
    }

    /// The checkout line is git's word, read at the wake: HEAD's short sha
    /// and subject, the files git would report as changed (untracked
    /// included — they are the ones that exist nowhere else), and the age
    /// of the last commit. A directory git cannot answer for gives no line.
    #[test]
    fn the_worktree_line_is_read_from_git_at_the_wake() {
        let repo = tempfile::tempdir().expect("a repository");
        let git = |args: &[&str]| {
            let done = crate::proc::quiet_command("git")
                .arg("-C")
                .arg(repo.path())
                .args(args)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
                .output()
                .expect("git runs");
            assert!(
                done.status.success(),
                "{:?}: {}",
                args,
                String::from_utf8_lossy(&done.stderr)
            );
            String::from_utf8_lossy(&done.stdout).trim().to_string()
        };
        assert_eq!(
            worktree_state(repo.path(), 0),
            None,
            "no repository, no line"
        );
        git(&["init", "-q"]);
        assert_eq!(worktree_state(repo.path(), 0), None, "no commit, no line");
        fs::write(repo.path().join("a.txt"), "a\n").expect("write");
        git(&["add", "a.txt"]);
        git(&[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "feat: the first line of work",
        ]);
        let head = git(&["rev-parse", "--short=7", "HEAD"]);
        let committed: u64 = git(&["log", "-1", "--format=%ct"]).parse().expect("a time");
        fs::write(repo.path().join("a.txt"), "b\n").expect("modify");
        fs::write(repo.path().join("new.txt"), "new\n").expect("untracked");
        let state = worktree_state(repo.path(), committed + 90).expect("a line");
        assert_eq!(state.head, head);
        assert_eq!(state.subject, "feat: the first line of work");
        assert_eq!(state.uncommitted, 2, "{state:?}");
        assert_eq!(state.last_commit_age_secs, 90);
        assert!(
            state
                .line()
                .starts_with(&format!("Worktree now: HEAD {head} \"feat: the first"))
        );
    }

    #[test]
    fn a_working_receipt_suppresses_fallback_and_a_timeout_falls_back_once() {
        let started = Instant::now();
        let deadline = Duration::from_millis(RESUME_NUDGE_RECEIPT_MS);
        let mut rows = PendingNudges::default();
        rows.register(
            TEST_TERM_WITH_RECEIPT,
            pending("codex", zerocode_core::NudgeRoad::Composer, started),
        );
        rows.register(
            TEST_TERM_WITHOUT_RECEIPT,
            pending("claude", zerocode_core::NudgeRoad::Argv, started),
        );

        assert!(matches!(
            rows.working(TEST_TERM_WITH_RECEIPT, started + Duration::from_secs(3)),
            Some(Resolution::Working(_, elapsed)) if elapsed == Duration::from_secs(3)
        ));
        assert!(
            rows.timeout(TEST_TERM_WITH_RECEIPT, started + deadline)
                .is_none()
        );
        assert!(
            rows.timeout(
                TEST_TERM_WITHOUT_RECEIPT,
                started + deadline - Duration::from_millis(1)
            )
            .is_none()
        );
        // The first window's silence arms the one fallback, and the row is
        // KEPT so the fallback's own receipt can still close it.
        assert!(matches!(
            rows.timeout(TEST_TERM_WITHOUT_RECEIPT, started + deadline),
            Some(Resolution::FallbackDeliver(_))
        ));
        // A second window of silence after that fallback gives up, once.
        assert!(matches!(
            rows.timeout(TEST_TERM_WITHOUT_RECEIPT, started + deadline + deadline),
            Some(Resolution::GaveUp(_))
        ));
        assert!(
            rows.timeout(
                TEST_TERM_WITHOUT_RECEIPT,
                started + deadline + deadline + deadline
            )
            .is_none()
        );
    }

    /// The bug this fix closes: a fallback that LANDS must still be recorded
    /// as a `working` receipt. The old road removed the row the moment it
    /// armed the fallback, so the replacement's own `working` hook resolved
    /// nothing and the forensic line stood at `receipt=none` for a nudge the
    /// pane had taken — exactly the `receipt=none` the t-2715 evidence shows.
    #[test]
    fn a_fallback_that_lands_is_recorded_as_a_working_receipt() {
        let started = Instant::now();
        let deadline = Duration::from_millis(RESUME_NUDGE_RECEIPT_MS);
        let mut rows = PendingNudges::default();
        rows.register(
            TEST_TERM_WITHOUT_RECEIPT,
            pending("codex", zerocode_core::NudgeRoad::Composer, started),
        );
        assert!(matches!(
            rows.timeout(TEST_TERM_WITHOUT_RECEIPT, started + deadline),
            Some(Resolution::FallbackDeliver(_))
        ));
        // The replacement submits and Codex reports working a second later.
        // That is the receipt, and it must be filed — from the resume, not
        // from the fallback — even though the fallback already fired.
        let receipt_at = started + deadline + Duration::from_secs(1);
        assert!(matches!(
            rows.working(TEST_TERM_WITHOUT_RECEIPT, receipt_at),
            Some(Resolution::Working(_, elapsed))
                if elapsed == deadline + Duration::from_secs(1)
        ));
        // Nothing is left for the second beat to give up on.
        assert!(
            rows.timeout(TEST_TERM_WITHOUT_RECEIPT, receipt_at + deadline)
                .is_none()
        );
    }

    /// What one hermetic fake-Codex resume left behind: the bytes its stdin
    /// received, the argv it was launched with, and whether the one delivery
    /// path ever reached its submitting Enter.
    #[cfg(unix)]
    struct FakeCodexRun {
        stdin: String,
        argv: String,
        submitted: bool,
    }

    /// Spawn a fake `codex` from a shell `script`, resume it through the real
    /// composer delivery, and report what it received. The script writes its
    /// argv to `$FAKE_CODEX_ARGV`, may print any startup banner it likes, and
    /// copies its stdin to `$FAKE_CODEX_STDIN`. Both resume tests drive the
    /// SAME delivery path — the one the shell uses — so a banner that broke
    /// readiness would fail here exactly as it would in the window.
    #[cfg(unix)]
    fn drive_fake_codex_resume(script: &str) -> FakeCodexRun {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().expect("temporary fake codex");
        let bin = root.path().join("bin");
        fs::create_dir(&bin).expect("fake bin");
        let program = bin.join("codex");
        let stdin_path = root.path().join("stdin");
        let argv_path = root.path().join("argv");
        fs::write(&program, script).expect("fake codex script");
        let mut permissions = fs::metadata(&program)
            .expect("script metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&program, permissions).expect("executable fake codex");
        let inherited = std::env::var_os("PATH").unwrap_or_default();
        let path = std::env::join_paths(
            std::iter::once(bin.as_path().to_path_buf()).chain(std::env::split_paths(&inherited)),
        )
        .expect("fake PATH");
        let env = vec![
            ("PATH".to_string(), path.to_string_lossy().into_owned()),
            (
                "FAKE_CODEX_STDIN".to_string(),
                stdin_path.to_string_lossy().into_owned(),
            ),
            (
                "FAKE_CODEX_ARGV".to_string(),
                argv_path.to_string_lossy().into_owned(),
            ),
        ];
        let session = zerocode_core::ProviderSession {
            key: zerocode_core::SessionKey::SessionId,
            id: "01234567-session".to_string(),
            transcript_path: None,
        };
        let argv = zerocode_core::resume_argv(AgentKind::Codex, &session).expect("resume argv");
        assert!(!argv.iter().any(|word| word == RESTART_NUDGE));
        let (command, args) = argv.split_first().expect("resume command");
        let mut pty = PtyLane::spawn(command, args, Some(root.path()), &env, TEST_ROWS, TEST_COLS)
            .expect("spawn fake codex from PATH");
        let started = Instant::now();
        let mut delivery = crate::cmd::terminal::prompt_delivery_for(
            RESTART_NUDGE.to_string(),
            true,
            Some("codex"),
            crate::cmd::terminal::PromptReadiness::Mounting,
            None,
            started,
        );
        let deadline = started + Duration::from_secs(TEST_DEADLINE_SECS);
        let mut submitted = false;
        loop {
            let pumped = pty.pump();
            let marker = delivery.marker();
            let seen = {
                let grid = pty.terminal_mut().grid_mut();
                let drawn = grid.take_glyph_drawn(marker);
                Observed {
                    wrote: pumped.bytes > 0,
                    bracketed_paste: grid.bracketed_paste(),
                    cursor_shows: grid.cursor_shows(),
                    marker_written: drawn.anywhere,
                    marker_in_alt: drawn.in_alt_screen,
                    alt_screen: grid.alt_screen(),
                }
            };
            match delivery.poll(seen, Instant::now()) {
                DeliveryStep::Waiting => {}
                DeliveryStep::Write(bytes) => {
                    pty.write_input(&bytes).expect("type nudge");
                }
                // Its own arm: the Enter is the thing the lost-Enter case is
                // about, so the driver records that it fired rather than
                // folding it into the paste write.
                DeliveryStep::Submit(bytes) => {
                    submitted = true;
                    pty.write_input(&bytes).expect("submit nudge");
                }
                DeliveryStep::Done(DeliveryOutcome::Delivered) => break,
                DeliveryStep::Done(outcome) => panic!("nudge delivery failed: {outcome:?}"),
            }
            assert!(
                Instant::now() < deadline,
                "fake Codex never received its nudge"
            );
            thread::sleep(Duration::from_millis(TEST_POLL_MS));
        }
        pty.kill().expect("stop fake codex");
        FakeCodexRun {
            stdin: fs::read_to_string(&stdin_path).expect("captured stdin"),
            argv: fs::read_to_string(&argv_path).expect("captured argv"),
            submitted,
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_fake_codex_resume_receives_the_nudge_on_stdin_and_yields_one_log_line() {
        let run = drive_fake_codex_resume(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$FAKE_CODEX_ARGV\"\nprintf '\\033[?2004h'\nsleep 0.05\nprintf '\\342\\200\\272'\ncat > \"$FAKE_CODEX_STDIN\"\n",
        );
        assert!(
            run.stdin.contains(RESTART_NUDGE),
            "stdin transcript: {:?}",
            run.stdin
        );
        assert!(
            !run.argv.contains(RESTART_NUDGE),
            "argv transcript: {:?}",
            run.argv
        );
        let line = log_line(
            TEST_TERM_WITH_RECEIPT,
            "codex",
            "01234567-session",
            Some(zerocode_core::NudgeRoad::Composer),
            Some(Duration::from_secs(1)),
        );
        assert_eq!(
            line,
            "term 41 resumed codex 01234567 nudge=composer receipt=working@1s"
        );
    }

    /// The lost-Enter case, reproduced hermetically. A resumed Codex enters
    /// its alternate screen and prints its interrupted line and two MCP
    /// startup warnings BEFORE it draws the composer glyph — the exact screen
    /// the t-2715 evidence shows, where the nudge sat unsubmitted under
    /// `MCP startup incomplete (failed: atlassian-rovo-mcp, supabase)`. The
    /// one delivery path must still find the composer and press Enter: both
    /// the paste and its submitting carriage return reach Codex.
    #[cfg(unix)]
    #[test]
    fn a_codex_resume_that_banners_mcp_warnings_before_its_prompt_still_submits() {
        let run = drive_fake_codex_resume(concat!(
            "#!/bin/sh\n",
            "printf '%s\\n' \"$@\" > \"$FAKE_CODEX_ARGV\"\n",
            "printf '\\033[?1049h'\n",
            "printf 'Conversation interrupted - tell the model what to do differently.\\n'\n",
            "printf '! The supabase MCP server requires OAuth reauthentication.\\n'\n",
            "printf '! MCP startup incomplete (failed: atlassian-rovo-mcp, supabase)\\n'\n",
            "printf '\\033[?2004h'\n",
            "sleep 0.10\n",
            "printf '\\342\\200\\272 '\n",
            "cat > \"$FAKE_CODEX_STDIN\"\n",
        ));
        assert!(
            run.stdin.contains(RESTART_NUDGE),
            "the nudge never reached the composer under the banner: {:?}",
            run.stdin
        );
        assert!(run.submitted, "the delivery never pressed Enter");
        // A submit terminator follows the paste envelope's close, so the
        // words were sent rather than left sitting unsubmitted — the exact
        // t-2715 symptom. The delivery writes a carriage return; this fake's
        // cooked line discipline shows it as a newline, while a real Codex
        // TUI in raw mode reads the `\r` verbatim, so either proves the Enter
        // reached the composer.
        assert!(
            run.stdin.contains("\u{1b}[201~\r") || run.stdin.contains("\u{1b}[201~\n"),
            "the paste closed but no Enter followed it: {:?}",
            run.stdin
        );
    }

    /// Codex rollout lines in the shapes the real files carry
    /// (`codex-runtime-home/home/sessions/**/rollout-*.jsonl`).
    const ROLLOUT_STARTED: &str = r#"{"timestamp":"2026-09-05T05:27:08.553Z","type":"event_msg","payload":{"type":"task_started","turn_id":"01a07008"}}"#;
    const ROLLOUT_ITEM: &str = r#"{"timestamp":"2026-09-05T05:31:37.338Z","type":"event_msg","payload":{"type":"item_completed","turn_id":"01a07008","item":{"type":"CommandExecution","status":"completed"}}}"#;
    const ROLLOUT_COMPLETE: &str = r#"{"timestamp":"2026-09-05T05:31:44.900Z","type":"event_msg","payload":{"type":"task_complete","turn_id":"01a07008"}}"#;

    /// The t-2874 root. The window persists a Codex pane's mark from the
    /// hook's word, and the hook said `idle` while the pane's rollout was
    /// mid-`CommandExecution` (rollout 01a07004, 2026-09-05) — so the wake
    /// came back unmarked and `register_wake` returned before typing. The
    /// rollout the record already names is the one witness that outlives the
    /// process: a Codex wake asks it, and its word replaces the hook's both
    /// ways. Claude's hook-state rule is untouched.
    #[test]
    fn a_codex_wake_takes_its_mark_from_the_rollout_the_record_names() {
        let dir = tempfile::tempdir().expect("tempdir");
        let open = dir.path().join("open.jsonl");
        fs::write(&open, format!("{ROLLOUT_STARTED}\n{ROLLOUT_ITEM}\n")).expect("write");
        let closed = dir.path().join("closed.jsonl");
        fs::write(
            &closed,
            format!("{ROLLOUT_STARTED}\n{ROLLOUT_ITEM}\n{ROLLOUT_COMPLETE}\n"),
        )
        .expect("write");
        let open = open.to_string_lossy().into_owned();
        let closed = closed.to_string_lossy().into_owned();
        let missing = dir
            .path()
            .join("missing.jsonl")
            .to_string_lossy()
            .into_owned();

        // The evidence: no mark from the hook, a turn still open in the file.
        assert!(
            wake_interrupted("codex", false, Some(&open)),
            "a Codex pane cut mid-turn woke unmarked — the nudge road returns before typing"
        );
        // The inverse: a turn the file closed is not reopened, not even by a
        // mark the hook's `working` edge left behind.
        assert!(!wake_interrupted("codex", false, Some(&closed)));
        assert!(!wake_interrupted("codex", true, Some(&closed)));
        // No file to ask: the record's own word stands, both ways.
        assert!(wake_interrupted("codex", true, None));
        assert!(!wake_interrupted("codex", false, None));
        assert!(!wake_interrupted("codex", false, Some(&missing)));
        assert!(wake_interrupted("codex", true, Some(&missing)));
        // Claude's rule is the hook's word, whatever a file beside it says.
        assert!(!wake_interrupted("claude", false, Some(&open)));
        assert!(wake_interrupted("claude", true, Some(&closed)));
    }
}
