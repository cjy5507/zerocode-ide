import { openWindowTestPage } from "./window-boot.mjs";

const STOP_HOTKEY = "control+option+escape";
/* A pane (`term`) and the assignment it held when it looked (`scope`): what the
 * window's own door (`computer_progress_frame`) answers for, and nothing a
 * model names. */
const SUBJECT = Object.freeze({ term: 7, scope: "scope-a" });
const OTHER_TERM = 8;
const OTHER_SCOPE = "scope-b";
const NEXT_SCOPE = "scope-next";
const BURST_FRAMES = 8;
const OPEN_CLOSE_CYCLES = 5;

const installComputerSurfaceCensus = () => {
  const surfaceFiles = "/shell-computer";
  const bandSlipAway = "computerBandSlipAway";
  const live = { timeouts: new Map(), intervals: new Map(), animationFrames: new Map(), objectUrls: new Map() };
  const native = {
    setTimeout: window.setTimeout.bind(window),
    clearTimeout: window.clearTimeout.bind(window),
    setInterval: window.setInterval.bind(window),
    clearInterval: window.clearInterval.bind(window),
    requestAnimationFrame: window.requestAnimationFrame.bind(window),
    cancelAnimationFrame: window.cancelAnimationFrame.bind(window),
    createObjectURL: URL.createObjectURL.bind(URL),
    revokeObjectURL: URL.revokeObjectURL.bind(URL),
  };
  const creatorOfThisCall = () => {
    const stack = new Error().stack ?? "";
    return { surface: stack.includes(surfaceFiles), bandSlipAway: stack.includes(bandSlipAway) };
  };
  window.setTimeout = (callback, ...rest) => {
    if (typeof callback !== "function") return native.setTimeout(callback, ...rest);
    const creator = creatorOfThisCall();
    const handle = native.setTimeout((...fired) => {
      live.timeouts.delete(handle);
      return callback(...fired);
    }, ...rest);
    live.timeouts.set(handle, creator);
    return handle;
  };
  window.clearTimeout = (handle) => {
    live.timeouts.delete(handle);
    live.intervals.delete(handle);
    return native.clearTimeout(handle);
  };
  window.setInterval = (callback, ...rest) => {
    const creator = creatorOfThisCall();
    const handle = native.setInterval(callback, ...rest);
    live.intervals.set(handle, creator);
    return handle;
  };
  window.clearInterval = (handle) => {
    live.intervals.delete(handle);
    live.timeouts.delete(handle);
    return native.clearInterval(handle);
  };
  window.requestAnimationFrame = (callback) => {
    const creator = creatorOfThisCall();
    const handle = native.requestAnimationFrame((time) => {
      live.animationFrames.delete(handle);
      return callback(time);
    });
    live.animationFrames.set(handle, creator);
    return handle;
  };
  window.cancelAnimationFrame = (handle) => {
    live.animationFrames.delete(handle);
    return native.cancelAnimationFrame(handle);
  };
  URL.createObjectURL = (object) => {
    const creator = creatorOfThisCall();
    const url = native.createObjectURL(object);
    live.objectUrls.set(url, creator);
    return url;
  };
  URL.revokeObjectURL = (url) => {
    live.objectUrls.delete(url);
    return native.revokeObjectURL(url);
  };
  const countFromSurface = (table, wanted = () => true) =>
    [...table.values()].filter((creator) => creator.surface && wanted(creator)).length;
  window.__COMPUTER_SURFACE_CENSUS__ = {
    native,
    live: () => ({
      timeouts: countFromSurface(live.timeouts, (creator) => !creator.bandSlipAway),
      bandSlipAway: countFromSurface(live.timeouts, (creator) => creator.bandSlipAway),
      intervals: countFromSurface(live.intervals),
      animationFrames: countFromSurface(live.animationFrames),
      objectUrls: countFromSurface(live.objectUrls),
    }),
  };
};

const installComputerHands = ({ hotkey }) => {
  const census = window.__COMPUTER_SURFACE_CENSUS__;
  const nextFrame = () => new Promise((done) => census.native.requestAnimationFrame(() => done()));
  const nextTask = () => new Promise((done) => census.native.setTimeout(done, 0));
  const copyOf = (args) => (args === undefined ? null : JSON.parse(JSON.stringify(args)));
  window.__COMPUTER_HANDS__ = {
    census,
    settle: async () => {
      await nextTask();
      await nextFrame();
      await nextTask();
      await nextFrame();
    },
    announce: (name, payload) => {
      for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload });
    },
    operatorReport: (fields) => ({
      active: false, verb: "click", actions: 3, at: 1791990000000, stopped: null, hotkey, ...fields,
    }),
    readBand: () => {
      const band = document.getElementById("computer-band");
      const note = document.getElementById("computer-band-note");
      const view = document.getElementById("computer-band-view");
      return {
        shown: !band.hidden,
        showsStop: band.classList.contains("is-stopped"),
        offersStop: !document.getElementById("computer-band-stop").hidden,
        offersResume: !document.getElementById("computer-band-resume").hidden,
        offersView: view !== null && !view.hidden,
        note: note && !note.hidden ? note.textContent : null,
      };
    },
    pressBand: (button) => document.getElementById(`computer-band-${button}`).click(),
    sentCount: (command) => window.__COUNTS__[command] ?? 0,
    recordCommand: (command, answer) => {
      const calls = [];
      window.__ANSWER__[command] = (args) => {
        calls.push(copyOf(args));
        return answer(args);
      };
      return calls;
    },
    holdCommand: (command) => {
      const waiting = [];
      const held = { calls: [], fromComputerFiles: [], inFlight: 0, mostInFlight: 0 };
      window.__ANSWER__[command] = (args) => new Promise((resolve, reject) => {
        held.calls.push(copyOf(args));
        held.fromComputerFiles.push((new Error().stack ?? "").includes("/shell-computer"));
        held.inFlight += 1;
        held.mostInFlight = Math.max(held.mostInFlight, held.inFlight);
        waiting.push({ resolve, reject });
      });
      held.answerOldest = (value) => {
        const oldest = waiting.shift();
        if (!oldest) return false;
        held.inFlight -= 1;
        oldest.resolve(value);
        return true;
      };
      held.refuseOldest = (reason) => {
        const oldest = waiting.shift();
        if (!oldest) return false;
        held.inFlight -= 1;
        oldest.reject(new Error(reason));
        return true;
      };
      held.outstanding = () => waiting.length;
      return held;
    },
    refuseCommand: (command) => {
      window.__ANSWER__[command] = () => {
        throw new Error(`refused: ${command}`);
      };
    },
    releaseCommands: () => {
      for (const command of ["computer_stop", "computer_resume", "computer_progress_frame"]) {
        delete window.__ANSWER__[command];
      }
    },
    framePicture: (seq) => {
      const canvas = document.createElement("canvas");
      canvas.width = 2;
      canvas.height = 2;
      const drawing = canvas.getContext("2d");
      drawing.fillStyle = `rgb(${seq % 256}, ${(seq * 7) % 256}, ${(seq * 13) % 256})`;
      drawing.fillRect(0, 0, 2, 2);
      return canvas.toDataURL("image/png").split(",")[1];
    },
    /* The door's answer, shaped as the window gives it: the pane, the
     * assignment, the look's number and time, how it saw, and its picture
     * when one is kept. */
    look: (subject, seq, fields = {}) => ({
      term: subject.term, scope: subject.scope, seq, at: 1791990000000 + seq,
      perception: "screenshot", png: window.__COMPUTER_HANDS__.framePicture(seq), ...fields,
    }),
    openPanel: (subject) => {
      if (typeof openComputerProgress !== "function") return false;
      openComputerProgress(subject);
      return true;
    },
    closePanel: () => {
      if (typeof closeComputerProgress !== "function") return false;
      closeComputerProgress();
      return true;
    },
    readPanel: () => {
      const panel = document.getElementById("computer-progress");
      const frame = document.getElementById("computer-progress-frame");
      const note = document.getElementById("computer-progress-note");
      return {
        shown: panel !== null && !panel.hidden,
        seq: frame?.dataset.seq ?? null,
        picture: frame?.getAttribute("src") ?? null,
        note: note && !note.hidden ? note.textContent : null,
      };
    },
    frameListeners: () => ["computer:frame", "computer:activity"]
      .reduce((total, name) => total + (window.__LISTENERS__[name]?.length ?? 0), 0),
    /* The panel's say of who holds control: the desktop's line, each work
     * browser's row (its tab, its words, whether its button is offered) and
     * the hint beside the rows. */
    readControl: () => {
      const desk = document.getElementById("computer-progress-desk");
      const list = document.getElementById("computer-progress-browsers");
      const hint = document.getElementById("computer-progress-browser-hint");
      const rows = list && !list.hidden ? [...list.querySelectorAll("[data-tab]")] : [];
      return {
        desk: desk && !desk.hidden ? desk.textContent : null,
        rows: rows.map((row) => {
          const button = row.querySelector("button");
          return {
            tab: row.dataset.tab,
            words: row.querySelector("span")?.textContent ?? null,
            offersGo: !!button && !button.hidden && !button.disabled,
          };
        }),
        hint: hint && !hint.hidden ? hint.textContent : null,
      };
    },
  };
};

