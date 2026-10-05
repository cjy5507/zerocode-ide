/* A scene's own proof that a person can finish it (t-41387): the scene's
 * `selfcheck.mjs` — written by the scene's author, who never read the door —
 * drives the page as a person does (clicks, keys, the code read off the
 * phone), and the oracle says what the page took. Prints only a verdict per
 * scene and the keys that differ, never the page's shapes, so an engineer who
 * has not yet seen a scene can run its check without reading it.
 *
 *   node person-road.mjs --scene DIR [--scene DIR …]
 *
 * Exit 0 when every scene's person road finishes it.
 */

import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { chromium } from "../../../ui/tests/playwright-chromium.mjs";
import { extraKeys, readScene, sameAsSet, sameWhole, serveScene, wrongKeys } from "./scene-kit.mjs";

/* The time a person gets over one scene's whole road. */
const ROAD_MS = 120_000;

const folders = process.argv.slice(2).flatMap((word, at, all) => (all[at - 1] === "--scene" ? [word] : []));
if (!folders.length) { console.error("--scene DIR [--scene DIR …]"); process.exit(2); }

const browser = await chromium.launch();
let failed = 0;
try {
  for (const folder of folders) {
    const scene = await readScene(folder);
    const site = await serveScene(scene.folder);
    let verdict;
    try {
      const { solveAsPerson } = await import(pathToFileURL(join(scene.folder, "selfcheck.mjs")).href);
      const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
      page.on("dialog", (dialog) => dialog.dismiss().catch(() => {}));
      page.setDefaultTimeout(ROAD_MS);
      await page.goto(site.url);
      await solveAsPerson(page, scene.card);
      await page.waitForTimeout(500);
      const result = await page.evaluate(() => window.__sceneResult || null);
      const wrong = wrongKeys(result, scene.expected);
      // Both rules of success: the expected keys, and the page's whole result equal to the expected one.
      const rules = `whole=${Boolean(result) && sameWhole(result, scene.expected)} sameSet=${sameAsSet(result, scene.expected)} extra=${extraKeys(result, scene.expected).join("|") || "-"}`;
      verdict = wrong.length ? `FAIL  ${scene.name}  a person's road: ${wrong.length} keys differ (${wrong.join(", ")}) ${rules}` : `PASS  ${scene.name}  a person's road finishes it  ${rules}`;
      await page.close();
    } catch (error) {
      verdict = `FAIL  ${scene.name}  a person's road threw: ${String(error?.message || error).slice(0, 160)}`;
    }
    site.close();
    if (verdict.startsWith("FAIL")) failed += 1;
    console.log(verdict);
  }
} finally {
  await browser.close();
}
process.exit(failed ? 1 : 0);
