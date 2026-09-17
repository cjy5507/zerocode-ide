//! The session vault: every conversation these agents have ever had, on disk.
//!
//! Orca's `AiVaultPanel` (`AiVaultPanel-_kbRUyaA.js`, 4,176 lines, 1.4.158;
//! `AiVaultPanel-y6jLG4bz.js`, 3,250 lines, 1.4.164) plus its rules chunk
//! `ai-vault-session-drag-Ed90SLu9.js` (655 lines). Despite the name it holds no
//! secrets — it is an archive of past sessions, read out of each agent's own
//! store, searchable, groupable, and resumable.
//!
//! ## Why this is a read-only surface, and why that is the design
//!
//! Every file this module opens belongs to a vendor CLI. Nothing here writes,
//! renames, or deletes — not even a cache beside a session. A panel that browses
//! somebody's conversation history and can also damage it is not worth having,
//! and "we only meant to write our own index file" is how a product corrupts a
//! `~/.claude/projects` directory. The strongest form of that promise is that no
//! writer exists to call, which a gate asserts by name.
//!
//! ## What is measured, and against what
//!
//! The discovery roots and the resume invocations are read out of Orca. The two
//! FILE FORMATS implemented here were verified against real files on a working
//! machine — `~/.claude/projects/*/*.jsonl` and `~/.codex/sessions/**/rollout-*.jsonl`
//! — because a parser measured only from somebody else's parser is a guess about
//! a third party's format.
//!
//! The other agents' roots are in [`AGENT_SOURCES`] and their sessions are
//! *found* but not parsed; each reports a [`ScanIssue`] naming itself. That is
//! deliberate and it is the honest state: a card with no title, no directory and
//! no resumable id is worse than a line saying "this agent's format is not read
//! yet", because the first looks like a working feature.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The vault's only table of session and child limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Limits {
    pub choices: [usize; 3],
    pub absolute_max: usize,
    pub default: usize,
    pub children_per_parent: usize,
}

impl Limits {
    pub const DEFAULT: Self = {
        let choices = [50, 100, 200];
        Self {
            choices,
            absolute_max: 1000,
            default: choices[2],
            children_per_parent: choices[0],
        }
    };

    /// Zero means all, bounded by the absolute cap. Invalid saved choices
    /// fall back to the table default, so an old setting cannot empty the list.
    pub fn session_limit(self, requested: Option<usize>) -> usize {
        match requested {
            Some(0) => self.absolute_max,
            Some(value) if self.choices.contains(&value) => value,
            _ => self.default,
        }
    }

    /// Apply the canonical settings overlay without changing the walk's cap.
    pub fn overlaid(mut self, overlay: &BTreeMap<String, usize>) -> Self {
        if let Some(value) = overlay.get("vault.sessionLimit") {
            self.default = self.session_limit(Some(*value));
        }
        self
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Compatibility name for the table's default view limit.
pub const DEFAULT_SCAN_LIMIT: usize = Limits::DEFAULT.default;

/// How many candidate files are considered per limit
/// (`SESSION_PARSE_CANDIDATE_MULTIPLIER`) — a session file can turn out to be
/// empty, so the walk looks at more than it will keep.
pub const CANDIDATE_MULTIPLIER: usize = 3;

/// The deepest a session store is walked.
///
/// Codex nests by `sessions/<year>/<month>/<day>/`, which is the deepest real
/// layout; the extra levels are slack for a vendor that adds one. A bound is
/// required rather than nice: these roots are outside our control and a symlink
/// or a mount inside one would otherwise walk a disk.
pub const MAX_DEPTH: usize = 6;

/// The most files one root will look at, however deep.
pub const MAX_CANDIDATE_FILES: usize = 4000;

/// Bytes read from the head of a session file to identify it.
///
/// A long conversation is megabytes and the card needs the first few records.
/// Reading the whole file for every one of two hundred cards is the difference
/// between a panel that opens and one that stalls the window.
pub const MAX_HEAD_BYTES: usize = 256 * 1024;

/// How long a title or preview line is kept. Lives with the shared extraction
/// (`crate::transcript`) so the vault's previews and the board's card lines
/// cut at the same place.
pub use crate::transcript::SUMMARY_CHARS;
use crate::transcript::{clamp, message_text};

/// One environment variable that can name where an agent keeps its sessions,
/// and the tail under it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HomeVar {
    pub name: &'static str,
    /// Appended to the variable's value. Empty when the variable already names
    /// the sessions directory itself.
    pub tail: &'static [&'static str],
}

/// One agent's session store: where to look, and what a session file looks like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentSource {
    /// The agent's slug — Orca's `AI_VAULT_AGENTS` spelling, which is also ours
    /// for the agents we already know.
    pub slug: &'static str,
    pub label: &'static str,
    /// Path segments under `$HOME`, or under the environment override.
    pub segments: &'static [&'static str],
    /// The environment variables that name this agent's sessions instead of
    /// `$HOME`, in the order the agent itself resolves them.
    ///
    /// A LIST, because one agent reads two: Prime Agent takes a session
    /// directory verbatim and an agent directory with `sessions` under it, and
    /// the session one outranks the other. The first variable that is set wins
    /// and the rest are not consulted — the same "never cross-fall-back" rule
    /// the original states for these agents, because substituting another
    /// variable's directory silently reports one agent's sessions as another's.
    ///
    /// A user who moved their agent home meant it, and a scanner that ignores
    /// the variable reports an empty vault on exactly the machines that use one.
    pub homes: &'static [HomeVar],
    /// File extension a session file carries.
    pub extension: &'static str,
    /// A required file name, when the store puts one metadata file per session
    /// directory (`summary.json`, `metadata.json`).
    pub file_name: Option<&'static str>,
    /// A required file-name prefix (`session_`).
    pub file_prefix: Option<&'static str>,
    /// A path segment the file must be under (`agent-transcripts`).
    pub path_segment: Option<&'static str>,
    /// Transcripts under this directory belong beneath their owning session.
    pub child_dir: Option<&'static str>,
    /// How the file is read.
    pub format: Format,
}

/// The shape of a session file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Format {
    /// One JSON object per line, each carrying `sessionId` and (on most lines)
    /// `cwd`. Verified against `~/.claude/projects`.
    ClaudeLines,
    /// One JSON object per line, the first being `{"type":"session_meta",…}`.
    /// Verified against `~/.codex/sessions`.
    CodexRollout,
    /// One JSON object per line: `{step_index, source, type, status,
    /// created_at, content}`. Verified against
    /// `~/.gemini/antigravity/brain/*/.system_generated/logs/transcript.jsonl`
    /// (5 conversations on this machine).
    AntigravityTranscript,
    /// One JSON object per line, in either of the store's two shapes: vault
    /// lines `{"message":{"blocks":[…],"role":…},"type":"vault","vault_seq":N}`
    /// or a plain log opening with `session_meta` (id and epoch-ms times)
    /// followed by `{"message":{…},"turn_index":N,"type":"message"}` turns.
    /// Verified against `~/.zo/projects/<slug>/sessions/session-*.jsonl` — our
    /// own CLI's store, which no Orca roster names (사용자 보고 2026-08-19 "zo를
    /// 실행시키면 프로젝트 목록에 안 보임"; 재실측 같은 날: vault를 가진
    /// 프로젝트는 0.5%뿐, 나머지 전부가 plain 로그였다 — 1,157 파일 전수에서
    /// 첫 줄은 모두 `session_meta`, 턴 type은 `message`와 `compaction`뿐).
    ZoLines,
    /// Found, not parsed. See the module doc.
    Unread,
}

/// Every agent whose sessions Orca lists, with the root it reads
/// (index.js:137227-137240, and the per-agent `discoverFiles` calls at
/// :137262-137390 for the extensions and predicates).
pub const AGENT_SOURCES: [AgentSource; 16] = [
    AgentSource {
        slug: "claude",
        label: "Claude",
        segments: &[".claude", "projects"],
        homes: &[],
        extension: "jsonl",
        file_name: None,
        file_prefix: None,
        path_segment: None,
        // The containing session directory identifies the parent.
        child_dir: Some("subagents"),
        format: Format::ClaudeLines,
    },
    AgentSource {
        slug: "codex",
        label: "Codex",
        segments: &[".codex", "sessions"],
        homes: &[HomeVar {
            name: "CODEX_HOME",
            tail: &["sessions"],
        }],
        extension: "jsonl",
        file_name: None,
        file_prefix: None,
        path_segment: None,
        child_dir: None,
        format: Format::CodexRollout,
    },
    AgentSource {
        slug: "cursor",
        label: "Cursor",
        segments: &[".cursor", "projects"],
        homes: &[],
        extension: "jsonl",
        file_name: None,
        file_prefix: None,
        path_segment: Some("agent-transcripts"),
        child_dir: None,
        format: Format::Unread,
    },
    AgentSource {
        slug: "copilot",
        label: "Copilot CLI",
        segments: &[".copilot", "session-state"],
        homes: &[HomeVar {
            name: "COPILOT_HOME",
            tail: &["session-state"],
        }],
        extension: "jsonl",
        file_name: None,
        file_prefix: None,
        path_segment: None,
        child_dir: None,
        format: Format::Unread,
    },
    AgentSource {
        slug: "pi",
        label: "pi",
        segments: &[".pi", "agent", "sessions"],
        homes: &[HomeVar {
            name: "PI_CODING_AGENT_DIR",
            tail: &[],
        }],
        extension: "jsonl",
        file_name: None,
        file_prefix: None,
        path_segment: None,
        child_dir: None,
        format: Format::Unread,
    },
    AgentSource {
        slug: "omp",
        label: "omp",
        segments: &[".omp", "agent", "sessions"],
        homes: &[HomeVar {
            name: "OMP_CODING_AGENT_DIR",
            tail: &[],
        }],
        extension: "jsonl",
        file_name: None,
        file_prefix: None,
        path_segment: None,
        child_dir: None,
        format: Format::Unread,
    },
    AgentSource {
        slug: "prime-agent",
        label: "Prime Agent",
        segments: &[".prime", "agent", "sessions"],
        // Two variables, and the order is the agent's own
        // (`session-scanner-values.ts:178-197`). A SESSION directory is taken
        // verbatim — it already names the transcripts root — and outranks the
        // agent directory, which always gets `sessions` under it even when the
        // value is itself named `sessions`. The legacy spelling of the session
        // variable is still read, because a machine set up before it was
        // renamed is still that machine.
        homes: &[
            HomeVar {
                name: "PRIME_AGENT_SESSION_DIR",
                tail: &[],
            },
            HomeVar {
                name: "PRIME_AGENT_CODING_AGENT_SESSION_DIR",
                tail: &[],
            },
            HomeVar {
                name: "PRIME_AGENT_CODING_AGENT_DIR",
                tail: &["sessions"],
            },
        ],
        extension: "jsonl",
        file_name: None,
        file_prefix: None,
        path_segment: None,
        child_dir: None,
        format: Format::Unread,
    },
    AgentSource {
        slug: "grok",
        label: "Grok CLI",
        segments: &[".grok", "sessions"],
        homes: &[HomeVar {
            name: "GROK_HOME",
            tail: &["sessions"],
        }],
        extension: "json",
        file_name: Some("summary.json"),
        file_prefix: None,
        path_segment: None,
        child_dir: None,
        format: Format::Unread,
    },
    AgentSource {
        slug: "hermes",
        label: "Hermes",
        segments: &[".hermes", "sessions"],
        homes: &[],
        extension: "json",
        file_name: None,
        file_prefix: Some("session_"),
        path_segment: None,
        child_dir: None,
        format: Format::Unread,
    },
    AgentSource {
        slug: "rovo",
        label: "Rovo Dev",
        segments: &[".rovodev", "sessions"],
        homes: &[],
        extension: "json",
        file_name: Some("metadata.json"),
        file_prefix: None,
        path_segment: None,
        child_dir: None,
        format: Format::Unread,
    },
    AgentSource {
        slug: "devin",
        label: "Devin",
        segments: &[".local", "share", "devin", "cli", "transcripts"],
        homes: &[HomeVar {
            name: "DEVIN_HOME",
            tail: &["transcripts"],
        }],
        extension: "json",
        file_name: None,
        file_prefix: None,
        path_segment: None,
        child_dir: None,
        format: Format::Unread,
    },
    AgentSource {
        slug: "droid",
        label: "Droid",
        segments: &[".factory", "sessions"],
        homes: &[],
        extension: "json",
        file_name: None,
        file_prefix: None,
        path_segment: None,
        child_dir: None,
        format: Format::Unread,
    },
    AgentSource {
        slug: "openclaw",
        label: "OpenClaw",
        segments: &[".openclaw"],
        homes: &[HomeVar {
            name: "OPENCLAW_STATE_DIR",
            tail: &[],
        }],
        extension: "json",
        file_name: None,
        file_prefix: None,
        path_segment: None,
        child_dir: None,
        format: Format::Unread,
    },
    // Antigravity twice, because its store moved between versions and both
    // spellings are real: Orca 1.4.169 reads `~/.gemini/antigravity-cli/brain`
    // (`ANTIGRAVITY_BRAIN_DIR`, out/main/index.js:132414); the install on this
    // machine writes `~/.gemini/antigravity/brain`. One walks empty on any
    // given machine and costs a stat. The old `.antigravity` root this table
    // used to name was WRONG — it is the IDE's install directory, and every
    // "session" counted there was a VS Code extension manifest.
    AgentSource {
        slug: "antigravity",
        label: "Antigravity",
        segments: &[".gemini", "antigravity", "brain"],
        homes: &[],
        extension: "jsonl",
        file_name: Some("transcript.jsonl"),
        file_prefix: None,
        path_segment: None,
        child_dir: None,
        format: Format::AntigravityTranscript,
    },
    AgentSource {
        slug: "antigravity",
        label: "Antigravity",
        segments: &[".gemini", "antigravity-cli", "brain"],
        homes: &[],
        extension: "jsonl",
        file_name: Some("transcript.jsonl"),
        file_prefix: None,
        path_segment: None,
        child_dir: None,
        format: Format::AntigravityTranscript,
    },
    // Ours, not Orca's: `zo` appears in no `AI_VAULT_AGENTS` roster, and a
    // white label that lists eleven vendors' conversations while losing its
    // own was reported in exactly those words ("zo를 실행시키면 프로젝트
    // 목록에 안 보임"). The conversation lives in TWO shapes side by side:
    // most sessions are a plain `session-*.jsonl` log (meta line, then
    // `message` turns), a few also distill into a `.vault.jsonl` twin — when
    // both stand, the vault is the card and the plain twin is shadowed at
    // candidate time, and `.rot-*` rotations of the log are refused by name
    // (they repeat turns a live file already carries). `path_segment` keeps
    // the walk out of the sibling `session-prefs`/`memory`/`state`
    // directories. `ZO_CONFIG_HOME` is the head of zo's own home ladder
    // ("set ZO_CONFIG_HOME, ZO_HOME, or HOME" — the binary's own words);
    // the `$HOME/.zo` fallback below covers the other two rungs.
    AgentSource {
        slug: "zo",
        label: "Zo",
        segments: &[".zo", "projects"],
        homes: &[HomeVar {
            name: "ZO_CONFIG_HOME",
            tail: &["projects"],
        }],
        extension: "jsonl",
        file_name: None,
        file_prefix: Some("session-"),
        path_segment: Some("sessions"),
        child_dir: None,
        format: Format::ZoLines,
    },
];

/// The base command an agent resumes with (`defaultAiVaultResumeCommandBase`,
/// ai-vault-session-drag:115-126).
pub fn resume_base(slug: &str) -> &str {
    match slug {
        "cursor" => "cursor-agent",
        "rovo" => "acli",
        "openclaw" => "openclaw",
        "droid" => "droid",
        // Orca's fallback is the agent's `detectCmd`
        // (`defaultAiVaultResumeCommandBase`), and antigravity's is `agy` —
        // the same fact `AgentKind::command` records. The slug spelled a
        // binary that does not exist, so every reopen died on
        // `command not found`.
        "antigravity" => "agy",
        other => other,
    }
}

