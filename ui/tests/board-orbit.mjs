/* t-9444 · 관계 탭의 「행성계」 보기 — 회귀와 무게.
 *
 * 워크스페이스가 항성, 에이전트가 궤도를 도는 행성, 하위 에이전트가 위성,
 * 우편이 링크를 따라 흐르는 입자다(`ui/shell-board-orbit.js`). 픽스처는
 * 프로덕션과 같은 네 문으로만 들어간다 — `__PANES__`·`__LEDGER__`·
 * `__COLUMNS__`·`__OVERLAYS__` — 그래야 체크아웃이 원장 행을 타고 `places`까지
 * 와서 워크스페이스 넷으로 갈리는지가 실제로 재어진다.
 *
 * 여기서 고정하는 계약:
 *   ① 관계 탭에는 「행성계 | 카드」 토글이 있고, 저장이 없으면 행성계가 선다.
 *      고른 보기는 로컬 설정 한 키에 남아 다시 연 창에서도 선다.
 *   ② 수는 표 하나(`ORBIT`)에 있다 — 파일의 나머지에는 0·1·2 말고 수가 없다.
 *   ③ 보이지 않으면(작업 목록·카드 보기·숨은 판·숨은 문서) 행성계의 rAF는 0이다.
 *   ④ 움직임을 줄이라는 판에서는 정지 화면이다 — 상태가 바뀔 때만 한 장 다시 그린다.
 *   ⑤ 상태 적용은 행성계가 쓰는 사실이 바뀔 때만이다(갱신 호출 수로 단언).
 *   ⑥ 행성을 누르면 기존 선택 길로 인스펙터가 선다; 검색·범위·배율이 그대로 먹는다.
 *   ⑦ 행성계가 선 동안 접힌 카드 그림에는 아무것도 쓰지 않는다 — 카드로 돌아온 첫 판이
 *      그 사이의 갱신을 빠짐없이 그리고 맞춤은 한 번 돈다 (t-9532).
 *   ⑧ 실시간 지도는 카드 그림 위의 것이다 — 행성계에서 그 손잡이는 누를 수 없는 채로
 *      까닭을 말하고, 지도는 적지도 뛰지도 않는다; 카드로 돌아오면 그대로 선다 (t-9532).
 *
 *   node ui/tests/board-orbit.mjs                     기능 시험 + 무게 표(Chromium)
 *   node ui/tests/board-orbit.mjs --perf --engine webkit --json out.json
 *   node ui/tests/board-orbit.mjs --perf --dpr 2       2배 밀도(Chromium, CDP)
 */
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";
import { installBoardWaits } from "./board-waits.mjs";
import { median, quantile, webkitType } from "./coordinator-desk-perf.mjs";
import { frameBudgetHolds, loadNote, machineIsLoudNow } from "./machine-load.mjs";

const UI = resolve(dirname(fileURLToPath(import.meta.url)), "..");

/* 한 판: 워크스페이스 `workspaces`개, 에이전트 `agents`개(셋은 같은 워크스페이스의
 * 앞 행성을 부모로 드는 위성), 우편 링크 `links`개 중 `fresh`개가 방금 오간 것
 * (그 절반이 미확인). 페이지 안에서 통째로 도는 함수라 바깥 이름을 쓰지 않는다. */
