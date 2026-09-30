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

/// What a resumed worker is told about the commands the restart cut under
/// its pane (t-6428 ⑤) — the goodbye read them there as the window went and
/// summarized each in a few words. English, like the rest of the nudge,
/// because it is spoken to the agent; it reads the same after the turn
/// sentence and alone, for a turn that had ended with a gate still running.
pub(super) const CUT_COMMANDS_NUDGE: &str =
    "Commands still running under you when the window restarted were cut:";
/// What the worker does about them: its own call, command by command.
pub(super) const CUT_COMMANDS_TAIL: &str = "— run again whichever you still need.";

/// The sentence naming the cut commands, when there were any.
fn cut_line(cut: &[String]) -> Option<String> {
    (!cut.is_empty()).then(|| {
        let named: Vec<String> = cut.iter().map(|command| format!("`{command}`")).collect();
        format!(
            "{CUT_COMMANDS_NUDGE} {} {CUT_COMMANDS_TAIL}",
            named.join("; ")
        )
    })
}

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

/// The words a resumed pane is nudged with: the restart nudge when the turn
/// was cut, the commands the restart cut under the pane when there were any
/// (t-6428 ⑤), the checkout line when git could give one, and — for a pane
/// the ledger seated again as its worker — the seat sentence. One line, so
/// both delivery roads (Claude's argv, Codex's composer paste) carry it the
/// same.
pub(super) fn resume_nudge(
    turn_cut: bool,
    reseated: bool,
    state: Option<&WorktreeState>,
    cut: &[String],
) -> String {
    let said = cut_line(cut);
    let mut parts = Vec::new();
    if turn_cut || said.is_none() {
        parts.push(RESTART_NUDGE.to_string());
    }
    parts.extend(said);
    if let Some(state) = state {
        parts.push(state.line());
    }
    if reseated {
        parts.push(RESEATED_NUDGE.to_string());
    }
    parts.join(" ")
}

/// The words a restored worker's wake carries, or none (t-7812 E) — one
/// answer for both roads that bring a worker back: the ledger's reseat and
/// the window's resumed pane. A worker an account switch rested hears the
/// switch's words instead (t-7538), whichever road brings it back.
///
/// Only what the goodbye read as cut ([`restart_census::peek_cut`]): a turn
/// under way, or commands running under the pane (t-6428 ⑤). A worker at
/// rest with nothing cut is idle, and an idle worker is not told to go on —
/// its last turn ended, and a continue would put the next one in its mouth.
/// A worker that was waiting on a question for the person is not either: the
/// question was the person's to answer. A wake with no goodbye to read — a
/// crash's — hears nothing: a line typed on a guess is the blind re-send a
/// continuation must never be. A person's own tab never asks here at all;
/// the window restarting is not the person asking for more.
///
/// Read, not spent (t-7812 R2): the goodbye's word about a worker is spent
/// by the wake that hands these words to a pane holding it ([`nudge_spent`]),
/// so a wake that starts nothing — its seat refused, its spawn refused —
/// leaves them for the road that tries next.
///
/// [`restart_census::peek_cut`]: crate::orchestration::restart_census::peek_cut
pub(super) fn worker_nudge(root: &Path, worker: &str, checkout: Option<&Path>) -> Option<String> {
    let cut = crate::orchestration::restart_census::peek_cut(root, worker);
    // A worker an account switch rested is told the switch's own words and
    // not the restart's (t-7538): the same note, spent the same way.
    if let Some(words) = cut.switched {
        return Some(words);
    }
    if !cut.any() {
        return None;
    }
    let state = checkout_state(checkout);
    Some(resume_nudge(cut.turn, true, state.as_ref(), &cut.commands))
}

/// Where the checkout stands as a wake reads it, when it has one.
fn checkout_state(checkout: Option<&Path>) -> Option<WorktreeState> {
    checkout.and_then(|checkout| {
        worktree_state(
            checkout,
            u64::try_from(now_epoch_ms() / 1_000).unwrap_or_default(),
        )
    })
}

/// The words a person's resumed tab carries, or none (t-11537 C) — a
/// worker's continuation ([`resume_nudge`]) without the seat sentence, for
/// the one case the person did not choose: the goodbye read the tab's turn
/// as under way, so the restart cut it. A tab at rest, or one their own hand
/// stopped, left no entry and comes back as it stood. Read, not spent, like
/// a worker's: the words are spent once they may have reached the pane.
pub(super) fn tab_nudge(root: &Path, key: &str, checkout: Option<&Path>) -> Option<String> {
    let cut = crate::orchestration::restart_census::peek_cut(root, key);
    if !cut.turn {
        return None;
    }
    let state = checkout_state(checkout);
    Some(resume_nudge(true, false, state.as_ref(), &cut.commands))
}

/// What a coordinator whose run was still working is told on its way back
/// up when the window did not cut its own turn (t-11537) — beside
/// [`RESTART_NUDGE`], which says it for a turn the restart did cut.
pub(super) const COORDINATOR_NUDGE: &str = "The window restarted while your run was still working.";

/// What the restart took from a coordinator that no ledger row holds: the
/// timers of its own session. A check it scheduled to come back to its mail
/// lived in the CLI's memory and ended with the process.
pub(super) const COORDINATOR_TIMERS_NUDGE: &str = "Any periodic check you had scheduled in this \
session ended with the restart; set it again if you still need it, then carry on.";

/// The words a resumed coordinator's wake carries, or none (t-11537) — one
/// line, built like a worker's ([`resume_nudge`]) and placed by the same
/// road ([`place_words`]).
///
/// Only a run that was working: the coordinator's own turn cut, a worker
/// still carrying a dispatch, or a task dispatched. A coordinator whose
/// run stood idle is not told to go on — its last turn ended, and the
/// window restarting is not the person asking for more. Nor one whose last
/// turn a person's hand ended: that pane is theirs. The counts are the
/// ledger's, and the mail is named the way the pointer names it.
pub(super) fn coordinator_nudge(
    standing: &crate::orchestration::CoordinatorStanding,
) -> Option<String> {
    let cut = &standing.cut;
    if cut.ended.is_some_and(|ended| ended.interrupted) {
        return None;
    }
    if !cut.turn && standing.workers == 0 && standing.dispatched == 0 {
        return None;
    }
    let mut parts = vec![
        if cut.turn {
            RESTART_NUDGE
        } else {
            COORDINATOR_NUDGE
        }
        .to_string(),
        format!(
            "{} has {} worker(s) carrying a dispatch and {} task(s) dispatched.",
            standing.address, standing.workers, standing.dispatched
        ),
    ];
    if standing.unread > 0 {
        parts.push(
            zerocode_core::orchestration::pointer_text(standing.unread)
                .trim()
                .to_string(),
        );
    }
    parts.push(COORDINATOR_TIMERS_NUDGE.to_string());
    Some(parts.join(" "))
}

/// `worker`'s words were handed to a pane that holds it (t-7812 R2): they
/// reached it, or may have — and words that may have landed are never said
/// a second time. The goodbye's word about that worker is spent.
pub(super) fn nudge_spent(root: &Path, worker: &str) {
    crate::orchestration::restart_census::spend_cut(root, worker);
}

/// The goodbye's word a wake's words came from: the data root its note lives
/// in and the worker it was about — or, for a coordinator's wake, its run's
/// address (t-11537) — so the words are spent exactly when they may have
/// reached the pane (t-7812 R2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Owed {
    pub(super) root: PathBuf,
    pub(super) worker: String,
}