/* The dedicated work browser (t-62460) as its own suite stands it
 * (browser-workspace.mjs): the backend's answers for its tabs and its
 * `browser:workspace` word. The panel reads only what shell-browser.js took
 * in, so a mode change sent from here would be a second controller. */
const installWorkBrowsers = () => {
  const work = { modes: [], states: new Map() };
  window.__WORK_BROWSERS__ = work;
  window.__ANSWER__.open_browser_pane = (args) => args.label;
  window.__ANSWER__.browser_place = () => null;
  window.__ANSWER__.browser_zoom = () => null;
  window.__ANSWER__.close_browser_pane = () => null;
  window.__ANSWER__.set_browser_open_tabs = ({ tabs: records }) => ({ browser: { ...browserPrefs, open_tabs: records } });
  window.__ANSWER__.get_browser_workspaces = () => [...work.states.values()];
  window.__ANSWER__.set_browser_workspace_mode = (args) => {
    work.modes.push(args);
    return null;
  };
  work.open = (label) => {
    for (const hear of window.__LISTENERS__["browser:agent-open"] ?? []) {
      hear({ payload: { label, url: `https://example.com/${label}` } });
    }
  };
  work.say = (state) => {
    const said = { inFlight: 0, needsObservation: false, snapshotSupported: false, ...state };
    work.states.set(said.label, said);
    for (const hear of window.__LISTENERS__["browser:workspace"] ?? []) hear({ payload: said });
  };
  work.tab = (label) => tabs.find((one) => one.kind === "browser" && one.label === label) ?? null;
  work.words = (label) => {
    const tab = work.tab(label);
    return tab ? browserWorkspaceWords(tab) : null;
  };
};

const evaluateCase = (page, body, args) =>
  page.evaluate(body, args).catch((error) => ({ thrown: String(error?.message ?? error).slice(0, 300) }));

const asksFor = (asked, subject) => asked?.term === subject.term && asked?.scope === subject.scope;

const censusIsEmpty = (census) =>
  census?.timeouts === 0 && census?.intervals === 0 && census?.animationFrames === 0 && census?.objectUrls === 0;

async function openComputerPage(browser, origin) {
  const opened = await openWindowTestPage(browser, origin, {
    before: (page) => page.addInitScript(installComputerSurfaceCensus),
  });
  await opened.page.evaluate(installComputerHands, { hotkey: STOP_HOTKEY });
  return opened;
}

