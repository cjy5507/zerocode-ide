import { openWindowTestPage } from "./window-boot.mjs";

/* A finished worker's conversation does not come back on its own (t-57717).
 *
 * On 2026-10-15 and 16 three finished workers' conversations came back as
 * agent panes nobody had called, each on the first visit to its checkout
 * after a restart: the window read the conversation off its stored layout and
 * resumed it, and the log line said neither the road it came by nor whose it
 * was. The backend now answers a stored-record road's wake of such a
 * conversation with `finished` and starts nothing; this is the window's half.
 *
 *   1. Every wake names its road to `resume_session`: the boot's restore of
 *      the workspace in front is `restore-front`, its put-back behind the
 *      front `restore-behind`, a later first visit `visit`, a sleeping tab a
 *      person picks again `stage`, and a person's pick by name `reopen`.
 *   2. A `finished` answer stands a plain shell in the leaf, says so on its
 *      screen, starts no agent fresh, and carries nothing — the next save
 *      writes the leaf without the conversation.
 *   3. A workspace whose stored set the backend answered empty (the finished
 *      worker's tab struck) resumes nothing on its first visit and opens the
 *      way a workspace that stored nothing does.
 *
 * The backend stands before the window boots, so the boot's own restores are
 * the ones measured. Every path is synthetic. */

/* The workspace the harness boots in front (`project_root`), and four more. */
const FRONT = "/tmp/zerocode-window-test";
const BEHIND = "/tmp/finished-resume/behind";
const VISITED = "/tmp/finished-resume/visited";
const FINISHED = "/tmp/finished-resume/finished";
const EMPTIED = "/tmp/finished-resume/emptied";

const session = (n) => `57717a0c-0000-4000-8000-00000000000${n}`;
const IDS = {
  front: session(1),
  behind: session(2),
  visited: session(3),
  refused: session(4),
  finished: session(5),
  reopened: session(6),
};

/* The backend the page stands on: the pane file as a map, every resume and
 * every shell written down, the finished conversation answered as the backend
 * now answers it, one wake that refuses (and whose fallback shell fails once,
 * so its tab stays asleep for a person to pick), and the boot's put-back
 * behind the front naming one workspace. */
function standBackend({ panes, ids, standing }) {
  window.__FINISHED__ = { resumes: [], shells: [], saves: [], lastAgentAsks: 0, launches: 0 };
  let nextTerm = 9100;
  let refuseOnce = true;
  window.__ANSWER__.pane_layouts = ({ worktree }) => JSON.parse(JSON.stringify(panes[worktree] ?? []));
  window.__ANSWER__.save_pane_layouts = ({ worktree, layouts }) => {
    window.__FINISHED__.saves.push({ worktree, layouts: JSON.parse(JSON.stringify(layouts)) });
    return layouts.map((tab, at) => tab.id ?? 100 + at);
  };
  window.__ANSWER__.standing_pane_worktrees = () => standing;
  window.__ANSWER__.stage_layouts = () => null;
  window.__ANSWER__.save_stage_layouts = () => null;
  window.__ANSWER__.default_tabs = () => ({ tabs: [], applied: true });
  window.__ANSWER__.worktree_last_agent = () => {
    window.__FINISHED__.lastAgentAsks += 1;
    return null;
  };
  window.__ANSWER__.resume_session = async (args) => {
    window.__FINISHED__.resumes.push({ id: args.session.id, road: args.road ?? null, worktree: args.worktree ?? null });
    if (args.session.id === ids.refused && refuseOnce) throw new Error("the vendor dropped the session");
    if (args.session.id === ids.finished) return { term: (nextTerm += 1), standing: false, finished: "w-57717" };
    return { term: (nextTerm += 1), standing: false };
  };
  window.__ANSWER__.open_term_tab = (args) => {
    if (refuseOnce && typeof args?.notice === "string" && args.notice.includes("이어서 열지 못했습니다")) {
      refuseOnce = false;
      throw new Error("no shell this once");
    }
    window.__FINISHED__.shells.push({ plain: args?.plain === true, notice: args?.notice ?? null });
    return (nextTerm += 1);
  };
  window.__ANSWER__.launch_agent_tab = () => {
    window.__FINISHED__.launches += 1;
    return (nextTerm += 1);
  };
}

const record = (id, agent, conversation, extra = {}) => ({
  id,
  root: { type: "leaf" },
  agents: { 0: { agent, key: "session_id", id: conversation } },
  running: { 0: agent },
  ...extra,
});

