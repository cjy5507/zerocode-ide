/* What the window remembers about a pane ends with the pane — through the
 * interface, on the production file itself.
 *
 * `ui/shell-pane-lifecycle.js` reaches nothing in the document, so the whole
 * file runs in a bare context and every check below goes through what a caller
 * of the window sees: `map`, `set`, `dispose`, `holds`, `names`. Nothing reads
 * the file's text. The same file serves the window (`ui/index.html`); the
 * browser suite `pane-lifecycle.mjs` drives the declarations the window makes
 * with it.
 *
 *   node --test ui/tests/pane-lifecycle.test.mjs
 */
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const source = readFileSync(new URL("../shell-pane-lifecycle.js", import.meta.url), "utf8");

/* A window that has loaded only the lifecycle file: a context with no globals
 * at all, so a file that reached for `document` while loading would throw. */
function loadedWindow() {
  const context = vm.createContext({});
  vm.runInContext(source, context, { filename: "ui/shell-pane-lifecycle.js" });
  return context;
}

const newLifecycle = () => vm.runInContext("createPaneLifecycle()", loadedWindow());

/* Arrays made inside the context carry that context's prototype. */
const plain = (value) => JSON.parse(JSON.stringify(value));

/* The declared names, in declaration order, as the census reports them. */
const namesOf = (panes) => Object.keys(plain(panes.sizes()));

/* The collections a pane really keeps, in the shapes the window keys them. */
function paneState() {
  const panes = newLifecycle();
  return {
    panes,
    models: panes.map("models"),
    bells: panes.set("bells"),
    acknowledged: panes.map("acknowledged", { owns: (term, key) => key === `term:${term}` }),
    activities: panes.map("activities", {
      owns: (term, key) => key === `term:${term}` || key.startsWith(`sub:${term}:`),
    }),
  };
}

function fill(state, term) {
  state.models.set(term, `model-of-${term}`);
  state.bells.add(term);
  state.acknowledged.set(`term:${term}`, 1000 + term);
  state.activities.set(`term:${term}`, ["the shell"]);
  state.activities.set(`sub:${term}:helper-1`, ["a helper"]);
}

test("a declared collection is a real Map or Set, so its old readers and writers stay as they were", () => {
  const { panes, models, bells } = paneState();
  models.set(3, "claude");
  bells.add(3);
  assert.equal(Object.prototype.toString.call(models), "[object Map]");
  assert.equal(Object.prototype.toString.call(bells), "[object Set]");
  assert.equal(models.get(3), "claude");
  assert.equal(bells.has(3), true);
  assert.deepEqual([...models.keys()], [3]);
  assert.deepEqual(namesOf(panes), ["models", "bells", "acknowledged", "activities"]);
});

test("a name is declared once, and a declaration is checked when it is made", () => {
  const panes = newLifecycle();
  panes.map("models");
  assert.throws(() => panes.map("models"), /declared twice/);
  assert.throws(() => panes.set("models"), /declared twice/);
  assert.throws(() => panes.map(""), /needs a name/);
  assert.throws(() => panes.set(undefined), /needs a name/);
  assert.throws(() => panes.map("x", { owns: "term" }), /owns must be a function/);
  assert.throws(() => panes.map("y", { release: "drop" }), /release must be a function/);
  assert.throws(() => panes.set("z", { release: () => {} }), /holds no entry to release/);
  assert.deepEqual(namesOf(panes), ["models"], "a refused declaration leaves nothing behind");
});

test("disposing a pane empties every collection of it, whatever shape its keys take, and names what held something", () => {
  const state = paneState();
  fill(state, 7);
  state.models.set(8, "other");
  assert.deepEqual(plain(state.panes.holds(7)), ["models", "bells", "acknowledged", "activities"]);

  const emptied = state.panes.dispose(7);

  assert.deepEqual(plain(emptied), ["models", "bells", "acknowledged", "activities"]);
  assert.equal(state.models.has(7), false);
  assert.equal(state.bells.has(7), false);
  assert.equal(state.acknowledged.has("term:7"), false);
  assert.deepEqual([...state.activities.keys()], [], "the shell's card and its helper's card both went");
  assert.deepEqual(plain(state.panes.holds(7)), []);
  assert.equal(state.models.get(8), "other", "the neighbour kept what it had");
  assert.deepEqual(plain(state.panes.holds(8)), ["models"]);
});

