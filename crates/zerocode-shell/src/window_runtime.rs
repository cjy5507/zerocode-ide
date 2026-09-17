use super::*;

/* ---- following an agent's own conversation ---------------------------------
 *
 * A hook event carries the id the agent's VENDOR knows this conversation by, and
 * that id outlives our process. So a tab is not only a terminal that had an agent
 * in it — it is a conversation that can be closed and reopened. See
 * `zerocode_core::provider_session` for the measurement. */

/// What a pane's agent last said about itself.
///
/// `at` is when it said it, which is the board's sort key — Orca sorts each
/// column by `stateChangedAt` descending (AgentKanbanBoard-CzEPAxZN.js:1250), so
/// the freshest change is what a person sees first.
#[derive(Debug, Clone, Serialize)]
pub(super) struct PaneState {
    pub(super) state: zerocode_core::hook::HookState,
    /// Milliseconds since the epoch. The webview's own clock unit, so the card's
    /// relative time ("3m") is one subtraction on the other side.
    pub(super) at: i64,
    /// When the pane ENTERED this state, same unit.
    ///
    /// Split from `at` because the two answer different questions and Orca
    /// keeps them apart (`stateStartedAt`/`updatedAt`,
    /// agent-status-types.ts:98-102): a busy turn's dozens of tool events
    /// push `at` while this stands still. One field doing both jobs made the
    /// board re-bold seen cards on every tool call and reshuffle its columns
    /// mid-turn — the map's P0-5.
    pub(super) state_started_at: i64,
    /// The person's last prompt — Orca's `lastUserMessage`. Carried across
    /// events until the next prompt replaces it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) you: Option<String>,
    /// The agent's last answer — Orca's `lastAssistantMessage`. Replaced or
    /// CLEARED on every turn-ending event (see `PaneHookReport::said`), and
    /// carried across the working events in between.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) said: Option<String>,
    /// What it stopped to ask — Orca's `interactivePrompt`, alive for its one
    /// event only: any event that is not the asking clears it, because the
    /// agent moving on means the question was answered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) ask: Option<String>,
    /// And the ask's full shape when the question carries choices — same
    /// one-event validity, because answering a question the agent moved past
    /// would type keys into whatever replaced it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) ask_prompt: Option<zerocode_core::ask::AskPrompt>,
    /// The tool waiting on permission, when that is the stopping — same
    /// life, same reason: an Allow pressed after the agent moved on is a
    /// keystroke into whatever the TUI shows now.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) approval: Option<zerocode_core::ask::ApprovalPrompt>,
    /// How much of this stop one keystroke could finish — see
    /// [`hooks::PaneHookReport::submit_shape`].
    ///
    /// Third field on the same one-event life as `ask` and `approval`, and the
    /// same reason: a pane that answers "yes, Enter finishes this" about a
    /// question the agent has already moved past would clear a wait that is
    /// now somebody else's.
    #[serde(
        default,
        skip_serializing_if = "zerocode_core::ask::SubmitShape::is_closed"
    )]
    pub(super) submit_shape: zerocode_core::ask::SubmitShape,
    /// This `done` is a session boundary, not a completion — see
    /// [`hooks::PaneHookReport::session_boundary`].
    ///
    /// Kept on the ROW as well as the report because the consumers that must
    /// skip it are downstream of the row: the ring reads it before arming, and
    /// the delayed completion re-reads the row 1.5s later, by which time the
    /// report is gone.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(super) session_boundary: bool,
    /// A person ended this turn — see [`hooks::PaneHookReport::interrupted`].
    ///
    /// CLAMPED to `done`, exactly as Orca clamps it
    /// (`agent-status-types.ts:425`: `interrupted === true && state === 'done'`).
    /// An interrupt on a `working` row is a contradiction — the turn it
    /// interrupted has not ended — and a row carrying one would tell the board
    /// a live pane came to rest.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(super) interrupted: bool,
    /// What this pane was before a HELPER's wait took its word away, when one
    /// did — Orca's `stateBeforeWait`
    /// ([`zerocode_core::hook::wait_stash`] holds the whole rule).
    ///
    /// On the live row and not in the ledger, deliberately. The stash exists to
    /// answer a question only a running pane can be asked — somebody typed an
    /// answer into a pty — and a column no restore road ever reads is a column
    /// nothing writes for. It earns a place on disk the day a hydrated pane can
    /// still be answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) before_wait: Option<zerocode_core::hook::BeforeWait>,
    /// zo's latest `session_status.autonomy` snapshot. It rides beside the
    /// hook lifecycle word: pursuing a goal or running a loop is what a
    /// working pane is doing, not a new state colour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) autonomy: Option<zerocode_core::board::BoardCardAutonomy>,
}

/// A pane the board can draw, from what this process knows about it.
///
/// Assembled here rather than in the window because these three facts — which
/// agent, what state, since when — are held here and nowhere else. Everything
/// else on a card (the workspace's name, the project it belongs to, the task) is
/// the window's, so the window fills those in and asks
/// `zerocode_core::board::columns` to do the grouping.
#[derive(Debug, Clone, Serialize)]
pub(super) struct PaneAgent {
    pub(super) term: TermId,
    pub(super) agent: &'static str,
    /// The state word, in the hook vocabulary. `idle` when the agent has never
    /// reported — which is different from "we think it is idle": it is a pane we
    /// know holds an agent that has not spoken.
    pub(super) state: String,
    /// The LEDGER's word for this seat, when the ledger seated it — empty for
    /// every pane a person opened, which is most of them.
    ///
    /// `state` above is what the agent last said about itself, and for a pane
    /// nobody summoned that is the only thing anyone knows. For a worker this
    /// window retired it is a stale boast: the last hook said `working`,
    /// nothing will ever correct it, and the board went on drawing a finished
    /// worker as busy. The board decides which of the two wins; this only
    /// carries the second one to it.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(super) ledger: String,
    /// The hook channel's own state, separate from what the agent last said:
    /// `pending`, `heard`, `unreachable`, or `gone`.
    pub(super) hearing: &'static str,
    /// When that channel verdict began. Zero means the pane predates a clocked
    /// worker row and no failure marker has supplied one.
    pub(super) hearing_at: i64,
    pub(super) at: i64,
    /// When the state was entered — the board's sort key and the unseen
    /// judgement's reference, exactly as Orca's `stateChangedAt` is both.
    pub(super) state_started_at: i64,
    /// Whether this pane's conversation can be reopened, which the card shows.
    pub(super) resumable: bool,
    /// Which pane asked for this one, when a pane did — Orca's
    /// `parentTerminalHandle` (useWorktreeAgentRows-BZzQmOQU.js:14-30), and
    /// the whole input to the board's subagent tree. A handle, not a pane key:
    /// this side counts terminals, and turning `7` into `term:7` is the
    /// window's spelling of its own card ids.
    ///
    /// Absent for every agent a person opened, which is most of them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) parent: Option<TermId>,
    /// The card's two lines — Orca's `lastUserMessage`/`lastAgentMessage`
    /// (:26392-26393), read out of the hook stream and kept in `PaneState`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) you: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) said: Option<String>,
    /// And the question it is stuck on, when it is — Orca's `askSummary`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) ask: Option<String>,
    /// The question's choices, when the ask is an `AskUserQuestion` — what
    /// the card's answer rows are drawn from, and what travels back verbatim
    /// with the person's picks in `answer_ask`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) ask_prompt: Option<zerocode_core::ask::AskPrompt>,
    /// The tool waiting on permission, when the ask is a permission request
    /// — what the card's Allow and Deny are drawn from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) approval: Option<zerocode_core::ask::ApprovalPrompt>,
    /// The optional autonomous-work fact consumed by the board card's phase
    /// slot. Other agents and older zo builds simply omit it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) autonomy: Option<zerocode_core::board::BoardCardAutonomy>,
}

