/* ---- where the model menu stands -------------------------------------------
 *
 * The agent chip opens the model list (`openComposerMenu`, shell-composer.js).
 * The extension's rule for that popup is the plain one every anchored menu
 * keeps: it stands ON the chip that opened it, it stays inside the window, it
 * flips to the other side of the chip when its own side has no room, and it
 * goes on standing on the chip when the window is resized under it.
 *
 * These four are measured, not eyeballed: the chip's box and the menu's box
 * are read with `getBoundingClientRect` in a real Chromium, and the numbers
 * are compared. A popup that opens 300px away from its chip is a bug whether
 * or not anyone has screenshotted it. */

/* The menu is allowed this much air between itself and the chip — the gap the
 * painter leaves, plus a pixel of rounding. Anything more means it is hung off
 * something other than the chip. */
const CHIP_GAP_LIMIT = 14;
/* Edge alignment is exact up to sub-pixel layout. */
const EDGE_SLACK = 2;

/* Open the chip's menu on a settled page and hand back both boxes. */
const MEASURE = `(async () => {
  const settle = (ms) => new Promise((done) => setTimeout(done, ms));
  const chat = document.querySelector('.pane-slot[data-term="' + window.__TERM__ + '"] .pane-chat');
  const composer = chat?.querySelector(".worker-composer");
  const chip = composer?.querySelector(".worker-composer-agent");
  if (!chip) return { error: "no chip" };
  if (composer.querySelector(".composer-menu")) { chip.click(); await settle(80); }
  chip.click();
  await settle(260);
  const menu = composer.querySelector(".composer-menu");
  if (!menu) return { error: "no menu" };
  const c = chip.getBoundingClientRect();
  const m = menu.getBoundingClientRect();
  const round = (box) => ({ l: Math.round(box.left), r: Math.round(box.right), t: Math.round(box.top), b: Math.round(box.bottom) });
  return { chip: round(c), menu: round(m), vw: Math.round(innerWidth), vh: Math.round(innerHeight) };
})()`;

/* The three numbers every assertion below reads. */
function readings(seen) {
  const { chip, menu, vw, vh } = seen;
  const above = chip.t - menu.b; // air between the menu's foot and the chip's head
  const below = menu.t - chip.b; // air between the chip's foot and the menu's head
  return {
    leftOffBy: Math.abs(menu.l - chip.l),
    // The menu sits on one side of the chip or the other; the gap is whichever
    // side it took. A menu that overlaps the chip has neither.
    gap: above >= 0 ? above : below >= 0 ? below : -1,
    onChip: (above >= 0 && above <= CHIP_GAP_LIMIT) || (below >= 0 && below <= CHIP_GAP_LIMIT),
    flippedBelow: below >= 0,
    inWindow: menu.l >= -EDGE_SLACK && menu.r <= vw + EDGE_SLACK && menu.t >= -EDGE_SLACK && menu.b <= vh + EDGE_SLACK,
    offTopBy: menu.t < 0 ? -menu.t : 0,
    offLeftBy: menu.l < 0 ? -menu.l : 0,
    offRightBy: menu.r > vw ? menu.r - vw : 0,
  };
}