export async function testFinishedWorkerResume(browser, origin, ok) {
  const panes = {
    [FRONT]: [record(1, "claude", IDS.front, { focused: true })],
    [BEHIND]: [record(1, "codex", IDS.behind, { focused: true, awake: true })],
    [VISITED]: [record(1, "claude", IDS.visited, { focused: true }), record(2, "claude", IDS.refused)],
    [FINISHED]: [record(1, "claude", IDS.finished, { focused: true })],
    [EMPTIED]: [],
  };
  const page = await openWindowTestPage(browser, origin, {
    before: (held) =>
      held.addInitScript(
        ({ source, args }) => {
          eval(`(${source})`)(args);
        },
        { source: standBackend.toString(), args: { panes, ids: IDS, standing: [BEHIND] } },
      ),
  });
  try {
    const seen = await page.page.evaluate(
      async ({ FRONT, BEHIND, VISITED, FINISHED, EMPTIED, ids }) => {
        const settle = (ms = 60) => new Promise((done) => setTimeout(done, ms));
        const roadOf = (id) => window.__FINISHED__.resumes.filter((one) => one.id === id).map((one) => one.road);
        // The boot's own restores: the front, then the put-back behind it.
        for (let tries = 0; tries < 200 && roadOf(ids.behind).length === 0; tries += 1) await settle(50);
        await storedWakesSettled(FRONT);
        await storedWakesSettled(BEHIND);

        // A first visit: the eye's tab wakes, the second's wake refuses and its
        // fallback shell fails, so that tab stays asleep on the strip — then
        // the person picks it.
        activeWorktreePath = VISITED;
        await restoreActiveWorktreeTab();
        await storedWakesSettled(VISITED);
        await settle();
        const refusedTab = tabs.find((tab) => tab.worktree === VISITED && tab.storedId === 2);
        const refusedAsleep = Boolean(refusedTab?.asleep) && refusedTab?.wakeRefused === true;
        if (refusedTab) setActiveTab(refusedTab.id);
        await storedWakesSettled(VISITED);
        await settle();

        // The finished worker's checkout, visited.
        const shellsBefore = window.__FINISHED__.shells.length;
        const launchesBefore = window.__FINISHED__.launches;
        activeWorktreePath = FINISHED;
        await restoreActiveWorktreeTab();
        await storedWakesSettled(FINISHED);
        await settle(120);
        const finishedShells = window.__FINISHED__.shells.slice(shellsBefore);
        const finishedTab = tabs.find((tab) => tab.worktree === FINISHED && tab.kind === "term");
        const carried = finishedTab
          ? paneLeaves(finishedTab.layout).some((term) => paneSessions.get(term)?.session?.id === ids.finished)
          : null;
        persistPaneLayouts(FINISHED);
        await settle(120);
        const savedFinished = window.__FINISHED__.saves.filter((one) => one.worktree === FINISHED).at(-1);

        // The checkout whose only record the backend struck: nothing stored.
        const resumesBefore = window.__FINISHED__.resumes.length;
        const asksBefore = window.__FINISHED__.lastAgentAsks;
        const shellsBeforeEmpty = window.__FINISHED__.shells.length;
        activeWorktreePath = EMPTIED;
        await restoreActiveWorktreeTab();
        await settle(120);
        const emptied = {
          resumes: window.__FINISHED__.resumes.length - resumesBefore,
          asks: window.__FINISHED__.lastAgentAsks - asksBefore,
          shells: window.__FINISHED__.shells.length - shellsBeforeEmpty,
        };

        // A person's pick by name: the door the sidebar's row and the tab menu take.
        await resumeSession({ agent: "claude", session: { key: "session_id", id: ids.reopened } }, VISITED);
        await settle();

        return {
          front: roadOf(ids.front),
          behind: roadOf(ids.behind),
          visited: roadOf(ids.visited),
          refused: roadOf(ids.refused),
          refusedAsleep,
          finished: roadOf(ids.finished),
          finishedShells,
          finishedLaunches: window.__FINISHED__.launches - launchesBefore,
          carried,
          savedAgents: savedFinished ? savedFinished.layouts.map((tab) => tab.agents ?? null) : null,
          emptied,
          reopened: roadOf(ids.reopened),
        };
      },
      { FRONT, BEHIND, VISITED, FINISHED, EMPTIED, ids: IDS },
    );
    const same = (left, right) => JSON.stringify(left) === JSON.stringify(right);
    ok(
      "every wake names its road to the backend: restore-front, restore-behind, visit, stage once a person picks the tab again, reopen for a pick by name",
      same(seen.front, ["restore-front"]) &&
        same(seen.behind, ["restore-behind"]) &&
        same(seen.visited, ["visit"]) &&
        same(seen.refused, ["visit", "stage"]) &&
        seen.refusedAsleep &&
        same(seen.reopened, ["reopen"]),
      JSON.stringify(seen),
    );
    ok(
      "a finished worker's conversation stands a plain shell that says whose it was, starts no agent fresh, and carries nothing into the next save",
      same(seen.finished, ["visit"]) &&
        seen.finishedShells.length === 1 &&
        seen.finishedShells[0].plain &&
        String(seen.finishedShells[0].notice ?? "").includes("w-57717") &&
        seen.finishedLaunches === 0 &&
        seen.carried === false &&
        Array.isArray(seen.savedAgents) &&
        seen.savedAgents.length === 1 &&
        seen.savedAgents[0] === null,
      JSON.stringify(seen),
    );
    ok(
      "a checkout whose stored set came back empty resumes nothing and opens the way a workspace that stored nothing does",
      seen.emptied.resumes === 0 && seen.emptied.asks === 1 && seen.emptied.shells === 1,
      JSON.stringify(seen.emptied),
    );
    ok("the finished-worker restore raised no renderer errors", page.faults.length === 0, page.faults.join("\n"));
  } finally {
    await page.page.close();
  }
}
