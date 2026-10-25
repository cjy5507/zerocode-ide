// Simulate a native terminal open that takes longer than the former 800 ms nap.
const TERMINAL_OPEN_DELAY_MS = 1200;
const TERMINAL_OPEN_TIMEOUT_MS = 5000;

/* The new-terminal door opens a TAB, not a second pane in the tab in front.
 *
 * ⌘T and the 명령 실행 button — the two doors this suite presses — ask for
 * `openTermTab({ door: "terminal" })`. Their terminal was seated by the
 * stage's division rule (`tilePlacement`), so each press split the terminal
 * tab in front into two panes — two terminals that looked like the window had
 * opened two. The door is a tab (`createTab`); the rule that divides belongs
 * to launches the stage seats beside the work.
 *
 * A browser in front is the other case: the terminal stands beside it, in the
 * stage's own split, not in a far-away tab. That case is pressed below, by ⌘T
 * and by the file tree's 「터미널에서 열기」, the third door to a by-hand shell.
 *
 * The harness keeps ⌘T unbound by default (its own keybinding overrides carry
 * `terminal.newTab: []`), so this suite lifts those overrides first, as a
 * person's defaults would be. */
export async function testTerminalDoor(browser, origin, standBackend, ok) {
  const page = await browser.newPage();
  const faults = [];
  page.on("pageerror", (error) => faults.push(error?.stack ?? String(error)));
  await standBackend(page);
  await page.addInitScript((openDelayMs) => {
    const base = window.__TAURI__.core.invoke;
    window.__OPENED__ = 0;
    window.__TAURI__.core.invoke = (name, args) => {
      if (name === "open_term_tab") window.__OPENED__ += 1;
      if (name === "open_term_tab") {
        return new Promise((resolve) => setTimeout(resolve, openDelayMs)).then(() => base(name, args));
      }
      return base(name, args);
    };
  }, TERMINAL_OPEN_DELAY_MS);
  await page.goto(`${origin}/index.html`);
  await page.waitForFunction(() => typeof BOUND !== "undefined" && BOUND.size > 0);
  await page.evaluate(() => {
    keybindingOverrides = {};
    rebuildBound();
  });

  const terminals = () =>
    page.evaluate(() => ({
      tabs: tabs.filter((tab) => tab.kind === "term").length,
      panes: tabs.filter((tab) => tab.kind === "term").map((tab) => JSON.stringify(tab.layout).split('"type":"leaf"').length - 1),
    }));
  await page.waitForFunction(() => tabs.some((tab) => tab.kind === "term"), null,
    { timeout: TERMINAL_OPEN_TIMEOUT_MS });
  const before = await terminals();
  const primary = process.platform === "darwin" ? "Meta" : "Control";

  const opened = [];
  const step = async (label, act) => {
    const opening = await page.evaluate(() => window.__OPENED__);
    const tabCount = (await terminals()).tabs;
    await act();
    await page.waitForFunction((before) => tabs.filter((tab) => tab.kind === "term").length > before,
      tabCount, { timeout: TERMINAL_OPEN_TIMEOUT_MS });
    const after = await terminals();
    const count = (await page.evaluate(() => window.__OPENED__)) - opening;
    opened.push({ label, count, after });
  };
  await step("chord", () => page.keyboard.press(`${primary}+KeyT`));
  await step("chord-again", () => page.keyboard.press(`${primary}+KeyT`));
  await step("run-button", () => page.evaluate(() => document.getElementById("run-command").click()));

  const shown = JSON.stringify({ before, opened });
  ok(
    "each new-terminal press opens exactly one terminal",
    opened.every((step) => step.count === 1),
    shown,
  );
  ok(
    "each new-terminal press adds a tab, and no tab in front is divided",
    opened.every((step, at) => step.after.tabs === before.tabs + at + 1)
      && opened.every((step) => step.after.panes.every((count) => count === 1)),
    shown,
  );
  ok("the terminal doors raise no script errors", faults.length === 0, faults.join("\n"));
  await page.close();
  await testTerminalBesideBrowser(browser, origin, standBackend, ok);
  await testTreeTerminal(browser, origin, standBackend, ok);
}

/* A browser in front is the host a new terminal is asked beside (the stage's
 * own split, t-6379): ⌘T leaves the page in front and its terminal stands in a
 * group beside it. A terminal whose placement said "tab" looked nowhere ahead,
 * so it opened as a far-away tab instead (live report 2026-08-14: + over NAVER
 * opened a far-away tab). */
