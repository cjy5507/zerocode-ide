//! The window's independent 30-second PR feedback loop.
use crate::{AppState, ShellStateExt, checks_runtime, gh};
use gh::observer::{Budget, Client, SUBJECT_CALLS_MAX, read_mergeability, read_state};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use tauri::{AppHandle, Emitter, Manager};
use zerocode_core::checks::Limits;
use zerocode_core::scm_observer::{
    self as core, Book, Ci, Effects, FactKind, Observation, PrState, Subject,
};

#[derive(Default)]
pub(crate) struct Observer {
    loaded: Option<PathBuf>,
    book: Book,
    clients: BTreeMap<PathBuf, Client>,
    times: BTreeMap<PathBuf, Times>,
    next_root: usize,
}
#[derive(Default)]
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
    let output = crate::proc::quiet_command("git")
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
    let roots = live_roots(&state);
    let path = state_path(&state);
    let mut held = checks_runtime::remembered()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    held.load(&path)?;
    if roots.is_empty() {
        publish_notices(app, &held.book, &roots);
        return Ok(());
    }
    let mut budget = Budget::new(limits);
    held.observe(
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
    publish_notices(app, &held.book, &roots);
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
impl Observer {
    /// One tick over the live checkouts, apart from the window: the caller
    /// brings the roots, the tick's `gh` budget, the durable effects and the
    /// directory of the event log.
    fn observe(
        &mut self,
        roots: &[PathBuf],
        budget: &mut Budget,
        effects: &mut impl Effects,
        limits: &Limits,
        now_ms: i64,
        log_root: &Path,
    ) -> Result<(), String> {
        let Some(rotate) = self.next_root.checked_rem(roots.len()) else {
            return Ok(());
        };
        let mut candidates = Vec::new();
        let mut visited = 0;
        let mut reserved = 0;
        // Reserve complete subjects before discovery so the first root cannot
        // consume the last root's CI allocation. Rotate overload on the next tick.
        // A root that asks `gh` nothing reserves nothing.
        let capacity = (limits.gh_calls_max / SUBJECT_CALLS_MAX).max(1);
        for root in roots.iter().cycle().skip(rotate).take(roots.len()) {
            if reserved == capacity {
                break;
            }
            visited += 1;
            let Ok(branch) = checkout_token(root) else {
                continue;
            };
            if !on_a_branch(&branch) {
                continue;
            }
            if self.book.subjects.values().any(|t| {
                t.subject.root == root.to_string_lossy()
                    && t.subject.branch == branch
                    && t.record.stopped
            }) {
                continue;
            }
            reserved += 1;
            let client = self.clients.get(root).cloned().unwrap_or_default();
            match client.discover(root, budget) {
                Ok(Some(review)) => {
                    let discovered = subject(root, &review, branch);
                    if review.state == "open" || self.book.subjects.contains_key(&discovered.key())
                    {
                        candidates.push((root.clone(), review, discovered, client));
                    }
                }
                Ok(None) => {}
                // This checkout's alone: it is asked again next tick, and its
                // subject stays as it was — the book never reads an unanswered
                // checkout as a closed PR. Discovery reads no cache and moves
                // no cursor, so no other checkout's refresh is held back for it.
                Err(error) => log_fetch(log_root, "discovery", &error),
            }
        }
        self.next_root = (rotate + visited) % roots.len();
        self.book.discover(
            &candidates
                .iter()
                .map(|(_, _, subject, _)| subject.clone())
                .collect::<Vec<_>>(),
            effects,
        )?;
        let mut cache_updates = Vec::new();
        let mut failed_repos = BTreeSet::new();
        for (root, review, subject, mut candidate) in candidates {
            let key = subject.key();
            let times = self.times.get(&root);
            let force = due(times.and_then(|t| t.force), now_ms, limits.force_ms);
            let reviews_due = due(times.and_then(|t| t.reviews), now_ms, limits.review_ms);
            let mut observed = Observation {
                state: read_state(&review.state),
                ..Observation::default()
            };
            let mut complete = true;
            if observed.state == Some(PrState::Open) {
                // Fact families are independent: an unreadable review must not hide CI.
                match candidate.checks(&root, &review, force, budget) {
                    Ok(mut ci) => {
                        let needs_detail = self.book.subjects.get(&key).is_some_and(|held| {
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
                        if needs_detail
                            && let Err(error) =
                                candidate.detail(&root, &review, &mut ci, force, budget)
                        {
                            complete = false;
                            log_fetch(log_root, "CI details", &error);
                        } else {
                            observed.ci = Some(ci);
                        }
                    }
                    Err(error) => {
                        complete = false;
                        log_fetch(log_root, "CI", &error);
                    }
                }
                if reviews_due || force {
                    match candidate.reviews(&root, &review, budget) {
                        Ok(reviews) => observed.reviews = Some(reviews),
                        Err(error) => {
                            complete = false;
                            log_fetch(log_root, "reviews", &error);
                        }
                    }
                }
                observed.mergeable = read_mergeability(review.mergeable.as_deref());
                if observed.mergeable == Some(core::Mergeability::Conflicting) {
                    match candidate.stack(&root, &review, limits.stack_prs_max, force, budget) {
                        Ok(stacked) => observed.stacked = Some(stacked),
                        Err(error) => {
                            complete = false;
                            log_fetch(log_root, "stack", &error);
                        }
                    }
                }
            }
            if let Err(error) = self.book.apply(&key, &observed, limits, now_ms, effects) {
                complete = false;
                crate::note_window_event(log_root, &format!("scm-observer: {error}"));
            }
            if complete {
                cache_updates.push(CacheUpdate {
                    root,
                    repo: review.owner_repo.clone(),
                    client: candidate,
                    force,
                    reviewed: observed.reviews.is_some(),
                });
            } else {
                failed_repos.insert(review.owner_repo.clone());
            }
        }
        self.finish_refreshes(cache_updates, &failed_repos, now_ms);
        self.clients.retain(|root, _| roots.contains(root));
        self.times.retain(|root, _| roots.contains(root));
        Ok(())
    }
}
fn log_fetch(log_root: &Path, kind: &str, error: &gh::GhError) {
    crate::note_window_event(
        log_root,
        &format!("scm-observer: {kind} fetch {}", error.reason()),
    );
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
    let mut held = checks_runtime::remembered()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
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
    number: u64,
    kind: FactKind,
    opened_at: i64,
    resolved_at: Option<i64>,
}
fn publish_notices(app: &AppHandle, book: &Book, roots: &[PathBuf]) {
    let rows: Vec<_> = book
        .subjects
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
                    id: format!("{key}:{kind:?}"),
                    root: &held.subject.root,
                    number: held.subject.number,
                    kind: *kind,
                    opened_at: notice.opened_at,
                    resolved_at: notice.resolved_at,
                })
        })
        .collect();
    let _ = app.emit("scm:notices", rows);
}

#[cfg(test)]
mod tests {
    use super::*;
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

    use crate::vendor_cli::{CliError, CliOutput, FakeAnswer, FakeCall, FakeRunner};
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
            return Err(CliError::Refused(
                "the vendor CLI ran past its budget".into(),
            ));
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

    /// One observer tick against `github`; the `gh` calls it made.
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
        held.observe(roots, &mut budget, &mut Accepted, limits, now_ms, log_root)
            .expect("an observer tick");
        assert_eq!(budget.used, runner.calls().len());
        runner.calls()
    }

    fn logged_discovery_refusals(log_root: &Path) -> usize {
        std::fs::read_to_string(log_root.join("window-errors.log"))
            .unwrap_or_default()
            .matches("scm-observer: discovery fetch refused")
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
}