test("a pane is told apart from another whose id starts the same way", () => {
  const state = paneState();
  fill(state, 1);
  fill(state, 11);
  fill(state, 110);

  state.panes.dispose(1);

  assert.deepEqual(plain(state.panes.holds(1)), []);
  assert.deepEqual(plain(state.panes.holds(11)), ["models", "bells", "acknowledged", "activities"]);
  assert.deepEqual(plain(state.panes.holds(110)), ["models", "bells", "acknowledged", "activities"]);
  assert.deepEqual([...state.activities.keys()].sort(), ["sub:110:helper-1", "sub:11:helper-1", "term:11", "term:110"]);
});

test("the next shell to take an id starts empty, and ends cleanly in its turn", () => {
  const state = paneState();
  fill(state, 7);
  state.panes.dispose(7);

  assert.equal(state.models.get(7), undefined, "no model from a stranger");
  assert.equal(state.acknowledged.get("term:7"), undefined, "no acknowledgement from a stranger");
  assert.deepEqual(plain(state.panes.holds(7)), []);

  // Its own life.
  state.models.set(7, "second-life");
  state.bells.add(7);
  assert.deepEqual(plain(state.panes.holds(7)), ["models", "bells"]);
  assert.deepEqual(plain(state.panes.dispose(7)), ["models", "bells"]);
  assert.deepEqual(plain(state.panes.holds(7)), []);
});

test("disposing again is safe: nothing is released twice and nobody else is touched", () => {
  const panes = newLifecycle();
  const released = [];
  const pages = panes.map("pages", { release: (term, page) => released.push([term, page]) });
  const models = panes.map("models");
  pages.set(7, "page-7");
  models.set(7, "a");
  models.set(8, "b");

  assert.deepEqual(plain(panes.dispose(7)), ["pages", "models"]);
  assert.deepEqual(plain(panes.dispose(7)), []);
  assert.deepEqual(plain(panes.dispose(7)), []);

  assert.deepEqual(plain(released), [[7, "page-7"]], "the page was let go once");
  assert.equal(models.get(8), "b");
  assert.deepEqual(plain(panes.dispose(999)), [], "an id nobody used has nothing to release");
});

test("a pane that never opened its conversation is torn down like any other", () => {
  // `forgetPaneChat` used to end with the page's own teardown and then forget
  // the words and the usage reading — but it returned first when no page
  // stood, so a pane whose conversation was never opened kept both.
  const panes = newLifecycle();
  const letGo = [];
  const chats = panes.map("paneChats", { release: (term, held) => letGo.push([term, held.host]) });
  const live = panes.map("paneLive");
  const now = panes.map("paneNow");
  const usage = panes.map("paneUsage");

  live.set(5, [{ role: "assistant", text: "half a sentence" }]);
  now.set(5, { verb: "Read", target: "a file" });
  usage.set(5, { tokens: 1200 });
  live.set(6, [{ role: "assistant", text: "words" }]);
  chats.set(6, { host: "page-6" });

  assert.doesNotThrow(() => panes.dispose(5));
  assert.deepEqual(plain(panes.holds(5)), []);
  assert.deepEqual(plain(letGo), [], "no page stood, so none was let go");

  assert.deepEqual(plain(panes.dispose(6)), ["paneChats", "paneLive"]);
  assert.deepEqual(plain(letGo), [[6, "page-6"]]);
  assert.equal(chats.size + live.size + now.size + usage.size, 0);
});

