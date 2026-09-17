//! 권한·질문 텍스트 응답 — 파킹된 프롬프트와 그 답 해석.
//!
//! 렌더러는 `PermissionPrompt`/`UserQuestionPrompt` 블록을 화면에 찍고 **파킹만**
//! 한다(responder 를 drop 하면 hard deny). 답은 다음 stdin 줄에서 온다:
//! 권한은 선택지의 키 글자(zo TUI `key_to_permission_decision` 과 같은 어휘 —
//! `y` once · `a` always · `n` deny · Enter = deny), 질문은 번호(쉼표 다중)·
//! 라벨·자유 텍스트(`tui/modals/user_question.rs` 규칙 준용).

use runtime::message_stream::{
    PermissionDecision as RenderPermissionDecision, PermissionPrompt, UserQuestionPrompt,
};

/// 응답을 기다리는 프롬프트 — 정본은 렌더러(`ide::render`)가 정의하고 파킹해
/// 넘기는 그 타입이다. 여기서는 답을 채워 해소하는 메서드만 얹는다.
pub use crate::ide::render::PendingPrompt;

impl PendingPrompt {
    /// 한 줄을 답으로 해석해 해소를 시도한다. `None` = 해소됨, `Some(self)` =
    /// 이 줄은 답이 아니었다(프롬프트 유지, 호출자가 `hint()` 로 재안내).
    #[must_use]
    pub fn answer(self, line: &str) -> Option<PendingPrompt> {
        match self {
            PendingPrompt::Permission(prompt) => match parse_permission_answer(line, &prompt) {
                Some(decision) => {
                    let _ = prompt.responder.send(decision);
                    None
                }
                None => Some(PendingPrompt::Permission(prompt)),
            },
            PendingPrompt::Question(prompt) => match parse_question_answer(line, &prompt) {
                Some(answers) => {
                    let _ = prompt.responder.send(answers);
                    None
                }
                None => Some(PendingPrompt::Question(prompt)),
            },
        }
    }

    /// 프롬프트를 답 없이 닫는다 — 권한은 명시적 Deny, 질문은 responder drop.
    pub fn dismiss(self) {
        match self {
            PendingPrompt::Permission(prompt) => {
                let _ = prompt.responder.send(RenderPermissionDecision::Deny);
            }
            PendingPrompt::Question(prompt) => drop(prompt),
        }
    }

    /// 재안내용 한 줄 힌트.
    #[must_use]
    pub fn hint(&self) -> String {
        match self {
            PendingPrompt::Permission(prompt) => {
                let keys = prompt
                    .choices
                    .iter()
                    .map(|choice| format!("[{}] {}", choice.key, choice.label))
                    .collect::<Vec<_>>()
                    .join("  ");
                format!("{keys}  (Enter = deny)")
            }
            // 파킹 줄과 **같은 어휘**여야 한다 — 다시 안내하는 자리에서 답하는
            // 법이 달라지면 그 자체가 거짓말이다(`ide::render::question_keys`).
            PendingPrompt::Question(prompt) => {
                let count = prompt.options.len();
                if count == 0 {
                    "free text".to_string()
                } else if prompt.multi_select {
                    format!("[1]-[{count}] (comma for several) or free text")
                } else {
                    format!("[1]-[{count}] or free text")
                }
            }
        }
    }
}

/// 권한 답 해석. 빈 줄은 Deny(zo TUI 의 기본 커서와 동일). 키 글자는
/// 대소문자 무시로 선택지의 `key` 와 맞춘다; 모르는 글자는 `None`.
#[must_use]
pub fn parse_permission_answer(
    line: &str,
    prompt: &PermissionPrompt,
) -> Option<RenderPermissionDecision> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Some(RenderPermissionDecision::Deny);
    }
    let mut chars = trimmed.chars();
    let key = chars.next()?.to_ascii_lowercase();
    if chars.next().is_some() {
        return None;
    }
    prompt
        .choices
        .iter()
        .find(|choice| choice.key.to_ascii_lowercase() == key)
        .map(|choice| choice.decision)
}

