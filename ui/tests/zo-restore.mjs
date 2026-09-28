import { openWindowTestPage } from "./window-boot.mjs";

/* The zo panes a restart brings back (t-12063).
 *
 * On 2026-09-28 a restart brought back one zo conversation of three. The one
 * the window tried in another workspace died in about a second with nothing on
 * its screen: zo looks a conversation up in the store of the folder it is
 * started in, and the wake started it in whatever workspace was in front at
 * that moment rather than the tab's own — `session not found`, seven times in
 * zo's own log since 09-13, every one a conversation sitting whole in another
 * project's store. The pane that stood in its place was a bare shell, the
 * person typed `zo` into it, and the new conversation took the old one's line
 * in the file.
 *
 * The backend is stubbed with zo's one rule that matters here: a conversation
 * opens only in its own workspace's folder. The folder a wake opens in is the
 * workspace the door names, or — the old road — the window's active root. */
export async function exerciseZoRestore() {
  const settle = (ms = 60) => new Promise((done) => setTimeout(done, ms));
  const A = "/tmp/zo-restore/a";
  const B = "/tmp/zo-restore/b";
  const id = (n) => `session-179000000000${n}-0`;
  // Which workspace's store each conversation is written in.
  const owner = {
    [id(1)]: A, [id(2)]: A, [id(3)]: B, [id(4)]: A,
    "3f1e2d4c-0000-4000-8000-00000000c1a0": A,
    "019a0000-0000-7000-8000-00000000c0de": A,
  };
  const seen = {};
  let activeRoot = B;
  const calls = [];
  const shells = [];
  const saves = [];
  const standing = new Map();
  let nextTerm = 9600;
  const priorAnswers = { ...window.__ANSWER__ };
  const priorWorktree = activeWorktreePath;
  window.__ANSWER__.set_active_worktree = async (args) => {
    await settle(40);
    activeRoot = args.path;
    return null;
  };
  window.__ANSWER__.pane_layouts = () => [];
  window.__ANSWER__.resume_session = (args) => {
    const root = args.worktree ?? activeRoot;
    calls.push({ agent: args.agent, id: args.session.id, key: args.session.key, worktree: args.worktree ?? null, root, rows: args.rows, cols: args.cols });
    if (standing.has(args.session.id)) return { term: standing.get(args.session.id), standing: true };
    if (owner[args.session.id] !== root) {
      throw `the pane exited (code 1) before publishing its events channel — zo: session not found: ${args.session.id}`;
    }
    const term = (nextTerm += 1);
    standing.set(args.session.id, term);
    return { term, standing: false };
  };
  window.__ANSWER__.open_term_tab = (args) => {
    const term = (nextTerm += 1);
    shells.push({ term, notice: args.notice ?? null });
    return term;
  };
  window.__ANSWER__.resume_line = (args) => `${args.agent} --resume ${args.session.id}`;
  window.__ANSWER__.save_pane_layouts = (args) => {
    saves.push(JSON.parse(JSON.stringify(args)));
    return null;
  };
  const record = (at, agent, sessionId, focused = false) => ({
    id: at,
    root: { type: "leaf" },
    agents: { 0: { agent, key: "session_id", id: sessionId } },
    running: { 0: agent },
    focused,
  });
  const wakeAll = async (worktree) => {
    for (const tab of tabs.filter((one) => one.worktree === worktree && one.asleep)) await wakeStoredTab(tab);
    await storedWakesSettled(worktree);
  };

  // 1. Two workspaces, three zo panes — A holds two — restored while B is the
  //    workspace in front: every one comes back in its own workspace.
  activeWorktreePath = B;
  await restoreWorktreeLayouts(A, [record(1, "zo", id(1), true), record(2, "zo", id(2))]);
  await wakeAll(A);
  await restoreWorktreeLayouts(B, [record(1, "zo", id(3), true)]);
  await wakeAll(B);
  await settle();
  const first = calls.filter((call) => [id(1), id(2), id(3)].includes(call.id));
  seen.restoredZo = [id(1), id(2), id(3)].filter((one) => standing.has(one)).length;
  seen.eachInItsOwn = first.length === 3 && first.every((call) => call.worktree === owner[call.id]);
  seen.refusedShells = shells.length;

  // 2. A conversation that cannot come back says so in the pane that stands
  //    in for it: zo's own line, and how to go on.
  const lost = "session-1790000000009-0";
  const lostTab = openStoredTab(record(3, "zo", lost), A);
  const shellsBefore = shells.length;
  await mountStoredLayout(lostTab.asleep, A, lostTab);
  const stand = shells[shellsBefore];
  seen.refusedPaneSays = Boolean(
    stand?.notice?.includes("session not found") && stand.notice.includes(`cd ${A} && zo --resume ${lost}`),
  );
  seen.carried = paneSessions.get(stand?.term)?.carriedOnly === true;

  // 3. The person types `zo` into that shell. The new conversation takes the
  //    pane; the one it replaced is kept as owed and named in one line.
  const fresh = "session-1790000000010-0";
  const toastsBefore = document.querySelectorAll(".toast").length;
  const savesBefore = saves.length;
  for (const listener of window.__LISTENERS__["hook:agent"] ?? []) {
    listener({ payload: { term: stand?.term, state: "working", agent: "zo", resumable: true, session: { key: "session_id", id: fresh } } });
  }
  await settle();
  const told = [...document.querySelectorAll(".toast")].slice(toastsBefore).map((note) => note.textContent);
  seen.toldOwed = told.some((said) => said.includes(`cd ${A} && zo --resume ${lost}`));
  persistPaneLayouts(A);
  const lastA = saves.slice(savesBefore).filter((save) => save.worktree === A).at(-1);
  const heldTab = lastA?.layouts?.find((layout) => layout.agents?.[0]?.id === fresh);
  seen.newHeld = Boolean(heldTab);
  seen.owedKept = Boolean(heldTab?.owed?.some((one) => one.id === lost && one.agent === "zo"));

  // 4. One conversation, one process — and two doors on one conversation
  //    (the sidebar row pressed twice while its workspace is still opening)
  //    both land in its own workspace, and only one of them opens it.
  activeRoot = B;
  activeWorktreePath = B;
  const callsBefore = calls.length;
  const known = { agent: "zo", session: { key: "session_id", id: id(4) } };
  // The second press lands after the first has moved the window's word for
  // the workspace in front, and before the backend has moved its root.
  const pressed = reopenConversationIn(A, known);
  await settle(10);
  await Promise.all([pressed, reopenConversationIn(A, known)]);
  await settle();
  const doors = calls.slice(callsBefore).filter((call) => call.id === id(4));
  seen.doors = doors.map((call) => ({ root: call.root, worktree: call.worktree }));
  seen.twoDoorsOneProcess = doors.length >= 1 && doors.every((call) => call.root === A) && standing.has(id(4));

  // 5. Claude and Codex come back on the same road with the same words:
  //    their agent, their session, their grid — and their own workspace.
  activeRoot = B;
  activeWorktreePath = B;
  const claude = "3f1e2d4c-0000-4000-8000-00000000c1a0";
  const codex = "019a0000-0000-7000-8000-00000000c0de";
  const vendorsBefore = calls.length;
  await restoreWorktreeLayouts(A, [record(4, "claude", claude, true), record(5, "codex", codex)]);
  await wakeAll(A);
  await settle();
  const vendors = calls.slice(vendorsBefore);
  seen.vendors = vendors.map((call) => `${call.agent}:${call.key}:${call.worktree}`);
  seen.vendorsBack =
    vendors.length === 2 &&
    vendors.some((call) => call.agent === "claude" && call.id === claude && call.key === "session_id") &&
    vendors.some((call) => call.agent === "codex" && call.id === codex && call.key === "session_id") &&
    vendors.every((call) => call.worktree === A && Number.isInteger(call.rows) && Number.isInteger(call.cols)) &&
    standing.has(claude) && standing.has(codex);

  Object.assign(window.__ANSWER__, priorAnswers);
  for (const key of Object.keys(window.__ANSWER__)) if (!(key in priorAnswers)) delete window.__ANSWER__[key];
  activeWorktreePath = priorWorktree;
  return seen;
}

