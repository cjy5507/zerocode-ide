import { openWindowTestPage } from "./window-boot.mjs";

/* Computer Use beside the conversation, stage 1 (t-55387, parent t-54014).
 *
 * The band over the window (`ui/shell-computer.js`) already says the operator
 * is acting and offers the person's stop. What it does not do, and what this
 * suite holds the window to:
 *
 *  - the person's stop is the backend's to grant: the page shows the person
 *    holding the operator only after the backend answered, and a report the
 *    backend made before that answer cannot take the stop back off the screen
 *    (every report carries the backend's `revision`, and an older one is
 *    stale);
 *  - one press takes control once, however fast it is pressed again;
 *  - giving control back needs the person's explicit second press and a
 *    target the backend pinned, still the newest one — an unknown or replaced
 *    target sends nothing;
 *  - a panel beside the conversation shows the newest frame the operator's own
 *    look already took, fetched once per announced frame, for the one viewer it
 *    was opened for — another pane's, run's or target's frame or report never
 *    reaches it;
 *  - while nothing happens the panel does nothing: no request, no repaint;
 *    closing it lets go of its frame, and opening it again adds no listener.
 *
 * The contract the page is held to, all of it the backend's to decide:
 *
 *   computer:activity  { revision, owner: "agent"|"person"|"none", viewer,
 *                        target: { key, app } | null, stopped, active, actions, verb }
 *   computer:frame     { revision, viewer, target: { key, app }, at }   (no pixels)
 *   computer_progress_frame { viewer, revision } → { revision, png } | null
 *   computer_stop      { revision } → the backend's activity report
 *   computer_resume    { reset: false, revision, target } → the backend's report
 *   openComputerProgress(viewer) / closeComputerProgress()
 *   #computer-progress[data-owner] · #computer-progress-frame[data-revision]
 *   #computer-progress-take · #computer-progress-give
 *   #computer-progress-give-confirm · #computer-progress-give-cancel
 *
 * Every frame here is a synthetic one-pixel picture; no real screen, app or
 * code is read. A headless page cannot prove the native hand-over — that the
 * helper is stopped, or that a hidden browser tab took no global input — and
 * no check here claims it does.
 *
 *   WINDOW_SUITES=computer-progress node ui/tests/window.mjs */

const VIEWER = "pane:term-1";
const OTHER_VIEWER = "pane:term-2";
const TARGET = Object.freeze({ key: "viewer:pane:term-1/app:Fixture/window:front", app: "Fixture" });
const OTHER_TARGET = Object.freeze({ key: "viewer:pane:term-1/app:Other/window:front", app: "Other" });
const ONE_PIXEL_PNG =
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";
/* How long the idle check watches, in milliseconds: longer than the band's
 * own 8,000 ms slip-away would be a slow suite, and any timer the panel
 * armed for itself would fire well inside this. */
const IDLE_WATCH_MS = 1500;
const OPEN_CLOSE_CYCLES = 5;

/* The page-side hands every case shares: fire a backend event, wait a beat,
 * hold an answer until the case lets it go, and read what the panel shows.
 * A missing piece of the panel is said, not thrown, so each case reports its
 * own verdict. */
const installHands = () => {
  window.__cu = {
    fire: (name, payload) => {
      for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload });
    },
    settle: () => new Promise((done) => setTimeout(done, 80)),
    held: (command) => {
      const hold = { calls: [], release: null };
      window.__ANSWER__[command] = (args) => new Promise((answer) => {
        hold.calls.push(args);
        hold.release = answer;
      });
      return hold;
    },
    count: (command) => window.__COUNTS__?.[command] ?? 0,
    panel: () => document.getElementById("computer-progress"),
    piece: (id) => document.getElementById(id),
    open: (viewer) => {
      if (typeof openComputerProgress !== "function") return false;
      openComputerProgress(viewer);
      return true;
    },
    close: () => {
      if (typeof closeComputerProgress !== "function") return false;
      closeComputerProgress();
      return true;
    },
    activity: (fields) => ({
      active: true, verb: "click", actions: 1, stopped: null, hotkey: "control+option+escape",
      owner: "agent", viewer: "pane:term-1", target: null, ...fields,
    }),
    reset: () => {
      for (const name of ["computer_stop", "computer_resume", "computer_progress_frame"]) delete window.__ANSWER__[name];
    },
  };
};

const evaluateCase = (page, body, args) =>
  page.evaluate(body, args).catch((error) => ({ thrown: String(error?.message ?? error).slice(0, 300) }));

