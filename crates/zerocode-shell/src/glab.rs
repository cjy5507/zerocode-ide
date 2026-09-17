//! GitLab, reached the way `gh.rs` reaches GitHub: through the user's own CLI.
//!
//! Orca detects GitLab with no form and no token of its own — it asks whether
//! `glab` is on the machine and then reads one line out of `glab auth status`
//! (`.includes("Logged in")`, with a diagnose pass that widens to
//! `/logged in|authenticated/i` while refusing `/not logged in/i`). That is why
//! the person who filed this said GitLab "was just recognised": there is
//! nothing to recognise it WITH except the CLI they already signed in to.
//!
//! So this file owns the same two constraints `gh.rs` owns, for the same
//! reasons:
//!
//! - **One door.** This file declares the `glab` binary once and sends every
//!   call through `vendor_cli.rs`, the one shared spawn boundary. A second
//!   spawn site is a second idea of what "not installed" means, of what a
//!   refusal is, and of which PATH to look on — and the symptom is a card that
//!   says connected while a panel says missing.
//! - **The CLI owns the credential.** No token is read, passed, logged, or
//!   carried across IPC. A machine with no `glab` gets an honest "install it"
//!   instead of a login form this window would have to be trusted with.
//!
//! Everything that turns process output into an answer is a free function over
//! `&str`, so the contract is testable without a network, a token, or a
//! repository.

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::vendor_cli::{
    CliCall, CliError, CliOutput, CliRunner, ProcessRunner, VendorCli,
    json::{array, boolean, number, text},
};

/// How long the auth probe may take. Orca's own 10s: `glab auth status` talks
/// to every configured host, and a self-managed instance behind a VPN that is
/// not up answers by not answering. The settings pane must not wait on it.
const AUTH_STATUS_BUDGET: Duration = Duration::from_secs(10);

/// How long one round trip to the instance may take — a list, an item, a
/// comment, a close. Longer than the auth probe because this one leaves the
/// machine rather than reading a config file, and shorter than forever because
/// a skeleton or a disabled button is standing while it waits.
///
/// One number for reads and writes alike: a write is the same round trip, and
/// a second constant would be a second idea of when this instance is too slow
/// to wait for.
const CALL_BUDGET: Duration = Duration::from_secs(20);

/// The ceiling the wire carries, measured: `per_page=50` on the issue and todo
/// lists, and on the merge-request list when a whole PAGE is what is being read.
///
/// It stopped being the only answer when a second surface arrived. The merge
/// requests are read by two: the tasks page, which wants a page, and the
/// composer's picker, which wants twelve. Both numbers are the original's
/// (`PAGE_ROWS`, `PICKER_ROWS`) — one of them was simply never asked for here
/// before, and a constant that named itself the ceiling for "all three lists"
/// would have quietly made the picker read fifty.
const LIST_PER_PAGE: usize = 50;

/// And the whole-of-it page is bigger, measured apart (`PIPELINE_JOB_PAGE_SIZE`
/// and the diffs/members reads, out/main/index.js): each of those lists is
/// read once per open, not paged, so the ask is for the lot.
const WIDE_PAGE: usize = 100;

/// What `glab auth status` prints in front of each host it holds a login for.
/// Matched against a lowercased haystack, which is the case-insensitivity Orca
/// applies to the same string.
const SIGNED_IN_MARK: &str = "logged in to ";

/// The binary name is declared once; the shared runner owns how it is spawned.
const GLAB: VendorCli = VendorCli::new("glab");

/// Why a GitLab read could not be answered.
///
/// The auth probe needs two of these: `glab auth status` prints prose rather
/// than the JSON `gh` offers, so there is no such thing there as an answer
/// that parsed wrong — every text is readable, and what it says is the
/// measurement. The lists need the third, because a list read can be answered
/// by GitLab itself with a "no" that a person can act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GlabError {
    /// `glab` is not on PATH.
    Missing,
    /// `glab` started and the probe could not be completed — it hung past the
    /// budget, or the pipe broke under it.
    Refused(String),
    /// `glab` ran and the read did not happen, sorted into the five answers
    /// this window has sentences for.
    Denied(GlabDenial),
}

impl From<CliError> for GlabError {
    fn from(error: CliError) -> Self {
        match error {
            CliError::Missing => Self::Missing,
            CliError::Refused(message) => Self::Refused(message),
        }
    }
}

/// What a refused list read means, in the vocabulary the window translates.
///
/// A word, never the CLI's text. `glab` echoes the URL it called, the host it
/// called it on and — on the builds that print their config — the token it
/// authenticated with, and the renderer boundary is not a secret transport.
/// The window owns the sentence; this owns which sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GlabDenial {
    /// 403: signed in, and this token may not read this project.
    Forbidden,
    /// 404, or `glab` could not resolve a project for this checkout at all.
    NoProject,
    /// 429: too many reads, from this window or from anything else the
    /// person's `glab` does.
    RateLimited,
    /// The instance was never reached.
    Network,
    /// Everything else, including an answer that is not the JSON the API
    /// returns.
    Failed,
}

impl GlabDenial {
    /// The key the window looks up to say this in the reader's language.
    pub(crate) fn kind(self) -> &'static str {
        match self {
            Self::Forbidden => "forbidden",
            Self::NoProject => "no_project",
            Self::RateLimited => "rate_limited",
            Self::Network => "network",
            Self::Failed => "failed",
        }
    }
}

/// Run one bounded GitLab call through the shared process boundary.
fn run_glab(
    runner: &impl CliRunner,
    cwd: Option<&Path>,
    args: &[String],
    stdin: Option<&[u8]>,
    budget: Duration,
) -> Result<CliOutput, GlabError> {
    runner
        .run(
            GLAB,
            &CliCall {
                cwd,
                args,
                stdin,
                budget: Some(budget),
            },
        )
        .map_err(Into::into)
}

/// Canonical, secret-free GitLab integration state.
///
/// Three facts and nothing else. `glab auth status` also prints token scopes
/// and, on older builds, the token itself — none of that is read into this
/// struct, so none of it can reach the renderer by having appeared beside a
/// hostname in the output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct GlabStatus {
    installed: bool,
    authenticated: bool,
    /// The hosts the CLI says it is signed in to, in the order it named them.
    hosts: Vec<String>,
    /// `glab` looked for a token and found none — "No token found (checked
    /// config file, keyring, and environment variables)".
    ///
    /// Separate from `authenticated` because it is a different sentence: not
    /// signed in is a state, and having nowhere to look is the REASON, and the
    /// card can only tell somebody what to do if it can say which.
    tokenless: bool,
    /// The protocol `glab` says this machine's git operations are configured
    /// to use — "Git operations for gitlab.com configured to use ssh
    /// protocol".
    ///
    /// This is why a person can be told "not signed in" by a card and still
    /// push all day: git over ssh is a key, and the API token this card is
    /// about is a different credential entirely. Measured, reported as a bug
    /// against this card: "깃랩도 실제로 연결되는데 지금 안될로 나옴 실제로
    /// 프로젝트 풀한거에서 커밋해줘 하면 알아서 해주는데". Both halves were
    /// true; the card only said one of them.
    git_protocol: Option<String>,
}

impl GlabStatus {
    /// No `glab` on the machine. Not an error: it is the state the card has a
    /// remedy for, and an error would show "확인 실패" over an install button.
    fn missing() -> Self {
        Self {
            installed: false,
            authenticated: false,
            hosts: Vec::new(),
            tokenless: false,
            git_protocol: None,
        }
    }
}

/// Whether this output is a signed-in one, and to what.
///
/// Both streams, joined, because `glab` writes its status to **stderr** on the
/// versions that exit non-zero for a partially authenticated machine — reading
/// stdout alone reports a signed-in user as signed out. The three clauses are
/// Orca's: refuse "not logged in" first, then accept either of the two things
/// its diagnose pass accepts.
fn read_auth_status(stdout: &str, stderr: &str) -> GlabStatus {
    let haystack = format!("{stdout}\n{stderr}").to_lowercase();
    let authenticated = !haystack.contains("not logged in")
        && (haystack.contains("logged in") || haystack.contains("authenticated"));
    GlabStatus {
        installed: true,
        authenticated,
        hosts: signed_in_hosts(&haystack),
        tokenless: haystack.contains(NO_TOKEN_MARK),
        git_protocol: git_protocol_of(&haystack),
    }
}

/// What `glab` prints when every place a token could live was empty.
const NO_TOKEN_MARK: &str = "no token found";

/// The protocol out of `git operations for <host> configured to use <p>
/// protocol`.
///
/// Read by walking to the words between "to use" and "protocol" rather than
/// with a pattern, for the same reason [`signed_in_hosts`] scans: this binary
/// carries no regex engine. Only the shapes `glab` actually prints are
/// accepted (`ssh`, `https`), so a changed sentence reports nothing rather
/// than a fragment of one.
fn git_protocol_of(haystack: &str) -> Option<String> {
    for line in haystack.lines() {
        if !line.contains("git operations for") || !line.contains("protocol") {
            continue;
        }
        let after = line.split("to use").nth(1)?;
        let word = after.split_whitespace().next()?;
        if matches!(word, "ssh" | "https") {
            return Some(word.to_string());
        }
    }
    None
}

/// Every host named by a `logged in to <host>` line.
///
/// Scanned rather than matched with a regex crate this binary does not carry.
/// The charset is a hostname's, plus an optional `:port` — a self-managed
/// GitLab on a non-standard port is a host this window must be able to name,
/// and stopping at the colon would report it as a different machine.
fn signed_in_hosts(haystack: &str) -> Vec<String> {
    let mut hosts: Vec<String> = Vec::new();
    let mut rest = haystack;
    while let Some(at) = rest.find(SIGNED_IN_MARK) {
        let tail = &rest[at + SIGNED_IN_MARK.len()..];
        let bytes = tail.as_bytes();
        let mut end = 0;
        while end < bytes.len()
            && (bytes[end].is_ascii_alphanumeric() || matches!(bytes[end], b'.' | b'-'))
        {
            end += 1;
        }
        // A bare `:` after the name is punctuation, not a port. Only digits
        // behind it extend the host.
        let mut port_end = end;
        if bytes.get(end) == Some(&b':') {
            let mut digits = end + 1;
            while digits < bytes.len() && bytes[digits].is_ascii_digit() {
                digits += 1;
            }
            if digits > end + 1 {
                port_end = digits;
            }
        }
        if port_end > 0 {
            let host = &tail[..port_end];
            if !hosts.iter().any(|known| known == host) {
                hosts.push(host.to_string());
            }
        }
        // Always shorter than `rest` by at least the mark, so this terminates
        // even when the line named nothing readable.
        rest = &tail[port_end..];
    }
    hosts
}

/// The measured contract: not on PATH is a state, everything else is read out
/// of what the CLI said.
///
/// A non-zero exit is deliberately not a refusal. `glab` returns non-zero when
/// any configured host is unauthenticated and still prints the ones that are —
/// treating the code as the answer is how a machine signed in to gitlab.com
/// and not to its company instance reports as signed out entirely.
fn glab_status(runner: &impl CliRunner, cwd: Option<&Path>) -> Result<GlabStatus, GlabError> {
    let args = ["auth".to_string(), "status".to_string()];
    match run_glab(runner, cwd, &args, None, AUTH_STATUS_BUDGET) {
        Err(GlabError::Missing) => Ok(GlabStatus::missing()),
        Err(other) => Err(other),
        Ok(output) => Ok(read_auth_status(&output.stdout, &output.stderr)),
    }
}

