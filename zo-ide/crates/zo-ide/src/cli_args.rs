//! zo-cli `cli_args.rs` 의 엔진이 참조하는 **타입 몇 개만**.
//!
//! 164개 슬래시 명령의 파서·플래그 표는 가져오지 않는다. 포팅된 엔진 파일들이
//! `crate::cli_args::AllowedToolSet` 로 부르기 때문에 모듈 이름은 유지한다.

use std::collections::BTreeSet;

/// 툴 이름 집합 — `--allowedTools` / `--disallowedTools`.
pub type AllowedToolSet = BTreeSet<String>;
pub type DisallowedToolSet = BTreeSet<String>;

/// 카탈로그 단일 진실 [`api::resolve_model_alias`] 위임 — **게시된** 카탈로그 위에서.
///
/// 별칭은 발견이 옮겨 둔 곳을 가리킨다(`gemini-flash` 는 오늘 발견된 3.7). 인자
/// 파싱은 세션이 카탈로그를 게시하기 전에 묻는 자리라, 여기서 먼저 게시하지 않으면
/// 내장 스냅샷의 옛 답(3.6)이 그대로 세션의 모델이 됐다(2026-09-02 실측). 게시는
/// 같은 답을 다시 내도 알림을 되풀이하지 않는다.
#[must_use]
pub fn resolve_model_alias(model: &str) -> String {
    crate::runtime_support::publish_model_catalog();
    api::resolve_model_alias(model)
}

