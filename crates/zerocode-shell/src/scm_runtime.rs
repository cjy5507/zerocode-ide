use super::*;

/// One `--numstat -z` answer, keyed by the path the panel is showing — a
/// rename's NEW name. git writes `added\tremoved\tpath` for an ordinary
/// file; a rename carries an EMPTY third field and rides its two real paths
/// in the next two NUL fields (old, then new). Binary files count as `-` and
/// answer no tally at all — but a binary rename still owns its two path
/// fields, so they are consumed either way or every entry after it shifts.
pub(super) fn line_counts_by_path(numstat: &str) -> std::collections::HashMap<String, (u64, u64)> {
    let mut held = std::collections::HashMap::new();
    let mut fields = numstat.split('\0');
    while let Some(head) = fields.next() {
        if head.is_empty() {
            continue;
        }
        let mut parts = head.splitn(3, '\t');
        let (Some(added), Some(removed), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let shown = if path.is_empty() {
            let _origin = fields.next();
            match fields.next() {
                Some(new_name) => new_name.to_string(),
                None => break,
            }
        } else {
            path.to_string()
        };
        if let (Ok(added), Ok(removed)) = (added.parse::<u64>(), removed.parse::<u64>()) {
            held.insert(shown, (added, removed));
        }
    }
    held
}

/// A commit git refused, as the card needs to remember it.
pub(super) struct CommitFailure {
    /// Both pipes of the refused `git commit`, as git and its hooks wrote it.
    pub(super) text: String,
    /// The message the person was trying to commit, so the agent can put the
    /// work back where they left it.
    pub(super) message: String,
    pub(super) entries: Vec<zerocode_core::commit_failure::StagedEntry>,
}

/// What the amber conflicts card says.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ConflictCard {
    /// `merge` / `rebase` / `cherry-pick` / `unknown` — carried so the abort
    /// can name the operation the card was drawn for.
    pub(super) operation: &'static str,
    /// How many rows are still unresolved. **Zero is a real answer**: an
    /// operation can stand with every file resolved and nothing continued,
    /// and that is the card's other face (Orca's `OperationBanner`).
    pub(super) count: usize,
    /// Only merge and rebase have the wholesale undo git offers in this panel.
    pub(super) can_abort: bool,
    pub(super) prompt: String,
}

/// What the window paints under the commit box after a refusal.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CommitFailureCard {
    pub(super) summary: String,
    /// `Lint` or `Hook`, the pill beside the title — absent when the output
    /// names neither, because an unlabelled failure is better than a wrong
    /// label.
    pub(super) kind_label: Option<String>,
    /// The whole output, for the dialog.
    pub(super) detail_text: String,
    /// Whether the dialog would show anything the card does not already say.
    pub(super) has_details: bool,
    pub(super) prompt: String,
}

/// Split one porcelain code into the two questions the panel asks of it.
///
/// The index column is the first character and the working-tree column the
/// second. `?` and `!` are not letters about a column — untracked and ignored
/// use both slots to say one thing — so neither ever counts as staged.
///
/// **A conflict is neither.** `UU`, `AA` and `DD` have a letter in both
/// columns, and read column by column they landed in the staged list AND the
/// changed list — the same file twice, offering to unstage a thing that is not
/// staged. git reports an unmerged path as one state, and it is one row.
pub(super) fn scm_flags(code: &str) -> (bool, bool) {
    if ConflictKind::from_code(code).is_some() {
        return (false, true);
    }
    let mut columns = code.chars();
    let index = columns.next().unwrap_or(' ');
    let worktree = columns.next().unwrap_or(' ');
    (
        !matches!(index, ' ' | '?' | '!'),
        !matches!(worktree, ' ' | '!'),
    )
}

/// Which coding agents this machine can actually run.
///
/// The registry in `zerocode-core` says how to drive thirty-five of them; this
/// crosses that with `PATH`. Both halves are needed before a picker can be
/// honest — a list of thirty-five names is not a list of choices, and offering
/// one that is not installed makes the first thing somebody tries fail.
///
/// Every agent is answered for, installed or not, and in the registry's order.
/// A picker that lists only what is present cannot say "you could have this
/// one"; a picker that reorders itself as tools come and go is one you cannot
/// aim at.
/// Detection against the PATH the user's terminal would have.
///
/// The shell's PATH when it has answered, the process's own when it has not —
/// degraded to the old behaviour, never worse than it. `force` re-asks the
/// shell; anything else takes the cached answer, so only the first call and the
/// refresh button ever pay the shell's startup.
pub(super) fn detected_agents(force: bool) -> Vec<zerocode_core::AgentPresence> {
    if force {
        // The person just installed something: every binary verdict the
        // readiness snapshot holds was read off the old PATH.
        readiness_runtime::installs_changed();
    }
    let path = shell_path::hydrate(force)
        .map(std::ffi::OsString::from)
        .or_else(|| std::env::var_os("PATH"));
    agent_presence(path.as_deref(), std::env::consts::OS)
}

/// One re-enterable Claude conversation from the on-disk session store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct ClaudeSessionRow {
    /// The conversation as the resume road takes it: Claude's key, the store
    /// file's stem as its id (what `claude --resume` takes), and the file
    /// itself. A row that handed the window a bare id left the key to be
    /// spelled over there, beside a judge that compares records by it.
    pub(super) session: zerocode_core::ProviderSession,
    /// The first real user message, whitespace-folded and cut at 80 chars.
    pub(super) title: String,
    /// File mtime in epoch milliseconds — the row's relative-time badge.
    pub(super) at_ms: u64,
}

/// A Claude session id is the store file's stem: a lowercase hex UUID — the
/// shape Claude's row names for its store-resume road
/// (`zerocode_core::capabilities::IdShape::Uuid`). The store scan reads the
/// same rule the launch door refuses by, so a file the scan lists is one the
/// launch will take.
pub(super) fn is_claude_session_id(said: &str) -> bool {
    zerocode_core::capabilities::IdShape::Uuid.accepts(said)
}

/// The store directory candidates for one workspace path.
///
/// Claude Code names the folder by flattening the absolute path, but the
/// flattening changed across releases: older stores kept `.` while newer ones
/// turn it into `-` too. Both spellings are tried and the one that exists
/// wins, so a workspace under either era of the store still finds its
/// sessions. (Windows counterpart when that lane opens: `\` and `:` flatten
/// the same way and need the same two-candidate walk.)
pub(super) fn claude_project_slugs(path: &str) -> Vec<String> {
    let separators: String = path
        .chars()
        .map(|one| if one == '/' { '-' } else { one })
        .collect();
    let dots = separators.replace('.', "-");
    let mut said = vec![separators];
    if !said.contains(&dots) {
        said.push(dots);
    }
    said
}

/// The first REAL user message in a session head, or None for a transcript
/// that has none (a subagent's sidechain) — those are not worth re-entering.
/// `<`-headed lines are tool/caveat preambles, not something a person said.
pub(super) fn claude_session_title(head: &str) -> Option<String> {
    for line in head.lines() {
        if !line.contains("\"type\":\"user\"") {
            continue;
        }
        let Ok(row) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if row.get("type").and_then(|v| v.as_str()) != Some("user") {
            continue;
        }
        let content = row.get("message").and_then(|m| m.get("content"));
        let spoken = match content {
            Some(serde_json::Value::String(text)) => text.clone(),
            Some(serde_json::Value::Array(parts)) => parts
                .iter()
                .find_map(|part| {
                    (part.get("type").and_then(|v| v.as_str()) == Some("text"))
                        .then(|| part.get("text").and_then(|v| v.as_str()).unwrap_or(""))
                        .map(str::to_string)
                })
                .unwrap_or_default(),
            _ => String::new(),
        };
        let folded = spoken.split_whitespace().collect::<Vec<_>>().join(" ");
        if folded.is_empty() || folded.starts_with('<') {
            continue;
        }
        let cut: String = folded.chars().take(80).collect();
        return Some(cut);
    }
    None
}

/// The agent a new lane uses unless something says otherwise.
///
/// Stored rather than guessed from what is installed: a machine with four
/// agents on it has no obvious default, and picking the alphabetically first
/// one would be an arbitrary answer presented as a decision.
pub(super) fn stored_default_agent(
    legacy: &LegacySettings<'_>,
) -> zerocode_core::DefaultAgentPreference {
    let Some(text) = legacy.read_text(legacy_settings_file::DEFAULT_AGENT) else {
        return zerocode_core::DefaultAgentPreference::Auto;
    };
    serde_json::from_str::<zerocode_core::DefaultAgentPreference>(&text)
        .ok()
        .and_then(zerocode_core::DefaultAgentPreference::validated)
        // Migration from the original JSON string file.
        .or_else(|| {
            serde_json::from_str::<String>(&text)
                .ok()
                .and_then(|id| zerocode_core::DefaultAgentPreference::from_legacy(&id))
        })
        .unwrap_or_default()
}

/// Where the per-action recipes live.
///
/// One file, keyed by the action id — the same ids Orca keys its recipes by,
/// because a person who saved "resolveConflicts runs codex with these
/// arguments" is naming that string. TEXT actions key the same file: Orca's
/// two tables are one recipe machine, and `commitMessage` is as saveable as
/// `fixChecks`.
///
/// Two scopes inside it, `global` and `byRepo`, which is what 1-cr deferred
/// with a note. A file written before the second scope existed is a flat map
/// and is promoted to the global scope on read (`RecipeBook::parse`).
pub(super) fn launch_recipes_file(config_root: &Path) -> PathBuf {
    config_root.join(artifact_file::LAUNCH_RECIPES)
}

