import { openWindowTestPage } from "./window-boot.mjs";

// A pane follows its agent into the worktree its process moved to. The
// backend's `term:cwd` is stubbed; the catalog carries a project with a child
// checkout under `.zo/worktrees/` and a second project whose path is a prefix
// of a stranger's.
export async function exercisePaneFollow() {
  const settle = (ms = 80) => new Promise((done) => setTimeout(done, ms));
  const catalog = window.__ANSWER__.project_catalog;
  const priorLocale = locale;
  locale = "en";
  window.__ANSWER__.project_catalog = () => [
    {
      name: "follow", path: "/tmp/follow",
      worktrees: [
        { path: "/tmp/follow", branch: "main", is_main: true, active: true, is_folder: false },
        { path: "/tmp/follow/.zo/worktrees/feat", branch: "feat", is_main: false, active: false, is_folder: false },
        { path: "/tmp/follow/.zo/worktrees/detached", branch: null, is_main: false, active: false, is_folder: false },
      ],
    },
    { name: "follow-b", path: "/tmp/follow-b", worktrees: [{ path: "/tmp/follow-b", branch: "main", is_main: true, active: false, is_folder: false }] },
  ];
  await refreshWorktrees(); await settle();
  paintWorktreeAgents();
  // 「에이전트 창 없음」 is said once, on the project's head with the count of its checkouts that hold no agent window
  // (t-84799 A1) — not again on every row.
  const head = () => document.querySelector('.proj-row[data-project-path="/tmp/follow"] .proj-unseated');
  const emptyExplained = head()?.hidden === false && head()?.textContent === "No agent window 3";
  const rowsSayNothing = document.querySelectorAll(".wt-row .wt-unseated").length === 0;
  locale = "ko"; applyLocale();
  paintWorktreeAgents();
  const captionLocalized = head()?.textContent === "에이전트 창 없음 3";
  mountTermTab(9401, { agent: "zo", worktree: "/tmp/follow" }, { placement: "tab", focus: false });
  paneAgents.set(9401, "zo");
  const fire = (cwd) => { for (const handler of window.__LISTENERS__["term:cwd"] ?? []) handler({ payload: { term: 9401, cwd } }); };
  const seen = { before: tabOfTerm(9401)?.worktree };
  fire("/tmp/follow/.zo/worktrees/feat/crates/zerocode-shell"); await settle(); await settle();
  seen.followed = tabOfTerm(9401)?.worktree;
  // The agent stands in main, then follows into feat: of the three checkouts, the ones without a window are main and
  // detached — the head counts again.
  seen.emptyCaptionGone = head()?.textContent === "에이전트 창 없음 2";
  seen.emptyExplained = emptyExplained;
  seen.detachedExplained = rowsSayNothing;
  seen.captionLocalized = captionLocalized;
  fire("/tmp/follow-bee/src"); await settle(); await settle();
  seen.prefixSafe = tabOfTerm(9401)?.worktree;
  fire("/tmp/follow/ui"); await settle(); await settle();
  seen.backToRoot = tabOfTerm(9401)?.worktree;
  seen.agentKept = paneAgents.get(9401) === "zo" && !!tabOfTerm(9401);
  // A plain shell is not an agent: a person cd-ing into another checkout must
  // not have the tab jump groups under them.
  mountTermTab(9402, { worktree: "/tmp/follow" }, { placement: "tab", focus: false });
  for (const handler of window.__LISTENERS__["term:cwd"] ?? []) handler({ payload: { term: 9402, cwd: "/tmp/follow/.zo/worktrees/feat" } });
  await settle(); await settle();
  seen.plainShellStays = tabOfTerm(9402)?.worktree === "/tmp/follow";
  window.__ANSWER__.project_catalog = catalog;
  locale = priorLocale; applyLocale();
  await refreshWorktrees(); await settle();
  return seen;
}