/// How an agent is told which session to resume (`buildAgentResumeInvocation`,
/// :127-157).
///
/// Five different spellings across sixteen agents, and the differences are not
/// cosmetic — `codex resume <id>` with a `--resume` flag is an error, and a
/// resume that errors looks to the user like a session that was lost.
pub fn resume_invocation(slug: &str, base: &str, session_arg: &str) -> Option<String> {
    let words = match slug {
        "codex" => format!("{base} resume {session_arg}"),
        "rovo" => format!("{base} rovodev run --restore {session_arg}"),
        // Session stores scoped to a working directory: the resume has to run
        // from the session's own `cwd` or the CLI refuses.
        // Prime Agent embeds Pi's TUI and resumes the way Pi does — the
        // original routes the two through one branch for the same reason
        // (`ai-vault-resume-command.ts:262`).
        "opencode" | "pi" | "prime-agent" | "kimi" => {
            format!("{base} --session {session_arg}")
        }
        "copilot" => format!("{base} --resume={session_arg}"),
        "antigravity" => format!("{base} --conversation {session_arg}"),
        // `omp` resumes by absolute transcript path rather than id, but spells
        // the flag like the rest. `zo` is ours and spells it the same way —
        // `zo --resume [SESSION.jsonl|session-id|latest]`, its own --help.
        "claude" | "cursor" | "grok" | "hermes" | "devin" | "openclaw" | "droid" | "omp" | "zo" => {
            format!("{base} --resume {session_arg}")
        }
        _ => return None,
    };
    Some(words)
}

/// `'…'`-quote for a POSIX shell.
pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// A word on the resume line: bare while it is shell-plain, quoted the moment
/// it is not. The words this carries are launch flags, almost always plain —
/// and a line of needlessly quoted flags is a line nobody can read in the
/// panel.
pub fn resume_word(word: &str) -> String {
    let plain = !word.is_empty()
        && word.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(c, '_' | '-' | '+' | '=' | '.' | '/' | ':' | ',' | '@' | '%')
        });
    if plain {
        word.to_string()
    } else {
        shell_quote(word)
    }
}

/// Whether this agent's vault reopen carries the launch plan's args.
///
/// Orca's split exactly: a session of a `RESUMABLE_TUI_AGENTS` member goes
/// through `buildAgentResumeStartupPlan`, whose base command is the launch
/// command PLUS the resolved args; every other agent keeps the bare base
/// (renderer ai-vault-resume-command.ts:163-224). This list is that roster
/// intersected with the agents this vault scans — mimo-code and prime-agent
/// are the roster's other two members, and neither has a source in
/// [`AGENT_SOURCES`] yet.
///
/// Read off the agent's row (`Harness::vault_resume_carries_launch_args`),
/// where the other resume facts live, rather than as a list of names here.
pub fn resume_carries_launch_args(slug: &str) -> bool {
    crate::agent_capabilities(slug).is_some_and(|caps| caps.resume.vault_carries_launch_args)
}

/// The whole command a resume runs, including the `cd` and Codex's home
/// (`buildAiVaultResumeCommand` + `buildAiVaultResumeShellCommand`, :51-88).
///
/// POSIX only. The Windows branches (`cmd /d /s /c`, PowerShell `Set-Location`)
/// are measured in the ledger and not ported, because this product does not run
/// there yet and a half-right quoting rule on a shell we cannot test is a
/// command that damages a directory.
pub fn resume_command(session: &VaultSession) -> Option<String> {
    resume_command_with_base(session, None)
}

/// The same line, opened with a caller-resolved base — the launch words plus
/// the plan's own args, Orca's `commandOverride` road. `None` keeps the
/// measured default base. The base is a caller's job because it is a
/// SETTINGS fact (the person's saved launch args), and this module reads no
/// settings.
pub fn resume_command_with_base(session: &VaultSession, base: Option<&str>) -> Option<String> {
    if session.depth > 0 || session.parent_id.is_some() {
        return None;
    }
    let slug = session.agent.as_str();
    // `omp` addresses a session by transcript path; everyone else by id.
    let target = if slug == "omp" {
        session.file_path.clone()
    } else {
        session.session_id.clone()
    };
    if target.trim().is_empty() {
        return None;
    }
    let fallback = resume_base(slug);
    let base = match base {
        Some(said) if !said.trim().is_empty() => said.trim(),
        _ => fallback,
    };
    let invocation = resume_invocation(slug, base, &shell_quote(&target))?;
    let prefixed = match &session.codex_home {
        Some(home) if !home.trim().is_empty() => {
            format!("CODEX_HOME={} {invocation}", shell_quote(home.trim()))
        }
        _ => invocation,
    };
    Some(match &session.cwd {
        Some(cwd) if !cwd.trim().is_empty() => {
            format!("cd {} && {prefixed}", shell_quote(cwd))
        }
        _ => prefixed,
    })
}

/// Where a reopened session's shell should stand.
///
/// Orca resolves this from the workspace a card was DROPPED on
/// (`getAiVaultResumeWorkspacePath`, ai-vault-session-drag:18458) and only
/// falls back to the active one when nothing was dropped
/// (`targetWorktreeId = worktreeId ?? state.activeWorktreeId`, :17603). So the
/// drop target outranks the directory the conversation actually ran in, and
/// that order is the whole gesture: somebody who drags a session onto a
/// workspace is asking for it to continue THERE. Honouring `cwd` in that case
/// answers a question nobody asked, and the person finds out only after the
/// agent has started reading the wrong tree.
///
/// Every candidate has to be a directory that is still there, and one that is
/// not falls THROUGH to the next rather than failing: a worktree pruned since
/// the sidebar was painted, or a session directory deleted months ago, must
/// not turn "reopen this conversation" into an error dialog — there is always
/// somewhere for the window to stand.
///
/// `exists` is injected because the rule is worth testing without a
/// filesystem: the interesting cases here are about ORDER, and a test that
/// has to make three real directories to ask about order is a test that
/// stops being written.
pub fn resume_root(
    target_worktree: Option<&str>,
    session_cwd: Option<&str>,
    active_root: &str,
    exists: impl Fn(&str) -> bool,
) -> String {
    [target_worktree, session_cwd]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|candidate| !candidate.is_empty() && exists(candidate))
        .unwrap_or(active_root)
        .to_string()
}

/// One line of a session's conversation, for the card preview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewMessage {
    /// `user` or `assistant`. Only those two roles are shown
    /// (`CONVERSATION_ROLES`) — a tool result is not something a person said.
    pub role: String,
    pub text: String,
}

/// Recorded spawning context. Absent facts stay absent; vendor names are not
/// orchestration worker ids.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChildOrigin {
    #[serde(default)]
    pub pane: Option<String>,
    #[serde(default)]
    pub worker: Option<String>,
    #[serde(default)]
    pub tool_call_id: Option<String>,
}

/// A session as the panel shows it (`aiVaultSessionSchema`, index.js:138339-138372).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultSession {
    /// `<agent>:<session id>:<file path>` — unique across agents and stores, so
    /// the same conversation found through two roots is one card.
    pub id: String,
    pub agent: String,
    pub session_id: String,
    pub title: String,
    /// Where the agent was running. `None` for a store that does not record it,
    /// and the difference matters: a session with no `cwd` cannot be resumed
    /// into the right directory.
    pub cwd: Option<String>,
    pub branch: Option<String>,
    pub model: Option<String>,
    pub file_path: String,
    /// Set only for a Codex session found under a non-default home, which is
    /// what the resume has to put back in the environment.
    pub codex_home: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    /// The file's own mtime, as an RFC3339 string. Always present — it is the
    /// fallback both sorts land on.
    pub modified_at: String,
    pub message_count: usize,
    pub preview: Vec<PreviewMessage>,
    /// A command the user can copy. `None` when this agent cannot be resumed
    /// from here, which is a fact worth showing rather than a button that fails.
    pub resume: Option<String>,
    #[serde(default)]
    pub children: Vec<VaultSession>,
    #[serde(default)]
    pub depth: u8,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub origin: Option<ChildOrigin>,
}

impl VaultSession {
    /// Is there a conversation in here to come back to?
    /// (`isAiVaultSessionResumableContent`, :40-44)
    pub fn resumable(&self) -> bool {
        self.depth == 0
            && self.parent_id.is_none()
            && (self.message_count > 0
                || self
                    .preview
                    .iter()
                    .any(|line| line.role == "user" || line.role == "assistant"))
    }

    /// The time a sort uses: the requested field, falling back to the file's
    /// mtime (`compareSessions`, :211-217).
    pub fn sort_key(&self, sort: Sort) -> &str {
        let chosen = match sort {
            Sort::Created => self.created_at.as_deref(),
            Sort::Updated => self.updated_at.as_deref(),
        };
        chosen
            .filter(|value| !value.is_empty())
            .unwrap_or(&self.modified_at)
    }
}

/// Which timestamp orders the list (`isAiVaultSort`, :3745-3747).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Sort {
    /// Last touched. Orca's default (`DEFAULT_AI_VAULT_SORT`).
    #[default]
    Updated,
    Created,
}

/// What the list is divided by (`isAiVaultGroup`, :3748-3750).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Group {
    /// By the workspace this window manages that the conversation ran inside,
    /// named the way the sidebar names it — and by its directory when no
    /// workspace owns it. Default (`DEFAULT_AI_VAULT_GROUP`).
    ///
    /// This is Orca's `project`, and the fallback is the whole difference from
    /// [`Group::Folder`]: a person with three checkouts of one repository sees
    /// three headers here, each wearing the name they gave it, rather than three
    /// directories they have to read to tell apart.
    #[default]
    Project,
    /// By the directory itself, whoever owns it (`folder`).
    Folder,
    Agent,
}

/// A search string, split into a plain part and its two operators
/// (`parseVaultQuery`, :165-188).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Query {
    pub terms: Vec<String>,
    pub repo_terms: Vec<String>,
    pub path_terms: Vec<String>,
}

/// Split a query into tokens, honouring `repo:"two words"` and quoted phrases
/// (`tokenizeQuery`, :246-260).
///
/// Hand-written rather than a regex because the pattern has seven alternatives
/// and one of them decides whether the following text is an operator value —
/// which is the part a naive `split_whitespace` gets wrong, turning
/// `repo:"my project"` into a search for `project`.
pub fn parse_query(query: &str) -> Query {
    let mut parsed = Query::default();
    let bytes: Vec<char> = query.chars().collect();
    let mut at = 0usize;

    while at < bytes.len() {
        if bytes[at].is_whitespace() {
            at += 1;
            continue;
        }
        // An operator, possibly with a quoted value.
        let rest: String = bytes[at..].iter().collect();
        let operator = ["repo:", "path:"]
            .into_iter()
            .find(|name| rest.to_lowercase().starts_with(name));
        if let Some(operator) = operator {
            let after = at + operator.chars().count();
            let (value, next) = read_value(&bytes, after);
            let value = value.trim().to_lowercase();
            if !value.is_empty() {
                if operator == "repo:" {
                    parsed.repo_terms.push(value);
                } else {
                    parsed.path_terms.push(value);
                }
            }
            at = next;
            continue;
        }
        let (value, next) = read_value(&bytes, at);
        let value = value.trim().to_lowercase();
        if !value.is_empty() {
            parsed.terms.push(value);
        }
        at = next;
    }
    parsed
}

/// One token: a quoted run, or everything up to whitespace.
fn read_value(chars: &[char], from: usize) -> (String, usize) {
    let Some(&first) = chars.get(from) else {
        return (String::new(), from);
    };
    if first == '"' || first == '\'' {
        let mut value = String::new();
        let mut at = from + 1;
        while at < chars.len() && chars[at] != first {
            value.push(chars[at]);
            at += 1;
        }
        // A quote that never closes takes the rest, rather than the token being
        // dropped — a person mid-typing has an unclosed quote most of the time.
        return (value, (at + 1).min(chars.len()));
    }
    let mut value = String::new();
    let mut at = from;
    while at < chars.len() && !chars[at].is_whitespace() {
        value.push(chars[at]);
        at += 1;
    }
    (value, at)
}

/// Does this session match the query? (`matchesQuery`, :190-210)
///
/// Every plain term has to appear (AND, not OR) somewhere in the session's text.
/// `repo:` matches the group label and `path:` matches the directory and the
/// file — the two questions a person actually narrows by.
pub fn matches(session: &VaultSession, query: &Query) -> bool {
    if query.terms.is_empty() && query.repo_terms.is_empty() && query.path_terms.is_empty() {
        return true;
    }
    let mut haystack = String::new();
    for part in [
        Some(session.title.as_str()),
        Some(session.session_id.as_str()),
        Some(session.agent.as_str()),
        session.branch.as_deref(),
        session.model.as_deref(),
        session.cwd.as_deref(),
        Some(session.file_path.as_str()),
    ]
    .into_iter()
    .flatten()
    {
        haystack.push_str(part);
        haystack.push(' ');
    }
    for line in &session.preview {
        haystack.push_str(&line.text);
        haystack.push(' ');
    }
    let haystack = haystack.to_lowercase();
    if query.terms.iter().any(|term| !haystack.contains(term)) {
        return false;
    }
    let label = folder_label(session.cwd.as_deref()).to_lowercase();
    if query.repo_terms.iter().any(|term| !label.contains(term)) {
        return false;
    }
    let paths = format!(
        "{} {}",
        session.cwd.clone().unwrap_or_default(),
        session.file_path
    )
    .to_lowercase();
    if query.path_terms.iter().any(|term| !paths.contains(term)) {
        return false;
    }
    true
}

/// The header one session belongs under, and what that header says.
///
/// Three groupings, which is the original's set (`getGroupIdentity`) — and the
/// missing one was not a nicety: without [`Group::Project`] consulting the
/// workspaces this window manages, a person with three checkouts of one
/// repository got three headers naming three directories, while the sidebar
/// beside them named the same three checkouts. The usage pane already grouped
/// by workspace ([`crate::usage_stats::containing_worktree`]) and this asks it
/// the same question, so one conversation cannot be filed under a workspace in
/// one pane and under a bare directory in the other.
fn group_identity(
    session: &VaultSession,
    group: Group,
    worktrees: &[crate::usage_stats::WorktreeRef],
) -> (String, String) {
    if group == Group::Agent {
        return (session.agent.clone(), agent_label(&session.agent));
    }
    if group == Group::Project
        && let Some(cwd) = session.cwd.as_deref()
        && let Some(worktree) = crate::usage_stats::containing_worktree(cwd, worktrees)
    {
        return (
            format!("worktree:{}", worktree.worktree_id),
            worktree.display_name.clone(),
        );
    }
    (
        folder_group_key(session.cwd.as_deref()),
        folder_label(session.cwd.as_deref()),
    )
}

/// The one directory two spellings of it both name.
///
/// Separators normalised and a trailing one dropped, because a `cwd` is copied
/// verbatim out of a transcript and the same folder arrives with and without
/// the slash — each spelling otherwise its own group under an identical header.
///
/// Case is KEPT, which the original also insists on: lower-casing merges
/// `/w/App` and `/w/app`, and on the platforms where those are two directories
/// that is two projects' conversations in one pile. (It was lower-cased here
/// until this was measured against the original's own note.)
#[must_use]
pub fn folder_group_key(cwd: Option<&str>) -> String {
    let Some(cwd) = cwd.map(|cwd| cwd.replace('\\', "/")) else {
        return "unknown".to_string();
    };
    let trimmed = cwd.trim_end_matches('/');
    if trimmed.is_empty() {
        // `/` is the root, not a blank: it is a directory sessions can run in.
        return if cwd.is_empty() {
            "unknown".to_string()
        } else {
            "/".to_string()
        };
    }
    trimmed.to_string()
}

/// What a group of one agent's sessions is called.
///
/// The source's own label first — Orca's vault spells several agents
/// differently there than the launcher does (`pi` beside `Pi`), and those are
/// its words, not ours to normalise. OpenCode
/// has no source row to carry one, so the agent catalogue answers for it, which
/// is the same word the original's vault uses (`AI_VAULT_AGENT_LABELS`).
#[must_use]
pub fn agent_label(slug: &str) -> String {
    AGENT_SOURCES
        .iter()
        .find(|source| source.slug == slug)
        .map(|source| source.label.to_string())
        .or_else(|| crate::agent::agent_spec(slug).map(|spec| spec.name.to_string()))
        .unwrap_or_else(|| slug.to_string())
}

/// Every agent slug a card in this panel can wear.
///
/// The walked stores plus OpenCode, whose sessions are rows in a database and
/// so have no [`AgentSource`] to be found through. Anything that needs a fact
/// per agent — the person's saved launch words, for one — has to ask about all
/// of them, and this is the one place that knows the roster is wider than the
/// walk's.
pub fn shown_slugs() -> impl Iterator<Item = &'static str> {
    AGENT_SOURCES
        .iter()
        .map(|source| source.slug)
        .chain(std::iter::once(crate::vault_opencode::SLUG))
}

