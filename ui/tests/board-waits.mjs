import { installHarnessWaits } from "./harness-waits.mjs";
import { openWindowTestPage } from "./window-boot.mjs";

export async function installBoardWaits(page) {
  await installHarnessWaits(page);
  await page.evaluate(() => {
    window.__IN_FLIGHT__ ??= new Map();
    window.__BOARD_SETTLED__ = async () => {
      // Let a gesture's microtasks and ResizeObserver schedule their work.
      await new Promise((done) => requestAnimationFrame(done));
      await window.__UNTIL__(() => {
        const views = [...document.querySelectorAll("#board-view, .file-view")]
          .filter((view) => !view.hidden && view.querySelector(".agent-graph-layout"));
        return views.length > 0 && views.every((view) => agentGraphModels.has(view)
          && !agentGraphEdgeFrames.has(view) && !agentGraphFitFrames.has(view))
          && !agentGraphRefreshing && agentPaintFrame === null
          && !["board_snapshot", "pane_agents", "ledger_agents"].some((name) => window.__IN_FLIGHT__.get(name) > 0);
      }, "board snapshot, paint and graph geometry");
      await previewTail;
    };
  });
}

export async function testBoardWaits(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await installBoardWaits(page);
    await page.evaluate(() => {
      if (typeof agentBoardMode !== "undefined") agentBoardMode = "graph";
      let released = false;
      const waiting = [];
      const answer = {
        columns: [{ bucket: "working", cards: [{
          pane: "term:3", agent: "codex", state: "working", heading: "Held snapshot",
          task: "Held snapshot", project: "/fixture", worktree: "main", at: 1, changed_at: 1,
        }] }], attention_count: 0, total_count: 1,
      };
      window.__ANSWER__.board_snapshot = async () => {
        if (!released) await new Promise((done) => waiting.push(done));
        return answer;
      };
      window.__RELEASE_BOARD__ = () => {
        released = true;
        for (const done of waiting) done();
      };
      openBoard();
      window.__BOARD_WAIT_DONE__ = false;
      window.__BOARD_SETTLED__().then(() => { window.__BOARD_WAIT_DONE__ = true; })
        .catch((error) => { window.__BOARD_WAIT_ERROR__ = String(error); });
    });
    await page.waitForFunction(() => window.__IN_FLIGHT__.get("board_snapshot") > 0);
    const before = await page.evaluate(async () => {
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      return window.__BOARD_WAIT_DONE__;
    });
    await page.evaluate(() => window.__RELEASE_BOARD__());
    await page.waitForFunction(() => window.__BOARD_WAIT_DONE__ || window.__BOARD_WAIT_ERROR__);
    const after = await page.evaluate(() => ({
      error: window.__BOARD_WAIT_ERROR__ ?? null,
      labels: [...document.querySelectorAll(".agent-graph-node.is-agent .agent-graph-node-title")]
        .map((node) => node.textContent),
      snapshots: window.__IN_FLIGHT__.get("board_snapshot"),
    }));
    ok("board readiness waits for the held canonical snapshot and its graph geometry",
      !before && after.error === null && after.snapshots === 0
        && after.labels.includes("Held snapshot") && faults.length === 0,
      JSON.stringify({ before, ...after, faults }));
  } finally { await page.close(); }
}

if (process.argv[1] && import.meta.url === (await import("node:url")).pathToFileURL(process.argv[1]).href) {
  const { chromium, createWindowServer } = await import("./window-boot.mjs");
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch();
  try {
    await testBoardWaits(browser, origin, (name, pass, detail) => {
      console.log(`${pass ? "PASS" : "FAIL"} ${name} ${detail}`);
      if (!pass) process.exitCode = 1;
    });
  } finally { await browser.close(); files.close(); }
}
