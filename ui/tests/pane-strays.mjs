import { openWindowTestPage } from "./window-boot.mjs";

/* Panes that leave their tab, and the shells no tab and no row holds (t-56480).
 *
 * Reported 2026-10-15: 「이거 지금 보면 판이 안보임」 — a zo pane split beside
 * another zo pane left its tab while the person was in another project. Its
 * process kept running (the backend still held its pty), and nothing on screen
 * named it: no tab, no sidebar row, no number beside the 「+」, and not one line
 * in the window log saying which road took it. Three roads that do exactly
 * that were found by reading, and this suite walks each of them:
 *
 *   R1  `term:split` with no tab to land in — the leader is a pane somebody
 *       closed while it kept running, or its tab is gone;
 *   R2  a pane following its process into another checkout, when the tab is
 *       named after that pane (`term:<it>`) and still holds others;
 *   R3  a parked pane reopened from its row, into the same name.
 *
 * Then the two nets under every road: the slow road back (a live agent this
 * window drew, that no tab and no row holds, comes back as a row on the
 * clock's own `pane_agents` answer), the resource window's 열기 on a shell the
 * pool holds and the stage does not, and one window-log line per road
 * (`term N left tab T by <road> → <where>`) that carries no word a person
 * wrote. And the record a restart reads: when zo says its conversation moved
 * (/resume, /new — zo's SessionStart), the stored set names the new one.
 *
 * Every fixture is built from the window's own events and doors; no person's
 * pane, session or file is read. */
