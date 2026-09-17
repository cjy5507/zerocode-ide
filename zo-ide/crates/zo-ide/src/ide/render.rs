//! codex 동일 스타일 스트리밍 렌더러.
//!
//! 정본은 두 문서다: 문법표 `docs/codex-style.md`, 원본 바이트
//! `docs/codex-capture-v0.149.1.txt` (codex-cli 0.149.1 을 PTY 캡처).
//! 여기서 새 장식을 발명하지 않는다 — 캡처에 없는 모양이 필요하면 캡처에 있는
//! 문법(볼드 라벨 + 맨몸 값)을 빌려 쓴다.
//!
//! # 출력 규율
//!
//! append-only 다. 방출하는 이스케이프는 SGR 뿐이고, 줄바꿈은 `\n` 뿐이다.
//! 커서 이동·줄 지움·CSI ?2026 동기화·alt screen·`crossterm`/`ratatui` 는
//! 이 파일에 들어오지 않는다. 색은 전부 30번대 기본색이고 브라이트(90번대)는
//! 쓰지 않는다 — 캡처가 그렇다.
//!
//! # 측정된 문법
//!
//! ```text
//! {C36}user{R}                  ESC[36m user  ESC[0m
//! {C35}{I}codex{R}{R}           ESC[35m ESC[3m codex ESC[0m ESC[0m   ← 리셋이 둘
//! {C35}{I}exec{R}{R}
//! {B}<cmd>{R} in <cwd>          ESC[1m …  ESC[0m
//! {C32} succeeded in 0ms:{R}    앞 공백까지 색 안쪽 — 캡처 그대로
//! {C31} exited 1 in 0ms:{R}
//! {D}tokens used{R} / 6,575
//! ```
//!
//! 캡처의 `hook:` 줄과 줄끝 `\r` 은 각각 이 머신의 훅 설정·PTY ONLCR 산물이라
//! 계약이 아니다.
//!
//! # 어시스턴트 마크다운
//!
//! `codex exec` 실측은 마크다운을 **원문 그대로** 흘린다 (`### Sample`,
//! `- item`, 백틱이 전부 리터럴). 그래서 기본값이 원문이고, 포팅한
//! [`crate::render::TerminalRenderer`] 는 [`RenderOptions::render_markdown`]
//! 뒤로 물러난다. 그 옵션을 켜면 출력에 256색이 섞이므로 "30번대만" 규율은
//! 실측 일치를 포기한 사용자의 선택이 된다.
//!
//! # 블로킹 금지
//!
//! 블록 채널의 cap 은 64 다 — 렌더러가 한 번이라도 사람을 기다리면 그 뒤로
//! HTTP 바디 읽기가 멎는다. 그래서 [`RenderBlock::PermissionPrompt`] ·
//! [`RenderBlock::UserQuestionPrompt`] 는 여기서 응답하지 않고 통째로
//! [`PendingPrompt`] 로 반환된다 (파킹). responder 를 drop 하면 hard deny 이므로
//! 호출자는 반드시 그 값을 살려 둬야 한다.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::Write;
use std::time::Instant;

use runtime::message_stream::{
    AgentResultStatus, BashResult, DiffLineKind, DiffView, NotificationRoad, PermissionPrompt,
    RenderBlock, SystemLevel, TodoResultItem, TodoResultStatus, ToolCallStatus, ToolPreview, ToolResultBody,
    UserQuestionPrompt,
};

use crate::render::{MarkdownStreamState, TerminalRenderer, no_color_env};
use crate::util::ansi::{sanitize_inline, strip_ansi};

// ============================================================================
// 측정된 SGR 어휘 (docs/codex-capture-v0.149.1.txt)
// ============================================================================

/// SGR 0. 캡처는 `codex`/`exec` 라벨 뒤에만 이걸 **두 번** 붙인다.
const RESET: &str = "\u{1b}[0m";
/// SGR 1 — 배너 라벨과 명령줄의 볼드.
const BOLD: &str = "\u{1b}[1m";
/// SGR 2 — `tokens used` 푸터와 `System` 줄의 dim.
const DIM: &str = "\u{1b}[2m";
/// fg 36 — `user` 라벨.
const USER_FG: &str = "\u{1b}[36m";
/// fg 35 + SGR 3 — 어시스턴트 계열 라벨(`codex`/`exec`/`tool`).
const AGENT_FG_ITALIC: &str = "\u{1b}[35m\u{1b}[3m";
/// fg 32 — 성공한 툴 결과 헤더.
const OK_FG: &str = "\u{1b}[32m";
/// fg 31 — 실패한 툴 결과 헤더.
const ERR_FG: &str = "\u{1b}[31m";

/// 어시스턴트 텍스트 라벨. 철자는 문법표가 정한다 — 브랜딩으로 갈아끼우면
/// 캡처와의 골든 비교가 끊긴다.
const LABEL_AGENT: &str = "codex";
/// Bash 계열 툴콜 라벨.
const LABEL_EXEC: &str = "exec";
/// 비-Bash 툴콜 라벨. 캡처에 없는 확장이지만 문법은 `exec` 와 같다.
const LABEL_TOOL: &str = "tool";
/// 배너 위아래 구분선. 캡처는 하이픈 여덟 개다.
const RULE: &str = "--------";
/// 유휴 입력 마커. 캡처(비대화 `codex exec`)에는 없지만 대화 패인에는 있는
/// 유일한 장식이다 — 사람이 어디에 치는지 보여 준다.
const IDLE_MARKER: &str = "❯ ";

// ============================================================================
// 옵션
// ============================================================================