/// What a wake's delivery answered about its words (t-7812 R2). The one
/// thing that decides whether they may be typed again: only words that never
/// left, at a composer that was not ready, are still the wake's to place —
/// and only words left on the line with their Enter untaken are still the
/// wake's to press Enter for (t-17037).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Said {
    /// The words reached the child — its argv, a paste its composer took
    /// with its Enter, or one a person's hand, a parked question or a
    /// relaunch stopped short of its Enter: those are on the line for the
    /// person. Never typed again, whatever the hooks say after.
    Reached,
    /// The words are on the line and their Enter was not taken, with nobody
    /// else's hand on it (t-17037): the pane never reported taking them, or
    /// the terminal refused the Enter's write. Never typed again — the one
    /// fallback is an Enter alone, at the composer's next ready. Without it
    /// the words sat in the box after a restart until a person pressed
    /// Enter (2026-09-30, twice).
    Unsent,
    /// Nothing was sent: the composer never said it was ready. The line is
    /// still the wake's, and the one fallback may place the words there.
    NotReady,
    /// Nothing was sent, and the line was not the wake's to write on: a
    /// person's draft or hand, a parked question, another launch. Nothing is
    /// typed there again.
    Withheld,
}

impl Said {
    pub(crate) const fn of(outcome: DeliveryOutcome) -> Self {
        use zerocode_pty::ready::Refusal;
        match outcome {
            DeliveryOutcome::Unsubmitted(Refusal::NotTaken | Refusal::InputRejected) => {
                Self::Unsent
            }
            DeliveryOutcome::Delivered | DeliveryOutcome::Unsubmitted(_) => Self::Reached,
            DeliveryOutcome::TimedOut => Self::NotReady,
            DeliveryOutcome::Refused(_) => Self::Withheld,
        }
    }
}

/// Which one fallback a wake spent (t-17037) — in the log, so a transcript
/// tells an Enter pressed again from words typed again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Fallback {
    /// The words, typed at a composer that was not ready the first time.
    Typed,
    /// Enter alone, for words left on the line with their Enter untaken.
    Submitted,
}

