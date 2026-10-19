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
/* How many times the person's stop and resume are pressed over two agents'
 * work browsers to time how long the stop takes to reach them (t-62872). */
const STOP_TRIALS = 20;

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
      const browsers = document.getElementById("computer-band-browsers");
      return {
        shown: !band.hidden,
        showsStop: band.classList.contains("is-stopped"),
        offersStop: !document.getElementById("computer-band-stop").hidden,
        offersResume: !document.getElementById("computer-band-resume").hidden,
        offersView: view !== null && !view.hidden,
        note: note && !note.hidden ? note.textContent : null,
        browsers: browsers && !browsers.hidden ? browsers.textContent : null,
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
      const stopped = document.getElementById("computer-progress-stop-browsers");
      const rows = list && !list.hidden ? [...list.querySelectorAll("[data-tab]")] : [];
      return {
        desk: desk && !desk.hidden ? desk.textContent : null,
        stopBrowsers: stopped && !stopped.hidden ? stopped.textContent : null,
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
    /* A work browser's own bar as the person sees it, while it stands in front
     * of its pane: its mode, whether it offers to hand the browser to an agent,
     * and what it says of the person's stop (t-62872 r3). */
    readWorkBar: (label) => {
      const tab = tabs.find((one) => one.kind === "browser" && one.label === label);
      const host = tab ? groupOf(tab.pane)?.browserView : null;
      if (!host || host._browserTab !== tab) return null;
      const agent = host.querySelector(".browser-workspace-agent");
      const note = host.querySelector(".browser-workspace-stop-note");
      return {
        mode: tab.workspace?.mode ?? null,
        offersAgent: !!agent && !agent.hidden && !agent.disabled,
        says: note && !note.hidden ? note.textContent : null,
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

/* The dedicated work browser's control as the backend keeps it (t-62460,
 * `browser_workspace.rs`), small enough to race the person's stop against
 * (t-62872): a mode change names the revision it saw and a moved one is
 * refused as stale; it moves the revision into `pausing`, waits for the
 * agent's input in flight, and moves it again into the mode asked. An agent's
 * input is refused at the door while the person's stop stands — the backend
 * reads its own stop there, granted by the stop's answer and lifted by the
 * resume's — and is otherwise admitted only in the agent's hands, one at a
 * time. Every move is told to the page the way the backend tells it
 * (`browser:workspace`). What the door refuses is this model's own doing, so
 * the suite claims nothing of it (t-62872 r3): the window's walk and words are
 * what it proves. That no agent input lands once the person's stop stands is
 * the backend's door, and the Rust race test proves it
 * (`browser_workspace::tests::no_agent_input_gets_in_between_the_persons_stop_and_the_windows_pause`). */
const installWorkBrowserModel = () => {
  const model = {
    states: new Map(), inputs: [], asked: [], pauses: [], drains: new Map(), moving: new Map(), holds: new Map(),
    selfPausing: new Set(), personsStop: false,
  };
  window.__WORK_MODEL__ = model;
  window.__ANSWER__.open_browser_pane = (args) => args.label;
  window.__ANSWER__.browser_place = () => null;
  window.__ANSWER__.browser_zoom = () => null;
  window.__ANSWER__.close_browser_pane = ({ label }) => {
    model.states.delete(label);
    return null;
  };
  window.__ANSWER__.set_browser_open_tabs = ({ tabs: records }) => ({ browser: { ...browserPrefs, open_tabs: records } });
  const copy = (state) => ({
    label: state.label, mode: state.mode, revision: state.revision, inFlight: state.inFlight,
    needsObservation: state.needsObservation, ownerTerm: state.ownerTerm, snapshotSupported: false,
  });
  const tell = (state) => {
    for (const hear of window.__LISTENERS__["browser:workspace"] ?? []) hear({ payload: copy(state) });
  };
  window.__ANSWER__.get_browser_workspaces = () => [...model.states.values()].map(copy);
  model.open = async (labels) => {
    for (const label of labels) {
      for (const hear of window.__LISTENERS__["browser:agent-open"] ?? []) {
        hear({ payload: { label, url: `https://example.com/${label}` } });
      }
    }
    for (let round = 0; round < 50 && !labels.every((label) => model.tab(label)); round += 1) {
      await window.__COMPUTER_HANDS__.settle();
    }
  };
  model.tab = (label) => tabs.find((one) => one.kind === "browser" && one.label === label) ?? null;
  model.mode = (label) => model.tab(label)?.workspace?.mode ?? null;
  model.say = (fields) => {
    const state = { inFlight: 0, needsObservation: false, ...fields };
    model.states.set(state.label, state);
    tell(state);
  };
  /* The person's stop as the backend holds it: granted when the stop's answer
   * leaves, lifted when the resume's does. */
  model.stop = () => {
    model.personsStop = true;
  };
  model.lift = () => {
    model.personsStop = false;
  };
  /* The agent's input, admitted as the backend's door admits it — refused
   * while the person's stop stands, then as `begin_at` refuses: answers the
   * input's end, or null when it was refused. */
  model.input = (label) => {
    const state = model.states.get(label);
    if (model.personsStop || !state || state.mode !== "agent" || state.inFlight !== 0) {
      return null;
    }
    state.inFlight = 1;
    model.inputs.push({ label, at: performance.now(), revision: state.revision });
    return () => {
      state.inFlight = 0;
      const drained = model.drains.get(label);
      model.drains.delete(label);
      drained?.();
    };
  };
  /* An agent that keeps sending input as fast as the page's clock turns, each
   * input ending at once; answers how to stop it. */
  model.hammer = (labels) => {
    let running = true;
    const beat = () => {
      if (!running) return;
      for (const label of labels) model.input(label)?.();
      window.__COMPUTER_HANDS__.census.native.setTimeout(beat, 1);
    };
    beat();
    return () => { running = false; };
  };
  /* An agent whose own navigation moves its browser's revision just before
   * each of the next `times` mode changes reaches it (a popup's navigation,
   * `with_agent_popup`), the page not yet told. */
  model.moveBefore = (label, times) => model.moving.set(label, times);
  /* The backend pausing a browser on its own — an unfinished page change or
   * actor's work, a failed popup navigation — just before the next mode change
   * asked of it reaches it, the page not yet told. */
  model.pauseBefore = (label) => model.selfPausing.add(label);
  /* The next mode change asked of `label` waits until the answer is let go. */
  model.hold = (label) => {
    let release = () => {};
    const held = new Promise((done) => { release = done; });
    model.holds.set(label, held);
    return release;
  };
  window.__ANSWER__.set_browser_workspace_mode = async ({ label, mode, expectedRevision }) => {
    model.asked.push({ label, mode, expectedRevision, at: performance.now() });
    const held = model.holds.get(label);
    model.holds.delete(label);
    if (held) await held;
    const state = model.states.get(label);
    if (!state) throw new Error("workspace_missing: no such work browser");
    const moving = model.moving.get(label) ?? 0;
    if (moving > 0) {
      model.moving.set(label, moving - 1);
      state.revision += 1;
    }
    if (model.selfPausing.delete(label)) {
      state.revision += 1;
      state.mode = "paused";
      state.needsObservation = true;
    }
    if (state.revision !== expectedRevision) throw new Error("workspace_stale: the work browser's state moved");
    state.revision += 1;
    state.mode = "pausing";
    model.pauses.push({ label, mode, at: performance.now() });
    tell(state);
    if (state.inFlight !== 0) await new Promise((drained) => model.drains.set(label, drained));
    state.revision += 1;
    state.mode = mode;
    state.needsObservation = true;
    tell(state);
    return copy(state);
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
      hands.announce("computer:frame", { ...subject, seq: 43 });
      await hands.settle();
      frames.answerOldest(hands.look(subject, 43, { png: null, perception: "oversize" }));
      await hands.settle();
      const oversize = hands.readPanel();
      hands.closePanel();
      await hands.settle();
      hands.releaseCommands();
      return { withPicture, treeOnly, gone, oversize, census: hands.census.live() };
    }, { subject: SUBJECT });
    ok(
      "a newer look without a picture takes the older picture off the panel and says why, whether the look read the screen's structure or its picture is no longer kept",
      pictureless.withPicture?.seq === "40" && Boolean(pictureless.withPicture?.picture) &&
        pictureless.treeOnly?.seq === "41" && !pictureless.treeOnly?.picture && Boolean(pictureless.treeOnly?.note) &&
        pictureless.gone?.seq === "42" && !pictureless.gone?.picture && Boolean(pictureless.gone?.note) &&
        pictureless.treeOnly?.note !== pictureless.gone?.note && censusIsEmpty(pictureless.census),
      JSON.stringify(pictureless),
    );
    ok(
      "a look whose picture was too large to keep shows no picture and says so in its own words (t-63505)",
      pictureless.oversize?.seq === "43" && !pictureless.oversize?.picture && Boolean(pictureless.oversize?.note) &&
        pictureless.oversize.note !== pictureless.gone?.note && pictureless.oversize.note !== pictureless.treeOnly?.note,
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

    const focusing = await evaluateCase(page, async () => {
      const hands = window.__COMPUTER_HANDS__;
      hands.closePanel();
      await hands.settle();
      hands.recordCommand("computer_progress_frame", () => null);
      const view = document.getElementById("computer-band-view");
      view.focus();
      const before = document.activeElement === view;
      hands.pressBand("view");
      await hands.settle();
      const close = document.getElementById("computer-progress-close");
      close.focus();
      const inside = document.activeElement === close;
      close.click();
      await hands.settle();
      const seen = {
        before,
        inside,
        shown: hands.readPanel().shown,
        back: document.activeElement === view,
        focused: document.activeElement?.id || document.activeElement?.tagName || null,
      };
      // Open again for the cases that follow.
      hands.pressBand("view");
      await hands.settle();
      hands.releaseCommands();
      return seen;
    });
    ok(
      "closing the panel gives the keyboard back to where it was before the panel opened (t-63505)",
      focusing.before === true && focusing.inside === true && focusing.shown === false && focusing.back === true,
      JSON.stringify(focusing),
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
      hands.openPanel(subject);
      await hands.settle();
      const quiet = hands.readControl();
      hands.announce("computer:activity", hands.operatorReport({ revision: 30, active: true }));
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
      hands.announce("computer:activity", hands.operatorReport({ revision: 32, active: true }));
      await hands.settle();
      const actingAgain = hands.readControl();
      // Quiet again, so the band's own slip-away cannot change the line under
      // the cases that follow.
      hands.announce("computer:activity", hands.operatorReport({ revision: 32 }));
      await hands.settle();
      return { quiet, agents, whileStopWaits, granted, afterOlderReport, resumed, actingAgain };
    }, { subject: SUBJECT });
    ok(
      "with no Computer Use action going on the open panel does not say the agent holds the desktop, and says it once an action comes (t-63505)",
      Boolean(desk.quiet?.desk) && Boolean(desk.agents?.desk) && desk.quiet.desk !== desk.agents.desk,
      JSON.stringify(desk),
    );
    ok(
      "the open panel says in one line that the desktop is the agent's while it acts and no stop is granted, and a stop still waiting for the backend does not change it",
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
      "a granted resume takes the stop off the desktop line, which says the agent holds the desktop again only with its next action",
      Boolean(desk.resumed?.desk) && desk.resumed.desk !== desk.granted?.desk && desk.resumed.desk !== desk.agents?.desk &&
        desk.resumed.desk === desk.quiet?.desk && desk.actingAgain?.desk === desk.agents?.desk,
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

/* The person's one stop (t-62872): a stop the backend says is the person's
 * reaches every work browser an agent holds in this window — each through its
 * own bar's road, against the dedicated browser's control as the backend keeps
 * it (`installWorkBrowserModel`) — and the band's resume gives back only what
 * that stop paused. The band and the panel say how far the stop reached. */
async function testTheOneStop(browser, origin, ok) {
  const labelsOf = (asked, label) => (asked ?? []).filter((one) => one.label === label);
  {
    const { page, faults } = await openComputerPage(browser, origin);
    try {
      await page.evaluate(installWorkBrowserModel);
      const stopped = await evaluateCase(page, async ({ subject, otherTerm }) => {
        const hands = window.__COMPUTER_HANDS__;
        const model = window.__WORK_MODEL__;
        const tick = () => new Promise((done) => hands.census.native.setTimeout(done, 0));
        const all = ["browser-911", "browser-912", "browser-913", "browser-914"];
        const agents = ["browser-911", "browser-912"];
        await model.open(all);
        model.say({ label: "browser-911", mode: "agent", revision: 3, ownerTerm: String(subject.term) });
        model.say({ label: "browser-912", mode: "agent", revision: 5, ownerTerm: String(otherTerm) });
        model.say({ label: "browser-913", mode: "human", revision: 4, ownerTerm: String(subject.term) });
        model.say({ label: "browser-914", mode: "paused", revision: 6, ownerTerm: String(otherTerm) });
        hands.recordCommand("computer_progress_frame", () => null);
        hands.announce("computer:activity", hands.operatorReport({ revision: 40, active: true }));
        hands.openPanel(subject);
        // The work browser the person holds stands in front of its pane: its
        // bar is the one in view when the stop comes (t-62872 r3).
        const inView = model.tab("browser-913");
        if (inView) setActiveTab(inView.id);
        await hands.settle();
        const stopHammer = model.hammer(agents);
        await new Promise((done) => hands.census.native.setTimeout(done, 30));
        const landedBefore = model.inputs.length;
        // The window's pause of each is held on its way to the backend, so the
        // agent keeps trying in the moment between the stop and the pause.
        const releases = agents.map((label) => model.hold(label));
        hands.recordCommand("computer_stop", () => {
          model.stop();
          return { stopped: true, reason: "window", revision: 41, personsStop: true, helperSignalled: false };
        });
        const pressed = performance.now();
        hands.pressBand("stop");
        const asked = (label) => model.asked.some((one) => one.label === label);
        while (performance.now() - pressed < 3000 && !agents.every(asked)) await tick();
        await new Promise((done) => hands.census.native.setTimeout(done, 30));
        for (const release of releases) release();
        let pausedAt = null;
        while (performance.now() - pressed < 3000) {
          if (agents.every((label) => model.mode(label) === "paused")) {
            pausedAt = performance.now();
            break;
          }
          await tick();
        }
        await new Promise((done) => hands.census.native.setTimeout(done, 30));
        stopHammer();
        await hands.settle();
        const reached = agents.map((label) => model.pauses.find((one) => one.label === label && one.mode === "paused"));
        return {
          asked: model.asked.map(({ label, mode, expectedRevision }) => ({ label, mode, expectedRevision })),
          modes: Object.fromEntries(all.map((label) => [label, model.mode(label)])),
          landedBefore,
          reachedMs: reached.every(Boolean) ? Math.max(...reached.map((one) => one.at - pressed)) : null,
          pausedMs: pausedAt === null ? null : pausedAt - pressed,
          band: hands.readBand(),
          control: hands.readControl(),
        };
      }, { subject: SUBJECT, otherTerm: OTHER_TERM });
      const askedOf = (label) => labelsOf(stopped.asked, label);
      ok(
        "the person's stop pauses every work browser an agent holds in this window, of every pane, through its own bar's road with the revision the window held, and leaves the one the person holds and the one already paused as they are",
        askedOf("browser-911").length === 1 && askedOf("browser-911")[0].mode === "paused" &&
          askedOf("browser-911")[0].expectedRevision === 3 && askedOf("browser-912").length === 1 &&
          askedOf("browser-912")[0].mode === "paused" && askedOf("browser-912")[0].expectedRevision === 5 &&
          askedOf("browser-913").length === 0 && askedOf("browser-914").length === 0 &&
          stopped.modes?.["browser-911"] === "paused" && stopped.modes?.["browser-912"] === "paused" &&
          stopped.modes?.["browser-913"] === "human" && stopped.modes?.["browser-914"] === "paused",
        JSON.stringify(stopped),
      );
      // What the window does while its agents keep trying: its pauses reach
      // the backend and its bars stand paused. That none of their input lands
      // meanwhile is the backend door's, which the model here only stands in
      // for — the Rust race test proves it (t-62872 r3).
      ok(
        "while two agents keep sending input and the stop's pauses are held on their way, the person's stop pauses both work browsers once its pauses reach the backend (press-to-pause time in the detail)",
        stopped.landedBefore > 0 && stopped.reachedMs !== null && stopped.pausedMs !== null &&
          stopped.modes?.["browser-911"] === "paused" && stopped.modes?.["browser-912"] === "paused",
        JSON.stringify(stopped),
      );
      ok(
        "the band and the panel say in one line how many work browsers the person's stop paused",
        Boolean(stopped.band?.browsers) && stopped.band.browsers.includes("2") &&
          stopped.control?.stopBrowsers === stopped.band.browsers,
        JSON.stringify(stopped),
      );

      const idle = await evaluateCase(page, async () => {
        const hands = window.__COMPUTER_HANDS__;
        const watched = ["set_browser_workspace_mode", "get_browser_workspaces", "computer_progress_frame", "computer_stop", "computer_resume"];
        const before = watched.map((name) => hands.sentCount(name));
        await new Promise((done) => hands.census.native.setTimeout(done, 1500));
        return { sent: watched.filter((name, at) => hands.sentCount(name) !== before[at]), census: hands.census.live() };
      });
      ok(
        "with the person's stop standing over paused work browsers and the panel open, an idle second and a half asks nothing and leaves no timer, interval or animation frame of the Computer Use files",
        idle.sent?.length === 0 && idle.census?.timeouts === 0 && idle.census?.intervals === 0 &&
          idle.census?.animationFrames === 0,
        JSON.stringify(idle),
      );

      // The bars while the stop stands: the one in view when it came, read as
      // it stands, and the one of a work browser the stop paused, brought to
      // the front; the one in view is put back in front for the resume.
      const bars = await evaluateCase(page, async () => {
        const hands = window.__COMPUTER_HANDS__;
        const model = window.__WORK_MODEL__;
        const inView = hands.readWorkBar("browser-913");
        const paused = model.tab("browser-911");
        if (paused) setActiveTab(paused.id);
        await hands.settle();
        const pausedBar = hands.readWorkBar("browser-911");
        const held = model.tab("browser-913");
        if (held) setActiveTab(held.id);
        await hands.settle();
        return { inView, paused: pausedBar };
      });

      const resumed = await evaluateCase(page, async () => {
        const hands = window.__COMPUTER_HANDS__;
        const model = window.__WORK_MODEL__;
        const tick = () => new Promise((done) => hands.census.native.setTimeout(done, 0));
        const all = ["browser-911", "browser-912", "browser-913", "browser-914"];
        // The person takes one of the stopped browsers by hand, in its bar.
        await setBrowserWorkspaceMode(model.tab("browser-912"), "human");
        await hands.settle();
        const tookByHand = model.mode("browser-912");
        const askedBefore = model.asked.length;
        hands.recordCommand("computer_resume", () => {
          model.lift();
          return { resumed: true, revision: 42, stopped: null, personsStop: false, helper: "idle" };
        });
        hands.pressBand("resume");
        const pressed = performance.now();
        while (performance.now() - pressed < 3000 && model.mode("browser-911") !== "agent") await tick();
        await hands.settle();
        return {
          tookByHand,
          asked: model.asked.slice(askedBefore).map(({ label, mode, expectedRevision }) => ({ label, mode, expectedRevision })),
          modes: Object.fromEntries(all.map((label) => [label, model.mode(label)])),
          band: hands.readBand(),
          control: hands.readControl(),
        };
      });
      ok(
        "the band's resume gives back to the agent only the work browsers its stop paused that still stand where it left them: the one the person took by hand meanwhile, the one they held and the one paused before stay as they are",
        resumed.tookByHand === "human" && resumed.asked?.length === 1 && resumed.asked[0].label === "browser-911" &&
          resumed.asked[0].mode === "agent" && resumed.modes?.["browser-911"] === "agent" &&
          resumed.modes?.["browser-912"] === "human" && resumed.modes?.["browser-913"] === "human" &&
          resumed.modes?.["browser-914"] === "paused",
        JSON.stringify(resumed),
      );
      ok(
        "once the person's stop is lifted the band and the panel no longer say that work browsers are stopped",
        resumed.band?.browsers === null && resumed.control?.stopBrowsers === null,
        JSON.stringify(resumed),
      );
      const barAfter = await evaluateCase(page, async () => window.__COMPUTER_HANDS__.readWorkBar("browser-913"));
      ok(
        "while the person's stop stands a work browser's bar hands it to no agent, whose input waits for the band's resume, and says so — the bar in view when the stop came and the bar of one the stop paused alike — and once the resume ends the stop the bar in view offers the agent its browser again and says nothing of a stop (t-62872)",
        bars.inView?.mode === "human" && bars.inView.offersAgent === false && Boolean(bars.inView.says) &&
          bars.paused?.mode === "paused" && bars.paused.offersAgent === false && bars.paused.says === bars.inView.says &&
          barAfter?.mode === "human" && barAfter.offersAgent === true && barAfter.says === null,
        JSON.stringify({ bars, barAfter }),
      );

      const agentsStop = await evaluateCase(page, async () => {
        const hands = window.__COMPUTER_HANDS__;
        const model = window.__WORK_MODEL__;
        const tick = () => new Promise((done) => hands.census.native.setTimeout(done, 0));
        const askedBefore = model.asked.length;
        hands.announce("computer:activity", hands.operatorReport({ revision: 43, stopped: "request", personsStop: false }));
        await hands.settle();
        await hands.settle();
        const seen = { asked: model.asked.length - askedBefore, mode: model.mode("browser-911"), band: hands.readBand() };
        // The person's stop over the agent's own: the backend makes it the
        // person's, and it reaches the work browsers.
        hands.recordCommand("computer_stop", () => {
          model.stop();
          return { stopped: true, reason: "window", revision: 44, personsStop: true, helperSignalled: false };
        });
        hands.pressBand("stop");
        const pressed = performance.now();
        while (performance.now() - pressed < 3000 && model.mode("browser-911") !== "paused") await tick();
        await hands.settle();
        const over = { mode: model.mode("browser-911"), band: hands.readBand() };
        hands.recordCommand("computer_resume", () => {
          model.lift();
          return { resumed: true, revision: 45, stopped: null, personsStop: false, helper: "idle" };
        });
        hands.pressBand("resume");
        const lifted = performance.now();
        while (performance.now() - lifted < 3000 && model.mode("browser-911") !== "agent") await tick();
        await hands.settle();
        hands.announce("computer:activity", hands.operatorReport({ revision: 46 }));
        await hands.settle();
        return { ...seen, over, back: model.mode("browser-911") };
      });
      ok(
        "an agent's own stop touches no work browser",
        agentsStop.asked === 0 && agentsStop.mode === "agent" && agentsStop.band?.browsers === null,
        JSON.stringify(agentsStop),
      );
      ok(
        "over an agent's own stop the band still offers the person's stop, and pressing it stops the work browsers too (t-62872)",
        agentsStop.band?.offersStop === true && agentsStop.over?.mode === "paused" &&
          agentsStop.over?.band?.showsStop === true && agentsStop.back === "agent",
        JSON.stringify(agentsStop),
      );

      const rounds = await evaluateCase(page, async ({ trials, otherTerm }) => {
        const hands = window.__COMPUTER_HANDS__;
        const model = window.__WORK_MODEL__;
        const tick = () => new Promise((done) => hands.census.native.setTimeout(done, 0));
        const agents = ["browser-911", "browser-915"];
        await model.open(["browser-915"]);
        model.say({ label: "browser-915", mode: "agent", revision: 2, ownerTerm: String(otherTerm) });
        await hands.settle();
        const stopHammer = model.hammer(agents);
        const reached = [];
        const painted = [];
        let resumed = 0;
        let revision = 50;
        for (let trial = 0; trial < trials; trial += 1) {
          hands.announce("computer:activity", hands.operatorReport({ revision, active: true }));
          revision += 1;
          const stopRevision = revision;
          hands.recordCommand("computer_stop", () => {
            model.stop();
            return { stopped: true, reason: "window", revision: stopRevision, personsStop: true, helperSignalled: false };
          });
          const pausesBefore = model.pauses.length;
          const pressed = performance.now();
          hands.pressBand("stop");
          while (performance.now() - pressed < 2000 && !agents.every((label) => model.mode(label) === "paused")) await tick();
          if (!agents.every((label) => model.mode(label) === "paused")) break;
          painted.push(performance.now() - pressed);
          reached.push(Math.max(...model.pauses.slice(pausesBefore).filter((one) => one.mode === "paused").map((one) => one.at - pressed)));
          await hands.settle();
          revision += 1;
          const liftRevision = revision;
          hands.recordCommand("computer_resume", () => {
            model.lift();
            return { resumed: true, revision: liftRevision, stopped: null, personsStop: false, helper: "idle" };
          });
          hands.pressBand("resume");
          const lifted = performance.now();
          while (performance.now() - lifted < 2000 && !agents.every((label) => model.mode(label) === "agent")) await tick();
          if (agents.every((label) => model.mode(label) === "agent")) resumed += 1;
          await hands.settle();
        }
        stopHammer();
        const quantile = (values, q) => {
          const sorted = [...values].sort((a, b) => a - b);
          return sorted.length === 0 ? null : Number(sorted[Math.min(sorted.length - 1, Math.floor(q * sorted.length))].toFixed(2));
        };
        return {
          trials: painted.length,
          resumed,
          reachedBackendMs: { p50: quantile(reached, 0.5), p95: quantile(reached, 0.95) },
          pausedOnScreenMs: { p50: quantile(painted, 0.5), p95: quantile(painted, 0.95) },
        };
      }, { trials: STOP_TRIALS, otherTerm: OTHER_TERM });
      ok(
        `over ${STOP_TRIALS} stops and resumes of two agents' work browsers under constant input, every stop pauses both and every resume gives both back (press-to-pause times in the detail)`,
        rounds.trials === STOP_TRIALS && rounds.resumed === STOP_TRIALS,
        JSON.stringify(rounds),
      );
      ok("the one stop scenario raised no renderer errors", faults.length === 0, faults.join("\n"));
    } finally { await page.close(); }
  }
  {
    const { page, faults } = await openComputerPage(browser, origin);
    try {
      await page.evaluate(installWorkBrowserModel);
      const raced = await evaluateCase(page, async ({ subject, otherTerm }) => {
        const hands = window.__COMPUTER_HANDS__;
        const model = window.__WORK_MODEL__;
        const tick = () => new Promise((done) => hands.census.native.setTimeout(done, 0));
        const all = ["browser-921", "browser-922", "browser-923", "browser-924"];
        await model.open(all);
        model.say({ label: "browser-921", mode: "agent", revision: 3, ownerTerm: String(subject.term) });
        model.say({ label: "browser-922", mode: "agent", revision: 7, ownerTerm: String(otherTerm) });
        model.say({ label: "browser-923", mode: "agent", revision: 9, ownerTerm: String(subject.term) });
        model.say({ label: "browser-924", mode: "agent", revision: 2, ownerTerm: String(otherTerm) });
        hands.recordCommand("computer_progress_frame", () => null);
        hands.announce("computer:activity", hands.operatorReport({ revision: 60, active: true }));
        hands.openPanel(subject);
        await hands.settle();
        // 921: the agent's input is in flight when the stop comes. 922: the
        // agent moves the revision once, just before the pause reaches it.
        // 923: the agent moves it before every pause. 924: the person closes
        // it while the stop's pause of it is still out.
        const inFlight = model.input("browser-921");
        model.moveBefore("browser-922", 1);
        model.moveBefore("browser-923", Number.MAX_SAFE_INTEGER);
        const release924 = model.hold("browser-924");
        hands.recordCommand("computer_stop", () => {
          model.stop();
          return { stopped: true, reason: "window", revision: 61, personsStop: true, helperSignalled: false };
        });
        hands.pressBand("stop");
        const pressed = performance.now();
        while (performance.now() - pressed < 3000 && !model.asked.some((one) => one.label === "browser-924")) await tick();
        if (model.tab("browser-924")) closeTab(model.tab("browser-924").id);
        release924();
        while (performance.now() - pressed < 3000 && model.mode("browser-921") !== "pausing") await tick();
        await hands.settle();
        const whilePausing = {
          mode: model.mode("browser-921"),
          band: hands.readBand(),
        };
        inFlight?.();
        const tries = typeof BROWSER_WORKSPACE_STOP_TRIES === "number" ? BROWSER_WORKSPACE_STOP_TRIES : null;
        const quiet = (label) => !model.tab(label)?.workspacePending && !model.tab(label)?.workspaceReading;
        const settled = () => model.mode("browser-921") === "paused" && model.mode("browser-922") === "paused" &&
          tries !== null && model.asked.filter((one) => one.label === "browser-923").length >= tries &&
          quiet("browser-923") && hands.readBand().offersResume;
        const finished = performance.now();
        while (performance.now() - finished < 3000 && !settled()) await tick();
        await hands.settle();
        const asked = (label) => model.asked.filter((one) => one.label === label)
          .map(({ mode, expectedRevision }) => ({ mode, expectedRevision }));
        return {
          whilePausing,
          tries,
          modes: Object.fromEntries(all.map((label) => [label, model.mode(label)])),
          asked922: asked("browser-922"),
          asked923: asked("browser-923"),
          asked924: asked("browser-924"),
          band: hands.readBand(),
          control: hands.readControl(),
        };
      }, { subject: SUBJECT, otherTerm: OTHER_TERM });
      ok(
        "a work browser whose agent input is in flight when the person's stop comes stands pausing under the stop's pause until that input ends, and then paused",
        raced.whilePausing?.mode === "pausing" && raced.modes?.["browser-921"] === "paused",
        JSON.stringify(raced),
      );
      ok(
        "while a work browser is still being paused the band does not offer the resume yet, and says the stop is reaching the work browsers",
        raced.whilePausing?.band?.offersResume === false && Boolean(raced.whilePausing?.band?.browsers),
        JSON.stringify(raced),
      );
      ok(
        "a pause the backend refuses because the agent moved the revision is read again and asked again with the revision read, and the work browser ends paused",
        raced.asked922?.length === 2 && raced.asked922[0].expectedRevision === 7 &&
          raced.asked922[1].expectedRevision === 8 && raced.asked922.every((one) => one.mode === "paused") &&
          raced.modes?.["browser-922"] === "paused",
        JSON.stringify(raced),
      );
      ok(
        "a work browser the stop still cannot pause after its tries is left as it is, and the band and the panel say how many it paused and how many it could not, and a work browser closed meanwhile is skipped",
        typeof raced.tries === "number" && raced.tries > 1 && raced.asked923?.length === raced.tries &&
          raced.modes?.["browser-923"] === "agent" && raced.asked924?.length === 1 && raced.modes?.["browser-924"] === null &&
          Boolean(raced.band?.browsers) && raced.band.browsers.includes("1") && raced.band.browsers.includes("2") &&
          raced.control?.stopBrowsers === raced.band.browsers && raced.band?.offersResume === true,
        JSON.stringify(raced),
      );
      ok("the stop's race scenario raised no renderer errors", faults.length === 0, faults.join("\n"));
    } finally { await page.close(); }
  }
}

/* What the person's stop leaves behind (t-62872): a work browser the backend
 * paused on its own while the stop was reaching it is not the stop's to give
 * back; one the stop paused that its resume must leave paused is counted on
 * the band; a resume that failed after the backend lifted the stop lets the
 * stop's record go; and the band's words agree with a count of one in every
 * language. */
async function testWhatTheStopLeaves(browser, origin, ok) {
  const { page, faults } = await openComputerPage(browser, origin);
  try {
    await page.evaluate(installWorkBrowserModel);
    const selfPaused = await evaluateCase(page, async ({ subject, otherTerm }) => {
      const hands = window.__COMPUTER_HANDS__;
      const model = window.__WORK_MODEL__;
      const tick = () => new Promise((done) => hands.census.native.setTimeout(done, 0));
      const all = ["browser-931", "browser-932", "browser-933"];
      await model.open(all);
      model.say({ label: "browser-931", mode: "agent", revision: 3, ownerTerm: String(subject.term) });
      model.say({ label: "browser-932", mode: "agent", revision: 5, ownerTerm: String(otherTerm) });
      model.say({ label: "browser-933", mode: "agent", revision: 7, ownerTerm: String(subject.term) });
      hands.announce("computer:activity", hands.operatorReport({ revision: 90, active: true }));
      await hands.settle();
      // The backend pauses 932 on its own just before the stop's pause of it
      // arrives: an unfinished page change of its agent's.
      model.pauseBefore("browser-932");
      hands.recordCommand("computer_stop", () => {
        model.stop();
        return { stopped: true, reason: "window", revision: 91, personsStop: true, helperSignalled: false };
      });
      hands.pressBand("stop");
      const pressed = performance.now();
      const quiet = (label) => !model.tab(label)?.workspacePending && !model.tab(label)?.workspaceReading;
      const still = () => all.every((label) => model.mode(label) === "paused" && quiet(label)) &&
        hands.readBand().offersResume;
      while (performance.now() - pressed < 3000 && !still()) await tick();
      await hands.settle();
      const whileStopped = {
        modes: Object.fromEntries(all.map((label) => [label, model.mode(label)])),
        band: hands.readBand(),
      };
      // The person starts choosing another agent for 933 in its own bar.
      const choosing = model.tab("browser-933");
      if (choosing) choosing.workspaceOwnerChoice = String(otherTerm);
      const askedBefore = model.asked.length;
      hands.recordCommand("computer_resume", () => {
        model.lift();
        return { resumed: true, revision: 92, stopped: null, personsStop: false, helper: "idle" };
      });
      hands.pressBand("resume");
      const lifted = performance.now();
      while (performance.now() - lifted < 3000 && model.mode("browser-931") !== "agent") await tick();
      await hands.settle();
      const handedBack = model.asked.slice(askedBefore).filter((one) => one.mode === "agent").map((one) => one.label);
      return {
        whileStopped,
        handedBack,
        handedBackSelfPaused: handedBack.filter((label) => label === "browser-932").length,
        modes: Object.fromEntries(all.map((label) => [label, model.mode(label)])),
        band: hands.readBand(),
      };
    }, { subject: SUBJECT, otherTerm: OTHER_TERM });
    ok(
      "a work browser the backend paused on its own while the person's stop was reaching it is not the stop's: the band's resume gives back only what the stop's own pause left, and that one stays paused (t-62872)",
      selfPaused.whileStopped?.modes?.["browser-932"] === "paused" && selfPaused.handedBackSelfPaused === 0 &&
        selfPaused.handedBack?.includes("browser-931") === true && selfPaused.modes?.["browser-931"] === "agent" &&
        selfPaused.modes?.["browser-932"] === "paused",
      JSON.stringify(selfPaused),
    );
    ok(
      "a work browser the stop paused that its resume leaves paused, while the person chooses its agent, is counted on the band in one line (t-62872)",
      selfPaused.modes?.["browser-933"] === "paused" && selfPaused.handedBack?.includes("browser-933") === false &&
        selfPaused.band?.shown === true && Boolean(selfPaused.band?.browsers) && selfPaused.band.browsers.includes("1"),
      JSON.stringify(selfPaused),
    );

    const words = await evaluateCase(page, async () => {
      const saved = locale;
      const shapes = {
        pausing: { stopped: 0, failed: 0, pausing: 1, kept: 0 },
        failed: { stopped: 1, failed: 1, pausing: 0, kept: 0 },
        stopped: { stopped: 1, failed: 0, pausing: 0, kept: 0 },
      };
      const said = {};
      for (const code of ["ko", "en", "ja", "zh", "es"]) {
        locale = code;
        said[code] = Object.fromEntries(Object.entries(shapes)
          .map(([name, reach]) => [name, computerStopReachWords(reach)?.() ?? null]));
      }
      locale = saved;
      return said;
    });
    const plural = /\b1 (task browsers|navegadores)\b/u;
    ok(
      "with one work browser the band's words agree with the count in every language: no plural stands beside a one (t-62872)",
      ["ko", "en", "ja", "zh", "es"].every((code) => Object.values(words?.[code] ?? {}).length === 3 &&
        Object.values(words[code]).every((text) => typeof text === "string" && text.includes("1") && !plural.test(text))),
      JSON.stringify(words),
    );

    const failedResume = await evaluateCase(page, async ({ subject }) => {
      const hands = window.__COMPUTER_HANDS__;
      const model = window.__WORK_MODEL__;
      const tick = () => new Promise((done) => hands.census.native.setTimeout(done, 0));
      hands.recordCommand("computer_stop", () => {
        model.stop();
        return { stopped: true, reason: "window", revision: 95, personsStop: true, helperSignalled: false };
      });
      hands.pressBand("stop");
      const pressed = performance.now();
      while (performance.now() - pressed < 3000 &&
        !(model.mode("browser-931") === "paused" && hands.readBand().offersResume)) await tick();
      await hands.settle();
      const resumes = hands.holdCommand("computer_resume");
      hands.pressBand("resume");
      await hands.settle();
      // The backend lifted the stop and said so, but its answer to the band's
      // resume never came back.
      model.lift();
      hands.announce("computer:activity", hands.operatorReport({ revision: 96 }));
      await hands.settle();
      resumes.refuseOldest("the window lost the resume's answer");
      await hands.settle();
      const afterFailure = { band: hands.readBand() };
      // An agent is handed a new work browser once the stop is over.
      const askedBefore = model.asked.length;
      await model.open(["browser-934"]);
      model.say({ label: "browser-934", mode: "agent", revision: 2, ownerTerm: String(subject.term) });
      await hands.settle();
      await hands.settle();
      hands.releaseCommands();
      return {
        afterFailure,
        pausedAfterTheLift: model.asked.slice(askedBefore).filter((one) => one.label === "browser-934").length,
        mode934: model.mode("browser-934"),
        mode931: model.mode("browser-931"),
      };
    }, { subject: SUBJECT });
    ok(
      "when the band's resume fails after the backend already lifted the stop, the stop's record goes with the next report: a work browser an agent is handed afterwards is not paused, the band says no browser is stopped, and nothing was given back (t-62872)",
      failedResume.pausedAfterTheLift === 0 && failedResume.mode934 === "agent" &&
        failedResume.afterFailure?.band?.browsers === null && failedResume.mode931 === "paused",
      JSON.stringify(failedResume),
    );
    ok("the scenario of what the stop leaves raised no renderer errors", faults.length === 0, faults.join("\n"));
  } finally { await page.close(); }
}

/* The panel when the backend let its look go, and the band in the board
 * pop-out (t-62872): a door that answers nothing, or cannot tell the pane's
 * work, takes the picture off the open panel; the pop-out's band offers the
 * person's stop, which is safe from anywhere, and leaves the resume and the
 * work screen view to the main window. */
async function testThePanelAndThePopout(browser, origin, ok) {
  const { page, faults } = await openComputerPage(browser, origin);
  try {
    const gone = await evaluateCase(page, async ({ subject }) => {
      const hands = window.__COMPUTER_HANDS__;
      const frames = hands.holdCommand("computer_progress_frame");
      hands.openPanel(subject);
      await hands.settle();
      frames.answerOldest(hands.look(subject, 50));
      await hands.settle();
      const shown = hands.readPanel();
      // The pane looked while the window could not tell its work: the backend
      // let the older look go and told the window by a newer number.
      hands.announce("computer:frame", { ...subject, seq: 51 });
      await hands.settle();
      frames.answerOldest(null);
      await hands.settle();
      const answeredNothing = hands.readPanel();
      hands.announce("computer:frame", { ...subject, seq: 52 });
      await hands.settle();
      frames.answerOldest(hands.look(subject, 52));
      await hands.settle();
      const shownAgain = hands.readPanel();
      hands.announce("computer:frame", { ...subject, seq: 53 });
      await hands.settle();
      frames.refuseOldest("computer_progress_scope_unknown: the window cannot tell the pane's work");
      await hands.settle();
      const refused = hands.readPanel();
      hands.closePanel();
      await hands.settle();
      hands.releaseCommands();
      return { shown, answeredNothing, shownAgain, refused };
    }, { subject: SUBJECT });
    ok(
      "when the backend no longer has the look the open panel shows — its door answers nothing, or cannot tell the pane's work — the panel lets the picture go and says it is no longer kept (t-62872)",
      Boolean(gone.shown?.picture) && !gone.answeredNothing?.picture && Boolean(gone.answeredNothing?.note) &&
        Boolean(gone.shownAgain?.picture) && gone.shownAgain?.seq === "52" && !gone.refused?.picture &&
        Boolean(gone.refused?.note),
      JSON.stringify(gone),
    );

    const popout = await evaluateCase(page, async ({ subject }) => {
      const hands = window.__COMPUTER_HANDS__;
      const offered = (id) => {
        const node = document.getElementById(id);
        return !!node && !node.hidden && getComputedStyle(node).display !== "none";
      };
      hands.announce("computer:frame", { ...subject, seq: 60 });
      hands.announce("computer:activity", hands.operatorReport({ revision: 130, active: true }));
      await hands.settle();
      const main = { stop: offered("computer-band-stop"), view: offered("computer-band-view") };
      document.documentElement.dataset.surface = "popout";
      await hands.settle();
      const acting = { stop: offered("computer-band-stop"), view: offered("computer-band-view") };
      hands.announce("computer:activity", hands.operatorReport({ revision: 131, stopped: "window", personsStop: true }));
      await hands.settle();
      const stopped = {
        shows: hands.readBand().showsStop,
        resume: offered("computer-band-resume"),
        view: offered("computer-band-view"),
      };
      delete document.documentElement.dataset.surface;
      await hands.settle();
      const inTheMain = { resume: offered("computer-band-resume") };
      hands.recordCommand("computer_resume", () => ({
        resumed: true, revision: 132, stopped: null, personsStop: false, helper: "idle",
      }));
      hands.pressBand("resume");
      await hands.settle();
      hands.announce("computer:activity", hands.operatorReport({ revision: 132 }));
      await hands.settle();
      hands.releaseCommands();
      return { main, acting, stopped, inTheMain };
    }, { subject: SUBJECT });
    ok(
      "in the board pop-out the band offers the person's stop, which is safe from anywhere, and neither the resume nor the work screen view, which are the main window's (t-62872)",
      popout.main?.stop === true && popout.main?.view === true && popout.acting?.stop === true &&
        popout.acting?.view === false && popout.stopped?.shows === true && popout.stopped?.resume === false &&
        popout.stopped?.view === false && popout.inTheMain?.resume === true,
      JSON.stringify(popout),
    );
    ok("the panel and pop-out scenario raised no renderer errors", faults.length === 0, faults.join("\n"));
  } finally { await page.close(); }
}

/* A reload of the window during the person's stop (t-62872): the band reads
 * the backend's report once as the page loads — no clock — so the standing
 * stop shows at once and walks the work browsers, and the stop's record,
 * kept in the page's session under the stop's revision, lets the band's
 * resume give back exactly what that stop paused. */
async function testAReloadDuringTheStop(browser, origin, ok) {
  const { page, faults } = await openComputerPage(browser, origin);
  try {
    await page.evaluate(installWorkBrowserModel);
    const before = await evaluateCase(page, async ({ subject, otherTerm }) => {
      const hands = window.__COMPUTER_HANDS__;
      const model = window.__WORK_MODEL__;
      const tick = () => new Promise((done) => hands.census.native.setTimeout(done, 0));
      await model.open(["browser-951", "browser-952", "browser-953"]);
      model.say({ label: "browser-951", mode: "agent", revision: 3, ownerTerm: String(subject.term) });
      model.say({ label: "browser-952", mode: "agent", revision: 5, ownerTerm: String(otherTerm) });
      model.say({ label: "browser-953", mode: "paused", revision: 8, ownerTerm: String(otherTerm) });
      hands.announce("computer:activity", hands.operatorReport({ revision: 80, active: true }));
      await hands.settle();
      hands.recordCommand("computer_stop", () => {
        model.stop();
        return { stopped: true, reason: "window", revision: 81, personsStop: true, helperSignalled: false };
      });
      hands.pressBand("stop");
      const pressed = performance.now();
      while (performance.now() - pressed < 3000 &&
        !(model.mode("browser-951") === "paused" && model.mode("browser-952") === "paused")) await tick();
      await hands.settle();
      return { states: [...model.states.values()].map((state) => ({ ...state })), band: hands.readBand() };
    }, { subject: SUBJECT, otherTerm: OTHER_TERM });
    // The backend across the reload: the person's stop still stands at 81,
    // and that is what the page's one read of the report is told.
    await page.addInitScript(({ hotkey }) => {
      window.__GUARD_READS__ = [];
      window.__ANSWER__.computer_guard_status = () => {
        window.__GUARD_READS__.push((new Error().stack ?? "").includes("/shell-computer"));
        return { active: false, verb: null, actions: 3, at: 0, personsStop: true, stopped: "window", revision: 81, hotkey };
      };
    }, { hotkey: STOP_HOTKEY });
    await page.reload();
    await page.waitForFunction(() => typeof BOUND !== "undefined" && BOUND.size > 0);
    await page.evaluate(installComputerHands, { hotkey: STOP_HOTKEY });
    await page.evaluate(installWorkBrowserModel);
    const after = await evaluateCase(page, async ({ states, subject }) => {
      const hands = window.__COMPUTER_HANDS__;
      const model = window.__WORK_MODEL__;
      const tick = () => new Promise((done) => hands.census.native.setTimeout(done, 0));
      await hands.settle();
      const onLoad = { band: hands.readBand(), reads: window.__GUARD_READS__.length };
      model.stop();
      // The window holds the same work browsers again, as the backend kept
      // them, and an agent is handed one more meanwhile.
      await model.open([...states.map((state) => state.label), "browser-954"]);
      for (const state of states) model.say(state);
      model.say({ label: "browser-954", mode: "agent", revision: 2, ownerTerm: String(subject.term) });
      const arrived = performance.now();
      while (performance.now() - arrived < 3000 &&
        !(model.mode("browser-954") === "paused" && hands.readBand().offersResume)) await tick();
      await hands.settle();
      const reopened = { band: hands.readBand(), asked: model.asked.map(({ label, mode }) => ({ label, mode })) };
      const askedBefore = model.asked.length;
      hands.recordCommand("computer_resume", () => {
        model.lift();
        return { resumed: true, revision: 82, stopped: null, personsStop: false, helper: "idle" };
      });
      hands.pressBand("resume");
      const pressed = performance.now();
      const back = ["browser-951", "browser-952", "browser-954"];
      while (performance.now() - pressed < 3000 && !back.every((label) => model.mode(label) === "agent")) await tick();
      await hands.settle();
      hands.releaseCommands();
      return {
        onLoad,
        reopened,
        handedBack: model.asked.slice(askedBefore).map(({ label, mode, expectedRevision }) => ({ label, mode, expectedRevision })),
        modes: Object.fromEntries([...states.map((state) => state.label), "browser-954"]
          .map((label) => [label, model.mode(label)])),
        reads: window.__GUARD_READS__.length,
        fromTheBand: window.__GUARD_READS__.every(Boolean),
        census: hands.census.live(),
      };
    }, { states: before.states, subject: SUBJECT });
    ok(
      "a reload during the person's stop reads the backend's report once, from the band, shows the stop with its resume at once and pauses a work browser an agent is handed after it (t-62872)",
      after.onLoad?.band?.showsStop === true && after.onLoad?.band?.offersResume === true && after.onLoad?.reads === 1 &&
        after.reads === 1 && after.fromTheBand === true && after.reopened?.asked?.length === 1 &&
        after.reopened.asked[0].label === "browser-954" && after.reopened.asked[0].mode === "paused",
      JSON.stringify(after),
    );
    const revisionOf = (label) => before.states?.find((state) => state.label === label)?.revision;
    const handedBack = (label) => after.handedBack?.filter((one) => one.label === label) ?? [];
    ok(
      "after a reload during the person's stop the band's resume gives back exactly what that stop paused, before the reload and after it, and the work browser paused before it stays paused (t-62872)",
      after.handedBack?.length === 3 && after.handedBack.every((one) => one.mode === "agent") &&
        handedBack("browser-951")[0]?.expectedRevision === revisionOf("browser-951") &&
        handedBack("browser-952")[0]?.expectedRevision === revisionOf("browser-952") &&
        handedBack("browser-954").length === 1 && handedBack("browser-953").length === 0 &&
        after.modes?.["browser-953"] === "paused" && after.modes?.["browser-951"] === "agent" &&
        after.modes?.["browser-952"] === "agent" && after.modes?.["browser-954"] === "agent",
      JSON.stringify({ before, after }),
    );
    ok("the reload scenario raised no renderer errors", faults.length === 0, faults.join("\n"));
  } finally { await page.close(); }
}

export async function testComputerProgress(browser, origin, ok) {
  await testTheBand(browser, origin, ok);
  await testTheProgressPanel(browser, origin, ok);
  await testTheBandWithThePanel(browser, origin, ok);
  await testThePanelReadsTheWorkBrowser(browser, origin, ok);
  await testTheOneStop(browser, origin, ok);
  await testWhatTheStopLeaves(browser, origin, ok);
  await testThePanelAndThePopout(browser, origin, ok);
  await testAReloadDuringTheStop(browser, origin, ok);
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
