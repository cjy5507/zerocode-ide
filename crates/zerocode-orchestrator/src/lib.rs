//! One task, one worktree.
//!
//! Eight agents editing one checkout would fight over a single index, so every
//! dispatched task gets its own `git worktree`. This crate owns that lifetime:
//! create it, list what exists, and — only after a human said so — remove it.
//!
//! `git` runs as a **process**, for the same reason `zo` does
//! (`docs/architecture.md`): whatever git the user has installed is the git we
//! use, with no version compiled into our binary and no libgit2 to keep in step
//! with it.
//!
//! Three rules are enforced by the API rather than described in a comment:
//!
//! 1. **A task title is never trusted as a name.** Titles are free-form, often
//!    Korean, often pasted prose. [`naming::slugify`] reduces one to an
//!    ASCII-only component legal as both a git ref and a path on macOS and
//!    Windows.
//! 2. **Collisions are ordinary.** Two tasks called "fix the tests" must both get
//!    a worktree, so a taken path *or* a taken branch advances a numeric suffix
//!    ([`Orchestrator::create`]).
//! 3. **Removal destroys work, so it takes a promise.** [`Removal`] has no
//!    default and no silent variant: the caller states whether the user
//!    confirmed a clean removal or chose to discard changes they were shown.

pub mod effect_journal;
pub mod git_publisher;
pub mod handoff;
pub mod ledger_store;
pub mod naming;
pub mod publish;
pub mod runtime_actor;
pub mod test_evidence;
pub mod workflow;
pub mod workflow_store;
pub mod worktree_evidence;

mod authority_lock;
mod bounded_process;

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use zerocode_core::WorktreeTask;
use zerocode_core::conflict::ConflictOperation;

pub use naming::{MAX_SLUG_CHARS, slugify};

/// Executable name, per platform. Resolved on `PATH` like every other tool this
/// product runs; [`Orchestrator::open_with_git`] takes a specific binary for a
/// machine whose git is not on it.
#[cfg(windows)]
pub const GIT_EXECUTABLE: &str = "git.exe";
/// Executable name, per platform.
#[cfg(not(windows))]
pub const GIT_EXECUTABLE: &str = "git";

/// Branch namespace for task worktrees. Keeping them under one prefix is what
/// lets a person tell "a lane made this" from "I made this" in `git branch`.
pub const DEFAULT_BRANCH_PREFIX: &str = "wt";

/// How many suffixed names to try before giving up.
///
/// A hundred, which is Orca's measured loop (`suffix 1..100` in
/// `createLocalWorktree`). It used to be twenty here on the argument that
/// twenty worktrees for one title is a mistake worth reporting — but the
/// failure it produces is "no free name for `fix-tests`", which reads as a
/// bug in this product rather than as a full namespace, and the search costs
/// one `show-ref` per candidate that nothing else was going to do anyway.
const MAX_NAME_ATTEMPTS: usize = 100;

/// Git plumbing variables that would silently redirect every command at another
/// repository. They are set whenever we are spawned from a hook or from a shell
/// in the middle of a rebase, and inheriting them is the kind of bug that
/// reads as "git is broken", not as "the environment was wrong".
const INHERITED_GIT_VARS: [&str; 7] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
    "GIT_NAMESPACE",
    "GIT_PREFIX",
];

#[derive(Debug, thiserror::Error)]
pub enum OrchestratorError {
    #[error("could not run `git`: {0}")]
    GitUnavailable(#[source] io::Error),
    #[error("{path} is not inside a git repository")]
    NotAGitRepository { path: PathBuf },
    #[error("`git {command}` failed ({status}): {stderr}")]
    Git {
        command: String,
        status: String,
        stderr: String,
    },
    #[error("could not prepare the worktree directory {path}: {source}")]
    WorktreeRoot {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error(
        "no free name for `{base}` after {attempts} attempts; remove some worktrees for this task \
         or rename it (last refusal: {last_failure})"
    )]
    NamesExhausted {
        base: String,
        attempts: usize,
        last_failure: String,
    },
    #[error(
        "{path} is on a detached HEAD at {head}, so removing it would leave that commit reachable \
         from nothing; put a branch on it first, or confirm discarding it"
    )]
    DetachedHead { path: PathBuf, head: String },
    #[error("{path} is not a worktree of this repository")]
    UnknownWorktree { path: PathBuf },
    #[error("refusing to remove {path}: it is the repository itself, not a task worktree")]
    RefusesToRemoveMainWorktree { path: PathBuf },
    #[error("refusing to remove {path}: git reports that the worktree is locked")]
    LockedWorktree { path: PathBuf },
    #[error(
        "{path} has {} uncommitted change(s) that exist nowhere else; show them and ask before \
         discarding",
        changes.len()
    )]
    UncommittedChanges { path: PathBuf, changes: Vec<String> },
    #[error("refusing to touch {path}: it resolves outside the worktree")]
    PathEscapesWorktree { path: PathBuf },
    #[error("a commit needs a message")]
    EmptyCommitMessage,
    #[error("nothing is staged, so there is nothing to commit")]
    NothingStaged,
    #[error("git has no wholesale undo for this operation")]
    NothingToAbort,
    #[error("branch `{branch}` already exists; pick another name or reuse that branch")]
    BranchTaken { branch: String },
    #[error("branch `{branch}` does not exist, so there is nothing to check out")]
    NoSuchBranch { branch: String },
}

/// What a create is being asked for beyond a name.
///
/// A struct rather than four positional arguments because three of the four
/// are almost always absent, and because the answers are not independent —
/// `reuse_branch` without `branch` is a request with no subject, and the
/// create refuses it by needing a branch to check out.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NewWorktree<'a> {
    /// Where to cut the new branch from. `None` is `HEAD`.
    pub start_point: Option<&'a str>,
    /// The branch name somebody typed, instead of prefix + slug. Validated by
    /// git before it gets here (`check-ref-format`), because git is the only
    /// thing that knows what its own refs may be called.
    pub branch: Option<&'a str>,
    /// Check `branch` out instead of cutting it. Orca's "Reuse branch".
    pub reuse_branch: bool,
}

/// One entry of `git worktree list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    /// Absolute path, as git reports it.
    pub path: PathBuf,
    /// Commit the worktree is on, absent for a worktree that has no HEAD yet.
    pub head: Option<String>,
    /// Short branch name (`wt/drain-gate`), absent when detached or bare.
    pub branch: Option<String>,
    /// The repository's own checkout. It is listed first by git and can never be
    /// removed, so it is worth distinguishing at the type level.
    pub is_main: bool,
    pub bare: bool,
    pub detached: bool,
    /// Deliberately pinned by someone; removal needs their attention first.
    pub locked: bool,
    /// git believes the directory is gone. [`Orchestrator::prune`] clears these.
    pub prunable: bool,
}

/// One `git status --porcelain` record, taken apart.
///
/// The path is stored exactly as git gave it under `-z`, where nothing is
/// quoted and nothing is escaped, so it can be handed straight back as a
/// pathspec. That round trip is the whole reason this type exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusEntry {
    /// The two status columns exactly as git wrote them — ` M`, `M `, `??`,
    /// `RM`. Kept verbatim because the column a letter sits in *is* the fact:
    /// ` M` is a change in the worktree and `M ` the same change already
    /// staged, and trimming makes those two indistinguishable.
    pub code: String,
    /// Where the file is now.
    pub path: String,
    /// Where a renamed or copied file came from. git emits this as a field of
    /// its own under `-z`, so it is read rather than guessed out of the path —
    /// an ordinary filename is free to contain ` -> `.
    pub origin: Option<String>,
    /// Set when this path is a SUBMODULE, and why it is dirty. [`None`] for
    /// every ordinary file.
    ///
    /// The reason the panel needs it: a changed submodule wears the same ` M`
    /// as a changed file, and the two are not the same thing. The parent
    /// repository can stage a moved commit pointer; it cannot stage file
    /// changes living inside the submodule's own worktree.
    pub submodule: Option<Submodule>,
}

/// Why a submodule is dirty — git's own three answers, in the `sub` field of
/// `status --porcelain=v2` (`S<c><m><u>`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Submodule {
    /// The commit this submodule is checked out at is not the one recorded.
    pub commit_changed: bool,
    /// Tracked files inside the submodule have been modified.
    pub tracked_changes: bool,
    /// The submodule holds untracked files.
    pub untracked_changes: bool,
}

impl StatusEntry {
    /// The record as `git status` would have printed it, for a person to read.
    ///
    /// The path is quoted the way porcelain's line format quotes, because a
    /// filename is allowed to contain a newline: printed raw into a list, one
    /// file becomes two apparent entries, and a confirmation dialog then shows
    /// a loss that is not the one it is about to take. `-z` is what makes the
    /// path usable as a pathspec; this is what makes it safe to *show*.
    #[must_use]
    pub fn display(&self) -> String {
        match &self.origin {
            Some(origin) => format!(
                "{} {} -> {}",
                self.code,
                quote_for_display(origin),
                quote_for_display(&self.path)
            ),
            None => format!("{} {}", self.code, quote_for_display(&self.path)),
        }
    }
}

/// A path as one readable token.
///
/// Left exactly alone when it is ordinary, and wrapped in quotes with C-style
/// escapes when it carries anything that would otherwise let it be read as
/// more than one thing. Display only — the raw path is what goes back to git.
///
/// A space counts, which is also what git's own line format does — measured:
/// `?? "plain space.txt"`. Without it a rename cannot be read back, because
/// `a` → `b -> c` and `a -> b` → `c` both print as `a -> b -> c`.
fn quote_for_display(path: &str) -> String {
    if !path
        .chars()
        .any(|c| c.is_control() || c == '"' || c == '\\' || c == ' ')
    {
        return path.to_string();
    }
    let mut quoted = String::with_capacity(path.len() + 2);
    quoted.push('"');
    for character in path.chars() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\t' => quoted.push_str("\\t"),
            '\r' => quoted.push_str("\\r"),
            control if control.is_control() => {
                quoted.push_str(&format!("\\{:03o}", control as u32));
            }
            ordinary => quoted.push(ordinary),
        }
    }
    quoted.push('"');
    quoted
}

/// Everything that would be lost if a worktree were deleted right now.
///
/// The two lists are separate because git treats them completely differently
/// and only one of them is visible by default.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PendingLoss {
    /// Tracked modifications and untracked files — one status record each, so
    /// the caller can show exactly what git would say.
    pub uncommitted: Vec<StatusEntry>,
    /// Paths the repository was told to ignore: `.env`, `target/`, a local
    /// config file someone wrote by hand.
    ///
    /// These are why this type exists. `git status` does not mention them and
    /// `git worktree remove` does **not** consider them when it decides a
    /// worktree is clean — it deletes them without a word. A cleanup that
    /// consulted only git would therefore throw away the one file in the
    /// worktree that cannot be regenerated.
    pub ignored: Vec<String>,
}

impl PendingLoss {
    /// Nothing at all would be lost.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.uncommitted.is_empty() && self.ignored.is_empty()
    }

    /// A token that stands for exactly this loss and nothing else.
    ///
    /// For the caller that has to answer "is what I showed still what I am
    /// about to delete?". **Rendered text cannot do that job.** It is written
    /// to be read, and a filename is free to contain a newline or an arrow and
    /// so take the shape of a different loss entirely — one file named
    /// `a\n  b` prints exactly like two files `a` and `b`. Quoting makes the
    /// text honest, but a person's safety should not rest on the escaping
    /// rules of a *display* staying perfect forever.
    ///
    /// So this is built from the raw fields, framed by counts and separated by
    /// NUL — the one byte a path cannot contain. Two different losses cannot
    /// share one.
    #[must_use]
    pub fn fingerprint(&self) -> String {
        let mut token = format!("{}\0", self.uncommitted.len());
        for entry in &self.uncommitted {
            token.push_str(&entry.code);
            token.push('\0');
            token.push_str(&entry.path);
            token.push('\0');
            token.push_str(entry.origin.as_deref().unwrap_or_default());
            token.push('\0');
        }
        token.push_str(&self.ignored.len().to_string());
        token.push('\0');
        for path in &self.ignored {
            token.push_str(path);
            token.push('\0');
        }
        token
    }

    /// The uncommitted records as lines for a person to read.
    #[must_use]
    pub fn uncommitted_display(&self) -> Vec<String> {
        self.uncommitted.iter().map(StatusEntry::display).collect()
    }

    /// The ignored paths as lines for a person to read.
    ///
    /// Quoted for the same reason the records above are: one path has to read
    /// as one line, or a list of them cannot be trusted by the eye either.
    #[must_use]
    pub fn ignored_display(&self) -> Vec<String> {
        self.ignored
            .iter()
            .map(|path| quote_for_display(path))
            .collect()
    }
}

/// What the caller promises about a removal.
///
/// There is no `Removal::Anyway` and no default, on purpose. Cleanup deletes a
/// directory a person may have unsaved work in, and the product rule is that a
/// human confirms it every time (PRD F7). Making the promise a parameter means
/// a silent cleanup path cannot be written by accident — it has to be typed out.
///
/// **Neither variant is a substitute for asking.** Both delete
/// [`PendingLoss::ignored`] paths, because git deletes them either way and we
/// cannot tell `target/` from `.env`. Show the user
/// [`Orchestrator::pending_loss`] before calling this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Removal {
    /// Refuse while tracked or untracked changes exist, failing with
    /// [`OrchestratorError::UncommittedChanges`] listing them. Ignored paths do
    /// not block it — a lane that built anything has a `target/`, and refusing
    /// on that would mean no worktree could ever be cleaned up.
    ConfirmedIfClean,
    /// The user was shown [`Orchestrator::pending_loss`] and chose to discard
    /// all of it.
    ConfirmedDiscardingChanges,
}

/// Creates and removes the worktrees that lanes run in.
#[derive(Debug, Clone)]
pub struct Orchestrator {
    git: OsString,
    repo_root: PathBuf,
    git_common_dir: PathBuf,
    worktree_root: PathBuf,
    branch_prefix: String,
}

