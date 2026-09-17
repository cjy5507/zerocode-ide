//! zo 가 실제로 처리하는 슬래시 명령의 **정본 목록**.
//!
//! 여기 한 자리에만 적힌다 — 컴포저 팝업(`tui::slash`)도, 맨 터미널과 파이프
//! 두 프런트엔드의 디스패치도 이 열거에서 이름을 읽으므로 목록과 처리가
//! 갈라질 수 없다.

use crate::tui::fast;

/// zo 가 실제로 처리하는 슬래시 열하나.
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
    Permissions,
    New,
    Resume,
    Compact,
    Goal,
    Loop,
    Status,
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
        Self::Permissions,
        Self::New,
        Self::Resume,
        Self::Compact,
        Self::Goal,
        Self::Loop,
        Self::Status,
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
            Self::Permissions => "/permissions",
            Self::New => "/new",
            Self::Resume => "/resume",
            Self::Compact => "/compact",
            Self::Goal => "/goal",
            Self::Loop => "/loop",
            Self::Status => "/status",
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
            Self::Permissions => "read-only, workspace-write or danger-full-access",
            Self::New => "start a new chat during a conversation",
            // 원본 문안 그대로(`slash_command.rs:95`), 캡처
            // `codex-tui-v0.149.1-resume-picker.bin` 의 26.07s 프레임.
            Self::Resume => "resume a saved chat",
            Self::Compact => "compact the conversation to free context",
            Self::Goal => "persist a goal and run bounded autonomous checks",
            Self::Loop => "repeat a prompt on a bounded session schedule",
            Self::Status => "model, account, limits, session and context",
            Self::Help => "show the shortcut card",
            Self::Exit => "exit zo",
            Self::Clear => "clear the terminal and start a new chat",
        }
    }

    /// 파이프 프런트엔드(`zo -p`, `ide/run_loop`)가 이 명령을 처리하는가.
    ///
    /// 넷이 빠진다. `/resume` 는 뷰포트를 가져가는 피커가 정본이라 append-only
    /// 스트림에는 그릴 자리가 없고, `/fast` 는 맨 터미널 TUI 에만 붙은 토글이다.
    /// `/new` 와 `/clear` 는 여러 채팅을 오가는 대화형 TUI 명령이라 한 번 실행하고
    /// 끝나는 파이프 프런트엔드에는 없다.
    /// `/goal` 은 반대로 **파이프에서도 처리한다**: 상태/진행은 append-only
    /// 통지로 표현되고, 자율 턴·권한 질문은 파이프가 이미 가진 턴 드라이버와
    /// permission prompt 경로를 그대로 쓴다. 즉 별도 모달도 승인 우회도 없다.
    /// 빠진 넷은 파이프에서 '없는 명령' 과 같은 길로 나간다 — 그 사실을 여기
    /// 한 번 적어 디스패치와 `/help` 목록이 같은 답을 읽는다.
    #[must_use]
    pub const fn handled_in_pipe(self) -> bool {
        !matches!(self, Self::Fast | Self::New | Self::Resume | Self::Clear)
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

#[cfg(test)]
mod tests {
    use super::{render_catalog, Slash};
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
    fn the_keep_list_is_the_eleven_commands_zo_handles() {
        let names: Vec<&str> = Slash::CATALOG.iter().map(|command| command.name()).collect();
        assert_eq!(
            names,
            vec![
                "/model",
                "/fast",
                "/permissions",
                "/new",
                "/resume",
                "/compact",
                "/goal",
                "/loop",
                "/status",
                "/help",
                "/exit",
                "/clear"
            ]
        );
    }
}
