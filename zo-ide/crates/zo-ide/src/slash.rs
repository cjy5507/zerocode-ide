//! zo 가 실제로 처리하는 슬래시 명령의 **정본 목록**.
//!
//! 여기 한 자리에만 적힌다 — 컴포저 팝업(`tui::slash`)도, 맨 터미널과 파이프
//! 두 프런트엔드의 디스패치도 이 열거에서 이름을 읽으므로 목록과 처리가
//! 갈라질 수 없다. 목록에 없는 `/이름` 은 프롬프트 명령·플러그인 명령·스킬
//! 순으로 한 번 더 찾는다([`prompt_command`]). 그 셋의 목록도 한 자리에서
//! 나온다([`callables`]) — 팝업과 `zo commands` 가 같은 줄을 읽는다.

use std::path::{Path, PathBuf};

use crate::tui::fast;

/// zo 가 실제로 처리하는 슬래시 명령.
///
/// 순서는 팝업이 그리는 순서다. `/new` 는 codex `SlashCommand` 열거와 같이
/// `/permissions` 와 `/resume` 사이에, `/clear` 는 종료 명령 뒤에 선다.
/// `/fast` 가 `/model` 바로 뒤인 것은 dynamic service-tier 삽입 순서다.
/// `/goal` 은 codex 열거를 흉내 낸 항목이 아니라 zo의 장시간 세션 확장이다.
/// 대화 상태를 바꾸는 `/compact` 뒤, 읽기 전용 `/status` 앞에 두어 "세션 제어
/// → 상태 조회" 경계를 보존한다.
///
/// `/effort` 는 **없다**. codex 에 없는 명령이고(사용자 판정: "`/effort`
/// 필요없음"), 추론 강도는 codex 와 똑같이 `/model` 피커의 2단계에서 고른다.
/// `--effort` CLI 플래그와 `Effort::from_token` 은 그대로다. `//` 는 평문
/// 이스케이프라 여기 없다 — 팝업도 `//` 로 시작하면 물러난다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slash {
    Model,
    Fast,
    /// zo's own: show or hide the model's thinking cells (t-5872). codex has
    /// no command for it — its `hide_agent_reasoning` is a config key — and
    /// it stands beside `/fast` as the other display toggle.
    Thinking,
    Permissions,
    /// `/plan off` approves the plan the model submitted in plan mode — the
    /// plan card's own words — by leaving read-only for workspace-write, as
    /// Shift+Tab steps from read-only; `/plan on` goes back to plan mode.
    Plan,
    New,
    Resume,
    Find,
    Diff,
    Fork,
    Rename,
    Export,
    /// `/copy`: the last answer, as the model wrote it, onto the clipboard —
    /// what a mouse selection of an indented transcript cannot give.
    Copy,
    Compact,
    /// `/rewind`: list the workspace checkpoints, restore guarded file
    /// writes (`/rewind 12 [force]`) or drop the last finished turn
    /// (`/rewind turn`) — the words `commands`' help already prints (t-19459).
    Rewind,
    Goal,
    Loop,
    Status,
    /// Claude Code's `/context`: what fills the context window and where each
    /// compaction tier sits (`ConversationRuntime::context_breakdown_report`).
    /// Read-only, so it stands beside `/status`.
    Context,
    /// codex 0.157.1 `/warnings`: page through the warnings this conversation
    /// has shown (F2 opens the same viewer). Read-only, so it stands beside
    /// `/status`.
    Warnings,
    Help,
    Exit,
    Clear,
}


/// The catalog as `zo commands` prints it: one row per command, in popup
/// order, with the same words the popup shows. JSON for a machine (the
/// window's composer palette), a two-column text for a person.
#[must_use]
pub fn render_catalog(json: bool) -> String {
    if json {
        let rows: Vec<serde_json::Value> = Slash::CATALOG
            .iter()
            .map(|slash| {
                serde_json::json!({
                    "name": slash.name(),
                    "description": slash.description(),
                })
            })
            .collect();
        return serde_json::Value::Array(rows).to_string();
    }
    let width = Slash::CATALOG
        .iter()
        .map(|slash| slash.name().len())
        .max()
        .unwrap_or(0);
    Slash::CATALOG
        .iter()
        .map(|slash| format!("{:<width$}  {}", slash.name(), slash.description()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A `/name` a folder can call beyond zo's own keep-list ([`Slash::CATALOG`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Callable {
    /// With its slash, as the popup prints it and the person types it.
    pub name: String,
    pub description: String,
    pub source: CallableSource,
}

/// Where a [`Callable`] comes from — the `source` `zo commands --json` writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallableSource {
    /// A command file: `commands/` in zo's home, or `.zo/commands` up the
    /// folder's tree.
    Prompt {
        path: PathBuf,
        scope: commands::PromptCommandScope,
        shadows: Vec<PathBuf>,
    },
    /// A command an enabled plugin declares.
    Plugin { plugin: String, root: Option<PathBuf> },
    /// A skill, from the catalog the `Skill` tool and the prompt index read
    /// ([`runtime::discover_all_skills`]).
    Skill { path: PathBuf },
}

impl CallableSource {
    /// The word `zo commands --json` writes as `source`.
    #[must_use]
    pub const fn word(&self) -> &'static str {
        match self {
            Self::Prompt { .. } => "prompt",
            Self::Plugin { .. } => "plugin",
            Self::Skill { .. } => "skill",
        }
    }
}

