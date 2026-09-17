// Runs on the graph harness's existing backend fixture and real SVG renderer.
export async function testKnowledgePerformance(page, ok) {
  const seen = await page.evaluate(async () => {
    knowledgeQuery = "";
    knowledgeTagsPicked.clear();
    knowledgeOrphansOnly = knowledgeGhostsOnly = knowledgeTypedOnly = false;
    knowledgeAliveOnly = knowledgeColdOnly = knowledgeMergeOnly = false;
    knowledgeShowSources = false;
    knowledgeLintLens = knowledgeSelectedKey = knowledgePath = null;
    knowledgeSlicerCutoff = 0;
    const view = document.querySelector(".knowledge-view:not([hidden])");
    const report = (pages, prefix = "") => {
      const answer = window.__buildVaultGraph__({ path: "/performance", sources: false },
        { pages, linksPer: 3, ghosts: 0, tags: ["core", "reading"] });
      for (const node of answer.graph.nodes) node.id = prefix + node.id;
      return answer;
    };
    const paint = async (answer) => {
      knowledgeReport = answer;
      await paintKnowledgeView();
      const layout = knowledgeLayouts.get(view);
      // Settle exactly one step to publish positions, without a wall-clock wait.
      layout.left = 1;
      knowledgeSettle(view);
      await new Promise((done) => requestAnimationFrame(done));
      return knowledgeLayouts.get(view);
    };
    await paint(report(1000));
    const original = new Map([...view.querySelector(".knowledge-nodes").children]
      .map((node) => [node.dataset.graphKey, node]));
    const created = knowledgeNodeCreations;
    let layout = await paint(report(1001));
    const retained = layout.nodeEls.filter((node) => original.get(node.dataset.graphKey) === node).length;
    const additions = knowledgeNodeCreations - created;
    const current = knowledgeReport;
    const oldModel = layout.model;
    knowledgeQuery = "rel:mentions";
    await paintKnowledgeView();
    const searchReusedModel = knowledgeLayouts.get(view).model === oldModel;
    const mentions = knowledgeLayouts.get(view).searchMatch.some((value) => value === 1);
    knowledgeQuery = "rel:constructor";
    await paintKnowledgeView();
    const unknownRelationEmpty = knowledgeLayouts.get(view).searchMatch.every((value) => value === 0);
    knowledgeQuery = "";
    const renamed = structuredClone(current);
    renamed.graph.nodes[0].title = "Renamed while its links stay the same";
    const selected = renamed.graph.nodes[0].id;
    selectKnowledgeNode(view, selected);
    litKnowledge(view, layout, selected);
    await paint(renamed);
    const renamedLabel = view.querySelector(".knowledge-label").textContent;
    const selectionHeld = view.querySelector(".knowledge-node").classList.contains("is-selected")
      && view.querySelector(".knowledge-node").classList.contains("is-lit");
    knowledgeSelectedKey = null;

    // Lens-hidden pages keep their positions; pages removed from the vault do not.
    knowledgeTypedOnly = true;
    await paintKnowledgeView();
    const lensRemembered = knowledgeLayouts.get(view).placed.size;
    const visibleKeys = new Set(knowledgeLayouts.get(view).model.keys);
    const removed = knowledgeReport.graph.nodes.findIndex((node) => !visibleKeys.has(node.id));
    const hiddenGone = structuredClone(knowledgeReport);
    const removedKey = hiddenGone.graph.nodes[removed].id;
    hiddenGone.graph.nodes.splice(removed, 1);
    hiddenGone.graph.edges = hiddenGone.graph.edges
      .filter((edge) => edge.from !== removed && edge.to !== removed)
      .map((edge) => ({ ...edge, from: edge.from > removed ? edge.from - 1 : edge.from,
        to: edge.to > removed ? edge.to - 1 : edge.to }));
    const visibleLayout = knowledgeLayouts.get(view);
    await paint(hiddenGone);
    const hiddenReleased = knowledgeLayouts.get(view) === visibleLayout
      && !visibleLayout.placed.has(removedKey);
    knowledgeTypedOnly = false;
    await paintKnowledgeView();
    for (let generation = 0; generation < 4; generation += 1) {
      layout = await paint(report(40, `generation-${generation}/`));
    }
    const remembered = layout.placed.size;
    const liveKeys = new Set(layout.model.keys);
    const onlyLive = [...layout.placed.keys()].every((key) => liveKeys.has(key));

    const parallel = report(6);
    parallel.graph.edges = [{ from: 0, to: 1, kind: "mentions" }, { from: 0, to: 1, kind: "related" }];
    layout = await paint(parallel);
    paintKnowledgeFrame(view, layout);
    const parallelLines = view.querySelectorAll(".knowledge-edge").length;
    const rewired = structuredClone(parallel);
    rewired.graph.edges[1].to = 2;
    layout = await paint(rewired);
    const rewiredKey = layout.measured[1].key;
    const rewiredCorrectly = rewiredKey.includes(rewired.graph.nodes[2].id);

    const timings = [];
    for (const count of [1000, 5000]) {
      layout = await paint(report(count));
      layout.left = 0;
      paintKnowledgeFrame(view, layout);
      const picture = view.querySelector(".knowledge-picture");
      const canvas = view.querySelector(".knowledge-canvas");
      const beforeBox = picture.getAttribute("viewBox");
      let measurements = 0;
      const originalPainter = paintGraphEdges;
      paintGraphEdges = (...args) => { measurements += 1; return originalPainter(...args); };
      const samples = [];
      try {
        canvas.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, button: 0, clientX: 100, clientY: 100 }));
        for (let at = 0; at < 40; at += 1) {
          const began = performance.now();
          window.dispatchEvent(new PointerEvent("pointermove", { clientX: 110 + at, clientY: 120 + at }));
          samples.push(performance.now() - began);
        }
        window.dispatchEvent(new PointerEvent("pointerup"));
      } finally { paintGraphEdges = originalPainter; }
      samples.sort((a, b) => a - b);
      const node = layout.nodeEls[0];
      const beforeStep = node.getAttribute("transform");
      knowledgeForceStep(layout);
      paintKnowledgeFrame(view, layout, { cameraOnly: true });
      timings.push({ count, edges: layout.model.edgeCount,
        medianMs: samples[Math.floor(samples.length / 2)],
        p95Ms: samples[Math.floor(samples.length * 0.95)], measurements,
        moved: picture.getAttribute("viewBox") !== beforeBox,
        followsForce: node.getAttribute("transform") !== beforeStep });
    }
    /* t-4140 S1: 주변 탐색은 하위 그래프만 DOM에 세운다 — 천 쪽에서 전체·깊이 1·깊이 2의
       점·선 수와 모드 전환의 처리 시간(3회 중앙값), 그리고 돌아온 전체 지도가 같은 SVG
       점을 다시 붙이는지. 시간은 코드 실행의 처리 시간이지 설치 앱의 FPS가 아니다. */
    layout = await paint(report(1000));
    layout.left = 0;
    paintKnowledgeFrame(view, layout);
    const centre = layout.model.keys[0];
    const whole = { nodes: view.querySelectorAll(".knowledge-node").length,
      edges: view.querySelectorAll(".knowledge-edge").length };
    const wholeEls = [...view.querySelector(".knowledge-nodes").children];
    selectKnowledgeNode(view, centre);
    const median = (rows) => rows.slice().sort((a, b) => a - b)[Math.floor(rows.length / 2)];
    const frame = () => new Promise((done) => requestAnimationFrame(() => done(performance.now())));
    /* 한 전환의 값 둘: 그리는 호출의 처리 시간과, 그 뒤 첫 프레임까지의 시간(브라우저의
       스타일·레이아웃이 그 안에 든다). 전환 사이에 프레임 하나를 둔다 — 두 전환을 붙여
       재면 앞 전환의 미룬 레이아웃이 뒤 전환의 첫 읽기에 얹혀 남의 값이 된다. */
    const switchTo = async (mode, depth) => {
      await frame();
      knowledgeFocusDepth = depth;
      setKnowledgeMode(view, mode, { centre, paint: false });
      const began = performance.now();
      await paintKnowledgeView();
      const painted = performance.now();
      const shown = await frame();
      return { paintMs: painted - began, frameMs: shown - painted };
    };
    const explore = {};
    for (const depth of [1, 2]) {
      const paints = [];
      const frames = [];
      for (let round = 0; round < 3; round += 1) {
        await switchTo("global", depth);
        const took = await switchTo("local", depth);
        paints.push(took.paintMs);
        frames.push(took.frameMs);
      }
      explore[`depth${depth}`] = { nodes: view.querySelectorAll(".knowledge-node").length,
        edges: view.querySelectorAll(".knowledge-edge").length,
        switchMs: median(paints), frameMs: median(frames),
        hidden: view.querySelectorAll(".knowledge-picture.is-focused .knowledge-node:not(.is-focus-lit)").length };
    }
    const backPaints = [];
    const backFrames = [];
    for (let round = 0; round < 3; round += 1) {
      await switchTo("local", 1);
      const took = await switchTo("global", 1);
      backPaints.push(took.paintMs);
      backFrames.push(took.frameMs);
    }
    knowledgeFocusDepth = 1;
    const restored = { nodes: view.querySelectorAll(".knowledge-node").length,
      edges: view.querySelectorAll(".knowledge-edge").length,
      switchMs: median(backPaints), frameMs: median(backFrames),
      retained: [...view.querySelector(".knowledge-nodes").children].every((node, at) => node === wholeEls[at]),
      mode: knowledgeMode };
    const neighbourhood = knowledgeNeighbours(layout, 0).nodes.size;
    selectKnowledgeNode(view, null);
    return { retained, additions, searchReusedModel, mentions, unknownRelationEmpty, renamedLabel, selectionHeld,
      lensRemembered, hiddenReleased, remembered, onlyLive, parallelLines, rewiredCorrectly, timings,
      explore: { whole, ...explore, restored, neighbourhood } };
  });
  ok("explore draws only the neighbourhood's nodes and edges, and the whole map comes back with the same SVG nodes",
    seen.explore.depth1.nodes === seen.explore.neighbourhood
      && seen.explore.depth1.nodes < seen.explore.whole.nodes
      && seen.explore.depth1.edges < seen.explore.whole.edges
      && seen.explore.depth2.nodes > seen.explore.depth1.nodes
      && seen.explore.depth1.hidden === 0
      && seen.explore.restored.nodes === seen.explore.whole.nodes
      && seen.explore.restored.edges === seen.explore.whole.edges
      && seen.explore.restored.retained
      && seen.explore.restored.mode === "global",
    JSON.stringify(seen.explore));
  console.log(`METRIC knowledge explore: ${JSON.stringify(seen.explore)}`);
  ok("one new page reuses a thousand existing SVG nodes and refreshes an unchanged topology's title",
    seen.retained === 1000 && seen.additions === 1 && seen.renamedLabel.startsWith("Renamed")
      && seen.selectionHeld, JSON.stringify(seen));
  ok("search reuses the graph model while rel:mentions still matches",
    seen.searchReusedModel && seen.mentions && seen.unknownRelationEmpty, JSON.stringify(seen));
  ok("positions survive a lens but deleted pages leave the remembered layout",
    seen.lensRemembered === 1001 && seen.hiddenReleased && seen.remembered === 40 && seen.onlyLive, JSON.stringify(seen));
  ok("panning a settled graph moves its camera without traversing its edges",
    seen.timings.every((row) => row.moved && row.measurements === 0 && row.followsForce), JSON.stringify(seen.timings));
  ok("parallel relation kinds keep distinct SVG lines and a same-count rewire follows its new endpoint",
    seen.parallelLines === 2 && seen.rewiredCorrectly, JSON.stringify(seen));
  console.log(`METRIC knowledge incremental/retained: ${JSON.stringify(seen)}`);
}

