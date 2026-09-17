//! 구조화 이벤트 채널 — IDE 의 권한 모달·Stop 버튼·상태 프레임이 지나는 길.
//!
//! 계약: **TCP loopback + 토큰**(대화형 TUI의 자동 `127.0.0.1:0`, 또는
//! `--events-bind 127.0.0.1:PORT`; 토큰은 `ZO_SERVE_TOKEN` 또는 자동 생성,
//! `port-staging/zo-cli/src/serve_auth.rs` 방식)이다 — UDS가 아니라 TCP인 이유는
//! IDE의 `zerocode-harness`가 TCP JSON-RPC 클라이언트라서, 어휘·프레이밍
//! (`jsonrpc` 키 유무로 응답/프레임 구분)을 serve 와이어와 동일하게 유지하면
//! 하네스를 **무변경**으로 재사용하기 때문이다.
//! out = `SerializableRenderBlock` NDJSON(`permission_prompt{prompt_id,choices}`
//! 포함) + status/turn 프레임. in = `permission.respond` / `question.respond` /
//! `session.cancel_turn` / `session.steer`.
//! 세션 연속성 확장 문: 같은 와이어 위에 `session.subscribe`/detach 계열을
//! 다시 세울 재료가 `port-staging`의 serve.rs(rehydrate 608-644,
//! `run_turn_detached`)·`serve/pair.rs`(스펙테이터 허브)에 보존돼 있다.
//!
//! # 이 모듈이 여는 문 (프런트엔드가 잡는 손잡이)
//!
//! 채널은 세션도 렌더러도 알지 못한다. 프런트엔드(`ide::run_loop`, `tui::app`)가
//! 자기 턴 루프에서 다음 넷을 부르면 배선이 끝난다 — 계약서와 정확한 배선
//! 지점은 `docs/events-channel.md`.
//!
//! 1. 블록마다 [`EventsChannel::publish`] — 프롬프트면 `prompt_id` 를 돌려준다.
//! 2. 그 `prompt_id` 로 [`wait_answer`] 를 `select!` 한 갈래에 건다.
//!    IDE 가 먼저 답하면 [`resolve_pending`] 이 패인의 파킹을 대신 풀고,
//!    패인이 먼저 답하면 [`EventsChannel::retire_prompt`] 가 IDE 모달을 내린다.
//!    **먼저 온 답이 이긴다** — 등록부가 한 번만 값을 받는다
//!    ([`channel::state::ChannelState::answer_prompt`]).
//! 3. 턴 앞뒤로 [`EventsChannel::begin_turn`] / [`EventsChannel::end_turn`].
//! 4. [`next_command`](또는 폴링 쪽 [`EventsChannel::take_incoming`])으로
//!    Stop·스티어를 받아 기존 `cancel_by_user` 경로와 스티어 큐에 넣는다.
//!
//! 2·4 의 `select!` 갈래는 채널이 없을 때 **영원히 기다리는** 미래여야 한다.
//! 그 모양은 두 프런트엔드가 똑같이 틀리기 쉬워 [`wait_answer`] ·
//! [`next_command`] 로 여기 한 벌만 둔다. 그 위의 턴 배선 전체는
//! `crate::session::turn_scaffold` 가 든다.

use std::io::Write as _;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use runtime::message_stream::RenderBlock;
use serde::Serialize;
use serde_json::json;
use tokio::net::TcpListener;

use crate::ide::channel::{self, auth::TokenPolicy, state::ChannelState};
use crate::session::plain_session::{ReplayItem, StatusSnapshot};
use crate::session::subagent_progress::SubagentProgressWatcher;
use tools::AgentRegistry;
use crate::sinks::SerializableRenderBlock;

pub use channel::auth::TOKEN_ENV;
pub use channel::state::{
    AccountCard, ActivityCard, Answer, Command, HistoryEntry, PromptKind, StatusCard,
};
pub use channel::wire::{
    ResolvedBy, TurnOutcome, TurnPhase, FRAME_PROMPT_RESOLVED, FRAME_SESSION_STATUS,
    FRAME_SUBAGENTS, FRAME_TURN,
};

/// 바인드 주소를 실어 오는 환경변수 — `--events-bind` 가 없을 때만 읽는다.
///
/// 창이 패인을 띄우는 길은 두 가지다: 인자로 주거나(`--events-bind`), 환경으로
/// 주거나. `zerocode-lane` 은 이미 [`TOKEN_ENV`] 를 pty 환경에 심어 넘기므로
/// (`spawn_attach`), 주소도 같은 방식으로 넘길 수 있어야 한다.
pub const BIND_ENV: &str = "ZO_EVENTS_BIND";

/// 실제로 붙은 주소를 적어 둘 일회성 파일. `--events-bind 127.0.0.1:0` 처럼
/// 커널이 번호를 고르게 한 기존 창 런치가 그 번호를 기다리는 길이다.
/// 없으면 안 쓰며, 앱의 상시 발견 파일과는 별개다.
pub const ADDR_FILE_ENV: &str = "ZO_EVENTS_ADDR_FILE";

/// 대화형 zo가 별도 배선 없이 여는 이벤트 채널. 포트는 커널이 고른다.
pub const INTERACTIVE_BIND: &str = "127.0.0.1:0";

/// 채널 하나를 여는 데 필요한 전부.
#[derive(Debug, Clone)]
pub struct EventsConfig {
    /// 루프백 주소. 포트 `0` 은 커널이 고른다.
    pub bind: String,
    /// 요구할 공유 비밀. `None` 이면 무인증(루프백 전용이므로 허용).
    pub token: Option<String>,
    /// 이 채널이 말하는 세션 id.
    pub session_id: String,
    /// 붙은 주소를 적어 둘 파일.
    pub addr_file: Option<PathBuf>,
    /// 앱이 프로세스 id로 찾는 완결 레코드. 기존 일회성 [`ADDR_FILE_ENV`]
    /// 파일과 달리 주소·토큰·세션 id 세 줄을 담고 채널 수명 동안 살아 있다.
    pub discovery_file: Option<PathBuf>,
}

impl EventsConfig {
    /// 런치 인자와 환경에서 설정을 짓는다. 명시적 주소는 모든 표면에서 기존
    /// 동작을 유지하고, 주소가 없는 대화형 표면은 포트 0의 루프백 채널과
    /// `TMPDIR/zo-events-<pid>.addr` 발견 파일을 스스로 만든다.
    #[must_use]
    pub fn from_launch(
        bind_flag: Option<&str>,
        session_id: &str,
        interactive: bool,
    ) -> Option<Self> {
        Self::from_inputs(
            bind_flag,
            std::env::var(BIND_ENV).ok(),
            TokenPolicy::from_env().into_token(),
            std::env::var_os(ADDR_FILE_ENV).map(PathBuf::from),
            session_id,
            interactive,
            &std::env::temp_dir(),
            std::process::id(),
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the pure launch seam keeps process environment, pid, and TMPDIR out of tests"
    )]
    fn from_inputs(
        bind_flag: Option<&str>,
        bind_env: Option<String>,
        mut token: Option<String>,
        addr_file: Option<PathBuf>,
        session_id: &str,
        interactive: bool,
        runtime_dir: &Path,
        pid: u32,
    ) -> Option<Self> {
        let bind = bind_flag
            .map(str::to_string)
            .or_else(|| bind_env.filter(|bind| !bind.trim().is_empty()))
            .or_else(|| interactive.then(|| INTERACTIVE_BIND.to_string()))?;
        let discovery_file = interactive
            .then(|| runtime_dir.join(format!("zo-events-{pid}.addr")));
        if discovery_file.is_some() && token.is_none() {
            token = Some(generate_discovery_token());
        }
        Some(Self {
            bind,
            token,
            session_id: session_id.to_string(),
            addr_file,
            discovery_file,
        })
    }
}

