//! Strict, conditional GitHub reads for the autonomous PR observer.
use super::*;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;
use zerocode_core::checks::Limits;
use zerocode_core::scm_observer::{Ci, Mergeability, PrState, ReviewItem};

/// Discovery + three CI endpoints + one annotation + one review query + one stack page.
pub(crate) const SUBJECT_CALLS_MAX: usize = 7;

pub(crate) struct Budget<'a> {
    runner: &'a dyn CliRunner,
    pub(crate) used: usize,
    pub(crate) max: usize,
    /// How long one call may run — [`Limits::gh_call_ms`].
    call_ms: u64,
}
impl Budget<'static> {
    pub(crate) fn new(limits: &Limits) -> Self {
        Self::over(&ProcessRunner, limits)
    }
}
impl<'a> Budget<'a> {
    /// A budget spent through `runner`; the table says how many calls and how
    /// long each may take.
    pub(crate) fn over(runner: &'a dyn CliRunner, limits: &Limits) -> Self {
        Self {
            runner,
            used: 0,
            max: limits.gh_calls_max,
            call_ms: limits.gh_call_ms,
        }
    }
}
impl Budget<'_> {
    pub(crate) fn remaining(&self) -> usize {
        self.max.saturating_sub(self.used)
    }
    fn call(&mut self, cwd: &Path, args: &[String]) -> Result<CliOutput, GhError> {
        if self.remaining() == 0 {
            return Err(GhError::Refused("SCM tick call budget exhausted".into()));
        }
        self.used += 1;
        let _auth = AUTH_LIFECYCLE
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        call_gh(
            self.runner,
            Some(cwd),
            args,
            None,
            Some(Duration::from_millis(self.call_ms)),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HttpSnapshot {
    etag: Option<String>,
    body: String,
    more: bool,
}

fn unreadable(message: &str) -> GhError {
    GhError::Unreadable(message.into())
}

fn read_http(raw: &str, cached: Option<&HttpSnapshot>) -> Result<HttpSnapshot, GhError> {
    let normalized = raw.replace("\r\n", "\n");
    let (headers, body) = normalized
        .split_once("\n\n")
        .ok_or_else(|| unreadable("missing HTTP headers"))?;
    let status = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1));
    match status {
        Some("304") => cached
            .cloned()
            .ok_or_else(|| unreadable("304 without an acknowledged cached body")),
        Some("200") => Ok(HttpSnapshot {
            body: body.into(),
            more: headers.lines().any(|line| {
                line.to_ascii_lowercase().starts_with("link:") && line.contains("rel=\"next\"")
            }),
            etag: headers.lines().find_map(|line| {
                let (key, value) = line.split_once(':')?;
                key.eq_ignore_ascii_case("etag")
                    .then(|| value.trim().to_owned())
            }),
        }),
        _ => Err(unreadable("SCM HTTP request did not return 200 or 304")),
    }
}