/// What [`callables`] found, and the plugin registry's error when it could
/// not be read — the folder's other commands and skills are listed anyway.
#[derive(Debug, Clone, Default)]
pub struct Callables {
    pub rows: Vec<Callable>,
    pub plugin_error: Option<String>,
}

/// The `/names` `cwd` can call beyond the keep-list, in the order
/// [`prompt_command`] asks for a name — command files, then plugins'
/// commands, then skills — each name once: a name the keep-list (its aliases
/// too) or an earlier row already holds is the earlier one's, there and on
/// submit. Discovery reads metadata only; no script, inline shell, plugin
/// initialization or model request runs here.
#[must_use]
pub fn callables(cwd: &Path) -> Callables {
    collect_callables(cwd, true)
}

/// [`callables`] without the plugins' commands: what a key may wait for. The
/// plugin registry is built by syncing the bundled plugins under a lock a
/// peer process can hold for seconds, so the composer's popup lists command
/// files and skills, read from disk, and a plugin's command still runs when
/// typed ([`prompt_command`]).
#[must_use]
pub fn callables_on_disk(cwd: &Path) -> Vec<Callable> {
    collect_callables(cwd, false).rows
}

fn collect_callables(cwd: &Path, plugins: bool) -> Callables {
    let mut seen: std::collections::BTreeSet<String> = Slash::CATALOG
        .iter()
        .map(|slash| slash.word())
        .chain(Slash::ALIASES.iter().map(|(alias, _)| *alias))
        .map(str::to_lowercase)
        .collect();
    let mut rows = Vec::new();
    for command in commands::discover_prompt_commands(cwd) {
        if !seen.insert(command.name.to_lowercase()) {
            continue;
        }
        rows.push(Callable {
            name: format!("/{}", command.name),
            description: command.summary(),
            source: CallableSource::Prompt { path: command.path, scope: command.scope, shadows: command.shadows },
        });
    }
    let registry = plugins.then(|| {
        crate::management_cli::manager(cwd)
            .and_then(|manager| manager.plugin_registry().map_err(|error| error.to_string()))
    });
    let plugin_error = match registry {
        None => None,
        Some(Ok(registry)) => {
            for (plugin, name, description) in registry.slash_command_specs() {
                let name = name.trim_start_matches('/');
                if !seen.insert(name.to_lowercase()) {
                    continue;
                }
                let root = registry.get(&plugin).and_then(|plugin| plugin.metadata().root.clone());
                rows.push(Callable {
                    name: format!("/{name}"),
                    description,
                    source: CallableSource::Plugin { plugin, root },
                });
            }
            None
        }
        Some(Err(error)) => Some(error),
    };
    for skill in runtime::discover_all_skills(cwd) {
        // A name a person cannot type as one word after the slash is not a
        // command: the line would split at its space, or read as a path.
        if skill.name.contains(|ch: char| ch.is_whitespace() || ch == '/') || !seen.insert(skill.name.to_lowercase()) {
            continue;
        }
        rows.push(Callable {
            name: format!("/{}", skill.name),
            description: skill.description.unwrap_or_default(),
            source: CallableSource::Skill { path: skill.path },
        });
    }
    Callables { rows, plugin_error }
}

/// Commands callable in this cwd: the keep-list, then [`callables`].
/// Discovery reads metadata only; no script, inline shell, plugin
/// initialization, or model request runs here.
pub fn render_available_catalog(cwd: &Path, json: bool) -> Result<String, String> {
    let found = callables(cwd);
    if let Some(error) = found.plugin_error {
        return Err(error);
    }
    let mut rows: Vec<serde_json::Value> = Slash::CATALOG.iter().map(|slash| serde_json::json!({
        "name":slash.name(), "description":slash.description(), "source":"builtin", "pipe":slash.handled_in_pipe()
    })).collect();
    for callable in found.rows {
        let mut row = serde_json::json!({
            "name": callable.name, "description": callable.description, "source": callable.source.word(),
        });
        let extra = match callable.source {
            CallableSource::Prompt { path, scope, shadows } => {
                serde_json::json!({"path": path, "scope": format!("{scope:?}"), "shadows": shadows})
            }
            CallableSource::Plugin { plugin, root } => serde_json::json!({"plugin": plugin, "path": root}),
            CallableSource::Skill { path } => serde_json::json!({"path": path}),
        };
        if let (Some(row), serde_json::Value::Object(extra)) = (row.as_object_mut(), extra) {
            row.extend(extra);
        }
        rows.push(row);
    }
    if json { Ok(serde_json::Value::Array(rows).to_string()) }
    else { Ok(rows.iter().map(|row| format!("{}  {} [{}]",row["name"].as_str().unwrap_or(""),row["description"].as_str().unwrap_or(""),row["source"].as_str().unwrap_or(""))).collect::<Vec<_>>().join("\n")) }
}

