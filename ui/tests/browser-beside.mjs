/* Where a browser tab stands when it is opened beside the pane in front.
 *
 * A pane narrower than 720px, divided, leaves two halves nobody can use: a published artifact has opened by that rule
 * (a tab of its maker's group instead of a split) since t-11958. The browser had no such rule, so at the window's
 * default size a link, a sign-in page or 「옆에 작업 보기」 divided the 649px stage into two 320px panes — a page a
 * quarter of the window wide, narrower than the file column beside it (measured 2026-11-25).
 *
 * What is held here: a browser tab nobody asked to split for stands beside only where there is room — a right-hand
 * seat that is already there, or a group at least that wide to divide — and is otherwise a tab of the group in front;
 * the doors whose own words ask for the side — the palette's 「옆에 분할」, the title bar's 「옆에 작업 보기」 — are
 * the person's hand and divide a narrow group as before.
 *
 * And the room a browser beside gets: while one stands in a side group the file column is folded, so the page has the
 * column's 350px too (960px to 1135px in a 2560px window, measured 2026-11-25). The column comes back when no browser
 * shows beside any more; the person's own press of its toggle, either way, is theirs and is left as they made it.
 *
 *   node ui/tests/browser-beside.mjs
 *   WINDOW_SUITES=browser-beside node ui/tests/window.mjs */
import { pathToFileURL } from "node:url";
import { resolve } from "node:path";
import { readFile } from "node:fs/promises";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";

/* The rule's own number, read from the window's source rather than kept in a second place. */
const BESIDE_MIN = Number((await readFile(new URL("../shell-browser.js", import.meta.url), "utf8"))
  .match(/const STAGE_BESIDE_MIN_PX = (\d+);/)?.[1]);

/* The window's default size: its stage is 649px. And one whose stage (1097px) has room to divide. */
const DEFAULT_WINDOW = { width: 1280, height: 800 };
const ROOMY_WINDOW = { width: 1728, height: 1117 };

async function standWindow(browser, origin, viewport) {
  const opened = await openWindowTestPage(browser, origin, { context: { viewport } });
  await opened.page.waitForFunction(() => {
    const terminal = tabs.find((tab) => tab.kind === "term");
    return !!terminal && activePaneOf(terminal) !== null;
  });
  await opened.page.evaluate(() => {
    let minted = 940;
    window.__ANSWER__.browser_place = () => null;
    window.__ANSWER__.browser_zoom = () => null;
    window.__ANSWER__.browser_navigate = () => null;
    window.__ANSWER__.open_browser_pane = (args) => args.label ?? `browser-${++minted}`;
    window.__besideSeen = () => {
      const front = currentTab();
      const browsers = tabs.filter((one) => one.kind === "browser");
      const last = browsers.at(-1);
      return {
        groups: stageGroups().length,
        stage: Math.round(stage.getBoundingClientRect().width),
        front: front?.kind ?? null,
        frontPane: front?.pane ?? null,
        browsers: browsers.length,
        lastPane: last?.pane ?? null,
        lastWidth: last ? Math.round(groupOf(last.pane)?.el?.getBoundingClientRect().width ?? 0) : null,
        terminalPane: tabs.find((one) => one.kind === "term")?.pane ?? null,
        // The file column: folded by its own state, and how wide it stands on screen.
        asideFolded: folded.aside,
        aside: Math.round(document.querySelector("aside.files")?.getBoundingClientRect().width ?? 0),
      };
    };
  });
  return opened;
}

/* The panes are placed and the file column, if it was folded or opened, has finished easing. */
const settled = async (page) => {
  const frames = () => page.evaluate(() => new Promise((done) => {
    requestAnimationFrame(() => requestAnimationFrame(() => setTimeout(done, 80)));
  }));
  await frames();
  await page.waitForFunction(() => !document.getElementById("workbench").classList.contains("is-folding"));
  await frames();
};
const seen = (page) => page.evaluate(() => window.__besideSeen());
/* A group that was not divided is the stage's width, less the pixel its own edge takes. */
const wholeStage = (group, stage) => stage - group >= 0 && stage - group <= 2;

