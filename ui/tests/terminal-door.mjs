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
 * stage's own split, not in a far-away tab. That case is pressed below.
 *
 * The harness keeps ⌘T unbound by default (its own keybinding overrides carry
 * `terminal.newTab: []`), so this suite lifts those overrides first, as a
 * person's defaults would be. */
export async function testTerminalDoor(browser, origin, standBackend, ok) {
  const page = await browser.newPage();
  const faults = [];
  page.on("pageerror", (error) => faults.push(error?.stack ?? String(error)));
  await standBackend(page);
  await page.addInitScript(() => {
    const base = window.__TAURI__.core.invoke;
    window.__OPENED__ = 0;
    window.__TAURI__.core.invoke = (name, args) => {
      if (name === "open_term_tab") window.__OPENED__ += 1;
      return base(name, args);
    };
  });
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
  const before = await terminals();
  const primary = process.platform === "darwin" ? "Meta" : "Control";

  const opened = [];
  const step = async (label, act) => {
    const opening = await page.evaluate(() => window.__OPENED__);
    await act();
    await page.waitForTimeout(800);
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
  await page.keyboard.press(`${primary}+KeyT`);
  await page.waitForTimeout(800);

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
