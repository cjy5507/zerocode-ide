//! Scm commands.

use crate::*;

#[tauri::command]
pub(crate) async fn scm_status(
    state: State<'_, AppState>,
    uncapped: Option<bool>,
) -> Result<WorkingTree, String> {
    let here = state.active();
    let Some(orchestrator) = here.orchestrator else {
        return Ok(WorkingTree {
            changed: Vec::new(),
            ignored: Vec::new(),
            operation: None,
            capped: false,
            total: 0,
            limit: SCM_STATUS_LIMIT,
        });
    };
    let loss = orchestrator
        .pending_loss(&here.root)
        .map_err(|error| error.to_string())?;
    // Already taken apart by the orchestrator, so nothing here re-reads a
    // path out of a status line. A rename is badged on the name the tree
    // is showing now, which is `path`; its origin no longer exists on
    // disk.
    // The tally is decoration on a row the status already stands for — a
    // numstat that fails must not blank the panel, so its absence is an empty
    // map and every row simply shows no counts.
    let counts = orchestrator
        .numstat(&here.root)
        .map(|text| line_counts_by_path(&text))
        .unwrap_or_default();
    let mut changed: Vec<ScmEntry> = loss
        .uncommitted
        .into_iter()
        .map(|entry| {
            let (staged, changed) = scm_flags(&entry.code);
            let (added, removed) = counts
                .get(&entry.path)
                .copied()
                .map_or((None, None), |(added, removed)| {
                    (Some(added), Some(removed))
                });
            ScmEntry {
                conflict: ConflictKind::from_code(&entry.code).map(ConflictKind::label),
                submodule: entry.submodule.map(|found| ScmSubmodule::of(found, staged)),
                path: entry.path,
                code: entry.code,
                staged,
                changed,
                origin: entry.origin,
                added,
                removed,
            }
        })
        .collect();
    // Asked on every answer, not only when a conflicted row already stands.
    // A merge or rebase BETWEEN steps — every file resolved, nothing yet
    // continued — has no conflicted row and is still the most important thing
    // about this checkout: the panel must say it, and the commit box must get
    // out of the way (Orca's `shouldRenderCommitArea`, `component-gates.ts:5-10`,
    // renders it only when `unresolved === 0 && operation === 'unknown'`).
    // Asking always used to mean a subprocess per refresh; it does not any
    // more — `conflict_operation` reads the git directory rather than asking
    // git where it is.
    //
    // The answer is judged on the FULL list, not the capped one: a conflict
    // past the cap is still a conflict this checkout is holding, and a card
    // that vanished because a thousand untracked files sorted in front of it
    // would be the banner hiding the panel's most important fact.
    let operation = orchestrator
        .conflict_operation(&here.root)
        // A checkout that holds conflicts is in SOME operation even if this
        // call could not say which; `unknown` is a state the prompt and the
        // card both have words for, and losing the whole status over it would
        // be the worse trade.
        .unwrap_or_default();
    let operation = (operation != ConflictOperation::Unknown
        || changed.iter().any(|entry| entry.conflict.is_some()))
    .then(|| operation.id());
    // Orca's cap (`capGitStatusEntries`): the first `limit` rows and a flag,
    // with the retry asking again uncapped. Judged fresh on every answer —
    // the sticky `didHitLimit` in Orca's version exists for its incremental
    // update path, and this side re-reads the whole status every time.
    let total = changed.len();
    let capped = !uncapped.unwrap_or(false) && total > SCM_STATUS_LIMIT;
    if capped {
        changed.truncate(SCM_STATUS_LIMIT);
    }
    Ok(WorkingTree {
        changed,
        ignored: loss.ignored,
        operation,
        capped,
        total,
        limit: SCM_STATUS_LIMIT,
    })
}

/// Put a whole section in the index at once — the section header's `모두
/// 스테이지` and the primary button's Stage All, which name their paths
/// outright the way Orca's `handleStageAllPaths` does (stageable rows only,
/// judged where the list is). One `git add` carries the lot; the orchestrator
/// wraps every path in `:(literal)` exactly as the single-path road does.
#[tauri::command]
pub(crate) async fn stage_paths(
    state: State<'_, AppState>,
    paths: Vec<String>,
) -> Result<(), String> {
    let here = state.active();
    let orchestrator = here.orchestrator.ok_or(NOT_A_REPOSITORY)?;
    orchestrator
        .stage(here.root, &paths)
        .map_err(|error| error.to_string())
}

/// Take a whole section back out of the index — the header's `모두 스테이지
/// 해제`. The working tree is untouched, exactly like the single-path road.
#[tauri::command]
pub(crate) async fn unstage_paths(
    state: State<'_, AppState>,
    paths: Vec<String>,
) -> Result<(), String> {
    let here = state.active();
    let orchestrator = here.orchestrator.ok_or(NOT_A_REPOSITORY)?;
    orchestrator
        .unstage(here.root, &paths)
        .map_err(|error| error.to_string())
}