/// The helpers running inside one pane's agent.
///
/// The shape both roads deliver: `hook:subagent` carries one of these when a
/// helper starts or stops, and `pane_subagents` carries one per pane for a
/// window that was not listening. Same field names on purpose — a board that
/// seeds and then listens applies one merge, not two.
/// A nested-agent run's page opening: who, where, and the whole command.
#[derive(Clone, Serialize)]
pub(super) struct WorkerOpened {
    pub(super) id: String,
    pub(super) term: TermId,
    pub(super) agent: String,
    pub(super) name: String,
    pub(super) command: String,
    /// The checkout the run is happening in, straight off the hook envelope.
    ///
    /// The window used to work this out by looking up the parent terminal's
    /// TAB, which is a guess with a hole in it: a run can announce itself
    /// before a restored or background terminal has been mounted, and then
    /// there is no tab to ask and the page lands on whichever checkout
    /// happens to be in front — the cross-project placement all over again.
    /// The envelope has carried the answer all along (`ZEROCODE_WORKTREE_ID`
    /// is the worktree's path, the same string the window keys tabs by).
    pub(super) worktree: String,
}

/// And its end: how it ended, and whatever text the vendor handed back.
#[derive(Clone, Serialize)]
pub(super) struct WorkerDone {
    pub(super) id: String,
    pub(super) status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) output: Option<String>,
    pub(super) truncated: bool,
}

/// A mirror file coming alive (or closing) under one pane's run.
#[derive(Clone, Serialize)]
pub(super) struct MirrorNote {
    pub(super) term: TermId,
    pub(super) agent: String,
    pub(super) path: String,
}

/// Names the fallback run ids, for the payloads that carry none.
pub(super) static WORKER_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[derive(Debug, Clone, Serialize)]
pub(super) struct PaneSubagents {
    pub(super) term: TermId,
    pub(super) rows: Vec<hooks::SubagentRow>,
}

/// What one card has been doing, newest last.
///
/// The shape both roads deliver, like [`PaneSubagents`]: `hook:activity`
/// carries one of these when an agent moves, and `pane_activities` carries one
/// per card for a window that was not listening. `pane` is the board's own
/// card name — `term:3` for a pane, `sub:3:a-1` for a helper running inside
/// one — so a surface that already knows which card it is drawing does not
/// have to translate anything to find its line.
#[derive(Debug, Clone, Serialize)]
pub(super) struct PaneActivities {
    pub(super) pane: String,
    pub(super) activities: Vec<StampedActivity>,
}

/* ---- 보드 팝아웃: 이 제품의 두 번째 OS 창 -------------------------------
 *
 * Orca가 별도 창으로 내보내는 표면은 **에이전트 대시보드 하나뿐**이고,
 * 그 창은 하나뿐이며(재요청은 포커스), 터미널은 옮겨지지 않는다 — 팝아웃은
 * 살아 있는 pty에 뷰어를 하나 더 붙일 뿐이다.
 *
 * 릴레이는 이식하지 않았다. Orca는 메인 렌더러가 250ms마다 스냅샷을 만들어
 * 메인 프로세스를 거쳐 팝아웃으로 밀지만, 여기서는 백엔드가 진실의 원천이라
 * 팝아웃 웹뷰가 `pane_agents`·`board_columns`를 **직접** 부른다. 창을 건너는
 * 것은 두 가지 사실뿐이다: "봤다"(ack)와 "저기로 데려가 달라"(reveal).
 * 그래서 이 파일에는 스냅샷 검증기도, 마지막 스냅샷을 들고 있는 변수도,
 * 카드 개수 상한도 없다 — 옮길 것이 아예 없다. */

/// 팝아웃 창의 라벨.
///
/// 창이 하나뿐이므로 id로 키를 만든 표가 아니라 상수 하나다 — Orca의
/// 등록소도 같은 이유로 모듈 스코프 변수 한 개다.
pub(super) const BOARD_POPOUT_LABEL: &str = "board-popout";

/// 팝아웃이 처음 열릴 때의 크기와, 어떤 경우에도 내려가지 않는 하한.
/// Orca 실측값 그대로(960×720, 480×360).
pub(super) const POPOUT_DEFAULT_SIZE: (f64, f64) = (960.0, 720.0);
pub(super) const POPOUT_MIN_SIZE: (f64, f64) = (480.0, 360.0);

/// 메인 창의 하한. `tauri.conf.json` `windows[0]`의 minWidth/minHeight 그대로다
/// — 게이트는 **이 창의** 바닥을 말해야 한다(Orca는 자기 600×400,
/// `createMainWindow.ts:178-179`).
pub(super) const MAIN_MIN_SIZE: (f64, f64) = (720.0, 480.0);

/// 경계를 디스크에 적기까지 기다리는 시간.
///
/// 창을 한 번 끄는 동안 이동 이벤트는 수십 번 오고, 그것을 다 쓰면 파일이
/// 드래그를 따라 쓰인다. 마지막 한 번만 남긴다(Orca도 500ms —
/// `createMainWindow.ts:425`).
pub(super) const WINDOW_BOUNDS_DEBOUNCE: Duration = Duration::from_millis(500);

/// 부팅 복원이 일으킨 리사이즈 폭풍이 지나가고, (크기, 배율) 쌍이 같은
/// 순간의 것이 되기까지 기다리는 시간 — 경계 디바운스(500ms)보다 한 박자
/// 뒤다. 이 잠 뒤의 재선언은 경주의 승패와 무관하게 맞는 수를 말한다.
pub(super) const WEBVIEW_REFIT_SETTLE: Duration = Duration::from_millis(800);