async function testTheBand(browser, origin, ok) {
  const { page, faults } = await openComputerPage(browser, origin);
  try {
    const legacy = await evaluateCase(page, async () => {
      const hands = window.__COMPUTER_HANDS__;
      hands.announce("computer:activity", hands.operatorReport({ active: true }));
      await hands.settle();
      const working = hands.readBand();
      hands.announce("computer:activity", hands.operatorReport({ stopped: "window" }));
      await hands.settle();
      const stopped = hands.readBand();
      const resumes = hands.recordCommand("computer_resume", () => ({ resumed: true, helper: "idle" }));
      hands.pressBand("resume");
      await hands.settle();
      hands.releaseCommands();
      return {
        working,
        stopped,
        resumes,
        resumesWithoutRevision: resumes.filter((args) => typeof args?.revision !== "number").length,
      };
    });
    ok(
      "before any versioned report, a report without a revision still paints the band as it does today",
      legacy.working?.shown === true && legacy.working?.showsStop === false && legacy.stopped?.showsStop === true,
      JSON.stringify(legacy),
    );
    ok(
      "a report without a revision gives no authority: no resume is sent without a revision the backend gave",
      legacy.resumesWithoutRevision === 0,
      JSON.stringify(legacy),
    );

    const take = await evaluateCase(page, async () => {
      const hands = window.__COMPUTER_HANDS__;
      hands.announce("computer:activity", hands.operatorReport({ revision: 10, active: true }));
      await hands.settle();
      const stops = hands.holdCommand("computer_stop");
      const resumesBefore = hands.sentCount("computer_resume");
      hands.pressBand("stop");
      hands.pressBand("stop");
      await hands.settle();
      const whileStopWaits = hands.readBand();
      hands.pressBand("resume");
      await hands.settle();
      const resumesWhileStopWaits = hands.sentCount("computer_resume") - resumesBefore;
      hands.announce("computer:activity", hands.operatorReport({ revision: 11, stopped: "window" }));
      stops.answerOldest({ stopped: true, reason: "window", helperSignalled: false });
      await hands.settle();
      const granted = hands.readBand();
      hands.announce("computer:activity", hands.operatorReport({ revision: 10, active: true }));
      await hands.settle();
      const afterOlderReport = hands.readBand();
      hands.announce("computer:activity", hands.operatorReport({ active: true }));
      await hands.settle();
      const afterUnversionedReport = hands.readBand();
      while (stops.outstanding() > 0) stops.answerOldest({ stopped: true, reason: "window", helperSignalled: false });
      await hands.settle();
      hands.releaseCommands();
      return {
        stopsSent: stops.calls.length,
        whileStopWaits,
        resumesWhileStopWaits,
        granted,
        afterOlderReport,
        afterUnversionedReport,
      };
    });
    ok(
      "one press of the band's stop sends one stop, however fast it is pressed again",
      take.stopsSent === 1,
      JSON.stringify(take),
    );
    ok(
      "while the stop waits for the backend the band neither shows it granted nor offers the resume, and a resume pressed then sends nothing",
      take.whileStopWaits?.showsStop === false && take.whileStopWaits?.offersResume === false &&
        take.resumesWhileStopWaits === 0,
      JSON.stringify(take),
    );
    ok(
      "the band shows the person's stop once the backend's versioned report grants it",
      take.granted?.showsStop === true && take.granted?.offersResume === true,
      JSON.stringify(take),
    );
    ok(
      "a versioned report older than the granted stop cannot take it off the band",
      take.afterOlderReport?.showsStop === true && take.afterOlderReport?.offersResume === true &&
        take.afterOlderReport?.offersStop === false,
      JSON.stringify(take),
    );
    ok(
      "a report without a revision that arrives after the granted stop cannot take it off the band",
      take.afterUnversionedReport?.showsStop === true && take.afterUnversionedReport?.offersResume === true,
      JSON.stringify(take),
    );

    const refusedStop = await evaluateCase(page, async () => {
      const hands = window.__COMPUTER_HANDS__;
      hands.announce("computer:activity", hands.operatorReport({ revision: 20, active: true }));
      await hands.settle();
      const stopsBefore = hands.sentCount("computer_stop");
      hands.refuseCommand("computer_stop");
      hands.pressBand("stop");
      await hands.settle();
      const afterRefusal = hands.readBand();
      hands.pressBand("stop");
      await hands.settle();
      hands.releaseCommands();
      return { afterRefusal, stopsSent: hands.sentCount("computer_stop") - stopsBefore };
    });
    ok(
      "a stop the backend refused is not shown as granted, and the band offers the stop again",
      refusedStop.afterRefusal?.showsStop === false && refusedStop.afterRefusal?.offersStop === true &&
        refusedStop.stopsSent === 2,
      JSON.stringify(refusedStop),
    );

    const resume = await evaluateCase(page, async () => {
      const hands = window.__COMPUTER_HANDS__;
      hands.announce("computer:activity", hands.operatorReport({ revision: 30, stopped: "window" }));
      await hands.settle();
      const resumes = hands.holdCommand("computer_resume");
      hands.pressBand("resume");
      hands.pressBand("resume");
      await hands.settle();
      const whileResumeWaits = hands.readBand();
      resumes.answerOldest({ resumed: true, helper: "idle" });
      hands.announce("computer:activity", hands.operatorReport({ revision: 31 }));
      await hands.settle();
      const lifted = hands.readBand();
      while (resumes.outstanding() > 0) resumes.answerOldest({ resumed: true, helper: "idle" });
      await hands.settle();
      hands.releaseCommands();
      return { resumesSent: resumes.calls.length, resumeArgs: resumes.calls[0] ?? null, whileResumeWaits, lifted };
    });
    ok(
      "one press of resume sends one resume, carrying the revision of the stop it lifts and reset false",
      resume.resumesSent === 1 && resume.resumeArgs?.revision === 30 && resume.resumeArgs?.reset === false,
      JSON.stringify(resume),
    );
    ok(
      "while the resume waits for the backend the band keeps showing the person's stop",
      resume.whileResumeWaits?.showsStop === true,
      JSON.stringify(resume),
    );
    ok(
      "the stop leaves the band once the backend's versioned report lifts it",
      resume.lifted?.showsStop === false && resume.lifted?.offersResume === false,
      JSON.stringify(resume),
    );

    const refusedResume = await evaluateCase(page, async () => {
      const hands = window.__COMPUTER_HANDS__;
      hands.announce("computer:activity", hands.operatorReport({ revision: 40, stopped: "window" }));
      await hands.settle();
      const resumesBefore = hands.sentCount("computer_resume");
      hands.refuseCommand("computer_resume");
      hands.pressBand("resume");
      await hands.settle();
      const afterRefusal = hands.readBand();
      hands.pressBand("resume");
      await hands.settle();
      hands.releaseCommands();
      return { afterRefusal, resumesSent: hands.sentCount("computer_resume") - resumesBefore };
    });
    ok(
      "a resume the backend refused leaves the person's stop on the band and offers the resume again",
      refusedResume.afterRefusal?.showsStop === true && refusedResume.afterRefusal?.offersResume === true &&
        refusedResume.resumesSent === 2,
      JSON.stringify(refusedResume),
    );

    const lateLift = await evaluateCase(page, async () => {
      const hands = window.__COMPUTER_HANDS__;
      hands.announce("computer:activity", hands.operatorReport({ revision: 50, stopped: "window" }));
      await hands.settle();
      const resumes = hands.holdCommand("computer_resume");
      hands.pressBand("resume");
      await hands.settle();
      hands.announce("computer:activity", hands.operatorReport({ revision: 52, stopped: "hotkey" }));
      hands.announce("computer:activity", hands.operatorReport({ revision: 51 }));
      resumes.answerOldest({ resumed: true, helper: "idle" });
      await hands.settle();
      const afterLateLift = hands.readBand();
      while (resumes.outstanding() > 0) resumes.answerOldest({ resumed: true, helper: "idle" });
      await hands.settle();
      hands.releaseCommands();
      return { resumeArgs: resumes.calls[0] ?? null, afterLateLift };
    });
    ok(
      "a lift report and a resume answer that arrive after the person's next stop cannot take that stop off the band",
      lateLift.afterLateLift?.showsStop === true && lateLift.afterLateLift?.offersResume === true,
      JSON.stringify(lateLift),
    );

    const failedResume = await evaluateCase(page, async () => {
      const hands = window.__COMPUTER_HANDS__;
      hands.announce("computer:activity", hands.operatorReport({ revision: 60, stopped: "window" }));
      await hands.settle();
      const resumes = hands.recordCommand("computer_resume", () => ({
        resumed: false, revision: 60, stopped: "window", helper: { error: "helper_unreachable" },
      }));
      hands.pressBand("resume");
      await hands.settle();
      const afterFailure = hands.readBand();
      hands.pressBand("resume");
      await hands.settle();
      hands.releaseCommands();
      return { resumesSent: resumes.length, afterFailure };
    });
    ok(
      "a resume the backend answered as not resumed leaves the person's stop on the band, says so, and offers the resume again",
      failedResume.afterFailure?.showsStop === true && failedResume.afterFailure?.offersResume === true &&
        Boolean(failedResume.afterFailure?.note) && failedResume.resumesSent === 2,
      JSON.stringify(failedResume),
    );

    const stopWhileResuming = await evaluateCase(page, async () => {
      const hands = window.__COMPUTER_HANDS__;
      hands.announce("computer:activity", hands.operatorReport({ revision: 70, stopped: "window" }));
      await hands.settle();
      const resumes = hands.holdCommand("computer_resume");
      const stops = hands.holdCommand("computer_stop");
      hands.pressBand("resume");
      await hands.settle();
      const whileResumeWaits = hands.readBand();
      hands.pressBand("stop");
      await hands.settle();
      const stopsSentAtOnce = stops.calls.length;
      resumes.answerOldest({ resumed: true, revision: 71, stopped: null, helper: "idle" });
      await hands.settle();
      const afterLateResume = hands.readBand();
      stops.answerOldest({ stopped: true, reason: "window", revision: 72, helperSignalled: false });
      await hands.settle();
      const afterStop = hands.readBand();
      while (resumes.outstanding() > 0) resumes.answerOldest({ resumed: false, revision: 72, stopped: "window" });
      while (stops.outstanding() > 0) stops.answerOldest({ stopped: true, reason: "window", revision: 73 });
      await hands.settle();
      hands.releaseCommands();
      return { whileResumeWaits, stopsSentAtOnce, afterLateResume, afterStop };
    });
    ok(
      "while a resume waits the band keeps the person's stop and offers the stop, and pressing it sends one stop at once",
      stopWhileResuming.whileResumeWaits?.showsStop === true && stopWhileResuming.whileResumeWaits?.offersStop === true &&
        stopWhileResuming.stopsSentAtOnce === 1,
      JSON.stringify(stopWhileResuming),
    );
    ok(
      "a resume answer that arrives after the person pressed stop again cannot take that stop off the band",
      stopWhileResuming.afterLateResume?.showsStop === true && stopWhileResuming.afterStop?.showsStop === true &&
        stopWhileResuming.afterStop?.offersResume === true,
      JSON.stringify(stopWhileResuming),
    );
    ok("the band scenario raised no renderer errors", faults.length === 0, faults.join("\n"));
  } finally { await page.close(); }
}

