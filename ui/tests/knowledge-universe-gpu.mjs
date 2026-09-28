/* 우주 보기(t-12443)의 수를 진짜 GPU에서 잰다 — 승인된 시안 v4의 성능 표와 같은 틀.
 *
 * 시안(docs/design/knowledge-graph-3d-v4/README.md 「성능·메모리」)은 WebKit에서, 무대 CSS 1166×796, 화면 배율 상한
 * 1.5(캔버스 1749×1194)로, 시안과 같은 모양의 합성 볼트 850 · 5,000 · 10,000쪽을 쟀다. 여기서도 같다: 창의
 * 무대가 그 크기가 되는 판에서, 같은 모양의 합성 볼트(knowledge-universe-fixture.mjs)를 연다.
 *
 * - 첫 3D 프레임: 평면 지도가 선 판에서 3D를 누른 때부터 우주의 첫 장까지(자료·장면 짓기·셰이더·첫 렌더).
 * - 프레임 일: 한 장(장면 + 빛 번짐 + 합성)을 그리고 1픽셀을 읽어 GPU가 끝날 때까지 기다린 시간 120번 — 위쪽 한계.
 *   잇단 평균: 60장을 잇달아 그린 뒤 한 번 기다린 평균(시안 `costTest`).
 * - 회전: 움직임을 켜고 쉬게 둔 6초 — 프레임 간격, 25 ms 넘은 프레임, 한 장의 JS 일(`frame`), 드로우 · 점 · 삼각형.
 * - 메모리: 렌더러 장부(기하 · 텍스처 · 프로그램), 버퍼와 텍스처의 바이트, 프레임버퍼(장면 목표 · 번짐 사다리 · 캔버스).
 * - 쉼: 움직임을 끄고 손을 뗀 뒤 3초 동안 그린 장 수.
 * - 날기: 가장 큰 은하를 골라 날아가는 동안 한 장의 일(이름표 자리 포함, GPU 완료까지).
 * - 힙(Chromium만 — `performance.memory`): 850쪽을 60초 돌리며 GC 뒤의 JS 힙을 10초마다.
 *
 *   node ui/tests/knowledge-universe-gpu.mjs [--engine webkit|chromium] [--pages 850,5000,10000]
 *     [--parts first,cost,spin,idle,fly] [--heap] [--out file.json]
 *
 * 표는 stdout의 JSON이다. 무거운 판이 도는 기계에서는 한 번에 한 크기씩(`--pages 850`) 돌린다. */
import { createRequire } from "node:module";
import { createServer } from "node:http";
import { readFile, writeFile } from "node:fs/promises";
import { loadavg } from "node:os";
import { dirname, extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { seedKnowledgeWindow } from "./knowledge-fixture.mjs";
import { seedUniverseVault } from "./knowledge-universe-fixture.mjs";

const UI = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const require = createRequire(import.meta.url);
const playwright = require("playwright");

const flag = (name, fallback) => {
  const at = process.argv.indexOf(`--${name}`);
  return at < 0 ? fallback : (process.argv[at + 1] ?? "true");
};
const engineName = flag("engine", "webkit");
const sizes = flag("pages", "850,5000,10000").split(",").map(Number);
const wantedParts = new Set(flag("parts", "first,cost,spin,idle,fly").split(","));
const wantHeap = process.argv.includes("--heap");
const out = flag("out", null);

/* 시안의 판: 무대 CSS 1166×796, 화면 배율 2에 상한 1.5. 창의 무대가 그 크기가 되는 창 크기는 판의 머리·옆 판에
 * 달렸으므로 한 번 재어 맞춘다(`fitStage`). */
const STAGE = Object.freeze({ width: 1166, height: 796, scale: 2 });
const COST_FRAMES = 120;
const BURST_FRAMES = 60;
const SPIN_MS = 6000;
const IDLE_SETTLE_MS = 600;
const IDLE_MS = 3000;
const LATE_FRAME_MS = 25;
const HEAP_MS = 60000;
const HEAP_EVERY_MS = 10000;

const files = createServer(async (request, response) => {
  const asked = decodeURIComponent((request.url ?? "/").split("?")[0]);
  const path = join(UI, asked === "/" ? "index.html" : asked);
  if (!path.startsWith(UI)) {
    response.writeHead(403).end();
    return;
  }
  try {
    const data = await readFile(path);
    const type = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css",
      ".mjs": "text/javascript", ".png": "image/png", ".ico": "image/x-icon" };
    response.writeHead(200, { "content-type": type[extname(path)] ?? "text/plain" });
    response.end(data);
  } catch {
    response.writeHead(404).end();
  }
});
await new Promise((done) => files.listen(0, "127.0.0.1", done));
const origin = `http://127.0.0.1:${files.address().port}`;