/// Re-read the GitLab CLI's standing, optionally re-hydrating the login shell
/// PATH first for the explicit "다시 확인" gesture — the person pressing it has
/// usually just installed `glab`.
pub(crate) fn integration_status(
    cwd: &Path,
    force_path_refresh: bool,
) -> Result<GlabStatus, GlabError> {
    if force_path_refresh {
        let _ = crate::shell_path::hydrate(true);
    }
    // The checkout is the cwd because `glab` infers the host from the
    // repository's remote when it is standing in one; a self-managed instance
    // is otherwise invisible to a probe run from the process's own directory.
    glab_status(&ProcessRunner, Some(cwd))
}

/* ---- the lists the tasks page draws (1-g56b) -----------------------------
 *
 * Three reads, all through the same `glab api` this file already owns. The
 * queries are Orca's own, measured, and they live HERE rather than in the
 * window for the reason the GitHub half states about `gh::search_plan`: a
 * window that builds its own query string is a window whose chips and whose
 * question can disagree, and the disagreement shows up as an empty list
 * nobody can explain. */

/// One issue or merge request, as the row needs it.
///
/// `state` is GitLab's own word (`opened`/`merged`/`closed`/`locked`), not a
/// translation of it: the row prints what the instance said, and a window that
/// re-spelled it would have to be kept in step with an API it does not own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct GlabItem {
    /// `issue` or `mr`. Which one decides the row's sigil (`#42` / `!42`) and
    /// nothing else, so it is a word rather than a flag.
    kind: String,
    /// The **iid** — the number a person reads, per project. GitLab's `id` is
    /// global and appears nowhere a human looks.
    number: u64,
    title: String,
    state: String,
    updated_at: String,
    url: String,
    /// Who opened it — `author.username`, the same field the original maps
    /// (`main/gitlab/mappers.ts:247`).
    ///
    /// The composer's ROW does not draw it: the original's GitLab row is a
    /// mark, a `!N`, and a title, where its GitHub row carries chips. It is
    /// carried because the item this row hands on is the same item every other
    /// surface reads, and dropping a field the mapper supplies means the next
    /// surface that wants it re-derives it from somewhere worse.
    author: Option<String>,
    /// The branch the merge request is FROM, and the branch it is INTO
    /// (`mappers.ts:248-249`). Absent on an issue, which has neither.
    ///
    /// These two are why this struct grew: without them a picked merge request
    /// cannot say where to cut from, and a checkout made anyway is cut from the
    /// repository's default branch — a workspace that looks like the review and
    /// contains none of it.
    source_branch: Option<String>,
    target_branch: Option<String>,
    /// Is the head on a DIFFERENT project — a fork.
    ///
    /// The original's rule, and both halves of it matter
    /// (`mappers.ts:250-253`): the two project ids must BOTH be present and
    /// must differ. A missing pair is `false`, not `true` — reading it the
    /// other way sends every merge request from a payload that omits them down
    /// the fork road, which is the road that cannot resolve a head.
    cross_repo: bool,
}

/// One pending todo. Not an item: a todo names an action somebody took on
/// something, lives across every project the person can see, and its row opens
/// the page rather than a dialog (measured).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct GlabTodo {
    action: String,
    target_type: String,
    target_number: Option<u64>,
    target_title: String,
    project_path: String,
    target_url: String,
    updated_at: String,
}

/// Which of the two things a row can be.
///
/// An enum rather than the window's word for the same reason [`MrState`] is
/// one: `issue`/`mr` arrives from the renderer, and what it selects is a URL
/// segment and a CLI verb's noun. Passing the string through would let a word
/// this file does not know become part of a call it makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GlabKind {
    Issue,
    Mr,
}

impl GlabKind {
    /// The word the window sends and the row wears, or nothing.
    pub(crate) fn from_word(said: &str) -> Option<Self> {
        match said {
            "issue" => Some(Self::Issue),
            "mr" => Some(Self::Mr),
            _ => None,
        }
    }

    /// The row's word — and, not by coincidence, the noun `glab` itself uses
    /// (`glab issue close`, `glab mr close`). One string because they are one
    /// vocabulary; two would drift the day a third kind appears.
    fn word(self) -> &'static str {
        match self {
            Self::Issue => "issue",
            Self::Mr => "mr",
        }
    }

    /// The API's collection. Not the noun: GitLab's REST paths spell them
    /// `issues` and `merge_requests`.
    fn collection(self) -> &'static str {
        match self {
            Self::Issue => "issues",
            Self::Mr => "merge_requests",
        }
    }
}

/// Which merge requests to ask for. An enum, not the renderer's word: the
/// query is built from literals only, so no string a window sends can become
/// part of a URL this process calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MrState {
    Opened,
    Merged,
    Closed,
    All,
}

impl MrState {
    /// The word the window sends, or nothing. An unrecognised filter is
    /// refused rather than guessed at — guessing shows a list that is not the
    /// one the chips claim.
    pub(crate) fn from_word(said: &str) -> Option<Self> {
        match said {
            "opened" => Some(Self::Opened),
            "merged" => Some(Self::Merged),
            "closed" => Some(Self::Closed),
            "all" => Some(Self::All),
            _ => None,
        }
    }

    /// What rides in the query. `all` rides as nothing: the parameter's
    /// absence IS every state, and `state=all` is not a value the API has.
    fn param(self) -> Option<&'static str> {
        match self {
            Self::Opened => Some("opened"),
            Self::Merged => Some("merged"),
            Self::Closed => Some("closed"),
            Self::All => None,
        }
    }
}

/// `projects/:id` — glab's own placeholder for "the project this checkout
/// belongs to".
///
/// The alternative is to resolve the project ourselves out of the remote URL
/// and encode it, which is a second idea of which project is open: `glab`
/// already picks the host AND the project from the repository it is standing
/// in, which is exactly what running it with `cwd` = the checkout buys. A
/// window with four worktrees open therefore asks about the right one by
/// standing in it, and nothing about a remote is parsed here.
const PROJECT_REF: &str = "projects/:id";

fn issues_path(assigned_to_me: bool) -> String {
    // `state=opened` always, and the chip toggles the ASSIGNEE (measured):
    // "나에게 할당됨" narrows who, never what state — a chip that also
    // reopened closed issues would be two filters wearing one label.
    let mut path = format!(
        "{PROJECT_REF}/issues?per_page={LIST_PER_PAGE}&order_by=updated_at&sort=desc&state=opened"
    );
    if assigned_to_me {
        path.push_str("&scope=assigned_to_me");
    }
    path
}

/// The MR list, for a caller that says how many rows it wants and what it is
/// looking for.
///
/// Two callers, two page sizes, and that is the original's own arrangement:
/// the tasks page reads a page of fifty, the composer's picker asks for twelve
/// (`RESULT_LIMIT`, `new-workspace/SmartWorkspaceNameField.tsx:159`). Reading
/// the client alone would have given one number for both.
///
/// The whole query, verbatim from `main/gitlab/client.ts:494-499`:
///
/// ```text
/// const stateParam = state === 'all' ? '' : `&state=${state}`
/// const searchParam = query?.trim() ? `&search=${encodeURIComponent(query.trim())}` : ''
/// `projects/${…}/merge_requests?page=${page}&per_page=${perPage}` +
///   `&order_by=updated_at&sort=desc&with_merge_status_recheck=false${stateParam}${searchParam}`
/// ```
fn mrs_path(state: MrState, per_page: usize, query: Option<&str>) -> String {
    // `with_merge_status_recheck=false` is measured and is the load-bearing
    // one: asking GitLab to recompute mergeability for fifty rows turns a list
    // read into fifty background jobs on somebody's instance.
    let mut path = format!(
        "{PROJECT_REF}/merge_requests?page=1&per_page={per_page}\
         &order_by=updated_at&sort=desc&with_merge_status_recheck=false"
    );
    if let Some(word) = state.param() {
        path.push_str("&state=");
        path.push_str(word);
    }
    // ENCODED, and encoded with the SAME set the rest of this application uses
    // (`remote_repo::encode_component`). This is the first caller-supplied text
    // ever to reach a path string in this file, and raw it would let a typed
    // `&` write another parameter — a person searching for `a&state=closed`
    // would silently change which merge requests the chip is asking for.
    if let Some(text) = search_text(query) {
        path.push_str("&search=");
        path.push_str(&crate::remote_repo::encode_component(text));
    }
    path
}

/// The typed text, or nothing.
///
/// The original's test is `query?.trim() ? … : ''` — an empty search parameter
/// is not sent at all rather than sent empty, which GitLab reads as "match the
/// empty string" and answers with nothing.
fn search_text(query: Option<&str>) -> Option<&str> {
    let trimmed = query?.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn todos_path() -> String {
    format!("todos?state=pending&per_page={LIST_PER_PAGE}")
}

/// The head of one item — everything the dialog's top half is made of.
fn item_path(kind: GlabKind, number: u64) -> String {
    format!("{PROJECT_REF}/{}/{number}", kind.collection())
}

/// The discussions collection itself — what a POST lands on.
fn discussions_home(kind: GlabKind, number: u64) -> String {
    format!("{PROJECT_REF}/{}/{number}/discussions", kind.collection())
}

/// The conversation. Discussions rather than `/notes`, because a note alone
/// cannot say whether the thread it belongs to was resolved, and the badge
/// beside a review comment is the whole reason a person opens this half.
fn discussions_path(kind: GlabKind, number: u64) -> String {
    format!(
        "{}?per_page={LIST_PER_PAGE}",
        discussions_home(kind, number)
    )
}

/// Where a new comment lands. `/notes` and not `/discussions`: a plain comment
/// is a note on the item, and posting it as a discussion would open a thread
/// GitLab then asks somebody to resolve.
fn notes_path(kind: GlabKind, number: u64) -> String {
    format!("{PROJECT_REF}/{}/{number}/notes", kind.collection())
}

/// A pipeline's jobs, the lot (measured page size — one read per open).
fn pipeline_jobs_path(pipeline: u64) -> String {
    format!("{PROJECT_REF}/pipelines/{pipeline}/jobs?per_page={WIDE_PAGE}")
}

/// Where a retry lands. The job's own id — a retry is about one job, and the
/// pipeline it belongs to is GitLab's to work out.
fn job_retry_path(job: u64) -> String {
    format!("{PROJECT_REF}/jobs/{job}/retry")
}

fn job_trace_path(job: u64) -> String {
    format!("{PROJECT_REF}/jobs/{job}/trace")
}

/// A merge request's changed files, one page of a hundred (the measured page
/// size; deeper diffs than that wait for a person to open GitLab).
fn mr_diffs_path(number: u64) -> String {
    format!("{PROJECT_REF}/merge_requests/{number}/diffs?per_page={WIDE_PAGE}")
}

fn mr_approvals_path(number: u64) -> String {
    format!("{PROJECT_REF}/merge_requests/{number}/approvals")
}

fn mr_approval_state_path(number: u64) -> String {
    format!("{PROJECT_REF}/merge_requests/{number}/approval_state")
}

/// Everybody the project could name as a reviewer. One page of a hundred and
/// no `--paginate` (measured uses both): the jq pipeline `--paginate` needs is
/// a second parser, and a bench beyond a hundred names needs a search box this
/// card does not have. Recorded deviation.
fn members_path() -> String {
    format!("{PROJECT_REF}/members/all?per_page={WIDE_PAGE}")
}

/// What a refused read means, read out of what the CLI said and then dropped.
///
/// Both streams, because `glab` prints the API's error body on stdout and its
/// own complaints on stderr, and which one carries the answer depends on how
/// far the call got. The text never leaves this function.
fn classify(stdout: &str, stderr: &str) -> GlabDenial {
    let said = format!("{stdout}\n{stderr}").to_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|needle| said.contains(needle));
    if has(&["403", "401", "forbidden", "unauthorized"]) {
        GlabDenial::Forbidden
    } else if has(&["404", "not found", "could not resolve a gitlab project"]) {
        GlabDenial::NoProject
    } else if has(&["429", "rate limit", "too many requests"]) {
        GlabDenial::RateLimited
    } else if has(&[
        "could not resolve host",
        "no such host",
        "connection refused",
        "network is unreachable",
        "dial tcp",
        "i/o timeout",
        "timed out",
    ]) {
        GlabDenial::Network
    } else {
        GlabDenial::Failed
    }
}