impl Slash {
    /// 팝업 순서 그대로의 전체 목록.
    pub const CATALOG: &'static [Self] = &[
        Self::Model,
        Self::Fast,
        Self::Thinking,
        Self::Permissions,
        Self::Plan,
        Self::New,
        Self::Resume,
        Self::Find,
        Self::Diff,
        Self::Fork,
        Self::Rename,
        Self::Export,
        Self::Copy,
        Self::Compact,
        Self::Rewind,
        Self::Goal,
        Self::Loop,
        Self::Status,
        Self::Context,
        Self::Warnings,
        Self::Help,
        Self::Exit,
        Self::Clear,
    ];

    /// 팝업에 찍히는 이름 — 슬래시를 포함한다.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Model => "/model",
            Self::Fast => "/fast",
            Self::Thinking => "/thinking",
            Self::Permissions => "/permissions",
            Self::Plan => "/plan",
            Self::New => "/new",
            Self::Resume => "/resume",
            Self::Find => "/find",
            Self::Diff => "/diff",
            Self::Fork => "/fork",
            Self::Rename => "/rename",
            Self::Export => "/export",
            Self::Copy => "/copy",
            Self::Compact => "/compact",
            Self::Rewind => "/rewind",
            Self::Goal => "/goal",
            Self::Loop => "/loop",
            Self::Status => "/status",
            Self::Context => "/context",
            Self::Warnings => "/warnings",
            Self::Help => "/help",
            Self::Exit => "/exit",
            Self::Clear => "/clear",
        }
    }

    /// 컴포저가 넘겨주는 낱말 — 슬래시를 뗀 [`Self::name`] 이다. 같은 사실을
    /// 두 번 적지 않으려고 여기서 자른다.
    #[must_use]
    pub fn word(self) -> &'static str {
        &self.name()[1..]
    }

    /// codex 문법의 한 줄 설명.
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            // 캡처(`codex-tui-v0.149.1-model-picker.bin`, 8.24s 프레임)의 팝업 줄.
            Self::Model => "choose what model and reasoning effort to use",
            Self::Fast => fast::COMMAND_DESCRIPTION,
            Self::Thinking => crate::tui::strings::THINKING_COMMAND_DESCRIPTION,
            Self::Permissions => "read-only, workspace-write or danger-full-access",
            Self::Plan => crate::tui::strings::PLAN_COMMAND_DESCRIPTION,
            Self::New => "start a new chat during a conversation",
            // 원본 문안 그대로(`slash_command.rs:95`), 캡처
            // `codex-tui-v0.149.1-resume-picker.bin` 의 26.07s 프레임.
            Self::Resume => "resume a saved chat",
            Self::Find => "find in saved conversation and tool output",
            Self::Diff => "review this turn’s recorded file changes",
            Self::Fork => "fork this chat into independent saved history",
            Self::Rename => "rename this saved chat",
            Self::Export => "export this chat to a new JSONL file",
            Self::Copy => "copy the last answer as markdown",
            Self::Compact => "compact the conversation to free context",
            Self::Rewind => crate::tui::strings::REWIND_COMMAND_DESCRIPTION,
            Self::Goal => "persist a goal and run bounded autonomous checks",
            Self::Loop => "repeat a prompt on a bounded session schedule",
            Self::Status => "model, account, limits, session and context",
            Self::Context => "what fills the context window and when it compacts",
            // codex `slash_command.rs:117`.
            Self::Warnings => "view retained warnings and diagnostic details",
            Self::Help => "show the shortcut card",
            Self::Exit => "exit zo",
            Self::Clear => "clear the terminal and start a new chat",
        }
    }

    /// 파이프 프런트엔드(`zo -p`, `ide/run_loop`)가 이 명령을 처리하는가.
    ///
    /// `/plan` 도 빠진다: 승인할 계획 카드는 화면에만 뜨고, 파이프의 권한은
    /// `--permission-mode` 와 `/permissions` 가 정한다.
    ///
    /// 일곱이 빠진다. `/rewind` 는 여러 턴을 오가는 대화형 명령이라 한 번 실행하고
    /// 끝나는 파이프에는 없다.
    /// (이어지는 여섯은 아래와 같다.) `/resume` 는 뷰포트를 가져가는 피커가 정본이라 append-only
    /// 스트림에는 그릴 자리가 없고, `/warnings` 도 뷰포트를 가져가는 뷰어라 같다
    /// (파이프의 경고는 이미 스트림에 한 줄씩 남는다). `/fast` 와 `/thinking` 은
    /// 맨 터미널 TUI 에만 붙은 토글이다(파이프는 `--show-thinking` 플래그가 같은
    /// 일을 한다).
    /// `/new` 와 `/clear` 는 여러 채팅을 오가는 대화형 TUI 명령이라 한 번 실행하고
    /// 끝나는 파이프 프런트엔드에는 없다.
    /// `/goal` 은 반대로 **파이프에서도 처리한다**: 상태/진행은 append-only
    /// 통지로 표현되고, 자율 턴·권한 질문은 파이프가 이미 가진 턴 드라이버와
    /// permission prompt 경로를 그대로 쓴다. 즉 별도 모달도 승인 우회도 없다.
    /// 빠진 넷은 파이프에서 '없는 명령' 과 같은 길로 나간다 — 그 사실을 여기
    /// 한 번 적어 디스패치와 `/help` 목록이 같은 답을 읽는다.
    #[must_use]
    pub const fn handled_in_pipe(self) -> bool {
        !matches!(
            self,
            Self::Fast
                | Self::Thinking
                | Self::Plan
                | Self::Rewind
                | Self::New
                | Self::Resume
                | Self::Find
                | Self::Diff
                | Self::Warnings
                | Self::Copy
                | Self::Clear
        )
    }

    /// 컴포저 한 줄이 이 명령인가 — 슬래시가 붙은 채로 묻는다(`"/exit"`).
    /// 인자는 받지 않는 자리에서 쓴다(턴 중 `/exit` 가로채기).
    #[must_use]
    pub fn is_bare_line(self, line: &str) -> bool {
        line.strip_prefix('/')
            .is_some_and(|word| Self::from_word(word) == Some(self))
    }

    /// The spellings [`Self::from_word`] takes beside the keep-list's own
    /// words: listed nowhere, still zo's, so no command file or skill may
    /// take them ([`callables`]).
    pub const ALIASES: &'static [(&'static str, Self)] = &[("quit", Self::Exit), ("permission", Self::Permissions)];

    /// 사람이 친 낱말(슬래시는 이미 떼어 낸 상태) → 명령.
    ///
    /// 별칭 둘([`Self::ALIASES`])은 목록에 없지만 받는다: `quit` 은 `/exit`,
    /// `permission` 은 `/permissions` 로 — 팝업이 권하지 않는 철자라도 친
    /// 사람의 뜻은 분명하다.
    #[must_use]
    pub fn from_word(word: &str) -> Option<Self> {
        Self::ALIASES
            .iter()
            .find(|(alias, _)| *alias == word)
            .map(|(_, command)| *command)
            .or_else(|| Self::CATALOG.iter().copied().find(|command| command.word() == word))
    }
}

