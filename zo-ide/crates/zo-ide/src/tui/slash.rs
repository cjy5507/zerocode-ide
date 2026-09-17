//! 컴포저 자동완성 팝업의 재료 — 목록 자체는 [`crate::slash`] 가 든다.
//!
//! 캡처(`docs/captures/codex-tui-v0.149.1-model-picker.bin`, 8.24s 프레임)에서
//! codex 는 `/model` 을 타이핑하는 동안 컴포저 **아래**(푸터 자리)에 한 줄
//! 팝업을 띄운다:
//!
//! ```text
//! [21;3H ESC[1m ESC[38;5;6m /model  choose what model and reasoning effort to use
//! ```
//!
//! 곧 이름과 설명이 두 칸 사이를 두고 나란히 서고, **고른 줄은 줄 전체가
//! bold + 시안**이다(같은 캡처의 피커도 고른 줄에 같은 스타일을 쓴다).

use crate::slash::Slash;

/// What a submitted line is, read from its head.
///
/// `/` opens a slash command — but it also opens every Unix absolute path,
/// and a person who pastes `/Users/dev/Desktop/web.config 이 파일 확인해봐`
/// means the path, not a command named `Users/joe/Desktop/web.config` (live
/// report 2026-09-09: zo answered "is not in zo" instead of reading the
/// file). A command is ONE word after the slash, so a first word holding
/// another `/` is a path; and a first word that names something on disk
/// (`/tmp 정리해줘`) is a path too — `on_disk` answers that, so the shape
/// rule stays pure and the filesystem is asked only at the door.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Line<'a> {
    /// Does not start with `/`.
    Plain,
    /// `//text` — the escape for a line that really starts with `/`; carries
    /// the text with one slash removed.
    Escaped(&'a str),
    /// A filesystem path at the head of the line — goes to the model as text.
    Path,
    /// A slash command: the text after the slash, arguments included.
    Command(&'a str),
}

#[must_use]
pub fn classify(text: &str, on_disk: impl Fn(&str) -> bool) -> Line<'_> {
    let Some(after) = text.strip_prefix('/') else {
        return Line::Plain;
    };
    if after.starts_with('/') {
        return Line::Escaped(after);
    }
    let first_word = after
        .split(char::is_whitespace)
        .next()
        .unwrap_or_default();
    if first_word.contains('/') || (!first_word.is_empty() && on_disk(&text[..=first_word.len()])) {
        return Line::Path;
    }
    Line::Command(after)
}

/// 컴포저 원문에 맞는 명령들. 팝업이 뜰 자리가 아니면 빈 벡터다.
///
/// 뜨는 조건은 codex 와 같다: `/` 로 시작하고, 아직 인자를 안 쓴 한 낱말이며,
/// `//`(평문 이스케이프)가 아니다.
#[must_use]
pub fn matches(text: &str) -> Vec<Slash> {
    matches_for_model(text, true)
}

