import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import vm from "node:vm";

const source = await readFile(new URL("../shell-status.js", import.meta.url), "utf8");
const start = source.indexOf("/* PR conditions");
const end = source.indexOf('listen("scm:notices"', start);
assert.ok(start >= 0 && end > start, "SCM notifications use the existing toast surface");
const TOAST_MS = Number(source.match(/const TOAST_MS = (\d+);/)?.[1]);

// One window: its own copy of the block, its own toasts, marks and ledger calls.
function openWindow() {
  const shown = [];
  const calls = [];
  const elements = new Map();
  const context = vm.createContext({
    Map, Set,
    t: (_key, fallback, values) => fallback.replace(/\{\{(\w+)\}\}/g, (_, k) => values[k]),
    toast: (text, kind, options) => {
      const note = { text, kind, options, removed: false, remove() { this.removed = true; } };
      shown.push(note); return note;
    },
    invoke: (cmd, args) => { calls.push({ cmd, args }); return Promise.resolve(null); },
    el: (id) => {
      if (!elements.has(id)) {
        elements.set(id, { hidden: true, dataset: {}, attrs: {}, setAttribute(k, v) { this.attrs[k] = v; }, removeAttribute(k) { delete this.attrs[k]; } });
      }
      return elements.get(id);
    },
  });
  vm.runInContext(source.slice(start, end), context);
  return {
    shown, calls, elements,
    paint: (rows) => { context.rows = rows; vm.runInContext("paintScmNotices(rows)", context); },
  };
}

const conflict = { id: "pr:mergeable", root: "/wt/topic", checkout: "topic", number: 12, kind: "mergeable", opened_at: 1, resolved_at: null };

// Printed before any check below, so a failing run still reports the numbers.
{
  const first = openWindow();
  first.paint([conflict]);
  const ledger = first.calls.filter((call) => call.cmd === "scm_observer_shown").map(() => ({ ...conflict, shown_at: 7 }));
  const restarted = openWindow();
  restarted.paint(ledger.length ? ledger : [conflict]);
  console.log(`MEASURE restart_toasts=${restarted.shown.length} (first window said ${first.shown.length})`);
  console.log(`MEASURE toast_lifetime=${first.shown[0]?.options.sticky === true ? "infinite (sticky)" : `${TOAST_MS} ms`}`);
}

const w = openWindow();
w.paint([conflict]); w.paint([conflict]);
assert.equal(w.shown.length, 1, "identical observations show one toast");
assert.notEqual(w.shown[0].options.sticky, true, "a condition's toast leaves on its own");
assert.equal(w.shown[0].text, "PR #12 병합 충돌 · topic", "the words name the checkout, not its path");
assert.equal(w.shown[0].options.tip, "/wt/topic", "the full path is the tooltip");
assert.ok(w.calls.some((call) => call.cmd === "scm_observer_shown" && call.args.id === "pr:mergeable" && call.args.openedAt === 1),
  "showing a condition records the mark in the ledger");
assert.equal(w.elements.get("sb-scm-notices").hidden, false, "a standing condition is a mark on the status bar");
assert.ok(w.elements.get("sb-scm-notices-label").textContent.includes("1"), "the mark counts the standing conditions");
w.shown[0].options.action.run();
assert.ok(w.calls.some((call) => call.cmd === "scm_observer_dismiss" && call.args.id === "pr:mergeable" && call.args.openedAt === 1),
  "closing a toast records the close in the ledger");
w.paint([conflict]);
assert.equal(w.shown.length, 1, "manual dismissal does not rearm the fact");
assert.equal(w.elements.get("sb-scm-notices").hidden, false, "closing the toast keeps the condition on the status bar");
w.paint([{ ...conflict, resolved_at: 2 }]);
assert.ok(w.shown[0].removed, "resolution closes the existing toast");
assert.equal(w.elements.get("sb-scm-notices").hidden, true, "a resolved condition leaves the status bar");
w.paint([{ ...conflict, opened_at: 3 }]);
assert.equal(w.shown.length, 2, "a new conflict after resolution rearms");
w.paint([]);
assert.ok(w.shown[1].removed, "a removed subject closes its surface");

const restarted = openWindow();
restarted.paint([{ ...conflict, shown_at: 7 }]);
assert.equal(restarted.shown.length, 0, "a restarted window does not say a condition the ledger marked shown");
assert.equal(restarted.elements.get("sb-scm-notices").hidden, false, "a restarted window still shows the standing mark");
const closed = openWindow();
closed.paint([{ ...conflict, dismissed_at: 8 }]);
assert.equal(closed.shown.length, 0, "a condition the ledger marked closed stays silent");

const many = openWindow();
const ci = { id: "pr:ci", root: "/wt/other", checkout: "other", number: 4, kind: "ci", opened_at: 1, resolved_at: null };
many.paint([conflict, ci]);
assert.equal(many.shown[1].text, "PR #4 CI 실패 · other", "a CI failure names its checkout");
assert.equal(many.elements.get("sb-scm-notices-label").textContent, "PR 조건 2", "the mark counts both standing conditions");
many.paint([{ ...conflict, resolved_at: 2 }, ci]);
assert.equal(many.elements.get("sb-scm-notices-label").textContent, "PR 조건 1", "a resolved condition leaves the count");
many.paint([]);
assert.equal(many.elements.get("sb-scm-notices").hidden, true, "no standing condition hides the mark");
console.log("SCM notice lifecycle: passed");