impl Orchestrator {
    /// Open the repository `path` belongs to, with the `git` that `PATH`
    /// resolves.
    ///
    /// The root is resolved by git rather than by walking for a `.git` entry, so
    /// opening a subdirectory, a symlinked path, or an existing linked worktree
    /// all land on the same repository.
    ///
    /// # Errors
    ///
    /// [`OrchestratorError::GitUnavailable`] if `git` cannot be run at all, or
    /// [`OrchestratorError::NotAGitRepository`] if it runs and reports that
    /// `path` is outside one.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, OrchestratorError> {
        Self::open_with_git(path, GIT_EXECUTABLE)
    }

    /// [`Self::open`], but with a specific `git` binary — for a machine whose
    /// git is not on `PATH`, or a setting that pins one.
    ///
    /// The binary applies from the very first command: the repository is
    /// *resolved* with it, and every command the returned orchestrator runs
    /// uses it. An override that only kicked in after a successful `open` would
    /// be no override at all — opening is itself a git invocation.
    pub fn open_with_git(
        path: impl AsRef<Path>,
        git: impl Into<OsString>,
    ) -> Result<Self, OrchestratorError> {
        let path = path.as_ref();
        let git = git.into();
        // Resolve both roots in one git invocation. The checkout root is used
        // for the orchestrator's ordinary commands; the common directory is
        // the stable identity and config home shared by linked worktrees.
        let output = run(
            &git,
            path,
            [
                "rev-parse",
                "--path-format=absolute",
                "--show-toplevel",
                "--git-common-dir",
            ],
        )?;
        if !output.ok {
            return Err(OrchestratorError::NotAGitRepository {
                path: path.to_path_buf(),
            });
        }
        let mut roots = output
            .stdout
            .split_terminator('\n')
            .map(|line| line.strip_suffix('\r').unwrap_or(line));
        let repo_root = PathBuf::from(roots.next().unwrap_or_default());
        // git always prints this second line for a successful invocation. The
        // fallback preserves the old open behavior for a custom git wrapper
        // that only forwards the first answer; the normal path is still the
        // combined result above.
        let git_common_dir = roots
            .next()
            .map(PathBuf::from)
            .unwrap_or_else(|| repo_root.join(".git"));
        let worktree_root = default_worktree_root(&repo_root);
        Ok(Self {
            git,
            repo_root,
            git_common_dir,
            worktree_root,
            branch_prefix: DEFAULT_BRANCH_PREFIX.to_string(),
        })
    }

    /// Put task worktrees somewhere other than the default.
    #[must_use]
    pub fn with_worktree_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.worktree_root = root.into();
        self
    }

    /// Put task branches at the top level, under no prefix at all.
    ///
    /// One of the three prefix modes Orca offers (`branchPrefix: "none"`), and
    /// the reason [`Self::prefixed_branch`] exists: "no prefix" has to be a
    /// state of this value rather than a `format!` a caller skips, or the
    /// window and the create loop would spell the same branch differently.
    #[must_use]
    pub fn without_branch_prefix(mut self) -> Self {
        self.branch_prefix.clear();
        self
    }

    /// Namespace task branches under something other than `wt`.
    ///
    /// Each `/` separated component is slugified, so a prefix cannot be the
    /// thing that produces an invalid ref.
    #[must_use]
    pub fn with_branch_prefix(mut self, prefix: &str) -> Self {
        let cleaned: Vec<String> = prefix
            .split('/')
            .map(naming::slugify)
            .filter(|part| !part.is_empty())
            .collect();
        if !cleaned.is_empty() {
            self.branch_prefix = cleaned.join("/");
        }
        self
    }

    #[must_use]
    pub fn repo_root(&self) -> &Path {
        &self.repo_root
    }

    #[must_use]
    pub fn worktree_root(&self) -> &Path {
        &self.worktree_root
    }

    #[must_use]
    pub fn branch_prefix(&self) -> &str {
        &self.branch_prefix
    }

    /// Create a worktree for `task`, branched from the repository's `HEAD`.
    ///
    /// The name comes from the task title, and both the path and the branch are
    /// checked before each attempt. Either being taken advances the suffix,
    /// because they collide independently: a finished task leaves its branch
    /// behind after its directory is gone, and a user can create a directory
    /// git knows nothing about.
    ///
    /// Losing the check-then-create race is also handled — another process can
    /// take the name in between — but only when the name really did become
    /// taken. Anything else is returned immediately rather than retried
    /// nineteen more times under a different name.
    ///
    /// # Errors
    ///
    /// [`OrchestratorError::WorktreeRoot`] if the directory worktrees live in
    /// cannot be created, [`OrchestratorError::NamesExhausted`] if every
    /// candidate is taken — it carries git's last refusal, so a real failure
    /// cannot hide behind a naming one — or [`OrchestratorError::Git`] for
    /// anything git refused that was not a taken name.
    pub fn create(&self, task: &WorktreeTask) -> Result<Worktree, OrchestratorError> {
        self.create_from(task, None)
    }

    /// [`Self::create`], cut from a named start point instead of `HEAD`.
    ///
    /// The base is handed to git verbatim and git is the validator: a branch
    /// that does not exist, a bad name, a tag — git refuses with its own words
    /// and the refusal comes back as [`OrchestratorError::Git`]. Parsing or
    /// pre-checking it here would be a second opinion about a fact only git
    /// holds.
    pub fn create_from(
        &self,
        task: &WorktreeTask,
        start_point: Option<&str>,
    ) -> Result<Worktree, OrchestratorError> {
        self.create_with(
            task,
            &NewWorktree {
                start_point,
                ..NewWorktree::default()
            },
        )
    }

    /// [`Self::create_from`] with the three answers the create dialog can give
    /// that `HEAD` alone cannot: a branch name somebody typed, an existing
    /// branch to check out rather than cut, and the start point.
    ///
    /// One function rather than three, because the loop underneath is the
    /// value: a taken path and a taken branch collide independently, and every
    /// caller needs the same suffix search over both.
    ///
    /// # Errors
    ///
    /// [`OrchestratorError::BranchTaken`] when a branch somebody NAMED is
    /// already there — a typed name is not a suggestion, so it is not
    /// suffixed — plus everything [`Self::create`] can fail with.
    pub fn create_with(
        &self,
        task: &WorktreeTask,
        options: &NewWorktree<'_>,
    ) -> Result<Worktree, OrchestratorError> {
        fs::create_dir_all(&self.worktree_root).map_err(|source| {
            OrchestratorError::WorktreeRoot {
                path: self.worktree_root.clone(),
                source,
            }
        })?;

        let base = naming::slugify(&task.task_title);
        // A branch somebody typed is checked ONCE, up front, and refused by
        // name. Advancing a suffix under it would hand them `feature/login-2`
        // after they asked for `feature/login`, which is a different branch
        // than the one they named — Orca refuses the same case with
        // `Branch "X" already exists locally`.
        if let Some(named) = options.branch {
            if !options.reuse_branch && self.branch_exists(named)? {
                return Err(OrchestratorError::BranchTaken {
                    branch: named.to_string(),
                });
            }
            if options.reuse_branch && !self.branch_exists(named)? {
                return Err(OrchestratorError::NoSuchBranch {
                    branch: named.to_string(),
                });
            }
        }
        // Carried so an exhausted search can say why git last refused. Without
        // it, a genuine failure that happened to look like a taken name would
        // surface as a name-shaped error and hide its own cause.
        let mut last_failure = String::from("every candidate name was already taken");

        for attempt in 1..=MAX_NAME_ATTEMPTS {
            let name = naming::candidate(&base, attempt);
            let path = self.worktree_root.join(&name);
            let branch = match options.branch {
                Some(named) => named.to_string(),
                None => self.prefixed_branch(&name),
            };
            // A named branch takes itself out of the suffix search: it was
            // settled above, and asking again here would advance the DIRECTORY
            // suffix for a branch that is deliberately being reused.
            let branch_free = options.branch.is_some() || !self.branch_exists(&branch)?;
            if path.exists() || !branch_free {
                continue;
            }

            let mut args: Vec<OsString> = if options.reuse_branch {
                // Checking out what is already there: no `-b`, no start point.
                // git resolves the branch itself, and a start point would be a
                // second opinion about where a branch that exists already is.
                ["worktree", "add"].iter().map(OsString::from).collect()
            } else {
                // `--no-track` is Orca's own `performAddWorktree`. Without it a
                // base that happens to be a remote-tracking ref makes git set
                // an upstream nobody asked for, and the first `git push` then
                // goes at somebody else's branch.
                ["worktree", "add", "--no-track", "-b", &branch]
                    .iter()
                    .map(OsString::from)
                    .collect()
            };
            args.push(path.clone().into_os_string());
            if options.reuse_branch {
                args.push(OsString::from(&branch));
            } else {
                args.push(OsString::from(options.start_point.unwrap_or("HEAD")));
            }

            let output = self.git(&args)?;
            if output.ok {
                return self.worktree_at(&path);
            }
            if !self.name_is_taken(&path, &branch)? {
                return Err(self.failure("worktree add", &output));
            }
            last_failure = self.failure("worktree add", &output).to_string();
        }
        Err(OrchestratorError::NamesExhausted {
            base,
            attempts: MAX_NAME_ATTEMPTS,
            last_failure,
        })
    }

    /// The branch a name lands on under this repository's prefix — and the
    /// name itself when the prefix has been switched off.
    ///
    /// One function so the create loop and anything that wants to SHOW the
    /// branch before it exists cannot spell it differently. Worker checkout
    /// names stay flat on disk, while [`naming::branch_component_from_checkout`]
    /// groups their readable slug below the task id in the branch.
    #[must_use]
    pub fn prefixed_branch(&self, name: &str) -> String {
        let component = naming::branch_component_from_checkout(name);
        if self.branch_prefix.is_empty() {
            component
        } else {
            format!("{}/{}", self.branch_prefix, component)
        }
    }

    /// Is there a local branch by this name? Asked of git, never inferred from
    /// a message.
    ///
    /// # Errors
    ///
    /// [`OrchestratorError::GitUnavailable`] if git cannot be run.
    pub fn has_branch(&self, branch: &str) -> Result<bool, OrchestratorError> {
        self.branch_exists(branch)
    }

    /// Did `git worktree add` fail because this name is taken?
    ///
    /// Answered by asking git what is true *now*, never by reading its message.
    /// stderr is prose written for a person: it is translated, it is reworded
    /// between releases, and it is not an interface anything should parse.
    ///
    /// Measured against real git, the distinction is clean. A genuine failure —
    /// an unborn `HEAD`, an invalid start point — leaves neither the branch nor
    /// the path behind, so finding either is exactly what "taken" means.
    fn name_is_taken(&self, path: &Path, branch: &str) -> Result<bool, OrchestratorError> {
        Ok(path.exists() || self.branch_exists(branch)?)
    }

    /// Every worktree of this repository, the repository's own checkout first.
    ///
    /// # Errors
    ///
    /// [`OrchestratorError::Git`] if git cannot list them.
    pub fn list(&self) -> Result<Vec<Worktree>, OrchestratorError> {
        let output = self.git(&["worktree", "list", "--porcelain"])?;
        if !output.ok {
            return Err(self.failure("worktree list", &output));
        }
        Ok(parse_worktree_list(&output.stdout))
    }

    /// The creation bases recorded for this repository, read from its shared
    /// config without starting another git process.
    ///
    /// `branch.<name>.base` is written to the repository's shared config when
    /// a worktree is created. The common directory was resolved alongside the
    /// checkout root in [`Self::open`], so the catalog can read this small
    /// file directly after its one `worktree list` invocation.
    #[must_use]
    pub fn creation_bases(&self) -> HashMap<String, String> {
        zerocode_core::git_config::branch_bases(&self.git_common_dir.join("config"))
            .into_iter()
            .collect()
    }

    /// 이 저장소의 모든 체크아웃이 공유하는 하나의 자리 — 사이드바가 같은
    /// 저장소를 두 블록으로 세우지 않게 하는 열쇠(1-g42).
    ///
    /// `--show-toplevel`은 워크트리 자신을 말하지만 `--git-common-dir`은
    /// 모두가 공유하는 그 `.git`을 말한다. 두 답은 [`Self::open`]이 한
    /// 번의 `rev-parse`에서 함께 읽어 둔다. bare 저장소는 그 디렉터리
    /// 자체가 저장소지만, 열쇠는 고유하기만 하면 되므로 부모를 취해도
    /// 충돌하지 않는다.
    ///
    /// # Errors
    ///
    /// The `Result` shape is retained for existing callers; after a successful
    /// [`Self::open`], the combined answer is already available and this method
    /// does not start or fail another process.
    pub fn shared_root(&self) -> Result<PathBuf, OrchestratorError> {
        Ok(self
            .git_common_dir
            .parent()
            .map_or_else(|| self.git_common_dir.clone(), std::path::Path::to_path_buf))
    }

    /// What would be lost if this worktree were deleted right now — the list a
    /// confirmation dialog has to show before it asks.
    ///
    /// Untracked files count: they exist in no commit, so the directory is the
    /// only place they are. Ignored paths count too, and separately, for the
    /// reason on [`PendingLoss::ignored`]. Committed work does *not*, because
    /// [`Self::remove`] leaves the branch behind and those commits stay
    /// reachable.
    ///
    /// One `git status` invocation answers both: `--ignored` adds the `!!`
    /// entries and collapses an ignored directory to a single line, so a built
    /// worktree reports `target/` rather than forty thousand files.
    ///
    /// **`matching`, not `traditional`.** They are documented as different and
    /// on every shape measured they answer identically — but `traditional`
    /// costs twenty times as much. On this repository it was **438 ms against
    /// 21 ms**, for byte-identical output: seven entries, 189 bytes. The
    /// difference is that `traditional` walks INTO each ignored directory to
    /// prove it may collapse it, and then collapses it. `matching` reads the
    /// ignore rules and answers.
    ///
    /// The shape that could have disagreed was checked rather than assumed: a
    /// repository with 4,000 files under a pattern written without a trailing
    /// slash — the case where `matching` might have listed the files instead
    /// of the directory — reports `build/` and `out/` under both.
    ///
    /// This is the single most expensive thing a workspace switch does, and
    /// the file panel and the source-control panel both wait on it.
    pub fn pending_loss(
        &self,
        worktree: impl AsRef<Path>,
    ) -> Result<PendingLoss, OrchestratorError> {
        let output = run(
            &self.git,
            worktree.as_ref(),
            ["status", "--porcelain=v2", "-z", "--ignored=matching"],
        )?;
        if !output.ok {
            return Err(self.failure("status --porcelain=v2 -z --ignored", &output));
        }
        Ok(parse_status(&output.stdout))
    }

    /// How many commits `head` carries that `base` does not — the count that
    /// says whether the work in a checkout has LANDED anywhere.
    ///
    /// [`Self::pending_loss`] answers the other half of the same question and
    /// deliberately stops where this one starts: committed work is not "lost"
    /// by a removal, because [`Self::remove`] leaves the branch behind. That
    /// argument is sound about the OBJECTS and says nothing about the person.
    /// A finished worker's branch that no base has taken yet is work somebody
    /// still has to go and look at, and the directory it is spread out in —
    /// with its build, its index, its reflog — is where they would go. So an
    /// automatic reclaim asks this too, and keeps anything that answers
    /// anything but zero.
    ///
    /// `Ok(None)` is the answer that matters most: one of the two names does
    /// not resolve here, so this repository cannot answer at all — a base
    /// branch somebody has since deleted lands there. It must be read as
    /// doubt, never as zero.
    ///
    /// Both names are resolved to object ids before the range is asked. Not
    /// caution about the refs, which git would resolve itself: it is what
    /// keeps a stored name beginning with `-` out of git's option parser, and
    /// what makes "the base is gone" answerable apart from "the range is
    /// empty".
    ///
    /// # Errors
    ///
    /// [`OrchestratorError::GitUnavailable`] if git cannot be run, or
    /// [`OrchestratorError::Git`] if `rev-list` itself refuses.
    pub fn commits_beyond(
        &self,
        worktree: impl AsRef<Path>,
        base: &str,
        head: &str,
    ) -> Result<Option<usize>, OrchestratorError> {
        let worktree = worktree.as_ref();
        let (Some(base), Some(head)) = (
            self.commit_of(worktree, base)?,
            self.commit_of(worktree, head)?,
        ) else {
            return Ok(None);
        };
        // `--` so the range can never be read as a path, and the ids are ids:
        // whatever the two names were spelled as, git is counting commits.
        let output = run(
            &self.git,
            worktree,
            ["rev-list", "--count", &format!("{base}..{head}"), "--"],
        )?;
        if !output.ok {
            return Err(self.failure("rev-list --count", &output));
        }
        // A count git printed and this could not read is not zero. It is the
        // same "no answer" the unresolved ref gives, and a caller weighing a
        // removal has to keep the checkout on it.
        Ok(output.stdout.trim().parse::<usize>().ok())
    }

    /// The commit one name stands for, or `None` when this repository has no
    /// such name. `--quiet` so an absent ref is an exit code rather than a
    /// sentence on stderr, `--end-of-options` so a stored name cannot become
    /// an option.
    fn commit_of(
        &self,
        worktree: &Path,
        reference: &str,
    ) -> Result<Option<String>, OrchestratorError> {
        let output = run(
            &self.git,
            worktree,
            [
                "rev-parse",
                "--verify",
                "--quiet",
                "--end-of-options",
                &format!("{reference}^{{commit}}"),
            ],
        )?;
        if !output.ok {
            return Ok(None);
        }
        let oid = output.stdout.trim();
        Ok((!oid.is_empty()).then(|| oid.to_string()))
    }

    /// Which git operation this checkout is halfway through.
    ///
    /// Orca reads the git directory (`detectConflictOperation`,
    /// out/main/index.js:62613) and so does this — the state lives in files
    /// there, and no porcelain command reports it in one word.
    ///
    /// The directory is READ, not asked: `zerocode_core::git_dir::of` follows
    /// the same `gitdir:` pointer git would, and a checkout in a linked
    /// worktree — which is most of what this application opens — is exactly
    /// the case it exists for. That matters here because this answer is now
    /// wanted on EVERY status refresh rather than only when a conflicted row
    /// already stands: a rebase between steps has no unresolved file and is
    /// still a thing the panel must say. A subprocess per refresh for that
    /// would have been the reason not to ask. `rev-parse` stays as the
    /// fallback for a path that is not a checkout root.
    ///
    /// The order is Orca's: `MERGE_HEAD`, then a rebase directory, then
    /// `CHERRY_PICK_HEAD`. A checkout in none of them answers `Unknown`, which
    /// is a real answer and not a failure — `git apply -3` and `git am` leave
    /// conflicts too, and naming one of these three for them would put a
    /// command in the prompt that errors.
    pub fn conflict_operation(
        &self,
        worktree: impl AsRef<Path>,
    ) -> Result<ConflictOperation, OrchestratorError> {
        let worktree = worktree.as_ref();
        let git_dir = match zerocode_core::git_dir::of(worktree) {
            Some(read) => read,
            None => {
                let found = run(&self.git, worktree, ["rev-parse", "--absolute-git-dir"])?;
                if !found.ok {
                    return Err(self.failure("rev-parse --absolute-git-dir", &found));
                }
                PathBuf::from(found.stdout.trim())
            }
        };
        if git_dir.join("MERGE_HEAD").exists() {
            return Ok(ConflictOperation::Merge);
        }
        // Both spellings: `rebase-merge` is the interactive/merge backend and
        // `rebase-apply` the am backend, and a rebase stopped at a conflict can
        // be in either.
        if git_dir.join("rebase-merge").exists() || git_dir.join("rebase-apply").exists() {
            return Ok(ConflictOperation::Rebase);
        }
        if git_dir.join("CHERRY_PICK_HEAD").exists() {
            return Ok(ConflictOperation::CherryPick);
        }
        Ok(ConflictOperation::Unknown)
    }

    /// What a dirty submodule is dirty ABOUT — read only when somebody opens
    /// one.
    ///
    /// A port of the original's `getSubmoduleStatus` (`git/status.ts:520-570`).
    /// The parent status never recurses: a repository with twenty submodules
    /// would otherwise pay twenty inner `git status` calls on every refresh,
    /// and nested submodules make that a tree walk. So this is asked once, per
    /// submodule, when a row is expanded.
    ///
    /// Two halves, and which one applies is the parent row's own area:
    ///
    ///   * **Unstaged** — the submodule's own `git status`, plus the files
    ///     changed between the commit the parent records and the one the
    ///     submodule is actually checked out at. Both, because either can be
    ///     empty: a submodule can be dirty inside with its pointer unmoved, or
    ///     have a moved pointer and a spotless worktree, and a row that opened
    ///     to nothing in the second case would be a lie about what changed.
    ///   * **Staged** — ONLY the recorded range, `HEAD` to the index. Scanning
    ///     the submodule's worktree there is work whose answer nobody asked
    ///     for: what is staged in the PARENT is the pointer, and nothing else.
    ///
    /// Range rows win on overlap, which is the original's rule and the right
    /// one: the same path can be both, and the range is what the diff this row
    /// opens will show.
    pub fn submodule_status(
        &self,
        worktree: impl AsRef<Path>,
        submodule: &str,
        staged: bool,
        limit: usize,
    ) -> Result<SubmoduleChanges, OrchestratorError> {
        let worktree = worktree.as_ref();
        let inside = self.submodule_worktree(worktree, submodule)?;

        let mut entries = if staged {
            Vec::new()
        } else {
            let output = run(
                &self.git,
                &inside,
                ["status", "--porcelain=v2", "-z", "--untracked-files=all"],
            )?;
            if !output.ok {
                return Err(self.failure("status --porcelain=v2 -z (submodule)", &output));
            }
            parse_status(&output.stdout).uncommitted
        };
        let mut tallies = if staged {
            HashMap::new()
        } else {
            self.tallies_of(&inside, &["diff", "-z", "--numstat", "-M", "-C", "HEAD"])
        };

        // Where the pointer was, and where it is. Unstaged compares what the
        // index records against the submodule's real HEAD; staged compares the
        // parent's HEAD against its index, because that IS the staged change.
        let from = if staged {
            self.gitlink_in_tree(worktree, "HEAD", submodule)
        } else {
            self.gitlink_in_index(worktree, submodule)
                .or_else(|| self.gitlink_in_tree(worktree, "HEAD", submodule))
        };
        let to = if staged {
            self.gitlink_in_index(worktree, submodule)
        } else {
            run(&self.git, &inside, ["rev-parse", "HEAD"])
                .ok()
                .filter(|output| output.ok)
                .map(|output| output.stdout.trim().to_string())
                .filter(|oid| !oid.is_empty())
        };
        if let (Some(from), Some(to)) = (&from, &to)
            && from != to
        {
            let ranged = self.range_entries(&inside, from, to);
            let seen: std::collections::HashSet<&str> =
                ranged.iter().map(|entry| entry.path.as_str()).collect();
            entries.retain(|entry| !seen.contains(entry.path.as_str()));
            tallies.extend(
                self.tallies_of(&inside, &["diff", "-z", "--numstat", "-M", "-C", from, to]),
            );
            let mut all = ranged;
            all.append(&mut entries);
            entries = all;
        }

        let total = entries.len();
        let capped = total > limit;
        entries.truncate(limit);
        Ok(SubmoduleChanges {
            entries,
            tallies,
            capped,
        })
    }

    /// The submodule's own worktree, refused if the name reaches out of the
    /// parent.
    ///
    /// The original's `resolveSubmoduleWorktreePath` (`git/status.ts:505-514`)
    /// and the same three refusals — empty, absolute, or escaping. The name
    /// comes from a status record, but it arrives here through the webview,
    /// and a path that leaves the worktree would run git somewhere nobody
    /// chose.
    fn submodule_worktree(
        &self,
        worktree: &Path,
        submodule: &str,
    ) -> Result<PathBuf, OrchestratorError> {
        let escaped = || OrchestratorError::PathEscapesWorktree {
            path: PathBuf::from(submodule),
        };
        if submodule.is_empty() || submodule.contains('\0') {
            return Err(escaped());
        }
        let named = Path::new(submodule);
        if named.is_absolute() || !lexical_worktree_child(worktree, named) {
            return Err(escaped());
        }
        Ok(worktree.join(named))
    }

    /// The commit a gitlink points at, in a tree or in the index. Absent
    /// rather than failing: a submodule the parent has never recorded is an
    /// ordinary state, and so is one git cannot read right now.
    fn gitlink_in_tree(&self, worktree: &Path, tree: &str, submodule: &str) -> Option<String> {
        let output = run(&self.git, worktree, ["ls-tree", tree, "--", submodule]).ok()?;
        output
            .ok
            .then(|| gitlink_oid(&output.stdout, "160000 commit "))
            .flatten()
    }

    fn gitlink_in_index(&self, worktree: &Path, submodule: &str) -> Option<String> {
        let output = run(&self.git, worktree, ["ls-files", "-s", "--", submodule]).ok()?;
        output
            .ok
            .then(|| gitlink_oid(&output.stdout, "160000 "))
            .flatten()
    }

    /// The files that changed between two commits of the submodule, as status
    /// records the panel can paint like any other row.
    fn range_entries(&self, inside: &Path, from: &str, to: &str) -> Vec<StatusEntry> {
        let Ok(output) = run(
            &self.git,
            inside,
            ["diff", "--name-status", "-z", "-M", "-C", from, to],
        ) else {
            return Vec::new();
        };
        if !output.ok {
            return Vec::new();
        }
        parse_name_status(&output.stdout)
    }

    /// `added\tremoved\tpath` for one diff, keyed by path.
    fn tallies_of(&self, inside: &Path, args: &[&str]) -> HashMap<String, (u64, u64)> {
        run(&self.git, inside, args.iter().copied())
            .ok()
            .filter(|output| output.ok)
            .map(|output| parse_numstat(&output.stdout))
            .unwrap_or_default()
    }

    /// Undo the operation in progress, wholesale.
    ///
    /// Only the two git offers as `--abort` in the panel, and the caller has to
    /// say which — a function that read the state itself could abort a rebase
    /// somebody started in the second between the card being drawn and the
    /// button being pressed.
    pub fn abort_operation(
        &self,
        worktree: impl AsRef<Path>,
        operation: ConflictOperation,
    ) -> Result<(), OrchestratorError> {
        let worktree = worktree.as_ref();
        let name = match operation {
            ConflictOperation::Merge => "merge",
            ConflictOperation::Rebase => "rebase",
            // Nothing else is offered, and inventing one here would be a
            // command nobody measured running against somebody's checkout.
            _ => return Err(OrchestratorError::NothingToAbort),
        };
        let output = run(&self.git, worktree, [name, "--abort"])?;
        if !output.ok {
            return Err(self.failure(&format!("{name} --abort"), &output));
        }
        Ok(())
    }

    /// The unified diff for one path in `worktree`, measured from `HEAD`.
    ///
    /// From `HEAD` rather than from the index: a file that was staged and then
    /// edited again has two diffs, and a review showing only the unstaged half
    /// hides work the person already decided to keep. What a reviewer is
    /// asking is how far this file has moved since the last commit.
    ///
    /// An untracked file comes back empty rather than as an error — it exists
    /// in no commit, so there is nothing for git to compare it against, and
    /// that is an answer rather than a failure. A repository whose first
    /// commit has not landed is the same answer for the same reason, and it
    /// is checked for rather than discovered: `git diff HEAD` fails outright
    /// there, and reporting "bad revision" for a fresh repository would be
    /// blaming the user for its newness.
    ///
    /// **`paths` is plural because a rename has two names.** git applies the
    /// pathspec filter *before* it detects renames, so asking about the
    /// destination alone answers with the whole file as an addition; naming
    /// both sides is what produces the diff a reviewer is looking for.
    ///
    /// Every path is a pathspec behind `--`, so none can be read as an
    /// option, and git itself refuses one that leaves the worktree. Each also
    /// carries `:(literal)`, because a pathspec is a **glob** by default: a
    /// file honestly named `star*name.txt` would otherwise diff whatever else
    /// that pattern happened to match, and not itself.
    pub fn diff<S: AsRef<OsStr>>(
        &self,
        worktree: impl AsRef<Path>,
        paths: &[S],
    ) -> Result<String, OrchestratorError> {
        let worktree = worktree.as_ref();
        if !self.has_commits(worktree)? {
            return Ok(String::new());
        }
        let mut args: Vec<OsString> = ["diff", "HEAD", "--find-renames", "--"]
            .iter()
            .map(OsString::from)
            .collect();
        args.extend(paths.iter().map(|path| {
            let mut literal = OsString::from(":(literal)");
            literal.push(path.as_ref());
            literal
        }));
        let output = run(&self.git, worktree, &args)?;
        if !output.ok {
            return Err(self.failure("diff HEAD", &output));
        }
        Ok(output.stdout)
    }

    /// Per-file line counts against `HEAD` — `--numstat -z`, verbatim, for the
    /// panel that shows a tally beside each changed row. `-z` because a path
    /// with a tab or a newline in it would otherwise be C-quoted into a name
    /// the join can never match. The empty answer is a repository whose first
    /// commit has not landed, for the reason spelled out on [`Self::diff`].
    pub fn numstat(&self, worktree: impl AsRef<Path>) -> Result<String, OrchestratorError> {
        let worktree = worktree.as_ref();
        if !self.has_commits(worktree)? {
            return Ok(String::new());
        }
        let output = run(
            &self.git,
            worktree,
            ["diff", "HEAD", "--numstat", "--find-renames", "-z"],
        )?;
        if !output.ok {
            return Err(self.failure("diff HEAD --numstat", &output));
        }
        Ok(output.stdout)
    }

    /// Put `paths` in the index — what the source-control panel's stage does.
    ///
    /// `add` rather than `stage`: the two are the same command, and `add` is
    /// the one every git on every machine has answered to for twenty years.
    /// It also stages a *deletion*, which is why the panel can offer one
    /// control for a row whatever happened to it.
    ///
    /// Every path is a pathspec behind `--` carrying `:(literal)`, for the
    /// reason spelled out on [`Self::diff`]: a pathspec is a glob by default,
    /// so a file honestly named `star*name.txt` would otherwise stage
    /// whatever else that pattern matched instead of itself.
    pub fn stage<S: AsRef<OsStr>>(
        &self,
        worktree: impl AsRef<Path>,
        paths: &[S],
    ) -> Result<(), OrchestratorError> {
        self.index_command(worktree.as_ref(), &["add", "--"], paths, "add")
    }

    /// Take `paths` back out of the index, leaving the working tree alone.
    ///
    /// Two commands, because git has two answers and only one of them works
    /// on a repository whose first commit has not landed. `restore --staged`
    /// resets an entry to the version in `HEAD` — and an unborn `HEAD` has no
    /// version to reset to, so it fails outright. `rm --cached` is the answer
    /// there: it drops the entry instead of restoring it.
    ///
    /// Which one to use is decided by asking git what is true, never by
    /// reading its refusal, which is translated.
    pub fn unstage<S: AsRef<OsStr>>(
        &self,
        worktree: impl AsRef<Path>,
        paths: &[S],
    ) -> Result<(), OrchestratorError> {
        let worktree = worktree.as_ref();
        if self.has_commits(worktree)? {
            self.index_command(
                worktree,
                &["restore", "--staged", "--"],
                paths,
                "restore --staged",
            )
        } else {
            self.index_command(
                worktree,
                &["rm", "--cached", "-r", "--"],
                paths,
                "rm --cached",
            )
        }
    }

    /// Throw away the working-tree changes on `paths`, restoring each to the
    /// version in `HEAD`.
    ///
    /// Orca's own argv (`discardChanges`/`bulkDiscardChanges`, out/main/
    /// index.js of 1.4.180): `restore --worktree --source=HEAD -- <paths>`.
    /// `--source=HEAD` is spelled out because a bare `restore` reads from the
    /// INDEX — a file that was staged and then edited again would "discard" to
    /// its staged half instead of to the last commit, which is not what the
    /// dialog promised. Containment is git's own: a pathspec cannot name a
    /// file outside the repository, so there is no path here that reaches the
    /// rest of the disk.
    ///
    /// This is the panel's ONE destructive verb pair with
    /// [`Self::clean_untracked`], and neither is called without the window's
    /// confirmation dialog in front of it.
    pub fn discard_worktree<S: AsRef<OsStr>>(
        &self,
        worktree: impl AsRef<Path>,
        paths: &[S],
    ) -> Result<(), OrchestratorError> {
        self.index_command(
            worktree.as_ref(),
            &["restore", "--worktree", "--source=HEAD", "--"],
            paths,
            "restore --worktree",
        )
    }

    /// Delete untracked `paths` outright — the only discard an untracked file
    /// can have, because there is no committed version to restore it to.
    ///
    /// Orca's argv (`cleanUntrackedPaths`): `clean -ffdx -- <paths>`. The
    /// doubled `-f` is deliberate — a nested git checkout refuses a single
    /// force — `-d` reaches directories, and `-x` reaches paths that are also
    /// ignored. None of the three widens what is deleted: the pathspecs name
    /// exactly the rows that were confirmed, and `clean` cannot touch a
    /// tracked file at all.
    pub fn clean_untracked<S: AsRef<OsStr>>(
        &self,
        worktree: impl AsRef<Path>,
        paths: &[S],
    ) -> Result<(), OrchestratorError> {
        self.index_command(worktree.as_ref(), &["clean", "-ffdx", "--"], paths, "clean")
    }

    /// One index command over a list of literal pathspecs, a hundred at a
    /// time.
    ///
    /// The chunking is Orca's `BULK_CHUNK_SIZE` and it is load-bearing: a
    /// "stage all" over a few thousand generated files put every path on ONE
    /// command line, and the kernel refused the whole exec with `E2BIG` —
    /// the panel's biggest button failing exactly when it was most needed
    /// (the map's P0-18). A failed chunk stops the walk; what landed stays
    /// landed, which is also what Orca's sequential awaits do.
    fn index_command<S: AsRef<OsStr>>(
        &self,
        worktree: &Path,
        leading: &[&str],
        paths: &[S],
        named: &str,
    ) -> Result<(), OrchestratorError> {
        for chunk in paths.chunks(BULK_CHUNK) {
            if chunk.is_empty() {
                continue;
            }
            let mut args: Vec<OsString> = leading.iter().map(OsString::from).collect();
            args.extend(chunk.iter().map(|path| literal_pathspec(path.as_ref())));
            let output = run(&self.git, worktree, &args)?;
            if !output.ok {
                return Err(self.failure(named, &output));
            }
        }
        Ok(())
    }

    /// Throw away whatever `paths` hold, letting GIT say which of them are
    /// tracked — Orca's `bulkDiscardChanges` (status.ts:2242-2295), whole.
    ///
    /// The window's status rows also know, but they are a snapshot: a file
    /// that was committed, ignored, or hand-deleted after the paint would be
    /// classified from stale memory, and the wrong verb runs — `restore` on
    /// an untracked path errors the whole batch, `clean` on a tracked one is
    /// silently skipped. Asking `ls-files` at discard time makes the verb
    /// follow the truth of the moment.
    ///
    /// The untracked half is walked with Orca's suspicion
    /// (`git-discard-path-safety.ts`): every target must resolve inside the
    /// worktree's REAL path — a symlink may be deleted as a leaf, but a
    /// symlinked parent must not redirect the removal outside — and the
    /// check runs again after the tracked restore, because that restore can
    /// itself lay down new links (`beforeRemove` recheck).
    pub fn discard_changes(
        &self,
        worktree: impl AsRef<Path>,
        paths: &[String],
    ) -> Result<(), OrchestratorError> {
        let worktree = worktree.as_ref();
        if paths.is_empty() {
            return Ok(());
        }
        for path in paths {
            if !lexical_worktree_child(worktree, Path::new(path)) {
                return Err(OrchestratorError::PathEscapesWorktree {
                    path: PathBuf::from(path),
                });
            }
        }
        let tracked_specs = self.tracked_paths_of(worktree, paths)?;
        let (tracked, untracked): (Vec<&String>, Vec<&String>) = paths
            .iter()
            .partition(|path| names_tracked_path(path, &tracked_specs));

        for path in &untracked {
            validate_untracked_discard_target(worktree, Path::new(path.as_str()))?;
        }
        self.discard_worktree(worktree, &tracked)?;
        for path in &untracked {
            validate_untracked_discard_target(worktree, Path::new(path.as_str()))?;
        }
        self.clean_untracked(worktree, &untracked)
    }

    /// Everything git tracks under these pathspecs, chunked like every other
    /// bulk road — a tracked directory can hold enough paths to blow a
    /// single command line (`listTrackedPathSpecs`).
    fn tracked_paths_of(
        &self,
        worktree: &Path,
        paths: &[String],
    ) -> Result<Vec<String>, OrchestratorError> {
        let mut tracked = Vec::new();
        for chunk in paths.chunks(BULK_CHUNK) {
            let mut args: Vec<OsString> = vec![
                OsString::from("ls-files"),
                OsString::from("-z"),
                OsString::from("--"),
            ];
            args.extend(chunk.iter().map(|path| literal_pathspec(path.as_ref())));
            let output = run(&self.git, worktree, &args)?;
            if !output.ok {
                return Err(self.failure("ls-files", &output));
            }
            tracked.extend(
                output
                    .stdout
                    .split('\0')
                    .filter(|name| !name.is_empty())
                    .map(str::to_string),
            );
        }
        Ok(tracked)
    }

    /// Commit what is staged, and answer with the short hash it landed as.
    ///
    /// **Only what is staged.** There is no `-a` here on purpose: the panel
    /// shows two groups precisely so a person can decide what goes in this
    /// commit, and quietly sweeping in the other group would make that
    /// decision a lie.
    ///
    /// Both refusals are made here rather than left to git, because git's are
    /// prose written for a terminal — a dialog needs to know *which* thing
    /// was wrong to say anything useful about it.
    ///
    /// # Errors
    ///
    /// [`OrchestratorError::EmptyCommitMessage`] if the message is blank,
    /// [`OrchestratorError::NothingStaged`] if the index holds nothing, and
    /// [`OrchestratorError::Git`] for anything git itself refused — an
    /// unconfigured `user.email` being the one a person actually hits.
    pub fn commit(
        &self,
        worktree: impl AsRef<Path>,
        message: &str,
    ) -> Result<String, OrchestratorError> {
        let worktree = worktree.as_ref();
        if message.trim().is_empty() {
            return Err(OrchestratorError::EmptyCommitMessage);
        }
        // `--quiet` exits 0 when the index matches HEAD — that is, when
        // nothing is staged. Asked before committing so the refusal names the
        // real problem instead of forwarding git's prose about it.
        if run(&self.git, worktree, ["diff", "--cached", "--quiet"])?.ok {
            return Err(OrchestratorError::NothingStaged);
        }
        let output = run(&self.git, worktree, ["commit", "-m", message])?;
        if !output.ok {
            return Err(self.failure("commit", &output));
        }
        let landed = run(&self.git, worktree, ["rev-parse", "--short", "HEAD"])?;
        if !landed.ok {
            return Err(self.failure("rev-parse --short HEAD", &landed));
        }
        Ok(landed.stdout.trim().to_string())
    }

    /// Does this worktree's `HEAD` point at a commit yet?
    ///
    /// An unborn `HEAD` — a repository whose first commit has not landed — is
    /// an ordinary state and answers `false`. Anything *else* that stops
    /// `rev-parse` is a real failure and is reported as one: swallowing it
    /// would turn a damaged repository into a file that merely has no changes,
    /// which is the kind of quiet wrong answer this crate exists to avoid.
    ///
    /// The two are told apart by asking `symbolic-ref`, which still answers on
    /// an unborn HEAD because the branch name exists before its first commit
    /// does — never by reading git's prose, which is translated.
    ///
    /// **The line is drawn where git itself draws it, and no finer.** A
    /// symbolic `HEAD` whose branch ref was deleted out from under it is
    /// indistinguishable from an unborn one through plumbing — `rev-parse`,
    /// `symbolic-ref` and `HEAD@{0}` answer identically in both, measured —
    /// and `git status` shows both as a tree of additions. Both therefore
    /// answer `false` and produce an empty diff, which is the same story the
    /// person is already being told everywhere else in the window.
    fn has_commits(&self, worktree: &Path) -> Result<bool, OrchestratorError> {
        if run(
            &self.git,
            worktree,
            ["rev-parse", "--verify", "--quiet", "HEAD"],
        )?
        .ok
        {
            return Ok(true);
        }
        if run(&self.git, worktree, ["symbolic-ref", "--quiet", "HEAD"])?.ok {
            return Ok(false);
        }
        // Asked again without `--quiet`, so the failure carries git's own
        // stderr rather than the silence the probe requested.
        let output = run(&self.git, worktree, ["rev-parse", "--verify", "HEAD"])?;
        if output.ok {
            return Ok(true);
        }
        Err(self.failure("rev-parse --verify HEAD", &output))
    }

    /// Remove a task worktree, given the caller's promise about confirmation.
    ///
    /// The branch survives. That is deliberate: a lane can finish with commits
    /// that exist nowhere else, and deleting the ref along with the directory
    /// would turn "clean up this worktree" into "throw away the work". A branch
    /// left behind is visible, `git branch -d`-able clutter; a deleted one is
    /// gone.
    ///
    /// That argument is also the limit of [`Removal::ConfirmedIfClean`], which
    /// therefore refuses a detached worktree: with no branch to survive it,
    /// removal makes its commits unreachable.
    pub fn remove(
        &self,
        worktree: impl AsRef<Path>,
        removal: Removal,
    ) -> Result<(), OrchestratorError> {
        let worktree = worktree.as_ref();
        let known = self
            .list()?
            .into_iter()
            .find(|candidate| same_worktree_path(&candidate.path, worktree))
            .ok_or_else(|| OrchestratorError::UnknownWorktree {
                path: worktree.to_path_buf(),
            })?;
        if known.locked {
            return Err(OrchestratorError::LockedWorktree { path: known.path });
        }
        if known.is_main {
            return Err(OrchestratorError::RefusesToRemoveMainWorktree { path: known.path });
        }

        let mut args: Vec<OsString> = ["worktree", "remove"].iter().map(OsString::from).collect();
        match removal {
            Removal::ConfirmedIfClean => {
                // "Clean" rests on one argument: the branch outlives the
                // directory, so committed work stays reachable. A detached HEAD
                // has no branch, and that argument collapses — `git status` is
                // empty, git removes the worktree without complaint, and the
                // commits become unreachable. So this variant refuses, and
                // names the commit the user would need to save.
                if known.branch.is_none() {
                    return Err(OrchestratorError::DetachedHead {
                        head: known
                            .head
                            .unwrap_or_else(|| "an unrecorded commit".to_string()),
                        path: known.path,
                    });
                }
                let changes = self.pending_loss(&known.path)?.uncommitted;
                if !changes.is_empty() {
                    return Err(OrchestratorError::UncommittedChanges {
                        path: known.path,
                        changes: changes.iter().map(StatusEntry::display).collect(),
                    });
                }
            }
            Removal::ConfirmedDiscardingChanges => args.push(OsString::from("--force")),
        }
        args.push(known.path.clone().into_os_string());

        let output = self.git(&args)?;
        if !output.ok {
            return Err(self.failure("worktree remove", &output));
        }
        Ok(())
    }

    /// Drop git's administrative records for worktrees whose directories are
    /// gone — the state left behind when a user deletes one in Finder.
    ///
    /// # Errors
    ///
    /// [`OrchestratorError::Git`] if git refuses to prune.
    pub fn prune(&self) -> Result<(), OrchestratorError> {
        // Git itself skips administrative records marked `locked`; keeping
        // this as the one prune operation preserves that guarantee without
        // reaching into `.git/worktrees` and duplicating Git's rules.
        let output = self.git(&["worktree", "prune"])?;
        if !output.ok {
            return Err(self.failure("worktree prune", &output));
        }
        Ok(())
    }

    fn branch_exists(&self, branch: &str) -> Result<bool, OrchestratorError> {
        let reference = format!("refs/heads/{branch}");
        let output = self.git(&["show-ref", "--verify", "--quiet", &reference])?;
        Ok(output.ok)
    }

    fn worktree_at(&self, path: &Path) -> Result<Worktree, OrchestratorError> {
        self.list()?
            .into_iter()
            .find(|candidate| same_worktree_path(&candidate.path, path))
            .ok_or_else(|| OrchestratorError::UnknownWorktree {
                path: path.to_path_buf(),
            })
    }

    fn git<S: AsRef<OsStr>>(&self, args: &[S]) -> Result<GitOutput, OrchestratorError> {
        run(&self.git, &self.repo_root, args)
    }

    /// One committed file's contents, as bytes.
    ///
    /// Bytes rather than a string because the callers that need this are looking
    /// at things git does not treat as text — an image at `HEAD`, to put beside
    /// the one on disk. Every other path through here converts lossily, which for
    /// a PNG means a file that will not decode.
    ///
    /// A path git does not have at that revision is `None`, not an error: a file
    /// that was just added has no committed version, and that is the answer.
    pub fn show(
        &self,
        worktree: impl AsRef<Path>,
        revision: &str,
        path: &str,
    ) -> Result<Option<Vec<u8>>, OrchestratorError> {
        // `--` so a path that looks like a revision is still read as a path.
        let output = git_command(&self.git, worktree.as_ref())
            .args(["show", &format!("{revision}:{path}"), "--"])
            .output()
            .map_err(OrchestratorError::GitUnavailable)?;
        if !output.status.success() {
            return Ok(None);
        }
        Ok(Some(output.stdout))
    }

    fn failure(&self, command: &str, output: &GitOutput) -> OrchestratorError {
        // BOTH pipes, stderr first.
        //
        // git states its own complaints on stderr, so that half leads. But the
        // commit that fails most often is the one a pre-commit hook blocked,
        // and a hook writes its diagnostics to STDOUT — husky, lint-staged,
        // lefthook and every linter underneath them do. Read from stderr alone,
        // the person whose commit was just refused is told "no output", which
        // is the least useful true sentence available: the reason is sitting in
        // the other pipe, unread.
        let said = [output.stderr.trim(), output.stdout.trim()]
            .into_iter()
            .filter(|one| !one.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        OrchestratorError::Git {
            command: command.to_string(),
            status: output.status.clone(),
            stderr: if said.is_empty() {
                "no output".to_string()
            } else {
                said
            },
        }
    }
}

