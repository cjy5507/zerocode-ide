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
 * the person's hand and divide a narrow group as before. So is a split the person chose in the palette, when a door
 * follows that choice: the Jira card's 「ZeroCode 브라우저에서 열기」.
 *
 * And the room a browser beside gets: while one stands in a side group the file column is folded, so the page has the
 * column's 350px too (960px to 1135px in a 2560px window, measured 2026-11-25). The column comes back when no browser
 * shows beside any more; the person's own press of its toggle, either way, is theirs and is left as they made it —
 * for as long as a browser shows beside, so a second one arriving there does not fold what they opened.
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

/* An agent's work browser, standing behind the terminal in front: the title bar's 「옆에 작업 보기」 is what shows it. */
const standWorkBrowser = async (page) => {
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
      // Opened again and then folded by the person's own press: that fold is theirs too, and a closing browser
      // leaves it.
      await openByLink(page, "https://example.com/roomy-refolded");
      await page.evaluate(() => document.getElementById("toggle-aside").click());
      await settled(page);
      await page.evaluate(() => document.getElementById("toggle-aside").click());
      await settled(page);
      await page.evaluate(() => closeTab(tabs.find((one) => one.kind === "browser").id));
      await settled(page);
      const refolded = await seen(page);
      ok("a file column the person opened and folded again beside a browser stays folded when the browser closes",
        refolded.browsers === 0 && refolded.asideFolded === true, JSON.stringify(refolded));
      await page.evaluate(() => document.getElementById("toggle-aside").click());
      await settled(page);
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
    // A browser that joins a right-hand neighbour without taking the keyboard still comes to that group's front, so
    // it shows beside: the column folds for it once and stays folded — no fold and give-back in the same breath.
    const { page, faults } = await standWindow(browser, origin, ROOMY_WINDOW);
    try {
      await page.evaluate(async () => {
        const source = currentTab();
        const right = splitStageBesideFocused("horizontal");
        await openTermTab({ placement: "tab" });
        const side = currentTab();
        side.pane = right;
        setActiveTab(side.id);
        setActiveTab(source.id);
        // Every change of the column's folded state, in order.
        window.__folds = [];
        const bench = document.getElementById("workbench");
        let was = bench.classList.contains("is-aside-folded");
        new MutationObserver(() => {
          const now = bench.classList.contains("is-aside-folded");
          if (now !== was) window.__folds.push(now);
          was = now;
        }).observe(bench, { attributes: true, attributeFilter: ["class"] });
      });
      await settled(page);
      const before = await seen(page);
      await page.evaluate(() => openBrowserTab("https://example.com/joins-quietly", { beside: true, focus: false }));
      await settled(page);
      const joined = await page.evaluate(() => {
        const tab = tabs.find((one) => one.kind === "browser");
        return { ...window.__besideSeen(), shows: activeTabIn(tab.pane)?.id === tab.id, folds: window.__folds };
      });
      ok("a browser that joins a neighbour without the keyboard shows beside, and the file column folds for it once",
        before.groups === 2 && joined.groups === 2 && joined.browsers === 1 && joined.shows === true
          && joined.front === "term" && joined.asideFolded === true && JSON.stringify(joined.folds) === "[true]",
        JSON.stringify({ before, joined }));
      ok("the neighbour's quiet browser raises no page errors", faults.length === 0, faults.join("\n"));
    } finally { await page.close(); }
  }
  {
    const { page, faults } = await standWindow(browser, origin, DEFAULT_WINDOW);
    try {
      await standWorkBrowser(page);
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
  {
    // A column the person opened again while a browser shows beside is theirs for as long as one shows there. The
    // column is folded when a browser ARRIVES beside where none showed; another one arriving beside the first — by a
    // link, or by 「옆에 작업 보기」 — folded it a second time over the seven tabs the person had just opened (Codex
    // review t-130084, 2026-11-25).
    const { page, faults } = await standWindow(browser, origin, ROOMY_WINDOW);
    try {
      await standWorkBrowser(page);
      const facts = () => page.evaluate(() => {
        const shown = tabs.filter((one) => one.kind === "browser" && activeTabIn(one.pane)?.id === one.id);
        return {
          groups: stageGroups().length,
          front: currentTab()?.kind ?? null,
          showing: shown.map((one) => one.label),
          beside: stageGroups().length > 1 && shown.length > 0,
          asideFolded: folded.aside,
          aside: Math.round(document.querySelector("aside.files")?.getBoundingClientRect().width ?? 0),
        };
      });
      const toTerminal = () => page.evaluate(() => setActiveTab(tabs.find((one) => one.kind === "term").id));
      await openByLink(page, "https://example.com/shows-beside");
      const first = await facts();
      await page.evaluate(() => document.getElementById("toggle-aside").click());
      await settled(page);
      const reopened = await facts();
      await toTerminal();
      await openByLink(page, "https://example.com/arrives-beside");
      const second = await facts();
      ok("a file column the person opened again stays open when another browser arrives beside one that shows",
        first.beside && first.asideFolded === true && reopened.beside && reopened.asideFolded === false && reopened.aside > 0
          && second.groups === first.groups && second.beside && second.asideFolded === false && second.aside === reopened.aside,
        JSON.stringify({ first, reopened, second }));
      // Both leave: nothing shows beside, and the column is as the person left it. The next browser to arrive beside
      // folds it, as the first did — the rule's first half still stands.
      await page.evaluate(async () => {
        for (const one of tabs.filter((tab) => tab.kind === "browser" && !tab.agentWorkspace)) await closeTab(one.id);
      });
      await settled(page);
      const emptied = await facts();
      await openByLink(page, "https://example.com/arrives-alone");
      const again = await facts();
      ok("with no browser left beside, the next one to arrive folds the column as the first did",
        emptied.showing.length === 0 && emptied.asideFolded === false && again.beside && again.asideFolded === true,
        JSON.stringify({ emptied, again }));
      // And 「옆에 작업 보기」 is an arrival like a link's: beside a browser that already shows, it leaves the column
      // the person opened.
      await page.evaluate(() => document.getElementById("toggle-aside").click());
      await settled(page);
      const reopenedAgain = await facts();
      await toTerminal();
      await page.evaluate(() => document.querySelector("#browser-workspace-launcher").click());
      await settled(page);
      const watched = await facts();
      ok("and 「옆에 작업 보기」 beside a browser that shows leaves the column the person opened",
        reopenedAgain.beside && reopenedAgain.asideFolded === false && watched.groups === again.groups
          && watched.front === "browser" && watched.showing.includes("browser-960")
          && watched.asideFolded === false && watched.aside === reopenedAgain.aside,
        JSON.stringify({ again, reopenedAgain, watched }));
      ok("the reopened column raises no page errors", faults.length === 0, faults.join("\n"));
    } finally { await page.close(); }
  }
  // The Jira card's 「ZeroCode 브라우저에서 열기」 stands its page where the + palette last stood a browser. A split
  // chosen there is the person's hand here too, so a narrow group is divided for it as the palette divides it; the
  // room rule had made it a tab of the group in front (Codex review t-130084, 2026-11-25). A chosen single tab is
  // a tab, as before.
  for (const chosen of ["split", "single"]) {
    const { page, faults } = await standWindow(browser, origin, DEFAULT_WINDOW);
    try {
      const before = await seen(page);
      await page.evaluate((chosen) => {
        rememberBrowserPlacement(chosen);
        jiraItem = { url: "https://example.com/browse/ACME-1" };
        document.getElementById("jira-item-open-app").click();
      }, chosen);
      await page.waitForFunction(() => tabs.some((one) => one.kind === "browser"));
      await settled(page);
      const opened = await seen(page);
      if (chosen === "split") {
        ok("the Jira card's browser door honours a split the person chose in the palette: a narrow group is divided for it",
          before.stage < BESIDE_MIN && opened.browsers === 1 && opened.groups === before.groups + 1
            && opened.lastPane !== before.terminalPane && opened.lastWidth * 2 <= opened.stage,
          JSON.stringify({ rule: BESIDE_MIN, before, opened }));
      } else {
        ok("and a single tab chosen there is a tab of the group in front, the whole group wide",
          opened.browsers === 1 && opened.groups === before.groups && opened.lastPane === before.terminalPane
            && wholeStage(opened.lastWidth, before.stage),
          JSON.stringify({ before, opened }));
      }
      ok(`the Jira card's door (${chosen}) raises no page errors`, faults.length === 0, faults.join("\n"));
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