/// The most one `!` line of a command file may run when a screen expands it
/// (t-81313 P3-6, decided by 4275 in m-94038): the TUI runs the line on its own
/// loop, so keys, repaint and Esc wait for as long as it runs — up to the Bash
/// tool's 120 s without this. A line that outruns it is stopped, and the screen
/// says a slow line held the window.
pub const BANG_LINE_LIMIT_ON_SCREEN_MS: u64 = 10_000;

/// The limit a command file's `!` lines run under for `origin`: the screen's
/// ([`BANG_LINE_LIMIT_ON_SCREEN_MS`]), or none of ours for a pipe, which has no
/// screen to hold still and keeps the Bash tool's own.
const fn bang_line_limit(origin: ModeOrigin) -> Option<u64> {
    if origin.piped {
        None
    } else {
        Some(BANG_LINE_LIMIT_ON_SCREEN_MS)
    }
}

/// Who stands behind the session's permission mode, as a command file's `!`
/// lines ask it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ModeOrigin {
    /// The person named the mode with `--permission-mode` on this run's
    /// command line. The default and a mode a settings file pins are not
    /// named. A pane the ZeroCode window opens is named too: the window puts
    /// `--permission-mode danger-full-access` on the command line of the zo it
    /// starts (`zerocode_core::launch`) and writes the folder's trust record
    /// beforehand (`agent_trust_presets`) — the window's own consent model,
    /// where opening a folder is trusting it.
    pub named: bool,
    /// The pipe front-end runs the command (`-p`, or no terminal): nobody
    /// is there to answer a trust prompt.
    pub piped: bool,
}

/// A `/word` expanded: the turn it runs as, and the lines the screen shows
/// once before that turn goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptExpansion {
    pub text: String,
    pub shown: Vec<String>,
}

