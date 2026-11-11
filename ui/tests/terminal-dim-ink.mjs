import { installTerminalWaits } from "./terminal-grid-selection.mjs";

// SGR 2 dims the ink and nothing under it. A TUI paints a band — a tinted
// background across a row — and writes dim words on it: a placeholder in an
// input box, the marker before what a person said. Drawn with `opacity`, the
// dim run's ground was dimmed with its ink, and the word stood on a box of
// another grey inside the band (zo's composer: #2d2e35 on #38393f). The band
// is one colour from edge to edge; only reverse video, where the foreground
// IS the ground, dims its block.
export async function testDimInk(page, ok) {
  await installTerminalWaits(page);
  const seen = await page.evaluate(async () => {
    const priorAnswers = { ...window.__ANSWER__ };
    try {
      window.__ANSWER__.term_snapshot = () => null;
      const term = await openTermTab({ placement: "tab" });
      const view = termView(term);
      setActiveTab(tabOfTerm(term).id);
      await window.__TERMINAL_READY__(term, view);
      const COLS = 48;
      const ROWS = 3;
      const band = { rgb: [56, 57, 63] };
      const word = (text, style) => [...text].map((ch) => ({ ch, style: { ...style } }));
      // A band from edge to edge: a plain word, a dim one, a dim underlined
      // one, and a dim one in reverse video.
      const cells = [
        ...word("plain ", { bg: band }),
        ...word("dim ", { bg: band, dim: true }),
        ...word("lined ", { bg: band, dim: true, underline: true }),
        ...word("turned", { bg: band, dim: true, reverse: true }),
      ];
      while (cells.length < COLS) cells.push({ ch: " ", style: { bg: band } });
      const blank = () => Array.from({ length: COLS }, () => ({ ch: " " }));
      await window.__TERM_FEED__(term, {
        rows: [{ index: 0, cells }, { index: 1, cells: blank() }, { index: 2, cells: blank() }],
        scrolled_lines: 0, cursor: [2, 0], title: null, alt_screen: false, size: [ROWS, COLS], full: true,
        cursor_visible: true, mouse_tracking: "off", mouse_sgr: false, bell: false, view_offset: 0,
        scrollback_len: 0, folds: [],
      });
      await window.__UNTIL__(() => view.pre.querySelectorAll(":scope > .term-row").length === ROWS, "dim frame");
      const row = view.pre.querySelector(":scope > .term-row");
      const run = (text) => [...row.querySelectorAll("span")].find((span) => span.textContent === text);
      // The share of its colour a painted colour keeps, however the engine
      // spells it: `rgba(r, g, b, a)`, `color(srgb r g b / a)`, the mix left
      // unresolved (`color-mix(in srgb, currentcolor 60%, transparent)`), or
      // an opaque colour.
      const share = (color) => {
        const mixed = /color-mix\([^,]*,\s*currentcolor\s+([\d.]+)%/i.exec(color);
        if (mixed !== null) return Number(mixed[1]) / 100;
        const alpha = /\/\s*([\d.]+%?)\s*\)\s*$/.exec(color) ?? /rgba\([^)]*,\s*([\d.]+)\s*\)\s*$/.exec(color);
        if (alpha === null) return 1;
        return alpha[1].endsWith("%") ? Number.parseFloat(alpha[1]) / 100 : Number(alpha[1]);
      };
      const read = (text) => {
        const span = run(text);
        if (span === undefined) return null;
        const style = getComputedStyle(span);
        return {
          cls: span.className,
          ground: style.backgroundColor,
          opacity: Number(style.opacity),
          ink: share(style.webkitTextFillColor),
          line: share(style.textDecorationColor),
        };
      };
      const token = getComputedStyle(document.documentElement).getPropertyValue("--term-dim-ink").trim();
      return {
        token,
        plain: read("plain "),
        dim: read("dim "),
        lined: read("lined "),
        turned: read("turned"),
      };
    } finally {
      Object.assign(window.__ANSWER__, priorAnswers);
    }
  });
  const detail = JSON.stringify(seen);
  const kept = Number.parseFloat(seen.token) / 100;
  const near = (value, wanted) => Math.abs(value - wanted) < 0.011;
  ok(
    "terminal-dim: the token says how much of its colour dim ink keeps",
    seen.token.endsWith("%") && kept > 0 && kept < 1,
    detail,
  );
  ok(
    "terminal-dim: a dim word stands on the ground its neighbours stand on",
    seen.plain !== null && seen.dim !== null
      && seen.dim.ground === seen.plain.ground && seen.dim.opacity === 1 && seen.plain.opacity === 1,
    detail,
  );
  ok(
    "terminal-dim: the ink is dimmed by the token's share, and a plain word's is whole",
    seen.dim !== null && near(seen.dim.ink, kept) && seen.plain !== null && seen.plain.ink === 1,
    detail,
  );
  ok(
    "terminal-dim: an underline dims with the ink it underlines",
    seen.lined !== null && seen.lined.cls.split(" ").includes("underline")
      && near(seen.lined.ink, kept) && near(seen.lined.line, kept)
      && seen.lined.ground === seen.plain?.ground && seen.lined.opacity === 1,
    detail,
  );
  ok(
    "terminal-dim: under reverse video the block dims, because there the ink is the ground",
    seen.turned !== null && seen.turned.cls.split(" ").includes("dim-reverse")
      && !seen.turned.cls.split(" ").includes("dim")
      && near(seen.turned.opacity, kept) && seen.turned.ink === 1,
    detail,
  );
}
