//! REPL 본체 — stdin 단일 소유자와 턴 드라이버를 잇는 `select!` 상태기계.
//!
//! 터미널 상태를 절대 건드리지 않는다: raw mode 도 alt screen 도 없고, 시작과
//! 끝에 bracketed-paste 모드 스위치(`ESC[?2004h`/`l`)만 낸다 — IDE 의 프롬프트
//! 배달·붙여넣기 계약이 그 모드를 본다. 화면은 전부 [`Renderer`] 가 그린다.
//!
//! 한 턴의 배선은 `crate::session::turn_scaffold::TurnScaffold` 가 든다 —
//! 이 파일과 [`crate::tui::app`] 이 같은 한 벌을 쓴다. `ChannelPrompter` + 권한
//! 펌프가 권한 요청을 `RenderBlock::PermissionPrompt` 로 바꿔 렌더 채널에 싣고,
//! `TuiUserQuestionChannel` 이 질문을 같은 채널에 싣는다. 여기 남는 것은 화면
//! 처리뿐이다: 렌더러는 둘을 **파킹**해 돌려주고, 다음 stdin 줄이 답이 된다.
//! 턴 중 다른 줄은 스티어 큐로.

use std::io::{IsTerminal, Write};
use std::time::{Duration, Instant};

use runtime::message_stream::{BlockIdGen, RenderBlock, SystemLevel, ToolCallStatus};
use tokio::sync::mpsc;

use super::args::{HeadlessLoop, HeadlessLoopTrigger, RenderFlags};
#[cfg(test)]
use super::events::{forward_subagent_frames, SubagentFrameSink};
use super::input::{pump_stdin, InputEvent};
use super::prompt::PendingPrompt;
use super::reporter::HookReporter;
use super::channel::state::Command;
use super::channel::wire::ResolvedBy;
use super::events::{self, start_subagent_frame_relay, PromptKind};
use super::render::{InputSource, RenderOptions, Renderer, SessionBanner};
use crate::session::plain_session::{permission_label, PlainSession};
use crate::slash::Slash;
use crate::session::turn_scaffold::TurnScaffold;
use crate::autonomy::driver::{self, wait_for_wakeup, Drive, LoopTurnEnd, TurnOutcome};
use crate::autonomy::loops::LoopTurn;

const PASTE_MODE_ON: &str = "\u{1b}[?2004h";
const PASTE_MODE_OFF: &str = "\u{1b}[?2004l";
/// 유휴 상태에서 Ctrl-C 두 번이 이 간격 안에 오면 종료.
const DOUBLE_INTERRUPT_WINDOW: Duration = Duration::from_secs(1);

const KEEP_LIST_HELP: &str = "\
/model <alias>        모델 교체 (effort 는 TUI 의 /model 피커 2단계에서)
/permissions <mode>   read-only·workspace-write·danger-full-access
/compact [focus]      대화 압축
/goal [command]       지속 목표·bounded autonomous gate
/loop [command]       횟수·간격·파일 변화 반복
/status               사용량 카드 — 모델·권한·세션·컨텍스트·한도
/help                 이 목록
/exit                 종료 (Ctrl-D 도 같다)
그 외 슬래시는 zerocode IDE 에서 하세요.";

/// 루프가 끝난 이유 — 세션 종료 훅의 `reason` 이 된다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitReason {
    /// `/exit` · Ctrl-D · Ctrl-C 두 번.
    UserExit,
    /// stdout 이 닫혔다(호스트가 패인을 죽였다).
    OutputClosed,
    /// The requested bounded headless loop reached its completion condition.
    LoopCompleted,
    /// An until-loop or autonomy budget stopped before completion.
    AutonomousLimit,
    /// Ctrl-C or an interrupted unattended turn stopped the headless loop.
    Interrupted,
}