test("what an entry holds is let go as the entry leaves, by dispose or by hand", () => {
  const panes = newLifecycle();
  const seen = [];
  const pages = panes.map("pages", {
    release: (term, page) => seen.push({ term, page, standingWhenReleased: pages.has(term) && pages.get(term) === page }),
  });
  pages.set(1, "a");
  pages.set(2, "b");
  pages.set(3, "c");

  assert.equal(pages.delete(1), true);
  assert.equal(pages.delete(1), false, "an entry that is not there has nothing to release");
  panes.dispose(2);
  pages.clear();

  assert.deepEqual(
    plain(seen),
    [
      { term: 1, page: "a", standingWhenReleased: true },
      { term: 2, page: "b", standingWhenReleased: true },
      { term: 3, page: "c", standingWhenReleased: true },
    ],
    "each entry is released once, while it still stands",
  );
  assert.equal(pages.size, 0);
});

test("a release that throws strands nothing: every collection is emptied, then the first error is thrown", () => {
  const panes = newLifecycle();
  const order = [];
  const first = panes.map("first", {
    release: () => {
      order.push("first");
      throw new Error("first release failed");
    },
  });
  const middle = panes.map("middle");
  const last = panes.map("last", { release: () => order.push("last") });
  const third = panes.map("third", {
    release: () => {
      order.push("third");
      throw new Error("third release failed");
    },
  });
  for (const store of [first, middle, last, third]) store.set(7, "x");

  assert.throws(() => panes.dispose(7), /first release failed/);

  assert.deepEqual(plain(panes.holds(7)), [], "the failing entries left too");
  assert.equal(middle.has(7), false);
  assert.deepEqual(plain(order), ["first", "last", "third"], "a failure did not stop the collections after it");
  assert.deepEqual(plain(panes.dispose(7)), [], "and the pane is gone for good");
});

test("the census counts every declared collection by name, and reads zero once every pane has ended", () => {
  const state = paneState();
  assert.deepEqual(plain(state.panes.sizes()), { models: 0, bells: 0, acknowledged: 0, activities: 0 });

  fill(state, 7);
  fill(state, 8);
  assert.deepEqual(plain(state.panes.sizes()), { models: 2, bells: 2, acknowledged: 2, activities: 4 });

  state.panes.dispose(7);
  assert.deepEqual(plain(state.panes.sizes()), { models: 1, bells: 1, acknowledged: 1, activities: 2 });
  state.panes.dispose(8);
  assert.deepEqual(plain(state.panes.sizes()), { models: 0, bells: 0, acknowledged: 0, activities: 0 });
});

test("the observation clock rides with the pane's agent state", () => {
  const window_ = loadedWindow();
  window_.panes = vm.runInContext("createPaneLifecycle()", window_);
  const hook = vm.runInContext("paneHookStates(panes)", window_);

  assert.equal(hook.observedAt(7), 0, "nothing observed yet");
  hook.observe(7, 100);
  hook.set(7, "working");
  hook.noteSnapshot(7);
  assert.equal(hook.observedAt(7), 100);
  assert.equal(hook.consumeSnapshot(7), true, "the mark is heard once");
  assert.equal(hook.consumeSnapshot(7), false);

  // A state deleted by hand takes its observation and its mark with it.
  hook.observe(8, 200);
  hook.noteSnapshot(8);
  hook.set(8, "done");
  assert.equal(hook.delete(8), true);
  assert.equal(hook.observedAt(8), 0);
  assert.equal(hook.consumeSnapshot(8), false);

  // And so does clearing every state.
  hook.observe(9, 300);
  hook.noteSnapshot(9);
  hook.clear();
  assert.equal(hook.observedAt(7), 0);
  assert.equal(hook.observedAt(9), 0);
  assert.equal(hook.consumeSnapshot(9), false);
});