/* ---- 재는 자 (6차 P0) ------------------------------------------------------
 *
 * 5차는 실제 볼트를 실제 창에서 재고 그 값을 손으로 `measurements.json`에 적었다.
 * 그래서 「이름표가 겹치는가」·「점이 제 원반 밖으로 나갔는가」는 사람이 한 번 본
 * 답이지 다음 사람이 다시 물을 수 있는 답이 아니었다. 6차의 첫 걸음은 그 물음들을
 * **시험이 묻는 것**으로 옮기는 일이다: 아래 한 함수가 네 시나리오에 같은 여덟 개의
 * 수를 내고, 그 수는 페인터가 SVG이든 GL이든 같은 뜻이다.
 *
 * 재는 규약(설계 §10.2): 한 번의 값은 적지 않는다. 같은 세션에서 전(직전 커밋)과
 * 후를 **번갈아** 세 번 이상 돌리고 중앙값만 적으며 첫 캡처는 버린다 — 그 운전은
 * `knowledge-ab.mjs`가 한다. 여기서는 한 판의 값만 정직하게 낸다.
 *
 * 「드로우 수」는 두 페인터가 같은 뜻으로 읽는다: **한 프레임에 그리는 쪽에 넘기는
 * 일감의 수**. SVG에서는 세 층이 든 요소의 수(브라우저가 래스터화하는 단위)이고,
 * GL에서는 인스턴스 드로우 호출의 수다. 그래서 만 쪽의 3만 대 다섯이 한 표의 두
 * 줄에 선다. */

/* 시나리오 넷. 선은 쪽수의 2.5배가 되도록 `linksPer: 3`이다(외로운 쪽 5분의 1과
 * 일곱 쪽마다의 타입 관계까지 세면 실측 2.51~2.54배). 실제 볼트의 줄은 그 볼트의
 * 모양(359쪽·유령 53)이고, 진짜 볼트의 수는 실제 창에서 따로 잰다 — 백엔드의
 * 스캔은 이 하네스가 부를 수 있는 문이 아니다. */
