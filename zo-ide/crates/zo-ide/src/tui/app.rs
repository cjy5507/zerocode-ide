//! 대화형 프런트의 상태기계 — 부팅 카드부터 턴 구동까지.
//!
//! 엔진 경계는 그대로다: [`PlainSession`] 이 턴을 돌리고 `RenderBlock` 채널이
//! 화면 재료를 준다. 이 파일은 그 재료를 [`super::cells`] 로 셀을 만들어
//! [`super::painter::Painter`] 에 넘기고, 키보드를 컴포저·다이얼로그로 나른다.
//!
//! 화면 상태(`Ui`)와 세션은 **다른 구조체**다. 턴 future 가 세션을 빌린 채
//! 도는 동안에도 shimmer 프레임을 그려야 하는데, 한 구조체 안에 있으면
//! 빌림이 겹친다.
//!
//! 인터럽트 규칙은 codex 와 같다 — 턴 중에는 Esc·Ctrl-C 가 취소,
//! 유휴에서 컴포저가 비어 있으면 Ctrl-C 두 번이 종료, 비어 있지 않으면
//! 한 번은 지우기다.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use base64::Engine;
use core_types::usage::TokenUsage;
use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, Event, EventStream, KeyCode, KeyEvent,
    KeyEventKind, KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use futures_util::StreamExt;
use runtime::message_stream::anthropic::tools::{
    format_indirected_tool_result, format_tool_result_from_raw, preview_summary,
    preview_tool_input,
};
use runtime::message_stream::{
    AgentResultStatus, BlockIdGen, NotificationRoad, PermissionDecision, RenderBlock, SystemLevel,
    ToolCallId, ToolCallStatus, ToolPresentation, ToolPreview, ToolResultBody,
};
use runtime::PermissionMode;

use super::agents;
use super::bell;
use super::transcript;
use super::tty;
use super::ansi::{char_width, Line, Style};
use super::cells::{self, MarkdownStream};
use super::clipboard_paste::paste_image_to_temp_png;
use super::chunking::COMMIT_TICK;
use super::composer::{Composer, Submission};
use super::effort_effect::{EffortEffect, EffortTier};
use super::fast;
use super::folds::{FoldIds, FoldMode};
use super::models::{self, ModelChoice};
use super::painter::{Painter, MIN_ROWS};
use super::palette::LateOscGuard;
use super::pending_input::PendingInputs;
use super::permissions::{self, active_permission_label, permission_rank};
use super::question;
use super::sessions;
use super::slash;
use super::summary::{self, SessionSummary};
use super::tools::{Explored, Outcome, ToolCall, ToolGroup, ToolKind};
use super::view::{
    self, Dialog, Frame, Picker, PickerRow, Popup, PopupRow, Status, StatusDetailsCapitalization,
    STATUS_DETAILS_DEFAULT_MAX_LINES,
};
use crate::effort::Effort;
use crate::autonomy::driver::{self, wait_for_wakeup, LoopTurnEnd, TurnOutcome};
use crate::autonomy::loops::LoopTurn;
use crate::autonomy::scheduler::now_unix_ms;
use crate::goal;
use crate::ide::args::RenderFlags;
use crate::ide::channel::state::Command;
use crate::ide::channel::wire::ResolvedBy;
use crate::ide::events::{self, PromptKind};
use crate::ide::prompt::PendingPrompt;
use crate::ide::reporter::HookReporter;
use crate::ide::run_loop::ExitReason;
use crate::session::plain_session::{LaunchFlags, OpenOptions, PlainSession, ReplayItem};
use crate::session::subagent_progress::{SubagentProgress, SubagentProgressWatcher};
use crate::session::turn_scaffold::TurnScaffold;
use crate::session::{AgentCompletionPump, AgentFollowup};
use crate::slash::Slash;

/// 움직이는 화면의 샘플링 간격.
///
/// codex 는 이 둘을 **다른 케이던스**로 돌린다: shimmer 는 32ms
/// (`status_indicator_widget.rs:246`), effort 전환 줄은 33ms
/// (`bottom_pane/effort_status_line.rs`). 우리 드라이브 루프는 티커가 하나라
/// 둘 중 하나를 골라야 하는데, **32ms 쪽이 맞다** — shimmer 는 이 틱이 곧
/// 케이던스지만 effort 이펙트는 벽시계로 자기 구간을 재므로
/// (`elapsed_at`), 조금 자주 표본을 떠도 620/700/360/340/480 타임라인은
/// 한 밀리초도 움직이지 않는다. 반대로 33ms 를 고르면 shimmer 만 실측에서
/// 밀린다.
const FRAME_TICK: Duration = Duration::from_millis(32);

/// How often an idle zo asks the pty for its size (see [`Ui::reconcile_size`]).
/// One `TIOCGWINSZ` a second is nothing; a viewport parked mid-screen for an
/// hour because a resize signal never arrived is what it prevents.
const SIZE_POLL: Duration = Duration::from_secs(1);
/// Ignore quit-shaped control noise while a freshly rendered pane settles.
///
/// The window can finish a pointer-driven launch while terminal protocol
/// replies and the hidden input sink are still crossing. Eight hundred
/// milliseconds bounds that machine handoff below the existing one-second
/// human double-interrupt gesture.
const STARTUP_QUIT_GRACE: Duration = Duration::from_millis(800);
/// 유휴에서 Ctrl-C 두 번이 이 안에 오면 종료.
const DOUBLE_INTERRUPT_WINDOW: Duration = Duration::from_secs(1);
const QUIT_SHORTCUT_REMINDER: &str = "ctrl + c again to quit";
/// 한 프레임에서 따라잡을 커밋 틱의 상한.
///
/// codex 는 커밋 애니메이션을 `COMMIT_ANIMATION_TICK`(= `TARGET_FRAME_INTERVAL`
/// = 8.33ms) 마다 돌린다. 우리 프레임은 [`FRAME_TICK`](32ms) 이라 그 사이에 네
/// 틱쯤이 밀린다 — 프레임마다 밀린 만큼 따라잡되, 한 프레임이 스크롤백에 쏟는
/// 양은 여기서 막는다(케이던스를 결정하는 것은 `chunking::Policy` 다).
const MAX_CATCH_UP_TICKS: u32 = 8;

/// Measure the worktree identity that `ZeroCode` recorded at creation time.
///
/// This runs once when a TUI session is attached (and once more on `/resume`),
/// never from paint. `symbolic-ref` rejects detached HEADs, while requiring the
/// `wt/` namespace and its `branch.<name>.base` record keeps ordinary checkouts
/// and repositories that happen to have an upstream out of `ZeroCode` chrome.
fn worktree_context(cwd: &Path) -> Option<String> {
    let branch = git_value(cwd, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    if !branch.starts_with("wt/") {
        return None;
    }
    let key = format!("branch.{branch}.base");
    let base = git_value(cwd, &["config", "--get", &key])?;
    Some(format!("{branch} ← {base}"))
}

fn git_value(cwd: &Path, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    let value = crate::util::ansi::sanitize_inline(stdout.lines().next()?.trim());
    (!value.is_empty()).then_some(value)
}

fn run_due_commit_ticks(
    last_commit: &mut Option<Instant>,
    frame_now: Instant,
    mut policy_now: impl FnMut() -> Instant,
    mut tick: impl FnMut(Instant) -> Vec<Line>,
) -> Vec<Line> {
    let last = last_commit.unwrap_or(frame_now);
    let elapsed = frame_now.saturating_duration_since(last);
    let ticks = (elapsed.as_nanos() / COMMIT_TICK.as_nanos()).max(1);
    let ticks = u32::try_from(ticks)
        .unwrap_or(u32::MAX)
        .min(MAX_CATCH_UP_TICKS);
    let mut out = Vec::new();
    for _ in 0..ticks {
        out.extend(tick(policy_now()));
    }
    *last_commit = Some(last + COMMIT_TICK * ticks);
    out
}

/// `RenderBlock` 이 TUI 에 닿은 순서를 남기는 opt-in 계측 파일.
///
/// 값은 로그 파일 경로다. 환경변수가 없으면 파일도 락도 만들지 않는다.
/// stdout/stderr 를 쓰지 않는 이유는 전체 화면 PTY 바이트와 계측을 섞지 않고,
/// 두 실행의 화면 타이밍을 같은 방식으로 비교하기 위해서다.
const PROFILE_TOOL_RESULTS_ENV: &str = "ZO_PROFILE_TOOL_RESULTS";
static PROFILE_TOOL_RESULTS: OnceLock<Option<Mutex<std::fs::File>>> = OnceLock::new();

/// `/model` 피커 2단계의 사다리 — forge `Effort::ALL` 을 순서 그대로 전부.
///
/// codex 는 이 두 단계(`Select Model and Effort`)가 effort 를 고르는 **유일한**
/// 자리다. zo 도 그렇다 — `/effort` 는 이 라운드에서 없앴다.
const PICKER_EFFORTS: &[Effort] = Effort::ALL;

/// `/resume` 피커가 읽는 최근 세션 수. codex 는 페이지네이션으로 더 불러오지만
/// (`resume_picker::pagination`) 우리 번호 피커는 한 페이지라 여기서 끊는다.
const RESUME_LIST_LIMIT: usize = crate::resume::DEFAULT_LIST_LIMIT;

/// TUI 가 열릴 수 있는 화면인지. 아니면 호출자가 파이프 경로로 되돌린다.
#[must_use]
pub fn terminal_is_usable() -> bool {
    crossterm::terminal::size().is_ok_and(|(cols, rows)| cols >= 20 && rows >= MIN_ROWS)
}

/// 대화형 프런트를 끝까지 돈다.
pub async fn run(
    session: PlainSession,
    flags: RenderFlags,
) -> Result<ExitReason, Box<dyn std::error::Error>> {
    Box::pin(run_with_last_message(session, flags, None)).await
}

/// [`run`] 에 `--last-message` 의 목적지를 더한 것.
///
/// 대화형 프런트에도 있는 이유는 계약이기 때문이다 — 같은 플래그가 파이프
/// 에서는 답을 남기고 터미널에서는 안 남기면, 그건 플래그가 아니라 우연이다.
pub async fn run_with_last_message(
    session: PlainSession,
    flags: RenderFlags,
    last_message: Option<std::path::PathBuf>,
) -> Result<ExitReason, Box<dyn std::error::Error>> {
    // This is the interactive main session, so an `Agent` call that omits
    // `background` detaches.
    //
    // The tool's own description promises exactly that ("Detached by default
    // in the interactive main session"), and the dispatcher
    // already defers to this cell — but nothing outside tests ever set it, so
    // every omitted `background` blocked. A blocking spawn pins the whole turn
    // for as long as the child runs, and while a turn is pinned inside a tool
    // there is no boundary to fold a steer into: the user types, sees
    // "steer queued", and the model does not answer for minutes. That is the
    // symptom this one line fixes.
    //
    // Only here. A sub-agent executor gets a fresh `ToolContext`, and a piped
    // or headless run has no REPL to re-inject a detached result into — both
    // keep the blocking default, or the child's report would be lost.
    session.set_background_agent_default(true);
    enable_raw_mode()?;
    let _ = execute!(std::io::stdout(), EnableBracketedPaste);
    // 상태기계 future 는 30KB 남짓이다 — select! 가 그걸 스택에 얹으면
    // 재귀 호출부에서 부담이 되므로 힙에 둔다.
    let outcome = match App::new(session, flags) {
        Ok(app) => Box::pin(app.drive()).await,
        Err(error) => Err(error),
    };
    let _ = execute!(std::io::stdout(), DisableBracketedPaste);
    let _ = disable_raw_mode();
    let (reason, summary, last_answer) = outcome?;
    crate::ide::run_loop::write_last_message(last_message.as_deref(), &last_answer);
    print_exit_summary(summary);
    Ok(reason)
}

/// 종료 요약을 **cooked 모드로 돌아온 뒤**에 찍는다 — codex `main.rs` 도
/// `run_main` 이 끝난 다음에 찍는다. raw 모드에서 찍으면 tty 의 ONLCR 이 없어
/// `\n` 이 `\r\n` 이 되지 않고 둘째 줄이 계단으로 밀린다.
///
/// 터미널이 이미 닫혔으면 쓰기가 실패하고, 그것으로 끝이다
/// ([`SessionSummary::write_exit_lines`]).
fn print_exit_summary(summary: Option<SessionSummary>) {
    if let Some(summary) = summary {
        let _ = summary.write_exit_lines(&mut std::io::stdout().lock(), !crate::render::no_color_env());
    }
}

/// What a teammate pane's whole life came to (t-2513 §2.2): why it left and
/// how many turns it answered. The per-turn results are already on disk —
/// the loop writes each `result-<n>.json` the moment its turn ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TeammateLife {
    pub reason: runtime::subagent_panes::CloseReason,
    pub turns: u32,
}

/// Run a parent's brief as its teammate: the first turn, then idle for the
/// parent's next words, until something ends the pane.
///
/// The same front-end a person drives, deliberately: the whole point of a
/// teammate pane is that its work is watchable and interruptible, so this is
/// not a headless runner with a TUI painted over it. It differs from [`run`]
/// in three places — the first turn arrives from a brief instead of from the
/// keyboard, later turns arrive over the events channel (`session.steer`),
/// and the process leaves when its parent closes it, its parent's channel
/// disappears, its idle budget runs out, or a person at its keyboard leaves
/// (`crate::teammate`).
pub async fn run_teammate(
    session: PlainSession,
    flags: RenderFlags,
    prompt: String,
    banner: String,
    lifecycle: crate::teammate::Lifecycle,
) -> Result<TeammateLife, Box<dyn std::error::Error>> {
    enable_raw_mode()?;
    let _ = execute!(std::io::stdout(), EnableBracketedPaste);
    let outcome = match App::new(session, flags) {
        Ok(app) => Box::pin(app.drive_teammate(prompt, banner, lifecycle)).await,
        Err(error) => Err(error),
    };
    let _ = execute!(std::io::stdout(), DisableBracketedPaste);
    let _ = disable_raw_mode();
    let (summary, life) = outcome?;
    print_exit_summary(summary);
    Ok(life)
}

/// 지금 열려 있는 스트리밍 세그먼트.
#[derive(Debug)]
enum Segment {
    None,
    Text(u64, MarkdownStream),
    Reasoning(u64, MarkdownStream),
}

/// announce 된 도구 호출 — 결과 블록이 이름과 대상 경로를 되찾는다.
///
/// 시간은 여기 없다. 도는 셀 자체가 [`ToolGroup`] 안에서 제 시작 시각을 들고
/// 있고(codex `ExecCall::start_time`), 걸린 시간은 codex 의 커밋된 셀 문법에
/// 아예 나오지 않는다.
#[derive(Debug)]
struct PendingTool {
    name: String,
    /// Effective inner name used to format a `CapabilityInvoke` result.
    result_name: String,
    detail: String,
    /// Edit·Write 의 대상 — 결과 diff 에 경로가 없을 때의 폴백.
    path: String,
    /// Typed preview kind retained for an orphaned result.
    kind: ToolKind,
    /// Successful internal preparation stays available in the transcript only.
    presentation: ToolPresentation,
}

#[derive(Debug)]
struct QuietLoopCell {
    loop_id: String,
    lines: Vec<Line>,
}

#[derive(Clone, Copy)]
enum LiveSlotRequest<'a> {
    Tool(&'a ToolKind),
    Quiet(&'a str),
}

/// 사람의 답을 기다리는 프롬프트와 그 화면.
struct Parked {
    prompt: PendingPrompt,
    screen: ParkedScreen,
    /// IDE 이벤트 채널에 등록된 프롬프트 id — 패인과 IDE 모달이 같은 id 를 본다.
    prompt_id: Option<u64>,
}

/// 파킹된 프롬프트가 뷰포트에 세우는 화면. codex 도 이 둘을 다른 위젯으로
/// 그린다 — 권한은 `bottom_pane/approval_overlay.rs`(번호 다이얼로그),
/// 질문은 `bottom_pane/request_user_input/`(진행 줄 + 시안 질문 + 힌트 푸터).
enum ParkedScreen {
    Dialog(Dialog),
    Question(view::Question),
}

/// `/model` 피커의 단계. 캡처의 제목이 "Select Model **and Effort**" 이므로
/// 모델을 고른 뒤 같은 문법으로 effort 를 잇는다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Model,
    Effort,
}

/// 떠 있는 `/model` 피커 — 목록·화면·단계.
struct ModelPicker {
    models: Vec<ModelChoice>,
    view: Picker,
    stage: Stage,
    /// 1단계에서 고른 모델. 2단계에서 effort 와 함께 확정된다.
    chosen: Option<String>,
}

/// 떠 있는 `/permissions` 피커 — codex의 approval preset 행을 기존 zo 권한
/// 모드로 되돌릴 매핑을 함께 든다.
struct PermissionPicker {
    modes: Vec<PermissionMode>,
    view: Picker,
}

/// 턴 중에 눌렀지만 지금 돌 수 없는 일 — 턴이 끝나면 [`App`] 이 푼다.
///
/// codex 는 턴 중에도 `/model`·`/permissions`·`/status` 를 바로 돌린다
/// (`slash_command_blocked_by_active_task` + `available_during_task`). zo 는
/// 턴이 도는 동안 세션을 **턴 task 가 통째로 가져가므로** 화면만 만지는 것
/// (피커 열기·단축키 카드)은 즉시 하고, 세션을 만져야 하는 것은 여기 담아
/// 턴 직후에 돌린다 — 사용자에게는 codex 와 같은 순간에 반응하고 결과가 다음
/// 턴부터 적용된다.
enum Deferred {
    /// `/model` 피커가 턴 중에 확정한 모델 + effort.
    Choice(String, Effort),
    /// `/fast` 피커가 즉시 표시한 serving-tier 전환.
    Fast(bool),
    /// `/permissions` 피커가 즉시 표시한 permission-mode 전환.
    Permission(PermissionMode),
    /// 세션을 만져야 하는 슬래시 한 줄(앞의 `/` 는 뗀 상태).
    Slash(String),
    /// `/resume` 피커가 턴 중에 고른 세션 id.
    Resume(String),
}

/// 한 키가 남긴 일.
enum KeyOutcome {
    Nothing,
    Submitted(Submission),
    /// `/model` 피커가 확정한 모델과 effort. 세션을 만지는 일이라 화면이 아니라
    /// [`App`] 이 처리한다.
    Chosen(String, Effort),
    /// `/permissions` 피커가 고른 zo 권한 모드.
    Permission(PermissionMode),
    /// `/resume` 피커가 고른 세션 id. 세션을 갈아 끼우는 일이라 [`App`] 이 한다.
    Resumed(String),
    /// Codex's fixed Alt+A binding: open this session's running-agent overview.
    OpenAgents,
}

/// 화면 쪽 상태 전부.
/// Whether the current text cell's head has been checked for an imitated
/// `[earlier reasoning]` label: still holding its first bytes, or past that.
#[derive(Debug)]
enum PassportGate {
    Holding(String),
    Passed,
}

impl Default for PassportGate {
    fn default() -> Self {
        Self::Holding(String::new())
    }
}

#[derive(Default)]
struct SubagentWave {
    seen: HashSet<String>,
    running: usize,
}

impl SubagentWave {
    fn refresh(&mut self, progress: &[SubagentProgress]) {
        self.seen
            .extend(progress.iter().map(|agent| agent.agent_id.clone()));
        self.running = progress.len();
    }

    fn summary(&self) -> Option<String> {
        (!self.seen.is_empty()).then(|| super::strings::helper_wave(self.seen.len(), self.running))
    }
}

enum PublishedActivity {
    Never,
    Value(Option<events::ActivityCard>),
}

struct Ui {
    flags: RenderFlags,
    painter: Painter<std::io::Stdout>,
    paint_probe: Option<std::fs::File>,
    composer: Composer,
    /// Turn-time input is bottom-pane state until it actually becomes a user
    /// turn. Enter steers and Tab queues; neither is optimistic history.
    pending_input: PendingInputs,
    /// History handed over since the last frame, inserted once that frame has
    /// sized the head — see [`Ui::history`].
    pending_history: Vec<Line>,
    /// The head of a text cell, held until it is known not to be an imitated
    /// `[earlier reasoning]` label — see [`Ui::absorb_passport_label`].
    passport_gate: PassportGate,
    segment: Segment,
    /// announce 된 채 결과를 기다리는 호출들.
    tools: HashMap<String, PendingTool>,
    /// Calls already represented by an append-only history cell this turn.
    ///
    /// A late duplicate result cannot edit that cell, but it must not create a
    /// second orphan cell either. The set is cleared at each turn boundary, so
    /// its size is bounded by one turn's calls.
    committed_tool_calls: HashSet<String>,
    /// Announced calls the live cell could not seat, in announce order.
    ///
    /// zo announces a message's whole batch before any result, and the
    /// viewport has one live cell. A call of another kind used to take the
    /// slot from a cell whose calls were still running, closing them as
    /// failed — four successful edits drawn as `✘ Failed to apply patch`
    /// because a `bash` was announced after them (t-3063). Now it waits
    /// here and is seated when the cell finishes ([`Ui::seat_waiting_tools`]);
    /// a result that arrives first lands as its own committed cell.
    unseated: Vec<String>,
    /// 아직 히스토리로 안 내려간 도구 셀 — codex `transcript.active_cell` 에
    /// 앉는 `ExecCell` 하나다. 전부 성공으로 끝나도 닫지 않는다: 다음 명령이
    /// 와서 `Ran N commands` 로 접힐 수 있게 열어 둔다
    /// (`exec_cell/model.rs::should_flush`).
    tool_cell: Option<ToolGroup>,
    /// A consecutive noop streak stays in the viewport's one mutable cell.
    /// The line is committed only when unrelated history needs the slot.
    quiet_loop_cell: Option<QuietLoopCell>,
    /// History produced by an iteration that may still report `noop:true`.
    /// It remains off scrollback until the receipt resolves that question.
    quiet_candidate: Option<Vec<Line>>,
    /// This turn performed concrete exec/MCP/edit work and earns a final rule.
    had_work_activity: bool,
    /// Successful spawn-family results in this foreground turn. Kept separate
    /// from live progress rows: a fast child may finish between watcher polls.
    turn_agent_count: usize,
    /// 접기 마커의 id 발급기 — 세션 안에서 단조 증가한다.
    folds: FoldIds,
    /// 접기 본문을 낼지 — `ZeroCode` 패인 안인가 맨 터미널인가. 런치 env 를
    /// [`App::new`] 가 한 번만 읽어 여기 실어 둔다(렌더러는 env 를 안 본다).
    fold_mode: FoldMode,
    /// 마지막으로 돌린 커밋 애니메이션 틱의 시각.
    last_commit: Option<Instant>,
    /// 턴이 끝나면 풀 일.
    deferred: Vec<Deferred>,
    status: Option<Status>,
    /// Ordinary foreground-tool context, used whenever no sub-agent rows are
    /// active. Kept outside [`Status`] so a watcher snapshot can temporarily
    /// take over the detail area without losing the tool text beneath it.
    tool_status_detail: Option<String>,
    /// Latest owned snapshot from the asynchronous manifest/transcript watcher.
    /// Paint code only formats these values; it never performs I/O.
    subagent_progress: Vec<SubagentProgress>,
    /// Helpers observed in the current foreground wave. Finished helpers leave
    /// the running snapshot but stay in this count until the turn ends.
    subagent_wave: SubagentWave,
    /// Last activity object sent, including an explicit idle clear.
    published_activity: PublishedActivity,
    /// Reasoning text of the block in flight, kept only until its first bold
    /// heading closes. That heading becomes the shimmer word — see
    /// [`first_bold_heading`] — so it is collected even when the thinking body
    /// itself is hidden: "what is it doing" is the question a long unattended
    /// run needs answered, and it must not depend on `--show-thinking`.
    ///
    /// `Some((id, Some(buf)))` is still looking inside block `id`;
    /// `Some((id, None))` already took that block's heading and stops buffering.
    reasoning_scan: Option<(u64, Option<String>)>,
    parked: Option<Parked>,
    picker: Option<ModelPicker>,
    /// 떠 있는 `/resume` 화면 — codex `resume_picker.rs` 를 옮긴 것
    /// ([`super::sessions::SessionPicker`]). 번호 피커와 달리 자기 크롬
    /// (검색·툴바·구분선)을 들고 있어서 [`Picker`] 가 아니다. `picker` 와 같은
    /// 뷰포트 자리를 쓰므로 둘이 동시에 뜨는 일은 없다
    /// ([`Ui::open_resume_picker`] 가 먼저 확인한다).
    sessions: Option<sessions::SessionPicker>,
    /// 떠 있는 `/permissions` 피커. 모델·재개 피커와 같은 뷰포트 자리를 쓴다.
    permissions: Option<PermissionPicker>,
    /// Alt+A session-scoped in-process agent overview.
    agents: Option<agents::Overview>,
    /// Ctrl+T 로 연 트랜스크립트 페이저. 턴 중에도 열린다 — 긴 자율 실행에서
    /// 20분 전 도구가 무엇을 찍었는지 읽으려고 턴을 끊을 수는 없다.
    transcript: Option<transcript::Transcript>,
    /// 이 UI 가 실제로 보여 준 턴들. 세션에서 읽지 않고 여기 쌓는 이유는
    /// 하나다 — 턴이 도는 동안 세션은 턴 future 가 가져가서 없다
    /// ([`App::session`] 이 "only away while a turn runs" 라고 적는 그것),
    /// 그런데 트랜스크립트가 가장 필요한 때가 바로 그때다. `/resume` 은
    /// 재생하면서 같은 자리들을 지나가므로 이어붙기도 저절로 맞는다.
    transcript_items: Vec<transcript::Entry>,
    /// 스트리밍 중인 답변의 원문 — `done` 에서 한 항목으로 앉는다.
    transcript_answer: String,
    /// 마지막으로 **완성된** 어시스턴트 답변 — `--last-message` 의 값.
    last_answer: String,
    /// [`Ui::transcript_items`] 가 지금 쥔 본문 바이트. 예산을 넘으면 오래된
    /// 도구 본문부터 놓는다 — 세는 값을 따로 들고 다니는 이유는 매 도구마다
    /// 목록 전체를 다시 재면 O(n²) 이기 때문이다.
    transcript_bytes: usize,
    /// 자동완성 팝업에서 고른 줄. 팝업 자체는 컴포저 원문에서 매번 다시
    /// 만든다 — 상태로 남길 것은 커서뿐이다.
    popup_selected: usize,
    reporter: Option<HookReporter>,
    model: String,
    /// The model actually on the wire when it is not `model` — a
    /// refusal or quota fallback, a deep-gate leg — with the footer's word for
    /// why. Set by `RenderBlock::WireModel`; cleared when a request goes out
    /// on the session model again, or when the person picks a model.
    wire: Option<super::wire_model::Badge>,
    /// provider 모델 id에서 분리한 TUI fast 표시 상태.
    fast: bool,
    effort: String,
    /// 현재 tier의 지속 caret accent와, 끝나면 유휴로 돌아가는 일회성 전환.
    effort_tier: Option<EffortTier>,
    effort_effect: Option<EffortEffect>,
    /// Context tokens occupied **right now**, from the latest response.
    ///
    /// Deliberately not derived from [`Self::usage`], which is the session
    /// cumulative and drives cost. With prompt caching, cumulative
    /// `context_tokens()` adds roughly the whole window again on every turn, so
    /// a long session reads as `0% context left` after a handful of turns.
    /// codex splits the same way — its percentage comes from `last_token_usage`
    /// and the session total is only the fallback when no window is known.
    /// While armed, an OSC reply that arrived after the palette probe gave up
    /// is swallowed instead of typed. See [`Ui::swallow_late_osc_reply`].
    osc_guard: Option<LateOscGuard>,
    ctx_tokens: u64,
    permission_mode: PermissionMode,
    /// Clone of the registry's shared permission cell, so Shift+Tab reaches a
    /// running turn. See [`PlainSession::permission_cell`] for what it does and
    /// does not switch.
    permission_cell: Option<std::sync::Arc<std::sync::Mutex<Option<PermissionMode>>>>,
    /// Display label used by the footer and session card (`~/…` below HOME).
    cwd: String,
    /// Exact filesystem identity used by the resume picker's `[Cwd]` filter.
    /// A display label is not a path: `canonicalize("~/…")` cannot match the
    /// absolute path stored in a session sidecar.
    session_cwd: std::path::PathBuf,
    /// Cwd plus measured `wt/<branch> ← <base>` for the footer only.
    /// Status/session cards keep [`Self::cwd`] so their Codex cell bytes stay put.
    footer_location: String,
    /// Until this instant, quit-shaped C0 input cannot claim to be a person's
    /// deliberate gesture. Ordinary composer input is unaffected.
    startup_quit_grace_until: Option<Instant>,
    last_interrupt: Option<Instant>,
    /// 지금 세션의 id. 화면이 이미 아는 사실 중 하나로, 턴이 도는 동안
    /// `/status` 가 세션을 건드리지 않고 카드를 세우는 데 쓴다.
    session_id: String,
    /// The session's agent registry — what the roster scan, the Alt+A
    /// overview and the reporter's background-task list read through.
    /// Rebound with `session_id` at `/new` and `/resume`.
    registry: std::sync::Arc<tools::AgentRegistry>,
    /// `/status`가 턴 중에도 세션 borrow 없이 읽는 지속 목표 두 행.
    goal: String,
    autonomous: String,
    loops: String,
    /// 지금 세션이 지금까지 쓴 토큰 — codex `chat_widget.token_usage()` 자리다.
    /// 원장(`RenderBlock::Usage`)이 턴마다 누적값을 주므로 마지막 것만 든다.
    /// 세션이 갈리면 [`Self::reset_conversation`] 이 0 으로 되돌린다.
    usage: TokenUsage,
    usage_known: bool,
    exit: Option<ExitReason>,
}

struct App {
    /// 턴이 도는 동안에는 세션이 **여기 없다** — 턴 future 가 통째로 가져가
    /// 다른 task 에서 돈다([`App::turn`]). 그래야 턴 준비의 동기 구간이
    /// 프레임 틱과 키 입력을 붙잡지 않는다.
    session: Option<PlainSession>,
    agent_completion_pump: Option<AgentCompletionPump>,
    /// SIGTERM and SIGHUP, when the platform delivers them: either ends the
    /// session the way Esc-then-exit does — the turn cancelled, the screen
    /// put back, the transcript flushed — instead of the process dying with
    /// the terminal in raw mode ("cc처럼 완벽하게 종료되야함", 2026-09-02).
    signals: Option<TerminationSignals>,
    /// Session-wide roster forwarding for the pane's events channel. Unlike
    /// the turn-local watcher used for paint, this remains alive while a
    /// detached child outlives its spawning foreground turn.
    subagent_frame_relay: Option<events::SubagentFrameRelay>,
    /// A parent's `teammate.close` that arrived mid-turn (t-2513 §2.2): the
    /// turn was cancelled, and the teammate loop reads the reason here to
    /// write its closing document instead of waiting for the next word.
    close_requested: Option<String>,
    ui: Ui,
}

/// How an idle teammate's wait ended.
enum IdleOutcome {
    /// The parent's next words: the next turn's prompt.
    NextTurn(String),
    /// The pane is done, and why.
    Close(runtime::subagent_panes::CloseReason),
}

async fn recv_agent_followup(pump: &mut Option<AgentCompletionPump>) -> AgentFollowup {
    let Some(pump) = pump.as_mut() else {
        return std::future::pending().await;
    };
    match pump.recv_followup().await {
        Some(followup) => followup,
        None => std::future::pending().await,
    }
}

/// What the shared autonomy driver borrows from the TUI: the app (session,
/// screen, reporter) and the event stream a turn reads.
struct TuiFrontend<'a> {
    app: &'a mut App,
    events: &'a mut EventStream,
}

impl driver::AutonomyFrontend for TuiFrontend<'_> {
    fn session(&mut self) -> Option<&mut PlainSession> {
        self.app.session.as_mut()
    }

    fn leaving(&self) -> bool {
        self.app.ui.exit.is_some()
    }

    fn note(&mut self, level: SystemLevel, text: &str) {
        self.app.ui.note(level, text);
    }

    fn reporter(&self) -> Option<&HookReporter> {
        self.app.ui.reporter.as_ref()
    }

    async fn run_turn(&mut self, prompt: &str, allow_writes: bool) -> Option<TurnOutcome> {
        let input = Submission {
            text: prompt.to_string(),
            image_paths: Vec::new(),
        };
        // Boxed here, where a turn actually runs: awaited inline, the turn's
        // screen-sized state rode up through the autonomy driver into
        // `drive_autonomy`, which the event loop builds after every key.
        let outcome = Box::pin(
            self.app
                .turn(&input, self.events, Some(allow_writes), None),
        )
        .await;
        self.app.session.is_some().then_some(outcome)
    }

    fn status_changed(&mut self) {
        self.app.sync_goal_display();
        self.app.ui.draw();
    }

    fn loop_turn_started(&mut self, _turn: &LoopTurn, dynamic_iteration: bool) {
        if dynamic_iteration {
            self.app.ui.begin_quiet_candidate();
        }
    }

    fn loop_turn_ended(&mut self, turn: &LoopTurn, dynamic_iteration: bool, end: LoopTurnEnd<'_>) {
        let (noop, quiet_streak, notice) = match end {
            LoopTurnEnd::Recorded {
                noop,
                quiet_streak,
                notice,
            } => (noop, quiet_streak, Some(notice)),
            LoopTurnEnd::Dropped => (false, 0, None),
        };
        if dynamic_iteration {
            self.app
                .ui
                .finish_quiet_candidate(&turn.id, noop, quiet_streak, now_unix_ms());
        }
        // A quiet self-paced iteration folds into its candidate row instead
        // of printing its notice.
        if let Some(notice) = notice.filter(|_| !(dynamic_iteration && noop)) {
            self.app.ui.note(SystemLevel::Info, notice);
        }
    }
}

fn load_submission_images(paths: &[PathBuf]) -> Result<Vec<(String, String)>, String> {
    paths
        .iter()
        .map(|path| {
            std::fs::read(path)
                .map(|bytes| ("image/png".to_string(), base64::engine::general_purpose::STANDARD.encode(bytes)))
                .map_err(|error| format!("could not read pasted image {}: {error}", path.display()))
        })
        .collect()
}

/// Codex `history_cell/separators.rs::FinalMessageSeparator::display_lines`,
/// extended only when this turn successfully launched sub-agents. The
/// ` · agents N` suffix uses Codex's own metric-label separator grammar.
///
/// One deliberate departure: Codex draws a bare rule for a turn under a
/// minute and labels only the longer ones. A bare full-width line at the end
/// of an answer read as nothing to the person watching ("끝날때 ㅡㅡㅡㅡ 이게
/// 너무 이상함"), so every turn's rule carries its label — the line then says
/// what it is, however short the turn was. Under a minute the label says so
/// in words rather than seconds: a count that ticks between runs would make
/// every short turn's screen a different screen, and the e2e goldens hold
/// five of them.
fn turn_separator(width: usize, elapsed_seconds: u64, agent_count: usize) -> Line {
    let agents = if agent_count > 0 {
        format!(" · agents {agent_count}")
    } else {
        String::new()
    };
    let worked = if elapsed_seconds < 60 {
        "under a minute".to_string()
    } else {
        super::shimmer::fmt_elapsed_compact(elapsed_seconds)
    };
    let label = format!("─ Worked for {worked}{agents} ─");
    let mut clipped = String::new();
    let mut label_width = 0usize;
    for ch in label.chars() {
        let cell_width = char_width(ch);
        if label_width + cell_width > width {
            break;
        }
        label_width += cell_width;
        clipped.push(ch);
    }
    clipped.push_str(&"─".repeat(width.saturating_sub(label_width)));
    Line::from_text(clipped).styled(Style::new().dim())
}

// ============================================================================
// 화면
// ============================================================================

/// codex binds image paste to **ctrl+v** and **ctrl+alt+v** only
/// (`keymap.rs`: `fixed.paste_image` → `ctrl(V)`, `ctrl_alt(V)`). Alt alone is
/// deliberately not one of them: on macOS Option+V is how a person types `√`,
/// and stealing it would make an ordinary character unreachable.
fn is_paste_image_key(key: &KeyEvent, ch: char) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL) && ch.eq_ignore_ascii_case(&'v')
}