impl ExitReason {
    const fn hook_reason(self) -> &'static str {
        match self {
            ExitReason::UserExit => "exit",
            ExitReason::OutputClosed => "output_closed",
            ExitReason::LoopCompleted => "loop_completed",
            ExitReason::AutonomousLimit => "autonomy_limit",
            ExitReason::Interrupted => "interrupted",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeadlessRunOutcome {
    pub reason: ExitReason,
    pub exit_code: u8,
}

/// 세션을 받아 REPL 을 끝까지 돈다. 호출자는 tokio 멀티스레드 런타임 안에서
/// 이 future 를 `block_on` 한다(세션은 그 **밖**에서 열어야 한다 — 런타임
/// 빌드가 ambient 런타임 안에서는 패닉한다).
#[allow(clippy::too_many_lines)] // 유휴 select!와 종료 훅까지 한 생애인 frontend loop.
pub async fn run(
    session: PlainSession,
    flags: RenderFlags,
) -> Result<ExitReason, Box<dyn std::error::Error>> {
    Box::pin(run_with_last_message(session, flags, None)).await
}


/// Nobody is at the keyboard — stdin is not a terminal, or it has already
/// closed — so a parked prompt must be decided by policy rather than waited
/// on: left parked it held the turn forever (a headless `zo --plain` under
/// `workspace-write` sat on a `bash` approval for six minutes in a benchmark
/// while Claude Code and codex finished; 2026-09-02). A permission is granted
/// once when the mode allows writes at all and refused under read-only; a
/// question has no answer to give and is dismissed. Either way the turn goes
/// on, and the note says what was decided in the person's absence.
fn decide_unattended<W: Write>(
    prompt: PendingPrompt,
    mode: core_types::PermissionMode,
    renderer: &mut Renderer<W>,
) -> Option<PendingPrompt> {
    match prompt {
        PendingPrompt::Permission(inner) => {
            let tool = inner.tool_name.clone();
            let writes = !matches!(mode, core_types::PermissionMode::ReadOnly);
            let key = if writes { "y" } else { "n" };
            let left = PendingPrompt::Permission(inner).answer(key);
            note(
                renderer,
                &if writes {
                    format!("unattended: {tool} approved once ({})", permission_label(mode))
                } else {
                    format!("unattended: {tool} denied (read-only)")
                },
            );
            left
        }
        PendingPrompt::Question(inner) => {
            PendingPrompt::Question(inner).dismiss();
            note(renderer, "unattended: a question had nobody to answer it; the turn goes on");
            None
        }
    }
}

/// 이번 실행의 렌더 옵션.
///
/// tty 판별은 여기서 **한 번**만 잰다 — 렌더러는 env 도 tty 도 직접 읽지
/// 않는다. 테스트가 두 세계(패인 / 파이프)를 주입할 수 있어야 하기 때문이다.
fn render_options(flags: RenderFlags, session: &PlainSession) -> RenderOptions {
    RenderOptions {
        render_markdown: flags.render_markdown,
        show_thinking: flags.show_thinking,
        json: flags.json,
        input: if std::io::stdin().is_terminal() {
            InputSource::Tty
        } else {
            InputSource::Pipe
        },
        ..RenderOptions::from_env(session.cwd.to_string_lossy().into_owned())
    }
}

/// `--last-message` 를 쓴다. 두 프런트엔드가 같은 함수를 부른다.
///
/// 훅과 드리머가 끝난 **뒤**에 부른다 — 호출자가 이 파일을 보고 다음 단계를
/// 시작하므로, 파일이 나타났을 때 세션은 이미 정리돼 있어야 한다. 빈 답(도구만
/// 돌고 끝난 턴)도 그대로 쓴다: 파일이 없는 것과 답이 비어 있는 것은 호출자
/// 에게 다른 사실이다.
pub fn write_last_message(path: Option<&std::path::Path>, answer: &str) {
    let Some(path) = path else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(error) = std::fs::write(path, answer) {
        eprintln!("zo: --last-message {}: {error}", path.display());
    }
}

/// [`run`] 에 `--last-message` 의 목적지를 더한 것.
///
/// 종료할 때 마지막으로 완성된 어시스턴트 답변 하나를 그 파일에 쓴다 —
/// 호출자가 전체 출력을 파싱하지 않고 답만 읽는 문이다(`codex exec` 의
/// 같은 이름 플래그와 같은 계약).
pub async fn run_with_last_message(
    session: PlainSession,
    flags: RenderFlags,
    last_message: Option<std::path::PathBuf>,
) -> Result<ExitReason, Box<dyn std::error::Error>> {
    Ok(
        Box::pin(run_frontend(session, flags, last_message, None))
            .await?
            .reason,
    )
}

/// Run the first stdin prompt through the shared bounded loop engine.
pub async fn run_headless_loop(
    session: PlainSession,
    flags: RenderFlags,
    last_message: Option<std::path::PathBuf>,
    headless_loop: HeadlessLoop,
) -> Result<HeadlessRunOutcome, Box<dyn std::error::Error>> {
    Box::pin(run_frontend(session, flags, last_message, Some(headless_loop))).await
}

#[allow(clippy::too_many_lines)] // Session-lifetime REPL, including hook/channel teardown.
async fn run_frontend(
    mut session: PlainSession,
    flags: RenderFlags,
    last_message: Option<std::path::PathBuf>,
    headless_loop: Option<HeadlessLoop>,
) -> Result<HeadlessRunOutcome, Box<dyn std::error::Error>> {
    install_stderr_redirect(flags);
    let mut stdout = std::io::stdout();
    // 괄호 붙여넣기 모드는 사람의 터미널에 거는 것이다. JSON 계약은 첫 줄부터
    // 파싱돼야 하므로 제어 시퀀스를 한 바이트도 내지 않는다.
    if !flags.json {
        let _ = stdout.write_all(PASTE_MODE_ON.as_bytes());
        let _ = stdout.flush();
    }

    let options = render_options(flags, &session);
    let mut renderer = Renderer::new(std::io::stdout(), options);
    // What the catalog layer learned before there was a renderer to say it
    // on — an alias that moved, a settings pin. Once each.
    for text in crate::runtime_support::drain_catalog_notices() {
        note(&mut renderer, &text);
    }
    if !flags.json {
        // 배너는 사람에게 하는 인사다. JSON 계약에는 그 어휘가 없고, 넣으면
        // 첫 줄부터 파싱이 깨진다.
        renderer.banner(&banner_for(&session, flags));
    }
    session.fire_session_start();
    let _subagent_frame_relay =
        start_subagent_frame_relay(events::channel(), session.registry(), session.handle.id.clone());
    let reporter = HookReporter::from_env();
    if let Some(reporter) = reporter.as_ref() {
        reporter.session_start(
            if session.resumed() { "resume" } else { "startup" },
            &session.handle.id,
            &session.handle.path.to_string_lossy(),
            &session.cwd.to_string_lossy(),
            &session.model,
        );
    }

    let (input_tx, mut input_rx) = mpsc::channel::<InputEvent>(16);
    tokio::spawn(pump_stdin(input_tx));
    let mut headless_loop = headless_loop.map(HeadlessLoopRun::new);
    let mut input_open = true;

    // 유휴로 들어간다 — 배너 직후가 첫 마커 자리다.
    renderer.idle_marker();
    let mut last_interrupt: Option<Instant> = None;
    let reason = loop {
        if renderer.write_failed() {
            break ExitReason::OutputClosed;
        }
        let next_wakeup = session.next_autonomy_wakeup();
        tokio::select! {
            biased;
            event = input_rx.recv(), if input_open => match event {
                Some(InputEvent::Submit(line)) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        // 빈 줄에도 마커는 다시 나와야 한다 — tty 에코가 앞
                        // 마커의 줄을 이미 닫았다.
                        renderer.idle_marker();
                        continue;
                    }
                    if let Some(spec) = headless_loop.as_mut().filter(|spec| spec.id.is_none()) {
                        let trigger = match &spec.config.trigger {
                            HeadlessLoopTrigger::Every { duration, .. } => {
                                crate::autonomy::scheduler::Trigger::Every {
                                    seconds: duration.as_secs(),
                                }
                            }
                            HeadlessLoopTrigger::Until { command } => {
                                crate::autonomy::scheduler::Trigger::Until {
                                    cmd: command.clone(),
                                }
                            }
                        };
                        spec.id = Some(session.start_headless_loop(
                            trigger,
                            trimmed.to_string(),
                            spec.config.max_runs,
                        )?);
                    } else if headless_loop.is_some() {
                        note(&mut renderer, "headless loop ignores stdin after its first prompt");
                    } else if let Some(command) = trimmed.strip_prefix('/') {
                        match handle_slash(&mut session, command, &mut renderer, reporter.as_ref()) {
                            SlashOutcome::Continue => {}
                            SlashOutcome::Exit => break ExitReason::UserExit,
                            SlashOutcome::Turn(prompt) => {
                                if run_one_turn(&mut session, &prompt, &mut renderer, &mut input_rx, reporter.as_ref(), false, false).await.exit
                                    == TurnExit::ExitAfter
                                {
                                    break ExitReason::UserExit;
                                }
                            }
                        }
                    } else if run_one_turn(&mut session, trimmed, &mut renderer, &mut input_rx, reporter.as_ref(), false, false).await.exit
                        == TurnExit::ExitAfter
                    {
                        break ExitReason::UserExit;
                    }
                    if drive_autonomy(
                        &mut session,
                        &mut renderer,
                        &mut input_rx,
                        reporter.as_ref(),
                        headless_loop.as_ref().and_then(HeadlessLoopRun::id),
                    )
                    .await
                        == Drive::Leaving
                    {
                        break ExitReason::UserExit;
                    }
                    if let Some(reason) = headless_exit_reason(&session, headless_loop.as_ref()) {
                        break reason;
                    }
                    // 턴이든 슬래시든 여기가 유휴 복귀 지점이다.
                    renderer.idle_marker();
                }
                Some(InputEvent::Eof) | None => {
                    input_open = false;
                    if headless_loop.as_ref().and_then(HeadlessLoopRun::id).is_none() {
                        if headless_loop.is_some() {
                            return Err("headless loop requires a non-empty prompt on stdin".into());
                        }
                        break ExitReason::UserExit;
                    }
                }
            },
            () = wait_for_wakeup(next_wakeup) => {
                if drive_autonomy(
                    &mut session,
                    &mut renderer,
                    &mut input_rx,
                    reporter.as_ref(),
                    headless_loop.as_ref().and_then(HeadlessLoopRun::id),
                )
                .await
                    == Drive::Leaving
                {
                    break ExitReason::UserExit;
                }
                if let Some(reason) = headless_exit_reason(&session, headless_loop.as_ref()) {
                    break reason;
                }
                renderer.idle_marker();
            },
            command = events::next_command(events::channel()) => {
                if let Command::AccountSwitch { label } = command {
                    note(
                        &mut renderer,
                        &crate::status_format::account_switch_notice(&label),
                    );
                    renderer.idle_marker();
                }
            },
            _ = tokio::signal::ctrl_c() => {
                if headless_loop.is_some() {
                    break ExitReason::Interrupted;
                }
                let now = Instant::now();
                if last_interrupt.is_some_and(|previous| now.duration_since(previous) <= DOUBLE_INTERRUPT_WINDOW) {
                    break ExitReason::UserExit;
                }
                last_interrupt = Some(now);
                note(&mut renderer, "Ctrl-C 한 번 더 누르면 종료합니다 (또는 /exit)");
                renderer.idle_marker();
            }
        }
    };