impl Fallback {
    const fn word(self) -> &'static str {
        match self {
            Self::Typed => "typed",
            Self::Submitted => "submitted",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PendingNudge {
    pub(super) agent: String,
    pub(super) session_id: String,
    pub(super) road: zerocode_core::NudgeRoad,
    /// The exact words this wake carries, so the composer fallback types
    /// what the first delivery meant to say and not a second, shorter nudge.
    pub(super) text: String,
    pub(super) started: Instant,
    /// When the first receipt window closed, if it has. The row is KEPT past
    /// it — and past the one fallback, when that went — so the pane's own
    /// `working` hook can still close it: a row removed at the first beat
    /// made that receipt land on nothing and the forensic line stand at
    /// `receipt=none` for a nudge the pane had taken.
    pub(super) first_window_closed: Option<Instant>,
    /// What the delivery answered, once it has (t-7812 R2). An argv's words
    /// are [`Said::Reached`] from the start: they rode the launch.
    pub(super) said: Option<Said>,
    /// The launch these words were for — the pane's fingerprint when the wake
    /// armed — so a fallback never types at a program relaunched since.
    pub(super) launch: Option<u64>,
    /// The line's hand count read before the words were placed (t-17037): an
    /// Enter pressed again goes only on a line no hand has reached since.
    pub(super) hand: Option<u64>,
    /// The one fallback, once it went — typed or submitted, never both, and
    /// never twice.
    pub(super) fallback: Option<Fallback>,
    /// The goodbye's word to spend once the words may have reached the pane;
    /// `None` once spent, or for words that owe nobody.
    pub(super) owed: Option<Owed>,
}

impl PendingNudge {
    /// The goodbye's word, spent: the words reached the pane, or may have.
    fn spend(&mut self) {
        if let Some(owed) = self.owed.take() {
            nudge_spent(&owed.root, &owed.worker);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Resolution {
    /// A `working` hook closed the wake — the nudge was taken. Carries the
    /// time from the resume to the hook, whichever delivery earned it.
    Working(PendingNudge, Duration),
    /// The first receipt window closed with the words never sent, at a
    /// composer that was not ready: deliver the one fallback now. The row
    /// stays, waiting for this delivery's own answer and receipt.
    FallbackDeliver(PendingNudge),
    /// The delivery answered that the words are on the line and their Enter
    /// was not taken (t-17037): press Enter alone, once, when the composer
    /// next says it is ready. The row stays, for that Enter's own answer and
    /// the pane's `working` hook.
    FallbackSubmit(PendingNudge),
    /// A full window after the first closed and still no `working`: give up
    /// on this wake and file it `receipt=none`. Nothing is delivered — the
    /// hook's silence is not a sign the words were lost (t-7812 R2).
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

    /// Whether a wake is still waiting on `term`.
    pub(super) fn holds(&self, term: TermId) -> bool {
        self.rows.contains_key(&term)
    }

    pub(super) fn working(&mut self, term: TermId, now: Instant) -> Option<Resolution> {
        let mut pending = self.rows.remove(&term)?;
        // The pane took input: whatever its delivery has said so far, the
        // words are not the wake's to say again.
        pending.spend();
        Some(Resolution::Working(
            pending.clone(),
            now.saturating_duration_since(pending.started),
        ))
    }

    /// What the delivery answered about the words (t-7812 R2). Words that
    /// reached the pane spend the goodbye's word the moment that is known.
    ///
    /// Words left on the line with their Enter untaken arm the one fallback
    /// on the spot (t-17037): an Enter alone, which itself waits for the
    /// composer to say it is ready. The fallback gets a full receipt window
    /// of its own from `now` when the first has already closed. A wake whose
    /// one fallback already went — typed or submitted — arms nothing more.
    pub(super) fn heard(
        &mut self,
        term: TermId,
        outcome: DeliveryOutcome,
        now: Instant,
    ) -> Option<Resolution> {
        let pending = self.rows.get_mut(&term)?;
        let said = Said::of(outcome);
        pending.said = Some(said);
        if matches!(said, Said::Reached | Said::Unsent) {
            pending.spend();
        }
        if said != Said::Unsent || pending.fallback.is_some() {
            return None;
        }
        pending.fallback = Some(Fallback::Submitted);
        if pending.first_window_closed.is_some() {
            pending.first_window_closed = Some(now);
        }
        Some(Resolution::FallbackSubmit(pending.clone()))
    }

    /// One receipt window's verdict, read on the timer's beat; `window` is
    /// the product's [`RESUME_NUDGE_RECEIPT_MS`] everywhere but a test's
    /// short clock.
    ///
    /// The first window's close arms the one composer fallback — only for
    /// words that never left a composer that was not ready — and keeps the
    /// row either way; the close of the window after it gives up. A hook's
    /// silence alone never types anything (t-7812 R2): words that reached
    /// the pane, or may have, whose `working` report was late or never came,
    /// are not said twice. A `working` hook on either side of a beat removes
    /// the row first, so this answers `None` and no line is filed twice.
    pub(super) fn timeout(
        &mut self,
        term: TermId,
        now: Instant,
        window: Duration,
    ) -> Option<Resolution> {
        let deadline = window;
        let pending = self.rows.get_mut(&term)?;
        match pending.first_window_closed {
            None => {
                if now.saturating_duration_since(pending.started) < deadline {
                    return None;
                }
                pending.first_window_closed = Some(now);
                if pending.said != Some(Said::NotReady) || pending.fallback.is_some() {
                    return None;
                }
                pending.fallback = Some(Fallback::Typed);
                Some(Resolution::FallbackDeliver(pending.clone()))
            }
            Some(closed) => {
                if now.saturating_duration_since(closed) < deadline {
                    return None;
                }
                let mut pending = self.rows.remove(&term)?;
                pending.give_up();
                Some(Resolution::GaveUp(pending))
            }
        }
    }

    /// The one fallback was placed at the composer: until its own answer
    /// comes, nobody can say the words did not land (t-7812 R2).
    pub(super) fn placed_again(&mut self, term: TermId) {
        if let Some(pending) = self.rows.get_mut(&term) {
            pending.said = None;
        }
    }

    fn remove(&mut self, term: TermId) -> Option<PendingNudge> {
        let mut pending = self.rows.remove(&term)?;
        pending.give_up();
        Some(pending)
    }
}

impl PendingNudge {
    /// The wake ends unanswered. Words nobody could say were sent — a
    /// delivery that never answered — may have landed, so the goodbye's word
    /// is spent; words known never to have left keep it (t-7812 R2).
    fn give_up(&mut self) {
        if !matches!(self.said, Some(Said::NotReady | Said::Withheld)) {
            self.spend();
        }
    }
}

pub(super) fn log_line(
    term: TermId,
    agent: &str,
    session_id: &str,
    road: Option<zerocode_core::NudgeRoad>,
    receipt: Option<Duration>,
) -> String {
    let receipt = receipt.map_or_else(
        || "none".to_string(),
        |elapsed| format!("working@{}s", elapsed.as_secs()),
    );
    format!(
        "{} receipt={receipt}",
        resumed_head(term, agent, session_id, road)
    )
}

/// What every wake's line opens with: the pane, the agent, the session's
/// first characters and the road its words took.
fn resumed_head(
    term: TermId,
    agent: &str,
    session_id: &str,
    road: Option<zerocode_core::NudgeRoad>,
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
    format!("term {term} resumed {agent} {session} nudge={road}")
}

/// The line a wake's one fallback leaves as it goes (t-17037): `submitted
/// again` for an Enter alone, `typed again` for the words at a composer that
/// was not ready — the one word a transcript tells them apart by.
pub(super) fn fallback_line(term: TermId, pending: &PendingNudge, fallback: Fallback) -> String {
    format!(
        "{} {} again",
        resumed_head(
            term,
            &pending.agent,
            &pending.session_id,
            Some(pending.road)
        ),
        fallback.word()
    )
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

/// One worker wake's words, as the wake hands them over (t-7812 R2).
pub(super) struct Words<'a> {
    pub(super) agent: &'a str,
    pub(super) session_id: &'a str,
    /// The road the agent's row says a resume nudge takes.
    pub(super) road: zerocode_core::NudgeRoad,
    pub(super) text: String,
    /// The launch the pane holds as the words are placed.
    pub(super) launch: Option<u64>,
    /// The line's hand count as the words are placed (t-17037).
    pub(super) hand: Option<u64>,
    pub(super) owed: Owed,
}

/// Place a worker wake's words — the one delivery a wake makes on its own —
/// and build the row its receipt waits in (t-7812 R2).
///
/// Words on the argv road rode the launch itself (`resume_argv_continuing`
/// put them there, and the pane started only after its seat was durable):
/// they reached the pane, and the goodbye's word is spent now. Words for a
/// composer are typed at it while it mounts; `typed` answers that delivery's
/// own answer, which decides the rest in [`watch`]. A composer that could
/// not be asked at all — its terminal already gone — took nothing.
pub(super) fn place_words(
    typed: impl FnOnce(&str) -> Option<std::sync::mpsc::Receiver<DeliveryOutcome>>,
    words: Words<'_>,
) -> (
    PendingNudge,
    Option<std::sync::mpsc::Receiver<DeliveryOutcome>>,
) {
    let (said, delivery) = match words.road {
        zerocode_core::NudgeRoad::Argv => (Some(Said::Reached), None),
        zerocode_core::NudgeRoad::Composer => match typed(&words.text) {
            Some(delivery) => (None, Some(delivery)),
            None => (Some(Said::Withheld), None),
        },
    };
    let mut pending = PendingNudge {
        agent: words.agent.to_string(),
        session_id: words.session_id.to_string(),
        road: words.road,
        text: words.text,
        started: Instant::now(),
        first_window_closed: None,
        said,
        launch: words.launch,
        hand: words.hand,
        fallback: None,
        owed: Some(words.owed),
    };
    if said == Some(Said::Reached) {
        pending.spend();
    }
    (pending, delivery)
}

/// The line a restored worker's continuation files once its pane reported
/// taking it (t-18353): the same `receipt=working@Ns` the wake road files, so
/// the person can read at the next restart that every restored pane took its
/// words — until now only a continuation left unsent left a line at all.
/// Only for a pane whose row reports the prompts it takes (its delivery is
/// `Delivered` on that report, not on the Enter's write); any other pane's
/// delivery says nothing about the words being taken, and files no line.
pub(crate) fn continuation_taken_line(
    term: TermId,
    agent: &str,
    session_id: &str,
    elapsed: Duration,
) -> Option<String> {
    zerocode_core::agent_capabilities(agent)
        .is_some_and(|caps| caps.submit_ack != zerocode_core::capabilities::SubmitAck::None)
        .then(|| {
            log_line(
                term,
                agent,
                session_id,
                Some(zerocode_core::NudgeRoad::Composer),
                Some(elapsed),
            )
        })
}

/// File [`continuation_taken_line`] in the window's log.
pub(crate) fn note_continuation_taken(
    state: &AppState,
    term: TermId,
    agent: &str,
    session_id: &str,
    elapsed: Duration,
) {
    if let Some(line) = continuation_taken_line(term, agent, session_id, elapsed) {
        note_window_event(state.local_data_root(), &line);
    }
}

/// The line for a sleeper's wake that started nothing because the ledger
/// would not seat it (t-7812 R1): the pane would have been a conversation
/// the ledger does not know, which is the fault the witness exists to close.
pub(super) fn unseated_line(term: TermId, agent: &str, worker: &str, why: &str) -> String {
    format!(
        "term {term} did not resume {agent}: sleeping worker {worker} could not be seated ({why})"
    )
}

/// The line for a wake that started nothing because the conversation's last
/// program, closed by an account switch, is not seen gone yet (t-7538, astra
/// R3): a second process beside it would be two writers on one transcript.
pub(super) fn held_line(term: TermId, agent: &str, worker: &str) -> String {
    format!(
        "term {term} did not resume {agent}: worker {worker}'s last pane's program has not been \
         seen to leave, so its conversation is not opened beside it"
    )
}

/// The line a door leaves when it cannot read whether a conversation is
/// held (t-7538, astra R3-1): the ledger is on disk and does not answer, so
/// nothing is opened on a guess.
pub(super) fn unread_hold_line(term: TermId, agent: &str, why: &str) -> String {
    format!(
        "term {term} did not resume {agent}: the ledger could not be read to see whether a \
         program a switch closed still holds this conversation ({why}); it opens once the \
         ledger reads again"
    )
}

/// What a wake's receipt watch needs from the window it waits in (t-7812):
/// the table its row sits in, a composer to place the one fallback at, and
/// the log. The product's is the window's own state; a test's is a fake
/// window, which runs the very same watch on a short clock.
pub(crate) trait WakeReceipts {
    /// The table of wakes waiting on a receipt.
    fn rows(&self) -> std::sync::MutexGuard<'_, PendingNudges>;
    /// The launch the pane at `term` holds now.
    fn launch(&self, term: TermId) -> Option<u64>;
    /// Place the one fallback at a composer at rest, beside whatever a person
    /// left on its line. Answers the delivery's own answer, when it can.
    fn type_again(
        &self,
        term: TermId,
        agent: &str,
        text: &str,
    ) -> Option<std::sync::mpsc::Receiver<DeliveryOutcome>>;
    /// Press Enter alone at `term` once its composer next says it is ready,
    /// on a line no hand has reached since `hand` (t-17037). Answers the
    /// delivery's own answer, when it can.
    fn submit_again(
        &self,
        term: TermId,
        agent: &str,
        hand: Option<u64>,
    ) -> Option<std::sync::mpsc::Receiver<DeliveryOutcome>>;
    /// One line in the window's log.
    fn note(&self, line: &str);
}

fn note_resolution(
    window: &dyn WakeReceipts,
    term: TermId,
    resolution: Resolution,
) -> Option<std::sync::mpsc::Receiver<DeliveryOutcome>> {
    match resolution {
        Resolution::Working(pending, elapsed) => {
            window.note(&log_line(
                term,
                &pending.agent,
                &pending.session_id,
                Some(pending.road),
                Some(elapsed),
            ));
            None
        }
        // Deliver the one fallback and file nothing yet: its own answer and
        // its own `working` hook are what the row waits for. Never at another
        // launch: a pane relaunched since the wake holds a program these
        // words were not for (t-7812 R2).
        Resolution::FallbackDeliver(pending) => {
            if window.launch(term) != pending.launch {
                return None;
            }
            window.note(&fallback_line(term, &pending, Fallback::Typed));
            window.type_again(term, &pending.agent, &pending.text)
        }
        // The Enter alone, and only at the launch the words were placed at:
        // the delivery itself withholds it from a line a person reached.
        Resolution::FallbackSubmit(pending) => {
            if window.launch(term) != pending.launch {
                return None;
            }
            window.note(&fallback_line(term, &pending, Fallback::Submitted));
            window.submit_again(term, &pending.agent, pending.hand)
        }
        // A full window after the first and still no receipt: the line is
        // filed `receipt=none`, and nothing is delivered again.
        Resolution::GaveUp(pending) => {
            window.note(&log_line(
                term,
                &pending.agent,
                &pending.session_id,
                Some(pending.road),
                None,
            ));
            None
        }
    }
}

/// The most beats one wake's watch runs: the first window, a window for
/// the one fallback, and the close that gives up. The table gives up on its
/// own by then; this only keeps a watch from outliving its row.
const WATCH_BEATS: usize = 3;

/// Watch one marked wake through its receipt windows (t-3058, t-7812 R2,
/// t-17037), each `window` long — the product's is
/// [`RESUME_NUDGE_RECEIPT_MS`].
///
/// `delivery` is the first delivery's own answer, for words typed at a
/// composer; words that rode the argv come registered as already said. It is
/// heard the moment it comes, however many windows that takes: words left on
/// the line with their Enter untaken get the one fallback there and then —
/// an Enter alone, which waits for the composer's ready. A window's close
/// gives words that never left a composer that was not ready the one
/// fallback, typed beside whatever a person has left on the line; every
/// other wake waits out the window after for its `working` hook. Nothing
/// here types, or presses, because a hook was silent.
pub(super) fn watch(
    receipts: &dyn WakeReceipts,
    term: TermId,
    delivery: Option<std::sync::mpsc::Receiver<DeliveryOutcome>>,
    window: Duration,
) {
    let mut delivery = delivery;
    for _ in 0..WATCH_BEATS {
        if !receipts.rows().holds(term) {
            break;
        }
        let beat = Instant::now() + window;
        delivery = hear_until(receipts, term, delivery, beat);
        std::thread::sleep(beat.saturating_duration_since(Instant::now()));
        let resolution = receipts.rows().timeout(term, Instant::now(), window);
        match resolution {
            Some(resolution) => {
                if let Some(next) = placed(receipts, term, resolution) {
                    delivery = Some(next);
                }
            }
            None if !receipts.rows().holds(term) => break,
            None => {}
        }
    }
}

/// Listen for a delivery's answer until `beat`, acting on each as it comes
/// (t-17037): an answer can arm the Enter alone, whose own answer is then
/// listened for too. Answers what is still unanswered at the beat.
fn hear_until(
    receipts: &dyn WakeReceipts,
    term: TermId,
    mut delivery: Option<std::sync::mpsc::Receiver<DeliveryOutcome>>,
    beat: Instant,
) -> Option<std::sync::mpsc::Receiver<DeliveryOutcome>> {
    while let Some(answer) = delivery.take() {
        match answer.recv_timeout(beat.saturating_duration_since(Instant::now())) {
            Ok(outcome) => {
                let fallback = receipts.rows().heard(term, outcome, Instant::now());
                delivery = fallback.and_then(|resolution| placed(receipts, term, resolution));
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => return Some(answer),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {}
        }
    }
    None
}

/// Act on a resolution; a fallback that went leaves the row waiting on its
/// own answer — until then nobody can say what it did.
fn placed(
    receipts: &dyn WakeReceipts,
    term: TermId,
    resolution: Resolution,
) -> Option<std::sync::mpsc::Receiver<DeliveryOutcome>> {
    let delivery = note_resolution(receipts, term, resolution);
    if delivery.is_some() {
        receipts.rows().placed_again(term);
    }
    delivery
}

/// The window's own receipts: its pending table, its typed-prompt door and
/// its log.
struct WindowReceipts(AppHandle);

impl WakeReceipts for WindowReceipts {
    fn rows(&self) -> std::sync::MutexGuard<'_, PendingNudges> {
        self.0.state::<AppState>().inner().pending_nudges()
    }

    fn launch(&self, term: TermId) -> Option<u64> {
        crate::cmd::terminal::launch_of(&self.0.state::<AppState>(), term)
    }

    fn type_again(
        &self,
        term: TermId,
        agent: &str,
        text: &str,
    ) -> Option<std::sync::mpsc::Receiver<DeliveryOutcome>> {
        crate::cmd::terminal::type_prompt_at_term(
            &self.0.state::<AppState>(),
            term,
            text.to_string(),
            true,
            Some(agent),
            crate::cmd::terminal::PromptReadiness::RestingBesideADraft,
        )
        .ok()
    }

    fn submit_again(
        &self,
        term: TermId,
        agent: &str,
        hand: Option<u64>,
    ) -> Option<std::sync::mpsc::Receiver<DeliveryOutcome>> {
        crate::cmd::terminal::press_enter_at_term(
            &self.0.state::<AppState>(),
            term,
            Some(agent),
            hand,
        )
        .ok()
    }

    fn note(&self, line: &str) {
        note_window_event(self.0.state::<AppState>().local_data_root(), line);
    }
}

/// Type a wake's words at a composer that is still mounting — the first
/// delivery of a composer-road nudge. Answers the delivery's own answer.
pub(super) fn deliver_composer(
    state: &AppState,
    term: TermId,
    agent: &str,
    text: &str,
) -> Option<std::sync::mpsc::Receiver<DeliveryOutcome>> {
    crate::cmd::terminal::type_prompt_at_term(
        state,
        term,
        text.to_string(),
        true,
        Some(agent),
        crate::cmd::terminal::PromptReadiness::Mounting,
    )
    .ok()
}

/// Arm the product's receipt watch for one marked wake: its row, and the
/// timer thread that walks [`watch`] on the product's window.
pub(super) fn arm_in_window(
    app: &AppHandle,
    term: TermId,
    pending: PendingNudge,
    delivery: Option<std::sync::mpsc::Receiver<DeliveryOutcome>>,
) {
    app.state::<AppState>()
        .pending_nudges()
        .register(term, pending);
    let app = app.clone();
    std::thread::spawn(move || {
        watch(
            &WindowReceipts(app),
            term,
            delivery,
            Duration::from_millis(RESUME_NUDGE_RECEIPT_MS),
        );
    });
}

/// A restored worker's continuation, typed by the ledger's reseat road
/// (t-17037), answered that its words are on the line and their Enter was
/// not taken. The words are the reseat's own and already spent
/// (`deliver_continuation`); what is left is the same one fallback a wake's
/// row gets — an Enter alone at the composer's next ready — so it takes the
/// same row and the same watch. `hand` is the line's hand count read before
/// the words were typed.
pub(crate) fn arm_left_unsent(
    app: &AppHandle,
    term: TermId,
    words: LeftUnsent<'_>,
    outcome: DeliveryOutcome,
) {
    if let Some((pending, delivery)) = left_unsent(words, outcome) {
        arm_in_window(app, term, pending, Some(delivery));
    }
}

/// The row a reseat's continuation waits in, and its delivery's answer to
/// be heard first, when that answer left the words unsent (t-17037).
fn left_unsent(
    words: LeftUnsent<'_>,
    outcome: DeliveryOutcome,
) -> Option<(PendingNudge, std::sync::mpsc::Receiver<DeliveryOutcome>)> {
    if Said::of(outcome) != Said::Unsent {
        return None;
    }
    let pending = PendingNudge {
        agent: words.agent.to_string(),
        session_id: words.session_id.to_string(),
        road: zerocode_core::NudgeRoad::Composer,
        text: words.text.to_string(),
        started: Instant::now(),
        first_window_closed: None,
        said: None,
        launch: words.launch,
        hand: words.hand,
        fallback: None,
        owed: None,
    };
    let (answer, delivery) = std::sync::mpsc::sync_channel(1);
    answer.send(outcome).ok()?;
    Some((pending, delivery))
}

/// What the reseat road knows about the words it typed (t-17037).
pub(crate) struct LeftUnsent<'a> {
    pub(crate) agent: &'a str,
    pub(crate) session_id: &'a str,
    pub(crate) text: &'a str,
    /// The launch and the hand count the line held before the words were
    /// typed.
    pub(crate) launch: Option<u64>,
    pub(crate) hand: Option<u64>,
}

/// The first working hook is the receipt for a marked wake.
pub(super) fn received_working(app: &AppHandle, term: TermId) {
    received(&WindowReceipts(app.clone()), term);
}

/// A `working` hook reached `term`: its wake, if one waits, is closed.
pub(super) fn received(receipts: &dyn WakeReceipts, term: TermId) {
    let resolution = receipts.rows().working(term, Instant::now());
    if let Some(resolution) = resolution {
        note_resolution(receipts, term, resolution);
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
            first_window_closed: None,
            said: None,
            launch: None,
            hand: None,
            fallback: None,
            owed: None,
        }
    }

    /// A row whose delivery already answered `said`.
    fn answered(
        agent: &str,
        road: zerocode_core::NudgeRoad,
        started: Instant,
        said: Said,
    ) -> PendingNudge {
        PendingNudge {
            said: Some(said),
            ..pending(agent, road, started)
        }
    }

    /// t-3058: the words a restored worker hears. The seat sentence names
    /// the two things a resumed worker got wrong by itself — re-orienting
    /// from scratch and re-running every gate — and rides only on a pane the
    /// ledger seated again; the checkout line rides on any resume that git
    /// could answer for. One line, whichever road delivers it.
    /// t-6428 ⑤: a wake names the commands the restart cut under its pane,
    /// in the goodbye's own summaries, after the turn sentence when the turn
    /// was cut and alone when it had ended; nothing cut leaves the nudge as
    /// it always was.
    #[test]
    fn a_wake_names_the_commands_its_restart_cut() {
        let cut = vec![
            "cargo test -p zerocode-shell".to_string(),
            "just gate".to_string(),
        ];
        let both = resume_nudge(true, true, None, &cut);
        assert_eq!(
            both,
            format!(
                "{RESTART_NUDGE} {CUT_COMMANDS_NUDGE} `cargo test -p zerocode-shell`; `just gate` \
                 {CUT_COMMANDS_TAIL} {RESEATED_NUDGE}"
            )
        );
        assert!(!both.contains('\n'), "the nudge must stay one line");
        let ended = resume_nudge(false, true, None, &cut);
        assert!(
            ended.starts_with(CUT_COMMANDS_NUDGE) && !ended.contains(RESTART_NUDGE),
            "a turn that had ended was told it was cut: {ended}"
        );
        assert_eq!(resume_nudge(true, false, None, &[]), RESTART_NUDGE);
    }

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
        let plain = resume_nudge(true, false, None, &[]);
        assert_eq!(plain, RESTART_NUDGE);
        let seated = resume_nudge(true, true, Some(&state), &[]);
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
        let unseated = resume_nudge(true, false, Some(&state), &[]);
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
        // A composer whose words never left: it was never ready.
        rows.register(
            TEST_TERM_WITHOUT_RECEIPT,
            answered(
                "codex",
                zerocode_core::NudgeRoad::Composer,
                started,
                Said::NotReady,
            ),
        );

        assert!(matches!(
            rows.working(TEST_TERM_WITH_RECEIPT, started + Duration::from_secs(3)),
            Some(Resolution::Working(_, elapsed)) if elapsed == Duration::from_secs(3)
        ));
        assert!(
            rows.timeout(TEST_TERM_WITH_RECEIPT, started + deadline, deadline)
                .is_none()
        );
        assert!(
            rows.timeout(
                TEST_TERM_WITHOUT_RECEIPT,
                started + deadline - Duration::from_millis(1),
                deadline
            )
            .is_none()
        );
        // The first window's close arms the one fallback, and the row is
        // KEPT so the fallback's own receipt can still close it.
        assert!(matches!(
            rows.timeout(TEST_TERM_WITHOUT_RECEIPT, started + deadline, deadline),
            Some(Resolution::FallbackDeliver(_))
        ));
        // A second window of silence after that fallback gives up, once.
        assert!(matches!(
            rows.timeout(
                TEST_TERM_WITHOUT_RECEIPT,
                started + deadline + deadline,
                deadline
            ),
            Some(Resolution::GaveUp(_))
        ));
        assert!(
            rows.timeout(
                TEST_TERM_WITHOUT_RECEIPT,
                started + deadline + deadline + deadline,
                deadline
            )
            .is_none()
        );
    }

    /// t-7812 R2: a hook's silence is not a lost delivery. Words that
    /// reached the pane — on its argv, or taken by its composer, Enter or
    /// not — whose `working` report came late or never came are not typed
    /// a second time: the first window closes on nothing and the second
    /// files `receipt=none`. Nor are words the line refused (a person's
    /// draft, a relaunch), nor words whose delivery never answered: nobody
    /// can say they were not sent. Only words that never left a composer
    /// that was not ready get the one fallback.
    #[test]
    fn a_silent_hook_never_types_the_words_again() {
        let started = Instant::now();
        let deadline = Duration::from_millis(RESUME_NUDGE_RECEIPT_MS);
        let argv = zerocode_core::NudgeRoad::Argv;
        let composer = zerocode_core::NudgeRoad::Composer;
        let rows_for = |row: PendingNudge| {
            let mut rows = PendingNudges::default();
            rows.register(TEST_TERM_WITHOUT_RECEIPT, row);
            rows
        };
        for (shape, row) in [
            (
                "argv, said at launch",
                answered("claude", argv, started, Said::Reached),
            ),
            (
                "composer, delivered",
                answered("codex", composer, started, Said::Reached),
            ),
            (
                "composer, refused by the line",
                answered("codex", composer, started, Said::Withheld),
            ),
            (
                "composer, never answered",
                pending("codex", composer, started),
            ),
        ] {
            let mut rows = rows_for(row);
            assert!(
                rows.timeout(TEST_TERM_WITHOUT_RECEIPT, started + deadline, deadline)
                    .is_none(),
                "{shape}: the first window typed the words again"
            );
            assert!(
                matches!(
                    rows.timeout(
                        TEST_TERM_WITHOUT_RECEIPT,
                        started + deadline + deadline,
                        deadline
                    ),
                    Some(Resolution::GaveUp(_))
                ),
                "{shape}: the wake never gave up"
            );
        }
        // What the delivery answers is what the row believes.
        assert_eq!(Said::of(DeliveryOutcome::Delivered), Said::Reached);
        assert_eq!(
            Said::of(DeliveryOutcome::Unsubmitted(
                zerocode_pty::ready::Refusal::HandReached
            )),
            Said::Reached,
            "a paste that went in without its Enter still reached the pane"
        );
        assert_eq!(Said::of(DeliveryOutcome::TimedOut), Said::NotReady);
        assert_eq!(
            Said::of(DeliveryOutcome::Refused(
                zerocode_pty::ready::Refusal::HoldsADraft
            )),
            Said::Withheld
        );
        let mut rows = rows_for(pending("codex", composer, started));
        rows.heard(
            TEST_TERM_WITHOUT_RECEIPT,
            DeliveryOutcome::Delivered,
            started,
        );
        assert!(
            rows.timeout(TEST_TERM_WITHOUT_RECEIPT, started + deadline, deadline)
                .is_none(),
            "a delivered composer was typed at again"
        );
    }

    /// t-7812 R2: the goodbye's word is spent exactly when the words may
    /// have reached the pane — a delivery that answered it took them, a
    /// `working` hook, or a wake that ended with the delivery unanswered —
    /// and kept when they are known never to have left, for the road that
    /// tries next.
    #[test]
    fn the_goodbyes_word_is_spent_only_when_the_words_may_have_landed() {
        use crate::orchestration::restart_census::{self, RestartCensus, Turn, WorkerCut};
        let started = Instant::now();
        let deadline = Duration::from_millis(RESUME_NUDGE_RECEIPT_MS);
        let composer = zerocode_core::NudgeRoad::Composer;
        let root = tempfile::tempdir().expect("a data root");
        let owed_by = |worker: &str| Owed {
            root: root.path().to_path_buf(),
            worker: worker.to_string(),
        };
        let cut = |worker: &str| WorkerCut {
            worker: worker.to_string(),
            agent: "codex".to_string(),
            term: 1,
            turn: Turn::Running,
            commands: Some(Vec::new()),
        };
        restart_census::leave_cut(
            root.path(),
            &RestartCensus {
                workers: [
                    "w-heard",
                    "w-hook",
                    "w-silent",
                    "w-unready",
                    "w-refused",
                    "w-unplaced",
                ]
                .into_iter()
                .map(cut)
                .collect(),
                coordinators: Vec::new(),
                tabs: Vec::new(),
                took_ms: 0,
            },
            &|_| false,
        )
        .expect("the goodbye");
        let owed = |worker: &str| restart_census::peek_cut(root.path(), worker).any();
        let row = |worker: &str| PendingNudge {
            owed: Some(owed_by(worker)),
            ..pending("codex", composer, started)
        };
        let mut rows = PendingNudges::default();
        for (term, worker) in [
            (1, "w-heard"),
            (2, "w-hook"),
            (3, "w-silent"),
            (4, "w-unready"),
            (5, "w-refused"),
        ] {
            rows.register(term, row(worker));
        }
        rows.heard(1, DeliveryOutcome::Delivered, started);
        assert!(!owed("w-heard"), "a delivered continuation is still owed");
        rows.working(2, started + Duration::from_secs(1));
        assert!(!owed("w-hook"), "a pane that took input is still owed");
        rows.heard(4, DeliveryOutcome::TimedOut, started);
        rows.heard(
            5,
            DeliveryOutcome::Refused(zerocode_pty::ready::Refusal::LaunchChanged),
            started,
        );
        rows.register(6, row("w-unplaced"));
        rows.heard(6, DeliveryOutcome::TimedOut, started);
        for term in [3, 4, 5, 6] {
            let first = rows.timeout(term, started + deadline, deadline);
            assert_eq!(
                matches!(first, Some(Resolution::FallbackDeliver(_))),
                term == 4 || term == 6,
                "term {term}: {first:?}"
            );
            // The fallback reached the composer for w-unready; w-unplaced's
            // pane was relaunched and it never went.
            if term == 4 {
                rows.placed_again(term);
            }
            let _ = rows.timeout(term, started + deadline + deadline, deadline);
        }
        assert!(
            !owed("w-silent"),
            "a delivery nobody heard back from may have landed, and was kept to be said again"
        );
        assert!(
            owed("w-refused"),
            "words the line refused were spent though they never left"
        );
        // The one fallback went out for the composer that was never ready,
        // and its own answer never came: that too may have landed.
        assert!(
            !owed("w-unready"),
            "a fallback nobody heard back from was kept to be said again"
        );
        assert!(
            owed("w-unplaced"),
            "words whose fallback never went were spent though they never left"
        );
    }

    /// t-7812 R2, the other road: words on the argv reach the pane with its
    /// launch, and nothing types them — the row is born said, and the
    /// goodbye's word is spent as it is placed. Words for a composer that
    /// could not be asked at all were never sent.
    #[test]
    fn words_on_the_argv_are_said_by_the_launch_and_never_typed() {
        use crate::orchestration::restart_census::{self, RestartCensus, Turn, WorkerCut};
        let root = tempfile::tempdir().expect("a data root");
        restart_census::leave_cut(
            root.path(),
            &RestartCensus {
                workers: ["w-argv", "w-gone"]
                    .into_iter()
                    .map(|worker| WorkerCut {
                        worker: worker.to_string(),
                        agent: "claude".to_string(),
                        term: 1,
                        turn: Turn::Running,
                        commands: Some(Vec::new()),
                    })
                    .collect(),
                coordinators: Vec::new(),
                tabs: Vec::new(),
                took_ms: 0,
            },
            &|_| false,
        )
        .expect("the goodbye");
        let words = |road, worker: &str| Words {
            agent: "claude",
            session_id: "01234567-session",
            road,
            text: RESTART_NUDGE.to_string(),
            launch: Some(7),
            hand: None,
            owed: Owed {
                root: root.path().to_path_buf(),
                worker: worker.to_string(),
            },
        };
        let typed = std::cell::Cell::new(0);
        let (row, delivery) = place_words(
            |_| {
                typed.set(typed.get() + 1);
                None
            },
            words(zerocode_core::NudgeRoad::Argv, "w-argv"),
        );
        assert_eq!(typed.get(), 0, "argv words were typed at the composer too");
        assert!(delivery.is_none());
        assert_eq!(row.said, Some(Said::Reached));
        assert!(row.owed.is_none());
        assert!(!restart_census::peek_cut(root.path(), "w-argv").any());
        let (row, delivery) = place_words(
            |_| {
                typed.set(typed.get() + 1);
                None
            },
            words(zerocode_core::NudgeRoad::Composer, "w-gone"),
        );
        assert_eq!(typed.get(), 1);
        assert!(delivery.is_none());
        assert_eq!(row.said, Some(Said::Withheld));
        assert!(
            restart_census::peek_cut(root.path(), "w-gone").any(),
            "words never sent were spent"
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
            answered(
                "codex",
                zerocode_core::NudgeRoad::Composer,
                started,
                Said::NotReady,
            ),
        );
        assert!(matches!(
            rows.timeout(TEST_TERM_WITHOUT_RECEIPT, started + deadline, deadline),
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
            rows.timeout(TEST_TERM_WITHOUT_RECEIPT, receipt_at + deadline, deadline)
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

    /* ---- an Enter left untaken is pressed once, alone (t-17037) -------- */

    /// A receipt window short enough for a test to wait out every beat.
    const SHORT_WINDOW: Duration = Duration::from_millis(15);

    /// A fake window under the product's own watch: it books every word
    /// typed again and every Enter pressed again, answers each fallback the
    /// way a test told it to, and keeps the log.
    #[derive(Default)]
    struct FakeWindow {
        rows: std::sync::Mutex<PendingNudges>,
        launch: Option<u64>,
        typed: std::sync::Mutex<Vec<String>>,
        entered: std::sync::Mutex<Vec<Option<u64>>>,
        noted: std::sync::Mutex<Vec<String>>,
        /// What each fallback's delivery answers, in order; `Delivered` past
        /// the end.
        answers: std::sync::Mutex<std::collections::VecDeque<DeliveryOutcome>>,
    }

    impl FakeWindow {
        fn answering(answers: &[DeliveryOutcome]) -> Self {
            Self {
                answers: std::sync::Mutex::new(answers.iter().copied().collect()),
                ..Self::default()
            }
        }

        fn answer(&self) -> std::sync::mpsc::Receiver<DeliveryOutcome> {
            let outcome = self
                .answers
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(DeliveryOutcome::Delivered);
            answered_with(outcome)
        }

        /// Walk one wake on the product's watch, its first delivery having
        /// answered `first` (or nothing, for words on the argv).
        fn walk(&self, term: TermId, row: PendingNudge, first: Option<DeliveryOutcome>) {
            self.rows.lock().unwrap().register(term, row);
            watch(self, term, first.map(answered_with), SHORT_WINDOW);
        }

        fn typed(&self) -> usize {
            self.typed.lock().unwrap().len()
        }

        fn entered(&self) -> Vec<Option<u64>> {
            self.entered.lock().unwrap().clone()
        }

        fn noted_with(&self, words: &str) -> usize {
            self.noted
                .lock()
                .unwrap()
                .iter()
                .filter(|line| line.contains(words))
                .count()
        }
    }

    impl WakeReceipts for FakeWindow {
        fn rows(&self) -> std::sync::MutexGuard<'_, PendingNudges> {
            self.rows.lock().unwrap()
        }

        fn launch(&self, _term: TermId) -> Option<u64> {
            self.launch
        }

        fn type_again(
            &self,
            _term: TermId,
            _agent: &str,
            text: &str,
        ) -> Option<std::sync::mpsc::Receiver<DeliveryOutcome>> {
            self.typed.lock().unwrap().push(text.to_string());
            Some(self.answer())
        }

        fn submit_again(
            &self,
            _term: TermId,
            _agent: &str,
            hand: Option<u64>,
        ) -> Option<std::sync::mpsc::Receiver<DeliveryOutcome>> {
            self.entered.lock().unwrap().push(hand);
            Some(self.answer())
        }

        fn note(&self, line: &str) {
            self.noted.lock().unwrap().push(line.to_string());
        }
    }

    /// A delivery's answer, already given.
    fn answered_with(outcome: DeliveryOutcome) -> std::sync::mpsc::Receiver<DeliveryOutcome> {
        let (said, heard) = std::sync::mpsc::sync_channel(1);
        said.send(outcome).expect("the answer");
        heard
    }

    const NOT_TAKEN: DeliveryOutcome =
        DeliveryOutcome::Unsubmitted(zerocode_pty::ready::Refusal::NotTaken);

    /// 07:08 on 2026-09-30: the words went in, their Enter was never taken,
    /// and they sat in the composer until a person pressed Enter. Words left
    /// on the line with their Enter untaken get exactly one Enter alone —
    /// never a second paste — and a pane that still reports nothing after it
    /// is not pressed again. The line says `submitted again`.
    #[test]
    fn an_enter_left_untaken_is_pressed_once_and_the_words_are_never_typed_again() {
        let window = FakeWindow::answering(&[NOT_TAKEN]);
        let row = PendingNudge {
            hand: Some(11),
            ..pending("codex", zerocode_core::NudgeRoad::Composer, Instant::now())
        };
        window.walk(TEST_TERM_WITHOUT_RECEIPT, row, Some(NOT_TAKEN));
        assert_eq!(
            window.entered(),
            vec![Some(11)],
            "not exactly one Enter, or not on the hand count from before the words"
        );
        assert_eq!(window.typed(), 0, "the words were typed a second time");
        assert_eq!(window.noted_with("submitted again"), 1);
        assert_eq!(window.noted_with("typed again"), 0);
        assert_eq!(
            window.noted_with("receipt=none"),
            1,
            "the wake never gave up"
        );
        assert!(!window.rows().holds(TEST_TERM_WITHOUT_RECEIPT));
        // The terminal refusing the Enter's write is the same state.
        assert_eq!(
            Said::of(DeliveryOutcome::Unsubmitted(
                zerocode_pty::ready::Refusal::InputRejected
            )),
            Said::Unsent
        );
    }

    /// A person's hand in between withdraws the Enter. Their own Enter (or
    /// the pane taking the words late) is the pane's `working` hook, which
    /// closes the row before any Enter is armed; a hand that only edits the
    /// line reaches the Enter alone at its write, which the delivery then
    /// withholds (`prompt_transaction` tests). A hand, a parked question or a
    /// relaunch that stopped the first Enter already made the words the
    /// person's, and no Enter is ever pressed for them.
    #[test]
    fn a_person_s_hand_in_between_withdraws_the_enter() {
        use zerocode_pty::ready::Refusal;
        let started = Instant::now();
        let composer = zerocode_core::NudgeRoad::Composer;
        let mut rows = PendingNudges::default();
        rows.register(
            TEST_TERM_WITHOUT_RECEIPT,
            pending("claude", composer, started),
        );
        assert!(rows.working(TEST_TERM_WITHOUT_RECEIPT, started).is_some());
        assert!(
            rows.heard(TEST_TERM_WITHOUT_RECEIPT, NOT_TAKEN, started)
                .is_none(),
            "an Enter was armed on a pane that had already taken input"
        );
        for why in [
            Refusal::HandReached,
            Refusal::Parked,
            Refusal::LaunchChanged,
        ] {
            let window = FakeWindow::default();
            window.walk(
                TEST_TERM_WITHOUT_RECEIPT,
                pending("claude", composer, Instant::now()),
                Some(DeliveryOutcome::Unsubmitted(why)),
            );
            assert!(window.entered().is_empty(), "{why:?}: an Enter was pressed");
            assert_eq!(window.typed(), 0, "{why:?}");
        }
        // A pane relaunched since the words were placed is not pressed at.
        let window = FakeWindow {
            launch: Some(2),
            ..FakeWindow::default()
        };
        window.walk(
            TEST_TERM_WITHOUT_RECEIPT,
            PendingNudge {
                launch: Some(1),
                ..pending("claude", composer, Instant::now())
            },
            Some(NOT_TAKEN),
        );
        assert!(
            window.entered().is_empty(),
            "an Enter went to another launch"
        );
    }

    /// Words delivered with their Enter taken need nothing more; words that
    /// never left a composer that was not ready keep today's one typing
    /// fallback — `typed again`, and no Enter alone beside it.
    #[test]
    fn a_delivered_wake_is_left_alone_and_an_unready_one_is_typed_once() {
        let composer = zerocode_core::NudgeRoad::Composer;
        let delivered = FakeWindow::default();
        delivered.walk(
            TEST_TERM_WITH_RECEIPT,
            pending("codex", composer, Instant::now()),
            Some(DeliveryOutcome::Delivered),
        );
        assert!(delivered.entered().is_empty());
        assert_eq!(delivered.typed(), 0);
        assert_eq!(delivered.noted_with("again"), 0);

        // The typed fallback's own paste lands without its Enter taken: that
        // was the wake's one fallback, and no Enter follows it.
        let unready = FakeWindow::answering(&[NOT_TAKEN]);
        unready.walk(
            TEST_TERM_WITHOUT_RECEIPT,
            pending("codex", composer, Instant::now()),
            Some(DeliveryOutcome::TimedOut),
        );
        assert_eq!(
            unready.typed(),
            1,
            "the unready composer was not typed at once"
        );
        assert!(unready.entered().is_empty(), "a second fallback went");
        assert_eq!(unready.noted_with("typed again"), 1);
        assert_eq!(unready.noted_with("submitted again"), 0);
    }

    /// The fallback's line names which one went, beside the wake's own head.
    #[test]
    fn the_fallback_line_says_submitted_or_typed_again() {
        let row = pending("codex", zerocode_core::NudgeRoad::Composer, Instant::now());
        assert_eq!(
            fallback_line(TEST_TERM_WITH_RECEIPT, &row, Fallback::Submitted),
            "term 41 resumed codex 01234567 nudge=composer submitted again"
        );
        assert_eq!(
            fallback_line(TEST_TERM_WITH_RECEIPT, &row, Fallback::Typed),
            "term 41 resumed codex 01234567 nudge=composer typed again"
        );
    }

    /// Every catalog row, on both roads that bring a conversation back: the
    /// window's wake by the row's own nudge road, and the ledger's reseat,
    /// which types every agent's continuation at its composer. Words on the
    /// argv are never typed or pressed for; words a composer left with their
    /// Enter untaken get exactly one Enter alone. No agent is special.
    #[test]
    fn every_catalog_row_gets_one_enter_for_words_left_untaken() {
        use crate::orchestration::restart_census::{self, RestartCensus};
        let root = tempfile::tempdir().expect("a data root");
        restart_census::leave_cut(
            root.path(),
            &RestartCensus {
                workers: Vec::new(),
                coordinators: Vec::new(),
                tabs: Vec::new(),
                took_ms: 0,
            },
            &|_| false,
        )
        .expect("the goodbye");
        for spec in &zerocode_core::AGENT_SPECS {
            let window = FakeWindow::default();
            let (row, first) = place_words(
                |_| Some(answered_with(NOT_TAKEN)),
                Words {
                    agent: spec.id,
                    session_id: "01234567-session",
                    road: spec.resume_nudge,
                    text: RESTART_NUDGE.to_string(),
                    launch: None,
                    hand: None,
                    owed: Owed {
                        root: root.path().to_path_buf(),
                        worker: format!("w-{}", spec.id),
                    },
                },
            );
            window.rows().register(TEST_TERM_WITHOUT_RECEIPT, row);
            watch(&window, TEST_TERM_WITHOUT_RECEIPT, first, SHORT_WINDOW);
            let enters = match spec.resume_nudge {
                zerocode_core::NudgeRoad::Argv => 0,
                zerocode_core::NudgeRoad::Composer => 1,
            };
            assert_eq!(window.entered().len(), enters, "{}: the wake", spec.id);
            assert_eq!(window.typed(), 0, "{}: the wake typed again", spec.id);

            let reseat = FakeWindow::default();
            let (row, first) = left_unsent(
                LeftUnsent {
                    agent: spec.id,
                    session_id: "01234567-session",
                    text: RESTART_NUDGE,
                    launch: None,
                    hand: Some(3),
                },
                NOT_TAKEN,
            )
            .expect("an untaken Enter is watched");
            reseat.rows().register(TEST_TERM_WITHOUT_RECEIPT, row);
            watch(
                &reseat,
                TEST_TERM_WITHOUT_RECEIPT,
                Some(first),
                SHORT_WINDOW,
            );
            assert_eq!(reseat.entered(), vec![Some(3)], "{}: the reseat", spec.id);
            assert_eq!(reseat.typed(), 0, "{}: the reseat typed again", spec.id);
            assert!(
                left_unsent(
                    LeftUnsent {
                        agent: spec.id,
                        session_id: "",
                        text: RESTART_NUDGE,
                        launch: None,
                        hand: None,
                    },
                    DeliveryOutcome::Delivered,
                )
                .is_none(),
                "{}: a delivered continuation was watched",
                spec.id
            );
        }
    }

    /// When, after its start, `delivery` first writes anything at a program
    /// that behaves as one just started (t-18353): a handshake round, then a
    /// first frame 50 ms later that draws the composer's glyph and shows the
    /// cursor, then nothing at all — round after round on a 50 ms clock,
    /// which is what 2.1.285 gives for seconds after its first frame — until
    /// the delivery writes or gives up. `None` when it gave up without
    /// writing. A synthetic clock, so the answer is exact and instant.
    fn first_write_at_a_program_just_started(
        mut delivery: PromptDelivery,
        start: Instant,
    ) -> Option<Duration> {
        const ROUND: Duration = Duration::from_millis(50);
        let line = zerocode_pty::ready::Line::default();
        let mut now = start;
        for round in 0..3_000 {
            let seen = match round {
                0 => Observed {
                    wrote: true,
                    bracketed_paste: true,
                    ..Observed::default()
                },
                1 => Observed {
                    wrote: true,
                    bracketed_paste: true,
                    cursor_shows: 1,
                    marker_written: true,
                    marker_in_alt: true,
                    alt_screen: true,
                },
                _ => Observed {
                    wrote: false,
                    bracketed_paste: true,
                    cursor_shows: 1,
                    marker_written: false,
                    marker_in_alt: false,
                    alt_screen: true,
                },
            };
            match delivery.poll_line(seen, line, now) {
                DeliveryStep::Write(_) | DeliveryStep::Submit(_) => return Some(now - start),
                DeliveryStep::Done(_) => return None,
                DeliveryStep::Waiting => {}
            }
            now += ROUND;
        }
        None
    }

    /// A restored worker's continuation is typed at a program the window
    /// started moments ago, and waits behind the door its row names for that
    /// (t-18353) — every catalog row walked. The Enter pressed again for it
    /// does not: its words were placed behind that door some ten seconds
    /// before, so it waits for rest, as it always did (t-17037), whatever
    /// the row.
    ///
    /// Measured on Claude Code 2.1.285: its first frame draws the composer
    /// 30 ms after its handshake, and words placed there are lost, or left
    /// unsent with every Enter after them ignored. The door the reseat used
    /// — a running composer's rest — opens on that first glyph. A row that
    /// says it has such a hand-over (`start_settle_ms`) is typed at only
    /// after its settle; a row that says nothing is typed at exactly as
    /// before.
    #[test]
    fn a_restored_pane_is_typed_at_only_after_its_row_says_it_has_settled() {
        use crate::cmd::terminal::{PromptReadiness, prompt_delivery_for};
        let start = Instant::now();
        let mut settled = Vec::new();
        for spec in &zerocode_core::AGENT_SPECS {
            let row = Some(spec.id);
            let door = |readiness, words: &str| {
                first_write_at_a_program_just_started(
                    prompt_delivery_for(words.to_string(), true, row, readiness, None, start),
                    start,
                )
            };
            let running = door(PromptReadiness::RestingBesideADraft, "words");
            let restored = door(PromptReadiness::MountingBesideADraft, "words");
            let enter_again = door(PromptReadiness::EnterAgain(None), "");
            // The words and the Enter alone both go on the round a running
            // composer's door opens, so one time answers for both — for
            // every row: the Enter pressed again never waits out a settle.
            assert_eq!(
                enter_again, running,
                "{}: the Enter pressed again did not wait for rest alone",
                spec.id
            );
            match start_settle_for(row) {
                Some(settle) => {
                    settled.push(spec.id);
                    assert!(
                        running.is_some_and(|at| at < settle),
                        "{}: the door the reseat used no longer opens inside the hand-over \
                         ({running:?}, settle {settle:?}) — the measurement this test stands on",
                        spec.id
                    );
                    assert!(
                        restored.is_some_and(|at| at >= settle),
                        "{}: the words were placed at a program still handing over its \
                         start-up ({restored:?}, settle {settle:?})",
                        spec.id
                    );
                }
                None => assert_eq!(
                    restored, running,
                    "{}: a row with no hand-over was typed at differently",
                    spec.id
                ),
            }
        }
        assert_eq!(
            settled,
            ["claude"],
            "the rows that wait for a hand-over are the measured ones"
        );
    }

    /// A restored worker's continuation the pane reported taking files the
    /// wake's own receipt line (t-18353) — every catalog row walked: a row
    /// whose program reports the prompts it takes files
    /// `receipt=working@Ns` in the wake road's exact words, and a row that
    /// reports nothing files none, because its delivery says nothing about
    /// the words being taken.
    #[test]
    fn a_continuation_a_pane_reported_taking_files_the_wake_receipt_line() {
        for spec in &zerocode_core::AGENT_SPECS {
            let line = continuation_taken_line(
                TEST_TERM_WITH_RECEIPT,
                spec.id,
                "01234567-session",
                Duration::from_secs(9),
            );
            let reports = spec.harness.submit_ack != zerocode_core::capabilities::SubmitAck::None;
            assert_eq!(line.is_some(), reports, "{}", spec.id);
            if let Some(line) = line {
                assert_eq!(
                    line,
                    format!(
                        "term 41 resumed {} 01234567 nudge=composer receipt=working@9s",
                        spec.id
                    ),
                    "{}",
                    spec.id
                );
            }
        }
    }
}
