import { createRequire } from "node:module";
import { createServer } from "node:http";
import { readFile, mkdir } from "node:fs/promises";
import { dirname, extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { KNOWLEDGE_SCENES, knowledgeSweepFor, measureKnowledgeGlParity, measureKnowledgePainterSwap,
  measureKnowledgeScenes,
  testKnowledgePerformance } from "./knowledge-performance.mjs";
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
    initialFitRatio: ((layout.bounds.maxX - layout.bounds.minX) * layout.scale)
      / canvas.clientWidth,
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
    // 잎은 작고, 대표 지식은 그보다 크고, 군집의 중심이 가장 크다.
    brain.tierRadii.leaf >= 3 &&
    brain.tierRadii.major > brain.tierRadii.leaf &&
    brain.tierRadii.core > brain.tierRadii.major &&
    brain.radiusMin >= 3 &&
    brain.radiusMax === brain.tierRadii.core &&
    brain.labelBelow &&
    brain.labelPaintOrder.includes("stroke") &&
    parseFloat(brain.labelStrokeWidth) === 2 &&
    brain.initialFitRatio >= 0.68 &&
    brain.initialFitRatio <= 0.74 &&
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
    // 카파시의 lint 여덟 줄과 라이브 층의 셋(t-2931) — 열한 줄, 수는 픽스처가
    // 지은 `graph.lint.counts`의 것(라이브 층이 없는 답에서 셋은 0)이고 창은
    // 하나도 세지 않는다.
    brainOpen.overview.health.length === 11 &&
    JSON.stringify(brainOpen.overview.health) === JSON.stringify(brainOpen.overview.healthExpected) &&
    brainOpen.overview.health.includes("orphans=1") &&
    brainOpen.overview.orphanTile === "1" &&
    brainOpen.overview.health.includes("ghosts=3") &&
    brainOpen.overview.health.includes("index_gaps=12") &&
    brainOpen.overview.health.includes("contradictions=1") &&
    brainOpen.overview.health.includes("superseded=1") &&
    JSON.stringify(brainOpen.overview.healthDoorless) === JSON.stringify(["unlogged_raw"]) &&
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
    brainFixture.orphanTile === "1" &&
    brainFixture.ghostTile === "1" &&
    brainFixture.clean.length === 0,
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
  "eight cluster hues and five relation colors keep three-to-one contrast in both themes",
  [brainContrast.dark, brainContrast.light].every((theme) => theme.colors.length === 13
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
  const fitRatio = (spread * layout.scale) / canvasBox.clientWidth;
  /* 09-16: 카메라는 두 축을 다 판 안에 넣는다(contain) — 폭만 맞추던 옛 셈은 판이
     짧아지면 그림의 위아래를 잘랐다. 그래서 「토큰의 몫을 차지한다」는 **묶는 축**의
     이야기이고, 다른 축은 그보다 작다. 앞 케이스의 배율(zoom)은 여전히 물려받는다 —
     볼트가 자라도 사람의 카메라는 뛰지 않는다. */
  const tallSpread = (layout.bounds.maxY - layout.bounds.minY)
    * (view.classList.contains("is-tier-compact") || view.classList.contains("is-tier-tiny")
      ? layout.tuning.yScaleCompact
      : view.classList.contains("is-tier-middle") ? layout.tuning.yScaleMiddle : 1);
  const fitTallRatio = (tallSpread * layout.scale) / canvasBox.clientHeight;
  const fitExpected = layout.tuning.fitRatio * layout.zoom;
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
    brainScale.worstGap < 12 * 8 &&
    brainScale.spread > 400 && brainScale.spread < 6000 &&
    // 묶는 축이 토큰의 몫을 차지하고, 두 축 다 그 몫을 넘지 않는다(contain).
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
    haloPaint.pairedRatio <= 1.1 &&
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

// Test 11: Shortest path (Shift-click, undirected BFS preferring typed edges, chain A → … → B untranslated, Esc clears, < 1ms)
const pathTest = await page.evaluate(async () => {
  // Test BFS performance on 1020 nodes first. The function is pure, so its
  // cost is the least of a few runs: one run is a sample of the machine's
  // load as much as of the search, and a gate on that lone sample went red
  // at 1.1 ms in four release lanes while the same search answered in 0.5.
  const view1020 = document.querySelector(".knowledge-view:not([hidden])");
  const layout1020 = knowledgeLayouts.get(view1020);
  let bfs1020Ms = Infinity;
  if (typeof findKnowledgeShortestPath === "function") {
    for (let run = 0; run < 5; run += 1) {
      const t0 = performance.now();
      findKnowledgeShortestPath(layout1020.model, 0, 500);
      bfs1020Ms = Math.min(bfs1020Ms, performance.now() - t0);
    }
  }

  // Setup custom graph with 6 pages:
  // Node 0 -> Node 1 (mentions), Node 1 -> Node 3 (mentions)  [mentions path: 0 -> 1 -> 3]
  // Node 0 -> Node 2 (depends_on), Node 2 -> Node 3 (implements)  [typed path: 0 -> 2 -> 3]
  // Node 4 (isolated), Node 5 (mentions to 4)
  knowledgeSelectedKey = null;
  knowledgeSlicerCutoff = 0;
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
  await new Promise((done) => setTimeout(done, 300));

  const view = document.querySelector(".knowledge-view:not([hidden])");
  const canvas = view.querySelector(".knowledge-canvas");
  const nodes = [...view.querySelectorAll(".knowledge-node")];

  // 1. Select Node 0
  nodes[0]?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  await new Promise((done) => setTimeout(done, 50));
  const node0Selected = nodes[0]?.classList.contains("is-selected") ?? false;
  // Focus depth 1 (the default a person starts at): node 3 is two hops from node 0.
  view.querySelector('[data-knowledge-depth="1"]')?.click();
  await new Promise((done) => setTimeout(done, 50));

  // 2. Shift-click Node 3
  nodes[3]?.dispatchEvent(new MouseEvent("click", { bubbles: true, shiftKey: true }));
  await new Promise((done) => setTimeout(done, 50));

  const picture = view.querySelector(".knowledge-picture");
  const isPathActive = picture?.classList.contains("is-path") ?? false;
  const pathLitNodes = [...view.querySelectorAll(".knowledge-node.is-path-lit")]
    .map((n) => n.dataset.graphKey).sort();
  const pathDimNodes = view.querySelectorAll(".knowledge-node.is-path-dim").length;
  const pathLitEdges = view.querySelectorAll(".knowledge-edge.is-path-lit").length;

  const chainEl = view.querySelector(".knowledge-inspector-chain");
  const chainText = chainEl?.textContent ?? "";
  const chainVisible = chainEl && !chainEl.hidden;

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

  // Esc clears path
  canvas.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  await new Promise((done) => setTimeout(done, 50));

  const pathClearedAfterEsc = !picture?.classList.contains("is-path")
    && view.querySelectorAll(".knowledge-node.is-path-lit").length === 0
    && (chainEl?.hidden ?? true);
  const node0StillSelected = nodes[0]?.classList.contains("is-selected") ?? false;

  // Esc again deselects node
  canvas.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  await new Promise((done) => setTimeout(done, 50));
  const node0Deselected = !(nodes[0]?.classList.contains("is-selected") ?? false);

  return {
    bfs1020Ms: Math.round(bfs1020Ms * 100) / 100,
    node0Selected,
    isPathActive,
    pathLitNodes,
    pathDimNodes,
    pathLitEdges,
    chainVisible,
    chainText,
    pathNodeOpacity,
    offPathOpacity,
    dimEdgeOpacity,
    onPathStroke,
    highlightInk,
    pathClearedAfterEsc,
    node0StillSelected,
    node0Deselected,
  };
});

ok(
  "shortest path: Shift-click runs undirected BFS preferring typed edges, shows untranslated chain, and Esc clears",
  pathTest.node0Selected &&
    pathTest.isPathActive &&
    pathTest.pathLitNodes.length === 3 &&
    pathTest.pathLitNodes.includes("wiki/Page-0000.md") &&
    pathTest.pathLitNodes.includes("wiki/Page-0002.md") &&
    pathTest.pathLitNodes.includes("wiki/Page-0003.md") &&
    !pathTest.pathLitNodes.includes("wiki/Page-0001.md") &&
    pathTest.pathDimNodes > 0 &&
    pathTest.pathLitEdges === 2 &&
    pathTest.chainVisible &&
    pathTest.chainText.includes("depends_on") &&
    pathTest.chainText.includes("implements") &&
    !pathTest.chainText.includes("mentions") &&
    pathTest.pathNodeOpacity.length === 3 &&
    pathTest.pathNodeOpacity.every((opacity) => opacity === 1) &&
    Math.abs(pathTest.offPathOpacity - pathTest.dimEdgeOpacity) < 0.01 &&
    pathTest.highlightInk !== "" &&
    pathTest.onPathStroke === pathTest.highlightInk &&
    pathTest.pathClearedAfterEsc &&
    pathTest.node0StillSelected &&
    pathTest.node0Deselected &&
    pathTest.bfs1020Ms < 1.0,
  JSON.stringify(pathTest),
);
console.log(`   shortest path 1020 nodes BFS time: ${pathTest.bfs1020Ms}ms`);

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
  const chain = "개념 0 → depends_on → 개념 2 → implements → 개념 3";
  const same = (seen) => seen.path
    && JSON.stringify(seen.lit) === JSON.stringify(route)
    && JSON.stringify(seen.edges) === JSON.stringify(edges)
    && seen.chain === chain;
  ok("a shown path is held by its two pages and found again when a lens re-seats the picture",
    same(pathKeyed.before)
      && same(pathKeyed.typed)
      && !pathKeyed.orphans.path && pathKeyed.orphans.lit.length === 0 && pathKeyed.orphans.chain === null
      && same(pathKeyed.back),
    JSON.stringify(pathKeyed));
}

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
      && /^[a-z_?]+/.test(row.note) && row.why.length > 0
      && row.note === `${row.kindWord}${row.why}`)
    && panel.chosen.incoming.every((row) => /\bkind-[a-z_]+\b/.test(row.mark) && row.dir === "←"
      && /^[a-z_?]+/.test(row.note) && row.why.length > 0
      && row.note === `${row.kindWord}${row.why}`)
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
  supply: { components: 2, vulnerabilities: 1 } };