/// The last two path segments, which is how a directory is named in a header
/// (`folderLabel`, :153-163).
pub fn folder_label(path: Option<&str>) -> String {
    let Some(path) = path.filter(|value| !value.trim().is_empty()) else {
        return "알 수 없는 위치".to_string();
    };
    // The separator swap has to happen before the split, so the split works on
    // an owned string and the parts are owned with it.
    let normalized = path.replace('\\', "/");
    let parts: Vec<&str> = normalized
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    match parts.len() {
        0 => path.to_string(),
        1 => parts[0].to_string(),
        n => format!("{}/{}", parts[n - 2], parts[n - 1]),
    }
}

/// What to show: the search, the agents switched off, and the two orderings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct VaultQuery {
    #[serde(default)]
    pub query: String,
    /// Agent slugs to leave out. Orca stores the disabled ones rather than the
    /// enabled ones, so a newly supported agent appears without the user having
    /// to go and switch it on.
    #[serde(default)]
    pub disabled_agents: Vec<String>,
    #[serde(default)]
    pub sort: Sort,
    #[serde(default)]
    pub group: Group,
    /// Hide the sessions with nothing in them
    /// (`DEFAULT_AI_VAULT_HIDE_EMPTY_SESSIONS` is false).
    #[serde(default)]
    pub hide_empty: bool,
    /// Only sessions under one of these directories. Empty means everywhere.
    #[serde(default)]
    pub scope_paths: Vec<String>,
    /// Zero asks for all sessions up to the absolute cap.
    #[serde(default)]
    pub limit: Option<usize>,
}

/// One header and the sessions under it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VaultGroup {
    pub key: String,
    pub label: String,
    pub sessions: Vec<VaultSession>,
}

/// Something the scan could not do, named so the panel can say it
/// (`aiVaultScanIssueSchema`, index.js:138374-138384).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanIssue {
    pub agent: String,
    pub path: String,
    /// A token the window translates, not a sentence — the same rule as the hook
    /// rows. `unread-format` and `unreadable` are the two.
    pub reason: String,
    /// How many files were found and skipped, for `unread-format`.
    #[serde(default)]
    pub count: usize,
}

/// Combines scan issues that share the same agent and reason into a single issue
/// with the summed count.
#[must_use]
pub fn fold_issues(issues: Vec<ScanIssue>) -> Vec<ScanIssue> {
    let mut folded: BTreeMap<(String, String), (usize, String)> = BTreeMap::new();
    for issue in issues {
        let entry = folded
            .entry((issue.agent, issue.reason))
            .or_insert_with(|| (0, issue.path));
        entry.0 += issue.count.max(1);
    }
    folded
        .into_iter()
        .map(|((agent, reason), (count, path))| ScanIssue {
            agent,
            path,
            reason,
            count,
        })
        .collect()
}

/// What the panel draws.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VaultView {
    pub groups: Vec<VaultGroup>,
    /// Every agent that has a session here, whether or not it is switched on —
    /// the filter's own list. Counted BEFORE the filter, because a list that
    /// loses the row you just switched off is a list you cannot switch back on.
    pub agents: Vec<VaultAgentRow>,
    pub issues: Vec<ScanIssue>,
    /// After filtering.
    pub shown: usize,
    /// Before it, so "3 of 180" can be said.
    pub total: usize,
    /// After filters, before the view limit.
    pub matched: usize,
    pub limits: Limits,
    /// True when the walk hit [`MAX_CANDIDATE_FILES`], so the list is a recent
    /// slice rather than everything. A truncated list that says it is complete
    /// is the failure this flag exists to prevent.
    pub truncated: bool,
}

/// One agent in the filter list, with how many sessions it brought.
///
/// The count is ours rather than the original's — its list is the whole
/// seventeen-agent roster with a checkbox each, most of them empty on any real
/// machine. Naming only the agents that HAVE sessions, with their number, is
/// the same control with the noise removed: switching off an agent that brought
/// nothing changes nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VaultAgentRow {
    pub slug: String,
    pub label: String,
    pub sessions: usize,
}

/// The agents these sessions came from, most sessions first.
///
/// Ties broken by label so the list does not reorder itself between two scans
/// of the same machine.
#[must_use]
fn agent_rows(sessions: &[VaultSession]) -> Vec<VaultAgentRow> {
    let mut counted: BTreeMap<&str, usize> = BTreeMap::new();
    for session in sessions {
        *counted.entry(session.agent.as_str()).or_default() += 1;
    }
    let mut rows: Vec<VaultAgentRow> = counted
        .into_iter()
        .map(|(slug, sessions)| VaultAgentRow {
            slug: slug.to_string(),
            label: agent_label(slug),
            sessions,
        })
        .collect();
    rows.sort_by(|left, right| {
        right
            .sessions
            .cmp(&left.sessions)
            .then_with(|| left.label.cmp(&right.label))
    });
    rows
}

/// Filter, sort and group — the whole panel's decision, in one place.
///
/// Rust rather than the window for the same reason the board's columns are: the
/// header counts and the rows under them come from one function, so they cannot
/// disagree.
pub fn view(
    sessions: Vec<VaultSession>,
    query: &VaultQuery,
    issues: Vec<ScanIssue>,
    truncated: bool,
    worktrees: &[crate::usage_stats::WorktreeRef],
) -> VaultView {
    let total = sessions.len();
    let agents = agent_rows(&sessions);
    let parsed = parse_query(&query.query);
    let mut kept: Vec<VaultSession> = sessions
        .into_iter()
        .filter(|session| {
            !query
                .disabled_agents
                .iter()
                .any(|off| off == &session.agent)
        })
        .filter(|session| !query.hide_empty || session.resumable())
        .filter(|session| {
            if query.scope_paths.is_empty() {
                return true;
            }
            let Some(cwd) = session.cwd.as_deref() else {
                return false;
            };
            query
                .scope_paths
                .iter()
                .any(|scope| inside_or_equal(scope, cwd))
        })
        .filter(|session| matches(session, &parsed))
        .collect();

    // Newest first, and the id breaks a tie so the order does not wobble between
    // two scans of the same second.
    kept.sort_by(|left, right| {
        right
            .sort_key(query.sort)
            .cmp(left.sort_key(query.sort))
            .then_with(|| left.id.cmp(&right.id))
    });
    let matched = kept.len();
    kept.truncate(Limits::DEFAULT.session_limit(query.limit));
    let shown = kept.len();
    let truncated = truncated || shown < matched;

    // Insertion order is the sorted order, so the first group is the one holding
    // the newest session — which is what a reader expects at the top.
    let mut order: Vec<String> = Vec::new();
    let mut groups: BTreeMap<String, VaultGroup> = BTreeMap::new();
    for session in kept {
        let (key, label) = group_identity(&session, query.group, worktrees);
        if !groups.contains_key(&key) {
            order.push(key.clone());
            groups.insert(
                key.clone(),
                VaultGroup {
                    key: key.clone(),
                    label,
                    sessions: Vec::new(),
                },
            );
        }
        groups
            .get_mut(&key)
            .expect("the group was just inserted")
            .sessions
            .push(session);
    }

    let mut ordered: Vec<VaultGroup> = order
        .into_iter()
        .filter_map(|key| groups.remove(&key))
        .collect();
    widen_repeated_labels(&mut ordered);

    VaultView {
        groups: ordered,
        agents,
        issues: fold_issues(issues),
        shown,
        total,
        matched,
        limits: Limits::DEFAULT,
        truncated,
    }
}

/// Give two groups that would wear the same header enough path to tell apart.
///
/// Orca does not do this, and on a real machine it shows: a bench run with
/// `…/implement-median/claude-code/t1` and `…/debug-blank-line/claude-code/t1`
/// draws two adjacent headers both reading `claude-code/t1`, which looks like the
/// list repeated itself. One more segment each is enough almost always; the full
/// path is the fallback, because a header that is long is still better than two
/// headers that are the same.
fn widen_repeated_labels(groups: &mut [VaultGroup]) {
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for group in groups.iter() {
        *seen.entry(group.label.clone()).or_default() += 1;
    }
    let repeated: Vec<String> = seen
        .into_iter()
        .filter(|(_, count)| *count > 1)
        .map(|(label, _)| label)
        .collect();
    if repeated.is_empty() {
        return;
    }
    for group in groups.iter_mut() {
        if !repeated.contains(&group.label) {
            continue;
        }
        // The key is a directory for a folder group (and for a project group
        // that fell back to one), the workspace id for a named workspace, and
        // the slug for an agent. Only a path can be widened — an agent slug
        // never repeats, and two workspaces wearing one name are two rows the
        // sidebar itself spells apart.
        let normalized = group.key.replace('\\', "/");
        let parts: Vec<&str> = normalized
            .split('/')
            .filter(|part| !part.is_empty())
            .collect();
        group.label = if parts.len() > 2 {
            parts[parts.len() - 3..].join("/")
        } else {
            group.key.clone()
        };
    }
}

/// Is `candidate` the same directory as `root`, or under it?
///
/// Segment-wise, so `/a/bc` is not read as inside `/a/b` — the prefix test that
/// would be, and the reason a scope filter has to be written rather than assumed.
pub fn inside_or_equal(root: &str, candidate: &str) -> bool {
    let split = |value: &str| -> Vec<String> {
        value
            .replace('\\', "/")
            .split('/')
            .filter(|part| !part.is_empty())
            .map(|part| part.to_string())
            .collect()
    };
    let root = split(root);
    let candidate = split(candidate);
    root.len() <= candidate.len() && root.iter().zip(&candidate).all(|(one, two)| one == two)
}

// ------------------------------------------------------------------ the scan

/// The sessions directory this machine's environment names, if it names one.
///
/// The first variable in [`AgentSource::homes`] that is set decides, with its
/// own tail joined on; the rest are not read. Split out from [`roots`] so the
/// path arithmetic stays a pure function — the first version read the variable
/// inside `roots`, which made every test of it mutate a process-global, and two
/// such tests running in parallel flake each other, which is how this was
/// found.
pub fn env_home(source: &AgentSource) -> Option<PathBuf> {
    named_home(
        source,
        |name| std::env::var(name).ok(),
        std::env::var_os("HOME").map(PathBuf::from).as_deref(),
    )
}

/// The same answer, from a reader a test can supply.
///
/// The environment is passed in for the reason the split above exists: a test
/// that sets a process-global flakes whatever runs beside it.
pub fn named_home(
    source: &AgentSource,
    read: impl Fn(&str) -> Option<String>,
    home: Option<&Path>,
) -> Option<PathBuf> {
    source.homes.iter().find_map(|held| {
        let mut path = configured_dir(&read(held.name)?, home)?;
        for tail in held.tail {
            path.push(tail);
        }
        Some(path)
    })
}

/// A directory an environment variable named, or `None` when it named nothing
/// usable.
///
/// Two rules, both the original's (`absoluteConfiguredDir`). A leading `~` is
/// expanded, because the CLI expands it itself and a value set outside a shell
/// — a config file, a plist, a quoted assignment — still means the home
/// directory. And a RELATIVE value is refused rather than resolved: `sessions`
/// or `.` would resolve against whatever directory this process happens to be
/// in, which is a root nobody chose and a vault that reads as empty or, worse,
/// as somebody else's.
fn configured_dir(raw: &str, home: Option<&Path>) -> Option<PathBuf> {
    let trimmed = raw.trim();
    let expanded = match (trimmed.strip_prefix('~'), home) {
        (Some(""), Some(home)) => home.to_path_buf(),
        (Some(rest), Some(home)) if rest.starts_with(['/', '\\']) => home.join(&rest[1..]),
        _ => PathBuf::from(trimmed),
    };
    let text = expanded.to_str()?.trim_end_matches(['/', '\\']).to_string();
    let path = PathBuf::from(text);
    (!path.as_os_str().is_empty() && path.is_absolute()).then_some(path)
}

/// The roots one agent's sessions can be under.
///
/// `override_home` is what [`env_home`] found — already the full sessions
/// directory, tail and all. Both are looked at: a user who set `CODEX_HOME`
/// today may still have last month's sessions in `~/.codex`.
pub fn roots(source: &AgentSource, home: &Path, override_home: Option<&Path>) -> Vec<PathBuf> {
    let mut found = Vec::new();
    if let Some(named) = override_home.filter(|_| !source.homes.is_empty()) {
        found.push(named.to_path_buf());
    }
    let mut fallback = home.to_path_buf();
    for segment in source.segments {
        fallback.push(segment);
    }
    if !found.contains(&fallback) {
        found.push(fallback);
    }
    found
}

/// Does this file look like one of the agent's session files?
pub fn wanted(source: &AgentSource, path: &Path) -> bool {
    if path.extension().and_then(|value| value.to_str()) != Some(source.extension) {
        return false;
    }
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if let Some(want) = source.file_name
        && name != want
    {
        return false;
    }
    if let Some(prefix) = source.file_prefix
        && !name.starts_with(prefix)
    {
        return false;
    }
    // Zo rotates its plain log into `session-….rot-<ms>.jsonl` files that
    // repeat turns the live log still carries — the parser cannot tell a
    // rotation from the log it rotated (same lines), so the NAME refuses it
    // here, or every long conversation stands twice.
    if source.format == Format::ZoLines && name.contains(".rot-") {
        return false;
    }
    if let Some(segment) = source.path_segment
        && !path
            .components()
            .any(|part| part.as_os_str().to_str() == Some(segment))
    {
        return false;
    }
    true
}

/// A candidate file: where it is, whose it is, and when it changed.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Candidate {
    agent: &'static AgentSource,
    path: PathBuf,
    modified: std::time::SystemTime,
    /// Set for a Codex file found under a non-default home.
    codex_home: Option<String>,
}

/// Walk every agent's store and read what can be read.
///
/// Bounded three ways, and every bound is because these directories are not
/// ours: [`MAX_DEPTH`] on nesting, [`MAX_CANDIDATE_FILES`] per root, and
/// [`MAX_HEAD_BYTES`] per file. Symlinks are not followed — a link inside
/// somebody's session store pointing at `/` is not a hypothetical, it is what a
/// backup tool leaves behind.
pub fn scan(home: &Path, limit: usize) -> (Vec<VaultSession>, Vec<ScanIssue>, bool) {
    scan_with(home, limit, &env_home)
}