const median = (values) => values.slice().sort((one, two) => one - two)[Math.floor(values.length / 2)] ?? 0;
const quantile = (values, share) => {
  const sorted = values.slice().sort((one, two) => one - two);
  return sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * share))] ?? 0;
};
const round = (value, digits = 2) => Math.round(value * 10 ** digits) / 10 ** digits;

async function openWindow(engine, { heap = false } = {}) {
  /* 헤드리스 Chromium은 macOS에서 ANGLE의 Metal 백엔드로 GPU를 쓴다(인자 없이는 WebGL2가 서지 않는다). */
  const browser = await engine.launch(engineName === "chromium"
    ? { args: ["--enable-precise-memory-info", "--use-angle=metal", "--enable-gpu", "--ignore-gpu-blocklist",
      ...(heap ? ["--js-flags=--expose-gc"] : [])] }
    : {});
  const context = await browser.newContext({ viewport: { width: 1920, height: 1000 }, deviceScaleFactor: STAGE.scale });
  const page = await context.newPage();
  const faults = [];
  page.on("pageerror", (error) => faults.push(String(error).slice(0, 300)));
  await seedKnowledgeWindow(page);
  await seedUniverseVault(page);
  await page.goto(`${origin}/index.html`);
  await page.waitForFunction(() => typeof BOUND !== "undefined" && BOUND.size > 0);
  return { browser, page, faults };
}

/* 볼트를 열고 평면 지도가 앉을 때까지 — 우주는 그 자리에서 떠오른다. */
async function openVault(page, pages) {
  await page.evaluate((pages) => {
    secondBrainVault = "/universe";
    window.__VAULT__ = window.__universeVaultSpec__(pages);
    noteKnowledgeExploreLines({ "/universe": JSON.stringify({ mode: "global", dimension: "2d" }) });
    const view = document.querySelector(".knowledge-view:not([hidden])");
    if (view === null) {
      document.getElementById("nav-knowledge").click();
      return;
    }
    const held = knowledgeUniverses.get(view);
    if (held !== undefined) setKnowledgeDimension(view, "2d");
    knowledgeDimension = "2d";
    knowledgeAskedAt = 0;
    void refreshKnowledgeGraph({ force: true });
  }, pages);
  await page.waitForFunction((pages) => {
    const view = document.querySelector(".knowledge-view:not([hidden])");
    const layout = view === null ? undefined : knowledgeLayouts.get(view);
    return layout !== undefined && layout.count >= pages && layout.left === 0;
  }, pages, { timeout: 600000, polling: 250 });
}

/* 창의 무대가 시안의 무대(1166×796)가 되게 창 크기를 맞춘다. */
async function fitStage(page) {
  for (let round = 0; round < 3; round += 1) {
    const stage = await page.evaluate(() => {
      const box = document.querySelector(".knowledge-view:not([hidden]) .knowledge-canvas").getBoundingClientRect();
      return { width: box.width, height: box.height, viewport: [window.innerWidth, window.innerHeight] };
    });
    const width = Math.round(stage.viewport[0] + STAGE.width - stage.width);
    const height = Math.round(stage.viewport[1] + STAGE.height - stage.height);
    if (width === stage.viewport[0] && height === stage.viewport[1]) return stage;
    await page.setViewportSize({ width, height });
    await page.waitForTimeout(300);
  }
  return page.evaluate(() => {
    const box = document.querySelector(".knowledge-view:not([hidden]) .knowledge-canvas").getBoundingClientRect();
    return { width: box.width, height: box.height, viewport: [window.innerWidth, window.innerHeight] };
  });
}

