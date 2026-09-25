/* 실시간 조율 지도의 무게 (t-7288).
 *
 * 재는 자는 코디네이터 데스크의 것과 **같은 자**다(`coordinator-desk-perf.mjs`의
 * `installPerfHands`·`median`·`quantile`을 그대로 빌려 온다) — 두 표면을 서로
 * 다른 자로 재면 나란히 놓을 수 없고, 이 저장소의 예산 문법은 밀리초 문턱이
 * 아니라 **같은 조건에서의 전/후**다.
 *
 * 재는 판은 **셋**이다: 기준 커밋(1733234a)의 제품, 이 트리에서 지도를 끈
 * 제품, 이 트리에서 지도를 켠 제품. 앞의 둘이 따로인 것은 손잡이 OFF 가 곧
 * 변경 전이 **아니기** 때문이다 — OFF/ON 은 기능의 값을 말하고, 기준 대 OFF 는
 * 지도를 쓰지 않는 사람이 치르게 된 값을 말한다. 둘은 다른 질문이다.
 *
 * 기준 판은 `git archive 1733234a ui | tar -x` 로 푼 사본 **안에서** 같은 파일로
 * 잰다(코디네이터 데스크의 실측이 쓰는 그 방법 그대로). 그 사본에는 이 표면이
 * 없으므로 이 파일은 **없을 때도 돈다** — 없는 손잡이는 건너뛰고 나머지를 같은
 * 조건으로 잰다.
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

/* 지금 돌고 있는 애니메이션 — **이름까지** 센다.
 *
 * 수 하나로는 답이 되지 않는다: 이 판에는 이 변경 이전부터 도는 것이 있고
 * (작업 중인 카드의 상태 표시), 「유휴에서 1」이 그 오래된 하나인지 이 표면의
 * 맥박인지 가리지 못하면 0도 1도 아무 말을 하지 않는다. 지도의 몫은 이름이
 * `zc-live-` 로 시작하는 것들이고, 그 수가 유휴·숨김에서 0이어야 한다. */
const runningAnimations = (page) => page.evaluate(() => {
  const running = document.querySelector("#board-view")
    .getAnimations({ subtree: true })
    .filter((one) => one.playState === "running");
  const names = running.map((one) => one.animationName ?? one.transitionProperty ?? "?");
  return {
    total: running.length,
    live: names.filter((name) => String(name).startsWith("zc-live-")).length,
    names: [...new Set(names)].sort(),
  };
});

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
    if (typeof agentGraphLiveHandles !== "function") return null;
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
    /* 기준 제품에는 손잡이가 없다. 없으면 없는 대로 관계 그림만 잰다 —
     * 그것이 바로 기준이 답해야 할 질문이다. */
    if (typeof setAgentGraphLive === "function") setAgentGraphLive(view, on);
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

  /* 모양은 **첫 그림 직후**에 센다. 폴을 다 돌린 뒤에 세면 그 수에는 고른 것과
   * 인스펙터가 마지막으로 그린 것이 섞이고, 세 판이 서로 다른 자리에서 세어져
   * 뺄셈이 아무 뜻도 갖지 못한다. */
  const shape = await page.evaluate(() => {
    const view = document.querySelector("#board-view");
    const model = agentGraphModels.get(view);
    const mapped = typeof agentGraphLiveOn === "function";
    return {
      mapPresent: mapped,
      mapOn: mapped ? agentGraphLiveOn() : false,
      elements: view.querySelectorAll("*").length,
      nodes: view.querySelectorAll(".agent-graph-node").length,
      edges: view.querySelectorAll(".agent-graph-edges [data-graph-edge]").length,
      waitCells: view.querySelectorAll(".agent-graph-wait").length,
      liveEdges: (model?.liveEdges ?? []).length,
      events: mapped ? agentGraphLiveRecentEvents().length : 0,
      coverage: mapped ? agentGraphLiveCoverage() : null,
    };
  });

  const quiet = [];
  const moved = [];
  for (let at = 0; at < polls; at += 1) {
    quiet.push(await onePoll(page, false, at));
    moved.push(await onePoll(page, true, at));
  }

  const burst = await measureBurst(page, { tag: live ? "on" : "off" });
  const animations = await runningAnimations(page);

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
    burst,
    animations,
  };
}

/* 한 판에 사건이 쏟아질 때. 맥박의 상한은 **애니메이션만** 접어야 하고,
 * 사건 목록과 간선의 실제 수, 그리고 인스펙터로 가는 길은 그대로여야 한다 —
 * 조밀한 판에서 숨겨지는 것은 움직임뿐이다. */
