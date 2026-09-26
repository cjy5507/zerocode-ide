/* t-9444 → t-10118 · 관계 탭의 「입체」 보기 — 회귀와 무게.
 *
 * 워크스페이스·에이전트·하위 에이전트가 3차원 좌표에 서고, 원근 카메라가 그 공간을
 * 비추며(`ui/shell-board-orbit.js`), 우편과 의존은 공간의 곡선을 따라 흐른다. 픽스처는
 * 프로덕션과 같은 네 문으로만 들어간다 — `__PANES__`·`__LEDGER__`·`__COLUMNS__`·
 * `__OVERLAYS__` — 그래야 체크아웃이 원장 행을 타고 `places`까지 와서 워크스페이스
 * 넷으로 갈리는지가 실제로 재어진다.
 *
 * 여기서 고정하는 계약:
 *   ① 관계 탭에는 「입체 | 카드」 토글이 있고, 저장이 없으면 입체가 선다. 고른 보기는
 *      로컬 설정 한 키에 남아 다시 연 창에서도 선다. 카메라는 남지 않는다(열 때마다 맞춤).
 *   ② 수는 표 하나(`ORBIT`)에 있다 — 파일의 나머지에는 0·1·2 말고 수가 없고, 색은 토큰이
 *      든다. 이 보기의 낱말은 어느 언어로도 행성·항성·궤도·위성을 말하지 않는다.
 *   ③ 보이지 않으면(작업 목록·카드 보기·숨은 판·숨은 문서) rAF는 0이다.
 *   ④ 멈춘 판의 rAF도 0이다 — 움직임을 줄이라는 판, 손이나 초점이 라벨에 든 판, 사람이
 *      돌려 놓은 뒤 흐를 것이 없는 판. 움직임을 줄인 판은 상태가 바뀔 때만 한 장 그린다.
 *   ⑤ 상태 적용은 이 보기가 쓰는 사실이 바뀔 때만이다(갱신 호출 수로 단언).
 *   ⑥ 점을 누르면 기존 선택 길로 인스펙터가 선다; 검색·범위·배율이 그대로 먹는다.
 *   ⑦ 입체가 선 동안 접힌 카드 그림에는 아무것도 쓰지 않는다 — 카드로 돌아온 첫 판이
 *      그 사이의 갱신을 빠짐없이 그리고 맞춤은 한 번 돈다 (t-9532).
 *   ⑧ 실시간 지도는 카드 그림 위의 것이다 — 입체에서 그 손잡이는 누를 수 없는 채로
 *      까닭을 말하고, 지도는 적지도 뛰지도 않는다; 카드로 돌아오면 그대로 선다 (t-9532).
 *   ⑨ 공간: 같은 입력은 같은 자리이고, 에이전트 하나가 와도 남의 자리는 움직이지 않는다.
 *      메인 워크스페이스는 원점, 다른 워크스페이스는 구면 위, 에이전트는 제 워크스페이스
 *      둘레의 작은 구면, 하위 에이전트는 부모 곁.
 *   ⑩ 투영: 카메라를 90° 돌리면 화면의 가로가 세계의 z를 따른다(원근 — 크기는 깊이에
 *      반비례). 먼 것이 먼저 그려지고 옅으며, 이름은 가까운 몇만 선다.
 *   ⑪ 간선: 구조·계보·의존·우편이 공간의 선이고 우편의 점은 그 곡선 위를 탄다. 카메라가
 *      멈춘 프레임은 간선을 다시 투영하지 않고, 프레임이 쓰는 DOM은 라벨의 style뿐이다.
 *   ⑫ 카메라: 끌면 돌고 놓으면 미끄러져 선다; 두 번 누르면 맞춤으로 돌아가 다시 돈다.
 *
 *   node ui/tests/board-orbit.mjs                     기능 시험 + 무게 표(6·20·60, Chromium)
 *   node ui/tests/board-orbit.mjs --perf --engine webkit --json out.json
 *   node ui/tests/board-orbit.mjs --perf --dpr 2       2배 밀도(Chromium, CDP)
 *   node ui/tests/board-orbit.mjs --shots <dir>        6·20·60 × 다크·라이트 사진 여섯 장
 */
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";
import { installBoardWaits } from "./board-waits.mjs";
import { median, quantile, webkitType } from "./coordinator-desk-perf.mjs";
import { frameBudgetHolds, loadNote, machineIsLoudNow } from "./machine-load.mjs";

const UI = resolve(dirname(fileURLToPath(import.meta.url)), "..");

/* 무게와 사진을 재는 판 셋. 우편과 의존은 판에 비례하고, 60 판의 우편 120(방금 오간 60)은
 * 우편이 많은 판의 조건이다 — 간선의 투영 값을 그 조건에서 잰다. */
const ORBIT_SIZES = Object.freeze({
  6: Object.freeze({ workspaces: 2, agents: 6, links: 8, fresh: 4, deps: 2 }),
  20: Object.freeze({ workspaces: 4, agents: 20, links: 40, fresh: 20, deps: 4 }),
  60: Object.freeze({ workspaces: 8, agents: 60, links: 120, fresh: 60, deps: 12 }),
});

/* 한 판: 워크스페이스 `workspaces`개(첫째가 메인), 에이전트 `agents`개(열둘마다 셋은 같은
 * 워크스페이스의 앞 에이전트를 부모로 드는 하위 에이전트), 우편 링크 `links`개 중
 * `fresh`개가 방금 오간 것(그 절반이 미확인), 과업 의존 `deps`개. 페이지 안에서 통째로
 * 도는 함수라 바깥 이름을 쓰지 않는다. */
