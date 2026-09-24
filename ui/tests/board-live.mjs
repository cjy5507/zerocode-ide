/* t-7288 · 실시간 조율 지도의 회귀.
 *
 * 픽스처는 프로덕션과 같은 네 문으로만 들어간다 — `__PANES__`·`__LEDGER__`·
 * `__COLUMNS__`·`__OVERLAYS__`. 그래야 `run`/`task_id`/`dispatch_id`가 Rust
 * 왕복을 지나 `places`까지 살아남는지가 실제로 재어진다(카드에 값을 직접 박는
 * 픽스처는 그 왕복이 키를 버리므로 거짓 초록이다).
 *
 * 여기서 고정하는 계약 넷:
 *   ① 실시간은 **고르는** 그림이다 — 작업 목록이 보드의 기본값이다.
 *   ② 맥박은 **실제로 기록된 사건** 하나에 한 번이다 — 같은 판을 두 번 읽어도,
 *      숨겼다 돌아와도, 옛 snapshot이 늦게 와도 수가 늘지 않는다.
 *   ③ 없는 사실은 **없다고 말한다** — 배달 상태·답장·소환자·보기 밖 끝점 수.
 *   ④ 보는 일은 아무것도 **소비하지 않는다** — 메일 확인도, 모델 호출도 없다.
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

export async function testBoardLive(browser, origin, ok) {
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
    ok("the graph without the live map is the graph it was", await page.evaluate(() => {
      const view = document.querySelector("#board-view");
      return !view.classList.contains("is-live-map")
        && view.querySelectorAll(".agent-graph-edge.is-live-relation").length === 0
        && view.querySelectorAll("[data-live-beat]").length === 0
        && [...view.querySelectorAll(".agent-graph-wait")].every((chip) => chip.textContent === "");
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
      view.querySelector('[data-board-mode="tasks"]').click();
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
      const withMap = { rows: rows(), live: view.classList.contains("is-live-map"),
        beats: view.querySelectorAll("[data-live-beat]").length,
        waits: [...view.querySelectorAll(".agent-graph-wait")].filter((chip) => chip.textContent !== "").length,
        events: view.querySelectorAll(".agent-live-events").length };
      setAgentGraphLive(view, false);
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
      const without = rows();
      setAgentGraphLive(view, true);
      view.querySelector('[data-board-mode="graph"]').click();
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
      return withMap.rows === without && withMap.rows !== ""
        && !withMap.live && withMap.beats === 0 && withMap.waits === 0 && withMap.events === 0;
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

    const again = await page.evaluate(async () => {
      const before = agentGraphLiveRecentEvents().length;
      await paintBoardView(undefined, { force: true });
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
      return { before, after: agentGraphLiveRecentEvents().length };
    });
    ok("duplicate_snapshot_does_not_replay_pulses",
      again.before === again.after && again.after === 1, JSON.stringify(again));

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
     * 그 사실을 장부가 스스로 말한다. */
    const saturated = await page.evaluate(async () => {
      const now = window.__LIVE_NOW__;
      const T = now + 9_000;
      const step = async (id) => {
        window.__OVERLAYS__ = window.__LIVE_OVERLAYS__(now, { mail: [
          { from: "term:306", to: "term:301", count: 1, unread: 0, at: T, verb: "mail",
            last_message: { id, run: "run-1", from: "worker:w-parent", to: "run:run-1",
              kind: "status", created_ms: T } },
        ] });
        await paintBoardView(undefined, { force: true });
        await window.__BOARD_SETTLED__();
      };
      for (let at = 0; at < 20; at += 1) await step(`m-burst-${at}`);
      const coverage = agentGraphLiveCoverage();
      const before = agentGraphLiveRecentEvents().length;
      await step("m-burst-0");
      return { coverage, before, after: agentGraphLiveRecentEvents().length };
    });
    /* 상한을 넘긴 관계에서 옛 사건이 다시 오면 맥박이 될 수 있다 — 그것이 이
     * 설계가 **인정하는 한계**다. 시험이 고정하는 것은 「그런 일이 없다」가
     * 아니라 「그런 일이 있을 수 있다고 장부가 말한다」이다. 한 이름표로
     * 누락과 재생을 동시에 풀었다는 주장은 여기 없다. */
    ok("a_saturated_relation_says_its_dedupe_coverage_broke_instead_of_claiming_it",
      saturated.coverage.truncated > 0 && saturated.coverage.complete === false,
      JSON.stringify(saturated));
    ok("past_the_same_timestamp_limit_the_ledger_admits_a_replay_is_possible",
      saturated.after >= saturated.before, JSON.stringify(saturated));
    ok("the_inspector_repeats_that_limit_in_words", await page.evaluate(() => {
      const view = document.querySelector("#board-view");
      selectAgentGraphEntity(view, "agent:term:301");
      const notes = [...view.querySelectorAll(".agent-live-events-note")]
        .map((node) => node.textContent).join(" ");
      return notes.includes(t("board.live.coverageTruncated", "중복 제거 범위가 끊긴 관계 {{count}}개 — 그 구간의 사건은 표시되지 않을 수 있습니다.",
        { count: agentGraphLiveCoverage().truncated }));
    }));

    /* 처음 보는 관계와 기억을 잃은 관계는 서로 다른 답을 받는다. 앞의 것은
     * 실제로 처음 보는 사건이고, 뒤의 것은 조용히 다시 맞춘다 — 둘을 한
     * 이름표로 가릴 수는 없으므로 장부가 그 둘을 따로 적는다. */
    ok("a_relation_evicted_by_the_cap_resyncs_quietly_instead_of_pulsing",
      await page.evaluate(() => {
        const now = window.__LIVE_NOW__;
        const overlays = (id, from) => ({ columns: [], overlays: { mail: [
          { from, to: "term:301", count: 1, unread: 0, at: now + 30_000, verb: "mail",
            last_message: { id, run: "run-1", from: `worker:${from}`, to: "run:run-1",
              kind: "status", created_ms: now + 30_000 } },
        ] } });
        // 처음 보는 관계: 실제로 처음 보는 사건이다.
        const before = agentGraphLiveRecentEvents().length;
        agentGraphLiveObserve(overlays("m-fresh", "term:401"), new Map(), Date.now());
        const firstSight = agentGraphLiveRecentEvents().length;
        // 상한을 넘겨 그 관계를 밀어낸 뒤 다시 오면 조용히 다시 맞춘다.
        for (let at = 0; at < 300; at += 1) {
          agentGraphLiveObserve(overlays(`m-push-${at}`, `term:5${at}`), new Map(), Date.now());
        }
        const pushed = agentGraphLiveRecentEvents().length;
        agentGraphLiveObserve(overlays("m-fresh-2", "term:401"), new Map(), Date.now());
        return { grew: firstSight === before + 1,
          quiet: agentGraphLiveRecentEvents().length === pushed,
          coverage: agentGraphLiveCoverage() };
      }).then((seen) => seen.grew && seen.quiet && seen.coverage.dropped > 0));

    /* out-of-order: 옛 stamp 는 상태를 과거로 돌리지 않는다. */
    const reordered = await page.evaluate(async () => {
      const now = window.__LIVE_NOW__;
      const before = agentGraphLiveRecentEvents().length;
      window.__OVERLAYS__ = window.__LIVE_OVERLAYS__(now, { mail: [
        { from: "term:306", to: "term:301", count: 1, unread: 0, at: now - 999_000, verb: "mail",
          last_message: { id: "m-ancient", run: "run-1", from: "worker:w-parent",
            to: "run:run-1", kind: "status", created_ms: now - 999_000 } },
      ] });
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
      return { before, after: agentGraphLiveRecentEvents().length };
    });
    ok("an_out_of_order_snapshot_does_not_move_state_backwards",
      reordered.before === reordered.after, JSON.stringify(reordered));

    /* 늦게 도착한 응답은 지금의 지도를 덮지 못한다. */
    ok("a_late_response_from_an_earlier_scope_is_dropped", await page.evaluate(async () => {
      const now = window.__LIVE_NOW__;
      const stale = agentGraphLiveGeneration();
      agentGraphLiveScopeMoved();
      const before = agentGraphLiveRecentEvents().length;
      agentGraphLiveObserve({ columns: [], overlays: window.__LIVE_OVERLAYS__(now, { mail: [
        { from: "term:307", to: "term:301", count: 1, unread: 0, at: now + 20_000, verb: "mail",
          last_message: { id: "m-stale", run: "run-1", from: "worker:w-child", to: "run:run-1",
            kind: "status", created_ms: now + 20_000 } },
      ] }) }, new Map(), Date.now(), stale);
      return agentGraphLiveRecentEvents().length === before;
    }));

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
        return notes.includes(agentGraphLiveUnknownWord("delivery"))
          && notes.includes(agentGraphLiveUnknownWord("reply"))
          // 미확인이 있는 선은 그 수를, 없는 선은 「미제공」을 — 「확인됨」은 없다.
          && labels.some((word) => word.endsWith(t("board.live.deliveryNotSaid", "배달 상태 미제공")))
          && labels.some((word) => word.endsWith(t("board.live.pending", "미확인 {{count}}", { count: 1 })))
          && labels.every((word) => !word.includes(t("board.verified", "검증됨")));
      }));

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
      await new Promise((done) => setTimeout(done, 1_200)); // 맥박이 다 꺼질 때까지
      await window.__BOARD_SETTLED__();
      const firstRow = view.querySelector(".agent-graph-node")?.getBoundingClientRect().top ?? 0;
      let mutations = 0;
      /* 재는 것은 **그림**이다 — 판의 머리와 툴바는 이 표면보다 오래된 손들이
       * 지나는 자리이고, 여기서 고정하려는 계약은 「아무 일도 없는 판에서
       * 렌더러가 DOM을 건드리지 않는다」이다. */
      const watch = new MutationObserver((records) => { mutations += records.length; });
      watch.observe(view.querySelector(".agent-graph-layout"),
        { subtree: true, childList: true, attributes: true, characterData: true });
      const before = [agentGraphNodeCreations, agentGraphLayoutRuns, agentGraphEdgeMeasureRuns];
      await paintBoardView(undefined, { force: false });
      await window.__BOARD_SETTLED__();
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      watch.disconnect();
      return { mutations, before, after: [agentGraphNodeCreations, agentGraphLayoutRuns, agentGraphEdgeMeasureRuns],
        firstRow, nowRow: view.querySelector(".agent-graph-node")?.getBoundingClientRect().top ?? 0,
        handles: agentGraphLiveHandles() };
    });
    ok("quiet_poll_has_zero_dom_mutations", quiet.mutations === 0, JSON.stringify(quiet));
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
     * 멈추고 얻은 0을 실시간의 0으로 파는 일을 막는 한 줄이다. */
    ok("the_elapsed_age_clock_still_ticks_while_the_rest_is_quiet",
      await page.evaluate(async () => {
        const view = document.querySelector("#board-view");
        const read = () => view.querySelector(
          '.agent-graph-node[data-graph-key="agent:worker:w-gone"] .agent-graph-wait-age')?.textContent;
        const before = read();
        const row = window.__LEDGER__.find((one) => one.worker === "w-gone");
        row.pane_missing_since_ms -= 10 * 60_000;
        await paintBoardView(undefined, { force: true });
        await window.__BOARD_SETTLED__();
        return typeof before === "string" && before !== "" && read() !== before;
      }));

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

    await mkdir("output/playwright/board-live", { recursive: true });
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

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const server = await createWindowServer();
  const browser = await chromium.launch();
  const lines = [];
  const report = (name, pass, detail = "") =>
    lines.push(`${pass ? "PASS" : "FAIL"}  ${name}${detail ? `  — ${detail}` : ""}`);
  try {
    await testBoardLive(browser, server.origin, report);
  } finally {
    await browser.close();
    await server.close();
  }
  console.log(lines.join("\n"));
  console.log(`\n${lines.filter((line) => line.startsWith("PASS")).length}/${lines.length} passed`);
  process.exit(lines.some((line) => line.startsWith("FAIL")) ? 1 : 0);
}
