/* What the docked session history costs the window to draw (t-103659) — the numbers its list is held to.
 *
 * The scene is one answer of `vault_sessions` the size a working machine gets: 200 sessions in 24 groups, three
 * preview lines each, one session in ten with helpers under it. The backend is the harness's stub, so what is read
 * here is the window's own share: the cards it builds, the elements it leaves under the list, and the JS, style and
 * layout time of each gesture, read from the engine's own counters.
 *
 * Gestures, each on a fresh answer:
 *   open       the activity tab pressed and the first answer drawn
 *   again      the panel left and come back to
 *   keystroke  one letter typed into the search (the stub answers the same 200 — the worst case of a wide query)
 *   toggle     one card pressed open
 *   refresh    the reload button
 *
 * Numbers per gesture: `asked` (calls to the backend), `cardsBuilt` (calls that build a card), `cards` and `elements` (what
 * stands under the list afterwards), and `ms` (script + style + layout). The counts are the same on any machine and
 * are what the `vault-dock` suite holds; the times are recorded with the load they were read under.
 *
 * A before is another tree's `ui/` with this file copied in (`git archive <base> ui | tar -x`).
 *
 *   node ui/tests/vault-dock-perf.mjs [--rounds 5] [--sessions 200] [--json out.json]
 *   WINDOW_CPU_THROTTLE=4 node ui/tests/vault-dock-perf.mjs          low spec (cpu ×4) */
import { writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";
import { median } from "./coordinator-desk-perf.mjs";
import { loadNote } from "./machine-load.mjs";

/* The answer's shape: how many sessions, in how many groups, and how often a session has helpers. */
export const VAULT_DOCK_SCENE = Object.freeze({ sessions: 200, groups: 24, childEvery: 10, children: 3, previewChars: 240 });

/* One `vault_sessions` answer of `scene`'s size, stood in the page as the stub's answer. Every word is made up. */
export async function standVaultDockScene(page, scene = VAULT_DOCK_SCENE) {
  await page.evaluate((scene) => {
    const line = (seed, chars) => `${seed} ${"세션 기록 줄을 채우는 지어낸 글 ".repeat(Math.ceil(chars / 18))}`.slice(0, chars);
    const card = (n, extra = {}) => ({
      id: `claude:fixture-${n}:/store/fixture-${n}.jsonl`, agent: n % 4 === 3 ? "codex" : "claude",
      session_id: `fixture-${n}`, title: `지어낸 세션 ${n} — 목록을 재는 제목`, cwd: `/work/project-${n % scene.groups}`,
      branch: n % 3 === 0 ? `wt/fixture-${n}` : null, model: "model-x", file_path: `/store/fixture-${n}.jsonl`,
      codex_home: null, created_at: "2026-11-11T00:00:00.000Z", updated_at: "2026-11-11T00:00:00.000Z",
      modified_at: "2026-11-11T00:00:00.000Z", message_count: 12 + n,
      preview: [
        { role: "user", text: line(`물음 ${n}`, scene.previewChars) },
        { role: "assistant", text: line(`답 ${n}`, scene.previewChars) },
        { role: "user", text: line(`다음 물음 ${n}`, scene.previewChars) },
      ],
      resume: `claude --resume 'fixture-${n}'`, children: [], depth: 0, parent_id: null, origin: null, ...extra,
    });
    const groups = Array.from({ length: scene.groups }, (_, at) => ({ key: `/work/project-${at}`, label: `work/project-${at}`, sessions: [] }));
    for (let n = 0; n < scene.sessions; n += 1) {
      const row = card(n);
      if (n % scene.childEvery === 0) {
        row.children = Array.from({ length: scene.children }, (_, at) => card(`${n}-${at}`, {
          depth: 1, parent_id: row.id, resume: null, preview: [{ role: "assistant", text: line(`도우미 ${at}`, 80) }],
        }));
      }
      groups[n % scene.groups].sessions.push(row);
    }
    window.__VAULT_LIMIT_ROWS__ = undefined;
    window.__VAULT__ = {
      groups, agents: [{ slug: "claude", label: "Claude", sessions: 150 }, { slug: "codex", label: "Codex", sessions: 50 }],
      issues: [], shown: scene.sessions, total: 1000, matched: 1000, truncated: true, hidden: { empty: 0, window_made: 0, measured: 0 }, refreshing: false,
      limits: { choices: [50, 100, 200], absolute_max: 1000, default: 200, children_per_parent: 50 },
    };
    vaultSessionLimit = scene.sessions;
  }, scene);
}

/* One gesture, measured: `run` is evaluated in the page between two readings of the engine's counters, with every
 * card the list gains counted and every ask of the backend counted. */
export async function measureVaultDockGesture(page, cdp, run) {
  const counters = async () => {
    const { metrics } = await cdp.send("Performance.getMetrics");
    const of = (name) => metrics.find((one) => one.name === name)?.value ?? 0;
    return { layout: of("LayoutDuration") * 1000, style: of("RecalcStyleDuration") * 1000, script: of("ScriptDuration") * 1000 };
  };
  await page.evaluate(() => {
    // Every card the window builds, counted where it is built. (Counting the nodes the list gains would count a
    // card that was kept and only moved to its place in the new layout.)
    if (!window.__VDOCK_BUILDER__) {
      window.__VDOCK_BUILDER__ = window.vaultDockCard;
      window.vaultDockCard = function countedVaultDockCard(...args) {
        window.__VDOCK_BUILT__ += 1;
        return window.__VDOCK_BUILDER__.apply(this, args);
      };
    }
    window.__VDOCK_BUILT__ = 0;
    // The harness counts every ask by its command (`__COUNTS__`, window-boot).
    window.__VDOCK_ASKED_FROM__ = window.__COUNTS__.vault_sessions ?? 0;
  });
  const before = await counters();
  await page.evaluate(run);
  // Two frames, so the style and layout the gesture left owing are in the reading.
  await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
  const after = await counters();
  const seen = await page.evaluate(() => {
    const list = document.getElementById("vdock-list");
    return {
      asked: (window.__COUNTS__.vault_sessions ?? 0) - window.__VDOCK_ASKED_FROM__, cardsBuilt: window.__VDOCK_BUILT__,
      cards: list.querySelectorAll(".vdock-card").length, elements: list.querySelectorAll("*").length,
    };
  });
  const parts = { script: after.script - before.script, style: after.style - before.style, layout: after.layout - before.layout };
  return { ...seen, ms: parts.script + parts.style + parts.layout, ...parts };
}

/* The five gestures, in the order a person makes them. */
export const VAULT_DOCK_GESTURES = Object.freeze({
  open: async () => {
    document.getElementById("activity-vault-tab").click();
    await new Promise((done) => setTimeout(done, 60));
  },
  again: async () => {
    document.getElementById("activity-files-tab").click();
    await new Promise((done) => setTimeout(done, 30));
    document.getElementById("activity-vault-tab").click();
    await new Promise((done) => setTimeout(done, 60));
  },
  keystroke: async () => {
    const box = document.getElementById("vdock-query");
    box.value = "a";
    box.dispatchEvent(new Event("input", { bubbles: true }));
    await new Promise((done) => setTimeout(done, 60));
  },
  toggle: async () => {
    document.querySelector("#vdock-list .vdock-card")?.click();
    await new Promise((done) => setTimeout(done, 60));
  },
  refresh: async () => {
    document.getElementById("vdock-refresh").click();
    await new Promise((done) => setTimeout(done, 60));
  },
});

async function main() {
  const flag = (name, fallback) => {
    const at = process.argv.indexOf(name);
    return at === -1 ? fallback : process.argv[at + 1];
  };
  const rounds = Number(flag("--rounds", 5));
  const scene = { ...VAULT_DOCK_SCENE, sessions: Number(flag("--sessions", VAULT_DOCK_SCENE.sessions)) };
  const { origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  const samples = Object.fromEntries(Object.keys(VAULT_DOCK_GESTURES).map((name) => [name, []]));
  for (let round = 0; round < rounds; round += 1) {
    const { page, faults } = await openWindowTestPage(browser, origin);
    await page.setViewportSize({ width: 1600, height: 900 });
    const cdp = await page.context().newCDPSession(page);
    await cdp.send("Performance.enable");
    const throttle = Number(process.env.WINDOW_CPU_THROTTLE ?? 1);
    if (throttle > 1) await cdp.send("Emulation.setCPUThrottlingRate", { rate: throttle });
    await standVaultDockScene(page, scene);
    for (const [name, run] of Object.entries(VAULT_DOCK_GESTURES)) {
      samples[name].push(await measureVaultDockGesture(page, cdp, run));
    }
    await page.close();
    if (faults.length) throw new Error(faults.join("\n"));
  }
  await browser.close();
  const report = { scene, rounds, throttle: Number(process.env.WINDOW_CPU_THROTTLE ?? 1), load: loadNote() };
  for (const [name, runs] of Object.entries(samples)) {
    const mid = (key) => median(runs.map((one) => one[key]));
    report[name] = {
      asked: mid("asked"), cardsBuilt: mid("cardsBuilt"), cards: mid("cards"), elements: mid("elements"),
      ms: Number(mid("ms").toFixed(1)), script: Number(mid("script").toFixed(1)),
      style: Number(mid("style").toFixed(1)), layout: Number(mid("layout").toFixed(1)),
    };
  }
  const out = flag("--json", null);
  if (out) writeFileSync(resolve(out), `${JSON.stringify(report, null, 2)}\n`);
  console.log(JSON.stringify(report, null, 2));
  process.exit(0);
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) await main();
