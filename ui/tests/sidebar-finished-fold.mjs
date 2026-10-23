import { openWindowTestPage } from "./window-boot.mjs";
import { mkdir } from "node:fs/promises";
import { join } from "node:path";
import { createRequire } from "node:module";

export async function testSidebarFinishedFold({ browser, origin, ok, faults }) {
  const { page } = await openWindowTestPage(browser, origin, { faults });
  const axeModule = createRequire(import.meta.url)("@axe-core/playwright");
  const AxeBuilder = axeModule.default ?? axeModule;
  try {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await page.setViewportSize({ width: 1280, height: 1600 });
    await page.waitForFunction(() => projectsRead);
    await page.evaluate(async () => {
      setLocale("ko", { persist: false });
      setEveryProjectClosed(false);
      const names = ["finished-1", "finished-2", "finished-3", "finished-4", "failed", "active", "busy", "unreported", "shell", "asking"];
      window.__LEDGER__ = names.map((name, index) => ({
        run: "run-1", worker: `w-${index}`, agent: "claude", state: "released", ledger: "released",
        checkout: `/r/${name}`, task: name, task_id: `t-${index}`, settled: true,
        reported: name !== "unreported", failed: name === "failed", asking: name === "asking", at: 1,
        review: { verified: true, merged: false, written: true, author: "coordinator" },
        progress: { stage: "verified", source: "a".repeat(40) },
      }));
      window.__FINISHED_ACTIVE__ = "/r/active";
      window.__ANSWER__.project_catalog = () => [{
        name: "r", path: "/r", worktrees: names.map((name, index) => ({
          path: `/r/${name}`, branch: `wt/t-${index}`, task_id: `t-${index}`, is_main: false,
          active: `/r/${name}` === window.__FINISHED_ACTIVE__, is_folder: false,
          ownership: "zerocode-managed", external_hidden: false,
          landing: { state: "unlanded", ahead: 1, dirty: false, compare_ref: "origin/main" },
        })),
      }];
      tabs.push({ kind: "term", worktree: "/r/busy", id: "tab-98650", layout: { type: "leaf", term: 98650 } });
      tabs.push({ kind: "term", worktree: "/r/shell", id: "tab-98651", layout: { type: "leaf", term: 98651 } });
      paneAgents.set(98650, "claude");
      for (const listener of window.__LISTENERS__["hook:agent"] ?? []) {
        listener({ payload: { term: 98650, state: "working", agent: "claude", session: "s-98650" } });
      }
      for (const listener of window.__LISTENERS__["ledger:changed"] ?? []) listener({ payload: 9 });
      for (let attempt = 0; attempt < 80 && checkoutLedger.size < names.length; attempt += 1) {
        await new Promise((resolve) => setTimeout(resolve, 25));
      }
      setSidebarView({ groupBy: "repo" });
      await refreshWorktrees();
      await window.__PAINTED__();
      window.__FINISHED_SHOWN__ = () => [...document.querySelectorAll(".wt-row[data-worktree-path]")]
        .map((node) => node.dataset.worktreePath).filter((path) => path.startsWith("/r/"));
    });
    const before = await page.evaluate(() => ({
      shown: window.__FINISHED_SHOWN__(),
      folds: [...document.querySelectorAll(".wt-landing-fold")].map((node) => ({ text: node.textContent, expanded: node.getAttribute("aria-expanded") })),
    }));
    const folded = before.folds.length === 1 && before.folds[0].text.includes("4")
      && before.folds[0].expanded === "false"
      && !before.shown.some((path) => path.startsWith("/r/finished-"));
    ok("finished unlanded cards fold into one accessible pending-landing row", folded, JSON.stringify(before));
    if (!folded) return;
    ok("failed, active, live, unreported, shell and asking work stays visible", ["failed", "active", "busy", "unreported", "shell", "asking"].every((name) => before.shown.includes(`/r/${name}`)), JSON.stringify(before.shown));
    const awaiting = await page.evaluate(async () => {
      for (const [name, claimed] of [["finished-2", false], ["finished-3", true]]) {
        const row = window.__LEDGER__.find((one) => one.checkout === `/r/${name}`);
        row.review = { verified: false, merged: false, written: false, claimed_verified: claimed, author: "worker" };
        row.progress = { stage: "report_only", ...(claimed ? { claimed: "verified" } : {}) };
      }
      for (const listener of window.__LISTENERS__["ledger:changed"] ?? []) listener({ payload: 10 });
      while (paneLedgerAsking || ledgerAgentsPending) await new Promise((resolve) => setTimeout(resolve, 10));
      await refreshWorktrees();
      await window.__PAINTED__();
      return window.__FINISHED_SHOWN__();
    });
    ok("a report awaiting review and a worker's unverified claim remain visible without opening the fold",
      ["finished-2", "finished-3"].every((name) => awaiting.includes(`/r/${name}`)), JSON.stringify(awaiting));
    await page.evaluate(async () => {
      for (const row of window.__LEDGER__.filter((one) => ["/r/finished-2", "/r/finished-3"].includes(one.checkout))) {
        row.review = { verified: true, merged: false, written: true, author: "coordinator" };
        row.progress = { stage: "verified", source: "a".repeat(40) };
      }
      for (const listener of window.__LISTENERS__["ledger:changed"] ?? []) listener({ payload: 11 });
      while (paneLedgerAsking || ledgerAgentsPending) await new Promise((resolve) => setTimeout(resolve, 10));
      await refreshWorktrees();
      await window.__PAINTED__();
    });
    await page.locator(".wt-landing-fold").click();
    await page.waitForFunction(() => window.__FINISHED_SHOWN__().filter((path) => path.startsWith("/r/finished-")).length === 4);
    const expanded = await page.locator(".wt-landing-fold").getAttribute("aria-expanded");
    ok("the pending-landing row reveals every preserved checkout", expanded === "true", expanded);
    await page.locator(".wt-landing-fold").click();
    await page.waitForFunction(() => !window.__FINISHED_SHOWN__().some((path) => path.startsWith("/r/finished-")));
    await page.evaluate(async () => { window.__FINISHED_ACTIVE__ = "/r/finished-1"; await refreshWorktrees(); });
    ok("selecting a folded checkout immediately makes it visible", await page.evaluate(() => window.__FINISHED_SHOWN__().includes("/r/finished-1")));
    await page.evaluate(async () => { setSidebarView({ groupBy: "none" }); await refreshWorktrees(); });
    ok("flat lists keep all checkouts reachable without a project fold", await page.evaluate(() => window.__FINISHED_SHOWN__().length === 10));
    await page.evaluate(async () => { window.__FINISHED_ACTIVE__ = "/r/active"; setSidebarView({ groupBy: "repo" }); await refreshWorktrees(); });
    const shots = process.env.WORKER_FINISH_SHOTS;
    if (shots) await mkdir(shots, { recursive: true });
    for (const treatment of ["dark", "light"]) {
      await page.setViewportSize({ width: 1280, height: 1600 });
      await page.evaluate((value) => { theme = value; applyTheme(); }, treatment);
      const audit = await new AxeBuilder({ page }).include(".wt-landing-fold").withTags(["wcag2a", "wcag2aa"]).analyze();
      ok(`the pending-landing control is accessible in ${treatment}`, audit.violations.length === 0,
        JSON.stringify(audit.violations.map((one) => ({ id: one.id, nodes: one.nodes.map((node) => node.target) }))));
      if (shots) for (const width of [1280, 320]) {
        await page.setViewportSize({ width, height: 900 });
        await page.screenshot({ path: join(shots, `finished-${treatment}-${width}.png`), animations: "disabled" });
      }
    }
  } finally {
    await page.close();
  }
}