export function orbitFixture({ workspaces = 4, agents = 12, links = 40, fresh = 20, deps = 0 } = {}) {
  window.__ORBIT_OPENED_AT__ = performance.now();
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
  /* 하위 에이전트: 열둘마다 5·10·11번째, 부모는 워크스페이스 수만큼 앞 — 같은 워크스페이스다
   * (기본 판에서 5→1·10→6·11→7). */
  const MOONS = new Map(Array.from({ length: agents }, (_, at) => at)
    .filter((at) => [5, 10, 11].includes(at % 12) && at - workspaces >= 0)
    .map((at) => [at, at - workspaces]));
  const terms = Array.from({ length: agents }, (_, at) => 401 + at);
  const treeOf = (at) => trees[at % trees.length];
  const branchOf = (at) => (at % trees.length === 0 ? "main" : `wt/w${at % trees.length}`);
  const parentOf = (at) => (MOONS.has(at) ? terms[MOONS.get(at)] : null);

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

  /* 링크: 서로 다른 쌍 `links`개. 묶음(에이전트 수만큼)마다 차이가 1·6·11·4이고, 작은 판에서
   * 그 차이가 자기 자신이나 앞 묶음과 겹치면 다음 차이로 민다. 앞의 `fresh`개가 방금 오간
   * 것이고 그 짝수 번째가 미확인이다. */
  const offsets = [1, 6, 11, 4];
  const shifts = [];
  for (let block = 0; block * agents < links; block += 1) {
    let shift = offsets[block % offsets.length] % agents;
    while (shift === 0 || shifts.includes(shift)) shift = (shift + 1) % agents;
    shifts.push(shift);
  }
  const mail = [];
  for (let k = 0; k < links; k += 1) {
    const from = terms[k % agents];
    const to = terms[(k % agents + shifts[Math.floor(k / agents)]) % agents];
    const recent = k < fresh;
    const at = recent ? now - 20_000 - k * 1_000 : now - 7_200_000 - k * 1_000;
    mail.push({ from: `term:${from}`, to: `term:${to}`, count: 1 + (k % 3), unread: recent && k % 2 === 0 ? 1 : 0,
      at, verb: "mail", last_message: { id: `m-o-${k}`, run: "run-o1", from: `worker:w-${from}`,
        to: `worker:w-${to}`, kind: "status", created_ms: at } });
  }
  /* 과업 의존: 앞선 과업(`from`)이 뒤의 과업(`to`)을 막는다 — 다섯 칸 간격의 서로 다른 쌍. */
  const taskDependencies = Array.from({ length: deps }, (_, d) => {
    const to = (d * 5 + 3) % agents;
    const from = (d * 5 + 1) % agents;
    return { run: "run-o1", task: `t-o${to}`, dependency: `t-o${from}`, from: `term:${terms[from]}`,
      to: `term:${terms[to]}`, task_state: "pending", dependency_state: "working", task_created_ms: now - 60_000 };
  });
  window.__OVERLAYS__ = { latest: null, mail, dependencies: [], task_dependencies: taskDependencies, merge: [] };

  /* 최근 활동: 작업 중인 에이전트마다 다른 양. */
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

/* 이 보기의 rAF만 센다 — 창의 다른 손(간선 측정·맞춤·터미널)이 거는 프레임은 이 수에
 * 들지 않는다. 콜백의 **이름**으로 가린다: 이 보기의 박자는 제 이름을 가진 최상위 함수
 * 하나(`agentOrbitTick`)다. 카메라와 투영의 검사는 판의 상태(`agentOrbitStates`)를
 * 직접 읽고, 투영은 아래 신탁(`__ORBIT_ORACLE__`)이 따로 셈한 값과 견준다. */
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
    window.__ORBIT_STATE__ = () => (typeof agentOrbitStates === "undefined" ? null
      : agentOrbitStates.get(document.querySelector("#board-view")) ?? null);
    /* 카메라를 한 자리에 세운다 — 저절로 도는 것도 미끄러짐도 없이. */
    window.__ORBIT_HOLD_CAMERA__ = (yaw, pitch) => {
      const state = window.__ORBIT_STATE__();
      Object.assign(state.camera, { yaw, pitch, spin: false, turning: 0, tilting: 0 });
      state.dirty = true;
      agentOrbitWake();
    };
    /* 신탁: 세계의 한 점을 카메라로. y축(세로) 둘레로 yaw, 그다음 x축 둘레로 pitch, 그리고
     * 원근 — 깊이는 카메라에서 잰 거리, 화면의 크기는 초점 거리 ÷ 깊이. 프로덕션의 셈을
     * 부르지 않고 따로 셈한다: 두 셈이 같아야 그림이 3차원에서 온 것이다. */
    window.__ORBIT_ORACLE__ = ([x, y, z], camera, lens) => {
      const cosYaw = Math.cos(camera.yaw);
      const sinYaw = Math.sin(camera.yaw);
      const cosPitch = Math.cos(camera.pitch);
      const sinPitch = Math.sin(camera.pitch);
      const turnedX = x * cosYaw - z * sinYaw;
      const turnedZ = x * sinYaw + z * cosYaw;
      const tiltedY = y * cosPitch + turnedZ * sinPitch;
      const tiltedZ = turnedZ * cosPitch - y * sinPitch;
      const depth = lens.distance - tiltedZ;
      const scale = lens.focal / depth;
      return { x: lens.cx + turnedX * scale, y: lens.cy + tiltedY * scale, depth, scale };
    };
    window.__ORBIT_BEZIER__ = ([a, c, b], t) => [0, 1, 2]
      .map((axis) => (1 - t) ** 2 * a[axis] + 2 * (1 - t) * t * c[axis] + t ** 2 * b[axis]);
  });
}

const openOrbit = async (page, fixture = {}) => {
  await installBoardWaits(page);
  await installOrbitCounters(page);
  await page.evaluate(orbitFixture, fixture);
  await page.evaluate(() => window.__BOARD_SETTLED__());
};

/* 한 구간 동안 이 보기가 건 rAF와 그린 그림의 수. */
const countOver = (page, ms) => page.evaluate(async (wait) => {
  await window.__ORBIT_FRAMES__(2);
  const rafs = window.__ORBIT_RAFS__;
  const frames = window.__ORBIT__()?.frames ?? null;
  await window.__ORBIT_WAIT__(wait);
  return { rafs: window.__ORBIT_RAFS__ - rafs, frames: (window.__ORBIT__()?.frames ?? 0) - (frames ?? 0),
    handles: window.__ORBIT__() };
}, ms);

/* 판의 빈 모서리 — 라벨이 서지 않는 자리(그림은 판 가운데에 맞춰 선다). 끌기와 두 번
 * 누르기가 여기서 시작한다. */
const stageCorner = (page) => page.evaluate(() => {
  const box = document.querySelector("#board-view .agent-orbit").getBoundingClientRect();
  return { x: box.left + 10, y: box.top + 10 };
});

/* 묶음들, 각자 제 판에서. 한 묶음이 넘어져도 나머지는 돈다 — 넘어진 묶음은 제 이름의
 * FAIL 한 줄로 남는다. */
export async function testBoardOrbit(browser, origin, ok) {
  const parts = [
    ["table", () => testOrbitShape(ok)],
    ["view", () => testOrbitView(browser, origin, ok)],
    ["space", () => testOrbitSpace(browser, origin, ok)],
    ["camera", () => testOrbitCamera(browser, origin, ok)],
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
      ok(`the 3D view ${name} checks ran to their end`, false,
        String(error?.message ?? error).split("\n").slice(0, 2).join(" | "));
    }
  }
}

/* 사람이 뺀 낱말, 다섯 언어로. 라틴 글자는 낱말 경계로 잰다(「start」는 「star」가 아니다). */
const PLANET_WORDS = [/행성/, /항성/, /궤도/, /위성/, /\borbits?\b/i, /\bplanets?\b/i, /\bstars?\b/i,
  /\bmoons?\b/i, /惑星/, /恒星/, /軌道/, /衛星/, /行星/, /轨道/, /卫星/, /[óo]rbitas?/i, /\bplanetas?\b/i,
  /\bestrellas?\b/i, /\bsat[ée]lites?\b/i, /\blunas?\b/i];

