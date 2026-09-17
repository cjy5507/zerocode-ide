/* 진짜 GPU에서 재는 자 (6차 P2).
 *
 * 게이트의 하네스는 헤드리스 크로미엄에서 돈다. 거기서 WebGL2를 켜면 켜지는 것은
 * 소프트웨어 래스터라이저(SwiftShader)이고, 그 판의 프레임 시간은 기계의 값이지
 * 코드의 값이 아니다 — 그래서 게이트는 **계약**(드로우 수·그린 점·이름표 자리)만
 * 묻는다. 시간은 여기서 잰다.
 *
 * 엔진은 기본이 WebKit이다. 창(ZeroCode)은 macOS에서 Tauri의 WKWebView이고,
 * Playwright의 webkit이 같은 엔진이다 — 설계 §9가 든 위험(「WKWebView의 WebGL2
 * 차이·RGBA32F 필터」)을 물을 수 있는 자리가 여기뿐이다. `--engine chromium`은
 * 창을 띄운 크로미엄(진짜 GPU)으로 같은 것을 잰다.
 *
 *   node ui/tests/knowledge-gpu.mjs [--engine webkit|chromium] [--shots]
 *
 * `--shots`를 주면 dark·light 스크린샷을 docs/design/knowledge-graph-3d-20260916/에
 * 남긴다. 표는 stdout의 JSON이고 같은 폴더의 measurements.json에도 쓴다.
 */
import { createRequire } from "node:module";
import { createServer } from "node:http";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { measureKnowledgeScenes } from "./knowledge-performance.mjs";
import { seedKnowledgeWindow } from "./knowledge-fixture.mjs";
import { measureKnowledgeSupplyScene } from "./knowledge-supply.mjs";

const UI = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const ROOT = resolve(UI, "..");
const SHOTS = join(ROOT, "docs/design/knowledge-graph-3d-20260916");
const require = createRequire(import.meta.url);
const playwright = require("playwright");

const flag = (name, fallback) => {
  const at = process.argv.indexOf(`--${name}`);
  return at < 0 ? fallback : (process.argv[at + 1] ?? "true");
};
const engineName = flag("engine", "webkit");
const wantShots = process.argv.includes("--shots");

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

/* 크로미엄은 창을 띄워야 진짜 GPU를 쓴다(헤드리스는 소프트웨어다). WebKit은
 * 헤드리스에서도 이 기계의 GPU로 합성한다. */
const engine = playwright[engineName];
const browser = await engine.launch(engineName === "chromium"
  ? { headless: false, args: ["--enable-precise-memory-info"] }
  : {});
const page = await browser.newPage({ viewport: { width: 1280, height: 860 } });
const faults = [];
page.on("pageerror", (error) => faults.push(String(error).slice(0, 300)));
await seedKnowledgeWindow(page);
await page.goto(`${origin}/index.html`);
await page.waitForFunction(() => typeof BOUND !== "undefined" && BOUND.size > 0);
await page.evaluate(() => {
  window.__VAULT__ = { pages: 12, linksPer: 2, ghosts: 2, tags: ["core", "reading"] };
  document.getElementById("nav-knowledge").click();
});
await page.waitForFunction(
  () => document.querySelector(".knowledge-view:not([hidden])") !== null,
  null, { timeout: 15000 },
);

const able = await page.evaluate(() => ({
  webgl2: knowledgeGlSupported(),
  renderer: (() => {
    const gl = document.createElement("canvas").getContext("webgl2");
    return gl === null ? null : gl.getParameter(gl.RENDERER);
  })(),
}));

/* 표는 한 줄이 한 손이다. 두 손이 같은 시나리오·같은 판·같은 세션에서 돈다 —
 * 짝지어 잰 값만 견줄 수 있다(볼트의 paint-timings-quantize 규약). */
