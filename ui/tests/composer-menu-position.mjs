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

/* ---- how the composer's menus open and close (2026-11-08) ------------------
 *
 * The person: 「모델 열고 effort 열고 닫을때 재대로 안열리고 렉이 걸림」. Four
 * things a hand does, each read off the page:
 *  1. A press on the chip's words closes the menu it opened. The press counted
 *     as one outside the menu and closed it on pointerdown, and the click that
 *     followed opened it again, so only Escape or the chip's padding closed it.
 *  2. A list scrolled stays where it was scrolled. Standing the menu again —
 *     on its own scroll, on the page scrolling under it — measured it at full
 *     height first, which threw the list back to its top.
 *  3. Two opens that answer out of order leave one menu. The effort chip asks
 *     the backend before its menu stands; a model chip pressed meanwhile stood
 *     its own, then the late effort menu stood beside it, and the first could
 *     no longer be closed.
 *  4. A burst of scrolls under an open menu stands it once a frame, not once a
 *     scroll: each stand lays the whole page out three times, and a streaming
 *     conversation scrolls on every word. */
export async function testComposerMenuHandling(page, ok) {
  await page.setViewportSize({ width: 1280, height: 900 });
  const term = await page.evaluate(async () => {
    const tell = (name, payload) => {
      for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload });
    };
    const settle = (ms) => new Promise((done) => setTimeout(done, ms));
    window.__ANSWER__.term_paste = () => null;
    window.__ANSWER__.term_key = () => null;
    window.__ANSWER__.agent_models = () => Array.from({ length: 30 }, (_, at) => ({
      id: `m-${at}`, display_name: `Model number ${at}`, provider: "claude",
    }));
    window.__ANSWER__.pane_log = () => ({ found: true, next: 2, turns: [{ role: "user", text: "hi" }] });
    window.__EFFORT_WAIT__ = 0;
    window.__ANSWER__.pane_effort = async () => {
      await settle(window.__EFFORT_WAIT__);
      return {
        current: "high", witness: "turn", ladder: ["low", "medium", "high"], door: "picker", command: null,
        stuck: null, rewritesCache: false, pending: null,
      };
    };
    const term = await openTermTab({ placement: "tab" });
    tell("hook:agent", { term, state: "done", agent: "claude", session: "s-menu-hand", resumable: false, model: "m-0", permission_mode: "default" });
    await window.__PAINTED__();
    el("view-toggle-chat").click();
    await settle(240);
    window.__TERM__ = term;
    return term;
  });
  const chip = `.pane-slot[data-term="${term}"] .worker-composer-agent`;
  const menus = () => page.evaluate((chip) => ({
    count: document.querySelectorAll(".composer-menu").length,
    expanded: document.querySelector(chip)?.getAttribute("aria-expanded") ?? null,
  }), chip);

  // 1. Open with a press on the chip's words, close with another.
  await page.click(`${chip} .worker-composer-pill-words`);
  await page.waitForTimeout(200);
  const opened = await menus();
  await page.click(`${chip} .worker-composer-pill-words`);
  await page.waitForTimeout(200);
  const closed = await menus();
  ok(
    "a press on the model chip's words closes the menu it opened — it does not close on the press and open again on the click",
    opened.count === 1 && opened.expanded === "true" && closed.count === 0 && closed.expanded === "false",
    JSON.stringify({ opened, closed }),
  );

  // 2. Scroll the list, then let the menu stand again under it.
  await page.click(`${chip} .worker-composer-pill-words`);
  await page.waitForTimeout(200);
  const scrolled = await page.evaluate(async (term) => {
    const frame = () => new Promise((done) => requestAnimationFrame(done));
    const menu = document.querySelector(".composer-menu");
    if (!menu) return { error: "no menu" };
    const scrollable = menu.scrollHeight > menu.clientHeight + 100;
    menu.scrollTop = 140;
    for (let beat = 0; beat < 3; beat += 1) await frame();
    const own = menu.scrollTop;
    document.querySelector(`.pane-slot[data-term="${term}"] .helper-turns`)?.dispatchEvent(new Event("scroll"));
    window.dispatchEvent(new Event("resize"));
    for (let beat = 0; beat < 3; beat += 1) await frame();
    return { scrollable, own, after: menu.scrollTop };
  }, term);
  ok(
    "a model list scrolled stays where it was scrolled — through its own scroll and through the page scrolling and the window resizing under it",
    scrolled.scrollable === true && scrolled.own === 140 && scrolled.after === 140,
    JSON.stringify(scrolled),
  );
  await page.keyboard.press("Escape");
  await page.waitForTimeout(120);

  // 3. The effort chip asks the backend; the model chip is pressed meanwhile.
  const race = await page.evaluate(async (term) => {
    const settle = (ms) => new Promise((done) => setTimeout(done, ms));
    const composer = document.querySelector(`.pane-slot[data-term="${term}"] .worker-composer`);
    const effort = composer?.querySelector(".worker-composer-effort");
    const model = composer?.querySelector(".worker-composer-agent");
    if (!effort || !model) return { error: "no chips" };
    window.__EFFORT_WAIT__ = 300;
    effort.click();
    await settle(30);
    model.click();
    await settle(520);
    window.__EFFORT_WAIT__ = 0;
    const standing = [...document.querySelectorAll(".composer-menu")];
    return {
      menus: standing.length,
      effortMenu: standing.some((menu) => menu.classList.contains("composer-effort-menu")),
      model: model.getAttribute("aria-expanded"),
      effort: effort.getAttribute("aria-expanded"),
    };
  }, term);
  ok(
    "an effort menu whose answer comes after the model chip was pressed stands nothing — one menu stands, the model's, and the effort chip is not left open",
    race.menus === 1 && race.effortMenu === false && race.model === "true" && race.effort === "false",
    JSON.stringify(race),
  );

  // 4. Thirty scrolls in one task under the open model menu.
  const burst = await page.evaluate(async (term) => {
    const frame = () => new Promise((done) => requestAnimationFrame(done));
    const real = window.standComposerMenu;
    if (typeof real !== "function") return { error: "standComposerMenu is not reachable" };
    let stands = 0;
    window.standComposerMenu = (...args) => {
      stands += 1;
      return real(...args);
    };
    try {
      const list = document.querySelector(`.pane-slot[data-term="${term}"] .helper-turns`);
      for (let at = 0; at < 30; at += 1) list?.dispatchEvent(new Event("scroll"));
      await frame();
      await frame();
    } finally {
      window.standComposerMenu = real;
    }
    return { stands, open: document.querySelectorAll(".composer-menu").length };
  }, term);
  ok(
    "thirty scrolls under an open menu stand it once a frame — not thirty times — and it stays open",
    burst.stands >= 1 && burst.stands <= 2 && burst.open === 1,
    JSON.stringify(burst),
  );
  await page.keyboard.press("Escape");
}