/// Clone before fetching. Publish this candidate cache only after facts and ACKs are durable.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Client {
    cache: BTreeMap<String, HttpSnapshot>,
}
impl Client {
    fn api(
        &mut self,
        cwd: &Path,
        path: &str,
        force: bool,
        budget: &mut Budget,
    ) -> Result<String, GhError> {
        let old = self.cache.get(path).filter(|_| may_use_auth_cache());
        let mut args = vec!["api".into(), "--include".into(), path.into()];
        if !force && let Some(etag) = old.and_then(|held| held.etag.as_ref()) {
            args.extend(["-H".into(), format!("If-None-Match: {etag}")]);
        }
        let output = budget.call(cwd, &args)?;
        // gh versions can return nonzero for 304; only that precise response may reuse a body.
        if !output.success
            && output
                .stdout
                .lines()
                .next()
                .is_none_or(|line| line.split_whitespace().nth(1) != Some("304"))
        {
            return Err(GhError::Refused(output.stderr));
        }
        let snapshot = read_http(&output.stdout, old)?;
        if snapshot.more && !path.contains("/annotations?") {
            return Err(unreadable("SCM response exceeds the bounded page"));
        }
        self.cache.insert(path.into(), snapshot.clone());
        // Old heads and annotation IDs are only cache entries, never cursors.
        while self.cache.len() > 32 {
            self.cache.pop_first();
        }
        Ok(snapshot.body)
    }
    pub(crate) fn discover(
        &self,
        cwd: &Path,
        budget: &mut Budget,
    ) -> Result<Option<HostedReview>, GhError> {
        let args = ["pr".into(), "view".into(), "--json".into(),
            "number,title,url,state,isDraft,headRefOid,headRepository,headRepositoryOwner,mergeable,mergeStateStatus,updatedAt,baseRefName".into()];
        let output = budget.call(cwd, &args)?;
        if !output.success {
            if output.stderr.contains("no pull requests found for branch") {
                return Ok(None);
            }
            return Err(GhError::Refused(output.stderr));
        }
        let mut review =
            read_pull_request(&output.stdout)?.ok_or_else(|| unreadable("missing PR identity"))?;
        let body = parse_json(&output.stdout)?;
        if read_state(&review.state).is_none() || review.head_sha.is_empty() {
            return Err(unreadable("incomplete PR state or head"));
        }
        // The PR belongs to its base repository, including fork PRs. Its URL
        // is authoritative; headRepository is the contributor's push origin.
        review.owner_repo = pr_repository(&review.url, review.number)
            .ok_or_else(|| unreadable("invalid PR URL coordinates"))?;
        if review.mergeable.as_deref() != Some("conflicting")
            && body.get("mergeStateStatus").and_then(Value::as_str) == Some("BLOCKED")
        {
            review.mergeable = Some("blocked".into());
        }
        Ok(Some(review))
    }

    pub(crate) fn checks(
        &mut self,
        cwd: &Path,
        review: &HostedReview,
        force: bool,
        budget: &mut Budget,
    ) -> Result<Ci, GhError> {
        self.checks_for(cwd, &review.owner_repo, &review.head_sha, force, budget)
    }
    pub(super) fn checks_for(
        &mut self,
        cwd: &Path,
        repo: &str,
        head: &str,
        force: bool,
        budget: &mut Budget,
    ) -> Result<Ci, GhError> {
        let prefix = format!("repos/{repo}/commits/{head}");
        let raw = self.api(
            cwd,
            &format!("{prefix}/check-runs?per_page={CHECKS_PER_PAGE}"),
            force,
            budget,
        )?;
        let body = parse_json(&raw)?;
        let rows = complete_rows(&body, "check_runs")?;
        if rows.iter().any(|row| {
            row.get("name").and_then(Value::as_str).is_none()
                || !matches!(
                    row.get("status").and_then(Value::as_str),
                    Some("queued" | "in_progress" | "completed")
                )
        }) {
            return Err(unreadable("invalid check run"));
        }
        let mut checks = read_check_runs(&raw)?;
        let raw = self.api(
            cwd,
            &format!("{prefix}/status?per_page={CHECKS_PER_PAGE}"),
            force,
            budget,
        )?;
        let body = parse_json(&raw)?;
        let rows = complete_rows(&body, "statuses")?;
        if rows.iter().any(|row| {
            row.get("context").and_then(Value::as_str).is_none()
                || !matches!(
                    row.get("state").and_then(Value::as_str),
                    Some("success" | "failure" | "error" | "pending")
                )
        }) {
            return Err(unreadable("invalid commit status"));
        }
        checks.extend(read_commit_statuses(&raw, &checks)?);
        let raw = self.api(
            cwd,
            &format!("{prefix}/check-suites?per_page={CHECKS_PER_PAGE}"),
            force,
            budget,
        )?;
        complete_rows(&parse_json(&raw)?, "check_suites")?;
        checks.extend(read_blocked_suites(&raw, head, repo, &checks)?);
        zerocode_core::checks::sort_for_display(&mut checks);
        Ok(Ci {
            head: head.to_owned(),
            checks,
            detail: String::new(),
        })
    }

