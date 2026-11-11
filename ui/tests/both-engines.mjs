/* A window suite run on its own, in both layout engines.
 *
 * The window gate (`window.mjs`) drives Chromium; the installed window is a
 * WKWebView. What a terminal draws is the engine's answer — which face it
 * falls back to, where it rounds a baseline — so the suites that measure it
 * can also be run alone against both: `node ui/tests/<suite>.mjs`. One JSON
 * line per check, and the exit code is red when any check was. */
import { createRequire } from "node:module";
import { pathToFileURL } from "node:url";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";

/** Whether the module at `url` is the script node was started with. */
export const startedAs = (url) => Boolean(process.argv[1]) && url === pathToFileURL(process.argv[1]).href;

/** Run `tests` — each `(page, ok)` — on one fresh window page per engine. */
export async function runInBothEngines(tests) {
  const { webkit } = createRequire(import.meta.url)("playwright");
  const { files, origin } = await createWindowServer();
  try {
    for (const [name, engine] of [["chromium", chromium], ["webkit", webkit]]) {
      const browser = await engine.launch();
      try {
        const { page, faults } = await openWindowTestPage(browser, origin);
        const record = (test, pass, detail) => {
          console.log(JSON.stringify({ engine: name, test, pass, detail: JSON.parse(detail) }));
          if (!pass) process.exitCode = 1;
        };
        for (const test of tests) await test(page, record);
        if (faults.length) throw new Error(faults.join("\n"));
      } finally { await browser.close(); }
    }
  } finally { files.close(); }
}
