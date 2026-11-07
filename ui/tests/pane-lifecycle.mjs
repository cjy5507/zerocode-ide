import { openWindowTestPage } from "./window-boot.mjs";

/* What the window remembers about a pane ends with the pane — on the real
 * roads, in a real page.
 *
 * `pane-lifecycle.test.mjs` holds the lifecycle's interface to account in a bare
 * context. This suite asks the other half: that the window declares what a pane
 * keeps, and that the roads a pane really ends by — the tab's close, the
 * program's exit, a detached agent's last word — leave nothing behind for the
 * next terminal to take the id: no badge, model, sentence or helper row.
 *
 * Two scenes, each run in the page (`page.evaluate`), so they hold no reference
 * outside themselves:
 *
 *   exercisePaneLeftovers   the omissions the lifecycle was made to close. It
 *       reads only what the window has always had (never `paneLifecycle`), so it
 *       can be run against a checkout from before the lifecycle, and must fail
 *       there: that is its red.
 *   exercisePaneLifecycle   the lifecycle on the real roads: teardown with a
 *       neighbour left alone, an id that starts empty, a pane ended twice, a
 *       detached agent, the board's borrowed screen, the snapshot fence, a
 *       failing release, and the census put back where it started. */

/* A pane whose window was reloaded under it. The backend outlives a webview
 * reload, and boot re-seeds `paneSessions` from what it kept (`pane_sessions`) —
 * without a conversation record, which only a hook makes. The agent goes on
 * streaming on its own channel, so the window holds live words and a status for
 * a pane that has never had a conversation page.
 *
 * Closing such a pane left them: `forgetPaneChat` returned first when no page
 * stood, and took none of the pane's other facts with it. It left, as well, the
 * time of the last key the person typed (`typedInto`, forgotten only on the
 * sleep road) and the hush the notify seat put on the pane's last ring
 * (`hushedPanes`, forgotten only when the state moved). */
export async function exercisePaneLeftovers() {
  const settle = (ms = 30) => new Promise((done) => setTimeout(done, ms));
  const until = async (done, tries = 100) => {
    for (let at = 0; at < tries && !done(); at += 1) await settle(20);
    return done();
  };
  const tell = (name, payload) => {
    for (const listener of window.__LISTENERS__[name] ?? []) listener({ payload });
  };
  // The 30 s snapshot beat writes a state of its own into the very maps this
  // scene reads; held, it cannot land between two of its reads.
  window.__HOLD_POLLERS__ = true;
  try {
    const term = await openTermTab({ placement: "tab" });
    const tab = tabOfTerm(term);
    const session = { key: "session_id", id: `reloaded-${term}` };
    // Seeded the way boot seeds it, and nothing more: no hook has come.
    paneSessions.set(term, { agent: "zo", session, resumable: true });
    noteTypedInto(term);
    tell("session:frame", { session: session.id, frame: { type: "text_delta", id: "m1", text: "half a sentence", done: false } });
    tell("session:frame", {
      session: session.id,
      frame: { type: "session_status", activity: { verb: "Read", target: "a file", phase: "started" } },
    });
    tell("notify:call", { term, call: "batch" });

    const read = () => ({
      typedInto: typedInto.has(term),
      hushedPanes: hushedPanes.has(term),
      paneLive: paneLive.has(term),
      paneNow: paneNow.has(term),
    });
    const peak = read();
    const chatless = !paneChats.has(term);

    setConfirmCloseRunning(false);
    closeTab(tab.id);
    const closed = await until(() => !tabs.includes(tab));
    setConfirmCloseRunning(true);
    await settle();

    return { term, peak, chatless, closed, left: read() };
  } finally {
    window.__HOLD_POLLERS__ = false;
    window.__RELEASE_POLLERS__();
  }
}

