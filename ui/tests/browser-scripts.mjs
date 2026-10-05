/* The browser door's form scripts as the pane runs them, built from the Rust
 * they ship in — one builder for the door's own tests (browser-door.mjs)
 * and the form bench's drivers (tools/computer-bench/form-scenes/drive.mjs),
 * so neither restates a script, a table or a number the Rust owns:
 *
 * - `doorScript(request, body)` is `automation_script`'s shape;
 * - `FORM_REQUEST` is `form_request()`; `fieldsScript()`,
 *   `fillWriteScript(entries)` and `fillReadScript(held, epoch)` (the two
 *   halves of one pass of `fill`) and `evalFormScript(expression)` are `fields`,
 *   `fill` and an eval that names the form pair (`eval_script` +
 *   `inlined_eval_body`);
 * - `fillPasses(run, bundle, expect)` is the window's `fill_passes`: the
 *   whole bundle, held on its first pass to the form read (`expect`, a
 *   fingerprint), then every poll, inside the pending wait and the passes,
 *   what a later pass may still find — each pass the write, the page's settle
 *   (`settled`, the window's `settle_with`) when it wrote, and the read-back;
 * - `readScript`, `typeScript`, `waitScript` and `scrollScript` are the page
 *   scripts of those verbs, and `AGENT_CONTEXT` the paragraphs a launch
 *   prompt carries (`delegation::with_agent_selection_contract`) — what the
 *   form bench's stand-in window (form-scenes/door-desk.mjs) runs and tells.
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
const GUARDED = await source("crates/zerocode-core/src/guarded.rs");
const DELEGATION = await source("crates/zerocode-core/src/delegation.rs");
const GUEST_KEY = ((await source("ui/browser-guest-key.txt")) || "").trim();
const VALUE_QUESTION = JSON.parse((await source("crates/zerocode-core/fixtures/type-value/question.json")) || "{}");

export const need = (name, value) => {
  if (value === null || value === undefined || value === "" || (Array.isArray(value) && !value.length)) {
    throw new Error(`${name} is not in the Rust the pane runs`);
  }
  return value;
};

export const HELPERS = rustText(DOOR, "BROWSER_AUTOMATION_HELPERS");
export const OBSERVE_HELPERS = rustText(DOOR, "BROWSER_OBSERVE_HELPERS");
export const MARK_HELPERS = rustText(DOOR, "BROWSER_MARK_HELPERS");
export const CLICK_BODY = rustText(DOOR, "CLICK_BODY");
export const TYPE_BODY = rustText(DOOR, "TYPE_BODY");
/* The paragraphs `with_agent_selection_contract` adds to a launch prompt: the
 * orchestration contract, then the surfaces the window owns. */
export const AGENT_CONTEXT = rustText(DELEGATION, "AGENT_SELECTION_CONTEXT");
export const FORM_HELPERS = rustText(FORM, "BROWSER_FORM_HELPERS");
export const FIELDS_BODY = rustText(FORM, "BROWSER_FIELDS_BODY");
export const FILL_HELPERS = rustText(FORM, "BROWSER_FILL_HELPERS");
export const FILL_BODY = rustText(FORM, "BROWSER_FILL_BODY");
/* A press by selector in a form its agent read: what the page says before it and what it says after
 * the page has settled (`press_in_form`). */
export const PRESS_BEFORE_BODY = rustText(FORM, "BROWSER_PRESS_BEFORE_BODY");
export const PRESS_AFTER_BODY = rustText(FORM, "BROWSER_PRESS_AFTER_BODY");
export const EVAL_FORM = rustText(FORM, "BROWSER_EVAL_FORM");
export const EVAL_FORM_OBJECT = rustText(CORE, "BROWSER_EVAL_FORM_OBJECT");
/* What the window settles a page by after a press (`settle_after_press`). */
export const SETTLE_BODY = rustText(DOOR, "BROWSER_SETTLE_BODY");
export const SETTLE_MS = rustNumber(CORE, "BROWSER_SETTLE_MS");
export const SETTLE_QUIET_MS = rustNumber(CORE, "BROWSER_SETTLE_QUIET_MS");
export const SETTLE_BUSY = rustList(CORE, "BROWSER_SETTLE_BUSY");

