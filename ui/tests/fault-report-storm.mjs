import { pathToFileURL } from "node:url";
import { resolve } from "node:path";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";

/* A window reports a fault, not every echo of it (t-20972).
 *
 * The window tells the backend about every uncaught error and rejection
 * (`log_window_error`), because a release webview has no console anybody can
 * read. That report is itself a request, and it used to be one nobody caught:
 * when the backend refused it, its rejection was an uncaught rejection, which
 * is a fault, which was reported, which was refused, which was a fault — one
 * request chain per fault, running for as long as the page lived.
 *
 * The backend refuses every command of a webview it does not know. The artifact
 * gallery's thumbnail pane is such a webview, and it loads whatever page an
 * artifact names — on 2026-10-01 that was this window's own `ui/index.html`
 * (the picture the pane took of it is in the gallery's cache), whose boot fires
 * dozens of requests that are refused. Every refused one became a chain, and a
 * chain is one request on the main thread's custom-scheme road for each turn of
 * the page's event loop. The 7.4 s hang of 17:28 ended with that thumbnail; a
 * page of that shape asks about ten thousand times a second (`tools/ipc-road`,
 * its `storm` page), and the first check below counts the chain.
 *
 * The rules pinned here:
 *   - a report the backend refuses is nobody's fault: one fault, one report;
 *   - a fault that repeats is reported a bounded number of times at once, and
 *     heard again once the allowance refills — a recurring fault stays visible
 *     while its repeats stay cheap;
 *   - the report still says what the fault was, for a backend that answers;
 *   - the Korean-input husk is told through the same door (`tellWindowLog`),
 *     so it has the same rule and no second way into the window log. */
export async function testFaultReportStorm(browser, origin, ok) {
  // One page at a time: before the fix the first never stops asking, and a page
  // that is still spinning must not be sharing a renderer with the next check.
  const refused = await openWindowTestPage(browser, origin);
  try {
    const refusedSeen = await refused.page.evaluate(async () => {
      const wait = (ms) => new Promise((done) => setTimeout(done, ms));
      const asked = () => window.__COUNTS__.log_window_error ?? 0;
      // A webview the backend does not know: it refuses the report itself.
      window.__FAIL__.add("log_window_error");
      window.__COUNTS__ = {};
      Promise.reject(new Error("a fault in a webview the backend does not know"));
      await wait(500);
      const reports = asked();
      const mark = reports;
      await wait(250);
      return { reports, stillAsking: asked() - mark };
    });
    ok(
      "a report the backend refuses is not a fault of its own — one fault, one report, and the request chain ends there",
      refusedSeen.reports >= 1 && refusedSeen.reports <= 2 && refusedSeen.stillAsking === 0,
      JSON.stringify(refusedSeen),
    );
  } finally {
    await refused.page.close();
  }

  const answered = await openWindowTestPage(browser, origin);
  try {
    const answeredSeen = await answered.page.evaluate(async () => {
      const wait = (ms) => new Promise((done) => setTimeout(done, ms));
      const messages = [];
      window.__ANSWER__.log_window_error = (args) => (messages.push(args.message), null);
      const fault = (text) =>
        window.dispatchEvent(new ErrorEvent("error", { message: text, filename: "x.js", lineno: 7 }));
      // The page declares what it lets through at once and how long an
      // allowance takes to refill; a tree that declares nothing has no bound.
      const burst = typeof FAULT_REPORT_BURST === "number" ? FAULT_REPORT_BURST : null;
      const refill = typeof FAULT_REPORT_REFILL_MS === "number" ? FAULT_REPORT_REFILL_MS : null;

      fault("the first fault");
      await wait(30);
      const first = messages.slice();

      for (let n = 0; n < 200; n += 1) fault("the same fault again");
      await wait(60);
      const afterRepeats = messages.length;

      // The Korean-input husk has one door into the window log, the faults'
      // own: with the allowance spent it is held back too.
      strayJamoReportedAt = 0;
      reportStrayJamo("rescue.flush", "ㅋ");
      await wait(30);
      const huskWhileSpent = messages.length - afterRepeats;

      await wait((refill ?? 1000) + 150);
      fault("a fault after the allowance refilled");
      await wait(30);
      const afterRefill = messages.length;
      const heardAgain = messages.filter((line) => line.includes("a fault after the allowance refilled")).length;
      const last = messages.at(-1) ?? "";

      await wait((refill ?? 1000) + 150);
      strayJamoReportedAt = 0;
      reportStrayJamo("rescue.flush", "ㅋ");
      await wait(30);
      return {
        burst,
        refill,
        first,
        afterRepeats,
        huskWhileSpent,
        heardAgain,
        last,
        huskAfterRefill: messages.length - afterRefill,
        huskLine: messages.at(-1) ?? "",
      };
    });
    ok(
      "a fault thrown over and over is reported a bounded number of times, however fast it repeats",
      answeredSeen.burst !== null &&
        answeredSeen.afterRepeats > 1 &&
        answeredSeen.afterRepeats <= answeredSeen.burst,
      JSON.stringify(answeredSeen),
    );
    ok(
      "the report still says what the fault was, and a later fault is heard once the allowance has refilled",
      answeredSeen.first.length === 1 &&
        answeredSeen.first[0].includes("the first fault") &&
        answeredSeen.first[0].includes("x.js:7") &&
        answeredSeen.heardAgain === 1 &&
        answeredSeen.last.includes("a fault after the allowance refilled"),
      JSON.stringify(answeredSeen),
    );
    ok(
      "the Korean-input husk speaks through the same door and the same allowance as the faults",
      answeredSeen.huskWhileSpent === 0 &&
        answeredSeen.huskAfterRefill === 1 &&
        answeredSeen.huskLine.startsWith("ime: bare jamo left through rescue.flush"),
      JSON.stringify(answeredSeen),
    );
  } finally {
    await answered.page.close();
  }
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  let failures = 0;
  try {
    await testFaultReportStorm(browser, origin, (name, pass, detail = "") => {
      console.log(`${pass ? "PASS" : "FAIL"} ${name}${!pass && detail ? `\n${detail}` : ""}`);
      if (!pass) failures++;
    });
  } finally { await browser.close(); files.close(); }
  if (failures) process.exitCode = 1;
}