pub(super) fn stored_recipe_book(
    config_root: &Path,
) -> zerocode_core::source_control_ai::RecipeBook {
    std::fs::read_to_string(launch_recipes_file(config_root))
        .ok()
        .map(|text| zerocode_core::source_control_ai::RecipeBook::parse(&text))
        .unwrap_or_default()
}

/// The repository a workspace path names — the key a repo-scoped recipe is
/// filed under.
///
/// The REPOSITORY, not the checkout that was open when somebody pressed save:
/// a recipe saved from one worktree is the same repository's recipe in all of
/// its worktrees, which is what makes this scope "this repository" instead of
/// "this directory". Resolved through the catalog for the reason every other
/// path command is — the webview cannot turn an arbitrary string into a key in
/// a file this window later reads back.
pub(super) fn recipe_scope_key(
    config_root: &Path,
    scope: Option<String>,
) -> Result<Option<String>, String> {
    let Some(path) = scope else {
        return Ok(None);
    };
    Ok(Some(match known_workspace_context(config_root, &path)? {
        KnownWorkspace::Git(orchestrator, _) => orchestrator.repo_root().display().to_string(),
        KnownWorkspace::Folder(folder) => folder.display().to_string(),
    }))
}

/// The scope the window is standing in, for the launch road to resolve
/// against. Free of git: the active context already knows its repository.
pub(super) fn active_recipe_scope(state: &AppState) -> Option<String> {
    Some(
        state
            .active_project_context()
            .repo_root
            .display()
            .to_string(),
    )
}

/// Everything a launch action needs to start, resolved in one place.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ResolvedLaunch {
    pub(super) agent: String,
    /// The base prompt through the recipe's template, trimmed.
    pub(super) prompt: String,
    /// The recipe's extra CLI arguments, split and validated.
    pub(super) args: Vec<String>,
}

/// Everything one `git status` knows, for both panels that need it.
///
/// This used to be two commands. They asked the orchestrator the same
/// question — `pending_loss`, which is one `git status --porcelain -z
/// --ignored` subprocess — and differed only in what they threw away
/// afterwards: the source-control panel wants the changed rows taken
/// apart into staged/changed, the file tree wants a code per path plus the
/// ignored ones so build output reads as `target/` rather than as an ordinary
/// folder. Two commands meant **two git runs per workspace switch**, back to
/// back, on the click's critical path — and on a repository with a large
/// ignored tree that walk is the expensive half.
///
/// Asked through the orchestrator rather than by spawning git here. That path
/// already removes the `GIT_DIR` family of variables, which are set whenever
/// this window is launched from a hook or from a shell in the middle of a
/// rebase and would silently point the command at another repository, and it
/// already pins the locale its own parsing depends on.
///
/// A project that is not a repository has nothing to commit, and that is an
/// empty answer rather than a failure. **git failing is not that**, and this
/// answers with the error instead: a panel that says "변경된 파일이 없습니다"
/// about a repository it could not read is telling someone their work is
/// safe when it does not know.
#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(super) struct WorkingTree {
    pub(super) changed: Vec<ScmEntry>,
    /// Paths git is ignoring, which the tree badges as `!!` and the
    /// source-control panel never lists.
    pub(super) ignored: Vec<String>,
    /// The git operation this checkout is halfway through, when it is holding
    /// conflicts. `None` the rest of the time, and asked for only when a
    /// conflicted row is actually present — the answer costs a `rev-parse` and
    /// four `stat`s, and every workspace switch pays for this call.
    pub(super) operation: Option<&'static str>,
    /// Whether `changed` was cut off at [`SCM_STATUS_LIMIT`]. The window
    /// stands Orca's amber banner on this and offers a retry that asks
    /// uncapped — the flag is judged per answer, never remembered, so a tree
    /// that shrank below the limit simply stops wearing it.
    pub(super) capped: bool,
    /// How many entries the status really held, said only when `capped` —
    /// the banner's honest denominator.
    pub(super) total: usize,
    /// The cap itself, shipped rather than duplicated in the window: the
    /// sentence "처음 {n}개만 표시됩니다" must quote the number this side
    /// actually cut at.
    pub(super) limit: usize,
}

/// How many status entries the panel is handed before the banner takes over.
/// Orca's `DEFAULT_GIT_STATUS_LIMIT = 1e3` (out/main/index.js of 1.4.180):
/// past a thousand rows the list is no longer being read, it is being
/// scrolled past, and every row still costs a DOM node and a numstat lookup.
pub(super) const SCM_STATUS_LIMIT: usize = 1_000;

/// Both discard verbs walk the orchestrator's ONE classified road, Orca's
/// `bulkDiscardChanges` (status.ts:2242-2295): git — not the panel's
/// painted snapshot — says which paths are tracked at discard time, tracked
/// ones are restored to `HEAD`, untracked ones are deleted behind the
/// symlink fence of `git-discard-path-safety.ts`. A row whose kind changed
/// after the paint still gets the right verb instead of erroring the batch
/// or being silently skipped.
pub(super) fn discard_by_git_truth(
    state: State<'_, AppState>,
    paths: Vec<String>,
) -> Result<(), String> {
    let here = state.active();
    let orchestrator = here.orchestrator.ok_or(NOT_A_REPOSITORY)?;
    orchestrator
        .discard_changes(here.root, &paths)
        .map_err(|error| error.to_string())
}

/// What the index held when the commit was refused.
///
/// Best effort: a status that itself fails leaves the list empty, and the
/// prompt already has a line for that case which tells the agent to start with
/// `git status`. Losing the card over it would be the worse trade.
pub(super) fn staged_at_failure(
    orchestrator: &Orchestrator,
    root: &Path,
) -> Vec<zerocode_core::commit_failure::StagedEntry> {
    let Ok(loss) = orchestrator.pending_loss(root) else {
        return Vec::new();
    };
    loss.uncommitted
        .into_iter()
        .filter(|entry| scm_flags(&entry.code).0)
        .map(|entry| zerocode_core::commit_failure::StagedEntry {
            path: entry.path,
            status: staged_status_word(&entry.code).to_string(),
            area: "staged".to_string(),
        })
        .collect()
}

/// Orca's word for one index column (`parseBranchStatusChar`,
/// out/main/index.js:62540-62554). Anything unrecognised is "modified", which
/// is the shape of the sentence rather than a claim about the file.
pub(super) fn staged_status_word(code: &str) -> &'static str {
    match code.chars().next().unwrap_or(' ') {
        'A' => "added",
        'D' => "deleted",
        'R' => "renamed",
        'C' => "copied",
        _ => "modified",
    }
}

/// Where this branch stands against its upstream, for the sync button.
///
/// `None` upstream means the branch has never been published — the button's
/// "publish" state. Counts come from one `rev-list --left-right --count`,
/// which is the same single question Orca's `git.upstreamStatus` answers.
///
/// `behind_commits_are_patch_equivalent` is the third fact, and the one that
/// decides whether a diverged branch is offered a force push at all. `None`
/// means the question was never asked — Orca leaves it `undefined` unless the
/// branch is both ahead AND behind, because that is the only standing in which
/// the answer could change the button.
#[derive(Serialize)]
pub(super) struct UpstreamStatus {
    pub(super) upstream: Option<String>,
    pub(super) ahead: u32,
    pub(super) behind: u32,
    pub(super) behind_commits_are_patch_equivalent: Option<bool>,
}

/// Are the commits only the upstream has just older copies of mine?
///
/// The whole safety of force-with-lease promotion is this one reading of
/// `--cherry-mark`'s output. `=` means "a patch-identical commit exists on the
/// other side", so a listing that is ALL `=` is a remote holding nothing but
/// the pre-rebase, pre-amend versions of my own work — safe to replace. One
/// `+` line is somebody else's commit, and the answer is no.
///
/// Empty output answers `false`, not `true`: "no commits listed" is what a
/// failed or confused read looks like as well, and the conservative reading is
/// the only one that cannot lose work. Orca's `upstreamOnlyCommitsArePatchEquivalent`
/// takes the same position with the same `hasCommit` flag.
pub(super) fn upstream_only_commits_are_patch_equivalent(cherry_mark_output: &str) -> bool {
    let mut has_commit = false;
    for raw in cherry_mark_output.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        has_commit = true;
        if !line.starts_with('=') {
            return false;
        }
    }
    has_commit
}

/// What a refused push WAS, beside what git said about it.
///
/// The window needs the kind and the sentence separately: the kind decides
/// whether the panel re-fetches and re-reads the standing (Orca's
/// `isNonFastForwardRemoteError` loop), the text is what a person reads when
/// none of the known kinds fit.
#[derive(Serialize)]
pub(super) struct PushFailure {
    pub(super) kind: &'static str,
    pub(super) text: String,
}