/// 열려 있는 이벤트 채널.
///
/// 프로세스에 하나, 세션에 하나다. drop 하면 받아들이기 루프가 멈추고 —
/// 이미 붙어 있던 연결은 제 소켓이 닫히며 끝나고 발견 파일도 사라진다.
pub struct EventsChannel {
    state: Arc<ChannelState>,
    local_addr: SocketAddr,
    accept: tokio::task::JoinHandle<()>,
    discovery_file: Option<PathBuf>,
}

impl Drop for EventsChannel {
    fn drop(&mut self) {
        self.accept.abort();
        if let Some(path) = self.discovery_file.as_ref() {
            let _ = std::fs::remove_file(path);
        }
    }
}

impl EventsChannel {
    /// 소켓을 열고 받아들이기 루프를 띄운다. tokio 런타임 **안**에서 불러야
    /// 한다(`main.rs` 의 `block_on` 안).
    ///
    /// # Errors
    ///
    /// 루프백이 아닌 주소, 붙일 수 없는 포트, 주소/발견 파일을 쓸 수 없을 때.
    pub async fn open(config: &EventsConfig) -> Result<Self, String> {
        let requested = channel::auth::resolve_loopback_bind(&config.bind)?;
        let listener = TcpListener::bind(requested)
            .await
            .map_err(|error| format!("--events-bind {}: {error}", config.bind))?;
        let local_addr = listener
            .local_addr()
            .map_err(|error| format!("--events-bind {}: {error}", config.bind))?;
        if let Some(path) = config.addr_file.as_ref() {
            // 창이 이 줄을 읽고 하네스를 붙인다. 못 쓰면 채널이 열려도
            // 아무도 못 찾으므로 조용히 넘기지 않는다.
            write_whole(path, &format!("{local_addr}\n"))?;
        }
        if let Some(path) = config.discovery_file.as_ref() {
            if let Err(error) = write_discovery_file(
                path,
                local_addr,
                config.token.as_deref(),
                &config.session_id,
            ) {
                if let Some(legacy) = config.addr_file.as_ref() {
                    let _ = std::fs::remove_file(legacy);
                }
                return Err(error);
            }
        }
        let state = Arc::new(ChannelState::new(config.session_id.clone()));
        let auth = TokenPolicy::new(config.token.clone());
        let accept = tokio::spawn(channel::server::accept_loop(
            listener,
            Arc::clone(&state),
            auth,
        ));
        Ok(Self {
            state,
            local_addr,
            accept,
            discovery_file: config.discovery_file.clone(),
        })
    }

    /// Install public facts after session resolution and before any turn.
    pub fn install_capabilities(&self, requested: &channel::capabilities::Requested,
        session: &crate::session::plain_session::PlainSession,
        brief: Option<&runtime::subagent_panes::Brief>,
        contract: Option<&channel::capabilities::Contract>) {
        self.state.update_capabilities(|snapshot| {
            snapshot.no_spawn = session.launch_flags().disable_spawn_family;
            snapshot.install(requested, &session.status(), session.selection_origins(),
                channel::capabilities::Mode::from_environment(snapshot.no_spawn), brief, contract);
        });
    }

    /// Declare this pane a teammate (t-2513 §2.2): `session.steer` with no
    /// turn running opens the next one, and `teammate.close` is answered.
    pub fn set_idle_steer(&self, accepted: bool) {
        self.state.set_idle_steer(accepted);
    }

    /// Install where `auth.reload` fans out to — this session's live pane
    /// children (t-2513 §2.6). `None` for a session with no children yet.
    pub fn set_child_fanout(&self, fanout: Option<channel::state::ChildFanout>) {
        self.state.set_child_fanout(fanout);
    }

    /// Install where a child's `mcp.call` is answered — this session's MCP
    /// runtime (t-2513 §2.1). `None` for a session without MCP servers.
    pub fn set_mcp_bridge(&self, bridge: Option<channel::state::McpBridge>) {
        self.state.set_mcp_bridge(bridge);
    }

    /// The discovery file this channel wrote, if it is app-discoverable.
    #[must_use]
    pub fn discovery_file(&self) -> Option<&Path> {
        self.discovery_file.as_deref()
    }

    /// Install the verdict of a launch the exact contract refused. There is
    /// no session: the receipt says what was asked and why nothing ran.
    pub fn install_refused_capabilities(&self, requested: &channel::capabilities::Requested,
        contract: &channel::capabilities::Contract) {
        self.state.update_capabilities(|snapshot| snapshot.install_refusal(requested, contract));
    }

    /// Wait until a host has read `session.capabilities` once, or `hold`
    /// passes. A refused exact launch leaves right after: the verdict was
    /// either read or nobody was going to read it.
    pub async fn wait_for_capabilities_read(&self, hold: std::time::Duration) {
        let deadline = tokio::time::Instant::now() + hold;
        while self.state.capabilities_reads() == 0 && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    }

    /// 실제로 붙은 주소(포트 `0` 을 줬다면 커널이 고른 번호가 들어 있다).
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Whether anybody is reading this channel's frames — a window that has
    /// called `session.subscribe`. The fact `PushNotification` judges its
    /// `window` road by: a bare terminal's auto-opened channel has a listener
    /// and no reader, and must not count.
    #[must_use]
    pub fn has_subscribers(&self) -> bool {
        self.state.subscriber_count() > 0
    }

    /// 렌더 블록 한 장을 프레임으로 내보낸다.
    ///
    /// 프롬프트 블록이면 채널 전역 `prompt_id` 를 발급해 프레임에 싣고 그
    /// 번호를 돌려준다 — 호출자는 그 번호로 [`Self::answer`] 를 기다리고,
    /// 패인이 먼저 답했다면 [`Self::retire_prompt`] 로 내린다.
    /// 프롬프트가 아니면 `None`.
    #[must_use]
    pub fn publish(&self, block: &RenderBlock) -> Option<u64> {
        let projected = SerializableRenderBlock::from_block(block);
        let Some(kind) = prompt_kind_of(block) else {
            // 흔한 길(텍스트 델타·툴 호출)은 `Value` 를 거치지 않고 바로 줄이
            // 된다. 한 턴에 수천 장이 지나가는 경로라 중간 표현 하나가 그대로
            // 할당 수천 번이다.
            self.state.publish(&projected);
            return None;
        };
        let prompt_id = self.state.register_prompt(kind);
        let mut frame = serde_json::to_value(&projected).unwrap_or_else(|_| json!({}));
        if let Some(object) = frame.as_object_mut() {
            // 직렬화기는 블록 id 를 `prompt_id` 로 쓴다. 그 id 는 턴마다 0 에서
            // 다시 세므로 소켓 너머에서는 라우팅 키가 될 수 없다 —
            // `ChannelState::register_prompt` 참고.
            object.insert("prompt_id".to_string(), json!(prompt_id));
        }
        self.state.publish(&frame);
        Some(prompt_id)
    }

    /// 직렬화할 수 있는 것 하나를 프레임으로 내보낸다(상태·턴처럼 렌더
    /// 블록이 아닌 것).
    pub fn publish_frame<T: Serialize>(&self, frame: &T) {
        self.state.publish(frame);
    }

