/* Answers about EVERY pane at once, coming back after one of them ended
 * (t-99473).
 *
 * The window asks the backend for what it knows of all its panes — the agents
 * (`agent_terms`), their helpers (`pane_subagents`), what they are doing
 * (`pane_activities`), their conversations (`pane_sessions`), and the retained
 * state of every agent (`pane_agents`, asked every thirty seconds). The backend
 * answers on its own time, and a pane can end while an answer is out: written
 * back, the answer undoes the disposal, and the next shell at that id opens on
 * a stranger's agent, helper rows and badge. The observation clock cannot
 * refuse it, because disposing the pane cleared the clock — an old stamp then
 * looks new again.
 *
 * Each question is asked through the pane lifecycle (`paneLifecycle.ask`) and
 * writes only for the panes that did not end while it was out. A pane that
 * merely has no tab is not an ended pane: the backend can own an agent this
 * window holds no tab for, and bringing it back is what adoption is for.
 *
 * The production functions, cut out of `ui/shell.js` as they stand and run
 * beside the real lifecycle, with a backend the test answers by hand.
 *
 *   node --test ui/tests/pane-late-seed.test.mjs */
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
import { parse } from "acorn";

const read = (path) => readFileSync(new URL(path, import.meta.url), "utf8");
const shell = read("../shell.js");
const lifecycle = read("../shell-pane-lifecycle.js");
const plain = (value) => JSON.parse(JSON.stringify(value));
const declarations = parse(shell, { ecmaVersion: "latest" }).body;

function production(name) {
  const node = declarations.find(node => node.type === "FunctionDeclaration" && node.id?.name === name);
  assert.ok(node, `the production ${name} function is present in shell.js`);
  return shell.slice(node.start, node.end);
}

function statement(name) {
  const node = declarations.find(node => node.type === "VariableDeclaration" && node.declarations.some(entry => entry.id?.name === name));
  assert.ok(node, `the production declaration ${name} is present in shell.js`);
  return shell.slice(node.start, node.end);
}

/* A window holding what the seeds write into, with a backend answered by hand:
 * every `invoke` waits in `asked` until the test answers it. */
function windowAnswering(...functions) {
  const asked = {};
  const heard = { rendered: 0, painted: 0, clock: 0, schedules: [], adopted: [], settled: [] };
  const context = vm.createContext({
    invoke: (command) => new Promise((resolve, reject) => (asked[command] ??= []).push({ resolve, reject })),
    refreshPaneLedger: () => {},
    renderTabs: () => { heard.rendered += 1; },
    paintWorktreeAgents: () => { heard.painted += 1; },
    agentClock: { sync: () => { heard.clock += 1; } },
    helperClock: { sync: () => {} },
    settleComposerQueuesFor: (term) => heard.settled.push(term),
    scheduleAgentPaint: (surfaces) => heard.schedules.push(surfaces),
    adoptStrandedAgents: (rows) => heard.adopted.push(rows.map((row) => row.term)),
  });
  vm.runInContext(lifecycle, context);
  vm.runInContext(
    `const paneAgents = paneLifecycle.map("paneAgents");
     const paneSessions = paneLifecycle.map("paneSessions");
     const paneSubagents = paneLifecycle.map("paneSubagents");
     const paneActivities = paneLifecycle.map("paneActivities", {
       owns: (term, card) => card === "term:" + term || card.startsWith("sub:" + term + ":"),
     });
     const paneAutonomy = paneLifecycle.map("paneAutonomy");
     const hookStamps = paneLifecycle.map("hookStamps");
     const hookStates = paneHookStates();
     const WIRE_CARD_PREFIX = "wire:";
     let paneSnapshotAsking = false;`,
    context,
  );
  vm.runInContext(statement("HOOK_RANK"), context);
  for (const name of functions) vm.runInContext(production(name), context);
  const run = (code) => vm.runInContext(code, context);
  return {
    context, heard, run,
    /* The oldest question still out for `command`, answered. */
    answer: (command, value) => asked[command].shift().resolve(value),
    refuse: (command, error) => asked[command].shift().reject(error),
    ends: (term) => run(`paneLifecycle.dispose(${term})`),
    holds: (term) => plain(run(`paneLifecycle.holds(${term})`)),
    pending: () => run("paneLifecycle.pending()"),
  };
}