/// Which refusal this is, from git's own stderr.
///
/// Orca decides this by string matching too (`normalizeGitErrorMessage` in the
/// main process, `resolveRemoteOperationErrorMessage` in the renderer), and the
/// order below is the part that matters: `stale info` is what a BROKEN LEASE
/// looks like, and it arrives alongside the ordinary rejection wording — so a
/// classifier that answered "rejected" first would tell somebody to pull when
/// what they need is a fetch. Lowercased once because git's own casing drifts
/// between the summary line and the hint block.
pub(super) fn classify_push_failure(stderr: &str) -> &'static str {
    let text = stderr.to_lowercase();
    let rejected = text.contains("non-fast-forward")
        || text.contains("fetch first")
        || text.contains("updates were rejected");
    let stale = text.contains("stale info");
    if rejected || stale {
        return if stale { "stale-lease" } else { "rejected" };
    }
    if text.contains("could not read username") || text.contains("authentication failed") {
        return "auth";
    }
    if text.contains("could not resolve host") || text.contains("network is unreachable") {
        return "network";
    }
    if text.contains("no tracking information") || text.contains("no upstream") {
        return "no-upstream";
    }
    "other"
}

/// Take the login out of any URL in a message before it is shown or logged.
///
/// git echoes the remote it was given, and a remote given as
/// `https://user:token@host/repo` puts a live credential into the panel and
/// into every error report copied out of it. Orca strips the same thing
/// (`stripCredentialsFromMessage`) for the same reason.
pub(super) fn strip_credentials(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("://") {
        let (head, tail) = rest.split_at(at + 3);
        out.push_str(head);
        // The authority ends at the first slash or whitespace; everything up to
        // and including the LAST `@` inside it is the userinfo.
        let end = tail
            .find(|c: char| c == '/' || c.is_whitespace())
            .unwrap_or(tail.len());
        let (authority, after) = tail.split_at(end);
        match authority.rfind('@') {
            Some(mark) => out.push_str(&authority[mark + 1..]),
            None => out.push_str(authority),
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

/// What one dirty submodule holds, read when somebody opens its row.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SubmoduleChanges {
    /// Rows whose paths are RELATIVE TO THE SUBMODULE. The window prefixes
    /// them with the row they hang under and stamps that row's path on them,
    /// which is what makes them read-only from here.
    pub(super) entries: Vec<ScmEntry>,
    /// More changes existed than came back.
    pub(super) capped: bool,
}

/// Read something out of git in `root`, or say what git said.
///
/// A closure in each generator before there were three of them; one function
/// now, because the failure mode a shared reader prevents is real — a copy that
/// forgets `!status.success()` reports an empty diff as "nothing staged".
///
/// **This is now a wrapper over the host boundary, and that is deliberate.**
/// The mechanism moved to [`zerocode_core::host::LocalVcs`]; what is left here
/// is one line that resolves the host and forwards. Every caller of this
/// function therefore already crosses the boundary, and the remaining work is
/// to delete this wrapper as each caller learns to carry its own [`Host`] —
/// which [`upstream_status`] has already done. Migrating a dozen call sites at
/// once to prove a point is how a refactor becomes a regression.
pub(super) fn git_text(root: &Path, args: &[&str]) -> Result<String, String> {
    Host::for_workspace(root).vcs().text(root, args)
}

/// Run a one-shot text generation and return what it printed.
///
/// The source-control TEXT actions' road over [`run_once`]: the draft
/// model in plan mode, the prompt the action wrote, and what the run printed
/// — or the action's own sentence for why there is nothing.
pub(super) fn run_text_generation(
    config_root: &Path,
    root: &Path,
    prompt: &str,
    timed_out: &str,
) -> Result<String, String> {
    let program = claude_program().ok_or("claude를 PATH에서 찾지 못했습니다")?;
    let env = claude_reading_env(config_root)?;
    let once = run_once(
        &program,
        Some(root),
        &zerocode_core::commit_message::argv(zerocode_core::commit_message::DEFAULT_MODEL),
        &env,
        prompt,
        zerocode_core::commit_message::GENERATION_TIMEOUT,
    )
    .map_err(|failure| match failure {
        OnceFailure::Spawn(said) => said,
        OnceFailure::TimedOut => timed_out.to_string(),
    })?;
    if !once.success {
        let said = once.stderr_tail.trim();
        return Err(if said.is_empty() {
            "claude가 초안을 만들지 못했습니다".to_string()
        } else {
            said.chars().take(300).collect()
        });
    }
    Ok(once.stdout)
}

/// What one headless run came to: whether it exited well, what it printed,
/// and the tail of what it said on stderr.
pub(crate) struct Once {
    pub(crate) success: bool,
    pub(crate) stdout: String,
    pub(crate) stderr_tail: String,
}

/// Why a headless run never finished.
#[derive(Debug)]
pub(crate) enum OnceFailure {
    /// The program did not start, or could not be waited on.
    Spawn(String),
    /// It outlived its wall, and was ended with every process it started.
    TimedOut,
}

/// The environment a headless `claude` reads the selected account from, its
/// secure store prepared first — a READING door: a one-shot installs no
/// login, and a run that materialized on the way would put a credential
/// write behind an ordinary button press. Every name comes back with its
/// value, an empty value meaning "take it out of the child's environment".
///
/// # Errors
/// The selected account's store could not be prepared.
pub(crate) fn claude_reading_env(config_root: &Path) -> Result<Vec<(String, String)>, String> {
    accounts::prepare_selected_store(config_root)?;
    Ok(accounts::reading_env_for(config_root, "claude"))
}

/// Run an agent's CLI once, headless — `program` with `argv`, under `env`
/// (the account it runs as: [`claude_reading_env`] for Claude Code, the Codex
/// home for Codex).
///
/// The plumbing every one-shot shares — the source-control TEXT actions and
/// the Computer Use generator's login roads (t-10372): the hydrated PATH, the
/// account, the prompt over stdin (a staged diff or a field's words on argv
/// would hit command-line limits and sit in every `ps`), both pipes drained
/// on their own threads, a bounded wait that KILLS — the run's whole process
/// group — rather than merely stops waiting, and a bounded read so a
/// generator that starts streaming a conversation is cut off instead of
/// buffered.
///
/// A one-shot is no pane's: the coordinates that tie a process to a pane
/// ([`crate::hooks::PANE_COORDINATES`]) are taken out of its environment, so
/// nothing it does can be heard as a pane's turn or counted in a pane's
/// census, whichever process started the window.
///
/// Factored out at the second caller rather than the third: each of those is a
/// lesson this window already paid for once, and a copy that forgets one of
/// them fails in a way nobody notices until it matters.
pub(crate) fn run_once(
    program: &str,
    cwd: Option<&Path>,
    argv: &[String],
    env: &[(String, String)],
    prompt: &str,
    wall: Duration,
) -> Result<Once, OnceFailure> {
    use std::io::{Read, Write};
    let deadline = std::time::Instant::now() + wall;
    let mut command = crate::proc::quiet_command(program);
    command
        .args(argv)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    if let Some(path) = shell_path::hydrated() {
        command.env("PATH", path);
    }
    for name in crate::hooks::PANE_COORDINATES {
        command.env_remove(name);
    }
    // The account the picker names — same contract as every other spawn: an
    // empty value means "take it out of the child's environment".
    for (name, value) in env {
        if value.is_empty() {
            command.env_remove(name);
        } else {
            command.env(name, value);
        }
    }
    #[cfg(unix)]
    crate::codex_queue::prepare_process_group(&mut command);
    let mut child = command
        .spawn()
        .map_err(|error| OnceFailure::Spawn(error.to_string()))?;

    // The whole prompt, then EOF — `-p` reads stdin to the end, and a stdin
    // left open is a generator that never starts.
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(prompt.as_bytes());
    }
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    // The reader says when the run closed its output, so a run that answers
    // is read the moment it does rather than at the next poll.
    let (closed, heard) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut held = Vec::new();
        if let Some(pipe) = stdout.as_mut() {
            let _ = pipe
                .by_ref()
                .take(zerocode_core::commit_message::MAX_OUTPUT_BYTES as u64)
                .read_to_end(&mut held);
        }
        let _ = closed.send(());
        held
    });
    let stderr_drain = std::thread::spawn(move || {
        let mut tail = Vec::new();
        if let Some(pipe) = stderr.as_mut() {
            let _ = pipe.by_ref().take(16 * 1024).read_to_end(&mut tail);
        }
        tail
    });
    let left = deadline.saturating_duration_since(std::time::Instant::now());
    let _ = heard.recv_timeout(left);
    let status = loop {
        let failure = match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(GENERATION_POLL_EVERY);
                continue;
            }
            Ok(None) => OnceFailure::TimedOut,
            Err(error) => OnceFailure::Spawn(error.to_string()),
        };
        // The run's process group was made for it alone, so everything it
        // started goes with it.
        #[cfg(unix)]
        let _ = crate::codex_queue::signal_process_group(child.id(), libc::SIGKILL);
        let _ = child.kill();
        let _ = child.wait();
        return Err(failure);
    };
    let held = reader.join().unwrap_or_default();
    let tail = stderr_drain.join().unwrap_or_default();
    Ok(Once {
        success: status.success(),
        stdout: String::from_utf8_lossy(&held).into_owned(),
        stderr_tail: String::from_utf8_lossy(&tail).into_owned(),
    })
}

/// What a generated pull request came back as, on its way to the dialog.
#[derive(Serialize)]
pub(super) struct PullRequestDraft {
    pub(super) base: String,
    pub(super) title: String,
    pub(super) body: String,
    pub(super) draft: bool,
}