/// A native geometry query should ride the next main-thread turn. Five
/// seconds is a failure boundary, not a debounce: beyond it the window event
/// loop is unavailable and an async refitter must return rather than occupy a
/// runtime worker forever.
#[cfg(target_os = "macos")]
pub(super) const WEBVIEW_NATIVE_QUERY_TIMEOUT: Duration = Duration::from_secs(5);

/// 리핏 요청은 "한 번 더 재라"는 신호라서 같은 요청을 쌓을 이유가 없다.
///
/// 리사이즈 드래그 한 번이 수백 개의 잠자는 작업을 만들지 않도록 단일 작업자
/// 앞에 최신 신호 하나만 둔다. 작업자는 그 하나를 받은 뒤 조용해질 때까지
/// 기다리므로, 용량 자체가 이벤트 병합 계약이다.
pub(super) const WEBVIEW_REFIT_SIGNAL_CAPACITY: usize = 1;

/// 창이 마지막으로 있던 자리, 논리 픽셀로.
///
/// 크기는 **내부** 크기, 위치는 **외부** 위치다 — 만들 때 쓰는
/// `inner_size`/`position`과 같은 두 사각형이라야 복원이 정확하다. 섞으면
/// 열 때마다 제목 표시줄 높이만큼 창이 아래로 기어간다.
///
/// `maximized`는 `#[serde(default)]`라 이 필드가 없던 시절의
/// `popout-bounds.json`도 그대로 읽힌다 — 팝아웃은 최대화를 기억한 적이
/// 없고, 없던 것은 false다.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug, Default)]
pub(super) struct WindowBounds {
    pub(super) x: f64,
    pub(super) y: f64,
    pub(super) width: f64,
    pub(super) height: f64,
    #[serde(default)]
    pub(super) maximized: bool,
}

/// 팝아웃이 창을 건너 보내는 것의 전부: 어느 카드였는가.
#[derive(Serialize, Clone)]
pub(super) struct BoardPane {
    pub(super) pane: String,
}

pub(super) fn stored_window_bounds(file: &Path) -> Option<WindowBounds> {
    let text = std::fs::read_to_string(file).ok()?;
    serde_json::from_str::<WindowBounds>(&text).ok()
}

pub(super) fn write_window_bounds(file: &Path, bounds: WindowBounds) {
    if let Some(parent) = file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string(&bounds) {
        let _ = std::fs::write(file, text);
    }
}

/// 적어도 되는 경계인가.
///
/// 최소치**보다 큰** 사각형만 적는다 — 최소화되거나 화면 밖으로 밀린 순간이
/// 최소치로 읽히고, 그것을 적으면 다음에 열리는 창이 그 모양으로 태어난다.
/// 저장과 복원이 **같은** 판정을 쓰는 것이 요점이다.
///
/// 경계가 정확히 최소치인 것도 버린다. Orca가 사건 번호까지 달아 둔 자리라서
/// 그렇다 — 창을 부수는 동안 resize/move/unmaximize가 **최소 크기에서** 한 번
/// 더 오고, 그것이 기억된 크기를 덮는다(`createMainWindow.ts:392,417-421`,
/// PR #1269). 최소치에 정확히 맞춘 창을 일부러 쓰던 사람은 그 크기를 잃지만,
/// 잃는 쪽이 매번 창이 쪼그라드는 쪽보다 낫다.
pub(super) fn savable_bounds(bounds: WindowBounds, min: (f64, f64)) -> Option<WindowBounds> {
    let (min_width, min_height) = min;
    (bounds.width > min_width && bounds.height > min_height).then_some(bounds)
}

/// 어느 화면 하나라도 이 사각형의 쓸 만한 조각을 품고 있는가.
///
/// Orca의 `rectHasVisibleAreaOnAnyDisplay`: 겹치는 넓이가 요구치를 가로·세로
/// 양쪽에서 넘어야 한다. 1px만 걸친 창은 "화면 위에 있다"가 아니다.
pub(super) fn rect_has_visible_area(
    rect: WindowBounds,
    screens: &[WindowBounds],
    need_width: f64,
    need_height: f64,
) -> bool {
    screens.iter().any(|screen| {
        let width = (rect.x + rect.width).min(screen.x + screen.width) - rect.x.max(screen.x);
        let height = (rect.y + rect.height).min(screen.y + screen.height) - rect.y.max(screen.y);
        width >= need_width && height >= need_height
    })
}

/// 복원해도 되는 경계.
///
/// 없거나, 최소치보다 작거나, 어느 화면에도 걸치지 않으면 버리고 기본
/// 크기로 연다. 모니터를 뽑고 온 사람에게 보이지 않는 창을 건네는 것이
/// 이 검사가 막는 유일한 일이고, 그래서 요구치는 최소치의 절반이다 —
/// 반쯤 걸친 창은 잡아서 끌어올 수 있다.
pub(super) fn restorable_bounds(
    raw: Option<WindowBounds>,
    screens: &[WindowBounds],
    min: (f64, f64),
) -> Option<WindowBounds> {
    let kept = savable_bounds(raw?, min)?;
    let (min_width, min_height) = min;
    rect_has_visible_area(kept, screens, min_width / 2.0, min_height / 2.0).then_some(kept)
}

/// 이 기계가 지금 들고 있는 화면들, 논리 픽셀의 사각형으로.
/// 이 기계가 지금 들고 있는 화면들 — **작업 영역**의 사각형으로.
///
/// 화면 전체가 아니라 작업 영역인 것이 요점이다(Orca `display.workArea`,
/// `window-bounds-validation.ts:20`). 메뉴 바와 독이 덮은 띠는 창을 놓을 수
/// 있는 자리가 아니므로, 전체 화면으로 재면 그 띠 밑에 숨은 창을 "보인다"고
/// 판정한다 — 제목 표시줄이 메뉴 바 뒤에 있는 창은 잡아서 끌어올 수도 없다.
pub(super) fn monitor_rects(app: &AppHandle) -> Vec<WindowBounds> {
    app.available_monitors()
        .unwrap_or_default()
        .iter()
        .map(work_area_of)
        .collect()
}

/// 한 화면의 작업 영역, 논리 픽셀로.
pub(super) fn work_area_of(monitor: &tauri::window::Monitor) -> WindowBounds {
    let scale = monitor.scale_factor();
    let area = monitor.work_area();
    let at = area.position.to_logical::<f64>(scale);
    let size = area.size.to_logical::<f64>(scale);
    WindowBounds {
        x: at.x,
        y: at.y,
        width: size.width,
        height: size.height,
        maximized: false,
    }
}