export async function exercisePaneLifecycle() {
  const settle = (ms = 30) => new Promise((done) => setTimeout(done, ms));
  const until = async (done, tries = 100) => {
    for (let at = 0; at < tries && !done(); at += 1) await settle(20);
    return done();
  };
  const tell = (name, payload) => {
    for (const listener of window.__LISTENERS__[name] ?? []) listener({ payload });
  };
  const closeTabAndWait = async (tab) => {
    setConfirmCloseRunning(false);
    closeTab(tab.id);
    const gone = await until(() => !tabs.includes(tab));
    setConfirmCloseRunning(true);
    await settle();
    return gone;
  };
  // The fake backend counts terminal ids upward. A pane is opened here so the
  // count is known, and an id can be taken a second time through the real road:
  // the count is stepped back, the pane opened, and the count put back.
  let highest = 0;
  const openPane = async () => {
    const term = await openTermTab({ placement: "tab" });
    highest = Math.max(highest, term);
    return term;
  };
  const reopenAt = async (term) => {
    window.__NEXT_TERM__ = term - 1;
    const again = await openTermTab({ placement: "tab" });
    window.__NEXT_TERM__ = highest;
    return again;
  };
  // Anything a road throws on its own, in the page: an exception or a rejection
  // nobody caught. The suite's `faults` hears the same, but only as a total.
  const raised = [];
  const hear = (event) => raised.push(String(event.message ?? event.reason?.message ?? event.reason ?? "unknown"));
  window.addEventListener("error", hear);
  window.addEventListener("unhandledrejection", hear);

  const seen = {};
  const baseline = paneLifecycle.sizes();
  window.__HOLD_POLLERS__ = true;
  const realClear = window.clearTimeout;
  const realSync = agentClock.sync;
  try {
    /* What a pane keeps once it has been heard from: through the window's own
     * listeners where a road exists, directly where the road is a backend round
     * trip or a restore. These are the names `paneLifecycle.holds` should then
     * say. A fresh tab may itself set `paneBorn` and `acknowledgedPanes`, so a
     * check that an id starts empty leaves those two out. */
    const REACHED = [
      "hookStates", "hookObservations", "hookSnapshotMarks", "hookStamps", "paneModels", "panePermissionModes",
      "panePrompts", "paneSaid", "paneAnswer", "paneAgents", "paneSessions", "paneChats", "paneSubagents",
      "paneActivities", "hushedPanes", "typedInto", "paneLive", "paneNow", "paneUsage", "paneEfforts",
      "paneBorn", "withheldNotices", "paneConversationWords", "paneAutonomy", "restoredWorkers",
      "workerListTeams", "paneAsks", "bellRang", "placedWorkers", "acknowledgedPanes",
      "reviewedAnswers", "paneReviewSaid",
    ];
    const FRESH_TAB_MAY_SET = ["paneBorn", "acknowledgedPanes"];
    const fill = (term, label) => {
      const session = { key: "session_id", id: `lifecycle-${label}` };
      tell("hook:agent", {
        term, state: "working", agent: "claude", model: "model-x", permission_mode: "plan",
        prompt: `prompt ${label}`, said: `said ${label}`, session, resumable: true,
      });
      tell("hook:agent", { term, state: "done", agent: "claude", answer: `answer ${label}`, session, resumable: true });
      tell("hook:subagent", { term, rows: [{ id: `helper-${label}`, name: "explore", state: "working" }] });
      tell("hook:activity", {
        pane: `term:${term}`,
        activities: [{ seq: 0, activity: { verb: "bash", target: "cargo test", phase: "started" } }],
      });
      tell("hook:activity", {
        pane: `sub:${term}:helper-${label}`,
        activities: [{ seq: 0, activity: { verb: "grep", target: "leak", phase: "started" } }],
      });
      tell("session:frame", { session: session.id, frame: { type: "text_delta", id: "m1", text: `live ${label}`, done: false } });
      tell("session:frame", {
        session: session.id,
        frame: { type: "session_status", activity: { verb: "Read", target: "a file", phase: "started" } },
      });
      tell("notify:call", { term, call: "batch" });
      noteTypedInto(term);
      rememberPaneUsage(term, { tokens: 1200, window: 200000 });
      rememberPaneEffort(term, { level: "high" });
      hookStates.noteSnapshot(term);
      paneBorn.set(term, Date.now());
      withheldNotices.set(term, new Map());
      paneConversationWords.set(term, { line: `a word about ${label}`, session: session.id });
      rememberPaneAutonomy(term, "full", session.id);
      restoredWorkers.set(term, "session");
      workerListTeams.add(term);
      // The live page can paint this entry while its neighbour closes. Seed
      // a real permission card, not an impossible ask with neither question.
      paneAsks.set(term, {
        agent: "claude", approval: { tool: "Read", summary: `fixture ${label}`, options: [], edits: [], plan: null },
        ask_prompt: null, sig: `ask-${label}`,
      });
      bellRang.add(term);
      acknowledgedPanes.set(`term:${term}`, Date.now());
      reviewedAnswers.set(term, `key-${label}`);
      paneReviewSaid.set(term, 1);
      placedWorkers.set(term, {
        worker: `w-${label}`, seenAfterMs: 0, seen: false, seenTimer: setTimeout(() => {}, 600_000),
      });
    };
    const holding = (term) => paneLifecycle.holds(term);
    const missing = (term) => REACHED.filter((name) => !holding(term).includes(name));

    // ---- teardown by the road a person closes by, a neighbour left alone ----
    const cleared = [];
    window.clearTimeout = (handle) => {
      cleared.push(handle);
      return realClear.call(window, handle);
    };
    const first = await openPane();
    const second = await openPane();
    const firstTab = tabOfTerm(first);
    const secondTab = tabOfTerm(second);
    fill(first, "first");
    fill(second, "second");
    const timerOfFirst = placedWorkers.get(first).seenTimer;
    const timerOfSecond = placedWorkers.get(second).seenTimer;
    const neighbour = holding(second);
    seen.teardown = { peakMissing: missing(first), neighbourPeakMissing: missing(second) };
    seen.teardown.tabGone = await closeTabAndWait(firstTab);
    seen.teardown.left = holding(first);
    seen.teardown.neighbourKept = JSON.stringify(holding(second)) === JSON.stringify(neighbour);
    seen.teardown.neighbourModel = paneModels.get(second) ?? null;
    seen.teardown.timerLetGo = cleared.includes(timerOfFirst);
    seen.teardown.neighbourTimerKept = !cleared.includes(timerOfSecond);
    window.clearTimeout = realClear;

    // ---- the id starts empty: the same id, taken again through the real road ----
    const reused = await reopenAt(first);
    const reusedTab = tabOfTerm(reused);
    seen.reuse = {
      sameId: reused === first,
      inherited: holding(reused).filter((name) => !FRESH_TAB_MAY_SET.includes(name)),
      badge: hookStates.get(reused) ?? null,
      fence: hookStates.observedAt(reused),
    };
    tell("hook:agent", { term: reused, state: "working", agent: "codex", session: { key: "session_id", id: "second-life" }, resumable: true });
    seen.reuse.next = {
      agent: paneAgents.get(reused) ?? null,
      model: paneModels.get(reused) ?? null,
      said: paneSaid.get(reused) ?? null,
      session: paneSessions.get(reused)?.session?.id ?? null,
    };
    seen.reuse.nextEnded = await closeTabAndWait(reusedTab);
    seen.reuse.nextLeft = holding(reused);

    // ---- a pane ended twice: the exit the backend announces, then more closes ----
    raised.length = 0;
    tell("term:exited", { term: first });
    await settle();
    dropTermView(first);
    dropTermView(first);
    seen.twice = { left: holding(first), neighbour: holding(second).length > 0, raised: [...raised] };

    // ---- a detached agent keeps what the window knows; its last word ends all of it ----
    const host = await openPane();
    const hostTab = tabOfTerm(host);
    const teammate = host + 20_000;
    tell("term:split", { parent: host, term: teammate, direction: "vertical", title: "teammate", helper: "zo-helper-teammate" });
    await settle();
    tell("hook:agent", {
      term: teammate, state: "working", agent: "claude", model: "model-t",
      session: { key: "session_id", id: "teammate" }, resumable: true,
    });
    seen.detach = { host, parent: paneParents.get(teammate) ?? null, helper: paneHelpers.get(teammate) ?? null };
    await closePaneLeaf(hostTab, teammate);
    seen.detach.detached = detachedAgents.has(teammate);
    seen.detach.keptWhileDetached = paneAgents.has(teammate) && paneModels.get(teammate) === "model-t" && hookStates.has(teammate);
    seen.detach.screenGone = !termViews.has(teammate);
    seen.detach.holdsWhileDetached = holding(teammate);
    dropTermView(teammate);
    seen.detach.left = holding(teammate);
    seen.detach.onList = detachedAgents.has(teammate);
    seen.detach.edgeGone = !paneParents.has(teammate) && !paneHelpers.has(teammate);
    await closeTabAndWait(hostTab);

    // ---- the board's borrowed screen goes back; the shell's end forgets the card ----
    const peeked = 70_000 + first;
    previewScreens.set(peeked, document.createElement("div"));
    previewRows.set(peeked, [{ index: 0, text: "tail" }]);
    dropTermScreen(peeked);
    seen.preview = { keptOnReturn: previewScreens.has(peeked) && previewRows.has(peeked) };
    dropTermView(peeked);
    seen.preview.goneWithShell = !previewScreens.has(peeked) && !previewRows.has(peeked) && holding(peeked).length === 0;

    // ---- an old snapshot's fence belongs to the shell that was observed ----
    const fenced = 80_000 + first;
    const snapshot = [{ term: fenced, agent: "codex", state: "done", at: 300, state_started_at: 250 }];
    hookStates.set(fenced, "working");
    hookStates.observe(fenced, 900);
    const older = reconcilePaneAgentSnapshot(snapshot);
    seen.fence = { olderRefused: older === false, stateKept: hookStates.get(fenced) === "working" };
    dropTermView(fenced);
    const next = reconcilePaneAgentSnapshot(snapshot);
    seen.fence.nextShellTakes = next === true && hookStates.get(fenced) === "done" && hookStamps.get(fenced) === 250;
    dropTermView(fenced);
    seen.fence.left = holding(fenced);

    // ---- a release that fails leaves neither a screen nor a clock behind ----
    const doomed = await openPane();
    const doomedTab = tabOfTerm(doomed);
    placedWorkers.set(doomed, {
      worker: "w-doomed", seenAfterMs: 0, seen: false,
      get seenTimer() { throw new Error("release failed on purpose"); },
    });
    paneModels.set(doomed, "model-doomed");
    let syncs = 0;
    agentClock.sync = () => {
      syncs += 1;
      return realSync();
    };
    const hadView = termViews.has(doomed);
    let thrown = null;
    try {
      dropTermView(doomed);
    } catch (error) {
      thrown = String(error?.message ?? error);
    }
    agentClock.sync = realSync;
    seen.failing = { hadView, thrown, screenGone: !termViews.has(doomed), clockAsked: syncs, left: holding(doomed) };
    await closeTabAndWait(doomedTab);

    // ---- an answer that comes back after its pane ended, and after the id was taken again ----
    // The backend answers on its own time. Here it holds the answer to the
    // effort question while the person closes the pane, then lets the shell that
    // takes the id ask for its own, and answers the old question first.
    // The held beats go here: a question the harness parks as a beat's would never
    // reach the answer this scene hands it.
    window.__HOLD_POLLERS__ = false;
    window.__RELEASE_POLLERS__();
    const late = await openPane();
    const lateTab = tabOfTerm(late);
    const efforts = [];
    window.__ANSWER__.pane_effort = () => new Promise((resolve) => efforts.push(resolve));
    const effortAsking = refreshPaneEffort(late);
    await settle();
    seen.late = { out: efforts.length };
    seen.late.closed = await closeTabAndWait(lateTab);
    const reborn = await reopenAt(late);
    const rebornAsking = refreshPaneEffort(reborn);
    await settle();
    seen.late.sameId = reborn === late;
    seen.late.outAgain = efforts.length;
    efforts[0]({ level: "stranger" });
    await effortAsking;
    seen.late.strangerWritten = paneEfforts.get(reborn)?.said ?? null;
    efforts[1]({ level: "mine" });
    await rebornAsking;
    seen.late.mine = paneEfforts.get(reborn)?.said?.level ?? null;
    await closeTabAndWait(tabOfTerm(reborn));
    seen.late.left = holding(late);
    delete window.__ANSWER__.pane_effort;

    // ---- a snapshot of every pane, answered after one of them ended ----
    // `pane_agents` is the retained state of every agent, asked on the clock. The
    // pane that ends while it is out must not come back from it — not even
    // through a stamp that looks new against the clock disposing cleared — while
    // a pane that stands, and an agent this window holds no tab for, are applied.
    const doomedByAnswer = await openPane();
    const standing = await openPane();
    const doomedByAnswerTab = tabOfTerm(doomedByAnswer);
    hookStates.set(doomedByAnswer, "working");
    hookStates.observe(doomedByAnswer, 900);
    // Wait until no snapshot is out: one asked while another is in flight is
    // not asked at all.
    await until(() => !paneSnapshotAsking);
    const backendOwned = 90_000 + highest;
    const snapshots = [];
    window.__ANSWER__.pane_agents = () => new Promise((resolve) => snapshots.push(resolve));
    const refreshing = refreshPaneAgentSnapshot();
    await settle();
    seen.snapshot = { out: snapshots.length };
    seen.snapshot.closed = await closeTabAndWait(doomedByAnswerTab);
    snapshots[0]([
      { term: doomedByAnswer, agent: "codex", state: "done", at: 300, state_started_at: 250 },
      { term: standing, agent: "claude", state: "working", at: 400, state_started_at: 350 },
      { term: backendOwned, agent: "claude", state: "working", at: 500, state_started_at: 450 },
    ]);
    await refreshing;
    seen.snapshot.goneState = hookStates.get(doomedByAnswer) ?? null;
    seen.snapshot.goneStamp = hookStamps.get(doomedByAnswer) ?? null;
    seen.snapshot.goneFence = hookStates.observedAt(doomedByAnswer);
    seen.snapshot.goneHolds = holding(doomedByAnswer);
    seen.snapshot.standingState = hookStates.get(standing) ?? null;
    seen.snapshot.backendOwnedState = hookStates.get(backendOwned) ?? null;
    delete window.__ANSWER__.pane_agents;
    // The backend's own agent was seen once, which adoption remembers for the next
    // answer; this scene does not want it adopted.
    strandSightings.clear();
    hookStates.delete(backendOwned);
    hookStamps.delete(backendOwned);
    await closeTabAndWait(tabOfTerm(standing));

    // ---- the seeds: the agents the backend lists, answered after a pane ended ----
    const seedDoomed = await openPane();
    const seedStanding = await openPane();
    const seedDoomedTab = tabOfTerm(seedDoomed);
    const agentLists = [];
    window.__ANSWER__.agent_terms = () => new Promise((resolve) => agentLists.push(resolve));
    const seeding = seedPaneAgents();
    await settle();
    await closeTabAndWait(seedDoomedTab);
    agentLists[0]([[seedDoomed, "claude"], [seedStanding, "codex"]]);
    await seeding;
    seen.seed = {
      goneAgent: paneAgents.get(seedDoomed) ?? null,
      goneHolds: holding(seedDoomed),
      standingAgent: paneAgents.get(seedStanding) ?? null,
    };
    delete window.__ANSWER__.agent_terms;
    await closeTabAndWait(tabOfTerm(seedStanding));

    // ---- and everything put back where the window started ----
    await closeTabAndWait(secondTab);
    await settle(80);
    const after = paneLifecycle.sizes();
    seen.census = {
      questionsOut: paneLifecycle.pending(),
      declared: Object.keys(after).length,
      moved: Object.keys(after).filter((name) => after[name] !== baseline[name]).map((name) => `${name} ${baseline[name]}→${after[name]}`),
    };
    seen.raised = [...raised];
    return seen;
  } finally {
    window.clearTimeout = realClear;
    agentClock.sync = realSync;
    window.__HOLD_POLLERS__ = false;
    window.__RELEASE_POLLERS__();
    window.removeEventListener("error", hear);
    window.removeEventListener("unhandledrejection", hear);
  }
}