pub(super) fn draft_pull_request(
    config_root: &Path,
    root: &Path,
    base: &str,
    title: &str,
    body: &str,
    draft: bool,
) -> Result<PullRequestDraft, String> {
    use zerocode_core::source_control_ai as ai;
    let base = base.trim();
    // Orca's own guard (`getPullRequestDraftContext`, out/main/index.js:108713)
    // and it is a real one: this string reaches `git merge-base` as an
    // argument, and one that opens with `-` is read as a flag.
    if base.is_empty() || base.starts_with('-') {
        return Err("비교할 기준 브랜치가 없습니다".to_string());
    }
    // The merge base, not the base tip: a branch whose base has moved on since
    // it forked would otherwise have every commit that landed on the base
    // reported as part of THIS change.
    let merge_base = git_text(root, &["merge-base", base, "HEAD"])
        .map_err(|_| format!("{base}와(과) 갈라진 지점을 찾지 못했습니다"))?;
    let merge_base = merge_base.trim();
    if merge_base.is_empty() {
        return Err(format!("{base}와(과) 갈라진 지점을 찾지 못했습니다"));
    }
    // No fetch of our own, deliberately — Orca fetches the base first
    // (`fetchComparisonBase`, :108691). A generate button that silently goes to
    // the network can hang on a slow remote and fail on a plane, and this
    // window has an explicit 가져오기 for that. The cost is a base that may be
    // behind, which widens the range rather than corrupting it.
    let range = format!("{merge_base}..HEAD");
    let commit_summary = git_text(
        root,
        &["log", "--pretty=format:- %s", "--max-count=50", &range],
    )
    .unwrap_or_default();
    let change_summary = git_text(root, &["diff", "--name-status", &range]).unwrap_or_default();
    let patch = git_text(
        root,
        &[
            "diff",
            "--patch",
            "--minimal",
            "--no-color",
            "--no-ext-diff",
            &range,
        ],
    )
    .unwrap_or_default();
    if commit_summary.trim().is_empty()
        && change_summary.trim().is_empty()
        && patch.trim().is_empty()
    {
        return Err(format!("{base} 이후로 달라진 것이 없습니다"));
    }

    let context = ai::PullRequestContext {
        branch: current_branch(root),
        base: base.to_string(),
        current_title: title.to_string(),
        current_body: body.to_string(),
        current_draft: draft,
        commit_summary: commit_summary.trim_end().to_string(),
        change_summary: change_summary.trim_end().to_string(),
        // The same fair split the commit drafter uses: a naive head cut spends
        // the whole budget on whichever file sorts first, and the description
        // comes back about one file of a forty-file branch.
        patch: zerocode_core::commit_message::truncate_diff_for_prompt(
            &patch,
            zerocode_core::commit_message::STAGED_DIFF_BYTE_BUDGET,
        ),
    };
    let prompt = ai::pull_request_prompt(&context, "");
    let raw = run_text_generation(
        config_root,
        root,
        &prompt,
        "PR 초안 생성이 60초를 넘겨 중단했습니다",
    )?;
    let fields = ai::parse_pull_request_fields(&raw, &context)?;
    Ok(PullRequestDraft {
        base: fields.base,
        title: fields.title,
        body: fields.body,
        draft: fields.draft,
    })
}

/// What the pull-request dialog opens holding.
#[derive(Serialize)]
pub(super) struct PullRequestSeed {
    /// The branch a review would target — see [`hosted_review_base`].
    pub(super) base: Option<String>,
    pub(super) branch: Option<String>,
    /// The subject of the newest commit this branch has that its base does
    /// not — the title somebody would type anyway on a one-commit branch.
    pub(super) title: String,
}

/// The branch a review targets, in the form a provider takes it: a plain
/// branch name, never `origin/main`.
///
/// One spelling for the whole review road — the door's ladder and the
/// composer's seed must never disagree about what the base is, or the door
/// refuses a branch the dialog would happily have opened.
///
/// `git` writes `refs/remotes/origin/HEAD` at clone time and never again, so
/// a repository made with `git init` (or one whose remote was added later)
/// has no such ref — hence [`default_base_probe`]'s named fallback rather
/// than that one read alone. Its answer is `remote/branch` when it came from
/// the remote and a bare local name when it came from the fallback; the split
/// is at the FIRST slash because everything after it is the branch, slashes
/// and all (`origin/release/1.0` → `release/1.0`).
pub(super) fn hosted_review_base(host: &Host, root: &Path) -> Option<String> {
    let probed = default_base_probe(host, root)?;
    Some(match probed.split_once('/') {
        Some((_, branch)) if !branch.is_empty() => branch.to_string(),
        _ => probed,
    })
}

/// Why a checkout cannot open a review yet, and what would clear it.
///
/// The names are the original's (`hosted-review.ts:139-149`) and so is the
/// order they are decided in (`hosted-review-creation.ts:555-620`): a rung
/// shadows every rung below it, so a detached HEAD is reported as one even
/// when the tree is also dirty. The rungs this window has no producer for —
/// `auth_required` (a `gh` probe), `fork_head_unsupported`,
/// `base_not_on_remote` — are absent rather than guessed.
#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum HostedReviewBlock {
    DetachedHead,
    ExistingReview,
    UnsupportedProvider,
    DefaultBranch,
    Dirty,
    NoUpstream,
    NeedsSync,
    AuthRequired,
    NeedsPush,
}

/// The one step that would clear the blocker standing.
#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum HostedReviewStep {
    Commit,
    Publish,
    Sync,
    Authenticate,
    Push,
    OpenExistingReview,
}

/// Everything [`hosted_review_ladder`] reads, gathered in one place so the
/// walk itself is a pure function a test can drive rung by rung.
///
/// The split is the original's own: its main process resolves the branch and
/// the base, while the renderer hands over the facts it is already holding
/// (`HostedReviewCreationEligibilityArgs` — `hasUncommittedChanges`,
/// `hasUpstream`, `ahead`, `behind`, a linked review). Asking git a second
/// time for what the panel just painted would be two answers to one question.
pub(super) struct HostedReviewFacts<'a> {
    pub(super) branch: Option<String>,
    pub(super) base: Option<String>,
    /// Whether a remote exists at all.
    pub(super) remote: bool,
    /// Which forge that remote belongs to, when its host says so.
    ///
    /// `None` is NOT "no forge": a GitHub Enterprise install is an ordinary
    /// hostname nobody can recognise from its spelling, and `gh` speaks to it
    /// perfectly well. Only a POSITIVE reading of some other forge closes the
    /// create road — and then the manual link is the road instead.
    pub(super) provider: Option<remote_repo::Provider>,
    /// The provider's own review page for this branch, when one can be named.
    /// Carried through the walk rather than built from its answer, because
    /// every rung that refuses the create road is a rung where this link is
    /// the only road left.
    pub(super) manual_url: Option<String>,
    /// Whether `gh` can act on this repository's host — asked LAZILY, because
    /// it is a process and a network round trip, and most walks stop at a rung
    /// above it. The original is lazy for the same reason: its probe is
    /// `await`ed at the rung, not gathered with the other facts.
    pub(super) authenticated: &'a dyn Fn() -> bool,
    /// A review already standing for this branch.
    pub(super) review: bool,
    pub(super) dirty: bool,
    /// `None` when the upstream standing is not known — which is NOT the same
    /// as "no upstream", and the ladder keeps them apart.
    pub(super) upstream: Option<bool>,
    pub(super) ahead: u32,
    pub(super) behind: u32,
}

/// Whether this checkout can open a review, and what stands in the way.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub(super) struct HostedReviewEligibility {
    pub(super) branch: Option<String>,
    pub(super) base: Option<String>,
    pub(super) can_create: bool,
    pub(super) blocked_reason: Option<HostedReviewBlock>,
    pub(super) next_action: Option<HostedReviewStep>,
    /// The forge this checkout's remote belongs to, when its host says so.
    pub(super) provider: Option<remote_repo::Provider>,
    /// The provider's own "open a review" page for this branch — the road for
    /// every checkout `gh` cannot create on, and a second one for those it
    /// can. Absent whenever a link would land on nothing; see
    /// [`remote_repo::manual_review_url`].
    pub(super) manual_url: Option<String>,
}