// The checkout an agent has only just cut inside the repository — Claude Code's
// `.claude/worktrees/<name>`, zo's `.zo/worktrees/<name>` — is one the window's
// list does not hold yet, while the repository's own root contains its path.
// The backend names the checkout the folder is in (`root`); a list that names
// another is read again instead of believed. And the pane in front of the
// person takes the window with it, there and back, when going takes nothing
// from them: the stage draws only the workspace in front, so a pane that
// follows alone leaves the person looking at another tab, their own pane a
// card away.
export async function exerciseFollowIntoAFreshCheckout() {
  const settle = (ms = 80) => new Promise((done) => setTimeout(done, ms));
  const tell = (term, cwd, root) => {
    const payload = { term, cwd, ...(root ? { root } : {}) };
    for (const handler of window.__LISTENERS__["term:cwd"] ?? []) handler({ payload });
  };
  const A = activeWorktreePath;
  const fresh = `${A}/.claude/worktrees/probe`;
  // A second checkout cut later, listed only from the moment the test says.
  const later = `${A}/.claude/worktrees/later`;
  let laterListed = false;
  const nested = `${A}/vendor/lib`;
  const catalog = window.__ANSWER__.project_catalog;
  const activating = window.__ANSWER__.set_active_worktree;
  // The backend's own rule: the checkout `set_active_worktree` chose is the
  // one the list marks active, and the window re-reads where it stands from
  // that mark.
  let standing = A;
  window.__ANSWER__.set_active_worktree = ({ path }) => { standing = path; return "main"; };
  const rows = (withFresh) => [{
    name: "zerocode", path: A,
    worktrees: [
      { path: A, branch: "main", is_main: true, active: standing === A, is_folder: false },
      ...(withFresh ? [{
        path: fresh, branch: "worktree-probe", is_main: false, active: standing === fresh, is_folder: false,
        ownership: "agent-scratch", external_hidden: true,
      }] : []),
      ...(laterListed ? [{
        path: later, branch: "worktree-later", is_main: false, active: standing === later, is_folder: false,
        ownership: "agent-scratch", external_hidden: true,
      }] : []),
    ],
  }];
  // The pane file as the backend keeps it: a save replaces a workspace's set
  // and names its tabs, and a read answers with what was saved. A pane
  // re-seated into a workspace nobody has stepped into is therefore IN that
  // workspace's stored set when the window first goes there.
  const paneFile = {};
  const saving = window.__ANSWER__.save_pane_layouts;
  const readingPanes = window.__ANSWER__.pane_layouts;
  window.__ANSWER__.save_pane_layouts = ({ worktree, layouts }) => {
    const held = paneFile[worktree] ?? [];
    let next = Math.max(0, ...Object.values(paneFile).flat().map((tab) => tab.id ?? 0), ...held.map((tab) => tab.id ?? 0), ...layouts.map((tab) => tab.id ?? 0)) + 1;
    const kept = layouts.map((tab) => ({ ...JSON.parse(JSON.stringify(tab)), id: tab.id ?? next++ }));
    if (kept.length === 0) delete paneFile[worktree];
    else paneFile[worktree] = kept;
    return kept.map((tab) => tab.id);
  };
  window.__ANSWER__.pane_layouts = ({ worktree }) => JSON.parse(JSON.stringify(paneFile[worktree] ?? []));
  let reads = 0;
  window.__ANSWER__.project_catalog = () => rows(false);
  setEveryProjectClosed(false);
  await refreshWorktrees(); await settle();
  // From here the repository has the checkout; the window's list does not
  // until it reads again.
  window.__ANSWER__.project_catalog = () => { reads += 1; return rows(true); };
  const listed = () => shownWorktrees(projects.find((one) => one.path === A)).some((row) => row.path === fresh);
  const seen = {};

  // A pane nobody is at: it follows, and the window stays where it is.
  mountTermTab(9411, { agent: "claude", worktree: A }, { placement: "tab", focus: false });
  paneAgents.set(9411, "claude");
  const frontBefore = activeTabId;
  tell(9411, `${fresh}/crates`, fresh); await settle(); await settle(); await settle();
  seen.fresh = {
    followed: tabOfTerm(9411)?.worktree ?? null,
    read: reads > 0,
    windowStayed: activeWorktreePath === A && activeTabId === frontBefore,
    listed: listed(),
  };
  // 「에이전트 스크래치 숨기기」 hides scratch nobody is in — not the checkout a
  // pane of this window is working in, whose card is the way back to it.
  const hideBefore = hideAgentScratchWorkspaces;
  hideAgentScratchWorkspaces = true;
  seen.hide = { whileAgentThere: listed() };
  tell(9411, A, A); await settle(); await settle();
  seen.hide.backHome = tabOfTerm(9411)?.worktree === A;
  seen.hide.afterAgentLeft = listed();
  hideAgentScratchWorkspaces = hideBefore;

  // A checkout the list will never hold — a repository nested in the project
  // — is asked about once an event and never followed into; a folder the
  // backend names no checkout for is not asked about at all.
  const before = reads;
  tell(9411, `${nested}/src`, nested); await settle(); await settle();
  const once = reads - before;
  tell(9411, `${A}/ui`); await settle(); await settle();
  seen.nested = { stays: tabOfTerm(9411)?.worktree === A, once, unnamedAsksNothing: reads - before === once };

  // The newest report about a pane is the one that places it: a report that
  // waits on a read of the list is dropped by the next one, which says the
  // process stands at home after all.
  // The first report names a checkout the list does not hold yet, so it
  // waits on a read — and that read WILL list the checkout: were the report
  // still waiting when the read answered, the pane would be moved there.
  tell(9411, `${later}/src`, later);
  laterListed = true;
  tell(9411, A, A);
  await settle(); await settle(); await settle();
  seen.newest = {
    listed: projects.some((one) => one.worktrees.some((row) => row.path === later)),
    home: tabOfTerm(9411)?.worktree === A,
    windowStayed: activeWorktreePath === A,
  };

  // The pane in front of the person: the window goes with it, and comes back
  // with it when the agent leaves the checkout. "In front" is what the
  // keyboard itself reads (`frontKeyboardTarget`): the tab the stage shows in
  // the focused leaf, and the pane the keyboard is in.
  mountTermTab(9412, { agent: "claude", worktree: A }, { placement: "tab", focus: true });
  paneAgents.set(9412, "claude");
  setActiveTab(tabOfTerm(9412).id);
  // What the trips must not add: a shell nobody asked for. A list read beside
  // the switch once answered with the checkout being left, and the stage
  // restored for THAT one gave it a first terminal.
  const shells = () => tabs.filter((one) => one.kind === "term").flatMap((one) => paneLeaves(one.layout)).length;
  const shellsBefore = shells();
  tell(9412, `${fresh}/src`, fresh); await settle(); await settle(); await settle(); await settle();
  const moved = tabOfTerm(9412);
  // The checkout is one the window has not stepped into, and its stored set
  // holds the pane that was just re-seated there. Putting that set back must
  // not stand a second tab for the one conversation.
  const there = tabs.filter((one) => one.kind === "term" && one.worktree === fresh);
  seen.went = {
    worktree: moved?.worktree ?? null,
    window: activeWorktreePath,
    keyboard: frontKeyboardTarget()?.term === 9412,
    onStage: activeTabIn(focusedPane)?.id === moved?.id && currentTab()?.id === moved?.id,
    stored: (paneFile[fresh] ?? []).length,
    oneTabThere: there.length === 1 && there[0].id === moved?.id,
    noneAsleep: !tabs.some((one) => one.asleep),
    noShellAdded: shells() === shellsBefore,
  };
  tell(9412, `${A}/ui`, A); await settle(); await settle(); await settle(); await settle();
  const home = tabOfTerm(9412);
  seen.cameBack = {
    worktree: home?.worktree ?? null,
    window: activeWorktreePath,
    keyboard: frontKeyboardTarget()?.term === 9412,
    onStage: activeTabIn(focusedPane)?.id === home?.id && currentTab()?.id === home?.id,
    noShellAdded: shells() === shellsBefore,
    oneTabHome: tabs.filter((one) => one.kind === "term" && paneLeaves(one.layout ?? null).includes(9412)).length === 1,
  };
  // A move of the person's own made while the window is on its way is the
  // later move, and it stays: the pane goes, the window does not follow it.
  mountTermTab(9414, { agent: "claude", worktree: A }, { placement: "tab", focus: true });
  paneAgents.set(9414, "claude");
  setActiveTab(tabOfTerm(9414).id);
  const handInFront = frontKeyboardTarget()?.term === 9414;
  tell(9414, `${fresh}/src`, fresh);
  // The press comes after the report, by the clock the window compares them
  // with — the two are one turn apart here, and the clock ticks coarsely.
  const reported = performance.now();
  while (performance.now() <= reported) { /* the next tick */ }
  noteHandMoved("pointerdown");
  await settle(); await settle(); await settle(); await settle();
  seen.handMoved = {
    inFront: handInFront,
    paneWent: tabOfTerm(9414)?.worktree === fresh,
    windowStayed: activeWorktreePath === A,
  };
  tell(9414, A, A); await settle(); await settle();
  closePaneLeaf(tabOfTerm(9414), 9414); await settle();

  // With a document on the stage the window does not go — going lets go of
  // every document, and nobody pressed anything. The pane leaves alone, as it
  // always has, the document stands, and the keyboard does not go with a pane
  // that is no longer in view.
  // Answered for as long as the document stands: a tab brought forward is
  // read again to see whether its file moved.
  const reading = window.__ANSWER__.read_text_file;
  window.__ANSWER__.read_text_file = () => ({ text: "read me", version: 1 });
  await openPath(`${A}/README.md`); await settle(); await settle();
  const paper = tabs.find(isDocument) ?? null;
  setActiveTab(tabOfTerm(9412).id);
  // The pane is in front before it goes — without that, "the window stayed"
  // would say nothing about the rule that holds it.
  const atKeyboard = frontKeyboardTarget()?.term === 9412;
  tell(9412, `${fresh}/src`, fresh); await settle(); await settle(); await settle();
  const away = tabOfTerm(9412);
  seen.withDocument = {
    opened: paper !== null,
    atKeyboard,
    paneWent: away?.worktree === fresh,
    windowStayed: activeWorktreePath === A,
    documentStands: paper !== null && tabs.includes(paper),
    keyboardNotWithIt: frontKeyboardTarget()?.term !== 9412 && activeTabIn(focusedPane)?.worktree === A,
  };
  // And a page covering the stage is not closed for it either.
  tell(9412, A, A); await settle(); await settle();
  if (paper) dropTab(paper.id);
  setActiveTab(tabOfTerm(9412).id);
  setSettingsOpen(true); await settle();
  // Under the page the keyboard reads no pane at all; the pane is still the
  // one the stage holds in the focused leaf.
  const underPage = activeTabIn(focusedPane)?.id === tabOfTerm(9412)?.id && activePaneOf(tabOfTerm(9412)) === 9412;
  tell(9412, `${fresh}/src`, fresh); await settle(); await settle(); await settle();
  seen.underSettings = {
    atKeyboard: underPage,
    paneWent: tabOfTerm(9412)?.worktree === fresh,
    windowStayed: activeWorktreePath === A,
    pageStands: !settingsView.hidden,
  };
  setSettingsOpen(false);
  // A split gives up the one pane and keeps the rest. With the window held
  // where it is (a document again), the tab that stayed is still the tab in
  // front, the keyboard on the pane beside the one that left.
  tell(9412, A, A); await settle(); await settle();
  await openPath(`${A}/README.md`); await settle(); await settle();
  const pair = tabOfTerm(9412);
  tileTermPane(pair, 9413, { agent: "claude" }, "right", false, 9412);
  paneAgents.set(9413, "claude");
  setActiveTab(pair.id);
  setActivePane(pair, 9412);
  const pairAtKeyboard = frontKeyboardTarget()?.term === 9412;
  tell(9412, `${fresh}/src`, fresh); await settle(); await settle(); await settle();
  seen.split = {
    atKeyboard: pairAtKeyboard,
    leafWent: tabOfTerm(9412)?.worktree === fresh && tabOfTerm(9412)?.id !== pair.id,
    restStayed: tabOfTerm(9413)?.id === pair.id && pair.worktree === A,
    windowStayed: activeWorktreePath === A,
    keyboardKept: activeTabIn(focusedPane)?.id === pair.id && frontKeyboardTarget()?.term === 9413,
  };
  const paperAgain = tabs.find(isDocument) ?? null;
  if (paperAgain) dropTab(paperAgain.id);
  if (reading) window.__ANSWER__.read_text_file = reading;
  else delete window.__ANSWER__.read_text_file;
  seen.paths = { A, fresh };
  window.__ANSWER__.project_catalog = catalog;
  if (activating) window.__ANSWER__.set_active_worktree = activating;
  else delete window.__ANSWER__.set_active_worktree;
  if (saving) window.__ANSWER__.save_pane_layouts = saving;
  else delete window.__ANSWER__.save_pane_layouts;
  if (readingPanes) window.__ANSWER__.pane_layouts = readingPanes;
  else delete window.__ANSWER__.pane_layouts;
  await refreshWorktrees(); await settle();
  return seen;
}

