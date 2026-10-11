/* The new-terminal door opens a TAB, not a second pane in the tab in front.
 *
 * ⌘T, the 명령 실행 button, the ＋ palette's 새 터미널 row and the empty stage's
 * invitation all ask for `openTermTab({ door: "terminal" })`. Their terminal
 * was seated by the stage's division rule (`tilePlacement`), so each press
 * split the terminal tab in front into two panes — two terminals that looked
 * like the window had opened two. The door is a tab (`createTab`); the rule
 * that divides belongs to launches the stage seats beside the work.
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
}