/// Walk the rungs in the measured order.
///
/// Two of them are easy to collapse and must not be. An unknown upstream
/// stops the walk with NO blocker (`hasUpstream !== true` → `blockedReason:
/// null`): nothing has been proven, and naming a blocker would send somebody
/// to publish a branch that may already be published. And the last rung needs
/// a base — `canCreate: Boolean(baseBranch)` — because a review with nothing
/// to target is not a review.
pub(super) fn hosted_review_ladder(facts: &HostedReviewFacts<'_>) -> HostedReviewEligibility {
    let branch = facts
        .branch
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty());
    let base = facts
        .base
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty());
    let stop = |reason, step| HostedReviewEligibility {
        branch: branch.map(str::to_string),
        base: base.map(str::to_string),
        can_create: false,
        blocked_reason: reason,
        next_action: step,
        provider: facts.provider,
        manual_url: facts.manual_url.clone(),
    };
    // `HEAD` is what git answers with when no branch is checked out, and it
    // is a legal branch name nobody has: both readings end the same way.
    let Some(branch) = branch.filter(|name| *name != "HEAD") else {
        return stop(Some(HostedReviewBlock::DetachedHead), None);
    };
    if facts.review {
        return stop(
            Some(HostedReviewBlock::ExistingReview),
            Some(HostedReviewStep::OpenExistingReview),
        );
    }
    // No remote at all, or one that positively belongs to a forge this
    // window's `gh` road cannot create on. An UNRECOGNISED host is not this:
    // GitHub Enterprise is an ordinary hostname and `gh` handles it.
    if !facts.remote
        || facts
            .provider
            .is_some_and(|provider| provider != remote_repo::Provider::Github)
    {
        return stop(Some(HostedReviewBlock::UnsupportedProvider), None);
    }
    // Case-folded, because a review from `Main` onto `main` is the same
    // refusal (`branch.toLowerCase() === baseBranch.toLowerCase()`).
    if base.is_some_and(|base| base.to_lowercase() == branch.to_lowercase()) {
        return stop(Some(HostedReviewBlock::DefaultBranch), None);
    }
    if facts.dirty {
        return stop(
            Some(HostedReviewBlock::Dirty),
            Some(HostedReviewStep::Commit),
        );
    }
    if facts.upstream == Some(false) {
        return stop(
            Some(HostedReviewBlock::NoUpstream),
            Some(HostedReviewStep::Publish),
        );
    }
    if facts.upstream != Some(true) {
        return stop(None, None);
    }
    if facts.behind > 0 {
        return stop(
            Some(HostedReviewBlock::NeedsSync),
            Some(HostedReviewStep::Sync),
        );
    }
    // Before the push and after the sync, which is the original's own place
    // for it. A branch that still has to be pushed is not the thing to say
    // when the account that would open the review is not signed in.
    if !(facts.authenticated)() {
        return stop(
            Some(HostedReviewBlock::AuthRequired),
            Some(HostedReviewStep::Authenticate),
        );
    }
    if facts.ahead > 0 {
        return stop(
            Some(HostedReviewBlock::NeedsPush),
            Some(HostedReviewStep::Push),
        );
    }
    HostedReviewEligibility {
        branch: Some(branch.to_string()),
        base: base.map(str::to_string),
        can_create: base.is_some(),
        blocked_reason: None,
        next_action: None,
        provider: facts.provider,
        manual_url: facts.manual_url.clone(),
    }
}

/// How long `gh`'s authentication standing is believed without asking again.
///
/// The probe is a process AND a token validation over the network, and the
/// ladder is walked whenever a fact under it moves — a commit, a push, a
/// branch. Two minutes is the distance between "the door follows a
/// `gh auth login` while somebody is still looking at the terminal they ran
/// it in" and "every commit spends a round trip on a question whose answer
/// changes about once a month". The original spends one per resolve.
pub(super) const HOSTED_REVIEW_AUTH_FRESH: Duration = Duration::from_secs(120);

/// What `gh` last said about each host's accounts, and when.
pub(super) type HostedReviewAuthMemory = HashMap<String, (Instant, bool)>;

pub(super) fn hosted_review_auth_cache() -> std::sync::MutexGuard<'static, HostedReviewAuthMemory> {
    static STANDING: std::sync::OnceLock<Mutex<HostedReviewAuthMemory>> =
        std::sync::OnceLock::new();
    STANDING
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Whether `gh` can act on this repository's host, asked at most once every
/// [`HOSTED_REVIEW_AUTH_FRESH`].
///
/// Keyed by the REMOTE, because that is what decides the host the answer is
/// about; two checkouts of the same repository share one answer, and a
/// checkout of somebody else's does not borrow it.
pub(super) fn hosted_review_authenticated(root: &Path, remote: Option<&str>) -> bool {
    let key = remote.unwrap_or_default().to_string();
    {
        let mut cache = hosted_review_auth_cache();
        let now = Instant::now();
        cache.retain(|_, (at, _)| now.duration_since(*at) < HOSTED_REVIEW_AUTH_FRESH);
        if let Some((_, standing)) = cache.get(&key) {
            return *standing;
        }
    }
    let standing = gh::is_authenticated(root, remote);
    hosted_review_auth_cache().insert(key, (Instant::now(), standing));
    standing
}

pub(super) fn draft_branch_name(
    config_root: &Path,
    root: &Path,
    work: &str,
    prefix: &str,
) -> Result<String, String> {
    use zerocode_core::source_control_ai as ai;
    let context = ai::BranchNameContext {
        first_prompt: work.to_string(),
        assistant_message: None,
    };
    let prompt = ai::branch_name_prompt(&context, "");
    let raw = run_text_generation(
        config_root,
        root,
        &prompt,
        "이름 짓기가 60초를 넘겨 중단했습니다",
    )?;
    let slug = ai::sanitize_branch_slug(&raw, ai::MAX_BRANCH_NAME_WORDS);
    // The prefix the branch machinery adds comes back off, so a model that
    // helpfully included it does not produce `joe/joe-fix-login`.
    let slug = ai::strip_configured_branch_prefix(&slug, prefix);
    if slug.is_empty() {
        return Err("쓸 만한 이름이 나오지 않았습니다".to_string());
    }
    Ok(slug)
}

pub(super) fn draft_commit_message(config_root: &Path, root: &Path) -> Result<String, String> {
    let summary = git_text(root, &["diff", "--cached", "--name-status"])?;
    if summary.trim().is_empty() {
        return Err("스테이지된 변경이 없습니다".to_string());
    }
    let patch = git_text(root, &["diff", "--cached"])?;
    let prompt = zerocode_core::commit_message::prompt(
        current_branch(root).as_deref(),
        summary.trim_end(),
        &patch,
    );
    let raw = run_text_generation(
        config_root,
        root,
        &prompt,
        "초안 생성이 60초를 넘겨 중단했습니다",
    )?;
    Ok(zerocode_core::commit_message::tidy(&raw))
}

/// The commit graph, drawn: rows with their geometry, and the two refs the
/// panel names in badges and boundary rows.
#[derive(Serialize)]
pub(super) struct GitHistory {
    pub(super) rows: Vec<zerocode_core::git_graph::HistoryRow>,
    /// The log was cut at the limit — there is more past the last row.
    pub(super) has_more: bool,
    pub(super) current: Option<zerocode_core::git_graph::HistoryRef>,
    pub(super) remote: Option<zerocode_core::git_graph::HistoryRef>,
}

/// Orca's page size and ceiling for the history log (out/main/index.js:106760).
pub(super) const GIT_HISTORY_DEFAULT_LIMIT: usize = 50;
pub(super) const GIT_HISTORY_MAX_LIMIT: usize = 200;

pub(super) fn read_git_history(root: &Path, limit: Option<usize>) -> Result<GitHistory, String> {
    use zerocode_core::git_graph::{self, HistoryRef};
    let limit = limit
        .unwrap_or(GIT_HISTORY_DEFAULT_LIMIT)
        .clamp(1, GIT_HISTORY_MAX_LIMIT);
    let git = |args: &[&str]| -> Result<String, String> {
        let out = crate::proc::quiet_command("git")
            .args(args)
            .current_dir(root)
            .output()
            .map_err(|error| error.to_string())?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
    };

    // No commits yet — or no repository — is an ordinary state the panel
    // shows as an empty history, not as an error banner.
    let Ok(head) = git(&["rev-parse", "HEAD"]) else {
        return Ok(GitHistory {
            rows: Vec::new(),
            has_more: false,
            current: None,
            remote: None,
        });
    };
    let current = match git(&["symbolic-ref", "--quiet", "--short", "HEAD"]) {
        Ok(name) if !name.is_empty() => HistoryRef {
            id: format!("refs/heads/{name}"),
            name,
            revision: head.clone(),
            category: "branches".to_string(),
            color: None,
        },
        // Detached: the current ref is the commit itself, named by its
        // short hash the way Orca names it.
        _ => HistoryRef {
            id: head.clone(),
            name: head.chars().take(7).collect(),
            revision: head.clone(),
            category: "commits".to_string(),
            color: None,
        },
    };
    let remote = git(&[
        "for-each-ref",
        "--format=%(upstream)%00%(upstream:short)",
        &format!("refs/heads/{}", current.name),
    ])
    .ok()
    .and_then(|said| {
        let (full, short) = said.split_once('\0')?;
        if full.is_empty() {
            return None;
        }
        let revision = git(&["rev-parse", full]).ok()?;
        Some(HistoryRef {
            id: full.to_string(),
            name: if short.is_empty() {
                full.trim_start_matches("refs/remotes/").to_string()
            } else {
                short.to_string()
            },
            revision,
            category: "remote branches".to_string(),
            color: None,
        })
    });
    let merge_base = remote
        .as_ref()
        .and_then(|remote| git(&["merge-base", &current.revision, &remote.revision]).ok())
        .filter(|base| !base.is_empty());

    let ask = format!("-n{}", limit + 1);
    let format = format!("--format={}", git_graph::COMMIT_FORMAT);
    let log = git(&[
        "log",
        "-z",
        "--topo-order",
        "--decorate=full",
        &ask,
        &format,
    ])?;
    let mut items = git_graph::parse_history_log(&log);
    let has_more = items.len() > limit;
    items.truncate(limit);

    let has_incoming = remote
        .as_ref()
        .zip(merge_base.as_deref())
        .is_some_and(|(remote, merge_base)| remote.revision != merge_base);
    let has_outgoing = merge_base
        .as_deref()
        .is_some_and(|merge_base| current.revision != merge_base);
    let rows = git_graph::history_rows(
        items,
        Some(&current),
        remote.as_ref(),
        None,
        has_incoming,
        has_outgoing,
        merge_base.as_deref(),
    );
    Ok(GitHistory {
        rows,
        has_more,
        current: Some(current),
        remote,
    })
}