test("a snapshot answered after a pane ended does not bring it back, even with a stamp that looks new, and the live neighbours are applied", async () => {
  const w = windowAnswering("refreshPaneAgentSnapshot", "reconcilePaneAgentSnapshot", "rememberPaneAutonomy");
  w.run('hookStates.set(7, "working"); hookStates.observe(7, 900); hookStamps.set(7, 1);');

  const refreshing = w.context.refreshPaneAgentSnapshot();
  w.ends(7);
  assert.equal(w.run("hookStates.observedAt(7)"), 0, "disposing cleared the clock that would have refused an old stamp");
  w.answer("pane_agents", [
    { term: 7, agent: "codex", state: "done", at: 300, state_started_at: 250 },
    { term: 8, agent: "claude", state: "working", at: 400, state_started_at: 350 },
  ]);
  await refreshing;

  assert.equal(w.run("hookStates.get(7)"), undefined);
  assert.equal(w.run("hookStamps.get(7)"), undefined);
  assert.equal(w.run("hookStates.observedAt(7)"), 0);
  assert.deepEqual(w.holds(7), []);
  assert.equal(w.run("hookStates.get(8)"), "working", "the pane that stood is applied");
  assert.equal(w.run("hookStamps.get(8)"), 350);
  assert.deepEqual(w.heard.adopted, [[8]], "only the panes that stand are offered to adoption");
  assert.deepEqual(w.heard.settled, [8], "only the surviving pane's transition may settle its queue");
  assert.equal(w.pending(), 0);
});

test("a snapshot's agent this window holds no tab for is still applied and still offered to adoption", async () => {
  // Never ended, never had a tab: the backend's own. Filtering by tab would lose it.
  const w = windowAnswering("refreshPaneAgentSnapshot", "reconcilePaneAgentSnapshot", "rememberPaneAutonomy");
  const refreshing = w.context.refreshPaneAgentSnapshot();
  w.answer("pane_agents", [{ term: 9, agent: "claude", state: "working", at: 500, state_started_at: 450 }]);
  await refreshing;
  assert.equal(w.run("hookStates.get(9)"), "working");
  assert.deepEqual(w.heard.adopted, [[9]]);
});

test("an answer meant for the shell before is not written over the shell that took the id", async () => {
  const w = windowAnswering("seedPaneAgents");
  const seeding = w.context.seedPaneAgents();
  w.ends(7);
  w.run('paneAgents.set(7, "second-life")'); // the next shell takes the id and says what it is
  w.answer("agent_terms", [[7, "first-life"], [8, "codex"]]);
  await seeding;
  assert.equal(w.run("paneAgents.get(7)"), "second-life");
  assert.equal(w.run("paneAgents.get(8)"), "codex");
});

test("the agents the backend lists are written for the panes that stand, and the surfaces repaint once", async () => {
  const w = windowAnswering("seedPaneAgents");
  const seeding = w.context.seedPaneAgents();
  w.ends(7);
  w.answer("agent_terms", [[7, "claude"], [8, "codex"]]);
  await seeding;
  assert.equal(w.run("paneAgents.has(7)"), false);
  assert.equal(w.run("paneAgents.get(8)"), "codex");
  assert.deepEqual([w.heard.rendered, w.heard.painted, w.heard.clock], [1, 1, 1]);
  assert.equal(w.pending(), 0);
});

test("the helpers the backend lists are written for the panes that stand", async () => {
  const w = windowAnswering("seedSubagents");
  const seeding = w.context.seedSubagents();
  w.ends(7);
  w.answer("pane_subagents", [
    { term: 7, rows: [{ id: "helper-7" }] },
    { term: 8, rows: [{ id: "helper-8" }] },
    { term: 9, rows: [] },
  ]);
  await seeding;
  assert.equal(w.run("paneSubagents.has(7)"), false);
  assert.deepEqual(plain(w.run("paneSubagents.get(8)")), [{ id: "helper-8" }]);
  assert.equal(w.run("paneSubagents.has(9)"), false, "no rows, nothing to keep — as before");
  assert.equal(w.pending(), 0);
});