export async function exercisePaneStrays() {
  const settle = (ms = 60) => new Promise((done) => setTimeout(done, ms));
  const tell = (name, payload) => {
    for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload });
  };
  const seen = {};
  const A = activeWorktreePath;
  const B = `${A}/.zo/worktrees/strays-b`;
  const D = `${A}/.zo/worktrees/strays-d`;
  const C = "/tmp/strays-c";
  const priorAnswers = { ...window.__ANSWER__ };
  const priorPanes = window.__PANES__;
  window.__ANSWER__.project_catalog = () => [
    {
      name: "zerocode",
      path: A,
      worktrees: [
        { path: A, branch: "main", is_main: true, active: true, is_folder: false },
        { path: B, branch: "strays-b", is_main: false, active: false, is_folder: false },
        { path: D, branch: "strays-d", is_main: false, active: false, is_folder: false },
      ],
    },
    { name: "strays-c", path: C, worktrees: [{ path: C, branch: "main", is_main: true, active: false, is_folder: false }] },
  ];
  setEveryProjectClosed(false);
  await refreshWorktrees();
  await settle();
  const lines = [];
  window.__ANSWER__.log_window_error = (args) => {
    lines.push(String(args?.message ?? ""));
    return null;
  };
  const held = (term) => Boolean(tabOfTerm(term)) || detachedAgents.has(term);
  const invisible = (terms) => terms.filter((term) => !held(term)).length;
  const agentPane = (term, worktree) => {
    mountTermTab(term, { agent: "Zo", worktree }, { placement: "tab", focus: false });
    paneAgents.set(term, "zo");
    return tabOfTerm(term);
  };
  const plainPane = (term, worktree) => {
    mountTermTab(term, { worktree }, { placement: "tab", focus: false });
    return tabOfTerm(term);
  };
  const splitIn = (tab, term, agent = true) => {
    tileTermPane(tab, term, agent ? { agent: "Zo" } : {}, "right", false, paneLeaves(tab.layout)[0]);
    if (agent) paneAgents.set(term, "zo");
  };

  // ---- R1: a split with nowhere to land ----
  const r1 = agentPane(9700, A);
  splitIn(r1, 9701);
  closePaneLeaf(r1, 9701);
  const leaderParked = detachedAgents.has(9701);
  tell("term:split", { parent: 9701, term: 9702, direction: "vertical", agent: "zo" });
  tell("term:split", { parent: 9799, term: 9703, direction: "vertical", agent: "claude" });
  await settle();
  seen.r1 = {
    leaderParked,
    invisible: invisible([9702, 9703]),
    besideLeader: detachedAgents.get(9702) === A,
    childOfLeader: paneParents.get(9702) === 9701,
    noTabMade: !tabOfTerm(9702) && !tabOfTerm(9703),
    rowStands: worktreeAgentRows(A).some((row) => row.term === 9702 && row.detached),
  };

  // ---- R2: the pane a tab is named after follows its process away ----
  const r2 = agentPane(9710, A);
  splitIn(r2, 9711);
  const r2Id = r2.id;
  tell("term:cwd", { term: 9710, cwd: `${B}/src` });
  await settle();
  await settle();
  seen.r2 = {
    namedAfterIt: r2Id === "term:9710",
    movedTo: tabOfTerm(9710)?.worktree ?? null,
    ownTab: Boolean(tabOfTerm(9710)) && tabOfTerm(9710).id !== r2Id,
    leftStands: tabOfTerm(9711)?.id === r2Id && tabOfTerm(9711)?.worktree === A,
    invisible: invisible([9710, 9711]),
  };

  // ---- R3: a parked pane reopened from its row, into the name its old tab wears ----
  const r3 = agentPane(9720, A);
  splitIn(r3, 9721);
  const r3Id = r3.id;
  closePaneLeaf(r3, 9720);
  const parked = detachedAgents.has(9720) && tabOfTerm(9721)?.id === r3Id;
  await focusAgentPane(A, null, 9720, "zo");
  await settle();
  seen.r3 = {
    parked,
    reopened: Boolean(tabOfTerm(9720)) && !detachedAgents.has(9720),
    besideKept: tabOfTerm(9721)?.id === r3Id,
    twoTabs: Boolean(tabOfTerm(9720)) && tabOfTerm(9720)?.id !== tabOfTerm(9721)?.id,
    invisible: invisible([9720, 9721]),
  };

  // ---- the slow road back: a road nobody wrote down takes a pane and says nothing ----
  const lost = agentPane(9730, A);
  splitIn(lost, 9731);
  lost.layout = prunePanes(lost.layout, new Set([9730]));
  renderPanes(lost);
  window.__PANES__ = [9730, 9731].map((term) => ({
    term, agent: "zo", state: "done", hearing: "heard", at: 0, resumable: false,
  }));
  const lostBefore = invisible([9731]);
  // One answer alone is a moment, not a stray: a tab may be standing up. (A
  // window without the slow road has neither name; it then fails below, at the
  // checks, rather than here.)
  if (typeof strandSightings !== "undefined") strandSightings.clear();
  if (typeof adoptStrandedAgents === "function") adoptStrandedAgents(window.__PANES__);
  const afterOneAnswer = held(9731);
  // The clock's own road: its `pane_agents` answer is the second sighting.
  for (let tries = 0; tries < 3 && !held(9731); tries += 1) {
    await refreshPaneAgentSnapshot();
    await settle(20);
  }
  await window.__PAINTED__();
  const lostAfter = invisible([9731]);
  const row = worktreeAgentRows(A).find((one) => one.term === 9731);
  const rowNode = document.querySelector('.wt-agent[data-term="9731"]');
  if (rowNode) rowNode.click();
  else if (row) await focusAgentPane(row.worktree, null, 9731, "zo");
  await settle();
  seen.slow = {
    before: lostBefore,
    afterOneAnswer,
    after: lostAfter,
    row: row ? { detached: row.detached === true, worktree: row.worktree } : null,
    rowDrawn: Boolean(rowNode),
    reopened: Boolean(tabOfTerm(9731)) && !detachedAgents.has(9731),
  };
  window.__PANES__ = priorPanes;

  // What the slow road costs a beat: the scan over fifty tabs of four panes and
  // a two-hundred-agent answer, the tabs standing only for the length of the loop.
  const bench = Array.from({ length: 50 }, (_, n) => ({
    id: `strays-bench-${n}`,
    kind: "term",
    worktree: A,
    layout: {
      type: "split",
      direction: "vertical",
      first: { type: "split", direction: "horizontal", first: paneLeaf(20000 + n * 4), second: paneLeaf(20001 + n * 4) },
      second: { type: "split", direction: "horizontal", first: paneLeaf(20002 + n * 4), second: paneLeaf(20003 + n * 4) },
    },
  }));
  const benchPanes = Array.from({ length: 200 }, (_, n) => ({ term: 20000 + n, agent: "zo", state: "working", hearing: "heard" }));
  if (typeof strandedAgentTerms === "function") {
    tabs.push(...bench);
    const runs = 2000;
    const started = performance.now();
    let found = 0;
    for (let n = 0; n < runs; n += 1) found += strandedAgentTerms(benchPanes).length;
    const spent = performance.now() - started;
    for (const one of bench) tabs.splice(tabs.indexOf(one), 1);
    seen.scanCost = { microsPerBeat: Number(((spent / runs) * 1000).toFixed(2)), found };
  } else {
    seen.scanCost = { microsPerBeat: null, found: null };
  }

  // ---- the resource window: a shell of the pool that the stage lost ----
  const plain = plainPane(9740, A);
  splitIn(plain, 9741, false);
  plain.layout = prunePanes(plain.layout, new Set([9740]));
  renderPanes(plain);
  const priorSessions = resourceManager.sessions;
  resourceManager.sessions = [{ term: 9741, running: true, agent: null, program: "zsh" }];
  const node = resourceSessionNode({
    key: "term:9741", kind: "term", term: 9741, label: "terminal 9741", bound: false, cpu: null, memory: null,
  }, A);
  const door = node.querySelector(".resource-session-open");
  const lostPlain = invisible([9741]);
  door?.click();
  await settle();
  resourceManager.sessions = priorSessions;
  seen.resource = {
    door: door?.tagName ?? null,
    said: door?.getAttribute("aria-label") ?? null,
    before: lostPlain,
    reopened: Boolean(tabOfTerm(9741)),
  };

  // ---- the window log: every road, one line a pane, no word a person wrote ----
  const PERSON = "사람이-쓴-글";
  const roads = [];
  const walk = async (road, expected, step) => {
    const from = lines.length;
    await step();
    await settle();
    const said = lines.slice(from);
    roads.push({
      road,
      expected,
      said: said.length,
      fits: said.length === expected && said.every((line) => line.includes(` by ${road} → `)),
    });
  };
  const worded = (tab, term) => {
    tab.paneTitles = { ...(tab.paneTitles ?? {}), [term]: `${PERSON}-title` };
    panePrompts.set(term, `${PERSON}-prompt`);
    noteTermTitle(term, `${PERSON}-osc`);
  };

  const t1 = agentPane(9800, A);
  splitIn(t1, 9801);
  worded(t1, 9801);
  await walk("close-pane", 1, () => closePaneLeaf(t1, 9801));
  const t2 = plainPane(9802, A);
  splitIn(t2, 9803, false);
  worded(t2, 9803);
  await walk("close-pane", 1, () => closePaneLeaf(t2, 9803));
  const t3 = agentPane(9804, A);
  splitIn(t3, 9805);
  worded(t3, 9805);
  await walk("follow-cwd", 1, () => tell("term:cwd", { term: 9805, cwd: B }));
  agentPane(9806, A);
  await walk("follow-cwd", 1, () => tell("term:cwd", { term: 9806, cwd: `${B}/ui` }));
  const t5 = agentPane(9807, A);
  splitIn(t5, 9808);
  await walk("seat-ledger", 1, () =>
    tell("term:worker", { parent: 9807, term: 9808, worktree: B, agent: "claude" }));
  agentPane(9809, A);
  agentPane(9810, A);
  await walk("seat-answer", 1, () => reseatWorkerByAnswer(9809, 9810, A, "claude", "background"));
  agentPane(9811, A);
  agentPane(9812, A);
  await walk("seat-answer", 1, () => reseatWorkerByAnswer(9811, 9812, A, "claude", "split"));
  agentPane(9813, A);
  tell("term:split", { parent: 9813, term: 9814, direction: "vertical", agent: "zo" });
  tell("term:split", { parent: 9813, term: 9815, direction: "horizontal", agent: "zo" });
  await settle();
  await walk("park-team", 2, () =>
    tell("term:split", { parent: 9813, term: 9816, direction: "horizontal", agent: "zo" }));
  revealListedWorker(9814, A, "zo");
  await settle();
  await walk("reveal-listed", 1, () => revealListedWorker(9815, A, "zo"));
  const t9 = agentPane(9817, A);
  splitIn(t9, 9818);
  await walk("exit", 1, () => tell("term:exited", { term: 9818 }));
  const t10 = plainPane(9819, A);
  splitIn(t10, 9820, false);
  await walk("close-tab", 2, () => closeTab(t10.id));
  plainPane(9821, B);
  const inB = tabs
    .filter((tab) => tab.kind === "term" && tab.worktree === B)
    .reduce((sum, tab) => sum + paneLeaves(tab.layout).length, 0);
  await walk("worktree-removed", inB, () => dropWorktreeSurfaces(B));
  plainPane(9822, C);
  await walk("project-removed", 1, async () => {
    const removing = removeProjectFromList(C);
    await settle();
    document.getElementById("ask-yes").click();
    await removing;
  });
  const t13 = agentPane(9823, A);
  let mirrors = 9824;
  window.__ANSWER__.open_mirror_term = () => mirrors++;
  await openMirrorPane(9823, "codex", "/tmp/strays-mirror/run.log");
  await walk("mirror-replaced", 1, () => openMirrorPane(9823, "codex", "/tmp/strays-mirror/run.log"));
  seen.mirrorLeaves = paneLeaves(t13.layout);

  const shape =
    /^term \d+ (left tab \S+|found no tab) by [a-z-]+ → (closed|exited|detached \S+|tab \S+ in \S+|worktree \S+)$/;
  seen.log = {
    roads,
    lines: lines.length,
    foundNoTab: {
      split: lines.filter((line) => line.includes(" found no tab by split → ")).length,
      snapshot: lines.filter((line) => line.includes(" found no tab by snapshot → ")).length,
      resource: lines.filter((line) => line.includes(" found no tab by resource → ")).length,
    },
    shaped: lines.every((line) => shape.test(line)),
    misshaped: lines.filter((line) => !shape.test(line)).slice(0, 3),
    noPersonWords: lines.every((line) => !line.includes(PERSON)),
    sample: lines.slice(0, 4),
  };

  // ---- the record a restart reads, after zo moved its conversation ----
  const OLD = "session-1791959770709-3";
  const NEW = "session-1792033054702-0";
  const saves = [];
  window.__ANSWER__.save_pane_layouts = (args) => {
    saves.push(JSON.parse(JSON.stringify(args)));
    return null;
  };
  const woken = [];
  let nextWake = 9900;
  window.__ANSWER__.resume_session = (args) => {
    woken.push(args.session.id);
    nextWake += 1;
    return { term: nextWake, standing: false };
  };
  const record = (id) => ({
    id: 1,
    root: { type: "leaf" },
    agents: { 0: { agent: "zo", key: "session_id", id } },
    running: { 0: "zo" },
    focused: true,
  });
  const wakeAll = async (worktree) => {
    for (const tab of tabs.filter((one) => one.worktree === worktree && one.asleep)) await wakeStoredTab(tab);
    await storedWakesSettled(worktree);
  };
  await restoreWorktreeLayouts(D, [record(OLD)]);
  await wakeAll(D);
  const zoTerm = nextWake;
  const zoTab = tabOfTerm(zoTerm);
  const savesBefore = saves.length;
  tell("hook:agent", {
    term: zoTerm,
    state: "done",
    agent: "zo",
    resumable: true,
    session_boundary: true,
    session: { key: "session_id", id: NEW, transcript_path: `/tmp/strays-zo/${NEW}.jsonl` },
  });
  await settle();
  const stored = saves.slice(savesBefore).filter((save) => save.worktree === D).at(-1);
  const kept = stored?.layouts?.find((layout) => layout.agents?.[0]?.agent === "zo") ?? null;
  // The 10-15 scene's other half, step by step: a zo split beside the restored
  // pane, the checkout left behind, and the split's process saying where it
  // stands — the same folder, then another checkout. Nothing may be left with
  // no tab and no row.
  if (zoTab) {
    splitIn(zoTab, 9950);
    tell("term:cwd", { term: 9950, cwd: D });
    await settle();
    const stayed = tabOfTerm(9950)?.id === zoTab.id;
    tell("term:cwd", { term: 9950, cwd: `${B}/deep` });
    await settle();
    await settle();
    seen.scene = {
      tabNamed: zoTab.id,
      sameFolderStays: stayed,
      otherCheckoutMoves: tabOfTerm(9950)?.worktree ?? null,
      restoredStays: tabOfTerm(zoTerm)?.id === zoTab.id,
      invisible: invisible([zoTerm, 9950]),
    };
  }
  // The restart: the window's tab goes, the file's record comes back.
  if (zoTab) dropTab(zoTab.id);
  const wokenBefore = woken.length;
  if (kept) {
    await restoreWorktreeLayouts(D, [kept]);
    await wakeAll(D);
  }
  seen.record = {
    first: woken[0] ?? null,
    stored: kept?.agents?.[0]?.id ?? null,
    restartResumes: woken.slice(wokenBefore),
  };

  Object.assign(window.__ANSWER__, priorAnswers);
  for (const key of Object.keys(window.__ANSWER__)) if (!(key in priorAnswers)) delete window.__ANSWER__[key];
  await refreshWorktrees();
  return seen;
}