/// A commit id exactly as this window's own history reported it — it goes back
/// onto a git command line, so nothing but a hex object name may pass. The
/// history rows are the only place these ids are minted, but this channel is
/// the webview's, and a check at the door beats trusting the caller.
pub(super) fn history_hash(id: &str) -> Result<&str, String> {
    let hex = (4..=40).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_hexdigit());
    if hex {
        Ok(id)
    } else {
        Err("커밋이 아닙니다".to_string())
    }
}

/// What one history row opens into: the whole message the subject was cut
/// from, and the files the commit touched.
#[derive(Serialize)]
pub(super) struct CommitSheet {
    pub(super) message: String,
    pub(super) entries: Vec<zerocode_core::git_graph::CommitFile>,
}

/// Both sides of one file at one commit. The before side is read under the
/// origin name — a renamed file is filed under its old name in the parent —
/// and either side failing to exist is a fact (`added`, `deleted`), not an
/// error: the empty string is what "it was not there" looks like to a diff.
pub(super) fn commit_documents(
    root: &Path,
    id: &str,
    path: &str,
    origin: Option<&str>,
) -> Option<DiffTexts> {
    let before = origin.unwrap_or(path);
    let original = git_text(root, &["show", &format!("{id}^:{before}")]).unwrap_or_default();
    let modified = git_text(root, &["show", &format!("{id}:{path}")]).unwrap_or_default();
    if texts_render_limit(&original, &modified).is_some() {
        return None;
    }
    Some(DiffTexts {
        original,
        modified,
        version: None,
    })
}

/// One line of a diff, as the review surface draws it — the conversation's
/// inline diff draws the same row (`zerocode_core::compact_diff`).
pub(super) use zerocode_core::compact_diff::DiffLine;

/// The `-old +new` starting lines a hunk header declares.
pub(super) fn hunk_starts(header: &str) -> Option<(u32, u32)> {
    let ranges = header.strip_prefix("@@ ")?.split_once(" @@")?.0;
    let (old, new) = ranges.split_once(' ')?;
    let start = |range: &str| range.split(',').next()?.parse::<u32>().ok();
    Some((
        start(old.strip_prefix('-')?)?,
        start(new.strip_prefix('+')?)?,
    ))
}

/// Parse a unified diff into numbered lines.
///
/// Pure, because the numbering is both the part that is easy to get wrong and
/// the part a reviewer trusts without checking: a hunk header says where each
/// side restarts, an added line advances only the new side, a removed line
/// only the old, and `\ No newline at end of file` advances neither, being a
/// note about the line above rather than a line of its own.
pub(super) fn parse_unified_diff(diff: &str) -> Vec<DiffLine> {
    let mut lines = Vec::new();
    let (mut old, mut new) = (0, 0);
    let mut in_hunk = false;

    for raw in diff.lines() {
        // A new file's header ends the previous file's hunks.
        //
        // Without this the next file's `---` and `+++` lines are read as body:
        // they start with `-` and `+`, so they become a removed line and an added
        // line, numbered into whatever the last hunk was counting. It shows up on
        // any diff carrying more than one file — which `file_diff` already
        // produces for a RENAME, since that is asked about under both names.
        if raw.starts_with("diff --git ") {
            in_hunk = false;
        }
        if raw.starts_with("@@") {
            (old, new) = hunk_starts(raw).unwrap_or((old, new));
            in_hunk = true;
            lines.push(DiffLine::marker("hunk", raw));
            continue;
        }
        if !in_hunk {
            lines.push(DiffLine::marker("meta", raw));
            continue;
        }
        let mut body = raw.chars();
        match body.next() {
            Some('+') => {
                lines.push(DiffLine {
                    kind: "add".into(),
                    text: body.as_str().to_string(),
                    old: None,
                    new: Some(new),
                });
                new += 1;
            }
            Some('-') => {
                lines.push(DiffLine {
                    kind: "del".into(),
                    text: body.as_str().to_string(),
                    old: Some(old),
                    new: None,
                });
                old += 1;
            }
            Some('\\') => lines.push(DiffLine::marker("meta", raw)),
            // A context line, including the empty one git writes as a bare
            // newline rather than as a space.
            _ => {
                lines.push(DiffLine {
                    kind: "ctx".into(),
                    text: body.as_str().to_string(),
                    old: Some(old),
                    new: Some(new),
                });
                old += 1;
                new += 1;
            }
        }
    }
    lines
}

/// Orca's own render ceiling (`large-diff-render-limit`):
/// `MAX_RENDERED_DIFF_LINES_PER_SIDE = 120_000`,
/// `MAX_RENDERED_DIFF_COMBINED_CHARACTERS = 6_000_000`.
///
/// Applied HERE, to the raw unified diff, before anything is parsed or
/// serialized — the window never receives a payload it could only choke on,
/// which is one step earlier than Orca can afford (its limit runs in the
/// renderer, after the content already crossed). Orca measures the two file
/// contents its editor would mount; this window renders the diff itself, so
/// the budget rides the diff's own text — same numbers, same order:
/// characters first because length is free, then lines counted only up to
/// the limit, so a pathological file costs a bounded walk and nothing more.
pub(super) const MAX_DIFF_LINES: usize = 120_000;
pub(super) const MAX_DIFF_CHARACTERS: usize = 6_000_000;

/// Why a diff was withheld, with the figures the fallback card shows.
#[derive(Serialize, Debug, PartialEq)]
pub(super) struct DiffLimit {
    /// `"line-count"` or `"character-count"` — Orca's own two reasons.
    pub(super) reason: &'static str,
    /// Lines counted up to the limit; zero when the character check refused
    /// first, which Orca's card prints as "not counted".
    pub(super) line_count: usize,
    /// The count stopped at the ceiling rather than finishing — the card
    /// wears a `+` for it.
    pub(super) line_count_is_minimum: bool,
    pub(super) character_count: usize,
    pub(super) max_lines: usize,
    pub(super) max_characters: usize,
}

/// Lines in `text`, counted only as far as one past `max`.
///
/// Past the ceiling the answer is already "more than we will draw", and the
/// rest of the walk buys a bigger number nobody reads. Shared by the two
/// questions below so a pathological file costs the same bounded walk whether
/// it is asked about as a diff or as the pair of documents behind one.
pub(super) fn lines_up_to(text: &str, max: usize) -> usize {
    let mut lines = 0usize;
    for byte in text.bytes() {
        if byte == b'\n' {
            lines += 1;
            if lines > max {
                return lines;
            }
        }
    }
    lines
}

pub(super) fn over_ceiling(
    reason: &'static str,
    line_count: usize,
    character_count: usize,
) -> DiffLimit {
    DiffLimit {
        reason,
        line_count,
        line_count_is_minimum: true,
        character_count,
        max_lines: MAX_DIFF_LINES,
        max_characters: MAX_DIFF_CHARACTERS,
    }
}

/// The ceiling, asked of a raw diff. `None` means it fits.
pub(super) fn diff_render_limit(diff: &str) -> Option<DiffLimit> {
    if diff.len() > MAX_DIFF_CHARACTERS {
        return Some(over_ceiling("character-count", 0, diff.len()));
    }
    let lines = lines_up_to(diff, MAX_DIFF_LINES);
    if lines > MAX_DIFF_LINES {
        return Some(over_ceiling("line-count", lines, diff.len()));
    }
    None
}

/// The same ceiling, asked of the two documents a merge view would mount.
///
/// A SECOND question, not a second answer. The diff and the pair of files it
/// was computed from are different sizes, and it is the pair the editor holds
/// in memory: a 6 MB generated file with one line changed produces a four-line
/// diff that sails through [`diff_render_limit`] and twelve megabytes of
/// document. Orca's two constants are named for exactly these operands —
/// `MAX_RENDERED_DIFF_LINES_PER_SIDE` and
/// `MAX_RENDERED_DIFF_COMBINED_CHARACTERS` — so this is the ceiling read the
/// way it was written, against the side and against the sum.
pub(super) fn texts_render_limit(original: &str, modified: &str) -> Option<DiffLimit> {
    let characters = original.len() + modified.len();
    if characters > MAX_DIFF_CHARACTERS {
        return Some(over_ceiling("character-count", 0, characters));
    }
    let lines = lines_up_to(original, MAX_DIFF_LINES).max(lines_up_to(modified, MAX_DIFF_LINES));
    if lines > MAX_DIFF_LINES {
        return Some(over_ceiling("line-count", lines, characters));
    }
    None
}

/// The two documents a merge view mounts, and what a save has to hand back.
///
/// Present or absent as a unit, because half of it is not a diff: an editor
/// with a modified side and no committed side to compare it against is a file
/// tab wearing the wrong header. Absent whenever anything at all refused —
/// the ceiling, an unreadable working copy, a diff with no changed lines in
/// it — and the window falls back to the rows it has always drawn.
#[derive(Serialize)]
pub(super) struct DiffTexts {
    /// `git show HEAD:<path>` — the committed side. Never editable, which is
    /// Orca's `originalEditable: false` and is also just true: there is no
    /// file on disk that this text is.
    pub(super) original: String,
    /// The working file, which is the one on disk and the one a save writes.
    pub(super) modified: String,
    /// The stamp [`write_text_file`] demands back, or absent when there is no
    /// working file to stamp — a deletion. That absence IS the read-only
    /// signal (Orca's `readOnly: !editable`): a document the window could not
    /// save is a document it must not offer to edit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) version: Option<String>,
}

