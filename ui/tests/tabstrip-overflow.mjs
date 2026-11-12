import { mkdir } from "node:fs/promises";
import { pathToFileURL } from "node:url";
import { resolve } from "node:path";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";

/* The title bar's tab strip under many tabs (t-17078).
 *
 * The person's report (2026-09-30): 「헤더 쪽 탭이 늘어날수록 화면이 깨지는
 * 버그」. Measured before the fix: the bar is a grid item whose floor was its
 * min-content, so sixty tabs made a 10437px-wide title bar in a 720px window —
 * the strip never scrolled (scrollWidth == clientWidth) and the controls on
 * the right stood 9600px off screen; even five tabs left the bar at 1083px
 * once the window narrowed to 1024 or less. These are the numbers the bar must
 * now keep at every width and tab count. 2026-11-12 (「많아지면 문제임」) added
 * what a crowded strip must still give: a name for each tab it names, the ＋
 * on screen, a close that lands on its own tab, and a waiting tab that says so
 * from out of sight. */

const WIDTHS = [1280, 1024, 800, 720];
const COUNTS = [5, 20, 60];

async function grow(page, from, to) {
  await page.evaluate(async ([from, to]) => {
    for (let i = from; i < to; i++) {
      // Terminals and files mixed, long names included — synthetic names only.
      if (i % 3 === 0) await openTermTab();
      else await openFile(`/tmp/zerocode-window-test/src/${i % 2 ? "a-rather-long-module-name-for-tab-" : "f"}${i}.rs`);
    }
  }, [from, to]);
  await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
}

/* Everything one case is judged on. A tab counts as seen only when its box
 * lies inside the tab list — the strip's scroller — clear of whichever fade is
 * armed on that side. */