/* A link's door: the terminal's and a document's links, the sign-in pages and a found dev server all open this way. */
const openByLink = async (page, url) => {
  await page.evaluate((url) => openBrowserTab(url, { beside: true }), url);
  await settled(page);
};

/* The palette's own row, pressed as a person presses it. */
const openByPaletteSplit = async (page) => {
  await page.evaluate(() => openTabCreate(document.querySelector(".tab-new")));
  await page.waitForFunction(() => [...document.querySelectorAll("#tab-create-pop .tc-row")]
    .some((row) => row.textContent.includes(t("tabs.newBrowserSplit", "새 브라우저 탭 — 옆에 분할"))));
  await page.evaluate(() => {
    [...document.querySelectorAll("#tab-create-pop .tc-row")]
      .find((row) => row.textContent.includes(t("tabs.newBrowserSplit", "새 브라우저 탭 — 옆에 분할"))).click();
  });
  await page.waitForFunction(() => tabs.some((one) => one.kind === "browser"));
  await settled(page);
};

export async function testBrowserBeside(browser, origin, ok) {
  let narrowStage = null;
  {
    const { page, faults } = await standWindow(browser, origin, DEFAULT_WINDOW);
    try {
      const before = await seen(page);
      narrowStage = before.stage;
      await openByLink(page, "https://example.com/by-link");
      const linked = await seen(page);
      ok("a link opened beside a group too narrow to divide is a tab of that group, the whole group wide",
        linked.groups === before.groups && linked.browsers === 1
          && linked.lastPane === before.terminalPane && linked.front === "browser" && wholeStage(linked.lastWidth, before.stage),
        JSON.stringify({ before, linked }));
      ok("a browser that is a tab of the group in front leaves the file column as it stood",
        linked.asideFolded === before.asideFolded && linked.aside === before.aside, JSON.stringify({ before, linked }));
      ok("a narrow window's link raises no page errors", faults.length === 0, faults.join("\n"));
    } finally { await page.close(); }
  }
  {
    const { page, faults } = await standWindow(browser, origin, DEFAULT_WINDOW);
    try {
      const before = await seen(page);
      await openByPaletteSplit(page);
      const split = await seen(page);
      ok("the palette's 「옆에 분할」 divides a narrow group all the same: the person chose it by hand",
        split.groups === before.groups + 1 && split.lastPane !== before.terminalPane && split.lastWidth * 2 <= split.stage,
        JSON.stringify({ before, split }));
      // A seat that already stands to the right is reused whatever its width: nothing new is divided.
      await page.evaluate(() => setActiveTab(tabs.find((one) => one.kind === "term").id));
      await openByLink(page, "https://example.com/joins");
      const joined = await seen(page);
      ok("a link opened beside a group with a right-hand neighbour joins that neighbour as a tab",
        joined.groups === split.groups && joined.browsers === 2 && joined.lastPane === split.lastPane,
        JSON.stringify({ split, joined }));
      ok("the palette's split raises no page errors", faults.length === 0, faults.join("\n"));
    } finally { await page.close(); }
  }
  {
    const { page, faults } = await standWindow(browser, origin, ROOMY_WINDOW);
    try {
      const before = await seen(page);
      await openByLink(page, "https://example.com/roomy");
      const linked = await seen(page);
      ok("a link opened beside a group with room divides it, as before",
        linked.groups === before.groups + 1 && linked.lastPane !== before.terminalPane && linked.lastWidth * 2 <= linked.stage,
        JSON.stringify({ before, linked }));
      ok("while a browser stands beside, the file column is folded and the page has its width too",
        before.asideFolded === false && before.aside > 0 && linked.asideFolded === true && linked.aside === 0
          && linked.stage >= before.stage + before.aside - 2 && linked.lastWidth > before.stage / 2 + before.aside / 2 - 8,
        JSON.stringify({ before, linked }));
      await page.evaluate(() => closeTab(tabs.find((one) => one.kind === "browser").id));
      await settled(page);
      const closed = await seen(page);
      ok("and comes back when the browser beside is closed",
        closed.browsers === 0 && closed.asideFolded === false && closed.aside === before.aside && closed.stage === before.stage,
        JSON.stringify({ before, closed }));
      // The person opens the column again while a browser stands beside: it stays as they left it, both ways.
      await openByLink(page, "https://example.com/roomy-two");
      await page.evaluate(() => document.getElementById("toggle-aside").click());
      await settled(page);
      const reopened = await seen(page);
      await page.evaluate(() => closeTab(tabs.find((one) => one.kind === "browser").id));
      await settled(page);
      const kept = await seen(page);
      ok("a file column the person opened again beside a browser stays open, then and after the browser closes",
        reopened.asideFolded === false && reopened.aside === before.aside && kept.asideFolded === false,
        JSON.stringify({ reopened, kept }));
      // And one the person folded themselves is theirs: a browser beside neither opens it nor, closing, unfolds it.
      await page.evaluate(() => document.getElementById("toggle-aside").click());
      await settled(page);
      await openByLink(page, "https://example.com/roomy-three");
      await page.evaluate(() => closeTab(tabs.find((one) => one.kind === "browser").id));
      await settled(page);
      const own = await seen(page);
      ok("a file column the person folded themselves stays folded after a browser beside comes and goes",
        own.browsers === 0 && own.asideFolded === true, JSON.stringify(own));
      ok("the rule's width, read from the window's own source, lies between the two stages measured here",
        Number.isInteger(BESIDE_MIN) && narrowStage < BESIDE_MIN && BESIDE_MIN <= before.stage,
        JSON.stringify({ rule: BESIDE_MIN, narrowStage, roomyStage: before.stage }));
      ok("a roomy window's link raises no page errors", faults.length === 0, faults.join("\n"));
    } finally { await page.close(); }
  }
  {
    const { page, faults } = await standWindow(browser, origin, DEFAULT_WINDOW);
    try {
      await page.evaluate(() => {
        const terminal = tabs.find((tab) => tab.kind === "term");
        const term = activePaneOf(terminal) ?? terminal.term;
        paneSessions.delete(term);
        paneAgents.set(term, "codex");
        const state = { label: "browser-960", mode: "agent", revision: 3, inFlight: 0, needsObservation: false,
          ownerTerm: String(term), snapshotSupported: true };
        window.__ANSWER__.get_browser_workspaces = () => [state];
        window.__ANSWER__.browser_snapshot = () => "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jr1sAAAAASUVORK5CYII=";
        for (const hear of window.__LISTENERS__["browser:agent-open"] ?? []) {
          hear({ payload: { label: "browser-960", url: "https://example.com/work", term } });
        }
      });
      await page.waitForFunction(() => tabs.some((one) => one.label === "browser-960"));
      await page.evaluate(() => {
        for (const hear of window.__LISTENERS__["browser:workspace"] ?? []) {
          hear({ payload: window.__ANSWER__.get_browser_workspaces()[0] });
        }
      });
      await settled(page);
      const before = await seen(page);
      await page.evaluate(() => document.querySelector("#browser-workspace-launcher").click());
      await settled(page);
      const watched = await seen(page);
      ok("「옆에 작업 보기」 divides a narrow group all the same: its own words ask for the side",
        before.front === "term" && watched.groups === before.groups + 1 && watched.front === "browser"
          && watched.frontPane !== before.terminalPane && watched.lastWidth * 2 <= watched.stage,
        JSON.stringify({ before, watched }));
      await page.evaluate(() => document.querySelector("#browser-workspace-launcher").click());
      await settled(page);
      const folded = await seen(page);
      ok("and folding it puts the terminal back in front, the stage as it was",
        folded.groups === before.groups && folded.front === "term" && folded.frontPane === before.terminalPane,
        JSON.stringify({ before, folded }));
      ok("the watched work browser raises no page errors", faults.length === 0, faults.join("\n"));
    } finally { await page.close(); }
  }
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  let failed = 0;
  try {
    await testBrowserBeside(browser, origin, (name, pass, details = "") => {
      console.log(`${pass ? "PASS" : "FAIL"} ${name}${!pass ? `\n${details}` : ""}`);
      if (!pass) failed++;
    });
  } finally { await browser.close(); files.close(); }
  if (failed) process.exitCode = 1;
}
