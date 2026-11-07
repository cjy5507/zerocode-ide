/* The door a shell's end goes through, with the collections the window really
 * declares (t-99473).
 *
 * `pane-lifecycle.test.mjs` holds the lifecycle to account in a bare context,
 * and `pane-lifecycle.mjs` closes real shells in a real page. This is the span
 * between them, small enough to run with no browser: the window's OWN
 * declarations — every `const … = paneLifecycle.map/set(…)` in `ui/*.js`, cut
 * out as written — and the production `dropTermView`, run together with the
 * effects that are not collections stood in by counters.
 *
 * Every declared collection is filled for the pane that ends, whatever it is
 * called, so a collection declared tomorrow is covered the day it is declared.
 * A collection this test cannot fill fails it, with what to add: a collection no
 * test fills is one nobody knows is released.
 *
 *   node --test ui/tests/pane-close-door.test.mjs */
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
import { parse } from "acorn";
import { scriptsOf } from "../../scripts/window-scope.mjs";

const here = new URL("../", import.meta.url);
const lifecycle = readFileSync(new URL("shell-pane-lifecycle.js", here), "utf8");
const windowFiles = scriptsOf(readFileSync(new URL("index.html", here), "utf8"))
  .map(one => one.name).filter(name => !name.startsWith("vendor/"));
const sources = Object.fromEntries(windowFiles.map((name) => [name, readFileSync(new URL(name, here), "utf8")]));
const programs = Object.fromEntries(Object.entries(sources).map(([name, text]) => [name, parse(text, { ecmaVersion: "latest" })]));
const plain = (value) => JSON.parse(JSON.stringify(value));

/* The window's declarations of what a pane keeps, as written. */
function declarations() {
  const found = [];
  for (const [file, text] of Object.entries(sources)) {
    for (const statement of programs[file].body) {
      if (statement.type !== "VariableDeclaration") continue;
      for (const entry of statement.declarations) {
        const call = entry.init;
        if (call?.type !== "CallExpression") continue;
        const callee = call.callee;
        const hook = callee.type === "Identifier" && callee.name === "paneHookStates";
        const collection = callee.type === "MemberExpression" && callee.object.name === "paneLifecycle"
          && ["map", "set"].includes(callee.computed ? callee.property.value : callee.property.name);
        if (!hook && !collection) continue;
        assert.equal(entry.id.type, "Identifier", "a pane collection needs a named binding for the disposal test");
        found.push({ name: entry.id.name, file, text: `${statement.kind} ${text.slice(entry.start, entry.end)};` });
      }
    }
  }
  return found;
}

function production(name) {
  for (const [file, text] of Object.entries(sources)) {
    const node = programs[file].body.find(node => node.type === "FunctionDeclaration" && node.id?.name === name);
    if (node) return text.slice(node.start, node.end);
  }
  assert.fail(`the production ${name} function is present in ui/`);
}

/* The keys a pane has in the collections whose key is not the bare id. */
const keysFor = (name, term) => (
  { acknowledgedPanes: [`term:${term}`], paneActivities: [`term:${term}`, `sub:${term}:helper`] }[name] ?? [term]
);

/* The value an entry of each collection holds, where it has to have a shape: a
 * conversation page and a placed worker hand their page and timer to `release`. */
const SHAPES = {
  paneChats: (term) => ({ host: null, tab: { pane: `chat:${term}` }, run: { queue: ["waiting"], id: `chat:${term}` } }),
  placedWorkers: () => ({ worker: "w", seenAfterMs: 0, seen: false, seenTimer: 123 }),
};

function windowWithDeclarations() {
  const said = { board: 0, clock: 0, screens: [], released: [], chatsLetGo: [] };
  const context = vm.createContext({
    askBoardAfterClose: () => { said.board += 1; },
    agentClock: { sync: () => { said.clock += 1; } },
    dropTermScreen: (term) => said.screens.push(term),
    previewedTerms: new Set([7, 8, 9]),
    // What a conversation page lets go of, stood in for: the page itself is the
    // browser suite's. A page is let go through these four.
    dropWorkerScreen: (pane) => said.chatsLetGo.push(pane),
    standBehind: () => {},
    dropComposerRelease: () => {},
    clearQueueStartWatch: () => {},
    clearTimeout: (handle) => said.released.push(handle),
  });
  vm.runInContext(lifecycle, context);
  for (const name of ["releasePaneChat", "cardTerm"]) vm.runInContext(production(name), context);
  const declared = declarations();
  for (const { text } of declared) vm.runInContext(text, context);
  vm.runInContext(production("dropTermView"), context);
  const run = (code) => vm.runInContext(code, context);
  return {
    context, said, declared: declared.map((one) => one.name), run,
    names: () => Object.keys(plain(run("paneLifecycle.sizes()"))),
    holds: (term) => plain(run(`paneLifecycle.holds(${term})`)),
    end: (term) => run(`dropTermView(${term})`),
  };
}