/// Whether the call happened, and what it printed if it did. The one place a
/// non-zero exit becomes a word — every road out of this file goes through it,
/// so there is one answer to "what does a refused `glab` mean".
fn checked(output: CliOutput) -> Result<String, GlabError> {
    if output.success {
        return Ok(output.stdout);
    }
    Err(GlabError::Denied(classify(&output.stdout, &output.stderr)))
}

/// One `glab api` read. What shape the body should have is the caller's
/// business; that it is JSON at all is this one's.
fn read_json(runner: &impl CliRunner, cwd: &Path, path: String) -> Result<Value, GlabError> {
    let args = ["api".to_string(), path];
    let body = checked(run_glab(runner, Some(cwd), &args, None, CALL_BUDGET)?)?;
    // An answer that is not JSON at all is the generic word, with the junk
    // left where it was found — `glab` prints the URL it called and the host
    // it called it on, and a refusal is not a transport for either.
    serde_json::from_str::<Value>(&body).map_err(|_| GlabError::Denied(GlabDenial::Failed))
}

/// One `glab api` list read. The rows come back untouched; what each list
/// means of them is its own function's business.
fn read_list(runner: &impl CliRunner, cwd: &Path, path: String) -> Result<Vec<Value>, GlabError> {
    // A body that is not the array the API documents is an error object GitLab
    // answered with exit 0 — those carry a `message` worth sorting. It is
    // sorted and then dropped, never quoted.
    match read_json(runner, cwd, path)? {
        Value::Array(rows) => Ok(rows),
        other => Err(GlabError::Denied(classify(&other.to_string(), ""))),
    }
}

/// A call whose whole answer is whether it happened: a comment posted, an item
/// closed. What GitLab printed about it is not shown to anybody.
fn ran(
    runner: &impl CliRunner,
    cwd: &Path,
    args: &[String],
    stdin: Option<&[u8]>,
) -> Result<(), GlabError> {
    checked(run_glab(runner, Some(cwd), args, stdin, CALL_BUDGET)?)?;
    Ok(())
}

/// A row is kept only when it has the two facts its gestures need: the number
/// it is called by, and the page it opens. A row that cannot open is a dead
/// click, and the tasks page has exactly one thing every row does.
fn read_item(value: &Value, kind: GlabKind) -> Option<GlabItem> {
    let project = |name: &str| number(value, name);
    Some(GlabItem {
        kind: kind.word().to_string(),
        number: number(value, "iid")?,
        title: text(value, "title").unwrap_or_default(),
        state: text(value, "state").unwrap_or_default(),
        updated_at: text(value, "updated_at").unwrap_or_default(),
        url: text(value, "web_url")?,
        author: value.get("author").and_then(|one| text(one, "username")),
        source_branch: text(value, "source_branch"),
        target_branch: text(value, "target_branch"),
        cross_repo: match (project("source_project_id"), project("target_project_id")) {
            (Some(from), Some(into)) => from != into,
            // Both or neither. A payload that names one and not the other has
            // not said the merge request is a fork's.
            _ => false,
        },
    })
}

fn read_todo(value: &Value) -> Option<GlabTodo> {
    let target = value.get("target");
    Some(GlabTodo {
        action: text(value, "action_name").unwrap_or_default(),
        target_type: text(value, "target_type").unwrap_or_default(),
        target_number: target.and_then(|one| number(one, "iid")),
        target_title: target
            .and_then(|one| text(one, "title"))
            .unwrap_or_default(),
        project_path: value
            .get("project")
            .and_then(|one| text(one, "path_with_namespace"))
            .unwrap_or_default(),
        // The whole row is this link (measured: a todo opens the page, there is
        // no dialog), so a todo without one is a row that does nothing.
        target_url: text(value, "target_url")?,
        updated_at: text(value, "updated_at").unwrap_or_default(),
    })
}

fn list_issues(
    runner: &impl CliRunner,
    cwd: &Path,
    assigned_to_me: bool,
) -> Result<Vec<GlabItem>, GlabError> {
    Ok(read_list(runner, cwd, issues_path(assigned_to_me))?
        .iter()
        .filter_map(|row| read_item(row, GlabKind::Issue))
        .collect())
}

fn list_mrs(
    runner: &impl CliRunner,
    cwd: &Path,
    state: MrState,
    per_page: usize,
    query: Option<&str>,
) -> Result<Vec<GlabItem>, GlabError> {
    Ok(read_list(runner, cwd, mrs_path(state, per_page, query))?
        .iter()
        .filter_map(|row| read_item(row, GlabKind::Mr))
        .collect())
}

fn list_todos(runner: &impl CliRunner, cwd: &Path) -> Result<Vec<GlabTodo>, GlabError> {
    Ok(read_list(runner, cwd, todos_path())?
        .iter()
        .filter_map(read_todo)
        .collect())
}

/// The open issues of the checkout's project, optionally only the ones
/// assigned to the person signed in.
pub(crate) fn fetch_issues(cwd: &Path, assigned_to_me: bool) -> Result<Vec<GlabItem>, GlabError> {
    list_issues(&ProcessRunner, cwd, assigned_to_me)
}

/// The merge requests of the checkout's project, in one state or all of them,
/// optionally only the ones matching what somebody typed.
pub(crate) fn fetch_mrs(
    cwd: &Path,
    state: MrState,
    per_page: usize,
    query: Option<&str>,
) -> Result<Vec<GlabItem>, GlabError> {
    list_mrs(&ProcessRunner, cwd, state, per_page, query)
}

/// How many rows the tasks page reads at once.
pub(crate) const PAGE_ROWS: usize = LIST_PER_PAGE;

/// How many the composer's picker reads.
///
/// The original's `RESULT_LIMIT` (`SmartWorkspaceNameField.tsx:159`). A picker
/// is read top to bottom by somebody deciding, not scanned; fifty rows in a
/// dialog is a list nobody finishes.
pub(crate) const PICKER_ROWS: usize = 12;

/// The person's own pending todos. Not scoped to the project: GitLab's todos
/// are an inbox across everything they can see, and the row names which
/// project each one came from because of that.
pub(crate) fn fetch_todos(cwd: &Path) -> Result<Vec<GlabTodo>, GlabError> {
    list_todos(&ProcessRunner, cwd)
}

/* ---- one item, opened (1-g56c) -------------------------------------------
 *
 * A row's dialog: the head, the description, and the voices under it. Two
 * reads, because GitLab keeps them apart — the item knows its title, state and
 * labels, and the discussions know who said what. */

/// One voice in the conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct GlabComment {
    author: String,
    created_at: String,
    body: String,
    /// Whether the thread this note belongs to is settled — `None` when the
    /// instance said nothing, which is what a plain comment on an issue looks
    /// like. Three states, so a badge is drawn for "resolved" only and never
    /// for "we do not know".
    resolved: Option<bool>,
}

/// One issue or merge request with its dialog's worth of facts.
///
/// `state` stays GitLab's own word here as it does on the row ([`GlabItem`]),
/// and for the same reason: the badge prints what the instance said.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct GlabItemDetail {
    kind: String,
    number: u64,
    title: String,
    state: String,
    author: String,
    created_at: String,
    body: String,
    url: String,
    labels: Vec<String>,
    comments: Vec<GlabComment>,
    /// The head pipeline a merge request is riding, when GitLab named one —
    /// the pipeline tab's whole reason to stand. An issue never has it.
    pipeline_id: Option<u64>,
    /// Who is asked to review. Read off the same head the rest of the card
    /// is — the measured code's fallback road made the main one, because the
    /// head already carries it and a second endpoint is a second wait.
    /// Recorded deviation. An issue's list is simply empty.
    reviewers: Vec<GlabUser>,
    /// The three commits an inline comment pins itself against — off the
    /// head's `diff_refs`, merge requests only. Without them the inline
    /// form refuses before anything travels.
    diff_refs: Option<GlabDiffRefs>,
}

/// Where a merge request's diff stands: the measured `diff_refs` triple,
/// carried whole because a position POST needs all three or none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct GlabDiffRefs {
    base_sha: String,
    start_sha: String,
    head_sha: String,
}

fn read_diff_refs(head: &Value) -> Option<GlabDiffRefs> {
    let refs = head.get("diff_refs")?;
    Some(GlabDiffRefs {
        base_sha: text(refs, "base_sha")?,
        start_sha: text(refs, "start_sha")?,
        head_sha: text(refs, "head_sha")?,
    })
}

/// Somebody GitLab can name. The id is what a reviewer PUT carries and the
/// username is what the chip says — nothing else of the person travels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct GlabUser {
    id: u64,
    username: String,
}

fn read_user(row: &Value) -> Option<GlabUser> {
    Some(GlabUser {
        id: number(row, "id")?,
        username: text(row, "username")?,
    })
}

/// The head's `reviewers` rows. A row without an id or username is dropped —
/// a chip that cannot say who it is cannot be removed either.
fn read_reviewers(head: &Value) -> Vec<GlabUser> {
    array(head, "reviewers")
        .iter()
        .filter_map(read_user)
        .collect()
}

/// Who wrote a thing. `username` and not `name`: it is the handle GitLab shows
/// beside a comment, and the one a person can search for.
fn read_author(value: &Value) -> String {
    value
        .get("author")
        .and_then(|one| text(one, "username"))
        .unwrap_or_default()
}

/// The label chips. REST v4 answers `["bug", "backend"]`, and the same
/// endpoint asked `with_labels_details=true` answers objects — this reads
/// either, because which one arrives is a query parameter away.
fn read_labels(value: &Value) -> Vec<String> {
    array(value, "labels")
        .iter()
        .filter_map(|row| match row {
            Value::String(name) => {
                let named = name.trim();
                (!named.is_empty()).then(|| named.to_string())
            }
            _ => text(row, "name"),
        })
        .collect()
}

/// Every discussion's notes, flattened into the one list the dialog draws.
fn read_comments(discussions: &[Value]) -> Vec<GlabComment> {
    let mut voices = Vec::new();
    for talk in discussions {
        // The thread answers "is this settled", and older instances answer it
        // only on the notes. Ask the thread first and fall back to the note,
        // so a resolved review comment wears its badge either way.
        let settled = boolean(talk, "resolved");
        for note in array(talk, "notes") {
            // `system: true` is GitLab narrating itself — "changed the
            // description", "assigned to @hana", "mentioned in commit …".
            // Nobody wrote those, so they are not voices in a conversation,
            // and a dialog that showed them would bury the two that were.
            if boolean(note, "system") == Some(true) {
                continue;
            }
            voices.push(GlabComment {
                author: read_author(note),
                created_at: text(note, "created_at").unwrap_or_default(),
                body: text(note, "body").unwrap_or_default(),
                resolved: settled.or_else(|| boolean(note, "resolved")),
            });
        }
    }
    voices
}

fn item_detail(
    runner: &impl CliRunner,
    cwd: &Path,
    kind: GlabKind,
    item_number: u64,
) -> Result<GlabItemDetail, GlabError> {
    let head = read_json(runner, cwd, item_path(kind, item_number))?;
    // `{"message":"404 Project Not Found"}` is an object too, and GitLab
    // answers some refusals with a zero exit. The item asked for is the one
    // that knows its own number; anything else is sorted into a word.
    if number(&head, "iid") != Some(item_number) {
        return Err(GlabError::Denied(classify(&head.to_string(), "")));
    }
    let talk = read_list(runner, cwd, discussions_path(kind, item_number))?;
    Ok(GlabItemDetail {
        kind: kind.word().to_string(),
        number: item_number,
        title: text(&head, "title").unwrap_or_default(),
        state: text(&head, "state").unwrap_or_default(),
        author: read_author(&head),
        created_at: text(&head, "created_at").unwrap_or_default(),
        // GitLab calls the body `description` on both kinds; the window calls
        // it a body because that is what it draws.
        body: text(&head, "description").unwrap_or_default(),
        url: text(&head, "web_url").unwrap_or_default(),
        labels: read_labels(&head),
        comments: read_comments(&talk),
        // Measured precedence: `head_pipeline ?? pipeline` — older instances
        // answer only the second word for the same fact.
        pipeline_id: head
            .get("head_pipeline")
            .filter(|held| !held.is_null())
            .or_else(|| head.get("pipeline"))
            .and_then(|held| number(held, "id")),
        reviewers: read_reviewers(&head),
        diff_refs: read_diff_refs(&head),
    })
}

