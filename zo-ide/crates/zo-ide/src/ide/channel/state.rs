//! 채널이 붙들고 있는 것 — 프레임 팬아웃·프롬프트 등록부·들어온 명령.
//!
//! 소켓 쪽([`super::server`])과 프런트엔드 쪽([`crate::ide::events`])이 이
//! 하나를 `Arc` 로 나눠 쥔다. 세션(`PlainSession`)은 여기 들어오지 않는다:
//! 턴이 도는 동안 세션은 턴 future 가 소유하고 있어, 소켓 핸들러가 그것을
//! 잠그려 들면 답이 턴을 기다리게 된다. 대신 프런트엔드가 상태 카드와
//! 히스토리를 **밀어 넣고**, 소켓은 마지막으로 밀린 값만 읽는다.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use runtime::message_stream::PermissionDecision;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, watch, Notify};

/// 구독자가 뒤처졌을 때 버퍼가 붙드는 줄 수. 한 턴의 델타는 이보다 훨씬
/// 많지만, 뒤처진 구독자에게 중요한 것은 최신 상태이지 놓친 델타가 아니다 —
/// `Lagged` 는 건너뛰고 계속 읽는다([`super::server`]).
const FRAME_BUFFER: usize = 1024;

/// 프런트엔드가 소켓에 답을 넘길 때 쓰는 어휘.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// 권한 모달의 결정.
    Permission(PermissionDecision),
    /// 질문 모달이 고른 라벨(또는 자유 텍스트) 목록.
    Question(Vec<String>),
}

/// 답이 프롬프트에 닿지 못한 이유. 코드가 아니라 **사정**을 돌려주는 것은
/// 상태가 JSON-RPC 를 알 이유가 없기 때문이다 — 코드로 낮추는 일은
/// [`super::server`] 한 곳에서만 한다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnswerRefusal {
    /// 그런 프롬프트가 없다 — 처음부터 없었거나, 이미 답을 받고 은퇴했다.
    NotWaiting,
    /// 프롬프트는 있는데 답의 종류가 다르다.
    WrongKind,
}

/// 등록된 프롬프트의 종류 — `permission.respond` 가 질문 프롬프트를,
/// `question.respond` 가 권한 프롬프트를 해소하지 못하게 막는다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    /// 권한 승인 요청.
    Permission,
    /// 툴이 사람에게 던진 질문.
    Question,
}

impl PromptKind {
    /// 이 종류의 프롬프트가 받을 수 있는 답인가.
    #[must_use]
    pub const fn accepts(self, answer: &Answer) -> bool {
        matches!(
            (self, answer),
            (PromptKind::Permission, Answer::Permission(_))
                | (PromptKind::Question, Answer::Question(_))
        )
    }
}

/// IDE 가 채널로 보내온, 프런트엔드가 처리해야 할 것.
///
/// 프롬프트 답은 여기 없다 — 그것은 프롬프트 등록부로 곧장 가고, 파킹된
/// 프롬프트를 쥔 쪽이 [`ChannelState::wait_answer`] 로 받는다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// IDE 의 Stop 버튼. `turn_id` 가 실려 있으면 그 턴에 대한 것이다.
    CancelTurn {
        /// 클라이언트가 지목한 턴(없으면 지금 도는 턴).
        turn_id: Option<u64>,
    },
    /// 도는 턴에 끼워 넣을 사람의 말.
    Steer {
        /// 넣을 텍스트.
        text: String,
    },
    /// A successful `auth.reload` whose display label should be shown once by
    /// whichever frontend currently owns the pane.
    AccountSwitch {
        label: String,
    },
    /// The parent asked this teammate to leave (`teammate.close`, t-2513
    /// §2.2). An idle teammate writes its closing document and exits; one
    /// mid-turn cancels the turn first. A root session never receives it —
    /// the server refuses the method unless the pane declared itself a
    /// teammate.
    Close {
        /// The parent's word for why, carried into `result-final.json`.
        reason: String,
    },
}

