/* The browser pane's toolbar in a narrow pane.
 *
 * The layout survives a 320px pane the way it survives a 320px window: a toolbar wraps, and no fixed minimum hides an
 * action. The toolbar held ten buttons and the address in one row that did not wrap, so in the pane a split leaves at
 * the window's default size (320px) the last two — 「기본 브라우저로 열기」 and the ⚙ menu — stood outside the pane,
 * where nothing could press them, and the address was a 44px stub. Measured, 2026-11-25: one row needed 360px.
 * Those two doors now stand in the ⚙ menu: eight buttons and the address need 300px, so the default window's split
 * fits one row again and the two rows are for what is narrower still — a third column, a window under the default.
 *
 * What is held here: a pane too narrow for one row stands the toolbar in two — the navigation and the address above,
 * the tools below — with every control inside the pane and under the pointer; a pane wide enough keeps the one row it
 * had; the rows follow the pane as it is resized; and the page begins under whatever the toolbar's height is.
 *
 *   node ui/tests/browser-toolbar.mjs
 *   WINDOW_SUITES=browser-toolbar node ui/tests/window.mjs */
import { pathToFileURL } from "node:url";
import { resolve } from "node:path";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";

/* The window's default size (crates/zerocode-shell/tauri.conf.json): its stage is 649px, a split leaves 320px. */
const DEFAULT_WINDOW = { width: 1280, height: 800 };
/* A window a little under the default: its split leaves 280px, less than one row needs. */
const NARROW_WINDOW = { width: 1200, height: 800 };
/* A window whose split leaves a pane one row fits in with room to spare (380px). */
const ROOMY_WINDOW = { width: 1400, height: 800 };
/* The toolbar's own controls: three to navigate, the address, four tools and the menu. */
const CONTROLS = 9;

async function standBrowser(browser, origin, viewport, { beside }) {
  const opened = await openWindowTestPage(browser, origin, { context: { viewport } });
  await opened.page.waitForFunction(() => {
    const terminal = tabs.find((tab) => tab.kind === "term");
    return !!terminal && activePaneOf(terminal) !== null;
  });
  await opened.page.evaluate(async ({ beside }) => {
    window.__ANSWER__.browser_place = () => null;
    window.__ANSWER__.browser_zoom = () => null;
    window.__ANSWER__.open_browser_pane = () => "browser-930";
    // The palette's own 「옆에 분할」: the person's hand divides a narrow group (browser-beside.mjs).
    const label = await openBrowserTab("https://example.com/docs/getting-started", beside ? { beside: true, splitNarrow: true } : {});
    for (const hear of window.__LISTENERS__["browser:nav"] ?? []) {
      hear({ payload: { label, url: "https://example.com/docs/getting-started", state: "finished" } });
    }
  }, { beside });
  await settled(opened.page);
  return opened;
}

/* The pane's observers have answered and the frame after them is painted. */
const settled = (page) => page.evaluate(() => new Promise((done) => {
  requestAnimationFrame(() => requestAnimationFrame(() => setTimeout(done, 60)));
}));

/* The toolbar as the person meets it: the pane's width, every shown control's box inside the pane, whether the pointer
 * at its centre lands on it, and where the page begins. */
const toolbar = (page) => page.evaluate(() => {
  const tab = tabs.find((one) => one.kind === "browser" && one.label === "browser-930");
  const host = groupOf(tab.pane).browserView;
  const pane = host.getBoundingClientRect();
  const bar = host.querySelector(".browser-toolbar");
  const controls = [...bar.children].filter((node) => node.checkVisibility()).map((node) => {
    const box = node.getBoundingClientRect();
    const under = document.elementFromPoint(box.left + box.width / 2, box.top + box.height / 2);
    return {
      name: [...node.classList].at(-1),
      left: Math.round(box.left - pane.left),
      right: Math.round(box.right - pane.left),
      top: Math.round(box.top - pane.top),
      reachable: under !== null && (under === node || node.contains(under)),
    };
  });
  const barBox = bar.getBoundingClientRect();
  const padding = parseFloat(getComputedStyle(bar).paddingRight);
  return {
    pane: Math.round(pane.width),
    padding,
    controls,
    rows: [...new Set(controls.map((one) => one.top))].sort((left, right) => left - right),
    barBottom: Math.round(barBox.bottom - pane.top),
    pageTop: Math.round(host.querySelector(".browser-body").getBoundingClientRect().top - pane.top),
    lifted: host.querySelector(".browser-address").classList.contains("is-overlaying"),
  };
});

const outside = (seen) => seen.controls.filter((one) => one.left < 0 || one.right > seen.pane || !one.reachable)
  .map((one) => one.name);
const NAVIGATION = ["browser-back", "browser-forward", "browser-reload", "browser-address-slot"];