pub(super) fn window_bounds_of(window: &tauri::WebviewWindow) -> Option<WindowBounds> {
    let scale = window.scale_factor().ok()?;
    let at = window.outer_position().ok()?.to_logical::<f64>(scale);
    #[cfg(target_os = "macos")]
    let size = macos_webview_parent_size(window.as_ref())
        .ok()?
        .to_logical::<f64>(scale);
    #[cfg(not(target_os = "macos"))]
    let size = window.inner_size().ok()?.to_logical::<f64>(scale);
    Some(WindowBounds {
        x: at.x,
        y: at.y,
        width: size.width,
        height: size.height,
        maximized: window.is_maximized().unwrap_or(false),
    })
}

/// 창이 움직이거나 크기가 바뀔 때마다 자리를 적는다 — 500ms 뒤에, 한 번.
///
/// 최소화·전체화면인 순간은 건너뛴다: 그때의 경계는 창이 있던 자리가 아니고,
/// 적어 두면 다음에 열리는 창이 그 모양을 물려받는다.
///
/// 세대 카운터가 **창마다 하나**인 것이 이 함수가 일반화되면서 고쳐진
/// 것이다. 전역 카운터 하나를 두 창이 나눠 쓰면, 한 창을 드래그하는 동안
/// 다른 창의 대기 중인 저장이 "내가 마지막이 아니다"를 보고 조용히 사라진다
/// — 팝아웃 하나뿐일 때는 일어날 수 없던 일이고, 메인 창이 합류하는 순간
/// 일어난다.
///
/// 최대화 상태와 사각형은 **원자적인 짝**이다(Orca `createMainWindow.ts:411-417`).
/// 최대화된 창의 사각형은 화면 전체이므로, 그것을 적으면 다음 실행에서
/// 최대화를 풀었을 때 돌아갈 자리가 사라진다. 그래서 최대화된 순간에는
/// 파일에 이미 있던 사각형을 그대로 두고 깃발만 세운다.
pub(super) fn watch_window_bounds(window: &tauri::WebviewWindow, file: PathBuf, min: (f64, f64)) {
    use std::sync::atomic::{AtomicU64, Ordering};

    let owner = window.clone();
    let writes = std::sync::Arc::new(AtomicU64::new(0));
    window.on_window_event(move |event| {
        if !matches!(
            event,
            tauri::WindowEvent::Resized(_) | tauri::WindowEvent::Moved(_)
        ) {
            return;
        }
        let mine = writes.fetch_add(1, Ordering::Relaxed) + 1;
        let writes = std::sync::Arc::clone(&writes);
        let window = owner.clone();
        let file = file.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(WINDOW_BOUNDS_DEBOUNCE).await;
            if writes.load(Ordering::Relaxed) != mine {
                return;
            }
            if window.is_minimized().unwrap_or(false) || window.is_fullscreen().unwrap_or(false) {
                return;
            }
            let Some(now) = window_bounds_of(&window) else {
                return;
            };
            if now.maximized {
                // 사각형은 손대지 않는다 — 지금 것은 화면 전체이고, 파일에
                // 있는 것은 사람이 최대화 버튼을 누르기 직전의 자리다. 아직
                // 아무 사각형도 없다면 0으로 남긴다: 복원 게이트가 그것을
                // 버리므로 "최대화만 기억한다"가 그대로 표현된다 — Orca가
                // `windowBounds` 없이 `windowMaximized`만 적는 것과 같은 상태.
                let kept = stored_window_bounds(&file).unwrap_or_default();
                write_window_bounds(
                    &file,
                    WindowBounds {
                        maximized: true,
                        ..kept
                    },
                );
                return;
            }
            if let Some(bounds) = savable_bounds(now, min) {
                write_window_bounds(&file, bounds);
            }
        });
    });
}

/// 창이 사라지면 그 창이 읽고 있던 셸도 함께 잊는다.
///
/// 감시 집합이 창 라벨로 키를 잡은 값을 치르는 자리다. 없으면 미리보기를 열어
/// 둔 채 닫힌 팝아웃의 선언이 그대로 남아, 아무도 읽지 않는 셸의 프레임이
/// 영영 흘러간다 — 이 조각이 없애려던 바로 그 비용을, 죽은 창 하나가 되살리는
/// 셈이다. 파괴는 창이 사라졌다는 사실 자체이므로 여기서 듣는다.
pub(super) fn forget_watched_when_closed(app: &AppHandle, window: &tauri::WebviewWindow) {
    let handle = app.clone();
    let label = window.label().to_string();
    window.on_window_event(move |event| {
        if !matches!(event, tauri::WindowEvent::Destroyed) {
            return;
        }
        let state = handle.state::<AppState>();
        // The shells it read lose it as a reader on the pump's next round
        // (`FrameReaders::reconcile`), shares and all.
        state.watched_terms().remove(&label);
        // 미리보기 선언도 같은 문으로 나간다. 보드 팝아웃은 카드마다 판을
        // 미리보므로, 이 줄이 없으면 닫힌 창 하나가 살아 있는 에이전트 전부의
        // 꼬리를 아무도 없는 곳으로 계속 흘려보낸다.
        handle.state::<AppState>().previewed_terms().remove(&label);
    });
}