/// One short line naming the tool now running, for the status detail.
///
/// Short on purpose. codex keeps its status header terse so the elapsed and
/// interrupt hints stay on screen (`chatwidget/command_lifecycle.rs`), and its
/// `update_inline_message` doc warns that verbose status prose gets truncated
/// and hides them. The transcript already carries the full call; this line only
/// has to answer "what is it doing right now".
fn status_detail_for(kind: &ToolKind) -> String {
    match kind {
        ToolKind::Command { command, .. } => command.clone(),
        ToolKind::Edit { path } => format!("editing {path}"),
        ToolKind::Explore(items) => items
            .first()
            .map_or_else(|| "exploring".to_string(), explored_detail),
        ToolKind::Plan(_) => "updating plan".to_string(),
        ToolKind::WebSearch { detail } if detail.is_empty() => "searching the web".to_string(),
        ToolKind::WebSearch { detail } => format!("searching the web for {detail}"),
        ToolKind::Spawn { label, .. } => format!("spawning {label}"),
        ToolKind::AgentResult { label, .. } => format!("agent result from {label}"),
        ToolKind::Call { name, detail } if detail.is_empty() => name.clone(),
        ToolKind::Call { name, detail } => format!("{name} · {detail}"),
    }
}

/// One helper's live row inside its spawn cell — Claude Code's
/// `⎿ Running… 12 tool uses · 30s`, carrying the activity zo already polls.
///
/// The status line below answers "which helpers are alive"; this answers "what
/// is THIS one doing", so the count leads and the label — already in the cell
/// header above the row — is left out.
fn subagent_cell_progress(progress: &SubagentProgress) -> String {
    subagent_progress_line(progress)
}

/// One helper's line under `Working`: `label · activity · elapsed`, then the
/// tool count the way Claude Code writes it under a running task ("12 tool
/// uses"), then a quiet warning. The count is what moves while the activity
/// line stands still, so a person can tell a helper at work from one stuck.
fn subagent_status_detail(progress: &SubagentProgress) -> String {
    let mut line = subagent_progress_line(progress);
    if let Some(quiet) = progress.no_new_output_for {
        line.push_str(core_types::helper_run::FACT_SEPARATOR);
        let _ = write!(
            line,
            "no new output for {}",
            super::shimmer::fmt_elapsed_compact(quiet.as_secs())
        );
    }
    line
}

fn subagent_progress_line(progress: &SubagentProgress) -> String {
    let label = crate::util::ansi::sanitize_inline(&progress.label);
    let activity = super::activity::Activity::from_said(&progress.activity);
    super::strings::helper(&label, progress.tool_calls, progress.elapsed, &activity)
}

fn subagent_event_arrived(before: &[SubagentProgress], after: &[SubagentProgress]) -> bool {
    before.len() != after.len()
        || after.iter().any(|next| {
            before
                .iter()
                .find(|previous| previous.agent_id == next.agent_id)
                .is_none_or(|previous| {
                    previous.tool_calls != next.tool_calls || previous.activity != next.activity
                })
        })
}

fn explored_detail(item: &Explored) -> String {
    match item {
        Explored::Read { name } => format!("reading {name}"),
        Explored::List { path } => format!("listing {path}"),
        Explored::Search { query, path: None } => format!("searching {query}"),
        Explored::Search {
            query,
            path: Some(path),
        } => format!("searching {query} in {path}"),
    }
}

/// The kind an unattached result should be drawn as.
///
/// The old code forced `ToolKind::Call` here, so a result that missed its cell
/// lost its rendering: an edit came back as a generic `Called edit_file(…)`
/// with the raw patch text — no line numbers, no colors, no `(+N -M)`. The
/// result still carries everything needed to draw it properly, so use it.
/// Falling back to the announced call's name only when it carries nothing.
fn orphan_kind(outcome: &Outcome, announced: Option<&PendingTool>) -> ToolKind {
    if let Some(change) = outcome.change.as_ref() {
        return ToolKind::Edit {
            path: change.path.clone(),
        };
    }
    announced.map_or_else(
        || ToolKind::Call {
            name: "tool".to_string(),
            detail: String::new(),
        },
        |pending| pending.kind.clone(),
    )
}

/// The mode Shift+Tab moves to.
///
/// The picker offers four rows but only three distinct modes — two of them are
/// the same `WorkspaceWrite` with different approval prose. Cycling walks the
/// **distinct** modes so every press changes something; a press that appeared
/// to do nothing would read as a dropped keystroke.
const fn next_permission_mode(current: PermissionMode) -> PermissionMode {
    match current {
        PermissionMode::ReadOnly => PermissionMode::WorkspaceWrite,
        PermissionMode::WorkspaceWrite => PermissionMode::DangerFullAccess,
        _ => PermissionMode::ReadOnly,
    }
}

/// Publish `mode` into the registry's shared cell, reporting whether it landed.
///
/// A missing cell (no session yet) and a poisoned one both mean the same thing
/// — the live half did not happen — and the caller must say so rather than
/// imply the whole switch took effect.
fn write_permission_cell(
    cell: Option<&std::sync::Mutex<Option<PermissionMode>>>,
    mode: PermissionMode,
) -> bool {
    let Some(cell) = cell else {
        return false;
    };
    let Ok(mut slot) = cell.lock() else {
        return false;
    };
    *slot = Some(mode);
    true
}

/// The first `**bold**` heading in `text`, if one has closed yet.
///
/// codex's rule (`chatwidget.rs::extract_first_bold`) verbatim, including the
/// two refusals that matter while a stream is still arriving: an **unclosed**
/// `**` returns `None` so the header waits for more deltas instead of flashing
/// a half-written phrase, and an empty `****` returns `None` rather than
/// blanking the shimmer.
fn first_bold_heading(text: &str) -> Option<&str> {
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while i + 1 < bytes.len() {
        if bytes[i] == b'*' && bytes[i + 1] == b'*' {
            let start = i + 2;
            let mut j = start;
            while j + 1 < bytes.len() {
                if bytes[j] == b'*' && bytes[j + 1] == b'*' {
                    let trimmed = text[start..j].trim();
                    return (!trimmed.is_empty()).then_some(trimmed);
                }
                j += 1;
            }
            // No closing marker yet — wait rather than guess.
            return None;
        }
        i += 1;
    }
    None
}

/// The plan-submission tool whose result gets its own screen.
///
/// `ExitPlanModeV2` never restores write permission — approval is a separate
/// human action (`/plan off`). See `tools/src/plan_mode_v2.rs`.
const PLAN_SUBMISSION_TOOL: &str = "ExitPlanModeV2";

/// Pull the plan body and its saved path out of an `ExitPlanModeV2` result.
///
/// Returns `None` for anything that is not that shape, so a changed tool
/// output degrades to the ordinary tool cell rather than an empty plan screen.
/// The artifact write is best-effort in the tool, so `planPath` is optional
/// while the plan itself is not.
fn parse_submitted_plan(body: &ToolResultBody) -> Option<(String, Option<String>)> {
    // The tool returns pretty JSON. The live formatter shapes a JSON result
    // it has no renderer for as `Generic` (`format_tool_result_from_raw` →
    // `format_generic_result`), and a wrapped `CapabilityInvoke` result is
    // unwrapped into the same shape; a plain-text body is the other way the
    // same document can arrive. Anything else is not the submission we know.
    let (ToolResultBody::Text { content, .. } | ToolResultBody::Generic { content, .. }) = body
    else {
        return None;
    };
    let value: serde_json::Value = serde_json::from_str(content).ok()?;
    let plan = value.get("plan")?.as_str()?.trim();
    if plan.is_empty() {
        return None;
    }
    let path = value
        .get("planPath")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    Some((plan.to_string(), path))
}

/// 놓은 본문이 남기는 자리. 원문은 세션 전사(디스크)에 그대로 있다.
const DROPPED_BODY: &str =
    "(output dropped to bound memory — the session transcript on disk still has it)";

/// 항목 하나를 트랜스크립트 저장소에 적고, 예산을 넘으면 **오래된 도구
/// 본문부터** 놓는다.
///
/// 자유 함수인 것은 정책을 혼자 시험하기 위해서다 — 이 규칙이 틀리면 긴
/// 실행에서 메모리가 새거나(안 놓거나) 방금 일어난 일을 못 보게 된다
/// (새 것부터 놓거나).
fn record_transcript_item(
    items: &mut Vec<transcript::Entry>,
    bytes: &mut usize,
    item: ReplayItem,
    budget: usize,
) {
    record_transcript_entry(
        items,
        bytes,
        transcript::Entry::Replay(item),
        budget,
    );
}

fn record_transcript_entry(
    items: &mut Vec<transcript::Entry>,
    bytes: &mut usize,
    item: transcript::Entry,
    budget: usize,
) {
    *bytes = bytes.saturating_add(entry_bytes(&item));
    items.push(item);
    // 사용자 프롬프트와 답변은 작고 목록의 뼈대라 남긴다. 부피는 도구
    // 본문이고, 사람이 이 화면을 여는 건 거의 언제나 방금 일어난 일을
    // 보려는 것이므로 앞에서부터 놓는다.
    let mut index = 0;
    while *bytes > budget && index < items.len() {
        if let transcript::Entry::Replay(ReplayItem::ToolCall {
            output: Some(body), ..
        }) = &mut items[index]
        {
            if body != DROPPED_BODY {
                let freed = body.len().saturating_sub(DROPPED_BODY.len());
                DROPPED_BODY.clone_into(body);
                *bytes = bytes.saturating_sub(freed);
            }
        }
        index += 1;
    }
}

fn entry_bytes(item: &transcript::Entry) -> usize {
    match item {
        transcript::Entry::Replay(item) => item_bytes(item),
        transcript::Entry::Reasoning(text) => text.len(),
    }
}

/// 한 항목이 쥔 본문 바이트 — 트랜스크립트 예산이 세는 값.
fn item_bytes(item: &ReplayItem) -> usize {
    match item {
        ReplayItem::User(text) | ReplayItem::Assistant(text) => text.len(),
        ReplayItem::AgentResult { label, body, .. } => label.len() + body.len(),
        ReplayItem::ToolCall {
            name,
            input,
            output,
            ..
        } => name.len() + input.len() + output.as_ref().map_or(0, String::len),
    }
}

impl Ui {
    fn width(&self) -> usize {
        self.painter.cols() as usize
    }

    /// 한 프레임 — 커밋 큐를 한 번 흘리고, 높이를 맞추고 뷰포트를 그린다.
    /// 이 사이에 쌓인 히스토리 삽입도 같은 프레임에 함께 나간다.
    fn draw(&mut self) {
        self.draw_with_queue(|| 0);
    }

    fn draw_with_queue(&mut self, pending_blocks: impl FnOnce() -> usize) {
        let sample = super::paint_probe::start(
            self.paint_probe.is_some(),
            || pending_blocks() + self.pending_history.len() + match &self.segment {
                Segment::Text(_, stream) | Segment::Reasoning(_, stream) => stream.queued_lines(),
                Segment::None => 0,
            },
            Instant::now,
        );
        self.reconcile_size();
        self.commit_stream(Instant::now());
        self.paint();
        super::paint_probe::frame(self.paint_probe.as_mut(), sample);
    }

    /// The once-a-second look at the terminal ([`SIZE_POLL`]): its size, and
    /// whose it is. A foreground group that is not zo's means no key ever
    /// arrives — see [`tty`]. `true` when the screen was resized; the caller
    /// draws.
    fn tend_terminal(&mut self) -> bool {
        tty::reclaim_foreground();
        self.reconcile_size()
    }

    /// The pty's size, asked for directly, against the size the screen was
    /// laid out for — codex's own rule. Its `custom_terminal.rs::draw` reads
    /// `self.size()` from the backend on **every** frame and `autoresize`
    /// resizes when it differs from `last_known_screen_size`; and its
    /// `TuiEvent::Resume` doc says why an event-only path is not enough:
    /// "resize events are not delivered while the process is suspended"
    /// (rust-v0.150.1). A host that resizes the pty without a `SIGWINCH`
    /// reaching this process is the same hole, and the [`Event::Resize`] path
    /// never runs for it. Measured in a `ZeroCode` pane (2026-08-30): the pane
    /// was 44 rows while the viewport sat parked at row 33 with blank rows
    /// under it, and an answer's table body had vanished — rows laid out for
    /// a taller terminal than the pane had become were clamped onto its last
    /// row and overwrote each other. In the hermetic harness a delivered
    /// `SIGWINCH` repaints correctly, so this is the path for the signal that
    /// never came. Called from [`Self::draw`] like codex, plus once a second
    /// while idle ([`SIZE_POLL`]) — the one extension over codex, whose idle
    /// screen waits for the next event to notice. `true` when the screen was
    /// resized; the caller draws.
    fn reconcile_size(&mut self) -> bool {
        let Ok((cols, rows)) = crossterm::terminal::size() else {
            return false;
        };
        // `Painter::resize` clamps; compare against what it would keep, or a
        // pty below the minimum would resize (and repaint) every frame.
        let cols = cols.max(1);
        let rows = rows.max(MIN_ROWS);
        if cols == self.painter.cols() && rows == self.painter.rows() {
            return false;
        }
        self.resize(cols, rows);
        true
    }

    /// 지금 화면에 **스스로 움직이는 것**이 있는가 — 그렇다면 사람이 아무것도
    /// 하지 않아도 프레임을 계속 내야 한다.
    ///
    /// 움직이는 것은 셋뿐이다.
    ///
    /// - 상태 줄([`Status::line`])과, 그 줄의 `elapsed` 를 함께 받는 활성 도구
    ///   셀([`Self::active_cell`]). shimmer 위상도 `(Ns)` 카운터도 경과 시간의
    ///   함수라 프레임마다 달라진다.
    /// - 확정 행을 스크롤백으로 흘려보내는 커밋 애니메이션
    ///   ([`Self::commit_stream`]) — 아직 흘릴 것이 남아 있을 때.
    /// - Max·Ultra·Smart를 새로 고른 뒤 2.5초 안에 끝나는 컴포저/푸터 전환.
    ///
    /// 그 밖의 화면(컴포저·피커·팝업·다이얼로그·히스토리)은 **사건이 있어야**
    /// 달라진다. 그래서 유휴에서 [`FRAME_TICK`] 틱이 하는 일은 같은 바이트를
    /// 초당 서른한 번 다시 계산하는 것뿐이다 — [`App::drive`] 가 이 답으로 그
    /// 틱을 끈다. 뷰포트에 스스로 움직이는 것을 새로 들이면 여기도 함께
    /// 넓혀야 한다.
    fn animating(&self) -> bool {
        if self.status.is_some() {
            return true;
        }
        if self
            .effort_effect
            .as_ref()
            .is_some_and(|effect| !effect.is_finished_at(Instant::now()))
        {
            return true;
        }
        match &self.segment {
            Segment::None => false,
            Segment::Text(_, stream) | Segment::Reasoning(_, stream) => stream.is_draining(),
        }
    }

    /// 커밋 애니메이션 — 확정된 행을 스크롤백으로 흘려보낸다.
    ///
    /// codex 는 이것을 `COMMIT_ANIMATION_TICK`(8.33ms) 마다 한 번 돌리고
    /// (`app.rs`), 한 틱에 몇 행을 뽑을지는 [`super::chunking`] 의 정책이
    /// 정한다 — Smooth 는 한 행, 백로그가 쌓이면 `CatchUp` 이 한 번에 비운다.
    /// 우리 프레임은 32ms 라 그 사이 밀린 틱을 여기서 따라잡는다.
    fn commit_stream(&mut self, now: Instant) {
        let (Segment::Text(_, stream) | Segment::Reasoning(_, stream)) = &mut self.segment else {
            self.last_commit = None;
            return;
        };
        if !stream.is_draining() {
            self.last_commit = Some(now);
            return;
        }
        let out = run_due_commit_ticks(
            &mut self.last_commit,
            now,
            Instant::now,
            |decision_now| stream.tick(decision_now),
        );
        self.history(&out);
    }

    /// 스트리밍 큐에 배출 대기 중인 줄이 있는지 여부.
    /// 참일 때 8.33ms(120 FPS) 고빈도 커밋 애니메이션 티커가 작동한다.
    #[must_use]
    pub(crate) fn has_streaming_drain(&self) -> bool {
        match &self.segment {
            Segment::Text(_, stream) | Segment::Reasoning(_, stream) => stream.is_draining(),
            Segment::None => false,
        }
    }

    /// Feed a stable reasoning heading to the status header.
    ///
    /// The header remains `Working` until a closed `**heading**` arrives. A
    /// partial reasoning sentence is neither stable task status nor worth a
    /// repaint on every streaming delta.
    fn absorb_reasoning_heading(&mut self, id: u64, text: &str, done: bool) {
        if done {
            self.reasoning_scan = None;
            if let Some(status) = self.status.as_mut() {
                status.set_header(None);
            }
            return;
        }
        match &mut self.reasoning_scan {
            // Already took this block's closed heading — stop buffering it.
            Some((scan_id, None)) if *scan_id == id => return,
            Some((scan_id, Some(buffer))) if *scan_id == id => {
                if buffer.len() < 256 {
                    buffer.push_str(text);
                }
            }
            // A new block starts its own search, and its own heading.
            _ => self.reasoning_scan = Some((id, Some(text.to_string()))),
        }
        let Some((_, Some(buffer))) = &self.reasoning_scan else {
            return;
        };
        if let Some(heading) = first_bold_heading(buffer) {
            let heading = heading.to_string();
            if let Some((_, slot)) = self.reasoning_scan.as_mut() {
                *slot = None;
            }
            if let Some(status) = self.status.as_mut() {
                status.set_header(Some(&heading));
            }
        }
    }

    /// Take a permission change while a turn is running.
    ///
    /// The live half lands now and the rest is queued, and the note says which
    /// is which. Reporting "runs when this turn ends" alone was wrong once the
    /// shared cell existed — the boundary really does move immediately — and
    /// reporting a plain success would be worse, because the running turn keeps
    /// asking for approval under the old policy.
    fn take_permission_mode_mid_turn(&mut self, mode: PermissionMode) {
        let label = active_permission_label(mode);
        let narrowing = permission_rank(mode) < permission_rank(self.permission_mode);
        let live = self.apply_live_permission_mode(mode);
        self.deferred.push(Deferred::Permission(mode));
        let text = match (live, narrowing) {
            // Widening: the live half is the useful half — the boundary opens
            // now and the approval policy follows at the turn boundary.
            (true, false) => {
                format!("Permissions → {label} · file access now, approvals when this turn ends")
            }
            // Narrowing: the same sentence would read as "read-only is in
            // force", and it is not. The shared cell restores the workspace
            // boundary immediately, but this turn keeps the policy it started
            // with, so writes it already considers allowed keep going. Measured:
            // 12 in-workspace writes ran to completion after the switch while
            // only escapes were refused. Say what actually stops it.
            (true, true) => format!(
                "Permissions → {label} · workspace boundary now; this turn keeps its \
                 approvals until it ends — press esc to stop it"
            ),
            (false, _) => format!("Permissions → {label} when this turn ends"),
        };
        self.note(SystemLevel::Info, &text);
    }

    fn next_permission_mode(&self) -> PermissionMode {
        next_permission_mode(self.permission_mode)
    }

    /// Widen (or narrow) what tools may touch **right now**, for a turn that is
    /// already running.
    ///
    /// Only half of a permission switch can be live. The registry's shared cell
    /// governs the file-tool workspace boundary and every registry clone reads
    /// it, so writing it here takes effect on the next tool call. The approval
    /// policy lives inside the runtime the turn owns behind `&mut`, so it can
    /// only swap once the turn ends. Returns whether the live half landed, so
    /// the caller can tell the user which half they got instead of implying the
    /// whole switch did.
    fn apply_live_permission_mode(&mut self, mode: PermissionMode) -> bool {
        self.permission_mode = mode;
        write_permission_cell(self.permission_cell.as_deref(), mode)
    }

    /// Put (or clear) ordinary foreground-tool context under the status header.
    /// Drop a palette reply that arrived after its probe gave up.
    ///
    /// Returns `true` when the key was part of that reply and must not reach
    /// the composer or the key bindings. Disarms once the window closes, so
    /// afterwards this costs one `Option` check per key.
    fn swallow_late_osc_reply(&mut self, key: &KeyEvent) -> bool {
        let now = Instant::now();
        let Some(guard) = self.osc_guard.as_mut() else {
            return false;
        };
        let swallowed = guard.swallows(key, now);
        if now >= guard.until {
            self.osc_guard = None;
        }
        swallowed
    }

    fn set_status_detail(&mut self, detail: Option<String>) {
        self.tool_status_detail = detail;
        self.refresh_status_details();
    }

    /// Install the collector's latest in-memory snapshot. Agent rows take the
    /// bounded details area while any are live; the ordinary tool context is
    /// restored as soon as the snapshot becomes empty.
    fn set_subagent_progress(&mut self, progress: Vec<SubagentProgress>) {
        if subagent_event_arrived(&self.subagent_progress, &progress) {
            if let Some(status) = self.status.as_mut() {
                status.note_activity_event();
            }
        }
        self.subagent_wave.refresh(&progress);
        self.subagent_progress = progress;
        if let Some(overview) = self.agents.as_mut() {
            overview.refresh(self.subagent_progress.clone());
        }
        self.refresh_spawn_cell_progress();
        self.refresh_status_details();
    }

    /// Hand the poller's newest rows to the spawn cell in the viewport.
    ///
    /// The rows are rebuilt only here — once per poll (a second), not once per
    /// frame — and each is keyed by the `tool_use` id the helper stamped on its
    /// own manifest, which is the only key two concurrent helpers cannot
    /// share.
    fn refresh_spawn_cell_progress(&mut self) {
        if !self.tool_cell.as_ref().is_some_and(ToolGroup::is_spawning) {
            return;
        }
        let rows: Vec<(String, String)> = self
            .subagent_progress
            .iter()
            .filter_map(|row| {
                Some((row.tool_call_id.clone()?, subagent_cell_progress(row)))
            })
            .collect();
        if let Some(cell) = self.tool_cell.as_mut() {
            cell.set_spawn_progress(&rows);
        }
    }

    fn refresh_status_details(&mut self) {
        let mut details = Vec::new();
        if let Some(summary) = self.subagent_wave.summary() {
            details.push(summary);
            let spawn_cell_owns_rows = self
                .tool_cell
                .as_ref()
                .is_some_and(|cell| cell.is_spawning() && cell.is_active());
            if !spawn_cell_owns_rows {
                details.extend(self.subagent_progress.iter().map(subagent_status_detail));
            }
        } else {
            details.extend(self.tool_status_detail.iter().cloned());
        }
        if let Some(status) = self.status.as_mut() {
            status.set_details_max_lines(details.len().max(STATUS_DETAILS_DEFAULT_MAX_LINES));
            status.set_details(details, StatusDetailsCapitalization::Preserve);
        }
    }

    /// Put the Working line's fact on the events channel, once per change.
    ///
    /// This is the ONLY road for elapsed seconds, model waits, retries and
    /// quiet stretches: `session_status.activity` is a status card the window
    /// replaces in place, so saying it every second costs nothing and demotes
    /// nothing. The hook road is deliberately absent here. A `PreToolUse` is
    /// one real tool start ([`Ui::observe`]); repeating it every second — and
    /// spelling `quiet` or `waiting` as a tool — turned a pane parked on an
    /// approval back to "working" in the window, dropped its Allow/Deny
    /// panel, and filled its activity ring with a stopwatch (t-2550).
    fn publish_working_activity(&mut self, ide: Option<&events::EventsChannel>) {
        let card = self
            .status
            .as_ref()
            .and_then(Status::activity)
            .map(|activity| events::ActivityCard {
                verb: activity.verb(),
                target: activity.target(),
                phase: super::strings::ACTIVITY_PHASE_STARTED.to_string(),
                elapsed_secs: activity.elapsed().as_secs(),
            });
        if matches!(&self.published_activity, PublishedActivity::Value(last) if last == &card) {
            return;
        }
        if let Some(channel) = ide {
            channel.publish_activity(card.clone());
        }
        self.published_activity = PublishedActivity::Value(card);
    }

    /// 대화가 갈렸다 — 열려 있던 스트림·도구 셀·상태를 놓는다.
    ///
    /// `/resume` 과 `/new` 가 세션을 갈아 끼우는 자리에서 부른다. 스크롤백은
    /// 건드리지 않는다(append-only 계약); `/clear` 만 별도로 터미널을 비운다.
    fn reset_conversation(&mut self) {
        self.transcript = None;
        self.transcript_items.clear();
        self.transcript_bytes = 0;
        self.transcript_answer.clear();
        self.last_answer.clear();
        self.usage = TokenUsage::default();
        self.ctx_tokens = 0;
        self.usage_known = false;
        self.segment = Segment::None;
        self.pending_input.clear();
        self.tools.clear();
        self.unseated.clear();
        self.committed_tool_calls.clear();
        self.tool_cell = None;
        self.quiet_loop_cell = None;
        self.quiet_candidate = None;
        self.turn_agent_count = 0;
        self.status = None;
        self.tool_status_detail = None;
        self.subagent_progress.clear();
        self.subagent_wave = SubagentWave::default();
        self.published_activity = PublishedActivity::Never;
        self.parked = None;
        self.permissions = None;
        self.agents = None;
        self.last_commit = None;
        self.effort_effect = None;
    }

    /// 화면이 이미 아는 것만으로 세운 `/status` 재료.
    ///
    /// 턴이 도는 동안 세션은 턴 task 에 있어 물어볼 수 없다. 그런데 카드가 묻는
    /// 일곱 가지는 푸터와 배너가 이미 들고 있는 것들이라, 그때는 여기서 답한다.
    /// 다른 점은 컨텍스트 토큰 하나뿐이다: 세션은 런타임의 추정치를 주고 여기는
    /// 원장이 실어 준 마지막 누적 사용량으로 센다([`usage_context_tokens`]) —
    /// 도는 턴이 아직 보고하지 않은 몫만큼 뒤처지지만, 턴 중에 물었으니 그것이
    /// 정직한 답이다.
    fn status_facts(&self) -> crate::status_format::StatusFacts {
        crate::status_format::StatusFacts {
            model: self.model.clone(),
            effort: (!self.effort.is_empty()).then(|| self.effort.clone()),
            cwd: self.cwd.clone(),
            permissions: crate::permission_mode::mode_label(self.permission_mode),
            // A queued `Deferred::Permission` IS the pending half: the label
            // above already moved, and the approval policy has not.
            permissions_pending: self
                .deferred
                .iter()
                .any(|deferred| matches!(deferred, Deferred::Permission(_))),
            session_id: self.session_id.clone(),
            context_tokens: self.ctx_tokens,
            goal: self.goal.clone(),
            autonomous: self.autonomous.clone(),
            loops: self.loops.clone(),
        }
    }

    /// 뷰포트의 **활성 셀 칸** 하나. codex 도 칸은 하나다
    /// (`transcript.active_cell`) — 도는 도구 셀이 우선이고, 없으면 스트림의
    /// 미확정 꼬리가 앉는다. 끝났지만 아직 안 접힌 도구 셀도 이 칸에 남는다
    /// (`should_flush` 가 거짓인 동안).
    ///
    /// 두 번째 값은 "꼬리가 칸을 쥐었는가" — 그때 codex 는 Working 줄을
    /// 감춘다(`sync_active_stream_tail` → `hide_status_indicator`).
    fn active_cell(&self, width: usize) -> (Vec<Line>, bool) {
        let elapsed = self.status.as_ref().map_or(Duration::ZERO, |status| status.elapsed);
        if let Some(cell) = self.tool_cell.as_ref().filter(|cell| cell.is_active()) {
            return (cell.lines(width, elapsed), false);
        }
        let tail = match &self.segment {
            Segment::None => Vec::new(),
            Segment::Text(_, stream) | Segment::Reasoning(_, stream) => stream.tail(),
        };
        if !tail.is_empty() {
            return (tail, true);
        }
        match self.tool_cell.as_ref() {
            Some(cell) => (cell.lines(width, elapsed), false),
            None => (
                self.quiet_loop_cell
                    .as_ref()
                    .map_or_else(Vec::new, |cell| cell.lines.clone()),
                false,
            ),
        }
    }

    fn paint(&mut self) {
        let now = Instant::now();
        // 페이저는 뷰포트를 통째로 쓴다. 렌더가 창 높이를 정하므로 여기서
        // 미리 접어 두고(`&mut`), 프레임에는 그 줄들을 빌려준다 — 아래의
        // 불변 빌림들(`parked_dialog`, `overlay`)보다 **먼저**여야 한다.
        let pager_width = self.painter.cols() as usize;
        let pager_rows = self.painter.max_height() as usize;
        let pager = self
            .transcript
            .as_mut()
            .map(|view| view.lines(pager_width, pager_rows));
        let shortcuts = (self.composer.text().trim() == "?").then(view::shortcut_card);
        let dialog = self.parked_dialog();
        let question = self
            .parked_question()
            .or_else(|| self.agents.as_ref().and_then(agents::Overview::question));
        let composer = self
            .agents
            .as_ref()
            .and_then(agents::Overview::composer)
            .unwrap_or(&self.composer);
        let popup = self.popup();
        let (active, tail_owns_slot) = self.active_cell(self.painter.cols() as usize);
        let pending_input = self.pending_input.lines(self.painter.cols() as usize);
        let display_effort = fast::display_effort(&self.effort, self.fast);
        let context_used_tokens = self
            .usage_known
            .then_some(self.ctx_tokens);
        let context_left = context_used_tokens.and_then(|used_tokens| {
            crate::status_format::context_left_percent(
                used_tokens,
                api::context_window_for_model(&self.model),
            )
        });
        let dream = crate::dream::last_pass_line();
        // A surface up for a while and then gone — a picker, a dialog, a
        // question, the composer's own suggestion list — is a popup: drawn
        // over the rows above the head, and given back when it closes, so
        // the head is where it was and nothing is left blank. Its height
        // comes from what the painter can give back, not from the screen
        // alone. Growth that becomes history (a streaming cell, a typed
        // paragraph) is not a popup: it scrolls, and the history it turns
        // into pushes the head back down.
        let whole = dialog.is_some()
            || self.overlay().is_some()
            || self.sessions.is_some()
            || pager.is_some()
            || question.is_some()
            || popup.is_some();
        let max_rows = if whole {
            self.painter.popup_budget()
        } else {
            self.painter.max_height()
        };
        let (footer_model, model_note) = self.footer_model();
        let frame = Frame {
            composer,
            effort_tier: self.effort_tier,
            effort_effect: self.effort_effect.as_ref(),
            now,
            status: self.status.as_ref(),
            pending_input: (!pending_input.is_empty()).then_some(pending_input.as_slice()),
            active: (!active.is_empty()).then_some(active.as_slice()),
            tail_owns_slot,
            dialog,
            question,
            picker: self.overlay(),
            sessions: self.sessions.as_ref(),
            pager: pager.as_deref(),
            popup: popup.as_ref(),
            shortcuts: shortcuts.as_deref(),
            model: footer_model,
            effort: &display_effort,
            model_note,
            cwd: &self.footer_location,
            context_left,
            context_used_tokens,
            plan_mode: self.permission_mode == PermissionMode::ReadOnly,
            goal_status: goal::footer_status_from_snapshot(&self.goal, &self.autonomous),
            loop_status: (self.loops != "none").then_some(self.loops.as_str()),
            dream: dream.as_deref(),
            width: self.painter.cols() as usize,
            max_rows: max_rows as usize,
        };
        let (rows, cursor) = view::build(&frame);
        let height = u16::try_from(rows.len()).unwrap_or(u16::MAX);
        if whole {
            // History goes under the popup first; then the popup goes back up
            // over the rows that are there now.
            self.flush_history();
            self.painter.set_popup_height(height);
        } else {
            // The head is sized first, so a finished cell's lines push it back
            // down onto the floor — see `history`. History under a head that
            // covers the screen folds it to one row on the floor first, so the
            // size is asked for again: the second call is a no-op otherwise,
            // and after a fold it grows the head back from the floor, so the
            // rows painted and the cursor placed below agree with `height`.
            self.painter.set_height(height);
            self.flush_history();
            self.painter.set_height(height);
            // Nobody working: the rows a mid-turn shrink left under the
            // footer go back above it, and the composer rests on the floor.
            if self.status.is_none() {
                self.painter.settle_on_the_floor();
            }
        }
        self.painter.paint(&rows);
        let cursor = cursor.map(|(col, row)| self.painter.absolute(col, row));
        self.painter.end(cursor);
    }

    /// History is held until the frame that follows it has sized the head.
    ///
    /// A finished cell hands over its lines and, in the same frame, the head
    /// shrinks by the rows the cell took. Inserted first, the lines scrolled
    /// in above a head that then shrank in place and left blank rows under
    /// itself — the composer floating mid-screen until enough later history
    /// pushed it down ("자꾸 빈공백이 발생해", 2026-09-02). Inserted after
    /// the shrink, the same lines push the head straight back onto the floor.
    fn history(&mut self, lines: &[Line]) {
        if lines.is_empty() {
            return;
        }
        if let Some(candidate) = self.quiet_candidate.as_mut() {
            candidate.extend_from_slice(lines);
            return;
        }
        self.commit_quiet_loop_cell();
        self.pending_history.extend_from_slice(lines);
    }

    fn begin_quiet_candidate(&mut self) {
        debug_assert!(self.quiet_candidate.is_none());
        self.quiet_candidate = Some(Vec::new());
    }

    fn finish_quiet_candidate(
        &mut self,
        loop_id: &str,
        noop: bool,
        quiet_streak: u32,
        now_unix_ms: u64,
    ) {
        let buffered = self.quiet_candidate.take().unwrap_or_default();
        if !noop {
            self.commit_quiet_loop_cell();
            self.pending_history.extend(buffered);
            return;
        }
        if !self.may_take_the_live_slot(LiveSlotRequest::Quiet(loop_id)) {
            self.commit_quiet_loop_cell();
        }
        let clock = crate::status_format::format_reset(
            now_unix_ms / 1_000,
            now_unix_ms / 1_000,
        )
        .replacen("resets ", "", 1);
        self.quiet_loop_cell = Some(QuietLoopCell {
            loop_id: loop_id.to_string(),
            lines: cells::quiet_loop_cell(loop_id, quiet_streak, &clock, self.width()),
        });
    }

    fn commit_quiet_loop_cell(&mut self) {
        let Some(cell) = self.quiet_loop_cell.take() else {
            return;
        };
        self.pending_history.push(Line::empty());
        self.pending_history.extend(cell.lines);
    }

    /// The text to show of a delta, with a leading `[earlier reasoning]` label
    /// held back and dropped.
    ///
    /// A model that has just read carried thought sometimes writes its own
    /// reply in that shape (gpt-5.6 after a handoff, 2026-09-02); the wire
    /// now tells it not to, and this is the display boundary's belt for the
    /// times it does anyway. The first bytes of a cell are held until they
    /// are longer than the label or stop matching it — at most nineteen
    /// characters, a fraction of a frame — so nothing is drawn twice.
    fn absorb_passport_label(&mut self, delta: &str, done: bool) -> String {
        let label = runtime::REASONING_PASSPORT_LABEL;
        let held = match &mut self.passport_gate {
            PassportGate::Passed => {
                if done {
                    self.passport_gate = PassportGate::default();
                }
                return delta.to_string();
            }
            PassportGate::Holding(held) => {
                held.push_str(delta);
                let undecided =
                    !done && held.len() < label.len() && label.starts_with(held.as_str());
                if undecided {
                    return String::new();
                }
                std::mem::take(held)
            }
        };
        self.passport_gate = if done {
            PassportGate::default()
        } else {
            PassportGate::Passed
        };
        runtime::strip_reasoning_passport_label(&held).to_string()
    }

    fn flush_history(&mut self) {
        if self.pending_history.is_empty() {
            return;
        }
        let lines = std::mem::take(&mut self.pending_history);
        self.painter.insert_history(&lines);
    }

    /// 화면이 바뀌었다 — 아직 안 나간 스트림 부분을 새 폭으로 다시 접는다
    /// (codex `StreamCore::set_width`). 이미 스크롤백에 내려간 행은 그대로다.
    fn resize(&mut self, cols: u16, rows: u16) {
        self.painter.resize(cols, rows);
        let width = self.width();
        if let Segment::Text(_, stream) | Segment::Reasoning(_, stream) = &mut self.segment {
            stream.set_width(width);
        }
    }

    fn note(&mut self, level: SystemLevel, text: &str) {
        let width = self.width();
        let cell = cells::system_cell(level, text, width);
        self.history(&cell);
    }