const rows = [];
const noted = [];
const note = (name, pass, detail) => noted.push({ name, pass: Boolean(pass), detail });
for (const painter of able.webgl2 ? ["svg", "gl", "svg", "gl"] : ["svg", "svg"]) {
  rows.push({ painter, scenes: await measureKnowledgeScenes(page, note, { painter }) });
}
const median = (values) => values.slice().sort((one, two) => one - two)[Math.floor(values.length / 2)];
const table = {};
for (const painter of able.webgl2 ? ["svg", "gl"] : ["svg"]) {
  const runs = rows.filter((row) => row.painter === painter).map((row) => row.scenes);
  table[painter] = runs[0].map((scene, at) => {
    const pick = (key) => median(runs.map((run) => run[at][key]));
    return { name: scene.name, nodes: scene.nodes, edges: scene.edges,
      firstPaintMs: pick("firstPaintMs"), frameP95Ms: pick("frameP95Ms"),
      frameWorstMs: pick("frameWorstMs"), frameMedianMs: pick("frameMedianMs"),
      cameraP95Ms: pick("cameraP95Ms"), cameraMedianMs: pick("cameraMedianMs"),
      draws: scene.draws, labelsShown: scene.labelsShown,
      labelOverlaps: scene.labelOverlaps, discOverlapPairs: scene.discOverlapPairs,
      outsideDisc: scene.outsideDisc, heapMb: pick("heapMb") };
  });
}

/* 공급망 렌즈(P4)의 첫 그림 — 설계 §2 G7: 볼트 모양 장면(359쪽·유령 53)에 천 개의 구성요소, 창의
 * 엔진에서 GL ≤ 300 ms. 판마다 세 번(한 번은 손을 처음 세우는 값) 재어 중앙값을 적고, 표본도 함께 둔다.
 * SVG는 같은 판의 비교 수로 적는다. */
const SUPPLY_FIRST_PAINT_GATE_MS = 300;
const SUPPLY_ROUNDS = 3;
const supply = {};
for (const painter of able.webgl2 ? ["svg", "gl"] : ["svg"]) {
  supply[painter] = await measureKnowledgeSupplyScene(page, note, { painter, rounds: SUPPLY_ROUNDS,
    gate: painter === "gl" ? SUPPLY_FIRST_PAINT_GATE_MS : null });
}

if (wantShots && able.webgl2) {
  await mkdir(SHOTS, { recursive: true });
  for (const theme of ["dark", "light"]) {
    for (const painter of ["gl", "svg"]) {
      await page.setViewportSize({ width: 1998, height: 1069 });
      await page.evaluate(async ({ next, hand }) => {
        setTheme(next);
        knowledgePainterKind = hand;
        const view = document.querySelector(".knowledge-view:not([hidden])");
        window.__VAULT__ = { pages: 359, ghosts: 53, linksPer: 3,
          tags: ["core", "reading", "tools"] };
        knowledgeLayouts.delete(view);
        knowledgeAskedAt = 0;
        await refreshKnowledgeGraph({ force: true });
        await paintKnowledgeView();
        for (let round = 0; round < 600; round += 1) {
          await new Promise((done) => requestAnimationFrame(done));
          if ((knowledgeLayouts.get(view)?.left ?? 1) === 0) break;
        }
      }, { next: theme, hand: painter });
      await page.waitForTimeout(400);
      await page.screenshot({ path: join(SHOTS, `p2-${painter}-${theme}.png`) });
    }
  }
  await page.evaluate(() => setTheme("dark"));
}

const measured = { engine: engineName, renderer: able.renderer, webgl2: able.webgl2,
  takenAt: new Date().toISOString(), rounds: 2, table, supply,
  contracts: noted.filter((row) => !row.pass).length === 0 ? "green" : noted.filter((row) => !row.pass),
  faults };
await mkdir(SHOTS, { recursive: true });
await writeFile(join(SHOTS, `measurements-${engineName}.json`), `${JSON.stringify(measured, null, 2)}\n`);
console.log(JSON.stringify(measured, null, 1));
await browser.close();
files.close();
