/* t-4048 · Fable 적대 검증 — 에이전트 관계 보드의 실제 통합.
 *
 * 구현자의 하네스와 겹치지 않는 독립 사례들이다. 픽스처는 `__PANES__`·
 * `__LEDGER__`·`__COLUMNS__`·`__OVERLAYS__` 네 문으로만 들어간다 — 프로덕션과
 * 같은 길(`boardCards()` → `places` 페리 → `board_snapshot`)을 타야 `run`/
 * `task_id` 가 정말 살아남는지 잰다. 카드에 `run` 을 직접 박는 픽스처는
 * 프로덕션의 Rust 왕복이 그 키를 버리므로 거짓 초록이다.
 *
 *   node ui/tests/agent-relations-adversarial.mjs
 */
import { mkdir } from "node:fs/promises";
import { pathToFileURL } from "node:url";
import { resolve } from "node:path";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";
import { installBoardWaits } from "./board-waits.mjs";

/* 한 판. 같은 제목 「구현」이 run-1 에 둘(t-A, t-B), run-2 에 하나(t-D);
 * 같은 쌍(202→203)에 spawned+dependency+mail 셋; 좌석 없는 worker: 행 하나
 * (체크아웃 미보고); 원장이 모르는 손 판(term:201); 레인 하나; 질문 카드 하나. */
export function relationsFixture() {
  // 페이지 안에서 평가되므로 모듈 상수는 여기 있어야 한다.
  const ROOT = "/repos/zerocode";
  const WT_A = "/repos/zerocode-wt/a";
  const WT_B = "/repos/zerocode-wt/b";
  const OTHER = "/repos/other";
  const now = Date.now();
  const ledgerRow = (over) => ({
    run: "run-1", worker: "", agent: "codex", state: "working", ledger: "active", hearing: "hook",
    hearing_at: now, checkout: "", task: "", task_id: "", reported: false, review: null,
    term: null, at: now - 60_000, ...over,
  });
  const card = (pane, heading, state, over = {}) => ({
    pane, heading, state, agent: "codex", project: ROOT, worktree: "main", task: "",
    you: "", said: "", ask: "", parent: "", ledger: "", unseen: false,
    changed_at: now - 30_000, at: now - 30_000, ...over,
  });
  projects = [
    { name: "zerocode", path: ROOT, worktrees: [
      { path: ROOT, branch: "main", base: "", is_main: true },
      { path: WT_A, branch: "wt/a", base: "main", is_main: false },
      { path: WT_B, branch: "wt/b", base: "main", is_main: false },
    ] },
    { name: "other", path: OTHER, worktrees: [{ path: OTHER, branch: "main", base: "", is_main: true }] },
  ];
  window.__PANES__ = [
    { term: 201, agent: "claude", state: "working", at: now - 10_000, state_started_at: now - 10_000, resumable: false },
    { term: 202, agent: "codex", state: "working", at: now - 5_000, state_started_at: now - 5_000, resumable: false },
    { term: 203, agent: "codex", state: "working", at: now - 4_000, state_started_at: now - 4_000, resumable: false, parent: 202 },
    { term: 206, agent: "codex", state: "working", at: now - 3_000, state_started_at: now - 3_000, resumable: false },
    { term: 207, agent: "claude", state: "needs-attention", at: now - 2_000, state_started_at: now - 2_000, resumable: false,
      ask: "기존 동작을 유지할까요?",
      ask_prompt: { questions: [{ question: "기존 동작을 유지할까요?", multi_select: false,
        options: [{ label: "유지", description: "" }, { label: "변경", description: "" }] }] } },
  ];
  window.__LEDGER__ = [
    ledgerRow({ worker: "w-1", checkout: WT_A, task: "구현", task_id: "t-A", term: 202 }),
    ledgerRow({ worker: "w-2", checkout: WT_B, task: "구현", task_id: "t-B", term: 203 }),
    // 좌석 없는 행: 체크아웃을 아직 보고하지 않았다.
    ledgerRow({ worker: "w-3", checkout: "", task: "검토", task_id: "t-C", term: null, agent: "claude" }),
    ledgerRow({ run: "run-2", worker: "w-5", checkout: ROOT, task: "구현", task_id: "t-D", term: 206 }),
    ledgerRow({ worker: "w-6", checkout: WT_A, task: "경계 조건", task_id: "t-E", term: 207, agent: "claude" }),
  ];
  window.__COLUMNS__ = [
    { bucket: "attention", cards: [card("term:207", "경계 조건", "needs-attention", {
      agent: "claude", worktree: "wt/a", task: "경계 조건", ask: "기존 동작을 유지할까요?",
      ask_prompt: window.__PANES__[4].ask_prompt,
    })] },
    { bucket: "working", cards: [
      card("term:201", "손으로 연 판", "working", { agent: "claude", you: "직접 시작한 작업" }),
      card("term:202", "구현", "working", { worktree: "wt/a", task: "구현", said: "세션 복구 경로를 고치는 중" }),
      card("sub:202:helper", "재현 조건 정리", "working", { parent: "term:202", worktree: "wt/a", at: 0, changed_at: 0 }),
      card("term:203", "구현", "working", { worktree: "wt/b", task: "구현", parent: "term:202" }),
      card("worker:w-3", "검토", "working", { agent: "claude", project: "", worktree: "" }),
      card("term:206", "구현", "working", { task: "구현" }),
      card("lane:l1", "레인 작업", "working", { project: OTHER, worktree: "main" }),
    ] },
  ];
  window.__OVERLAYS__ = {
    latest: "term:206",
    mail: [
      { from: "term:202", to: "term:203", count: 2, unread: 1, at: now - 1_000 },
      { from: "term:203", to: "term:202", count: 1, unread: 0, at: now - 900 },
    ],
    dependencies: [{ from: "term:202", to: "term:203", count: 1, verb: "blocks" }],
    merge: [],
  };
  paneActivities.set("term:202", [{ at: now - 5_000, activity: { verb: "edit", target: "src/auth/session.rs", phase: "started" } }]);
  agentBoardMode = "graph";
  agentGraphSelectedKey = null;
  agentGraphOverlayMode = "none";
  agentGraphFollowing = false;
  boardQuery = "";
  boardBroken = false;
  if (typeof activeTabId !== "undefined" && activeTabId === "board") dropTab("board");
  openBoard();
}