    fn user_cell(&mut self, text: &str) {
        let width = self.width();
        self.record_transcript(ReplayItem::User(text.to_string()));
        let cell = cells::user_cell(text, width);
        self.history(&cell);
    }

    /// Codex's Ctrl/Alt-V path: clipboards can offer a Finder file list or
    /// Chrome-style image data, so capture both through `clipboard_paste`.
    fn paste_image(&mut self) {
        match paste_image_to_temp_png() {
            Ok((path, _info)) => self.composer.attach_image(path),
            Err(error) => {
                let text = format!("Failed to paste image: {error}");
                self.note(SystemLevel::Error, &text);
            }
        }
    }

    /// 파킹된 권한 다이얼로그.
    fn parked_dialog(&self) -> Option<&Dialog> {
        match self.parked.as_ref()?.screen {
            ParkedScreen::Dialog(ref dialog) => Some(dialog),
            ParkedScreen::Question(_) => None,
        }
    }

    /// 파킹된 질문 오버레이.
    fn parked_question(&self) -> Option<&view::Question> {
        match self.parked.as_ref()?.screen {
            ParkedScreen::Question(ref question) => Some(question),
            ParkedScreen::Dialog(_) => None,
        }
    }

    /// 뷰포트를 통째로 가져간 **번호** 피커 화면. `/resume` 은 자기 크롬을
    /// 들고 따로 온다([`Ui::sessions`]) — 같은 자리를 쓰지만 문법이 다르다.
    fn overlay(&self) -> Option<&Picker> {
        self.picker
            .as_ref()
            .map(|picker| &picker.view)
            .or_else(|| self.permissions.as_ref().map(|picker| &picker.view))
            .or_else(|| self.agents.as_ref().and_then(agents::Overview::picker))
    }

    /// 지금 컴포저 원문에 뜰 자동완성 팝업. 원문이 진실이므로 매 프레임
    /// 다시 만든다 — 상태로 남는 것은 고른 줄뿐이다.
    /// 턴 중에도 뜬다 — codex 의 컴포저는 task 가 도는 동안에도 `/` 팝업을
    /// 띄우고, 명령마다 `available_during_task()` 로 돌릴지 안내할지 가른다
    /// (`chatwidget/slash_dispatch.rs::slash_command_blocked_by_active_task`).
    /// 물러나는 것은 뷰포트를 통째로 쓰는 화면(피커·다이얼로그)뿐이다.
    fn popup(&self) -> Option<Popup> {
        if self.overlay().is_some()
            || self.sessions.is_some()
            || self.parked.is_some()
            || self.agents.is_some()
        {
            return None;
        }
        let hits = slash::matches_for_model(self.composer.text(), fast::supported(&self.model));
        if hits.is_empty() {
            return None;
        }
        let selected = self.popup_selected.min(hits.len() - 1);
        Some(Popup {
            rows: hits
                .into_iter()
                .map(|command| PopupRow {
                    name: command.name().to_string(),
                    description: command.description().to_string(),
                })
                .collect(),
            selected,
        })
    }

    /// 팝업이 고른 이름으로 컴포저를 갈아 끼운다(Tab·Enter 공용).
    fn complete_from_popup(&mut self) -> bool {
        let Some(name) = self
            .popup()
            .and_then(|popup| popup.selection().map(str::to_string))
        else {
            return false;
        };
        if name == self.composer.text() {
            return false;
        }
        self.composer.clear();
        self.composer.insert_str(&name);
        true
    }

    // ------------------------------------------------------------------
    // 키
    // ------------------------------------------------------------------