    pub(crate) fn detail(
        &mut self,
        cwd: &Path,
        review: &HostedReview,
        ci: &mut Ci,
        force: bool,
        budget: &mut Budget,
    ) -> Result<(), GhError> {
        let Some(check) = ci
            .checks
            .iter()
            .find(|c| c.effective_conclusion().reads_as_failed())
        else {
            return Ok(());
        };
        if let Some(id) = check.check_run_id {
            let raw = self.api(
                cwd,
                &format!(
                    "repos/{}/check-runs/{id}/annotations?per_page={ANNOTATIONS_PER_PAGE}",
                    review.owner_repo
                ),
                force,
                budget,
            )?;
            if !parse_json(&raw)?.is_array() {
                return Err(unreadable("invalid annotations"));
            }
            let annotations = read_annotations(&raw)?;
            ci.detail = annotations
                .first()
                .map(|a| {
                    format!(
                        "{}:{}: {}",
                        a.path.as_deref().unwrap_or("check"),
                        a.start_line.unwrap_or_default(),
                        a.message
                    )
                })
                .unwrap_or_else(|| {
                    "No annotations were returned; open the failed check for logs.".into()
                });
        } else {
            ci.detail = check
                .url
                .as_deref()
                .map(|url| format!("Failure details: {url}"))
                .unwrap_or_default();
        }
        Ok(())
    }

    pub(crate) fn stack(
        &mut self,
        cwd: &Path,
        review: &HostedReview,
        max: usize,
        force: bool,
        budget: &mut Budget,
    ) -> Result<bool, GhError> {
        let raw = self.api(
            cwd,
            &format!(
                "repos/{}/pulls?state=open&per_page={}",
                review.owner_repo,
                max.clamp(1, 100)
            ),
            force,
            budget,
        )?;
        if !parse_json(&raw)?.is_array() {
            return Err(unreadable("invalid stack list"));
        }
        let reviews = read_stack_reviews(&raw)?;
        let links: Vec<_> = reviews.iter().map(StackReview::link).collect();
        let order = zerocode_core::checks::stack_order(review.number, &links);
        // Only the PRs in our head repository can be stack parents. REST base
        // repo lists may also contain unrelated fork branches with the same name.
        let rows = parse_json(&raw)?;
        let base = review
            .base_ref
            .as_deref()
            .ok_or_else(|| unreadable("missing base branch"))?;
        Ok(rows.as_array().into_iter().flatten().any(|row| {
            let id = row.get("number").and_then(Value::as_u64);
            id.is_some_and(|id| id != review.number && order.chain.contains(&id))
                && row.pointer("/head/ref").and_then(Value::as_str) == Some(base)
                && row.pointer("/head/repo/full_name").and_then(Value::as_str)
                    == Some(review.owner_repo.as_str())
        }))
    }

    pub(crate) fn reviews(
        &self,
        cwd: &Path,
        review: &HostedReview,
        budget: &mut Budget,
    ) -> Result<Vec<ReviewItem>, GhError> {
        let (owner, repo) = review
            .owner_repo
            .split_once('/')
            .ok_or_else(|| unreadable("invalid repository"))?;
        let args = vec![
            "api".into(),
            "graphql".into(),
            "-f".into(),
            format!("query={REVIEW_QUERY}"),
            "-f".into(),
            format!("owner={owner}"),
            "-f".into(),
            format!("repo={repo}"),
            "-F".into(),
            format!("number={}", review.number),
        ];
        read_reviews(&checked(budget.call(cwd, &args)?)?)
    }
}

const REVIEW_QUERY: &str = r#"query($owner:String!,$repo:String!,$number:Int!){repository(owner:$owner,name:$repo){pullRequest(number:$number){reviews(first:100){nodes{databaseId state body author{login}} pageInfo{hasNextPage}} reviewThreads(first:100){nodes{isResolved comments(first:100){nodes{databaseId body author{login} path line} pageInfo{hasNextPage}}} pageInfo{hasNextPage}}}}}"#;

