/* 작업 상황판 「지금」 (t-67607 → t-70028) — 사람이 통과시킨 시안 B를 창이 그리는가.
 *
 * 시안(아티팩트 p-90883818b247d08e v2, reports/t-67561/b.html)과 같은 예시 상황을
 * 창의 데스크 답으로 세우고(`draftBoardOverlay`), 시안이 스스로 잰 넷을 같은 셈으로
 * 잰다: 1440×900 첫 화면의 숫자(개수·시각·분) ≤ 8, 묶음(카드) 3, 글자 크기 단계 4,
 * 원장의 영어 진단 노출 0. 그리고 네 탭(일정은 t-67482), 당신 차례의 단추가 원장의
 * 문(`desk_reply`·`desk_hold_resolve`·`desk_review_pass`)으로만 가는지, 원장이 없는 창에서 예전
 * 작업 목록이 그대로 서는지, 30과업·7워커의 그림이 저사양(CPU 4배 감속)에서 3 ms·
 * 요소 400 안인지를 잰다.
 *
 * 사람 10-22(t-70028, 2000px 창의 사진): 「화면 크기에 맞추고 프로그레스도 멈춰 있음」·「시안대로
 * 디자인이 안 나옴」. 그래서 더 잰다: 1920·2560 폭에서 판이 창의 폭을 다 쓰고 시안의 격자(2:1,
 * 표는 폭 전체)로 나누는가, 상태가 시안의 채운 점인가(멈춘 그림으로 읽히던 고리가 아니라), 행의
 * 칸이 시안의 말(모델의 표시 이름·가지)인가, 빌드 줄 카드가 시안의 카드인가(남은 단계 「대기」,
 * 끝난 레인은 한 줄로 접힘, 줄이 비면 「줄 비어 있음」), 1분 박자가 원장의 움직임 없이도 시간
 * 글자를 고치되 바뀐 글자만 쓰고 숨은 창에서는 아무것도 쓰지 않는가, 그리고 통과를 원장이 받은
 * 뒤 행이 빠질 때까지 단추가 닫혀 있는가(검토 t-68962 P3 ⑥).
 *
 *   node ui/tests/board-now.mjs
 *   WINDOW_SUITES=board-now node ui/tests/window.mjs */
import { mkdir } from "node:fs/promises";
import { pathToFileURL } from "node:url";
import { resolve } from "node:path";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";
import { installBoardWaits } from "./board-waits.mjs";
import { coordinatorDeskFixture } from "./coordinator-desk.mjs";
import { installPerfHands, median } from "./coordinator-desk-perf.mjs";

const SHOTS = "output/playwright/board-now";

/* 시안 B의 예시 상황 — 세 목표, 질문 하나·검토 하나, 도는 일 둘·기다림 하나·멈춤
 * 하나, 끝난 일 셋, 발행 레인 하나(결합 시험·앱 빌드 끝, zo 시험 8분째, 발행 대기), 그 뒤 빌드
 * 줄에서 차례를 기다리는 재선택·대기 고침. 워커 탭과 기록 탭의 시안 값도 싣는다(t-70264): 워커마다
 * 계속 판정이 읽은 비용·호출과 갈라져 나온 뒤의 줄, 그리고 오늘 끝난 일 여섯(목표 밖의 셋은 지금
 * 표에 서지 않는다). 데스크 픽스처가 세운 답 위에 덮는다(그 픽스처가 응답 손·보드 열기를 맡는다).
 * `extra`는 과업 수를 늘려 성능을 잰다. `lane`이 "green"이면 레인은 07:31 무렵 초록으로 끝났고
 * 빌드 줄은 비어 있다. A page function: no closure. */