    /// 유휴 상태의 한 사건.
    fn idle_event(&mut self, event: &Event) -> KeyOutcome {
        match event {
            Event::Resize(cols, rows) => {
                self.resize(*cols, *rows);
                KeyOutcome::Nothing
            }
            Event::Paste(text) => {
                if self
                    .agents
                    .as_mut()
                    .is_some_and(|overview| overview.paste(text))
                {
                    return KeyOutcome::Nothing;
                }
                // The ZeroCode window pastes an image as the PATH of a temp
                // file it just wrote, so a paste that decodes as an image
                // becomes an attachment rather than a line of path text.
                // Anything else — including a path to a non-image — is
                // ordinary text, exactly as before.
                if !self.composer.handle_paste_image_path(text) {
                    self.composer.insert_pasted(text);
                }
                KeyOutcome::Nothing
            }
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                // The stamp `PushNotification` reads: a key here is a person
                // at this keyboard, whatever the key does next.
                tools::KeyboardPresence::process().note_input();
                if self.swallow_late_osc_reply(key) {
                    return KeyOutcome::Nothing;
                }
                if self.transcript.is_some() {
                    self.transcript_key(*key);
                    return KeyOutcome::Nothing;
                }
                if self.agents.is_some() {
                    return self.agents_key(*key);
                }
                if self.picker.is_some() {
                    return self.picker_key(*key);
                }
                if self.sessions.is_some() {
                    return self.session_picker_key(*key);
                }
                if self.permissions.is_some() {
                    return self.permission_picker_key(*key);
                }
                if self.parked.is_some() {
                    self.parked_key(*key);
                    return KeyOutcome::Nothing;
                }
                self.idle_key(*key)
            }
            _ => KeyOutcome::Nothing,
        }
    }

    #[allow(clippy::too_many_lines)] // 평평한 키 match — 한 arm 씩.
    fn idle_key(&mut self, key: KeyEvent) -> KeyOutcome {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        if alt
            && key.code == KeyCode::Up
            && self.popup().is_none()
            && self.pending_input.edit_latest_queued(&mut self.composer)
        {
            return KeyOutcome::Nothing;
        }
        match key.code {
            _ if transcript::is_open_key(&key) => {
                self.open_transcript();
                KeyOutcome::Nothing
            }
            _ if agents::is_open_key(&key) => KeyOutcome::OpenAgents,
            KeyCode::Char(ch) if is_paste_image_key(&key, ch) => {
                self.paste_image();
                KeyOutcome::Nothing
            }
            KeyCode::Char('c') if control => {
                let now = Instant::now();
                if self.startup_blocks_quit(now) {
                    return KeyOutcome::Nothing;
                }
                if !self.composer.is_empty() {
                    self.composer.clear();
                    self.last_interrupt = None;
                    return KeyOutcome::Nothing;
                }
                if self
                    .last_interrupt
                    .is_some_and(|last| now.duration_since(last) <= DOUBLE_INTERRUPT_WINDOW)
                {
                    self.exit = Some(ExitReason::UserExit);
                    return KeyOutcome::Nothing;
                }
                self.last_interrupt = Some(now);
                self.note(SystemLevel::Info, QUIT_SHORTCUT_REMINDER);
                KeyOutcome::Nothing
            }
            KeyCode::Char('d') if control => {
                if !self.startup_blocks_quit(Instant::now()) && self.composer.is_empty() {
                    self.exit = Some(ExitReason::UserExit);
                }
                KeyOutcome::Nothing
            }
            KeyCode::Char('u') if control => {
                self.composer.kill_to_start();
                KeyOutcome::Nothing
            }
            KeyCode::Char('k') if control => {
                self.composer.kill_to_end();
                KeyOutcome::Nothing
            }
            KeyCode::Char('w') if control => {
                self.composer.kill_word_left();
                KeyOutcome::Nothing
            }
            KeyCode::Char('a') if control => {
                self.composer.home();
                KeyOutcome::Nothing
            }
            KeyCode::Char('e') if control => {
                self.composer.end();
                KeyOutcome::Nothing
            }
            KeyCode::Char(ch) => {
                self.last_interrupt = None;
                self.popup_selected = 0;
                self.composer.insert_char(ch);
                KeyOutcome::Nothing
            }
            KeyCode::Tab => {
                let _ = self.complete_from_popup();
                KeyOutcome::Nothing
            }
            // codex binds the permission cycle to Shift+Tab and advertises it
            // in the footer as `(shift+tab to cycle)`. crossterm reports it as
            // `BackTab`, with the SHIFT modifier folded into the code.
            KeyCode::BackTab => KeyOutcome::Permission(self.next_permission_mode()),
            KeyCode::Enter => {
                self.last_interrupt = None;
                // 팝업이 떠 있으면 고른 항목이 제출된다 — codex 도 `/mo` 에
                // Enter 를 치면 `/model` 이 돈다.
                let _ = self.complete_from_popup();
                self.popup_selected = 0;
                KeyOutcome::Submitted(self.composer.submit())
            }
            KeyCode::Backspace => {
                self.popup_selected = 0;
                self.composer.backspace();
                KeyOutcome::Nothing
            }
            KeyCode::Delete => {
                self.popup_selected = 0;
                self.composer.delete();
                KeyOutcome::Nothing
            }
            KeyCode::Left if control => {
                self.composer.word_left();
                KeyOutcome::Nothing
            }
            KeyCode::Right if control => {
                self.composer.word_right();
                KeyOutcome::Nothing
            }
            KeyCode::Left => {
                self.composer.left();
                KeyOutcome::Nothing
            }
            KeyCode::Right => {
                self.composer.right();
                KeyOutcome::Nothing
            }
            KeyCode::Home => {
                self.composer.home();
                KeyOutcome::Nothing
            }
            KeyCode::End => {
                self.composer.end();
                KeyOutcome::Nothing
            }
            KeyCode::Up => {
                if self.popup().is_some() {
                    self.popup_selected = self.popup_selected.saturating_sub(1);
                    return KeyOutcome::Nothing;
                }
                self.composer.history_prev();
                KeyOutcome::Nothing
            }
            KeyCode::Down => {
                if let Some(popup) = self.popup() {
                    self.popup_selected =
                        (self.popup_selected + 1).min(popup.rows.len().saturating_sub(1));
                    return KeyOutcome::Nothing;
                }
                self.composer.history_next();
                KeyOutcome::Nothing
            }
            KeyCode::Esc => {
                self.popup_selected = 0;
                self.composer.clear();
                KeyOutcome::Nothing
            }
            _ => KeyOutcome::Nothing,
        }
    }

    /// Whether a quit-shaped key belongs to the launch handoff rather than a
    /// human cadence. An ignored Ctrl-C never arms the later double press.
    fn startup_blocks_quit(&mut self, now: Instant) -> bool {
        let blocked = self
            .startup_quit_grace_until
            .is_some_and(|until| now < until);
        if blocked {
            self.last_interrupt = None;
        } else {
            self.startup_quit_grace_until = None;
        }
        blocked
    }

    // ------------------------------------------------------------------
    // Ctrl+T transcript pager

    /// 트랜스크립트 저장소가 쥘 수 있는 본문 바이트.
    ///
    /// 도구 출력은 UI 에 닿기 전에 이미 잘려 있다(`TruncationConfig`: bash
    /// 16 KiB, 나머지 30 KiB). 잘려 있어도 **호출 수만큼** 쌓이는 것은 그대로라,
    /// 장시간 자율 실행이면 이 목록이 유일하게 무한히 자라는 것이 된다
    /// (도구 1,000회 × 평균 5 KiB ≈ 5 MiB, 최악 30 MiB, 그리고 멈추지 않는다).
    /// 세션 자신의 메시지는 압축이 묶어 주고, 페인터 캐시는 화면 크기가
    /// 묶어 준다 — 여기만 묶는 것이 없었다.
    ///
    /// 4 MiB 는 최근 도구 출력 수백 개를 담는다. 넘치면 **오래된 본문부터**
    /// 놓는데, 사람이 이 화면을 여는 건 거의 언제나 방금 일어난 일을 보려는
    /// 것이기 때문이다. 그리고 놓은 자리는 지우지 않고 한 줄로 남긴다 —
    /// 목록에서 통째로 사라진 호출은 본문이 없는 호출보다 나쁘다.
    const TRANSCRIPT_BYTE_BUDGET: usize = 4 * 1024 * 1024;

    fn record_transcript(&mut self, item: ReplayItem) {
        record_transcript_item(
            &mut self.transcript_items,
            &mut self.transcript_bytes,
            item,
            Self::TRANSCRIPT_BYTE_BUDGET,
        );
    }

    fn record_reasoning_transcript(&mut self, reasoning: String) {
        record_transcript_entry(
            &mut self.transcript_items,
            &mut self.transcript_bytes,
            transcript::Entry::Reasoning(reasoning),
            Self::TRANSCRIPT_BYTE_BUDGET,
        );
    }

    /// Open the transcript on the session's replay items.
    ///
    /// Built fresh at the current width every time rather than kept in sync:
    /// a session that ran for hours is still only its own turns, and a stale
    /// overlay that missed the last twenty minutes is worse than a slow one.
    fn open_transcript(&mut self) {
        let items = self.transcript_items.clone();
        self.transcript = Some(transcript::Transcript::from_entries(&items, self.width()));
    }

    fn transcript_key(&mut self, key: KeyEvent) {
        let Some(view) = self.transcript.as_mut() else {
            return;
        };
        if view.key(key) == transcript::Outcome::Close {
            self.transcript = None;
        }
    }

    // Alt+A agents overview
    // ------------------------------------------------------------------

    fn open_agents(&mut self, progress: Vec<SubagentProgress>) {
        if self.picker.is_some()
            || self.sessions.is_some()
            || self.permissions.is_some()
            || self.parked.is_some()
        {
            return;
        }
        self.subagent_progress.clone_from(&progress);
        self.agents = Some(agents::Overview::new(
            progress,
            std::sync::Arc::clone(&self.registry),
        ));
    }

    fn agents_key(&mut self, key: KeyEvent) -> KeyOutcome {
        let Some(overview) = self.agents.as_mut() else {
            return KeyOutcome::Nothing;
        };
        let effect = overview.key(key, &self.session_id, self.permission_mode);
        if let Some(agent_id) = effect.remove_agent {
            self.subagent_progress
                .retain(|agent| agent.agent_id != agent_id);
        }
        if effect.close {
            self.agents = None;
        }
        if let Some((level, text)) = effect.notice {
            let level = match level {
                agents::NoticeLevel::Info => SystemLevel::Info,
                agents::NoticeLevel::Warn => SystemLevel::Warn,
                agents::NoticeLevel::Error => SystemLevel::Error,
            };
            self.note(level, &text);
        }
        KeyOutcome::Nothing
    }

    // ------------------------------------------------------------------
    // `/model` 피커
    // ------------------------------------------------------------------

    /// 피커를 연다. 자격증명 있는 provider 가 하나도 없으면 열지 않고 안내만
    /// 남긴다 — 빈 목록을 띄우면 esc 밖에 할 일이 없다.
    fn open_model_picker(&mut self) {
        // A connection: what discovery learned since the last publish goes
        // live for this list, and the sources that have gone quiet for the
        // TTL are asked again for the next one (t-3054).
        crate::runtime_support::model_picker_opening();
        let models = models::choices();
        if models.is_empty() {
            self.note(
                SystemLevel::Info,
                "no model provider is connected — /model <alias> still switches directly",
            );
            return;
        }
        let view = Self::model_view(&models, &self.model);
        self.picker = Some(ModelPicker {
            models,
            view,
            stage: Stage::Model,
            chosen: None,
        });
    }

    /// 고른 모델·effort 를 **화면에** 즉시 세운다 — codex
    /// `chatwidget/settings.rs::set_model` 이 턴 여부를 보지 않고
    /// `refresh_model_dependent_surfaces()`(푸터 + 세션 헤더)를 부르는 그
    /// 자리다. 아무 문구도 찍지 않는다: 원본의 `model_popups.rs` 액션은
    /// `UpdateModel`/`UpdateReasoningEffort` 만 보내고 히스토리 셀은
    /// 경고(`ultra_reasoning_concurrency_warning`)일 때만 넣는다.
    ///
    /// 엔진은 여기서 안 바뀐다. 진행 중인 요청은 이미 자기 파라미터로 떠났고
    /// codex 도 in-flight 요청은 못 바꾼다 — 화면은 지금, 엔진은 턴 경계다.
    fn show_choice(&mut self, model: &str, effort: Effort) {
        let previous_effort = Effort::from_token(&self.effort);
        let previous_footer = self.passive_footer_line();
        self.sync_model_display(model);
        self.effort = effort.canonical().to_string();
        let next_tier = EffortTier::from_effort(effort);
        if previous_effort != Some(effort) {
            self.effort_effect = if crate::render::no_color_env() {
                None
            } else {
                next_tier.map(|tier| EffortEffect::new(tier, previous_footer))
            };
        }
        self.effort_tier = next_tier;
    }

    /// 전환 직전의 실제 푸터 한 줄. 이 값을 잡아 둬야 Codex처럼 옛 모델/effort가
    /// 오른쪽으로 밀려난 뒤 새 줄이 돌아온다.
    /// The footer's model field and its note: the model on the wire with the
    /// footer's word for the swap while one stands, else the session model.
    fn footer_model(&self) -> (&str, Option<&str>) {
        match &self.wire {
            Some(badge) => (badge.model.as_str(), Some(badge.label)),
            None => (self.model.as_str(), None),
        }
    }

    fn passive_footer_line(&self) -> Line {
        let display_effort = fast::display_effort(&self.effort, self.fast);
        let context_used_tokens = self
            .usage_known
            .then_some(self.ctx_tokens);
        let context_left = context_used_tokens.and_then(|used_tokens| {
            crate::status_format::context_left_percent(
                used_tokens,
                api::context_window_for_model(&self.model),
            )
        });
        let hint = (self.composer.is_empty() && self.composer.text().trim() != "?")
            .then_some(view::SHORTCUT_HINT);
        let (model, model_note) = self.footer_model();
        view::footer(
            model,
            &display_effort,
            model_note,
            &self.footer_location,
            self.width(),
            hint,
            context_left,
            context_used_tokens,
            self.permission_mode == PermissionMode::ReadOnly,
            goal::footer_status_from_snapshot(&self.goal, &self.autonomous),
            (self.loops != "none").then_some(self.loops.as_str()),
        )
    }

    /// Provider model id를 표시 이름과 transient fast 토큰으로 나눈다.
    fn sync_model_display(&mut self, model: &str) {
        let (model, fast) = fast::display(model);
        self.model = model;
        self.fast = fast;
        // A pick (or a session resync) is the setting again; a swap that is
        // still on the wire announces itself anew with the next request.
        self.wire = None;
    }

    /// 현재 화면 상태에서 fast를 토글하고, 실제 session 변경은 App이 턴
    /// 경계에서 수행할 수 있도록 원하는 상태만 돌려준다.
    fn toggle_fast_display(&mut self) -> Option<bool> {
        if !fast::supported(&self.model) {
            return None;
        }
        self.fast = !self.fast;
        Some(self.fast)
    }

    /// 1단계 화면. `(current)` 는 지금 모델, `(default)` 는 zo 가 인자 없이
    /// 띄우는 모델이다 — 캡처의 codex 피커가 쓰는 그 두 표시다.
    fn model_view(models: &[ModelChoice], current: &str) -> Picker {
        let default = crate::cli_args::resolve_model_alias(crate::DEFAULT_MODEL);
        let mut selected = 0;
        let rows = models
            .iter()
            .enumerate()
            .map(|(index, model)| {
                let mut label = model.id.clone();
                if model.id.eq_ignore_ascii_case(current) {
                    label.push_str(" (current)");
                    selected = index;
                } else if model.id.eq_ignore_ascii_case(&default) {
                    label.push_str(" (default)");
                }
                let unlisted = model.unlisted_since.map(models::local_clock);
                PickerRow {
                    label,
                    description: unlisted.as_deref().map_or_else(
                        || model.description.clone(),
                        super::strings::unlisted_since,
                    ),
                    dim: unlisted.is_some(),
                }
            })
            .collect();
        Picker {
            title: "Select Model and Effort".to_string(),
            title_style: Style::new().bold(),
            note: "Only providers with credentials are listed — /model <alias> switches directly."
                .to_string(),
            rows,
            selected,
            footer: "Press enter to confirm or esc to go back".to_string(),
        }
    }

    /// 2단계 화면 — forge 사다리 여덟 단을 순서 그대로.
    ///
    /// `off · low · medium · high · xhigh · max · ultra · smart`
    /// (`crates/zo-ide/src/effort.rs::Effort::ALL`). 지금 값에 `(current)`,
    /// 기본값(`plain_session::DEFAULT_EFFORT` = high)에 `(default)` — 모델
    /// 1단계와 같은 표기다.
    fn effort_view(model: &str, current: Option<Effort>) -> Picker {
        let selected = current
            .and_then(|effort| PICKER_EFFORTS.iter().position(|level| *level == effort))
            .unwrap_or_else(|| {
                PICKER_EFFORTS
                    .iter()
                    .position(|level| *level == crate::session::plain_session::DEFAULT_EFFORT)
                    .unwrap_or(0)
            });
        Picker {
            title: "Select Reasoning Effort".to_string(),
            title_style: Style::new().bold(),
            note: format!("{model} — this effort applies to every turn from here on."),
            rows: PICKER_EFFORTS
                .iter()
                .enumerate()
                .map(|(index, effort)| {
                    let mut label = effort.canonical().to_string();
                    if index == selected {
                        label.push_str(" (current)");
                    } else if *effort == crate::session::plain_session::DEFAULT_EFFORT {
                        label.push_str(" (default)");
                    }
                    PickerRow {
                        label,
                        description: effort_description(*effort).to_string(),
                        dim: false,
                    }
                })
                .collect(),
            selected,
            footer: "Press enter to confirm or esc to go back".to_string(),
        }
    }

    /// 피커가 떠 있는 동안의 키 — ↑↓ 와 숫자 둘 다, enter 확정 / esc 뒤로.
    fn picker_key(&mut self, key: KeyEvent) -> KeyOutcome {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        let mut confirm = false;
        if let Some(picker) = self.picker.as_mut() {
            match key.code {
                KeyCode::Up => picker.view.up(),
                KeyCode::Down => picker.view.down(),
                KeyCode::Char(ch) if ch.is_ascii_digit() => {
                    let digit = ch.to_digit(10).unwrap_or(0) as usize;
                    confirm = picker.view.select_number(digit);
                }
                KeyCode::Enter => confirm = true,
                _ => {}
            }
        }
        if confirm {
            return self.confirm_picker();
        }
        if key.code == KeyCode::Esc || (control && key.code == KeyCode::Char('c')) {
            self.retreat_picker();
        }
        KeyOutcome::Nothing
    }

    /// `/permissions` 피커의 키 — Codex 번호 피커와 같은 ↑↓·숫자·Enter·Esc
    /// 문법을 쓰되, 확정 결과는 기존 zo [`PermissionMode`]로 올린다.
    fn permission_picker_key(&mut self, key: KeyEvent) -> KeyOutcome {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        let mut confirm = false;
        if let Some(picker) = self.permissions.as_mut() {
            match key.code {
                KeyCode::Up => picker.view.up(),
                KeyCode::Down => picker.view.down(),
                KeyCode::Char(ch) if ch.is_ascii_digit() => {
                    let digit = ch.to_digit(10).unwrap_or(0) as usize;
                    confirm = picker.view.select_number(digit);
                }
                KeyCode::Enter => confirm = true,
                _ => {}
            }
        }
        if confirm {
            let mode = self
                .permissions
                .as_ref()
                .and_then(|picker| picker.modes.get(picker.view.selected))
                .copied();
            self.permissions = None;
            return mode.map_or(KeyOutcome::Nothing, KeyOutcome::Permission);
        }
        if key.code == KeyCode::Esc || (control && key.code == KeyCode::Char('c')) {
            self.permissions = None;
        }
        KeyOutcome::Nothing
    }

    /// Codex-style permissions screen. The rows and descriptions come from
    /// the v0.150.0 approval presets; only the final mode mapping is zo-owned.
    fn open_permissions_picker(&mut self) {
        if self.picker.is_some() || self.sessions.is_some() {
            return;
        }
        let choices = permissions::choices(self.permission_mode);
        let modes = choices.iter().map(|choice| choice.mode).collect();
        let selected = choices
            .iter()
            .position(|choice| choice.mode == self.permission_mode)
            .unwrap_or(0);
        let rows = choices
            .into_iter()
            .map(|choice| PickerRow {
                label: choice.label,
                description: choice.description.to_string(),
                dim: false,
            })
            .collect();
        self.permissions = Some(PermissionPicker {
            modes,
            view: Picker {
                title: permissions::TITLE.to_string(),
                title_style: Style::new().bold(),
                // Codex has no subtitle on this screen; retaining the empty
                // note keeps the title/row spacing byte-compatible.
                note: String::new(),
                rows,
                selected,
                footer: "Press enter to confirm or esc to go back".to_string(),
            },
        });
    }

    /// enter — 1단계면 effort 단계로 잇고, 2단계면 둘을 함께 확정한다.
    fn confirm_picker(&mut self) -> KeyOutcome {
        let current_effort = Effort::from_token(&self.effort);
        let Some(picker) = self.picker.as_mut() else {
            return KeyOutcome::Nothing;
        };
        match picker.stage {
            Stage::Model => {
                let Some(choice) = picker.models.get(picker.view.selected) else {
                    return KeyOutcome::Nothing;
                };
                let id = choice.id.clone();
                picker.view = Self::effort_view(&id, current_effort);
                picker.stage = Stage::Effort;
                picker.chosen = Some(id);
                KeyOutcome::Nothing
            }
            Stage::Effort => {
                let effort = PICKER_EFFORTS
                    .get(picker.view.selected)
                    .copied()
                    .unwrap_or(Effort::High);
                let model = picker.chosen.clone();
                self.picker = None;
                model.map_or(KeyOutcome::Nothing, |model| {
                    KeyOutcome::Chosen(model, effort)
                })
            }
        }
    }

    // ------------------------------------------------------------------
    // `/resume` 피커
    // ------------------------------------------------------------------

    /// 최근 세션 피커를 연다. 목록이 비었거나 못 읽으면 열지 않고 알린다 —
    /// codex 도 빈 목록에는 `render_empty_state_line` 한 줄만 남긴다.
    fn open_resume_picker(&mut self) {
        if self.picker.is_some() {
            return;
        }
        let listed = match crate::resume::list_recent_sessions_limited(RESUME_LIST_LIMIT) {
            Ok(listed) => listed,
            Err(error) => {
                let text = format!("resume: could not read the session list: {error}");
                self.note(SystemLevel::Error, &text);
                return;
            }
        };
        // Never the session that is running: resuming it fails on its own
        // writer lease, and it sat at the top of the list as the newest row.
        let listed: Vec<_> = listed
            .into_iter()
            .filter(|session| session.id != self.session_id)
            .collect();
        let rows = sessions::rows(&listed);
        if rows.is_empty() {
            self.note(SystemLevel::Info, "no saved sessions in this workspace");
            return;
        }
        self.sessions = Some(sessions::SessionPicker::new(
            rows,
            Some(self.session_cwd.clone()),
            sessions::now_millis(),
        ));
    }

    /// `/resume` 한 키 — codex `resume_picker.rs` 의 키 표 그대로.
    ///
    /// 타이핑은 **검색으로** 간다(원본도 그렇다). 그래서 예전의 숫자 선택은
    /// 없다: `3` 은 세 번째 행이 아니라 검색어 `3` 이다. 고르는 것은 ↑/↓ 와
    /// enter 이고, `tab` 이 툴바 컨트롤을 옮기고 ←/→ 가 그 값을 바꾼다.
    /// `ctrl+o` 는 밀도, `esc` 는 검색어가 있으면 그것부터 지운다
    /// (`footer_hint_lines` 의 `esc_label`).
    fn session_picker_key(&mut self, key: KeyEvent) -> KeyOutcome {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        let mut confirm = false;
        let mut close = false;
        if let Some(picker) = self.sessions.as_mut() {
            match key.code {
                KeyCode::Char('o') if control => picker.toggle_density(),
                KeyCode::Char('c') if control => close = true,
                KeyCode::Up => picker.up(),
                KeyCode::Down => picker.down(),
                KeyCode::Tab => picker.focus_next(),
                KeyCode::Left | KeyCode::Right => picker.change_option(),
                KeyCode::Backspace => picker.backspace(),
                KeyCode::Enter => confirm = true,
                KeyCode::Esc => {
                    if picker.has_query() {
                        picker.clear_query();
                    } else {
                        close = true;
                    }
                }
                KeyCode::Char(ch) if !control => picker.push_char(ch),
                _ => {}
            }
        }
        if confirm {
            let chosen = self
                .sessions
                .as_ref()
                .and_then(sessions::SessionPicker::selected_id)
                .map(ToOwned::to_owned);
            if let Some(id) = chosen {
                self.sessions = None;
                return KeyOutcome::Resumed(id);
            }
        }
        if close {
            self.sessions = None;
        }
        KeyOutcome::Nothing
    }

    /// esc — 2단계면 모델 목록으로 돌아가고, 1단계면 닫는다("esc to go back").
    fn retreat_picker(&mut self) {
        let model = self.model.clone();
        let Some(picker) = self.picker.as_mut() else {
            return;
        };
        if picker.stage == Stage::Effort {
            picker.view = Self::model_view(&picker.models, &model);
            if let Some(chosen) = picker.chosen.as_ref() {
                if let Some(index) = picker
                    .models
                    .iter()
                    .position(|choice| choice.id.eq_ignore_ascii_case(chosen))
                {
                    picker.view.selected = index;
                }
            }
            picker.stage = Stage::Model;
            picker.chosen = None;
            return;
        }
        self.picker = None;
    }

    /// 프롬프트가 파킹된 동안의 키. 권한은 번호 다이얼로그, 질문은
    /// [`Self::question_key`] 의 오버레이가 받는다.
    fn parked_key(&mut self, key: KeyEvent) {
        if self.parked_question().is_some() {
            self.question_key(key);
            return;
        }
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        let mut confirm = false;
        if let Some(dialog) = self.parked.as_mut().and_then(|parked| match parked.screen {
            ParkedScreen::Dialog(ref mut dialog) => Some(dialog),
            ParkedScreen::Question(_) => None,
        }) {
            match key.code {
                KeyCode::Up => dialog.selected = dialog.selected.saturating_sub(1),
                KeyCode::Down => {
                    dialog.selected =
                        (dialog.selected + 1).min(dialog.options.len().saturating_sub(1));
                }
                KeyCode::Char(ch) if ch.is_ascii_digit() => {
                    let index = ch.to_digit(10).unwrap_or(0) as usize;
                    if index >= 1 && index <= dialog.options.len() {
                        dialog.selected = index - 1;
                        confirm = true;
                    }
                }
                KeyCode::Enter => confirm = true,
                _ => {}
            }
        }
        if confirm {
            self.answer_dialog();
            return;
        }
        if key.code == KeyCode::Esc || (control && key.code == KeyCode::Char('c')) {
            self.dismiss_parked();
        }
    }

    fn answer_dialog(&mut self) {
        let Some(parked) = self.parked.take() else {
            return;
        };
        events::retire_prompt(parked.prompt_id, ResolvedBy::Pane);
        let (selected, label) = match &parked.screen {
            ParkedScreen::Dialog(dialog) => (
                dialog.selected,
                dialog
                    .options
                    .get(dialog.selected)
                    .cloned()
                    .unwrap_or_default(),
            ),
            ParkedScreen::Question(_) => (0, String::new()),
        };
        match parked.prompt {
            PendingPrompt::Permission(prompt) => {
                let decision = prompt
                    .choices
                    .get(selected)
                    .map_or(PermissionDecision::Deny, |choice| choice.decision);
                let _ = prompt.responder.send(decision);
            }
            PendingPrompt::Question(prompt) => {
                let answer = prompt
                    .options
                    .get(selected)
                    .map_or_else(|| label.clone(), |option| option.label.clone());
                let _ = prompt.responder.send(vec![answer]);
            }
        }
        self.note(SystemLevel::Info, &format!("› {label}"));
    }

    /// 질문 오버레이의 키 — codex `request_user_input::handle_key_event` 의
    /// `Focus::Options` 갈래를 우리가 가진 만큼만 옮겼다. ↑↓ 는 목록을 돌고
    /// (`move_up_wrap`/`move_down_wrap`, `k`/`j` 도 같은 자리), 숫자는 그 줄을
    /// 고르고 바로 낸다(`option_index_for_digit` → `go_next_or_submit`). 마지막
    /// Other 행이나 옵션 포커스에서의 일반 문자 입력은 인라인 자유 입력으로
    /// 전환한다. space 는 codex 의 다중 선택 위젯이 쓰는 토글이다.
    fn question_key(&mut self, key: KeyEvent) {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        if self.handle_other_question_key(key) {
            return;
        }
        if key.code == KeyCode::Esc || (control && key.code == KeyCode::Char('c')) {
            self.dismiss_parked();
            return;
        }
        if self.parked_question().is_some_and(view::Question::is_freeform) {
            if key.code == KeyCode::Enter {
                let line = self.composer.submit().text;
                let answer = line.trim().to_string();
                if answer.is_empty() {
                    return;
                }
                self.answer_question(vec![answer]);
                return;
            }
            let _ = self.idle_key(key);
            return;
        }
        self.handle_option_question_key(key);
    }

    /// 활성 Other 컴포저가 키를 소비했는가.
    fn handle_other_question_key(&mut self, key: KeyEvent) -> bool {
        if !self
            .parked_question()
            .is_some_and(view::Question::is_other_input)
        {
            return false;
        }
        if key.code == KeyCode::Esc {
            if let Some(question) = self.parked.as_mut().and_then(|parked| match parked.screen {
                ParkedScreen::Question(ref mut question) => Some(question),
                ParkedScreen::Dialog(_) => None,
            }) {
                question.cancel_other_input();
            }
            self.composer.clear();
            return true;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.dismiss_parked();
            return true;
        }
        if key.code == KeyCode::Enter {
            let custom = self.composer.text().to_string();
            let answers = self
                .parked_question()
                .map(|question| question.answers_with_custom(&custom))
                .unwrap_or_default();
            if !answers.is_empty() {
                let _ = self.composer.submit();
                self.answer_question(answers);
            }
            return true;
        }
        let _ = self.idle_key(key);
        true
    }

    fn handle_option_question_key(&mut self, key: KeyEvent) {
        let mut confirm = false;
        let mut open_other = false;
        let mut type_into_other = false;
        let text_modifiers = KeyModifiers::CONTROL
            | KeyModifiers::ALT
            | KeyModifiers::SUPER
            | KeyModifiers::HYPER
            | KeyModifiers::META;
        let plain_character = matches!(key.code, KeyCode::Char(_))
            && !key.modifiers.intersects(text_modifiers);
        if let Some(question) = self.parked.as_mut().and_then(|parked| match parked.screen {
            ParkedScreen::Question(ref mut question) => Some(question),
            ParkedScreen::Dialog(_) => None,
        }) {
            let multi = question.is_multi();
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => question.up(),
                KeyCode::Down | KeyCode::Char('j') => question.down(),
                KeyCode::Char(' ') if multi => {
                    if question.is_other_selected() {
                        open_other = question.begin_other_input();
                    } else {
                        question.toggle();
                    }
                }
                KeyCode::Char(ch) if ch.is_ascii_digit() => {
                    let digit = ch.to_digit(10).unwrap_or(0) as usize;
                    if question.select_number(digit) {
                        if question.is_other_selected() {
                            open_other = question.begin_other_input();
                        } else if multi {
                            question.toggle();
                        } else {
                            confirm = true;
                        }
                    }
                }
                KeyCode::Enter => {
                    if question.is_other_selected() {
                        open_other = question.begin_other_input();
                    } else {
                        confirm = true;
                    }
                }
                KeyCode::Char(_) if plain_character => {
                    open_other = question.begin_other_input();
                    type_into_other = open_other;
                }
                _ => {}
            }
        }
        if type_into_other {
            let _ = self.idle_key(key);
            return;
        }
        if open_other {
            return;
        }
        if confirm {
            let answers = self
                .parked_question()
                .map(view::Question::answers)
                .unwrap_or_default();
            self.answer_question(answers);
        }
    }

    /// 오버레이가 고른 답으로 질문을 닫는다. codex 도 여기서 화면을 접고
    /// `RequestUserInputResultCell` 을 스크롤백에 남긴다.
    fn answer_question(&mut self, answers: Vec<String>) {
        if answers.is_empty() {
            return;
        }
        let Some(parked) = self.parked.take() else {
            return;
        };
        let Parked {
            prompt,
            screen,
            prompt_id,
        } = parked;
        match prompt {
            PendingPrompt::Question(prompt) => {
                events::retire_prompt(prompt_id, ResolvedBy::Pane);
                let width = self.width();
                let cell = cells::question_cell(&prompt.question, &answers, width);
                let _ = prompt.responder.send(answers);
                self.history(&cell);
            }
            // 질문 화면에서만 부르므로 닿지 않는다. 그래도 프롬프트를 잃으면
            // responder drop 이 곧 hard deny 라, 되돌려 놓는다.
            prompt @ PendingPrompt::Permission(_) => {
                self.parked = Some(Parked {
                    prompt,
                    screen,
                    prompt_id,
                });
            }
        }
    }

    fn dismiss_parked(&mut self) {
        let Some(parked) = self.parked.take() else {
            return;
        };
        events::retire_prompt(parked.prompt_id, ResolvedBy::Dismissed);
        // 끊긴 질문도 스크롤백에 흔적을 남긴다 — codex 의
        // `RequestUserInputResultCell { interrupted: true }` 자리다.
        let question = match &parked.prompt {
            PendingPrompt::Question(prompt) => Some(prompt.question.clone()),
            PendingPrompt::Permission(_) => None,
        };
        parked.prompt.dismiss();
        match question {
            Some(question) => {
                let width = self.width();
                let cell = cells::question_cell(&question, &[], width);
                self.history(&cell);
            }
            None => self.note(SystemLevel::Info, "dismissed"),
        }
    }

    fn park(&mut self, prompt: PendingPrompt) {
        let screen = match &prompt {
            PendingPrompt::Permission(permission) => ParkedScreen::Dialog(Dialog {
                title: "Allow".to_string(),
                subject: crate::util::ansi::sanitize_inline(&permission.tool_name),
                body: permission
                    .audit_hint
                    .clone()
                    .unwrap_or_else(|| permission.reasoning.clone()),
                options: permission
                    .choices
                    .iter()
                    .map(|choice| choice.label.clone())
                    .collect(),
                selected: 0,
                footer: "Press enter to continue".to_string(),
            }),
            // 세 갈래 모두 같은 오버레이다 — 보기가 있으면 번호 목록,
            // 다중이면 그 목록에 `[x]`, 없으면 컴포저가 답을 받는다.
            PendingPrompt::Question(question) => {
                ParkedScreen::Question(question::view(question))
            }
        };
        self.parked = Some(Parked {
            prompt,
            screen,
            prompt_id: None,
        });
    }

    // ------------------------------------------------------------------
    // 턴 중의 키
    // ------------------------------------------------------------------

    /// 턴 중의 한 사건. 다시 그려야 하면 참.
    ///
    /// 취소와 스티어는 화면이 아니라 배선이라 [`TurnScaffold`] 가 든다 — 여기서는
    /// 어떤 키가 그것을 부르는지만 정한다.
    fn turn_event(&mut self, event: &Event, turn: &TurnScaffold, exit_after: &mut bool) -> bool {
        match event {
            Event::Resize(cols, rows) => {
                self.resize(*cols, *rows);
                true
            }
            Event::Paste(text) => {
                if self
                    .agents
                    .as_mut()
                    .is_some_and(|overview| overview.paste(text))
                {
                    return true;
                }
                if !self.composer.handle_paste_image_path(text) {
                    self.composer.insert_pasted(text);
                }
                true
            }
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                tools::KeyboardPresence::process().note_input();
                self.turn_key(key, turn, exit_after)
            }
            _ => false,
        }
    }

    /// One key while a turn is running.
    ///
    /// Split out of [`Self::turn_event`] so the event dispatch stays readable:
    /// the other arms are two lines each and this one carries every mid-turn
    /// binding.
    fn turn_key(&mut self, key: &KeyEvent, turn: &TurnScaffold, exit_after: &mut bool) -> bool {
        // Before anything reads this key: a palette reply that arrived
        // late is not the operator, and its leading ESC would otherwise
        // read as an interrupt.
        if self.swallow_late_osc_reply(key) {
            return true;
        }
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        let interrupt = key.code == KeyCode::Esc || (control && key.code == KeyCode::Char('c'));
        if self.transcript.is_some() {
            self.transcript_key(*key);
            return true;
        }
        if self.agents.is_some() {
            self.agents_key(*key);
            return true;
        }
        if self.picker.is_some() {
            // 턴 중에도 피커는 돌고, 확정은 codex 처럼 **그 자리에서**
            // 화면에 반영된다([`Ui::show_choice`]). 세션(엔진)만 턴
            // 경계로 미룬다 — 그 요청은 이미 옛 파라미터로 떠났다.
            if let KeyOutcome::Chosen(model, effort) = self.picker_key(*key) {
                self.show_choice(&model, effort);
                self.deferred.push(Deferred::Choice(model, effort));
            }
            return true;
        }
        if self.permissions.is_some() {
            if let KeyOutcome::Permission(mode) = self.permission_picker_key(*key) {
                self.take_permission_mode_mid_turn(mode);
            }
            return true;
        }
        if self.sessions.is_some() {
            // codex `SlashCommand::Resume` 은 `available_during_task()`
            // 가 참이라 턴 중에도 피커가 돈다. 고른 세션으로 갈아 끼우는
            // 것만 턴 경계로 미룬다 — 도는 턴이 옛 세션에 쓰고 있다.
            if let KeyOutcome::Resumed(id) = self.session_picker_key(*key) {
                self.deferred.push(Deferred::Resume(id));
                self.note(SystemLevel::Info, "↳ /resume runs when this turn ends");
            }
            return true;
        }
        if self.parked.is_some() {
            self.parked_key(*key);
            return true;
        }
        if transcript::is_open_key(key) {
            self.open_transcript();
            return true;
        }
        if agents::is_open_key(key) {
            self.open_agents(self.subagent_progress.clone());
            return true;
        }
        if key.code == KeyCode::Up
            && key.modifiers.contains(KeyModifiers::ALT)
            && self.popup().is_none()
            && self.pending_input.edit_latest_queued(&mut self.composer)
        {
            return true;
        }
        if interrupt {
            if self.pending_input.has_pending_steers() {
                self.pending_input.interrupt_and_submit_steers();
                turn.cancel_turn();
                return true;
            }
            turn.cancel_turn();
            self.note(SystemLevel::Info, "interrupted");
            return true;
        }
        if let KeyCode::Char(ch) = key.code {
            if is_paste_image_key(key, ch) {
                self.paste_image();
                return true;
            }
        }
        // Shift+Tab cycles mid-turn too. That is the whole point of the
        // key for an unattended run: a turn that is already asking for
        // approvals is exactly when a person wants to widen the mode.
        if key.code == KeyCode::BackTab {
            self.take_permission_mode_mid_turn(self.next_permission_mode());
            return true;
        }
        if key.code == KeyCode::Tab && self.popup().is_none() {
            self.queue_composer_input();
            return true;
        }
        if key.code == KeyCode::Enter {
            self.submit_composer_during_turn(turn, exit_after);
            return true;
        }
        let _ = self.idle_key(*key);
        true
    }

    fn queue_composer_input(&mut self) {
        let submission = self.composer.submit();
        if !submission.text.trim().is_empty() || !submission.image_paths.is_empty() {
            self.pending_input.push_queued(submission);
        }
    }

    fn submit_composer_during_turn(
        &mut self,
        turn: &TurnScaffold,
        exit_after: &mut bool,
    ) {
        // 팝업이 떠 있으면 고른 항목이 제출된다 — 유휴와 같다.
        let _ = self.complete_from_popup();
        self.popup_selected = 0;
        let submission = self.composer.submit();
        let line = submission.text;
        let trimmed = line.trim().to_string();
        if trimmed.is_empty() {
            return;
        }
        if Slash::Exit.is_bare_line(&trimmed) {
            *exit_after = true;
            return;
        }
        // A pasted absolute path (`/Users/dev/…`) is prose for the model, not a
        // command — `slash::classify` tells the two apart; everything that is
        // not a command falls through to the queue as the line it was.
        if let slash::Line::Command(command) =
            slash::classify(&trimmed, |path| std::path::Path::new(path).exists())
        {
            self.user_cell(&trimmed);
            self.turn_slash(command);
            return;
        }
        // The runtime steering queue is text-only. Preserve a mid-turn image
        // as an ordinary queued follow-up instead of dropping its attachment.
        if !submission.image_paths.is_empty() {
            self.pending_input.push_queued(Submission {
                text: trimmed,
                image_paths: submission.image_paths,
            });
        } else if turn.steer(trimmed.clone()) {
            self.pending_input.push_steer(trimmed);
        } else {
            // `composer.submit()` emptied the box; give the input back when
            // this turn has no runtime steering queue.
            self.composer.insert_str(&line);
            self.note(
                SystemLevel::Warn,
                "this turn cannot take input — your line is back in the composer",
            );
        }
    }

    /// 턴이 도는 동안의 슬래시 한 줄.
    ///
    /// 가르는 기준은 codex `slash_command_blocked_by_active_task` 다 —
    /// `available_during_task()` 가 참인 명령(`/model`·`/permissions`·
    /// `/status`·`/exit`)은 턴 중에도 받고, 대화 상태를 바꾸는 `/new`·`/clear`·
    /// `/compact`·`/goal` mutation은
    /// `'/x' is disabled while a task is in progress.` 로 돌려보낸다.
    ///
    /// zo 의 한 가지 사정: 턴이 도는 동안 세션은 턴 task 가 가져가 있다. 그래서
    /// **화면만 만지는 것**(피커 열기·단축키 카드)은 그 자리에서 하고, 세션을
    /// 만져야 하는 것은 [`Deferred`] 로 담아 턴 직후에 돌린다 — 사용자에게는
    /// codex 와 같은 순간에 반응하고, 결과는 다음 턴부터 적용된다.
    fn turn_slash(&mut self, command: &str) {
        let (name, arg) = match command.split_once(char::is_whitespace) {
            Some((name, arg)) => (name, arg.trim()),
            None => (command, ""),
        };
        match Slash::from_word(name) {
            Some(Slash::Help) => {
                let width = self.width();
                let body = view::shortcut_card().join("\n");
                let cell = cells::verbatim_cell(&body, width);
                self.history(&cell);
            }
            Some(Slash::Model) if arg.is_empty() => self.open_model_picker(),
            // codex `SlashCommand::Resume::available_during_task()` 는 참이다 —
            // 턴 중에도 피커를 띄운다. 확정한 세션으로 갈아 끼우는 것만 턴
            // 경계로 미룬다(도는 턴이 옛 세션에 쓰고 있다).
            Some(Slash::Resume) if arg.is_empty() => self.open_resume_picker(),
            Some(Slash::Resume) => {
                self.deferred.push(Deferred::Resume(arg.to_string()));
                self.note(SystemLevel::Info, "↳ /resume runs when this turn ends");
            }
            // `/model <alias>` 도 화면은 즉시다 — 피커 확정과 같은 길이라야
            // 한다(codex 는 둘 다 `UpdateModel` 하나로 흐른다). 별칭은 세션을
            // 안 건드리고도 풀 수 있으므로 푸터·다음 카드가 바로 새 이름을
            // 읽는다. 문구는 없다: codex 가 안 찍는다.
            Some(Slash::Model) => {
                self.sync_model_display(arg);
                self.deferred.push(Deferred::Slash(command.to_string()));
            }
            Some(Slash::Fast) => {
                if let Some(enabled) = self.toggle_fast_display() {
                    self.deferred.push(Deferred::Fast(enabled));
                } else {
                    self.note(SystemLevel::Info, fast::UNSUPPORTED_COMMAND_MESSAGE);
                }
            }
            Some(Slash::Permissions) if arg.is_empty() => {
                self.open_permissions_picker();
            }
            // codex `available_during_task()` 가 참인 그대로, 턴 중에도 답한다 —
            // 카드가 묻는 것을 화면이 이미 알기 때문이다([`Ui::status_facts`]).
            // 긴 세션에서 "지금 얼마나 남았나" 는 턴이 도는 동안 가장 궁금하다.
            Some(Slash::Status) => {
                let width = self.width();
                let rows = crate::status_format::card_from_facts(&self.status_facts(), width);
                self.history(&rows);
            }
            Some(Slash::Goal) if arg.is_empty() || matches!(arg, "status" | "show") => {
                self.note(
                    SystemLevel::Info,
                    &format!("goal: {} · autonomous: {}", self.goal, self.autonomous),
                );
            }
            // 권한은 세션에 **쓰는** 명령이라 여전히 턴 경계로 미룬다.
            Some(Slash::Permissions) => {
                self.deferred.push(Deferred::Slash(command.to_string()));
                let text = format!("↳ /{name} runs when this turn ends");
                self.note(SystemLevel::Info, &text);
            }
            Some(Slash::New) => {
                self.note(
                    SystemLevel::Error,
                    "'/new' is disabled while a task is in progress.",
                );
            }
            Some(Slash::Clear) => {
                self.note(
                    SystemLevel::Error,
                    "'/clear' is disabled while a task is in progress.",
                );
            }
            Some(Slash::Compact) => {
                let text = "'/compact' is disabled while a task is in progress.";
                self.note(SystemLevel::Error, text);
            }
            Some(Slash::Goal) => {
                self.deferred.push(Deferred::Slash(command.to_string()));
                self.note(SystemLevel::Info, "↳ /goal runs when this turn ends");
            }
            Some(Slash::Loop) => {
                self.deferred.push(Deferred::Slash(command.to_string()));
                self.note(SystemLevel::Info, "↳ /loop runs when this turn ends");
            }
            // `/exit` 는 여기 오지 않는다 — 제출 자리에서 먼저 걸러 턴을 끝까지
            // 돌리고 나간다.
            Some(Slash::Exit) | None => {
                let text =
                    format!("/{name} is not in zo — use zerocode IDE for it (? for shortcuts)");
                self.note(SystemLevel::Info, &text);
            }
        }
    }

    // ------------------------------------------------------------------
    // 블록 → 셀
    // ------------------------------------------------------------------

    #[allow(clippy::too_many_lines)] // RenderBlock 갈래마다 한 arm.
    fn block(&mut self, block: RenderBlock) {
        let width = self.width();
        match block {
            RenderBlock::StreamPhase(phase) => {
                if let Some(status) = self.status.as_mut() {
                    status.note_stream_phase(phase);
                }
            }
            RenderBlock::TextDelta { id, text, done } => {
                self.note_stream_content();
                self.open_text(id.0);
                let text = self.absorb_passport_label(&text, done);
                self.transcript_answer.push_str(&text);
                if let Segment::Text(_, stream) = &mut self.segment {
                    stream.push(&text);
                }
                if done {
                    let answer = std::mem::take(&mut self.transcript_answer);
                    if !answer.trim().is_empty() {
                        // `--last-message` 가 쓰는 값. 산문 경로의
                        // `Renderer::last_assistant_message` 와 같은 뜻이라야
                        // 계약이 프런트엔드에 따라 달라지지 않는다.
                        self.last_answer.clone_from(&answer);
                        self.record_transcript(ReplayItem::Assistant(answer));
                    }
                    self.close_segment();
                }
            }
            RenderBlock::Reasoning { id, text, done, .. } => {
                self.note_stream_content();
                // The shimmer word is updated first and unconditionally: the
                // heading is the answer to "what is it doing", and hiding the
                // thinking body must not also hide that.
                self.absorb_reasoning_heading(id.0, &text, done);
                if !self.flags.show_thinking {
                    return;
                }
                self.open_reasoning(id.0);
                if let Segment::Reasoning(_, stream) = &mut self.segment {
                    stream.push(&text);
                }
                if done {
                    self.close_segment();
                }
            }
            RenderBlock::ToolCall {
                tool_call_id,
                name,
                summary,
                preview,
                status,
                ..
            } => {
                self.note_stream_content();
                if status == ToolCallStatus::Pending || self.tools.contains_key(&tool_call_id.0) {
                    return;
                }
                let kind = ToolKind::from_preview(&name, &preview, &summary);
                let presentation = preview.presentation();
                let activity = super::activity::Activity::from_preview(&name, &preview);
                let result_name = kind.result_formatter_name(&name).to_string();
                let detail = preview_path(&preview);
                // A running tool is more specific than whatever the model was
                // last thinking about, so it takes the detail line under the
                // header. codex keeps the header short for the same reason its
                // `command_lifecycle` does: the interrupt hint must stay visible.
                self.set_status_detail(Some(status_detail_for(&kind)));
                if let Some(status) = self.status.as_mut() {
                    status.note_tool_started(
                        &tool_call_id.0,
                        &activity.tool,
                        activity.target.as_deref(),
                    );
                }
                // 도구가 시작하면 스트림을 먼저 접는다 — codex 도 exec 이
                // 들어오면 활성 칸을 비우고(`flush_active_cell`) 그 자리에
                // ExecCell 을 앉힌다. 칸은 하나다.
                self.close_segment();
                if self.may_take_the_live_slot(LiveSlotRequest::Tool(&kind)) {
                    self.open_tool(tool_call_id.0.clone(), kind.clone());
                } else {
                    self.unseated.push(tool_call_id.0.clone());
                }
                self.tools.insert(
                    tool_call_id.0,
                    PendingTool {
                        name,
                        result_name,
                        detail: summary,
                        path: detail,
                        kind,
                        presentation,
                    },
                );
            }
            RenderBlock::ToolResult {
                tool_call_id,
                is_error,
                body,
                ..
            } => {
                if let Some(status) = self.status.as_mut() {
                    status.note_tool_finished(&tool_call_id.0);
                }
                // A result can arrive without a matching announce; it is still
                // concrete work and its orphan cell earns the turn separator.
                self.had_work_activity = true;
                let announced = self.tools.remove(&tool_call_id.0);
                // A result that came before its call was seated: the call
                // lands as its own committed cell (or is discarded as a
                // transient) and never needs the slot.
                self.unseated.retain(|waiting| *waiting != tool_call_id.0);
                let already_committed = announced.is_none()
                    && self.committed_tool_calls.contains(&tool_call_id.0)
                    && !self
                        .tool_cell
                        .as_ref()
                        .is_some_and(|cell| cell.contains_call(&tool_call_id.0));
                if already_committed {
                    return;
                }
                if !is_error
                    && announced
                        .as_ref()
                        .is_some_and(|pending| matches!(&pending.kind, ToolKind::Spawn { .. }))
                {
                    self.turn_agent_count = self.turn_agent_count.saturating_add(1);
                }
                let body = match announced.as_ref() {
                    Some(pending) => format_indirected_tool_result(
                        &pending.name,
                        &pending.result_name,
                        body,
                        is_error,
                    ),
                    None => body,
                };
                // 결과가 붙는 자리에서 한 번만 적는다. 아직 도는 도구는 화면에
                // 살아 있으므로 여기 없어도 사람이 못 보는 것이 아니다 —
                // 반대로 접혀서 못 보게 되는 것이 바로 이 `body` 다.
                self.record_transcript(ReplayItem::ToolCall {
                    name: announced
                        .as_ref()
                        .map_or_else(|| "tool".to_string(), |pending| pending.name.clone()),
                    input: announced
                        .as_ref()
                        .map_or_else(String::new, |pending| pending.detail.clone()),
                    // 셀이 접기 전의 원문 — 색도 잘라내기도 얹기 전.
                    output: Some(super::tools::body_text(&body)),
                    is_error,
                });
                self.set_status_detail(None);
                if !is_error
                    && announced.as_ref().is_some_and(|pending| {
                        pending.presentation == ToolPresentation::TransientOnSuccess
                    })
                {
                    self.discard_transient_tool(&tool_call_id.0);
                    return;
                }
                // 제출된 플랜은 도구 묶음에 끼지 않는다. 승인해야 할 사람이
                // 승인할 대상을 봐야 하는데, 평범한 도구 결과 셀은 JSON 의
                // `message` 한 줄만 보여 준다 — 플랜 본문이 화면에서 사라진다.
                // 판정은 유효 이름으로 한다 — `ExitPlanModeV2` 는 와이어에 없어
                // (t-2903) 엄격한 프로바이더는 `CapabilityInvoke` 로 감싸 부른다.
                if !is_error
                    && announced
                        .as_ref()
                        .is_some_and(|pending| pending.result_name == PLAN_SUBMISSION_TOOL)
                {
                    if let Some((plan, plan_path)) = parse_submitted_plan(&body) {
                        let width = self.width();
                        self.close_segment();
                        self.flush_tool_cell();
                        let cell =
                            cells::proposed_plan_cell(&plan, plan_path.as_deref(), width);
                        self.history(&cell);
                        self.seat_waiting_tools();
                        return;
                    }
                }
                let path = announced.as_ref().map_or("", |pending| pending.path.as_str());
                let result_kind = ToolKind::from_result(&body);
                let outcome = Outcome::from_result(is_error, &body, path);
                write_tool_profile(format_args!(
                    "session=ui block=Outcome id={} ok={} change={}",
                    tool_call_id.0,
                    outcome.ok,
                    outcome.change.is_some(),
                ));
                self.complete_tool(
                    &tool_call_id.0,
                    outcome,
                    announced.as_ref(),
                    result_kind,
                );
            }
            RenderBlock::AgentResult {
                label,
                status,
                summary,
                body,
                ..
            } => {
                self.record_transcript(ReplayItem::AgentResult {
                    label: label.clone(),
                    status,
                    summary: summary.clone(),
                    body: body.clone(),
                });
                // 하위 에이전트의 보고는 codex 의 MCP 셀 문법으로 — 도구
                // 묶음에 끼지 않는 제 셀이다(`history_cell/mcp.rs`). 배경
                // bash 의 완료만은 같은 명령을 전경에서 돌린 `Ran` 셀이다
                // (t-3177) — 전사에 남는 본문은 위에서 이미 원문 그대로 적었다.
                self.close_segment();
                self.flush_tool_cell();
                let (kind, outcome) =
                    super::tools::background_command_cell(&label, status, &body).unwrap_or_else(
                        || {
                            (
                                ToolKind::AgentResult {
                                    label: crate::util::ansi::sanitize_inline(&label),
                                    summary: summary
                                        .as_deref()
                                        .map(crate::util::ansi::sanitize_inline),
                                },
                                Outcome {
                                    declined: false,
                                    ok: status == AgentResultStatus::Completed,
                                    output: body.clone(),
                                    change: None,
                                },
                            )
                        },
                    );
                let mut group = ToolGroup::new(ToolCall::new(AGENT_RESULT_ID.to_string(), kind));
                group.complete(AGENT_RESULT_ID, outcome);
                let cell = group.committed(width, &mut self.folds, self.fold_mode);
                self.history(&cell);
            }
            RenderBlock::System { level, text, .. } => {
                if let Some(steer) = text.strip_prefix(runtime::STEERING_ECHO_PREFIX) {
                    // This is not a system note. It is core's acknowledgement
                    // that the pending steer crossed a model boundary. Commit
                    // the completed tool first, retire the preview, then add
                    // exactly one normal user cell at that boundary.
                    self.close_segment();
                    self.flush_tool_cell();
                    self.pending_input.acknowledge_steer(steer);
                    // A peer's words (`SendMessage`) are not the person's: they
                    // wear the envelope cell, never the tinted user band.
                    match runtime::parse_peer_message(steer) {
                        Some(peer) => {
                            let cell = cells::peer_message_cell(peer.from, peer.body, width);
                            self.history(&cell);
                        }
                        None => self.user_cell(steer),
                    }
                } else if level == SystemLevel::Housekeeping {
                    // The session's plumbing, not the work: a word beside the
                    // status line while the turn runs, no transcript row, and
                    // the streaming text is not closed around it (t-3177).
                    // Outside a turn there is no status row and nothing to
                    // say — the wire and the plain frontends still carry it.
                    if let Some(status) = self.status.as_mut() {
                        status.note_housekeeping(&text);
                    }
                } else {
                    self.close_segment();
                    let cell = cells::system_cell(level, &text, width);
                    self.history(&cell);
                }
            }
            RenderBlock::UserNotice { message, .. } => {
                self.close_segment();
                let cell = cells::verbatim_cell(&message, width);
                self.history(&cell);
            }
            RenderBlock::Notification {
                title,
                body,
                level,
                road,
                ..
            } => {
                self.close_segment();
                let cell = cells::system_cell(
                    level,
                    &super::strings::push_notification_line(road, &body),
                    width,
                );
                self.history(&cell);
                // Outside the window the terminal itself is the notifier; the
                // bytes ride the next frame so they cannot split a row.
                if road == NotificationRoad::Terminal {
                    self.painter.emit_raw(&bell::terminal_notification(&title, &body));
                }
            }
            RenderBlock::Card { card, .. } => {
                self.close_segment();
                let cell = cells::verbatim_cell(&card.plain_text(), width);
                self.history(&cell);
            }
            RenderBlock::PermissionPrompt(prompt) => {
                self.close_segment();
                self.park(PendingPrompt::Permission(prompt));
            }
            RenderBlock::UserQuestionPrompt(prompt) => {
                self.close_segment();
                self.park(PendingPrompt::Question(prompt));
            }
            // 누적 토큰은 화면에 줄을 만들지 않는다 — 세션이 끝나거나 갈릴
            // 때 나가는 usage 요약의 재료다([`super::summary`]).
            RenderBlock::Usage {
                ctx_tokens,
                cumulative,
                ..
            } => {
                self.usage = cumulative;
                self.ctx_tokens = ctx_tokens;
                self.usage_known = true;
            }
            // The model this request goes out on: the footer's model field
            // follows it, with the reason, while it is not the session model.
            RenderBlock::WireModel(wire) => {
                self.wire = super::wire_model::badge(&wire);
            }
            // 유저 셀은 제출 순간 이미 넣었다. 나머지는 라이브 원장의 재료지
            // 트랜스크립트의 재료가 아니다.
            RenderBlock::UserMessage { .. }
            | RenderBlock::Image { .. }
            | RenderBlock::RateLimit(_)
            | RenderBlock::CompactionProgress { .. } => {}
        }
    }

    /// Content from the model ends the "waiting for the model" note.
    fn note_stream_content(&mut self) {
        if let Some(status) = self.status.as_mut() {
            status.note_stream_content();
        }
    }

    fn open_text(&mut self, id: u64) {
        if matches!(&self.segment, Segment::Text(open, _) if *open == id) {
            return;
        }
        self.close_segment();
        // 답변이 시작하면 앞의 도구 셀은 히스토리로 내려간다 — append-only
        // 이므로 답변 행이 그 앞을 지나가면 순서가 뒤집힌다. 아직 도는 셀은
        // 그대로 둔다(codex 도 활성 칸은 하나지만 도는 exec 을 버리지 않는다).
        self.flush_tool_cell();
        self.segment = Segment::Text(id, MarkdownStream::answer(self.width()));
        self.last_commit = None;
    }

    fn open_reasoning(&mut self, id: u64) {
        if matches!(&self.segment, Segment::Reasoning(open, _) if *open == id) {
            return;
        }
        self.close_segment();
        self.flush_tool_cell();
        self.segment = Segment::Reasoning(id, MarkdownStream::reasoning(self.width()));
        self.last_commit = None;
    }

    fn close_segment(&mut self) {
        let (tail, reasoning) = match &mut self.segment {
            Segment::None => (Vec::new(), None),
            Segment::Text(_, stream) => (stream.finish(), None),
            Segment::Reasoning(_, stream) => {
                let tail = stream.finish();
                (tail, stream.take_reasoning_transcript())
            }
        };
        self.segment = Segment::None;
        self.last_commit = None;
        if let Some(reasoning) = reasoning {
            self.record_reasoning_transcript(reasoning);
        }
        self.history(&tail);
    }

    // ------------------------------------------------------------------
    // 도구 셀 — codex `ExecCell` 의 생애
    // ------------------------------------------------------------------

    /// Whether this announced call may take the viewport's one live cell.
    ///
    /// zo announces a message's whole tool batch before any result arrives
    /// (see [`ToolGroup::accepts`]), and taking the slot from a cell that is
    /// still running closes its calls as failed. That was harmless while a
    /// delegation had no live cell at all; once it had one, a `Read` and an
    /// `Agent` announced together took turns marking each other red — and
    /// four edits announced before a `bash` were committed as
    /// `✘ Failed to apply patch` while their results, all `is_error: false`,
    /// were still on their way (2026-09-07 23:43, t-3063).
    ///
    /// So a running cell is never evicted, whatever was announced. The call
    /// that stays out loses nothing: it waits in [`Ui::unseated`] and is
    /// seated when the cell finishes, or its result lands first as its own
    /// committed cell through the orphan path.
    fn may_take_the_live_slot(&self, request: LiveSlotRequest<'_>) -> bool {
        match request {
            LiveSlotRequest::Quiet(loop_id) => {
                self.tool_cell.is_none()
                    && self
                        .quiet_loop_cell
                        .as_ref()
                        .is_none_or(|cell| cell.loop_id == loop_id)
            }
            LiveSlotRequest::Tool(kind) => self
                .tool_cell
                .as_ref()
                .is_none_or(|cell| cell.accepts(kind) || !cell.is_active()),
        }
    }

    /// Seat the calls that waited for the live cell, in announce order, now
    /// that nothing in it is running. A waiting call whose result already
    /// came has left [`Ui::tools`] and is skipped; the first one seated that
    /// the cell does not accept retires the finished cell to history and
    /// opens its own, which is the picture the person expects — the edits
    /// committed as `Edited N files`, the command that followed them live.
    fn seat_waiting_tools(&mut self) {
        while self.tool_cell.as_ref().is_none_or(|cell| !cell.is_active()) {
            if self.unseated.is_empty() {
                return;
            }
            let id = self.unseated.remove(0);
            let Some(kind) = self.tools.get(&id).map(|pending| pending.kind.clone()) else {
                continue;
            };
            self.open_tool(id, kind);
        }
    }

    /// 새 호출을 지금 셀에 붙이거나, 못 붙이면 셀을 갈아 끼운다 —
    /// codex `ExecCell::add_call` 이 거짓을 내면 `chatwidget` 이 셀을 새로
    /// 여는 그 흐름이다.
    fn open_tool(&mut self, id: String, kind: ToolKind) {
        self.had_work_activity = true;
        if let Some(cell) = self.tool_cell.as_mut() {
            if cell.accepts(&kind) {
                cell.push(ToolCall::new(id, kind));
                return;
            }
        }
        self.retire_tool_cell();
        self.tool_cell = Some(ToolGroup::new(ToolCall::new(id, kind)));
    }

    /// 결과를 셀에 붙이고, codex 의 `should_flush` 가 참이면 히스토리로 접는다.
    ///
    /// `complete` 가 거짓이면 라우팅 불일치다(codex 도 그 거짓을 "orphan
    /// `exec_end`" 신호로 쓴다) — 남의 셀에 붙이는 대신 그 결과만으로 셀 하나를
    /// 세워 바로 커밋한다.
    fn complete_tool(
        &mut self,
        id: &str,
        outcome: Outcome,
        announced: Option<&PendingTool>,
        result_kind: Option<ToolKind>,
    ) {
        let attached = self.tool_cell.as_mut().is_some_and(|cell| {
            match result_kind.as_ref() {
                Some(kind) => cell.complete_with_kind(id, outcome.clone(), kind.clone()),
                None => cell.complete(id, outcome.clone()),
            }
        });
        if !attached {
            if self.committed_tool_calls.contains(id) {
                return;
            }
            // The open cell is only retired when nothing in it is still
            // running. Retiring an active one calls `mark_failed`, which writes
            // `ok: false, output: "", change: None` into every unfinished call —
            // and that is exactly the shape of the reported defect: a
            // **successful** edit committed as a red bullet with `(no output)`
            // and no `(+N -M)`. A result that could not attach says nothing
            // about the calls that are still waiting for theirs.
            if !self.tool_cell.as_ref().is_some_and(ToolGroup::is_active) {
                self.retire_tool_cell();
            }
            let kind = result_kind.unwrap_or_else(|| orphan_kind(&outcome, announced));
            let mut orphan = ToolGroup::new(ToolCall::new(id.to_string(), kind));
            orphan.complete(id, outcome);
            let width = self.width();
            let cell = orphan.committed(width, &mut self.folds, self.fold_mode);
            self.history(&cell);
            self.committed_tool_calls.insert(id.to_string());
            self.seat_waiting_tools();
            return;
        }
        if self.tool_cell.as_ref().is_some_and(ToolGroup::should_flush) {
            self.flush_tool_cell();
        }
        self.seat_waiting_tools();
        self.refresh_status_details();
    }

    /// Remove a successful internal-preparation call from the live cell.
    ///
    /// Its transcript entry is recorded before this point, so Ctrl+T retains
    /// the existing detail while ordinary history returns to useful work.
    fn discard_transient_tool(&mut self, id: &str) {
        if self.tool_cell.as_mut().is_some_and(|cell| cell.discard(id))
            && self.tool_cell.as_ref().is_some_and(ToolGroup::is_empty)
        {
            self.tool_cell = None;
        }
        self.seat_waiting_tools();
        self.refresh_status_details();
    }

    /// 끝난 셀을 히스토리로 내린다. 아직 도는 셀은 건드리지 않는다 — 활성
    /// 칸에서 계속 살아야 한다.
    fn flush_tool_cell(&mut self) {
        if self.tool_cell.as_ref().is_some_and(ToolGroup::is_active) {
            return;
        }
        self.retire_tool_cell();
    }

    /// 셀을 무조건 내린다. 아직 도는 호출은 codex `ExecCell::mark_failed` 처럼
    /// 실패로 닫는다 — 턴이 끊겼거나, 그 셀에 못 붙는 도구가 자리를 달라고
    /// 왔거나, 턴이 끝났다.
    fn retire_tool_cell(&mut self) {
        let Some(mut cell) = self.tool_cell.take() else {
            return;
        };
        if cell.is_empty() {
            return;
        }
        cell.mark_failed();
        self.committed_tool_calls
            .extend(cell.call_ids().map(str::to_string));
        let width = self.width();
        let lines = cell.committed(width, &mut self.folds, self.fold_mode);
        self.history(&lines);
    }

    /// 훅 보고용 관찰 — 블록을 소비하지 않는다.
    fn observe(&self, block: &RenderBlock, session_id: &str, last_assistant: &mut String) {
        self.profile_tool_block(block, session_id);
        let Some(reporter) = self.reporter.as_ref() else {
            return;
        };
        match block {
            RenderBlock::TextDelta { text, .. } => last_assistant.push_str(text),
            RenderBlock::ToolCall {
                tool_call_id,
                name,
                summary,
                preview,
                status,
                ..
            } => {
                if *status != ToolCallStatus::Pending && !self.tools.contains_key(&tool_call_id.0) {
                    // One `PreToolUse` per call the runtime minted: the first
                    // sighting of this id, never a later status of it and
                    // never a repaint. Two Reads of one path are two ids and
                    // two hooks; the seconds a call has been running are the
                    // channel's fact (`publish_working_activity`), not a hook.
                    let activity = super::activity::Activity::from_preview(name, preview);
                    reporter.pre_tool_use(
                        name,
                        activity.hook_input(summary),
                        &activity.started_card(),
                        session_id,
                    );
                    if crate::ide::reporter::HookReporter::spawns_subagent(name) {
                        reporter.subagent_start(
                            &tool_call_id.0,
                            name,
                            crate::ide::reporter::HookReporter::detail_of(name, summary),
                            session_id,
                        );
                    }
                }
            }
            RenderBlock::ToolResult {
                tool_call_id,
                is_error,
                ..
            } => {
                let name = self
                    .tools
                    .get(&tool_call_id.0)
                    .map_or("tool", |call| call.name.as_str());
                reporter.post_tool_use(name, *is_error, session_id);
                if crate::ide::reporter::HookReporter::spawns_subagent(name) {
                    reporter.subagent_stop(
                        &tool_call_id.0,
                        name,
                        "",
                        session_id,
                        &crate::session::subagent_progress::background_tasks_for_session(
                            &self.registry,
                            session_id,
                        ),
                    );
                }
            }
            RenderBlock::PermissionPrompt(prompt) => {
                reporter.permission_request(&prompt.tool_name, &prompt.reasoning, session_id);
            }
            RenderBlock::UserQuestionPrompt(prompt) => {
                reporter.question(&prompt.question, session_id);
            }
            _ => {}
        }
    }

    /// 도구 블록이 화면 상태기계에 닿은 **그 순간**의 변종과 라우팅 상태.
    ///
    /// 이 계측은 [`PROFILE_TOOL_RESULTS_ENV`] 가 파일 경로로 주어졌을 때만
    /// 열린다. 특히 결과를 소비하기 전에 찍으므로 `Diff` 가 위에서부터
    /// 사라졌는지, 아니면 TUI 안에서 잃었는지를 한 줄로 가른다.
    fn profile_tool_block(&self, block: &RenderBlock, session_id: &str) {
        match block {
            RenderBlock::ToolCall {
                tool_call_id,
                name,
                status,
                ..
            } => {
                write_tool_profile(format_args!(
                    "session={session_id} block=ToolCall id={} name={name} status={status:?} announced={}",
                    tool_call_id.0,
                    self.tools.contains_key(&tool_call_id.0),
                ));
            }
            RenderBlock::ToolResult {
                tool_call_id,
                is_error,
                body,
                ..
            } => {
                write_tool_profile(format_args!(
                    "session={session_id} block=ToolResult id={} is_error={is_error} body={} announced={}",
                    tool_call_id.0,
                    tool_result_profile(body),
                    self.tools.contains_key(&tool_call_id.0),
                ));
            }
            _ => {}
        }
    }
}

