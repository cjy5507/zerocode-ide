//! The window's independent 30-second PR feedback loop.
use crate::{AppState, ShellStateExt, checks_runtime, gh};
use gh::observer::{Budget, Client, SUBJECT_CALLS_MAX, read_mergeability, read_state};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Mutex, MutexGuard};
use tauri::{AppHandle, Emitter, Manager};
use zerocode_core::checks::Limits;
use zerocode_core::scm_observer::{
    self as core, Book, Ci, Effects, FactKind, Observation, Operation, PrState, Subject,
};

mod health;
use health::TickHealth;

#[derive(Default)]
pub(crate) struct Observer {
    loaded: Option<PathBuf>,
    book: Book,
    clients: BTreeMap<PathBuf, Client>,
    times: BTreeMap<PathBuf, Times>,
    next_root: usize,
    retry_roots: BTreeSet<PathBuf>,
    /// While a tick is out asking GitHub: the checkouts a person asked to
    /// retry since it was planned, and when. The tick's close writes them
    /// again after its own answers, so a retry asked for during a tick
    /// outlives the tick, as it did when it had to wait for it (t-47457).
    retried: Option<BTreeMap<PathBuf, i64>>,
}
#[derive(Clone, Copy, Default)]
struct Times {
    reviews: Option<i64>,
    force: Option<i64>,
}

struct CacheUpdate {
    root: PathBuf,
    repo: String,
    client: Client,
    force: bool,
    reviewed: bool,
}
impl Observer {
    fn request_retry(
        &mut self,
        root: &Path,
        now_ms: i64,
        effects: &mut impl Effects,
    ) -> Result<(), String> {
        let mut health = self.book.health.clone();
        health.retry(&root.to_string_lossy(), now_ms);
        self.book.update_health(health, effects)?;
        self.retry_roots.insert(root.to_path_buf());
        if let Some(retried) = &mut self.retried {
            retried.insert(root.to_path_buf(), now_ms);
        }
        Ok(())
    }

    fn finish_refreshes(
        &mut self,
        updates: Vec<CacheUpdate>,
        failed: &BTreeSet<String>,
        now_ms: i64,
    ) {
        for update in updates {
            if failed.contains(&update.repo) {
                continue;
            }
            self.clients.insert(update.root.clone(), update.client);
            let times = self.times.entry(update.root).or_default();
            if update.reviewed {
                times.reviews = Some(now_ms);
            }
            if update.force {
                times.force = Some(now_ms);
            }
        }
    }
}

struct WindowEffects<'a> {
    path: &'a Path,
    now_ms: i64,
}
impl Effects for WindowEffects<'_> {
    fn save(&mut self, book: &Book) -> Result<(), String> {
        let bytes = serde_json::to_vec(book).map_err(|e| e.to_string())?;
        let committed =
            crate::durable_file::replace_bytes(self.path, &bytes).map_err(|e| e.to_string())?;
        if !committed.platform_durable() {
            return Err("SCM state directory sync failed".into());
        }
        Ok(())
    }
    fn mail(&mut self, subject: &Subject, receipt: &str, body: &str) -> Result<(), String> {
        let identity = zerocode_core::untrusted::fence(
            "GitHub PR identity",
            &format!("PR #{} · {}", subject.number, subject.url),
            1024,
        );
        if crate::orchestration::post_observation_once(
            None,
            &checks_runtime::worktree_address(Path::new(&subject.root)),
            &format!("{identity}\n{body}"),
            receipt,
            self.now_ms,
        ) {
            Ok(())
        } else {
            Err("SCM mail was not accepted by the ledger".into())
        }
    }
}
impl Observer {
    fn load(&mut self, path: &Path) -> Result<(), String> {
        if self.loaded.as_deref() == Some(path) {
            return Ok(());
        }
        let book = match std::fs::read(path) {
            Ok(raw) => serde_json::from_slice(&raw).map_err(|e| format!("SCM state: {e}"))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Book::default(),
            Err(e) => return Err(e.to_string()),
        };
        self.book = book;
        self.loaded = Some(path.into());
        self.clients.clear();
        self.times.clear();
        Ok(())
    }

    /// Marks one notice in the observer's own book, the same book the poll
    /// saves: a second copy read from the file could drop the poll's latest write.
    fn mark_notice(
        &mut self,
        path: &Path,
        id: &str,
        opened_at: i64,
        mark: core::NoticeMark,
        now_ms: i64,
    ) -> Result<bool, String> {
        self.load(path)?;
        self.book.mark_notice(
            id,
            opened_at,
            mark,
            now_ms,
            &mut WindowEffects { path, now_ms },
        )
    }

    /// The checkouts a tick looks at: the live ones and the ones a person
    /// asked to retry, each once, in order.
    fn tick_roots(&mut self, path: &Path, mut live: Vec<PathBuf>) -> Result<Vec<PathBuf>, String> {
        self.load(path)?;
        live.extend(self.retry_roots.iter().cloned());
        live.sort();
        live.dedup();
        Ok(live)
    }
}

/// The observer, held. Every reader and writer of the observer comes through
/// here, and a tick holds it a statement at a time, never while `gh` runs
/// (t-47457; the source contract `scm_observer_lock` reads this file for it).
fn hold(observer: &Mutex<Observer>) -> MutexGuard<'_, Observer> {
    observer
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The window showed a condition's toast, or the person closed it. The mark
/// goes into the ledger, so a restarted window reads it back.
pub(crate) fn set_notice_mark(
    app: &AppHandle,
    id: &str,
    opened_at: i64,
    mark: core::NoticeMark,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    let path = state_path(&state);
    let mut held = hold(checks_runtime::remembered());
    held.mark_notice(&path, id, opened_at, mark, crate::now_epoch_ms())?;
    Ok(())
}

fn state_path(state: &AppState) -> PathBuf {
    state.config_root().join("scm-observer.json")
}