/// 메인 창을 지난번 있던 자리로 돌려놓고, 그 뒤로 움직임을 따라 적는다.
///
/// Orca는 이것을 UI 스토어의 두 열쇠로 들고 있다 — `windowBounds`와
/// `windowMaximized`(`createMainWindow.ts:230,246`). 여기서도 **둘은 따로
/// 판정된다**: 사각형은 화면 검사를 통과해야 쓰이지만, 최대화 깃발은 통과
/// 여부와 무관하게 선다. 모니터를 뽑고 온 사람의 저장된 사각형은 버려지고,
/// 그래도 최대화로 열리던 창은 최대화로 열려야 한다.
///
/// 창이 `tauri.conf.json`의 크기로 이미 보이는 상태에서 옮겨지므로 부팅에
/// 한 번 튄다 — 맵에 잔여로 적었다. 창을 숨겨서 시작하는 길은 이 조각보다
/// 크고, 웹뷰가 죽으면 영영 안 보이는 창이 된다.
pub(super) fn restore_main_window_bounds(app: &AppHandle) {
    let Some(main) = app.get_webview_window("main") else {
        return;
    };
    let file = app
        .state::<AppState>()
        .local_data_root()
        .join(artifact_file::MAIN_WINDOW_BOUNDS);
    let raw = stored_window_bounds(&file);
    if let Some(saved) = restorable_bounds(raw, &monitor_rects(app), MAIN_MIN_SIZE) {
        let _ = main.set_size(tauri::LogicalSize::new(saved.width, saved.height));
        let _ = main.set_position(tauri::LogicalPosition::new(saved.x, saved.y));
    } else if let Some(area) = first_run_size(app) {
        // 자리는 주지 않는다 — 크기만. OS 가 새 창을 놓는 규칙이 이 창에도
        // 그대로 적용되어야 하고, Orca 도 `defaultBounds` 에 width/height 만
        // 담는다 (`createMainWindow.ts:249-255`).
        let _ = main.set_size(tauri::LogicalSize::new(area.0, area.1));
    }
    if raw.is_some_and(|bounds| bounds.maximized) {
        let _ = main.maximize();
    }
    watch_window_bounds(&main, file, MAIN_MIN_SIZE);
    // 복원이 창을 옮겨 놓은 다음이 정확히 그 경주가 끝난 자리다 — 여기서
    // 웹뷰를 창에 다시 맞춰 두면, 2x 화면에서 절반으로 태어난 부팅도 사람
    // 손을 빌리지 않고 낫는다.
    keep_main_webview_fitted(app, "main");
}

/// 처음 켠 창이 설 크기 — 주 화면의 작업 영역 전체.
///
/// Orca 의 이유 그대로: *"on first launch fill the primary display work area
/// so the window feels spacious without maximize(); saved bounds win later"*
/// (`createMainWindow.ts:248`). 1280×800 로 태어난 창은 27인치 화면에서
/// 우표만 하고, 사람이 맨 처음 하는 일이 창을 끌어 늘리는 것이 된다.
///
/// 주 화면을 못 읽으면 `None` — 그러면 `tauri.conf.json` 이 말한 크기가 그대로
/// 선다. 여기서 수를 지어내면 그 수가 두 번째 기본값이 된다.
pub(super) fn first_run_size(app: &AppHandle) -> Option<(f64, f64)> {
    let monitor = app.primary_monitor().ok().flatten()?;
    let area = work_area_of(&monitor);
    (area.width > 0.0 && area.height > 0.0).then_some((area.width, area.height))
}

/// 창의 웹뷰를 창 크기로 되돌려 앉힌다 — 창이 이미 아는 사실의 재선언이라
/// 맞는 웹뷰에는 아무 일도 아니고, 틀린 웹뷰는 이 한 번으로 창을 다시 채운다.
///
/// macOS 멀티웹뷰(`unstable`)의 메인 웹뷰는 리사이즈 사건으로만 크기를
/// 받는데, 부팅 복원이 창을 2x 화면에 앉히는 동안 사건의 물리 크기와 창의
/// 배율이 **서로 다른 순간**에서 읽히면 논리 크기가 배율로 한 번 더 나뉜다.
/// 그러면 웹뷰가 창의 정확히 절반에 앉는다 — 2026-08-25 두 부팅 연속으로
/// 측정(창 3016×1776px, 웹뷰 1508×888px, 좌상단 1/4 렌더링), 1x 화면에서는
/// ÷1이라 무증상이고, 사람이 창을 한 번 끌면 다음 리사이즈가 고쳐 놓았다.
/// 그 치유를 사람 손에 맡기지 않는 것이 이 함수다.
///
/// 대상은 창과 **같은 라벨**의 웹뷰 하나뿐이다. 브라우저 판의 사각형은 UI가
/// 재서 선언하는 것이라, 여기서 만지면 그 선언을 덮는다.
#[derive(Debug)]
pub(super) struct WebviewFit {
    pub(super) window: tauri::PhysicalSize<u32>,
    pub(super) position: tauri::PhysicalPosition<i32>,
    pub(super) webview: tauri::PhysicalSize<u32>,
}

impl WebviewFit {
    pub(super) fn matches(&self) -> bool {
        self.position == tauri::PhysicalPosition::new(0, 0) && self.window == self.webview
    }
}

/// The real macOS content rectangle, not Tauri's multiwebview `inner_size`.
///
/// With no additional child pane, tauri-runtime-wry answers Window::InnerSize
/// from the main webview's own frame. That made a frozen 1280x800 webview and
/// its supposedly 1920x980 window compare equal forever. Wry's parent is the
/// NSWindow content view it attached the WKWebView to, so its bounds are the
/// uncontaminated source on both the overlay-titlebar main window and the
/// normally decorated board popout.
#[cfg(target_os = "macos")]
pub(super) fn macos_webview_parent_size(
    webview: &tauri::Webview,
) -> Result<tauri::PhysicalSize<u32>, String> {
    let (answer, read) = std::sync::mpsc::sync_channel(1);
    webview
        .with_webview(move |platform| {
            let measured = (|| {
                let view: &objc2_web_kit::WKWebView = unsafe { &*platform.inner().cast() };
                let parent = unsafe { view.superview() }
                    .ok_or_else(|| "the main webview has no content parent".to_string())?;
                let bounds = parent.bounds();
                let window: &objc2_app_kit::NSWindow = unsafe { &*platform.ns_window().cast() };
                let scale = window.backingScaleFactor();
                let width = bounds.size.width * scale;
                let height = bounds.size.height * scale;
                if !width.is_finite()
                    || !height.is_finite()
                    || width <= 0.0
                    || height <= 0.0
                    || width > u32::MAX as f64
                    || height > u32::MAX as f64
                {
                    return Err(format!(
                        "invalid native webview parent size: {width}x{height}"
                    ));
                }
                Ok(tauri::PhysicalSize::new(
                    width.round() as u32,
                    height.round() as u32,
                ))
            })();
            let _ = answer.send(measured);
        })
        .map_err(|error| error.to_string())?;
    read.recv_timeout(WEBVIEW_NATIVE_QUERY_TIMEOUT)
        .map_err(|error| format!("native webview parent size timed out: {error}"))?
}

#[cfg(target_os = "macos")]
pub(super) fn window_size_for_webview(
    _window: &tauri::Window,
    webview: &tauri::Webview,
) -> Result<tauri::PhysicalSize<u32>, String> {
    macos_webview_parent_size(webview)
}

#[cfg(not(target_os = "macos"))]
pub(super) fn window_size_for_webview(
    window: &tauri::Window,
    _webview: &tauri::Webview,
) -> Result<tauri::PhysicalSize<u32>, String> {
    window.inner_size().map_err(|error| error.to_string())
}