struct GitOutput {
    ok: bool,
    status: String,
    stdout: String,
    stderr: String,
}

/// Orca's `BULK_CHUNK_SIZE`: how many pathspecs ride one git invocation.
const BULK_CHUNK: usize = 100;

/// `:(literal)path` — a filename with a `*` in it is a filename, not a glob.
fn literal_pathspec(path: &OsStr) -> OsString {
    let mut literal = OsString::from(":(literal)");
    literal.push(path);
    literal
}

/// Is `path`, resolved lexically against the worktree, a strict child of it?
/// The refusals are Orca's `isWithinWorktree` list: empty, the worktree
/// itself, anything that climbs out, anything absolute that lands elsewhere.
fn lexical_worktree_child(worktree: &Path, path: &Path) -> bool {
    let target = if path.is_absolute() {
        path.to_path_buf()
    } else {
        worktree.join(path)
    };
    let mut resolved = PathBuf::new();
    for part in target.components() {
        match part {
            std::path::Component::ParentDir => {
                if !resolved.pop() {
                    return false;
                }
            }
            std::path::Component::CurDir => {}
            other => resolved.push(other),
        }
    }
    resolved.starts_with(worktree) && resolved != worktree
}

/// Does `path` name (or sit above) something `ls-files` listed? Orca's
/// `isTrackedPathSpec`: exact match, or the tracked path lives under the
/// asked directory — separators normalised, trailing slashes shed.
fn names_tracked_path(path: &str, tracked: &[String]) -> bool {
    let wanted = normalized_git_path(path);
    tracked.iter().any(|entry| {
        let entry = normalized_git_path(entry);
        entry == wanted || entry.starts_with(&format!("{wanted}/"))
    })
}