/// Never infer a pane's checkout from the currently focused workspace.
fn live_roots(state: &AppState) -> Vec<PathBuf> {
    let living: BTreeSet<_> = state.terminals().terms().into_iter().collect();
    let agents: Vec<_> = state
        .agent_terms()
        .keys()
        .filter(|term| living.contains(term))
        .copied()
        .collect();
    let isolated = state.isolated_worker_terms().clone();
    let envs = state.team_envs().clone();
    agents
        .into_iter()
        .filter_map(|term| {
            isolated
                .get(&term)
                .map(|held| held.path.clone())
                .or_else(|| envs.get(&term).and_then(|env| crate::leader_worktree(env)))
                .or_else(|| crate::seated_worktree(state, term).map(PathBuf::from))
        })
        .filter(|root| root.is_dir())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// The commit, then the ref the checkout stands on — `HEAD` again when it is
/// detached.
fn checkout_token(root: &Path) -> Result<String, String> {
    let output = zerocode_core::host::lock_free_git(crate::proc::quiet_command("git"))
        .args(["rev-parse", "HEAD", "--symbolic-full-name", "HEAD"])
        .current_dir(root)
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err("cannot read checkout HEAD".into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}
/// A pull request's head is a branch, so a checkout on none has no PR to ask
/// about: `gh pr view` refuses a detached checkout outright ("not on any
/// branch", 0.06 s measured 2026-09-17), every tick, and that refusal is an
/// absence rather than a failed observation.
fn on_a_branch(token: &str) -> bool {
    token
        .lines()
        .nth(1)
        .is_some_and(|name| name.starts_with("refs/heads/"))
}
fn subject(root: &Path, review: &gh::HostedReview, branch: String) -> Subject {
    Subject {
        root: root.to_string_lossy().into_owned(),
        repo: review.owner_repo.clone(),
        number: review.number,
        url: review.url.clone(),
        head: review.head_sha.clone(),
        branch,
    }
}
fn due(last: Option<i64>, now: i64, interval: u64) -> bool {
    last.is_none_or(|last| now.saturating_sub(last) >= i64::try_from(interval).unwrap_or(i64::MAX))
}

static BUSY: AtomicBool = AtomicBool::new(false);
static NEXT: AtomicI64 = AtomicI64::new(0);
struct Permit;
impl Drop for Permit {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::Release);
    }
}

/// Dispatched by StandingBeat. Network work gets its own permit so the window's
/// one-second mail/triage beat keeps running even when GitHub is slow.
pub(crate) fn sweep(app: &AppHandle, now_ms: i64) {
    if now_ms < NEXT.load(Ordering::Acquire) || BUSY.swap(true, Ordering::AcqRel) {
        return;
    }
    let limits = checks_runtime::limits(&app.state::<AppState>());
    NEXT.store(
        now_ms.saturating_add(i64::try_from(limits.observe_ms).unwrap_or(i64::MAX)),
        Ordering::Release,
    );
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _permit = Permit;
        if let Err(error) = poll(&app, &limits, now_ms) {
            crate::note_window_event(
                app.state::<AppState>().local_data_root(),
                &format!("scm-observer: {error}"),
            );
        }
    });
}

fn poll(app: &AppHandle, limits: &Limits, now_ms: i64) -> Result<(), String> {
    let started = std::time::Instant::now();
    let state = app.state::<AppState>();
    let observer = checks_runtime::remembered();
    let live = live_roots(&state);
    let path = state_path(&state);
    let roots = hold(observer).tick_roots(&path, live)?;
    if roots.is_empty() {
        publish_notices(app, &hold(observer).book, &roots);
        return Ok(());
    }
    let mut budget = Budget::new(limits);
    observe(
        observer,
        &roots,
        &mut budget,
        &mut WindowEffects {
            path: &path,
            now_ms,
        },
        limits,
        now_ms,
        state.local_data_root(),
    )?;
    publish_notices(app, &hold(observer).book, &roots);
    // The tick's numbers go into the window's event log only when they are
    // news: a tick that ran past one gh call's own ceiling, or one that spent
    // its whole call budget. An ordinary tick every thirty seconds was a
    // quarter of that log by bytes (3,397 lines over 35 hours, 2026-09-17)
    // and said nothing anyone acts on.
    let elapsed = started.elapsed();
    if elapsed.as_millis() >= u128::from(limits.gh_call_ms) || budget.used >= budget.max {
        crate::note_window_event(
            state.local_data_root(),
            &format!(
                "scm-observer: tick gh_calls={} elapsed_ms={}",
                budget.used,
                elapsed.as_millis()
            ),
        );
    }
    Ok(())
}

/// One tick over the live checkouts, apart from the window: the caller
/// brings the roots, the tick's `gh` budget, the durable effects and the
/// directory of the event log. The observer is held one statement at a
/// time — to read what the tick needs and to write down what it heard — and
/// never while `gh` runs, so the Checks pane's health read and a person's
/// retry wait on the book alone, never on GitHub (t-47457).
fn observe(
    observer: &Mutex<Observer>,
    roots: &[PathBuf],
    budget: &mut Budget,
    effects: &mut impl Effects,
    limits: &Limits,
    now_ms: i64,
    log_root: &Path,
) -> Result<(), String> {
    let Some(mut tick) = hold(observer).plan(roots, now_ms) else {
        return Ok(());
    };
    let discovered = tick.discover(observer, roots, budget, limits);
    let candidates = hold(observer).record_discovery(discovered, effects)?;
    let mut refreshes = Refreshes::default();
    for found in candidates {
        let mut refreshed = tick.refresh(observer, found, budget, limits, now_ms);
        let applied =
            hold(observer)
                .book
                .apply(&refreshed.key, &refreshed.observed, limits, now_ms, effects);
        if let Err(error) = applied {
            refreshed.complete = false;
            crate::note_window_event(log_root, &format!("scm-observer: {error}"));
        }
        if refreshed.complete {
            refreshes.done.push(refreshed.update);
        } else {
            refreshes.failed.insert(refreshed.update.repo);
        }
    }
    hold(observer).close(roots, tick.health, refreshes, effects, now_ms, log_root)
}

/// A tick's own state while it asks GitHub with the observer free: where
/// its rotation starts, and the health it judges what is due by.
struct Tick {
    rotate: usize,
    health: TickHealth,
}

