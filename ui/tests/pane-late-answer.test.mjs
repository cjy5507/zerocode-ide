/* An answer that comes back after its pane ended says nothing (t-99473).
 *
 * The backend answers on its own time. `refreshPaneEffort` asks it for a pane's
 * effort and `movePaneEffort` presses a rung, and either can still be out when
 * the pane closes. The answer then used to write a record for a pane that is
 * not there — and for the next shell at the same id, a stranger's effort chip.
 * Both are held to the record they were asked into: if it is gone, or another
 * shell has made its own, the answer is dropped.
 *
 * The production functions, cut out of `ui/shell-composer.js` as they stand and
 * run beside the real lifecycle (`ui/shell-pane-lifecycle.js`), with a backend
 * the test answers by hand and the moment of each answer its own.
 *
 *   node --test ui/tests/pane-late-answer.test.mjs */
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
import { parse } from "acorn";

const read = (path) => readFileSync(new URL(path, import.meta.url), "utf8");
const composer = read("../shell-composer.js");
const lifecycle = read("../shell-pane-lifecycle.js");
const declarations = parse(composer, { ecmaVersion: "latest" }).body;

const production = ["refreshPaneEffort", "rememberPaneEffort", "movePaneEffort"].map((name) => {
  const node = declarations.find(node => node.type === "FunctionDeclaration" && node.id?.name === name);
  assert.ok(node, `the production ${name} function is present in shell-composer.js`);
  return composer.slice(node.start, node.end);
});

const plain = (value) => JSON.parse(JSON.stringify(value));

/* A window with the lifecycle's own `paneEfforts` and a backend answered by
 * hand: every call to `invoke` waits in `asked` until the test settles it. */
function windowAnswered({ invoke = null } = {}) {
  const asked = [];
  const said = { repainted: [], errors: [] };
  const context = vm.createContext({
    invoke: (command, args) => {
      if (invoke) return invoke(command, args);
      return new Promise((resolve, reject) => asked.push({ command, args, resolve, reject }));
    },
    paintComposerChipsFor: (term) => said.repainted.push(term),
    showError: (words) => said.errors.push(words),
    effortRefusalWords: (text) => `refused: ${text}`,
  });
  vm.runInContext(lifecycle, context);
  vm.runInContext('const paneEfforts = paneLifecycle.map("paneEfforts");', context);
  for (const source of production) vm.runInContext(source, context);
  const run = (code) => vm.runInContext(code, context);
  return {
    context, asked, said, run,
    efforts: (term) => run(`paneEfforts.get(${term})`) ?? null,
    ends: (term) => run(`paneLifecycle.dispose(${term})`),
    holds: (term) => plain(run(`paneLifecycle.holds(${term})`)),
  };
}

test("an answer that comes back after its pane ended says nothing, and the asker still hears it", async () => {
  const { context, asked, said, efforts, ends, holds } = windowAnswered();
  const pending = context.refreshPaneEffort(7);
  assert.equal(asked.length, 1);

  ends(7);
  asked[0].resolve({ level: "high" });

  assert.deepEqual(plain(await pending), { level: "high" });
  assert.equal(efforts(7), null, "no record for a pane that is gone");
  assert.deepEqual(holds(7), []);
  assert.deepEqual(said.repainted, [], "no chip is repainted for a pane that is gone");
});

test("the next shell at the same id is not written to by the answer meant for the one before", async () => {
  const { context, asked, said, efforts, ends } = windowAnswered();
  const before = context.refreshPaneEffort(7);
  ends(7);

  // A new shell takes the id and asks for its own.
  const after = context.refreshPaneEffort(7);
  assert.equal(asked.length, 2);
  const mine = efforts(7);

  asked[0].resolve({ level: "old" });
  await before;
  assert.equal(efforts(7), mine, "the new shell's record is the one standing");
  assert.equal(efforts(7).said, undefined, "nothing of the old answer in it");
  assert.deepEqual(said.repainted, []);

  // The old question ending does not take the new one's mark with it: a third
  // caller is still told to wait for the new answer instead of asking again.
  const third = context.refreshPaneEffort(7);
  assert.equal(asked.length, 2, "one question per pane at a time");

  asked[1].resolve({ level: "new" });
  assert.deepEqual(plain(await after), { level: "new" });
  assert.deepEqual(plain(await third), { level: "new" });
  assert.deepEqual(plain(efforts(7).said), { level: "new" });
  assert.deepEqual(said.repainted, [7]);
});

