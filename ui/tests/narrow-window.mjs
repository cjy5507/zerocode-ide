import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { chromium, createWindowServer, openWindowTestPage, recordBootPhases, waitBootSettled } from "./window-boot.mjs";

/* The window on a laptop's own screen (t-77622).
 *
 * The person's photo (2026-10-27): a Mac's built-in screen, the window about
 * 1000 pt wide, the sidebar and the source-control panel both open — the
 * right end of the title bar (「터미널 | 대화」), the source-control panel's
 * message box and commit button, and the status bar's right end were cut off,
 * and the status bar broke onto a second line. The harness stands the same
 * window up at a virtual width and counts what lies past an edge it belongs
 * inside; nothing here reads the real display. */

export const NARROW_WIDTHS = [1000, 900, 800, 720];
export const WIDE_WIDTHS = [1440, 1920];
export const LOCALES = ["ko", "en", "ja", "zh", "es"];

/* What the harness stands on the bar, past every paint of the window's own:
 * the project, the model and the terminal count (t-77829). */
const STOOD = Object.freeze({ "sb-project": "zerocode", "sb-model": "claude-opus-5-5 · auto", "sb-count": "9" });

/* The boot's own paints of the bar are behind it: the last phase it awaits
 * and the last phase it reaches — the standing workspaces' resumes, not
 * awaited — then two frames (t-77829). Needs `recordBootPhases` as the
 * page's `before`. Values stood before then were painted over by the boot,
 * differently in each run: ja, en and es measured an empty project, an
 * empty model and three terminals. */
export async function bootDone(page) {
  await waitBootSettled(page);
  await page.waitForFunction(() => window.__BOOT_PHASES__.includes("terminals_resumed"));
  await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
}

/* Whether the bar still says what the harness stood. */
export function standing(stood) {
  const said = Object.fromEntries(Object.keys(stood).map((id) => [id, el(id).textContent]));
  return { held: Object.entries(stood).every(([id, text]) => said[id] === text), said };
}

/* The account line the photo showed, spoken in the catalog's own words, and a
 * memory/terminal count — the longest thing the bar says. The window's own
 * paints its steps set going land first, and only then are the values put. */
export async function stand(page, locale) {
  await page.evaluate(async ([locale, stood]) => {
    const twoFrames = () => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
    setLocale(locale);
    for (let i = 0; i < 3; i++) await openTermTab();
    // The active pane runs an agent: the title bar's 터미널 | 대화 segment.
    el("view-toggle").hidden = false;
    folded.sidebar = false;
    folded.aside = false;
    setActivityItem("scm");
    // The account line the photo showed, through the real paint: three
    // windows (one named), the next account, the cooldown and the auto word.
    const hour = 3_600_000;
    window.__USAGE__ = {
      provider: "claude", account: "a-fixture", status: "ok", error: null, updated_at: Date.now(),
      session: { used_percent: 21, window_minutes: 300, resets_at: Date.now() + 2.87 * hour },
      weekly: { used_percent: 92, window_minutes: 10080, resets_at: Date.now() + 115 * hour },
      fable_weekly: { used_percent: 4, window_minutes: 10080, resets_at: Date.now() + 115 * hour },
    };
    window.__ACCOUNT_USAGE__ = {
      accounts: [],
      plan: {
        mode: "auto", active: "a-fixture", decision: { kind: "stay", why: "cooldown" }, landing: "a-fixture",
        fitness: [
          { id: "a-fixture", room_percent: 4, unfit: null, next_reset_ms: null },
          { id: "b-fixture", room_percent: 80, unfit: null, next_reset_ms: null },
        ],
        next: { id: "b-fixture", room_percent: 80, unfit: null, next_reset_ms: null },
        walled: [], last_switch_ms: null, cooldown_until_ms: Date.now() + 3 * 60_000,
        failed_recently: [], token: "narrow-window", now_ms: Date.now(),
      },
      sent: 0, fetching: false,
    };
    await refreshClaudeUsage(false);
    await refreshClaudeAccountUsage(false);
    // The tab strip's and the status bar's paints the three tabs set going.
    await twoFrames();
    for (const [id, text] of Object.entries(stood)) el(id).textContent = text;
    el("sb-usage").textContent = `ctx ${(6693).toLocaleString()}`;
    el("sb-memory").textContent = "727.4 MB";
    el("sb-ports").hidden = false;
    el("sb-ports-count").textContent = "3";
    el("commit-box").hidden = false;
    await twoFrames();
  }, [locale, STOOD]);
}