/// Throw away the working-tree changes on `paths` — the panel's discard
/// wording. The window never calls this without its confirmation dialog;
/// the dialog is what makes it a decision rather than a slip.
#[tauri::command]
pub(crate) async fn discard_paths(
    state: State<'_, AppState>,
    paths: Vec<String>,
) -> Result<(), String> {
    discard_by_git_truth(state, paths)
}

/// Delete untracked `paths` — the same road in the delete wording rather
/// than the discard one: nothing is being restored, something is being
/// removed. Behind the same confirmation dialog.
#[tauri::command]
pub(crate) async fn delete_untracked(
    state: State<'_, AppState>,
    paths: Vec<String>,
) -> Result<(), String> {
    discard_by_git_truth(state, paths)
}

/// Put one path in the index.
///
/// The path is whatever `scm_status` reported, which came out of `-z` and so
/// is a real pathspec — the orchestrator wraps it in `:(literal)` before git
/// sees it.
#[tauri::command]
pub(crate) async fn stage_path(state: State<'_, AppState>, path: String) -> Result<(), String> {
    let here = state.active();
    let orchestrator = here.orchestrator.ok_or(NOT_A_REPOSITORY)?;
    orchestrator
        .stage(here.root, &[path])
        .map_err(|error| error.to_string())
}

/// Take one path back out of the index. The working tree is not touched —
/// this is not a discard, and nothing here has a path that discards.
#[tauri::command]
pub(crate) async fn unstage_path(state: State<'_, AppState>, path: String) -> Result<(), String> {
    let here = state.active();
    let orchestrator = here.orchestrator.ok_or(NOT_A_REPOSITORY)?;
    orchestrator
        .unstage(here.root, &[path])
        .map_err(|error| error.to_string())
}

/// Commit what is staged, answering with the short hash it landed as so the
/// panel can say where the work went instead of just going quiet.
///
/// Async because a commit runs hooks — a `pre-commit` that formats the tree is
/// ordinary, and it is seconds the window must not stop painting for.
/// A commit git itself refused, kept for the card that offers to fix it.
///
/// **Only git's refusals.** Our own two — an empty message, an empty index —
/// are answered by the note under the box and nothing else: they are not
/// failures to investigate, they are the person being told what to do next, and
/// a recovery card offering an agent for them would be a button that cannot
/// help.
#[tauri::command]
pub(crate) async fn commit_staged(
    state: State<'_, AppState>,
    message: String,
) -> Result<String, String> {
    let here = state.active();
    let orchestrator = here.orchestrator.ok_or(NOT_A_REPOSITORY)?;
    let root = here.root;
    let attempted = message.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        match orchestrator.commit(&root, &message) {
            Ok(landed) => Ok(landed),
            // Read in the same blocking hop that failed, because "at failure
            // time" is what the prompt claims of this list. A refused commit
            // leaves the index alone, so this IS the index the commit carried.
            Err(error) => {
                let blocked = match &error {
                    OrchestratorError::Git {
                        command, stderr, ..
                    } if command == "commit" => {
                        Some((stderr.clone(), staged_at_failure(&orchestrator, &root)))
                    }
                    _ => None,
                };
                Err((error.to_string(), blocked))
            }
        }
    })
    .await
    .map_err(|join| join.to_string())?;
    match outcome {
        Ok(landed) => {
            *state.commit_failure() = None;
            Ok(landed)
        }
        Err((said, blocked)) => {
            *state.commit_failure() = blocked.map(|(text, entries)| CommitFailure {
                text,
                message: attempted,
                entries,
            });
            Err(said)
        }
    }
}

/// The card under the commit box after git refused: what happened, and the
/// briefing an agent would need to fix it.
///
/// Built here rather than in the window because every part of it is measured —
/// which word the summary picks, which pill it earns, whether there is more to
/// show, and a prompt whose rules are the difference between an agent that
/// fixes a lint error and one that runs `git reset --hard`.
#[tauri::command]
pub(crate) fn commit_failure_card(state: State<'_, AppState>) -> Option<CommitFailureCard> {
    let _crumb = crate::crumbs::Command::enter("commit_failure_card");
    let held = state.commit_failure();
    let failure = held.as_ref()?;
    let summary = commit_failure::summarize_commit_failure(&failure.text);
    Some(CommitFailureCard {
        summary: summary.clone(),
        kind_label: commit_failure::commit_failure_kind_label(&summary).map(str::to_string),
        has_details: commit_failure::has_expanded_commit_failure_details(&failure.text, &summary),
        detail_text: failure.text.clone(),
        prompt: commit_failure::fix_commit_failure_prompt(
            &commit_failure::CommitFailureContext {
                worktree_path: Some(state.active_root().display().to_string()),
                commit_message: failure.message.clone(),
                error: failure.text.clone(),
                entries: failure.entries.clone(),
            },
            "",
        ),
    })
}

