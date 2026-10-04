/* The browser door's form scripts as the pane runs them, built from the Rust
 * they ship in — one builder for the door's own tests (browser-door.mjs)
 * and the form bench's drivers (tools/computer-bench/form-scenes/drive.mjs),
 * so neither restates a script, a table or a number the Rust owns:
 *
 * - `doorScript(request, body)` is `automation_script`'s shape;
 * - `FORM_REQUEST` is `form_request()`; `fieldsScript()`, `fillScript(entries)`
 *   and `evalFormScript(expression)` are `fields`, one pass of `fill` and an
 *   eval that names the form pair (`eval_script` + `inlined_eval_body`);
 * - `fillPasses(run, bundle)` is the window's `fill_passes`: the whole
 *   bundle, then every poll, inside the pending wait and the passes, what a
 *   later pass may still find.
 *
 * A constant the Rust does not hold reads null, and `need` names it. */

import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { rustList, rustNumber, rustText } from "./rust-source.mjs";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const source = (path) => readFile(resolve(ROOT, path), "utf8").catch(() => "");
const DOOR = await source("crates/zerocode-shell/src/cmd/browser.rs");
const FORM = await source("crates/zerocode-shell/src/cmd/browser/form.rs");
const CORE = await source("crates/zerocode-core/src/agent_browser.rs");
const FORM_CORE = await source("crates/zerocode-core/src/browser_form.rs");
const SCREEN = await source("crates/zerocode-core/src/screen_action.rs");
const JEV = await source("crates/zerocode-core/src/jev.rs");
const VALUE_QUESTION = JSON.parse((await source("crates/zerocode-core/fixtures/type-value/question.json")) || "{}");

export const need = (name, value) => {
  if (value === null || value === undefined || value === "" || (Array.isArray(value) && !value.length)) {
    throw new Error(`${name} is not in the Rust the pane runs`);
  }
  return value;
};

export const HELPERS = rustText(DOOR, "BROWSER_AUTOMATION_HELPERS");
export const MARK_HELPERS = rustText(DOOR, "BROWSER_MARK_HELPERS");
export const CLICK_BODY = rustText(DOOR, "CLICK_BODY");
export const FORM_HELPERS = rustText(FORM, "BROWSER_FORM_HELPERS");
export const FIELDS_BODY = rustText(FORM, "BROWSER_FIELDS_BODY");
export const FILL_HELPERS = rustText(FORM, "BROWSER_FILL_HELPERS");
export const FILL_BODY = rustText(FORM, "BROWSER_FILL_BODY");
export const EVAL_FORM = rustText(FORM, "BROWSER_EVAL_FORM");
export const EVAL_FORM_OBJECT = rustText(CORE, "BROWSER_EVAL_FORM_OBJECT");

/* The window's clock for a fill's passes (`fill_passes`). */
export const FILL_PENDING_MS = rustNumber(FORM_CORE, "BROWSER_FILL_PENDING_MS");
export const FILL_PASSES = rustNumber(FORM_CORE, "BROWSER_FILL_PASSES");
export const FILL_POLL_MS = rustNumber(CORE, "BROWSER_WAIT_POLL_MS");
/* The statuses another pass may still find (`FillStatus::tries_again`). */
export const TRIES_AGAIN = ["mismatch", "not_found", "no_option", "disabled"];

