use super::*;

/// 한 작업 폴더가 비교 ref(기본은 원격 추적 main)에 들어갔는지, git이 직접 한
/// 말 — 원장의 단계(검증 대기·완료)와는 다른 축이다.
///
/// 분류는 이 파일 한 곳에서만 한다. 사이드바 행과 git 패널 머리가 같은 값을
/// 읽고, 뒤따르는 원장 알림 과업도 같은 값을 재사용한다.
///
/// 여섯 상태, 그리고 서로 오분류하기 쉬운 쌍:
///
///   - `landed`: 자기 커밋이 모두 비교 ref에 들어 있다 — 조상이거나, 병합 결과가
///     비교 ref의 나무와 같거나(squash·cherry-pick), `git cherry`가 모두 같은
///     내용이라 한다.
///   - `unlanded`: 내용 기준으로 `ahead`개가 들어 있지 않다.
///   - `no_commits`: 생성 이후 자기 커밋이 없다. 조상이라는 점에서 `landed`와
///     같아 보이므로 **브랜치 reflog의 첫 줄(생성 지점)과 HEAD가 같은가**로
///     가른다. 비교 ref가 앞으로 가도 이 답은 변하지 않는다.
///   - `no_ref`: 비교 ref가 없거나 커밋을 가리키지 못한다.
///   - `unknown`: 조상인데 생성 지점을 읽을 자국이 없어 `landed`와 `no_commits`를
///     가를 수 없다(만료된 reflog, 복제본의 브랜치). 짐작하지 않는다.
///
/// 분리 HEAD는 `detached`로 따로 표시하고 `no_commits` 판정을 하지 않는다 —
/// 생성 지점을 적어 둘 브랜치가 없다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct WorktreeLanding {
    pub(super) state: &'static str,
    pub(super) detached: bool,
    /// `unlanded`일 때 내용 기준으로 비교 ref에 없는 커밋 수.
    pub(super) ahead: u32,
    /// 추적 중인 파일에 커밋하지 않은 변경이 있다.
    pub(super) dirty: bool,
    /// When `dirty` was last read, epoch milliseconds. A `status` is the one
    /// thing no ref can key, so it is re-read in the background once it is
    /// [`LANDING_DIRTY_TTL`] old and the catalog is read again; the tooltip says
    /// when, rather than promising a delay.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) dirty_checked_ms: Option<i64>,
    /// 비교한 ref의 이름. 비교 ref가 아예 없으면 없다.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) compare_ref: Option<String>,
    /// 그 ref가 마지막으로 움직인 시각(그 ref의 reflog), epoch 밀리초. 오래된
    /// 비교일 수 있음을 사람이 알게 한다. fetch는 이 기능이 하지 않는다.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) ref_updated_ms: Option<i64>,
    /// 이 작업을 처음 담은 비교 ref의 first-parent 커밋. 조상으로 들어간
    /// 경우에만 안다 — 내용으로만 들어간 것은 그런 커밋이 없다.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) landed_in: Option<LandedIn>,
}

impl WorktreeLanding {
    /// A row the cache has never answered for. The list goes out with this, and
    /// the classification follows in the background (`run_landing_job`).
    fn pending() -> Self {
        Self {
            state: "pending",
            detached: false,
            ahead: 0,
            dirty: false,
            dirty_checked_ms: None,
            compare_ref: None,
            ref_updated_ms: None,
            landed_in: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct LandedIn {
    pub(super) sha: String,
    pub(super) time_ms: i64,
}

/// 한 저장소가 한 번의 새로고침에 한 번만 묻는 비교 ref의 사실.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LandingBase {
    pub(super) name: Option<String>,
    pub(super) oid: Option<String>,
    pub(super) updated_ms: Option<i64>,
}

/// 비교 ref를 정한다: 프로젝트의 `worktree_base_ref`가 있으면 그것, 없으면
/// 저장소의 기본 브랜치(`default_base_probe`). 이름만 묻고 가져오지 않는다.
pub(super) fn resolve_landing_base(
    host: &Host,
    repo_root: &Path,
    pinned: Option<&str>,
) -> LandingBase {
    let name = pinned
        .map(str::trim)
        .filter(|one| !one.is_empty())
        .map(str::to_string)
        .or_else(|| default_base_probe(host, repo_root));
    let Some(name) = name else {
        return LandingBase {
            name: None,
            oid: None,
            updated_ms: None,
        };
    };
    let oid = optional_git_text(
        host,
        repo_root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{name}^{{commit}}"),
        ],
    );
    let updated_ms = oid.as_ref().and_then(|_| {
        optional_git_text(
            host,
            repo_root,
            &["reflog", "show", "--format=%ct", "-1", &name],
        )
        .and_then(|said| said.parse::<i64>().ok())
        .map(|seconds| seconds.saturating_mul(1000))
    });
    LandingBase {
        name: Some(name),
        oid,
        updated_ms,
    }
}

/// How long a row's "unsaved changes" answer stands. The one thing the landing
/// reads that no git ref can key is the working tree, and it costs a `status`
/// per row. Past this age the next catalog read serves the old answer at once
/// and queues the re-read in the background; inside it nothing is asked.
const LANDING_DIRTY_TTL: Duration = Duration::from_secs(5);

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_millis()).unwrap_or(i64::MAX)
        })
}