/// What a tick's discovery heard: where the rotation goes next, the
/// checkouts whose retry it answered, and each PR `gh pr view` named.
struct Discovered {
    next_root: usize,
    settled: Vec<PathBuf>,
    found: Vec<Found>,
}

/// A PR a tick found: its checkout, what `gh pr view` said, the subject it
/// is in the book as, and the candidate cache to read it with.
struct Found {
    root: PathBuf,
    review: gh::HostedReview,
    subject: Subject,
    client: Client,
}

/// One PR's facts as a tick read them, whether every due read came back, and
/// the cache and clocks to keep once the facts are written down.
struct Refreshed {
    key: String,
    observed: Observation,
    complete: bool,
    update: CacheUpdate,
}

/// What a tick's refreshes leave for its close: the caches and clocks of the
/// PRs read whole, and the repositories where one was not.
#[derive(Default)]
struct Refreshes {
    done: Vec<CacheUpdate>,
    failed: BTreeSet<String>,
}

impl Observer {
    /// Opens a tick over `roots`, with a note of the retries asked for until
    /// it closes. `None` when there is no checkout to look at.
    fn plan(&mut self, roots: &[PathBuf], now_ms: i64) -> Option<Tick> {
        let rotate = self.next_root.checked_rem(roots.len())?;
        self.retried = Some(BTreeMap::new());
        Some(Tick {
            rotate,
            health: TickHealth::new(&self.book, now_ms),
        })
    }

    /// Writes a tick's discovery down: the rotation moves on, the retries it
    /// answered are done unless a person asked again since the tick was
    /// planned, and each PR the tick will read is in the book before any of
    /// its facts — a PR no longer open only when the book already follows it.
    fn record_discovery(
        &mut self,
        discovered: Discovered,
        effects: &mut impl Effects,
    ) -> Result<Vec<Found>, String> {
        self.next_root = discovered.next_root;
        for root in &discovered.settled {
            let asked_again = self
                .retried
                .as_ref()
                .is_some_and(|retried| retried.contains_key(root));
            if !asked_again {
                self.retry_roots.remove(root);
            }
        }
        let candidates: Vec<Found> = discovered
            .found
            .into_iter()
            .filter(|found| {
                found.review.state == "open"
                    || self.book.subjects.contains_key(&found.subject.key())
            })
            .collect();
        self.book.discover(
            &candidates
                .iter()
                .map(|found| found.subject.clone())
                .collect::<Vec<_>>(),
            effects,
        )?;
        Ok(candidates)
    }

    /// Closes a tick: the caches and clocks of the PRs it read whole, the
    /// checkouts it no longer looks at let go, and what it heard about each
    /// operation's health written into the book's health as that stands now.
    /// The retries a person asked for while it ran are then written again,
    /// after the tick's own answers.
    fn close(
        &mut self,
        roots: &[PathBuf],
        health: TickHealth,
        refreshes: Refreshes,
        effects: &mut impl Effects,
        now_ms: i64,
        log_root: &Path,
    ) -> Result<(), String> {
        self.finish_refreshes(refreshes.done, &refreshes.failed, now_ms);
        self.clients.retain(|root, _| roots.contains(root));
        self.times.retain(|root, _| roots.contains(root));
        health.commit(&mut self.book, effects, log_root)?;
        for (root, asked_ms) in self.retried.take().unwrap_or_default() {
            self.request_retry(&root, asked_ms, effects)?;
        }
        Ok(())
    }
}

impl Tick {
    /// Asks each due checkout's branch for its pull request, from where the
    /// last tick stopped and for as many checkouts as the budget has whole
    /// subjects for. What it reads of the observer, it reads in a statement
    /// of its own.
    fn discover(
        &mut self,
        observer: &Mutex<Observer>,
        roots: &[PathBuf],
        budget: &mut Budget,
        limits: &Limits,
    ) -> Discovered {
        let mut settled = Vec::new();
        let mut found = Vec::new();
        let mut visited = 0;
        let mut reserved = 0;
        // Reserve complete subjects before discovery so the first root cannot
        // consume the last root's CI allocation. Rotate overload on the next tick.
        // A root that asks `gh` nothing reserves nothing.
        let capacity = (limits.gh_calls_max / SUBJECT_CALLS_MAX).max(1);
        for root in roots.iter().cycle().skip(self.rotate).take(roots.len()) {
            if reserved == capacity {
                break;
            }
            visited += 1;
            let Ok(branch) = checkout_token(root) else {
                continue;
            };
            if !on_a_branch(&branch) {
                settled.push(root.clone());
                continue;
            }
            if !self.health.due(root, Operation::Discovery, &branch) {
                continue;
            }
            let stopped = hold(observer).book.subjects.values().any(|t| {
                t.subject.root == root.to_string_lossy()
                    && t.subject.branch == branch
                    && t.record.stopped
            });
            if stopped {
                settled.push(root.clone());
                continue;
            }
            reserved += 1;
            let client = hold(observer)
                .clients
                .get(root)
                .cloned()
                .unwrap_or_default();
            let discovery = client.discover(root, budget);
            if !matches!(discovery, Err(gh::GhError::Budget)) {
                settled.push(root.clone());
            }
            self.health
                .observe(root, Operation::Discovery, &branch, &discovery);
            match discovery {
                Ok(Some(review)) => {
                    let discovered = subject(root, &review, branch);
                    found.push(Found {
                        root: root.clone(),
                        review,
                        subject: discovered,
                        client,
                    });
                }
                Ok(None) => {
                    for operation in [
                        Operation::Checks,
                        Operation::Details,
                        Operation::Reviews,
                        Operation::Stack,
                    ] {
                        self.health.forget(root, operation);
                    }
                }
                Err(_) => {}
            }
        }
        Discovered {
            next_root: (self.rotate + visited) % roots.len(),
            settled,
            found,
        }
    }