/* The door's own sentences and limits a stand-in window answers with
 * (`automate_wait`, `input_said`, `checked_expression`). */
export const CLICK_SAID = rustText(DOOR, "CLICK_SAID");
export const TYPED_SAID = rustText(DOOR, "TYPED_SAID");
export const WAIT_TIMED_OUT = rustText(DOOR, "WAIT_TIMED_OUT");
export const EXPRESSION_CAP = rustNumber(DOOR, "BROWSER_EXPRESSION_CAP");
export const WAIT_DEFAULT_MS = rustNumber(CORE, "BROWSER_WAIT_DEFAULT_MS");
export const WAIT_MIN_MS = rustNumber(CORE, "BROWSER_WAIT_MIN_MS");
export const WAIT_MAX_MS = rustNumber(CORE, "BROWSER_WAIT_MAX_MS");

/* The window's clock for a fill's passes (`fill_passes`). */
export const FILL_PENDING_MS = rustNumber(FORM_CORE, "BROWSER_FILL_PENDING_MS");
export const FILL_PASSES = rustNumber(FORM_CORE, "BROWSER_FILL_PASSES");
export const FILL_POLL_MS = rustNumber(CORE, "BROWSER_WAIT_POLL_MS");
/* The statuses another pass may still find (`FillStatus::tries_again`). */
export const TRIES_AGAIN = ["mismatch", "not_found", "no_option", "disabled"];

const CANDIDATE_CAP = rustNumber(JEV, "SCREEN_CANDIDATE_CAP");
/* `guarded::HELD_ROWS`: the rows of the window's one table of presses that
 * cannot be taken back, whole and in the table's order — a row the source
 * lacks makes the whole list null, so `need` names it, not a shorter list. */
const HELD_ROWS = ["PAYMENT_WORDS", "TRANSFER_WORDS", "DELETE_WORDS", "COMMIT_WORDS"].map((row) => rustList(GUARDED, row));
export const FORM_REQUEST = {
  holds: HELD_ROWS.every(Array.isArray) ? HELD_ROWS.flat() : null,
  controls: rustList(FORM_CORE, "BROWSER_FORM_CONTROLS"), notFields: rustList(FORM_CORE, "BROWSER_FORM_NOT_FIELDS"),
  actions: rustList(FORM_CORE, "BROWSER_FORM_ACTIONS"), scopes: rustList(FORM_CORE, "BROWSER_FORM_SCOPES"),
  options: rustList(FORM_CORE, "BROWSER_FORM_OPTIONS"), days: rustList(FORM_CORE, "BROWSER_FORM_DAYS"),
  dayWords: rustList(FORM_CORE, "BROWSER_FORM_DAY_WORDS"), frameSeparator: rustText(FORM_CORE, "BROWSER_FORM_FRAME_SEPARATOR"),
  frameDepth: rustNumber(FORM_CORE, "BROWSER_FORM_FRAME_DEPTH"), captionDepth: rustNumber(FORM_CORE, "BROWSER_FORM_CAPTION_DEPTH"),
  monthDays: rustNumber(FORM_CORE, "BROWSER_FORM_MONTH_DAYS"), monthPages: rustNumber(FORM_CORE, "BROWSER_FORM_MONTH_PAGES"),
  lists: rustList(FORM_CORE, "BROWSER_FORM_LISTS"), listItems: rustList(FORM_CORE, "BROWSER_FORM_LIST_ITEMS"),
  pressables: rustList(FORM_CORE, "BROWSER_FORM_PRESSABLES"), scanCap: rustNumber(FORM_CORE, "BROWSER_FORM_SCAN_CAP"),
  live: rustList(FORM_CORE, "BROWSER_FORM_LIVE"), freshCap: rustNumber(FORM_CORE, "BROWSER_FORM_FRESH_CAP"),
  pieceCap: rustNumber(FORM_CORE, "BROWSER_FORM_PIECE_CAP"),
  watch: GUEST_KEY,
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
  // Every table and cap of the request, by name: a constant the reader
  // missed would otherwise surface as the page script's `evaluation_failed`.
  for (const [key, value] of Object.entries(FORM_REQUEST)) need(`form_request().${key}`, value);
  return doorScript(request, `${need("BROWSER_OBSERVE_HELPERS", OBSERVE_HELPERS)}\n${need("BROWSER_MARK_HELPERS", MARK_HELPERS)}\n`
    + `${need("BROWSER_FORM_HELPERS", FORM_HELPERS)}\n${body}`);
};
export const fieldsScript = () => formScript(FORM_REQUEST, need("BROWSER_FIELDS_BODY", FIELDS_BODY));
/* The two halves of one fill pass (t-41387): the write — held to `expect` — and,
 * once the page has settled, the read-back of what the write held, in the
 * document `epoch` it was made in. */