/// The parent side of `auth.reload`: told the params the window sent, it
/// carries them to every live pane child (t-2513 §2.6). Installed by the host
/// that knows the children — the frontend with the session's registry — and
/// run off the socket loop, so a slow child never delays the window's answer.
pub type ChildFanout = Arc<dyn Fn(&serde_json::Value) + Send + Sync>;

/// The parent side of `mcp.call`: one named MCP tool call routed to this
/// session's MCP runtime (the same dispatch the inline passthrough uses), or
/// an error rendered as text. Installed by the session host; absent when the
/// session has no MCP servers, in which case the method fails by name.
pub type McpBridge = Arc<dyn Fn(&str, &serde_json::Value) -> Result<String, String> + Send + Sync>;

/// `session.info` / `session_status` 프레임이 읽는 한 장.
///
/// 프런트엔드가 밀어 넣는다 — 세션을 잠그지 않고도 소켓이 답할 수 있도록.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusCard {
    /// 활성 모델 id.
    pub model: String,
    /// 정규화된 권한 라벨(`read-only`/`workspace-write`/`danger-full-access`).
    pub permission_mode: String,
    /// effort 라벨. 없으면 `None`.
    pub effort: Option<String>,
    /// 세션의 작업 디렉터리.
    pub cwd: String,
    /// 추정 컨텍스트 토큰.
    pub ctx_tokens: u64,
    /// 모델의 컨텍스트 창 크기.
    pub context_window: u64,
    /// 이 트리에는 git 조회가 없다 — 언제나 `None` 이고, 하네스/서브스크라이버
    /// 쪽 스키마를 맞추려고 자리만 지킨다.
    pub git_branch: Option<String>,
    /// 이 세션의 모델이 말을 거는 계정 — 창의 상태바가 그대로 보여 줄 수 있게
    /// (`docs/design/zo-ide-account-oauth.md` §2.3).
    ///
    /// 계정 저장소가 없는 프로바이더(xAI·Ollama)와 아무 자격도 못 찾은 판은
    /// `None` 이고, 프레임에서 그 열쇠는 통째로 빠진다 — 모르는 것을 빈
    /// 이름으로 그리면 창은 "로그아웃" 으로 읽는다.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub account: Option<AccountCard>,
    /// Persistent goal/loop controller state for board and sidebar consumers.
    #[serde(default)]
    pub autonomy: crate::autonomy::AutonomyStatus,
    /// What the live Working row says now. Omitted while the first token is
    /// still inside the normal grace period and whenever the pane is idle.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub activity: Option<ActivityCard>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityCard {
    pub verb: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub target: Option<String>,
    pub phase: String,
    pub elapsed_secs: u64,
}

/// `session_status.account` 의 몸통.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountCard {
    /// `anthropic` | `openai` | `google`.
    pub provider: String,
    /// 창이 `auth.reload` 에 실어 보낸 표시 이름. 창이 말해 주지 않았으면
    /// `None` — 이메일이나 계정 id 를 zo 가 지어내지 않는다.
    pub label: Option<String>,
    /// `ide-managed` | `own-login` | `keychain` | `env`.
    pub origin: String,
}

/// `session.subscribe` 가 돌려주는 히스토리 한 줄. `zo serve` 의
/// `HistoryEntry` 와 같은 모양이라 하네스 `first_question_in` 이 세션 이름을
/// 여기서 읽어 낸다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// `system` | `user` | `assistant` | `tool`.
    pub role: String,
    /// 납작하게 편 본문.
    pub text: String,
}

/// 등록된 프롬프트 하나 — 답이 오면 `watch` 로 기다리는 모두에게 알린다.
///
/// oneshot 이 아닌 이유는 둘이다. 답 하나를 **둘**이 볼 수 있어야 하고(파킹된
/// 프롬프트를 푸는 쪽과 자기 모달을 내리는 쪽), 아무도 아직 기다리지 않는
/// 사이에 도착한 답도 **남아 있어야** 한다 — oneshot 은 받는 쪽이 없으면
/// 답을 잃는다.
struct PromptSlot {
    kind: PromptKind,
    answer: watch::Sender<Option<Answer>>,
}

