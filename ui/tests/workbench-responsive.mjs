import { mkdir } from "node:fs/promises";
import { pathToFileURL } from "node:url";
import { resolve } from "node:path";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";
import { workspaceBoardFixture } from "./workspace-board.mjs";
import { taskBoardFixture } from "./task-board.mjs";

export async function testWorkbenchResponsive(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.evaluate(workspaceBoardFixture);
    await page.evaluate(taskBoardFixture);
    await page.evaluate(() => {
      setWorkspaceBoardOpen(false);
      window.__VAULT__ = { pages: 72, linksPer: 2, ghosts: 2, tags: ["reference"] };
      secondBrainVault = "/vault";
      paintKnowledgeEntry();
    });
    await mkdir("output/playwright/connected-workbench", { recursive: true });
    const sizes = [[320, 320], [360, 640], [768, 320], [1280, 720], [1920, 1080], [3840, 2160]];
    for (const id of ["tasks", "workspaces", "knowledge", "artifacts"]) {
      await page.evaluate((id) => openWorkbenchView(id), id);
      await page.waitForSelector(`.workbench-nav-link[data-workbench-view="${id}"][aria-current="page"]:visible`);
      for (const [width, height] of sizes) {
        await page.setViewportSize({ width, height });
        const size = await page.evaluate(async ({ id, width, height }) => {
          const view = id === "workspaces" ? el("workspace-board")
            : docHost(tabs.find((tab) => tab.kind === (id === "tasks" ? "board" : id)).pane, id === "tasks" ? "board" : id);
          // The product's surfaces are also split leaves. Isolate a leaf at
          // this size so the desktop's sidebar cannot choose its width for it.
          view.style.position = "fixed"; view.style.inset = "0";
          view.style.width = `${width}px`; view.style.height = `${height}px`; view.style.zIndex = "100";
          await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
          const nav = view.querySelector(".workbench-nav");
          const head = view.querySelector(".agent-graph-head, .workspace-board-head, .knowledge-head, .artifacts-head");
          const body = view.querySelector(".task-board-surface, .workspace-board-body, .knowledge-body, .artifacts-body");
          const rect = (node) => { const b = node?.getBoundingClientRect(); return b && { top: b.top, bottom: b.bottom, width: b.width, height: b.height }; };
          return { nav: { width: nav.clientWidth, scroll: nav.scrollWidth, ...rect(nav) },
            head: head && { width: head.clientWidth, scroll: head.scrollWidth, ...rect(head) }, body: rect(body),
            view: rect(view), links: [...nav.querySelectorAll("[data-workbench-view]")].map(rect) };
        }, { id, width, height });
        const valid = size.nav.scroll <= size.nav.width + 1 && size.nav.height > 0 &&
          (!size.head || size.head.scroll <= size.head.width + 1) &&
          size.body?.height > 0 && size.body.bottom <= size.view.bottom + 1 &&
          size.links.every((link) => link.width > 0 && link.height > 0);
        ok(`${id} remains usable in a ${width}×${height} leaf`, valid, JSON.stringify(size));
        if (width === 320 || width === 1280) await page.screenshot({ path: `output/playwright/connected-workbench/${id}-dark-${width}.png` });
      }
      await page.setViewportSize({ width: 1440, height: 1000 });
      // A narrow leaf inside a wide viewport proves container-based reflow.
      const split = await page.evaluate(async (id) => {
        const view = id === "workspaces" ? el("workspace-board")
          : docHost(tabs.find((tab) => tab.kind === (id === "tasks" ? "board" : id)).pane, id === "tasks" ? "board" : id);
        view.style.width = "320px"; view.style.height = "640px";
        applyTheme("light");
        await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
        const nav = view.querySelector(".workbench-nav");
        return { width: nav.clientWidth, scroll: nav.scrollWidth };
      }, id);
      ok(`${id} uses its own 320px leaf width in a wide light-theme window`, split.scroll <= split.width + 1, JSON.stringify(split));
      await page.screenshot({ path: `output/playwright/connected-workbench/${id}-light-split.png` });
      await page.evaluate((id) => {
        const view = id === "workspaces" ? el("workspace-board")
          : docHost(tabs.find((tab) => tab.kind === (id === "tasks" ? "board" : id)).pane, id === "tasks" ? "board" : id);
        view.removeAttribute("style"); applyTheme("dark");
      }, id);
    }
    ok("responsive workbench raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally { await page.close(); }
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  let failures = 0;
  try {
    await testWorkbenchResponsive(browser, origin, (name, pass, detail = "") => {
      console.log(`${pass ? "PASS" : "FAIL"} ${name}${!pass && detail ? `\n${detail}` : ""}`);
      if (!pass) failures++;
    });
  } finally { await browser.close(); files.close(); }
  if (failures) process.exitCode = 1;
}