/// One file's diff, or the reason it was withheld — never both.
#[derive(Serialize)]
pub(super) struct DiffView {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) limit: Option<DiffLimit>,
    pub(super) lines: Vec<DiffLine>,
    /// The pair the merge view mounts. Absent on every road the rows still
    /// have to cover, which is why this is an option rather than two empty
    /// strings: "" is a real document (a file that was added, a file that was
    /// deleted) and cannot also mean "there is nothing here".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) texts: Option<DiffTexts>,
}

/// The committed side and the working side, for the merge view.
///
/// Reached only after [`diff_render_limit`] passed, and refuses again for
/// itself — so there is no road on which a withheld file reaches the editor
/// through this door. `git show` is the reader because it is the reader:
/// [`git_text`] already owns "run git and say what git said", and a second
/// runner here would be a second place that forgets to check the exit status.
pub(super) fn diff_documents(root: &Path, path: &str, origin: Option<&str>) -> Option<DiffTexts> {
    // A rename is asked about under its new name, but the committed text is
    // filed under the old one — `HEAD:<new>` on a renamed file is a path git
    // has never heard of.
    let committed = origin.unwrap_or(path);
    // Failure is a fact, not an error: a file that was added has no committed
    // side, and the empty string is what "it did not exist yet" looks like to
    // a diff.
    let original = git_text(root, &["show", &format!("HEAD:{committed}")]).unwrap_or_default();
    let target = resolve_in_project(root, path).ok()?;
    let (modified, version) = match read_text_at(&target) {
        Ok(file) => (file.text, Some(file.version)),
        // A deletion: the file is gone, so the modified side is empty and
        // there is no stamp — which is what makes this view read-only.
        Err(_) if !target.exists() => (String::new(), None),
        // Binary, or past the editor's own 4 MiB read cap. Nothing to mount,
        // and the rows below still describe it perfectly well.
        Err(_) => return None,
    };
    if texts_render_limit(&original, &modified).is_some() {
        return None;
    }
    Some(DiffTexts {
        original,
        modified,
        version,
    })
}

/// Whether git's index holds this path — `ls-files --error-unmatch`, answered
/// by its exit code. `:(literal)` for the same reason every pathspec in this
/// file carries it: a pathspec is a glob by default, and a file honestly named
/// `star*name.txt` would otherwise answer for whatever the pattern matched.
pub(super) fn git_tracks(root: &Path, path: &str) -> bool {
    git_text(
        root,
        &[
            "ls-files",
            "--error-unmatch",
            "--",
            &format!(":(literal){path}"),
        ],
    )
    .is_ok()
}

/// An untracked file, spelled as the unified diff a staged-new file would get:
/// header, `new file mode`, one hunk of nothing → everything. Synthesizing the
/// TEXT rather than the rows is the point — it walks through the same size
/// gate and the same parser as every real answer, so there is no second shape
/// to keep honest. Unreadable (binary, past the editor's read cap, escaped the
/// project) stays the empty answer it already was.
pub(super) fn untracked_as_added(root: &Path, path: &str) -> String {
    let Ok(target) = resolve_in_project(root, path) else {
        return String::new();
    };
    let Ok(file) = read_text_at(&target) else {
        return String::new();
    };
    // git itself writes no hunk for an empty new file — the metadata is the
    // whole statement, and `@@ -0,0 +0,0 @@` would be a hunk that says nothing.
    if file.text.is_empty() {
        return format!("diff --git a/{path} b/{path}\nnew file mode 100644\n");
    }
    let count = file.text.lines().count();
    let body: String = file.text.lines().map(|line| format!("+{line}\n")).collect();
    format!(
        "diff --git a/{path} b/{path}\nnew file mode 100644\n--- /dev/null\n+++ b/{path}\n@@ -0,0 +1,{count} @@\n{body}"
    )
}

/// One file's section of a whole-worktree diff.
///
/// Split from one answer where the COMMITTED comparison still asks once
/// (`worktree_committed_diff` — its base is a merge-base question, not a
/// per-file one). The uncommitted combined view stopped riding this road in
/// G3: it lists entries first and asks `file_diff` per section as one scrolls
/// into view — the original's own lazy contract (`loadSection`,
/// CombinedDiffViewer.tsx), because a person who reads the first file of two
/// hundred should not pay for the other hundred and ninety-nine up front.
#[derive(Serialize, Debug, PartialEq)]
pub(super) struct DiffSection {
    pub(super) path: String,
    /// Where a rename came from, so the header can name both ends.
    pub(super) origin: Option<String>,
    /// Git's semantic file state, for the combined-diff file tree. The tree
    /// must not guess from +/- counts: a modified file may contain only adds.
    pub(super) status: &'static str,
    pub(super) added: usize,
    pub(super) removed: usize,
    pub(super) lines: Vec<DiffLine>,
    /// Present when this file's diff was withheld for size — the section
    /// paints the fallback card instead of rows, and `lines` is empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) limit: Option<DiffLimit>,
}

/// The two paths a `diff --git a/x b/y` line names, as best it can.
///
/// **This is a fallback and it is ambiguous.** git writes the two paths
/// unquoted and separated by a space, so a file whose own name contains ` b/`
/// makes the boundary undecidable from this line alone:
/// `diff --git a/x b/y.txt b/x b/y.txt` has three candidate split points and
/// nothing in the line says which is right.
///
/// So this is only reached when nothing better is present. The unambiguous
/// sources — one path per line — are preferred in [`split_unified_diff`]:
/// `+++ b/<path>` and `--- a/<path>` for a file with content changes, and
/// `rename to` / `rename from` for a pure rename that has no hunks at all.
pub(super) fn diff_git_paths(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix("diff --git ")?.strip_prefix("a/")?;
    // The first candidate rather than the last: `a/<old> b/<new>` puts the real
    // separator before anything the new path could contribute.
    let at = rest.find(" b/")?;
    let (old, new) = (&rest[..at], &rest[at + 3..]);
    if old.is_empty() || new.is_empty() {
        return None;
    }
    Some((old.to_string(), new.to_string()))
}

/// The single path a `--- a/x` or `+++ b/x` line carries, if it is a real one.
///
/// `/dev/null` is git saying the file does not exist on that side — an addition
/// or a deletion — and is not a path.
pub(super) fn diff_side_path(line: &str, marker: &str, prefix: &str) -> Option<String> {
    let rest = line.strip_prefix(marker)?.trim_end();
    if rest == "/dev/null" {
        return None;
    }
    Some(rest.strip_prefix(prefix).unwrap_or(rest).to_string())
}

/// Split a multi-file unified diff into one section per file.
///
/// Pure, and the counting is the part a reviewer trusts without checking: the
/// tallies are the `add` and `del` lines of that file's own hunks, so they cannot
/// disagree with what is drawn under them. Orca takes its counts from git's
/// numstat instead, which is a second source of truth for the same number.
pub(super) fn split_unified_diff(diff: &str) -> Vec<DiffSection> {
    /// What we know about one file while reading it. `path` and `origin` are
    /// refined as the unambiguous lines arrive.
    struct Reading<'a> {
        path: String,
        origin: Option<String>,
        status: &'static str,
        lines: Vec<&'a str>,
    }
    let mut held: Vec<Reading<'_>> = Vec::new();
    for line in diff.lines() {
        if let Some((old, new)) = diff_git_paths(line) {
            held.push(Reading {
                path: new,
                origin: Some(old),
                status: "modified",
                lines: Vec::new(),
            });
        }
        // Anything before the first `diff --git` belongs to no file. git does not
        // emit that, and inventing a section for it would put a header with no
        // path on the screen.
        let Some(reading) = held.last_mut() else {
            continue;
        };
        reading.lines.push(line);
        if line.starts_with("new file mode ") || line == "--- /dev/null" {
            reading.status = "added";
        } else if line.starts_with("deleted file mode ") || line == "+++ /dev/null" {
            reading.status = "deleted";
        } else if line.starts_with("rename from ") || line.starts_with("rename to ") {
            reading.status = "renamed";
        }
        // The unambiguous names, each on a line of its own. A deletion's
        // `+++ /dev/null` leaves the path as the header's guess, which is the
        // only name that file has left.
        if let Some(path) = diff_side_path(line, "+++ ", "b/") {
            reading.path = path;
        } else if let Some(path) = diff_side_path(line, "rename to ", "") {
            reading.path = path;
        }
        if let Some(origin) = diff_side_path(line, "--- ", "a/") {
            reading.origin = Some(origin);
        } else if let Some(origin) = diff_side_path(line, "rename from ", "") {
            reading.origin = Some(origin);
        }
    }
    held.into_iter()
        .map(|reading| {
            let text = reading.lines.join("\n");
            // Per FILE, not per answer: one enormous generated file must not
            // take nineteen reviewable neighbours down with it.
            if let Some(limit) = diff_render_limit(&text) {
                return DiffSection {
                    added: 0,
                    removed: 0,
                    lines: Vec::new(),
                    limit: Some(limit),
                    origin: reading.origin.filter(|origin| *origin != reading.path),
                    path: reading.path,
                    status: reading.status,
                };
            }
            let lines = parse_unified_diff(&text);
            DiffSection {
                added: lines.iter().filter(|line| line.kind == "add").count(),
                removed: lines.iter().filter(|line| line.kind == "del").count(),
                lines,
                limit: None,
                // Only a rename carries an origin worth showing; for every other
                // file the two names are the same word twice.
                origin: reading.origin.filter(|origin| *origin != reading.path),
                path: reading.path,
                status: reading.status,
            }
        })
        .collect()
}