#[derive(Clone)]
struct HeldLanding {
    key: String,
    landing: WorktreeLanding,
}

type LandingCache = Mutex<HashMap<PathBuf, HeldLanding>>;

/// 행마다 마지막으로 답한 분류. (head, 비교 ref oid, 브랜치)가 같으면 분류는
/// 같다 — 분류가 읽는 것은 그 셋과 불변의 생성 지점뿐이다.
fn landing_cache() -> &'static LandingCache {
    static CACHE: OnceLock<LandingCache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn landing_key(head: Option<&str>, branch: Option<&str>, base: &LandingBase) -> String {
    format!(
        "{}|{}|{}",
        head.unwrap_or(""),
        base.oid.as_deref().unwrap_or(""),
        branch.unwrap_or("")
    )
}

fn held_landing(path: &Path) -> Option<HeldLanding> {
    landing_cache().lock().ok()?.get(path).cloned()
}

/// Whether a held answer still answers for these facts: the same key, and an
/// unsaved-changes reading younger than `dirty_ttl`.
fn stands(held: &HeldLanding, key: &str, dirty_ttl: Duration) -> bool {
    let ttl_ms = i64::try_from(dirty_ttl.as_millis()).unwrap_or(i64::MAX);
    held.key == key
        && held
            .landing
            .dirty_checked_ms
            .is_some_and(|at| now_ms().saturating_sub(at) < ttl_ms)
}

/// Whether two answers say the same thing about the work. The two stamps that
/// move without the work moving are left out: when `dirty` was read, and when
/// the compare ref last moved (it only words the tooltip).
fn same_facts(left: &WorktreeLanding, right: &WorktreeLanding) -> bool {
    let mut other = right.clone();
    other.dirty_checked_ms = left.dirty_checked_ms;
    other.ref_updated_ms = left.ref_updated_ms;
    *left == other
}

/// One row the background has to answer for.
pub(super) struct LandingRow {
    path: PathBuf,
    branch: Option<String>,
    head: Option<String>,
}

/// What one repository's catalog read left to do: the rows whose answer is
/// missing, moved or too old.
pub(super) struct LandingJob {
    repo_root: PathBuf,
    pinned: Option<String>,
    rows: Vec<LandingRow>,
}

/// Stamp every row of one repository with what the cache already knows, and say
/// what is left to ask.
///
/// Nothing here starts a git process. A row the cache never answered for is
/// `pending`; one whose head or ref moved keeps its old answer until the new
/// one is ready, so a commit does not make a chip flash. The job — [`None`]
/// when every row stands — is run off the catalog's road by
/// [`spawn_landing_jobs`].
pub(super) fn attach_landings(
    entries: &mut [WorktreeEntry],
    repo_root: &Path,
    repository: &settings::SettingsRepository,
) -> Option<LandingJob> {
    let pinned = stored_project_settings_at(repository, &project_settings_key(repo_root))
        .ok()
        .and_then(|stored| stored.worktree_base_ref);
    plan_landings(entries, repo_root, pinned.as_deref())
}

fn plan_landings(
    entries: &mut [WorktreeEntry],
    repo_root: &Path,
    pinned: Option<&str>,
) -> Option<LandingJob> {
    let base = known_landing_base(repo_root, pinned);
    let mut rows = Vec::new();
    for entry in entries.iter_mut() {
        if entry.is_main || entry.is_folder || entry.prunable {
            continue;
        }
        let path = PathBuf::from(&entry.path);
        let held = held_landing(&path);
        let standing = match (&held, &base) {
            (Some(held), Some(base)) => stands(
                held,
                &landing_key(entry.head.as_deref(), entry.branch.as_deref(), base),
                LANDING_DIRTY_TTL,
            ),
            _ => false,
        };
        let mut served = held.map_or_else(WorktreeLanding::pending, |held| held.landing);
        if let Some(base) = &base {
            served.compare_ref = base.name.clone();
            served.ref_updated_ms = base.updated_ms;
        }
        entry.landing = Some(served);
        if !standing {
            rows.push(LandingRow {
                path,
                branch: entry.branch.clone(),
                head: entry.head.clone(),
            });
        }
    }
    (!rows.is_empty()).then(|| LandingJob {
        repo_root: repo_root.to_path_buf(),
        pinned: pinned.map(str::to_string),
        rows,
    })
}

