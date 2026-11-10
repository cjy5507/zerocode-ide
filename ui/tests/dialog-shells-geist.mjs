import { openWindowTestPage } from "./window-boot.mjs";

/* The last six glass surfaces (2026-11-11). The person passed G in the right
 * column's Geist draft (p-39057edd7d8a28d6): menus and dialogs are opaque and
 * ringed, not glass. That pass moved seven surfaces; six more were still glass
 * — the window-wide `body [role="dialog"]` rule, more specific than their own
 * class, painted a 90% veil, a sheen and a 24px backdrop blur over the
 * `--surface-deck` their own rules asked for, and the automation editor asked
 * for glass itself. This suite holds the six to the material the window's
 * dialog and menu already wear, in both treatments, and stands the two that a
 * person meets on the first run — onboarding and the tour — at 320px with
 * reduced motion, where a ground that changes must not cost a word its
 * contrast or push the card off the screen.
 *
 *   WINDOW_SUITES=dialog-shells-geist node ui/tests/window.mjs */

const SURFACES = [
  { name: "the automation editor", selector: "#auto-form", depth: "float" },
  { name: "the your-turn list", selector: "#your-turn-list", depth: "drop" },
  { name: "the board's settings", selector: "#workspace-board-settings-pop", depth: "drop" },
  { name: "the Jev drawers", selector: ".jev-custom-drawer", depth: "float", count: 2 },
  { name: "onboarding", selector: ".onboard", depth: "float" },
  { name: "the tour's card", selector: "#tour-panel", depth: "float" },
];

/* Computed material, which a hidden surface answers as well as a shown one:
 * a declared ground, filter, border and shadow do not wait for layout. */
function readMaterial(surfaces) {
  const style = (node) => getComputedStyle(node);
  const probe = document.body.appendChild(document.createElement("i"));
  probe.style.color = "var(--surface-deck)";
  const deck = style(probe).color;
  probe.remove();
  const dialog = style(document.querySelector(".qo")).boxShadow;
  const menu = style(document.getElementById("sidebar-menu")).boxShadow;
  return surfaces.map(({ selector, depth }) => [...document.querySelectorAll(selector)].map((node) => {
    const s = style(node);
    return {
      selector,
      ground: s.backgroundColor,
      deck,
      image: s.backgroundImage,
      blur: s.backdropFilter || s.webkitBackdropFilter || "none",
      border: ["Top", "Right", "Bottom", "Left"].map((side) => s[`border${side}Width`]).join(" "),
      shadow: s.boxShadow,
      want: depth === "float" ? dialog : menu,
    };
  }));
}

/* Every visible word under a root, held against the first opaque ground behind
 * it; WCAG AA (4.5:1, or 3:1 from 24px or 18.66px bold). Plus whether the root
 * stays inside the viewport and the page grows no sideways scroll. */
function readFit(selector) {
  const parse = (text) => {
    const m = text.match(/rgba?\(\s*([\d.]+)[ ,]+([\d.]+)[ ,]+([\d.]+)(?:\s*[,/]\s*([\d.]+%?))?/);
    if (!m) return null;
    const a = m[4] === undefined ? 1 : m[4].endsWith("%") ? parseFloat(m[4]) / 100 : parseFloat(m[4]);
    return [+m[1], +m[2], +m[3], a];
  };
  const lum = ([r, g, b]) => [r, g, b].map((v) => { v /= 255; return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; })
    .reduce((sum, v, i) => sum + v * [0.2126, 0.7152, 0.0722][i], 0);
  const ratio = (a, b) => { const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p); return (x + 0.05) / (y + 0.05); };
  const groundOf = (node) => {
    for (let at = node; at; at = at.parentElement) {
      const c = parse(getComputedStyle(at).backgroundColor);
      if (c && c[3] > 0.99) return c;
    }
    return [0, 0, 0, 1];
  };
  const root = document.querySelector(selector);
  const r = root.getBoundingClientRect();
  const low = [];
  for (const node of root.querySelectorAll("*")) {
    const s = getComputedStyle(node);
    const words = [...node.childNodes].some((n) => n.nodeType === 3 && n.textContent.trim());
    const box = node.getBoundingClientRect();
    if (!words || box.width < 1 || box.height < 1 || s.visibility === "hidden" || +s.opacity === 0) continue;
    const ink = parse(s.color);
    if (!ink) continue;
    const ground = groundOf(node);
    // A translucent ink is read over its ground.
    const seen = ink.slice(0, 3).map((v, i) => v * ink[3] + ground[i] * (1 - ink[3]));
    const size = parseFloat(s.fontSize);
    const large = size >= 24 || (size >= 18.66 && +s.fontWeight >= 700);
    const got = ratio(seen, ground);
    if (got < (large ? 3 : 4.5)) low.push(`${node.tagName.toLowerCase()}.${[...node.classList].join(".")} ${got.toFixed(2)}`);
  }
  return {
    inside: r.left >= -0.5 && r.right <= innerWidth + 0.5 && r.width > 0,
    sideways: document.documentElement.scrollWidth - document.documentElement.clientWidth,
    low,
  };
}

