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
 * now keep at every width and tab count. */

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
 * lies inside the strip clear of whichever fade is armed on that side. */
function measure() {
  const strip = el("tabstrip");
  const box = (node) => node.getBoundingClientRect();
  const fade = parseFloat(getComputedStyle(strip).scrollPaddingLeft) || 0;
  const seen = (node) => {
    const view = box(strip);
    const it = box(node);
    const leftFade = strip.scrollLeft > 0 ? fade : 0;
    const rightFade = strip.scrollLeft + strip.clientWidth < strip.scrollWidth - 1 ? fade : 0;
    return it.left >= view.left + leftFade - 1 && it.right <= view.right - rightFade + 1;
  };
  const nodes = [...strip.querySelectorAll(".tab")];
  const shown = {};
  for (const [name, pick] of [["last", nodes.at(-1)], ["first", nodes[0]], ["middle", nodes[nodes.length >> 1]]]) {
    setActiveTab(pick.dataset.tab);
    shown[name] = seen(strip.querySelector(`.tab[data-tab="${CSS.escape(pick.dataset.tab)}"]`));
  }
  const right = box(document.querySelector(".titlebar-right"));
  const label = nodes[0].querySelector(".tab-label");
  return {
    tabs: nodes.length,
    barHeight: Math.round(box(document.querySelector(".titlebar")).height),
    barWidth: Math.round(box(document.querySelector(".titlebar")).width),
    rightWidth: Math.round(right.width),
    rightInside: right.left >= 0 && right.right <= innerWidth,
    pageWidth: document.documentElement.scrollWidth,
    viewport: innerWidth,
    stageTop: Math.round(box(document.querySelector(".stage")).top),
    scrolls: strip.scrollWidth > strip.clientWidth,
    narrowest: Math.round(Math.min(...nodes.map((node) => box(node).width))),
    minWidth: parseFloat(getComputedStyle(nodes[0]).minWidth),
    ellipsis: getComputedStyle(label).textOverflow === "ellipsis",
    closeKept: nodes.every((node) => node.querySelector(".tab-close, .tab-pin") &&
      box(node.querySelector(".tab-close, .tab-pin")).width > 0),
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
      }, theme);
      let open = 0;
      let base = null;
      for (const count of COUNTS) {
        await grow(page, open, count);
        open = count;
        for (const width of WIDTHS) {
          await page.setViewportSize({ width, height: 860 });
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
          if (m.scrolls) {
            ok(`tabs shrink to their floor before the strip scrolls (${tag})`,
              m.narrowest === m.minWidth && m.ellipsis && m.closeKept, JSON.stringify(m));
          }
          if (count === 60 && (width === 800 || width === 720)) {
            await page.screenshot({ path: `${shots}/${theme}-60-${width}.png` });
          }
        }
      }

      // The keyboard's way through the tabs lands on a tab the person can see.
      await page.setViewportSize({ width: 720, height: 860 });
      await page.evaluate(() => setActiveTab(el("tabstrip").querySelector(".tab").dataset.tab));
      await page.keyboard.down("Control");
      await page.keyboard.press("Shift+Tab");
      await page.keyboard.up("Control");
      const keyed = await page.evaluate(() => {
        const strip = el("tabstrip");
        const node = strip.querySelector(".tab.is-active");
        const fade = parseFloat(getComputedStyle(strip).scrollPaddingLeft);
        const it = node.getBoundingClientRect();
        const view = strip.getBoundingClientRect();
        return { moved: node !== strip.querySelector(".tab"), left: it.left - view.left, right: view.right - it.right, fade };
      });
      ok(`Control+Shift+Tab lands on a tab clear of the fade (${theme})`,
        keyed.moved && keyed.left >= keyed.fade - 1 && keyed.right >= -1, JSON.stringify(keyed));

      // A repaint with the same tab in front keeps where the person scrolled.
      const kept = await page.evaluate(() => {
        const strip = el("tabstrip");
        strip.scrollLeft = 300;
        const before = strip.scrollLeft;
        renderTabs();
        return { before, after: strip.scrollLeft };
      });
      ok(`the strip's scroll survives a repaint (${theme})`, kept.after === kept.before, JSON.stringify(kept));

      // Dragging still reorders with sixty tabs: the front tab, by hand, one
      // neighbour to the left.
      const drag = await page.evaluate(() => {
        const strip = el("tabstrip");
        const all = strip.querySelectorAll(".tab");
        // A tab from the middle of sixty, brought in front and scrolled to
        // the strip's middle, so it and its left neighbour are both in reach.
        setActiveTab(all[30].dataset.tab);
        const it = all[30].getBoundingClientRect();
        const view = strip.getBoundingClientRect();
        strip.scrollLeft += it.left + it.width / 2 - (view.left + view.width / 2);
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