/* 원본은 볼트의 앞 다섯 쪽이 하나씩 인용한다(`knowledge-fixture.mjs`). */
const grammarCount = grammarVault.pages + grammarVault.ghosts + Math.min(5, grammarVault.pages)
  + grammarVault.supply.components + grammarVault.supply.vulnerabilities;
await page.setViewportSize({ width: 1280, height: 860 });
const grammarOpened = await page.evaluate((vault) => {
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
    knowledgeReport = null;
    knowledgeAskedAt = 0;
    window.__VAULT__ = vault;
    dropTab("knowledge");
    document.getElementById("nav-knowledge").click();
    return "";
  } catch (error) {
    return String(error?.stack ?? error);
  }
}, grammarVault);
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
       * 비율로 곱한 반지름을 같은 비율로 다시 나누는 셈은 무엇도 묻지 않는다. */
      const promised = seat < 0 ? 0
        : knowledgeNodeRadius(layout.model.degree[seat], layout.tuning, layout.tier[seat]);
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
    await paintKnowledgeView();
  }
});
/* 넓이의 허용: 윤곽의 좌표는 0.01 px로 반올림되고(가장 작은 3 px 점에서 넓이 0.1% 안),
 * 설계의 합격선은 2%다(§2 G3). */
const AREA_SLACK = 0.02;
const shapeDetail = JSON.stringify(shapeGrammar);
ok("P1 G3: five kinds meet five shapes one to one, and every shape has one geometry row",
  !shapeGrammar.thrown
    && shapeGrammar.table.kinds.join(",") === "page,ghost,source,component,vulnerability"
    && shapeGrammar.table.distinct === shapeGrammar.table.kinds.length && shapeGrammar.table.geometry,
  shapeDetail);