async function testTheProgressPanel(browser, origin, ok) {
  const { page, faults } = await openComputerPage(browser, origin);
  const nextRunSubject = { ...SUBJECT, scope: NEXT_SCOPE };
  try {
    const opening = await evaluateCase(page, async ({ subject }) => {
      const hands = window.__COMPUTER_HANDS__;
      const frames = hands.holdCommand("computer_progress_frame");
      const opened = hands.openPanel(subject);
      await hands.settle();
      const onOpen = {
        panel: hands.readPanel(),
        fetches: frames.calls.length,
        asked: frames.calls[0] ?? null,
        fromComputerFiles: frames.fromComputerFiles[0] ?? null,
      };
      frames.answerOldest(null);
      await hands.settle();
      const withNothingYet = { panel: hands.readPanel(), fetches: frames.calls.length };
      hands.announce("computer:frame", { ...subject, seq: 20 });
      await hands.settle();
      const announcedAsk = frames.calls[1] ?? null;
      frames.answerOldest(hands.look(subject, 20));
      await hands.settle();
      return {
        opened,
        onOpen,
        withNothingYet,
        announcedAsk,
        fetchesInAll: frames.calls.length,
        painted: hands.readPanel(),
        outstanding: frames.outstanding(),
      };
    }, { subject: SUBJECT });
    ok(
      "the progress panel opens for one subject and asks once for that subject's newest frame, from the Computer Use files",
      opening.opened === true && opening.onOpen?.panel.shown === true && opening.onOpen?.fetches === 1 &&
        asksFor(opening.onOpen?.asked, SUBJECT) && opening.onOpen?.fromComputerFiles === true,
      JSON.stringify(opening),
    );
    ok(
      "with no frame yet the panel shows no picture and asks nothing more until a frame is announced",
      opening.withNothingYet?.panel.shown === true && !opening.withNothingYet?.panel.picture &&
        opening.withNothingYet?.fetches === 1,
      JSON.stringify(opening),
    );
    ok(
      "an announced newer frame is fetched once, for its subject and number, and painted with that number",
      opening.fetchesInAll === 2 && asksFor(opening.announcedAsk, SUBJECT) && opening.announcedAsk?.seq === 20 &&
        opening.painted?.seq === "20" && Boolean(opening.painted?.picture) && opening.outstanding === 0,
      JSON.stringify(opening),
    );

    const foreign = await evaluateCase(page, async ({ subject, otherTerm, otherScope }) => {
      const hands = window.__COMPUTER_HANDS__;
      const frames = hands.holdCommand("computer_progress_frame");
      hands.announce("computer:frame", { ...subject, term: otherTerm, seq: 35 });
      hands.announce("computer:frame", { ...subject, scope: otherScope, seq: 36 });
      hands.announce("computer:frame", { seq: 37 });
      hands.announce("computer:frame", { ...subject, seq: 19 });
      hands.announce("computer:frame", { ...subject, seq: 20 });
      await hands.settle();
      const fetchesForForeignOrOld = frames.calls.length;
      const afterForeign = hands.readPanel();
      hands.announce("computer:frame", { ...subject, seq: 21 });
      await hands.settle();
      const ownAsk = frames.calls.at(-1) ?? null;
      frames.answerOldest(hands.look(subject, 21));
      await hands.settle();
      return {
        fetchesForForeignOrOld,
        afterForeign,
        ownAsk,
        fetchesInAll: frames.calls.length,
        painted: hands.readPanel(),
        outstanding: frames.outstanding(),
      };
    }, { subject: SUBJECT, otherTerm: OTHER_TERM, otherScope: OTHER_SCOPE });
    ok(
      "a frame of another pane, of another assignment in the same pane, of no pane, or not newer than the frame shown is never fetched",
      foreign.fetchesForForeignOrOld === 0 && foreign.afterForeign?.seq === "20",
      JSON.stringify(foreign),
    );
    ok(
      "a foreign frame numbered above the next own frame does not keep the own frame from being fetched and painted",
      foreign.fetchesInAll === 1 && asksFor(foreign.ownAsk, SUBJECT) && foreign.ownAsk?.seq === 21 &&
        foreign.painted?.seq === "21" && foreign.outstanding === 0,
      JSON.stringify(foreign),
    );

    const newestOfBurst = 22 + BURST_FRAMES;
    const burst = await evaluateCase(page, async ({ subject, burstFrames }) => {
      const hands = window.__COMPUTER_HANDS__;
      const frames = hands.holdCommand("computer_progress_frame");
      hands.announce("computer:frame", { ...subject, seq: 22 });
      await hands.settle();
      const fetchesBeforeBurst = frames.calls.length;
      for (let seq = 23; seq <= 22 + burstFrames; seq += 1) {
        hands.announce("computer:frame", { ...subject, seq });
      }
      await hands.settle();
      const fetchesDuringBurst = frames.calls.length;
      frames.answerOldest(hands.look(subject, 22));
      await hands.settle();
      const fetchesAfterFirstAnswer = frames.calls.length;
      const nextAsk = frames.calls[1] ?? null;
      frames.answerOldest(hands.look(subject, 22 + burstFrames));
      await hands.settle();
      return {
        fetchesBeforeBurst,
        fetchesDuringBurst,
        fetchesAfterFirstAnswer,
        nextAsk,
        fetchesInAll: frames.calls.length,
        mostInFlight: frames.mostInFlight,
        outstanding: frames.outstanding(),
        painted: hands.readPanel(),
        census: hands.census.live(),
      };
    }, { subject: SUBJECT, burstFrames: BURST_FRAMES });
    ok(
      "while one frame fetch is out, a burst of announcements asks for nothing more, and the next fetch asks only for the newest",
      burst.fetchesBeforeBurst === 1 && burst.fetchesDuringBurst === 1 && burst.fetchesAfterFirstAnswer === 2 &&
        burst.nextAsk?.seq === newestOfBurst && burst.fetchesInAll === 2,
      JSON.stringify(burst),
    );
    ok(
      "at most one frame fetch is out at a time, and the panel holds at most one picture of its own",
      burst.mostInFlight === 1 && burst.outstanding === 0 && typeof burst.census?.objectUrls === "number" &&
        burst.census.objectUrls <= 1,
      JSON.stringify(burst),
    );
    ok(
      "the newest frame of the burst is the one on screen",
      burst.painted?.seq === String(newestOfBurst),
      JSON.stringify(burst),
    );

    const olderAnswer = await evaluateCase(page, async ({ subject, shown }) => {
      const hands = window.__COMPUTER_HANDS__;
      const frames = hands.holdCommand("computer_progress_frame");
      hands.announce("computer:frame", { ...subject, seq: shown + 1 });
      await hands.settle();
      frames.answerOldest(hands.look(subject, shown - 1));
      await hands.settle();
      await hands.settle();
      return { fetchesInAll: frames.calls.length, outstanding: frames.outstanding(), painted: hands.readPanel() };
    }, { subject: SUBJECT, shown: newestOfBurst });
    ok(
      "an answer older than the frame on screen does not replace it and is not asked for again",
      olderAnswer.fetchesInAll === 1 && olderAnswer.outstanding === 0 &&
        olderAnswer.painted?.seq === String(newestOfBurst),
      JSON.stringify(olderAnswer),
    );

    const nextRun = await evaluateCase(page, async ({ subject, nextRunSubject, oldRevision }) => {
      const hands = window.__COMPUTER_HANDS__;
      const frames = hands.holdCommand("computer_progress_frame");
      hands.announce("computer:frame", { ...subject, seq: oldRevision });
      await hands.settle();
      const reopened = hands.openPanel(nextRunSubject);
      await hands.settle();
      const afterReplacement = hands.readPanel();
      hands.announce("computer:frame", { ...nextRunSubject, seq: 3 });
      hands.announce("computer:frame", { ...subject, seq: oldRevision + 1 });
      await hands.settle();
      const fetchesWhileOldIsOut = frames.calls.length;
      frames.answerOldest(hands.look(subject, oldRevision));
      await hands.settle();
      const afterOldAnswer = hands.readPanel();
      for (let round = 0; round < 3 && frames.outstanding() > 0; round += 1) {
        frames.answerOldest(hands.look(nextRunSubject, 3));
        await hands.settle();
      }
      return {
        reopened,
        afterReplacement,
        fetchesWhileOldIsOut,
        afterOldAnswer,
        laterAsks: frames.calls.slice(1),
        mostInFlight: frames.mostInFlight,
        outstanding: frames.outstanding(),
        painted: hands.readPanel(),
      };
    }, { subject: SUBJECT, nextRunSubject, oldRevision: newestOfBurst + 2 });
    ok(
      "opening the panel for the pane's next assignment takes the old assignment's frame off the panel at once",
      nextRun.reopened === true && nextRun.afterReplacement?.shown === true &&
        nextRun.afterReplacement?.seq !== String(newestOfBurst) && !nextRun.afterReplacement?.picture,
      JSON.stringify(nextRun),
    );
    ok(
      "the old assignment's late answer paints nothing, and only the new assignment's frame is asked for after it, one fetch at a time",
      nextRun.fetchesWhileOldIsOut === 1 && nextRun.afterOldAnswer?.seq !== String(newestOfBurst + 2) &&
        nextRun.laterAsks?.length === 1 && asksFor(nextRun.laterAsks[0], nextRunSubject) &&
        nextRun.mostInFlight === 1 && nextRun.outstanding === 0,
      JSON.stringify(nextRun),
    );
    ok(
      "the new assignment's frame is painted although its number is lower than the old assignment's",
      nextRun.painted?.seq === "3" && Boolean(nextRun.painted?.picture),
      JSON.stringify(nextRun),
    );

    const failed = await evaluateCase(page, async ({ nextRunSubject }) => {
      const hands = window.__COMPUTER_HANDS__;
      const frames = hands.holdCommand("computer_progress_frame");
      hands.announce("computer:frame", { ...nextRunSubject, seq: 4 });
      await hands.settle();
      frames.refuseOldest("no frame");
      await hands.settle();
      const afterFailure = hands.readPanel();
      await hands.settle();
      return {
        afterFailure,
        fetchesInAll: frames.calls.length,
        outstanding: frames.outstanding(),
        census: hands.census.live(),
      };
    }, { nextRunSubject });
    ok(
      "a frame that failed to arrive leaves the last good frame on screen",
      failed.afterFailure?.seq === "3" && Boolean(failed.afterFailure?.picture),
      JSON.stringify(failed),
    );
    ok(
      "after a failed fetch the open panel holds no timer, animation frame or fetch of the Computer Use files, so nothing is asked again until a frame is announced",
      failed.fetchesInAll === 1 && failed.outstanding === 0 && failed.census?.timeouts === 0 &&
        failed.census?.intervals === 0 && failed.census?.animationFrames === 0,
      JSON.stringify(failed),
    );

    const closing = await evaluateCase(page, async ({ nextRunSubject }) => {
      const hands = window.__COMPUTER_HANDS__;
      const frames = hands.holdCommand("computer_progress_frame");
      hands.announce("computer:frame", { ...nextRunSubject, seq: 5 });
      await hands.settle();
      const fetchesBeforeClose = frames.calls.length;
      const closed = hands.closePanel();
      await hands.settle();
      const afterClose = hands.readPanel();
      frames.answerOldest(hands.look(nextRunSubject, 5));
      await hands.settle();
      const afterLateAnswer = hands.readPanel();
      const censusAfterClose = hands.census.live();
      hands.announce("computer:frame", { ...nextRunSubject, seq: 6 });
      await hands.settle();
      return {
        fetchesBeforeClose,
        closed,
        afterClose,
        afterLateAnswer,
        censusAfterClose,
        fetchesWhileClosed: frames.calls.length - fetchesBeforeClose,
        outstanding: frames.outstanding(),
      };
    }, { nextRunSubject });
    ok(
      "closing the panel with a fetch out lets go of its picture, and the late answer paints nothing",
      closing.fetchesBeforeClose === 1 && closing.closed === true && closing.afterClose?.shown === false &&
        !closing.afterClose?.picture && !closing.afterLateAnswer?.picture && closing.afterLateAnswer?.shown === false &&
        censusIsEmpty(closing.censusAfterClose),
      JSON.stringify(closing),
    );
    ok(
      "a frame announced while the panel is closed is not fetched",
      closing.closed === true && closing.fetchesWhileClosed === 0 && closing.outstanding === 0,
      JSON.stringify(closing),
    );

    const reopening = await evaluateCase(page, async ({ subject, cycles }) => {
      const hands = window.__COMPUTER_HANDS__;
      hands.recordCommand("computer_progress_frame", () => null);
      const firstOpen = hands.openPanel(subject);
      hands.closePanel();
      await hands.settle();
      const listenersAfterFirstOpen = hands.frameListeners();
      for (let round = 0; round < cycles; round += 1) {
        hands.openPanel(subject);
        hands.closePanel();
      }
      await hands.settle();
      const census = hands.census.live();
      hands.releaseCommands();
      return {
        firstOpen,
        listenersAfterFirstOpen,
        listenerGrowth: hands.frameListeners() - listenersAfterFirstOpen,
        census,
      };
    }, { subject: SUBJECT, cycles: OPEN_CLOSE_CYCLES });
    ok(
      "opening and closing the panel again adds no event listener and leaves no timer, animation frame or picture behind",
      reopening.firstOpen === true && reopening.listenerGrowth === 0 && censusIsEmpty(reopening.census),
      JSON.stringify(reopening),
    );

    const pictureless = await evaluateCase(page, async ({ subject }) => {
      const hands = window.__COMPUTER_HANDS__;
      const frames = hands.holdCommand("computer_progress_frame");
      hands.openPanel(subject);
      await hands.settle();
      frames.answerOldest(hands.look(subject, 40));
      await hands.settle();
      const withPicture = hands.readPanel();
      hands.announce("computer:frame", { ...subject, seq: 41 });
      await hands.settle();
      frames.answerOldest(hands.look(subject, 41, { png: null, perception: "accessibility" }));
      await hands.settle();
      const treeOnly = hands.readPanel();
      hands.announce("computer:frame", { ...subject, seq: 42 });
      await hands.settle();
      frames.answerOldest(hands.look(subject, 42, { png: null, perception: "gone" }));
      await hands.settle();
      const gone = hands.readPanel();
      hands.closePanel();
      await hands.settle();
      hands.releaseCommands();
      return { withPicture, treeOnly, gone, census: hands.census.live() };
    }, { subject: SUBJECT });
    ok(
      "a newer look without a picture takes the older picture off the panel and says why, whether the look read the screen's structure or its picture is no longer kept",
      pictureless.withPicture?.seq === "40" && Boolean(pictureless.withPicture?.picture) &&
        pictureless.treeOnly?.seq === "41" && !pictureless.treeOnly?.picture && Boolean(pictureless.treeOnly?.note) &&
        pictureless.gone?.seq === "42" && !pictureless.gone?.picture && Boolean(pictureless.gone?.note) &&
        pictureless.treeOnly?.note !== pictureless.gone?.note && censusIsEmpty(pictureless.census),
      JSON.stringify(pictureless),
    );
    ok("the progress panel scenario raised no renderer errors", faults.length === 0, faults.join("\n"));
  } finally { await page.close(); }
}