/* ② 표 하나, 그리고 낱말. 파일을 읽어 `ORBIT` 표를 도려내고 나머지에 남은 수를 센다 — 0·1·2는
 * 셈의 문법(반, 한 바퀴의 두 배, 없음)이라 수로 치지 않는다. 색 리터럴은 어디에도 없어야
 * 한다: 색은 CSS 토큰이 든다. 낱말은 사람이 읽는 세 곳을 잰다 — 다섯 카탈로그의
 * `board.orbit.*` 값, 마크업이 그 키로 든 글, 이 파일의 `t()` 폴백. 주석은 재지 않는다. */
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

  const catalog = readFileSync(resolve(UI, "shell-i18n.js"), "utf8");
  const markup = readFileSync(resolve(UI, "index.html"), "utf8").replace(/<!--[\s\S]*?-->/g, "");
  const values = [...catalog.matchAll(/"(board\.orbit\.[\w]+)":\s*"((?:\\.|[^"\\])*)"/g)]
    .map((hit) => ({ where: `catalog ${hit[1]}`, words: hit[2] }));
  const marked = [
    ...[...markup.matchAll(/data-i18n="(board\.orbit\.[\w]+)"[^>]*>([^<]*)</g)]
      .map((hit) => ({ where: `markup ${hit[1]}`, words: hit[2] })),
    ...[...markup.matchAll(/data-i18n-aria="(board\.orbit\.[\w]+)"[^>]*aria-label="([^"]*)"/g)]
      .map((hit) => ({ where: `markup aria ${hit[1]}`, words: hit[2] })),
  ];
  const fallbacks = [...code.matchAll(/\bt\(\s*"([^"]+)",\s*"((?:\\.|[^"\\])*)"/g)]
    .map((hit) => ({ where: `fallback ${hit[1]}`, words: hit[2] }));
  const said = [...values, ...marked, ...fallbacks];
  const planetary = said.filter((one) => PLANET_WORDS.some((word) => word.test(one.words)));
  ok("the 3D view's words name no planet, star, moon or orbit in any language",
    values.length >= 30 && marked.length >= 4 && fallbacks.length >= 4 && planetary.length === 0,
    JSON.stringify({ values: values.length, marked: marked.length, fallbacks: fallbacks.length,
      planetary: planetary.map((one) => `${one.where}: ${one.words}`) }));

  const named = [...catalog.matchAll(/"board\.orbit\.orbit":\s*"([^"]*)"/g)].map((hit) => hit[1]);
  const hints = [...catalog.matchAll(/"board\.orbit\.hint":\s*"([^"]*)"/g)].map((hit) => hit[1]);
  const button = markup.match(/data-relations-view="orbit"[^>]*>([^<]*)</)?.[1] ?? "";
  ok("the view is called 입체 — 3D in English and Spanish, 立体 in Japanese and Chinese",
    JSON.stringify(named) === JSON.stringify(["입체", "3D", "立体", "立体", "3D"]) && button === "입체"
      && hints[0] === "점을 누르면 상세가 열립니다", JSON.stringify({ named, button, hint: hints[0] }));
}

/* ① ⑥ 입체가 서고, 누르면 고른다. */
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
        words: toggle.map((button) => button.textContent.trim()),
        stageShown: Boolean(stage) && !stage.hidden && stage.getBoundingClientRect().width > 0,
        cardsHidden: view.querySelector(".agent-graph-scroll")?.hidden === true,
        role: canvas?.getAttribute("role") ?? null,
        summary: canvas?.getAttribute("aria-label") ?? "",
        hint: view.querySelector(".agent-graph-gesture-hint")?.textContent ?? "",
        agents: labels.filter((label) => label.dataset.orbitKey.startsWith("agent:")).length,
        workspaces: labels.filter((label) => label.dataset.orbitKey.startsWith("workspace:")).length,
        buttons: labels.every((label) => label.tagName === "BUTTON" && label.getAttribute("aria-label")),
        handles: window.__ORBIT__(),
      };
    });
    ok("the relations tab opens on the 3D view by default", shape.choice === "orbit"
      && shape.stageShown && shape.cardsHidden
      && JSON.stringify(shape.toggle) === JSON.stringify([["orbit", "true"], ["cards", "false"]])
      && shape.words[0] === "입체" && shape.hint === "점을 누르면 상세가 열립니다",
      JSON.stringify(shape));
    ok("the 3D view draws every agent and every workspace as a point with a button",
      shape.agents === 12 && shape.workspaces === 4 && shape.handles?.bodies === 16 && shape.buttons,
      JSON.stringify(shape));
    ok("the canvas is an image that says the counts", shape.role === "img"
      && /12/.test(shape.summary) && /5/.test(shape.summary) && /2/.test(shape.summary), shape.summary);
    ok("mail rides the links as dots, brighter where unread",
      shape.handles?.links === 40 && shape.handles?.particles === 20 && shape.handles?.bright === 10,
      JSON.stringify(shape.handles));
    ok("sub-agents stand beside their parent", shape.handles?.children === 3, JSON.stringify(shape.handles));

    const running = await countOver(page, 400);
    ok("a shown 3D view turns", running.rafs > 0 && running.frames > 0 && running.handles?.camera?.spin === true,
      JSON.stringify(running));

    /* 클릭. 움직이는 과녁이라 사람의 손이 먼저 올라가면 카메라가 멈춘다(hover) — Playwright의
     * 손도 같은 길로 간다: 올리고, 멈춘 것을 누른다. 누르는 것은 가장 가까운 에이전트 중
     * 제 가운데가 제 것인 라벨이다 — 먼 라벨은 가까운 라벨 밑에 서는 것이 이 보기의 규칙이다. */
    const key = await page.evaluate(() => {
      const order = window.__ORBIT__()?.order ?? [];
      for (const candidate of [...order].reverse()) {
        if (!candidate.startsWith("agent:")) continue;
        const label = document.querySelector(`#board-view [data-orbit-key="${candidate}"]`);
        const box = label?.getBoundingClientRect();
        if (!box || box.width === 0) continue;
        const hit = document.elementFromPoint(box.left + box.width / 2, box.top + box.height / 2);
        if (hit?.closest("[data-orbit-key]") === label) return candidate;
      }
      return "agent:term:404";
    });
    await page.hover(`[data-orbit-key="${key}"]`, { force: true, timeout: 5_000 });
    await page.evaluate(() => window.__ORBIT_WAIT__(500));
    const held = await page.evaluate((wanted) => {
      const label = document.querySelector(`#board-view [data-orbit-key="${wanted}"]`);
      const before = label?.style.translate;
      return new Promise((done) => setTimeout(() => done({ paused: window.__ORBIT__()?.paused,
        still: label?.style.translate === before }), 200));
    }, key);
    ok("hovering a point stills the camera so it can be clicked", held.paused === true && held.still === true,
      JSON.stringify({ key, ...held }));
    await page.click(`[data-orbit-key="${key}"]`, { timeout: 5_000 });
    const picked = await page.evaluate((wanted) => {
      const view = document.querySelector("#board-view");
      const label = view.querySelector(`[data-orbit-key="${wanted}"]`);
      const term = Number(wanted.split(":").pop());
      return {
        selected: agentGraphSelectedKey,
        pressed: label?.getAttribute("aria-pressed"),
        inspector: view.querySelector(".agent-inspector-title")?.textContent ?? "",
        task: window.__LEDGER__.find((row) => row.term === term)?.task ?? null,
        focused: document.activeElement?.dataset?.orbitKey ?? null,
        paused: window.__ORBIT__()?.paused,
      };
    }, key);
    ok("clicking a point selects that agent through the existing selection road",
      picked.selected === key && picked.pressed === "true" && Boolean(picked.task)
        && picked.inspector.includes(picked.task), JSON.stringify({ key, ...picked }));
    /* 누른 라벨은 초점을 든다 — 키보드로 온 사람의 과녁도 움직이지 않는다. */
    ok("a focused label holds the camera still", picked.focused !== key || picked.paused === true,
      JSON.stringify(picked));

    /* ⑤ 같은 판을 다시 그려도, 프레임이 흘러도 상태 적용은 그대로다. 손은 먼저 치운다 — 멈춘
     * 판에서도 적용의 수는 같아야 하지만, 맥동을 재는 판은 도는 판이다. */
    await page.mouse.move(1, 1);
    await page.evaluate(() => document.activeElement?.blur?.());
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
    ok("the 3D view applies state only when what it draws moves",
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
    ok("search dims the points that do not match", lens.dimmed === 11 && lens.lit === false, JSON.stringify(lens));
    ok("a run scope draws only that scope's agents",
      lens.scoped.length > 0 && JSON.stringify(lens.scoped) === JSON.stringify(lens.members), JSON.stringify(lens));
    ok("the existing zoom controls bring the camera closer and fit resets it",
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
    ok("a theme switch re-reads the 3D view's ink once, from one observer",
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
    ok("the 3D view adds at most fifty elements over the card view", dom.delta <= 50, JSON.stringify(dom));

    ok("the 3D view page raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* ⑨ ⑩ ⑪ 공간·투영·간선. 기본 판에 과업 의존 넷을 더한다. */
async function testOrbitSpace(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await openOrbit(page, { deps: 4 });
    await page.mouse.move(1, 1);
    await page.evaluate(() => window.__ORBIT_FRAMES__(3));

    /* ⑨ 자리. */
    const places = await page.evaluate(() => {
      const state = window.__ORBIT_STATE__();
      const bodies = [...(state?.bodies?.values() ?? [])];
      const gap = (a, b) => Math.hypot(a[0] - b[0], a[1] - b[1], a[2] - b[2]);
      /* 워크스페이스의 키는 프로젝트 경로와 체크아웃 경로를 NUL로 잇는다(`agentGraphModel`). */
      const main = state?.bodies.get("workspace:/repos/orbit\u0000/repos/orbit");
      const hubs = bodies.filter((body) => body.kind === "workspace" && body !== main);
      const agents = bodies.filter((body) => body.kind === "agent");
      const children = bodies.filter((body) => body.kind === "child");
      const angle = (a, b) => Math.acos(Math.min(1, Math.max(-1,
        (a[0] * b[0] + a[1] * b[1] + a[2] * b[2]) / (Math.hypot(...a) * Math.hypot(...b))))) * 180 / Math.PI;
      const spread = [0, 1, 2].map((axis) => Math.max(...bodies.map((body) => body.world?.[axis] ?? 0))
        - Math.min(...bodies.map((body) => body.world?.[axis] ?? 0)));
      return {
        count: bodies.length,
        spread,
        reach: state?.reach ?? null,
        main: main?.world ?? null,
        hubs: hubs.map((hub) => Math.hypot(...hub.world)),
        hubAngle: Math.min(...hubs.flatMap((one, at) => hubs.slice(at + 1).map((two) => angle(one.world, two.world)))),
        agents: agents.map((body) => gap(body.world, state.bodies.get(body.hostKey)?.world ?? [NaN, NaN, NaN])),
        children: children.map((body) => {
          const parent = state.bodies.get(body.hostKey);
          const hub = state.bodies.get(parent?.hostKey);
          return { parent: gap(body.world, parent?.world ?? [NaN, NaN, NaN]),
            hub: gap(body.world, hub?.world ?? [NaN, NaN, NaN]) };
        }),
        radii: { workspace: ORBIT.space?.workspace, agent: ORBIT.space?.agent, child: ORBIT.space?.child },
      };
    });
    const near = (value, wanted) => Math.abs(value - wanted) < 1e-6;
    ok("every point stands at a place in three dimensions",
      places.count === 16 && places.spread.every((width) => width > (places.reach ?? Infinity) * 0.4),
      JSON.stringify(places));
    ok("the main workspace stands at the origin and the others on a sphere around it, spread apart",
      places.main !== null && places.main.every((axis) => axis === 0) && places.hubs.length === 3
        && places.hubs.every((distance) => near(distance, places.radii.workspace)) && places.hubAngle >= 60,
      JSON.stringify(places));
    ok("an agent stands on a small sphere around its workspace and a sub-agent beside its parent",
      places.agents.length === 9 && places.agents.every((distance) => near(distance, places.radii.agent))
        && places.children.length === 3
        && places.children.every((one) => near(one.parent, places.radii.child) && one.parent < one.hub),
      JSON.stringify(places));

    const worlds = () => page.evaluate(() => Object.fromEntries(
      [...window.__ORBIT_STATE__().bodies.values()].map((body) => [body.key, [...body.target]])));
    const first = await worlds();
    await page.evaluate(async () => {
      document.querySelector('#board-view [data-relations-view="cards"]').click();
      await window.__BOARD_SETTLED__();
      document.querySelector('#board-view [data-relations-view="orbit"]').click();
      await window.__BOARD_SETTLED__();
      await window.__ORBIT_FRAMES__(2);
    });
    const again = await worlds();
    ok("the same input stands every point at the same place",
      Object.keys(first).length === 16 && JSON.stringify(first) === JSON.stringify(again),
      JSON.stringify({ first, again }));

    /* ⑩ 투영: 카메라를 정면(yaw 0·pitch 0)에 세우고, 90° 돌려 세운다. */
    const seen = (yaw, pitch) => page.evaluate(async ([yaw, pitch]) => {
      window.__ORBIT_HOLD_CAMERA__(yaw, pitch);
      await window.__ORBIT_FRAMES__(3);
      const state = window.__ORBIT_STATE__();
      const lens = agentOrbitLens(state);
      const bodies = [...state.bodies.values()];
      const worst = Math.max(...bodies.map((body) => {
        const wanted = window.__ORBIT_ORACLE__(body.world, state.camera, lens);
        return Math.max(Math.abs(wanted.x - body.seen.x), Math.abs(wanted.y - body.seen.y),
          Math.abs(wanted.depth - body.seen.depth) / wanted.depth);
      }));
      /* 90°의 셈을 신탁 없이 한 번 더: 화면의 가로 = −z × 초점 ÷ (거리 − x). */
      const quarter = Math.max(...bodies.map((body) => Math.abs((body.seen.x - lens.cx)
        - (-body.world[2]) * lens.focal / (lens.distance - body.world[0]))));
      const sizes = bodies.map((body) => Math.abs(body.seen.scale * body.seen.depth - lens.focal));
      return { worst, quarter, size: Math.max(...sizes), focalOverDistance: lens.focal / lens.distance,
        scale: state.scale, xs: Object.fromEntries(bodies.map((body) => [body.key, body.seen.x])) };
    }, [yaw, pitch]);
    const front = await seen(0, 0);
    const turned = await seen(Math.PI / 2, 0);
    const moved = Object.keys(front.xs).filter((key) => Math.abs(front.xs[key] - turned.xs[key]) > 1).length;
    ok("a quarter turn of the camera swaps x and z on the screen: the picture is projected from three dimensions",
      front.worst < 0.5 && turned.worst < 0.5 && turned.quarter < 0.5 && moved >= 12,
      JSON.stringify({ front: front.worst, turned: turned.worst, quarter: turned.quarter, moved }));
    ok("a nearer point is drawn larger: size is the focal length over the depth",
      front.size < 1e-6 && turned.size < 1e-6 && Math.abs(front.focalOverDistance - front.scale) < 1e-9,
      JSON.stringify({ front: front.size, turned: turned.size, focalOverDistance: front.focalOverDistance,
        scale: front.scale }));

    const depth = await page.evaluate(async () => {
      window.__ORBIT_HOLD_CAMERA__(ORBIT.camera.yaw, ORBIT.camera.pitch);
      await window.__ORBIT_FRAMES__(3);
      const state = window.__ORBIT_STATE__();
      const order = window.__ORBIT__().order ?? [];
      const depths = order.map((key) => state.bodies.get(key)?.seen.depth ?? NaN);
      const fogs = order.map((key) => state.bodies.get(key)?.seen.fog ?? NaN);
      const others = [...state.bodies.values()].filter((body) => body.kind !== "workspace")
        .sort((left, right) => left.seen.depth - right.seen.depth);
      const named = (body) => body.label?.style.getPropertyValue("--orbit-name") === "1";
      const hubsNamed = [...state.bodies.values()].filter((body) => body.kind === "workspace").every(named);
      const wantNamed = others.slice(0, ORBIT.label.near).map((body) => body.key);
      const isNamed = others.filter(named).map((body) => body.key);
      return { order: order.length, depths, fogs, near: ORBIT.label.near, far: ORBIT.fog.far, hubsNamed,
        wantNamed, isNamed, handlesNamed: window.__ORBIT__().named };
    });
    ok("far points are drawn first: the painter's order runs from the deepest to the nearest",
      depth.order === 16 && depth.depths.every((value, at) => at === 0 || value <= depth.depths[at - 1]),
      JSON.stringify(depth.depths));
    ok("far points fade: the fog thins a point by its depth and never below the table's far alpha",
      depth.fogs.every((value, at) => value >= depth.far - 1e-9 && value <= 1 + 1e-9
        && (at === 0 || value >= depth.fogs[at - 1] - 1e-9)) && depth.fogs[depth.fogs.length - 1] > depth.fogs[0],
      JSON.stringify(depth.fogs));
    ok("only the nearest points carry their names, and every workspace keeps its own",
      depth.hubsNamed && depth.near < 12 && JSON.stringify(depth.isNamed) === JSON.stringify(depth.wantNamed)
        && depth.handlesNamed === depth.near + 4, JSON.stringify(depth));

    /* ⑪ 간선: 구조(메인 → 워크스페이스 셋, 워크스페이스 → 에이전트 아홉)·계보 셋·의존 넷·우편 마흔. */
    const edges = await page.evaluate(() => {
      const state = window.__ORBIT_STATE__();
      const handles = window.__ORBIT__();
      const outward = (curve) => {
        const [a, c, b] = curve;
        const middle = [0, 1, 2].map((axis) => (a[axis] + b[axis]) / 2);
        return Math.hypot(...middle) < 1 || Math.hypot(...c) > Math.hypot(...middle);
      };
      const lens = agentOrbitLens(state);
      const riders = state.links.flatMap((link) => link.particles.map((particle) => {
        const wanted = window.__ORBIT_ORACLE__(window.__ORBIT_BEZIER__(link.curve, particle.t), state.camera, lens);
        return Math.hypot(wanted.x - particle.seen.x, wanted.y - particle.seen.y);
      }));
      return {
        edges: handles.edges,
        links: handles.links,
        bent: [...state.links, ...state.edges.filter((edge) => edge.kind === "dependency")]
          .every((edge) => Array.isArray(edge.curve) && outward(edge.curve)),
        riders: riders.length,
        worstRider: Math.max(...riders),
      };
    });
    ok("structure, lineage, dependency and mail are lines in the space",
      JSON.stringify(edges.edges) === JSON.stringify({ structure: 12, spawned: 3, dependency: 4 })
        && edges.links === 40, JSON.stringify(edges));
    ok("mail and dependency curves bow away from the middle and the mail dots ride them",
      edges.bent && edges.riders === 20 && edges.worstRider < 0.5, JSON.stringify(edges));

    /* 카메라가 멈춘 프레임: 점은 흐르되 간선은 다시 투영하지 않는다. 카메라가 움직이면 한다. */
    const cache = await page.evaluate(async () => {
      await window.__ORBIT_FRAMES__(3);
      const before = window.__ORBIT__();
      await window.__ORBIT_FRAMES__(20);
      const still = window.__ORBIT__();
      const state = window.__ORBIT_STATE__();
      state.camera.yaw += 0.2;
      state.dirty = true;
      agentOrbitWake();
      await window.__ORBIT_FRAMES__(3);
      const turned = window.__ORBIT__();
      return { frames: still.frames - before.frames, projections: still.projections - before.projections,
        afterTurn: turned.projections - still.projections };
    });
    ok("a still camera reuses its projected edges while the mail dots flow",
      cache.frames >= 10 && cache.projections === 0 && cache.afterTurn >= 1, JSON.stringify(cache));

    /* 프레임의 DOM: 카메라가 도는 동안 라벨의 style(자리·깊이·이름)만 — 새 노드·클래스·글자 0. */
    const writes = await page.evaluate(async () => {
      const state = window.__ORBIT_STATE__();
      state.camera.spin = true;
      state.dirty = true;
      agentOrbitWake();
      await window.__ORBIT_FRAMES__(3);
      const stage = document.querySelector("#board-view .agent-orbit");
      const labels = stage.querySelectorAll(".agent-orbit-label").length;
      const kinds = {};
      let styled = 0;
      const watch = new MutationObserver((records) => {
        for (const record of records) {
          const kind = record.type === "attributes" ? `${record.type}:${record.attributeName}` : record.type;
          const onLabel = record.target instanceof Element && record.target.classList.contains("agent-orbit-label");
          if (kind === "attributes:style" && onLabel) styled += 1;
          else kinds[kind] = (kinds[kind] ?? 0) + 1;
        }
      });
      watch.observe(stage, { subtree: true, childList: true, attributes: true, characterData: true });
      const frames = window.__ORBIT__().frames;
      await window.__ORBIT_FRAMES__(30);
      watch.disconnect();
      return { styled, others: kinds, frames: window.__ORBIT__().frames - frames, labels,
        after: stage.querySelectorAll(".agent-orbit-label").length };
    });
    ok("while the camera turns a frame writes only the labels' style — no node, no class, no text",
      writes.frames >= 10 && writes.styled > 0 && Object.keys(writes.others).length === 0
        && writes.labels === writes.after, JSON.stringify(writes));

    /* ⑨ 에이전트 하나가 와도 남의 자리는 그대로 — 세계에서도, 멈춘 카메라의 화면에서도. */
    const arrival = await page.evaluate(async () => {
      window.__ORBIT_HOLD_CAMERA__(ORBIT.camera.yaw, ORBIT.camera.pitch);
      await window.__ORBIT_FRAMES__(3);
      const state = window.__ORBIT_STATE__();
      const read = () => Object.fromEntries([...state.bodies.values()].map((body) =>
        [body.key, { world: [...body.target], x: body.seen.x, y: body.seen.y }]));
      const before = read();
      const now = window.__ORBIT_NOW__;
      window.__PANES__.push({ term: 413, agent: "codex", state: "working", at: now - 5_000,
        state_started_at: now - 5_000, resumable: false });
      window.__LEDGER__.push({ ...window.__LEDGER__[1], worker: "w-413", task: "새 일", task_id: "t-o13",
        dispatch_id: "dp-o13", term: 413, pane: "%13" });
      const working = window.__COLUMNS__.find((column) => column.bucket === "working");
      working.cards.push({ ...working.cards[0], pane: "term:413", heading: "새 일", task: "새 일",
        worktree: "wt/w1", parent: "" });
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
      await window.__ORBIT_FRAMES__(3);
      const after = read();
      const shifted = Object.keys(before).filter((key) => JSON.stringify(before[key].world) !== JSON.stringify(after[key]?.world)
        || Math.abs(before[key].x - after[key].x) > 0.01 || Math.abs(before[key].y - after[key].y) > 0.01);
      return { arrived: Boolean(after["agent:term:413"]), bodies: Object.keys(after).length, shifted };
    });
    ok("an agent that arrives moves no other point, in the space or on the screen",
      arrival.arrived && arrival.bodies === 17 && arrival.shifted.length === 0, JSON.stringify(arrival));
    ok("the space page raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* ⑫ 카메라. 우편이 흐르지 않는 판(방금 오간 것 0) — 멈춘 카메라의 판에 흐를 것이 없다. */
async function testOrbitCamera(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await openOrbit(page, { fresh: 0 });
    await page.mouse.move(1, 1);
    const camera = () => page.evaluate(() => ({ ...window.__ORBIT__().camera, zoom: agentGraphZoom }));
    const opened = await camera();
    await page.evaluate(() => window.__ORBIT_WAIT__(300));
    const later = await camera();
    ok("the 3D view opens turning slowly by itself, at the table's pitch",
      opened.spin === true && later.yaw !== opened.yaw && Math.abs(later.pitch - opened.pitch) < 1e-9,
      JSON.stringify({ opened, later }));

    const corner = await stageCorner(page);
    const turn = await page.evaluate(() => ORBIT.camera.turn);
    const before = await camera();
    await page.mouse.move(corner.x, corner.y);
    await page.mouse.down();
    await page.mouse.move(corner.x + 80, corner.y + 40, { steps: 4 });
    await page.mouse.move(corner.x + 160, corner.y + 80, { steps: 4 });
    const dragged = await camera();
    await page.mouse.up();
    ok("dragging turns the camera: across is yaw, down is pitch",
      dragged.spin === false && Math.abs((dragged.yaw - before.yaw) - 160 * turn) < 160 * turn * 0.25
        && Math.abs((dragged.pitch - before.pitch) - 80 * turn) < 1e-6,
      JSON.stringify({ before, dragged, turn }));
    await page.evaluate(() => window.__ORBIT_WAIT__(1_500));
    const glided = await camera();
    const settled = await countOver(page, 500);
    const stood = await camera();
    ok("let go, the camera glides and stands where it was left, asking no frames",
      glided.yaw !== dragged.yaw && stood.yaw === glided.yaw && stood.spin === false
        && settled.rafs === 0 && settled.frames === 0, JSON.stringify({ dragged, glided, stood, settled }));

    await page.mouse.move(corner.x, corner.y);
    await page.mouse.down();
    await page.mouse.move(corner.x, corner.y + 2_000, { steps: 8 });
    await page.mouse.up();
    const tilted = await page.evaluate(() => ({ pitch: window.__ORBIT__().camera.pitch, max: ORBIT.camera.pitchMax }));
    ok("the pitch stops at the table's limit", Math.abs(tilted.pitch - tilted.max) < 1e-9, JSON.stringify(tilted));

    await page.mouse.dblclick(corner.x, corner.y);
    await page.evaluate(() => window.__ORBIT_FRAMES__(3));
    const fitted = await page.evaluate(() => ({ ...window.__ORBIT__().camera, zoom: agentGraphZoom,
      pitchWanted: ORBIT.camera.pitch }));
    const turning = await countOver(page, 300);
    ok("a double click fits: zoom 1, the table's pitch, and the camera turns again",
      fitted.zoom === 1 && fitted.spin === true && Math.abs(fitted.pitch - fitted.pitchWanted) < 1e-9
        && turning.rafs > 0, JSON.stringify({ fitted, turning: turning.rafs }));
    ok("the camera page raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* ③ ④ 보이지 않으면 rAF 0 — 작업 목록·카드 보기·숨은 판·숨은 문서, 손이 오른 판도 0,
 * 그리고 초점 없는 창은 30 fps. */
async function testOrbitGates(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await openOrbit(page);
    const shown = await countOver(page, 300);
    ok("gate control: the shown 3D view asks frames", shown.rafs > 0, JSON.stringify(shown));

    /* 에이전트의 라벨 — 워크스페이스의 키는 NUL을 품어 CSS 선택자로 잡히지 않는다. */
    const key = await page.evaluate(() => document.querySelector('#board-view [data-orbit-key^="agent:"]')?.dataset.orbitKey);
    await page.hover(`[data-orbit-key="${key}"]`, { force: true, timeout: 5_000 });
    /* 막 선 판의 점들이 나타나기를 마칠 시간까지 — 멈춤이 0이라고 말하는 것은 그 뒤의 판이다. */
    await page.evaluate(() => window.__ORBIT_WAIT__(1_200));
    const held = await countOver(page, 400);
    ok("a hand on a label holds the 3D view still and asks no frames",
      held.rafs === 0 && held.frames === 0 && held.handles?.paused === true, JSON.stringify(held));
    await page.mouse.move(1, 1);
    const released = await countOver(page, 300);
    ok("the hand leaving turns it again", released.rafs > 0, JSON.stringify(released));

    await page.evaluate(async () => {
      document.querySelector('#board-view [data-board-mode="tasks"]').click();
      await window.__BOARD_SETTLED__();
    });
    const tasks = await countOver(page, 400);
    ok("the task list stops the 3D view's frames", tasks.rafs === 0 && tasks.frames === 0, JSON.stringify(tasks));

    await page.evaluate(async () => {
      document.querySelector('#board-view [data-board-mode="graph"]').click();
      await window.__BOARD_SETTLED__();
    });
    const back = await countOver(page, 300);
    ok("returning to the relations tab starts the 3D view again", back.rafs > 0, JSON.stringify(back));

    await page.evaluate(async () => {
      document.querySelector('#board-view [data-relations-view="cards"]').click();
      await window.__BOARD_SETTLED__();
    });
    const cards = await countOver(page, 400);
    ok("the card view stops the 3D view's frames", cards.rafs === 0 && cards.frames === 0, JSON.stringify(cards));
    await page.evaluate(async () => {
      document.querySelector('#board-view [data-relations-view="orbit"]').click();
      await window.__BOARD_SETTLED__();
    });

    await page.evaluate(() => { document.querySelector("#board-view").hidden = true; });
    const tabHidden = await countOver(page, 400);
    ok("a hidden board asks no 3D frames", tabHidden.rafs === 0 && tabHidden.frames === 0, JSON.stringify(tabHidden));
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
    ok("a hidden document asks no 3D frames", docHidden.rafs === 0 && docHidden.frames === 0, JSON.stringify(docHidden));
    await page.evaluate(() => {
      Object.defineProperty(document, "hidden", { configurable: true, get: () => false });
      document.dispatchEvent(new Event("visibilitychange"));
    });
    const woke = await countOver(page, 300);
    ok("the document coming back starts the 3D view again", woke.rafs > 0, JSON.stringify(woke));

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
    ok("an unfocused window draws the 3D view at most thirty times a second",
      blurred.fps > 0 && blurred.fps <= 31, JSON.stringify(blurred));
    ok("the gates page raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* ④ 움직임을 줄이라는 판: 기본은 카드, 입체를 고르면 정지 화면 — 저절로 돌지 않고, 상태가
 * 바뀔 때만 한 장, 끌면 옮길 때마다 한 장(rAF 없이). */
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
    const yaw = await page.evaluate(() => window.__ORBIT__()?.camera?.yaw ?? null);
    const still = await countOver(page, 800);
    ok("reduced motion stands the 3D view still: no turning and no frames while nothing changes",
      still.rafs === 0 && still.frames === 0 && (still.handles?.frames ?? 0) >= 1 && yaw !== null
        && still.handles?.camera?.yaw === yaw, JSON.stringify({ yaw, still }));
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

    const corner = await stageCorner(page);
    const dragged = await page.evaluate(() => ({ frames: window.__ORBIT__().frames, rafs: window.__ORBIT_RAFS__,
      yaw: window.__ORBIT__().camera.yaw }));
    await page.mouse.move(corner.x, corner.y);
    await page.mouse.down();
    await page.mouse.move(corner.x + 120, corner.y, { steps: 6 });
    await page.mouse.up();
    const after = await page.evaluate(async () => {
      await window.__ORBIT_WAIT__(400);
      return { frames: window.__ORBIT__().frames, rafs: window.__ORBIT_RAFS__, yaw: window.__ORBIT__().camera.yaw };
    });
    ok("under reduced motion a drag turns the still picture a frame a move, with no glide and no frames asked",
      after.yaw > dragged.yaw && after.frames > dragged.frames && after.rafs === dragged.rafs,
      JSON.stringify({ dragged, after }));
    ok("the reduced-motion page raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* ① 고른 보기는 로컬 설정 한 키에 남고, 다시 연 창에서도 선다. 카메라는 남지 않는다. */
async function testOrbitRemembered(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await openOrbit(page);
    const corner = await stageCorner(page);
    await page.mouse.move(corner.x, corner.y);
    await page.mouse.down();
    await page.mouse.move(corner.x + 100, corner.y + 30, { steps: 4 });
    await page.mouse.up();
    const saved = await page.evaluate(async () => {
      document.querySelector('#board-view [data-relations-view="cards"]').click();
      await window.__BOARD_SETTLED__();
      const keys = Object.keys(localStorage).filter((key) => key.includes("relations"));
      return { keys, value: keys.map((key) => localStorage.getItem(key)),
        camera: Object.keys(localStorage).filter((key) => /orbit|camera|yaw|pitch|3d/i.test(key)),
        stage: document.querySelector("#board-view .agent-orbit")?.hidden,
        cards: document.querySelector("#board-view .agent-graph-scroll")?.hidden };
    });
    ok("choosing the card view is saved under one local key, and the camera is saved nowhere",
      saved.keys.length === 1 && saved.value[0] === "cards" && saved.stage === true && saved.cards === false
        && saved.camera.length === 0, JSON.stringify(saved));
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
    const refit = await page.evaluate(async () => {
      document.querySelector('#board-view [data-relations-view="orbit"]').click();
      await window.__BOARD_SETTLED__();
      await window.__ORBIT_FRAMES__(2);
      return { ...window.__ORBIT__().camera, pitchWanted: ORBIT.camera.pitch };
    });
    ok("the 3D view it returns to opens at the table's camera, turning",
      refit.spin === true && Math.abs(refit.pitch - refit.pitchWanted) < 1e-9, JSON.stringify(refit));
    ok("the remembered page raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* ⑦ 입체가 선 동안 접힌 카드 그림에는 아무것도 쓰지 않는다. 같은 판 세 번, 카드 하나가 끝난 판,
 * 새 에이전트가 온 판 — 카드 판(`.agent-graph-scroll`) 아래의 변이가 0이고, 카드 노드를 짓지도
 * 간선을 재지도 않는다. 보는 자리가 카드 판뿐인 것은 입체가 제 캔버스와 라벨을 프레임마다 쓰기
 * 때문이다. 모델의 배치(`agentGraphLayoutRuns`)는 입체도 읽는 모델 안의 셈이라 수만 적는다.
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
    ok("the 3D view writes nothing into the folded card picture: no mutation, no card built, no edge measured",
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
    ok("the card view it returns to draws every update the 3D view skipped, fits once, and its edges stand where a fresh measure puts them",
      back.fits === 1 && JSON.stringify(back.nodes) === JSON.stringify(back.wanted) && back.finished === true
      && back.arrived && back.edges > 0 && back.remeasuredInPlace, JSON.stringify(back));
    ok("the folded-cards page raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* ⑧ 실시간 지도는 카드 그림 위의 것이다. 카드에서 켠 지도를 입체로 가져가면 손잡이는 눌린
 * 채로 누를 수 없고(`aria-disabled` — 초점과 손은 받아 팁이 까닭을 말한다), 눌러도 켜진 채다.
 * 지도는 그동안 사건을 적지도 박자를 얹지도 않고, 인스펙터도 지도의 줄을 싣지 않는다. 카드로
 * 돌아오면 지도가 그대로 서고 입체에 있던 동안의 일은 조용한 기준선이 되며, 그 뒤의 새
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
    ok("in the 3D view the live map's handle stays pressed, cannot be pressed, and says the map shows in the card view",
      cardsOn.on && cardsOn.disabled !== "true" && orbitOn.orbit && orbitOn.on && orbitOn.disabled === "true"
      && orbitOn.pressed === "true" && orbitOn.cardsOnly !== "" && orbitOn.tip === orbitOn.cardsOnly
      && orbitOn.label === orbitOn.cardsOnly && orbitOn.key === "board.orbit.liveInCards" && pressed.on,
      JSON.stringify({ cardsOn, orbitOn, pressedOn: pressed.on }));
    ok("in the 3D view the live map records nothing, beats nothing, and the inspector carries none of its lines",
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
    ok("back on the card view the live map stands as it was left and what came in the 3D view stays a quiet baseline",
      returned.shown && returned.disabled !== "true" && returned.tip === returned.title
      && returned.key === "board.live.title" && returned.liveEdges > 0 && returned.inspectorEvents === 1
      && returned.events === cardsOn.events && returned.beats === 0
      && baseline.events === cardsOn.events && baseline.beats === 0, JSON.stringify({ returned, baseline }));
    ok("back on the card view a new event is recorded and beats",
      cardsMail.events === baseline.events + 1 && cardsMail.beats > 0, JSON.stringify(cardsMail));
    ok("the 3D view live page raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* ---- 무게 ------------------------------------------------------------------
 *
 * 한 프레임의 값은 둘로 잰다(`a-frame-instrument-that-stops-at-apply…`의 교훈): 이 보기의
 * 그리는 JS(그리는 함수를 감싼 벽시계), 그리고 rAF가 실제로 돌아오는 간격 — 같은 판에서
 * 입체를 숨긴 **정지 대조군**의 간격과 나란히. JS가 싸도 간격이 벌어지면 그 몫은 스타일·
 * 레이아웃·합성이다. 카메라가 도는 판과 멈춘 판(점만 흐른다)을 따로 잰다 — 멈춘 판은 간선을
 * 다시 투영하지 않는 판이다. 옛 트리(카메라가 없는 그림)에서도 같은 함수가 돈다: 전/후를
 * 같은 자로 재기 위해서다. */
export async function measureBoardOrbit(page, { seconds = 3, fixture = {} } = {}) {
  await installBoardWaits(page);
  await installOrbitCounters(page);
  /* 첫 그림까지: 픽스처가 보드를 열기 시작한 때부터 이 보기의 첫 그림이 끝난 때까지. */
  await page.evaluate(() => {
    const draw = window.agentOrbitDraw;
    window.__ORBIT_FIRST__ = null;
    if (typeof draw !== "function") return;
    window.agentOrbitDraw = function agentOrbitDraw(...args) {
      const answer = draw(...args);
      if (window.__ORBIT_FIRST__ === null && (window.__ORBIT__()?.frames ?? 0) > 0) {
        window.__ORBIT_FIRST__ = performance.now();
        window.agentOrbitDraw = draw;
      }
      return answer;
    };
  });
  await page.evaluate(orbitFixture, fixture);
  await page.evaluate(() => window.__BOARD_SETTLED__());
  await page.evaluate(() => window.__UNTIL__(() => window.__ORBIT_FIRST__ !== null, "the first 3D frame", 5_000));
  const firstFrameMs = await page.evaluate(() => window.__ORBIT_FIRST__ - window.__ORBIT_OPENED_AT__);
  const product = await page.evaluate(() => typeof agentOrbitHandles === "function");
  const camera = await page.evaluate(() => Boolean(window.__ORBIT__()?.camera));
  await page.mouse.move(1, 1);
  const probe = (on, { still = false } = {}) => page.evaluate(async ({ on, still, seconds }) => {
    const view = document.querySelector("#board-view");
    const want = on ? "orbit" : "cards";
    const button = view.querySelector(`[data-relations-view="${want}"]`);
    if (button && button.getAttribute("aria-pressed") !== "true") {
      button.click();
      await window.__BOARD_SETTLED__();
    }
    const state = window.__ORBIT_STATE__?.();
    if (on && state?.camera) {
      state.camera.spin = !still;
      state.dirty = true;
      agentOrbitWake();
    }
    await window.__ORBIT_FRAMES__(4);
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
    /* 프레임의 DOM 쓰기: 라벨의 style과 그 밖의 것을 나눠 센다. */
    const stage = view.querySelector(".agent-orbit");
    let styled = 0;
    let others = 0;
    const watch = new MutationObserver((records) => {
      for (const record of records) {
        const onLabel = record.target instanceof Element && record.target.classList.contains("agent-orbit-label");
        if (record.type === "attributes" && record.attributeName === "style" && onLabel) styled += 1;
        else others += 1;
      }
    });
    if (on && stage) watch.observe(stage, { subtree: true, childList: true, attributes: true, characterData: true });
    const labels = stage?.querySelectorAll(".agent-orbit-label").length ?? 0;
    let last = null;
    let running = true;
    const gap = (stamp) => {
      if (last !== null) gaps.push(stamp - last);
      last = stamp;
      if (running) requestAnimationFrame(gap);
    };
    requestAnimationFrame(gap);
    const frames = window.__ORBIT__()?.frames ?? 0;
    const projections = window.__ORBIT__()?.projections ?? 0;
    const from = performance.now();
    await window.__ORBIT_WAIT__(seconds * 1000);
    const elapsed = (performance.now() - from) / 1000;
    running = false;
    watch.disconnect();
    if (on && typeof draw === "function") window.agentOrbitDraw = draw;
    const drawn = (window.__ORBIT__()?.frames ?? 0) - frames;
    if (on && state?.camera) {
      state.camera.spin = true;
      state.dirty = true;
      agentOrbitWake();
    }
    return { elements, times, gaps, fps: drawn / elapsed, drawn,
      projections: (window.__ORBIT__()?.projections ?? 0) - projections,
      styledPerFrame: drawn > 0 ? styled / drawn : 0, others,
      newLabels: (stage?.querySelectorAll(".agent-orbit-label").length ?? 0) - labels,
      pixels: on && stage ? stage.querySelector("canvas").width * stage.querySelector("canvas").height : null,
      handles: window.__ORBIT__() };
  }, { on, still, seconds });
  const cards = await probe(false);
  const orbit = product ? await probe(true) : null;
  const still = product && camera && (fixture.fresh ?? 20) > 0 ? await probe(true, { still: true }) : null;
  /* 손이 라벨에 오른 판의 rAF — 이 보기가 0이라고 말한 수. */
  const held = product ? await page.evaluate(async () => {
    const label = document.querySelector("#board-view .agent-orbit-label");
    label?.dispatchEvent(new PointerEvent("pointerover", { bubbles: true }));
    await window.__ORBIT_WAIT__(600);
    const rafs = window.__ORBIT_RAFS__;
    await window.__ORBIT_WAIT__(1_000);
    const asked = window.__ORBIT_RAFS__ - rafs;
    label?.dispatchEvent(new PointerEvent("pointerout", { bubbles: true }));
    await window.__ORBIT_FRAMES__(2);
    return asked;
  }) : null;
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
  const round = (value) => (value === null || value === undefined || Number.isNaN(value) ? null : Number(value.toFixed(3)));
  const frameOf = (probed) => (probed ? {
    frameP50: round(median(probed.times)), frameP95: round(quantile(probed.times, 0.95)),
    frameMax: probed.times.length ? round(Math.max(...probed.times)) : null, frames: probed.times.length,
    fps: round(probed.fps), gapP50: round(median(probed.gaps)), gapP95: round(quantile(probed.gaps, 0.95)),
    projections: probed.projections, styledPerFrame: round(probed.styledPerFrame), others: probed.others,
    newLabels: probed.newLabels,
  } : null);
  return {
    product: product ? (camera ? "3D view (this tree)" : "flat orbit (baseline)") : "no picture",
    viewport: await page.evaluate(() => ({ width: innerWidth, height: innerHeight, dpr: devicePixelRatio })),
    fixture: { workspaces: 4, agents: 12, links: 40, fresh: 20, deps: 0, ...fixture },
    firstFrameMs: round(firstFrameMs),
    pixels: orbit?.pixels ?? null,
    cards: { elements: cards.elements, gapP50: round(median(cards.gaps)), gapP95: round(quantile(cards.gaps, 0.95)) },
    orbit: orbit ? { elements: orbit.elements, ...frameOf(orbit), handles: orbit.handles } : null,
    still: frameOf(still),
    domDelta: orbit ? orbit.elements - cards.elements : null,
    heldRafs: held,
    hiddenRafs: hidden,
    reduced,
    load: loadNote(),
    loud: machineIsLoudNow(),
  };
}

const ORBIT_FRAME_BUDGET_MS = 8;
/* 요소 예산은 브리핑의 판(워크스페이스 4·에이전트 12)의 것이다. 다른 판에서는 몸 하나가
 * 얹는 요소 수(버튼 + 이름 = 2)를 잰다 — 판이 커지면 늘어나는 것이 옳은 수다. */
const ORBIT_DOM_BUDGET = 50;
const ORBIT_ELEMENTS_PER_BODY = 2;
const orbitBriefFixture = (fixture) =>
  fixture.workspaces === 4 && fixture.agents === 12 && fixture.links === 40 && fixture.fresh === 20;

function orbitVerdict(measured) {
  if (!measured.orbit) return { holds: false, why: "no 3D view in this product" };
  const bodies = measured.orbit.handles?.bodies ?? 0;
  const checks = {
    frameP95: frameBudgetHolds(measured.orbit.frameP95, ORBIT_FRAME_BUDGET_MS),
    stillP95: measured.still === null || frameBudgetHolds(measured.still.frameP95, ORBIT_FRAME_BUDGET_MS),
    stillReprojects: measured.still === null || measured.still.projections === 0,
    heldRafs: measured.heldRafs === 0,
    hiddenRafs: measured.hiddenRafs === 0,
    reducedStill: measured.reduced?.frames === 0 && measured.reduced?.rafs === 0,
    writesOnlyLabelStyle: measured.orbit.others === 0 && measured.orbit.newLabels === 0,
    domDelta: orbitBriefFixture(measured.fixture)
      ? measured.domDelta <= ORBIT_DOM_BUDGET
      : measured.domDelta <= ORBIT_ELEMENTS_PER_BODY * bodies,
  };
  return { holds: Object.values(checks).every(Boolean), checks };
}

function orbitTable(measured, engine) {
  const orbit = measured.orbit ?? {};
  const still = measured.still ?? {};
  const edges = orbit.handles?.edges ?? {};
  return [
    `| engine | ${engine} | ${measured.viewport.width}×${measured.viewport.height} @${measured.viewport.dpr}x | ${measured.load} |`,
    `| fixture | ${JSON.stringify(measured.fixture)} | ${measured.product} | |`,
    "|---|---|---|---|",
    `| bodies · edges (structure/lineage/dependency) · mail links · dots | ${orbit.handles?.bodies} · ${edges.structure}/${edges.spawned}/${edges.dependency} · ${orbit.handles?.links} · ${orbit.handles?.particles} | | |`,
    `| turning: frame JS p50 / p95 / max (ms) | ${orbit.frameP50} / ${orbit.frameP95} / ${orbit.frameMax} | budget ≤ ${ORBIT_FRAME_BUDGET_MS} | ${orbit.frames} frames |`,
    `| still camera, dots flowing: frame JS p50 / p95 (ms) | ${still.frameP50 ?? "—"} / ${still.frameP95 ?? "—"} | edge re-projections ${still.projections ?? "—"} (must be 0) | ${still.frames ?? "—"} frames |`,
    `| frames drawn / s | ${orbit.fps} | rAF gap p50/p95 3D ${orbit.gapP50}/${orbit.gapP95} ms | still control ${measured.cards.gapP50}/${measured.cards.gapP95} ms |`,
    `| DOM writes a frame (label style) · other writes · new labels | ${orbit.styledPerFrame} · ${orbit.others} · ${orbit.newLabels} | others and new must be 0 | |`,
    `| first frame after opening (ms) · canvas pixels | ${measured.firstFrameMs} · ${measured.pixels} | | |`,
    `| rAF (1 s): hand on a label · board hidden · reduced motion | ${measured.heldRafs} · ${measured.hiddenRafs} · ${measured.reduced?.rafs} | must be 0 · 0 · 0 | |`,
    `| elements card view → 3D view | ${measured.cards.elements} → ${orbit.elements} | delta ${measured.domDelta}`
      + ` (${orbitBriefFixture(measured.fixture) ? `≤ ${ORBIT_DOM_BUDGET}` : `≤ ${ORBIT_ELEMENTS_PER_BODY} × ${orbit.handles?.bodies} bodies`})`
      + ` | ${orbit.handles?.bodies} bodies |`,
  ].join("\n");
}

/* 사진 여섯 장: 판 셋 × 다크·라이트. 카메라는 여는 판의 자리(`agentOrbitAim`, 표의 기울기)에
 * 세운다(옛 트리에서는 그림이 도는 대로) — 같은 판을 같은 각도로 다시 찍을 수 있게. */
async function shootBoardOrbit(browser, origin, dir, dpr) {
  mkdirSync(dir, { recursive: true });
  const shots = [];
  for (const [size, fixture] of Object.entries(ORBIT_SIZES)) {
    for (const theme of ["dark", "light"]) {
      const { page } = await openWindowTestPage(browser, origin);
      try {
        await page.setViewportSize({ width: 1440, height: 960 });
        if (dpr !== 1) {
          const cdp = await page.context().newCDPSession(page);
          await cdp.send("Emulation.setDeviceMetricsOverride",
            { width: 1440, height: 960, deviceScaleFactor: dpr, mobile: false });
        }
        await openOrbit(page, fixture);
        await page.mouse.move(1, 1);
        await page.evaluate(async (theme) => {
          if (theme === "light") document.documentElement.dataset.theme = "light";
          else delete document.documentElement.dataset.theme;
          await window.__ORBIT_WAIT__(1_200);
          const state = window.__ORBIT_STATE__?.();
          if (state?.camera) window.__ORBIT_HOLD_CAMERA__(agentOrbitAim(state), ORBIT.camera.pitch);
          await window.__ORBIT_FRAMES__(4);
        }, theme);
        const path = join(dir, `3d-${size}-${theme}.png`);
        await page.locator("#board-view .agent-graph-surface").screenshot({ path, caret: "hide" });
        shots.push(path);
      } finally {
        await page.close();
      }
    }
  }
  return shots;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const option = (name, fallback) => {
    const at = process.argv.indexOf(name);
    return at > 0 && process.argv[at + 1] ? process.argv[at + 1] : fallback;
  };
  const engine = option("--engine", "chromium");
  const perfOnly = process.argv.includes("--perf");
  const shotDir = option("--shots", null);
  const dpr = Number(option("--dpr", 1));
  const { files, origin } = await createWindowServer();
  const browserType = engine === "webkit" ? webkitType() : chromium;
  const browser = await browserType.launch();
  const lines = [];
  const report = (name, pass, detail = "") =>
    lines.push(`${pass ? "PASS" : "FAIL"}  ${name}${detail ? `  — ${detail}` : ""}`);
  const tables = [];
  const measures = [];
  try {
    if (shotDir) {
      const shots = await shootBoardOrbit(browser, origin, resolve(shotDir), dpr);
      console.log(shots.join("\n"));
    } else {
      if (!perfOnly) await testBoardOrbit(browser, origin, report);
      /* 판의 크기 — 기본은 셋(6·20·60). 크기를 주면 그 한 판. */
      const given = Object.fromEntries(["workspaces", "agents", "links", "fresh", "deps"]
        .filter((name) => option(`--${name}`, null) !== null)
        .map((name) => [name, Number(option(`--${name}`, 0))]));
      const fixtures = Object.keys(given).length > 0 ? [given] : Object.values(ORBIT_SIZES);
      for (const fixture of fixtures) {
        const { page } = await openWindowTestPage(browser, origin);
        await page.setViewportSize({ width: 1440, height: 960 });
        /* 설치 앱의 화면은 2배 밀도다. Chromium은 그 밀도를 CDP로 흉내 낸다 — 캔버스가 네 배의
         * 픽셀을 칠하는 판의 값을 같은 표로 잰다. */
        if (dpr !== 1 && engine === "chromium") {
          const cdp = await page.context().newCDPSession(page);
          await cdp.send("Emulation.setDeviceMetricsOverride",
            { width: 1440, height: 960, deviceScaleFactor: dpr, mobile: false });
        }
        const measured = await measureBoardOrbit(page, { seconds: Number(option("--seconds", 3)), fixture });
        await page.close();
        const verdict = orbitVerdict(measured);
        report(`3D view weight on ${engine}, ${measured.fixture.agents} agents: frame p95 ≤ ${ORBIT_FRAME_BUDGET_MS} ms turning and still, no re-projection while still, held/hidden/reduced rAF 0, only label style written, DOM budget`,
          verdict.holds, JSON.stringify(verdict.checks ?? verdict.why));
        tables.push(orbitTable(measured, engine));
        measures.push({ measured, verdict });
      }
    }
  } finally {
    await browser.close();
    files.close();
  }
  const out = option("--json", null);
  if (out) writeFileSync(out, `${JSON.stringify({ engine, measures }, null, 2)}\n`);
  if (!shotDir) {
    console.log(lines.join("\n"));
    for (const table of tables) console.log(`\n${table}`);
    console.log(`\n${lines.filter((line) => line.startsWith("PASS")).length}/${lines.length} passed`);
    process.exit(lines.some((line) => line.startsWith("FAIL")) ? 1 : 0);
  }
}
