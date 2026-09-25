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
    let state = checkout.and_then(|checkout| {
        worktree_state(
            checkout,
            u64::try_from(now_epoch_ms() / 1_000).unwrap_or_default(),
        )
    });
    Some(resume_nudge(cut.turn, true, state.as_ref(), &cut.commands))
}

/// `worker`'s words were handed to a pane that holds it (t-7812 R2): they
/// reached it, or may have — and words that may have landed are never said
/// a second time. The goodbye's word about that worker is spent.
pub(super) fn nudge_spent(root: &Path, worker: &str) {
    crate::orchestration::restart_census::spend_cut(root, worker);
}

/// The goodbye's word a wake's words came from: the data root its note lives
/// in and the worker it was about, so the words are spent exactly when they
/// may have reached the pane (t-7812 R2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Owed {
    pub(super) root: PathBuf,
    pub(super) worker: String,
}

/// What a wake's delivery answered about its words (t-7812 R2). The one
/// thing that decides whether they may be typed again: only words that never
/// left, at a composer that was not ready, are still the wake's to place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Said {
    /// The words reached the child — its argv, or a paste its composer took,
    /// Enter or not. Never typed again, whatever the hooks say after.
    Reached,
    /// Nothing was sent: the composer never said it was ready. The line is
    /// still the wake's, and the one fallback may place the words there.
    NotReady,
    /// Nothing was sent, and the line was not the wake's to write on: a
    /// person's draft or hand, a parked question, another launch. Nothing is
    /// typed there again.
    Withheld,
}

impl Said {
    pub(super) const fn of(outcome: DeliveryOutcome) -> Self {
        match outcome {
            DeliveryOutcome::Delivered | DeliveryOutcome::Unsubmitted(_) => Self::Reached,
            DeliveryOutcome::TimedOut => Self::NotReady,
            DeliveryOutcome::Refused(_) => Self::Withheld,
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
    pub(super) fn heard(&mut self, term: TermId, outcome: DeliveryOutcome) {
        if let Some(pending) = self.rows.get_mut(&term) {
            let said = Said::of(outcome);
            pending.said = Some(said);
            if said == Said::Reached {
                pending.spend();
            }
        }
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
                (pending.said == Some(Said::NotReady))
                    .then(|| Resolution::FallbackDeliver(pending.clone()))
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

/// One worker wake's words, as the wake hands them over (t-7812 R2).
pub(super) struct Words<'a> {
    pub(super) agent: &'a str,
    pub(super) session_id: &'a str,
    /// The road the agent's row says a resume nudge takes.
    pub(super) road: zerocode_core::NudgeRoad,
    pub(super) text: String,
    /// The launch the pane holds as the words are placed.
    pub(super) launch: Option<u64>,
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
        owed: Some(words.owed),
    };
    if said == Some(Said::Reached) {
        pending.spend();
    }
    (pending, delivery)
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
            window.type_again(term, &pending.agent, &pending.text)
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

/// Watch one marked wake through its two receipt windows (t-3058, t-7812
/// R2), each `window` long — the product's is [`RESUME_NUDGE_RECEIPT_MS`].
///
/// `delivery` is the first delivery's own answer, for words typed at a
/// composer; words that rode the argv come registered as already said. The
/// first window hears that answer and then closes: words that never left a
/// composer that was not ready get the one fallback, typed beside whatever a
/// person has left on the line, and every other wake just waits out the
/// second window for its `working` hook. Nothing here types because a hook
/// was silent.
pub(super) fn watch(
    receipts: &dyn WakeReceipts,
    term: TermId,
    delivery: Option<std::sync::mpsc::Receiver<DeliveryOutcome>>,
    window: Duration,
) {
    let mut delivery = delivery;
    for _ in 0..2 {
        let beat = Instant::now() + window;
        if let Some(answer) = delivery.take()
            && let Ok(outcome) = answer.recv_timeout(window)
        {
            receipts.rows().heard(term, outcome);
        }
        std::thread::sleep(beat.saturating_duration_since(Instant::now()));
        let resolution = receipts.rows().timeout(term, Instant::now(), window);
        match resolution {
            Some(resolution) => {
                delivery = note_resolution(receipts, term, resolution);
                if delivery.is_some() {
                    receipts.rows().placed_again(term);
                }
            }
            None if !receipts.rows().holds(term) => break,
            None => {}
        }
    }
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
        rows.heard(TEST_TERM_WITHOUT_RECEIPT, DeliveryOutcome::Delivered);
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
        rows.heard(1, DeliveryOutcome::Delivered);
        assert!(!owed("w-heard"), "a delivered continuation is still owed");
        rows.working(2, started + Duration::from_secs(1));
        assert!(!owed("w-hook"), "a pane that took input is still owed");
        rows.heard(4, DeliveryOutcome::TimedOut);
        rows.heard(
            5,
            DeliveryOutcome::Refused(zerocode_pty::ready::Refusal::LaunchChanged),
        );
        rows.register(6, row("w-unplaced"));
        rows.heard(6, DeliveryOutcome::TimedOut);
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
}