    session.fire_session_end(reason.hook_reason());
    // 세션 종료 훅이 드리머를 돌린 **뒤**라야 승격 결과가 남아 있다. 파이프는
    // 종료 요약이 없으므로 이 한 줄이 "긴 세션이 기억을 남겼다"의 유일한
    // 증거다 — 승격된 것이 없으면 줄도 없다.
    if let Some(line) = crate::dream::session_end_line() {
        note(&mut renderer, &line);
    }
    if let Some(reporter) = reporter.as_ref() {
        reporter
            .session_end(&session.handle.id, reason.hook_reason())
            .await;
    }
    let _ = session.persist();
    write_last_message(last_message.as_deref(), renderer.last_assistant_message());
    if !flags.json {
        let _ = stdout.write_all(PASTE_MODE_OFF.as_bytes());
    }
    let _ = stdout.flush();
    let exit_code = match reason {
        ExitReason::AutonomousLimit => crate::autonomy::limits::HEADLESS_LOOP_EXIT_LIMIT,
        ExitReason::Interrupted => crate::autonomy::limits::HEADLESS_LOOP_EXIT_INTERRUPTED,
        ExitReason::UserExit | ExitReason::OutputClosed | ExitReason::LoopCompleted => {
            crate::autonomy::limits::HEADLESS_LOOP_EXIT_DONE
        }
    };
    Ok(HeadlessRunOutcome { reason, exit_code })
}

struct HeadlessLoopRun {
    config: HeadlessLoop,
    id: Option<String>,
}

impl HeadlessLoopRun {
    fn new(config: HeadlessLoop) -> Self {
        Self { config, id: None }
    }

    fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }
}

fn headless_exit_reason(
    session: &PlainSession,
    run: Option<&HeadlessLoopRun>,
) -> Option<ExitReason> {
    use crate::autonomy::loops::HeadlessLoopState;
    match session.headless_loop_state(run?.id()?)? {
        HeadlessLoopState::Running => None,
        HeadlessLoopState::Done => Some(ExitReason::LoopCompleted),
        HeadlessLoopState::Limit => Some(ExitReason::AutonomousLimit),
        HeadlessLoopState::Interrupted => Some(ExitReason::Interrupted),
    }
}

// ============================================================================
// stderr 리다이렉트
// ============================================================================

/// 리다이렉트 대상 — `~/.zo/logs/zo-ide.log` (`ZO_CONFIG_HOME`/`ZO_HOME` 존중).
#[must_use]
pub fn stderr_log_path() -> std::path::PathBuf {
    core_types::paths::default_config_home()
        .join("logs")
        .join("zo-ide.log")
}