async function measureBurst(page, { events = 24, tag = "x" } = {}) {
  return page.evaluate(async ({ count, tag }) => {
    if (typeof agentGraphLiveOn !== "function") return null;
    const now = window.__LIVE_NOW__;
    /* 두 상태(OFF·ON)가 **같은 판**에서 잇달아 재어지므로 장부는 앞선 상태의
     * 사건을 이미 안다. id 와 시각이 같으면 둘째 상태의 쏟아짐은 통째로
     * 「이미 본 것」이 되고, 그때 0으로 적히는 맥박 수는 상한이 지켜졌다는
     * 뜻이 아니라 아무 사건도 없었다는 뜻이다. 상태마다 이름을 달리 준다. */
    const seed = tag === "on" ? 700_000 : 500_000;
    const mail = [];
    /* 끝점 쌍은 **서로 달라야** 한다. 백엔드는 쌍마다 한 줄만 싣는데
     * (`BTreeMap<(from,to), GraphOverlayEdge>`), 같은 쌍을 여러 줄로 넣으면
     * 이 창에 올 수 없는 판을 지어내는 것이고 같은 키의 간선이 여러 개 그려져
     * 재는 수가 전부 틀어진다 — 맥박 여섯이 스물하나로 보였다. */
    const seats = ["term:301", "term:302", "term:303", "term:304", "term:306",
      "term:307", "term:311", "term:312"];
    const pairs = [];
    for (const from of seats) for (const to of seats) if (from !== to) pairs.push([from, to]);
    for (let at = 0; at < count; at += 1) {
      const [from, to] = pairs[at % pairs.length];
      mail.push({ from, to, count: at + 1, unread: at % 2, at: now + seed + at,
        verb: "mail",
        last_message: { id: `m-burst-${tag}-${at}`, run: "run-1", from: `worker:w-${at}`,
          to: `worker:w-${at + 1}`, kind: "status", created_ms: now + seed + at } });
    }
    window.__OVERLAYS__ = window.__LIVE_OVERLAYS__(now, { mail });
    const from = performance.now();
    window.__LAYOUT_READS__ = 0;
    await paintBoardView(undefined, { force: true });
    await window.__BOARD_SETTLED__();
    const paintMs = performance.now() - from;
    const view = document.querySelector("#board-view");
    /* 맥박은 **여기서** 센다. 아래의 느린 안정화(`__PERF_SETTLED__`)는 바쁜
     * 기계에서 맥박의 수명을 넘겨 버리고, 그러면 표에 0이 적히는데 그 0은
     * 상한이 지켜졌다는 뜻이 아니라 다 꺼진 뒤에 세었다는 뜻이다. */
    const marked = [...view.querySelectorAll("[data-live-beat]")];
    const beats = marked.length;
    const held = typeof agentGraphLiveHandles === "function" ? agentGraphLiveHandles().pulses : null;
    await window.__PERF_SETTLED__();
    /* 그리고 쏟아지는 중에도 인스펙터로 가는 길이 남아 있는가 — 접히는 것은
     * 움직임뿐이라는 약속의 나머지 절반이다. */
    selectAgentGraphEntity(view, "agent:term:302");
    const inspector = view.querySelector(".agent-inspector-pane.is-relations");
    return {
      events: count,
      paintMs: Number(paintMs.toFixed(2)),
      layoutReadsInFrame: window.__LAYOUT_READS__,
      /* 동시에 뛰는 맥박 — 토큰의 상한을 넘지 않아야 한다. */
      beats, pulsesHeld: held,
      burstToken: agentGraphTuning(view)?.liveBurst ?? null,
      /* 접힌 것은 움직임뿐: 사건 줄과 간선은 그대로 선다. */
      listed: view.querySelectorAll(".agent-live-event").length,
      edges: view.querySelectorAll(".agent-graph-edges [data-graph-edge]").length,
      inspectorReachable: inspector !== null && inspector.hidden === false
        && inspector.textContent.trim().length > 0,
    };
  }, { count: events, tag });
}