/// `--json` 이 낼 수 있는 `type` 값 전부.
///
/// 소비자가 `match` 를 쓰는 값이므로 **계약**이다. 이름을 바꾸거나 새 값을
/// 내면서 이 목록을 안 고치면 밖에서는 알 길이 없고, 그래서
/// `tests::the_json_vocabulary_matches_what_the_emitter_can_say` 가 방출부
/// 원문과 이 목록을 맞대 본다 — 한쪽에만 있는 이름이 있으면 빨개진다.
///
/// * `user` — 사람이 보낸 프롬프트.
/// * `assistant_delta` / `assistant` — 흘러가는 조각과 완성된 답. 둘 다 낸다.
/// * `reasoning` — `--show-thinking` 일 때만.
/// * `tool_call` / `tool_result` — `id` 로 이어진다. 본문은 접지 않은 원문.
/// * `agent_result` — 하위 에이전트 한 판의 보고.
/// * `system` / `notice` / `card` — 호스트가 내는 알림.
/// * `usage` — 누적 토큰.
/// * `stream_phase` — 모델 요청의 자리: 보냈고 아직 첫 이벤트가 없다
///   (`request_sent`), 또는 재시도를 기다린다(`retrying`). 자동화가 "멈췄나"를
///   묻지 않게 하는 신호다.
/// * `wire_model` — 이 요청이 실제로 나가는 모델과 그 이유(`model`,
///   `source`: `session`·`refusal_fallback`·`quota_fallback`·…). 폴백·레그
///   스왑 뒤 답을 쓴 모델이 설정 모델과 다를 때 자동화가 그것을 안다.
/// * `permission_request` / `question` — 턴이 사람을 기다리는 자리.
/// * `loop` — one bounded headless-loop iteration and its controller outcome.
pub const JSON_EVENT_TYPES: &[&str] = &[
    "user",
    "assistant_delta",
    "assistant",
    "reasoning",
    "tool_call",
    "tool_result",
    "agent_result",
    "system",
    "notice",
    "notify",
    "card",
    "usage",
    "stream_phase",
    "wire_model",
    "permission_request",
    "question",
    "loop",
    "launch_refused",
];

/// The one line a `--json` run prints when its exact launch contract refused
/// the selection (`crate::launch_contract`): the reason code, its sentence and
/// the safe selection fields the receipt carries — never argv, env or the
/// request envelope itself. Printed before anything else, then the process
/// exits; a consumer that sees it knows no turn ran.
#[must_use]
pub fn launch_refused_json(reason: &str, detail: &str, contract: &serde_json::Value) -> String {
    serde_json::json!({
        "type": "launch_refused",
        "reason": reason,
        "detail": detail,
        "version": contract["version"],
        "requested": contract["requested"],
    })
    .to_string()
}

/// 렌더러의 노브. 전부 기본값이 실측 일치(codex exec)다.
///
/// 넷 다 독립된 on/off 다 — `RenderFlags` 와 같은 이유로 enum 이 아니다
/// (묶으면 조합이 곱해진다).
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct RenderOptions {
    /// 어시스턴트 텍스트를 마크다운으로 렌더할지. `false`(기본)면 실측대로
    /// 원문 그대로 흘린다. `true` 면 [`TerminalRenderer`] 를 태우므로 출력에
    /// 256색이 섞인다.
    pub render_markdown: bool,
    /// [`RenderBlock::Reasoning`] 을 dim 으로 보일지. 기본은 숨김 —
    /// `codex exec` 는 reasoning summaries 를 끄고 돈다.
    pub show_thinking: bool,
    /// SGR 방출 여부. `false` 면 출력에서 이스케이프가 **한 바이트도** 나가지
    /// 않는다(`NO_COLOR`).
    pub color: bool,
    /// `exec` 명령줄의 `in <cwd>` 꼬리. 비면 꼬리를 통째로 뺀다.
    pub cwd: String,
    /// stdin 이 무엇인가. 호출자 `run_loop` 가 `is_terminal()` 을 한 번 재서
    /// 넘긴다 — 렌더러는 env 도 tty 도 직접 읽지 않는다(테스트 주입 가능).
    pub input: InputSource,
    /// 사람이 아니라 **프로그램**이 읽는 출력. 참이면 산문 대신 이벤트마다
    /// JSON 한 줄을 낸다(`Renderer::json_line`).
    ///
    /// `--plain` 은 파이프에서도 사람이 읽기 좋게 만든 것이고, 그게 바로
    /// 자동화가 못 쓰는 이유다 — 색·정렬·접힘·요약이 섞인 산문을 다시
    /// 파싱해야 한다. 장시간 자율 실행을 다른 도구가 몰려면 안정된 어휘가
    /// 필요하고, 이것이 그 어휘다.
    pub json: bool,
    /// 어시스턴트 라벨 재정의. `None` 이면 기본 실측 라벨(`LABEL_AGENT` = "codex")을
    /// 사용하고, 지정되면("zo" 등) 해당 라벨을 렌더링에 사용한다.
    pub agent_label: Option<String>,
}

/// stdin 의 정체. 패인(cooked tty)과 파이프는 에코가 있고 없고가 다르다.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum InputSource {
    /// 파이프·파일 리다이렉트. 에코가 없으니 `user` 라벨 + 원문을 렌더러가
    /// 찍고 유휴 마커는 찍지 않는다. 기본값이라 `codex exec` 골든 바이트는
    /// 이 변경 뒤에도 그대로다.
    #[default]
    Pipe,
    /// 사람의 cooked tty. 타이핑은 tty 가 이미 에코했으므로
    /// [`RenderBlock::UserMessage`] 재에코를 생략하고, 유휴로 돌아올 때
    /// [`Renderer::idle_marker`] 를 찍는다.
    Tty,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            render_markdown: false,
            show_thinking: false,
            color: true,
            cwd: String::new(),
            input: InputSource::Pipe,
            json: false,
            agent_label: None,
        }
    }
}

impl RenderOptions {
    /// 환경에서 읽는 기본값 — `NO_COLOR` 만 본다. 나머지 노브는 CLI 플래그의
    /// 몫이라 여기서 건드리지 않는다.
    #[must_use]
    pub fn from_env(cwd: impl Into<String>) -> Self {
        Self {
            color: !no_color_env(),
            cwd: cwd.into(),
            ..Self::default()
        }
    }
}

