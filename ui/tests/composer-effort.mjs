/* The composer's effort chip (t-81532, the person's 04:1x ask: 「노력표시가
 * 안보이는데 노력표시도 바꿔서 쓸수있게」). Beside the model chip of a pane's
 * conversation stands the effort the pane's session last ran a turn at, as the
 * backend reads it (`pane_effort`); pressing it opens the rungs, and a rung
 * picked on a road that rewrites the cache asks once more with the cost in
 * front of the person before `pane_effort_press` goes. A turn under way, and
 * a CLI no typed key moves, leave every rung shown and none pressable, with
 * the reason said. The keyboard reaches all of it.
 *
 * `COMPOSER_EFFORT_SHOTS=<folder>` keeps a picture of the open menu in each
 * treatment there; without it nothing is written. */
import { mkdirSync } from "node:fs";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";

const LADDER = ["low", "medium", "high", "xhigh", "max"];

/* A Claude pane whose session last ran at `high`, at rest. */
const CLAUDE_AT_REST = Object.freeze({
  current: "high", witness: "turn", ladder: LADDER, door: "picker", command: null,
  stuck: null, rewritesCache: true, pending: null,
});

/* Open a pane's conversation with the backend answering `said` for its
 * effort, and count what the window asks. */
async function openPane(page, said) {
  await page.evaluate(async (said) => {
    const tell = (name, payload) => {
      for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload });
    };
    const settle = (ms) => new Promise((done) => setTimeout(done, ms));
    window.__EFFORT__ = { said, asks: 0, presses: [] };
    window.__ANSWER__.term_paste = () => null;
    window.__ANSWER__.term_key = () => null;
    window.__ANSWER__.agent_models = () => [{ id: "claude-opus-5-5", display_name: "Opus 5.5", provider: "claude" }];
    window.__ANSWER__.pane_log = () => ({ found: true, next: 2, turns: [{ role: "user", text: "hi" }], usage: { tokens: 120000, window: 1000000 } });
    window.__ANSWER__.pane_effort = () => {
      window.__EFFORT__.asks += 1;
      return window.__EFFORT__.said;
    };
    window.__ANSWER__.pane_effort_press = (args) => {
      window.__EFFORT__.presses.push(args);
      window.__EFFORT__.said = { ...window.__EFFORT__.said, pending: args.to };
      return window.__EFFORT__.said;
    };
    const term = await openTermTab({ placement: "tab" });
    tell("hook:agent", { term, state: "done", agent: "claude", session: "s-effort", resumable: false, model: "claude-opus-5-5", permission_mode: "default" });
    await window.__PAINTED__();
    el("view-toggle-chat").click();
    await settle(240);
    window.__TERM__ = term;
    window.__TELL__ = tell;
  }, said);
}

/* The chip and what it says, read off the page. */
function readChip() {
  const chat = document.querySelector(`.pane-slot[data-term="${window.__TERM__}"] .pane-chat`);
  const composer = chat?.querySelector(".worker-composer");
  const chip = composer?.querySelector(".worker-composer-effort");
  if (!chip) return { error: "no effort chip" };
  const menu = document.querySelector(".composer-menu");
  const chevron = chip.querySelector(".worker-composer-chevron");
  return {
    hidden: chip.hidden,
    words: chip.querySelector(".worker-composer-pill-words")?.textContent ?? "",
    label: chip.getAttribute("aria-label") ?? "",
    tip: chip.dataset.tip ?? "",
    popup: chip.getAttribute("aria-haspopup"),
    expanded: chip.getAttribute("aria-expanded"),
    afterModel: chip.previousElementSibling?.classList.contains("worker-composer-agent") ?? false,
    chevronShown: chevron ? getComputedStyle(chevron).visibility === "visible" : false,
    focused: document.activeElement === chip,
    menu: menu
      ? {
          items: [...menu.querySelectorAll(".composer-menu-item")].map((item) => ({
            words: item.querySelector(".composer-menu-name")?.textContent ?? "",
            current: item.getAttribute("aria-current") === "true",
            disabled: item.getAttribute("aria-disabled") === "true",
            role: item.getAttribute("role"),
            focused: document.activeElement === item,
          })),
          notes: [...menu.querySelectorAll(".composer-menu-empty")].map((note) => ({ kind: note.className, words: note.textContent })),
          head: menu.querySelector(".composer-menu-head")?.textContent ?? "",
        }
      : null,
    menus: document.querySelectorAll(".composer-menu").length,
    asks: window.__EFFORT__.asks,
    presses: window.__EFFORT__.presses,
  };
}