/// Run the jobs a catalog read left, each on a thread of its own, and tell the
/// window once per job that moved something. A job that changed nothing says
/// nothing, so a window that re-reads on the notice cannot start a loop.
pub(super) fn spawn_landing_jobs(app: &AppHandle, jobs: Vec<LandingJob>) {
    for job in jobs {
        let app = app.clone();
        let _ = std::thread::Builder::new()
            .name("landing".to_string())
            .spawn(move || {
                run_landing_job(&job, &|| {
                    let _ = app.emit("worktree:landing", ());
                });
            });
    }
}

/// Answer a job's rows and call `notify` once if any of them now says something
/// different from what the window was last given.
fn run_landing_job(job: &LandingJob, notify: &(dyn Fn() + Sync)) {
    // One job per repository at a time. A second read of the same catalog queues
    // behind the first and finds its rows already standing, which is cheaper
    // than answering them twice.
    let gate = repo_gate(&job.repo_root);
    let _turn = gate
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let host = Host::for_workspace(&job.repo_root);
    let base = landing_base(&host, &job.repo_root, job.pinned.as_deref());
    let moved = std::thread::scope(|scope| {
        let handles: Vec<_> = job
            .rows
            .iter()
            .map(|row| {
                let (host, base) = (&host, &base);
                scope.spawn(move || refresh_row(host, row, base))
            })
            .collect();
        handles
            .into_iter()
            .fold(false, |any, handle| handle.join().unwrap_or(false) || any)
    });
    if moved {
        notify();
    }
}

fn refresh_row(host: &Host, row: &LandingRow, base: &LandingBase) -> bool {
    let key = landing_key(row.head.as_deref(), row.branch.as_deref(), base);
    let before = held_landing(&row.path);
    if before
        .as_ref()
        .is_some_and(|held| stands(held, &key, LANDING_DIRTY_TTL))
    {
        return false;
    }
    let after = worktree_landing(
        host,
        &row.path,
        row.branch.as_deref(),
        row.head.as_deref(),
        base,
    );
    before.is_none_or(|held| !same_facts(&held.landing, &after))
}

fn repo_gate(repo_root: &Path) -> Arc<Mutex<()>> {
    static GATES: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();
    let mut gates = GATES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    Arc::clone(gates.entry(repo_root.to_path_buf()).or_default())
}

/// 한 작업 폴더의 분류. 바뀐 행만 git을 몇 번 부르고, 같은 행은 TTL 안에서
/// 한 번도 부르지 않는다.
fn worktree_landing(
    host: &Host,
    path: &Path,
    branch: Option<&str>,
    head: Option<&str>,
    base: &LandingBase,
) -> WorktreeLanding {
    worktree_landing_within(host, path, branch, head, base, LANDING_DIRTY_TTL)
}

fn worktree_landing_within(
    host: &Host,
    path: &Path,
    branch: Option<&str>,
    head: Option<&str>,
    base: &LandingBase,
    dirty_ttl: Duration,
) -> WorktreeLanding {
    let key = landing_key(head, branch, base);
    let held = held_landing(path);
    if let Some(held) = held.as_ref()
        && stands(held, &key, dirty_ttl)
    {
        return WorktreeLanding {
            compare_ref: base.name.clone(),
            ref_updated_ms: base.updated_ms,
            ..held.landing.clone()
        };
    }
    let mut landing = match held {
        Some(held) if held.key == key => held.landing,
        _ => classify_landing(host, path, branch, head, base),
    };
    landing.compare_ref = base.name.clone();
    landing.ref_updated_ms = base.updated_ms;
    landing.dirty = optional_git_text(
        host,
        path,
        &["status", "--porcelain", "--untracked-files=no"],
    )
    .is_some();
    landing.dirty_checked_ms = Some(now_ms());
    if let Ok(mut cache) = landing_cache().lock() {
        cache.insert(
            path.to_path_buf(),
            HeldLanding {
                key,
                landing: landing.clone(),
            },
        );
    }
    landing
}

struct HeldBase {
    pinned: Option<String>,
    stamp: Vec<Option<std::time::SystemTime>>,
    base: LandingBase,
}

fn base_cache() -> &'static Mutex<HashMap<PathBuf, HeldBase>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, HeldBase>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The files a fetch or a branch move writes, as modification times: the
/// packed refs, the origin/HEAD pointer, the ref compared with and its
/// reflog, and the two directories a new ref appears in. Reading them is a
/// handful of `stat`s, so an unchanged repository asks git nothing.
fn base_stamp(git_dir: &Path, name: Option<&str>) -> Vec<Option<std::time::SystemTime>> {
    let mut files = vec![
        "packed-refs".to_string(),
        "refs/remotes/origin/HEAD".to_string(),
        "refs/remotes".to_string(),
        "refs/heads".to_string(),
    ];
    if let Some(name) = name {
        files.push(format!("refs/remotes/{name}"));
        files.push(format!("refs/heads/{name}"));
        files.push(format!("logs/refs/remotes/{name}"));
    }
    files
        .iter()
        .map(|file| {
            std::fs::metadata(git_dir.join(file))
                .and_then(|meta| meta.modified())
                .ok()
        })
        .collect()
}