/// The scan, with the agent-home lookup handed in.
///
/// Exists so a test can point one agent somewhere without setting a variable for
/// the whole process.
pub fn scan_with(
    home: &Path,
    limit: usize,
    override_home: &dyn Fn(&AgentSource) -> Option<PathBuf>,
) -> (Vec<VaultSession>, Vec<ScanIssue>, bool) {
    let mut candidates: Vec<Candidate> = Vec::new();
    let mut issues: Vec<ScanIssue> = Vec::new();
    let mut truncated = false;

    for source in &AGENT_SOURCES {
        let mut unread = 0usize;
        let mut unread_at: Option<PathBuf> = None;
        for root in roots(source, home, override_home(source).as_deref()) {
            if !root.is_dir() {
                continue;
            }
            let mut found: Vec<PathBuf> = Vec::new();
            walk(&root, source, 0, &mut found, &mut truncated);
            for path in found {
                // An agent we can find but not read is counted, not faked.
                if source.format == Format::Unread {
                    unread += 1;
                    unread_at = unread_at.or_else(|| Some(root.clone()));
                    continue;
                }
                let Ok(meta) = std::fs::metadata(&path) else {
                    continue;
                };
                let modified = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
                let codex_home = if source.slug == "codex" {
                    codex_home_of(&path, home)
                } else {
                    None
                };
                candidates.push(Candidate {
                    agent: source,
                    path,
                    modified,
                    codex_home,
                });
            }
        }
        if unread > 0 {
            issues.push(ScanIssue {
                agent: source.slug.to_string(),
                path: unread_at
                    .map(|path| path.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                reason: "unread-format".to_string(),
                count: unread,
            });
        }
    }

    // A zo conversation may stand in two files at once — the plain log and
    // its distilled `.vault.jsonl` twin (measured coexisting, 2026-08-19).
    // The vault wins, and it wins HERE rather than at the id dedup below:
    // a card's id embeds the file path, so the two shapes never collide
    // there, and the choice must be made before the limit can truncate one
    // twin and keep the other.
    let vaulted: std::collections::HashSet<PathBuf> = candidates
        .iter()
        .filter(|candidate| candidate.agent.format == Format::ZoLines)
        .filter_map(|candidate| {
            let name = candidate.path.file_name()?.to_str()?;
            let stem = name.strip_suffix(".vault.jsonl")?;
            Some(candidate.path.with_file_name(format!("{stem}.jsonl")))
        })
        .collect();
    candidates.retain(|candidate| {
        candidate.agent.format != Format::ZoLines || !vaulted.contains(&candidate.path)
    });

    // Newest file first, then parse only as far down as the limit needs. The
    // multiplier is slack for the files that turn out to hold no session.
    candidates.sort_by(|left, right| right.modified.cmp(&left.modified));
    candidates.dedup_by(|left, right| left.path == right.path);
    let candidate_limit = Limits::DEFAULT
        .absolute_max
        .saturating_mul(CANDIDATE_MULTIPLIER);
    truncated |= candidates.len() > candidate_limit;
    candidates.truncate(candidate_limit);

    let mut sessions: Vec<VaultSession> = Vec::new();
    for candidate in candidates {
        match read_session(&candidate) {
            Ok(Some(session)) => sessions.push(session),
            Ok(None) => {}
            Err(why) => issues.push(ScanIssue {
                agent: candidate.agent.slug.to_string(),
                path: candidate.path.to_string_lossy().into_owned(),
                reason: why,
                count: 1,
            }),
        }
    }

    // The same conversation reached through two roots is one card.
    sessions.sort_by(|left, right| left.id.cmp(&right.id));
    sessions.dedup_by(|left, right| left.id == right.id);
    attach_children(&mut sessions, &mut issues);
    sessions.sort_by(|left, right| {
        right
            .modified_at
            .cmp(&left.modified_at)
            .then_with(|| left.id.cmp(&right.id))
    });
    let limit = limit.min(Limits::DEFAULT.absolute_max);
    truncated |= sessions.len() > limit;
    sessions.truncate(limit);
    (sessions, fold_issues(issues), truncated)
}

/// A child transcript named by the session-owned roster snapshot.
#[derive(Debug, Clone)]
pub struct ChildTranscript {
    pub id: String,
    pub parent: String,
    pub path: PathBuf,
    pub origin: ChildOrigin,
}

/// Read roster transcripts only while constructing a walk. The live query
/// receives the kept result and never visits these paths again.
pub fn read_roster_children(
    sessions: &mut Vec<VaultSession>,
    roster: &[ChildTranscript],
    issues: &mut Vec<ScanIssue>,
) {
    let source = AGENT_SOURCES
        .iter()
        .find(|source| source.slug == "zo")
        .expect("zo source");
    let mut counts = BTreeMap::new();
    for child in roster {
        let count = counts.entry(&child.parent).or_insert(0);
        if *count >= Limits::DEFAULT.children_per_parent {
            issues.push(ScanIssue {
                agent: "zo".into(),
                path: child.path.to_string_lossy().into_owned(),
                reason: "child-limit".into(),
                count: 1,
            });
            continue;
        }
        *count += 1;
        let result = std::fs::symlink_metadata(&child.path)
            .ok()
            .filter(|meta| meta.is_file())
            .ok_or_else(|| "unreadable".to_string())
            .and_then(|meta| {
                read_session(&Candidate {
                    agent: source,
                    path: child.path.clone(),
                    modified: meta.modified().unwrap_or(std::time::UNIX_EPOCH),
                    codex_home: None,
                })
            });
        match result {
            Ok(Some(mut row)) => {
                row.session_id = child.id.clone();
                row.id = format!("zo:{}:{}", child.id, row.file_path);
                row.parent_id = Some(child.parent.clone());
                row.depth = 1;
                row.origin = Some(child.origin.clone());
                child_preview(&mut row);
                sessions.push(row);
            }
            _ => issues.push(ScanIssue {
                agent: "zo".into(),
                path: child.path.to_string_lossy().into_owned(),
                reason: "unreadable".into(),
                count: 1,
            }),
        }
    }
    attach_children(sessions, issues);
}

fn child_preview(child: &mut VaultSession) {
    child.resume = None;
    child.preview.truncate(1);
    if let Some(line) = child.preview.first_mut() {
        line.text = line.text.lines().next().unwrap_or_default().to_string();
    }
}

fn attach_children(sessions: &mut Vec<VaultSession>, issues: &mut Vec<ScanIssue>) {
    let mut children = Vec::new();
    let mut parents = Vec::new();
    for row in sessions.drain(..) {
        if row.depth > 0 || row.parent_id.is_some() {
            children.push(row);
        } else {
            parents.push(row);
        }
    }
    let child_ids: std::collections::HashSet<(String, String)> = children
        .iter()
        .map(|child| (child.agent.clone(), child.session_id.clone()))
        .collect();
    children.sort_by(|left, right| {
        right
            .modified_at
            .cmp(&left.modified_at)
            .then_with(|| left.id.cmp(&right.id))
    });
    for mut child in children {
        let owner = child.parent_id.as_deref().unwrap_or_default();
        let mut matching = parents.iter_mut().filter(|parent| {
            parent.agent == child.agent
                && if child.agent == "claude" {
                    parent.file_path == owner
                } else {
                    parent.session_id == owner && parent.codex_home == child.codex_home
                }
        });
        let first = matching.next();
        let reason = if matching.next().is_some() {
            Some("child-parent-ambiguous")
        } else if let Some(parent) = first {
            if parent.children.iter().any(|held| held.id == child.id) {
                continue;
            }
            if parent.children.len() >= Limits::DEFAULT.children_per_parent {
                Some("child-limit")
            } else {
                child.parent_id = Some(parent.id.clone());
                child.depth = 1;
                child_preview(&mut child);
                parent.children.push(child);
                continue;
            }
        } else if child_ids.contains(&(child.agent.clone(), owner.to_string())) {
            Some("child-depth")
        } else {
            Some("child-parent-missing")
        };
        if let Some(reason) = reason {
            issues.push(ScanIssue {
                agent: child.agent,
                path: child.file_path,
                reason: reason.into(),
                count: 1,
            });
        }
    }
    *sessions = parents;
}

/// One list out of two, newest first, cut back to the limit.
///
/// The panel has two readers — the walk above and OpenCode's database — and
/// each honours the limit alone. Their lists are therefore each already the
/// newest `limit` of their own store, so the newest `limit` of the two
/// together is in here somewhere and nothing older can be missing; merging
/// after the fact costs one sort and needs no reader to know about the other.
///
/// The stamps compare as text because both readers write them the same way
/// ([`crate::civil::iso_utc_of`] — fixed width, always UTC), which makes
/// alphabetical order and chronological order the same order.
///
/// Returns whether anything was dropped, which the header says out loud.
#[must_use]
pub fn merge(
    mut sessions: Vec<VaultSession>,
    extra: Vec<VaultSession>,
    limit: usize,
) -> (Vec<VaultSession>, bool) {
    let held: std::collections::HashSet<&str> =
        sessions.iter().map(|one| one.id.as_str()).collect();
    let mut fresh: Vec<VaultSession> = extra
        .into_iter()
        .filter(|one| !held.contains(one.id.as_str()))
        .collect();
    sessions.append(&mut fresh);
    sessions.sort_by(|left, right| {
        right
            .modified_at
            .cmp(&left.modified_at)
            .then_with(|| left.id.cmp(&right.id))
    });
    let dropped = sessions.len() > limit;
    sessions.truncate(limit);
    (sessions, dropped)
}

/// The `CODEX_HOME` a session file implies, when it is not the default one.
///
/// `<home>/sessions/<y>/<m>/<d>/rollout-….jsonl` — so the home is the parent of
/// the `sessions` directory. Reported only when it differs from `~/.codex`,
/// because a resume that sets `CODEX_HOME` to the default value would override a
/// variable the user had set on purpose.
fn codex_home_of(path: &Path, home: &Path) -> Option<String> {
    let mut at = path.parent();
    while let Some(dir) = at {
        if dir.file_name().and_then(|name| name.to_str()) == Some("sessions") {
            let found = dir.parent()?;
            if found == home.join(".codex") {
                return None;
            }
            return Some(found.to_string_lossy().into_owned());
        }
        at = dir.parent();
    }
    None
}

fn walk(
    dir: &Path,
    source: &AgentSource,
    depth: usize,
    found: &mut Vec<PathBuf>,
    truncated: &mut bool,
) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if found.len() >= MAX_CANDIDATE_FILES {
            *truncated = true;
            return;
        }
        let path = entry.path();
        // `symlink_metadata`, so a link is seen as a link and skipped rather
        // than followed into a loop or out of the store entirely.
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            walk(&path, source, depth + 1, found, truncated);
            continue;
        }
        if wanted(source, &path) {
            found.push(path);
        }
    }
}

/// The head of a file, as text, bounded.
fn read_head(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut buffer = vec![0u8; MAX_HEAD_BYTES];
    let read = file.read(&mut buffer)?;
    buffer.truncate(read);
    // Lossy, because a session file is a vendor's and may hold anything; a
    // panel that shows nothing because one byte was invalid UTF-8 is worse.
    Ok(String::from_utf8_lossy(&buffer).into_owned())
}

fn read_session(candidate: &Candidate) -> Result<Option<VaultSession>, String> {
    let text = read_head(&candidate.path).map_err(|_| "unreadable".to_string())?;
    let mut session = match candidate.agent.format {
        Format::ClaudeLines => claude_session(&text),
        Format::CodexRollout => codex_session(&text),
        Format::AntigravityTranscript => antigravity_session(&text),
        Format::ZoLines => zo_session(&text),
        Format::Unread => None,
    }
    .unwrap_or_default();
    // Antigravity's transcript does not say which conversation it is — the id
    // is the brain directory the file lives under
    // (`brain/<id>/.system_generated/logs/transcript.jsonl`), which is also
    // how Orca reads it (`antigravityConversationIdFromTranscriptPath`).
    if session.session_id.trim().is_empty()
        && candidate.agent.format == Format::AntigravityTranscript
        && let Some(id) = candidate
            .path
            .ancestors()
            .nth(3)
            .and_then(|dir| dir.file_name())
            .and_then(|name| name.to_str())
    {
        session.session_id = id.to_string();
    }
    // Zo's vault lines carry no id either — the file is named by it, with a
    // `.vault` tail the generic stem fallback below would keep. Stripped here
    // because the id becomes the resume argument, and `zo --resume
    // 'session-…-0.vault'` names a session that does not exist.
    if session.session_id.trim().is_empty()
        && candidate.agent.format == Format::ZoLines
        && let Some(id) = candidate
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_suffix(".vault.jsonl"))
    {
        session.session_id = id.to_string();
    }
    // The parser looked and said this FILE is not a session — different from
    // "could not read it". Listing it as a session would be a junk row named
    // after a file, on every visit, for every project.
    if session.not_a_session {
        return Ok(None);
    }

    if session.session_id.trim().is_empty() {
        // Fall back to the file's own name, which every store derives from the
        // id. A session we can address is worth showing even when its records
        // were not the shape we expected.
        session.session_id = candidate
            .path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default()
            .to_string();
    }
    if session.session_id.trim().is_empty() {
        return Ok(None);
    }

    if let Some(child_dir) = candidate.agent.child_dir
        && let Some(dir) = candidate
            .path
            .ancestors()
            .find(|dir| dir.file_name().and_then(|s| s.to_str()) == Some(child_dir))
    {
        session.parent_id = dir.parent().map(|parent| {
            parent
                .with_extension("jsonl")
                .to_string_lossy()
                .into_owned()
        });
        session.depth = 1;
        session.session_id = candidate
            .path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
    }
    Ok(Some(card(
        candidate.agent.slug,
        candidate.path.to_string_lossy().into_owned(),
        rfc3339(candidate.modified),
        candidate.codex_home.clone(),
        session,
    )))
}

/// The card, once a reader has said everything only it could know.
///
/// The last stretch of every read is the same wherever the session came from:
/// name the card, compose the id the panel dedups on, stand in for a missing
/// `updated_at`, and write the resume line. Two readers arrive here — the file
/// walk above and OpenCode's database ([`crate::vault_opencode`]) — and a
/// second copy of these thirty lines would be two kinds of row in one list:
/// one whose `<tag>`-wrapped title got peeled and one whose did not.
///
/// `modified_at` is the caller's because only the caller knows what "when was
/// this touched" means for its store — a file's mtime, or a row's own stamp.
#[must_use]
pub fn card(
    agent: &str,
    file_path: String,
    modified_at: String,
    codex_home: Option<String>,
    parsed: Parsed,
) -> VaultSession {
    let title = if parsed.title.trim().is_empty() {
        parsed
            .preview
            .iter()
            .filter(|line| line.role == "user")
            .find_map(|line| untagged_title(&line.text))
            .map(|words| clamp(&words))
            .unwrap_or_else(|| parsed.session_id.clone())
    } else {
        clamp(&parsed.title)
    };

    let mut built = VaultSession {
        id: format!("{agent}:{}:{file_path}", parsed.session_id),
        agent: agent.to_string(),
        session_id: parsed.session_id,
        title,
        cwd: parsed.cwd,
        branch: parsed.branch,
        model: parsed.model,
        file_path,
        codex_home,
        created_at: parsed.created_at,
        updated_at: parsed.updated_at.or_else(|| Some(modified_at.clone())),
        modified_at,
        message_count: parsed.message_count,
        preview: parsed.preview,
        resume: None,
        children: Vec::new(),
        depth: parsed.depth,
        parent_id: parsed.parent_id,
        origin: None,
    };
    if built.depth > 0 {
        child_preview(&mut built);
    }
    built.resume = resume_command(&built);
    built
}

/// The words a card may be NAMED by, out of one user message.
///
/// Wrapper tags a CLI prepended — `<local-command-caveat>…`,
/// `<teammate-message from=…>`, `<command-name>…` — are peeled off the front,
/// and what remains is the person's part; `None` when nothing readable is
/// left, so the caller can try the next user line. The PREVIEW keeps the raw
/// words: the tag there is honest context, and Orca's own list shows it
/// (실측 #11 — its preview lines carry `<codex_internal_context …`). Orca
/// also lets such a tag stand AS the title, and that is the one part not
/// followed: it was named broken in exactly these words — "세션 내용도
/// orca처럼 나와야하는데 태그가 붙어서 보임" — so the title alone is peeled
/// (기록된 개선, 1-g101).
fn untagged_title(words: &str) -> Option<String> {
    let mut rest = words.trim_start();
    while rest.starts_with('<') {
        let Some(end) = rest.find('>') else { break };
        rest = rest[end + 1..].trim_start();
    }
    // And the closing halves off the tail, so a message that is one whole
    // envelope names itself by its inside alone. Only `</…>` groups at the
    // very end — a `<` standing MID-sentence is often the person's own
    // ("a < b", `Vec<String>`), and mangling real words to hide a tag would
    // be the worse trade.
    let mut tail = rest.trim_end();
    while tail.ends_with('>') {
        let Some(open) = tail.rfind("</") else { break };
        if tail[open..].find('>') != Some(tail[open..].len() - 1) {
            break;
        }
        tail = tail[..open].trim_end();
    }
    // Still `<`-headed after the peel — an unclosed wrapper — is not a name;
    // the caller moves to the next user line, the retained scanner's own
    // stance on `<`-headed content.
    (!tail.is_empty() && !tail.starts_with('<')).then(|| tail.to_string())
}

/// What a parser gets out of one session, before the scan fills in the rest.
///
/// Public because a second reader fills it: OpenCode's sessions are rows in a
/// database rather than files in a directory
/// ([`crate::vault_opencode`]), and both roads have to end at [`card`] or the
/// panel shows two kinds of row.
#[derive(Debug, Default)]
pub struct Parsed {
    /// The store's own id for the conversation. Each store answers this
    /// differently — a field, a file name, a parent directory, a primary key —
    /// so [`card`] takes it from here rather than deciding.
    pub session_id: String,
    pub title: String,
    pub cwd: Option<String>,
    pub branch: Option<String>,
    pub model: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub message_count: usize,
    pub preview: Vec<PreviewMessage>,
    /// The parser read the file fine and it is NOT a session — skip it
    /// entirely, rather than falling back to a file-stem row. A database row
    /// is never this: it was found by asking for sessions.
    pub not_a_session: bool,
    pub parent_id: Option<String>,
    pub depth: u8,
}

/// How many conversation lines a card keeps.
pub const PREVIEW_LINES: usize = 3;