async function testTheBandWithThePanel(browser, origin, ok) {
  const { page, faults } = await openComputerPage(browser, origin);
  try {
    const viewing = await evaluateCase(page, async ({ subject }) => {
      const hands = window.__COMPUTER_HANDS__;
      hands.announce("computer:activity", hands.operatorReport({ revision: 1, active: true }));
      await hands.settle();
      const beforeAnyLook = hands.readBand();
      hands.announce("computer:frame", { ...subject, seq: 4 });
      await hands.settle();
      const afterLook = hands.readBand();
      const fetchesBeforePress = hands.sentCount("computer_progress_frame");
      const frames = hands.holdCommand("computer_progress_frame");
      hands.pressBand("view");
      await hands.settle();
      const asked = frames.calls[0] ?? null;
      frames.answerOldest(hands.look(subject, 4));
      await hands.settle();
      return { beforeAnyLook, afterLook, fetchesBeforePress, fetches: frames.calls.length, asked, panel: hands.readPanel() };
    }, { subject: SUBJECT });
    ok(
      "the band offers its screen view only after a look was announced, and the view opens the panel for that pane and assignment and asks once",
      viewing.beforeAnyLook?.offersView === false && viewing.afterLook?.offersView === true &&
        viewing.fetchesBeforePress === 0 && viewing.fetches === 1 && asksFor(viewing.asked, SUBJECT) &&
        viewing.panel?.shown === true && viewing.panel?.seq === "4" && Boolean(viewing.panel?.picture),
      JSON.stringify(viewing),
    );

    await page.setViewportSize({ width: 360, height: 640 });
    const small = await evaluateCase(page, async () => {
      const hands = window.__COMPUTER_HANDS__;
      hands.announce("computer:activity", hands.operatorReport({ revision: 2, stopped: "window" }));
      await hands.settle();
      const inside = (node) => {
        const box = node.getBoundingClientRect();
        return box.width > 0 && box.height > 0 && box.left >= 0 && box.top >= 0 &&
          box.right <= window.innerWidth + 0.5 && box.bottom <= window.innerHeight + 0.5;
      };
      const band = document.getElementById("computer-band");
      const panel = document.getElementById("computer-progress");
      const buttons = [...band.querySelectorAll("button"), ...panel.querySelectorAll("button")]
        .filter((button) => !button.hidden && button.getClientRects().length > 0);
      return {
        bandInside: inside(band),
        buttons: buttons.length,
        buttonsInside: buttons.every(inside),
        panelShown: !panel.hidden,
        panelInside: inside(panel),
        panelHeight: panel.getBoundingClientRect().height,
        screen: [window.innerWidth, window.innerHeight],
      };
    });
    ok(
      "on a 360 by 640 screen the band, its buttons and the open panel stay on screen, and the panel takes at most half the height",
      small.bandInside === true && small.buttons >= 3 && small.buttonsInside === true && small.panelShown === true &&
        small.panelInside === true && small.panelHeight <= small.screen?.[1] / 2,
      JSON.stringify(small),
    );

    const words = await evaluateCase(page, async () => {
      const hands = window.__COMPUTER_HANDS__;
      const resumes = hands.holdCommand("computer_resume");
      hands.pressBand("resume");
      await hands.settle();
      const title = () => document.getElementById("computer-progress-title")?.textContent ?? "";
      const before = { note: hands.readBand().note, title: title() };
      const savedLocale = locale;
      locale = "en";
      applyLocale();
      await hands.settle();
      const english = { note: hands.readBand().note, title: title() };
      locale = savedLocale;
      applyLocale();
      await hands.settle();
      while (resumes.outstanding() > 0) resumes.answerOldest({ resumed: false, revision: 2, stopped: "window" });
      await hands.settle();
      hands.closePanel();
      await hands.settle();
      hands.releaseCommands();
      return { before, english, census: hands.census.live() };
    });
    const hangul = /[\u3131-\uD79D]/u;
    ok(
      "a language change repaints the band's waiting words and the panel's title",
      Boolean(words.before?.note) && Boolean(words.english?.note) && words.before?.note !== words.english?.note &&
        !hangul.test(words.english?.note ?? "") && Boolean(words.before?.title) &&
        words.before?.title !== words.english?.title && !hangul.test(words.english?.title ?? "") &&
        censusIsEmpty(words.census),
      JSON.stringify(words),
    );
    ok("the band and panel scenario raised no renderer errors", faults.length === 0, faults.join("\n"));
  } finally { await page.close(); }
}