export const KNOWLEDGE_SCENES = Object.freeze([
  Object.freeze({ name: "vault-shaped", pages: 359, ghosts: 53, linksPer: 3 }),
  Object.freeze({ name: "k1", pages: 1000, ghosts: 20, linksPer: 3 }),
  Object.freeze({ name: "k5", pages: 5000, ghosts: 100, linksPer: 3 }),
  Object.freeze({ name: "k10", pages: 10000, ghosts: 200, linksPer: 3 }),
]);

/* 재는 방법의 수들 — 그림의 수가 아니라 계측의 수다(그림의 수는 tokens.css에 산다).
 * 카메라 120프레임은 설계 §2의 잣대 그대로이고, 열두 프레임에 한 번의 돌리는
 * 「카메라만 바뀐 프레임」과 「기하가 바뀐 프레임」을 한 표본에 섞기 위한 것이다:
 * 팬만 120번 재면 페인터가 일찍 돌아서는 길만 재게 된다. */
/* 게이트(`just knowledge-browser-test`)가 치르는 몫 (검증 09-16).
 *
 * 겹침·원반 수는 훑기가 끝나 판을 제자리로 되돌린 **뒤에** 세므로, 계약은 훑기의
 * 프레임 수에 기대지 않는다 — 게이트는 카메라 길을 한 번 지나가는 것으로 족하고,
 * 프레임 시간의 표는 `knowledge-gpu.mjs`가 진짜 GPU에서 `SWEEP.frames`로 잰다.
 * GL 판은 소프트웨어 래스터라이저라 그 시간은 코드의 수가 아니고, 크기에 따라
 * 달라지는 배치의 계약(만 쪽까지)은 SVG 판이 같은 격자로 이미 묻는다 — GL 판이
 * 묻는 것은 크기와 무관한 손의 계약(드로우 수·오버레이·겹침)이라 작은 두 장면으로
 * 족하다. 실측(워커 로그): 게이트의 장면 측정이 SVG 100.5 s + GL 75.6 s였고, 그중
 * GL의 오천·만 쪽이 63.6 s, 훑기 프레임이 약 19 s였다. */
export const KNOWLEDGE_GATE = Object.freeze({
  frames: 2,
  glScenes: Object.freeze(["vault-shaped", "k1"]),
});

/* 전·후를 재는 운전자(`knowledge-ab.mjs`)도 이 하네스를 돌리지만 읽는 것은 **시간의
 * 표**다 — 그 판은 환경의 `KNOWLEDGE_SWEEP=full`로 게이트의 몫 대신 전체 훑기(모든
 * 장면, `SWEEP.frames`)를 청한다. 게이트는 환경을 건드리지 않는다. */
export function knowledgeSweepFor(env) {
  return env.KNOWLEDGE_SWEEP === "full"
    ? { frames: SWEEP.frames, glScenes: KNOWLEDGE_SCENES.map((scene) => scene.name) }
    : KNOWLEDGE_GATE;
}

const SWEEP = Object.freeze({
  /* 5차의 표는 1998×1069에서 잰 것이다 — 같은 판에서 재야 두 표가 같은 줄을 읽는다. */
  wide: 1998,
  tall: 1069,
  frames: 120,
  panX: 7,
  panY: 3,
  dollyEvery: 12,
  dollyStep: 1.02,
  settleFrames: 1200,
  /* 겹침을 셀 때의 여유(px). 상자가 꼭짓점 하나로 스치는 것은 겹침이 아니다. */
  overlapSlack: 0.5,
  /* 제 원반 밖인가 — 부동소수의 같은 값이 둘로 갈리지 않을 만큼의 상대 여유. */
  discSlack: 1.0001,
});

/* 네 시나리오를 한 판에서 잰다. 돌려주는 것은 시나리오마다의 한 줄이고, 그 줄의
 * 이름은 설계 §2의 표 그대로다. */