/// Claude's store: one JSON object per line.
///
/// Verified against `~/.claude/projects/*/*.jsonl` on a working machine. `cwd`,
/// `gitBranch` and `sessionId` ride on every message record; `message.content`
/// is a string for a user turn and an array of typed parts for an assistant's.
fn claude_session(text: &str) -> Option<Parsed> {
    let mut parsed = Parsed::default();
    for line in text.lines() {
        let Ok(record) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if parsed.session_id.is_empty()
            && let Some(id) = record.get("sessionId").and_then(|v| v.as_str())
        {
            parsed.session_id = id.to_string();
        }
        if parsed.cwd.is_none() {
            parsed.cwd = record
                .get("cwd")
                .and_then(|v| v.as_str())
                .map(str::to_string);
        }
        if parsed.branch.is_none() {
            parsed.branch = record
                .get("gitBranch")
                .and_then(|v| v.as_str())
                .filter(|value| !value.is_empty())
                .map(str::to_string);
        }
        let when = record.get("timestamp").and_then(|v| v.as_str());
        if let Some(when) = when {
            if parsed.created_at.is_none() {
                parsed.created_at = Some(when.to_string());
            }
            parsed.updated_at = Some(when.to_string());
        }

        let kind = record.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if kind != "user" && kind != "assistant" {
            continue;
        }
        let Some(message) = record.get("message") else {
            continue;
        };
        if parsed.model.is_none() {
            parsed.model = message
                .get("model")
                .and_then(|v| v.as_str())
                .map(str::to_string);
        }
        parsed.message_count += 1;
        if parsed.preview.len() < PREVIEW_LINES
            && let Some(text) = message_text(message.get("content"))
        {
            parsed.preview.push(PreviewMessage {
                role: kind.to_string(),
                text: clamp(&text),
            });
        }
    }
    Some(parsed)
}

/// Codex's rollout: one JSON object per line, the first a `session_meta`.
///
/// Verified against `~/.codex/sessions/**/rollout-*.jsonl`. The meta payload
/// holds the id, the `cwd` and the session's own start time — which is why a
/// Codex card can be right even when the rest of the file is a shape we do not
/// know.
fn codex_session(text: &str) -> Option<Parsed> {
    let mut parsed = Parsed::default();
    for line in text.lines() {
        let Ok(record) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let kind = record.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if let Some(when) = record.get("timestamp").and_then(|v| v.as_str()) {
            parsed.updated_at = Some(when.to_string());
        }
        if kind == "session_meta" {
            let payload = record.get("payload").unwrap_or(&serde_json::Value::Null);
            if let Some(id) = payload.get("id").and_then(|v| v.as_str()) {
                parsed.session_id = id.to_string();
            }
            parsed.cwd = payload
                .get("cwd")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            parsed.created_at = payload
                .get("timestamp")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            parsed.model = payload
                .get("model")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            if let Some(source) = payload.pointer("/source/subagent") {
                parsed.parent_id = Some(
                    payload
                        .get("parent_thread_id")
                        .or_else(|| source.pointer("/thread_spawn/parent_thread_id"))
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                );
                parsed.depth = 1;
            }
            continue;
        }
        // Codex wraps a turn as `{type:"response_item", payload:{type:"message",
        // role, content:[{text}]}}`. Only the two conversation roles count.
        let payload = record.get("payload").unwrap_or(&record);
        let role = payload.get("role").and_then(|v| v.as_str()).unwrap_or("");
        if role != "user" && role != "assistant" {
            continue;
        }
        parsed.message_count += 1;
        if parsed.preview.len() < PREVIEW_LINES
            && let Some(text) = message_text(payload.get("content"))
        {
            parsed.preview.push(PreviewMessage {
                role: role.to_string(),
                text: clamp(&text),
            });
        }
    }
    Some(parsed)
}

/// Zo's store, both of its shapes (re-measured whole, 2026-08-19: 1,157
/// plain logs across `~/.zo/projects/*/sessions`, every first line
/// `session_meta`, every turn type `message` or `compaction`; 47 vault
/// twins):
///
/// - the plain log `session-*.jsonl` — first line
///   `{"created_at_ms","session_id","type":"session_meta","updated_at_ms",
///   "version"}`, then `{"message":{"blocks":[…],"role":…},"turn_index",
///   "type":"message"}` turns whose roles run user/assistant/tool/system;
/// - the distilled `session-*.vault.jsonl` — `type:"vault"` lines carrying
///   the same `message.blocks` shape, no meta line.
///
/// Both turn kinds land in one loop because the message under them is one
/// shape. Only user and assistant turns count or preview — tool and system
/// turns are machinery, `compaction` is bookkeeping, and thinking/tool_use
/// blocks carry no `text` so [`message_text`] drops them on its own. A file
/// where nothing spoke — the meta-only log of a session that never got a
/// first word, 995 of the 1,157 — answers `not_a_session` instead of
/// standing as a junk row. The meta line hands the plain log its id and both
/// epoch-ms times; the vault twin has neither, so its id is shed from the
/// file name and its clock is the file's mtime (`read_session`'s fallbacks).
/// No shape says a `cwd` — the grouping falls back to where the file lives,
/// the stance other hash-only stores use.
fn zo_session(text: &str) -> Option<Parsed> {
    let mut parsed = Parsed::default();
    let mut spoke = false;
    for line in text.lines() {
        let Ok(record) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let kind = record.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if kind == "session_meta" {
            if let Some(id) = record.get("session_id").and_then(|v| v.as_str()) {
                parsed.session_id = id.to_string();
            }
            if let Some(ms) = record.get("created_at_ms").and_then(|v| v.as_u64()) {
                parsed.created_at = i64::try_from(ms).ok().map(crate::civil::iso_utc_of);
            }
            if let Some(ms) = record.get("updated_at_ms").and_then(|v| v.as_u64()) {
                parsed.updated_at = i64::try_from(ms).ok().map(crate::civil::iso_utc_of);
            }
            continue;
        }
        if kind != "vault" && kind != "message" {
            continue;
        }
        spoke = true;
        let Some(message) = record.get("message") else {
            continue;
        };
        let role = message.get("role").and_then(|v| v.as_str()).unwrap_or("");
        if role != "user" && role != "assistant" {
            continue;
        }
        parsed.message_count += 1;
        if parsed.preview.len() < PREVIEW_LINES
            && let Some(words) = message_text(message.get("blocks"))
        {
            parsed.preview.push(PreviewMessage {
                role: role.to_string(),
                text: clamp(&words),
            });
        }
    }
    if !spoke {
        parsed.not_a_session = true;
    }
    Some(parsed)
}

/// Antigravity's transcript: one JSON object per line.
///
/// Verified against
/// `~/.gemini/antigravity/brain/*/.system_generated/logs/transcript.jsonl` on
/// a working machine — 5 conversations. A line is a STEP, not a message:
/// `{step_index, source, type, status, created_at, content}` where most
/// steps are tool runs (`VIEW_FILE`, `GREP_SEARCH`, `MCP_TOOL`, …) and the
/// conversation is the two kinds this reads:
///
/// - `type: "USER_INPUT"` — the person's turn, with the actual request
///   wrapped in `<USER_REQUEST>…</USER_REQUEST>` between metadata blocks the
///   CLI added for its own model;
/// - `type: "PLANNER_RESPONSE", source: "MODEL"` — the assistant's turn
///   (every planner line in every real file carries `source: "MODEL"`, and
///   Orca's own extractor requires the pair, out/main/index.js — the
///   `record.source === "MODEL" && record.type === "PLANNER_RESPONSE"`
///   branch of its assistant reader).
///
/// The conversation's ID is not in the file — it is the `brain/<id>/`
/// directory, filled in by the scan from the path.
fn antigravity_session(text: &str) -> Option<Parsed> {
    let mut parsed = Parsed::default();
    for line in text.lines() {
        let Ok(record) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if let Some(when) = record.get("created_at").and_then(|v| v.as_str()) {
            if parsed.created_at.is_none() {
                parsed.created_at = Some(when.to_string());
            }
            parsed.updated_at = Some(when.to_string());
        }
        let kind = record.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let role = match kind {
            "USER_INPUT" => "user",
            "PLANNER_RESPONSE"
                if record.get("source").and_then(|v| v.as_str()) == Some("MODEL") =>
            {
                "assistant"
            }
            _ => continue,
        };
        parsed.message_count += 1;
        let Some(words) = record
            .get("content")
            .and_then(|v| v.as_str())
            .map(|content| {
                if role == "user" {
                    user_request_in(content)
                } else {
                    content.to_string()
                }
            })
            .filter(|words| !words.trim().is_empty())
        else {
            continue;
        };
        if role == "user" && parsed.title.is_empty() {
            parsed.title = clamp(&words);
        }
        if parsed.preview.len() < PREVIEW_LINES {
            parsed.preview.push(PreviewMessage {
                role: role.to_string(),
                text: clamp(&words),
            });
        }
    }
    Some(parsed)
}

/// The person's words out of a `USER_INPUT` step's content.
///
/// The CLI wraps the actual request in `<USER_REQUEST>…</USER_REQUEST>` and
/// appends metadata blocks of its own (`<ADDITIONAL_METADATA>`,
/// `<USER_SETTINGS_CHANGE>`) — a title taken from the whole content would
/// begin with the request and end in boilerplate about the local time. A
/// content with no wrapper is used whole: the wrapper is the CLI's habit,
/// not a promise.
fn user_request_in(content: &str) -> String {
    let inner = content
        .split_once("<USER_REQUEST>")
        .and_then(|(_, rest)| rest.split_once("</USER_REQUEST>"))
        .map(|(inner, _)| inner);
    inner.unwrap_or(content).trim().to_string()
}

