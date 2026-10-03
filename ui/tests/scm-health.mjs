import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";

const fixture = JSON.parse(await readFile(new URL("../../fixtures/scm-health.json", import.meta.url), "utf8"));

export async function testScmHealth(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.evaluate((snapshot) => {
      window.__SCM_OBSERVER_HEALTH__ = snapshot;
      activeWorktreePath = snapshot.root;
      folded.aside = false;
      setActivityItem("scm");
    }, fixture);
    const panel = page.locator("#scm-observer-health");
    await panel.waitFor({ state: "visible" });
    const words = await panel.textContent();
    ok("a failed observation shows its cause, last success, failure streak and retry time",
      ["응답 시간 초과", "마지막 성공 기록 없음", "연속 1회 실패", "다음 재시도"].every((part) => words.includes(part)), words);
    ok("health is an accessible status, not a fake successful PR observation",
      await panel.getAttribute("role") === "status" && !words.includes("정상"), words);
    const quiet = await page.evaluate(() => {
      const held = document.querySelector("#scm-observer-health button");
      paintScmObserverHealth();
      return held === document.querySelector("#scm-observer-health button");
    });
    ok("an unchanged health update preserves the retry control", quiet);

    await page.evaluate(() => { window.__SCM_OBSERVER_RETRY_HOLD__ = true; });
    await panel.getByRole("button", { name: "지금 재시도" }).click();
    await page.waitForFunction(() => window.__SCM_OBSERVER_RETRIES__.length === 1);
    ok("manual retry names only the selected checkout and prevents duplicate clicks",
      await panel.getByRole("button").isDisabled()
        && await page.evaluate((root) => window.__SCM_OBSERVER_RETRIES__[0] === root, fixture.root));
    await page.evaluate((snapshot) => {
      activeWorktreePath = "/repos/another";
      paintScmObserverHealth();
      window.__SCM_OBSERVER_RETRY_FINISH__(snapshot);
    }, fixture);
    await page.waitForFunction(() => scmObserverRetryRoot === null);
    ok("a retry response from the previous checkout cannot paint the current one", await panel.count() === 0);

    await page.evaluate((snapshot) => {
      activeWorktreePath = snapshot.root;
      const recovered = snapshot.rows.map((row) => ({ ...row, failure: null, lastSuccessMs: 40,
        lastAttemptMs: 40, failedSinceMs: null, consecutiveFailures: 0, nextRetryMs: null }));
      for (const listener of window.__LISTENERS__["scm:health"] ?? []) listener({ payload: recovered });
    }, fixture);
    ok("a successful observation clears the failure and shows the recovered state",
      (await panel.textContent()).includes("SCM 자동 확인 정상") && await panel.getByRole("button").count() === 0);
    await page.evaluate(async () => {
      window.__FAIL__.add("scm_observer_health");
      await refreshScmObserverHealth();
    });
    ok("a failed state read is unknown rather than a healthy integration",
      (await panel.textContent()).includes("연동 상태를 읽지 못했습니다") && !(await panel.textContent()).includes("정상"));
    await page.setViewportSize({ width: 900, height: 800 });
    ok("health and recovery controls fit the source-control column", await panel.evaluate((node) => node.scrollWidth <= node.clientWidth));
    if (process.env.SCM_HEALTH_SCREENSHOT) await page.screenshot({ path: process.env.SCM_HEALTH_SCREENSHOT });
    ok("SCM health raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1] ?? "")).href) {
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  let failures = 0;
  try {
    await testScmHealth(browser, origin, (name, pass, detail = "") => {
      console.log(`${pass ? "PASS" : "FAIL"} ${name}${!pass && detail ? `\n${detail}` : ""}`);
      if (!pass) failures++;
    });
  } finally {
    await browser.close();
    files.close();
  }
  if (failures) process.exitCode = 1;
}