/// The compare ref as it was last asked of git, when nothing it reads has moved
/// since — by `stat` alone. [`None`] means somebody has to ask git.
fn known_landing_base(repo_root: &Path, pinned: Option<&str>) -> Option<LandingBase> {
    let git_dir = repo_root.join(".git");
    if !git_dir.is_dir() {
        return None;
    }
    let cache = base_cache().lock().ok()?;
    let held = cache.get(repo_root)?;
    (held.pinned.as_deref() == pinned
        && held.stamp == base_stamp(&git_dir, held.base.name.as_deref()))
    .then(|| held.base.clone())
}

/// [`resolve_landing_base`], asked of git only when the stamp of the files it
/// reads has moved. A checkout whose `.git` is not a directory (a linked one
/// opened as the project) is asked every time rather than guessed at.
fn landing_base(host: &Host, repo_root: &Path, pinned: Option<&str>) -> LandingBase {
    if let Some(base) = known_landing_base(repo_root, pinned) {
        return base;
    }
    let base = resolve_landing_base(host, repo_root, pinned);
    let git_dir = repo_root.join(".git");
    if git_dir.is_dir() {
        let stamp = base_stamp(&git_dir, base.name.as_deref());
        if let Ok(mut held) = base_cache().lock() {
            held.insert(
                repo_root.to_path_buf(),
                HeldBase {
                    pinned: pinned.map(str::to_string),
                    stamp,
                    base: base.clone(),
                },
            );
        }
    }
    base
}

/// What the window polls to learn that a landing may have moved: the
/// modification times of the files a commit, a checkout, a branch move or a
/// fetch writes — the compare ref's, and each linked checkout's `HEAD` and its
/// reflog. Only `stat`; no git process and no network, and a stamp that cannot
/// be read is a stamp that does not move.
pub(crate) fn landing_stamp_of(roots: Vec<String>) -> String {
    fn at(path: &Path) -> String {
        std::fs::metadata(path)
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or_else(|| "-".to_string(), |since| since.as_nanos().to_string())
    }
    let mut said = String::new();
    for root in roots {
        said.push_str(&root);
        said.push(':');
        let root = Path::new(&root);
        let git_dir = root.join(".git");
        let name = base_cache()
            .lock()
            .ok()
            .and_then(|cache| cache.get(root).and_then(|held| held.base.name.clone()));
        for mark in base_stamp(&git_dir, name.as_deref()) {
            said.push_str(&format!("{mark:?},"));
        }
        said.push_str(&at(&git_dir.join("logs/HEAD")));
        if let Some(dir) = linked_worktrees_dir(root)
            && let Ok(entries) = std::fs::read_dir(&dir)
        {
            let mut names: Vec<String> = entries
                .filter_map(|entry| Some(entry.ok()?.file_name().to_string_lossy().into_owned()))
                .collect();
            names.sort_unstable();
            for name in names {
                let home = dir.join(&name);
                said.push_str(&format!(
                    ",{name}={}/{}",
                    at(&home.join("HEAD")),
                    at(&home.join("logs/HEAD"))
                ));
            }
        }
        said.push(';');
    }
    said
}

fn classify_landing(
    host: &Host,
    path: &Path,
    branch: Option<&str>,
    head: Option<&str>,
    base: &LandingBase,
) -> WorktreeLanding {
    let mut landing = WorktreeLanding {
        state: "unknown",
        detached: branch.is_none(),
        ahead: 0,
        dirty: false,
        dirty_checked_ms: None,
        compare_ref: base.name.clone(),
        ref_updated_ms: base.updated_ms,
        landed_in: None,
    };
    let Some(target) = base.oid.as_deref() else {
        landing.state = "no_ref";
        return landing;
    };
    let Some(head) = head else {
        return landing;
    };
    let creation = branch.and_then(|name| creation_point(host, path, name));
    if creation.as_deref() == Some(head) {
        landing.state = "no_commits";
        return landing;
    }
    let ancestor = host
        .vcs()
        .text(path, &["merge-base", "--is-ancestor", head, target])
        .is_ok();
    if ancestor {
        if branch.is_some() && creation.is_none() {
            return landing;
        }
        landing.state = "landed";
        landing.landed_in = landed_commit(host, path, head, target);
        return landing;
    }
    if merge_changes_nothing(host, path, head, target) {
        landing.state = "landed";
        return landing;
    }
    let ahead = host
        .vcs()
        .text(path, &["cherry", target, head])
        .map_or(0, |said| {
            said.lines().filter(|line| line.starts_with('+')).count()
        });
    if ahead == 0 {
        landing.state = "landed";
    } else {
        landing.state = "unlanded";
        landing.ahead = u32::try_from(ahead).unwrap_or(u32::MAX);
    }
    landing
}

