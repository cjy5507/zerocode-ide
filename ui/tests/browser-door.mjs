/* The browser door's page scripts, run for real in Chromium.
 *
 * `crates/zerocode-shell/src/cmd/browser.rs` ships the helpers every verb's
 * page script begins with (`BROWSER_AUTOMATION_HELPERS`) and the bodies of
 * click and wait. This file reads those sources the way `browser-ring.mjs`
 * reads the menu — the string the Rust hands the pane, not a copy — and proves
 * one rule against a page that keeps hidden twins of its controls: the element
 * a selector names is the FIRST ONE A PERSON COULD SEE. A responsive admin
 * (a company admin, 2026-09-15) puts a `display: none` copy of its search form first in
 * the document; a door that took the document's first match clicked nothing,
 * typed into a field nobody reads, and timed a `wait` out with the control on
 * screen the whole time.
 *
 * The shared find highlighter is also exercised against a changing DOM: a
 * Flow must not reuse an earlier text count after its preceding action.
 *
 *   node ui/tests/browser-door.mjs
 */

import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "./playwright-chromium.mjs";

const UI = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const DOOR = await readFile(resolve(UI, "../crates/zerocode-shell/src/cmd/browser.rs"), "utf8");
const RUNTIME = await readFile(resolve(UI, "../crates/zerocode-shell/src/browser_runtime.rs"), "utf8");
const FIND_INSTALL = RUNTIME.match(/const BROWSER_FIND_INSTALL: &str = r##"([\s\S]*?)"##;/)[1];
const HELPERS = DOOR.match(/const BROWSER_AUTOMATION_HELPERS: &str = r#"([\s\S]*?)"#;/)[1];
const CLICK_BODY = DOOR.match(/const CLICK_BODY: &str = r#"([\s\S]*?)"#;/)[1];
// The wait's body is written inline in `automate_wait`; it is the raw string
// right after the door's own words about the picker.
const WAIT_BODY = DOOR.match(/could see\. A page whose only visible match stands behind a hidden\n\s*\/\/ twin used to time this wait out with the control on screen\.\n\s*r#"([\s\S]*?)"#,/)[1];

/* Exactly `automation_script`'s shape (browser.rs): helpers, the request, the
 * body inside one try. Built here from the same three pieces so the test runs
 * the string the pane runs. */
const script = (request, body) =>
  `(() => {\n${HELPERS}\nconst request = ${JSON.stringify(request)};\ntry {\n${body}\n} catch (_) { return zcFail("evaluation_failed"); }\n})()`;

const SELECT_BODY = `
const selected = zcSelect(request.selector);
return zcEncode({ ok: true, value: {
  code: selected.code || null, hidden: !!selected.hidden,
  id: selected.element ? selected.element.id || null : null } });
`;

const FIXTURE = `<!doctype html><title>door fixture</title>
<style>.gone { display: none } .clear { opacity: 0 }</style>
<form class="twin gone"><input class="who" id="who-hidden"><button class="go" id="go-hidden">Search</button></form>
<form class="twin"><input class="who" id="who-seen"><button class="go" id="go-seen">Search</button></form>
<button class="ghost gone" id="ghost-a">a</button><button class="ghost" hidden id="ghost-b">b</button>
<button class="faint clear" id="faint">c</button>
<p id="log"></p>
<script>
  for (const button of document.querySelectorAll("button")) {
    button.addEventListener("click", (event) => {
      event.preventDefault();
      document.getElementById("log").textContent += event.currentTarget.id + ";";
    });
  }
</script>`;

const results = [];
const pass = (name, detail = "") => results.push({ name, pass: true, detail });
const fail = (name, detail) => results.push({ name, pass: false, detail: String(detail) });
async function test(name, run) {
  try {
    const detail = await run();
    pass(name, detail ?? "");
  } catch (error) {
    fail(name, error?.stack ?? error);
  }
}
function assert(condition, message, detail = undefined) {
  if (!condition) {
    throw new Error(detail === undefined ? message : `${message}: ${JSON.stringify(detail)}`);
  }
}

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 900, height: 600 } });
await page.setContent(FIXTURE);
const run = async (request, body) => JSON.parse(await page.evaluate(script(request, body)));

await test("zcSelect answers the first element a person could see, past a hidden twin", async () => {
  const seen = await run({ selector: ".go" }, SELECT_BODY);
  assert(seen.ok && seen.value.id === "go-seen" && !seen.value.hidden, "the visible twin was not chosen", seen);
  const field = await run({ selector: ".who" }, SELECT_BODY);
  assert(field.value.id === "who-seen", "the visible field was not chosen", field);
  return `chose ${seen.value.id}, ${field.value.id}`;
});

await test("when every match is hidden zcSelect answers the first one, marked hidden", async () => {
  const ghost = await run({ selector: ".ghost" }, SELECT_BODY);
  assert(ghost.ok && ghost.value.id === "ghost-a" && ghost.value.hidden && ghost.value.code === null, "the all-hidden case is not marked", ghost);
  const faint = await run({ selector: ".faint" }, SELECT_BODY);
  assert(faint.value.hidden, "an opacity-0 control counts as hidden", faint);
  return JSON.stringify(ghost.value);
});

await test("zcSelect still names a missing selector and an invalid one by code", async () => {
  const missing = await run({ selector: ".nothing" }, SELECT_BODY);
  const invalid = await run({ selector: "<<<" }, SELECT_BODY);
  assert(missing.value.code === "selector_not_found", "missing", missing);
  assert(invalid.value.code === "invalid_selector", "invalid", invalid);
  return `${missing.value.code}, ${invalid.value.code}`;
});

await test("wait is true for a control whose only visible copy stands behind a hidden twin", async () => {
  const go = await run({ selector: ".go" }, WAIT_BODY);
  const ghost = await run({ selector: ".ghost" }, WAIT_BODY);
  const missing = await run({ selector: ".nothing" }, WAIT_BODY);
  const invalid = await run({ selector: "<<<" }, WAIT_BODY);
  assert(go.ok && go.value === true, "the visible twin does not satisfy the wait", go);
  assert(ghost.ok && ghost.value === false, "an all-hidden match satisfies the wait", ghost);
  assert(missing.ok && missing.value === false, "a missing selector satisfies the wait", missing);
  assert(!invalid.ok && invalid.code === "invalid_selector", "an invalid selector is not refused by name", invalid);
  return "go:true ghost:false missing:false invalid:refused";
});

await test("click presses the visible twin and refuses when every match is hidden", async () => {
  const pressed = await run({ selector: ".go" }, CLICK_BODY);
  const log = await page.evaluate(() => document.getElementById("log").textContent);
  assert(pressed.ok, "the click was refused", pressed);
  assert(log === "go-seen;", "the visible twin was not the one pressed", log);
  const ghost = await run({ selector: ".ghost" }, CLICK_BODY);
  assert(!ghost.ok && ghost.code === "element_not_visible", "an all-hidden match is not refused as not visible", ghost);
  return `pressed ${log} refused ${ghost.code}`;
});

/* A Flow asks the same question after its page changes. Reusing detached
 * highlight nodes must not turn yesterday's answer into today's oracle. */
async function findFixture(runCase) {
  const findPage = await browser.newPage();
  try {
    await findPage.setContent('<p id="status">Ready</p><div id="added"></div>');
    await findPage.evaluate(FIND_INSTALL);
    await runCase(findPage);
  } finally { await findPage.close(); }
}

await test("find rechecks replaced page text with the same query in the same task", () => findFixture(async (findPage) => {
  const counts = await findPage.evaluate(() => {
    const find = window.__zerocodeFind;
    const before = find.run("Ready", true, false).count;
    document.getElementById("status").textContent = "Working";
    return [before, find.run("Ready", true, false).count];
  });
  assert(JSON.stringify(counts) === "[1,0]", "a removed match remained present", counts);
}));

await test("find discovers new matches after an empty same-query result", () => findFixture(async (findPage) => {
  const counts = await findPage.evaluate(() => {
    const find = window.__zerocodeFind;
    const before = find.run("Finished", true, false).count;
    document.getElementById("added").appendChild(document.createTextNode("Finished"));
    return [before, find.run("Finished", true, false).count];
  });
  assert(JSON.stringify(counts) === "[0,1]", "an added match stayed absent", counts);
}));

await test("find invalidates edited characters and mutations delivered between calls", () => findFixture(async (findPage) => {
  await findPage.evaluate(() => {
    window.__zerocodeFind.run("Ready", true, false);
    document.querySelector("mark").firstChild.nodeValue = "Working";
  });
  const result = await findPage.evaluate(() => window.__zerocodeFind.run("Ready", true, false));
  assert(result.count === 0 && result.index === 0, "edited characters stayed matched", result);
}));

await test("find still cycles both ways on unchanged text and clears only its own marks", () => findFixture(async (findPage) => {
  const state = await findPage.evaluate(() => {
    const find = window.__zerocodeFind;
    document.getElementById("status").textContent = "Ready Ready";
    const hits = [find.run("Ready", true, false), find.run("Ready", true, false), find.run("Ready", false, false)];
    find.clear();
    return { hits, text: document.getElementById("status").textContent, marks: document.querySelectorAll("[data-zc-find]").length };
  });
  assert(JSON.stringify(state.hits.map((hit) => hit.index)) === "[1,2,1]" && state.hits.every((hit) => hit.count === 2), "unchanged matches no longer cycle", state);
  assert(state.text === "Ready Ready" && state.marks === 0, "clear damaged the page", state);
}));

await browser.close();

let failed = 0;
for (const result of results) {
  if (!result.pass) failed += 1;
  const detail = result.detail ? `  — ${result.detail}` : "";
  console.log(`${result.pass ? "PASS" : "FAIL"}  ${result.name}${detail}`);
}
console.log(`\n${results.length - failed}/${results.length} passed`);
process.exit(failed ? 1 : 0);