test("a pane is asked one question at a time, and the mark clears when it is answered", async () => {
  const { context, asked, said, efforts } = windowAnswered();
  const first = context.refreshPaneEffort(7);
  const second = context.refreshPaneEffort(7);
  assert.equal(asked.length, 1);
  assert.ok(efforts(7).asking, "the question is marked while it is out");

  asked[0].resolve({ level: "medium" });
  assert.deepEqual(plain(await first), { level: "medium" });
  assert.deepEqual(plain(await second), { level: "medium" });
  assert.equal(efforts(7).asking, null);
  assert.deepEqual(said.repainted, [7]);

  // The answer is asked for again once the mark is clear.
  const again = context.refreshPaneEffort(7);
  assert.equal(asked.length, 2);
  asked[1].resolve({ level: "medium" });
  await again;
  assert.deepEqual(said.repainted, [7], "the same answer repaints nothing");
});

test("a rejected answer is an empty one, and a backend that fails at once is safe", async () => {
  const rejected = windowAnswered();
  const pending = rejected.context.refreshPaneEffort(7);
  rejected.asked[0].reject(new Error("no pane"));
  assert.equal(await pending, null);
  assert.equal(rejected.efforts(7).said, null);

  // `invoke` throwing before it returns a promise must not read the question's own
  // mark before it exists.
  const sudden = windowAnswered({
    invoke: () => {
      throw new Error("no bridge");
    },
  });
  assert.equal(await sudden.context.refreshPaneEffort(8), null);
  assert.equal(sudden.efforts(8).said, null);
  assert.equal(sudden.efforts(8).asking, null, "the mark is clear");
});

test("a press answered after its pane ended writes nothing", async () => {
  const { context, asked, said, efforts, ends, holds } = windowAnswered();
  context.rememberPaneEffort(7, { level: "low" });
  said.repainted.length = 0;

  const pressing = context.movePaneEffort({ term: 7 }, "high");
  assert.equal(asked[0].command, "pane_effort_press");
  ends(7);
  asked[0].resolve({ level: "high" });
  await pressing;

  assert.equal(efforts(7), null);
  assert.deepEqual(holds(7), []);
  assert.deepEqual(said.repainted, []);
});

test("a press the backend refuses, after its pane ended, asks nothing again", async () => {
  const { context, asked, said, ends } = windowAnswered();
  context.rememberPaneEffort(7, { level: "low" });

  const pressing = context.movePaneEffort({ term: 7 }, "high");
  ends(7);
  asked[0].reject(new Error("the picker did not move"));
  await pressing;

  assert.deepEqual(said.errors, ["refused: Error: the picker did not move"], "the person who pressed is still told");
  assert.equal(asked.length, 1, "and no question is asked for a pane that is gone");
});

test("on a live pane a press writes its answer, and a refused one asks again", async () => {
  const { context, asked, said, efforts } = windowAnswered();
  context.rememberPaneEffort(7, { level: "low" });
  said.repainted.length = 0;

  const pressed = context.movePaneEffort({ term: 7 }, "high");
  asked[0].resolve({ level: "high" });
  await pressed;
  assert.deepEqual(plain(efforts(7).said), { level: "high" });
  assert.deepEqual(said.repainted, [7]);

  const refused = context.movePaneEffort({ term: 7 }, "max");
  asked[1].reject(new Error("busy"));
  await refused;
  assert.deepEqual(said.errors, ["refused: Error: busy"]);
  assert.equal(asked[2].command, "pane_effort", "a refusal asks the backend what the effort is now");
});