    /// Reads one PR's facts: its CI and, when the book says a failure will be
    /// mailed, the failure's detail; its reviews when they are due; and its
    /// stack when it conflicts. What it reads of the observer, it reads in a
    /// statement of its own.
    fn refresh(
        &mut self,
        observer: &Mutex<Observer>,
        found: Found,
        budget: &mut Budget,
        limits: &Limits,
        now_ms: i64,
    ) -> Refreshed {
        let Found {
            root,
            review,
            subject,
            client: mut candidate,
        } = found;
        let key = subject.key();
        let times = hold(observer).times.get(&root).copied();
        let force = due(times.and_then(|t| t.force), now_ms, limits.force_ms);
        let reviews_due = due(times.and_then(|t| t.reviews), now_ms, limits.review_ms);
        let health = &mut self.health;
        let mut observed = Observation {
            state: read_state(&review.state),
            ..Observation::default()
        };
        let mut complete = true;
        if observed.state == Some(PrState::Open) {
            // Fact families are independent: an unreadable review must not hide CI.
            let checks = if health.due(&root, Operation::Checks, &review.head_sha) {
                let checks = candidate.checks(&root, &review, force, budget);
                health.observe(&root, Operation::Checks, &review.head_sha, &checks);
                Some(checks)
            } else {
                None
            };
            match checks {
                Some(Ok(mut ci)) => {
                    let needs_detail = hold(observer).book.subjects.get(&key).is_some_and(|held| {
                        core::react(
                            &held.record,
                            &Observation {
                                ci: Some(ci.clone()),
                                ..Observation::default()
                            },
                            limits,
                        )
                        .iter()
                        .any(|r| r.body.is_some())
                    });
                    if needs_detail {
                        if health.due(&root, Operation::Details, &review.head_sha) {
                            let detail = candidate.detail(&root, &review, &mut ci, force, budget);
                            health.observe(&root, Operation::Details, &review.head_sha, &detail);
                            if detail.is_ok() {
                                observed.ci = Some(ci);
                            } else {
                                complete = false;
                            }
                        } else {
                            complete = false;
                        }
                    } else {
                        health.forget(&root, Operation::Details);
                        observed.ci = Some(ci);
                    }
                }
                Some(Err(_)) | None => {
                    complete = false;
                }
            }
            if reviews_due || force {
                if health.due(&root, Operation::Reviews, &review.head_sha) {
                    let reviews = candidate.reviews(&root, &review, budget);
                    health.observe(&root, Operation::Reviews, &review.head_sha, &reviews);
                    match reviews {
                        Ok(reviews) => observed.reviews = Some(reviews),
                        Err(_) => complete = false,
                    }
                } else {
                    complete = false;
                }
            }
            observed.mergeable = read_mergeability(review.mergeable.as_deref());
            if observed.mergeable == Some(core::Mergeability::Conflicting) {
                if health.due(&root, Operation::Stack, &review.head_sha) {
                    let stack =
                        candidate.stack(&root, &review, limits.stack_prs_max, force, budget);
                    health.observe(&root, Operation::Stack, &review.head_sha, &stack);
                    match stack {
                        Ok(stacked) => observed.stacked = Some(stacked),
                        Err(_) => complete = false,
                    }
                } else {
                    complete = false;
                }
            } else {
                health.forget(&root, Operation::Stack);
            }
        } else {
            for operation in [
                Operation::Checks,
                Operation::Details,
                Operation::Reviews,
                Operation::Stack,
            ] {
                health.forget(&root, operation);
            }
        }
        let reviewed = observed.reviews.is_some();
        Refreshed {
            key,
            observed,
            complete,
            update: CacheUpdate {
                root,
                repo: review.owner_repo,
                client: candidate,
                force,
                reviewed,
            },
        }
    }
}

/// Panel and background observations use this same durable book and delivery receipt.
pub(crate) fn note_checks(
    app: &AppHandle,
    root: &Path,
    review: &gh::HostedReview,
    ci: &Ci,
    limits: &Limits,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    let roots = live_roots(&state);
    if !roots.iter().any(|live| live == root) || review.state != "open" {
        return Ok(());
    }
    let path = state_path(&state);
    let mut held = hold(checks_runtime::remembered());
    held.load(&path)?;
    let subject = subject(root, review, checkout_token(root)?);
    let key = subject.key();
    // A panel request started on an older head cannot overwrite the background's newer head.
    if held
        .book
        .subjects
        .get(&key)
        .is_some_and(|t| t.subject.head != subject.head)
    {
        return Ok(());
    }
    let mut effects = WindowEffects {
        path: &path,
        now_ms: crate::now_epoch_ms(),
    };
    held.book.discover(&[subject], &mut effects)?;
    held.book.apply(
        &key,
        &Observation {
            ci: Some(ci.clone()),
            ..Observation::default()
        },
        limits,
        effects.now_ms,
        &mut effects,
    )?;
    publish_notices(app, &held.book, &roots);
    Ok(())
}

#[derive(Clone, Serialize)]
struct NoticeRow<'a> {
    id: String,
    root: &'a str,
    /// The checkout's folder name: the words say this, and the full path is the tooltip.
    checkout: String,
    number: u64,
    kind: FactKind,
    opened_at: i64,
    resolved_at: Option<i64>,
    shown_at: Option<i64>,
    dismissed_at: Option<i64>,
}
fn notice_rows<'a>(book: &'a Book, roots: &[PathBuf]) -> Vec<NoticeRow<'a>> {
    book.subjects
        .iter()
        .filter(|(_, held)| {
            roots
                .iter()
                .any(|root| root == Path::new(&held.subject.root))
        })
        .flat_map(|(key, held)| {
            held.record
                .notices
                .iter()
                .map(move |(kind, notice)| NoticeRow {
                    id: core::notice_id(key, *kind),
                    root: &held.subject.root,
                    checkout: checkout_name(Path::new(&held.subject.root)),
                    number: held.subject.number,
                    kind: *kind,
                    opened_at: notice.opened_at,
                    resolved_at: notice.resolved_at,
                    shown_at: notice.shown_at,
                    dismissed_at: notice.dismissed_at,
                })
        })
        .collect()
}
fn checkout_name(root: &Path) -> String {
    root.file_name()
        .unwrap_or(root.as_os_str())
        .to_string_lossy()
        .into_owned()
}
fn publish_notices(app: &AppHandle, book: &Book, roots: &[PathBuf]) {
    let _ = app.emit("scm:notices", notice_rows(book, roots));
    let _ = app.emit("scm:health", book.health.rows());
}

