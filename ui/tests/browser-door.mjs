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

await browser.close();

let failed = 0;
for (const result of results) {
  if (!result.pass) failed += 1;
  const detail = result.detail ? `  — ${result.detail}` : "";
  console.log(`${result.pass ? "PASS" : "FAIL"}  ${result.name}${detail}`);
}
console.log(`\n${results.length - failed}/${results.length} passed`);
process.exit(failed ? 1 : 0);
