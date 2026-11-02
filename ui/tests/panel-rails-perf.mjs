import { chromium, createWindowServer, openWindowTestPage, standRails } from "./window-boot.mjs";
import { standRailFixture } from "./panel-rails.mjs";

/* What dragging the window's edge costs, with the rails and without (t-84803): the page's main-thread work between two
 * frames at each of 242 widths, 1600 to 390 px and back in 10 px steps, read from the engine's own counters. Without
 * means their tokens at 0px, which are the rules this window had before them. Run it on a quiet machine.
 *
 *   node ui/tests/panel-rails-perf.mjs                   as it is
 *   WINDOW_CPU_THROTTLE=4 node ui/tests/panel-rails-perf.mjs   the low-spec profile's cpu4x */

const BUDGET_MS = 1000 / 60;
const { origin } = await createWindowServer();
const browser = await chromium.launch({ headless: true });

async function measure(rails) {
  const { page, faults } = await openWindowTestPage(browser, origin, { context: { reducedMotion: "no-preference" } });
  await page.setViewportSize({ width: 1600, height: 800 });
  await standRailFixture(page);
  if (rails) await standRails(page);
  const cdp = await page.context().newCDPSession(page);
  await cdp.send("Performance.enable");
  const counters = async () => {
    const { metrics } = await cdp.send("Performance.getMetrics");
    const of = (name) => metrics.find((one) => one.name === name)?.value ?? 0;
    return (of("LayoutDuration") + of("RecalcStyleDuration") + of("ScriptDuration")) * 1000;
  };
  const steps = [];
  const widths = [];
  for (let px = 1600; px >= 390; px -= 10) widths.push(px);
  for (let px = 390; px <= 1600; px += 10) widths.push(px);
  await page.evaluate(() => new Promise((done) => setTimeout(done, 400)));
  for (const px of widths) {
    const before = await counters();
    await page.setViewportSize({ width: px, height: 800 });
    await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
    steps.push({ px, ms: await counters() - before });
  }
  await page.close();
  if (faults.length) throw new Error(faults.join("\n"));
  const sorted = steps.map((step) => step.ms).sort((a, b) => a - b);
  const at = (share) => sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * share))];
  return { steps: steps.length, p50: at(0.5), p95: at(0.95), max: sorted.at(-1), over: sorted.filter((ms) => ms > BUDGET_MS).length,
    slowest: [...steps].sort((a, b) => b.ms - a.ms).slice(0, 5).map((step) => `${step.px}px ${step.ms.toFixed(1)}ms`) };
}

const before = await measure(false);
const after = await measure(true);
await browser.close();
console.log(JSON.stringify({ throttle: Number(process.env.WINDOW_CPU_THROTTLE ?? 1), budgetMs: BUDGET_MS, before, after }, null, 2));