/// Keep-list matches gated by the current model capability. Codex's fast
/// command is dynamic: unsupported models do not get a `/fast` suggestion and
/// a manually submitted command follows the same unrecognized-command path.
#[must_use]
pub fn matches_for_model(text: &str, fast_supported: bool) -> Vec<Slash> {
    // The popup asks no filesystem: a path-shaped word is enough to close it,
    // and a one-word root directory simply matches no command below.
    let Line::Command(_) = classify(text, |_| false) else {
        return Vec::new();
    };
    if text.chars().any(char::is_whitespace) {
        return Vec::new();
    }
    Slash::CATALOG
        .iter()
        .copied()
        .filter(|command| *command != Slash::Fast || fast_supported)
        .filter(|command| command.name().starts_with(text))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{Line, classify, matches, matches_for_model};
    use crate::slash::Slash;
    /// 순서는 codex built-in 목록과 동적 `/fast` 항목의 상대 순서다.
    /// 캡처(`codex-tui-v0.149.1-resume-picker.bin`, 26.07s)의 팝업 줄:
    /// `ESC[1mESC[38;5;6;49m/resume  resume a saved chat`.
    #[test]
    fn resume_carries_the_captured_description() {
        let hits = matches("/res");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name(), "/resume");
        assert_eq!(hits[0].description(), "resume a saved chat");
    }

    #[test]
    fn new_and_clear_carry_the_codex_descriptions() {
        let new = matches("/new");
        assert_eq!(new, [Slash::New]);
        assert_eq!(new[0].description(), "start a new chat during a conversation");

        let clear = matches("/clear");
        assert_eq!(clear, [Slash::Clear]);
        assert_eq!(
            clear[0].description(),
            "clear the terminal and start a new chat"
        );
    }

    #[test]
    fn a_bare_slash_offers_every_command() {
        assert_eq!(matches("/").len(), Slash::CATALOG.len());
    }

    #[test]
    fn fast_is_hidden_for_models_without_the_capability() {
        assert!(matches_for_model("/", false)
            .iter()
            .all(|command| command.name() != "/fast"));
        assert!(matches_for_model("/fa", false).is_empty());
        assert_eq!(matches_for_model("/fa", true)[0].name(), "/fast");
    }

    #[test]
    fn typing_narrows_the_list() {
        let hits = matches("/mo");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name(), "/model");
        assert_eq!(hits[0].description(), "choose what model and reasoning effort to use");
    }

    #[test]
    fn the_literal_escape_and_arguments_retire_the_popup() {
        assert!(matches("//model").is_empty());
        assert!(matches("/model opus").is_empty());
        assert!(matches("hello").is_empty());
    }

    #[test]
    fn an_unknown_command_offers_nothing() {
        assert!(matches("/zzz").is_empty());
    }

    /// codex 에 없는 명령이라 뺐다 — 팝업도 그것을 모른다.
    #[test]
    fn effort_is_no_longer_a_command() {
        assert!(matches("/effort").is_empty());
        assert!(!Slash::CATALOG.iter().any(|command| command.name() == "/effort"));
    }

    /// 사용자가 문장 머리에 절대경로를 붙여 넣었다 — `/Users/dev/Desktop/web.config
    /// 이 파일 확인해봐`(라이브 보고 2026-09-09). 첫 낱말 안에 `/` 가 더 있으면
    /// 명령 이름이 아니라 경로다. 모델로 보내야 하고, 「zo 에 없는 명령」이라
    /// 거절하면 안 된다.
    #[test]
    fn a_pasted_absolute_path_is_not_a_command() {
        let never = |_: &str| false;
        assert_eq!(
            classify("/Users/dev/Desktop/web.config 이 파일 확인해봐", never),
            Line::Path
        );
        assert_eq!(classify("/Users/dev/Desktop/web.config", never), Line::Path);
        assert!(matches("/Users/dev/x").is_empty());
    }

    /// `/tmp 정리해줘` — 한 낱말이라 모양만으로는 명령과 같다. 디스크에 그 이름이
    /// 있으면 경로다; 없으면(가짜 `/resume` 디렉터리는 없다) 명령이다.
    #[test]
    fn a_root_directory_at_the_head_is_a_path_only_when_it_exists() {
        assert_eq!(classify("/tmp 정리해줘", |path| path == "/tmp"), Line::Path);
        assert_eq!(
            classify("/tmp 정리해줘", |_| false),
            Line::Command("tmp 정리해줘")
        );
        assert_eq!(classify("/resume", |path| path == "/tmp"), Line::Command("resume"));
    }

    /// 명령·이스케이프·평문은 그대로다 — 빈 슬래시는 팝업을 여는 명령이다.
    #[test]
    fn commands_the_escape_and_plain_text_are_untouched() {
        let never = |_: &str| false;
        assert_eq!(classify("/resume", never), Line::Command("resume"));
        assert_eq!(classify("/model gpt-5", never), Line::Command("model gpt-5"));
        assert_eq!(classify("/", never), Line::Command(""));
        assert_eq!(classify("//literal slash", never), Line::Escaped("/literal slash"));
        assert_eq!(classify("hello", never), Line::Plain);
    }
}