fn write_tool_profile(args: std::fmt::Arguments<'_>) {
    let Some(file) = PROFILE_TOOL_RESULTS
        .get_or_init(|| {
            let path = std::env::var_os(PROFILE_TOOL_RESULTS_ENV)?;
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .ok()
                .map(Mutex::new)
        })
        .as_ref()
    else {
        return;
    };
    let Ok(mut file) = file.lock() else {
        return;
    };
    let _ = writeln!(file, "{args}");
}

fn tool_result_profile(body: &ToolResultBody) -> String {
    match body {
        ToolResultBody::Text { content, truncated } => {
            format!("Text(len={},truncated={truncated})", content.len())
        }
        ToolResultBody::Bash(result) => format!(
            "Bash(exit={},stdout_len={},stderr_len={})",
            result.exit_code,
            result.stdout.len(),
            result.stderr.len()
        ),
        ToolResultBody::Read {
            path,
            content,
            truncated,
            ..
        } => format!(
            "Read(path={path:?},len={},truncated={truncated})",
            content.len()
        ),
        ToolResultBody::Diff(diff) => format!("Diff(hunks={})", diff.hunks.len()),
        ToolResultBody::Listing { entries, truncated } => format!(
            "Listing(entries={},truncated={truncated})",
            entries.len()
        ),
        ToolResultBody::Todos(items) => format!("Todos(items={})", items.len()),
        ToolResultBody::Declined { reason } => format!("Declined(len={})", reason.len()),
        ToolResultBody::Generic {
            name,
            content,
            truncated,
        } => format!(
            "Generic(name={name:?},len={},truncated={truncated})",
            content.len()
        ),
    }
}

// ============================================================================
// 세션과 화면을 잇는 루프
// ============================================================================

impl App {
    fn new(
        mut session: PlainSession,
        flags: RenderFlags,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let (cols, rows) = crossterm::terminal::size()?;
        let (_, cursor_row) = crossterm::cursor::position().unwrap_or((0, 0));
        let color = !crate::render::no_color_env();
        let painter = Painter::new(std::io::stdout(), cols, rows, cursor_row, color);
        let status = session.status();
        let composer_seed = session.composer_history_seed();
        let composer = Composer::for_session(&status.session_id, composer_seed);
        let (model, fast) = fast::display(&status.model);
        let effort = status.effort.unwrap_or("").to_string();
        let effort_tier = Effort::from_token(&effort).and_then(EffortTier::from_effort);
        let agent_completion_pump = session.start_agent_completion_pump();
        let subagent_frame_relay = events::start_subagent_frame_relay(
            events::channel(),
            session.registry(),
            status.session_id.clone(),
        );
        let session_cwd = session.cwd.clone();
        let cwd = view::short_cwd(&session_cwd.to_string_lossy());
        let worktree_context = worktree_context(&session_cwd);
        let footer_location = view::footer_location(&cwd, worktree_context.as_deref());
        Ok(Self {
            ui: Ui {
                flags,
                painter,
                paint_probe: None,
                composer,
                pending_input: PendingInputs::default(),
                segment: Segment::None,
                tools: HashMap::new(),
                unseated: Vec::new(),
                committed_tool_calls: HashSet::new(),
                tool_cell: None,
                quiet_loop_cell: None,
                quiet_candidate: None,
                had_work_activity: false,
                turn_agent_count: 0,
                folds: FoldIds::default(),
                fold_mode: FoldMode::from_env(),
                last_commit: None,
                deferred: Vec::new(),
                pending_history: Vec::new(),
                passport_gate: PassportGate::default(),
                status: None,
                tool_status_detail: None,
                subagent_progress: Vec::new(),
                subagent_wave: SubagentWave::default(),
                published_activity: PublishedActivity::Never,
                reasoning_scan: None,
                parked: None,
                picker: None,
                sessions: None,
                permissions: None,
                agents: None,
                transcript: None,
                transcript_items: Vec::new(),
                transcript_answer: String::new(),
                transcript_bytes: 0,
                last_answer: String::new(),
                popup_selected: 0,
                reporter: HookReporter::from_env(),
                model,
                fast,
                wire: None,
                effort,
                effort_tier,
                effort_effect: None,
                osc_guard: LateOscGuard::armed_if_needed(),
                ctx_tokens: 0,
                permission_mode: session.permission_mode,
                permission_cell: Some(session.permission_cell()),
                cwd,
                session_cwd,
                footer_location,
                startup_quit_grace_until: Some(Instant::now() + STARTUP_QUIT_GRACE),
                session_id: status.session_id.clone(),
                registry: session.registry(),
                goal: status.goal,
                autonomous: status.autonomous,
                loops: status.loops,
                last_interrupt: None,
                usage: TokenUsage::default(),
                usage_known: false,
                exit: None,
            },
            session: Some(session),
            agent_completion_pump,
            subagent_frame_relay,
            close_requested: None,
            signals: TerminationSignals::install(),
        })
    }

    /// 턴 밖에서만 부른다 — 턴이 도는 동안에는 세션이 다른 task 에 있다.
    fn session(&self) -> &PlainSession {
        self.session
            .as_ref()
            .expect("the session is only away while a turn runs")
    }

    fn session_mut(&mut self) -> &mut PlainSession {
        self.session
            .as_mut()
            .expect("the session is only away while a turn runs")
    }

    /// Idle Alt+A performs one explicit on-demand scan. It is never called by
    /// paint or frame ticks, and the filesystem work stays off the async core
    /// that drives terminal input.
    fn open_agents(&mut self) {
        let session_id = self.ui.session_id.clone();
        let registry = std::sync::Arc::clone(&self.ui.registry);
        let progress = tokio::task::block_in_place(|| {
            crate::session::subagent_progress::snapshot_for_session(&registry, &session_id)
        });
        self.ui.open_agents(progress);
    }

    /// 끝까지 돈다. 종료 요약은 화면을 놓은 뒤 [`run`] 이 찍는다 — raw 모드가
    /// 풀린 다음이라야 `println!` 이 codex 와 같은 바이트를 낸다.
    /// 마지막 값은 이번 실행에서 완성된 마지막 어시스턴트 답변이다 —
    /// `--last-message` 가 쓰는 것.
    async fn drive(
        mut self,
    ) -> Result<(ExitReason, Option<SessionSummary>, String), Box<dyn std::error::Error>> {
        self.boot();
        let mut events = EventStream::new();
        let mut ticker = tokio::time::interval(FRAME_TICK);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut size_poll = tokio::time::interval(SIZE_POLL);
        size_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        let reason = loop {
            if let Some(reason) = self.ui.exit.take() {
                break reason;
            }
            if self.ui.painter.failed() {
                break ExitReason::OutputClosed;
            }
            // What the catalog layer learned before there was a screen to say
            // it on — an alias that moved, a settings pin. Once each.
            for text in crate::runtime_support::drain_catalog_notices() {
                self.ui.note(SystemLevel::Info, &text);
            }
            let next_wakeup = self.session().next_autonomy_wakeup();
            tokio::select! {
                biased;
                event = events.next() => match event {
                    Some(Ok(event)) => {
                        match self.ui.idle_event(&event) {
                            KeyOutcome::Submitted(line) => {
                                if let Some(prompt) = self.submit(&line) {
                                    // Heap-pinned: the chain grew past clippy's large-future line when the
                                    // turn loop learned a fourth command (t-2513), and a turn's future
                                    // carries a screen's worth of state anyway.
                                    Box::pin(self.run_turn_chain(prompt, &mut events, None)).await;
                                }
                            }
                            KeyOutcome::Chosen(model, effort) => self.apply_choice(&model, effort),
                            KeyOutcome::Permission(mode) => self.apply_permission_mode(mode),
                            KeyOutcome::Resumed(id) => self.resume_session(&id),
                            KeyOutcome::OpenAgents => self.open_agents(),
                            KeyOutcome::Nothing => {}
                        }
                        self.drive_autonomy(&mut events).await;
                        self.ui.draw();
                    }
                    Some(Err(_)) | None => break ExitReason::UserExit,
                },
                () = TerminationSignals::delivered(&mut self.signals) => break ExitReason::UserExit,
                followup = recv_agent_followup(&mut self.agent_completion_pump) => {
                    let prompt = Submission {
                        text: followup.text.clone(),
                        image_paths: Vec::new(),
                    };
                    Box::pin(self.run_turn_chain(prompt, &mut events, Some(followup))).await;
                    self.drive_autonomy(&mut events).await;
                    self.ui.draw();
                },
                () = wait_for_wakeup(next_wakeup) => {
                    self.drive_autonomy(&mut events).await;
                    self.ui.draw();
                },
                command = crate::ide::events::next_command(crate::ide::events::channel()) => {
                    if let Command::AccountSwitch { label } = command {
                        let notice = crate::status_format::account_switch_notice(&label);
                        self.ui.status = Some(Status::notice(notice));
                        self.ui.draw();
                        // Keep the already-painted row for this idle beat, but
                        // do not make later resize/timer frames print it again.
                        self.ui.status = None;
                    }
                },
                // 움직일 것이 있을 때만 틱이 그린다([`Ui::animating`]). 유휴에는
                // 이 팔이 아예 꺼져 루프가 다음 키·리사이즈까지 잠들고, 프레임은
                // 한 장도 나가지 않는다. 애니메이션이 다시 뜨면 `interval` 은
                // 지나간 마감을 즉시 한 번 돌려주고 그 뒤로 다시 32ms 케이던스를
                // 잡으므로(`MissedTickBehavior::Delay`), 도는 동안의 프레임 간격과
                // 바이트는 그대로다.
                _ = ticker.tick(), if self.ui.animating() => self.ui.draw(),
                // 유휴에는 프레임이 없으니 리사이즈 신호가 빠지면 아무도 모른다 —
                // 초당 한 번 pty 에 직접 묻는다([`Ui::reconcile_size`]).
                _ = size_poll.tick() => if self.ui.tend_terminal() { self.ui.draw(); },
            }
        };

        // 정리 future 는 화면 상태를 통째로 안고 가므로 힙에 둔다.
        // `finish` 가 self 를 가져가므로 답은 그 **전에** 꺼낸다.
        let last_answer = std::mem::take(&mut self.ui.last_answer);
        let summary = Box::pin(self.finish(reason)).await;
        Ok((reason, summary, last_answer))
    }

    /// A teammate's whole life (t-2513 §2.2): the brief's turn, then idle for
    /// the parent's next words, each answered into its own `result-<n>.json`,
    /// until the pane is ended — by the parent (`teammate.close`), by the
    /// parent's channel going away past the grace, by the idle budget, or by
    /// a person at this keyboard.
    ///
    /// `turn` rather than `run_turn_chain`: a chain exists to start what a
    /// person queued while the last turn ran, and to fold in a background
    /// helper's completion. A teammate has neither — it cannot spawn, and
    /// nobody is queueing at its keyboard; its queue is the parent's channel.
    async fn drive_teammate(
        mut self,
        prompt: String,
        banner: String,
        lifecycle: crate::teammate::Lifecycle,
    ) -> Result<(Option<SessionSummary>, TeammateLife), Box<dyn std::error::Error>> {
        use crate::teammate::{close_reason_from, result_for_turn, write_closing};
        use runtime::subagent_panes::CloseReason;

        self.boot();
        // Who sent this, said before the prompt is echoed: a person who opens
        // this pane by hand has to be able to tell it from a session they
        // started themselves.
        self.ui.note(SystemLevel::Info, &banner);
        let mut events = EventStream::new();
        let transcript = self.session().handle.path.clone();
        let mut turn = lifecycle.first_turn;
        let mut answered = 0_u32;
        let mut prior_output_tokens = 0_u64;
        let mut next_prompt = Some(prompt);
        let reason = loop {
            if let Some(prompt) = next_prompt.take() {
                // Both futures hold a screen's worth of state; on the heap,
                // like every other turn future in this file.
                let report =
                    Box::pin(self.teammate_turn(prompt, &mut events, &mut prior_output_tokens)).await;
                // The one write that must happen. A child that answered and
                // could not say so leaves its parent waiting out the whole
                // budget for work already done.
                result_for_turn(&lifecycle.agent_id, turn, &report, &transcript)
                    .write_turn(&lifecycle.directory, turn)
                    .map_err(|error| format!("could not write the teammate's result: {error}"))?;
                answered += 1;
                if self.session.is_none() {
                    // The turn task panicked; there is no session to run the
                    // next turn on. Leave the way a person would.
                    break CloseReason::UserExit;
                }
            }
            if let Some(word) = self.close_requested.take() {
                break close_reason_from(&word);
            }
            if self.ui.exit.take().is_some() {
                break CloseReason::UserExit;
            }
            self.ui.note(SystemLevel::Info, super::strings::TEAMMATE_IDLE);
            self.ui.draw();
            match Box::pin(self.await_parent(&lifecycle, &mut events)).await {
                IdleOutcome::NextTurn(text) => {
                    turn += 1;
                    self.ui.note(SystemLevel::Info, &super::strings::teammate_next_turn(turn));
                    next_prompt = Some(text);
                }
                IdleOutcome::Close(reason) => break reason,
            }
        };
        self.ui.note(SystemLevel::Info, super::strings::teammate_closing_reason(reason));
        self.ui.note(SystemLevel::Success, super::strings::TEAMMATE_CLOSING);
        self.ui.draw();
        // Said LAST, after every per-turn result: the parent's watcher reads
        // it as "no more answers are coming" (`PaneOutcome::Closed`).
        write_closing(&lifecycle, reason, answered)
            .map_err(|error| format!("could not write the teammate's closing document: {error}"))?;
        lifecycle.retire_channel();
        let summary = Box::pin(self.finish(ExitReason::UserExit)).await;
        Ok((
            summary,
            TeammateLife {
                reason,
                turns: answered,
            },
        ))
    }

    /// One teammate turn, reported for its `result-<n>.json`.
    async fn teammate_turn(
        &mut self,
        prompt: String,
        events: &mut EventStream,
        prior_output_tokens: &mut u64,
    ) -> crate::teammate::TurnReport {
        let submission = Submission {
            text: prompt,
            image_paths: Vec::new(),
        };
        let completion = match self.submit(&submission) {
            Some(prompt) => Box::pin(self.turn(&prompt, events, None, None)).await,
            // An empty brief cannot become a turn. The parent gets the same
            // sentence the screen does rather than an empty answer.
            None => TurnOutcome {
                summary: None,
                error: Some("the brief carried no prompt".to_string()),
                permission_blocked: false,
                question_blocked: false,
                cancelled: false,
            },
        };
        // The session's usage counter is cumulative; each result reports its
        // own turn's spend, as an inline child's completion does.
        let total_output_tokens = u64::from(self.ui.usage.output_tokens);
        let output_tokens = total_output_tokens.saturating_sub(*prior_output_tokens);
        *prior_output_tokens = total_output_tokens;
        crate::teammate::TurnReport {
            answer: std::mem::take(&mut self.ui.last_answer),
            cancelled: completion.cancelled,
            error: completion.error,
            output_tokens,
            tool_calls: self.ui.committed_tool_calls.len() as u64,
        }
    }

    /// Wait, idle, for whatever ends the wait: the parent's next words, the
    /// parent's `teammate.close`, the parent's channel gone past the grace,
    /// the idle budget, or a person leaving.
    ///
    /// Nothing runs here, so nothing is cancelled; a person's typed line is
    /// answered with a note rather than a turn — the parent drives this pane,
    /// and a locally opened turn would write a result nobody is waiting on
    /// under a number the parent counts differently.
    async fn await_parent(
        &mut self,
        lifecycle: &crate::teammate::Lifecycle,
        events: &mut EventStream,
    ) -> IdleOutcome {
        use crate::teammate::{IdleClock, ParentWatch};
        use runtime::subagent_panes::CloseReason;

        let idle = IdleClock::start(lifecycle.idle_budget);
        let mut watch = ParentWatch::new(lifecycle.limits.parent_liveness_grace);
        let mut liveness = tokio::time::interval(lifecycle.limits.parent_liveness_poll);
        liveness.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // The first tick of an interval is immediate; the parent was there a
        // moment ago, so the first look waits a whole period.
        liveness.tick().await;
        let mut size_poll = tokio::time::interval(SIZE_POLL);
        size_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let idle_deadline = tokio::time::Instant::from_std(idle.deadline());
        loop {
            tokio::select! {
                biased;
                event = events.next() => match event {
                    Some(Ok(event)) => {
                        if let KeyOutcome::Submitted(_) = self.ui.idle_event(&event) {
                            self.ui.note(SystemLevel::Info, super::strings::TEAMMATE_PARENT_DRIVES);
                        }
                        if self.ui.exit.take().is_some() {
                            return IdleOutcome::Close(CloseReason::UserExit);
                        }
                        self.ui.draw();
                    }
                    // The terminal is gone: the pane was closed under us.
                    Some(Err(_)) | None => return IdleOutcome::Close(CloseReason::UserExit),
                },
                () = TerminationSignals::delivered(&mut self.signals) => {
                    return IdleOutcome::Close(CloseReason::UserExit);
                }
                command = crate::ide::events::next_command(crate::ide::events::channel()) => match command {
                    Command::Steer { text } => return IdleOutcome::NextTurn(text),
                    Command::Close { reason } => {
                        return IdleOutcome::Close(crate::teammate::close_reason_from(&reason));
                    }
                    // Nothing is running; a Stop that arrives now is late.
                    Command::CancelTurn { .. } => {}
                    Command::AccountSwitch { label } => {
                        self.ui.note(
                            SystemLevel::Info,
                            &crate::status_format::account_switch_notice(&label),
                        );
                        self.ui.draw();
                    }
                },
                _ = liveness.tick() => {
                    if let Some(discovery) = lifecycle.parent_channel.clone() {
                        // A dead parent costs the connect timeout; off the
                        // frame loop so the screen stays live.
                        let timeout = lifecycle.limits.channel_timeout;
                        let alive = tokio::task::spawn_blocking(move || ParentWatch::probe(&discovery, timeout))
                            .await
                            .unwrap_or(true);
                        if watch.observe(alive, Instant::now()) {
                            return IdleOutcome::Close(CloseReason::ParentLost);
                        }
                    }
                }
                () = tokio::time::sleep_until(idle_deadline) => {
                    return IdleOutcome::Close(CloseReason::IdleBudget);
                }
                _ = size_poll.tick() => if self.ui.tend_terminal() { self.ui.draw(); },
            }
        }
    }

    /// 배너 자리 — codex 의 부팅 카드.
    fn boot(&mut self) {
        self.session().fire_session_start();
        if let Some(reporter) = self.ui.reporter.as_ref() {
            reporter.session_start(
                if self.session().resumed() { "resume" } else { "startup" },
                &self.session().handle.id,
                &self.session().handle.path.to_string_lossy(),
                &self.session().cwd.to_string_lossy(),
                &self.session().model,
            );
        }
        self.ui.painter.set_height(5);
        self.session_card();
        if self.session().resumed() {
            self.replay_history();
        }
        self.ui.paint();
    }

    /// codex 의 세션 정보 셀 — 부팅 카드다. 새로 열 때도, `/resume` 이 세션을
    /// 갈아 끼울 때도 같은 자리에 선다: codex
    /// `app/session_lifecycle.rs::resume_target_session` 은 재개 스레드를
    /// `ThreadAttachPresentation::SessionLineage` 로 붙이고, 그 길이
    /// `handle_thread_session` → `on_session_configured_with_display…` 의
    /// `SessionConfiguredDisplay::Normal` 갈래라 `new_session_info` 로 만든
    /// 헤더 상자를 히스토리에 **다시** 커밋한 뒤 턴을 재생한다.
    fn session_card(&mut self) {
        let width = self.ui.width();
        let display_effort = fast::display_effort(&self.ui.effort, self.ui.fast);
        let card = cells::boot_card(
            crate::status_format::PRODUCT,
            env!("CARGO_PKG_VERSION"),
            &self.ui.model,
            &display_effort,
            &self.ui.cwd,
            width,
        );
        self.ui.history(&card);
    }

    /// 재개한 세션의 이전 대화를 히스토리에 다시 그린다 — codex
    /// `chatwidget/replay.rs::replay_thread_turns`("Replay a subset of initial
    /// events into the UI to seed the transcript when resuming an existing
    /// session").
    ///
    /// 원본의 규율 둘을 그대로 지킨다.
    ///
    /// - **재생은 라이브와 같은 경로를 탄다.** codex 의 `replay_thread_item` 은
    ///   `handle_thread_item` 으로 들어가고, 거기서 유저 셀·답변 셀·도구 셀은
    ///   실시간 사건과 **같은 핸들러**(`on_committed_user_message` ·
    ///   `on_agent_message_item_completed` · `handle_command_execution_started_now`
    ///   + `…_completed_now`)가 만든다. 그래서 재생 셀과 실시간 셀의 바이트가
    ///     같다. 우리도 [`ReplayItem`] 을 `RenderBlock` 으로 되돌려
    ///     [`Ui::block`] 에 먹인다 — 셀 조립부는 한 벌뿐이다.
    /// - **안전한 항목만 재생한다.** 원본이 "only safe-to-replay items are
    ///   rendered to avoid triggering side effects" 라 적는 자리이고,
    ///   `PlainSession::replay_items` 가 이미 System·thinking 을 빼고
    ///   `ToolResult` 를 그 호출에 붙여 준다.
    ///
    /// 도구 미리보기·결과 본문은 라이브 스트림과 **같은 함수**로 만든다
    /// (`preview_tool_input` → `preview_summary`, `format_tool_result_from_raw`)
    /// — 파서가 `RenderBlock::ToolCall`/`ToolResult` 를 만들 때 부르는 그것이다
    /// (`message_stream/anthropic/parser.rs`, `conversation/streaming_turn.rs`).
    fn replay_history(&mut self) {
        let items = self.session().replay_items();
        if items.is_empty() {
            return;
        }
        let ids = BlockIdGen::default();
        for (index, item) in items.into_iter().enumerate() {
            match item {
                ReplayItem::User(text) => self.ui.user_cell(&text),
                ReplayItem::Assistant(text) => self.ui.block(RenderBlock::TextDelta {
                    id: ids.next(),
                    text,
                    done: true,
                }),
                ReplayItem::AgentResult {
                    label,
                    status,
                    summary,
                    body,
                } => self.ui.block(RenderBlock::AgentResult {
                    id: ids.next(),
                    label,
                    status,
                    summary,
                    body,
                }),
                ReplayItem::ToolCall {
                    name,
                    input,
                    output,
                    is_error,
                } => {
                    // 라이브 파서와 같은 폴백이다 — 입력이 JSON 이 아니면 그
                    // 문자열 자체를 값으로 삼는다(`parser.rs` 의
                    // `unwrap_or(Value::String(partial_json.clone()))`).
                    let parsed = serde_json::from_str(&input)
                        .unwrap_or(serde_json::Value::String(input));
                    let preview = preview_tool_input(&name, &parsed);
                    let summary = preview_summary(&preview);
                    // 트랜스크립트의 호출 id 는 여기 오지 않는다(재생 항목은
                    // 이름·입력·출력만 든다). 묶임 판정은 **이 셀 안에서만**
                    // 쓰이므로 재생 순번으로 유일하게 만든다.
                    let tool_call_id = ToolCallId(format!("replay-{index}"));
                    self.ui.block(RenderBlock::ToolCall {
                        id: ids.next(),
                        tool_call_id: tool_call_id.clone(),
                        name: name.clone(),
                        summary,
                        preview,
                        status: ToolCallStatus::Running,
                    });
                    if let Some(output) = output {
                        let body = format_tool_result_from_raw(&name, &output, is_error);
                        self.ui.block(RenderBlock::ToolResult {
                            id: ids.next(),
                            tool_call_id,
                            is_error,
                            body,
                        });
                    }
                }
            }
        }
        // 턴 끝과 같은 마무리 — 열린 스트림을 확정하고 아직 도는 도구 셀을
        // 접는다(결과 없이 끝난 호출은 codex `mark_failed` 처럼 닫힌다).
        self.ui.close_segment();
        self.ui.retire_tool_cell();
    }

    /// 화면을 놓고 세션을 닫는다. 종료 요약을 돌려주면 [`run`] 이 찍는다.
    async fn finish(mut self, reason: ExitReason) -> Option<SessionSummary> {
        // Lines handed over since the last frame still belong on screen.
        self.ui.commit_quiet_loop_cell();
        self.ui.flush_history();
        self.ui.painter.leave();
        // Stop polling before the session-end hooks and persistence run. This
        // also covers a turn-task panic, where `self.session` is already gone.
        self.subagent_frame_relay = None;
        // 턴 task 가 패닉으로 죽었으면 세션이 돌아오지 않는다 — 그때도 화면
        // 정리는 마쳤으니 훅·영속만 건너뛴다.
        let session = self.session.as_mut()?;
        session.fire_session_end(hook_reason(reason));
        if let Some(reporter) = self.ui.reporter.as_ref() {
            reporter
                .session_end(&session.handle.id, hook_reason(reason))
                .await;
        }
        let _ = session.persist();
        // 요약은 **영속 뒤**에 뜬다: 재개 힌트는 트랜스크립트가 실재하고 비어
        // 있지 않을 때만 서기 때문이다(codex `rollout_path_is_resumable`).
        summary::session_summary(
            self.ui.usage,
            &session.handle.id,
            &session.handle.path,
            crate::dream::session_end_line(),
        )
    }

    /// 제출된 한 줄. 턴으로 갈 프롬프트면 그것을 돌려준다.
    fn submit(&mut self, submission: &Submission) -> Option<Submission> {
        let trimmed = submission.text.trim();
        if trimmed.is_empty() {
            return None;
        }
        // `/resume` is a command; `/Users/dev/Desktop/web.config 이 파일 확인해봐`
        // is a sentence that happens to start with a path (live report
        // 2026-09-09) — `slash::classify` reads the head and asks the disk
        // about a one-word head like `/tmp`.
        match slash::classify(trimmed, |path| std::path::Path::new(path).exists()) {
            slash::Line::Escaped(unescaped) => {
                // `//foo` 는 슬래시로 시작하는 평문의 이스케이프다.
                let text = unescaped.to_string();
                self.ui.user_cell(&text);
                return Some(Submission {
                    text,
                    image_paths: submission.image_paths.clone(),
                });
            }
            slash::Line::Command(command) => {
                self.ui.user_cell(trimmed);
                self.slash(command);
                return None;
            }
            slash::Line::Plain | slash::Line::Path => {}
        }
        self.ui.user_cell(trimmed);
        Some(Submission {
            text: trimmed.to_string(),
            image_paths: submission.image_paths.clone(),
        })
    }

    #[allow(clippy::too_many_lines)] // keep-list 슬래시마다 한 arm.
    fn slash(&mut self, command: &str) {
        let (name, arg) = match command.split_once(char::is_whitespace) {
            Some((name, arg)) => (name, arg.trim()),
            None => (command, ""),
        };
        match Slash::from_word(name) {
            Some(Slash::Exit) => self.ui.exit = Some(ExitReason::UserExit),
            Some(Slash::Help) => {
                let width = self.ui.width();
                let body = view::shortcut_card().join("\n");
                let cell = cells::verbatim_cell(&body, width);
                self.ui.history(&cell);
            }
            // codex 는 `/status` 에 한 줄이 아니라 카드로 답한다 — 조립은
            // `status_format::session_status_card` 한 함수뿐이고, 파이프도
            // 같은 행을 인쇄한다.
            Some(Slash::Status) => {
                let width = self.ui.width();
                let rows = crate::status_format::session_status_card(self.session(), width);
                // 카드 앞에 빈 줄을 넣지 않는다 — 캡처의 `/status` 는 유저 셀의
                // 꼬리 빈 줄 **하나** 뒤에 곧장 상자를 세운다(부팅 카드와 같다).
                self.ui.history(&rows);
            }
            Some(Slash::Model) => {
                if arg.is_empty() {
                    self.ui.open_model_picker();
                    return;
                }
                self.set_model(arg);
            }
            Some(Slash::Fast) => {
                let enabled = !self.ui.fast;
                self.set_fast_mode(enabled);
            }
            Some(Slash::New) => {
                self.start_fresh_session((!arg.is_empty()).then_some(arg), false);
            }
            // codex `/resume` 는 인자를 받지 않는다 — 언제나 피커를 띄운다
            // (`slash_command.rs::Resume`, `resume_picker.rs`). 인자로 온 낱말은
            // 목록의 첫 필터가 아니라(우리는 검색이 없다) 바로 그 세션 id 로
            // 읽는 편이 쓸모 있으므로 그때만 곧장 갈아 끼운다.
            Some(Slash::Resume) => {
                if arg.is_empty() {
                    self.ui.open_resume_picker();
                } else {
                    self.resume_session(arg);
                }
            }
            Some(Slash::Permissions) => {
                if arg.is_empty() {
                    self.ui.open_permissions_picker();
                    return;
                }
                match crate::permission_mode::normalize_permission_mode(arg) {
                    Some(label) => {
                        let mode = crate::permission_mode::permission_mode_from_label(label);
                        self.session_mut().set_permission_mode(mode);
                        self.persist_or_warn();
                        self.ui.permission_mode = mode;
                        let text = format!("permissions → {label}");
                        self.ui.note(SystemLevel::Info, &text);
                    }
                    None => self.ui.note(
                        SystemLevel::Info,
                        "permissions: read-only · workspace-write · danger-full-access",
                    ),
                }
            }
            Some(Slash::Compact) => {
                let focus = (!arg.is_empty()).then_some(arg);
                let result = tokio::task::block_in_place(|| self.session_mut().compact(focus));
                match result {
                    Ok((0, kept)) => {
                        let text = format!("compact: nothing to compact ({kept} messages)");
                        self.ui.note(SystemLevel::Info, &text);
                    }
                    Ok((removed, kept)) => {
                        let text = format!("compact: {removed} removed · {kept} kept");
                        self.ui.note(SystemLevel::Info, &text);
                    }
                    Err(error) => {
                        let text = format!("compact failed: {error}");
                        self.ui.note(SystemLevel::Error, &text);
                    }
                }
            }
            Some(Slash::Goal) => self.apply_goal_command(arg),
            Some(Slash::Loop) => self.apply_loop_command(arg),
            Some(Slash::Clear) => {
                self.start_fresh_session((!arg.is_empty()).then_some(arg), true);
            }
            None => {
                let text =
                    format!("/{name} is not in zo — use zerocode IDE for it (? for shortcuts)");
                self.ui.note(SystemLevel::Info, &text);
            }
        }
    }

    /// 라이브 모델 전환. 세션 재빌드는 동기라 `block_in_place` 로 실행자에
    /// 알린다. 실패는 여기서 빨간 줄로 남기고, 성공 통지는 호출자가 한다 —
    /// 피커는 모델과 effort 를 한 줄로 함께 알리기 때문이다.
    fn switch_model(&mut self, model: &str) -> bool {
        let result = tokio::task::block_in_place(|| self.session_mut().set_model(model));
        match result {
            Ok(()) => {
                let model = self.session().model.clone();
                self.ui.sync_model_display(&model);
                // The board's pane badge is sticky: without this it keeps
                // showing whatever `SessionStart` said, forever.
                if let Some(reporter) = self.ui.reporter.as_ref() {
                    reporter.set_model(&model);
                }
                true
            }
            Err(error) => {
                let text = format!("model change failed: {error}");
                self.ui.note(SystemLevel::Error, &text);
                false
            }
        }
    }

    fn set_model(&mut self, model: &str) {
        if self.switch_model(model) {
            let text = format!("model → {}", self.ui.model);
            self.ui.note(SystemLevel::Info, &text);
        }
    }

    /// Apply the TUI fast toggle through the existing model/transport seam.
    /// `api::openai_fast_variant_pair` is the sole capability decision; this
    /// method only selects its requested member and refreshes the display.
    fn set_fast_mode(&mut self, enabled: bool) {
        let current = self.session().model.clone();
        let Some(target) = fast::target(&current, enabled) else {
            self.ui
                .note(SystemLevel::Info, fast::UNSUPPORTED_COMMAND_MESSAGE);
            return;
        };
        if !self.switch_model(&target) {
            return;
        }
        let service_tier = if enabled {
            fast::PRIORITY_REQUEST_VALUE
        } else {
            fast::DEFAULT_REQUEST_VALUE
        };
        self.ui.note(
            SystemLevel::Info,
            &format!("Service tier set to {service_tier}"),
        );
    }

    /// Apply one of the picker mappings and persist the session snapshot. The
    /// runtime's permission enforcer is intentionally left as the enforcement
    /// owner; TUI only changes its active mode through the existing setter.
    fn apply_permission_mode(&mut self, mode: PermissionMode) {
        self.session_mut().set_permission_mode(mode);
        self.persist_or_warn();
        self.ui.permission_mode = mode;
        let label = active_permission_label(mode);
        self.ui
            .note(SystemLevel::Info, &format!("Permissions updated to {label}"));
    }

    /// 턴 중에 눌린 것들을 턴 직후에 푼다 — 세션이 돌아온 지금이 그 자리다.
    fn drain_deferred(&mut self) {
        for work in std::mem::take(&mut self.ui.deferred) {
            match work {
                Deferred::Choice(model, effort) => self.apply_choice(&model, effort),
                Deferred::Fast(enabled) => self.set_fast_mode(enabled),
                Deferred::Permission(mode) => self.apply_permission_mode(mode),
                Deferred::Slash(command) => self.slash(&command),
                Deferred::Resume(id) => self.resume_session(&id),
            }
        }
    }

    /// 세션을 영속하고, 실패하면 **화면에 남긴다**.
    ///
    /// 네 자리(종료·`/permissions`·피커·`/resume`)가 `let _ =` 로 삼켰다 —
    /// UI 는 "Permissions updated" 를 보이는데 스냅샷은 저장되지 않아 다음
    /// `/resume` 에서 조용히 사라지고, 사람은 원인을 볼 길이 없었다.
    fn persist_or_warn(&mut self) {
        if let Some(session) = self.session.as_ref() {
            if let Err(error) = session.persist() {
                let text = format!("session not saved: {error}");
                self.ui.note(SystemLevel::Warn, &text);
            }
        }
    }

    fn apply_goal_command(&mut self, raw: &str) {
        let reporter = self.ui.reporter.clone();
        match self.session_mut().goal_command(raw) {
            Ok(result) => {
                self.ui.note(SystemLevel::Info, &result.notice);
                let event = if result.notice.starts_with("goal: running") {
                    Some("GoalStarted")
                } else if result.notice.contains("paused") || result.notice.contains("stopped") {
                    Some("GoalPaused")
                } else if result.notice.contains("completed") {
                    Some("GoalAchieved")
                } else {
                    None
                };
                if let (Some(reporter), Some(event)) = (reporter.as_ref(), event) {
                    reporter.autonomy_goal(event, &self.ui.session_id, &result.notice);
                }
            }
            Err(error) => self
                .ui
                .note(SystemLevel::Error, &format!("goal command failed: {error}")),
        }
        self.sync_goal_display();
    }

    fn apply_loop_command(&mut self, raw: &str) {
        match self.session_mut().loop_command(raw) {
            Ok(notice) => self.ui.note(SystemLevel::Info, &notice),
            Err(error) => self
                .ui
                .note(SystemLevel::Error, &format!("loop command failed: {error}")),
        }
        self.sync_goal_display();
    }

    /// `/goal` mutation, autonomous progress, `/resume`, `/new`, and `/clear`
    /// all refresh these two cached rows so a mid-turn `/status` never needs to
    /// borrow the session.
    fn sync_goal_display(&mut self) {
        let status = self.session().status();
        if let Some(channel) = events::channel() {
            channel.publish_status(&status, &self.session().cwd);
        }
        self.ui.goal = status.goal;
        self.ui.autonomous = status.autonomous;
        self.ui.loops = status.loops;
    }

