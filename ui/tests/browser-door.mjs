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
import { FORM_REQUEST, evalFormScript, fieldsScript, fillPasses } from "./browser-scripts.mjs";

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

await browser.close();

let failed = 0;
for (const result of results) {
  if (!result.pass) failed += 1;
  const detail = result.detail ? `  — ${result.detail}` : "";
  console.log(`${result.pass ? "PASS" : "FAIL"}  ${result.name}${detail}`);
}
console.log(`\n${results.length - failed}/${results.length} passed`);
endRun(failed ? 1 : 0);