export const fillWriteScript = (entries, expect = null) => formScript({ ...FORM_REQUEST, entries, expect, phase: "write" },
  `${need("BROWSER_FILL_HELPERS", FILL_HELPERS)}\n${need("BROWSER_FILL_BODY", FILL_BODY)}`);
export const fillReadScript = (held, epoch) => formScript({ ...FORM_REQUEST, held, epoch, phase: "read" },
  `${need("BROWSER_FILL_HELPERS", FILL_HELPERS)}\n${need("BROWSER_FILL_BODY", FILL_BODY)}`);
/* The two halves of a press in a known form: before it, the document with the settle's watch and what the
 * form's boxes show; after the page has settled, the form read again with what stood before. */
export const pressBeforeScript = () => formScript(FORM_REQUEST, need("BROWSER_PRESS_BEFORE_BODY", PRESS_BEFORE_BODY));
export const pressAfterScript = (before) => formScript({ ...FORM_REQUEST, before }, need("BROWSER_PRESS_AFTER_BODY", PRESS_AFTER_BODY));
/* `settle_after_press`'s script: what the page says of itself on one settle poll. */
export const settleScript = () => doorScript({ watch: need("the settle's watch name", GUEST_KEY), busy: need("BROWSER_SETTLE_BUSY", SETTLE_BUSY).join(",") },
  `${need("BROWSER_OBSERVE_HELPERS", OBSERVE_HELPERS)}\n${need("BROWSER_SETTLE_BODY", SETTLE_BODY)}`);
/* `inlined_eval_body`: the expression written into the script, a thenable
 * refused, `undefined` said as such. */
const inlinedEvalBody = (expression) =>
  `const value = (\n${expression}\n);\nif (value && typeof value.then === "function") return zcFail("async_value");\n`
  + `const held = value === undefined ? { type: "undefined" } : value;\nreturn zcEncode({ ok: true, value: held });`;
export const evalFormScript = (expression) => formScript(FORM_REQUEST,
  `${need("BROWSER_FILL_HELPERS", FILL_HELPERS)}\n${need("BROWSER_EVAL_FORM", EVAL_FORM)}\n${inlinedEvalBody(expression)}`);
/* `eval_script`: an expression that names the form pair gets its helpers and
 * its object; any other expression's script is left as it was. */
export const evalScript = (expression) => expression.includes(`${need("BROWSER_EVAL_FORM_OBJECT", EVAL_FORM_OBJECT)}.`)
  ? evalFormScript(expression) : doorScript({}, inlinedEvalBody(expression));
export const clickScript = (selector) => doorScript({ selector, blockRoots: "", expect: null }, need("CLICK_BODY", CLICK_BODY));
/* `type`: the keys road, or — for `--value-stdin` — the setter road. */
export const typeScript = (selector, text, road) =>
  doorScript({ selector, text, road, blockRoots: "" }, need("TYPE_BODY", TYPE_BODY));