#[derive(Serialize)]
pub(crate) struct HealthSnapshot {
    root: String,
    rows: Vec<core::HealthRow>,
}

pub(crate) fn health_snapshot(state: &AppState) -> Result<HealthSnapshot, String> {
    let root = state.active().root.to_string_lossy().into_owned();
    health_rows(checks_runtime::remembered(), &state_path(state), root)
}

/// The health rows of the checkout at `root`, as `observer`'s book has them.
fn health_rows(
    observer: &Mutex<Observer>,
    path: &Path,
    root: String,
) -> Result<HealthSnapshot, String> {
    let mut held = hold(observer);
    held.load(path)?;
    Ok(HealthSnapshot {
        rows: held
            .book
            .health
            .rows()
            .into_iter()
            .filter(|row| row.root == root)
            .collect(),
        root,
    })
}

pub(crate) fn retry(app: &AppHandle, root: &Path) -> Result<HealthSnapshot, String> {
    let state = app.state::<AppState>();
    let here = state.active();
    if here.root != root || here.orchestrator.is_none() {
        return Err("the selected checkout changed or is not a repository".into());
    }
    let now_ms = crate::now_epoch_ms();
    let path = state_path(&state);
    {
        let mut held = hold(checks_runtime::remembered());
        held.load(&path)?;
        held.request_retry(
            root,
            now_ms,
            &mut WindowEffects {
                path: &path,
                now_ms,
            },
        )?;
    }
    NEXT.store(0, Ordering::Release);
    sweep(app, now_ms);
    health_snapshot(&state)
}