fn comment_item(
    runner: &impl CliRunner,
    cwd: &Path,
    kind: GlabKind,
    number: u64,
    body: &str,
) -> Result<(), GlabError> {
    // `-F body=@-` reads the field's value from STDIN — `glab api` checks the
    // `@` before it does anything else with the text, so nothing typed here is
    // number-converted, placeholder-expanded, or (the point) written into
    // argv, where every process list on the machine would carry it.
    let args = [
        "api".to_string(),
        "-X".to_string(),
        "POST".to_string(),
        notes_path(kind, number),
        "-F".to_string(),
        "body=@-".to_string(),
    ];
    ran(runner, cwd, &args, Some(body.as_bytes()))
}

fn set_item_open(
    runner: &impl CliRunner,
    cwd: &Path,
    kind: GlabKind,
    number: u64,
    open: bool,
) -> Result<(), GlabError> {
    // `glab issue close 42`, standing in the checkout — the same way the lists
    // are asked. No `-R`: which project this is comes from where `glab` is
    // run, and naming it a second way is a second answer to that question.
    // Whether this person may is GitLab's judgement, not ours.
    let verb = if open { "reopen" } else { "close" };
    let args = [
        kind.word().to_string(),
        verb.to_string(),
        number.to_string(),
    ];
    ran(runner, cwd, &args, None)
}

/* ---- the pipeline tab (1-g56d) ------------------------------------------
 *
 * A merge request's head pipeline, read as the jobs it ran. Three calls, all
 * through the one door: the list, a retry, a log. */

/// One job the pipeline ran.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct GlabJob {
    id: u64,
    name: String,
    stage: String,
    /// GitLab's own word (`success`, `failed`, `running`, `manual`, …) — the
    /// window picks a tone for the four it knows and leaves the rest neutral,
    /// the state-word rule the item card already keeps.
    status: String,
    web_url: String,
    /// Seconds, when the instance counted them.
    duration: Option<f64>,
}

fn read_jobs(rows: &[Value]) -> Vec<GlabJob> {
    rows.iter()
        .filter_map(|row| {
            Some(GlabJob {
                id: number(row, "id")?,
                name: text(row, "name").unwrap_or_default(),
                stage: text(row, "stage").unwrap_or_default(),
                status: text(row, "status").unwrap_or_default(),
                web_url: text(row, "web_url").unwrap_or_default(),
                duration: row.get("duration").and_then(Value::as_f64),
            })
        })
        .collect()
}

fn list_pipeline_jobs(
    runner: &impl CliRunner,
    cwd: &Path,
    pipeline: u64,
) -> Result<Vec<GlabJob>, GlabError> {
    Ok(read_jobs(&read_list(
        runner,
        cwd,
        pipeline_jobs_path(pipeline),
    )?))
}

fn retry_job(runner: &impl CliRunner, cwd: &Path, job: u64) -> Result<(), GlabError> {
    let args = [
        "api".to_string(),
        "-X".to_string(),
        "POST".to_string(),
        job_retry_path(job),
    ];
    ran(runner, cwd, &args, None)
}

/// The job's log, whole. **A missing log is an empty log, not a refusal** —
/// measured (`isMissingJobLogError` → `ok, trace: ""`): a job that never wrote
/// a line 404s here, and sorting that into "project not found" would tell
/// somebody their checkout broke because a job was quiet.
fn job_trace(runner: &impl CliRunner, cwd: &Path, job: u64) -> Result<String, GlabError> {
    let args = ["api".to_string(), job_trace_path(job)];
    let output = run_glab(runner, Some(cwd), &args, None, CALL_BUDGET)?;
    if output.success {
        return Ok(output.stdout);
    }
    let said = format!("{}\n{}", output.stdout, output.stderr).to_lowercase();
    if said.contains("404") || said.contains("not found") {
        return Ok(String::new());
    }
    Err(GlabError::Denied(classify(&output.stdout, &output.stderr)))
}

/* ---- merging and editing a merge request (1-g56e) ------------------------ */

/* ---- the review board (1-g56f) ------------------------------------------
 *
 * The description pane's reviewers card and the Files tab. Both ride one
 * lazy read after the dialog stands — the pipeline tab's shape — so the
 * card's first paint owes nothing to three more round-trips. */

/// One approval rule's standing, as `approval_state` tells it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct GlabApprovalRule {
    name: String,
    required: u64,
    approved: bool,
}

/// What the two approval endpoints answered. Either may refuse alone —
/// approvals are a paid-tier fact on some instances — so every field is
/// its own maybe rather than the whole card failing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct GlabApprovals {
    left: Option<u64>,
    required: Option<u64>,
    rules: Vec<GlabApprovalRule>,
}

/// One changed file. `old_path` stands only when it differs — a rename —
/// which is the measured shape, and the counts are counted here so the
/// window never parses a diff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct GlabMrFile {
    path: String,
    old_path: Option<String>,
    additions: u64,
    deletions: u64,
    /// The raw unified diff, empty when GitLab held it back (binary, too
    /// large) — the window words that emptiness, it does not guess at it.
    diff: String,
}

/// The lazy read's answer: approvals and files together, one invoke.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct GlabMrReview {
    approvals: Option<GlabApprovals>,
    files: Vec<GlabMrFile>,
}

/// `+` lines against `-` lines, headers excluded — the measured count
/// (`countDiffLines`), so the numbers match what GitLab's own UI shows.
fn diff_line_counts(diff: &str) -> (u64, u64) {
    let mut additions = 0;
    let mut deletions = 0;
    for line in diff.lines() {
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if line.starts_with('+') {
            additions += 1;
        } else if line.starts_with('-') {
            deletions += 1;
        }
    }
    (additions, deletions)
}

fn read_mr_files(runner: &impl CliRunner, cwd: &Path, number: u64) -> Vec<GlabMrFile> {
    // A refused diffs read is an empty tab, not a dead card — the measured
    // leniency (`.catch(() => [])`), kept because the reads that refuse here
    // have already refused on the head this dialog stands on.
    let Ok(rows) = read_list(runner, cwd, mr_diffs_path(number)) else {
        return Vec::new();
    };
    rows.iter()
        .filter_map(|row| {
            let new_path = text(row, "new_path");
            let old_path = text(row, "old_path");
            let path = new_path.or_else(|| old_path.clone())?;
            let diff = text(row, "diff").unwrap_or_default();
            let (additions, deletions) = diff_line_counts(&diff);
            Some(GlabMrFile {
                old_path: old_path.filter(|was| *was != path),
                path,
                additions,
                deletions,
                diff,
            })
        })
        .collect()
}

fn read_approvals(runner: &impl CliRunner, cwd: &Path, item_number: u64) -> Option<GlabApprovals> {
    let head = read_json(runner, cwd, mr_approvals_path(item_number)).ok();
    let state = read_json(runner, cwd, mr_approval_state_path(item_number)).ok();
    if head.is_none() && state.is_none() {
        return None;
    }
    let count = |name: &str| head.as_ref().and_then(|held| number(held, name));
    let rules = state.as_ref().map_or_else(Vec::new, |held| {
        array(held, "rules")
            .iter()
            .map(|rule| GlabApprovalRule {
                name: text(rule, "name").unwrap_or_default(),
                required: number(rule, "approvals_required").unwrap_or(0),
                approved: boolean(rule, "approved") == Some(true),
            })
            .collect()
    });
    Some(GlabApprovals {
        left: count("approvals_left"),
        required: count("approvals_required"),
        rules,
    })
}

fn mr_review(runner: &impl CliRunner, cwd: &Path, number: u64) -> GlabMrReview {
    GlabMrReview {
        approvals: read_approvals(runner, cwd, number),
        files: read_mr_files(runner, cwd, number),
    }
}

/// Replace the reviewer set whole — the measured shape: one `reviewer_ids[]`
/// per id, and the bare `reviewer_ids=` when the set empties. Ids are
/// numbers, not somebody's words, so argv may carry them.
fn set_mr_reviewers(
    runner: &impl CliRunner,
    cwd: &Path,
    number: u64,
    ids: &[u64],
) -> Result<(), GlabError> {
    let mut args = vec![
        "api".to_string(),
        "-X".to_string(),
        "PUT".to_string(),
        item_path(GlabKind::Mr, number),
    ];
    if ids.is_empty() {
        args.push("-f".to_string());
        args.push("reviewer_ids=".to_string());
    } else {
        for id in ids {
            args.push("-f".to_string());
            args.push(format!("reviewer_ids[]={id}"));
        }
    }
    ran(runner, cwd, &args, None)
}

/// One inline comment, pinned to a line of a changed file. The position
/// facts ride argv — they are the diff's own data (paths the diffs read
/// answered, commit ids, a number), not somebody's words — while the body
/// itself travels on stdin. Measured sends the body as an argv `-f` too;
/// **recorded deviation**, the comment rule of this house.
fn inline_comment(
    runner: &impl CliRunner,
    cwd: &Path,
    number: u64,
    place: &MrInlinePlace,
    body: &str,
) -> Result<(), GlabError> {
    let position = |name: &str, value: &str| format!("position[{name}]={value}");
    let args = [
        "api".to_string(),
        "-X".to_string(),
        "POST".to_string(),
        discussions_home(GlabKind::Mr, number),
        "-F".to_string(),
        "body=@-".to_string(),
        "-f".to_string(),
        position("position_type", "text"),
        "-f".to_string(),
        position("base_sha", &place.refs.base_sha),
        "-f".to_string(),
        position("start_sha", &place.refs.start_sha),
        "-f".to_string(),
        position("head_sha", &place.refs.head_sha),
        "-f".to_string(),
        // The measured fallback: an unrenamed file's old path IS its path.
        position("old_path", place.old_path.as_deref().unwrap_or(&place.path)),
        "-f".to_string(),
        position("new_path", &place.path),
        "-f".to_string(),
        position("new_line", &place.line.to_string()),
    ];
    ran(runner, cwd, &args, Some(body.as_bytes()))
}

/// Where an inline comment lands, as the window says it — one object,
/// because the three commits and the file travel together or not at all.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct MrInlinePlace {
    path: String,
    #[serde(default)]
    old_path: Option<String>,
    line: u64,
    refs: GlabDiffRefs,
}

/// Everybody the project could name as a reviewer. Rows that cannot say who
/// they are drop out here, the same rule the head's reviewers live by.
fn project_members(runner: &impl CliRunner, cwd: &Path) -> Result<Vec<GlabUser>, GlabError> {
    let rows = read_list(runner, cwd, members_path())?;
    Ok(rows.iter().filter_map(read_user).collect())
}

/// How a merge lands. An enum, not the window's word — what it selects is a
/// CLI flag, and `merge` is the flag-less default the CLI owns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MergeMethod {
    Merge,
    Squash,
    Rebase,
}

impl MergeMethod {
    pub(crate) fn from_word(said: &str) -> Option<Self> {
        Some(match said {
            "merge" => Self::Merge,
            "squash" => Self::Squash,
            "rebase" => Self::Rebase,
            _ => return None,
        })
    }

