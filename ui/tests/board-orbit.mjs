/* t-9444 → t-10118 · 관계 탭의 「입체」 보기 — 회귀와 무게.
 *
 * 워크스페이스는 떠 있는 판(메인이 왼쪽의 본진, 나머지는 오른쪽의 섬)이고 에이전트는 그
 * 위의 로봇이며, 지시는 빛의 레일을 따라 왼쪽에서 오른쪽으로 흐르고, 직교 카메라가 그것을
 * 비스듬히 내려다본다(`ui/shell-board-orbit.js`, three.js 위의 WebGL2). 픽스처는
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
 *   ⑨ 자리: 같은 입력은 같은 자리이고, 에이전트 하나가 와도 남의 자리는 움직이지 않는다.
 *      메인 워크스페이스(본진)가 가장 왼쪽이고, 다른 판의 지시를 받은 섬은 지시한 판의
 *      오른쪽에 서며, 판끼리는 60 판에서도 겹치지 않는다. 에이전트는 제 판 위에, 하위
 *      에이전트는 부모 곁에, 조율자는 단 위에 1.5배로 앉는다. 이름표는 6·20·60 판과 좁은
 *      판에서 서로 겹치지 않는다(비킬 자리가 없으면 칩으로 접힌다). (흐름 보드에서 다시 씀 —
 *      구면의 자리를 흐름의 자리로.)
 *   ⑩ 투영: 직교다 — 화면의 자리는 따로 셈한 직교 신탁과 같고, 카메라를 90° 돌리면 화면의
 *      가로가 세계의 x 대신 z를 따르며, 같은 길이는 깊이와 무관하게 같은 길이로 그려진다.
 *      (흐름 보드에서 다시 씀 — 원근·안개·화가의 차례를 직교로.)
 *   ⑪ 레일: 계보는 덮개를 두른 빛의 관, 의존은 덮개 없는 가는 관이고 구조는 선이 아니라
 *      앉은 자리다. 방금 오간 우편만 보낸 쪽 → 받는 쪽의 호를 탄다. 카메라가 멈춘 프레임은
 *      이름표를 다시 앉히지 않고, 프레임이 쓰는 DOM은 라벨의 style뿐이다.
 *   ⑫ 카메라: 끌면 돌고 놓으면 미끄러져 선다; 두 번 누르면 맞춤으로 돌아가 다시 돈다.
 *   ⑬ 그릴 수 없는 창: WebGL2가 없거나 컨텍스트를 잃은 창은 카드로 서고, 입체 단추는 누를
 *      수 없는 채로 까닭을 말한다; 컨텍스트가 돌아오면 입체로 다시 선다. 토글을 오가도 GPU의
 *      자원(`renderer.info.memory`)은 자라지 않는다. 둘째 칸에 선 판을 닫아도 첫 판은 입체로 선다.
 *
 * 헤드리스 Chromium은 WebGL2를 기본으로 주지 않는다 — 이 시험은 SwiftShader를 켠 판
 * (`ORBIT_GL_ARGS`)에서 돈다(받은 브라우저가 WebGL2를 주지 않으면 그 곁에 하나를 띄운다).
 * 그래서 이 판의 프레임 시간은 소프트웨어 래스터라이저의 것이 아니라 JS의 것을 잰다.
 *
 *   node ui/tests/board-orbit.mjs                     기능 시험 + 무게 표(6·20·60, Chromium)
 *   node ui/tests/board-orbit.mjs --perf --engine webkit --json out.json
 *   node ui/tests/board-orbit.mjs --perf --dpr 2       2배 밀도(Chromium, CDP)
 *   node ui/tests/board-orbit.mjs --shots <dir>        6·20·60 × 다크·라이트 사진 여섯 장 + 좁은 판 한 장
 *   node ui/tests/board-orbit.mjs --main-shots <dir>   마지막 입력이 「계속」인 메인 판: 입체·카드·사이드바 한 장씩
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
export function orbitFixture({ workspaces = 4, agents = 12, links = 40, fresh = 20, deps = 0, calm = false,
  chain = false } = {}) {
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
  /* 고요한 판(`calm`): 작업 중과 확인 필요가 대기로 선다 — 흐를 것이 없는 판. */
  const stateOf = (at) => {
    const [bucket, state] = STATES[at % STATES.length];
    return calm && (bucket === "working" || bucket === "attention") ? ["idle", "idle"] : [bucket, state];
  };
  const TASKS = ["조율", "구현", "독립 검증", "문서", "리팩터", "성능 측정", "릴리즈 노트",
    "재현", "보안 검토", "마이그레이션", "시험 보강", "정리"];
  /* 이름은 겹치지 않는다: 한 바퀴를 넘으면 조율을 뺀 이름에 바퀴의 번호가 붙는다(「구현 2」). */
  const taskOf = (at) => at < TASKS.length ? TASKS[at]
    : `${TASKS[((at - TASKS.length) % (TASKS.length - 1)) + 1]} ${Math.floor((at - TASKS.length) / (TASKS.length - 1)) + 2}`;
  /* 하위 에이전트: 열둘마다 5·10·11번째, 부모는 워크스페이스 수만큼 앞 — 같은 워크스페이스다
   * (기본 판에서 5→1·10→6·11→7). 그리고 조율자: 메인의 첫 에이전트가 섬마다 첫 에이전트에게
   * 지시했다(1·2·3 → 0) — 판을 건너는 계보. */
  const MOONS = new Map(Array.from({ length: agents }, (_, at) => at)
    .filter((at) => [5, 10, 11].includes(at % 12) && at - workspaces >= 0)
    .map((at) => [at, at - workspaces]));
  /* `chain`: 섬마다 첫 에이전트가 앞 섬의 첫 에이전트에게서 지시를 받는다 — 섬이 열 하나씩 오른쪽에 선다. */
  for (let at = 1; at < Math.min(workspaces, agents); at += 1) MOONS.set(at, chain ? at - 1 : 0);
  const terms = Array.from({ length: agents }, (_, at) => 401 + at);
  const treeOf = (at) => trees[at % trees.length];
  const branchOf = (at) => (at % trees.length === 0 ? "main" : `wt/w${at % trees.length}`);
  const parentOf = (at) => (MOONS.has(at) ? terms[MOONS.get(at)] : null);
  /* 모델: 셋에 하나가 Codex, 나머지는 Claude — 몸의 두 계열이 한 판에 함께 선다. */
  const modelOf = (at) => (at % 3 === 2 ? "codex" : "claude");

  window.__PANES__ = terms.map((term, at) => ({
    term, agent: modelOf(at), state: stateOf(at)[1],
    at: now - 10_000, state_started_at: now - 10_000, resumable: false,
    ...(parentOf(at) ? { parent: parentOf(at) } : {}),
  }));
  window.__LEDGER__ = terms.map((term, at) => ({
    run: "run-o1", worker: `w-${term}`, agent: modelOf(at), state: "working", ledger: "active",
    hearing: "pending", hearing_at: now - 120_000, checkout: treeOf(at), task: taskOf(at),
    task_id: `t-o${at}`, reported: false, review: null, dispatch_id: `dp-o${at}`,
    dispatch_started_ms: now - 300_000, retry_of: null, term, at: now - 300_000, model: null,
    effort: null, pane: `%${at}`, asking: false, wall: null, quiet_at: null, pane_missing_since_ms: null,
  }));
  const columns = new Map([["attention", []], ["working", []], ["done", []], ["idle", []]]);
  terms.forEach((term, at) => {
    const [bucket, state] = stateOf(at);
    columns.get(bucket).push({
      pane: `term:${term}`, heading: taskOf(at), state, agent: modelOf(at),
      project: ROOT, worktree: branchOf(at), task: taskOf(at), you: "", said: "",
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
    if (stateOf(at)[0] !== "working") return;
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
    /* 신탁: 세계의 한 점을 직교 카메라로. 카메라는 과녁에서 (yaw, pitch) 쪽에 서서 과녁을
     * 보고, 화면의 가로는 세로축과 시선의 외적, 세로는 그 둘의 외적이다. 크기는 깊이와 무관한
     * 배율 하나다. 프로덕션의 셈(three.js의 행렬)을 부르지 않고 따로 셈한다: 두 셈이 같아야
     * 그림이 그 카메라에서 온 것이다. */
    window.__ORBIT_ORACLE__ = ([x, y, z], lens) => {
      const [dx, dy, dz] = [x - lens.target[0], y - lens.target[1], z - lens.target[2]];
      const [cosYaw, sinYaw] = [Math.cos(lens.yaw), Math.sin(lens.yaw)];
      const [cosPitch, sinPitch] = [Math.cos(lens.pitch), Math.sin(lens.pitch)];
      const across = dx * cosYaw - dz * sinYaw;
      const up = -dx * sinPitch * sinYaw + dy * cosPitch - dz * sinPitch * cosYaw;
      return { x: lens.cx + lens.scale * across, y: lens.cy - lens.scale * up };
    };
    window.__ORBIT_BEZIER__ = ([a, c, b], t) => [0, 1, 2]
      .map((axis) => (1 - t) ** 2 * a[axis] + 2 * (1 - t) * t * c[axis] + t ** 2 * b[axis]);
  });
}

export const openOrbit = async (page, fixture = {}) => {
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
export async function testBoardOrbit(given, origin, ok) {
  const { browser, own } = await orbitGlBrowser(given);
  const parts = [
    ["table", () => testOrbitShape(ok)],
    ["fallback", () => testOrbitFallback(browser, origin, ok)],
    ["view", () => testOrbitView(browser, origin, ok)],
    ["space", () => testOrbitSpace(browser, origin, ok)],
    ["camera", () => testOrbitCamera(browser, origin, ok)],
    ["gates", () => testOrbitGates(browser, origin, ok)],
    ["stillness", () => testOrbitStillness(browser, origin, ok)],
    ["memory", () => testOrbitRemembered(browser, origin, ok)],
    ["cards", () => testOrbitLeavesTheCardsAlone(browser, origin, ok)],
    ["live", () => testOrbitLiveMap(browser, origin, ok)],
    ["main name", () => testOrbitMainName(browser, origin, ok)],
  ];
  try {
    for (const [name, part] of parts) {
      try {
        await part();
      } catch (error) {
        ok(`the 3D view ${name} checks ran to their end`, false,
          String(error?.message ?? error).split("\n").slice(0, 2).join(" | "));
      }
    }
  } finally {
    if (own) await browser.close();
  }
}

/* 손(올리기·누르기)의 기다림. SwiftShader의 한 장은 붐비는 기계에서 1초에 가깝고, 손은 그림
 * 몇 장을 기다린다 — 5초는 그 판에서 모자랐다. */
const ORBIT_ACTION_MS = 20_000;

/* 헤드리스 Chromium에 WebGL2를 주는 손잡이 — SwiftShader(소프트웨어 래스터라이저). */
export const ORBIT_GL_ARGS = ["--enable-unsafe-swiftshader"];

/* WebGL2를 주는 브라우저: 받은 것이 주면 그것, 아니면(창 전체 시험의 Chromium) 그 곁에
 * SwiftShader를 켠 하나를 띄워 이 묶음이 닫는다. */
async function orbitGlBrowser(browser) {
  const context = await browser.newContext();
  const page = await context.newPage();
  const capable = await page.evaluate(() => Boolean(document.createElement("canvas").getContext("webgl2")));
  await context.close();
  if (capable || browser.browserType().name() !== "chromium") return { browser, own: false };
  return { browser: await browser.browserType().launch({ args: ORBIT_GL_ARGS }), own: true };
}

/* ⑬ 그릴 수 없는 창. WebGL2를 주지 않는 창(초기 스크립트가 `webgl2`를 거절한다)은 카드로
 * 서고 입체 단추는 까닭을 말한다. 그릴 수 있는 창에서는 컨텍스트를 잃으면 카드로, 돌아오면
 * 입체로 선다. 토글을 오가도 GPU의 자원은 자라지 않는다. */
async function testOrbitFallback(browser, origin, ok) {
  const denied = await openWindowTestPage(browser, origin, { before: (page) => page.addInitScript(() => {
    const context = HTMLCanvasElement.prototype.getContext;
    HTMLCanvasElement.prototype.getContext = function getContext(kind, ...rest) {
      return kind === "webgl2" ? null : context.call(this, kind, ...rest);
    };
  }) });
  try {
    await openOrbit(denied.page);
    const plain = await denied.page.evaluate(async () => {
      const view = document.querySelector("#board-view");
      const button = view.querySelector('[data-relations-view="orbit"]');
      button.click();
      await window.__BOARD_SETTLED__();
      return { showing: agentOrbitShowing(view), broken: window.__ORBIT__()?.broken,
        cards: view.querySelectorAll(".agent-graph-node.is-agent").length,
        scroll: view.querySelector(".agent-graph-scroll")?.hidden === false,
        disabled: button.getAttribute("aria-disabled"), tip: button.dataset.tip ?? "",
        pressed: view.querySelector('[data-relations-view="cards"]').getAttribute("aria-pressed") };
    });
    ok("a window without WebGL2 stands on the card view and the 3D button says why",
      plain.showing === false && plain.broken === "unsupported" && plain.cards > 0 && plain.scroll && plain.disabled === "true"
        && plain.pressed === "true" && plain.tip.length > 0, JSON.stringify(plain));
    ok("the no-WebGL2 page raises no browser errors", denied.faults.length === 0, denied.faults.join("\n"));
  } finally {
    await denied.page.close();
  }
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await openOrbit(page);
    const lost = await page.evaluate(async () => {
      const view = document.querySelector("#board-view");
      const state = window.__ORBIT_STATE__();
      const loser = state.gl.renderer.getContext().getExtension("WEBGL_lose_context");
      loser.loseContext();
      await window.__ORBIT_WAIT__(300);
      await window.__BOARD_SETTLED__();
      const button = view.querySelector('[data-relations-view="orbit"]');
      const during = { showing: agentOrbitShowing(view), broken: window.__ORBIT__().broken,
        disabled: button.getAttribute("aria-disabled"), tip: button.dataset.tip ?? "",
        cards: view.querySelectorAll(".agent-graph-node.is-agent").length,
        scroll: view.querySelector(".agent-graph-scroll")?.hidden === false };
      loser.restoreContext();
      await window.__ORBIT_WAIT__(500);
      await window.__BOARD_SETTLED__();
      await window.__ORBIT_FRAMES__(4);
      return { during, after: { showing: agentOrbitShowing(view), broken: window.__ORBIT__().broken,
        frames: window.__ORBIT__().frames, labels: window.__ORBIT__().labels } };
    });
    ok("a lost graphics context stands the board on cards with the reason, and its return stands the 3D view again",
      lost.during.showing === false && lost.during.broken === "lost" && lost.during.disabled === "true"
        && lost.during.tip.length > 0 && lost.during.cards > 0 && lost.during.scroll && lost.after.showing === true
        && lost.after.broken === null && lost.after.labels === 16, JSON.stringify(lost));
    const memory = await page.evaluate(async () => {
      const view = document.querySelector("#board-view");
      const read = () => {
        const info = window.__ORBIT_STATE__().gl.renderer.info;
        return { geometries: info.memory.geometries, textures: info.memory.textures, programs: info.programs.length };
      };
      /* 판이 서지 않으면 무엇이 남았는지 한 줄로 말한다: 그림의 모형, 가장자리·맞춤 박자, 새로 읽기, 칠하기
       * 박자, 받는 중인 것, 그리고 입체 판의 상태. */
      const settle = async (step) => {
        try {
          await window.__BOARD_SETTLED__();
        } catch (error) {
          const state = agentOrbitStates.get(view);
          throw new Error(`${error.message} after ${step}: ${JSON.stringify({ model: agentGraphModels.has(view),
            edge: agentGraphEdgeFrames.has(view), fit: agentGraphFitFrames.has(view), refreshing: agentGraphRefreshing,
            paint: agentPaintFrame, flight: ["board_snapshot", "pane_agents", "ledger_agents"]
              .map((name) => window.__IN_FLIGHT__.get(name) ?? 0), showing: agentOrbitShowing(view),
            broken: window.__ORBIT__().broken, scene: Boolean(state?.scene), dirty: state?.dirty ?? null,
            moving: state ? agentOrbitMoving(state) : null })}`);
        }
      };
      const flip = async (round) => {
        view.querySelector('[data-relations-view="cards"]').click();
        await settle(`cards ${round}`);
        view.querySelector('[data-relations-view="orbit"]').click();
        await settle(`3D ${round}`);
        await window.__ORBIT_FRAMES__(4);
      };
      await flip(0);
      const first = read();
      for (let round = 1; round <= 5; round += 1) await flip(round);
      return { first, last: read(), builds: window.__ORBIT__().builds };
    }).catch((error) => {
      /* 앞의 두 줄만 보고되니, 그때까지의 브라우저 오류를 첫 줄에 붙인다. */
      const [head] = String(error.message).split("\n");
      throw new Error(`${head} · browser errors so far: ${faults.join(" ¶ ").replace(/\s+/g, " ").slice(0, 900) || "none"}`);
    });
    ok("toggling between the views five times grows no GPU resource",
      JSON.stringify(memory.first) === JSON.stringify(memory.last) && memory.first.geometries > 0, JSON.stringify(memory));

    /* 둘째 칸에 선 판(첫 판의 복제)을 닫는다: 그 판의 컨텍스트는 놓이고, 그 유실의 소식이 창의 그림을
     * 카드로 넘기지 않는다 — 첫 판은 입체로 선다. */
    const panes = await page.evaluate(async () => {
      const first = document.querySelector("#board-view");
      const tab = boardTab();
      const added = nextGroupId;
      nextGroupId += 1;
      setStageTree(splitStageLeaf(stageTree(), 0, added, "horizontal", "second"));
      tab.pane = added;
      setActiveTab(tab.id);
      renderTabs();
      updateStage();
      await paintBoardView(tab, { force: true });
      const second = docHost(added, "board");
      for (let wait = 0; wait < 100 && !agentOrbitStates.has(second); wait += 1) await window.__ORBIT_WAIT__(100);
      await window.__ORBIT_FRAMES__(4);
      const context = agentOrbitStates.get(second)?.gl?.renderer.getContext() ?? null;
      const opened = { apart: second !== first, showing: agentOrbitShowing(second), context: Boolean(context) };
      tab.pane = 0;
      setActiveTab(tab.id);
      collapseStageGroup(added);
      renderTabs();
      updateStage();
      await paintBoardView(tab, { force: true });
      await window.__BOARD_SETTLED__();
      await window.__ORBIT_FRAMES__(4);
      await window.__ORBIT_WAIT__(500);
      await window.__ORBIT_FRAMES__(4);
      return { opened, closed: { connected: second.isConnected, released: !agentOrbitStates.has(second),
        lost: context?.isContextLost() ?? null }, broken: window.__ORBIT__().broken, showing: agentOrbitShowing(first),
        labels: window.__ORBIT__().labels };
    });
    ok("closing a second pane's board lets its context go and leaves the first board standing in 3D",
      panes.opened.apart && panes.opened.showing && panes.opened.context && panes.closed.connected === false
        && panes.closed.released && panes.closed.lost === true && panes.broken === null && panes.showing === true
        && panes.labels === 16, JSON.stringify(panes));
    ok("the context-loss page raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
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
      && hints[0] === "캐릭터나 이름표를 누르면 상세가 열립니다", JSON.stringify({ named, button, hint: hints[0] }));
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
      && shape.words[0] === "입체" && shape.hint === "캐릭터나 이름표를 누르면 상세가 열립니다",
      JSON.stringify(shape));
    ok("the 3D view draws every agent as a robot and every workspace as a platform, each with a button",
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
    await page.hover(`[data-orbit-key="${key}"]`, { force: true, timeout: ORBIT_ACTION_MS });
    await page.evaluate(() => window.__ORBIT_WAIT__(500));
    const held = await page.evaluate((wanted) => {
      const label = document.querySelector(`#board-view [data-orbit-key="${wanted}"]`);
      const before = label?.style.translate;
      return new Promise((done) => setTimeout(() => done({ paused: window.__ORBIT__()?.paused,
        still: label?.style.translate === before }), 200));
    }, key);
    ok("hovering a point stills the camera so it can be clicked", held.paused === true && held.still === true,
      JSON.stringify({ key, ...held }));
    await page.click(`[data-orbit-key="${key}"]`, { timeout: ORBIT_ACTION_MS });
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

/* ⑨ ⑩ ⑪ 자리·투영·레일. 기본 판에 과업 의존 넷을 더한다 — 조율자가 본진에 앉아 섬마다 첫
 * 에이전트에게 지시한 판이다. */
const readSeats = (page) => page.evaluate(() => {
  const state = window.__ORBIT_STATE__();
  return Object.fromEntries([...state.bodies.values()].map((body) => [body.key, body.world
    ? [...body.world] : [body.left, body.right, body.back, body.front]]));
});

/* 판과 이름표의 네모가 서로 겹치는가 — 판은 빛나는 테까지, 이름표는 화면의 네모. */
const orbitOverlaps = (page) => page.evaluate(() => {
  const state = window.__ORBIT_STATE__();
  const plates = state.plates.map((plate) => ({ key: plate.key, x: plate.left - plate.rim, y: plate.back - plate.rim,
    w: plate.right - plate.left + plate.rim * 2, h: plate.front - plate.back + plate.rim * 2 }));
  const hit = (a, b) => a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
  const pairs = (list) => list.flatMap((one, at) => list.slice(at + 1).filter((two) => hit(one, two))
    .map((two) => `${one.key} × ${two.key}`));
  const tags = state.tags;
  const unnamed = [...state.bodies.values()].filter((body) => body.lead && body.named !== "1").map((body) => body.key);
  return { plates: pairs(plates), tags: pairs(tags), named: state.named, labels: state.host.childElementCount, unnamed,
    outside: tags.filter((tag) => tag.x < 0 || tag.y < 0 || tag.x + tag.w > state.width || tag.y + tag.h > state.height)
      .map((tag) => tag.key), width: state.width };
});

/* 여는 카메라의 화면에서: 판의 윗면(네 모서리)끼리 겹치는가, 로봇이 제 판의 윗면 안에 서는가,
 * 로봇(발에서 머리까지)이 다른 판의 윗면을 덮는가, 판이 판 밖이나 흐름 한 줄·범례에 걸리는가,
 * 이름표가 판의 윗면에 서는가. 반 픽셀 안의 닿음은 겹침이 아니다. */
export const orbitClearance = (page) => page.evaluate(() => {
  const state = window.__ORBIT_STATE__();
  const SLACK = 0.5;
  const seen = (x, y, z) => {
    const at = agentOrbitScreen(state, x, y, z, {});
    return [at.x, at.y];
  };
  const top = (plate) => [seen(plate.left, 0, plate.back), seen(plate.right, 0, plate.back),
    seen(plate.right, 0, plate.front), seen(plate.left, 0, plate.front)];
  const crosses = (one, two) => [one, two].every((shape) => shape.every(([ax, ay], at) => {
    const [bx, by] = shape[(at + 1) % shape.length];
    if (ax === bx && ay === by) return true;
    const length = Math.hypot(bx - ax, by - ay);
    const cast = (list) => list.map(([x, y]) => (x * (ay - by) + y * (bx - ax)) / length);
    const [a, b] = [cast(one), cast(two)];
    return Math.min(Math.max(...a) - Math.min(...b), Math.max(...b) - Math.min(...a)) > SLACK;
  }));
  const within = ([px, py], shape) => {
    const sides = shape.map(([ax, ay], at) => {
      const [bx, by] = shape[(at + 1) % shape.length];
      return ((bx - ax) * (py - ay) - (by - ay) * (px - ax)) / Math.hypot(bx - ax, by - ay);
    });
    return sides.every((side) => side >= -SLACK) || sides.every((side) => side <= SLACK);
  };
  const box = (rect) => [[rect.x, rect.y], [rect.x + rect.w, rect.y], [rect.x + rect.w, rect.y + rect.h],
    [rect.x, rect.y + rect.h]];
  const tops = new Map(state.plates.map((plate) => [plate.key, top(plate)]));
  const name = (key) => key.split("/").pop();
  const overlaps = [];
  const plates = [...tops.entries()];
  plates.forEach(([one, first], at) => plates.slice(at + 1).forEach(([two, second]) => {
    if (crosses(first, second)) overlaps.push(`${name(one)} × ${name(two)}`);
  }));
  const strays = [];
  const covers = [];
  for (const body of state.agents) {
    const [x, y, z] = body.world;
    const foot = seen(x, y, z);
    const head = seen(x, y + ORBIT.bot.tall * body.scale, z);
    if (!within(foot, tops.get(body.plate.key))) strays.push(body.name);
    for (const [key, shape] of plates) {
      if (key !== body.plate.key && crosses([foot, head], shape)) covers.push(`${body.name} → ${name(key)}`);
    }
  }
  const reserved = (state.reserved ?? []).map(box);
  const blocked = plates.filter(([, shape]) => shape.some(([x, y]) => x < 0 || y < 0 || x > state.width
    || y > state.height) || reserved.some((rect) => crosses(rect, shape))).map(([key]) => name(key));
  /* 에이전트의 이름표만 — 판의 이름은 제 판 앞 모서리에 붙은 글이다. */
  const tagged = (state.tags ?? []).filter((tag) => state.bodies.get(tag.key)?.kind !== "workspace")
    .filter((tag) => plates.some(([, shape]) => crosses(box(tag), shape)))
    .map((tag) => state.bodies.get(tag.key)?.name ?? tag.key);
  /* 판의 이름(판 앞 모서리 밑의 글, 접히면 그 자리의 칩)은 범례와 겹치지 않는다 — 가장 아래 판의 이름까지.
   * 모듈의 장부가 아니라 문서의 네모로 잰다: 무대 안에 보이는 이름표와 보이는 범례. */
  const stage = document.querySelector("#board-view .agent-orbit");
  const rectOf = (node) => {
    const rect = node?.getBoundingClientRect();
    const look = node ? getComputedStyle(node) : null;
    return rect && rect.width > 0 && rect.height > 0 && look.display !== "none" && look.visibility !== "hidden"
      ? rect : null;
  };
  const meet = (one, two) => Math.min(one.right, two.right) - Math.max(one.left, two.left) > SLACK
    && Math.min(one.bottom, two.bottom) - Math.max(one.top, two.top) > SLACK;
  const room = rectOf(stage);
  const legend = rectOf(stage?.querySelector(".agent-orbit-key"));
  const captions = [...(stage?.querySelectorAll(".agent-orbit-label.is-workspace") ?? [])]
    .map((node) => ({ node, rect: rectOf(node) })).filter(({ rect }) => rect && room && meet(rect, room));
  const lowest = captions.reduce((low, one) => (!low || one.rect.bottom > low.rect.bottom ? one : low), null);
  const said = ({ node }) => node.getAttribute("aria-label")?.split(",")[0] ?? "?";
  const onLegend = legend ? captions.filter(({ rect }) => meet(rect, legend)).map(said) : [];
  return { overlaps, strays, covers, blocked, tagged, reserved: reserved.length, fit: state.fit,
    captions: captions.length, onLegend, legendTop: legend ? Math.round(legend.top - room.top) : null,
    lowest: lowest ? { name: said(lowest), bottom: Math.round(lowest.rect.bottom - room.top),
      onLegend: legend ? meet(lowest.rect, legend) : null } : null };
});

async function testOrbitSpace(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await openOrbit(page, { deps: 4 });
    await page.mouse.move(1, 1);
    await page.evaluate(() => window.__ORBIT_FRAMES__(3));

    /* ⑨ 자리. */
    const places = await page.evaluate(() => {
      const state = window.__ORBIT_STATE__();
      const bodies = [...state.bodies.values()];
      const deck = state.plates.find((plate) => plate.deck);
      const islands = state.plates.filter((plate) => !plate.deck);
      const agents = state.agents;
      const parent = (body) => state.bodies.get(body.parentKey);
      const lead = agents.find((body) => body.lead) ?? null;
      return {
        count: bodies.length,
        deck: deck ? { left: deck.left, right: deck.right, path: deck.workspace.path } : null,
        islands: islands.map((plate) => ({ key: plate.key, left: plate.left })),
        seated: agents.filter((body) => !(body.world[0] > body.plate.left && body.world[0] < body.plate.right
          && body.world[2] > body.plate.back && body.world[2] < body.plate.front)).map((body) => body.key),
        crossing: agents.filter((body) => body.parentKey && parent(body).plate !== body.plate)
          .map((body) => ({ key: body.key, left: body.plate.left, parentRight: parent(body).plate.right })),
        children: agents.filter((body) => body.kind === "child").map((body) => ({ key: body.key,
          gap: Math.hypot(body.world[0] - parent(body).world[0], body.world[2] - parent(body).world[2]),
          seat: body.plate.deck ? ORBIT.deck.seat : ORBIT.island.seat })),
        lead: lead ? { key: lead.key, deck: lead.plate.deck, scale: lead.scale, lift: lead.world[1],
          children: lead.children.length } : null,
        leads: agents.filter((body) => body.lead).length,
        leadScale: ORBIT.bot.scale.lead,
      };
    });
    ok("every agent sits on its own workspace's platform, and the main workspace is the platform on the far left",
      places.count === 16 && places.seated.length === 0 && places.deck?.path === "/repos/orbit"
        && places.deck.right <= 0 && places.islands.length === 3
        && places.islands.every((island) => island.left > places.deck.right), JSON.stringify(places));
    ok("a workspace that took its work from another stands to that one's right",
      places.crossing.length === 3 && places.crossing.every((one) => one.left > one.parentRight), JSON.stringify(places));
    ok("a sub-agent sits beside its parent, and the coordinator stands on the main platform's dais, half again as large",
      places.children.length === 3 && places.children.every((one) => one.gap <= one.seat * 1.5)
        && places.leads === 1 && places.lead?.deck === true && places.lead.scale === places.leadScale
        && places.lead.lift > 0 && places.lead.children === 3, JSON.stringify(places));

    const seats = await readSeats(page);
    const again = await openWindowTestPage(browser, origin);
    let same = null;
    try {
      await openOrbit(again.page, { deps: 4 });
      same = await readSeats(again.page);
    } finally {
      await again.page.close();
    }
    ok("the same input seats every agent and every platform at the same place",
      Object.keys(seats).length === 16 && JSON.stringify(same) === JSON.stringify(seats),
      JSON.stringify({ seats, same }));

    /* ⑩ 투영: 직교. 카메라를 한 자리에 세우고 프로덕션의 투영(`agentOrbitScreen`)을 신탁과 견준다. */
    const projection = await page.evaluate(async () => {
      const state = window.__ORBIT_STATE__();
      const agents = state.agents;
      const look = async (yaw) => {
        window.__ORBIT_HOLD_CAMERA__(yaw, ORBIT.camera.pitch);
        await window.__ORBIT_FRAMES__(3);
        const lens = state.lens;
        const miss = Math.max(...agents.map((body) => {
          const seen = agentOrbitScreen(state, ...body.world);
          const wanted = window.__ORBIT_ORACLE__(body.world, lens);
          return Math.hypot(seen.x - wanted.x, seen.y - wanted.y);
        }));
        /* 세계의 x로 100, z로 100 — 화면의 가로가 무엇을 따르는가. */
        const [x, y, z] = agents[0].world;
        const from = agentOrbitScreen(state, x, y, z);
        const alongX = agentOrbitScreen(state, x + 100, y, z).x - from.x;
        const alongZ = agentOrbitScreen(state, x, y, z + 100).x - from.x;
        /* 같은 길이를 가장 가까운 로봇과 가장 먼 로봇에서 — 직교면 같은 길이다. */
        const depths = agents.map((body) => ({ body, depth: agentOrbitScreen(state, ...body.world).depth }))
          .sort((left, right) => left.depth - right.depth);
        const span = (body) => {
          const [bx, by, bz] = body.world;
          const a = agentOrbitScreen(state, bx, by, bz);
          const b = agentOrbitScreen(state, bx + 100, by, bz);
          return Math.hypot(b.x - a.x, b.y - a.y);
        };
        return { miss, alongX, alongZ, near: span(depths[0].body), far: span(depths[depths.length - 1].body),
          depths: [depths[0].depth, depths[depths.length - 1].depth], scale: lens.scale };
      };
      const front = await look(0);
      const quarter = await look(Math.PI / 2);
      window.__ORBIT_HOLD_CAMERA__(ORBIT.camera.yaw, ORBIT.camera.pitch);
      await window.__ORBIT_FRAMES__(3);
      return { front, quarter };
    });
    const { front, quarter } = projection;
    ok("every robot stands on the screen where an orthographic camera puts it, and a quarter turn swaps x for z",
      front.miss < 0.5 && quarter.miss < 0.5 && Math.abs(front.alongX - 100 * front.scale) < 0.01
        && Math.abs(front.alongZ) < 0.01 && Math.abs(quarter.alongX) < 0.01
        && Math.abs(quarter.alongZ + 100 * quarter.scale) < 0.01, JSON.stringify(projection));
    ok("nothing is drawn larger for being nearer: the same length is the same on the screen at any depth",
      front.depths[1] - front.depths[0] > 50 && Math.abs(front.near - front.far) < 0.01, JSON.stringify(front));

    /* ⑪ 레일과 우편. */
    const rails = await page.evaluate(() => {
      const state = window.__ORBIT_STATE__();
      const flowing = state.links.filter((link) => link.flowing);
      const on = flowing.map((link) => {
        const wanted = agentOrbitAlong(link.curve, link.spot, [0, 0, 0]);
        return Math.hypot(...wanted.map((value, axis) => value - link.head[axis]));
      });
      const ends = flowing.every((link) => link.curve[0][0] === link.from.world[0] && link.curve[0][2] === link.from.world[2]
        && link.curve[2][0] === link.to.world[0] && link.curve[2][2] === link.to.world[2]
        && link.curve[1][1] > Math.max(link.curve[0][1], link.curve[2][1]));
      return {
        handles: window.__ORBIT__(),
        sheathed: state.rails.filter((rail) => rail.kind === "spawned").every((rail) => rail.sheathRange !== null),
        bare: state.rails.filter((rail) => rail.kind === "dependency").every((rail) => rail.sheathRange === null),
        structure: state.rails.filter((rail) => rail.from.kind === "workspace" || rail.to.kind === "workspace").length,
        fresh: state.links.filter((link) => link.fresh).length, on: Math.max(...on), ends,
      };
    });
    ok("lineage is a sheathed rail, a dependency a bare thin rail, and no line runs to a workspace",
      rails.handles.edges.spawned === 6 && rails.handles.edges.dependency === 4 && rails.sheathed && rails.bare
        && rails.structure === 0, JSON.stringify({ ...rails, handles: rails.handles.edges }));
    ok("only fresh mail flows, from the sender's arc end to the receiver's, riding the arc",
      rails.handles.links === 40 && rails.handles.particles === 20 && rails.fresh === 20 && rails.on < 1e-6 && rails.ends,
      JSON.stringify({ ...rails, handles: undefined, particles: rails.handles.particles }));

    /* 멈춘 카메라: 알갱이는 흐르고 이름표는 다시 앉지 않는다; 카메라가 돌면 다시 앉는다. */
    const cache = await page.evaluate(async () => {
      window.__ORBIT_HOLD_CAMERA__(ORBIT.camera.yaw, ORBIT.camera.pitch);
      await window.__ORBIT_FRAMES__(3);
      const state = window.__ORBIT_STATE__();
      state.flowing = true;
      const frames = window.__ORBIT__().frames;
      const projections = window.__ORBIT__().projections;
      await window.__ORBIT_FRAMES__(20);
      const held = { frames: window.__ORBIT__().frames - frames, projections: window.__ORBIT__().projections - projections };
      state.camera.spin = true;
      state.dirty = true;
      agentOrbitWake();
      await window.__ORBIT_FRAMES__(5);
      return { ...held, afterTurn: window.__ORBIT__().projections - projections - held.projections };
    });
    ok("a still camera leaves the name tags where they stand while the comets flow",
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

    /* ⑨ 빈자리에 앉는 에이전트 하나는 아무도 움직이지 않는다 — 세계에서도, 멈춘 카메라의 화면에서도
     * (판이 자라지 않으니 맞춤도 그대로다). 판을 자라게 하는 손님은 아래(`growth`)에서 따로 본다. */
    const arrival = await page.evaluate(async () => {
      window.__ORBIT_HOLD_CAMERA__(ORBIT.camera.yaw, ORBIT.camera.pitch);
      await window.__ORBIT_FRAMES__(3);
      const state = window.__ORBIT_STATE__();
      const read = () => Object.fromEntries([...state.bodies.values()].filter((body) => body.world).map((body) =>
        [body.key, { world: [...body.world], x: body.seen.x, y: body.seen.y }]));
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
    ok("an agent that takes a spare seat moves no one, on the platforms or on the screen",
      arrival.arrived && arrival.bodies === 13 && arrival.shifted.length === 0, JSON.stringify(arrival));
    ok("the space page raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }

  /* ⑨ 섬을 자라게 하는 손님들: 섬마다 열이 하나씩 오른쪽에 서는 판(`chain`)에서 첫 섬의 빈자리를 채우고
   * 자랄 몫(`island.growth`열)을 다 쓴다. 남의 자리와 다른 판은 세계에서 그대로이고(화면은 맞춤이 다시
   * 서니 움직여도 된다), 자란 섬과 다음 열 사이에는 열의 틈이 남는다. */
  const grown = await openWindowTestPage(browser, origin);
  try {
    await openOrbit(grown.page, { chain: true });
    await grown.page.mouse.move(1, 1);
    const growth = await grown.page.evaluate(async () => {
      const state = window.__ORBIT_STATE__();
      const plate = state.bodies.get("agent:term:402").plate;
      const read = () => Object.fromEntries([...state.bodies.values()].map((body) => [body.key, body.world
        ? [...body.world] : [body.left, body.right, body.back, body.front]]));
      const before = read();
      const row = state.rows.get(plate.key);
      const count = row.rows * row.cols - plate.members.length + ORBIT.island.growth * row.rows;
      const now = window.__ORBIT_NOW__;
      const working = window.__COLUMNS__.find((column) => column.bucket === "working");
      const added = [];
      for (let at = 0; at < count; at += 1) {
        const term = 420 + at;
        window.__PANES__.push({ term, agent: "claude", state: "working", at: now - 5_000,
          state_started_at: now - 5_000, resumable: false });
        window.__LEDGER__.push({ ...window.__LEDGER__[1], worker: `w-${term}`, task: `자람 ${term}`,
          task_id: `t-g${term}`, dispatch_id: `dp-g${term}`, term, pane: `%${term}` });
        working.cards.push({ ...working.cards[0], pane: `term:${term}`, heading: `자람 ${term}`, task: `자람 ${term}`,
          worktree: "wt/w1", parent: "" });
        added.push(`agent:term:${term}`);
      }
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
      await window.__ORBIT_FRAMES__(3);
      const after = read();
      const column = state.columns.get(plate.key);
      const later = state.plates.filter((one) => !one.deck && state.columns.get(one.key) > column);
      return { count, seated: added.filter((key) => {
        const body = state.bodies.get(key);
        return body?.plate === plate && body.world[0] > plate.left && body.world[0] < plate.right;
      }).length,
        columns: [state.columns.get(plate.key), ...later.map((one) => state.columns.get(one.key))],
        grew: after[plate.key][1] - before[plate.key][1], wanted: ORBIT.island.growth * ORBIT.island.seat,
        moved: Object.keys(before).filter((key) => key !== plate.key && JSON.stringify(before[key]) !== JSON.stringify(after[key])),
        gap: later.length > 0 ? Math.min(...later.map((one) => one.left)) - plate.right : null, column: ORBIT.flow.column };
    });
    const worlds = await orbitOverlaps(grown.page);
    ok("agents that grow an island by its whole allowance move no one else, and the island stays a column's gap "
      + "short of the next column",
    growth.seated === growth.count && growth.count > 0 && growth.grew === growth.wanted && growth.moved.length === 0
      && growth.columns.length >= 2 && growth.gap >= growth.column && worlds.plates.length === 0,
    JSON.stringify({ ...growth, overlaps: worlds.plates }));
    ok("the growing-island page raises no browser errors", grown.faults.length === 0, grown.faults.join("\n"));
  } finally {
    await grown.page.close();
  }

  /* ⑨ 판과 이름표가 겹치지 않는다 — 6·20·60 판과 좁은 판에서. */
  const crowds = [];
  for (const [size, width] of [...Object.keys(ORBIT_SIZES).map((size) => [size, 1440]), ["60", 1024]]) {
    const one = await openWindowTestPage(browser, origin);
    try {
      await one.page.setViewportSize({ width, height: 960 });
      await openOrbit(one.page, ORBIT_SIZES[size]);
      await one.page.mouse.move(1, 1);
      await one.page.evaluate(() => {
        window.__ORBIT_HOLD_CAMERA__(ORBIT.camera.yaw, ORBIT.camera.pitch);
        return window.__ORBIT_FRAMES__(4);
      });
      crowds.push({ size, ...(await orbitOverlaps(one.page)), viewport: width, clear: await orbitClearance(one.page),
        faults: one.faults.length });
    } finally {
      await one.page.close();
    }
  }
  ok("no two platforms overlap and no two name tags overlap, at 6, 20 and 60 agents and on a narrow board",
    crowds.length === 4 && crowds.every((one) => one.plates.length === 0 && one.tags.length === 0
      && one.outside.length === 0 && one.named > 0 && one.faults === 0),
    JSON.stringify(crowds.map(({ clear, ...one }) => one)));
  /* 이름이 서는 몫의 바닥: 조율자는 늘 이름으로 서고, 판마다 이름표의 이만큼은 칩으로 접히지 않는다
   * (60 판은 판의 자리가 모자라 본진의 에이전트가 칩으로 접힌다 — 그래서 바닥이 낮다). */
  const floors = { "6:1440": 0.75, "20:1440": 0.5, "60:1440": 0.2, "60:1024": 0.12 };
  ok("the coordinator is always named, and on every board a floor of the labels stand named rather than folded",
    crowds.length === 4 && crowds.every((one) => one.unnamed.length === 0
      && one.named >= one.labels * floors[`${one.size}:${one.viewport}`]),
    JSON.stringify(crowds.map((one) => ({ size: one.size, viewport: one.viewport, named: `${one.named}/${one.labels}`,
      floor: floors[`${one.size}:${one.viewport}`], unnamed: one.unnamed }))));
  const clearOf = (pick) => JSON.stringify(crowds.map((one) => ({ size: one.size, viewport: one.viewport,
    fit: one.clear.fit, ...pick(one.clear) })));
  ok("on the screen no two platform tops overlap, every robot stands inside its own platform's top, and no robot "
    + "covers another platform, at 6, 20 and 60 agents and on a narrow board",
  crowds.length === 4 && crowds.every(({ clear }) => clear.overlaps.length === 0 && clear.strays.length === 0
    && clear.covers.length === 0),
  clearOf(({ overlaps, strays, covers }) => ({ overlaps, strays, covers })));
  ok("every platform stands inside the stage, clear of the glance line and the legend",
    crowds.length === 4 && crowds.every(({ clear }) => clear.blocked.length === 0 && clear.reserved === 2),
    clearOf(({ blocked, reserved }) => ({ blocked, reserved })));
  ok("every platform caption, down to the lowest, clears the legend box",
    crowds.length === 4 && crowds.every(({ clear }) => clear.captions > 0 && clear.legendTop !== null
      && clear.lowest?.onLegend === false && clear.onLegend.length === 0),
    clearOf(({ captions, lowest, legendTop, onLegend }) => ({ captions, lowest, legendTop, onLegend })));
  ok("no agent's name tag stands on a platform's top", crowds.length === 4 && crowds.every(({ clear }) => clear.tagged.length === 0),
    clearOf(({ tagged }) => ({ tagged })));
}

/* ⑫ 카메라. 흐를 것이 없는 판 — 방금 오간 우편 0, 작업 중·확인 필요 0(`calm`): 멈춘 카메라의
 * 판에 움직일 것이 없다. 끄는 길이는 표의 yaw 폭(여는 자리에서 ±`range`) 안이다. */
async function testOrbitCamera(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await openOrbit(page, { fresh: 0, calm: true });
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
    await page.mouse.move(corner.x + 60, corner.y + 40, { steps: 4 });
    await page.mouse.move(corner.x + 120, corner.y + 80, { steps: 4 });
    const dragged = await camera();
    await page.mouse.up();
    ok("dragging turns the camera: across is yaw, down is pitch",
      dragged.spin === false && Math.abs((dragged.yaw - before.yaw) - 120 * turn) < 120 * turn * 0.25
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
    await page.hover(`[data-orbit-key="${key}"]`, { force: true, timeout: ORBIT_ACTION_MS });
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

/* t-11540 · 마지막 입력이 「계속」인 메인 판. 조율자 하나가 메인 워크스페이스에서 워커 둘을
 * 부렸고, 사람은 그 판에 뜻 있는 요청을 한 번 쳤고, 창은 우편 안내를 한 번 쳤고, 사람은 마지막에
 * 「계속」을 쳤다 — 셋 다 프로덕션의 한 문(`hook:agent`)으로 들어간다. 이어 가라는 말과 안내는 보고가
 * 「이름 아님」이라 싣는다(판정은 core의 한 표). 조율자의 이름은 앞의 요청이어야 한다. */
export function mainNameFixture() {
  const LEAD = 7101;
  const WORKERS = [7102, 7103];
  const ASKED = "보드 이름 고치고 릴리즈까지";
  const path = projects[0].worktrees[0].path;
  const now = Date.now();
  for (const term of [LEAD, ...WORKERS]) mountTermTab(term, { agent: "claude", worktree: path }, { focus: false, placement: "tab" });
  window.__PANES__ = [LEAD, ...WORKERS].map((term) => ({
    term, agent: "claude", state: "working", at: now, state_started_at: now, resumable: false,
    ...(term === LEAD ? {} : { parent: LEAD }),
  }));
  window.__LEDGER__ = [];
  window.__OVERLAYS__ = { latest: null, mail: [], dependencies: [], task_dependencies: [], merge: [] };
  window.__ANSWER__.board_columns = ({ cards }) => ["attention", "working", "done", "idle"].map((bucket) => ({
    bucket,
    cards: cards.filter((card) => card.state === bucket).map((card) => ({
      lineage: { depth: 0, is_first_sibling: true, is_last_sibling: true, child_count: 0 }, ...card,
    })),
  }));
  const said = (term, prompt, nothing) => {
    for (const listener of window.__LISTENERS__["hook:agent"] ?? []) {
      listener({ payload: { term, agent: "claude", state: "working", event: "UserPromptSubmit", resumable: false,
        prompt, ...(nothing ? { prompt_names_nothing: true } : {}) } });
    }
  };
  said(WORKERS[0], "사이드바 이름 한 함수로", false);
  said(WORKERS[1], "입체 보기 이름표 시험", false);
  said(LEAD, ASKED, false);
  said(LEAD, "You have 1 orchestration message. Run `zerocode-orc check`.", true);
  said(LEAD, "계속", true);
  agentBoardMode = "graph";
  agentGraphSelectedKey = null;
  agentGraphScopeKey = "";
  boardQuery = "";
  if (typeof activeTabId !== "undefined" && activeTabId === "board") dropTab("board");
  openBoard();
  return { lead: LEAD, asked: ASKED, path };
}

/* 조율자 판의 이름은 어디서나 하나다: 사이드바의 행, 상황판의 카드와 상세 패널, 입체 보기의
 * 이름표, 한 줄 요약의 주어 — 모두 앞의 뜻 있는 요청이고 「계속」은 어디에도 없다. 요약은
 * 이미 있는 문장 그대로 지시한 작업의 수를 말한다(새 문구 0). */
export async function testOrbitMainName(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await installBoardWaits(page);
    await installOrbitCounters(page);
    const { lead, asked } = await page.evaluate(mainNameFixture);
    await page.evaluate(() => window.__BOARD_SETTLED__());
    await page.waitForFunction((key) => document.querySelector(`#board-view [data-orbit-key="${key}"]`),
      `agent:term:${lead}`, { timeout: ORBIT_ACTION_MS });
    const named = await page.evaluate(({ term, key }) => {
      const view = docHost(boardTab().pane, "board");
      const model = agentGraphModels.get(view);
      const entry = model.agents.find((one) => one.card.pane === `term:${term}`);
      const row = worktreeAgentRows(tabOfTerm(term).worktree).find((one) => one.term === term);
      const glance = [...document.querySelector("#board-view .agent-orbit-glance-line").children]
        .map((part) => part.textContent);
      return {
        prompt: panePrompts.get(term),
        sidebar: agentRowPrimary(row, agentRowState(row)),
        sidebarDrawn: document.querySelector(`.wt-agent[data-term="${term}"] .wt-agent-name`)?.textContent ?? "",
        heading: entry.card.heading,
        identity: agentGraphIdentity(entry.card),
        entity: model.entities.get(entry.key)?.label ?? "",
        label: document.querySelector(`#board-view [data-orbit-key="${key}"]`)?.textContent ?? "",
        glance,
        tab: tabLabel(tabOfTerm(term)),
      };
    }, { term: lead, key: `agent:term:${lead}` });
    const everywhere = [named.prompt, named.sidebar, named.sidebarDrawn, named.heading, named.identity,
      named.entity, named.glance[1], named.tab];
    ok("a go-on word and the window's pointer never name the main pane: every surface says the request",
      everywhere.every((word) => word === asked) && named.label.includes(asked), JSON.stringify(named));
    ok("the one-line story counts the coordinator's work in the words it already had, under the request's name",
      named.glance.join("") === `${asked}가 작업 2개에 지시를 보냈고, 모두 작업 중입니다.`, JSON.stringify(named.glance));
    ok("the main-name page raised no error", faults.length === 0, faults.join(" | "));
  } finally {
    await page.close();
  }
}

/* 전/후 사진(t-11540): 같은 픽스처를 입체 보기·카드 보기·사이드바에서 한 장씩. */
async function shootMainName(browser, origin, dir) {
  mkdirSync(dir, { recursive: true });
  const { page } = await openWindowTestPage(browser, origin);
  const shots = [];
  try {
    await page.setViewportSize({ width: 1440, height: 900 });
    await installBoardWaits(page);
    await installOrbitCounters(page);
    const { lead } = await page.evaluate(mainNameFixture);
    await page.evaluate(() => window.__BOARD_SETTLED__());
    await page.waitForFunction((key) => document.querySelector(`#board-view [data-orbit-key="${key}"]`),
      `agent:term:${lead}`, { timeout: ORBIT_ACTION_MS });
    await page.evaluate(() => window.__ORBIT_FRAMES__(4));
    const shoot = async (name) => {
      const file = join(dir, `${name}.png`);
      await page.screenshot({ path: file, animations: "disabled" });
      shots.push(file);
    };
    await shoot("main-name-3d");
    await page.evaluate(() => document.querySelector('#board-view [data-relations-view="cards"]').click());
    await page.evaluate(() => window.__BOARD_SETTLED__());
    await shoot("main-name-cards");
    const row = page.locator(`.wt-agent[data-term="${lead}"]`);
    const box = await row.boundingBox();
    await page.screenshot({ path: join(dir, "main-name-sidebar.png"), animations: "disabled",
      clip: { x: 0, y: Math.max(0, box.y - 120), width: Math.max(box.x + box.width + 40, 320), height: 280 } });
    shots.push(join(dir, "main-name-sidebar.png"));
  } finally {
    await page.close();
  }
  return shots;
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
  const gpu = orbit.handles?.gpu ?? {};
  return [
    `| engine | ${engine} | ${measured.viewport.width}×${measured.viewport.height} @${measured.viewport.dpr}x | ${measured.load} |`,
    `| fixture | ${JSON.stringify(measured.fixture)} | ${measured.product} | |`,
    "|---|---|---|---|",
    `| bodies · rails (lineage/dependency) · mail links · flowing mail | ${orbit.handles?.bodies} · ${edges.spawned}/${edges.dependency} · ${orbit.handles?.links} · ${orbit.handles?.particles} | | |`,
    `| GPU a frame: draw calls (scene / with bloom) · triangles | ${gpu.sceneCalls} / ${gpu.calls} · ${gpu.triangles} | bloom ${orbit.handles?.bloom} | |`,
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

/* 사진 여섯 장: 판 셋 × 다크·라이트. 카메라는 여는 판의 자리(표의 `camera.yaw`·`camera.pitch`)에
 * 세운다(옛 트리에서는 그림이 도는 대로) — 같은 판을 같은 각도로 다시 찍을 수 있게. */
async function shootBoardOrbit(browser, origin, dir, dpr) {
  mkdirSync(dir, { recursive: true });
  const shots = [];
  const plan = Object.entries(ORBIT_SIZES).flatMap(([size, fixture]) =>
    ["dark", "light"].map((theme) => ({ size, fixture, theme, width: 1440, name: `3d-${size}-${theme}` })));
  plan.push({ size: "60", fixture: ORBIT_SIZES[60], theme: "dark", width: 1024, name: "3d-60-dark-narrow" });
  for (const { fixture, theme, width, name } of plan) {
    {
      const { page } = await openWindowTestPage(browser, origin);
      try {
        await page.setViewportSize({ width, height: 960 });
        if (dpr !== 1) {
          const cdp = await page.context().newCDPSession(page);
          await cdp.send("Emulation.setDeviceMetricsOverride",
            { width, height: 960, deviceScaleFactor: dpr, mobile: false });
        }
        await openOrbit(page, fixture);
        await page.mouse.move(1, 1);
        await page.evaluate(async (theme) => {
          if (theme === "light") document.documentElement.dataset.theme = "light";
          else delete document.documentElement.dataset.theme;
          await window.__ORBIT_WAIT__(1_200);
          const state = window.__ORBIT_STATE__?.();
          if (state?.camera) window.__ORBIT_HOLD_CAMERA__(ORBIT.camera.yaw, ORBIT.camera.pitch);
          await window.__ORBIT_FRAMES__(4);
        }, theme);
        const path = join(dir, `${name}.png`);
        /* 판의 네모를 잘라 찍는다 — 요소 사진은 두 박자 동안 멈춘 요소를 기다리는데, 바쁜 기계의
         * 소프트웨어 GL에서는 그 두 박자가 제한 시간을 넘는다. */
        const clip = await page.locator("#board-view .agent-graph-surface").boundingBox();
        await page.screenshot({ path, clip, caret: "hide", timeout: 60_000 });
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
  const mainShotDir = option("--main-shots", null);
  const dpr = Number(option("--dpr", 1));
  const { files, origin } = await createWindowServer();
  const browserType = engine === "webkit" ? webkitType() : chromium;
  const browser = await browserType.launch(engine === "webkit" ? {} : { args: ORBIT_GL_ARGS });
  const lines = [];
  const report = (name, pass, detail = "") =>
    lines.push(`${pass ? "PASS" : "FAIL"}  ${name}${detail ? `  — ${detail}` : ""}`);
  const tables = [];
  const measures = [];
  try {
    if (mainShotDir) {
      const { browser: drawing, own } = await orbitGlBrowser(browser);
      console.log((await shootMainName(drawing, origin, resolve(mainShotDir))).join("\n"));
      if (own) await drawing.close();
    } else if (shotDir) {
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
  if (!shotDir && !mainShotDir) {
    console.log(lines.join("\n"));
    for (const table of tables) console.log(`\n${table}`);
    console.log(`\n${lines.filter((line) => line.startsWith("PASS")).length}/${lines.length} passed`);
    process.exit(lines.some((line) => line.startsWith("FAIL")) ? 1 : 0);
  }
}