/* Who holds control, read in the panel (t-62464): the desktop as the backend
 * granted its stop, and each work browser the panel's pane owns as the
 * dedicated browser's own state says it — in its revision order — with a way
 * to that browser's own bar, where its control changes. The panel sends no
 * control of its own. */
async function testThePanelReadsTheWorkBrowser(browser, origin, ok) {
  const { page, faults } = await openComputerPage(browser, origin);
  try {
    await page.evaluate(installWorkBrowsers);
    const desk = await evaluateCase(page, async ({ subject }) => {
      const hands = window.__COMPUTER_HANDS__;
      hands.recordCommand("computer_progress_frame", () => null);
      hands.announce("computer:activity", hands.operatorReport({ revision: 30, active: true }));
      hands.openPanel(subject);
      await hands.settle();
      const agents = hands.readControl();
      const stops = hands.holdCommand("computer_stop");
      hands.pressBand("stop");
      await hands.settle();
      const whileStopWaits = hands.readControl();
      hands.announce("computer:activity", hands.operatorReport({ revision: 31, stopped: "window" }));
      stops.answerOldest({ stopped: true, reason: "window", revision: 31, helperSignalled: false });
      await hands.settle();
      const granted = hands.readControl();
      hands.announce("computer:activity", hands.operatorReport({ revision: 30, active: true }));
      await hands.settle();
      const afterOlderReport = hands.readControl();
      hands.recordCommand("computer_resume", () => ({ resumed: true, revision: 32, stopped: null, helper: "idle" }));
      hands.pressBand("resume");
      await hands.settle();
      const resumed = hands.readControl();
      return { agents, whileStopWaits, granted, afterOlderReport, resumed };
    }, { subject: SUBJECT });
    ok(
      "the open panel says in one line that the desktop is the agent's while no stop is granted, and a stop still waiting for the backend does not change it",
      Boolean(desk.agents?.desk) && desk.whileStopWaits?.desk === desk.agents?.desk,
      JSON.stringify(desk),
    );
    ok(
      "the desktop line says the stop once the backend grants it, with its reason, and an older report cannot take it back",
      Boolean(desk.granted?.desk) && desk.granted?.desk !== desk.agents?.desk && desk.granted?.desk.includes("window") &&
        desk.afterOlderReport?.desk === desk.granted?.desk,
      JSON.stringify(desk),
    );
    ok(
      "a granted resume gives the desktop line back to the agent",
      Boolean(desk.resumed?.desk) && desk.resumed?.desk === desk.agents?.desk,
      JSON.stringify(desk),
    );

    const rows = await evaluateCase(page, async ({ subject, otherTerm }) => {
      const hands = window.__COMPUTER_HANDS__;
      const work = window.__WORK_BROWSERS__;
      const owner = String(subject.term);
      const other = String(otherTerm);
      work.open("browser-901");
      work.open("browser-902");
      for (let round = 0; round < 50 && !(work.tab("browser-901") && work.tab("browser-902")); round += 1) {
        await hands.settle();
      }
      work.say({ label: "browser-901", mode: "agent", revision: 3, ownerTerm: owner });
      work.say({ label: "browser-902", mode: "agent", revision: 2, ownerTerm: other });
      await hands.settle();
      const owned = hands.readControl();
      const agentWords = work.words("browser-901");
      work.say({ label: "browser-901", mode: "human", revision: 5, ownerTerm: owner });
      await hands.settle();
      const human = hands.readControl();
      const humanWords = work.words("browser-901");
      work.say({ label: "browser-901", mode: "agent", revision: 4, ownerTerm: owner });
      await hands.settle();
      const afterOlder = hands.readControl();
      work.say({ label: "browser-901", mode: "paused", revision: 6, ownerTerm: other });
      await hands.settle();
      const handedAway = hands.readControl();
      work.say({ label: "browser-901", mode: "agent", revision: 7, ownerTerm: owner });
      await hands.settle();
      const handedBack = hands.readControl();
      return {
        owned, agentWords, human, humanWords, afterOlder, handedAway, handedBack,
        tab: work.tab("browser-901")?.id ?? null,
        otherTab: work.tab("browser-902")?.id ?? null,
        bar: { human: BROWSER_WORKSPACE_ACTION_WORDS.human(), agent: BROWSER_WORKSPACE_ACTION_WORDS.agent() },
      };
    }, { subject: SUBJECT, otherTerm: OTHER_TERM });
    ok(
      "a work browser the panel's pane owns stands in the panel in the dedicated browser's own words for its state, and one another pane owns does not",
      Boolean(rows.tab) && rows.owned?.rows?.length === 1 && rows.owned.rows[0].tab === rows.tab &&
        Boolean(rows.agentWords) && rows.owned.rows[0].words?.includes(rows.agentWords) &&
        !rows.owned.rows.some((row) => row.tab === rows.otherTab),
      JSON.stringify(rows),
    );
    ok(
      "the work browser's row follows the dedicated browser's newer revision, and an older revision cannot change it back",
      rows.human?.rows?.length === 1 && Boolean(rows.humanWords) && rows.humanWords !== rows.agentWords &&
        rows.human.rows[0].words?.includes(rows.humanWords) && rows.afterOlder?.rows?.[0]?.words === rows.human.rows[0].words,
      JSON.stringify(rows),
    );
    ok(
      "a work browser handed to another pane leaves the panel, and stands in it again once it is handed back",
      rows.handedAway?.rows?.length === 0 && rows.handedBack?.rows?.length === 1 && rows.handedBack.rows[0].tab === rows.tab,
      JSON.stringify(rows),
    );
    ok(
      "beside a work browser's row the panel says that its control changes in that browser's own bar, in the bar's own words, and says nothing of it with no row",
      Boolean(rows.owned?.hint) && rows.owned.hint.includes(rows.bar?.human) && rows.owned.hint.includes(rows.bar?.agent) &&
        rows.handedAway?.hint === null && rows.handedBack?.rows?.[0]?.offersGo === true,
      JSON.stringify(rows),
    );

    const idle = await evaluateCase(page, async () => {
      const hands = window.__COMPUTER_HANDS__;
      const before = hands.readControl();
      const savedLocale = locale;
      locale = "en";
      applyLocale();
      await hands.settle();
      const english = hands.readControl();
      locale = savedLocale;
      applyLocale();
      await hands.settle();
      const watched = ["computer_progress_frame", "get_browser_workspaces", "set_browser_workspace_mode", "browser_snapshot"];
      const sentBefore = watched.map((name) => hands.sentCount(name));
      await new Promise((done) => hands.census.native.setTimeout(done, 1500));
      const sent = watched.filter((name, at) => hands.sentCount(name) !== sentBefore[at]);
      return { before, english, census: hands.census.live(), sent, after: hands.readControl() };
    });
    const hangul = /[ㄱ-힝]/u;
    ok(
      "a language change repaints the panel's desktop line, the work browser's row and the hint beside it",
      Boolean(idle.before?.desk) && Boolean(idle.english?.desk) && idle.english.desk !== idle.before.desk &&
        !hangul.test(idle.english.desk) && idle.english?.rows?.length === 1 &&
        idle.english.rows[0].words !== idle.before?.rows?.[0]?.words && !hangul.test(idle.english.rows[0].words ?? "") &&
        Boolean(idle.english?.hint) && !hangul.test(idle.english.hint) && idle.after?.desk === idle.before.desk,
      JSON.stringify(idle),
    );
    ok(
      "with the panel open beside a work browser's row, an idle second and a half asks nothing and leaves no timer, interval or animation frame of the Computer Use files",
      idle.sent?.length === 0 && idle.census?.timeouts === 0 && idle.census?.intervals === 0 &&
        idle.census?.animationFrames === 0,
      JSON.stringify(idle),
    );

    const going = await evaluateCase(page, async () => {
      const hands = window.__COMPUTER_HANDS__;
      const work = window.__WORK_BROWSERS__;
      const tab = work.tab("browser-901");
      const row = tab ? document.querySelector(`#computer-progress-browsers [data-tab="${CSS.escape(tab.id)}"]`) : null;
      const button = row?.querySelector("button") ?? null;
      const offered = !!button && !button.hidden && !button.disabled;
      const control = ["set_browser_workspace_mode", "computer_stop", "computer_resume"];
      const sentBefore = control.map((name) => hands.sentCount(name));
      button?.click();
      await hands.settle();
      const sent = control.map((name, at) => hands.sentCount(name) - sentBefore[at]);
      const viewing = { active: activeTabId, viewing: tab?.workspaceViewing === true };
      closeTab(tab.id);
      await hands.settle();
      const afterClose = hands.readControl();
      return { tab: tab?.id ?? null, offered, viewing, sent, modes: work.modes.length, afterClose };
    });
    ok(
      "the row's button opens that work browser's own bar beside the conversation and moves no control: no mode change, stop or resume is sent",
      going.offered === true && going.viewing?.active === going.tab && going.viewing?.viewing === true &&
        going.sent?.every((count) => count === 0) && going.modes === 0,
      JSON.stringify(going),
    );
    ok(
      "a closed work browser leaves the panel at once, and the hint beside it goes too",
      going.afterClose?.rows?.length === 0 && going.afterClose?.hint === null && Boolean(going.afterClose?.desk),
      JSON.stringify(going),
    );
    ok("the work browser scenario raised no renderer errors", faults.length === 0, faults.join("\n"));
  } finally { await page.close(); }
}

export async function testComputerProgress(browser, origin, ok) {
  await testTheBand(browser, origin, ok);
  await testTheProgressPanel(browser, origin, ok);
  await testTheBandWithThePanel(browser, origin, ok);
  await testThePanelReadsTheWorkBrowser(browser, origin, ok);
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
