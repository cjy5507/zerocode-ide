import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import vm from "node:vm";

// The window says a pane's waiting mail was not typed once per condition, not
// once per mail (t-45801). The script's own listeners run here, with `listen`
// stubbed to keep each callback.
const source = await readFile(new URL("../shell-term.js", import.meta.url), "utf8");
const start = source.indexOf("const MAIL_WAITING_WORDS");
assert.ok(start >= 0, "the mail-waiting words live in the terminal's own script");
const listeners = [...source.matchAll(/listen\("term:(?:mail-waiting|mail-waiting-cleared|exited)", \(event\) => \{[\s\S]*?\n\}\);/g)];
const end = Math.max(start, ...listeners.map((match) => match.index + match[0].length));

const said = [];
const callbacks = new Map();
const context = vm.createContext({
  t: (_key, fallback, values) => fallback.replace(/\{\{(\w+)\}\}/g, (_, k) => values[k]),
  toast: (words) => { said.push(words); return null; },
  listen: (name, callback) => { callbacks.set(name, callback); },
});
vm.runInContext(source.slice(start, end), context);
const deliver = (name, payload) => callbacks.get(name)?.({ payload });

const holds = { term: 2, why: "holds_a_draft" };
deliver("term:mail-waiting", holds);
deliver("term:mail-waiting", holds);
deliver("term:mail-waiting", holds);
console.log(`MEASURE mail_toasts_for_three_mails_of_one_condition=${said.length}`);
assert.equal(said.length, 1, "three waiting mails for one condition say one toast");
assert.ok(said[0].includes("터미널 2의 에이전트에게 온 우편을 입력하지 않았습니다"), "the toast names its terminal");

deliver("term:mail-waiting", { term: 3, why: "holds_a_draft" });
assert.equal(said.length, 2, "another pane's condition is its own toast");
deliver("term:mail-waiting", { term: 2, why: "parked" });
assert.equal(said.length, 3, "a different reason on the same pane is a different condition");

deliver("term:mail-waiting-cleared", { term: 2 });
deliver("term:mail-waiting", holds);
assert.equal(said.length, 4, "a landed pointer ends the condition, so its return is said again");

deliver("term:exited", { term: 2 });
deliver("term:mail-waiting", { term: 2, why: "not_a_reason_we_know" });
assert.equal(said.length, 5, "a pane that exited starts its conditions afresh");
assert.ok(said[4].includes("우편함에 안전하게 있고, 에이전트가 다음 확인에서 읽습니다"), "an unknown reason reads as the plain one");
console.log("mail-waiting lifecycle: passed");