export async function testPaneLifecycle(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    const left = await page.evaluate(exercisePaneLeftovers);
    ok(
      "pane lifecycle: a pane reloaded under the window, with no conversation page, still holds the typed-at time, the hush, the live words and the status",
      left.chatless && left.closed && Object.values(left.peak).every(Boolean),
      JSON.stringify(left),
    );
    ok(
      "pane lifecycle: closing that pane leaves none of the typed-at time, the hush, the live words or the status for the next terminal to take its id",
      left.closed && Object.values(left.left).every((held) => held === false),
      JSON.stringify(left),
    );

    const seen = await page.evaluate(exercisePaneLifecycle);
    ok(
      "pane lifecycle: a pane heard from on every road holds its state under every name the window declared for it",
      seen.teardown.peakMissing.length === 0 && seen.teardown.neighbourPeakMissing.length === 0,
      JSON.stringify(seen.teardown),
    );
    ok(
      "pane lifecycle: closing a tab leaves nothing of its shell and lets its timer go, and leaves the neighbour's state and timer alone",
      seen.teardown.tabGone && seen.teardown.left.length === 0 && seen.teardown.timerLetGo &&
        seen.teardown.neighbourKept && seen.teardown.neighbourModel === "model-x" && seen.teardown.neighbourTimerKept,
      JSON.stringify(seen.teardown),
    );
    ok(
      "pane lifecycle: the next terminal to take an id starts without the badge, model, conversation, words or fence of the one before, and ends cleanly in its turn",
      seen.reuse.sameId && seen.reuse.inherited.length === 0 && seen.reuse.badge === null && seen.reuse.fence === 0 &&
        seen.reuse.next.agent === "codex" && seen.reuse.next.model === null && seen.reuse.next.said === null &&
        seen.reuse.next.session === "second-life" && seen.reuse.nextEnded && seen.reuse.nextLeft.length === 0,
      JSON.stringify(seen.reuse),
    );
    ok(
      "pane lifecycle: a pane closed, then announced exited by the backend, then closed again, ends once and raises nothing",
      seen.twice.left.length === 0 && seen.twice.neighbour && seen.twice.raised.length === 0,
      JSON.stringify(seen.twice),
    );
    ok(
      "pane lifecycle: a detached agent's screen goes while the window keeps what it knows, and its last word ends all of it, its row and its edges with it",
      seen.detach.parent === seen.detach.host && seen.detach.helper === "zo-helper-teammate" && seen.detach.detached &&
        seen.detach.keptWhileDetached && seen.detach.screenGone && seen.detach.holdsWhileDetached.includes("detachedAgents") &&
        seen.detach.left.length === 0 && !seen.detach.onList && seen.detach.edgeGone,
      JSON.stringify(seen.detach),
    );
    ok(
      "pane lifecycle: the board's screen goes back without forgetting the card, and the shell's end forgets it",
      seen.preview.keptOnReturn && seen.preview.goneWithShell,
      JSON.stringify(seen.preview),
    );
    ok(
      "pane lifecycle: an older snapshot is refused by the fence of the shell observed, and the next shell at that id takes it",
      seen.fence.olderRefused && seen.fence.stateKept && seen.fence.nextShellTakes && seen.fence.left.length === 0,
      JSON.stringify(seen.fence),
    );
    ok(
      "pane lifecycle: a release that throws still ends the shell — the screen is dropped, the clock asked, every collection emptied — and the error is not swallowed",
      seen.failing.hadView && /release failed on purpose/.test(seen.failing.thrown ?? "") && seen.failing.screenGone &&
        seen.failing.clockAsked >= 1 && seen.failing.left.length === 0,
      JSON.stringify(seen.failing),
    );
    ok(
      "pane lifecycle: an effort answer held while its pane was closed, and the id taken again, says nothing to the new shell, which hears its own",
      seen.late.out === 1 && seen.late.closed && seen.late.sameId && seen.late.outAgain === 2 &&
        seen.late.strangerWritten === null && seen.late.mine === "mine" && seen.late.left.length === 0,
      JSON.stringify(seen.late),
    );
    ok(
      "pane lifecycle: a snapshot of every agent answered after a pane ended does not bring it back, even with a stamp that looks new, and applies the pane that stands and the agent no tab holds",
      seen.snapshot.out === 1 && seen.snapshot.closed && seen.snapshot.goneState === null && seen.snapshot.goneStamp === null &&
        seen.snapshot.goneFence === 0 && seen.snapshot.goneHolds.length === 0 &&
        seen.snapshot.standingState === "working" && seen.snapshot.backendOwnedState === "working",
      JSON.stringify(seen.snapshot),
    );
    ok(
      "pane lifecycle: the agents the backend lists at boot are written for the pane that stands and not for the one that ended while the question was out",
      seen.seed.goneAgent === null && seen.seed.goneHolds.length === 0 && seen.seed.standingAgent === "codex",
      JSON.stringify(seen.seed),
    );
    ok(
      "pane lifecycle: no question is left out once the window is at rest",
      seen.census.questionsOut === 0,
      JSON.stringify(seen.census),
    );
    ok(
      "pane lifecycle: after every pane here has ended, every declared collection is back to the count the window started with",
      seen.census.declared > 20 && seen.census.moved.length === 0,
      JSON.stringify(seen.census),
    );
    ok("pane lifecycle raised nothing the page left uncaught", seen.raised.length === 0 && faults.length === 0, `${seen.raised.join("\n")}${faults.join("\n")}`);
  } finally {
    await page.close();
  }
}