export async function measureKnowledgeScenes(page, ok,
  { painter = "svg", frames = SWEEP.frames, scenes: list = KNOWLEDGE_SCENES } = {}) {
  const seatWas = page.viewportSize();
  await page.setViewportSize({ width: SWEEP.wide, height: SWEEP.tall });
  const scenes = await page.evaluate(async ({ list, sweep, hand }) => {
    /* 재는 손을 못박고, 끝에서 못박기 전의 값(`null`이면 창이 고르는 손)으로 되돌린다. */
    const pinned = knowledgePainterKind;
    knowledgePainterKind = hand;
    /* 앞 케이스가 남긴 렌즈·검색·고른 점은 첫 그림의 값을 바꾼다. */
    knowledgeQuery = "";
    knowledgeTagsPicked.clear();
    knowledgeOrphansOnly = knowledgeGhostsOnly = knowledgeTypedOnly = false;
    knowledgeAliveOnly = knowledgeColdOnly = knowledgeMergeOnly = false;
    knowledgeShowSources = false;
    knowledgeLintLens = knowledgeSelectedKey = knowledgePath = null;
    knowledgeSlicerCutoff = 0;
    knowledgeClusterPicked = -1;
    const view = document.querySelector(".knowledge-view:not([hidden])");
    /* 전체 지도에서 잰다. 모드를 세우는 것만으로는 모자라다 — 처음 열린 판은
     * 진입 규칙(S2)이 「마지막으로 보던 곳」을 다시 세우려고 기다리고 있어서,
     * 그 규칙이 다음 그림에서 주변 탐색으로 되돌린다(실측: WebKit에서 만 쪽
     * 시나리오가 점 열한 개의 이웃을 재고 있었다 — 드로우 346, 이름표 11). */
    knowledgeEntryPending = false;
    knowledgeRevealKey = null;
    knowledgeRevealMode = null;
    knowledgeMode = "global";
    setKnowledgeMode(view, "global", { paint: false });
    const frame = () => new Promise((done) => requestAnimationFrame(done));
    const median = (rows) => rows.slice().sort((a, b) => a - b)[Math.floor(rows.length / 2)];
    const round2 = (value) => Math.round(value * 100) / 100;

    /* 서 있는 이름표의 상자들 — 페인터가 무엇이든 화면에 선 글자를 묻는다. SVG는
     * 점의 <text>와 주제의 이름판이고, GL은 오버레이의 <div>다. 상자는 DOM에게
     * 묻는다: 격자가 어림한 폭이 아니라 **실제로 그려진 글자**가 겹치는지가 답이다. */
    const labelBoxes = () => window.__knowledgeLabelBoxes__(view);
    const overlapPairs = (boxes) => window.__knowledgeOverlapPairs__(boxes, sweep.overlapSlack);
    /* 원반끼리 겹치는 쌍 — 그린 원반(`clusterReach`)으로. 배치가 약속한 반지름이
     * 아니라 그림에 실제로 선 반지름이 「어디까지가 이 주제」를 말한다. */
    const discOverlaps = (layout) => {
      let pairs = 0;
      for (let left = 0; left < layout.namedCount; left += 1) {
        if (layout.clusterTally[left] === 0) continue;
        for (let right = left + 1; right < layout.namedCount; right += 1) {
          if (layout.clusterTally[right] === 0) continue;
          const gap = Math.hypot(layout.clusterX[left] - layout.clusterX[right],
            layout.clusterY[left] - layout.clusterY[right]);
          if (gap < (layout.clusterReach[left] + layout.clusterReach[right]) / sweep.discSlack) {
            pairs += 1;
          }
        }
      }
      return pairs;
    };
    /* 제 원반 밖의 점 — 훑기의 제약이 모든 점을 홈에서 `homeR` 안에 붙든다는
     * 약속을 그림에게 되묻는다. */
    const outsideDisc = (layout) => {
      let strays = 0;
      for (let at = 0; at < layout.count; at += 1) {
        if (layout.drawn !== null && layout.drawn[at] === 0) continue;
        const rank = layout.community[at];
        if (rank >= layout.namedCount || !layout.communityHomed[rank]) continue;
        const gap = Math.hypot(layout.x[at] - layout.communityHomeX[rank],
          layout.y[at] - layout.communityHomeY[rank]);
        if (gap > layout.communityHomeR[rank] * sweep.discSlack) strays += 1;
      }
      return strays;
    };
    /* 한 프레임의 일감 수. 페인터가 제 수를 적어 두면 그것이 답이고(P1 이후),
     * 없으면 세 층이 든 요소를 센다 — 같은 뜻의 두 셈이다. */
    /* 한 프레임의 일감 수는 손이 달라도 뜻이 같다: 그리는 쪽에 넘기는 일감의 수.
     * SVG에서는 세 층이 든 요소(브라우저가 래스터화하는 단위)이고, GL에서는
     * 인스턴스 드로우 호출이다. */
    const drawCount = (layout) => (knowledgePainterFor(view).id === "svg"
      ? view.querySelectorAll(".knowledge-clusters *, .knowledge-edges *, .knowledge-nodes *").length
      : layout.paintStats.draws);
    const heapMb = () => (performance.memory
      ? Math.round((performance.memory.usedJSHeapSize / (1024 * 1024)) * 10) / 10
      : null);

    const rows = [];
    for (const scene of list) {
      const answer = window.__buildVaultGraph__(
        { path: `/scene/${scene.name}`, sources: false },
        { pages: scene.pages, ghosts: scene.ghosts, linksPer: scene.linksPer,
          tags: ["core", "reading", "tools"] },
      );
      /* 앞 시나리오가 남긴 자리 기억과 DOM은 다음 시나리오의 첫 그림을 싸게 만든다 —
       * 볼트 이름을 바꾸고 판을 버려 언제나 「처음 여는 볼트」를 잰다. */
      knowledgeLayouts.delete(view);
      const host = view.querySelector(".knowledge-nodes");
      host.dataset.knowledgeSignature = "";
      host.dataset.knowledgeVault = "";
      host.replaceChildren();
      view.querySelector(".knowledge-edges").replaceChildren();
      knowledgeReport = answer;
      const began = performance.now();
      await paintKnowledgeView();
      await frame();
      const firstPaintMs = performance.now() - began;
      let waited = 0;
      while (waited < sweep.settleFrames && (knowledgeLayouts.get(view)?.left ?? 0) > 0) {
        await frame();
        waited += 1;
      }
      const layout = knowledgeLayouts.get(view);
      const zoomWas = layout.zoom;
      /* 한 프레임의 값 둘. `samples`는 페인터가 쓴 처리 시간이고, `gaps`는 rAF와 rAF
       * 사이 — 브라우저의 스타일·레이아웃·래스터화가 그 안에 든다. 만 쪽의 SVG에서
       * 둘은 열 배 넘게 갈린다(요소 9만 개를 굽는 시간은 우리 호출 밖에 있다).
       * 예산(설계 §2의 p95 < 16 ms)이 묻는 것은 사람이 보는 **프레임**이므로 표의
       * 판정은 `frameP95Ms`가 지고, 처리 시간은 그 옆에 선다. */
      const samples = [];
      const gaps = [];
      let last = await frame();
      for (let at = 0; at < sweep.frames; at += 1) {
        const tick = performance.now();
        layout.panX += sweep.panX;
        layout.panY += sweep.panY;
        if (at % sweep.dollyEvery === sweep.dollyEvery - 1) layout.zoom *= sweep.dollyStep;
        paintKnowledgeFrame(view, layout, { cameraOnly: true });
        samples.push(performance.now() - tick);
        const now = await frame();
        gaps.push(now - last);
        last = now;
      }
      samples.sort((one, two) => one - two);
      gaps.sort((one, two) => one - two);
      layout.zoom = zoomWas;
      layout.panX = 0;
      layout.panY = 0;
      paintKnowledgeFrame(view, layout);
      rows.push({
        name: scene.name,
        nodes: layout.count,
        edges: layout.model.edgeCount,
        clusters: layout.namedCount,
        firstPaintMs: Math.round(firstPaintMs),
        settleFrames: waited,
        cameraP95Ms: round2(samples[Math.floor(samples.length * 0.95)]),
        cameraWorstMs: round2(samples[samples.length - 1]),
        cameraMedianMs: round2(median(samples)),
        frameP95Ms: round2(gaps[Math.floor(gaps.length * 0.95)]),
        frameWorstMs: round2(gaps[gaps.length - 1]),
        frameMedianMs: round2(median(gaps)),
        canvasWide: Math.round(view.querySelector(".knowledge-canvas").clientWidth),
        tier: view.dataset.knowledgeTier ?? "",
        labelsShown: labelBoxes().length,
        labelOverlaps: overlapPairs(labelBoxes()),
        discOverlapPairs: discOverlaps(layout),
        outsideDisc: outsideDisc(layout),
        draws: drawCount(layout),
        heapMb: heapMb(),
        sceneMs: Math.round(performance.now() - began),
      });
    }
    /* 재고 나면 손을 되돌린다 — 다음 케이스가 남의 손 위에서 서지 않게. */
    knowledgePainterKind = pinned;
    await paintKnowledgeView();
    return rows;
  }, { list, sweep: { ...SWEEP, frames }, hand: painter });
  if (seatWas) await page.setViewportSize(seatWas);

  const byName = Object.fromEntries(scenes.map((row) => [row.name, row]));
  /* 계약은 「빠르다」가 아니라 「겹치지 않는다」다 — 속도의 수는 표에 적히고 예산은
   * 설계 §2가 단계마다 확정한다.
   *
   * **이름표 겹침과 제 원반 밖의 점은 어느 크기에서도 0이다** — 5차가 그림의 성질로
   * 약속한 둘이고, 격자와 훑기의 제약은 점의 수에 기대지 않는다.
   *
   * **원반끼리의 겹침은 5차가 잰 크기(볼트·천 쪽)에서만 0으로 묻는다.** 첫 실측이
   * 여기서 나왔다: 오천 쪽에서 7쌍, 만 쪽에서 42쌍의 주제 원반이 겹친다. 군집 홈의
   * 이완은 한 판에서 `clusterVisits`만큼의 쌍 방문을 쓰는데, 군집이 수십이 되면 그
   * 예산 안에서 인력과 밀어내기가 균형에 닿지 못한다. 이 수를 0으로 만드는 것은
   * 이완의 일이지 페인터의 일이 아니므로 6차의 P0~P2는 그것을 **재고 적을** 뿐이다 —
   * 고치는 자리는 배치이고, 고치기 전에 숫자가 있어야 한다. */
  /* 묻는 크기는 이 판이 잰 장면들이다 — 가장 큰 장면이 문장의 끝이다. */
  const largest = list[list.length - 1];
  ok(`${painter}: labels never overlap and no page leaves its own disc, from the vault to ${largest.pages} pages`,
    scenes.every((row) => row.labelOverlaps === 0 && row.outsideDisc === 0
      && row.nodes > 0 && row.labelsShown > 0),
    JSON.stringify(scenes));
  ok(`${painter}: at the sizes the fifth round measured, the topic discs still do not touch`,
    ["vault-shaped", "k1"].every((name) => byName[name] === undefined || byName[name].discOverlapPairs === 0),
    JSON.stringify(scenes.map((row) => `${row.name}:${row.clusters}c/${row.discOverlapPairs}p`)));
  /* 선은 쪽수의 2.5배다 — 시나리오가 제 모양을 지키는지 먼저 묻는다. 이 줄이
   * 없으면 「만 쪽에서 빨랐다」가 실은 「선이 적었다」일 수 있다. */
  ok(`${painter}: the fabricated scenes carry two and a half edges per page, up to ${largest.pages} pages`,
    byName[largest.name].nodes >= largest.pages
      && scenes.filter((row) => row.name !== "vault-shaped")
        .every((row) => row.edges >= row.nodes * 2.4),
    JSON.stringify(scenes.map((row) => `${row.name}:${row.nodes}/${row.edges}`)));
  if (painter === "gl") {
    /* 설계 §5의 약속 둘: 드로우는 다섯을 넘지 않고, 노드당 DOM은 0이다 — 이름표만
     * DOM이고 그 수는 판 폭의 예산이 묶는다(점의 수가 아니라). */
    ok(`gl: at most five instanced draws carry ${largest.pages} pages, and no page owns an element of its own`,
      scenes.every((row) => row.draws > 0 && row.draws <= 5 && row.labelsShown <= 64),
      JSON.stringify(scenes.map((row) => `${row.name}:${row.draws}draws/${row.labelsShown}labels`)));
  }
  console.log(`METRIC knowledge scenes (${painter}): ${JSON.stringify(scenes)}`);
  return scenes;
}