export function draftBoardOverlay({ now = Date.now(), holds = false, extra = 0, ledger = true, lane = "running" } = {}) {
  const minute = 60_000;
  const run = "run-desk";
  const goal = (parent, parentTitle) => ({ parent, parent_title: parentTitle });
  const ship = goal("t-g1", "새 버전 내보내기");
  const board = goal("t-g2", "작업 상황판을 한눈에");
  const hands = goal("t-g3", "컴퓨터 사용을 사람처럼");
  const head = (n) => n.toString(16).padStart(7, "0").repeat(6).slice(0, 40);
  const task = (id, title, stage, extra = {}) => ({
    run, id, title, stage, parent: null, parent_title: null, gate: null, blocked_by: [], closed: null,
    created_ms: now - 90 * minute, cost: null, writing: null, progress: null, ...extra,
  });
  // Finished today at the draft's clock (10:24, 09:42, 08:15, 07:58 seen from 10:30), in the draft's order and
  // inside today whatever the clock says.
  const midnight = new Date(now);
  midnight.setHours(0, 0, 0, 0);
  const finishedAt = (ago) => Math.max(midnight.getTime() + (600 - ago) * 1000, now - ago * minute);
  const landed = (ago, took) => ({ times: { started_ms: finishedAt(ago) - took * minute, landed_ms: finishedAt(ago) } });
  const tasks = [
    task("t-66912", "재선택·대기 고침", "ready", ship),
    task("t-67561", "상황판 디자인 시안 2차", "dispatched", { ...board, progress: { stage: "working", attempt: "dp-67562" } }),
    task("t-66788", "반복 관측 줄이기", "dispatched", { ...hands, progress: { stage: "working", attempt: "dp-66789" } }),
    task("t-66410", "증거 계약 설계", "failed", { ...hands, progress: { stage: "failed", attempt: "dp-66411" } }),
    task("t-66790", "헬퍼 회차 A·B·C", "reported", {
      ...hands, created_ms: now - 12 * minute,
      progress: { stage: "report_only", attempt: "dp-66791", source: head(7) },
      times: { started_ms: now - 64 * minute, handed_ms: now - 12 * minute },
      hand_in: { conditions: [{ number: 1, text: "빨강 시험이 초록이 됨", state: "checked" }],
        decisions: "헬퍼가 같은 창을 세 번 보던 것을 한 번으로", blocked: "", next: "" },
    }),
    task("t-62493", "컴퓨터 사용 실제 입력", "merged", { ...hands, ...landed(135, 240) }),
    task("t-62872", "패널 정지 하나", "merged", { ...hands, ...landed(152, 50) }),
    task("t-67064", "작업 브라우저 혼자 쓰기", "deployed", { ...hands, ...landed(48, 130) }),
    task("t-63394", "zo가 사람의 결정을 묻기", "deployed", landed(48.2, 180)),
    task("t-66982", "공급자 오류를 말로", "deployed", landed(48.4, 85)),
    task("t-67536", "상황판 시안 1차", "nothing_to_land",
      { times: { started_ms: finishedAt(6) - 35 * minute, verified_ms: finishedAt(6) } }),
  ];
  if (holds) {
    tasks.push(task("t-66500", "남긴 폴더 보기", "gate", {
      ...hands,
      gate: { id: "gate-701", kind: "unlanded", worker: "w-66501", checkout: "/repos/zerocode/workspaces/t-66500",
        options: [], examined: { kind: "unlanded", branch: "wt/t-66500", base: "main", commits: 2 },
        question: "w-66501 exited leaving 2 commit(s) on wt/t-66500 that main does not have — land them, then resolve this with \"harvest\"; or \"dispatch again\" to hand the task out afresh" },
    }));
    tasks.push(task("t-66600", "기준 커밋 고르기", "gate", {
      ...board,
      gate: { id: "gate-702", kind: "asked", worker: null, checkout: null, examined: null,
        options: ["main 기준", "릴리스 기준"], question: "기준 커밋을 main으로 할까요, 지난 릴리스로 할까요?" },
    }));
  }
  for (let n = 0; n < extra; n += 1) {
    tasks.push(task(`t-${70000 + n}`, `더 도는 과업 ${n + 1}`, n % 3 === 0 ? "ready" : "dispatched",
      { ...(n % 2 ? board : hands), progress: { stage: "working", attempt: `dp-${80000 + n}` } }));
  }
  /* The workers table's numbers (t-70264): what the gate read — spend and calls — and the lines since the fork,
   * the draft's own; the question's worker waits on its answer, the reviewed one and the one whose work waits
   * for the build line rest. */
  const workers = [
    ["w-67562", "t-67561", "상황판 디자인 시안 2차", "claude", "claude-opus-5-5", 241, 21, null, "high", 0.84, 46, [1204, 0]],
    ["w-67491", "t-67490", "일정 타임라인", "claude", "claude-opus-5-5", 239, 48, null, "high", 2.1, 118, [312, 41]],
    ["w-66791", "t-66790", "헬퍼 회차 A·B·C", "claude", "claude-sonnet-5-5", 233, 64, null, "medium", 1.35, 203, [188, 96]],
    ["w-66789", "t-66788", "반복 관측 줄이기", "claude", "claude-sonnet-5-5", 236, 26, null, "medium", 0.62, 88, [74, 12]],
    ["w-66411", "t-66410", "증거 계약 설계", "codex", "gpt-6-astra", null, 120, now - 30 * minute, "high", 1.9, 154, [0, 0]],
    ["w-66913", "t-66912", "재선택·대기 고침", "claude", "claude-opus-5-5", 228, 52, null, "medium", 1.48, 131, [96, 58]],
    ["w-66501", "t-66500", "남긴 폴더 보기", "codex", "gpt-6-astra", null, 70, null, "high", 0.4, 30, [12, 3]],
  ];
  for (let n = 0; n < extra && workers.length < 7; n += 1) workers.push([`w-9${n}`, `t-${70000 + n}`, `더 도는 과업 ${n + 1}`, "claude", "claude-opus-5-5", 260 + n, 5, null, "high", 0.1, 5, [1, 0]]);
  const resting = new Set(["w-66791", "w-66913"]);
  const gateOf = (spent, steps) => ({
    mode: "notify", verdict: "continue", reasons: [], cost: "read", task_spent_usd: spent, task_limit_usd: null,
    metrics: { steps, since_checkpoint: steps, checkpoints: 0, rework_permille: 40, rise_permille: null, step_usd: 0.02,
      spent_usd: spent, unpriced_calls: 0 },
    day_spent_usd: 9.31, day_limit_usd: null, snapshot: null, at_ms: now,
  });
  window.__LEDGER__ = workers.map(([worker, taskId, title, agent, model, term, age, missing, effort, spent, steps]) => ({
    run, worker, agent, state: "working", ledger: "active", hearing: term ? "heard" : "gone", hearing_at: now - minute,
    checkout: `/repos/zerocode/workspaces/${taskId}`, task: title, task_id: taskId, reported: worker === "w-66791",
    dispatch_id: `dp-${worker.slice(2)}`, dispatch_started_ms: now - age * minute, retry_of: null,
    review: { verified: false, merged: false, deployed: false, written: false },
    progress: { stage: worker === "w-66791" ? "report_only" : "working", attempt: `dp-${worker.slice(2)}` },
    term, at: now - 2 * minute, model, effort, pane: `%${term ?? 0}`, asking: worker === "w-67491", wall: null,
    quiet_at: null, pane_missing_since_ms: missing, gate: gateOf(spent, steps),
  }));
  window.__COLUMNS__ = [{ bucket: "attention", cards: [] }, { bucket: "working", cards: workers
    .filter(([worker, , , , , term]) => term && !resting.has(worker))
    .map(([, taskId, title, agent, , term]) => ({
      pane: `term:${term}`, heading: title, state: "working", agent, project: "/repos/zerocode", worktree: "main",
      task: title, task_id: taskId, ask: "", said: "", you: "", parent: "", ledger: "active",
      at: now - minute, changed_at: now - 9 * minute, unseen: false,
    })) }, { bucket: "done", cards: [] }, { bucket: "idle", cards: [] }];
  const mail = [{
    run, id: "m-67495", kind: "question", from: "worker:w-67491", worker: "w-67491", task_id: "t-67490",
    task: "일정 타임라인", body: "끝날 시각을 단계별 중앙값으로 셀까요, 전체 걸린 시간으로 셀까요?",
    reason: null, resets_at_ms: null, created_ms: now - 3 * minute, delivery: "delivered", delivery_id: "d-67496",
    batch: 1, notices: 1,
  }];
  const stages = ["pending", "ready", "dispatched", "reported", "verified", "merged", "deployed", "gate", "blocked",
    "stale", "failed", "unreviewable", "waived", "nothing_to_land", "unknown", "closed"];
  window.__DESK__ = {
    revision: 1,
    runs: ledger ? [{ run, name: "시안 B", seat: true }] : [],
    mail: ledger ? mail : [], news: [], counts: { mail: ledger ? mail.length : 0, news: 0, folded: 0 },
    folded_batches: [], land_batches: [],
    tasks: ledger ? tasks : [],
    stages: stages.map((stage) => ({ stage, count: ledger ? tasks.filter((one) => one.stage === stage).length : 0 })),
  };
  // The lane as its status file says it — the real lane's step names (tools/release/lane.sh).
  const sha = "3e13aaef1aa0b3e2c4d5e6f708192a3b4c5d6e7f";
  const greenAt = now - 100 * minute;
  window.__RELEASE__ = lane === "green" ? {
    sha, phase: "sweep", version: "1.1.84",
    started_at: new Date(greenAt - 101 * minute).toISOString(), updated_at: new Date(greenAt).toISOString(),
    phases: [{ name: "gate-root", rc: 0, secs: 3168 }, { name: "gate-zo", rc: 0, secs: 1719 },
      { name: "build-app", rc: 0, secs: 1090 }, { name: "publish", rc: 0, secs: 42 }],
    disk_free_gb: 20, outcome: "green", reason: "",
  } : {
    sha, phase: "gate-zo", version: "1.1.84",
    started_at: new Date(now - 41 * minute).toISOString(), updated_at: new Date(now - minute).toISOString(),
    phases: [{ name: "gate-root", rc: 0, secs: 720 }, { name: "build-app", rc: 0, secs: 1260 }],
    disk_free_gb: 21, outcome: null, reason: "",
  };
  // The schedule's reading (t-67482): the running lane's steps ahead with their usual time, and the
  // build line behind it — 재선택·대기 고침's job waits its turn. A green lane leaves the line empty.
  window.__ANSWER__.schedule_timeline = () => ({
    revision: lane === "green" ? 2 : 1, unchanged: false, read_ms: now,
    line: { found: true, paused: false, runner_lanes: [1, 2], disk_low: false, quiet_wanted: false },
    jobs: lane === "green" ? [] : [{
      name: `${Math.floor(now / 1000)}-run4275-t66912-build`, who: "run4275-t66912", state: "waiting", class: "plain",
      lane: null, cap_secs: 1800, queued_ms: now - 5 * minute, start_ms: null, end_ms: null, secs: null, rc: null,
      kind: null, gate: null, part: null, what: null, expected: { secs: 900, samples: 3 },
    }],
    lanes: lane === "green" ? [] : [{
      sha: sha.slice(0, 8), version: "1.1.84", start_ms: now - 41 * minute, end_ms: null, verdict: null, reason: null,
      running: true,
      stages: [
        { name: "gate-root", start_ms: now - 41 * minute, end_ms: now - 29 * minute, rc: 0, secs: 720 },
        { name: "build-app", start_ms: now - 29 * minute, end_ms: now - 8 * minute, rc: 0, secs: 1260 },
        { name: "gate-zo", start_ms: now - 8 * minute, end_ms: null, rc: null, secs: null },
      ],
      ahead: [{ name: "gate-zo", expected: { secs: 14 * 60, samples: 5 } }, { name: "publish", expected: { secs: 120, samples: 5 } }],
    }],
  });
  // The window's model catalog (`agent_models`, zo's live list): the names the draft's worker column reads.
  const catalog = {
    claude: [{ id: "claude-opus-5-5", display_name: "Opus 5.5", provider: "claude" },
      { id: "claude-sonnet-5-5", display_name: "Sonnet 5.5", provider: "claude" }],
    codex: [{ id: "gpt-6-astra", display_name: "GPT-6-Astra", provider: "openai" }],
  };
  window.__ANSWER__.agent_models = ({ agent }) => catalog[agent] ?? [];
  // The workers' checkouts as git has them — the draft's branch column and the workers table's lines.
  const linesOf = new Map(workers.map(([, taskId, , , , , , , , , , moved]) => [taskId, moved]));
  window.__ANSWER__.desk_checkouts = ({ paths }) => paths.map((path) => {
    const moved = linesOf.get(path.split("/").pop());
    return { path, branch: `wt/${path.split("/").pop()}`, base: "main", beyond_base: 1, dirty_files: 0,
      lines: moved ? { added: moved[0], removed: moved[1] } : null };
  });
  window.__ANSWER__.desk_hold_resolve = (args) => (window.__DESK_SENT__.push({ verb: "hold", ...args }), { gateId: args.gate });
  window.__ANSWER__.desk_review_pass = (args) => (window.__DESK_SENT__.push({ verb: "pass", ...args }), { taskId: args.task });
  for (const listener of window.__LISTENERS__["ledger:changed"] ?? []) listener({ payload: now });
}

