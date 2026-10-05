/* A scene's own proof that a person can finish it (t-41387): the scene's
 * `selfcheck.mjs` — written by the scene's author, who never read the door —
 * drives the page as a person does (clicks, keys, the code read off the
 * phone), and the oracle says what the page took. Prints only a verdict per
 * scene and the keys that differ, never the page's shapes, so an engineer who
 * has not yet seen a scene can run its check without reading it. The verdict is
 * said under the oracle's three rules side by side (the first rule as it was,
 * the whole result but for the keys the scene declares, the expected keys
 * alone) with the keys only the page recorded; `--out FILE` keeps each scene's
 * page result whole beside them, so a verdict can be counted again later.
 *
 *   node person-road.mjs --scene DIR [--scene DIR …] [--out FILE.json]
 *
 * Exit 0 when every scene's person road finishes it.
 */

import { writeFile } from "node:fs/promises";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { chromium } from "../../../ui/tests/playwright-chromium.mjs";
import { readScene, serveScene, verdictWords, verdicts } from "./scene-kit.mjs";

/* The time a person gets over one scene's whole road. */
const ROAD_MS = 120_000;

const folders = process.argv.slice(2).flatMap((word, at, all) => (all[at - 1] === "--scene" ? [word] : []));
const outAt = process.argv.indexOf("--out");
const outPath = outAt >= 0 ? process.argv[outAt + 1] : null;
if (!folders.length) { console.error("--scene DIR [--scene DIR …] [--out FILE.json]"); process.exit(2); }

const browser = await chromium.launch();
const rows = [];
let failed = 0;
try {
  for (const folder of folders) {
    const scene = await readScene(folder);
    const site = await serveScene(scene.folder);
    let verdict;
    let row = { scene: scene.name, pass: false, result: null };
    try {
      const { solveAsPerson } = await import(pathToFileURL(join(scene.folder, "selfcheck.mjs")).href);
      const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
      page.on("dialog", (dialog) => dialog.dismiss().catch(() => {}));
      page.setDefaultTimeout(ROAD_MS);
      await page.goto(site.url);
      await solveAsPerson(page, scene.card);
      await page.waitForTimeout(500);
      const result = await page.evaluate(() => window.__sceneResult || null);
      // The rules of success side by side; a key only the page holds that the scene does not say the page makes is a field an agent could have written.
      const v = verdicts(result, scene.expected, scene.made, scene.shapes);
      row = { scene: scene.name, ...v, result };
      verdict = v.pass
        ? `PASS  ${scene.name}  a person's road finishes it  ${verdictWords(v)}`
        : `FAIL  ${scene.name}  a person's road: ${v.wrong.length} keys differ (${v.wrong.join(", ")}), ${v.extra.length} keys only the page holds (${v.extra.join(", ")}) ${verdictWords(v)}`;
      await page.close();
    } catch (error) {
      verdict = `FAIL  ${scene.name}  a person's road threw: ${String(error?.message || error).slice(0, 160)}`;
      row = { scene: scene.name, pass: false, threw: String(error?.message || error).slice(0, 400), result: null };
    }
    site.close();
    if (verdict.startsWith("FAIL")) failed += 1;
    rows.push(row);
    console.log(verdict);
  }
} finally {
  await browser.close();
}
if (outPath) await writeFile(outPath, JSON.stringify(rows, null, 2) + "\n");
process.exit(failed ? 1 : 0);
