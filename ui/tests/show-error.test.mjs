/* The window's one error reporter never throws over the error it was handed
 * (t-94962, ZO's Windows CI 2026-11-05).
 *
 * `tabs` is a top-level `const` in shell-term.js. When that script stops while
 * it loads, before its `const tabs` line runs, V8 reads the never-initialized
 * binding as "tabs is not defined" from every other script. On a Windows run of
 * PR #15 a page's terminal script stopped early, `openTermTab`'s
 * `open_term_tab` then failed, and `showError` turned that failure into its
 * own ReferenceError — so neither the failure nor the person heard anything.
 *
 * The production functions run in a context of their own, with the toast and
 * placeholder hands counted. */
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const status = readFileSync(new URL("../shell-status.js", import.meta.url), "utf8");
const sources = ["openTabsIfLoaded", "showError"].map((name) => {
  const source = status.match(new RegExp(`^function ${name}\\(.*?^\\}`, "ms"))?.[0];
  assert.ok(source, `the production ${name} function is present in shell-status.js`);
  return source;
});

/* A window whose shell-term.js got as far as `loaded` says: its `const tabs`
 * line ran with `open` tabs, or the script stopped before it. */
function windowWhere(loaded, open = []) {
  const said = { toasts: [], logged: [], placeholder: 0, stage: 0 };
  const context = vm.createContext({
    String,
    stageError: null,
    console: { error: (error) => said.logged.push(String(error)) },
    standingToast: () => false,
    toast: (words, kind) => said.toasts.push({ words, kind }),
    paintStagePlaceholder: () => { said.placeholder += 1; },
    updateStage: () => { said.stage += 1; },
    __OPEN__: open,
  });
  if (loaded) {
    vm.runInContext("const tabs = __OPEN__;", context);
  } else {
    assert.throws(
      () => vm.runInContext('throw new Error("shell-term.js stopped while loading"); const tabs = [];', context),
      /stopped while loading/,
    );
    assert.throws(() => vm.runInContext("tabs.length", context), /tabs is not defined/,
      "the stopped script leaves tabs unreadable, as the Windows page did");
  }
  for (const source of sources) vm.runInContext(source, context);
  return { context, said };
}

test("a terminal script that stopped while loading: the failure is said, not replaced by a ReferenceError", () => {
  const { context, said } = windowWhere(false);
  assert.doesNotThrow(() => context.showError(new Error("open_term_tab refused")));
  assert.deepEqual(said.toasts, [{ words: "Error: open_term_tab refused", kind: "halt" }]);
  assert.deepEqual(said.logged, ["Error: open_term_tab refused"]);
  assert.equal(said.placeholder, 0, "the stage placeholder belongs to the script that stopped");
});

test("no tab open: the failure is held for the stage placeholder, as before", () => {
  const { context, said } = windowWhere(true, []);
  context.showError("refused");
  assert.equal(context.stageError, "refused");
  assert.equal(said.placeholder, 1);
  assert.equal(said.stage, 1);
  assert.deepEqual(said.toasts, []);
});

test("a tab open: the failure is a toast, as before", () => {
  const { context, said } = windowWhere(true, [{ id: "term:1" }]);
  context.showError("refused");
  assert.deepEqual(said.toasts, [{ words: "refused", kind: "halt" }]);
  assert.equal(said.placeholder, 0);
  assert.equal(context.stageError, null);
});