    fn switch_effort(&mut self, effort: Effort) {
        // 선택은 즉시 선다. 저장이 실패하면 그 사실만 알린다 — 다음 세션
        // 기본값이 조용히 옛 값으로 돌아가는 편이 더 나쁘다.
        if let Err(error) = self.session_mut().set_effort(effort) {
            let text = format!("effort not saved for the next session: {error}");
            self.ui.note(SystemLevel::Warn, &text);
        }
        self.ui.effort = effort.canonical().to_string();
    }

    /// 피커가 확정한 모델 + effort 를 **엔진에** 세운다. effort 를 먼저 세운다 —
    /// `set_model` 이 런타임을 다시 세우면서 그때의 effort 로 thinking 설정을
    /// 굽기 때문이다.
    ///
    /// 성공하면 아무것도 찍지 않는다: codex `model_popups.rs` 의 확정 액션은
    /// `UpdateModel`/`UpdateReasoningEffort` 만 보내고 히스토리에는 아무 셀도
    /// 넣지 않는다. 화면 값은 이미 [`Ui::show_choice`] 가 세웠으므로, 실패한
    /// 때만 `switch_model` 의 빨간 줄 뒤로 세션의 진짜 값을 되읽어 되돌린다.
    fn apply_choice(&mut self, model: &str, effort: Effort) {
        // 유휴 피커도 턴 중 피커와 같은 즉시 화면 경로를 탄다. 턴 중에는 이미
        // `show_choice`가 불렸으므로 같은 선택은 이펙트를 다시 시작하지 않는다.
        self.ui.show_choice(model, effort);
        self.switch_effort(effort);
        if !self.switch_model(model) {
            self.resync_model_display();
        }
    }

    /// Start a fresh chat while keeping the previous one resumable. `/clear`
    /// takes the same road after purging both the visible screen and scrollback.
    fn start_fresh_session(&mut self, name: Option<&str>, clear_terminal: bool) {
        let launch: LaunchFlags = self.session().launch_flags();
        let current = self.session();
        let options = OpenOptions {
            person_model_pin: None,
            headless: false,
            cwd: current.cwd.clone(),
            model: current.model.clone(),
            permission_mode: current.permission_mode,
            effort: current.effort(),
            resume: None,
            allowed_tools: launch.allowed_tools,
            disable_spawn_family: launch.disable_spawn_family,
            mcp_config: launch.mcp_config,
            // A session the person opens from the TUI is their own choice,
            // not the launch contract's; the contract's receipt is immutable.
            exact_selection: false,
            teammate_harness: None,
            parent_registry: None,
            remote_mcp: None,
        };
        if clear_terminal {
            self.ui.pending_history.clear();
            self.ui.painter.clear_terminal();
        }
        let opened = tokio::task::block_in_place(|| PlainSession::open(options));
        match opened {
            Ok(mut session) => {
                let name_error = name.and_then(|name| session.set_name(name).err());
                let composer_session_id = session.handle.id.clone();
                let composer_seed = session.composer_history_seed();
                self.subagent_frame_relay = None;
                let previous_summary = self.session.take().map(|mut previous| {
                    let reason = if clear_terminal { "clear" } else { "new" };
                    previous.fire_session_end(reason);
                    let _ = previous.persist();
                    summary::session_summary(
                        self.ui.usage,
                        &previous.handle.id,
                        &previous.handle.path,
                        crate::dream::session_end_line(),
                    )
                });
                let agent_completion_pump = session.start_agent_completion_pump();
                self.session = Some(session);
                self.agent_completion_pump = agent_completion_pump;
                self.subagent_frame_relay = events::start_subagent_frame_relay(
                    events::channel(),
                    self.session().registry(),
                    composer_session_id.clone(),
                );
                self.ui.reset_conversation();
                self.ui
                    .composer
                    .bind_session(&composer_session_id, composer_seed);
                self.resync_model_display();
                self.sync_goal_display();
                let cwd_path = self.session().cwd.clone();
                let cwd = view::short_cwd(&cwd_path.to_string_lossy());
                let context = worktree_context(&cwd_path);
                self.ui.footer_location = view::footer_location(&cwd, context.as_deref());
                self.ui.cwd = cwd;
                self.ui.session_cwd = cwd_path;
                self.ui.session_id = self.session().handle.id.clone();
                self.ui.registry = self.session().registry();
                self.session_card();
                if let Some(Some(summary)) = previous_summary {
                    let width = self.ui.width();
                    let cell = cells::plain_cell(&summary.history_lines(), width);
                    self.ui.history(&cell);
                }
                if let Some(error) = name_error {
                    let text = format!("Failed to name the new session: {error}");
                    self.ui.note(SystemLevel::Error, &text);
                }
                self.session().fire_session_start();
            }
            Err(error) => {
                let text = format!("new chat failed: {error}");
                self.ui.note(SystemLevel::Error, &text);
            }
        }
    }

    /// 고른 세션으로 갈아 끼운다 — codex 의 `/resume` 확정이 하는 일이다.
    ///
    /// 순서도 codex 그대로다(`app/session_lifecycle.rs::resume_target_session`):
    /// 새 세션을 붙이고 → 세션 정보 셀(부팅 카드)을 히스토리에 커밋하고 →
    /// 이전 턴을 재생한다([`Self::replay_history`]). 옛 라운드의
    /// `resumed session <id>` 한 줄은 **codex 에 없으므로** 뺐다 — 재개를
    /// 알리는 것은 그 카드다.
    ///
    /// 런치 플래그는 지금 도는 세션에게 묻는다
    /// (`PlainSession::launch_flags`) — 5라운드가 argv 를 한 번 더 파싱했던
    /// 자리다.
    fn resume_session(&mut self, id: &str) {
        let launch: LaunchFlags = self.session().launch_flags();
        let current = self.session();
        let options = OpenOptions {
            person_model_pin: None,
            // A session opened FROM the TUI has a human at the keyboard by
            // construction.
            headless: false,
            cwd: current.cwd.clone(),
            model: current.model.clone(),
            permission_mode: current.permission_mode,
            effort: current.effort(),
            resume: Some(id.to_string()),
            // 이 셋만 런치에서 온다 — 나머지는 지금 도는 세션이 진실이다.
            allowed_tools: launch.allowed_tools,
            disable_spawn_family: launch.disable_spawn_family,
            mcp_config: launch.mcp_config,
            exact_selection: false,
            teammate_harness: None,
            parent_registry: None,
            remote_mcp: None,
        };
        let opened = tokio::task::block_in_place(|| PlainSession::open(options));
        match opened {
            Ok(mut session) => {
                let composer_seed = session.composer_history_seed();
                let composer_session_id = session.handle.id.clone();
                // The old roster must not publish into the new session's pane
                // while the two session lifecycles cross at `/resume`.
                self.subagent_frame_relay = None;
                // 옛 세션은 끝났음을 알리고 저장한 뒤 놓는다. 요약은 저장 뒤에
                // 뜬다 — 재개 힌트가 트랜스크립트의 실재를 보기 때문이다.
                let mut previous_summary = None;
                if let Some(mut previous) = self.session.take() {
                    previous.fire_session_end(hook_reason(ExitReason::UserExit));
                    let _ = previous.persist();
                    previous_summary = summary::session_summary(
                        self.ui.usage,
                        &previous.handle.id,
                        &previous.handle.path,
                        crate::dream::session_end_line(),
                    );
                }
                // `PlainSession::open` registered the new process-global
                // sender already. Start consuming it only after the old
                // session retired its marks, so a queued late completion
                // cannot cross the `/resume` boundary.
                let agent_completion_pump = session.start_agent_completion_pump();
                self.session = Some(session);
                self.agent_completion_pump = agent_completion_pump;
                self.subagent_frame_relay = events::start_subagent_frame_relay(
                    events::channel(),
                    self.session().registry(),
                    composer_session_id.clone(),
                );
                self.ui.reset_conversation();
                self.ui
                    .composer
                    .bind_session(&composer_session_id, composer_seed);
                self.resync_model_display();
                self.sync_goal_display();
                let cwd_path = self.session().cwd.clone();
                let cwd = view::short_cwd(&cwd_path.to_string_lossy());
                let context = worktree_context(&cwd_path);
                self.ui.footer_location = view::footer_location(&cwd, context.as_deref());
                self.ui.cwd = cwd;
                self.ui.session_cwd = cwd_path;
                self.ui.session_id = self.session().handle.id.clone();
                self.ui.registry = self.session().registry();
                self.session_card();
                self.replay_history();
                // 옛 세션의 요약은 **재생 뒤**에 선다 — codex
                // `resume_target_session` 도 새 스레드를 붙이고 재생까지 마친
                // 다음에야 `add_plain_history_lines` 로 민다.
                if let Some(summary) = previous_summary {
                    let width = self.ui.width();
                    let cell = cells::plain_cell(&summary.history_lines(), width);
                    self.ui.history(&cell);
                }
                self.session().fire_session_start();
            }
            Err(error) => {
                let text = format!("resume failed: {error}");
                self.ui.note(SystemLevel::Error, &text);
            }
        }
    }

    /// 화면의 모델·effort 를 세션의 진짜 값으로 되읽는다 — 즉시 반영이
    /// 앞서 나갔다가 엔진 교체가 실패한 자리를 메운다.
    fn resync_model_display(&mut self) {
        let session_model = self.session().model.clone();
        self.ui.sync_model_display(&session_model);
        if let Some(effort) = self.session().effort() {
            let canonical = effort.canonical();
            if self.ui.effort != canonical {
                self.ui.effort_effect = None;
            }
            self.ui.effort = canonical.to_string();
            self.ui.effort_tier = EffortTier::from_effort(effort);
        }
        self.ui.permission_mode = self.session().permission_mode;
    }

    /// Run one submitted turn and any follow-ups which become runnable at its
    /// boundary. Codex starts exactly one queued message per completed turn;
    /// the loop repeats only after that next turn also completes.
    async fn run_turn_chain(
        &mut self,
        initial: Submission,
        events: &mut EventStream,
        initial_followup: Option<AgentFollowup>,
    ) {
        let mut next_turn = Some((initial, initial_followup));
        while let Some((input, followup)) = next_turn.take() {
            let completion = Box::pin(self.turn(&input, events, None, followup.as_ref())).await;
            if self.ui.exit.is_some() || self.session.is_none() {
                break;
            }

            if let Some(followup) = self
                .agent_completion_pump
                .as_mut()
                .and_then(AgentCompletionPump::try_recv_followup)
            {
                next_turn = Some((
                    Submission {
                        text: followup.text.clone(),
                        image_paths: Vec::new(),
                    },
                    Some(followup),
                ));
                continue;
            }

            let mut pending = self.next_input_after_turn(completion.cancelled);
            while let Some(submission) = pending {
                if let Some(prompt) = self.submit(&submission) {
                    next_turn = Some((prompt, None));
                    break;
                }
                // A queued slash command may finish without starting a model
                // turn. Keep draining until one starts or the queue is empty.
                pending = self.ui.pending_input.pop_next_turn();
            }
        }
    }

    /// Decide what starts after a turn boundary. A steer-specific Esc wins
    /// over ordinary queued follow-ups and becomes one merged fresh user turn.
    fn next_input_after_turn(&mut self, cancelled: bool) -> Option<Submission> {
        if let Some(steers) = self.ui.pending_input.take_interrupted_steers() {
            // Unlike Codex app-server, zo's runtime queue is host-owned and
            // survives cancellation. Remove only the UI steers being
            // resubmitted; unrelated IDE steering remains queued.
            self.remove_runtime_steers(&steers);
            self.ui.note(
                SystemLevel::Info,
                "Model interrupted to submit steer instructions.",
            );
            return Some(Submission {
                text: steers.join("\n\n"),
                image_paths: Vec::new(),
            });
        }

        // A plain interrupt restores control to the composer. In particular,
        // it must not unexpectedly auto-send a Tab-queued draft.
        if cancelled {
            return None;
        }
        let rejected = self.ui.pending_input.reject_unacknowledged_steers();
        self.remove_runtime_steers(&rejected);
        self.ui.pending_input.pop_next_turn()
    }

    fn remove_runtime_steers(&self, steers: &[String]) {
        let Some(queue) = self.session().steering_handle() else {
            return;
        };
        let mut queue = queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for steer in steers {
            if let Some(index) = queue.iter().position(|queued| queued == steer) {
                queue.remove(index);
            }
        }
    }

    /// Every due `/goal` and `/loop` turn, through the driver both frontends
    /// share (`autonomy::driver`). There is no idle timer: when nothing is
    /// armed this returns at once and the 10-second idle draw contract stays.
    async fn drive_autonomy(&mut self, events: &mut EventStream) {
        let mut front = TuiFrontend { app: self, events };
        driver::drive(&mut front).await;
    }

    /// 한 턴. 세션과 화면은 서로 다른 필드라 turn future 가 도는 동안에도
    /// shimmer 프레임을 그릴 수 있다.
    #[allow(clippy::too_many_lines)] // 한 턴의 생애 — 준비·루프·정리가 한 자리에.
    async fn turn(
        &mut self,
        input: &Submission,
        events: &mut EventStream,
        autonomous_allow_writes: Option<bool>,
        agent_followup: Option<&AgentFollowup>,
    ) -> TurnOutcome {
        let images = match load_submission_images(&input.image_paths) {
            Ok(images) => images,
            Err(error) => {
                self.ui.note(SystemLevel::Error, &error);
                return TurnOutcome {
                    summary: None,
                    error: Some(error),
                    permission_blocked: false,
                    question_blocked: false,
                    cancelled: false,
                };
            }
        };
        let mut session = self
            .session
            .take()
            .expect("the session is only away while a turn runs");
        let saved_permission = autonomous_allow_writes
            .map(|allow_writes| session.begin_autonomous_turn(allow_writes));
        let ui = &mut self.ui;
        // 턴이 시작되는 순간의 크기로 접는다 — 유휴 폴 사이에 판이 바뀌었을 수 있다.
        ui.reconcile_size();
        let session_id = session.handle.id.clone();
        let registry = session.registry();
        let transcript = session.handle.path.to_string_lossy().into_owned();
        if agent_followup.is_none() {
            if let Some(reporter) = ui.reporter.as_ref() {
                reporter.user_prompt_submit(&input.text, &session_id);
            }
        }
        let mut last_assistant = String::new();
        // 턴의 배선 한 벌 — 블록 채널·권한 펌프·질문 채널·취소 신호·스티어 큐,
        // 그리고 IDE 채널의 턴 경계까지 [`TurnScaffold`] 가 든다. 여기 남는 것은
        // 화면 처리뿐이다.
        let (mut turn_scaffold, mut block_rx) = TurnScaffold::start(&mut session);
        if let Some(followup) = agent_followup {
            let agent_result = followup.render_block(&turn_scaffold.ids);
            let _ = turn_scaffold.publish(&agent_result);
            ui.block(agent_result);
        }
        if let Some(inbox) = session.agent_notification_inbox() {
            if let Some(agent_completion_pump) = self.agent_completion_pump.as_ref() {
                agent_completion_pump.begin_turn(inbox);
            }
        }
        ui.paint_probe = super::paint_probe::begin();
        let started = Instant::now();
        ui.had_work_activity = false;
        ui.turn_agent_count = 0;
        ui.committed_tool_calls.clear();
        ui.tool_status_detail = None;
        ui.subagent_wave = SubagentWave::default();
        ui.published_activity = PublishedActivity::Never;
        ui.set_subagent_progress(Vec::new());
        ui.status = Some(Status::working(Duration::ZERO));
        ui.publish_working_activity(turn_scaffold.ide);
        ui.draw_with_queue(|| block_rx.len());

        // 턴은 **다른 task** 에서 돈다([`TurnLaunch::spawn`]). 같은 select! 안에서
        // 폴링하면 턴 준비의 동기 구간(프롬프트 조립·클라이언트 빌드·훅)이 이
        // task 를 통째로 붙잡아 그동안 틱도 키도 못 돈다 — 실측으로 제출 직후
        // 1.8초 동안 프레임이 한 장도 안 나갔다(물결 정지, esc 도 늦게 먹힘).
        // 옮겨 놓으면 아래 `ticker` 가 제출 순간부터 32ms 마다 그리고, Esc 는 그
        // 자리에서 [`TurnScaffold::cancel_turn`] 을 부른다.
        // Start the watcher before the turn task: its timestamp is the turn
        // ownership boundary, so agents still running from the previous turn
        // cannot be relabelled as children of this one.
        let mut subagent_watcher =
            SubagentProgressWatcher::start(std::sync::Arc::clone(&registry), session_id.clone());
        let mut subagent_watcher_open = true;
        let mut turn = turn_scaffold
            .launch()
            .spawn(session, input.text.clone(), images);
        let mut ticker = tokio::time::interval(FRAME_TICK);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut commit_ticker = tokio::time::interval(COMMIT_TICK);
        commit_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut size_poll = tokio::time::interval(SIZE_POLL);
        size_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut exit_after = false;
        // The terminal is gone — its reads fail, or the stream ended. The
        // turn is cancelled and the session ends after it; the stream is not
        // polled again, because a stream that has failed fails at once,
        // forever, and polling it in this loop was a core spun at 100% for
        // as long as the tool it was waiting on took (a hung `cargo test`
        // whose interactive zsh had taken the pane's foreground, 2026-09-02).
        let mut input_lost = false;
        let mut permission_blocked = false;
        let mut question_blocked = false;

        let joined = loop {
            // select! 의 팔이 `ui` 를 빌리지 않도록, 기다릴 프롬프트는 Copy 값으로 뽑는다.
            let waiting = ui.parked.as_ref().and_then(|parked| {
                let kind = match &parked.prompt {
                    PendingPrompt::Permission(_) => PromptKind::Permission,
                    PendingPrompt::Question(_) => PromptKind::Question,
                };
                parked.prompt_id.map(|id| (id, kind))
            });
            tokio::select! {
                biased;
                result = &mut turn => break result,
                Some(block) = block_rx.recv() => {
                    let is_permission = matches!(&block, RenderBlock::PermissionPrompt(_));
                    let is_question = matches!(&block, RenderBlock::UserQuestionPrompt(_));
                    ui.observe(&block, &session_id, &mut last_assistant);
                    let published = turn_scaffold.publish(&block);
                    ui.block(block);
                    ui.tag_parked(published);
                    ui.publish_working_activity(turn_scaffold.ide);
                    if autonomous_allow_writes.is_some() && (is_permission || is_question) {
                        permission_blocked |= is_permission;
                        question_blocked |= is_question;
                        if let Some(parked) = ui.parked.take() {
                            events::retire_prompt(parked.prompt_id, ResolvedBy::Dismissed);
                            parked.prompt.dismiss();
                        }
                        turn_scaffold.cancel_turn();
                        ui.note(
                            SystemLevel::Warn,
                            if is_permission {
                                "autonomous turn stopped: approval requires a person"
                            } else {
                                "autonomous turn stopped: a question requires a person"
                            },
                        );
                    }
                    ui.draw_with_queue(|| block_rx.len());
                }
                progress = subagent_watcher.changed(), if subagent_watcher_open => {
                    if let Some(progress) = progress {
                        ui.set_subagent_progress(progress);
                        ui.publish_working_activity(turn_scaffold.ide);
                        ui.draw_with_queue(|| block_rx.len());
                    } else {
                        subagent_watcher_open = false;
                    }
                }
                Some(answer) = events::wait_answer(turn_scaffold.ide, waiting) => {
                    // IDE 모달이 먼저 답했다 — 패인의 다이얼로그를 그 답으로 닫는다.
                    if let Some(parked) = ui.parked.take() {
                        let resolved = events::resolve_pending(parked.prompt, &answer);
                        events::retire_prompt(parked.prompt_id, if resolved { ResolvedBy::Ide } else { ResolvedBy::Dismissed });
                        ui.note(SystemLevel::Info, "answered in the IDE");
                        ui.draw_with_queue(|| block_rx.len());
                    }
                }
                command = events::next_command(turn_scaffold.ide) => match command {
                    Command::CancelTurn { .. } => {
                        // IDE 의 Stop — Esc 와 같은 길.
                        if let Some(parked) = ui.parked.take() {
                            events::retire_prompt(parked.prompt_id, ResolvedBy::Dismissed);
                            parked.prompt.dismiss();
                        }
                        turn_scaffold.cancel_turn();
                        ui.draw_with_queue(|| block_rx.len());
                    }
                    Command::Steer { text } => {
                        let _ = turn_scaffold.steer(text);
                    }
                    Command::AccountSwitch { label } => {
                        ui.note(
                            SystemLevel::Info,
                            &crate::status_format::account_switch_notice(&label),
                        );
                        ui.draw_with_queue(|| block_rx.len());
                    }
                    // The parent closes this teammate mid-turn (t-2513 §2.2):
                    // the turn ends the way Stop ends it, and the reason is
                    // kept for the closing document the teammate loop writes.
                    Command::Close { reason } => {
                        if let Some(parked) = ui.parked.take() {
                            events::retire_prompt(parked.prompt_id, ResolvedBy::Dismissed);
                            parked.prompt.dismiss();
                        }
                        self.close_requested = Some(reason);
                        turn_scaffold.cancel_turn();
                        ui.draw_with_queue(|| block_rx.len());
                    }
                },
                event = events.next(), if !input_lost => {
                    if let Some(Ok(event)) = event {
                        if ui.turn_event(&event, &turn_scaffold, &mut exit_after) {
                            ui.draw_with_queue(|| block_rx.len());
                        }
                    } else {
                        input_lost = true;
                        turn_scaffold.cancel_turn();
                        exit_after = true;
                    }
                }
                () = TerminationSignals::delivered(&mut self.signals) => {
                    // The same road as Esc followed by exit: the turn is
                    // cancelled now and the session ends once it has folded.
                    turn_scaffold.cancel_turn();
                    exit_after = true;
                }
                // 파킹된 프롬프트처럼 프레임이 멈춘 구간에서도 크기는 따라간다.
                _ = size_poll.tick() => if ui.tend_terminal() { ui.draw_with_queue(|| block_rx.len()); },
                _ = commit_ticker.tick(), if ui.has_streaming_drain() => {
                    ui.draw_with_queue(|| block_rx.len());
                }
                _ = ticker.tick() => {
                    if let Some(status) = ui.status.as_mut() {
                        status.elapsed = started.elapsed();
                    }
                    ui.publish_working_activity(turn_scaffold.ide);
                    ui.draw_with_queue(|| block_rx.len());
                }
            }
        };

        // 턴 task 가 패닉했다면 세션은 돌아오지 않는다. 다음 슬래시 하나에
        // 또 터지느니 이 자리에서 알리고 나간다.
        let mut lost_session = false;
        let outcome = match joined {
            Ok((session, outcome)) => {
                self.session = Some(session);
                outcome
            }
            Err(error) => {
                lost_session = true;
                Err(format!("turn task ended: {error}"))
            }
        };
        if let Some(agent_completion_pump) = self.agent_completion_pump.as_ref() {
            agent_completion_pump.finish_turn();
        }
        let ui = &mut self.ui;

        // 턴 task 가 sender 를 놓았으니 남은 블록을 비운다 — 훅 보고와 IDE
        // 채널을 거쳐서다([`TurnScaffold::drain`]).
        turn_scaffold.drain(&mut block_rx, |block| {
            ui.observe(&block, &session_id, &mut last_assistant);
            ui.block(block);
        });
        ui.close_segment();
        // 아직 열린 도구 셀은 여기서 접힌다 — 도는 것이 남아 있으면 codex 의
        // `mark_failed` 로 닫고 커밋한다.
        ui.retire_tool_cell();
        if let Some(parked) = ui.parked.take() {
            parked.prompt.dismiss();
        }
        let cancelled = turn_scaffold.cancelled();
        ui.status = None;
        ui.publish_working_activity(turn_scaffold.ide);
        turn_scaffold.finish(&outcome);
        ui.tool_status_detail = None;
        // Keep the watcher's last owned snapshot for an Alt+A viewer that is
        // still open as the foreground turn ends. The next explicit open does
        // an on-demand session scan, and the next turn clears before polling.
        ui.tools.clear();
        ui.unseated.clear();
        if let Err(error) = &outcome {
            // 사람이 Esc 로 끊은 턴은 실패가 아니다 — 취소 경로가 남기는
            // "receiver dropped" 를 빨간 줄로 올리면 인터럽트가 사고처럼 보인다.
            if !cancelled {
                let text = format!("turn ended: {error}");
                ui.note(SystemLevel::Error, &text);
            }
        }
        if ui.had_work_activity {
            let separator = turn_separator(
                ui.width(),
                started.elapsed().as_secs(),
                ui.turn_agent_count,
            );
            // A cell of its own, so it carries the gap every cell carries:
            // drawn flush under the answer's last line it read as part of
            // the answer ("이거 붙는거") rather than as the turn's edge.
            ui.history(&[Line::empty(), separator]);
            ui.had_work_activity = false;
        }
        ui.turn_agent_count = 0;
        if let Some(reporter) = ui.reporter.as_ref() {
            reporter
                .stop(
                    &last_assistant,
                    &session_id,
                    &transcript,
                    &crate::session::subagent_progress::background_tasks_for_session(
                        &ui.registry,
                        &session_id,
                    ),
                )
                .await;
        }
        if exit_after || lost_session {
            ui.exit = Some(ExitReason::UserExit);
        }
        if let (Some(saved), Some(session)) = (saved_permission, self.session.as_mut()) {
            session.finish_autonomous_turn(saved);
        }
        if !lost_session {
            self.drain_deferred();
        }
        let completion = TurnOutcome {
            summary: outcome.as_ref().ok().cloned(),
            error: outcome.err(),
            permission_blocked,
            question_blocked,
            cancelled,
        };
        self.ui.draw_with_queue(|| block_rx.len());
        super::paint_probe::end(self.ui.paint_probe.take());
        completion
    }
}

/// 하위 에이전트 보고 셀의 호출 id — 묶임 판정에 안 쓰이는 자리표다.
impl Ui {
    /// 방금 파킹된 프롬프트에 채널의 `prompt_id` 를 얹는다(이미 있으면 그대로).
    fn tag_parked(&mut self, published: Option<u64>) {
        if let (Some(parked), Some(id)) = (self.parked.as_mut(), published) {
            if parked.prompt_id.is_none() {
                parked.prompt_id = Some(id);
            }
        }
    }
}

const AGENT_RESULT_ID: &str = "agent-result";

/// 피커 2단계의 설명 문안 — forge `effort_picker.rs::description` 기준.
///
/// `smart` 만 원문과 다르다. 원문 꼬리는 `+ parallel agent orchestration` 인데
/// zo-ide 는 spawn 계열 도구를 끄고 병렬 오케스트레이션을 `ZeroCode` IDE 에
/// 맡기므로 그 약속을 화면에 쓸 수 없다. 대신 사용자가 부른 옛 이름
/// `ultracode` 를 병기해 이 줄에서 찾을 수 있게 한다
/// (`Effort::from_token` 은 `ultracode`/`smartcode`/`uc` 를 이미 Smart 로 받는다).
const fn effort_description(effort: Effort) -> &'static str {
    match effort {
        Effort::Smart => "dynamic top band, xhigh up to ceiling (ultracode)",
        other => other.description(),
    }
}

/// 결과 diff 에 경로가 없을 때 쓸 폴백 — preview 가 들고 있는 대상 파일.
fn preview_path(preview: &ToolPreview) -> String {
    match preview {
        ToolPreview::Write { path, .. }
        | ToolPreview::Edit { path, .. }
        | ToolPreview::Read { path, .. } => path.clone(),
        _ => String::new(),
    }
}

const fn hook_reason(reason: ExitReason) -> &'static str {
    match reason {
        ExitReason::UserExit => "exit",
        ExitReason::OutputClosed => "output_closed",
        ExitReason::LoopCompleted => "loop_completed",
        ExitReason::AutonomousLimit => "autonomy_limit",
        ExitReason::Interrupted => "interrupted",
    }
}

/// A text cell whose first bytes spell the passport label — arriving in
/// pieces, as a stream does — shows the reply without the label; a cell that
/// merely starts with a bracket is shown whole, and the hold never delays
/// more than the label's own length.
#[test]
fn an_imitated_passport_label_never_reaches_the_screen() {
    let mut ui = test_ui();
    let ids = BlockIdGen::default();
    let id = ids.next();
    for (text, done) in [("[earlier", false), (" reasoning]\n\n", false), ("Hello there", true)] {
        ui.block(RenderBlock::TextDelta {
            id,
            text: text.to_string(),
            done,
        });
    }
    ui.flush_history();
    let frame = ui.painter.take_frame();
    assert!(!frame.contains("earlier reasoning"), "the label leaked: {frame:?}");
    assert!(frame.contains("Hello there"), "the reply was lost: {frame:?}");
    assert_eq!(ui.last_answer, "Hello there");

    let id = ids.next();
    ui.block(RenderBlock::TextDelta {
        id,
        text: "[note] kept".to_string(),
        done: true,
    });
    ui.flush_history();
    let frame = ui.painter.take_frame();
    assert!(frame.contains("[note] kept"), "a bracket that is not the label was held back: {frame:?}");
}

#[test]
fn five_quiet_candidates_collapse_before_scrollback() {
    let mut ui = test_ui();
    for count in 1..=5 {
        ui.begin_quiet_candidate();
        ui.history(&[Line::from_text(format!("discard iteration {count}"))]);
        ui.finish_quiet_candidate("loop-1", true, count, 0);
    }

    assert!(ui.pending_history.is_empty());
    let quiet = ui.quiet_loop_cell.as_ref().expect("one live quiet cell");
    assert_eq!(quiet.lines.len(), 1);
    assert!(quiet.lines[0].plain().contains("quiet ×5"));

    ui.history(&[Line::from_text("next real cell")]);
    let plain: Vec<String> = ui.pending_history.iter().map(Line::plain).collect();
    assert_eq!(plain.len(), 3);
    assert_eq!(plain[0], "");
    assert!(plain[1].contains("quiet ×5"));
    assert_eq!(plain[2], "next real cell");
}

/// The termination signals a session listens for, installed once at start.
///
/// Absent on platforms without them, and absent when tokio cannot register
/// them (a sandbox that forbids signal handlers): then the arms that await
/// them never fire, and the session ends the ways it always could.
struct TerminationSignals {
    #[cfg(unix)]
    terminate: tokio::signal::unix::Signal,
    #[cfg(unix)]
    hangup: tokio::signal::unix::Signal,
}

impl TerminationSignals {
    fn install() -> Option<Self> {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            let terminate = signal(SignalKind::terminate()).ok()?;
            let hangup = signal(SignalKind::hangup()).ok()?;
            Some(Self { terminate, hangup })
        }
        #[cfg(not(unix))]
        {
            None
        }
    }

    /// Resolves when either signal arrives; pends forever when none are
    /// installed, so a `select!` arm on it costs nothing.
    async fn delivered(signals: &mut Option<Self>) {
        match signals {
            #[cfg(unix)]
            Some(signals) => {
                tokio::select! {
                    _ = signals.terminate.recv() => {}
                    _ = signals.hangup.recv() => {}
                }
            }
            _ => std::future::pending::<()>().await,
        }
    }
}

