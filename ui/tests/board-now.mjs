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
import { frameBudgetHolds, loadNote } from "./machine-load.mjs";
import { nodeCounterSource } from "./node-count.mjs";

const SHOTS = "output/playwright/board-now";
/* The low-spec paint's bound and its rounds: 15 paints of the settled board at the low-spec profile's CPU 4x
 * (tools/low-spec/profile.mjs). The page's clock moves in steps of 0.1 ms, so a reading below the next step is at most
 * the budget. */
const PAINT_MS_MAX = 3;
const CLOCK_STEP_MS = 0.1;
const PAINT_ROUNDS = 15;
const CPU_THROTTLE = 4;

/* Paints the settled board over itself `PAINT_ROUNDS` times at the low-spec CPU, timing each paint, and counts, on
 * every road a node is made by and in every record the surface takes, what those paints did to the page. The clock is
 * this machine's; the counts are the code's, the same on a shared runner as on a quiet machine of our own — a paint
 * over the same facts that builds rows again or rewrites a cell is what puts it over its 3 ms, and it shows in the
 * counts whatever the runner's speed. */
async function paintSettledBoard(page, cdp) {
  await cdp.send("Emulation.setCPUThrottlingRate", { rate: CPU_THROTTLE });
  try {
    return await page.evaluate(({ rounds, counterSource }) => {
      const view = document.querySelector("#board-view");
      // (node-count.mjs)
      const counter = new Function(`return (${counterSource})`)()();
      const watch = new MutationObserver(() => {});
      watch.observe(view.querySelector(".task-board-surface"), { subtree: true, childList: true, attributes: true, characterData: true });
      const paints = [];
      let written = 0;
      try {
        for (let round = 0; round < rounds; round += 1) {
          const from = performance.now();
          paintCoordinatorDesk(view);
          paints.push(performance.now() - from);
          written += watch.takeRecords().length;
        }
      } finally {
        watch.disconnect();
        counter.restore();
      }
      return { paints, made: counter.made, written };
    }, { rounds: PAINT_ROUNDS, counterSource: nodeCounterSource });
  } finally {
    await cdp.send("Emulation.setCPUThrottlingRate", { rate: 1 });
  }
}

/* 시안 B의 예시 상황 — 세 목표, 질문 하나·검토 하나, 도는 일 둘·기다림 하나·멈춤
 * 하나, 끝난 일 셋, 빌드 하나(새 버전 발행 — 결합 시험·앱 빌드 끝, zo 시험 8분째, 발행 대기: 쓰는 쪽이
 * 남긴 기록의 말 그대로), 그리고 일정의 빌드 줄에서 차례를 기다리는 재선택·대기 고침. 워커 탭과 기록 탭의 시안 값도 싣는다(t-70264): 워커마다
 * 계속 판정이 읽은 비용·호출과 갈라져 나온 뒤의 줄, 그리고 오늘 끝난 일 여섯(목표 밖의 셋은 지금
 * 표에 서지 않는다). 데스크 픽스처가 세운 답 위에 덮는다(그 픽스처가 응답 손·보드 열기를 맡는다).
 * `extra`는 과업 수를 늘려 성능을 잰다. `build`가 "passed"면 빌드는 07:31 무렵 통과로 끝났고 빌드
 * 줄은 비어 있다. "none"이면 이 프로젝트엔 빌드 기록도 빌드 줄도 없다. A page function: no closure. */
