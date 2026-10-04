//! `delegate` — 일을 적고, 그 일 위에 워커를 띄우고, 워커에게 제 id를 알리는 첫 편지까지
//! 한 번의 계획으로 한다 (t-34501 4단계).
//!
//! 이 파일은 빨강 단계의 빈 모양이다: 시험이 컴파일되고 단언에서 떨어지도록 이름과 모양만 있다.

use super::*;

/// 이 동사의 이름.
pub const VERB: &str = "delegate";

/// 위임한 워커가 어디까지 왔는가.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Reported { message: String, body: String },
    Asking { message: String, body: String },
    Ended { task_status: &'static str },
}

impl Outcome {
    /// 기다림이 끝나는 까닭을 한 낱말로.
    pub const fn word(&self) -> &'static str {
        ""
    }
}

pub(super) fn strip_wait(_words: &mut Words) {}

#[allow(clippy::too_many_arguments)]
pub(super) fn plan(
    _ledger: &mut Ledger,
    _team: &mut Team,
    _launcher: &dyn Launcher,
    _words: &Words,
    _pane: &str,
    _now_ms: i64,
    _actor: &str,
    _caller: &str,
    _seat: &str,
) -> Result<Decided, String> {
    Err("delegate is not written yet".to_string())
}

#[must_use]
pub fn outcome(_run: &Run, _worker: &str, _dispatch: &str) -> Option<Outcome> {
    None
}

/// `--wait`가 말하는 기다림의 길이(밀리초).
pub fn wait_budget(_argv: &[String]) -> Result<Option<u32>, String> {
    Err("delegate is not written yet".to_string())
}

/// 위임의 답에 기다림이 본 것을 덧붙인다.
#[must_use]
pub fn with_waited(
    answer: &str,
    _outcome: Option<&Outcome>,
    _waited_ms: u64,
    _budget_ms: u32,
) -> String {
    answer.to_string()
}