/* 시안의 셈(reports/t-67561/src/common.js `firstScreen`)을 창의 판에: 보이는 첫 화면
 * (창 높이 안)의 숫자 낱말, 카드, 글자 크기 단계, 원장의 영어 진단, 요소 수. 숫자는
 * 시각·분·개수만 센다 — 과업 id·sha·폴더 이름·모델 이름은 숫자가 아니라 이름이다(`code`,
 * 옮기지 않는 이름 `translate="no"`, 글자 뒤에 붙은 수). 시안의 셈도 시각과 개수(`data-fig`)만
 * 셌고, 그 첫 화면의 「Opus 5.5」·「Sonnet 5.5」는 세지 않았다. */
function boardFirstScreen() {
  const view = document.querySelector("#board-view");
  const surface = view.querySelector(".task-board-surface");
  const fold = window.innerHeight;
  const visible = (el) => el.checkVisibility({ checkOpacity: true, checkVisibilityCSS: true })
    && el.getBoundingClientRect().top < fold && el.getBoundingClientRect().bottom > 0;
  const all = [...surface.querySelectorAll("*")].filter(visible);
  const walker = document.createTreeWalker(surface, NodeFilter.SHOW_TEXT);
  let figures = 0;
  let raw = 0;
  const words = [];
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    const host = node.parentElement;
    if (!host || !node.textContent.trim() || !visible(host) || host.closest('code, pre, details:not([open]), [translate="no"]')) continue;
    words.push(node.textContent);
    figures += (node.textContent.match(/(?<![\p{L}\-.\d])\d+(?:[:.,]\d+)*/gu) ?? []).length;
    if (/exited in|nobody has looked|before reporting|dispatch again|resolve this/i.test(node.textContent)) raw += 1;
  }
  const sizes = new Set(all.filter((el) => [...el.childNodes].some((one) => one.nodeType === 3 && one.textContent.trim()))
    .map((el) => parseFloat(getComputedStyle(el).fontSize)));
  const cards = all.filter((el) => el.matches('.board-desk-block:is([data-desk-block="asks"], [data-desk-block="release"], [data-desk-block="running"])'));
  return { figures, raw, sizes: [...sizes].sort((a, b) => b - a), cards: cards.length, elements: all.length, words: words.join(" ¦ ") };
}

/* Every word on the Now tab against what it is drawn on — the draft's own contrast reckoning
 * (reports/t-67561/src/common.js): the text's colour over the grounds stacked under it, up to the
 * first opaque one. A disabled control is left out, as WCAG leaves it. A page function; another
 * surface's scene hands it the selector of its own scope. */
export function nowContrast(scopeSelector = "#board-view :is(.board-now-tabs, .task-board-intro, .task-board-desk)") {
  const parse = (value) => {
    let found = /^rgba?\(([^)]+)\)$/.exec(value);
    if (found) {
      const parts = found[1].split(/[\s,/]+/).filter(Boolean).map(Number);
      return [parts[0], parts[1], parts[2], parts[3] ?? 1];
    }
    found = /^color\(srgb ([^)]+)\)$/.exec(value);
    if (found) {
      const parts = found[1].split(/[\s/]+/).filter(Boolean).map(Number);
      return [parts[0] * 255, parts[1] * 255, parts[2] * 255, parts[3] ?? 1];
    }
    return [0, 0, 0, 0];
  };
  const over = (top, bottom) => {
    const alpha = top[3] + bottom[3] * (1 - top[3]);
    if (alpha === 0) return [0, 0, 0, 0];
    return [0, 1, 2].map((at) => (top[at] * top[3] + bottom[at] * bottom[3] * (1 - top[3])) / alpha).concat([alpha]);
  };
  const lum = (colour) => {
    const [r, g, b] = colour.slice(0, 3).map((part) => {
      const value = part / 255;
      return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
    });
    return 0.2126 * r + 0.7152 * g + 0.0722 * b;
  };
  const groundOf = (node) => {
    const layers = [];
    for (let at = node; at && at.nodeType === 1; at = at.parentElement) {
      const colour = parse(getComputedStyle(at).backgroundColor);
      if (colour[3] > 0) layers.push(colour);
      if (colour[3] >= 1) break;
    }
    let ground = [255, 255, 255, 1];
    for (let at = layers.length - 1; at >= 0; at -= 1) ground = over(layers[at], ground);
    return ground;
  };
  const scope = [...document.querySelectorAll(scopeSelector)];
  let min = 99;
  let where = "";
  const below = [];
  for (const root of scope) {
    for (const node of [root, ...root.querySelectorAll("*")]) {
      const own = [...node.childNodes].some((child) => child.nodeType === 3 && child.textContent.trim());
      if (!own || !node.checkVisibility({ checkOpacity: true, checkVisibilityCSS: true }) || node.closest(":disabled")) continue;
      const ground = groundOf(node);
      const ink = over(parse(getComputedStyle(node).color), ground);
      const [high, low] = [lum(ink), lum(ground)].sort((a, b) => b - a);
      const ratio = (high + 0.05) / (low + 0.05);
      // A glyph that is itself a picture (an agent's mark, `role="img"`) is a graphic:
      // WCAG 1.4.11 asks 3:1 of it, and it is still reported when below.
      if (node.matches('[role="img"]')) {
        if (ratio < 3) below.push(`${node.className || node.tagName}: ${node.textContent.trim().slice(0, 24)} ${ratio.toFixed(2)}`);
        continue;
      }
      if (ratio < min) { min = ratio; where = node.textContent.trim().slice(0, 24); }
      if (ratio < 4.5) below.push(`${node.className || node.tagName}: ${node.textContent.trim().slice(0, 24)} ${ratio.toFixed(2)}`);
    }
  }
  return { min: Number(min.toFixed(2)), where, below };
}