export async function testPanePreviewLifecycle(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    const seen = await page.evaluate(async () => {
      window.__HOLD_POLLERS__ = true;
      try {
        const before = paneLifecycle.sizes();
        for (let at = 0; at < 8; at += 1) {
          const term = 90_000 + at;
          await openBoardPeek({ pane: `term:${term}`, heading: `card ${at}`, agent: "claude" }, term);
          closeBoardPeek();
        }
        const after = paneLifecycle.sizes();
        const grew = Object.keys(after).filter((name) => after[name] !== before[name] && name !== "acknowledgedPanes");
        const owned = await openTermTab({ placement: "tab" });
        const tab = tabOfTerm(owned);
        const born = paneBorn.get(owned);
        paneModels.set(owned, "kept-model");
        paneSessions.set(owned, { agent: "claude", session: { key: "session_id", id: "kept-session" } });
        await openBoardPeek({ pane: `term:${owned}`, heading: "owned", agent: "claude" }, owned);
        closeBoardPeek();
        const kept = typeof born === "number" && paneBorn.get(owned) === born &&
          paneModels.get(owned) === "kept-model" && paneSessions.get(owned).session.id === "kept-session" &&
          acknowledgedPanes.has(`term:${owned}`);
        // A preview may become a real split while its dialog is still open.
        const adopted = 90_020;
        await openBoardPeek({ pane: `term:${adopted}`, heading: "adopted", agent: "claude" }, adopted);
        const preview = termViews.get(adopted);
        for (const listener of window.__LISTENERS__["term:split"] ?? []) {
          listener({ payload: { parent: owned, term: adopted, direction: "vertical", title: "adopted" } });
        }
        closeBoardPeek();
        const joined = tabOfTerm(adopted) === tab && termViews.get(adopted) === preview && typeof paneBorn.get(adopted) === "number";
        setConfirmCloseRunning(false);
        closeTab(tab.id);
        setConfirmCloseRunning(true);
        for (let at = 0; at < 100 && tabs.includes(tab); at += 1) await new Promise((done) => setTimeout(done, 20));
        return { grew, kept, joined, closed: !tabs.includes(tab), bornLeft: paneBorn.has(owned) || paneBorn.has(adopted) };
      } finally {
        window.__HOLD_POLLERS__ = false;
        window.__RELEASE_POLLERS__();
      }
    });
    ok("preview-only shells leave no pane metadata except their acknowledgement", seen.grew.length === 0, JSON.stringify(seen));
    ok("returning a borrowed screen preserves its owned pane's age, model and session", seen.kept, JSON.stringify(seen));
    ok("a preview adopted by a tab gets a pane lifetime and is forgotten when that tab closes",
      seen.joined && seen.closed && !seen.bornLeft, JSON.stringify(seen));
    ok("preview lifetime scenes raise no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}