/// The turn a `/word` the keep-list does not have runs as, when it names a
/// prompt command: that command's body with `arg` filled in, expanded against
/// `cwd`. Where commands are looked for and which wins a shared name is the
/// commands crate's ([`commands::discover_prompt_commands`]): `commands/` in
/// zo's home, then `.zo/commands/` in `cwd` and each folder above it up to the
/// repository's top or short of the person's home. The
/// keep-list is asked first, so a command file cannot shadow one of zo's own.
/// [`PromptExpansion::shown`] names the file, any it hides, and each `!` line
/// with what came of it, for the screen to show before the turn goes.
/// Its `!` commands run only what the file's `allowed-tools` grants, none
/// while `mode` is read-only, and none from a file in the working folder or
/// above it until the person has given their word for the folder that holds
/// it: trust in that folder (`workspace_trust::person_trusts_folder`)
/// or a mode they named (`origin.named`). A mode the default or a settings
/// file gives — a repository can pin its own — is not that word, and a pipe
/// (`origin.piped`) takes only a named mode: nobody there can answer for the
/// folder, so an old trust record does not speak for this run. Every `!`
/// command, a zo-home file's too, also answers to the person's permission
/// rules: a deny or an ask rule that matches leaves it unrun. Under a screen
/// (`!origin.piped`) a `!` command that runs past
/// [`BANG_LINE_LIMIT_ON_SCREEN_MS`] is stopped and said to have held the window.
/// A name no command file holds is asked of the plugins' commands, then of the
/// skills (`skill_command`) — the order [`callables`] lists them in.
#[must_use]
pub fn prompt_command(
    cwd: &Path,
    name: &str,
    arg: &str,
    mode: core_types::PermissionMode,
    origin: ModeOrigin,
) -> Option<PromptExpansion> {
    let found = commands::discover_prompt_commands(cwd);
    let Some(command) = commands::find_prompt_command(&found, name) else {
        return plugin_command(cwd, name, arg, mode, origin).or_else(|| skill_command(cwd, name, arg));
    };
    let workspace = match command.workspace_folder() {
        None => commands::WorkspaceTrust::Trusted,
        Some(_) if origin.named => commands::WorkspaceTrust::Trusted,
        // A pipe is a run with no head: nobody is there to answer for the
        // folder, so it takes the more careful side. A trust record was given
        // at another time, to what the folder held then; only the mode the
        // person names on this run's command line speaks for it (t-81313 P3-4,
        // m-81282).
        Some(_) if origin.piped => commands::WorkspaceTrust::UnnamedPipe,
        Some(folder) if crate::workspace_trust::person_trusts_folder(folder) => {
            commands::WorkspaceTrust::Trusted
        }
        Some(_) => commands::WorkspaceTrust::Untrusted,
    };
    // The rules the session's Bash was built with
    // (`conversation_support::permission_policy`), read again here because
    // a turn may be holding the session.
    let config = runtime::ConfigLoader::default_for(cwd).load();
    let unreadable = config.as_ref().err().map(ToString::to_string).unwrap_or_default();
    let gate = commands::InlineShellGate {
        read_only: mode == core_types::PermissionMode::ReadOnly,
        workspace,
        rules: config
            .as_ref()
            .map(runtime::RuntimeConfig::permission_rules)
            .map_err(|_| unreadable.as_str()),
        line_limit_ms: bang_line_limit(origin),
    };
    let rendered = command.render_prompt_traced(arg, cwd, gate);
    let whose = match command.scope {
        commands::PromptCommandScope::ZoHome => "your zo home",
        commands::PromptCommandScope::Workspace => "repository",
    };
    let mut shown = vec![format!("/{} — {} ({whose})", command.name, command.path.display())];
    shown.extend(command.shadows.iter().map(|hidden| {
        format!("  hides {} — that file's /{} does not run", hidden.display(), command.name)
    }));
    shown.extend(rendered.trace.iter().map(|line| format!("  {line}")));
    Some(PromptExpansion { text: rendered.text, shown })
}

fn plugin_command(cwd: &Path, name: &str, arg: &str, mode: core_types::PermissionMode, origin: ModeOrigin) -> Option<PromptExpansion> {
    let registry = crate::management_cli::manager(cwd).ok()?.plugin_registry().ok()?;
    let (plugin, command) = registry.find_slash_command(name)?;
    let id = plugin.metadata().id.clone();
    let config = runtime::ConfigLoader::default_for(cwd).load();
    let trusted = origin.named || (!origin.piped && crate::workspace_trust::person_trusts_folder(cwd));
    let refusal = if mode == core_types::PermissionMode::ReadOnly {
        Some("plugin scripts do not run in read-only mode".to_string())
    } else if !trusted {
        Some("plugin scripts require a trusted workspace or an explicit permission mode".to_string())
    } else {
        match config {
            Err(error) => Some(error.to_string()),
            Ok(config) => {
                let policy = runtime::PermissionPolicy::new(mode).with_permission_rules(config.permission_rules());
                let input = serde_json::json!({"command":command.command,"args":arg}).to_string();
                policy.deny_reason("Bash", &input).or_else(|| match policy.authorize("Bash", &input, None) {
                    runtime::PermissionOutcome::Allow => None,
                    runtime::PermissionOutcome::Deny { reason } => Some(reason),
                })
            }
        }
    };
    let mut shown = vec![format!("/{} — plugin {id}", command.name)];
    let text = if let Some(reason) = refusal { shown.push(reason.clone()); format!("Plugin command not run: {reason}") }
        else { match plugin.validate().and_then(|()| registry.run_slash_command(name, arg)) {
            Ok(text) => text,
            Err(error) => { shown.push(format!("plugin command failed: {error}")); format!("Plugin command failed: {error}") }
        }};
    Some(PromptExpansion { text, shown })
}

/// The placeholder a skill's body may hold for the words after its name.
const ARGUMENTS_PLACEHOLDER: &str = "$ARGUMENTS";

/// The turn `/name` runs as when it names a skill: Claude Code's `/skill` —
/// the skill's body, after the folder it lives in (so the files it points to
/// resolve), with the words after the name where its body says
/// `$ARGUMENTS`, or under `ARGUMENTS:` at its end. The line the person typed
/// heads it, so what reads the turn's first words — the saved chat's title,
/// the queue's preview — reads that line, not the skill's folder. The skill
/// is the one listed under that name ([`runtime::discover_all_skills`],
/// proposed drafts left out), or else the one in a folder of that name — the
/// name the `Skill` tool loads by; nothing in it runs here.
fn skill_command(cwd: &Path, name: &str, arg: &str) -> Option<PromptExpansion> {
    let skills = runtime::discover_all_skills(cwd);
    let in_folder = |skill: &&runtime::SkillIndexEntry| {
        skill.path.parent().and_then(Path::file_name).is_some_and(|folder| folder.eq_ignore_ascii_case(name))
    };
    let skill = skills
        .iter()
        .find(|skill| skill.name.eq_ignore_ascii_case(name))
        .or_else(|| skills.iter().find(in_folder))?;
    let content = std::fs::read_to_string(&skill.path).ok()?;
    let folder = skill.path.parent().unwrap_or(cwd);
    Some(PromptExpansion {
        text: skill_turn(&skill.name, folder, runtime::skill_body(&content), arg),
        shown: vec![format!("/{} — skill {}", skill.name, skill.path.display())],
    })
}