export async function measureBoardLive(page, { polls = 30 } = {}) {
  await installPerfHands(page);
  await installLayoutCounter(page);
  await page.evaluate(liveFixture);
  await page.waitForSelector(".task-board-row");

  const mapped = await page.evaluate(() => typeof setAgentGraphLive === "function");
  const off = await measureOneState(page, { live: false, polls });
  const on = mapped ? await measureOneState(page, { live: true, polls }) : null;
  const expiry = mapped ? await measureExpiry(page) : null;

  /* 유휴와 숨김. 맥박이 다 꺼진 뒤에도, 숨긴 판에서도 도는 것이 없어야 한다. */
  await page.evaluate(() => new Promise((done) => setTimeout(done, 1_400)));
  const idleAnimations = await runningAnimations(page);
  const hiddenHandlesOf = () => page.evaluate(() =>
    (typeof agentGraphLiveHandles === "function" ? agentGraphLiveHandles() : null));
  await page.evaluate(async () => {
    Object.defineProperty(document, "hidden", { configurable: true, get: () => true });
    document.dispatchEvent(new Event("visibilitychange"));
    await paintBoardView(undefined, { force: true });
  });
  const hiddenAnimations = await runningAnimations(page);
  const hiddenHandles = await hiddenHandlesOf();
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
    product: mapped ? "this tree" : "baseline (no live map)",
    viewport,
    polls,
    off,
    on,
    /* 지도의 **몫**: 켠 판 − 끈 판. 이 표의 요점이 그 뺄셈이다. */
    /* ON − OFF. 이것은 **기능의 값**이지 변경 전 대비가 아니다 — 그 비교는
     * 기준 판의 같은 표와 나란히 놓아야 나온다(driver 가 셋을 함께 찍는다). */
    onMinusOff: on ? {
      firstPaintMs: Number((on.firstPaintMs - off.firstPaintMs).toFixed(2)),
      quietPaintP50: Number(((on.quiet.paintP50 ?? 0) - (off.quiet.paintP50 ?? 0)).toFixed(3)),
      movedPaintP50: Number(((on.moved.paintP50 ?? 0) - (off.moved.paintP50 ?? 0)).toFixed(3)),
      quietLayoutReadsInFrame: on.quiet.layoutReadsInFrame - off.quiet.layoutReadsInFrame,
      movedLayoutReadsInFrame: on.moved.layoutReadsInFrame - off.moved.layoutReadsInFrame,
      elements: on.shape.elements - off.shape.elements,
      edges: on.shape.edges - off.shape.edges,
    } : null,
    expiry,
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
      onMovedP50: on ? frameBudgetHolds(on.moved.paintP50 ?? 0) : null,
      onMovedP95: on ? frameBudgetHolds(on.moved.paintP95 ?? 0) : null,
      onFirstPaint: on ? frameBudgetHolds(on.firstPaintMs) : null,
    },
  };
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const argOf = (name, fallback) => {
    const at = process.argv.indexOf(`--${name}`);
    return at > 0 && process.argv[at + 1] ? Number(process.argv[at + 1]) : fallback;
  };
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch();
  let measured;
  try {
    const { page } = await openWindowTestPage(browser, origin);
    await page.setViewportSize({ width: 1440, height: 960 });
    measured = await measureBoardLive(page, { polls: argOf("polls", 30) });
    await page.close();
  } finally {
    await browser.close();
    files.close();
  }
  const out = process.argv.indexOf("--json");
  if (out > 0 && process.argv[out + 1]) {
    writeFileSync(process.argv[out + 1], `${JSON.stringify(measured, null, 2)}\n`);
  }
  console.log(JSON.stringify(measured, null, 2));
  /* 판정은 사람이 한다 — 이 파일은 수를 내놓을 뿐 문턱을 지어내지 않는다.
   * 다만 **설계가 0이라고 말한 것들**은 여기서 큰 소리로 말한다: 0이 아니면
   * 그것은 성능이 나쁘다는 뜻이 아니라 설계가 틀렸다는 뜻이다. */
  /* 「0이어야 한다」고 설계가 말한 것들. 애니메이션은 **지도의 몫**을 센다 —
   * 이 판에 이 변경 이전부터 도는 것이 있고, 그것까지 0이라고 주장하는 것은
   * 이 표면이 하지 않은 약속이다. */
  const zeros = measured.on ? {
    quietMutations: measured.on.quiet.mutations,
    quietRowDeltaPx: measured.on.quiet.rowDeltaPx,
    pulseExpiryLayoutReads: measured.expiry.reads,
    burstBeatsOverToken: Math.max(0, measured.on.burst.beats - (measured.on.burst.burstToken ?? 0)),
    idleLiveAnimations: measured.idleAnimations.live,
    hiddenLiveAnimations: measured.hiddenAnimations.live,
    modelCalls: measured.modelCalls,
  } : { baselineModelCalls: measured.modelCalls };
  const broken = Object.entries(zeros).filter(([, value]) => value !== 0);
  const reachable = !measured.on || measured.on.burst.inspectorReachable === true;
  console.log(`\n${broken.length === 0 ? "every declared zero holds" : `NOT ZERO: ${broken.map(([name]) => name).join(", ")}`}`);
  console.log(`  ${JSON.stringify(zeros)}`);
  console.log(`  product: ${measured.product}`);
  if (measured.on) console.log(`  inspector reachable under a ${measured.on.burst.events}-event burst: ${reachable}`);
  console.log(`  animations idle ${JSON.stringify(measured.idleAnimations)} · hidden ${JSON.stringify(measured.hiddenAnimations)}`);
  process.exit(broken.length === 0 && reachable ? 0 : 1);
}
