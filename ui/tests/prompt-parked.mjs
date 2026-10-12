import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import vm from "node:vm";

// A prompt sent to an agent that is still working is parked, not lost
// (t-47952): the pump says `why: "busy"` the round it parks the words, and
// the window says they go in when the turn ends — no error, nothing on the
// clipboard. A timeout keeps the clipboard road, now named `not_ready`. The
// script's own listener runs here, with `listen` stubbed to keep it.
const source = await readFile(new URL("../shell.js", import.meta.url), "utf8");
const start = source.indexOf("const TERM_WITHHELD = Object.freeze({");
assert.ok(start >= 0, "the withheld table lives in the window's own script");
const listener = source.slice(start).match(/listen\("term:prompt", \(event\) => \{[\s\S]*?\n\}\);/);
assert.ok(listener, "the window listens for the pump's verdict on a prompt");
const end = start + listener.index + listener[0].length;

const toasts = [];
const errors = [];
const copied = [];
const callbacks = new Map();
const context = vm.createContext({
  t: (_key, fallback, values = {}) => fallback.replace(/\{\{(\w+)\}\}/g, (_, k) => values[k]),
  toast: (words, kind = "") => { toasts.push({ words, kind }); return { dataset: {} }; },
  showError: (error) => { errors.push(String(error)); },
  clipboardText: { write: (text) => { copied.push(text); return Promise.resolve(); } },
  listen: (name, callback) => { callbacks.set(name, callback); },
});
vm.runInContext(source.slice(start, end), context);
const deliver = (payload) => callbacks.get("term:prompt")?.({ payload });

// Parked: one plain notice that the words go in at the turn's end.
deliver({ term: 3, delivered: false, pasted: false, why: "busy", text: null });
assert.deepEqual(
  toasts.map((note) => note.words),
  ["터미널 3의 에이전트가 일하는 중이라 끝나면 보냅니다."],
  "a parked prompt says it goes in when the agent's turn ends",
);
assert.equal(toasts[0].kind, "", "a parked prompt is a report, not a failure");
assert.deepEqual(errors, [], "a parked prompt is no error");
assert.deepEqual(copied, [], "a parked prompt's words are still on their way, not on the clipboard");

// Its landing, a turn later, says nothing more.
deliver({ term: 3, delivered: true, pasted: true, why: null, text: null });
assert.equal(toasts.length + errors.length, 1, "a parked prompt that lands is not said twice");

// A timeout keeps the clipboard road, named or not.
deliver({ term: 3, delivered: false, pasted: false, why: "not_ready", text: "look at this page" });
assert.deepEqual(copied, ["look at this page"], "a timed-out prompt's words reach the clipboard");
assert.equal(errors.length, 1, "a timed-out prompt is said as lost");
assert.ok(errors[0].includes("터미널 3의 에이전트가 프롬프트를 받지 못했습니다"), "the timeout names its terminal");
assert.ok(errors[0].includes("클립보드"), "the timeout says where the words went");
deliver({ term: 5, delivered: false, pasted: false, why: null, text: "and this" });
assert.deepEqual(copied, ["look at this page", "and this"], "a timeout without a word keeps the same road");
assert.equal(errors.length, 2);

// A refusal is still the guard's, worded by its table.
deliver({ term: 4, delivered: false, pasted: false, why: "holds_a_draft", text: "mail" });
assert.equal(toasts.length, 2, "a refusal is still a toast of its own");
assert.ok(toasts[1].words.includes("입력 중인 글이 있어"), "a refusal is worded by the withheld table");
console.log("prompt parked behind a working agent: passed");