export async function testBrowserToolbar(browser, origin, ok) {
  {
    const { page, faults } = await standBrowser(browser, origin, NARROW_WINDOW, { beside: true });
    try {
      const narrow = await toolbar(page);
      ok("a browser pane too narrow for one toolbar row keeps every control inside it and under the pointer",
        narrow.pane < 300 && narrow.controls.length === CONTROLS && outside(narrow).length === 0,
        JSON.stringify({ pane: narrow.pane, outside: outside(narrow), controls: narrow.controls }));
      const above = narrow.controls.filter((one) => one.top === narrow.rows[0]).map((one) => one.name);
      const below = narrow.controls.filter((one) => one.top === narrow.rows[1]).map((one) => one.name);
      const address = narrow.controls.find((one) => one.name === "browser-address-slot");
      ok("its toolbar stands in two rows: the navigation and the address above, the tools below, the address to the pane's edge",
        narrow.rows.length === 2 && JSON.stringify(above) === JSON.stringify(NAVIGATION)
          && below.length === CONTROLS - NAVIGATION.length
          && below.at(-1) === "browser-menu" && address.right >= narrow.pane - narrow.padding - 1,
        JSON.stringify({ rows: narrow.rows, above, below, address, pane: narrow.pane }));
      ok("the page begins under the toolbar's two rows", narrow.pageTop >= narrow.barBottom,
        JSON.stringify({ barBottom: narrow.barBottom, pageTop: narrow.pageTop }));
      await page.evaluate(() => {
        const tab = tabs.find((one) => one.kind === "browser" && one.label === "browser-930");
        groupOf(tab.pane).browserView.querySelector(".browser-address").focus();
      });
      await settled(page);
      const focused = await toolbar(page);
      ok("an address that has the row to itself is typed where it stands, not lifted over the toolbar",
        focused.lifted === false && outside(focused).length === 0, JSON.stringify({ lifted: focused.lifted, outside: outside(focused) }));
      // The hand leaves the address: in one row a focused address under 220px is lifted over the buttons, as before.
      await page.evaluate(() => document.activeElement?.blur());
      await page.setViewportSize(ROOMY_WINDOW);
      await settled(page);
      const widened = await toolbar(page);
      ok("the same pane widened past one row's need stands the toolbar in one row again",
        widened.pane >= 300 && widened.rows.length === 1 && outside(widened).length === 0,
        JSON.stringify({ pane: widened.pane, rows: widened.rows, outside: outside(widened) }));
      await page.setViewportSize(NARROW_WINDOW);
      await settled(page);
      const narrowed = await toolbar(page);
      ok("and narrowed again, in two", narrowed.rows.length === 2 && outside(narrowed).length === 0,
        JSON.stringify({ pane: narrowed.pane, rows: narrowed.rows, outside: outside(narrowed) }));
      ok("the narrow toolbar raises no page errors", faults.length === 0, faults.join("\n"));
    } finally { await page.close(); }
  }
  {
    const { page, faults } = await standBrowser(browser, origin, DEFAULT_WINDOW, { beside: true });
    try {
      const split = await toolbar(page);
      ok("the default window's split (320px) fits the toolbar in one row, the menu inside the pane",
        split.pane >= 300 && split.pane < 360 && split.rows.length === 1 && split.controls.length === CONTROLS
          && outside(split).length === 0 && split.controls.at(-1).name === "browser-menu",
        JSON.stringify({ pane: split.pane, rows: split.rows, outside: outside(split), last: split.controls.at(-1) }));
      ok("the default split's toolbar raises no page errors", faults.length === 0, faults.join("\n"));
    } finally { await page.close(); }
  }
  {
    const { page, faults } = await standBrowser(browser, origin, DEFAULT_WINDOW, { beside: false });
    try {
      const wide = await toolbar(page);
      ok("a pane wide enough keeps the one toolbar row it had, the address taking what the buttons leave",
        wide.pane >= 300 && wide.rows.length === 1 && wide.controls.length === CONTROLS && outside(wide).length === 0
          && wide.barBottom <= 40,
        JSON.stringify({ pane: wide.pane, rows: wide.rows, barBottom: wide.barBottom, outside: outside(wide) }));
      // One picture, one meaning: the globe is the built-in browser (the link menu's row, the tab, the palette's
      // rows), the arrow leaving a box is a door to the outside, and finding is the magnifier every search field wears.
      const marks = await page.evaluate(async () => {
        const mark = (node) => node?.querySelector("use")?.getAttribute("href") ?? null;
        const tab = document.querySelector('.tab[data-tab="browser:browser-930"]');
        const host = groupOf(tabs.find((one) => one.label === "browser-930").pane).browserView;
        openTabCreate(document.querySelector(".tab-new"));
        await new Promise((done) => setTimeout(done, 120));
        const rows = [...document.querySelectorAll("#tab-create-pop .tc-row")]
          .filter((row) => row.textContent.includes(t("tabs.newBrowser", "새 브라우저 탭")))
          .map(mark);
        closeTabCreate("test");
        return { tab: mark(tab), find: mark(host.querySelector(".browser-find")), rows };
      });
      ok("a browser's tab and the palette's browser rows wear the globe, and the find button the magnifier",
        marks.tab === "#i-globe" && marks.find === "#i-search" && marks.rows.length === 2
          && marks.rows.every((one) => one === "#i-globe"),
        JSON.stringify(marks));
      ok("the wide toolbar raises no page errors", faults.length === 0, faults.join("\n"));
    } finally { await page.close(); }
  }
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  let failed = 0;
  try {
    await testBrowserToolbar(browser, origin, (name, pass, details = "") => {
      console.log(`${pass ? "PASS" : "FAIL"} ${name}${!pass ? `\n${details}` : ""}`);
      if (!pass) failed++;
    });
  } finally { await browser.close(); files.close(); }
  if (failed) process.exitCode = 1;
}
