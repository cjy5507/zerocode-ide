import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import vm from "node:vm";

const source = await readFile(new URL("../shell-status.js", import.meta.url), "utf8");
const start = source.indexOf("const scmNotices = new Map();");
const end = source.indexOf('listen("scm:notices"', start);
assert.ok(start >= 0 && end > start, "SCM notifications use the existing toast surface");
const shown = [];
const context = vm.createContext({
  Map, Set,
  t: (_key, fallback, values) => fallback.replace(/\{\{(\w+)\}\}/g, (_, k) => values[k]),
  toast: (text, kind, options) => {
    const note = { text, kind, options, removed: false, remove() { this.removed = true; } };
    shown.push(note); return note;
  },
});
vm.runInContext(source.slice(start, end), context);
const paint = (rows) => { context.rows = rows; vm.runInContext("paintScmNotices(rows)", context); };
const conflict = { id: "pr:mergeable", root: "/wt/topic", number: 12, kind: "mergeable", opened_at: 1, resolved_at: null };
paint([conflict]); paint([conflict]);
assert.equal(shown.length, 1, "identical observations show one toast");
assert.equal(shown[0].options.sticky, true);
shown[0].options.action.run();
paint([conflict]);
assert.equal(shown.length, 1, "manual dismissal does not rearm the fact");
paint([{ ...conflict, resolved_at: 2 }]);
assert.ok(shown[0].removed, "resolution closes the existing toast");
paint([{ ...conflict, opened_at: 3 }]);
assert.equal(shown.length, 2, "a new conflict after resolution rearms");
paint([]);
assert.ok(shown[1].removed, "a removed subject closes its surface");
console.log("SCM notice lifecycle: passed");