fn normalized_git_path(path: &str) -> String {
    path.replace('\\', "/").trim_end_matches('/').to_string()
}

/// The suspicion an untracked removal walks under
/// (`validateUntrackedDiscardTarget`): the target must be a strict child of
/// the worktree lexically AND its REAL path must stay inside the worktree's
/// real path. A symlink is deletable as a leaf — its PARENT directory is
/// what must not point elsewhere — and a path that does not exist yet is
/// judged by its nearest existing parent, so a `clean` cannot be steered
/// through a directory that materialises as a link.
fn validate_untracked_discard_target(
    worktree: &Path,
    path: &Path,
) -> Result<(), OrchestratorError> {
    let escaped = || OrchestratorError::PathEscapesWorktree {
        path: path.to_path_buf(),
    };
    if !lexical_worktree_child(worktree, path) {
        return Err(escaped());
    }
    let real_worktree = worktree.canonicalize().map_err(|_| escaped())?;
    let target = if path.is_absolute() {
        path.to_path_buf()
    } else {
        worktree.join(path)
    };
    let judged = match std::fs::symlink_metadata(&target) {
        Ok(meta) if meta.file_type().is_symlink() => {
            target.parent().map(Path::to_path_buf).ok_or_else(escaped)?
        }
        Ok(_) => target.clone(),
        Err(_) => {
            // Nothing there: climb to the nearest parent that exists and
            // judge that — the directory the removal would walk through.
            let mut parent = target.parent().map(Path::to_path_buf);
            loop {
                match parent {
                    Some(held) => {
                        if held.exists() {
                            break held;
                        }
                        parent = held.parent().map(Path::to_path_buf);
                    }
                    None => return Err(escaped()),
                }
            }
        }
    };
    let real = judged.canonicalize().map_err(|_| escaped())?;
    if real == real_worktree || real.starts_with(&real_worktree) {
        Ok(())
    } else {
        Err(escaped())
    }
}