test("a card's shell is read the one way: a shell's own card and each of its helpers' cards are the shell's, a wire's are nobody's", () => {
  const w = windowAnswering("cardTerm");
  const of = (card) => w.run(`cardTerm(${JSON.stringify(card)})`);
  assert.equal(of("term:3"), 3);
  assert.equal(of("sub:3:a-1"), 3);
  assert.equal(of("sub:30:a-1"), 30);
  assert.equal(of("wire:9"), null);
  assert.equal(of("something else"), null);
});

test("the activities the backend lists are written for the panes that stand — a shell's card and its helpers' — and never for a wire", async () => {
  const w = windowAnswering("seedActivities", "cardTerm");
  const seeding = w.context.seedActivities();
  w.ends(7);
  const said = (pane) => ({ pane, activities: [{ seq: 0, activity: { verb: "bash", target: pane, phase: "started" } }] });
  w.answer("pane_activities", [said("term:7"), said("sub:7:helper"), said("term:8"), said("sub:8:helper"), said("wire:9"), { pane: "term:10", activities: [] }]);
  await seeding;
  assert.deepEqual(plain([...w.run("paneActivities.keys()")]), ["term:8", "sub:8:helper"]);
  assert.deepEqual(w.holds(7), []);
  assert.equal(w.pending(), 0);
});

test("the conversations the backend lists are written for the panes that stand", async () => {
  const w = windowAnswering("seedPaneSessions");
  const seeding = w.context.seedPaneSessions();
  w.ends(7);
  w.answer("pane_sessions", [
    { term: 7, agent: "claude", session: { key: "session_id", id: "s-7" }, resumable: true },
    { term: 8, agent: "zo", session: { key: "session_id", id: "s-8" }, resumable: false },
  ]);
  await seeding;
  assert.equal(w.run("paneSessions.has(7)"), false);
  assert.deepEqual(plain(w.run("paneSessions.get(8)")), { agent: "zo", session: { key: "session_id", id: "s-8" }, resumable: false });
  assert.equal(w.heard.clock, 1, "the clock is asked once the map stands");
  assert.equal(w.pending(), 0);
});

test("a seed whose question fails is a quiet failure, and leaves no fence open", async () => {
  const w = windowAnswering("seedPaneAgents", "seedSubagents", "seedActivities", "seedPaneSessions", "cardTerm");
  const all = [
    w.context.seedPaneAgents(), w.context.seedSubagents(), w.context.seedActivities(), w.context.seedPaneSessions(),
  ];
  assert.equal(w.pending(), 4);
  w.refuse("agent_terms", new Error("no backend"));
  w.refuse("pane_subagents", new Error("no backend"));
  w.refuse("pane_activities", new Error("no backend"));
  w.refuse("pane_sessions", new Error("no backend"));
  await Promise.all(all);
  assert.equal(w.pending(), 0);
  assert.deepEqual(
    Object.values(plain(w.run("paneLifecycle.sizes()"))).filter((size) => size !== 0),
    [],
    "a failed question wrote nothing",
  );
});

test("a snapshot that fails once may be asked again", async () => {
  const w = windowAnswering("refreshPaneAgentSnapshot", "reconcilePaneAgentSnapshot", "rememberPaneAutonomy");
  const first = w.context.refreshPaneAgentSnapshot();
  const overlapping = w.context.refreshPaneAgentSnapshot();
  w.refuse("pane_agents", new Error("closing"));
  await Promise.all([first, overlapping]);
  assert.equal(w.pending(), 0);
  assert.equal(w.run("paneSnapshotAsking"), false, "the one-at-a-time mark is clear after a failure");
  const again = w.context.refreshPaneAgentSnapshot();
  w.answer("pane_agents", [{ term: 8, agent: "claude", state: "done", at: 10, state_started_at: 9 }]);
  await again;
  assert.equal(w.run("hookStates.get(8)"), "done");
});