    /// 상태 카드를 갈아 끼우고 `session_status` 프레임을 낸다.
    pub fn publish_status(&self, status: &StatusSnapshot, cwd: &Path) {
        self.state.update_capabilities(|snapshot| {
            // Mode settings are captured at launch. Activity/status publication
            // must never reopen settings or scan the catalog on each heartbeat.
            let mode = snapshot.mode.clone();
            snapshot.refresh(status, mode);
        });
        let card = status_card(status, cwd);
        self.state.publish_snapshot(
            FRAME_SESSION_STATUS,
            &StatusFrame {
                kind: FRAME_SESSION_STATUS,
                session: self.state.session_id(),
                card: &card,
            },
        );
        self.state.set_status(card);
    }

    /// Replace only the live activity fact on the last complete status card.
    pub fn publish_activity(&self, activity: Option<ActivityCard>) {
        let mut card = self.state.status();
        card.activity = activity;
        self.state.publish_snapshot(
            FRAME_SESSION_STATUS,
            &StatusFrame {
                kind: FRAME_SESSION_STATUS,
                session: self.state.session_id(),
                card: &card,
            },
        );
        self.state.set_status(card);
    }

    /// 하이드레이션 히스토리(구독한 클라이언트가 처음 받는 것)를 갈아 끼운다.
    pub fn set_history(&self, items: &[ReplayItem]) {
        self.state.set_history(history_from_replay(items));
    }

    /// 턴 시작 — 번호를 발급하고 `turn{phase:"start"}` 를 낸다.
    #[must_use]
    pub fn begin_turn(&self) -> u64 {
        let turn_id = self.state.begin_turn();
        self.state
            .publish_snapshot(FRAME_TURN, &channel::wire::TurnFrame::started(turn_id));
        turn_id
    }

    /// 턴 종료 — 남은 프롬프트를 거두고, `turn{phase:"end"}` 를 낸 뒤 도는
    /// 턴을 지운다.
    ///
    /// 아직 답을 못 받은 프롬프트는 여기서 은퇴한다. 프롬프트는 제 턴보다
    /// 오래 살 수 없다: 런타임 쪽 responder 는 턴이 죽으며 이미 drop 됐고,
    /// 그것을 남겨 두면 창에는 아무도 받지 않을 모달이 서 있고 등록부는
    /// 프로세스가 끝날 때까지 자란다. 은퇴 프레임을 **먼저** 보내고 나서
    /// 턴 종료를 알린다 — 창이 모달을 내린 뒤에 턴이 끝난 것을 본다.
    pub fn end_turn(&self, turn_id: u64, outcome: TurnOutcome, error: Option<&str>) {
        for prompt_id in self.state.drain_prompts() {
            self.state
                .publish(&channel::wire::PromptResolvedFrame::new(
                    prompt_id,
                    ResolvedBy::Dismissed,
                ));
        }
        self.state.end_turn();
        self.state.publish_snapshot(
            FRAME_TURN,
            &channel::wire::TurnFrame::ended(turn_id, outcome, error),
        );
    }

    /// 이 프롬프트에 IDE 가 답할 때까지 기다린다. 프롬프트가 은퇴하면
    /// (패인이 먼저 답했다) `None`.
    pub async fn answer(&self, prompt_id: u64) -> Option<Answer> {
        self.state.wait_answer(prompt_id).await
    }

    /// [`Self::answer`] 의 권한 전용 판.
    pub async fn permission_decision(
        &self,
        prompt_id: u64,
    ) -> Option<runtime::message_stream::PermissionDecision> {
        match self.state.wait_answer(prompt_id).await? {
            Answer::Permission(decision) => Some(decision),
            Answer::Question(_) => None,
        }
    }

    /// [`Self::answer`] 의 질문 전용 판.
    pub async fn question_answer(&self, prompt_id: u64) -> Option<Vec<String>> {
        match self.state.wait_answer(prompt_id).await? {
            Answer::Question(answers) => Some(answers),
            Answer::Permission(_) => None,
        }
    }

    /// 프롬프트를 등록부에서 빼고 IDE 에 내리라고 알린다.
    ///
    /// `by` 는 누가 답했는지. IDE 는 이 프레임을
    /// 보고 자기 모달을 큐에서 뺀다 — 없으면 패인에서 y 를 누른 뒤에도 창에
    /// 모달이 남아, 이미 은퇴한 `prompt_id` 를 두드리게 된다.
    pub fn retire_prompt(&self, prompt_id: u64, by: ResolvedBy) {
        if self.state.retire_prompt(prompt_id) {
            self.state
                .publish(&channel::wire::PromptResolvedFrame::new(prompt_id, by));
        }
    }

    /// 들어온 명령을 비우고 가져간다(폴링 쪽).
    #[must_use]
    pub fn take_incoming(&self) -> Vec<Command> {
        self.state.take_commands()
    }

    /// 명령 하나가 올 때까지 기다린다(`select!` 갈래 쪽).
    pub async fn next_command(&self) -> Command {
        self.state.wait_command().await
    }

    /// 등록부에 살아 있는 프롬프트 수 — 테스트·진단용.
    #[must_use]
    pub fn live_prompts(&self) -> usize {
        self.state.live_prompts()
    }
}

/// Narrow seam between a session-scoped watcher and the live channel.
///
/// The in-memory implementation in `ide::run_loop` tests keeps the existing
/// serve-road contract independently testable, while the channel-backed
/// implementation below is shared by serve and the interactive TUI.
pub(crate) trait SubagentFrameSink: Send + 'static {
    fn publish_subagents(&self, frame: &SubagentsFrame<'_>);
}

struct LiveSubagentFrameSink {
    state: Arc<ChannelState>,
    active: Mutex<bool>,
}

impl SubagentFrameSink for Arc<LiveSubagentFrameSink> {
    fn publish_subagents(&self, frame: &SubagentsFrame<'_>) {
        let active = self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *active {
            // The roster is a snapshot kind: kept for a late subscriber and
            // stamped with the channel's own `seq`.
            self.state.publish_subagents_snapshot(frame);
        }
    }
}

/// Owns the forwarding task for exactly as long as one frontend session.
pub(crate) struct SubagentFrameRelay {
    task: tokio::task::JoinHandle<()>,
    sink: Arc<LiveSubagentFrameSink>,
    /// The registry the forwarded frames were the whole of, so the final
    /// empty frame can say WHOSE rows it retires (design t-2511 §3: the
    /// `/resume` boundary frame carries the previous registry id). `None`
    /// for the hand-fed test seam, whose frames carry no marks.
    registry: Option<Arc<AgentRegistry>>,
}

impl Drop for SubagentFrameRelay {
    fn drop(&mut self) {
        self.task.abort();
        let mut active = self
            .sink
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if std::mem::replace(&mut *active, false) {
            // A complete empty snapshot retires the old session's rows before
            // `/resume` starts a new watcher on this same socket. Holding the
            // gate through publish makes this the final old-session frame even
            // if the forwarding task was between `changed()` and its sink.
            // It names the OLD registry and a generation past every frame it
            // sent, so a window folds exactly that session's rows and never
            // drops the frame as stale.
            let registry_id = self
                .registry
                .as_ref()
                .and_then(|registry| registry.session_id().map(str::to_string));
            let generation = self
                .registry
                .as_ref()
                .map(|registry| registry.note_roster(std::iter::empty::<String>()));
            self.sink.state.publish_subagents_snapshot(&SubagentsFrame::for_registry(
                &[],
                registry_id.as_deref(),
                generation,
            ));
        }
    }
}