fn skill_turn(name: &str, folder: &Path, body: &str, arg: &str) -> String {
    let typed = if arg.is_empty() { format!("/{name}") } else { format!("/{name} {arg}") };
    let body = body.trim();
    let body = if body.contains(ARGUMENTS_PLACEHOLDER) {
        body.replace(ARGUMENTS_PLACEHOLDER, arg)
    } else if arg.is_empty() {
        body.to_string()
    } else {
        format!("{body}\n\nARGUMENTS: {arg}")
    };
    format!("{typed}\n\nBase directory for this skill: {}\n\n{body}\n", folder.display())
}

#[cfg(test)]
mod tests {
    use super::{render_catalog, ModeOrigin, Slash};

    /// [`super::prompt_command`]'s turn, in a TUI run whose mode nobody named.
    fn prompt_command(
        cwd: &std::path::Path,
        name: &str,
        arg: &str,
        mode: core_types::PermissionMode,
    ) -> Option<String> {
        super::prompt_command(cwd, name, arg, mode, ModeOrigin::default()).map(|expansion| expansion.text)
    }

    /// A command file in the working folder expands with its arguments; a
    /// word no file names is not a prompt command. The folder is a fresh one,
    /// so neither the person's home nor this checkout decides the answer.
    #[test]
    fn a_prompt_command_expands_by_its_name_from_the_working_folder() {
        let _lock = crate::test_env_lock();
        let _home = crate::support::IsolatedHome::new();
        let folder = tempfile::tempdir().expect("a working folder");
        let commands = folder.path().join(".zo").join("commands");
        std::fs::create_dir_all(&commands).expect("the command folder");
        std::fs::write(
            commands.join("fruit.md"),
            "---\ndescription: name one fruit\n---\nName the fruit $ARGUMENTS in one word.\n",
        )
        .expect("the prompt command");
        let mode = core_types::PermissionMode::WorkspaceWrite;
        let expanded =
            prompt_command(folder.path(), "fruit", "pineapple", mode).expect("/fruit expands");
        assert_eq!(expanded.trim(), "Name the fruit pineapple in one word.");
        assert_eq!(
            prompt_command(folder.path(), "FRUIT", "kiwi", mode).map(|text| text.trim().to_string()),
            Some("Name the fruit kiwi in one word.".to_string()),
            "names match without case, as the commands crate finds them"
        );
        assert_eq!(prompt_command(folder.path(), "no-such-command-z12", "", mode), None);
    }

    /// Read-only mode runs no `!` line, even one the file's `allowed-tools`
    /// grants and in a folder the person trusts; the same file in
    /// workspace-write runs it. The person's trust here is the settings that
    /// trust every folder, in a zo home of the test's own.
    #[test]
    fn a_prompt_command_runs_no_bang_line_in_read_only_mode() {
        use core_types::PermissionMode;
        let _lock = crate::test_env_lock();
        let home = crate::support::IsolatedHome::new();
        std::fs::write(
            home.zo_home.join(crate::preferences::PREFERENCES_FILE_NAME),
            r#"{"workspaceTrust":"always-full"}"#,
        )
        .expect("settings that trust every folder");
        let folder = tempfile::tempdir().expect("a working folder");
        let commands = folder.path().join(".zo").join("commands");
        std::fs::create_dir_all(&commands).expect("the command folder");
        let marker = folder.path().join("probe-ran");
        // `!` lines run in the Bash tool's POSIX shell — Git's `sh` on
        // Windows, where a backslash is an escape — so the path is spelled
        // with forward slashes there.
        let path = marker.display().to_string();
        let path = if cfg!(windows) { path.replace('\\', "/") } else { path };
        let touch = format!("touch {path}");
        std::fs::write(
            commands.join("probe.md"),
            format!("---\nallowed-tools: Bash\n---\n!{touch}\nDone.\n"),
        )
        .expect("the prompt command");

        let read_only = prompt_command(folder.path(), "probe", "", PermissionMode::ReadOnly)
            .expect("/probe expands");
        assert!(!marker.exists(), "read-only mode ran a `!` line");
        assert_eq!(read_only, format!("[!{touch}: not run in read-only mode]\nDone.\n"));

        let writable = prompt_command(folder.path(), "probe", "", PermissionMode::WorkspaceWrite)
            .expect("/probe expands");
        assert!(marker.exists(), "workspace-write did not run the granted `!` line");
        assert_eq!(writable, "\nDone.\n");
    }

