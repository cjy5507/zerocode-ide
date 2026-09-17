import { pathToFileURL } from "node:url";
import { resolve } from "node:path";
import { mkdir } from "node:fs/promises";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";

export async function testCoordinatorPanel(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.evaluate(async () => {
      const term = await openTermTab({ placement: "tab" });
      paneAgents.set(term, "codex");
      window.__COORD_TERM__ = term;
      window.__COORD_CALLS__ = [];
      window.__COORD_EFFECTS__ = 0;
      window.__COORD_KEYS__ = new Set();
      window.__COORD_STATUS__ = { runId: "run-a", coordinator: { generation: 4, held: true, seat: "old/%1" },
        policy: null, quotaSupported: true, eligiblePanes: [{ term: 999, seat: "other/%1", agent: "codex" }], receipts: [] };
      window.__ANSWER__.coordinator_seat_runs = () => ({ runs: [{ runId: "run-a", name: "UI 개선" }, { runId: "run-b", name: "회귀 검증" }] });
      window.__ANSWER__.coordinator_handover_status = ({ runId }) => structuredClone({ ...window.__COORD_STATUS__, runId });
      window.__ANSWER__.set_coordinator_handover = (args) => {
        window.__COORD_CALLS__.push({ action: "policy", ...args });
        window.__COORD_STATUS__.policy = args.targetTerm === null ? null : { status: "armed", targetTerm: args.targetTerm };
        return window.__COORD_STATUS__;
      };
      window.__ANSWER__.claim_coordinator_seat = (args) => {
        window.__COORD_CALLS__.push({ action: "claim", ...args });
        if (!window.__COORD_KEYS__.has(args.retryRequest)) {
          window.__COORD_KEYS__.add(args.retryRequest);
          window.__COORD_EFFECTS__++;
          window.__COORD_STATUS__.coordinator.generation++;
          throw new Error("fixture: reply was lost");
        }
        return { runId: args.runId, seated: true, moved: true, generation: 5, coordinator: window.__COORD_STATUS__.coordinator };
      };
      openTermMenu(100, 100, term);
    });
    await page.locator(".term-menu-row").filter({ hasText: "코디네이터와 인계" }).click();
    await page.waitForFunction(() => !document.getElementById("coordinator-claim").disabled);
    ok("the pane menu opens a run picker and the existing modal focus contract",
      await page.locator("#coordinator-scrim").isVisible() && await page.locator("#coordinator-run option").count() === 2 &&
      await page.evaluate(() => modalTop()?.root.id === "coordinator-scrim"));

    await page.click("#coordinator-claim");
    await page.waitForFunction(() => document.getElementById("coordinator-result").textContent.includes("reply was lost"));
    await page.click("#coordinator-claim");
    await page.waitForFunction(() => document.getElementById("coordinator-result").textContent.includes("맡았습니다"));
    const replay = await page.evaluate(() => ({ calls: window.__COORD_CALLS__.filter((call) => call.action === "claim"),
      effects: window.__COORD_EFFECTS__, term: window.__COORD_TERM__ }));
    ok("a lost claim reply retries the same intent and observed generation for the selected pane",
      replay.calls.length === 2 && replay.effects === 1 && replay.calls[0].retryRequest === replay.calls[1].retryRequest &&
      replay.calls.every((call) => call.targetTerm === replay.term && call.expectedGeneration === 4 && call.runId === "run-a"), JSON.stringify(replay));

    await page.selectOption("#coordinator-target", "999");
    await page.click("#coordinator-save");
    await page.waitForFunction(() => document.getElementById("coordinator-policy").textContent.includes("예약"));
    const policy = await page.evaluate(() => window.__COORD_CALLS__.find((call) => call.action === "policy"));
    ok("automatic handover submits only a selected terminal and ledger generation",
      policy.targetTerm === 999 && policy.expectedGeneration === 5 && !Object.hasOwn(policy, "capability") && !Object.hasOwn(policy, "witness"), JSON.stringify(policy));

    await page.evaluate(() => { window.__COORD_STATUS__.quotaSupported = false; });
    await page.click("#coordinator-refresh");
    await page.waitForFunction(() => !coordinatorPanel.busy);
    ok("an unmeasured quota cannot arm an automatic transfer", await page.locator("#coordinator-save").isDisabled());
    await page.selectOption("#coordinator-target", "");
    ok("disabling an existing order remains available without quota readings", await page.locator("#coordinator-save").isEnabled());

    const stale = await page.evaluate(async () => {
      let finishOld;
      window.__ANSWER__.coordinator_handover_status = ({ runId }) => runId === "run-b"
        ? new Promise((done) => { finishOld = done; })
        : { ...window.__COORD_STATUS__, runId };
      document.getElementById("coordinator-run").value = "run-b";
      const old = loadCoordinatorStatus(coordinatorPanel);
      document.getElementById("coordinator-run").value = "run-a";
      await loadCoordinatorStatus(coordinatorPanel);
      finishOld({ ...window.__COORD_STATUS__, runId: "run-b", coordinator: { generation: 99 } });
      await old;
      return { run: coordinatorPanel.status.runId, generation: coordinatorPanel.status.coordinator.generation };
    });
    ok("a late status reply cannot replace the selected run", stale.run === "run-a" && stale.generation === 5, JSON.stringify(stale));
    await mkdir("output/playwright/coordinator", { recursive: true });
    for (const width of [1280, 360]) {
      await page.setViewportSize({ width, height: 1000 });
      const fits = await page.locator(".coordinator-panel").evaluate((node) => {
        const rect = node.getBoundingClientRect();
        return node.scrollWidth <= node.clientWidth + 1 && rect.left >= 0 && rect.right <= innerWidth;
      });
      ok(`coordinator controls fit ${width}px`, fits);
      await page.screenshot({ path: `output/playwright/coordinator/${width}.png` });
    }
    await page.keyboard.press("Escape");
    ok("Escape closes the coordinator dialog through the modal stack", await page.locator("#coordinator-scrim").isHidden() && await page.evaluate(() => coordinatorPanel === null));
    ok("coordinator controls raised no page errors", faults.length === 0, faults.join("\n"));
  } finally { await page.close(); }
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  let failed = 0;
  try { await testCoordinatorPanel(browser, origin, (name, pass, details = "") => {
    console.log(`${pass ? "PASS" : "FAIL"} ${name}${!pass ? `\n${details}` : ""}`);
    if (!pass) failed++;
  }); } finally { await browser.close(); files.close(); }
  if (failed) process.exitCode = 1;
}