/// 브랜치가 만들어진 커밋 — 그 브랜치 reflog의 가장 오래된 줄.
fn creation_point(host: &Host, path: &Path, branch: &str) -> Option<String> {
    let said = host
        .vcs()
        .text(
            path,
            &[
                "reflog",
                "show",
                "--format=%H",
                &format!("refs/heads/{branch}"),
            ],
        )
        .ok()?;
    said.lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .map(str::to_string)
}

/// `head`를 `target`에 병합해도 나무가 바뀌지 않는가. squash나 cherry-pick으로
/// 내용만 들어간 작업이 여기서 잡힌다. 충돌하면(git이 0이 아닌 값으로 끝난다)
/// 같다고 말하지 않는다.
fn merge_changes_nothing(host: &Host, path: &Path, head: &str, target: &str) -> bool {
    let Ok(merged) = host
        .vcs()
        .text(path, &["merge-tree", "--write-tree", target, head])
    else {
        return false;
    };
    let Some(tree) = merged.lines().next().map(str::trim) else {
        return false;
    };
    optional_git_text(host, path, &["rev-parse", &format!("{target}^{{tree}}")])
        .is_some_and(|target_tree| target_tree == tree)
}

/// 이 작업을 처음 담은 비교 ref의 first-parent 커밋. `head`가 그 줄기 위에
/// 있으면(빨리감기) `head` 자신이다.
fn landed_commit(host: &Host, path: &Path, head: &str, target: &str) -> Option<LandedIn> {
    let range = format!("{head}..{target}");
    let said = host
        .vcs()
        .text(
            path,
            &[
                "log",
                "--first-parent",
                "--ancestry-path",
                "--format=%H %ct %P",
                &range,
            ],
        )
        .ok()?;
    let oldest = said.lines().map(str::trim).rfind(|line| !line.is_empty());
    let (sha, seconds) = match oldest {
        Some(line) => {
            let mut fields = line.split_whitespace();
            let sha = fields.next()?;
            let seconds = fields.next()?;
            // 첫 부모가 `head`이면 `head`가 줄기 위에 있다 — 담은 커밋은 head다.
            if fields.next() == Some(head) {
                return commit_time(host, path, head).map(|time_ms| LandedIn {
                    sha: head.to_string(),
                    time_ms,
                });
            }
            (sha.to_string(), seconds.parse::<i64>().ok()?)
        }
        None => {
            let time_ms = commit_time(host, path, target)?;
            return Some(LandedIn {
                sha: target.to_string(),
                time_ms,
            });
        }
    };
    Some(LandedIn {
        sha,
        time_ms: seconds.saturating_mul(1000),
    })
}