    /// What the screen shows once before an expanded command's turn: the
    /// file it came from, a repository file of the same name it hides, and
    /// every `!` line with what came of it — the exit code, or the line
    /// that stood in for it.
    #[test]
    fn an_expansion_shows_its_file_what_it_hides_and_each_bang_line() {
        use core_types::PermissionMode;
        let _lock = crate::test_env_lock();
        let home = crate::support::IsolatedHome::new();
        let root = tempfile::tempdir().expect("a test root");
        let project = root.path().join("project");
        let personal = home.zo_home.join("commands");
        let repo = project.join(".zo").join("commands");
        std::fs::create_dir_all(&personal).expect("personal commands");
        std::fs::create_dir_all(&repo).expect("repo commands");
        std::fs::write(
            personal.join("probe.md"),
            "---\nallowed-tools: Bash\n---\n!echo hi\n!false\nDone.\n",
        )
        .expect("the person's command");
        std::fs::write(repo.join("probe.md"), "---\nallowed-tools: Bash\n---\n!echo repo\nRepo.\n")
            .expect("the repository's command");
        let mode = PermissionMode::WorkspaceWrite;

        let expansion = super::prompt_command(&project, "probe", "", mode, ModeOrigin::default())
            .expect("/probe expands");
        assert_eq!(expansion.text, "hi\n\nDone.\n");
        assert_eq!(
            expansion.shown,
            vec![
                format!("/probe — {} (your zo home)", personal.join("probe.md").display()),
                format!(
                    "  hides {} — that file's /probe does not run",
                    repo.join("probe.md").display()
                ),
                "  ! echo hi → exit 0".to_string(),
                "  ! false → exit 1".to_string(),
            ]
        );

        std::fs::remove_file(personal.join("probe.md")).expect("the person's command goes");
        let expansion = super::prompt_command(&project, "probe", "", mode, ModeOrigin::default())
            .expect("/probe expands");
        assert_eq!(
            expansion.shown,
            vec![
                format!("/probe — {} (repository)", repo.join("probe.md").display()),
                format!(
                    "  ! echo repo → not run — {} is not trusted; a repository's command file runs its ! lines only in a folder you trust, or under a --permission-mode you name",
                    project.display()
                ),
            ]
        );
    }

    /// The screen puts ten seconds on a `!` line; a pipe puts none of its own
    /// (t-81313 P3-6).
    #[test]
    fn the_screen_limits_a_bang_line_to_ten_seconds_and_a_pipe_does_not() {
        let screen = ModeOrigin { named: false, piped: false };
        assert_eq!(super::bang_line_limit(screen), Some(10_000));
        assert_eq!(super::bang_line_limit(ModeOrigin { named: true, ..screen }), Some(10_000));
        assert_eq!(super::bang_line_limit(ModeOrigin { piped: true, ..screen }), None);
    }

    #[test]
    fn zo_commands_prints_the_catalog_it_handles_in_popup_order() {
        let json: Vec<serde_json::Value> =
            serde_json::from_str(&render_catalog(true)).expect("zo commands --json is JSON");
        let names: Vec<&str> = json.iter().map(|row| row["name"].as_str().unwrap()).collect();
        let catalog: Vec<&str> = Slash::CATALOG.iter().map(|slash| slash.name()).collect();
        assert_eq!(names, catalog, "the JSON is the popup's order");
        assert!(
            json.iter().all(|row| !row["description"].as_str().unwrap().is_empty()),
            "every row carries the popup's description"
        );
        let text = render_catalog(false);
        assert!(text.lines().count() == Slash::CATALOG.len());
        assert!(text.starts_with("/model"), "{text}");
    }

    #[test]
    fn the_keep_list_is_the_commands_zo_handles() {
        let names: Vec<&str> = Slash::CATALOG.iter().map(|command| command.name()).collect();
        assert_eq!(
            names,
            vec![
                "/model",
                "/fast",
                "/thinking",
                "/permissions",
                "/plan",
                "/new",
                "/resume",
                "/find",
                "/diff",
                "/fork",
                "/rename",
                "/export",
                "/copy",
                "/compact",
                "/rewind",
                "/goal",
                "/loop",
                "/status",
                "/context",
                "/warnings",
                "/help",
                "/exit",
                "/clear"
            ]
        );
    }

    /// Writes `.zo/skills/<dir>/SKILL.md` under `folder`.
    fn write_skill(folder: &std::path::Path, dir: &str, content: &str) -> std::path::PathBuf {
        let skill = folder.join(".zo").join("skills").join(dir);
        std::fs::create_dir_all(&skill).expect("the skill folder");
        std::fs::write(skill.join("SKILL.md"), content).expect("the skill");
        skill
    }

    /// `/name` of a skill runs as Claude Code's `/skill` does: the folder, the
    /// body without its frontmatter, and the words after the name under
    /// `ARGUMENTS:` — or where the body says `$ARGUMENTS`. The line as typed
    /// heads the turn, so the saved chat's title is that line. The name
    /// matches without case, as a command file's does.
    #[test]
    fn a_skill_runs_by_its_name_as_its_body_with_the_words_after_it() {
        let _lock = crate::test_env_lock();
        let _home = crate::support::IsolatedHome::new();
        let folder = tempfile::tempdir().expect("a working folder");
        let skill = write_skill(
            folder.path(),
            "review-loop",
            "---\nname: review-loop\ndescription: review in two passes\n---\n# Review\n\nRead it twice.\n",
        );
        let mode = core_types::PermissionMode::WorkspaceWrite;

        let expansion = super::prompt_command(folder.path(), "Review-Loop", "the parser", mode, ModeOrigin::default())
            .expect("/review-loop runs the skill");
        assert_eq!(
            expansion.text,
            format!(
                "/review-loop the parser\n\nBase directory for this skill: {}\n\n# Review\n\nRead it twice.\n\nARGUMENTS: the parser\n",
                skill.display()
            )
        );
        assert_eq!(expansion.shown, vec![format!("/review-loop — skill {}", skill.join("SKILL.md").display())]);

        let bare = prompt_command(folder.path(), "review-loop", "", mode).expect("/review-loop alone runs it");
        assert!(bare.starts_with("/review-loop\n\n"), "{bare:?}");
        assert!(bare.ends_with("Read it twice.\n"), "no words, no ARGUMENTS line: {bare:?}");

        write_skill(folder.path(), "fill-in", "---\ndescription: d\n---\nCheck $ARGUMENTS now.\n");
        assert_eq!(
            prompt_command(folder.path(), "fill-in", "the lexer", mode).as_deref().and_then(|text| text.lines().last()),
            Some("Check the lexer now.")
        );
    }

