import { mkdir } from "node:fs/promises";

export async function testCrashReport(browser, origin, standBackend, ok) {
  const page = await browser.newPage({ viewport: { width: 1280, height: 860 } });
  await standBackend(page);
  await page.addInitScript(() => {
    window.__CRASH_FIXTURE__ = {
      at_ms: 1788690000000, kind: "panic", summary: "pane update failed",
      crumbs_path: "/tmp/diagnostics/crash/crumbs.jsonl", bundle_ready: false,
      crumbs: [{ at_ms: 1788689999800, line: "command enter focus_lane" }, { at_ms: 1788689999890, line: "hook pane=7 Working" }],
      ledger: { workers: 2, dispatches: 1 },
    };
    const base = window.__TAURI__.core.invoke;
    window.__CRASH_CALLS__ = [];
    window.__TAURI__.core.invoke = async (name, args) => {
      if (name === "boot_report") return { ...await base(name, args), last_crash: window.__CRASH_FIXTURE__ };
      if (name === "crash_bundle" || name === "crash_open_log") {
        window.__CRASH_CALLS__.push(name);
        return "/tmp/diagnostics/crash/1788690000000";
      }
      return base(name, args);
    };
  });
  await page.goto(`${origin}/index.html`);
  await page.waitForFunction(() => typeof BOUND !== "undefined" && BOUND.size > 0);
  const sheet = page.locator("#crash-sheet");
  ok("last_crash opens the boot sheet once", await sheet.isVisible());
  if (!await sheet.isVisible()) { await page.close(); return; }
  ok("crash sheet includes its last breadcrumbs", (await sheet.textContent()).includes("focus_lane"));
  if (process.env.CRASH_CAPTURE) {
    await mkdir(process.env.CRASH_CAPTURE, { recursive: true });
    await page.evaluate(() => Promise.all(document.getAnimations().filter(a => a.effect?.getTiming().iterations !== Infinity).map(a => a.finished.catch(() => {}))));
    await page.screenshot({ path: `${process.env.CRASH_CAPTURE}/sheet.png` });
  }
  // t-3014 §2.4: the beat files the incident as a task and tells the open
  // sheet by event; the sheet gains one line through t(), and nothing else moves.
  const filed = page.locator("#crash-filed");
  ok("crash sheet carries no task line before one is filed", await filed.isHidden());
  await page.evaluate(() => noteCrashTriaged({ reportId: "r-1", taskId: "t-7" }));
  ok("a filed crash task is named on the open sheet", await filed.isVisible() && (await filed.textContent()).includes("t-7"));
  ok("the filed line is spoken through the catalog, not a literal", await page.evaluate(() => {
    const line = document.getElementById("crash-filed").textContent;
    return line === t("crash.filed", "", { task: "t-7" }) && line.includes("t-7");
  }));
  ok("the filed line has ko/en/ja/zh/es translations", await page.evaluate(() => ["ko", "en", "ja", "zh", "es"].every(code => (CATALOG[code]["crash.filed"] ?? "").includes("{{task}}"))));
  ok("the crash sheet has no native title attribute", await page.evaluate(() => !document.querySelector("#crash-sheet [title]")));
  await page.locator("#crash-bundle").click();
  await page.waitForFunction(() => window.__CRASH_CALLS__.includes("crash_bundle"));
  ok("bundle button invokes the one folder door once", await page.evaluate(() => __CRASH_CALLS__.filter(n => n === "crash_bundle").length === 1));
  await page.locator("#crash-open-log").click();
  ok("log button opens the fixed log door", await page.evaluate(() => __CRASH_CALLS__.includes("crash_open_log")));
  await page.locator("#crash-close").click();
  await page.evaluate(() => showLastCrash(window.__CRASH_FIXTURE__));
  ok("closed crash sheet does not reopen on a repeated boot result", !await sheet.isVisible());
  await page.evaluate(() => {
    showHangBadge({ ms: 6000, count: 1 });
    showHangBadge({ ms: 6000, count: 1 });
  });
  ok("second threshold paints one status badge", await page.locator("#sb-hang").count() === 1 && await page.locator("#sb-hang").isVisible());
  ok("hang badge has en/ja/zh/es translations", await page.evaluate(() => ["en", "ja", "zh", "es"].every(code => Boolean(CATALOG[code]["crash.hangBadge"]))));
  const pixels = await page.locator("#sb-hang").evaluate(node => ({ text: node.textContent, width: node.getBoundingClientRect().width, x: node.getBoundingClientRect().x, color: getComputedStyle(node).color }));
  ok("hang badge contains its duration and has room to read it", pixels.text.includes("6") && pixels.width > 100, JSON.stringify(pixels));
  if (process.env.CRASH_CAPTURE) await page.screenshot({ path: `${process.env.CRASH_CAPTURE}/badge.png` });
  await page.close();
}