/* The sizes the chosen tab's words stand in, over the same scope as `nowContrast` — the draft's yardstick is
 * four at most (t-70264). A page function. */
function boardTypeSizes() {
  const view = document.querySelector("#board-view");
  const sizes = new Set();
  for (const root of view.querySelectorAll(".board-now-tabs, .task-board-intro, .task-board-desk")) {
    for (const node of [root, ...root.querySelectorAll("*")]) {
      const own = [...node.childNodes].some((child) => child.nodeType === 3 && child.textContent.trim());
      if (own && node.checkVisibility({ checkOpacity: true, checkVisibilityCSS: true })) {
        sizes.add(parseFloat(getComputedStyle(node).fontSize));
      }
    }
  }
  return [...sizes].sort((a, b) => b - a);
}

/* The board alone, the window's size, so the first screen is the board's. */
async function fullBoard(page) {
  await page.evaluate(async () => {
    const view = document.querySelector("#board-view");
    Object.assign(view.style, { position: "fixed", inset: "0", width: "100vw", height: "100vh", zIndex: "100" });
    await window.__BOARD_SETTLED__();
    for (let beat = 0; beat < 30 && (deskLedgerAsking || deskPaintFrame !== null || boardScheduleAsking); beat += 1) {
      await new Promise((done) => requestAnimationFrame(done));
    }
    await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
  });
}

/* The draft's scene, with the facts the desk reads on its slow beat asked at once: the lane's
 * status, the schedule's reading, the workers' checkouts and their models. */
export async function draftScene(page, options = {}) {
  await page.evaluate(draftBoardOverlay, options);
  await page.evaluate(async () => {
    await askReleaseStatus();
    refreshBoardSchedule({ fresh: true });
    askDeskCheckouts();
    await paintBoardView();
    await window.__UNTIL__(() => !boardScheduleAsking && !deskCheckoutsAsking, "the schedule's and the checkouts' answers");
    // The worker column names models by the window's catalog: read here once per agent, so the scene
    // stands whether or not this board asks for it itself.
    await Promise.all(["claude", "codex"].map((agent) => agentModelsFor(agent)));
    paintCoordinatorDesk(document.querySelector("#board-view"));
  });
  await fullBoard(page);
}

/* The 지금 tab's boxes and words that the draft fixes — one read for the grid, the dots and the rows. */
function readNow() {
  const view = document.querySelector("#board-view");
  const surface = view.querySelector(".task-board-surface");
  const box = (node) => {
    const rect = node?.getBoundingClientRect();
    return rect ? { left: Math.round(rect.left), right: Math.round(rect.right), width: Math.round(rect.width) } : null;
  };
  const style = getComputedStyle(surface);
  const inner = surface.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight);
  // A dot is its cell's ::before (the rows, the legend, the build line's status).
  const dotOf = (node) => (node ? getComputedStyle(node, "::before").backgroundColor : null);
  const token = (name) => {
    const probe = document.createElement("i");
    probe.style.color = `var(${name})`;
    document.body.append(probe);
    const said = getComputedStyle(probe).color;
    probe.remove();
    return said;
  };
  const row = (id) => view.querySelector(`.board-run[data-task="run-desk/${id}"]`);
  const cells = (node) => {
    if (!node) return null;
    const work = node.querySelector(".board-run-work");
    return {
      word: node.querySelector(".board-run-state")?.textContent,
      work: work.querySelector(".board-run-work-text")?.textContent ?? work.textContent,
      raw: work.querySelector(".board-run-raw")?.textContent ?? "",
      who: node.querySelector(".board-run-who")?.textContent,
      branch: node.querySelector(".board-run-branch")?.textContent,
      end: node.querySelector(".board-run-end")?.textContent,
      dot: dotOf(node.querySelector(".board-run-state")),
    };
  };
  const card = view.querySelector('[data-desk-block="release"]');
  return {
    inner: Math.round(inner),
    surface: box(surface),
    tabs: box(view.querySelector(".board-now-tabs")),
    desk: box(view.querySelector(".task-board-desk")),
    asks: box(view.querySelector('[data-desk-block="asks"]')),
    release: box(card),
    running: box(view.querySelector('[data-desk-block="running"]')),
    sideways: surface.scrollWidth > surface.clientWidth + 1,
    legend: [...view.querySelectorAll(".board-legend > li")].map((item) => [item.textContent, dotOf(item)]),
    tokens: { run: token("--status-dot-run"), queue: token("--status-dot-queue"), halt: token("--status-dot-halt"),
      done: token("--status-dot-done") },
    spinning: [...view.querySelectorAll('.task-board-desk [data-desk-block="running"] *, .task-board-desk [data-desk-block="release"] *')]
      .filter((node) => getComputedStyle(node).animationName !== "none" || getComputedStyle(node, "::before").animationName !== "none")
      .map((node) => node.className).slice(0, 5),
    rows: { queue: cells(row("t-66912")), run: cells(row("t-67561")), runHands: cells(row("t-66788")), halt: cells(row("t-66410")) },
    order: [...view.querySelectorAll(".board-run")].map((node) => node.dataset.task.split("/")[1]),
    reported: Boolean(row("t-66790")),
    doneFold: [...view.querySelectorAll(".board-goal.is-done .board-goal-name")].map((node) => node.textContent),
    runsFoot: view.querySelector(".board-runs-foot")?.textContent,
    runningHint: view.querySelector('[data-desk-block="running"] .board-desk-head-note')?.textContent,
    line: card ? {
      status: card.querySelector(".board-desk-release-status")?.textContent,
      dot: dotOf(card.querySelector(".board-desk-release-status")),
      meta: card.querySelector(".board-desk-release-meta")?.textContent,
      listShown: card.querySelector(".board-desk-release-phases")?.checkVisibility() ?? false,
      steps: [...card.querySelectorAll(".board-desk-release-phase")].map((item) =>
        [item.className.replace("board-desk-release-phase ", ""), item.children[1].textContent, item.children[2].textContent]),
      times: [...card.querySelectorAll(".board-desk-release-times tr")].map((item) => item.textContent),
      log: card.querySelector(".board-desk-release-log > summary")?.textContent,
      foot: card.querySelector(".board-line-foot")?.checkVisibility() ? card.querySelector(".board-line-foot").textContent : "",
    } : null,
    askMeta: [...view.querySelectorAll('[data-desk-block="asks"] .board-asks > li')].map((item) =>
      (item.querySelector(".board-desk-letter-who") ?? item.querySelector(".board-ask-meta"))?.textContent),
    badge: (() => {
      const kind = view.querySelector('[data-desk-block="asks"] .board-ask-kind');
      return kind ? { ground: getComputedStyle(kind).backgroundColor, ink: getComputedStyle(kind).color } : null;
    })(),
  };
}