/* Settle the page for a few paints. */
async function settle(page, ms = 160) {
  await page.evaluate(async (ms) => {
    await new Promise((done) => setTimeout(done, ms));
    for (let beat = 0; beat < 2; beat += 1) await window.__PAINTED__();
  }, ms);
}

/* Focus the item whose words are `words` and press `key` on it. */
async function keyOnItem(page, words, key) {
  await page.evaluate((words) => {
    const item = [...document.querySelectorAll(".composer-menu .composer-menu-item")]
      .find((one) => one.querySelector(".composer-menu-name")?.textContent === words);
    item?.focus();
  }, words);
  await page.keyboard.press(key);
  await settle(page);
}

export async function testComposerEffort(browser, origin, ok, { shots = process.env.COMPOSER_EFFORT_SHOTS ?? "" } = {}) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.setViewportSize({ width: 1280, height: 900 });
    await openPane(page, CLAUDE_AT_REST);
    await settle(page);
    const shown = await page.evaluate(readChip);
    ok("effort: the composer wears the pane's effort beside its model — the session's own word, named for a screen reader, opening a menu",
      !shown.error && !shown.hidden && shown.words === "high" && shown.afterModel && shown.popup === "menu" &&
        shown.label.includes("high") && shown.label === shown.tip && shown.chevronShown,
      JSON.stringify(shown));

    // The turn's edges ask again; the dozens of events inside one do not.
    const asked = shown.asks;
    await page.evaluate(async () => {
      const at = { term: window.__TERM__, agent: "claude", session: "s-effort", resumable: false, model: "claude-opus-5-5" };
      window.__TELL__("hook:agent", { ...at, state: "working" });
      for (let event = 0; event < 6; event += 1) window.__TELL__("hook:agent", { ...at, state: "working" });
      await window.__PAINTED__();
    });
    await settle(page);
    const working = await page.evaluate(readChip);
    ok("effort: a turn starting asks the backend once, and the tool events inside the turn ask nothing",
      working.asks === asked + 1, JSON.stringify({ before: asked, after: working.asks }));
    ok("effort: while a turn runs the chip says the word and that it changes between turns",
      working.words === "high" && working.tip.includes("high") && working.tip !== shown.tip, JSON.stringify(working));

    // Opened mid-turn: every rung shown, none pressable, the reason said.
    await page.evaluate(() => document.querySelector(`.pane-slot[data-term="${window.__TERM__}"] .worker-composer-effort`).focus());
    await page.keyboard.press("Enter");
    await settle(page, 240);
    const midTurn = await page.evaluate(readChip);
    ok("effort: opened by Enter mid-turn, the menu lists every rung with the current one ticked, none pressable, and says why",
      midTurn.menu?.items.length === LADDER.length && midTurn.menu.items.every((item) => item.disabled && item.role === "menuitem") &&
        midTurn.menu.items.find((item) => item.current)?.words === "high" &&
        midTurn.menu.notes.some((note) => note.kind.includes("composer-effort-stuck")) && midTurn.expanded === "true",
      JSON.stringify(midTurn.menu));
    await keyOnItem(page, "max", "Enter");
    const refused = await page.evaluate(readChip);
    ok("effort: pressing a rung mid-turn sends nothing", refused.presses.length === 0, JSON.stringify(refused.presses));
    await page.keyboard.press("Escape");
    await settle(page);

    // The turn ends: the edge asks again, and the rungs are offered.
    await page.evaluate(async () => {
      window.__TELL__("hook:agent", { term: window.__TERM__, state: "done", agent: "claude", session: "s-effort", resumable: false, model: "claude-opus-5-5" });
      await window.__PAINTED__();
    });
    await settle(page);
    await page.evaluate(() => document.querySelector(`.pane-slot[data-term="${window.__TERM__}"] .worker-composer-effort`).focus());
    await page.keyboard.press("Space");
    await settle(page, 240);
    const atRest = await page.evaluate(readChip);
    ok("effort: opened by Space between turns, the other rungs are pressable, the focus stands on the current one, and the cost of a move is said in one line",
      atRest.menu?.items.filter((item) => !item.disabled).length === LADDER.length - 1 &&
        atRest.menu.items.find((item) => item.focused)?.words === "high" &&
        atRest.menu.notes.some((note) => note.kind.includes("composer-effort-cost") && note.words.includes("120,000")),
      JSON.stringify(atRest.menu));
    if (shots) {
      mkdirSync(shots, { recursive: true });
      for (const theme of ["dark", "light"]) {
        await page.evaluate((theme) => { document.documentElement.dataset.theme = theme; }, theme);
        await settle(page);
        await page.screenshot({ path: join(shots, `after-menu-${theme}.png`) });
      }
      await page.evaluate(() => { document.documentElement.dataset.theme = "dark"; });
    }

    // A rung picked: the menu asks once more with the cost; only its yes presses.
    await keyOnItem(page, "max", "Enter");
    const confirm = await page.evaluate(readChip);
    ok("effort: a rung picked on a road that rewrites the cache asks once more, the cost in front of the person, and sends nothing yet",
      confirm.presses.length === 0 && confirm.menu?.items.length === 2 && confirm.menu.head.includes("max") &&
        confirm.menu.notes.some((note) => note.kind.includes("composer-effort-cost")),
      JSON.stringify(confirm.menu));
    await keyOnItem(page, confirm.menu?.items[0]?.words ?? "", "Enter");
    await settle(page, 200);
    const pressed = await page.evaluate(readChip);
    ok("effort: the yes presses once, for this pane and that rung, and the chip says the rung it moved to",
      pressed.presses.length === 1 && pressed.presses[0].to === "max" && pressed.presses[0].term === await page.evaluate(() => window.__TERM__) &&
        pressed.words === "max" && pressed.menu === null,
      JSON.stringify(pressed));
    if (shots) {
      for (const theme of ["dark", "light"]) {
        await page.evaluate((theme) => { document.documentElement.dataset.theme = theme; }, theme);
        await settle(page);
        const chat = page.locator(`.pane-slot .pane-chat .worker-composer`).first();
        await chat.screenshot({ path: join(shots, `after-chip-${theme}.png`) });
      }
      await page.evaluate(() => { document.documentElement.dataset.theme = "dark"; });
    }

    // A CLI no typed key moves: shown, no chevron, every rung shut with the reason.
    await page.evaluate(async () => {
      window.__EFFORT__.said = { current: "xhigh", witness: "summons", ladder: ["minimal", "low", "medium", "high", "xhigh", "max", "ultra"], door: "relaunch", command: null, stuck: "relaunch", rewritesCache: false, pending: null };
      window.__TELL__("hook:agent", { term: window.__TERM__, state: "working", agent: "claude", session: "s-effort", resumable: false });
      window.__TELL__("hook:agent", { term: window.__TERM__, state: "done", agent: "claude", session: "s-effort", resumable: false });
      await window.__PAINTED__();
    });
    await settle(page, 200);
    await page.evaluate(() => document.querySelector(`.pane-slot[data-term="${window.__TERM__}"] .worker-composer-effort`).click());
    await settle(page, 240);
    const relaunch = await page.evaluate(readChip);
    ok("effort: a CLI whose effort only a fresh launch sets is shown — its word, no chevron — and every rung is shut with that reason",
      relaunch.words === "xhigh" && !relaunch.chevronShown && relaunch.menu?.items.every((item) => item.disabled) &&
        relaunch.menu.notes.some((note) => note.kind.includes("composer-effort-stuck")) &&
        !relaunch.menu.notes.some((note) => note.kind.includes("composer-effort-cost")),
      JSON.stringify(relaunch));
    await keyOnItem(page, "low", "Enter");
    const shut = await page.evaluate(readChip);
    ok("effort: pressing a shut rung sends nothing", shut.presses.length === 1, JSON.stringify(shut.presses));
    ok("effort: the page raised no errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  let failed = 0;
  try {
    await testComposerEffort(browser, origin, (name, pass, details = "") => {
      console.log(`${pass ? "PASS" : "FAIL"} ${name}${!pass ? `\n  ${details}` : ""}`);
      if (!pass) failed++;
    });
  } finally {
    await browser.close();
    files.close();
  }
  if (failed) process.exitCode = 1;
}