/* 한 크기의 줄 — 3D를 누르고 첫 장, 그다음 부분마다. */
async function measureSize(page, pages) {
  await openVault(page, pages);
  const stage = await fitStage(page);
  const row = { pages, stage: [Math.round(stage.width), Math.round(stage.height)] };
  Object.assign(row, await page.evaluate(async () => {
    const view = document.querySelector(".knowledge-view:not([hidden])");
    const frame = () => new Promise((done) => requestAnimationFrame(done));
    const began = performance.now();
    setKnowledgeDimension(view, "3d");
    const built = performance.now() - began;
    for (let round = 0; round < 600; round += 1) {
      if ((knowledgeUniverses.get(view)?.frames ?? 0) > 0) break;
      await frame();
    }
    const first = performance.now() - began;
    const universe = knowledgeUniverses.get(view);
    const layout = knowledgeLayouts.get(view);
    return { firstFrameMs: Math.round(first), buildMs: Math.round(built), edges: layout.model.edgeCount,
      galaxies: universe.map.galaxies, clusters: universe.map.rows - universe.map.galaxies,
      filaments: universe.map.filaments.length };
  }));
  const measuredParts = await page.evaluate(async ({ want, costFrames, burstFrames, spinMs, idleSettle, idleMs, lateMs }) => {
    const view = document.querySelector(".knowledge-view:not([hidden])");
    const universe = knowledgeUniverses.get(view);
    const frame = () => new Promise((done) => requestAnimationFrame(done));
    const sleep = (ms) => new Promise((done) => setTimeout(done, ms));
    const gl = universe.renderer.getContext();
    const pixel = new Uint8Array(4);
    const finish = () => gl.readPixels(0, 0, 1, 1, gl.RGBA, gl.UNSIGNED_BYTE, pixel);
    const result = {};
    /* 처음 자리에서 멈춰 선 판. */
    await frame();
    await frame();
    /* 프레임 일(시안 `costTest`): 한 장씩 그리고 기다리기를 `costFrames`번, 잇달아 `burstFrames`장 뒤 한 번 기다린 평균. */
    const costTest = () => {
      const each = [];
      for (let at = 0; at < costFrames; at += 1) {
        universe.yaw += 0.003;
        universe.time += 1 / 60;
        universe.uniforms.uTime.value = universe.time;
        universe.camVersion += 1;
        universe.placeCamera();
        const began = performance.now();
        universe.render();
        finish();
        each.push(performance.now() - began);
      }
      const began = performance.now();
      for (let at = 0; at < burstFrames; at += 1) {
        universe.yaw += 0.003;
        universe.placeCamera();
        universe.render();
      }
      finish();
      const burst = (performance.now() - began) / burstFrames;
      const sorted = each.slice().sort((one, two) => one - two);
      return { medianMs: sorted[Math.floor(sorted.length / 2)], p95Ms: sorted[Math.floor(sorted.length * 0.95)],
        worstMs: sorted[sorted.length - 1], burstMs: burst };
    };
    if (want.includes("cost")) {
      result.cost = costTest();
      const info = universe.renderer.info;
      result.draws = { calls: info.render.calls, points: info.render.points, triangles: info.render.triangles };
      const target = universe.post.target;
      const mips = universe.post.mips.map((one) => [one.width, one.height]);
      const texelBytes = universe.floatTargets ? 8 : 4;
      const canvas = [universe.canvas.width, universe.canvas.height];
      const sceneBytes = target.width * target.height * texelBytes;
      const bloomBytes = mips.reduce((sum, [wide, tall]) => sum + wide * tall * texelBytes, 0);
      const canvasBytes = canvas[0] * canvas[1] * 4;
      let buffers = 0;
      const counted = new Set();
      for (const geometry of universe.built.geometries) {
        for (const attribute of Object.values(geometry.attributes)) {
          if (counted.has(attribute)) continue;
          counted.add(attribute);
          buffers += attribute.array.byteLength;
        }
      }
      for (const texture of [universe.galTexture, ...universe.nodeTextures]) buffers += texture?.image?.data?.byteLength ?? 0;
      result.memory = { geometries: info.memory.geometries, textures: info.memory.textures, programs: info.programs.length,
        buffersMb: buffers / 1048576, framebufferMb: (sceneBytes + bloomBytes + canvasBytes) / 1048576,
        scene: [target.width, target.height], mips, canvas, halfFloat: universe.floatTargets };
    }
    if (want.includes("spin")) {
      /* 움직임을 켜고 이미 오래 쉰 판 — 시안의 벤치처럼 곧바로 돈다. 한 장의 JS 일은 `frame`을 감싸 잰다. */
      const plain = universe.frame;
      const work = [];
      universe.frame = function timed(now) {
        const began = performance.now();
        plain.call(this, now);
        work.push(performance.now() - began);
      };
      setKnowledgeUniverseMotion(view, true);
      universe.lastTouch = -1e9;
      universe.spin = 1;
      universe.invalidate();
      const stamps = [];
      const until = performance.now() + spinMs;
      while (performance.now() < until) stamps.push(await frame());
      universe.frame = plain;
      const same = knowledgeUniverses.get(view) === universe;
      const gaps = [];
      for (let at = 1; at < stamps.length; at += 1) gaps.push(stamps[at] - stamps[at - 1]);
      const sorted = gaps.slice().sort((one, two) => one - two);
      const js = work.slice().sort((one, two) => one - two);
      result.spin = { frames: work.length, sameUniverse: same, drawn: universe.frames, gapMedianMs: sorted[Math.floor(sorted.length / 2)],
        gapP95Ms: sorted[Math.floor(sorted.length * 0.95)], late: gaps.filter((gap) => gap > lateMs).length,
        jsMedianMs: js[Math.floor(js.length / 2)], jsP95Ms: js[Math.floor(js.length * 0.95)] };
    }
    if (want.includes("idle")) {
      setKnowledgeUniverseMotion(view, false);
      await sleep(idleSettle);
      const from = universe.frames;
      await sleep(idleMs);
      result.idle = { framesIn3s: universe.frames - from };
      setKnowledgeUniverseMotion(view, true);
    }
    if (want.includes("fly")) {
      /* 가장 큰 은하로 — 날아가는 동안 한 장의 일(이름표 자리 포함)을 GPU가 끝날 때까지. */
      setKnowledgeUniverseMotion(view, false);
      universe.goHome();
      for (let round = 0; round < 300 && universe.tween.on; round += 1) await frame();
      const plain = universe.frame;
      const work = [];
      const js = [];
      universe.frame = function timed(now) {
        const began = performance.now();
        plain.call(this, now);
        js.push(performance.now() - began);
        finish();
        work.push(performance.now() - began);
      };
      toggleKnowledgeCluster(view, 0);
      await frame();
      for (let round = 0; round < 300 && universe.tween.on; round += 1) await frame();
      await frame();
      universe.frame = plain;
      const sorted = work.slice().sort((one, two) => one - two);
      const jsSorted = js.slice().sort((one, two) => one - two);
      /* 한 장의 JS 일(이름표 자리 포함)과, 그 장을 GPU가 끝낼 때까지(`finish`)의 일. */
      result.fly = { frames: work.length, jsMedianMs: jsSorted[Math.floor(jsSorted.length / 2)],
        jsP95Ms: jsSorted[Math.floor(jsSorted.length * 0.95)], medianMs: sorted[Math.floor(sorted.length / 2)],
        p95Ms: sorted[Math.floor(sorted.length * 0.95)], worstMs: sorted[sorted.length - 1],
        names: universe.host.querySelectorAll(".knowledge-universe-name.is-on").length };
      /* 날아온 가까운 자리의 프레임 일 — 은하가 화면을 채운 자리라 채우기가 가장 무겁다(시안 `kg.costWith`를 같은
       * 자리에서 견준다). */
      result.close = costTest();
      toggleKnowledgeCluster(view, 0);
      setKnowledgeUniverseMotion(view, true);
    }
    return result;
  }, { want: [...wantedParts], costFrames: COST_FRAMES, burstFrames: BURST_FRAMES, spinMs: SPIN_MS,
    idleSettle: IDLE_SETTLE_MS, idleMs: IDLE_MS, lateMs: LATE_FRAME_MS });
  return { ...row, ...measuredParts };
}