export async function testPaneStrays(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.waitForFunction(() => projectsRead);
    const seen = await page.evaluate(exercisePaneStrays);
    const invisibleAfter = seen.r1.invisible + seen.r2.invisible + seen.r3.invisible + seen.slow.after;
    console.log(
      `MEASURE live panes left with no tab and no row by the four scenes (R1's two teammates, R2, R3, a road nobody logged): ${invisibleAfter}`,
    );
    console.log(`MEASURE the slow road's scan: ${seen.scanCost.microsPerBeat} µs a beat over 200 panes and 200 agents`);
    console.log(`MEASURE window-log lines: ${seen.log.lines}; roads walked ${seen.log.roads.length}`);
    ok(
      "R1: a teammate whose leader has no tab stands as a row beside its leader — a closed-but-running leader's checkout, else the one in front — and no tab is made",
      seen.r1.leaderParked && seen.r1.invisible === 0 && seen.r1.besideLeader && seen.r1.childOfLeader &&
        seen.r1.noTabMade && seen.r1.rowStands,
      JSON.stringify(seen.r1),
    );
    ok(
      "R2: the pane a split tab is named after follows its process into another checkout as a tab of its own, and the pane left behind keeps the tab",
      seen.r2.namedAfterIt && seen.r2.movedTo?.endsWith("/strays-b") && seen.r2.ownTab && seen.r2.leftStands &&
        seen.r2.invisible === 0,
      JSON.stringify(seen.r2),
    );
    ok(
      "R3: a parked pane reopened from its row takes a tab of its own, and the pane still in the tab named after it stays there",
      seen.r3.parked && seen.r3.reopened && seen.r3.besideKept && seen.r3.twoTabs && seen.r3.invisible === 0,
      JSON.stringify(seen.r3),
    );
    ok(
      "a live agent no tab and no row holds comes back as a row on the clock's second answer, and its row reopens it",
      seen.slow.before === 1 && seen.slow.afterOneAnswer === false && seen.slow.after === 0 &&
        seen.slow.row?.detached === true && seen.slow.reopened,
      JSON.stringify(seen.slow),
    );
    ok(
      "the slow road's scan costs a beat microseconds, not frames",
      seen.scanCost.microsPerBeat !== null && seen.scanCost.microsPerBeat < 1000 && seen.scanCost.found === 0,
      JSON.stringify(seen.scanCost),
    );
    ok(
      "the resource window opens a shell of the pool that no tab and no row holds",
      seen.resource.before === 1 && seen.resource.door === "BUTTON" && Boolean(seen.resource.said) && seen.resource.reopened,
      JSON.stringify(seen.resource),
    );
    ok(
      "every road a pane leaves its tab by writes one window-log line a pane, in one shape, with no word a person wrote",
      seen.log.roads.length === 14 && seen.log.roads.every((road) => road.fits) &&
        seen.log.foundNoTab.split === 2 && seen.log.foundNoTab.snapshot === 1 && seen.log.foundNoTab.resource === 1 &&
        seen.log.shaped && seen.log.noPersonWords,
      JSON.stringify(seen.log),
    );
    ok(
      "the 10-15 scene's steps leave no live pane without a tab or a row: the same folder keeps the split, another checkout moves it to a tab of its own",
      seen.scene?.sameFolderStays === true && seen.scene.otherCheckoutMoves?.endsWith("/strays-b") &&
        seen.scene.restoredStays && seen.scene.invisible === 0,
      JSON.stringify(seen.scene ?? null),
    );
    ok(
      "zo's word that its conversation moved reaches the stored record, and the restart resumes the conversation zo named last",
      seen.record.first === "session-1791959770709-3" && seen.record.stored === "session-1792033054702-0" &&
        JSON.stringify(seen.record.restartResumes) === JSON.stringify(["session-1792033054702-0"]),
      JSON.stringify(seen.record),
    );
    ok("walking the roads raised no renderer errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}