test("a disposed pane leaves no observation behind, even one recorded before any state", () => {
  const window_ = loadedWindow();
  const panes = (window_.panes = vm.runInContext("createPaneLifecycle()", window_));
  const hook = vm.runInContext("paneHookStates(panes)", window_);
  assert.deepEqual(namesOf(panes), ["hookStates", "hookObservations", "hookSnapshotMarks"]);

  // The webview observes a hook before the state it carries is stored, and a
  // snapshot is marked after: neither depends on a state standing.
  hook.observe(7, 900);
  hook.noteSnapshot(7);
  assert.deepEqual(plain(panes.holds(7)), ["hookObservations", "hookSnapshotMarks"]);
  hook.set(7, "working");
  assert.deepEqual(plain(panes.holds(7)), ["hookStates", "hookObservations", "hookSnapshotMarks"]);

  panes.dispose(7);

  assert.deepEqual(plain(panes.holds(7)), []);
  assert.equal(hook.get(7), undefined);
  // The fence of the shell that ended must not hold back the next shell's
  // first snapshot: a snapshot stamped 300 is newer than nothing.
  assert.equal(hook.observedAt(7), 0);
  assert.equal(hook.consumeSnapshot(7), false);
});

test("state seeded by a restore, in any order, is released in one pass whether or not the pane ever reported", () => {
  // A restored pane is seeded (agent, conversation, birth word) before its
  // first hook arrives; the order the seeds land in is the backend's, not ours.
  const orders = [
    ["agents", "sessions", "restored"],
    ["restored", "agents", "sessions"],
    ["sessions", "restored", "agents"],
  ];
  for (const order of orders) {
    const window_ = loadedWindow();
    const panes = (window_.panes = vm.runInContext("createPaneLifecycle()", window_));
    const seeds = {
      agents: panes.map("agents"),
      sessions: panes.map("sessions"),
      restored: panes.map("restored"),
    };
    const hook = vm.runInContext("paneHookStates(panes)", window_);
    for (const name of order) seeds[name].set(40, name);

    // Closed before its first report: nothing but the seeds stands.
    assert.deepEqual(plain(panes.dispose(40)), ["agents", "sessions", "restored"], order.join(","));
    assert.deepEqual(plain(panes.holds(40)), []);

    // The same id restored again: seeds take, and the first report finds the
    // birth word to clear and no stale fence.
    for (const name of order) seeds[name].set(40, `${name}-again`);
    assert.equal(hook.observedAt(40), 0);
    assert.equal(seeds.restored.delete(40), true, "the first real hook clears the restore word once");
    hook.observe(40, 50);
    hook.set(40, "working");
    assert.deepEqual(plain(panes.holds(40)), ["agents", "sessions", "hookStates", "hookObservations"]);
  }
});

/* A question the backend answers later, with the answer in hand. */
function deferred() {
  let settle = null;
  let refuse = null;
  const promise = new Promise((resolve, reject) => {
    settle = resolve;
    refuse = reject;
  });
  return { promise, settle, refuse };
}

test("an answer is written only for the panes that did not end while the question was out", async () => {
  // A seed of every pane the backend knows, asked at boot, the way
  // `seedPaneAgents` asks it: rows of [pane, agent].
  const panes = newLifecycle();
  const agents = panes.map("agents");
  const answer = deferred();
  const written = [];
  const seeding = panes.ask(
    () => answer.promise,
    (rows, ended) => {
      for (const [pane, agent] of rows) {
        if (ended(pane)) continue;
        agents.set(pane, agent);
        written.push(pane);
      }
      return rows.length;
    },
  );
  assert.equal(panes.pending(), 1);

  panes.dispose(7); // closed while the question was out
  answer.settle([[7, "claude"], [8, "codex"], [9, "claude"]]);

  assert.equal(await seeding, 3, "the question answers what its receive returned");
  assert.deepEqual(written, [8, 9], "the pane that ended is not brought back; the live neighbours are written");
  assert.equal(agents.has(7), false);
  assert.deepEqual([...agents.keys()], [8, 9]);
  assert.equal(panes.pending(), 0);
});

