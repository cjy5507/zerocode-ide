// The control of the low-spec profile's frame rows: an empty page's requestAnimationFrame
// gaps, under the same scheduling the other probes ran under. A profile whose
// control gap is over one display frame (16.7 ms) cannot judge a frame metric —
// `taskpolicy -b` coalesces timers and frames (idle control p50 105 ms, 2026-10-01) —
// so the report marks those rows 판정 불가 instead of handing out an artifact as a regression.
import { createRequire } from "node:module";
const { chromium } = createRequire(import.meta.url)("playwright");
const browser = await chromium.launch();
const page = await browser.newPage();
await page.setContent("<body></body>");
const gaps = await page.evaluate(() => new Promise((done) => {
  const seen = [];
  const tick = (at) => { seen.push(at); if (seen.length < 91) requestAnimationFrame(tick); else done(seen.slice(31).map((one, i) => one - seen[30 + i])); };
  requestAnimationFrame(tick);
}));
await browser.close();
gaps.sort((a, b) => a - b);
const at = (q) => gaps[Math.min(gaps.length - 1, Math.floor(gaps.length * q))];
console.log(`METRIC frame supply: {"controlGapP50":${at(0.5).toFixed(1)},"controlGapP95":${at(0.95).toFixed(1)}}`);