async function testTerminalBesideBrowser(browser, origin, standBackend, ok) {
  const page = await browser.newPage();
  const faults = [];
  page.on("pageerror", (error) => faults.push(error?.stack ?? String(error)));
  await standBackend(page);
  await page.goto(`${origin}/index.html`);
  await page.waitForFunction(() => typeof BOUND !== "undefined" && BOUND.size > 0);
  await page.evaluate(() => {
    keybindingOverrides = {};
    rebuildBound();
  });
  await page.evaluate(() => openBrowserTab("about:blank"));
  await page.waitForFunction(() => tabs.some((tab) => tab.kind === "browser"));

  const primary = process.platform === "darwin" ? "Meta" : "Control";
  const tabCount = await page.evaluate(() => tabs.filter((tab) => tab.kind === "term").length);
  await page.keyboard.press(`${primary}+KeyT`);
  await page.waitForFunction((before) => tabs.filter((tab) => tab.kind === "term").length > before,
    tabCount, { timeout: TERMINAL_OPEN_TIMEOUT_MS });

  const seen = await page.evaluate(() => {
    const leaves = (node) => (node.type === "leaf" ? 1 : leaves(node.first) + leaves(node.second));
    // Every tab names the group it belongs to (`pane`), so "beside" is read off
    // the groups: the terminal is in another group than the browser, and the
    // stage is divided into two.
    const term = [...tabs].reverse().find((tab) => tab.kind === "term");
    const browser = tabs.find((tab) => tab.kind === "browser");
    return {
      beside: term !== undefined && browser !== undefined && term.pane !== browser.pane,
      stagePanes: leaves(stageTree()),
      browserStays: browser !== undefined,
    };
  });
  const shown = JSON.stringify(seen);
  ok(
    "a terminal asked for while a browser is in front stands beside it, not in a far-away tab",
    seen.beside && seen.stagePanes === 2,
    shown,
  );
  ok(
    "the browser in front stays open beside the new terminal, and the door raises no script errors",
    seen.browserStays && faults.length === 0,
    shown + (faults.length ? "\n" + faults.join("\n") : ""),
  );
  await page.close();
}

/* The file tree's 「터미널에서 열기」 is the third door to a by-hand shell
 * (t-47782): a bare shell started in the folder, opened as a TAB — Orca's
 * `createNewTerminalTab(…, { startupCwd })` — never a division of the pane that
 * holds the keyboard. It said "tab" for that, the word that looks nowhere
 * ahead, so with a browser in front the folder's shell went to a far-away tab
 * while ⌘T's stood beside the page. Both fronts are pressed, each on a page of
 * its own: a browser in front gets the shell beside it, a terminal in front
 * gets one more tab. */
async function testTreeTerminal(browser, origin, standBackend, ok) {
  const browserInFront = await pressTreeTerminal(browser, origin, standBackend, "browser");
  const terminalInFront = await pressTreeTerminal(browser, origin, standBackend, "terminal");
  const { before, seen } = browserInFront;
  const shown = JSON.stringify(browserInFront);
  ok(
    "the file tree's Open in Terminal with a browser in front seats the folder's shell beside the browser, not in a far-away tab",
    seen.offered && seen.ownTab && seen.group !== seen.browserGroup && seen.stagePanes === 2,
    shown,
  );
  ok(
    "the browser in front stays shown in its own group and gains no far-away terminal tab behind it",
    seen.browserShown && seen.termTabsWithBrowser === before.termTabsWithBrowser,
    shown,
  );
  const terminal = terminalInFront.seen;
  const was = terminalInFront.before;
  ok(
    "the file tree's Open in Terminal with a terminal in front opens one new tab and divides no terminal",
    terminal.offered && terminal.ownTab && !terminal.divided
      && terminal.termTabs === was.termTabs + 1 && terminal.stagePanes === was.stagePanes,
    JSON.stringify(terminalInFront),
  );
  ok(
    "the file tree's terminal is a bare shell started in the folder whose menu was opened",
    [seen, terminal].every((one) => one.shell?.cwd === one.folder && one.shell?.plain === true),
    JSON.stringify([seen.shell, seen.folder, terminal.shell, terminal.folder]),
  );
  const faults = [...browserInFront.faults, ...terminalInFront.faults];
  ok("the file tree's terminal door raises no script errors", faults.length === 0, faults.join("\n"));
}