/// 패인에서 stderr 를 걷어낸다 — fd 2 를 로그 파일로 `dup2` 한다.
///
/// 패인에 섞이던 줄들(`Using Claude Code session credentials.` · `[zo] session
/// retention: …`)은 우리 코드가 아니라 코어(api 키체인, `runtime_support` 의
/// 정리 스레드)가 낸다. 소스마다 quiet 게이트를 다는 대신 fd 하나를 옮기면
/// **지금 것과 앞으로 생길 것**이 한 번에 조용해진다.
///
/// 트레이드 두 가지를 알고 쓴다:
/// - 패닉 메시지도 로그로 간다. 크래시 진단은 [`stderr_log_path`] 를 봐야 한다.
/// - 되돌릴 문이 필요하면 `--verbose-stderr`.
///
/// 조건: stderr 가 **터미널일 때만** 옮긴다. 호출자가 이미
/// `2> err.txt` 로 돌려 놨다면 그 의도가 우선이다. 파일을 열거나 `dup2` 가
/// 실패하면 조용히 포기한다 — 로그를 못 쓴다고 REPL 이 안 열릴 이유는 없다.
///
/// 자리 한계: [`run`] 의 첫 줄에서 부르므로 세션을 여는 동안(`PlainSession::open`)
/// 나온 stderr — 키체인의 `Using Claude Code session credentials.`, 시작 인증
/// 실패 안내 — 는 이미 패인에 닿은 뒤다. 그것까지 걷어내려면 호출부가 세션을
/// 열기 **전에** 이 함수를 한 번 부르면 된다(두 번 불러도 안전하다).
pub fn install_stderr_redirect(flags: RenderFlags) {
    if flags.verbose_stderr || !std::io::stderr().is_terminal() {
        return;
    }
    #[cfg(unix)]
    {
        let _ = redirect_stderr_to_log();
    }
}

#[cfg(unix)]
fn redirect_stderr_to_log() -> std::io::Result<()> {
    use std::os::unix::io::AsRawFd;

    let path = stderr_log_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    // 워크스페이스가 `unsafe_code = "forbid"` 라 libc 직접 호출 대신 nix.
    nix::unistd::dup2(log.as_raw_fd(), std::io::stderr().as_raw_fd())
        .map_err(std::io::Error::from)?;
    // fd 2 가 이제 같은 open file description 을 가리킨다 — `log` 가 여기서
    // drop 되며 제 fd 만 닫는다.
    eprintln!("--- zo-ide stderr (pid {}) ---", std::process::id());
    Ok(())
}

enum SlashOutcome {
    Continue,
    Exit,
    /// 슬래시가 아닌 것으로 판정돼 프롬프트로 보낼 텍스트(예: `//` 이스케이프).
    Turn(String),
}

#[expect(
    clippy::too_many_lines,
    reason = "the append-only slash dispatch remains one exhaustive catalog match"
)]
fn handle_slash<W: Write>(
    session: &mut PlainSession,
    command: &str,
    renderer: &mut Renderer<W>,
    reporter: Option<&HookReporter>,
) -> SlashOutcome {
    if let Some(rest) = command.strip_prefix('/') {
        return SlashOutcome::Turn(format!("/{rest}"));
    }
    let (name, arg) = match command.split_once(char::is_whitespace) {
        Some((name, arg)) => (name, arg.trim()),
        None => (command, ""),
    };
    match Slash::from_word(name) {
        Some(Slash::Exit) => SlashOutcome::Exit,
        Some(Slash::Help) => {
            // System 블록은 이제 첫 줄만 남는다(멀티라인 System 은 codex 에
            // 없다). 일곱 줄짜리 keep-list 는 통지가 아니라 카드라 원문
            // 블록으로 흘린다.
            notice(renderer, KEEP_LIST_HELP);
            SlashOutcome::Continue
        }
        // TUI 와 **같은 행**이다 — 조립은 `status_format::session_status_card`
        // 한 함수뿐이고, 여기서는 그 행을 그대로 인쇄한다(`/help` 카드가 이미
        // 그 길이다). 색을 끈 렌더러가 SGR 을 걷어낸다.
        Some(Slash::Status) => {
            let rows = crate::status_format::session_status_card(session, terminal_width());
            notice(renderer, &crate::status_format::card_text(&rows));
            SlashOutcome::Continue
        }
        Some(Slash::Model) => {
            if arg.is_empty() {
                note(renderer, &format!(
                    "model {} — 바꾸려면 /model <alias>",
                    session.model
                ));
                return SlashOutcome::Continue;
            }
            let result = tokio::task::block_in_place(|| session.set_model(arg));
            match result {
                Ok(()) => {
                    note(renderer, &format!("model → {}", session.model));
                    // Same reason as the TUI path: the board's badge is sticky
                    // and would freeze on the replaced model.
                    if let Some(reporter) = reporter {
                        reporter.set_model(&session.model);
                    }
                    if let Some(channel) = events::channel() {
                        channel.publish_status(&session.status(), &session.cwd);
                    }
                }
                Err(error) => note(renderer, &format!("model 교체 실패: {error}")),
            }
            SlashOutcome::Continue
        }
        Some(Slash::Permissions) => {
            match crate::permission_mode::normalize_permission_mode(arg) {
                Some(label) => {
                    let mode = crate::permission_mode::permission_mode_from_label(label);
                    session.set_permission_mode(mode);
                    note(renderer, &format!("permissions → {label}"));
                }
                _ => note(renderer, 
                    "permissions: read-only · workspace-write · danger-full-access",
                ),
            }
            SlashOutcome::Continue
        }
        Some(Slash::Compact) => {
            let focus = (!arg.is_empty()).then_some(arg);
            let result = tokio::task::block_in_place(|| session.compact(focus));
            match result {
                Ok((0, kept)) => {
                    note(renderer, &format!("compact: 압축할 것이 없음 ({kept} messages)"));
                }
                Ok((removed, kept)) => {
                    note(renderer, &format!("compact: {removed} removed · {kept} kept"));
                }
                Err(error) => note(renderer, &format!("compact 실패: {error}")),
            }
            SlashOutcome::Continue
        }
        Some(Slash::Goal) => {
            match session.goal_command(arg) {
                Ok(result) => {
                    note(renderer, &result.notice);
                    if let Some(reporter) = reporter {
                        let event = if result.notice.starts_with("goal: running") {
                            Some("GoalStarted")
                        } else if result.notice.contains("paused")
                            || result.notice.contains("stopped")
                        {
                            Some("GoalPaused")
                        } else if result.notice.contains("completed") {
                            Some("GoalAchieved")
                        } else {
                            None
                        };
                        if let Some(event) = event {
                            reporter.autonomy_goal(event, &session.handle.id, &result.notice);
                        }
                    }
                    if let Some(channel) = events::channel() {
                        channel.publish_status(&session.status(), &session.cwd);
                    }
                }
                Err(error) => note_error(renderer, &format!("goal command failed: {error}")),
            }
            SlashOutcome::Continue
        }
        Some(Slash::Loop) => {
            match session.loop_command(arg) {
                Ok(notice) => {
                    note(renderer, &notice);
                    if let Some(channel) = events::channel() {
                        channel.publish_status(&session.status(), &session.cwd);
                    }
                }
                Err(error) => note_error(renderer, &format!("loop command failed: {error}")),
            }
            SlashOutcome::Continue
        }
        // 파이프가 들지 않는 둘 — 이유는 [`Slash::handled_in_pipe`] 에 적혀
        // 있고, `/help` 목록도 그 술어를 읽는다(아래 테스트가 묶어 둔다).
        // 여기를 `_` 로 두지 않는 것은 명령이 늘 때 컴파일러가 답을 받아 내게
        // 하기 위해서다.
        Some(Slash::Fast | Slash::New | Slash::Resume | Slash::Clear) | None => {
            unknown_command(renderer, name)
        }
    }
}

