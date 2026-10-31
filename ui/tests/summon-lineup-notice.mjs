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

const settingsSource = await readFile(new URL("../shell-settings.js", import.meta.url), "utf8");
const readStart = settingsSource.indexOf('listen("summon-lineup:read"');
const readEnd = settingsSource.indexOf("\n});", readStart) + "\n});".length;
assert.ok(readStart >= 0 && readEnd > readStart, "settings listens for summon-lineup:read");
let readHandler = null;
let typesafeRefreshes = 0;
let connectionRefreshes = 0;
const fakeAgentModelLists = new Map([["zo", { at: 1, rows: [{ id: "old" }] }]]);
const fakeBoardModelsAsked = new Set(["zo"]);
const fakeController = {
  busy: false,
  picking: null,
  setup: null,
  editing: null,
  deletion: null,
  refresh: () => { connectionRefreshes += 1; },
};
const readState = {
  agentModelLists: fakeAgentModelLists,
  boardModelsAsked: fakeBoardModelsAsked,
  settingsView: { hidden: false },
  settingsPane: "judgment-models",
  typesafeBusy: false,
  refreshTypeSafe: () => { typesafeRefreshes += 1; },
  modelConnectionsController: fakeController,
  listen: (_name, fn) => { readHandler = fn; },
};
vm.runInContext(settingsSource.slice(readStart, readEnd), vm.createContext(readState));
assert.equal(typeof readHandler, "function");

readHandler();
assert.equal(fakeAgentModelLists.size, 0, "summon-lineup:read clears cached composer model lists");
assert.equal(fakeBoardModelsAsked.size, 0, "summon-lineup:read clears cached board model lookups");
assert.equal(connectionRefreshes, 1, "open judgment-models pane refreshes when idle");
assert.equal(typesafeRefreshes, 0);

fakeController.setup = { title: "start" };
readHandler();
assert.equal(connectionRefreshes, 1, "active setup wizard is not clobbered by background lineup read");
fakeController.setup = null;

readState.settingsPane = "orchestration";
readHandler();
assert.equal(typesafeRefreshes, 1, "open orchestration pane refreshes on lineup read");

const connectionsSource = await readFile(new URL("../shell-connections.js", import.meta.url), "utf8");
assert.ok(connectionsSource.includes("this.zo?.otherLogins"), "login rows surface otherLogins models for non-zo logins");

console.log("summon lineup notice: passed");