/* One press of the tree's 「터미널에서 열기」 on a folder, with `front` ("browser"
 * or "terminal") in front of the focused group, and what the stage holds before
 * and after. Each shell the window asks for is written down with the folder it
 * starts in, so the press is found by its answer rather than by a count. */
async function pressTreeTerminal(browser, origin, standBackend, front) {
  const page = await browser.newPage();
  const faults = [];
  page.on("pageerror", (error) => faults.push(error?.stack ?? String(error)));
  await standBackend(page);
  await page.addInitScript(() => {
    const base = window.__TAURI__.core.invoke;
    window.__SHELLS__ = [];
    window.__TAURI__.core.invoke = async (name, args, options) => {
      const answer = await base(name, args, options);
      if (name === "open_term_tab") {
        window.__SHELLS__.push({ term: answer, cwd: args?.cwd ?? null, plain: args?.plain ?? null });
      }
      return answer;
    };
  });
  await page.goto(`${origin}/index.html`);
  await page.waitForFunction(() => typeof BOUND !== "undefined" && BOUND.size > 0);
  // The window seats its first terminal on boot; a case starts once it has, so
  // what is opened below comes after it on the strip.
  await page.waitForFunction(() => tabs.some((tab) => tab.kind === "term"), null, { timeout: 5000 }).catch(() => {});
  // A checkout with one folder, on the files panel.
  await page.evaluate(async () => {
    window.__ANSWER__.list_dir = ({ path }) =>
      path === "" ? [{ name: "src", is_dir: true }, { name: "README.md", is_dir: false }] : [];
    revealActivity("files", "name");
    fileSearch.blur();
    await loadTree(fileTree, "");
  });
  if (front === "browser") {
    await page.evaluate(() => openBrowserTab("about:blank"));
    await page.waitForFunction(() => tabs.some((tab) => tab.kind === "browser"));
  } else {
    await page.evaluate(async () => {
      if (currentTab()?.kind !== "term") await openTermTab({ door: "terminal" });
    });
    await page.waitForFunction(() => currentTab()?.kind === "term");
  }
  const result = await page.evaluate(async () => {
    const leaves = (node) => (node.type === "leaf" ? 1 : leaves(node.first) + leaves(node.second));
    const stage = () => {
      const terms = tabs.filter((tab) => tab.kind === "term");
      const browserTab = tabs.find((tab) => tab.kind === "browser") ?? null;
      return {
        frontGroup: focusedPane,
        groups: stageGroups(),
        seats: tabs.map((tab) => `${tab.kind}:${tab.pane}`),
        stagePanes: leaves(stageTree()),
        termTabs: terms.length,
        termTabsWithBrowser: browserTab === null ? null : terms.filter((tab) => tab.pane === browserTab.pane).length,
      };
    };
    const before = stage();
    const folder = `${activeWorktreePath}/src`;
    const asked = window.__SHELLS__.length;
    const shellOfPress = () => window.__SHELLS__.slice(asked).find((one) => one.cwd === folder) ?? null;
    const seatOf = (term) =>
      tabs.find((tab) => tab.kind === "term" && paneLeaves(tab.layout).includes(term)) ?? null;
    fileTree
      .querySelector('.tree-row[data-tree-path="src"]')
      ?.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 40, clientY: 40 }));
    const item = [...document.querySelectorAll("#sidebar-menu .sidebar-menu-item")]
      .find((one) => one.textContent.includes(t("tree.menu.openInTerminal", "터미널에서 열기")));
    if (!item) {
      closeSidebarMenu();
      return { before, seen: { offered: false } };
    }
    item.click();
    const deadline = performance.now() + 4000;
    while (performance.now() < deadline && !(shellOfPress() && seatOf(shellOfPress().term))) {
      await new Promise((done) => setTimeout(done, 50));
    }
    const shell = shellOfPress();
    const seat = shell === null ? null : seatOf(shell.term);
    const browserTab = tabs.find((tab) => tab.kind === "browser") ?? null;
    return {
      before,
      seen: {
        offered: true,
        folder,
        shell,
        ownTab: seat !== null && paneLeaves(seat.layout).length === 1,
        group: seat?.pane ?? null,
        browserGroup: browserTab?.pane ?? null,
        browserShown: browserTab !== null && activeTabIn(browserTab.pane)?.id === browserTab.id,
        divided: tabs.some((tab) => tab.kind === "term" && paneLeaves(tab.layout).length > 1),
        ...stage(),
      },
    };
  });
  await page.close();
  return { ...result, faults };
}