    fn flag(self) -> Option<&'static str> {
        match self {
            Self::Merge => None,
            Self::Squash => Some("--squash"),
            Self::Rebase => Some("--rebase"),
        }
    }
}

/// `glab mr merge <n> --yes [--squash|--rebase]` — measured, minus `-R`
/// (the checkout's cwd answers "which project", the list-read rule).
/// `--yes` because there is no terminal here to answer the CLI's prompt.
fn merge_mr(
    runner: &impl CliRunner,
    cwd: &Path,
    number: u64,
    method: MergeMethod,
) -> Result<(), GlabError> {
    let mut args = vec![
        "mr".to_string(),
        "merge".to_string(),
        number.to_string(),
        "--yes".to_string(),
    ];
    if let Some(flag) = method.flag() {
        args.push(flag.to_string());
    }
    ran(runner, cwd, &args, None)
}

/// One editable field of a merge request. An enum so no window string can
/// become a field name in a PUT — an arbitrary field is an arbitrary write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MrField {
    Title,
    Description,
    AddLabels,
    RemoveLabels,
}

impl MrField {
    fn name(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Description => "description",
            Self::AddLabels => "add_labels",
            Self::RemoveLabels => "remove_labels",
        }
    }
}

/// One field, one PUT, the value on stdin. Orca sends every field as an argv
/// `-f` in one call (updateMR, out/main/index.js) — **recorded deviation**:
/// titles and bodies are somebody's words, and this house does not put words
/// on argv (the comment rule). One call per field is the price, and edits are
/// a rare hand.
fn update_mr_field(
    runner: &impl CliRunner,
    cwd: &Path,
    number: u64,
    field: MrField,
    value: &str,
) -> Result<(), GlabError> {
    let args = [
        "api".to_string(),
        "-X".to_string(),
        "PUT".to_string(),
        item_path(GlabKind::Mr, number),
        "-F".to_string(),
        format!("{}=@-", field.name()),
    ];
    ran(runner, cwd, &args, Some(value.as_bytes()))
}

/// Merge, by the method the person picked.
pub(crate) fn merge_merge_request(
    cwd: &Path,
    number: u64,
    method: MergeMethod,
) -> Result<(), GlabError> {
    merge_mr(&ProcessRunner, cwd, number, method)
}

/// Save one edited field, the value off argv.
pub(crate) fn update_merge_request_field(
    cwd: &Path,
    number: u64,
    field: MrField,
    value: &str,
) -> Result<(), GlabError> {
    update_mr_field(&ProcessRunner, cwd, number, field, value)
}

/// The jobs a merge request's head pipeline ran.
pub(crate) fn fetch_pipeline_jobs(cwd: &Path, pipeline: u64) -> Result<Vec<GlabJob>, GlabError> {
    list_pipeline_jobs(&ProcessRunner, cwd, pipeline)
}

/// Retry one job, by the id its row carries.
pub(crate) fn retry_pipeline_job(cwd: &Path, job: u64) -> Result<(), GlabError> {
    retry_job(&ProcessRunner, cwd, job)
}

/// One job's log, empty when it never wrote one.
/// The reviewers card's and Files tab's one lazy read.
pub(crate) fn fetch_mr_review(cwd: &Path, number: u64) -> GlabMrReview {
    mr_review(&ProcessRunner, cwd, number)
}

/// Replace who reviews, ids alone.
pub(crate) fn update_merge_request_reviewers(
    cwd: &Path,
    number: u64,
    ids: &[u64],
) -> Result<(), GlabError> {
    set_mr_reviewers(&ProcessRunner, cwd, number, ids)
}

/// The bench a reviewer can be picked from.
pub(crate) fn fetch_project_members(cwd: &Path) -> Result<Vec<GlabUser>, GlabError> {
    project_members(&ProcessRunner, cwd)
}

/// Pin one comment to one line of one changed file.
pub(crate) fn post_inline_comment(
    cwd: &Path,
    number: u64,
    place: &MrInlinePlace,
    body: &str,
) -> Result<(), GlabError> {
    inline_comment(&ProcessRunner, cwd, number, place, body)
}

pub(crate) fn fetch_job_trace(cwd: &Path, job: u64) -> Result<String, GlabError> {
    job_trace(&ProcessRunner, cwd, job)
}

/// One item with its conversation, for the dialog a row opens.
pub(crate) fn fetch_item_detail(
    cwd: &Path,
    kind: GlabKind,
    number: u64,
) -> Result<GlabItemDetail, GlabError> {
    item_detail(&ProcessRunner, cwd, kind, number)
}

/// A comment, with the text on stdin and never on argv.
pub(crate) fn comment_on_item(
    cwd: &Path,
    kind: GlabKind,
    number: u64,
    body: &str,
) -> Result<(), GlabError> {
    comment_item(&ProcessRunner, cwd, kind, number, body)
}

