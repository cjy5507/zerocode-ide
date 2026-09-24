#![allow(dead_code)] // Intentionally unwired until the source-pin cutover.

//! 창의 소스 전부 — 몇 개 파일로 나뉘어 있든 한 건초더미로.
//!
//! 300개의 소스텍스트 핀이 읽던 것은 "`ui/shell.js`라는 파일"이 아니라 "이
//! 창의 프런트엔드"다. 분리가 그 둘을 갈라놓는 날, `block_after`는 패닉하고
//! 부정형 단언은 빈 건초더미 위에서 조용히 참이 된다
//! (`main.rs:38131`–`38153` 주석이 세어 둔 그 실패의 JS 쪽 판박이).
//!
//! 시험 전용이다. `#[cfg(test)]`가 항목마다 붙는 것은 `main.rs`의 배송 절반에
//! 그 속성이 서면 `main.rs:38155`의 파수꾼이 깨지기 때문이다 — 그러니 선언은
//! 맨몸 `mod ui_source;`이고 시험이라는 사실은 여기서 말한다.

/// 문서가 실행하는 순서 그대로, 이름과 함께.
///
/// 이 순서는 `ui/index.html`의 `<script defer>` 순서와 같아야 하고
/// (`main.rs:46768`의 게이트 B가 그 둘을 맞춘다), 같아야 하는 이유는
/// `block_after`가 **첫 일치**를 집기 때문이다: 순서가 곧 답이다.
#[cfg(test)]
pub(crate) const WINDOW_PARTS: &[(&str, &str)] = &[
    ("shell-boot.js", include_str!("../../../ui/shell-boot.js")),
    ("shell-i18n.js", include_str!("../../../ui/shell-i18n.js")),
    (
        "shell-term-selection.js",
        include_str!("../../../ui/shell-term-selection.js"),
    ),
    ("shell-term.js", include_str!("../../../ui/shell-term.js")),
    (
        "shell-status.js",
        include_str!("../../../ui/shell-status.js"),
    ),
    ("shell-doc.js", include_str!("../../../ui/shell-doc.js")),
    (
        "shell-knowledge-3d.js",
        include_str!("../../../ui/shell-knowledge-3d.js"),
    ),
    (
        "shell-knowledge.js",
        include_str!("../../../ui/shell-knowledge.js"),
    ),
    (
        "shell-knowledge-supply.js",
        include_str!("../../../ui/shell-knowledge-supply.js"),
    ),
    ("shell-scm.js", include_str!("../../../ui/shell-scm.js")),
    (
        "shell-browser.js",
        include_str!("../../../ui/shell-browser.js"),
    ),
    (
        "shell-workspace.js",
        include_str!("../../../ui/shell-workspace.js"),
    ),
    ("shell-input.js", include_str!("../../../ui/shell-input.js")),
    (
        "shell-explorer-search.js",
        include_str!("../../../ui/shell-explorer-search.js"),
    ),
    (
        "shell-explorer-tree.js",
        include_str!("../../../ui/shell-explorer-tree.js"),
    ),
    (
        "shell-path-browser.js",
        include_str!("../../../ui/shell-path-browser.js"),
    ),
    (
        "shell-attach.js",
        include_str!("../../../ui/shell-attach.js"),
    ),
    (
        "shell-composer.js",
        include_str!("../../../ui/shell-composer.js"),
    ),
    (
        "shell-update.js",
        include_str!("../../../ui/shell-update.js"),
    ),
    (
        "shell-computer.js",
        include_str!("../../../ui/shell-computer.js"),
    ),
    (
        "shell-settings.js",
        include_str!("../../../ui/shell-settings.js"),
    ),
    ("shell-jev.js", include_str!("../../../ui/shell-jev.js")),
    ("shell-flow.js", include_str!("../../../ui/shell-flow.js")),
    (
        "shell-remote.js",
        include_str!("../../../ui/shell-remote.js"),
    ),
    ("shell-sftp.js", include_str!("../../../ui/shell-sftp.js")),
    (
        "shell-conversation-view.js",
        include_str!("../../../ui/shell-conversation-view.js"),
    ),
    ("shell-board.js", include_str!("../../../ui/shell-board.js")),
    ("shell.js", include_str!("../../../ui/shell.js")),
];

/// 파트를 순서대로 이어 붙인 한 건초더미. 한 번 만들고 계속 빌려준다.
///
/// 이어 붙이는 자리에 아무것도 끼우지 않는다 — 각 파트가 이미 개행으로
/// 끝나므로, 자르기가 순수한 이동인 한 이 문자열은 자르기 전 `ui/shell.js`와
/// **바이트까지 같다.** 줄을 세는 시험 아홉 개가 그 위에 서 있다.
#[cfg(test)]
pub(crate) fn window_source() -> &'static str {
    static JOINED: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    JOINED
        .get_or_init(|| WINDOW_PARTS.iter().map(|(_, part)| *part).collect())
        .as_str()
}
