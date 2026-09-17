//! `AskUserQuestion` 오버레이의 재료 — 정본은 codex
//! `tui/src/bottom_pane/request_user_input/`.
//!
//! 그 오버레이는 질문 여러 개를 한 번에 물어보고(`Question 1/2`), 보기마다
//! 메모를 달 수 있고(`tab to add notes`), 시간이 지나면 스스로 답을 낸다
//! (`auto-resolves in 1m 00s`). zo 의 `AskUserQuestion` 은 한 번에 **질문
//! 하나**를 묻고 메모도 자동 해소도 없다 — 그래서 여기서는 그 화면의
//! **문법만** 옮기고, 우리가 실제로 하는 말만 남긴다.
//!
//! 옮겨 온 것과 그 출처:
//!
//! | 화면 요소 | codex 출처 |
//! |---|---|
//! | `Question 1/1 (1 unanswered)` (dim) | `progress_prefix_text()` |
//! | ` · <chip>` 꼬리 | `render_ui_at` 의 countdown 꼬리(`" · ".dim()`) |
//! | 질문 줄이 시안 | `render_ui_at`: `if answered { plain } else { cyan }` |
//! | `› 1. <label>  <설명>` 행 | `option_rows()` + `render_rows` |
//! | 마지막 `None of the above` 행 | `is_other` + `idx == options.len()` |
//! | `[x]`/`[ ]` 표시 | `multi_select_picker.rs::build_rows` |
//! | `a | b | c` 푸터 | `footer_tips()` + `TIP_SEPARATOR` |
//!
//! **다중 선택**: codex 의 `request_user_input` 에는 쌍둥이가 없다 — 질문
//! 하나에 답 하나다(`option_rows` 의 `selected_idx: Option<usize>`,
//! `select_current_option`). 있다고 지어내지 않고, codex 가 여럿을 고르게 할
//! 때 쓰는 위젯(`bottom_pane/multi_select_picker.rs`)의 문법만 최소한으로
//! 얹었다: 라벨 앞의 `[x]`/`[ ]` 와 space 토글이다("Each row shows:
//! `› [x] Item Name` where `›` indicates cursor position and `[x]` or `[ ]`
//! indicates enabled/disabled state"). 번호는 `request_user_input` 쪽에서
//! 그대로 온다 — 파이프 로드가 이미 번호로 답을 받기 때문이다
//! (`ide::prompt::parse_question_answer`).

use runtime::message_stream::{QuestionOption, UserQuestionPrompt};

use super::view::{PickerRow, Question, Tip};

const OTHER_OPTION_LABEL: &str = "None of the above";
const OTHER_OPTION_DESCRIPTION: &str = "Type your own answer.";

/// 파킹된 질문 하나의 진행 줄. codex `progress_prefix_text()` 는 답이 없는
/// 동안 `(N unanswered)` 를 달고, zo 는 언제나 질문 하나가 파킹된 상태다.
/// `chip` 은 `AskUserQuestion` 의 `header` — codex 에는 없는 자리라
/// 진행 줄의 꼬리(countdown 이 앉는 그 자리, `" · "` dim)에 붙인다.
#[must_use]
pub fn progress(chip: Option<&str>) -> String {
    let base = "Question 1/1 (1 unanswered)".to_string();
    match chip.map(str::trim).filter(|chip| !chip.is_empty()) {
        Some(chip) => format!("{base} · {chip}"),
        None => base,
    }
}

/// 보기를 피커 행으로. 설명이 없으면 오른쪽 열은 비운다 — codex 의
/// `option_rows` 도 `description` 을 그대로 흘린다. 고정 보기가 하나라도 있으면
/// zo 스키마에 없는 `is_other` 를 항상 켠 것으로 읽어 마지막 자유 입력 행을 붙인다.
#[must_use]
pub fn rows(options: &[QuestionOption]) -> Vec<PickerRow> {
    let mut rows = options
        .iter()
        .map(|option| PickerRow {
            label: crate::util::ansi::sanitize_inline(&option.label),
            description: option
                .description
                .as_deref()
                .map(crate::util::ansi::sanitize_inline)
                .unwrap_or_default(),
            dim: false,
        })
        .collect::<Vec<_>>();
    if !rows.is_empty() {
        rows.push(PickerRow {
            label: OTHER_OPTION_LABEL.to_string(),
            description: OTHER_OPTION_DESCRIPTION.to_string(),
            dim: false,
        });
    }
    rows
}

/// 푸터 힌트. 문법은 codex `footer_tips()` — 지금 눌러야 할 키 하나만
/// 강조하고(`FooterTip::highlighted`) 나머지는 dim 이다. 우리에게 없는
/// 힌트(`tab to add notes`·`←/→ to navigate questions`)는 짓지 않는다.
#[must_use]
pub fn tips(has_options: bool, multi_select: bool) -> Vec<Tip> {
    let mut out = Vec::new();
    if has_options && multi_select {
        // codex `multi_select_picker` 의 기본 안내는 "Press space to toggle".
        out.push(Tip::new("space to toggle"));
    }
    out.push(Tip::highlighted("enter to submit answer"));
    out.push(Tip::new("esc to interrupt"));
    out
}