/* ---- 페인터 자리의 계약 (6차 P1) -------------------------------------------
 *
 * 페인터가 둘이 되는 순간 두 가지를 물어야 한다: **같은 답을 내는가**(픽킹·그린 점의
 * 수)와 **놓는가**(버퍼·요소·리스너). 아래 한 함수가 표의 페인터를 전부 돌며 그 둘을
 * 묻는다 — P1에서는 줄이 하나(SVG)이므로 「열 번 갈아 끼워도 판이 자란 자리가
 * 없다」를 묻고, P2에서 GL 줄이 들어오면 같은 물음이 두 손을 오간다. */
export async function measureKnowledgePainterSwap(page, ok) {
  const swap = await page.evaluate(async () => {
    const view = document.querySelector(".knowledge-view:not([hidden])");
    const frame = () => new Promise((done) => requestAnimationFrame(done));
    const kinds = KNOWLEDGE_PAINTERS.map((row) => row.id);
    const shape = (painter) => ["mount", "paintTopology", "paintDress", "paintFrame", "pick", "dispose"]
      .every((one) => typeof painter[one] === "function");
    /* 모양은 판에 세우지 않고 묻는다(`make`는 손을 짓기만 한다). 세워서 물으면 WebGL2가
     * 없는 이 판에서 GL 손은 서지 못하고 SVG로 돌아온 손이 GL의 이름으로 답한다 — 그러면
     * GL의 모양은 한 번도 물은 적이 없다. */
    const interfaced = KNOWLEDGE_PAINTERS.every((row) => shape(row.make()));
    /* 아래의 대조는 브라우저가 그린 **점의 요소**를 읽는다 — SVG 손을 못박고, 끝에서 못박기
     * 전의 값으로 되돌린다. GL 손의 계약은 제 판(`measureKnowledgeGlParity`)이 묻는다. */
    const pinned = knowledgePainterKind;
    knowledgePainterKind = "svg";
    /* 그림 하나를 세우고 그 위에서 묻는다 — 빈 판은 아무것도 놓을 것이 없다. */
    knowledgeReport = window.__buildVaultGraph__({ path: "/painter", sources: false },
      { pages: 400, linksPer: 3, ghosts: 20, tags: ["core", "reading"] });
    await paintKnowledgeView();
    await frame();
    const layout = knowledgeLayouts.get(view);

    /* 픽킹의 대조 — 두 물음을 한 자리에서.
     *
     * (1) 투영기가 말하는 자리에 **브라우저가 실제로 그린 잉크**가 있는가: 점의
     * <circle>이 차지한 상자의 한가운데와 투영한 자리가 1px 안에서 같아야 한다.
     * 이 한 줄이 이름표 격자와 GL이 함께 기대는 계약이다 — 격자는 점의 몸이 어디에
     * 있는지를 이 수로 예약하고, GL은 같은 수로 빌보드를 세운다.
     *
     * (2) 그 자리를 짚으면 페인터가 그 점을 답하는가.
     *
     * 브라우저의 히트테스트는 답의 **잣대가 아니다**: 몸이 겹친 점들 위에서
     * `elementFromPoint`는 나중에 그려진 것을 돌려주고(실측: 자리 0을 짚으면 409),
     * 페인터는 가장 가까운 중심을 돌려준다. 둘 다 옳은 답이라 견줄 수 없다. 겹치지
     * 않는 자리에서는 같다는 것만 적어 둔다. */
    const canvasBox = view.querySelector(".knowledge-canvas").getBoundingClientRect();
    const painter = knowledgePainterFor(view);
    const probes = [];
    for (const seat of [0, Math.floor(layout.count / 3), layout.count - 1]) {
      if (layout.drawn !== null && layout.drawn[seat] === 0) continue;
      const px = layout.project.screenX(seat);
      const py = layout.project.screenY(seat);
      if (px < 0 || py < 0 || px > canvasBox.width || py > canvasBox.height) continue;
      const dot = layout.nodeEls[seat]?.querySelector(".knowledge-dot");
      if (!dot) continue;
      const ink = dot.getBoundingClientRect();
      const under = document.elementFromPoint(canvasBox.left + px, canvasBox.top + py);
      const hit = under?.closest(".knowledge-node");
      probes.push({ seat, picked: painter.pick(px, py),
        driftX: Math.round((px - (ink.left + ink.width / 2 - canvasBox.left)) * 100) / 100,
        driftY: Math.round((py - (ink.top + ink.height / 2 - canvasBox.top)) * 100) / 100,
        hit: hit === null || hit === undefined ? -1 : Number(hit.dataset.graphSeat) });
    }
    /* 빈 자리는 빈 답이다 — 점 하나 없는 구석을 짚으면 -1. */
    const emptyPick = painter.pick(-9999, -9999);

    /* 놓는가. 열 번 갈아 끼우고 판이 자란 자리를 센다 — 요소 수, 자리 배열의 길이,
     * 그리고 JS 힙. 힙은 수거를 한 번 돌린 뒤에 읽는다(`gc`가 있으면). */
    const settle = async () => {
      await paintKnowledgeView();
      await frame();
      return {
        elements: view.querySelectorAll(
          ".knowledge-clusters *, .knowledge-edges *, .knowledge-nodes *").length,
        nodeEls: knowledgeLayouts.get(view).nodeEls.length,
        cached: knowledgeLayouts.get(view).nodeCache.size,
      };
    };
    /* 수거를 두 번 돌리고 읽는다 — 한 번의 `gc()`는 방금 놓은 것을 아직 들고 있는
     * 세대를 남긴다. 손잡이가 없는 판에서는 0을 돌려주고 판정은 요소 수만 본다. */
    const heap = async () => {
      for (let round = 0; round < 2; round += 1) {
        if (typeof window.gc === "function") window.gc();
        await frame();
      }
      return performance.memory ? performance.memory.usedJSHeapSize : 0;
    };
    const swapOnce = async () => {
      const held = knowledgePainterFor(view);
      held.dispose();
      knowledgePainters.delete(view);
      const bare = view.querySelectorAll(".knowledge-nodes *").length === 0;
      await settle();
      return bare;
    };
    /* 기준은 **한 번 갈아 낀 판**이다. 첫 그림 직후의 힙에는 판을 짓느라 생긴
     * 쓰레기가 아직 남아 있어서, 그것을 기준으로 삼으면 열 번을 갈아 낀 뒤가 언제나
     * 「줄었다」로 읽힌다(실측 -5.7%). 두 읽기를 같은 자리에 놓아야 그 차가 새는
     * 것인지 아닌지를 말한다. */
    await swapOnce();
    const before = await settle();
    const heapBefore = await heap();
    let emptied = 0;
    for (let round = 0; round < 10; round += 1) {
      if (await swapOnce()) emptied += 1;
    }
    const after = await settle();
    const heapAfter = await heap();
    knowledgePainterKind = pinned;
    return { kinds, interfaced, probes, emptyPick, before, after, emptied,
      heapBefore, heapAfter,
      heapDrift: heapBefore === 0 ? 0
        : Math.round(((heapAfter - heapBefore) / heapBefore) * 1000) / 10 };
  });
  ok("every painter wears the same five-method shape, the projector lands on the ink the browser drew, and that spot picks its own seat",
    swap.interfaced && swap.probes.length > 0
      && swap.probes.every((row) => row.picked === row.seat && row.hit >= 0
        && Math.abs(row.driftX) < 1 && Math.abs(row.driftY) < 1)
      && swap.emptyPick === -1,
    JSON.stringify({ kinds: swap.kinds, probes: swap.probes, emptyPick: swap.emptyPick }));
  /* ±5%: 판 하나를 열 번 갈아 끼운 뒤의 힙이 처음과 같아야 한다. 갈아 끼우는
   * 동안 판이 요소를 쥐고 있으면(놓지 않은 창고·리스너·버퍼) 이 수가 자란다. */
  ok("ten painter swaps leave the picture, its element count and the JS heap where they started",
    swap.emptied === 10 && swap.after.elements === swap.before.elements
      && swap.after.nodeEls === swap.before.nodeEls
      && swap.after.cached === swap.before.cached
      && Math.abs(swap.heapDrift) <= 5,
    JSON.stringify(swap));
  console.log(`METRIC knowledge painter swap: ${JSON.stringify({ kinds: swap.kinds,
    elements: swap.before.elements, heapBefore: swap.heapBefore, heapAfter: swap.heapAfter,
    heapDrift: `${swap.heapDrift}%` })}`);
  return swap;
}