export async function testZoRestore(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    const seen = await page.evaluate(exerciseZoRestore);
    console.log(`MEASURE zo panes back after a restart with another workspace in front: ${seen.restoredZo}/3`);
    ok(
      "every stored zo pane wakes in its own workspace's folder, whichever workspace is in front — two workspaces, three panes, all three back",
      seen.restoredZo === 3 && seen.eachInItsOwn && seen.refusedShells === 0,
      JSON.stringify(seen),
    );
    ok(
      "a conversation that cannot come back says so in the pane that stands in for it — zo's own line and the command that goes on — and the shell carries it",
      seen.refusedPaneSays && seen.carried,
      JSON.stringify(seen),
    );
    ok(
      "a new conversation typed into that shell does not erase the one it replaced: kept as owed in the file, and named in one line with how to go on",
      seen.toldOwed && seen.newHeld && seen.owedKept,
      JSON.stringify(seen),
    );
    ok(
      "two doors on one conversation while its workspace is still opening both wake it in that workspace, and one process opens it",
      seen.twoDoorsOneProcess,
      JSON.stringify(seen),
    );
    ok(
      "Claude and Codex panes come back on the same road with the same agent, session and grid",
      seen.vendorsBack,
      JSON.stringify(seen),
    );
    ok("restoring zo panes raised no renderer errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}