export async function testBoardNow(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await installBoardWaits(page);
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.evaluate(coordinatorDeskFixture, { tasks: 0, workers: 1, mail: 0 });
    await draftScene(page);

    /* ---- 네 탭과 맨 위 한 문장 ---- */
    const head = await page.evaluate(() => {
      const surface = document.querySelector("#board-view .task-board-surface");
      return {
        tab: surface.dataset.boardTab, ledger: surface.dataset.boardLedger,
        tabs: [...surface.querySelectorAll(".board-now-tab")].map((one) => [one.dataset.boardTabButton, one.getAttribute("aria-selected"), one.tabIndex]),
        title: surface.querySelector(".board-now-title").textContent,
        sub: surface.querySelector(".task-board-summary").textContent,
      };
    });
    ok("the board opens on the Now tab of four, the chosen one alone in the Tab order",
      head.tab === "now" && head.ledger === "on" && JSON.stringify(head.tabs)
        === JSON.stringify([["now", "true", 0], ["schedule", "false", -1], ["workers", "false", -1], ["history", "false", -1]]),
      JSON.stringify(head));
    ok("the sentence on top counts what waits on the person — one question and one review",
      head.title === "답할 일 2개 — 나머지는 저절로 돕니다", JSON.stringify(head));
    ok("the second line names what ends next and when — the release, at the time its steps' medians add up to",
      /^다음으로 끝나는 일은 새 버전 발행, .+쯤입니다\.$/.test(head.sub), head.sub);

    /* ---- 시안의 넷: 숫자 ≤ 8 · 묶음 3 · 글자 단계 4 · 영어 진단 0 ---- */
    const first = await page.evaluate(boardFirstScreen);
    ok("the first screen at 1440×900 carries at most eight figures", first.figures <= 8, JSON.stringify(first));
    ok("the first screen is three groups: your turn, the build line, running now", first.cards === 3, JSON.stringify(first));
    ok("the first screen sets its words in at most four sizes", first.sizes.length <= 4, JSON.stringify(first.sizes));
    ok("no English sentence of the ledger stands on the first screen", first.raw === 0, first.words);

    /* ---- 시안의 줄과 점 (t-70028) ---- */
    const now = await page.evaluate(readNow);
    ok("a state is the draft's filled dot beside its word — running amber, waiting grey, stopped red, finished cyan",
      JSON.stringify(now.legend.map(([word]) => word)) === JSON.stringify(["도는 중", "차례 기다림", "멈춤", "끝남"])
        && now.legend.map(([, dot]) => dot).join() === [now.tokens.run, now.tokens.queue, now.tokens.halt, now.tokens.done].join()
        && now.rows.run?.dot === now.tokens.run && now.rows.queue?.dot === now.tokens.queue
        && now.rows.halt?.dot === now.tokens.halt,
      JSON.stringify({ legend: now.legend, tokens: now.tokens, rows: now.rows }));
    ok("nothing on the Now tab's cards turns: no ring that reads as stopped when the minute has not moved",
      now.spinning.length === 0, JSON.stringify(now.spinning));
    ok("a row speaks the draft's columns: the model's catalog name, the branch, the time running",
      now.rows.run?.who === "Opus 5.5" && now.rows.runHands?.who === "Sonnet 5.5" && now.rows.halt?.who === "GPT-6-Astra"
        && now.rows.run?.branch === "wt/t-67561" && /^\d+분째$/.test(now.rows.run?.end ?? ""),
      JSON.stringify(now.rows));
    ok("a stopped row says 멈춤 at its end and keeps 원문 보기 beside its words; a waiting one says 대기",
      now.rows.halt?.end === "멈춤" && now.rows.halt?.raw === "원문 보기" && now.rows.halt?.work.includes("워커 화면이 사라졌습니다")
        && now.rows.queue?.end === "대기" && now.rows.queue?.word === "차례 기다림",
      JSON.stringify(now.rows));
    ok("the badge of your turn is the draft's solid blue under white words",
      now.badge?.ink === "rgb(255, 255, 255)" && now.badge?.ground === "rgb(0, 98, 209)", JSON.stringify(now.badge));
    ok("each row of your turn carries the draft's line: the goal, the task and its id, the model, how long ago",
      now.askMeta[0] === "일정 타임라인 t-67490 · Opus 5.5 · 3분 전"
        && now.askMeta[1] === "컴퓨터 사용을 사람처럼 · 헬퍼 회차 A·B·C t-66790 · Sonnet 5.5 · 12분 전 — 통과시키면 다음 발행에 같이 실립니다",
      JSON.stringify(now.askMeta));

    /* ---- 빌드 줄: 시안의 카드 — 도는 단계, 남은 단계, 다음 차례 ---- */
    ok("the build line card says running at its right, and which of how many steps the lane is on",
      now.line?.status === "도는 중" && now.line.dot === now.tokens.run && now.line.meta.endsWith("· 네 단계 중 셋째"),
      JSON.stringify(now.line));
    ok("its steps are the lane's in Korean: two done, the running one with its minutes, the one ahead waiting",
      JSON.stringify(now.line?.steps?.map(([state, name]) => [state, name])) === JSON.stringify([
        ["is-done", "결합 시험"], ["is-done", "앱 빌드"], ["is-running", "zo 시험"], ["is-ahead", "발행"]])
        && now.line.steps[0][2] === "끝" && /^\d+분째$/.test(now.line.steps[2][2]) && now.line.steps[3][2] === "대기",
      JSON.stringify(now.line));
    // The record is built for whoever opens it: folded, it carries no table.
    await page.click('#board-view [data-desk-block="release"] .board-desk-release-log > summary');
    await fullBoard(page);
    const opened = await page.evaluate(readNow);
    await page.click('#board-view [data-desk-block="release"] .board-desk-release-log > summary');
    await fullBoard(page);
    ok("behind 단계별 시간 · 원문 로그 each step has its time, the running one beside its usual — built only when opened",
      now.line?.log === "단계별 시간 · 원문 로그" && now.line.times.length === 0 && opened.line.times.length === 4
        && opened.line.times[0] === "결합 시험12분" && /^zo 시험\d+분째 · 평소 14분$/.test(opened.line.times[2])
        && opened.line.times[3] === "발행약 2분",
      JSON.stringify({ folded: now.line?.times, opened: opened.line?.times }));
    ok("its foot names the build line's next turn by the task's name", now.line?.foot === "다음 차례 — 재선택·대기 고침",
      JSON.stringify(now.line));

    /* ---- 당신 차례: 질문·검토, 행 안의 첫 행동 ---- */
    const asks = await page.evaluate(() => [...document.querySelectorAll('#board-view [data-desk-block="asks"] .board-asks > li')]
      .map((row) => ({ key: row.dataset.ask, buttons: [...row.querySelectorAll("button:not([hidden])")].map((one) => one.textContent) })));
    ok("your turn holds the question first, with its answer button, then the report with read and pass",
      asks.length === 2 && asks[0].key.startsWith("review:") === false && asks[0].buttons.includes("답하기")
        && asks[1].buttons.includes("보고 읽기") && asks[1].buttons.includes("통과"), JSON.stringify(asks));
    await page.click('#board-view [data-ask^="review:"] [data-act="pass"]');
    const passed = await page.evaluate(async () => {
      await new Promise((done) => setTimeout(done, 0));
      await new Promise((done) => requestAnimationFrame(done));
      return { sent: window.__DESK_SENT__.filter((one) => one.verb === "pass"),
        note: document.querySelector('#board-view [data-ask^="review:"] .board-ask-note').textContent };
    });
    ok("pass goes to the ledger as the seat, naming the attempt and the source it drew, and says so on the row",
      passed.sent.length === 1 && passed.sent[0].attempt === "dp-66791" && passed.sent[0].source?.length === 40
        && passed.sent[0].retryRequest?.startsWith("ui-board-desk_review_pass-") && passed.note.includes("t-66790"),
      JSON.stringify(passed));
    // The ledger took it; until its next beat takes the row away, a second press is no second request.
    const shut = await page.evaluate(async () => {
      const pass = document.querySelector('#board-view [data-ask^="review:"] [data-act="pass"]');
      const disabled = pass.disabled;
      // A press that slips through (the button forced open) still sends nothing a second time.
      pass.disabled = false;
      pass.click();
      await new Promise((done) => setTimeout(done, 0));
      paintCoordinatorDesk(document.querySelector("#board-view"));
      return { disabled, sent: window.__DESK_SENT__.filter((one) => one.verb === "pass").length,
        repainted: document.querySelector('#board-view [data-ask^="review:"] [data-act="pass"]').disabled };
    });
    ok("once the ledger took the pass, the row's button stays shut and a second press sends nothing (P3 ⑥)",
      shut.disabled && shut.sent === 1 && shut.repainted, JSON.stringify(shut));

    /* ---- 지금 도는 일: 목표별, 시안의 차례 — 도는 일, 기다림, 멈춤 ---- */
    const runs = await page.evaluate(() => {
      const table = document.querySelector('#board-view [data-desk-block="running"] .board-runs');
      return {
        goals: [...table.querySelectorAll(".board-goal:not(.is-done) .board-goal-name")].map((one) => one.textContent),
        rows: [...table.querySelectorAll(".board-run")].map((row) => [row.dataset.task, row.querySelector(".board-run-state").textContent]),
        legend: [...document.querySelectorAll('#board-view [data-desk-block="running"] .board-legend li')].map((one) => one.textContent),
      };
    });
    ok("running now folds its rows under their goals, running first and a stopped one last within its goal",
      runs.goals.length === 3 && runs.rows.findIndex(([id]) => id.endsWith("t-66788")) < runs.rows.findIndex(([id]) => id.endsWith("t-66410")),
      JSON.stringify(runs));
    ok("the legend reads running, waiting its turn, stopped and finished",
      JSON.stringify(runs.legend) === JSON.stringify(["도는 중", "차례 기다림", "멈춤", "끝남"]), JSON.stringify(runs.legend));
    // Today's count is the draft's six (t-70264): the History tab's table holds the same rows — the three under
    // their goal here, and three finished outside any goal that this table does not draw.
    ok("a report waiting on you is your turn's row, not a finished one: three finished tasks fold, and today's count is the foot",
      !now.reported && JSON.stringify(now.doneFold) === JSON.stringify(["끝난 과업 3개"])
        && now.runsFoot === "오늘 끝난 일 6개 · 기록에서 보기" && now.runningHint === "목표별 · 먼저 시작한 일부터",
      JSON.stringify(now));

    /* ---- 결정: 우리말 한 줄 · 원문은 접힌 안 · 단추는 원장의 두 답 ---- */
    await draftScene(page, { holds: true });
    await page.evaluate(() => { boardNowChoice.asksAll = true; paintCoordinatorDesk(document.querySelector("#board-view")); });
    const hold = await page.evaluate(() => {
      const row = document.querySelector('#board-view [data-ask="hold:run-desk/gate-701"]');
      const asked = document.querySelector('#board-view [data-ask="hold:run-desk/gate-702"]');
      return {
        title: row.querySelector(".board-ask-title").textContent,
        buttons: [...row.querySelectorAll(".board-ask-acts > button:not([hidden])")].map((one) => one.textContent),
        raw: row.querySelector(".board-raw")?.open === false && row.querySelector(".board-raw-text").textContent.includes("exited leaving"),
        asked: [...asked.querySelectorAll(".board-ask-acts > button:not([hidden])")].map((one) => one.textContent),
        askedTitle: asked.querySelector(".board-ask-title").textContent,
        english: /exited|harvest|dispatch again/.test(row.innerText),
      };
    });
    ok("a hold reads in Korean — what happened and what to do — with the ledger's sentence folded behind 원문 보기",
      hold.title.includes("w-66501") && hold.title.includes("2개") && hold.raw && !hold.english, JSON.stringify(hold));
    ok("a hold offers open, collect and start again", JSON.stringify(hold.buttons) === JSON.stringify(["열기", "거두기", "다시 띄우기"]),
      JSON.stringify(hold.buttons));
    ok("a decision somebody opened offers its asker's answers and the person's own words",
      JSON.stringify(hold.asked) === JSON.stringify(["main 기준", "릴리스 기준", "직접 답하기"]) && hold.askedTitle.startsWith("기준 커밋을"),
      JSON.stringify(hold));
    await page.click('#board-view [data-ask="hold:run-desk/gate-701"] [data-act="harvest"]');
    await page.click('#board-view [data-ask="hold:run-desk/gate-702"] [data-act="option-1"]');
    const resolved = await page.evaluate(async () => {
      await new Promise((done) => setTimeout(done, 0));
      return window.__DESK_SENT__.filter((one) => one.verb === "hold").map(({ gate, act, answer }) => [gate, act, answer]);
    });
    ok("the two presses reach gate-resolve as the ledger's answer and the asker's option, nothing invented",
      JSON.stringify(resolved) === JSON.stringify([["gate-701", "harvest", null], ["gate-702", "answer", "릴리스 기준"]]),
      JSON.stringify(resolved));

    /* ---- 탭: 워커는 워커 탭에만, 목록은 기록 탭에 ---- */
    await page.click('#board-view [data-board-tab-button="workers"]');
    const workersTab = await page.evaluate(() => ({
      workers: document.querySelector('#board-view [data-desk-block="workers"]')?.checkVisibility() ?? false,
      asks: document.querySelector('#board-view [data-desk-block="asks"]') !== null,
      head: document.querySelector("#board-view .task-board-intro").checkVisibility(),
    }));
    ok("the Workers tab shows the workers and holds nothing of Now", workersTab.workers && !workersTab.asks && !workersTab.head,
      JSON.stringify(workersTab));
    await page.keyboard.press("ArrowRight");
    const historyTab = await page.evaluate(() => ({
      tab: document.querySelector("#board-view .task-board-surface").dataset.boardTab,
      focused: document.activeElement?.dataset.boardTabButton,
      list: document.querySelector("#board-view .task-board-filters").checkVisibility(),
    }));
    ok("an arrow key moves to History, which holds the agents' list", historyTab.tab === "history"
      && historyTab.focused === "history" && historyTab.list, JSON.stringify(historyTab));
    await page.click('#board-view [data-board-tab-button="now"]');

    /* ---- 다크·라이트 · 1440·720, 가로로 흐르지 않음 ---- */
    await mkdir(SHOTS, { recursive: true });
    for (const theme of ["dark", "light"]) {
      for (const width of [1440, 720]) {
        await page.setViewportSize({ width, height: 900 });
        await page.evaluate((light) => {
          if (light) document.documentElement.setAttribute("data-theme", "light");
          else document.documentElement.removeAttribute("data-theme");
        }, theme === "light");
        await fullBoard(page);
        const size = await page.evaluate(() => {
          const surface = document.querySelector("#board-view .task-board-surface");
          return { width: surface.clientWidth, scroll: surface.scrollWidth };
        });
        ok(`the board has no horizontal overflow at ${width}px (${theme})`, size.scroll <= size.width + 1, JSON.stringify(size));
        if (width === 1440) {
          // A theme that just changed is still crossfading its colours: read them once every transition ended.
          await page.evaluate(() => Promise.all(document.getAnimations()
            .filter((one) => one instanceof CSSTransition).map((one) => one.finished.catch(() => {}))));
          const contrast = await page.evaluate(nowContrast);
          ok(`every word on the Now tab clears 4.5:1 in ${theme} (lowest ${contrast.min}:1)`, contrast.below.length === 0
            && contrast.min >= 4.5, JSON.stringify(contrast));
          // The Workers and History tabs (t-70264), each with its first row opened so the details are read too.
          for (const id of ["workers", "history"]) {
            await page.evaluate((one) => {
              const view = document.querySelector("#board-view");
              chooseBoardTab(view, one, false);
              view.querySelector(".board-table tbody :is(.board-desk-worker-fold, .board-today-fold)")?.click();
            }, id);
            await fullBoard(page);
            await page.evaluate(() => Promise.all(document.getAnimations()
              .filter((one) => one instanceof CSSTransition).map((one) => one.finished.catch(() => {}))));
            const tabContrast = await page.evaluate(nowContrast);
            const sizes = await page.evaluate(boardTypeSizes);
            const opened = await page.evaluate(() => document.querySelectorAll(
              "#board-view .board-table tbody :is(.board-desk-worker-more, .board-today-more):not([hidden])").length);
            ok(`every word on the ${id} tab clears 4.5:1 in ${theme} and stands in at most four sizes (lowest ${tabContrast.min}:1; ${sizes.join(", ")} px)`,
              opened === 1 && tabContrast.below.length === 0 && tabContrast.min >= 4.5 && sizes.length <= 4,
              JSON.stringify({ opened, tabContrast, sizes }));
            await page.evaluate(() => {
              // A board without the tables (a red tree) has no opened rows to forget.
              deskChoice.workersOpen?.clear();
              deskChoice.todayOpen?.clear();
              paintCoordinatorDesk(document.querySelector("#board-view"));
            });
          }
          await page.evaluate(() => chooseBoardTab(document.querySelector("#board-view"), "now", false));
          await fullBoard(page);
        }
        await page.screenshot({ path: `${SHOTS}/board-${width}-${theme}.png` });
      }
    }
    await page.evaluate(() => document.documentElement.removeAttribute("data-theme"));

    /* ---- 넓은 창: 판은 창의 폭을 다 쓰고, 시안의 격자로 나눈다 (t-70028) ---- */
    for (const width of [1920, 2560]) {
      await page.setViewportSize({ width, height: 1200 });
      await fullBoard(page);
      const wide = await page.evaluate(readNow);
      const gap = await page.evaluate(() => parseFloat(getComputedStyle(document.documentElement).getPropertyValue("--space-5")));
      ok(`at ${width}px the board runs gutter to gutter — no column in the middle of an empty floor`,
        Math.abs(wide.desk.width - wide.inner) <= 1 && !wide.sideways && wide.tabs.left === wide.surface.left
          && Math.abs(wide.running.width - wide.desk.width) <= 1,
        JSON.stringify(wide));
      ok(`at ${width}px your turn and the build line split the width two to one, side by side`,
        Math.abs(wide.asks.width - (wide.desk.width - gap) * 2 / 3) <= 2 && Math.abs(wide.release.right - wide.desk.right) <= 1,
        JSON.stringify({ asks: wide.asks, release: wide.release, desk: wide.desk }));
      await page.screenshot({ path: `${SHOTS}/board-${width}-dark.png` });
    }
    await page.setViewportSize({ width: 1440, height: 900 });

    /* ---- 끝난 레인은 한 줄로 접히고, 줄이 비면 그렇다고 말한다 ---- */
    await draftScene(page, { lane: "green" });
    const green = await page.evaluate(readNow);
    ok("a finished lane folds to its verdict and when it ended, its steps behind the record",
      /^초록 · \d{2}:\d{2} 끝남$/.test(green.line?.status ?? "") && green.line.dot === green.tokens.done
        && !green.line.listShown && green.line.steps.length === 0 && /· \d+시간 \d+분 걸림$/.test(green.line.meta),
      JSON.stringify(green.line));
    ok("with nothing waiting on the build line the card says the line is empty", green.line?.foot === "줄 비어 있음",
      JSON.stringify(green.line));

    /* ---- 원장이 없는 창: 탭도 문장도 없이 예전 목록 ---- */
    await page.evaluate(draftBoardOverlay, { ledger: false });
    await fullBoard(page);
    const bare = await page.evaluate(() => {
      const surface = document.querySelector("#board-view .task-board-surface");
      return { ledger: surface.dataset.boardLedger, tabs: surface.querySelector(".board-now-tabs").checkVisibility(),
        list: surface.querySelector(".task-board-sections").checkVisibility() };
    });
    ok("a window with no ledger run keeps the agents' list whole, without tabs", bare.ledger === "none" && !bare.tabs && bare.list,
      JSON.stringify(bare));

    /* ---- 30과업·7워커, CPU 4배 감속: 그림 ≤ 3 ms · 요소 ≤ 400 ---- */
    await page.evaluate(draftBoardOverlay, { extra: 22 });
    await installPerfHands(page);
    await fullBoard(page);
    const cdp = await page.context().newCDPSession(page);
    await cdp.send("Emulation.setCPUThrottlingRate", { rate: 4 });
    const paints = [];
    for (let round = 0; round < 15; round += 1) {
      paints.push(await page.evaluate(() => {
        const view = document.querySelector("#board-view");
        const from = performance.now();
        paintCoordinatorDesk(view);
        return performance.now() - from;
      }));
    }
    await cdp.send("Emulation.setCPUThrottlingRate", { rate: 1 });
    const weight = await page.evaluate(() => ({
      tasks: deskLedger.tasks.length, workers: deskAgents.length,
      elements: document.querySelector("#board-view .task-board-surface").querySelectorAll("*").length,
    }));
    const p50 = median(paints);
    ok(`thirty tasks and seven workers paint in at most 3 ms on the low-spec profile (p50 ${p50.toFixed(2)} ms)`,
      weight.tasks >= 30 && weight.workers >= 7 && p50 <= 3, JSON.stringify({ ...weight, paints: paints.map((one) => one.toFixed(2)) }));
    ok(`the board stays within 400 elements (${weight.elements})`, weight.elements <= 400, JSON.stringify(weight));

    /* ---- 워커·기록 탭 (t-70264): 같은 장면, 같은 자 — 그림 ≤ 3 ms · 요소 ≤ 400 ---- */
    for (const id of ["workers", "history"]) {
      await page.evaluate((one) => chooseBoardTab(document.querySelector("#board-view"), one, false), id);
      await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
      await cdp.send("Emulation.setCPUThrottlingRate", { rate: 4 });
      const tabPaints = [];
      for (let round = 0; round < 15; round += 1) {
        tabPaints.push(await page.evaluate(() => {
          const view = document.querySelector("#board-view");
          const from = performance.now();
          paintCoordinatorDesk(view);
          return performance.now() - from;
        }));
      }
      await cdp.send("Emulation.setCPUThrottlingRate", { rate: 1 });
      const tabWeight = await page.evaluate(() => ({
        elements: document.querySelector("#board-view .task-board-surface").querySelectorAll("*").length,
        rows: document.querySelectorAll("#board-view .board-table tbody").length,
      }));
      const tabP50 = median(tabPaints);
      ok(`the ${id} tab paints in at most 3 ms on the low-spec profile and stays within 400 elements (p50 ${tabP50.toFixed(2)} ms, ${tabWeight.elements} elements)`,
        tabWeight.rows >= 6 && tabP50 <= 3 && tabWeight.elements <= 400,
        JSON.stringify({ ...tabWeight, paints: tabPaints.map((one) => one.toFixed(2)) }));
    }
    await page.evaluate(() => chooseBoardTab(document.querySelector("#board-view"), "now", false));
    ok("the board raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
  await testBoardNowMinute(browser, origin, ok);
}

/* ---- 1분 박자: 원장이 움직이지 않아도 시간 글자가 간다, 바뀐 글자만, 숨으면 0 ----
 *
 * 사람 10-22: 「프로그레스도 멈춰 있음」. 가짜 시계로 1분을 보내면 도는 행의 「N분째」, 빌드 줄 단계의
 * 「N분째」, 질문의 「N분 전」이 하나씩 오른다 — 원장 이벤트(`ledger:changed`)는 하나도 없이. 그 1분에
 * 판에 생긴 변경은 그 글자들뿐이고, 창이 숨은 3분 동안은 아무것도 쓰지 않는다. */
async function testBoardNowMinute(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin, {
    before: async (one) => { await one.clock.install(); },
  });
  try {
    await installBoardWaits(page);
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.evaluate(coordinatorDeskFixture, { tasks: 0, workers: 1, mail: 0 });
    await draftScene(page);
    await page.clock.runFor(2_000);
    const minuteWords = () => page.evaluate(() => {
      const board = document.querySelector("#board-view");
      const pick = (selector) => [...board.querySelectorAll(selector)].map((node) => node.textContent.trim());
      return {
        rows: pick(".board-run .board-run-end").filter((word) => word.endsWith("분째")),
        step: pick(".board-desk-release-phase.is-running .board-desk-release-step-word"),
        asked: pick('.board-asks > li.board-desk-letter .board-desk-letter-who'),
      };
    });
    const minutes = (words) => [...words.rows, ...words.step].map((word) => Number.parseInt(word, 10));
    const before = await minuteWords();
    await page.evaluate(() => {
      window.__MINUTE_WRITES__ = [];
      const surface = document.querySelector("#board-view .task-board-surface");
      window.__MINUTE_WATCH__ = new MutationObserver((records) => {
        for (const record of records) window.__MINUTE_WRITES__.push(`${record.type}:${record.target.className || record.target.parentElement?.className}`);
      });
      window.__MINUTE_WATCH__.observe(surface, { subtree: true, childList: true, characterData: true, attributes: true });
    });
    const asked = await page.evaluate(() => window.__IN_FLIGHT__ ? [...window.__IN_FLIGHT__.keys()] : []);
    // The beat is one minute; its phase is the board's opening, so a word can wait a beat to turn.
    await page.clock.runFor(2 * 60_000);
    const after = await minuteWords();
    const writes = await page.evaluate(() => window.__MINUTE_WRITES__.splice(0));
    const moved = minutes(after).map((count, at) => count - minutes(before)[at]);
    const age = (words) => Number.parseInt(words.asked[0]?.split(" · ").at(-1) ?? "", 10);
    ok("within two minutes, with no ledger event, every running minute moved on: the rows, the build step, the question's age",
      before.rows.length === 2 && before.step.length === 1 && moved.length === 3 && moved.every((step) => step >= 1 && step <= 2)
        && age(after) - age(before) >= 1,
      JSON.stringify({ before, after, moved, asked }));
    ok("those minutes write only the words that changed — the time cells and the age, nothing else on the board",
      writes.length > 0 && writes.length <= 16 && writes.every((one) => one.startsWith("childList:")
        // the time cells, the build step's minutes, the record's time cell, and the rows' ages
        && ["board-run-end", "board-desk-release-step-word", "", "board-desk-letter-who", "board-desk-letter-age",
          "board-ask-meta"].some((name) => one.endsWith(`:${name}`))),
      JSON.stringify(writes));
    /* ---- 워커 탭의 1분 (t-70264): 원장이 움직이지 않아도 경과 칸이 오른다 — 쓰는 것은 그 글자와 마지막
     * 활동의 나이뿐이다 ---- */
    await page.evaluate(() => chooseBoardTab(document.querySelector("#board-view"), "workers", false));
    await page.clock.runFor(2_000);
    const tookWords = () => page.evaluate(() => [...document.querySelectorAll("#board-view .board-desk-worker-took")]
      .map((node) => node.textContent));
    const tookBefore = await tookWords();
    await page.evaluate(() => window.__MINUTE_WRITES__.splice(0));
    await page.clock.runFor(2 * 60_000);
    const tookAfter = await tookWords();
    const tookWrites = await page.evaluate(() => window.__MINUTE_WRITES__.splice(0));
    ok("on the Workers tab, within two minutes and with no ledger event, every elapsed cell moved on — and only those words and the last word's age were written",
      tookBefore.length === 7 && tookAfter.length === 7 && tookAfter.every((word, at) => word !== tookBefore[at]) &&
      tookWrites.length > 0 && tookWrites.every((one) => one.startsWith("childList:") &&
        ["board-desk-worker-took", "board-desk-worker-age"].some((name) => one.includes(name))),
      JSON.stringify({ tookBefore, tookAfter, tookWrites }));
    await page.evaluate(() => chooseBoardTab(document.querySelector("#board-view"), "now", false));
    await page.clock.runFor(2_000);
    await page.evaluate(() => window.__MINUTE_WRITES__.splice(0));
    await page.evaluate(() => {
      Object.defineProperty(document, "visibilityState", { configurable: true, get: () => "hidden" });
      Object.defineProperty(document, "hidden", { configurable: true, get: () => true });
      document.dispatchEvent(new Event("visibilitychange"));
    });
    await page.clock.runFor(3 * 60_000);
    const hidden = await page.evaluate(() => window.__MINUTE_WRITES__.splice(0));
    ok("a hidden window's board writes nothing for three minutes", hidden.length === 0, JSON.stringify(hidden));
    ok("the minute raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  let failures = 0;
  try {
    await testBoardNow(browser, origin, (name, pass, detail = "") => {
      console.log(`${pass ? "PASS" : "FAIL"} ${name}${!pass && detail ? `\n${detail}` : ""}`);
      if (!pass) failures++;
    });
  } finally {
    await browser.close();
    files.close();
  }
  if (failures) process.exitCode = 1;
}