export async function testComposerMenuPosition(page, ok) {
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.evaluate(async () => {
    const tell = (name, payload) => {
      for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload });
    };
    const settle = (ms) => new Promise((done) => setTimeout(done, ms));
    window.__ANSWER__.term_paste = () => null;
    window.__ANSWER__.term_key = () => null;
    // A catalog long enough that the list has real height, as Claude's has.
    window.__ANSWER__.agent_models = () => Array.from({ length: 14 }, (_, at) => ({
      id: `m-${at}`, display_name: `Model number ${at}`, provider: "claude",
    }));
    window.__ANSWER__.pane_log = () => ({ found: true, next: 2, turns: [{ role: "user", text: "hi" }] });
    const term = await openTermTab({ placement: "tab" });
    tell("hook:agent", { term, state: "working", agent: "claude", session: "s-menu", resumable: false, model: "m-0", permission_mode: "bypassPermissions" });
    await window.__PAINTED__();
    el("view-toggle-chat").click();
    await settle(220);
    window.__TERM__ = term;
  });

  // 1. The popup stands on the chip that opened it.
  const resting = await page.evaluate(MEASURE);
  const rest = readings(resting);
  ok(
    "the model menu is anchored to the agent chip that opened it — its left edge on the chip's left edge, its foot on the chip",
    rest.leftOffBy <= EDGE_SLACK && rest.onChip,
    JSON.stringify({ ...resting, ...rest }),
  );

  // 2. No room above: it flips under the chip instead of off the top edge.
  //    A short conversation pinned to the top of the window is a split pane's
  //    share of the screen — the composer sits near the top edge.
  await page.evaluate(async () => {
    const settle = (ms) => new Promise((done) => setTimeout(done, ms));
    const chat = document.querySelector(`.pane-slot[data-term="${window.__TERM__}"] .pane-chat`);
    Object.assign(chat.style, { position: "fixed", left: "40px", top: "0px", width: "520px", height: "230px", zIndex: "100" });
    await settle(200);
  });
  const pinned = await page.evaluate(MEASURE);
  const flip = readings(pinned);
  ok(
    "with no room above it, the model menu flips below the chip and stays inside the window instead of running off the top edge",
    flip.flippedBelow && flip.inWindow && flip.onChip,
    JSON.stringify({ ...pinned, ...flip }),
  );

  // 3. A narrow pane: the list is wider than the composer, and must not spill
  //    off the window's left edge.
  await page.evaluate(async () => {
    const settle = (ms) => new Promise((done) => setTimeout(done, ms));
    const chat = document.querySelector(`.pane-slot[data-term="${window.__TERM__}"] .pane-chat`);
    Object.assign(chat.style, { position: "fixed", left: "0px", top: "300px", width: "210px", height: "430px", zIndex: "100" });
    await settle(200);
  });
  const narrow = await page.evaluate(MEASURE);
  const clamp = readings(narrow);
  ok(
    "in a pane narrower than the list, the model menu is held inside the window instead of spilling past its edge",
    clamp.inWindow,
    JSON.stringify({ ...narrow, ...clamp }),
  );

  // 4. The window is resized under an open menu: it goes on standing on the chip.
  await page.evaluate(async () => {
    const settle = (ms) => new Promise((done) => setTimeout(done, ms));
    const chat = document.querySelector(`.pane-slot[data-term="${window.__TERM__}"] .pane-chat`);
    chat.style.cssText = "";
    await settle(200);
  });
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.evaluate(MEASURE); // leaves the menu open
  await page.setViewportSize({ width: 720, height: 660 });
  await page.waitForTimeout(260);
  const moved = await page.evaluate(() => {
    const chat = document.querySelector(`.pane-slot[data-term="${window.__TERM__}"] .pane-chat`);
    const composer = chat?.querySelector(".worker-composer");
    const chip = composer?.querySelector(".worker-composer-agent");
    const menu = composer?.querySelector(".composer-menu");
    if (!menu) return { error: "the menu closed itself on resize" };
    const round = (box) => ({ l: Math.round(box.left), r: Math.round(box.right), t: Math.round(box.top), b: Math.round(box.bottom) });
    return { chip: round(chip.getBoundingClientRect()), menu: round(menu.getBoundingClientRect()), vw: Math.round(innerWidth), vh: Math.round(innerHeight) };
  });
  const follow = moved.error ? null : readings(moved);
  ok(
    "an open model menu follows its chip when the window is resized under it, rather than staying where the chip used to be",
    Boolean(follow) && follow.leftOffBy <= EDGE_SLACK && follow.onChip && follow.inWindow,
    JSON.stringify({ ...moved, ...(follow ?? {}) }),
  );

  // 5. The `/` palette is the composer's other popup, and it has the same edge
  //    to fall off. It spans the composer rather than hanging off a chip, so
  //    its rule is only the vertical one: above the box where there is room,
  //    below it where there is not, never off the top of the window.
  await page.evaluate(async () => {
    const settle = (ms) => new Promise((done) => setTimeout(done, ms));
    window.__ANSWER__.slash_commands = () => ({
      version: "1.0",
      snapshot_version: null,
      source: "cli",
      commands: Array.from({ length: 12 }, (_, at) => ({
        name: `command-${at}`,
        args: "",
        about: `The ${at}th thing this CLI can be told to do`,
      })),
    });
    const chat = document.querySelector(`.pane-slot[data-term="${window.__TERM__}"] .pane-chat`);
    chat.style.cssText = "";
    // Close the model menu, so the two popups are never measured together.
    chat.querySelector(".worker-composer-agent")?.click();
    await settle(120);
    Object.assign(chat.style, { position: "fixed", left: "40px", top: "0px", width: "520px", height: "230px", zIndex: "100" });
    await settle(200);
  });
  const palette = await page.evaluate(async () => {
    const settle = (ms) => new Promise((done) => setTimeout(done, ms));
    const chat = document.querySelector(`.pane-slot[data-term="${window.__TERM__}"] .pane-chat`);
    const composer = chat?.querySelector(".worker-composer");
    composer?.querySelector(".worker-composer-slash")?.click();
    await settle(340);
    const pane = composer?.querySelector(".composer-slash");
    if (!pane || pane.hidden) return { error: "the palette did not open" };
    const box = composer.querySelector("textarea") ?? composer;
    const round = (rect) => ({ l: Math.round(rect.left), r: Math.round(rect.right), t: Math.round(rect.top), b: Math.round(rect.bottom) });
    return { pane: round(pane.getBoundingClientRect()), box: round(box.getBoundingClientRect()), vw: Math.round(innerWidth), vh: Math.round(innerHeight) };
  });
  const spill = palette.error
    ? null
    : {
        offTopBy: palette.pane.t < 0 ? -palette.pane.t : 0,
        offBottomBy: palette.pane.b > palette.vh ? palette.pane.b - palette.vh : 0,
        flippedBelow: palette.pane.t >= palette.box.b - EDGE_SLACK,
      };
  ok(
    "with no room above it, the `/` palette flips below the box and stays inside the window instead of running off the top edge",
    Boolean(spill) && spill.offTopBy === 0 && spill.offBottomBy === 0 && spill.flippedBelow,
    JSON.stringify({ ...palette, ...(spill ?? {}) }),
  );
}
