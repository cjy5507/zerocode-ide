import { pathToFileURL } from "node:url";
import { resolve } from "node:path";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";

export async function testStartupProjects(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.waitForFunction(() => startupProjectFoldsReady && projectsRead);
    const cold = await page.evaluate(async () => {
      const states = ["working", "needs-attention", "done", "idle"];
      const catalog = states.map((state) => ({ name: state, path: `/restart/${state}`,
        worktrees: [{ path: `/restart/${state}`, branch: "main", is_main: true, is_folder: false, active: state === "working" }] }));
      window.__ANSWER__.project_catalog = () => catalog;
      sidebarGroupBy = "repo";
      startupProjectFoldsReady = false;
      startupFoldProjects.clear(); closedProjects.clear(); projectsRead = false;
      await refreshWorktrees();
      states.forEach((state, index) => {
        const term = 9701 + index;
        mountTermTab(term, { agent: "codex", worktree: `/restart/${state}` }, { placement: "tab", focus: false });
        paneAgents.set(term, "codex"); hookStates.set(term, state);
      });
      paintWorktreeAgents();
      return { closed: [...closedProjects].sort(), headers: document.querySelectorAll(".proj-row[data-project-path]").length };
    });
    ok("startup keeps project headers and initially folds their contents while live state is being restored",
      cold.closed.length === 4 && cold.headers === 4, JSON.stringify(cold));
    await page.evaluate(() => { startupProjectFoldsReady = true; paintWorktreeAgents(); });
    await page.waitForFunction(() => document.querySelector('.proj[data-project-path="/restart/working"] .wt-row'));
    const restored = await page.evaluate(() => ({
      open: [...document.querySelectorAll(".proj[data-project-path]")]
        .filter((node) => !node.querySelector(".proj-children").hidden).map((node) => node.dataset.projectPath).sort(),
      collapsedRows: document.querySelectorAll('.proj[data-project-path="/restart/done"] .wt-row, .proj[data-project-path="/restart/idle"] .wt-row').length,
    }));
    ok("only working and attention projects expand; ended and idle projects remain like a folded folder",
      restored.open.join() === "/restart/needs-attention,/restart/working" && restored.collapsedRows === 0, JSON.stringify(restored));
    await page.click('.proj-row[data-project-path="/restart/done"]');
    await page.waitForSelector('.proj[data-project-path="/restart/done"] .wt-row');
    await page.click('.proj-row[data-project-path="/restart/working"]');
    await page.evaluate(async () => { paintWorktreeAgents(); await refreshWorktrees(); });
    const choices = await page.evaluate(() => ({ doneOpen: !closedProjects.has("/restart/done"),
      workingClosed: closedProjects.has("/restart/working"), retained: projects.length }));
    ok("manual opening and closing wins over later live updates without removing projects",
      choices.doneOpen && choices.workingClosed && choices.retained === 4, JSON.stringify(choices));
    await page.evaluate(() => { hookStates.set(9704, "working"); paintWorktreeAgents(); });
    await page.waitForSelector('.proj[data-project-path="/restart/idle"] .wt-row');
    ok("a late live restore can reveal a project the person has not folded manually",
      await page.locator('.proj[data-project-path="/restart/idle"] .proj-children').isVisible());
    ok("startup project folding raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally { await page.close(); }
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  let failures = 0;
  try { await testStartupProjects(browser, origin, (name, pass, detail = "") => {
    console.log(`${pass ? "PASS" : "FAIL"} ${name}${!pass && detail ? `\n${detail}` : ""}`); if (!pass) failures++;
  }); } finally { await browser.close(); files.close(); }
  if (failures) process.exitCode = 1;
}