/// Why the committed-change comparison chose its base. The renderer displays
/// this distinction but never recomputes it: worktree and repository pins win
/// before the global preference, exactly as they do in Orca.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum SourceControlCompareBaseSource {
    Worktree,
    Repository,
    BranchUpstream,
    RepositoryDefault,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct SourceControlCompareContext {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) head: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) base_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) source: Option<SourceControlCompareBaseSource>,
    pub(super) options: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) error: Option<String>,
    /// This branch's whole work against the compare base — the header row's
    /// line-total chip (Orca's `GitBranchLineTotal`). Absent when the compare
    /// cannot stand (no base, no merge base): the chip renders nothing, which
    /// is the original's own posture for an unknown total.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) line_total: Option<CompareLineTotal>,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub(super) struct CompareLineTotal {
    pub(super) added: u64,
    pub(super) removed: u64,
}

/// Sum a `--numstat` answer. Binary rows print `-` for both counts — they are
/// skipped, not zeroes: a binary change is real work the numbers cannot say.
pub(super) fn numstat_total(text: &str) -> CompareLineTotal {
    let mut total = CompareLineTotal {
        added: 0,
        removed: 0,
    };
    for line in text.lines() {
        let mut fields = line.split('\t');
        let (Some(added), Some(removed)) = (fields.next(), fields.next()) else {
            continue;
        };
        let (Ok(added), Ok(removed)) = (added.trim().parse::<u64>(), removed.trim().parse::<u64>())
        else {
            continue;
        };
        total.added += added;
        total.removed += removed;
    }
    total
}

#[derive(Serialize)]
pub(super) struct CommittedDiff {
    #[serde(flatten)]
    pub(super) context: SourceControlCompareContext,
    pub(super) sections: Vec<DiffSection>,
}

pub(super) struct ResolvedSourceControlCompare {
    pub(super) context: SourceControlCompareContext,
    pub(super) base_oid: Option<String>,
    pub(super) head_oid: Option<String>,
}

pub(super) fn optional_git_text(host: &Host, root: &Path, args: &[&str]) -> Option<String> {
    host.vcs()
        .text(root, args)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(super) fn current_git_branch(host: &Host, root: &Path) -> Option<String> {
    optional_git_text(host, root, &["symbolic-ref", "--quiet", "--short", "HEAD"])
}

pub(super) fn worktree_compare_base_pin(host: &Host, root: &Path, branch: &str) -> Option<String> {
    optional_git_text(
        host,
        root,
        &[
            "config",
            "--local",
            "--get",
            &format!("branch.{branch}.base"),
        ],
    )
}

/// Resolve an arbitrary stored ref to a commit before it reaches `merge-base`
/// or `diff`. `--end-of-options` matters here: old settings are untrusted file
/// input, and a ref beginning with `-` must never become a git option.
pub(super) fn compare_commit_oid(host: &Host, root: &Path, reference: &str) -> Option<String> {
    optional_git_text(
        host,
        root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &format!("{reference}^{{commit}}"),
        ],
    )
    .filter(|oid| matches!(oid.len(), 40 | 64) && oid.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

pub(super) fn source_control_compare_options(
    host: &Host,
    root: &Path,
    head: Option<&str>,
    selected: Option<&str>,
) -> Vec<String> {
    let mut options = optional_git_text(
        host,
        root,
        &[
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/heads/",
            "refs/remotes/",
        ],
    )
    .map(|listed| {
        listed
            .lines()
            .map(str::trim)
            .filter(|reference| {
                !reference.is_empty() && !reference.ends_with("/HEAD") && Some(*reference) != head
            })
            .map(str::to_string)
            .collect::<Vec<_>>()
    })
    .unwrap_or_default();
    if let Some(selected) = selected.filter(|selected| !selected.is_empty()) {
        options.push(selected.to_string());
    }
    options.sort();
    options.dedup();
    options
}

pub(super) fn resolve_source_control_compare(
    root: &Path,
    repo_root: &Path,
    repository: &settings::SettingsRepository,
    mode: SourceControlCompareBase,
) -> Result<ResolvedSourceControlCompare, String> {
    let host = Host::for_workspace(root);
    let head = current_git_branch(&host, root);
    let worktree_pin = head
        .as_deref()
        .and_then(|branch| worktree_compare_base_pin(&host, root, branch));
    let repository_pin = stored_project_settings_at(repository, &project_settings_key(repo_root))?
        .worktree_base_ref
        .and_then(|reference| {
            let reference = reference.trim().to_string();
            (!reference.is_empty()).then_some(reference)
        });
    let upstream = optional_git_text(&host, root, &["rev-parse", "--abbrev-ref", "@{upstream}"]);
    let repository_default = default_base_probe(&host, repo_root);

    let (base_ref, source) = if let Some(reference) = worktree_pin {
        (
            Some(reference),
            Some(SourceControlCompareBaseSource::Worktree),
        )
    } else if let Some(reference) = repository_pin {
        (
            Some(reference),
            Some(SourceControlCompareBaseSource::Repository),
        )
    } else if mode == SourceControlCompareBase::BranchUpstream {
        if let Some(reference) = upstream {
            (
                Some(reference),
                Some(SourceControlCompareBaseSource::BranchUpstream),
            )
        } else {
            (
                repository_default,
                Some(SourceControlCompareBaseSource::RepositoryDefault),
            )
        }
    } else {
        (
            repository_default,
            Some(SourceControlCompareBaseSource::RepositoryDefault),
        )
    };
    let source = base_ref.as_ref().and(source);
    let base_oid = base_ref
        .as_deref()
        .and_then(|reference| compare_commit_oid(&host, root, reference));
    let head_oid = compare_commit_oid(&host, root, "HEAD");
    let error = match (&base_ref, &base_oid, &head_oid) {
        (None, _, _) => {
            Some("No repository default branch is available for comparison.".to_string())
        }
        (Some(reference), None, _) => Some(format!(
            "Base ref {reference} could not be resolved in this repository."
        )),
        (_, _, None) => Some(
            "This branch does not have a committed HEAD yet, so compare-to-base is unavailable."
                .to_string(),
        ),
        _ => None,
    };
    let options = source_control_compare_options(&host, root, head.as_deref(), base_ref.as_deref());
    // The branch's whole work against the base, summed for the header chip —
    // the same merge-base range the committed diff walks, but `--numstat`
    // only: the row must not pay for every committed line to say two numbers.
    let line_total = match (&base_oid, &head_oid) {
        (Some(base), Some(head_at)) => {
            optional_git_text(&host, root, &["merge-base", base, head_at])
                .and_then(|merge_base| {
                    optional_git_text(
                        &host,
                        root,
                        &["diff", "--numstat", &merge_base, head_at, "--"],
                    )
                })
                .map(|answer| numstat_total(&answer))
        }
        _ => None,
    };
    Ok(ResolvedSourceControlCompare {
        context: SourceControlCompareContext {
            head,
            base_ref,
            source,
            options,
            error,
            line_total,
        },
        base_oid,
        head_oid,
    })
}

pub(super) fn active_source_control_compare(
    state: &AppState,
) -> Result<(PathBuf, ResolvedSourceControlCompare), String> {
    let here = state.active();
    let orchestrator = here.orchestrator.ok_or(NOT_A_REPOSITORY)?;
    let repo_root = orchestrator.repo_root().to_path_buf();
    let mode = load_settings_for_boot(state.settings())
        .document
        .source_control_compare_base;
    let resolved = resolve_source_control_compare(&here.root, &repo_root, state.settings(), mode)?;
    Ok((here.root, resolved))
}

/// Every committed change from the selected base's merge-base to HEAD. This
/// is a second source for the existing combined Changes renderer, not a second
/// renderer and not a mutation of the ordinary worktree-diff semantics.
pub(super) fn committed_diff_from_resolved(
    root: &Path,
    mut resolved: ResolvedSourceControlCompare,
) -> Result<CommittedDiff, String> {
    if resolved.context.error.is_some() {
        return Ok(CommittedDiff {
            context: resolved.context,
            sections: Vec::new(),
        });
    }
    let (base_oid, head_oid) = (
        resolved
            .base_oid
            .as_deref()
            .expect("validated compare base"),
        resolved.head_oid.as_deref().expect("validated HEAD"),
    );
    let host = Host::for_workspace(root);
    let merge_base = match host.vcs().text(root, &["merge-base", base_oid, head_oid]) {
        Ok(value) => value.trim().to_string(),
        Err(_) => {
            let base = resolved.context.base_ref.as_deref().unwrap_or("base");
            resolved.context.error = Some(format!(
                "This branch and {base} do not share a merge base, so compare-to-base is unavailable."
            ));
            return Ok(CommittedDiff {
                context: resolved.context,
                sections: Vec::new(),
            });
        }
    };
    let diff = host
        .vcs()
        .text(
            root,
            &["diff", "--find-renames", &merge_base, head_oid, "--"],
        )
        .map_err(|error| error.to_string())?;
    Ok(CommittedDiff {
        context: resolved.context,
        sections: split_unified_diff(&diff),
    })
}