test("a pane that ended in the gap between the answer arriving and its being written is an ended pane too", async () => {
  const panes = newLifecycle();
  const answer = deferred();
  let heard = null;
  const asking = panes.ask(
    () => answer.promise,
    (_rows, ended) => {
      heard = { seven: ended(7), eight: ended(8) };
    },
  );
  // The disposal is already queued when the answer is handed over, so it runs
  // between the two: the answer is out, `receive` has not run yet.
  queueMicrotask(() => panes.dispose(7));
  answer.settle("rows");
  await asking;
  assert.deepEqual(heard, { seven: true, eight: false });
});

test("a pane that ended before the question was asked is not ended for it, and each question has its own fence", async () => {
  const panes = newLifecycle();
  panes.dispose(7); // before either question
  const early = deferred();
  const late = deferred();
  const heard = {};
  const first = panes.ask(() => early.promise, (_a, ended) => { heard.first = [ended(7), ended(8)]; });
  panes.dispose(8); // while the first is out, before the second is asked
  const second = panes.ask(() => late.promise, (_a, ended) => { heard.second = [ended(7), ended(8)]; });
  assert.equal(panes.pending(), 2);
  early.settle(null);
  late.settle(null);
  await Promise.all([first, second]);
  assert.deepEqual(heard, { first: [false, true], second: [false, false] });
  assert.equal(panes.pending(), 0);
});

test("a pane that ends from inside the write is ended for what follows it in the same write", async () => {
  const panes = newLifecycle();
  let after = null;
  await panes.ask(
    () => "rows",
    (_rows, ended) => {
      const before = ended(7);
      panes.dispose(7);
      after = [before, ended(7)];
    },
  );
  assert.deepEqual(after, [false, true]);
});

test("a question that fails, or whose write fails, closes its fence and lets the error go on", async () => {
  const panes = newLifecycle();
  await assert.rejects(panes.ask(() => Promise.reject(new Error("the backend refused")), () => {}), /backend refused/);
  assert.equal(panes.pending(), 0);
  await assert.rejects(
    panes.ask(() => {
      throw new Error("the request threw before it returned");
    }, () => {}),
    /threw before it returned/,
  );
  assert.equal(panes.pending(), 0);
  await assert.rejects(
    panes.ask(() => "rows", () => {
      throw new Error("the write failed");
    }),
    /write failed/,
  );
  assert.equal(panes.pending(), 0, "no fence is left open by a failure");
});

test("a disposal that fails midway still tells the questions out about the pane", async () => {
  const panes = newLifecycle();
  const pages = panes.map("pages", {
    release: () => {
      throw new Error("release failed");
    },
  });
  pages.set(7, "page");
  const answer = deferred();
  let heard = null;
  const asking = panes.ask(() => answer.promise, (_a, ended) => { heard = ended(7); });
  assert.throws(() => panes.dispose(7), /release failed/);
  answer.settle(null);
  await asking;
  assert.equal(heard, true);
});

test("a question that ends for a pane nobody asked about leaves nothing standing", async () => {
  // The fence is the only thing a question keeps, and it keeps it for the
  // question's own while: a window that asks all day and closes panes all day
  // holds no more at rest than a window that never did.
  const panes = newLifecycle();
  for (let round = 0; round < 200; round += 1) {
    const answer = deferred();
    const asking = panes.ask(() => answer.promise, () => {});
    panes.dispose(round);
    panes.dispose(round + 1_000);
    answer.settle(null);
    await asking;
  }
  assert.equal(panes.pending(), 0);
});

test("the lifecycle file loads in a window with nothing in it and declares nothing by itself", () => {
  const window_ = loadedWindow();
  assert.deepEqual(plain(vm.runInContext("paneLifecycle.sizes()", window_)), {});
  // The window's one lifecycle is what `paneHookStates()` declares on by default.
  vm.runInContext("paneHookStates()", window_);
  assert.deepEqual(plain(vm.runInContext("paneLifecycle.sizes()", window_)), {
    hookStates: 0,
    hookObservations: 0,
    hookSnapshotMarks: 0,
  });
  assert.deepEqual(plain(vm.runInContext("paneLifecycle.dispose(1)", window_)), []);
});