export function draftBoardOverlay({ now = Date.now(), holds = false, longHold = false, longAsk = false, extra = 0, ledger = true, build = "running", withdrawn = null, stageMinutes = 8, stalled = null } = {}) {
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
      // `withdrawn`: the coordinator took back its verification of this attempt and source (t-81913).
      progress: { stage: "report_only", attempt: "dp-66791", source: head(7), ...(withdrawn === null ? {} : { withdrawn }) },
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
  // Decisions whose askers offered long answers (t-79611): the question must keep its width.
  if (longHold) {
    const decision = (id, gateId, title, question, options) => task(id, title, "gate", {
      ...board, gate: { id: gateId, kind: "asked", worker: null, checkout: null, examined: null, options, question },
    });
    // three whole sentences
    tasks.push(decision("t-66700", "gate-703", "예시 설정 기본값 정하기",
      "예시 설정의 기본값을 어느 쪽으로 정할까요? 아래 세 가지 중에서 골라 주세요.", [
        "예시 설정의 기본값은 지금 쓰는 쪽으로 두고, 바꾸는 일은 다음 묶음에서 한꺼번에 하기(되돌리기 쉬움)",
        "기본값을 새 쪽으로 바로 바꾸되, 예전 값으로 돌아가는 길을 설정 화면에 한 줄로 남겨 두기",
        "두 값을 모두 보여 주는 선택 칸을 먼저 만들고, 기본값은 사람이 한 번 고른 뒤에 정하기 — 시간이 더 든다",
      ]));
    // an answer and a question that each hold one long word with no space and no hyphen or slash to break at (a branch name, a hash)
    tasks.push(decision("t-66701", "gate-704", "예시 체크아웃 고르기",
      "이 과업은 example_workspace_project_t12345_long_checkout_name_0123456789abcdef0123456789abcdef0123456789abcdef 에서 이어 쓸까요?", [
        "example_workspace_project_t12345_long_checkout_name_0123456789abcdef0123456789abcdef0123456789abcdef",
        "새 체크아웃",
      ]));
    // six sentences
    tasks.push(decision("t-66702", "gate-705", "예시 순서 정하기",
      "다음 여섯 가지 순서 중 어느 것으로 할까요?",
      [1, 2, 3, 4, 5, 6].map((n) => `${n}번째 순서: 먼저 이 묶음을 닫고 나서, 남은 일을 같은 날 안에 차례로 이어서 하기`)));
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
    task: "일정 타임라인", body: longAsk
      // The person's 11-08 screen: a coordinator's question of thirteen lines, paths and all.
      ? `[범위 상의 · 대화 화면 1단계] 끝날 시각을 단계별 중앙값으로 셀까요, 전체 걸린 시간으로 셀까요? ${"- 바뀔 파일: ui/tokens.css · ui/shell.css · ui/shell-conversation-view.js · ui/shell-composer.js. ".repeat(6)}답을 기다리는 동안은 읽기·계획만 합니다.`
      : "끝날 시각을 단계별 중앙값으로 셀까요, 전체 걸린 시간으로 셀까요?",
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
  /* The project's one build as its writer recorded it (`board_builds`): the release script's, in the words it
   * reported — the window keeps no step names. Four steps on the third, each beside its usual time; a passed one
   * ended 100 minutes ago. Answered with a revision, as the backend does. */
  const sha = "3e13aaef1aa0b3e2c4d5e6f708192a3b4c5d6e7f";
  const passedAt = now - 100 * minute;
  const step = (name, ko, usualSecs, extra) => ({ name, words: { ko }, expected: { secs: usualSecs, samples: 5 }, ...extra });
  const release = { id: "release", name: { ko: "새 버전 발행", en: "Publish a new version" }, label: "v1.1.84", sha };
  window.__BUILDS__ = build === "none" ? [] : build === "passed" ? [{
    ...release, state: "passed", started_ms: passedAt - 101 * minute, updated_ms: passedAt,
    steps: [step("gate-root", "결합 시험", 3000, { state: "done", secs: 3168 }),
      step("build-app", "앱 빌드", 1100, { state: "done", secs: 1090 }), step("gate-zo", "zo 시험", 840, { state: "done", secs: 1719 }),
      step("publish", "발행", 120, { state: "done", secs: 42 })],
  }] : [{
    ...release, state: "running", started_ms: now - (33 + stageMinutes) * minute,
    running_since_ms: now - stageMinutes * minute, updated_ms: now - minute, ...(stalled ? { stalled } : {}),
    steps: [step("gate-root", "결합 시험", 720, { state: "done", secs: 720 }),
      step("build-app", "앱 빌드", 1260, { state: "done", secs: 1260 }), step("gate-zo", "zo 시험", 14 * 60, { state: "running" }),
      step("publish", "발행", 120, { state: "ahead" })],
  }];
  // The schedule's reading (t-67482): the same runs the card reads, and the build line behind them — 재선택·대기
  // 고침's job waits its turn. A passed build leaves the line empty, and a project with neither has no line at all.
  window.__ANSWER__.schedule_timeline = () => ({
    revision: { running: 1, passed: 2, none: 3 }[build], unchanged: false, read_ms: now,
    line: { found: build !== "none", paused: false, runner_lanes: [1, 2], disk_low: false, quiet_wanted: false },
    jobs: build !== "running" ? [] : [{
      name: `${Math.floor(now / 1000)}-run4275-t66912-build`, who: "run4275-t66912", state: "waiting", class: "plain",
      lane: null, cap_secs: 1800, queued_ms: now - 5 * minute, start_ms: null, end_ms: null, secs: null, rc: null,
      kind: null, gate: null, part: null, what: null, expected: { secs: 900, samples: 3 },
    }],
    builds: window.__BUILDS__,
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
 * 셌고, 그 첫 화면의 「Opus 5.5」·「Sonnet 5.5」는 세지 않았다. 사람·에이전트가 쓴 과업과
 * 목표의 제목도 이름이다(t-78802): 판이 센 수가 아니라 이름의 일부라, 「상황판 디자인 시안
 * 2차」의 2는 시안의 셈에 없다 — 제목은 홀로 서든 문장 속에 끼든 빼고 센다. */
function boardFirstScreen() {
  const view = document.querySelector("#board-view");
  const surface = view.querySelector(".task-board-surface");
  const fold = window.innerHeight;
  const visible = (el) => el.checkVisibility({ checkOpacity: true, checkVisibilityCSS: true })
    && el.getBoundingClientRect().top < fold && el.getBoundingClientRect().bottom > 0;
  const all = [...surface.querySelectorAll("*")].filter(visible);
  const walker = document.createTreeWalker(surface, NodeFilter.SHOW_TEXT);
  const names = [...new Set(deskLedger.tasks.flatMap((task) => [task.title, task.parent_title]).filter(Boolean))]
    .sort((a, b) => b.length - a.length);
  let figures = 0;
  let raw = 0;
  const words = [];
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    const host = node.parentElement;
    if (!host || !node.textContent.trim() || !visible(host) || host.closest('code, pre, details:not([open]), [translate="no"]')) continue;
    // The build card's step band is a chart, approved with a time under each segment (the build card's Geist
    // draft): its labels are the band's scale, one per segment at most `BUILD_BAND_MOST`, and not figures of
    // the screen's sentences. The card's own line — how long the build has run — is counted.
    if (host.closest(".board-build-band")) continue;
    words.push(node.textContent);
    const said = names.reduce((text, name) => text.replaceAll(name, " "), node.textContent);
    figures += (said.match(/(?<![\p{L}\-.\d])\d+(?:[:.,]\d+)*/gu) ?? []).length;
    if (/exited in|nobody has looked|before reporting|dispatch again|resolve this/i.test(node.textContent)) raw += 1;
  }
  const sizes = new Set(all.filter((el) => [...el.childNodes].some((one) => one.nodeType === 3 && one.textContent.trim()))
    .map((el) => parseFloat(getComputedStyle(el).fontSize)));
  const cards = all.filter((el) => el.matches('.board-desk-block:is([data-desk-block="asks"], [data-desk-block="builds"], [data-desk-block="running"])'));
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

/* The draft's scene, with the facts the desk reads on its slow beat asked at once: the builds,
 * the schedule's reading, the workers' checkouts and their models. */
export async function draftScene(page, options = {}) {
  await page.evaluate(draftBoardOverlay, options);
  await page.evaluate(async () => {
    await askBoardBuilds();
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
    return rect ? { left: Math.round(rect.left), right: Math.round(rect.right), width: Math.round(rect.width),
      top: Math.round(rect.top), bottom: Math.round(rect.bottom) } : null;
  };
  const style = getComputedStyle(surface);
  const inner = surface.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight);
  // A dot is its cell's ::before (the rows, the legend, the build's state).
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
  const card = view.querySelector('[data-desk-block="builds"]');
  const one = card?.querySelector(".board-build");
  const shown = (node) => Boolean(node) && !node.hidden;
  return {
    inner: Math.round(inner),
    surface: box(surface),
    tabs: box(view.querySelector(".board-now-tabs")),
    desk: box(view.querySelector(".task-board-desk")),
    asks: box(view.querySelector('[data-desk-block="asks"]')),
    builds: box(card),
    running: box(view.querySelector('[data-desk-block="running"]')),
    sideways: surface.scrollWidth > surface.clientWidth + 1,
    legend: [...view.querySelectorAll(".board-legend > li")].map((item) => [item.textContent, dotOf(item)]),
    tokens: { run: token("--status-dot-run"), queue: token("--status-dot-queue"), halt: token("--status-dot-halt"),
      done: token("--status-dot-done") },
    spinning: [...view.querySelectorAll('.task-board-desk [data-desk-block="running"] *, .task-board-desk [data-desk-block="builds"] *')]
      .filter((node) => getComputedStyle(node).animationName !== "none" || getComputedStyle(node, "::before").animationName !== "none")
      .map((node) => node.className).slice(0, 5),
    rows: { queue: cells(row("t-66912")), run: cells(row("t-67561")), runHands: cells(row("t-66788")), halt: cells(row("t-66410")) },
    order: [...view.querySelectorAll(".board-run")].map((node) => node.dataset.task.split("/")[1]),
    reported: Boolean(row("t-66790")),
    doneFold: [...view.querySelectorAll(".board-goal.is-done .board-goal-name")].map((node) => node.textContent),
    runsFoot: view.querySelector(".board-runs-foot")?.textContent,
    runningHint: view.querySelector('[data-desk-block="running"] .board-desk-head-note')?.textContent,
    build: one ? {
      cls: one.className,
      state: one.querySelector(".board-build-state")?.textContent,
      // The state's dot stands before the build's name (11-08 draft v3); the word at its right draws none.
      dot: one.querySelector(".board-build-dot") ? getComputedStyle(one.querySelector(".board-build-dot")).backgroundColor : null,
      meta: [...one.querySelector(".board-build-meta").children].filter(shown).map((node) => node.textContent),
      band: [...one.querySelectorAll(".board-build-seg")].map((seg) =>
        [seg.className.replace("board-build-seg ", ""), seg.children[1].textContent, seg.children[2].textContent]),
      bar: parseFloat(getComputedStyle(one.querySelector(".board-build-bar")).height),
      timesShown: [...one.querySelectorAll(".board-build-seg-time")].filter((node) => node.checkVisibility()).length,
      log: one.querySelector(".board-build-log-head")?.textContent,
      rows: [...one.querySelectorAll(".board-build-log-step")].map((row) => `${row.children[1].textContent}${row.children[2].textContent}`),
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
    ok("the second line names what ends next and when — the running build, at the time its steps' usual times add up to",
      /^다음으로 끝나는 일은 새 버전 발행 v1\.1\.84, .+쯤입니다\.$/.test(head.sub), head.sub);

    /* ---- 늦은 단계와 멈춘 단계: 지난 시각 대신 「평소보다 늦음」, 기록이 적은 「멈춘 듯」 ---- */
    const sentence = async (options) => {
      await draftScene(page, options);
      return page.evaluate(() => document.querySelector("#board-view .task-board-summary").textContent);
    };
    const late = await sentence({ stageMinutes: 30 });
    ok("a step past its usual time says it is late and how long it has run, never a time that has gone by",
      late === "새 버전 발행 v1.1.84 · 평소보다 늦음 · zo 시험 30분째", late);
    /* 「21분」은 그리는 시계로 센다: 페이지 시계를 한 시각에 묶고 기록이 적은 시각을 그 시계에서
     * 뺀다 — 그리기가 늦게 와도 같은 수다. */
    const pinned = await page.evaluate(() => {
      window.__TRUE_NOW__ = Date.now;
      const at = Date.now();
      Date.now = () => at;
      return at;
    });
    let stalledSaid;
    try {
      stalledSaid = await sentence({ stageMinutes: 30,
        stalled: { step: "gate-zo", what: "test", quiet_since_ms: pinned - 21 * 60_000 - 5_000, cap_secs: 2400 } });
    } finally {
      await page.evaluate(() => { Date.now = window.__TRUE_NOW__; delete window.__TRUE_NOW__; });
    }
    ok("a step the record says has written nothing for long is the sentence, in the record's words",
      stalledSaid === "새 버전 발행 v1.1.84 · 멈춘 듯 — test 21분 출력 없음", stalledSaid);
    const onTime = await sentence({ stageMinutes: 8 });
    ok("a step within its usual time still names when the build ends", /^다음으로 끝나는 일은 새 버전 발행 v1\.1\.84, .+쯤입니다\.$/.test(onTime), onTime);

    /* ---- 시안의 넷: 숫자 ≤ 8 · 묶음 3 · 글자 단계 4 · 영어 진단 0 ---- */
    const first = await page.evaluate(boardFirstScreen);
    /* 2차 디자인(t-78802)의 수: 8/8, 여유 0 — 2개·시각·3분·12분·21분째·26분째·끝난 과업 3개·오늘 끝난 일 6개. 빌드
     * 카드가 낮아져 마지막 것이 900 px 안에 올라왔고, 그 자리는 카드가 냈다: 홀로 선 빌드는 차례 원을 접고, 단계가
     * 도는 동안의 경과는 도는 칸 하나가 말한다(띠는 그림이라 셈 밖이다 — `boardFirstScreen`).
     * 여덟째인 끝난 과업 수는 시안 15(상황판 머리 253 → 136 px)가 돌려준 자리로 900 px 안에 올라왔다.
     * 다음에 숫자가 하나 더 필요하면 예산을 올리지 말고, 목록이 선 자리(끝난 과업 줄을 900 px 아래로,
     * 또는 접기)에서 자리를 낸다.
     * 11-08 시안 v3: 기다리는 행의 끝이 「대기」 대신 기다린 시간(1시간째)이 되어 숫자가 하나 늘었고, 그 자리는 표의 발이
     * 냈다 — 오늘 끝난 수는 기록 탭이 세고, 발은 「오늘 끝난 일 · 기록에서 보기」 문만 말한다. */
    ok("the first screen at 1440×900 carries at most eight figures", first.figures <= 8, JSON.stringify(first));
    ok("the first screen is three groups: your turn, the builds, running now", first.cards === 3, JSON.stringify(first));
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
    ok("a stopped row says 멈춤 at its end and keeps 원문 보기 beside its words; a waiting one says once what it waits for — no worker sent yet, and whose run — and how long it has waited",
      now.rows.halt?.end === "멈춤" && now.rows.halt?.raw === "원문 보기" && now.rows.halt?.work.includes("워커 화면이 사라졌습니다")
        && now.rows.queue?.end === "1시간째" && now.rows.queue?.word === "차례 기다림"
        && now.rows.queue?.work === "띄울 워커가 없습니다 · 시안 B",
      JSON.stringify(now.rows));
    ok("the badge of your turn is the draft's solid blue under white words",
      now.badge?.ink === "rgb(255, 255, 255)" && now.badge?.ground === "rgb(0, 98, 209)", JSON.stringify(now.badge));
    ok("each row of your turn carries the draft's line: the goal, the task and its id, the model, how long ago",
      now.askMeta[0] === "일정 타임라인 t-67490 · Opus 5.5 · 3분 전"
        && now.askMeta[1] === "컴퓨터 사용을 사람처럼 · 헬퍼 회차 A·B·C t-66790 · Sonnet 5.5 · 12분 전 — 통과시키면 다음 발행에 같이 실립니다",
      JSON.stringify(now.askMeta));

    /* ---- 긴 질문은 세 줄로 접힌다 (11-08 사람의 화면: 질문 하나가 열세 줄로 카드 높이를 다 썼다) ---- */
    await draftScene(page, { longAsk: true });
    const readFold = () => page.evaluate(() => {
      const row = document.querySelector('#board-view .board-asks > li[data-ask^="q"], #board-view .board-asks > li');
      const body = row?.querySelector(".board-desk-letter-body");
      const toggle = row?.querySelector(".board-ask-fold");
      const line = body ? parseFloat(getComputedStyle(body).lineHeight) : 0;
      return { lines: body ? Math.round(body.getBoundingClientRect().height / line) : 0, toggle: toggle && !toggle.hidden ? toggle.textContent : null,
        expanded: toggle?.getAttribute("aria-expanded") ?? null };
    });
    const foldedAsk = await readFold();
    await page.click("#board-view .board-ask-fold");
    await fullBoard(page);
    const openedAsk = await readFold();
    await page.click("#board-view .board-ask-fold");
    await draftScene(page);
    const shortAsk = await readFold();
    ok("a long question shows three lines and 「전체 보기」, which opens it whole and turns to 「접기」; a short one has no fold",
      foldedAsk.lines === 3 && foldedAsk.toggle === "전체 보기" && foldedAsk.expanded === "false"
        && openedAsk.lines > 3 && openedAsk.toggle === "접기" && openedAsk.expanded === "true"
        && shortAsk.toggle === null,
      JSON.stringify({ foldedAsk, openedAsk, shortAsk }));

    /* ---- 빌드: 시안의 카드 — 상태, 몇째 단계, 평소 시간이 길이인 띠 ---- */
    ok("the build card says running at its right, and which of how many steps the build is on",
      now.build?.state === "도는 중" && now.build.dot === now.tokens.run && now.build.meta.includes("네 단계 중 셋째"),
      JSON.stringify(now.build));
    ok("its band is the record's steps in its words: two done with what they took, the running one with its minutes beside its usual, the one ahead with its usual",
      JSON.stringify(now.build?.band.map(([state, name]) => [state, name])) === JSON.stringify([
        ["is-done", "결합 시험"], ["is-done", "앱 빌드"], ["is-running", "zo 시험"], ["is-ahead", "발행"]])
        && now.build.band[0][2] === "12분" && /^\d+분째 · 평소 14분$/.test(now.build.band[2][2]) && now.build.band[3][2] === "약 2분",
      JSON.stringify(now.build));
    // The step list is built for whoever opens it: folded, it carries no row.
    await page.click('#board-view [data-desk-block="builds"] .board-build-log-head');
    await fullBoard(page);
    const opened = await page.evaluate(readNow);
    await page.click('#board-view [data-desk-block="builds"] .board-build-log-head');
    await fullBoard(page);
    ok("behind 「단계 4개」 each step has its time, the running one beside its usual — built only when opened",
      now.build?.log === "단계 4개" && now.build.rows.length === 0 && opened.build.rows.length === 4
        && opened.build.rows[0] === "결합 시험12분" && /^zo 시험\d+분째 · 평소 14분$/.test(opened.build.rows[2])
        && opened.build.rows[3] === "발행약 2분",
      JSON.stringify({ folded: now.build?.rows, opened: opened.build?.rows }));

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
      const passes = () => window.__DESK_SENT__.filter((one) => one.verb === "pass").length;
      // The count before the press, not a count of 1: an earlier scene's pass is not this press's to explain (t-81913 r1 P3-6).
      const before = passes();
      // A press that slips through (the button forced open) still sends nothing a second time.
      pass.disabled = false;
      pass.click();
      await new Promise((done) => setTimeout(done, 0));
      paintCoordinatorDesk(document.querySelector("#board-view"));
      return { disabled, before, sentBySecondPress: passes() - before,
        repainted: document.querySelector('#board-view [data-ask^="review:"] [data-act="pass"]').disabled };
    });
    ok("once the ledger took the pass, the row's button stays shut and a second press sends nothing (P3 ⑥)",
      shut.disabled && shut.sentBySecondPress === 0 && shut.repainted, JSON.stringify(shut));

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
    // Today's count lives on the History tab (11-08): the foot keeps its door, so the waiting row's wait has its figure.
    ok("a report waiting on you is your turn's row, not a finished one: three finished tasks fold, and the foot is the door to today's record",
      !now.reported && JSON.stringify(now.doneFold) === JSON.stringify(["끝난 과업 3개"])
        && now.runsFoot === "오늘 끝난 일 · 기록에서 보기" && now.runningHint === "목표별 · 먼저 시작한 일부터",
      JSON.stringify(now));

    /* ---- 검증 철회(t-81913): 통과 대신 철회 사실·까닭, 단추는 보고 읽기 하나, 다시 통과하는 길은 줄 끝 말 ---- */
    const withdrawnRow = async (why) => {
      await draftScene(page, { withdrawn: why });
      return page.evaluate(() => {
        boardPresses.clear();
        paintCoordinatorDesk(document.querySelector("#board-view"));
        const row = document.querySelector('#board-view [data-ask="review:run-desk/t-66790"]');
        return {
          kind: row.querySelector(".board-ask-kind").textContent,
          title: row.querySelector(".board-ask-title").textContent,
          meta: row.querySelector(".board-ask-meta").textContent,
          buttons: [...row.querySelectorAll(".board-ask-acts > button:not([hidden])")].map((one) => one.textContent),
          pass: row.querySelector('[data-act="pass"]')?.hidden ?? true,
        };
      });
    };
    // A press that slips through to the hidden pass (the button forced) sends nothing either.
    const forced = async () => page.evaluate(async () => {
      document.querySelector('#board-view [data-ask="review:run-desk/t-66790"] [data-act="pass"]')?.click();
      await new Promise((done) => setTimeout(done, 0));
      return window.__DESK_SENT__.filter((one) => one.verb === "pass").length;
    });
    const withdrawn = await withdrawnRow("독립 검수가 가격 가드의 시험이 없다고 봄");
    ok("a withdrawn verification's row says so and why in place of the pass, leaves reading the report as its one button, and its line names the road to pass again",
      withdrawn.kind === "검토" && withdrawn.pass
        && withdrawn.title === "헬퍼 회차 A·B·C 검증이 철회됐습니다 — 독립 검수가 가격 가드의 시험이 없다고 봄"
        && JSON.stringify(withdrawn.buttons) === JSON.stringify(["보고 읽기"])
        && withdrawn.meta.endsWith(" — 새 검수 결과를 적거나 새로 넘겨야 다시 통과됩니다"),
      JSON.stringify(withdrawn));
    const slipped = await forced();
    ok("a forced press on a withdrawn row's hidden pass sends nothing to the ledger", slipped === 1, String(slipped));
    const unsaid = await withdrawnRow("");
    ok("a withdrawal written without words says that it gave none, never a pass",
      unsaid.pass && unsaid.title === "헬퍼 회차 A·B·C 검증이 철회됐습니다 — 까닭은 적지 않았습니다"
        && JSON.stringify(unsaid.buttons) === JSON.stringify(["보고 읽기"]), JSON.stringify(unsaid));
    await page.click('#board-view [data-ask="review:run-desk/t-66790"] [data-act="read"]');
    const reread = await page.evaluate(async () => {
      await new Promise((done) => requestAnimationFrame(done));
      const row = document.querySelector('#board-view [data-ask="review:run-desk/t-66790"]');
      const read = row.querySelector('[data-act="read"]');
      return { open: read.getAttribute("aria-expanded"), name: read.textContent,
        report: Boolean(row.querySelector(":scope > .board-ask-report")),
        sent: window.__DESK_SENT__.filter((one) => one.verb === "pass").length };
    });
    ok("보고 읽기 on a withdrawn row opens the report, turns into 보고 접기 and writes nothing to the ledger",
      reread.open === "true" && reread.name === "보고 접기" && reread.report && reread.sent === 1, JSON.stringify(reread));
    await page.evaluate(() => boardNowChoice.reports.clear());

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

    /* ---- 긴 선택지 결정 행 (t-79611): 질문과 목표 줄은 읽히는 폭을 지키고, 선택지는 잘리지 않는다 ---- */
    await draftScene(page, { holds: true, longHold: true });
    await page.evaluate(() => { boardNowChoice.asksAll = true; paintCoordinatorDesk(document.querySelector("#board-view")); });
    await mkdir(SHOTS, { recursive: true });
    const longScenes = [
      { key: "gate-703", label: "three long answers", count: 4, first: "예시 설정의 기본값은" },
      { key: "gate-704", label: "one unbreakable word as an answer and in the question", count: 3, first: "example_workspace" },
      { key: "gate-705", label: "six answers", count: 7, first: "1번째 순서" },
    ];
    for (const width of [2560, 1920, 1440, 1000, 720]) {
      await page.setViewportSize({ width, height: 900 });
      await fullBoard(page);
      const squeeze = await page.evaluate((keys) => {
        const surface = document.querySelector("#board-view .task-board-surface");
        const share = parseFloat(getComputedStyle(document.documentElement).getPropertyValue("--board-ask-acts-share")) / 100;
        const read = (key) => {
          const row = document.querySelector(`#board-view [data-ask="hold:run-desk/${key}"]`);
          const rect = row.getBoundingClientRect();
          const style = getComputedStyle(row);
          const line = row.querySelector(".board-ask-line").getBoundingClientRect();
          const title = row.querySelector(".board-ask-title");
          const lineHeight = parseFloat(getComputedStyle(title).lineHeight);
          const buttons = [...row.querySelectorAll(".board-ask-acts > button:not([hidden])")];
          const acts = row.querySelector(".board-ask-acts").getBoundingClientRect();
          // How wide the question would stand on one line: the lines it needs follow from the width it keeps.
          const probe = title.cloneNode(true);
          probe.style.cssText = "position:absolute;visibility:hidden;width:max-content;white-space:nowrap;";
          title.parentNode.appendChild(probe);
          const oneLine = probe.getBoundingClientRect().width;
          probe.remove();
          const titleWidth = title.getBoundingClientRect().width;
          const inner = rect.width - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight);
          return {
            row: Math.round(rect.width),
            // The question column keeps at least what the buttons' share leaves of the row's inner width.
            keeps: Math.round(inner * (1 - share) - parseFloat(style.columnGap)),
            lineWidth: Math.round(line.width),
            titleWidth: Math.round(titleWidth),
            titleLines: Math.round(title.getBoundingClientRect().height / lineHeight),
            // A column that keeps its share wraps the question into the lines its length asks for, plus one of slack.
            linesAllowed: Math.ceil(oneLine / titleWidth) + 1,
            metaWidth: Math.round(row.querySelector(".board-ask-meta").getBoundingClientRect().width),
            sidewaysRow: row.scrollWidth > row.clientWidth + 1,
            clipped: buttons.filter((one) => one.scrollWidth > one.clientWidth + 1).length,
            outside: buttons.filter((one) => one.getBoundingClientRect().right > rect.right + 1).length,
            tops: [...new Set(buttons.map((one) => Math.round(one.getBoundingClientRect().top)))].length,
            words: buttons.map((one) => one.textContent),
            // Beside the question (not stacked under it), the buttons take no more than their share.
            actsWidth: Math.round(acts.width),
            actsCap: Math.round(inner * share) + 1,
            stacked: acts.left <= line.left + 1,
            // The goal line hangs right under the question line, however tall the answers beside them stand.
            gapBelowQuestion: Math.round(row.querySelector(".board-ask-meta").getBoundingClientRect().top - line.bottom),
            rowGap: parseFloat(style.rowGap),
            idLines: Math.max(...[...row.querySelectorAll(".board-meta-id")].map((one) => one.getClientRects().length)),
          };
        };
        const rows = Object.fromEntries([...keys, "gate-702"].map((key) => [key, read(key)]));
        return { rows, sideways: surface.scrollWidth > surface.clientWidth + 1 };
      }, longScenes.map((one) => one.key));
      for (const scene of longScenes) {
        const one = squeeze.rows[scene.key];
        ok(`at ${width}px a decision with ${scene.label} keeps its question and goal line readable (the question keeps the share the buttons leave)`,
          one.lineWidth >= one.keeps - 1 && one.titleLines <= one.linesAllowed && one.metaWidth >= one.keeps - 1
            && (one.stacked || one.actsWidth <= one.actsCap), JSON.stringify(one));
        ok(`at ${width}px with ${scene.label} the goal line hangs right under the question and the task number stays in one piece`,
          one.gapBelowQuestion <= one.rowGap + 1 && one.idLines === 1, JSON.stringify(one));
        ok(`at ${width}px with ${scene.label} the answers are read whole and nothing runs sideways`,
          one.clipped === 0 && one.outside === 0 && !one.sidewaysRow && !squeeze.sideways
            && one.words.length === scene.count && one.words[0].startsWith(scene.first) && one.words.at(-1) === "직접 답하기",
          JSON.stringify(squeeze));
      }
      ok(`at ${width}px the short answers stay on one line beside their question`,
        squeeze.rows["gate-702"].clipped === 0 && (width < 1000 || (squeeze.rows["gate-702"].titleLines === 1 && squeeze.rows["gate-702"].tops === 1)),
        JSON.stringify(squeeze.rows["gate-702"]));
      await page.locator('#board-view [data-ask="hold:run-desk/gate-703"]').screenshot({ path: `${SHOTS}/hold-long-${width}.png` });
      await page.locator('#board-view [data-ask="hold:run-desk/gate-704"]').screenshot({ path: `${SHOTS}/hold-path-${width}.png` });
      await page.locator('#board-view [data-ask="hold:run-desk/gate-705"]').screenshot({ path: `${SHOTS}/hold-six-${width}.png` });
    }
    await page.setViewportSize({ width: 1440, height: 900 });
    await draftScene(page, { holds: true });
    await page.evaluate(() => { boardNowChoice.asksAll = true; paintCoordinatorDesk(document.querySelector("#board-view")); });

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
    const readHistoryTab = () => page.evaluate(() => {
      const fold = document.querySelector("#board-view .task-board-agents-fold");
      return {
        tab: document.querySelector("#board-view .task-board-surface").dataset.boardTab,
        focused: document.activeElement?.dataset.boardTabButton,
        fold: fold?.checkVisibility() ? `${fold.textContent}|${fold.getAttribute("aria-expanded")}` : "",
        list: document.querySelector("#board-view .task-board-filters").checkVisibility(),
      };
    });
    const historyTab = await readHistoryTab();
    await page.click("#board-view .task-board-agents-fold");
    const historyOpened = await readHistoryTab();
    await page.click("#board-view .task-board-agents-fold");
    const historyFolded = await readHistoryTab();
    // 11-08 draft v3: under the record, the agents' old list waits folded behind one line that counts it.
    ok("an arrow key moves to History, which keeps the agents' list folded under 에이전트 대화 N until pressed",
      historyTab.tab === "history" && historyTab.focused === "history" &&
      /^에이전트 대화 \d+\|false$/.test(historyTab.fold) && !historyTab.list &&
      historyOpened.fold.endsWith("|true") && historyOpened.list &&
      historyFolded.fold.endsWith("|false") && !historyFolded.list,
      JSON.stringify({ historyTab, historyOpened, historyFolded }));
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
        Math.abs(wide.desk.width - wide.inner) <= 1 && !wide.sideways && wide.tabs.left === wide.surface.left,
        JSON.stringify(wide));
      // 11-08 draft v3: running now stands under your turn in the wide column, and the build card keeps the right
      // column down both — no hole under a short your-turn card.
      ok(`at ${width}px running now stands right under your turn, and the build card runs down the right column beside both`,
        Math.abs(wide.running.left - wide.asks.left) <= 1 && Math.abs(wide.running.width - wide.asks.width) <= 1
          && wide.running.top - wide.asks.bottom <= gap + 1 && wide.running.top > wide.asks.bottom
          && Math.abs(wide.builds.top - wide.asks.top) <= 1 && wide.builds.left > wide.asks.right,
        JSON.stringify({ asks: wide.asks, builds: wide.builds, running: wide.running, gap }));
      ok(`at ${width}px your turn and the build card split the width two to one, side by side`,
        Math.abs(wide.asks.width - (wide.desk.width - gap) * 2 / 3) <= 2 && Math.abs(wide.builds.right - wide.desk.right) <= 1,
        JSON.stringify({ asks: wide.asks, builds: wide.builds, desk: wide.desk }));
      await page.screenshot({ path: `${SHOTS}/board-${width}-dark.png` });
    }
    await page.setViewportSize({ width: 1440, height: 900 });

    /* ---- 끝난 빌드: 판정과 끝난 시각, 걸린 시간 — 띠는 얇아지고 칸의 시간 글자는 접힌다 ---- */
    await draftScene(page, { build: "passed" });
    const ended = await page.evaluate(readNow);
    const thin = await page.evaluate(() => parseFloat(getComputedStyle(document.documentElement).getPropertyValue("--space-1")));
    ok("a finished build folds to its verdict and when it ended, a thin band, and what it took; its steps' times behind the record",
      /^통과 · \d{2}:\d{2} 끝남$/.test(ended.build?.state ?? "") && ended.build.dot === ended.tokens.done
        && ended.build.cls === "board-build is-passed is-ended" && ended.build.bar === thin && ended.build.timesShown === 0
        && /^\d+시간 \d+분 걸림$/.test(ended.build.meta.at(-1)),
      JSON.stringify(ended.build));

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

    /* ---- 30과업·7워커, CPU 4배 감속: 그림 ≤ 3 ms · 요소 ≤ 400 ----
     *
     * The 3 ms is a wall clock, and a wall clock is judged only on a quiet machine of our own: a GitHub-hosted runner
     * records it (machine-load.mjs). Windows runners read p50 1.30 and 3.40 ms for the same history tab on two days'
     * runs (41568641665, 41533657435). What makes a paint dear is judged everywhere, by count: a paint over the facts
     * it already drew makes no node and writes nothing. */
    await page.evaluate(draftBoardOverlay, { extra: 22 });
    await installPerfHands(page);
    await fullBoard(page);
    const cdp = await page.context().newCDPSession(page);
    const nowPaint = await paintSettledBoard(page, cdp);
    const weight = await page.evaluate(() => ({
      tasks: deskLedger.tasks.length, workers: deskAgents.length,
      elements: document.querySelector("#board-view .task-board-surface").querySelectorAll("*").length,
    }));
    const p50 = median(nowPaint.paints);
    const nowDetail = JSON.stringify({ ...weight, made: nowPaint.made, written: nowPaint.written,
      paints: nowPaint.paints.map((one) => one.toFixed(2)) });
    ok(`thirty tasks and seven workers paint in at most ${PAINT_MS_MAX} ms on the low-spec profile (p50 ${p50.toFixed(2)} ms, ${loadNote()})`,
      weight.tasks >= 30 && weight.workers >= 7 && frameBudgetHolds(p50, PAINT_MS_MAX + CLOCK_STEP_MS), nowDetail);
    ok(`painted over itself ${PAINT_ROUNDS} times, the thirty-task board makes no node and writes nothing (${nowPaint.made} made, ${nowPaint.written} written)`,
      weight.tasks >= 30 && weight.workers >= 7 && nowPaint.made === 0 && nowPaint.written === 0, nowDetail);
    ok(`the board stays within 400 elements (${weight.elements})`, weight.elements <= 400, JSON.stringify(weight));

    /* ---- 워커·기록 탭 (t-70264): 같은 장면, 같은 자 — 그림 ≤ 3 ms · 요소 ≤ 400 ---- */
    for (const id of ["workers", "history"]) {
      await page.evaluate((one) => chooseBoardTab(document.querySelector("#board-view"), one, false), id);
      await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
      const tabPaint = await paintSettledBoard(page, cdp);
      const tabWeight = await page.evaluate(() => ({
        elements: document.querySelector("#board-view .task-board-surface").querySelectorAll("*").length,
        rows: document.querySelectorAll("#board-view .board-table tbody").length,
      }));
      const tabP50 = median(tabPaint.paints);
      const tabDetail = JSON.stringify({ ...tabWeight, made: tabPaint.made, written: tabPaint.written,
        paints: tabPaint.paints.map((one) => one.toFixed(2)) });
      ok(`the ${id} tab paints in at most ${PAINT_MS_MAX} ms on the low-spec profile and stays within 400 elements (p50 ${tabP50.toFixed(2)} ms, ${tabWeight.elements} elements, ${loadNote()})`,
        tabWeight.rows >= 6 && frameBudgetHolds(tabP50, PAINT_MS_MAX + CLOCK_STEP_MS) && tabWeight.elements <= 400, tabDetail);
      ok(`painted over itself ${PAINT_ROUNDS} times, the ${id} tab makes no node and writes nothing (${tabPaint.made} made, ${tabPaint.written} written)`,
        tabWeight.rows >= 6 && tabPaint.made === 0 && tabPaint.written === 0, tabDetail);
    }
    await page.evaluate(() => chooseBoardTab(document.querySelector("#board-view"), "now", false));

    /* ---- 빌드가 없는 프로젝트: 카드는 접히고 당신 차례가 줄을 다 쓴다 ---- */
    await draftScene(page, { build: "none" });
    const none = await page.evaluate(readNow);
    ok("a project with no build record folds the build card, and your turn takes the whole row",
      none.builds?.width === 0 && none.asks.width > 0 && Math.abs(none.asks.width - none.desk.width) <= 1,
      JSON.stringify({ desk: none.desk, asks: none.asks, builds: none.builds }));
    ok("the board raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
  await testBoardNowClarity(browser, origin, ok);
  await testBoardNowMinute(browser, origin, ok);
}

/* ---- 한눈에 읽히는 상황판 (11-10) ---------------------------------------------
 *
 * 사람의 11-10 화면(「지금은 상황판이 뭐하고 있는지 모르겠어」)과 같은 상황: 답할 일은 없고 원장의 과업은 모두 끝났는데,
 * 원장 밖의 에이전트 대화 셋이 일하고, 이틀 전에 취소된 빌드 둘과 통과한 발행 하나(한 단계는 실패로 적혔다)가 있고,
 * 이 컴퓨터의 빌드 줄에서 일감 하나가 돈다. 그때 판은 「지금 도는 일이 없습니다」라 했고 빈 「당신 차례」 카드가 화면의
 * 3분의 2를 썼다. `waiting`이면 대화 하나가 사람의 답을 기다린다. A page function: no closure. */
export function quietBoardOverlay({ now = Date.now(), waiting = false } = {}) {
  const minute = 60_000;
  const hour = 60 * minute;
  const run = "run-desk";
  const sha = (n) => n.toString(16).padStart(7, "0").repeat(6).slice(0, 40);
  const midnight = new Date(now);
  midnight.setHours(0, 0, 0, 0);
  const finishedAt = (ago) => Math.max(midnight.getTime() + minute, now - ago * minute);
  const task = (id, title, stage, extra = {}) => ({
    run, id, title, stage, parent: null, parent_title: null, gate: null, blocked_by: [], closed: null,
    created_ms: now - 300 * minute, cost: null, writing: null, progress: null, ...extra,
  });
  const landed = (ago, took) => ({ times: { started_ms: finishedAt(ago) - took * minute, landed_ms: finishedAt(ago) } });
  const tasks = [
    task("t-88001", "내비 바 고리 맞추기", "merged", landed(44, 70)),
    task("t-88002", "표 편집 길", "merged", landed(520, 60)),
  ];
  window.__LEDGER__ = [];
  const card = (term, heading, state, age, extra = {}) => ({
    pane: `term:${term}`, heading, state, agent: "claude", project: "/repos/zerocode", worktree: "main",
    task: "", task_id: null, ask: "", said: "", you: "", parent: "", ledger: "none",
    at: now - minute, changed_at: now - age * minute, unseen: false, ...extra,
  });
  window.__COLUMNS__ = [
    { bucket: "attention", cards: waiting
      ? [card(310, "배포 준비", "needs-attention", 4, { ask: "이대로 배포해도 될까요?" })] : [] },
    { bucket: "working", cards: [
      card(301, "발행 조율", "working", 9),
      card(302, "시험 결과 정리", "working", 7),
      card(303, "도구 판 확인", "working", 5, { agent: "codex" }),
    ] },
    { bucket: "done", cards: [card(304, "설계 검토", "done", 30)] },
    { bucket: "idle", cards: [] },
  ];
  const stages = ["pending", "ready", "dispatched", "reported", "verified", "merged", "deployed", "gate", "blocked",
    "stale", "failed", "unreviewable", "waived", "nothing_to_land", "unknown", "closed"];
  window.__DESK__ = {
    revision: 31,
    runs: [{ run, name: "발행 조율", seat: true }],
    mail: [], news: [], counts: { mail: 0, news: 0, folded: 0 }, folded_batches: [], land_batches: [],
    tasks,
    stages: stages.map((stage) => ({ stage, count: tasks.filter((one) => one.stage === stage).length })),
  };
  const step = (name, ko, usual, extra) => ({ name, words: { ko }, expected: { secs: usual, samples: 5 }, ...extra });
  const old = now - 51 * hour;
  window.__BUILDS__ = [{
    id: "cargo", name: { ko: "Cargo 빌드", en: "Cargo build" }, tool: "cargo", sha: sha(0x9dc18f3),
    state: "cancelled", started_ms: old, updated_ms: old + 4_000,
    steps: [step("check", "검사", 90, { state: "failed", secs: 4 }), step("test", "시험", 600, { state: "ahead" })],
  }, {
    id: "just", name: { ko: "just 빌드", en: "just build" }, tool: "just", sha: sha(0x9dc18f3),
    state: "cancelled", started_ms: old + minute, updated_ms: old + minute + 20_000,
    steps: [step("lint", "린트", 120, { state: "ahead" }), step("test", "시험", 900, { state: "ahead" })],
  }, {
    id: "release", name: { ko: "새 버전 발행", en: "Publish a new version" }, label: "v1.1.90", sha: sha(0x4ae9809),
    state: "passed", started_ms: now - 260 * minute, updated_ms: now - 120 * minute,
    steps: [step("bundle", "소스 묶기", 30, { state: "done", secs: 21 }),
      step("gate-root", "결합 시험", 3900, { state: "failed", secs: 4069 }),
      step("publish", "발행", 120, { state: "done", secs: 116 })],
  }];
  window.__BUILDS_RUNNABLE__ = [];
  window.__BUILDS_LINE__ = {
    jobs: [{ id: "j-quiet-1", project: "/repos/zerocode/.git", checkout: "/repos/zerocode-wt/checks", owner: "checks",
      what: "착지 머리 전체 시험", cap_secs: 3600, command: ["just", "test"], state: "running",
      queued_ms: now - 4 * minute, started_ms: now - 3 * minute, slot: 0 }],
    places: {},
  };
  window.__ANSWER__.schedule_timeline = () => ({
    revision: 41, unchanged: false, read_ms: now,
    line: { found: true, paused: false, runner_lanes: [1], disk_low: false, quiet_wanted: false },
    jobs: [], builds: window.__BUILDS__,
  });
  for (const listener of window.__LISTENERS__["ledger:changed"] ?? []) listener({ payload: now });
}

/* The quiet scene with the facts the desk reads on its slow beat asked at once, as `draftScene` does. The builds are
 * asked twice: the first ask may have left before the scene was laid over. */
async function quietScene(page, options = {}) {
  await page.evaluate(quietBoardOverlay, options);
  await page.evaluate(async () => {
    await askBoardBuilds();
    await askBoardBuilds();
    refreshBoardSchedule({ fresh: true });
    await paintBoardView();
    await window.__UNTIL__(() => !boardScheduleAsking, "the schedule's answer");
    paintCoordinatorDesk(document.querySelector("#board-view"));
  });
  await fullBoard(page);
}

async function testBoardNowClarity(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await installBoardWaits(page);
    // The person's board pane: 1,240 px beside the sidebar and the right column.
    await page.setViewportSize({ width: 1240, height: 920 });
    await page.evaluate(coordinatorDeskFixture, { tasks: 0, workers: 1, mail: 0 });
    await draftScene(page, { build: "passed" });
    await quietScene(page);
    const readQuiet = () => page.evaluate(() => {
      const view = document.querySelector("#board-view");
      const surface = view.querySelector(".task-board-surface");
      const token = (name) => {
        const probe = document.createElement("i");
        probe.style.color = `var(${name})`;
        document.body.append(probe);
        const said = getComputedStyle(probe).color;
        probe.remove();
        return said;
      };
      const shown = (node) => Boolean(node) && node.checkVisibility();
      const asks = view.querySelector('[data-desk-block="asks"]');
      const running = view.querySelector('[data-desk-block="running"]');
      const builds = view.querySelector('[data-desk-block="builds"]');
      const count = surface.querySelector('[data-board-tab-button="now"] .board-now-tab-count');
      const talkRows = [...running.querySelectorAll(".board-run.is-talk")];
      const bar = (card, name) => {
        const seg = [...card.querySelectorAll(".board-build-seg")].find((one) => one.children[1].textContent === name);
        return seg ? getComputedStyle(seg.querySelector(".board-build-bar")).backgroundColor : null;
      };
      const cards = [...builds.querySelectorAll(".board-build")];
      return {
        title: surface.querySelector(".board-now-title").textContent,
        sub: surface.querySelector(".task-board-summary").textContent,
        tabCount: { shown: shown(count), text: count.dataset.count ?? "", tone: count.dataset.tone },
        tips: [...surface.querySelectorAll(".board-now-tab")].map((tab) => tab.dataset.tip ?? ""),
        asks: {
          clear: asks.classList.contains("is-clear"), height: Math.round(asks.getBoundingClientRect().height),
          dot: getComputedStyle(asks.querySelector(".board-desk-head"), "::before").display,
          foot: asks.querySelector(".board-asks-foot").textContent,
          rows: [...asks.querySelectorAll(".board-asks > li")].map((row) => ({
            kind: row.querySelector(".board-ask-kind")?.textContent, title: row.querySelector(".board-ask-title")?.textContent,
            meta: row.querySelector(".board-ask-meta")?.textContent,
            buttons: [...row.querySelectorAll("button:not([hidden])")].map((one) => one.textContent) })),
        },
        running: {
          hint: running.querySelector(".board-desk-head-note").textContent,
          goals: [...running.querySelectorAll(".board-goal")].map((head) =>
            [head.querySelector(".board-goal-name").textContent, head.querySelector(".board-goal-note")?.textContent ?? ""]),
          rows: talkRows.map((row) => ({
            name: row.querySelector(".board-run-door").textContent, door: row.querySelector(".board-run-door").tagName,
            state: row.querySelector(".board-run-state").textContent, work: row.querySelector(".board-run-work").textContent,
            workWidth: Math.round(row.querySelector(".board-run-work").getBoundingClientRect().width),
            who: row.querySelector(".board-run-who").textContent, tree: row.querySelector(".board-run-branch").textContent,
            end: row.querySelector(".board-run-end").textContent,
            dot: getComputedStyle(row.querySelector(".board-run-state"), "::before").backgroundColor })),
          empty: shown(running.querySelector(".board-runs-empty")) ? running.querySelector(".board-runs-empty").textContent : "",
        },
        builds: {
          order: [...builds.querySelector(".board-desk-body").children].filter(shown).map((node) =>
            node.classList.contains("board-line") ? `line${node.classList.contains("is-live") ? ":live" : ""}`
              : node.classList.contains("board-build-old") ? "old"
                : node.classList.contains("board-build") ? `card:${node.dataset.build}` : "foot"),
          old: builds.querySelector(".board-build-old") ? {
            words: builds.querySelector(".board-build-old-word").textContent,
            note: builds.querySelector(".board-build-old-note").textContent,
            open: builds.querySelector(".board-build-old").getAttribute("aria-expanded") } : null,
          passedFailedStep: cards.filter((card) => card.dataset.build === "release").map((card) => bar(card, "결합 시험"))[0] ?? null,
          cancelledStep: cards.filter((card) => card.dataset.build === "cargo").map((card) => bar(card, "검사"))[0] ?? null,
        },
        tokens: { run: token("--status-dot-run"), halt: token("--status-dot-halt"), wait: token("--signal-wait"),
          mark: token("--edge-mark"), you: token("--status-dot-you") },
        sideways: surface.scrollWidth > surface.clientWidth + 1,
      };
    });
    const quiet = await readQuiet();

    /* ---- 맨 위 문장: 원장 밖에서 도는 것도 센다 ---- */
    ok("with nothing to answer the sentence says so, and the line under it says what runs — three agent conversations and a job on the build line — never 「지금 도는 일이 없습니다」",
      quiet.title === "지금 답할 일이 없습니다" && quiet.sub === "지금 도는 일 — 에이전트 대화 3 · 빌드 줄 1.", JSON.stringify([quiet.title, quiet.sub]));
    ok("each tab says in a tip what it answers, and the Now tab carries no count while it is the chosen tab",
      quiet.tips.length === 4 && quiet.tips.every((tip) => tip.length > 0) && quiet.tabCount.shown === false,
      JSON.stringify({ tips: quiet.tips, count: quiet.tabCount }));

    /* ---- 빈 「당신 차례」는 한 줄이다 ---- */
    ok("your turn with nothing waiting folds to one line — its name beside 「기다리는 결정은 없습니다」, no blue dot, no empty box",
      quiet.asks.clear && quiet.asks.height <= 64 && quiet.asks.dot === "none" && quiet.asks.rows.length === 0
        && quiet.asks.foot === "지금 당신을 기다리는 결정은 없습니다.", JSON.stringify(quiet.asks));

    /* ---- 지금 도는 일: 에이전트 대화의 묶음, 하는 일이 보이는 행 ---- */
    ok("running now stands with the agent conversations as one group, earliest started first, each row a door to its conversation",
      JSON.stringify(quiet.running.goals) === JSON.stringify([["에이전트 대화", "3"]]) && quiet.running.hint === "먼저 시작한 일부터"
        && JSON.stringify(quiet.running.rows.map((row) => row.name)) === JSON.stringify(["발행 조율", "시험 결과 정리", "도구 판 확인"])
        && quiet.running.rows.every((row) => row.door === "BUTTON" && row.state === "도는 중" && row.dot === quiet.tokens.run
          && row.tree === "main" && /^\d+분째$/.test(row.end))
        && quiet.running.rows[2].who === "Codex" && quiet.running.empty === "",
      JSON.stringify(quiet.running));
    ok("on the person's 1,240 px pane a row keeps what the agent is doing readable — the cell is not squeezed to nothing beside the fixed columns",
      quiet.running.rows.every((row) => row.work === "모델의 응답을 기다리고 있어요." && row.workWidth >= 200) && !quiet.sideways,
      JSON.stringify(quiet.running.rows.map((row) => [row.work, row.workWidth])));
    const opened = await page.evaluate(async () => {
      const asked = [];
      const real = openBoardConversation;
      openBoardConversation = async (card) => { asked.push(card.pane); };
      try {
        document.querySelector("#board-view .board-run.is-talk .board-run-door").click();
        await new Promise((done) => setTimeout(done, 0));
      } finally {
        openBoardConversation = real;
      }
      return asked;
    });
    ok("pressing a conversation's name opens that conversation", JSON.stringify(opened) === JSON.stringify(["term:301"]), JSON.stringify(opened));
    // A conversation of another project the window knows stands on that project's board — as the desk and the builds do.
    const scoped = await page.evaluate(async () => {
      const view = document.querySelector("#board-view");
      const working = window.__COLUMNS__.find((column) => column.bucket === "working");
      projects.push({ name: "other", path: "/repos/other", worktrees: [] });
      working.cards.push({ ...working.cards[0], pane: "term:399", heading: "다른 프로젝트의 일", project: "/repos/other" });
      const read = async () => {
        await paintBoardView();
        paintCoordinatorDesk(view);
        return { names: [...view.querySelectorAll('[data-desk-block="running"] .board-run-door')].map((one) => one.textContent),
          sub: view.querySelector(".task-board-summary").textContent };
      };
      try {
        return await read();
      } finally {
        working.cards.pop();
        projects.pop();
        await read();
      }
    });
    ok("a conversation working in another project the window knows is not counted or listed on this project's board",
      scoped.names.length === 3 && !scoped.names.includes("다른 프로젝트의 일") && scoped.sub === "지금 도는 일 — 에이전트 대화 3 · 빌드 줄 1.",
      JSON.stringify(scoped));

    /* ---- 빌드: 도는 줄이 위에, 지난 빌드는 접힌 한 줄 ---- */
    ok("the build line with a job running stands over the cards, and builds that ended more than a day ago fold behind one line that names them",
      JSON.stringify(quiet.builds.order) === JSON.stringify(["line:live", "card:release", "old", "foot"])
        && quiet.builds.old?.words === "지난 빌드 2개" && quiet.builds.old.note.startsWith("Cargo 빌드 · just 빌드 — 마지막 ")
        && quiet.builds.old.open === "false",
      JSON.stringify(quiet.builds));
    ok("a step recorded as failed inside a build that passed is drawn as a caution, not in the red of a failed build",
      quiet.builds.passedFailedStep === quiet.tokens.wait && quiet.builds.passedFailedStep !== quiet.tokens.halt,
      JSON.stringify({ step: quiet.builds.passedFailedStep, tokens: quiet.tokens }));
    await page.click("#board-view .board-build-old");
    await fullBoard(page);
    const unfolded = await readQuiet();
    await page.click("#board-view .board-build-old");
    await fullBoard(page);
    const refolded = await readQuiet();
    ok("pressing the line stands the old builds as cards under it — a cancelled one's stopped step in grey, not red — and pressing again folds them",
      JSON.stringify(unfolded.builds.order) === JSON.stringify(["line:live", "card:release", "old", "card:cargo", "card:just", "foot"])
        && unfolded.builds.old.open === "true" && unfolded.builds.cancelledStep === unfolded.tokens.mark
        && JSON.stringify(refolded.builds.order) === JSON.stringify(["line:live", "card:release", "old", "foot"]),
      JSON.stringify({ unfolded: unfolded.builds, refolded: refolded.builds.order }));

    /* ---- 사람을 기다리는 대화는 당신 차례의 행이다 ---- */
    await quietScene(page, { waiting: true });
    const waiting = await readQuiet();
    ok("a conversation waiting on the person is a row of your turn — its kind, what it asks, its name and how long, and one door — and the sentence counts it",
      waiting.title === "답할 일 1개 — 나머지는 저절로 돕니다" && !waiting.asks.clear && waiting.asks.rows.length === 1
        && waiting.asks.rows[0].kind === "질문" && waiting.asks.rows[0].title === "이대로 배포해도 될까요?"
        && /^배포 준비 · .+ · \d+분 전$/.test(waiting.asks.rows[0].meta)
        && JSON.stringify(waiting.asks.rows[0].buttons) === JSON.stringify(["대화 열기"]),
      JSON.stringify({ title: waiting.title, asks: waiting.asks }));

    /* ---- 워커 탭: 빈 상태의 말, 에이전트 대화의 카드, 이 기계의 머리; 다른 탭에서는 「지금」 곁에 수 ---- */
    await page.evaluate(() => chooseBoardTab(document.querySelector("#board-view"), "workers", false));
    await fullBoard(page);
    const workers = await page.evaluate(() => {
      const view = document.querySelector("#board-view");
      const count = view.querySelector('[data-board-tab-button="now"] .board-now-tab-count');
      const block = (id) => view.querySelector(`.task-board-desk > [data-desk-block="${id}"]`);
      const talks = block("talks");
      // Your turn's solid blue as the page resolves it — the count is held against the token, not a colour written here.
      const probe = document.createElement("i");
      probe.style.color = "var(--signal-you-solid)";
      document.body.append(probe);
      const solid = getComputedStyle(probe).color;
      probe.remove();
      return {
        count: { shown: count.checkVisibility(), text: count.dataset.count ?? "", tone: count.dataset.tone, solid,
          drawn: getComputedStyle(count, "::before").content, tab: count.parentElement.textContent,
          ground: getComputedStyle(count).backgroundColor, name: count.parentElement.getAttribute("aria-label") },
        head: block("workers")?.querySelector(".board-desk-head")?.textContent ?? "",
        note: block("workers")?.querySelector(".board-desk-block-note")?.textContent ?? "",
        talks: talks ? {
          head: talks.querySelector(".board-desk-head").textContent,
          rows: [...talks.querySelectorAll(".board-run")].map((row) => [row.className, row.querySelector(".board-run-door").textContent,
            row.querySelector(".board-run-state").textContent, row.querySelector(".board-run-work").textContent]),
          foot: talks.querySelector(".board-runs-foot").textContent,
        } : null,
        machine: block("machine")?.querySelector(".board-desk-head")?.checkVisibility()
          ? block("machine").querySelector(".board-desk-head").textContent : "",
      };
    });
    ok("away from the Now tab its tab carries the count of what waits on the person, in your-turn's blue, and names it for a screen reader — the tab's own words stay 「지금」",
      workers.count.shown && workers.count.text === "1" && workers.count.drawn === '"1"' && workers.count.tone === "you"
        && workers.count.ground === workers.count.solid && workers.count.tab === "지금" && workers.count.name === "지금, 답할 일 1개",
      JSON.stringify(workers.count));
    ok("the Workers tab with no worker says so and what will stand there, instead of an empty screen",
      workers.head === "지금 소환된 워커가 없습니다" && workers.note.startsWith("코디네이터가 워커를 띄우면 "), JSON.stringify(workers));
    ok("under it the agent conversations stand as rows — the one waiting on the person first, as your turn — with a door to the rest on the History tab, and this machine's card carries its name",
      workers.talks?.head === "에이전트 대화 4" && workers.talks.rows.length === 4
        && workers.talks.rows[0][0] === "board-run is-you is-talk" && workers.talks.rows[0][1] === "배포 준비"
        && workers.talks.rows[0][2] === "당신 차례" && workers.talks.rows[0][3] === "이대로 배포해도 될까요?"
        && workers.talks.rows.slice(1).every((row) => row[0] === "board-run is-run is-talk")
        && workers.talks.foot === "끝난 대화와 쉬는 대화 · 기록에서 보기" && workers.machine === "이 기계",
      JSON.stringify(workers));

    /* ---- 기록 탭: 접힌 「에이전트 대화 N」이 그 안에 무엇이 있는지 말한다 ---- */
    await page.click('#board-view [data-desk-block="talks"] .board-runs-foot .board-link');
    await fullBoard(page);
    const history = await page.evaluate(() => {
      const surface = document.querySelector("#board-view .task-board-surface");
      const fold = surface.querySelector(".task-board-agents-fold");
      return { tab: surface.dataset.boardTab, words: fold.querySelector(".task-board-agents-fold-words").textContent,
        note: fold.dataset.note, shown: getComputedStyle(fold, "::after").content, open: fold.getAttribute("aria-expanded"),
        list: surface.querySelector(".task-board-sections").checkVisibility(),
        sections: surface.querySelector(".task-board-sections").textContent };
    });
    ok("the door goes to the History tab with the agents' list opened, and the fold's line says how many work and how many wait on the person",
      history.tab === "history" && history.words === "에이전트 대화 5" && history.note === "진행 중 3 · 내 확인 필요 1"
        && history.shown.includes("진행 중 3") && history.open === "true" && history.list
        // The fold speaks the opened list's own section names: the same word stands beside the same count inside.
        && history.sections.includes("진행 중") && history.sections.includes("내 확인 필요"),
      JSON.stringify(history));
    await page.evaluate(() => {
      boardNowChoice.agentsOpen = false;
      chooseBoardTab(document.querySelector("#board-view"), "now", false);
    });

    /* ---- 아무것도 돌지 않는 판: 빈 표 대신 한 줄 ---- */
    await page.evaluate(() => {
      window.__COLUMNS__ = [];
      window.__BUILDS__ = [];
      window.__BUILDS_LINE__ = null;
    });
    await page.evaluate(async () => {
      for (const listener of window.__LISTENERS__["ledger:changed"] ?? []) listener({ payload: Date.now() });
      await askBoardBuilds();
      await askBoardBuilds();
      await paintBoardView();
      paintCoordinatorDesk(document.querySelector("#board-view"));
    });
    await fullBoard(page);
    const idle = await page.evaluate(() => {
      const view = document.querySelector("#board-view");
      const running = view.querySelector('[data-desk-block="running"]');
      return {
        sub: view.querySelector(".task-board-summary").textContent,
        stands: running?.checkVisibility() ?? false,
        empty: running?.querySelector(".board-runs-empty")?.checkVisibility() ? running.querySelector(".board-runs-empty").textContent : "",
        table: running?.querySelector(".board-runs")?.checkVisibility() ?? false,
        legend: running?.querySelector(".board-legend")?.checkVisibility() ?? false,
        builds: view.querySelector('[data-desk-block="builds"]')?.checkVisibility() ?? false,
      };
    });
    ok("with nothing running the card says so and what will stand there — no empty table, no legend, no build card",
      idle.sub === "지금 도는 일이 없습니다." && idle.stands && idle.empty.startsWith("지금 도는 일이 없습니다 · ")
        && !idle.table && !idle.legend && !idle.builds, JSON.stringify(idle));
    ok("the quiet board raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* ---- 1분 박자: 원장이 움직이지 않아도 시간 글자가 간다, 바뀐 글자만, 숨으면 0 ----
 *
 * 사람 10-22: 「프로그레스도 멈춰 있음」. 가짜 시계로 1분을 보내면 도는 행의 「N분째」, 도는 빌드 단계의
 * 「N분째」, 질문의 「N분 전」이 하나씩 오른다 — 원장 이벤트(`ledger:changed`)는 하나도 없이. 그 1분에
 * 판에 생긴 변경은 그 글자들과 도는 칸이 찬 만큼뿐이다. 도는 빌드는 5초마다 다시 묻지만 움직이지 않은
 * 기록은 한 마디로 답해 아무것도 쓰지 않고, 창이 숨은 3분 동안은 묻지도 쓰지도 않는다. */
async function testBoardNowMinute(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin, {
    // A fixed local noon, not the real time: the fake clock runs on 2 + 2 + 3 minutes, and a run near
    // midnight crossed into the next day and rewrote the date words too (v1.1.121 gate at 00:00).
    before: async (one) => { await one.clock.install({ time: new Date("2026-10-15T12:00:00") }); },
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
        step: pick(".board-build-seg.is-running .board-build-seg-time"),
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
    const buildAsks = () => page.evaluate(() => window.__BUILDS_ASKED__.length);
    const asksBefore = await buildAsks();
    // The beat is one minute; its phase is the board's opening, so a word can wait a beat to turn.
    await page.clock.runFor(2 * 60_000);
    const after = await minuteWords();
    const writes = await page.evaluate(() => window.__MINUTE_WRITES__.splice(0));
    const asksRunning = (await buildAsks()) - asksBefore;
    const moved = minutes(after).map((count, at) => count - minutes(before)[at]);
    const age = (words) => Number.parseInt(words.asked[0]?.split(" · ").at(-1) ?? "", 10);
    ok("within two minutes, with no ledger event, every running minute moved on: the rows, the build step, the question's age",
      before.rows.length === 2 && before.step.length === 1 && moved.length === 3 && moved.every((step) => step >= 1 && step <= 2)
        && age(after) - age(before) >= 1,
      JSON.stringify({ before, after, moved, asked }));
    ok("those minutes write only the words that changed — the time cells, the build's minutes and how far its running step is filled, the ages",
      writes.length > 0 && writes.length <= 16 && writes.every((one) => one === "attributes:board-build-seg is-running"
        || (one.startsWith("childList:")
        // the time cells, the running step's minutes, and the rows' ages
        && ["board-run-end", "board-build-seg-time", "", "board-desk-letter-who", "board-desk-letter-age",
          "board-ask-meta"].some((name) => one.endsWith(`:${name}`)))),
      JSON.stringify(writes));
    ok("while a build runs on the card it is asked every five seconds, and a record that did not move writes nothing",
      asksRunning >= 20 && asksRunning <= 26, JSON.stringify({ asksRunning, writes }));
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
    // The doing-now cell of a resting worker says when it was last heard (11-08 draft v3): its age moves with the minute too.
    ok("on the Workers tab, within two minutes and with no ledger event, every elapsed cell moved on — and only those words, the last word's age and a resting worker's 「마지막 활동 · N분 전」 were written",
      tookBefore.length === 7 && tookAfter.length === 7 && tookAfter.every((word, at) => word !== tookBefore[at]) &&
      tookWrites.length > 0 && tookWrites.every((one) => one.startsWith("childList:") &&
        ["board-desk-worker-took", "board-desk-worker-age", "board-desk-worker-work"].some((name) => one.includes(name))),
      JSON.stringify({ tookBefore, tookAfter, tookWrites }));
    await page.evaluate(() => chooseBoardTab(document.querySelector("#board-view"), "now", false));
    await page.clock.runFor(2_000);
    await page.evaluate(() => window.__MINUTE_WRITES__.splice(0));
    await page.evaluate(() => {
      Object.defineProperty(document, "visibilityState", { configurable: true, get: () => "hidden" });
      Object.defineProperty(document, "hidden", { configurable: true, get: () => true });
      document.dispatchEvent(new Event("visibilitychange"));
    });
    const asksShown = await buildAsks();
    await page.clock.runFor(3 * 60_000);
    const hidden = await page.evaluate(() => window.__MINUTE_WRITES__.splice(0));
    const asksHidden = (await buildAsks()) - asksShown;
    ok("a hidden window's board writes nothing for three minutes, and asks for no build", hidden.length === 0 && asksHidden === 0,
      JSON.stringify({ hidden, asksHidden }));
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