/* Fill every declared collection for `term`. */
function fill(w, term, { skip = [] } = {}) {
  for (const name of w.names()) {
    // The hook state's observation clock and snapshot marks are filled with it.
    if (skip.includes(name) || name === "hookObservations" || name === "hookSnapshotMarks") continue;
    if (w.run(`${name} instanceof Set`)) {
      w.run(`${name}.add(${term})`);
    } else if (name === "hookStates") {
      w.run(`hookStates.set(${term}, "done"); hookStates.observe(${term}, 5); hookStates.noteSnapshot(${term});`);
    } else {
      for (const key of keysFor(name, term)) {
        w.context.__key = key;
        w.context.__value = SHAPES[name]?.(term) ?? `${name}-${term}`;
        w.run(`${name}.set(__key, __value)`);
      }
    }
  }
}

test("the window declares what a pane keeps with the lifecycle, and the declarations are read as written", () => {
  const w = windowWithDeclarations();
  assert.ok(w.names().length >= 36, `the declarations were found: ${w.names().length}`);
  assert.deepEqual([...new Set(w.names())].length, w.names().length, "a name is declared once");
  // A few that the omissions were about.
  for (const name of ["typedInto", "hushedPanes", "paneLive", "paneNow", "paneChats", "hookStates", "hookObservations"]) {
    assert.ok(w.names().includes(name), `${name} is declared with the lifecycle`);
  }
});

test("every collection the window declares is filled for the pane that ends, so a new declaration cannot slip past this test", () => {
  const w = windowWithDeclarations();
  fill(w, 7);
  const unfilled = w.names().filter((name) => !w.holds(7).includes(name));
  assert.deepEqual(unfilled, [], "this test cannot fill these: give each a key or a shape in `keysFor` or `SHAPES`");
});

test("the door releases every declared collection of the pane that ended and nothing of its neighbour, and runs the effects that are not collections", () => {
  const w = windowWithDeclarations();
  fill(w, 7);
  fill(w, 8);
  const neighbour = w.holds(8);

  w.end(7);

  assert.deepEqual(w.holds(7), [], "nothing of the shell is left under any name");
  assert.deepEqual(w.holds(8), neighbour, "the neighbour keeps all it had");
  assert.deepEqual(w.said.chatsLetGo, ["chat:7"], "its conversation page was let go, and only its");
  assert.deepEqual(w.said.released, [123], "the placed worker's timer was let go");
  assert.deepEqual([w.said.board, w.said.clock, w.said.screens], [1, 1, [7]]);
  assert.equal(w.run("previewedTerms.has(7)"), false);
  assert.equal(w.run("previewedTerms.has(8)"), true);
});

test("a pane that never had a conversation page is released like any other: its live words, its status and the rest", () => {
  // A pane whose window was reloaded under it: its session is seeded, no hook
  // has come, and so no conversation record — but the agent keeps streaming.
  const w = windowWithDeclarations();
  fill(w, 9, { skip: ["paneChats", "paneUsage", "paneEfforts", "reviewedAnswers", "paneReviewSaid"] });
  assert.ok(w.holds(9).includes("paneLive") && w.holds(9).includes("paneNow"));
  assert.ok(!w.holds(9).includes("paneChats"));

  w.end(9);

  assert.deepEqual(w.holds(9), []);
  assert.deepEqual(w.said.chatsLetGo, [], "no page stood, so none was let go");
});

test("the time of the last key typed, and the hush on the last ring, go with the pane on the ordinary close road", () => {
  const w = windowWithDeclarations();
  w.run("typedInto.set(7, 1000); hushedPanes.set(7, 'batch');");
  w.end(7);
  assert.equal(w.run("typedInto.has(7) || hushedPanes.has(7)"), false);
});

test("a collection whose release fails still lets the shell end: every collection is emptied, the screen and the clock are asked, and the error goes on", () => {
  const w = windowWithDeclarations();
  fill(w, 7);
  w.run(`placedWorkers.set(7, { get seenTimer() { throw new Error("release failed on purpose"); } })`);

  assert.throws(() => w.end(7), /release failed on purpose/);

  assert.deepEqual(w.holds(7), []);
  assert.deepEqual([w.said.board, w.said.clock, w.said.screens], [1, 1, [7]], "the effects ran in spite of it");
  assert.equal(w.run("previewedTerms.has(7)"), false);
});

test("the door is safe to walk through again for a shell that has already ended", () => {
  const w = windowWithDeclarations();
  fill(w, 7);
  w.end(7);
  assert.doesNotThrow(() => w.end(7));
  assert.deepEqual(w.holds(7), []);
  assert.equal(w.said.chatsLetGo.length, 1, "the page was let go once");
});
