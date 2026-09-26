/* t-7288 · 실시간 조율 지도의 회귀.
 *
 * 픽스처는 프로덕션과 같은 네 문으로만 들어간다 — `__PANES__`·`__LEDGER__`·
 * `__COLUMNS__`·`__OVERLAYS__`. 그래야 `run`/`task_id`/`dispatch_id`가 Rust
 * 왕복을 지나 `places`까지 살아남는지가 실제로 재어진다(카드에 값을 직접 박는
 * 픽스처는 그 왕복이 키를 버리므로 거짓 초록이다).
 *
 * 여기서 고정하는 계약:
 *   ① 실시간은 **고르는** 그림이다 — 작업 목록이 보드의 기본값이다.
 *   ② 맥박은 **실제로 기록된 사건** 하나에 한 번이다 — 같은 판을 두 번 읽어도,
 *      숨겼다 돌아와도, 옛 snapshot이 늦게 와도, 기억의 상한을 넘은 뒤에도
 *      옛 사건이 새 사건이 되지 않는다. 수의 기준선도 새 사건과 함께만 움직인다.
 *   ③ 없는 사실은 **없다고 말한다** — 배달 상태·답장·소환자·보기 밖 끝점 수,
 *      그리고 결과와 의존 전이의 발생 시각.
 *   ③′ 사건을 누르면 **그 사건의** 근거로 간다 — 더 새 메시지나 새 시도로,
 *      같은 시도의 바뀐 사유나 철회된 사실로 대신 가지 않는다.
 *   ④ 보는 일은 아무것도 **소비하지 않는다** — 메일 확인도, 모델 호출도 없다.
 *   ⑤ 오래 사는 창에서도 장부는 **유계**이고, 수명 문을 지나면 시계·맥박이
 *      남지 않으며, 떠났다 돌아온 첫 판은 조용한 기준선이다(둘째 창).
 *   ⑥ 늦게 도착한 답은 성공이든 실패든 **아무것도 쓰지 않는다**, 지금의 실패는
 *      복구 카드를 세운다, 한 판은 제가 물은 원장 행만 읽는다(셋째 창).
 *
 *   node ui/tests/board-live.mjs
 */
import { mkdir } from "node:fs/promises";
import { pathToFileURL } from "node:url";
import { resolve } from "node:path";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";
import { installBoardWaits } from "./board-waits.mjs";

/* 한 판, 두 런.
 *
 *   run-1 코디네이터 좌석(term:301) → 구현자 term:302, 독립 검증자 term:303
 *   run-2 코디네이터 좌석(term:311) → 워커 term:312
 *   두 좌석 사이에 기록된 교신 한 줄 (cross-run)
 *   term:302 는 **주장만** 했다(reported) — 아무도 검증하지 않았다
 *   term:303 은 코디네이터가 **검증됨**으로 적은 사실을 든다
 *   term:304 는 벽 앞에 서 있고, worker:w-gone 은 판이 사라졌다
 *   term:305 는 run/worker/dispatch 가 없다 — 확인되지 않은 카드
 *   term:306 은 끝난 워커이고 그 아래 살아 있는 자식이 하나 있다
 *   t-later 는 선행 조건 하나를 기다린다 (명시적 게이트)
 */
export function liveFixture() {
  /* 판의 관계를 짓는 손을 **페이지 안에** 세운다. 이 함수는 통째로 직렬화되어
   * 브라우저에서 돌므로, 다음 판을 만드는 시험들이 부를 수 있는 곳은 여기뿐이다. */
  window.__LIVE_OVERLAYS__ = (now, over = {}) => {
    const message = (id, run, from, to, kind, at) => ({ id, run, from, to, kind, created_ms: at });
    return {
      latest: "term:302",
      mail: [
        /* 배정: 코디네이터 좌석 → 구현자. 실제 dispatch 메시지 행이다. */
        { from: "term:301", to: "term:302", count: 1, unread: 0, at: now - 200_000, verb: "mail",
          last_message: message("m-dispatch-1", "run-1", "run:run-1", "worker:w-impl",
            "dispatch", now - 200_000) },
        /* 구현자 → 코디네이터, 아직 확인되지 않은 보고. */
        { from: "term:302", to: "term:301", count: 2, unread: 1, at: now - 100_000, verb: "mail",
          last_message: message("m-report-1", "run-1", "worker:w-impl", "run:run-1",
            "worker_done", now - 100_000) },
        /* 두 코디네이터 사이의 교신 — 이름이나 cwd가 아니라 기록된 주소로. */
        { from: "term:301", to: "term:311", count: 1, unread: 0, at: now - 50_000, verb: "mail",
          last_message: message("m-cross-1", "run-1", "run:run-1", "run:run-2",
            "status", now - 50_000) },
        ...(over.mail ?? []),
      ],
      dependencies: [
        { from: "term:302", to: "term:303", count: 1, unread: 0, at: now - 180_000, verb: "blocks" },
      ],
      task_dependencies: [
        { run: "run-1", task: "t-verify", dependency: "t-impl", task_state: "pending",
          dependency_state: "dispatched", task_created_ms: now - 180_000,
          from: "term:302", to: "term:303" },
        ...(over.task_dependencies ?? []),
      ],
      merge: [],
    };
  };
  const ROOT = "/repos/zerocode";
  const WT_A = "/repos/zerocode-wt/a";
  const WT_B = "/repos/zerocode-wt/b";
  const now = Date.now();
  window.__LIVE_NOW__ = now;
  const ledgerRow = (over) => ({
    run: "run-1", worker: "", agent: "codex", state: "working", ledger: "active",
    hearing: "pending", hearing_at: now - 120_000, checkout: "", task: "", task_id: "",
    reported: false, review: null, dispatch_id: "", dispatch_started_ms: now - 300_000,
    retry_of: null, term: null, at: now - 300_000, model: null, effort: null, pane: "%1",
    asking: false, wall: null, quiet_at: null, pane_missing_since_ms: null, ...over,
  });
  const card = (pane, heading, state, over = {}) => ({
    pane, heading, state, agent: "codex", project: ROOT, worktree: "main", task: "",
    you: "", said: "", ask: "", parent: "", ledger: "", unseen: false,
    changed_at: now - 30_000, at: now - 30_000, ...over,
  });
  projects = [{ name: "zerocode", path: ROOT, worktrees: [
    { path: ROOT, branch: "main", base: "", is_main: true },
    { path: WT_A, branch: "wt/a", base: "main", is_main: false },
    { path: WT_B, branch: "wt/b", base: "main", is_main: false },
  ] }];
  window.__PANES__ = [301, 302, 303, 304, 306, 307, 311, 312].map((term) => ({
    term, agent: "codex", state: "working", at: now - 10_000,
    state_started_at: now - 10_000, resumable: false,
    ...(term === 307 ? { parent: 306 } : {}),
  }));
  window.__LEDGER__ = [
    ledgerRow({ worker: "w-c1", checkout: ROOT, task: "조율", task_id: "t-c1",
      dispatch_id: "dp-c1", term: 301 }),
    /* 주장만 있는 결과: 워커는 보고했고 코디네이터는 아직 아무것도 적지 않았다. */
    ledgerRow({ worker: "w-impl", checkout: WT_A, task: "구현", task_id: "t-impl",
      dispatch_id: "dp-impl", reported: true, term: 302 }),
    /* 받아들여진 사실: 코디네이터가 검증됨을 적었다. */
    ledgerRow({ worker: "w-verify", checkout: WT_B, task: "독립 검증", task_id: "t-verify",
      dispatch_id: "dp-verify", reported: true, review: { verified: true }, term: 303 }),
    /* 벽 앞에서 기다리는 워커 — 사유도 시작도 원장이 적었다. */
    ledgerRow({ worker: "w-wall", checkout: WT_A, task: "재시도", task_id: "t-retry",
      dispatch_id: "dp-retry-2", retry_of: "dp-retry-1", term: 304,
      wall: { wall: "m-wall", observed_at_ms: now - 90_000, resets_at_ms: now + 600_000,
        reset_waitable: true, stands_until_ms: now + 600_000 } }),
    /* 판이 사라진 워커: 좌석이 없고 사라진 때가 적혀 있다. */
    ledgerRow({ worker: "w-gone", checkout: WT_B, task: "사라진 판", task_id: "t-gone",
      dispatch_id: "dp-gone", term: null, hearing: "gone",
      pane_missing_since_ms: now - 240_000 }),
    /* 끝난 워커와 그 아래 살아 있는 자식. */
    ledgerRow({ worker: "w-parent", checkout: ROOT, task: "끝난 일", task_id: "t-parent",
      dispatch_id: "dp-parent", reported: true, state: "idle", term: 306 }),
    ledgerRow({ worker: "w-child", checkout: ROOT, task: "이어받은 일", task_id: "t-child",
      dispatch_id: "dp-child", term: 307 }),
    ledgerRow({ run: "run-2", worker: "w-c2", checkout: ROOT, task: "다른 런 조율",
      task_id: "t-c2", dispatch_id: "dp-c2", term: 311 }),
    ledgerRow({ run: "run-2", worker: "w-other", checkout: ROOT, task: "다른 런 구현",
      task_id: "t-other", dispatch_id: "dp-other", term: 312 }),
  ];
  window.__COLUMNS__ = [
    { bucket: "working", cards: [
      card("term:301", "조율", "working", { task: "조율" }),
      card("term:302", "구현", "working", { worktree: "wt/a", task: "구현" }),
      card("term:303", "독립 검증", "working", { worktree: "wt/b", task: "독립 검증" }),
      card("term:304", "재시도", "working", { worktree: "wt/a", task: "재시도" }),
      /* 주체가 없는 카드: 원장이 이 자리를 누구의 것이라고 말하지 않았다. */
      card("term:305", "원장이 모르는 판", "working"),
      card("term:306", "끝난 일", "done"),
      card("term:307", "이어받은 일", "working", { parent: "term:306" }),
      card("term:311", "다른 런 조율", "working"),
      card("term:312", "다른 런 구현", "working"),
      card("worker:w-gone", "사라진 판", "working", { worktree: "wt/b" }),
    ] },
  ];
  window.__OVERLAYS__ = window.__LIVE_OVERLAYS__(now);
  /* 실시간 지도는 카드 그림 위의 것이다 (t-9532). 관계 탭은 행성계로 열리므로 지도를 재는
   * 이 시험들은 탭의 토글로 카드를 먼저 고른다 — 행성계가 선 판의 지도는 board-orbit ⑧이 잰다. */
  if (typeof setAgentOrbitChoice === "function") setAgentOrbitChoice(document.getElementById("board-view"), "cards");
  agentBoardMode = "tasks";
  agentGraphSelectedKey = null;
  agentGraphSelectedEdgeKey = null;
  agentGraphOverlayMode = "none";
  agentGraphFollowing = false;
  agentGraphScopeKey = "";
  boardQuery = "";
  boardBroken = false;
  if (typeof activeTabId !== "undefined" && activeTabId === "board") dropTab("board");
  openBoard();
}

/* 판을 한 번 더 받는다 — 프로덕션과 같은 문으로. */
const repaint = (page) => page.evaluate(async () => {
  await paintBoardView(undefined, { force: true });
  await window.__BOARD_SETTLED__();
});

const liveState = (page) => page.evaluate(() => ({
  events: agentGraphLiveRecentEvents().map((event) => ({ key: event.key, kind: event.kind })),
  handles: agentGraphLiveHandles(),
  coverage: agentGraphLiveCoverage(),
  pulsing: [...document.querySelectorAll("#board-view [data-live-beat]")].length,
}));

/* 세 창, 한 suite. 둘째와 셋째는 제 창에서 돈다 — 상한을 몇 배로 넘기는 흐름과
 * 붙잡은 답은 장부의 기억을 크게 움직이므로, 첫 창의 사례들과 섞이면 한쪽의
 * 빨강이 다른 쪽이 남긴 자국이 된다. */
export async function testBoardLive(browser, origin, ok) {
  await testLiveMap(browser, origin, ok);
  await testLiveLedgerBounds(browser, origin, ok);
  await testLateAnswers(browser, origin, ok);
}