/// Apply the full-window rectangle through the platform road that actually
/// owns the main webview's native frame.
///
/// Tauri's macOS `set_bounds` reaches `wry::WebView::set_bounds`, which only
/// mutates webviews created with `build_as_child`; a `WebviewWindow` is the
/// window content view and that call deliberately no-ops. Its WKWebView is
/// supposed to follow its Wry parent through an autoresizing mask, but the
/// maximize/restore race this guard repairs is precisely a missed native
/// resize. Re-state both the mask and the parent's current bounds on the main
/// thread. Child browser panes are never passed here.
#[cfg(target_os = "macos")]
pub(super) fn apply_webview_fit(
    webview: &tauri::Webview,
    target: tauri::PhysicalSize<u32>,
) -> Result<(), String> {
    // Keep Tauri's autoresize ledger at 1:1 as well. Its SetBounds handler
    // updates width_rate/height_rate even where wry's WindowContent native
    // setter is a no-op; skipping this would let the next Resized event
    // reapply the poisoned boot ratio over the raw frame repaired below.
    webview
        .set_bounds(tauri::Rect {
            position: tauri::PhysicalPosition::new(0, 0).into(),
            size: target.into(),
        })
        .map_err(|error| error.to_string())?;
    webview
        .with_webview(|platform| {
            use objc2_app_kit::NSAutoresizingMaskOptions;

            // Main-thread-only by `with_webview`'s contract. Wry installed
            // this WKWebView under one content parent when it built the
            // WebviewWindow, so a live view always has the owner we measure.
            let view: &objc2_web_kit::WKWebView = unsafe { &*platform.inner().cast() };
            let Some(parent) = (unsafe { view.superview() }) else {
                return;
            };
            view.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewHeightSizable
                    | NSAutoresizingMaskOptions::ViewWidthSizable,
            );
            view.setFrame(parent.bounds());
        })
        .map_err(|error| error.to_string())
}

#[cfg(not(target_os = "macos"))]
pub(super) fn apply_webview_fit(
    webview: &tauri::Webview,
    target: tauri::PhysicalSize<u32>,
) -> Result<(), String> {
    webview
        .set_bounds(tauri::Rect {
            position: tauri::PhysicalPosition::new(0, 0).into(),
            size: target.into(),
        })
        .map_err(|error| error.to_string())
}

pub(super) fn refit_main_webview(app: &AppHandle, label: &str) -> Result<WebviewFit, String> {
    let window = app
        .get_window(label)
        .ok_or_else(|| format!("window {label} is gone"))?;
    let webview = app
        .get_webview(label)
        .ok_or_else(|| format!("webview {label} is gone"))?;
    // Physical-to-physical on purpose. Converting through a scale factor read
    // in a different instant was the original half-size race.
    let target = window_size_for_webview(&window, &webview)?;
    apply_webview_fit(&webview, target)?;
    // Read every side AGAIN after the one bounds request. Tauri processes
    // webview changes later; if maximize/restore moved the window in between,
    // comparing against the first read would certify the poisoned ratio. A
    // single bounds message also changes the auto-resize rates and native
    // frame once, instead of flashing through a moved-old-size intermediate.
    let window = window_size_for_webview(&window, &webview)?;
    let position = webview.position().map_err(|error| error.to_string())?;
    let webview = webview.size().map_err(|error| error.to_string())?;
    Ok(WebviewFit {
        window,
        position,
        webview,
    })
}

pub(super) async fn converge_main_webview(app: AppHandle, label: &'static str) {
    let mut mismatch_logged = false;
    loop {
        match refit_main_webview(&app, label) {
            Ok(fit) if fit.matches() => {
                if mismatch_logged {
                    note_window_event(
                        app.state::<AppState>().local_data_root(),
                        &format!(
                            "webview {label} converged at {}x{}",
                            fit.window.width, fit.window.height
                        ),
                    );
                }
                return;
            }
            Ok(fit) => {
                if !mismatch_logged {
                    note_window_event(
                        app.state::<AppState>().local_data_root(),
                        &format!(
                            "webview {label} mismatch: window {}x{}, webview {}x{} at {},{}",
                            fit.window.width,
                            fit.window.height,
                            fit.webview.width,
                            fit.webview.height,
                            fit.position.x,
                            fit.position.y
                        ),
                    );
                    mismatch_logged = true;
                }
            }
            Err(error) => {
                // A destroyed target is a normal end; another error is still
                // bounded to one diagnostic instead of an immortal retry log.
                if app.get_window(label).is_some() || app.get_webview(label).is_some() {
                    note_window_event(
                        app.state::<AppState>().local_data_root(),
                        &format!("webview {label} refit stopped: {error}"),
                    );
                }
                return;
            }
        }
        tokio::time::sleep(WEBVIEW_REFIT_SETTLE).await;
    }
}

/// 마지막 리핏 요청 뒤로 창의 크기가 조용해질 때까지 기다린다.
///
/// `true`는 안정 구간이 왔다는 뜻이고, `false`는 창과 함께 모든 sender가
/// 사라졌다는 뜻이다. 타임아웃 하나를 새로 만드는 것은 싸고, 네이티브 웹뷰를
/// 리사이즈 중간 프레임마다 다시 합성하는 것은 비싸다.
pub(super) async fn wait_for_main_webview_refit_quiet(
    requests: &mut tokio::sync::mpsc::Receiver<()>,
) -> bool {
    loop {
        match tokio::time::timeout(WEBVIEW_REFIT_SETTLE, requests.recv()).await {
            Ok(Some(())) => continue,
            Ok(None) => return false,
            Err(_) => return true,
        }
    }
}

/// 창 하나당 하나뿐인 리핏 작업자.
///
/// 이벤트 콜백은 신호만 병합하고, 이 작업자가 안정화와 수렴을 순서대로 맡는다.
/// 따라서 리사이즈 이벤트 수와 비동기 작업 수가 비례하지 않는다.
pub(super) async fn run_main_webview_refitter(
    app: AppHandle,
    label: &'static str,
    mut requests: tokio::sync::mpsc::Receiver<()>,
) {
    let mut converged_once = false;
    while requests.recv().await.is_some() {
        if !wait_for_main_webview_refit_quiet(&mut requests).await {
            return;
        }
        converge_main_webview(app.clone(), label).await;
        if !converged_once {
            converged_once = true;
            tauri::async_runtime::spawn(nudge_main_window_after_boot(app.clone(), label));
        }
    }
}

/// How long after the first convergence the boot nudge waits: past the
/// restore's last resize and the first paint, so the nudge is the LAST
/// geometry change of boot rather than one more contender in the race.
pub(super) const WINDOW_BOOT_NUDGE_DELAY: Duration = Duration::from_millis(1500);
/// The gap between the two halves of the nudge — long enough for AppKit to
/// process the first as its own resize rather than coalescing the pair.
pub(super) const WINDOW_BOOT_NUDGE_HOLD: Duration = Duration::from_millis(120);