const CANDIDATE_CAP = rustNumber(JEV, "SCREEN_CANDIDATE_CAP");
export const FORM_REQUEST = {
  controls: rustList(FORM_CORE, "BROWSER_FORM_CONTROLS"), notFields: rustList(FORM_CORE, "BROWSER_FORM_NOT_FIELDS"),
  actions: rustList(FORM_CORE, "BROWSER_FORM_ACTIONS"), scopes: rustList(FORM_CORE, "BROWSER_FORM_SCOPES"),
  options: rustList(FORM_CORE, "BROWSER_FORM_OPTIONS"), days: rustList(FORM_CORE, "BROWSER_FORM_DAYS"),
  dayWords: rustList(FORM_CORE, "BROWSER_FORM_DAY_WORDS"), frameSeparator: rustText(FORM_CORE, "BROWSER_FORM_FRAME_SEPARATOR"),
  frameDepth: rustNumber(FORM_CORE, "BROWSER_FORM_FRAME_DEPTH"), captionDepth: rustNumber(FORM_CORE, "BROWSER_FORM_CAPTION_DEPTH"),
  on: rustList(FORM_CORE, "BROWSER_FILL_ON"), off: rustList(FORM_CORE, "BROWSER_FILL_OFF"),
  field: { regions: rustList(CORE, "BROWSER_FIELD_REGIONS"), headings: rustList(CORE, "BROWSER_FIELD_HEADINGS") },
  fieldCap: rustNumber(FORM_CORE, "BROWSER_FORM_FIELD_CAP"), actionCap: rustNumber(FORM_CORE, "BROWSER_FORM_ACTION_CAP"),
  optionCap: rustNumber(FORM_CORE, "BROWSER_FORM_OPTION_CAP"),
  wordCap: Math.floor(rustNumber(SCREEN, "SHOWS_CHAR_CAP") / CANDIDATE_CAP),
  valueCap: VALUE_QUESTION.valueCharCap, answerCap: rustNumber(DOOR, "BROWSER_CALLBACK_CAP"),
};

export const doorScript = (request, body) =>
  `(() => {\n${need("BROWSER_AUTOMATION_HELPERS", HELPERS)}\nconst request = ${JSON.stringify(request)};\ntry {\n${body}\n} catch (_) { return zcFail("evaluation_failed"); }\n})()`;
const formScript = (request, body) => {
  need("BROWSER_FORM_CONTROLS", FORM_REQUEST.controls);
  return doorScript(request, `${need("BROWSER_MARK_HELPERS", MARK_HELPERS)}\n${need("BROWSER_FORM_HELPERS", FORM_HELPERS)}\n${body}`);
};
export const fieldsScript = () => formScript(FORM_REQUEST, need("BROWSER_FIELDS_BODY", FIELDS_BODY));
export const fillScript = (entries) => formScript({ ...FORM_REQUEST, entries },
  `${need("BROWSER_FILL_HELPERS", FILL_HELPERS)}\n${need("BROWSER_FILL_BODY", FILL_BODY)}`);
/* `inlined_eval_body`: the expression written into the script, a thenable
 * refused, `undefined` said as such. */
export const evalFormScript = (expression) => formScript(FORM_REQUEST,
  `${need("BROWSER_FILL_HELPERS", FILL_HELPERS)}\n${need("BROWSER_EVAL_FORM", EVAL_FORM)}\n`
  + `const value = (\n${expression}\n);\nif (value && typeof value.then === "function") return zcFail("async_value");\n`
  + `const held = value === undefined ? { type: "undefined" } : value;\nreturn zcEncode({ ok: true, value: held });`);
export const clickScript = (selector) => doorScript({ selector, blockRoots: "", expect: null }, need("CLICK_BODY", CLICK_BODY));

/* `fill_passes` over `run(script) → page answer`: the whole bundle, then
 * what another pass may still find, every poll, until the wait or the
 * passes run out. Answers each entry's last word, what is left, the passes. */
export async function fillPasses(run, bundle) {
  const entries = Array.isArray(bundle) ? bundle : Object.entries(bundle).map(([handle, value]) => ({ handle, value }));
  const last = new Map();
  const began = Date.now();
  let asked = entries, left = [], passes = 0;
  while (asked.length) {
    const pass = await run(fillScript(asked));
    if (!pass.ok) throw new Error(`a fill pass was refused: ${JSON.stringify(pass)}`);
    passes += 1;
    for (const result of pass.value.results) last.set(result.handle, result);
    left = pass.value.left;
    asked = entries.filter((entry) => TRIES_AGAIN.includes(last.get(entry.handle)?.status));
    if (!asked.length || passes >= FILL_PASSES || Date.now() - began >= FILL_PENDING_MS) break;
    await new Promise((done) => setTimeout(done, FILL_POLL_MS));
  }
  return { results: entries.map((entry) => last.get(entry.handle)), left, passes };
}