fn require_array<'a>(body: &'a Value, key: &str) -> Result<&'a [Value], GhError> {
    body.get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| unreadable("missing SCM array"))
}
fn complete_rows<'a>(body: &'a Value, field: &str) -> Result<&'a [Value], GhError> {
    let rows = require_array(body, field)?;
    if body
        .get("total_count")
        .and_then(Value::as_u64)
        .is_some_and(|total| total > rows.len() as u64)
    {
        return Err(unreadable("incomplete SCM page"));
    }
    Ok(rows)
}
fn nodes(connection: &Value) -> Result<&[Value], GhError> {
    if connection
        .pointer("/pageInfo/hasNextPage")
        .and_then(Value::as_bool)
        != Some(false)
    {
        return Err(unreadable("incomplete review page"));
    }
    require_array(connection, "nodes")
}
fn read_reviews(raw: &str) -> Result<Vec<ReviewItem>, GhError> {
    let body = parse_json(raw)?;
    if body
        .get("errors")
        .is_some_and(|errors| !errors.as_array().is_some_and(Vec::is_empty))
    {
        return Err(unreadable("review query failed"));
    }
    let pr = body
        .pointer("/data/repository/pullRequest")
        .ok_or_else(|| unreadable("missing review data"))?;
    let mut out = Vec::new();
    for row in nodes(&pr["reviews"])? {
        out.push(review_item(row, "review", false)?);
    }
    let mut latest = BTreeMap::new();
    for (index, item) in out.iter().enumerate() {
        if matches!(item.state.as_str(), "approved" | "changes_requested") {
            latest.insert(item.author.clone(), index);
        }
    }
    for (index, item) in out.iter_mut().enumerate() {
        if matches!(item.state.as_str(), "approved" | "changes_requested")
            && latest
                .get(&item.author)
                .is_some_and(|latest| *latest != index)
        {
            item.resolved = true;
        }
    }
    for thread in nodes(&pr["reviewThreads"])? {
        let resolved = thread["isResolved"]
            .as_bool()
            .ok_or_else(|| unreadable("missing thread resolution"))?;
        for row in nodes(&thread["comments"])? {
            out.push(review_item(row, "comment", resolved)?);
        }
    }
    Ok(out)
}
fn review_item(row: &Value, prefix: &str, resolved: bool) -> Result<ReviewItem, GhError> {
    let id = row["databaseId"]
        .as_u64()
        .ok_or_else(|| unreadable("missing review id"))?;
    Ok(ReviewItem {
        id: format!("{prefix}:{id}"),
        author: row
            .pointer("/author/login")
            .and_then(Value::as_str)
            .unwrap_or("deleted user")
            .into(),
        body: row["body"]
            .as_str()
            .ok_or_else(|| unreadable("missing review body"))?
            .into(),
        path: row["path"].as_str().map(str::to_owned),
        line: row["line"].as_u64(),
        state: row["state"]
            .as_str()
            .unwrap_or("commented")
            .to_ascii_lowercase(),
        resolved,
    })
}
pub(crate) fn read_state(word: &str) -> Option<PrState> {
    match word.to_ascii_lowercase().as_str() {
        "open" => Some(PrState::Open),
        "merged" => Some(PrState::Merged),
        "closed" => Some(PrState::Closed),
        _ => None,
    }
}
pub(crate) fn read_mergeability(word: Option<&str>) -> Option<Mergeability> {
    match word?.to_ascii_lowercase().as_str() {
        "conflicting" => Some(Mergeability::Conflicting),
        "blocked" => Some(Mergeability::Blocked),
        "mergeable" => Some(Mergeability::Mergeable),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_http_requires_a_cached_body_for_304_and_preserves_etags() {
        let fresh = read_http("HTTP/2.0 200 OK\r\nETag: W/\"v1\"\r\n\r\n[]", None).unwrap();
        assert_eq!(fresh.etag.as_deref(), Some("W/\"v1\""));
        assert_eq!(
            read_http("HTTP/2.0 304 Not Modified\r\n\r\n", Some(&fresh)).unwrap(),
            fresh
        );
        assert!(read_http("HTTP/2.0 304 Not Modified\r\n\r\n", None).is_err());
        assert!(read_http("HTTP/2.0 403 Forbidden\r\n\r\n{}", Some(&fresh)).is_err());
    }

    #[test]
    fn read_reviews_keeps_ids_authors_locations_and_resolved_threads() {
        let raw = r#"{"data":{"repository":{"pullRequest":{
            "reviews":{"nodes":[{"databaseId":1,"state":"CHANGES_REQUESTED","author":{"login":"r"},"body":"fix it"}],"pageInfo":{"hasNextPage":false}},
            "reviewThreads":{"nodes":[{"isResolved":true,"comments":{"nodes":[{"databaseId":2,"author":{"login":"r"},"path":"lib.rs","line":8,"body":"comment"}],"pageInfo":{"hasNextPage":false}}}],"pageInfo":{"hasNextPage":false}}
        }}}}"#;
        let rows = read_reviews(raw).unwrap();
        assert_eq!(rows[0].id, "review:1");
        assert_eq!(rows[0].state, "changes_requested");
        assert_eq!(rows[1].path.as_deref(), Some("lib.rs"));
        assert_eq!(rows[1].line, Some(8));
        assert!(rows[1].resolved);
        assert!(
            read_reviews(&raw.replace("\"hasNextPage\":false", "\"hasNextPage\":true")).is_err()
        );
        assert!(read_reviews("{\"errors\":[{\"message\":\"rate limit\"}],\"data\":null}").is_err());
        assert!(read_reviews("{}").is_err());
    }

    #[test]
    fn read_mergeability_never_treats_unknown_or_missing_as_resolution() {
        assert_eq!(
            read_mergeability(Some("CONFLICTING")),
            Some(Mergeability::Conflicting)
        );
        assert_eq!(
            read_mergeability(Some("BLOCKED")),
            Some(Mergeability::Blocked)
        );
        assert_eq!(
            read_mergeability(Some("MERGEABLE")),
            Some(Mergeability::Mergeable)
        );
        assert_eq!(read_mergeability(Some("UNKNOWN")), None);
        assert_eq!(read_mergeability(None), None);
    }
    #[derive(Default)]
    struct Fake {
        answers: Mutex<std::collections::VecDeque<CliOutput>>,
        calls: Mutex<Vec<Vec<String>>>,
    }
    impl Fake {
        fn push(&self, value: Value, http: bool) {
            self.answers.lock().unwrap().push_back(CliOutput {
                success: true,
                stdout: if http {
                    format!("HTTP/2.0 200 OK\r\nETag: W/\"fixture\"\r\n\r\n{value}")
                } else {
                    value.to_string()
                },
                stderr: String::new(),
            });
        }
    }
    impl CliRunner for Fake {
        fn run(&self, _: VendorCli, call: &CliCall<'_>) -> Result<CliOutput, CliError> {
            assert!(call.budget.is_some());
            self.calls.lock().unwrap().push(call.args.to_vec());
            Ok(self
                .answers
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected gh call"))
        }
    }
    #[test]
    fn fixture_tick_measures_seven_calls_and_acknowledged_mail_without_a_panel() {
        use zerocode_core::scm_observer::{Book, Effects, Observation, Subject};
        let started = Instant::now();
        let runner = Fake::default();
        runner.push(serde_json::json!({"number":12,"title":"fix","url":"https://github.com/base/repo/pull/12", "state":"OPEN", "headRefOid":"sha", "headRepository":{"name":"fork"}, "headRepositoryOwner":{"login":"contributor"}, "baseRefName":"main", "mergeable":"CONFLICTING"}), false);
        runner.push(serde_json::json!({"check_runs":[{"id":3,"name":"ci/test","status":"completed","conclusion":"failure"}]}), true);
        runner.push(serde_json::json!({"statuses":[]}), true);
        runner.push(serde_json::json!({"check_suites":[]}), true);
        runner.push(
            serde_json::json!([{"path":"src/lib.rs","start_line":8,"message":"assertion failed"}]),
            true,
        );
        runner.push(serde_json::json!({"data":{"repository":{"pullRequest":{
            "reviews":{"nodes":[{"databaseId":1,"state":"CHANGES_REQUESTED","author":{"login":"reviewer"},"body":"fix failure"}],"pageInfo":{"hasNextPage":false}},
            "reviewThreads":{"nodes":[{"isResolved":false,"comments":{"nodes":[{"databaseId":2,"body":"check this line","path":"src/lib.rs","line":8,"author":{"login":"reviewer"}}],"pageInfo":{"hasNextPage":false}}}],"pageInfo":{"hasNextPage":false}}
        }}}}), false);
        runner.push(serde_json::json!([]), true);
        let mut budget = Budget::over(
            &runner,
            &Limits {
                gh_calls_max: SUBJECT_CALLS_MAX,
                ..Limits::default()
            },
        );
        let mut client = Client::default();
        let root = Path::new("/fixture");
        let review = client.discover(root, &mut budget).unwrap().unwrap();
        assert_eq!(
            review.owner_repo, "base/repo",
            "fork checks must use the PR's base repository"
        );
        let mut ci = client.checks(root, &review, true, &mut budget).unwrap();
        client
            .detail(root, &review, &mut ci, true, &mut budget)
            .unwrap();
        assert!(ci.detail.contains("assertion failed"));
        let reviews = client.reviews(root, &review, &mut budget).unwrap();
        let stacked = client.stack(root, &review, 50, true, &mut budget).unwrap();
        let observation = Observation {
            ci: Some(ci),
            reviews: Some(reviews),
            state: Some(PrState::Open),
            mergeable: Some(Mergeability::Conflicting),
            stacked: Some(stacked),
        };
        #[derive(Default)]
        struct Mail {
            rows: BTreeMap<String, String>,
        }
        impl Effects for Mail {
            fn save(&mut self, _: &Book) -> Result<(), String> {
                Ok(())
            }
            fn mail(&mut self, _: &Subject, key: &str, body: &str) -> Result<(), String> {
                self.rows.entry(key.into()).or_insert_with(|| body.into());
                Ok(())
            }
        }
        let mut host = Mail::default();
        let mut book = Book::default();
        let subject = Subject {
            root: "/fixture".into(),
            repo: review.owner_repo,
            number: review.number,
            url: review.url,
            head: review.head_sha,
            branch: "topic".into(),
        };
        let key = subject.key();
        book.discover(&[subject], &mut host).unwrap();
        book.apply(
            &key,
            &observation,
            &zerocode_core::checks::Limits::default(),
            30_000,
            &mut host,
        )
        .unwrap();
        let latency = started.elapsed();
        assert_eq!(host.rows.len(), 4);
        book.apply(
            &key,
            &observation,
            &zerocode_core::checks::Limits::default(),
            60_000,
            &mut host,
        )
        .unwrap();
        assert_eq!(
            host.rows.len(),
            4,
            "the next tick must not mail the same facts"
        );
        assert_eq!(budget.used, 7);
        assert!(
            client
                .reviews(
                    root,
                    &super::HostedReview {
                        owner_repo: "base/repo".into(),
                        number: 12,
                        title: String::new(),
                        url: String::new(),
                        state: "open".into(),
                        draft: false,
                        head_sha: "sha".into(),
                        mergeable: None,
                        updated_at: None,
                        base_ref: None
                    },
                    &mut budget
                )
                .is_err()
        );
        assert_eq!(
            runner.calls.lock().unwrap().len(),
            7,
            "the eighth process was never launched"
        );
        println!(
            "SCM fixture: gh_calls={} due_tick_to_ledger_us={} simulated_poll_latency_ms=30000 mails={}",
            budget.used,
            latency.as_micros(),
            host.rows.len()
        );
    }
    #[test]
    fn conditional_candidates_can_be_discarded_and_force_fetch_omits_the_etag() {
        let runner = Fake::default();
        runner.push(serde_json::json!([]), true);
        runner.push(serde_json::json!([1]), true);
        runner.push(serde_json::json!([]), true);
        let mut budget = Budget::over(
            &runner,
            &Limits {
                gh_calls_max: 3,
                ..Limits::default()
            },
        );
        let mut acknowledged = Client::default();
        acknowledged
            .api(
                Path::new("/fixture"),
                "repos/o/r/checks",
                false,
                &mut budget,
            )
            .unwrap();
        let mut candidate = acknowledged.clone();
        candidate
            .api(
                Path::new("/fixture"),
                "repos/o/r/checks",
                false,
                &mut budget,
            )
            .unwrap();
        assert_ne!(
            candidate, acknowledged,
            "a failed write must leave the candidate unpublished"
        );
        acknowledged
            .api(Path::new("/fixture"), "repos/o/r/checks", true, &mut budget)
            .unwrap();
        let calls = runner.calls.lock().unwrap();
        assert!(calls[1].iter().any(|arg| arg.starts_with("If-None-Match:")));
        assert!(!calls[2].iter().any(|arg| arg.starts_with("If-None-Match:")));
    }
}