/// Read the branch's standing, synchronously — this is a read like `scm_status`,
/// two `rev-parse`-class calls against local refs and no network.
///
/// A branch with no upstream is not a failure: `rev-parse @{upstream}` refusing
/// IS the answer, and it is the state the button calls "publish".
///
/// **The first caller migrated onto the host boundary.** It asks
/// [`Host::for_workspace`] once, then every git read in this function goes
/// through that host — there is no second place in here that decides where git
/// runs. That is the whole shape phase 0 exists to establish; see
/// [`zerocode_core::host`] for why (Orca sprayed 737 inline connection-id
/// branches instead).
#[tauri::command]
pub(crate) async fn upstream_status(state: State<'_, AppState>) -> Result<UpstreamStatus, String> {
    let here = state.active();
    if here.orchestrator.is_none() {
        return Err(NOT_A_REPOSITORY.to_string());
    }
    let root = here.root;
    let host = Host::for_workspace(&root);
    let Ok(upstream) = host
        .vcs()
        .text(&root, &["rev-parse", "--abbrev-ref", "@{upstream}"])
    else {
        return Ok(UpstreamStatus {
            upstream: None,
            ahead: 0,
            behind: 0,
            behind_commits_are_patch_equivalent: None,
        });
    };
    let upstream = upstream.trim().to_string();
    let counts = host.vcs().text(
        &root,
        &["rev-list", "--left-right", "--count", "@{upstream}...HEAD"],
    )?;
    // `--left-right` counts the left side first, and the left side here is the
    // upstream: commits it has that HEAD does not, which is BEHIND. Swapping
    // these two reads is a button that offers a push when a pull is due.
    let mut split = counts.split_whitespace();
    let behind = split.next().and_then(|n| n.parse().ok()).unwrap_or(0);
    let ahead = split.next().and_then(|n| n.parse().ok()).unwrap_or(0);
    // The second git call, and ONLY when the branch is diverged. Orca guards it
    // the same way (`counts.ahead > 0 && counts.behind > 0`), and the guard is
    // not an optimisation: every other standing already has an unambiguous
    // button, so paying for a `log` walk there would be a subprocess spent to
    // answer a question nobody asked.
    let behind_commits_are_patch_equivalent = (ahead > 0 && behind > 0).then(|| {
        let range = format!("HEAD...{upstream}");
        host.vcs()
            .text(
                &root,
                &[
                    "log",
                    "--oneline",
                    "--cherry-mark",
                    "--right-only",
                    &range,
                    "--",
                ],
            )
            .map(|listing| upstream_only_commits_are_patch_equivalent(&listing))
            .unwrap_or(false)
    });
    Ok(UpstreamStatus {
        upstream: Some(upstream),
        ahead,
        behind,
        behind_commits_are_patch_equivalent,
    })
}

/// Push the branch, publishing it if it has never been pushed.
///
/// Orca's own argv (`gitPush`): `--set-upstream` always, `origin HEAD` when
/// nothing more specific is configured — publishing and pushing are the same
/// button, which is why the caller does not have to know whether an upstream
/// exists yet. `--force-with-lease` goes in BARE, right after `push`: the lease
/// is git's own default (the remote-tracking ref as of the last fetch), and
/// computing an expected OID here would replace the one check that makes this
/// safe with our own guess about it.
///
/// Async because this is the network: a push to a slow remote is seconds the
/// window must not stop painting for. `git_text` is the runner because it is
/// already the one place that checks `!status.success()` and answers with git's
/// own stderr — the refusal a rejected push needs to show is git's sentence,
/// not ours.
#[tauri::command]
pub(crate) async fn scm_push(
    state: State<'_, AppState>,
    force_with_lease: bool,
) -> Result<(), PushFailure> {
    let here = state.active();
    if here.orchestrator.is_none() {
        return Err(PushFailure {
            kind: "other",
            text: NOT_A_REPOSITORY.to_string(),
        });
    }
    let root = here.root;
    tauri::async_runtime::spawn_blocking(move || {
        let mut args = vec!["push"];
        if force_with_lease {
            args.push("--force-with-lease");
        }
        args.extend_from_slice(&["--set-upstream", "origin", "HEAD"]);
        git_text(&root, &args).map(|_| ())
    })
    .await
    .map_err(|join| PushFailure {
        kind: "other",
        text: join.to_string(),
    })?
    .map_err(|stderr| {
        let text = strip_credentials(&stderr);
        PushFailure {
            kind: classify_push_failure(&text),
            text,
        }
    })
}

