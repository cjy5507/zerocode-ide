/* The Artifacts tab read by task, photographed at the tab's real width (t-36910, stage 2) — what a
 * person sees in a 1440 window with both side panels open, in both themes, from the window harness
 * with the synthetic catalog of `artifact-cards.mjs` and the tasks of `artifact-tasks.mjs`.
 *
 *   node ui/tests/artifact-tasks-shots.mjs <folder>
 *
 * Five pictures for each theme: the list of tasks, one task read with its evidence under the head,
 * a before-and-after comparison, a task whose files are gone, and — in a window wide enough — the
 * evidence as a column beside the report. */
import { mkdir } from "node:fs/promises";
import { resolve } from "node:path";
import { createWindowServer, launchWindowBrowser, openWindowTestPage } from "./window-boot.mjs";
import { REAL_WINDOW, standCatalog } from "./artifact-cards.mjs";
import { standTasks } from "./artifact-tasks.mjs";

const folder = resolve(process.argv[2] ?? "output/artifact-tasks");
await mkdir(folder, { recursive: true });

/* How long a picture waits for the reader to stand, in ms: the fake runtime answers at once, and the
 * rest is the bundle's frame and two frames of layout with a margin. */
const SETTLED_AFTER_MS = 500;
/* A window wide enough for the evidence column (the tab is 1100px or more). */
const WIDE_WINDOW = Object.freeze({ width: 1800, height: 900 });

/* What each picture shows: the task opened (none for the list alone), whether the first pair of
 * pictures is compared, and the window it is taken in. */
const SCENES = Object.freeze([
  { name: "tasks-list", task: null, compare: false, window: REAL_WINDOW },
  { name: "tasks-reader", task: "t-501", compare: false, window: REAL_WINDOW },
  { name: "tasks-compare", task: "t-501", compare: true, window: REAL_WINDOW },
  { name: "tasks-lost", task: "t-502", compare: false, window: REAL_WINDOW },
  { name: "tasks-wide", task: "t-501", compare: false, window: WIDE_WINDOW },
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
      await standTasks(page);
      // The task whose evidence is gone still has its report: the picture shows it under the names
      // that are struck through. (The suites' catalog gives that report no body.)
      await page.evaluate(() => {
        const answer = window.__ANSWER__.artifact_preview;
        const kept = [
          "# 검증 보고 — 큐가 비워지는가", "",
          "비워진다. 세 번 닫아 세 번 모두 남은 일이 0건이었다.", "",
          "## 본 것", "",
          "닫기 전 큐에 남은 일은 7건, 닫은 뒤에는 0건이다. 호출한 쪽의 기록에도 7이 적혔다.", "",
        ].join("\n");
        window.__ANSWER__.artifact_preview = (args) => (args.id === "r-review"
          ? { kind: "markdown", text: kept, bytes: kept.length, truncated: false }
          : answer(args));
      });
      await page.evaluate(async (wait) => {
        dropTab("artifacts");
        Object.assign(artifactFilter, { tab: ARTIFACT_TASKS_TAB, query: "", origin: null, project: null, agent: null, period: "all", showMissing: false });
        artifactTabPicked = false;
        el("nav-artifacts").click();
        await new Promise((done) => setTimeout(done, wait));
      }, SETTLED_AFTER_MS);
      for (const scene of SCENES) {
        await page.setViewportSize(scene.window);
        await page.evaluate(async ({ scene, wait }) => {
          const view = artifactsView();
          closeArtifactTask();
          paintArtifactTasks(view);
          paintArtifactDrawer(view);
          if (scene.task) openArtifactTask(view, scene.task, { reveal: true });
          await new Promise((done) => setTimeout(done, wait));
          if (scene.compare) {
            [...view.querySelectorAll("button.artifact-evidence")].find((one) => one.dataset.before)?.click();
            await new Promise((done) => setTimeout(done, wait));
          }
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
