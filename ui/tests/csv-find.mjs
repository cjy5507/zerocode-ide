import { openWindowTestPage } from "./window-boot.mjs";
import { runInBothEngines, startedAs } from "./both-engines.mjs";

/* The sheet tab's find reaches rows the tab has not drawn (t-116954).
 *
 * The CSV tab draws only the rows on screen, and the doc find used to walk the
 * DOM — so a value on a row off the screen was "일치 없음", said without a
 * word of doubt (a 1 MiB sheet's row 19,162, in both engines). The sheet is
 * searched now: every cell counts, a step scrolls to its row, draws it and
 * washes the cell, a row number is not content, and closing the bar takes
 * the wash off for good. Run alone it checks both engines:
 * `node ui/tests/csv-find.mjs`.
 *
 * Synthetic cells only: 5,000 rows of `id,name,code`, a needle in every
 * hundredth row's name and one code found nowhere else near the end. Every
 * cell is letters (an id and a code are numbers written in a to z), so a bare
 * number is on screen only as a row number. */

const ROWS = 5000;
const NEEDLE = "sheet-needle";
const FAR = "far-only-code";
const FAR_ROW = 4700;

/* A number in the letters a to z, so no cell holds a digit. */
const letters = (value) => {
  let out = "";
  for (let left = value; left > 0; left = Math.floor(left / 26)) out = String.fromCharCode(97 + (left % 26)) + out;
  return out || "a";
};

function sheetText() {
  const rows = ["id,name,code"];
  for (let at = 1; at <= ROWS; at += 1) {
    const name = at % 100 === 0 ? `name ${NEEDLE}` : "name plain";
    const code = at === FAR_ROW ? FAR : `code-${letters(at * 7919)}`;
    rows.push(`id-${letters(at)},${name},${code}`);
  }
  return `${rows.join("\n")}\n`;
}

async function checks(page, ok) {
  const text = sheetText();
  await page.evaluate(async ({ text }) => {
    const frames = (n) => new Promise((done) => { const tick = () => (--n <= 0 ? done() : requestAnimationFrame(tick)); requestAnimationFrame(tick); });
    window.__ANSWER__.read_text_file = () => ({ text, version: "csv-find" });
    await openCsv("/tmp/zerocode-window-test/find.csv");
    while (document.querySelector(".csv-view:not([hidden]) .csv-row") === null) await new Promise((done) => setTimeout(done, 5));
    await frames(2);
  }, { text });

  // Through the find bar's own functions: search, then step back once from the start — the last match.
  const find = (word) => page.evaluate(async ({ word }) => {
    const frames = (n) => new Promise((done) => { const tick = () => (--n <= 0 ? done() : requestAnimationFrame(tick)); requestAnimationFrame(tick); });
    const tab = tabs.find((one) => one.kind === "csv");
    const host = docHost(tab.pane, "csv");
    openDocSearch(tab, host);
    tab.docFindQuery = word;
    host.querySelector(".doc-find-input").value = word;
    applyDocSearch(tab, host);
    moveDocMatch(tab, host, -1);
    await frames(2);
    const said = host.querySelector(".doc-find-count").textContent;
    const active = CSS.highlights?.get("mdview-search-active-match");
    const range = active ? [...active][0] : null;
    const cell = range?.startContainer.parentElement ?? null;
    const body = host.querySelector(".csv-body").getBoundingClientRect();
    const box = cell?.getBoundingClientRect();
    // Below the head that stays on top, and inside the body.
    const onScreen = Boolean(box) && box.top >= body.top + 27 && box.bottom <= body.bottom + 1;
    return { said, onScreen, cellText: cell?.textContent ?? null, drawn: host.querySelectorAll(".csv-row").length };
  }, { word });

  const needles = await find(NEEDLE);
  ok("find counts a needle on every row that holds one, drawn or not",
    needles.said === `${ROWS / 100} / ${ROWS / 100}`, JSON.stringify(needles));
  ok("the last needle's row is drawn, on screen below the head, and its cell is washed",
    needles.onScreen && needles.cellText === `name ${NEEDLE}` && needles.drawn < ROWS / 10, JSON.stringify(needles));

  const far = await find(FAR);
  ok("a value only a row near the end holds is found and brought on screen",
    far.said === "1 / 1" && far.onScreen && far.cellText === FAR, JSON.stringify(far));

  // Row 17 stands on the first screen with its number drawn; no cell holds a digit.
  await page.evaluate(() => { document.querySelector(".csv-view:not([hidden]) .csv-body").scrollTop = 0; });
  const number = await find("17");
  ok("a row number is not the sheet's content: a bare number found only there is no match",
    !/\d/.test(text) && number.said === "일치 없음", JSON.stringify(number));

  // "a" stands twice in every "name plain" — ten thousand times in all.
  const many = await find("a");
  ok("past the bound the count says so rather than a number it did not finish",
    / \/ 5000\+$/.test(many.said), JSON.stringify({ said: many.said }));

  // Closing the bar takes the wash off, and a scroll that draws other rows puts none back.
  const closed = await page.evaluate(async () => {
    const frames = (n) => new Promise((done) => { const tick = () => (--n <= 0 ? done() : requestAnimationFrame(tick)); requestAnimationFrame(tick); });
    const tab = tabs.find((one) => one.kind === "csv");
    const host = docHost(tab.pane, "csv");
    tab.docFindQuery = "sheet-needle";
    applyDocSearch(tab, host);
    closeDocSearch(tab, host);
    host.querySelector(".csv-body").scrollTop = 0;
    await frames(2);
    host.querySelector(".csv-body").scrollTop = 28 * 2000;
    await frames(3);
    return { washed: CSS.highlights?.get("mdview-search-match")?.size ?? 0 };
  });
  ok("closing the find takes the wash off, and scrolling draws no wash back", closed.washed === 0, JSON.stringify(closed));
}

export async function testCsvFind(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await checks(page, ok);
    ok("the sheet's find raises no browser errors", faults.length === 0, JSON.stringify(faults.slice(0, 3)));
  } finally {
    await page.close();
  }
}

if (startedAs(import.meta.url)) await runInBothEngines([checks]);