/// Fetch, without touching the working tree.
///
/// The lease's baseline IS the last fetch (bare `--force-with-lease` compares
/// against the remote-tracking ref), so this is not a convenience — it is what
/// makes a refused force push answerable. The window runs it after a rejection
/// and before the sync path's re-reading, which is exactly where Orca runs it.
#[tauri::command]
pub(crate) async fn scm_fetch(state: State<'_, AppState>) -> Result<(), String> {
    let here = state.active();
    if here.orchestrator.is_none() {
        return Err(NOT_A_REPOSITORY.to_string());
    }
    let root = here.root;
    tauri::async_runtime::spawn_blocking(move || git_text(&root, &["fetch"]).map(|_| ()))
        .await
        .map_err(|join| join.to_string())?
}

/// Pull from the configured upstream.
///
/// Bare `git pull`, which is Orca's `gitPull` when an upstream is configured —
/// the branch's own merge/rebase configuration decides what that means, and
/// this window does not override it. `ff_only` is the menu's Fast-forward:
/// Orca's `gitFastForward` is the same pull carrying `--ff-only` and nothing
/// else, which is why it is a mode here rather than a second command. Async
/// for the same reason as the push, and its refusals — a dirty tree, a
/// conflict, a pull that cannot fast-forward — arrive as git wrote them.
#[tauri::command]
pub(crate) async fn scm_pull(
    state: State<'_, AppState>,
    ff_only: Option<bool>,
) -> Result<(), String> {
    let here = state.active();
    if here.orchestrator.is_none() {
        return Err(NOT_A_REPOSITORY.to_string());
    }
    let root = here.root;
    tauri::async_runtime::spawn_blocking(move || {
        let args: &[&str] = if ff_only.unwrap_or(false) {
            &["pull", "--ff-only"]
        } else {
            &["pull"]
        };
        git_text(&root, args).map(|_| ())
    })
    .await
    .map_err(|join| join.to_string())?
}

/// The amber card above the file list, when the checkout is holding conflicts.
///
/// Orca's `ConflictSummaryCard` (SourceControl-e46DLHZz.js:13800) and the
/// prompt behind its button (`buildResolveConflictsPrompt`, :5596). Read fresh
/// rather than remembered: unlike a refused commit, which is an event, this is
/// a STATE — the person may have resolved half of it in another terminal since
/// the panel last drew, and a card built from a memory would offer to fix files
/// that are already fine.
#[tauri::command]
pub(crate) async fn conflict_card(
    state: State<'_, AppState>,
) -> Result<Option<ConflictCard>, String> {
    let here = state.active();
    let Some(orchestrator) = here.orchestrator else {
        return Ok(None);
    };
    let loss = orchestrator
        .pending_loss(&here.root)
        .map_err(|error| error.to_string())?;
    let entries: Vec<conflict::ConflictEntry> = loss
        .uncommitted
        .into_iter()
        .filter_map(|entry| {
            ConflictKind::from_code(&entry.code).map(|kind| conflict::ConflictEntry {
                path: entry.path,
                kind: Some(kind),
            })
        })
        .collect();
    let operation = orchestrator
        .conflict_operation(&here.root)
        .unwrap_or_default();
    // Nothing unresolved AND nothing in progress is the ordinary state — no
    // card. Either one alone is a card: conflicts standing is the summary, and
    // an operation with everything resolved is the banner Orca draws beside it
    // (`content-status.tsx:65-74`) for the case its own comment names —
    // "between steps, or resolved but pre-continue".
    if entries.is_empty() && operation == ConflictOperation::Unknown {
        return Ok(None);
    }
    Ok(Some(ConflictCard {
        operation: operation.id(),
        count: entries.len(),
        can_abort: operation.can_abort(),
        prompt: conflict::resolve_conflicts_prompt(
            operation,
            &entries,
            Some(&here.root.display().to_string()),
        ),
    }))
}