function measure() {
  const strip = el("tabstrip");
  const list = strip.querySelector(".tab-list");
  const box = (node) => node.getBoundingClientRect();
  const fade = parseFloat(getComputedStyle(list).scrollPaddingLeft) || 0;
  const seen = (node) => {
    const view = box(list);
    const it = box(node);
    const leftFade = list.scrollLeft > 0 ? fade : 0;
    const rightFade = list.scrollLeft + list.clientWidth < list.scrollWidth - 1 ? fade : 0;
    return it.left >= view.left + leftFade - 1 && it.right <= view.right - rightFade + 1;
  };
  const nodes = [...strip.querySelectorAll(".tab")];
  const shown = {};
  for (const [name, pick] of [["last", nodes.at(-1)], ["first", nodes[0]], ["middle", nodes[nodes.length >> 1]]]) {
    setActiveTab(pick.dataset.tab);
    shown[name] = seen(strip.querySelector(`.tab[data-tab="${CSS.escape(pick.dataset.tab)}"]`));
  }
  const root = getComputedStyle(document.documentElement);
  const token = (name) => root.getPropertyValue(name).trim();
  const right = box(document.querySelector(".titlebar-right"));
  const label = nodes[0].querySelector(".tab-label");
  const nameOf = (node) => node.querySelector(".tab-label");
  // Named: neither the window's fold nor the crowd's has taken its name off the eye.
  const named = nodes.filter((node) => !node.hasAttribute("data-folded") && getComputedStyle(nameOf(node)).clipPath === "none");
  const folded = nodes.filter((node) => node.hasAttribute("data-folded"));
  const inView = (node) => box(node).left >= box(list).left - 1 && box(node).right <= box(list).right + 1;
  const add = strip.querySelector(".tab-new");
  const addShown = getComputedStyle(add).display !== "none";
  return {
    tabs: nodes.length,
    barHeight: Math.round(box(document.querySelector(".titlebar")).height),
    barWidth: Math.round(box(document.querySelector(".titlebar")).width),
    rightWidth: Math.round(right.width),
    rightInside: right.left >= 0 && right.right <= innerWidth,
    pageWidth: document.documentElement.scrollWidth,
    viewport: innerWidth,
    stageTop: Math.round(box(document.querySelector(".stage")).top),
    scrolls: list.scrollWidth > list.clientWidth,
    // The window's own folds (t-78107, 시안 17), read off the tokens they fold at.
    windowFold: matchMedia(`(max-width: ${token("--fold-sidebar-rail")})`).matches,
    menuFold: matchMedia(`(max-width: ${token("--fold-tab-menu")})`).matches,
    floor: parseFloat(token("--tab-min-width")),
    crowdWidth: parseFloat(token("--tab-crowd-width")),
    namedNarrowest: named.length ? Math.round(Math.min(...named.map((node) => box(node).width))) : null,
    // Each name the strip shows is whole, or has at least three Hangul characters' room.
    namesKept: named.every((node) => nameOf(node).scrollWidth <= nameOf(node).clientWidth + 1 || box(nameOf(node)).width >= 36),
    crowdFolded: folded.length,
    foldedWidths: [...new Set(folded.map((node) => Math.round(box(node).width)))],
    // The 1000 px fold: every tab off stage keeps its spoken name but not its place on the eye.
    windowFolded: nodes.filter((node) => !node.classList.contains("is-active"))
      .every((node) => getComputedStyle(nameOf(node)).clipPath !== "none"),
    front: Math.round(box(strip.querySelector(".tab.is-active")).width),
    // A tab sized to its name may stand under the floor when the whole name fits in less.
    frontNameWhole: ((name) => name.scrollWidth <= name.clientWidth + 1)(strip.querySelector(".tab.is-active .tab-label")),
    frontOnly: nodes.filter((node) => node.getClientRects().length > 0).length === 1,
    frontClose: box(strip.querySelector(".tab.is-active").querySelector(".tab-close, .tab-pin")).width > 0,
    ellipsis: getComputedStyle(label).textOverflow === "ellipsis",
    closeKept: nodes.filter((node) => !node.hasAttribute("data-folded")).every((node) =>
      node.querySelector(".tab-close, .tab-pin") && box(node.querySelector(".tab-close, .tab-pin")).width > 0),
    // A press on the centre of a close lands on that close's own tab — in a crowded strip it stood 8 px past the
    // tab's edge, and the neighbour took the press (2026-11-12).
    closeOwn: named.filter(inView).every((node) => {
      const it = box(node.querySelector(".tab-close, .tab-pin"));
      return document.elementFromPoint(it.left + it.width / 2, it.top + it.height / 2)?.closest(".tab") === node;
    }),
    // The ＋ stands after the list, on screen, never scrolled off with the tabs (it stood 685 px past the strip at
    // twenty tabs in a 1440 px window).
    addOnScreen: addShown ? box(add).left >= box(strip).left - 1 && box(add).right <= box(strip).right + 1 && box(add).right <= innerWidth : null,
    shown,
  };
}