/* What lies past an edge it belongs inside, by region. A box is out when a
 * visible part of it is farther than a pixel beyond its region's right edge. */
export function measure() {
  const visible = (node) => {
    const r = node.getBoundingClientRect();
    if (r.width === 0 || r.height === 0) return false;
    for (let n = node; n && n !== document.body; n = n.parentElement) {
      const s = getComputedStyle(n);
      if (s.display === "none" || s.visibility === "hidden" || n.hidden) return false;
    }
    return true;
  };
  const past = (root, selector, edge) => {
    const out = [];
    for (const node of root.querySelectorAll(selector)) {
      if (!visible(node)) continue;
      const r = node.getBoundingClientRect();
      if (r.right > edge + 1 || r.left < -1) out.push(`${node.id ? "#" + node.id : "." + [...node.classList].join(".")}`);
    }
    return out;
  };
  const vw = innerWidth;
  const bar = document.querySelector(".titlebar");
  const right = document.querySelector(".titlebar-right");
  const aside = document.querySelector(".files");
  const status = document.querySelector(".statusbar");
  // Text lines the bar speaks in: one band per distinct text-box top.
  const bands = new Set();
  const walker = document.createTreeWalker(document.querySelector(".statusbar"), NodeFilter.SHOW_TEXT);
  for (let text = walker.nextNode(); text; text = walker.nextNode()) {
    if (!text.textContent.trim() || !visible(text.parentElement)) continue;
    const range = document.createRange();
    range.selectNodeContents(text);
    for (const q of range.getClientRects()) if (q.width > 0) bands.add(Math.round(q.top / 8));
  }
  const titlebar = past(right, "*", Math.min(vw, bar.getBoundingClientRect().right));
  const asideRect = aside.getBoundingClientRect();
  const panel = past(aside, "button, textarea, input, select, .btn", Math.min(asideRect.right, vw));
  const barRect = status.getBoundingClientRect();
  const statusOut = past(status, ".sb-item, .sb-button, .sb-figure, .sb-claude", Math.min(vw, barRect.right - 4));
  const statusRows = bands.size;
  const wrapped = statusRows > 1 ? ["lines:" + statusRows] : [];
  return {
    vw,
    titlebar, panel,
    status: statusOut, wrapped,
    statusHeight: Math.round(barRect.height),
    statusRows,
    pageWidth: document.documentElement.scrollWidth,
    asideWidth: Math.round(asideRect.width),
    toggleRight: Math.round(document.getElementById("view-toggle-chat").getBoundingClientRect().right),
    clipped: titlebar.length + panel.length + statusOut.length + wrapped.length,
  };
}

/* The bar's account line as words, at the width the page is now — and whether
 * the figure it shows is whole on screen, not cut by an ellipsis (innerText
 * reads the whole string either way). */
const barWords = () => {
  const shown = getComputedStyle(el("sb-claude-pct")).display === "none" ? el("sb-claude-pct").nextElementSibling : el("sb-claude-pct");
  return {
    figure: shown.innerText.trim(),
    whole: shown.clientWidth > 0 && shown.scrollWidth <= shown.clientWidth + 1,
    next: getComputedStyle(el("sb-claude-next")).display !== "none",
  };
};

/* The bar's own gap and the token it should be at the width the page is now.
 * The width is the bar's content box — what its container query reads — so
 * the gap closes exactly where the account line folds to its numbers. */
const barGap = () => {
  const token = (name) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  const bar = document.querySelector(".statusbar");
  const style = getComputedStyle(bar);
  const width = bar.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight);
  return {
    width,
    gap: style.columnGap,
    want: token(width <= 900 ? "--space-2" : width <= 1100 ? "--space-3" : "--space-4"),
  };
};

/* Window widths on both sides of each of the bar's thresholds: inside the
 * bar's own padding, where the window is wider than the line the container
 * reads, and one point past it, where nothing has folded yet. */
export const EDGE_WIDTHS = [1125, 1124, 1110, 925, 924, 910];

/* A crowded bar (t-77705): the Codex PTY and PR marks through their own
 * paints, the refresh button, and — with `codex` — a second gauge through the
 * real usage paint. */