#[cfg(test)]
fn test_ui() -> Ui {
    let mut painter = Painter::new(std::io::stdout(), 80, 24, 1, false);
    painter.set_height(1);
    painter.take_frame();
    Ui {
        flags: RenderFlags {
            show_thinking: true,
            ..RenderFlags::default()
        },
        painter,
        paint_probe: None,
        composer: Composer::new(),
        pending_input: PendingInputs::default(),
        segment: Segment::None,
        tools: HashMap::new(),
        unseated: Vec::new(),
        committed_tool_calls: HashSet::new(),
        tool_cell: None,
        quiet_loop_cell: None,
        quiet_candidate: None,
        had_work_activity: false,
        turn_agent_count: 0,
        folds: FoldIds::default(),
        fold_mode: FoldMode::Bare,
        last_commit: None,
        deferred: Vec::new(),
        pending_history: Vec::new(),
        passport_gate: PassportGate::default(),
        status: None,
        tool_status_detail: None,
        subagent_progress: Vec::new(),
        subagent_wave: SubagentWave::default(),
        published_activity: PublishedActivity::Never,
        reasoning_scan: None,
        parked: None,
        picker: None,
        sessions: None,
        permissions: None,
        agents: None,
        transcript: None,
        transcript_items: Vec::new(),
        transcript_answer: String::new(),
        last_answer: String::new(),
        transcript_bytes: 0,
        popup_selected: 0,
        reporter: None,
        model: "test-model".to_string(),
        fast: false,
        wire: None,
        effort: String::new(),
        effort_tier: None,
        effort_effect: None,
        osc_guard: None,
        ctx_tokens: 0,
        permission_mode: PermissionMode::WorkspaceWrite,
        permission_cell: None,
        cwd: "/test".to_string(),
        session_cwd: std::path::PathBuf::from("/test"),
        footer_location: "/test".to_string(),
        startup_quit_grace_until: None,
        last_interrupt: None,
        session_id: "test-session".to_string(),
        registry: tools::AgentRegistry::at_root_for_tests("session-test", std::path::Path::new("/tmp")),
        goal: String::new(),
        autonomous: String::new(),
        loops: String::new(),
        usage: TokenUsage::default(),
        usage_known: false,
        exit: None,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        entry_bytes, events, format_tool_result_from_raw, item_bytes, preview_summary,
        preview_tool_input, record_transcript_item, test_ui, transcript, BlockIdGen, RenderBlock,
        PublishedActivity, ReplayItem, ToolCallId, ToolCallStatus, AgentResultStatus, Line,
        DROPPED_BODY,
    };
    use serde_json::{Value, json};

    fn running_helper(
        id: &str,
        label: &str,
        activity: &str,
        tool_calls: u64,
        elapsed: u64,
    ) -> crate::session::subagent_progress::SubagentProgress {
        crate::session::subagent_progress::SubagentProgress {
            agent_id: id.to_string(),
            tool_call_id: Some("workflow-1".to_string()),
            label: label.to_string(),
            model: None,
            activity: activity.to_string(),
            recent_tools: vec![activity.to_string()],
            tool_calls,
            output_tail: String::new(),
            started_epoch: 100,
            elapsed: Duration::from_secs(elapsed),
            no_new_output_for: None,
            transcript_path: None,
            pane: None,
            last_receipt: None,
        }
    }

    #[test]
    fn a_workflow_wave_has_a_summary_and_one_row_per_running_helper() {
        let mut ui = test_ui();
        ui.status = Some(crate::tui::view::Status::working(Duration::from_secs(12)));
        ui.set_subagent_progress(vec![
            running_helper("agent-a", "scout", "Read · src/tui/view.rs", 3, 9),
            running_helper("agent-b", "reviewer", "Bash · cargo test", 1, 4),
        ]);

        let details = &ui.status.as_ref().expect("working status").details;
        assert_eq!(
            details,
            &[
                "agents 2 · running 2 · done 0",
                "scout · 3 tool uses · 9s · Read · tui/view.rs",
                "reviewer · 1 tool use · 4s · Bash · cargo",
            ]
        );
    }

    #[test]
    fn helper_elapsed_repaints_are_not_mistaken_for_tool_events() {
        use crate::autonomy::limits::DEFAULT_QUIET_AFTER_SECS;

        let mut ui = test_ui();
        ui.status = Some(crate::tui::view::Status::working(Duration::ZERO));
        let mut helper = running_helper("agent-a", "scout", "Read · src/lib.rs", 1, 0);
        ui.set_subagent_progress(vec![helper.clone()]);

        ui.status.as_mut().expect("working status").elapsed =
            Duration::from_secs(DEFAULT_QUIET_AFTER_SECS);
        helper.elapsed = Duration::from_secs(DEFAULT_QUIET_AFTER_SECS);
        ui.set_subagent_progress(vec![helper.clone()]);
        assert!(
            ui.status
                .as_ref()
                .expect("working status")
                .line(120)
                .plain()
                .contains("quiet 1m 00s")
        );

        helper.tool_calls += 1;
        helper.activity = "Grep · Status".to_string();
        ui.set_subagent_progress(vec![helper]);
        assert!(
            !ui.status
                .as_ref()
                .expect("working status")
                .line(120)
                .plain()
                .contains("quiet")
        );
    }

    #[test]
    fn working_tool_fact_maps_to_the_session_status_activity_shape() {
        let mut ui = test_ui();
        let mut status = crate::tui::view::Status::working(Duration::from_secs(3));
        status.note_tool_started(
            "toolu_read",
            "Read",
            Some("workspace/crates/zo-ide/src/tui/view.rs"),
        );
        status.elapsed = Duration::from_secs(10);
        ui.status = Some(status);

        ui.publish_working_activity(None);
        let PublishedActivity::Value(Some(activity)) = &ui.published_activity else {
            panic!("published activity missing");
        };
        assert_eq!(activity.verb, "read");
        assert_eq!(activity.target.as_deref(), Some("tui/view.rs"));
        assert_eq!(activity.phase, "started");
        assert_eq!(activity.elapsed_secs, 7);
    }

    // ---- hook road vs channel road (t-2550) ------------------------------
    //
    // The window reads a `PreToolUse` as claude's "the agent reached for a
    // tool": the pane turns Working and any parked approval or question is
    // dropped. So the hook road may carry REAL tool starts only, one per call
    // id, while seconds, waits, retries and quiet stretches ride the channel.

    fn ev(event: &str, tool_name: &str) -> (String, String) {
        (event.to_string(), tool_name.to_string())
    }

    /// The hook stream one screen sequence produced, as `(event, tool_name)`.
    fn hook_events(
        captured: &std::sync::Arc<std::sync::Mutex<Vec<crate::ide::reporter::HookEnvelope>>>,
    ) -> Vec<(String, String)> {
        captured
            .lock()
            .expect("hook capture")
            .iter()
            .map(|envelope| {
                ev(
                    &envelope.hook_event_name,
                    envelope.payload["tool_name"].as_str().unwrap_or_default(),
                )
            })
            .collect()
    }

    fn hooked_ui() -> (
        super::Ui,
        std::sync::Arc<std::sync::Mutex<Vec<crate::ide::reporter::HookEnvelope>>>,
    ) {
        let mut ui = test_ui();
        let (reporter, captured) = crate::ide::reporter::HookReporter::capturing();
        ui.reporter = Some(reporter);
        ui.status = Some(crate::tui::view::Status::working(Duration::ZERO));
        (ui, captured)
    }

    fn tool_call(ids: &BlockIdGen, call_id: &str, name: &str, input: &Value) -> RenderBlock {
        let preview = preview_tool_input(name, input);
        let summary = preview_summary(&preview);
        RenderBlock::ToolCall {
            id: ids.next(),
            tool_call_id: ToolCallId(call_id.to_string()),
            name: name.to_string(),
            summary,
            preview,
            status: ToolCallStatus::Running,
        }
    }

    fn permission_prompt(
        ids: &BlockIdGen,
        call_id: &str,
        tool_name: &str,
    ) -> (
        RenderBlock,
        tokio::sync::oneshot::Receiver<runtime::message_stream::PermissionDecision>,
    ) {
        use runtime::message_stream::types::{PermissionChoice, PermissionPrompt};
        use runtime::message_stream::PermissionDecision;

        let (responder, decision) = tokio::sync::oneshot::channel();
        let block = RenderBlock::PermissionPrompt(PermissionPrompt {
            id: ids.next(),
            tool_call_id: ToolCallId(call_id.to_string()),
            tool_name: tool_name.to_string(),
            reasoning: "writes outside the workspace".to_string(),
            audit_hint: None,
            choices: vec![
                PermissionChoice {
                    key: 'y',
                    label: "Allow once".to_string(),
                    decision: PermissionDecision::AllowOnce,
                },
                PermissionChoice {
                    key: 'n',
                    label: "Deny".to_string(),
                    decision: PermissionDecision::Deny,
                },
            ],
            responder,
        });
        (block, decision)
    }

    /// A block arriving the way the turn loop delivers one: observed for the
    /// hook road, consumed by the screen, then the channel fact refreshed.
    fn arrive(ui: &mut super::Ui, block: RenderBlock) {
        let mut last_assistant = String::new();
        ui.observe(&block, "session-a", &mut last_assistant);
        ui.block(block);
        ui.publish_working_activity(None);
    }

    /// The turn ticker landing on one elapsed second.
    fn tick(ui: &mut super::Ui, elapsed_secs: u64) {
        ui.status.as_mut().expect("working status").elapsed = Duration::from_secs(elapsed_secs);
        ui.publish_working_activity(None);
    }

    fn published_card(ui: &super::Ui) -> Option<events::ActivityCard> {
        match &ui.published_activity {
            PublishedActivity::Value(card) => card.clone(),
            PublishedActivity::Never => None,
        }
    }

    #[test]
    fn a_tool_start_hooks_once_per_call_id_not_per_target_or_repaint() {
        let (mut ui, captured) = hooked_ui();
        let ids = BlockIdGen::default();
        let read = json!({ "file_path": "workspace/crates/zo-ide/src/tui/view.rs" });

        arrive(&mut ui, tool_call(&ids, "toolu_1", "Read", &read));
        // The same call seen again — its status moved — is not a second start.
        arrive(&mut ui, tool_call(&ids, "toolu_1", "Read", &read));
        // A second real Read of the same file is a second call.
        arrive(&mut ui, tool_call(&ids, "toolu_2", "Read", &read));

        assert_eq!(
            hook_events(&captured),
            [ev("PreToolUse", "Read"), ev("PreToolUse", "Read")]
        );
        let posted = captured.lock().expect("hook capture");
        // The real tool name, the compact target the row shows, and the same
        // started fact the channel says — at second zero, where a start is.
        assert_eq!(posted[0].payload["tool_input"], "tui/view.rs");
        assert_eq!(
            posted[0].payload["activity"],
            json!({
                "verb": "read",
                "target": "tui/view.rs",
                "phase": "started",
                "elapsed_secs": 0
            })
        );
    }

    #[test]
    fn a_parked_permission_beside_a_running_tool_adds_no_tool_hooks() {
        let (mut ui, captured) = hooked_ui();
        let ids = BlockIdGen::default();

        arrive(
            &mut ui,
            tool_call(&ids, "toolu_bash", "Bash", &json!({ "command": "sleep 5 && printf done" })),
        );
        let (prompt, _decision) = permission_prompt(&ids, "toolu_edit", "Edit");
        arrive(&mut ui, prompt);
        assert!(ui.parked_dialog().is_some(), "the approval is parked");

        for second in 1..=5 {
            tick(&mut ui, second);
        }

        // The bridge heard one start and one request, then nothing: the pane
        // is a person's to answer, and a clock is not a tool reaching out.
        assert_eq!(
            hook_events(&captured),
            [ev("PreToolUse", "Bash"), ev("PermissionRequest", "Edit")]
        );
        // The seconds live on the channel fact, which kept counting.
        let card = published_card(&ui).expect("channel fact");
        assert_eq!(card.verb, "bash");
        assert_eq!(card.target.as_deref(), Some("sleep"));
        assert_eq!(card.elapsed_secs, 5);
        assert!(ui.parked_dialog().is_some(), "the approval is still parked");
    }

    #[test]
    fn quiet_while_a_permission_is_parked_is_a_channel_fact_not_a_tool_hook() {
        let (mut ui, captured) = hooked_ui();
        ui.status
            .as_mut()
            .expect("working status")
            .set_quiet_after(Duration::from_secs(1));
        let ids = BlockIdGen::default();
        let (prompt, _decision) = permission_prompt(&ids, "toolu_bash", "Bash");
        arrive(&mut ui, prompt);

        for second in 1..=3 {
            tick(&mut ui, second);
        }

        let card = published_card(&ui).expect("channel fact");
        assert_eq!(card.verb, crate::tui::strings::ACTIVITY_QUIET);
        assert_eq!(card.elapsed_secs, 3);
        // `quiet` is a status word, never a tool: the request stands alone.
        assert_eq!(hook_events(&captured), [ev("PermissionRequest", "Bash")]);
        assert!(ui.parked_dialog().is_some(), "the approval is still parked");
    }

    #[test]
    fn model_waits_and_retries_never_ride_the_tool_hook_road() {
        use crate::autonomy::limits::DEFAULT_STREAM_PHASE_AFTER_SECS;
        use runtime::message_stream::types::StreamPhase;

        let (mut ui, captured) = hooked_ui();
        ui.status
            .as_mut()
            .expect("working status")
            .note_stream_phase(StreamPhase::RequestSent { attempt: 1 });
        tick(&mut ui, DEFAULT_STREAM_PHASE_AFTER_SECS + 1);
        let waiting = published_card(&ui).expect("waiting fact");
        assert_eq!(waiting.verb, crate::tui::strings::ACTIVITY_WAITING);

        ui.status
            .as_mut()
            .expect("working status")
            .note_stream_phase(StreamPhase::Retrying { attempt: 2, delay_secs: 8 });
        // The same second again: the retry was noted at this elapsed, so the
        // whole delay is still ahead.
        tick(&mut ui, DEFAULT_STREAM_PHASE_AFTER_SECS + 1);
        let retrying = published_card(&ui).expect("reconnecting fact");
        assert_eq!(retrying.verb, crate::tui::strings::ACTIVITY_RECONNECTING);
        assert_eq!(retrying.target.as_deref(), Some("attempt 2 in 8s"));

        assert!(hook_events(&captured).is_empty(), "{:?}", hook_events(&captured));
    }

    #[test]
    fn a_question_hooks_its_tool_once_and_keeps_waiting_quietly() {
        use runtime::message_stream::{QuestionOption, UserQuestionPrompt};

        let (mut ui, captured) = hooked_ui();
        let ids = BlockIdGen::default();
        arrive(
            &mut ui,
            tool_call(
                &ids,
                "toolu_ask",
                "AskUserQuestion",
                &json!({ "questions": [{ "question": "Which auth method?" }] }),
            ),
        );
        let (responder, _answer) = tokio::sync::oneshot::channel();
        arrive(
            &mut ui,
            RenderBlock::UserQuestionPrompt(UserQuestionPrompt {
                id: ids.next(),
                question: "Which auth method?".to_string(),
                header: None,
                options: vec![QuestionOption::plain("OAuth"), QuestionOption::plain("API key")],
                multi_select: false,
                responder,
            }),
        );

        for second in 1..=3 {
            tick(&mut ui, second);
        }

        // One start for the tool the window reads as a question, one
        // notification with the question itself — and no repaint of either.
        assert_eq!(
            hook_events(&captured),
            [ev("PreToolUse", "AskUserQuestion"), ev("Notification", "")]
        );
        assert_eq!(
            captured.lock().expect("hook capture")[1].payload["message"],
            "question: Which auth method?"
        );
        let question = ui.parked_question().expect("the question is still parked");
        assert_eq!(question.question, "Which auth method?");
        assert!(question.rows.len() >= 2, "the choices survive the ticks");
    }

    #[test]
    fn a_finished_sibling_does_not_reannounce_the_tool_still_running() {
        use runtime::message_stream::types::ToolResultBody;

        let (mut ui, captured) = hooked_ui();
        let ids = BlockIdGen::default();
        arrive(
            &mut ui,
            tool_call(&ids, "toolu_read", "Read", &json!({ "file_path": "src/lib.rs" })),
        );
        arrive(
            &mut ui,
            tool_call(&ids, "toolu_bash", "Bash", &json!({ "command": "cargo test" })),
        );
        tick(&mut ui, 2);
        // The younger sibling finishes and the Working line falls back to the
        // Read — the same call it announced at second zero, not a new start.
        arrive(
            &mut ui,
            RenderBlock::ToolResult {
                id: ids.next(),
                tool_call_id: ToolCallId("toolu_bash".to_string()),
                is_error: false,
                body: ToolResultBody::Text {
                    content: "ok".to_string(),
                    truncated: false,
                },
            },
        );
        tick(&mut ui, 3);

        assert_eq!(
            hook_events(&captured),
            [
                ev("PreToolUse", "Read"),
                ev("PreToolUse", "Bash"),
                ev("PostToolUse", "Bash"),
            ]
        );
        let card = published_card(&ui).expect("channel fact");
        assert_eq!(card.verb, "read");
        assert_eq!(card.elapsed_secs, 3);
    }

    #[test]
    fn the_hook_stream_grows_with_tool_starts_not_with_seconds() {
        use runtime::message_stream::types::ToolResultBody;

        let (mut ui, captured) = hooked_ui();
        let ids = BlockIdGen::default();
        arrive(
            &mut ui,
            tool_call(&ids, "toolu_1", "Read", &json!({ "file_path": "src/lib.rs" })),
        );
        for second in 1..=120 {
            tick(&mut ui, second);
        }
        assert_eq!(hook_events(&captured), [ev("PreToolUse", "Read")]);
        assert_eq!(published_card(&ui).map(|card| card.elapsed_secs), Some(120));

        arrive(
            &mut ui,
            RenderBlock::ToolResult {
                id: ids.next(),
                tool_call_id: ToolCallId("toolu_1".to_string()),
                is_error: false,
                body: ToolResultBody::Text {
                    content: "pub mod tui;".to_string(),
                    truncated: false,
                },
            },
        );
        assert_eq!(
            hook_events(&captured),
            [ev("PreToolUse", "Read"), ev("PostToolUse", "Read")]
        );

        // The turn ends: the channel fact clears, and nothing is hooked for it.
        ui.status = None;
        ui.publish_working_activity(None);
        assert!(published_card(&ui).is_none());
        assert_eq!(hook_events(&captured).len(), 2);
    }

    #[tokio::test]
    // 채널 하나를 세워 로스터 변화를 끝까지 따라가는 한 편이라, 쪼개면 어느
    // 스냅샷이 어느 변화 뒤에 온 것인지 읽을 수 없다.
    #[allow(clippy::too_many_lines)]
    async fn tui_channel_emits_subagents_when_the_session_roster_changes() {
        use std::time::Duration;

        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        use tokio::net::TcpStream;
        use tokio::sync::watch;

        use crate::ide::events::{EventsChannel, EventsConfig};
        use crate::session::subagent_progress::{
            RosterSnapshot, SubagentProgress, SubagentProgressWatcher,
        };

        let channel = EventsChannel::open(&EventsConfig {
            bind: "127.0.0.1:0".to_string(),
            token: None,
            session_id: "tui-session".to_string(),
            addr_file: None,
            discovery_file: None,
        })
        .await
        .expect("open TUI events channel");
        let stream = TcpStream::connect(channel.local_addr())
            .await
            .expect("connect subscriber");
        let (read, mut write) = stream.into_split();
        write
            .write_all(
                br#"{"jsonrpc":"2.0","id":1,"method":"session.subscribe","params":{"id":"tui-session"}}
"#,
            )
            .await
            .expect("subscribe");
        let mut lines = BufReader::new(read).lines();
        let response = lines
            .next_line()
            .await
            .expect("read subscribe response")
            .expect("subscribe response");
        assert_eq!(
            serde_json::from_str::<Value>(&response)
                .expect("JSON-RPC response")
                .get("id"),
            Some(&json!(1)),
        );

        let (updates, receiver) = watch::channel(RosterSnapshot::default());
        let relay = events::start_subagent_frame_relay_with_watcher(
            Some(&channel),
            SubagentProgressWatcher::from_receiver(receiver),
            None,
        )
        .expect("TUI channel relay");
        updates
            .send(RosterSnapshot { generation: 0, agents: vec![SubagentProgress {
                agent_id: "agent-17".to_string(),
                tool_call_id: None,
                label: "verify TUI channel".to_string(),
                model: Some("gpt-5.6-sol".to_string()),
                activity: "working".to_string(),
                recent_tools: Vec::new(),
                tool_calls: 0,
                output_tail: String::new(),
                started_epoch: 123,
                elapsed: Duration::from_secs(7),
                no_new_output_for: None,
                transcript_path: None,
                pane: None,
                last_receipt: None,
            }]})
            .expect("roster change");

        let frame = tokio::time::timeout(Duration::from_secs(1), lines.next_line())
            .await
            .expect("subagents frame timeout")
            .expect("read subagents frame")
            .expect("subagents frame");
        assert_eq!(
            serde_json::from_str::<Value>(&frame).expect("subagents JSON"),
            json!({
                "type": "subagents",
                // The channel's own frame counter (t-2511 §3); the hand-fed
                // watcher has no registry, so no registry/generation marks.
                "seq": 1,
                "running": [{
                    "id": "agent-17",
                    "label": "verify TUI channel",
                    "model": "gpt-5.6-sol",
                    "activity": "working",
                    "tool_calls": 0,
                    "started_epoch": 123,
                    "transcript": null,
                    "pane": null,
                }],
            }),
        );

        drop(relay);
        let cleared = tokio::time::timeout(Duration::from_secs(1), lines.next_line())
            .await
            .expect("cleared roster timeout")
            .expect("read cleared roster")
            .expect("cleared roster");
        assert_eq!(
            serde_json::from_str::<Value>(&cleared).expect("cleared roster JSON"),
            json!({"type": "subagents", "running": [], "seq": 2}),
            "ending or replacing a TUI session must retire its old child rows",
        );
        tokio::time::timeout(Duration::from_secs(1), async {
            while !updates.is_closed() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("dropping the TUI session relay stops its watcher");
    }

    fn todo_input() -> Value {
        json!({
            "todos": [
                {
                    "content": "Wire it",
                    "activeForm": "Wiring it",
                    "status": "in_progress"
                },
                {
                    "content": "Verify it",
                    "activeForm": "Verifying it",
                    "status": "pending"
                }
            ]
        })
    }

    fn todo_result() -> String {
        json!({
            "oldTodos": [],
            "newTodos": todo_input()["todos"].clone(),
            "verificationNudgeNeeded": null
        })
        .to_string()
    }

    fn announce_todo(ui: &mut super::Ui, ids: &BlockIdGen, id: &str, wrapped: bool) {
        let (name, input) = if wrapped {
            (
                "CapabilityInvoke",
                json!({ "name": "TodoWrite", "input": todo_input() }),
            )
        } else {
            ("TodoWrite", todo_input())
        };
        let preview = preview_tool_input(name, &input);
        let summary = preview_summary(&preview);
        ui.block(RenderBlock::ToolCall {
            id: ids.next(),
            tool_call_id: ToolCallId(id.to_string()),
            name: name.to_string(),
            summary,
            preview,
            status: ToolCallStatus::Running,
        });
    }

    fn reason_between_call_and_result(ui: &mut super::Ui, ids: &BlockIdGen) {
        let id = ids.next();
        ui.block(RenderBlock::Reasoning {
            id,
            text: "**Checking the plan**".to_string(),
            signature: None,
            done: false,
        });
        ui.block(RenderBlock::Reasoning {
            id,
            text: String::new(),
            signature: None,
            done: true,
        });
    }

    fn settle_todo(ui: &mut super::Ui, ids: &BlockIdGen, id: &str, result_name: &str) {
        ui.block(RenderBlock::ToolResult {
            id: ids.next(),
            tool_call_id: ToolCallId(id.to_string()),
            is_error: false,
            body: format_tool_result_from_raw(result_name, &todo_result(), false),
        });
    }

    fn finish_tool_frame(ui: &mut super::Ui) -> String {
        ui.close_segment();
        ui.retire_tool_cell();
        // What a frame would do next: the lines handed over go to the painter.
        ui.flush_history();
        ui.painter.take_frame()
    }

    #[test]
    fn typing_while_an_option_is_focused_jumps_to_freeform_input() {
        use crossterm::event::{KeyCode, KeyEvent};
        use runtime::message_stream::{BlockId, QuestionOption, UserQuestionPrompt};

        let mut ui = test_ui();
        let (responder, _response) = tokio::sync::oneshot::channel();
        ui.park(super::PendingPrompt::Question(UserQuestionPrompt {
            id: BlockId(1),
            question: "Which auth method?".to_string(),
            header: None,
            options: vec![QuestionOption::plain("OAuth"), QuestionOption::plain("API key")],
            multi_select: false,
            responder,
        }));

        ui.question_key(KeyEvent::from(KeyCode::Char('x')));

        let question = ui.parked_question().expect("question stays parked");
        assert_eq!(question.selected, question.rows.len() - 1);
        assert_eq!(ui.composer.text(), "x");
    }

    #[test]
    fn selecting_other_opens_the_inline_composer_without_submitting() {
        use crossterm::event::{KeyCode, KeyEvent};
        use runtime::message_stream::{BlockId, QuestionOption, UserQuestionPrompt};

        let mut ui = test_ui();
        let (responder, mut response) = tokio::sync::oneshot::channel();
        ui.park(super::PendingPrompt::Question(UserQuestionPrompt {
            id: BlockId(2),
            question: "Which auth method?".to_string(),
            header: None,
            options: vec![QuestionOption::plain("OAuth"), QuestionOption::plain("API key")],
            multi_select: false,
            responder,
        }));

        ui.question_key(KeyEvent::from(KeyCode::Down));
        ui.question_key(KeyEvent::from(KeyCode::Down));
        ui.question_key(KeyEvent::from(KeyCode::Enter));

        assert!(
            ui.parked_question()
                .is_some_and(super::view::Question::is_other_input)
        );
        assert!(
            matches!(
                response.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            ),
            "opening Other must not answer the prompt"
        );
    }

    #[test]
    fn single_select_other_submission_round_trips_freeform_text() {
        use crossterm::event::{KeyCode, KeyEvent};
        use runtime::message_stream::{BlockId, QuestionOption, UserQuestionPrompt};

        let mut ui = test_ui();
        let (responder, mut response) = tokio::sync::oneshot::channel();
        ui.park(super::PendingPrompt::Question(UserQuestionPrompt {
            id: BlockId(3),
            question: "Which auth method?".to_string(),
            header: None,
            options: vec![QuestionOption::plain("OAuth"), QuestionOption::plain("API key")],
            multi_select: false,
            responder,
        }));

        for _ in 0..2 {
            ui.question_key(KeyEvent::from(KeyCode::Down));
        }
        ui.question_key(KeyEvent::from(KeyCode::Enter));
        for ch in "device flow".chars() {
            ui.question_key(KeyEvent::from(KeyCode::Char(ch)));
        }
        ui.question_key(KeyEvent::from(KeyCode::Enter));

        assert_eq!(
            response.try_recv().expect("question answered"),
            vec!["device flow".to_string()]
        );
    }

    #[test]
    fn multi_select_submission_appends_typed_custom_answer() {
        use crossterm::event::{KeyCode, KeyEvent};
        use runtime::message_stream::{BlockId, QuestionOption, UserQuestionPrompt};

        let mut ui = test_ui();
        let (responder, mut response) = tokio::sync::oneshot::channel();
        ui.park(super::PendingPrompt::Question(UserQuestionPrompt {
            id: BlockId(4),
            question: "Which checks?".to_string(),
            header: None,
            options: vec![QuestionOption::plain("Unit"), QuestionOption::plain("Integration")],
            multi_select: true,
            responder,
        }));

        ui.question_key(KeyEvent::from(KeyCode::Char(' ')));
        for ch in "manual probe".chars() {
            ui.question_key(KeyEvent::from(KeyCode::Char(ch)));
        }
        ui.question_key(KeyEvent::from(KeyCode::Enter));

        assert_eq!(
            response.try_recv().expect("question answered"),
            vec!["Unit".to_string(), "manual probe".to_string()]
        );
    }

    fn assert_one_plan_cell(frame: &str) {
        assert_eq!(
            (
                frame.matches("\u{1b}[K• Updated Plan").count(),
                frame.matches("\u{1b}[K• Called TodoWrite").count(),
            ),
            (1, 0),
            "one TodoWrite call must render exactly one plan cell: {frame:?}"
        );
    }

    fn tool(n: usize, body_bytes: usize) -> ReplayItem {
        ReplayItem::ToolCall {
            name: "bash".to_string(),
            input: format!("call {n}"),
            output: Some(format!("BODY_{n}_").repeat(body_bytes / 8)),
            is_error: false,
        }
    }

    #[test]
    fn direct_todowrite_survives_reasoning_before_its_result_as_one_plan_cell() {
        let mut ui = test_ui();
        let ids = BlockIdGen::default();
        announce_todo(&mut ui, &ids, "todo-direct", false);
        reason_between_call_and_result(&mut ui, &ids);
        settle_todo(&mut ui, &ids, "todo-direct", "TodoWrite");

        assert_one_plan_cell(&finish_tool_frame(&mut ui));
    }

    #[test]
    fn capability_invoke_todowrite_renders_the_inner_result_as_one_plan_cell() {
        let mut ui = test_ui();
        let ids = BlockIdGen::default();
        announce_todo(&mut ui, &ids, "todo-wrapped", true);
        reason_between_call_and_result(&mut ui, &ids);
        settle_todo(&mut ui, &ids, "todo-wrapped", "CapabilityInvoke");

        assert_one_plan_cell(&finish_tool_frame(&mut ui));
    }

    /// Plan submission left the wire in t-2903 (its seat went to `Agent`), so
    /// a strict provider reaches `ExitPlanModeV2` through `CapabilityInvoke`.
    /// The plan screen keys on the EFFECTIVE name: matched on the wire name it
    /// would fall back to the ordinary tool cell, and the human asked to
    /// approve the plan would see one JSON `message` line instead of the plan.
    #[test]
    fn a_plan_submitted_through_capability_invoke_still_gets_its_screen() {
        let mut ui = test_ui();
        let ids = BlockIdGen::default();
        for (call_id, name, input) in [
            (
                "plan-direct",
                "ExitPlanModeV2",
                json!({ "plan": "## Do the thing" }),
            ),
            (
                "plan-wrapped",
                "CapabilityInvoke",
                json!({ "name": "ExitPlanModeV2", "input": { "plan": "## Do the thing" } }),
            ),
        ] {
            let preview = preview_tool_input(name, &input);
            let summary = preview_summary(&preview);
            ui.block(RenderBlock::ToolCall {
                id: ids.next(),
                tool_call_id: ToolCallId(call_id.to_string()),
                name: name.to_string(),
                summary,
                preview,
                status: ToolCallStatus::Running,
            });
            let result = json!({
                "status": "ok",
                "plan": "## Do the thing",
                "summary": null,
                "planModeExited": false,
                "planPath": "/x/.zo/plans/p.md",
                "message": "plan saved"
            })
            .to_string();
            ui.block(RenderBlock::ToolResult {
                id: ids.next(),
                tool_call_id: ToolCallId(call_id.to_string()),
                is_error: false,
                body: format_tool_result_from_raw(name, &result, false),
            });
        }
        let frame = finish_tool_frame(&mut ui);
        assert_eq!(
            frame.matches("Proposed Plan").count(),
            2,
            "both the direct and the wrapped submission must take the plan screen: {frame:?}"
        );
    }

    #[test]
    fn a_late_duplicate_result_cannot_add_a_second_cell_for_the_same_todo_call() {
        let mut ui = test_ui();
        let ids = BlockIdGen::default();
        announce_todo(&mut ui, &ids, "todo-late", true);
        settle_todo(&mut ui, &ids, "todo-late", "CapabilityInvoke");
        reason_between_call_and_result(&mut ui, &ids);
        settle_todo(&mut ui, &ids, "todo-late", "TodoWrite");

        assert_one_plan_cell(&finish_tool_frame(&mut ui));
    }

    /// The transcript store is the one thing in a long run that nothing else
    /// bounds.
    ///
    /// The session's own messages are bounded by compaction and the painter's
    /// cache by the screen; this list grows with the number of tool calls and
    /// never shrank. Tool output reaches the UI already capped
    /// (`TruncationConfig`: bash 16 KiB, otherwise 30 KiB), which bounds one
    /// entry and not the list — 1,000 calls at 5 KiB is 5 MiB and it does not
    /// stop. Measured below against a 64 KiB budget so the arithmetic is
    /// legible; the real one is 4 MiB.
    #[test]
    fn the_transcript_store_stays_inside_its_budget() {
        const BUDGET: usize = 64 * 1024;
        let mut items = Vec::new();
        let mut bytes = 0;
        let unbounded: usize = (0..400).map(|n| item_bytes(&tool(n, 4096))).sum();

        for n in 0..400 {
            record_transcript_item(&mut items, &mut bytes, tool(n, 4096), BUDGET);
            assert!(
                bytes <= BUDGET.max(item_bytes(&tool(n, 4096))),
                "the store must never sit above its budget (at {n}: {bytes})"
            );
        }

        assert_eq!(items.len(), 400, "no call ever disappears from the list");
        assert!(
            unbounded > 1_000_000 && bytes < BUDGET + 8192,
            "unbounded would have been {unbounded} bytes; bounded is {bytes}"
        );
    }

    /// Oldest first. Someone opening the transcript is almost always looking at
    /// what just happened, so dropping the newest bodies would empty the screen
    /// they came for while the store still looked healthy.
    #[test]
    fn the_newest_bodies_are_the_ones_that_survive() {
        const BUDGET: usize = 32 * 1024;
        let mut items = Vec::new();
        let mut bytes = 0;
        for n in 0..200 {
            record_transcript_item(&mut items, &mut bytes, tool(n, 4096), BUDGET);
        }

        let last = match &items[199] {
            transcript::Entry::Replay(ReplayItem::ToolCall { output, .. }) => {
                output.clone().expect("body")
            }
            _ => panic!("tool call"),
        };
        assert!(
            last.contains("BODY_199_"),
            "the newest body is intact: {}",
            &last[..last.len().min(40)]
        );
        let first = match &items[0] {
            transcript::Entry::Replay(ReplayItem::ToolCall { output, .. }) => {
                output.clone().expect("body")
            }
            _ => panic!("tool call"),
        };
        assert_eq!(first, DROPPED_BODY, "the oldest body was the one released");
    }

    /// A dropped body is released once. Re-walking the list must not keep
    /// subtracting for entries already dropped, or the counter drifts below the
    /// truth and the store silently grows again.
    #[test]
    fn dropping_the_same_body_twice_does_not_double_count() {
        const BUDGET: usize = 8 * 1024;
        let mut items = Vec::new();
        let mut bytes = 0;
        for n in 0..60 {
            record_transcript_item(&mut items, &mut bytes, tool(n, 4096), BUDGET);
        }
        let live: usize = items.iter().map(entry_bytes).sum();
        assert_eq!(
            bytes, live,
            "the running counter must equal what the list actually holds"
        );
    }

    /// A changed or unexpected tool output must fall back to the ordinary tool
    /// cell, never to an empty plan screen — the screen exists to show the
    /// human what they are approving, and an empty one is worse than none.
    #[test]
    fn only_a_real_plan_submission_takes_over_the_screen() {
        use super::{parse_submitted_plan, ToolResultBody};

        let text = |value: serde_json::Value| ToolResultBody::Text {
            content: value.to_string(),
            truncated: false,
        };

        let (plan, path) = parse_submitted_plan(&text(serde_json::json!({
            "status": "ok",
            "plan": "## Do the thing",
            "planPath": "/x/.zo/plans/p.md",
        })))
        .expect("a well-formed submission is recognized");
        assert_eq!(plan, "## Do the thing");
        assert_eq!(path.as_deref(), Some("/x/.zo/plans/p.md"));

        // The artifact write is best-effort in the tool, so a missing path is
        // an ordinary outcome — the plan still deserves its screen.
        let (_, path) = parse_submitted_plan(&text(serde_json::json!({
            "status": "ok",
            "plan": "body",
        })))
        .expect("a submission without a saved path is still a plan");
        assert_eq!(path, None);

        assert!(parse_submitted_plan(&text(serde_json::json!("not a plan"))).is_none());
        assert!(parse_submitted_plan(&text(serde_json::json!({ "status": "ok" }))).is_none());
        assert!(parse_submitted_plan(&text(serde_json::json!({ "plan": "   " }))).is_none());
    }

    /// A palette reply that arrives after the probe gave up is not the
    /// operator. Measured with a 150ms reply: 46 characters of
    /// `]10;rgb:…` landed in the composer, one Enter from being sent — and its
    /// leading ESC would fire an interrupt nobody asked for.
    #[test]
    fn a_late_palette_reply_is_swallowed_whole() {
        use super::{KeyCode, KeyEvent, LateOscGuard};
        use std::time::{Duration, Instant};

        let now = Instant::now();
        let mut guard = LateOscGuard {
            until: now + Duration::from_secs(3),
            saw_escape: false,
            inside: false,
        };
        let key = |code| KeyEvent::from(code);

        // ESC ] 1 0 ; r g b : … ESC \
        assert!(guard.swallows(&key(KeyCode::Esc), now), "the leading ESC must not interrupt");
        assert!(guard.swallows(&key(KeyCode::Char(']')), now));
        for ch in "10;rgb:eeee/eeee/ecec".chars() {
            assert!(guard.swallows(&key(KeyCode::Char(ch)), now), "{ch} reached the composer");
        }
        assert!(guard.swallows(&key(KeyCode::Char('\\')), now), "the terminator goes too");

        // Closed: the operator owns every key again.
        assert!(!guard.swallows(&key(KeyCode::Char('h')), now));
    }

    /// An ESC that is not a reply must cost at most itself, and only inside the
    /// window — three seconds after boot, not forever.
    #[test]
    fn a_real_escape_disarms_the_guard_instead_of_being_eaten_forever() {
        use super::{KeyCode, KeyEvent, LateOscGuard};
        use std::time::{Duration, Instant};

        let now = Instant::now();
        let mut guard = LateOscGuard {
            until: now + Duration::from_secs(3),
            saw_escape: false,
            inside: false,
        };
        let key = |code| KeyEvent::from(code);

        assert!(guard.swallows(&key(KeyCode::Esc), now));
        // Not `]` — so this was a real Esc, and the guard stands down.
        assert!(!guard.swallows(&key(KeyCode::Char('x')), now));
        assert!(!guard.swallows(&key(KeyCode::Esc), now), "the guard must be disarmed");
    }

    /// Past the window nothing is touched, however it started.
    #[test]
    fn the_guard_expires() {
        use super::{KeyCode, KeyEvent, LateOscGuard};
        use std::time::{Duration, Instant};

        let now = Instant::now();
        let mut guard = LateOscGuard {
            until: now,
            saw_escape: false,
            inside: true,
        };
        assert!(!guard.swallows(&KeyEvent::from(KeyCode::Char('a')), now + Duration::from_secs(1)));
    }
    /// A result that missed its cell must still be drawn as what it is. The
    /// old code forced `ToolKind::Call`, so an edit came back as a generic
    /// `Called edit_file(…)` with the raw patch — no line numbers, no colors,
    /// no `(+N -M)`. That is half of the reported blocker, and it is fixable
    /// without knowing why the id missed.
    #[test]
    fn an_unattached_edit_is_still_drawn_as_an_edit() {
        use super::{orphan_kind, Outcome, PendingTool, ToolKind};
        use crate::tui::tools::Change;
        use runtime::message_stream::DiffView;

        let change = Change {
            verb: "Edited",
            path: "src/probe.rs".to_string(),
            added: 3,
            removed: 1,
            view: DiffView {
                old_path: Some("src/probe.rs".into()),
                new_path: Some("src/probe.rs".into()),
                language: Some("rust".into()),
                hunks: Vec::new(),
            },
        };
        let outcome = Outcome {
            declined: false,
            ok: true,
            output: String::new(),
            change: Some(change),
        };
        let announced = PendingTool {
            name: "edit_file".to_string(),
            result_name: "edit_file".to_string(),
            detail: String::new(),
            path: "src/probe.rs".to_string(),
            kind: ToolKind::Edit {
                path: "src/probe.rs".to_string(),
            },
            presentation: runtime::message_stream::ToolPresentation::Persistent,
        };

        assert!(
            matches!(orphan_kind(&outcome, Some(&announced)), ToolKind::Edit { path } if path == "src/probe.rs"),
            "an edit result must not degrade to a generic call cell"
        );
    }

    /// Without a diff, retain the announced typed preview. With nothing
    /// announced at all the cell still has to say something.
    #[test]
    fn an_unattached_result_without_a_diff_keeps_its_preview_kind() {
        use super::{orphan_kind, Outcome, PendingTool, ToolKind};

        let outcome = Outcome {
            declined: false,
            ok: true,
            output: "done".to_string(),
            change: None,
        };
        let announced = PendingTool {
            name: "bash".to_string(),
            result_name: "bash".to_string(),
            detail: "cargo test".to_string(),
            path: String::new(),
            kind: ToolKind::Command {
                command: "cargo test".to_string(),
                background: false,
            },
            presentation: runtime::message_stream::ToolPresentation::Persistent,
        };
        assert!(matches!(
            orphan_kind(&outcome, Some(&announced)),
            ToolKind::Command { command, .. } if command == "cargo test"
        ));
        assert!(matches!(
            orphan_kind(&outcome, None),
            ToolKind::Call { name, detail } if name == "tool" && detail.is_empty()
        ));
    }
    /// Widening and narrowing are not symmetric mid-turn, so they must not be
    /// reported with one sentence. The shared cell restores the workspace
    /// boundary either way, but the running turn keeps the approval policy it
    /// started with — measured: after narrowing to read-only, 12 in-workspace
    /// writes still ran to completion and only escapes were refused. Telling
    /// the operator "file access now" there reads as "read-only is in force".
    #[test]
    fn narrowing_and_widening_are_not_the_same_claim() {
        use super::{active_permission_label, permission_rank, PermissionMode};

        assert!(permission_rank(PermissionMode::ReadOnly) < permission_rank(PermissionMode::DangerFullAccess));
        assert!(
            permission_rank(PermissionMode::ReadOnly)
                < permission_rank(PermissionMode::WorkspaceWrite)
        );
        assert!(
            permission_rank(PermissionMode::WorkspaceWrite)
                < permission_rank(PermissionMode::DangerFullAccess)
        );
        assert_eq!(
            active_permission_label(PermissionMode::ReadOnly),
            crate::status_format::PLAN_MODE_FOOTER_TEXT
        );
    }
    /// codex binds the permission cycle to Shift+Tab, which crossterm reports
    /// as `BackTab`. The cycle walks the three DISTINCT modes: the picker shows
    /// four rows but two of them are the same `WorkspaceWrite`, and a press
    /// that changed nothing would read as a dropped key.
    #[test]
    fn shift_tab_cycles_the_three_distinct_permission_modes() {
        use super::{next_permission_mode, PermissionMode};

        assert_eq!(
            next_permission_mode(PermissionMode::ReadOnly),
            PermissionMode::WorkspaceWrite
        );
        assert_eq!(
            next_permission_mode(PermissionMode::WorkspaceWrite),
            PermissionMode::DangerFullAccess
        );
        assert_eq!(
            next_permission_mode(PermissionMode::DangerFullAccess),
            PermissionMode::ReadOnly
        );
    }

    /// Only half of a permission switch can be live, so the write reports
    /// whether it landed. Without a session there is no cell, and claiming
    /// success there would tell the user a boundary moved when it did not.
    #[test]
    fn the_live_half_says_whether_it_landed() {
        use super::{write_permission_cell, PermissionMode};
        use std::sync::Mutex;

        assert!(!write_permission_cell(None, PermissionMode::DangerFullAccess));

        let cell = Mutex::new(None);
        assert!(write_permission_cell(
            Some(&cell),
            PermissionMode::DangerFullAccess
        ));
        assert_eq!(
            *cell.lock().expect("cell"),
            Some(PermissionMode::DangerFullAccess)
        );
    }
    /// codex takes the shimmer word from the first bold heading of the model's
    /// reasoning (`chatwidget.rs::extract_first_bold`). Two refusals matter
    /// while deltas are still arriving: an unclosed `**` waits instead of
    /// flashing half a phrase, and an empty `****` never blanks the shimmer.
    #[test]
    fn the_shimmer_word_waits_for_a_closed_heading() {
        use super::first_bold_heading;

        assert_eq!(first_bold_heading("**Checking the test**\nbody"), Some("Checking the test"));
        assert_eq!(first_bold_heading("intro **Second** and **Third**"), Some("Second"));
        assert_eq!(first_bold_heading("  **  padded  **"), Some("padded"));

        assert_eq!(first_bold_heading("**still writing the head"), None);
        assert_eq!(first_bold_heading("****"), None);
        assert_eq!(first_bold_heading("no markers at all"), None);
        assert_eq!(first_bold_heading(""), None);
    }

    #[test]
    fn partial_reasoning_never_churns_the_status_heading() {
        let mut ui = test_ui();
        ui.status = Some(crate::tui::view::Status::working(Duration::ZERO));

        ui.absorb_reasoning_heading(7, "Inspecting the tool", false);
        assert_eq!(ui.status.as_ref().map(|status| status.header.as_str()), Some("Working"));
        ui.absorb_reasoning_heading(7, " display for narrow panes", false);
        assert_eq!(ui.status.as_ref().map(|status| status.header.as_str()), Some("Working"));

        ui.absorb_reasoning_heading(7, "\n**Polishing tool display**", false);
        assert_eq!(
            ui.status.as_ref().map(|status| status.header.as_str()),
            Some("Polishing tool display")
        );
    }

    /// The detail line names the tool now running and stays short — the
    /// elapsed and interrupt hints share that row and must survive.
    #[test]
    fn the_detail_line_names_what_is_running() {
        use super::{status_detail_for, Explored, ToolKind};

        assert_eq!(
            status_detail_for(&ToolKind::Command { command: "cargo test".into(), background: false }),
            "cargo test"
        );
        assert_eq!(
            status_detail_for(&ToolKind::Edit { path: "src/lib.rs".into() }),
            "editing src/lib.rs"
        );
        assert_eq!(
            status_detail_for(&ToolKind::Explore(vec![Explored::Read { name: "a.rs".into() }])),
            "reading a.rs"
        );
        assert_eq!(
            status_detail_for(&ToolKind::Explore(Vec::new())),
            "exploring",
            "a call with nothing parsed still says something"
        );
        assert_eq!(
            status_detail_for(&ToolKind::Call { name: "Agent".into(), detail: String::new() }),
            "Agent"
        );
    }
    /// A background bash that succeeded is the foreground `Ran` cell for its
    /// command — the command in the header, `(no output)` under it — while
    /// the transcript keeps the raw log the model read, tags and all.
    #[test]
    fn a_completed_background_bash_is_a_ran_cell_and_keeps_its_raw_log_in_the_transcript() {
        let mut ui = test_ui();
        let ids = BlockIdGen::default();
        let body = "$ python3 <<'PY'\nprint('detail')\nPY\n[pid 42]\n[exit 0]";
        ui.block(RenderBlock::AgentResult {
            id: ids.next(),
            label: "background bash".to_string(),
            status: AgentResultStatus::Completed,
            summary: None,
            body: body.to_string(),
        });
        assert!(matches!(
            ui.transcript_items.last(),
            Some(transcript::Entry::Replay(ReplayItem::AgentResult { label, status, body: saved, .. }))
                if label == "background bash"
                    && *status == AgentResultStatus::Completed
                    && saved == body
        ));
        let history: Vec<String> = ui.pending_history.iter().map(Line::plain).collect();
        assert_eq!(
            history,
            vec![
                String::new(),
                format!(
                    "• Ran python3 <<'PY' print('detail') PY {}",
                    super::super::strings::COMMAND_BACKGROUND
                ),
                "  └ (no output)".to_string(),
            ],
            "the completion is the foreground cell of its command"
        );
    }

    /// A background bash that failed used to come back as an agent card —
    /// `Failed background bash`, a 20-row budget, every line still wearing
    /// its `[stderr]` tag and the `[exit 1]` closer (07:10 screenshot,
    /// t-3177). Codex closes the exec cell in place. Here the completion is
    /// drawn by the foreground command formatter: the same `Ran <command>`
    /// header plus one dim word, the same five-row preview with the same
    /// elision, the same red bullet for the same exit code.
    #[test]
    fn a_failed_background_bash_completion_is_the_foreground_ran_cell_with_one_dim_word() {
        use runtime::message_stream::{BashResult, ToolResultBody};

        const COMMAND: &str = "sh -c 'python3 -m pytest >&2'";
        let stack: Vec<String> = (1..=75).map(|line| format!("stderr line {line}")).collect();

        // The control: the same command, the same output, the same exit
        // code, run in the foreground.
        let mut ui = test_ui();
        let ids = BlockIdGen::default();
        ui.block(tool_call(&ids, "fg-1", "bash", &json!({"command": COMMAND})));
        ui.block(RenderBlock::ToolResult {
            id: ids.next(),
            tool_call_id: ToolCallId("fg-1".to_string()),
            is_error: false,
            body: ToolResultBody::Bash(BashResult {
                exit_code: 1,
                stdout: String::new(),
                stderr: format!("{}\n", stack.join("\n")),
                truncated: false,
            }),
        });
        let foreground: Vec<String> = ui.pending_history.iter().map(Line::plain).collect();
        let foreground_head = foreground
            .iter()
            .position(|row| row.starts_with("• Ran "))
            .unwrap_or_else(|| panic!("no foreground Ran header: {foreground:?}"));
        ui.pending_history.clear();

        // The background completion, exactly as the pump re-injects it: the
        // host header, then the task log with its `$ command` / `[pid]` head,
        // tagged stderr, and `[exit N]` closer.
        let tagged: Vec<String> = stack.iter().map(|line| format!("[stderr] {line}")).collect();
        let body = format!(
            "[task notification — background agent `background bash` (id: task_7) failed. To follow up without losing its context, use SendMessage with that id/name as `to` to continue this agent]\n\n\
             $ {COMMAND}\n[pid 4242]\n{}\n[exit 1]",
            tagged.join("\n")
        );
        ui.block(RenderBlock::AgentResult {
            id: ids.next(),
            label: "background bash".to_string(),
            status: AgentResultStatus::Failed,
            summary: None,
            body,
        });
        let background: Vec<String> = ui.pending_history.iter().map(Line::plain).collect();

        for word in ["[stderr]", "[exit", "background bash", "Failed"] {
            assert!(
                !background.iter().any(|row| row.contains(word)),
                "`{word}` reached the screen: {background:#?}"
            );
        }
        let expected_head = format!(
            "{} {}",
            foreground[foreground_head],
            super::super::strings::COMMAND_BACKGROUND
        );
        assert_eq!(
            background.get(foreground_head).map(String::as_str),
            Some(expected_head.as_str()),
            "the header is the foreground header plus one dim word: {background:#?}"
        );
        assert_eq!(
            &background[foreground_head + 1..],
            &foreground[foreground_head + 1..],
            "the body must be the foreground preview, row for row"
        );
        assert_eq!(
            &background[..foreground_head],
            &foreground[..foreground_head],
            "the rows above the header (the cell's leading blank) match too"
        );
        // Five preview rows: two of head, the elision, two of tail.
        assert_eq!(
            &background[foreground_head + 1..],
            &[
                "  └ stderr line 1".to_string(),
                "    stderr line 2".to_string(),
                "    … +71 lines".to_string(),
                "    stderr line 74".to_string(),
                "    stderr line 75".to_string(),
            ]
        );
    }

    /// The runtime's housekeeping — `Context trim · cleared N old tool
    /// result(s) …`, `⚠ Context nearing auto-compaction …` — used to be a
    /// transcript cell each (07:20 screenshot, t-3177). Codex leaves no such
    /// line in history: the footer's context figure is the standing word.
    /// A housekeeping-level System block rides the status row, dim, in the
    /// place the `waiting for the model` phrase takes, and adds no history
    /// row; a warning or an error is still a cell.
    #[test]
    fn a_housekeeping_notice_rides_the_status_row_and_never_the_transcript() {
        use runtime::message_stream::SystemLevel;

        let trim = "Context trim · cleared 3 old tool result(s) (~12k tokens freed)";
        let mut ui = test_ui();
        let ids = BlockIdGen::default();
        ui.status = Some(crate::tui::view::Status::working(Duration::from_secs(3)));
        ui.block(RenderBlock::System {
            id: ids.next(),
            level: SystemLevel::Housekeeping,
            text: trim.to_string(),
        });
        let history: Vec<String> = ui.pending_history.iter().map(Line::plain).collect();
        assert!(history.is_empty(), "housekeeping grew the transcript: {history:#?}");
        let status = ui.status.as_ref().expect("the turn is live").line(120).plain();
        assert_eq!(
            status,
            format!("• Working (3s • esc to interrupt) · {trim}"),
            "the notice takes the activity's place on the status row"
        );

        ui.block(RenderBlock::System {
            id: ids.next(),
            level: SystemLevel::Warn,
            text: "the model declined; retrying".to_string(),
        });
        let history: Vec<String> = ui.pending_history.iter().map(Line::plain).collect();
        assert!(
            history.iter().any(|row| row.starts_with("⚠ the model declined")),
            "a warning is still a transcript cell: {history:#?}"
        );
    }

    /// A swap on the wire moves the footer's model field and names the reason
    /// (2026-09-10: the transcript said "retrying with the latest Opus model"
    /// while the footer kept saying the Fable session model). The next request
    /// on the session model clears it, and so does a person's `/model` pick.
    #[test]
    fn a_wire_model_swap_moves_the_footer_model_and_names_the_reason() {
        use runtime::message_stream::{WireModel, WireModelSource};

        let mut ui = test_ui();
        ui.sync_model_display("claude-fable-5-1");
        assert!(ui.passive_footer_line().plain().starts_with("  claude-fable-5-1 "));

        ui.block(RenderBlock::WireModel(WireModel {
            model: "claude-opus-5".to_string(),
            source: WireModelSource::RefusalFallback,
        }));
        let footer = ui.passive_footer_line().plain();
        assert!(footer.starts_with("  claude-opus-5 "), "footer was {footer:?}");
        assert!(footer.contains(" · safety fallback"), "footer was {footer:?}");

        ui.block(RenderBlock::WireModel(WireModel {
            model: "claude-fable-5-1".to_string(),
            source: WireModelSource::Session,
        }));
        let footer = ui.passive_footer_line().plain();
        assert!(footer.starts_with("  claude-fable-5-1 "), "footer was {footer:?}");
        assert!(!footer.contains("fallback"), "footer was {footer:?}");

        ui.block(RenderBlock::WireModel(WireModel {
            model: "gpt-5.6-sol".to_string(),
            source: WireModelSource::QuotaFallback,
        }));
        ui.sync_model_display("claude-opus-5");
        let footer = ui.passive_footer_line().plain();
        assert!(
            footer.starts_with("  claude-opus-5 "),
            "a pick is the setting again: {footer:?}"
        );
        assert!(!footer.contains("quota"), "footer was {footer:?}");
    }

    #[test]
    fn successful_preparation_stays_in_transcript_and_failed_discovery_stays_visible() {
        let mut ui = test_ui();
        let ids = BlockIdGen::default();
        let announce = |ui: &mut super::Ui, id: &str, query: &str| {
            let preview = preview_tool_input("ToolSearch", &json!({ "query": query }));
            let summary = preview_summary(&preview);
            ui.block(RenderBlock::ToolCall {
                id: ids.next(),
                tool_call_id: ToolCallId(id.to_string()),
                name: "ToolSearch".to_string(),
                summary,
                preview,
                status: ToolCallStatus::Running,
            });
        };

        announce(&mut ui, "search-ok", "select:ToolSearch");
        announce(&mut ui, "read-ok", "fixture.txt");
        assert!(ui.tool_cell.as_ref().is_some_and(super::ToolGroup::is_active));
        ui.block(RenderBlock::ToolResult {
            id: ids.next(),
            tool_call_id: ToolCallId("search-ok".to_string()),
            is_error: false,
            body: format_tool_result_from_raw("ToolSearch", "matched ToolSearch", false),
        });
        let active = ui.tool_cell.as_ref().expect("the neighboring tool stays live");
        assert!(active.contains_call("read-ok"));
        assert!(!active.contains_call("search-ok"));
        assert!(matches!(
            ui.transcript_items.last(),
            Some(transcript::Entry::Replay(ReplayItem::ToolCall { name, input, output, is_error }))
                if name == "ToolSearch"
                    && input == "ToolSearch select:ToolSearch"
                    && output.as_deref() == Some("matched ToolSearch")
                    && !is_error
        ));
        ui.block(RenderBlock::ToolResult {
            id: ids.next(),
            tool_call_id: ToolCallId("read-ok".to_string()),
            is_error: false,
            body: format_tool_result_from_raw("Read", "fixture content", false),
        });
        // Both discoveries succeeded and are transcript-only, so the live
        // cell is empty and nothing reached history. History used to hold a
        // row here only because the second announce had evicted the FIRST
        // cell as failed — the t-3063 defect, not a flush.
        assert!(ui.tool_cell.is_none(), "successful discovery is transcript-only");
        assert!(
            ui.pending_history.is_empty(),
            "a row was committed for the discovery pair: {:?}",
            ui.pending_history.iter().map(Line::plain).collect::<Vec<_>>()
        );

        let mut task_ui = test_ui();
        let task_preview = preview_tool_input("TaskOutput", &json!({ "task_id": "task_7" }));
        task_ui.block(RenderBlock::ToolCall {
            id: ids.next(),
            tool_call_id: ToolCallId("task-output".to_string()),
            name: "TaskOutput".to_string(),
            summary: preview_summary(&task_preview),
            preview: task_preview,
            status: ToolCallStatus::Running,
        });
        task_ui.block(RenderBlock::ToolResult {
            id: ids.next(),
            tool_call_id: ToolCallId("task-output".to_string()),
            is_error: false,
            body: format_tool_result_from_raw("TaskOutput", "task log", false),
        });
        assert!(task_ui.tool_cell.is_none(), "successful task polling is transcript-only");
        assert!(matches!(
            task_ui.transcript_items.last(),
            Some(transcript::Entry::Replay(ReplayItem::ToolCall { name, output, is_error, .. }))
                if name == "TaskOutput" && output.as_deref() == Some("task log") && !is_error
        ));

        announce(&mut ui, "search-error", "select:missing");
        ui.block(RenderBlock::ToolResult {
            id: ids.next(),
            tool_call_id: ToolCallId("search-error".to_string()),
            is_error: true,
            body: format_tool_result_from_raw("ToolSearch", "discovery denied", true),
        });
        let visible: Vec<String> = ui.pending_history.iter().map(Line::plain).collect();
        assert!(
            visible.iter().any(|line| line.contains("Called ToolSearch"))
                && visible.iter().any(|line| line.contains("discovery denied")),
            "failed discovery was hidden: {visible:?}"
        );
    }

    /// codex runs two different cadences and they must not collapse into one:
    /// the shimmer ticks at 32ms (`status_indicator_widget.rs:246`) while the
    /// effort transition ticks at 33ms (`bottom_pane/effort_status_line.rs`).
    /// Our single drive-loop ticker samples at the shimmer's rate because the
    /// effort effect measures its own phases against the wall clock; taking
    /// 33ms instead would silently slow the shimmer away from the capture.
    #[test]
    fn the_shimmer_and_the_effort_effect_keep_their_own_cadences() {
        use super::{Duration, FRAME_TICK};
        use crate::tui::effort_effect;

        assert_eq!(FRAME_TICK, Duration::from_millis(32));
        assert_eq!(effort_effect::FRAME_TICK, Duration::from_millis(33));
        assert!(
            FRAME_TICK <= effort_effect::FRAME_TICK,
            "the ticker must sample at least as often as the effect's own rate"
        );
    }

    #[test]
    fn work_turn_separator_is_always_labelled_and_keeps_codex_width_grammar() {
        use super::turn_separator;

        // A short turn is labelled too — a bare rule reads as nothing — and
        // its label has no ticking count, so the same turn draws the same row.
        for seconds in [0, 23, 59] {
            let short = turn_separator(40, seconds, 0);
            assert!(
                short.plain().starts_with("─ Worked for under a minute ─"),
                "{:?}",
                short.plain()
            );
            assert_eq!(short.width(), 40);
            assert!(short.style.dim);
        }
        assert!(turn_separator(40, 60, 0).plain().starts_with("─ Worked for 1m 00s ─"));

        let labelled = turn_separator(30, 125, 0);
        assert!(labelled.plain().starts_with("─ Worked for 2m 05s ─"));
        assert_eq!(labelled.width(), 30);
        assert!(labelled.style.dim);

        let clipped = turn_separator(8, 61, 0);
        assert_eq!(clipped.plain(), "─ Worked");
        assert_eq!(clipped.width(), 8);
    }

    #[test]
    fn only_agent_turns_extend_the_separator_with_codex_dot_grammar() {
        use super::turn_separator;

        assert!(!turn_separator(50, 30, 0).plain().contains("agents"));
        let agent_turn = turn_separator(50, 30, 2);
        assert!(
            agent_turn
                .plain()
                .starts_with("─ Worked for under a minute · agents 2 ─"),
            "separator was {:?}",
            agent_turn.plain()
        );
        assert_eq!(agent_turn.width(), 50);
    }

    #[test]
    fn successful_spawn_results_count_once_and_failed_spawns_do_not() {
        let mut ui = test_ui();
        let ids = BlockIdGen::default();
        for (call_id, is_error) in [("agent-ok", false), ("agent-failed", true)] {
            let input = json!({
                "name": call_id,
                "subagent_type": "worker",
                "description": "inspect the footer"
            });
            let preview = preview_tool_input("Agent", &input);
            let summary = preview_summary(&preview);
            ui.block(RenderBlock::ToolCall {
                id: ids.next(),
                tool_call_id: ToolCallId(call_id.to_string()),
                name: "Agent".to_string(),
                summary,
                preview,
                status: ToolCallStatus::Running,
            });
            ui.block(RenderBlock::ToolResult {
                id: ids.next(),
                tool_call_id: ToolCallId(call_id.to_string()),
                is_error,
                body: format_tool_result_from_raw(
                    "Agent",
                    if is_error { "spawn failed" } else { r#"{"status":"running"}"# },
                    is_error,
                ),
            });
        }

        assert_eq!(ui.turn_agent_count, 1);
    }

    /// A read and a delegation announced in one batch must both survive: the
    /// viewport has one live slot, and whichever took it used to close the
    /// other as failed. The delegation keeps the slot because its helper runs
    /// for minutes and the row is the only sign it is alive.
    #[test]
    fn a_read_announced_beside_a_delegation_costs_neither_its_cell() {
        let mut ui = test_ui();
        let ids = BlockIdGen::default();
        let announce = |ui: &mut super::Ui, call_id: &str, name: &str, input: serde_json::Value| {
            let preview = preview_tool_input(name, &input);
            let summary = preview_summary(&preview);
            ui.block(RenderBlock::ToolCall {
                id: ids.next(),
                tool_call_id: ToolCallId(call_id.to_string()),
                name: name.to_string(),
                summary,
                preview,
                status: ToolCallStatus::Running,
            });
        };

        announce(
            &mut ui,
            "spawn-1",
            "Agent",
            json!({ "name": "scout", "description": "inspect the footer" }),
        );
        assert!(
            ui.tool_cell.as_ref().is_some_and(super::ToolGroup::is_spawning),
            "the delegation opens the live cell"
        );

        announce(&mut ui, "read-1", "Read", json!({ "path": "src/main.rs" }));
        let cell = ui.tool_cell.as_ref().expect("the delegation keeps the slot");
        assert!(cell.is_spawning(), "the read must not evict a live helper");
        assert!(cell.is_active(), "and it must not close it as failed");
        assert!(cell.contains_call("spawn-1"));
    }

    /// Everything the frame put on screen, without its colours and cursor
    /// moves — what a person reads.
    fn plain_frame(frame: &str) -> String {
        let mut out = String::new();
        let mut chars = frame.chars();
        while let Some(ch) = chars.next() {
            if ch != '\u{1b}' {
                out.push(ch);
                continue;
            }
            match chars.next() {
                Some('[') => {
                    for ch in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&ch) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    // OSC: up to BEL or ST.
                    let mut last = None;
                    for ch in chars.by_ref() {
                        if ch == '\u{7}' || (last == Some('\u{1b}') && ch == '\\') {
                            break;
                        }
                        last = Some(ch);
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// Four edits announced before a `bash` in one batch were committed as
    /// `✘ Failed to apply patch` — four `✘ path` rows with no reason — the
    /// moment the `bash` was announced, and their results, every one
    /// `is_error: false`, arrived to a cell already in history and were
    /// dropped (2026-09-07 23:43, the lotto session, t-3063). zo announces a
    /// batch before any result, so the announce of another kind must wait
    /// for the running cell, never close it.
    #[test]
    fn edits_announced_before_a_bash_are_never_closed_as_failed_by_its_announce() {
        let mut ui = test_ui();
        let ids = BlockIdGen::default();
        let edited = |path: &str| {
            json!({
                "filePath": path, "oldString": "a", "newString": "b",
                "structuredPatch": [{"oldStart": 1, "oldLines": 1, "newStart": 1, "newLines": 1, "lines": ["-a", "+b"]}]
            })
            .to_string()
        };

        ui.block(tool_call(&ids, "edit-1", "edit_file", &json!({"path": "src/a.rs", "old_string": "a", "new_string": "b"})));
        ui.block(tool_call(&ids, "edit-2", "MultiEdit", &json!({"path": "src/b.rs", "edits": [{"old_string": "a", "new_string": "b"}]})));
        ui.block(tool_call(&ids, "bash-1", "bash", &json!({"command": "cargo test", "timeout": 1000})));
        let cell = ui.tool_cell.as_ref().expect("the edits keep the live cell");
        assert!(cell.is_editing() && cell.is_active(), "the bash announce closed the edits");
        assert!(cell.contains_call("edit-1") && cell.contains_call("edit-2"));
        assert_eq!(ui.unseated, vec!["bash-1".to_string()], "the bash waits for the cell");
        assert!(ui.pending_history.is_empty(), "nothing was committed by an announce");

        for (call, name, path) in [("edit-1", "edit_file", "src/a.rs"), ("edit-2", "MultiEdit", "src/b.rs")] {
            ui.block(RenderBlock::ToolResult {
                id: ids.next(),
                tool_call_id: ToolCallId(call.to_string()),
                is_error: false,
                body: format_tool_result_from_raw(name, &edited(path), false),
            });
        }
        // The edits are done: the bash takes the slot, the edits go to history.
        let cell = ui.tool_cell.as_ref().expect("the bash holds the live cell now");
        assert!(cell.contains_call("bash-1") && cell.is_active(), "the bash was not seated");
        assert!(ui.unseated.is_empty());

        ui.block(RenderBlock::ToolResult {
            id: ids.next(),
            tool_call_id: ToolCallId("bash-1".to_string()),
            is_error: false,
            body: format_tool_result_from_raw("bash", r#"{"stdout": "ok", "stderr": "", "exit_code": 0}"#, false),
        });
        let frame = plain_frame(&finish_tool_frame(&mut ui));
        assert!(!frame.contains("Failed to apply patch"), "{frame}");
        assert!(!frame.contains('✘'), "{frame}");
        let edited_at = frame.find("Edited 2 files (+2 -2)").unwrap_or_else(|| panic!("{frame}"));
        let ran_at = frame.find("Ran cargo test").unwrap_or_else(|| panic!("{frame}"));
        assert!(edited_at < ran_at, "the edits stand before the command that followed them: {frame}");
    }

    /// The other order: a command still running when an edit is announced.
    /// The edit waits too; its result lands as its own cell while the
    /// command keeps the slot, so neither is ever drawn red for the other.
    #[test]
    fn an_edit_announced_beside_a_running_command_costs_neither_its_cell() {
        let mut ui = test_ui();
        let ids = BlockIdGen::default();
        ui.block(tool_call(&ids, "bash-1", "bash", &json!({"command": "cargo test", "timeout": 1000})));
        ui.block(tool_call(&ids, "edit-1", "edit_file", &json!({"path": "src/a.rs", "old_string": "a", "new_string": "b"})));
        let cell = ui.tool_cell.as_ref().expect("the command keeps the live cell");
        assert!(cell.contains_call("bash-1") && cell.is_active(), "the edit announce closed the command");
        assert_eq!(ui.unseated, vec!["edit-1".to_string()]);

        ui.block(RenderBlock::ToolResult {
            id: ids.next(),
            tool_call_id: ToolCallId("edit-1".to_string()),
            is_error: false,
            body: format_tool_result_from_raw(
                "edit_file",
                &json!({"filePath": "src/a.rs", "oldString": "a", "newString": "b",
                    "structuredPatch": [{"oldStart": 1, "oldLines": 1, "newStart": 1, "newLines": 1, "lines": ["-a", "+b"]}]}).to_string(),
                false,
            ),
        });
        assert!(ui.unseated.is_empty(), "a result that came first needs no seat");
        let cell = ui.tool_cell.as_ref().expect("the command still holds the live cell");
        assert!(cell.contains_call("bash-1") && cell.is_active());

        ui.block(RenderBlock::ToolResult {
            id: ids.next(),
            tool_call_id: ToolCallId("bash-1".to_string()),
            is_error: false,
            body: format_tool_result_from_raw("bash", r#"{"stdout": "ok", "stderr": "", "exit_code": 0}"#, false),
        });
        let frame = plain_frame(&finish_tool_frame(&mut ui));
        assert!(!frame.contains('✘'), "{frame}");
        assert!(frame.contains("Edited src/a.rs (+1 -1)"), "{frame}");
        assert!(frame.contains("Ran cargo test"), "{frame}");
    }

    /// The `agents 1 · running 0 · done 1` line under a working header is a
    /// fact about the running turn: the helper the turn spawned has finished
    /// while the turn goes on. It is a detail of the status line, so it
    /// leaves with it — a frame with no status has no agents line, whatever
    /// the wave remembers (2026-09-07 23:4x screenshot, t-3063).
    #[test]
    fn the_agents_line_is_a_detail_of_the_working_line_and_leaves_with_it() {
        let mut ui = test_ui();
        ui.status = Some(crate::tui::view::Status::working(Duration::from_secs(1897)));
        ui.set_subagent_progress(vec![running_helper("agent-a", "stage-art", "Read · src/a.rs", 3, 9)]);
        ui.set_subagent_progress(Vec::new());
        assert_eq!(
            ui.status.as_ref().expect("working status").details,
            vec!["agents 1 · running 0 · done 1".to_string()]
        );

        // The turn ends: the status goes first (`run_turn`'s end), and the
        // bottom pane it drew goes with it.
        ui.status = None;
        assert!(ui.subagent_wave.summary().is_some(), "premise: the wave still remembers the helper");
        let composer = crate::tui::composer::Composer::new();
        let frame = crate::tui::view::Frame {
            composer: &composer,
            effort_tier: None,
            effort_effect: None,
            now: std::time::Instant::now(),
            status: ui.status.as_ref(),
            pending_input: None,
            active: None,
            tail_owns_slot: false,
            dialog: None,
            question: None,
            picker: None,
            sessions: None,
            pager: None,
            popup: None,
            shortcuts: None,
            model: "test-model",
            effort: "",
            model_note: None,
            cwd: "/test",
            context_left: None,
            context_used_tokens: None,
            plan_mode: false,
            goal_status: None,
            loop_status: None,
            dream: None,
            width: 80,
            max_rows: 23,
        };
        let (rows, _) = crate::tui::view::build(&frame);
        assert!(
            rows.iter().all(|row| !row.plain().contains("agents")),
            "{:?}",
            rows.iter().map(Line::plain).collect::<Vec<_>>()
        );
    }

    #[test]
    fn worktree_context_is_measured_from_git_and_detached_heads_are_silent() {
        use super::worktree_context;
        use std::process::Command;

        let repository = tempfile::tempdir().expect("temp repository");
        let git = |args: &[&str]| {
            let status = Command::new("git")
                .args(args)
                .current_dir(repository.path())
                .status()
                .expect("run git");
            assert!(status.success(), "git {args:?} failed with {status}");
        };
        git(&["init", "--quiet", "--initial-branch=main"]);
        git(&["config", "user.name", "ZeroCode Test"]);
        git(&["config", "user.email", "test@zerocode.invalid"]);
        std::fs::write(repository.path().join("seed"), "seed\n").expect("write seed");
        git(&["add", "seed"]);
        git(&["commit", "--quiet", "-m", "seed"]);
        assert_eq!(worktree_context(repository.path()), None);

        git(&["checkout", "--quiet", "-b", "wt/task"]);
        assert_eq!(worktree_context(repository.path()), None);
        git(&["config", "branch.wt/task.base", "main"]);

        assert_eq!(worktree_context(repository.path()).as_deref(), Some("wt/task ← main"));

        git(&["checkout", "--quiet", "--detach"]);
        assert_eq!(worktree_context(repository.path()), None);
    }

    #[test]
    fn a_late_frame_uses_wall_time_for_every_commit_policy_decision() {
        use super::{run_due_commit_ticks, COMMIT_TICK, MAX_CATCH_UP_TICKS};
        use std::time::{Duration, Instant};

        let enqueued_at = Instant::now();
        let frame_now = enqueued_at + Duration::from_millis(200);
        let mut last_commit = Some(enqueued_at);
        let mut oldest_ages = Vec::new();
        let _: Vec<crate::tui::ansi::Line> = run_due_commit_ticks(
            &mut last_commit,
            frame_now,
            || frame_now,
            |decision_now| {
                oldest_ages.push(decision_now.saturating_duration_since(enqueued_at));
                Vec::new()
            },
        );

        assert_eq!(
            oldest_ages.len(),
            usize::try_from(MAX_CATCH_UP_TICKS).expect("tick cap fits usize")
        );
        assert!(oldest_ages.iter().all(|age| *age == Duration::from_millis(200)));
        assert_eq!(
            last_commit,
            Some(enqueued_at + COMMIT_TICK * MAX_CATCH_UP_TICKS)
        );
    }

    #[test]
    fn quit_reminder_matches_codexs_key_hint_literal() {
        use super::QUIT_SHORTCUT_REMINDER;

        assert_eq!(QUIT_SHORTCUT_REMINDER, "ctrl + c again to quit");
    }

    #[test]
    fn startup_quit_controls_do_not_arm_or_exit_the_empty_composer() {
        use super::{test_ui, KeyCode, KeyEvent, KeyModifiers};
        use std::time::{Duration, Instant};

        let mut ui = test_ui();
        ui.startup_quit_grace_until = Some(Instant::now() + Duration::from_secs(1));
        let ctrl = |ch| KeyEvent::new(KeyCode::Char(ch), KeyModifiers::CONTROL);

        let _ = ui.idle_key(ctrl('c'));
        let _ = ui.idle_key(ctrl('c'));
        let _ = ui.idle_key(ctrl('d'));

        assert_eq!(ui.exit, None, "a launch-time quit control ended the session");
        assert_eq!(
            ui.last_interrupt, None,
            "a launch-time ctrl-c armed the later human quit gesture"
        );
    }

    #[test]
    fn ctrl_c_twice_keeps_codex_quit_semantics_after_startup_grace() {
        use super::{test_ui, ExitReason, KeyCode, KeyEvent, KeyModifiers};

        let mut ui = test_ui();
        ui.startup_quit_grace_until = None;
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);

        let _ = ui.idle_key(ctrl_c);
        let _ = ui.idle_key(ctrl_c);

        assert_eq!(ui.exit, Some(ExitReason::UserExit));
    }

    /// codex binds image paste to ctrl+v and ctrl+alt+v (`keymap.rs`:
    /// `fixed.paste_image`). Alt on its own must keep typing its character —
    /// on macOS Option+V is `√`, and a person who wants that glyph would
    /// otherwise get a clipboard probe instead.
    #[test]
    fn image_paste_needs_control_and_never_alt_alone() {
        use super::{is_paste_image_key, KeyCode, KeyEvent, KeyModifiers};

        let key = |modifiers| KeyEvent::new(KeyCode::Char('v'), modifiers);
        assert!(is_paste_image_key(&key(KeyModifiers::CONTROL), 'v'));
        assert!(is_paste_image_key(
            &key(KeyModifiers::CONTROL | KeyModifiers::ALT),
            'v'
        ));
        assert!(is_paste_image_key(&key(KeyModifiers::CONTROL), 'V'));

        assert!(!is_paste_image_key(&key(KeyModifiers::ALT), 'v'));
        assert!(!is_paste_image_key(&key(KeyModifiers::NONE), 'v'));
        assert!(!is_paste_image_key(&key(KeyModifiers::CONTROL), 'c'));
    }

    #[test]
    fn agents_overview_uses_codex_alt_a_without_stealing_ctrl_a() {
        use super::{agents, KeyCode, KeyEvent, KeyModifiers};

        let key = |ch, modifiers| KeyEvent::new(KeyCode::Char(ch), modifiers);
        assert!(agents::is_open_key(&key('a', KeyModifiers::ALT)));
        assert!(agents::is_open_key(&key(
            'A',
            KeyModifiers::ALT | KeyModifiers::SHIFT
        )));
        assert!(!agents::is_open_key(&key('a', KeyModifiers::CONTROL)));
        assert!(!agents::is_open_key(&key(
            'a',
            KeyModifiers::CONTROL | KeyModifiers::ALT
        )));
    }

    use std::io::Write;

    use super::load_submission_images;

    #[test]
    fn submitted_png_paths_become_runtime_image_blocks() {
        let mut image = tempfile::NamedTempFile::new().expect("temporary png");
        image.write_all(&[0, 1, 2]).expect("write png bytes");
        let images = load_submission_images(&[image.path().to_path_buf()]).expect("read png");
        assert_eq!(images, vec![("image/png".to_string(), "AAEC".to_string())]);
    }
}