/// 세션 시작 배너의 재료. 라벨 철자는 캡처 그대로다.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionBanner {
    /// 첫 줄 — 캡처의 `OpenAI Codex v0.149.1` 자리.
    pub header: String,
    /// `workdir:` 값.
    pub workdir: String,
    /// `model:` 값.
    pub model: String,
    /// `provider:` 값.
    pub provider: String,
    /// `approval:` 값.
    pub approval: String,
    /// `sandbox:` 값.
    pub sandbox: String,
    /// `reasoning effort:` 값.
    pub reasoning_effort: String,
    /// `reasoning summaries:` 값.
    pub reasoning_summaries: String,
    /// `session id:` 값.
    pub session_id: String,
}

/// 파킹된 프롬프트. 렌더러는 사람을 기다리지 않고 이걸 호출자에게 넘긴다.
///
/// 안에 든 responder 를 drop 하면 런타임은 그걸 **hard deny** 로 읽는다.
#[derive(Debug)]
pub enum PendingPrompt {
    /// 권한 승인 요청.
    Permission(PermissionPrompt),
    /// 툴이 사람에게 던진 질문.
    Question(UserQuestionPrompt),
}

// ============================================================================
// 시계
// ============================================================================

/// ` succeeded in <ms>:` 의 밀리초를 재는 단조 시계.
///
/// 골든 테스트가 시간을 고정할 수 있도록 주입 가능하게 뽑아 뒀다.
pub trait Clock: std::fmt::Debug + Send {
    /// 임의의 고정 기준점으로부터 흐른 밀리초. 단조여야 한다.
    fn now_millis(&self) -> u64;
}

/// [`Instant`] 기반 기본 시계.
#[derive(Debug)]
pub struct MonotonicClock {
    base: Instant,
}

impl MonotonicClock {
    /// 지금을 영점으로 잡는다.
    #[must_use]
    pub fn new() -> Self {
        Self { base: Instant::now() }
    }
}

