/* The Artifacts tab's cards and drawer, photographed at the tab's real width (t-36910) — what a
 * person sees in a 1440 window with both side panels open, in both themes, from the window harness
 * with the synthetic catalog of `artifact-cards.mjs`.
 *
 *   node ui/tests/artifact-cards-shots.mjs <folder>
 *
 * Three pictures for each theme: a report read in the drawer beside its column of cards, a step log
 * read as steps, and a tab a task filter emptied saying what is not linked. */
import { mkdir } from "node:fs/promises";
import { resolve } from "node:path";
import { createWindowServer, launchWindowBrowser, openWindowTestPage } from "./window-boot.mjs";
import { REAL_WINDOW, openTab, standCatalog } from "./artifact-cards.mjs";

const folder = resolve(process.argv[2] ?? "output/artifact-cards");
await mkdir(folder, { recursive: true });

/* How long a picture waits for the drawer's body to stand, in ms: the fake preview answers at once,
 * and the rest is two frames of layout with a margin. */
const SETTLED_AFTER_MS = 400;

/* What each picture shows: the tab it stands on, the row the drawer reads, and the task filtered to. */
const SCENES = Object.freeze([
  { name: "report", tab: "reports", select: "r-final", task: null },
  { name: "evidence", tab: "evidence", select: "e-steps", task: null },
  { name: "emptied", tab: "evidence", select: null, task: "t-501" },
]);

const { files, origin } = await createWindowServer();
const browser = await launchWindowBrowser();
try {
  for (const theme of ["dark", "light"]) {
    const { page, faults } = await openWindowTestPage(browser, origin);
    try {
      await page.setViewportSize(REAL_WINDOW);
      await page.evaluate((one) => {
        document.documentElement.dataset.theme = one;
      }, theme);
      await standCatalog(page);
      await openTab(page);
      for (const scene of SCENES) {
        await page.evaluate(async ({ scene, wait }) => {
          const view = artifactsView();
          artifactFilter.origin = scene.task ? { field: "task", value: scene.task, label: scene.task } : null;
          artifactSelectedId = null;
          view.querySelector(`[data-artifact-tab="${scene.tab}"]`).click();
          changeArtifactFilter(view);
          if (scene.select) selectArtifact(view, scene.select, { reveal: true });
          await new Promise((done) => setTimeout(done, wait));
        }, { scene, wait: SETTLED_AFTER_MS });
        const path = resolve(folder, `${theme}-${scene.name}.png`);
        await page.screenshot({ path });
        console.log(path);
      }
      if (faults.length) console.log(`faults (${theme}): ${faults.slice(0, 3).join(" | ")}`);
    } finally {
      await page.close();
    }
  }
} finally {
  await browser.close();
  files.close();
}