fn git_command(git: &OsStr, cwd: &Path) -> Command {
    let mut command = Command::new(git);
    // Paths read out of one git command are handed back to the next one as
    // pathspecs, so they have to survive the round trip. Left at its default,
    // `core.quotePath` escapes every non-ASCII byte — `한글.txt` comes back as
    // `"\355\225\234\352\270\200.txt"`, which matches no file on disk. For a
    // product whose users name files in Korean that is not an edge case.
    command.arg("-c").arg("core.quotePath=false");
    command.arg("-C").arg(cwd);
    for name in INHERITED_GIT_VARS {
        command.env_remove(name);
    }
    // git's *messages* are translated, and this crate reads them to tell a name
    // collision from a real failure. `LANGUAGE` is checked ahead of `LC_ALL` by
    // gettext, so pinning one without clearing the other still yields Korean on
    // a Korean desktop.
    command.env_remove("LANGUAGE");
    command.env("LC_ALL", "C");
    command
}

fn run<S: AsRef<OsStr>>(
    git: &OsStr,
    cwd: &Path,
    args: impl IntoIterator<Item = S>,
) -> Result<GitOutput, OrchestratorError> {
    let output = git_command(git, cwd)
        .args(args)
        .output()
        .map_err(OrchestratorError::GitUnavailable)?;
    Ok(GitOutput {
        ok: output.status.success(),
        status: output.status.to_string(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

/// Parse `git status --porcelain=v2 -z`.
///
/// `-z` is what makes these paths usable again. The line-based format quotes
/// and escapes: a path carrying a space comes back wrapped in quotation marks,
/// a tab or a newline comes back as `\t` or `\n`, and `core.quotePath` governs
/// only the non-ASCII half of that. A quoted spelling matches no file when it
/// is handed back as a pathspec, which is the whole job these strings have.
/// Under `-z` nothing is quoted, records end at a NUL, and a rename spends a
/// **second field** on where the file came from — so the origin is read rather
/// than cut out of the path on ` -> `, which an ordinary filename is free to
/// contain.
///
/// **Why version two.** The short format has no word for a submodule: a moved
/// commit pointer and a file edited inside the submodule's own worktree both
/// arrive as ` M`, indistinguishable from an ordinary modified file — and they
/// are not the same thing, because the parent repository can stage the first
/// and cannot stage the second. Version two carries a field for it
/// (`S<c><m><u>`) and nothing else does.
///
/// The record shapes, all NUL-terminated:
///
/// ```text
/// 1 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <path>
/// 2 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <X><score> <path>\0<origin>
/// u <XY> <sub> <m1> <m2> <m3> <mW> <h1> <h2> <h3> <path>
/// ? <path>
/// ! <path>
/// ```
///
/// The two status columns are translated back to the short format's spelling
/// on the way out: version two writes `.` where version one writes a space,
/// and every reader downstream — the conflict codes, the staged/changed split,
/// the rename test — is written against ` M` and `M `. Translating here keeps
/// that one fact in one place rather than teaching a dozen callers a second
/// alphabet.
///
/// A path that is not valid UTF-8 is still lossily converted upstream in
/// [`run`]; `-z` removes the quoting problem, not that one.
fn parse_status(stdout: &str) -> PendingLoss {
    let mut loss = PendingLoss::default();
    let mut fields = stdout.split('\0');

    while let Some(record) = fields.next() {
        // The stream ends with a NUL, so the final split is empty.
        let mut record = record.chars();
        let (Some(kind), Some(' ')) = (record.next(), record.next()) else {
            continue;
        };
        let rest = record.as_str();
        // Untracked and ignored spend no fields on anything: the path is the
        // whole record. An untracked directory arrives as `dir/`; the trailing
        // slash is dropped so the path compares against the tree's own rows.
        // The rest of the name is left exactly alone — a leading or trailing
        // space is part of it.
        if kind == '?' || kind == '!' {
            let path = rest.trim_end_matches('/').to_string();
            if kind == '!' {
                loss.ignored.push(path);
            } else {
                loss.uncommitted.push(StatusEntry {
                    code: "??".to_string(),
                    path,
                    origin: None,
                    submodule: None,
                });
            }
            continue;
        }
        // Changed, renamed and unmerged differ only in how many fields stand
        // between the path and the front of the record.
        let before_path = match kind {
            '1' => 7,
            '2' => 8,
            'u' => 9,
            _ => continue,
        };
        let mut parts = rest.splitn(before_path + 1, ' ');
        let (Some(xy), Some(sub)) = (parts.next(), parts.next()) else {
            continue;
        };
        let Some(path) = parts.nth(before_path - 2) else {
            continue;
        };
        let code = short_code(xy);
        // A rename or copy spends a second NUL field on where the file came
        // from. Reading it here is not optional: left in the stream it parses
        // as a record of its own and shifts every path after it.
        let origin = (kind == '2')
            .then(|| fields.next().map(str::to_string))
            .flatten();
        loss.uncommitted.push(StatusEntry {
            submodule: parse_submodule(sub, &code),
            code,
            path: path.trim_end_matches('/').to_string(),
            origin,
        });
    }
    loss
}

/// What one expanded submodule holds.
#[derive(Debug, Default, Clone)]
pub struct SubmoduleChanges {
    /// Inner status records, their paths RELATIVE TO THE SUBMODULE — the
    /// caller prefixes them, because the caller is the one that knows which
    /// row they hang under (the original says the same, `status.ts:517-518`).
    pub entries: Vec<StatusEntry>,
    /// `added, removed` per inner path, where a diff had numbers for it.
    pub tallies: HashMap<String, (u64, u64)>,
    /// More changes existed than were returned.
    pub capped: bool,
}

/// The commit id out of an `ls-tree`/`ls-files` line for a gitlink.
///
/// Both spellings begin `160000` and differ in what follows, so the marker is
/// the caller's. Absent when this path is not a gitlink at all — which is the
/// answer for a directory somebody named that git does not consider a
/// submodule.
fn gitlink_oid(stdout: &str, marker: &str) -> Option<String> {
    let rest = stdout.lines().find_map(|line| line.strip_prefix(marker))?;
    let oid: String = rest.chars().take_while(char::is_ascii_hexdigit).collect();
    (!oid.is_empty()).then_some(oid)
}

/// Parse `git diff --name-status -z -M -C <from> <to>`.
///
/// Under `-z` the status letter is a field of its own and so is each path, so
/// a rename spends three fields: `R100`, the old path, the new one. The letter
/// lands in the STAGED column, because a range between two commits is history
/// rather than something in this worktree waiting to be staged — the same
/// column `git diff --cached` would have used.
fn parse_name_status(stdout: &str) -> Vec<StatusEntry> {
    let mut entries = Vec::new();
    let mut fields = stdout.split('\0');
    while let Some(mark) = fields.next() {
        let Some(letter) = mark.chars().next() else {
            continue;
        };
        let renamed = letter == 'R' || letter == 'C';
        let Some(first) = fields.next() else {
            break;
        };
        let (path, origin) = if renamed {
            let Some(second) = fields.next() else {
                break;
            };
            (second.to_string(), Some(first.to_string()))
        } else {
            (first.to_string(), None)
        };
        entries.push(StatusEntry {
            code: format!("{letter} "),
            path,
            origin,
            submodule: None,
        });
    }
    entries
}

/// Parse `git diff -z --numstat`, keyed by the path the panel shows.
///
/// A rename carries an EMPTY third field and rides its two real paths in the
/// next two NUL fields (old, then new); a binary file counts as `-` and gets
/// no tally, but still owns its path fields or every entry after it shifts.
fn parse_numstat(stdout: &str) -> HashMap<String, (u64, u64)> {
    let mut held = HashMap::new();
    let mut fields = stdout.split('\0');
    while let Some(head) = fields.next() {
        if head.is_empty() {
            continue;
        }
        let mut columns = head.splitn(3, '\t');
        let (Some(added), Some(removed), Some(path)) =
            (columns.next(), columns.next(), columns.next())
        else {
            continue;
        };
        let path = if path.is_empty() {
            let (_old, new) = (fields.next(), fields.next());
            match new {
                Some(new) => new.to_string(),
                None => break,
            }
        } else {
            path.to_string()
        };
        if let (Ok(added), Ok(removed)) = (added.parse::<u64>(), removed.parse::<u64>()) {
            held.insert(path, (added, removed));
        }
    }
    held
}

/// Version two's `XY` in version one's spelling — `.` is version two's way of
/// writing "nothing here", and every reader downstream expects a space.
fn short_code(xy: &str) -> String {
    let mut code: String = xy
        .chars()
        .take(2)
        .map(|one| if one == '.' { ' ' } else { one })
        .collect();
    while code.len() < 2 {
        code.push(' ');
    }
    code
}

/// The `sub` field: `N...` for an ordinary path, `S<c><m><u>` for a submodule.
///
/// The second clause of `commit_changed` is the original's
/// (`git-status-porcelain-parser.ts:225`) and it covers the staged case: the
/// `sub` field describes the submodule's WORKTREE, so a commit pointer already
/// staged reads `S...` while `XY` says modified. Without that clause a staged
/// submodule bump would look like a submodule with nothing wrong.
fn parse_submodule(sub: &str, code: &str) -> Option<Submodule> {
    let mut letters = sub.chars();
    if letters.next() != Some('S') {
        return None;
    }
    let (commit, tracked, untracked) = (letters.next(), letters.next(), letters.next());
    Some(Submodule {
        commit_changed: commit == Some('C') || (sub == "S..." && code.contains('M')),
        tracked_changes: tracked == Some('M'),
        untracked_changes: untracked == Some('U'),
    })
}

/// Parse `git worktree list --porcelain`.
///
/// Records are separated by a blank line and the repository's own checkout is
/// always first. Attribute lines (`bare`, `detached`) have no value, and
/// `locked`/`prunable` may carry an optional reason. Embedded spaces survive,
/// because only the key delimiter is split off.
///
/// **A path containing a newline parses short.** git prints it verbatim, so the
/// remainder looks like a new record. We never create such a path — a slug
/// cannot contain one — so this can only affect a linked worktree someone else
/// made, and it fails safe: the truncated path matches nothing, so
/// [`Orchestrator::remove`] reports [`OrchestratorError::UnknownWorktree`]
/// rather than deleting the wrong directory. Switching to `-z` would mean
/// parsing bytes and building `OsString`s throughout, which is not worth it
/// until someone actually has one.
fn parse_worktree_list(output: &str) -> Vec<Worktree> {
    let mut worktrees: Vec<Worktree> = Vec::new();
    let mut current: Option<Worktree> = None;

    for line in output.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            if let Some(worktree) = current.take() {
                worktrees.push(worktree);
            }
            continue;
        }
        let (key, value) = line.split_once(' ').unwrap_or((line, ""));
        if key == "worktree" {
            if let Some(worktree) = current.take() {
                worktrees.push(worktree);
            }
            current = Some(Worktree {
                path: PathBuf::from(value),
                head: None,
                branch: None,
                is_main: worktrees.is_empty(),
                bare: false,
                detached: false,
                locked: false,
                prunable: false,
            });
            continue;
        }
        let Some(worktree) = current.as_mut() else {
            continue;
        };
        match key {
            "HEAD" => worktree.head = Some(value.to_string()),
            "branch" => {
                worktree.branch = Some(
                    value
                        .strip_prefix("refs/heads/")
                        .unwrap_or(value)
                        .to_string(),
                );
            }
            "bare" => worktree.bare = true,
            "detached" => worktree.detached = true,
            "locked" => worktree.locked = true,
            "prunable" => worktree.prunable = true,
            _ => {}
        }
    }
    if let Some(worktree) = current.take() {
        worktrees.push(worktree);
    }
    worktrees
}

/// Compare worktree paths the way the filesystem does.
///
/// git reports the resolved path, so on macOS a worktree we created under
/// `/var/folders/…` comes back as `/private/var/folders/…`. Comparing the
/// strings would make every removal fail with "not a worktree".
#[must_use]
pub fn same_worktree_path(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

/// Where task worktrees live when the caller does not say.
///
/// Outside the repository, deliberately: a worktree inside it would show up in
/// the project's own file tree, in `git status` as an untracked directory, and
/// in every `rg`/`cargo` walk the user runs. The directory name carries a hash
/// of the repository path so two projects that are both called `api` do not
/// share one root.
fn default_worktree_root(repo_root: &Path) -> PathBuf {
    let name = repo_root.file_name().map_or_else(
        || "repo".to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    let slug = naming::slugify(&name);
    home_dir()
        .join(".zerocode")
        .join("worktrees")
        .join(format!("{slug}-{}", short_hash(repo_root)))
}

fn home_dir() -> PathBuf {
    let key = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(key)
        .filter(|value| !value.is_empty())
        .map_or_else(std::env::temp_dir, PathBuf::from)
}

/// FNV-1a over the path bytes.
///
/// Stable across runs and machines, unlike the hasher behind `HashMap` — a
/// worktree root that moved every time the IDE restarted would strand the
/// previous run's worktrees.
fn short_hash(path: &Path) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in path.as_os_str().as_encoded_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:08x}", hash as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_porcelain_listing_is_parsed_record_by_record() {
        let listing = "\
worktree /home/dev/project
HEAD 1111111111111111111111111111111111111111
branch refs/heads/main

worktree /home/dev/.zerocode/worktrees/project-abcd1234/drain-gate
HEAD 2222222222222222222222222222222222222222
branch refs/heads/wt/drain-gate

worktree /home/dev/detached
HEAD 3333333333333333333333333333333333333333
detached
locked reason goes here
prunable gitdir file points to non-existent location
";
        let worktrees = parse_worktree_list(listing);
        assert_eq!(worktrees.len(), 3);

        assert!(worktrees[0].is_main);
        assert_eq!(worktrees[0].branch.as_deref(), Some("main"));

        assert!(!worktrees[1].is_main);
        assert_eq!(worktrees[1].branch.as_deref(), Some("wt/drain-gate"));
        assert_eq!(
            worktrees[1].path,
            PathBuf::from("/home/dev/.zerocode/worktrees/project-abcd1234/drain-gate")
        );

        let detached = &worktrees[2];
        assert!(detached.detached && detached.locked && detached.prunable);
        assert_eq!(detached.branch, None);
    }

    /// The last record has no trailing blank line in some git versions, and a
    /// bare main repository has no `HEAD` line at all.
    #[test]
    fn a_bare_repository_and_a_missing_trailing_blank_line_still_parse() {
        let worktrees = parse_worktree_list("worktree /srv/repo.git\nbare");
        assert_eq!(worktrees.len(), 1);
        assert!(worktrees[0].bare && worktrees[0].is_main);
        assert_eq!(worktrees[0].head, None);
    }

    /// Inheriting `GIT_DIR` from a hook or a mid-rebase shell would point every
    /// command at a different repository.
    #[test]
    fn git_plumbing_variables_are_cleared_from_the_child() {
        let command = git_command(OsStr::new("git"), Path::new("/tmp"));
        let removed: Vec<&OsStr> = command
            .get_envs()
            .filter(|(_, value)| value.is_none())
            .map(|(key, _)| key)
            .collect();
        for name in INHERITED_GIT_VARS {
            assert!(removed.contains(&OsStr::new(name)), "{name} was inherited");
        }
        assert!(
            removed.contains(&OsStr::new("LANGUAGE")),
            "LANGUAGE overrides LC_ALL for gettext, so it has to go too"
        );
        assert!(
            command
                .get_envs()
                .any(|(key, value)| key == "LC_ALL" && value == Some(OsStr::new("C")))
        );
    }

    /// 같은 저장소의 모든 체크아웃은 같은 `shared_root`를 말해야 한다 — 다른
    /// 답은 사이드바가 같은 저장소를 두 블록으로 세우는 버그다(1-g42).
    #[test]
    fn every_checkout_of_a_repository_shares_one_root() {
        let dir = empty_repository();
        std::fs::write(dir.path().join("a.txt"), "a").expect("write");
        stage_everything(dir.path());
        commit_everything(dir.path());
        let leaf = tempfile::tempdir().expect("leaf home");
        let leaf_path = leaf.path().join("leaf");
        let status = Command::new(GIT_EXECUTABLE)
            .arg("-C")
            .arg(dir.path())
            .args(["worktree", "add", "-q", "--detach"])
            .arg(&leaf_path)
            .status()
            .expect("git worktree add");
        assert!(status.success(), "git worktree add failed");

        let of_repo = Orchestrator::open(dir.path())
            .expect("open repo")
            .shared_root()
            .expect("shared root of repo");
        let of_leaf = Orchestrator::open(&leaf_path)
            .expect("open leaf")
            .shared_root()
            .expect("shared root of leaf");
        assert_eq!(
            of_repo, of_leaf,
            "a worktree must answer with its repository"
        );

        let other = empty_repository();
        let of_other = Orchestrator::open(other.path())
            .expect("open other")
            .shared_root()
            .expect("shared root of other");
        assert_ne!(
            of_repo, of_other,
            "distinct repositories must stay distinct"
        );
    }

    /// A real repository, no commits in it. Cheap enough to make per test and
    /// the only way to prove what git actually does with these two cases.
    fn empty_repository() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let status = Command::new(GIT_EXECUTABLE)
            .arg("init")
            .arg("--quiet")
            .arg(dir.path())
            .status()
            .expect("git init");
        assert!(status.success(), "git init failed");
        dir
    }

    /// Put the working tree in the index, which is where git looks to pair
    /// the two halves of a rename.
    fn stage_everything(repo: &Path) {
        let status = Command::new(GIT_EXECUTABLE)
            .arg("-C")
            .arg(repo)
            .args(["add", "-A"])
            .status()
            .expect("git add");
        assert!(status.success(), "git add failed");
    }

    /// Give the repository a `HEAD` to measure from. Identity is passed on the
    /// command line so the test does not depend on whoever is running it
    /// having configured one.
    fn commit_everything(repo: &Path) {
        for args in [
            vec!["add", "-A"],
            vec![
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "user.name=test",
                "commit",
                "--quiet",
                "-m",
                "base",
            ],
        ] {
            let status = Command::new(GIT_EXECUTABLE)
                .arg("-C")
                .arg(repo)
                .args(&args)
                .status()
                .expect("git");
            assert!(status.success(), "git {args:?} failed");
        }
    }

    /// A repository whose first commit has not landed has no `HEAD`, and
    /// `git diff HEAD` fails on it outright. Everything in it is untracked,
    /// so an empty diff is the true answer — a fresh repository must not read
    /// as a broken one.
    #[test]
    fn a_repository_with_no_commits_yet_diffs_to_nothing() {
        let dir = empty_repository();
        std::fs::write(dir.path().join("first.txt"), "hello\n").expect("write");
        let orchestrator = Orchestrator::open(dir.path()).expect("open");

        assert_eq!(
            orchestrator.diff(dir.path(), &["first.txt"]).expect("diff"),
            ""
        );
    }

    /// git escapes every non-ASCII byte of a path in `status --porcelain`
    /// unless told otherwise, and the escaped spelling matches no file when it
    /// is handed back as a pathspec. Unfixed, every Korean filename in the
    /// project refuses to open its own diff.
    #[test]
    fn a_korean_filename_comes_back_from_status_as_itself() {
        let dir = empty_repository();
        std::fs::write(dir.path().join("한글.txt"), "hello\n").expect("write");
        let orchestrator = Orchestrator::open(dir.path()).expect("open");

        let loss = orchestrator.pending_loss(dir.path()).expect("status");
        assert!(
            loss.uncommitted
                .iter()
                .any(|entry| entry.path == "한글.txt"),
            "status escaped the path: {:?}",
            loss.uncommitted
        );
    }

    /// Porcelain v1 quotes a path that carries a space, `core.quotePath` or
    /// not, so the string that came back was `"my -> file.txt"` — quotation
    /// marks and all — and handing that to git as a pathspec matches nothing.
    /// The literal ` -> ` inside it is the second half of the trap: a reader
    /// that splits a rename on that substring cuts an ordinary filename in
    /// two.
    #[test]
    fn a_path_carrying_a_space_and_an_arrow_survives_status_intact() {
        let dir = empty_repository();
        std::fs::write(dir.path().join("my -> file.txt"), "a\n").expect("write");
        let orchestrator = Orchestrator::open(dir.path()).expect("open");

        let loss = orchestrator.pending_loss(dir.path()).expect("status");
        assert_eq!(loss.uncommitted.len(), 1, "{loss:?}");
        assert_eq!(loss.uncommitted[0].path, "my -> file.txt");
        assert_eq!(loss.uncommitted[0].origin, None, "this is not a rename");
    }

    /// The two status columns, the untracked `??` the badge shows verbatim,
    /// and the trailing slash `--ignored` puts on a directory — which has to
    /// go, or the path never matches the tree's own `target` row and the panel
    /// shows build output as an ordinary folder.
    #[test]
    fn status_records_split_into_code_and_path() {
        let loss = parse_status(concat!(
            "1 .M N... 100644 100644 100644 aaa bbb ui/shell.js\0",
            "? new-file.txt\0",
            "1 A. N... 000000 100644 100644 000 ccc crates/x.rs\0",
            "! target/\0",
        ));

        assert_eq!(
            loss.uncommitted
                .iter()
                .map(|entry| (entry.path.as_str(), entry.code.as_str()))
                .collect::<Vec<_>>(),
            vec![
                // The column matters: ` M` is the worktree, `A ` the index —
                // and version two's `.` is translated back to the space every
                // reader downstream is written against.
                ("ui/shell.js", " M"),
                ("new-file.txt", "??"),
                ("crates/x.rs", "A "),
            ]
        );
        assert!(
            loss.uncommitted
                .iter()
                .all(|entry| entry.submodule.is_none()),
            "an ordinary file was read as a submodule: {:?}",
            loss.uncommitted
        );
        assert_eq!(loss.ignored, vec!["target".to_string()]);
        assert!(parse_status("").uncommitted.is_empty());
        // 머리 두 글자가 규격이 아닌 줄은 통째로 건너뛴다 — 헤더(`# branch.oid`)가
        // 딸려 오는 날 그것이 경로로 읽히지 않는다.
        assert!(parse_status("# branch.oid abc\0").uncommitted.is_empty());
    }

    /// 펼친 서브모듈이 읽어 오는 것들 — 이름-상태, 셈, 그리고 gitlink 커밋.
    #[test]
    fn an_expanded_submodule_reads_its_range_its_tallies_and_its_pointer() {
        // `-z`에서는 글자와 경로가 각각 한 필드이고, rename은 셋을 쓴다.
        let ranged = parse_name_status("M\0src/a.rs\0R100\0old.rs\0new.rs\0A\0src/b.rs\0");
        assert_eq!(
            ranged
                .iter()
                .map(|entry| (
                    entry.code.as_str(),
                    entry.path.as_str(),
                    entry.origin.as_deref()
                ))
                .collect::<Vec<_>>(),
            vec![
                // 범위는 이 워크트리가 올릴 것이 아니라 이미 있는 역사이므로
                // 글자는 스테이지 칸에 선다.
                ("M ", "src/a.rs", None),
                ("R ", "new.rs", Some("old.rs")),
                ("A ", "src/b.rs", None),
            ]
        );

        // rename은 셋째 칸이 비고 진짜 경로 둘을 뒤 필드에 싣는다. 이진 파일은
        // 셈이 없지만 자기 경로 칸은 그대로 쓴다 — 안 읽으면 그 뒤가 전부 밀린다.
        let tallies = parse_numstat("3\t1\tsrc/a.rs\0-\t-\timg.png\x002\t0\t\0old.rs\0new.rs\0");
        assert_eq!(tallies.get("src/a.rs"), Some(&(3, 1)));
        assert_eq!(tallies.get("new.rs"), Some(&(2, 0)));
        assert_eq!(tallies.get("img.png"), None);

        assert_eq!(
            gitlink_oid("160000 commit abc123\tvendor/lib\n", "160000 commit ").as_deref(),
            Some("abc123")
        );
        assert_eq!(
            gitlink_oid("160000 def456 0\tvendor/lib\n", "160000 ").as_deref(),
            Some("def456")
        );
        // gitlink가 아닌 경로는 답이 없다 — 사람이 이름 지은 아무 디렉터리.
        assert_eq!(
            gitlink_oid("100644 blob abc\tREADME\n", "160000 commit "),
            None
        );
    }

    /// 서브모듈 이름은 창을 지나 오므로, 워크트리 밖을 가리키면 거절한다.
    #[test]
    fn a_submodule_name_that_reaches_out_of_the_worktree_is_refused() {
        let dir = empty_repository();
        let orchestrator = Orchestrator::open(dir.path()).expect("open");
        let root = dir.path();
        for name in ["", "../elsewhere", "/etc", "vendor/../.."] {
            assert!(
                matches!(
                    orchestrator.submodule_worktree(root, name),
                    Err(OrchestratorError::PathEscapesWorktree { .. })
                ),
                "`{name}` was allowed to name a worktree"
            );
        }
        assert_eq!(
            orchestrator
                .submodule_worktree(root, "vendor/lib")
                .expect("an ordinary name"),
            root.join("vendor/lib")
        );
    }

    /// 서브모듈은 평범한 수정 파일이 아니고, 그 사실은 `sub` 필드에만 있다.
    ///
    /// 세 이유가 각각 다른 답을 만든다: 커밋 포인터가 움직인 것은 부모가
    /// 스테이지할 수 있고, 안쪽 파일이 더러운 것은 **부모가 스테이지할 수 없다**.
    /// 짧은 형식은 셋 다 ` M`으로만 말하므로 이 구분이 v2를 쓰는 이유 전부다.
    #[test]
    fn a_submodule_says_which_of_the_three_things_is_dirty() {
        let loss = parse_status(concat!(
            // 안쪽 워크트리만 더럽다(추적/미추적 둘 다), 커밋은 그대로.
            "1 .M SC.. 160000 160000 160000 aaa aaa vendor/moved\0",
            "1 .M S.MU 160000 160000 160000 bbb bbb vendor/dirty\0",
            // 이미 스테이지된 커밋 이동: `sub`은 워크트리를 말하므로 `S...`인데
            // `XY`가 M이다 — 이 갈래가 없으면 아무 문제 없는 서브모듈로 읽힌다.
            "1 M. S... 160000 160000 160000 ccc ddd vendor/staged\0",
            "1 .M N... 100644 100644 100644 eee eee ui/a.js\0",
        ));
        let of = |path: &str| {
            loss.uncommitted
                .iter()
                .find(|entry| entry.path == path)
                .and_then(|entry| entry.submodule)
        };
        assert_eq!(
            of("vendor/moved"),
            Some(Submodule {
                commit_changed: true,
                tracked_changes: false,
                untracked_changes: false
            })
        );
        assert_eq!(
            of("vendor/dirty"),
            Some(Submodule {
                commit_changed: false,
                tracked_changes: true,
                untracked_changes: true
            })
        );
        assert_eq!(
            of("vendor/staged"),
            Some(Submodule {
                commit_changed: true,
                tracked_changes: false,
                untracked_changes: false
            })
        );
        assert_eq!(of("ui/a.js"), None);
    }

    /// Only a rename or a copy spends a second field. An unmerged record does
    /// not, and reading one as though it did would swallow the record after it
    /// and shift every path from there on.
    #[test]
    fn an_unmerged_record_does_not_swallow_the_next_one() {
        let loss = parse_status(concat!(
            "u UU N... 100644 100644 100644 100644 aaa bbb ccc conflicted.txt\0",
            "1 .M N... 100644 100644 100644 ddd ddd after.txt\0",
        ));

        assert_eq!(loss.uncommitted.len(), 2, "{loss:?}");
        assert_eq!(loss.uncommitted[0].code, "UU");
        assert_eq!(loss.uncommitted[0].origin, None);
        assert_eq!(loss.uncommitted[1].path, "after.txt");
    }

    /// A rename spends a second field on where the file came from, so the
    /// badge lands on the name the tree is showing now and the origin is read
    /// rather than cut out of the path. The record after it must still parse:
    /// mis-pairing that extra field shifts every later entry by one.
    #[test]
    fn a_rename_carries_its_origin_in_a_field_of_its_own() {
        let loss = parse_status(concat!(
            "2 RM N... 100644 100644 100644 aaa bbb R100 ui/new.js\0ui/old.js\0",
            "1 .M N... 100644 100644 100644 ccc ccc ui/shell.css\0",
        ));

        assert_eq!(loss.uncommitted.len(), 2, "{loss:?}");
        assert_eq!(loss.uncommitted[0].path, "ui/new.js");
        assert_eq!(loss.uncommitted[0].code, "RM");
        assert_eq!(loss.uncommitted[0].origin.as_deref(), Some("ui/old.js"));
        assert_eq!(loss.uncommitted[1].path, "ui/shell.css");
        assert_eq!(loss.uncommitted[1].origin, None);
    }

    /// The string the removal confirmation shows, and the one it compares
    /// against when it re-reads the loss before discarding. Both sides render
    /// through here, so the shape is pinned rather than left to agree by
    /// accident — a rename has to name both ends or the dialog understates
    /// what goes.
    #[test]
    fn a_record_renders_the_way_git_would_have_printed_it() {
        let loss = parse_status(concat!(
            "1 .M N... 100644 100644 100644 aaa aaa ui/shell.js\0",
            "2 R. N... 100644 100644 100644 bbb ccc R100 docs/new.md\0docs/old.md\0",
        ));

        assert_eq!(
            loss.uncommitted
                .iter()
                .map(StatusEntry::display)
                .collect::<Vec<_>>(),
            vec![
                " M ui/shell.js".to_string(),
                "R  docs/old.md -> docs/new.md".to_string(),
            ]
        );
    }

    /// The end of the rename story, against real git.
    ///
    /// A rename only exists once it is staged — until then the destination is
    /// untracked, `git diff HEAD` cannot see it at all, and `status` reports a
    /// deletion and an unrelated new file rather than a rename. So this is the
    /// staged case, which is exactly the case `origin` is populated for, and
    /// it proves the pairing end to end: both names produce `rename from/to`,
    /// and the destination alone produces a file that appeared from nowhere —
    /// a plausible wrong answer, which is worse than an error.
    #[test]
    fn a_rename_diffs_as_a_rename_only_when_both_names_are_given() {
        let dir = empty_repository();
        std::fs::write(dir.path().join("old.md"), "alpha\nbeta\ngamma\n").expect("write");
        let orchestrator = Orchestrator::open(dir.path()).expect("open");
        commit_everything(dir.path());

        std::fs::rename(dir.path().join("old.md"), dir.path().join("new.md")).expect("rename");
        std::fs::write(dir.path().join("new.md"), "alpha\nbeta\nDELTA\n").expect("edit");
        stage_everything(dir.path());

        let loss = orchestrator.pending_loss(dir.path()).expect("status");
        let renamed = loss
            .uncommitted
            .iter()
            .find(|entry| entry.path == "new.md")
            .expect("the new name is reported");
        let origin = renamed
            .origin
            .as_deref()
            .expect("a staged rename carries where it came from");
        assert_eq!(origin, "old.md");

        let together = orchestrator
            .diff(dir.path(), &["new.md", origin])
            .expect("diff");
        assert!(
            together.contains("rename from old.md") && together.contains("rename to new.md"),
            "both names did not produce a rename: {together}"
        );

        let alone = orchestrator.diff(dir.path(), &["new.md"]).expect("diff");
        assert!(
            !alone.contains("rename from"),
            "the destination alone should not have been enough: {alone}"
        );
    }

    /// git puts the rename letter in whichever column the change happened in.
    /// A rename that was staged and then edited again reports ` R` — the
    /// letter in the *second* column — and reading only the first leaves the
    /// origin field sitting in the stream, where it parses as a record of its
    /// own and shifts every path after it.
    #[test]
    fn a_rename_in_the_second_column_still_carries_its_origin() {
        let loss = parse_status(concat!(
            "2 .R N... 100644 100644 100644 aaa bbb R100 new-long-name.txt\0old-long-name.txt\0",
            "1 .M N... 100644 100644 100644 ccc ccc after.txt\0",
        ));

        assert_eq!(loss.uncommitted.len(), 2, "{loss:?}");
        assert_eq!(loss.uncommitted[0].path, "new-long-name.txt");
        assert_eq!(
            loss.uncommitted[0].origin.as_deref(),
            Some("old-long-name.txt")
        );
        assert_eq!(loss.uncommitted[1].path, "after.txt");
        assert_eq!(loss.uncommitted[1].origin, None);
    }

    /// How the removal dialog assembles what it shows: two spaces of indent
    /// per record, joined by newlines (`describeLoss` in `ui/shell.js`). The
    /// forgery below only appears once the lines are joined, so the test has
    /// to look at the same string the dialog does.
    fn as_the_dialog_shows_it(loss: &PendingLoss) -> String {
        loss.uncommitted
            .iter()
            .map(|entry| format!("  {}", entry.display()))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// A filename may contain a newline, and `-z` hands it over intact.
    /// Printed raw, one untracked file becomes two apparent entries — so the
    /// removal dialog shows a loss that is not the one it is about to take,
    /// and its re-check finds that text unchanged against a completely
    /// different loss and lets a `--force` through.
    #[test]
    fn a_newline_in_a_path_cannot_forge_a_second_entry() {
        let forged = parse_status("? a\n  ? b\0");
        let genuine = parse_status("? a\0? b\0");

        assert_eq!(forged.uncommitted.len(), 1, "{forged:?}");
        assert_eq!(
            forged.uncommitted[0].path, "a\n  ? b",
            "the raw path is what goes back to git"
        );
        assert_ne!(
            as_the_dialog_shows_it(&forged),
            as_the_dialog_shows_it(&genuine),
            "one file forged the display of two"
        );
    }

    /// The ignored list is shown the same way and is deleted just as
    /// permanently, so a newline in one of *those* paths forges entries too.
    /// Measured collision before quoting reached this list: one ignored path
    /// `a\n  b` and two ignored paths `a`, `b` both printed as `  a\n  b`.
    #[test]
    fn an_ignored_path_with_a_newline_cannot_forge_two_entries() {
        let forged = parse_status("! a\n  b\0");
        let genuine = parse_status("! a\0! b\0");

        assert_eq!(forged.ignored.len(), 1, "{forged:?}");
        assert_ne!(
            forged.ignored_display(),
            genuine.ignored_display(),
            "one ignored path forged the display of two"
        );
    }

    /// A path may contain the arrow a rename is printed with, on either side.
    /// Unquoted, `a` → `b -> c` and `a -> b` → `c` both read as
    /// `R  a -> b -> c` — the same line for two different renames.
    #[test]
    fn two_different_renames_do_not_render_alike() {
        let one = parse_status("2 R. N... 100644 100644 100644 aaa bbb R100 b -> c\0a\0");
        let other = parse_status("2 R. N... 100644 100644 100644 aaa bbb R100 c\0a -> b\0");

        assert_ne!(
            one.uncommitted_display(),
            other.uncommitted_display(),
            "two different renames rendered as the same line"
        );
    }

    /// Identity does not go through the rendering at all. Even if two losses
    /// were ever to print alike, the token the confirmation compares is built
    /// from the raw fields and separates them.
    #[test]
    fn a_fingerprint_separates_losses_whose_text_could_collide() {
        assert_ne!(
            parse_status("? a\n  ? b\0").fingerprint(),
            parse_status("? a\0? b\0").fingerprint()
        );
        assert_ne!(
            parse_status("! a\n  b\0").fingerprint(),
            parse_status("! a\0! b\0").fingerprint()
        );
        assert_ne!(
            parse_status("2 R. N... 100644 100644 100644 aaa bbb R100 b -> c\0a\0").fingerprint(),
            parse_status("2 R. N... 100644 100644 100644 aaa bbb R100 c\0a -> b\0").fingerprint()
        );
    }

    /// The same loss twice is the same token — otherwise the confirmation
    /// would refuse every removal as "it just changed".
    #[test]
    fn a_fingerprint_is_stable_for_an_unchanged_loss() {
        let text = " M ui/shell.js\0R  docs/new.md\0docs/old.md\0!! target/\0";
        assert_eq!(
            parse_status(text).fingerprint(),
            parse_status(text).fingerprint()
        );
    }

    /// Give the repository an identity of its own so a commit does not depend
    /// on whoever is running the suite having configured one.
    fn configure_identity(repo: &Path) {
        for (key, value) in [
            ("user.name", "ZeroCode Test"),
            ("user.email", "t@example.com"),
        ] {
            assert!(
                Command::new(GIT_EXECUTABLE)
                    .args(["-C"])
                    .arg(repo)
                    .args(["config", key, value])
                    .status()
                    .expect("git config")
                    .success()
            );
        }
    }

    /// The status code a path currently carries, or `None` when git has
    /// nothing to say about it.
    fn code_of(loss: &PendingLoss, path: &str) -> Option<String> {
        loss.uncommitted
            .iter()
            .find(|entry| entry.path == path)
            .map(|entry| entry.code.clone())
    }

    /// Staging moves the letter from the worktree column into the index one —
    /// which is the whole reason `StatusEntry` keeps both columns, and the
    /// whole reason the panel can show two groups.
    #[test]
    fn staging_moves_a_path_into_the_index_column() {
        let repo = empty_repository();
        std::fs::write(repo.path().join("a.txt"), "one\n").expect("write");
        let orchestrator = Orchestrator::open(repo.path()).expect("open");

        let before = orchestrator.pending_loss(repo.path()).expect("status");
        assert_eq!(code_of(&before, "a.txt").as_deref(), Some("??"));

        orchestrator.stage(repo.path(), &["a.txt"]).expect("stage");

        let after = orchestrator.pending_loss(repo.path()).expect("status");
        assert_eq!(code_of(&after, "a.txt").as_deref(), Some("A "));
    }

    /// Unstaging before the first commit lands. `restore --staged` has no
    /// `HEAD` to restore from here and fails outright, so this is the case
    /// that proves the other command is actually reached.
    #[test]
    fn unstaging_works_on_a_repository_with_no_commits_yet() {
        let repo = empty_repository();
        std::fs::write(repo.path().join("a.txt"), "one\n").expect("write");
        let orchestrator = Orchestrator::open(repo.path()).expect("open");
        orchestrator.stage(repo.path(), &["a.txt"]).expect("stage");

        orchestrator
            .unstage(repo.path(), &["a.txt"])
            .expect("unstage");

        let after = orchestrator.pending_loss(repo.path()).expect("status");
        assert_eq!(code_of(&after, "a.txt").as_deref(), Some("??"));
        // The file itself is untouched: unstaging is not a discard.
        assert!(repo.path().join("a.txt").exists());
    }

    /// Unstaging once there *is* a commit takes the other branch, and still
    /// leaves the working tree alone.
    #[test]
    fn unstaging_after_a_commit_keeps_the_edit_in_the_working_tree() {
        let repo = empty_repository();
        configure_identity(repo.path());
        std::fs::write(repo.path().join("a.txt"), "one\n").expect("write");
        stage_everything(repo.path());
        commit_everything(repo.path());
        let orchestrator = Orchestrator::open(repo.path()).expect("open");
        std::fs::write(repo.path().join("a.txt"), "two\n").expect("rewrite");
        orchestrator.stage(repo.path(), &["a.txt"]).expect("stage");

        orchestrator
            .unstage(repo.path(), &["a.txt"])
            .expect("unstage");

        let after = orchestrator.pending_loss(repo.path()).expect("status");
        assert_eq!(code_of(&after, "a.txt").as_deref(), Some(" M"));
        assert_eq!(
            std::fs::read_to_string(repo.path().join("a.txt")).expect("read"),
            "two\n"
        );
    }

    /// A commit takes what was staged and nothing else. There is no `-a` in
    /// `commit`, and this is what says so: the panel shows two groups so a
    /// person can choose, and sweeping in the other group silently would make
    /// that choice a lie.
    #[test]
    fn a_commit_takes_only_what_was_staged() {
        let repo = empty_repository();
        configure_identity(repo.path());
        std::fs::write(repo.path().join("in.txt"), "in\n").expect("write");
        std::fs::write(repo.path().join("out.txt"), "out\n").expect("write");
        let orchestrator = Orchestrator::open(repo.path()).expect("open");
        orchestrator.stage(repo.path(), &["in.txt"]).expect("stage");

        let landed = orchestrator.commit(repo.path(), "첫 커밋").expect("commit");
        assert!(!landed.is_empty(), "a commit reports where it landed");

        let after = orchestrator.pending_loss(repo.path()).expect("status");
        assert_eq!(code_of(&after, "in.txt"), None, "the staged file went in");
        assert_eq!(
            code_of(&after, "out.txt").as_deref(),
            Some("??"),
            "the unstaged file stayed behind"
        );
    }

    /// git strips `#` lines from a message it opened an editor for. Measured
    /// here rather than trusted, because a person writing `#123 수정` in the
    /// panel would otherwise watch their whole message disappear.
    #[test]
    fn a_message_that_starts_with_a_hash_survives() {
        let repo = empty_repository();
        configure_identity(repo.path());
        std::fs::write(repo.path().join("a.txt"), "one\n").expect("write");
        let orchestrator = Orchestrator::open(repo.path()).expect("open");
        orchestrator.stage(repo.path(), &["a.txt"]).expect("stage");

        orchestrator
            .commit(repo.path(), "#123 로그인 오류 수정")
            .expect("commit");

        let subject = Command::new(GIT_EXECUTABLE)
            .args(["-C"])
            .arg(repo.path())
            .args(["log", "-1", "--format=%s"])
            .output()
            .expect("git log");
        assert_eq!(
            String::from_utf8_lossy(&subject.stdout).trim(),
            "#123 로그인 오류 수정"
        );
    }

    /// A real conflicted merge, because the operation is read off the git
    /// directory and only git puts the right files there. Nothing here is
    /// mocked: the repository is made, the branches diverge on one line, and
    /// the merge is left standing where it stopped.
    #[test]
    fn the_operation_in_progress_is_read_from_gits_own_marks() {
        let repo = empty_repository();
        configure_identity(repo.path());
        let git = |args: &[&str]| {
            Command::new(GIT_EXECUTABLE)
                .arg("-C")
                .arg(repo.path())
                .args(args)
                .output()
                .expect("git")
        };
        std::fs::write(repo.path().join("a.txt"), "base\n").expect("write");
        git(&["add", "a.txt"]);
        git(&["commit", "-m", "base"]);
        let orchestrator = Orchestrator::open(repo.path()).expect("open");

        // A settled checkout is in no operation, and that is not a failure.
        assert_eq!(
            orchestrator
                .conflict_operation(repo.path())
                .expect("settled"),
            ConflictOperation::Unknown
        );

        git(&["checkout", "-q", "-b", "other"]);
        std::fs::write(repo.path().join("a.txt"), "theirs\n").expect("write");
        git(&["commit", "-aqm", "theirs"]);
        git(&["checkout", "-q", "-"]);
        std::fs::write(repo.path().join("a.txt"), "ours\n").expect("write");
        git(&["commit", "-aqm", "ours"]);
        let merged = git(&["merge", "other"]);
        assert!(!merged.status.success(), "the merge should have conflicted");

        assert_eq!(
            orchestrator
                .conflict_operation(repo.path())
                .expect("mid-merge"),
            ConflictOperation::Merge
        );
        // And the file is reported with the code the kind table reads.
        let loss = orchestrator.pending_loss(repo.path()).expect("status");
        let conflicted = loss
            .uncommitted
            .iter()
            .find(|entry| entry.path == "a.txt")
            .expect("a.txt");
        assert_eq!(
            zerocode_core::conflict::ConflictKind::from_code(&conflicted.code),
            Some(zerocode_core::conflict::ConflictKind::BothModified),
            "the conflicted file reported {:?}",
            conflicted.code
        );

        // The undo git offers, and the refusal where it offers none.
        assert!(matches!(
            orchestrator.abort_operation(repo.path(), ConflictOperation::CherryPick),
            Err(OrchestratorError::NothingToAbort)
        ));
        orchestrator
            .abort_operation(repo.path(), ConflictOperation::Merge)
            .expect("abort");
        assert_eq!(
            orchestrator
                .conflict_operation(repo.path())
                .expect("after abort"),
            ConflictOperation::Unknown
        );
        assert_eq!(
            std::fs::read_to_string(repo.path().join("a.txt")).expect("read"),
            "ours\n",
            "the abort did not put our side back"
        );
    }

    /// The question an automatic reclaim asks before it deletes anything:
    /// has this branch's work landed where it belongs?
    ///
    /// Three answers, and the third is the one that has to be a separate
    /// answer rather than a zero — a base that is not here at all. Read as
    /// "nothing outstanding", it would hand a reclaim exactly the checkouts
    /// whose base somebody deleted, which are the ones most likely to hold
    /// the only copy of something.
    #[test]
    fn commits_beyond_separates_landed_work_from_a_base_that_is_gone() {
        let repo = empty_repository();
        configure_identity(repo.path());
        let git = |args: &[&str]| {
            let output = Command::new(GIT_EXECUTABLE)
                .arg("-C")
                .arg(repo.path())
                .args(args)
                .output()
                .expect("git");
            assert!(output.status.success(), "git {args:?} failed");
        };
        std::fs::write(repo.path().join("a.txt"), "base\n").expect("write");
        git(&["add", "a.txt"]);
        git(&["commit", "-qm", "base"]);
        git(&["branch", "-M", "main"]);
        let orchestrator = Orchestrator::open(repo.path()).expect("open");

        // A branch that has not moved carries nothing its base does not.
        git(&["branch", "wt/landed", "main"]);
        assert_eq!(
            orchestrator
                .commits_beyond(repo.path(), "main", "wt/landed")
                .expect("count"),
            Some(0)
        );

        // One commit on it, and the count is what a reclaim must stop on.
        git(&["checkout", "-q", "wt/landed"]);
        std::fs::write(repo.path().join("b.txt"), "work\n").expect("write");
        git(&["add", "b.txt"]);
        git(&["commit", "-qm", "work"]);
        assert_eq!(
            orchestrator
                .commits_beyond(repo.path(), "main", "wt/landed")
                .expect("count"),
            Some(1)
        );

        // And once the base has taken it, zero again — the base moving
        // forward is exactly how work stops being outstanding.
        git(&["checkout", "-q", "main"]);
        git(&["merge", "-q", "--ff-only", "wt/landed"]);
        assert_eq!(
            orchestrator
                .commits_beyond(repo.path(), "main", "wt/landed")
                .expect("count"),
            Some(0)
        );

        // A base nobody can resolve is not zero.
        assert_eq!(
            orchestrator
                .commits_beyond(repo.path(), "no-such-base", "wt/landed")
                .expect("count"),
            None
        );
        // Nor is a head nobody can resolve.
        assert_eq!(
            orchestrator
                .commits_beyond(repo.path(), "main", "no-such-branch")
                .expect("count"),
            None
        );
        // A name that would otherwise reach git's option parser resolves to
        // nothing rather than to an option.
        assert_eq!(
            orchestrator
                .commits_beyond(repo.path(), "--all", "wt/landed")
                .expect("count"),
            None
        );
    }

    /// The two destructive verbs behind the panel's confirmation dialog, on a
    /// real repository. A discard restores to `HEAD` and not to the index —
    /// the staged-then-edited file is the case that separates the two sources,
    /// and the wrong one silently "discards" to a half nobody was shown. A
    /// clean deletes the untracked file it was asked about and cannot reach a
    /// tracked one, whatever it is asked.
    #[test]
    fn a_discard_restores_head_and_a_clean_only_reaches_the_untracked() {
        let repo = empty_repository();
        configure_identity(repo.path());
        std::fs::write(repo.path().join("kept.txt"), "committed\n").expect("write");
        commit_everything(repo.path());
        let orchestrator = Orchestrator::open(repo.path()).expect("open");

        // Staged one wording, edited to another: the discard must land on the
        // COMMITTED text, walking past the staged half (`--source=HEAD`).
        std::fs::write(repo.path().join("kept.txt"), "staged\n").expect("write");
        orchestrator
            .stage(repo.path(), &["kept.txt"])
            .expect("stage");
        std::fs::write(repo.path().join("kept.txt"), "edited again\n").expect("write");
        orchestrator
            .discard_worktree(repo.path(), &["kept.txt"])
            .expect("discard");
        assert_eq!(
            std::fs::read_to_string(repo.path().join("kept.txt")).expect("read"),
            "committed\n",
            "a bare `restore` would have answered with the staged half"
        );

        // The untracked file goes; the tracked one beside it is not clean's
        // to take, even named outright.
        std::fs::write(repo.path().join("loose.txt"), "untracked\n").expect("write");
        orchestrator
            .clean_untracked(repo.path(), &["loose.txt", "kept.txt"])
            .expect("clean");
        assert!(
            !repo.path().join("loose.txt").exists(),
            "the untracked file should be gone"
        );
        assert!(
            repo.path().join("kept.txt").exists(),
            "clean must not reach a tracked file"
        );

        // Empty asks are answered without spawning git at all — the header's
        // bulk hands repaint on every refresh, and a filtered-out section
        // asks with nothing.
        orchestrator
            .discard_worktree::<&str>(repo.path(), &[])
            .expect("empty discard");
        orchestrator
            .clean_untracked::<&str>(repo.path(), &[])
            .expect("empty clean");
    }

    /// The commit that fails most often is the one a pre-commit hook blocked,
    /// and a hook writes its diagnostics to STDOUT — husky, lint-staged,
    /// lefthook and every linter under them. Read from stderr alone, the
    /// person whose commit was just refused was told "no output" while the
    /// reason sat unread in the other pipe.
    #[test]
    fn a_hook_that_speaks_on_stdout_is_still_heard() {
        let repo = empty_repository();
        configure_identity(repo.path());
        let hooks = repo.path().join(".git").join("hooks");
        std::fs::create_dir_all(&hooks).expect("hooks dir");
        let hook = hooks.join("pre-commit");
        // Everything on stdout, nothing on stderr — the ordinary shape.
        std::fs::write(
            &hook,
            "#!/bin/sh\necho 'lint: 3 problems in src/a.js'\nexit 1\n",
        )
        .expect("write hook");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        std::fs::write(repo.path().join("a.txt"), "one\n").expect("write");
        let orchestrator = Orchestrator::open(repo.path()).expect("open");
        orchestrator.stage(repo.path(), &["a.txt"]).expect("stage");

        let refused = orchestrator
            .commit(repo.path(), "블록될 커밋")
            .expect_err("the hook should have blocked this");
        let said = refused.to_string();
        assert!(
            said.contains("lint: 3 problems in src/a.js"),
            "the hook's own words never reached the person: {said}"
        );
        assert!(
            !said.contains("no output"),
            "a hook that said plenty was reported as silent: {said}"
        );
    }

    /// Both refusals are ours, so the dialog can say which thing was wrong
    /// instead of forwarding git's prose about it.
    #[test]
    fn a_commit_refuses_an_empty_message_and_an_empty_index() {
        let repo = empty_repository();
        configure_identity(repo.path());
        std::fs::write(repo.path().join("a.txt"), "one\n").expect("write");
        let orchestrator = Orchestrator::open(repo.path()).expect("open");

        assert!(matches!(
            orchestrator.commit(repo.path(), "   \n "),
            Err(OrchestratorError::EmptyCommitMessage)
        ));
        assert!(
            matches!(
                orchestrator.commit(repo.path(), "메시지는 있다"),
                Err(OrchestratorError::NothingStaged)
            ),
            "an untracked file is not staged content"
        );
    }

    /// A pathspec is a glob, so staging one file must not stage another that
    /// its name happens to match — the same trap `diff` already carries a
    /// `:(literal)` for.
    #[test]
    fn staging_a_glob_character_stages_that_file_and_no_other() {
        let repo = empty_repository();
        std::fs::write(repo.path().join("star*name.txt"), "literal\n").expect("write");
        std::fs::write(repo.path().join("starXname.txt"), "matched\n").expect("write");
        let orchestrator = Orchestrator::open(repo.path()).expect("open");

        orchestrator
            .stage(repo.path(), &["star*name.txt"])
            .expect("stage");

        let after = orchestrator.pending_loss(repo.path()).expect("status");
        assert_eq!(code_of(&after, "star*name.txt").as_deref(), Some("A "));
        assert_eq!(code_of(&after, "starXname.txt").as_deref(), Some("??"));
    }

    /// A filename is free to contain the arrow a rename is printed with, and
    /// free to keep a trailing space. Neither is a rename, and neither may be
    /// trimmed away — the string's whole job is to go back to git as a path.
    #[test]
    fn an_arrow_inside_a_filename_is_not_a_rename() {
        let loss = parse_status("? my -> file.txt\0? trailing \0");

        assert_eq!(loss.uncommitted[0].path, "my -> file.txt");
        assert_eq!(loss.uncommitted[0].origin, None);
        assert_eq!(loss.uncommitted[1].path, "trailing ");
    }

    /// A pathspec is a glob unless it is told not to be, so a file honestly
    /// named `star*name.txt` would otherwise diff whatever else that pattern
    /// matched — here, a different file entirely.
    #[test]
    fn a_glob_character_in_a_filename_diffs_that_file_and_no_other() {
        let dir = empty_repository();
        std::fs::write(dir.path().join("star*name.txt"), "one\n").expect("write");
        std::fs::write(dir.path().join("starXname.txt"), "one\n").expect("write");
        let orchestrator = Orchestrator::open(dir.path()).expect("open");
        commit_everything(dir.path());
        std::fs::write(dir.path().join("starXname.txt"), "two\n").expect("rewrite");

        assert_eq!(
            orchestrator
                .diff(dir.path(), &["star*name.txt"])
                .expect("diff"),
            "",
            "the glob dragged in a file the caller did not ask about"
        );
    }

    /// A branch ref deleted out from under `HEAD` leaves a symbolic ref
    /// pointing at nothing — and that is indistinguishable from a first commit
    /// that has not landed: `rev-parse`, `symbolic-ref` and `HEAD@{0}` all
    /// answer the same in both, and `git status` shows both as a tree of
    /// additions. The answer is therefore the same for both, pinned here so it
    /// stays a decision rather than an accident.
    #[test]
    fn a_branch_ref_deleted_under_head_reads_the_same_as_an_unborn_one() {
        let dir = empty_repository();
        std::fs::write(dir.path().join("first.txt"), "hello\n").expect("write");
        let orchestrator = Orchestrator::open(dir.path()).expect("open");
        commit_everything(dir.path());

        let branch = String::from_utf8(
            Command::new(GIT_EXECUTABLE)
                .arg("-C")
                .arg(dir.path())
                .args(["symbolic-ref", "--short", "HEAD"])
                .output()
                .expect("symbolic-ref")
                .stdout,
        )
        .expect("utf-8");
        std::fs::remove_file(
            dir.path()
                .join(".git")
                .join("refs")
                .join("heads")
                .join(branch.trim()),
        )
        .expect("delete the branch ref");

        assert_eq!(
            orchestrator.diff(dir.path(), &["first.txt"]).expect("diff"),
            "",
            "a dangling symbolic HEAD answers the same as an unborn one"
        );
    }

    /// A `HEAD` that cannot be resolved for any reason other than being unborn
    /// is a broken repository, and saying "no changes" about one would be a
    /// quiet wrong answer rather than a failure anybody could act on.
    #[test]
    fn a_damaged_head_is_reported_rather_than_read_as_empty() {
        let dir = empty_repository();
        std::fs::write(dir.path().join("first.txt"), "hello\n").expect("write");
        let orchestrator = Orchestrator::open(dir.path()).expect("open");
        commit_everything(dir.path());
        std::fs::write(dir.path().join(".git").join("HEAD"), "ref: refs/heads/\n")
            .expect("damage HEAD");

        assert!(
            orchestrator.diff(dir.path(), &["first.txt"]).is_err(),
            "a damaged HEAD came back as an empty diff"
        );
    }

    #[test]
    fn two_repositories_with_the_same_name_get_different_roots() {
        let first = default_worktree_root(Path::new("/home/dev/api"));
        let second = default_worktree_root(Path::new("/work/client/api"));
        assert_ne!(first, second);
        assert!(
            first
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("api-")
        );
        // Stable: the same repository resolves to the same root every run.
        assert_eq!(first, default_worktree_root(Path::new("/home/dev/api")));
    }

    /// The bulk roads ride chunks of a hundred (P0-18): a "stage all" over a
    /// few thousand files used to put every path on one command line, and
    /// the kernel refused the exec whole. Two hundred and fifty files cross
    /// three chunks; every one must land.
    #[test]
    fn a_bulk_stage_larger_than_one_chunk_lands_whole() {
        let dir = empty_repository();
        let mut paths = Vec::new();
        for at in 0..250 {
            let name = format!("gen-{at:03}.txt");
            std::fs::write(dir.path().join(&name), "made").expect("write");
            paths.push(name);
        }
        let orchestrator = Orchestrator::open(dir.path()).expect("open");
        orchestrator.stage(dir.path(), &paths).expect("stage all");
        // `ls-files` reads the index directly, so it works on the unborn
        // HEAD this repository still has.
        let listed = Command::new(GIT_EXECUTABLE)
            .arg("-C")
            .arg(dir.path())
            .args(["ls-files", "-z"])
            .output()
            .expect("git ls-files");
        let staged = String::from_utf8_lossy(&listed.stdout);
        assert_eq!(
            staged.split('\0').filter(|name| !name.is_empty()).count(),
            250,
            "a chunk went missing between the walk's fences"
        );
        assert_eq!(BULK_CHUNK, 100, "the measured chunk size moved");
    }

    /// The discard asks GIT which paths are tracked, at discard time — the
    /// window's rows are a snapshot, and the wrong verb on a stale row
    /// either errors the batch or silently skips a file somebody chose to
    /// lose (`bulkDiscardChanges`).
    #[test]
    fn a_discard_classifies_by_git_truth_not_by_what_a_panel_remembered() {
        let dir = empty_repository();
        std::fs::write(dir.path().join("kept.txt"), "committed\n").expect("write");
        stage_everything(dir.path());
        commit_everything(dir.path());
        std::fs::write(dir.path().join("kept.txt"), "edited\n").expect("edit");
        std::fs::write(dir.path().join("loose.txt"), "untracked\n").expect("write");

        let orchestrator = Orchestrator::open(dir.path()).expect("open");
        // ONE list, mixed on purpose — the caller does not have to know.
        orchestrator
            .discard_changes(
                dir.path(),
                &["kept.txt".to_string(), "loose.txt".to_string()],
            )
            .expect("discard");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("kept.txt")).expect("read"),
            "committed\n",
            "the tracked edit was not restored to HEAD"
        );
        assert!(
            !dir.path().join("loose.txt").exists(),
            "the untracked file survived its own discard"
        );
    }

    /// The untracked half walks under suspicion: a symlinked directory must
    /// not steer the removal outside the worktree, however the path was
    /// spelled — and a refusal leaves EVERYTHING untouched, the outside
    /// target most of all. (Unix-only: making the hostile link on Windows
    /// needs a privilege test machines do not hold; the fence itself is
    /// portable and proven in the test below this one.)
    #[cfg(unix)]
    #[test]
    fn a_symlinked_parent_cannot_steer_a_discard_outside_the_worktree() {
        let dir = empty_repository();
        let outside = tempfile::tempdir().expect("outside");
        std::fs::write(outside.path().join("victim.txt"), "elsewhere").expect("write");
        std::os::unix::fs::symlink(outside.path(), dir.path().join("door")).expect("symlink");

        let orchestrator = Orchestrator::open(dir.path()).expect("open");
        let refused = orchestrator
            .discard_changes(dir.path(), &["door/victim.txt".to_string()])
            .expect_err("a removal through a symlinked parent was allowed");
        assert!(matches!(
            refused,
            OrchestratorError::PathEscapesWorktree { .. }
        ));
        assert!(
            outside.path().join("victim.txt").exists(),
            "the refusal came after the harm"
        );

        // The link ITSELF is deletable — it is untracked litter, and taking
        // the leaf does not walk through it.
        orchestrator
            .discard_changes(dir.path(), &["door".to_string()])
            .expect("removing the symlink leaf");
        assert!(
            !dir.path().join("door").exists() && outside.path().join("victim.txt").exists(),
            "deleting the leaf reached through it"
        );
    }

    /// The lexical fence stands in front of everything else — the refusal
    /// lands before git is even asked — and it is pure path arithmetic, so
    /// every platform runs this one.
    #[test]
    fn paths_that_leave_the_worktree_are_refused_before_git_is_asked() {
        let dir = empty_repository();
        let orchestrator = Orchestrator::open(dir.path()).expect("open");
        for hostile in ["../sibling.txt", "", ".", "/etc/hosts"] {
            assert!(
                matches!(
                    orchestrator.discard_changes(dir.path(), &[hostile.to_string()]),
                    Err(OrchestratorError::PathEscapesWorktree { .. })
                ),
                "`{hostile}` got past the fence"
            );
        }
    }
}