/// Read one submodule's own changes.
///
/// Asked ONLY when a row is expanded, which is the whole design: a repository
/// with twenty submodules would otherwise spend twenty inner `git status`
/// calls on every refresh of the parent, and nested submodules make that a
/// tree walk (Orca's `useSourceControlSubmoduleStatus`, whose comment says the
/// same). `staged` is the parent row's own area — see
/// [`zerocode_orchestrator::Orchestrator::submodule_status`] for why the two
/// halves read different things.
#[tauri::command]
pub(crate) async fn submodule_status(
    state: State<'_, AppState>,
    path: String,
    staged: Option<bool>,
) -> Result<SubmoduleChanges, String> {
    let here = state.active();
    let orchestrator = here.orchestrator.ok_or(NOT_A_REPOSITORY)?;
    let root = here.root.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let found = orchestrator
            .submodule_status(&root, &path, staged.unwrap_or(false), SCM_STATUS_LIMIT)
            .map_err(|error| error.to_string())?;
        Ok(SubmoduleChanges {
            entries: found
                .entries
                .into_iter()
                .map(|entry| {
                    let (staged, changed) = scm_flags(&entry.code);
                    let (added, removed) = found
                        .tallies
                        .get(&entry.path)
                        .map_or((None, None), |(added, removed)| {
                            (Some(*added), Some(*removed))
                        });
                    ScmEntry {
                        conflict: ConflictKind::from_code(&entry.code).map(ConflictKind::label),
                        submodule: entry.submodule.map(|found| ScmSubmodule::of(found, staged)),
                        path: entry.path,
                        code: entry.code,
                        staged,
                        changed,
                        origin: entry.origin,
                        added,
                        removed,
                    }
                })
                .collect(),
            capped: found.capped,
        })
    })
    .await
    .map_err(|join| join.to_string())?
}

/// Undo the operation in progress.
///
/// The window asks with the operation the card was drawn for, and this refuses
/// if the checkout has moved on to another one — an abort is not undoable, and
/// "abort the merge" pressed against a rebase that started a second ago would
/// throw away work nobody meant to touch.
#[tauri::command]
pub(crate) async fn abort_conflict_operation(
    state: State<'_, AppState>,
    operation: String,
) -> Result<(), String> {
    let here = state.active();
    let orchestrator = here.orchestrator.ok_or(NOT_A_REPOSITORY)?;
    let asked = ConflictOperation::from_id(&operation);
    let now = orchestrator
        .conflict_operation(&here.root)
        .map_err(|error| error.to_string())?;
    if now != asked {
        return Err(format!(
            "이 체크아웃은 더 이상 {}가 진행 중이 아닙니다",
            asked.prompt_label()
        ));
    }
    orchestrator
        .abort_operation(&here.root, asked)
        .map_err(|error| error.to_string())
}

/// Draft a commit message from what is staged, with claude's one-shot mode.
///
/// The prompt and the argv are `zerocode_core::commit_message`'s, measured
/// off Orca (see the module doc); what lives here is the plumbing that has
/// burned this window before, each line for a reason it already paid for:
/// the program comes from the agent catalogue (a machine may have `claude`
/// under another name), the child gets the hydrated PATH (a Dock launch has
/// none), the SELECTED ACCOUNT's env (the drafter must bill the account the
/// picker names), the prompt rides stdin (a staged diff on argv hits
/// command-line limits), and the wait is bounded with a kill (a timeout
/// that only stops waiting has leaked a process).
#[tauri::command]
pub(crate) async fn generate_commit_message(state: State<'_, AppState>) -> Result<String, String> {
    let root = state.active_root();
    let config_root = state.config_root().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || draft_commit_message(&config_root, &root))
        .await
        .map_err(|join| join.to_string())?
}

/// Draft a pull request's fields from what this branch has that its base does
/// not.
///
/// Long — it runs a model — so the command puts it on the blocking pool.
#[tauri::command]
pub(crate) async fn generate_pull_request(
    state: State<'_, AppState>,
    base: String,
    title: String,
    body: String,
    draft: bool,
) -> Result<PullRequestDraft, String> {
    let root = state.active_root();
    let config_root = state.config_root().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        draft_pull_request(&config_root, &root, &base, &title, &body, draft)
    })
    .await
    .map_err(|join| join.to_string())?
}

#[tauri::command]
pub(crate) async fn pull_request_seed(
    state: State<'_, AppState>,
) -> Result<PullRequestSeed, String> {
    let root = state.active_root();
    let host = Host::for_workspace(&root);
    let base = hosted_review_base(&host, &root);
    // The title only when there is a base to measure against; without one
    // there is no "what this branch adds" to read a subject from.
    let title = base
        .as_deref()
        .and_then(|base| {
            let merge_base = git_text(&root, &["merge-base", base, "HEAD"]).ok()?;
            let range = format!("{}..HEAD", merge_base.trim());
            let subjects = git_text(&root, &["log", "--pretty=format:%s", &range]).ok()?;
            subjects
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .map(str::to_string)
        })
        .unwrap_or_default();
    // Infallible in substance — every miss above already degrades to an
    // empty seed — but an async command holding `State` must answer in
    // `Result` (Tauri's own rule), so the one arm is `Ok`.
    Ok(PullRequestSeed {
        base,
        // Asked of git rather than read out of `.git/HEAD`: in a linked
        // worktree that path is a pointer FILE, so the cheap read answers
        // "no branch" for every worktree — and a seed with no branch is a
        // composer that refuses to open.
        branch: current_git_branch(&host, &root),
        title,
    })
}

