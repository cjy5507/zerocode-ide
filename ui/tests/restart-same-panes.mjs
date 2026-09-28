import { openWindowTestPage } from "./window-boot.mjs";

/* A restart opens the panes that were open, where they were (t-14036).
 *
 * Asked for in so many words on 2026-09-29, after the 07:08 restart: the
 * projects and the panes in them are remembered and come back the same. Three
 * things did not. A terminal tab's place on the stage was never written — the
 * stage record holds documents only, so a group holding nothing but terminals
 * folded away and every terminal came back in one group, after the documents.
 * The restore then put the eye on a terminal even when it had been on a
 * document. And a workspace other than the one in front stayed asleep until
 * somebody clicked it, conversations that were running at the restart
 * included.
 *
 * Two pages are the two windows. The first arranges a workspace and saves it
 * through the window's own doors into a stub that answers the way the file's
 * one writer does (every tab named, the name handed back); the second opens
 * from what the first saved, with two more workspaces whose conversations the
 * exit found running — and one it found asleep. Nothing here is a real
 * project: every path is synthetic. */

const A = "/tmp/restart-same/a";
const B = "/tmp/restart-same/b";
const C = "/tmp/restart-same/c";

/* The backend both windows stand on: the pane file's one writer (a tab
 * without a name is given the next one, and the names go back to the
 * window), the stage file as sent, and a resume that answers after a beat. */
function standBackend({ panes, stage, resumeMs }) {
  const settle = (ms) => new Promise((done) => setTimeout(done, ms));
  window.__RESTART__ = { panes, stage, resumes: [], shells: 0 };
  let nextTerm = 7700;
  window.__ANSWER__.save_pane_layouts = ({ worktree, layouts }) => {
    const held = window.__RESTART__.panes[worktree] ?? [];
    let next = Math.max(0, ...held.map((tab) => tab.id ?? 0), ...layouts.map((tab) => tab.id ?? 0)) + 1;
    const kept = layouts.map((tab) => ({ ...JSON.parse(JSON.stringify(tab)), id: tab.id ?? next++ }));
    if (kept.length === 0) delete window.__RESTART__.panes[worktree];
    else window.__RESTART__.panes[worktree] = kept;
    return kept.map((tab) => tab.id);
  };
  window.__ANSWER__.pane_layouts = ({ worktree }) =>
    JSON.parse(JSON.stringify(window.__RESTART__.panes[worktree] ?? []));
  window.__ANSWER__.save_stage_layouts = ({ worktree, layout }) => {
    window.__RESTART__.stage[worktree] = JSON.parse(JSON.stringify(layout));
    return null;
  };
  window.__ANSWER__.stage_layouts = ({ worktree }) =>
    JSON.parse(JSON.stringify(window.__RESTART__.stage[worktree] ?? null));
  // The workspaces whose conversations the exit found running.
  window.__ANSWER__.standing_pane_worktrees = () =>
    Object.entries(window.__RESTART__.panes)
      .filter(([, set]) => set.some((tab) => tab.awake && (tab.agents || tab.running)))
      .map(([worktree]) => worktree)
      .sort();
  window.__ANSWER__.resume_session = async (args) => {
    await settle(resumeMs);
    window.__RESTART__.resumes.push({ id: args.session.id, worktree: args.worktree ?? null, at: performance.now() });
    return { term: (nextTerm += 1), standing: false };
  };
  window.__ANSWER__.open_term_tab = () => {
    window.__RESTART__.shells += 1;
    return (nextTerm += 1);
  };
  window.__ANSWER__.default_tabs = () => ({ tabs: [], applied: true });
  window.__ANSWER__.worktree_last_agent = () => null;
}

/* What one workspace looks like: its groups in the stage's walking order,
 * each the strip it draws — a document by its kind, a terminal by the name
 * the file knows it by — and where the eye is. */
function lookOf(worktree) {
  const tree = worktree === activeWorktreePath ? stageTree() : stageTrees.get(worktree);
  const groupsHere = tree ? stageGroups(tree) : [];
  const said = (tab) => (tab.kind === "term" ? `#${tab.storedId ?? "?"}` : tab.kind);
  const strips = groupsHere.map((group) =>
    tabs.filter((tab) => tab.worktree === worktree && tab.pane === group).map(said));
  const eye = tabs.find((tab) => tab.id === activeTabByWorktree.get(worktree));
  return {
    strips,
    eye: eye ? said(eye) : null,
    eyeGroup: eye ? groupsHere.indexOf(eye.pane) : -1,
  };
}

