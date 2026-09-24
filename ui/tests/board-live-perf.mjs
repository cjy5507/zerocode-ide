/* 실시간 조율 지도의 무게 (t-7288).
 *
 * 재는 자는 코디네이터 데스크의 것과 **같은 자**다(`coordinator-desk-perf.mjs`의
 * `installPerfHands`·`median`·`quantile`을 그대로 빌려 온다) — 두 표면을 서로
 * 다른 자로 재면 나란히 놓을 수 없고, 이 저장소의 예산 문법은 밀리초 문턱이
 * 아니라 **같은 조건에서의 전/후**다.
 *
 * 전/후는 손잡이 하나로 갈린다: 같은 픽스처·같은 판·같은 시계에서 지도를 끈
 * 채 한 번, 켠 채 한 번. 기준 커밋을 따로 풀지 않아도 되는 것은 이 변경이
 * **고르는 그림**이기 때문이다 — 끈 판이 곧 이 변경 이전의 판이다.
 *
 * 돌려주는 수:
 *   1. 첫 그리기 ms — 보드를 연 때부터 그림이 선 프레임까지, 강제 레이아웃 포함.
 *   2. 폴당 그리기 ms — 조용한 폴과 **실제 사건이 있는** 폴을 따로, p50·p95.
 *   3. 폴당 mutation — 그림(`.agent-graph-layout`) 위의 기록. 조용한 폴은 0.
 *   4. 조용한 폴의 첫 행 이동 px — 0이어야 한다.
 *   5. rAF 안의 배치 읽기 — 맥박이 **더하는** 몫. 기존 간선 측정이 rAF 안에서
 *      배치를 읽는 것은 이 표면의 오래된 사실이므로, 여기서 세는 것은 지도가
 *      거기에 **얼마나 더 얹는가**이다. 맥박이 꺼지는 길은 rAF가 아니라 시계
 *      하나이므로 그 몫은 0이어야 한다.
 *   6. 유휴·숨김의 움직임 — 맥박이 다 꺼진 뒤와 숨긴 판에서 도는 애니메이션 수.
 *   7. 모델 호출 — 0이어야 한다(`__COUNTS__`의 모든 문을 센다).
 *   8. 요소 수와 간선 수 — 지도가 켜지며 늘어난 몫.
 *   9. 조밀한 판 — 대표 규모에서 인스펙터가 여전히 닿는가.
 *  10. 벽시계 예산 — `machine-load.mjs`: 부하가 코어 수를 넘으면 기록만 한다.
 *
 * 실행 (빌드/시험 슬롯을 받은 뒤에만):
 *   node ui/tests/board-live-perf.mjs
 *   node ui/tests/board-live-perf.mjs --rounds 3 --polls 30 --json out.json
 */
import { writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";
import { installPerfHands, median, quantile } from "./coordinator-desk-perf.mjs";
import { liveFixture } from "./board-live.mjs";
import { FRAME_BUDGET_MS, frameBudgetHolds, loadNote, machineIsLoudNow } from "./machine-load.mjs";

/* rAF 안의 배치 읽기를 세는 손.
 *
 * 세는 이유는 「0이다」라고 말하기 위해서가 아니라 **누구의 0인가**를 가리기
 * 위해서다. 기존 간선 측정은 rAF 안에서 노드의 `offsetLeft`를 읽고(그것이
 * 한 번에 모아 읽는 그 설계다) 그 수는 이 변경 이전에도 같았다. 그러므로
 * 지도의 몫은 **켠 판의 수 − 끈 판의 수**이고, 맥박이 꺼지는 길은 그 몫을
 * 0으로 유지해야 한다. */
async function installLayoutCounter(page) {
  await page.evaluate(() => {
    window.__LAYOUT_READS__ = 0;
    window.__IN_FRAME__ = false;
    const raf = window.requestAnimationFrame.bind(window);
    window.requestAnimationFrame = (run) => raf((stamp) => {
      const held = window.__IN_FRAME__;
      window.__IN_FRAME__ = true;
      try {
        return run(stamp);
      } finally {
        window.__IN_FRAME__ = held;
      }
    });
    const countGetter = (proto, name) => {
      const held = Object.getOwnPropertyDescriptor(proto, name);
      if (!held?.get) return;
      Object.defineProperty(proto, name, {
        ...held,
        get() {
          if (window.__IN_FRAME__) window.__LAYOUT_READS__ += 1;
          return held.get.call(this);
        },
      });
    };
    for (const name of ["offsetLeft", "offsetTop", "offsetWidth", "offsetHeight"]) {
      countGetter(HTMLElement.prototype, name);
    }
    for (const name of ["clientWidth", "clientHeight", "scrollWidth", "scrollHeight"]) {
      countGetter(Element.prototype, name);
    }
    const rect = Element.prototype.getBoundingClientRect;
    Element.prototype.getBoundingClientRect = function counted(...args) {
      if (window.__IN_FRAME__) window.__LAYOUT_READS__ += 1;
      return rect.apply(this, args);
    };
  });
}

/* 지금 돌고 있는 애니메이션 수. 유휴와 숨김에서 0이어야 하는 그 수다. */
const runningAnimations = (page) => page.evaluate(() =>
  document.querySelector("#board-view").getAnimations({ subtree: true })
    .filter((one) => one.playState === "running").length);

/* 한 폴. `moved`면 **실제로 기록된 사건 하나**를 새로 싣는다 — 카드의 낱말을
 * 흔드는 것이 아니라, 원장이 새 메시지 id를 실어 온 판이다. */
async function onePoll(page, moved, index) {
  return page.evaluate(async ({ moved, index }) => {
    if (moved) {
      const now = window.__LIVE_NOW__;
      window.__OVERLAYS__ = window.__LIVE_OVERLAYS__(now, { mail: [
        { from: "term:303", to: "term:302", count: index + 1, unread: 1, at: now + index * 1_000,
          verb: "mail",
          last_message: { id: `m-poll-${index}`, run: "run-1", from: "worker:w-verify",
            to: "worker:w-impl", kind: "status", created_ms: now + index * 1_000 } },
      ] });
    }
    const view = document.querySelector("#board-view");
    const picture = view.querySelector(".agent-graph-layout");
    const firstRow = view.querySelector(".agent-graph-node")?.getBoundingClientRect().top ?? 0;
    const records = [];
    const watch = new MutationObserver((batch) => records.push(...batch));
    watch.observe(picture, { subtree: true, childList: true, attributes: true, characterData: true });
    window.__PERF_PAINT__ = 0;
    window.__LAYOUT_READS__ = 0;
    await paintBoardView(undefined, { force: false });
    await window.__PERF_SETTLED__();
    const paint = window.__PERF_PAINT__;
    const layout = window.__PERF_LAYOUT__();
    const reads = window.__LAYOUT_READS__;
    records.push(...watch.takeRecords());
    watch.disconnect();
    return { paint, layout, reads, mutations: records.length,
      rowDelta: Math.abs((view.querySelector(".agent-graph-node")?.getBoundingClientRect().top ?? 0) - firstRow) };
  }, { moved, index });
}

/* 맥박이 **꺼지는** 길이 배치를 읽는가. 그 길은 rAF가 아니라 시계 하나이고,
 * 하는 일은 `data-live-beat` 하나를 지우는 것뿐이므로 0이어야 한다. */
async function measureExpiry(page) {
  return page.evaluate(async () => {
    const now = window.__LIVE_NOW__;
    window.__OVERLAYS__ = window.__LIVE_OVERLAYS__(now, { mail: [
      { from: "term:306", to: "term:302", count: 1, unread: 1, at: now + 900_000, verb: "mail",
        last_message: { id: "m-expiry", run: "run-1", from: "worker:w-parent",
          to: "worker:w-impl", kind: "status", created_ms: now + 900_000 } },
    ] });
    await paintBoardView(undefined, { force: true });
    await window.__BOARD_SETTLED__();
    const lit = document.querySelectorAll("#board-view [data-live-beat]").length;
    window.__LAYOUT_READS__ = 0;
    const picture = document.querySelector("#board-view .agent-graph-layout");
    const records = [];
    const watch = new MutationObserver((batch) => records.push(...batch));
    watch.observe(picture, { subtree: true, childList: true, attributes: true, characterData: true });
    await new Promise((done) => setTimeout(done, 1_400));
    records.push(...watch.takeRecords());
    watch.disconnect();
    return { lit, reads: window.__LAYOUT_READS__, mutations: records.length,
      dark: document.querySelectorAll("#board-view [data-live-beat]").length,
      handles: agentGraphLiveHandles() };
  });
}

/* 한 판의 두 상태 — 지도를 끈 채, 켠 채. 같은 픽스처·같은 폴 수. */
async function measureOneState(page, { live, polls }) {
  await page.evaluate(async (on) => {
    const view = document.querySelector("#board-view");
    view.querySelector('[data-board-mode="graph"]').click();
    setAgentGraphLive(view, on);
    await paintBoardView(undefined, { force: true });
    await window.__BOARD_SETTLED__();
  }, live);

  const first = await page.evaluate(async () => {
    const view = document.querySelector("#board-view");
    window.__PERF_PAINT__ = 0;
    delete view.dataset.said;
    const from = performance.now();
    await paintBoardView(undefined, { force: true });
    await window.__PERF_SETTLED__();
    return performance.now() - from + window.__PERF_LAYOUT__();
  });

  const quiet = [];
  const moved = [];
  for (let at = 0; at < polls; at += 1) {
    quiet.push(await onePoll(page, false, at));
    moved.push(await onePoll(page, true, at));
  }

  const shape = await page.evaluate(() => {
    const view = document.querySelector("#board-view");
    const model = agentGraphModels.get(view);
    return {
      elements: view.querySelectorAll("*").length,
      nodes: view.querySelectorAll(".agent-graph-node").length,
      edges: view.querySelectorAll(".agent-graph-edges [data-graph-edge]").length,
      liveEdges: (model?.liveEdges ?? []).length,
      events: agentGraphLiveRecentEvents().length,
      coverage: agentGraphLiveCoverage(),
    };
  });

  return {
    firstPaintMs: Number(first.toFixed(2)),
    quiet: {
      paintP50: median(quiet.map((one) => one.paint)),
      paintP95: quantile(quiet.map((one) => one.paint), 0.95),
      mutations: Math.max(...quiet.map((one) => one.mutations)),
      rowDeltaPx: Math.max(...quiet.map((one) => one.rowDelta)),
      layoutReadsInFrame: Math.max(...quiet.map((one) => one.reads)),
    },
    moved: {
      paintP50: median(moved.map((one) => one.paint)),
      paintP95: quantile(moved.map((one) => one.paint), 0.95),
      mutations: median(moved.map((one) => one.mutations)),
      layoutReadsInFrame: median(moved.map((one) => one.reads)),
    },
    shape,
  };
}

/* 한 판에 사건이 쏟아질 때. 맥박의 상한은 **애니메이션만** 접어야 하고,
 * 사건 목록과 간선의 실제 수, 그리고 인스펙터로 가는 길은 그대로여야 한다 —
 * 조밀한 판에서 숨겨지는 것은 움직임뿐이다. */
async function measureBurst(page, { events = 24 } = {}) {
  return page.evaluate(async (count) => {
    const now = window.__LIVE_NOW__;
    const mail = [];
    for (let at = 0; at < count; at += 1) {
      /* 서로 다른 끝점 쌍 — 같은 관계에 스물넷을 얹으면 그것은 한 관계의
       * 같은 시각 상한을 재는 일이지 조밀한 판을 재는 일이 아니다. */
      const from = ["term:301", "term:302", "term:303", "term:304", "term:306", "term:311"][at % 6];
      const to = ["term:302", "term:303", "term:301", "term:311", "term:301", "term:312"][at % 6];
      mail.push({ from, to, count: at + 1, unread: at % 2, at: now + 500_000 + at,
        verb: "mail",
        last_message: { id: `m-burst-poll-${at}`, run: "run-1", from: `worker:w-${at}`,
          to: `worker:w-${at + 1}`, kind: "status", created_ms: now + 500_000 + at } });
    }
    window.__OVERLAYS__ = window.__LIVE_OVERLAYS__(now, { mail });
    const from = performance.now();
    window.__LAYOUT_READS__ = 0;
    await paintBoardView(undefined, { force: true });
    await window.__PERF_SETTLED__();
    const paintMs = performance.now() - from;
    const view = document.querySelector("#board-view");
    selectAgentGraphEntity(view, "agent:term:302");
    const inspector = view.querySelector(".agent-inspector-pane.is-relations");
    return {
      events: count,
      paintMs: Number(paintMs.toFixed(2)),
      layoutReadsInFrame: window.__LAYOUT_READS__,
      /* 동시에 뛰는 맥박 — 토큰의 상한을 넘지 않아야 한다. */
      beats: view.querySelectorAll("[data-live-beat]").length,
      burstToken: agentGraphTuning(view)?.liveBurst ?? null,
      /* 접힌 것은 움직임뿐: 사건 줄과 간선은 그대로 선다. */
      listed: view.querySelectorAll(".agent-live-event").length,
      edges: view.querySelectorAll(".agent-graph-edges [data-graph-edge]").length,
      inspectorReachable: inspector !== null && inspector.hidden === false
        && inspector.textContent.trim().length > 0,
    };
  }, events);
}

export async function measureBoardLive(page, { polls = 30 } = {}) {
  await installPerfHands(page);
  await installLayoutCounter(page);
  await page.evaluate(liveFixture);
  await page.waitForSelector(".task-board-row");

  const off = await measureOneState(page, { live: false, polls });
  const on = await measureOneState(page, { live: true, polls });
  const expiry = await measureExpiry(page);
  const burst = await measureBurst(page);

  /* 유휴와 숨김. 맥박이 다 꺼진 뒤에도, 숨긴 판에서도 도는 것이 없어야 한다. */
  await page.evaluate(() => new Promise((done) => setTimeout(done, 1_400)));
  const idleAnimations = await runningAnimations(page);
  await page.evaluate(async () => {
    Object.defineProperty(document, "hidden", { configurable: true, get: () => true });
    document.dispatchEvent(new Event("visibilitychange"));
    await paintBoardView(undefined, { force: true });
  });
  const hiddenAnimations = await runningAnimations(page);
  const hiddenHandles = await page.evaluate(() => agentGraphLiveHandles());
  await page.evaluate(() => {
    Object.defineProperty(document, "hidden", { configurable: true, get: () => false });
    document.dispatchEvent(new Event("visibilitychange"));
  });

  /* 이 표면이 부른 백엔드 문 전부. 모델을 부르는 문은 하나도 없어야 한다. */
  const doors = await page.evaluate(() => ({ ...window.__COUNTS__ }));
  const modelCalls = Object.keys(doors)
    .filter((name) => /jev|model|infer|complete|prompt/i.test(name))
    .reduce((total, name) => total + doors[name], 0);

  const viewport = await page.evaluate(() => ({
    width: window.innerWidth, height: window.innerHeight, dpr: window.devicePixelRatio,
  }));

  return {
    viewport,
    polls,
    off,
    on,
    /* 지도의 **몫**: 켠 판 − 끈 판. 이 표의 요점이 그 뺄셈이다. */
    delta: {
      firstPaintMs: Number((on.firstPaintMs - off.firstPaintMs).toFixed(2)),
      quietPaintP50: Number(((on.quiet.paintP50 ?? 0) - (off.quiet.paintP50 ?? 0)).toFixed(3)),
      movedPaintP50: Number(((on.moved.paintP50 ?? 0) - (off.moved.paintP50 ?? 0)).toFixed(3)),
      quietLayoutReadsInFrame: on.quiet.layoutReadsInFrame - off.quiet.layoutReadsInFrame,
      movedLayoutReadsInFrame: on.moved.layoutReadsInFrame - off.moved.layoutReadsInFrame,
      elements: on.shape.elements - off.shape.elements,
      edges: on.shape.edges - off.shape.edges,
    },
    expiry,
    burst,
    idleAnimations,
    hiddenAnimations,
    hiddenHandles,
    modelCalls,
    doors,
    load: loadNote(),
    loud: machineIsLoudNow(),
    /* 문턱은 지어내지 않는다 — 이 저장소가 이미 쓰는 프레임 예산 하나
     * (`FRAME_BUDGET_MS`, 여덟 짜리 열두 프레임)로 판정하고, 기계가 시끄러우면
     * 그 판정은 서지 않는다고 그 함수가 말한다. */
    frameBudgetMs: FRAME_BUDGET_MS,
    budget: {
      offMovedP50: frameBudgetHolds(off.moved.paintP50 ?? 0),
      onMovedP50: frameBudgetHolds(on.moved.paintP50 ?? 0),
      onMovedP95: frameBudgetHolds(on.moved.paintP95 ?? 0),
      onFirstPaint: frameBudgetHolds(on.firstPaintMs),
    },
  };
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const argOf = (name, fallback) => {
    const at = process.argv.indexOf(`--${name}`);
    return at > 0 && process.argv[at + 1] ? Number(process.argv[at + 1]) : fallback;
  };
  const server = await createWindowServer();
  const browser = await chromium.launch();
  let measured;
  try {
    const { page } = await openWindowTestPage(browser, server.origin);
    await page.setViewportSize({ width: 1440, height: 960 });
    measured = await measureBoardLive(page, { polls: argOf("polls", 30) });
    await page.close();
  } finally {
    await browser.close();
    await server.close();
  }
  const out = process.argv.indexOf("--json");
  if (out > 0 && process.argv[out + 1]) {
    writeFileSync(process.argv[out + 1], `${JSON.stringify(measured, null, 2)}\n`);
  }
  console.log(JSON.stringify(measured, null, 2));
  /* 판정은 사람이 한다 — 이 파일은 수를 내놓을 뿐 문턱을 지어내지 않는다.
   * 다만 **설계가 0이라고 말한 것들**은 여기서 큰 소리로 말한다: 0이 아니면
   * 그것은 성능이 나쁘다는 뜻이 아니라 설계가 틀렸다는 뜻이다. */
  const zeros = {
    quietMutations: measured.on.quiet.mutations,
    quietRowDeltaPx: measured.on.quiet.rowDeltaPx,
    pulseExpiryLayoutReads: measured.expiry.reads,
    burstBeatsOverToken: Math.max(0, measured.burst.beats - (measured.burst.burstToken ?? 0)),
    idleAnimations: measured.idleAnimations,
    hiddenAnimations: measured.hiddenAnimations,
    modelCalls: measured.modelCalls,
  };
  const broken = Object.entries(zeros).filter(([, value]) => value !== 0);
  const reachable = measured.burst.inspectorReachable === true;
  console.log(`\n${broken.length === 0 ? "every declared zero holds" : `NOT ZERO: ${broken.map(([name]) => name).join(", ")}`}`);
  console.log(`  ${JSON.stringify(zeros)}`);
  console.log(`  inspector reachable under a ${measured.burst.events}-event burst: ${reachable}`);
  process.exit(broken.length === 0 && reachable ? 0 : 1);
}