export async function testAgentRelations(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await installBoardWaits(page);
    await page.evaluate(relationsFixture);
    await page.evaluate(() => window.__BOARD_SETTLED__());

    /* C0 — 페리. run/task_id 가 places 를 타고 모델까지 오는가. */
    const ferry = await page.evaluate(() => {
      const view = document.querySelector("#board-view");
      const model = agentGraphModels.get(view);
      const place = (pane) => model?.entities.get(`agent:${pane}`)?.place ?? null;
      return {
        broken: boardBroken,
        agents: [...model.entities.keys()].filter((key) => key.startsWith("agent:")).sort(),
        runOf202: place("term:202")?.run ?? "", taskOf202: place("term:202")?.taskId ?? place("term:202")?.task_id ?? "",
        runOfWorker: place("worker:w-3")?.run ?? "", taskOfWorker: place("worker:w-3")?.taskId ?? place("worker:w-3")?.task_id ?? "",
        runOfHand: place("term:201")?.run ?? "", runOfLane: place("lane:l1")?.run ?? "",
        runOfSub: place("sub:202:helper")?.run ?? "",
        runCount: model.runCount,
      };
    });
    ok("C0 places ferry carries run and task id for seated, seatless and helper cards; hand pane and lane carry none",
      !ferry.broken && ferry.agents.length === 8 && ferry.runOf202 === "run-1" && ferry.taskOf202 === "t-A"
        && ferry.runOfWorker === "run-1" && ferry.taskOfWorker === "t-C" && ferry.runOfHand === "" && ferry.runOfLane === ""
        && ferry.runCount === 2, JSON.stringify(ferry));

    /* C1 — 같은 제목이 세 작업. 제목으로 합치면 여기서 빨갛다. */
    const titles = await page.evaluate(() => {
      const view = document.querySelector("#board-view");
      const model = agentGraphModels.get(view);
      const groups = taskBoardModel(model).groups;
      const named = groups.filter((group) => group.title === "구현");
      return { count: groups.length, implementations: named.length,
        members: named.map((group) => group.members.map((one) => one.key).sort().join("+")).sort() };
    });
    ok("C1 three tasks titled 구현 stay three groups; t-A keeps its helper and t-B is not folded into its parent's task",
      titles.implementations === 3
        && titles.members.includes("agent:sub:202:helper+agent:term:202")
        && titles.members.includes("agent:term:203") && titles.members.includes("agent:term:206"),
      JSON.stringify(titles));

    /* C10 — 오버레이만 바뀌는 판. 카드는 그대로, 메일 수만 는다. */
    const overlayOnly = await page.evaluate(async () => {
      const view = document.querySelector("#board-view");
      agentGraphSetOverlay(view, "mail");
      await window.__BOARD_SETTLED__();
      const before = view.querySelector(".agent-graph-overlay-label")?.textContent ?? "";
      const saidBefore = view.dataset.said;
      window.__OVERLAYS__.mail[0].count = 7;
      window.__OVERLAYS__.mail[0].unread = 5;
      await paintBoardView();
      await window.__BOARD_SETTLED__();
      const after = view.querySelector(".agent-graph-overlay-label")?.textContent ?? "";
      const badge = view.querySelector("[data-graph-key='agent:term:203'] .agent-graph-mail-badge")?.textContent ?? "";
      const saidMoved = view.dataset.said !== saidBefore;
      agentGraphSetOverlay(view, "none");
      await window.__BOARD_SETTLED__();
      return { before, after, badge, saidMoved };
    });
    ok("C10 an overlay-only change repaints the mail count and unread badge",
      overlayOnly.before !== overlayOnly.after && overlayOnly.after.includes("7") && overlayOnly.badge.includes("5"),
      JSON.stringify(overlayOnly));

    /* C9 — 초안 보존. 질문 카드에 답을 쓰는 동안 남의 카드가 움직인다. */
    const draft = await page.evaluate(async () => {
      const view = document.querySelector("#board-view");
      selectAgentGraphEntity(view, "agent:term:207", { focus: true });
      await window.__BOARD_SETTLED__();
      const field = view.querySelector(".board-ask-field");
      if (!field) return { field: false };
      field.focus();
      field.value = "작성 중";
      field.dispatchEvent(new Event("input", { bubbles: true }));
      // 한글 조합 중 — 조합이 끝나기 전에 판이 다시 그려진다.
      field.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true }));
      const working = window.__COLUMNS__.find((column) => column.bucket === "working").cards[1];
      working.said = "다른 에이전트가 말했다";
      working.at = Date.now();
      await paintBoardView();
      await window.__BOARD_SETTLED__();
      const again = view.querySelector(".board-ask-field");
      return { field: true, sameNode: again === field, value: again?.value ?? "",
        focused: document.activeElement === again, drafts: askDrafts.size };
    });
    ok("C9 an unrelated live update keeps the typed answer, its focus and one draft",
      draft.field && draft.value === "작성 중" && draft.focused && draft.drafts === 1, JSON.stringify(draft));

    /* C11 — hot 이 다른 run. 팔로우가 범위 밖으로 선택을 끌지 않는다. (구현 뒤 범위 API 로 보강) */
    const follow = await page.evaluate(async () => {
      const view = document.querySelector("#board-view");
      selectAgentGraphEntity(view, "agent:term:202", { focus: false });
      agentGraphFollowing = true;
      await paintBoardView(boardTab(), { force: true });
      await window.__BOARD_SETTLED__();
      const jumped = agentGraphSelectedKey;
      agentGraphFollowing = false;
      return { jumped };
    });
    ok("C11 follow moves to the hot agent only while it is on the screen", follow.jumped === "agent:term:206", JSON.stringify(follow));

    /* C4 — 낡은 범위/선택: 선택한 워커가 사라진다. 판은 부서지지 않는다. */
    const stale = await page.evaluate(async () => {
      const view = document.querySelector("#board-view");
      selectAgentGraphEntity(view, "agent:term:206", { focus: false });
      window.__LEDGER__ = window.__LEDGER__.filter((row) => row.worker !== "w-5");
      window.__PANES__ = window.__PANES__.filter((pane) => pane.term !== 206);
      const working = window.__COLUMNS__.find((column) => column.bucket === "working");
      working.cards = working.cards.filter((one) => one.pane !== "term:206");
      await paintBoardView(boardTab(), { force: true });
      await window.__BOARD_SETTLED__();
      const model = agentGraphModels.get(view);
      return { broken: boardBroken, selected: agentGraphSelectedKey, has206: model.entities.has("agent:term:206"),
        runCount: model.runCount };
    });
    ok("C4 a vanished selection falls back without breaking the board and the run count drops",
      !stale.broken && stale.selected !== "agent:term:206" && !stale.has206 && stale.runCount === 1, JSON.stringify(stale));

    /* C13 — 상한: 위상이 그대로인 200판에 노드 생성 0, 초안 표 유계. */
    const bounded = await page.evaluate(async () => {
      const view = document.querySelector("#board-view");
      const creations = agentGraphNodeCreations;
      const layouts = agentGraphLayoutRuns;
      const working = window.__COLUMNS__.find((column) => column.bucket === "working").cards[1];
      for (let round = 0; round < 200; round += 1) {
        working.said = `소식 ${round}`;
        working.at = Date.now() + round;
        await paintBoardView();
      }
      await window.__BOARD_SETTLED__();
      return { creations: agentGraphNodeCreations - creations, layouts: agentGraphLayoutRuns - layouts,
        drafts: askDrafts.size, nodes: view.querySelectorAll(".agent-graph-node").length };
    });
    ok("C13 two hundred content-only repaints create no node and run no layout", bounded.creations === 0 && bounded.layouts === 0,
      JSON.stringify(bounded));

    /* C12 — 폭·테마. 가로 넘침 없음. */
    await mkdir("output/playwright/agent-relations", { recursive: true });
    for (const width of [1440, 900, 560, 360]) {
      await page.setViewportSize({ width, height: 1000 });
      await page.evaluate(() => {
        const view = document.querySelector("#board-view");
        view.style.position = "fixed"; view.style.inset = "0"; view.style.width = "100vw";
        view.style.zIndex = "100"; view.style.height = "100vh";
      });
      await page.evaluate(() => window.__BOARD_SETTLED__());
      await page.screenshot({ path: `output/playwright/agent-relations/dark-${width}.png` });
      const size = await page.evaluate(() => {
        const surface = document.querySelector(".agent-graph-layout");
        const head = document.querySelector(".agent-graph-head");
        return { width: surface.clientWidth, scroll: surface.scrollWidth, head: head.clientWidth, headScroll: head.scrollWidth };
      });
      ok(`C12 relations view has no horizontal overflow at ${width}px`,
        size.scroll <= size.width + 1 && size.headScroll <= size.head + 1, JSON.stringify(size));
    }
    await page.setViewportSize({ width: 1440, height: 1000 });
    // `applyTheme()` reads the module's `theme`; an argument is ignored (shell-settings.js).
    const light = await page.evaluate(async () => {
      theme = "light"; applyTheme();
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      const view = document.querySelector("#board-view");
      return { theme: document.documentElement.dataset.theme ?? null,
        surface: getComputedStyle(view.querySelector(".agent-graph-surface")).backgroundColor,
        ink: getComputedStyle(view.querySelector(".agent-inspector-title")).color };
    });
    await page.screenshot({ path: "output/playwright/agent-relations/light-1440.png" });
    await page.evaluate(() => { theme = "dark"; applyTheme(); });
    ok("C12 the light treatment actually binds on the root and repaints the relations surface", light.theme === "light" && light.surface !== "rgb(9, 16, 21)", JSON.stringify(light));

    ok("agent relations raise no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}


/* 2단계 심층 사례 — 34fc579e 이후의 관계 UI 를 실제 DOM·포인터로 공격한다. */
export async function testAgentRelationsDeep(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await installBoardWaits(page);
    await page.evaluate(relationsFixture);
    await page.evaluate(() => window.__BOARD_SETTLED__());

    /* C6 — 같은 쌍의 병렬 관계: spawned·dependency·mail(양방향)·contains 가 각각 고를 수 있는 행이다. */
    const parallel = await page.evaluate(async () => {
      const view = document.querySelector("#board-view");
      selectAgentGraphEntity(view, "agent:term:203", { focus: false });
      await window.__BOARD_SETTLED__();
      const rows = [...view.querySelectorAll(".agent-relation-row")];
      const kinds = rows.map((row) => row.querySelector(".agent-relation-row-kind")?.textContent ?? "");
      const keys = rows.map((row) => row.dataset.relationKey);
      const details = [];
      for (const key of keys) {
        selectAgentGraphRelation(view, key);
        await window.__BOARD_SETTLED__();
        details.push([agentGraphSelectedEdgeKey, view.querySelector(".agent-inspector-title")?.textContent,
          view.querySelector(".agent-relation-explanation")?.textContent?.slice(0, 12)].join("|"));
      }
      return { kinds, keys, distinctKeys: new Set(keys).size, distinctDetails: new Set(details).size, details };
    });
    ok("C6 five parallel relations around one agent are five distinct selectable rows with distinct details",
      parallel.keys.length === 5 && parallel.distinctKeys === 5 && parallel.distinctDetails === 5
        && parallel.kinds.filter((kind) => kind === "메일").length === 2
        && parallel.kinds.includes("시작함") && parallel.kinds.includes("의존") && parallel.kinds.includes("품음"),
      JSON.stringify(parallel));

    /* C5 — 간선 정체성: 정렬상 앞서는 메일 쌍이 새로 생겨도 선택한 간선은 그대로다. */
    const identity = await page.evaluate(async () => {
      const view = document.querySelector("#board-view");
      selectAgentGraphRelation(view, "overlay:mail:agent:term:202>agent:term:203");
      await window.__BOARD_SETTLED__();
      const before = agentGraphSelectedEdgeKey;
      window.__OVERLAYS__.mail.unshift({ from: "term:201", to: "term:202", count: 1, unread: 1, at: Date.now() });
      await paintBoardView(boardTab(), { force: true });
      await window.__BOARD_SETTLED__();
      const litKeys = [...view.querySelectorAll(".agent-graph-edge.is-overlay.is-selected")].map((line) => line.closest("[data-graph-edge]")?.dataset.graphEdge ?? "");
      return { before, after: agentGraphSelectedEdgeKey, title: view.querySelector(".agent-inspector-title")?.textContent,
        endpoints: [...view.querySelectorAll(".agent-graph-node.is-relation-endpoint")].map((node) => node.dataset.graphKey).sort(), litKeys };
    });
    ok("C5 a mail pair sorting ahead of the selected one leaves the selected edge, its endpoints and its lit line in place",
      identity.before === identity.after && identity.after === "overlay:mail:agent:term:202>agent:term:203"
        && identity.endpoints.join() === "agent:term:202,agent:term:203", JSON.stringify(identity));

    /* C7 — 근거 진실성: 의존 상세는 과업 생성 시각·상태를 말하고, 상세 사실이 없으면 없다고 말한다. */
    const evidence = await page.evaluate(async () => {
      const view = document.querySelector("#board-view");
      selectAgentGraphRelation(view, "overlay:dependency:agent:term:202>agent:term:203");
      await window.__BOARD_SETTLED__();
      const withoutFacts = view.querySelector(".agent-relation-evidence")?.textContent ?? "";
      window.__OVERLAYS__.task_dependencies = [{ run: "run-1", task: "t-B", dependency: "t-A", task_state: "dispatched",
        dependency_state: "dispatched", task_created_ms: Date.now() - 90_000, from: "term:202", to: "term:203" }];
      await paintBoardView(boardTab(), { force: true });
      await window.__BOARD_SETTLED__();
      const withFacts = view.querySelector(".agent-relation-evidence")?.textContent ?? "";
      selectAgentGraphRelation(view, "overlay:mail:agent:term:202>agent:term:203");
      await window.__BOARD_SETTLED__();
      const mail = view.querySelector(".agent-relation-evidence")?.textContent ?? "";
      return { withoutFacts, withFacts, mail };
    });
    ok("C7 dependency evidence names the downstream task's creation time and both task states, never a completion it cannot know; mail evidence carries counts only",
      evidence.withoutFacts.includes("상세 상태가 없습니다") && !evidence.withoutFacts.includes("기록 시각")
        && evidence.withFacts.includes("t-A · 배차됨") && evidence.withFacts.includes("t-B · 배차됨") && evidence.withFacts.includes("후속 과업 생성")
        && !evidence.withFacts.includes("완료") && evidence.mail.includes("메시지 수") && !evidence.mail.includes("본문"),
      JSON.stringify(evidence));

    /* F2 는 백엔드 형상의 문제였다(454cd997·34fc579e 에서 red): 디스패치가 닫힌 워커는 current_cards 에 없어
     * task_dependencies.from 이 None 으로 왔고 UI 가 「보드에 없다」고 말했다. efd48662 가 행의 dispatch 를
     * run.dispatches 에서 되찾으므로 그 형상은 더 오지 않는다 — Rust 재현
     * relation_rows_keep_a_closed_attempts_identity_while_its_worker_is_still_summoned 이 지킨다. */

    /* C8 — 목적지: worker: 는 확인만, lane: 은 실행 열기, sub: 는 부모 판, 깨진 pane 은 조용히 비활성. */
    const destinations = await page.evaluate(async () => {
      const view = document.querySelector("#board-view");
      const read = async (key) => {
        selectAgentGraphEntity(view, key, { focus: false });
        await window.__BOARD_SETTLED__();
        const preview = view.querySelector(".agent-inspector-preview");
        const open = view.querySelector(".agent-inspector-open");
        return [preview.hidden, preview.disabled, preview.textContent.trim(), open.hidden, open.disabled, open.textContent.trim()].join("|");
      };
      const out = { worker: await read("agent:worker:w-3"), lane: await read("agent:lane:l1"), sub: await read("agent:sub:202:helper"), term: await read("agent:term:202") };
      const acked = acknowledgedPanes.has("worker:w-3");
      view.querySelector(".agent-inspector-preview").click();
      selectAgentGraphEntity(view, "agent:worker:w-3", { focus: false });
      await window.__BOARD_SETTLED__();
      view.querySelector(".agent-inspector-preview").click();
      out.workerAck = !acked && acknowledgedPanes.has("worker:w-3");
      out.broken = (() => { try { openBoardCard({ pane: "term:abc" }, "working"); revealAgentGraphCard({ pane: "sub:" }); return "no-throw"; } catch (error) { return String(error); } })();
      return out;
    });
    ok("C8 worker rows only acknowledge, lanes open the execution, helpers keep both doors, and a malformed pane id never throws",
      destinations.worker.startsWith("false|false|확인했어요|true")
        && destinations.lane.startsWith("true|") && destinations.lane.endsWith("|실행 열기") && destinations.lane.split("|")[3] === "false" && destinations.lane.split("|")[4] === "false"
        && destinations.sub.startsWith("false|false|미리보기|false") && destinations.term.startsWith("false|false|미리보기|false")
        && destinations.workerAck && destinations.broken === "no-throw",
      JSON.stringify(destinations));

    /* C3 — 재시도: 같은 [run, taskId] 에 끝난 시도와 현재 시도. 현재가 리드, 옛 시도는 「이전 시도」. */
    const retry = await page.evaluate(async () => {
      const view = document.querySelector("#board-view");
      const now = Date.now();
      window.__PANES__.push({ term: 205, agent: "codex", state: "done", at: now - 200_000, state_started_at: now - 200_000, resumable: false });
      window.__LEDGER__.push({ run: "run-1", worker: "w-old", agent: "codex", state: "working", ledger: "reclaimable", hearing: "hook",
        hearing_at: now, checkout: "/repos/zerocode-wt/a", task: "구현", task_id: "t-A", dispatch_id: "dp-old", dispatch_started_ms: now - 300_000,
        reported: true, retry_of: null, review: null, term: 205, at: now - 300_000 });
      const current = window.__LEDGER__.find((row) => row.worker === "w-1");
      current.dispatch_id = "dp-new"; current.dispatch_started_ms = now - 100_000; current.retry_of = "dp-old";
      window.__COLUMNS__.push({ bucket: "done", cards: [{ pane: "term:205", heading: "구현", state: "done", agent: "codex", project: "/repos/zerocode",
        worktree: "wt/a", task: "구현", you: "", said: "첫 시도 결과", ask: "", parent: "", ledger: "reclaimable", unseen: false, changed_at: now - 200_000, at: now - 200_000 }] });
      await paintBoardView(boardTab(), { force: true });
      await window.__BOARD_SETTLED__();
      const tasks = agentGraphTasksFor(view, agentGraphFullModel(view));
      const group = tasks.groups.find((one) => one.key === JSON.stringify(["run-1", "t-A"]));
      const chip = view.querySelector("[data-graph-key='agent:term:205'] .agent-graph-previous-attempt")?.textContent ?? "";
      const currentChip = view.querySelector("[data-graph-key='agent:term:202'] .agent-graph-previous-attempt");
      setAgentGraphScope(view, JSON.stringify(["run-1", "t-A"]));
      await window.__BOARD_SETTLED__();
      const scoped = agentGraphModels.get(view).agents.map((entry) => entry.key).sort();
      const option = [...view.querySelector(".agent-graph-scope").options].find((one) => one.value === JSON.stringify(["run-1", "t-A"]))?.textContent ?? "";
      selectAgentGraphEntity(view, "agent:term:202", { focus: false });
      await window.__BOARD_SETTLED__();
      const details = (() => { agentGraphInspectorTab = "details"; paintAgentGraph(view, agentGraphFullModel(view)); return view.querySelector(".agent-inspector-pane.is-details")?.textContent ?? ""; })();
      agentGraphInspectorTab = "relations";
      return { members: group?.members.map((one) => one.key).sort(), current: group?.currentMembers.map((one) => one.key).sort(),
        previous: group?.previousMembers.map((one) => one.key), bucket: group?.bucket, lead: group?.lead?.key, chip, currentChip: Boolean(currentChip), scoped, option, details };
    });
    ok("C3 a retried task groups both attempts, leads with the open attempt, marks the ended one as a previous attempt, scopes to all three and names the replaced dispatch",
      retry.members?.join() === "agent:sub:202:helper,agent:term:202,agent:term:205" && retry.current?.join() === "agent:sub:202:helper,agent:term:202"
        && retry.previous?.join() === "agent:term:205" && retry.bucket === "working" && retry.lead === "agent:term:202" && retry.chip === "이전 시도" && !retry.currentChip
        && retry.scoped.join() === "agent:sub:202:helper,agent:term:202,agent:term:205" && retry.option.includes("3")
        && retry.details.includes("dp-old") && retry.details.includes("dp-new"),
      JSON.stringify(retry));

    /* F1 의 UI 결과(닫힌 시도 행의 task_id 가 비어 과업 묶음에서 이탈)도 같은 Rust 재현이 지킨다. */

    /* C2 — 체크아웃 지연: 좌석 없는 워커의 checkout 이 뒤늦게 온다. 범위·선택·묶음 키가 살아남는다. */
    const late = await page.evaluate(async () => {
      const view = document.querySelector("#board-view");
      setAgentGraphScope(view, JSON.stringify(["run-1", "t-C"]));
      selectAgentGraphEntity(view, "agent:worker:w-3", { focus: false });
      await window.__BOARD_SETTLED__();
      const before = { scope: agentGraphScopeKey, selected: agentGraphSelectedKey, node: view.querySelector("[data-graph-key='agent:worker:w-3']") };
      window.__LEDGER__.find((row) => row.worker === "w-3").checkout = "/repos/zerocode-wt/b";
      await paintBoardView(boardTab(), { force: true });
      await window.__BOARD_SETTLED__();
      const after = { scope: agentGraphScopeKey, selected: agentGraphSelectedKey, sameNode: before.node === view.querySelector("[data-graph-key='agent:worker:w-3']"),
        workspace: agentGraphFullModel(view).entities.get("agent:worker:w-3")?.workspace.path, shown: agentGraphModels.get(view).agents.length,
        label: view.querySelector(".agent-graph-scope").selectedOptions[0]?.textContent, broken: boardBroken };
      return { before: { scope: before.scope, selected: before.selected }, after };
    });
    ok("C2 a checkout arriving later keeps the task scope, the selection and the node while moving the card to its real workspace",
      late.before.scope === late.after.scope && late.after.selected === "agent:worker:w-3" && late.after.sameNode
        && late.after.workspace === "/repos/zerocode-wt/b" && late.after.shown === 1 && !late.after.broken, JSON.stringify(late));

    /* C11' — 범위 안에서 hot 이 다른 run: 팔로우가 선택을 범위 밖으로 끌지 않는다. */
    const scopedFollow = await page.evaluate(async () => {
      const view = document.querySelector("#board-view");
      setAgentGraphScope(view, JSON.stringify(["run-1", "t-A"]));
      selectAgentGraphEntity(view, "agent:term:202", { focus: false });
      agentGraphFollowing = true;
      await paintBoardView(boardTab(), { force: true });
      await window.__BOARD_SETTLED__();
      const result = { selected: agentGraphSelectedKey, scope: agentGraphScopeKey };
      agentGraphFollowing = false;
      return result;
    });
    ok("C11' follow keeps the selection inside the chosen task when the hottest agent belongs to another run",
      scopedFollow.selected === "agent:term:202" && scopedFollow.scope === JSON.stringify(["run-1", "t-A"]), JSON.stringify(scopedFollow));

    /* C4' — 낡은 범위: 범위의 실행이 전부 사라지면 무엇을 말하는가. */
    const staleScope = await page.evaluate(async () => {
      const view = document.querySelector("#board-view");
      setAgentGraphScope(view, JSON.stringify(["run-2", "t-D"]));
      await window.__BOARD_SETTLED__();
      window.__LEDGER__ = window.__LEDGER__.filter((row) => row.worker !== "w-5");
      window.__PANES__ = window.__PANES__.filter((pane) => pane.term !== 206);
      for (const column of window.__COLUMNS__) column.cards = column.cards.filter((one) => one.pane !== "term:206");
      await paintBoardView(boardTab(), { force: true });
      await window.__BOARD_SETTLED__();
      const empty = view.querySelector(".agent-graph-empty");
      return { scope: agentGraphScopeKey, shown: agentGraphModels.get(view).agents.length, full: agentGraphFullModel(view).agents.length,
        emptyShown: empty && !empty.hidden, emptyText: empty?.textContent ?? "", count: view.querySelector(".agent-graph-scope-count")?.textContent ?? "",
        option: view.querySelector(".agent-graph-scope").selectedOptions[0]?.textContent ?? "", broken: boardBroken };
    });
    ok("C4' a scope whose executions all ended does not show the inventory-empty sentence while seven agents exist",
      !staleScope.broken && !(staleScope.emptyShown && staleScope.full > 0 && staleScope.emptyText.includes("표시할 활성 에이전트가 없습니다")),
      JSON.stringify(staleScope));

    /* C16 — 작업 화면 왕복: 관계 범위 A 를 둔 채 작업 화면에서 B 의 참여자를 고르고 관계로 돌아온다.
     * 돌아온 선택은 B 여야 하고 범위는 B 를 보여 줘야 한다 — 옛 범위 A 가 선택을 끌어가면 안 된다. */
    const roundTrip = await page.evaluate(async () => {
      const view = document.querySelector("#board-view");
      setAgentGraphScope(view, JSON.stringify(["run-1", "t-A"]));
      await window.__BOARD_SETTLED__();
      const boardView = view;
      for (const button of boardView.querySelectorAll("[data-board-mode]")) if (button.dataset.boardMode === "tasks") button.click();
      await window.__BOARD_SETTLED__();
      const rowB = [...boardView.querySelectorAll(".task-board-row")].find((row) => row.dataset.taskKey === JSON.stringify(["run-1", "t-B"]));
      rowB?.querySelector("button, .task-board-action")?.click();
      selectTaskBoardMember(boardView, "agent:term:203");
      await window.__BOARD_SETTLED__();
      const inTasks = { selected: agentGraphSelectedKey, mode: agentBoardMode };
      for (const button of boardView.querySelectorAll("[data-board-mode]")) if (button.dataset.boardMode === "graph") button.click();
      await window.__BOARD_SETTLED__();
      const drawn = agentGraphModels.get(boardView);
      return { rowB: Boolean(rowB), inTasks, back: { selected: agentGraphSelectedKey, scope: agentGraphScopeKey,
        shown: drawn.agents.map((entry) => entry.key).sort(), selectedDrawn: drawn.entities.has(agentGraphSelectedKey),
        label: boardView.querySelector(".agent-graph-scope").selectedOptions[0]?.textContent ?? "" } };
    });
    ok("C16 picking another task's member on the task board and returning to relations keeps that selection visible instead of snapping back to the old scope",
      roundTrip.inTasks.selected === "agent:term:203" && roundTrip.back.selected === "agent:term:203" && roundTrip.back.selectedDrawn,
      JSON.stringify(roundTrip));

    /* C9' — 조합 중 선택한 판 자신의 활동(ticker)이 움직인다: 카드가 다시 지어져도 입력 노드·값·조합이 산다. */
    const typingUnderActivity = await page.evaluate(async () => {
      const view = document.querySelector("#board-view");
      setAgentGraphScope(view, "");
      selectAgentGraphEntity(view, "agent:term:207", { focus: true });
      await window.__BOARD_SETTLED__();
      const field = view.querySelector(".board-ask-field");
      if (!field) return { field: false };
      field.focus();
      field.value = "조합 중";
      field.dispatchEvent(new Event("input", { bubbles: true }));
      field.dispatchEvent(new CompositionEvent("compositionstart", { data: "ㅈ", bubbles: true }));
      field.setSelectionRange(1, 3);
      paneActivities.set("term:207", [{ at: Date.now(), activity: { verb: "read", target: "docs/policy.md", phase: "started" } }]);
      window.__COLUMNS__[0].cards[0].said = "정책 문서를 읽는 중";
      window.__COLUMNS__[0].cards[0].at = Date.now();
      await paintBoardView(boardTab(), { force: true });
      await window.__BOARD_SETTLED__();
      const again = view.querySelector(".board-ask-field");
      const result = { field: true, sameNode: again === field, value: again?.value, focused: document.activeElement === again,
        selection: [again?.selectionStart, again?.selectionEnd].join("-"), drafts: askDrafts.size };
      field.dispatchEvent(new CompositionEvent("compositionend", { data: "ㅈ", bubbles: true }));
      return result;
    });
    ok("C9' the selected pane's own activity beat mid-composition keeps the same input node, its value, focus and selection",
      typingUnderActivity.field && typingUnderActivity.sameNode && typingUnderActivity.value === "조합 중" && typingUnderActivity.focused
        && typingUnderActivity.selection === "1-3" && typingUnderActivity.drafts === 1, JSON.stringify(typingUnderActivity));

    /* 포인터 — 넓은 판(side 티어, 서랍 없음)에서 간선 hit 층과 카드를 실제 마우스로 누른다.
     * 좁은 판에서는 hit 층이 꺼지고 서랍의 배경막이 캔버스를 덮으므로 거기서는 잴 것이 없다. */
    await page.setViewportSize({ width: 1800, height: 1000 });
    await page.evaluate(async () => {
      const view = document.querySelector("#board-view");
      // 하네스의 무대에서는 터미널 행이 보드 위에 겹친다(task-board.mjs 의 그림도 같은 손을 쓴다).
      view.style.position = "fixed"; view.style.inset = "0"; view.style.width = "100vw"; view.style.height = "100vh"; view.style.zIndex = "100";
      setAgentGraphScope(view, "");
      agentGraphSelectedEdgeKey = null;
      selectAgentGraphEntity(view, "agent:term:202", { focus: false });
      await window.__BOARD_SETTLED__();
      view.querySelector(".agent-graph-scroll").scrollTop = 0;
      await new Promise((done) => requestAnimationFrame(done));
    });
    const spots = await page.evaluate(() => {
      const view = document.querySelector("#board-view");
      const at = (key) => view.querySelector(`[data-graph-key='${key}']`).getBoundingClientRect();
      const a = at("agent:term:202"), b = at("agent:term:203");
      const points = { leftEdge: [b.left + 4, b.top + b.height / 2], center: [b.left + b.width / 2, b.top + b.height / 2],
        gutter: [(a.right + b.left) / 2, a.top + Math.min(a.height, b.height) / 2] };
      const under = {};
      for (const [name, [x, y]] of Object.entries(points)) {
        const el = document.elementFromPoint(x, y);
        under[name] = { x, y, cls: el?.getAttribute("class") ?? "", key: el?.closest("[data-graph-key]")?.dataset.graphKey ?? null };
      }
      return { under, tier: view.classList.contains("is-graph-list"), drawer: getComputedStyle(view.querySelector(".agent-graph-scrim")).display, hits: view.querySelectorAll(".agent-graph-edge-hit").length };
    });
    await page.mouse.click(spots.under.leftEdge.x, spots.under.leftEdge.y);
    const edgeClick = await page.evaluate(() => ({ selected: agentGraphSelectedKey, edge: agentGraphSelectedEdgeKey }));
    ok("pointer a click 4px inside a card's left border selects the card, not the edge whose 16px hit stroke ends there",
      !spots.tier && spots.drawer === "none" && edgeClick.selected === "agent:term:203" && edgeClick.edge === null,
      JSON.stringify({ spots, edgeClick }));
    await page.evaluate(async () => { selectAgentGraphEntity(document.querySelector("#board-view"), "agent:term:202", { focus: false }); await window.__BOARD_SETTLED__(); });
    await page.mouse.click(spots.under.center.x, spots.under.center.y);
    const centerClick = await page.evaluate(() => ({ selected: agentGraphSelectedKey, edge: agentGraphSelectedEdgeKey }));
    ok("pointer a click in the middle of a card selects the card", centerClick.selected === "agent:term:203" && centerClick.edge === null, JSON.stringify(centerClick));
    await page.evaluate(async () => { agentGraphSelectedEdgeKey = null; selectAgentGraphEntity(document.querySelector("#board-view"), "agent:term:202", { focus: false }); await window.__BOARD_SETTLED__(); });
    await page.mouse.click(spots.under.gutter.x, spots.under.gutter.y);
    const gutterClick = await page.evaluate(() => ({ selected: agentGraphSelectedKey, edge: agentGraphSelectedEdgeKey,
      title: document.querySelector("#board-view .agent-inspector-title")?.textContent }));
    ok("pointer a click on the gutter run of an edge selects that edge and opens its detail",
      gutterClick.edge !== null && gutterClick.title !== "구현", JSON.stringify({ gutter: spots.under.gutter, gutterClick }));
    await page.evaluate(() => { const view = document.querySelector("#board-view"); view.style.position = ""; view.style.inset = ""; view.style.width = ""; view.style.height = ""; view.style.zIndex = ""; });
    await page.setViewportSize({ width: 1280, height: 860 });

    /* 키보드 — Escape 는 서랍을 닫고, 관계 행은 Enter 로 고른다. */
    const keyboard = await page.evaluate(async () => {
      const view = document.querySelector("#board-view");
      selectAgentGraphEntity(view, "agent:term:203", { focus: true });
      await window.__BOARD_SETTLED__();
      const row = view.querySelector(".agent-relation-row");
      row.focus();
      row.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
      row.click();
      await window.__BOARD_SETTLED__();
      const picked = agentGraphSelectedEdgeKey;
      const tab = view.querySelector('[data-agent-inspector-tab="relations"]');
      tab.focus();
      tab.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
      const moved = view.querySelector('[data-agent-inspector-tab][aria-selected="true"]')?.dataset.agentInspectorTab;
      const nodes = view.querySelector(".agent-graph-nodes");
      const selectedNode = view.querySelector(".agent-graph-node.is-selected");
      selectedNode?.focus();
      nodes.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
      await window.__BOARD_SETTLED__();
      return { picked, moved, afterArrow: agentGraphSelectedKey, tabStops: [...view.querySelectorAll(".agent-graph-nodes [tabindex='0']")].length };
    });
    ok("keyboard relation rows pick with Enter, inspector tabs walk with arrows, and the tree still has one tab stop",
      keyboard.picked !== null && keyboard.moved === "activity" && keyboard.tabStops === 1, JSON.stringify(keyboard));

    ok("deep agent relations raise no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  let failures = 0;
  try {
    const report = (name, pass, detail = "") => {
      console.log(`${pass ? "PASS" : "FAIL"} ${name}${!pass && detail ? `\n${detail}` : ""}`);
      if (!pass) failures++;
    };
    await testAgentRelations(browser, origin, report);
    await testAgentRelationsDeep(browser, origin, report);
  } finally {
    await browser.close();
    files.close();
  }
  if (failures) process.exitCode = 1;
}