/// zo 가 들지 않는 슬래시 — 한 문구로만 답한다.
fn unknown_command<W: Write>(renderer: &mut Renderer<W>, name: &str) -> SlashOutcome {
    note(
        renderer,
        &format!("/{name} 는 zo 에 없습니다 — zerocode IDE 에서 하세요 (/help)"),
    );
    SlashOutcome::Continue
}

/// 턴이 끝난 뒤 루프가 할 일.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TurnExit {
    Continue,
    /// 턴 중 EOF 나 `/exit` 가 왔다 — 턴은 끝까지 돌리고 나서 나간다
    /// (`cat prompt | zo-ide` 가 답을 다 받도록).
    ExitAfter,
}

struct TurnCompletion {
    exit: TurnExit,
    outcome: TurnOutcome,
}

/// 턴 하나: 권한 펌프·질문 채널을 렌더 채널에 물리고, 블록을 그리면서 stdin 도
/// 계속 듣는다(파킹된 프롬프트의 답 / 스티어 / Ctrl-C 취소).
#[allow(clippy::too_many_lines)] // 한 턴의 select! 상태기계 — 갈래마다 한 arm
async fn run_one_turn<W: Write>(
    session: &mut PlainSession,
    input: &str,
    renderer: &mut Renderer<W>,
    input_rx: &mut mpsc::Receiver<InputEvent>,
    reporter: Option<&HookReporter>,
    autonomous: bool,
    allow_writes: bool,
) -> TurnCompletion {
    let saved_permission = autonomous.then(|| session.begin_autonomous_turn(allow_writes));
    let session_id = session.handle.id.clone();
    let registry = session.registry();
    let transcript_path = session.handle.path.to_string_lossy().into_owned();
    if let Some(reporter) = reporter {
        reporter.user_prompt_submit(input, &session_id);
    }
    // 훅 보고용 관찰 상태: 툴 이름(id→name), 이번 턴의 마지막 어시스턴트 텍스트.
    let mut tool_names: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut last_assistant = String::new();
    // 턴의 배선 한 벌 — 블록 채널·권한 펌프·질문 채널·취소 신호·스티어 큐,
    // 그리고 IDE 채널의 턴 경계까지 [`TurnScaffold`] 가 든다. 여기 남는 것은
    // 화면 처리뿐이다.
    let (mut turn_scaffold, mut block_rx) = TurnScaffold::start(session);
    renderer.push(RenderBlock::UserMessage {
        id: turn_scaffold.ids.next(),
        text: input.to_string(),
    });
    // Read before the turn borrows the session: the unattended decision
    // needs the mode while the turn is in flight.
    let permission_mode = session.permission_mode;
    let mut turn = Box::pin(turn_scaffold.launch().drive(session, input));

    let mut pending: Option<PendingPrompt> = None;
    // 채널에 등록된 프롬프트 id — 패인과 IDE 모달이 같은 id 를 본다.
    let mut pending_id: Option<u64> = None;
    let mut exit_after = false;
    let mut input_open = true;
    // stdin that is not a terminal has no person behind it: a pipe hands over
    // its lines and closes, and a prompt parked after that would wait forever.
    let unattended = !std::io::stdin().is_terminal();
    let mut permission_blocked = false;
    let mut question_blocked = false;
    let outcome = loop {
        // select! 의 팔이 `pending` 을 빌리지 않도록, 기다릴 프롬프트는 Copy 값으로 뽑는다.
        let waiting = pending.as_ref().zip(pending_id).map(|(prompt, id)| {
            let kind = match prompt {
                PendingPrompt::Permission(_) => PromptKind::Permission,
                PendingPrompt::Question(_) => PromptKind::Question,
            };
            (id, kind)
        });
        tokio::select! {
            biased;
            result = &mut turn => break result,
            Some(block) = block_rx.recv() => {
                let is_permission = matches!(&block, RenderBlock::PermissionPrompt(_));
                let is_question = matches!(&block, RenderBlock::UserQuestionPrompt(_));
                observe_for_reporter(
            reporter,
            &registry,
            &block,
            &session_id,
            &mut tool_names,
            &mut last_assistant,
        );
                let published = turn_scaffold.publish(&block);
                if let Some(prompt) = renderer.push(block) {
                    if autonomous && (is_permission || is_question) {
                        permission_blocked |= is_permission;
                        question_blocked |= is_question;
                        prompt.dismiss();
                        events::retire_prompt(published, ResolvedBy::Dismissed);
                        turn_scaffold.cancel_turn();
                        note(
                            renderer,
                            if is_permission {
                                "autonomous turn stopped: approval requires a person"
                            } else {
                                "autonomous turn stopped: a question requires a person"
                            },
                        );
                    } else if unattended && !input_open {
                        // Nobody will type an answer: decide now, note it,
                        // and let the turn continue.
                        pending = decide_unattended(prompt, permission_mode, renderer);
                        pending_id = if pending.is_some() { published } else {
                            events::retire_prompt(published, ResolvedBy::Pane);
                            None
                        };
                    } else {
                        pending = Some(prompt);
                        pending_id = published;
                    }
                }
            }
            Some(answer) = events::wait_answer(turn_scaffold.ide, waiting) => {
                // IDE 모달이 먼저 답했다 — 패인의 프롬프트를 그 답으로 닫는다.
                if let Some(prompt) = pending.take() {
                    let resolved = events::resolve_pending(prompt, &answer);
                    events::retire_prompt(
                        pending_id.take(),
                        if resolved { ResolvedBy::Ide } else { ResolvedBy::Dismissed },
                    );
                }
            }
            command = events::next_command(turn_scaffold.ide) => match command {
                // A `Close` is a stop here: the plain front-end is never a
                // teammate pane (a teammate is interactive by construction),
                // so the server refuses the method before it could arrive.
                Command::CancelTurn { .. } | Command::Close { .. } => {
                    if let Some(prompt) = pending.take() {
                        prompt.dismiss();
                    }
                    turn_scaffold.cancel_turn();
                }
                Command::Steer { text } => {
                    let _ = turn_scaffold.steer(text);
                }
                Command::AccountSwitch { label } => {
                    note(
                        renderer,
                        &crate::status_format::account_switch_notice(&label),
                    );
                }
            },
            event = input_rx.recv(), if input_open => match event {
                Some(InputEvent::Submit(line)) => {
                    let trimmed = line.trim();
                    if let Some(prompt) = pending.take() {
                        match prompt.answer(&line) {
                            None => events::retire_prompt(pending_id.take(), ResolvedBy::Pane),
                            Some(prompt) => {
                                note(renderer, &prompt.hint());
                                pending = Some(prompt);
                            }
                        }
                    } else if Slash::Exit.is_bare_line(trimmed) {
                        exit_after = true;
                    } else if trimmed.starts_with('/') && !trimmed.starts_with("//") {
                        note(renderer, "턴이 끝난 뒤 실행하세요 (지금 입력은 무시)");
                    } else if turn_scaffold.steer(line) {
                        note(renderer, "↳ steer queued");
                    }
                }
                Some(InputEvent::Eof) | None => {
                    // stdin 이 끝났다고 답을 버리지 않는다 — 턴은 끝까지, 그 뒤 종료.
                    input_open = false;
                    if !autonomous {
                        exit_after = true;
                    }
                    // A prompt already parked can no longer be answered by a
                    // person; policy answers it so the turn does not hang.
                    if let Some(prompt) = pending.take() {
                        pending = decide_unattended(prompt, permission_mode, renderer);
                        if pending.is_none() {
                            events::retire_prompt(pending_id.take(), ResolvedBy::Pane);
                        }
                    }
                }
            },
            _ = tokio::signal::ctrl_c() => {
                if let Some(prompt) = pending.take() {
                    prompt.dismiss();
                }
                turn_scaffold.cancel_turn();
            }
        }
    };
    // The boxed future borrows `session`; destroy it before restoring the
    // autonomous turn's scoped permission mode below.
    drop(turn);

    // 턴 future 가 sender 를 놓았으니 남은 블록을 비운다 — 훅 보고와 IDE
    // 채널을 거쳐서다([`TurnScaffold::drain`]).
    turn_scaffold.drain(&mut block_rx, |block| {
        observe_for_reporter(
            reporter,
            &registry,
            &block,
            &session_id,
            &mut tool_names,
            &mut last_assistant,
        );
        if let Some(prompt) = renderer.push(block) {
            prompt.dismiss();
        }
    });
    if let Some(prompt) = pending.take() {
        prompt.dismiss();
    }
    renderer.finish_turn();
    let cancelled = turn_scaffold.cancelled();
    turn_scaffold.finish(&outcome);
    if let Err(error) = &outcome {
        // 오류 수준이다 — 렌더러는 Error 만 자르지 않는다. 스트림 실패 사유가
        // 여러 줄이어도 사람이 대응할 근거를 잃지 않는다.
        note_error(renderer, &format!("turn ended: {error}"));
    }
    if let Some(reporter) = reporter {
        reporter
            .stop(
                &last_assistant,
                &session_id,
                &transcript_path,
                &crate::session::subagent_progress::background_tasks_for_session(
                    &registry,
                    &session_id,
                ),
            )
            .await;
    }
    if let Some(saved) = saved_permission {
        session.finish_autonomous_turn(saved);
    }
    let exit = if exit_after {
        TurnExit::ExitAfter
    } else {
        TurnExit::Continue
    };
    TurnCompletion {
        exit,
        outcome: TurnOutcome {
            summary: outcome.as_ref().ok().cloned(),
            error: outcome.err(),
            permission_blocked,
            question_blocked,
            cancelled,
        },
    }
}

