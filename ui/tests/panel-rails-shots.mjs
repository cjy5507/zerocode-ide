import { mkdir } from "node:fs/promises";
import { resolve } from "node:path";
import { chromium, createWindowServer, openWindowTestPage, standRails } from "./window-boot.mjs";
import { standRailFixture } from "./panel-rails.mjs";

/* The rails as a person first sees them, for the report (t-84803): the invented fixture at the draft's widths, in both
 * themes, and each sheet open once. Nothing is judged here; `panel-rails.mjs` measures.
 *
 *   node ui/tests/panel-rails-shots.mjs <folder> */

const out = resolve(process.argv[2] ?? "panel-rails-shots");
await mkdir(out, { recursive: true });
const { origin } = await createWindowServer();
const browser = await chromium.launch({ headless: true });
const { page, faults } = await openWindowTestPage(browser, origin);
const frames = () => page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
await page.setViewportSize({ width: 1440, height: 800 });
await standRailFixture(page);
await standRails(page);
await page.evaluate(() => { setActivityItem("pile"); });
// Past the colours' own easing: a theme just changed reads half way between the two.
const shot = async (name) => {
  await frames();
  await page.evaluate(() => new Promise((done) => setTimeout(done, 400)));
  await page.screenshot({ path: resolve(out, `${name}.png`) });
};
for (const theme of ["dark", "light"]) {
  await page.evaluate((theme) => { document.documentElement.dataset.theme = theme; }, theme);
  for (const width of [1600, 1360, 1000, 390]) {
    await page.setViewportSize({ width, height: 800 });
    await shot(`${theme}-${width}`);
  }
}
await page.evaluate(() => { document.documentElement.dataset.theme = "dark"; });
await page.setViewportSize({ width: 1000, height: 800 });
await page.evaluate(() => openRailSheet("aside"));
await shot("dark-1000-right-sheet");
await page.evaluate(() => { closeRailSheet("aside"); openRailSheet("sidebar"); });
await shot("dark-1000-sidebar-sheet");
await page.evaluate(() => closeRailSheet("sidebar"));
await page.setViewportSize({ width: 390, height: 800 });
await page.evaluate(() => openRailSheet("sidebar"));
await shot("dark-390-sidebar-sheet");
await page.evaluate(() => { closeRailSheet("sidebar"); openRailSheet("aside"); });
await shot("dark-390-right-sheet");
await browser.close();
console.log(`${out}: faults ${faults.length}`);
if (faults.length) console.log(faults.join("\n"));
process.exit(faults.length ? 1 : 0);