    /// A skill whose listed name cannot be typed as one word still runs by its
    /// folder's name — the name the `Skill` tool loads it by.
    #[test]
    fn a_skill_runs_by_its_folders_name_too() {
        let _lock = crate::test_env_lock();
        let _home = crate::support::IsolatedHome::new();
        let folder = tempfile::tempdir().expect("a working folder");
        write_skill(folder.path(), "release-notes", "---\nname: Release Notes\ndescription: d\n---\nWrite them.\n");
        let mode = core_types::PermissionMode::WorkspaceWrite;
        assert!(super::callables_on_disk(folder.path()).is_empty(), "a spaced name is not listed");
        let text = prompt_command(folder.path(), "release-notes", "", mode).expect("/release-notes runs the skill");
        assert!(text.ends_with("Write them.\n"), "{text:?}");
    }

    /// A proposed draft is not a skill anyone runs yet: `/name` finds nothing.
    #[test]
    fn a_proposed_skill_does_not_run() {
        let _lock = crate::test_env_lock();
        let _home = crate::support::IsolatedHome::new();
        let folder = tempfile::tempdir().expect("a working folder");
        write_skill(folder.path(), "draft", "---\ndescription: d\nstate: proposed\n---\nNot yet.\n");
        let mode = core_types::PermissionMode::WorkspaceWrite;
        assert_eq!(prompt_command(folder.path(), "draft", "", mode), None);
        assert!(super::callables(folder.path()).rows.iter().all(|row| row.name != "/draft"));
    }

    /// The list beyond the keep-list is the order a name is looked for in:
    /// command files, then skills. A name the keep-list or a command file
    /// holds is theirs — listed once, run as theirs — and a name nobody can
    /// type as one word is not listed.
    #[test]
    fn callables_list_command_files_then_skills_each_name_once() {
        use super::CallableSource;
        let _lock = crate::test_env_lock();
        let _home = crate::support::IsolatedHome::new();
        let folder = tempfile::tempdir().expect("a working folder");
        let commands = folder.path().join(".zo").join("commands");
        std::fs::create_dir_all(&commands).expect("the command folder");
        std::fs::write(commands.join("shared.md"), "---\ndescription: the file's\n---\nThe file's turn.\n")
            .expect("a command file");
        write_skill(folder.path(), "shared", "---\ndescription: the skill's\n---\nThe skill's turn.\n");
        write_skill(folder.path(), "model", "---\ndescription: not zo's /model\n---\nNo.\n");
        write_skill(folder.path(), "quit", "---\ndescription: not zo's /exit\n---\nNo.\n");
        write_skill(folder.path(), "spaced", "---\nname: two words\ndescription: d\n---\nNo.\n");
        write_skill(folder.path(), "audit", "---\ndescription: audit the change\n---\nAudit.\n");

        let found = super::callables(folder.path());
        assert_eq!(found.plugin_error, None);
        let rows: Vec<(&str, &str, &str)> = found
            .rows
            .iter()
            .map(|row| (row.name.as_str(), row.description.as_str(), row.source.word()))
            .collect();
        assert_eq!(rows, vec![("/shared", "the file's", "prompt"), ("/audit", "audit the change", "skill")]);
        assert!(matches!(&found.rows[1].source, CallableSource::Skill { path } if path.ends_with("audit/SKILL.md")));

        let mode = core_types::PermissionMode::WorkspaceWrite;
        assert_eq!(prompt_command(folder.path(), "shared", "", mode).as_deref().map(str::trim), Some("The file's turn."));
    }

    /// `zo commands --json` — what the window's palette reads — carries the
    /// skills, each with its `source` and path, after zo's own commands.
    #[test]
    fn zo_commands_json_lists_the_skills_after_the_keep_list() {
        let _lock = crate::test_env_lock();
        let _home = crate::support::IsolatedHome::new();
        let folder = tempfile::tempdir().expect("a working folder");
        let skill = write_skill(folder.path(), "audit", "---\ndescription: audit the change\n---\nAudit.\n");

        let json: Vec<serde_json::Value> =
            serde_json::from_str(&super::render_available_catalog(folder.path(), true).expect("the catalog"))
                .expect("zo commands --json is JSON");
        assert_eq!(json.len(), Slash::CATALOG.len() + 1);
        let row = &json[Slash::CATALOG.len()];
        assert_eq!(row["name"], "/audit");
        assert_eq!(row["description"], "audit the change");
        assert_eq!(row["source"], "skill");
        assert_eq!(row["path"], skill.join("SKILL.md").display().to_string());
    }
}