/// What the shared autonomy driver borrows from the append-only frontend.
/// Every action/gate/repair leg is still a normal [`run_one_turn`] with the
/// same permission prompter; unattended approval prompts are dismissed and
/// recorded as a pause.
struct HeadlessFrontend<'a, W: Write> {
    session: &'a mut PlainSession,
    renderer: &'a mut Renderer<W>,
    input_rx: &'a mut mpsc::Receiver<InputEvent>,
    reporter: Option<&'a HookReporter>,
    /// The `--loop-*` loop, whose iterations get a machine-facing boundary record.
    headless_id: Option<&'a str>,
}

impl<W: Write> driver::AutonomyFrontend for HeadlessFrontend<'_, W> {
    fn session(&mut self) -> Option<&mut PlainSession> {
        Some(&mut *self.session)
    }

    fn leaving(&self) -> bool {
        false
    }

    fn note(&mut self, level: SystemLevel, text: &str) {
        let _ = self.renderer.push(system_note(level, text));
    }

    fn reporter(&self) -> Option<&HookReporter> {
        self.reporter
    }

    async fn run_turn(&mut self, prompt: &str, allow_writes: bool) -> Option<TurnOutcome> {
        let completion = run_one_turn(
            self.session,
            prompt,
            self.renderer,
            self.input_rx,
            self.reporter,
            true,
            allow_writes,
        )
        .await;
        (completion.exit == TurnExit::Continue).then_some(completion.outcome)
    }

    fn status_changed(&mut self) {
        if let Some(channel) = events::channel() {
            channel.publish_status(&self.session.status(), &self.session.cwd);
        }
    }

    fn loop_turn_started(&mut self, _turn: &LoopTurn, _dynamic_iteration: bool) {}

    fn loop_turn_ended(&mut self, turn: &LoopTurn, _dynamic_iteration: bool, end: LoopTurnEnd<'_>) {
        let LoopTurnEnd::Recorded { notice, .. } = end else {
            return;
        };
        note(self.renderer, notice);
        if PlainSession::loop_turn_is_iteration(turn)
            && self.headless_id.is_some_and(|id| id == turn.id)
        {
            if let Some(iteration) = self.session.loop_iteration_status(&turn.id) {
                self.renderer.loop_iteration(
                    iteration.iteration,
                    iteration.of,
                    &iteration.trigger,
                    iteration.outcome.label(),
                );
            }
        }
    }
}