#[cfg(test)]
mod tests {
    use super::*;
    mod health;
    #[test]
    fn observer_cadences_are_independent_and_failures_do_not_spend_them() {
        let limits = Limits::default();
        assert!(due(None, 0, limits.observe_ms));
        assert!(!due(Some(0), 29_999, limits.observe_ms));
        assert!(due(Some(0), 30_000, limits.observe_ms));
        assert!(!due(Some(0), 30_000, limits.review_ms));
        assert!(due(Some(0), 120_000, limits.review_ms));
        assert!(due(Some(0), 300_000, limits.force_ms));
    }
    #[test]
    fn corrupt_durable_state_is_a_failure_not_a_fresh_observer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        std::fs::write(&path, b"broken").unwrap();
        let mut observer = Observer::default();
        assert!(observer.load(&path).is_err());
        assert!(observer.loaded.is_none());
    }
    #[test]
    fn one_failed_sibling_pins_the_repository_cache_and_review_clock() {
        let mut held = Observer::default();
        let update = || CacheUpdate {
            root: PathBuf::from("/wt/a"),
            repo: "o/r".into(),
            client: Client::default(),
            force: true,
            reviewed: true,
        };
        held.finish_refreshes(vec![update()], &BTreeSet::from(["o/r".into()]), 30_000);
        assert!(held.clients.is_empty());
        assert!(held.times.is_empty());
        held.finish_refreshes(vec![update()], &BTreeSet::new(), 60_000);
        assert_eq!(held.times[Path::new("/wt/a")].reviews, Some(60_000));
    }

    use crate::vendor_cli::{
        CliCall, CliError, CliOutput, CliRunner, FakeAnswer, FakeCall, FakeRunner, VendorCli,
    };
    use std::sync::{Condvar, mpsc};
    use std::time::{Duration, Instant};
    use zerocode_core::host::Host;

    /// A git checkout `name` under `parent` with one commit: on `branch`, or
    /// detached from that commit when there is none.
    fn checkout(parent: &Path, name: &str, branch: Option<&str>) -> PathBuf {
        let root = parent.join(name);
        std::fs::create_dir_all(&root).expect("a checkout directory");
        let git = |args: &[&str]| {
            Host::for_workspace(&root)
                .vcs()
                .text(&root, args)
                .unwrap_or_else(|error| panic!("git {args:?}: {error}"))
        };
        git(&["init", "-q", "-b", branch.unwrap_or("main")]);
        git(&[
            "-c",
            "user.name=ZeroCode Test",
            "-c",
            "user.email=test@zerocode",
            "commit",
            "--allow-empty",
            "-q",
            "-m",
            "first",
        ]);
        if branch.is_none() {
            git(&["checkout", "-q", "--detach"]);
        }
        root
    }

    fn checkout_name(call: &FakeCall) -> Option<&str> {
        call.cwd.as_deref()?.file_name()?.to_str()
    }

    /// `gh pr view --json …` for open PR `number` of `o/r`, mergeable.
    fn pull_request(number: u64) -> String {
        serde_json::json!({
            "number": number,
            "title": "fixture",
            "url": format!("https://github.com/o/r/pull/{number}"),
            "state": "OPEN",
            "isDraft": false,
            "headRefOid": format!("head-{number}"),
            "headRepository": {"name": "r"},
            "headRepositoryOwner": {"login": "o"},
            "mergeable": "MERGEABLE",
            "mergeStateStatus": "CLEAN",
            "baseRefName": "main"
        })
        .to_string()
    }

    /// GitHub as the fixture has it: checkouts `topic` and `flaky` are on open
    /// PRs #7 and #8 of `o/r` with one green check and no reviews, an ETag'd
    /// REST read is unchanged (304, which `gh` exits nonzero on), and a checkout
    /// on no branch gets `gh pr view`'s own refusal (wording measured
    /// 2026-09-17 on a detached checkout).
    fn github(call: &FakeCall) -> Result<CliOutput, CliError> {
        let answer = |success: bool, stdout: String, stderr: &str| {
            Ok(CliOutput {
                success,
                stdout,
                stderr: stderr.into(),
            })
        };
        let args: Vec<&str> = call.args.iter().map(String::as_str).collect();
        match args.as_slice() {
            ["pr", "view", ..] => match checkout_name(call) {
                Some("topic") => answer(true, pull_request(7), ""),
                Some("flaky") => answer(true, pull_request(8), ""),
                _ => answer(
                    false,
                    String::new(),
                    "could not determine current branch: failed to run git: not on any branch\n",
                ),
            },
            ["api", "graphql", ..] => answer(
                true,
                serde_json::json!({"data": {"repository": {"pullRequest": {
                    "reviews": {"nodes": [], "pageInfo": {"hasNextPage": false}},
                    "reviewThreads": {"nodes": [], "pageInfo": {"hasNextPage": false}}
                }}}})
                .to_string(),
                "",
            ),
            ["api", "--include", path, conditional @ ..] => {
                if conditional
                    .iter()
                    .any(|arg| arg.starts_with("If-None-Match:"))
                {
                    return answer(false, "HTTP/2.0 304 Not Modified\r\n\r\n".into(), "");
                }
                let rows = if path.contains("/check-runs?") {
                    serde_json::json!({"total_count": 1, "check_runs": [
                        {"id": 1, "name": "ci/test", "status": "completed", "conclusion": "success"}
                    ]})
                } else if path.contains("/status?") {
                    serde_json::json!({"total_count": 0, "statuses": []})
                } else if path.contains("/check-suites?") {
                    serde_json::json!({"total_count": 0, "check_suites": []})
                } else {
                    panic!("the fixture has no REST answer for {path}")
                };
                answer(
                    true,
                    format!("HTTP/2.0 200 OK\r\nETag: W/\"{path}\"\r\n\r\n{rows}"),
                    "",
                )
            }
            _ => panic!("the fixture has no gh answer for {args:?}"),
        }
    }

    /// The same GitHub, except checkout `flaky`'s discovery never comes back:
    /// the refusal of a call that ran past its budget.
    fn github_losing_flaky(call: &FakeCall) -> Result<CliOutput, CliError> {
        if call.args.first().is_some_and(|verb| verb == "pr")
            && checkout_name(call) == Some("flaky")
        {
            return Err(CliError::TimedOut);
        }
        github(call)
    }

    /// Every durable write and ledger letter accepted.
    struct Accepted;
    impl Effects for Accepted {
        fn save(&mut self, _: &Book) -> Result<(), String> {
            Ok(())
        }
        fn mail(&mut self, _: &Subject, _: &str, _: &str) -> Result<(), String> {
            Ok(())
        }
    }

    /// One observer tick against `github`, run the way the window runs it;
    /// the `gh` calls it made.
    fn tick(
        held: &mut Observer,
        roots: &[PathBuf],
        github: FakeAnswer,
        limits: &Limits,
        now_ms: i64,
        log_root: &Path,
    ) -> Vec<FakeCall> {
        let runner = FakeRunner::answering(github);
        let mut budget = Budget::over(&runner, limits);
        let observer = Mutex::new(std::mem::take(held));
        observe(
            &observer,
            roots,
            &mut budget,
            &mut Accepted,
            limits,
            now_ms,
            log_root,
        )
        .expect("an observer tick");
        *held = observer
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(budget.used, runner.calls().len());
        runner.calls()
    }

    /// `gh` as [`github`] answers it, except that no call comes back until the
    /// test lets GitHub answer: GitHub at its slowest, for as long as a test
    /// needs it to be.
    struct SlowGitHub {
        github: FakeRunner,
        /// The calls that came in, and whether GitHub answers yet.
        gate: Mutex<(usize, bool)>,
        moved: Condvar,
    }
    impl SlowGitHub {
        fn new() -> Self {
            Self::answering(github)
        }
        /// The same slow GitHub, answering as `answer` does once it answers.
        fn answering(answer: FakeAnswer) -> Self {
            Self {
                github: FakeRunner::answering(answer),
                gate: Mutex::new((0, false)),
                moved: Condvar::new(),
            }
        }
        /// Whether a call is out within `limit`.
        fn asked_within(&self, limit: Duration) -> bool {
            let gate = self.gate.lock().expect("the gate");
            let (gate, _) = self
                .moved
                .wait_timeout_while(gate, limit, |(calls, _)| *calls == 0)
                .expect("the gate");
            gate.0 > 0
        }
        fn answer(&self) {
            self.gate.lock().expect("the gate").1 = true;
            self.moved.notify_all();
        }
    }
    impl CliRunner for SlowGitHub {
        fn run(&self, cli: VendorCli, call: &CliCall<'_>) -> Result<CliOutput, CliError> {
            {
                let mut gate = self.gate.lock().expect("the gate");
                gate.0 += 1;
                self.moved.notify_all();
                let _answering = self
                    .moved
                    .wait_while(gate, |(_, answering)| !*answering)
                    .expect("the gate");
            }
            self.github.run(cli, call)
        }
    }
    /// GitHub answers once this goes, however the test ends: a tick still
    /// waiting on it would keep the test's threads from ever joining.
    struct AnswerOnDrop<'a>(&'a SlowGitHub);
    impl Drop for AnswerOnDrop<'_> {
        fn drop(&mut self) {
            self.0.answer();
        }
    }

    /// The Checks pane reads the observer's health while a tick is out asking
    /// GitHub, and the read comes back without waiting for GitHub (t-47457).
    /// A tick that held the observer for every `gh` call kept the read
    /// waiting through all of them: up to `gh_calls_max` calls of
    /// `gh_call_ms` each, 32 × 10 s = 320 s with the table's numbers. Here
    /// GitHub answers nothing until the read is back or `READ_WAIT_MAX` has
    /// passed.
    #[test]
    fn the_health_read_comes_back_while_a_tick_waits_on_gh() {
        /// Well over what the read takes when nothing holds the observer
        /// (under a millisecond): room for a loaded test machine.
        const READ_WAIT_MAX: Duration = Duration::from_secs(1);
        let dir = tempfile::tempdir().expect("checkouts");
        let topic = checkout(dir.path(), "topic", Some("topic"));
        let path = dir.path().join("scm-observer.json");
        let observer = Mutex::new(Observer::default());
        hold(&observer).load(&path).expect("the observer's book");
        let limits = Limits::default();
        let slow = SlowGitHub::new();
        let (read, waited, ticked) = std::thread::scope(|scope| {
            let answering = AnswerOnDrop(&slow);
            let ticking = scope.spawn(|| {
                let mut budget = Budget::over(&slow, &limits);
                observe(
                    &observer,
                    std::slice::from_ref(&topic),
                    &mut budget,
                    &mut Accepted,
                    &limits,
                    1_000_000,
                    dir.path(),
                )
            });
            assert!(
                slow.asked_within(Duration::from_secs(60)),
                "the tick never asked gh anything"
            );
            let (sent, came) = mpsc::channel();
            let (observer, path) = (&observer, &path);
            let root = topic.to_string_lossy().into_owned();
            let started = Instant::now();
            let _reading = scope.spawn(move || sent.send(health_rows(observer, path, root)));
            let read = came.recv_timeout(READ_WAIT_MAX);
            let waited = started.elapsed();
            drop(answering);
            (read, waited, ticking.join().expect("the tick's thread"))
        });
        let held_for_ms = u64::try_from(limits.gh_calls_max)
            .unwrap_or(u64::MAX)
            .saturating_mul(limits.gh_call_ms);
        println!(
            "SCM health read while a tick waits on gh: waited_ms={:.3} (a tick that holds \
             the observer through its calls: up to {} calls x {} ms = {held_for_ms} ms)",
            waited.as_secs_f64() * 1000.0,
            limits.gh_calls_max,
            limits.gh_call_ms,
        );
        let snapshot = read
            .unwrap_or_else(|_| {
                panic!(
                    "the health read waited {waited:?}, past {READ_WAIT_MAX:?}, for a tick \
                     that was waiting on gh"
                )
            })
            .expect("a health snapshot");
        assert_eq!(snapshot.root, topic.to_string_lossy());
        ticked.expect("the tick, once GitHub answered");
        assert_eq!(
            slow.github.calls().len(),
            5,
            "once GitHub answered, the tick asked what a first tick asks: discovery, three \
             CI reads and the reviews"
        );
        assert!(
            hold(&observer)
                .book
                .subjects
                .values()
                .any(|tracked| tracked.subject.number == 7),
            "the tick did not write down the PR GitHub named"
        );
    }

    /// A person asks to retry a checkout while a tick is out asking GitHub
    /// about it, and the tick's own answer — another failed discovery, with
    /// its longer backoff — does not undo the request: when the tick closes,
    /// the checkout is still due at the time of the request and still on the
    /// retry list, as when the request had to wait for the tick (t-47457).
    #[test]
    fn a_retry_asked_for_during_a_tick_outlives_the_tick() {
        /// Room for a loaded test machine; the request takes well under a
        /// millisecond when nothing holds the observer.
        const ASK_WAIT_MAX: Duration = Duration::from_secs(1);
        let dir = tempfile::tempdir().expect("checkouts");
        let flaky = checkout(dir.path(), "flaky", Some("flaky"));
        let topic = checkout(dir.path(), "topic", Some("topic"));
        let roots = [flaky.clone(), topic];
        let limits = Limits::default();
        let first_ms = 1_000_000;
        // flaky's discovery failed once: it is due again after the first backoff, 30 s.
        let next_ms = first_ms + 30_000;
        let asked_ms = next_ms + 5_000;
        let mut held = Observer::default();
        tick(
            &mut held,
            &roots,
            github_losing_flaky,
            &limits,
            first_ms,
            dir.path(),
        );
        let observer = Mutex::new(held);
        let slow = SlowGitHub::answering(github_losing_flaky);
        let (asked, ticked) = std::thread::scope(|scope| {
            let answering = AnswerOnDrop(&slow);
            let ticking = scope.spawn(|| {
                let mut budget = Budget::over(&slow, &limits);
                observe(
                    &observer,
                    &roots,
                    &mut budget,
                    &mut Accepted,
                    &limits,
                    next_ms,
                    dir.path(),
                )
            });
            assert!(
                slow.asked_within(Duration::from_secs(60)),
                "the tick never asked gh anything"
            );
            let (sent, came) = mpsc::channel();
            let (observer, flaky) = (&observer, &flaky);
            let _asking = scope.spawn(move || {
                sent.send(hold(observer).request_retry(flaky, asked_ms, &mut Accepted))
            });
            let asked = came.recv_timeout(ASK_WAIT_MAX);
            drop(answering);
            (asked, ticking.join().expect("the tick's thread"))
        });
        asked
            .expect("the retry waited for a tick that was waiting on gh")
            .expect("the retry");
        ticked.expect("the tick, once GitHub answered");
        let held = hold(&observer);
        assert_eq!(
            slow.github
                .calls()
                .first()
                .and_then(|call| call.cwd.clone()),
            Some(flaky.clone()),
            "the tick's first gh call was not flaky's discovery"
        );
        assert!(
            held.retry_roots.contains(&flaky),
            "the tick's close took flaky off the retry list it was put back on"
        );
        assert!(
            held.book.health.due(
                &flaky.to_string_lossy(),
                Operation::Discovery,
                &checkout_token(&flaky).expect("flaky's HEAD"),
                asked_ms
            ),
            "the tick's failed discovery overwrote the retry asked for while it ran"
        );
        assert!(held.retried.is_none());
    }

    fn logged_discovery_refusals(log_root: &Path) -> usize {
        std::fs::read_to_string(log_root.join("window-errors.log"))
            .unwrap_or_default()
            .matches("scm-observer: discovery fetch timeout")
            .count()
    }

    fn is_graphql(call: &FakeCall) -> bool {
        call.args.get(1).is_some_and(|arg| arg == "graphql")
    }

    /// A detached checkout (a coordinator's merge worktree) beside a checkout
    /// on an open PR: the detached one never reaches `gh`, and the PR's
    /// refresh clocks and ETags survive into the next tick — which then
    /// asks only whether anything changed.
    #[test]
    fn a_detached_checkout_asks_gh_nothing_and_holds_back_no_refresh() {
        let dir = tempfile::tempdir().expect("checkouts");
        let detached = checkout(dir.path(), "detached", None);
        let topic = checkout(dir.path(), "topic", Some("topic"));
        let roots = [detached.clone(), topic];
        let limits = Limits::default();
        let first_ms = 1_000_000;
        let next_ms = first_ms + i64::try_from(limits.observe_ms).expect("observe_ms");
        let mut held = Observer::default();
        let first = tick(&mut held, &roots, github, &limits, first_ms, dir.path());
        let next = tick(&mut held, &roots, github, &limits, next_ms, dir.path());
        let per_tick = [first.len(), next.len()];
        println!("SCM detached fixture: gh calls per tick {per_tick:?}");
        assert_eq!(
            per_tick,
            [5, 4],
            "gh calls per tick: the PR's discovery, three CI reads and its reviews, then \
             discovery and three conditional CI reads"
        );
        assert!(
            first
                .iter()
                .chain(&next)
                .all(|call| call.cwd.as_deref() != Some(detached.as_path())),
            "a checkout on no branch was asked for its pull request"
        );
        assert!(
            next.iter()
                .filter(|call| call.args.get(1).is_some_and(|arg| arg == "--include"))
                .all(|call| call
                    .args
                    .iter()
                    .any(|arg| arg.starts_with("If-None-Match:"))),
            "the second tick fetched CI unconditionally again"
        );
        assert!(
            !next.iter().any(is_graphql),
            "the second tick read reviews again before review_ms"
        );
        assert_eq!(logged_discovery_refusals(dir.path()), 0);
    }

    /// A discovery that really fails (here: past its budget) is its checkout's
    /// alone. The sibling on the same repository keeps the refresh it just made,
    /// the failed checkout's subject stays exactly as it was — nothing reads a
    /// missing answer as a closed PR — and it is asked again the next tick.
    #[test]
    fn a_failed_discovery_holds_back_only_its_own_checkout_and_closes_nothing() {
        let dir = tempfile::tempdir().expect("checkouts");
        let flaky = checkout(dir.path(), "flaky", Some("flaky"));
        let topic = checkout(dir.path(), "topic", Some("topic"));
        let roots = [flaky.clone(), topic.clone()];
        let limits = Limits::default();
        let first_ms = 1_000_000;
        let reviews_ms = first_ms + i64::try_from(limits.review_ms).expect("review_ms");
        let next_ms = reviews_ms + i64::try_from(limits.observe_ms).expect("observe_ms");
        let mut held = Observer::default();
        assert_eq!(
            tick(&mut held, &roots, github, &limits, first_ms, dir.path()).len(),
            10
        );
        let flaky_subject = |held: &Observer| {
            held.book
                .subjects
                .values()
                .find(|tracked| Path::new(&tracked.subject.root) == flaky)
                .cloned()
                .expect("flaky's PR was discovered")
        };
        let flaky_before = flaky_subject(&held);
        let lost = tick(
            &mut held,
            &roots,
            github_losing_flaky,
            &limits,
            reviews_ms,
            dir.path(),
        );
        assert_eq!(
            lost.len(),
            6,
            "flaky's failed discovery, then topic's discovery, three CI reads and its due reviews"
        );
        assert_eq!(logged_discovery_refusals(dir.path()), 1);
        assert_eq!(
            flaky_subject(&held),
            flaky_before,
            "a failed discovery changed the subject it could not see"
        );
        assert!(!flaky_before.record.stopped);
        assert_eq!(held.times[&flaky].reviews, Some(first_ms));
        assert_eq!(
            held.times[&topic].reviews,
            Some(reviews_ms),
            "the sibling's review refresh was thrown away with the failed discovery"
        );
        let again = tick(&mut held, &roots, github, &limits, next_ms, dir.path());
        assert!(
            again
                .iter()
                .any(|call| call.cwd.as_deref() == Some(flaky.as_path()) && is_graphql(call)),
            "the checkout whose discovery failed was not observed again"
        );
        assert_eq!(
            again.len(),
            9,
            "flaky: discovery, three CI reads, due reviews; topic: discovery and three CI reads"
        );
        assert_eq!(held.times[&flaky].reviews, Some(next_ms));
    }

    /// The subject slots are reserved for checkouts that spend `gh` calls: a
    /// detached checkout ahead in the rotation must not take the only one.
    #[test]
    fn a_checkout_that_asks_nothing_takes_no_subject_slot() {
        let dir = tempfile::tempdir().expect("checkouts");
        let roots = [
            checkout(dir.path(), "detached", None),
            checkout(dir.path(), "topic", Some("topic")),
        ];
        let limits = Limits {
            gh_calls_max: SUBJECT_CALLS_MAX,
            ..Limits::default()
        };
        let mut held = Observer::default();
        let calls = tick(&mut held, &roots, github, &limits, 1_000_000, dir.path());
        assert_eq!(
            calls.len(),
            5,
            "the one subject slot went to the detached checkout instead of the PR"
        );
    }

    #[test]
    fn a_mark_is_written_to_the_ledger_file_and_the_row_carries_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scm-observer.json");
        let subject = Subject {
            root: "/wt/topic".into(),
            repo: "o/r".into(),
            number: 4,
            url: "https://github.com/o/r/pull/4".into(),
            head: "a".into(),
            branch: "topic".into(),
        };
        let key = subject.key();
        let mut book = Book::default();
        book.subjects.insert(
            key.clone(),
            core::Tracked {
                subject,
                record: core::Record::default(),
            },
        );
        book.subjects.get_mut(&key).unwrap().record.notices.insert(
            FactKind::Ci,
            core::Notice {
                opened_at: 10,
                resolved_at: None,
                shown_at: None,
                dismissed_at: None,
            },
        );
        std::fs::write(&path, serde_json::to_vec(&book).unwrap()).unwrap();
        let id = core::notice_id(&key, FactKind::Ci);
        let mut held = Observer::default();
        assert!(
            held.mark_notice(&path, &id, 10, core::NoticeMark::Dismissed, 20)
                .unwrap()
        );
        let saved: Book = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let rows = notice_rows(&saved, &[PathBuf::from("/wt/topic")]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].checkout, "topic");
        assert_eq!(rows[0].root, "/wt/topic");
        assert_eq!((rows[0].shown_at, rows[0].dismissed_at), (None, Some(20)));
    }
}