/// The review door's face, decided before it is pressed.
///
/// Until now the branch guards lived past the click: pressing Create PR asked
/// for a seed and only then said "check out a branch". The original decides
/// first (`getHostedReviewCreationEligibility`) so the door can wear the
/// reason as a tooltip, which is the difference between a control that
/// explains itself and one that argues after the fact.
///
/// `upstream` is the STANDING (`None` when nobody measured it) and
/// `upstream_name` is the tracking ref's full `<remote>/<branch>`. They are
/// two arguments rather than one because a missing name and an unmeasured
/// standing are different answers, and only the second one stops the ladder
/// without naming a blocker.
#[tauri::command]
pub(crate) async fn hosted_review_eligibility(
    state: State<'_, AppState>,
    dirty: bool,
    upstream: Option<bool>,
    upstream_name: Option<String>,
    ahead: u32,
    behind: u32,
    review: bool,
) -> Result<HostedReviewEligibility, String> {
    let root = state.active_root();
    tauri::async_runtime::spawn_blocking(move || {
        let host = Host::for_workspace(&root);
        let remote_name = git_text(&root, &["remote"])
            .ok()
            .and_then(|listed| gh::pick_remote(&listed).ok());
        let remote_url = remote_name
            .as_deref()
            .and_then(|name| git_text(&root, &["remote", "get-url", name]).ok())
            .map(|url| url.trim().to_string());
        let branch = current_git_branch(&host, &root);
        let base = hosted_review_base(&host, &root);
        let authenticated = || hosted_review_authenticated(&root, remote_url.as_deref());
        let manual_url = remote_repo::manual_review_url(&remote_repo::ManualReview {
            base_ref: base.as_deref(),
            branch_name: branch.as_deref(),
            repo_remote_name: remote_name.as_deref(),
            repo_remote_url: remote_url.as_deref(),
            upstream_name: upstream_name.as_deref(),
        });
        hosted_review_ladder(&HostedReviewFacts {
            branch,
            base,
            remote: remote_name.is_some(),
            provider: remote_url
                .as_deref()
                .and_then(remote_repo::parse_remote_repo)
                .and_then(|repo| repo.provider),
            manual_url,
            authenticated: &authenticated,
            review,
            dirty,
            upstream,
            ahead,
            behind,
        })
    })
    .await
    .map_err(|join| join.to_string())
}

#[tauri::command]
pub(crate) async fn create_pull_request(
    state: State<'_, AppState>,
    base: String,
    title: String,
    body: String,
    draft: bool,
) -> Result<String, String> {
    let root = state.active_root();
    if title.trim().is_empty() {
        return Err("제목이 비어 있습니다".to_string());
    }
    if base.trim().is_empty() {
        return Err("기준 브랜치가 비어 있습니다".to_string());
    }
    // `gh` pushes the branch when it has to, so this is minutes-capable work on
    // a blocking thread rather than something to hold the UI thread for.
    let made = tauri::async_runtime::spawn_blocking(move || {
        gh::create_pull_request(&root, base.trim(), title.trim(), &body, draft).map_err(|error| {
            match error {
                gh::GhError::Missing => "GitHub CLI(gh)를 찾지 못했습니다".to_string(),
                gh::GhError::Refused(said) | gh::GhError::Unreadable(said) => said,
            }
        })
    })
    .await
    .map_err(|join| join.to_string())?;
    // Counted on the way out, not on the way in: a refused `gh` is not a pull
    // request, and the figure at the head of the stats pane says how many were
    // OPENED.
    if made.is_ok() {
        stats_events_store::record(|counters| counters.pr_created(epoch_ms_now()));
    }
    made
}

/// Name a branch after the work somebody is about to start.
///
/// The WORK, not a diff — there is no diff yet when a branch is named, which is
/// why Orca's prompt takes the first prompt of the session rather than a patch.
#[tauri::command]
pub(crate) async fn generate_branch_name(
    state: State<'_, AppState>,
    prompt: String,
    prefix: Option<String>,
) -> Result<String, String> {
    let root = state.active_root();
    let config_root = state.config_root().to_path_buf();
    let said = prompt.trim().to_string();
    if said.is_empty() {
        return Err("무엇을 할 작업인지 한 줄 적어주세요".to_string());
    }
    tauri::async_runtime::spawn_blocking(move || {
        draft_branch_name(
            &config_root,
            &root,
            &said,
            prefix.as_deref().unwrap_or_default(),
        )
    })
    .await
    .map_err(|join| join.to_string())?
}