/* The caps `automate_read` cuts a read at. */
const READ_CAPS = {
  title: rustNumber(DOOR, "BROWSER_TITLE_CAP"), url: rustNumber(DOOR, "BROWSER_URL_CAP"),
  text: rustNumber(DOOR, "BROWSER_READ_CAP"), dom: rustNumber(DOOR, "BROWSER_DOM_CAP"),
};
/* `read`: `automate_read`'s body (a format string in the Rust, so copied
 * here with its caps read from the Rust): the page's title, address and
 * visible text, and with a selector the element's reduced DOM. */
export const readScript = (selector = null) => {
  for (const [name, cap] of Object.entries(READ_CAPS)) need(`the read's ${name} cap`, cap);
  return doorScript({ selector }, `
const selected = request.selector === null
  ? { element: document.body || document.documentElement }
  : zcSelect(request.selector);
if (selected.code) return zcFail(selected.code);
const element = selected.element;
const visibleText = typeof element.innerText === "string" ? element.innerText : element.textContent;
return zcEncode({ ok: true, value: {
  title: String(document.title || "").slice(0, ${READ_CAPS.title}),
  url: zcSafeUrl(location.href).slice(0, ${READ_CAPS.url}),
  text: String(visibleText || "").slice(0, ${READ_CAPS.text}),
  dom: request.selector === null ? null : zcDom(element).slice(0, ${READ_CAPS.dom})
} });
`);
};
/* `wait`: one look at whether the first match a person could see is there
 * (`automate_wait` repeats it on its own clock). */
export const waitScript = (selector) => doorScript({ selector }, `
const selected = zcSelect(request.selector);
if (selected.code === "invalid_selector") return zcFail("invalid_selector");
return zcEncode({ ok: true, value: !selected.code && !selected.hidden && zcVisible(selected.element) });
`);
/* `scroll`: the page scrolled to its top or bottom, by an offset, or a
 * selector's first match brought to the middle (`automate_scroll`). */
export const scrollScript = (request) => doorScript(request, `
if (request.kind === "selector") {
  const selected = zcSelect(request.selector);
  if (selected.code) return zcFail(selected.code);
  selected.element.scrollIntoView({ block: "center", inline: "center", behavior: "auto" });
} else if (request.kind === "top") {
  window.scrollTo(0, 0);
} else if (request.kind === "bottom") {
  const body = document.body ? document.body.scrollHeight : 0;
  window.scrollTo(0, Math.max(document.documentElement.scrollHeight, body));
} else {
  window.scrollBy(Number(request.dx) || 0, Number(request.dy) || 0);
}
return zcEncode({ ok: true, value: { x: Math.round(window.scrollX), y: Math.round(window.scrollY) } });
`);

/* `settle_with`: poll the page's own facts (`settleScript`) until the document
 * a write was made in has stood still for the quiet window since the write
 * (`since`, the page's clock), or the wall runs out. Answers how it ended, as
 * the core words it: `ready`, `not_ready` or `invalidated`, and why. */
export async function settled(run, epoch, since) {
  const began = Date.now();
  const from = typeof since === "number" ? since : -Infinity;
  const wait = (ms) => new Promise((done) => setTimeout(done, Math.max(0, ms)));
  let why = "unanswered";
  const ended = (state, said) => ({ state, why: said, ms: Date.now() - began });
  for (;;) {
    if (SETTLE_MS - (Date.now() - began) <= 0) return ended("not_ready", why);
    let facts = null;
    try { facts = (await run(settleScript())).value; } catch { /* the page did not answer this poll */ }
    if (!facts) { await wait(Math.min(SETTLE_QUIET_MS, SETTLE_MS - (Date.now() - began))); continue; }
    if (facts.documentEpoch !== epoch) return ended("invalidated", "replaced");
    if (!facts.watched) return ended("not_ready", "unwatched");
    const still = facts.now - Math.max(facts.last, from);
    if (!facts.busy && still >= SETTLE_QUIET_MS) return ended("ready", "quiet");
    why = facts.busy ? "busy" : "moving";
    const next = facts.busy ? SETTLE_QUIET_MS : Math.min(Math.max(Math.ceil(SETTLE_QUIET_MS - still), 1), SETTLE_QUIET_MS);
    await wait(Math.min(next, SETTLE_MS - (Date.now() - began)));
  }
}