/// Every due `/goal` and `/loop` turn; `Leaving` when a turn saw EOF or `/exit`.
async fn drive_autonomy<W: Write>(
    session: &mut PlainSession,
    renderer: &mut Renderer<W>,
    input_rx: &mut mpsc::Receiver<InputEvent>,
    reporter: Option<&HookReporter>,
    headless_id: Option<&str>,
) -> Drive {
    let mut front = HeadlessFrontend {
        session,
        renderer,
        input_rx,
        reporter,
        headless_id,
    };
    driver::drive(&mut front).await
}

/// 블록을 렌더러에 넘기기 **전에** 훅 보고에 필요한 사실만 읽는다(블록은 소비하지
/// 않는다 — 프롬프트 블록은 responder 를 들고 있어 Clone 이 없다).
fn observe_for_reporter(
    reporter: Option<&HookReporter>,
    registry: &tools::AgentRegistry,
    block: &RenderBlock,
    session_id: &str,
    tool_names: &mut std::collections::HashMap<String, String>,
    last_assistant: &mut String,
) {
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
            let key = tool_call_id.0.clone();
            let first_sighting = !tool_names.contains_key(&key);
            tool_names.entry(key).or_insert_with(|| name.clone());
            if first_sighting && *status != ToolCallStatus::Pending {
                if let Some(reporter) = reporter {
                    let activity = crate::tui::activity::Activity::from_preview(name, preview);
                    reporter.pre_tool_use(
                        name,
                        activity.hook_input(summary),
                        &activity.started_card(),
                        session_id,
                    );
                    if HookReporter::spawns_subagent(name) {
                        reporter.subagent_start(
                            &tool_call_id.0,
                            name,
                            HookReporter::detail_of(name, summary),
                            session_id,
                        );
                    }
                }
            }
        }
        RenderBlock::ToolResult {
            tool_call_id,
            is_error,
            ..
        } => {
            if let Some(reporter) = reporter {
                let name = tool_names
                    .get(&tool_call_id.0)
                    .map_or("tool", String::as_str);
                reporter.post_tool_use(name, *is_error, session_id);
                if HookReporter::spawns_subagent(name) {
                    reporter.subagent_stop(
                        &tool_call_id.0,
                        name,
                        "",
                        session_id,
                        &crate::session::subagent_progress::background_tasks_for_session(
                            registry,
                            session_id,
                        ),
                    );
                }
            }
        }
        RenderBlock::PermissionPrompt(prompt) => {
            if let Some(reporter) = reporter {
                reporter.permission_request(&prompt.tool_name, &prompt.reasoning, session_id);
            }
        }
        RenderBlock::UserQuestionPrompt(prompt) => {
            if let Some(reporter) = reporter {
                reporter.question(&prompt.question, session_id);
            }
        }
        _ => {}
    }
}

fn banner_for(session: &PlainSession, flags: RenderFlags) -> SessionBanner {
    let status = session.status();
    let provider = crate::status_format::provider_for_model(&status.model);
    let provider = format!("{provider:?}").to_lowercase();
    SessionBanner {
        header: format!("zo v{}", env!("CARGO_PKG_VERSION")),
        workdir: session.cwd.to_string_lossy().into_owned(),
        model: status.model,
        provider,
        approval: status.permission_mode.to_string(),
        sandbox: permission_label(session.permission_mode).to_string(),
        reasoning_effort: status.effort.unwrap_or("-").to_string(),
        reasoning_summaries: if flags.show_thinking { "dim" } else { "none" }.to_string(),
        session_id: status.session_id,
    }
}

/// 카드의 천장을 정할 화면 폭. 파이프는 터미널 상태를 만지지 않으므로 크기만
/// 한 번 묻고, 답이 없으면(리다이렉트) POSIX 기본값 80칸으로 그린다.
fn terminal_width() -> usize {
    crossterm::terminal::size().map_or(80, |(columns, _)| usize::from(columns).max(20))
}

/// 시스템 노트 한 줄 — 파킹될 일이 없는 블록이라 반환값을 버린다.
///
/// 렌더러가 정보성 System 을 첫 줄로 자르므로 여기 넘기는 텍스트는 **한 줄**
/// 이어야 한다. 여러 줄짜리 본문은 [`notice`] 로.
fn note<W: Write>(renderer: &mut Renderer<W>, text: &str) {
    let _ = renderer.push(system_note(SystemLevel::Info, text));
}

/// 오류 수준 노트. 렌더러는 이것만 자르지 않는다.
fn note_error<W: Write>(renderer: &mut Renderer<W>, text: &str) {
    let _ = renderer.push(system_note(SystemLevel::Error, text));
}

/// 여러 줄짜리 본문(슬래시 카드)을 장식 없이 원문 그대로 흘린다.
fn notice<W: Write>(renderer: &mut Renderer<W>, text: &str) {
    let _ = renderer.push(RenderBlock::UserNotice {
        id: BlockIdGen::default().next(),
        message: text.to_string(),
    });
}