/// Resize the main window by one pixel and back, once, after boot.
///
/// A window born on a retina screen can come up with its webview hit-testing
/// one text row above where the pixels are — every terminal and browser pane
/// selects the row above the pointer, and the copy of that selection is the
/// wrong row too (live report 2026-09-02, right after a restart). The person's
/// own cure was to drag the window's edge once: a real resize makes WebKit
/// recompute the mapping. This is that drag, done by the window itself:
/// one pixel taller, then back, after the boot geometry has settled. A
/// window that is gone by then is left alone; a failed resize is one line in
/// the black box, never a retry.
pub(super) async fn nudge_main_window_after_boot(app: AppHandle, label: &'static str) {
    tokio::time::sleep(WINDOW_BOOT_NUDGE_DELAY).await;
    let Some(window) = app.get_window(label) else {
        note_window_event(
            app.state::<AppState>().local_data_root(),
            &format!("window {label} boot nudge skipped: the window is gone"),
        );
        return;
    };
    // A maximized (zoomed) window ignores `set_size` on macOS, and the nudge
    // used to return in silence right here — the 2026-09-02 cure never fired
    // on a zoomed window, so the boot hit-test race stayed through every
    // restart of 2026-09-07 (0 "nudged" lines in the day's log; the press on
    // "Chrome" selected the row below). The mapping that goes stale is the
    // webview's, so a zoomed window nudges the webview's own bounds instead.
    if window.is_maximized().unwrap_or(false) {
        nudge_main_webview_bounds(&app, label, "the window is maximized").await;
        return;
    }
    let Ok(size) = window.outer_size() else {
        note_window_event(
            app.state::<AppState>().local_data_root(),
            &format!("window {label} boot nudge skipped: its size could not be read"),
        );
        return;
    };
    let taller = tauri::PhysicalSize::new(size.width, size.height.saturating_add(1));
    if let Err(error) = window.set_size(taller) {
        nudge_main_webview_bounds(&app, label, &format!("window set_size refused: {error}")).await;
        return;
    }
    tokio::time::sleep(WINDOW_BOOT_NUDGE_HOLD).await;
    let Some(window) = app.get_window(label) else {
        return;
    };
    match window.set_size(size) {
        Ok(()) => note_window_event(
            app.state::<AppState>().local_data_root(),
            &format!(
                "window {label} nudged after boot: {}x{} → +1 → back",
                size.width, size.height
            ),
        ),
        Err(error) => note_window_event(
            app.state::<AppState>().local_data_root(),
            &format!("window {label} boot nudge could not restore its size: {error}"),
        ),
    }
}

/// The nudge for a window whose own size cannot move: grow the webview one
/// pixel, hold, then let the refitter put it back exactly where it belongs.
/// Every exit writes one line, so a boot that healed nothing says so.
pub(super) async fn nudge_main_webview_bounds(app: &AppHandle, label: &'static str, why: &str) {
    let Some(webview) = app.get_webview(label) else {
        note_window_event(
            app.state::<AppState>().local_data_root(),
            &format!("webview {label} boot nudge skipped ({why}): the webview is gone"),
        );
        return;
    };
    let Ok(size) = webview.size() else {
        note_window_event(
            app.state::<AppState>().local_data_root(),
            &format!("webview {label} boot nudge skipped ({why}): its size could not be read"),
        );
        return;
    };
    let taller = tauri::PhysicalSize::new(size.width, size.height.saturating_add(1));
    if let Err(error) = webview.set_size(taller) {
        note_window_event(
            app.state::<AppState>().local_data_root(),
            &format!("webview {label} boot nudge failed ({why}): set_size refused: {error}"),
        );
        return;
    }
    tokio::time::sleep(WINDOW_BOOT_NUDGE_HOLD).await;
    match refit_main_webview(app, label) {
        Ok(fit) => note_window_event(
            app.state::<AppState>().local_data_root(),
            &format!(
                "webview {label} nudged after boot ({why}): {}x{} → +1 → back at {}x{}",
                size.width, size.height, fit.webview.width, fit.webview.height
            ),
        ),
        Err(error) => note_window_event(
            app.state::<AppState>().local_data_root(),
            &format!("webview {label} boot nudge could not refit after +1 ({why}): {error}"),
        ),
    }
}

/// 부팅과 배율 변화 뒤에 [`refit_main_webview`]를 다시 말해 둔다.
///
/// 두 길이다. 배율 사건은 창이 다른 배율의 화면으로 건너간 그 순간이고 —
/// 같은 경주가 거기서도 열린다 — 지연된 한 번은 복원의 마지막 리사이즈까지
/// 지나간 자리다. 둘 다 한 박자 자고 나서 재는 이유는 하나다: 막 바뀐
/// 순간의 (크기, 배율)은 서로 어긋난 쌍일 수 있고, 그 어긋남이 바로 이
/// 조각이 지우려는 병이다.
pub(super) fn keep_main_webview_fitted(app: &AppHandle, label: &'static str) {
    let Some(window) = app.get_window(label) else {
        return;
    };
    let (request_refit, requests) = tokio::sync::mpsc::channel(WEBVIEW_REFIT_SIGNAL_CAPACITY);
    tauri::async_runtime::spawn(run_main_webview_refitter(app.clone(), label, requests));
    // 첫 신호도 같은 안정화 문을 지난다. 예전 구현은 부팅의 1280x800
    // construction frame이 저장된 1080x987 → 최대화 1920x980보다 먼저
    // "일치"한다고 답해 영구적으로 작은 웹뷰를 남겼다.
    let _ = request_refit.try_send(());
    // 배율 사건만 듣던 첫 판은 구멍이었다 — 창을 그냥 끌어 늘리는 동안
    // 어긋난 순간을 읽은 리사이즈는 배율 사건 없이도 웹뷰를 창보다 작게
    // 남긴다(라이브 보고 2026-08-25 오후, 우하단 흰 공백). 이제 크기가
    // 움직인 모든 사건을 한 작업자에게 병합하고, 마지막 사건 뒤에만
    // 재선언한다. 드래그 중간 프레임을 합성하지 않으므로 깜박이지 않는다.
    window.on_window_event(move |event| {
        if !matches!(
            event,
            tauri::WindowEvent::ScaleFactorChanged { .. } | tauri::WindowEvent::Resized(_)
        ) {
            return;
        }
        let _ = request_refit.try_send(());
    });
}