export async function testComputerProgress(browser, origin, ok) {
  const { page } = await openWindowTestPage(browser, origin);
  try {
    await page.evaluate(installHands);

    /* Take control: the person's stop shows only after the backend answered,
     * and a report the backend made before that answer cannot undo it. */
    const take = await evaluateCase(page, async () => {
      const { fire, settle, held, count, panel } = window.__cu;
      const seen = {};
      fire("computer:activity", window.__cu.activity({ revision: 10, target: { key: "t", app: "Fixture" } }));
      await settle();
      const stop = held("computer_stop");
      const before = count("computer_stop");
      document.getElementById("computer-band-stop").click();
      document.getElementById("computer-band-stop").click();
      await settle();
      seen.stopsSent = count("computer_stop") - before;
      seen.stopArgs = stop.calls[0] ?? null;
      seen.wordWhileWaiting = document.getElementById("computer-band-word").textContent;
      seen.stopShownWhileWaiting = !document.getElementById("computer-band-stop").hidden;
      stop.release?.(window.__cu.activity({ revision: 11, owner: "person", stopped: "window", active: false }));
      fire("computer:activity", window.__cu.activity({ revision: 11, owner: "person", stopped: "window", active: false }));
      await settle();
      seen.resumeShownAfterAck = !document.getElementById("computer-band-resume").hidden;
      fire("computer:activity", window.__cu.activity({ revision: 10, owner: "agent", stopped: null, active: true }));
      await settle();
      seen.resumeShownAfterStale = !document.getElementById("computer-band-resume").hidden;
      seen.stopShownAfterStale = !document.getElementById("computer-band-stop").hidden;
      seen.panelOwner = panel()?.dataset.owner ?? null;
      window.__cu.reset();
      return seen;
    });
    ok(
      "the person's stop is sent once however fast it is pressed, and carries the revision the page last saw",
      take.stopsSent === 1 && take.stopArgs?.revision === 10,
      JSON.stringify(take),
    );
    ok(
      "the page does not show the person holding the operator before the backend answered the stop",
      take.stopShownWhileWaiting === true && !String(take.wordWhileWaiting ?? "").includes("window"),
      JSON.stringify(take),
    );
    ok(
      "a report older than the backend's answer cannot take the person's stop back off the screen",
      take.resumeShownAfterAck === true && take.resumeShownAfterStale === true && take.stopShownAfterStale === false,
      JSON.stringify(take),
    );

    /* The panel beside the conversation: one announced frame, one fetch, one
     * repaint — for the viewer it was opened for, and the newest only. */
    const frames = await evaluateCase(page, async ({ viewer, otherViewer, target, otherTarget, png }) => {
      const { fire, settle, held, count, panel, piece, open } = window.__cu;
      const seen = { opened: open(viewer) };
      seen.panelShown = panel() !== null && !panel().hidden;
      const fetch = held("computer_progress_frame");
      const base = count("computer_progress_frame");
      fire("computer:frame", { revision: 20, viewer, target, at: 1 });
      await settle();
      seen.fetchesForOne = count("computer_progress_frame") - base;
      seen.fetchArgs = fetch.calls[0] ?? null;
      fetch.release?.({ revision: 20, png });
      await settle();
      seen.frameRevision = piece("computer-progress-frame")?.dataset.revision ?? null;
      const afterFirst = count("computer_progress_frame");
      fire("computer:frame", { revision: 19, viewer, target, at: 2 });
      fire("computer:frame", { revision: 30, viewer: otherViewer, target, at: 3 });
      fire("computer:frame", { revision: 31, viewer, target: { ...otherTarget, key: otherTarget.key.replace(viewer, otherViewer) }, at: 4 });
      fire("computer:activity", window.__cu.activity({ revision: 32, viewer: otherViewer, owner: "person", stopped: "window", active: false }));
      await settle();
      seen.fetchesForForeignOrStale = count("computer_progress_frame") - afterFirst;
      seen.ownerAfterForeign = panel()?.dataset.owner ?? null;
      seen.frameAfterForeign = piece("computer-progress-frame")?.dataset.revision ?? null;
      const late = held("computer_progress_frame");
      fire("computer:frame", { revision: 40, viewer, target, at: 5 });
      await settle();
      const lateFirst = late.release;
      fire("computer:frame", { revision: 41, viewer, target, at: 6 });
      await settle();
      late.release?.({ revision: 41, png });
      lateFirst?.({ revision: 40, png });
      await settle();
      seen.frameAfterLateReply = piece("computer-progress-frame")?.dataset.revision ?? null;
      window.__cu.close();
      window.__cu.reset();
      return seen;
    }, { viewer: VIEWER, otherViewer: OTHER_VIEWER, target: TARGET, otherTarget: OTHER_TARGET, png: ONE_PIXEL_PNG });
    ok(
      "the progress panel opens beside the conversation for one viewer",
      frames.opened === true && frames.panelShown === true,
      JSON.stringify(frames),
    );
    ok(
      "one announced frame is fetched once, for its own viewer and revision, and painted once",
      frames.fetchesForOne === 1 && frames.fetchArgs?.viewer === VIEWER && frames.fetchArgs?.revision === 20 &&
        frames.frameRevision === "20",
      JSON.stringify(frames),
    );
    ok(
      "a stale frame, another pane's frame or report, or another viewer's target never reaches the panel",
      frames.fetchesForForeignOrStale === 0 && frames.frameAfterForeign === "20" && frames.ownerAfterForeign !== "person",
      JSON.stringify(frames),
    );
    ok(
      "a late reply for an older frame cannot replace the newer frame on screen",
      frames.frameAfterLateReply === "41",
      JSON.stringify(frames),
    );

    /* Giving control back: two presses, a pinned target, still the newest. */
    const give = await evaluateCase(page, async ({ viewer, target, otherTarget }) => {
      const { fire, settle, count, piece, open } = window.__cu;
      const seen = { opened: open(viewer) };
      const resumed = [];
      window.__ANSWER__.computer_resume = (args) => {
        resumed.push(args);
        return window.__cu.activity({ revision: 60, owner: "agent", stopped: null, active: false, target: args?.target ?? null });
      };
      const base = count("computer_resume");
      fire("computer:activity", window.__cu.activity({ revision: 50, owner: "person", stopped: "window", active: false, target: null }));
      await settle();
      seen.giveDisabledWithoutTarget = piece("computer-progress-give")?.disabled ?? null;
      piece("computer-progress-give")?.click();
      await settle();
      piece("computer-progress-give-confirm")?.click();
      await settle();
      seen.sentWithoutTarget = count("computer_resume") - base;
      fire("computer:activity", window.__cu.activity({ revision: 51, owner: "person", stopped: "window", active: false, target }));
      await settle();
      piece("computer-progress-give")?.click();
      await settle();
      seen.sentOnFirstPress = count("computer_resume") - base;
      seen.confirmShown = piece("computer-progress-give-confirm") !== null && !piece("computer-progress-give-confirm").hidden;
      piece("computer-progress-give-cancel")?.click();
      await settle();
      seen.sentAfterCancel = count("computer_resume") - base;
      piece("computer-progress-give")?.click();
      await settle();
      fire("computer:activity", window.__cu.activity({ revision: 52, owner: "person", stopped: "window", active: false, target: otherTarget }));
      await settle();
      seen.confirmAfterTargetMoved = piece("computer-progress-give-confirm") !== null && !piece("computer-progress-give-confirm").hidden;
      piece("computer-progress-give-confirm")?.click();
      await settle();
      seen.sentForMovedTarget = count("computer_resume") - base;
      piece("computer-progress-give")?.click();
      await settle();
      piece("computer-progress-give-confirm")?.click();
      piece("computer-progress-give-confirm")?.click();
      await settle();
      seen.sentForFreshTarget = count("computer_resume") - base;
      seen.resumeArgs = resumed[0] ?? null;
      window.__cu.close();
      window.__cu.reset();
      return seen;
    }, { viewer: VIEWER, target: TARGET, otherTarget: OTHER_TARGET });
    ok(
      "give-back sends nothing while the backend has pinned no target",
      give.opened === true && give.giveDisabledWithoutTarget === true && give.sentWithoutTarget === 0,
      JSON.stringify(give),
    );
    ok(
      "give-back needs the person's explicit second press, and cancelling it sends nothing",
      give.sentOnFirstPress === 0 && give.confirmShown === true && give.sentAfterCancel === 0,
      JSON.stringify(give),
    );
    ok(
      "a target the backend replaced takes the person's confirmation away and sends nothing for it",
      give.opened === true && give.confirmAfterTargetMoved === false && give.sentForMovedTarget === 0,
      JSON.stringify(give),
    );
    ok(
      "a confirmed give-back is sent once, for the newest revision and its pinned target",
      give.sentForFreshTarget === 1 && give.resumeArgs?.revision === 52 &&
        give.resumeArgs?.target?.key === OTHER_TARGET.key && give.resumeArgs?.reset === false,
      JSON.stringify(give),
    );

    /* Idle, closed, opened again: no request, no repaint, nothing kept. */
    const quiet = await evaluateCase(page, async ({ viewer, target, png, watchMs, cycles }) => {
      const { fire, settle, count, panel, piece, open, close } = window.__cu;
      const seen = {};
      const listeners = () => ["computer:activity", "computer:frame"]
        .reduce((sum, name) => sum + (window.__LISTENERS__[name]?.length ?? 0), 0);
      const listenersBefore = listeners();
      window.__ANSWER__.computer_progress_frame = (args) => ({ revision: args.revision, png });
      seen.opened = open(viewer);
      fire("computer:frame", { revision: 70, viewer, target, at: 1 });
      await settle();
      const commands = () => Object.entries(window.__COUNTS__ ?? {})
        .filter(([name]) => name.startsWith("computer_"))
        .reduce((sum, [, n]) => sum + n, 0);
      const requestsBefore = commands();
      let mutations = 0;
      const watcher = panel() ? new MutationObserver((records) => { mutations += records.length; }) : null;
      watcher?.observe(panel(), { subtree: true, childList: true, attributes: true, characterData: true });
      await new Promise((done) => setTimeout(done, watchMs));
      watcher?.disconnect();
      seen.idleRequests = commands() - requestsBefore;
      seen.idleMutations = watcher ? mutations : null;
      seen.frameBeforeClose = piece("computer-progress-frame")?.getAttribute("src") ?? null;
      seen.closed = close();
      await settle();
      seen.hiddenAfterClose = panel()?.hidden ?? null;
      seen.frameAfterClose = piece("computer-progress-frame")?.getAttribute("src") ?? null;
      const fetchedClosed = count("computer_progress_frame");
      fire("computer:frame", { revision: 71, viewer, target, at: 2 });
      await settle();
      seen.fetchesWhileClosed = count("computer_progress_frame") - fetchedClosed;
      for (let round = 0; round < cycles; round += 1) {
        open(viewer);
        close();
      }
      await settle();
      seen.listenerGrowth = listeners() - listenersBefore;
      window.__cu.reset();
      return seen;
    }, { viewer: VIEWER, target: TARGET, png: ONE_PIXEL_PNG, watchMs: IDLE_WATCH_MS, cycles: OPEN_CLOSE_CYCLES });
    ok(
      "an open panel with nothing happening sends no request and repaints nothing",
      quiet.opened === true && quiet.idleRequests === 0 && quiet.idleMutations === 0 && Boolean(quiet.frameBeforeClose),
      JSON.stringify(quiet),
    );
    ok(
      "closing the panel lets go of its frame, and a frame announced while it is closed is not fetched",
      quiet.closed === true && quiet.hiddenAfterClose === true && !quiet.frameAfterClose && quiet.fetchesWhileClosed === 0,
      JSON.stringify(quiet),
    );
    ok(
      "opening and closing the panel again adds no listener",
      quiet.opened === true && quiet.listenerGrowth === 0,
      JSON.stringify(quiet),
    );

    /* A frame the backend could not give: the last good frame stays, and
     * nothing is asked again on its own. */
    const failed = await evaluateCase(page, async ({ viewer, target, png, watchMs }) => {
      const { fire, settle, count, piece, open } = window.__cu;
      const seen = { opened: open(viewer) };
      window.__ANSWER__.computer_progress_frame = (args) => ({ revision: args.revision, png });
      fire("computer:frame", { revision: 80, viewer, target, at: 1 });
      await settle();
      window.__ANSWER__.computer_progress_frame = () => { throw new Error("no frame"); };
      const base = count("computer_progress_frame");
      fire("computer:frame", { revision: 81, viewer, target, at: 2 });
      await new Promise((done) => setTimeout(done, watchMs));
      seen.fetchesAfterFailure = count("computer_progress_frame") - base;
      seen.frameAfterFailure = piece("computer-progress-frame")?.dataset.revision ?? null;
      window.__cu.close();
      window.__cu.reset();
      await settle();
      return seen;
    }, { viewer: VIEWER, target: TARGET, png: ONE_PIXEL_PNG, watchMs: IDLE_WATCH_MS });
    ok(
      "a frame that failed to arrive keeps the last good frame and is not retried on a timer",
      failed.opened === true && failed.fetchesAfterFailure === 1 && failed.frameAfterFailure === "80",
      JSON.stringify(failed),
    );
  } finally { await page.close(); }
}

if (process.argv[1] && import.meta.url === (await import("node:url")).pathToFileURL(process.argv[1]).href) {
  const { chromium, createWindowServer } = await import("./window-boot.mjs");
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch();
  try {
    await testComputerProgress(browser, origin, (name, pass, detail) => {
      console.log(`${pass ? "PASS" : "FAIL"} ${name} ${detail}`);
      if (!pass) process.exitCode = 1;
    });
  } finally { await browser.close(); files.close(); }
}
