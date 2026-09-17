import { pathToFileURL } from "node:url";
import { resolve } from "node:path";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";

export async function testBrowserRecovery(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.evaluate(async () => {
      window.__RECOVERY_NAV__ = [];
      window.__RECOVERY_PLACE__ = [];
      window.__ANSWER__.browser_navigate = (args) => { window.__RECOVERY_NAV__.push(args); return null; };
      window.__ANSWER__.browser_place = (args) => { window.__RECOVERY_PLACE__.push(args); return null; };
      const label = await openBrowserTab("http://127.0.0.1:54321/");
      window.__RECOVERY_TAB__ = tabs.find((tab) => tab.kind === "browser" && tab.label === label);
      window.__RECOVERY_EVENT__ = (state, url = window.__RECOVERY_TAB__.url) => {
        for (const handler of window.__LISTENERS__["browser:nav"] ?? []) handler({ payload: { label, url, state } });
      };
      window.__RECOVERY_EVENT__("dead");
      await browserPlacing;
    });
    await page.waitForSelector(".browser-failure");
    const first = await page.evaluate(() => ({ failure: !!window.__RECOVERY_TAB__.navFailure,
      hiddenNative: window.__RECOVERY_PLACE__.at(-1)?.shown === false,
      url: document.querySelector(".browser-failure-url").textContent,
      first: browserRetryDelay(window.__RECOVERY_TAB__) }));
    ok("a failed navigation replaces the native white surface with the recorded URL and recovery actions",
      first.failure && first.hiddenNative && first.url === "http://127.0.0.1:54321/" && first.first === BROWSER_FIRST_DELAY,
      JSON.stringify(first));
    await page.waitForFunction(() => window.__RECOVERY_NAV__.length > 0);
    const retry = await page.evaluate(async () => {
      window.__RECOVERY_EVENT__("started");
      await browserPlacing;
      return { calls: window.__RECOVERY_NAV__.length, failure: !!window.__RECOVERY_TAB__.navFailure,
        hiddenNative: window.__RECOVERY_PLACE__.at(-1)?.shown === false,
        delay: browserRetryDelay(window.__RECOVERY_TAB__) };
    });
    ok("automatic retry keeps the recovery surface visible and increases its delay",
      retry.calls === 1 && retry.failure && retry.hiddenNative && retry.delay > first.first, JSON.stringify(retry));
    const recovered = await page.evaluate(async () => {
      window.__RECOVERY_EVENT__("finished");
      await browserPlacing;
      return { failure: window.__RECOVERY_TAB__.navFailure, timer: window.__RECOVERY_TAB__.retryTimer,
        native: window.__RECOVERY_PLACE__.at(-1)?.shown };
    });
    ok("successful navigation stops retries and returns the native page", !recovered.failure && recovered.timer === null && recovered.native, JSON.stringify(recovered));
    const boardCover = await page.evaluate(async () => {
      setWorkspaceBoardOpen(true);
      await browserPlacing;
      const hidden = window.__RECOVERY_PLACE__.at(-1)?.shown === false;
      setWorkspaceBoardOpen(false);
      await browserPlacing;
      return { hidden, restored: window.__RECOVERY_PLACE__.at(-1)?.shown === true };
    });
    ok("the workspace board covers the native browser and restores it on close", boardCover.hidden && boardCover.restored, JSON.stringify(boardCover));
    const stale = await page.evaluate(() => {
      const tab = window.__RECOVERY_TAB__;
      tab.url = "http://127.0.0.1:54322/";
      window.__RECOVERY_EVENT__("dead", "http://127.0.0.1:54321/");
      return { url: tab.url, failure: tab.navFailure };
    });
    ok("a late failure cannot overwrite a newer address", stale.url.endsWith(":54322/") && !stale.failure, JSON.stringify(stale));
    await page.evaluate(() => window.__RECOVERY_EVENT__("dead"));
    await page.click(".browser-failure-auto");
    const paused = await page.evaluate(() => {
      const tab = window.__RECOVERY_TAB__;
      return { paused: tab.navFailure.paused, timer: tab.retryTimer };
    });
    ok("automatic retries can be stopped", paused.paused && paused.timer === null, JSON.stringify(paused));
    const closed = await page.evaluate(() => {
      const tab = window.__RECOVERY_TAB__;
      tab.loading = false;
      tab.navFailure.paused = false;
      tab.navFailure.attempts = 100;
      const capped = browserRetryDelay(tab) === BROWSER_RETRY_POLICY.maxMs;
      scheduleBrowserRetry(tab);
      const scheduled = tab.retryTimer != null;
      dropTab(tab.id);
      return { capped, scheduled, cancelled: tab.retryTimer === null };
    });
    ok("retry delay is capped and closing the tab cancels its timer", closed.capped && closed.scheduled && closed.cancelled, JSON.stringify(closed));
    ok("browser recovery raised no page errors", faults.length === 0, faults.join("\n"));
  } finally { await page.close(); }
}

// Read the production bound rather than maintaining a second timing table.
import { readFile } from "node:fs/promises";
const BROWSER_FIRST_DELAY = Number((await readFile(new URL("../shell-browser.js", import.meta.url), "utf8"))
  .match(/BROWSER_RETRY_POLICY = Object\.freeze\(\{ firstMs: (\d+)/)?.[1]);

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  let failed = 0;
  try { await testBrowserRecovery(browser, origin, (name, pass, details = "") => {
    console.log(`${pass ? "PASS" : "FAIL"} ${name}${!pass ? `\n${details}` : ""}`);
    if (!pass) failed++;
  }); } finally { await browser.close(); files.close(); }
  if (failed) process.exitCode = 1;
}