/// 메인 창이 사라지면 팝아웃도 사라진다.
///
/// Orca의 `mainWindow.on("closed", () => closeDashboardPopout())` 그대로.
/// 없으면 창을 닫은 사람에게 아무 데도 연결되지 않은 보드 한 장이 남고,
/// Windows·Linux에서는 그 창 하나 때문에 앱이 끝나지도 않는다. 부활은 없다 —
/// 다음 실행이 기억하는 것은 자리뿐이다.
pub(super) fn close_popout_with_main(app: &AppHandle) {
    let Some(main) = app.get_webview_window("main") else {
        return;
    };
    let handle = app.clone();
    main.on_window_event(move |event| {
        if !matches!(event, tauri::WindowEvent::Destroyed) {
            return;
        }
        if let Some(popout) = handle.get_webview_window(BOARD_POPOUT_LABEL) {
            let _ = popout.close();
        }
    });
}

/// How many terminal rows a continuation capture keeps — Orca serialises the
/// pane with `serialize({ scrollback: 800 })` before building the handoff
/// prompt (`prepareAgentSessionContinuationFromPane`,
/// OnboardingInlineCommandTerminal-D_M5zIRV.js:26877).
pub(super) const CONTINUATION_CAPTURE_ROWS: usize = 800;

/// The tail of a grid as plain text, for a session handoff.
///
/// Orca captures ANSI-styled bytes and then strips every escape and control
/// byte back out (`cleanAgentSessionForkTranscript`,
/// AgentSessionContinuationDialog-CNEVLexr.js:37-60) because xterm's
/// serialiser only speaks bytes. This grid keeps text and style apart, so the
/// text is read directly and only the cleaner's OTHER rule is applied here:
/// runs of blank lines collapse so no more than three newlines stand together
/// (`newlineRun < 3`, :51) — a TUI redraw leaves screensful of nothing, and
/// nothing is not context.
pub(super) fn continuation_capture(grid: &zerocode_pty::TerminalGrid) -> String {
    let scrollback: Vec<String> = (0..grid.scrollback_len())
        .map(|index| grid.scrollback_line(index))
        .collect();
    let mut rows: Vec<String> = scrollback;
    rows.extend((0..grid.screen_rows()).map(|row| grid.line(row)));
    while rows.last().is_some_and(|line| line.is_empty()) {
        rows.pop();
    }
    let tail = rows.len().saturating_sub(CONTINUATION_CAPTURE_ROWS);
    let mut kept = String::new();
    let mut blank_run = 0usize;
    for line in &rows[tail..] {
        if line.is_empty() {
            blank_run += 1;
            if blank_run > 2 {
                continue;
            }
        } else {
            blank_run = 0;
        }
        if !kept.is_empty() {
            kept.push('\n');
        }
        kept.push_str(line);
    }
    kept.trim().to_string()
}

/// Everything the continue-in-new-session dialog needs to know about one pane.
///
/// Orca assembles the same record in its renderer
/// (`prepareAgentSessionContinuationFromPane`: agent from the pane's status,
/// transcript path from the provider session, captured text only when there is
/// no path, and the last prompt/answer as status hints). These four facts are
/// held HERE — `agent_terms`, `pane_sessions`, the pty grids, `pane_states` —
/// so the window asks once instead of keeping a second copy of each ledger.
#[derive(Debug, Clone, Serialize)]
pub(super) struct ContinuationSource {
    /// The agent running in this pane, when the ledger knows one.
    pub(super) agent: Option<&'static str>,
    /// The provider session's saved transcript, when the agent named one.
    /// Present means the captured text is not sent — the file is the better
    /// context and the capture would be a worse copy of it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) transcript_path: Option<String>,
    /// The pane's recent output as plain text, only when no transcript exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) captured: Option<String>,
    /// The person's last prompt and the agent's last answer — Orca's
    /// `lastPrompt`/`lastAssistantMessage` status hints, optional both there
    /// and here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) you: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) said: Option<String>,
}

/// The session a pane is on, if its agent has told us.
#[derive(Serialize)]
pub(super) struct PaneSession {
    pub(super) term: TermId,
    pub(super) agent: &'static str,
    pub(super) session: zerocode_core::ProviderSession,
    /// True when this window can actually reopen it. Two agents report a session
    /// and offer no way back to it, so the row says which — an offer that fails
    /// is worse than no offer.
    pub(super) resumable: bool,
}

/// Open a terminal running this agent's own resume command.
///
/// The argv is BUILT by the core table, never assembled here, and the session id
/// is re-validated on the way in — it arrives from the webview, which means it
/// arrives from something that could have been anything. `resume_argv` refuses a
/// key mismatch and refuses an id that would read as a flag.
/// What a mid-turn pane is told on its way back up. English because it is
/// spoken to the AGENT, the same register as the worker briefing — and it
/// says why, so an agent whose work was actually finished does not invent
/// new work to justify the nudge.
pub(super) const RESTART_NUDGE: &str = "The window restarted and cut your last turn short. \
Continue exactly where you left off; if the work was already finished, say so briefly.";

/// The file half of the reclaim, alone with the disk so a test can hold it.
/// Answers true when the file was removed, false when bytes turned up and it
/// was put back where it stood.
pub(super) fn reclaim_untitled_file(path: &str) -> Result<bool, String> {
    let hostage = format!("{path}.zerocode-reclaim");
    std::fs::rename(path, &hostage).map_err(|error| error.to_string())?;
    let held = std::fs::metadata(&hostage).map_err(|error| error.to_string())?;
    if held.len() > 0 {
        std::fs::rename(&hostage, path).map_err(|error| error.to_string())?;
        return Ok(false);
    }
    std::fs::remove_file(&hostage).map_err(|error| error.to_string())?;
    Ok(true)
}

/// Put the main window in front of the person, whatever state it is in.
///
/// `show` first: a window parked in the tray is hidden, not minimized, and
/// focusing a hidden window does nothing anyone can see. The focus itself is
/// queued to the main thread BEHIND the show and the unminimize, because
/// tao's `set_focus` reads `isMiniaturized` and `isVisible` before it acts
/// and would read the state those two are still on their way to changing.
/// On macOS that focus is `makeKeyAndOrderFront` plus
/// `activateIgnoringOtherApps`, which is the whole point — a sheet attached
/// to this window comes forward with it. Answers whether there was a window.
pub(super) fn bring_main_window_forward(app: &AppHandle) -> bool {
    let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) else {
        return false;
    };
    let _ = window.show();
    let _ = window.unminimize();
    let focusing = window.clone();
    let _ = app.run_on_main_thread(move || {
        let _ = focusing.set_focus();
    });
    true
}
