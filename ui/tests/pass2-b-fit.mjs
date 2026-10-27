import { openWindowTestPage } from "./window-boot.mjs";
import { nowContrast } from "./board-now.mjs";
import { standPile } from "./stacked-work.mjs";
import { installTurns } from "./your-turn.mjs";

/* What IDE design pass 2 B draws — the 「당신 차례」 chip, its list and the
 * sidebar group, the pane's head, and 「쌓인 일」 — at the five widths the
 * passed draft p-6893560125c34552 folds at (item 17), in dark and in light.
 *
 *  - The chip never leaves: who needs the person now is the one sentence of
 *    the direction (docs/design/direction.md), so it stands at every width.
 *  - A surface on screen stays inside the window and never spills sideways;
 *    one the layout folds away (the sidebar, the right column) is folded, not
 *    cut.
 *  - Every word on those surfaces reads at 4.5:1 or more on what it is drawn
 *    on, in both themes — the Now tab's own reckoning (`nowContrast`).
 *
 *   WINDOW_SUITES=pass2-b-fit node ui/tests/window.mjs */

const WIDTHS = [1700, 1360, 1000, 760, 480];

const SURFACES = Object.freeze({
  chip: "#your-turn",
  list: "#your-turn-list",
  group: "#your-turn-group",
  // The pane's head line: the tab's head on the screen, the conversation's own in the conversation.
  head: ".pane-head:not([hidden]), .pane-chat:not(.is-behind) .chat-head",
  pile: "#activity-pile",
});

const SCOPE = Object.values(SURFACES).join(", ");

/* Each surface as it stands: folded away, or inside the window without a
 * sideways spill. A page function. */
function surfacesFit(surfaces) {
  const seen = {};
  for (const [name, selector] of Object.entries(surfaces)) {
    const node = document.querySelector(selector);
    if (!node?.checkVisibility({ checkOpacity: true, checkVisibilityCSS: true })) {
      seen[name] = "folded";
      continue;
    }
    const box = node.getBoundingClientRect();
    const inside = box.width > 0 && box.left >= -0.5 && box.right <= window.innerWidth + 0.5;
    const spill = node.scrollWidth - node.clientWidth;
    seen[name] = inside && spill <= 1
      ? "fits"
      : `spills ${Math.round(box.left)}..${Math.round(box.right)} of ${window.innerWidth}, ${spill} px over`;
  }
  return seen;
}

/* Three turns waiting, two unlanded checkouts, an agent's pane with its head,
 * the right column on 쌓인 일 and the list dropped open. A page function. */
async function standEverything() {
  window.__ANSWER__.pane_log = (args) => ({
    found: true, next: 1, turns: [{ role: "assistant", text: "다음은 `just gate`입니다." }].slice(args.after ?? 0),
  });
  const term = await openTermTab({ placement: "tab" });
  await window.__PAINTED__();
  for (const handler of window.__LISTENERS__["hook:agent"] ?? []) {
    handler({ payload: { term, state: "working", agent: "claude", session: { id: "s-fit-0001" }, resumable: true, model: "claude-fable-5-1" } });
  }
  await window.__setTurns(window.__turnCards());
  document.getElementById("activity-pile-tab")?.click();
  window.__chip().click();
  await window.__PAINTED__();
  await window.__settle();
}

export async function testPassTwoBFit(browser, origin, ok) {
  const { page } = await openWindowTestPage(browser, origin);
  try {
    await page.setViewportSize({ width: WIDTHS[0], height: 1000 });
    await page.evaluate(installTurns);
    await page.evaluate(standPile);
    await page.evaluate(standEverything);

    const fits = {};
    for (const width of WIDTHS) {
      await page.setViewportSize({ width, height: 1000 });
      await page.evaluate(() => window.__settle());
      fits[width] = await page.evaluate(surfacesFit, SURFACES);
    }
    ok(
      "at 1700, 1360, 1000, 760 and 480 px the chip stands, and every pass-2 B surface on screen fits the window without a sideways spill",
      WIDTHS.every((width) => fits[width].chip === "fits" &&
        Object.values(fits[width]).every((said) => said === "fits" || said === "folded")) &&
        Object.values(fits[WIDTHS[0]]).every((said) => said === "fits"),
      JSON.stringify(fits),
    );

    await page.setViewportSize({ width: WIDTHS[0], height: 1000 });
    const contrast = {};
    for (const theme of ["dark", "light"]) {
      await page.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" });
      await page.evaluate(async (name) => {
        document.documentElement.dataset.theme = name;
        await window.__settle();
      }, theme);
      contrast[theme] = await page.evaluate(nowContrast, SCOPE);
    }
    ok(
      "every word on the chip, the list, the group, the pane head and 쌓인 일 reads at 4.5:1 or more, in dark and in light",
      ["dark", "light"].every((theme) => contrast[theme].min >= 4.5 && contrast[theme].below.length === 0),
      JSON.stringify(contrast),
    );
  } finally { await page.close(); }
}