async function crowd(page, codex) {
  await page.evaluate(async (codex) => {
    codexPtyFallbacks.set(7, "the native route was refused");
    paintCodexPtyFallbacks();
    scmStanding = [
      { id: "pr-123", kind: "mergeable", number: 123, checkout: "zerocode" },
      { id: "pr-124", kind: "ci", number: 124, checkout: "zerocode" },
    ];
    paintScmMark();
    el("sb-usage-refresh").hidden = false;
    if (codex) {
      const hour = 3_600_000;
      window.__CODEX_USAGE__ = {
        provider: "codex", account: null, status: "ok", error: null, updated_at: Date.now(),
        session: { used_percent: 35, window_minutes: 300, resets_at: Date.now() + 4.2 * hour },
        weekly: { used_percent: 61, window_minutes: 10080, resets_at: Date.now() + 51 * hour },
      };
      await refreshCodexUsage(false);
    }
    await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
  }, codex);
}

/* Each mark on the bar: on screen inside the window, and still named. */
const marks = () => ["sb-codex-route", "sb-scm-notices"].map((id) => {
  const r = el(id).getBoundingClientRect();
  return { id, on: r.width > 0 && r.right <= innerWidth, name: el(id).getAttribute("aria-label") ?? "" };
});

export async function testNarrowWindow(browser, origin, ok) {
  const rows = [];
  for (const locale of LOCALES) {
    const { page, faults } = await openWindowTestPage(browser, origin, { before: recordBootPhases });
    try {
      await bootDone(page);
      await stand(page, locale);
      const stood = await page.evaluate(standing, STOOD);
      ok(`the bar says what was stood on it, past the window's own paints (${locale})`, stood.held, JSON.stringify(stood.said));
      for (const width of [...NARROW_WIDTHS, ...WIDE_WIDTHS]) {
        await page.setViewportSize({ width, height: 700 });
        await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
        const m = await page.evaluate(measure);
        const words = await page.evaluate(barWords);
        const gap = await page.evaluate(barGap);
        rows.push({ locale, width, ...m, words });
        // The bar's own gaps close with the window (t-77705): they sat in a
        // container query on the bar itself, which an element cannot make.
        ok(`the bar's gaps are the width's (${locale}, ${width} pt)`, gap.gap === gap.want, JSON.stringify(gap));
        const tag = `${locale}, ${width} pt`;
        if (width <= 1000) {
          ok(`nothing lies past the window's right edge (${tag})`, m.clipped === 0, JSON.stringify(m));
          ok(`the status bar keeps one line (${tag})`, m.statusRows === 1, JSON.stringify(m));
          ok(`the 터미널 | 대화 switch ends inside the window (${tag})`, m.toggleRight <= width, JSON.stringify(m));
        } else {
          // A wide window is left as it was: nothing cut, one line, every word of the account line.
          ok(`the wide window shows the account line whole (${tag})`,
            m.clipped === 0 && m.statusRows === 1 && words.next && words.figure.length > 30,
            JSON.stringify({ ...m, words }));
        }
        if (width <= 1100) {
          ok(`a narrow bar says the account line as its numbers (${tag})`,
            /^\d+% · \d+% · \d+%$/.test(words.figure) && !words.next, JSON.stringify(words));
          // Whole: the numbers are the last of the line to give (t-77705).
          ok(`a narrow bar's numbers are whole (${tag})`, words.whole, JSON.stringify(words));
        }
      }

      // The right panel at the narrowest the clamp lets it be and narrower still:
      // its message box, commit button and header buttons stay inside it.
      await page.setViewportSize({ width: 1000, height: 700 });
      for (const aside of [200, 155]) {
        await page.evaluate((aside) => document.documentElement.style.setProperty("--aside-width", `${aside}px`), aside);
        await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
        const m = await page.evaluate(measure);
        ok(`the source-control panel's message box and buttons stay inside a ${aside}px panel (${locale})`,
          m.panel.length === 0 && m.clipped === 0, JSON.stringify(m.panel));
      }

      // What the narrow bar folded away is on hover: the full sentence, appended to the tip it had.
      const tip = await page.evaluate(() => {
        const figure = el("sb-claude-pct");
        figure.dispatchEvent(new PointerEvent("pointerover", { bubbles: true }));
        const holder = figure.closest(".sb-claude");
        return { tip: holder.dataset.tip, full: `${figure.textContent} — ${el("sb-claude-next").textContent}` };
      });
      ok(`a shortened account line is spoken whole on hover (${locale})`,
        tip.tip.endsWith(tip.full) && tip.full.length > 20, JSON.stringify(tip));

      // The keyboard reaches the same sentence (t-77705): the roster, focused,
      // shows its tip at once, and the tip carries the folded figure.
      const keyed = await page.evaluate(() => {
        el("sb-usage-roster").focus();
        const shown = document.querySelector(".tooltip");
        const said = shown.hidden ? "" : shown.textContent;
        el("sb-usage-roster").blur();
        return { said, figure: el("sb-claude-pct").textContent };
      });
      ok(`a shortened account line is spoken whole to the keyboard (${locale})`,
        keyed.figure.length > 20 && keyed.said.includes(keyed.figure), JSON.stringify(keyed));

      // An item that fits again drops the words it was shortened by (t-77705):
      // the model's tip named the old model after the window widened.
      const stale = await page.evaluate(async () => {
        const frames = () => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
        const model = el("sb-model");
        const was = model.textContent;
        model.textContent = `claude-opus-5-5 · auto · ${"overlong ".repeat(12)}`;
        await frames();
        model.dispatchEvent(new PointerEvent("pointerover", { bubbles: true }));
        const narrow = model.dataset.tip ?? "";
        model.textContent = "claude-sonnet-5-5";
        await frames();
        model.dispatchEvent(new PointerEvent("pointerover", { bubbles: true }));
        const after = model.dataset.tip ?? "";
        model.textContent = was;
        await frames();
        return { narrow, after };
      });
      ok(`an item that fits again drops the words it was shortened by (${locale})`,
        stale.narrow.includes("overlong") && !stale.after.includes("overlong"), JSON.stringify(stale));

      // The gap closes where the numbers fold, not 24 px of padding later:
      // the window just past each threshold still folds (t-77705 review).
      for (const width of EDGE_WIDTHS) {
        await page.setViewportSize({ width, height: 700 });
        await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
        const gap = await page.evaluate(barGap);
        ok(`the bar's gaps are the width's (${locale}, ${width} pt)`, gap.gap === gap.want, JSON.stringify(gap));
      }

      // A crowded bar (t-77705): with a mark, and with a mark and a second
      // gauge, nothing lies past the edge, the bar keeps one line, every mark
      // stays on it and named, and the first gauge's numbers stay whole.
      for (const codex of [false, true]) {
        await crowd(page, codex);
        for (const width of NARROW_WIDTHS) {
          await page.setViewportSize({ width, height: 700 });
          await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
          const m = await page.evaluate(measure);
          const words = await page.evaluate(barWords);
          const on = await page.evaluate(marks);
          const tag = `${locale}, ${width} pt, ${codex ? "a mark and a second gauge" : "a mark"}`;
          ok(`a crowded bar keeps inside the window on one line (${tag})`,
            m.clipped === 0 && m.statusRows === 1, JSON.stringify(m));
          ok(`a crowded bar keeps every mark on it (${tag})`,
            on.every((one) => one.on && one.name.length > 0), JSON.stringify(on));
          ok(`a crowded bar keeps its numbers whole (${tag})`, words.whole, JSON.stringify(words));
          if (width <= 800) {
            // What folded is on the hover, the account's short id included.
            const said = await page.evaluate(() => {
              el("sb-claude-pct").dispatchEvent(new PointerEvent("pointerover", { bubbles: true }));
              const account = el("sb-claude-account");
              return {
                tip: el("sb-claude").dataset.tip ?? "",
                account: account.hidden ? "" : account.textContent.trim(),
                folded: getComputedStyle(account).display === "none",
              };
            });
            ok(`a crowded bar's folded account id is on the hover (${tag})`,
              said.folded && said.account.length > 0 && said.tip.includes(said.account), JSON.stringify(said));
          }
        }
      }
      ok(`the window raises no browser errors (${locale})`, faults.length === 0, faults.join("\n"));
    } finally {
      await page.close();
    }
  }
  return rows;
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1] ?? "")).href) {
  const { origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  let failed = 0;
  const rows = await testNarrowWindow(browser, origin, (name, pass, detail) => {
    if (!pass) failed += 1;
    if (process.env.NARROW_VERBOSE || !pass) console.log(`${pass ? "PASS" : "FAIL"}  ${name}${pass ? "" : "  — " + (detail ?? "")}`);
  });
  for (const r of rows) console.log(`${r.locale} ${r.width}: line=\"${r.words.figure}\" clipped=${r.clipped} title=${r.titlebar.length} panel=${r.panel.length} status=${r.status.length} wrapped=${r.wrapped.length} rows=${r.statusRows} aside=${r.asideWidth} page=${r.pageWidth}`);
  await browser.close();
  process.exit(failed ? 1 : 0);
}