/// The commit history of the active worktree, laid out.
///
/// The git conversation is Orca's, call for call (out/main/index.js:106478-
/// 106760): `rev-parse HEAD` for the revision, `symbolic-ref --quiet --short`
/// for the branch (a detached HEAD is a nameless current ref, not an error),
/// `for-each-ref` for the upstream — verified with a `rev-parse`, because a
/// configured upstream whose remote branch is gone must not draw incoming
/// rows — `merge-base` for where the two histories part, and one `-z` log in
/// the measured format, asked for `limit+1` rows so the answer also says
/// whether there are more. The layout and geometry are all
/// `zerocode_core::git_graph`; the window only draws elements.
#[tauri::command]
pub(crate) async fn git_history(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> Result<GitHistory, String> {
    let root = state.active_root();
    tauri::async_runtime::spawn_blocking(move || read_git_history(&root, limit))
        .await
        .map_err(|join| join.to_string())?
}

/// The files one commit touched — Orca's expand-a-row answer, read the way its
/// main process reads it (`diff-tree --name-status`, out/main/index.js:63438)
/// but under `-z`, because a path may carry the very tabs and newlines the
/// default format delimits with. `-M` so a rename is one row wearing both
/// names rather than a delete and an unrelated add; `--root` so the first
/// commit of a repository answers its files instead of nothing. A merge
/// commit answers no rows — git's own stance for a commit that changed
/// nothing itself — and the window says so in words.
#[tauri::command]
pub(crate) async fn commit_files(
    state: State<'_, AppState>,
    id: String,
) -> Result<CommitSheet, String> {
    let root = state.active_root();
    tauri::async_runtime::spawn_blocking(move || {
        let id = history_hash(&id)?;
        let message = git_text(&root, &["log", "-1", "--format=%B", id])?
            .trim_end()
            .to_string();
        let raw = git_text(
            &root,
            &[
                "diff-tree",
                "-r",
                "-M",
                "-z",
                "--name-status",
                "--root",
                "--no-commit-id",
                id,
            ],
        )?;
        Ok(CommitSheet {
            message,
            entries: zerocode_core::git_graph::parse_commit_files(&raw),
        })
    })
    .await
    .map_err(|join| join.to_string())?
}

/// One file's change in one commit, shaped exactly like [`file_diff`]'s answer
/// so the diff tab it opens is the diff tab this window already has. The two
/// sides are committed blobs, so `texts` never carries a version — there is
/// nothing a save could write to, and the merge view reads that as read-only.
#[tauri::command]
pub(crate) async fn commit_file_diff(
    state: State<'_, AppState>,
    id: String,
    path: String,
    origin: Option<String>,
) -> Result<DiffView, String> {
    let root = state.active_root();
    tauri::async_runtime::spawn_blocking(move || {
        let id = history_hash(&id)?;
        let mut args = vec![
            "diff-tree",
            "-r",
            "-M",
            "-p",
            "--root",
            "--no-commit-id",
            id,
            "--",
            &path,
        ];
        if let Some(from) = origin.as_deref() {
            args.push(from);
        }
        let diff = git_text(&root, &args)?;
        // The one gate, above everything — the same stance as `file_diff`: a
        // withheld diff hands back the reason and nothing else.
        if let Some(limit) = diff_render_limit(&diff) {
            return Ok(DiffView {
                limit: Some(limit),
                lines: Vec::new(),
                texts: None,
            });
        }
        let lines = parse_unified_diff(&diff);
        let changed = lines
            .iter()
            .any(|line| line.kind == "add" || line.kind == "del");
        let texts = changed
            .then(|| commit_documents(&root, id, &path, origin.as_deref()))
            .flatten();
        Ok(DiffView {
            limit: None,
            lines,
            texts,
        })
    })
    .await
    .map_err(|join| join.to_string())?
}

/// The web page one commit lives at, opened in the system browser. The URL is
/// built in Rust (`git_graph::commit_web_url`) from the checkout's `origin`,
/// and leaves through [`open_url`]'s own scheme check by construction — it is
/// `https` or it is a refusal here.
#[tauri::command]
pub(crate) async fn open_commit_remote(
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    let root = state.active_root();
    let url = tauri::async_runtime::spawn_blocking(move || -> Result<String, String> {
        let id = history_hash(&id)?;
        let remote = git_text(&root, &["remote", "get-url", "origin"])
            .map_err(|_| "origin 원격이 없습니다".to_string())?;
        zerocode_core::git_graph::commit_web_url(remote.trim(), id)
            .ok_or_else(|| "원격이 웹 주소가 아닙니다".to_string())
    })
    .await
    .map_err(|join| join.to_string())??;
    open_url(url)
}

/// How one file in the active worktree has moved since the last commit.
///
/// An empty answer is a real one: the file is untracked — or the repository
/// has no commits at all — so nothing holds a version for git to compare it
/// with.
#[tauri::command]
pub(crate) async fn file_diff(
    state: State<'_, AppState>,
    path: String,
) -> Result<DiffView, String> {
    let here = state.active();
    let orchestrator = here.orchestrator.ok_or(NOT_A_REPOSITORY)?;
    let root = here.root;
    // A renamed file is asked about under both of its names. git filters by
    // pathspec before it detects renames, so the destination alone comes back
    // as a file that appeared from nowhere — every line an addition, none of
    // them the change the person is trying to review.
    let mut paths = vec![path.clone()];
    let mut origin = None;
    if let Ok(loss) = orchestrator.pending_loss(&root)
        && let Some(from) = loss
            .uncommitted
            .into_iter()
            .find(|entry| entry.path == path)
            .and_then(|entry| entry.origin)
    {
        paths.push(from.clone());
        origin = Some(from);
    }
    let mut diff = orchestrator
        .diff(&root, &paths)
        .map_err(|error| error.to_string())?;
    // An untracked file gets the empty answer from `diff HEAD` — it exists in
    // no commit. The original still shows it: its untracked lane reads the
    // working file and lays it against nothing, every line an addition. Same
    // here, as a SYNTHESIZED unified diff shaped exactly like the one a
    // staged-new file already gets — the same gates, the same parse and the
    // same rows downstream, so the single view and the combined view cannot
    // learn two spellings of "a new file".
    if diff.trim().is_empty() && !git_tracks(&root, &path) {
        diff = untracked_as_added(&root, &path);
    }
    // The one gate, and it is above everything: a withheld diff hands back the
    // reason and NOTHING else. `texts` is not filled in on this road, so the
    // merge view cannot be handed a file the ceiling refused — the fallback
    // card is the whole answer, exactly as it was.
    if let Some(limit) = diff_render_limit(&diff) {
        return Ok(DiffView {
            limit: Some(limit),
            lines: Vec::new(),
            texts: None,
        });
    }
    let lines = parse_unified_diff(&diff);
    // Only where git found changed lines. A diff that is all metadata — a pure
    // rename, "Binary files differ", or the empty answer an untracked file
    // gets — has no two versions to lay side by side, and the rows already say
    // so in words.
    let changed = lines
        .iter()
        .any(|line| line.kind == "add" || line.kind == "del");
    let texts = changed
        .then(|| diff_documents(&root, &path, origin.as_deref()))
        .flatten();
    Ok(DiffView {
        limit: None,
        lines,
        texts,
    })
}

/// The light-weight branch/base answer painted in Source Control. It is kept
/// separate from the diff so merely opening the panel does not transfer every
/// committed line across IPC.
#[tauri::command]
pub(crate) async fn source_control_compare_context(
    state: State<'_, AppState>,
) -> Result<SourceControlCompareContext, String> {
    Ok(active_source_control_compare(&state)?.1.context)
}

#[tauri::command]
pub(crate) async fn worktree_committed_diff(
    state: State<'_, AppState>,
) -> Result<CommittedDiff, String> {
    let (root, resolved) = active_source_control_compare(&state)?;
    committed_diff_from_resolved(&root, resolved)
}

/// Pin or clear the current branch's compare base. Only refs returned by git's
/// own branch inventory are accepted, so a webview string cannot become a git
/// option or an arbitrary revision expression.
#[tauri::command(async)]
pub(crate) fn set_worktree_compare_base(
    state: State<'_, AppState>,
    reference: Option<String>,
) -> Result<SourceControlCompareContext, String> {
    let here = state.active();
    let orchestrator = here.orchestrator.ok_or(NOT_A_REPOSITORY)?;
    let repo_root = orchestrator.repo_root().to_path_buf();
    let host = Host::for_workspace(&here.root);
    let branch = current_git_branch(&host, &here.root)
        .ok_or_else(|| "A detached HEAD cannot own a worktree compare base.".to_string())?;
    let key = format!("branch.{branch}.base");
    if let Some(reference) = reference {
        let reference = reference.trim();
        let allowed = source_control_compare_options(&host, &here.root, Some(&branch), None);
        if reference.is_empty() || !allowed.iter().any(|candidate| candidate == reference) {
            return Err("Choose a branch ref from this repository.".to_string());
        }
        host.vcs()
            .text(&here.root, &["config", "--local", &key, reference])
            .map_err(|error| error.to_string())?;
    } else if worktree_compare_base_pin(&host, &here.root, &branch).is_some() {
        host.vcs()
            .text(&here.root, &["config", "--local", "--unset-all", &key])
            .map_err(|error| error.to_string())?;
    }
    let mode = load_settings_for_boot(state.settings())
        .document
        .source_control_compare_base;
    Ok(resolve_source_control_compare(&here.root, &repo_root, state.settings(), mode)?.context)
}