/* 60초 회전의 힙(Chromium) — GC 뒤의 JS 힙을 10초마다. 늘면 한 장마다 무언가가 남는다. */
async function measureHeap(page) {
  await openVault(page, 850);
  await fitStage(page);
  return page.evaluate(async ({ spanMs, everyMs }) => {
    const view = document.querySelector(".knowledge-view:not([hidden])");
    const frame = () => new Promise((done) => requestAnimationFrame(done));
    setKnowledgeDimension(view, "3d");
    for (let round = 0; round < 600 && (knowledgeUniverses.get(view)?.frames ?? 0) === 0; round += 1) await frame();
    const universe = knowledgeUniverses.get(view);
    setKnowledgeUniverseMotion(view, true);
    universe.lastTouch = -1e9;
    universe.invalidate();
    const heap = () => {
      globalThis.gc?.();
      return Math.round((performance.memory.usedJSHeapSize / 1048576) * 100) / 100;
    };
    const samples = [heap()];
    const began = performance.now();
    const framesFrom = universe.frames;
    let next = began + everyMs;
    while (performance.now() - began < spanMs) {
      await frame();
      if (performance.now() >= next) {
        samples.push(heap());
        next += everyMs;
      }
    }
    return { samplesMb: samples, frames: universe.frames - framesFrom };
  }, { spanMs: HEAP_MS, everyMs: HEAP_EVERY_MS });
}