async function testLiveMap(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await installBoardWaits(page);
    await page.evaluate(liveFixture);
    await page.waitForSelector(".task-board-row");

    /* ---- ① 고르는 그림 ------------------------------------------------- */

    ok("live_view_is_optional_and_keeps_task_list_default", await page.evaluate(() => {
      const view = document.querySelector("#board-view");
      return agentBoardMode === "tasks" && agentGraphLiveOn() === false
        && view.classList.contains("is-task-board")
        && view.querySelector(".agent-graph-live") !== null;
    }));

    await page.evaluate(async () => {
      document.querySelector('[data-board-mode="graph"]').click();
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
    });
    /* 지도를 끈 판은 이 변경 **이전의 판**이다: 실시간 선도, 맥박도, 기다림의
     * 칸도 DOM 에 없다. 빈 칸을 늘 세워 두고 CSS 로 접는 것이 아니라 아예
     * 짓지 않는다 — 보드의 기본값에 카드마다 요소 셋을 얹지 않기 위해서다. */
    ok("the graph without the live map is the graph it was", await page.evaluate(() => {
      const view = document.querySelector("#board-view");
      return !view.classList.contains("is-live-map")
        && view.querySelectorAll(".agent-graph-edge.is-live-relation").length === 0
        && view.querySelectorAll("[data-live-beat]").length === 0
        && view.querySelectorAll(".agent-graph-wait").length === 0;
    }));

    /* 켜는 판은 **조용히** 선다: 이미 쌓여 있던 관계가 한꺼번에 맥박이 되지
     * 않는다. 이것이 실시간 지도의 첫 계약이다. */
    await page.evaluate(async () => {
      document.querySelector(".agent-graph-live").click();
      await window.__BOARD_SETTLED__();
    });
    const opened = await liveState(page);
    ok("turning_the_live_map_on_adopts_the_present_quietly",
      opened.events.length === 0 && opened.pulsing === 0 && opened.handles.timers === 0,
      JSON.stringify(opened));
    ok("the live map draws the recorded relations it holds", await page.evaluate(() => {
      const view = document.querySelector("#board-view");
      return view.classList.contains("is-live-map")
        && view.querySelectorAll(".agent-graph-edge.is-live-relation").length >= 3;
    }));

    /* 그리고 지도가 **켜져 있는 채로** 작업 목록으로 돌아가도 그 목록은 지도가
     * 없던 때와 같은 목록이다 — 기본 보기를 바꾸지 않는다는 약속은 손잡이를
     * 끈 판이 아니라 켠 판에서 지켜져야 한다. */
    ok("the_task_list_is_unchanged_while_the_live_map_is_on", await page.evaluate(async () => {
      const view = document.querySelector("#board-view");
      const rows = () => [...view.querySelectorAll(".task-board-row")]
        .map((row) => row.textContent).join("\u001e");
      /* 두 목록을 **같은 시계**로 견준다: 행이 나이를 낱말로 들고 있어서, 분
       * 경계가 두 읽기 사이에 끼면 옳게 움직인 낱말이 다름으로 읽힌다. 이
       * 사례가 묻는 것은 지도가 목록을 바꾸는가이지 시간이 흐르는가가 아니다. */
      const trueNow = Date.now;
      const pinned = trueNow.call(Date);
      Date.now = () => pinned;
      try {
      view.querySelector('[data-board-mode="tasks"]').click();
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
      /* 세는 자리는 **보이는 작업 표면**이다. 숨은 관계 그림은 지난 판의 DOM을
       * 그대로 들고 있고(다시 열 때 쓰라고), 그것은 작업 목록이 달라진 것이
       * 아니다 — 여기서 고정하려는 약속은 「사람이 보는 기본 화면이 그대로인가」다. */
      const surface = view.querySelector(".task-board-surface");
      const withMap = { rows: rows(), live: view.classList.contains("is-live-map"),
        beats: surface.querySelectorAll("[data-live-beat]").length,
        waits: surface.querySelectorAll(".agent-graph-wait").length,
        events: surface.querySelectorAll(".agent-live-events").length,
        graphHidden: view.querySelector(".agent-graph-surface").hidden };
      setAgentGraphLive(view, false);
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
      const without = rows();
      setAgentGraphLive(view, true);
      view.querySelector('[data-board-mode="graph"]').click();
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
      window.__TASKLIST__ = { ...withMap, rows: undefined, same: withMap.rows === without };
      return withMap.rows === without && withMap.rows !== "" && withMap.graphHidden
        && !withMap.live && withMap.beats === 0 && withMap.waits === 0 && withMap.events === 0;
      } finally { Date.now = trueNow; }
    }), await page.evaluate(() => JSON.stringify(window.__TASKLIST__ ?? null)));

    /* 그리고 손잡이 하나가 그 셋을 실제로 붙였다 뗀다 — 카드가 다시 지어지지
     * 않으면 켠 판에도 칸이 서지 않고, 끈 판에서도 칸이 남는다. */
    ok("the_wait_cell_is_built_by_the_toggle_and_not_before", await page.evaluate(async () => {
      const view = document.querySelector("#board-view");
      const cells = () => view.querySelectorAll(".agent-graph-wait").length;
      const cards = () => view.querySelectorAll(".agent-graph-node.is-agent").length;
      const on = { cells: cells(), cards: cards() };
      setAgentGraphLive(view, false);
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
      const off = { cells: cells(), cards: cards() };
      setAgentGraphLive(view, true);
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
      const back = cells();
      return on.cells === on.cards && on.cards > 0 && off.cells === 0
        && off.cards === on.cards && back === on.cells;
    }));

    /* ---- ② 맥박은 실제 사건 하나에 한 번 ------------------------------- */

    const pulsed = await page.evaluate(async () => {
      const now = window.__LIVE_NOW__;
      window.__OVERLAYS__ = window.__LIVE_OVERLAYS__(now, { mail: [
        { from: "term:311", to: "term:301", count: 1, unread: 1, at: now + 1_000, verb: "mail",
          last_message: { id: "m-cross-2", run: "run-2", from: "run:run-2", to: "run:run-1",
            kind: "status", created_ms: now + 1_000 } },
      ] });
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
      return [...document.querySelectorAll("#board-view [data-live-beat]")]
        .map((node) => node.getAttribute("data-graph-edge")
          ?? node.parentElement?.getAttribute("data-graph-edge") ?? node.dataset.graphKey);
    });
    ok("a_new_actual_event_pulses_once",
      pulsed.filter(Boolean).length === 1
      && pulsed[0] === "overlay:mail:agent:term:311>agent:term:301",
      JSON.stringify(pulsed));

    /* 그리고 그 맥박이 **실제로 돈다**. 속성만 보면 CSS 쪽에서 규칙 하나가
     * 이 맥박을 먹어도 초록이다 — 메일 선의 무한 흐름을 세우는 규칙이 바로 그
     * 자리에 서 있고, 한 단만 더 구체적이면 animation:none 이 이긴다. */
    ok("the_pulse_actually_runs_and_is_not_eaten_by_the_flow_stop_rule",
      await page.evaluate(async () => {
        /* 제 사건을 스스로 낸다 — 앞 사례의 맥박을 빌려 읽으면 그 사이의 왕복이
         * 맥박의 수명(900 ms)보다 길어지는 날 이 사례가 이유 없이 빨개진다. */
        const now = window.__LIVE_NOW__;
        window.__OVERLAYS__ = window.__LIVE_OVERLAYS__(now, { mail: [
          { from: "term:312", to: "term:311", count: 1, unread: 1, at: now + 2_000, verb: "mail",
            last_message: { id: "m-runs", run: "run-2", from: "worker:w-other",
              to: "run:run-2", kind: "status", created_ms: now + 2_000 } },
        ] });
        await paintBoardView(undefined, { force: true });
        await window.__BOARD_SETTLED__();
        const beaten = document.querySelector(
          '#board-view [data-graph-edge="overlay:mail:agent:term:312>agent:term:311"] path[data-live-beat]');
        if (!beaten) return false;
        const dress = getComputedStyle(beaten);
        return ["zc-live-edge-a", "zc-live-edge-b"].includes(dress.animationName)
          && dress.animationIterationCount === "1";
      }));

    const again = await page.evaluate(async () => {
      const before = agentGraphLiveRecentEvents().length;
      await paintBoardView(undefined, { force: true });
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
      return { before, after: agentGraphLiveRecentEvents().length };
    });
    /* 같은 판을 두 번 더 읽어도 사건의 수는 그대로다. 절대값을 못 박지 않는
     * 것은 이 사례의 요점이 「몇 건인가」가 아니라 「다시 세지 않는가」이기
     * 때문이다 — 앞 사례가 하나를 더 내면 그 수는 따라 움직인다. */
    ok("duplicate_snapshot_does_not_replay_pulses",
      again.before === again.after && again.before > 0, JSON.stringify(again));

    /* 처음 서는 카드의 첫 배정도 맥박을 받는다. 장부는 사건을 본 자리에서
     * 한 번 얹지만 그때 그 카드는 아직 없었다 — 카드가 다 선 뒤에 한 번 더
     * 얹지 않으면 새 워커의 첫 사건만 조용히 사라진다. */
    ok("a_card_that_first_appears_with_its_assignment_still_pulses",
      await page.evaluate(async () => {
        const now = window.__LIVE_NOW__;
        window.__PANES__ = [...window.__PANES__,
          { term: 321, agent: "codex", state: "working", at: now, state_started_at: now, resumable: false }];
        window.__LEDGER__ = [...window.__LEDGER__, {
          ...window.__LEDGER__[0], worker: "w-fresh", task: "갓 온 일", task_id: "t-fresh",
          dispatch_id: "dp-fresh", term: 321, reported: false, review: null }];
        window.__COLUMNS__[0].cards.push({ ...window.__COLUMNS__[0].cards[1],
          pane: "term:321", heading: "갓 온 일", task: "갓 온 일", parent: "" });
        await paintBoardView(undefined, { force: true });
        await window.__BOARD_SETTLED__();
        return document.querySelector(
          '#board-view .agent-graph-node[data-graph-key="agent:term:321"][data-live-beat]') !== null;
      }));

    /* 같은 밀리초의 서로 다른 두 사건은 합쳐지지 않는다. A@T → B@T → A@T 를
     * 그 차례로 먹인다: 둘째는 새 사건, 셋째는 이미 본 것이다. */
    const sameStamp = await page.evaluate(async () => {
      const now = window.__LIVE_NOW__;
      const T = now + 5_000;
      const edge = (id) => ({ mail: [
        { from: "term:303", to: "term:301", count: 1, unread: 0, at: T, verb: "mail",
          last_message: { id, run: "run-1", from: "worker:w-verify", to: "run:run-1",
            kind: "status", created_ms: T } },
      ] });
      const step = async (id) => {
        window.__OVERLAYS__ = window.__LIVE_OVERLAYS__(now, edge(id));
        await paintBoardView(undefined, { force: true });
        await window.__BOARD_SETTLED__();
        return agentGraphLiveRecentEvents().map((event) => event.key);
      };
      return { a: await step("m-same-a"), b: await step("m-same-b"), again: await step("m-same-a") };
    });
    ok("two_real_events_at_one_timestamp_are_not_merged",
      sameStamp.b.length === sameStamp.a.length + 1
      && sameStamp.b[0].endsWith("m-same-b") && sameStamp.b[1].endsWith("m-same-a"),
      JSON.stringify(sameStamp));
    ok("an_old_event_returning_at_the_same_timestamp_does_not_pulse_again",
      sameStamp.again.length === sameStamp.b.length,
      JSON.stringify(sameStamp.again));

    /* 상한을 넘기면 완전한 중복 제거를 주장하지 않고 조용히 다시 맞춘다 —
     * 그 사실을 장부가 스스로 말한다. 그리고 「조용히」는 말 그대로다: 그 시각의
     * 기억이 온전하지 않은 동안 그 시각의 모르는 사건은 새 것도 옛것도 아닌 채로
     * 맞추고, 시각이 지나가야 다시 센다. 포화 **뒤**에 옛 사건이 다시 와도 목록의
     * 머리에 서지 않고 실제 맥박도 뛰지 않는다 — 길이가 아니라 머리의 신원과
     * 켜진 맥박을 본다. 상한 **안**의 두 사건이 각각 한 번이라는 양성은 위의
     * `two_real_events_at_one_timestamp_are_not_merged`가 그대로 지킨다. */
    const saturated = await page.evaluate(async () => {
      const now = window.__LIVE_NOW__;
      const T = now + 9_000;
      const edgeKey = "overlay:mail:agent:term:306>agent:term:301";
      const step = async (id, at = T) => {
        window.__OVERLAYS__ = window.__LIVE_OVERLAYS__(now, { mail: [
          { from: "term:306", to: "term:301", count: 1, unread: 0, at, verb: "mail",
            last_message: { id, run: "run-1", from: "worker:w-parent", to: "run:run-1",
              kind: "status", created_ms: at } },
        ] });
        await paintBoardView(undefined, { force: true });
        await window.__BOARD_SETTLED__();
      };
      for (let at = 0; at < 20; at += 1) await step(`m-burst-${at}`);
      const coverage = agentGraphLiveCoverage();
      /* 켜진 맥박이 모두 꺼진 뒤에 되돌린다 — 그래야 재생이 **새로** 켠 맥박을
       * 셀 수 있다. */
      await window.__UNTIL__(() => agentGraphLiveHandles().pulses === 0, "pulses expire", 5_000);
      const head = () => agentGraphLiveRecentEvents()[0]?.key ?? null;
      const lit = () => ({ pulses: agentGraphLiveHandles().pulses,
        beats: document.querySelectorAll(
          `#board-view .agent-graph-edges [data-graph-edge="${edgeKey}"] path[data-live-beat]`).length });
      const before = head();
      /* 포화 전에 본 사건 하나와, 포화 뒤에 온 사건 하나를 다시 보낸다. */
      await step("m-burst-0");
      const first = { head: head(), ...lit() };
      await step("m-burst-17");
      const second = { head: head(), ...lit() };
      /* 인스펙터가 그 한계를 같은 낱말로 되풀이하는가 — 시각이 지나가 그 관계가
       * 다시 온전해지기 **전에** 읽는다. */
      const view = document.querySelector("#board-view");
      selectAgentGraphEntity(view, "agent:term:301");
      const notes = [...view.querySelectorAll(".agent-live-events-note")]
        .map((node) => node.textContent).join(" ");
      const said = notes.includes(t("board.live.coverageTruncated", "중복 제거 범위가 끊긴 관계 {{count}}개 — 그 구간의 사건은 표시되지 않을 수 있습니다.",
        { count: agentGraphLiveCoverage().truncated }));
      /* 시각이 지나가면 그 관계는 다시 온전하다 — 다음 실제 사건은 뛴다. */
      await step("m-burst-next", T + 1);
      const next = { head: head(), ...lit() };
      return { coverage, before, first, second, said, next };
    });
    ok("a_saturated_relation_says_its_dedupe_coverage_broke_instead_of_claiming_it",
      saturated.coverage.truncated > 0 && saturated.coverage.complete === false,
      JSON.stringify(saturated));
    ok("past_the_same_timestamp_limit_an_old_event_does_not_return_as_new",
      saturated.first.head === saturated.before && saturated.second.head === saturated.before
      && saturated.first.pulses === 0 && saturated.first.beats === 0
      && saturated.second.pulses === 0 && saturated.second.beats === 0,
      JSON.stringify(saturated));
    ok("after_the_saturated_timestamp_passes_the_next_real_event_pulses",
      saturated.next.head?.endsWith("m-burst-next") === true && saturated.next.pulses > 0,
      JSON.stringify(saturated));
    ok("the_inspector_repeats_that_limit_in_words", saturated.said, JSON.stringify(saturated));

    /* 같은 자리가 판을 하나 건너뛰고 다시 뛸 때도 실제로 뛴다. 박자를 판마다
     * 하나로 번갈아 적으면 건너뛴 자리가 두 판 만에 같은 글자를 다시 받고,
     * 같은 값을 다시 쓰는 것은 쓰기가 아니므로 그 맥박은 조용히 사라진다. */
    ok("a_relation_that_skips_a_snapshot_still_pulses_when_it_returns",
      await page.evaluate(async () => {
        const now = window.__LIVE_NOW__;
        const key = "overlay:mail:agent:term:304>agent:term:301";
        /* 맞히기용 겹(`.agent-graph-edge-targets`)도 같은 키를 들고 있으므로
         * 선을 그리는 겹을 이름으로 집는다 — 그러지 않으면 맥박이 제대로
         * 적혀 있어도 엉뚱한 요소를 읽고 빨개진다. */
        const beat = () => document.querySelector(
          `#board-view .agent-graph-edges [data-graph-edge="${key}"] path`)
          ?.getAttribute("data-live-beat") ?? null;
        const mail = (id, from, at) => ({ columns: [], overlays: { mail: [
          { from, to: "term:301", count: 1, unread: 0, at, verb: "mail",
            last_message: { id, run: "run-1", from: `worker:${from}`, to: "run:run-1",
              kind: "status", created_ms: at } },
        ] } });
        /* 선이 서 있어야 맥박이 앉을 자리가 있으므로 한 번은 실제로 그린다. */
        window.__OVERLAYS__ = window.__LIVE_OVERLAYS__(now, mail("m-skip-1", "term:304", now + 40_000).overlays);
        await paintBoardView(undefined, { force: true });
        await window.__BOARD_SETTLED__();
        const first = beat();
        /* 나머지 둘은 장부에 바로 먹인다 — 판을 세 번 다시 그리면 그 사이에
         * 맥박이 수명을 다해, 이 사례가 재려는 「아직 빛나는 자리가 다시
         * 뛴다」가 아니라 「꺼졌다가 새로 뛴다」를 재게 된다. */
        agentGraphLiveObserve(mail("m-skip-2", "term:306", now + 41_000), new Map(), Date.now());
        const between = beat();
        agentGraphLiveObserve(mail("m-skip-3", "term:304", now + 42_000), new Map(), Date.now());
        const third = beat();
        window.__SKIP__ = { first, between, third,
          pulses: agentGraphLiveHandles().pulses };
        return first !== null && between === first && third !== null && third !== first;
      }), await page.evaluate(() => JSON.stringify(window.__SKIP__ ?? null)));

    /* out-of-order: 옛 stamp 의 판은 상태를 과거로 돌리지 않는다 — 목록의 머리도,
     * 수의 기준선도, 인스펙터의 줄도. 목록의 **길이**는 상한(24)에 닿은 뒤로는
     * 새 사건이 와도 그대로라 아무것도 증명하지 못하므로, 머리의 신원과 실제로
     * 센 증가분과 그 줄이 화면에 쓴 낱말을 본다.
     *
     *   200/8통 → (옛) 100/3통 → 201/9통
     *
     * 옛 판이 기준선을 3으로 되돌리면 마지막 판은 「이번 판에 6통」이라 말한다.
     * 최신 관측 8 이후 실제로 늘어난 것은 1통이다. */
    const reordered = await page.evaluate(async () => {
      const now = window.__LIVE_NOW__;
      const view = document.querySelector("#board-view");
      const T = now + 10_000;
      const step = async (id, at, count) => {
        window.__OVERLAYS__ = window.__LIVE_OVERLAYS__(now, { mail: [
          { from: "term:311", to: "term:312", count, unread: 0, at, verb: "mail",
            last_message: { id, run: "run-2", from: "run:run-2", to: "worker:w-other",
              kind: "status", created_ms: at } },
        ] });
        await paintBoardView(undefined, { force: true });
        await window.__BOARD_SETTLED__();
        const [head] = agentGraphLiveRecentEvents();
        return head ? { key: head.key, added: head.added } : null;
      };
      const seen = await step("m-order-199", T + 199, 7);
      const latest = await step("m-order-200", T + 200, 8);
      const stale = await step("m-order-100", T + 100, 3);
      const next = await step("m-order-201", T + 201, 9);
      /* 그 줄이 화면에 쓴 낱말 — 증가분이 1이면 「늘었습니다」 줄은 서지 않는다. */
      selectAgentGraphEntity(view, "agent:term:311");
      const row = [...view.querySelectorAll(".agent-live-event-main")]
        .find((one) => one.dataset.liveEvent.endsWith("m-order-201"));
      return { seen, latest, stale, next,
        said: row?.querySelector(".agent-live-event-added")?.textContent ?? null, drawn: Boolean(row) };
    });
    ok("an_out_of_order_snapshot_does_not_move_state_backwards",
      reordered.latest?.key.endsWith("m-order-200") === true
      && reordered.stale?.key === reordered.latest.key,
      JSON.stringify(reordered));
    ok("a_stale_snapshot_does_not_rewind_the_count_baseline",
      reordered.latest?.added === 1 && reordered.next?.key.endsWith("m-order-201") === true
      && reordered.next.added === 1 && reordered.drawn && reordered.said === null,
      JSON.stringify(reordered));

    /* 같은 시각의 **이미 본** 사건이 다시 와도 수의 기준선은 내려가지 않는다. 옛
     * stamp만 막으면 같은 시각의 옛 ID가 그 문을 지난다 — 같은 밀리초의 두 실제
     * 사건을 받는 양성(`two_real_events_at_one_timestamp_are_not_merged`)이 바로 그
     * 길을 연다.
     *
     *   m3@T/3통 → m8@T/8통 → (다시) m3@T/3통 → m9@T+1/9통
     *
     * 재전달이 기준선을 3으로 내리면 마지막 판은 「이번 판에 6통」이라 말한다.
     * 최신 관측 8 이후 실제로 는 것은 1통이다. 그리고 수가 **줄어든** 새 사건은
     * (보존 기간·주소 재배치) 증가를 추정하지 않고 모름으로 서며, 그 뒤로는 새
     * 수에서 다시 센다. 모두 실제 그리기 문을 지나고, 인스펙터가 쓴 낱말을 읽는다. */
    const replayed = await page.evaluate(async () => {
      const now = window.__LIVE_NOW__;
      const view = document.querySelector("#board-view");
      const T = now + 11_000;
      const step = async (id, at, count) => {
        window.__OVERLAYS__ = window.__LIVE_OVERLAYS__(now, { mail: [
          { from: "term:307", to: "term:301", count, unread: 0, at, verb: "mail",
            last_message: { id, run: "run-1", from: "worker:w-child", to: "run:run-1",
              kind: "status", created_ms: at } },
        ] });
        await paintBoardView(undefined, { force: true });
        await window.__BOARD_SETTLED__();
        const [head] = agentGraphLiveRecentEvents();
        return head ? { key: head.key, added: head.added } : null;
      };
      /* 그 줄이 화면에 쓴 증가의 낱말 — 없으면 `null`. */
      const said = (id) => {
        selectAgentGraphEntity(view, "agent:term:301");
        const row = [...view.querySelectorAll(".agent-live-event-main")]
          .find((one) => one.dataset.liveEvent.endsWith(id));
        return { drawn: Boolean(row),
          added: row?.querySelector(".agent-live-event-added")?.textContent ?? null };
      };
      const first = await step("m-replay-3", T, 3);
      const second = await step("m-replay-8", T, 8);
      const again = await step("m-replay-3", T, 3);
      const next = await step("m-replay-9", T + 1, 9);
      const nextSaid = said("m-replay-9");
      const shrunk = await step("m-replay-shrunk", T + 2, 7);
      const shrunkSaid = said("m-replay-shrunk");
      const resumed = await step("m-replay-11", T + 3, 9);
      return { first, second, again, next, nextSaid, shrunk, shrunkSaid, resumed,
        wrong: t("board.live.added", "이번 판에 {{count}}통 늘었습니다", { count: 6 }) };
    });
    ok("a_replayed_event_at_the_same_timestamp_does_not_lower_the_count_baseline",
      replayed.second?.key.endsWith("m-replay-8") === true && replayed.second.added === 5
      && replayed.again?.key === replayed.second.key
      && replayed.next?.key.endsWith("m-replay-9") === true && replayed.next.added === 1
      && replayed.nextSaid.drawn && replayed.nextSaid.added === null,
      JSON.stringify(replayed));
    ok("a_new_event_whose_count_shrank_says_no_increase_and_counts_on_from_there",
      replayed.shrunk?.key.endsWith("m-replay-shrunk") === true && replayed.shrunk.added === null
      && replayed.shrunkSaid.drawn && replayed.shrunkSaid.added === null
      && replayed.resumed?.key.endsWith("m-replay-11") === true && replayed.resumed.added === 2,
      JSON.stringify(replayed));

    /* 가릴 수 없는 판(`adopt` — 같은 시각의 기억이 온전하지 않은 관계의 모르는
     * 사건)의 수도 기준선이 되지 않는다. 그 사건은 새 것일 수도 기억에서 밀려난
     * 옛것일 수도 있으므로, 그 뒤 첫 새 사건의 증가는 추정하지 않고 모름이다. */
    const adopted = await page.evaluate(async () => {
      const now = window.__LIVE_NOW__;
      const T = now + 12_000;
      /* 장부에 바로 먹이는 판도 **그리는 판과 같은** 카드·자리·원장 행을 든다 — 빈
       * 자리를 주면 두 끝의 주체가 달라져 기준선이 어느 제품에서든 끊기고, 이
       * 사례는 아무것도 재지 못한다. */
      const model = agentGraphFullModel(document.querySelector("#board-view"));
      const mail = (id, at, count) => ({ columns: model.source.columns, overlays: { mail: [
        { from: "term:304", to: "term:303", count, unread: 0, at, verb: "mail",
          last_message: { id, run: "run-1", from: "worker:w-wall", to: "worker:w-verify",
            kind: "status", created_ms: at } },
      ] } });
      const feed = (answer) => agentGraphLiveObserve(answer, model.source.places, Date.now(),
        { ledger: model.source.ledger });
      const keep = agentGraphLiveCoverage().sameStampKeep;
      /* 그 시각의 기억을 넘치게 채운다 — 넘친 것부터는 조용히 맞춘다. */
      for (let at = 0; at <= keep; at += 1) feed(mail(`m-adopt-${at}`, T, 20 + at));
      /* 넘친 뒤에 온, 더 작은 수의 모르는 사건. */
      feed(mail("m-adopt-low", T, 12));
      /* 시각이 지나간 첫 새 사건 — 실제 그리기 문으로. */
      window.__OVERLAYS__ = window.__LIVE_OVERLAYS__(now, mail("m-adopt-next", T + 1, 40).overlays);
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
      const [head] = agentGraphLiveRecentEvents();
      return { head: head ? { key: head.key, added: head.added } : null,
        truncated: agentGraphLiveCoverage().truncated };
    });
    ok("an_adopted_event_does_not_become_the_count_baseline",
      adopted.head?.key.endsWith("m-adopt-next") === true && adopted.head.added === null,
      JSON.stringify(adopted));

    /* 재시작·처음 판은 활동이 아니다. */
    ok("old_initial_snapshot_and_restart_do_not_invent_events", await page.evaluate(async () => {
      boardBroken = true;
      await paintBoardView(undefined, { force: true });
      retryBoardPaint();
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
      return agentGraphLiveRecentEvents().length === 0
        && document.querySelectorAll("#board-view [data-live-beat]").length === 0;
    }));

    /* 판이 둘일 때도 둘 다 맥박을 받는다. 보드를 팝아웃으로 빼면 본창의 탭
     * 장부는 빈손이 되지만 그림은 저쪽 문서에 멀쩡히 서 있다 — 그래서 판을
     * 찾는 손은 탭이 아니라 문서에게 묻는다. 복제된 판이 그 계약의 대역이다. */
    ok("a_second_board_surface_is_dressed_by_the_same_ledger",
      await page.evaluate(async () => {
        const original = document.querySelector("#board-view");
        const copy = original.cloneNode(true);
        copy.removeAttribute("id");
        original.parentNode.append(copy);
        try {
          paintAgentGraph(copy, agentGraphFullModel(original));
          const found = agentGraphLiveViews().length;
          const now = window.__LIVE_NOW__;
          window.__OVERLAYS__ = window.__LIVE_OVERLAYS__(now, { mail: [
            { from: "term:307", to: "term:306", count: 1, unread: 0, at: now + 3_000,
              verb: "mail",
              last_message: { id: "m-two-surfaces", run: "run-1", from: "worker:w-child",
                to: "worker:w-parent", kind: "status", created_ms: now + 3_000 } },
          ] });
          await paintBoardView(undefined, { force: true });
          await window.__BOARD_SETTLED__();
          paintAgentGraph(copy, agentGraphFullModel(original));
          return found >= 2
            && original.querySelectorAll("[data-live-beat]").length > 0
            && copy.querySelectorAll("[data-live-beat]").length > 0;
        } finally {
          copy.remove();
        }
      }));

    /* ---- 자리는 신원이 아니다 ------------------------------------------- */

    ok("a_reused_pane_does_not_inherit_the_previous_subjects_events",
      await page.evaluate(async () => {
        const now = window.__LIVE_NOW__;
        const pane = "term:312";
        const place = (dispatch) => new Map([[pane, { run: "run-2", workerId: "w-other",
          dispatchId: dispatch, taskId: "t-other", dispatchStarted: now, reported: false,
          retryOf: null }]]);
        agentGraphLiveObserve({ columns: [], overlays: { mail: [] } }, place("dp-other"), Date.now());
        const first = agentGraphLiveRecentEvents().length;
        // 같은 자리, 다른 시도 — 앞 주체의 watermark 가 이 사건을 삼키면 안 된다.
        agentGraphLiveObserve({ columns: [], overlays: { mail: [] } }, place("dp-other-2"), Date.now());
        const second = agentGraphLiveRecentEvents().length;
        return second > first;
      }));
    ok("a_card_without_run_worker_and_dispatch_reads_as_unverified",
      await page.evaluate(() => {
        const view = document.querySelector("#board-view");
        const node = view.querySelector('.agent-graph-node[data-graph-key="agent:term:305"]');
        const chip = node?.querySelector(".agent-graph-wait-cause");
        return chip?.textContent === t("board.live.unverified", "확인되지 않음");
      }));

    /* ---- ③ 없는 사실은 없다고 말한다 ------------------------------------ */

    ok("waiting_names_its_recorded_cause_not_an_idle_guess", await page.evaluate(() => {
      const view = document.querySelector("#board-view");
      const walled = view.querySelector('.agent-graph-node[data-graph-key="agent:term:304"] .agent-graph-wait');
      const gone = view.querySelector('.agent-graph-node[data-graph-key="agent:worker:w-gone"] .agent-graph-wait');
      const quiet = view.querySelector('.agent-graph-node[data-graph-key="agent:term:307"] .agent-graph-wait');
      return walled?.classList.contains("is-walled") && walled.textContent.trim() !== ""
        && gone?.classList.contains("is-gone") && gone.textContent.includes("전")
        && quiet?.textContent.trim() === "";
    }));

    /* 사건 줄은 **무엇을 기다리는지**까지 말한다. 종류만 「대기」라고 적고
     * 사유를 빼면, 그 줄은 제가 답해야 할 질문에 답하지 않는다.
     *
     * 「대기 사건이 없으면 통과」하는 탈출구를 두지 않는다 — 그런 조건은
     * 아무것도 하지 않은 판에서도 초록이라 아무것도 고정하지 못한다. 실제로
     * 대기를 하나 **만들고** 그 문장을 읽는다. */
    ok("a_wait_event_row_names_its_cause_in_the_desks_own_word",
      await page.evaluate(async () => {
        const now = window.__LIVE_NOW__;
        /* 새 사유를 실제로 기록한다: 이 워커의 판이 방금 사라졌다. */
        const row = window.__LEDGER__.find((one) => one.worker === "w-impl");
        row.hearing = "gone";
        row.pane_missing_since_ms = now + 70_000;
        row.term = null;
        window.__PANES__ = window.__PANES__.filter((one) => one.term !== 302);
        const card = window.__COLUMNS__[0].cards.find((one) => one.pane === "term:302");
        card.pane = "worker:w-impl";
        await paintBoardView(undefined, { force: true });
        await window.__BOARD_SETTLED__();
        const view = document.querySelector("#board-view");
        selectAgentGraphEntity(view, "agent:worker:w-impl");
        const rows = [...view.querySelectorAll(".agent-live-event")].map((one) => one.textContent);
        const gone = DESK_HEALTH.find((one) => one.id === "gone");
        const kind = t("board.live.wait", "대기");
        const waits = rows.filter((said) => said.includes(kind));
        window.__WAIT_ROWS__ = rows;
        /* 대기 줄이 실제로 하나 있고, 그 줄이 사유를 데스크의 낱말로 적는다.
         * 둘 중 하나라도 없으면 빨강이다. */
        const held = waits.length > 0 && waits.some((said) => said.includes(t(gone.key, gone.word)));
        /* 그리고 픽스처를 **되돌린다**. 이 사례는 판 하나를 없애 보는 것이고,
         * 뒤의 사례들은 그 판이 서 있는 것을 전제한다 — 없애 둔 채로 넘기면
         * 뒤에서 나는 빨강은 그 사례의 결함이 아니라 이 사례가 남긴 자국이다. */
        row.hearing = "pending";
        row.pane_missing_since_ms = null;
        row.term = 302;
        window.__PANES__ = [...window.__PANES__,
          { term: 302, agent: "codex", state: "working", at: now - 10_000,
            state_started_at: now - 10_000, resumable: false }];
        card.pane = "term:302";
        await paintBoardView(undefined, { force: true });
        await window.__BOARD_SETTLED__();
        return held;
      }), await page.evaluate(() => JSON.stringify(window.__WAIT_ROWS__ ?? [])));

    ok("worker_claim_is_not_verified_or_merged", await page.evaluate(() => {
      const view = document.querySelector("#board-view");
      const stage = (pane) => {
        const row = window.__LEDGER__.find((one) => one.term === pane);
        return ledgerReviewStage({ reported: row.reported }, row);
      };
      const claimed = ledgerReviewWord({ reported: true, review: null });
      const accepted = ledgerReviewWord({ reported: true, review: { verified: true } });
      return stage(302) === "reported" && stage(303) === "verified"
        && claimed !== accepted && claimed === t("board.awaitingReview", "검증 대기")
        && view !== null;
    }));

    /* 착지한 권위 seam(t-6815)이 실제로 싣는 사실로 — 워커가 스스로 적은
     * 「검증됐다」는 **주장**이고, 코디네이터가 그 시도의 출처를 보고 적은
     * 「검증됨」은 **사실**이다. 두 사건은 실제 그리기 문을 지나 각자 제 낱말로
     * 서고, 주장은 사실의 낱말도, 배정도 되지 않는다. 그리고 그 seam이 내는 모든
     * 낱말이 지도의 단계 하나로 되읽힌다 — seam이 낱말을 늘리면 여기서 빨개진다. */
    const authority = await page.evaluate(async () => {
      const view = document.querySelector("#board-view");
      const held = window.__LEDGER__;
      window.__LEDGER__ = held.map((row) => {
        if (row.worker === "w-impl") {
          return { ...row, reported: true, review: { verified: false, merged: false, deployed: false,
            written: false, claimed_verified: true, claimed_merged: false, claimed_deployed: false,
            author: "worker" } };
        }
        if (row.worker === "w-verify") {
          return { ...row, reported: true, review: { verified: true, merged: true, deployed: false,
            written: true, claimed_verified: false, claimed_merged: false, claimed_deployed: false,
            author: "coordinator", attempt: "dp-verify", source: "0123abc" } };
        }
        return row;
      });
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
      selectAgentGraphEntity(view, "agent:term:301");
      const said = (dispatch) => {
        const event = agentGraphLiveRecentEvents().find((one) => one.evidence?.dispatchId === dispatch
          && (one.kind === "result" || one.kind === "assignment"));
        const row = event && [...view.querySelectorAll(".agent-live-event-main")]
          .find((one) => one.dataset.liveEvent === event.key);
        return event ? { kind: event.kind, stage: event.evidence.stage, at: event.at,
          facts: row?.querySelector(".agent-live-event-facts")?.textContent ?? null } : null;
      };
      const claim = said("dp-impl");
      const fact = said("dp-verify");
      /* seam이 답할 수 있는 모든 낱말이 단계 하나로 되읽히는가. */
      const reviews = [{ deployed: true }, { merged: true }, { verified: true },
        { claimed_deployed: true }, { claimed_merged: true }, { claimed_verified: true }, {}];
      const unread = reviews.filter((review) =>
        ledgerReviewStage({ reported: true }, { reported: true, review }) === "dispatched");
      /* 표의 `flag`가 그 seam이 그 낱말을 고를 때 읽는 칸인가 — 그 칸 하나만 선
       * 행을 seam이 그 줄의 낱말로 읽어야 하고, 지난 결과 사건이 「그 사실이 지금도
       * 서 있는가」를 물을 때(`agentGraphLiveStageHolds`) 그 행에서 참이어야 한다. */
      const misnamed = AGENT_GRAPH_LIVE_STAGES.filter((one) => {
        const row = one.flag === "reported"
          ? { reported: true, review: null }
          : { reported: false, review: { [one.flag]: true } };
        return ledgerReviewWord(row) !== t(one.key, one.word)
          || !agentGraphLiveStageHolds(one.stage, null, row);
      }).map((one) => one.stage);
      window.__LEDGER__ = held;
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
      return { claim, fact, unread, misnamed,
        words: { claimed: t("board.claimedVerified", "검증됐다 함"), verified: t("board.verified", "검증됨"),
          merged: t("board.merged", "병합됨") } };
    });
    ok("a_workers_claim_and_a_coordinators_fact_stand_as_different_results",
      authority.claim?.kind === "result" && authority.claim.stage === "claimed-verified"
      && authority.claim.at === 0 && authority.claim.facts?.includes(authority.words.claimed) === true
      && !authority.claim.facts.includes(authority.words.verified)
      && authority.fact?.kind === "result" && authority.fact.stage === "merged"
      && authority.fact.facts?.includes(authority.words.merged) === true,
      JSON.stringify(authority));
    ok("every_word_the_review_seam_answers_reads_back_as_a_stage",
      authority.unread.length === 0, JSON.stringify(authority.unread));
    ok("each_stage_names_the_ledger_field_the_review_seam_reads_for_its_word",
      authority.misnamed.length === 0, JSON.stringify(authority.misnamed));

    ok("a_recorded_dispatch_and_message_point_to_their_exact_evidence",
      await page.evaluate(() => {
        const view = document.querySelector("#board-view");
        selectAgentGraphRelation(view, "overlay:mail:agent:term:301>agent:term:302");
        const said = view.querySelector(".agent-relation-evidence")?.textContent ?? "";
        return said.includes("m-dispatch-1") && said.includes("run-1")
          && said.includes("worker:w-impl");
      }));

    ok("delivery_and_reply_stay_unknown_instead_of_being_guessed",
      await page.evaluate(() => {
        const view = document.querySelector("#board-view");
        const notes = [...view.querySelectorAll(".agent-relation-evidence .agent-relation-note")]
          .map((node) => node.textContent).join(" ");
        const labels = [...view.querySelectorAll(".agent-graph-overlay-label")]
          .map((node) => node.textContent);
        window.__DELIV__ = { notes: notes.slice(0, 300), labels };
        return notes.includes(agentGraphLiveUnknownWord("delivery"))
          && notes.includes(agentGraphLiveUnknownWord("reply"))
          // 미확인이 있는 선은 그 수를, 없는 선은 「미제공」을 — 「확인됨」은 없다.
          && labels.some((word) => word.endsWith(t("board.live.deliveryNotSaid", "배달 상태 미제공")))
          && labels.some((word) => word.endsWith(t("board.live.pending", "미확인 {{count}}", { count: 1 })))
          && labels.every((word) => !word.includes(t("board.verified", "검증됨")));
      }), await page.evaluate(() => JSON.stringify(window.__DELIV__ ?? null)));

    ok("the_number_of_endpoints_outside_this_view_is_reported_as_not_provided",
      await page.evaluate(() => {
        const view = document.querySelector("#board-view");
        selectAgentGraphEntity(view, "agent:term:301");
        const notes = [...view.querySelectorAll(".agent-live-events-note")]
          .map((node) => node.textContent).join(" ");
        return notes.includes(agentGraphLiveUnknownWord("outside"));
      }));

    ok("a_paneless_workers_summoner_is_named_as_not_provided_not_as_absent",
      await page.evaluate(() => {
        const view = document.querySelector("#board-view");
        selectAgentGraphEntity(view, "agent:worker:w-gone");
        view.querySelector('[data-agent-inspector-tab="details"]').click();
        const said = view.querySelector(".agent-inspector-pane.is-details")?.textContent ?? "";
        const back = view.querySelector('[data-agent-inspector-tab="relations"]');
        back.click();
        return said.includes(agentGraphLiveUnknownWord("summoner"));
      }));

    /* ---- ③′ 사건이 가리키는 근거 -----------------------------------------
     *
     * 사건 줄을 누르면 **그 사건의** 근거로 간다. 같은 두 자리 사이에 더 새
     * 메시지가 왔거나, 같은 판에 다른 시도가 앉았거나, 끝점이 사라졌으면 지금의
     * 관계·판으로 대신 가지 않는다 — 그곳이 보여 주는 것은 그 뒤의 사건이다.
     * 누른 줄이 제 식별자와 까닭을 편다. 음성마다 양성 하나: 가장 새 사건은
     * 이미 있는 문으로 그대로 간다. */
    await page.evaluate(() => {
      /* 누르는 손 하나. 목록은 고른 관계가 없을 때 머리에 서므로 코디네이터를
       * 골라 둔 뒤, 조건에 맞는 줄을 **실제로** 누른다. */
      window.__PRESS_EVENT__ = (match) => {
        const view = document.querySelector("#board-view");
        selectAgentGraphEntity(view, "agent:term:301");
        const events = agentGraphLiveRecentEvents();
        const target = events.find(match);
        const button = target && [...view.querySelectorAll(".agent-live-event-main")]
          .find((one) => one.dataset.liveEvent === target.key);
        if (!button) return { found: false, listed: events.map((event) => event.key) };
        button.click();
        const row = [...view.querySelectorAll(".agent-live-event-main")]
          .find((one) => one.dataset.liveEvent === target.key)?.closest(".agent-live-event");
        return {
          found: true,
          selectedKey: agentGraphSelectedKey,
          selectedEdge: agentGraphSelectedEdgeKey,
          evidence: view.querySelector(".agent-relation-evidence")?.textContent ?? "",
          detail: row?.querySelector(".agent-live-event-detail")?.textContent ?? "",
          pressed: row?.querySelector(".agent-live-event-main")?.getAttribute("aria-pressed") ?? null,
          when: row?.querySelector(".agent-live-event-when")?.textContent ?? "",
          at: target.at,
        };
      };
      window.__LIVE_WORDS__ = () => ({
        notLatest: t("board.live.eventNotLatest",
          "이 사건의 근거는 지금 스냅샷의 최신 기록이 아닙니다. 지금 기록으로 대신 열지 않고, 아래 식별자로 원장에서 확인합니다."),
        outside: t("board.live.eventOutside",
          "이 사건의 끝점은 지금 스냅샷에 없습니다. 아래 식별자로 원장에서 확인합니다."),
        unknown: t("board.live.occurredUnknown", "발생 시각 미제공"),
      });
    });

    /* 같은 두 자리 사이에 m1 → m2. m1 을 누르면 m2 의 근거가 열리면 안 된다. */
    const pastMessage = await page.evaluate(async () => {
      const now = window.__LIVE_NOW__;
      const mail = (id, at) => ({ mail: [
        { from: "term:312", to: "term:301", count: 1, unread: 0, at, verb: "mail",
          last_message: { id, run: "run-2", from: "worker:w-other", to: "run:run-1",
            kind: "status", created_ms: at } },
      ] });
      for (const [id, at] of [["m-past-1", now + 61_000], ["m-past-2", now + 62_000]]) {
        window.__OVERLAYS__ = window.__LIVE_OVERLAYS__(now, mail(id, at));
        await paintBoardView(undefined, { force: true });
        await window.__BOARD_SETTLED__();
      }
      return {
        old: window.__PRESS_EVENT__((event) => event.evidence?.messageId === "m-past-1"),
        latest: window.__PRESS_EVENT__((event) => event.evidence?.messageId === "m-past-2"),
        words: window.__LIVE_WORDS__(),
      };
    });
    ok("a_past_message_event_opens_its_own_evidence_not_the_newer_message",
      pastMessage.old.found && pastMessage.old.selectedEdge === null
      && !pastMessage.old.evidence.includes("m-past-2")
      && pastMessage.old.detail.includes("m-past-1") && !pastMessage.old.detail.includes("m-past-2")
      && pastMessage.old.detail.includes(pastMessage.words.notLatest)
      && pastMessage.old.pressed === "true",
      JSON.stringify(pastMessage));
    ok("the_latest_message_event_opens_the_relation_that_carries_it",
      pastMessage.latest.found
      && pastMessage.latest.selectedEdge === "overlay:mail:agent:term:312>agent:term:301"
      && pastMessage.latest.evidence.includes("m-past-2") && pastMessage.latest.detail === "",
      JSON.stringify(pastMessage));

    /* 같은 판(term:312)에 W1/D1 → W2/D2. D1 의 배정을 누르면 W2 의 실행 상세로
     * 가면 안 된다 — 같은 탐색키일 뿐 다른 사람이다. */
    const reseated = await page.evaluate(async () => {
      const now = window.__LIVE_NOW__;
      const at = window.__LEDGER__.findIndex((one) => one.term === 312);
      const held = window.__LEDGER__[at];
      const seat = async (worker, dispatch, started) => {
        window.__LEDGER__ = window.__LEDGER__.map((row, index) => index === at
          ? { ...held, worker, dispatch_id: dispatch, dispatch_started_ms: started,
            task_id: `t-${dispatch}`, task: `자리 ${dispatch}` }
          : row);
        await paintBoardView(undefined, { force: true });
        await window.__BOARD_SETTLED__();
      };
      await seat("w-seat-1", "dp-seat-1", now + 63_000);
      await seat("w-seat-2", "dp-seat-2", now + 64_000);
      const old = window.__PRESS_EVENT__((event) => event.evidence?.dispatchId === "dp-seat-1");
      const latest = window.__PRESS_EVENT__((event) => event.evidence?.dispatchId === "dp-seat-2");
      window.__LEDGER__ = window.__LEDGER__.map((row, index) => index === at ? held : row);
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
      return { old, latest, words: window.__LIVE_WORDS__() };
    });
    ok("an_old_attempts_event_does_not_open_the_new_workers_details",
      reseated.old.found && reseated.old.selectedKey === "agent:term:301"
      && reseated.old.detail.includes("dp-seat-1") && reseated.old.detail.includes("w-seat-1")
      && !reseated.old.detail.includes("dp-seat-2")
      && reseated.old.detail.includes(reseated.words.notLatest),
      JSON.stringify(reseated));
    ok("the_current_attempts_event_opens_its_seat",
      reseated.latest.found && reseated.latest.selectedKey === "agent:term:312"
      && reseated.latest.detail === "",
      JSON.stringify(reseated));

    /* 끝점이 스냅샷을 떠난 사건은 **범위 밖**이라고 말한다 — 조용히 아무 데도
     * 가지 않는 것도, 비슷한 다른 판으로 가는 것도 아니다. */
    const departed = await page.evaluate(async () => {
      const now = window.__LIVE_NOW__;
      const pane = { term: 322, agent: "codex", state: "working", at: now, state_started_at: now,
        resumable: false };
      window.__PANES__ = [...window.__PANES__, pane];
      window.__LEDGER__ = [...window.__LEDGER__, { ...window.__LEDGER__[0], worker: "w-leaving",
        task: "곧 떠날 일", task_id: "t-leaving", dispatch_id: "dp-leaving",
        dispatch_started_ms: now + 65_000, term: 322, reported: false, review: null }];
      window.__COLUMNS__[0].cards = [...window.__COLUMNS__[0].cards, {
        ...window.__COLUMNS__[0].cards[1], pane: "term:322", heading: "곧 떠날 일",
        task: "곧 떠날 일", parent: "" }];
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
      const listed = agentGraphLiveRecentEvents().some((event) => event.evidence?.dispatchId === "dp-leaving");
      window.__PANES__ = window.__PANES__.filter((one) => one.term !== 322);
      window.__LEDGER__ = window.__LEDGER__.filter((one) => one.worker !== "w-leaving");
      window.__COLUMNS__[0].cards = window.__COLUMNS__[0].cards.filter((one) => one.pane !== "term:322");
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
      const pressed = window.__PRESS_EVENT__((event) => event.evidence?.dispatchId === "dp-leaving");
      return { listed, pressed, words: window.__LIVE_WORDS__() };
    });
    ok("an_event_whose_endpoint_left_the_snapshot_says_it_is_outside",
      departed.listed && departed.pressed.found && departed.pressed.selectedKey === "agent:term:301"
      && departed.pressed.detail.includes(departed.words.outside)
      && departed.pressed.detail.includes("dp-leaving"),
      JSON.stringify(departed));

    /* 결과와 의존 전이의 **발생 시각**은 이 판에 없다. 배차가 시작된 때나 후속
     * 과업이 생긴 때를 그 사건의 나이로 빌리면, 방금 온 보고가 「5분 전」이
     * 된다. 모르는 것은 모른다고 적고, 이 창이 본 때를 따로 적는다. 발생
     * 시각이 기록된 사건(메시지·배정)은 그 시각을 쓴다. */
    const timed = await page.evaluate(async () => {
      const now = window.__LIVE_NOW__;
      const view = document.querySelector("#board-view");
      /* 벽 앞의 워커가 방금 보고했다 — 배차는 5분 전에 시작됐다. */
      const at = window.__LEDGER__.findIndex((one) => one.worker === "w-wall");
      const held = window.__LEDGER__[at];
      window.__LEDGER__ = window.__LEDGER__.map((row, index) => index === at
        ? { ...held, reported: true } : row);
      /* 선행 조건 t-impl 이 끝나 t-verify 가 실행 대기로 넘어갔다 — 과업은 3분 전에 생겼다. */
      const overlays = window.__LIVE_OVERLAYS__(now);
      overlays.task_dependencies = overlays.task_dependencies.map((fact) => fact.task === "t-verify"
        ? { ...fact, task_state: "ready", dependency_state: "done" } : fact);
      window.__OVERLAYS__ = overlays;
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
      selectAgentGraphEntity(view, "agent:term:301");
      const when = (match) => {
        const event = agentGraphLiveRecentEvents().find(match);
        const row = event && [...view.querySelectorAll(".agent-live-event-main")]
          .find((one) => one.dataset.liveEvent === event.key);
        return event ? { at: event.at, said: row?.querySelector(".agent-live-event-when")?.textContent ?? null }
          : null;
      };
      const clock = Date.now();
      const ago = (stamp) => t("board.desk.mailAge", "{{time}} 전", { time: agoWord(stamp, clock) });
      const result = when((event) => event.kind === "result" && event.evidence?.dispatchId === "dp-retry-2");
      const dependency = when((event) => event.kind === "dependency" && event.evidence?.taskId === "t-verify");
      const assignment = when((event) => event.kind === "assignment" && event.evidence?.dispatchId === "dp-seat-2");
      const message = when((event) => event.kind === "message" && event.evidence?.messageId === "m-past-2");
      window.__LEDGER__ = window.__LEDGER__.map((row, index) => index === at ? held : row);
      window.__OVERLAYS__ = window.__LIVE_OVERLAYS__(now);
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
      return { result, dependency, assignment, message,
        borrowed: { result: ago(held.dispatch_started_ms), dependency: ago(now - 180_000) },
        recorded: { assignment: assignment ? ago(assignment.at) : null, message: message ? ago(message.at) : null },
        unknown: window.__LIVE_WORDS__().unknown };
    });
    ok("a_result_and_a_dependency_transition_do_not_borrow_a_start_time_as_their_age",
      timed.result !== null && timed.result.at === 0 && timed.result.said !== timed.borrowed.result
      && timed.result.said?.includes(timed.unknown) === true
      && timed.dependency !== null && timed.dependency.at === 0
      && timed.dependency.said !== timed.borrowed.dependency
      && timed.dependency.said?.includes(timed.unknown) === true,
      JSON.stringify(timed));
    ok("a_message_and_an_assignment_keep_their_recorded_time",
      timed.message !== null && timed.message.said === timed.recorded.message
      && timed.assignment !== null && timed.assignment.at > 0
      && timed.assignment.said === timed.recorded.assignment,
      JSON.stringify(timed));

    /* **같은 시도**(같은 run/worker/dispatch, 같은 판)라도 사건의 근거는 바뀐다. 자리
     * 셋이 같다고 지금의 판으로 가면, 누른 줄은 옛 사유·옛 사실을 말하는데 열린
     * 판은 그 뒤의 것을 말한다 — 누른 사건의 근거가 지금 스냅샷에 **같은 사실로**
     * 남아 있을 때만 지금의 문으로 간다.
     *
     * 기다림: 한도 벽@T1 → 잠듦@T2. 옛 벽 사건은 제 사유와 그 사유가 적힌 때를 편다. */
    const rewaited = await page.evaluate(async () => {
      const now = window.__LIVE_NOW__;
      const at = window.__LEDGER__.findIndex((one) => one.worker === "w-wall");
      const held = window.__LEDGER__[at];
      const T1 = now + 66_000;
      const T2 = now + 67_000;
      const write = async (over) => {
        window.__LEDGER__ = window.__LEDGER__.map((row, index) => index === at ? { ...held, ...over } : row);
        await paintBoardView(undefined, { force: true });
        await window.__BOARD_SETTLED__();
      };
      await write({ wall: { ...held.wall, wall: "m-wall-t1", observed_at_ms: T1 } });
      await write({ wall: null, ledger: "sleeping", quiet_at: T2 });
      const wait = (since) => (event) => event.kind === "wait" && event.evidence?.since === since;
      const old = window.__PRESS_EVENT__(wait(T1));
      const latest = window.__PRESS_EVENT__(wait(T2));
      window.__LEDGER__ = window.__LEDGER__.map((row, index) => index === at ? held : row);
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
      const word = (id) => { const health = DESK_HEALTH.find((one) => one.id === id); return t(health.key, health.word); };
      return { old, latest, walled: word("walled"), asleep: word("asleep"),
        words: window.__LIVE_WORDS__() };
    });
    ok("a_past_wait_of_the_same_attempt_opens_its_own_cause_not_the_current_one",
      rewaited.old.found && rewaited.old.selectedKey === "agent:term:301"
      && rewaited.old.detail.includes(rewaited.walled) && !rewaited.old.detail.includes(rewaited.asleep)
      && rewaited.old.detail.includes(rewaited.words.notLatest) && rewaited.old.pressed === "true",
      JSON.stringify(rewaited));
    ok("the_current_wait_of_that_attempt_opens_its_seat",
      rewaited.latest.found && rewaited.latest.selectedKey === "agent:term:304"
      && rewaited.latest.detail === "",
      JSON.stringify(rewaited));

    /* 결과: 같은 시도에서 코디네이터의 「검증됨」이 **철회되고** 워커의 주장만 남는다.
     * 옛 검증 사건은 지금의 판으로 가지 않고 그때의 사실을 편다. 옛 사실이 지금도
     * 참인 판(검증된 뒤 병합됨 — 검증은 그대로 남는다)은 지금의 판으로 가도 누른
     * 줄과 열린 판이 서로 다른 것을 말하지 않는다. */
    const retracted = await page.evaluate(async () => {
      const now = window.__LIVE_NOW__;
      const base = window.__LEDGER__[0];
      const verified = { verified: true, merged: false, deployed: false, written: true,
        claimed_verified: false, claimed_merged: false, claimed_deployed: false,
        author: "coordinator", source: "0123abc" };
      const seat = (term, worker, dispatch) => ({
        pane: { term, agent: "codex", state: "working", at: now, state_started_at: now, resumable: false },
        row: { ...base, worker, task: `시도 ${dispatch}`, task_id: `t-${dispatch}`, dispatch_id: dispatch,
          dispatch_started_ms: now + 68_000, term, reported: true, review: { ...verified, attempt: dispatch } },
        card: { ...window.__COLUMNS__[0].cards[1], pane: `term:${term}`, heading: `시도 ${dispatch}`,
          task: `시도 ${dispatch}`, parent: "" },
      });
      const seats = [seat(323, "w-retract", "dp-retract"), seat(324, "w-land", "dp-land")];
      const held = { panes: window.__PANES__, ledger: window.__LEDGER__,
        columns: structuredClone(window.__COLUMNS__) };
      window.__PANES__ = [...held.panes, ...seats.map((one) => one.pane)];
      window.__LEDGER__ = [...held.ledger, ...seats.map((one) => one.row)];
      window.__COLUMNS__[0].cards = [...window.__COLUMNS__[0].cards, ...seats.map((one) => one.card)];
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
      window.__LEDGER__ = window.__LEDGER__.map((row) => row.worker === "w-retract"
        ? { ...row, review: { ...row.review, verified: false, claimed_verified: true, author: "worker" } }
        : row.worker === "w-land" ? { ...row, review: { ...row.review, merged: true } } : row);
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
      const result = (dispatch, stage) => (event) => event.kind === "result"
        && event.evidence?.dispatchId === dispatch && event.evidence.stage === stage;
      const old = window.__PRESS_EVENT__(result("dp-retract", "verified"));
      const latest = window.__PRESS_EVENT__(result("dp-retract", "claimed-verified"));
      const landed = window.__PRESS_EVENT__(result("dp-land", "verified"));
      window.__PANES__ = held.panes;
      window.__LEDGER__ = held.ledger;
      window.__COLUMNS__ = held.columns;
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
      return { old, latest, landed, words: { ...window.__LIVE_WORDS__(),
        verified: t("board.verified", "검증됨"), claimed: t("board.claimedVerified", "검증됐다 함") } };
    });
    ok("a_retracted_result_of_the_same_attempt_opens_the_fact_it_recorded",
      retracted.old.found && retracted.old.selectedKey === "agent:term:301"
      && retracted.old.detail.includes(retracted.words.verified)
      && !retracted.old.detail.includes(retracted.words.claimed)
      && retracted.old.detail.includes("dp-retract")
      && retracted.old.detail.includes(retracted.words.notLatest),
      JSON.stringify(retracted));
    ok("the_current_result_and_a_fact_the_present_still_holds_open_their_seat",
      retracted.latest.found && retracted.latest.selectedKey === "agent:term:323"
      && retracted.latest.detail === ""
      && retracted.landed.found && retracted.landed.selectedKey === "agent:term:324"
      && retracted.landed.detail === "",
      JSON.stringify(retracted));

    /* **검토만** 바뀐 판. 코디네이터가 w-verify 의 배포를 적었다가 거둔다 — 카드의
     * 어느 칸도 움직이지 않고(카드는 `reported`만 싣는다) 그 사실은 원장 행의
     * `review`에만 있다. 제품의 박자(강제 없음)로 지도가 그 결과를 보고, 거둔 뒤에는
     * 모델의 원장 한 벌도 그 행을 따라가 옛 배포 사건이 지금이 아니라고 말해야 한다.
     * 위의 사례들은 강제로 그려 이 길을 지나지 않는다. 서명이 분 경계에서 흔들리지
     * 않도록 시계를 멈춘다. */
    const reviewOnly = await page.evaluate(async () => {
      const at = window.__LEDGER__.findIndex((one) => one.worker === "w-verify");
      const held = window.__LEDGER__[at];
      const beat = async () => {
        scheduleAgentPaint(["board"]);
        await window.__BOARD_SETTLED__();
      };
      const write = async (review) => {
        window.__LEDGER__ = window.__LEDGER__.map((row, index) => index === at ? { ...held, review } : row);
        await beat();
      };
      const base = { verified: true, merged: true, deployed: false, written: true,
        claimed_verified: false, claimed_merged: false, claimed_deployed: false,
        author: "coordinator", attempt: "dp-verify", source: "0123abc" };
      const trueNow = Date.now;
      const pinned = trueNow.call(Date);
      Date.now = () => pinned;
      try {
        await paintBoardView(undefined, { force: true });
        await window.__BOARD_SETTLED__();
        await write({ ...base, deployed: true });
        const listed = agentGraphLiveRecentEvents().find((event) => event.kind === "result"
          && event.evidence?.dispatchId === "dp-verify" && event.evidence.stage === "deployed") ?? null;
        await write(base);
        const ledger = agentGraphFullModel(document.querySelector("#board-view"))
          ?.source.ledger?.get("term:303")?.review?.deployed ?? null;
        const pressed = listed ? window.__PRESS_EVENT__((event) => event.key === listed.key) : null;
        return { listed: listed?.key ?? null, ledger, pressed,
          words: { ...window.__LIVE_WORDS__(), deployed: t("board.deployed", "배포됨") } };
      } finally {
        Date.now = trueNow;
        window.__LEDGER__ = window.__LEDGER__.map((row, index) => index === at ? held : row);
        await paintBoardView(undefined, { force: true });
        await window.__BOARD_SETTLED__();
      }
    });
    ok("a_review_only_change_reaches_the_map_on_the_products_beat",
      reviewOnly.listed !== null && reviewOnly.ledger === false, JSON.stringify(reviewOnly));
    ok("a_withdrawn_review_fact_opens_its_own_evidence_on_the_products_beat",
      reviewOnly.pressed?.found === true && reviewOnly.pressed.selectedKey === "agent:term:301"
      && reviewOnly.pressed.detail.includes(reviewOnly.words.deployed)
      && reviewOnly.pressed.detail.includes(reviewOnly.words.notLatest),
      JSON.stringify(reviewOnly));

    /* ---- ④ 보는 일은 아무것도 소비하지 않는다 --------------------------- */

    const doors = await page.evaluate(async () => {
      const before = { ...window.__COUNTS__ };
      const view = document.querySelector("#board-view");
      selectAgentGraphEntity(view, "agent:term:302");
      selectAgentGraphRelation(view, "overlay:mail:agent:term:302>agent:term:301");
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
      const after = { ...window.__COUNTS__ };
      const grew = Object.keys(after).filter((name) => (after[name] ?? 0) > (before[name] ?? 0));
      return grew;
    });
    ok("viewing_never_acknowledges_or_replies_to_mail",
      !doors.some((name) => /ack|reply|send|answer_ask|approve/.test(name)), JSON.stringify(doors));
    ok("new_visualization_makes_zero_model_calls",
      !doors.some((name) => /jev|model|infer|complete/i.test(name)), JSON.stringify(doors));

    ok("selecting_an_edge_preserves_focus_and_opens_its_real_receipt",
      await page.evaluate(() => {
        const view = document.querySelector("#board-view");
        selectAgentGraphEntity(view, "agent:term:302", { focus: true });
        const focused = document.activeElement;
        selectAgentGraphRelation(view, "overlay:mail:agent:term:302>agent:term:301");
        const evidence = view.querySelector(".agent-relation-evidence")?.textContent ?? "";
        return document.activeElement === focused && evidence.includes("m-report-1");
      }));

    /* ---- 조용한 판 ------------------------------------------------------ */

    const quiet = await page.evaluate(async () => {
      const view = document.querySelector("#board-view");
      await window.__BOARD_SETTLED__();
      /* 맥박이 다 꺼질 때까지 — 실제 시계로. 맥박의 시계는 제가 켜진 때와
       * 지금을 견주므로, 시계를 멈춘 채로는 영영 꺼지지 않는다. */
      await window.__UNTIL__(() => agentGraphLiveHandles().pulses === 0, "pulses expire", 5_000);
      await window.__BOARD_SETTLED__();
      /* **같은 시계**로 잰다 — 인수 조건이 그렇게 적혀 있고, 그래야 이 수가
       * 「아무것도 안 바뀌었는데 그렸다」를 뜻한다. 비교하는 두 그림을 **둘 다**
       * 멈춘 시계로 그린다: 기준 그림을 실제 시계로 그리면 카드와 기다림의 나이
       * 낱말이 두 그림 사이에서 분 경계를 넘는 판이 있고, 그때 올라오는 것은
       * 렌더러의 결함이 아니라 **옳게 움직인 그림**이다.
       *
       * 이것이 시간을 멈춰 얻은 0을 실시간의 0으로 파는 일이 되지 않는 것은,
       * 시계가 실제로 흐를 때 그 낱말이 바뀌는지를 아래 사례가 따로 — 원장은
       * 그대로 두고 제품의 박자로 — 확인하기 때문이다. 멈춤은 이 측정 안에서만이다. */
      const trueNow = Date.now;
      const pinned = trueNow.call(Date);
      Date.now = () => pinned;
      let taken;
      let firstRow = 0;
      let mutations = 0;
      /* 재는 것은 **그림**이다 — 판의 머리와 툴바는 이 표면보다 오래된 손들이
       * 지나는 자리이고, 여기서 고정하려는 계약은 「아무 일도 없는 판에서
       * 렌더러가 DOM을 건드리지 않는다」이다. */
      const watch = new MutationObserver((records) => { mutations += records.length; });
      try {
        await paintBoardView(undefined, { force: true });
        await window.__BOARD_SETTLED__();
        firstRow = view.querySelector(".agent-graph-node")?.getBoundingClientRect().top ?? 0;
        watch.observe(view.querySelector(".agent-graph-layout"),
          { subtree: true, childList: true, attributes: true, characterData: true });
        const before = [agentGraphNodeCreations, agentGraphLayoutRuns, agentGraphEdgeMeasureRuns];
        await paintBoardView(undefined, { force: false });
        await window.__BOARD_SETTLED__();
        await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
        taken = { before, after: [agentGraphNodeCreations, agentGraphLayoutRuns, agentGraphEdgeMeasureRuns] };
      } finally {
        Date.now = trueNow;
      }
      watch.disconnect();
      return { mutations, ...taken, pinnedClock: true,
        firstRow, nowRow: view.querySelector(".agent-graph-node")?.getBoundingClientRect().top ?? 0,
        handles: agentGraphLiveHandles() };
    });
    ok("quiet_poll_has_zero_dom_mutations", quiet.mutations === 0, JSON.stringify(quiet));
    ok("the_quiet_poll_reused_dom_layout_and_measured_edges",
      quiet.before.join() === quiet.after.join(), JSON.stringify(quiet));
    ok("settled_poll_keeps_first_row_at_same_position",
      quiet.firstRow === quiet.nowRow, JSON.stringify(quiet));
    ok("a_finished_pulse_returns_to_idle",
      quiet.handles.timers === 0 && quiet.handles.pulses === 0, JSON.stringify(quiet.handles));
    /* 이 표면 위에서 **영원히 도는 것**이 하나도 없다. 카드의 빛은 그 위의
     * 한 겹에 얹히므로 의사 요소까지 함께 센다. */
    ok("no_perpetual_orbit", await page.evaluate(() => {
      const view = document.querySelector("#board-view");
      const spun = [];
      for (const node of view.querySelectorAll(
        ".agent-graph-edge, .agent-graph-node, .agent-graph-mail-pulse")) {
        for (const part of [null, "::before", "::after"]) {
          const dress = getComputedStyle(node, part);
          if (dress.animationName !== "none" && dress.animationIterationCount.includes("infinite")) {
            spun.push(`${node.getAttribute("class")}${part ?? ""}:${dress.animationName}`);
          }
        }
      }
      window.__LIVE_SPUN__ = spun;
      return spun.length === 0;
    }), await page.evaluate(() => JSON.stringify(window.__LIVE_SPUN__ ?? [])));

    /* 시계는 **멈추지 않는다**: 나이 낱말은 박자마다 바뀌어야 한다. 시간을
     * 멈추고 얻은 0을 실시간의 0으로 파는 일을 막는 한 줄이다.
     *
     * 원장의 행은 **한 칸도 바꾸지 않는다** — 바꾸면 이 사례는 「데이터가 바뀌면
     * 다시 그린다」를 재게 된다. 흐르는 것은 시계뿐이고, 다시 그리게 하는 것은
     * 제품의 시계가 박자마다 하는 그 일(`agentGraphClockBeat`의 콜백이 부르는
     * `scheduleAgentPaint(["board"])`, 강제 없음)이다. 카드의 나이 낱말이 같은
     * 분에 함께 바뀌면 판이 다시 그려진 까닭을 가릴 수 없으므로, 카드와 다른
     * 시각은 하루 전으로 두어 한 분이 흘러도 그대로이게 한다. */
    const ticked = await page.evaluate(async () => {
      const view = document.querySelector("#board-view");
      const read = () => view.querySelector(
        '.agent-graph-node[data-graph-key="agent:worker:w-gone"] .agent-graph-wait-age')?.textContent ?? null;
      const trueNow = Date.now;
      const base = trueNow.call(Date);
      const dayAgo = base - 26 * 3_600_000;
      const held = { columns: structuredClone(window.__COLUMNS__), ledger: structuredClone(window.__LEDGER__) };
      for (const column of window.__COLUMNS__) {
        for (const card of column.cards) Object.assign(card, { at: dayAgo, changed_at: dayAgo });
      }
      window.__LEDGER__ = window.__LEDGER__.map((row) => ({ ...row, at: dayAgo, hearing_at: dayAgo,
        ...(row.worker === "w-gone" ? { pane_missing_since_ms: base - 4 * 60_000 - 5_000 } : {}) }));
      const ledger = JSON.stringify(window.__LEDGER__);
      Date.now = () => base;
      try {
        await paintBoardView(undefined, { force: true });
        await window.__BOARD_SETTLED__();
        const before = read();
        Date.now = () => base + 60_000;
        scheduleAgentPaint(["board"]);
        await window.__BOARD_SETTLED__();
        return { before, after: read(), untouched: JSON.stringify(window.__LEDGER__) === ledger,
          expected: t("board.desk.mailAge", "{{time}} 전", { time: agoWord(base - 4 * 60_000 - 5_000, base + 60_000) }) };
      } finally {
        Date.now = trueNow;
        window.__COLUMNS__ = held.columns;
        window.__LEDGER__ = held.ledger;
        await paintBoardView(undefined, { force: true });
        await window.__BOARD_SETTLED__();
      }
    });
    ok("the_elapsed_age_clock_ticks_on_the_products_beat_with_the_ledger_unchanged",
      typeof ticked.before === "string" && ticked.before !== "" && ticked.untouched
      && ticked.after !== ticked.before && ticked.after === ticked.expected,
      JSON.stringify(ticked));

    /* ---- 숨김과 움직임 줄이기 -------------------------------------------- */

    const hidden = await page.evaluate(async () => {
      Object.defineProperty(document, "hidden", { configurable: true, get: () => true });
      document.dispatchEvent(new Event("visibilitychange"));
      const now = window.__LIVE_NOW__;
      window.__OVERLAYS__ = window.__LIVE_OVERLAYS__(now, { mail: [
        { from: "term:303", to: "term:302", count: 1, unread: 1, at: now + 60_000, verb: "mail",
          last_message: { id: "m-while-hidden", run: "run-1", from: "worker:w-verify",
            to: "worker:w-impl", kind: "status", created_ms: now + 60_000 } },
      ] });
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
      const whileHidden = { beats: document.querySelectorAll("#board-view [data-live-beat]").length,
        handles: agentGraphLiveHandles() };
      Object.defineProperty(document, "hidden", { configurable: true, get: () => false });
      document.dispatchEvent(new Event("visibilitychange"));
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
      return { whileHidden, backBeats: document.querySelectorAll("#board-view [data-live-beat]").length };
    });
    ok("hidden_view_does_not_run_animations",
      hidden.whileHidden.beats === 0 && hidden.whileHidden.handles.timers === 0,
      JSON.stringify(hidden));
    ok("returning_from_hidden_does_not_replay_the_backlog",
      hidden.backBeats === 0, JSON.stringify(hidden));

    await page.emulateMedia({ reducedMotion: "reduce" });
    const reduced = await page.evaluate(async () => {
      const now = window.__LIVE_NOW__;
      window.__OVERLAYS__ = window.__LIVE_OVERLAYS__(now, { mail: [
        { from: "term:302", to: "term:303", count: 1, unread: 1, at: now + 120_000, verb: "mail",
          last_message: { id: "m-reduced", run: "run-1", from: "worker:w-impl",
            to: "worker:w-verify", kind: "status", created_ms: now + 120_000 } },
      ] });
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
      /* 카드의 빛은 그 위의 한 겹(`::after`)에 얹히므로 거기서 읽는다 —
       * 선은 제 위에서 직접. 둘 다 이름이 `none`이어야 한다. */
      const marked = [...document.querySelectorAll("#board-view [data-live-beat]")];
      return { marked: marked.length,
        animations: marked.map((node) => getComputedStyle(node,
          node.classList.contains("agent-graph-node") ? "::after" : null).animationName) };
    });
    ok("reduced_motion_does_not_run_animations_and_still_shows_what_happened",
      reduced.marked > 0 && reduced.animations.every((name) => name === "none"),
      JSON.stringify(reduced));
    await page.emulateMedia({ reducedMotion: null });

    /* ---- 다섯 언어와 판의 폭 -------------------------------------------- */

    ok("new_ja_copy_is_translated", await page.evaluate(() => {
      const korean = t("board.live.events", "최근 사건");
      setLocale("ja");
      const japanese = t("board.live.events", "최근 사건");
      const scope = t("board.live.eventsScope", "");
      setLocale("ko");
      return japanese !== korean && japanese.length > 0 && scope.length > 0;
    }));

    /* 상한은 **한 번에 화면에서 뛰는 수**다. 판 하나의 들어오는 수만 자르면
     * 맥박이 살아 있는 동안 판이 여러 번 와서 눈앞의 수는 상한을 훌쩍 넘는다.
     *
     * 이 사례가 맨 뒤에 서는 이유: 여기서 먹이는 시각이 한참 앞선 미래라
     * 여러 관계의 watermark 를 그리로 끌어올린다. 앞에 두면 뒤의 사례들이
     * 보내는 「새 사건」이 그보다 옛것이 되어 조용히 삼켜지고, 그때 나는
     * 빨강은 그 사례의 결함이 아니라 이 사례가 남긴 자국이다. */
    ok("the_burst_cap_counts_what_is_lit_at_once_not_what_arrived_in_one_snapshot",
      await page.evaluate(async () => {
        const view = document.querySelector("#board-view");
        const token = agentGraphTuning(view).liveBurst;
        const now = window.__LIVE_NOW__;
        const pairs = [["term:301", "term:302"], ["term:302", "term:303"],
          ["term:303", "term:301"], ["term:304", "term:301"],
          ["term:306", "term:301"], ["term:307", "term:306"],
          ["term:311", "term:312"], ["term:312", "term:311"]];
        /* 판을 잇달아 넷 먹인다 — 맥박이 다 꺼지기 전에. */
        for (let round = 0; round < 4; round += 1) {
          agentGraphLiveObserve({ columns: [], overlays: { mail: pairs.map(([from, to], at) => ({
            from, to, count: 1, unread: 0, at: now + 800_000 + round * 100 + at, verb: "mail",
            last_message: { id: `m-cap-${round}-${at}`, run: "run-1", from: `worker:w-${at}`,
              to: `worker:w-${at + 1}`, kind: "status",
              created_ms: now + 800_000 + round * 100 + at },
          })) } }, new Map(), Date.now());
        }
        const lit = agentGraphLiveHandles().pulses;
        window.__CAP__ = { token, lit, rounds: 4, pairs: pairs.length,
          listed: agentGraphLiveRecentEvents().length };
        /* 뛰는 것은 상한 안, 그러나 사건 목록은 접히지 않는다. */
        return lit <= token && agentGraphLiveRecentEvents().length > token;
      }), await page.evaluate(() => JSON.stringify(window.__CAP__ ?? null)));

    /* ---- 브리핑이 청한 네 장 ------------------------------------------- */
    await mkdir("output/playwright/board-live", { recursive: true });
    await page.setViewportSize({ width: 1440, height: 960 });
    const shot = async (name) => {
      await page.evaluate(() => window.__BOARD_SETTLED__());
      await page.locator("#board-view").screenshot({
        path: `output/playwright/board-live/state-${name}.png` });
    };

    /* ① 조용한 판 — 지도는 서 있고 아무것도 뛰지 않는다. */
    await page.evaluate(async () => {
      const view = document.querySelector("#board-view");
      selectAgentGraphEntity(view, "agent:term:302");
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
    });
    await page.waitForFunction(() => agentGraphLiveHandles().pulses === 0);
    await shot("calm");

    /* ② 방금 일어난 일 — 실제 메시지 하나가 맥박 한 번으로. */
    await page.evaluate(async () => {
      const now = window.__LIVE_NOW__;
      window.__OVERLAYS__ = window.__LIVE_OVERLAYS__(now, { mail: [
        { from: "term:303", to: "term:301", count: 3, unread: 2, at: now + 900_000, verb: "mail",
          last_message: { id: "m-shot-live", run: "run-1", from: "worker:w-verify",
            to: "run:run-1", kind: "worker_done", created_ms: now + 900_000 } },
      ] });
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
    });
    ok("the active-event screenshot has a live beat to show",
      await page.evaluate(() => document.querySelectorAll("#board-view [data-live-beat]").length > 0));
    await page.locator("#board-view").screenshot({
      path: "output/playwright/board-live/state-active-event.png" });

    /* ③ 기다리는 판 — 사유와 나이를 원장의 낱말로. */
    await page.evaluate(async () => {
      const view = document.querySelector("#board-view");
      selectAgentGraphEntity(view, "agent:term:304");
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
    });
    ok("the waiting screenshot has a recorded cause to show",
      await page.evaluate(() => [...document.querySelectorAll("#board-view .agent-graph-wait")]
        .some((chip) => chip.textContent.trim() !== "")));
    await shot("waiting");

    /* ④ 고른 선의 근거 — 실제 메시지 ID 가 인스펙터에 선다. */
    await page.evaluate(() => {
      const view = document.querySelector("#board-view");
      setAgentGraphInspectorOpen(view, true);
      selectAgentGraphRelation(view, "overlay:mail:agent:term:301>agent:term:302");
    });
    ok("the evidence screenshot shows the real message id",
      await page.evaluate(() => (document.querySelector("#board-view .agent-relation-evidence")
        ?.textContent ?? "").includes("m-dispatch-1")));
    /* 그리고 그 근거가 사건 목록보다 **위**에 선다: 목록은 스물넷까지 서므로
     * 머리에 두면 방금 누른 선의 ID 가 화면 밖으로 밀린다. */
    ok("a_selected_edges_evidence_stands_above_the_event_list",
      await page.evaluate(() => {
        const pane = document.querySelector("#board-view .agent-inspector-pane.is-relations");
        const detail = pane?.querySelector(".agent-relation-detail");
        const events = pane?.querySelector(".agent-live-events");
        if (!detail || !events) return false;
        return (detail.compareDocumentPosition(events) & Node.DOCUMENT_POSITION_FOLLOWING) !== 0;
      }));
    await shot("edge-evidence");


    for (const width of [1440, 900, 560, 360]) {
      await page.setViewportSize({ width, height: 960 });
      await page.evaluate(async () => {
        const view = document.querySelector("#board-view");
        setAgentGraphInspectorOpen(view, false);
        paintAgentGraphEdges(view);
        await window.__BOARD_SETTLED__();
      });
      const layout = await page.evaluate(() => {
        const view = document.querySelector("#board-view");
        const live = view.querySelector(".agent-graph-live");
        return { overflow: view.scrollWidth > view.clientWidth + 1,
          reachable: live !== null && getComputedStyle(live).display !== "none" };
      });
      ok(`controls_and_inspector_remain_reachable at ${width}px`,
        !layout.overflow && layout.reachable, JSON.stringify(layout));
      await page.screenshot({ path: `output/playwright/board-live/live-${width}.png` });
    }

    ok("the live map raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* 지도를 켠 관계 그림 한 판 — 둘째·셋째 창의 출발점. */
async function openLiveMap(page) {
  await installBoardWaits(page);
  await page.evaluate(liveFixture);
  await page.waitForSelector(".task-board-row");
  await page.evaluate(async () => {
    document.querySelector('[data-board-mode="graph"]').click();
    await paintBoardView(undefined, { force: true });
    await window.__BOARD_SETTLED__();
    document.querySelector(".agent-graph-live").click();
    await window.__BOARD_SETTLED__();
  });
}

/* ---- 오래 사는 창의 상한과 수명 문 -----------------------------------------
 *
 * 한 범위를 그대로 둔 채 관계와 판을 상한의 몇 배로 흘려보내고, 장부가 드는
 * 모든 것의 수를 잰다. 그리고 실제 수명 문 — 손잡이, 작업 목록, 판 닫기, 범위
 * 거두기와 사라진 범위, 판을 품은 자리의 숨김 — 을 지나며 시계·맥박·박자·프레임이
 * 남지 않고 귀가 늘지 않는지 본다. */
async function testLiveLedgerBounds(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await openLiveMap(page);
    /* 이 창의 맥박은 오래 산다 — 수명 문을 지나는 사이 맥박이 제 수명으로
     * 꺼져서, 문이 거둔 것처럼 보이지 않도록. 토큰 하나를 판에서 덮는다. */
    await page.evaluate(() => {
      const view = document.querySelector("#board-view");
      view.style.setProperty("--agent-graph-live-pulse-ms", "60000");
      agentGraphTunings.delete(view);
    });

    /* 처음 보는 관계와 기억을 잃은 관계는 서로 다른 답을 받는다. 앞의 것은
     * 실제로 처음 보는 사건이고, 뒤의 것은 조용히 다시 맞춘다. */
    const evicted = await page.evaluate(() => {
      const now = window.__LIVE_NOW__;
      const overlays = (id, from, at) => ({ columns: [], overlays: { mail: [
        { from, to: "term:301", count: 1, unread: 0, at, verb: "mail",
          last_message: { id, run: "run-1", from: `worker:${from}`, to: "run:run-1",
            kind: "status", created_ms: at } },
      ] } });
      /* 목록에는 상한이 있어 길이로는 아무것도 못 센다 — **맨 앞의 사건이
       * 무엇인가**를 본다. */
      const head = () => agentGraphLiveRecentEvents()[0]?.key ?? null;
      const before = head();
      agentGraphLiveObserve(overlays("m-fresh", "term:401", now + 30_000), new Map(), Date.now());
      const firstSight = head();
      // 상한을 넘겨 그 관계를 밀어낸다.
      const laneKeep = agentGraphLiveCoverage().laneKeep;
      for (let at = 0; at < laneKeep + 44; at += 1) {
        agentGraphLiveObserve(overlays(`m-push-${at}`, `term:5${at}`, now + 30_001 + at),
          new Map(), Date.now());
      }
      const pushed = head();
      /* 밀려났던 관계가 **돌아온다** — 처음에는 더 옛 사건을 들고(늦게 온
       * 판), 그 다음에 한때 보았던 그 사건을 다시 들고. 둘 다 새 사건이
       * 아니다: 앞의 것은 옛것이고, 뒤의 것은 이미 본 것이다. */
      agentGraphLiveObserve(overlays("m-older", "term:401", now + 29_000), new Map(), Date.now());
      const staleReturn = head();
      agentGraphLiveObserve(overlays("m-fresh", "term:401", now + 30_000), new Map(), Date.now());
      const replay = head();
      /* 잊은 것보다 늦은 실제 사건은 여전히 새 사건이다. */
      agentGraphLiveObserve(overlays("m-fresh-late", "term:401", now + 90_000), new Map(), Date.now());
      return { before, firstSight, pushed, staleReturn, replay, after: head(),
        coverage: agentGraphLiveCoverage() };
    });
    ok("a_relation_evicted_by_the_cap_resyncs_quietly_instead_of_pulsing",
      // 처음 보는 관계는 사건이 되고 …
      evicted.firstSight !== evicted.before && evicted.firstSight?.endsWith("m-fresh") === true
      // … 상한에 밀려났다 돌아온 같은 관계는 조용하고, 장부는 그 사실을 말한다.
      && evicted.staleReturn === evicted.pushed && evicted.coverage.complete === false,
      JSON.stringify(evicted));
    ok("an_evicted_relation_that_returns_older_first_does_not_replay_what_it_had_seen",
      evicted.replay === evicted.pushed, JSON.stringify(evicted));
    ok("after_forgetting_a_later_real_event_still_counts",
      evicted.after?.endsWith("m-fresh-late") === true, JSON.stringify(evicted));

    /* 한 범위 안에서 판이 하나씩 앉았다 떠나기를 상한의 네 배. 판마다 제 관계와
     * 제 시도를 들고 오고 다시는 돌아오지 않는다 — 오래 사는 창에서 되돌아오지
     * 않는 관계와 떠난 판이 쌓이는 모양 그대로다. 잴 때마다 장부가 드는 모든 것의
     * 최댓값을 본다: 관계, 관계마다의 같은 시각 키, 사건 목록, 맥박, 시계. 옛 판이
     * 따로 들던 두 보조 기억(밀려난 관계의 이름표, 자리마다의 주체)이 다시 생기면
     * 그 이름으로 함께 센다. */
    const churn = await page.evaluate(() => {
      const now = window.__LIVE_NOW__ + 100_000;
      const coverage = agentGraphLiveCoverage();
      const view = document.querySelector("#board-view");
      const burst = agentGraphTuning(view).liveBurst;
      const held = () => {
        let keys = 0;
        for (const lane of liveLanes.values()) keys += lane.keys.size;
        const handles = agentGraphLiveHandles();
        return { lanes: liveLanes.size, keys, events: agentGraphLiveRecentEvents().length,
          pulses: handles.pulses, timers: handles.timers,
          tombstones: typeof liveDropped === "undefined" ? 0 : liveDropped.size,
          seats: typeof liveSubjects === "undefined" ? 0 : liveSubjects.size };
      };
      const rounds = coverage.laneKeep * 4;
      const peak = {};
      for (let at = 0; at < rounds; at += 1) {
        const pane = `term:${9000 + at}`;
        const places = new Map([[pane, { run: "run-9", workerId: `w-${at}`,
          dispatchId: `dp-${at}`, taskId: `t-${at}`, dispatchStarted: now + at,
          reported: false, retryOf: null }]]);
        agentGraphLiveObserve({ columns: [], overlays: { mail: [
          { from: pane, to: "term:301", count: 1, unread: 0, at: now + at, verb: "mail",
            last_message: { id: `m-churn-${at}`, run: "run-9", from: `worker:w-${at}`,
              to: "run:run-1", kind: "status", created_ms: now + at } },
        ] } }, places, now + at);
        for (const [name, value] of Object.entries(held())) peak[name] = Math.max(peak[name] ?? 0, value);
      }
      return { rounds, peak, final: held(), laneKeep: coverage.laneKeep,
        sameStampKeep: coverage.sameStampKeep, eventKeep: 24, burst };
    });
    ok("the_live_ledger_state_stays_bounded_under_relation_and_pane_churn",
      churn.peak.lanes <= churn.laneKeep
      && churn.peak.keys <= churn.laneKeep * churn.sameStampKeep
      && churn.peak.events <= churn.eventKeep
      && churn.peak.pulses <= churn.burst && churn.peak.timers <= 1
      && churn.peak.tombstones <= churn.laneKeep && churn.peak.seats <= churn.laneKeep,
      JSON.stringify(churn));

    /* 실제 수명 문. 문마다 먼저 실제 사건 하나로 맥박을 켜고(켜졌는지 확인한다),
     * 문을 지난 **직후** 시계·맥박·화면의 박자·그림 프레임을 센다. 판을 닫는
     * 문과 판을 품은 자리를 숨기는 문은 판의 크기를 재는 관찰자가 알리므로
     * 프레임 둘을 기다린다. */
    const doors = await page.evaluate(async () => {
      const view = document.querySelector("#board-view");
      const now = window.__LIVE_NOW__;
      let stamp = now + 400_000;
      const frames = () => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      const light = async () => {
        stamp += 1_000;
        window.__OVERLAYS__ = window.__LIVE_OVERLAYS__(now, { mail: [
          { from: "term:302", to: "term:303", count: 1, unread: 0, at: stamp, verb: "mail",
            last_message: { id: `m-door-${stamp}`, run: "run-1", from: "worker:w-impl",
              to: "worker:w-verify", kind: "status", created_ms: stamp } },
        ] });
        await paintBoardView(undefined, { force: true });
        await window.__BOARD_SETTLED__();
        const handles = agentGraphLiveHandles();
        return { timers: handles.timers, pulses: handles.pulses };
      };
      const left = async ({ settle = true } = {}) => {
        if (settle) await window.__BOARD_SETTLED__();
        await frames();
        const handles = agentGraphLiveHandles();
        return { timers: handles.timers, pulses: handles.pulses,
          beats: document.querySelectorAll(".agent-board [data-live-beat]").length,
          frames: agentGraphEdgeFrames.has(view) || agentGraphFitFrames.has(view) };
      };
      /* 문을 지나는 동안 문서와 창에 새로 걸리는 귀. */
      let heard = 0;
      const listen = EventTarget.prototype.addEventListener;
      EventTarget.prototype.addEventListener = function (...args) {
        if (this === document || this === window) heard += 1;
        return listen.apply(this, args);
      };
      const listeners = agentGraphLiveHandles().listeners;
      const record = {};
      const scope = () => view.querySelector(".agent-graph-scope");
      /* 범위를 옮긴 판은 다음 판을 기준선으로 삼킨다 — 그 한 판을 받아 두어야
       * 뒤의 사건이 실제 사건으로 선다. */
      const choose = async (key) => {
        scope().value = key;
        scope().dispatchEvent(new Event("change"));
        await paintBoardView(undefined, { force: true });
        await window.__BOARD_SETTLED__();
      };
      /* 숨었다 돌아온 판은 그 한 판을 기준선으로 받는다 — 범위를 옮긴 판과 같다. */
      const returned = async () => {
        await paintBoardView(undefined, { force: true });
        await window.__BOARD_SETTLED__();
      };
      try {
        for (let cycle = 0; cycle < 2; cycle += 1) {
          const at = {};
          /* ① 손잡이를 끄고 다시 켠다. */
          at.offLit = await light();
          view.querySelector(".agent-graph-live").click();
          at.off = await left();
          view.querySelector(".agent-graph-live").click();
          at.on = await left();
          /* ② 작업 목록으로 갔다 관계로 돌아온다. 돌아온 첫 판은 기준선이다 — 그 한
           *    판을 받아 두어야 뒤의 사건이 실제 사건으로 선다(아래 `returned`가 그
           *    계약 자체를 잰다). */
          at.tasksLit = await light();
          view.querySelector('[data-board-mode="tasks"]').click();
          at.tasks = await left();
          view.querySelector('[data-board-mode="graph"]').click();
          await window.__BOARD_SETTLED__();
          await returned();
          /* ③ 판을 닫았다 다시 연다. */
          at.closeLit = await light();
          dropTab("board");
          at.closed = await left({ settle: false });
          openBoard();
          await window.__BOARD_SETTLED__();
          /* ④ 범위를 골랐다 거둔다 — 범위 고르개 그 손으로. */
          const key = [...scope().options].map((option) => option.value).find((value) => value !== "");
          await choose(key);
          at.scopeLit = await light();
          await choose("");
          at.scopeCleared = await left();
          /* ⑤ 고른 범위가 스냅샷에서 **사라진다** — 그 범위의 판들이 떠났다. */
          await choose(key);
          at.goneLit = await light();
          const full = agentGraphFullModel(view);
          const members = new Set(agentGraphTasksFor(view, full).groups
            .find((one) => one.key === key)?.members.map((entry) => entry.card.pane) ?? []);
          const fixture = { panes: window.__PANES__, ledger: window.__LEDGER__,
            columns: structuredClone(window.__COLUMNS__) };
          window.__PANES__ = window.__PANES__.filter((one) => !members.has(`term:${one.term}`));
          window.__LEDGER__ = window.__LEDGER__.filter((row) =>
            !members.has(row.term == null ? `worker:${row.worker}` : `term:${row.term}`));
          for (const column of window.__COLUMNS__) {
            column.cards = column.cards.filter((card) => !members.has(card.pane));
          }
          await paintBoardView(undefined, { force: true });
          at.gone = await left();
          at.goneMembers = members.size;
          window.__PANES__ = fixture.panes;
          window.__LEDGER__ = fixture.ledger;
          window.__COLUMNS__ = fixture.columns;
          await choose("");
          await paintBoardView(undefined, { force: true });
          await window.__BOARD_SETTLED__();
          /* ⑥ 판을 품은 자리가 숨는다. */
          at.hideLit = await light();
          view.parentElement.hidden = true;
          at.hidden = await left({ settle: false });
          view.parentElement.hidden = false;
          await window.__BOARD_SETTLED__();
          await returned();
          at.heard = heard;
          record[`cycle${cycle}`] = at;
        }
      } finally {
        EventTarget.prototype.addEventListener = listen;
      }
      return { record, listeners: { before: listeners, after: agentGraphLiveHandles().listeners } };
    });
    const doorNames = [["off", "offLit"], ["tasks", "tasksLit"], ["closed", "closeLit"],
      ["scopeCleared", "scopeLit"], ["gone", "goneLit"], ["hidden", "hideLit"]];
    for (const [door, lit] of doorNames) {
      const passed = ["cycle0", "cycle1"].every((cycle) => {
        const at = doors.record[cycle];
        return at[lit].timers === 1 && at[lit].pulses > 0
          && at[door].timers === 0 && at[door].pulses === 0 && at[door].beats === 0
          && at[door].frames === false;
      });
      ok(`the_${door}_door_leaves_no_timer_pulse_or_frame_behind`, passed,
        JSON.stringify(Object.fromEntries(["cycle0", "cycle1"].map((cycle) =>
          [cycle, { lit: doors.record[cycle][lit], door: doors.record[cycle][door] }]))));
    }
    ok("turning_the_map_back_on_after_a_door_stays_quiet",
      ["cycle0", "cycle1"].every((cycle) => doors.record[cycle].on.timers === 0
        && doors.record[cycle].on.pulses === 0),
      JSON.stringify(doors.record.cycle1.on));
    ok("passing_the_doors_twice_adds_no_listener",
      doors.listeners.before === doors.listeners.after
      && doors.record.cycle1.heard === doors.record.cycle0.heard,
      JSON.stringify({ listeners: doors.listeners,
        heard: [doors.record.cycle0.heard, doors.record.cycle1.heard] }));

    /* 마지막으로 보이던 지도를 떠났다 **돌아온 첫 판**은 조용한 기준선이다. 문서는
     * 앞에 있는 채로 — 판을 품은 자리가 숨거나, 작업 목록으로 가거나, 판을 닫는다.
     * 떠난 사이 원장에 사건이 적히고, 제품이 떠난 뒤에도 그리는 문(숨김·작업 목록)
     * 에서는 한 판이 그것을 읽는다. 그 뒤로 사건이 하나 더 적히고, 이 지도는 그것을
     * 아직 읽지 않았다. 돌아온 첫 판이 그 backlog를 새 활동으로 세우면 오래전에
     * 일어난 일이 지금 막 뛴다.
     *
     * 떠난 사이의 판도, 돌아온 첫 판도 목록의 머리·맥박·화면의 박자를 움직이지
     * 않는다. 그리고 그 다음 **실제** 새 사건은 한 번 뛴다 — 조용함이 먹통이 아니다. */
    const returning = await page.evaluate(async () => {
      const view = document.querySelector("#board-view");
      const now = window.__LIVE_NOW__;
      let stamp = now + 700_000;
      const frames = () => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      const record = (name) => {
        stamp += 1_000;
        const id = `${name}-${stamp}`;
        window.__OVERLAYS__ = window.__LIVE_OVERLAYS__(now, { mail: [
          { from: "term:303", to: "term:302", count: 1, unread: 0, at: stamp, verb: "mail",
            last_message: { id, run: "run-1", from: "worker:w-verify", to: "worker:w-impl",
              kind: "status", created_ms: stamp } },
        ] });
        return id;
      };
      const paint = async () => {
        await paintBoardView(undefined, { force: true });
        await window.__BOARD_SETTLED__();
        await frames();
      };
      const read = () => {
        const handles = agentGraphLiveHandles();
        return { head: agentGraphLiveRecentEvents()[0]?.key ?? null, timers: handles.timers,
          pulses: handles.pulses, beats: document.querySelectorAll(".agent-board [data-live-beat]").length };
      };
      const doors = {
        hidden: {
          leave: async () => { view.parentElement.hidden = true; await frames(); },
          back: async () => { view.parentElement.hidden = false; await frames(); },
          painting: true },
        tasks: {
          leave: async () => { view.querySelector('[data-board-mode="tasks"]').click(); await frames(); },
          back: async () => { view.querySelector('[data-board-mode="graph"]').click(); await frames(); },
          painting: true },
        /* 닫힌 판은 그리지 않는다(`boardIsOpen`) — 다시 연 판이 그 자리에서 그린다. */
        closed: {
          leave: async () => { dropTab("board"); await frames(); },
          back: async () => { openBoard(); await window.__BOARD_SETTLED__(); await frames(); },
          painting: false },
      };
      const out = {};
      for (const [name, door] of Object.entries(doors)) {
        record("m-shown");
        await paint();
        const before = read();
        await door.leave();
        record("m-away");
        if (door.painting) await paint();
        const away = read();
        const late = record("m-away-late");
        await door.back();
        await paint();
        const first = read();
        const after = record("m-after");
        await paint();
        const next = read();
        out[name] = { before, away, late, first, after, next };
      }
      return out;
    });
    for (const [door, seen] of Object.entries(returning)) {
      ok(`a_hidden_map_adopts_the_changed_return_snapshot_without_new_activity (${door})`,
        seen.away.head === seen.before.head && seen.away.pulses === 0 && seen.away.beats === 0
        && seen.first.head === seen.before.head && seen.first.pulses === 0
        && seen.first.timers === 0 && seen.first.beats === 0,
        JSON.stringify(seen));
      ok(`after_the_quiet_return_the_next_real_event_pulses_once (${door})`,
        seen.next.head?.endsWith(seen.after) === true && seen.next.pulses === 1
        && seen.next.timers === 1 && seen.next.beats === 1,
        JSON.stringify(seen));
    }

    /* 돌아온 첫 판이 떠나기 전과 **같은 판**일 때. 제품의 박자(`scheduleAgentPaint`,
     * 강제 없음)는 서명이 같은 판을 그리지 않고 건너뛴다 — 그 판도 빚을 치러야
     * 한다. 치르지 않으면 빚이 다음에 **달라진** 판으로 넘어가, 돌아온 뒤에 실제로
     * 일어난 첫 사건을 기준선으로 삼킨다. 서명이 분 경계에서 흔들리지 않도록 시계를
     * 멈추고(맥박은 이 창의 긴 수명 안에서 센다), 모든 판을 제품의 박자로 받는다. */
    const unchanged = await page.evaluate(async () => {
      const view = document.querySelector("#board-view");
      const now = window.__LIVE_NOW__;
      let stamp = now + 800_000;
      const frames = () => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      const record = (name) => {
        stamp += 1_000;
        const id = `${name}-${stamp}`;
        window.__OVERLAYS__ = window.__LIVE_OVERLAYS__(now, { mail: [
          { from: "term:303", to: "term:302", count: 1, unread: 0, at: stamp, verb: "mail",
            last_message: { id, run: "run-1", from: "worker:w-verify", to: "worker:w-impl",
              kind: "status", created_ms: stamp } },
        ] });
        return id;
      };
      const beat = async () => {
        scheduleAgentPaint(["board"]);
        await window.__BOARD_SETTLED__();
        await frames();
      };
      const read = () => ({ head: agentGraphLiveRecentEvents()[0]?.key ?? null,
        pulses: agentGraphLiveHandles().pulses });
      const doors = {
        hidden: {
          leave: async () => { view.parentElement.hidden = true; await frames(); },
          back: async () => { view.parentElement.hidden = false; await frames(); } },
        closed: {
          leave: async () => { dropTab("board"); await frames(); },
          back: async () => { openBoard(); await window.__BOARD_SETTLED__(); await frames(); } },
      };
      const trueNow = Date.now;
      const pinned = trueNow.call(Date);
      Date.now = () => pinned;
      const out = {};
      try {
        await paintBoardView(undefined, { force: true });
        await window.__BOARD_SETTLED__();
        for (const [name, door] of Object.entries(doors)) {
          record("m-steady");
          await beat();
          const before = read();
          await door.leave();
          await door.back();
          await beat();
          const first = read();
          const after = record("m-after-return");
          await beat();
          out[name] = { before, first, after, next: read() };
        }
      } finally {
        Date.now = trueNow;
      }
      return out;
    });
    for (const [door, seen] of Object.entries(unchanged)) {
      ok(`an_unchanged_first_snapshot_after_the_return_pays_the_baseline (${door})`,
        seen.before.head?.includes("m-steady") === true
        && seen.first.head === seen.before.head && seen.first.pulses === 0
        && seen.next.head?.endsWith(seen.after) === true && seen.next.pulses === 1,
        JSON.stringify(seen));
    }

    ok("the bounded ledger page raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* ---- 늦게 도착한 답 ----------------------------------------------------------
 *
 * 실제 입구(`paintBoardView` → `boardCards` → `board_snapshot`)를 지나는 두 판을
 * 겹쳐 세운다. `board_snapshot`을 붙잡아 앞 판 A를 세워 두고, 그 사이 범위를
 * 옮기거나 판의 시도를 바꾸고, 뒤 판 B를 먼저 들여보낸 뒤 A를 늦게 도착시킨다.
 * 늦은 A는 **아무것도** 쓰지 않아야 한다 — 배지, 초안, 펼친 타임라인, 오버레이,
 * 서명, 실시간 장부, 모델, 선택. 그리고 한 판은 제가 물은 원장 행만 읽는다. */
async function testLateAnswers(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await openLiveMap(page);
    await page.evaluate(() => {
      /* 붙잡는 문. 판이 `board_snapshot`을 묻는 **그 순간**의 답을 하네스에게서
       * 빌려 굳혀 두고, 풀어 줄 때 돌려준다. 하네스의 `invoke`는 답을 짓는 손을
       * 곧바로(첫 `await` 전에) 부르므로, 붙잡는 문을 잠깐 비키는 사이 다른 답이
       * 끼지 못한다. */
      window.__PARKED_SNAPSHOTS__ = [];
      const held = (args) => {
        delete window.__ANSWER__.board_snapshot;
        const asked = window.__TAURI__.core.invoke("board_snapshot", args);
        window.__ANSWER__.board_snapshot = held;
        return asked.then((answer) => {
          const frozen = structuredClone(answer);
          /* 붙잡은 답은 풀어 줄 수도, **거절할** 수도 있다 — 늦게 온 실패도 늦게 온
           * 성공과 같은 문을 지나야 하기 때문이다. */
          return new Promise((release, refuse) => {
            window.__PARKED_SNAPSHOTS__.push({ release: () => release(frozen),
              refuse: () => refuse(new Error("board_snapshot refused while held")) });
          });
        });
      };
      window.__HOLD_SNAPSHOTS__ = (on) => {
        if (on) window.__ANSWER__.board_snapshot = held;
        else delete window.__ANSWER__.board_snapshot;
      };
      window.__RELEASE_SNAPSHOT__ = (index) => window.__PARKED_SNAPSHOTS__[index].release();
      window.__REFUSE_SNAPSHOT__ = (index) => window.__PARKED_SNAPSHOTS__[index].refuse();
      /* 판을 모으는 앞 절반(`pane_agents`)도 같은 방법으로 붙잡는다. */
      window.__PARKED_PANES__ = [];
      window.__HOLD_PANES__ = (on) => {
        if (!on) {
          delete window.__ANSWER__.pane_agents;
          return;
        }
        window.__ANSWER__.pane_agents = () => new Promise((release, refuse) => {
          window.__PARKED_PANES__.push({ release: () => release(window.__PANES__ ?? []),
            refuse: () => refuse(new Error("pane_agents refused while held")) });
        });
      };
      /* 판이 사람에게 띄운 오류 — 문장 그대로, 띄운 차례로. */
      window.__SHOWN_ERRORS__ = [];
      const shown = window.showError;
      window.showError = (error) => {
        window.__SHOWN_ERRORS__.push(String(error));
        return shown(error);
      };
      /* 멈춘 판의 복구 카드가 서 있는가. */
      window.__BROKEN__ = () => {
        const view = document.querySelector("#board-view");
        return { flag: boardBroken, card: view.querySelector(".board-broken")?.hidden === false,
          layout: view.querySelector(".agent-graph-layout")?.hidden === false,
          retry: view.querySelector(".board-broken .board-retry")?.onclick === retryBoardPaint };
      };
      /* 붙잡은 것을 모두 풀고 범위를 거둔 뒤 지금 판으로 한 번 — 다음 사례의 출발점.
       * 앞 사례가 판을 멈춰 세웠으면(고치기 전 제품) 다시 시도로 세운다: 그러지
       * 않으면 뒤 사례는 멈춘 판 앞에서 붙잡기를 기다리다 시간이 다 되고, 그 빨강은
       * 그 사례의 결함이 아니라 앞 사례가 남긴 자국이다. */
      window.__UNPARK_ALL__ = async () => {
        window.__HOLD_SNAPSHOTS__(false);
        window.__HOLD_PANES__(false);
        await window.__FRAMES__();
        for (const parked of window.__PARKED_SNAPSHOTS__.splice(0)) parked.release();
        for (const parked of window.__PARKED_PANES__.splice(0)) parked.release();
        await window.__BOARD_SETTLED__();
        if (boardBroken) {
          document.querySelector("#board-view .board-broken .board-retry")?.click();
          await window.__BOARD_SETTLED__();
        }
        const scope = document.querySelector("#board-view .agent-graph-scope");
        if (scope.value !== "") {
          scope.value = "";
          scope.dispatchEvent(new Event("change"));
        }
        await paintBoardView(undefined, { force: true });
        await window.__BOARD_SETTLED__();
      };
      /* 지금 화면이 무엇을 쓰고 있는가 — 늦은 답이 쓸 수 있는 모든 자리. */
      window.__BOARD_WROTE__ = () => {
        const view = document.querySelector("#board-view");
        const model = agentGraphModels.get(view);
        return {
          badge: el("board-badge")?.textContent ?? null,
          drafts: [...askDrafts.keys()].sort(),
          timelines: [...agentGraphExpandedTimelines].sort(),
          overlays: JSON.stringify(agentGraphSnapshotOverlays),
          said: view.dataset.said ?? null,
          events: agentGraphLiveRecentEvents().map((event) => event.key),
          pulses: agentGraphLiveHandles().pulses,
          model,
          selected: [agentGraphSelectedKey, agentGraphSelectedEdgeKey],
          scope: agentGraphScopeKey,
          ledger: model?.source.ledger ?? null,
        };
      };
      window.__SAME_WRITES__ = (left, right) => Object.keys(left).filter((name) =>
        (name === "model" || name === "ledger") ? left[name] !== right[name]
          : JSON.stringify(left[name]) !== JSON.stringify(right[name]));
      window.__FRAMES__ = () => new Promise((done) =>
        requestAnimationFrame(() => requestAnimationFrame(done)));
    });

    /* ① 범위를 옮긴 사이 늦게 온 답. A는 옛 범위에서 떠났고, 그 답에는 지금은
     *    없는 사실들이 실려 있다: 확인할 카드 둘(배지 2), 아직 묻지 않은 카드,
     *    아직 없던 카드, 그리고 새 메시지 하나. */
    const moved = await page.evaluate(async () => {
      const view = document.querySelector("#board-view");
      const now = window.__LIVE_NOW__;
      const base = { columns: structuredClone(window.__COLUMNS__), overlays: window.__OVERLAYS__ };
      const card = (pane, over = {}) => ({ ...base.columns[0].cards.find((one) => one.pane === pane), ...over });
      window.__COLUMNS__ = [
        { bucket: "attention", cards: [card("term:305"), card("term:307")] },
        { bucket: "working", cards: base.columns[0].cards.filter((one) =>
          one.pane !== "term:305" && one.pane !== "term:307") },
      ];
      window.__OVERLAYS__ = window.__LIVE_OVERLAYS__(now, { mail: [
        { from: "term:307", to: "term:306", count: 1, unread: 1, at: now + 50_000, verb: "mail",
          last_message: { id: "m-late-a", run: "run-1", from: "worker:w-child",
            to: "worker:w-parent", kind: "status", created_ms: now + 50_000 } },
      ] });
      window.__HOLD_SNAPSHOTS__(true);
      window.__A__ = paintBoardView(undefined, { force: true });
      await window.__UNTIL__(() => window.__PARKED_SNAPSHOTS__.length === 1, "A parked", 5_000);
      /* 사람이 범위를 옮긴다 — 범위 고르개 그 손으로. */
      const scope = view.querySelector(".agent-graph-scope");
      const key = [...scope.options].map((option) => option.value).find((value) => value !== "");
      scope.value = key;
      scope.dispatchEvent(new Event("change"));
      /* 그 사이 세상은 이렇게 됐다: 확인할 카드는 하나(묻는 판 term:303), 새
       * 카드 term:330, 메시지는 그대로. */
      window.__COLUMNS__ = [
        { bucket: "attention", cards: [card("term:303", { state: "needs-attention", ask: "계속할까요?",
          ask_prompt: { questions: [{ question: "계속할까요?", multi_select: false,
            options: [{ label: "예", description: "계속합니다." }] }] } })] },
        { bucket: "working", cards: [...base.columns[0].cards.filter((one) => one.pane !== "term:303"),
          card("term:302", { pane: "term:330", heading: "새 카드", task: "새 카드" })] },
      ];
      window.__OVERLAYS__ = base.overlays;
      window.__B__ = paintBoardView(undefined, { force: true });
      await window.__UNTIL__(() => window.__PARKED_SNAPSHOTS__.length === 2, "B parked", 5_000);
      window.__RELEASE_SNAPSHOT__(1);
      await window.__B__;
      await window.__FRAMES__();
      /* 사람이 B 위에서 손댄 것: 묻는 카드의 초안, 새 카드의 펼친 타임라인. */
      const asking = agentGraphFullModel(view).agents.find((entry) => entry.card.pane === "term:303");
      if (asking) askDraftFor(asking.card);
      agentGraphExpandedTimelines.add("term:330");
      const committed = window.__BOARD_WROTE__();
      window.__RELEASE_SNAPSHOT__(0);
      const late = await window.__A__;
      const after = window.__BOARD_WROTE__();
      const changed = window.__SAME_WRITES__(committed, after);
      /* 버린 답의 몫 — 사람이 누른 다시 그리기는 **지금의** 답으로 한 번 더. */
      window.__HOLD_SNAPSHOTS__(false);
      await window.__FRAMES__();
      for (const parked of window.__PARKED_SNAPSHOTS__.splice(0)) parked.release();
      await window.__BOARD_SETTLED__();
      const again = window.__BOARD_WROTE__();
      const out = {
        late, changed, scope: key,
        committed: { ...committed, model: undefined, ledger: undefined, said: undefined },
        after: { ...after, model: undefined, ledger: undefined, said: undefined },
        again: { badge: again.badge, events: again.events, scope: again.scope,
          lateMessage: again.overlays.includes("m-late-a"),
          drafts: again.drafts, timelines: again.timelines },
        answered: window.__COUNTS__.board_snapshot ?? 0,
      };
      window.__COLUMNS__ = base.columns;
      scope.value = "";
      scope.dispatchEvent(new Event("change"));
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
      return out;
    });
    ok("a_late_answer_from_before_a_scope_move_writes_nothing",
      moved.late === null && moved.changed.length === 0
      && moved.committed.badge === "1" && moved.committed.drafts.includes("term:303")
      && moved.committed.timelines.includes("term:330"),
      JSON.stringify(moved));
    ok("the_dropped_answers_repaint_is_asked_again_from_the_present",
      moved.again.badge === "1" && moved.again.lateMessage === false
      && moved.again.scope === moved.scope && moved.again.drafts.includes("term:303"),
      JSON.stringify(moved.again));

    /* ② 같은 판에 새 시도가 앉은 사이 늦게 온 답. B가 먼저 새 시도(dp-next)를
     *    그렸고, 그보다 **먼저 떠난** A가 옛 시도(dp-other)를 들고 늦게 온다. */
    const attempt = await page.evaluate(async () => {
      const view = document.querySelector("#board-view");
      const now = window.__LIVE_NOW__;
      const at = window.__LEDGER__.findIndex((one) => one.term === 312);
      const held = window.__LEDGER__[at];
      window.__HOLD_SNAPSHOTS__(true);
      window.__A__ = paintBoardView(undefined, { force: true });
      await window.__UNTIL__(() => window.__PARKED_SNAPSHOTS__.length === 1, "A parked", 5_000);
      window.__LEDGER__ = window.__LEDGER__.map((row, index) => index === at
        ? { ...held, worker: "w-next", dispatch_id: "dp-next", dispatch_started_ms: now + 70_000,
          task_id: "t-next", task: "다음 시도", retry_of: "dp-other" }
        : row);
      window.__B__ = paintBoardView(undefined, { force: true });
      await window.__UNTIL__(() => window.__PARKED_SNAPSHOTS__.length === 2, "B parked", 5_000);
      window.__RELEASE_SNAPSHOT__(1);
      await window.__B__;
      await window.__FRAMES__();
      const seat = () => agentGraphModels.get(view)?.source.places.get("term:312")?.dispatchId ?? null;
      const committed = { ...window.__BOARD_WROTE__(), seat: seat() };
      window.__RELEASE_SNAPSHOT__(0);
      const late = await window.__A__;
      const after = { ...window.__BOARD_WROTE__(), seat: seat() };
      const changed = window.__SAME_WRITES__(committed, after);
      window.__HOLD_SNAPSHOTS__(false);
      await window.__FRAMES__();
      for (const parked of window.__PARKED_SNAPSHOTS__.splice(0)) parked.release();
      await window.__BOARD_SETTLED__();
      const out = { late, changed, committedSeat: committed.seat, afterSeat: after.seat,
        assigned: agentGraphLiveRecentEvents().some((event) => event.evidence?.dispatchId === "dp-next") };
      window.__LEDGER__ = window.__LEDGER__.map((row, index) => index === at ? held : row);
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
      return out;
    });
    ok("an_answer_asked_before_an_attempt_change_does_not_repaint_the_old_attempt",
      attempt.late === null && attempt.changed.length === 0
      && attempt.committedSeat === "dp-next" && attempt.afterSeat === "dp-next" && attempt.assigned,
      JSON.stringify(attempt));

    /* ③ 두 판이 겹칠 때 한 판은 **제가 물은** 원장 행만 읽는다. A가 물을 때
     *    w-impl 은 한도 벽 앞이었고, B가 물을 때는 벽이 걷혔다. A가 먼저 들어가면
     *    그 판의 노드는 A의 행(벽)을 말해야 한다 — 나중에 떠난 B의 행이 아니라. */
    const bundle = await page.evaluate(async () => {
      const view = document.querySelector("#board-view");
      const now = Date.now();
      const at = window.__LEDGER__.findIndex((one) => one.worker === "w-impl");
      const held = window.__LEDGER__[at];
      const chip = () => {
        const node = view.querySelector('.agent-graph-node[data-graph-key="agent:term:302"] .agent-graph-wait');
        return { classes: node?.className ?? null, said: node?.textContent ?? null };
      };
      window.__LEDGER__ = window.__LEDGER__.map((row, index) => index === at
        ? { ...held, wall: { wall: "m-wall-a", observed_at_ms: now - 60_000, resets_at_ms: now + 600_000,
          reset_waitable: true, stands_until_ms: now + 600_000 } }
        : row);
      window.__HOLD_SNAPSHOTS__(true);
      window.__A__ = paintBoardView(undefined, { force: true });
      await window.__UNTIL__(() => window.__PARKED_SNAPSHOTS__.length === 1, "A parked", 5_000);
      window.__LEDGER__ = window.__LEDGER__.map((row, index) => index === at ? { ...held, wall: null } : row);
      window.__B__ = paintBoardView(undefined, { force: true });
      await window.__UNTIL__(() => window.__PARKED_SNAPSHOTS__.length === 2, "B parked", 5_000);
      window.__RELEASE_SNAPSHOT__(0);
      const first = await window.__A__;
      await window.__FRAMES__();
      const asA = { chip: chip(),
        wall: agentGraphModels.get(view)?.source.ledger?.get("term:302")?.wall?.wall ?? null };
      window.__RELEASE_SNAPSHOT__(1);
      const second = await window.__B__;
      await window.__FRAMES__();
      const asB = { chip: chip(),
        wall: agentGraphModels.get(view)?.source.ledger?.get("term:302")?.wall?.wall ?? null };
      window.__HOLD_SNAPSHOTS__(false);
      for (const parked of window.__PARKED_SNAPSHOTS__.splice(0)) parked.release();
      await window.__BOARD_SETTLED__();
      window.__LEDGER__ = window.__LEDGER__.map((row, index) => index === at ? held : row);
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
      return { painted: [first !== null, second !== null], asA, asB };
    });
    ok("a_paint_reads_the_ledger_rows_of_its_own_snapshot_bundle",
      bundle.painted.every(Boolean)
      && bundle.asA.chip.classes?.includes("is-walled") === true && bundle.asA.wall === "m-wall-a"
      && bundle.asB.chip.classes?.includes("is-walled") === false && bundle.asB.wall === null,
      JSON.stringify(bundle));

    /* ④ 늦게 온 **실패**. 늦은 성공만 조용히 버려서는 「늦은 답은 아무것도 쓰지
     *    않는다」가 완성되지 않는다 — 옛 범위에서 떠난 요청이 넘어지면 그 실패도
     *    옛것이다: 판을 멈춰 세우지도(`boardBroken`), 복구 카드를 세우지도, 오류를
     *    띄우지도 않는다. 넷을 본다: 범위를 옮긴 뒤 **새 답 없이** 옛 요청이 거절됨,
     *    새 답이 들어간 뒤 옛 요청이 거절됨, 옛 요청의 판 모으기(`pane_agents`)가
     *    거절됨, 훅의 박자가 물은 옛 요청이 거절됨. */
    const stale = await page.evaluate(async () => {
      const view = document.querySelector("#board-view");
      const scope = view.querySelector(".agent-graph-scope");
      const key = [...scope.options].map((option) => option.value).find((value) => value !== "");
      const move = async () => {
        scope.value = key;
        scope.dispatchEvent(new Event("change"));
        await window.__FRAMES__();
      };
      /* 붙잡은 답이 넘어지기 전과 뒤에 판이 쓴 모든 것, 멈춤 상태, 띄운 오류. */
      const across = async (fall) => {
        window.__SHOWN_ERRORS__.length = 0;
        const committed = { ...window.__BOARD_WROTE__(), broken: window.__BROKEN__() };
        const late = await fall();
        await window.__FRAMES__();
        const after = { ...window.__BOARD_WROTE__(), broken: window.__BROKEN__() };
        return { late, changed: window.__SAME_WRITES__(committed, after), broken: after.broken,
          shown: [...window.__SHOWN_ERRORS__] };
      };
      const out = {};
      /* ⓐ A가 옛 범위에서 기다리는 사이 범위를 옮기고, 새 답 없이 A가 넘어진다. */
      window.__HOLD_SNAPSHOTS__(true);
      window.__A__ = paintBoardView(undefined, { force: true });
      await window.__UNTIL__(() => window.__PARKED_SNAPSHOTS__.length === 1, "A parked", 5_000);
      await move();
      out.alone = await across(async () => {
        window.__REFUSE_SNAPSHOT__(0);
        return window.__A__;
      });
      await window.__UNPARK_ALL__();
      /* ⓑ A가 기다리는 사이 범위를 옮기고, 새 범위의 B가 먼저 들어간 뒤 A가 넘어진다. */
      window.__HOLD_SNAPSHOTS__(true);
      window.__A__ = paintBoardView(undefined, { force: true });
      await window.__UNTIL__(() => window.__PARKED_SNAPSHOTS__.length === 1, "A parked", 5_000);
      await move();
      window.__B__ = paintBoardView(undefined, { force: true });
      await window.__UNTIL__(() => window.__PARKED_SNAPSHOTS__.length === 2, "B parked", 5_000);
      window.__RELEASE_SNAPSHOT__(1);
      await window.__B__;
      await window.__FRAMES__();
      out.afterB = await across(async () => {
        window.__REFUSE_SNAPSHOT__(0);
        return window.__A__;
      });
      await window.__UNPARK_ALL__();
      /* ⓒ A의 판 모으기가 기다리는 사이 범위를 옮기고, 그 모으기가 넘어진다. */
      window.__HOLD_PANES__(true);
      window.__A__ = paintBoardView(undefined, { force: true });
      await window.__UNTIL__(() => window.__PARKED_PANES__.length === 1, "A gather parked", 5_000);
      await move();
      out.gather = await across(async () => {
        window.__PARKED_PANES__[0].refuse();
        return window.__A__;
      });
      await window.__UNPARK_ALL__();
      /* ⓓ 훅의 박자가 묻는 사이 범위를 옮기고, 그 답이 넘어진다. */
      window.__HOLD_SNAPSHOTS__(true);
      const hook = refreshAgentGraphSurfaces({ badge: false, graph: true });
      await window.__UNTIL__(() => window.__PARKED_SNAPSHOTS__.length === 1, "hook parked", 5_000);
      await move();
      out.hook = await across(async () => {
        window.__REFUSE_SNAPSHOT__(0);
        return hook;
      });
      await window.__UNPARK_ALL__();
      return out;
    });
    for (const [road, seen] of Object.entries(stale)) {
      ok(`a_stale_request_that_fails_writes_nothing (${road})`,
        seen.changed.length === 0 && seen.broken.flag === false && seen.broken.card === false
        && seen.broken.layout === true && seen.shown.length === 0 && (seen.late ?? null) === null,
        JSON.stringify(seen));
    }

    /* ⑤ 지금 세대의 **진짜** 실패는 그대로 말한다. 판은 멈춘 판의 복구 카드를
     *    세우고, 그 카드는 이미 떠나 있던 다른 답이 늦게 성공해도 덮이지 않는다 —
     *    멈춘 판을 여는 것은 다시 시도뿐이다. 다시 시도하면 판이 선다. 훅의 박자가
     *    물은 지금의 실패도 예전처럼 오류로 뜬다. */
    const current = await page.evaluate(async () => {
      const view = document.querySelector("#board-view");
      window.__SHOWN_ERRORS__.length = 0;
      window.__HOLD_SNAPSHOTS__(true);
      window.__A__ = paintBoardView(undefined, { force: true });
      await window.__UNTIL__(() => window.__PARKED_SNAPSHOTS__.length === 1, "A parked", 5_000);
      window.__B__ = paintBoardView(undefined, { force: true });
      await window.__UNTIL__(() => window.__PARKED_SNAPSHOTS__.length === 2, "B parked", 5_000);
      window.__REFUSE_SNAPSHOT__(0);
      const late = await window.__A__;
      await window.__FRAMES__();
      const broke = window.__BROKEN__();
      window.__RELEASE_SNAPSHOT__(1);
      const other = await window.__B__;
      await window.__FRAMES__();
      const held = window.__BROKEN__();
      window.__HOLD_SNAPSHOTS__(false);
      view.querySelector(".board-broken .board-retry").click();
      await window.__BOARD_SETTLED__();
      const retried = { ...window.__BROKEN__(),
        cards: view.querySelectorAll(".agent-graph-node.is-agent").length };
      /* 훅의 박자가 물은 지금의 실패. */
      window.__HOLD_SNAPSHOTS__(true);
      const hook = refreshAgentGraphSurfaces({ badge: false, graph: true });
      await window.__UNTIL__(() => window.__PARKED_SNAPSHOTS__.length === 3, "hook parked", 5_000);
      window.__REFUSE_SNAPSHOT__(2);
      await hook;
      const hookShown = [...window.__SHOWN_ERRORS__];
      await window.__UNPARK_ALL__();
      return { late, broke, other, held, retried, hookShown, after: window.__BROKEN__() };
    });
    ok("a_current_failure_still_stands_the_recovery_card",
      current.late === null && current.broke.flag === true && current.broke.card === true
      && current.broke.layout === false && current.broke.retry === true,
      JSON.stringify(current));
    ok("an_answer_already_in_flight_does_not_paint_over_the_recovery_card",
      current.other === null && current.held.flag === true && current.held.card === true
      && current.held.layout === false,
      JSON.stringify(current));
    ok("retrying_a_broken_board_stands_it_again_and_a_current_hook_failure_still_says_so",
      current.retried.flag === false && current.retried.card === false && current.retried.layout === true
      && current.retried.cards > 0 && current.hookShown.length === 1
      && current.hookShown[0].includes("board_snapshot refused while held")
      && current.after.flag === false && current.after.layout === true,
      JSON.stringify(current));

    ok("the late answers page raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  /* 창 서버는 `{ files, origin }`을 돌려준다 — 닫는 것은 그 파일 서버다(board-orbit의 실행기와
   * 같은 모양). `server.close()`를 부르면 시험을 다 돈 뒤 finally에서 넘어져 보고가 서지 않았다. */
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch();
  const lines = [];
  const report = (name, pass, detail = "") =>
    lines.push(`${pass ? "PASS" : "FAIL"}  ${name}${detail ? `  — ${detail}` : ""}`);
  try {
    await testBoardLive(browser, origin, report);
  } finally {
    await browser.close();
    files.close();
  }
  console.log(lines.join("\n"));
  console.log(`\n${lines.filter((line) => line.startsWith("PASS")).length}/${lines.length} passed`);
  process.exit(lines.some((line) => line.startsWith("FAIL")) ? 1 : 0);
}