/// Start a full-session watcher when this process has a live events channel.
///
/// The watcher reads through the session's registry and every frame it
/// forwards names that registry and its roster generation.
pub(crate) fn start_subagent_frame_relay(
    ide: Option<&EventsChannel>,
    registry: Arc<AgentRegistry>,
    session_id: String,
) -> Option<SubagentFrameRelay> {
    let ide = ide?;
    let watcher = SubagentProgressWatcher::start_for_session(Arc::clone(&registry), session_id);
    start_subagent_frame_relay_with_watcher(Some(ide), watcher, Some(registry))
}

/// Test seam for driving the real channel with a deterministic watcher.
/// `registry` is what the final empty frame names on drop; `None` sends the
/// unmarked frame a producer without a session registry would.
pub(crate) fn start_subagent_frame_relay_with_watcher(
    ide: Option<&EventsChannel>,
    watcher: SubagentProgressWatcher,
    registry: Option<Arc<AgentRegistry>>,
) -> Option<SubagentFrameRelay> {
    let ide = ide?;
    let sink = Arc::new(LiveSubagentFrameSink {
        state: Arc::clone(&ide.state),
        active: Mutex::new(true),
    });
    let task = tokio::spawn(forward_subagent_frames(watcher, Arc::clone(&sink)));
    Some(SubagentFrameRelay {
        task,
        sink,
        registry,
    })
}

pub(crate) async fn forward_subagent_frames<S: SubagentFrameSink>(
    mut watcher: SubagentProgressWatcher,
    sink: S,
) {
    while let Some(snapshot) = watcher.changed_roster().await {
        let registry = watcher.registry_id();
        sink.publish_subagents(&SubagentsFrame::for_registry(
            &snapshot.agents,
            registry,
            registry.map(|_| snapshot.generation),
        ));
    }
}

fn generate_discovery_token() -> String {
    format!(
        "{:032x}{:032x}",
        rand::random::<u128>(),
        rand::random::<u128>()
    )
}

fn write_discovery_file(
    path: &Path,
    local_addr: SocketAddr,
    token: Option<&str>,
    session_id: &str,
) -> Result<(), String> {
    let token = token.ok_or_else(|| {
        format!(
            "{}: an app-discoverable events channel requires a token",
            path.display()
        )
    })?;
    for (label, value) in [("token", token), ("session id", session_id)] {
        if value.is_empty() || value.contains(['\r', '\n']) {
            return Err(format!(
                "{}: events discovery {label} must be one non-empty line",
                path.display()
            ));
        }
    }
    write_whole(path, &format!("{local_addr}\n{token}\n{session_id}\n"))
}

/// Write `contents` so a reader polling for `path` never sees it half
/// written: staged beside it, synced, renamed into place. The window and the
/// e2e harness read these files the moment they appear, and a plain
/// `fs::write` let them read an empty address.
fn write_whole(path: &Path, contents: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{}: no parent directory", path.display()))?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    staged
        .write_all(contents.as_bytes())
        .and_then(|()| staged.as_file().sync_data())
        .map_err(|error| format!("{}: {error}", path.display()))?;
    staged
        .persist(path)
        .map_err(|error| format!("{}: {}", path.display(), error.error))?;
    Ok(())
}

/// 프로세스에 하나뿐인 채널.
///
/// 전역인 이유는 배선 때문이다. 채널은 tokio 런타임 안에서 열려야 하는데
/// (`main.rs`), 그것을 쓰는 두 프런트엔드(`ide::run_loop`·`tui::app`)는 이미
/// 서명이 잡힌 `run(session, flags)` 로 들어간다. 인자를 하나 더 얹으면 두
/// 프런트엔드와 그 호출부가 함께 바뀌어야 하고, 그 둘은 지금 서로 다른
/// 워커의 손에 있다. 전역 하나면 배선이 **한 줄 추가**로 끝난다.
static CHANNEL: OnceLock<EventsChannel> = OnceLock::new();

/// 열린 채널을 프로세스 전역에 놓는다. 두 번째 호출은 조용히 무시된다.
pub fn install(channel: EventsChannel) {
    let _ = CHANNEL.set(channel);
}

/// 채널이 있으면 빌려준다. headless 실행에 명시 배선이 없으면 `None`.
#[must_use]
pub fn channel() -> Option<&'static EventsChannel> {
    CHANNEL.get()
}

/// IDE 가 보낸 답으로 패인에 파킹된 프롬프트를 해소한다.
///
/// 종류가 어긋나면 아무것도 하지 않고 `false` — 소켓 쪽에서 이미 걸러지지만
/// (`ChannelState::answer_prompt`), 여기서도 조용히 잘못된 답을 흘려보내지
/// 않는다. `true` 면 프롬프트는 소비됐고 화면에서 내려도 된다.
#[must_use]
pub fn resolve_pending(prompt: crate::ide::render::PendingPrompt, answer: &Answer) -> bool {
    use crate::ide::render::PendingPrompt;
    match (prompt, answer) {
        (PendingPrompt::Permission(prompt), Answer::Permission(decision)) => {
            // 진 쪽의 responder 는 값 없이 drop 된다 — 런타임은 그것을 hard
            // deny 로 읽는다. 여기서는 이긴 답을 그대로 실어 보낸다.
            let _ = prompt.responder.send(*decision);
            true
        }
        (PendingPrompt::Question(prompt), Answer::Question(answers)) => {
            let _ = prompt.responder.send(answers.clone());
            true
        }
        _ => false,
    }
}

/// 파킹된 프롬프트에 IDE 모달이 답할 때까지 기다린다.
///
/// 기다릴 것이 없으면 — 채널이 없거나(headless 무배선), 파킹이 없거나, 파킹이
/// 채널에 등록되지 않았으면 — **영원히** 기다린다. `select!` 의 다른 팔이
/// 이기라는 뜻이다: 여기서 즉시 `None` 을 돌려주면 그 팔이 매번 이겨 루프가
/// 바쁜 대기가 된다.
///
/// 종류가 맞는 답만 받는다. 소켓 쪽에서 이미 걸러지지만
/// ([`channel::state::ChannelState::answer_prompt`]), 어긋난 답이 여기까지 와도
/// 파킹을 잘못 닫지 않는다.
pub async fn wait_answer(
    ide: Option<&EventsChannel>,
    waiting: Option<(u64, PromptKind)>,
) -> Option<Answer> {
    let (Some(channel), Some((prompt_id, kind))) = (ide, waiting) else {
        return std::future::pending().await;
    };
    match kind {
        PromptKind::Permission => channel
            .permission_decision(prompt_id)
            .await
            .map(Answer::Permission),
        PromptKind::Question => channel.question_answer(prompt_id).await.map(Answer::Question),
    }
}

/// 채널에 등록된 프롬프트를 은퇴시키고 IDE 에 모달을 내리라고 알린다 —
/// 채널이 열려 있지 않거나(headless 무배선) 등록된 id 가 없으면 아무 일도 없다.
///
/// 부르는 자리가 여섯이다(패인이 먼저 답했다·IDE 가 먼저 답했다·Stop·턴 종료).
/// 그 여섯이 저마다 `if let (Some(channel), Some(id))` 를 다시 쓰면 한 자리가
/// 빠져도 아무도 모른다 — 창에는 아무도 답할 수 없는 모달이 남는다.
pub fn retire_prompt(prompt_id: Option<u64>, by: ResolvedBy) {
    if let (Some(channel), Some(prompt_id)) = (channel(), prompt_id) {
        channel.retire_prompt(prompt_id, by);
    }
}

