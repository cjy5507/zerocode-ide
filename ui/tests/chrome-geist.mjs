import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";

/* The window's chrome in the passed design 「또렷한 Geist」 (p-6893560125c34552,
 * the person 2026-10-28 02:5x; t-78107). Each scene below is one item of the
 * design's 「무엇이 바뀌나」 list, measured on a laid-out page — computed
 * style and boxes, not pictures. */

const frames = (page) => page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));

/* Geist ships with the window: both faces load from the window's own files
 * and the chrome draws in them, not in the system face the stack falls to. */
async function testGeistShips(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    const drawn = await page.evaluate(async () => {
      await Promise.all([document.fonts.load("13px Geist"), document.fonts.load("13px 'Geist Mono'")]);
      await document.fonts.ready;
      const loaded = (family) => [...document.fonts].some((face) => face.family.replace(/"/g, "") === family && face.status === "loaded");
      // A face that is not there draws as the generic after it; a monospace
      // after the sans face and a serif after the mono one make that measurable.
      const width = (font) => {
        const pen = document.createElement("canvas").getContext("2d");
        pen.font = font;
        return pen.measureText("Geist 지금 누가 나를 필요로 하는가 0123456789").width;
      };
      return {
        sans: loaded("Geist"),
        mono: loaded("Geist Mono"),
        sansDiffers: width("13px Geist, monospace") !== width("13px monospace"),
        monoDiffers: width("13px 'Geist Mono', serif") !== width("13px serif"),
        chrome: getComputedStyle(document.querySelector(".titlebar")).fontFamily,
      };
    });
    ok("Geist and Geist Mono load from the window's own files", drawn.sans && drawn.mono, JSON.stringify(drawn));
    ok("the faces draw, not their fallbacks", drawn.sansDiffers && drawn.monoDiffers, JSON.stringify(drawn));
    ok("the chrome asks for Geist first", /^"?Geist"?,/.test(drawn.chrome), drawn.chrome);
    ok("the font scene raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

export async function testChromeGeist(browser, origin, ok) {
  await testGeistShips(browser, origin, ok);
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1] ?? "")).href) {
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  let failed = 0;
  try {
    await testChromeGeist(browser, origin, (name, pass, detail) => {
      if (!pass) failed += 1;
      console.log(`${pass ? "PASS" : "FAIL"}  ${name}${pass ? "" : "  — " + (detail ?? "")}`);
    });
  } finally {
    await browser.close();
    files.close();
  }
  process.exit(failed ? 1 : 0);
}

export { frames };