/// A system time as an RFC3339 string.
///
/// The formatting is [`crate::civil::iso_utc_of`]'s, which is where this
/// tree's one millisecond-to-stamp conversion lives — three hand-rolled
/// copies of it agreed by luck rather than by design.
fn rfc3339(when: std::time::SystemTime) -> String {
    let millis = when
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|gap| i64::try_from(gap.as_millis()).ok())
        .unwrap_or(0);
    crate::civil::iso_utc_of(millis)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(agent: &str, id: &str, cwd: Option<&str>, when: &str) -> VaultSession {
        VaultSession {
            id: format!("{agent}:{id}:/f/{id}.jsonl"),
            agent: agent.to_string(),
            session_id: id.to_string(),
            title: format!("{id} title"),
            cwd: cwd.map(str::to_string),
            branch: None,
            model: None,
            file_path: format!("/f/{id}.jsonl"),
            codex_home: None,
            created_at: Some(when.to_string()),
            updated_at: Some(when.to_string()),
            modified_at: when.to_string(),
            message_count: 2,
            preview: vec![PreviewMessage {
                role: "user".into(),
                text: "make the tests pass".into(),
            }],
            resume: None,
            children: Vec::new(),
            depth: 0,
            parent_id: None,
            origin: None,
        }
    }

    #[test]
    fn child_cap_and_nested_children_report_why_they_are_not_shown() {
        let mut rows = vec![session("codex", "parent", None, "now")];
        for n in 0..=Limits::DEFAULT.children_per_parent {
            let mut child = session("codex", &format!("child-{n}"), None, "now");
            child.parent_id = Some("parent".into());
            child.depth = 1;
            rows.push(child);
        }
        let mut nested = session("codex", "nested", None, "now");
        nested.parent_id = Some("child-0".into());
        nested.depth = 1;
        rows.push(nested);
        let mut issues = Vec::new();
        attach_children(&mut rows, &mut issues);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].children.len(), Limits::DEFAULT.children_per_parent);
        assert!(issues.iter().any(|issue| issue.reason == "child-limit"));
        assert!(issues.iter().any(|issue| issue.reason == "child-depth"));
    }

    #[test]
    fn old_session_json_and_limits_overlay_remain_compatible() {
        let mut json = serde_json::to_value(session("claude", "old", None, "now")).unwrap();
        for key in ["children", "depth", "parent_id", "origin"] {
            json.as_object_mut().unwrap().remove(key);
        }
        let old: VaultSession = serde_json::from_value(json).unwrap();
        assert!(old.children.is_empty());
        assert_eq!(old.depth, 0);
        assert_eq!(old.parent_id, None);
        assert_eq!(old.origin, None);
        for limit in Limits::DEFAULT.choices.into_iter().chain([0]) {
            let overlay = BTreeMap::from([("vault.sessionLimit".into(), limit)]);
            assert_eq!(
                Limits::DEFAULT.overlaid(&overlay).default,
                Limits::DEFAULT.session_limit(Some(limit))
            );
        }
    }

    #[test]
    fn roster_children_keep_recorded_origin_and_one_preview_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("helper.session.jsonl");
        std::fs::write(&path, r#"{"type":"message","message":{"role":"assistant","blocks":[{"type":"text","text":"preview"}]}}"#).unwrap();
        let origin = ChildOrigin {
            pane: Some("term-12".into()),
            worker: Some("w-34".into()),
            tool_call_id: Some("call-56".into()),
        };
        let roster = vec![ChildTranscript {
            id: "helper".into(),
            parent: "parent".into(),
            path,
            origin: origin.clone(),
        }];
        let mut rows = vec![session("zo", "parent", None, "now")];
        let mut issues = Vec::new();
        read_roster_children(&mut rows, &roster, &mut issues);
        assert!(issues.is_empty(), "{issues:?}");
        let child = &rows[0].children[0];
        assert_eq!(child.origin, Some(origin));
        assert_eq!(child.preview.len(), 1);
        assert!(!child.resumable());
        assert_eq!(resume_command(child), None);
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn subagent_rows_attach_to_their_parent_without_changing_its_resume() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".claude/projects/project");
        std::fs::create_dir_all(root.join("parent/subagents")).unwrap();
        std::fs::write(root.join("parent.jsonl"),
            r#"{"type":"user","sessionId":"parent","message":{"role":"user","content":"parent ask"}}"#).unwrap();
        std::fs::write(root.join("parent/subagents/agent-child.jsonl"),
            concat!(r#"{"type":"user","sessionId":"parent","message":{"role":"user","content":"child ask"}}"#, "\n",
            r#"{"type":"assistant","sessionId":"parent","message":{"role":"assistant","content":"child answer"}}"#)).unwrap();
        let (rows, issues, _) = scan_with(dir.path(), DEFAULT_SCAN_LIMIT, &|_| None);
        assert!(issues.is_empty(), "{issues:?}");
        assert_eq!(rows.len(), 1);
        let json = serde_json::to_value(&rows[0]).unwrap();
        assert_eq!(json["children"].as_array().map(Vec::len), Some(1));
        assert_eq!(json["children"][0]["depth"], 1);
        assert_eq!(json["children"][0]["parent_id"], rows[0].id);
        assert_eq!(
            json["children"][0]["preview"].as_array().map(Vec::len),
            Some(1)
        );
        assert!(json["children"][0]["resume"].is_null());
        assert_eq!(rows[0].message_count, 1);
        assert!(rows[0].resumable());
    }

    #[test]
    fn orphan_subagents_are_reported_instead_of_becoming_parent_cards() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir
            .path()
            .join(".claude/projects/project/missing/subagents");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("agent-child.jsonl"),
            r#"{"type":"user","sessionId":"missing","message":{"role":"user","content":"child ask"}}"#).unwrap();
        let (rows, issues, _) = scan_with(dir.path(), DEFAULT_SCAN_LIMIT, &|_| None);
        assert!(rows.is_empty());
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].reason, "child-parent-missing");
    }

    #[test]
    fn scan_issues_are_folded_by_agent_and_reason() {
        let raw = vec![
            ScanIssue {
                agent: "codex".into(),
                path: "/path/1".into(),
                reason: "child-parent-missing".into(),
                count: 1,
            },
            ScanIssue {
                agent: "codex".into(),
                path: "/path/2".into(),
                reason: "child-parent-missing".into(),
                count: 1,
            },
            ScanIssue {
                agent: "claude".into(),
                path: "/path/3".into(),
                reason: "child-limit".into(),
                count: 1,
            },
        ];
        let folded = fold_issues(raw);
        assert_eq!(folded.len(), 2);
        let codex = folded.iter().find(|i| i.agent == "codex").unwrap();
        assert_eq!(codex.reason, "child-parent-missing");
        assert_eq!(codex.count, 2);
        let claude = folded.iter().find(|i| i.agent == "claude").unwrap();
        assert_eq!(claude.reason, "child-limit");
        assert_eq!(claude.count, 1);
    }

    #[test]
    fn measured_codex_rollout_metadata_names_the_parent() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".codex/sessions/2026/09/05");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("rollout-parent.jsonl"),
            r#"{"type":"session_meta","payload":{"id":"parent","source":"cli"}}"#,
        )
        .unwrap();
        std::fs::write(root.join("rollout-child.jsonl"),
            r#"{"type":"session_meta","payload":{"id":"child","source":{"subagent":{"thread_spawn":{"parent_thread_id":"parent","depth":1}}},"parent_thread_id":"parent"}}"#).unwrap();
        let (rows, issues, _) = scan_with(dir.path(), DEFAULT_SCAN_LIMIT, &|_| None);
        assert!(issues.is_empty(), "{issues:?}");
        assert_eq!(rows.len(), 1);
        let json = serde_json::to_value(&rows[0]).unwrap();
        assert_eq!(json["children"][0]["session_id"], "child");
        assert!(
            !rows[0].resumable(),
            "child content cannot make an empty parent resumable"
        );
    }

    #[test]
    fn session_limit_cuts_the_sorted_view_without_losing_the_total() {
        let rows = (0..250)
            .map(|n| session("claude", &format!("{n:03}"), None, "2026-09-05"))
            .collect();
        let query: VaultQuery = serde_json::from_value(serde_json::json!({"limit": 50})).unwrap();
        let answer = view(rows, &query, vec![], false, &[]);
        assert_eq!(answer.shown, 50);
        assert_eq!(answer.total, 250);
        assert!(answer.truncated);
    }

    /// A card's NAME is what the person said, never the wrapper a CLI put in
    /// front of it — and a message that is nothing but wrapper yields the
    /// words inside. Previews are untouched by design (the raw tag there is
    /// honest context, and Orca's own list shows it).
    #[test]
    fn a_title_sheds_the_tags_a_cli_dressed_the_words_in() {
        assert_eq!(
            untagged_title("fix the tests"),
            Some("fix the tests".into())
        );
        assert_eq!(
            untagged_title(
                "<local-command-caveat>Caveat: generated by the user</local-command-caveat>"
            ),
            Some("Caveat: generated by the user".into())
        );
        // A `<` inside the person's own sentence is the person's — untouched.
        assert_eq!(
            untagged_title("compare a < b in Vec<String>"),
            Some("compare a < b in Vec<String>".into())
        );
        assert_eq!(
            untagged_title("<teammate-message from=\"lead\"><attachment>1 # Orca 소스 지도"),
            Some("1 # Orca 소스 지도".into())
        );
        assert_eq!(untagged_title("  <a><b>  "), None);
        assert_eq!(untagged_title("<unclosed"), None);
        assert_eq!(untagged_title(""), None);
    }

    /// Every agent's resume spelling, because a wrong one is not a smaller
    /// feature — it is a resume that errors, which reads as a lost session.
    #[test]
    fn each_agent_is_told_about_a_session_the_way_it_asks() {
        assert_eq!(
            resume_invocation("codex", "codex", "'abc'"),
            Some("codex resume 'abc'".into())
        );
        assert_eq!(
            resume_invocation("claude", "claude", "'abc'"),
            Some("claude --resume 'abc'".into())
        );
        assert_eq!(
            resume_invocation("copilot", "copilot", "'abc'"),
            Some("copilot --resume='abc'".into())
        );
        assert_eq!(
            resume_invocation("pi", "pi", "'abc'"),
            Some("pi --session 'abc'".into())
        );
        assert_eq!(
            resume_invocation("rovo", "acli", "'abc'"),
            Some("acli rovodev run --restore 'abc'".into())
        );
        assert_eq!(
            resume_invocation("antigravity", "agy", "'abc'"),
            Some("agy --conversation 'abc'".into())
        );
        // An agent we have no spelling for gets no command rather than a guess.
        assert_eq!(resume_invocation("nobody", "nobody", "'a'"), None);

        // The base command differs from the slug where the BINARY does —
        // antigravity's is `agy`, and the slug spelling was a reopen that
        // died on `command not found`.
        assert_eq!(resume_base("cursor"), "cursor-agent");
        assert_eq!(resume_base("rovo"), "acli");
        assert_eq!(resume_base("antigravity"), "agy");
        assert_eq!(resume_base("claude"), "claude");

        // Every agent in the table can be resumed, or the card offers nothing.
        for source in AGENT_SOURCES {
            assert!(
                resume_invocation(source.slug, resume_base(source.slug), "'x'").is_some(),
                "{} has a session store and no resume spelling",
                source.slug
            );
        }
    }

    /// The reopen wears the launch plan's clothes when the caller hands them
    /// over, and the roster of who carries them is Orca's
    /// (`RESUMABLE_TUI_AGENTS` ∩ this scanner).
    #[test]
    fn a_reopen_can_open_with_the_launch_buttons_base() {
        let held = session("claude", "s1", Some("/w"), "2026-01-01T00:00:00Z");
        assert_eq!(
            resume_command_with_base(&held, Some("claude --dangerously-skip-permissions")),
            Some("cd '/w' && claude --dangerously-skip-permissions --resume 's1'".into())
        );
        // A blank base falls back to the measured word.
        assert_eq!(
            resume_command_with_base(&held, Some("  ")),
            resume_command(&held)
        );
        // Plain flags ride bare; anything else is quoted.
        assert_eq!(resume_word("--yolo"), "--yolo");
        assert_eq!(resume_word("--allow \"*\""), "'--allow \"*\"'");
        for slug in [
            "claude",
            "codex",
            "antigravity",
            "opencode",
            "pi",
            "droid",
            "grok",
            "devin",
            "omp",
        ] {
            assert!(resume_carries_launch_args(slug), "{slug} lost its args");
        }
        // The rest keep the bare base, as Orca's plain road does.
        for slug in ["cursor", "copilot", "hermes", "openclaw", "rovo", "kimi"] {
            assert!(!resume_carries_launch_args(slug), "{slug} grew args");
        }
    }

    /// The whole command: the `cd` first, then Codex's home, then the agent.
    #[test]
    fn a_resume_lands_in_the_directory_the_session_ran_in() {
        let mut held = session("claude", "s1", Some("/w/it's mine"), "2026-01-01T00:00:00Z");
        let command = resume_command(&held).expect("a command");
        // The directory is quoted, and an apostrophe in it does not end the quote.
        assert_eq!(command, r"cd '/w/it'\''s mine' && claude --resume 's1'");

        // A Codex session found under a non-default home carries it back.
        held.agent = "codex".into();
        held.codex_home = Some("/data/codex-runtime-home/home".into());
        let codex = resume_command(&held).expect("a command");
        assert!(
            codex.contains("CODEX_HOME='/data/codex-runtime-home/home' codex resume 's1'"),
            "{codex}"
        );
        // And the env prefix sits AFTER the cd, so the variable reaches codex.
        assert!(codex.starts_with("cd '/w/it"), "{codex}");

        // `omp` resumes by transcript path, not id.
        let mut omp = session("omp", "s2", None, "2026-01-01T00:00:00Z");
        omp.file_path = "/f/transcript.jsonl".into();
        assert_eq!(
            resume_command(&omp),
            Some("omp --resume '/f/transcript.jsonl'".into())
        );

        // No id is no command.
        let mut blank = session("claude", "", None, "2026-01-01T00:00:00Z");
        blank.session_id = String::new();
        assert_eq!(resume_command(&blank), None);
    }

    /// The workspace a card was dropped on wins, because the drop IS the
    /// request. Reopening from the button drops nothing and keeps the old
    /// behaviour — the session's own directory — so the two gestures have to
    /// stay distinguishable in this one function.
    #[test]
    fn a_dropped_workspace_outranks_the_directory_a_session_ran_in() {
        let real = |path: &str| matches!(path, "/w/feature" | "/w/zerocode" | "/w/main");

        // Dropped: there, and the session's own directory does not get a vote
        // even though it is a perfectly good directory.
        assert_eq!(
            resume_root(Some("/w/feature"), Some("/w/zerocode"), "/w/main", real),
            "/w/feature"
        );
        // Not dropped — the 다시 열기 button — is the session's own directory.
        assert_eq!(
            resume_root(None, Some("/w/zerocode"), "/w/main", real),
            "/w/zerocode"
        );
        // Neither: where this window is standing.
        assert_eq!(resume_root(None, None, "/w/main", real), "/w/main");
        // A drop onto the workspace the window is already on is still a drop,
        // and still outranks a session that ran somewhere else.
        assert_eq!(
            resume_root(Some("/w/main"), Some("/w/zerocode"), "/w/main", real),
            "/w/main"
        );
    }

    /// Every step falls through rather than failing. A workspace that has been
    /// pruned since the sidebar drew it, and a session directory deleted long
    /// ago, are both ordinary — and a person who asked to reopen a
    /// conversation should get a shell, not a dialog.
    #[test]
    fn a_target_that_is_no_longer_a_directory_falls_through_to_the_next_one() {
        let real = |path: &str| matches!(path, "/w/zerocode" | "/w/main");

        // Dropped on a workspace that git has since pruned: the session's own.
        assert_eq!(
            resume_root(Some("/w/pruned"), Some("/w/zerocode"), "/w/main", real),
            "/w/zerocode"
        );
        // And when that is gone too, the active root — never an error.
        assert_eq!(
            resume_root(Some("/w/pruned"), Some("/w/deleted"), "/w/main", real),
            "/w/main"
        );
        // Blank is not a candidate: a store that records an empty `cwd` must
        // not be read as "the directory named ``".
        assert_eq!(
            resume_root(Some(""), Some("   "), "/w/main", real),
            "/w/main"
        );
        // The active root is taken as given — it is this window's own state,
        // not a path somebody sent in, and there is nothing after it to fall
        // through to.
        assert_eq!(resume_root(None, None, "/w/vanished", real), "/w/vanished");
    }

    /// A query is not a whitespace split: the operators take quoted values, and
    /// getting that wrong turns `repo:"my project"` into a search for `project`.
    #[test]
    fn a_search_keeps_its_operators_and_its_quoted_phrases() {
        let parsed = parse_query("fix repo:\"my project\" path:/w/a 'two words'");
        assert_eq!(parsed.terms, vec!["fix", "two words"]);
        assert_eq!(parsed.repo_terms, vec!["my project"]);
        assert_eq!(parsed.path_terms, vec!["/w/a"]);

        // Case folded, and an operator with no value is dropped rather than
        // becoming a term that matches nothing.
        let odd = parse_query("REPO:Zero repo: PATH:");
        assert_eq!(odd.repo_terms, vec!["zero"]);
        assert!(odd.terms.is_empty(), "{odd:?}");
        assert!(odd.path_terms.is_empty(), "{odd:?}");

        // An unclosed quote takes the rest, because that is what a person
        // half-way through typing has.
        assert_eq!(parse_query("\"still typing").terms, vec!["still typing"]);
        assert_eq!(parse_query("   ").terms, Vec::<String>::new());
    }

    /// Terms are ANDed across every field a card shows, including the preview —
    /// a search that only looked at titles would miss the thing the user
    /// remembers, which is what they said.
    #[test]
    fn every_term_has_to_appear_somewhere_the_card_shows() {
        let mut held = session("claude", "s1", Some("/w/zerocode"), "2026-01-01T00:00:00Z");
        held.branch = Some("feature/vault".into());

        assert!(matches(&held, &parse_query("")));
        assert!(matches(&held, &parse_query("tests")));
        assert!(matches(&held, &parse_query("tests vault")));
        // Both terms must hit; one miss is a miss.
        assert!(!matches(&held, &parse_query("tests nonsense")));
        assert!(matches(&held, &parse_query("repo:zerocode")));
        assert!(!matches(&held, &parse_query("repo:other")));
        assert!(matches(&held, &parse_query("path:/w/zero")));
        assert!(!matches(&held, &parse_query("path:/elsewhere")));
        // The agent's own name is searchable, which is how people filter first.
        assert!(matches(&held, &parse_query("claude")));
    }

    /// The panel's whole decision, and the two things a header must not lie
    /// about: which sessions are under it, and how many there are in total.
    #[test]
    fn the_list_is_grouped_newest_first_and_counts_what_it_hid() {
        let sessions = vec![
            session("claude", "old", Some("/w/a"), "2026-01-01T00:00:00Z"),
            session("codex", "new", Some("/w/b"), "2026-03-01T00:00:00Z"),
            session("claude", "mid", Some("/w/a"), "2026-02-01T00:00:00Z"),
        ];

        let by_project = view(
            sessions.clone(),
            &VaultQuery::default(),
            Vec::new(),
            false,
            &[],
        );
        assert_eq!(by_project.total, 3);
        assert_eq!(by_project.shown, 3);
        // The group holding the newest session comes first.
        assert_eq!(
            by_project
                .groups
                .iter()
                .map(|group| group.label.as_str())
                .collect::<Vec<_>>(),
            vec!["w/b", "w/a"]
        );
        // And within a group, newest first.
        assert_eq!(
            by_project.groups[1]
                .sessions
                .iter()
                .map(|one| one.session_id.as_str())
                .collect::<Vec<_>>(),
            vec!["mid", "old"]
        );

        // Grouping by agent uses the registry's label, not the slug.
        let by_agent = view(
            sessions.clone(),
            &VaultQuery {
                group: Group::Agent,
                ..Default::default()
            },
            Vec::new(),
            false,
            &[],
        );
        assert_eq!(by_agent.groups[0].label, "Codex");

        // An agent switched off leaves the total alone — "1 of 3" is the honest
        // pair, and counting the filtered list twice would say "1 of 1".
        let filtered = view(
            sessions.clone(),
            &VaultQuery {
                disabled_agents: vec!["claude".into()],
                ..Default::default()
            },
            Vec::new(),
            false,
            &[],
        );
        assert_eq!((filtered.shown, filtered.total), (1, 3));

        // Sorting by creation is a different question from last touched.
        let mut created_first = sessions.clone();
        created_first[0].created_at = Some("2026-09-01T00:00:00Z".into());
        let by_created = view(
            created_first,
            &VaultQuery {
                sort: Sort::Created,
                ..Default::default()
            },
            Vec::new(),
            false,
            &[],
        );
        assert_eq!(by_created.groups[0].sessions[0].session_id, "old");
    }

    /// Three groupings, three questions — and the one this panel could not
    /// answer was "which of my workspaces was this".
    ///
    /// A person with two checkouts of one repository saw two headers naming two
    /// directories, while the sidebar beside them named the same two checkouts.
    #[test]
    fn the_three_groupings_answer_three_different_questions() {
        let refs = vec![
            crate::usage_stats::WorktreeRef {
                repo_id: "repo".into(),
                worktree_id: "/w/main".into(),
                path: "/w/main".into(),
                display_name: "repo".into(),
            },
            crate::usage_stats::WorktreeRef {
                repo_id: "repo".into(),
                worktree_id: "/w/main/trees/port".into(),
                path: "/w/main/trees/port".into(),
                display_name: "repo · port".into(),
            },
        ];
        let sessions = vec![
            session(
                "claude",
                "a",
                Some("/w/main/crates"),
                "2026-03-01T00:00:00Z",
            ),
            session(
                "codex",
                "b",
                Some("/w/main/trees/port"),
                "2026-02-01T00:00:00Z",
            ),
            session(
                "claude",
                "c",
                Some("/elsewhere/thing"),
                "2026-01-01T00:00:00Z",
            ),
        ];
        let grouped = |group: Group| {
            view(
                sessions.clone(),
                &VaultQuery {
                    group,
                    ..Default::default()
                },
                Vec::new(),
                false,
                &refs,
            )
            .groups
            .iter()
            .map(|one| one.label.clone())
            .collect::<Vec<String>>()
        };
        // By workspace: the sidebar's own names, the nested checkout claiming
        // its own session rather than the repository that contains it — and a
        // directory nobody manages still gets a header.
        assert_eq!(
            grouped(Group::Project),
            ["repo", "repo · port", "elsewhere/thing"]
        );
        // By folder: the directories, whoever owns them.
        assert_eq!(
            grouped(Group::Folder),
            ["main/crates", "trees/port", "elsewhere/thing"]
        );
        assert_eq!(grouped(Group::Agent), ["Claude", "Codex"]);
    }

    /// The filter's list names the agents that are here, with their numbers —
    /// and keeps naming one after it has been switched off.
    ///
    /// A list computed after the filter loses the row you just unchecked, which
    /// is a filter you cannot undo without reloading the panel.
    #[test]
    fn the_agent_list_survives_the_agent_being_switched_off() {
        let sessions = vec![
            session("claude", "a", Some("/w/a"), "2026-03-01T00:00:00Z"),
            session("claude", "b", Some("/w/a"), "2026-02-01T00:00:00Z"),
            session("codex", "c", Some("/w/b"), "2026-01-01T00:00:00Z"),
        ];
        let all = view(
            sessions.clone(),
            &VaultQuery::default(),
            Vec::new(),
            false,
            &[],
        );
        assert_eq!(
            all.agents
                .iter()
                .map(|row| (row.slug.as_str(), row.label.as_str(), row.sessions))
                .collect::<Vec<_>>(),
            [("claude", "Claude", 2), ("codex", "Codex", 1)],
            "most sessions first, named the way a group header names them"
        );

        let without = view(
            sessions,
            &VaultQuery {
                disabled_agents: vec!["claude".into()],
                ..Default::default()
            },
            Vec::new(),
            false,
            &[],
        );
        assert_eq!(without.shown, 1);
        assert_eq!(
            without.agents, all.agents,
            "the switched-off agent left the filter list with its sessions"
        );
    }

    /// One directory is one group however its path was spelled — and two
    /// directories are never one because their names differ only in case.
    #[test]
    fn a_folder_is_grouped_by_what_it_is_not_by_how_it_was_typed() {
        let sessions = vec![
            session("claude", "a", Some("/w/repo"), "2026-03-01T00:00:00Z"),
            session("claude", "b", Some("/w/repo/"), "2026-02-01T00:00:00Z"),
            session("claude", "c", Some("\\w\\repo"), "2026-01-01T00:00:00Z"),
        ];
        let one = view(
            sessions,
            &VaultQuery {
                group: Group::Folder,
                ..Default::default()
            },
            Vec::new(),
            false,
            &[],
        );
        assert_eq!(one.groups.len(), 1, "one folder drew three headers");
        assert_eq!(one.groups[0].sessions.len(), 3);

        // Case is not a spelling of the same directory: where these are two
        // folders, merging them is two projects' conversations in one pile.
        let cased = view(
            vec![
                session("claude", "a", Some("/w/App"), "2026-03-01T00:00:00Z"),
                session("claude", "b", Some("/w/app"), "2026-02-01T00:00:00Z"),
            ],
            &VaultQuery {
                group: Group::Folder,
                ..Default::default()
            },
            Vec::new(),
            false,
            &[],
        );
        assert_eq!(cased.groups.len(), 2, "two folders drew one header");
        assert_eq!(folder_group_key(None), "unknown");
        assert_eq!(folder_group_key(Some("/")), "/");
    }

    /// Two directories whose last two segments match must not draw the same
    /// header. On a real machine this is not hypothetical — a bench run makes a
    /// dozen of them, and two identical adjacent headers read as a repeated list.
    #[test]
    fn two_directories_with_the_same_tail_get_different_headers() {
        let sessions = vec![
            session(
                "claude",
                "one",
                Some("/w/bench/implement-median/claude-code/t1"),
                "2026-01-02T00:00:00Z",
            ),
            session(
                "claude",
                "two",
                Some("/w/bench/debug-blank-line/claude-code/t1"),
                "2026-01-01T00:00:00Z",
            ),
            session(
                "claude",
                "three",
                Some("/w/zerocode"),
                "2026-01-03T00:00:00Z",
            ),
        ];
        let shown = view(sessions, &VaultQuery::default(), Vec::new(), false, &[]);
        let labels: Vec<&str> = shown
            .groups
            .iter()
            .map(|group| group.label.as_str())
            .collect();
        assert_eq!(
            labels,
            vec![
                "w/zerocode",
                "implement-median/claude-code/t1",
                "debug-blank-line/claude-code/t1"
            ],
            "{labels:?}"
        );
        // The one that was already unique keeps the plain two-segment label —
        // only a repeat pays for the extra width.
        assert_eq!(labels[0].matches('/').count(), 1);
    }

    /// A truncated scan says so all the way to the panel. A list that is a recent
    /// slice and claims to be everything is the failure this carries.
    #[test]
    fn a_truncated_scan_stays_truncated_through_the_view() {
        let shown = view(
            vec![session("claude", "a", Some("/w/a"), "2026-01-01T00:00:00Z")],
            &VaultQuery::default(),
            Vec::new(),
            true,
            &[],
        );
        assert!(shown.truncated);
        assert!(!view(Vec::new(), &VaultQuery::default(), Vec::new(), false, &[]).truncated);
    }

    /// A scope is segment-wise. `/w/ab` is not inside `/w/a`, and the prefix
    /// test that says it is would show another project's sessions under this one.
    #[test]
    fn a_scope_does_not_leak_into_a_sibling_with_a_longer_name() {
        assert!(inside_or_equal("/w/a", "/w/a"));
        assert!(inside_or_equal("/w/a", "/w/a/b"));
        assert!(!inside_or_equal("/w/a", "/w/ab"));
        assert!(!inside_or_equal("/w/a/b", "/w/a"));

        let sessions = vec![
            session("claude", "in", Some("/w/a/deep"), "2026-01-01T00:00:00Z"),
            session("claude", "out", Some("/w/ab"), "2026-01-01T00:00:00Z"),
            session("claude", "none", None, "2026-01-01T00:00:00Z"),
        ];
        let scoped = view(
            sessions,
            &VaultQuery {
                scope_paths: vec!["/w/a".into()],
                ..Default::default()
            },
            Vec::new(),
            false,
            &[],
        );
        assert_eq!(scoped.shown, 1);
        assert_eq!(scoped.groups[0].sessions[0].session_id, "in");
    }

    /// Empty sessions: there by default, hidden on request — and "empty" means
    /// no conversation, not no file.
    #[test]
    fn a_session_nobody_spoke_in_is_shown_unless_asked_otherwise() {
        let mut blank = session("claude", "blank", Some("/w/a"), "2026-01-01T00:00:00Z");
        blank.message_count = 0;
        blank.preview.clear();
        assert!(!blank.resumable());

        let mut spoke = blank.clone();
        spoke.preview = vec![PreviewMessage {
            role: "user".into(),
            text: "hello".into(),
        }];
        assert!(
            spoke.resumable(),
            "a message with no count is still a message"
        );

        let default_view = view(
            vec![blank.clone()],
            &VaultQuery::default(),
            Vec::new(),
            false,
            &[],
        );
        assert_eq!(default_view.shown, 1);

        let hidden = view(
            vec![blank],
            &VaultQuery {
                hide_empty: true,
                ..Default::default()
            },
            Vec::new(),
            false,
            &[],
        );
        assert_eq!((hidden.shown, hidden.total), (0, 1));
    }

    /// The discovery table: every agent has a root, and the file predicates are
    /// the ones that keep a store from producing nonsense cards.
    #[test]
    fn each_store_is_looked_for_where_its_agent_keeps_it() {
        let home = Path::new("/h");
        let claude = AGENT_SOURCES
            .iter()
            .find(|one| one.slug == "claude")
            .expect("claude");
        assert_eq!(
            roots(claude, home, None),
            vec![PathBuf::from("/h/.claude/projects")]
        );
        assert!(wanted(
            claude,
            Path::new("/h/.claude/projects/-w-a/x.jsonl")
        ));
        assert!(!wanted(
            claude,
            Path::new("/h/.claude/projects/-w-a/x.json")
        ));

        // Cursor's transcripts live beside other things it keeps per project.
        let cursor = AGENT_SOURCES
            .iter()
            .find(|one| one.slug == "cursor")
            .expect("cursor");
        assert!(wanted(
            cursor,
            Path::new("/h/.cursor/projects/p/agent-transcripts/x.jsonl")
        ));
        assert!(!wanted(
            cursor,
            Path::new("/h/.cursor/projects/p/other/x.jsonl")
        ));

        // Grok and Rovo keep one metadata file per session directory; every
        // other file in there would be a duplicate card.
        let grok = AGENT_SOURCES
            .iter()
            .find(|one| one.slug == "grok")
            .expect("grok");
        assert!(wanted(grok, Path::new("/h/.grok/sessions/s/summary.json")));
        assert!(!wanted(
            grok,
            Path::new("/h/.grok/sessions/s/messages.json")
        ));

        // Hermes names its files rather than its directories.
        let hermes = AGENT_SOURCES
            .iter()
            .find(|one| one.slug == "hermes")
            .expect("hermes");
        assert!(wanted(
            hermes,
            Path::new("/h/.hermes/sessions/session_1.json")
        ));
        assert!(!wanted(hermes, Path::new("/h/.hermes/sessions/index.json")));

        // Every entry names a real extension and a label, so no card can appear
        // with a blank agent name.
        for source in AGENT_SOURCES {
            assert!(!source.slug.is_empty() && !source.label.is_empty());
            assert!(
                ["json", "jsonl"].contains(&source.extension),
                "{}",
                source.slug
            );
            assert!(!source.segments.is_empty(), "{}", source.slug);
        }
    }

    /// Claude's records, in the shape a real file carries them. The lines are
    /// copied from `~/.claude/projects` on a working machine and trimmed — the
    /// keys and the two content shapes are what was actually there.
    #[test]
    fn a_claude_file_gives_up_its_directory_branch_and_first_words() {
        let file = concat!(
            r#"{"type":"last-prompt","leafUuid":"x","sessionId":"b1ee393b"}"#,
            "\n",
            r#"{"type":"mode","mode":"normal","sessionId":"b1ee393b"}"#,
            "\n",
            r#"{"type":"attachment","cwd":"/w/zerocode","gitBranch":"main","sessionId":"b1ee393b","timestamp":"2026-08-07T06:12:18.268Z","attachment":{"type":"hook_success"}}"#,
            "\n",
            r#"{"type":"user","cwd":"/w/zerocode","gitBranch":"main","sessionId":"b1ee393b","timestamp":"2026-08-07T06:12:20.000Z","message":{"role":"user","content":"  fix   report.py  "}}"#,
            "\n",
            r#"{"type":"assistant","sessionId":"b1ee393b","timestamp":"2026-08-07T06:12:25.000Z","message":{"role":"assistant","model":"claude-opus-5","content":[{"type":"text","text":"I'll look at the files."},{"type":"tool_use","id":"t1"}]}}"#,
            "\n",
        );
        let parsed = claude_session(file).expect("parsed");
        assert_eq!(parsed.session_id, "b1ee393b");
        assert_eq!(parsed.cwd.as_deref(), Some("/w/zerocode"));
        assert_eq!(parsed.branch.as_deref(), Some("main"));
        assert_eq!(parsed.model.as_deref(), Some("claude-opus-5"));
        // Only the two conversation roles are counted; `mode` and `attachment`
        // are bookkeeping and would inflate every card.
        assert_eq!(parsed.message_count, 2);
        assert_eq!(parsed.preview.len(), 2);
        // Whitespace collapsed, so a card is one line.
        assert_eq!(parsed.preview[0].text, "fix report.py");
        // An assistant's typed parts join their text and drop the tool call.
        assert_eq!(parsed.preview[1].text, "I'll look at the files.");
        // First timestamp is the creation, last is the update.
        assert_eq!(
            parsed.created_at.as_deref(),
            Some("2026-08-07T06:12:18.268Z")
        );
        assert_eq!(
            parsed.updated_at.as_deref(),
            Some("2026-08-07T06:12:25.000Z")
        );
    }

    /// Codex's rollout, likewise copied from a real file.
    #[test]
    fn a_codex_rollout_is_identified_by_its_meta_line() {
        let file = concat!(
            r#"{"timestamp":"2026-05-31T08:22:18.528Z","type":"session_meta","payload":{"id":"019e7d20","timestamp":"2026-05-31T08:21:50.336Z","cwd":"/Users/dev/2026/forge-code","cli_version":"0.135.0","model":"gpt-5"}}"#,
            "\n",
            r#"{"timestamp":"2026-05-31T08:23:00.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"port the thing"}]}}"#,
            "\n",
            r#"{"timestamp":"2026-05-31T08:23:40.000Z","type":"event_msg","payload":{"type":"token_count"}}"#,
            "\n",
        );
        let parsed = codex_session(file).expect("parsed");
        assert_eq!(parsed.session_id, "019e7d20");
        assert_eq!(parsed.cwd.as_deref(), Some("/Users/dev/2026/forge-code"));
        assert_eq!(parsed.model.as_deref(), Some("gpt-5"));
        // The meta's own timestamp is the start, not the line's wrapper.
        assert_eq!(
            parsed.created_at.as_deref(),
            Some("2026-05-31T08:21:50.336Z")
        );
        // A `token_count` is not a turn.
        assert_eq!(parsed.message_count, 1);
        assert_eq!(parsed.preview[0].text, "port the thing");

        // A file that is not a rollout at all yields no id, and the scan then
        // falls back to the file name rather than dropping the session.
        assert_eq!(codex_session("not json\n").expect("parsed").session_id, "");
    }

    /// Our own CLI's sessions are cards too — and each conversation is ONE
    /// card, though three files in its directory hold its lines.
    ///
    /// The lines are the measured ones (`~/.zo/projects/<slug>/sessions`,
    /// re-measured whole 2026-08-19): the plain log carries the conversation
    /// under a `session_meta` first line, the `.vault.jsonl` twin distills
    /// the same turns, and `.rot-*` rotations repeat turns the live log
    /// still has. When the twins coexist the VAULT is the card — chosen at
    /// candidate time, because a card's id embeds its file path and the id
    /// dedup can never see the two as one — its id must shed the `.vault`
    /// tail (it becomes the resume argument), the rotation is refused by
    /// name even though its lines would parse, and a signature block adds
    /// no words.
    #[test]
    fn a_zo_conversation_is_one_card_and_its_bookkeeping_is_none() {
        let dir = tempfile::tempdir().expect("temp");
        let home = dir.path();
        let sessions_dir = home.join(".zo/projects/Users-w-a-0f3a/sessions");
        std::fs::create_dir_all(&sessions_dir).expect("mkdir");
        std::fs::write(
            sessions_dir.join("session-9-0.vault.jsonl"),
            concat!(
                r#"{"message":{"blocks":[{"text":"report.py가 죽습니다 고쳐주세요","type":"text"}],"role":"user"},"type":"vault","vault_seq":0}"#,
                "\n",
                r#"{"message":{"blocks":[{"signature":"CAIS","type":"thinking"},{"text":"고쳤습니다","type":"text"}],"role":"assistant"},"type":"vault","vault_seq":1}"#,
                "\n",
            ),
        )
        .expect("write");
        std::fs::write(
            sessions_dir.join("session-9-0.jsonl"),
            concat!(
                r#"{"created_at_ms":1787124638194,"session_id":"session-9-0","type":"session_meta","updated_at_ms":1787124638194,"version":1}"#,
                "\n",
                r#"{"message":{"blocks":[{"text":"report.py가 죽습니다 고쳐주세요","type":"text"}],"role":"user"},"turn_index":0,"type":"message"}"#,
                "\n",
                r#"{"message":{"blocks":[{"text":"고쳤습니다","type":"text"}],"role":"assistant"},"turn_index":1,"type":"message"}"#,
                "\n",
            ),
        )
        .expect("write");
        std::fs::write(
            sessions_dir.join("session-9-0.rot-123.jsonl"),
            concat!(
                r#"{"message":{"blocks":[{"text":"옛 회전분","type":"text"}],"role":"user"},"turn_index":0,"type":"message"}"#,
                "\n",
            ),
        )
        .expect("write");

        let (sessions, _, _) = scan(home, DEFAULT_SCAN_LIMIT);
        let zo: Vec<_> = sessions.iter().filter(|one| one.agent == "zo").collect();
        assert_eq!(
            zo.len(),
            1,
            "the plain twin or a rotation became a card too: {:?}",
            zo.iter().map(|one| &one.file_path).collect::<Vec<_>>()
        );
        let card = zo[0];
        // The vault twin is the one that stood.
        assert!(
            card.file_path.ends_with(".vault.jsonl"),
            "the plain twin shadowed the vault: {}",
            card.file_path
        );
        assert_eq!(card.session_id, "session-9-0");
        assert_eq!(card.title, "report.py가 죽습니다 고쳐주세요");
        assert_eq!(card.message_count, 2);
        assert_eq!(card.preview.len(), 2);
        // The signature block added no words; the text beside it did.
        assert_eq!(card.preview[1].text, "고쳤습니다");
        assert_eq!(card.cwd, None);
        assert_eq!(card.resume.as_deref(), Some("zo --resume 'session-9-0'"));
        // No vault line said a time, so the file's own mtime answered.
        assert!(card.updated_at.is_some());
    }

    /// The other 99.5% of zo's sessions have no vault twin at all — the
    /// plain log IS the conversation (재실측 2026-08-19: vault를 가진 프로젝트는
    /// 9,300 중 46뿐인데 카드가 그만큼만 서서, 사용자 보고가 대부분의
    /// 프로젝트에서 열린 채였다).
    ///
    /// The meta line hands the card its id and both epoch-ms clocks; tool
    /// and system turns and `compaction` lines are machinery and count as
    /// nothing; a meta-only log — a session that never got a first word —
    /// is no card at all.
    #[test]
    fn a_plain_zo_log_cards_without_a_vault_twin() {
        let dir = tempfile::tempdir().expect("temp");
        let home = dir.path();
        let sessions_dir = home.join(".zo/projects/Users-w-b-1a2b/sessions");
        std::fs::create_dir_all(&sessions_dir).expect("mkdir");
        std::fs::write(
            sessions_dir.join("session-1785288389469-0.jsonl"),
            concat!(
                r#"{"created_at_ms":1785288389469,"session_id":"session-1785288389469-0","type":"session_meta","updated_at_ms":1785288401000,"version":1}"#,
                "\n",
                r#"{"message":{"blocks":[{"text":"재현 스크립트를 돌려줘","type":"text"}],"role":"user"},"turn_index":0,"type":"message"}"#,
                "\n",
                r#"{"message":{"blocks":[{"id":"t1","input":{},"name":"bash","type":"tool_use"}],"model":"m","role":"assistant","usage":{}},"turn_index":1,"type":"message"}"#,
                "\n",
                r#"{"message":{"blocks":[{"is_error":false,"output":"ok","tool_name":"bash","tool_use_id":"t1","type":"tool_result"}],"role":"tool"},"turn_index":2,"type":"message"}"#,
                "\n",
                r#"{"message":{"blocks":[{"text":"환경 안내","type":"text"}],"role":"system"},"turn_index":3,"type":"message"}"#,
                "\n",
                r#"{"message":{"blocks":[{"signature":"CAIS","thinking":"…","type":"thinking"},{"text":"돌렸고 통과했습니다","type":"text"}],"role":"assistant","model":"m","usage":{}},"turn_index":4,"type":"message"}"#,
                "\n",
                r#"{"type":"compaction"}"#,
                "\n",
            ),
        )
        .expect("write");
        // A session that never got a first word: meta and nothing else.
        std::fs::write(
            sessions_dir.join("session-1785248211341-0.jsonl"),
            concat!(
                r#"{"created_at_ms":1785248211341,"session_id":"session-1785248211341-0","type":"session_meta","updated_at_ms":1785248211341,"version":1}"#,
                "\n",
            ),
        )
        .expect("write");

        let (sessions, _, _) = scan(home, DEFAULT_SCAN_LIMIT);
        let zo: Vec<_> = sessions.iter().filter(|one| one.agent == "zo").collect();
        assert_eq!(
            zo.len(),
            1,
            "the wordless log became a card, or the spoken one did not: {:?}",
            zo.iter().map(|one| &one.file_path).collect::<Vec<_>>()
        );
        let card = zo[0];
        assert_eq!(card.session_id, "session-1785288389469-0");
        assert_eq!(card.title, "재현 스크립트를 돌려줘");
        // user + the two assistant turns; tool, system and compaction are
        // machinery. The tool_use-only assistant turn counts but says
        // nothing, so the preview holds the two spoken lines.
        assert_eq!(card.message_count, 3);
        assert_eq!(card.preview.len(), 2);
        assert_eq!(card.preview[1].text, "돌렸고 통과했습니다");
        // The meta line's epoch-ms clocks, not the file's mtime.
        assert_eq!(card.created_at.as_deref(), Some("2026-07-29T01:26:29.469Z"));
        assert_eq!(card.updated_at.as_deref(), Some("2026-07-29T01:26:41.000Z"));
        assert_eq!(
            card.resume.as_deref(),
            Some("zo --resume 'session-1785288389469-0'")
        );
    }

    /// The scan against a real directory tree: the two formats read, the rest
    /// counted, and the three bounds respected.
    #[test]
    fn a_scan_reads_what_it_knows_and_says_what_it_skipped() {
        let dir = tempfile::tempdir().expect("temp");
        let home = dir.path();

        let claude = home.join(".claude/projects/-w-a");
        std::fs::create_dir_all(&claude).expect("mkdir");
        std::fs::write(
            claude.join("aaa.jsonl"),
            concat!(
                r#"{"type":"user","cwd":"/w/a","sessionId":"aaa","timestamp":"2026-01-01T00:00:00Z","message":{"role":"user","content":"hello there"}}"#,
                "\n",
            ),
        )
        .expect("write");
        // A subagent transcript is not a session and must not double the card.
        let subagents = claude.join("aaa/subagents");
        std::fs::create_dir_all(&subagents).expect("mkdir");
        std::fs::write(
            subagents.join("bbb.jsonl"),
            r#"{"type":"user","sessionId":"bbb","message":{"role":"user","content":"sub"}}"#,
        )
        .expect("write");

        let codex = home.join(".codex/sessions/2026/05/31");
        std::fs::create_dir_all(&codex).expect("mkdir");
        std::fs::write(
            codex.join("rollout-1.jsonl"),
            concat!(
                r#"{"type":"session_meta","payload":{"id":"ccc","cwd":"/w/b","timestamp":"2026-05-31T00:00:00Z"}}"#,
                "\n",
            ),
        )
        .expect("write");

        // An agent we find and cannot read yet.
        let hermes = home.join(".hermes/sessions");
        std::fs::create_dir_all(&hermes).expect("mkdir");
        std::fs::write(hermes.join("session_1.json"), "{}").expect("write");
        // And a file in the same place that is not a session file at all.
        std::fs::write(hermes.join("index.json"), "{}").expect("write");

        let (sessions, issues, truncated) = scan_with(home, DEFAULT_SCAN_LIMIT, &|_source| None);
        assert!(!truncated);
        let ids: Vec<&str> = sessions.iter().map(|one| one.session_id.as_str()).collect();
        assert!(ids.contains(&"aaa"), "{ids:?}");
        assert!(ids.contains(&"ccc"), "{ids:?}");
        assert!(
            !ids.contains(&"bbb"),
            "a subagent transcript became a session card: {ids:?}"
        );

        let read = sessions
            .iter()
            .find(|one| one.session_id == "aaa")
            .expect("the claude session");
        assert_eq!(read.cwd.as_deref(), Some("/w/a"));
        assert_eq!(read.title, "hello there");
        assert_eq!(
            read.resume.as_deref(),
            Some("cd '/w/a' && claude --resume 'aaa'")
        );
        // The default Codex home is NOT put in the environment — doing so would
        // override a variable the user may have set on purpose.
        let rollout = sessions
            .iter()
            .find(|one| one.session_id == "ccc")
            .expect("the codex session");
        assert_eq!(rollout.codex_home, None);
        assert_eq!(
            rollout.resume.as_deref(),
            Some("cd '/w/b' && codex resume 'ccc'")
        );
        // mtime is always there, so both sorts have something to land on.
        assert!(
            rollout.modified_at.ends_with('Z'),
            "{}",
            rollout.modified_at
        );

        // The unread agent is named once with a count, not silently dropped —
        // and the file that was not a session file is not counted either.
        let hermes_issue = issues
            .iter()
            .find(|issue| issue.agent == "hermes")
            .expect("an issue for hermes");
        assert_eq!(hermes_issue.reason, "unread-format");
        assert_eq!(hermes_issue.count, 1);
        // Nothing claims to have read a format it cannot.
        assert!(
            !issues.iter().any(|issue| issue.agent == "claude"),
            "{issues:?}"
        );
    }

    /// A symlink in somebody else's session store is not followed. A loop would
    /// hang the scan; a link out of the store would read files the panel has no
    /// business opening.
    #[test]
    fn a_link_inside_a_store_is_not_followed() {
        let dir = tempfile::tempdir().expect("temp");
        let home = dir.path();
        let projects = home.join(".claude/projects/-w-a");
        std::fs::create_dir_all(&projects).expect("mkdir");
        std::fs::write(
            projects.join("aaa.jsonl"),
            r#"{"type":"user","cwd":"/w/a","sessionId":"aaa","message":{"role":"user","content":"hi"}}"#,
        )
        .expect("write");

        #[cfg(unix)]
        {
            // A loop back to the root of the walk.
            std::os::unix::fs::symlink(home.join(".claude/projects"), projects.join("loop"))
                .expect("symlink");
            // And a link at a file the walk would otherwise want.
            std::os::unix::fs::symlink(projects.join("aaa.jsonl"), projects.join("copy.jsonl"))
                .expect("symlink");
        }

        // This test is about walking THIS fixture. A developer running the
        // suite may legitimately export CODEX_HOME; `scan` would include that
        // second real store and turn its session count into an assertion about
        // the machine. The injectable door exists to keep those facts apart.
        let (sessions, _, truncated) = scan_with(home, DEFAULT_SCAN_LIMIT, &|_source| None);
        assert!(!truncated, "the walk did not terminate cleanly");
        assert_eq!(sessions.len(), 1, "{sessions:#?}");
    }

    /// A Codex session under a home that is not the default carries it, so the
    /// resume puts it back — a resume that forgets it reads the wrong store and
    /// reports the session as gone.
    #[test]
    fn a_codex_session_outside_the_default_home_carries_it_to_the_resume() {
        let dir = tempfile::tempdir().expect("temp");
        let home = dir.path();
        let mirror = home.join(".zerocode/codex-runtime-home/home");
        let sessions_dir = mirror.join("sessions/2026/01/01");
        std::fs::create_dir_all(&sessions_dir).expect("mkdir");
        std::fs::write(
            sessions_dir.join("rollout-1.jsonl"),
            r#"{"type":"session_meta","payload":{"id":"ddd","cwd":"/w/c"}}"#,
        )
        .expect("write");

        let (sessions, _, _) = scan_with(home, DEFAULT_SCAN_LIMIT, &|source| {
            (source.slug == "codex").then(|| mirror.clone())
        });

        let found = sessions
            .iter()
            .find(|one| one.session_id == "ddd")
            .expect("the mirrored session");
        assert_eq!(
            found.codex_home.as_deref(),
            Some(mirror.to_string_lossy().as_ref())
        );
        let resume = found.resume.as_deref().expect("a command");
        assert!(resume.contains("CODEX_HOME="), "{resume}");
        assert!(resume.contains("codex resume 'ddd'"), "{resume}");
    }

    /// The timestamp formatter, because a leap-year rule that is nearly right
    /// puts a card under the wrong day and the sort then reads it wrong.
    #[test]
    fn a_file_time_becomes_the_date_it_actually_is() {
        let at =
            |seconds: u64| rfc3339(std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds));
        assert_eq!(at(0), "1970-01-01T00:00:00.000Z");
        // A leap day, and the day after it.
        assert_eq!(at(951_782_400), "2000-02-29T00:00:00.000Z");
        assert_eq!(at(951_868_800), "2000-03-01T00:00:00.000Z");
        // 1900 is not a leap year and 2000 is; a rule that only checks `% 4`
        // gets one of these wrong.
        assert_eq!(at(1_709_164_800), "2024-02-29T00:00:00.000Z");
        assert_eq!(at(1_740_787_199), "2025-02-28T23:59:59.000Z");
        assert_eq!(at(1_767_225_600), "2026-01-01T00:00:00.000Z");
        // And the strings sort the way the times do, which is the only property
        // the panel actually depends on.
        assert!(at(1_767_225_600) < at(1_767_312_000));
    }

    /// A long first line is cut on a word break, so a card is a line and not a
    /// paragraph.
    #[test]
    fn a_long_prompt_becomes_one_readable_line() {
        let long = "word ".repeat(200);
        let cut = clamp(&long);
        assert!(cut.chars().count() <= SUMMARY_CHARS + 1, "{}", cut.len());
        assert!(cut.ends_with('…'));
        assert!(!cut.contains("  "));
        // Short text is left exactly as it was, minus the collapsing.
        assert_eq!(clamp("  a   b  "), "a b");
    }

    /// Prime Agent reads two variables, and the order is not ours to choose.
    ///
    /// A session directory names the transcripts root and is taken verbatim; an
    /// agent directory always gets `sessions` under it, even when the value is
    /// itself named `sessions`. The session one wins, and the loser is not
    /// consulted — substituting the other variable's directory would report one
    /// agent's sessions under another's name, which is the "never
    /// cross-fall-back" rule the original states for this family.
    #[test]
    fn prime_agent_reads_its_two_variables_in_its_own_order() {
        let prime = AGENT_SOURCES
            .iter()
            .find(|one| one.slug == "prime-agent")
            .expect("prime-agent");
        let home = Path::new("/h");
        let named = |set: &[(&str, &str)]| {
            let held: Vec<(String, String)> = set
                .iter()
                .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                .collect();
            named_home(
                prime,
                |want| {
                    held.iter()
                        .find(|(name, _)| name == want)
                        .map(|(_, value)| value.clone())
                },
                Some(home),
            )
        };

        // Nothing set: no override, and the default root stands alone.
        assert_eq!(named(&[]), None);
        assert_eq!(
            roots(prime, home, None),
            vec![PathBuf::from("/h/.prime/agent/sessions")]
        );

        // The agent directory gets `sessions` — unconditionally, so a value
        // already called `sessions` nests one deeper rather than being reused.
        assert_eq!(
            named(&[("PRIME_AGENT_CODING_AGENT_DIR", "/w/prime")]),
            Some(PathBuf::from("/w/prime/sessions"))
        );
        assert_eq!(
            named(&[("PRIME_AGENT_CODING_AGENT_DIR", "/w/sessions")]),
            Some(PathBuf::from("/w/sessions/sessions"))
        );

        // The session directory is verbatim, and outranks the agent one.
        assert_eq!(
            named(&[("PRIME_AGENT_SESSION_DIR", "/w/transcripts")]),
            Some(PathBuf::from("/w/transcripts"))
        );
        assert_eq!(
            named(&[
                ("PRIME_AGENT_SESSION_DIR", "/w/transcripts"),
                ("PRIME_AGENT_CODING_AGENT_DIR", "/w/prime"),
            ]),
            Some(PathBuf::from("/w/transcripts")),
            "the agent directory was consulted while a session directory stood"
        );
        // The spelling this variable had before it was renamed still works, and
        // still loses to the current one.
        assert_eq!(
            named(&[("PRIME_AGENT_CODING_AGENT_SESSION_DIR", "/w/legacy")]),
            Some(PathBuf::from("/w/legacy"))
        );
        assert_eq!(
            named(&[
                ("PRIME_AGENT_SESSION_DIR", "/w/now"),
                ("PRIME_AGENT_CODING_AGENT_SESSION_DIR", "/w/legacy"),
            ]),
            Some(PathBuf::from("/w/now"))
        );
    }

    /// A variable that names a relative path names nothing.
    ///
    /// `sessions`, `.` or `..` would resolve against whatever directory this
    /// process happens to be in — a root nobody chose. The original refuses
    /// them for the same reason (`absoluteConfiguredDir`), and a leading `~` is
    /// expanded because the CLI expands it itself, so a value set in a config
    /// file or a plist still means the home directory.
    #[test]
    fn a_named_home_must_be_absolute_and_a_tilde_is_the_home() {
        let pi = AGENT_SOURCES
            .iter()
            .find(|one| one.slug == "pi")
            .expect("pi");
        let home = Path::new("/home/someone");
        let named = |value: &str| {
            let held = value.to_string();
            named_home(pi, |_| Some(held.clone()), Some(home))
        };
        for refused in ["", "   ", "sessions", ".", "..", "./agent"] {
            assert_eq!(named(refused), None, "`{refused}` became a root");
        }
        assert_eq!(named("~"), Some(PathBuf::from("/home/someone")));
        assert_eq!(
            named("~/.pi/agent/sessions"),
            Some(PathBuf::from("/home/someone/.pi/agent/sessions"))
        );
        // A trailing separator is not a different directory.
        assert_eq!(named("/w/pi/"), Some(PathBuf::from("/w/pi")));
        assert_eq!(named("/w/pi"), Some(PathBuf::from("/w/pi")));
    }

    /// An override home is honoured, and the default is still looked at — a user
    /// with `CODEX_HOME` set may still have older sessions in `~/.codex`.
    #[test]
    fn an_agent_home_from_the_environment_is_looked_at_first() {
        let codex = AGENT_SOURCES
            .iter()
            .find(|one| one.slug == "codex")
            .expect("codex");
        // Passed in rather than set in the environment: a test that mutates a
        // process-global flakes whatever runs beside it, which is how the split
        // between `env_home` and `roots` was found.
        // The override is the sessions directory itself — the variable's tail is
        // joined where the variable is READ ([`named_home`]), so that a source
        // with two variables can give them different tails.
        assert_eq!(
            roots(
                codex,
                Path::new("/h"),
                Some(Path::new("/elsewhere/codex/sessions"))
            ),
            vec![
                PathBuf::from("/elsewhere/codex/sessions"),
                PathBuf::from("/h/.codex/sessions")
            ]
        );
        // Nothing set: the default, once.
        assert_eq!(
            roots(codex, Path::new("/h"), None),
            vec![PathBuf::from("/h/.codex/sessions")]
        );
        // An override for an agent that has no variable is ignored rather than
        // silently becoming a second root nobody asked for.
        let claude = AGENT_SOURCES
            .iter()
            .find(|one| one.slug == "claude")
            .expect("claude");
        assert_eq!(
            roots(claude, Path::new("/h"), Some(Path::new("/elsewhere"))),
            vec![PathBuf::from("/h/.claude/projects")]
        );
    }
}