const engine = playwright[engineName];
const measured = { engine: engineName, takenAt: new Date().toISOString(), stage: STAGE, rows: [] };
const { browser, page, faults } = await openWindow(engine, { heap: wantHeap });
try {
  measured.renderer = await page.evaluate(() => {
    const gl = document.createElement("canvas").getContext("webgl2");
    if (gl === null) return null;
    const named = gl.getExtension("WEBGL_debug_renderer_info");
    return named === null ? gl.getParameter(gl.RENDERER) : gl.getParameter(named.UNMASKED_RENDERER_WEBGL);
  });
  if (wantHeap) {
    measured.heap = await measureHeap(page);
  } else {
    /* 기계의 1분 부하를 앞뒤로 적는다 — 늦은 장이 부하의 탓인지 가르려고(시안의 표도 부하를 곁에 적었다). */
    for (const pages of sizes) {
      const loadBefore = round(loadavg()[0], 1);
      const row = await measureSize(page, pages);
      measured.rows.push({ ...row, load: [loadBefore, round(loadavg()[0], 1)] });
    }
  }
  measured.faults = faults;
} finally {
  await browser.close();
  files.close();
}
const table = measured.rows.map((row) => ({
  pages: row.pages, load: row.load, edges: row.edges, galaxies: row.galaxies, filaments: row.filaments,
  firstFrameMs: row.firstFrameMs, buildMs: row.buildMs,
  frameWork: row.cost === undefined ? null : `${round(row.cost.medianMs, 1)} · ${round(row.cost.p95Ms, 1)} ms`,
  burstMs: row.cost === undefined ? null : round(row.cost.burstMs),
  draws: row.draws?.calls ?? null,
  spin: row.spin === undefined ? null
    : `gap ${round(row.spin.gapMedianMs, 1)} · ${round(row.spin.gapP95Ms, 1)} ms, late ${row.spin.late}/${row.spin.frames}, JS ${round(row.spin.jsMedianMs, 2)} · ${round(row.spin.jsP95Ms, 2)} ms`,
  framebufferMb: row.memory === undefined ? null : round(row.memory.framebufferMb, 1),
  buffersMb: row.memory === undefined ? null : round(row.memory.buffersMb),
  idleFrames: row.idle?.framesIn3s ?? null,
  closeWork: row.close === undefined ? null : `${round(row.close.medianMs, 1)} · ${round(row.close.p95Ms, 1)} ms`,
  fly: row.fly === undefined ? null : `JS ${round(row.fly.jsMedianMs, 2)} · ${round(row.fly.jsP95Ms, 2)} ms, with GPU ${round(row.fly.medianMs, 1)} · ${round(row.fly.p95Ms, 1)} ms (${row.fly.frames} frames, ${row.fly.names} names)`,
}));
console.log(JSON.stringify({ ...measured, table }, null, 1));
if (out !== null) await writeFile(out, `${JSON.stringify(measured, null, 2)}\n`);
