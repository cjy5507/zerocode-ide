import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import vm from "node:vm";

/* t-14437: a model that arrives in today's lineup is said once, in one line
 * per set of models whatever number of agents took it in, and a folded one
 * in a line of its own. */
const source = await readFile(new URL("../shell-status.js", import.meta.url), "utf8");
const start = source.indexOf('listen("summon-lineup:changed"');
const end = source.indexOf("\n});", start) + "\n});".length;
assert.ok(start >= 0 && end > start, "the lineup notice listens on the window's own event");
const shown = [];
let handler = null;
const context = vm.createContext({
  Map, Set, Array,
  t: (_key, fallback, values) => fallback.replace(/\{\{(\w+)\}\}/g, (_, k) => values[k]),
  toast: (text) => shown.push(text),
  listen: (_name, fn) => { handler = fn; },
});
vm.runInContext(source.slice(start, end), context);
assert.equal(typeof handler, "function");
handler({ payload: [
  { agent: "claude", entered: ["model-b"], folded: [] },
  { agent: "zo", entered: ["model-b"], folded: ["model-old"] },
  { agent: "codex", entered: [], folded: ["model-old"] },
] });
assert.equal(shown.length, 2, `one line for the arrival, one for the fold: ${shown.join(" | ")}`);
assert.ok(shown[0].includes("model-b") && shown[0].includes("claude·zo"), shown[0]);
assert.ok(shown[1].includes("model-old") && !shown[1].includes("model-old, model-old"), shown[1]);
handler({ payload: null });
assert.equal(shown.length, 2, "an empty event says nothing");
console.log("summon lineup notice: passed");