export async function testPaneFollowsCwd(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    const seen = await page.evaluate(exercisePaneFollow);
    ok(
      "a pane follows its agent into the checkout its process moved to — the deepest containing worktree, never a prefix sibling — and back; a plain shell stays put",
      seen.before === "/tmp/follow" &&
        seen.followed === "/tmp/follow/.zo/worktrees/feat" &&
        seen.prefixSafe === "/tmp/follow/.zo/worktrees/feat" &&
        seen.backToRoot === "/tmp/follow" &&
        seen.agentKept &&
        seen.plainShellStays && seen.emptyExplained && seen.emptyCaptionGone && seen.detachedExplained && seen.captionLocalized,
      JSON.stringify(seen),
    );
    ok("following a pane raised no renderer errors", faults.length === 0, faults.join("\n"));
  } finally { await page.close(); }
  const fresh = await openWindowTestPage(browser, origin);
  try {
    const seen = await fresh.page.evaluate(exerciseFollowIntoAFreshCheckout);
    const { A, fresh: checkout } = seen.paths ?? {};
    ok(
      "a pane follows its agent into a checkout cut a moment ago inside the repository — the list is read again when the backend names a checkout it does not hold",
      seen.fresh?.followed === checkout && seen.fresh.read && seen.fresh.windowStayed && seen.fresh.listed,
      JSON.stringify(seen.fresh),
    );
    ok(
      "hiding agent scratch keeps the card of a checkout a pane of this window is working in, and hides it once the pane has left",
      seen.hide?.whileAgentThere === true && seen.hide.backHome && seen.hide.afterAgentLeft === false,
      JSON.stringify(seen.hide),
    );
    ok(
      "a checkout the list never holds is asked about once an event and not followed into; a folder with no checkout named asks nothing",
      seen.nested?.stays && seen.nested.once === 1 && seen.nested.unnamedAsksNothing,
      JSON.stringify(seen.nested),
    );
    ok(
      "a report that waits on the list is replaced by the next one about the same pane",
      seen.newest?.listed && seen.newest.home && seen.newest.windowStayed,
      JSON.stringify(seen.newest),
    );
    ok(
      "the pane in front of the person takes the window with it into a checkout it has not stepped into — its own stored record not put back as a second tab — and back",
      seen.went?.worktree === checkout && seen.went.window === checkout && seen.went.keyboard && seen.went.onStage &&
        seen.went.stored === 1 && seen.went.oneTabThere && seen.went.noneAsleep && seen.went.noShellAdded &&
        seen.cameBack?.worktree === A && seen.cameBack.window === A && seen.cameBack.keyboard && seen.cameBack.onStage &&
        seen.cameBack.noShellAdded && seen.cameBack.oneTabHome,
      JSON.stringify({ went: seen.went, cameBack: seen.cameBack }),
    );
    ok(
      "a move of the person's own made while the window is on its way stays: the pane goes and the window does not follow",
      seen.handMoved?.inFront && seen.handMoved.paneWent && seen.handMoved.windowStayed,
      JSON.stringify(seen.handMoved),
    );
    ok(
      "with a document on the stage, or a page over it, the window stays: the pane goes alone, nothing is closed, and the keyboard does not go with it",
      seen.withDocument?.opened && seen.withDocument.atKeyboard && seen.withDocument.paneWent &&
        seen.withDocument.windowStayed && seen.withDocument.documentStands && seen.withDocument.keyboardNotWithIt &&
        seen.underSettings?.atKeyboard && seen.underSettings.paneWent && seen.underSettings.windowStayed &&
        seen.underSettings.pageStands,
      JSON.stringify({ withDocument: seen.withDocument, underSettings: seen.underSettings }),
    );
    ok(
      "a split gives up the pane that follows and keeps the rest in front, the keyboard on the pane beside it",
      seen.split?.atKeyboard && seen.split.leafWent && seen.split.restStayed && seen.split.windowStayed &&
        seen.split.keyboardKept,
      JSON.stringify(seen.split),
    );
    ok("following into a fresh checkout raised no renderer errors", fresh.faults.length === 0, fresh.faults.join("\n"));
  } finally { await fresh.page.close(); }
}