export async function testTabstripOverflow(browser, origin, ok) {
  const shots = "output/playwright/tabstrip";
  await mkdir(shots, { recursive: true });
  for (const theme of ["dark", "light"]) {
    const { page, faults } = await openWindowTestPage(browser, origin);
    try {
      await page.evaluate((theme) => {
        window.__ANSWER__.read_text_file = () => ({ text: "x\n", mtime: 1, size: 2 });
        setTheme(theme);
        // The strip's observer folds tabs; a fold that resized what it observes would loop it (2026-11-12, the
        // settings harness's "ResizeObserver loop completed with undelivered notifications").
        window.__resizeLoops = [];
        addEventListener("error", (event) => {
          if (/ResizeObserver loop/.test(event.message ?? "")) window.__resizeLoops.push(event.message);
        });
      }, theme);
      let open = 0;
      let base = null;
      for (const count of COUNTS) {
        await grow(page, open, count);
        open = count;
        for (const width of WIDTHS) {
          await page.setViewportSize({ width, height: 860 });
          // The window answers a resize on its next frame (the strip's observer, t-78107).
          await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
          const m = await page.evaluate(measure);
          base ??= m;
          const tag = `${theme}, ${count} tabs, ${width}px`;
          ok(`the title bar stays one row inside the window (${tag})`,
            m.barHeight === base.barHeight && m.barWidth === width && m.pageWidth <= width &&
              m.stageTop === base.stageTop,
            JSON.stringify(m));
          ok(`the controls on the right keep their width on screen (${tag})`,
            m.rightInside && m.rightWidth === base.rightWidth, JSON.stringify(m));
          ok(`the tab in front is never behind the fade (${tag})`,
            m.shown.last && m.shown.first && m.shown.middle, JSON.stringify(m.shown));
          if (m.menuFold) {
            ok(`under 760 px the strip is the tab in front, at its floor or with its whole name, and the menu of every tab (${tag})`,
              m.frontOnly && (m.front >= m.floor || m.frontNameWhole) && m.ellipsis && m.frontClose, JSON.stringify(m));
          } else if (m.windowFold) {
            ok(`a 1000 px window folds the tabs not on stage to their mark and keeps the floor for the one in front (${tag})`,
              m.windowFolded && (m.front >= m.floor || m.frontNameWhole) && m.ellipsis && m.closeKept, JSON.stringify(m));
          } else if (m.scrolls || m.crowdFolded > 0) {
            ok(`a crowded strip keeps each name it shows at the floor or wider and folds its quiet tabs to their mark first (${tag})`,
              m.namedNarrowest >= m.floor && m.namesKept && m.crowdFolded > 0 && m.foldedWidths.every((one) => one === m.crowdWidth) &&
                m.ellipsis && m.closeKept && (!m.scrolls || m.crowdFolded === m.tabs - 1),
              JSON.stringify(m));
          }
          ok(`the ＋ stands on screen after the tabs (${tag})`, m.addOnScreen === true, JSON.stringify(m));
          ok(`a press on a close lands on its own tab (${tag})`, m.closeOwn, JSON.stringify(m));
          if (count === 60 && (width === 800 || width === 720)) {
            await page.screenshot({ path: `${shots}/${theme}-60-${width}.png` });
          }
        }
      }

      // The keyboard's way through the tabs lands on a tab the person can see —
      // in the narrowest window that still scrolls a strip of tabs (under 760 px
      // the strip is one tab and its menu).
      await page.setViewportSize({ width: 800, height: 860 });
      await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
      await page.evaluate(() => setActiveTab(el("tabstrip").querySelector(".tab").dataset.tab));
      await page.keyboard.down("Control");
      await page.keyboard.press("Shift+Tab");
      await page.keyboard.up("Control");
      const keyed = await page.evaluate(() => {
        const list = el("tabstrip").querySelector(".tab-list");
        const node = list.querySelector(".tab.is-active");
        const fade = parseFloat(getComputedStyle(list).scrollPaddingLeft);
        const it = node.getBoundingClientRect();
        const view = list.getBoundingClientRect();
        return { moved: node !== list.querySelector(".tab"), left: it.left - view.left, right: view.right - it.right, fade };
      });
      ok(`Control+Shift+Tab lands on a tab clear of the fade (${theme})`,
        keyed.moved && keyed.left >= keyed.fade - 1 && keyed.right >= -1, JSON.stringify(keyed));

      // A repaint with the same tab in front keeps where the person scrolled.
      const kept = await page.evaluate(() => {
        const list = el("tabstrip").querySelector(".tab-list");
        list.scrollLeft = 300;
        const before = list.scrollLeft;
        renderTabs();
        return { before, after: list.scrollLeft };
      });
      ok(`the strip's scroll survives a repaint (${theme})`, kept.after === kept.before, JSON.stringify(kept));

      // Dragging still reorders with sixty tabs: the front tab, by hand, one
      // neighbour to the left.
      const drag = await page.evaluate(() => {
        const strip = el("tabstrip");
        const list = strip.querySelector(".tab-list");
        const all = strip.querySelectorAll(".tab");
        // A tab from the middle of sixty, brought in front and scrolled to
        // the list's middle, so it and its left neighbour are both in reach.
        setActiveTab(all[30].dataset.tab);
        const it = all[30].getBoundingClientRect();
        const view = list.getBoundingClientRect();
        list.scrollLeft += it.left + it.width / 2 - (view.left + view.width / 2);
        const node = strip.querySelector(".tab.is-active");
        const prev = node.previousElementSibling;
        const a = node.getBoundingClientRect();
        const b = prev.getBoundingClientRect();
        return { id: node.dataset.tab, was: [...strip.querySelectorAll(".tab")].indexOf(node), from: [a.left + a.width / 2, a.top + a.height / 2], to: [b.left + b.width / 4, b.top + b.height / 2] };
      });
      await page.mouse.move(...drag.from);
      await page.mouse.down();
      await page.mouse.move(drag.from[0] - 20, drag.from[1], { steps: 4 });
      await page.mouse.move(...drag.to, { steps: 8 });
      await page.mouse.up();
      const order = await page.evaluate((id) => {
        const nodes = [...el("tabstrip").querySelectorAll(".tab")].map((node) => node.dataset.tab);
        return { at: nodes.indexOf(id), count: nodes.length };
      }, drag.id);
      ok(`dragging a tab still reorders the strip at sixty tabs (${theme})`,
        order.count === 60 && order.at === drag.was - 1, JSON.stringify({ ...order, drag }));
      // A tab that waits on the person, scrolled out of sight, still says so: the menu after the list wears the
      // you-ring and its tip says why, and the ring goes once the tab is in view again (2026-11-12).
      await page.setViewportSize({ width: 1280, height: 860 });
      await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
      const away = await page.evaluate(async () => {
        const tell = (name, payload) => { for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload }); };
        const term = await openTermTab({ placement: "tab" });
        paneAgents.set(term, "claude");
        tell("hook:agent", { term, state: "needs-attention", agent: "claude", session: "s-out-of-sight" });
        setActiveTab(el("tabstrip").querySelector(".tab").dataset.tab);
        renderTabs();
        await window.__PAINTED__();
        await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
        const strip = el("tabstrip");
        const list = strip.querySelector(".tab-list");
        const menu = strip.querySelector(".tab-menu");
        const ringShown = () => getComputedStyle(menu.querySelector(".tab-menu-ring")).display !== "none";
        const seen = { waiting: Boolean(list.querySelector(".tab.is-waiting")), far: menu.classList.contains("is-waiting"), ring: ringShown(), tip: menu.dataset.tip };
        list.scrollLeft = list.scrollWidth;
        await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
        seen.near = menu.classList.contains("is-waiting");
        seen.ringNear = ringShown();
        return seen;
      });
      ok(`a tab that waits on you out of sight rings the tab menu, and stops once it is in view (${theme})`,
        away.waiting && away.far && away.ring && away.tip !== "이 판의 탭 모두" && !away.near && !away.ringNear, JSON.stringify(away));
      const loops = await page.evaluate(() => window.__resizeLoops);
      ok(`folding a crowded strip never loops its resize observer (${theme})`, loops.length === 0, JSON.stringify(loops));
      ok(`the crowded strip raises no browser errors (${theme})`, faults.length === 0, faults.join("\n"));
    } finally {
      await page.close();
    }
  }
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  let failures = 0;
  try {
    await testTabstripOverflow(browser, origin, (name, pass, detail = "") => {
      console.log(`${pass ? "PASS" : "FAIL"} ${name}${!pass && detail ? `\n${detail}` : ""}`);
      if (!pass) failures++;
    });
  } finally { await browser.close(); files.close(); }
  if (failures) process.exitCode = 1;
}