export async function testDialogShellsGeist(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await page.setViewportSize({ width: 1280, height: 800 });
    const read = {};
    for (const theme of ["dark", "light"]) {
      await page.evaluate((theme) => { document.documentElement.dataset.theme = theme; }, theme);
      read[theme] = (await page.evaluate(readMaterial, SURFACES)).flat();
    }
    const wrong = [];
    for (const [theme, rows] of Object.entries(read)) {
      for (const row of rows) {
        const off = [];
        if (row.ground !== row.deck) off.push(`ground ${row.ground} ≠ deck ${row.deck}`);
        if (row.image !== "none") off.push(`image ${row.image.slice(0, 40)}`);
        if (row.blur !== "none") off.push(`blur ${row.blur}`);
        if (row.border !== "0px 0px 0px 0px") off.push(`border ${row.border}`);
        if (row.shadow !== row.want) off.push(`shadow ${row.shadow.slice(0, 60)}`);
        if (off.length) wrong.push(`${theme} ${row.selector}: ${off.join("; ")}`);
      }
    }
    const counted = SURFACES.every((one) => read.dark.filter((row) => row.selector === one.selector).length === (one.count ?? 1));
    ok(
      "the six surfaces that were still glass — the automation editor, the your-turn list, the board's settings, the two Jev drawers, onboarding and the tour's card — wear the window's one floating material in both treatments: the opaque deck, no blur, no sheen, no border, and the dialog's or the menu's own ringed shadow",
      counted && wrong.length === 0,
      counted ? wrong.join("\n") : JSON.stringify(read.dark.map((row) => row.selector)),
    );

    /* The two surfaces of the first run, at the narrowest window, both treatments. */
    await page.setViewportSize({ width: 320, height: 640 });
    const firstRun = {};
    for (const theme of ["dark", "light"]) {
      await page.evaluate((theme) => { document.documentElement.dataset.theme = theme; }, theme);
      await page.evaluate(async () => {
        openOnboarding({ flow_version: 1, closed_at: null, outcome: null, last_completed_step: -1, checklist: { chose_agent: false, dismissed: false } });
        await new Promise((done) => setTimeout(done, 200));
      });
      firstRun[`${theme} onboarding`] = await page.evaluate(readFit, ".onboard");
      await page.evaluate(() => { document.getElementById("onb-scrim").hidden = true; });
      // The tour points at something on the board, so the board stands first
      // (as the person's first visit to it does) — with nothing to point at
      // the card is not drawn at all.
      firstRun[`${theme} tour`] = await page.evaluate(async (fit) => {
        for (const scrim of document.querySelectorAll(".scrim:not([hidden])")) scrim.hidden = true;
        if (activeTabId !== "board") document.getElementById("nav-agents").click();
        await new Promise((done) => setTimeout(done, 300));
        const pointed = tourTargetOf(TOURS.board[0]) !== null;
        startTour("board");
        await new Promise((done) => setTimeout(done, 200));
        // The card takes the focus as it opens; the window-wide focus halo
        // must not trade its ring and float for a slate band.
        const panel = document.getElementById("tour-panel");
        const kept = document.activeElement === panel
          && getComputedStyle(panel).boxShadow === getComputedStyle(document.querySelector(".qo")).boxShadow;
        const seen = { pointed, kept, ...(new Function(`return (${fit})`)())("#tour-panel") };
        closeTour();
        return seen;
      }, readFit.toString());
    }
    const bad = Object.entries(firstRun).filter(([, fit]) => fit.pointed === false || fit.kept === false || !fit.inside || fit.sideways > 0 || fit.low.length);
    ok(
      "onboarding and the tour's card stay inside a 320px window with no sideways scroll, and every word on them keeps WCAG AA against the opaque ground, dark and light, with reduced motion; the tour's card, focused as it opens, keeps the dialog's ringed float",
      bad.length === 0,
      JSON.stringify(Object.fromEntries(bad.length ? bad : Object.entries(firstRun))),
    );
    ok("the dialog shells raise no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}
