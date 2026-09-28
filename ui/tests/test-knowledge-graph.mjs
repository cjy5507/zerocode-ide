import { createRequire } from "node:module";
import { frameBudgetHolds, loadNote, machineIsLoudNow } from "./machine-load.mjs";
import { createServer } from "node:http";
import { readFile, mkdir } from "node:fs/promises";
import { dirname, extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { KNOWLEDGE_SCENES, knowledgeSweepFor, measureKnowledgeGlParity, measureKnowledgePainterSwap,
  measureKnowledgeScenes,
  testKnowledgePerformance } from "./knowledge-performance.mjs";
import { testKnowledgeCode, measureKnowledgeCodeScene } from "./knowledge-code.mjs";
import { seedKnowledgeWindow } from "./knowledge-fixture.mjs";
import { measureKnowledgeSupplyParity, measureKnowledgeSupplyScene, testKnowledgeSupply } from "./knowledge-supply.mjs";

const UI = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const require = createRequire(import.meta.url);
const { chromium } = require("playwright");

const files = createServer(async (request, response) => {
  const asked = decodeURIComponent((request.url ?? "/").split("?")[0]);
  const path = join(UI, asked === "/" ? "index.html" : asked);
  if (!path.startsWith(UI)) {
    response.writeHead(403).end();
    return;
  }
  try {
    const data = await readFile(path);
    const type = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".mjs": "text/javascript", ".png": "image/png", ".ico": "image/x-icon" };
    response.writeHead(200, { "content-type": type[extname(path)] ?? "text/plain" });
    response.end(data);
  } catch {
    response.writeHead(404).end();
  }
});
await new Promise((done) => files.listen(0, "127.0.0.1", done));
const origin = `http://127.0.0.1:${files.address().port}`;

/* 힙을 재려면 두 손잡이가 필요하다(6차 P1): `--enable-precise-memory-info`가 없으면
 * `performance.memory`는 100 KB 단위로 뭉개진 값을 내고, `--expose-gc`가 없으면
 * 「놓았는가」를 물을 때 아직 수거되지 않은 쓰레기가 답을 대신한다. 두 손잡이는
 * 재는 방법을 바꿀 뿐 그리는 코드를 바꾸지 않는다 — 전·후 두 판이 같은 손잡이로
 * 돈다. */
const HEAP_ARGS = ["--enable-precise-memory-info", "--js-flags=--expose-gc"];
/* 헤드리스 크로미엄은 기본으로 WebGL2를 주지 않는다 — GL 페인터의 계약을 물으려면
 * 켜야 하고, 켜지는 것은 **소프트웨어 래스터라이저**(SwiftShader)다.
 *
 * 그래서 판을 둘로 나눈다. 이 손잡이를 2D의 판에 걸면 SVG의 래스터화까지 소프트웨어가
 * 지고 그 판의 모든 수가 열 배 느려진다(실측: 천 쪽 최악 프레임 46 → 436 ms, 캔버스
 * 캡처 50 → 651 ms). 2D의 수는 GPU가 있는 판에서, GL의 **계약**(드로우 수·그린 점·
 * 이름표 자리)은 소프트웨어 판에서 — 계약은 기계와 무관하고 시간은 그렇지 않다.
 * GL의 시간은 실제 GPU에서 따로 잰다(docs/design/knowledge-graph-3d-20260916/). */
const GL_ARGS = [...HEAP_ARGS, "--use-gl=angle", "--use-angle=swiftshader",
  "--enable-unsafe-swiftshader"];
const browser = await chromium.launch({ args: HEAP_ARGS });
const page = await browser.newPage({ viewport: { width: 1280, height: 860 } });
const faults = [];
page.on("pageerror", (error) => faults.push(error?.stack ?? String(error)));

await seedKnowledgeWindow(page);

await page.goto(`${origin}/index.html`);
await page.waitForFunction(() => typeof BOUND !== "undefined" && BOUND.size > 0);

const results = [];
const ok = (name, pass, detail = "") => {
  results.push({ name, pass: !!pass, detail });
  console.log(`${pass ? "PASS" : "FAIL"} ${name}`);
  if (!pass) console.error("   detail:", detail);
};

// Test 1: Brain initial render
const brain = await page.evaluate(async () => {
  window.__VAULT__ = { pages: 12, linksPer: 2, ghosts: 3, tags: ["core", "reading"] };
  secondBrainVault = "/vault";
  /* 이 하네스의 볼트는 마지막으로 전체 지도로 본 볼트다 — 이 검사와 뒤의 첫 그림·
     프레임·DOM 예산 검사는 전체 그림을 재는 검사라서(설정 문서의 줄이 서는 자리와
     같은 문). 진짜 첫 방문(줄 없음 → 주변 탐색)은 S2가 줄을 지우고 따로 검사한다. */
  noteKnowledgeExploreLines({ "/vault": JSON.stringify({ mode: "global" }) });
  paintKnowledgeEntry();
  const entryHidden = el("nav-knowledge").hidden;
  el("nav-knowledge").click();
  const viewOf = () => [...document.querySelectorAll(".file-view")]
    .find((one) => one.classList.contains("knowledge-view") && !one.hidden);
  /* 앉는 과정이 보여야 한다(3차): 판이 서자마자 남은 예산이 있고, 그 예산이 여러
   * 프레임에 걸쳐 줄어든다. 프레임 수를 세고 나서야 아래의 자리들을 읽는다. */
  let pacedLeft = 0;
  for (let round = 0; round < 120; round += 1) {
    await new Promise((done) => requestAnimationFrame(done));
    const held = viewOf() ? knowledgeLayouts.get(viewOf()) : null;
    if (held && held.count > 0) {
      pacedLeft = held.left;
      break;
    }
  }
  let settleFrames = 0;
  while (settleFrames < 600 && (knowledgeLayouts.get(viewOf())?.left ?? 0) > 0) {
    await new Promise((done) => requestAnimationFrame(done));
    settleFrames += 1;
  }
  const view = viewOf();
  const nodes = [...(view?.querySelectorAll(".knowledge-node") ?? [])];
  const first = nodes[0]?.getAttribute("transform") ?? "";
  const radii = nodes.map((node) => Number(node.querySelector(".knowledge-dot")
    ?.getAttribute("r") ?? 0)).filter((radius) => radius > 0);
  const communityHues = new Map();
  const hueAgree = nodes.every((node) => {
    if (!node.dataset.hue) return true;
    const held = communityHues.get(node.dataset.community);
    if (held !== undefined && held !== node.dataset.hue) return false;
    communityHues.set(node.dataset.community, node.dataset.hue);
    return true;
  });
  const label = nodes[0]?.querySelector(".knowledge-label");
  const labelStyle = label ? getComputedStyle(label) : null;
  const layout = knowledgeLayouts.get(view);
  const canvas = view.querySelector(".knowledge-canvas");
  const hub = nodes.find((node) => node.dataset.hub === "true");
  const hubHalo = hub?.querySelector(".knowledge-halo");
  /* 두 점의 원이 겹치는가 (09-16). 3차는 「반지름 합의 두 배」라는 여유까지 재고
     있었지만, 단이 생긴 뒤로 그 비율은 큰 중심 하나가 잎 곁에 앉기만 해도 0.6이
     된다 — 그것은 겹침이 아니라 이웃함이다. 계약은 겹치지 않는 것이다. */
  let overlappingDots = 0;
  for (let left = 0; left < nodes.length; left += 1) {
    for (let right = left + 1; right < nodes.length; right += 1) {
      const distance = Math.hypot(layout.x[left] - layout.x[right], layout.y[left] - layout.y[right])
        * layout.scale;
      if (distance < radii[left] + radii[right]) overlappingDots += 1;
    }
  }
  /* 이름표가 서로 겹치는가 — 화면 상자로(09-16). 격자가 자리를 나눠 준 뒤의 답이라
     이 수가 0이 아니면 격자가 일하지 않은 것이다. 군집의 이름과 그 아래 한 줄도
     같은 격자를 쓰므로 함께 잰다. */
  const wordBoxes = [...view.querySelectorAll(".knowledge-label, .knowledge-cluster-name, .knowledge-cluster-count")]
    .filter((one) => one.getClientRects().length > 0 && getComputedStyle(one).display !== "none")
    .map((one) => one.getBoundingClientRect());
  let labelOverlaps = 0;
  for (let left = 0; left < wordBoxes.length; left += 1) {
    for (let right = left + 1; right < wordBoxes.length; right += 1) {
      const a = wordBoxes[left];
      const b = wordBoxes[right];
      if (a.right > b.left && b.right > a.left && a.bottom > b.top && b.bottom > a.top) {
        labelOverlaps += 1;
      }
    }
  }
  /* 원반끼리 겹치는가 — 성좌 배치의 약속(09-16). */
  const discs = [...view.querySelectorAll(".knowledge-nebula")]
    .map((one) => [Number(one.getAttribute("cx")), Number(one.getAttribute("cy")),
      Number(one.getAttribute("r"))]);
  let discOverlaps = 0;
  for (let left = 0; left < discs.length; left += 1) {
    for (let right = left + 1; right < discs.length; right += 1) {
      const gap = Math.hypot(discs[left][0] - discs[right][0], discs[left][1] - discs[right][1])
        - discs[left][2] - discs[right][2];
      if (gap < 0) discOverlaps += 1;
    }
  }
  /* 제 원반 밖에 선 점 — 훑기의 제약이 지키는 성질. */
  let strayNodes = 0;
  for (let at = 0; at < layout.count; at += 1) {
    const rank = layout.community[at];
    if (layout.communityHomed[rank] !== 1) continue;
    const far = Math.hypot(layout.x[at] - layout.communityHomeX[rank],
      layout.y[at] - layout.communityHomeY[rank]);
    if (far > layout.communityHomeR[rank] + 0.5) strayNodes += 1;
  }
  return {
    entryHidden,
    tab: [...document.querySelectorAll(".tab")].map((one) => one.textContent).join("|"),
    asks: window.__GRAPH_ASKS__ ?? 0,
    askedPath: window.__GRAPH_ASKED__?.path ?? null,
    nodeCount: nodes.length,
    ghostCount: view.querySelectorAll(".knowledge-node.is-ghost").length,
    edgeCount: view.querySelectorAll(".knowledge-edge").length,
    ghostEdges: view.querySelectorAll(".knowledge-edge.is-ghost").length,
    placed: /translate\(-?\d/u.test(first),
    hues: new Set(nodes.map((one) => one.dataset.hue).filter(Boolean)).size,
    hueAgree,
    pacedLeft,
    settleFrames,
    communities: nodes.every((one) => one.dataset.community !== undefined),
    clusterLabels: [...view.querySelectorAll(".knowledge-cluster-name")]
      .map((one) => one.textContent),
    nebulas: view.querySelectorAll(".knowledge-nebula").length,
    clusterLayerFirst: view.querySelector(".knowledge-picture > g")
      ?.classList.contains("knowledge-clusters") ?? false,
    namedClusters: knowledgeLayouts.get(view).namedCount,
    chips: [...view.querySelectorAll(".knowledge-chip")].map((one) => one.textContent),
    stat: view.querySelector(".knowledge-stat").textContent,
    /* 이름이 실제로 선 점의 수, 그리고 군집의 이름이 그림 위에 서 있는가. */
    namedNodes: view.querySelectorAll(".knowledge-node.is-named").length,
    clusterNameShown: view.querySelector(".knowledge-cluster-label") !== null
      && getComputedStyle(view.querySelector(".knowledge-cluster-label")).display !== "none",
    labelOverlaps,
    discOverlaps,
    strayNodes,
    pictures: view.querySelectorAll(".knowledge-picture").length,
    edgeLayers: view.querySelectorAll(".knowledge-edges").length,
    pageShape: nodes.find((one) => one.classList.contains("is-page"))
      ?.querySelector(".knowledge-dot")?.tagName ?? "",
    ghostShape: nodes.find((one) => one.classList.contains("is-ghost"))
      ?.querySelector(".knowledge-dot")?.tagName ?? "",
    radiusMin: Math.min(...radii),
    radiusMax: Math.max(...radii),
    /* 위계의 세 크기(09-16): 군집의 중심 > 대표 지식 > 잎. */
    tierRadii: Object.fromEntries(["core", "major", "leaf"].map((word) => [word,
      Math.max(0, ...nodes.filter((one) => one.dataset.tier === word)
        .map((one) => Number(one.querySelector(".knowledge-dot")?.getAttribute("r") ?? 0)))])),
    labelBelow: Number(label?.getAttribute("y") ?? 0)
      > Number(nodes[0]?.querySelector(".knowledge-dot")?.getAttribute("r") ?? 0),
    labelPaintOrder: labelStyle?.paintOrder ?? "",
    labelStrokeWidth: labelStyle?.strokeWidth ?? "",
    /* 전체 지도의 맞춤(t-12029): 묶는 축이 제 방을 채운다 — 방은 판의 `mapFit` 몫(세로는 아래 범례 띠 `mapFoot`을
     * 뺀 높이의 몫)이되, 작은 판에서는 가장자리에 `margin` px를 남긴 만큼이다. 그리고 그림의 경계는 판의 네 가장자리
     * (아래는 띠의 위)에서 적어도 `margin` px 떨어진다 — 가장자리의 점이 제 이름 한 줄을 세울 자리다. */
    ...(() => {
      const { mapFit, mapFoot, margin } = layout.tuning;
      const wide = canvas.clientWidth;
      const tall = canvas.clientHeight;
      const foot = Math.min(mapFoot, tall * (1 - mapFit));
      const roomX = Math.min(wide * mapFit, wide - 2 * margin);
      const roomY = Math.min((tall - foot) * mapFit, tall - foot - 2 * margin);
      const camera = layout.viewBoxRect;
      const screenY = (y) => (camera.middleY + (y - camera.middleY) * camera.yScale - camera.y) * layout.scale;
      return {
        initialFitRatio: Math.max(((layout.bounds.maxX - layout.bounds.minX) * layout.scale) / roomX,
          ((layout.bounds.maxY - layout.bounds.minY) * camera.yScale * layout.scale) / roomY),
        initialFitWanted: layout.zoom,
        initialEdgeRoom: Math.round(Math.min((layout.bounds.minX - camera.x) * layout.scale,
          wide - (layout.bounds.maxX - camera.x) * layout.scale, screenY(layout.bounds.minY),
          tall - foot - screenY(layout.bounds.maxY))),
        initialEdgeWanted: margin,
        canvasWide: wide,
        canvasTall: tall,
      };
    })(),
    overlappingDots,
    gradients: view.querySelectorAll("defs radialGradient[id^='knowledge-node-gradient-']").length,
    halos: view.querySelectorAll(".knowledge-halo").length,
    haloRatio: Number(hubHalo?.getAttribute("r") ?? 0)
      / Number(hub?.querySelector(".knowledge-dot")?.getAttribute("r") ?? 1),
    hubHaloDisplay: hubHalo ? getComputedStyle(hubHalo).display : "none",
    /* 서 있는 줄만 센다 — 공급망 렌즈(P4)의 줄은 그 렌즈가 켜졌을 때만 선다. */
    legendNodes: view.querySelectorAll(".knowledge-legend [data-node-kind]:not([hidden])").length,
    legendEdges: view.querySelectorAll(".knowledge-legend [data-edge-kind]:not([hidden])").length,
    overview: {
      hubs: view.querySelectorAll(".knowledge-overview-hubs button").length,
      tags: view.querySelectorAll(".knowledge-overview-tags button").length,
      recent: view.querySelectorAll(".knowledge-overview-recent button").length,
    },
  };
});
ok(
  "the knowledge graph draws the vault's wiki as points and its wikilinks as lines",
  brain.entryHidden === false &&
    brain.tab.includes("지식 그래프") &&
    brain.asks === 1 &&
    brain.askedPath === "/vault" &&
    brain.ghostEdges === 3 &&
    brain.nodeCount === 15 &&
    brain.ghostCount === 3 &&
    brain.edgeCount > 12 &&
    brain.placed &&
    // 색은 군집이다(3차): 같은 군집의 점은 같은 색이고, 이름 있는 군집마다 성운
    // 하나와 이름표 하나가 점 뒤의 층에 선다. 앉는 과정은 여러 프레임에 걸친다.
    brain.hues >= 1 &&
    brain.hues === brain.namedClusters &&
    brain.hueAgree &&
    brain.communities &&
    brain.clusterLabels.length === brain.namedClusters &&
    brain.nebulas === brain.namedClusters &&
    brain.clusterLabels.every((word) => word.length > 0) &&
    brain.clusterLayerFirst &&
    brain.pacedLeft > 0 &&
    brain.settleFrames >= 20 &&
    brain.chips.length === 2 &&
    brain.chips[0].startsWith("core") &&
    brain.stat.includes("12/12") &&
    // 성좌 배치(09-16): 주제의 이름이 그림 위에 서고, 이름표는 격자가 나눠 준
    // 자리에 서며(겹침 0), 원반끼리도 점과 원반도 겹치지 않는다. 옛 계약이던
    // 「차수 문턱 LOD의 is-labelled」는 사라졌다 — 이름이 서는지를 정하는 것이
    // 이제 차수가 아니라 자리이기 때문이다.
    brain.clusterNameShown &&
    brain.namedNodes > 0 &&
    brain.labelOverlaps === 0 &&
    brain.discOverlaps === 0 &&
    brain.strayNodes === 0 &&
    brain.pictures === 1 &&
    brain.edgeLayers === 1 &&
    brain.pageShape === "circle" &&
    brain.ghostShape === "circle" &&
    // 잎은 작고, 대표 지식은 그보다 크고, 군집의 중심이 가장 크다. 전체 지도의 점은 작다(t-12029 시안 v2
    // 「spread」: 잎 2.4, 가장 작은 부스러기 2 — 그보다 작으면 점이 아니라 먼지다).
    brain.tierRadii.leaf >= 2.4 &&
    brain.tierRadii.major > brain.tierRadii.leaf &&
    brain.tierRadii.core > brain.tierRadii.major &&
    brain.radiusMin >= 2 &&
    brain.radiusMax === brain.tierRadii.core &&
    brain.labelBelow &&
    brain.labelPaintOrder.includes("stroke") &&
    parseFloat(brain.labelStrokeWidth) === 2 &&
    Math.abs(brain.initialFitRatio - brain.initialFitWanted) < 0.02 &&
    brain.initialEdgeRoom >= brain.initialEdgeWanted - 1 &&
    brain.overlappingDots === 0 &&
    brain.gradients === 8 &&
    brain.halos === brain.nodeCount &&
    Math.abs(brain.haloRatio - 1.8) < 0.01 &&
    brain.hubHaloDisplay === "none" &&
    // 범례는 점 넷(페이지·유령·원본·회상됨)과 선 일곱(여섯 관계 + merge?) — t-2931.
    brain.legendNodes === 4 &&
    brain.legendEdges === 7 &&
    brain.overview.hubs === 5 &&
    brain.overview.tags === 2 &&
    brain.overview.recent === 5,
  JSON.stringify(brain),
);

// The contract names these four shots; keep the paths stable for the report.
await mkdir(join(UI, "..", "output/playwright"), { recursive: true });
await page.screenshot({ path: join(UI, "..", "output/playwright/knowledge-graph.png") });

// Test 2: Search highlights in place; lenses alone change membership.
const brainFilters = await page.evaluate(async () => {
  const viewOf = () => [...document.querySelectorAll(".file-view")]
    .find((one) => one.classList.contains("knowledge-view") && !one.hidden);
  const count = () => viewOf().querySelectorAll(".knowledge-node").length;
  const before = window.__GRAPH_ASKS__;
  const all = count();
  const layoutBefore = knowledgeLayouts.get(viewOf());
  const creationsBefore = knowledgeNodeCreations;

  const find = viewOf().querySelector(".knowledge-query");
  for (let at = 0; at < 100; at += 1) {
    find.value = `개념 ${at % 12}`;
    find.dispatchEvent(new Event("input", { bubbles: true }));
  }
  find.value = "개념 3";
  find.dispatchEvent(new Event("input", { bubbles: true }));
  await new Promise((done) => setTimeout(done, 60));
  const search = {
    nodes: count(),
    matches: viewOf().querySelectorAll(".knowledge-node.is-search-match").length,
    dimmed: viewOf().querySelectorAll(".knowledge-node.is-search-dim").length,
    stat: viewOf().querySelector(".knowledge-stat").textContent,
    creations: knowledgeNodeCreations - creationsBefore,
    sameLayout: knowledgeLayouts.get(viewOf()) === layoutBefore,
  };
  const searchAsked = window.__GRAPH_ASKS__;

  find.value = "";
  find.dispatchEvent(new Event("input", { bubbles: true }));
  await new Promise((done) => setTimeout(done, 60));

  const flag = (name) => viewOf().querySelector(`[data-knowledge-flag="${name}"]`);
  const remembered = viewOf().querySelector('[data-graph-key="wiki/Page-0000.md"]')
    ?.getAttribute("transform");
  flag("orphans").click();
  await new Promise((done) => setTimeout(done, 60));
  const orphans = count();
  const orphansPressed = flag("orphans").getAttribute("aria-pressed");
  flag("orphans").click();
  await new Promise((done) => setTimeout(done, 60));
  const restoredPlace = viewOf().querySelector('[data-graph-key="wiki/Page-0000.md"]')
    ?.getAttribute("transform");

  flag("ghosts").click();
  await new Promise((done) => setTimeout(done, 60));
  const ghostView = {
    nodes: count(),
    ghosts: viewOf().querySelectorAll(".knowledge-node.is-ghost").length,
  };
  flag("ghosts").click();
  await new Promise((done) => setTimeout(done, 60));

  const chip = viewOf().querySelector(".knowledge-chip");
  const tag = chip.dataset.knowledgeTag;
  chip.click();
  await new Promise((done) => setTimeout(done, 60));
  const tagged = {
    nodes: count(),
    matches: viewOf().querySelectorAll(".knowledge-node.is-search-match").length,
    query: viewOf().querySelector(".knowledge-query").value,
  };
  knowledgeTagsPicked.clear();
  find.value = "";
  find.dispatchEvent(new Event("input", { bubbles: true }));
  await new Promise((done) => setTimeout(done, 80));

  const asksAfterLocalFilters = window.__GRAPH_ASKS__;
  flag("sources").click();
  await new Promise((done) => setTimeout(done, 300));
  const withSources = {
    asked: window.__GRAPH_ASKED__?.sources === true,
    asks: window.__GRAPH_ASKS__,
    sources: viewOf().querySelectorAll(".knowledge-node.is-source").length,
    shape: viewOf().querySelector(".knowledge-node.is-source .knowledge-dot")?.dataset.shape ?? "",
  };
  flag("sources").click();
  await new Promise((done) => setTimeout(done, 300));
  return { before, all, search, searchAsked, orphans, orphansPressed, remembered, restoredPlace,
    ghostView, tagged, asksAfterLocalFilters, withSources, restored: count() };
});
ok(
  "search dims in place while lenses change membership and remember coordinates",
  brainFilters.all === 15 &&
    brainFilters.search.nodes === brainFilters.all &&
    brainFilters.search.matches === 1 &&
    brainFilters.search.dimmed === brainFilters.all - 1 &&
    brainFilters.search.stat.includes("1") &&
    brainFilters.search.creations === 0 &&
    brainFilters.search.sameLayout &&
    brainFilters.orphans === 2 &&
    brainFilters.orphansPressed === "true" &&
    brainFilters.ghostView.ghosts === 3 &&
    brainFilters.searchAsked === brainFilters.before &&
    brainFilters.ghostView.nodes > 3 &&
    brainFilters.tagged.nodes === brainFilters.all &&
    brainFilters.tagged.matches > 0 &&
    brainFilters.tagged.matches < brainFilters.all &&
    brainFilters.tagged.query === "core" &&
    brainFilters.remembered === brainFilters.restoredPlace &&
    brainFilters.asksAfterLocalFilters === brainFilters.before &&
    brainFilters.withSources.asked &&
    brainFilters.withSources.asks === brainFilters.before + 1 &&
    brainFilters.withSources.sources === 5 &&
    brainFilters.withSources.shape === "diamond" &&
    brainFilters.restored === 15,
  JSON.stringify(brainFilters),
);

// Test 3: Typed relations test
const brainTyped = await page.evaluate(async () => {
  const viewOf = () => [...document.querySelectorAll(".file-view")]
    .find((one) => one.classList.contains("knowledge-view") && !one.hidden);
  const flag = (name) => viewOf().querySelector(`[data-knowledge-flag="${name}"]`);
  knowledgeQuery = "";
  knowledgeTagsPicked.clear();
  window.__VAULT__ = { pages: 40, linksPer: 2, ghosts: 3,
    tags: ["core", "reading", "tools", "design", "systems"], allTyped: true };
  dropTab("knowledge");
  document.getElementById("nav-knowledge").click();
  await new Promise((done) => setTimeout(done, 400));
  const tally = () => ({
    nodes: viewOf().querySelectorAll(".knowledge-node").length,
    edges: viewOf().querySelectorAll(".knowledge-edge").length,
    typed: viewOf().querySelectorAll(".knowledge-edge.is-typed").length,
    named: viewOf().querySelectorAll(".knowledge-edge.kind-related").length,
  });
  const before = tally();
  const asks = window.__GRAPH_ASKS__;
  const strokeOf = (selector) => {
    const line = viewOf().querySelector(selector);
    if (!line) return { width: "0", ink: "" };
    const style = getComputedStyle(line);
    return { width: style.strokeWidth, ink: style.stroke };
  };
  const plainInk = strokeOf(".knowledge-edge:not(.is-typed):not(.is-ghost)");
  /* 본문 링크의 잉크는 이제 둘이다(09-16): 군집 **안**의 실은 그 주제의 색을 옅게
     띠고, 주제를 **건너는** 실은 더 옅은 중립색이다. 둘 다 타입 관계보다 옅다. */
  const intraInk = strokeOf(".knowledge-edge.is-intra:not(.is-typed):not(.is-ghost)");
  const interInk = strokeOf(".knowledge-edge.is-inter:not(.is-typed):not(.is-ghost)");
  const typedInk = strokeOf(".knowledge-edge.is-typed");
  const colorProbe = document.createElement("canvas");
  colorProbe.width = 1;
  colorProbe.height = 1;
  const colorBrush = colorProbe.getContext("2d", { willReadFrequently: true });
  colorBrush.clearRect(0, 0, 1, 1);
  colorBrush.fillStyle = plainInk.ink;
  colorBrush.fillRect(0, 0, 1, 1);
  const alphaOf = (ink) => {
    colorBrush.clearRect(0, 0, 1, 1);
    colorBrush.fillStyle = ink;
    colorBrush.fillRect(0, 0, 1, 1);
    return colorBrush.getImageData(0, 0, 1, 1).data[3] / 255;
  };
  const plainAlpha = colorBrush.getImageData(0, 0, 1, 1).data[3] / 255;
  const intraAlpha = alphaOf(intraInk.ink);
  const interAlpha = alphaOf(interInk.ink);
  const typedAlpha = alphaOf(typedInk.ink);
  const relationStyles = Object.fromEntries(
    ["related", "implements", "depends_on", "supersedes", "contradicts"].map((kind) => {
      const edge = viewOf().querySelector(`.knowledge-edge.kind-${kind}`);
      const style = getComputedStyle(edge);
      return [kind, `${style.stroke}|${style.strokeDasharray}|${edge.getAttribute("marker-end") ?? ""}`];
    }),
  );
  const arrows = Object.fromEntries(["implements", "depends_on", "supersedes", "contradicts"]
    .map((kind) => [kind, viewOf().querySelector(`.knowledge-edge.kind-${kind}`)
      ?.getAttribute("marker-end") ?? ""]));
  const relatedArrow = viewOf().querySelector(".knowledge-edge.kind-related")
    ?.getAttribute("marker-end") ?? "";
  const contradictsDash = getComputedStyle(
    viewOf().querySelector(".knowledge-edge.kind-contradicts"),
  ).strokeDasharray;
  const markerScreenSize = () => {
    const view = viewOf();
    const layout = knowledgeLayouts.get(view);
    const marker = view.querySelector("#knowledge-arrow-implements");
    return {
      units: marker?.getAttribute("markerUnits") ?? "",
      px: Number(marker?.getAttribute("markerWidth") ?? 0) * layout.scale,
    };
  };
  const markerAtFit = markerScreenSize();
  viewOf().querySelector(".knowledge-zoom-in").click();
  await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
  const markerAfterZoom = markerScreenSize();
  const line = viewOf().querySelector(".knowledge-edge.kind-implements");
  const numbers = (line?.getAttribute("d") ?? "").match(/-?\d+(?:\.\d+)?/g)?.map(Number) ?? [];
  const edgeSeat = line
    ? [...viewOf().querySelector(".knowledge-edges").children].indexOf(line.parentElement)
    : -1;
  const targetKey = line ? knowledgeLayouts.get(viewOf()).model.keys[
    knowledgeLayouts.get(viewOf()).model.to[edgeSeat]
  ] : "";
  const target = [...viewOf().querySelectorAll(".knowledge-node")]
    .find((node) => node.dataset.graphKey === targetKey);
  const center = (target?.getAttribute("transform") ?? "")
    .match(/translate\((-?\d+(?:\.\d+)?) (-?\d+(?:\.\d+)?)\)/)?.slice(1).map(Number) ?? [];
  const targetRadius = Number(target?.querySelector(".knowledge-dot")?.getAttribute("r") ?? 0);
  const endpointGapPx = numbers.length === 4 && center.length === 2
    ? Math.hypot(numbers[2] - center[0], numbers[3] - center[1])
      * knowledgeLayouts.get(viewOf()).scale
    : 0;

  /* 들어간 배율(fit 대비 `--knowledge-cluster-label-until`)에서는 이름표와 성운이
   * 함께 물러난다 — 확대된 성운은 화면 전체를 덮는 그라데이션이라 값도 없고 프레임도
   * 먹는다(실측 265ms). 성운은 `display: none`이어야 한다(투명한 원도 페인트 목록에 남는다). */
  const zoomedIn = await (async () => {
    const view = viewOf();
    const layout = knowledgeLayouts.get(view);
    for (let press = 0; press < 40 && layout.zoom < layout.tuning.clusterLabelUntil; press += 1) {
      view.querySelector(".knowledge-zoom-in").click();
    }
    await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
    const nebula = view.querySelector(".knowledge-nebula");
    const label = view.querySelector(".knowledge-cluster-label");
    return {
      zoom: knowledgeLayouts.get(view).zoom,
      marked: view.querySelector(".knowledge-picture").classList.contains("is-zoomed-in"),
      nebulaDisplay: nebula ? getComputedStyle(nebula).display : "none",
      /* 불투명도는 전이 중이라 두 프레임 뒤에 0.89로 읽힌다; 물러남의 즉시 증거는
       * 함께 꺼지는 pointer-events다(들어간 배율에서 이름표는 클릭할 수 없다). */
      labelPointer: label ? getComputedStyle(label).pointerEvents : "none",
    };
  })();

  /* 「전체 보기」는 난다(3차) — 그 비행이 아직 걷는 동안 렌즈가 판을 갈아 끼운다.
   * 옛 판에 묶인 걸음이 지워진 선을 도로 그리면 아래의 `only.edges`가 그것을 잡는다
   * (window.mjs가 먼저 잡았던 결함). */
  viewOf().querySelector(".knowledge-zoom-fit").click();
  flag("typed").click();
  await new Promise((done) => setTimeout(done, 80));
  const only = tally();
  const pressed = flag("typed").getAttribute("aria-pressed");
  flag("typed").click();
  await new Promise((done) => setTimeout(done, 80));
  const restored = tally();
  const asksAfterTyped = window.__GRAPH_ASKS__;
  /* 관계 낱말은 검색어다(3차): `contradicts`를 치면 그 관계의 양 끝이 밝는다. */
  const find = viewOf().querySelector(".knowledge-query");
  find.value = "contradicts";
  find.dispatchEvent(new Event("input", { bubbles: true }));
  await new Promise((done) => setTimeout(done, 60));
  const wordMatches = viewOf().querySelectorAll(".knowledge-node.is-search-match").length;
  find.value = "";
  find.dispatchEvent(new Event("input", { bubbles: true }));
  await new Promise((done) => setTimeout(done, 60));

  window.__VAULT__ = { pages: 40, linksPer: 2, ghosts: 3,
    tags: ["core", "reading", "tools", "design", "systems"], allTyped: true, untyped: true };
  dropTab("knowledge");
  document.getElementById("nav-knowledge").click();
  await new Promise((done) => setTimeout(done, 400));
  const old = tally();
  const oldChipHidden = flag("typed").hidden;
  window.__VAULT__ = { pages: 40, linksPer: 2, ghosts: 3,
    tags: ["core", "reading", "tools", "design", "systems"], allTyped: true };
  return { before, only, pressed, restored, old, oldChipHidden, plainInk, typedInk,
    arrows, relatedArrow, contradictsDash, markerAtFit, markerAfterZoom, endpointGapPx, zoomedIn,
    targetRadius, plainAlpha, intraAlpha, interAlpha, typedAlpha,
    relationStyles, asks, asksAfterTyped, wordMatches };
});
ok(
  "typed relations use directional markers and the typed lens owns membership",
  brainTyped.before.typed >= 5 &&
    brainTyped.before.named >= 1 &&
    brainTyped.before.edges > brainTyped.before.typed &&
    parseFloat(brainTyped.plainInk.width) === 1 &&
    parseFloat(brainTyped.typedInk.width) === 1 &&
    // 본문 링크는 옅고, 그중에서도 주제를 건너는 것이 가장 옅다 — 주제 사이에서
    // 읽혀야 하는 것은 프론트매터가 이름을 준 관계다(09-16). 하나의 수였던 옛
    // 계약(0.28)이 두 수가 된 자리.
    brainTyped.intraAlpha > brainTyped.interAlpha &&
    brainTyped.typedAlpha > brainTyped.intraAlpha &&
    brainTyped.interAlpha > 0 &&
    brainTyped.intraAlpha < 0.5 &&
    brainTyped.wordMatches >= 2 &&
    brainTyped.typedInk.ink !== brainTyped.plainInk.ink &&
    new Set(Object.values(brainTyped.relationStyles)).size === 5 &&
    brainTyped.only.edges === brainTyped.before.typed &&
    brainTyped.only.typed === brainTyped.before.typed &&
    brainTyped.only.nodes < brainTyped.before.nodes &&
    brainTyped.only.nodes >= 8 &&
    brainTyped.pressed === "true" &&
    brainTyped.restored.edges === brainTyped.before.edges &&
    brainTyped.restored.nodes === brainTyped.before.nodes &&
    brainTyped.asksAfterTyped === brainTyped.asks &&
    // 옛 페이로드 라운드는 위와 같은 마흔 페이지 볼트를 `kind` 없이 다시 답한
    // 것이다 — 그래서 점의 수는 그대로이고, 달라지는 것은 선의 이름뿐이다.
    brainTyped.old.nodes === brainTyped.before.nodes &&
    brainTyped.old.edges === brainTyped.before.edges &&
    brainTyped.old.typed === 0 &&
    brainTyped.oldChipHidden === true &&
    Object.values(brainTyped.arrows).every((marker) => marker.startsWith("url(")) &&
    brainTyped.relatedArrow === "" &&
    brainTyped.markerAtFit.units === "userSpaceOnUse" &&
    brainTyped.markerAtFit.px >= 6 &&
    brainTyped.markerAtFit.px <= 8 &&
    Math.abs(brainTyped.markerAtFit.px - brainTyped.markerAfterZoom.px) < 0.2 &&
    brainTyped.zoomedIn.marked === true &&
    brainTyped.zoomedIn.nebulaDisplay === "none" &&
    brainTyped.zoomedIn.labelPointer === "none" &&
    brainTyped.endpointGapPx >= brainTyped.targetRadius - 0.5 &&
    brainTyped.contradictsDash !== "none" &&
    brainTyped.contradictsDash !== "0px",
  JSON.stringify(brainTyped),
);

// Test 4: Hover interaction test (Adversarial mutation write count verification)
const brainHover = await page.evaluate(async () => {
  const view = [...document.querySelectorAll(".file-view")]
    .find((one) => one.classList.contains("knowledge-view") && !one.hidden);
  // 앉는 중의 프레임은 LOD 클래스를 뒤집을 수 있다 — 호버의 쓰기만 세려면 먼저 앉힌다.
  for (let round = 0; round < 600 && (knowledgeLayouts.get(view)?.left ?? 0) > 0; round += 1) {
    await new Promise((done) => requestAnimationFrame(done));
  }
  const canvas = view.querySelector(".knowledge-canvas");
  const picture = view.querySelector(".knowledge-picture");
  const node = [...view.querySelectorAll(".knowledge-node")]
    .find((one) => !one.classList.contains("is-ghost")
      && Number(one.dataset.graphSeat) === 0);
  const touched = [];
  const watch = new MutationObserver((rows) => {
    for (const row of rows) touched.push(row.target.getAttribute("class") ?? "");
  });
  watch.observe(picture, { attributes: true, attributeFilter: ["class"], subtree: true });
  node.dispatchEvent(new PointerEvent("pointermove", { bubbles: true }));
  await new Promise((done) => requestAnimationFrame(done));
  const lit = {
    tracing: picture.classList.contains("is-tracing"),
    nodes: view.querySelectorAll(".knowledge-node.is-lit").length,
    edges: view.querySelectorAll(".knowledge-edge.is-lit").length,
    self: node.classList.contains("is-lit"),
  };
  const far = view.querySelector(".knowledge-node:not(.is-lit)");
  const dimmed = getComputedStyle(far).opacity;
  const litWrites = touched.length;
  return {
    ...lit,
    litWrites,
    dimmed,
  };
});

// Take screenshot during active tracing/hover
await page.screenshot({ path: join(UI, "..", "output/playwright/knowledge-graph-hover.png") });

const brainHoverRelease = await page.evaluate(async () => {
  const view = [...document.querySelectorAll(".file-view")]
    .find((one) => one.classList.contains("knowledge-view") && !one.hidden);
  const canvas = view.querySelector(".knowledge-canvas");
  const picture = view.querySelector(".knowledge-picture");
  const far = view.querySelector(".knowledge-node:not(.is-lit)");
  const touched = [];
  const watch = new MutationObserver((rows) => {
    for (const row of rows) touched.push(row.target.getAttribute("class") ?? "");
  });
  watch.observe(picture, { attributes: true, attributeFilter: ["class"], subtree: true });
  canvas.dispatchEvent(new PointerEvent("pointerleave", { bubbles: true }));
  await new Promise((done) => requestAnimationFrame(done));
  watch.disconnect();
  return {
    unlitWrites: touched.length,
    plain: getComputedStyle(far).opacity,
    after: {
      tracing: picture.classList.contains("is-tracing"),
      nodes: view.querySelectorAll(".knowledge-node.is-lit").length,
      edges: view.querySelectorAll(".knowledge-edge.is-lit").length,
    },
  };
});
/* 점을 쥐고 끈다(3차). 쥔 점은 포인터를 따르고 이웃은 스프링으로 따라오며, 놓은
 * 자리는 기억되고, 끈 몸짓의 click은 고르기가 되지 않는다. */
const brainDrag = await page.evaluate(async () => {
  const view = [...document.querySelectorAll(".file-view")]
    .find((one) => one.classList.contains("knowledge-view") && !one.hidden);
  const layout = knowledgeLayouts.get(view);
  const canvas = view.querySelector(".knowledge-canvas");
  const node = [...view.querySelectorAll(".knowledge-node.is-page")]
    .find((one) => layout.model.degree[Number(one.dataset.graphSeat)] > 0);
  const seat = Number(node.dataset.graphSeat);
  const key = layout.model.keys[seat];
  const neighbour = layout.model.neighbour[layout.model.start[seat]];
  const before = {
    x: layout.x[seat],
    neighbourX: layout.x[neighbour],
    selected: knowledgeSelectedKey,
  };
  const dot = node.querySelector(".knowledge-dot");
  const box = dot.getBoundingClientRect();
  const startX = box.left + box.width / 2;
  const startY = box.top + box.height / 2;
  const pointer = (type, target, clientX, clientY) => target.dispatchEvent(
    new PointerEvent(type, { bubbles: true, button: 0, clientX, clientY, pointerId: 1 }),
  );
  pointer("pointerdown", dot, startX, startY);
  pointer("pointermove", window, startX + 60, startY);
  await new Promise((done) => requestAnimationFrame(done));
  const midway = {
    pinned: layout.pinned,
    dragging: canvas.classList.contains("is-dragging"),
    panned: canvas.classList.contains("is-panning"),
    x: layout.x[seat],
  };
  pointer("pointerup", window, startX + 60, startY);
  canvas.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  for (let round = 0; round < 600 && (knowledgeLayouts.get(view)?.left ?? 0) > 0; round += 1) {
    await new Promise((done) => requestAnimationFrame(done));
  }
  return {
    seat,
    before,
    midway,
    movedPx: (midway.x - before.x) * layout.scale,
    after: {
      pinned: layout.pinned,
      dragging: canvas.classList.contains("is-dragging"),
      neighbourMoved: layout.x[neighbour] !== before.neighbourX,
      selected: knowledgeSelectedKey,
      remembered: layout.placed.get(key)?.[0] === layout.x[seat],
      dragged: layout.dragged,
    },
  };
});
const brainFocus = await page.evaluate(async () => {
  const view = [...document.querySelectorAll(".file-view")]
    .find((one) => one.classList.contains("knowledge-view") && !one.hidden);
  const layout = knowledgeLayouts.get(view);
  const seat = 0;
  const key = layout.model.keys[seat];
  const expectedAt = (depth) => {
    const visited = new Uint8Array(layout.count);
    const queue = new Int32Array(layout.count);
    const level = new Int32Array(layout.count);
    let read = 0;
    let write = 1;
    queue[0] = seat;
    visited[seat] = 1;
    while (read < write) {
      const here = queue[read++];
      if (level[here] >= depth) continue;
      for (let at = layout.model.start[here]; at < layout.model.start[here + 1]; at += 1) {
        const next = layout.model.neighbour[at];
        if (visited[next]) continue;
        visited[next] = 1;
        level[next] = level[here] + 1;
        queue[write++] = next;
      }
    }
    let edges = 0;
    for (let at = 0; at < layout.model.edgeCount; at += 1) {
      if (visited[layout.model.from[at]] && visited[layout.model.to[at]]) edges += 1;
    }
    return { nodes: write, edges };
  };
  const creations = knowledgeNodeCreations;
  selectKnowledgeNode(view, key);
  const depths = [];
  for (const depth of [1, 2, 3]) {
    view.querySelector(`[data-knowledge-depth="${depth}"]`)?.click();
    await new Promise((done) => requestAnimationFrame(done));
    depths.push({
      depth,
      expected: expectedAt(depth),
      nodes: view.querySelectorAll(".knowledge-node.is-focus-lit").length,
      edges: view.querySelectorAll(".knowledge-edge.is-focus-lit").length,
    });
  }
  for (let at = 0; at < 100; at += 1) selectKnowledgeNode(view, key);
  await new Promise((done) => setTimeout(done, 200));
  const selected = view.querySelector(".knowledge-node.is-selected");
  const focusedEdge = view.querySelector(".knowledge-edge.is-focus-lit");
  return {
    key,
    selected: knowledgeSelectedKey,
    depths,
    creations: knowledgeNodeCreations - creations,
    focused: view.querySelector(".knowledge-picture").classList.contains("is-focused"),
    selectedHaloDisplay: getComputedStyle(selected.querySelector(".knowledge-halo")).display,
    selectedDotAnimation: getComputedStyle(selected.querySelector(".knowledge-dot")).animationName,
    focusFlow: getComputedStyle(focusedEdge).animationName,
  };
});
await page.screenshot({ path: join(UI, "..", "output/playwright/knowledge-graph-focus.png") });
const brainFocusClear = await page.evaluate(async () => {
  const view = [...document.querySelectorAll(".file-view")]
    .find((one) => one.classList.contains("knowledge-view") && !one.hidden);
  const canvas = view.querySelector(".knowledge-canvas");
  canvas.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  await new Promise((done) => requestAnimationFrame(done));
  const escape = knowledgeSelectedKey === null
    && !view.querySelector(".knowledge-picture").classList.contains("is-focused");
  selectKnowledgeNode(view, knowledgeLayouts.get(view).model.keys[0]);
  canvas.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  await new Promise((done) => requestAnimationFrame(done));
  return { escape, blank: knowledgeSelectedKey === null };
});
ok(
  "hover is one hop and focus depths use the reusable CSR without rebuilding nodes",
  brainHover.tracing &&
    brainHover.self &&
    brainHover.nodes > 1 &&
    brainHover.edges > 0 &&
    brainHover.litWrites === brainHover.nodes + brainHover.edges + 1 &&
    brainHoverRelease.unlitWrites === brainHover.litWrites &&
    brainHover.dimmed !== brainHoverRelease.plain &&
    brainHoverRelease.after.tracing === false &&
    brainHoverRelease.after.nodes === 0 &&
    brainHoverRelease.after.edges === 0 &&
    brainFocus.selected === brainFocus.key &&
    brainFocus.focused &&
    brainFocus.creations === 0 &&
    brainFocus.selectedHaloDisplay === "none" &&
    brainFocus.selectedDotAnimation === "none" &&
    brainFocus.focusFlow === "none" &&
    brainFocus.depths.every((row) => row.nodes === row.expected.nodes
      && row.edges === row.expected.edges) &&
    brainFocusClear.escape &&
    brainFocusClear.blank &&
    brainDrag.midway.pinned === brainDrag.seat &&
    brainDrag.midway.dragging &&
    !brainDrag.midway.panned &&
    Math.abs(brainDrag.movedPx - 60) < 1 &&
    brainDrag.after.pinned === -1 &&
    !brainDrag.after.dragging &&
    brainDrag.after.neighbourMoved &&
    brainDrag.after.selected === brainDrag.before.selected &&
    brainDrag.after.remembered &&
    !brainDrag.after.dragged,
  JSON.stringify({ hover: brainHover, release: brainHoverRelease, drag: brainDrag,
    focus: brainFocus, clear: brainFocusClear }),
);

// Test 5: The inspector has stable overview/page-card states; opening is explicit.
const brainOpen = await page.evaluate(async () => {
  knowledgeSelectedKey = null;
  window.__VAULT__ = { pages: 12, linksPer: 2, ghosts: 3, tags: ["core", "reading"],
    allTyped: true };
  dropTab("knowledge");
  document.getElementById("nav-knowledge").click();
  await new Promise((done) => setTimeout(done, 400));
  const heldDisk = window.__ANSWER__.read_text_file;
  window.__ANSWER__.read_text_file = (args) => (args.path?.startsWith("/vault/")
    ? { text: `# ${args.path}\n\n[[Page-0001]]\n`, version: "1:1" }
    : heldDisk?.(args) ?? null);
  const viewOf = () => [...document.querySelectorAll(".file-view")]
    .find((one) => one.classList.contains("knowledge-view") && !one.hidden);
  const view = viewOf();
  const panel = view.querySelector(".knowledge-inspector");
  const overview = {
    tiles: panel.querySelectorAll(".knowledge-stat-tile").length,
    tileHierarchy: (() => {
      const tile = panel.querySelector(".knowledge-stat-tile");
      const value = tile?.querySelector("dd");
      const label = tile?.querySelector("dt");
      return value && label
        ? parseFloat(getComputedStyle(value).fontSize) > parseFloat(getComputedStyle(label).fontSize)
        : false;
    })(),
    hubs: panel.querySelectorAll(".knowledge-overview-hubs button").length,
    tags: panel.querySelectorAll(".knowledge-overview-tags button").length,
    kinds: panel.querySelectorAll(".knowledge-overview-kinds li").length,
    recent: panel.querySelectorAll(".knowledge-overview-recent button").length,
    /* 볼트 건강: 카파시의 lint 목록이 백엔드의 표 그대로 줄로 선다 — 여덟 줄, 수는
     * 픽스처가 지은 `graph.lint.counts`의 것이고 창은 하나도 세지 않는다. allTyped
     * 픽스처는 모순 하나·대체된 페이지 하나·고아 하나(외로운 4번: 나가는 선은
     * 매었지만 아무도 가리키지 않는다)를 만든다. */
    health: [...panel.querySelectorAll(".knowledge-health-row")].map((row) =>
      `${row.dataset.knowledgeLint}=${row.querySelector(".knowledge-inspector-note").textContent}`),
    healthExpected: (() => {
      const counts = window.__buildVaultGraph__({ path: "/vault" }, window.__VAULT__).graph.lint.counts;
      return KNOWLEDGE_HEALTH_ROWS.map((row) => `${row.lint}=${counts[row.count] ?? 0}`);
    })(),
    healthDoorless: [...panel.querySelectorAll("button[data-knowledge-lint]:disabled")]
      .map((press) => press.dataset.knowledgeLint),
    orphanTile: panel.querySelector('[data-knowledge-count="orphans"]')?.textContent,
    clusters: panel.querySelectorAll(".knowledge-overview-clusters button").length,
    meters: [...panel.querySelectorAll(
      ".knowledge-overview-hubs li, .knowledge-overview-tags li, .knowledge-overview-kinds li, .knowledge-overview-recent li",
    )].every((row) => {
      const value = Number.parseFloat(row.style.getPropertyValue("--meter-fill"));
      return row.classList.contains("knowledge-meter") && value > 0 && value <= 1;
    }),
  };
  panel.querySelector(".knowledge-overview-hubs button")?.click();
  await new Promise((done) => requestAnimationFrame(done));
  const hubMovedSelection = knowledgeSelectedKey !== null;
  /* 이름 있는 군집의 페이지(순위 0은 이름 있는 군집이 하나라도 있으면 늘 그것이다)
   * — 카드의 군집 줄이 서는지 볼 수 있는 페이지. */
  const pages = [...viewOf().querySelectorAll(".knowledge-node")]
    .filter((one) => !one.classList.contains("is-ghost"));
  const page = pages.find((one) => one.dataset.community === "0") ?? pages[0];
  const key = page.dataset.graphKey;
  window.__GRAPH_PAGE_ASKED__ = null;
  page.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  await new Promise((done) => setTimeout(done, 80));
  const askedOnSelect = window.__GRAPH_PAGE_ASKED__;
  const card = {
    excerpt: panel.querySelector(".knowledge-inspector-excerpt")?.textContent ?? "",
    outgoing: panel.querySelectorAll(".knowledge-relations-outgoing button").length,
    incoming: panel.querySelectorAll(".knowledge-relations-incoming button").length,
    backlinks: panel.querySelector(".knowledge-backlinks")?.textContent ?? "",
    folder: panel.querySelector(".knowledge-inspector-folder")?.textContent ?? "",
    /* 깊이의 라디오 그룹은 캔버스 위 빵부스러기 아래에 산다(시안 이식). */
    radiogroup: viewOf().querySelector('.knowledge-depth[role="radiogroup"]')?.getAttribute("aria-label") ?? "",
    center: !!panel.querySelector(".knowledge-inspector-center"),
    clusterShown: panel.querySelector(".knowledge-inspector-cluster")?.hidden === false,
  };
  /* 밝히기(3차): 군집 줄을 누르면 그 군집만 밝고 나머지는 물러선다 — 멤버십은
   * 그대로, 카메라는 그 군집으로 난다. 다시 누르면 놓는다. */
  const clusterRow = panel.querySelector(".knowledge-overview-clusters button");
  clusterRow?.click();
  await new Promise((done) => requestAnimationFrame(done));
  const spot = {
    on: view.querySelector(".knowledge-picture").classList.contains("is-spotlight"),
    lit: view.querySelectorAll(".knowledge-node.is-spotlit").length,
    nodes: view.querySelectorAll(".knowledge-node").length,
    active: clusterRow?.classList.contains("is-active") ?? false,
    flying: knowledgeLayouts.get(view).flight !== null,
  };
  clusterRow?.click();
  await new Promise((done) => requestAnimationFrame(done));
  spot.off = !view.querySelector(".knowledge-picture").classList.contains("is-spotlight");
  const nativeReplace = panel.replaceChildren.bind(panel);
  let replacements = 0;
  panel.replaceChildren = (...args) => {
    replacements += 1;
    return nativeReplace(...args);
  };
  selectKnowledgeNode(view, key);
  selectKnowledgeNode(view, key);
  panel.replaceChildren = nativeReplace;
  panel.querySelector(".knowledge-inspector-open")?.click();
  await new Promise((done) => setTimeout(done, 160));
  const opened = tabs.map((tab) => tab.id);

  document.getElementById("nav-knowledge").click();
  await new Promise((done) => setTimeout(done, 100));
  const ghost = [...viewOf().querySelectorAll(".knowledge-node.is-ghost")][0];
  selectKnowledgeNode(viewOf(), ghost.dataset.graphKey);
  const ghostCard = {
    saysMissing: panel.textContent.includes("아직 없는"),
    incoming: panel.querySelectorAll(".knowledge-relations-incoming button").length,
  };
  dropTab(`file:/vault/${key}`);
  return {
    key,
    askedId: window.__GRAPH_PAGE_ASKED__?.id ?? null,
    askedPath: window.__GRAPH_PAGE_ASKED__?.path ?? null,
    askedOnSelect,
    opened,
    overview,
    hubMovedSelection,
    card,
    spot,
    replacements,
    ghostCard,
  };
});
ok(
  "the inspector switches from vault overview to stable page cards with an explicit open action",
  brainOpen.askedId === brainOpen.key &&
    brainOpen.askedPath === "/vault" &&
    brainOpen.askedOnSelect === null &&
    brainOpen.opened.includes(`file:/vault/${brainOpen.key}`) &&
    brainOpen.overview.hubs === 5 &&
    brainOpen.overview.tiles === 4 &&
    brainOpen.overview.tileHierarchy &&
    brainOpen.overview.meters &&
    brainOpen.overview.tags === 2 &&
    brainOpen.overview.kinds >= 2 &&
    brainOpen.overview.recent === 5 &&
    // 카파시의 lint 여덟 줄, 근거 없는 간선 한 줄(t-5966), 라이브 층의 셋(t-2931) —
    // 열두 줄, 수는 픽스처가 지은 `graph.lint.counts`의 것(라이브 층이 없는 답에서
    // 셋은 0)이고 창은 하나도 세지 않는다.
    brainOpen.overview.health.length === 12 &&
    JSON.stringify(brainOpen.overview.health) === JSON.stringify(brainOpen.overview.healthExpected) &&
    brainOpen.overview.health.includes("orphans=1") &&
    brainOpen.overview.orphanTile === "1" &&
    brainOpen.overview.health.includes("ghosts=3") &&
    brainOpen.overview.health.includes("index_gaps=12") &&
    brainOpen.overview.health.includes("contradictions=1") &&
    brainOpen.overview.health.includes("superseded=1") &&
    JSON.stringify(brainOpen.overview.healthDoorless) === JSON.stringify(["unlogged_raw", "unsourced_edges"]) &&
    brainOpen.overview.health.includes("recalledToday=0") &&
    brainOpen.overview.health.includes("neverRecalled=0") &&
    brainOpen.overview.health.includes("merge=0") &&
    brainOpen.overview.clusters >= 1 &&
    brainOpen.spot.on &&
    brainOpen.spot.lit > 0 &&
    brainOpen.spot.lit < brainOpen.spot.nodes &&
    brainOpen.spot.nodes === 15 &&
    brainOpen.spot.active &&
    brainOpen.spot.flying &&
    brainOpen.spot.off &&
    brainOpen.card.clusterShown === (brainOpen.overview.clusters > 0) &&
    brainOpen.hubMovedSelection &&
    brainOpen.card.excerpt.includes("첫 문단") &&
    brainOpen.card.outgoing > 0 &&
    brainOpen.card.backlinks.includes("0") &&
    brainOpen.card.folder.includes("topics") &&
    brainOpen.card.radiogroup !== "" &&
    brainOpen.card.center &&
    brainOpen.replacements === 0 &&
    brainOpen.ghostCard.saysMissing &&
    brainOpen.ghostCard.incoming > 0,
  JSON.stringify(brainOpen),
);

/* 건강 카드의 줄은 문이다(3차): 고아는 렌즈(멤버십), 모순은 관계 낱말 검색(디밍만).
 * 다시 누르면 놓는다. */
const brainLint = await page.evaluate(async () => {
  const viewOf = () => [...document.querySelectorAll(".file-view")]
    .find((one) => one.classList.contains("knowledge-view") && !one.hidden);
  const count = () => viewOf().querySelectorAll(".knowledge-node").length;
  const press = (lint) => viewOf().querySelector(`button[data-knowledge-lint="${lint}"]`);
  const pause = () => new Promise((done) => setTimeout(done, 60));
  /* allTyped 픽스처에는 고아가 없으므로 렌즈의 문은 유령 줄로 본다: 유령 셋과
   * 그것을 가리키는 페이지가 남는다. */
  press("ghosts").click();
  await pause();
  const ghosts = {
    nodes: count(),
    ghostNodes: viewOf().querySelectorAll(".knowledge-node.is-ghost").length,
    on: press("ghosts").classList.contains("is-active"),
    lens: knowledgeGhostsOnly,
  };
  press("ghosts").click();
  await pause();
  press("contradictions").click();
  await pause();
  const contradictions = {
    query: viewOf().querySelector(".knowledge-query").value,
    matches: viewOf().querySelectorAll(".knowledge-node.is-search-match").length,
    nodes: count(),
    on: press("contradictions").classList.contains("is-active"),
  };
  press("contradictions").click();
  await pause();
  /* 표의 줄이 렌즈다: 색인 누락은 합성 볼트의 열두 페이지 전부(index.md가 없다)
   * — 유령 셋은 표에 없으므로 남지 않는다. 다시 누르면 놓는다. */
  press("index_gaps").click();
  await pause();
  const indexGaps = {
    nodes: count(),
    on: press("index_gaps").classList.contains("is-active"),
    lens: knowledgeLintLens,
  };
  press("index_gaps").click();
  await pause();
  /* raw 항목은 그림에 점이 없으니 문이 없다 — 눌러도 아무것도 바뀌지 않는다. */
  press("unlogged_raw").click();
  await pause();
  return {
    ghosts,
    contradictions,
    indexGaps,
    restored: {
      nodes: count(),
      query: viewOf().querySelector(".knowledge-query").value,
      lens: knowledgeGhostsOnly,
      lintLens: knowledgeLintLens,
    },
  };
});
ok(
  "the vault health rows open the missing-page lens, the contradiction search and the index-gap lens, and close them again",
  brainLint.ghosts.nodes > 3 &&
    brainLint.ghosts.nodes < 15 &&
    brainLint.ghosts.ghostNodes === 3 &&
    brainLint.ghosts.on &&
    brainLint.ghosts.lens &&
    brainLint.contradictions.query === "contradicts" &&
    brainLint.contradictions.matches === 2 &&
    brainLint.contradictions.nodes === 15 &&
    brainLint.contradictions.on &&
    brainLint.indexGaps.nodes === 12 &&
    brainLint.indexGaps.on &&
    brainLint.indexGaps.lens === "index_gaps" &&
    brainLint.restored.nodes === 15 &&
    brainLint.restored.query === "" &&
    brainLint.restored.lens === false &&
    brainLint.restored.lintLens === null,
  JSON.stringify(brainLint),
);

/* 레시피와 카드는 한 표를 읽는다: fixtures/vault-lint/expected.json은
 * `zerocode vault-lint`가 픽스처 볼트에 답하는 표이고(코어 시험과 CLI 시험이
 * 못 박는다), 같은 파일을 백엔드의 답으로 실으면 카드의 여덟 줄과 타일 둘이 그
 * 수 그대로 선다 — 창은 아무것도 세지 않았다는 증거. */
const expectedLint = JSON.parse(
  await readFile(resolve(UI, "..", "fixtures/vault-lint/expected.json"), "utf8"),
);
const brainFixture = await page.evaluate(async (lint) => {
  knowledgeSelectedKey = null;
  window.__VAULT__ = { pages: 12, linksPer: 2, ghosts: 3, tags: ["core", "reading"], lint };
  dropTab("knowledge");
  document.getElementById("nav-knowledge").click();
  await new Promise((done) => setTimeout(done, 400));
  const viewOf = () => [...document.querySelectorAll(".file-view")]
    .find((one) => one.classList.contains("knowledge-view") && !one.hidden);
  const panel = viewOf().querySelector(".knowledge-inspector");
  return {
    health: [...panel.querySelectorAll(".knowledge-health-row")].map((row) =>
      `${row.dataset.knowledgeLint}=${row.querySelector(".knowledge-inspector-note").textContent}`),
    expected: KNOWLEDGE_HEALTH_ROWS.map((row) => `${row.lint}=${lint.counts[row.count] ?? 0}`),
    orphanTile: panel.querySelector('[data-knowledge-count="orphans"]')?.textContent,
    ghostTile: panel.querySelector('[data-knowledge-count="ghosts"]')?.textContent,
    // The three live rows (recalled today, never recalled, merge candidates)
    // are painted `is-clean` at zero by design — the fixture vault has no
    // recall trace — so only the lint rows count as "clean" here. Without
    // this filter the test had been red since t-2931 unit 3 added them.
    // The one lint row that is clean on purpose is `unsourced_edges`
    // (t-5966): zero on any scanned vault by construction, and the fixture
    // holds no line the scanner did not write.
    clean: [...panel.querySelectorAll(".knowledge-health-row.is-clean")]
      .map((row) => row.dataset.knowledgeLint)
      .filter((lint) => !KNOWLEDGE_HEALTH_ROWS.find((row) => row.lint === lint)?.live),
  };
}, expectedLint);
ok(
  "the vault health card paints exactly the table `zerocode vault-lint` answers for the fixture vault",
  JSON.stringify(brainFixture.health) === JSON.stringify(brainFixture.expected) &&
    brainFixture.health.includes("index_gaps=2") &&
    brainFixture.health.includes("unlogged_raw=1") &&
    brainFixture.health.includes("unsourced_edges=0") &&
    brainFixture.orphanTile === "1" &&
    brainFixture.ghostTile === "1" &&
    JSON.stringify(brainFixture.clean) === JSON.stringify(["unsourced_edges"]),
  JSON.stringify(brainFixture),
);

// Test 6: Empty Vault Test (Adversarial edge case 0 nodes)
const brainEmpty = await page.evaluate(async () => {
  window.__QUICK__ = [{
    id: "second-brain-abc123-ingest",
    label: "raw 취합",
    workspace: "/vault",
    body: "raw/에 있는 것을 취합해 줘.",
    agent: "claude",
    append_enter: true,
  }];
  window.__ANSWER__.quick_commands = () => window.__QUICK__;
  window.__VAULT__ = { pages: 0 };
  dropTab("knowledge");
  document.getElementById("nav-knowledge").click();
  await new Promise((done) => setTimeout(done, 300));
  const view = [...document.querySelectorAll(".file-view")]
    .find((one) => one.classList.contains("knowledge-view") && !one.hidden);
  const empty = view.querySelector(".knowledge-empty");
  const act = empty.querySelector(".knowledge-empty-act");
  window.__LAUNCHED__ = null;
  act.click();
  await new Promise((done) => setTimeout(done, 150));
  return {
    hidden: empty.hidden,
    word: empty.querySelector(".knowledge-empty-word").textContent,
    act: act.textContent,
    nodes: view.querySelectorAll(".knowledge-node").length,
    gridOff: view.querySelector(".knowledge-canvas").classList.contains("is-empty"),
    ranPrompt: window.__LAUNCHED__?.prompt ?? null,
    ranAgent: window.__LAUNCHED__?.agent ?? null,
  };
});
ok(
  "an empty vault says what to do and its button runs the vault's own ingest command",
  brainEmpty.hidden === false &&
    brainEmpty.word.includes("raw/") &&
    brainEmpty.act === "raw 취합" &&
    brainEmpty.nodes === 0 &&
    brainEmpty.gridOff &&
    brainEmpty.ranAgent === "claude" &&
    brainEmpty.ranPrompt.includes("취합"),
  JSON.stringify(brainEmpty),
);

// Test 7: Four container-width tiers keep every destination in the DOM.
await page.evaluate(async () => {
  window.__VAULT__ = { pages: 12, linksPer: 2, ghosts: 3,
    tags: ["core", "reading", "tools", "design", "systems", "notes", "ideas", "archive"] };
  dropTab("knowledge");
  document.getElementById("nav-knowledge").click();
  await new Promise((done) => setTimeout(done, 350));
});
const brainResponsive = [];
for (const width of [1280, 800, 560, 280]) {
  brainResponsive.push(await page.evaluate(async (wide) => {
    const view = [...document.querySelectorAll(".file-view")]
      .find((one) => one.classList.contains("knowledge-view") && !one.hidden);
    view.style.width = `${wide}px`;
    view.style.maxWidth = `${wide}px`;
    view.style.flex = "0 0 auto";
    await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
    const surface = view.querySelector(".knowledge-surface");
    const inspector = view.querySelector(".knowledge-inspector");
    const surfaceBox = surface?.getBoundingClientRect();
    const inspectorBox = inspector.getBoundingClientRect();
    const toolbarTops = [...view.querySelectorAll(
      ".knowledge-head input, .knowledge-head button, .knowledge-head .knowledge-stat",
    )].filter((node) => node.offsetParent !== null)
      .map((node) => {
        const box = node.getBoundingClientRect();
        return Math.round((box.top + box.bottom) / 2);
      });
    return {
      width: wide,
      actual: Math.round(view.getBoundingClientRect().width),
      tier: ["wide", "middle", "compact", "tiny"]
        .find((name) => view.classList.contains(`is-tier-${name}`)) ?? "",
      sideBySide: !!surfaceBox && inspectorBox.left >= surfaceBox.right - 1,
      stacked: !!surfaceBox && inspectorBox.top >= surfaceBox.bottom - 1,
      lensToggle: !!view.querySelector(".knowledge-lens-toggle")
        && view.querySelector(".knowledge-lens-toggle").offsetParent !== null,
      legendToggle: !!view.querySelector(".knowledge-legend-toggle")
        && view.querySelector(".knowledge-legend-toggle").offsetParent !== null,
      compactStat: !!view.querySelector(".knowledge-stat-compact")
        && view.querySelector(".knowledge-stat-compact").offsetParent !== null,
      toolbarRows: new Set(toolbarTops).size,
      /* 태그 칩은 「보기」 메뉴 안에 산다(시안 이식) — 열고 세고 닫는다. */
      visibleTags: (() => {
        const menu = view.querySelector(".knowledge-lens-popover");
        menu.classList.add("is-open");
        const shown = [...view.querySelectorAll(".knowledge-chips .knowledge-chip")]
          .filter((node) => node.offsetParent !== null).length;
        menu.classList.remove("is-open");
        return shown;
      })(),
      moreTags: view.querySelector(".knowledge-tags-more-toggle")?.textContent ?? "",
      destinations: [".knowledge-inspector", ".knowledge-legend", ".knowledge-lens-popover"]
        .every((selector) => view.querySelector(selector)?.isConnected),
      noOverflow: view.scrollWidth <= view.clientWidth,
    };
  }, width));
  if (width === 560) {
    await page.screenshot({ path: join(UI, "..", "output/playwright/knowledge-graph-narrow.png") });
  }
}
await page.evaluate(() => {
  const view = [...document.querySelectorAll(".file-view")]
    .find((one) => one.classList.contains("knowledge-view") && !one.hidden);
  view.style.width = "";
  view.style.maxWidth = "";
  view.style.flex = "";
});
ok(
  "container tiers stack, compact and pop over without overflow or deleting destinations",
  brainResponsive[0].tier === "wide" &&
    brainResponsive[0].sideBySide &&
    brainResponsive[0].toolbarRows === 1 &&
    brainResponsive[0].visibleTags === 6 &&
    brainResponsive[0].moreTags === "+2" &&
    brainResponsive[1].tier === "middle" &&
    brainResponsive[1].stacked &&
    brainResponsive[1].lensToggle &&
    brainResponsive[2].tier === "compact" &&
    brainResponsive[2].stacked &&
    brainResponsive[2].lensToggle &&
    brainResponsive[2].compactStat &&
    brainResponsive[3].tier === "tiny" &&
    brainResponsive[3].legendToggle &&
    brainResponsive.every((row) => row.destinations && row.noOverflow),
  JSON.stringify(brainResponsive),
);

// Test 8: Every hue and relation ink clears 3:1 on both graph grounds.
const brainContrast = await page.evaluate(async () => {
  window.__VAULT__ = { pages: 40, linksPer: 2, ghosts: 3,
    tags: ["tag3", "tag7", "tag0", "tag1", "tag2"], allTyped: true };
  dropTab("knowledge");
  document.getElementById("nav-knowledge").click();
  await new Promise((done) => setTimeout(done, 350));
  /* 색은 낱말이 아니라 **픽셀**로 읽는다.
   *
   * 계산된 색이 어느 공간의 낱말로 돌아오는지는 브라우저가 정한다 — 라이트
   * 판의 `color-mix`는 `oklab(...)`으로 돌아오고, `rgb(` 만 아는 눈은 그 답을
   * 「못 읽었다」가 아니라 「대비 0」으로 읽어 판을 거짓으로 빨갛게 만든다.
   * 그래서 그 색을 1×1 캔버스에 칠하고 sRGB 바이트를 도로 읽는다: 사람의 눈에
   * 닿는 바로 그 값이고, 공간이 몇 개든 답이 하나다. */
  const paint = document.createElement("canvas").getContext("2d", { willReadFrequently: true });
  const parse = (color) => {
    if (!color) return null;
    const sentinel = "#010203";
    paint.fillStyle = sentinel;
    paint.fillStyle = color;
    if (paint.fillStyle === sentinel) return null;
    paint.clearRect(0, 0, 1, 1);
    paint.fillRect(0, 0, 1, 1);
    const held = paint.getImageData(0, 0, 1, 1).data;
    return [held[0] / 255, held[1] / 255, held[2] / 255];
  };
  const luminance = (color) => {
    const channels = parse(color);
    if (!channels) return null;
    return channels.map((channel) => (channel <= 0.04045
      ? channel / 12.92
      : ((channel + 0.055) / 1.055) ** 2.4))
      .reduce((sum, channel, at) => sum + channel * [0.2126, 0.7152, 0.0722][at], 0);
  };
  const ratio = (left, right) => {
    const a = luminance(left);
    const b = luminance(right);
    if (a === null || b === null) return 0;
    return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
  };
  const measure = () => {
    const view = [...document.querySelectorAll(".file-view")]
      .find((one) => one.classList.contains("knowledge-view") && !one.hidden);
    const ground = getComputedStyle(view.querySelector(".knowledge-canvas")).backgroundColor;
    const probe = document.createElementNS(SVG_NS, "g");
    probe.setAttribute("class", "knowledge-node is-page");
    const dot = document.createElementNS(SVG_NS, "circle");
    dot.setAttribute("class", "knowledge-dot");
    dot.style.transition = "none";
    probe.appendChild(dot);
    view.querySelector(".knowledge-picture").appendChild(probe);
    const hues = [0, 1, 2, 3, 4, 5, 6, 7].map((hue) => {
      probe.dataset.hue = String(hue);
      const ink = getComputedStyle(dot).fill;
      return { name: `hue-${hue}`, ink, ratio: ratio(ink, ground) };
    });
    /* 색이 없는 군집의 쪽(t-12029) — 안개색도 같은 선을 넘는다. */
    delete probe.dataset.hue;
    const quietInk = getComputedStyle(dot).fill;
    hues.push({ name: "quiet", ink: quietInk, ratio: ratio(quietInk, ground) });
    probe.remove();
    const relations = ["related", "implements", "depends_on", "supersedes", "contradicts"]
      .map((kind) => {
        const edge = view.querySelector(`.knowledge-edge.kind-${kind}`);
        const ink = edge ? getComputedStyle(edge).stroke : "";
        return { name: kind, ink, ratio: ratio(ink, ground) };
      });
    return { ground, colors: [...hues, ...relations] };
  };
  const dark = measure();
  setTheme("light");
  /* 두 프레임은 모자라다. 점의 잉크에는 전이가 걸려 있고, 전이 중의
     `getComputedStyle`은 **가는 중인 색**을 — oklab으로 보간된 중간값을 —
     돌려준다. 실측: 두 프레임 뒤의 라이트 판이 다섯 색 모두 다크 램프를 그대로
     답했다(그리고 그 답은 `oklab(...)`이었다). 전이가 끝나기를 기다린다. */
  await new Promise((done) => setTimeout(done, 700));
  const light = measure();
  return { dark, light };
});
await page.screenshot({ path: join(UI, "..", "output/playwright/knowledge-graph-light.png") });
ok(
  "eight cluster hues, the quiet grey and five relation colors keep three-to-one contrast in both themes",
  [brainContrast.dark, brainContrast.light].every((theme) => theme.colors.length === 14
    && theme.colors.every((color) => color.ratio >= 3)),
  JSON.stringify(brainContrast),
);

// Reset back to dark
await page.evaluate(() => setTheme("dark"));

// Test 9: Scale test (1000 nodes)
const brainScale = await page.evaluate(async () => {
  window.__VAULT__ = { pages: 1000, linksPer: 2, ghosts: 20, tags: ["core", "reading", "tools"] };
  dropTab("knowledge");
  const frames = [];
  const clock = () => {
    frames.push(performance.now());
    if (frames.length < 240) requestAnimationFrame(clock);
  };
  requestAnimationFrame(clock);
  window.__GRAPH_ANSWERED_AT__ = 0;
  document.getElementById("nav-knowledge").click();
  const viewOf = () => [...document.querySelectorAll(".file-view")]
    .find((one) => one.classList.contains("knowledge-view") && !one.hidden);
  let firstPaint = 0;
  for (let round = 0; round < 200 && firstPaint === 0; round += 1) {
    await new Promise((done) => requestAnimationFrame(done));
    const node = viewOf()?.querySelector(".knowledge-node");
    if (node && /translate\(-?\d/u.test(node.getAttribute("transform") ?? "")) {
      firstPaint = performance.now() - window.__GRAPH_ANSWERED_AT__;
    }
  }
  const view = viewOf();
  const nodes = view.querySelectorAll(".knowledge-node").length;
  const edges = view.querySelectorAll(".knowledge-edge").length;
  /* 천 쪽의 전체 지도에서 이름이 서는 점은 한 줌이다 — 예산(토큰)을 넘지 않고,
     그것들끼리 겹치지도 않는다(09-16). 옛 계약이던 「is-labelled가 꺼져 있다」는
     차수 문턱 LOD와 함께 사라졌다. */
  const named = view.querySelectorAll(".knowledge-node.is-named").length;
  const labelBudget = Number(getComputedStyle(view).getPropertyValue("--knowledge-label-budget"));
  for (let round = 0; round < 400; round += 1) {
    await new Promise((done) => requestAnimationFrame(done));
    if ((knowledgeLayouts.get(view)?.left ?? 0) === 0) break;
  }
  let worstGap = 0;
  for (let at = 1; at < frames.length; at += 1) {
    worstGap = Math.max(worstGap, frames[at] - frames[at - 1]);
  }
  const layout = knowledgeLayouts.get(view);
  const spread = layout.bounds.maxX - layout.bounds.minX;
  /* 앞 케이스가 쥔 카메라(얼린 폭)를 천 쪽의 새 판이 물려받으면 그림이 네 배로
   * 확대된다 — 실측으로 프레임 50→265ms의 원인이었다. 새 점이 있는 판은 제 폭으로
   * 선다. */
  const canvasBox = view.querySelector(".knowledge-canvas");
  /* 09-16: 카메라는 두 축을 다 판 안에 넣는다(contain) — 폭만 맞추던 옛 셈은 판이
     짧아지면 그림의 위아래를 잘랐다. 그래서 「토큰의 몫을 차지한다」는 **묶는 축**의
     이야기이고, 다른 축은 그보다 작다. 앞 케이스의 배율(zoom)은 여전히 물려받는다 —
     볼트가 자라도 사람의 카메라는 뛰지 않는다. */
  const tallSpread = (layout.bounds.maxY - layout.bounds.minY)
    * (view.classList.contains("is-tier-compact") || view.classList.contains("is-tier-tiny")
      ? layout.tuning.yScaleCompact
      : view.classList.contains("is-tier-middle") ? layout.tuning.yScaleMiddle : 1);
  /* 두 축을 제 방에 대어 잰다(t-12029): 방은 판의 `mapFit` 몫이고 세로는 아래 범례 띠(`mapFoot`)를 뺀 높이의
   * 몫이되, 작은 판에서는 가장자리에 `margin` px를 남긴 만큼이다. 묶는 축이 제 방을 채운다(배율을 곱한 만큼). */
  const { mapFit, mapFoot, margin } = layout.tuning;
  const footRoom = Math.min(mapFoot, canvasBox.clientHeight * (1 - mapFit));
  const roomX = Math.min(canvasBox.clientWidth * mapFit, canvasBox.clientWidth - 2 * margin);
  const roomY = Math.min((canvasBox.clientHeight - footRoom) * mapFit, canvasBox.clientHeight - footRoom - 2 * margin);
  const fitRatio = (spread * layout.scale) / roomX;
  const fitTallRatio = (tallSpread * layout.scale) / roomY;
  const fitExpected = layout.zoom;
  const settle = async () => {
    knowledgeLayouts.delete(view);
    view.querySelector(".knowledge-nodes").dataset.knowledgeSignature = "";
    await paintKnowledgeView();
    for (let round = 0; round < 600; round += 1) {
      await new Promise((done) => requestAnimationFrame(done));
      if ((knowledgeLayouts.get(view)?.left ?? 0) === 0) break;
    }
    const held = knowledgeLayouts.get(view);
    return ["wiki/Page-0000.md", "wiki/Page-0500.md", "ghost:아직 없는 3"].map((key) => {
      const seat = held.model.keys.indexOf(key);
      return `${key}@${Math.round(held.x[seat])},${Math.round(held.y[seat])}`;
    }).join("|");
  };
  const seatedFirst = await settle();
  const seatedAgain = await settle();
  return {
    firstPaint: Math.round(firstPaint),
    nodes,
    edges,
    worstGap: Math.round(worstGap),
    spread: Math.round(spread),
    fitRatio: Math.round(fitRatio * 1000) / 1000,
    fitTallRatio: Math.round(fitTallRatio * 1000) / 1000,
    fitExpected: Math.round(fitExpected * 1000) / 1000,
    zoom: layout.zoom,
    layoutRuns: knowledgeLayoutRuns,
    named,
    labelBudget,
    seats: seatedFirst,
    deterministic: seatedFirst === seatedAgain,
    halos: view.querySelectorAll(".knowledge-halo").length,
    maxNodeParts: Math.max(...[...view.querySelectorAll(".knowledge-node")]
      .map((node) => node.children.length)),
    /* 군집(3차): 천 페이지에서도 이름 있는 군집마다 성운·이름표 한 쌍뿐이다. */
    namedClusters: layout.namedCount,
    clusterLabels: view.querySelectorAll(".knowledge-cluster-label").length,
    nebulas: view.querySelectorAll(".knowledge-nebula").length,
  };
});
/* 허브의 halo: 두 번째 <circle>인가 SVG `filter`인가.
 *
 * 재는 것은 캔버스를 실제로 래스터화하는 시간이다. 한 모드를 한 번씩만 재면 그
 * 답은 잡음이다 — 실측(1020 노드·656 halo): 같은 판의 두 라운드가 58.8/62.7과
 * 62.6/58.7로 서로 반대를 말했다. 그래서 두 모드를 **번갈아** 여러 번 재고
 * 견주며, 판정은 「원이 필터보다 비싸지 않다」이다. 여유 10%가 그 잡음의
 * 폭이고, 이보다 좁은 판정은 코드가 아니라 기계를 재게 된다.
 *
 * 견주는 것은 **같은 라운드의 짝 비율**(원 ÷ 필터)의 중앙값이다. 캡처 시간은
 * 두 봉우리(~49 ms·~57 ms, 프레임 양자화)로 갈리고, 한 봉우리 안에서 두 모드는
 * 같은 값이다 — 모드마다 따로 낸 중앙값끼리 견주면 코드가 아니라 표본이 봉우리를
 * 어떻게 나눴는지를 잰다. 1.3.33 레인(2026-09-11)이 57.1 대 49.6으로 루트 게이트를
 * 붉게 만든 것이 그것이고, 같은 표본 다섯 판(부하 둘·조용 셋)에서 옛 판정은
 * 두 번, 최솟값 판정은 한 번 떨어졌다. 짝 비율의 중앙값은 0.99~1.02였다 — 짝은
 * 같은 순간의 부하와 봉우리를 나눠 가진다. 첫 캡처는 캔버스를 처음 굽는 값(늘
 * 65~91 ms)이라 재기 전에 한 번 버린다. */
const measureHaloPaint = async (mode) => {
  await page.evaluate((next) => {
    const view = [...document.querySelectorAll(".file-view")]
      .find((one) => one.classList.contains("knowledge-view") && !one.hidden);
    const picture = view.querySelector(".knowledge-picture");
    let filter = picture.querySelector("#knowledge-benchmark-glow");
    if (!filter) {
      filter = document.createElementNS("http://www.w3.org/2000/svg", "filter");
      filter.id = "knowledge-benchmark-glow";
      filter.innerHTML = '<feGaussianBlur in="SourceGraphic" stdDeviation="2.5"/>';
      picture.querySelector("defs").appendChild(filter);
    }
    for (const node of picture.querySelectorAll('[data-hub="true"]')) {
      const halo = node.querySelector(".knowledge-halo");
      const dot = node.querySelector(".knowledge-dot");
      if (halo) halo.style.visibility = next === "circle" ? "visible" : "hidden";
      if (next === "filter") dot.setAttribute("filter", "url(#knowledge-benchmark-glow)");
      else dot.removeAttribute("filter");
    }
  }, mode);
  const began = performance.now();
  await page.locator(".file-view.knowledge-view:not([hidden]) .knowledge-canvas").screenshot();
  return performance.now() - began;
};
const haloSamples = { circle: [], filter: [] };
await measureHaloPaint("filter");
for (let round = 0; round < 7; round += 1) {
  for (const mode of ["circle", "filter"]) haloSamples[mode].push(await measureHaloPaint(mode));
}
const haloMedian = (held) => [...held].sort((left, right) => left - right)[Math.floor(held.length / 2)];
const haloPaint = {
  circleMs: Math.round(haloMedian(haloSamples.circle) * 10) / 10,
  filterMs: Math.round(haloMedian(haloSamples.filter) * 10) / 10,
  pairedRatio: Math.round(
    haloMedian(haloSamples.circle.map((ms, round) => ms / haloSamples.filter[round])) * 100,
  ) / 100,
};
await measureHaloPaint("circle");

const [knowledgeText, shellText, cssText, tokenText] = await Promise.all([
  readFile(join(UI, "shell-knowledge.js"), "utf8"),
  readFile(join(UI, "shell.js"), "utf8"),
  readFile(join(UI, "shell.css"), "utf8"),
  readFile(join(UI, "tokens.css"), "utf8"),
]);
const block = (text, marker) => {
  const start = text.indexOf(marker);
  const rest = text.slice(start);
  const end = rest.indexOf("\n/* ----", marker.length);
  return end < 0 ? rest : rest.slice(0, end);
};
const knowledgeSources = {
  js: knowledgeText.slice(knowledgeText.indexOf("/* ---- 지식 그래프")),
  css: block(cssText, "/* ---- 지식 그래프"),
  tokens: tokenText.split("\n").filter((line) => line.includes("--knowledge-")).join("\n"),
};
const sourceGate = {
  hex: Object.values(knowledgeSources).flatMap((source) => source.match(/#[\da-f]{3,8}\b/giu) ?? []),
  hueMappings: (knowledgeSources.css.match(/\.knowledge-view \[data-hue=/gu) ?? []).length,
  shared: ["paintGraphEdges(", "wireGraphDrag(", "watchGraphResize(", "scheduleGraphFrame("]
    .every((marker) => knowledgeSources.js.includes(marker)),
  copiedGeometryPin: shellText.includes("graphEdgeGeometry.set(line, [...edge.at])"),
};
ok(
  "a thousand pages meet the frame, halo, DOM, determinism and source-hardcoding contracts",
  brainScale.nodes === 1020 &&
    brainScale.edges > 1500 &&
    brainScale.firstPaint > 0 &&
    brainScale.firstPaint < 300 &&
    frameBudgetHolds(brainScale.worstGap) &&
    brainScale.spread > 400 && brainScale.spread < 6000 &&
    // 묶는 축이 제 방을 채우고, 두 축 다 제 방을 넘지 않는다(contain).
    Math.abs(Math.max(brainScale.fitRatio, brainScale.fitTallRatio) - brainScale.fitExpected) < 0.02 &&
    brainScale.fitRatio <= brainScale.fitExpected + 0.02 &&
    brainScale.fitTallRatio <= brainScale.fitExpected + 0.02 &&
    // 천 쪽에서 이름이 서는 점은 한 줌이고 예산(토큰)을 넘지 않는다(09-16).
    brainScale.named <= brainScale.labelBudget &&
    brainScale.deterministic &&
    brainScale.halos === brainScale.nodes &&
    brainScale.maxNodeParts <= 3 &&
    brainScale.namedClusters > 0 &&
    brainScale.clusterLabels === brainScale.namedClusters &&
    brainScale.nebulas === brainScale.namedClusters &&
    /* 두 칠 방식의 시간 비율도 벽시계다 — 시끄러운 기계에서는 기록만(machine-load.mjs). */
    (haloPaint.pairedRatio <= 1.1 || machineIsLoudNow()) &&
    sourceGate.hex.length === 0 &&
    sourceGate.hueMappings === 8 &&
    sourceGate.shared &&
    sourceGate.copiedGeometryPin,
  JSON.stringify({ scale: brainScale, haloPaint, sourceGate }),
);

// Test 10: Time slicer (collapsed by default, 'all', dimming only, dragging < 16ms at 1020 nodes)
const slicerScale = await page.evaluate(async () => {
  const view = document.querySelector(".knowledge-view:not([hidden])");
  const layout = knowledgeLayouts.get(view);
  const layoutRunsBefore = knowledgeLayoutRuns;
  const nodesBefore = view.querySelectorAll(".knowledge-node").length;
  const sampleCoordsBefore = [...view.querySelectorAll(".knowledge-node")]
    .slice(0, 5).map((n) => n.getAttribute("transform"));

  const slicer = view.querySelector(".knowledge-slicer");
  const toggle = slicer?.querySelector(".knowledge-slicer-toggle");
  const popover = slicer?.querySelector(".knowledge-slicer-popover");
  const slider = slicer?.querySelector(".knowledge-slicer-slider");

  const collapsedDefault = toggle?.getAttribute("aria-expanded") === "false"
    && !popover?.classList.contains("is-open");
  const defaultLabel = toggle?.textContent ?? "";

  toggle?.click();
  const opened = toggle?.getAttribute("aria-expanded") === "true"
    && popover?.classList.contains("is-open");

  if (!slicer || !slider) {
    return { hasSlicer: false };
  }

  const frameDurations = [];
  for (let step = 1; step <= 10; step += 1) {
    slider.value = String(step * 10);
    const t0 = performance.now();
    slider.dispatchEvent(new Event("input", { bubbles: true }));
    const duration = performance.now() - t0;
    frameDurations.push(duration);
    await new Promise((done) => requestAnimationFrame(done));
  }
  const worstDragGap = Math.max(...frameDurations);

  slider.value = "50";
  slider.dispatchEvent(new Event("input", { bubbles: true }));
  await new Promise((done) => requestAnimationFrame(done));

  const dimmedCount = view.querySelectorAll(".knowledge-node.is-slice-dim").length;
  const activeCount = view.querySelectorAll(".knowledge-node:not(.is-slice-dim)").length;
  const isSliced = view.querySelector(".knowledge-picture").classList.contains("is-sliced");
  const nodesAfter = view.querySelectorAll(".knowledge-node").length;
  const sampleCoordsAfter = [...view.querySelectorAll(".knowledge-node")]
    .slice(0, 5).map((n) => n.getAttribute("transform"));
  const layoutRunsAfterSlice = knowledgeLayoutRuns;

  /* 다음 프레임도 같은 흐림을 입어야 한다(K19): 빈 캔버스를 누르는 몸짓 하나가
   * 프레임을 다시 그리고, 그 프레임의 선 옷이 슬라이서의 흐림을 지우면 점은 흐린데
   * 선만 온 불투명도로 돌아온다. 재는 것은 클래스가 아니라 보이는 불투명도다. */
  const outsideEdge = [...view.querySelectorAll(".knowledge-edges > g")].find((group) => {
    const [from, to] = group.dataset.graphEdge.split(">");
    const dim = (key) => view.querySelector(`.knowledge-node[data-graph-key="${key}"]`)
      ?.classList.contains("is-slice-dim");
    return dim(from) && dim(to);
  });
  view.querySelector(".knowledge-canvas").dispatchEvent(new MouseEvent("click", { bubbles: true }));
  await new Promise((done) => requestAnimationFrame(done));
  const outsideLine = outsideEdge?.querySelector("path");
  if (outsideLine) {
    getComputedStyle(outsideLine).opacity;
    await Promise.all(outsideLine.getAnimations().map((one) => one.finished.catch(() => {})));
  }
  const sliceEdgeOpacity = outsideLine ? Number(getComputedStyle(outsideLine).opacity) : -1;
  const dimEdgeOpacity = Number(getComputedStyle(view).getPropertyValue("--knowledge-dim-edge-opacity"));

  view.querySelector(".knowledge-canvas").dispatchEvent(new MouseEvent("mousemove", { bubbles: true, clientX: 200, clientY: 200 }));
  await new Promise((done) => requestAnimationFrame(done));
  const layoutRunsAfterMove = knowledgeLayoutRuns;

  slider.value = "0";
  slider.dispatchEvent(new Event("input", { bubbles: true }));
  await new Promise((done) => requestAnimationFrame(done));

  const resetDimmed = view.querySelectorAll(".knowledge-node.is-slice-dim").length;
  const resetIsSliced = view.querySelector(".knowledge-picture").classList.contains("is-sliced");

  return {
    hasSlicer: !!slicer,
    collapsedDefault,
    defaultLabel,
    opened,
    worstDragGap: Math.round(worstDragGap * 10) / 10,
    membershipUnchanged: nodesBefore === 1020 && nodesAfter === 1020,
    coordsUnchanged: JSON.stringify(sampleCoordsBefore) === JSON.stringify(sampleCoordsAfter),
    noRelayout: layoutRunsBefore === layoutRunsAfterSlice && layoutRunsAfterSlice === layoutRunsAfterMove,
    isSliced,
    dimmedCount,
    activeCount,
    resetDimmed,
    resetIsSliced,
    sliceEdgeOpacity,
    dimEdgeOpacity,
  };
});

ok(
  "the time slicer collapses to 'all', dims only without relayout, and drags under 16ms at 1020 nodes",
  slicerScale.hasSlicer &&
    slicerScale.collapsedDefault &&
    (slicerScale.defaultLabel.includes("전체") || slicerScale.defaultLabel.includes("All")) &&
    slicerScale.opened &&
    slicerScale.worstDragGap < 16 &&
    slicerScale.membershipUnchanged &&
    slicerScale.coordsUnchanged &&
    slicerScale.noRelayout &&
    slicerScale.isSliced &&
    slicerScale.dimmedCount > 0 &&
    slicerScale.activeCount > 0 &&
    slicerScale.resetDimmed === 0 &&
    !slicerScale.resetIsSliced &&
    slicerScale.dimEdgeOpacity > 0 &&
    Math.abs(slicerScale.sliceEdgeOpacity - slicerScale.dimEdgeOpacity) < 0.01,
  JSON.stringify(slicerScale),
);
console.log(`   slicer worst drag gap at 1020 nodes: ${slicerScale.worstDragGap}ms`);

// Test 11: Paths (t-5966 G3). The calculator is the backend's (`second_brain_paths`); Shift-click asks it with
// the two page keys and the vault, the answer's first path (fewest bare mentions at the shortest length) lights
// the picture, the card lists every path with its hops' kind and road, picking another re-lights, Esc clears,
// the ask form reaches the same door by a typed name — and putting an answer onto the 1020-page picture
// (route + highlight + frame) stays inside the frame budget.
const pathTest = await page.evaluate(async () => {
  const wait = (ms) => new Promise((done) => setTimeout(done, ms));
  const view1020 = document.querySelector(".knowledge-view:not([hidden])");
  const layout1020 = knowledgeLayouts.get(view1020);
  const keys1020 = layout1020.model.keys;
  /* 픽스처의 천 쪽은 가까운 뒤 쪽으로만 잇는다(`nearest`) — 여섯 홉 안에 닿는 짝을 묻는다. */
  const answer1020 = await window.__ANSWER__.second_brain_paths({ from: keys1020[0], to: keys1020[12] });
  /* 경로가 프레임에 더하는 값: 같은 판의 맨 프레임(경로 없음)과 경로를 밝힌 프레임을 번갈아 재고
   * 둘의 최솟값의 차를 본다 — 한 프레임 전체는 기계와 부하의 것이고, 경로의 몫은 그 차다. */
  let routeMs = Infinity;
  let frameMs = Infinity;
  let markMs = Infinity;
  let lightMs = Infinity;
  for (let run = 0; run < 5; run += 1) {
    highlightKnowledgePath(view1020, layout1020, null);
    const b0 = performance.now();
    paintKnowledgeFrame(view1020, layout1020);
    frameMs = Math.min(frameMs, performance.now() - b0);
    const t0 = performance.now();
    const route = knowledgeRoute(layout1020.model, { report: answer1020, picked: 0 });
    const t1 = performance.now();
    highlightKnowledgePath(view1020, layout1020, route);
    const t2 = performance.now();
    paintKnowledgeFrame(view1020, layout1020);
    const t3 = performance.now();
    routeMs = Math.min(routeMs, t1 - t0);
    markMs = Math.min(markMs, t2 - t1);
    lightMs = Math.min(lightMs, t3 - t1);
  }
  highlightKnowledgePath(view1020, layout1020, null);
  paintKnowledgeFrame(view1020, layout1020);
  const paths1020 = answer1020.paths.length;

  // Setup custom graph with 6 pages:
  // Node 0 -> Node 1 (mentions), Node 1 -> Node 3 (mentions)  [bare path: 0 -> 1 -> 3]
  // Node 0 -> Node 2 (depends_on), Node 2 -> Node 3 (implements)  [typed path: 0 -> 2 -> 3]
  // Node 4 -> Node 5 (mentions), apart from the rest
  knowledgeSelectedKey = null;
  knowledgeSlicerCutoff = 0;
  knowledgeProvenanceHidden.clear();
  window.__PATHS_ASKED__ = [];
  window.__VAULT__ = {
    pages: 6,
    customEdges: [
      { from: 0, to: 1, kind: "mentions" },
      { from: 1, to: 3, kind: "mentions" },
      { from: 0, to: 2, kind: "depends_on" },
      { from: 2, to: 3, kind: "implements" },
      { from: 4, to: 5, kind: "mentions" },
    ],
  };
  dropTab("knowledge");
  document.getElementById("nav-knowledge").click();
  await wait(300);

  const view = document.querySelector(".knowledge-view:not([hidden])");
  const canvas = view.querySelector(".knowledge-canvas");
  const nodes = [...view.querySelectorAll(".knowledge-node")];
  const shown = () => ({
    lit: [...view.querySelectorAll(".knowledge-node.is-path-lit")].map((n) => n.dataset.graphKey).sort(),
    /* 물러선 점은 판의 규칙이 흐린다 — 클래스가 아니라 계산된 불투명도로 센다. */
    dim: [...view.querySelectorAll(".knowledge-node")]
      .filter((one) => Number(getComputedStyle(one).opacity) < 1).length,
    edges: view.querySelectorAll(".knowledge-edge.is-path-lit").length,
    chain: view.querySelector(".knowledge-chain-body")?.textContent ?? "",
    roads: [...view.querySelectorAll(".knowledge-chain-road")].map((one) => one.dataset.edgeProvenance),
    list: [...view.querySelectorAll(".knowledge-path-pick")].map((one) => `${one.textContent}|${one.getAttribute("aria-pressed")}`),
    note: view.querySelector(".knowledge-path-note")?.textContent ?? "",
    sectionHidden: view.querySelector(".knowledge-inspector-chain")?.hidden ?? true,
    pathed: view.querySelector(".knowledge-picture").classList.contains("is-path"),
  });

  // 1. Select Node 0
  nodes[0]?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  await wait(50);
  const node0Selected = nodes[0]?.classList.contains("is-selected") ?? false;
  // Focus depth 1 (the default a person starts at): node 3 is two hops from node 0.
  view.querySelector('[data-knowledge-depth="1"]')?.click();
  await wait(50);

  // 2. Shift-click Node 3 — the backend is asked once with both keys, the vault and the sources flag.
  nodes[3]?.dispatchEvent(new MouseEvent("click", { bubbles: true, shiftKey: true }));
  await wait(80);
  const asked = window.__PATHS_ASKED__.map((one) => ({ ...one }));
  const first = shown();

  /* 경로는 다음 프레임에도 보여야 한다(K19·K22): 배율 단추 하나가 프레임을 다시
   * 그린 뒤, 경로 밖의 선은 흐림의 불투명도이고 경로의 선은 밝힘의 잉크다.
   * 밝힘의 잉크는 토큰을 같은 SVG 안에서 풀어 얻는다. */
  const settledStyle = async (node) => {
    getComputedStyle(node).opacity;
    await Promise.all(node.getAnimations().map((one) => one.finished.catch(() => {})));
    return getComputedStyle(node);
  };
  /* 경로의 점은 앞선 선택의 포커스 흐림을 입지 않는다(K20): 깊이 1에서 A를 고르고
   * 두 홉 밖의 B를 Shift-클릭하면, 사람이 방금 누른 B와 그 앞의 홉이 온전히 보여야
   * 한다. */
  const pathNodeOpacity = [];
  for (const at of [0, 2, 3]) pathNodeOpacity.push(Number((await settledStyle(nodes[at])).opacity));
  view.querySelector(".knowledge-zoom-in").click();
  await new Promise((done) => requestAnimationFrame(done));
  const picture = view.querySelector(".knowledge-picture");
  // Two pages can carry both mentions and a typed relation: each has its own line.
  const edgeLine = (from, to, kind) => view
    .querySelector(`.knowledge-edges > g[data-graph-edge="wiki/Page-000${from}.md>wiki/Page-000${to}.md>${kind}"] path`);
  const probe = document.createElementNS("http://www.w3.org/2000/svg", "path");
  probe.style.stroke = "var(--knowledge-highlight)";
  picture.appendChild(probe);
  const highlightInk = getComputedStyle(probe).stroke;
  probe.remove();
  const offPathOpacity = Number((await settledStyle(edgeLine(0, 1, "mentions"))).opacity);
  const onPathStroke = (await settledStyle(edgeLine(0, 2, "depends_on"))).stroke;
  const dimEdgeOpacity = Number(getComputedStyle(view).getPropertyValue("--knowledge-dim-edge-opacity"));

  // 3. Pick the second path in the list — the bare one lights, the answer is not asked again.
  view.querySelectorAll(".knowledge-path-pick")[1]?.click();
  await wait(50);
  const second = shown();
  const askedAfterPick = window.__PATHS_ASKED__.length;

  // 4. Esc clears the path and keeps the selection.
  canvas.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  await wait(50);
  const cleared = shown();
  const node0StillSelected = nodes[0]?.classList.contains("is-selected") ?? false;

  // 5. The ask form reaches the same door with a typed name; the backend resolves it.
  const ask = view.querySelector(".knowledge-path-ask");
  ask.querySelector(".knowledge-path-to").value = "개념 3";
  ask.requestSubmit();
  await wait(80);
  const askedByName = window.__PATHS_ASKED__.at(-1);
  const byName = { ...shown(), targetKey: knowledgePath?.targetKey ?? null };

  // 6. A name nothing answers to is said in the backend's own sentence, and nothing lights.
  ask.querySelector(".knowledge-path-to").value = "없는 페이지";
  ask.requestSubmit();
  await wait(80);
  const refused = shown();

  canvas.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  await wait(30);
  canvas.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  await wait(50);
  const node0Deselected = !(nodes[0]?.classList.contains("is-selected") ?? false);

  return {
    routeMs: Math.round(routeMs * 100) / 100,
    frameMs: Math.round(frameMs * 100) / 100,
    markMs: Math.round(markMs * 100) / 100,
    lightMs: Math.round(lightMs * 100) / 100,
    paths1020,
    node0Selected,
    asked,
    first,
    pathNodeOpacity,
    offPathOpacity,
    dimEdgeOpacity,
    onPathStroke,
    highlightInk,
    second,
    askedAfterPick,
    cleared,
    node0StillSelected,
    askedByName,
    byName,
    refused,
    node0Deselected,
  };
});

ok(
  "paths: Shift-click asks the backend's one calculator, its first path lights the picture, the card lists every path with kind and road, picking re-lights, the ask form resolves a name, a refusal is said, and Esc clears",
  pathTest.node0Selected
    && pathTest.asked.length === 1
    && pathTest.asked[0].from === "wiki/Page-0000.md" && pathTest.asked[0].to === "wiki/Page-0003.md"
    && pathTest.asked[0].path === "/vault" && pathTest.asked[0].sources === false && pathTest.asked[0].k === undefined
    && pathTest.first.pathed && !pathTest.first.sectionHidden
    && JSON.stringify(pathTest.first.lit) === JSON.stringify(["wiki/Page-0000.md", "wiki/Page-0002.md", "wiki/Page-0003.md"])
    && pathTest.first.dim > 0 && pathTest.first.edges === 2
    && pathTest.first.chain === "개념 0 → depends_on·선언 → 개념 2 → implements·선언 → 개념 3"
    && JSON.stringify(pathTest.first.roads) === JSON.stringify(["declared", "declared"])
    && JSON.stringify(pathTest.first.list) === JSON.stringify(["1. 2홉 · depends_on · implements|true", "2. 2홉 · mentions · mentions|false"])
    && pathTest.first.note === "경로 2 · 최단 2홉"
    && pathTest.pathNodeOpacity.length === 3 && pathTest.pathNodeOpacity.every((opacity) => opacity === 1)
    && Math.abs(pathTest.offPathOpacity - pathTest.dimEdgeOpacity) < 0.01
    && pathTest.highlightInk !== "" && pathTest.onPathStroke === pathTest.highlightInk
    && JSON.stringify(pathTest.second.lit) === JSON.stringify(["wiki/Page-0000.md", "wiki/Page-0001.md", "wiki/Page-0003.md"])
    && pathTest.second.chain === "개념 0 → mentions·추론 → 개념 1 → mentions·추론 → 개념 3"
    && JSON.stringify(pathTest.second.list) === JSON.stringify(["1. 2홉 · depends_on · implements|false", "2. 2홉 · mentions · mentions|true"])
    && pathTest.askedAfterPick === 1
    && !pathTest.cleared.pathed && pathTest.cleared.lit.length === 0 && pathTest.cleared.sectionHidden
    && pathTest.node0StillSelected
    && pathTest.askedByName.to === "개념 3" && pathTest.askedByName.from === "wiki/Page-0000.md"
    && pathTest.byName.targetKey === "wiki/Page-0003.md" && pathTest.byName.pathed && pathTest.byName.edges === 2
    && !pathTest.refused.pathed && pathTest.refused.lit.length === 0 && !pathTest.refused.sectionHidden
    && pathTest.refused.note.includes("없는 페이지") && pathTest.refused.list.length === 0
    && pathTest.node0Deselected
    /* 경로를 밝힌 프레임은 이 판의 최악 프레임 예산(`FRAME_BUDGET_MS`, 조용한 기계에서만 판정 — machine-load.mjs, 천 쪽의 앉는 프레임과 같은 자) 안이다.
     * 판의 `is-path` 한 클래스가 점·선 전부의 옷을 바꾸므로 그 프레임은 스타일 재계산을 강제로
     * 치른다 — 실측: 점·선마다 흐림 클래스를 쓰던 판 102 ms → 판의 규칙으로 79 ms(09-22). */
    /* 경로 계산 시간도 벽시계다 — 시끄러운 기계에서는 기록만(machine-load.mjs). */
    && pathTest.paths1020 > 0 && (pathTest.routeMs < 2 || machineIsLoudNow()) && frameBudgetHolds(pathTest.lightMs),
  JSON.stringify(pathTest),
);
console.log(`METRIC knowledge path onto 1020 nodes: route ${pathTest.routeMs}ms; bare frame ${pathTest.frameMs}ms; `
  + `mark ${pathTest.markMs}ms; mark+frame ${pathTest.lightMs}ms; paths ${pathTest.paths1020}; ${loadNote()}`);

/* Test 11b: 경로는 두 페이지의 열쇠로 든다(K21). 렌즈 하나가 점들을 새 자리에 앉히면
 * 옛 자리 번호는 다른 페이지를 가리킨다 — 그때 밝는 것은 엉뚱한 점이고 인스펙터의
 * 사슬은 다른 페이지의 제목을 읽는다. 같은 두 페이지를 새 그림에서 다시 찾아야 하고,
 * 끝점이 그림에서 빠지면 경로는 서지 않았다가 돌아오면 다시 선다. */
const pathKeyed = await page.evaluate(async () => {
  const view = document.querySelector(".knowledge-view:not([hidden])");
  const picture = view.querySelector(".knowledge-picture");
  const wait = (ms) => new Promise((done) => setTimeout(done, ms));
  const nodeOf = (key) => view.querySelector(`.knowledge-node[data-graph-key="wiki/Page-000${key}.md"]`);
  const shown = () => ({
    lit: [...view.querySelectorAll(".knowledge-node.is-path-lit")].map((one) => one.dataset.graphKey).sort(),
    edges: [...view.querySelectorAll(".knowledge-edges > g")]
      .filter((group) => group.querySelector("path")?.classList.contains("is-path-lit"))
      .map((group) => group.dataset.graphEdge).sort(),
    chain: view.querySelector(".knowledge-inspector-chain")?.hidden
      ? null
      : view.querySelector(".knowledge-chain-body")?.textContent ?? null,
    /* 답은 남고 그림만 없는 판(t-5966): 요약 줄이 그렇다고 말한다. */
    note: view.querySelector(".knowledge-path-note")?.textContent ?? "",
    path: picture.classList.contains("is-path"),
  });
  nodeOf(0).dispatchEvent(new MouseEvent("click", { bubbles: true }));
  await wait(50);
  nodeOf(3).dispatchEvent(new MouseEvent("click", { bubbles: true, shiftKey: true }));
  await wait(50);
  const before = shown();
  const flag = (name) => view.querySelector(`[data-knowledge-flag="${name}"]`);
  flag("typed").click();
  await wait(80);
  const typed = shown();
  flag("typed").click();
  await wait(80);
  flag("orphans").click();
  await wait(80);
  const orphans = shown();
  flag("orphans").click();
  await wait(80);
  const back = shown();
  const canvas = view.querySelector(".knowledge-canvas");
  canvas.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  canvas.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  await wait(50);
  return { before, typed, orphans, back };
});
{
  const route = ["wiki/Page-0000.md", "wiki/Page-0002.md", "wiki/Page-0003.md"];
  const edges = ["wiki/Page-0000.md>wiki/Page-0002.md>depends_on", "wiki/Page-0002.md>wiki/Page-0003.md>implements"];
  const chain = "개념 0 → depends_on·선언 → 개념 2 → implements·선언 → 개념 3";
  const same = (seen) => seen.path
    && JSON.stringify(seen.lit) === JSON.stringify(route)
    && JSON.stringify(seen.edges) === JSON.stringify(edges)
    && seen.chain === chain && !seen.note.includes("이 그림에는 없는 경로");
  /* 끝점이 렌즈에서 빠지면 그림에는 길이 없고(밝힌 점·선 없음), 답의 사슬은 카드에 남아
   * 「이 그림에는 없는 경로」라고 말한다 — 열쇠가 돌아오면 그림도 돌아온다(K21). */
  ok("a shown path is held by its two pages and found again when a lens re-seats the picture",
    same(pathKeyed.before)
      && same(pathKeyed.typed)
      && !pathKeyed.orphans.path && pathKeyed.orphans.lit.length === 0
      && pathKeyed.orphans.chain === chain && pathKeyed.orphans.note.includes("이 그림에는 없는 경로")
      && same(pathKeyed.back),
    JSON.stringify(pathKeyed));
}

/* Test 11d(t-5966 G1): 선의 근거. 백엔드가 선마다 `provenance`를 적어 보내고, 창은 그것을
 * 읽기만 한다 — 렌즈의 세 토글(기본 전부 켜짐)은 그 길의 선을 그림에서 빼고 점은 남긴다;
 * 범례에 세 줄이 서고; 고른 점의 관계 줄마다 근거의 칩이 서며; 밝은 선의 낱말은 관계와
 * 근거를 <title>로 든다. 건강 카드의 「근거 없는 간선」은 백엔드 표의 0을 그대로 읽는다. */
const provenanceLens = await page.evaluate(async () => {
  const wait = (ms) => new Promise((done) => setTimeout(done, ms));
  knowledgeSelectedKey = null;
  knowledgeQuery = "";
  knowledgeProvenanceHidden.clear();
  window.__VAULT__ = {
    pages: 5,
    customEdges: [
      { from: 0, to: 1, kind: "mentions" },
      { from: 0, to: 2, kind: "depends_on" },
      { from: 2, to: 3, kind: "implements" },
      { from: 3, to: 4, kind: "mentions" },
    ],
  };
  dropTab("knowledge");
  document.getElementById("nav-knowledge").click();
  await wait(300);
  const view = document.querySelector(".knowledge-view:not([hidden])");
  const edgesOf = () => [...view.querySelectorAll(".knowledge-edges .knowledge-edge")];
  const roadsOf = () => edgesOf().map((line) => ["measured", "declared", "inferred"]
    .find((road) => line.classList.contains(`is-${road}`)) ?? "none").sort();
  const nodesOf = () => view.querySelectorAll(".knowledge-node").length;
  const flags = [...view.querySelectorAll("[data-knowledge-provenance]")]
    .map((one) => [one.dataset.knowledgeProvenance, one.getAttribute("aria-pressed")]);
  const legend = [...view.querySelectorAll(".knowledge-legend [data-edge-provenance]")]
    .map((one) => one.dataset.edgeProvenance);
  const before = { edges: edgesOf().length, roads: roadsOf(), nodes: nodesOf() };
  /* 「추론」을 끄면 본문 링크의 두 선이 빠지고 점 다섯은 그대로다. */
  view.querySelector('[data-knowledge-provenance="inferred"]').click();
  await wait(80);
  const inferredOff = { edges: edgesOf().length, roads: roadsOf(), nodes: nodesOf(),
    pressed: view.querySelector('[data-knowledge-provenance="inferred"]').getAttribute("aria-pressed") };
  view.querySelector('[data-knowledge-provenance="inferred"]').click();
  await wait(80);
  const back = { edges: edgesOf().length, roads: roadsOf() };
  /* 고른 점의 관계 줄: 나가는 둘(본문·키)에 근거의 칩이 각각 선다. */
  selectKnowledgeNode(view, "wiki/Page-0000.md");
  await wait(60);
  const chips = [...view.querySelectorAll(".knowledge-relations-outgoing .knowledge-relation-provenance")]
    .map((one) => one.dataset.edgeProvenance).sort();
  /* 밝은 선의 낱말은 관계와 근거를 툴팁으로 든다(방향 있는 관계에만 낱말이 선다). */
  litKnowledge(view, knowledgeLayouts.get(view), "wiki/Page-0000.md");
  await wait(30);
  const titles = [...view.querySelectorAll(".knowledge-edge-label title")].map((one) => one.textContent);
  litKnowledge(view, knowledgeLayouts.get(view), null);
  selectKnowledgeNode(view, null);
  const health = view.querySelector('.knowledge-health-row[data-knowledge-lint="unsourced_edges"]');
  const overview = [...view.querySelectorAll(".knowledge-overview-provenances li")]
    .map((one) => `${one.querySelector(".knowledge-relation-word").dataset.edgeProvenance}=${one.querySelector(".knowledge-inspector-note").textContent}`);
  return { flags, legend, before, inferredOff, back, chips, titles,
    health: health ? { note: health.querySelector(".knowledge-inspector-note").textContent,
      disabled: health.querySelector("button").disabled } : null,
    overview };
});
ok("provenance: three toggles default on, a hidden road drops its lines and keeps the points, the legend, the card chips and the label tooltip read the answer's road",
  JSON.stringify(provenanceLens.flags) === JSON.stringify([["measured", "true"], ["declared", "true"], ["inferred", "true"]])
    && JSON.stringify(provenanceLens.legend) === JSON.stringify(["measured", "declared", "inferred"])
    && provenanceLens.before.edges === 4
    && JSON.stringify(provenanceLens.before.roads) === JSON.stringify(["declared", "declared", "inferred", "inferred"])
    && provenanceLens.inferredOff.edges === 2
    && JSON.stringify(provenanceLens.inferredOff.roads) === JSON.stringify(["declared", "declared"])
    && provenanceLens.inferredOff.nodes === provenanceLens.before.nodes
    && provenanceLens.inferredOff.pressed === "false"
    && provenanceLens.back.edges === 4
    && JSON.stringify(provenanceLens.chips) === JSON.stringify(["declared", "inferred"])
    && provenanceLens.titles.length === 1 && provenanceLens.titles[0].startsWith("depends_on · ")
    && provenanceLens.health?.note === "0" && provenanceLens.health?.disabled === true
    && JSON.stringify(provenanceLens.overview) === JSON.stringify(["measured=0", "declared=2", "inferred=2"]),
  JSON.stringify(provenanceLens));

/* Test 11e(t-5966 G4): HTML로 내보내기. 단추 하나가 지금 렌즈의 부분그래프 — 점의 자리·쉬는 옷·
 * 선의 관계·근거·잉크 — 를 백엔드의 한 문(`second_brain_export_html`)에 건네고, 영수증을 토스트로
 * 말한다. 「타입 관계만」이 켜진 판에서는 그 렌즈가 남긴 점과 선만 실린다. */
const exportTest = await page.evaluate(async () => {
  const wait = (ms) => new Promise((done) => setTimeout(done, ms));
  knowledgeSelectedKey = null;
  knowledgeQuery = "";
  knowledgeProvenanceHidden.clear();
  window.__EXPORTS__ = [];
  window.__VAULT__ = {
    pages: 5,
    customEdges: [
      { from: 0, to: 1, kind: "mentions" },
      { from: 0, to: 2, kind: "depends_on" },
      { from: 2, to: 3, kind: "implements" },
      { from: 3, to: 4, kind: "mentions" },
    ],
  };
  dropTab("knowledge");
  document.getElementById("nav-knowledge").click();
  await wait(300);
  const view = document.querySelector(".knowledge-view:not([hidden])");
  view.querySelector(".knowledge-export").click();
  await wait(80);
  const whole = window.__EXPORTS__[0]?.input ?? null;
  const toastWhole = document.querySelector(".toast")?.textContent ?? "";
  view.querySelector('[data-knowledge-flag="typed"]').click();
  await wait(80);
  view.querySelector(".knowledge-export").click();
  await wait(80);
  const typed = window.__EXPORTS__[1]?.input ?? null;
  view.querySelector('[data-knowledge-flag="typed"]').click();
  await wait(50);
  const rgba = (word) => /^rgba\(\d+, \d+, \d+, [\d.]+\)$/.test(word);
  const colour = (word) => /^rgba?\(/.test(word);
  return {
    asked: window.__EXPORTS__.length,
    whole: whole === null ? null : {
      title: whole.title, vault: whole.vault, lenses: whole.lenses, nodes: whole.nodes.length, edges: whole.edges.length,
      theme: Object.values(whole.theme).every(colour),
      nodeInks: whole.nodes.every((node) => rgba(node.fill) && rgba(node.stroke) && node.r > 0 && typeof node.x === "number"),
      edgeInks: whole.edges.every((edge) => rgba(edge.ink) && ["measured", "declared", "inferred"].includes(edge.provenance)),
      directed: whole.edges.filter((edge) => edge.directed).map((edge) => edge.kind).sort(),
      legend: whole.legend.map((row) => row.kind).sort(),
      exportedAt: whole.exported_at,
    },
    typed: typed === null ? null : { nodes: typed.nodes.length, edges: typed.edges.length, lenses: typed.lenses,
      kinds: typed.edges.map((edge) => edge.kind).sort() },
    toastWhole,
  };
});
ok("export: the button hands the lens's picture — settled points in their computed inks, every line with kind, road and ink — to the one backend door and says the receipt; a lens narrows what is handed over",
  exportTest.asked === 2 && exportTest.whole !== null && exportTest.typed !== null
    && exportTest.whole.vault === "/vault" && exportTest.whole.title.includes("vault")
    && exportTest.whole.nodes === 5 && exportTest.whole.edges === 4
    && exportTest.whole.theme && exportTest.whole.nodeInks && exportTest.whole.edgeInks
    && JSON.stringify(exportTest.whole.directed) === JSON.stringify(["depends_on", "implements"])
    && JSON.stringify(exportTest.whole.legend) === JSON.stringify(["depends_on", "implements", "mentions"])
    && /^\d{4}-\d{2}-\d{2}T/.test(exportTest.whole.exportedAt)
    && exportTest.toastWhole.includes("KB") && exportTest.toastWhole.includes("5")
    && exportTest.typed.nodes === 3 && exportTest.typed.edges === 2
    && JSON.stringify(exportTest.typed.kinds) === JSON.stringify(["depends_on", "implements"])
    && exportTest.typed.lenses.some((word) => word.includes("타입")),
  JSON.stringify(exportTest));

/* Test 11c: 시간 슬라이서의 프리셋은 벽시계의 창이다(K24) — 「24시간」은 지금에서
 * 24시간 안에 고쳐지거나 회상된 페이지를 남긴다. 볼트의 시간 폭에 대한 백분율로
 * 옮겨 1% 단위로 반올림하고 1~100에 가두면, 400일 볼트의 「24시간」은 가장 새 페이지
 * 하나만 남기고 3시간 전의 페이지를 흐린다. 그리고 슬라이서의 낱말은 언어를 바꿔도
 * 지금의 창을 말한다(낮은 발견): 퍼센트는 퍼센트로, 프리셋은 그 이름으로 — 「전체」로
 * 되돌아가지 않는다. 검색어 저장 단추의 ★도 남는다. */
const slicerPresets = await page.evaluate(async () => {
  const hour = 3600000;
  const day = 24 * hour;
  const now = Date.now();
  window.__VAULT__ = {
    pages: 6,
    tags: ["core"],
    customEdges: [
      { from: 0, to: 1 }, { from: 1, to: 2 }, { from: 2, to: 3 }, { from: 3, to: 4 }, { from: 4, to: 5 },
    ],
    modifiedMs: [now - 400 * day, now - 40 * day, now - 5 * day, now - 3 * hour, now - 10 * 60000, now - 2 * day],
  };
  dropTab("knowledge");
  document.getElementById("nav-knowledge").click();
  const viewOf = () => document.querySelector(".knowledge-view:not([hidden])");
  // The previous case also drew six pages: wait for THIS vault's times to be drawn.
  const oldest = window.__VAULT__.modifiedMs[0];
  for (let round = 0; round < 200
    && knowledgeLayouts.get(viewOf())?.model.modified[0] !== oldest; round += 1) {
    await new Promise((done) => setTimeout(done, 20));
  }
  const view = viewOf();
  const frame = () => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
  const lit = () => [...view.querySelectorAll(".knowledge-node:not(.is-slice-dim)")]
    .map((node) => Number(node.dataset.graphKey.slice("wiki/Page-".length, -".md".length))).sort();
  const label = () => view.querySelector(".knowledge-slicer-label").textContent;
  const preset = async (id) => {
    view.querySelector(`.knowledge-slicer-preset[data-preset="${id}"]`).click();
    await frame();
    return { lit: lit(), label: label() };
  };
  const got = {};
  for (const id of ["1h", "24h", "7d", "30d"]) got[id] = await preset(id);
  // The page edited three hours ago is visibly lit under 24h, the 40-day-old one dimmed.
  await preset("24h");
  const opacityOf = async (seat) => {
    const node = view.querySelector(`.knowledge-node[data-graph-key="wiki/Page-000${seat}.md"]`);
    getComputedStyle(node).opacity;
    await Promise.all(node.getAnimations().map((one) => one.finished.catch(() => {})));
    return Number(getComputedStyle(node).opacity);
  };
  const recent = await opacityOf(3);
  const old = await opacityOf(1);
  // A language change keeps the slicer's words and the ★.
  locale = "en";
  applyLocale();
  const presetInEnglish = label();
  const slider = view.querySelector(".knowledge-slicer-slider");
  slider.value = "40";
  slider.dispatchEvent(new Event("input", { bubbles: true }));
  applyLocale();
  const percentInEnglish = label();
  const star = view.querySelector(".knowledge-save-phrase").textContent;
  locale = "ko";
  applyLocale();
  await preset("all");
  return { got, recent, old, dim: Number(getComputedStyle(view).getPropertyValue("--knowledge-dim-opacity")),
    presetInEnglish, percentInEnglish, star, reset: lit().length };
});
ok("slicer presets keep the pages edited within that wall-clock window, and the slicer keeps its words across a language change",
  JSON.stringify(slicerPresets.got["1h"].lit) === JSON.stringify([4])
    && JSON.stringify(slicerPresets.got["24h"].lit) === JSON.stringify([3, 4])
    && JSON.stringify(slicerPresets.got["7d"].lit) === JSON.stringify([2, 3, 4, 5])
    && JSON.stringify(slicerPresets.got["30d"].lit) === JSON.stringify([2, 3, 4, 5])
    && slicerPresets.got["24h"].label === "24시간"
    && slicerPresets.recent === 1
    && Math.abs(slicerPresets.old - slicerPresets.dim) < 0.01
    && slicerPresets.presetInEnglish === "24h"
    && slicerPresets.percentInEnglish === "40%"
    && slicerPresets.star.startsWith("★")
    && slicerPresets.reset === 6,
  JSON.stringify(slicerPresets));

await page.setViewportSize({ width: 1600, height: 1000 });
await page.emulateMedia({ reducedMotion: "reduce" });
await page.evaluate(() => {
  setPanelFolded("aside", true);
  knowledgeQuery = "";
  knowledgeTagsPicked.clear();
  knowledgeSelectedKey = null;
  knowledgeClusterPicked = -1;
  window.__VAULT__ = { pages: 180, linksPer: 3, ghosts: 12,
    tags: ["설계", "개발", "독서", "기록", "도구"], allTyped: true,
    titles: Array.from({ length: 180 }, (_, at) => `${["지식 그래프에서 연결을 읽는 방법", "작은 모듈과 명확한 인터페이스", "에이전트 실행과 결과 검증", "문서에서 다음 질문을 찾기", "매일 쌓이는 기록 정리"][at % 5]} ${at}`) };
  dropTab("knowledge");
  document.getElementById("nav-knowledge").click();
});
await page.waitForFunction(() => {
  const view = document.querySelector(".knowledge-view:not([hidden])");
  return knowledgeLayouts.get(view)?.count === 192;
});
await page.locator(".knowledge-view:not([hidden]) .knowledge-zoom-fit").click();
await page.waitForFunction(() => {
  const layout = knowledgeLayouts.get(document.querySelector(".knowledge-view:not([hidden])"));
  return layout?.left === 0 && layout.flight === null && layout.zoom === 1;
});
await page.mouse.move(0, 0);
const quietThemes = [];
for (const theme of ["dark", "light"]) {
  await page.evaluate((next) => setTheme(next), theme);
  await page.waitForTimeout(700);
  quietThemes.push(await page.evaluate(() => {
    const view = document.querySelector(".knowledge-view:not([hidden])");
    const shown = (selector) => [...view.querySelectorAll(selector)]
      .filter((node) => getComputedStyle(node).display !== "none").length;
    return {
      search: {
        appearance: getComputedStyle(view.querySelector(".knowledge-query")).appearance,
        height: parseFloat(getComputedStyle(view.querySelector(".knowledge-query")).height),
      },
      background: getComputedStyle(view.querySelector(".knowledge-canvas")).backgroundImage,
      halos: shown(".knowledge-halo"),
      nebulas: shown(".knowledge-nebula"),
      clusterLabels: shown(".knowledge-cluster-label"),
      labels: shown(".knowledge-label"),
      nodes: view.querySelectorAll(".knowledge-node").length,
      /* 이름표끼리 겹치는가 — 군집의 이름도 같은 격자를 쓰므로 함께 잰다(09-16). */
      labelOverlaps: (() => {
        const boxes = [...view.querySelectorAll(".knowledge-label, .knowledge-cluster-name, .knowledge-cluster-count")]
          .filter((one) => getComputedStyle(one).display !== "none" && one.getClientRects().length > 0)
          .map((one) => one.getBoundingClientRect());
        let hits = 0;
        for (let left = 0; left < boxes.length; left += 1) {
          for (let right = left + 1; right < boxes.length; right += 1) {
            const a = boxes[left];
            const b = boxes[right];
            if (a.right > b.left && b.right > a.left && a.bottom > b.top && b.bottom > a.top) hits += 1;
          }
        }
        return hits;
      })(),
      /* 빛 번짐이 없다 — 점에도 성운에도 필터가 걸리지 않는다. */
      nebulaFilter: view.querySelector(".knowledge-nebula")
        ? getComputedStyle(view.querySelector(".knowledge-nebula")).filter : "none",
      dots: [...view.querySelectorAll(".knowledge-node.is-page .knowledge-dot")]
        .every((dot) => !getComputedStyle(dot).fill.includes("url(") && getComputedStyle(dot).filter === "none"),
      noOverflow: view.scrollWidth <= view.clientWidth,
    };
  }));
  await page.screenshot({ path: join(UI, "..", `output/playwright/knowledge-graph-obsidian-${theme}.png`) });
}
const quietInteractions = await page.evaluate(async () => {
  const view = document.querySelector(".knowledge-view:not([hidden])");
  const layout = knowledgeLayouts.get(view);
  /* 확대가 세부를 드러내는가 — 세는 것은 **몫**이다(09-16). 들어가면 화면에 남는
     점이 줄므로 이름의 절대 수는 줄 수 있고, 늘어야 하는 것은 「화면에 있는 점 중
     이름을 얻은 몫」이다. */
  const named = () => {
    const box = view.querySelector(".knowledge-canvas").getBoundingClientRect();
    let onScreen = 0;
    let withName = 0;
    for (const node of view.querySelectorAll(".knowledge-node")) {
      const seat = node.getBoundingClientRect();
      if (seat.right < box.left || seat.left > box.right
        || seat.bottom < box.top || seat.top > box.bottom) continue;
      onScreen += 1;
      if (node.classList.contains("is-named")) withName += 1;
    }
    return onScreen === 0 ? 0 : withName / onScreen;
  };
  /* 이름이 서지 않은 잎 하나 — 짚으면 그 이름이 선다(09-16). 옛 「data-label-tier」
     문턱이 사라진 자리: 이름이 서는지는 차수가 아니라 격자가 정한다. */
  const node = view.querySelector('.knowledge-node[data-tier="leaf"]:not(.is-named)')
    ?? view.querySelector(".knowledge-node");
  node.dispatchEvent(new PointerEvent("pointermove", { bubbles: true }));
  await new Promise((done) => requestAnimationFrame(done));
  const hoverLabel = getComputedStyle(node.querySelector(".knowledge-label")).display;
  view.querySelector(".knowledge-canvas").dispatchEvent(new PointerEvent("pointerleave", { bubbles: true }));
  selectKnowledgeNode(view, node.dataset.graphKey);
  await new Promise((done) => requestAnimationFrame(done));
  const selected = {
    label: getComputedStyle(node.querySelector(".knowledge-label")).display,
    halo: getComputedStyle(node.querySelector(".knowledge-halo")).display,
    animations: view.querySelector(".knowledge-picture").getAnimations({ subtree: true }).length,
    edgeMotion: [...view.querySelectorAll(".knowledge-edge.is-focus-lit")]
      .every((edge) => getComputedStyle(edge).animationName === "none"),
  };
  view.querySelector(".knowledge-canvas").dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  const fitShare = named();
  for (let press = 0; press < 12; press += 1) view.querySelector(".knowledge-zoom-in").click();
  await new Promise((done) => requestAnimationFrame(done));
  await new Promise((done) => requestAnimationFrame(done));
  return { hoverLabel, selected, zoom: layout.zoom, fitShare, zoomedShare: named(),
    zoomedLabels: [...view.querySelectorAll(".knowledge-label")]
      .filter((label) => getComputedStyle(label).display !== "none").length };
});
/* 09-16, 성좌 배치. 이 검사의 옛 계약은 「성운도 군집 이름표도 서지 않는다
   (nebulas === 0 && clusterLabels === 0)」와 「이름표는 허브의 것뿐
   (labels === hubs)」이었다. 사용자의 조건이 그 둘을 뒤집었다 — 전체 지도의 첫
   물음이 「어떤 주제들이 있는가」이고, 범례는 점을 보면서 읽을 수 없는 자리다.
   바뀌지 않은 것은 「조용함」의 뜻이다: 평평한 페인트, 빛 번짐 없음, 선택은
   움직이지 않는다. 새로 더한 것은 「겹치지 않는다」와 「확대하면 몫이 는다」. */
ok("the quiet graph uses flat paint, a named topic per disc and static selection in both themes",
  quietThemes.every((theme) => theme.search.appearance === "none" && theme.search.height === 28
    && theme.background === "none" && theme.halos === 0
    && theme.nebulas > 0 && theme.nebulas === theme.clusterLabels
    && theme.nebulaFilter === "none"
    && theme.dots && theme.noOverflow
    && theme.labelOverlaps === 0 && theme.labels < theme.nodes)
    && quietInteractions.hoverLabel === "block"
    && quietInteractions.selected.label === "block"
    && quietInteractions.selected.halo === "none"
    && quietInteractions.selected.edgeMotion && quietInteractions.selected.animations === 0
    && quietInteractions.zoomedShare > quietInteractions.fitShare,
  JSON.stringify({ quietThemes, quietInteractions }));
await page.emulateMedia({ reducedMotion: "no-preference" });

// Test 15: Saved scenes (★ button, popover, save, reload view, restore lens/chips/search/selection/camera, missing node drop, delete)
const sceneResult = await page.evaluate(async () => {
  const view = document.querySelector(".knowledge-view:not([hidden])");
  const layout = knowledgeLayouts.get(view);
  const starBtn = view?.querySelector(".knowledge-scene-toggle");
  const popover = view?.querySelector(".knowledge-scene-popover");
  const hasStar = !!starBtn && starBtn.textContent.includes("★");
  const defaultCollapsed = starBtn?.getAttribute("aria-expanded") === "false"
    && !popover?.classList.contains("is-open");

  starBtn?.click();
  const opened = starBtn?.getAttribute("aria-expanded") === "true"
    && popover?.classList.contains("is-open");

  // Configure view state
  knowledgeOrphansOnly = true;
  knowledgeTagsPicked.clear();
  knowledgeTagsPicked.add("core");
  knowledgeQuery = "Page-0004";
  const queryInput = view?.querySelector(".knowledge-query");
  if (queryInput) queryInput.value = "Page-0004";
  knowledgeFocusDepth = 2;
  await paintKnowledgeView();
  const activeLayout = knowledgeLayouts.get(view);
  const targetNodeId = "wiki/Page-0004.md";
  selectKnowledgeNode(view, targetNodeId);
  if (activeLayout) {
    activeLayout.zoom = 1.75;
    activeLayout.panX = 25;
    activeLayout.panY = -15;
    activeLayout.zoomTaken = true;
    paintKnowledgeFrame(view, activeLayout);
  }

  // Save scene
  const nameInput = popover?.querySelector(".knowledge-scene-input");
  const saveBtn = popover?.querySelector(".knowledge-scene-save");
  if (nameInput) nameInput.value = "my-focus-scene";
  saveBtn?.click();
  await new Promise((done) => setTimeout(done, 100));

  const items = [...(popover?.querySelectorAll(".knowledge-scene-item") ?? [])];
  const savedName = items[0]?.querySelector(".knowledge-scene-name")?.textContent ?? "";

  // Check persistence in the settings document (what the backend was asked to write)
  const storedJson = JSON.parse(window.__SCENE_LINES__()["/vault"] ?? "{}")
    .scenes?.some((one) => one.name === "my-focus-scene");

  // Reload the view (reset all state)
  knowledgeOrphansOnly = false;
  knowledgeTagsPicked.clear();
  knowledgeQuery = "";
  if (queryInput) queryInput.value = "";
  knowledgeFocusDepth = 1;
  selectKnowledgeNode(view, null);
  await paintKnowledgeView();
  const resetLayout = knowledgeLayouts.get(view);
  if (resetLayout) {
    resetLayout.zoom = 1;
    resetLayout.panX = 0;
    resetLayout.panY = 0;
    resetLayout.zoomTaken = false;
  }

  const resetState = {
    orphans: knowledgeOrphansOnly,
    tags: [...knowledgeTagsPicked],
    query: knowledgeQuery,
    selected: knowledgeSelectedKey,
    depth: knowledgeFocusDepth,
    zoom: resetLayout?.zoom,
    panX: resetLayout?.panX,
  };

  // Restore the scene
  items[0]?.querySelector(".knowledge-scene-load")?.click();
  await new Promise((done) => setTimeout(done, 100));

  const restoredLayout = knowledgeLayouts.get(view);
  const restoredState = {
    orphans: knowledgeOrphansOnly,
    tags: [...knowledgeTagsPicked],
    query: knowledgeQuery,
    selected: knowledgeSelectedKey,
    depth: knowledgeFocusDepth,
    zoom: restoredLayout?.zoom,
    panX: restoredLayout?.panX,
    panY: restoredLayout?.panY,
  };

  // Missing node dropped silently
  // Save another scene with a non-existent node
  if (nameInput) nameInput.value = "ghost-scene";
  selectKnowledgeNode(view, "wiki/NonExistent.md");
  saveBtn?.click();
  await new Promise((done) => setTimeout(done, 100));

  // Reload and restore ghost scene
  selectKnowledgeNode(view, targetNodeId);
  const ghostItem = [...(popover?.querySelectorAll(".knowledge-scene-item") ?? [])]
    .find((it) => it.querySelector(".knowledge-scene-name")?.textContent === "ghost-scene");
  ghostItem?.querySelector(".knowledge-scene-load")?.click();
  await new Promise((done) => setTimeout(done, 100));
  const droppedSelection = knowledgeSelectedKey === null;

  // Delete scene
  const deleteBtn = ghostItem?.querySelector(".knowledge-scene-delete");
  deleteBtn?.click();
  await new Promise((done) => setTimeout(done, 100));
  const remainingCount = popover?.querySelectorAll(".knowledge-scene-item").length ?? 0;

  return {
    hasStar,
    defaultCollapsed,
    opened,
    savedName,
    hasStore: !!storedJson,
    resetState,
    restoredState,
    droppedSelection,
    remainingCount,
  };
});
ok("saved scenes: ★ button saves view state, reloads and restores camera, lens, chips, search and selection, dropping missing nodes",
  sceneResult.hasStar
    && sceneResult.defaultCollapsed
    && sceneResult.opened
    && sceneResult.savedName === "my-focus-scene"
    && sceneResult.hasStore
    && sceneResult.resetState.orphans === false
    && sceneResult.resetState.query === ""
    && sceneResult.restoredState.orphans === true
    && sceneResult.restoredState.tags.includes("core")
    && sceneResult.restoredState.query === "Page-0004"
    && sceneResult.restoredState.selected === "wiki/Page-0004.md"
    && sceneResult.restoredState.depth === 2
    && Math.abs(sceneResult.restoredState.zoom - 1.75) < 0.05
    && sceneResult.restoredState.panX === 25
    && sceneResult.restoredState.panY === -15
    && sceneResult.droppedSelection
    && sceneResult.remainingCount === 1,
  JSON.stringify(sceneResult));

/* Test 15b: 시야를 되살리면 그 시야의 렌즈가 그대로 선다(낮은 발견).
 * 「원본도」는 답에 없는 점을 만드는 렌즈라 깃발만 세우면 원본 점이 서지 않고, 고른
 * 원본 점은 「없는 점」으로 조용히 떨어졌다. 「회상된 적 없음」(건강 카드가 여는 냉각
 * 렌즈)은 저장도 되돌리기도 안 되어, 켜 둔 채 되살린 시야는 저장한 그림과 다른 점들을
 * 세웠다. */
const sceneLens = await page.evaluate(async () => {
  const view = document.querySelector(".knowledge-view:not([hidden])");
  const popover = view.querySelector(".knowledge-scene-popover");
  const wait = (ms) => new Promise((done) => setTimeout(done, ms));
  const flag = (name) => view.querySelector(`[data-knowledge-flag="${name}"]`);
  const count = (selector) => view.querySelectorAll(selector).length;
  const save = async (name) => {
    popover.querySelector(".knowledge-scene-input").value = name;
    popover.querySelector(".knowledge-scene-save").click();
    await wait(80);
  };
  const restore = async (name) => {
    [...popover.querySelectorAll(".knowledge-scene-item")]
      .find((item) => item.querySelector(".knowledge-scene-name")?.textContent === name)
      ?.querySelector(".knowledge-scene-load")?.click();
    await wait(300);
  };
  knowledgeOrphansOnly = false;
  knowledgeTagsPicked.clear();
  knowledgeQuery = "";
  view.querySelector(".knowledge-query").value = "";
  selectKnowledgeNode(view, null);
  await paintKnowledgeView();
  // Sources: on, pick a source node, save; off; restore.
  flag("sources").click();
  await wait(300);
  selectKnowledgeNode(view, "raw/source-0.md");
  await save("with-sources");
  flag("sources").click();
  await wait(300);
  const sourcesOff = count(".knowledge-node.is-source");
  await restore("with-sources");
  const sources = {
    off: sourcesOff,
    pressed: flag("sources").getAttribute("aria-pressed"),
    drawn: count(".knowledge-node.is-source"),
    selected: knowledgeSelectedKey,
  };
  flag("sources").click();
  await wait(300);
  // Cold lens: saved off; turned on from the health card; restore brings it back off.
  selectKnowledgeNode(view, null);
  await save("cold-off");
  const ghostsBefore = count(".knowledge-node.is-ghost");
  view.querySelector('button[data-knowledge-lint="neverRecalled"]').click();
  await wait(80);
  const ghostsCold = count(".knowledge-node.is-ghost");
  await restore("cold-off");
  const cold = { ghostsBefore, ghostsCold, ghostsRestored: count(".knowledge-node.is-ghost"), lens: knowledgeColdOnly };
  for (const name of ["with-sources", "cold-off"]) {
    [...popover.querySelectorAll(".knowledge-scene-item")]
      .find((item) => item.querySelector(".knowledge-scene-name")?.textContent === name)
      ?.querySelector(".knowledge-scene-delete")?.click();
    await wait(80);
  }
  knowledgeColdOnly = false;
  knowledgeShowSources = false;
  await refreshKnowledgeGraph({ force: true });
  return { sources, cold };
});
ok("restoring a scene reproduces its lens: sources are fetched again and the cold lens is saved and put back",
  sceneLens.sources.off === 0
    && sceneLens.sources.pressed === "true"
    && sceneLens.sources.drawn > 0
    && sceneLens.sources.selected === "raw/source-0.md"
    && sceneLens.cold.ghostsBefore > 0
    && sceneLens.cold.ghostsCold === 0
    && sceneLens.cold.ghostsRestored === sceneLens.cold.ghostsBefore
    && sceneLens.cold.lens === false,
  JSON.stringify(sceneLens));

// Test 16: Search tokens (tag, rel, since, kind, hub, mixed free text, dimming principle, autocomplete chips, saved phrases)
const searchTokenResult = await page.evaluate(async () => {
  const view = document.querySelector(".knowledge-view:not([hidden])");
  const queryInput = view?.querySelector(".knowledge-query");

  // Reset any lenses or active filters
  knowledgeOrphansOnly = false;
  knowledgeGhostsOnly = false;
  knowledgeTypedOnly = false;
  knowledgeShowSources = false;
  knowledgeTagsPicked.clear();
  selectKnowledgeNode(view, null);

  // Set up synthetic vault with known tokens
  window.__VAULT__ = {
    pages: 10,
    linksPer: 2,
    ghosts: 2,
    tags: ["core", "tools", "reading"],
    allTyped: true,
  };
  await refreshKnowledgeGraph({ force: true });
  await new Promise((done) => setTimeout(done, 100));

  const totalNodes = view?.querySelectorAll(".knowledge-node").length ?? 0;
  const initialPositions = [...(view?.querySelectorAll(".knowledge-node") ?? [])]
    .map((n) => n.getAttribute("transform"));

  // Check autocomplete chips exist for token names: tag:, rel:, since:, kind:, hub
  const tokenChips = [...(view?.querySelectorAll(".knowledge-token-chip") ?? [])];
  const chipTokens = tokenChips.map((c) => c.dataset.token ?? c.textContent.trim());
  const hasAllTokenChips = ["tag:", "rel:", "since:", "kind:", "hub"].every((tok) => chipTokens.includes(tok));

  const layoutDegreeFour = () => {
    const model = knowledgeLayouts.get(view)?.model;
    return model ? [...model.degree].filter((degree) => degree >= 4).length : 0;
  };
  // Helper to run query and count matches and dims
  const runQuery = async (q) => {
    if (queryInput) {
      queryInput.value = q;
      queryInput.dispatchEvent(new Event("input", { bubbles: true }));
    }
    await new Promise((done) => setTimeout(done, 80));
    const matches = view?.querySelectorAll(".knowledge-node.is-search-match").length ?? 0;
    const dimmed = view?.querySelectorAll(".knowledge-node.is-search-dim").length ?? 0;
    const currentNodes = view?.querySelectorAll(".knowledge-node").length ?? 0;
    return { matches, dimmed, currentNodes };
  };

  // 1. tag:<t>
  const tagRes = await runQuery("tag:core");
  // 2. kind:<page|ghost|source>
  const kindRes = await runQuery("kind:ghost");
  // 3. hub — the token lights exactly the pages the picture draws as hubs (K23):
  // the drawn floor is the stricter of the hub-degree token and the top 5%, so a
  // bare `degree >= 4` reading lit most of a large vault as "hubs".
  const hubRes = await runQuery("hub");
  const keysOf = (selector) => [...(view?.querySelectorAll(selector) ?? [])]
    .map((node) => node.dataset.graphKey).sort();
  hubRes.lit = keysOf(".knowledge-node.is-search-match");
  hubRes.drawn = keysOf('.knowledge-node[data-hub="true"]');
  hubRes.degreeFour = layoutDegreeFour();
  // 4. rel:<relation> (untranslated frontmatter key)
  const relRes = await runQuery("rel:related");
  // 5. Mixed: tag:core kind:page
  const mixedRes = await runQuery("tag:core kind:page");
  // 6. Mixed with free text: hub 개념
  const freeRes = await runQuery("hub 개념");

  // Dimming principle: node count and positions remain unchanged
  const postPositions = [...(view?.querySelectorAll(".knowledge-node") ?? [])]
    .map((n) => n.getAttribute("transform"));
  const sameCoords = initialPositions.length > 0
    && initialPositions.every((pos, idx) => pos === postPositions[idx]);

  // Click an autocomplete chip
  const hubChip = tokenChips.find((c) => (c.dataset.token ?? c.textContent.trim()) === "hub");
  if (queryInput) queryInput.value = "";
  hubChip?.click();
  const queryAfterChip = queryInput?.value.trim() ?? "";

  // Saved search phrases: save current phrase
  if (queryInput) {
    queryInput.value = "tag:core hub";
    queryInput.dispatchEvent(new Event("input", { bubbles: true }));
  }
  const savePhraseBtn = view?.querySelector(".knowledge-save-phrase");
  savePhraseBtn?.click();
  await new Promise((done) => setTimeout(done, 100));

  // Check saved phrase chip appears
  const phraseChips = [...(view?.querySelectorAll(".knowledge-search-phrase-chip") ?? [])];
  const savedPhraseText = phraseChips[0]?.querySelector(".knowledge-phrase-text")?.textContent
    ?? phraseChips[0]?.textContent ?? "";

  // Check store persistence in the settings document
  const storePhrases = JSON.parse(window.__SCENE_LINES__()["/vault"] ?? "{}").phrases ?? [];

  // Clear search and click saved phrase chip to apply
  if (queryInput) {
    queryInput.value = "";
    queryInput.dispatchEvent(new Event("input", { bubbles: true }));
  }
  const clickTarget = phraseChips[0]?.querySelector(".knowledge-phrase-text") ?? phraseChips[0];
  clickTarget?.click();
  await new Promise((done) => setTimeout(done, 80));
  const appliedQuery = queryInput?.value ?? "";

  // Delete saved phrase
  const deletePhraseBtn = phraseChips[0]?.querySelector(".knowledge-delete-phrase");
  deletePhraseBtn?.click();
  await new Promise((done) => setTimeout(done, 100));
  const remainingPhrases = view?.querySelectorAll(".knowledge-search-phrase-chip").length ?? 0;

  return {
    totalNodes,
    hasAllTokenChips,
    tagRes,
    kindRes,
    hubRes,
    relRes,
    mixedRes,
    freeRes,
    sameCoords,
    queryAfterChip,
    savedPhraseText,
    storeHasPhrase: storePhrases.includes("tag:core hub"),
    appliedQuery,
    remainingPhrases,
  };
});
ok("search tokens: tag, rel, since, kind, hub and free text dim without moving nodes, with autocomplete and saved phrase chips",
  searchTokenResult.hasAllTokenChips
    && searchTokenResult.tagRes.matches > 0
    && searchTokenResult.tagRes.dimmed > 0
    && searchTokenResult.tagRes.matches + searchTokenResult.tagRes.dimmed === searchTokenResult.totalNodes
    && searchTokenResult.kindRes.matches === 2 // 2 ghosts in synthetic vault
    && searchTokenResult.hubRes.matches > 0
    && searchTokenResult.hubRes.drawn.length > 0
    && searchTokenResult.hubRes.degreeFour > searchTokenResult.hubRes.drawn.length
    && JSON.stringify(searchTokenResult.hubRes.lit) === JSON.stringify(searchTokenResult.hubRes.drawn)
    && searchTokenResult.relRes.matches > 0
    && searchTokenResult.mixedRes.matches > 0
    && searchTokenResult.mixedRes.matches <= searchTokenResult.tagRes.matches
    && searchTokenResult.sameCoords
    && searchTokenResult.queryAfterChip.includes("hub")
    && searchTokenResult.savedPhraseText.includes("tag:core hub")
    && searchTokenResult.storeHasPhrase
    && searchTokenResult.appliedQuery === "tag:core hub"
    && searchTokenResult.remainingPhrases === 0,
  JSON.stringify(searchTokenResult));

/* Test 16b: 태그 칩과 검색 상자는 같은 것을 말한다(낮은 발견 둘).
 * (1) 토큰 칩이나 저장된 검색어가 검색어를 바꾸면 켜져 있던 태그 칩은 꺼진다 — 꺼지지
 *     않은 칩을 다시 누르면 태그를 다시 거는 대신 검색어를 지웠다.
 * (2) 볼트의 태그가 검색 문법의 낱말(`hub`)과 같아도 그 칩은 그 태그의 페이지를 밝힌다. */
const chipQuery = await page.evaluate(async () => {
  const view = document.querySelector(".knowledge-view:not([hidden])");
  const find = view.querySelector(".knowledge-query");
  const wait = (ms) => new Promise((done) => setTimeout(done, ms));
  knowledgeTagsPicked.clear();
  find.value = "";
  find.dispatchEvent(new Event("input", { bubbles: true }));
  window.__VAULT__ = { pages: 12, linksPer: 2, ghosts: 0, tags: ["hub", "core", "tools"], allTyped: true };
  await refreshKnowledgeGraph({ force: true });
  await wait(80);
  const chip = (tag) => view.querySelector(`.knowledge-chip[data-knowledge-tag="${tag}"]`);
  const pressed = (tag) => chip(tag)?.getAttribute("aria-pressed") === "true";
  // (1a) token chip
  chip("core").click();
  await wait(50);
  const coreOn = { pressed: pressed("core"), query: find.value };
  view.querySelector('.knowledge-token-chip[data-token="kind:"]').click();
  await wait(50);
  const afterToken = { pressed: pressed("core"), query: find.value };
  chip("core").click();
  await wait(50);
  const coreAgain = { pressed: pressed("core"), query: find.value };
  // (1b) saved phrase chip
  find.value = "rel:related";
  find.dispatchEvent(new Event("input", { bubbles: true }));
  view.querySelector(".knowledge-save-phrase").click();
  await wait(80);
  chip("core").click();
  await wait(50);
  const phrase = [...view.querySelectorAll(".knowledge-search-phrase-chip")]
    .find((one) => one.querySelector(".knowledge-phrase-text")?.textContent === "rel:related");
  phrase?.querySelector(".knowledge-phrase-text")?.click();
  await wait(50);
  const afterPhrase = { pressed: pressed("core"), query: find.value };
  phrase?.querySelector(".knowledge-delete-phrase")?.click();
  await wait(80);
  // (2) the tag named like a token
  chip("hub").click();
  await wait(50);
  const tagged = knowledgeLayouts.get(view).model;
  const hubTagged = tagged.keys.filter((key, at) => tagged.tags[at].includes("hub")).sort();
  const hubLit = [...view.querySelectorAll(".knowledge-node.is-search-match")]
    .map((node) => node.dataset.graphKey).sort();
  const hubChip = { pressed: pressed("hub"), query: find.value };
  chip("hub").click();
  await wait(50);
  return { coreOn, afterToken, coreAgain, afterPhrase, hubTagged, hubLit, hubChip, cleared: find.value };
});
ok("tag chips follow the search box: a token or saved phrase releases the lit chip, and a tag named like a token still finds its pages",
  chipQuery.coreOn.pressed && chipQuery.coreOn.query === "core"
    && !chipQuery.afterToken.pressed && chipQuery.afterToken.query === "core kind:"
    && chipQuery.coreAgain.pressed && chipQuery.coreAgain.query === "core"
    && !chipQuery.afterPhrase.pressed && chipQuery.afterPhrase.query === "rel:related"
    && chipQuery.hubTagged.length === 4
    && JSON.stringify(chipQuery.hubLit) === JSON.stringify(chipQuery.hubTagged)
    && chipQuery.hubChip.pressed
    && chipQuery.cleared === "",
  JSON.stringify(chipQuery));

/* ---- t-4140 S1: 「전체 지도 | 주변 탐색」 ----
 *
 * 주변 탐색은 흐리기가 아니다: 선택 노드와 깊이 안의 하위 그래프만 DOM에 서고
 * 나머지 점·선은 빠진다. 좌표·군집·카메라는 전체 지도의 것이라 돌아오면 그대로다.
 * 빵부스러기가 중심을 말하고, ← 이전이 중심·깊이·배율을 되짚는다. */
const explore = await page.evaluate(async () => {
  const viewOf = () => document.querySelector(".knowledge-view:not([hidden])");
  const wait = (ms) => new Promise((done) => setTimeout(done, ms));
  const count = (selector) => viewOf().querySelectorAll(selector).length;
  knowledgeQuery = "";
  knowledgeOrphansOnly = knowledgeGhostsOnly = knowledgeTypedOnly = false;
  knowledgeAliveOnly = knowledgeColdOnly = knowledgeMergeOnly = false;
  knowledgeShowSources = false;
  knowledgeLintLens = knowledgeSelectedKey = knowledgePath = null;
  knowledgeTagsPicked.clear();
  knowledgeFocusDepth = 1;
  viewOf()?.querySelector(".knowledge-query")?.setAttribute("value", "");
  window.__VAULT__ = { pages: 40, linksPer: 2, ghosts: 2, tags: ["core", "reading"] };
  knowledgeAskedAt = 0;
  dropTab("knowledge");
  el("nav-knowledge").click();
  for (let round = 0; round < 300; round += 1) {
    await wait(20);
    const held = viewOf() ? knowledgeLayouts.get(viewOf()) : null;
    if (held && held.count === 42 && held.left === 0) break;
  }
  const view = viewOf();
  const layout = knowledgeLayouts.get(view);
  const model = layout.model;
  const modeButtons = [...view.querySelectorAll(".knowledge-mode-step")]
    .map((one) => `${one.dataset.knowledgeMode}:${one.getAttribute("aria-pressed")}`);
  const crumbHiddenInGlobal = view.querySelector(".knowledge-crumb")?.hidden ?? null;
  const before = { nodes: count(".knowledge-node"), edges: count(".knowledge-edge") };
  const firstEls = [...view.querySelectorAll(".knowledge-node")].slice(0, 5);
  const centre = "wiki/Page-0000.md";
  const seat = model.keys.indexOf(centre);
  selectKnowledgeNode(view, centre);
  layout.zoom = 1.3;
  layout.panX = 20;
  layout.panY = -10;
  layout.zoomTaken = true;
  paintKnowledgeFrame(view, layout);
  const xs = Array.from(layout.x);
  const ys = Array.from(layout.y);
  const community = Array.from(layout.community);
  /* 기대하는 하위 그래프: 중심 + 직접 이웃, 그 안의 선. */
  const near = knowledgeNeighbours(layout, seat);
  let nearEdges = 0;
  for (let at = 0; at < model.edgeCount; at += 1) {
    if (near.nodes.has(model.from[at]) && near.nodes.has(model.to[at])) nearEdges += 1;
  }
  /* 고리(시안 이식): 깊이 k의 점은 k번째 고리 위에 고르게, 중심은 제 지도 자리에,
   * 두 축 다 판 안에(contain), 지도의 좌표는 스태시에 그대로. */
  const ringOf = (level) => {
    const l = knowledgeLayouts.get(view);
    const ring = l.ring;
    const seats = [];
    const shown = [];
    for (let at = 0; at < l.count; at += 1) {
      if (l.drawn[at] !== 1) continue;
      shown.push(at);
      if (at !== seat && l.drawnLevel[at] === level) seats.push(at);
    }
    const radius = ring.radii[level] ?? null;
    const radii = seats.map((at) => Math.hypot(l.x[at] - ring.x, (l.y[at] - ring.y) / ring.squash));
    const angles = seats.map((at) => Math.atan2((l.y[at] - ring.y) / ring.squash, l.x[at] - ring.x))
      .sort((left, right) => left - right);
    let gapMin = Infinity;
    let gapMax = 0;
    for (let at = 0; at < angles.length; at += 1) {
      const gap = at === 0 ? angles[0] + 2 * Math.PI - angles[angles.length - 1] : angles[at] - angles[at - 1];
      gapMin = Math.min(gapMin, gap);
      gapMax = Math.max(gapMax, gap);
    }
    const box = view.querySelector(".knowledge-picture").getAttribute("viewBox").split(" ").map(Number);
    return {
      count: seats.length,
      radius,
      onRing: radii.every((one) => Math.abs(one - radius) < 0.5),
      floor: radius >= l.tuning.ringMin - 1e-6
        && radius >= (seats.length * l.tuning.ringPitch) / (2 * Math.PI) - 1e-3,
      even: seats.length < 2 || gapMax - gapMin < 1e-3,
      centred: l.x[seat] === xs[seat] && l.y[seat] === ys[seat],
      inside: shown.every((at) => l.x[at] >= box[0] && l.x[at] <= box[0] + box[2]
        && l.drawY[at] >= box[1] && l.drawY[at] <= box[1] + box[3]),
      mapKept: xs.every((one, at) => one === l.mapX[at]) && ys.every((one, at) => one === l.mapY[at]),
    };
  };
  view.querySelector('.knowledge-mode-step[data-knowledge-mode="local"]').click();
  await wait(80);
  const drawnKeys = [...view.querySelectorAll(".knowledge-node")].map((one) => one.dataset.graphKey);
  const depth1 = {
    mode: knowledgeMode,
    nodes: count(".knowledge-node"),
    edges: count(".knowledge-edge"),
    expectedNodes: near.nodes.size,
    expectedPages: [...near.nodes].filter((at) => model.kinds[at] === "page").length,
    expectedEdges: nearEdges,
    onlyNear: drawnKeys.every((key) => near.nodes.has(model.keys.indexOf(key))),
    dimmed: count(".knowledge-node.is-search-dim, .knowledge-picture.is-focused .knowledge-node:not(.is-focus-lit)"),
    pressed: [...view.querySelectorAll(".knowledge-mode-step")]
      .map((one) => `${one.dataset.knowledgeMode}:${one.getAttribute("aria-pressed")}`),
    crumbHidden: view.querySelector(".knowledge-crumb").hidden,
    crumb: view.querySelector(".knowledge-crumb-here").textContent,
    backDisabled: view.querySelector(".knowledge-crumb-back").disabled,
    stat: view.querySelector(".knowledge-stat").textContent,
    zoomReset: knowledgeLayouts.get(view).zoom === 1 && knowledgeLayouts.get(view).panX === 0,
    sameLayout: knowledgeLayouts.get(view) === layout,
  };
  const ring1 = ringOf(1);
  view.querySelector('[data-knowledge-depth="2"]').click();
  await wait(80);
  const depth2 = { nodes: count(".knowledge-node"), edges: count(".knowledge-edge"), depth: knowledgeFocusDepth };
  const ring2 = { inner: ringOf(1), outer: ringOf(2) };
  const other = [...view.querySelectorAll(".knowledge-node")].find((one) => one.dataset.graphKey !== centre);
  other.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  await wait(80);
  const recentred = {
    selected: knowledgeSelectedKey,
    expected: other.dataset.graphKey,
    crumb: view.querySelector(".knowledge-crumb-here").textContent,
    title: model.titles[model.keys.indexOf(other.dataset.graphKey)],
    history: knowledgeHistory.length,
    backDisabled: view.querySelector(".knowledge-crumb-back").disabled,
  };
  view.querySelector(".knowledge-crumb-back").click();
  await wait(80);
  const backed = { selected: knowledgeSelectedKey, depth: knowledgeFocusDepth, history: knowledgeHistory.length };
  const held = knowledgeLayouts.get(view);
  /* 주변 탐색 안에서 지도의 자리는 스태시에 산다 — 고리가 그것을 건드리지 않는다. */
  const positionsKept = xs.every((x, at) => x === held.mapX[at]) && ys.every((y, at) => y === held.mapY[at]);
  view.querySelector('.knowledge-mode-step[data-knowledge-mode="global"]').click();
  await wait(80);
  const back = knowledgeLayouts.get(view);
  const after = {
    mode: knowledgeMode,
    nodes: count(".knowledge-node"),
    edges: count(".knowledge-edge"),
    zoom: back.zoom,
    panX: back.panX,
    panY: back.panY,
    crumbHidden: view.querySelector(".knowledge-crumb").hidden,
    communityKept: community.every((rank, at) => rank === back.community[at]),
    positionsKept: xs.every((x, at) => x === back.x[at]) && ys.every((y, at) => y === back.y[at]),
    released: back.ring === null && back.mapX === null && back.mapY === null,
    retained: [...view.querySelectorAll(".knowledge-node")].slice(0, 5)
      .every((one, at) => one === firstEls[at]),
    selected: knowledgeSelectedKey,
  };
  knowledgeFocusDepth = 1;
  selectKnowledgeNode(view, null);
  return { modeButtons, crumbHiddenInGlobal, before, depth1, ring1, depth2, ring2, recentred, backed, positionsKept, after };
});
ok("t-4140 S1: 주변 탐색 draws only the centre's subgraph, breadcrumb and ← 이전 walk the visits, and the whole map returns with its positions, clusters and camera",
  explore.modeButtons.join(",") === "global:true,local:false"
    && explore.crumbHiddenInGlobal === true
    && explore.before.nodes === 42
    && explore.depth1.mode === "local"
    && explore.depth1.nodes === explore.depth1.expectedNodes
    && explore.depth1.edges === explore.depth1.expectedEdges
    && explore.depth1.nodes < explore.before.nodes
    && explore.depth1.onlyNear
    && explore.depth1.dimmed === 0
    && explore.depth1.pressed.join(",") === "global:false,local:true"
    && explore.depth1.crumbHidden === false
    && explore.depth1.crumb === "개념 0"
    && explore.depth1.backDisabled === true
    && explore.depth1.stat.includes(`${explore.depth1.expectedPages}/40`)
    && explore.depth1.zoomReset
    && explore.depth1.sameLayout
    && explore.ring1.count === explore.depth1.nodes - 1
    && explore.ring1.onRing && explore.ring1.floor && explore.ring1.even
    && explore.ring1.centred && explore.ring1.inside && explore.ring1.mapKept
    && explore.depth2.nodes > explore.depth1.nodes
    && explore.depth2.depth === 2
    && explore.ring2.inner.count + explore.ring2.outer.count === explore.depth2.nodes - 1
    && explore.ring2.outer.count > 0
    && explore.ring2.outer.radius > explore.ring2.inner.radius
    && explore.ring2.inner.onRing && explore.ring2.outer.onRing
    && explore.ring2.outer.floor && explore.ring2.outer.even
    && explore.ring2.outer.inside && explore.ring2.outer.mapKept
    && explore.recentred.selected === explore.recentred.expected
    && explore.recentred.crumb === explore.recentred.title
    && explore.recentred.history === 1
    && explore.recentred.backDisabled === false
    && explore.backed.selected === "wiki/Page-0000.md"
    && explore.backed.depth === 2
    && explore.backed.history === 0
    && explore.positionsKept
    && explore.after.mode === "global"
    && explore.after.nodes === 42
    && explore.after.edges === explore.before.edges
    && explore.after.zoom === 1.3 && explore.after.panX === 20 && explore.after.panY === -10
    && explore.after.crumbHidden === true
    && explore.after.communityKept
    && explore.after.positionsKept
    && explore.after.released
    && explore.after.retained
    && explore.after.selected === "wiki/Page-0000.md",
  JSON.stringify(explore));

/* 고리가 선 동안의 훑기는 지도의 것이다: 주변 탐색 안에서 남은 훑기가 돌아도 화면의
 * 고리는 그대로고, 전체 지도(`mapX`·`mapY`)가 뒤에서 앉는다 — 첫 화면이 주변 탐색인
 * 볼트도 전체 지도로 나가면 이미 앉은 지도를 만난다. 자리(`placed`)는 지도의 것이다. */
const mapSettle = await page.evaluate(async () => {
  const view = document.querySelector(".knowledge-view:not([hidden])");
  const wait = (ms) => new Promise((done) => setTimeout(done, ms));
  const layout = knowledgeLayouts.get(view);
  const centre = "wiki/Page-0000.md";
  const seat = layout.model.keys.indexOf(centre);
  selectKnowledgeNode(view, centre);
  view.querySelector('.knowledge-mode-step[data-knowledge-mode="local"]').click();
  await wait(80);
  const ringX = Array.from(layout.x);
  const ringY = Array.from(layout.y);
  const mapBefore = Array.from(layout.mapX);
  layout.left = 30;
  knowledgeSettle(view);
  for (let round = 0; round < 300 && layout.left > 0; round += 1) await wait(20);
  const during = {
    left: layout.left,
    ringStands: layout.ring !== null,
    ringStill: ringX.every((x, at) => x === layout.x[at]) && ringY.every((y, at) => y === layout.y[at]),
    mapMoved: mapBefore.some((x, at) => x !== layout.mapX[at]),
    placedIsMap: layout.placed.get(centre)?.[0] === layout.mapX[seat]
      && layout.placed.get(centre)?.[1] === layout.mapY[seat],
  };
  const settled = Array.from(layout.mapX);
  view.querySelector('.knowledge-mode-step[data-knowledge-mode="global"]').click();
  await wait(80);
  const after = {
    mapShown: settled.every((x, at) => x === layout.x[at]),
    released: layout.ring === null && layout.mapX === null,
    left: layout.left,
  };
  selectKnowledgeNode(view, null);
  return { during, after };
});
ok("t-4140 S1: the settle that runs while the ring stands moves the whole map behind it, never the ring, and the whole map returns already seated",
  mapSettle.during.left === 0
    && mapSettle.during.ringStands
    && mapSettle.during.ringStill
    && mapSettle.during.mapMoved
    && mapSettle.during.placedIsMap
    && mapSettle.after.mapShown
    && mapSettle.after.released
    && mapSettle.after.left === 0,
  JSON.stringify(mapSettle));

/* Long Korean titles and a folded hub reproduce the crowded card in the real
 * vault. Measure the rendered boxes, including the narrow stacked inspector. */
const readableLocal = await page.evaluate(async () => {
  const view = document.querySelector(".knowledge-view:not([hidden])");
  const ready = async (condition) => {
    for (let frame = 0; frame < 600 && !condition(); frame += 1) {
      await new Promise(requestAnimationFrame);
    }
    if (!condition()) throw new Error("Knowledge readability fixture did not settle");
  };
  const edges = Array.from({ length: 8 }, (_, i) => ({ from: 0, to: i + 1, kind: "related" }));
  for (let i = 9; i < 30; i += 1) edges.push({ from: 1, to: i, kind: "depends_on" });
  edges.push({ from: 2, to: 3, kind: "implements" });
  window.__VAULT__ = { pages: 30, ghosts: 0, tags: ["design"], customEdges: edges,
    titles: Array.from({ length: 30 }, (_, i) => `페이지 ${i}: 컴퓨터 사용의 요청 처리와 화면 관찰은 서로 다른 작업이다`) };
  knowledgeAskedAt = 0;
  view.querySelector(".knowledge-refresh").click();
  await ready(() => knowledgeLayouts.get(view)?.count === 30 && knowledgeLayouts.get(view)?.left === 0);
  knowledgeFocusDepth = 2;
  setKnowledgeMode(view, "local", { centre: "wiki/Page-0000.md" });
  const originalStyle = view.getAttribute("style");
  const measures = [];
  for (const width of [1260, 800]) {
    Object.assign(view.style, { position: "fixed", inset: "0", width: `${width}px`, height: "760px", zIndex: "100" });
    paintKnowledgeView(view);
    await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
    const canvas = view.querySelector(".knowledge-canvas").getBoundingClientRect();
    const labels = [...view.querySelectorAll(".knowledge-label")].map((one) => one.getBoundingClientRect());
    const controls = [".knowledge-depth", ".knowledge-crumb", ".knowledge-zoom"]
      .map((selector) => view.querySelector(selector)?.getBoundingClientRect()).filter(Boolean);
    const overlaps = (a, b) => a.left < b.right && a.right > b.left && a.top < b.bottom && a.bottom > b.top;
    const inspector = view.querySelector(".knowledge-inspector");
    const rows = [...view.querySelectorAll(".knowledge-relation-row")];
    const note = rows[0].querySelector(".knowledge-inspector-note").getBoundingClientRect();
    const title = rows[0].querySelector(".knowledge-inspector-row").getBoundingClientRect();
    measures.push({ width,
      labelsContained: labels.every((r) => r.left >= canvas.left && r.right <= canvas.right
        && r.top >= canvas.top && r.bottom <= canvas.bottom),
      labelsSeparate: labels.every((a, i) => labels.slice(i + 1).every((b) => !overlaps(a, b))),
      controlsClear: labels.every((a) => controls.every((b) => !overlaps(a, b))),
      cardContained: inspector.scrollWidth <= inspector.clientWidth,
      relationBelowTitle: note.top >= title.bottom,
      folded: rows.some((row) => row.querySelector(".knowledge-unfold")),
      actionsBeforeRelations: !!(view.querySelector(".knowledge-inspector-acts")
        .compareDocumentPosition(view.querySelector(".knowledge-relations-outgoing")) & Node.DOCUMENT_POSITION_FOLLOWING),
    });
  }
  if (originalStyle === null) view.removeAttribute("style");
  else view.setAttribute("style", originalStyle);
  setKnowledgeMode(view, "global");
  /* 09-16: 전체 지도의 이름표는 네 자리 중 하나에 선다(아래·위·오른쪽·왼쪽,
     `placeKnowledgeLabels`). 고리에서 돌아온 이름은 그 넷 중 하나이거나, 서지
     않는 이름이면 기본값(가운데·x 0)으로 되돌아간다 — 고리의 자세가 남지 않는다. */
  const restored = [...view.querySelectorAll(".knowledge-label")].every((one) => {
    const anchor = one.getAttribute("text-anchor");
    const named = one.closest(".knowledge-node").classList.contains("is-named");
    if (!named) return anchor === "middle" && Number(one.getAttribute("x")) === 0;
    return ["middle", "start", "end"].includes(anchor);
  });
  knowledgeFocusDepth = 1;
  selectKnowledgeNode(view, null);
  return { measures, restored };
});
ok("long local labels clear one another and the controls, folded relation metadata fits below its title, and global labels recover their anchors",
  readableLocal.restored && readableLocal.measures.every((one) => one.labelsContained
    && one.labelsSeparate && one.controlsClear && one.cardContained && one.relationBelowTitle
    && one.folded && one.actionsBeforeRelations), JSON.stringify(readableLocal));

/* 고차수 허브는 접힌다: 깊이 2에서 허브의 이웃은 그려지지 않고, 허브가 「+N」을
 * 달며, 카드의 줄이 관계 종류별 묶음과 「펼치기」를 든다 — 펼치기는 명시적이다. */
const folding = await page.evaluate(async () => {
  const viewOf = () => document.querySelector(".knowledge-view:not([hidden])");
  const wait = (ms) => new Promise((done) => setTimeout(done, ms));
  const count = (selector) => viewOf().querySelectorAll(selector).length;
  const edges = [{ from: 0, to: 1, kind: "mentions" }, { from: 0, to: 17, kind: "related" }];
  for (let at = 2; at <= 9; at += 1) edges.push({ from: 1, to: at, kind: "mentions" });
  for (let at = 10; at <= 16; at += 1) edges.push({ from: 1, to: at, kind: "related" });
  window.__VAULT__ = { pages: 20, ghosts: 0, tags: ["core"], customEdges: edges };
  knowledgeAskedAt = 0;
  viewOf().querySelector(".knowledge-refresh").click();
  for (let round = 0; round < 300; round += 1) {
    await wait(20);
    const held = knowledgeLayouts.get(viewOf());
    if (held && held.count === 20 && held.left === 0) break;
  }
  const view = viewOf();
  const hub = "wiki/Page-0001.md";
  const foldFloor = KNOWLEDGE_EXPLORE.foldDegree;
  const hubDegree = knowledgeLayouts.get(view).model.degree[1];
  selectKnowledgeNode(view, "wiki/Page-0000.md");
  knowledgeFocusDepth = 2;
  view.querySelector('.knowledge-mode-step[data-knowledge-mode="local"]').click();
  await wait(80);
  const hubNode = view.querySelector(`.knowledge-node[data-graph-key="${hub}"]`);
  const folded = {
    nodes: count(".knowledge-node"),
    edges: count(".knowledge-edge"),
    hubFolded: hubNode?.classList.contains("is-folded") ?? null,
    omitted: hubNode?.dataset.folded ?? null,
    label: hubNode?.querySelector(".knowledge-fold")?.textContent ?? "",
    crumbCount: view.querySelector(".knowledge-crumb-count").textContent,
  };
  const row = [...view.querySelectorAll(".knowledge-relations-outgoing li")]
    .find((one) => one.querySelector("[data-knowledge-key]")?.dataset.knowledgeKey === hub);
  const unfold = row?.querySelector("[data-knowledge-unfold]") ?? null;
  const bundle = row?.querySelector(".knowledge-inspector-note")?.textContent ?? "";
  const unfoldWord = unfold?.textContent ?? "";
  unfold?.click();
  await wait(80);
  const unfolded = {
    nodes: count(".knowledge-node"),
    edges: count(".knowledge-edge"),
    hubFolded: view.querySelector(`.knowledge-node[data-graph-key="${hub}"]`)?.classList.contains("is-folded"),
    unfoldGone: [...view.querySelectorAll(".knowledge-relations-outgoing li")]
      .find((one) => one.querySelector("[data-knowledge-key]")?.dataset.knowledgeKey === hub)
      ?.querySelector("[data-knowledge-unfold]") === null,
  };
  /* 새 중심으로 가면 접힘은 다시 접힌다 — 펼침은 이 탐색의 것이다. */
  view.querySelector(`.knowledge-node[data-graph-key="wiki/Page-0017.md"]`)
    .dispatchEvent(new MouseEvent("click", { bubbles: true }));
  await wait(80);
  const moved = { unfoldedSize: knowledgeUnfolded.size, nodes: count(".knowledge-node") };
  view.querySelector('.knowledge-mode-step[data-knowledge-mode="global"]').click();
  await wait(80);
  knowledgeFocusDepth = 1;
  selectKnowledgeNode(view, null);
  return { foldFloor, hubDegree, folded, bundle, unfoldWord, unfolded, moved, all: count(".knowledge-node"),
    unfoldExpected: t("knowledge.unfold", "펼치기") };
});
ok("t-4140 S1: a high-degree hub inside the neighbourhood stays folded with its omitted links bundled by kind until 펼치기 is pressed",
  folding.hubDegree >= folding.foldFloor
    && folding.folded.nodes === 3
    && folding.folded.edges === 2
    && folding.folded.hubFolded === true
    && folding.folded.omitted === "15"
    && folding.folded.label === "+15"
    && folding.bundle.includes("mentions 8") && folding.bundle.includes("related 7")
    && folding.unfoldWord === folding.unfoldExpected
    && folding.unfolded.nodes === 18
    && folding.unfolded.edges === 17
    && folding.unfolded.hubFolded === false
    && folding.unfolded.unfoldGone
    && folding.moved.unfoldedSize === 0
    && folding.moved.nodes === 3
    && folding.all === 20,
  JSON.stringify(folding));

/* ---- t-4140 S2: 진입 규칙 ----
 *
 * 첫 방문은 주변 탐색(승인된 시안의 화면; 중심은 마지막 회상 → 색인 아닌 최다 연결),
 * 「연결 보기」로 들어오면 그 페이지의 주변 탐색, 이후는 이 볼트의 마지막 모드 — 설정
 * 문서의 한 줄(`second_brain_explore`, 볼트별 JSON 한 줄, 시야와 같은 자리)이 그것을
 * 든다. 노드 수는 어디에도 없다: 규칙은 순수 함수 하나다. */
const entry = await page.evaluate(async () => {
  const viewOf = () => document.querySelector(".knowledge-view:not([hidden])");
  const wait = (ms) => new Promise((done) => setTimeout(done, ms));
  const count = (selector) => viewOf().querySelectorAll(selector).length;
  const settle = async (pages) => {
    for (let round = 0; round < 300; round += 1) {
      await wait(20);
      const held = viewOf() ? knowledgeLayouts.get(viewOf()) : null;
      if (held && held.count === pages && held.left === 0 && knowledgeReport !== null) break;
    }
  };
  const stored = () => JSON.parse(window.__EXPLORE_LINES__()["/vault"] ?? "null");
  const rule = [
    knowledgeStartMode(null, null),
    knowledgeStartMode({ mode: "local" }, null),
    knowledgeStartMode({ mode: "global" }, null),
    knowledgeStartMode({ mode: "bogus" }, null),
    knowledgeStartMode(null, "wiki/Page-0001.md"),
    knowledgeStartMode({ mode: "global" }, "wiki/Page-0001.md"),
  ];
  knowledgeQuery = "";
  knowledgeOrphansOnly = knowledgeGhostsOnly = knowledgeTypedOnly = false;
  knowledgeAliveOnly = knowledgeColdOnly = knowledgeMergeOnly = false;
  knowledgeShowSources = false;
  knowledgeLintLens = knowledgeSelectedKey = knowledgePath = null;
  knowledgeTagsPicked.clear();
  knowledgeFocusDepth = 1;
  noteKnowledgeExploreLines({});
  window.__VAULT__ = { pages: 12, linksPer: 2, ghosts: 0, tags: ["core", "reading"] };
  knowledgeAskedAt = 0;
  dropTab("knowledge");
  /* 진짜 첫 방문: 이 볼트의 줄이 없다(하네스가 심어 둔 「전체 지도」 줄도 지운다). */
  window.__ANSWER__.set_second_brain_explore({ vault: "/vault", explore: "{}" });
  el("nav-knowledge").click();
  await settle(12);
  const first = { mode: knowledgeMode, centre: knowledgeSelectedKey, nodes: count(".knowledge-node"), stored: stored() };
  const view = viewOf();
  /* 전체 지도를 한 번 누르면 그것이 이 볼트의 마지막 모드다. */
  view.querySelector('.knowledge-mode-step[data-knowledge-mode="global"]').click();
  await wait(KNOWLEDGE_EXPLORE.persistMs + 200);
  const chose = { mode: knowledgeMode, stored: stored() };
  /* 주변으로 들어가면 이 볼트의 줄이 적힌다 — 모드·중심·깊이. */
  selectKnowledgeNode(view, "wiki/Page-0003.md");
  view.querySelector('.knowledge-mode-step[data-knowledge-mode="local"]').click();
  await wait(KNOWLEDGE_EXPLORE.persistMs + 200);
  const written = stored();
  view.querySelector('[data-knowledge-depth="2"]').click();
  await wait(KNOWLEDGE_EXPLORE.persistMs + 200);
  const deepened = stored();
  view.querySelector('.knowledge-mode-step[data-knowledge-mode="global"]').click();
  await wait(KNOWLEDGE_EXPLORE.persistMs + 200);
  const backOut = stored();
  /* 다른 창(또는 부팅)이 든 줄: 다시 열면 그 줄이 모드·중심·깊이를 정한다. */
  dropTab("knowledge");
  knowledgeSelectedKey = null;
  knowledgeFocusDepth = 1;
  noteKnowledgeExploreLines({ "/vault": JSON.stringify({ mode: "local", centre: "wiki/Page-0004.md", depth: 2 }) });
  el("nav-knowledge").click();
  await settle(12);
  const restored = {
    mode: knowledgeMode, centre: knowledgeSelectedKey, depth: knowledgeFocusDepth,
    nodes: count(".knowledge-node"), crumb: viewOf().querySelector(".knowledge-crumb-here").textContent,
  };
  /* 줄이 전체 지도를 말해도 「연결 보기」는 그 페이지의 주변이다. */
  dropTab("knowledge");
  noteKnowledgeExploreLines({ "/vault": JSON.stringify({ mode: "global" }) });
  knowledgeSelectedKey = null;
  revealKnowledgePage("wiki/Page-0006.md", { mode: "local" });
  await settle(12);
  const revealed = { mode: knowledgeMode, centre: knowledgeSelectedKey,
    crumb: viewOf().querySelector(".knowledge-crumb-here").textContent };
  /* 워크벤치의 문: 「지식에서 검색」 옆의 「연결 보기」가 그 작업이 참고한 지식으로 간다. */
  const related = workbenchRelatedActions({
    key: "task", workspace: { path: "", scoped: false, label: "" }, card: { pane: "term:7", title: "T", state: "idle" },
  });
  const order = [...related.querySelectorAll("button")].map((one) => one.className.split(" ").pop());
  const seat = taskBoardSeatOf({ pane: "term:7" });
  taskBoardRecallsHeld.set(seat.key, { at: Date.now(), rows: [{ page: "wiki/Page-0002.md", title: "개념 2", at_ms: Date.now() }] });
  dropTab("knowledge");
  noteKnowledgeExploreLines({ "/vault": JSON.stringify({ mode: "global" }) });
  knowledgeSelectedKey = null;
  related.querySelector(".workbench-related-links").click();
  await settle(12);
  const viaWorkbench = { mode: knowledgeMode, centre: knowledgeSelectedKey,
    word: related.querySelector(".workbench-related-links").textContent,
    expected: t("workbench.viewLinks", "연결 보기") };
  taskBoardRecallsHeld.delete(seat.key);
  viewOf().querySelector('.knowledge-mode-step[data-knowledge-mode="global"]').click();
  await wait(KNOWLEDGE_EXPLORE.persistMs + 200);
  selectKnowledgeNode(viewOf(), null);
  /* 전체 지도에서 시작하는 검사: 이 볼트의 마지막 모드가 전체 지도였다(첫 방문은 주변 탐색, S2). */
  noteKnowledgeExploreLines({ "/vault": JSON.stringify({ mode: "global" }) });
  return { rule, first, chose, written, deepened, backOut, restored, revealed, order, viaWorkbench };
});
ok("t-4140 S2: the first visit is the neighbourhood of what the person last used, 「연결 보기」 enters that page's neighbourhood, and the vault's last mode, centre and depth live in the settings document",
  entry.rule.join(",") === "local,local,global,local,local,local"
    && entry.first.mode === "local"
    && entry.first.centre !== null
    && entry.first.nodes > 0 && entry.first.nodes < 12
    && (entry.first.stored === null || entry.first.stored.mode === "local")
    && entry.chose.mode === "global"
    && entry.chose.stored?.mode === "global"
    && entry.written?.mode === "local"
    && entry.written?.centre === "wiki/Page-0003.md"
    && entry.written?.depth === 1
    && entry.deepened?.depth === 2
    && entry.backOut?.mode === "global"
    && entry.restored.mode === "local"
    && entry.restored.centre === "wiki/Page-0004.md"
    && entry.restored.depth === 2
    && entry.restored.nodes < 12
    && entry.restored.crumb === "개념 4"
    && entry.revealed.mode === "local"
    && entry.revealed.centre === "wiki/Page-0006.md"
    && entry.revealed.crumb === "개념 6"
    && entry.order.indexOf("workbench-related-links") === entry.order.indexOf("workbench-related-knowledge") + 1
    && entry.viaWorkbench.mode === "local"
    && entry.viaWorkbench.centre === "wiki/Page-0002.md"
    && entry.viaWorkbench.word === entry.viaWorkbench.expected,
  JSON.stringify(entry));

/* ---- t-4140 S3: 검색 후보 목록 ----
 *
 * 입력 중에 제목·폴더·관계 힌트가 있는 후보가 서고(상한은 표), ↑↓가 옮기고 Enter가
 * 고르며, 줄마다 「주변 탐색으로」가 있다. 토큰 자동완성과 저장 문구는 그대로 산다. */
const candidates = await page.evaluate(async () => {
  const viewOf = () => document.querySelector(".knowledge-view:not([hidden])");
  const wait = (ms) => new Promise((done) => setTimeout(done, ms));
  knowledgeQuery = "";
  knowledgeOrphansOnly = knowledgeGhostsOnly = knowledgeTypedOnly = false;
  knowledgeAliveOnly = knowledgeColdOnly = knowledgeMergeOnly = false;
  knowledgeShowSources = false;
  knowledgeLintLens = knowledgeSelectedKey = knowledgePath = null;
  knowledgeTagsPicked.clear();
  knowledgeFocusDepth = 1;
  /* 전체 지도에서 시작하는 검사: 이 볼트의 마지막 모드가 전체 지도였다(첫 방문은 주변 탐색, S2). */
  noteKnowledgeExploreLines({ "/vault": JSON.stringify({ mode: "global" }) });
  window.__VAULT__ = { pages: 12, linksPer: 2, ghosts: 0, tags: ["core", "reading"] };
  knowledgeAskedAt = 0;
  dropTab("knowledge");
  el("nav-knowledge").click();
  for (let round = 0; round < 300; round += 1) {
    await wait(20);
    const held = viewOf() ? knowledgeLayouts.get(viewOf()) : null;
    if (held && held.count === 12 && held.left === 0 && knowledgeReport !== null) break;
  }
  const view = viewOf();
  const find = view.querySelector(".knowledge-query");
  const box = view.querySelector(".knowledge-search");
  const list = view.querySelector(".knowledge-search-results");
  const type = async (text) => {
    find.focus();
    find.value = text;
    find.dispatchEvent(new Event("input", { bubbles: true }));
    await wait(40);
  };
  const key = (name, init = {}) => find.dispatchEvent(new KeyboardEvent("keydown", { key: name, bubbles: true, cancelable: true, ...init }));
  const rows = () => [...list.querySelectorAll('[role="option"]')];
  await type("개념 1");
  const three = {
    role: list.getAttribute("role"),
    suggesting: box.classList.contains("is-suggesting"),
    titles: rows().map((row) => row.querySelector(".knowledge-candidate-title").textContent),
    notes: rows().map((row) => row.querySelector(".knowledge-candidate-note").textContent),
    exploreWords: rows().map((row) => row.querySelector(".knowledge-candidate-explore").textContent),
    /* 서 있는 칩만 — `sev:`은 공급망 렌즈(P4)가 켜졌을 때만 선다. */
    tokens: view.querySelectorAll(".knowledge-search-tokens .knowledge-token-chip:not([hidden])").length,
    phrases: view.querySelector(".knowledge-saved-phrases") !== null,
  };
  await type("개념");
  const capped = { rows: rows().length, cap: KNOWLEDGE_EXPLORE.candidates, matches: knowledgeLayouts.get(view).searchMatch.filter((one) => one === 1).length };
  key("ArrowDown");
  key("ArrowDown");
  const second = rows()[1];
  const moved = { selected: rows().map((row) => row.getAttribute("aria-selected")), secondKey: second.dataset.knowledgeKey };
  key("Enter");
  await wait(60);
  const picked = { selected: knowledgeSelectedKey, mode: knowledgeMode, suggesting: box.classList.contains("is-suggesting"), query: knowledgeQuery };
  await type("개념 5");
  key("Escape");
  const closed = box.classList.contains("is-suggesting");
  await type("개념 5");
  rows()[0].querySelector(".knowledge-candidate-explore").click();
  await wait(80);
  const explored = { mode: knowledgeMode, centre: knowledgeSelectedKey, query: knowledgeQuery, value: find.value,
    crumb: view.querySelector(".knowledge-crumb-here").textContent };
  await type("zzz-nothing");
  const empty = { rows: rows().length, word: view.querySelector(".knowledge-candidates-empty")?.textContent ?? "",
    hidden: view.querySelector(".knowledge-candidates-empty")?.hidden ?? null,
    expected: t("knowledge.noCandidates", "일치하는 페이지가 없습니다") };
  await type("");
  view.querySelector('.knowledge-mode-step[data-knowledge-mode="global"]').click();
  await wait(80);
  selectKnowledgeNode(viewOf(), null);
  return { three, capped, moved, picked, closed, explored, empty, exploreExpected: t("knowledge.candidateExplore", "주변 탐색으로") };
});
ok("t-4140 S3: typing lists ranked candidates with folder and relation hints under the table's cap, ↑↓ and Enter pick one, each row opens 주변 탐색, and the token and phrase helpers stay",
  candidates.three.role === "listbox"
    && candidates.three.suggesting
    && candidates.three.titles.join(",") === "개념 1,개념 10,개념 11"
    && candidates.three.notes.every((note) => typeof note === "string")
    && candidates.three.exploreWords.every((word) => word === candidates.exploreExpected)
    && candidates.three.tokens === 5
    && candidates.three.phrases
    && candidates.capped.matches === 12
    && candidates.capped.rows === candidates.capped.cap
    && candidates.moved.selected.filter((one) => one === "true").length === 1
    && candidates.moved.selected[1] === "true"
    && candidates.picked.selected === candidates.moved.secondKey
    && candidates.picked.mode === "global"
    && candidates.picked.suggesting === false
    && candidates.picked.query === "개념"
    && candidates.closed === false
    && candidates.explored.mode === "local"
    && candidates.explored.centre === "wiki/Page-0005.md"
    && candidates.explored.query === "" && candidates.explored.value === ""
    && candidates.explored.crumb === "개념 5"
    && candidates.empty.rows === 0
    && candidates.empty.hidden === false
    && candidates.empty.word === candidates.empty.expected,
  JSON.stringify(candidates));

/* ---- t-4140 S4: 오른쪽 패널 ----
 *
 * 선택 없음은 개요, 선택은 제목·요약·연결 이유(방향·종류)·원문 위치. 활동 로그(BUS LOG)는
 * 별도 탭이라 상시 스크롤이 탐색을 방해하지 않는다. */
const panel = await page.evaluate(async () => {
  const viewOf = () => document.querySelector(".knowledge-view:not([hidden])");
  const wait = (ms) => new Promise((done) => setTimeout(done, ms));
  knowledgeQuery = "";
  knowledgeOrphansOnly = knowledgeGhostsOnly = knowledgeTypedOnly = false;
  knowledgeAliveOnly = knowledgeColdOnly = knowledgeMergeOnly = false;
  knowledgeShowSources = false;
  knowledgeLintLens = knowledgeSelectedKey = knowledgePath = null;
  knowledgeTagsPicked.clear();
  knowledgeFocusDepth = 1;
  /* 전체 지도에서 시작하는 검사: 이 볼트의 마지막 모드가 전체 지도였다(첫 방문은 주변 탐색, S2). */
  noteKnowledgeExploreLines({ "/vault": JSON.stringify({ mode: "global" }) });
  const now = Date.now();
  window.__VAULT__ = {
    pages: 12, linksPer: 2, ghosts: 0, tags: ["core", "reading"], allTyped: true,
    live: {
      now_ms: now, recalled: [],
      bus: [{ at_ms: now - 1000, kind: "created", page: "wiki/Page-0005.md", note: "개념 5" }],
      merge: [],
      limits: { activity_window_minutes: 30, today_hours: 24, bus_rows_max: 4 },
    },
  };
  knowledgeAskedAt = 0;
  dropTab("knowledge");
  el("nav-knowledge").click();
  for (let round = 0; round < 300; round += 1) {
    await wait(20);
    const held = viewOf() ? knowledgeLayouts.get(viewOf()) : null;
    if (held && held.count === 12 && held.left === 0 && knowledgeReport !== null) break;
  }
  const view = viewOf();
  const tabs = view.querySelector(".knowledge-inspector-tabs");
  const tabState = () => [...tabs.querySelectorAll('[role="tab"]')]
    .map((one) => `${one.dataset.knowledgeTab}:${one.getAttribute("aria-selected")}`).join(",");
  const pagePanel = view.querySelector(".knowledge-inspector-page");
  const activityPanel = view.querySelector(".knowledge-inspector-activity");
  const bus = view.querySelector(".knowledge-overview-bus");
  const initial = {
    tablist: tabs.getAttribute("role"),
    tabs: tabState(),
    pageHidden: pagePanel.hidden,
    activityHidden: activityPanel.hidden,
    overviewShown: !view.querySelector(".knowledge-overview").hidden,
    busInActivity: activityPanel.contains(bus),
    busInOverview: view.querySelector(".knowledge-overview").contains(bus),
    busHidden: bus.hidden,
    words: [...tabs.querySelectorAll('[role="tab"]')].map((one) => one.textContent),
    expected: [t("knowledge.tabPage", "페이지"), t("knowledge.tabActivity", "활동")],
  };
  tabs.querySelector('[data-knowledge-tab="activity"]').click();
  await wait(40);
  const switched = {
    tabs: tabState(), pageHidden: pagePanel.hidden, activityHidden: activityPanel.hidden,
    rows: activityPanel.querySelectorAll(".knowledge-bus-row").length,
  };
  /* 점을 고르면 페이지 탭으로 — 카드가 답이다. */
  selectKnowledgeNode(view, "wiki/Page-0000.md");
  await wait(40);
  const card = view.querySelector(".knowledge-card");
  const rowsOf = (selector) => [...view.querySelectorAll(`${selector} li`)].map((row) => ({
    mark: row.querySelector(".knowledge-relation-mark")?.className ?? "",
    dir: row.querySelector(".knowledge-relation-dir")?.textContent ?? "",
    note: row.querySelector(".knowledge-inspector-note")?.textContent ?? "",
    why: row.querySelector(".knowledge-relation-why")?.textContent ?? "",
    /* 근거의 칩(t-5966) — 뜻 뒤에 누가 썼는가. */
    road: row.querySelector(".knowledge-relation-provenance")?.textContent ?? "",
    kindWord: row.querySelector(".knowledge-inspector-note")?.firstChild?.textContent ?? "",
  }));
  const chosen = {
    tabs: tabState(),
    pageHidden: pagePanel.hidden,
    cardShown: !card.hidden,
    title: card.querySelector(".knowledge-inspector-title").textContent,
    summary: card.querySelector(".knowledge-inspector-excerpt").textContent,
    path: card.querySelector(".knowledge-inspector-path").textContent,
    openShown: !card.querySelector(".knowledge-inspector-open").hidden,
    outgoing: rowsOf(".knowledge-relations-outgoing"),
    incoming: rowsOf(".knowledge-relations-incoming"),
  };
  /* 탭은 다시 그려도 남는다. */
  tabs.querySelector('[data-knowledge-tab="activity"]').click();
  await paintKnowledgeView();
  const kept = { tabs: tabState(), activityHidden: activityPanel.hidden };
  tabs.querySelector('[data-knowledge-tab="page"]').click();
  selectKnowledgeNode(view, null);
  window.__VAULT__ = { pages: 12, linksPer: 2, ghosts: 0, tags: ["core", "reading"] };
  return { initial, switched, chosen, kept };
});
ok("t-4140 S4: the inspector is two tabs — the page (overview or the card with title, summary, path, directed and typed relation rows) and the activity log — and the chosen tab survives a repaint",
  panel.initial.tablist === "tablist"
    && panel.initial.tabs === "page:true,activity:false"
    && panel.initial.pageHidden === false
    && panel.initial.activityHidden === true
    && panel.initial.overviewShown
    && panel.initial.busInActivity && !panel.initial.busInOverview
    && panel.initial.busHidden === false
    && JSON.stringify(panel.initial.words) === JSON.stringify(panel.initial.expected)
    && panel.switched.tabs === "page:false,activity:true"
    && panel.switched.pageHidden === true && panel.switched.activityHidden === false
    && panel.switched.rows === 1
    && panel.chosen.tabs === "page:true,activity:false"
    && panel.chosen.pageHidden === false
    && panel.chosen.cardShown
    && panel.chosen.title === "개념 0"
    && panel.chosen.summary.length > 0
    && panel.chosen.path === "wiki/Page-0000.md"
    && panel.chosen.openShown
    && panel.chosen.outgoing.length > 0
    // 낱말 칸은 관계 키 그대로 시작하고(번역 금지 — 볼트에 적힌 키다) 그 뒤에
    // 그 키의 뜻이 쉬운 말로 붙는다(09-16): 「왜 연결됐는지」에 답하는 한 마디.
    && panel.chosen.outgoing.every((row) => /\bkind-[a-z_]+\b/.test(row.mark) && row.dir === "→"
      && /^[a-z_?]+/.test(row.note) && row.why.length > 0 && row.road.length > 0
      && row.note === `${row.kindWord}${row.why}${row.road}`)
    && panel.chosen.incoming.every((row) => /\bkind-[a-z_]+\b/.test(row.mark) && row.dir === "←"
      && /^[a-z_?]+/.test(row.note) && row.why.length > 0 && row.road.length > 0
      && row.note === `${row.kindWord}${row.why}${row.road}`)
    && panel.kept.tabs === "page:false,activity:true" && panel.kept.activityHidden === false,
  JSON.stringify(panel));

/* ---- t-4140 S6: 관리용 문서 ----
 *
 * `index`·`log`의 목차/일지 링크는 표시 설정으로 켜고 끈다(기본은 접힘 — 09-16에 뒤집혔다:
 * 그 둘의 차수가 전체 간선의 삼분의 일이라, 서 있는 전체 지도는 늘 한 덩어리였다).
 * 끄는 것은 그림에서만이다: 페이지는 모델·검색·관계 목록에 그대로 있고, 파일명만 보고
 * 삭제되거나 의미 관계에서 자동으로 빠지지 않는다. */
const nav = await page.evaluate(async () => {
  const viewOf = () => document.querySelector(".knowledge-view:not([hidden])");
  const wait = (ms) => new Promise((done) => setTimeout(done, ms));
  const count = (selector) => viewOf().querySelectorAll(selector).length;
  knowledgeQuery = "";
  knowledgeOrphansOnly = knowledgeGhostsOnly = knowledgeTypedOnly = false;
  knowledgeAliveOnly = knowledgeColdOnly = knowledgeMergeOnly = false;
  knowledgeShowSources = false;
  knowledgeLintLens = knowledgeSelectedKey = knowledgePath = null;
  knowledgeTagsPicked.clear();
  knowledgeFocusDepth = 1;
  /* 전체 지도에서 시작하는 검사: 이 볼트의 마지막 모드가 전체 지도였다(첫 방문은 주변 탐색, S2). */
  noteKnowledgeExploreLines({ "/vault": JSON.stringify({ mode: "global" }) });
  const edges = [];
  for (let at = 2; at <= 9; at += 1) edges.push({ from: 0, to: at, kind: "mentions" });
  edges.push({ from: 1, to: 2, kind: "mentions" }, { from: 1, to: 3, kind: "mentions" });
  /* 0→4는 이미 본문 링크라 두 번째 선은 서지 않는다(픽스처의 dedupe): 선은 열한 개다. */
  edges.push({ from: 2, to: 3, kind: "related" }, { from: 0, to: 4, kind: "implements" });
  window.__VAULT__ = {
    pages: 12, ghosts: 0, tags: ["core"], customEdges: edges,
    ids: { 0: "wiki/index.md", 1: "wiki/log.md" }, titles: { 0: "index", 1: "log" },
  };
  knowledgeAskedAt = 0;
  dropTab("knowledge");
  el("nav-knowledge").click();
  for (let round = 0; round < 300; round += 1) {
    await wait(20);
    const held = viewOf() ? knowledgeLayouts.get(viewOf()) : null;
    if (held && held.count === 12 && held.left === 0 && knowledgeReport !== null) break;
  }
  const view = viewOf();
  const flag = view.querySelector('[data-knowledge-flag="nav"]');
  const shownWord = flag?.getAttribute("aria-label") ?? "";
  /* 09-16: 기본이 뒤집혔다 — 판이 서면 목차·일지는 **접혀** 있고, 스위치 한 번이
     그 둘을 그림에 세운다. 볼트에서 그 두 쪽의 차수가 353·336(전체 간선의 32%)이라
     그것이 선 전체 지도는 어떤 배치를 써도 한 덩어리이기 때문이다. 계약의 나머지는
     그대로다: 모델·검색·관계 목록에는 언제나 있다. */
  const off = {
    pressed: flag?.getAttribute("aria-pressed"),
    nodes: count(".knowledge-node"),
    edges: count(".knowledge-edge"),
    index: view.querySelector('.knowledge-node[data-graph-key="wiki/index.md"]') !== null,
    log: view.querySelector('.knowledge-node[data-graph-key="wiki/log.md"]') !== null,
    stat: view.querySelector(".knowledge-stat").textContent,
    modelCount: knowledgeLayouts.get(view).model.count,
    pagesTile: view.querySelector('[data-knowledge-count="pages"]').textContent,
  };
  /* 검색과 관계 목록에는 그대로 있다. */
  const find = view.querySelector(".knowledge-query");
  find.focus();
  find.value = "index";
  find.dispatchEvent(new Event("input", { bubbles: true }));
  await wait(40);
  const searched = [...view.querySelectorAll('.knowledge-search-results [role="option"] .knowledge-candidate-title')].map((one) => one.textContent);
  find.value = "";
  find.dispatchEvent(new Event("input", { bubbles: true }));
  view.querySelector(".knowledge-search").classList.remove("is-suggesting");
  selectKnowledgeNode(view, "wiki/Page-0004.md");
  await wait(40);
  const incoming = [...view.querySelectorAll(".knowledge-relations-incoming li [data-knowledge-key]")].map((one) => one.dataset.knowledgeKey);
  /* 주변 탐색도 같은 설정을 읽는다: 4의 깊이 2에서 index를 지나 8쪽이 오지 않는다. */
  knowledgeFocusDepth = 2;
  view.querySelector('.knowledge-mode-step[data-knowledge-mode="local"]').click();
  await wait(60);
  const local = { nodes: count(".knowledge-node"), index: view.querySelector('.knowledge-node[data-graph-key="wiki/index.md"]') !== null };
  view.querySelector('.knowledge-mode-step[data-knowledge-mode="global"]').click();
  await wait(60);
  knowledgeFocusDepth = 1;
  /* 시야는 이 설정을 든다; 없는 시야는 켜진 채다. */
  const scene = captureCurrentKnowledgeScene("nav-off", view, knowledgeLayouts.get(view));
  await restoreKnowledgeScene(view, { name: "old", lens: {}, tags: [], search: "", selected: null, depth: 1 });
  await wait(60);
  const restoredDefault = { shown: knowledgeNavShown, nodes: count(".knowledge-node"), pressed: flag?.getAttribute("aria-pressed") };
  await restoreKnowledgeScene(view, scene);
  await wait(60);
  const restoredOff = { shown: knowledgeNavShown, nodes: count(".knowledge-node") };
  flag?.click();
  await wait(60);
  const back = { pressed: flag?.getAttribute("aria-pressed"), nodes: count(".knowledge-node"),
    edges: count(".knowledge-edge"),
    index: view.querySelector('.knowledge-node[data-graph-key="wiki/index.md"]') !== null };
  flag?.click();
  await wait(60);
  const folded = { nodes: count(".knowledge-node"), edges: count(".knowledge-edge") };
  selectKnowledgeNode(view, null);
  return { shownWord, expected: t("knowledge.navPages", "목차·일지 표시"), off, searched, incoming, local, sceneNav: scene.lens.nav, restoredDefault, restoredOff, back, folded };
});
ok("t-4140 S6: the index/log display setting removes only their picture — pages stay in the model, search and relation lists, local mode reads it, scenes carry it, and the default is folded",
  nav.shownWord === nav.expected
    && nav.off.pressed === "false"
    && nav.off.nodes === 10 && nav.off.edges === 1
    && !nav.off.index && !nav.off.log
    && nav.off.stat.includes("10/12")
    && nav.off.modelCount === 12
    && nav.off.pagesTile === "12"
    && nav.searched.includes("index")
    && nav.incoming.includes("wiki/index.md")
    && nav.local.nodes === 1 && !nav.local.index
    && nav.sceneNav === false
    && nav.restoredDefault.shown === true && nav.restoredDefault.nodes === 12 && nav.restoredDefault.pressed === "true"
    && nav.restoredOff.shown === false && nav.restoredOff.nodes === 10
    && nav.back.pressed === "true" && nav.back.nodes === 12 && nav.back.edges === 11 && nav.back.index
    && nav.folded.nodes === 10 && nav.folded.edges === 1,
  JSON.stringify(nav));

/* ---- t-4140 S7: 접근성 ----
 *
 * 점·후보·관계 목록을 키보드로 옮기고 고른다(명시적 포커스 링), 움직임을 줄이라는 판을
 * 따르고, 색이 관계의 뜻을 혼자 나르지 않는다(모양·라벨 병기). */
await page.emulateMedia({ reducedMotion: "no-preference" });
const access = await page.evaluate(async () => {
  const viewOf = () => document.querySelector(".knowledge-view:not([hidden])");
  const wait = (ms) => new Promise((done) => setTimeout(done, ms));
  knowledgeQuery = "";
  knowledgeOrphansOnly = knowledgeGhostsOnly = knowledgeTypedOnly = false;
  knowledgeAliveOnly = knowledgeColdOnly = knowledgeMergeOnly = false;
  knowledgeShowSources = false;
  knowledgeLintLens = knowledgeSelectedKey = knowledgePath = null;
  knowledgeTagsPicked.clear();
  knowledgeFocusDepth = 1;
  /* 전체 지도에서 시작하는 검사: 이 볼트의 마지막 모드가 전체 지도였다(첫 방문은 주변 탐색, S2). */
  noteKnowledgeExploreLines({ "/vault": JSON.stringify({ mode: "global" }) });
  window.__VAULT__ = { pages: 12, linksPer: 2, ghosts: 0, tags: ["core", "reading"], allTyped: true };
  knowledgeAskedAt = 0;
  dropTab("knowledge");
  el("nav-knowledge").click();
  for (let round = 0; round < 300; round += 1) {
    await wait(20);
    const held = viewOf() ? knowledgeLayouts.get(viewOf()) : null;
    if (held && held.count === 12 && held.left === 0 && knowledgeReport !== null) break;
  }
  const view = viewOf();
  const canvas = view.querySelector(".knowledge-canvas");
  const press = (target, name, init = {}) => target.dispatchEvent(new KeyboardEvent("keydown", { key: name, bubbles: true, cancelable: true, ...init }));
  /* 1. 점: 캔버스가 한 위젯이고 화살표가 점 사이를 걷는다; 고른 점은 굵은 테두리를 두른다. */
  canvas.focus();
  press(canvas, "ArrowDown");
  await wait(40);
  const node = view.querySelector(".knowledge-node.is-selected");
  const dot = node?.querySelector(".knowledge-dot");
  const plain = view.querySelector(".knowledge-node:not(.is-selected) .knowledge-dot");
  const nodes = {
    application: canvas.getAttribute("role"),
    selected: knowledgeSelectedKey,
    ring: dot && plain ? Number.parseFloat(getComputedStyle(dot).strokeWidth) > Number.parseFloat(getComputedStyle(plain).strokeWidth) : false,
  };
  /* 2. 관계 목록: ↑↓가 줄 사이를 옮기고, Home/End가 끝으로 간다. */
  const rows = () => [...view.querySelectorAll(".knowledge-relations-outgoing .knowledge-inspector-row")];
  rows()[0].focus();
  press(rows()[0], "ArrowDown");
  const second = document.activeElement === rows()[1];
  press(document.activeElement, "End");
  const last = document.activeElement === rows()[rows().length - 1];
  press(document.activeElement, "Home");
  const first = document.activeElement === rows()[0];
  press(document.activeElement, "ArrowUp");
  const stays = document.activeElement === rows()[0];
  /* 3. 후보: 콤보박스가 열림을 말한다. */
  const find = view.querySelector(".knowledge-query");
  find.focus();
  find.value = "개념";
  find.dispatchEvent(new Event("input", { bubbles: true }));
  await wait(40);
  const combo = {
    role: find.getAttribute("role"),
    autocomplete: find.getAttribute("aria-autocomplete"),
    open: find.getAttribute("aria-expanded"),
    options: view.querySelectorAll('.knowledge-search-results [role="option"]').length,
  };
  press(find, "Escape");
  combo.closed = find.getAttribute("aria-expanded");
  find.value = "";
  find.dispatchEvent(new Event("input", { bubbles: true }));
  /* 4. 색이 혼자 말하지 않는다: 방향 있는 선은 화살표 표식을, 종류는 선의 모양(대시)을 든다. */
  const dashOf = (kind) => getComputedStyle(view.querySelector(`.knowledge-edge.kind-${kind}`)).strokeDasharray;
  const shape = {
    arrow: view.querySelector(".knowledge-edge.kind-depends_on")?.getAttribute("marker-end") ?? "",
    dashDiffers: dashOf("depends_on") !== dashOf("related"),
    markWords: [...view.querySelectorAll(".knowledge-relations-outgoing li")]
      .every((row) => row.querySelector(".knowledge-relation-mark") !== null
        && /^[→←]$/.test(row.querySelector(".knowledge-relation-dir")?.textContent ?? "")
        && row.querySelector(".knowledge-inspector-note").textContent.length > 0),
  };
  /* 5. 포커스 링: 새 손잡이마다 `:focus-visible` 규칙이 있다. */
  const selectors = [];
  for (const sheet of document.styleSheets) {
    try { for (const rule of sheet.cssRules) if (rule.selectorText) selectors.push(rule.selectorText); } catch { /* cross-origin */ }
  }
  const ringFor = (name) => selectors.some((one) => one.includes(`${name}:focus-visible`));
  const rings = ["knowledge-mode-step", "knowledge-crumb-back", "knowledge-crumb-root", "knowledge-candidate", "knowledge-candidate-explore", "knowledge-unfold", "knowledge-inspector-row"]
    .map((name) => [name, ringFor(`.${name}`)]);
  return { nodes, list: { second, last, first, stays }, combo, shape, rings };
});
/* 6. 움직임을 줄이라는 판: 카메라는 날지 않고 닿는다 — 주변 탐색의 재중심도 같다. */
await page.emulateMedia({ reducedMotion: "reduce" });
const stillness = await page.evaluate(async () => {
  const view = document.querySelector(".knowledge-view:not([hidden])");
  const layout = knowledgeLayouts.get(view);
  const seat = layout.model.keys.indexOf("wiki/Page-0007.md");
  knowledgeCenterOn(view, layout, seat);
  const landed = layout.flight === null && Math.abs(layout.panX - (layout.x[seat] - (layout.bounds.minX + layout.bounds.maxX) / 2)) < 1e-6;
  const reduced = knowledgeMotionReduced();
  selectKnowledgeNode(view, null);
  return { landed, reduced };
});
await page.emulateMedia({ reducedMotion: "no-preference" });
ok("t-4140 S7: nodes, candidates and relation lists move and select from the keyboard with explicit focus rings, reduced motion lands the camera without a flight, and relation meaning is carried by shape and words as well as colour",
  access.nodes.application === "application"
    && access.nodes.selected !== null
    && access.nodes.ring
    && access.list.second && access.list.last && access.list.first && access.list.stays
    && access.combo.role === "combobox"
    && access.combo.autocomplete === "list"
    && access.combo.open === "true"
    && access.combo.options > 0
    && access.combo.closed === "false"
    && access.shape.arrow.startsWith("url(")
    && access.shape.dashDiffers
    && access.shape.markWords
    && access.rings.every(([, has]) => has)
    && stillness.reduced && stillness.landed,
  JSON.stringify({ access, stillness }));

/* ---- t-4140 S5: 연결 만들기 ----
 *
 * 출발 노드의 「연결 추가」 → 대상 검색(후보 목록) → 관계 유형·방향 → 미리보기 → 저장·되돌리기.
 * 저장은 Rust 문 하나(`second_brain_relate`)이고 되돌리기는 같은 문의 반대 호출이다.
 * 드래그로 잇기는 수식키(Alt)가 든 보조 수단이라 점의 배치와 섞이지 않는다. */
const linking = await page.evaluate(async () => {
  const viewOf = () => document.querySelector(".knowledge-view:not([hidden])");
  const wait = (ms) => new Promise((done) => setTimeout(done, ms));
  knowledgeQuery = "";
  knowledgeOrphansOnly = knowledgeGhostsOnly = knowledgeTypedOnly = false;
  knowledgeAliveOnly = knowledgeColdOnly = knowledgeMergeOnly = false;
  knowledgeShowSources = false;
  knowledgeLintLens = knowledgeSelectedKey = knowledgePath = null;
  knowledgeTagsPicked.clear();
  knowledgeFocusDepth = 1;
  /* 전체 지도에서 시작하는 검사: 이 볼트의 마지막 모드가 전체 지도였다(첫 방문은 주변 탐색, S2). */
  noteKnowledgeExploreLines({ "/vault": JSON.stringify({ mode: "global" }) });
  window.__RELATE__ = [];
  window.__VAULT__ = { pages: 12, linksPer: 2, ghosts: 1, tags: ["core", "reading"] };
  knowledgeAskedAt = 0;
  dropTab("knowledge");
  el("nav-knowledge").click();
  for (let round = 0; round < 300; round += 1) {
    await wait(20);
    const held = viewOf() ? knowledgeLayouts.get(viewOf()) : null;
    if (held && held.count === 13 && held.left === 0 && knowledgeReport !== null) break;
  }
  const view = viewOf();
  const card = view.querySelector(".knowledge-card");
  const form = card.querySelector(".knowledge-link-form");
  const link = card.querySelector(".knowledge-inspector-link");
  const target = form.querySelector(".knowledge-link-target");
  const preview = () => form.querySelector(".knowledge-link-preview").textContent;
  const press = (node, name, init = {}) => node.dispatchEvent(new KeyboardEvent("keydown", { key: name, bubbles: true, cancelable: true, ...init }));
  const type = async (text) => {
    target.focus();
    target.value = text;
    target.dispatchEvent(new Event("input", { bubbles: true }));
    await wait(40);
  };
  /* 유령에는 문이 없다 — 쓸 frontmatter가 없다. */
  selectKnowledgeNode(view, "ghost:아직 없는 0");
  await wait(40);
  const ghostHidden = link.hidden;
  selectKnowledgeNode(view, "wiki/Page-0000.md");
  await wait(40);
  const closed = form.hidden;
  link.click();
  await wait(40);
  const opened = { shown: !form.hidden, saveDisabled: form.querySelector(".knowledge-link-save").disabled,
    kinds: [...form.querySelector(".knowledge-link-kind").options].map((one) => one.value),
    word: link.textContent, expected: t("knowledge.addLink", "연결 추가") };
  await type("개념 3");
  const options = [...form.querySelectorAll('[role="option"]')].map((one) => one.dataset.knowledgeLinkKey);
  press(target, "Enter");
  await wait(40);
  const picked = { target: knowledgeLinkTarget, value: target.value, preview: preview(),
    saveDisabled: form.querySelector(".knowledge-link-save").disabled };
  const kind = form.querySelector(".knowledge-link-kind");
  kind.value = "depends_on";
  kind.dispatchEvent(new Event("change", { bubbles: true }));
  const previewDepends = preview();
  form.querySelector(".knowledge-link-save").click();
  await wait(150);
  const saved = window.__RELATE__[0] ?? null;
  const toastAct = document.querySelector(".toast--acting .toast-action");
  const toast = { shown: toastAct !== null, word: toastAct?.textContent ?? "", expected: t("knowledge.undoLink", "되돌리기"),
    formClosed: form.hidden };
  toastAct?.click();
  await wait(150);
  const undone = window.__RELATE__[1] ?? null;
  /* 반대 방향: 대상이 출발이 된다. */
  link.click();
  await wait(40);
  await type("개념 4");
  press(target, "Enter");
  await wait(40);
  form.querySelector('[data-knowledge-link-direction="inward"]').click();
  const previewInward = preview();
  form.querySelector(".knowledge-link-save").click();
  await wait(150);
  const inward = window.__RELATE__[2] ?? null;
  /* Ctrl/Cmd+Z가 마지막 연결을 되돌린다. */
  const canvas = view.querySelector(".knowledge-canvas");
  canvas.focus();
  press(canvas, "z", { metaKey: true });
  await wait(150);
  const undoneByKey = window.__RELATE__[3] ?? null;
  /* Alt+드래그: 점 0에서 점 5로 — 점은 움직이지 않고 서식이 대상을 든 채 열린다. */
  const layout = knowledgeLayouts.get(view);
  const at = (key) => {
    const box = view.querySelector(`.knowledge-node[data-graph-key="${key}"] .knowledge-dot`).getBoundingClientRect();
    return { x: box.left + box.width / 2, y: box.top + box.height / 2 };
  };
  const source = at("wiki/Page-0000.md");
  const dest = at("wiki/Page-0005.md");
  const xBefore = layout.x[0];
  view.querySelector('.knowledge-node[data-graph-key="wiki/Page-0000.md"]')
    .dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, button: 0, altKey: true, clientX: source.x, clientY: source.y, pointerId: 7 }));
  window.dispatchEvent(new PointerEvent("pointermove", { clientX: dest.x, clientY: dest.y, altKey: true, pointerId: 7 }));
  window.dispatchEvent(new PointerEvent("pointerup", { clientX: dest.x, clientY: dest.y, altKey: true, pointerId: 7 }));
  await wait(80);
  const dragged = { shown: !form.hidden, target: knowledgeLinkTarget, selected: knowledgeSelectedKey,
    still: layout.x[0] === xBefore, linkingClass: canvas.classList.contains("is-linking") };
  form.querySelector(".knowledge-link-cancel").click();
  const cancelled = form.hidden;
  selectKnowledgeNode(view, null);
  return { ghostHidden, closed, opened, options, picked, previewDepends, saved, toast, undone, previewInward, inward, undoneByKey, dragged, cancelled };
});
ok("t-4140 S5: 「연결 추가」 searches a target, picks kind and direction, previews the frontmatter change, saves through one Rust door, undoes through the same door, and Alt+drag prefills the target without moving the node",
  linking.ghostHidden === true
    && linking.closed === true
    && linking.opened.shown && linking.opened.saveDisabled
    && linking.opened.kinds.join(",") === "related,implements,depends_on,supersedes,contradicts"
    && linking.opened.word === linking.opened.expected
    && linking.options[0] === "wiki/Page-0003.md"
    && linking.picked.target === "wiki/Page-0003.md"
    && linking.picked.value === "개념 3"
    && linking.picked.preview.includes("개념 0") && linking.picked.preview.includes("개념 3") && linking.picked.preview.includes("related")
    && linking.picked.saveDisabled === false
    && linking.previewDepends.includes("depends_on")
    && linking.saved?.path === "/vault" && linking.saved?.from === "wiki/Page-0000.md" && linking.saved?.to === "wiki/Page-0003.md"
    && linking.saved?.kind === "depends_on" && !linking.saved?.remove
    && linking.toast.shown && linking.toast.word === linking.toast.expected && linking.toast.formClosed
    && linking.undone?.from === "wiki/Page-0000.md" && linking.undone?.to === "wiki/Page-0003.md" && linking.undone?.remove === true
    && linking.previewInward.includes("개념 4") && linking.previewInward.indexOf("개념 4") < linking.previewInward.indexOf("개념 0")
    && linking.inward?.from === "wiki/Page-0004.md" && linking.inward?.to === "wiki/Page-0000.md" && linking.inward?.kind === "related"
    && linking.undoneByKey?.from === "wiki/Page-0004.md" && linking.undoneByKey?.remove === true
    && linking.dragged.shown && linking.dragged.target === "wiki/Page-0005.md" && linking.dragged.selected === "wiki/Page-0000.md"
    && linking.dragged.still && linking.dragged.linkingClass === false
    && linking.cancelled === true,
  JSON.stringify(linking));

const pixelShots = [
  { width: 1998, height: 1069, filesOpen: true,
    path: "/tmp/t-2465-knowledge-1998x1069-files-open.png" },
  { width: 1998, height: 1069, filesOpen: false,
    path: "/tmp/t-2465-knowledge-1998x1069-files-closed.png" },
  { width: 1280, height: 800, filesOpen: false,
    path: "/tmp/t-2465-knowledge-1280x800.png" },
  { width: 800, height: 600, filesOpen: false,
    path: "/tmp/t-2465-knowledge-800x600.png" },
];
for (const shot of pixelShots) {
  await page.setViewportSize({ width: shot.width, height: shot.height });
  await page.evaluate(async (filesOpen) => {
    setTheme("dark");
    setPanelFolded("aside", !filesOpen);
    knowledgeQuery = "";
    knowledgeOrphansOnly = false;
    knowledgeGhostsOnly = false;
    knowledgeTypedOnly = false;
    knowledgeShowSources = false;
    knowledgeTagsPicked.clear();
    knowledgeSelectedKey = null;
    window.__VAULT__ = { pages: 14, linksPer: 2, ghosts: 3,
      tags: ["core", "reading", "tools", "design", "systems", "notes", "ideas", "archive"],
      allTyped: true };
    dropTab("knowledge");
    document.getElementById("nav-knowledge").click();
  }, shot.filesOpen);
  await page.waitForFunction(() => {
    const view = [...document.querySelectorAll(".file-view")]
      .find((one) => one.classList.contains("knowledge-view") && !one.hidden);
    const layout = view ? knowledgeLayouts.get(view) : null;
    return layout?.model.count === 17 && layout.left === 0;
  }, null, { timeout: 8000 });
  await page.locator(".knowledge-view:not([hidden]) .knowledge-zoom-fit").click();
  await page.waitForFunction(() => {
    const layout = knowledgeLayouts.get(document.querySelector(".knowledge-view:not([hidden])"));
    return layout?.flight === null && layout.zoom === 1;
  });
  await page.mouse.move(0, 0);
  await new Promise((done) => setTimeout(done, 120));
  await page.screenshot({ path: shot.path });
}

/* Test 17: 저장 시야는 설정 문서의 것이다 — 창을 새로 읽으면(재시작·리로드) 부팅
 *보고가 싣고 온 줄에서 목록이 서고, 다음 저장은 그 줄 위에 얹는다. 창이 제 메모리의
 * 사본만 들면 새로 읽은 창의 첫 저장이 디스크의 시야를 전부 지운다(K18). 마지막에
 * 서는 것은 이 케이스만 판 전체를 새로 읽기 때문이다. */
await page.evaluate(() => sessionStorage.setItem("__KNOWLEDGE_REBOOT__", "1"));
await page.reload();
await page.waitForFunction(() => typeof BOUND !== "undefined" && BOUND.size > 0);
await page.waitForFunction(() => secondBrainVault === "/vault", null, { timeout: 8000 })
  .catch(() => {});
const sceneReload = await page.evaluate(async () => {
  const persisted = () => JSON.parse(window.__SCENE_LINES__()["/vault"] ?? "{}");
  const before = persisted();
  window.__VAULT__ = { pages: 6, linksPer: 1, ghosts: 0, tags: ["core"] };
  document.getElementById("nav-knowledge").click();
  const viewOf = () => document.querySelector(".knowledge-view:not([hidden])");
  for (let round = 0; round < 200 && !(knowledgeLayouts.get(viewOf())?.count > 0); round += 1) {
    await new Promise((done) => setTimeout(done, 20));
  }
  const view = viewOf();
  view.querySelector(".knowledge-scene-toggle").click();
  const listed = [...view.querySelectorAll(".knowledge-scene-name")].map((one) => one.textContent);
  const find = view.querySelector(".knowledge-query");
  find.value = "tag:core";
  find.dispatchEvent(new Event("input", { bubbles: true }));
  view.querySelector(".knowledge-save-phrase").click();
  await new Promise((done) => setTimeout(done, 100));
  const after = persisted();
  return {
    vault: secondBrainVault,
    before: (before.scenes ?? []).map((one) => one.name),
    listed,
    afterScenes: (after.scenes ?? []).map((one) => one.name),
    afterPhrases: after.phrases ?? [],
  };
});
ok("saved scenes live in the settings document: a reloaded window lists them and its next save keeps them",
  sceneReload.before.includes("my-focus-scene")
    && sceneReload.listed.includes("my-focus-scene")
    && sceneReload.afterScenes.includes("my-focus-scene")
    && sceneReload.afterPhrases.includes("tag:core"),
  JSON.stringify(sceneReload));

await testKnowledgePerformance(page, ok);
await measureKnowledgePainterSwap(page, ok);
const sweep = knowledgeSweepFor(process.env);
await measureKnowledgeScenes(page, ok, { frames: sweep.frames });

/* ---- 모양의 문법 (지식 그래프 P1, 2026-09-17) --------------------------------
 *
 * 사람의 말: 「지식그래프 svg gl 문구 빼줘 그리고 네모 세모 … 지금 포자 형태의 동그라미로만
 * 되어있는데 의미를 넣어서」. 설계(docs/design/knowledge-supply-chain-20260917.md §2)의
 * 합격선 넷을 시험이 묻는 수로 옮긴다:
 *   G1 그리는 손의 낱말(SVG·GL·WebGL)이 다섯 언어의 판(글자와 사람이 읽는 속성)과 번역 표
 *      어디에도 없다.
 *   G2 손은 사람이 아니라 창이 고른다 — 표(`KNOWLEDGE_PAINTERS`)에서 설 수 있는 첫 줄.
 *   G3 종류 다섯이 모양 다섯에 하나씩 닿고, 같은 크기면 같은 넓이다. 두 손이 같은 모양을
 *      그리는지는 GL 판의 픽셀 대조가 묻는다(아래).
 *   G4 범례의 모든 줄이 그림과 같은 모양을 같은 표에서 그린다.
 *
 * 이 케이스들은 판의 시간 측정(METRIC) **뒤**에 선다 — 짝지어 재는 전(main)의 하네스와
 * 같은 순서로 측정이 돌게. 판(`evaluate`)이 던지면 그 던짐이 FAIL 한 줄의 근거가 되고
 * 하네스는 끝까지 달린다(모양의 표가 없는 제품에서도 뒤의 METRIC이 선다). */
const PAINTER_WORDS = /\b(?:SVG|GL|WebGL\d*)\b/iu;
const grammarVault = { pages: 12, linksPer: 2, ghosts: 2, tags: ["core", "reading"],
  supply: { components: 2, vulnerabilities: 1 }, code: { files: 1, symbols: 1 } };
/* 원본은 볼트의 앞 다섯 쪽이 하나씩 인용한다(`knowledge-fixture.mjs`). 코드 층(t-5970)은 렌즈가 켜지고
 * 워크스페이스가 있을 때만 선다 — 이 판은 둘 다 세운다. */
const grammarCount = grammarVault.pages + grammarVault.ghosts + Math.min(5, grammarVault.pages)
  + grammarVault.supply.components + grammarVault.supply.vulnerabilities
  + grammarVault.code.files + grammarVault.code.symbols;
const GRAMMAR_WORKSPACE = "/workspace/acme";
await page.setViewportSize({ width: 1280, height: 860 });
const grammarOpened = await page.evaluate(({ vault, workspace }) => {
  try {
    knowledgeQuery = "";
    knowledgeTagsPicked.clear();
    knowledgeOrphansOnly = knowledgeGhostsOnly = knowledgeTypedOnly = false;
    knowledgeAliveOnly = knowledgeColdOnly = knowledgeMergeOnly = false;
    knowledgeLintLens = knowledgeSelectedKey = knowledgePath = null;
    knowledgeSlicerCutoff = 0;
    knowledgeClusterPicked = -1;
    knowledgeEntryPending = false;
    knowledgeRevealKey = null;
    knowledgeRevealMode = null;
    knowledgeMode = "global";
    /* 창이 손을 고르는 판 — 앞 케이스가 못박은 손을 놓는다. */
    knowledgePainterKind = null;
    knowledgeShowSources = true;
    knowledgeShowCode = true;
    window.__GRAMMAR_HELD_WORKTREE__ = activeWorktreePath;
    activeWorktreePath = workspace;
    knowledgeReport = null;
    knowledgeAskedAt = 0;
    window.__VAULT__ = vault;
    dropTab("knowledge");
    document.getElementById("nav-knowledge").click();
    return "";
  } catch (error) {
    return String(error?.stack ?? error);
  }
}, { vault: grammarVault, workspace: GRAMMAR_WORKSPACE });
const grammarStood = grammarOpened === "" && await page.waitForFunction((count) => {
  const view = document.querySelector(".knowledge-view:not([hidden])");
  const layout = view ? knowledgeLayouts.get(view) : undefined;
  return layout !== undefined && layout.model.count === count;
}, grammarCount, { timeout: 8000 }).then(() => true, () => false);
/* 세 물음을 따로 묻는다 — 한 물음이 던져도(모양의 표가 없는 제품) 나머지는 제 행동으로 갈린다. */
const grammarAsk = (run, arg) => (grammarStood ? page.evaluate(run, arg)
  : { thrown: grammarOpened || `the vault of every kind (${grammarCount} points) never stood` });

/* G2 — 못박지 않은 판에서 서는 손은 표의 설 수 있는 첫 줄이고, 사람이 고를 손잡이는 없다. */
const painterPick = await grammarAsk(async () => {
  try {
    const view = document.querySelector(".knowledge-view:not([hidden])");
    return {
      order: KNOWLEDGE_PAINTERS.map((row) => row.id),
      able: KNOWLEDGE_PAINTERS.map((row) => row.able()),
      pinned: knowledgePainterKind,
      standing: knowledgePainterFor(view).id,
      firstAble: KNOWLEDGE_PAINTERS.find((row) => row.able())?.id ?? "",
    };
  } catch (error) {
    return { thrown: String(error?.stack ?? error) };
  }
});

/* G1 — 다섯 언어에서 판의 글자와 사람이 읽는 속성, 그리고 알림의 자리. 메뉴는 열어서 읽는다(손의
 * 줄이 서 있던 자리다). 번역 표는 언어마다 모든 값을. */
const painterWords = await grammarAsk(async ({ wordSource }) => {
  try {
    const words = new RegExp(wordSource, "iu");
    const view = document.querySelector(".knowledge-view:not([hidden])");
    const HUMAN_ATTRIBUTES = ["aria-label", "aria-description", "title", "data-tip", "placeholder", "alt"];
    const heard = (root) => {
      if (root === null) return [];
      const said = [];
      const inText = root.textContent.match(words);
      if (inText !== null) said.push(`text:${inText[0]}`);
      for (const one of root.querySelectorAll("*")) {
        for (const name of HUMAN_ATTRIBUTES) {
          const value = one.getAttribute(name);
          if (value !== null && words.test(value)) said.push(`${name}:${value}`);
        }
      }
      return said;
    };
    const localeWas = locale;
    const menu = view.querySelector(".knowledge-lens-toggle");
    const spoken = {};
    try {
      for (const { code } of LOCALES.filter((one) => one.code !== "system")) {
        locale = code;
        applyLocale();
        await paintKnowledgeView();
        menu.click();
        spoken[code] = [...heard(view), ...heard(document.querySelector(".toasts"))];
        menu.click();
      }
    } finally {
      locale = localeWas;
      applyLocale();
    }
    const catalog = Object.entries(CATALOG).flatMap(([code, rows]) => Object.entries(rows)
      .filter(([, said]) => words.test(String(said))).map(([key]) => `${code}:${key}`));
    /* 사람이 손을 고르던 줄은 판에 없다. */
    const controls = view.querySelectorAll(".knowledge-painter, [data-knowledge-painter]").length;
    return { spoken, catalog, controls };
  } catch (error) {
    return { thrown: String(error?.stack ?? error) };
  }
}, { wordSource: PAINTER_WORDS.source });

/* G3·G4 — 모양의 표, 그림의 점, 범례의 줄. */
const shapeGrammar = await grammarAsk(async () => {
  const view = document.querySelector(".knowledge-view:not([hidden])");
  try {
    const frame = () => new Promise((done) => requestAnimationFrame(done));
    const settle = async () => {
      for (let round = 0; round < 600; round += 1) {
        await frame();
        if ((knowledgeLayouts.get(view)?.left ?? 1) === 0) return;
      }
    };
    setKnowledgeMode(view, "global");
    /* 그림 검사는 점의 **요소**를 읽는다 — SVG 손을 못박는다(끝에서 놓는다). */
    knowledgePainterKind = "svg";
    await paintKnowledgeView();
    await settle();
    const layout = knowledgeLayouts.get(view);

    /* 표: 종류 다섯, 서로 다른 모양 다섯, 모양마다 기하 한 줄. */
    const kinds = Object.keys(KNOWLEDGE_NODE_SHAPES);
    const shapes = kinds.map((kind) => KNOWLEDGE_NODE_SHAPES[kind]);
    const table = { kinds, shapes, distinct: new Set(shapes).size,
      geometry: shapes.every((shape) => KNOWLEDGE_SHAPES[shape] !== undefined) };

    /* 다각형의 넓이 — 윤곽은 절대 좌표의 M·L·H·V·Z뿐이다. */
    const polygonArea = (path) => {
      const points = [];
      let x = 0;
      let y = 0;
      for (const [, op, rest] of path.matchAll(/([MLHVZ])([^MLHVZ]*)/gu)) {
        const numbers = (rest.match(/-?\d+(?:\.\d+)?/gu) ?? []).map(Number);
        if (op === "M" || op === "L") [x, y] = numbers;
        else if (op === "H") [x] = numbers;
        else if (op === "V") [y] = numbers;
        else continue;
        points.push([x, y]);
      }
      let twice = 0;
      for (let at = 0; at < points.length; at += 1) {
        const [ax, ay] = points[at];
        const [bx, by] = points[(at + 1) % points.length];
        twice += ax * by - bx * ay;
      }
      return Math.abs(twice) / 2;
    };
    const round4 = (value) => Math.round(value * 10000) / 10000;
    const picture = {};
    for (const kind of kinds) {
      const shape = KNOWLEDGE_NODE_SHAPES[kind];
      const row = KNOWLEDGE_SHAPES[shape];
      const seat = layout.model.kinds.findIndex((one, at) => one === kind
        && (layout.drawn === null || layout.drawn[at] === 1));
      const dot = seat < 0 ? null : layout.nodeEls[seat]?.querySelector(".knowledge-dot") ?? null;
      /* 크기가 약속한 원 — 모양과 무관한 반지름이다. 그린 넓이를 **이 원**의 넓이로 나눈다: 모양의
       * 비율로 곱한 반지름을 같은 비율로 다시 나누는 셈은 무엇도 묻지 않는다. 전체 지도의 쪽과 유령이 약속하는
       * 것은 지도의 작은 점이고(`knowledgeMapRadius`, t-12029 「spread」), 렌즈가 더한 점과 주변 탐색은 √차수 램프다. */
      const ramp = seat < 0 ? 0
        : knowledgeNodeRadius(layout.model.degree[seat], layout.tuning, layout.tier[seat]);
      const promised = seat < 0 ? 0
        : layout.ring === null && (kind === "page" || kind === "ghost")
          ? knowledgeMapRadius(kind, layout.tier[seat], layout.community[seat] >= layout.namedCount, layout.tuning, ramp)
          : ramp;
      const reach = seat < 0 ? 0 : layout.radius[seat];
      const area = dot === null ? 0
        : dot.localName === "circle" ? Math.PI * Number(dot.getAttribute("r")) ** 2
        : polygonArea(dot.getAttribute("d") ?? "");
      picture[kind] = {
        shape,
        drawn: dot?.dataset.shape ?? "",
        element: dot?.localName ?? "",
        wantedElement: row.outline === null ? "circle" : "path",
        fromTable: dot !== null && (row.outline === null
          ? dot.getAttribute("r") === String(reach)
          : dot.getAttribute("d") === row.outline(reach)),
        reachError: promised > 0 ? round4(Math.abs(reach / promised - row.reach)) : 1,
        areaRatio: promised > 0 ? round4(area / (Math.PI * promised * promised)) : 0,
        /* 점선은 GL에서는 표의 칸이, SVG에서는 옷이 긋는다 — 둘이 같은 말을 해야 한다. */
        dashedAgrees: dot !== null && (getComputedStyle(dot).strokeDasharray !== "none") === row.dashed,
      };
    }

    /* 범례의 모든 줄. 회상은 종류가 아니라 페이지를 두른 고리(`.knowledge-halo`, 점선 원)라 그 모양이
     * 채우지 않는 점선 원인지를 묻는다. */
    const legend = [...view.querySelectorAll(".knowledge-legend [data-node-kind]")].map((item) => {
      const kind = item.dataset.nodeKind;
      const ink = item.querySelector(".knowledge-legend-node .knowledge-legend-ink");
      const shape = ink?.dataset.shape ?? "";
      const row = KNOWLEDGE_SHAPES[shape];
      return {
        kind,
        shape,
        sameAsPicture: kind in KNOWLEDGE_NODE_SHAPES
          ? shape === picture[kind].drawn
          : kind === "recalled" && row?.outline === null && row?.dashed === true
            && layout.nodeEls.some((node) => node?.querySelector(".knowledge-halo")?.localName === "circle"),
        fromTable: row !== undefined && (row.outline === null
          ? ink.localName === "circle" && ink.getAttribute("r") === String(row.reach)
          : ink.localName === "path" && ink.getAttribute("d") === row.outline(row.reach)),
      };
    });
    return { table, picture, legend };
  } catch (error) {
    return { thrown: String(error?.stack ?? error) };
  } finally {
    knowledgePainterKind = null;
    knowledgeShowSources = false;
    knowledgeShowCode = false;
    activeWorktreePath = window.__GRAMMAR_HELD_WORKTREE__ ?? null;
    await paintKnowledgeView();
  }
});
/* 넓이의 허용: 윤곽의 좌표는 0.01 px로 반올림되고(가장 작은 3 px 점에서 넓이 0.1% 안),
 * 설계의 합격선은 2%다(§2 G3). */
const AREA_SLACK = 0.02;
const shapeDetail = JSON.stringify(shapeGrammar);
ok("P1 G3: seven kinds meet seven shapes one to one, and every shape has one geometry row",
  !shapeGrammar.thrown
    && shapeGrammar.table.kinds.join(",")
      === "page,ghost,source,component,vulnerability,code_file,code_symbol"
    && shapeGrammar.table.distinct === shapeGrammar.table.kinds.length && shapeGrammar.table.geometry,
  shapeDetail);
ok("P1 G3: every kind stands in its own shape, drawn from its geometry row, at the area its size promises",
  !shapeGrammar.thrown && Object.values(shapeGrammar.picture).every((row) => row.drawn === row.shape
    && row.element === row.wantedElement && row.fromTable && row.reachError < 1e-4
    && Math.abs(row.areaRatio - 1) < AREA_SLACK && row.dashedAgrees),
  shapeDetail);
ok("P1 G4: every legend row draws the picture's own shape from the same geometry row",
  !shapeGrammar.thrown && ["page", "ghost", "source", "recalled", "code_file", "code_symbol"]
    .every((kind) => shapeGrammar.legend.some((row) => row.kind === kind))
    && shapeGrammar.legend.every((row) => row.sameAsPicture && row.fromTable),
  shapeDetail);
ok("P1 G1: no painter word (SVG, GL, WebGL) in the view's words, human attributes or toasts in five languages, nor anywhere in the translation table",
  !painterWords.thrown && Object.keys(painterWords.spoken).length === 5
    && Object.values(painterWords.spoken).every((said) => said.length === 0)
    && painterWords.catalog.length === 0 && painterWords.controls === 0,
  JSON.stringify(painterWords));
ok("P1 G2: the window, not a person, picks the painter — unpinned, the table's first able row stands and 2D is always able",
  !painterPick.thrown && painterPick.pinned === null
    && painterPick.standing === painterPick.firstAble
    && painterPick.order.at(-1) === "svg" && painterPick.able.at(-1) === true,
  JSON.stringify(painterPick));

/* 공급망 렌즈(P4·P5) — 모양의 문법 위에 선다. 계약은 `knowledge-supply.mjs`에, 천 개의 구성요소의
 * 첫 그림은 이 판(SVG)과 GL 판(아래)에서 적고, 진짜 GPU의 시간은 `knowledge-gpu.mjs`가 WebKit에서 잰다. */
await testKnowledgeSupply(page, ok);
await measureKnowledgeSupplyScene(page, ok, { painter: "svg" });

/* 코드 층(t-5970 G2) — 같은 모양의 문법 위에 서는 또 하나의 렌즈. 계약과 실측은 `knowledge-code.mjs`. */
await testKnowledgeCode(page, ok);
await measureKnowledgeCodeScene(page, ok);

/* GL의 계약은 제 판에서 묻는다(위의 `GL_ARGS` 주석). 그 판의 시간은 소프트웨어의
 * 것이므로 훑는 프레임을 짧게 잡는다 — 여기서 세는 것은 드로우와 자리이지 ms가
 * 아니다. */
const glBrowser = await chromium.launch({ args: GL_ARGS });
const glPage = await glBrowser.newPage({ viewport: { width: 1280, height: 860 } });
glPage.on("pageerror", (error) => faults.push(error?.stack ?? String(error)));
await seedKnowledgeWindow(glPage);
await glPage.goto(`${origin}/index.html`);
await glPage.waitForFunction(() => typeof BOUND !== "undefined" && BOUND.size > 0);
await glPage.evaluate(() => {
  window.__VAULT__ = { pages: 12, linksPer: 2, ghosts: 2, tags: ["core", "reading"] };
  document.getElementById("nav-knowledge").click();
});
await glPage.waitForFunction(() => document.querySelector(".knowledge-view:not([hidden])") !== null,
  null, { timeout: 8000 });
await measureKnowledgeGlParity(glPage, ok);
await measureKnowledgeScenes(glPage, ok, { painter: "gl", frames: sweep.frames,
  scenes: KNOWLEDGE_SCENES.filter((scene) => sweep.glScenes.includes(scene.name)) });

/* P1 G2 — WebGL2가 서는 판에서는 사람이 고르지 않아도 GL이 선다. */
const glPicked = await glPage.evaluate(async () => {
  try {
    const view = document.querySelector(".knowledge-view:not([hidden])");
    knowledgePainterKind = null;
    await paintKnowledgeView();
    await new Promise((done) => requestAnimationFrame(done));
    return { able: knowledgeGlSupported(), pinned: knowledgePainterKind,
      standing: knowledgePainterFor(view).id,
      canvases: view.querySelectorAll(".knowledge-gl").length,
      /* 점의 층만 센다 — GL 손은 CSS의 색을 읽으려고 숨은 견본 점(`.knowledge-gl-swatch`)을 따로 둔다. */
      elements: view.querySelectorAll(".knowledge-nodes .knowledge-node").length };
  } catch (error) {
    return { thrown: String(error?.stack ?? error) };
  }
});
ok("P1 G2: where WebGL2 stands the window draws with GL on its own — nothing pinned, one GL canvas, no element per page",
  !glPicked.thrown && glPicked.able && glPicked.pinned === null && glPicked.standing === "gl"
    && glPicked.canvases === 1 && glPicked.elements === 0,
  JSON.stringify(glPicked));
await measureKnowledgeSupplyParity(glPage, ok);
await measureKnowledgeSupplyScene(glPage, ok, { painter: "gl" });

/* 확대하면 이름이 선다(09-28, `placeKnowledgeLabels`). 같은 판을 두 손으로 배율 1과 2에서
 * 세운다. 배율 1에서는 두 손이 같은 격자의 답을 입는다. 이름표가 모든 점 위에 서는 GL 손은
 * 확대한 판에서 이름이 남의 점을 덮어도 되므로 제 배율 1보다, 그리고 같은 배율의 SVG 손보다
 * 많은 이름을 세운다 — 어느 손도 이름끼리는 덮지 않고, 예산(배율에 비례)을 넘지 않는다.
 * 제목은 실제 볼트처럼 문장이고(짧은 이름은 점 사이에도 선다), 이름 없는 군집의 쪽들이 바깥
 * 띠에 선다 — 배율 1의 전체 지도에서 그 쪽들은 예산을 쓰지 않는다(옛 격자는 같은 판에서 이름
 * 40개 중 36개를 띠에 세웠다). 판은 공급망 장면과 같은 넓은 창이다. */
const zoomSeat = glPage.viewportSize();
await glPage.setViewportSize({ width: 1998, height: 1069 });
const zoomNames = await glPage.evaluate(async () => {
  try {
    const frame = () => new Promise((done) => requestAnimationFrame(done));
    const view = document.querySelector(".knowledge-view:not([hidden])");
    setKnowledgeMode(view, "global", { paint: false });
    knowledgeSelectedKey = null;
    knowledgeQuery = "";
    const pages = 400;
    const answer = window.__buildVaultGraph__({ path: "/scene/zoom-names", sources: false }, {
      pages, ghosts: 10, linksPer: 3, tags: ["core", "reading", "tools"],
      titles: Array.from({ length: pages }, (unused, at) => `개념 ${at}의 제목은 문장이라 점 사이에 서기 어렵다`),
    });
    for (let at = 0; at < 12; at += 1) {
      answer.graph.nodes.push({ ...answer.graph.nodes[0], id: `wiki/Alone-${at}.md`, title: `홀로 선 쪽 ${at}` });
    }
    answer.graph.pages += 12;
    knowledgeLayouts.delete(view);
    const host = view.querySelector(".knowledge-nodes");
    host.dataset.knowledgeSignature = "";
    host.dataset.knowledgeVault = "";
    host.replaceChildren();
    view.querySelector(".knowledge-edges").replaceChildren();
    /* 새 볼트의 첫 방문은 주변 탐색이다(t-4140 S2) — 이 판이 묻는 것은 전체 지도다. */
    noteKnowledgeExploreLines({ [answer.vault]: JSON.stringify({ mode: "global" }) });
    knowledgeReport = answer;
    knowledgeAskedAt = Date.now();
    const read = () => {
      const layout = knowledgeLayouts.get(view);
      const { count, labelShown, community, namedCount, labelAtX, labelAtY, labelEm, tuning } = layout;
      const kinds = layout.model.kinds;
      const boxes = [];
      let named = 0;
      let strays = 0;
      let strayNamed = 0;
      for (let at = 0; at < count; at += 1) {
        const stray = community[at] >= namedCount && (kinds[at] === "page" || kinds[at] === "ghost");
        if (stray) strays += 1;
        if (labelShown[at] !== 1) continue;
        named += 1;
        if (stray) strayNamed += 1;
        const half = (labelEm[at] * tuning.labelPx) / 2;
        boxes.push([labelAtX[at] - half, labelAtY[at] - tuning.labelPx / 2,
          labelAtX[at] + half, labelAtY[at] + tuning.labelPx / 2]);
      }
      let overlaps = 0;
      for (let left = 0; left < boxes.length; left += 1) {
        for (let right = left + 1; right < boxes.length; right += 1) {
          const one = boxes[left];
          const two = boxes[right];
          if (one[0] < two[2] && two[0] < one[2] && one[1] < two[3] && two[1] < one[3]) overlaps += 1;
        }
      }
      const budget = Math.round(tuning.labelBudget[view.dataset.knowledgeTier ?? "wide"] * layout.zoom);
      return { painter: knowledgePainterFor(view).id, ring: layout.ring !== null, count, zoom: layout.zoom,
        named, budget, strays, strayNamed, overlaps };
    };
    const at = async (hand, zoom) => {
      knowledgePainterKind = hand;
      await paintKnowledgeView();
      for (let wait = 0; wait < 1200 && (knowledgeLayouts.get(view)?.left ?? 1) > 0; wait += 1) await frame();
      const layout = knowledgeLayouts.get(view);
      if (zoom === 1) fitKnowledgeGraph(view, layout);
      else takeKnowledgeZoom(view, layout, zoom);
      await paintKnowledgeView();
      for (let wait = 0; wait < 3; wait += 1) await frame();
      return read();
    };
    const rows = [await at("svg", 1), await at("gl", 1), await at("svg", 2), await at("gl", 2)];
    knowledgePainterKind = null;
    fitKnowledgeGraph(view, knowledgeLayouts.get(view));
    await paintKnowledgeView();
    return { rows };
  } catch (error) {
    return { thrown: String(error?.stack ?? error) };
  }
});
await glPage.setViewportSize(zoomSeat);
{
  const [svgFit, glFit, svgZoomed, glZoomed] = zoomNames.rows ?? [];
  ok("zooming in names more of the map: GL names may cover other points but never other names, and the rim's pages stay quiet at the overview",
    !zoomNames.thrown
      && zoomNames.rows.every((row) => !row.ring && row.overlaps === 0 && row.named <= row.budget)
      && svgFit.painter === "svg" && glFit.painter === "gl" && glZoomed.zoom === 2
      /* 배율 1에서 GL은 SVG만큼은 이름을 세운다 — 같지는 않다: GL의 주제 이름판은 부스러기 점 위에도
       * 서므로(t-11500) 이름판이 남긴 자리가 두 손에서 다르다. */
      && glFit.named >= svgFit.named
      && glZoomed.named > glFit.named && glZoomed.named > svgZoomed.named
      && svgFit.strays > 0 && svgFit.strayNamed === 0 && glFit.strayNamed === 0,
    JSON.stringify(zoomNames));
}

/* 테마를 바꾸면 GL 손의 그림도 바뀐다(t-11500). SVG는 스타일시트가 곧 옷이라 저절로 따라오지만,
 * GL의 점·선은 견본에게 물어 바이트로 올린 색이다 — 창의 테마 단추 길(`setTheme`) 한 번 뒤에
 * 아무도 건드리지 않은 그림이, 같은 장면·같은 카메라를 그 테마로 처음부터 그린 그림과 픽셀까지
 * 같아야 한다. 두 방향, 전체 지도와 주변 탐색. 옛 코드는 넷 모두에서 다른 그림이었다(점·선이
 * 앞 테마의 색으로 남았다 — 카메라를 움직여도). */
const themeScene = async (mode, from) => glPage.evaluate(async ({ mode, from }) => {
  const frame = () => new Promise((done) => requestAnimationFrame(done));
  setTheme(from);
  knowledgePainterKind = "gl";
  knowledgeQuery = "";
  knowledgeSelectedKey = null;
  const view = document.querySelector(".knowledge-view:not([hidden])");
  const answer = window.__buildVaultGraph__({ path: `/scene/theme-${mode}`, sources: false },
    { pages: 160, ghosts: 12, linksPer: 3, tags: ["core", "reading", "tools"] });
  knowledgeLayouts.delete(view);
  const host = view.querySelector(".knowledge-nodes");
  host.dataset.knowledgeSignature = "";
  host.dataset.knowledgeVault = "";
  host.replaceChildren();
  view.querySelector(".knowledge-edges").replaceChildren();
  noteKnowledgeExploreLines({ [answer.vault]: JSON.stringify({ mode: "global" }) });
  setKnowledgeMode(view, "global", { paint: false });
  knowledgeReport = answer;
  await paintKnowledgeView();
  for (let wait = 0; wait < 1200 && (knowledgeLayouts.get(view)?.left ?? 1) > 0; wait += 1) await frame();
  if (mode === "local") {
    setKnowledgeMode(view, "local", { paint: false });
    await paintKnowledgeView();
    for (let wait = 0; wait < 1200 && (knowledgeLayouts.get(view)?.left ?? 1) > 0; wait += 1) await frame();
  }
  await paintKnowledgeView();
  for (let wait = 0; wait < 3; wait += 1) await frame();
  return { painter: knowledgePainterFor(view).id, mode: knowledgeMode };
}, { mode, from });
/* 전이가 끝난 뒤의 **GL 층** — 캔버스 위의 HTML·SVG 층(범례, 배율 단추, 알림, 이름판의 <svg>, 이름표
 * 오버레이)은 숨기고 찍는다. 그 층들은 스타일시트가 곧 옷이라 이 결함과 무관하고, 범례 점의 가장자리
 * 한 픽셀이 합성의 반올림으로 한 단계 달라지는 일이 있다(실측 Chromium: GL 층은 0, 범례 점 1픽셀). 끝이
 * 있는 애니메이션(테마가 건 CSS 전이)도 다 끝나기를 기다린다. */
const themeShot = async () => {
  await glPage.waitForTimeout(700);
  await glPage.evaluate(() => Promise.all(document.getAnimations()
    .filter((one) => Number.isFinite(one.effect?.getComputedTiming().endTime ?? Number.POSITIVE_INFINITY))
    .map((one) => one.finished.catch(() => null))));
  const layers = ".knowledge-cluster-legend, .knowledge-zoom, .knowledge-error, .knowledge-crumb, .knowledge-legend,"
    + " .knowledge-picture, .knowledge-gl-labels";
  await glPage.evaluate((layers) => {
    for (const one of document.querySelectorAll(layers)) one.style.visibility = "hidden";
    return new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
  }, layers);
  const shot = await glPage.locator(".knowledge-view:not([hidden]) .knowledge-canvas").screenshot();
  await glPage.evaluate((layers) => {
    for (const one of document.querySelectorAll(layers)) one.style.visibility = "";
  }, layers);
  return shot;
};
const themeRuns = [];
for (const mode of ["global", "local"]) {
  for (const [from, to] of [["dark", "light"], ["light", "dark"]]) {
    const scene = await themeScene(mode, from);
    await glPage.evaluate(async (to) => {
      setTheme(to);
      for (let wait = 0; wait < 10; wait += 1) await new Promise((done) => requestAnimationFrame(done));
    }, to);
    const switched = await themeShot();
    /* 처음 그린 그림: GL 손을 놓고 새로 세운다 — 새 손은 새 테마의 견본을 처음 묻는다(자리·카메라는 판의 것이라 그대로). */
    await glPage.evaluate(async () => {
      knowledgePainterKind = "svg";
      await paintKnowledgeView();
      knowledgePainterKind = "gl";
      await paintKnowledgeView();
      for (let wait = 0; wait < 3; wait += 1) await new Promise((done) => requestAnimationFrame(done));
    });
    const fresh = await themeShot();
    themeRuns.push({ ...scene, from, to, same: switched.equals(fresh) });
  }
}
await glPage.evaluate(() => setTheme("dark"));
ok("switching the theme repaints the GL picture as a fresh paint in that theme (both ways, overview and local)",
  themeRuns.length === 4 && themeRuns.every((run) => run.painter === "gl" && run.same)
    && themeRuns.filter((run) => run.mode === "local").length === 2,
  JSON.stringify(themeRuns));

/* 주변 탐색은 두 손이 같은 문법으로 그린다(t-11500). SVG의 `is-local` 절은 성운(전체 지도의 배경
 * 원)을 숨기고, 중심에 닿는 바퀴살만 또렷하게 두고 이웃끼리 잇는 선은 옅게 두며, 점을 군집의
 * 색으로 칠하고 중심을 키워 후광을 두른다. 같은 장면을 두 손으로 세워 SVG가 계산한 옷과 GL이
 * 올린 바이트를 선마다·점마다 견준다. 옛 GL은 성운을 그렸고 모든 선을 한 옷으로 그렸다. */
const localGrammar = await glPage.evaluate(async () => {
  try {
    const frame = () => new Promise((done) => requestAnimationFrame(done));
    const settle = async () => {
      for (let wait = 0; wait < 3; wait += 1) await frame();
      /* 점과 선의 옷은 전이(`--motion-fast`)로 바뀐다 — 끝난 값을 읽는다. */
      await new Promise((done) => setTimeout(done, 400));
    };
    knowledgeQuery = "";
    knowledgeSelectedKey = null;
    knowledgeClusterPicked = -1;
    const view = document.querySelector(".knowledge-view:not([hidden])");
    const answer = window.__buildVaultGraph__({ path: "/scene/local-grammar", sources: false },
      { pages: 160, ghosts: 12, linksPer: 3, tags: ["core", "reading", "tools"] });
    knowledgeLayouts.delete(view);
    const host = view.querySelector(".knowledge-nodes");
    host.dataset.knowledgeSignature = "";
    host.dataset.knowledgeVault = "";
    host.replaceChildren();
    view.querySelector(".knowledge-edges").replaceChildren();
    noteKnowledgeExploreLines({ [answer.vault]: JSON.stringify({ mode: "global" }) });
    setKnowledgeMode(view, "global", { paint: false });
    knowledgePainterKind = "svg";
    knowledgeReport = answer;
    await paintKnowledgeView();
    for (let wait = 0; wait < 1200 && (knowledgeLayouts.get(view)?.left ?? 1) > 0; wait += 1) await frame();
    setKnowledgeMode(view, "local", { paint: false });
    await paintKnowledgeView();
    await settle();
    const layout = knowledgeLayouts.get(view);
    const ink = (word, opacity = 1) => {
      const into = new Float32Array(4);
      knowledgeGlColor(word, into, 0);
      into[3] *= opacity;
      return [...into];
    };
    /* SVG의 답: 계산된 옷. */
    const svgEdges = new Map();
    for (let at = 0; at < layout.model.edgeCount; at += 1) {
      const line = knowledgeEdgeLine(layout, at);
      if (line === null) continue;
      const style = getComputedStyle(line);
      svgEdges.set(at, { ink: ink(style.stroke, Number.parseFloat(style.opacity)),
        width: Number.parseFloat(style.strokeWidth),
        spoke: line.classList.contains("is-spoke") });
    }
    const centre = layout.ring.seat;
    const svgNodes = new Map();
    for (let at = 0; at < layout.count; at += 1) {
      const dot = layout.nodeEls[at]?.querySelector(".knowledge-dot");
      if (dot === undefined || dot === null || layout.model.kinds[at] !== "page") continue;
      const style = getComputedStyle(dot);
      svgNodes.set(at, { fill: ink(style.fill), stroke: ink(style.stroke) });
    }
    const centreDot = getComputedStyle(layout.nodeEls[centre].querySelector(".knowledge-dot"));
    const centreScale = new DOMMatrix(centreDot.transform).a;
    const centreHalo = getComputedStyle(layout.nodeEls[centre].querySelector(".knowledge-halo")).display;
    const nebula = view.querySelector(".knowledge-picture .knowledge-nebula");
    const svgNebula = nebula === null ? "none" : getComputedStyle(nebula).display;
    /* GL의 답: 올린 바이트. */
    knowledgePainterKind = "gl";
    await paintKnowledgeView();
    await settle();
    const painter = knowledgePainters.get(view);
    const near = (one, two, slack) => one.every((value, at) => Math.abs(value - two[at]) <= slack);
    const byte = 2.5 / 255;
    let edgeMisses = 0;
    const edgeMissed = [];
    const glWidths = { spoke: new Set(), context: new Set() };
    for (let seat = 0; seat < painter.counts.edges; seat += 1) {
      const head = painter.edgeEnds[seat * 2];
      const tail = painter.edgeEnds[seat * 2 + 1];
      let at = -1;
      for (let edge = 0; edge < layout.model.edgeCount; edge += 1) {
        if (layout.model.from[edge] === head && layout.model.to[edge] === tail && svgEdges.has(edge)) { at = edge; break; }
      }
      if (at < 0) continue;
      const svg = svgEdges.get(at);
      const gl = [0, 1, 2, 3].map((channel) => painter.edgeInk[seat * 4 + channel] / 255);
      const width = painter.edgeShape[seat * 4] / 8;
      glWidths[svg.spoke ? "spoke" : "context"].add(`${width}/${Math.round(gl[3] * 255)}`);
      if (!near(gl, svg.ink, byte) || Math.abs(width - svg.width) > 1 / 8) {
        edgeMisses += 1;
        if (edgeMissed.length < 3) edgeMissed.push({ at, spoke: svg.spoke, svg, gl, width });
      }
    }
    let nodeMisses = 0;
    const nodeMissed = [];
    for (let seat = 0; seat < painter.counts.nodes; seat += 1) {
      const at = painter.nodeSeat[seat];
      const svg = svgNodes.get(at);
      if (svg === undefined) continue;
      const fill = [0, 1, 2, 3].map((channel) => painter.nodeFill[seat * 4 + channel] / 255);
      const stroke = [0, 1, 2, 3].map((channel) => painter.nodeStroke[seat * 4 + channel] / 255);
      if (!near(fill, svg.fill, byte) || !near(stroke, svg.stroke, byte)) {
        nodeMisses += 1;
        if (nodeMissed.length < 3) nodeMissed.push({ at, centre: at === centre, svg, fill, stroke });
      }
    }
    const centreRadius = painter.seatData[centre * 4 + 3];
    let centreRing = false;
    for (let ring = 0; ring < painter.counts.rings; ring += 1) {
      if (painter.ringSeat[ring] === centre) centreRing = true;
    }
    const result = {
      svgEdges: svgEdges.size, glEdges: painter.counts.edges, edgeMisses, edgeMissed,
      svgNodes: svgNodes.size, nodeMisses, nodeMissed,
      glWidths: { spoke: [...glWidths.spoke], context: [...glWidths.context] },
      svgNebula, glDiscs: painter.counts.discs,
      centreScale, centreRadius, centreBody: layout.radius[centre], centreHalo, centreRing,
    };
    knowledgePainterKind = null;
    setKnowledgeMode(view, "global", { paint: false });
    await paintKnowledgeView();
    return result;
  } catch (error) {
    return { thrown: String(error?.stack ?? error) };
  }
});
ok("GL local exploration wears the SVG's grammar: no nebula, spokes apart from context lines, cluster-coloured points, a larger centre with its halo",
  !localGrammar.thrown && localGrammar.svgNebula === "none" && localGrammar.glDiscs === 0
    && localGrammar.svgEdges > 10 && localGrammar.edgeMisses === 0
    && localGrammar.glWidths.spoke.length > 0 && localGrammar.glWidths.context.length > 0
    && localGrammar.glWidths.spoke.every((row) => !localGrammar.glWidths.context.includes(row))
    && localGrammar.svgNodes > 5 && localGrammar.nodeMisses === 0
    && localGrammar.centreScale > 1
    && Math.abs(localGrammar.centreRadius - localGrammar.centreBody * localGrammar.centreScale) < 0.01
    && localGrammar.centreHalo !== "none" && localGrammar.centreRing,
  JSON.stringify(localGrammar));

/* 주제의 이름판은 그리는 글자만큼 자리를 쥔다(t-11500). 판이 그린 줄마다 그 가운데 줄의 칸이 모두
 * 이번 격자에 쥐어졌는지, 판의 줄끼리·판과 이름이 겹치지 않는지를 두 손에서 묻는다 — 판 여섯·스물(넓은
 * 판)과 빽빽한 좁은 판. 판은 이름 한 줄이다(t-12029, 시안 v2 「oneLine」): 선 판마다 보이는 줄이 꼭 하나. SVG의
 * 판은 점을 덮지 않는다(점이 판의 글자 위에 그려지므로). GL의 판은 점 위의 층이라 제 원반 곁에 선다 — 가까운
 * 자리가 다 막힌 판은 점을 덮고서라도(t-12029): 모든 GL 판이 원반 가장자리의 가까운 세 겹 안이다. */
const plateScene = (tags, pages) => glPage.evaluate(async ({ tags, pages }) => {
  const frame = () => new Promise((done) => requestAnimationFrame(done));
  const view = document.querySelector(".knowledge-view:not([hidden])");
  const rows = [];
  try {
    for (const hand of ["svg", "gl"]) {
      knowledgePainterKind = hand;
      knowledgeQuery = "";
      knowledgeSelectedKey = null;
      knowledgeClusterPicked = -1;
      const answer = window.__buildVaultGraph__({ path: `/scene/plates-${tags}`, sources: false },
        { pages, ghosts: 30, linksPer: 3, tags: Array.from({ length: tags }, (unused, at) => `topic-${at}`) });
      knowledgeLayouts.delete(view);
      const host = view.querySelector(".knowledge-nodes");
      host.dataset.knowledgeSignature = "";
      host.dataset.knowledgeVault = "";
      host.replaceChildren();
      view.querySelector(".knowledge-edges").replaceChildren();
      noteKnowledgeExploreLines({ [answer.vault]: JSON.stringify({ mode: "global" }) });
      setKnowledgeMode(view, "global", { paint: false });
      knowledgeReport = answer;
      await paintKnowledgeView();
      for (let wait = 0; wait < 1500 && (knowledgeLayouts.get(view)?.left ?? 1) > 0; wait += 1) await frame();
      const layout = knowledgeLayouts.get(view);
      fitKnowledgeGraph(view, layout);
      await paintKnowledgeView();
      for (let wait = 0; wait < 3; wait += 1) await frame();
      const canvas = view.querySelector(".knowledge-canvas").getBoundingClientRect();
      const cell = Math.max(1, layout.tuning.labelCell);
      const { labelCells, labelCols, labelRows, labelGeneration } = layout;
      const lines = [];
      let unreserved = 0;
      let standing = 0;
      let drawnLines = 0;
      let folded = 0;
      for (let rank = 0; rank < layout.namedCount; rank += 1) {
        const held = layout.clusterEls[rank];
        if (!held || layout.clusterTally[rank] === 0) continue;
        if (held.label.classList.contains("is-folded")) {
          folded += 1;
          continue;
        }
        standing += 1;
        for (const line of held.label.querySelectorAll("text")) {
          if (getComputedStyle(line).display === "none") continue;
          const seat = line.getBoundingClientRect();
          if (seat.width === 0) continue;
          drawnLines += 1;
          lines.push({ rank, box: [seat.left, seat.top, seat.right, seat.bottom] });
          /* 그 줄의 가운데 줄의 칸 — 글자가 서는 모든 칸이 이번 격자에 쥐어져 있어야 한다. */
          const row = Math.floor(((seat.top + seat.bottom) / 2 - canvas.top) / cell);
          for (let col = Math.floor((seat.left + 1 - canvas.left) / cell);
            col <= Math.floor((seat.right - 1 - canvas.left) / cell); col += 1) {
            if (row < 0 || col < 0 || row >= labelRows || col >= labelCols) continue;
            if (labelCells[row * labelCols + col] !== labelGeneration) unreserved += 1;
          }
        }
      }
      const hit = (one, two) => one[2] - two[0] > 0.5 && two[2] - one[0] > 0.5
        && one[3] - two[1] > 0.5 && two[3] - one[1] > 0.5;
      let overlaps = 0;
      for (let one = 0; one < lines.length; one += 1) {
        for (let two = one + 1; two < lines.length; two += 1) {
          if (lines[one].rank !== lines[two].rank && hit(lines[one].box, lines[two].box)) overlaps += 1;
        }
      }
      const words = [...view.querySelectorAll(".knowledge-node.is-named .knowledge-label, .knowledge-gl-label:not([hidden])")]
        .map((word) => {
          const seat = word.getBoundingClientRect();
          return [seat.left, seat.top, seat.right, seat.bottom];
        })
        .filter((box) => box[2] > box[0]);
      for (const line of lines) for (const word of words) if (hit(line.box, word)) overlaps += 1;
      /* 조작부(범례·배율 단추·빵부스러기)와는 겹치지 않는다. */
      for (const control of view.querySelectorAll(".knowledge-cluster-legend, .knowledge-zoom, .knowledge-crumb, .knowledge-legend")) {
        const seat = control.getBoundingClientRect();
        if (seat.width === 0 || control.hidden || getComputedStyle(control).display === "none") continue;
        for (const line of lines) if (hit(line.box, [seat.left, seat.top, seat.right, seat.bottom])) overlaps += 1;
      }
      /* 판의 줄 아래에 깔린 점: 부스러기(이름 없는 군집의 쪽·유령)만 되고, 그것도 GL에서만(t-11500 E). */
      let underRim = 0;
      let underOther = 0;
      const kinds = layout.model.kinds;
      for (let at = 0; at < layout.count; at += 1) {
        if (layout.drawn !== null && layout.drawn[at] === 0) continue;
        const px = canvas.left + layout.project.screenX(at);
        const py = canvas.top + layout.project.screenY(at);
        const body = [px - layout.radius[at], py - layout.radius[at], px + layout.radius[at], py + layout.radius[at]];
        if (!lines.some((line) => hit(line.box, body))) continue;
        const rim = layout.community[at] >= layout.namedCount && (kinds[at] === "page" || kinds[at] === "ghost");
        if (rim) underRim += 1;
        else underOther += 1;
      }
      /* 판의 가운데에서 제 원반 가장자리까지(화면 px) — 가장 먼 판. */
      let farthest = 0;
      for (let rank = 0; rank < layout.namedCount; rank += 1) {
        const held = layout.clusterEls[rank];
        if (!held || layout.clusterTally[rank] === 0 || held.label.classList.contains("is-folded")) continue;
        const seat = held.label.querySelector("text").getBoundingClientRect();
        const discX = canvas.left + (layout.clusterX[rank] - layout.viewBoxRect.x) * layout.scale;
        const camera = layout.viewBoxRect;
        const discY = canvas.top + (camera.middleY + (layout.clusterY[rank] - camera.middleY) * camera.yScale
          - camera.y) * layout.scale;
        const gapX = Math.max(0, Math.abs((seat.left + seat.right) / 2 - discX) - seat.width / 2);
        const gapY = Math.max(0, Math.abs((seat.top + seat.bottom) / 2 - discY) - seat.height / 2);
        farthest = Math.max(farthest, Math.hypot(gapX, gapY) - layout.clusterReach[rank] * layout.scale);
      }
      rows.push({ tags, hand: knowledgePainterFor(view).id, standing, drawnLines, folded, unreserved, overlaps,
        underRim, underOther, farthest: Math.round(farthest) });
    }
    return { rows };
  } catch (error) {
    return { thrown: String(error?.stack ?? error), rows };
  } finally {
    knowledgePainterKind = null;
  }
}, { tags, pages });
/* 시안의 가까운 세 겹(원반 가장자리에서 px) — GL 판 가운데 가장 먼 것도 가장 먼 겹의 두 배 안이다(대각선은
 * 가장자리의 0.72라 겹보다 조금 더 멀다; 옛 판은 124 px 밖에 섰다). */
const KNOWLEDGE_PLATE_GAPS = await glPage.evaluate(() => [...KNOWLEDGE_PLATE_SEATS.gaps]);
const plateSeat = glPage.viewportSize();
const plateSeats = { rows: [] };
for (const [tags, pages, wide, tall] of [[6, 359, 1280, 860], [20, 600, 1280, 860], [20, 600, 900, 700]]) {
  await glPage.setViewportSize({ width: wide, height: tall });
  const scene = await plateScene(tags, pages);
  if (scene.thrown) plateSeats.thrown = scene.thrown;
  plateSeats.rows.push(...scene.rows.map((row) => ({ ...row, wide })));
}
await glPage.setViewportSize(plateSeat);
ok("cluster plates reserve the words they wear: every plate is one drawn line on its own cells, no plate meets another plate, a name or a control, SVG plates cover no point, and every GL plate stands at its own disc's edge",
  !plateSeats.thrown && plateSeats.rows.length === 6
    && plateSeats.rows.every((row) => row.unreserved === 0 && row.overlaps === 0 && row.standing > 0)
    && plateSeats.rows.every((row) => row.drawnLines === row.standing)
    && plateSeats.rows.every((row) => (row.hand === "gl"
      ? row.farthest < 2 * Math.max(...KNOWLEDGE_PLATE_GAPS) : row.underOther === 0 && row.underRim === 0)),
  JSON.stringify(plateSeats));

/* 이름판의 글자는 두 테마에서 바탕과 대비를 지킨다(t-11500 E) — GL의 판은 부스러기 점 위에도 서므로, 글자는
 * 제 바탕색 테두리 위에 선다: 잴 것은 글자와 판의 바탕. 색 있는 주제의 이름은 색 칸의 잉크(Test 8과 같은 3:1),
 * 색 없는 주제의 작은 이름(t-12029)은 작은 글자의 4.5:1. 색은 Test 8처럼 픽셀로 읽는다. */
const plateInk = await glPage.evaluate(async () => {
  const paint = document.createElement("canvas").getContext("2d", { willReadFrequently: true });
  const parse = (color) => {
    const sentinel = "#010203";
    paint.fillStyle = sentinel;
    paint.fillStyle = color;
    if (paint.fillStyle === sentinel) return null;
    paint.clearRect(0, 0, 1, 1);
    paint.fillRect(0, 0, 1, 1);
    const held = paint.getImageData(0, 0, 1, 1).data;
    return [held[0] / 255, held[1] / 255, held[2] / 255];
  };
  const luminance = (color) => {
    const channels = parse(color);
    if (!channels) return null;
    return channels.map((channel) => (channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4))
      .reduce((sum, channel, at) => sum + channel * [0.2126, 0.7152, 0.0722][at], 0);
  };
  const ratio = (left, right) => {
    const a = luminance(left);
    const b = luminance(right);
    return a === null || b === null ? 0 : (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
  };
  const view = document.querySelector(".knowledge-view:not([hidden])");
  const rows = [];
  for (const theme of ["dark", "light"]) {
    setTheme(theme);
    await new Promise((done) => setTimeout(done, 700));
    const ground = getComputedStyle(view.querySelector(".knowledge-canvas")).backgroundColor;
    const layout = knowledgeLayouts.get(view);
    for (let rank = 0; rank < layout.namedCount; rank += 1) {
      const label = layout.clusterEls[rank]?.label;
      if (!label || label.classList.contains("is-folded")) continue;
      const quiet = label.classList.contains("is-quiet");
      for (const [line, least] of [[".knowledge-cluster-name", quiet ? 4.5 : 3]]) {
        const word = label.querySelector(line);
        if (!word) continue;
        const style = getComputedStyle(word);
        rows.push({ theme, rank, line, least, ratio: Math.round(ratio(style.fill, ground) * 100) / 100,
          outline: ratio(style.stroke, ground) < 1.01 });
      }
    }
  }
  setTheme("dark");
  return rows;
});
ok("cluster plate words keep their contrast on the graph ground in both themes, standing on an outline of that ground",
  plateInk.length > 0 && ["dark", "light"].every((theme) => plateInk.some((row) => row.theme === theme))
    && plateInk.every((row) => row.ratio >= row.least && row.outline),
  JSON.stringify(plateInk.filter((row) => row.ratio < row.least || !row.outline).slice(0, 8)));

/* 부스러기 띠는 이름 있는 원반들의 윤곽에 붙는다(t-11500). 원반 둘이 아령처럼 선 볼트(주제 둘 × 120쪽,
 * 한 가닥으로 이어짐)에 고아 40쪽을 두면, 가장 먼 원반 끝을 반지름으로 한 원 밖에서 띠를 시작하는
 * 옛 배치는 아령의 옆구리에서 원반 반지름만큼의 빈 땅을 남긴다. 윤곽(원반들의 볼록 껍질 — 지지 함수
 * h(u) = max(c·u + r))에서 첫 부스러기들까지의 거리가 나선 한 걸음 안이고, 모든 부스러기가 모든
 * 원반과 여전히 띠의 틈(`--knowledge-stray-gap`, t-12029) 이상 떨어지는지 묻는다. */
const rimHug = await glPage.evaluate(async () => {
  const frame = () => new Promise((done) => requestAnimationFrame(done));
  try {
    const view = document.querySelector(".knowledge-view:not([hidden])");
    const side = 120;
    const lonely = 40;
    const customEdges = [];
    for (const base of [0, side]) {
      for (let at = 0; at < side; at += 1) {
        customEdges.push({ from: base + at, to: base + ((at + 1) % side) },
          { from: base + at, to: base + ((at + 2) % side) });
      }
    }
    customEdges.push({ from: 0, to: side });
    const answer = window.__buildVaultGraph__({ path: "/scene/rim-hug", sources: false },
      { pages: side * 2 + lonely, ghosts: 0, tags: ["core", "reading"], customEdges });
    knowledgeLayouts.delete(view);
    const host = view.querySelector(".knowledge-nodes");
    host.dataset.knowledgeSignature = "";
    host.dataset.knowledgeVault = "";
    host.replaceChildren();
    view.querySelector(".knowledge-edges").replaceChildren();
    noteKnowledgeExploreLines({ [answer.vault]: JSON.stringify({ mode: "global" }) });
    setKnowledgeMode(view, "global", { paint: false });
    knowledgeReport = answer;
    await paintKnowledgeView();
    for (let wait = 0; wait < 1500 && (knowledgeLayouts.get(view)?.left ?? 1) > 0; wait += 1) await frame();
    const layout = knowledgeLayouts.get(view);
    const { communityHomeX: homeX, communityHomeY: homeY, communityHomeR: homeR, namedCount: named, tuning } = layout;
    const ids = homeR.length;
    let widest = 0;
    for (let rank = named; rank < ids; rank += 1) widest = Math.max(widest, homeR[rank]);
    const pitch = 2 * widest + tuning.clusterPitch;
    let firstExcess = 0;
    let closest = Number.POSITIVE_INFINITY;
    for (let rank = named; rank < ids; rank += 1) {
      const reach = Math.hypot(homeX[rank], homeY[rank]);
      const alongX = homeX[rank] / reach;
      const alongY = homeY[rank] / reach;
      let outline = 0;
      for (let disc = 0; disc < named; disc += 1) {
        outline = Math.max(outline, homeX[disc] * alongX + homeY[disc] * alongY + homeR[disc]);
        closest = Math.min(closest, Math.hypot(homeX[rank] - homeX[disc], homeY[rank] - homeY[disc])
          - homeR[rank] - homeR[disc]);
      }
      /* 첫 여덟 자리(황금각이 판을 한 바퀴 두른다) — 윤곽 밖으로 gap + 가장 큰 부스러기만큼 떨어진 곳이
       * 첫 줄이고, 나선은 거기서 한 걸음 안에서 자란다. */
      if (rank - named < 8) firstExcess = Math.max(firstExcess, reach - outline - tuning.strayGap - widest);
    }
    return { named, strays: ids - named, pitch: Math.round(pitch), gap: tuning.strayGap,
      firstExcess: Math.round(firstExcess), closest: Math.round(closest) };
  } catch (error) {
    return { thrown: String(error?.stack ?? error) };
  }
});
ok("the rim of stray pages hugs the named discs' outline, a spiral step at most, and keeps its gap from every disc",
  !rimHug.thrown && rimHug.named >= 2 && rimHug.strays >= 40
    && rimHug.firstExcess <= rimHug.pitch && rimHug.closest >= rimHug.gap,
  JSON.stringify(rimHug));

/* 이웃한 군집은 같은 색을 입지 않는다(t-11500) — 그리고 색은 가장 큰 여덟의 것이다(t-12029, 시안 v2
 * 「colour」, 한 함수). 색 칸은 여덟이고 주제는 스물 — 순위로 돌려 쓰던 색은 1·9·17위에게 같은 금색을
 * 입혀 이웃에 세웠다. 앞의 여덟은 순위의 칸을 하나씩 입고(그래서 어느 두 원반도, 이웃이든 아니든, 같은
 * 색이 아니다) 나머지는 색이 없는지 묻는다. 색은 여덟 칸의 토큰에서만 온다 — 두 테마의 대비는 그 토큰의
 * 시험(Test 8)이 이미 지킨다. */
const clusterInks = await glPage.evaluate(async () => {
  const frame = () => new Promise((done) => requestAnimationFrame(done));
  try {
    const view = document.querySelector(".knowledge-view:not([hidden])");
    const answer = window.__buildVaultGraph__({ path: "/scene/cluster-inks", sources: false },
      { pages: 600, ghosts: 30, linksPer: 3, tags: Array.from({ length: 20 }, (unused, at) => `topic-${at}`) });
    knowledgeLayouts.delete(view);
    const host = view.querySelector(".knowledge-nodes");
    host.dataset.knowledgeSignature = "";
    host.dataset.knowledgeVault = "";
    host.replaceChildren();
    view.querySelector(".knowledge-edges").replaceChildren();
    noteKnowledgeExploreLines({ [answer.vault]: JSON.stringify({ mode: "global" }) });
    setKnowledgeMode(view, "global", { paint: false });
    knowledgeReport = answer;
    await paintKnowledgeView();
    for (let wait = 0; wait < 1500 && (knowledgeLayouts.get(view)?.left ?? 1) > 0; wait += 1) await frame();
    const layout = knowledgeLayouts.get(view);
    const { communityHomeX: homeX, communityHomeY: homeY, communityHomeR: homeR, communityHue: hue,
      namedCount: named } = layout;
    let sameAsNearest = 0;
    let firstEightMoved = 0;
    let restColoured = 0;
    for (let rank = 0; rank < named; rank += 1) {
      let nearest = -1;
      let room = Number.POSITIVE_INFINITY;
      for (let other = 0; other < named; other += 1) {
        if (other === rank) continue;
        const gap = Math.hypot(homeX[rank] - homeX[other], homeY[rank] - homeY[other]) - homeR[rank] - homeR[other];
        if (gap < room) {
          room = gap;
          nearest = other;
        }
      }
      if (nearest >= 0 && hue[rank] >= 0 && hue[nearest] === hue[rank]) sameAsNearest += 1;
      if (rank < 8 && hue[rank] !== rank) firstEightMoved += 1;
      if (rank >= 8 && hue[rank] !== -1) restColoured += 1;
    }
    const cells = new Set([...hue.slice(0, named)].filter((one) => one >= 0));
    const shared = [...hue.slice(0, named)].filter((one, at, all) => one >= 0 && all.indexOf(one) !== at).length;
    return { named, sameAsNearest, firstEightMoved, restColoured, shared, cells: [...cells].sort() };
  } catch (error) {
    return { thrown: String(error?.stack ?? error) };
  }
});
ok("the eight largest clusters wear the eight hues, one each — so no disc shares its hue with its nearest named disc — and the rest wear none",
  !clusterInks.thrown && clusterInks.named > 8 && clusterInks.sameAsNearest === 0
    && clusterInks.firstEightMoved === 0 && clusterInks.cells.length === 8 && clusterInks.shared === 0
    && clusterInks.restColoured === 0,
  JSON.stringify(clusterInks));

/* 승인된 시안 v2의 전체 지도(t-12029)를 묻는 장면 — 주제마다 가지(삼진 나무 + 중심으로 가는 선 + 드문 고리)를
 * 두고, 주제 사이에는 `between`의 [왼쪽, 오른쪽, 가닥]만큼 선을 긋는다. 쪽의 태그가 주제의 낱말이라 군집의
 * 이름이 그 낱말이 되고, 뒤에 `lonely`개의 고아가 선다. 난수 없음. */
const topicVault = ({ sizes, between, lonely = 0, ghosts = 0 }) => {
  const words = ["alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta", "iota", "kappa",
    "lambda", "mu", "nu", "xi", "omicron", "pi", "rho", "sigma", "tau", "upsilon"];
  const starts = [];
  let pages = 0;
  for (const size of sizes) {
    starts.push(pages);
    pages += size;
  }
  const tags = [];
  const titles = [];
  sizes.forEach((size, topic) => {
    for (let at = 0; at < size; at += 1) {
      tags.push(words[topic]);
      titles.push(`${words[topic]} ${at}`);
    }
  });
  for (let at = 0; at < lonely; at += 1) {
    tags.push("loose");
    titles.push(`loose ${at}`);
  }
  const customEdges = [];
  sizes.forEach((size, topic) => {
    const base = starts[topic];
    for (let at = 1; at < size; at += 1) {
      customEdges.push({ from: base + at, to: base + Math.floor((at - 1) / 3) });
      if (at % 2 === 0) customEdges.push({ from: base + at, to: base });
      if (at % 4 === 0 && at >= 5) customEdges.push({ from: base + at, to: base + at - 5 });
    }
  });
  for (const [left, right, count] of between) {
    for (let k = 0; k < count; k += 1) {
      customEdges.push({ from: starts[left] + ((k * 7 + 3) % sizes[left]),
        to: starts[right] + ((k * 11 + 5) % sizes[right]) });
    }
  }
  const total = pages + lonely;
  for (let ghost = 0; ghost < ghosts; ghost += 1) {
    customEdges.push({ from: starts[ghost % sizes.length] + 1, to: total + ghost });
  }
  return { spec: { pages: total, ghosts, tags, titles, customEdges }, starts, words: words.slice(0, sizes.length) };
};
/* 한 볼트를 한 손으로 세운다 — 전체 지도, 아무것도 고르지 않은 판, 다 앉고 맞춘 뒤. */
await glPage.evaluate(() => {
  window.__standKnowledgeScene__ = async (path, spec, hand) => {
    const frame = () => new Promise((done) => requestAnimationFrame(done));
    const view = document.querySelector(".knowledge-view:not([hidden])");
    knowledgePainterKind = hand;
    knowledgeQuery = "";
    knowledgeSelectedKey = null;
    knowledgeClusterPicked = -1;
    knowledgePath = null;
    const answer = window.__buildVaultGraph__({ path, sources: false }, spec);
    knowledgeLayouts.delete(view);
    const host = view.querySelector(".knowledge-nodes");
    host.dataset.knowledgeSignature = "";
    host.dataset.knowledgeVault = "";
    host.replaceChildren();
    view.querySelector(".knowledge-edges").replaceChildren();
    noteKnowledgeExploreLines({ [answer.vault]: JSON.stringify({ mode: "global" }) });
    setKnowledgeMode(view, "global", { paint: false });
    knowledgeReport = answer;
    knowledgeAskedAt = Date.now();
    await paintKnowledgeView();
    for (let wait = 0; wait < 1500 && (knowledgeLayouts.get(view)?.left ?? 1) > 0; wait += 1) await frame();
    const layout = knowledgeLayouts.get(view);
    fitKnowledgeGraph(view, layout);
    await paintKnowledgeView();
    for (let wait = 0; wait < 3; wait += 1) await frame();
    return { view, layout };
  };
});

/* 열넷의 주제(40…10쪽) — 색 여덟과 회색 여섯을 세우는 볼트. 색(「colour」)과 한 줄 이름(「oneLine」)의 두 검사가 같이
 * 선다(t-12029). */
const fourteenTopics = topicVault({ sizes: [40, 36, 32, 30, 28, 26, 24, 22, 20, 18, 16, 14, 12, 10],
  between: [[0, 1, 3], [2, 3, 2], [4, 5, 2], [6, 7, 2], [8, 9, 1], [10, 11, 1], [12, 13, 1]] });

/* 군집 사이 선은 묶어서(t-12029, 승인된 시안 v2 「bundle」). 다섯 주제(30·24·20·16·12쪽)에 주제 사이 선
 * 12·9·3·2·1·1가닥과 고아 넷. 쉬는 전체 지도에서 두 손 모두: 그려진 선은 한 군집 안의 선뿐이고(군집을 건너는
 * 낱낱의 선은 가운데를 덮지 않는다), 묶음은 선이 8가닥 이상인 쌍에만 한 줄씩이며 굵기가 그 수를 말한다
 * (0.9 + √n × 0.55 px). 「강한 묶음」 목록은 선이 많은 순. 한 군집을 고르거나 그 이름판에 포인터를 올리면 그
 * 군집을 건너는 낱낱의 선이 돌아오고 그 군집에 닿는 묶음은 제 잉크를 입는다. 짚은 점의 선은 군집을 건너도
 * 선다. 주변 탐색에는 묶음이 없고 선은 그대로다. */
const bundleVault = topicVault({ sizes: [30, 24, 20, 16, 12],
  between: [[0, 1, 12], [0, 2, 9], [1, 2, 3], [2, 3, 2], [0, 3, 1], [3, 4, 1]], lonely: 4 });
const bundles = await glPage.evaluate(async ({ spec, starts }) => {
  const frame = () => new Promise((done) => requestAnimationFrame(done));
  const rows = [];
  try {
    for (const hand of ["svg", "gl"]) {
      const { view, layout } = await window.__standKnowledgeScene__("/scene/bundles", spec, hand);
      const { from, to, edgeCount } = layout.model;
      const community = layout.community;
      const topicRank = starts.map((first) => community[first]);
      /* 그려진 선 — SVG는 보이는 <path>, GL은 선 패스에 올린 끝점 쌍. 선 번호로 돌려 센다. */
      const drawnLines = () => {
        const painter = knowledgePainterFor(view);
        const seen = [];
        if (painter.id === "gl") {
          const ends = new Map();
          for (let at = 0; at < edgeCount; at += 1) ends.set(`${from[at]}>${to[at]}`, at);
          for (let seat = 0; seat < painter.counts.edges; seat += 1) {
            const at = ends.get(`${painter.edgeEnds[seat * 2]}>${painter.edgeEnds[seat * 2 + 1]}`);
            if (at !== undefined) seen.push(at);
          }
        } else {
          for (let at = 0; at < edgeCount; at += 1) {
            const line = layout.edgeEls[at];
            if (line && getComputedStyle(line).display !== "none") seen.push(at);
          }
        }
        return seen;
      };
      const inside = (at) => community[from[at]] === community[to[at]];
      const touches = (at, rank) => community[from[at]] === rank || community[to[at]] === rank;
      const intra = [];
      const pairs = new Map();
      for (let at = 0; at < edgeCount; at += 1) {
        if (layout.drawnEdge !== null && layout.drawnEdge[at] === 0) continue;
        if (inside(at)) {
          intra.push(at);
          continue;
        }
        const left = Math.min(community[from[at]], community[to[at]]);
        const right = Math.max(community[from[at]], community[to[at]]);
        if (right >= layout.namedCount) continue;
        pairs.set(`${left}|${right}`, (pairs.get(`${left}|${right}`) ?? 0) + 1);
      }
      const wanted = [...pairs].filter(([, count]) => count >= 8).map(([pair, count]) => `${pair}:${count}`).sort();
      const readTies = () => [...view.querySelectorAll(".knowledge-underlay .knowledge-tie")]
        .filter((line) => getComputedStyle(line).display !== "none")
        .map((line) => ({ at: Number(line.dataset.tie), width: Number(line.getAttribute("stroke-width")),
          mine: line.classList.contains("is-mine"), curved: /Q/u.test(line.getAttribute("d") ?? "") }));
      const rest = drawnLines();
      const tiesAtRest = readTies();
      const listed = [...view.querySelectorAll(".knowledge-overview-ties .knowledge-inspector-row")]
        .map((row) => ({ tie: Number(row.dataset.knowledgeTie), words: row.textContent }));
      /* 한 군집을 고른다 — 그 군집을 건너는 선이 돌아온다. */
      const picked = topicRank[0];
      toggleKnowledgeCluster(view, picked);
      await paintKnowledgeView();
      for (let wait = 0; wait < 3; wait += 1) await frame();
      const pickedLines = drawnLines();
      const tiesPicked = readTies();
      toggleKnowledgeCluster(view, picked);
      await paintKnowledgeView();
      /* 다른 군집의 이름판에 포인터를 올린다(포인터의 길 그대로) — 그 군집의 선이 돌아오고, 떠나면 물러선다. */
      const hovered = topicRank[1];
      const plate = layout.clusterEls[hovered].label;
      plate.dispatchEvent(new PointerEvent("pointermove", { bubbles: true, clientX: 1, clientY: 1 }));
      for (let wait = 0; wait < 3; wait += 1) await frame();
      const hoverLines = drawnLines();
      view.querySelector(".knowledge-canvas").dispatchEvent(new PointerEvent("pointerleave"));
      for (let wait = 0; wait < 3; wait += 1) await frame();
      const leftLines = drawnLines();
      /* 한 점을 짚는다 — 그 점의 선은 군집을 건너도 선다. */
      const bridgeEnd = starts[0] + 3;
      litKnowledge(view, layout, layout.model.keys[bridgeEnd]);
      for (let wait = 0; wait < 3; wait += 1) await frame();
      const litLines = drawnLines();
      const litWanted = [...layout.lit];
      litKnowledge(view, layout, null);
      /* 주변 탐색 — 묶음이 없고 선은 제 옷 그대로 선다(중심은 두 주제로 건너는 선을 든 쪽). */
      setKnowledgeMode(view, "local", { centre: layout.model.keys[bridgeEnd], paint: false });
      await paintKnowledgeView();
      for (let wait = 0; wait < 3; wait += 1) await frame();
      const localTies = readTies().length;
      const localBetween = drawnLines().filter((at) => !inside(at)).length;
      setKnowledgeMode(view, "global");
      knowledgeSelectedKey = null;
      await paintKnowledgeView();
      rows.push({
        hand: knowledgePainterFor(view).id,
        topics: new Set(topicRank).size, named: layout.namedCount,
        restDrawn: rest.length, restBetween: rest.filter((at) => !inside(at)).length, intra: intra.length,
        wanted, drawnTies: tiesAtRest.map((one) => {
          const tie = layout.ties[one.at];
          return `${tie.left}|${tie.right}:${tie.count}`;
        }).sort(),
        widthsOk: tiesAtRest.every((one) => Math.abs(one.width
          - (layout.tuning.tieWidth + Math.sqrt(layout.ties[one.at].count) * layout.tuning.tieGrow)) < 0.01),
        curved: tiesAtRest.every((one) => one.curved),
        least: layout.tuning.tieLeast, widthBase: layout.tuning.tieWidth, grow: layout.tuning.tieGrow,
        listed: listed.map((row) => layout.ties[row.tie]?.count ?? -1),
        listedWords: listed.every((row) => row.words.includes("↔")),
        pickedDrawn: pickedLines.length,
        pickedWanted: intra.length + (() => {
          let count = 0;
          for (let at = 0; at < edgeCount; at += 1) if (!inside(at) && touches(at, picked)) count += 1;
          return count;
        })(),
        pickedMine: tiesPicked.filter((one) => one.mine).map((one) => {
          const tie = layout.ties[one.at];
          return tie.left === picked || tie.right === picked;
        }),
        hoverDrawn: hoverLines.length,
        hoverWanted: intra.length + (() => {
          let count = 0;
          for (let at = 0; at < edgeCount; at += 1) if (!inside(at) && touches(at, hovered)) count += 1;
          return count;
        })(),
        leftDrawn: leftLines.length,
        litHasBetween: litWanted.filter((at) => !inside(at)).every((at) => litLines.includes(at))
          && litWanted.some((at) => !inside(at)),
        localTies, localBetween,
      });
    }
    return { rows };
  } catch (error) {
    return { thrown: String(error?.stack ?? error), rows };
  } finally {
    knowledgePainterKind = null;
  }
}, { spec: bundleVault.spec, starts: bundleVault.starts });
ok("the overview bundles the lines between clusters: at rest only a cluster's own lines stand, one tie per pair of eight lines or more speaks the count in its width, the strong ties list them, and picking or pointing at a cluster brings its lines back — on both hands",
  !bundles.thrown && bundles.rows.length === 2 && bundles.rows.every((row) => row.topics === 5 && row.named >= 5
    && row.least === 8 && row.widthBase === 0.9 && row.grow === 0.55
    && row.restBetween === 0 && row.restDrawn === row.intra
    && row.wanted.length === 2 && JSON.stringify(row.drawnTies) === JSON.stringify(row.wanted)
    && row.widthsOk && row.curved
    && row.listed.length === row.wanted.length && row.listed.every((count, at, all) => at === 0 || all[at - 1] >= count)
    && row.listedWords
    && row.pickedDrawn === row.pickedWanted && row.pickedMine.length >= 1 && row.pickedMine.every(Boolean)
    && row.hoverDrawn === row.hoverWanted && row.leftDrawn === row.intra
    && row.litHasBetween && row.localTies === 0 && row.localBetween > 0),
  JSON.stringify(bundles));

/* 묶음이 하나도 없는 지도는 선을 잃지 않는다(t-12029). 「쉬는 지도에서 군집을 건너는 낱선은 묶음이 말한다」는 시안의
 * 규칙은 수백 가닥이 가운데를 덮는 볼트의 것이다 — 8가닥에 닿는 쌍이 없는 작은 볼트(창 하네스의 12쪽 볼트: 선 25개
 * 중 11개)에서는 그 선을 대신 말할 줄이 없어 주제들이 끊긴 섬으로 보였다. 세 주제(12·9·6쪽)에 주제 사이 3·2·1가닥:
 * 두 손 모두 모든 선이 선다. 다리는 묶음이 없어도 곡선과 번호로 서고 그 한 가닥만 선 층에서 비킨다 — 곡선이 그
 * 선이다: 다리 장면(40·32·24·18·12쪽)에서 8가닥 쌍만 뺀 볼트는 묶음 0·다리 셋이고, 선 층에 없는 선은 그 셋뿐이다. */
const tielessVault = topicVault({ sizes: [12, 9, 6], between: [[0, 1, 3], [1, 2, 2], [0, 2, 1]], lonely: 2 });
const bridgeOnlyVault = topicVault({ sizes: [40, 32, 24, 18, 12],
  between: [[0, 2, 1], [1, 2, 1], [1, 3, 1], [2, 3, 2], [3, 4, 1]], lonely: 3 });
const tieless = await glPage.evaluate(async ({ small, lone }) => {
  const rows = [];
  try {
    for (const hand of ["svg", "gl"]) {
      for (const [path, spec] of [["/scene/tieless", small], ["/scene/bridge-only", lone]]) {
        const { view, layout } = await window.__standKnowledgeScene__(path, spec, hand);
        const { from, to, edgeCount } = layout.model;
        const painter = knowledgePainterFor(view);
        const seen = new Set();
        if (painter.id === "gl") {
          const ends = new Map();
          for (let at = 0; at < edgeCount; at += 1) ends.set(`${from[at]}>${to[at]}`, at);
          for (let seat = 0; seat < painter.counts.edges; seat += 1) {
            const at = ends.get(`${painter.edgeEnds[seat * 2]}>${painter.edgeEnds[seat * 2 + 1]}`);
            if (at !== undefined) seen.add(at);
          }
        } else {
          for (let at = 0; at < edgeCount; at += 1) {
            const line = layout.edgeEls[at];
            if (line && getComputedStyle(line).display !== "none") seen.add(at);
          }
        }
        let between = 0;
        const missing = [];
        for (let at = 0; at < edgeCount; at += 1) {
          if (layout.drawnEdge !== null && layout.drawnEdge[at] === 0) continue;
          if (layout.community[from[at]] !== layout.community[to[at]]) between += 1;
          if (!seen.has(at)) missing.push(at);
        }
        rows.push({ hand: painter.id, path, named: layout.namedCount, ties: layout.ties.length,
          tiesDrawn: layout.tiesDrawn, between, bridges: layout.bridges.map((bridge) => bridge.first),
          bridgesDrawn: layout.bridgesDrawn, missing });
      }
    }
    return { rows };
  } catch (error) {
    return { thrown: String(error?.stack ?? error), rows };
  } finally {
    knowledgePainterKind = null;
  }
}, { small: tielessVault.spec, lone: bridgeOnlyVault.spec });
ok("a map with no tie keeps every line between its topics, and a bridge still stands as its curve with only its one line stepping aside — on both hands",
  !tieless.thrown && tieless.rows.length === 4 && tieless.rows.every((row) => row.ties === 0 && row.tiesDrawn === 0
    && (row.path === "/scene/tieless"
      ? row.named >= 3 && row.between >= 6 && row.missing.length === 0 && row.bridges.length === 0
      : row.named >= 5 && row.between >= 6 && row.bridges.length === 3 && row.bridgesDrawn === 3
        && JSON.stringify([...row.missing].sort((a, b) => a - b))
          === JSON.stringify([...row.bridges].sort((a, b) => a - b)))),
  JSON.stringify(tieless));

/* 가지가 보이는 군집(t-12029, 승인된 시안 v2 「spread」). 다섯 주제(64·52·44·34·26쪽)에 유령 셋을 더한 볼트에서 두 손 모두: 전체
 * 지도의 쪽은 작은 점이다 — 잎 2.4 · 대표 지식 5.5 · 중심 10 px, 유령 2.2, 부스러기 2(두 손이 그린 크기로 잰다, SVG는
 * 점의 `r`, GL은 위치 텍스처의 반지름). 원반은 이완이 정한 반지름(√멤버 × pitch, 최소 반지름)의 1.3배이고 서로
 * 24 이상 떨어진다. 한 군집 안에서 화면의 몸이 서로 겹치는 점은 드물다(공이 풀렸다 — 기반은 점의 23%가 겹쳤다).
 * 주변 탐색의 점은 √차수 램프 그대로다. 겹침은 화면의 일이라 실측과 같은 넓은 판에서 잰다(캔버스 약 1065 × 937). */
const spreadVault = topicVault({ sizes: [64, 52, 44, 34, 26],
  between: [[0, 1, 12], [0, 2, 9], [1, 2, 3], [2, 3, 2], [0, 3, 1], [3, 4, 1]], lonely: 4, ghosts: 3 });
const spreadSeat = glPage.viewportSize();
await glPage.setViewportSize({ width: 1998, height: 1069 });
const spreads = await glPage.evaluate(async ({ spec, starts }) => {
  const frame = () => new Promise((done) => requestAnimationFrame(done));
  const rows = [];
  try {
    for (const hand of ["svg", "gl"]) {
      const { view, layout } = await window.__standKnowledgeScene__("/scene/spread", spec, hand);
      const painter = knowledgePainterFor(view);
      const { kinds } = layout.model;
      /* 그린 크기 — 두 손의 것을 따로 읽는다. */
      const drawnRadius = (at) => {
        if (painter.id === "gl") return painter.seatData[at * 4 + 3];
        const dot = layout.nodeEls[at]?.querySelector(".knowledge-dot");
        return dot ? Number(dot.getAttribute("r")) : -1;
      };
      const sizes = { leaf: new Set(), major: new Set(), core: new Set(), ghost: new Set(), stray: new Set() };
      for (let at = 0; at < layout.count; at += 1) {
        const size = Math.round(drawnRadius(at) * 100) / 100;
        const named = layout.community[at] < layout.namedCount;
        if (kinds[at] === "ghost") sizes.ghost.add(size);
        else if (!named) sizes.stray.add(size);
        else sizes[["leaf", "major", "core"][layout.tier[at]]].add(size);
      }
      /* 원반: 이완의 반지름 × 1.3, 그리고 서로의 틈. */
      const members = new Int32Array(layout.communityHomeR.length);
      for (let at = 0; at < layout.count; at += 1) members[layout.community[at]] += 1;
      const { tuning, communityHomeX: homeX, communityHomeY: homeY, communityHomeR: homeR, namedCount: named } = layout;
      let radiusMiss = 0;
      let closest = Number.POSITIVE_INFINITY;
      for (let rank = 0; rank < named; rank += 1) {
        const base = Math.max(tuning.clusterMinRadius, tuning.clusterPitch * Math.sqrt(Math.max(1, members[rank])));
        if (Math.abs(homeR[rank] - base * 1.3) > 0.01) radiusMiss += 1;
        for (let other = rank + 1; other < named; other += 1) {
          closest = Math.min(closest, Math.hypot(homeX[rank] - homeX[other], homeY[rank] - homeY[other])
            - homeR[rank] - homeR[other]);
        }
      }
      /* 한 군집 안에서 화면의 몸이 겹치는 점 — 그린 크기와 화면 좌표로. */
      let crowded = 0;
      let clustered = 0;
      const screen = (at) => [layout.project.screenX(at), layout.project.screenY(at)];
      for (let one = 0; one < layout.count; one += 1) {
        if (layout.community[one] >= named) continue;
        clustered += 1;
        const [ax, ay] = screen(one);
        for (let two = one + 1; two < layout.count; two += 1) {
          if (layout.community[two] !== layout.community[one]) continue;
          const [bx, by] = screen(two);
          if (Math.hypot(ax - bx, ay - by) < drawnRadius(one) + drawnRadius(two)) crowded += 1;
        }
      }
      /* 주변 탐색 — 고리의 점은 √차수 램프(`radiusRing`)의 크기다. */
      setKnowledgeMode(view, "local", { centre: layout.model.keys[starts[0]], paint: false });
      await paintKnowledgeView();
      for (let wait = 0; wait < 3; wait += 1) await frame();
      let ringMiss = 0;
      let ringDrawn = 0;
      for (let at = 0; at < layout.count; at += 1) {
        if (layout.drawn !== null && layout.drawn[at] === 0) continue;
        ringDrawn += 1;
        const want = knowledgeNodeRadius(layout.model.degree[at], tuning, layout.tier[at])
          * KNOWLEDGE_SHAPES[knowledgeShapeOf(kinds[at])].reach;
        const got = drawnRadius(at);
        /* 주변 탐색의 중심은 GL에서 제 배율만큼 크게 선다(SVG의 `transform: scale`). */
        if (at !== layout.ring?.seat && Math.abs(got - want) > 0.01) ringMiss += 1;
      }
      setKnowledgeMode(view, "global");
      await paintKnowledgeView();
      for (let wait = 0; wait < 3; wait += 1) await frame();
      const leafSeat = [...layout.model.keys.keys()].find((at) => kinds[at] === "page"
        && layout.community[at] < named && layout.tier[at] === 0);
      const backLeaf = Math.round(drawnRadius(leafSeat) * 100) / 100;
      rows.push({ hand: painter.id, named,
        sizes: Object.fromEntries(Object.entries(sizes).map(([key, held]) => [key, [...held].sort()])),
        radiusMiss, closest: Math.round(closest), gapLeast: tuning.clusterGapLeast,
        crowded, clustered, ringDrawn, ringMiss, backLeaf });
    }
    return { rows };
  } catch (error) {
    return { thrown: String(error?.stack ?? error), rows };
  } finally {
    knowledgePainterKind = null;
  }
}, { spec: spreadVault.spec, starts: spreadVault.starts });
await glPage.setViewportSize(spreadSeat);
{
  const one = (held, value) => held.length === 1 && held[0] === value;
  ok("the overview's clusters show their branches: small points (leaf 2.4, major 5.5, core 10, ghost 2.2, stray 2) in discs 1.3 times wider that keep their gap, few points covering each other, and the local exploration keeps its sizes — on both hands",
    !spreads.thrown && spreads.rows.length === 2 && spreads.rows.every((row) => row.named >= 5
      && one(row.sizes.leaf, 2.4) && one(row.sizes.major, 5.5) && one(row.sizes.core, 10)
      && one(row.sizes.ghost, 2.2) && one(row.sizes.stray, 2)
      && row.radiusMiss === 0 && row.closest >= row.gapLeast
      && row.crowded <= row.clustered * 0.05
      && row.ringDrawn > 1 && row.ringMiss === 0 && row.backLeaf === 2.4),
    JSON.stringify(spreads));
}

/* 색이 다시 주제를 가른다(t-12029, 승인된 시안 v2 「colour」). 열넷의 주제에서 두 손 모두: 가장 큰 여덟만 여덟 칸을
 * 하나씩 입고 나머지는 색이 없다. 잎은 제 군집의 색 그대로(안개색을 섞지 않는다), 색 없는 군집의 쪽은 안개색이다
 * (SVG의 칠을 픽셀로). 모든 이름 있는 군집이 원반을 갖고(색 없는 군집은 안개색 원반 — GL은 예전에 그 원반을 아예
 * 그리지 않았다), 쉬는 원반에는 테두리가 없으며 밝힌 군집의 원반에만 선다(두 손). 범례는 색 여덟과 「작은 주제
 * n」 한 줄. */
const colours = await glPage.evaluate(async ({ spec }) => {
  const frame = () => new Promise((done) => requestAnimationFrame(done));
  const paint = document.createElement("canvas").getContext("2d", { willReadFrequently: true });
  const rgb = (color) => {
    paint.clearRect(0, 0, 1, 1);
    paint.fillStyle = "#000";
    paint.fillStyle = color;
    paint.fillRect(0, 0, 1, 1);
    return [...paint.getImageData(0, 0, 1, 1).data.slice(0, 3)];
  };
  const near = (one, two) => one.every((value, at) => Math.abs(value - two[at]) <= 2);
  const rows = [];
  try {
    for (const hand of ["svg", "gl"]) {
      const { view, layout } = await window.__standKnowledgeScene__("/scene/colours", spec, hand);
      const painter = knowledgePainterFor(view);
      const { namedCount: named, communityHue: hue } = layout;
      const huesOk = [...hue.slice(0, named)].every((one, rank) => one === (rank < 8 ? rank : -1));
      /* 잎의 칠 — SVG의 계산된 칠을 토큰의 색과 픽셀로 견준다. */
      const root = getComputedStyle(view);
      let leafChecked = 0;
      let leafMiss = 0;
      if (hand === "svg") {
        for (let at = 0; at < layout.count; at += 1) {
          if (layout.model.kinds[at] !== "page" || layout.tier[at] !== 0 || layout.community[at] >= named) continue;
          const dot = layout.nodeEls[at]?.querySelector(".knowledge-dot");
          if (!dot) continue;
          const own = hue[layout.community[at]];
          const want = own >= 0 ? root.getPropertyValue(`--knowledge-hue-${own}`) : root.getPropertyValue("--ink-mist");
          leafChecked += 1;
          if (!near(rgb(getComputedStyle(dot).fill), rgb(want.trim()))) leafMiss += 1;
        }
      }
      /* 원반: 이름 있는 군집마다 하나, 쉬는 테두리 없음, 밝힌 군집에만 테두리. */
      const discsAt = () => {
        if (painter.id === "gl") {
          return { count: painter.counts.discs,
            rims: Array.from({ length: painter.counts.discs }, (unused, at) => painter.discRim[at * 2]) };
        }
        const discs = layout.clusterEls.map((held) => getComputedStyle(held.nebula));
        return { count: discs.filter((style) => style.display !== "none" && rgb(style.fill).some((one) => one > 0)
          && style.fill !== "none").length,
        rims: discs.map((style) => (style.stroke === "none" ? 0 : 1)) };
      };
      const rest = discsAt();
      const lit = named - 1;
      toggleKnowledgeCluster(view, lit);
      await paintKnowledgeView();
      for (let wait = 0; wait < 3; wait += 1) await frame();
      const picked = discsAt();
      toggleKnowledgeCluster(view, lit);
      await paintKnowledgeView();
      /* 범례. */
      const legend = view.querySelector(".knowledge-cluster-legend");
      const keys = [...legend.querySelectorAll("button.knowledge-cluster-key")].map((key) => Number(key.dataset.hue));
      const quiet = legend.querySelector(".knowledge-cluster-key.is-quiet")?.textContent ?? "";
      rows.push({ hand: painter.id, named, huesOk, leafChecked, leafMiss,
        discs: rest.count, restRims: rest.rims.filter((one) => one > 0).length,
        pickedRims: picked.rims.filter((one) => one > 0).length,
        keys, quiet, quietCount: named - 8 });
    }
    return { rows };
  } catch (error) {
    return { thrown: String(error?.stack ?? error), rows };
  } finally {
    knowledgePainterKind = null;
  }
}, { spec: fourteenTopics.spec });
ok("colour sorts the topics again: the eight largest clusters wear the eight hues and the rest are quiet grey, leaves wear their cluster's full ink, every cluster has a flat disc whose edge stands only when lit, and the legend says eight colours and the smaller topics — on both hands",
  !colours.thrown && colours.rows.length === 2 && colours.rows.every((row) => row.named > 8 && row.huesOk
    && row.discs === row.named && row.restRims === 0 && row.pickedRims === 1
    && JSON.stringify(row.keys) === JSON.stringify([0, 1, 2, 3, 4, 5, 6, 7])
    && row.quiet.includes(String(row.quietCount)))
    && colours.rows[0].leafChecked > 100 && colours.rows[0].leafMiss === 0,
  JSON.stringify(colours));

/* 군집 이름은 한 줄(t-12029, 승인된 시안 v2 「oneLine」). 같은 열넷의 주제(색 여덟 + 회색 여섯)에서 두 손 모두: 선
 * 이름판마다 보이는 글자는 이름 한 줄이고(대표 지식·쪽 수 줄이 없다), 색 있는 판은 제 색의 13 px, 회색 판은 안개색의
 * 11.5 px다. 쪽 수와 대표 지식은 이름에 올리면 나온다 — 판의 팁(`data-tip`)과 읽는 이의 이름이 그것을 말하고,
 * 키보드로 판에 서면 창의 팁이 곧바로 선다. 배율 1의 이름은 또렷하고 확대할수록 물러선다(배율 3에서 0.3 쪽으로). */
const oneLines = await glPage.evaluate(async ({ spec }) => {
  const frame = () => new Promise((done) => requestAnimationFrame(done));
  const paint = document.createElement("canvas").getContext("2d", { willReadFrequently: true });
  const rgb = (color) => {
    paint.clearRect(0, 0, 1, 1);
    paint.fillStyle = "#000";
    paint.fillStyle = color;
    paint.fillRect(0, 0, 1, 1);
    return [...paint.getImageData(0, 0, 1, 1).data.slice(0, 3)];
  };
  const near = (one, two) => one.every((value, at) => Math.abs(value - two[at]) <= 2);
  const rows = [];
  try {
    for (const hand of ["svg", "gl"]) {
      const { view, layout } = await window.__standKnowledgeScene__("/scene/one-line", spec, hand);
      const root = getComputedStyle(view);
      let standing = 0;
      let manyLines = 0;
      let styleMiss = 0;
      let tipMiss = 0;
      let quietStanding = 0;
      let firstPlate = null;
      for (let rank = 0; rank < layout.namedCount; rank += 1) {
        const label = layout.clusterEls[rank]?.label;
        if (!label || layout.clusterTally[rank] === 0 || label.classList.contains("is-folded")) continue;
        standing += 1;
        firstPlate ??= label;
        const texts = [...label.querySelectorAll("text")]
          .filter((one) => getComputedStyle(one).display !== "none" && one.getBoundingClientRect().width > 0);
        if (texts.length !== 1) manyLines += 1;
        const name = label.querySelector(".knowledge-cluster-name");
        const style = getComputedStyle(name);
        const hue = layout.communityHue[rank];
        if (hue < 0) quietStanding += 1;
        const wantPx = hue >= 0 ? 13 : 11.5;
        const wantInk = hue >= 0 ? root.getPropertyValue(`--knowledge-hue-${hue}`) : root.getPropertyValue("--ink-mist");
        if (Math.abs(Number.parseFloat(style.fontSize) - wantPx) > 0.01 || !near(rgb(style.fill), rgb(wantInk.trim()))) {
          styleMiss += 1;
        }
        const core = layout.communityCore[rank];
        const tip = label.dataset.tip ?? "";
        if (!tip.includes(String(layout.communitySize[rank])) || (core >= 0 && !tip.includes(layout.model.titles[core]))
          || !(label.getAttribute("aria-label") ?? "").includes(tip)) tipMiss += 1;
      }
      /* 키보드로 판에 선다 — 창의 팁이 곧바로 그 말을 한다. */
      firstPlate.focus();
      await frame();
      const tipNode = document.querySelector('.tooltip[role="tooltip"]');
      const focusTip = { shown: tipNode !== null && !tipNode.hidden, words: tipNode?.textContent ?? "",
        want: firstPlate.dataset.tip };
      firstPlate.blur();
      /* 확대하면 이름이 물러선다. */
      const nameOf = () => firstPlate.querySelector(".knowledge-cluster-name");
      const fitOpacity = Number(getComputedStyle(nameOf()).opacity);
      takeKnowledgeZoom(view, layout, 3);
      await paintKnowledgeView();
      for (let wait = 0; wait < 3; wait += 1) await frame();
      const zoomedOpacity = Number(getComputedStyle(nameOf()).opacity);
      fitKnowledgeGraph(view, layout);
      await paintKnowledgeView();
      rows.push({ hand: knowledgePainterFor(view).id, standing, quietStanding, manyLines, styleMiss, tipMiss,
        focusTip, fitOpacity, zoomedOpacity });
    }
    return { rows };
  } catch (error) {
    return { thrown: String(error?.stack ?? error), rows };
  } finally {
    knowledgePainterKind = null;
  }
}, { spec: fourteenTopics.spec });
ok("a cluster's name is one line: every standing plate draws only its name, coloured in its ink at 13 px or quiet mist at 11.5 px, its pages and main page are in its tip and accessible name, the tip stands when the keyboard reaches it, and the names step back as the map zooms in — on both hands",
  !oneLines.thrown && oneLines.rows.length === 2 && oneLines.rows.every((row) => row.standing > 8
    && row.quietStanding > 0 && row.manyLines === 0 && row.styleMiss === 0 && row.tipMiss === 0
    && row.focusTip.shown && row.focusTip.words === row.focusTip.want
    && row.fitOpacity > 0.9 && row.zoomedOpacity < 0.5),
  JSON.stringify(oneLines));

/* 놀라운 연결(t-12029, 승인된 시안 v2 「bridges」). 다섯 주제(40·32·24·18·12쪽)에서 한 가닥으로만 이어진 쌍은
 * 0–2·1–2·1–3(다리), 두 가닥인 2–3과 작은 쪽이 15쪽이 못 되는 3–4는 다리가 아니다. 두 손 모두: 다리는 작은 쪽이 큰
 * 순서로 그 셋이고, 두 끝 군집의 색으로 이은 곡선(그라데이션의 두 멈춤이 두 색)과 1부터의 번호 원이며, 번호 원
 * 위에는 어느 이름도 서지 않는다. 「놀라운 연결」 목록은 같은 순서·같은 번호에 두 끝 쪽의 제목을 팁으로 들고,
 * 줄을 짚으면 그 다리만 굵어지고 나머지는 물러서며, 누르면 그 한 가닥의 앞 쪽이 골라진다. 주변 탐색에는 다리가
 * 없다. */
const bridgeVault = topicVault({ sizes: [40, 32, 24, 18, 12],
  between: [[0, 1, 12], [0, 2, 1], [1, 2, 1], [1, 3, 1], [2, 3, 2], [3, 4, 1]], lonely: 3 });
const bridgeScene = await glPage.evaluate(async ({ spec, starts }) => {
  const frame = () => new Promise((done) => requestAnimationFrame(done));
  const paint = document.createElement("canvas").getContext("2d", { willReadFrequently: true });
  const rgb = (color) => {
    paint.clearRect(0, 0, 1, 1);
    paint.fillStyle = "#000";
    paint.fillStyle = color;
    paint.fillRect(0, 0, 1, 1);
    return [...paint.getImageData(0, 0, 1, 1).data.slice(0, 3)];
  };
  const near = (one, two) => one.every((value, at) => Math.abs(value - two[at]) <= 2);
  const hit = (one, two) => one.left < two.right && two.left < one.right && one.top < two.bottom && two.top < one.bottom;
  const rows = [];
  try {
    for (const hand of ["svg", "gl"]) {
      const { view, layout } = await window.__standKnowledgeScene__("/scene/bridges", spec, hand);
      const { from, to, edgeCount, keys, titles } = layout.model;
      const { community, communitySize: size, namedCount: named } = layout;
      const topicRank = starts.map((first) => community[first]);
      /* 기대: 그려지는 선에서 이름 있는 쌍마다 세어, 한 가닥이고 작은 쪽이 15쪽 이상인 쌍. */
      const pairs = new Map();
      for (let at = 0; at < edgeCount; at += 1) {
        const left = Math.min(community[from[at]], community[to[at]]);
        const right = Math.max(community[from[at]], community[to[at]]);
        if (left === right || right >= named) continue;
        const held = pairs.get(`${left}|${right}`) ?? { left, right, count: 0 };
        held.count += 1;
        pairs.set(`${left}|${right}`, held);
      }
      const wanted = [...pairs.values()].filter((pair) => pair.count === 1 && Math.min(size[pair.left], size[pair.right]) >= 15)
        .sort((one, two) => Math.min(size[two.left], size[two.right]) - Math.min(size[one.left], size[one.right])
          || Math.max(size[two.left], size[two.right]) - Math.max(size[one.left], size[one.right]))
        .map((pair) => `${pair.left}|${pair.right}`);
      const got = layout.bridges.map((bridge) => `${bridge.left}|${bridge.right}`);
      const root = getComputedStyle(view);
      const inkOf = (rank) => rgb((layout.communityHue[rank] >= 0 ? root.getPropertyValue(`--knowledge-hue-${layout.communityHue[rank]}`)
        : root.getPropertyValue("--ink-mist")).trim());
      const groups = [...view.querySelectorAll(".knowledge-underlay .knowledge-bridges > g")];
      let inkMiss = 0;
      const numbers = [];
      groups.forEach((group, at) => {
        const bridge = layout.bridges[at];
        const line = group.querySelector(".knowledge-bridge");
        const id = /url\(["']?#([^"')]+)/u.exec(line.getAttribute("stroke") ?? "")?.[1];
        const stops = [...(view.querySelector(`#${id}`)?.querySelectorAll("stop") ?? [])]
          .map((stop) => rgb(getComputedStyle(stop).stopColor));
        const head = from[bridge.first];
        const tail = to[bridge.first];
        if (stops.length !== 2 || !near(stops[0], inkOf(community[head])) || !near(stops[1], inkOf(community[tail]))) {
          inkMiss += 1;
        }
        numbers.push(group.querySelector(".knowledge-bridge-badge text").textContent);
      });
      /* 번호 원 위의 이름 — 주제의 이름판과 쪽의 이름표 둘 다. */
      const badges = groups.map((group) => group.querySelector(".knowledge-bridge-badge circle").getBoundingClientRect());
      const words = [...view.querySelectorAll(".knowledge-cluster-label:not(.is-folded) text, .knowledge-node.is-named .knowledge-label, .knowledge-gl-label:not([hidden])")]
        .map((word) => word.getBoundingClientRect()).filter((box) => box.width > 0);
      const covered = badges.filter((badge) => words.some((word) => hit(badge, word))).length;
      /* 목록. */
      const listRows = [...view.querySelectorAll(".knowledge-overview-bridges .knowledge-inspector-row")];
      const listed = listRows.map((row) => row.parentElement.querySelector(".knowledge-inspector-note").textContent);
      const tipsOk = listRows.every((row, at) => {
        const bridge = layout.bridges[at];
        return row.dataset.tip.includes(titles[from[bridge.first]]) && row.dataset.tip.includes(titles[to[bridge.first]]);
      });
      /* 둘째 줄을 짚는다 — 그 다리만 굵고 나머지는 물러선다. */
      let hot = null;
      if (listRows.length > 1) {
        listRows[1].dispatchEvent(new PointerEvent("pointerover", { bubbles: true }));
        await frame();
        hot = groups.map((group) => ({ width: Number(group.querySelector(".knowledge-bridge").getAttribute("stroke-width")),
          opacity: Number(group.getAttribute("opacity")) }));
        view.querySelector(".knowledge-inspector").dispatchEvent(new PointerEvent("pointerleave"));
        await frame();
      }
      /* 첫 줄을 누른다 — 그 한 가닥의 앞 쪽이 골라진다. */
      listRows[0]?.click();
      await frame();
      const picked = knowledgeSelectedKey === keys[from[layout.bridges[0].first]];
      knowledgeSelectedKey = null;
      await paintKnowledgeView();
      /* 주변 탐색 — 다리가 없다. */
      setKnowledgeMode(view, "local", { centre: keys[starts[0]], paint: false });
      await paintKnowledgeView();
      for (let wait = 0; wait < 3; wait += 1) await frame();
      const localBridges = view.querySelectorAll(".knowledge-underlay .knowledge-bridges > g").length;
      setKnowledgeMode(view, "global");
      knowledgeSelectedKey = null;
      await paintKnowledgeView();
      rows.push({ hand: knowledgePainterFor(view).id, topics: new Set(topicRank).size, wanted, got, drawn: groups.length,
        inkMiss, numbers, covered, listed, tipsOk, hot, picked, localBridges,
        least: layout.tuning.bridgeLeast });
    }
    return { rows };
  } catch (error) {
    return { thrown: String(error?.stack ?? error), rows };
  } finally {
    knowledgePainterKind = null;
  }
}, { spec: bridgeVault.spec, starts: bridgeVault.starts });
ok("surprising links stand out: the single lines between two large topics are curves in both clusters' inks with numbers no name covers, listed in the same order with their pages in a tip; pointing at a row thickens that bridge and quiets the rest, pressing it picks the line's first page — on both hands",
  !bridgeScene.thrown && bridgeScene.rows.length === 2 && bridgeScene.rows.every((row) => row.topics === 5
    && row.least === 15 && row.wanted.length === 3 && JSON.stringify(row.got) === JSON.stringify(row.wanted)
    && row.drawn === 3 && row.inkMiss === 0 && JSON.stringify(row.numbers) === JSON.stringify(["1", "2", "3"])
    && row.covered === 0 && JSON.stringify(row.listed) === JSON.stringify(["1", "2", "3"]) && row.tipsOk
    && row.hot !== null && row.hot[1].width === 2.6 && row.hot[1].opacity === 1
    && row.hot[0].opacity === 0.3 && row.hot[2].opacity === 0.3
    && row.picked && row.localBridges === 0),
  JSON.stringify(bridgeScene));

/* 지도가 판을 채운다(t-12029, 승인된 시안 v2 「fit」 마무리). 열넷의 주제에 고아 스물을 더한 볼트를 넓은 판에
 * 세우고 두 손 모두: 그림의 경계는 묶는 축에서 판의 `mapFit`(0.9) 몫을 차지하고(세로는 아래 범례 띠 `mapFoot`을
 * 뺀 높이의 몫, 다른 축은 그 몫을 넘지 않는다), 원반들이 판의 60% 넘게 걸친다(기반: 72%의 몫과 원반 사이 틈만큼
 * 떨어진 부스러기 띠 때문에 원반이 판의 가운데 작게 섰다). 어떤 원반도 아래의 범례·배율 단추와 겹치지 않고, 부스러기
 * 띠는 모든 원반과 띠의 틈 이상 떨어진다. */
const fitVault = topicVault({ sizes: [40, 36, 32, 30, 28, 26, 24, 22, 20, 18, 16, 14, 12, 10],
  between: [[0, 1, 3], [2, 3, 2], [4, 5, 2], [6, 7, 2], [8, 9, 1], [10, 11, 1], [12, 13, 1]], lonely: 20 });
const fitSeat = glPage.viewportSize();
await glPage.setViewportSize({ width: 1998, height: 1069 });
const fits = await glPage.evaluate(async ({ spec }) => {
  const rows = [];
  try {
    for (const hand of ["svg", "gl"]) {
      const { view, layout } = await window.__standKnowledgeScene__("/scene/fit", spec, hand);
      const { tuning, bounds, scale } = layout;
      const canvas = view.querySelector(".knowledge-canvas").getBoundingClientRect();
      const foot = Math.min(tuning.mapFoot, canvas.height * (1 - tuning.mapFit));
      const camera = layout.viewBoxRect;
      const shareX = ((bounds.maxX - bounds.minX) * scale) / canvas.width;
      const shareY = ((bounds.maxY - bounds.minY) * camera.yScale * scale) / (canvas.height - foot);
      /* 원반들이 판에 걸친 몫 — 화면의 원(눌린 판에서는 타원)의 합집합 상자. */
      const discs = { left: Infinity, right: -Infinity, top: Infinity, bottom: -Infinity };
      const circles = [];
      for (let rank = 0; rank < layout.namedCount; rank += 1) {
        if (layout.clusterTally[rank] === 0) continue;
        const reach = layout.clusterReach[rank] * scale;
        const x = canvas.left + (layout.clusterX[rank] - camera.x) * scale;
        const y = canvas.top + (camera.middleY + (layout.clusterY[rank] - camera.middleY) * camera.yScale - camera.y) * scale;
        circles.push({ x, y, rx: reach, ry: reach * camera.yScale });
        discs.left = Math.min(discs.left, x - reach);
        discs.right = Math.max(discs.right, x + reach);
        discs.top = Math.min(discs.top, y - reach * camera.yScale);
        discs.bottom = Math.max(discs.bottom, y + reach * camera.yScale);
      }
      const spanX = (discs.right - discs.left) / canvas.width;
      const spanY = (discs.bottom - discs.top) / canvas.height;
      /* 아래의 조작부와 원반 — 상자 대 타원의 가장 가까운 점. */
      let underControls = 0;
      for (const control of view.querySelectorAll(".knowledge-cluster-legend, .knowledge-zoom")) {
        const box = control.getBoundingClientRect();
        if (box.width === 0 || control.hidden) continue;
        for (const disc of circles) {
          const nearX = Math.min(Math.max(disc.x, box.left), box.right);
          const nearY = Math.min(Math.max(disc.y, box.top), box.bottom);
          if (((nearX - disc.x) / disc.rx) ** 2 + ((nearY - disc.y) / disc.ry) ** 2 < 1) underControls += 1;
        }
      }
      /* 부스러기 띠와 원반의 틈(그림 단위). */
      const { communityHomeX: homeX, communityHomeY: homeY, communityHomeR: homeR, namedCount: named } = layout;
      let strayClosest = Infinity;
      for (let rank = named; rank < homeR.length; rank += 1) {
        for (let disc = 0; disc < named; disc += 1) {
          strayClosest = Math.min(strayClosest, Math.hypot(homeX[rank] - homeX[disc], homeY[rank] - homeY[disc])
            - homeR[rank] - homeR[disc]);
        }
      }
      rows.push({ hand: knowledgePainterFor(view).id, mapFit: tuning.mapFit, foot,
        shareX: Math.round(shareX * 1000) / 1000, shareY: Math.round(shareY * 1000) / 1000,
        spanX: Math.round(spanX * 100), spanY: Math.round(spanY * 100), underControls,
        strayClosest: Math.round(strayClosest), strayGap: tuning.strayGap });
    }
    return { rows };
  } catch (error) {
    return { thrown: String(error?.stack ?? error), rows };
  } finally {
    knowledgePainterKind = null;
  }
}, { spec: fitVault.spec });
await glPage.setViewportSize(fitSeat);
ok("the map fills the canvas: its bounds take 0.9 of the binding axis above the legend's band, the discs span over 60% of the canvas, no disc lies under the legend or the zoom buttons, and the stray band keeps its gap — on both hands",
  !fits.thrown && fits.rows.length === 2 && fits.rows.every((row) => row.mapFit === 0.9
    && Math.abs(Math.max(row.shareX, row.shareY) - row.mapFit) < 0.02 && row.shareX <= row.mapFit + 0.02
    && row.shareY <= row.mapFit + 0.02 && Math.max(row.spanX, row.spanY) > 60 && row.underControls === 0
    && row.strayClosest >= row.strayGap),
  JSON.stringify(fits));

/* P1 G3 — 두 손이 같은 모양을 그리는가, 픽셀로.
 *
 * 다섯 종류의 점 하나씩을 선 없이 한 줄로 세우고, 같은 자리·같은 크기를 SVG 손과 GL 손으로
 * 한 번씩 찍는다. 점마다 칠해진 픽셀(배경에서 벗어난 픽셀)의 가면을 떠서 셋을 묻는다:
 *   (1) 가면이 표의 모양인가 — 같은 넓이의 네 모양(원·사각·마름모·삼각)의 이상적인 가면 중
 *       가장 많이 겹치는(IoU) 것이 표의 모양이고, 그 겹침이 하한을 넘는다. 고리는 속이 비고
 *       둘레에 점선이 선다.
 *   (2) 두 손의 가면이 서로 겹친다.
 *   (3) 사람의 말로 적은 자리: 사각형 모서리 안쪽 점은 칠해지고 같은 크기의 원에서는 비었다,
 *       삼각형은 위를 가리킨다(꼭짓점 쪽은 칠해지고 밑변 아래는 비었다), 마름모는 꼭짓점 쪽이
 *       칠해지고 모서리 쪽이 비었다.
 * 점의 크기는 크기 토큰을 판에 덮어 키운다 — 3 px 점의 모서리는 픽셀 두셋이라 모양을 가르지
 * 못한다. GL이 서지 않는 판이면 이유를 적고 건너뛴다(SKIP 줄 — 조용히 초록이 되지 않게). */
const SHAPE_PIXELS = Object.freeze({
  /* 크기가 약속한 반지름(px). 삼각형의 외접원이 56 px이 되고, 아래 판정 자리 중 가장 좁은
   * 여유(사각 모서리 안쪽 점 3.7 px)가 테두리(≤ 1.5 px)와 가장자리 부드러움(1 px)보다 넓다. */
  radius: 36,
  /* 그 반지름을 입히는 크기 토큰 — √차수 램프의 넷과, 전체 지도에서 쪽과 유령이 입는 지도 크기
   * 다섯(t-12029). 지도 크기를 빼면 쪽은 2 px·유령은 2.2 px로 서서 모양을 가르지 못한다. */
  sizeTokens: Object.freeze(["--knowledge-node-radius-min", "--knowledge-node-radius-max",
    "--knowledge-core-radius", "--knowledge-major-radius", "--knowledge-map-leaf", "--knowledge-map-major",
    "--knowledge-map-core", "--knowledge-map-ghost", "--knowledge-map-stray"]),
  /* 점 사이(그래프 단위) — 폭에 맞춘 카메라에서 이웃 외접원이 닿지 않는지는 아래에서 묻는다. */
  pitch: 100,
  /* 점의 상자를 외접원 밖으로 넓히는 폭(px) — 테두리와 부드러움이 상자 안에 들게. */
  pad: 6,
  /* 칠해졌는가: 배경에서 벗어난 폭이 점의 잉크가 벗어난 폭의 절반을 넘는다. */
  inkShare: 0.5,
  /* 겹침의 하한 0.9: 같은 넓이의 서로 다른 모양끼리는 원·마름모 0.834, 같은 모양을 1 px 안쪽으로
   * 줄인 것과는 최소 0.938(사각)이다 — 둘 사이에 선다. 정육각형과 원(t-5970)만은 같은 넓이에서
   * 0.928로 이 선을 넘는다: 그 둘은 「가장 잘 맞는 모양이 제 모양인가」(`best`)가 가른다. */
  overlap: 0.9,
  /* 고리의 잉크를 찾는 둘레 띠(px) — 유령의 테두리 굵기(1.5 px)만큼 안팎으로. */
  ringBand: 1.5,
  /* 둘레를 짚는 간격(호의 px)과, 짚는 반지름 셋(외접원에서 안·가운데·밖 반 픽셀) — 테두리가 픽셀
   * 격자에 걸친 자리에서도 칠해진 점을 놓치지 않게. */
  ringStep: 0.5,
  ringProbe: Object.freeze([-0.5, 0, 0.5]),
  /* 점선 조각의 수는 둘레 ÷ 점선의 주기(토큰)이고, 두 손 다 그 수의 20% 안이어야 한다 —
   * 끊기지 않은 고리는 조각 0~1, 부서진 고리는 조각이 넘친다. 칠해진 몫(점선의 반)은 두 손이
   * 0.15 안에서 같아야 한다(끝이 부드러운 SVG의 점선과 끝이 곧은 GL의 점선). */
  dashSlack: 0.2,
  dutyGap: 0.15,
  /* 채운 모양의 한가운데 색은 두 손에서 같다 — 곧은 잉크를 8비트로 반올림하는 차만큼(실측 ≤ 1)
   * 여유를 두고. 반투명 잉크(원본 34%)에 알파를 두 번 곱하던 GL은 한가운데가 (57,69,70)이 아니라
   * (26,32,34)였다(09-17). */
  centreLevels: 3,
  /* 고리 테두리의 짙기(GL ÷ SVG) 하한. GL의 부드러운 가장자리(1 px)는 1.5 px 테두리의 한가운데서도
   * 잉크가 1 − smoothstep(−0.25, 1.75, 0) = 0.957에서 멈추고, 테두리를 투명한 몸과 섞던 GL은
   * 0.914였다(09-17 실측) — 둘 사이에 선다. */
  ringPeak: 0.93,
});
/* 같은 넓이의 네 모양의 외접원 비율 — 제품의 표가 아니라 넓이의 식에서(시험이 제품의 수로
 * 제품을 재지 않게). 점의 상자는 가장 멀리 닿는 후보까지 담는다. */
const IDEAL_REACH = Object.freeze({ circle: 1, square: Math.sqrt(Math.PI / 2), diamond: Math.sqrt(Math.PI / 2),
  triangle: Math.sqrt((4 * Math.PI) / (3 * Math.sqrt(3))),
  hexagon: Math.sqrt((2 * Math.PI) / (3 * Math.sqrt(3))), cross: Math.sqrt(Math.PI / 2) });

/* 한 판에서 두 손을 찍어 견준다 — 판의 기기 픽셀 비율이 무엇이든. 찍은 그림은 기기 픽셀이고,
 * 모양의 판정은 CSS 픽셀로 한다(점의 크기는 CSS 픽셀의 약속이다). */
const shapePixelsOn = async (target) => {
  /* 움직임을 줄이는 판에서 세운다 — 도착의 페이드와 옷의 전이가 찍는 픽셀에 끼지 않게. */
  await target.emulateMedia({ reducedMotion: "reduce" });
  const scene = await target.evaluate(async (tune) => {
    try {
      if (!knowledgeGlSupported()) return { skip: "this browser has no WebGL2 context" };
      const view = document.querySelector(".knowledge-view:not([hidden])");
      const frame = () => new Promise((done) => requestAnimationFrame(done));
      for (const name of tune.sizeTokens) view.style.setProperty(name, String(tune.radius));
      knowledgeTunings.delete(view);
      const spec = { pages: 1, ghosts: 1, tags: [], customEdges: [],
        supply: { components: 1, vulnerabilities: 1 }, code: { files: 1, symbols: 1 } };
      window.__VAULT__ = spec;
      knowledgeShowSources = true;
      knowledgeShowCode = true;
      knowledgeQuery = "";
      knowledgeTagsPicked.clear();
      knowledgeSelectedKey = null;
      knowledgeClusterPicked = -1;
      knowledgeEntryPending = false;
      knowledgeRevealKey = null;
      knowledgeRevealMode = null;
      knowledgeMode = "global";
      setKnowledgeMode(view, "global", { paint: false });
      knowledgeLayouts.delete(view);
      const host = view.querySelector(".knowledge-nodes");
      host.dataset.knowledgeSignature = "";
      host.dataset.knowledgeVault = "";
      host.replaceChildren();
      view.querySelector(".knowledge-edges").replaceChildren();
      /* 늦게 온 백엔드의 답이 이 그림을 덮지 않게 — 바닥 시간 안의 청은 버려진다. */
      knowledgeAskedAt = Date.now();
      knowledgeReport = window.__buildVaultGraph__({ path: "/shapes", sources: true, code: true,
        project: "/workspace/acme" }, spec);
      knowledgePainterKind = "svg";
      await paintKnowledgeView();
      for (let round = 0; round < 600 && (knowledgeLayouts.get(view)?.left ?? 1) > 0; round += 1) {
        await frame();
      }
      const style = document.createElement("style");
      style.id = "knowledge-shape-pixels";
      /* 이름표는 모양이 아니다 — 두 손이 이름을 다른 요소로 세우므로 가린다. */
      style.textContent = ".knowledge-view .knowledge-label, .knowledge-view .knowledge-gl-label"
        + " { visibility: hidden !important; }";
      document.head.appendChild(style);
      return { ratio: window.devicePixelRatio };
    } catch (error) {
      return { thrown: String(error?.stack ?? error) };
    }
  }, SHAPE_PIXELS);
  if (scene.skip) return scene;
  /* 한 손으로 그리고, 점마다의 자리(판 좌표)와 크기를 돌려준다. 자리는 두 손에서 같아야 한다. */
  const seatShapes = (hand) => target.evaluate(async ({ next, tune }) => {
    const view = document.querySelector(".knowledge-view:not([hidden])");
    const frame = () => new Promise((done) => requestAnimationFrame(done));
    knowledgePainterKind = next;
    await paintKnowledgeView();
    const layout = knowledgeLayouts.get(view);
    layout.left = 0;
    const kinds = Object.keys(KNOWLEDGE_NODE_SHAPES);
    const seats = kinds.map((kind) => layout.model.kinds.indexOf(kind));
    seats.forEach((seat, at) => {
      layout.x[seat] = at * tune.pitch;
      layout.y[seat] = 0;
    });
    knowledgeBounds(layout);
    fitKnowledgeGraph(view, layout);
    await frame();
    await Promise.all(view.getAnimations({ subtree: true }).map((one) => one.finished.catch(() => {})));
    await frame();
    const canvas = view.querySelector(".knowledge-canvas");
    const box = canvas.getBoundingClientRect();
    return {
      hand: knowledgePainterFor(view).id,
      elements: view.querySelectorAll(".knowledge-nodes .knowledge-node").length,
      nodes: kinds.map((kind, at) => {
        const seat = seats[at];
        const x = box.left + layout.project.screenX(seat);
        const y = box.top + layout.project.screenY(seat);
        const under = document.elementFromPoint(x, y);
        return { kind, shape: KNOWLEDGE_NODE_SHAPES[kind], x, y, reach: layout.radius[seat],
          promised: knowledgeNodeRadius(layout.model.degree[seat], layout.tuning, layout.tier[seat]),
          /* 점의 한가운데를 다른 판(인스펙터·범례)이 덮고 있지 않은가. */
          open: under !== null && canvas.contains(under) };
      }),
    };
  }, { next: hand, tune: SHAPE_PIXELS });
  const seatWas = target.viewportSize();
  await target.setViewportSize({ width: 1998, height: 1069 });
  const drawn = {};
  const shots = {};
  const boxHalf = (node) => Math.ceil(node.promised * Math.max(...Object.values(IDEAL_REACH)) + SHAPE_PIXELS.pad);
  if (!scene.thrown) {
    for (const hand of ["svg", "gl"]) {
      drawn[hand] = await seatShapes(hand);
      const nodes = drawn[hand].nodes;
      const left = Math.floor(Math.min(...nodes.map((node) => node.x - boxHalf(node))));
      const top = Math.floor(Math.min(...nodes.map((node) => node.y - boxHalf(node))));
      const clip = { x: left, y: top,
        width: Math.ceil(Math.max(...nodes.map((node) => node.x + boxHalf(node)))) - left,
        height: Math.ceil(Math.max(...nodes.map((node) => node.y + boxHalf(node)))) - top };
      shots[hand] = { clip, png: (await target.screenshot({ clip, type: "png" })).toString("base64") };
    }
  }
  const pixels = scene.thrown ? { thrown: scene.thrown } : await target.evaluate(async ({ drawn, shots, tune, ideal }) => {
    try {
      const decode = async ({ png, clip }) => {
        const bytes = Uint8Array.from(atob(png), (glyph) => glyph.charCodeAt(0));
        const bitmap = await createImageBitmap(new Blob([bytes], { type: "image/png" }),
          { colorSpaceConversion: "none", premultiplyAlpha: "none" });
        const surface = new OffscreenCanvas(bitmap.width, bitmap.height);
        const brush = surface.getContext("2d", { willReadFrequently: true });
        brush.drawImage(bitmap, 0, 0);
        /* 찍은 그림의 기기 픽셀 ÷ CSS 픽셀 — 판의 비율 그대로. */
        return { clip, wide: bitmap.width, tall: bitmap.height, scale: bitmap.width / clip.width,
          data: brush.getImageData(0, 0, bitmap.width, bitmap.height).data };
      };
      const ROOT_HALF = Math.SQRT1_2;
      const ROOT3 = Math.sqrt(3);
      /* 외접원 반지름 1에 내접한 모양 — 제품의 기하가 아니라 기하의 정의로 적는다. */
      const inside = {
        circle: (x, y) => x * x + y * y <= 1,
        square: (x, y) => Math.abs(x) <= ROOT_HALF && Math.abs(y) <= ROOT_HALF,
        diamond: (x, y) => Math.abs(x) + Math.abs(y) <= 1,
        triangle: (x, y) => y <= 0.5 && y >= -1 + ROOT3 * Math.abs(x),
        /* 위아래가 평평한 정육각형(꼭짓점 (±1, 0))과, 팔 끝 모서리가 외접원 위에 선 십자. */
        hexagon: (x, y) => Math.abs(y) <= ROOT3 / 2 && ROOT3 * Math.abs(x) + Math.abs(y) <= ROOT3,
        cross: (x, y) => {
          const arm = 3 / Math.sqrt(10);
          const side = 1 / Math.sqrt(10);
          return (Math.abs(x) <= side && Math.abs(y) <= arm) || (Math.abs(y) <= side && Math.abs(x) <= arm);
        },
      };
      const most = Math.max(...Object.values(ideal));
      const median = (values) => values.slice().sort((one, two) => one - two)[Math.floor(values.length / 2)];
      const round3 = (value) => Math.round(value * 1000) / 1000;
      /* 판의 한 CSS 자리가 든 기기 픽셀의 열쇠 — 두 손의 가면을 자리로 맞댄다. */
      const keyAt = (scale, x, y) => `${Math.floor(x * scale)},${Math.floor(y * scale)}`;
      /* 점 하나의 가면: 상자 안 기기 픽셀마다 칠해졌는가. 거리(`dx`·`dy`)는 CSS 픽셀이다. 배경은 상자
       * 테두리 픽셀의 중앙값이다. */
      const maskOf = (image, node) => {
        const { scale } = image;
        const half = Math.ceil((node.promised * most + tune.pad) * scale);
        const cx = (node.x - image.clip.x) * scale;
        const cy = (node.y - image.clip.y) * scale;
        const px = (x, y) => {
          const at = (y * image.wide + x) * 4;
          return [image.data[at], image.data[at + 1], image.data[at + 2]];
        };
        const x0 = Math.max(0, Math.floor(cx - half));
        const x1 = Math.min(image.wide - 1, Math.ceil(cx + half));
        const y0 = Math.max(0, Math.floor(cy - half));
        const y1 = Math.min(image.tall - 1, Math.ceil(cy + half));
        const rim = [];
        for (let x = x0; x <= x1; x += 1) rim.push(px(x, y0), px(x, y1));
        for (let y = y0; y <= y1; y += 1) rim.push(px(x0, y), px(x1, y));
        const ground = [0, 1, 2].map((channel) => median(rim.map((one) => one[channel])));
        const originX = Math.round(image.clip.x * scale);
        const originY = Math.round(image.clip.y * scale);
        const cells = [];
        for (let y = y0; y <= y1; y += 1) {
          for (let x = x0; x <= x1; x += 1) {
            const away = Math.max(...px(x, y).map((value, channel) => Math.abs(value - ground[channel])));
            cells.push({ key: `${x + originX},${y + originY}`, dx: (x + 0.5 - cx) / scale, dy: (y + 0.5 - cy) / scale, away });
          }
        }
        const centre = px(Math.floor(cx), Math.floor(cy));
        /* 점의 잉크가 배경에서 벗어난 폭: 고리는 둘레 띠에서, 나머지는 상자 전체에서 가장 멀리. */
        const ring = node.shape === "ring";
        const level = Math.max(0, ...cells.filter((cell) => !ring
          || Math.abs(Math.hypot(cell.dx, cell.dy) - node.reach) <= tune.ringBand).map((cell) => cell.away));
        for (const cell of cells) cell.inked = level > 0 && cell.away >= level * tune.inkShare;
        return { cells, level, centre, scale, inked: new Set(cells.filter((cell) => cell.inked).map((cell) => cell.key)) };
      };
      const iou = (cells, isIn) => {
        let both = 0;
        let either = 0;
        for (const cell of cells) {
          const want = isIn(cell);
          if (want && cell.inked) both += 1;
          if (want || cell.inked) either += 1;
        }
        return either === 0 ? 0 : both / either;
      };
      /* CSS 자리 하나가 든 기기 픽셀이 칠해졌는가. */
      const inkedAt = (mask, dx, dy) => {
        const reach = 0.5 / mask.scale;
        const cell = mask.cells.find((one) => Math.abs(one.dx - dx) <= reach && Math.abs(one.dy - dy) <= reach);
        return cell === undefined ? null : cell.inked;
      };
      const images = { svg: await decode(shots.svg), gl: await decode(shots.gl) };
      /* 점선의 한 주기(px) — 긋는 길이와 쉬는 길이, 유령의 점선 토큰 그대로. */
      const dashPeriod = getComputedStyle(document.querySelector(".knowledge-view:not([hidden])"))
        .getPropertyValue("--knowledge-node-ghost-dash").trim().split(/[\s,]+/u).slice(0, 2)
        .map(Number).reduce((sum, one) => sum + one, 0);
      const rows = {};
      for (const node of drawn.svg.nodes) {
        const twin = drawn.gl.nodes.find((one) => one.kind === node.kind);
        /* 이웃 점의 상자와 닿지 않는가 — 닿으면 이웃의 잉크가 이 가면에 든다. */
        const apart = drawn.svg.nodes.every((other) => other === node
          || Math.abs(other.x - node.x) >= 2 * Math.ceil(node.promised * most + tune.pad));
        const masks = { svg: maskOf(images.svg, node), gl: maskOf(images.gl, twin) };
        const r = node.promised;
        const row = { shape: node.shape, reach: node.reach, open: node.open && twin.open, apart,
          sameSeat: Math.abs(node.x - twin.x) < 0.5 && Math.abs(node.y - twin.y) < 0.5,
          level: { svg: masks.svg.level, gl: masks.gl.level } };
        if (node.shape === "ring") {
          /* 둘레를 따라 짚는다: 칠해진 점의 몫과 칠해진 조각(이어진 점들)의 수. */
          const around = (mask) => {
            const byKey = new Map(mask.cells.map((cell) => [cell.key, cell]));
            const steps = Math.round((2 * Math.PI * node.reach) / tune.ringStep);
            const inked = [];
            for (let at = 0; at < steps; at += 1) {
              const angle = (at / steps) * 2 * Math.PI;
              inked.push(tune.ringProbe.some((lift) => byKey.get(keyAt(mask.scale,
                node.x + Math.cos(angle) * (node.reach + lift),
                node.y + Math.sin(angle) * (node.reach + lift)))?.inked === true));
            }
            let pieces = 0;
            for (let at = 0; at < steps; at += 1) {
              if (inked[at] && !inked[(at + steps - 1) % steps]) pieces += 1;
            }
            return { share: round3(inked.filter(Boolean).length / steps), pieces };
          };
          const hollow = (mask) => mask.cells.filter((cell) => Math.hypot(cell.dx, cell.dy) < node.reach - 3 * tune.ringBand)
            .every((cell) => !cell.inked);
          row.around = { svg: around(masks.svg), gl: around(masks.gl) };
          row.peak = round3(masks.gl.level / Math.max(1, masks.svg.level));
          row.hollow = { svg: hollow(masks.svg), gl: hollow(masks.gl) };
          row.pieces = Math.round((2 * Math.PI * node.reach) / dashPeriod);
        } else {
          const classify = (mask) => Object.fromEntries(Object.keys(inside).map((shape) => [shape,
            round3(iou(mask.cells, (cell) => inside[shape](cell.dx / (r * ideal[shape]), cell.dy / (r * ideal[shape]))))]));
          row.fit = { svg: classify(masks.svg), gl: classify(masks.gl) };
          row.centre = { svg: masks.svg.centre, gl: masks.gl.centre };
          row.best = Object.fromEntries(["svg", "gl"].map((hand) => [hand,
            Object.entries(row.fit[hand]).sort((one, two) => two[1] - one[1])[0][0]]));
          let both = 0;
          for (const key of masks.svg.inked) if (masks.gl.inked.has(key)) both += 1;
          const either = masks.svg.inked.size + masks.gl.inked.size - both;
          row.hands = either === 0 ? 0 : round3(both / either);
          const R = node.reach;
          const points = {
            square: [["corner inside the square, outside the circle of its size", 0.78 * r, 0.78 * r, true]],
            circle: [["the same corner point on a circle", 0.78 * r, 0.78 * r, false]],
            triangle: [["toward the apex", 0, -0.75 * R, true], ["below the base", 0, 0.75 * R, false]],
            diamond: [["toward the top tip", 0, -0.85 * R, true], ["at the corner a square would fill", 0.6 * R, 0.6 * R, false]],
            hexagon: [["toward a vertex, past the circle of its size", 0.96 * R, 0, true]],
            cross: [["down an arm, past the circle of its size", 0, -0.88 * R, true],
              ["between two arms, where a circle or a square would fill", 0.5 * R, 0.5 * R, false]],
          }[node.shape] ?? [];
          row.points = points.map(([name, dx, dy, want]) => ({ name, want,
            svg: inkedAt(masks.svg, dx, dy), gl: inkedAt(masks.gl, dx, dy) }));
        }
        rows[node.kind] = row;
      }
      return { ratio: images.gl.scale, hands: [drawn.svg.hand, drawn.gl.hand],
        elements: [drawn.svg.elements, drawn.gl.elements], rows };
    } catch (error) {
      return { thrown: String(error?.stack ?? error) };
    }
  }, { drawn, shots, tune: SHAPE_PIXELS, ideal: IDEAL_REACH });
  await target.evaluate((tune) => {
    document.getElementById("knowledge-shape-pixels")?.remove();
    const view = document.querySelector(".knowledge-view:not([hidden])");
    for (const name of tune.sizeTokens) view?.style.removeProperty(name);
    if (view) knowledgeTunings.delete(view);
    knowledgePainterKind = null;
    knowledgeShowSources = false;
    knowledgeShowCode = false;
  }, SHAPE_PIXELS);
  if (seatWas) await target.setViewportSize(seatWas);
  return pixels;
};

/* 한 번은 이 판(기기 픽셀 1배)에서, 한 번은 창이 실제로 서는 레티나(2배)에서 — GL 손의 크기가
 * CSS 픽셀의 약속을 지키는지는 기기 픽셀이 CSS 픽셀과 다른 판에서만 드러난다. */
const RETINA_RATIO = 2;
const retinaContext = await glBrowser.newContext({ viewport: { width: 1280, height: 860 },
  deviceScaleFactor: RETINA_RATIO });
const retinaPage = await retinaContext.newPage();
retinaPage.on("pageerror", (error) => faults.push(error?.stack ?? String(error)));
await seedKnowledgeWindow(retinaPage);
await retinaPage.goto(`${origin}/index.html`);
await retinaPage.waitForFunction(() => typeof BOUND !== "undefined" && BOUND.size > 0);
await retinaPage.evaluate(() => document.getElementById("nav-knowledge").click());
await retinaPage.waitForFunction(() => document.querySelector(".knowledge-view:not([hidden])") !== null,
  null, { timeout: 8000 });
const pixelRuns = [await shapePixelsOn(glPage), await shapePixelsOn(retinaPage)];
await retinaContext.close();
const skipped = pixelRuns.find((run) => run.skip);
if (skipped) {
  console.log(`SKIP P1 G3 pixel comparison of the two painters: ${skipped.skip}`);
} else {
  const pixelDetail = JSON.stringify(pixelRuns);
  const ratiosSeen = pixelRuns.map((run) => run.ratio).join(",") === `1,${RETINA_RATIO}`;
  const filledOf = (run) => (run.thrown ? [] : Object.values(run.rows).filter((row) => row.shape !== "ring"));
  ok("P1 G3: at 1x and 2x device pixels the two painters stand the seven kinds on the same seats, apart and unhidden, one with elements and one without",
    ratiosSeen && pixelRuns.every((run) => !run.thrown && run.hands.join(",") === "svg,gl"
      && run.elements[0] === 7 && run.elements[1] === 0 && Object.values(run.rows).length === 7
      && Object.values(run.rows).every((row) => row.sameSeat && row.open && row.apart)),
    pixelDetail);
  ok("P1 G3: at 1x and 2x device pixels each filled kind is its table shape in both painters, the two painters' ink overlaps, and its centre is the same colour",
    ratiosSeen && pixelRuns.every((run) => filledOf(run).length === 6 && filledOf(run).every((row) => row.best.svg === row.shape
      && row.best.gl === row.shape
      && row.fit.svg[row.shape] >= SHAPE_PIXELS.overlap && row.fit.gl[row.shape] >= SHAPE_PIXELS.overlap
      && row.hands >= SHAPE_PIXELS.overlap
      && row.centre.svg.every((value, channel) => Math.abs(value - row.centre.gl[channel]) <= SHAPE_PIXELS.centreLevels))),
    pixelDetail);
  ok("P1 G3: at 1x and 2x device pixels the square's corner fills where a circle's does not, the triangle points up, the diamond fills its tips, the hexagon reaches its vertices and the cross leaves its corners open, in both painters",
    ratiosSeen && pixelRuns.every((run) => filledOf(run).length === 6 && filledOf(run).every((row) => row.points.length > 0
      && row.points.every((point) => point.svg === point.want && point.gl === point.want))),
    pixelDetail);
  /* 고리는 모양으로 고른다 — 유령이 다른 모양을 입은 판에서도 판정이 던지지 않고 빨갛게 서게. */
  ok("P1 G3: at 1x and 2x device pixels the ghost is a hollow ring broken into the token's dashes, as strong in both painters",
    ratiosSeen && pixelRuns.every((run) => {
      const rings = run.thrown ? [] : Object.values(run.rows).filter((row) => row.shape === "ring");
      return rings.length === 1 && rings.every((row) => row.hollow.svg && row.hollow.gl
        && row.peak >= SHAPE_PIXELS.ringPeak
        && ["svg", "gl"].every((hand) => row.pieces > 0
          && Math.abs(row.around[hand].pieces - row.pieces) <= row.pieces * SHAPE_PIXELS.dashSlack)
        && Math.abs(row.around.svg.share - row.around.gl.share) <= SHAPE_PIXELS.dutyGap);
    }),
    pixelDetail);
  /* 초록일 때도 수가 남게 — 모양마다 제 모양과의 겹침(SVG·GL)과 두 손의 겹침, 고리는 점선 조각. */
  console.log(`METRIC knowledge shape pixels: ${JSON.stringify(pixelRuns.map((run) => (run.thrown ? { thrown: run.thrown }
    : { ratio: run.ratio, ...Object.fromEntries(Object.entries(run.rows).map(([kind, row]) => [kind, row.shape === "ring"
      ? { pieces: row.pieces, svg: row.around.svg, gl: row.around.gl, peak: row.peak }
      : { own: [row.fit.svg[row.shape], row.fit.gl[row.shape]], hands: row.hands, centre: row.centre }])) })))}`);
}
await glBrowser.close();

console.log(`METRIC knowledge graph 1020 nodes: first paint ${brainScale.firstPaint}ms; `
  + `worst frame gap ${brainScale.worstGap}ms (${loadNote()}); spread ${brainScale.spread}; `
  + `DOM/node ${brainScale.maxNodeParts}; halo circle ${haloPaint.circleMs}ms `
  + `vs filter ${haloPaint.filterMs}ms (paired ${haloPaint.pairedRatio})`);
console.log(`PIXEL ${pixelShots.map((shot) => shot.path).join(" ")}`);

await browser.close();
files.close();

const failed = results.filter((r) => !r.pass);
if (failed.length > 0) {
  console.error(`\nFAILED ${failed.length} / ${results.length} tests`);
  process.exit(1);
} else {
  console.log(`\nALL ${results.length} TESTS PASSED!`);
  process.exit(0);
}