const record = (id, agent, session, extra = {}) => ({
  id,
  root: { type: "leaf" },
  ...(agent && { agents: { 0: { agent, key: "session_id", id: session } }, running: { 0: agent } }),
  ...extra,
});

export async function testRestartSamePanes(browser, origin, ok) {
  const settle = (ms = 60) => new Promise((done) => setTimeout(done, ms));
  const session = (n) => `session-17906000000${n}-0`;

  // ---- the window before the restart ----
  const before = await openWindowTestPage(browser, origin);
  let saved;
  let looked;
  try {
    ({ saved, looked } = await before.page.evaluate(
      async ({ B, stored, standBackendSource, lookOfSource }) => {
        const settle = (ms = 60) => new Promise((done) => setTimeout(done, ms));
        eval(`(${standBackendSource})`)({ panes: { [B]: stored }, stage: {}, resumeMs: 5 });
        const look = eval(`(${lookOfSource})`);
        activeWorktreePath = B;
        focusedPane = stageGroups()[0];
        await restoreActiveWorktreeTab();
        await storedWakesSettled(B);
        await reopenStageTab({ kind: "board" });
        const board = tabs.find((tab) => tab.worktree === B && tab.kind === "board");
        const byId = (id) => tabs.find((tab) => tab.worktree === B && tab.storedId === id);
        // The stage divided: the board and the zo conversation on the left,
        // the claude and codex conversations on the right.
        const left = board.pane;
        const right = nextGroupId;
        nextGroupId += 1;
        setStageTree(splitStageLeaf(stageTree(), left, right, SPLIT_DIRECTION.right, SPLIT_POSITION.right));
        byId(2).pane = right;
        byId(4).pane = right;
        // A plain shell opened on the left after the board, as a tab.
        setActiveTab(board.id);
        await openTermTab({ placement: "tab", door: "terminal" });
        await settle();
        // The codex tab is closed before the window goes: closed stays closed.
        dropTab(byId(4).id, { closed: true });
        // The eye on the board — a document — on the left.
        setActiveTab(board.id);
        renderTabs();
        updateStage();
        persistPaneLayouts(B);
        persistStageLayouts({ worktree: B });
        await settle(200);
        return { saved: window.__RESTART__, looked: look(B) };
      },
      {
        B,
        stored: [
          record(1, "zo", session(1), { focused: true }),
          record(2, "claude", session(2)),
          record(3, null, null),
          record(4, "codex", session(4)),
        ],
        standBackendSource: standBackend.toString(),
        lookOfSource: lookOf.toString(),
      },
    ));
  } finally {
    await before.page.close();
  }

  // ---- the window after it ----
  const after = await openWindowTestPage(browser, origin);
  try {
    const seen = await after.page.evaluate(
      async ({ A, B, C, saved, standBackendSource, lookOfSource, resumeMs }) => {
        const session = (n) => `session-17906000000${n}-0`;
        const settle = (ms = 60) => new Promise((done) => setTimeout(done, ms));
        const panes = {
          ...saved.panes,
          // What the exit leaves for two workspaces nobody was looking at:
          // A ran one conversation and held another asleep; C ran codex
          // beside a plain shell.
          [A]: [
            { id: 1, root: { type: "leaf" }, agents: { 0: { agent: "zo", key: "session_id", id: session(11) } }, running: { 0: "zo" }, focused: true, awake: true },
            { id: 2, root: { type: "leaf" }, agents: { 0: { agent: "claude", key: "session_id", id: session(12) } }, running: { 0: "claude" } },
          ],
          [C]: [
            { id: 1, root: { type: "leaf" }, agents: { 0: { agent: "codex", key: "session_id", id: session(21) } }, running: { 0: "codex" }, focused: true, awake: true },
            { id: 2, root: { type: "leaf" }, awake: true },
          ],
        };
        eval(`(${standBackendSource})`)({ panes, stage: saved.stage, resumeMs });
        const look = eval(`(${lookOfSource})`);
        const heapBefore = performance.memory?.usedJSHeapSize ?? 0;
        const started = performance.now();
        activeWorktreePath = B;
        focusedPane = stageGroups()[0];
        await restoreActiveWorktreeTab();
        const frontEye = tabs.find((tab) => tab.id === activeTabId);
        const frontUsable = performance.now() - started;
        // The person steps into C before anything behind the front reached
        // it, and opens a shell of their own there.
        activeWorktreePath = C;
        await restoreActiveWorktreeTab();
        await openTermTab({ placement: "tab", door: "terminal" });
        await storedWakesSettled(C);
        activeWorktreePath = B;
        await restoreActiveWorktreeTab();
        // What the boot starts behind the front, when this window has it.
        if (typeof restoreStandingWorkspaces === "function") await restoreStandingWorkspaces();
        for (const worktree of [A, B, C]) await storedWakesSettled(worktree);
        await settle();
        const resumed = [...window.__RESTART__.resumes];
        const allBack = resumed.length > 0 ? Math.max(...resumed.map((one) => one.at)) - started : null;
        const heapAfter = performance.memory?.usedJSHeapSize ?? 0;
        const lookB = look(B);
        const cTabs = tabs.filter((tab) => tab.worktree === C).length;
        // And stepping into A later finds its set already there, once.
        activeWorktreePath = A;
        await restoreActiveWorktreeTab();
        await storedWakesSettled(A);
        const aTabs = tabs.filter((tab) => tab.worktree === A).length;
        const aEye = tabs.find((tab) => tab.id === activeTabId);
        const onceMore = window.__RESTART__.resumes.filter((one) => one.id === session(11)).length;
        return {
          lookB,
          frontEye: frontEye?.storedId ?? null,
          frontUsable: Math.round(frontUsable),
          allBack: allBack === null ? null : Math.round(allBack),
          resumed: resumed.map((one) => `${one.id}@${one.worktree}`),
          cTabs,
          aTabs,
          aEye: aEye?.storedId ?? null,
          onceMore,
          heapKb: Math.round((heapAfter - heapBefore) / 1024),
        };
      },
      { A, B, C, saved, standBackendSource: standBackend.toString(), lookOfSource: lookOf.toString(), resumeMs: 40 },
    );
    // C's codex is back because the person stepped into C; the other three
    // came back with nobody touching anything.
    const want = [`${session(1)}@${B}`, `${session(2)}@${B}`, `${session(11)}@${A}`, `${session(21)}@${C}`];
    const back = want.filter((one) => seen.resumed.filter((said) => said === one).length === 1).length;
    console.log(`MEASURE conversations running at the restart back before anybody touched their workspace: ${back - 1}/${want.length - 1}`);
    console.log(`MEASURE restart to every running conversation back: ${seen.allBack ?? "never"} ms; front workspace usable: ${seen.frontUsable} ms; heap +${seen.heapKb} KiB`);
    ok(
      "every terminal comes back in the stage group it stood in, in the same strip order, beside the documents",
      JSON.stringify(seen.lookB.strips) === JSON.stringify(looked.strips) && !JSON.stringify(looked.strips).includes("#?"),
      JSON.stringify({ before: looked.strips, after: seen.lookB.strips }),
    );
    ok(
      "the eye comes back on the tab and in the group it was on",
      seen.lookB.eye === looked.eye && seen.lookB.eyeGroup === looked.eyeGroup,
      JSON.stringify({ before: looked, after: seen.lookB }),
    );
    ok(
      "every conversation running at the restart comes back without a click, once, in its own workspace — the one asleep at the restart and the closed one stay as they were",
      back === want.length &&
        seen.resumed.length === want.length &&
        !seen.resumed.some((one) => one.startsWith(session(12)) || one.startsWith(session(4))),
      JSON.stringify(seen.resumed),
    );
    ok(
      "a workspace the person opened first keeps what they opened, and a workspace put back behind the front is not put back twice",
      seen.cTabs === 3 && seen.aTabs === 2 && seen.aEye === 1 && seen.onceMore === 1,
      JSON.stringify({ cTabs: seen.cTabs, aTabs: seen.aTabs, aEye: seen.aEye, onceMore: seen.onceMore }),
    );
    ok("restoring the panes raised no renderer errors", after.faults.length === 0 && before.faults.length === 0, [...before.faults, ...after.faults].join("\n"));
  } finally {
    await after.page.close();
  }
}