fn system_note(level: SystemLevel, text: &str) -> RenderBlock {
    RenderBlock::System {
        id: BlockIdGen::default().next(),
        level,
        text: text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::{json, Value};
    use tokio::sync::{mpsc, watch};

    use super::{
        forward_subagent_frames, start_subagent_frame_relay, SubagentFrameSink, KEEP_LIST_HELP,
    };
    use crate::ide::events::SubagentsFrame;
    use crate::session::subagent_progress::{RosterSnapshot, SubagentProgress, SubagentProgressWatcher};
    use crate::slash::Slash;

    struct MockSubagentSink {
        frames: mpsc::UnboundedSender<Value>,
    }

    impl SubagentFrameSink for MockSubagentSink {
        fn publish_subagents(&self, frame: &SubagentsFrame<'_>) {
            self.frames
                .send(serde_json::to_value(frame).expect("serialize subagent frame"))
                .expect("frame receiver");
        }
    }

    fn agent(id: &str, label: &str, model: Option<&str>, started_epoch: u64) -> SubagentProgress {
        SubagentProgress {
            agent_id: id.to_string(),
            tool_call_id: None,
            label: label.to_string(),
            model: model.map(str::to_string),
            activity: "Read · src/tui/view.rs".to_string(),
            recent_tools: Vec::new(),
            tool_calls: 4,
            output_tail: String::new(),
            started_epoch,
            elapsed: Duration::from_secs(7),
            no_new_output_for: None,
            transcript_path: None,
            pane: None,
            last_receipt: None,
        }
    }

    #[tokio::test]
    async fn a_watcher_change_emits_one_complete_subagent_frame() {
        let (updates, receiver) = watch::channel(RosterSnapshot::default());
        let watcher = SubagentProgressWatcher::from_receiver(receiver);
        let (frames, mut emitted) = mpsc::unbounded_channel();
        let relay = tokio::spawn(forward_subagent_frames(
            watcher,
            MockSubagentSink { frames },
        ));

        updates
            .send(RosterSnapshot {
                agents: vec![
                    agent("a-17", "verify events", Some("gpt-5.6-sol"), 123),
                    agent("a-18", "", None, 124),
                ],
                generation: 0,
            })
            .expect("watcher update");
        assert_eq!(
            emitted.recv().await.expect("subagent frame"),
            json!({
                "type": "subagents",
                "running": [{
                    "id": "a-17",
                    "label": "verify events",
                    "model": "gpt-5.6-sol",
                    "activity": "Read · src/tui/view.rs",
                    "tool_calls": 4,
                    "started_epoch": 123,
                    "transcript": null,
                    "pane": null,
                }, {
                    "id": "a-18",
                    "label": null,
                    "model": null,
                    "activity": "Read · src/tui/view.rs",
                    "tool_calls": 4,
                    "started_epoch": 124,
                    "transcript": null,
                    "pane": null,
                }],
            })
        );

        drop(updates);
        relay.await.expect("relay stops with its watcher");
    }

    /// A helper that got a pane of its own says which, and a helper that is a
    /// thread of this process says nothing.
    ///
    /// The difference is what a window draws with: a pane id means "there is
    /// a screen you can open", and inventing one for an in-process helper
    /// would point the reader at the PARENT's screen and call it the child.
    #[tokio::test]
    async fn a_helper_with_a_pane_of_its_own_says_which_one() {
        let (updates, receiver) = watch::channel(RosterSnapshot::default());
        let watcher = SubagentProgressWatcher::from_receiver(receiver);
        let (frames, mut emitted) = mpsc::unbounded_channel();
        let relay = tokio::spawn(forward_subagent_frames(
            watcher,
            MockSubagentSink { frames },
        ));

        let mut split = agent("a-19", "recon-graph", None, 125);
        split.pane = Some("%4".to_string());
        updates
            .send(RosterSnapshot {
                agents: vec![split, agent("a-20", "in-process", None, 126)],
                generation: 0,
            })
            .expect("watcher update");
        let frame = emitted.recv().await.expect("subagent frame");
        let running = frame["running"].as_array().expect("running");
        assert_eq!(running[0]["pane"], json!("%4"));
        assert_eq!(running[1]["pane"], Value::Null);

        drop(updates);
        relay.await.expect("relay stops with its watcher");
    }

    #[tokio::test]
    async fn an_empty_watcher_snapshot_emits_the_all_done_signal() {
        let (updates, receiver) = watch::channel(RosterSnapshot {
            agents: vec![agent("a-17", "verify", None, 123)],
            generation: 0,
        });
        let watcher = SubagentProgressWatcher::from_receiver(receiver);
        let (frames, mut emitted) = mpsc::unbounded_channel();
        let relay = tokio::spawn(forward_subagent_frames(
            watcher,
            MockSubagentSink { frames },
        ));

        updates.send(RosterSnapshot::default()).expect("all agents finished");
        assert_eq!(
            emitted.recv().await.expect("empty subagent frame"),
            json!({"type": "subagents", "running": []})
        );

        drop(updates);
        relay.await.expect("relay stops with its watcher");
    }

    #[test]
    fn non_ide_mode_does_not_start_a_subagent_relay() {
        assert!(start_subagent_frame_relay(
            None,
            tools::AgentRegistry::at_root_for_tests("session-a", std::path::Path::new("/tmp")),
            "session-a".to_string()
        )
        .is_none());
    }

    /// 파이프의 `/help` 목록과 파이프가 실제로 드는 명령은 같은 사실을 읽는다.
    /// 예전에는 둘이 손으로 적혀 있어, keep-list 에 명령을 더하면 도움말이
    /// 조용히 뒤처졌다(맨 터미널 단축키 카드가 `/resume` 를 빠뜨린 채 있었던
    /// 것과 같은 어긋남).
    #[test]
    fn the_pipe_help_lists_exactly_what_the_pipe_road_handles() {
        for command in Slash::CATALOG {
            assert_eq!(
                KEEP_LIST_HELP.contains(command.name()),
                command.handled_in_pipe(),
                "{} 의 도움말 등재와 처리 여부가 어긋난다",
                command.name()
            );
        }
    }
}