/// Close or reopen, through the verb `glab` owns.
pub(crate) fn open_or_close_item(
    cwd: &Path,
    kind: GlabKind,
    number: u64,
    open: bool,
) -> Result<(), GlabError> {
    set_item_open(&ProcessRunner, cwd, kind, number, open)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vendor_cli::{FakeCall, FakeRunner};

    /// A machine whose `glab` has no token, quoted from the one that reported
    /// this ("깃랩도 실제로 연결되는데 지금 안될로 나옴 실제로 프로젝트 풀한거에서
    /// 커밋해줘 하면 알아서 해주는데" — `glab 1.113.0`, verbatim).
    ///
    /// Both halves of that report were true. The API has no credential, and
    /// git operations are configured over ssh — which is a key, not this
    /// token — so `git push` works while every MR read fails. The card said
    /// only the first half, so it read as "GitLab is not connected".
    const NO_TOKEN_STATUS: &str = "gitlab.com\n  \
        x gitlab.com: API call failed: GET https://gitlab.com/api/v4/user: 401 \
        {message: 401 Unauthorized}\n  \
        \u{2713} Git operations for gitlab.com configured to use ssh protocol.\n  \
        \u{2713} API calls for gitlab.com are made over https protocol.\n  \
        ! No token found (checked config file, keyring, and environment \
        variables).\n  \
        X could not authenticate to one or more of the configured GitLab \
        instances.\n";

    /// Not signed in, and the two facts that say WHY and what still works.
    #[test]
    fn a_tokenless_machine_says_so_and_says_git_still_has_a_key() {
        let read = read_auth_status("", NO_TOKEN_STATUS);
        assert!(read.installed, "the CLI answered, so it is installed");
        assert!(
            !read.authenticated,
            "a 401 and 'no token found' read as signed in:\n{NO_TOKEN_STATUS}"
        );
        assert!(
            read.tokenless,
            "the reason went missing, so the card can only say 'not signed in'"
        );
        assert_eq!(
            read.git_protocol.as_deref(),
            Some("ssh"),
            "the card cannot tell somebody their pushes still work"
        );
        assert!(
            read.hosts.is_empty(),
            "a host it is NOT signed in to was named"
        );
    }

    /// And a signed-in machine reports neither of them.
    #[test]
    fn a_signed_in_machine_has_no_reason_to_report() {
        let read = read_auth_status(
            "gitlab.com\n  \u{2713} Logged in to gitlab.com as tester (keyring)\n  \
             \u{2713} Git operations for gitlab.com configured to use https protocol.\n",
            "",
        );
        assert!(read.authenticated, "a signed-in machine read as signed out");
        assert!(
            !read.tokenless,
            "a signed-in machine was said to have no token"
        );
        assert_eq!(read.git_protocol.as_deref(), Some("https"));
        assert_eq!(read.hosts, vec!["gitlab.com".to_string()]);
    }

    fn said(success: bool, stdout: &str, stderr: &str) -> Result<CliOutput, CliError> {
        Ok(CliOutput {
            success,
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
        })
    }

    /// No `glab` is a state with a remedy, not a failure — and nothing is
    /// claimed about authentication on a machine that was never asked.
    #[test]
    fn a_machine_without_the_cli_is_not_installed_rather_than_broken() {
        let runner = FakeRunner::with([Err(CliError::Missing)]);
        let status = glab_status(&runner, None).expect("missing is a state");
        assert_eq!(status, GlabStatus::missing());
        assert!(!status.installed && !status.authenticated && status.hosts.is_empty());
        // The one command, with the one budget. A probe with no ceiling is how
        // a settings pane hangs on an unreachable self-managed instance.
        assert_eq!(
            runner.calls(),
            vec![FakeCall {
                cli: GLAB,
                cwd: None,
                args: vec!["auth".to_string(), "status".to_string()],
                stdin: None,
                budget: Some(AUTH_STATUS_BUDGET),
            }],
        );
    }

    /// Installed and signed out. `glab` says so on stderr and exits non-zero,
    /// and the card must read that as "run this command", not as "no CLI".
    #[test]
    fn an_installed_cli_that_is_signed_out_is_installed() {
        let runner = FakeRunner::with([said(
            false,
            "",
            "gitlab.com\n  x Not logged in to gitlab.com\n",
        )]);
        let status = glab_status(&runner, None).expect("a readable answer");
        assert!(status.installed, "a CLI that ran was reported absent");
        assert!(
            !status.authenticated,
            "`Not logged in` was read as a login because it contains `logged in`"
        );
    }

    /// The ordinary signed-in shape, exit 0, on stdout.
    #[test]
    fn a_signed_in_cli_is_connected() {
        let runner = FakeRunner::with([said(
            true,
            "gitlab.com\n  ✓ Logged in to gitlab.com as hana (~/.config/glab-cli/config.yml)\n\
             \x20 ✓ Token: **************************\n",
            "",
        )]);
        let status = glab_status(&runner, None).expect("a readable answer");
        assert!(status.installed && status.authenticated);
        assert_eq!(status.hosts, vec!["gitlab.com".to_string()]);
        // The masked token line is in the output and not in the answer.
        assert!(
            !format!("{status:?}").contains('*'),
            "the CLI's token line reached the renderer snapshot: {status:?}"
        );
    }

    /// Orca's quirk, kept: the exit code is not the answer. `glab` writes its
    /// status to **stderr** and can exit non-zero while holding a perfectly
    /// good login, and reading the code instead of the text is how a signed-in
    /// machine gets offered a login command.
    #[test]
    fn a_nonzero_exit_that_still_says_logged_in_counts() {
        let runner = FakeRunner::with([said(
            false,
            "",
            "gitlab.com\n  ✓ Logged in to gitlab.com as hana\n",
        )]);
        let status = glab_status(&runner, None).expect("a readable answer");
        assert!(
            status.authenticated && status.hosts == vec!["gitlab.com".to_string()],
            "a signed-in machine was read as signed out because glab exited non-zero"
        );
    }

    /// And where the two readings disagree, the refusal wins. A machine with
    /// gitlab.com signed in and its company instance signed OUT is not a
    /// machine this window may call connected: the card's job is to say what
    /// still needs doing, and `hosts` names which login it does have.
    #[test]
    fn a_signed_out_host_beside_a_signed_in_one_is_not_connected() {
        let runner = FakeRunner::with([said(
            false,
            "",
            "gitlab.com\n  ✓ Logged in to gitlab.com as hana\n\
             gitlab.acme.test\n  x Not logged in to gitlab.acme.test\n",
        )]);
        let status = glab_status(&runner, None).expect("a readable answer");
        assert!(
            status.installed && !status.authenticated,
            "a partially signed-out machine reported as fully connected"
        );
    }

    /// Two hosts, one of them self-managed on a port. The port belongs to the
    /// host: without it the card names a machine nobody is signed in to.
    #[test]
    fn every_signed_in_host_is_named_port_and_all() {
        let runner = FakeRunner::with([said(
            true,
            "gitlab.com\n  ✓ Logged in to gitlab.com as hana\n\
             gitlab.acme.test:8443\n  ✓ Logged in to gitlab.acme.test:8443 as hana\n\
             \x20 - Logged in to gitlab.com as hana\n",
            "",
        )]);
        let status = glab_status(&runner, None).expect("a readable answer");
        assert_eq!(
            status.hosts,
            vec![
                "gitlab.com".to_string(),
                "gitlab.acme.test:8443".to_string()
            ],
            "the host list lost a port, invented one, or repeated a host"
        );
    }

    /// A colon that is punctuation is not a port, and an unreadable line ends
    /// the scan instead of looping on it.
    #[test]
    fn a_trailing_colon_is_punctuation_and_a_nameless_line_terminates() {
        assert_eq!(
            signed_in_hosts("logged in to gitlab.com: as hana"),
            vec!["gitlab.com".to_string()]
        );
        assert_eq!(signed_in_hosts("logged in to !!!"), Vec::<String>::new());
        assert_eq!(signed_in_hosts("logged in to "), Vec::<String>::new());
    }

    /// A probe that never returns is a refusal, and a refusal never carries
    /// what the CLI said — provider output is not a secret transport.
    #[test]
    fn a_probe_that_hangs_is_refused_without_quoting_the_cli() {
        let runner =
            FakeRunner::with([Err(CliError::Refused("GitLab auth probe timed out".into()))]);
        let error = glab_status(&runner, None).expect_err("a hung probe is not a status");
        assert!(matches!(error, GlabError::Refused(_)));
    }

    fn here() -> &'static Path {
        Path::new("/tmp/checkout")
    }

    /// The endpoint one `glab api` run asked for, with the ceiling checked on
    /// the way past — a read without one is how a settings pane hangs.
    fn endpoint(call: &FakeCall) -> String {
        assert_eq!(
            call.budget,
            Some(CALL_BUDGET),
            "a read left without a ceiling"
        );
        assert_eq!(
            call.args.first().map(String::as_str),
            Some("api"),
            "{:?}",
            call.args
        );
        call.args.get(1).cloned().unwrap_or_default()
    }

    fn asked(runner: &FakeRunner) -> String {
        let calls = runner.calls();
        assert_eq!(calls.len(), 1, "one list read is one `glab` run: {calls:?}");
        endpoint(calls.first().expect("a call"))
    }

    /// The issue query is Orca's, and the chip in it moves the ASSIGNEE.
    ///
    /// `state=opened` stands in both questions: the tasks page has no closed
    /// issues to show, so a chip that dropped it would answer a question
    /// nobody asked with a list nobody can act on.
    #[test]
    fn the_issue_query_keeps_its_state_and_moves_only_the_assignee() {
        let runner = FakeRunner::with([said(true, "[]", "")]);
        list_issues(&runner, here(), false).expect("an empty list is an answer");
        let everyone = asked(&runner);
        assert_eq!(
            everyone,
            "projects/:id/issues?per_page=50&order_by=updated_at&sort=desc&state=opened"
        );

        let runner = FakeRunner::with([said(true, "[]", "")]);
        list_issues(&runner, here(), true).expect("an empty list is an answer");
        assert_eq!(
            asked(&runner),
            format!("{everyone}&scope=assigned_to_me"),
            "the assigned chip changed something other than the scope"
        );
    }

    /// Each MR chip is one `state`, and 모두 is the parameter's ABSENCE —
    /// `state=all` is not a value the API has, and sending it lists nothing.
    #[test]
    fn every_mr_chip_is_one_state_and_all_sends_none() {
        for (chip, wanted) in [
            (MrState::Opened, Some("&state=opened")),
            (MrState::Merged, Some("&state=merged")),
            (MrState::Closed, Some("&state=closed")),
            (MrState::All, None),
        ] {
            let runner = FakeRunner::with([said(true, "[]", "")]);
            list_mrs(&runner, here(), chip, PAGE_ROWS, None).expect("an empty list is an answer");
            let path = asked(&runner);
            let base = "projects/:id/merge_requests?page=1&per_page=50\
                        &order_by=updated_at&sort=desc&with_merge_status_recheck=false";
            assert_eq!(
                path,
                format!("{base}{}", wanted.unwrap_or_default()),
                "{chip:?}"
            );
        }
        // And the words the window may send are exactly those four.
        assert_eq!(MrState::from_word("merged"), Some(MrState::Merged));
        assert_eq!(MrState::from_word("draft"), None);
    }

    /// Both lists read the **iid** and the page, and a row missing either one
    /// is a row whose gestures are dead.
    #[test]
    fn a_row_carries_the_number_a_person_reads_and_the_page_it_opens() {
        let runner = FakeRunner::with([said(
            true,
            r#"[
                {"id": 90210, "iid": 42, "title": "로그인이 두 번 튕김",
                 "state": "opened", "updated_at": "2026-08-16T01:02:03Z",
                 "web_url": "https://gitlab.com/acme/app/-/merge_requests/42"},
                {"id": 90211, "iid": 43, "title": "번호는 있는데 주소가 없다",
                 "state": "merged", "updated_at": "2026-08-15T00:00:00Z"},
                {"id": 90212, "title": "주소는 있는데 번호가 없다",
                 "web_url": "https://gitlab.com/acme/app/-/merge_requests/44"}
            ]"#,
            "",
        )]);
        let rows =
            list_mrs(&runner, here(), MrState::Opened, PAGE_ROWS, None).expect("a readable answer");
        assert_eq!(
            rows,
            vec![GlabItem {
                kind: "mr".to_string(),
                number: 42,
                title: "로그인이 두 번 튕김".to_string(),
                state: "opened".to_string(),
                updated_at: "2026-08-16T01:02:03Z".to_string(),
                url: "https://gitlab.com/acme/app/-/merge_requests/42".to_string(),
                author: None,
                source_branch: None,
                target_branch: None,
                cross_repo: false,
            }],
            "a row was kept without the number it is called by or the page it opens"
        );
    }

    /// 고른 MR이 어디서 잘라야 하는지를 말하는 세 사실, 그리고 **포크 판정의
    /// 기본값**.
    ///
    /// 원본의 규칙은 두 조건이 다 필요하다(`main/gitlab/mappers.ts:250-253`):
    /// 두 프로젝트 id가 **둘 다 있고** 서로 달라야 포크다. 한쪽만 있는 페이로드는
    /// 「포크가 아니라고 말한 것」이 아니라 **아무 말도 안 한 것**이고, 그것을
    /// 포크로 읽으면 모든 MR이 head를 풀 수 없는 도로로 간다.
    #[test]
    fn a_merge_request_says_where_to_cut_from_and_a_missing_pair_is_not_a_fork() {
        let runner = FakeRunner::with([said(
            true,
            r#"[
                {"iid": 7, "title": "포크에서 온 것", "state": "opened",
                 "web_url": "https://gitlab.com/acme/app/-/merge_requests/7",
                 "source_branch": "fix/login", "target_branch": "main",
                 "author": {"username": "somebody"},
                 "source_project_id": 11, "target_project_id": 22},
                {"iid": 8, "title": "같은 저장소", "state": "opened",
                 "web_url": "https://gitlab.com/acme/app/-/merge_requests/8",
                 "source_branch": "chore/deps", "target_branch": "main",
                 "source_project_id": 22, "target_project_id": 22},
                {"iid": 9, "title": "한쪽만 말한 페이로드", "state": "opened",
                 "web_url": "https://gitlab.com/acme/app/-/merge_requests/9",
                 "source_branch": "spike", "target_branch": "main",
                 "source_project_id": 22}
            ]"#,
            "",
        )]);
        let rows = list_mrs(&runner, here(), MrState::Opened, PICKER_ROWS, None)
            .expect("a readable answer");
        assert_eq!(
            rows.iter().map(|row| row.cross_repo).collect::<Vec<_>>(),
            vec![true, false, false],
            "the fork judgement stopped needing both project ids"
        );
        assert_eq!(rows[0].source_branch.as_deref(), Some("fix/login"));
        assert_eq!(rows[0].target_branch.as_deref(), Some("main"));
        assert_eq!(rows[0].author.as_deref(), Some("somebody"));
        // 이슈에는 둘 다 없다 — 있는 척하지 않는다.
        assert_eq!(rows[2].author, None);
    }

    /// 타이핑한 글자는 **인코딩되어** 실린다, 그리고 빈 글자는 아예 안 실린다.
    ///
    /// 원본: `query?.trim() ? `&search=${encodeURIComponent(query.trim())}` : ''`
    /// (`main/gitlab/client.ts:496`). 이 파일에서 경로 문자열에 들어가는 첫
    /// **사람이 준 텍스트**이고, 날것으로 실으면 `&` 하나가 다른 파라미터를 쓴다.
    #[test]
    fn what_somebody_types_rides_encoded_and_an_empty_query_does_not_ride() {
        let asked_with = |query: Option<&str>, rows: usize| {
            let runner = FakeRunner::with([said(true, "[]", "")]);
            list_mrs(&runner, here(), MrState::Opened, rows, query).expect("an answer");
            asked(&runner)
        };
        let plain = asked_with(None, PAGE_ROWS);
        assert!(
            !plain.contains("&search="),
            "an absent query still rode: {plain}"
        );
        assert!(plain.contains("per_page=50"));

        // 다듬어서 비면 없는 것과 같다.
        assert_eq!(asked_with(Some("   "), PAGE_ROWS), plain);

        // 그리고 고르는 자리는 열두 줄을 읽는다(원본 `RESULT_LIMIT`).
        let typed = asked_with(Some("  a&state=closed  "), PICKER_ROWS);
        assert!(typed.contains("per_page=12"), "{typed}");
        assert!(
            typed.ends_with("&search=a%26state%3Dclosed"),
            "the typed text rode raw, so an ampersand could write a second \
             parameter: {typed}"
        );
        assert_eq!(
            typed.matches("&state=").count(),
            1,
            "a typed `&state=` became a second state parameter: {typed}"
        );
    }

    /// A todo names an action, a target, and the project it came from — and
    /// the whole row is its link, so one without a link is not a row.
    #[test]
    fn a_todo_carries_its_action_target_and_project() {
        let runner = FakeRunner::with([said(
            true,
            r#"[
                {"action_name": "marked_for_review", "target_type": "MergeRequest",
                 "target": {"iid": 7, "title": "리다이렉트 수리"},
                 "project": {"path_with_namespace": "acme/app"},
                 "target_url": "https://gitlab.com/acme/app/-/merge_requests/7",
                 "updated_at": "2026-08-16T02:00:00Z"},
                {"action_name": "assigned", "target_type": "Issue",
                 "target": {"title": "번호 없는 대상"},
                 "project": {"path_with_namespace": "acme/app"}}
            ]"#,
            "",
        )]);
        let rows = list_todos(&runner, here()).expect("a readable answer");
        assert_eq!(asked(&runner), "todos?state=pending&per_page=50");
        assert_eq!(
            rows,
            vec![GlabTodo {
                action: "marked_for_review".to_string(),
                target_type: "MergeRequest".to_string(),
                target_number: Some(7),
                target_title: "리다이렉트 수리".to_string(),
                project_path: "acme/app".to_string(),
                target_url: "https://gitlab.com/acme/app/-/merge_requests/7".to_string(),
                updated_at: "2026-08-16T02:00:00Z".to_string(),
            }],
            "a todo with no page to open was kept as a row"
        );
    }

    /// A refusal becomes a WORD. The window owns the sentence, and what the
    /// CLI said stays inside this file — `glab` prints the URL it called and
    /// the host it called it on, and one of its builds prints the token.
    #[test]
    fn a_refused_read_is_sorted_into_a_word_and_never_quoted() {
        for (stdout, stderr, wanted) in [
            (
                r#"{"message":"404 Project Not Found"}"#,
                "",
                GlabDenial::NoProject,
            ),
            (
                "",
                "HTTP 403: Forbidden (token gl-pat-secret)",
                GlabDenial::Forbidden,
            ),
            ("", "HTTP 429: Too Many Requests", GlabDenial::RateLimited),
            (
                "",
                "Get \"https://gitlab.acme.test/api/v4\": dial tcp: i/o timeout",
                GlabDenial::Network,
            ),
            ("", "something nobody has seen yet", GlabDenial::Failed),
        ] {
            let runner = FakeRunner::with([said(false, stdout, stderr)]);
            let error = list_issues(&runner, here(), false).expect_err("a refusal is not a list");
            assert_eq!(
                error,
                GlabError::Denied(wanted),
                "`{stderr}{stdout}` was sorted wrong"
            );
            let carried = format!("{error:?}");
            assert!(
                !carried.contains("gl-pat-secret") && !carried.contains("gitlab.acme.test"),
                "the CLI's own text rode out inside the refusal: {carried}"
            );
        }
    }

    /// A pair of clean answers for the two reads one dialog makes.
    fn opened_item(kind: &str, number: u64) -> Result<CliOutput, CliError> {
        said(
            true,
            &format!(
                r#"{{"iid": {number}, "title": "리다이렉트 수리", "state": "opened",
                    "created_at": "2026-08-01T09:00:00Z",
                    "description": "재현: 로그인 → 뒤로 → 다시 로그인",
                    "web_url": "https://gitlab.com/acme/app/-/{kind}/{number}",
                    "author": {{"username": "hana", "name": "하나"}},
                    "labels": ["bug", "backend"]}}"#
            ),
            "",
        )
    }

    /// The dialog's two reads are the two the API keeps apart, and each kind
    /// asks its own collection: `issues` and `merge_requests` are not
    /// interchangeable, and a dialog that asked the wrong one would show the
    /// merge request that happens to share an issue's number.
    #[test]
    fn a_dialog_reads_the_head_and_the_conversation_of_its_own_kind() {
        for (kind, collection, number) in [
            (GlabKind::Issue, "issues", 7),
            (GlabKind::Mr, "merge_requests", 42),
        ] {
            let runner = FakeRunner::with([opened_item(collection, number), said(true, "[]", "")]);
            let detail = item_detail(&runner, here(), kind, number).expect("a readable answer");
            let calls = runner.calls();
            assert_eq!(calls.len(), 2, "a dialog is two reads: {calls:?}");
            assert_eq!(
                endpoint(&calls[0]),
                format!("projects/:id/{collection}/{number}")
            );
            assert_eq!(
                endpoint(&calls[1]),
                format!("projects/:id/{collection}/{number}/discussions?per_page=50")
            );
            assert_eq!(detail.kind, kind.word());
            assert_eq!(detail.number, number);
            assert_eq!(detail.author, "hana", "the handle beside the title is gone");
            assert_eq!(detail.body, "재현: 로그인 → 뒤로 → 다시 로그인");
            assert_eq!(
                detail.labels,
                vec!["bug".to_string(), "backend".to_string()]
            );
        }
        // And the word the window may send is exactly one of those two.
        assert_eq!(GlabKind::from_word("mr"), Some(GlabKind::Mr));
        assert_eq!(GlabKind::from_word("pr"), None);
    }

    /// GitLab narrates itself in the same list it keeps comments in, and the
    /// narration is not a voice: a dialog that drew "changed the description"
    /// as a comment buries the two somebody wrote under twenty nobody did.
    /// Resolution comes off the THREAD when it says so, and off the note when
    /// only the note does.
    #[test]
    fn system_notes_are_not_voices_and_a_settled_thread_says_so() {
        let runner = FakeRunner::with([
            opened_item("merge_requests", 42),
            said(
                true,
                r#"[
                    {"id": "d1", "resolved": true, "notes": [
                        {"body": "여기 왜 두 번 부르나요?", "system": false,
                         "created_at": "2026-08-02T10:00:00Z",
                         "author": {"username": "lee"}},
                        {"body": "고쳤습니다", "system": false,
                         "created_at": "2026-08-02T11:00:00Z",
                         "author": {"username": "hana"}}
                    ]},
                    {"id": "d2", "notes": [
                        {"body": "changed the description", "system": true,
                         "author": {"username": "hana"}},
                        {"body": "리뷰 부탁드립니다", "system": false, "resolved": false,
                         "created_at": "2026-08-03T09:00:00Z",
                         "author": {"username": "hana"}}
                    ]},
                    {"id": "d3"}
                ]"#,
                "",
            ),
        ]);
        let detail = item_detail(&runner, here(), GlabKind::Mr, 42).expect("a readable answer");
        assert_eq!(
            detail
                .comments
                .iter()
                .map(|one| (one.author.as_str(), one.resolved))
                .collect::<Vec<_>>(),
            vec![
                ("lee", Some(true)),
                ("hana", Some(true)),
                ("hana", Some(false)),
            ],
            "a system note was kept as a comment, or a thread's resolution was \
             lost on the way to its notes: {:?}",
            detail.comments
        );
    }

    #[test]
    fn a_merge_lands_by_the_picked_method_and_answers_no_prompt() {
        let runner = FakeRunner::with([said(true, "", ""), said(true, "", "")]);
        merge_mr(&runner, here(), 42, MergeMethod::Merge).expect("merged");
        merge_mr(&runner, here(), 42, MergeMethod::Squash).expect("squashed");
        let calls = runner.calls();
        assert_eq!(calls[0].args, vec!["mr", "merge", "42", "--yes"]);
        assert_eq!(
            calls[1].args,
            vec!["mr", "merge", "42", "--yes", "--squash"]
        );
    }

    #[test]
    fn an_edited_field_travels_on_stdin_one_put_apiece() {
        let runner = FakeRunner::with([said(true, "{}", "")]);
        update_mr_field(&runner, here(), 42, MrField::Title, "고친 제목 --yes").expect("saved");
        let call = &runner.calls()[0];
        assert_eq!(
            call.args,
            vec![
                "api",
                "-X",
                "PUT",
                "projects/:id/merge_requests/42",
                "-F",
                "title=@-"
            ]
        );
        // 제목이 argv 어디에도 없다 — 말은 파이프로 간다(댓글의 그 규칙).
        assert!(call.args.iter().all(|arg| !arg.contains("고친")));
        assert_eq!(call.stdin.as_deref(), Some("고친 제목 --yes".as_bytes()));
    }

    #[test]
    fn a_merge_request_names_its_head_pipeline_and_an_issue_never_does() {
        // 신판이 head_pipeline을, 옛 인스턴스가 pipeline만 답하는 실측의 그
        // 우선순위 — 그리고 null인 head_pipeline은 이름이 아니라 부재다.
        let runner = FakeRunner::with([
            said(
                true,
                r#"{"iid": 42, "title": "t", "state": "opened",
                    "head_pipeline": null, "pipeline": {"id": 900}}"#,
                "",
            ),
            said(true, "[]", ""),
            said(
                true,
                r#"{"iid": 43, "title": "t", "state": "opened",
                    "head_pipeline": {"id": 901}, "pipeline": {"id": 900}}"#,
                "",
            ),
            said(true, "[]", ""),
        ]);
        let fallback = item_detail(&runner, here(), GlabKind::Mr, 42).expect("readable");
        assert_eq!(fallback.pipeline_id, Some(900));
        let named = item_detail(&runner, here(), GlabKind::Mr, 43).expect("readable");
        assert_eq!(named.pipeline_id, Some(901));
    }

    #[test]
    fn a_pipeline_lists_its_jobs_the_lot_at_once() {
        let runner = FakeRunner::with([said(
            true,
            r#"[
                {"id": 7, "name": "build", "stage": "build", "status": "success",
                 "web_url": "https://gitlab.com/acme/app/-/jobs/7", "duration": 41.5},
                {"id": 8, "name": "test", "stage": "test", "status": "running",
                 "web_url": "https://gitlab.com/acme/app/-/jobs/8", "duration": null},
                {"nameless": true}
            ]"#,
            "",
        )]);
        let jobs = list_pipeline_jobs(&runner, here(), 900).expect("readable");
        assert_eq!(
            runner.calls()[0].args,
            vec![
                "api".to_string(),
                "projects/:id/pipelines/900/jobs?per_page=100".to_string()
            ],
            "the jobs read left the measured page or the one door"
        );
        assert_eq!(jobs.len(), 2, "a row without an id is not a job");
        assert_eq!(jobs[0].duration, Some(41.5));
        assert_eq!(jobs[1].duration, None);
        assert_eq!(jobs[1].status, "running");
    }

    #[test]
    fn a_retry_is_a_post_at_the_job_alone() {
        let runner = FakeRunner::with([said(true, "{}", "")]);
        retry_job(&runner, here(), 8).expect("a retry that ran");
        assert_eq!(
            runner.calls()[0].args,
            vec![
                "api".to_string(),
                "-X".to_string(),
                "POST".to_string(),
                "projects/:id/jobs/8/retry".to_string()
            ]
        );
    }

    #[test]
    fn a_quiet_job_answers_an_empty_log_not_a_refusal() {
        let runner = FakeRunner::with([
            said(true, "line one\nline two", ""),
            said(false, r#"{"message":"404 Not Found"}"#, ""),
            said(false, "", "403 Forbidden"),
        ]);
        assert_eq!(
            job_trace(&runner, here(), 7).expect("a written log"),
            "line one\nline two"
        );
        assert_eq!(
            job_trace(&runner, here(), 8).expect("a quiet log"),
            "",
            "a job that never wrote a line was sorted into a refusal"
        );
        assert!(
            job_trace(&runner, here(), 9).is_err(),
            "a real refusal was swallowed as an empty log"
        );
    }

    /// The triple travels whole or not at all — two of three commits is a
    /// position GitLab would refuse in a worse voice later.
    #[test]
    fn a_merge_request_carries_its_diff_refs_whole_or_not_at_all() {
        let runner = FakeRunner::with([
            said(
                true,
                r#"{"iid": 42, "title": "t", "state": "opened",
                    "diff_refs": {"base_sha": "b1", "start_sha": "s1", "head_sha": "h1"}}"#,
                "",
            ),
            said(true, "[]", ""),
            said(
                true,
                r#"{"iid": 43, "title": "t", "state": "opened",
                    "diff_refs": {"base_sha": "b1", "head_sha": "h1"}}"#,
                "",
            ),
            said(true, "[]", ""),
        ]);
        let whole = item_detail(&runner, here(), GlabKind::Mr, 42).expect("readable");
        assert_eq!(
            whole.diff_refs,
            Some(GlabDiffRefs {
                base_sha: "b1".to_string(),
                start_sha: "s1".to_string(),
                head_sha: "h1".to_string()
            })
        );
        let torn = item_detail(&runner, here(), GlabKind::Mr, 43).expect("readable");
        assert_eq!(torn.diff_refs, None, "a torn triple survived as a position");
    }

    /// The position is the diff's own data and rides argv; the words ride
    /// stdin. And a file that never moved names its old path as itself —
    /// the measured fallback.
    #[test]
    fn an_inline_comment_pins_its_place_and_speaks_on_stdin() {
        let refs = GlabDiffRefs {
            base_sha: "b1".to_string(),
            start_sha: "s1".to_string(),
            head_sha: "h1".to_string(),
        };
        let runner = FakeRunner::with([said(true, "{}", ""), said(true, "{}", "")]);
        let moved = MrInlinePlace {
            path: "docs/b.md".to_string(),
            old_path: Some("docs/a.md".to_string()),
            line: 12,
            refs: refs.clone(),
        };
        inline_comment(&runner, here(), 42, &moved, "여기 왜요? --yes").expect("pinned");
        let unmoved = MrInlinePlace {
            path: "src/app.rs".to_string(),
            old_path: None,
            line: 3,
            refs,
        };
        inline_comment(&runner, here(), 42, &unmoved, "한 줄").expect("pinned");
        let calls = runner.calls();
        assert_eq!(
            calls[0].args,
            vec![
                "api",
                "-X",
                "POST",
                "projects/:id/merge_requests/42/discussions",
                "-F",
                "body=@-",
                "-f",
                "position[position_type]=text",
                "-f",
                "position[base_sha]=b1",
                "-f",
                "position[start_sha]=s1",
                "-f",
                "position[head_sha]=h1",
                "-f",
                "position[old_path]=docs/a.md",
                "-f",
                "position[new_path]=docs/b.md",
                "-f",
                "position[new_line]=12"
            ]
        );
        // 말은 argv 어디에도 없다 — 파이프로 간다(댓글의 그 규칙).
        assert!(calls[0].args.iter().all(|arg| !arg.contains("왜요")));
        assert_eq!(
            calls[0].stdin.as_deref(),
            Some("여기 왜요? --yes".as_bytes())
        );
        assert!(
            calls[1]
                .args
                .contains(&"position[old_path]=src/app.rs".to_string()),
            "an unmoved file should name itself as its old path: {:?}",
            calls[1].args
        );
    }

    /// The head already says who reviews, and the card reads it there —
    /// no second endpoint for a fact the first answer carried. A row that
    /// cannot say who it is (no id, or no username) never becomes a chip.
    #[test]
    fn a_dialogs_head_names_its_reviewers() {
        let runner = FakeRunner::with([
            said(
                true,
                r#"{"iid": 42, "title": "t", "state": "opened",
                    "reviewers": [
                        {"id": 7, "username": "hana"},
                        {"username": "ghost"},
                        {"id": 9}
                    ]}"#,
                "",
            ),
            said(true, "[]", ""),
        ]);
        let detail = item_detail(&runner, here(), GlabKind::Mr, 42).expect("readable");
        assert_eq!(
            detail.reviewers,
            vec![GlabUser {
                id: 7,
                username: "hana".to_string()
            }],
            "a nameless or id-less row survived into the chips"
        );
    }

    /// The counts are counted here, headers excluded, and a rename keeps
    /// where it came from — while a file with no path at all is not a row.
    #[test]
    fn a_changed_file_counts_its_own_lines_and_keeps_a_rename() {
        let runner = FakeRunner::with([said(
            true,
            r#"[
                {"new_path": "src/app.rs", "old_path": "src/app.rs",
                 "diff": "--- a/src/app.rs\n+++ b/src/app.rs\n+one\n+two\n-gone\n context"},
                {"new_path": "docs/b.md", "old_path": "docs/a.md", "diff": "+moved\n"},
                {"new_path": "logo.png", "old_path": "logo.png", "diff": ""},
                {"diff": "+orphan"}
            ]"#,
            "",
        )]);
        let files = read_mr_files(&runner, here(), 42);
        assert_eq!(
            endpoint(&runner.calls()[0]),
            "projects/:id/merge_requests/42/diffs?per_page=100"
        );
        assert_eq!(files.len(), 3, "a pathless row became a file: {files:?}");
        assert_eq!(files[0].additions, 2, "a +++ header was counted as a line");
        assert_eq!(files[0].deletions, 1, "a --- header was counted as a line");
        assert_eq!(files[0].old_path, None, "an unmoved file claims a rename");
        assert_eq!(files[1].old_path.as_deref(), Some("docs/a.md"));
        assert_eq!(files[2].diff, "", "a held-back diff should stay empty");
    }

    /// Approvals are a paid-tier fact on some instances: either endpoint may
    /// refuse alone and the other's facts still stand. Only a double refusal
    /// is silence, and a refused diffs read is an empty tab, not a dead card.
    #[test]
    fn approval_facts_survive_one_refusal_and_a_double_refusal_is_none() {
        let runner = FakeRunner::with([
            said(
                true,
                r#"{"approvals_left": 1, "approvals_required": 2}"#,
                "",
            ),
            said(false, "", "HTTP 403: Forbidden"),
            said(
                true,
                r#"[{"new_path": "a.rs", "old_path": "a.rs", "diff": "+x"}]"#,
                "",
            ),
        ]);
        let review = mr_review(&runner, here(), 42);
        let approvals = review.approvals.expect("one answer is an answer");
        assert_eq!(approvals.left, Some(1));
        assert_eq!(approvals.required, Some(2));
        assert_eq!(approvals.rules, vec![]);
        assert_eq!(review.files.len(), 1);

        let runner = FakeRunner::with([
            said(false, "", "HTTP 403: Forbidden"),
            said(
                true,
                r#"{"rules": [
                    {"name": "QA", "approvals_required": 1, "approved": false},
                    {"name": "Security", "approvals_required": 2, "approved": true}
                ]}"#,
                "",
            ),
            said(false, r#"{"message":"404 Not Found"}"#, ""),
        ]);
        let review = mr_review(&runner, here(), 42);
        let approvals = review.approvals.expect("the rules alone still stand");
        assert_eq!(approvals.left, None);
        assert_eq!(
            approvals.rules,
            vec![
                GlabApprovalRule {
                    name: "QA".to_string(),
                    required: 1,
                    approved: false
                },
                GlabApprovalRule {
                    name: "Security".to_string(),
                    required: 2,
                    approved: true
                }
            ]
        );
        assert_eq!(review.files, vec![], "a refused diffs read killed the card");

        let runner = FakeRunner::with([
            said(false, "", "HTTP 403: Forbidden"),
            said(false, "", "HTTP 403: Forbidden"),
            said(true, "[]", ""),
        ]);
        assert_eq!(
            mr_review(&runner, here(), 42).approvals,
            None,
            "a double refusal invented an approvals card"
        );
    }

    /// The set travels whole: one `reviewer_ids[]` per id, and emptying the
    /// set is the bare field — the measured spelling for "nobody".
    #[test]
    fn a_reviewer_set_travels_as_ids_and_empties_to_a_bare_field() {
        let runner = FakeRunner::with([said(true, "{}", ""), said(true, "{}", "")]);
        set_mr_reviewers(&runner, here(), 42, &[7, 9]).expect("set");
        set_mr_reviewers(&runner, here(), 42, &[]).expect("cleared");
        let calls = runner.calls();
        assert_eq!(
            calls[0].args,
            vec![
                "api",
                "-X",
                "PUT",
                "projects/:id/merge_requests/42",
                "-f",
                "reviewer_ids[]=7",
                "-f",
                "reviewer_ids[]=9"
            ]
        );
        assert_eq!(
            calls[1].args,
            vec![
                "api",
                "-X",
                "PUT",
                "projects/:id/merge_requests/42",
                "-f",
                "reviewer_ids="
            ]
        );
    }

    /// The bench is one wide page, and rows that cannot say who they are
    /// drop out — the chips' own rule.
    #[test]
    fn the_bench_drops_a_row_that_cannot_say_who_it_is() {
        let runner = FakeRunner::with([said(
            true,
            r#"[
                {"id": 7, "username": "hana", "state": "active"},
                {"id": 8},
                {"username": "ghost"}
            ]"#,
            "",
        )]);
        let bench = project_members(&runner, here()).expect("readable");
        assert_eq!(
            endpoint(&runner.calls()[0]),
            "projects/:id/members/all?per_page=100"
        );
        assert_eq!(
            bench,
            vec![GlabUser {
                id: 7,
                username: "hana".to_string()
            }]
        );
    }

    /// The comment travels on STDIN, and the argv carries a literal `@-`.
    ///
    /// This is the whole security claim of the write half: `ps` on a shared
    /// machine reads every argument of every process, and what somebody typed
    /// into a dialog is not for that list.
    #[test]
    fn a_comment_travels_on_stdin_never_argv() {
        let secret = "토큰이 새는 줄: glpat-not-in-argv";
        let runner = FakeRunner::with([said(true, "{}", "")]);
        comment_item(&runner, here(), GlabKind::Issue, 7, secret).expect("a posted comment");
        let calls = runner.calls();
        assert_eq!(calls.len(), 1, "one comment is one `glab` run: {calls:?}");
        assert_eq!(
            calls[0].args,
            vec![
                "api".to_string(),
                "-X".to_string(),
                "POST".to_string(),
                "projects/:id/issues/7/notes".to_string(),
                "-F".to_string(),
                "body=@-".to_string(),
            ]
        );
        assert_eq!(calls[0].stdin.as_deref(), Some(secret.as_bytes()));
        assert!(
            !calls[0].args.iter().any(|arg| arg.contains(secret)),
            "what somebody wrote rode out in the process list: {:?}",
            calls[0].args
        );
        // A merge request posts to its own collection, or the note lands on
        // the issue that shares its number.
        let runner = FakeRunner::with([said(true, "{}", "")]);
        comment_item(&runner, here(), GlabKind::Mr, 42, "확인했습니다").expect("a posted comment");
        assert_eq!(
            runner.calls()[0].args.get(3).map(String::as_str),
            Some("projects/:id/merge_requests/42/notes")
        );
    }

    /// Close and reopen are `glab`'s own verbs, on the noun of the kind, in
    /// the checkout. Nothing about a project is named twice.
    #[test]
    fn closing_and_reopening_map_to_the_verb_the_cli_owns() {
        for (kind, open, wanted) in [
            (GlabKind::Issue, false, ["issue", "close", "7"]),
            (GlabKind::Issue, true, ["issue", "reopen", "7"]),
            (GlabKind::Mr, false, ["mr", "close", "7"]),
            (GlabKind::Mr, true, ["mr", "reopen", "7"]),
        ] {
            let runner = FakeRunner::with([said(true, "", "")]);
            set_item_open(&runner, here(), kind, 7, open).expect("a state change");
            let calls = runner.calls();
            assert_eq!(calls[0].args, wanted, "{kind:?} open={open}");
            assert_eq!(calls[0].stdin, None, "a close was fed something to read");
            assert!(
                !calls[0].args.iter().any(|arg| arg == "-R"),
                "the project was named a second way beside the checkout: {:?}",
                calls[0].args
            );
        }
    }

    /// A dialog's refusals sort like every other read's, and an item that is
    /// not the one asked for — including the error object GitLab answers with
    /// a zero exit — is a refusal rather than an empty dialog.
    #[test]
    fn a_refused_dialog_is_a_word_and_a_wrong_item_is_not_an_answer() {
        let runner = FakeRunner::with([said(false, r#"{"message":"404 Project Not Found"}"#, "")]);
        assert_eq!(
            item_detail(&runner, here(), GlabKind::Issue, 7),
            Err(GlabError::Denied(GlabDenial::NoProject))
        );

        let runner = FakeRunner::with([said(true, r#"{"message":"404 Not found"}"#, "")]);
        assert_eq!(
            item_detail(&runner, here(), GlabKind::Mr, 42),
            Err(GlabError::Denied(GlabDenial::NoProject)),
            "an error object answered with exit 0 was painted as an item"
        );

        let runner = FakeRunner::with([said(false, "", "HTTP 403: Forbidden (gl-pat-secret)")]);
        let error = comment_item(&runner, here(), GlabKind::Issue, 7, "안녕")
            .expect_err("a refusal is not a posted comment");
        assert_eq!(error, GlabError::Denied(GlabDenial::Forbidden));
        assert!(
            !format!("{error:?}").contains("gl-pat-secret"),
            "the CLI's own text rode out inside the refusal: {error:?}"
        );
    }

    /// And an answer that is not JSON at all is the generic word, with the
    /// junk left where it was found.
    #[test]
    fn an_unreadable_answer_fails_without_carrying_the_junk() {
        let runner = FakeRunner::with([said(true, "not json, and a token gl-pat-secret", "")]);
        let error = list_todos(&runner, here()).expect_err("junk is not a list");
        assert_eq!(error, GlabError::Denied(GlabDenial::Failed));
        assert!(
            !format!("{error:?}").contains("gl-pat-secret"),
            "the unreadable answer rode out inside the refusal"
        );
    }
}