export function orbitFixture({ workspaces = 4, agents = 12, links = 40, fresh = 20 } = {}) {
  const ROOT = "/repos/orbit";
  const trees = [ROOT, ...Array.from({ length: workspaces - 1 }, (_, at) => `/repos/orbit-wt/w${at + 1}`)];
  const now = Date.now();
  window.__ORBIT_NOW__ = now;
  projects = [{ name: "orbit", path: ROOT, worktrees: trees.map((path, at) => ({
    path, branch: at === 0 ? "main" : `wt/w${at}`, base: at === 0 ? "" : "main", is_main: at === 0,
  })) }];
  /* 상태의 차례: 작업 중이 가장 많고 확인 필요·대기·완료·실패가 다 선다. */
  const STATES = [
    ["working", "working"], ["working", "working"], ["attention", "needs-attention"],
    ["working", "working"], ["idle", "idle"], ["done", "done"], ["working", "working"],
    ["attention", "needs-attention"], ["working", "working"], ["done", "failed"],
    ["idle", "idle"], ["done", "done"],
  ];
  const TASKS = ["조율", "구현", "독립 검증", "문서", "리팩터", "성능 측정", "릴리즈 노트",
    "재현", "보안 검토", "마이그레이션", "시험 보강", "정리"];
  const MOONS = new Map([[5, 1], [10, 6], [11, 7]]);
  const terms = Array.from({ length: agents }, (_, at) => 401 + at);
  const treeOf = (at) => trees[at % trees.length];
  const branchOf = (at) => (at % trees.length === 0 ? "main" : `wt/w${at % trees.length}`);
  const parentOf = (at) => (MOONS.has(at) && MOONS.get(at) < agents ? terms[MOONS.get(at)] : null);

  window.__PANES__ = terms.map((term, at) => ({
    term, agent: "codex", state: STATES[at % STATES.length][1] === "needs-attention"
      ? "needs-attention" : STATES[at % STATES.length][1],
    at: now - 10_000, state_started_at: now - 10_000, resumable: false,
    ...(parentOf(at) ? { parent: parentOf(at) } : {}),
  }));
  window.__LEDGER__ = terms.map((term, at) => ({
    run: "run-o1", worker: `w-${term}`, agent: "codex", state: "working", ledger: "active",
    hearing: "pending", hearing_at: now - 120_000, checkout: treeOf(at), task: TASKS[at % TASKS.length],
    task_id: `t-o${at}`, reported: false, review: null, dispatch_id: `dp-o${at}`,
    dispatch_started_ms: now - 300_000, retry_of: null, term, at: now - 300_000, model: null,
    effort: null, pane: `%${at}`, asking: false, wall: null, quiet_at: null, pane_missing_since_ms: null,
  }));
  const columns = new Map([["attention", []], ["working", []], ["done", []], ["idle", []]]);
  terms.forEach((term, at) => {
    const [bucket, state] = STATES[at % STATES.length];
    columns.get(bucket).push({
      pane: `term:${term}`, heading: TASKS[at % TASKS.length], state, agent: "codex",
      project: ROOT, worktree: branchOf(at), task: TASKS[at % TASKS.length], you: "", said: "",
      ask: bucket === "attention" ? "진행할까요?" : "", parent: parentOf(at) ? `term:${parentOf(at)}` : "",
      ledger: "", unseen: false, changed_at: now - 30_000, at: now - 30_000,
    });
  });
  window.__COLUMNS__ = [...columns].map(([bucket, cards]) => ({ bucket, cards }));

  /* 링크: 서로 다른 쌍 `links`개(차이 1·6·11·4로 겹치지 않는다). 앞의 `fresh`개가
   * 방금 오간 것이고 그 짝수 번째가 미확인이다. */
  const offsets = [1, 6, 11, 4];
  const mail = [];
  for (let k = 0; k < links; k += 1) {
    const from = terms[k % agents];
    const to = terms[(k % agents + offsets[Math.floor(k / agents) % offsets.length]) % agents];
    const recent = k < fresh;
    const at = recent ? now - 20_000 - k * 1_000 : now - 7_200_000 - k * 1_000;
    mail.push({ from: `term:${from}`, to: `term:${to}`, count: 1 + (k % 3), unread: recent && k % 2 === 0 ? 1 : 0,
      at, verb: "mail", last_message: { id: `m-o-${k}`, run: "run-o1", from: `worker:w-${from}`,
        to: `worker:w-${to}`, kind: "status", created_ms: at } });
  }
  window.__OVERLAYS__ = { latest: null, mail, dependencies: [], task_dependencies: [], merge: [] };

  /* 최근 활동: 작업 중인 행성마다 다른 양 — 공전이 활동량을 따라 빨라진다. */
  paneActivities.clear();
  terms.forEach((term, at) => {
    if (STATES[at % STATES.length][0] !== "working") return;
    const busy = [12, 6, 3, 1, 9][at % 5];
    paneActivities.set(`term:${term}`, Array.from({ length: busy }, (_, n) => ({
      at: now - 2_000 - n * 4_000,
      activity: { verb: n % 2 ? "edit" : "read", target: `src/orbit/${n}.rs`, phase: "finished" },
    })));
  });

  agentBoardMode = "graph";
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

/* 행성계의 rAF만 센다 — 창의 다른 손(간선 측정·맞춤·터미널)이 거는 프레임은
 * 이 수에 들지 않는다. 콜백의 **이름**으로 가린다: 행성계의 박자는 제 이름을
 * 가진 최상위 함수 하나(`agentOrbitTick`)다. */
async function installOrbitCounters(page) {
  await page.evaluate(() => {
    if (window.__ORBIT_COUNTING__) return;
    window.__ORBIT_COUNTING__ = true;
    window.__ORBIT_RAFS__ = 0;
    const raf = window.requestAnimationFrame.bind(window);
    window.requestAnimationFrame = (run) => {
      if (run?.name === "agentOrbitTick") window.__ORBIT_RAFS__ += 1;
      return raf(run);
    };
    window.__ORBIT_WAIT__ = (ms) => new Promise((done) => setTimeout(done, ms));
    window.__ORBIT_FRAMES__ = (count = 2) => new Promise((done) => {
      let left = count;
      const step = () => (left-- <= 0 ? done() : raf(step));
      raf(step);
    });
    window.__ORBIT__ = () => (typeof agentOrbitHandles === "function" ? agentOrbitHandles() : null);
  });
}

const openOrbit = async (page, fixture = {}) => {
  await installBoardWaits(page);
  await installOrbitCounters(page);
  await page.evaluate(orbitFixture, fixture);
  await page.evaluate(() => window.__BOARD_SETTLED__());
};

const orbitView = () => document.querySelector("#board-view");

/* 한 구간 동안 행성계가 건 rAF와 그린 그림의 수. */
const countOver = (page, ms) => page.evaluate(async (wait) => {
  await window.__ORBIT_FRAMES__(2);
  const rafs = window.__ORBIT_RAFS__;
  const frames = window.__ORBIT__()?.frames ?? null;
  await window.__ORBIT_WAIT__(wait);
  return { rafs: window.__ORBIT_RAFS__ - rafs, frames: (window.__ORBIT__()?.frames ?? 0) - (frames ?? 0),
    handles: window.__ORBIT__() };
}, ms);

/* 다섯 묶음, 각자 제 판에서. 한 묶음이 넘어져도 나머지는 돈다 — 넘어진 묶음은
 * 제 이름의 FAIL 한 줄로 남는다. */
export async function testBoardOrbit(browser, origin, ok) {
  const parts = [
    ["table", () => testOrbitShape(ok)],
    ["view", () => testOrbitView(browser, origin, ok)],
    ["gates", () => testOrbitGates(browser, origin, ok)],
    ["stillness", () => testOrbitStillness(browser, origin, ok)],
    ["memory", () => testOrbitRemembered(browser, origin, ok)],
    ["cards", () => testOrbitLeavesTheCardsAlone(browser, origin, ok)],
    ["live", () => testOrbitLiveMap(browser, origin, ok)],
  ];
  for (const [name, part] of parts) {
    try {
      await part();
    } catch (error) {
      ok(`the orbit ${name} checks ran to their end`, false,
        String(error?.message ?? error).split("\n").slice(0, 2).join(" | "));
    }
  }
}

/* ② 표 하나. 파일을 읽어 `ORBIT` 표를 도려내고 나머지에 남은 수를 센다 — 0·1·2는
 * 셈의 문법(반, 한 바퀴의 두 배, 없음)이라 수로 치지 않는다. 색 리터럴은 어디에도
 * 없어야 한다: 색은 CSS 토큰이 든다. */
async function testOrbitShape(ok) {
  let source = "";
  try {
    source = readFileSync(resolve(UI, "shell-board-orbit.js"), "utf8");
  } catch {
    ok("orbit_part_exists", false, "ui/shell-board-orbit.js is missing");
    return;
  }
  ok("orbit_part_exists", true);
  const code = source.replace(/\/\*[\s\S]*?\*\//g, "").replace(/\/\/[^\n]*/g, "");
  const opens = code.indexOf("const ORBIT = Object.freeze({");
  const closes = opens < 0 ? -1 : code.indexOf("\n});", opens);
  ok("orbit_numbers_live_in_one_table", opens >= 0 && closes > opens, "no `const ORBIT = Object.freeze({` … `});` table");
  if (opens < 0 || closes < 0) return;
  const outside = code.slice(0, opens) + code.slice(closes + 4);
  const strings = outside.replace(/`(?:\\.|[^`\\])*`|"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'/g, '""');
  const numbers = [...strings.matchAll(/(?<![\w.$])(\d+(?:_\d+)*(?:\.\d+)?|\.\d+)(?![\w$])/g)]
    .map((hit) => hit[1])
    .filter((literal) => !["0", "1", "2"].includes(literal));
  ok("orbit_has_no_number_outside_its_table", numbers.length === 0,
    `literals outside ORBIT: ${[...new Set(numbers)].join(", ")}`);
  const colours = [...outside.matchAll(/#[0-9a-fA-F]{3,8}\b|\brgba?\(\s*\d/g)].map((hit) => hit[0]);
  ok("orbit_has_no_colour_literal", colours.length === 0, colours.join(", "));
}

/* ① ⑥ 행성계가 서고, 누르면 고른다. */
async function testOrbitView(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await openOrbit(page);
    const shape = await page.evaluate(() => {
      const view = document.querySelector("#board-view");
      const stage = view.querySelector(".agent-orbit");
      const canvas = stage?.querySelector("canvas.agent-orbit-canvas");
      const labels = [...(stage?.querySelectorAll("[data-orbit-key]") ?? [])];
      const toggle = [...view.querySelectorAll("[data-relations-view]")];
      return {
        choice: typeof agentOrbitChoice === "function" ? agentOrbitChoice() : null,
        toggle: toggle.map((button) => [button.dataset.relationsView, button.getAttribute("aria-pressed")]),
        stageShown: Boolean(stage) && !stage.hidden && stage.getBoundingClientRect().width > 0,
        cardsHidden: view.querySelector(".agent-graph-scroll")?.hidden === true,
        role: canvas?.getAttribute("role") ?? null,
        summary: canvas?.getAttribute("aria-label") ?? "",
        planets: labels.filter((label) => label.dataset.orbitKey.startsWith("agent:")).length,
        stars: labels.filter((label) => label.dataset.orbitKey.startsWith("workspace:")).length,
        buttons: labels.every((label) => label.tagName === "BUTTON" && label.getAttribute("aria-label")),
        handles: window.__ORBIT__(),
      };
    });
    ok("the relations tab opens on the orbit view by default", shape.choice === "orbit"
      && shape.stageShown && shape.cardsHidden
      && JSON.stringify(shape.toggle) === JSON.stringify([["orbit", "true"], ["cards", "false"]]),
      JSON.stringify(shape));
    ok("the orbit draws every agent as a planet and every workspace as a star",
      shape.planets === 12 && shape.stars === 4 && shape.handles?.bodies === 16 && shape.buttons,
      JSON.stringify(shape));
    ok("the canvas is an image that says the counts", shape.role === "img"
      && /12/.test(shape.summary) && /5/.test(shape.summary) && /2/.test(shape.summary), shape.summary);
    ok("mail rides the links as particles, brighter where unread",
      shape.handles?.links === 40 && shape.handles?.particles === 20 && shape.handles?.bright === 10,
      JSON.stringify(shape.handles));
    ok("sub-agents orbit their parent as moons", shape.handles?.moons === 3, JSON.stringify(shape.handles));

    const running = await countOver(page, 400);
    ok("a shown orbit moves", running.rafs > 0 && running.frames > 0, JSON.stringify(running));

    /* 클릭. 움직이는 과녁이라 사람의 손이 먼저 올라가면 궤도가 멈춘다(hover) —
     * Playwright의 손도 같은 길로 간다: 올리고, 멈춘 것을 누른다. */
    const key = "agent:term:404";
    await page.hover(`[data-orbit-key="${key}"]`, { force: true, timeout: 5_000 });
    await page.evaluate(() => window.__ORBIT_WAIT__(500));
    const held = await page.evaluate((wanted) => {
      const label = document.querySelector(`#board-view [data-orbit-key="${wanted}"]`);
      const before = label?.style.translate;
      return new Promise((done) => setTimeout(() => done({ paused: window.__ORBIT__()?.paused,
        still: label?.style.translate === before }), 200));
    }, key);
    ok("hovering a planet stills the orbit so it can be clicked", held.paused === true && held.still === true,
      JSON.stringify(held));
    await page.click(`[data-orbit-key="${key}"]`, { timeout: 5_000 });
    const picked = await page.evaluate((wanted) => {
      const view = document.querySelector("#board-view");
      const label = view.querySelector(`[data-orbit-key="${wanted}"]`);
      return {
        selected: agentGraphSelectedKey,
        pressed: label?.getAttribute("aria-pressed"),
        inspector: view.querySelector(".agent-inspector-title")?.textContent ?? "",
        focused: document.activeElement?.dataset?.orbitKey ?? null,
        paused: window.__ORBIT__()?.paused,
      };
    }, key);
    ok("clicking a planet selects that agent through the existing selection road",
      picked.selected === key && picked.pressed === "true" && picked.inspector.includes("문서"),
      JSON.stringify(picked));
    /* 누른 라벨은 초점을 든다 — 키보드로 온 사람의 과녁도 움직이지 않는다. */
    ok("a focused planet label holds the orbit still", picked.focused !== key || picked.paused === true,
      JSON.stringify(picked));

    /* ⑤ 같은 판을 다시 그려도, 프레임이 흘러도 상태 적용은 그대로다. 손은 먼저
     * 치운다 — 멈춘 궤도에서도 적용의 수는 같아야 하지만, 맥동을 재는 판은 도는 판이다. */
    await page.mouse.move(1, 1);
    const applies = await page.evaluate(async () => {
      const before = window.__ORBIT__().applied;
      for (let at = 0; at < 5; at += 1) {
        await paintBoardView(undefined, { force: true });
        await window.__BOARD_SETTLED__();
      }
      await window.__ORBIT_FRAMES__(30);
      const same = window.__ORBIT__().applied;
      const pulses = window.__ORBIT__().pulses;
      const card = window.__COLUMNS__.find((column) => column.bucket === "working").cards
        .find((one) => one.pane === "term:402");
      const working = window.__COLUMNS__.find((column) => column.bucket === "working");
      working.cards = working.cards.filter((one) => one !== card);
      window.__COLUMNS__.find((column) => column.bucket === "done").cards.push({ ...card, state: "done" });
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
      return { before, same, moved: window.__ORBIT__().applied, pulses, pulsed: window.__ORBIT__().pulses };
    });
    ok("the orbit applies state only when the projection moves",
      applies.same === applies.before && applies.moved === applies.before + 1, JSON.stringify(applies));
    ok("a turn that ends pulses once", applies.pulsed === applies.pulses + 1, JSON.stringify(applies));

    /* ⑥ 검색·범위·배율. */
    const lens = await page.evaluate(async () => {
      const view = document.querySelector("#board-view");
      boardQuery = "보안 검토";
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
      const dimmed = [...view.querySelectorAll('.agent-orbit [data-orbit-key^="agent:"]')]
        .filter((label) => label.classList.contains("is-search-dimmed")).length;
      const lit = view.querySelector('.agent-orbit [data-orbit-key="agent:term:409"]')
        ?.classList.contains("is-search-dimmed");
      boardQuery = "";
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
      const full = agentGraphFullModel(view);
      const group = agentGraphTasksFor(view, full).groups.find((one) => one.members.length > 0
        && one.members.length < full.agents.length);
      setAgentGraphScope(view, group.key);
      await window.__BOARD_SETTLED__();
      const scoped = [...view.querySelectorAll('.agent-orbit [data-orbit-key^="agent:"]')]
        .map((label) => label.dataset.orbitKey).sort();
      const members = group.members.map((entry) => entry.key).sort();
      setAgentGraphScope(view, "");
      await window.__BOARD_SETTLED__();
      const zoomBefore = window.__ORBIT__().scale;
      view.querySelector(".agent-graph-zoom-in").click();
      await window.__ORBIT_FRAMES__(3);
      const zoomed = { zoom: agentGraphZoom, scale: window.__ORBIT__().scale };
      view.querySelector(".agent-graph-fit").click();
      await window.__ORBIT_FRAMES__(3);
      return { dimmed, lit, scoped, members, zoomBefore, zoomed,
        fitted: { zoom: agentGraphZoom, scale: window.__ORBIT__().scale } };
    });
    ok("search dims the planets that do not match", lens.dimmed === 11 && lens.lit === false, JSON.stringify(lens));
    ok("a run scope draws only that scope's agents",
      lens.scoped.length > 0 && JSON.stringify(lens.scoped) === JSON.stringify(lens.members), JSON.stringify(lens));
    ok("the existing zoom controls zoom the orbit and fit resets it",
      lens.zoomed.zoom > 1 && lens.zoomed.scale > lens.zoomBefore && lens.fitted.zoom === 1
      && Math.abs(lens.fitted.scale - lens.zoomBefore) < 1e-6, JSON.stringify(lens));

    /* 테마: 잉크는 한 곳(MutationObserver)에서 한 번 다시 읽는다. */
    const ink = await page.evaluate(async () => {
      const before = window.__ORBIT__().inkReads;
      document.documentElement.dataset.theme = "light";
      await window.__ORBIT_FRAMES__(2);
      const light = window.__ORBIT__().inkReads;
      document.documentElement.dataset.unrelated = "x";
      await window.__ORBIT_FRAMES__(2);
      const unrelated = window.__ORBIT__().inkReads;
      delete document.documentElement.dataset.unrelated;
      delete document.documentElement.dataset.theme;
      await window.__ORBIT_FRAMES__(2);
      return { before, light, unrelated, dark: window.__ORBIT__().inkReads };
    });
    ok("a theme switch re-reads the orbit's ink once, from one observer",
      ink.light === ink.before + 1 && ink.unrelated === ink.light && ink.dark === ink.light + 1, JSON.stringify(ink));

    /* DOM 예산: 카드 보기 대비 +50 이하. */
    const dom = await page.evaluate(async () => {
      const view = document.querySelector("#board-view");
      const count = () => view.querySelectorAll("*").length;
      const orbit = count();
      view.querySelector('[data-relations-view="cards"]').click();
      await window.__BOARD_SETTLED__();
      const cards = count();
      view.querySelector('[data-relations-view="orbit"]').click();
      await window.__BOARD_SETTLED__();
      return { orbit, cards, delta: orbit - cards };
    });
    ok("the orbit adds at most fifty elements over the card view", dom.delta <= 50, JSON.stringify(dom));

    ok("the orbit page raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* ③ 보이지 않으면 rAF 0 — 작업 목록·카드 보기·숨은 판·숨은 문서, 그리고 초점 없는 창은 30 fps. */
async function testOrbitGates(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await openOrbit(page);
    const shown = await countOver(page, 300);
    ok("gate control: the shown orbit asks frames", shown.rafs > 0, JSON.stringify(shown));

    await page.evaluate(async () => {
      document.querySelector('#board-view [data-board-mode="tasks"]').click();
      await window.__BOARD_SETTLED__();
    });
    const tasks = await countOver(page, 400);
    ok("the task list stops the orbit's frames", tasks.rafs === 0 && tasks.frames === 0, JSON.stringify(tasks));

    await page.evaluate(async () => {
      document.querySelector('#board-view [data-board-mode="graph"]').click();
      await window.__BOARD_SETTLED__();
    });
    const back = await countOver(page, 300);
    ok("returning to the relations tab starts the orbit again", back.rafs > 0, JSON.stringify(back));

    await page.evaluate(async () => {
      document.querySelector('#board-view [data-relations-view="cards"]').click();
      await window.__BOARD_SETTLED__();
    });
    const cards = await countOver(page, 400);
    ok("the card view stops the orbit's frames", cards.rafs === 0 && cards.frames === 0, JSON.stringify(cards));
    await page.evaluate(async () => {
      document.querySelector('#board-view [data-relations-view="orbit"]').click();
      await window.__BOARD_SETTLED__();
    });

    await page.evaluate(() => { document.querySelector("#board-view").hidden = true; });
    const tabHidden = await countOver(page, 400);
    ok("a hidden board asks no orbit frames", tabHidden.rafs === 0 && tabHidden.frames === 0, JSON.stringify(tabHidden));
    await page.evaluate(async () => {
      document.querySelector("#board-view").hidden = false;
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
    });

    await page.evaluate(() => {
      Object.defineProperty(document, "hidden", { configurable: true, get: () => true });
      document.dispatchEvent(new Event("visibilitychange"));
    });
    const docHidden = await countOver(page, 400);
    ok("a hidden document asks no orbit frames", docHidden.rafs === 0 && docHidden.frames === 0, JSON.stringify(docHidden));
    await page.evaluate(() => {
      Object.defineProperty(document, "hidden", { configurable: true, get: () => false });
      document.dispatchEvent(new Event("visibilitychange"));
    });
    const woke = await countOver(page, 300);
    ok("the document coming back starts the orbit again", woke.rafs > 0, JSON.stringify(woke));

    /* 초점 없는 창: 그림은 30 fps 이하 — 박자는 돌되 그림을 건너뛴다. */
    const blurred = await page.evaluate(async () => {
      const held = document.hasFocus;
      document.hasFocus = () => false;
      try {
        await window.__ORBIT_FRAMES__(2);
        const frames = window.__ORBIT__().frames;
        const from = performance.now();
        await window.__ORBIT_WAIT__(1_000);
        const seconds = (performance.now() - from) / 1000;
        return { fps: (window.__ORBIT__().frames - frames) / seconds };
      } finally {
        document.hasFocus = held;
      }
    });
    ok("an unfocused window draws the orbit at most thirty times a second",
      blurred.fps > 0 && blurred.fps <= 31, JSON.stringify(blurred));
    ok("the gates page raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* ④ 움직임을 줄이라는 판: 기본은 카드, 행성계를 고르면 정지 화면 — 상태가 바뀔 때만 한 장. */
async function testOrbitStillness(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin, {
    before: (surface) => surface.emulateMedia({ reducedMotion: "reduce" }),
  });
  try {
    await openOrbit(page);
    const opened = await page.evaluate(() => ({
      choice: typeof agentOrbitChoice === "function" ? agentOrbitChoice() : null,
      stage: document.querySelector("#board-view .agent-orbit")?.hidden ?? null,
    }));
    ok("a reduced-motion person starts on the card view", opened.choice === "cards" && opened.stage === true,
      JSON.stringify(opened));
    await page.evaluate(async () => {
      document.querySelector('#board-view [data-relations-view="orbit"]').click();
      await window.__BOARD_SETTLED__();
    });
    const still = await countOver(page, 800);
    ok("reduced motion stands the orbit still: no frames while nothing changes",
      still.rafs === 0 && still.frames === 0 && (still.handles?.frames ?? 0) >= 1, JSON.stringify(still));
    const changed = await page.evaluate(async () => {
      const before = window.__ORBIT__().frames;
      const working = window.__COLUMNS__.find((column) => column.bucket === "working");
      const card = working.cards.find((one) => one.pane === "term:407");
      working.cards = working.cards.filter((one) => one !== card);
      window.__COLUMNS__.find((column) => column.bucket === "done").cards.push({ ...card, state: "done" });
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
      await window.__ORBIT_WAIT__(300);
      return { before, after: window.__ORBIT__().frames, rafs: window.__ORBIT_RAFS__ };
    });
    ok("reduced motion redraws exactly one still frame when the state moves",
      changed.after === changed.before + 1, JSON.stringify(changed));
    ok("the reduced-motion page raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* ① 고른 보기는 로컬 설정 한 키에 남고, 다시 연 창에서도 선다. */
async function testOrbitRemembered(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await openOrbit(page);
    const saved = await page.evaluate(async () => {
      document.querySelector('#board-view [data-relations-view="cards"]').click();
      await window.__BOARD_SETTLED__();
      const keys = Object.keys(localStorage).filter((key) => key.includes("relations"));
      return { keys, value: keys.map((key) => localStorage.getItem(key)),
        stage: document.querySelector("#board-view .agent-orbit")?.hidden,
        cards: document.querySelector("#board-view .agent-graph-scroll")?.hidden };
    });
    ok("choosing the card view is saved under one local key",
      saved.keys.length === 1 && saved.value[0] === "cards" && saved.stage === true && saved.cards === false,
      JSON.stringify(saved));
    await page.reload();
    await page.waitForFunction(() => typeof BOUND !== "undefined" && BOUND.size > 0);
    await openOrbit(page);
    const reopened = await page.evaluate(() => ({
      choice: typeof agentOrbitChoice === "function" ? agentOrbitChoice() : null,
      stage: document.querySelector("#board-view .agent-orbit")?.hidden,
      pressed: document.querySelector('#board-view [data-relations-view="cards"]')?.getAttribute("aria-pressed"),
    }));
    ok("a reopened window stands on the saved view", reopened.choice === "cards" && reopened.stage === true
      && reopened.pressed === "true", JSON.stringify(reopened));
    ok("the remembered page raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* ⑦ 행성계가 선 동안 접힌 카드 그림에는 아무것도 쓰지 않는다. 같은 판 세 번, 카드 하나가
 * 끝난 판, 새 에이전트가 온 판 — 카드 판(`.agent-graph-scroll`) 아래의 변이가 0이고, 카드
 * 노드를 짓지도 간선을 재지도 않는다. 보는 자리가 카드 판뿐인 것은 행성계가 제 캔버스와
 * 라벨을 프레임마다 쓰기 때문이다. 모델의 배치(`agentGraphLayoutRuns`)는 행성계도 읽는 모델
 * 안의 셈이라 수만 적는다.
 *
 * 그리고 카드로 돌아온 첫 판이 건너뛴 갱신을 한 번에 그린다: 노드가 모델과 같고(끝난 카드는
 * 끝난 옷, 새 에이전트는 제 노드), 간선은 새로 재도 같은 자리이며, 맞춤은 한 번 돈다. */
async function testOrbitLeavesTheCardsAlone(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    /* 무게를 재는 판과 같은 폭 — 하네스의 기본 폭에서 카드 판은 목록 티어(맞춤이 없다)다. */
    await page.setViewportSize({ width: 1440, height: 960 });
    await openOrbit(page);
    const folded = await page.evaluate(async () => {
      const view = document.querySelector("#board-view");
      let changes = 0;
      const watch = new MutationObserver((records) => { changes += records.length; });
      watch.observe(view.querySelector(".agent-graph-scroll"),
        { subtree: true, childList: true, attributes: true, characterData: true });
      const counts = () => [agentGraphNodeCreations, agentGraphEdgeMeasureRuns, agentGraphLayoutRuns];
      const before = counts();
      const paint = async () => {
        await paintBoardView(undefined, { force: true });
        await window.__BOARD_SETTLED__();
        await window.__ORBIT_FRAMES__(3);
      };
      for (let at = 0; at < 3; at += 1) await paint();
      const working = window.__COLUMNS__.find((column) => column.bucket === "working");
      const finished = working.cards.find((one) => one.pane === "term:402");
      working.cards = working.cards.filter((one) => one !== finished);
      window.__COLUMNS__.find((column) => column.bucket === "done").cards.push({ ...finished, state: "done" });
      await paint();
      const now = window.__ORBIT_NOW__;
      window.__PANES__.push({ term: 413, agent: "codex", state: "working", at: now - 5_000,
        state_started_at: now - 5_000, resumable: false });
      window.__LEDGER__.push({ ...window.__LEDGER__[0], worker: "w-413", task: "새 일", task_id: "t-o13",
        dispatch_id: "dp-o13", term: 413, pane: "%13" });
      working.cards.push({ ...working.cards[0], pane: "term:413", heading: "새 일", task: "새 일", parent: "" });
      await paint();
      changes += watch.takeRecords().length;
      watch.disconnect();
      const after = counts();
      return { orbit: agentOrbitShowing(view), paints: 5, changes, nodeCreations: after[0] - before[0],
        edgeMeasures: after[1] - before[1], layoutRuns: after[2] - before[2] };
    });
    ok("the orbit writes nothing into the folded card picture: no mutation, no card built, no edge measured",
      folded.orbit && folded.changes === 0 && folded.nodeCreations === 0 && folded.edgeMeasures === 0,
      JSON.stringify(folded));

    const back = await page.evaluate(async () => {
      const view = document.querySelector("#board-view");
      const fit = window.fitAgentGraph;
      let fits = 0;
      window.fitAgentGraph = function fitAgentGraph(target, options = {}) {
        if (target === view && !options.pass) fits += 1;
        return fit(target, options);
      };
      try {
        view.querySelector('[data-relations-view="cards"]').click();
        await window.__BOARD_SETTLED__();
        await window.__ORBIT_FRAMES__(4);
      } finally {
        window.fitAgentGraph = fit;
      }
      const drawn = () => [...view.querySelectorAll('.agent-graph-nodes .agent-graph-node[data-graph-key^="agent:"]')]
        .map((node) => node.dataset.graphKey).sort();
      const edges = () => [...view.querySelectorAll(".agent-graph-edges [data-graph-edge]")]
        .map((group) => `${group.dataset.graphEdge} ${group.querySelector("path")?.getAttribute("d")}`);
      const standing = edges();
      paintAgentGraphEdges(view);
      return {
        fits,
        nodes: drawn(),
        wanted: agentGraphModels.get(view).nodes.filter((entity) => entity.type === "agent")
          .map((entity) => entity.key).sort(),
        finished: view.querySelector('.agent-graph-node[data-graph-key="agent:term:402"]')
          ?.classList.contains("is-done") ?? null,
        arrived: drawn().includes("agent:term:413"),
        edges: standing.length,
        remeasuredInPlace: JSON.stringify(standing) === JSON.stringify(edges()),
      };
    });
    ok("the card view it returns to draws every update the orbit skipped, fits once, and its edges stand where a fresh measure puts them",
      back.fits === 1 && JSON.stringify(back.nodes) === JSON.stringify(back.wanted) && back.finished === true
      && back.arrived && back.edges > 0 && back.remeasuredInPlace, JSON.stringify(back));
    ok("the folded-cards page raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* ⑧ 실시간 지도는 카드 그림 위의 것이다. 카드에서 켠 지도를 행성계로 가져가면 손잡이는 눌린
 * 채로 누를 수 없고(`aria-disabled` — 초점과 손은 받아 팁이 까닭을 말한다), 눌러도 켜진 채다.
 * 지도는 그동안 사건을 적지도 박자를 얹지도 않고, 인스펙터도 지도의 줄을 싣지 않는다. 카드로
 * 돌아오면 지도가 그대로 서고 행성계에 있던 동안의 일은 조용한 기준선이 되며, 그 뒤의 새
 * 사건은 뛴다. */
async function testOrbitLiveMap(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await openOrbit(page);
    const read = () => page.evaluate(() => {
      const view = document.querySelector("#board-view");
      const live = view.querySelector(".agent-graph-live");
      return {
        orbit: agentOrbitShowing(view),
        on: agentGraphLiveOn(),
        shown: agentGraphLiveShown(),
        disabled: live.getAttribute("aria-disabled"),
        pressed: live.getAttribute("aria-pressed"),
        tip: live.dataset.tip,
        label: live.getAttribute("aria-label"),
        key: live.dataset.i18nTitle,
        cardsOnly: t("board.orbit.liveInCards", ""),
        title: t("board.live.title", "실시간 조율"),
        events: agentGraphLiveRecentEvents().length,
        beats: view.querySelectorAll("[data-live-beat]").length,
        handles: agentGraphLiveHandles(),
        inspectorEvents: view.querySelectorAll(".agent-inspector .agent-live-events").length,
        liveEdges: view.querySelectorAll(".agent-graph-edge.is-live-relation").length,
      };
    });
    /* 우편 링크 하나(401 → 402)에 새 메시지가 실린 판 — 원장이 새 id를 실어 온 그 모양이다. */
    const mailArrives = (id) => page.evaluate(async (id) => {
      const now = Date.now();
      const [first, ...rest] = window.__OVERLAYS__.mail;
      window.__OVERLAYS__.mail = [{ ...first, count: first.count + 1, unread: 1, at: now,
        last_message: { ...first.last_message, id, created_ms: now } }, ...rest];
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
    }, id);
    const choose = (want) => page.evaluate(async (want) => {
      document.querySelector(`#board-view [data-relations-view="${want}"]`).click();
      await window.__BOARD_SETTLED__();
      await window.__ORBIT_FRAMES__(2);
    }, want);

    await choose("cards");
    await page.evaluate(async () => {
      const view = document.querySelector("#board-view");
      agentGraphInspectorTab = "relations";
      selectAgentGraphEntity(view, "agent:term:401");
      view.querySelector(".agent-graph-live").click();
      await window.__BOARD_SETTLED__();
    });
    const cardsOn = await read();
    await choose("orbit");
    const orbitOn = await read();
    await page.evaluate(async () => {
      document.querySelector("#board-view .agent-graph-live").click();
      await window.__BOARD_SETTLED__();
    });
    const pressed = await read();
    await mailArrives("m-o-orbit");
    const orbitMail = await read();
    ok("in the orbit the live map's handle stays pressed, cannot be pressed, and says the map shows in the card view",
      cardsOn.on && cardsOn.disabled !== "true" && orbitOn.orbit && orbitOn.on && orbitOn.disabled === "true"
      && orbitOn.pressed === "true" && orbitOn.cardsOnly !== "" && orbitOn.tip === orbitOn.cardsOnly
      && orbitOn.label === orbitOn.cardsOnly && orbitOn.key === "board.orbit.liveInCards" && pressed.on,
      JSON.stringify({ cardsOn, orbitOn, pressedOn: pressed.on }));
    ok("in the orbit the live map records nothing, beats nothing, and the inspector carries none of its lines",
      !orbitOn.shown && orbitMail.events === cardsOn.events && orbitMail.beats === 0
      && orbitMail.handles.timers === 0 && orbitMail.handles.pulses === 0 && orbitMail.inspectorEvents === 0,
      JSON.stringify({ cardsOn: cardsOn.events, orbitMail }));

    await choose("cards");
    const returned = await read();
    /* 토글은 원장을 다시 읽지 않는다 — 돌아온 뒤 첫 읽기가 조용한 기준선이고(`liveBaselineDue`),
     * 그다음 판의 새 메시지가 사건이다. */
    await page.evaluate(async () => {
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
    });
    const baseline = await read();
    await mailArrives("m-o-cards");
    const cardsMail = await read();
    ok("back on the card view the live map stands as it was left and what came in the orbit stays a quiet baseline",
      returned.shown && returned.disabled !== "true" && returned.tip === returned.title
      && returned.key === "board.live.title" && returned.liveEdges > 0 && returned.inspectorEvents === 1
      && returned.events === cardsOn.events && returned.beats === 0
      && baseline.events === cardsOn.events && baseline.beats === 0, JSON.stringify({ returned, baseline }));
    ok("back on the card view a new event is recorded and beats",
      cardsMail.events === baseline.events + 1 && cardsMail.beats > 0, JSON.stringify(cardsMail));
    ok("the orbit live page raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* ---- 무게 ------------------------------------------------------------------
 *
 * 한 프레임의 값은 둘로 잰다(`a-frame-instrument-that-stops-at-apply…`의 교훈):
 * 행성계의 그리는 JS(박자 함수를 감싼 벽시계), 그리고 rAF가 실제로 돌아오는 간격 —
 * 같은 판에서 행성계를 숨긴 **정지 대조군**의 간격과 나란히. JS가 싸도 간격이
 * 벌어지면 그 몫은 스타일·레이아웃·합성이다. */
export async function measureBoardOrbit(page, { seconds = 3, fixture = {} } = {}) {
  await openOrbit(page, fixture);
  const product = await page.evaluate(() => typeof agentOrbitHandles === "function");
  const probe = (on) => page.evaluate(async ({ on, seconds }) => {
    const view = document.querySelector("#board-view");
    const want = on ? "orbit" : "cards";
    const button = view.querySelector(`[data-relations-view="${want}"]`);
    if (button && button.getAttribute("aria-pressed") !== "true") {
      button.click();
      await window.__BOARD_SETTLED__();
    }
    const elements = view.querySelectorAll("*").length;
    const times = [];
    const gaps = [];
    /* 그린 판만 잰다 — 박자는 120 Hz 화면에서 둘에 하나를 건너뛰고(초당 60장 상한),
     * 건너뛴 박자의 0.0 ms를 섞으면 중앙값이 그림의 값이 아니라 건너뜀의 값이 된다. */
    const draw = window.agentOrbitDraw;
    if (on && typeof draw === "function") {
      window.agentOrbitDraw = function agentOrbitDraw(...args) {
        const from = performance.now();
        const answer = draw(...args);
        times.push(performance.now() - from);
        return answer;
      };
    }
    let last = null;
    let running = true;
    const gap = (stamp) => {
      if (last !== null) gaps.push(stamp - last);
      last = stamp;
      if (running) requestAnimationFrame(gap);
    };
    requestAnimationFrame(gap);
    const frames = window.__ORBIT__()?.frames ?? 0;
    const from = performance.now();
    await window.__ORBIT_WAIT__(seconds * 1000);
    const elapsed = (performance.now() - from) / 1000;
    running = false;
    if (on && typeof draw === "function") window.agentOrbitDraw = draw;
    return { elements, times, gaps, fps: ((window.__ORBIT__()?.frames ?? 0) - frames) / elapsed,
      handles: window.__ORBIT__() };
  }, { on, seconds });
  const cards = await probe(false);
  const orbit = product ? await probe(true) : null;
  /* 숨은 판과 움직임을 줄인 판의 rAF — 설계가 0이라고 말한 수. */
  const hidden = product ? await page.evaluate(async () => {
    const view = document.querySelector("#board-view");
    view.hidden = true;
    await window.__ORBIT_FRAMES__(2);
    const rafs = window.__ORBIT_RAFS__;
    await window.__ORBIT_WAIT__(1_000);
    const asked = window.__ORBIT_RAFS__ - rafs;
    view.hidden = false;
    await paintBoardView(undefined, { force: true });
    await window.__BOARD_SETTLED__();
    return asked;
  }) : null;
  await page.emulateMedia({ reducedMotion: "reduce" });
  const reduced = product ? await page.evaluate(async () => {
    await window.__ORBIT_FRAMES__(2);
    const frames = window.__ORBIT__().frames;
    const rafs = window.__ORBIT_RAFS__;
    await window.__ORBIT_WAIT__(1_000);
    return { frames: window.__ORBIT__().frames - frames, rafs: window.__ORBIT_RAFS__ - rafs };
  }) : null;
  await page.emulateMedia({ reducedMotion: "no-preference" });
  const round = (value) => (value === null ? null : Number(value.toFixed(3)));
  return {
    product: product ? "this tree" : "baseline (no orbit)",
    viewport: await page.evaluate(() => ({ width: innerWidth, height: innerHeight, dpr: devicePixelRatio })),
    fixture: { workspaces: 4, agents: 12, links: 40, fresh: 20, ...fixture },
    cards: { elements: cards.elements, gapP50: round(median(cards.gaps)), gapP95: round(quantile(cards.gaps, 0.95)) },
    orbit: orbit ? {
      elements: orbit.elements,
      frameP50: round(median(orbit.times)), frameP95: round(quantile(orbit.times, 0.95)),
      frameMax: round(Math.max(...orbit.times)), frames: orbit.times.length,
      fps: round(orbit.fps), gapP50: round(median(orbit.gaps)), gapP95: round(quantile(orbit.gaps, 0.95)),
      handles: orbit.handles,
    } : null,
    domDelta: orbit ? orbit.elements - cards.elements : null,
    hiddenRafs: hidden,
    reduced,
    load: loadNote(),
    loud: machineIsLoudNow(),
  };
}

const ORBIT_FRAME_BUDGET_MS = 8;
/* 요소 예산은 브리핑의 판(워크스페이스 4·에이전트 12)의 것이다. 더 큰 판에서는 몸
 * 하나가 얹는 요소 수(버튼 + 이름 = 2)를 잰다 — 판이 커지면 늘어나는 것이 옳은 수다. */
const ORBIT_DOM_BUDGET = 50;
const ORBIT_ELEMENTS_PER_BODY = 2;
const orbitBriefFixture = (fixture) =>
  fixture.workspaces === 4 && fixture.agents === 12 && fixture.links === 40 && fixture.fresh === 20;

function orbitVerdict(measured) {
  if (!measured.orbit) return { holds: false, why: "no orbit in this product" };
  const bodies = measured.orbit.handles?.bodies ?? 0;
  const checks = {
    frameP95: frameBudgetHolds(measured.orbit.frameP95, ORBIT_FRAME_BUDGET_MS),
    hiddenRafs: measured.hiddenRafs === 0,
    reducedStill: measured.reduced?.frames === 0 && measured.reduced?.rafs === 0,
    domDelta: orbitBriefFixture(measured.fixture)
      ? measured.domDelta <= ORBIT_DOM_BUDGET
      : measured.domDelta <= ORBIT_ELEMENTS_PER_BODY * bodies,
  };
  return { holds: Object.values(checks).every(Boolean), checks };
}

function orbitTable(measured, engine) {
  const orbit = measured.orbit ?? {};
  return [
    `| engine | ${engine} | ${measured.viewport.width}×${measured.viewport.height} @${measured.viewport.dpr}x | ${measured.load} |`,
    `| fixture | ${JSON.stringify(measured.fixture)} | | |`,
    "|---|---|---|---|",
    `| frame JS p50 / p95 / max (ms) | ${orbit.frameP50} / ${orbit.frameP95} / ${orbit.frameMax} | budget ≤ ${ORBIT_FRAME_BUDGET_MS} | ${orbit.frames} frames |`,
    `| frames drawn / s | ${orbit.fps} | rAF gap p50/p95 orbit ${orbit.gapP50}/${orbit.gapP95} ms | still control ${measured.cards.gapP50}/${measured.cards.gapP95} ms |`,
    `| orbit rAF while the board is hidden (1 s) | ${measured.hiddenRafs} | must be 0 | |`,
    `| reduced motion frames / rAF (1 s) | ${measured.reduced?.frames} / ${measured.reduced?.rafs} | must be 0 / 0 | |`,
    `| elements card view → orbit view | ${measured.cards.elements} → ${orbit.elements} | delta ${measured.domDelta}`
      + ` (${orbitBriefFixture(measured.fixture) ? `≤ ${ORBIT_DOM_BUDGET}` : `≤ ${ORBIT_ELEMENTS_PER_BODY} × ${orbit.handles?.bodies} bodies`})`
      + ` | ${orbit.handles?.bodies} bodies |`,
  ].join("\n");
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const option = (name, fallback) => {
    const at = process.argv.indexOf(name);
    return at > 0 && process.argv[at + 1] ? process.argv[at + 1] : fallback;
  };
  const engine = option("--engine", "chromium");
  const perfOnly = process.argv.includes("--perf");
  const { files, origin } = await createWindowServer();
  const browserType = engine === "webkit" ? webkitType() : chromium;
  const browser = await browserType.launch();
  const lines = [];
  const report = (name, pass, detail = "") =>
    lines.push(`${pass ? "PASS" : "FAIL"}  ${name}${detail ? `  — ${detail}` : ""}`);
  let measured = null;
  try {
    if (!perfOnly) await testBoardOrbit(browser, origin, report);
    const { page } = await openWindowTestPage(browser, origin);
    await page.setViewportSize({ width: 1440, height: 960 });
    /* 설치 앱의 화면은 2배 밀도다. Chromium은 그 밀도를 CDP로 흉내 낸다 — 캔버스가
     * 네 배의 픽셀을 칠하는 판의 값을 같은 표로 잰다. */
    const dpr = Number(option("--dpr", 1));
    if (dpr !== 1 && engine === "chromium") {
      const cdp = await page.context().newCDPSession(page);
      await cdp.send("Emulation.setDeviceMetricsOverride",
        { width: 1440, height: 960, deviceScaleFactor: dpr, mobile: false });
    }
    /* 픽스처의 크기 — 기본은 브리핑의 판(4·12·40·20). 더 큰 판의 값을 같은 표로. */
    const fixture = Object.fromEntries(["workspaces", "agents", "links", "fresh"]
      .filter((name) => option(`--${name}`, null) !== null)
      .map((name) => [name, Number(option(`--${name}`, 0))]));
    measured = await measureBoardOrbit(page, { seconds: Number(option("--seconds", 3)), fixture });
    await page.close();
  } finally {
    await browser.close();
    files.close();
  }
  const verdict = orbitVerdict(measured);
  report(`orbit weight on ${engine}: frame p95 ≤ ${ORBIT_FRAME_BUDGET_MS} ms, hidden rAF 0, reduced motion still, DOM budget`,
    verdict.holds, JSON.stringify(verdict.checks ?? verdict.why));
  const out = option("--json", null);
  if (out) writeFileSync(out, `${JSON.stringify({ engine, measured, verdict }, null, 2)}\n`);
  console.log(lines.join("\n"));
  console.log(`\n${orbitTable(measured, engine)}`);
  console.log(`\n${lines.filter((line) => line.startsWith("PASS")).length}/${lines.length} passed`);
  process.exit(lines.some((line) => line.startsWith("FAIL")) ? 1 : 0);
}