/// The kinds whose LATEST frame the channel keeps for a late subscriber, in
/// the order `session.subscribe` appends them after the replay history —
/// the order the window hydrates them in (`note_zo_session_history`).
pub const SNAPSHOT_KINDS: [&str; 4] = [
    super::wire::FRAME_SESSION_STATUS,
    super::wire::FRAME_TURN,
    super::wire::FRAME_SUBAGENTS,
    super::wire::FRAME_SESSION_CAPABILITIES,
];

/// 채널의 공유 상태.
pub struct ChannelState {
    session_id: String,
    frames: broadcast::Sender<Arc<String>>,
    status: Mutex<StatusCard>,
    capabilities: Mutex<super::capabilities::Snapshot>,
    history: Mutex<Vec<HistoryEntry>>,
    /// The last frame of each snapshot kind (`session_status`, `turn`,
    /// `subagents`), as the bytes it went out as. A subscriber that attaches
    /// after the fact gets them appended to its history so its status bar,
    /// turn indicator and helper roster come back without waiting for the
    /// next change (design t-2511 §3). Replaced, never appended: a snapshot
    /// is the whole state of its kind.
    snapshots: Mutex<HashMap<&'static str, String>>,
    prompts: Mutex<HashMap<u64, PromptSlot>>,
    next_prompt_id: AtomicU64,
    commands: Mutex<VecDeque<Command>>,
    commands_arrived: Notify,
    turn: Mutex<Option<u64>>,
    next_turn_id: AtomicU64,
    seq: AtomicU64,
    /// How many `session.capabilities` requests were answered. A refused
    /// exact launch holds its channel until this is nonzero or a deadline
    /// passes, so the host reads the verdict rather than a closed socket.
    capabilities_reads: AtomicU64,
    /// This pane is a teammate (t-2513): `session.steer` with no turn running
    /// opens its NEXT turn instead of being refused, and `teammate.close` is
    /// a method it answers. Off for a root session, whose idle composer is
    /// the person's.
    idle_steer: std::sync::atomic::AtomicBool,
    /// Where `auth.reload` fans out to, when this pane has children.
    child_fanout: Mutex<Option<ChildFanout>>,
    /// Where `mcp.call` is answered, when this session has MCP servers.
    mcp_bridge: Mutex<Option<McpBridge>>,
}

impl ChannelState {
    /// 빈 상태 하나. `session_id` 는 `session.list`/`info` 가 답하는 이름이다.
    #[must_use]
    pub fn new(session_id: String) -> Self {
        let (frames, _first) = broadcast::channel(FRAME_BUFFER);
        Self {
            capabilities: Mutex::new(super::capabilities::Snapshot::pending(&session_id)),
            session_id,
            frames,
            status: Mutex::new(StatusCard::default()),
            history: Mutex::new(Vec::new()),
            snapshots: Mutex::new(HashMap::new()),
            prompts: Mutex::new(HashMap::new()),
            next_prompt_id: AtomicU64::new(1),
            commands: Mutex::new(VecDeque::new()),
            commands_arrived: Notify::new(),
            turn: Mutex::new(None),
            next_turn_id: AtomicU64::new(1),
            seq: AtomicU64::new(0),
            capabilities_reads: AtomicU64::new(0),
            idle_steer: std::sync::atomic::AtomicBool::new(false),
            child_fanout: Mutex::new(None),
            mcp_bridge: Mutex::new(None),
        }
    }

    /// Declare this pane a teammate: an idle steer opens the next turn, and
    /// `teammate.close` is answered.
    pub fn set_idle_steer(&self, accepted: bool) {
        self.idle_steer.store(accepted, Ordering::Relaxed);
    }

    /// Whether a steer with no running turn opens the next one.
    #[must_use]
    pub fn accepts_idle_steer(&self) -> bool {
        self.idle_steer.load(Ordering::Relaxed)
    }

    /// Install where `auth.reload` fans out to.
    pub fn set_child_fanout(&self, fanout: Option<ChildFanout>) {
        *self.child_fanout.lock().unwrap_or_else(PoisonError::into_inner) = fanout;
    }