ok("P1 G3: every kind stands in its own shape, drawn from its geometry row, at the area its size promises",
  !shapeGrammar.thrown && Object.values(shapeGrammar.picture).every((row) => row.drawn === row.shape
    && row.element === row.wantedElement && row.fromTable && row.reachError < 1e-4
    && Math.abs(row.areaRatio - 1) < AREA_SLACK && row.dashedAgrees),
  shapeDetail);
ok("P1 G4: every legend row draws the picture's own shape from the same geometry row",
  !shapeGrammar.thrown && ["page", "ghost", "source", "recalled"]
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
  /* 점 사이(그래프 단위) — 폭에 맞춘 카메라에서 이웃 외접원이 닿지 않는지는 아래에서 묻는다. */
  pitch: 100,
  /* 점의 상자를 외접원 밖으로 넓히는 폭(px) — 테두리와 부드러움이 상자 안에 들게. */
  pad: 6,
  /* 칠해졌는가: 배경에서 벗어난 폭이 점의 잉크가 벗어난 폭의 절반을 넘는다. */
  inkShare: 0.5,
  /* 겹침의 하한 0.9: 같은 넓이의 서로 다른 모양끼리는 최대 0.839(원·마름모)이고, 같은 모양을
   * 1 px 안쪽으로 줄인 것과는 최소 0.938(사각)이다 — 둘 사이에 선다. */
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
  triangle: Math.sqrt((4 * Math.PI) / (3 * Math.sqrt(3))) });

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
      for (const name of ["--knowledge-node-radius-min", "--knowledge-node-radius-max",
        "--knowledge-core-radius", "--knowledge-major-radius"]) {
        view.style.setProperty(name, String(tune.radius));
      }
      knowledgeTunings.delete(view);
      const spec = { pages: 1, ghosts: 1, tags: [], customEdges: [],
        supply: { components: 1, vulnerabilities: 1 } };
      window.__VAULT__ = spec;
      knowledgeShowSources = true;
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
      knowledgeReport = window.__buildVaultGraph__({ path: "/shapes", sources: true }, spec);
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
  await target.evaluate(() => {
    document.getElementById("knowledge-shape-pixels")?.remove();
    const view = document.querySelector(".knowledge-view:not([hidden])");
    for (const name of ["--knowledge-node-radius-min", "--knowledge-node-radius-max",
      "--knowledge-core-radius", "--knowledge-major-radius"]) {
      view?.style.removeProperty(name);
    }
    if (view) knowledgeTunings.delete(view);
    knowledgePainterKind = null;
    knowledgeShowSources = false;
  });
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
  ok("P1 G3: at 1x and 2x device pixels the two painters stand the five kinds on the same seats, apart and unhidden, one with elements and one without",
    ratiosSeen && pixelRuns.every((run) => !run.thrown && run.hands.join(",") === "svg,gl"
      && run.elements[0] === 5 && run.elements[1] === 0 && Object.values(run.rows).length === 5
      && Object.values(run.rows).every((row) => row.sameSeat && row.open && row.apart)),
    pixelDetail);
  ok("P1 G3: at 1x and 2x device pixels each filled kind is its table shape in both painters, the two painters' ink overlaps, and its centre is the same colour",
    ratiosSeen && pixelRuns.every((run) => filledOf(run).length === 4 && filledOf(run).every((row) => row.best.svg === row.shape
      && row.best.gl === row.shape
      && row.fit.svg[row.shape] >= SHAPE_PIXELS.overlap && row.fit.gl[row.shape] >= SHAPE_PIXELS.overlap
      && row.hands >= SHAPE_PIXELS.overlap
      && row.centre.svg.every((value, channel) => Math.abs(value - row.centre.gl[channel]) <= SHAPE_PIXELS.centreLevels))),
    pixelDetail);
  ok("P1 G3: at 1x and 2x device pixels the square's corner fills where a circle's does not, the triangle points up and the diamond fills its tips, in both painters",
    ratiosSeen && pixelRuns.every((run) => filledOf(run).length === 4 && filledOf(run).every((row) => row.points.length > 0
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
  + `worst frame gap ${brainScale.worstGap}ms; spread ${brainScale.spread}; `
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