/// IDE 의 Stop·스티어 명령 하나 — 채널이 없으면 영원히 기다린다
/// ([`wait_answer`] 와 같은 이유다).
pub async fn next_command(ide: Option<&EventsChannel>) -> Command {
    match ide {
        Some(channel) => channel.next_command().await,
        None => std::future::pending().await,
    }
}

/// `session_status` 프레임의 몸통. 카드를 통째로 펼쳐 실어, 창이
/// `frame.model` 처럼 한 겹으로 읽는다 — `usage`/`rate_limit` 프레임을 읽는
/// 방식과 같다(`ui/shell.js::laneSignal`).
#[derive(Serialize)]
struct StatusFrame<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    session: &'a str,
    #[serde(flatten)]
    card: &'a StatusCard,
}

/// Complete running-subagent state for one session.
///
/// This is deliberately a snapshot rather than a delta: reconnect/history
/// replay can apply the same frame repeatedly, and an empty `running` array is
/// the unambiguous all-finished signal.
#[derive(Debug, Serialize)]
pub(crate) struct SubagentsFrame<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    /// The session registry this roster is the whole of (its session id).
    /// A window retires running rows born from THIS registry that the frame
    /// no longer lists, and leaves another registry's rows alone — two zo
    /// sessions sharing a pane, or the `/resume` boundary, no longer read as
    /// every helper finishing at once. Absent only on the hand-fed test seam.
    #[serde(skip_serializing_if = "Option::is_none")]
    registry: Option<&'a str>,
    /// The registry's roster-change counter at the time of the snapshot. A
    /// reader drops a frame older than one it already folded for the same
    /// session and registry — the subscribe replay racing the live stream.
    #[serde(skip_serializing_if = "Option::is_none")]
    generation: Option<u64>,
    running: Vec<RunningSubagent<'a>>,
}

impl<'a> SubagentsFrame<'a> {
    /// A roster that says which registry it is the whole of and how far
    /// along that registry's roster is.
    #[must_use]
    pub(crate) fn for_registry(
        progress: &'a [crate::session::subagent_progress::SubagentProgress],
        registry: Option<&'a str>,
        generation: Option<u64>,
    ) -> Self {
        let running = progress
            .iter()
            .map(|agent| RunningSubagent {
                id: &agent.agent_id,
                label: (!agent.label.trim().is_empty()).then_some(agent.label.as_str()),
                model: agent.model.as_deref(),
                activity: &agent.activity,
                tool_calls: agent.tool_calls,
                started_epoch: agent.started_epoch,
                transcript: agent
                    .transcript_path
                    .as_deref()
                    .and_then(std::path::Path::to_str),
                pane: agent.pane.as_deref(),
                last_receipt: agent.last_receipt.as_ref(),
            })
            .collect();
        Self {
            kind: FRAME_SUBAGENTS,
            registry,
            generation,
            running,
        }
    }
}

#[derive(Debug, Serialize)]
struct RunningSubagent<'a> {
    id: &'a str,
    label: Option<&'a str>,
    model: Option<&'a str>,
    /// What the helper is doing right now — the same observable state the
    /// status line shows (`Read · src/tui/view.rs`), never its static
    /// assignment. A window's roster reads this key by name.
    activity: &'a str,
    /// Tools the helper has started, the count Claude Code writes as
    /// `12 tool uses`. It is what moves while `activity` stands still, so a
    /// reader can tell a working helper from a stuck one.
    tool_calls: u64,
    started_epoch: u64,
    /// The helper's own transcript on disk, once written — see
    /// [`crate::session::subagent_progress::SubagentProgress::transcript_path`].
    transcript: Option<&'a str>,
    /// The pane this helper runs in, when it has one of its own. `null` for a
    /// helper that is a thread of the pane already on screen — a window that
    /// drew a lineage edge from an invented pane id would be drawing the
    /// parent as its own child.
    pane: Option<&'a str>,
    /// What the last `SendMessage` to this helper came to (t-2513 §2.3):
    /// `{receipt, at, reason?, turnId?}`. Absent — and absent from the wire —
    /// until somebody sends; a reader that does not know the key ignores it.
    #[serde(skip_serializing_if = "Option::is_none")]
    last_receipt: Option<&'a runtime::subagent_panes::SteerReceiptRecord>,
}

/// 프롬프트 블록인가 — 맞다면 어떤 종류인가.
const fn prompt_kind_of(block: &RenderBlock) -> Option<PromptKind> {
    match block {
        RenderBlock::PermissionPrompt(_) => Some(PromptKind::Permission),
        RenderBlock::UserQuestionPrompt(_) => Some(PromptKind::Question),
        _ => None,
    }
}

/// 상태 스냅샷 한 장 → 소켓이 답하는 카드.
#[must_use]
pub fn status_card(status: &StatusSnapshot, cwd: &Path) -> StatusCard {
    StatusCard {
        model: status.model.clone(),
        permission_mode: status.permission_mode.to_string(),
        effort: status.effort.map(str::to_string),
        cwd: cwd.to_string_lossy().into_owned(),
        ctx_tokens: status.context_tokens as u64,
        context_window: api::context_window_for_model(&status.model),
        // 이 트리에는 git 조회가 없다. `zo serve` 의 `SessionInfo` 자리를
        // 비워 두되 필드는 지운다 — 없는 값을 지어내지 않는다.
        git_branch: None,
        account: crate::runtime_support::account_facts_for_model(&status.model).map(
            |facts| AccountCard {
                provider: facts.provider.to_string(),
                label: facts.label,
                origin: facts.origin.slug().to_string(),
            },
        ),
        autonomy: status.autonomy.clone(),
        activity: None,
    }
}