impl Default for MonotonicClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for MonotonicClock {
    fn now_millis(&self) -> u64 {
        u64::try_from(self.base.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

// ============================================================================
// 렌더러
// ============================================================================

/// 한 번 announce 된 툴콜. 결과 줄이 시작 시각을 여기서 되찾는다.
#[derive(Debug)]
struct AnnouncedCall {
    started_ms: u64,
}

/// `RenderBlock` 스트림을 codex 문법의 append-only 바이트로 옮긴다.
///
/// 한 턴의 수명: [`Renderer::banner`] (선택) → [`Renderer::push`] 반복 →
/// [`Renderer::finish_turn`].
#[derive(Debug)]
pub struct Renderer<W: Write> {
    out: W,
    options: RenderOptions,
    clock: Box<dyn Clock>,
    markdown: MarkdownStreamState,
    terminal: TerminalRenderer,
    /// 같은 `tool_call_id` 로 Pending→Running→Ok 가 여러 번 온다. 이 맵이
    /// "exec 줄은 최초 1회" 를 지키고, `ToolResult`(id 만 들고 온다)에게
    /// 이름과 시작 시각을 돌려준다.
    announced: HashMap<String, AnnouncedCall>,
    /// 지금 열려 있는 어시스턴트 텍스트 세그먼트의 블록 id. 세그먼트가 바뀔
    /// 때마다 `codex` 라벨을 새로 찍는다.
    open_text: Option<u64>,
    /// 지금 열려 있는 reasoning 세그먼트의 블록 id.
    open_reasoning: Option<u64>,
    /// 마지막 [`RenderBlock::Usage`] 의 누적 토큰 — 턴말 푸터 재료.
    usage_total: Option<u64>,
    /// 흘러가는 중인 어시스턴트 텍스트의 누적. `done` 에서 [`Self::last_answer`]
    /// 로 옮겨 앉는다.
    answer_so_far: String,
    /// 마지막으로 **완성된** 어시스턴트 답변. `--last-message` 가 쓰는 값이고,
    /// 산문 모드와 JSON 모드가 같은 값을 본다 — 자동화가 출력 형식에 따라
    /// 다른 답을 받으면 그건 계약이 아니다.
    last_answer: String,
    /// 마지막으로 쓴 바이트가 `\n` 이었는지. 라벨 줄을 새로 열기 전에 이걸로
    /// 줄을 맞춘다(줄 지움 이스케이프 없이).
    at_line_start: bool,
    /// 쓰기가 한 번이라도 실패했는지. 렌더러는 실패에 panic 하지 않는다 —
    /// 패인이 닫혀도 턴은 끝까지 돌아야 한다.
    write_failed: bool,
}

impl<W: Write> Renderer<W> {
    /// 기본 시계로 렌더러를 연다.
    #[must_use]
    pub fn new(out: W, options: RenderOptions) -> Self {
        Self::with_clock(out, options, Box::new(MonotonicClock::new()))
    }

    /// 시계를 주입해서 연다 — 골든 테스트가 경과 ms 를 고정하는 문.
    #[must_use]
    pub fn with_clock(out: W, options: RenderOptions, clock: Box<dyn Clock>) -> Self {
        Self {
            out,
            options,
            clock,
            markdown: MarkdownStreamState::default(),
            terminal: TerminalRenderer::new(),
            announced: HashMap::new(),
            open_text: None,
            open_reasoning: None,
            usage_total: None,
            answer_so_far: String::new(),
            last_answer: String::new(),
            at_line_start: true,
            write_failed: false,
        }
    }

    /// 출력 쓰기가 한 번이라도 실패했는지.
    #[must_use]
    pub const fn write_failed(&self) -> bool {
        self.write_failed
    }

    /// 세션 시작 배너 — `header` + 구분선 + 볼드 라벨 목록 + 구분선.
    pub fn banner(&mut self, banner: &SessionBanner) {
        self.ensure_line_start();
        self.emit(&format!("{}\n{RULE}\n", banner.header));
        let rows: [(&str, &str); 8] = [
            ("workdir", &banner.workdir),
            ("model", &banner.model),
            ("provider", &banner.provider),
            ("approval", &banner.approval),
            ("sandbox", &banner.sandbox),
            ("reasoning effort", &banner.reasoning_effort),
            ("reasoning summaries", &banner.reasoning_summaries),
            ("session id", &banner.session_id),
        ];
        let mut block = String::new();
        for (label, value) in rows {
            let _ = writeln!(block, "{BOLD}{label}:{RESET} {value}");
        }
        block.push_str(RULE);
        block.push('\n');
        self.emit(&block);
    }

    /// 블록 하나를 소비한다. 사람의 답이 필요한 블록이면 그걸 파킹해서
    /// 돌려주고, 그 외에는 항상 `None` 이다. **절대 블로킹하지 않는다.**
    pub fn push(&mut self, block: RenderBlock) -> Option<PendingPrompt> {
        // 두 모드가 같은 답을 보게, 분기보다 **먼저** 센다.
        if let RenderBlock::TextDelta { text, done, .. } = &block {
            self.answer_so_far.push_str(text);
            if *done {
                self.last_answer = std::mem::take(&mut self.answer_so_far);
            }
        }
        if self.options.json {
            return self.push_json(block);
        }
        match block {
            RenderBlock::UserMessage { text, .. } => self.user_message(&text),
            RenderBlock::TextDelta { id, text, done } => self.text_delta(id.0, &text, done),
            RenderBlock::Reasoning { id, text, done, .. } => self.reasoning(id.0, &text, done),
            RenderBlock::ToolCall {
                tool_call_id,
                name,
                summary,
                preview,
                status,
                ..
            } => self.tool_call(&tool_call_id.0, &name, &summary, &preview, status),
            RenderBlock::ToolResult {
                tool_call_id,
                is_error,
                body,
                ..
            } => self.tool_result(&tool_call_id.0, is_error, &body),
            RenderBlock::AgentResult {
                label,
                status,
                body,
                ..
            } => self.agent_result(&label, status, &body),
            RenderBlock::System { level, text, .. } => self.system(level, &text),
            RenderBlock::UserNotice { message, .. } => self.verbatim_block(&message),
            RenderBlock::Notification {
                title,
                body,
                level,
                road,
                ..
            } => {
                self.system(level, &crate::tui::strings::push_notification_line(road, &body));
                // The plain frontend on a tty is "outside the window" too: the
                // same bytes the TUI's painter sends, through the one write door.
                if road == NotificationRoad::Terminal {
                    self.emit(&crate::tui::bell::terminal_notification(&title, &body));
                }
            }
            RenderBlock::Card { card, .. } => self.verbatim_block(&card.plain_text()),
            RenderBlock::Usage { cumulative, .. } => {
                self.usage_total = Some(cumulative.total_tokens_u64());
            }
            RenderBlock::PermissionPrompt(prompt) => {
                self.parked_line(&sanitize_inline(&prompt.tool_name), PERMISSION_KEYS);
                return Some(PendingPrompt::Permission(prompt));
            }
            RenderBlock::UserQuestionPrompt(prompt) => {
                let keys = question_keys(prompt.options.len(), prompt.multi_select);
                self.parked_line(&sanitize_inline(&prompt.question), &keys);
                self.question_options(&prompt.options);
                return Some(PendingPrompt::Question(prompt));
            }
            // 패인에 자리가 없는 것들: 이미지·레이트리밋·압축 진행률은 라이브
            // 원장(TUI)의 재료지 트랜스크립트의 재료가 아니다.
            RenderBlock::Image { .. }
            | RenderBlock::RateLimit(_)
            | RenderBlock::StreamPhase(_)
            | RenderBlock::WireModel(_)
            | RenderBlock::CompactionProgress { .. } => {}
        }
        None
    }

    /// 이벤트 하나를 JSON 한 줄로 낸다.
    ///
    /// 계약은 좁고 안정적이다 — 줄 하나가 객체 하나이고, `type` 이 그 어휘를
    /// 고른다. 산문 렌더러와 달리 **아무것도 접거나 줄이지 않는다**: 자동화가
    /// 다시 파싱해야 하는 요약은 계약이 아니라 잡음이다.
    ///
    /// 어시스턴트 텍스트는 델타와 완성본을 **둘 다** 낸다. 흐름을 보려는
    /// 소비자와 마지막 답만 필요한 소비자가 다른 도구인데, 하나만 내면 나머지
    /// 하나가 스스로 이어 붙여야 한다.
    fn push_json(&mut self, block: RenderBlock) -> Option<PendingPrompt> {
        match block {
            RenderBlock::UserMessage { text, .. } => {
                self.json_line(&serde_json::json!({"type": "user", "text": text}));
            }
            RenderBlock::TextDelta { text, done, .. } => {
                if !text.is_empty() {
                    self.json_line(
                        &serde_json::json!({"type": "assistant_delta", "text": text}),
                    );
                }
                if done {
                    // `push` 머리가 이미 옮겨 담았다.
                    let answer = self.last_answer.clone();
                    self.json_line(&serde_json::json!({"type": "assistant", "text": answer}));
                }
            }
            RenderBlock::Reasoning { text, done, .. } => {
                if self.options.show_thinking {
                    self.json_line(
                        &serde_json::json!({"type": "reasoning", "text": text, "done": done}),
                    );
                }
            }
            RenderBlock::ToolCall { .. }
            | RenderBlock::ToolResult { .. }
            | RenderBlock::AgentResult { .. } => self.json_work(block),
            RenderBlock::System { level, text, .. } => {
                self.json_line(&serde_json::json!({
                    "type": "system",
                    "level": format!("{level:?}").to_lowercase(),
                    "text": text,
                }));
            }
            RenderBlock::UserNotice { message, .. } => {
                self.json_line(&serde_json::json!({"type": "notice", "text": message}));
            }
            RenderBlock::Notification {
                title, body, road, ..
            } => {
                self.json_line(&serde_json::json!({
                    "type": "notify",
                    "title": title,
                    "body": body,
                    "road": road.as_str(),
                }));
            }
            RenderBlock::Card { card, .. } => {
                self.json_line(&serde_json::json!({"type": "card", "text": card.plain_text()}));
            }
            RenderBlock::Usage { cumulative, .. } => {
                let total = cumulative.total_tokens_u64();
                self.usage_total = Some(total);
                self.json_line(&serde_json::json!({"type": "usage", "total_tokens": total}));
            }
            // 승인과 질문은 **줄을 내고도 프롬프트를 돌려준다**. 자동화가
            // 무엇에 막혔는지 봐야 하고, 호출자는 여전히 그 요청을 처리해야
            // 한다(비대화형이면 거절로 앉는다).
            RenderBlock::PermissionPrompt(prompt) => {
                self.json_line(&serde_json::json!({
                    "type": "permission_request",
                    "tool": prompt.tool_name,
                }));
                return Some(PendingPrompt::Permission(prompt));
            }
            RenderBlock::UserQuestionPrompt(prompt) => {
                self.json_line(&serde_json::json!({
                    "type": "question",
                    "question": prompt.question,
                    "options": prompt
                        .options
                        .iter()
                        .map(|option| option.label.clone())
                        .collect::<Vec<_>>(),
                }));
                return Some(PendingPrompt::Question(prompt));
            }
            RenderBlock::StreamPhase(phase) => {
                use runtime::message_stream::types::StreamPhase;
                self.json_line(&match phase {
                    StreamPhase::RequestSent { attempt } => serde_json::json!({
                        "type": "stream_phase", "phase": "request_sent", "attempt": attempt
                    }),
                    StreamPhase::Retrying { attempt, delay_secs } => serde_json::json!({
                        "type": "stream_phase", "phase": "retrying", "attempt": attempt,
                        "delay_secs": delay_secs
                    }),
                    StreamPhase::QuietReasoning { since_secs } => serde_json::json!({
                        "type": "stream_phase", "phase": "quiet_reasoning", "since_secs": since_secs
                    }),
                });
            }
            RenderBlock::WireModel(wire) => {
                self.json_line(&serde_json::json!({
                    "type": "wire_model", "model": wire.model, "source": wire.source.as_tag()
                }));
            }
            RenderBlock::Image { .. }
            | RenderBlock::RateLimit(_)
            | RenderBlock::CompactionProgress { .. } => {}
        }
        None
    }

    /// 일을 나타내는 세 블록(도구 호출·결과·에이전트 결과)의 JSON.
    ///
    /// 셋을 한 자리에 두는 이유는 자동화가 이것들을 함께 읽기 때문이다 —
    /// `id` 로 호출과 결과를 잇고, 본문은 접지 않은 원문이다.
    fn json_work(&mut self, block: RenderBlock) {
        let value = match block {
            RenderBlock::ToolCall {
                tool_call_id,
                name,
                summary,
                status,
                ..
            } => serde_json::json!({
                "type": "tool_call",
                "id": tool_call_id.0,
                "name": name,
                "summary": summary,
                "status": format!("{status:?}").to_lowercase(),
            }),
            RenderBlock::ToolResult {
                tool_call_id,
                is_error,
                body,
                ..
            } => serde_json::json!({
                "type": "tool_result",
                "id": tool_call_id.0,
                "is_error": is_error,
                "body": crate::tui::tools::body_text(&body),
            }),
            RenderBlock::AgentResult {
                label,
                status,
                summary,
                body,
                ..
            } => serde_json::json!({
                "type": "agent_result",
                "label": label,
                "status": format!("{status:?}").to_lowercase(),
                "summary": summary,
                "body": body,
            }),
            _ => return,
        };
        self.json_line(&value);
    }

    /// 한 줄, 개행으로 끝난다. 색은 절대 섞지 않는다 — 이건 사람 것이 아니다.
    fn json_line(&mut self, value: &serde_json::Value) {
        let mut line = value.to_string();
        line.push('\n');
        let _ = self.out.write_all(line.as_bytes());
        let _ = self.out.flush();
    }

    /// Machine-facing boundary record for one headless loop iteration.
    pub fn loop_iteration(
        &mut self,
        iteration: u32,
        of: u32,
        trigger: &str,
        outcome: &str,
    ) {
        if self.options.json {
            self.json_line(&serde_json::json!({
                "type": "loop",
                "iteration": iteration,
                "of": of,
                "trigger": trigger,
                "outcome": outcome,
            }));
        }
    }

    /// 이번 실행에서 마지막으로 **완성된** 어시스턴트 답변.
    #[must_use]
    pub fn last_assistant_message(&self) -> &str {
        &self.last_answer
    }

    /// 턴을 닫는다: 열린 텍스트 세그먼트를 flush 하고, 본 적 있는 `Usage` 로
    /// `tokens used` 푸터를 찍는다. `Usage` 가 없었으면 푸터도 없다.
    pub fn finish_turn(&mut self) {
        // JSON 모드의 사용량은 이미 `{"type":"usage"}` 로 나갔다. 그 위에 산문
        // 푸터를 얹으면 파싱되지 않는 줄이 턴마다 하나씩 생긴다.
        if self.options.json {
            self.usage_total = None;
            return;
        }
        self.close_segments();
        if let Some(total) = self.usage_total.take() {
            self.emit(&format!(
                "{DIM}tokens used{RESET}\n{}\n",
                with_thousands(total)
            ));
        }
    }

    /// 유휴 입력 마커 한 줄 — 배너 직후·턴 종료 후·슬래시 처리 후에 찍는다.
    /// 파이프(비-TTY)에서는 한 바이트도 나가지 않는다(골든 불변).
    ///
    /// append-only 규율 그대로다: 커서 이동도 줄 지움도 없다. 줄바꿈을 **붙이지
    /// 않는** 것이 요점 — cooked tty 의 에코가 마커 뒤에 이어져 `❯ dd` 한 줄이
    /// 된다. 그 에코가 줄을 닫으므로 여기서 `at_line_start` 를 참으로 되돌려,
    /// 다음 라벨이 빈 줄 하나를 사이에 두지 않게 한다.
    pub fn idle_marker(&mut self) {
        // JSON 은 사람이 읽는 것이 아니다. tty 위에서 돌더라도 마커 한 글자가
        // 첫 줄에 붙으면 그 줄은 파싱되지 않는다 — 계약이 통째로 깨진다.
        if self.options.json || self.options.input != InputSource::Tty {
            return;
        }
        self.ensure_line_start();
        self.emit(&format!("{DIM}{IDLE_MARKER}{RESET}"));
        self.at_line_start = true;
    }

    // ------------------------------------------------------------------
    // 블록별 처리
    // ------------------------------------------------------------------

    fn user_message(&mut self, text: &str) {
        // cooked tty 는 방금 타이핑을 이미 에코했다. 그 위에 `user` 라벨 +
        // 원문을 다시 찍으면 같은 줄이 두 번 보인다. 파이프(비-TTY)에는 에코가
        // 없으니 그대로 찍는다 — `codex exec` 패리티이자 골든의 계약이다.
        if self.options.input == InputSource::Tty {
            return;
        }
        self.close_segments();
        self.label(USER_FG, "user", false);
        self.emit_line(text);
    }

    fn agent_label(&self) -> &str {
        self.options.agent_label.as_deref().unwrap_or(LABEL_AGENT)
    }

    fn text_delta(&mut self, id: u64, text: &str, done: bool) {
        if self.open_text != Some(id) {
            self.close_segments();
            let label = self.agent_label().to_string();
            self.label(AGENT_FG_ITALIC, &label, true);
            self.open_text = Some(id);
        }
        if !text.is_empty() {
            if self.options.render_markdown {
                if let Some(rendered) = self.markdown.push(&self.terminal, text) {
                    self.emit(&rendered);
                }
            } else {
                self.emit(text);
            }
        }
        if done {
            self.flush_markdown();
            self.ensure_line_start();
            self.open_text = None;
        }
    }

    fn reasoning(&mut self, id: u64, text: &str, done: bool) {
        if !self.options.show_thinking {
            return;
        }
        if self.open_reasoning != Some(id) {
            self.close_segments();
            self.open_reasoning = Some(id);
        }
        // 라벨은 없다 — 캡처에 reasoning 라벨이 없어서 지어내지 않는다. dim 만
        // 입힌다(문법표: "기본 숨김, --show-thinking 시 dim").
        if !text.is_empty() {
            self.emit(&format!("{DIM}{text}{RESET}"));
        }
        if done {
            self.ensure_line_start();
            self.open_reasoning = None;
        }
    }

    fn tool_call(
        &mut self,
        tool_call_id: &str,
        name: &str,
        summary: &str,
        preview: &ToolPreview,
        status: ToolCallStatus,
    ) {
        // 인자 JSON 이 아직 흐르는 Pending 은 preview 가 미완성이라 넘긴다.
        // 그 뒤 어느 상태로 오든 최초 1회만 찍는다 — 같은 콜이 Running 과
        // Ok 로 두 번 announce 되면 트랜스크립트에 exec 줄이 겹친다.
        if status == ToolCallStatus::Pending || self.announced.contains_key(tool_call_id) {
            return;
        }
        self.announced.insert(
            tool_call_id.to_string(),
            AnnouncedCall {
                started_ms: self.clock.now_millis(),
            },
        );
        self.close_segments();
        match preview {
            ToolPreview::Bash { command } => {
                self.label(AGENT_FG_ITALIC, LABEL_EXEC, true);
                let tail = if self.options.cwd.is_empty() {
                    String::new()
                } else {
                    format!(" in {}", self.options.cwd)
                };
                self.emit(&format!("{BOLD}{command}{RESET}{tail}\n"));
            }
            other => {
                self.label(AGENT_FG_ITALIC, LABEL_TOOL, true);
                let detail = preview_summary(other, summary);
                if detail.is_empty() {
                    self.emit(&format!("{BOLD}{name}{RESET}\n"));
                } else {
                    self.emit(&format!("{BOLD}{name}{RESET} {detail}\n"));
                }
            }
        }
    }

    fn tool_result(&mut self, tool_call_id: &str, is_error: bool, body: &ToolResultBody) {
        let announced = self.announced.remove(tool_call_id);
        let elapsed = announced.as_ref().map_or(0, |call| {
            self.clock.now_millis().saturating_sub(call.started_ms)
        });
        self.close_segments();
        let exit_code = match body {
            ToolResultBody::Bash(result) => result.exit_code,
            _ if is_error => 1,
            _ => 0,
        };
        let failed = is_error || exit_code != 0;
        let header = if failed {
            format!("{ERR_FG} exited {exit_code} in {elapsed}ms:{RESET}\n")
        } else {
            format!("{OK_FG} succeeded in {elapsed}ms:{RESET}\n")
        };
        self.emit(&header);
        let text = result_body_text(body);
        if !text.is_empty() {
            self.emit_line(&text);
        }
        // 캡처: 결과 출력 뒤에 빈 줄 하나. 다음 라벨이 여기 붙는다.
        self.emit("\n");
    }

    fn agent_result(&mut self, label: &str, status: AgentResultStatus, body: &str) {
        self.close_segments();
        self.label(AGENT_FG_ITALIC, LABEL_TOOL, true);
        let verdict = match status {
            AgentResultStatus::Completed => "completed",
            AgentResultStatus::Failed => "failed",
        };
        self.emit(&format!("{BOLD}{label}{RESET} {verdict}\n"));
        if !body.trim().is_empty() {
            self.emit_line(body);
        }
        self.emit("\n");
    }

    fn system(&mut self, level: SystemLevel, text: &str) {
        self.close_segments();
        // 문법표: `{D}· <text>{R}`. 심각도는 색을 바꾸지 않는다 — 캡처에
        // dim 말고 다른 system 색이 없다.
        //
        // 멀티라인 System 은 codex 에 없다. 재시도·통지 같은 정보성 줄은
        // **첫 줄만** 남기고 버린다(`· provider overloaded; retrying …` 가
        // 문단째 늘어지던 자리). 예외는 [`SystemLevel::Error`] 뿐 — 위험을
        // 알리는 본문을 잘라 내면 사람이 대응할 근거가 사라진다.
        if matches!(level, SystemLevel::Error) {
            for line in text.lines() {
                self.emit(&format!("{DIM}· {line}{RESET}\n"));
            }
        } else if let Some(first) = text.lines().next() {
            self.emit(&format!("{DIM}· {first}{RESET}\n"));
        }
    }

    /// codex 에 대응 문법이 없는 본문(슬래시 카드, `send_to_user` 메모)은
    /// 장식 없이 원문만 흘린다. 새 장식을 발명하지 않는다는 규율의 귀결이다.
    fn verbatim_block(&mut self, text: &str) {
        if text.trim().is_empty() {
            return;
        }
        self.close_segments();
        self.emit_line(text);
    }

    /// 파킹 줄. `approval:` 의 문법(볼드 주어 + 맨몸 꼬리)을 빌린 한 줄이다.
    fn parked_line(&mut self, subject: &str, keys: &str) {
        self.close_segments();
        self.emit(&format!("{BOLD}⏸ {subject}{RESET} — {keys}\n"));
    }

    /// 파킹된 질문의 보기들 — 번호와 라벨, 있으면 dim 한 줄 설명.
    ///
    /// 파킹 줄만으로는 **무엇을 고를 수 있는지**가 화면에 없다. TUI 는
    /// 오버레이가 보여 주지만 이 로드는 append-only 라 줄로 말해야 한다.
    /// 문법은 codex 가 답한 질문을 스크롤백에 남길 때 쓰는 그 들여쓰기다
    /// (`history_cell/request_user_input.rs`: 질문은 `  • `, 답은 `    `).
    fn question_options(&mut self, options: &[runtime::message_stream::QuestionOption]) {
        for (index, option) in options.iter().enumerate() {
            let label = sanitize_inline(&option.label);
            let detail = option
                .description
                .as_deref()
                .map(sanitize_inline)
                .filter(|detail| !detail.is_empty());
            match detail {
                Some(detail) => {
                    self.emit(&format!("  {}. {label}  {DIM}{detail}{RESET}\n", index + 1));
                }
                None => self.emit(&format!("  {}. {label}\n", index + 1)),
            }
        }
    }

    // ------------------------------------------------------------------
    // 하부 쓰기
    // ------------------------------------------------------------------

    fn label(&mut self, color: &str, text: &str, double_reset: bool) {
        self.ensure_line_start();
        // 캡처의 `codex`/`exec` 라벨은 리셋이 둘이다(fg 하나, italic 하나).
        // `user` 라벨은 하나다. 바이트를 맞춘다.
        let tail = if double_reset { "\u{1b}[0m\u{1b}[0m" } else { RESET };
        self.emit(&format!("{color}{text}{tail}\n"));
    }

    /// 열린 텍스트/reasoning 세그먼트를 닫고 줄을 맞춘다.
    fn close_segments(&mut self) {
        self.flush_markdown();
        self.open_text = None;
        self.open_reasoning = None;
        self.ensure_line_start();
    }

    fn flush_markdown(&mut self) {
        if !self.options.render_markdown {
            return;
        }
        if let Some(rendered) = self.markdown.flush(&self.terminal) {
            self.emit(&rendered);
        }
    }

    fn ensure_line_start(&mut self) {
        if !self.at_line_start {
            self.emit("\n");
        }
    }

    /// 본문 한 덩어리 + 줄 마감.
    fn emit_line(&mut self, text: &str) {
        self.emit(text);
        self.ensure_line_start();
    }

    /// 유일한 쓰기 문. `color == false` 면 여기서 SGR 을 걷어내므로 출력에
    /// 이스케이프가 한 바이트도 나가지 않는다.
    ///
    /// 제로 카피 최적화: `color == true` 일 때는 스트리밍 청크를 힙에 `to_string`으로
    /// 복제하지 않고 원문 바이트 슬라이스를 즉시 기록하여 토큰마다 발생하는 할당을 제거한다.
    fn emit(&mut self, chunk: &str) {
        if chunk.is_empty() {
            return;
        }
        let ends_with_newline = chunk.ends_with('\n');
        if self.options.color {
            if self.out.write_all(chunk.as_bytes()).is_err() {
                self.write_failed = true;
                return;
            }
        } else {
            let stripped = strip_ansi(chunk);
            if stripped.is_empty() {
                return;
            }
            if self.out.write_all(stripped.as_bytes()).is_err() {
                self.write_failed = true;
                return;
            }
        }
        if self.out.flush().is_err() {
            self.write_failed = true;
        }
        self.at_line_start = ends_with_newline;
    }
}

// ============================================================================
// 순수 헬퍼
// ============================================================================

/// 권한 프롬프트의 키 목록. 어휘는 zo TUI 의 `key_to_permission_decision` 과
/// 같다.
const PERMISSION_KEYS: &str = "[y]once [a]always [n]deny";

/// 질문 프롬프트의 키 목록. 선택지가 없으면 자유 입력만 받는다. 어휘는
/// `crate::ide::prompt::parse_question_answer` 가 실제로 받는 것과 같다 —
/// 다중 선택일 때만 쉼표 목록이 답이 된다.
fn question_keys(option_count: usize, multi_select: bool) -> String {
    if option_count == 0 {
        "free text".to_string()
    } else if multi_select {
        format!("[1]-[{option_count}] (comma for several) or free text")
    } else {
        format!("[1]-[{option_count}] or free text")
    }
}

/// 비-Bash 툴콜의 한 줄 요약. 구조화된 preview 를 우선하고, 없으면 어댑터가
/// 준 `summary` 로 떨어진다.
fn preview_summary(preview: &ToolPreview, fallback: &str) -> String {
    let detail = match preview {
        ToolPreview::Bash { command } => command.clone(),
        ToolPreview::Read { path, range } => match range {
            Some((start, end)) => format!("{path}:{start}-{end}"),
            None => path.clone(),
        },
        ToolPreview::Write { path, byte_count } => format!("{path} ({byte_count} bytes)"),
        ToolPreview::Edit { path, hunk_count } => format!("{path} ({hunk_count} hunks)"),
        ToolPreview::Glob { pattern } => pattern.clone(),
        ToolPreview::Grep { pattern, path } => match path {
            Some(path) => format!("{pattern} in {path}"),
            None => pattern.clone(),
        },
        ToolPreview::Search { query } => query.clone(),
        ToolPreview::Generic { input_summary, .. } => input_summary.clone(),
    };
    let detail = sanitize_inline(detail.trim());
    if detail.is_empty() {
        sanitize_inline(fallback.trim())
    } else {
        detail
    }
}

/// 툴 결과 본문 — codex 는 출력 **원문**을 그대로 흘린다. 여기서도 색·아이콘·
/// 잘라내기를 얹지 않는다. (`crate::tool_formatting` 의 카드 포매터는 TUI 용
/// 256색 어휘라 이 문법과 섞이지 않는다.)
fn result_body_text(body: &ToolResultBody) -> String {
    match body {
        ToolResultBody::Bash(result) => bash_body_text(result),
        ToolResultBody::Text { content, truncated }
        | ToolResultBody::Generic {
            content, truncated, ..
        }
        | ToolResultBody::Read {
            content, truncated, ..
        } => with_truncation_note(content.trim_end_matches('\n'), *truncated),
        ToolResultBody::Listing { entries, truncated } => {
            with_truncation_note(&entries.join("\n"), *truncated)
        }
        ToolResultBody::Diff(diff) => diff_body_text(diff),
        ToolResultBody::Todos(items) => todos_body_text(items),
        ToolResultBody::Declined { reason } => reason.trim_end_matches('\n').to_string(),
    }
}

fn bash_body_text(result: &BashResult) -> String {
    let mut parts = Vec::new();
    let stdout = result.stdout.trim_end_matches('\n');
    if !stdout.is_empty() {
        parts.push(stdout.to_string());
    }
    let stderr = result.stderr.trim_end_matches('\n');
    if !stderr.is_empty() {
        parts.push(stderr.to_string());
    }
    with_truncation_note(&parts.join("\n"), result.truncated)
}

fn diff_body_text(diff: &DiffView) -> String {
    let mut lines = Vec::new();
    for hunk in &diff.hunks {
        lines.push(format!(
            "@@ -{},{} +{},{} @@",
            hunk.old_start, hunk.old_lines, hunk.new_start, hunk.new_lines
        ));
        for line in &hunk.lines {
            let marker = match line.kind {
                DiffLineKind::Added => '+',
                DiffLineKind::Removed => '-',
                DiffLineKind::Context => ' ',
            };
            lines.push(format!("{marker}{}", line.text));
        }
    }
    lines.join("\n")
}

fn todos_body_text(items: &[TodoResultItem]) -> String {
    items
        .iter()
        .map(|item| match item.status {
            TodoResultStatus::Pending => format!("[ ] {}", item.content),
            TodoResultStatus::InProgress => format!("[~] {}", item.active_form),
            TodoResultStatus::Completed => format!("[x] {}", item.content),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 어댑터가 이미 자른 본문에 그 사실만 한 줄 덧붙인다. 렌더러가 스스로
/// 자르지는 않는다 — 툴은 인프로세스라 본문 전체가 여기 있다.
fn with_truncation_note(text: &str, truncated: bool) -> String {
    let text = text.trim_end_matches('\n');
    if !truncated {
        return text.to_string();
    }
    if text.is_empty() {
        "… (truncated)".to_string()
    } else {
        format!("{text}\n… (truncated)")
    }
}

/// 천 단위 콤마 — 캡처의 `6,575`.
fn with_thousands(value: u64) -> String {
    crate::util::group_digits(value, ',')
}

#[cfg(test)]
mod tests {
    /// The declared vocabulary and what the emitter can actually say must be
    /// the same set.
    ///
    /// [`super::JSON_EVENT_TYPES`] is what a consumer matches on, so it is a
    /// contract — and a contract that lives only in a doc comment drifts the
    /// first time somebody adds an arm. This reads the emitter's own source
    /// (the two `push_json`/`json_work` bodies are the only places a `"type"`
    /// literal is written) and requires equality in both directions: an
    /// emitted name missing from the list is undocumented, and a listed name
    /// nothing emits is a promise nobody keeps.
    #[test]
    fn the_json_vocabulary_matches_what_the_emitter_can_say() {
        let source = include_str!("render.rs");
        let body = &source[..source.find("#[cfg(test)]").expect("tests live at the end")];
        let mut emitted: Vec<&str> = Vec::new();
        let needle = "\"type\": \"";
        let mut rest = body;
        while let Some(at) = rest.find(needle) {
            rest = &rest[at + needle.len()..];
            let end = rest.find('"').expect("a type literal closes");
            emitted.push(&rest[..end]);
            rest = &rest[end..];
        }
        emitted.sort_unstable();
        emitted.dedup();

        let mut declared: Vec<&str> = super::JSON_EVENT_TYPES.to_vec();
        declared.sort_unstable();
        assert!(
            !emitted.is_empty(),
            "the scan found no `\"type\"` literals — it stopped seeing the emitter"
        );
        assert_eq!(
            emitted, declared,
            "the emitted vocabulary and JSON_EVENT_TYPES have diverged"
        );
    }

    use super::*;

    #[test]
    fn thousands_separator_matches_capture() {
        assert_eq!(with_thousands(0), "0");
        assert_eq!(with_thousands(999), "999");
        assert_eq!(with_thousands(6_575), "6,575");
        assert_eq!(with_thousands(1_000_000), "1,000,000");
    }

    #[test]
    fn question_keys_degrade_to_free_text() {
        assert_eq!(question_keys(0, false), "free text");
        assert_eq!(question_keys(0, true), "free text");
        assert_eq!(question_keys(3, false), "[1]-[3] or free text");
        assert_eq!(
            question_keys(3, true),
            "[1]-[3] (comma for several) or free text"
        );
    }

    #[test]
    fn truncation_note_is_only_added_when_flagged() {
        assert_eq!(with_truncation_note("out", false), "out");
        assert_eq!(with_truncation_note("out", true), "out\n… (truncated)");
        assert_eq!(with_truncation_note("", true), "… (truncated)");
    }
}
