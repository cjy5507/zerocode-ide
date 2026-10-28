//! zo 가 실제로 처리하는 슬래시 명령의 **정본 목록**.
//!
//! 여기 한 자리에만 적힌다 — 컴포저 팝업(`tui::slash`)도, 맨 터미널과 파이프
//! 두 프런트엔드의 디스패치도 이 열거에서 이름을 읽으므로 목록과 처리가
//! 갈라질 수 없다. 목록에 없는 `/이름` 은 프롬프트 명령으로 한 번 더 찾는다
//! ([`prompt_command`]).

use std::path::Path;

use crate::tui::fast;

/// zo 가 실제로 처리하는 슬래시 열여섯.
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
    New,
    Resume,
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

impl Slash {
    /// 팝업 순서 그대로의 전체 목록.
    pub const CATALOG: &'static [Self] = &[
        Self::Model,
        Self::Fast,
        Self::Thinking,
        Self::Permissions,
        Self::New,
        Self::Resume,
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
            Self::New => "/new",
            Self::Resume => "/resume",
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
            Self::New => "start a new chat during a conversation",
            // 원본 문안 그대로(`slash_command.rs:95`), 캡처
            // `codex-tui-v0.149.1-resume-picker.bin` 의 26.07s 프레임.
            Self::Resume => "resume a saved chat",
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
                | Self::Rewind
                | Self::New
                | Self::Resume
                | Self::Warnings
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

    /// 사람이 친 낱말(슬래시는 이미 떼어 낸 상태) → 명령.
    ///
    /// 별칭 둘은 목록에 없지만 받는다: `quit` 은 `/exit`, `permission` 은
    /// `/permissions` 로 — 팝업이 권하지 않는 철자라도 친 사람의 뜻은 분명하다.
    #[must_use]
    pub fn from_word(word: &str) -> Option<Self> {
        match word {
            "quit" => Some(Self::Exit),
            "permission" => Some(Self::Permissions),
            other => Self::CATALOG
                .iter()
                .copied()
                .find(|command| command.word() == other),
        }
    }
}

/// Who stands behind the session's permission mode, as a command file's `!`
/// lines ask it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ModeOrigin {
    /// The person named the mode with `--permission-mode` on this run's
    /// command line. The default and a mode a settings file pins are not
    /// named.
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
/// commands crate's ([`commands::discover_prompt_commands`]): `.zo/commands/`
/// in `cwd` and every folder above it, then `commands/` in zo's home. The
/// keep-list is asked first, so a command file cannot shadow one of zo's own.
/// Its `!` commands run only what the file's `allowed-tools` grants, none
/// while `mode` is read-only, and none from a file in the working folder or
/// above it until the person has given their word for the folder that holds
/// it: trust in that folder ([`crate::workspace_trust::person_trusts_folder`])
/// or a mode they named (`origin.named`). A mode the default or a settings
/// file gives — a repository can pin its own — is not that word, and a pipe
/// (`origin.piped`) takes only a named mode: nobody there can answer for the
/// folder, so an old trust record does not speak for this run. Every `!`
/// command, a zo-home file's too, also answers to the person's permission
/// rules: a deny or an ask rule that matches leaves it unrun.
#[must_use]
pub fn prompt_command(
    cwd: &Path,
    name: &str,
    arg: &str,
    mode: core_types::PermissionMode,
    origin: ModeOrigin,
) -> Option<PromptExpansion> {
    let found = commands::discover_prompt_commands(cwd);
    let command = commands::find_prompt_command(&found, name)?;
    let workspace = match command.workspace_folder() {
        None => commands::WorkspaceTrust::Trusted,
        Some(_) if origin.named => commands::WorkspaceTrust::Trusted,
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
    };
    Some(PromptExpansion {
        text: command.render_prompt_as(arg, cwd, gate),
        shown: Vec::new(),
    })
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
        use crate::support::EnvVarGuard;
        use core_types::PermissionMode;
        let _lock = crate::test_env_lock();
        let zo_home = tempfile::tempdir().expect("a zo home");
        std::fs::write(
            zo_home.path().join(crate::preferences::PREFERENCES_FILE_NAME),
            r#"{"workspaceTrust":"always-full"}"#,
        )
        .expect("settings that trust every folder");
        let _zo_home = EnvVarGuard::set("ZO_CONFIG_HOME", zo_home.path().to_str());
        let _no_zo_home = EnvVarGuard::set("ZO_HOME", None);
        let folder = tempfile::tempdir().expect("a working folder");
        let commands = folder.path().join(".zo").join("commands");
        std::fs::create_dir_all(&commands).expect("the command folder");
        let marker = folder.path().join("probe-ran");
        let touch = format!("touch {}", marker.display());
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
    fn the_keep_list_is_the_sixteen_commands_zo_handles() {
        let names: Vec<&str> = Slash::CATALOG.iter().map(|command| command.name()).collect();
        assert_eq!(
            names,
            vec![
                "/model",
                "/fast",
                "/thinking",
                "/permissions",
                "/new",
                "/resume",
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
}