/// 재개 재생 항목 → 구독 하이드레이션 히스토리.
///
/// 모양은 `zo serve` 의 `HistoryEntry`(`{role,text}`) 와 같다. 하네스
/// `first_question_in` 이 `role == "user"` 인 첫 줄에서 세션 이름을 읽으므로,
/// 역할 딱지는 장식이 아니라 계약이다.
#[must_use]
pub fn history_from_replay(items: &[ReplayItem]) -> Vec<HistoryEntry> {
    items
        .iter()
        .map(|item| match item {
            ReplayItem::User(text) => HistoryEntry {
                role: "user".to_string(),
                text: text.clone(),
            },
            ReplayItem::Assistant(text) => HistoryEntry {
                role: "assistant".to_string(),
                text: text.clone(),
            },
            ReplayItem::AgentResult {
                label,
                status,
                body,
                ..
            } => {
                let verdict = match status {
                    runtime::message_stream::AgentResultStatus::Completed => "completed",
                    runtime::message_stream::AgentResultStatus::Failed => "failed",
                };
                HistoryEntry {
                    role: "tool".to_string(),
                    text: format!("{label} {verdict}\n{body}"),
                }
            }
            ReplayItem::ToolCall { name, input, .. } => HistoryEntry {
                role: "tool".to_string(),
                text: format!("{name}({input})"),
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::message_stream::{
        BlockId, PermissionChoice, PermissionDecision, PermissionPrompt, ToolCallId,
    };

    fn permission_block(id: u64) -> RenderBlock {
        let (responder, _unused) = tokio::sync::oneshot::channel();
        RenderBlock::PermissionPrompt(PermissionPrompt {
            id: BlockId(id),
            tool_call_id: ToolCallId(String::new()),
            tool_name: "Bash".to_string(),
            reasoning: "run ls".to_string(),
            audit_hint: Some("[y] Allow".to_string()),
            choices: vec![PermissionChoice {
                key: 'y',
                label: "Allow once".to_string(),
                decision: PermissionDecision::AllowOnce,
            }],
            responder,
        })
    }

    async fn open_channel() -> EventsChannel {
        EventsChannel::open(&EventsConfig {
            bind: "127.0.0.1:0".to_string(),
            token: None,
            session_id: "session-under-test".to_string(),
            addr_file: None,
            discovery_file: None,
        })
        .await
        .expect("loopback bind")
    }

    /// r46 의 교훈: 새 필드는 JSON 방출에서 빠지기 쉽다. 카드가 아니라
    /// **프레임의 바이트**를 본다 — 창이 읽는 것이 그것이므로.
    #[test]
    fn the_status_frame_carries_the_account_the_pane_speaks_as() {
        let card = StatusCard {
            model: "claude-opus-5".to_string(),
            permission_mode: "workspace-write".to_string(),
            effort: None,
            cwd: "/tmp/x".to_string(),
            ctx_tokens: 0,
            context_window: 200_000,
            git_branch: None,
            account: Some(AccountCard {
                provider: "anthropic".to_string(),
                label: Some("work".to_string()),
                origin: "ide-managed".to_string(),
            }),
            autonomy: crate::autonomy::AutonomyStatus::default(),
            activity: None,
        };
        let frame = serde_json::to_value(StatusFrame {
            kind: FRAME_SESSION_STATUS,
            session: "s",
            card: &card,
        })
        .expect("a status frame serializes");

        assert_eq!(frame["type"], FRAME_SESSION_STATUS);
        assert_eq!(frame["account"]["provider"], "anthropic");
        assert_eq!(frame["account"]["label"], "work");
        assert_eq!(frame["account"]["origin"], "ide-managed");
    }

    #[test]
    fn session_status_autonomy_frame_bytes_are_stable() {
        let card = StatusCard {
            model: "claude-opus-5".to_string(),
            permission_mode: "read-only".to_string(),
            effort: Some("high".to_string()),
            cwd: "/tmp/x".to_string(),
            ctx_tokens: 7,
            context_window: 200_000,
            git_branch: None,
            account: None,
            autonomy: crate::autonomy::AutonomyStatus {
                goal: Some(crate::autonomy::GoalStatus {
                    phase: "running".to_string(),
                    next_at: Some(42_000),
                    gates_passed: 1,
                    gates_total: 2,
                    action_turns: 3,
                    stalled_turns: 0,
                    pause_reason: None,
                }),
                loops: Vec::new(),
                budget: crate::autonomy::SessionBudgetStatus::default(),
            },
            activity: None,
        };
        let bytes = serde_json::to_vec(&StatusFrame {
            kind: FRAME_SESSION_STATUS,
            session: "s",
            card: &card,
        })
        .expect("serialize");

        assert_eq!(
            String::from_utf8(bytes).expect("utf8"),
            r#"{"type":"session_status","session":"s","model":"claude-opus-5","permission_mode":"read-only","effort":"high","cwd":"/tmp/x","ctx_tokens":7,"context_window":200000,"git_branch":null,"autonomy":{"goal":{"phase":"running","next_at":42000,"gates_passed":1,"gates_total":2,"action_turns":3,"stalled_turns":0},"loops":[],"budget":{"continuations":0,"max_continuations":0,"assistant_turns":0,"max_assistant_turns":0,"output_tokens":0,"max_output_tokens":0,"active_millis":0,"max_wall_clock_secs":0}}}"#
        );
    }

    #[test]
    fn session_status_activity_is_an_additive_compact_fact() {
        let card = StatusCard {
            activity: Some(crate::ide::channel::state::ActivityCard {
                verb: "read".to_string(),
                target: Some("tui/view.rs".to_string()),
                phase: "started".to_string(),
                elapsed_secs: 7,
            }),
            ..StatusCard::default()
        };

        assert_eq!(
            serde_json::to_value(card).expect("status activity serializes")["activity"],
            serde_json::json!({
                "verb": "read",
                "target": "tui/view.rs",
                "phase": "started",
                "elapsed_secs": 7
            })
        );
    }

    #[tokio::test]
    async fn publishing_activity_replaces_only_the_live_status_fact() {
        let channel = open_channel().await;
        let mut seeded = StatusCard {
            model: "claude-opus-5".to_string(),
            cwd: "/tmp/work".to_string(),
            ..StatusCard::default()
        };
        seeded.activity = None;
        channel.state.set_status(seeded);
        channel.publish_activity(Some(ActivityCard {
            verb: "quiet".to_string(),
            target: Some("last: Read tui/view.rs".to_string()),
            phase: "started".to_string(),
            elapsed_secs: 65,
        }));

        let live = channel.state.status();
        assert_eq!(live.model, "claude-opus-5");
        assert_eq!(live.cwd, "/tmp/work");
        assert_eq!(
            live.activity.as_ref().map(|activity| activity.verb.as_str()),
            Some("quiet")
        );

        channel.publish_activity(None);
        assert!(channel.state.status().activity.is_none());
    }

    /// 계정을 모르는 판의 프레임에는 그 열쇠가 아예 없다 — 빈 이름을 실으면
    /// 창의 상태바가 "로그아웃" 을 그린다.
    #[test]
    fn a_pane_with_no_account_leaves_the_key_out_entirely() {
        let card = StatusCard {
            model: "grok-4".to_string(),
            permission_mode: "read-only".to_string(),
            effort: None,
            cwd: "/tmp/x".to_string(),
            ctx_tokens: 0,
            context_window: 200_000,
            git_branch: None,
            account: None,
            autonomy: crate::autonomy::AutonomyStatus::default(),
            activity: None,
        };
        let frame = serde_json::to_value(StatusFrame {
            kind: FRAME_SESSION_STATUS,
            session: "s",
            card: &card,
        })
        .expect("a status frame serializes");

        assert!(
            frame.get("account").is_none(),
            "an unknown account was published as a value: {frame}"
        );
    }

    #[test]
    fn an_interactive_surface_defaults_to_a_pid_discovery_file() {
        let runtime = tempfile::tempdir().expect("runtime directory");
        let expected = runtime.path().join("zo-events-42424.addr");

        let config = EventsConfig::from_inputs(
            None,
            None,
            None,
            None,
            "session-interactive",
            true,
            runtime.path(),
            42_424,
        )
        .expect("interactive surfaces always open a channel");

        assert_eq!(config.bind, "127.0.0.1:0");
        assert!(config.token.as_ref().is_some_and(|token| !token.is_empty()));
        assert_eq!(config.discovery_file.as_deref(), Some(expected.as_path()));
    }

    #[tokio::test]
    async fn a_discovery_file_is_complete_and_removed_with_its_channel() {
        let runtime = tempfile::tempdir().expect("runtime directory");
        let path = runtime.path().join("zo-events-42424.addr");
        let events = EventsChannel::open(&EventsConfig {
            bind: "127.0.0.1:0".to_string(),
            token: Some("private-token".to_string()),
            session_id: "session-discovered".to_string(),
            addr_file: None,
            discovery_file: Some(path.clone()),
        })
        .await
        .expect("open discovered channel");

        assert_eq!(
            std::fs::read_to_string(&path).expect("read discovery record"),
            format!(
                "{}\nprivate-token\nsession-discovered\n",
                events.local_addr()
            )
        );

        drop(events);
        assert!(!path.exists(), "a stopped channel left a live discovery record");
    }

    #[test]
    fn a_headless_surface_without_an_explicit_bind_creates_nothing() {
        let runtime = tempfile::tempdir().expect("runtime directory");

        let config = EventsConfig::from_inputs(
            None,
            None,
            None,
            None,
            "session-headless",
            false,
            runtime.path(),
            42_424,
        );

        assert!(config.is_none());
        assert!(!runtime.path().join("zo-events-42424.addr").exists());
    }

    #[test]
    fn an_explicit_headless_channel_keeps_the_legacy_bind_without_discovery() {
        let runtime = tempfile::tempdir().expect("runtime directory");
        let legacy = runtime.path().join("legacy.addr");

        let config = EventsConfig::from_inputs(
            Some("127.0.0.1:8788"),
            None,
            Some("callers-token".to_string()),
            Some(legacy.clone()),
            "session-headless",
            false,
            runtime.path(),
            42_424,
        )
        .expect("an explicit bind keeps the existing headless API");

        assert_eq!(
            (
                config.bind.as_str(),
                config.token.as_deref(),
                config.addr_file.as_deref(),
                config.discovery_file.as_deref(),
            ),
            (
                "127.0.0.1:8788",
                Some("callers-token"),
                Some(legacy.as_path()),
                None,
            )
        );
    }

    #[test]
    fn a_headless_surface_with_no_bind_anywhere_has_no_channel() {
        // 환경을 만지지 않고도 판정의 절반은 잰다: 플래그가 없고 env 가
        // 비어 있으면 설정이 지어지지 않는다.
        let _lock = crate::test_env_lock();
        let previous = std::env::var(BIND_ENV).ok();
        std::env::remove_var(BIND_ENV);
        assert!(EventsConfig::from_launch(None, "s", false).is_none());
        assert!(EventsConfig::from_launch(Some("127.0.0.1:0"), "s", false).is_some());
        assert!(EventsConfig::from_launch(None, "s", true).is_some());
        if let Some(previous) = previous {
            std::env::set_var(BIND_ENV, previous);
        }
    }

    #[tokio::test]
    async fn a_prompt_frame_carries_a_channel_wide_id_not_the_block_id() {
        let events = open_channel().await;
        // 두 턴이 같은 블록 id 0 을 다시 쓰더라도 채널 id 는 갈린다.
        let first = events.publish(&permission_block(0)).expect("prompt id");
        let second = events.publish(&permission_block(0)).expect("prompt id");
        assert_ne!(first, second);
        assert_eq!(events.live_prompts(), 2);
    }

    #[tokio::test]
    async fn retiring_a_prompt_releases_the_waiter() {
        let events = Arc::new(open_channel().await);
        let prompt_id = events.publish(&permission_block(1)).expect("prompt id");
        let waiting = tokio::spawn({
            let events = Arc::clone(&events);
            async move { events.answer(prompt_id).await }
        });
        events.retire_prompt(prompt_id, ResolvedBy::Pane);
        assert_eq!(waiting.await.expect("join"), None);
        assert_eq!(events.live_prompts(), 0);
    }

    #[tokio::test]
    async fn a_turn_that_ends_takes_its_unanswered_prompts_with_it() {
        let events = open_channel().await;
        let turn = events.begin_turn();
        let prompt_id = events.publish(&permission_block(0)).expect("prompt id");
        assert_eq!(events.live_prompts(), 1);

        events.end_turn(turn, TurnOutcome::Failed, Some("stream broke"));
        assert_eq!(
            events.live_prompts(),
            0,
            "턴보다 오래 사는 프롬프트는 아무도 답할 수 없는 모달이 된다"
        );
        // 은퇴한 뒤에는 기다림이 매달리지 않고 곧장 끝난다.
        assert_eq!(events.answer(prompt_id).await, None);
    }

    /// One TCP subscriber on the real channel, reading frames line by line.
    struct Subscriber {
        lines: tokio::io::Lines<tokio::io::BufReader<tokio::net::tcp::OwnedReadHalf>>,
        _write: tokio::net::tcp::OwnedWriteHalf,
        history: Vec<serde_json::Value>,
    }

    impl Subscriber {
        async fn attach(channel: &EventsChannel, session: &str) -> Self {
            use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
            let stream = tokio::net::TcpStream::connect(channel.local_addr())
                .await
                .expect("connect");
            let (read, mut write) = stream.into_split();
            write
                .write_all(
                    format!(
                        "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"session.subscribe\",\"params\":{{\"id\":\"{session}\"}}}}\n"
                    )
                    .as_bytes(),
                )
                .await
                .expect("subscribe");
            let mut lines = BufReader::new(read).lines();
            let response: serde_json::Value = serde_json::from_str(
                &lines
                    .next_line()
                    .await
                    .expect("read response")
                    .expect("response"),
            )
            .expect("json");
            let history = response["result"]["history"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            Self {
                lines,
                _write: write,
                history,
            }
        }

        async fn next(&mut self) -> serde_json::Value {
            let line = tokio::time::timeout(std::time::Duration::from_secs(2), self.lines.next_line())
                .await
                .expect("frame timeout")
                .expect("read frame")
                .expect("frame");
            serde_json::from_str(&line).expect("frame json")
        }

        fn roster_snapshot(&self) -> Option<&serde_json::Value> {
            self.history
                .iter()
                .rev()
                .find(|frame| frame["type"] == "subagents")
        }
    }

    fn roster(ids: &[&str]) -> Vec<crate::session::subagent_progress::SubagentProgress> {
        ids.iter()
            .enumerate()
            .map(|(at, id)| crate::session::subagent_progress::SubagentProgress {
                agent_id: (*id).to_string(),
                tool_call_id: None,
                label: (*id).to_string(),
                model: None,
                activity: "working".to_string(),
                recent_tools: Vec::new(),
                tool_calls: 0,
                output_tail: String::new(),
                started_epoch: 100 + at as u64,
                elapsed: std::time::Duration::from_secs(1),
                no_new_output_for: None,
                transcript_path: None,
                pane: None,
                last_receipt: None,
            })
            .collect()
    }

    /// t-2511 §3: every roster frame names its registry and generation and
    /// the channel's own `seq`; a subscriber that attaches AFTER a change
    /// gets the latest roster as a snapshot in its history, and a change
    /// after its subscription arrives on the stream with a later `seq` and a
    /// generation no lower than the snapshot's — so replay and live can only
    /// ever disagree in the direction the reader's stale rule resolves. The
    /// last child finishing is an EMPTY snapshot with its own generation,
    /// and a reconnect after that comes back to an empty roster.
    #[tokio::test]
    async fn roster_frames_carry_marks_and_a_reconnect_gets_the_latest_snapshot() {
        use tokio::sync::watch;
        use crate::session::subagent_progress::{RosterSnapshot, SubagentProgressWatcher};

        let store = tempfile::tempdir().expect("store");
        let registry = AgentRegistry::at_root_for_tests("session-under-test", store.path());
        let channel = open_channel().await;
        let (updates, receiver) = watch::channel(RosterSnapshot::default());
        let relay = start_subagent_frame_relay_with_watcher(
            Some(&channel),
            SubagentProgressWatcher::from_receiver_for_registry(receiver, "session-under-test"),
            Some(Arc::clone(&registry)),
        )
        .expect("relay");
        let mut early = Subscriber::attach(&channel, "session-under-test").await;
        assert!(early.roster_snapshot().is_none(), "nothing has been published yet");

        // A → child1: the first roster.
        let generation = registry.note_roster(["a1"]);
        updates
            .send(RosterSnapshot {
                agents: roster(&["a1"]),
                generation,
            })
            .expect("send");
        let frame = early.next().await;
        assert_eq!(frame["type"], "subagents");
        assert_eq!(frame["registry"], "session-under-test");
        assert_eq!(frame["generation"], 1);
        assert_eq!(frame["seq"], 1);
        assert_eq!(frame["running"][0]["id"], "a1");

        // Reconnect: the late subscriber's history ends with that roster.
        let mut late = Subscriber::attach(&channel, "session-under-test").await;
        let snapshot = late.roster_snapshot().expect("the roster rides the history").clone();
        assert_eq!(snapshot["generation"], 1);
        assert_eq!(snapshot["seq"], 1);
        assert_eq!(snapshot["running"][0]["id"], "a1");

        // B → child2: a change after the subscription streams, later than
        // the snapshot in both counters.
        let generation = registry.note_roster(["a1", "b1"]);
        updates
            .send(RosterSnapshot {
                agents: roster(&["a1", "b1"]),
                generation,
            })
            .expect("send");
        let frame = late.next().await;
        assert_eq!(frame["generation"], 2);
        assert_eq!(frame["seq"], 2);
        assert!(frame["seq"].as_u64() > snapshot["seq"].as_u64());
        assert!(frame["generation"].as_u64() >= snapshot["generation"].as_u64());
        let ids: Vec<&str> = frame["running"]
            .as_array()
            .expect("running")
            .iter()
            .map(|row| row["id"].as_str().expect("id"))
            .collect();
        assert_eq!(ids, ["a1", "b1"]);
        // The early subscriber saw the same delta, in the same order.
        assert_eq!(early.next().await["generation"], 2);

        // Everyone finished: an empty snapshot with a generation of its own.
        let generation = registry.note_roster(Vec::<String>::new());
        updates
            .send(RosterSnapshot {
                agents: Vec::new(),
                generation,
            })
            .expect("send");
        let frame = late.next().await;
        assert_eq!(frame["generation"], 3);
        assert_eq!(frame["running"], serde_json::json!([]));
        let after = Subscriber::attach(&channel, "session-under-test").await;
        let snapshot = after.roster_snapshot().expect("the empty roster is the snapshot");
        assert_eq!(snapshot["generation"], 3);
        assert_eq!(snapshot["running"], serde_json::json!([]));

        drop(relay);
        drop(updates);
    }

    /// t-2511 §3, the `/resume` boundary: the frame the old relay sends on
    /// drop names the OLD registry (so a window folds only that session's
    /// rows) with a generation past every frame it sent, and the new
    /// session's relay names its own registry from its first frame. The
    /// snapshot a late subscriber gets is the new session's.
    #[tokio::test]
    async fn the_resume_boundary_frame_names_the_old_registry_and_the_new_relay_its_own() {
        use tokio::sync::watch;
        use crate::session::subagent_progress::{RosterSnapshot, SubagentProgressWatcher};

        let store = tempfile::tempdir().expect("store");
        let old = AgentRegistry::at_root_for_tests("session-old", store.path());
        let new = AgentRegistry::at_root_for_tests("session-new", store.path());
        let channel = open_channel().await;
        let mut subscriber = Subscriber::attach(&channel, "session-under-test").await;

        let (old_updates, receiver) = watch::channel(RosterSnapshot::default());
        let old_relay = start_subagent_frame_relay_with_watcher(
            Some(&channel),
            SubagentProgressWatcher::from_receiver_for_registry(receiver, "session-old"),
            Some(Arc::clone(&old)),
        )
        .expect("old relay");
        let generation = old.note_roster(["a1"]);
        old_updates
            .send(RosterSnapshot {
                agents: roster(&["a1"]),
                generation,
            })
            .expect("send");
        let frame = subscriber.next().await;
        assert_eq!(frame["registry"], "session-old");
        assert_eq!(frame["generation"], 1);

        // `/resume`: the old relay goes first.
        drop(old_relay);
        let boundary = subscriber.next().await;
        assert_eq!(boundary["registry"], "session-old", "the boundary frame names the OLD registry");
        assert_eq!(boundary["running"], serde_json::json!([]));
        assert_eq!(boundary["generation"], 2, "past the last frame it sent, so never stale");

        let (new_updates, receiver) = watch::channel(RosterSnapshot::default());
        let new_relay = start_subagent_frame_relay_with_watcher(
            Some(&channel),
            SubagentProgressWatcher::from_receiver_for_registry(receiver, "session-new"),
            Some(Arc::clone(&new)),
        )
        .expect("new relay");
        let generation = new.note_roster(["b1"]);
        new_updates
            .send(RosterSnapshot {
                agents: roster(&["b1"]),
                generation,
            })
            .expect("send");
        let frame = subscriber.next().await;
        assert_eq!(frame["registry"], "session-new");
        assert_eq!(frame["generation"], 1, "the new registry counts from its own zero");
        assert_eq!(frame["running"][0]["id"], "b1");

        let late = Subscriber::attach(&channel, "session-under-test").await;
        let snapshot = late.roster_snapshot().expect("snapshot");
        assert_eq!(snapshot["registry"], "session-new");
        assert_eq!(snapshot["running"][0]["id"], "b1");
        drop(new_relay);
    }

    /// A relay fed without a registry — the hand-driven seam — sends the
    /// unmarked frames a pre-registry producer sent, marks and all absent.
    #[tokio::test]
    async fn an_unregistered_relay_sends_unmarked_frames() {
        use tokio::sync::watch;
        use crate::session::subagent_progress::{RosterSnapshot, SubagentProgressWatcher};

        let channel = open_channel().await;
        let mut subscriber = Subscriber::attach(&channel, "session-under-test").await;
        let (updates, receiver) = watch::channel(RosterSnapshot::default());
        let relay = start_subagent_frame_relay_with_watcher(
            Some(&channel),
            SubagentProgressWatcher::from_receiver(receiver),
            None,
        )
        .expect("relay");
        updates
            .send(RosterSnapshot {
                agents: roster(&["a1"]),
                generation: 9,
            })
            .expect("send");
        let frame = subscriber.next().await;
        assert!(frame.get("registry").is_none());
        assert!(frame.get("generation").is_none(), "no registry, no generation to claim");
        assert_eq!(frame["seq"], 1, "the channel counter is the channel's, not the registry's");
        drop(relay);
        let boundary = subscriber.next().await;
        assert_eq!(boundary, serde_json::json!({"type": "subagents", "running": [], "seq": 2}));
    }

    #[test]
    fn history_keeps_the_role_the_harness_reads() {
        let history = history_from_replay(&[
            ReplayItem::Assistant("나는 답이다".to_string()),
            ReplayItem::User("첫 물음".to_string()),
        ]);
        let value = serde_json::to_value(&history).expect("serialize");
        assert_eq!(
            zerocode_first_question(&value).as_deref(),
            Some("첫 물음"),
            "역할 딱지가 없으면 세션 이름이 어시스턴트 말로 잡힌다"
        );
    }

    /// 하네스 `first_question_in` 의 규칙(역할이 user 인 첫 줄)을 그대로 두고
    /// 잰다 — 하네스 크레이트는 dev-dep 이라 통합 테스트에서만 링크된다.
    fn zerocode_first_question(history: &serde_json::Value) -> Option<String> {
        history.as_array()?.iter().find_map(|entry| {
            let object = entry.as_object()?;
            if object.get("role").and_then(serde_json::Value::as_str)? != "user" {
                return None;
            }
            object
                .get("text")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
    }
}