/* ---- 두 손이 같은 그림을 그리는가 (6차 P2) ---------------------------------
 *
 * P2의 인수 기준은 「같은 그림」이고, 그 말을 시험이 물을 수 있는 세 수로 옮긴다:
 * **그려진 점의 수**, **이름표의 자리**, **밝힌 집합**. 둘 다 같은 `layout`을 읽고
 * 같은 격자를 부르므로 이 셋이 어긋나면 그것은 손의 잘못이지 그림의 차이가 아니다.
 *
 * 픽셀을 견주지 않는 이유: 같은 그림을 SDF로 그린 원과 SVG로 그린 원은 가장자리
 * 반 픽셀이 다르고, 그 차는 판마다 다르다. 사람이 읽는 것은 「무엇이 어디에 몇 개」이고
 * 그 셋은 기계와 무관하게 같아야 한다. 눈으로 보는 대조는 스크린샷이 진다
 * (docs/design/knowledge-graph-3d-20260916/). */
export async function measureKnowledgeGlParity(page, ok) {
  const seen = await page.evaluate(async ({ slack }) => {
    const view = document.querySelector(".knowledge-view:not([hidden])");
    const frame = () => new Promise((done) => requestAnimationFrame(done));
    knowledgeQuery = "";
    knowledgeSelectedKey = null;
    knowledgeClusterPicked = -1;
    /* 볼트 자체를 크게 만들고 답도 직접 싣는다 — 하나만으로는 모자랐다: 답만
     * 실으면 탭을 막 연 판에 뒤늦게 도착한 백엔드의 답이 그 그림을 덮고(실측:
     * 600쪽을 세웠는데 12쪽이 서서 대조가 점 열한 개 위에서 돌았다), 볼트만
     * 바꾸면 그 답이 올 때까지 판이 비어 있다. 둘 다 같은 그림을 말하면 어느 쪽이
     * 이겨도 대조는 600쪽 위에서 돈다. */
    window.__VAULT__ = { pages: 600, linksPer: 3, ghosts: 40,
      tags: ["core", "reading", "tools"] };
    knowledgeReport = window.__buildVaultGraph__({ path: "/parity", sources: false },
      window.__VAULT__);
    knowledgeEntryPending = false;
    knowledgeRevealKey = null;
    knowledgeMode = "global";
    /* 두 손을 번갈아 못박고, 끝에서 못박기 전의 값(`null`이면 창이 고르는 손)으로 되돌린다. */
    const pinned = knowledgePainterKind;
    knowledgePainterKind = "svg";
    knowledgeLayouts.delete(view);
    await paintKnowledgeView();
    /* 판이 제 답을 물고 앉을 때까지 — 탭을 막 연 판은 백엔드의 답을 한 번 더
     * 물어 오고, 그 답이 이 그림을 덮는다(실측: 600쪽을 세웠는데 12쪽이 섰다). */
    for (let round = 0; round < 400; round += 1) {
      await frame();
      const held = knowledgeLayouts.get(view);
      if (held !== undefined && held.count > 600 && held.left === 0) break;
    }
    /* 사람이 한 점을 고른 판에서 견준다 — 선택·이웃 밝힘·이름표 예외가 한꺼번에
     * 걸린 그림이라야 「밝힌 집합」이 뜻을 가진다. */
    const layout = knowledgeLayouts.get(view);
    const centre = layout.model.keys[0];
    selectKnowledgeNode(view, centre);
    litKnowledge(view, layout, centre);
    await paintKnowledgeView();
    await frame();
    const snapshot = () => {
      const held = knowledgeLayouts.get(view);
      let drawn = 0;
      for (let at = 0; at < held.count; at += 1) {
        if (held.drawn === null || held.drawn[at] === 1) drawn += 1;
      }
      return {
        painter: knowledgePainterFor(view).id,
        drawn,
        points: knowledgePainterFor(view).id === "svg"
          ? view.querySelectorAll(".knowledge-node").length
          : held.paintStats.points,
        labels: [...held.labelShown].join(""),
        where: [...held.labelWhere].join(","),
        lit: [...held.litNodes].sort((one, two) => one - two).join(","),
        draws: knowledgePainterFor(view).id === "svg" ? -1 : held.paintStats.draws,
        /* 서 있는 이름표의 자리 — 화면 픽셀로. 두 손이 같은 격자를 부르므로
         * 이 수들도 같아야 한다(반올림 한 칸까지). */
        seats: (() => {
          const rows = [];
          for (let at = 0; at < held.count && rows.length < 40; at += 1) {
            if (held.labelShown[at] !== 1) continue;
            rows.push(`${at}@${Math.round(held.project.screenX(at))},`
              + `${Math.round(held.project.screenY(at))}:${held.labelWhere[at]}`);
          }
          return rows.join("|");
        })(),
      };
    };
    const svg = snapshot();
    const svgElements = view.querySelectorAll(".knowledge-nodes *").length;
    knowledgePainterKind = "gl";
    await paintKnowledgeView();
    await frame();
    const gl = snapshot();
    const glElements = view.querySelectorAll(".knowledge-nodes *").length;
    const glLabels = view.querySelectorAll(".knowledge-gl-label:not([hidden])").length;
    /* 호버는 프레임을 부르지 않는다 — SVG에서는 클래스가 곧 그림이므로. 요소 없는 손은
     * 그 순간 제 옷을 다시 채워야 한다(`paintDress`). 다시 그리기 **없이** 짚고, GL이
     * 올린 점의 불투명도로 묻는다(검증 09-16: 알리지 않던 판은 호버가 그림에 닿지
     * 않았고, 위의 대조는 짚은 뒤 한 번 더 그려서 그것을 못 봤다). */
    const glHand = knowledgePainters.get(view);
    const held = knowledgeLayouts.get(view);
    const inkAlpha = (seat) => {
      for (let at = 0; at < glHand.counts.nodes; at += 1) {
        if (glHand.nodeSeat[at] === seat) return glHand.nodeFill[at * 4 + 3];
      }
      return -1;
    };
    selectKnowledgeNode(view, null);
    litKnowledge(view, held, null);
    const pointed = 1;
    const reach = knowledgeNeighbours(held, pointed);
    let far = -1;
    for (let at = 0; at < held.count && far < 0; at += 1) {
      if (at !== pointed && !reach.nodes.has(at) && (held.drawn === null || held.drawn[at] === 1)) far = at;
    }
    const near = [...reach.nodes].find((at) => at !== pointed) ?? pointed;
    const hover = { far, near, farBefore: inkAlpha(far), nearBefore: inkAlpha(near) };
    litKnowledge(view, held, held.model.keys[pointed]);
    hover.farDuring = inkAlpha(far);
    hover.nearDuring = inkAlpha(near);
    litKnowledge(view, held, null);
    hover.farAfter = inkAlpha(far);
    /* 오버레이의 이름표는 격자가 쥔 상자의 한가운데에 선다(`labelAtX/Y`). */
    const standsAtGrid = (() => {
      const spans = [...view.querySelectorAll(".knowledge-gl-label:not([hidden])")];
      let read = 0;
      for (let seat = 0; seat < held.count; seat += 1) {
        if (held.labelShown[seat] !== 1 || held.labelWhere[seat] < 0) continue;
        const moved = /translate\((-?\d+)px, (-?\d+)px\)/u.exec(spans[read]?.style.transform ?? "");
        read += 1;
        if (moved === null || Number(moved[1]) !== Math.round(held.labelAtX[seat])
          || Number(moved[2]) !== Math.round(held.labelAtY[seat])) return false;
      }
      return read > 0 && read === spans.length;
    })();
    /* 누르는 것은 GL 판에서도 눌린다: 주제의 이름판과 오버레이의 이름이 포인터의 자리에서
     * 제 요소로 잡히고(`elementFromPoint`는 pointer-events를 따른다), 이름을 누르면 그
     * 점이 골라진다(검증 09-16: 층의 `pointer-events: none`을 물려받아 이름판이 죽어 있었다). */
    const hitAt = (element) => {
      const box = element.getBoundingClientRect();
      const x = box.left + box.width / 2;
      const y = box.top + box.height / 2;
      const hit = document.elementFromPoint(x, y);
      return { hit: hit !== null && (hit === element || element.contains(hit)), x, y, target: hit };
    };
    const plate = view.querySelector(".knowledge-cluster-label:not(.is-folded)");
    const word = view.querySelector(".knowledge-gl-label:not([hidden])");
    const plateHit = plate === null ? null : hitAt(plate);
    const wordHit = word === null ? null : hitAt(word);
    let picked = null;
    if (wordHit?.hit) {
      wordHit.target.dispatchEvent(new MouseEvent("click", { bubbles: true, clientX: wordHit.x, clientY: wordHit.y }));
      picked = knowledgeSelectedKey;
    }
    const pressable = { plate: plateHit?.hit === true, label: wordHit?.hit === true,
      picked: picked !== null && picked === word?.dataset.graphKey };
    selectKnowledgeNode(view, null);
    /* 주변 탐색(첫 방문의 기본)도 같은 손으로 — 고리의 이름표는 격자가 쥔 상자에 서고
     * 서로 겹치지 않는다. 고리의 자리 셈(`knowledgeRingLabelSeat`)을 GL이 부르지
     * 않던 판에서는 격자가 비어 있는 자리로 상자를 쥐었다(검증 09-16). */
    setKnowledgeMode(view, "local", { centre: held.model.keys[pointed] });
    for (let round = 0; round < 400; round += 1) {
      await frame();
      const now = knowledgeLayouts.get(view);
      if (now !== undefined && now.ring !== null && now.left === 0) break;
    }
    const ringBoxes = window.__knowledgeLabelBoxes__(view);
    const ring = { painter: knowledgePainterFor(view).id,
      ringed: knowledgeLayouts.get(view)?.ring !== null,
      labels: ringBoxes.length, overlaps: window.__knowledgeOverlapPairs__(ringBoxes, slack) };
    setKnowledgeMode(view, "global");
    await paintKnowledgeView();
    await frame();
    /* 티어가 바뀌면 사람이 배율을 쥐지 않은 그림은 다시 맞춘다 — 손과 무관하게. 그림이
     * 섰는지를 SVG의 점 요소로 묻던 판에서는 GL 그림이 한 번도 다시 맞지 않았다
     * (검증 09-16). */
    const tierLayout = knowledgeLayouts.get(view);
    tierLayout.zoomTaken = false;
    const fitWas = fitKnowledgeGraph;
    let refits = 0;
    fitKnowledgeGraph = (...args) => {
      refits += 1;
      return fitWas(...args);
    };
    const tierWas = view.dataset.knowledgeTier;
    view.dataset.knowledgeTier = "";
    try {
      paintKnowledgeTier(view);
    } finally {
      fitKnowledgeGraph = fitWas;
    }
    const tierRefit = { painter: knowledgePainterFor(view).id, refits,
      tier: view.dataset.knowledgeTier, was: tierWas };
    /* 창이 사람에게 한 말의 수 — 손이 무너져도 사람이 할 일은 없으므로 알림은 0이어야 한다
     * (P1 G2: 「문맥 잃음은 조용히 SVG」). 알림은 제 시간이 지나면 스스로 떠나므로 선 수가
     * 아니라 **붙은** 수를 센다. */
    const told = () => {
      const heard = { toasts: 0, stop: null };
      const count = (rows) => {
        for (const row of rows) heard.toasts += row.addedNodes.length;
      };
      const listening = new MutationObserver(count);
      listening.observe(document.querySelector(".toasts"), { childList: true });
      heard.stop = () => {
        /* 아직 배달되지 않은 기록까지 센다 — `disconnect`는 그것을 버린다. */
        count(listening.takeRecords());
        listening.disconnect();
        return heard.toasts;
      };
      return heard;
    };
    /* 문맥을 잃으면 2D로 돌아오는가 — 판이 스스로 잃게 만들어 본다. */
    const canvas = view.querySelector(".knowledge-gl");
    const lost = canvas?.getContext("webgl2")?.getExtension("WEBGL_lose_context");
    let fellBack = null;
    if (lost) {
      const heard = told();
      canvas.dispatchEvent(new Event("webglcontextlost", { cancelable: true }));
      for (let round = 0; round < 200; round += 1) {
        await frame();
        if (view.querySelectorAll(".knowledge-node").length > 0) break;
      }
      fellBack = { kind: knowledgePainterKind,
        canvasGone: view.querySelector(".knowledge-gl") === null,
        nodesBack: view.querySelectorAll(".knowledge-node").length,
        toasts: heard.stop() };
    }
    /* 문맥이 서지 않는 판 — 시작에서 세운 캔버스·오버레이·견본이 남지 않고 2D로
     * 돌아오는가(검증 09-16: 두 실패 길이 캔버스와 GPU 문맥을 판에 남겼다). 판이
     * 스스로 거절하게 만든다. */
    const canvasContext = HTMLCanvasElement.prototype.getContext;
    HTMLCanvasElement.prototype.getContext = function refuse(kind, ...rest) {
      return kind === "webgl2" ? null : canvasContext.call(this, kind, ...rest);
    };
    knowledgePainterKind = "gl";
    const refusal = told();
    try {
      await paintKnowledgeView();
      await frame();
    } finally {
      HTMLCanvasElement.prototype.getContext = canvasContext;
    }
    const refused = { kind: knowledgePainterKind,
      canvases: view.querySelectorAll(".knowledge-gl").length,
      overlays: view.querySelectorAll(".knowledge-gl-labels").length,
      swatches: view.querySelectorAll(".knowledge-gl-swatch").length,
      nodes: view.querySelectorAll(".knowledge-node").length,
      toasts: refusal.stop() };
    knowledgePainterKind = pinned;
    knowledgeSelectedKey = null;
    await paintKnowledgeView();
    return { svg, gl, svgElements, glElements, glLabels, fellBack, hover, standsAtGrid, pressable, ring,
      tierRefit, refused,
      able: knowledgeGlSupported() };
  }, { slack: SWEEP.overlapSlack });
  ok("the GL painter draws the same picture: the same points, the same label seats and the same lit set",
    seen.able && seen.gl.painter === "gl"
      && seen.gl.points === seen.svg.points && seen.gl.drawn === seen.svg.drawn
      && seen.gl.labels === seen.svg.labels && seen.gl.where === seen.svg.where
      && seen.gl.seats === seen.svg.seats && seen.gl.lit === seen.svg.lit,
    JSON.stringify({ svg: { ...seen.svg, seats: seen.svg.seats.slice(0, 120) },
      gl: { ...seen.gl, seats: seen.gl.seats.slice(0, 120) }, able: seen.able }));
  ok("the GL picture owns no element per page — the labels it does own are the ones the grid stood",
    seen.glElements === 0 && seen.svgElements > 0
      && seen.glLabels === seen.svg.labels.split("").filter((one) => one === "1").length,
    JSON.stringify({ svgElements: seen.svgElements, glElements: seen.glElements,
      glLabels: seen.glLabels }));
  ok("a lost GL context falls back to 2D without a word to the person, and the pages come back as elements",
    seen.fellBack !== null && seen.fellBack.kind === "svg"
      && seen.fellBack.canvasGone && seen.fellBack.nodesBack > 0 && seen.fellBack.toasts === 0,
    JSON.stringify(seen.fellBack));
  ok("hovering on the GL hand redraws its dress without a repaint: far pages step back, the lit ones stay, letting go restores them",
    seen.hover.far >= 0 && seen.hover.farDuring < seen.hover.farBefore
      && seen.hover.nearDuring === seen.hover.nearBefore && seen.hover.farAfter === seen.hover.farBefore,
    JSON.stringify(seen.hover));
  ok("the GL overlay stands every label at the centre of the box the label grid reserved",
    seen.standsAtGrid === true, JSON.stringify({ standsAtGrid: seen.standsAtGrid }));
  ok("on the GL hand the topic plates still take the pointer and a label picks its own page",
    seen.pressable.plate && seen.pressable.label && seen.pressable.picked, JSON.stringify(seen.pressable));
  ok("on the GL hand a tier change still refits an untaken camera",
    seen.tierRefit.painter === "gl" && seen.tierRefit.refits === 1, JSON.stringify(seen.tierRefit));
  ok("in the neighbourhood ring the GL labels stand in the grid's boxes and none of them overlap",
    seen.ring.painter === "gl" && seen.ring.ringed && seen.ring.labels > 0 && seen.ring.overlaps === 0,
    JSON.stringify(seen.ring));
  ok("a GL hand that cannot start leaves no canvas, overlay, swatch or word behind, and the pages come back as elements",
    seen.refused.kind === "svg" && seen.refused.canvases === 0 && seen.refused.overlays === 0
      && seen.refused.swatches === 0 && seen.refused.nodes > 0 && seen.refused.toasts === 0,
    JSON.stringify(seen.refused));
  console.log(`METRIC knowledge gl parity: ${JSON.stringify({ points: seen.gl.points,
    draws: seen.gl.draws, svgElements: seen.svgElements, glElements: seen.glElements,
    glLabels: seen.glLabels })}`);
  return seen;
}
