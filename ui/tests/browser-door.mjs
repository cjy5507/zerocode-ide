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

import { endRun } from "./end-run.mjs";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "./playwright-chromium.mjs";
import { rustList, rustNumber, rustText } from "./rust-source.mjs";
import { FORM_REQUEST, evalFormScript, fieldsScript, fillPasses, fillReadScript, fillWriteScript } from "./browser-scripts.mjs";
import * as twin from "./browser-scripts.mjs";

const UI = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const DOOR = await readFile(resolve(UI, "../crates/zerocode-shell/src/cmd/browser.rs"), "utf8");
const RUNTIME = await readFile(resolve(UI, "../crates/zerocode-shell/src/browser_runtime.rs"), "utf8");
const FIND_INSTALL = RUNTIME.match(/const BROWSER_FIND_INSTALL: &str = r##"([\s\S]*?)"##;/)[1];
const HELPERS = DOOR.match(/const BROWSER_AUTOMATION_HELPERS: &str = r#"([\s\S]*?)"#;/)[1];
const CLICK_BODY = DOOR.match(/const CLICK_BODY: &str = r#"([\s\S]*?)"#;/)[1];
// The wait's body is written inline in `automate_wait`; it is the raw string
// right after the door's own words about the picker.
const WAIT_BODY = DOOR.match(/could see\. A page whose only visible match stands behind a hidden\n\s*\/\/ twin used to time this wait out with the control on screen\.\n\s*r#"([\s\S]*?)"#,/)[1];
const TYPE_BODY = DOOR.match(/const TYPE_BODY: &str = r#"([\s\S]*?)"#;/)[1];
// The read seat's block cutter, and the landmark list it cuts at — read from
// the use table the way the Rust reads it, so the fixture is cut where the
// window cuts (`zerocode_core::jev::BROWSER_READ_BLOCK_ROOTS`).
const READ_BLOCKS_BODY = DOOR.match(/const BROWSER_READ_BLOCKS_BODY: &str = r#"([\s\S]*?)"#;/)[1];
const JEV = await readFile(resolve(UI, "../crates/zerocode-core/src/jev.rs"), "utf8");
const BLOCK_ROOTS = [...JEV.match(/pub const BROWSER_READ_BLOCK_ROOTS: \[&str; \d+\] = \[([\s\S]*?)\];/)[1]
  .matchAll(/"([^"]+)"/g)].map((hit) => hit[1]);
const READ_FIXTURES = resolve(UI, "tests", "fixtures", "browser-read");

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

/* The read seat's cutter, on the ten fixture pages: every block is a
 * landmark or the run under one, addressed by the same chain a press
 * answers; nothing rendered is lost and nothing unrendered is read. */
const readRequest = { blockRoots: BLOCK_ROOTS.join(","), textCap: 20000, titleCap: 500, urlCap: 2000, answerCap: 256000 };
const words = (text) => text.replace(/\s+/g, " ").trim();
const { readdir } = await import("node:fs/promises");
const fixtures = (await readdir(READ_FIXTURES)).filter((name) => name.endsWith(".html")).sort();
assert(fixtures.length === 10, "ten fixture pages", fixtures);
const cut = {};
for (const name of fixtures) {
  const fixturePage = await browser.newPage({ viewport: { width: 900, height: 600 } });
  await fixturePage.goto("file://" + resolve(READ_FIXTURES, name));
  const answer = JSON.parse(await fixturePage.evaluate(script(readRequest, READ_BLOCKS_BODY)));
  assert(answer.ok, `${name}: the cutter refused`, answer);
  cut[name] = answer.value;
  await fixturePage.close();
}

await test("the cutter keeps every rendered word of the page and reads no script", async () => {
  for (const [name, page] of Object.entries(cut)) {
    const joined = words(page.blocks.map((block) => block.text).join("\n"));
    const whole = words(page.text);
    for (const word of whole.split(" ")) {
      assert(joined.includes(word), `${name}: the blocks lost "${word}"`);
    }
    assert(!joined.includes("never read"), `${name}: a script's text was read`, joined);
    assert(joined.includes("Menu") === false || name !== "news.html", `${name}: a hidden nav was read`);
  }
  return `${Object.keys(cut).length} pages, ${Object.values(cut).reduce((n, page) => n + page.blocks.length, 0)} blocks`;
});

await test("every block is a landmark or the run under one, addressed from the body", async () => {
  for (const [name, page] of Object.entries(cut)) {
    for (const block of page.blocks) {
      assert(block.path.startsWith("body"), `${name}: a block not under the body`, block.path);
      assert(block.text.trim().length > 0, `${name}: an empty block`, block.path);
      const last = block.path.split(">").pop();
      const tag = last.replace(/[#.\[].*$/, "");
      const structural = BLOCK_ROOTS.some((root) => root === tag || (root.startsWith("[role=") && last.includes(root.slice(1, -1))));
      assert(last === "*" || last === "body" || structural, `${name}: a block that is not a landmark`, block.path);
    }
  }
  const news = cut["news.html"];
  const paths = news.blocks.map((block) => block.path);
  assert(paths.includes("body>header#masthead>*"), "the masthead's own words are a run", paths);
  assert(paths.includes("body>header#masthead>nav"), "the nav inside the masthead is its own block", paths);
  assert(paths.includes("body>div#cookie[role=dialog]"), "a div with a dialog role is a landmark", paths);
  assert(paths.includes("body>main>article"), "the article is one block", paths);
  assert(paths.includes("body>main>aside.related"), "the related rail is one block", paths);
  assert(paths.includes("body>main>section.ad"), "the ad unit is one block", paths);
  assert(paths.includes("body>footer"), "the footer is one block", paths);
  assert(!paths.some((path) => path.includes("nav.hidden")), "a hidden nav is not a block", paths);
  return paths.join(" | ");
});

await test("two siblings of one tag are told apart by their index", async () => {
  const forum = cut["forum.html"];
  const paths = forum.blocks.map((block) => block.path);
  assert(paths.includes("body>main>section.replies>article[1]") && paths.includes("body>main>section.replies>article[2]"), "the replies are not numbered", paths);
  assert(paths.includes("body>main>article.op"), "the opening post is one block", paths);
  const landing = cut["landing.html"];
  const sections = landing.blocks.map((block) => block.path).filter((path) => path.startsWith("body>main>section"));
  assert(sections.length === 3 && new Set(sections).size === 3, "three sections, three addresses", sections);
  return `${paths.length} forum blocks`;
});

await test("a press names the block it landed in by the same chain the cutter wrote", async () => {
  const fixturePage = await browser.newPage({ viewport: { width: 900, height: 600 } });
  await fixturePage.goto("file://" + resolve(READ_FIXTURES, "news.html"));
  // The links are real: a press must name its block, not leave the page.
  await fixturePage.evaluate(() => document.addEventListener("click", (event) => event.preventDefault(), true));
  const chainOf = async (selector) => {
    const answer = JSON.parse(await fixturePage.evaluate(script({ selector, blockRoots: readRequest.blockRoots }, CLICK_BODY)));
    assert(answer.ok, `click ${selector} refused`, answer);
    return answer.value;
  };
  const accept = await chainOf("#accept");
  assert(accept.blockPath === "body>div#cookie[role=dialog]", "the cookie button is in the dialog block", accept);
  assert(accept.pageUrl.startsWith("file://"), "the press answers the page's address", accept);
  const world = await chainOf('nav[aria-label="sections"] a');
  assert(world.blockPath === "body>header#masthead>nav", "a section link is in the masthead's nav", world);
  const related = await chainOf("aside.related a");
  assert(related.blockPath === "body>main>aside.related", "a related link is in the rail", related);
  const typed = JSON.parse(await fixturePage.evaluate(script({ selector: "#accept", text: "x", road: "keys", blockRoots: readRequest.blockRoots }, TYPE_BODY)));
  assert(!typed.ok && typed.code === "element_not_editable", "a button is not typed into", typed);
  await fixturePage.close();
  return `${accept.blockPath}; ${world.blockPath}; ${related.blockPath}`;
});

/* ---- One look, a press's settle, the look's pin (t-6721) ----
 *
 * The marks page script now reads, in the SAME synchronous pass as the
 * numbers, what the page holds beside them: its document, the numbered
 * fields, and the containers, images and rows a goal may be about — under
 * the keys the walk's question reads (`screen_action::snapshot`). A press by
 * number re-proves the document and the field's value at the moment it
 * presses, and then the page settles on its document's own stillness —
 * never on a painted frame, which a hidden tab never paints. Observation
 * moves no focus, no selection and no scroll. Every constant below is read
 * from the Rust the pane runs, never restated. */
const CORE = await readFile(resolve(UI, "../crates/zerocode-core/src/agent_browser.rs"), "utf8");
const SCREEN = await readFile(resolve(UI, "../crates/zerocode-core/src/screen_action.rs"), "utf8");
const VALUE_QUESTION = JSON.parse(await readFile(resolve(UI, "../crates/zerocode-core/fixtures/type-value/question.json"), "utf8"));
const snapshotKey = (name) => rustText(SCREEN, name);
const observeKey = (head) => SCREEN.match(new RegExp(`pub const fn key\\(self\\)[\\s\\S]*?Self::${head} => "(\\w+)"`))[1];
const MARK_HELPERS = rustText(DOOR, "BROWSER_MARK_HELPERS");
const MARKS_BODY = rustText(DOOR, "BROWSER_MARKS_BODY");
const REMEASURE_BODY = rustText(DOOR, "BROWSER_REMEASURE_BODY");
const OBSERVE_HELPERS = rustText(DOOR, "BROWSER_OBSERVE_HELPERS");
const SETTLE_BODY = rustText(DOOR, "BROWSER_SETTLE_BODY");
const SETTLE_QUIET_MS = rustNumber(CORE, "BROWSER_SETTLE_QUIET_MS");
const SETTLE_MS = rustNumber(CORE, "BROWSER_SETTLE_MS");
const CANDIDATE_CAP = rustNumber(JEV, "SCREEN_CANDIDATE_CAP");
const KEYS = {
  epoch: snapshotKey("EPOCH_KEY"), kind: snapshotKey("FIELD_KIND_KEY"), secret: snapshotKey("FIELD_SECRET_KEY"),
  label: snapshotKey("LABEL_KEY"), placeholder: snapshotKey("FIELD_PLACEHOLDER_KEY"), near: snapshotKey("FIELD_NEAR_KEY"),
  value: snapshotKey("FIELD_VALUE_KEY"), role: snapshotKey("ROLE_KEY"), count: snapshotKey("COUNT_KEY"),
  alt: snapshotKey("ALT_KEY"), width: snapshotKey("WIDTH_KEY"), height: snapshotKey("HEIGHT_KEY"),
  text: snapshotKey("TEXT_KEY"), selector: snapshotKey("SELECTOR_KEY"),
  containers: observeKey("Container"), images: observeKey("Image"), rows: observeKey("Row"),
};
/* The request `automate_marks` hands the page (cmd/browser.rs
 * `marks_request`), from the same tables. */
const MARKS_REQUEST = {
  selectors: rustList(CORE, "BROWSER_MARKABLE"),
  answerCap: rustNumber(DOOR, "BROWSER_CALLBACK_CAP"),
  keys: KEYS,
  observed: {
    containers: rustList(CORE, "BROWSER_OBSERVED_CONTAINERS"), rows: rustList(CORE, "BROWSER_OBSERVED_ROWS"),
    images: rustList(CORE, "BROWSER_OBSERVED_IMAGES"), chrome: rustList(CORE, "BROWSER_OBSERVED_CHROME"),
    cap: CANDIDATE_CAP, imageMinPx: rustNumber(CORE, "BROWSER_OBSERVED_IMAGE_MIN_PX"),
  },
  field: { regions: rustList(CORE, "BROWSER_FIELD_REGIONS"), headings: rustList(CORE, "BROWSER_FIELD_HEADINGS") },
  wordCap: Math.floor(rustNumber(SCREEN, "SHOWS_CHAR_CAP") / CANDIDATE_CAP),
  valueCap: VALUE_QUESTION.valueCharCap,
};
/* The settle's watch is kept under the guest's own slot name
 * (`settle_watch`, cmd/browser.rs), read from the one file both sides read. */
const WATCH = (await readFile(resolve(UI, "browser-guest-key.txt"), "utf8")).trim();
const SETTLE_REQUEST = { watch: WATCH, busy: (rustList(CORE, "BROWSER_SETTLE_BUSY") || []).join(",") };
/* The page says a field holds a secret under the door's own word — the
 * encoder hides every key that names a secret — and the door answers it
 * under the question's key (`look_of`, cmd/browser.rs). */
const need = (name, value) => assert(value !== null && value !== undefined && value !== "", `${name} is not in the Rust the pane runs`);
const lookScript = () => { need("BROWSER_MARKS_BODY", MARKS_BODY); need("BROWSER_OBSERVE_HELPERS", OBSERVE_HELPERS);
  return script(MARKS_REQUEST, `${OBSERVE_HELPERS}\n${MARK_HELPERS}\n${MARKS_BODY}`); };
const remeasureScript = (selector) => { need("BROWSER_OBSERVE_HELPERS", OBSERVE_HELPERS);
  return script({ selector }, `${OBSERVE_HELPERS}\n${MARK_HELPERS}\n${REMEASURE_BODY}`); };
const settleScript = () => { need("BROWSER_SETTLE_BODY", SETTLE_BODY); need("BROWSER_OBSERVE_HELPERS", OBSERVE_HELPERS);
  return script(SETTLE_REQUEST, `${OBSERVE_HELPERS}\n${SETTLE_BODY}`); };
const pressScript = (selector, expect) => script({ selector, blockRoots: BLOCK_ROOTS.join(","), expect },
  `${OBSERVE_HELPERS || ""}\n${CLICK_BODY}`);
const evalJson = async (target, source) => JSON.parse(await target.evaluate(source));
/* A tab nobody is looking at, as far as the page can tell: hidden, and no
 * animation frame ever comes — every call is counted. Written into the page
 * itself, first thing, so it holds for the document the content makes. */
const HIDDEN = `<script>
  Object.defineProperty(document, "hidden", { get: () => true });
  Object.defineProperty(document, "visibilityState", { get: () => "hidden" });
  window.__frames = 0;
  window.requestAnimationFrame = () => { window.__frames += 1; return 0; };
</script>`;
const SNAPSHOT_FIXTURE = `<!doctype html><title>snapshot fixture</title>
<style>body{font:13px system-ui;margin:8px} img{display:inline-block;width:120px;height:40px} img.icon{width:16px;height:16px} input,select,textarea,button{font:inherit;margin:2px}</style>
<header><nav aria-label="site"><ul><li><a href="#home" id="home">Home</a></li><li><a href="#deals">Deals</a></li></ul></nav><img id="logo" alt="Shop logo"></header>
<main><h1>Travel search</h1>
<form onsubmit="return false">
<label>Origin <input id="origin" placeholder="From" value="Zurich"></label>
<label>Destination <input id="destination" placeholder="City" value="Lon"></label>
<label for="when">When</label><input id="when" type="date">
<input id="pw" type="password" aria-label="Password" value="hunter2">
<label>Size <select id="size"><option>S</option><option selected>M</option></select></label>
<span id="note-label">Note for the host</span><textarea id="note" aria-labelledby="note-label"></textarea>
<label><input id="agree" type="checkbox" checked> I agree</label>
<button type="button" id="search">Search</button><input type="submit" id="go" value="Go">
</form>
<section><h2>Results</h2><ul id="results" role="list" aria-label="Search results">
<li role="listitem"><img alt="iPhone 16 Pro, black"><span>iPhone 16 Pro 256GB — in stock</span> <button id="add">Add</button></li>
<li role="listitem"><img alt="iPhone 16 case"><span>iPhone 16 silicone case</span></li>
<li role="listitem"><img class="icon" alt=""><span>USB-C to USB-C cable 1 m</span></li>
</ul></section>
<div style="height:2000px"></div><img id="below" alt="Far below"></main>`;

/* A password field is judged once, for typing and for looking alike: the
 * platform's own facts — `type=password`, or the `current-password` a form
 * declares — and nothing else, in the one helper both scripts read. */
await test("a_secret_field_is_judged_once_for_typing_and_looking", async () => {
  assert(/const zcSecretField = /.test(HELPERS), "the rule is not one helper in the door's helpers");
  for (const word of ["\"password\"", "\"current-password\""]) assert(HELPERS.includes(word), `the helper lost ${word}`);
  assert(TYPE_BODY.includes("zcSecretField(element)"), "the typing does not read the one rule");
  const judged = await browser.newPage();
  try {
    await judged.setContent(`<form>
      <input id="password" type="password"><input id="current" autocomplete="current-password">
      <input id="Upper" type="PASSWORD"><input id="text"><input id="email" type="email" autocomplete="username">
      <input id="fresh" type="text" autocomplete="new-password"><textarea id="area"></textarea>
      <div id="editable" contenteditable="true">x</div></form>`);
    const expected = { password: true, current: true, Upper: true, text: false, email: false, fresh: false, area: false, editable: false };
    const typed = {};
    for (const id of Object.keys(expected)) {
      const answer = await evalJson(judged, script({ selector: "#" + id, text: "x", road: "keys", blockRoots: BLOCK_ROOTS.join(",") }, TYPE_BODY));
      typed[id] = answer.ok ? answer.value.secureField === true && answer.value.method === "held" : null;
    }
    assert(JSON.stringify(typed) === JSON.stringify(expected), "the typing's judgment changed", typed);
    const look = await evalJson(judged, lookScript());
    const looked = {};
    for (const face of look.value.faces) if (face.field) looked[face.selector.slice(1)] = face.field.masked;
    assert(JSON.stringify(looked) === JSON.stringify(expected), "the look judges another way than the typing", looked);
    return JSON.stringify(typed);
  } finally { await judged.close(); }
});

await test("one_snapshot_contains_controls_values_and_context_from_one_epoch", async () => {
  const look = await browser.newPage({ viewport: { width: 900, height: 700 } });
  try {
    await look.setContent(SNAPSHOT_FIXTURE);
    const quiet = await look.evaluate(() => {
      window.__records = 0;
      new MutationObserver((records) => { window.__records += records.length; })
        .observe(document, { subtree: true, childList: true, characterData: true, attributes: true });
      return String(performance.timeOrigin);
    });
    const answer = await evalJson(look, lookScript());
    assert(answer.ok, "the look was refused", answer);
    const { faces, snapshot } = answer.value;
    assert(snapshot && snapshot[KEYS.epoch] === quiet, "the look names its document by its epoch", snapshot);
    const byId = (id) => faces.find((face) => face.selector === "#" + id);
    const field = (id) => byId(id)?.field;
    assert(field("destination")?.[KEYS.kind] === "text" && field("destination")[KEYS.value] === "Lon"
      && field("destination")[KEYS.label] === "Destination" && field("destination")[KEYS.placeholder] === "City"
      && field("destination")[KEYS.near] === "Travel search" && field("destination").masked === false,
      "a text field's words and value", field("destination"));
    assert(field("when")?.[KEYS.kind] === "date" && field("when")[KEYS.label] === "When", "a label by `for`", field("when"));
    assert(field("pw")?.masked === true && field("pw")[KEYS.value] === "" && byId("pw").valueDigest === "secret",
      "a password's value and fingerprint never leave the page", byId("pw"));
    assert(field("size")?.[KEYS.kind] === "select" && field("size")[KEYS.label] === "Size" && field("size")[KEYS.value] === "M",
      "a select's label leaves its options out", field("size"));
    assert(field("note")?.[KEYS.label] === "Note for the host", "a label by aria-labelledby", field("note"));
    assert(field("agree")?.[KEYS.kind] === "checkbox" && field("agree")[KEYS.label] === "I agree", "a checkbox", field("agree"));
    assert(!field("search") && !field("go") && !field("home"), "a button, a submit and a link are no fields", [byId("search"), byId("go")]);
    const containers = snapshot[KEYS.containers], images = snapshot[KEYS.images], rows = snapshot[KEYS.rows];
    assert(containers.length === 1 && containers[0][KEYS.selector] === "#results" && containers[0][KEYS.role] === "list"
      && containers[0][KEYS.label] === "Search results" && containers[0][KEYS.count] === 3,
      "the results list, and not the site's navigation", containers);
    assert(images.map((one) => one[KEYS.alt]).join("|") === "iPhone 16 Pro, black|iPhone 16 case"
      && images.every((one) => one[KEYS.width] >= 32 && one[KEYS.selector]),
      "the item pictures on screen — no logo in the banner, no icon, nothing below the fold", images);
    assert(rows.length === 3 && rows[0][KEYS.text].startsWith("iPhone 16 Pro 256GB") && rows.every((row) => row[KEYS.selector]),
      "the result rows, not the navigation's", rows);
    assert(rows.length <= CANDIDATE_CAP && images.length <= CANDIDATE_CAP && containers.length <= CANDIDATE_CAP, "the table's cap");
    assert(await look.evaluate(() => window.__records) === 0, "the look wrote nothing to the page");

    // Page code the look itself runs (a value getter) changes the document
    // under it: an earlier control's words and an earlier field's value. The
    // answer is one state — the one after — never the half before.
    await look.evaluate(() => {
      const destination = document.getElementById("destination");
      const own = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value");
      let fired = false;
      Object.defineProperty(destination, "value", { configurable: true, set(v) { own.set.call(this, v); },
        get() {
          if (!fired) {
            fired = true;
            document.getElementById("home").textContent = "Start";
            own.set.call(document.getElementById("origin"), "Geneva");
            const row = document.createElement("li");
            row.setAttribute("role", "listitem");
            row.textContent = "New arrival";
            document.getElementById("results").append(row);
          }
          return own.get.call(this);
        } });
    });
    const moved = await evalJson(look, lookScript());
    assert(moved.ok, "a document that moved once is read again, not refused", moved);
    const again = (id) => moved.value.faces.find((face) => face.selector === "#" + id);
    assert(again("home").label === "Start", "the control read before the change is read after it", again("home"));
    assert(again("origin").field[KEYS.value] === "Geneva", "the field read before the change is read after it", again("origin"));
    assert(moved.value.snapshot[KEYS.rows].some((row) => row[KEYS.text] === "New arrival"), "the row the change added is there");

    // A value changed by the read's own page code, with no change to the
    // document a watch could see: the second read of every field's value
    // catches it, and the answer is the state after.
    await look.evaluate(() => {
      const destination = document.getElementById("destination");
      const own = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value");
      let fired = false;
      Object.defineProperty(destination, "value", { configurable: true, set(v) { own.set.call(this, v); },
        get() {
          if (!fired) { fired = true; own.set.call(document.getElementById("origin"), "Bern"); }
          return own.get.call(this);
        } });
    });
    const quietly = await evalJson(look, lookScript());
    const origin = quietly.value.faces.find((face) => face.selector === "#origin");
    assert(quietly.ok && origin.field[KEYS.value] === "Bern", "a value changed under the read is read after it", origin);

    // A document that moves on every read cannot be read in one state: the
    // look says so and hands over nothing of it.
    await look.evaluate(() => {
      const destination = document.getElementById("destination");
      const own = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value");
      Object.defineProperty(destination, "value", { configurable: true, set(v) { own.set.call(this, v); },
        get() { document.getElementById("home").textContent += "!"; return own.get.call(this); } });
    });
    const moving = await evalJson(look, lookScript());
    assert(!moving.ok && moving.code === "document_moving" && moving.value === undefined, "a moving document is refused whole", moving);
    return `${faces.length} faces, ${images.length} images, ${rows.length} rows`;
  } finally { await look.close(); }
});

await test("hidden_surface_short_settle_has_a_wall_deadline", async () => {
  const hidden = await browser.newPage();
  try {
    await hidden.setContent(`${HIDDEN}<button id="go">Go</button><p id="status">Ready</p><script>
      document.getElementById("go").addEventListener("click", () => { document.getElementById("status").textContent = "Went"; });
    </script>`);
    const epoch = await hidden.evaluate(() => String(performance.timeOrigin));
    const pressed = await evalJson(hidden, pressScript("#go", { epoch, value: null, watch: WATCH }));
    assert(pressed.ok && typeof pressed.value.pressedAt === "number", "the press answers the page's clock", pressed);
    await new Promise((done) => setTimeout(done, SETTLE_QUIET_MS + 20));
    const first = await evalJson(hidden, settleScript());
    assert(first.ok && first.value.hidden === true && first.value.watched === true, "a hidden page answers its facts", first);
    assert(first.value.documentEpoch === epoch && first.value.last >= pressed.value.pressedAt,
      "the watch the press set saw the press's own change", { pressed: pressed.value, first: first.value });
    // The watch stood before the press's first event, so the very first
    // poll, a quiet window later, already finds the document still — a watch
    // set by that poll would start its count there.
    assert(first.value.now - Math.max(first.value.last, pressed.value.pressedAt) >= SETTLE_QUIET_MS,
      "the first poll knew how long the document had stood still", first.value);
    const later = await evalJson(hidden, settleScript());
    assert(later.value.now - Math.max(later.value.last, pressed.value.pressedAt) >= SETTLE_QUIET_MS,
      "a hidden document that stood still says so, well inside the wall", later.value);
    assert(await hidden.evaluate(() => window.__frames) === 0, "no animation frame was asked for");

    // A hidden page that never stands still: over the whole wall, never a
    // quiet window — the page says what it is doing and keeps saying it.
    await hidden.evaluate(() => { let n = 0; window.__ticker = setInterval(() => { document.getElementById("status").textContent = "tick " + (n += 1); }, 10); });
    const began = Date.now();
    const stills = [];
    while (Date.now() - began < SETTLE_MS) {
      const facts = (await evalJson(hidden, settleScript())).value;
      stills.push(facts.now - facts.last);
    }
    const moving = stills.filter((still) => still < SETTLE_QUIET_MS).length;
    // A loaded machine may stall the page's timers once; the document
    // still reads as changing at nearly every poll.
    assert(moving >= stills.length * 0.8, "a document changing every 10 ms read as still", stills);
    const quietest = Math.max(...stills);
    await hidden.evaluate(() => clearInterval(window.__ticker));

    // Another document in the pane is another epoch.
    await hidden.goto("data:text/html,<p>elsewhere</p>");
    const replaced = await evalJson(hidden, settleScript());
    assert(replaced.value.documentEpoch !== epoch, "a replaced document names another epoch", replaced.value);
    return `still after ${Math.round(later.value.now - pressed.value.pressedAt)} ms; moving ${moving}/${stills.length} polls (stillest ${Math.round(quietest)} ms)`;
  } finally { await hidden.close(); }
});

/* A press that leaves its settle for later (t-9712) is answered with a look
 * taken the moment it was made, and the settle is finished by the pane's
 * next look: that first look must read what the press left and must not
 * itself count as a change — it only reads — so the settle it runs ahead of
 * ends exactly when it would have without it. */
await test("a_look_between_a_press_and_its_settle_reads_what_the_press_left_and_moves_no_settle", async () => {
  const hidden = await browser.newPage();
  try {
    await hidden.setContent(`${HIDDEN}<main><button id="next">Next: 2</button><p id="step">Step 1 of 4</p></main><script>
      let at = 1;
      document.getElementById("next").addEventListener("click", () => {
        at += 1;
        document.getElementById("step").textContent = "Step " + at + " of 4";
        document.getElementById("next").textContent = "Next: " + (at + 1);
      });
    </script>`);
    const epoch = await hidden.evaluate(() => String(performance.timeOrigin));
    const pressed = await evalJson(hidden, pressScript("#next", { epoch, value: null, watch: WATCH }));
    assert(pressed.ok, "the press was made", pressed);
    const first = await evalJson(hidden, lookScript());
    const lookedAt = await hidden.evaluate(() => performance.now());
    assert(first.ok, "the look right after the press read the page", first);
    const labels = first.value.faces.map((face) => face.label);
    assert(labels.includes("Next: 3"), "the first look reads what the press left", labels);
    await new Promise((done) => setTimeout(done, SETTLE_QUIET_MS + 20));
    const facts = (await evalJson(hidden, settleScript())).value;
    assert(facts.watched && facts.documentEpoch === epoch, "the press's own watch answers", facts);
    assert(facts.last <= lookedAt, "the look counted as no change", { facts, lookedAt });
    const still = facts.now - Math.max(facts.last, pressed.value.pressedAt);
    assert(still >= SETTLE_QUIET_MS, "the settle stood still since the press, the look inside it", { still, facts });
    return `first look ${Math.round(lookedAt - pressed.value.pressedAt)} ms after the press; still ${Math.round(still)} ms at the poll`;
  } finally { await hidden.close(); }
});

await test("visibility_is_not_confused_with_task_completion", async () => {
  const seen = await browser.newPage();
  try {
    // A painted page, frame after frame, whose document keeps changing on
    // every frame: visible and drawing, and not still.
    await seen.setContent(`<p id="clock">0</p><script>
      let n = 0; const tick = () => { document.getElementById("clock").textContent = String(n += 1); requestAnimationFrame(tick); };
      requestAnimationFrame(tick);
    </script>`);
    const facts = [];
    const began = Date.now();
    while (Date.now() - began < SETTLE_MS) facts.push((await evalJson(seen, settleScript())).value);
    assert(facts.every((one) => one.hidden === false), "the page was visible throughout");
    const moving = facts.filter((one) => one.now - one.last < SETTLE_QUIET_MS).length;
    assert(moving >= facts.length * 0.8, "a painted page still changing read as still", facts.slice(-3));
    assert(await seen.evaluate(() => Number(document.getElementById("clock").textContent)) > 3, "frames were painted all along");
    return `${moving}/${facts.length} polls still changing`;
  } finally { await seen.close(); }
});

await test("value_change_or_occlusion_invalidates_selected_target", async () => {
  const pinned = await browser.newPage();
  try {
    await pinned.setContent(`<style>#cover{position:fixed;left:0;top:0;width:100%;height:100%;background:#fff}</style>
      <input id="city" aria-label="City" value="Lon"><input id="pw" type="password" aria-label="Password" value="a">
      <label><input id="agree" type="checkbox"> ok</label><select id="size"><option>S</option><option>M</option></select>
      <button id="go">Go</button><p id="log"></p><script>
      window.__presses = 0;
      document.getElementById("go").addEventListener("click", () => { window.__presses += 1; });
      document.getElementById("city").addEventListener("click", () => { window.__presses += 1; });
    </script>`);
    const measure = async (selector) => (await evalJson(pinned, remeasureScript(selector))).value;
    const city = await measure("#city");
    assert(city.found && city.documentEpoch && city.valueDigest, "the re-measure names the document and the value", city);
    await pinned.fill("#city", "London");
    assert((await measure("#city")).valueDigest !== city.valueDigest, "a changed value is another digest");
    assert((await measure("#city")).label === "City", "while the words the old pin reads stayed");
    const agree = await measure("#agree");
    await pinned.check("#agree");
    assert((await measure("#agree")).valueDigest !== agree.valueDigest, "a checked box is another digest");
    const size = await measure("#size");
    await pinned.selectOption("#size", "M");
    assert((await measure("#size")).valueDigest !== size.valueDigest, "another option is another digest");
    const pw = await measure("#pw");
    await pinned.fill("#pw", "hunter2");
    assert(pw.valueDigest === "secret" && (await measure("#pw")).valueDigest === "secret", "a secret has no fingerprint");
    assert((await measure("#go")).valueDigest === null, "a button pins no value");

    const now = await measure("#city");
    const press = async (expect, selector = "#city") => evalJson(pinned, pressScript(selector, expect));
    const stale = await press({ epoch: "1.5", value: now.valueDigest, watch: WATCH });
    assert(!stale.ok && stale.code === "document_replaced", "another document refuses the press", stale);
    const typed = await press({ epoch: now.documentEpoch, value: city.valueDigest, watch: WATCH });
    assert(!typed.ok && typed.code === "value_changed", "a changed value refuses the press", typed);
    assert(await pinned.evaluate(() => window.__presses) === 0, "neither refusal pressed anything");
    await pinned.evaluate(() => { const cover = document.createElement("div"); cover.id = "cover"; document.body.append(cover); });
    const covered = await press({ epoch: now.documentEpoch, value: null, watch: WATCH }, "#go");
    assert(!covered.ok && covered.code === "element_obscured", "a covered control is not pressed", covered);
    await pinned.evaluate(() => document.getElementById("cover").remove());
    const held = await press({ epoch: now.documentEpoch, value: now.valueDigest, watch: WATCH });
    assert(held.ok && await pinned.evaluate(() => window.__presses) === 1, "the look that still holds presses once", held);
    return "document, value, checked, option, cover: refused; holding: pressed";
  } finally { await pinned.close(); }
});

await test("background_observation_does_not_focus_another_pane", async () => {
  const context = await browser.newContext({ viewport: { width: 900, height: 600 } });
  try {
    const person = await context.newPage();
    await person.setContent(`<input id="typing" value="half a sentence"><div style="height:3000px"></div>`);
    const background = await context.newPage();
    await background.setContent(HIDDEN + SNAPSHOT_FIXTURE);
    await person.bringToFront();
    await person.focus("#typing");
    await person.evaluate(() => { document.getElementById("typing").setSelectionRange(2, 6); window.scrollTo(0, 120); });
    const where = (target) => target.evaluate(() => ({ active: document.activeElement?.id || document.activeElement?.tagName,
      start: document.activeElement?.selectionStart ?? null, end: document.activeElement?.selectionEnd ?? null,
      y: Math.round(window.scrollY), selection: String(getSelection()) }));
    const personBefore = await where(person), backgroundBefore = await where(background);
    for (const target of [background, person]) {
      assert((await evalJson(target, lookScript())).ok, "the look ran");
      assert((await evalJson(target, remeasureScript("#typing, #destination"))).ok, "the re-measure ran");
      assert((await evalJson(target, settleScript())).ok, "the settle ran");
    }
    const personAfter = await where(person), backgroundAfter = await where(background);
    assert(JSON.stringify(personAfter) === JSON.stringify(personBefore), "the person's focus, selection and scroll stayed", { personBefore, personAfter });
    assert(JSON.stringify(backgroundAfter) === JSON.stringify(backgroundBefore), "the observed tab's did too", { backgroundBefore, backgroundAfter });
    return JSON.stringify(personAfter);
  } finally { await context.close(); }
});

/* ---- The form pair (t-37883): `fields` reads every field a page draws,
 * `fill` writes a bundle and reads each field back. The scripts and their
 * tables are read from the Rust the pane runs; the pages are the bench's
 * regression scenes (tools/computer-bench/form-scenes), each a shape real
 * sites build — the door is told nothing about them. */
const SCENES = resolve(UI, "../tools/computer-bench/form-scenes");
const scene = async (name) => readFile(resolve(SCENES, name, "scene.html"), "utf8");
const readFields = async (target) => {
  const read = await evalJson(target, fieldsScript());
  assert(read.ok, "the read was refused", read);
  return read.value;
};
const fillBundle = (target, bundle, expect = null) => fillPasses((source) => evalJson(target, source), bundle, expect);
const byHandle = (read) => Object.fromEntries(read.fields.map((field) => [field.handle, field]));
const handleOf = (read, label) => {
  const found = read.fields.filter((field) => field.label === label);
  assert(found.length === 1, `one field reads "${label}"`, read.fields.map((field) => field.label));
  return found[0].handle;
};

await test("a_form_read_names_every_field_by_the_pages_own_words", async () => {
  const booking = await browser.newPage({ viewport: { width: 900, height: 600 } });
  try {
    await booking.setContent(await scene("A"));
    const read = await readFields(booking);
    const labels = read.fields.map((field) => field.label);
    const expected = ["입차일", "입차 시간", "출차일", "출차 시간", "이름", "휴대폰 (1/3)", "휴대폰 (2/3)", "휴대폰 (3/3)",
      "이메일", "차량번호", "항공편", "인원", "주차 구역", "개인정보 수집·이용 동의 (필수)"];
    assert(JSON.stringify(labels) === JSON.stringify(expected), "the page's own words, in its order, below the fold too", labels);
    const fields = Object.fromEntries(read.fields.map((field) => [field.label, field]));
    assert(fields["입차 시간"].disabled && fields["입차 시간"].kind === "select", "a list not loaded yet is off", fields["입차 시간"]);
    assert(fields["출차 시간"].options.length === 38 && fields["출차 시간"].options[0] === "05:00" && fields["출차 시간"].moreOptions === 0,
      "a select's choices by their words", fields["출차 시간"]);
    assert(fields["휴대폰 (1/3)"].maxLength === 3 && fields["휴대폰 (2/3)"].maxLength === 4, "the parts say their length");
    assert(fields["주차 구역"].kind === "radio" && JSON.stringify(fields["주차 구역"].options) === JSON.stringify(["실내", "실외"])
      && fields["주차 구역"].required, "a radio group is one field", fields["주차 구역"]);
    assert(fields["개인정보 수집·이용 동의 (필수)"].value === false, "a checkbox hidden behind its label is read", fields);
    assert(fields["이름"].section === "예약자 정보" && fields["입차일"].section === "이용 일정", "the section is the legend");
    assert(read.fields.every((field) => field.required === !["항공편", "인원"].includes(field.label)), "required as the page marks it");
    const actions = read.actions.map((action) => action.label);
    assert(actions.includes("인증요청") && actions.includes("예약하기"), "the buttons beside the fields", read.actions);
    for (const field of read.fields) {
      const found = await booking.evaluate((handle) => document.querySelectorAll(handle).length, field.handle);
      assert(found >= 1, `the handle ${field.handle} finds its field`);
    }
    assert(!labels.includes("인증번호"), "a field the page has not drawn yet is not read");
    return `${read.fields.length} fields, ${read.actions.length} buttons`;
  } finally { await booking.close(); }
});

await test("a_fill_writes_a_bundle_in_one_call_and_reads_each_back", async () => {
  const booking = await browser.newPage({ viewport: { width: 900, height: 600 } });
  try {
    await booking.setContent(await scene("A"));
    const read = await readFields(booking);
    const at = (label) => handleOf(read, label);
    const bundle = {
      [at("입차일")]: "2026.11.3", [at("입차 시간")]: "07:30", [at("출차일")]: "2026-11-07", [at("출차 시간")]: "21:00",
      [at("이름")]: "김예시", [at("휴대폰 (1/3)")]: "010", [at("휴대폰 (2/3)")]: "5550", [at("휴대폰 (3/3)")]: "0142",
      [at("이메일")]: "kim@example.com", [at("차량번호")]: "12가3456", [at("항공편")]: "SH204", [at("인원")]: 2,
      [at("주차 구역")]: "실내", [at("개인정보 수집·이용 동의 (필수)")]: true,
    };
    const filled = await fillBundle(booking, bundle);
    const statuses = filled.results.map((result) => result.status);
    assert(statuses.every((status) => status === "set"), "every field took", filled.results);
    assert(filled.passes >= 2, "the time list a date loads took a later pass", filled.passes);
    assert(filled.results[0].now === "2026-11-03", "a date as written becomes the input's own form", filled.results[0]);
    assert(filled.left.length === 0, "nothing required is left", filled.left);
    const kept = await booking.evaluate(() => {
      document.querySelector('input[name="flight"]').dispatchEvent(new Event("input", { bubbles: true }));
      const form = document.getElementById("booking");
      return { name: form.elements.name.value, email: form.elements.email.value, time: form.elements["in-time"].value,
        lot: form.querySelector('input[name="lot"]:checked')?.value, agree: document.getElementById("agree").checked };
    });
    assert(JSON.stringify(kept) === JSON.stringify({ name: "김예시", email: "kim@example.com", time: "07:30", lot: "indoor", agree: true }),
      "the framework's own state holds what was written", kept);
    const again = await fillBundle(booking, { [at("이름")]: "김예시", [at("개인정보 수집·이용 동의 (필수)")]: true });
    assert(again.results.every((result) => result.status === "same"), "a value already held is not written again", again.results);
    return `${filled.passes} passes`;
  } finally { await booking.close(); }
});

await test("a_fill_refuses_by_name_what_it_must_not_or_cannot_write", async () => {
  const odd = await browser.newPage();
  try {
    await odd.setContent(`<form><label>비밀번호 <input id="pw" type="password"></label>
      <label>사진 <input id="photo" type="file"></label><label>코드 <input id="short" maxlength="4"></label>
      <label>도시 <select id="city"><option value="">선택</option><option>부산</option><option>대구</option></select></label></form>`);
    const filled = await fillBundle(odd, { "#pw": "hunter2", "#photo": "/x.png", "#short": "12345", "#city": "광주",
      "#nowhere": "x", "<<<": "x" });
    const said = Object.fromEntries(filled.results.map((result) => [result.handle, result.status]));
    assert(JSON.stringify(said) === JSON.stringify({ "#pw": "secret", "#photo": "file", "#short": "too_long", "#city": "no_option",
      "#nowhere": "not_found", "<<<": "invalid_handle" }), "each refusal by its name", said);
    const city = filled.results.find((result) => result.handle === "#city");
    assert(JSON.stringify(city.options) === JSON.stringify(["부산", "대구"]), "a missing choice says the choices there are", city);
    const untouched = await odd.evaluate(() => [document.getElementById("pw").value, document.getElementById("short").value]);
    assert(JSON.stringify(untouched) === JSON.stringify(["", ""]), "nothing refused was written", untouched);
    const read = await readFields(odd);
    assert(read.fields.find((field) => field.handle === "#pw").masked, "a secret is read as masked");
    return JSON.stringify(said);
  } finally { await odd.close(); }
});

await test("drawn_widgets_steps_and_a_frame_are_read_and_filled_by_their_words", async () => {
  const rental = await browser.newPage({ viewport: { width: 900, height: 700 } });
  try {
    await rental.setContent(await scene("B"));
    const step = async (bundle, next) => {
      const read = await readFields(rental);
      const filled = await fillBundle(rental, Object.fromEntries(Object.entries(bundle).map(([label, value]) => [handleOf(read, label), value])));
      assert(filled.results.every((result) => result.status === "set"), "every field of the step took", filled.results);
      if (next) {
        const button = read.actions.find((action) => action.label === next);
        assert(button, `the step's button ${next} is read`, read.actions);
        await rental.evaluate((handle) => document.querySelector(handle).click(), button.handle);
      }
      return { read, filled };
    };
    const one = await step({ "차종": "준중형 · 아반떼", "운전자 생년월일": "19880214", "연료": "휘발유" }, "다음");
    const model = one.read.fields.find((field) => field.label === "차종");
    assert(model.kind === "combobox", "a button that opens a list is a dropdown", model);
    const two = await step({ "반납일": "2026-11-07", "반납 장소": "기타", "보험 추가": true });
    assert(two.read.fields.find((field) => field.label === "반납일").readOnly, "a date the page keeps from typing is read-only");
    assert(two.filled.results[0].now === "2026.11.07", "the date was picked on the page's own calendar", two.filled.results[0]);
    await rental.waitForTimeout(450);
    await step({ "상세 장소": "북문 주차장" }, "다음");
    const three = await readFields(rental);
    const labels = three.fields.map((field) => field.label);
    for (const label of ["카드 소유자", "카드번호 (1/4)", "카드번호 (4/4)", "유효기간 (1/2)", "유효기간 (2/2)", "이용 약관에 동의합니다"]) {
      assert(labels.includes(label), `step 3 reads ${label}`, labels);
    }
    const framed = three.fields.find((field) => field.label === "카드 소유자");
    assert(framed.handle.includes(FORM_REQUEST.frameSeparator), "a field in a frame is named through its frame", framed);
    const at = (label) => handleOf(three, label);
    const paid = await fillBundle(rental, { [at("카드 소유자")]: "KIM YESI", [at("카드번호 (1/4)")]: "4000", [at("카드번호 (2/4)")]: "0012",
      [at("카드번호 (3/4)")]: "3456", [at("카드번호 (4/4)")]: "7899", [at("유효기간 (1/2)")]: "08", [at("유효기간 (2/2)")]: "28",
      [at("이용 약관에 동의합니다")]: true });
    assert(paid.results.every((result) => result.status === "set"), "the frame's fields and the page's took", paid.results);
    await rental.evaluate((handle) => document.querySelector(handle).click(), three.actions.find((action) => action.label === "결제하기").handle);
    const result = await rental.evaluate(() => window.__sceneResult);
    const expected = JSON.parse(await readFile(resolve(SCENES, "B", "expected.json"), "utf8"));
    assert(JSON.stringify(result) === JSON.stringify(expected), "the page took the booking", result);
    return "3 steps";
  } finally { await rental.close(); }
});

await test("a_tab_nobody_looks_at_reads_and_fills_and_the_persons_tab_keeps_its_focus", async () => {
  const context = await browser.newContext({ viewport: { width: 900, height: 600 } });
  try {
    const person = await context.newPage();
    await person.setContent(`<input id="typing" value="half a sentence"><div style="height:3000px"></div>`);
    const background = await context.newPage();
    await background.setContent(HIDDEN + await scene("A"));
    await person.bringToFront();
    await person.focus("#typing");
    await person.evaluate(() => { document.getElementById("typing").setSelectionRange(2, 6); window.scrollTo(0, 120); });
    const where = () => person.evaluate(() => ({ active: document.activeElement?.id, start: document.activeElement?.selectionStart,
      end: document.activeElement?.selectionEnd, y: Math.round(window.scrollY) }));
    const before = await where();
    const read = await readFields(background);
    const filled = await fillBundle(background, { [handleOf(read, "이름")]: "김예시", [handleOf(read, "주차 구역")]: "실외",
      [handleOf(read, "인원")]: "3" });
    assert(filled.results.every((result) => result.status === "set"), "the hidden tab filled", filled.results);
    assert(await background.evaluate(() => window.__frames) === 0, "no frame was waited for");
    const after = await where();
    assert(JSON.stringify(after) === JSON.stringify(before), "the person's focus, selection and scroll stayed", { before, after });
    return JSON.stringify(after);
  } finally { await context.close(); }
});

/* One eval, one step (`eval_script`): the expression reads the page's
 * fields, fills them by the words it read and says what is left — the
 * script the skill teaches, run as the door runs it. */
await test("one_eval_reads_fills_and_checks_a_step_by_the_words_it_read", async () => {
  const booking = await browser.newPage({ viewport: { width: 900, height: 600 } });
  try {
    await booking.setContent(await scene("A"));
    const card = JSON.parse(await readFile(resolve(SCENES, "A", "card.json"), "utf8"));
    const step = `(() => {
      const card = ${JSON.stringify(Object.fromEntries(card.facts.map((fact) => [fact.says, fact.value])))};
      const read = zerocode.fields();
      const bundle = {};
      for (const field of read.fields) {
        const words = field.label.replace(/ \\(\\d+\\/\\d+\\)$/, "");
        if (!(words in card)) continue;
        const part = field.label.match(/\\((\\d+)\\/(\\d+)\\)$/);
        bundle[field.handle] = part ? String(card[words]).split(/\\D+/)[Number(part[1]) - 1] : card[words];
      }
      const filled = zerocode.fill(bundle);
      return { statuses: filled.results.map((r) => r.label + ":" + r.status), left: filled.left.map((f) => f.label) };
    })()`;
    const first = await evalJson(booking, evalFormScript(step));
    assert(first.ok, "the one script ran", first);
    const notSet = first.value.statuses.filter((said) => !said.endsWith(":set"));
    assert(JSON.stringify(notSet) === JSON.stringify(["입차 시간:disabled"]), "all but the list a date loads took in one call", first.value);
    await booking.waitForTimeout(350);
    const second = await evalJson(booking, evalFormScript(step));
    assert(second.value.statuses.every((said) => said.endsWith(":set") || said.endsWith(":same")), "the next call finishes it", second.value);
    return `${first.value.statuses.length} fields in one call`;
  } finally { await booking.close(); }
});

/* A fill is held to the form its agent read (m-40824): a field renamed
 * after the read, and the whole bundle is refused unwritten — on the door's
 * road and inside one eval alike; the form as read takes it. */
await test("a_fill_on_a_form_that_changed_since_its_read_writes_nothing", async () => {
  const booking = await browser.newPage({ viewport: { width: 900, height: 600 } });
  try {
    await booking.setContent(await scene("A"));
    const read = await readFields(booking);
    assert(/^\d+:[0-9a-f]+$/.test(read.fingerprint || ""), "the read carries the form's fingerprint", read.fingerprint);
    const bundle = { [handleOf(read, "이름")]: "김예시", [handleOf(read, "항공편")]: "SH204" };
    await booking.evaluate(() => { document.querySelector('input[name="flight"]').closest("tr").querySelector("th").textContent = "편명"; });
    const stale = await fillBundle(booking, bundle, read.fingerprint);
    assert(stale.stale === true && stale.passes === 1, "a renamed field makes the form stale", stale);
    const held = await booking.evaluate(() => [document.querySelector('input[name="name"]').value,
      document.querySelector('input[name="flight"]').value]);
    assert(JSON.stringify(held) === JSON.stringify(["", ""]), "nothing was written", held);
    const again = await readFields(booking);
    assert(again.fingerprint !== read.fingerprint, "the form read again is another form");
    const fresh = await fillBundle(booking, { [handleOf(again, "이름")]: "김예시", [handleOf(again, "편명")]: "SH204" }, again.fingerprint);
    assert(!fresh.stale && fresh.results.every((result) => result.status === "set"), "the form as read takes the bundle", fresh);
    const script = await evalJson(booking, evalFormScript(`(() => {
      const read = zerocode.fields();
      document.querySelector('input[name="email"]').closest("tr").remove();
      return zerocode.fill({ [read.fields.find((f) => f.label === "차량번호").handle]: "12가3456" }, read);
    })()`));
    assert(script.ok && script.value.stale === true, "one eval's fill is held to the read it made", script);
    assert(await booking.evaluate(() => document.querySelector('input[name="car"]').value) === "", "and wrote nothing");
    return `${read.fingerprint} → ${again.fingerprint}`;
  } finally { await booking.close(); }
});

/* Dates on a page's own widgets (t-37883, third part): a date field the
 * page keeps from typing is filled on its calendar — the heading read as a
 * year and a month (its numbers, or the month's name the platform knows),
 * paged with its own controls, the day of that month pressed, never a day
 * the grid borrows from the months around it — a day that says its whole
 * date is trusted first, a field that takes typing gets its own format,
 * three selects take a date's numbers, and a calendar the fill cannot read
 * is answered `no_option` with what it shows. */
const PICKERS = `<!doctype html><html lang="ko"><meta charset="utf-8"><title>pickers</title>
<style>.pop{background:#fff;border:1px solid #999;padding:4px;display:inline-block}.pop .days{display:grid;grid-template-columns:repeat(7,30px)}</style>
<form>
  <div><label for="a">출발일</label> <input id="a" readonly></div>
  <div><label for="b">Return</label> <input id="b" readonly></div>
  <div><label for="c">체크인</label> <input id="c" readonly></div>
  <div><label for="d">생일</label> <input id="d" placeholder="YYYY.MM.DD"></div>
  <div><label for="e">기타일</label> <input id="e" readonly></div>
  <div>방문일 <select id="vy"><option value="">년</option><option>2026</option><option>2027</option></select>
    <select id="vm"><option value="">월</option></select> <select id="vd"><option value="">일</option></select></div>
</form>
<script>
  const pad = (n) => String(n).padStart(2, "0");
  for (let m = 1; m <= 12; m += 1) document.getElementById("vm").add(new Option(m + "월", String(m)));
  for (let d = 1; d <= 31; d += 1) document.getElementById("vd").add(new Option(d + "일", String(d)));
  const svg = '<svg width="10" height="10" viewBox="0 0 10 10"><path d="M2 5h6"/></svg>';
  // One calendar, shaped per field: a heading or none, pagers as symbols or
  // as icons, days that carry their whole date or only their number.
  function calendar(input, shape) {
    input.addEventListener("click", () => {
      if (input.parentElement.querySelector(".pop")) return;
      let [y, m] = shape.start;
      const pop = document.createElement("div");
      pop.className = "pop";
      const draw = () => {
        pop.replaceChildren();
        const top = document.createElement("div");
        const pager = (step) => {
          const button = document.createElement("button");
          button.type = "button";
          if (shape.pagers === "icons") { button.innerHTML = svg; button.setAttribute("aria-label", step < 0 ? "Previous month" : "Next month"); }
          else button.textContent = step < 0 ? "‹" : "›";
          button.addEventListener("click", () => { m += step; if (m < 1) { m = 12; y -= 1; } if (m > 12) { m = 1; y += 1; } draw(); });
          return button;
        };
        const close = document.createElement("button");
        close.type = "button";
        close.textContent = "×";
        close.addEventListener("click", () => pop.remove());
        if (shape.closeFirst) top.append(close);
        if (shape.pagers) top.append(pager(-1));
        if (shape.heading) { const h = document.createElement("strong"); h.textContent = shape.heading(y, m); top.append(h); }
        if (shape.pagers) top.append(pager(1));
        pop.append(top);
        const days = document.createElement("div");
        days.className = "days";
        const first = new Date(Date.UTC(y, m - 1, 1)).getUTCDay();
        const count = new Date(Date.UTC(y, m, 0)).getUTCDate();
        const before = new Date(Date.UTC(y, m - 1, 0)).getUTCDate();
        const cell = (d, own) => {
          const button = document.createElement("button");
          button.type = "button";
          button.textContent = String(d);
          if (shape.whole && own) button.dataset.date = y + "-" + pad(m) + "-" + pad(d);
          if (own && shape.off && shape.off(y, m, d)) button.disabled = true;
          button.addEventListener("click", () => { if (!own) return; input.value = y + "-" + pad(m) + "-" + pad(d); pop.remove(); });
          days.append(button);
        };
        for (let i = first - 1; i >= 0; i -= 1) cell(before - i, false);
        for (let d = 1; d <= count; d += 1) cell(d, true);
        for (let d = 1; (first + count + d - 1) % 7 !== 0; d += 1) cell(d, false);
        pop.append(days);
      };
      draw();
      input.parentElement.append(pop);
    });
  }
  const english = (y, m) => new Intl.DateTimeFormat("en", { month: "long", timeZone: "UTC" }).format(new Date(Date.UTC(y, m - 1, 1))) + " " + y;
  calendar(document.getElementById("a"), { start: [2026, 10], heading: (y, m) => y + "년 " + m + "월", pagers: "symbols", closeFirst: true,
    off: (y, m, d) => d === 31 });
  calendar(document.getElementById("b"), { start: [2026, 11], heading: english, pagers: "icons" });
  calendar(document.getElementById("c"), { start: [2026, 12], whole: true });
  calendar(document.getElementById("e"), { start: [2026, 10], pagers: "symbols" });
</script>`;

await test("a_date_is_filled_on_the_pages_own_calendar_and_in_the_fields_own_format", async () => {
  const pickers = await browser.newPage({ viewport: { width: 900, height: 700 } });
  try {
    await pickers.setContent(PICKERS);
    const filled = await fillBundle(pickers, { "#a": "2026-12-30", "#b": "2027-01-05", "#c": "2026-12-09", "#d": "2026-11-28",
      "#vy": "2026", "#vm": "11", "#vd": "04" });
    const said = Object.fromEntries(filled.results.map((result) => [result.handle, result.status]));
    assert(Object.values(said).every((status) => status === "set"), "every date took", filled.results);
    const held = await pickers.evaluate(() => ["a", "b", "c", "d", "vy", "vm", "vd"].map((id) => document.getElementById(id).value));
    assert(JSON.stringify(held) === JSON.stringify(["2026-12-30", "2027-01-05", "2026-12-09", "2026.11.28", "2026", "11", "4"]),
      "each field holds the asked date: paged two months past a closer, a month's name across a year, a day that says its date, the field's own format, three selects", held);
    return JSON.stringify(said);
  } finally { await pickers.close(); }
});

await test("a_calendar_the_fill_cannot_read_is_answered_with_what_it_shows", async () => {
  const pickers = await browser.newPage({ viewport: { width: 900, height: 700 } });
  try {
    await pickers.setContent(PICKERS);
    const filled = await fillBundle(pickers, { "#e": "2026-12-09" });
    const [only] = filled.results;
    assert(only.status === "no_option" && only.widget && only.widget.days && only.widget.pagers.length === 2,
      "no heading to read: no_option, with the calendar's days and pagers", only);
    const off = await fillBundle(pickers, { "#a": "2026-10-31" });
    assert(off.results[0].status === "no_option", "an off day is no choice", off.results[0]);
    assert(await pickers.evaluate(() => document.getElementById("a").value) === "", "and nothing was written");
    return JSON.stringify(only.widget);
  } finally { await pickers.close(); }
});

/* A calendar's own box may hold controls that page nothing (t-41387). A pager
 * is learnt by pressing a control with no words and reading the heading
 * again — so a control that SAYS it is something else (by its ARIA name, its
 * title, the picture it is drawn with, its value), and that something cannot
 * be taken back (a deletion, a send, a payment: the window's one table of
 * them, `request.holds`), is never pressed. A control with no name is pressed
 * as before — except one that says in its own markup that it submits its form
 * (`<button type="submit">`, `<input type="submit">`), whatever it is named:
 * pressed on a guess it would send the form. A `<button>` with no type at all
 * is still a candidate (calendar libraries draw their pagers so). */
const HELD_PICKERS = `<!doctype html><html lang="ko"><meta charset="utf-8"><title>held pickers</title>
<form>
  <div><label for="g">반납일</label> <input id="g" readonly></div>
  <div><label for="h">기타일</label> <input id="h" readonly></div>
  <div><label for="s">취소일</label> <input id="s" readonly></div>
  <div><label for="t">변경일</label> <input id="t" readonly></div>
</form>
<script>
  window.pressed = [];
  // The form is never really sent: a submit is counted and stopped.
  window.submitted = 0;
  document.querySelector("form").addEventListener("submit", (event) => { event.preventDefault(); window.submitted += 1; });
  const pad = (n) => String(n).padStart(2, "0");
  const dot = "data:image/gif;base64,R0lGODlhAQABAAAAACw=";
  // Controls that page nothing, each named another way — an ARIA name, a
  // title, an image's alt, an input button's value — and each counting its presses.
  const STRANGERS = '<button type="button" aria-label="삭제">🗑</button>'
    + '<button type="button" title="Submit">⏎</button>'
    + '<button type="button"><img alt="송금" src="' + dot + '"></button>'
    + '<input type="button" value="결제">';
  // Controls that say in their own markup they submit the form, with no name at all.
  const SUBMITTERS = '<button type="submit">▶</button>' + '<input type="submit" value="">';
  function calendar(input, shape) {
    input.addEventListener("click", () => {
      if (input.parentElement.querySelector(".pop")) return;
      let [y, m] = shape.start;
      const pop = document.createElement("div");
      pop.className = "pop";
      const draw = () => {
        pop.replaceChildren();
        const top = document.createElement("div");
        top.insertAdjacentHTML("beforeend", shape.controls || STRANGERS);
        for (const control of top.children) {
          control.addEventListener("click", () => window.pressed.push(
            control.getAttribute("aria-label") || control.title || control.value || (control.querySelector("img") || {}).alt || control.tagName));
        }
        const pager = (step) => {
          const button = document.createElement("button");
          // A pager drawn with no type at all, as calendar libraries do, stops the form's own submit.
          if (shape.typeless) button.addEventListener("click", (event) => event.preventDefault());
          else button.type = "button";
          button.textContent = step < 0 ? "‹" : "›";
          button.addEventListener("click", () => { m += step; if (m < 1) { m = 12; y -= 1; } if (m > 12) { m = 1; y += 1; } draw(); });
          return button;
        };
        if (shape.pagers) top.append(pager(-1));
        const heading = document.createElement("strong");
        heading.textContent = y + "년 " + m + "월";
        top.append(heading);
        if (shape.pagers) top.append(pager(1));
        pop.append(top);
        const days = document.createElement("div");
        days.className = "days";
        const first = new Date(Date.UTC(y, m - 1, 1)).getUTCDay();
        const count = new Date(Date.UTC(y, m, 0)).getUTCDate();
        const before = new Date(Date.UTC(y, m - 1, 0)).getUTCDate();
        const cell = (d, own) => {
          const button = document.createElement("button");
          button.type = "button";
          button.textContent = String(d);
          button.addEventListener("click", () => { if (!own) return; input.value = y + "-" + pad(m) + "-" + pad(d); pop.remove(); });
          days.append(button);
        };
        for (let i = first - 1; i >= 0; i -= 1) cell(before - i, false);
        for (let d = 1; d <= count; d += 1) cell(d, true);
        for (let d = 1; (first + count + d - 1) % 7 !== 0; d += 1) cell(d, false);
        pop.append(days);
      };
      draw();
      input.parentElement.append(pop);
    });
  }
  calendar(document.getElementById("g"), { start: [2026, 10], pagers: true });
  calendar(document.getElementById("h"), { start: [2026, 10], pagers: false });
  calendar(document.getElementById("s"), { start: [2026, 10], pagers: true, controls: SUBMITTERS, typeless: true });
  calendar(document.getElementById("t"), { start: [2026, 10], pagers: false, controls: SUBMITTERS });
</script>`;

await test("a_control_in_a_calendar_that_names_a_press_that_cannot_be_taken_back_is_never_pressed_to_learn_a_pager", async () => {
  const held = await browser.newPage({ viewport: { width: 900, height: 700 } });
  try {
    await held.setContent(HELD_PICKERS);
    const filled = await fillBundle(held, { "#g": "2026-12-09" });
    const [only] = filled.results;
    assert(only.status === "set", "the two controls with no name still paged the calendar to the day", only);
    assert(await held.evaluate(() => document.getElementById("g").value) === "2026-12-09", "and the day is the asked one");
    const pressed = await held.evaluate(() => window.pressed);
    assert(pressed.length === 0, "no control named for a deletion, a send or a payment was pressed", pressed);
    return "paged by the nameless pair; none of the four named controls was pressed";
  } finally { await held.close(); }
});

await test("a_calendar_whose_only_candidates_name_such_presses_is_answered_with_what_it_shows_and_none_is_pressed", async () => {
  const held = await browser.newPage({ viewport: { width: 900, height: 700 } });
  try {
    await held.setContent(HELD_PICKERS);
    const filled = await fillBundle(held, { "#h": "2026-12-09" });
    const [only] = filled.results;
    assert(only.status === "no_option" && only.widget && only.widget.month === "2026-10" && only.widget.pagers.length === 0,
      "no pager to page by: no_option, with the calendar's month and days and no pager", only);
    const pressed = await held.evaluate(() => window.pressed);
    assert(pressed.length === 0, "none of them was pressed to find out", pressed);
    assert(await held.evaluate(() => document.getElementById("h").value) === "", "and nothing was written");
    return JSON.stringify(only.widget);
  } finally { await held.close(); }
});

await test("a_control_that_says_it_submits_its_form_is_never_pressed_to_learn_a_pager_but_a_pager_with_no_type_still_is", async () => {
  const held = await browser.newPage({ viewport: { width: 900, height: 700 } });
  try {
    await held.setContent(HELD_PICKERS);
    const filled = await fillBundle(held, { "#s": "2026-12-09" });
    const [only] = filled.results;
    assert(only.status === "set", "the pagers with no type attribute still paged the calendar to the day", only);
    assert(await held.evaluate(() => document.getElementById("s").value) === "2026-12-09", "and the day is the asked one");
    const sent = await held.evaluate(() => ({ submitted: window.submitted, pressed: window.pressed }));
    assert(sent.submitted === 0 && sent.pressed.length === 0, "no control that says it submits was pressed, and the form was never sent", sent);
    return "paged by the typeless pair; the form was not sent";
  } finally { await held.close(); }
});

await test("a_calendar_whose_only_candidates_say_they_submit_is_answered_with_what_it_shows_and_the_form_is_not_sent", async () => {
  const held = await browser.newPage({ viewport: { width: 900, height: 700 } });
  try {
    await held.setContent(HELD_PICKERS);
    const filled = await fillBundle(held, { "#t": "2026-12-09" });
    const [only] = filled.results;
    assert(only.status === "no_option" && only.widget && only.widget.month === "2026-10" && only.widget.pagers.length === 0,
      "no pager to page by: no_option, with the calendar's month and days and no pager", only);
    const sent = await held.evaluate(() => ({ submitted: window.submitted, pressed: window.pressed }));
    assert(sent.submitted === 0 && sent.pressed.length === 0, "none of them was pressed to find out, and the form was never sent", sent);
    assert(await held.evaluate(() => document.getElementById("t").value) === "", "and nothing was written");
    return JSON.stringify(only.widget);
  } finally { await held.close(); }
});

/* Parts of one value with the page's unit words between them — an hour
 * and a minute (시 · 분), a year, a month and a day (년 · 월 · 일) — are one
 * caption's parts, not a caption each: the words between two parts are the
 * first part's unit. */
await test("parts_with_unit_words_between_them_are_one_captions_parts", async () => {
  const units = await browser.newPage();
  try {
    await units.setContent(`<!doctype html><html lang="ko"><meta charset="utf-8"><form>
      <div class="row"><div class="cap">출차 시각<em>*</em></div><div class="ctl">
        <select id="h"><option value="">시</option><option>00</option><option>09</option><option>18</option></select><span>시</span>
        <select id="m"><option value="">분</option><option>00</option><option>30</option></select><span>분</span></div></div>
      <div class="row"><div class="cap">생년월일</div><div class="ctl">
        <select id="y"><option value="">년</option><option>1988</option></select> 년
        <select id="mo"><option value="">월</option><option>2</option></select> 월
        <select id="d"><option value="">일</option><option>14</option></select> 일</div></div></form>`);
    const read = await readFields(units);
    const labels = read.fields.map((field) => field.label);
    assert(JSON.stringify(labels) === JSON.stringify(["출차 시각 (1/2)", "출차 시각 (2/2)", "생년월일 (1/3)", "생년월일 (2/3)", "생년월일 (3/3)"]),
      "the unit words between parts make no caption of their own", labels);
    const filled = await fillBundle(units, { [handleOf(read, "출차 시각 (1/2)")]: "18", [handleOf(read, "출차 시각 (2/2)")]: "00" });
    assert(filled.results.every((result) => result.status === "set"), "each part takes its number", filled.results);
    return labels.join(" · ");
  } finally { await units.close(); }
});

/* A dropdown drawn with no ARIA at all — a focusable box with a list beside
 * it, items filled late after another choice — is read as a field with its
 * items for choices, and filled by opening it and pressing the item. */
await test("a_focusable_box_with_a_list_beside_it_is_a_dropdown", async () => {
  const plain = await browser.newPage();
  try {
    await plain.setContent(`<!doctype html><html lang="ko"><meta charset="utf-8"><form>
      <div class="row"><div class="cap">터미널</div><div class="ctl"><select id="t"><option value="">선택</option><option value="A">A동</option><option value="B">B동</option></select></div></div>
      <div class="row"><div class="cap">주차장</div><div class="ctl"><div class="dd">
        <div class="btn" tabindex="0"><span class="val">터미널을 먼저 고르세요</span></div><ul class="list" hidden></ul></div></div></div></form>
      <script>
        const box = document.querySelector(".dd"), button = box.querySelector(".btn"), list = box.querySelector(".list");
        button.addEventListener("click", () => { list.hidden = !list.hidden; });
        list.addEventListener("click", (event) => {
          const item = event.target.closest("li");
          if (!item) return;
          button.querySelector(".val").textContent = item.textContent;
          box.dataset.value = item.dataset.value;
          list.hidden = true;
        });
        document.getElementById("t").addEventListener("change", (event) => setTimeout(() => {
          list.replaceChildren(...(event.target.value === "B" ? ["P3 단기", "P4 장기"] : ["P1 단기", "P2 장기"]).map((words, at) => {
            const item = document.createElement("li"); item.textContent = words; item.dataset.value = "p" + at; return item; }));
          button.querySelector(".val").textContent = "주차장을 고르세요";
        }, 150));
      </script>`);
    const read = await readFields(plain);
    const lot = read.fields.find((field) => field.label === "주차장");
    assert(lot && lot.kind === "dropdown", "a focusable box with a list beside it is a dropdown", read.fields);
    const filled = await fillBundle(plain, { [handleOf(read, "터미널")]: "B동", [lot.handle]: "P4 장기" });
    assert(filled.results.every((result) => result.status === "set"), "the late items are chosen by their words", filled.results);
    const shown = await plain.evaluate(() => [document.querySelector(".val").textContent, document.querySelector(".dd").dataset.value]);
    assert(JSON.stringify(shown) === JSON.stringify(["P4 장기", "p1"]), "the page took the item", shown);
    return `${filled.passes} passes`;
  } finally { await plain.close(); }
});

/* A net under every rule: what a person can press or focus beside the
 * fields that the read names no kind for — a box that shows a pointer, a
 * focusable chip with no list, a text with a click handler — is said as
 * \`unknown\` with its words and caption; what nobody can press (a notice), a
 * label of a field, a link, stays out. */
await test("a_pressable_thing_the_read_cannot_name_is_said_as_unknown", async () => {
  const odd = await browser.newPage();
  try {
    await odd.setContent(`<!doctype html><html lang="ko"><meta charset="utf-8"><form>
      <div class="row"><div class="cap">지역</div><div class="ctl"><div id="region" style="cursor:pointer">지역을 고르세요 <b>▾</b></div></div></div>
      <div class="row"><div class="cap">옵션</div><div class="ctl"><div class="chip" tabindex="0">아침 식사</div></div></div>
      <div class="row"><div class="cap">이름</div><div class="ctl"><input id="name"></div></div>
      <p class="notice">안내: 입력한 내용은 저장됩니다</p>
      <label style="cursor:pointer"><input type="checkbox" id="agree"> 동의</label>
      <a href="#help">도움말</a> <span id="more" onclick="void 0">더보기</span>
    </form>`);
    const read = await readFields(odd);
    const unknown = (read.unknowns || []).map((thing) => thing.label);
    assert(JSON.stringify(unknown) === JSON.stringify(["지역을 고르세요 ▾", "아침 식사", "더보기"]),
      "each pressable thing of no kind is said once, outermost, in the page's order — the notice, the label and the link are not", read.unknowns);
    assert(read.unknowns[0].caption === "지역" && read.unknowns[1].caption === "옵션", "with the caption beside it", read.unknowns);
    for (const thing of read.unknowns) {
      assert(await odd.evaluate((handle) => document.querySelectorAll(handle).length === 1, thing.handle), `the handle ${thing.handle} finds it`);
    }
    assert(read.fields.map((field) => field.label).join(",") === "이름,동의", "the fields are still the fields", read.fields);
    return unknown.join(" · ");
  } finally { await odd.close(); }
});

/* A field under a term (or a header) in a box of its own — no other field
 * in it — with its own words before it there, as a code box drawn under the
 * phone it was sent to, is named by those words; the parts in the term's own
 * cell, and a box with no words of its own, share the term. */
await test("a_field_in_its_own_box_under_a_term_is_named_by_its_own_words", async () => {
  const nested = await browser.newPage();
  try {
    await nested.setContent(`<!doctype html><html lang="ko"><meta charset="utf-8"><form><dl>
      <dt>휴대전화<em>*</em></dt><dd><select id="p1"><option>010</option><option>011</option></select><span>-</span>
        <input id="p2" maxlength="4"><span>-</span><input id="p3" maxlength="4">
        <div class="code"><div class="row"><span>인증번호</span><span class="w"><input id="code" maxlength="6"></span><button type="button">확인</button></div></div></dd>
      <dt>요금</dt><dd><div class="price"><input id="fee"></div></dd></dl>
      <table><tr><th>차량번호</th><td><div class="box"><span>앞자리</span><input id="car"></div></td></tr></table></form>`);
    const read = await readFields(nested);
    const labels = Object.fromEntries(read.fields.map((field) => [field.handle, field.label]));
    assert(labels["#code"] === "인증번호", "the code box is named by its own words, not the phone's term", labels);
    assert(labels["#car"] === "앞자리", "so is a box of its own under a header", labels);
    assert(String(labels["#fee"]).startsWith("요금"), "a box with no words of its own keeps the term", labels);
    assert(["#p1", "#p2", "#p3"].every((handle) => String(labels[handle]).startsWith("휴대전화")),
      "the parts in the term's own cell keep it", labels);
    return JSON.stringify(labels);
  } finally { await nested.close(); }
});

await test("a_frame_of_another_origin_is_named_not_read", async () => {
  const sealed = await browser.newPage();
  try {
    await sealed.setContent(`<form><label>이름 <input id="who"></label>
      <iframe id="card" src="data:text/html,<input id=inside>"></iframe></form>`);
    await sealed.waitForTimeout(100);
    const read = await readFields(sealed);
    assert(read.fields.length === 1 && read.fields[0].handle === "#who", "only the page's own field", read.fields);
    assert(read.sealedFrames.length === 1, "the other origin's frame is named", read.sealedFrames);
    return JSON.stringify(read.sealedFrames);
  } finally { await sealed.close(); }
});

/* A table cell with no header of its row is named by the header over its
 * column (t-41387): HTML's own table meaning, the same for any table whose
 * head says so — a `thead`, or a first row of nothing but headers, a head cell
 * that spans several columns covering each of them. A row's own header still
 * names its cells first, and a cell under no head keeps the words before it. */
await test("a_field_in_a_table_cell_with_no_row_header_is_named_by_the_header_over_its_column", async () => {
  const grid = await browser.newPage();
  try {
    await grid.setContent(`<!doctype html><html lang="en"><meta charset="utf-8"><form>
      <table><thead><tr><th>Seat</th><th colspan="2">Guest</th><th>Meal</th></tr></thead><tbody>
        <tr><td>A1</td><td><input id="first"></td><td><input id="last"></td>
          <td><select id="meal"><option value="">-</option><option>Fish</option><option>Veg</option></select></td></tr>
      </tbody></table>
      <table><tr><th>Name</th><th>Phone</th></tr><tr><td><input id="n"></td><td><input id="p"></td></tr></table>
      <table><thead><tr><td></td><th>Mon</th></tr></thead><tbody><tr><th scope="row">Morning</th><td><input id="mm"></td></tr></tbody></table>
      <table><tr><td>Voucher</td><td><input id="v"></td></tr></table>
    </form>`);
    const labels = Object.fromEntries((await readFields(grid)).fields.map((field) => [field.handle, field.label]));
    assert(labels["#first"] === "Guest #1" && labels["#last"] === "Guest #2" && labels["#meal"] === "Meal",
      "under a thead, a head cell that spans two columns covers both of them — and two fields of one name are told by number", labels);
    assert(labels["#n"] === "Name" && labels["#p"] === "Phone", "a first row of nothing but headers is the head too", labels);
    assert(labels["#mm"] === "Morning", "a row's own header still names its cell first", labels);
    assert(labels["#v"] === "Voucher", "a cell under no head keeps the words before it", labels);
    return JSON.stringify(labels);
  } finally { await grid.close(); }
});

/* The reason a field is off, and what there is to read (t-41387): a field
 * that is disabled or cannot be written says the words the page gives about
 * it — what its aria-describedby names, else its title, else the words of the
 * nearest box around it that holds no other field, minus its own labels —
 * and a field that is on says nothing more. A box the page lets scroll that
 * has more below what is shown is said; one read to its end, one with
 * nothing more to read, one holding a field and one not drawn are not. */
const REASONS = `<!doctype html><html lang="en"><meta charset="utf-8">
<style>.box{height:60px;overflow:auto;border:1px solid #999}</style>
<form>
  <div class="row"><label for="state">State</label> <select id="state" disabled aria-describedby="why-state"><option>-</option></select>
    <p id="why-state">Choose a country first.</p></div>
  <div class="row"><label for="promo">Promo code</label> <input id="promo" disabled title="Available after you create an account"></div>
  <div class="row"><label><input type="checkbox" id="accept" disabled> I have read the licence</label>
    <span class="note">Scroll the licence to its end to switch this on.</span></div>
  <div class="row"><label for="ref">Reference</label> <input id="ref" readonly value="X1">
    <small>Filled in by the shop; you cannot change it.</small></div>
  <div class="row"><label for="nick">Nickname</label> <input id="nick"><small>Shown to other guests.</small></div>
  <div id="licence" class="box"><p>Licence terms 1</p><p>2</p><p>3</p><p>4</p><p>5</p><p>6</p><p>7</p><p>8</p></div>
  <div id="read" class="box"><p>Privacy 1</p><p>2</p><p>3</p><p>4</p><p>5</p><p>6</p><p>7</p><p>8</p></div>
  <div id="short" class="box" style="height:200px"><p>One line.</p></div>
  <div id="editor" class="box"><textarea id="bio" rows="8"></textarea></div>
  <div id="gone" class="box" style="display:none"><p>1</p><p>2</p><p>3</p><p>4</p><p>5</p><p>6</p><p>7</p><p>8</p></div>
</form>
<script>document.getElementById("read").scrollTop = document.getElementById("read").scrollHeight;</script>`;

await test("a_field_that_is_off_or_cannot_be_written_says_the_words_the_page_gives_about_it", async () => {
  const why = await browser.newPage({ viewport: { width: 900, height: 900 } });
  try {
    await why.setContent(REASONS);
    const fields = byHandle(await readFields(why));
    assert(fields["#state"].hint === "Choose a country first.", "the words its aria-describedby names", fields["#state"]);
    assert(fields["#promo"].hint === "Available after you create an account", "else its title, a label having named it", fields["#promo"]);
    assert(fields["#accept"].hint === "Scroll the licence to its end to switch this on.", "else the words in its box beside its own label", fields["#accept"]);
    assert(fields["#ref"].hint === "Filled in by the shop; you cannot change it.", "a field that cannot be written, too", fields["#ref"]);
    assert(!fields["#nick"].hint, "a field that is on says nothing more", fields["#nick"]);
    return fields["#accept"].hint;
  } finally { await why.close(); }
});

await test("a_box_with_more_to_read_below_it_is_said_and_one_read_to_its_end_or_holding_a_field_is_not", async () => {
  const why = await browser.newPage({ viewport: { width: 900, height: 900 } });
  try {
    await why.setContent(REASONS);
    const read = await readFields(why);
    const boxes = (read.scrollBoxes || []).map((box) => `${box.handle} ${box.label.slice(0, 15)}`);
    assert(JSON.stringify(boxes) === JSON.stringify(["#licence Licence terms 1"]),
      "only the box with more below it: not the one read to its end, the short one, the one holding a field, the one not drawn", read.scrollBoxes);
    await why.evaluate(() => { const box = document.getElementById("licence"); box.scrollTop = box.scrollHeight; });
    assert(((await readFields(why)).scrollBoxes || []).length === 0, "and once it is read to its end it is not said again");
    return boxes.join(" · ");
  } finally { await why.close(); }
});

await test("a_fill_of_a_field_that_is_off_says_the_words_the_page_gives_about_it", async () => {
  const why = await browser.newPage({ viewport: { width: 900, height: 900 } });
  try {
    await why.setContent(REASONS);
    const filled = await fillBundle(why, { "#accept": true });
    const [only] = filled.results;
    assert(only.status === "disabled" && only.hint === "Scroll the licence to its end to switch this on.",
      "disabled, and why", only);
    return only.hint;
  } finally { await why.close(); }
});

/* The buttons a fill leaves (t-41387): a step's button that turns on once its
 * field is right, and one the page takes away, are said in the fill's own
 * answer — the agent presses the step's button with no read between — by the
 * page's own states: `disabled`, and not drawn. */
const STEP_BUTTONS = `<!doctype html><html lang="en"><meta charset="utf-8">
<form onsubmit="return false">
  <label for="mail">Contact email</label> <input id="mail" type="email">
  <button type="button" id="go" disabled>Continue</button>
  <button type="button" id="back">Go back</button>
  <button type="button" id="later">Skip this step</button>
</form>
<script>
  const mail = document.getElementById("mail");
  mail.addEventListener("input", () => {
    const ok = mail.validity.valid && mail.value !== "";
    document.getElementById("go").disabled = !ok;
    document.getElementById("later").hidden = ok;
  });
</script>`;
const buttonStates = (actions) => JSON.stringify(Object.fromEntries((Array.isArray(actions) ? actions : [])
  .map((action) => [action.label, action.disabled ? "off" : "on"])));

await test("a_fill_answers_the_state_of_the_buttons_the_form_has_after_it", async () => {
  const step = await browser.newPage({ viewport: { width: 900, height: 600 } });
  try {
    await step.setContent(STEP_BUTTONS);
    const before = await readFields(step);
    assert(buttonStates(before.actions) === JSON.stringify({ "Continue": "off", "Go back": "on", "Skip this step": "on" }),
      "before the fill the step's button is off", before.actions);
    const filled = await fillBundle(step, { "#mail": "kim@example.com" }, before.fingerprint);
    assert(buttonStates(filled.actions) === JSON.stringify({ "Continue": "on", "Go back": "on" }),
      "after it the step's button is on and the one the page took away is not there", filled.actions);
    assert(filled.fingerprint === before.fingerprint, "the form's own fields did not change", filled);
    return buttonStates(filled.actions);
  } finally { await step.close(); }
});

await test("a_fill_inside_an_eval_answers_the_buttons_after_it_too", async () => {
  const step = await browser.newPage({ viewport: { width: 900, height: 600 } });
  try {
    await step.setContent(STEP_BUTTONS);
    const script = `(() => { const read = zerocode.fields();
      const filled = zerocode.fill({ "#mail": "kim@example.com" }, read);
      return filled.actions; })()`;
    const answer = await evalJson(step, evalFormScript(script));
    assert(answer.ok, "the one script ran", answer);
    assert(buttonStates(answer.value) === JSON.stringify({ "Continue": "on", "Go back": "on" }),
      "the eval's fill says the buttons after it", answer.value);
    return buttonStates(answer.value);
  } finally { await step.close(); }
});

/* The page a fill reads is the page after it has settled (t-41387). A page that
 * updates after its input handler has returned — in a microtask (Vue's and
 * Svelte's nextTick, React's batched update), after a check that says it is busy
 * — shows a read made in the same instant as the write the screen as it was. So
 * the fill waits,
 * with the window's own settle (the one a press by number waits with), between
 * its write and its read: the buttons, each field's value and error and what is
 * left are the settled screen's; a page that never stands still is said so. */
const LATE_PAGE = (script) => `<!doctype html><html lang="en"><meta charset="utf-8">
<form onsubmit="return false">
  <label for="mail">Contact email</label> <input id="mail" type="email" aria-describedby="mail-err">
  <p id="mail-err" role="alert"></p>
  <button type="button" id="go" disabled>Continue</button>
  <p id="tick">0</p>
</form>
<script>${script}</script>`;
const AFTER_A_MICROTASK = LATE_PAGE(`
  const mail = document.getElementById("mail"), go = document.getElementById("go");
  mail.addEventListener("input", () => queueMicrotask(() => { go.disabled = !mail.value.includes("@"); }));`);
// An async check, as a page with one writes it: it says it is busy while the check runs
// (`aria-busy`), and shows its verdict 150 ms after the input.
const AFTER_A_BUSY_CHECK = LATE_PAGE(`
  const form = document.querySelector("form"), mail = document.getElementById("mail"), err = document.getElementById("mail-err");
  mail.removeAttribute("type");
  let timer = 0;
  mail.addEventListener("input", () => {
    clearTimeout(timer);
    form.setAttribute("aria-busy", "true");
    timer = setTimeout(() => {
      const bad = !mail.value.includes("@");
      mail.setAttribute("aria-invalid", bad ? "true" : "false");
      err.textContent = bad ? "Enter a valid email" : "";
      form.setAttribute("aria-busy", "false");
    }, 150);
  });`);
const NEVER_STILL = LATE_PAGE(`
  let ticks = 0;
  setInterval(() => { document.getElementById("tick").textContent = String(++ticks); }, 20);`);

await test("a_fill_reads_the_buttons_a_page_turns_on_after_its_handler_has_returned", async () => {
  const late = await browser.newPage({ viewport: { width: 900, height: 600 } });
  try {
    await late.setContent(AFTER_A_MICROTASK);
    const before = await readFields(late);
    const filled = await fillBundle(late, { "#mail": "kim@example.com" }, before.fingerprint);
    assert(buttonStates(filled.actions) === JSON.stringify({ "Continue": "on" }),
      "the button the page turned on after the write is on in the fill's answer", filled.actions);
    assert(filled.moving === false, "and the page stood still when it was read", filled.moving);
    return buttonStates(filled.actions);
  } finally { await late.close(); }
});

await test("a_fill_reads_the_error_a_check_shows_after_the_write_while_the_page_says_it_is_busy", async () => {
  const late = await browser.newPage({ viewport: { width: 900, height: 600 } });
  try {
    await late.setContent(AFTER_A_BUSY_CHECK);
    const before = await readFields(late);
    const filled = await fillBundle(late, { "#mail": "nope" }, before.fingerprint);
    const [only] = filled.results;
    assert(only.status === "set" && only.error === "Enter a valid email",
      "the error the page showed 150 ms after the write, once it was no longer busy, is the field's error", only);
    assert(filled.left.some((field) => field.error === "Enter a valid email"), "and the field is left with it", filled.left);
    return only.error;
  } finally { await late.close(); }
});

await test("a_fill_on_a_page_that_never_stands_still_ends_and_says_it_is_still_changing", async () => {
  const late = await browser.newPage({ viewport: { width: 900, height: 600 } });
  try {
    await late.setContent(NEVER_STILL);
    const before = await readFields(late);
    const began = Date.now();
    const filled = await fillBundle(late, { "#mail": "kim@example.com" }, before.fingerprint);
    const took = Date.now() - began;
    assert(filled.moving === true, "the fill says the page was still changing", filled.moving);
    assert(took < 4000, "and it ended inside the settle's time, not waited for", took);
    return `${took} ms`;
  } finally { await late.close(); }
});

// A check with no signal: a timer shows its verdict 150 ms after the input and the page says
// nothing while it waits. The settle is the window's own — a page that stood still for the
// quiet window is settled — so the fill does not see this error; the answer must not call
// itself the page's last word (the core's last line says so), and the next read sees it.
const AFTER_A_SILENT_TIMER = LATE_PAGE(`
  const mail = document.getElementById("mail"), err = document.getElementById("mail-err");
  mail.removeAttribute("type");
  let timer = 0;
  mail.addEventListener("input", () => {
    clearTimeout(timer);
    timer = setTimeout(() => {
      const bad = !mail.value.includes("@");
      mail.setAttribute("aria-invalid", bad ? "true" : "false");
      err.textContent = bad ? "Enter a valid email" : "";
    }, 150);
  });`);

await test("a_fill_does_not_see_an_error_a_silent_timer_shows_later_and_the_next_read_does", async () => {
  const late = await browser.newPage({ viewport: { width: 900, height: 600 } });
  try {
    await late.setContent(AFTER_A_SILENT_TIMER);
    const before = await readFields(late);
    const filled = await fillBundle(late, { "#mail": "nope" }, before.fingerprint);
    const [only] = filled.results;
    assert(only.status === "set" && only.error === "" && filled.left.length === 0,
      "the error the page shows after a silent 150 ms is not in the fill's answer — the limit, pinned", only);
    assert(filled.moving === false, "the page stood still for the quiet window, so the fill took it for settled", filled.moving);
    await late.waitForTimeout(300);
    const next = byHandle(await readFields(late));
    assert(next["#mail"].error === "Enter a valid email", "and the next read has it", next["#mail"]);
    return `fill: "${only.error}"; next read: "${next["#mail"].error}"`;
  } finally { await late.close(); }
});

await test("a_fill_read_in_another_document_than_the_write_says_the_page_was_replaced", async () => {
  const late = await browser.newPage({ viewport: { width: 900, height: 600 } });
  try {
    await late.setContent(AFTER_A_MICROTASK);
    const wrote = await evalJson(late, fillWriteScript([{ handle: "#mail", value: "kim@example.com" }], null));
    assert(wrote.ok && Array.isArray(wrote.value.held) && typeof wrote.value.epoch === "string" && wrote.value.wrote === true,
      "the write half answers what it held, the document it wrote in and that it wrote", wrote);
    const read = await evalJson(late, fillReadScript(wrote.value.held, "another-document"));
    assert(read.ok && read.value.results[0].status === "replaced",
      "read in a document that is not the one written in, the field is said replaced", read);
    const same = await evalJson(late, fillReadScript(wrote.value.held, wrote.value.epoch));
    assert(same.ok && same.value.results[0].status === "set", "and read in its own document it is read back", same);
    return read.value.results[0].status;
  } finally { await late.close(); }
});

/* What a review of the nets found wrong on pages shaped like our own scenes (t-41387):
 * a hint is the words nearest the field that are no one else's — not the text of
 * the box around it, a symbol, a hidden calendar or a field's own caption; a
 * column head follows the table's spans; a list a field offers is no box to read
 * to its end. */
const HINT_SHAPES = `<!doctype html><html lang="en"><meta charset="utf-8">
<style>.box{height:60px;overflow:auto;border:1px solid #999}</style>
<form>
  <div class="field">
    <label id="tl">Service terms <span class="req">*</span></label>
    <div id="tbox" class="box" tabindex="0"><p>Article 1. These terms apply to every booking.</p><p>Article 2. The deposit is kept.</p><p>Article 3. Cancel before noon.</p><p>Article 4. Pets are not allowed.</p><p>Article 5. Quiet after ten.</p><p>Article 6. Keys stay with the guest.</p><p>Article 7. Damage is charged.</p><p>Article 8. These terms end on checkout.</p></div>
    <div class="hint">Read the terms to the end to accept them.</div>
    <label class="chk"><input type="checkbox" id="accept" disabled><span>I accept the terms</span></label>
    <div id="err" role="alert" hidden>You must accept the terms.</div>
  </div>
  <div class="field">
    <label for="arrive">Arrival</label>
    <input id="arrive" readonly placeholder="YYYY-MM-DD">
    <span class="req">*</span>
    <div class="cal" hidden>Nov 2026 Mo Tu We Th Fr Sa Su</div>
  </div>
  <div class="field">
    <div id="news" role="checkbox" aria-checked="false" aria-disabled="true" tabindex="-1">Send me news</div>
  </div>
  <div class="field">
    <label for="pickup">Pickup</label> <input id="pickup" disabled> <span class="note">Choose a branch first.</span>
    <span class="note">Opening hours vary.</span>
  </div>
</form>`;
const SPAN_TABLES = `<!doctype html><html lang="en"><meta charset="utf-8">
<table id="two"><thead><tr><th rowspan="2">Name</th><th colspan="2">Contact</th></tr><tr><th>Phone</th><th>Email</th></tr></thead>
  <tbody><tr><td><input id="n1"></td><td><input id="p1"></td><td><input id="e1"></td></tr></tbody></table>
<table id="down"><thead><tr><th>Group</th><th>Item</th></tr></thead>
  <tbody><tr><td rowspan="2"><input id="g1"></td><td><input id="h1"></td></tr><tr><td><input id="h2"></td></tr></tbody></table>
<table id="title"><tr><th colspan="2">Booking</th></tr><tr><td>Guest</td><td><input id="t1"></td></tr></table>`;
const OFFERED_LIST = `<!doctype html><html lang="en"><meta charset="utf-8">
<style>#lst{height:40px;overflow:auto;margin:0;padding:0}</style>
<form>
  <input id="nick" disabled>
  <div id="city" role="combobox" aria-expanded="true" aria-controls="lst" tabindex="0">Choose a city</div>
  <ul id="lst" role="listbox"><li role="option">Seoul</li><li role="option">Busan</li><li role="option">Daegu</li><li role="option">Jeju</li><li role="option">Ulsan</li></ul>
</form>`;

await test("a_hint_is_the_words_nearest_the_field_not_the_box_around_it_a_symbol_a_hidden_calendar_or_its_own_words", async () => {
  const shapes = await browser.newPage({ viewport: { width: 900, height: 900 } });
  try {
    await shapes.setContent(HINT_SHAPES);
    const fields = byHandle(await readFields(shapes));
    assert(fields["#accept"].hint === "Read the terms to the end to accept them.",
      "the sentence beside the checkbox, not the terms box, its heading or a hidden error", fields["#accept"]);
    assert(!fields["#arrive"].hint, "a required mark and a hidden calendar are no hint", fields["#arrive"]);
    assert(!fields["#news"].hint, "a custom checkbox's own words are its name, not a hint", fields["#news"]);
    assert(fields["#pickup"].hint === "Choose a branch first.", "the nearest words after the field, not the second note", fields["#pickup"]);
    return fields["#accept"].hint;
  } finally { await shapes.close(); }
});

await test("a_column_head_follows_the_tables_spans_and_a_title_over_every_column_is_no_column_head", async () => {
  const spans = await browser.newPage({ viewport: { width: 900, height: 700 } });
  try {
    await spans.setContent(SPAN_TABLES);
    const labels = Object.fromEntries((await readFields(spans)).fields.map((field) => [field.handle, field.label]));
    assert(JSON.stringify([labels["#n1"], labels["#p1"], labels["#e1"]]) === JSON.stringify(["Name", "Phone", "Email"]),
      "a head cell spanning two rows names its own column, the head row under it the next two", labels);
    assert(JSON.stringify([labels["#g1"], labels["#h1"], labels["#h2"]]) === JSON.stringify(["Group", "Item #1", "Item #2"]),
      "a body cell spanning two rows leaves its column to the row below — and the two cells of one name are told by number", labels);
    assert(labels["#t1"] === "Guest", "a head cell over every column is the table's title: the words before the field stand", labels);
    return JSON.stringify(labels);
  } finally { await spans.close(); }
});

await test("a_list_a_field_offers_is_no_box_to_read_to_its_end", async () => {
  const offered = await browser.newPage({ viewport: { width: 900, height: 700 } });
  try {
    await offered.setContent(OFFERED_LIST);
    const read = await readFields(offered);
    const city = read.fields.find((field) => field.handle === "#city");
    assert(city && city.options.length === 5, "the dropdown's choices are read as the field's options", read.fields);
    assert((read.scrollBoxes || []).length === 0, "and the list that holds them is not named a box with more to read", read.scrollBoxes);
    return "no box";
  } finally { await offered.close(); }
});

/* What a read still leaves out (t-41592), each on a page of the shape the gap was found on and none a scene of the bench. */

/* A step with no field of its own — a review, a confirmation, a modal that asks
 * yes or no — still has its buttons: the drawn forms, dialogs and main regions
 * that hold no field are read for them too, and a page with no such box at all
 * is read whole. The site's menu outside such a box is still not said. */
await test("a_step_with_no_field_of_its_own_still_says_its_buttons", async () => {
  const bare = await browser.newPage({ viewport: { width: 900, height: 600 } });
  try {
    await bare.setContent(`<!doctype html><html lang="en"><meta charset="utf-8"><header><button id="menu">Menu</button></header>
      <main><h1>Review</h1><form id="review"><p>Check what you entered.</p>
        <button type="button" id="back">Back</button><button type="button" id="send" disabled>Send</button></form></main>`);
    const step = await readFields(bare);
    assert(step.fields.length === 0, "a review has no field", step.fields);
    assert(JSON.stringify(step.actions.map((action) => [action.label, action.disabled])) === JSON.stringify([["Back", false], ["Send", true]]),
      "its buttons are said, on and off — and not the site's menu outside it", step.actions);
    await bare.setContent(`<!doctype html><html lang="en"><meta charset="utf-8"><header><button id="menu">Menu</button></header>
      <form id="order"><label>Name <input id="name"></label><button type="button" id="next">Next</button></form>
      <div role="dialog" aria-label="Leave this page?"><p>Your answers are not saved.</p>
        <button type="button" id="stay">Stay</button><button type="button" id="leave">Leave</button></div>`);
    const asked = await readFields(bare);
    assert(JSON.stringify(asked.actions.map((action) => action.label)) === JSON.stringify(["Next", "Stay", "Leave"]),
      "the buttons of a dialog that holds no field are said beside the form's — and still not the menu", asked.actions);
    await bare.setContent(`<!doctype html><html lang="en"><meta charset="utf-8"><h1>All done</h1><p>Thank you.</p>
      <button type="button" id="again">Start again</button>`);
    const done = await readFields(bare);
    assert(JSON.stringify(done.actions.map((action) => action.label)) === JSON.stringify(["Start again"]),
      "a page with no field and no box to read is read whole", done.actions);
    return `${step.actions.length} + ${asked.actions.length} + ${done.actions.length} buttons`;
  } finally { await bare.close(); }
});

/* A button that opens a list (`aria-haspopup="listbox"`) is a field: named by the
 * words beside it, never by the value it shows; holding what it shows; saying
 * whether its list is open; offering the items of the list it controls or the
 * one beside it, drawn or not; and filled by opening it and pressing the item.
 * A button that opens a menu is still a button. */
const LIST_BUTTONS = `<!doctype html><html lang="en"><meta charset="utf-8"><style>[hidden]{display:none}</style>
<form>
  <div class="row"><span class="cap">Branch</span>
    <button type="button" id="branch" aria-haspopup="listbox" aria-expanded="false" aria-controls="branches">Select a branch</button>
    <ul id="branches" role="listbox" hidden><li role="option" data-value="hb">Hanbit</li><li role="option" data-value="sl">Seoul</li></ul></div>
  <div class="row"><label id="dl">Doctor</label>
    <button type="button" id="doctor" aria-haspopup="listbox" aria-expanded="false" aria-controls="docs" aria-labelledby="dl doctor">Dr. Seo</button>
    <ul id="docs" role="listbox" hidden><li role="option">Dr. Seo</li><li role="option">Dr. Min</li></ul></div>
  <div class="row"><span class="cap">Phone prefix</span>
    <button type="button" id="prefix" aria-haspopup="listbox" aria-expanded="false">US +1</button>
    <ul role="listbox" hidden><li role="option">US +1</li><li role="option">KR +82</li></ul></div>
  <button type="button" id="more" aria-haspopup="menu">More</button>
  <button type="button" id="go">Go</button>
</form>
<script>
  for (const button of document.querySelectorAll('[aria-haspopup="listbox"]')) {
    const list = document.getElementById(button.getAttribute("aria-controls")) || button.nextElementSibling;
    button.addEventListener("click", () => {
      const open = button.getAttribute("aria-expanded") !== "true";
      button.setAttribute("aria-expanded", String(open));
      list.hidden = !open;
    });
    list.addEventListener("click", (event) => {
      const option = event.target.closest("[role=option]");
      if (!option) return;
      button.textContent = option.textContent;
      button.dataset.value = option.dataset.value || option.textContent;
      button.setAttribute("aria-expanded", "false");
      list.hidden = true;
    });
  }
</script>`;
await test("a_button_that_opens_a_list_is_a_field_named_by_the_words_beside_it_and_filled_by_its_list", async () => {
  const popup = await browser.newPage({ viewport: { width: 900, height: 700 } });
  try {
    await popup.setContent(LIST_BUTTONS);
    const read = await readFields(popup);
    const field = (handle) => read.fields.find((one) => one.handle === handle);
    assert(["#branch", "#doctor", "#prefix"].every((handle) => field(handle)?.kind === "combobox"),
      "each button that opens a list is a field", read.fields);
    assert(JSON.stringify(["#branch", "#doctor", "#prefix"].map((handle) => field(handle).label)) === JSON.stringify(["Branch", "Doctor", "Phone prefix"]),
      "named by the words beside it — not by the value it shows, nor by a name that holds itself", read.fields.map((one) => one.label));
    assert(JSON.stringify(["#branch", "#doctor", "#prefix"].map((handle) => field(handle).value)) === JSON.stringify(["Select a branch", "Dr. Seo", "US +1"]),
      "holding what it shows", read.fields.map((one) => one.value));
    assert(["#branch", "#doctor", "#prefix"].every((handle) => field(handle).open === false), "saying its list is shut", read.fields);
    assert(JSON.stringify(field("#branch").options) === JSON.stringify(["Hanbit", "Seoul"])
      && JSON.stringify(field("#doctor").options) === JSON.stringify(["Dr. Seo", "Dr. Min"])
      && JSON.stringify(field("#prefix").options) === JSON.stringify(["US +1", "KR +82"]),
      "offering the items of the list it controls or the one beside it, though neither is drawn", read.fields);
    assert(JSON.stringify(read.actions.map((action) => action.label)) === JSON.stringify(["More", "Go"]),
      "a button that opens a menu is still a button, and a list button is no button beside its field", read.actions);
    assert((read.unknowns || []).length === 0, "and none is said a second time as a thing of no kind", read.unknowns);
    const filled = await fillBundle(popup, { "#branch": "Seoul", "#prefix": "KR +82" });
    assert(filled.results.every((result) => result.status === "set"), "the items are chosen by their words", filled.results);
    const taken = await popup.evaluate(() => ({ branch: document.getElementById("branch").textContent,
      value: document.getElementById("branch").dataset.value, prefix: document.getElementById("prefix").textContent }));
    assert(JSON.stringify(taken) === JSON.stringify({ branch: "Seoul", value: "sl", prefix: "KR +82" }), "the page took the items", taken);
    await popup.click("#doctor");
    const opened = (await readFields(popup)).fields.find((one) => one.handle === "#doctor");
    assert(opened.open === true, "a list that is open says so", opened);
    return `${read.fields.length} fields`;
  } finally { await popup.close(); }
});

/* A group of radios is named by its title — the name ARIA gives it, the legend of
 * its fieldset, else the heading just above it — and the sentence between that
 * title and the group is a note about it (`hint`), not its name. A caption in a
 * box of its own, under a heading that stands over other fields too, is still the
 * group's name. */
await test("a_group_is_named_by_its_title_and_the_sentence_between_title_and_group_is_a_note", async () => {
  const groups = await browser.newPage({ viewport: { width: 900, height: 900 } });
  try {
    await groups.setContent(`<!doctype html><html lang="en"><meta charset="utf-8"><form>
      <section><h2>Choose a pass</h2><p>Early prices until May.</p>
        <div role="radiogroup" aria-label="Choose a pass"><label><input type="radio" name="pass" value="a"> Standard</label>
          <label><input type="radio" name="pass" value="b"> Premium</label></div></section>
      <section><h2>Seating</h2><p>Seats go fast.</p>
        <div><label><input type="radio" name="seat" value="w"> Window</label><label><input type="radio" name="seat" value="a"> Aisle</label></div></section>
      <fieldset><legend>Meal</legend><p>Pick one.</p>
        <label><input type="radio" name="meal" value="v"> Veg</label><label><input type="radio" name="meal" value="m"> Meat</label></fieldset>
      <section><h2>Order</h2><label>Name <input id="who"></label>
        <div class="row"><span>Size</span><div><label><input type="radio" name="size" value="s"> Small</label><label><input type="radio" name="size" value="l"> Large</label></div></div></section>
    </form>`);
    const read = await readFields(groups);
    const group = (first) => read.fields.find((one) => one.kind === "radio" && one.options[0] === first);
    const pass = group("Standard"), seat = group("Window"), meal = group("Veg"), size = group("Small");
    assert(pass.label === "Choose a pass" && pass.hint === "Early prices until May.",
      "a group named by ARIA keeps its name and the sentence under the heading is its note", pass);
    assert(seat.label === "Seating" && seat.hint === "Seats go fast.",
      "a group with no name of its own is named by the heading above it, the sentence between them its note", seat);
    assert(meal.label === "Meal" && meal.hint === "Pick one.", "a legend is the title, the sentence after it a note", meal);
    assert(size.label === "Size" && !size.hint, "a caption in a box of its own is the name, though a heading stands over other fields above it", size);
    return [pass, seat, meal, size].map((one) => one.label).join(" · ");
  } finally { await groups.close(); }
});

/* A field whose value the door never reads — a password — is not "required and
 * empty": the fill's left-over list says nothing of it. */
await test("a_secret_field_is_not_counted_as_required_and_empty_after_a_fill", async () => {
  const secret = await browser.newPage({ viewport: { width: 900, height: 600 } });
  try {
    await secret.setContent(`<!doctype html><html lang="en"><meta charset="utf-8"><form>
      <label>Email <input id="mail" required></label><label>Password <input id="pw" type="password" required></label>
      <label>Nickname <input id="nick" required></label></form>`);
    const read = await readFields(secret);
    assert(read.fields.find((one) => one.handle === "#pw").masked, "the password is masked", read.fields);
    const filled = await fillBundle(secret, { "#mail": "kim@example.com" });
    assert(filled.results[0].status === "set", "the email took", filled.results);
    assert(JSON.stringify(filled.left.map((one) => one.handle)) === JSON.stringify(["#nick"]),
      "what is left is the field the door can see empty — not the one whose value it never reads", filled.left);
    return filled.left.map((one) => one.handle).join(",");
  } finally { await secret.close(); }
});

/* Fields of one read that go by the same words are told apart by the title of the
 * row, list item or card each stands in — `<title>: <words>` — else numbered, and
 * a number says it is only an order. Two fields of one name under
 * different headings are told by those headings too — a name is what a script reads a
 * field by. A fill says the name the read gave. */
await test("fields_of_one_name_are_told_apart_by_the_title_of_the_item_each_stands_in_else_numbered", async () => {
  const same = await browser.newPage({ viewport: { width: 900, height: 1200 } });
  try {
    await same.setContent(`<!doctype html><html lang="en"><meta charset="utf-8">
      <form><table><tr><th scope="row">Tomato 500g</th><td>3,000</td><td><label>Qty <input id="q1"></label></td></tr>
        <tr><th scope="row">Spinach 1 bunch</th><td>2,000</td><td><label>Qty <input id="q2"></label></td></tr></table>
      <ul><li><h3>Green tea</h3><label>Amount <input id="a1"></label></li><li><h3>Black tea</h3><label>Amount <input id="a2"></label></li></ul>
      <div class="card"><strong>Room A</strong><p>Sea view</p><label>Guests <select id="g1"><option>1</option><option>2</option></select></label></div>
      <div class="card"><strong>Room B</strong><p>Garden</p><label>Guests <select id="g2"><option>1</option><option>2</option></select></label></div>
      <div><label>Note <input id="n1"></label></div><div><label>Note <input id="n2"></label></div>
      <section><h2>Guest</h2><label>Surname <input id="s1"></label></section><section><h2>Host</h2><label>Surname <input id="s2"></label></section></form>`);
    const read = await readFields(same);
    const label = (handle) => read.fields.find((one) => one.handle === handle).label;
    assert(label("#q1") === "Tomato 500g: Qty" && label("#q2") === "Spinach 1 bunch: Qty", "rows are told apart by their header", read.fields.map((one) => one.label));
    assert(label("#a1") === "Green tea: Amount" && label("#a2") === "Black tea: Amount", "list items by their heading", read.fields.map((one) => one.label));
    assert(label("#g1") === "Room A: Guests" && label("#g2") === "Room B: Guests", "cards by their first words", read.fields.map((one) => one.label));
    assert(label("#n1") === "Note #1" && label("#n2") === "Note #2", "with nothing to tell them by, by number", read.fields.map((one) => one.label));
    assert(read.fields.find((one) => one.handle === "#n2").ordinal === true && !read.fields.find((one) => one.handle === "#q1").ordinal,
      "and the number says it is only an order", read.fields);
    assert(label("#s1") === "Guest: Surname" && label("#s2") === "Host: Surname",
      "fields of one name under different headings are told by them too — what is read by name is the name", read.fields.map((one) => one.label));
    const filled = await fillBundle(same, { "#q1": "2", "#n2": "later" });
    assert(filled.results[0].label === "Tomato 500g: Qty" && filled.results[1].label === "Note #2", "a fill says the name the read gave", filled.results);
    return `${read.fields.length} fields`;
  } finally { await same.close(); }
});

/* What a write or a press brings (t-41592): the text that stands new in the box around a field
 * — a check's verdict written beside it and tied to it by nothing — is said beside that field as
 * the page wrote it, never as an error; a field the page marks invalid and says nothing of is told
 * so; the notice a live region made is said beside no field; words the page tied to a field are its
 * error and are not said again as new. A press says the same, in a form its agent read. */
const BRINGS = `<!doctype html><html lang="en"><meta charset="utf-8"><form>
  <div class="row"><label for="mail">Email</label> <input id="mail"><span class="hint">We never share it.</span></div>
  <div class="row"><label for="age">Age</label> <input id="age" aria-invalid="false"></div>
  <div class="row"><label for="code">Code</label> <input id="code"></div>
  <div class="row"><label for="tip">Tip</label> <input id="tip" aria-describedby="tip-why"><span id="tip-why"></span></div>
  <div id="toast" role="status"></div>
</form>
<script>
  const mail = document.getElementById("mail");
  mail.addEventListener("change", () => {
    const old = mail.parentElement.querySelector(".err");
    if (old) old.remove();
    if (!mail.value.includes("@")) {
      const err = document.createElement("span"); err.className = "err"; err.textContent = "Enter a valid email";
      mail.parentElement.append(err);
    }
  });
  document.getElementById("age").addEventListener("change", (event) => {
    event.target.setAttribute("aria-invalid", String(Number(event.target.value) < 18));
  });
  document.getElementById("code").addEventListener("change", () => { document.getElementById("toast").textContent = "Code sent again"; });
  document.getElementById("tip").addEventListener("change", (event) => {
    const bad = Number(event.target.value) < 0;
    event.target.setAttribute("aria-invalid", String(bad));
    document.getElementById("tip-why").textContent = bad ? "A tip cannot be negative" : "";
  });
</script>`;
await test("a_fill_says_the_text_that_appeared_beside_a_field_it_wrote_and_a_notice_the_page_made", async () => {
  const brings = await browser.newPage({ viewport: { width: 900, height: 700 } });
  try {
    await brings.setContent(BRINGS);
    const before = await readFields(brings);
    const filled = await fillBundle(brings, { "#mail": "kim", "#age": "12", "#code": "1", "#tip": "-5" }, before.fingerprint);
    const result = (handle) => filled.results.find((one) => one.handle === handle);
    assert(JSON.stringify(result("#mail").fresh) === JSON.stringify(["Enter a valid email"]),
      "the text that appeared beside the field is said — not the sentence that was always there", result("#mail"));
    assert(result("#age").silent === true && result("#age").error === "aria-invalid" && !result("#age").fresh,
      "a field marked invalid with nothing said about why is told so", result("#age"));
    assert(!result("#code").fresh && !result("#code").silent, "a field with nothing new says nothing", result("#code"));
    assert(JSON.stringify(filled.alerts) === JSON.stringify(["Code sent again"]),
      "the notice a live region made is said, beside no field", filled.alerts);
    assert(result("#tip").error === "A tip cannot be negative" && !result("#tip").fresh && !result("#tip").silent,
      "words the page tied to the field are its error, not said a second time as new text", result("#tip"));
    assert(filled.left.some((one) => one.handle === "#age" && one.silent === true), "and a field that is left carries it too", filled.left);
    const again = await fillBundle(brings, { "#mail": "kim@example.com" });
    assert(!again.results[0].fresh && JSON.stringify(again.alerts) === JSON.stringify([]),
      "a value the page takes brings nothing — and what was said is not new twice", again);
    return JSON.stringify(result("#mail").fresh);
  } finally { await brings.close(); }
});

const PRESSES = `<!doctype html><html lang="en"><meta charset="utf-8"><form>
  <div class="row"><label for="mail">Email</label> <input id="mail"></div>
  <div class="row"><label for="phone">Phone</label> <input id="phone"></div>
  <button type="button" id="next">Next</button>
  <button type="button" id="idle">Nothing</button>
  <div id="step2" hidden><div class="row"><label for="city">City</label> <input id="city"></div></div>
</form>
<script>
  const mail = document.getElementById("mail"), phone = document.getElementById("phone");
  document.getElementById("next").addEventListener("click", () => {
    const old = mail.parentElement.querySelector(".err");
    if (old) old.remove();
    phone.setAttribute("aria-invalid", String(phone.value === ""));
    if (!mail.value.includes("@")) {
      const err = document.createElement("span"); err.className = "err"; err.textContent = "Enter a valid email";
      mail.parentElement.append(err);
      return;
    }
    document.getElementById("step2").hidden = false;
  });
</script>`;
await test("a_press_in_a_known_form_says_what_appeared_beside_its_field_and_a_press_that_moves_on_says_so", async () => {
  assert(typeof twin.pressInForm === "function", "the stand-in presses as the window does in a form that was read");
  const form = await browser.newPage({ viewport: { width: 900, height: 700 } });
  try {
    await form.setContent(PRESSES);
    const run = (source) => evalJson(form, source);
    const read = await readFields(form);
    const stayed = await twin.pressInForm(run, "#next");
    assert(stayed.read.fingerprint === read.fingerprint, "the form is the one that was read", stayed.read);
    const noted = Object.fromEntries(stayed.read.noted.map((one) => [one.handle, one]));
    assert(JSON.stringify(noted["#mail"]?.fresh) === JSON.stringify(["Enter a valid email"]),
      "what appeared beside the field is the reason the press did not move on", stayed.read.noted);
    assert(noted["#phone"]?.silent === true && noted["#phone"].error === "aria-invalid",
      "a field the page marked invalid and said nothing about is told so", stayed.read.noted);
    assert(stayed.settle.state === "ready" && stayed.moving === false, "the page was waited for", stayed.settle);
    const idle = await twin.pressInForm(run, "#idle");
    assert(idle.read.fingerprint === read.fingerprint && !idle.read.noted.some((one) => one.handle === "#mail")
      && JSON.stringify(idle.read.alerts) === JSON.stringify([]),
      "a press that changes nothing brings nothing new — the sentence said before is not new again", idle.read);
    await fillBundle(form, { "#mail": "kim@example.com" });
    const moved = await twin.pressInForm(run, "#next");
    assert(moved.read.fingerprint !== read.fingerprint, "a press that moves the form on brings a form that is not the one read", moved.read);
    return `${stayed.read.noted.length} noted`;
  } finally { await form.close(); }
});

/* An item is titled by its name — the words a person would call it by — not by a short label that
 * stands before it (a thumbnail's), nor by a price, a count or a total, which have letters in them
 * only as units: fields of one name in rows of a cart are told apart by the name of the product. */
await test("an_item_is_titled_by_its_name_not_by_a_short_label_before_it_or_a_price_or_total_around_it", async () => {
  const cart = await browser.newPage({ viewport: { width: 900, height: 900 } });
  try {
    const row = (name, desc, list, price) => `<li><input type="checkbox" aria-label="${name} select">
      <div class="thumb">${name.split(" ")[0]}</div>
      <div class="info"><p class="n">${name}</p><p class="d">${desc}</p><p class="price"><s>${list} won</s> ${price} won</p></div>
      <div class="step"><button type="button" aria-label="Less">-</button><input aria-label="Qty" value="1"><button type="button" aria-label="More">+</button></div>
      <strong class="total">${price} won</strong></li>`;
    await cart.setContent(`<!doctype html><html lang="en"><meta charset="utf-8"><form><ul>
      ${row("Green tea tin 100g", "Loose leaf · chilled", "7,900", "6,900")}
      ${row("Black tea tin 100g", "Smoked · room", "5,000", "4,200")}
      ${row("Mint 1 bunch", "Fresh · chilled", "0", "2,800")}</ul></form>`);
    const read = await readFields(cart);
    const quantities = read.fields.filter((field) => field.kind === "text").map((field) => field.label);
    assert(JSON.stringify(quantities) === JSON.stringify(["Green tea tin 100g: Qty", "Black tea tin 100g: Qty", "Mint 1 bunch: Qty"]),
      "each row's quantity is told by the product's name — not by its thumbnail's word, its price or its total", quantities);
    return quantities.join(" · ");
  } finally { await cart.close(); }
});

/* A button that opens a list is named by what labels it from outside: a label that names it by
 * `aria-labelledby` together with the element inside it that holds the value it shows is the
 * label's words alone — the value element is part of the button, not of its name. */
await test("a_list_button_is_named_by_the_words_beside_it_not_by_the_value_element_inside_it", async () => {
  const inner = await browser.newPage({ viewport: { width: 900, height: 600 } });
  try {
    await inner.setContent(`<!doctype html><html lang="en"><meta charset="utf-8"><form>
      <span class="glabel" id="t-l">Session track <b>*</b></span>
      <div class="dd"><button type="button" aria-haspopup="listbox" aria-expanded="false" aria-labelledby="t-l t-v" id="track">
        <span id="t-v">Choose a session…</span><span class="caret"></span></button>
        <ul role="listbox" hidden><li role="option">Lab</li><li role="option">Experiments</li></ul></div></form>`);
    const field = (await readFields(inner)).fields.find((one) => one.handle === "#track");
    assert(field && field.label === "Session track *", "the label's words, not the words of the value inside the button", field);
    assert(field.value === "Choose a session…", "and the value is what the button shows", field);
    return field.label;
  } finally { await inner.close(); }
});

/* Fields of one name in groups the page titles — a fieldset with its legend, as a party of two
 * is entered — are told by the title: `<legend>: <name>`, as a person says them. */
await test("fields_of_one_name_in_fieldsets_under_different_legends_are_told_by_the_legend", async () => {
  const party = await browser.newPage({ viewport: { width: 900, height: 900 } });
  try {
    await party.setContent(`<!doctype html><html lang="en"><meta charset="utf-8"><form>
      <fieldset><legend>Attendee 1 <span class="badge">Booking contact</span></legend>
        <label>First name <b>*</b> <input id="f1"></label><label>Last name <b>*</b> <input id="l1"></label></fieldset>
      <fieldset><legend>Attendee 2</legend>
        <label>First name <b>*</b> <input id="f2"></label><label>Last name <b>*</b> <input id="l2"></label></fieldset></form>`);
    const labels = Object.fromEntries((await readFields(party)).fields.map((one) => [one.handle, one.label]));
    assert(labels["#f1"] === "Attendee 1: First name *" && labels["#f2"] === "Attendee 2: First name *"
      && labels["#l1"] === "Attendee 1: Last name *" && labels["#l2"] === "Attendee 2: Last name *",
      "each is told by the legend of the fieldset it stands in", labels);
    return JSON.stringify(labels);
  } finally { await party.close(); }
});

/* A note is the sentence just under the title — the text blocks that follow the heading until the first
 * thing that is no mere text (a control, a button, a label, another heading, the group itself) — not every
 * word the page has between the title and the group. */
await test("a_note_is_the_sentence_just_under_the_title_not_every_word_between_title_and_group", async () => {
  const under = await browser.newPage({ viewport: { width: 900, height: 700 } });
  try {
    await under.setContent(`<!doctype html><html lang="en"><meta charset="utf-8"><form><section><h2>Schedule</h2>
      <p>Two nights at least.</p>
      <div class="row"><span class="cap">Pick-up</span> <button type="button">Choose a time</button></div>
      <div role="radiogroup" aria-label="Morning or evening"><label><input type="radio" name="ampm" value="a"> Morning</label>
        <label><input type="radio" name="ampm" value="p"> Evening</label></div></section></form>`);
    const group = (await readFields(under)).fields.find((one) => one.kind === "radio");
    assert(group.label === "Morning or evening" && group.hint === "Two nights at least.",
      "the sentence under the title is the note — not the caption of a button that stands between", group);
    return group.hint;
  } finally { await under.close(); }
});

/* What stands new beside a field is words: a counter that counts, a countdown that runs, and the
 * value the page shows of the field itself — in the control or beside it — are the field's own
 * number or value changing, not text the page brought; a sentence the page brought is still said. */
await test("a_counter_a_countdown_or_the_value_shown_beside_a_field_is_no_new_text", async () => {
  const live = await browser.newPage({ viewport: { width: 900, height: 700 } });
  try {
    await live.setContent(`<!doctype html><html lang="en"><meta charset="utf-8"><form>
      <div class="row"><label for="note">Note</label> <textarea id="note"></textarea><span id="count">0/100</span></div>
      <div class="row"><label for="code">Code</label> <input id="code"><span id="left">Valid for 03:00</span></div>
      <div class="row"><label for="nick">Nickname</label> <input id="nick"><span id="echo"></span></div>
      <div class="row"><label for="mail">Email</label> <input id="mail"></div>
    </form>
    <script>
      const $ = (id) => document.getElementById(id);
      $("note").addEventListener("change", (event) => { $("count").textContent = event.target.value.length + "/100"; });
      $("code").addEventListener("change", () => { $("left").textContent = "Valid for 02:59"; });
      $("nick").addEventListener("change", (event) => { $("echo").textContent = event.target.value; });
      $("mail").addEventListener("change", () => {
        const saved = document.createElement("span"); saved.textContent = "Saved to your profile"; $("mail").parentElement.append(saved);
      });
    </script>`);
    const before = await readFields(live);
    const filled = await fillBundle(live, { "#note": "hello there", "#code": "123456", "#nick": "Kim", "#mail": "kim@example.com" }, before.fingerprint);
    const result = (handle) => filled.results.find((one) => one.handle === handle);
    assert(!result("#note").fresh, "a counter that counts is no new text", result("#note"));
    assert(!result("#code").fresh, "a countdown that runs is no new text — its words are the same, its number is not", result("#code"));
    assert(!result("#nick").fresh, "the value shown beside the field is the field's own", result("#nick"));
    assert(JSON.stringify(result("#mail").fresh) === JSON.stringify(["Saved to your profile"]), "a sentence the page brought is still said", result("#mail"));
    return JSON.stringify(result("#mail").fresh);
  } finally { await live.close(); }
});

await test("the_item_a_dropdown_shows_in_its_own_box_is_no_new_text_beside_it", async () => {
  const shown = await browser.newPage({ viewport: { width: 900, height: 700 } });
  try {
    await shown.setContent(`<!doctype html><html lang="ko"><meta charset="utf-8"><form>
      <div class="row"><div class="cap">주차장</div><div class="ctl"><div class="dd">
        <div class="btn" tabindex="0"><span class="val">주차장을 고르세요</span></div>
        <ul class="list" hidden><li data-value="p0">P3 단기</li><li data-value="p1">P4 장기</li></ul></div></div></div></form>
      <script>
        const box = document.querySelector(".dd"), button = box.querySelector(".btn"), list = box.querySelector(".list");
        button.addEventListener("click", () => { list.hidden = !list.hidden; });
        list.addEventListener("click", (event) => {
          const item = event.target.closest("li");
          if (!item) return;
          button.querySelector(".val").textContent = item.textContent;
          list.hidden = true;
        });
      </script>`);
    const read = await readFields(shown);
    const lot = read.fields.find((field) => field.label === "주차장");
    assert(lot && lot.kind === "dropdown", "a focusable box with a list beside it is a dropdown", read.fields);
    const filled = await fillBundle(shown, { [lot.handle]: "P4 장기" }, read.fingerprint);
    assert(filled.results[0].status === "set", "the item is chosen by its words", filled.results);
    assert(!filled.results[0].fresh, "the item the box shows now is the field's own value, not text the page brought", filled.results[0]);
    return "own value";
  } finally { await shown.close(); }
});

/* A field marked invalid is told "the page does not say why" only when nothing but its own words
 * stands in its box: a sentence that stood beside it all along may be the reason. */
await test("a_field_marked_invalid_is_told_silent_only_when_nothing_but_its_own_words_stands_beside_it", async () => {
  const marks = await browser.newPage({ viewport: { width: 900, height: 700 } });
  try {
    await marks.setContent(`<!doctype html><html lang="en"><meta charset="utf-8"><form>
      <div class="row"><label for="a">Alpha</label> <input id="a"></div>
      <div class="row"><label for="b">Beta</label> <input id="b"><span class="msg">Use letters only</span></div>
    </form>
    <script>
      for (const id of ["a", "b"]) document.getElementById(id).addEventListener("change", (event) => event.target.setAttribute("aria-invalid", "true"));
    </script>`);
    const before = await readFields(marks);
    const filled = await fillBundle(marks, { "#a": "1", "#b": "2" }, before.fingerprint);
    const result = (handle) => filled.results.find((one) => one.handle === handle);
    assert(result("#a").silent === true && result("#a").error === "aria-invalid", "nothing but its own words beside it: the page says nothing of why", result("#a"));
    assert(result("#b").error === "aria-invalid" && !result("#b").silent && !result("#b").fresh,
      "a sentence stood beside it all along: it is not told that the page says nothing", result("#b"));
    assert(filled.left.some((one) => one.handle === "#a" && one.silent === true) && !filled.left.some((one) => one.handle === "#b" && one.silent),
      "and the fields that are left carry the same", filled.left);
    return "own words only";
  } finally { await marks.close(); }
});

/* Text that stands new in the form beside no single field is the page's too: the reason a check
 * writes under a value made of several fields (an address and its domain), a sentence under a group of
 * buttons that are no fields. It is said apart from the fields — once, never what a field's own box
 * already says, never a counter, never what the page's live regions said — after a fill and after a press. */
const ASIDE = `<!doctype html><html lang="en"><meta charset="utf-8"><form>
  <dl><div class="row"><dt>Email</dt><dd>
    <input id="e-id" aria-label="Email name"><span class="sep">@</span><input id="e-dom" aria-label="Email domain">
    <select id="e-sel" aria-label="Email provider"><option value="">own</option><option value="example.com">example.com</option></select>
    <div id="mailErr"></div></dd></div></dl>
  <div role="group" aria-label="Diet"><button type="button" aria-pressed="false">Vegan</button><button type="button" aria-pressed="false">Halal</button></div>
  <div id="dietErr"></div>
  <div class="row"><label for="city">City</label> <input id="city"><span id="cityErr"></span></div>
  <div class="row"><label for="plate">Plate</label> <input id="plate"><span id="count">0/10</span></div>
  <button type="button" id="go">Go</button>
  <div id="banner" role="alert"></div>
</form>
<script>
  (() => {
    const $ = (id) => document.getElementById(id);
    $("plate").addEventListener("change", (event) => {
      $("count").textContent = event.target.value.length + "/10";
      $("dietErr").textContent = "Choose a diet";
    });
    $("go").addEventListener("click", () => {
      $("mailErr").textContent = "Check the email address";
      $("dietErr").textContent = "Choose a diet";
      $("cityErr").textContent = "Pick a city";
      $("banner").textContent = "3 items need a fix";
    });
  })();
</script>`;
await test("text_that_appears_beside_no_single_field_is_said_apart_once_and_never_as_a_counter_or_a_fields_own_text", async () => {
  assert(typeof twin.pressInForm === "function", "the stand-in presses as the window does in a form that was read");
  const aside = await browser.newPage({ viewport: { width: 900, height: 700 } });
  try {
    await aside.setContent(ASIDE);
    const before = await readFields(aside);
    assert(before.outside === undefined, "a plain read says nothing of what is new", Object.keys(before));
    const filled = await fillBundle(aside, { "#plate": "abcdefg" }, before.fingerprint);
    assert(JSON.stringify(filled.outside) === JSON.stringify(["Choose a diet"]),
      "the sentence that appeared under the buttons is said — not the counter that counted", filled.outside);
    const again = await fillBundle(aside, { "#plate": "abcdefgh" });
    assert(JSON.stringify(again.outside) === JSON.stringify([]), "and what was said is not new twice", again.outside);
    await aside.setContent(ASIDE);
    const read = await readFields(aside);
    const run = (source) => evalJson(aside, source);
    const pressed = await twin.pressInForm(run, "#go");
    assert(pressed.read.fingerprint === read.fingerprint, "the form is the one that was read", pressed.read);
    assert(JSON.stringify(pressed.read.outside) === JSON.stringify(["Check the email address", "Choose a diet"]),
      "the reasons under the value made of several fields and under the buttons are said, in the page's order", pressed.read.outside);
    assert(JSON.stringify(pressed.read.alerts) === JSON.stringify(["3 items need a fix"]),
      "what the live region said is the notice — not said a second time as text beside no field", pressed.read.alerts);
    const noted = Object.fromEntries(pressed.read.noted.map((one) => [one.handle, one]));
    assert(JSON.stringify(noted["#city"]?.fresh) === JSON.stringify(["Pick a city"]),
      "text in a field's own box stays with the field", pressed.read.noted);
    const idle = await twin.pressInForm(run, "#go");
    assert(JSON.stringify(idle.read.outside) === JSON.stringify([]), "a press that brings nothing new says none", idle.read.outside);
    return JSON.stringify(pressed.read.outside);
  } finally { await aside.close(); }
});

/* A row of buttons that say whether they are pressed (`aria-pressed`) is one field (t-41656) — chips: its
 * title the name (ARIA's name for the row, its fieldset's legend, the words before it), its options the
 * buttons' words, its value the ones pressed — and is no longer said again as buttons. A toggle that stands in a
 * box with a field of its own (a "show" beside a password) is that field's, and stays a button. */
const CHIPS = `<!doctype html><html lang="en"><meta charset="utf-8"><form>
  <div id="coloursTitle">Pick colours</div>
  <div id="colours" role="group" aria-labelledby="coloursTitle">
    <button type="button" aria-pressed="false">Red</button>
    <button type="button" aria-pressed="true">Green</button>
    <button type="button" aria-pressed="false">Blue</button>
  </div>
  <fieldset><legend>Size</legend>
    <div id="size"><button type="button" aria-pressed="false">Small</button><button type="button" aria-pressed="true">Medium</button><button type="button" aria-pressed="false">Large</button></div>
  </fieldset>
  <div class="row"><label for="pw">Password</label>
    <span class="wrap"><input id="pw" type="password"><button type="button" id="show" aria-pressed="false">Show</button></span></div>
  <button type="button" id="go">Go</button>
</form>
<script>
  (() => {
    window.__presses = [];
    const pressed = (button) => button.getAttribute("aria-pressed") === "true";
    // One row lets several be pressed; the other is one choice of several, which lets go of the rest.
    document.getElementById("colours").addEventListener("click", (event) => {
      const button = event.target.closest("button");
      if (!button) return;
      window.__presses.push(button.textContent);
      button.setAttribute("aria-pressed", String(!pressed(button)));
    });
    document.getElementById("size").addEventListener("click", (event) => {
      const button = event.target.closest("button");
      if (!button) return;
      window.__presses.push(button.textContent);
      for (const other of document.querySelectorAll("#size button")) other.setAttribute("aria-pressed", String(other === button));
    });
  })();
</script>`;
const heldChips = (target, row) => target.evaluate((selector) =>
  [...document.querySelectorAll(selector + " button")].filter((button) => button.getAttribute("aria-pressed") === "true").map((button) => button.textContent), row);
const pressesOf = (target) => target.evaluate(() => window.__presses.slice());

await test("a_row_of_buttons_that_say_they_are_pressed_is_one_field_named_by_its_title_with_the_buttons_as_its_options", async () => {
  const chips = await browser.newPage({ viewport: { width: 900, height: 700 } });
  try {
    await chips.setContent(CHIPS);
    const read = await readFields(chips);
    const likes = read.fields.find((one) => one.handle === "#colours");
    const diet = read.fields.find((one) => one.handle === "#size");
    assert(likes && likes.kind === "chips", "a row of buttons that say whether they are pressed is one field of kind chips", read.fields);
    assert(likes.label === "Pick colours" && JSON.stringify(likes.options) === JSON.stringify(["Red", "Green", "Blue"])
      && JSON.stringify(likes.value) === JSON.stringify(["Green"]),
    "named by the title ARIA gives the row, its options the buttons' words, its value the buttons pressed", likes);
    assert(diet && diet.kind === "chips" && diet.label === "Size" && JSON.stringify(diet.value) === JSON.stringify(["Medium"]),
      "a row in a fieldset is named by the legend", diet);
    const buttons = read.actions.map((one) => one.label);
    assert(!buttons.some((label) => ["Red", "Green", "Blue", "Small", "Medium", "Large"].includes(label)),
      "the buttons of a row are not said again as buttons", buttons);
    assert(buttons.includes("Show") && buttons.includes("Go") && !read.fields.some((one) => one.kind === "chips" && one.handle !== "#colours" && one.handle !== "#size"),
      "a toggle beside a field of its own stays a button", { buttons, fields: read.fields.map((one) => one.handle) });
    await chips.setContent(`<!doctype html><html lang="en"><meta charset="utf-8"><form>
      <div id="yn"><button type="button" aria-pressed="false">Yes</button><button type="button" aria-pressed="false">No</button></div>
      <ul id="wrapped"><li><button type="button" aria-pressed="false">Alpha</button></li><li><button type="button" aria-pressed="true">Beta</button></li><li><button type="button" aria-pressed="false">Gamma</button></li></ul></form>`);
    const bare = await readFields(chips);
    const yn = bare.fields.find((one) => one.handle === "#yn");
    const wrapped = bare.fields.find((one) => one.handle === "#wrapped");
    assert(yn && yn.kind === "chips" && yn.label === "" && JSON.stringify(yn.options) === JSON.stringify(["Yes", "No"]),
      "a row the page gives no title is still one field, with the name empty for the words to say so", bare.fields);
    assert(wrapped && wrapped.kind === "chips" && JSON.stringify(wrapped.options) === JSON.stringify(["Alpha", "Beta", "Gamma"])
      && JSON.stringify(wrapped.value) === JSON.stringify(["Beta"]), "buttons each in a box of their own are one row", bare.fields);
    return [likes, diet, yn].map((one) => one.label || "(no title)").join(" · ");
  } finally { await chips.close(); }
});

await test("a_row_of_chips_is_written_by_pressing_only_the_buttons_whose_state_differs_as_the_page_behaves", async () => {
  const chips = await browser.newPage({ viewport: { width: 900, height: 700 } });
  try {
    await chips.setContent(CHIPS);
    const read = await readFields(chips);
    const set = await fillBundle(chips, { "#colours": ["Red", "Blue"] }, read.fingerprint);
    assert(set.results[0]?.status === "set" && JSON.stringify(set.results[0].now) === JSON.stringify(["Red", "Blue"]),
      "a list of the options wanted is written and read back as the list held", set.results);
    assert(JSON.stringify(await heldChips(chips, "#colours")) === JSON.stringify(["Red", "Blue"]), "the page holds what was asked", await heldChips(chips, "#colours"));
    assert(JSON.stringify((await pressesOf(chips)).sort()) === JSON.stringify(["Blue", "Green", "Red"]),
      "each button that differed was pressed once and no other", await pressesOf(chips));
    const same = await fillBundle(chips, { "#colours": ["Red", "Blue"] });
    assert(same.results[0]?.status === "same" && (await pressesOf(chips)).length === 3, "a row that already holds it is pressed no more", same.results);
    const single = await fillBundle(chips, { "#size": ["Small"] });
    const afterDiet = (await pressesOf(chips)).slice(3);
    assert(single.results[0]?.status === "set" && JSON.stringify(await heldChips(chips, "#size")) === JSON.stringify(["Small"]),
      "one choice of several: the page lets go of the other itself", single.results);
    assert(JSON.stringify(afterDiet) === JSON.stringify(["Small"]), "so the button the page had let go of is not pressed again", afterDiet);
    const words = await fillBundle(chips, { "#colours": "Green, Blue" });
    assert(words.results[0]?.status === "set" && JSON.stringify(await heldChips(chips, "#colours")) === JSON.stringify(["Green", "Blue"]),
      "the same options said as one text", words.results);
    const before = (await pressesOf(chips)).length;
    const nothing = await fillBundle(chips, { "#colours": ["Nonexistent"] });
    assert(nothing.results[0]?.status === "no_option" && JSON.stringify(nothing.results[0].options) === JSON.stringify(["Red", "Green", "Blue"])
      && (await pressesOf(chips)).length === before, "an option the row lacks is refused by name with the options, and nothing is pressed", nothing.results);
    return JSON.stringify(await pressesOf(chips));
  } finally { await chips.close(); }
});

/* A value split across several fields is told by the caption of the group the parts stand in (t-41656): a part
 * the page names only by its own short words ("Hour") takes the group's caption in front of it
 * (`<caption> — <words>`), when the group holds more than one such part and the words do not already say it. Fields
 * the page names by a label of its own, a part alone in its box and names that already hold the caption are as they were. */
const PARTS = `<!doctype html><html lang="en"><meta charset="utf-8"><form>
  <div id="rtCap">End time <span>*</span></div>
  <div role="group" aria-labelledby="rtCap">
    <div id="ampm" role="radiogroup" aria-label="AM or PM"><button type="button" role="radio" aria-checked="false">AM</button><button type="button" role="radio" aria-checked="false">PM</button></div>
    <select id="rtHour" aria-label="Hour"><option value="">-</option><option value="9">9</option></select>
    <select id="rtMin" aria-label="Minute"><option value="">-</option><option value="00">00</option></select>
  </div>
  <div id="ptCap">Start time</div>
  <div role="group" aria-labelledby="ptCap">
    <select id="ptHour" aria-label="Hour"><option value="">-</option><option value="9">9</option></select>
    <select id="ptMin" aria-label="Minute"><option value="">-</option><option value="00">00</option></select>
  </div>
  <div class="row"><span>Phone</span> <input id="p1" aria-label="Area"> <input id="p2" aria-label="Number"></div>
  <fieldset><legend>Billing</legend><label>Street <input id="street"></label><label>City <input id="city"></label></fieldset>
  <div class="row"><span>Search</span> <input id="q" aria-label="Query"></div>
  <dl><div class="row"><dt>Email</dt><dd><input id="e-id" aria-label="Email name"><span>@</span><input id="e-dom" aria-label="Email domain"></dd></div></dl>
</form>`;
await test("a_part_the_page_names_only_by_its_own_short_words_is_told_by_the_caption_of_its_group_or_row", async () => {
  const parts = await browser.newPage({ viewport: { width: 900, height: 900 } });
  try {
    await parts.setContent(PARTS);
    const read = await readFields(parts);
    const label = (handle) => read.fields.find((one) => one.handle === handle)?.label;
    assert(label("#rtHour") === "End time — Hour *" && label("#rtMin") === "End time — Minute *" && label("#ampm") === "End time — AM or PM *",
      "the parts of a group the page names say its caption, and the star the page put on the caption stays with them", read.fields.map((one) => one.label));
    assert(label("#ptHour") === "Start time — Hour" && label("#ptMin") === "Start time — Minute",
      "so two parts of one name are told apart by their groups, not by a number", read.fields.map((one) => one.label));
    assert(label("#p1") === "Phone — Area" && label("#p2") === "Phone — Number",
      "the parts side by side in one box under a caption say it too", read.fields.map((one) => one.label));
    assert(label("#street") === "Street" && label("#city") === "City" && label("#q") === "Query",
      "a field a label names, and a part alone in its box, are as they were", read.fields.map((one) => one.label));
    assert(label("#e-id") === "Email name" && label("#e-dom") === "Email domain",
      "a name that already holds the caption is not given it twice", read.fields.map((one) => one.label));
    assert(!read.fields.some((one) => one.ordinal), "no field is told by a number", read.fields.filter((one) => one.ordinal));
    return ["#rtHour", "#ptHour", "#p1"].map(label).join(" · ");
  } finally { await parts.close(); }
});

/* A button the page declares a submit — `type="submit"`, on a button or an input — is marked in the
 * buttons the read, a fill and a press say (t-41656); a button with no such type is not. */
await test("a_button_that_declares_it_submits_the_form_is_marked_and_one_that_does_not_is_not", async () => {
  const submits = await browser.newPage({ viewport: { width: 900, height: 600 } });
  try {
    await submits.setContent(`<!doctype html><html lang="en"><meta charset="utf-8"><form onsubmit="return false">
      <label for="a">A</label> <input id="a">
      <button type="submit" id="s1">Save</button><input type="submit" id="s2" value="Send">
      <button type="button" id="b1">Next</button><button id="b2">Maybe</button><button type="reset" id="r">Reset</button></form>`);
    const read = await readFields(submits);
    const mark = (actions) => Object.fromEntries(actions.map((one) => [one.label, one.submit === true]));
    assert(JSON.stringify(mark(read.actions)) === JSON.stringify({ Save: true, Send: true, Next: false, Maybe: false, Reset: false }),
      "only the buttons that declare a submit are marked", read.actions);
    const filled = await fillBundle(submits, { "#a": "x" }, read.fingerprint);
    assert(JSON.stringify(mark(filled.actions)) === JSON.stringify(mark(read.actions)), "a fill's buttons are marked the same", filled.actions);
    return JSON.stringify(mark(read.actions));
  } finally { await submits.close(); }
});

await browser.close();

let failed = 0;
for (const result of results) {
  if (!result.pass) failed += 1;
  const detail = result.detail ? `  — ${result.detail}` : "";
  console.log(`${result.pass ? "PASS" : "FAIL"}  ${result.name}${detail}`);
}
console.log(`\n${results.length - failed}/${results.length} passed`);
endRun(failed ? 1 : 0);