/// 파킹할 질문의 화면.
#[must_use]
pub fn view(prompt: &UserQuestionPrompt) -> Question {
    let rows = rows(&prompt.options);
    let has_options = !rows.is_empty();
    let multi_select = has_options && prompt.multi_select;
    Question {
        progress: progress(prompt.header.as_deref()),
        question: crate::util::ansi::sanitize_inline(&prompt.question),
        checked: if multi_select {
            vec![false; prompt.options.len()]
        } else {
            Vec::new()
        },
        other_index: has_options.then_some(prompt.options.len()),
        other_input: false,
        rows,
        selected: 0,
        tips: tips(has_options, multi_select),
    }
}

#[cfg(test)]
mod tests {
    use super::{progress, rows, tips, view};
    use runtime::message_stream::{BlockId, QuestionOption, UserQuestionPrompt};

    fn prompt(options: Vec<QuestionOption>, multi: bool) -> UserQuestionPrompt {
        let (responder, _rx) = tokio::sync::oneshot::channel();
        UserQuestionPrompt {
            id: BlockId(7),
            question: "Which auth method?".to_string(),
            header: Some("Auth".to_string()),
            options,
            multi_select: multi,
            responder,
        }
    }

    #[test]
    fn the_progress_row_is_the_codex_one_plus_our_chip() {
        assert_eq!(progress(None), "Question 1/1 (1 unanswered)");
        assert_eq!(progress(Some("  ")), "Question 1/1 (1 unanswered)");
        assert_eq!(progress(Some("Auth")), "Question 1/1 (1 unanswered) · Auth");
    }

    #[test]
    fn a_row_carries_the_label_and_its_one_line() {
        let built = rows(&[
            QuestionOption {
                label: "OAuth".to_string(),
                description: Some("Browser flow.".to_string()),
                preview: None,
            },
            QuestionOption::plain("API key"),
        ]);
        assert_eq!(built[0].label, "OAuth");
        assert_eq!(built[0].description, "Browser flow.");
        assert_eq!(built[1].label, "API key");
        assert_eq!(built[1].description, "");
    }

    #[test]
    fn an_option_question_renders_the_codex_other_row_last() {
        let view = view(&prompt(
            vec![QuestionOption::plain("OAuth"), QuestionOption::plain("API key")],
            false,
        ));

        let plain = view
            .lines(80, 24)
            .iter()
            .map(super::super::ansi::Line::plain)
            .collect::<Vec<_>>();

        assert!(
            plain.iter().any(|line| line.contains("3. None of the above")),
            "other row was missing from {plain:?}"
        );
    }

    /// 없는 힌트를 짓지 않는다 — 우리에게 메모도 다음 질문도 없다.
    #[test]
    fn the_tips_say_only_what_zo_can_do() {
        let single_tips = tips(true, false);
        let single: Vec<&str> = single_tips.iter().map(|tip| tip.text.as_str()).collect();
        assert_eq!(single, vec!["enter to submit answer", "esc to interrupt"]);
        let multi_tips = tips(true, true);
        let multi: Vec<&str> = multi_tips.iter().map(|tip| tip.text.as_str()).collect();
        assert_eq!(
            multi,
            vec!["space to toggle", "enter to submit answer", "esc to interrupt"]
        );
        assert!(tips(true, false)[0].highlight, "the submit key is the lit one");
        assert!(!tips(true, false)[1].highlight);
    }

    #[test]
    fn a_multi_select_view_starts_with_every_box_empty() {
        let view = view(&prompt(
            vec![QuestionOption::plain("a"), QuestionOption::plain("b")],
            true,
        ));
        assert!(view.is_multi());
        assert_eq!(view.checked, vec![false, false]);
        assert_eq!(view.answers(), vec!["a".to_string()], "커서 줄이 기본 답");
    }

    #[test]
    fn a_freeform_view_has_no_rows_and_no_toggle_hint() {
        let view = view(&prompt(Vec::new(), false));
        assert!(view.is_freeform());
        assert!(!view.is_multi());
        assert_eq!(
            view.tips.iter().map(|tip| tip.text.as_str()).collect::<Vec<_>>(),
            vec!["enter to submit answer", "esc to interrupt"]
        );
    }

    /// 다중 선택으로 왔어도 보기가 없으면 자유 서술이다 — 켤 상자가 없다.
    #[test]
    fn multi_select_without_options_is_still_freeform() {
        let view = view(&prompt(Vec::new(), true));
        assert!(view.is_freeform());
        assert!(!view.is_multi());
    }
}