/// 질문 답 해석. 번호(1-based, 쉼표 다중)는 라벨로 바뀌고, 그 외 텍스트는
/// 자유 답으로 그대로 간다. 빈 줄은 답이 아니다.
#[must_use]
pub fn parse_question_answer(line: &str, prompt: &UserQuestionPrompt) -> Option<Vec<String>> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    let picks: Option<Vec<usize>> = trimmed
        .split(',')
        .map(|part| part.trim().parse::<usize>().ok())
        .collect();
    if let Some(picks) = picks {
        let labels: Option<Vec<String>> = picks
            .iter()
            .map(|pick| {
                pick.checked_sub(1)
                    .and_then(|index| prompt.options.get(index))
                    .map(|option| option.label.clone())
            })
            .collect();
        if let Some(labels) = labels.filter(|labels| !labels.is_empty()) {
            if prompt.multi_select || labels.len() == 1 {
                return Some(labels);
            }
        }
        return None;
    }
    Some(vec![trimmed.to_string()])
}

#[cfg(test)]
mod tests {
    use super::{parse_permission_answer, parse_question_answer};
    use runtime::message_stream::{
        BlockId, PermissionChoice, PermissionDecision, PermissionPrompt, QuestionOption,
        ToolCallId, UserQuestionPrompt,
    };
    use tokio::sync::oneshot;

    fn permission_prompt() -> PermissionPrompt {
        let (tx, _rx) = oneshot::channel();
        PermissionPrompt {
            id: BlockId(1),
            tool_call_id: ToolCallId(String::new()),
            tool_name: "Bash".to_string(),
            reasoning: "ls".to_string(),
            audit_hint: None,
            choices: vec![
                PermissionChoice {
                    key: 'y',
                    label: "allow once".to_string(),
                    decision: PermissionDecision::AllowOnce,
                },
                PermissionChoice {
                    key: 'a',
                    label: "allow always".to_string(),
                    decision: PermissionDecision::AllowAlways,
                },
                PermissionChoice {
                    key: 'n',
                    label: "deny".to_string(),
                    decision: PermissionDecision::Deny,
                },
            ],
            responder: tx,
        }
    }

    fn question_prompt(multi: bool) -> UserQuestionPrompt {
        let (tx, _rx) = oneshot::channel();
        UserQuestionPrompt {
            id: BlockId(2),
            question: "which?".to_string(),
            header: None,
            options: vec![
                QuestionOption {
                    label: "alpha".to_string(),
                    description: None,
                    preview: None,
                },
                QuestionOption {
                    label: "beta".to_string(),
                    description: None,
                    preview: None,
                },
            ],
            multi_select: multi,
            responder: tx,
        }
    }

    #[test]
    fn permission_keys_follow_prompt_choices() {
        let prompt = permission_prompt();
        assert_eq!(parse_permission_answer("Y", &prompt), Some(PermissionDecision::AllowOnce));
        assert_eq!(parse_permission_answer("a", &prompt), Some(PermissionDecision::AllowAlways));
        assert_eq!(parse_permission_answer("", &prompt), Some(PermissionDecision::Deny));
        assert_eq!(parse_permission_answer("yes", &prompt), None);
        assert_eq!(parse_permission_answer("q", &prompt), None);
    }

    #[test]
    fn question_numbers_map_to_labels_and_text_passes_through() {
        let single = question_prompt(false);
        assert_eq!(parse_question_answer("2", &single), Some(vec!["beta".to_string()]));
        assert_eq!(parse_question_answer("1,2", &single), None);
        assert_eq!(parse_question_answer("9", &single), None);
        assert_eq!(
            parse_question_answer("something else", &single),
            Some(vec!["something else".to_string()])
        );
        assert_eq!(parse_question_answer("   ", &single), None);
        let multi = question_prompt(true);
        assert_eq!(
            parse_question_answer("1, 2", &multi),
            Some(vec!["alpha".to_string(), "beta".to_string()])
        );
    }
}
