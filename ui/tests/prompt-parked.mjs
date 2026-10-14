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

// The window's own words are not the person's to paste (t-52566): the mail
// pointer's advice that ran out of time at a restored zo worker came back as
// an error and landed on the person's clipboard. A timeout of the window's
// words is the sender's to answer — no error, nothing copied, even had words
// come with it; a refusal of them is still said, without the words; and a
// park of them is not the person's to hear about.
const pointer = "\nYou have 1 orchestration message. Run `zerocode-orc check`.\n";
const before = { toasts: toasts.length, errors: errors.length, copied: copied.length };
deliver({ term: 11, delivered: false, pasted: false, why: "not_ready", text: null, theirs: false });
deliver({ term: 14, delivered: false, pasted: false, why: "not_ready", text: pointer, theirs: false });
assert.equal(errors.length, before.errors, "the window's own words that ran out of time were said as the person's loss");
assert.equal(copied.length, before.copied, "the window's own words that ran out of time reached the clipboard");
deliver({ term: 12, delivered: false, pasted: false, why: "holds_a_draft", text: pointer, theirs: false });
assert.equal(toasts.length, before.toasts + 1, "a refusal of the window's own words is no longer said");
assert.equal(copied.length, before.copied, "a refusal handed the window's own words to the clipboard");
deliver({ term: 13, delivered: false, pasted: false, why: "busy", text: null, theirs: false });
assert.equal(toasts.length, before.toasts + 1, "the window's own words parked behind a working agent were announced");
// The person's own words keep their road, said as theirs.
deliver({ term: 15, delivered: false, pasted: false, why: "not_ready", text: "my own words", theirs: true });
assert.deepEqual(copied.slice(before.copied), ["my own words"], "the person's own words that ran out of time left the clipboard road");
assert.equal(errors.length, before.errors + 1, "the person's own lost words are no longer said");
console.log("prompt parked behind a working agent: passed");