/* `fill_passes` over `run(script) → page answer`: the whole bundle, held on
 * its first pass to `expect`, then what another pass may still find, every
 * poll, until the wait or the passes run out. A pass is its write, the page's
 * settle when it wrote something, and the read-back of what the write held.
 * Answers each entry's last word, what is left, the passes, whether the form
 * was stale, its fingerprint and its buttons after the last pass, whether the
 * page was still changing when the last settle ended (`moving`) — and each
 * `round`: the handles a pass was asked and what the page answered, for the
 * ledger that says the answer. */
export async function fillPasses(run, bundle, expect = null) {
  const entries = Array.isArray(bundle) ? bundle : Object.entries(bundle).map(([handle, value]) => ({ handle, value }));
  const last = new Map();
  const rounds = [];
  const began = Date.now();
  let asked = entries, left = [], passes = 0, fingerprint = "", actions = [], moving = false, alerts = [];
  while (asked.length) {
    const written = await run(fillWriteScript(asked, passes === 0 ? expect : null));
    if (!written.ok) throw new Error(`a fill pass was refused: ${JSON.stringify(written)}`);
    passes += 1;
    if (written.value.stale) {
      fingerprint = written.value.fingerprint;
      rounds.push({ asked: asked.map((entry) => entry.handle), pass: { stale: true, fingerprint } });
      return { results: entries.map(() => undefined), left: [], passes, stale: true, fingerprint, actions: [], moving: false, alerts: [], rounds };
    }
    const heard = written.value.wrote ? await settled(run, written.value.epoch, written.value.at) : null;
    const read = await run(fillReadScript(written.value.before ? { held: written.value.held, before: written.value.before } : written.value.held, written.value.epoch));
    if (!read.ok) throw new Error(`a fill pass was refused: ${JSON.stringify(read)}`);
    const pass = { ...read.value, moving: heard === null ? null : heard.state !== "ready" };
    fingerprint = pass.fingerprint;
    rounds.push({ asked: asked.map((entry) => entry.handle), pass });
    for (const result of pass.results) last.set(result.handle, result);
    left = pass.left;
    actions = pass.actions || [];
    if (pass.moving !== null) { moving = pass.moving; alerts = pass.alerts || []; }
    asked = entries.filter((entry) => TRIES_AGAIN.includes(last.get(entry.handle)?.status));
    if (!asked.length || passes >= FILL_PASSES || Date.now() - began >= FILL_PENDING_MS) break;
    await new Promise((done) => setTimeout(done, FILL_POLL_MS));
  }
  return { results: entries.map((entry) => last.get(entry.handle)), left, passes, stale: false, fingerprint, actions, moving, alerts, rounds };
}

/* `press_in_form` over `run(script) → page answer`: a press by selector in a form its agent read — the
 * page's text and the settle's watch before it, the press, the page's settle, then the form read again
 * with what stood before. Answers the press's own report, how the page settled, whether it was still
 * changing, and what the page said. */
export async function pressInForm(run, selector) {
  const stood = await run(pressBeforeScript());
  if (!stood.ok) throw new Error(`a press was refused before it was made: ${JSON.stringify(stood)}`);
  const pressed = await run(clickScript(selector));
  if (!pressed.ok) throw new Error(`click refused: ${JSON.stringify(pressed)}`);
  const heard = await settled(run, stood.value.epoch, pressed.value.pressedAt);
  const read = await run(pressAfterScript(stood.value.before));
  if (!read.ok) throw new Error(`the form was not read after a press: ${JSON.stringify(read)}`);
  return { pressed: pressed.value, settle: heard, read: read.value, moving: heard.state !== "ready" };
}