fn commit_time(host: &Host, path: &Path, commit: &str) -> Option<i64> {
    optional_git_text(host, path, &["log", "-1", "--format=%ct", commit])
        .and_then(|said| said.parse::<i64>().ok())
        .map(|seconds| seconds.saturating_mul(1000))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(repo: &Path, args: &[&str]) -> String {
        let output = crate::proc::quiet_command("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .expect("git");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    /// A repository with `main` and a remote-tracking `origin/main` that moves
    /// only when the test says so — the window never fetches for this.
    struct Bench {
        _temp: tempfile::TempDir,
        repo: PathBuf,
        root: PathBuf,
    }

    impl Bench {
        fn open() -> Self {
            let temp = tempfile::tempdir().expect("a landing repository");
            let root = temp.path().to_path_buf();
            let repo = root.join("repo");
            std::fs::create_dir_all(&repo).expect("repo");
            git(&repo, &["init", "-q", "-b", "main"]);
            git(&repo, &["config", "user.name", "ZeroCode Test"]);
            git(&repo, &["config", "user.email", "test@zerocode"]);
            std::fs::write(repo.join("a.txt"), "one\n").expect("a.txt");
            git(&repo, &["add", "."]);
            git(&repo, &["commit", "-q", "-m", "first"]);
            let bench = Self {
                _temp: temp,
                repo,
                root,
            };
            bench.publish();
            bench
        }

        /// What a fetch would have done: move `origin/main` to `main`.
        fn publish(&self) {
            git(
                &self.repo,
                &["update-ref", "refs/remotes/origin/main", "main"],
            );
        }

        fn worktree(&self, name: &str) -> PathBuf {
            let path = self.root.join(name);
            git(
                &self.repo,
                &[
                    "worktree",
                    "add",
                    "-q",
                    "-b",
                    &format!("wt/{name}"),
                    &path.to_string_lossy(),
                    "main",
                ],
            );
            path
        }

        fn commit(&self, at: &Path, file: &str, text: &str) {
            std::fs::write(at.join(file), text).expect("write");
            git(at, &["add", file]);
            git(at, &["commit", "-q", "-m", &format!("edit {file}")]);
        }

        fn landing(&self, at: &Path) -> WorktreeLanding {
            let host = Host::for_workspace(&self.repo);
            let base = resolve_landing_base(&host, &self.repo, None);
            let head = git(at, &["rev-parse", "HEAD"]);
            let branch = git(at, &["symbolic-ref", "--quiet", "--short", "HEAD"]);
            // A zero TTL: these tests change the working tree between asks and
            // want the answer of that moment, not of five seconds ago.
            worktree_landing_within(&host, at, Some(&branch), Some(&head), &base, Duration::ZERO)
        }
    }

    #[test]
    fn a_checkout_cut_and_never_committed_is_no_commits_even_after_main_moved() {
        let bench = Bench::open();
        let wt = bench.worktree("idle");
        assert_eq!(bench.landing(&wt).state, "no_commits");
        // main moves on and is published: the idle branch is now an ancestor of
        // origin/main, which is exactly what a merged one looks like.
        bench.commit(&bench.repo, "b.txt", "main\n");
        bench.publish();
        let landing = bench.landing(&wt);
        assert_eq!(landing.state, "no_commits");
        assert_eq!(landing.compare_ref.as_deref(), Some("origin/main"));
    }

    #[test]
    fn a_merge_commit_lands_the_branch_and_names_the_commit_that_took_it() {
        let bench = Bench::open();
        let wt = bench.worktree("merged");
        bench.commit(&wt, "w.txt", "work\n");
        assert_eq!(bench.landing(&wt).state, "unlanded");
        assert_eq!(bench.landing(&wt).ahead, 1);
        bench.commit(&bench.repo, "m.txt", "main moves\n");
        git(
            &bench.repo,
            &["merge", "--no-ff", "-q", "-m", "merge it", "wt/merged"],
        );
        bench.publish();
        let landing = bench.landing(&wt);
        assert_eq!(landing.state, "landed");
        let merge = git(&bench.repo, &["rev-parse", "main"]);
        assert_eq!(landing.landed_in.map(|one| one.sha), Some(merge));
    }

    #[test]
    fn a_fast_forward_names_the_head_itself() {
        let bench = Bench::open();
        let wt = bench.worktree("ff");
        bench.commit(&wt, "w.txt", "work\n");
        git(&bench.repo, &["merge", "--ff-only", "-q", "wt/ff"]);
        bench.commit(&bench.repo, "later.txt", "later\n");
        bench.publish();
        let landing = bench.landing(&wt);
        assert_eq!(landing.state, "landed");
        let head = git(&wt, &["rev-parse", "HEAD"]);
        assert_eq!(landing.landed_in.map(|one| one.sha), Some(head));
    }

    #[test]
    fn a_squash_lands_by_content_with_no_commit_to_name() {
        let bench = Bench::open();
        let wt = bench.worktree("squash");
        bench.commit(&wt, "w.txt", "one\n");
        bench.commit(&wt, "w2.txt", "two\n");
        git(&bench.repo, &["merge", "--squash", "wt/squash"]);
        git(&bench.repo, &["commit", "-q", "-m", "squashed"]);
        bench.publish();
        let landing = bench.landing(&wt);
        assert_eq!(landing.state, "landed");
        assert_eq!(landing.landed_in, None);
    }

    #[test]
    fn a_cherry_pick_lands_by_content() {
        let bench = Bench::open();
        let wt = bench.worktree("pick");
        bench.commit(&wt, "w.txt", "picked\n");
        let picked = git(&wt, &["rev-parse", "HEAD"]);
        bench.commit(&bench.repo, "m.txt", "main first\n");
        git(&bench.repo, &["cherry-pick", &picked]);
        bench.publish();
        assert_eq!(bench.landing(&wt).state, "landed");
    }

    #[test]
    fn the_count_is_by_content_not_by_commit_id() {
        let bench = Bench::open();
        let wt = bench.worktree("half");
        bench.commit(&wt, "w.txt", "one\n");
        let first = git(&wt, &["rev-parse", "HEAD"]);
        bench.commit(&wt, "w2.txt", "two\n");
        bench.commit(&wt, "w3.txt", "three\n");
        bench.commit(&bench.repo, "m.txt", "main first\n");
        git(&bench.repo, &["cherry-pick", &first]);
        bench.publish();
        let landing = bench.landing(&wt);
        assert_eq!((landing.state, landing.ahead), ("unlanded", 2));
    }

    #[test]
    fn a_moved_local_main_does_not_land_what_the_remote_ref_lacks() {
        let bench = Bench::open();
        let wt = bench.worktree("local");
        bench.commit(&wt, "w.txt", "work\n");
        git(&bench.repo, &["merge", "--ff-only", "-q", "wt/local"]);
        // Not published: origin/main is where it was.
        let landing = bench.landing(&wt);
        assert_eq!((landing.state, landing.ahead), ("unlanded", 1));
    }

    #[test]
    fn uncommitted_changes_ride_beside_the_state_and_untracked_files_do_not() {
        let bench = Bench::open();
        let wt = bench.worktree("dirty");
        assert!(!bench.landing(&wt).dirty);
        std::fs::write(wt.join("loose.txt"), "x\n").expect("untracked");
        assert!(!bench.landing(&wt).dirty);
        std::fs::write(wt.join("a.txt"), "changed\n").expect("tracked");
        let landing = bench.landing(&wt);
        assert!(landing.dirty);
        assert_eq!(landing.state, "no_commits");
    }

    #[test]
    fn a_missing_compare_ref_is_no_ref_not_landed() {
        let bench = Bench::open();
        let wt = bench.worktree("noref");
        bench.commit(&wt, "w.txt", "work\n");
        let host = Host::for_workspace(&bench.repo);
        let base = resolve_landing_base(&host, &bench.repo, Some("origin/gone"));
        let head = git(&wt, &["rev-parse", "HEAD"]);
        let landing = worktree_landing(&host, &wt, Some("wt/noref"), Some(&head), &base);
        assert_eq!(landing.state, "no_ref");
        assert_eq!(landing.compare_ref.as_deref(), Some("origin/gone"));
    }

    #[test]
    fn a_detached_head_inside_main_is_landed_and_one_outside_is_counted() {
        let bench = Bench::open();
        let host = Host::for_workspace(&bench.repo);
        let inside = bench.root.join("inside");
        git(
            &bench.repo,
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                &inside.to_string_lossy(),
                "main",
            ],
        );
        let base = resolve_landing_base(&host, &bench.repo, None);
        let head = git(&inside, &["rev-parse", "HEAD"]);
        let landing = worktree_landing(&host, &inside, None, Some(&head), &base);
        assert_eq!((landing.state, landing.detached), ("landed", true));

        let outside = bench.root.join("outside");
        git(
            &bench.repo,
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                &outside.to_string_lossy(),
                "main",
            ],
        );
        bench.commit(&outside, "w.txt", "loose\n");
        let head = git(&outside, &["rev-parse", "HEAD"]);
        let landing = worktree_landing(&host, &outside, None, Some(&head), &base);
        assert_eq!(
            (landing.state, landing.detached, landing.ahead),
            ("unlanded", true, 1)
        );
    }

    #[test]
    fn the_reflog_gone_makes_an_ancestor_unknown_rather_than_guessed() {
        let bench = Bench::open();
        let wt = bench.worktree("expired");
        git(&bench.repo, &["reflog", "expire", "--expire=now", "--all"]);
        let landing = bench.landing(&wt);
        assert_eq!(landing.state, "unknown");
    }

    #[test]
    fn a_second_ask_with_the_same_facts_answers_from_the_cache() {
        let bench = Bench::open();
        let wt = bench.worktree("cached");
        assert_eq!(bench.landing(&wt).state, "no_commits");
        // A fresh classification would now find no creation point and say
        // `unknown`; the cached one is keyed on (head, ref, branch) and holds.
        git(&bench.repo, &["reflog", "expire", "--expire=now", "--all"]);
        assert_eq!(bench.landing(&wt).state, "no_commits");
    }

    #[test]
    fn the_unsaved_answer_stands_for_its_ttl_and_the_row_starts_no_process_inside_it() {
        let bench = Bench::open();
        let wt = bench.worktree("ttl");
        let host = Host::for_workspace(&bench.repo);
        let base = resolve_landing_base(&host, &bench.repo, None);
        let head = git(&wt, &["rev-parse", "HEAD"]);
        let ask =
            |ttl| worktree_landing_within(&host, &wt, Some("wt/ttl"), Some(&head), &base, ttl);
        assert!(!ask(Duration::ZERO).dirty);
        std::fs::write(wt.join("a.txt"), "changed\n").expect("tracked");
        // Inside the TTL nothing is asked of git, so the edit is not seen yet.
        assert!(!ask(Duration::from_secs(3600)).dirty);
        // Past it, the one `status` runs and the edit is seen.
        assert!(ask(Duration::ZERO).dirty);
    }

    #[test]
    fn the_compare_ref_is_asked_of_git_again_only_when_its_files_moved() {
        let bench = Bench::open();
        let host = Host::for_workspace(&bench.repo);
        let first = landing_base(&host, &bench.repo, None);
        assert_eq!(landing_base(&host, &bench.repo, None), first);
        bench.commit(&bench.repo, "m.txt", "main moves\n");
        bench.publish();
        let moved = landing_base(&host, &bench.repo, None);
        assert_ne!(moved.oid, first.oid);
        assert_eq!(moved.oid, Some(git(&bench.repo, &["rev-parse", "main"])));
        // A different pin is a different question even with no file moving.
        let pinned = landing_base(&host, &bench.repo, Some("origin/gone"));
        assert_eq!(pinned.name.as_deref(), Some("origin/gone"));
        assert_eq!(pinned.oid, None);
    }

    fn entry(path: &Path, branch: &str, head: &str) -> WorktreeEntry {
        WorktreeEntry::new(
            Worktree {
                path: path.to_path_buf(),
                head: Some(head.to_string()),
                branch: Some(branch.to_string()),
                is_main: false,
                bare: false,
                detached: false,
                locked: false,
                prunable: false,
            },
            Path::new("/not/the/active/root"),
        )
    }

    fn noticed(job: &LandingJob) -> usize {
        let count = std::sync::atomic::AtomicUsize::new(0);
        run_landing_job(job, &|| {
            count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
        count.load(std::sync::atomic::Ordering::SeqCst)
    }

    #[test]
    fn a_row_never_answered_goes_out_pending_is_filled_behind_it_and_noticed_once() {
        let bench = Bench::open();
        let wt = bench.worktree("pend");
        let head = git(&wt, &["rev-parse", "HEAD"]);
        let mut rows = [entry(&wt, "wt/pend", &head)];
        let job = plan_landings(&mut rows, &bench.repo, None).expect("nothing is known yet");
        assert_eq!(
            rows[0].landing.as_ref().map(|one| one.state),
            Some("pending")
        );
        assert_eq!(noticed(&job), 1);
        // The same read again: the row is filled and standing, so there is no job
        // to run and nobody to tell.
        let mut again = [entry(&wt, "wt/pend", &head)];
        assert!(plan_landings(&mut again, &bench.repo, None).is_none());
        assert_eq!(
            again[0].landing.as_ref().map(|one| one.state),
            Some("no_commits")
        );
        // And running the old job once more finds every row standing: no notice.
        assert_eq!(noticed(&job), 0);
    }

    #[test]
    fn a_commit_in_a_checkout_moves_the_stamp_is_reclassified_in_the_background_and_noticed() {
        let bench = Bench::open();
        let wt = bench.worktree("moves");
        let head = git(&wt, &["rev-parse", "HEAD"]);
        let roots = vec![bench.repo.to_string_lossy().into_owned()];
        let mut rows = [entry(&wt, "wt/moves", &head)];
        let job = plan_landings(&mut rows, &bench.repo, None).expect("first read");
        assert_eq!(noticed(&job), 1);
        let quiet = landing_stamp_of(roots.clone());
        assert_eq!(
            landing_stamp_of(roots.clone()),
            quiet,
            "stat alone moves nothing"
        );
        bench.commit(&wt, "w.txt", "work\n");
        assert_ne!(
            landing_stamp_of(roots),
            quiet,
            "a commit in a checkout moves the stamp"
        );
        // The next read serves the old answer at once — no flash — and queues the
        // new one, which says something different and so is noticed.
        let moved_head = git(&wt, &["rev-parse", "HEAD"]);
        let mut next = [entry(&wt, "wt/moves", &moved_head)];
        let job = plan_landings(&mut next, &bench.repo, None).expect("the head moved");
        assert_eq!(
            next[0].landing.as_ref().map(|one| one.state),
            Some("no_commits")
        );
        assert_eq!(noticed(&job), 1);
        let mut last = [entry(&wt, "wt/moves", &moved_head)];
        assert!(plan_landings(&mut last, &bench.repo, None).is_none());
        assert_eq!(
            last[0].landing.as_ref().map(|one| (one.state, one.ahead)),
            Some(("unlanded", 1))
        );
    }

    #[test]
    fn the_compare_ref_moving_without_changing_any_answer_notices_nobody() {
        let bench = Bench::open();
        let wt = bench.worktree("quiet");
        let head = git(&wt, &["rev-parse", "HEAD"]);
        let mut rows = [entry(&wt, "wt/quiet", &head)];
        let job = plan_landings(&mut rows, &bench.repo, None).expect("first read");
        assert_eq!(noticed(&job), 1);
        // main moves on and is published: the stamp of the ref moved, so the
        // rows are asked again — and say the same thing, so nobody is told.
        bench.commit(&bench.repo, "m.txt", "main moves\n");
        bench.publish();
        let mut next = [entry(&wt, "wt/quiet", &head)];
        let job = plan_landings(&mut next, &bench.repo, None).expect("the ref moved");
        assert_eq!(noticed(&job), 0);
        let mut last = [entry(&wt, "wt/quiet", &head)];
        assert!(plan_landings(&mut last, &bench.repo, None).is_none());
    }
}