    /// The installed fan-out, if any.
    #[must_use]
    pub fn child_fanout(&self) -> Option<ChildFanout> {
        self.child_fanout
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Install where `mcp.call` is answered.
    pub fn set_mcp_bridge(&self, bridge: Option<McpBridge>) {
        *self.mcp_bridge.lock().unwrap_or_else(PoisonError::into_inner) = bridge;
    }

    /// The installed bridge, if any.
    #[must_use]
    pub fn mcp_bridge(&self) -> Option<McpBridge> {
        self.mcp_bridge
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// One `session.capabilities` answer went out.
    pub fn note_capabilities_read(&self) {
        self.capabilities_reads.fetch_add(1, Ordering::Relaxed);
    }

    /// How many `session.capabilities` answers went out so far.
    #[must_use]
    pub fn capabilities_reads(&self) -> u64 {
        self.capabilities_reads.load(Ordering::Relaxed)
    }

    /// 이 채널이 말하는 세션의 id.
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// 지금까지 흘려보낸 프레임 수 — `session.subscribe` 의 `next_seq`.
    #[must_use]
    pub fn next_seq(&self) -> u64 {
        self.seq.load(Ordering::Relaxed)
    }

    /// 새 구독자. 응답을 쓰기 **전에** 잡아야 그 사이 프레임이 새지 않는다.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<String>> {
        self.frames.subscribe()
    }

    /// How many subscribers are reading frames right now. The receiver
    /// `broadcast::channel` hands back at birth is dropped there, so a channel
    /// nobody subscribed to counts zero.
    #[must_use]
    pub fn subscriber_count(&self) -> usize {
        self.frames.receiver_count()
    }

    /// 직렬화가 끝난 한 줄을 모든 구독자에게. 구독자가 없으면 조용히
    /// 버려진다 — 맨 터미널에서 채널이 열려 있기만 한 상태가 정상이다.
    ///
    /// 줄 하나를 [`Arc`] 로 나눠 준다: 구독자가 둘이어도 문자열은 하나다.
    pub fn publish_line(&self, line: String) {
        self.seq.fetch_add(1, Ordering::Relaxed);
        let _ = self.frames.send(Arc::new(line));
    }

    /// 직렬화할 수 있는 것 하나를 프레임으로. 직렬화가 실패하면 조용히
    /// 버린다 — 프레임 한 장 때문에 턴이 멈출 이유는 없다.
    pub fn publish<T: serde::Serialize>(&self, frame: &T) {
        if let Ok(line) = serde_json::to_string(frame) {
            self.publish_line(line);
        }
    }

    /// A frame that is the WHOLE state of its kind: published like any other
    /// line and kept as the kind's latest snapshot, so `session.subscribe`
    /// can hand it to a late subscriber. `kind` must be the frame's `type`.
    pub fn publish_snapshot<T: serde::Serialize>(&self, kind: &'static str, frame: &T) {
        if let Ok(line) = serde_json::to_string(frame) {
            self.snapshots
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(kind, line.clone());
            self.publish_line(line);
        }
    }

    /// The `subagents` snapshot: like [`Self::publish_snapshot`], plus the
    /// channel's own frame sequence number stamped onto the frame as `seq`
    /// — the transport's counter, kept apart from the registry `generation`
    /// (roster changes) and an agent's `run_generation` (resumes), which the
    /// frame carries from the registry. The number is taken and the line
    /// sent under one increment, so `seq` is exactly this frame's position.
    pub fn publish_subagents_snapshot<T: serde::Serialize>(&self, frame: &T) {
        let Ok(mut value) = serde_json::to_value(frame) else {
            return;
        };
        let seq = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
        if let Some(object) = value.as_object_mut() {
            object.insert("seq".to_string(), serde_json::Value::from(seq));
        }
        let Ok(line) = serde_json::to_string(&value) else {
            return;
        };
        self.snapshots
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(super::wire::FRAME_SUBAGENTS, line.clone());
        let _ = self.frames.send(Arc::new(line));
    }

    /// The latest snapshot of `kind`, as its wire bytes.
    #[must_use]
    pub fn snapshot(&self, kind: &str) -> Option<String> {
        self.snapshots
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(kind)
            .cloned()
    }

    /// What `session.subscribe` hands a new subscriber: the replay history
    /// (`{role,text}` entries) followed by the latest snapshot of each kind in
    /// [`SNAPSHOT_KINDS`] order, as frame-shaped objects carrying `type`.
    ///
    /// Read AFTER the subscription is taken (`server::dispatch`), so a change
    /// that lands between the two is in the snapshot or in the stream or in
    /// both — never in neither. A duplicate is harmless: every snapshot kind
    /// is idempotent, and the `subagents` generation lets the reader drop the
    /// older of two.
    #[must_use]
    pub fn hydration_history(&self) -> Vec<serde_json::Value> {
        let mut history: Vec<serde_json::Value> = self
            .history()
            .into_iter()
            .filter_map(|entry| serde_json::to_value(entry).ok())
            .collect();
        let snapshots = self
            .snapshots
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        for kind in SNAPSHOT_KINDS {
            if let Some(frame) = snapshots
                .get(kind)
                .and_then(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            {
                history.push(frame);
            }
        }
        history
    }

    /// Clone only: the request handler never performs discovery or selection.
    #[must_use]
    pub fn capabilities(&self) -> super::capabilities::Snapshot {
        self.capabilities.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Serialize updates under the receipt lock so a slower producer cannot
    /// publish an older revision after a newer one. Return false on a no-op.
    pub fn update_capabilities(&self, update: impl FnOnce(&mut super::capabilities::Snapshot)) -> bool {
        let mut held = self.capabilities.lock().unwrap_or_else(PoisonError::into_inner);
        let mut next = held.clone();
        update(&mut next);
        next.revision = held.revision;
        if next == *held { return false; }
        if serde_json::to_value(&next).ok() == serde_json::to_value(&*held).ok() {
            *held = next;
            return false;
        }
        next.revision += 1;
        if let Some(current) = next.current.as_mut() {
            current["revision"] = serde_json::json!(next.revision);
        }
        let Ok(mut frame) = serde_json::to_value(&next) else { return false; };
        frame["type"] = serde_json::json!(super::wire::FRAME_SESSION_CAPABILITIES);
        // Reserve room for the channel sequence number before advancing it.
        if frame.to_string().len() + 40 > super::capabilities::MAX_BYTES { return false; }
        frame["seq"] = serde_json::json!(self.seq.fetch_add(1, Ordering::Relaxed) + 1);
        let line = frame.to_string();
        self.snapshots.lock().unwrap_or_else(PoisonError::into_inner)
            .insert(super::wire::FRAME_SESSION_CAPABILITIES, line.clone());
        *held = next;
        let _ = self.frames.send(Arc::new(line));
        true
    }

    /// 상태 카드를 갈아 끼운다.
    pub fn set_status(&self, status: StatusCard) {
        *self.status.lock().unwrap_or_else(PoisonError::into_inner) = status;
    }

    /// 마지막으로 밀린 상태 카드.
    #[must_use]
    pub fn status(&self) -> StatusCard {
        self.status
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// 하이드레이션 히스토리를 갈아 끼운다.
    pub fn set_history(&self, history: Vec<HistoryEntry>) {
        *self.history.lock().unwrap_or_else(PoisonError::into_inner) = history;
    }

    /// 마지막으로 밀린 히스토리.
    #[must_use]
    pub fn history(&self) -> Vec<HistoryEntry> {
        self.history
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// 프롬프트 하나를 등록하고 채널 전역 `prompt_id` 를 준다.
    ///
    /// 블록 id 를 그대로 쓰지 않는 이유: [`runtime::message_stream::BlockIdGen`]
    /// 은 턴마다 0 에서 다시 세므로 턴이 바뀌면 같은 번호가 또 나온다. `zo
    /// serve` 가 `SocketPrompterConfig::next_id` 로 서버 전역 단조 id 를 따로
    /// 두는 것과 같은 이유다.
    #[must_use]
    pub fn register_prompt(&self, kind: PromptKind) -> u64 {
        let prompt_id = self.next_prompt_id.fetch_add(1, Ordering::Relaxed);
        let (answer, _first) = watch::channel(None);
        self.prompts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(prompt_id, PromptSlot { kind, answer });
        prompt_id
    }

    /// 소켓에서 온 답을 등록부에 넣는다.
    ///
    /// # Errors
    ///
    /// [`AnswerRefusal`] — 그런 프롬프트가 없거나(이미 답을 받았거나),
    /// 답의 종류가 프롬프트의 종류와 다르다.
    pub fn answer_prompt(&self, prompt_id: u64, answer: Answer) -> Result<(), AnswerRefusal> {
        let prompts = self.prompts.lock().unwrap_or_else(PoisonError::into_inner);
        let slot = prompts.get(&prompt_id).ok_or(AnswerRefusal::NotWaiting)?;
        if !slot.kind.accepts(&answer) {
            return Err(AnswerRefusal::WrongKind);
        }
        // 먼저 온 답이 이긴다: 이미 값이 들어 있으면 덮지 않는다.
        if slot.answer.borrow().is_some() {
            return Err(AnswerRefusal::NotWaiting);
        }
        // `send` 가 아니라 `send_replace`: `send` 는 살아 있는 receiver 가
        // 하나도 없으면 값을 **버리고** Err 로 끝난다. 답이 프런트엔드가
        // 기다리기 시작하기 전에 도착하는 것은 흔한 순서이고(창이 패인보다
        // 빠를 수 있다), 그때 답이 사라지면 두 번째 답이 이기게 된다.
        slot.answer.send_replace(Some(answer));
        Ok(())
    }

    /// 이 프롬프트에 IDE 가 답할 때까지 기다린다. 프롬프트가 은퇴하면
    /// (패인이 먼저 답했다) `None`.
    pub async fn wait_answer(&self, prompt_id: u64) -> Option<Answer> {
        let mut receiver = {
            let prompts = self.prompts.lock().unwrap_or_else(PoisonError::into_inner);
            prompts.get(&prompt_id)?.answer.subscribe()
        };
        // 등록부에서 슬롯이 빠지면 sender 가 drop 되고 `wait_for` 가 Err 로
        // 끝난다 — 그것이 "패인이 이겼다" 의 신호다.
        let answer = receiver.wait_for(Option::is_some).await.ok()?;
        answer.clone()
    }

    /// 프롬프트를 등록부에서 뺀다 — 이후의 `permission.respond` 는 거절된다.
    /// 이미 없던 id 였으면 `false`.
    pub fn retire_prompt(&self, prompt_id: u64) -> bool {
        self.prompts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&prompt_id)
            .is_some()
    }

    /// 아직 답을 못 받은 프롬프트를 **전부** 등록부에서 빼고 그 번호들을
    /// 돌려준다. 턴이 끝나는 자리에서 부른다 — 프롬프트는 제 턴보다 오래
    /// 살 수 없다. 기다리던 쪽은 sender 가 drop 되며 함께 풀린다.
    #[must_use]
    pub fn drain_prompts(&self) -> Vec<u64> {
        self.prompts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain()
            .map(|(prompt_id, _slot)| prompt_id)
            .collect()
    }

    /// 지금 등록돼 있는 프롬프트 수(테스트·진단용).
    #[must_use]
    pub fn live_prompts(&self) -> usize {
        self.prompts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    /// 소켓에서 온 명령을 큐에 넣고 기다리는 쪽을 깨운다.
    pub fn push_command(&self, command: Command) {
        self.commands
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push_back(command);
        self.commands_arrived.notify_waiters();
    }

    /// 큐를 비우고 가져간다.
    #[must_use]
    pub fn take_commands(&self) -> Vec<Command> {
        self.commands
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain(..)
            .collect()
    }

    /// 명령 하나가 올 때까지 기다린다.
    pub async fn wait_command(&self) -> Command {
        loop {
            // 기다림을 **먼저** 등록한다 — 확인과 대기 사이에 들어온 명령이
            // 알림을 놓치는 창을 없앤다.
            let arrived = self.commands_arrived.notified();
            if let Some(command) = self
                .commands
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .pop_front()
            {
                return command;
            }
            arrived.await;
        }
    }

    /// 새 턴 번호를 발급하고 도는 턴으로 기록한다.
    #[must_use]
    pub fn begin_turn(&self) -> u64 {
        let turn_id = self.next_turn_id.fetch_add(1, Ordering::Relaxed);
        *self.turn.lock().unwrap_or_else(PoisonError::into_inner) = Some(turn_id);
        turn_id
    }

    /// 도는 턴을 지운다.
    pub fn end_turn(&self) {
        *self.turn.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    /// 지금 도는 턴 번호.
    #[must_use]
    pub fn turn(&self) -> Option<u64> {
        *self.turn.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_revisions_dedupe_and_hydrate_after_the_legacy_trio() {
        let state = ChannelState::new("endpoint".into());
        state.set_history(vec![HistoryEntry { role:"user".into(), text:"hello".into() }]);
        for kind in super::SNAPSHOT_KINDS.iter().take(3) {
            state.publish_snapshot(kind, &serde_json::json!({"type":kind}));
        }
        let mut stream = state.subscribe();
        assert!(state.update_capabilities(|s| s.identity["session_id"] = serde_json::json!("active")));
        let first = state.capabilities();
        let frame: serde_json::Value = serde_json::from_str(&stream.try_recv().unwrap()).unwrap();
        assert_eq!(frame["revision"], 1);
        assert_eq!(frame["seq"], 4);
        assert!(!state.update_capabilities(|s| s.identity["session_id"] = serde_json::json!("active")));
        assert!(stream.try_recv().is_err());
        assert_eq!(state.next_seq(),4);
        let history = state.hydration_history();
        assert_eq!(history[0]["text"], "hello");
        assert_eq!(history[4],frame);
        assert_eq!(history[1]["type"],"session_status");
        assert_eq!(history[2]["type"],"turn");
        assert_eq!(history[3]["type"],"subagents");
        assert!(state.update_capabilities(|s| s.identity["session_id"] = serde_json::json!("resumed")));
        assert_eq!(state.capabilities().revision,2);
        assert_eq!(state.capabilities().process,first.process);
        assert_eq!(state.capabilities().identity["channel_session_id"],"endpoint");
        let replacement = ChannelState::new("endpoint".into()).capabilities();
        assert_ne!(replacement.process["instance_id"],first.process["instance_id"]);
        assert_eq!(replacement.revision,0);
    }

    #[test]
    fn oversized_capabilities_never_replace_the_last_good_snapshot() {
        let state = ChannelState::new("s".into());
        let before = state.capabilities();
        assert!(!state.update_capabilities(|s| s.launch = serde_json::json!({"large":"x".repeat(32*1024)})));
        assert_eq!(state.capabilities(), before);
        assert_eq!(state.next_seq(),0);
    }

    #[test]
    fn prompt_ids_do_not_repeat_across_turns() {
        let state = ChannelState::new("s".to_string());
        let first = state.register_prompt(PromptKind::Permission);
        let second = state.register_prompt(PromptKind::Permission);
        assert_ne!(first, second, "블록 id 와 달리 채널 id 는 전역 단조여야 한다");
    }

    #[test]
    fn an_answer_of_the_wrong_kind_is_invalid_params() {
        let state = ChannelState::new("s".to_string());
        let prompt = state.register_prompt(PromptKind::Permission);
        assert_eq!(
            state.answer_prompt(prompt, Answer::Question(vec!["a".to_string()])),
            Err(AnswerRefusal::WrongKind)
        );
        assert!(state
            .answer_prompt(prompt, Answer::Permission(PermissionDecision::AllowOnce))
            .is_ok());
    }

    #[test]
    fn a_retired_prompt_refuses_a_late_answer() {
        let state = ChannelState::new("s".to_string());
        let prompt = state.register_prompt(PromptKind::Permission);
        assert!(state.retire_prompt(prompt));
        assert_eq!(
            state.answer_prompt(prompt, Answer::Permission(PermissionDecision::Deny)),
            Err(AnswerRefusal::NotWaiting)
        );
        assert!(!state.retire_prompt(prompt));
    }

    #[test]
    fn only_the_first_answer_lands() {
        let state = ChannelState::new("s".to_string());
        let prompt = state.register_prompt(PromptKind::Permission);
        assert!(state
            .answer_prompt(prompt, Answer::Permission(PermissionDecision::AllowOnce))
            .is_ok());
        assert_eq!(
            state.answer_prompt(prompt, Answer::Permission(PermissionDecision::Deny)),
            Err(AnswerRefusal::NotWaiting)
        );
    }

    #[tokio::test]
    async fn waiting_on_a_retired_prompt_ends_instead_of_hanging() {
        let state = Arc::new(ChannelState::new("s".to_string()));
        let prompt = state.register_prompt(PromptKind::Permission);
        let waiter = tokio::spawn({
            let state = Arc::clone(&state);
            async move { state.wait_answer(prompt).await }
        });
        // 패인이 먼저 답했다 — 등록부에서 빼면 기다리던 쪽이 풀린다.
        assert!(state.retire_prompt(prompt));
        assert_eq!(waiter.await.expect("join"), None);
    }

    /// The replay history keeps its `{role,text}` shape and the three snapshot
    /// kinds follow it as frame-shaped objects, latest of each, in the order
    /// the window hydrates them.
    #[test]
    fn subscribe_history_is_replay_then_the_latest_snapshot_of_each_kind() {
        let state = ChannelState::new("s".to_string());
        state.set_history(vec![HistoryEntry {
            role: "user".to_string(),
            text: "first question".to_string(),
        }]);
        state.publish_snapshot(
            super::super::wire::FRAME_TURN,
            &serde_json::json!({"type": "turn", "turn_id": 1, "phase": "start"}),
        );
        state.publish_subagents_snapshot(&serde_json::json!({
            "type": "subagents", "registry": "s", "generation": 3, "running": [{"id": "a1"}]
        }));
        state.publish_subagents_snapshot(&serde_json::json!({
            "type": "subagents", "registry": "s", "generation": 4, "running": []
        }));
        state.publish_snapshot(
            super::super::wire::FRAME_SESSION_STATUS,
            &serde_json::json!({"type": "session_status", "session": "s", "model": "m"}),
        );

        let history = state.hydration_history();
        assert_eq!(history[0]["role"], "user");
        assert_eq!(history[0]["text"], "first question");
        let kinds: Vec<&str> = history[1..]
            .iter()
            .map(|frame| frame["type"].as_str().expect("type"))
            .collect();
        assert_eq!(kinds, ["session_status", "turn", "subagents"]);
        let roster = &history[3];
        assert_eq!(roster["generation"], 4, "the LATEST subagents frame, not the first");
        assert_eq!(roster["running"], serde_json::json!([]));
        // turn (1), subagents (2), subagents (3), session_status (4).
        assert_eq!(roster["seq"], 3, "seq is the channel's frame position");
        assert_eq!(state.next_seq(), 4);
        assert_eq!(state.history().len(), 1, "session.list still counts messages only");
    }

    /// Nothing published, nothing appended: an idle channel's history is the
    /// replay alone.
    #[test]
    fn an_idle_channel_appends_no_snapshot() {
        let state = ChannelState::new("s".to_string());
        assert!(state.hydration_history().is_empty());
        assert!(state.snapshot(super::super::wire::FRAME_SUBAGENTS).is_none());
    }

    #[tokio::test]
    async fn a_command_pushed_before_the_wait_is_not_missed() {
        let state = Arc::new(ChannelState::new("s".to_string()));
        state.push_command(Command::CancelTurn { turn_id: None });
        assert_eq!(
            state.wait_command().await,
            Command::CancelTurn { turn_id: None }
        );
    }
}
