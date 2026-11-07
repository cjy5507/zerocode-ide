/* What the window's scripts call and read must exist (`scripts/window-scope.mjs`).
 *
 * The check is a parser's (acorn) and a scope analyzer's (eslint-scope), so what
 * is tested here is what a person writing the window would meet: a typo is
 * caught where it stands, a parameter is local to its function and no further,
 * and a new piece of the language needs no new rule. The cases the hand-written
 * Rust scanner this replaced was held to are all here, beside the ones it could
 * not read at all — a destructured parameter, a misspelled object, a block's
 * own `let`.
 *
 *   node --test ui/tests/window-scope.test.mjs */
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import * as acorn from "acorn";
import * as eslintScope from "eslint-scope";
import { BROWSER, ECMASCRIPT, PANE_NAMED_NOT_OWNED, VENDORED, checkWindowScope, run, scriptsOf } from "../../scripts/window-scope.mjs";

const root = fileURLToPath(new URL("../..", import.meta.url));
const script = join(root, "scripts", "window-scope.mjs");

const check = (text, options = {}) =>
  checkWindowScope({ scripts: [{ name: "ui/a.js", text }], acorn, eslintScope, ...options });
const found = (text, kind, options) => check(text, options).findings.filter((one) => one.kind === kind).map((one) => one.name);
const calls = (text, options) => found(text, "call", options);

test("a call to a name nobody wrote is caught, and said where it stands", () => {
  const result = check("function setFocused(id) {}\nstageLane(id);\n");
  const [call] = result.findings.filter((one) => one.kind === "call");
  assert.deepEqual(call, { kind: "call", name: "stageLane", file: "ui/a.js", line: 2, column: 1 });
  assert.deepEqual(calls("setFocused(1); function setFocused(id) {}"), []);
});

test("a constructor that nothing declares is a call of it, as a function is", () => {
  assert.deepEqual(calls("const made = new Missing(1);"), ["Missing"]);
  assert.deepEqual(calls("class Made {}\nconst made = new Made(1);"), []);
});

test("a name handed to something else is a read, and a misspelled object is caught — which a scan for call sites never saw", () => {
  assert.deepEqual(found("promise.finally(refreshThreads);", "read").sort(), ["promise", "refreshThreads"]);
  assert.deepEqual(found("agentClok.sync();", "read"), ["agentClok"]);
  assert.deepEqual(found("function refreshThreads() {} const promise = Promise.resolve(); promise.finally(refreshThreads);", "read"), []);
});

test("a parameter is local to its function and no further, for every kind of function", () => {
  assert.deepEqual(calls("new Promise((complete, fail) => { complete(); fail(); stillMissing(); });"), ["stillMissing"]);
  assert.deepEqual(calls("function fold(row, flip) { const top = row.top; flip(); }"), []);
  assert.deepEqual(calls("function fold(row, flip) { flip(); }\nflip();"), ["flip"]);
  assert.deepEqual(calls("const fold = function (row, flip) { flip(); };"), []);
  assert.deepEqual(calls("new Promise((complete, fail) => { setTimeout(() => { fail(); }, 0); });\nfail();"), ["fail"]);
});

test("an expression arrow's parameter is its own through the arrows inside its expression, and the statement's end closes it", () => {
  // shell-boot.js's `listen`: `handler` is the one it was handed (gate l108b).
  const inside =
    "const listen = (name, handler) => listenToBackend(name, (event) => answeringBackendEvent(name, () => handler(event)));";
  assert.deepEqual(calls(inside).sort(), ["answeringBackendEvent", "listenToBackend"]);
  assert.deepEqual(calls(`${inside}\nhandler();`).sort(), ["answeringBackendEvent", "handler", "listenToBackend"]);
  const argument = "items.map((one) => one.name).forEach((name) => paint(name)); one();";
  assert.deepEqual(calls(argument).sort(), ["one", "paint"]);
});

test("a call argument or a default value's call is a use of its name, never a declaration of it", () => {
  assert.deepEqual(calls("perform(complete, missing); missing();").sort(), ["missing", "perform"]);
  assert.deepEqual(calls("const callback = (complete = missing(), failure) => failure();"), ["missing"]);
  assert.ok(!found("perform(complete, missing);", "read").includes("callback"));
});

test("an object's method is a member and not a global, and a bare call of the same name still is", () => {
  assert.deepEqual(calls("const clipboardText = { async write(value) { return value; } };"), []);
  assert.deepEqual(calls("const clipboardText = { async write(value) { return value; } };\nwrite('x');"), ["write"]);
});

test("template text is text and a hole is code; two holes side by side are two expressions", () => {
  assert.deepEqual(calls("const c = `rgb(${level(1)} 0 0)`;"), ["level"]);
  const word = check("const word = `${state} ${t(\"k\", \"v\")}`;");
  assert.deepEqual(word.findings.filter((one) => one.kind === "call").map((one) => one.name), ["t"]);
  assert.deepEqual(word.findings.filter((one) => one.kind === "read").map((one) => one.name), ["state"]);
});

test("a regular expression's words are not calls, and a division is code", () => {
  assert.deepEqual(calls("if (!/^gh auth login(?: --web)?$/.test(command)) genuinely_missing();"), ["genuinely_missing"]);
  assert.deepEqual(calls("const ratio = total / still_missing();"), ["still_missing"]);
});

test("a block's own `let` and a function's own `const` hide nothing outside them, though a scan for declarations took them for the window's", () => {
  assert.deepEqual(calls("{ let inner = () => 1; inner(); }\ninner();"), ["inner"]);
  assert.deepEqual(calls("function a() { const helper = () => 1; helper(); }\nhelper();"), ["helper"]);
  assert.deepEqual(calls("for (const step of []) step();\nstep();"), ["step"]);
});

test("the language the scanner could not read needs no new rule", () => {
  const source = `
    function risky() {}
    const table = {};
    const items = [];
    const useDestructured = ({ pick, nested: [first] }, ...rest) => { pick(); first(); rest.map((one) => one()); };
    const withDefault = (run = () => 1, again = run) => run() + again();
    try { risky(); } catch ({ message }) { message.trim(); }
    try { risky(); } catch (failure) { failure(); }
    class Door { open(latch) { latch(); } static make(factory) { return factory(); } get shut() { return 1; } }
    for (const [key, act] of Object.entries(table)) act(key);
    async function* stream(source) { for await (const chunk of source) chunk(); }
    const maybe = null; maybe?.();
    label: for (const each of items) { each(); continue label; }
    const computed = { [risky.name]() { return 1; } };
  `;
  assert.deepEqual(check(source).findings, []);
});

test("a name declared in any script is seen from every other, whichever loads first", () => {
  const result = checkWindowScope({
    scripts: [
      { name: "ui/early.js", text: "function early() { later(); }\nconst waits = () => settle();" },
      { name: "ui/late.js", text: "function later() {}\nfunction settle() {}" },
    ],
    acorn,
    eslintScope,
  });
  assert.deepEqual(result.findings, []);
});

test("a top-level name declared twice across two scripts is a syntax error, found in the second, as the browser drops it", () => {
  const result = checkWindowScope({
    scripts: [{ name: "ui/a.js", text: "const taken = 1;\n" }, { name: "ui/b.js", text: "\nconst taken = 2;\n" }],
    acorn,
    eslintScope,
  });
  assert.equal(result.findings, undefined);
  assert.equal(result.error.kind, "syntax");
  assert.deepEqual([result.error.file, result.error.line], ["ui/b.js", 2]);
  assert.match(result.error.message, /already been declared/);
});

test("a syntax error names its script and line", () => {
  const result = checkWindowScope({
    scripts: [{ name: "ui/a.js", text: "const fine = 1;\n" }, { name: "ui/b.js", text: "const ok = 1;\nconst two = 2;\nfunction (\n" }],
    acorn,
    eslintScope,
  });
  assert.equal(result.error.kind, "syntax");
  assert.equal(result.error.file, "ui/b.js");
  assert.equal(result.error.line, 3);
});

test("duplicate policy rejects global function and var redeclarations", () => {
  for (const [first, second] of [
    ["function shared() {}", "function shared() {}"],
    ["var shared;", "function shared() {}"],
    ["var shared;", "var shared;"],
  ]) {
    const result = checkWindowScope({ scripts: [{ name: "ui/a.js", text: first }, { name: "ui/b.js", text: second }], acorn, eslintScope });
    const duplicates = result.findings.filter(one => one.kind === "duplicate");
    assert.equal(duplicates.length, 1);
    assert.equal(duplicates[0].name, "shared");
    assert.equal(duplicates[0].file, "ui/b.js");
    assert.equal(duplicates[0].previous.file, "ui/a.js");
  }
});

test("duplicate policy permits local shadowing and rejects same-script global redeclarations", () => {
  assert.deepEqual(check("function shared() {} function local() { var shared; }").findings, []);
  assert.equal(check("var shared; var shared;").findings.filter(one => one.kind === "duplicate").length, 1);
});

test("`typeof name` asks whether a name exists and is not a use of it", () => {
  assert.deepEqual(check("if (typeof maybeThere === \"function\") ok(); function ok() {}").findings, []);
  assert.deepEqual(found("typeof maybeThere.deeper;", "read"), ["maybeThere"]);
});

test("the language, the browser and a vendored bundle are allowed — each in its own kind, and a bundle only while the page loads it", () => {
  assert.deepEqual(check("new Map(); Math.max(1); JSON.parse('{}'); Reflect.has({}, 'a');").findings, []);
  assert.deepEqual(check("document.title; window.name; localStorage.length; new URL('x:');").findings, []);
  assert.deepEqual(check("THREE.Vector3;", { vendored: ["vendor/three.js"] }).findings, []);
  assert.deepEqual(found("THREE.Vector3;", "read"), ["THREE"], "a page that does not load three.js has no THREE");
  assert.deepEqual(found("Notification.permission;", "read"), ["Notification"], "a browser name not listed is a decision");
  const kinds = [ECMASCRIPT, BROWSER, Object.values(VENDORED).flat()];
  const everyName = kinds.flat();
  assert.equal(new Set(everyName).size, everyName.length, "no name is listed under two kinds");
});

test("a collection named like a pane's is declared with the lifecycle, or listed with why it is not a pane's", () => {
  assert.deepEqual(found("const paneFoo = new Map();", "ownership"), ["paneFoo"]);
  assert.deepEqual(found("let paneSeen = new Set();", "ownership"), ["paneSeen"]);
  assert.deepEqual(found("const paneWeak = new WeakMap();", "ownership"), ["paneWeak"]);
  assert.deepEqual(found("const paneFoo = paneLifecycle.map(\"paneFoo\");", "ownership"), [], "declared with the lifecycle");
  assert.deepEqual(found("function f() { const paneBar = new Map(); return paneBar; }", "ownership"), [], "only the window's own");
  assert.deepEqual(found("const typedInto = new Map(); const panes = new Map(); const PaneX = new Map();", "ownership"), [], "a rule about names");
  const listed = check("const paneHosts = new Map();", { notOwned: { paneHosts: "keyed by tab id" } });
  assert.deepEqual(listed.findings.filter((one) => one.kind === "ownership"), []);
  assert.deepEqual(listed.paneNamed, ["paneHosts"], "listed or not, every one is reported to the caller");
});

test("the page's own scripts are read, in its order: a comment is not a script, a vendored bundle is not parsed", () => {
  const markup = `<!doctype html><head>
    <!-- <script defer src="./commented.js"></script> -->
    <script defer src="./vendor/three.js"></script>
    <script defer src="./shell-a.js"></script>
    <script src='./shell-b.js?v=2'></script>
    <script>inline()</script>
    <script type="module" src="./esm.js"></script>
  </head>`;
  assert.deepEqual(scriptsOf(markup), [
    { name: "vendor/three.js", type: "" },
    { name: "shell-a.js", type: "" },
    { name: "shell-b.js", type: "" },
    { name: "esm.js", type: "module" },
  ]);
});

/* A page on disk, for the command. */
async function pageWith(files, extra = {}) {
  const folder = await mkdtemp(join(tmpdir(), "window-scope-"));
  await mkdir(join(folder, "ui", "vendor"), { recursive: true });
  for (const [name, text] of Object.entries(files)) await writeFile(join(folder, "ui", name), text);
  return { folder, ...extra };
}
async function ran(files, { json = false, notOwned = {} } = {}) {
  const { folder } = await pageWith(files);
  const lines = [];
  try {
    const code = await run({ root: folder, json, notOwned, out: (line) => lines.push(line) });
    return { code, lines };
  } finally {
    await rm(folder, { recursive: true, force: true });
  }
}
const html = (...sources) => `<!doctype html><head>${sources.map((src) => `<script defer src="./${src}"></script>`).join("")}</head>`;

test("duplicate policy reads only loaded scripts and fails the CLI for loaded duplicates", async () => {
  const files = { "index.html": html("one.js"), "one.js": "function shared() {}", "unused.js": "function shared() {}" };
  assert.equal((await ran(files)).code, 0);
  const loaded = await ran({ ...files, "index.html": html("one.js", "unused.js") });
  assert.equal(loaded.code, 1);
  assert(loaded.lines.some(line => line.includes("duplicate") && line.includes("shared")));
});

test("a page whose scripts use only what they declare passes, and says how many it read", async () => {
  const { code, lines } = await ran({
    "index.html": html("one.js", "two.js"),
    "one.js": "function one() { two(); }\n",
    "two.js": "function two() {}\none();\n",
  });
  assert.equal(code, 0);
  assert.deepEqual(lines, ["window-scope: 2 scripts, nothing used without a declaration"]);
});

test("a typo in any script the page loads fails the check, with its file, line and column", async () => {
  const { code, lines } = await ran({
    "index.html": html("one.js", "two.js"),
    "one.js": "function one() {}\n",
    "two.js": "one();\n  onne();\n",
  });
  assert.equal(code, 1);
  assert.match(lines[0], /^ui\/two\.js:2:3 {2}call {2}onne — calls a name nothing declares$/);
  assert.equal(lines.at(-1), "window-scope: 1 finding(s) in 2 scripts");
});

test("a function defined in a script the page does not load is not defined to the page", async () => {
  const { code, lines } = await ran({
    "index.html": html("main.js"),
    "main.js": "orphaned();\n",
    "orphan.js": "function orphaned() {}\n",
  });
  assert.equal(code, 1);
  assert.match(lines[0], /^ui\/main\.js:1:1 {2}call {2}orphaned/);
});

test("a vendored bundle's names are the page's only while the page loads the bundle", async () => {
  const withBundle = await ran({ "index.html": html("vendor/three.js", "main.js"), "vendor/three.js": "", "main.js": "THREE.Scene;\n" });
  assert.equal(withBundle.code, 0);
  const without = await ran({ "index.html": html("main.js"), "main.js": "THREE.Scene;\n" });
  assert.equal(without.code, 1);
});

test("a check that cannot be made fails: a missing page, a missing script, a syntax error, a double declaration, a module script", async () => {
  const noPage = await ran({});
  assert.equal(noPage.code, 2);
  assert.match(noPage.lines[0], /^window-scope: cannot check — .*index\.html cannot be read/);

  const missing = await ran({ "index.html": html("gone.js") });
  assert.equal(missing.code, 2);
  assert.match(missing.lines[0], /index\.html loads ui\/gone\.js, which is not there/);

  const syntax = await ran({ "index.html": html("a.js"), "a.js": "const x = 1;\nfunction (\n" });
  assert.equal(syntax.code, 2);
  assert.match(syntax.lines[0], /ui\/a\.js:2:/);

  const twice = await ran({ "index.html": html("a.js", "b.js"), "a.js": "const taken = 1;\n", "b.js": "const taken = 2;\n" });
  assert.equal(twice.code, 2);
  assert.match(twice.lines[0], /ui\/b\.js:1:\d+ Identifier 'taken' has already been declared/);

  const esm = await ran({ "index.html": '<script type="module" src="./a.js"></script>', "a.js": "export {};\n" });
  assert.equal(esm.code, 2);
  assert.match(esm.lines[0], /not a classic script/);

  const none = await ran({ "index.html": "<!doctype html>" });
  assert.equal(none.code, 2);
});

test("an exception for a collection that is no longer declared is itself a finding, so the list cannot outlive what it excuses", async () => {
  const { code, lines } = await ran(
    { "index.html": html("a.js"), "a.js": "const paneHosts = new Map();\n" },
    { notOwned: { paneHosts: "keyed by tab id", paneGone: "no longer there" } },
  );
  assert.equal(code, 1);
  assert.match(lines[0], /^scripts\/window-scope\.mjs:0:0 {2}stale {2}paneGone — is listed as not owned/);
});

test("the machine-readable answer carries the same findings, and the process exit code says it too", async () => {
  const { code, lines } = await ran({ "index.html": html("a.js"), "a.js": "missing();\n" }, { json: true });
  assert.equal(code, 1);
  const body = JSON.parse(lines.join("\n"));
  assert.equal(body.ok, false);
  assert.equal(body.scripts, 1);
  assert.deepEqual(body.findings, [{ kind: "call", name: "missing", file: "ui/a.js", line: 1, column: 1 }]);

  // The window as it stands, as a person runs the command from the checkout.
  const clean = spawnSync(process.execPath, [script, "--json"], { encoding: "utf8", cwd: root });
  assert.equal(clean.status, 0, clean.stdout + clean.stderr);
  assert.equal(JSON.parse(clean.stdout).ok, true);

  const { folder } = await pageWith({ "index.html": html("a.js"), "a.js": "function fine() {}\n" });
  try {
    await writeFile(join(folder, "ui", "a.js"), "typo();\n");
    const failing = spawnSync(process.execPath, [script, "--root", folder], { encoding: "utf8" });
    assert.equal(failing.status, 1);
    assert.match(failing.stdout, /ui\/a\.js:1:1 {2}call {2}typo/);
    await writeFile(join(folder, "ui", "a.js"), "function (\n");
    const unreadable = spawnSync(process.execPath, [script, "--root", folder], { encoding: "utf8" });
    assert.equal(unreadable.status, 2);
  } finally {
    await rm(folder, { recursive: true, force: true });
  }
});

/* The real window. */
async function realScripts() {
  const loaded = scriptsOf(await readFile(join(root, "ui", "index.html"), "utf8"));
  const names = loaded.filter((one) => !one.name.startsWith("vendor/")).map((one) => one.name);
  const scripts = await Promise.all(names.map(async (name) => ({ name: `ui/${name}`, text: await readFile(join(root, "ui", name), "utf8") })));
  return { scripts, vendored: loaded.map((one) => one.name).filter((name) => name.startsWith("vendor/")) };
}

test("the window as it stands uses nothing it does not declare, and every excuse in the list is still needed", async () => {
  const lines = [];
  const code = await run({ root, out: (line) => lines.push(line) });
  assert.equal(code, 0, lines.join("\n"));
  const { scripts } = await realScripts();
  assert.ok(scripts.length > 40, "the page's own list was read");
  assert.ok(scripts.some((one) => one.name === "ui/shell-secret.js") && scripts.some((one) => one.name === "ui/shell-self-act.js"), "the two scripts the hand copy of the list had left out");
  assert.deepEqual(Object.keys(PANE_NAMED_NOT_OWNED).filter((name) => !name.startsWith("pane")), []);
});

test("a typo seeded into the real window is caught where it stands, whether it is a call or an object", async () => {
  const { scripts, vendored } = await realScripts();
  // The two statements that end a shell's view, as they stand in shell-term.js.
  const clock = "    agentClock.sync();\n    previewedTerms.delete(term);\n    dropTermScreen(term);\n";
  const term = scripts.find((one) => one.name === "ui/shell-term.js").text;
  assert.equal(term.split(clock).length, 2, "the statements the typos are seeded into stand once");
  const lineOfClock = term.slice(0, term.indexOf(clock)).split("\n").length;
  const seed = (text) => scripts.map((one) => (one.name === "ui/shell-term.js" ? { ...one, text: text } : one));
  const checked = (text) => checkWindowScope({ scripts: seed(text), vendored, acorn, eslintScope, notOwned: PANE_NAMED_NOT_OWNED });

  const call = checked(term.replace(clock, clock.replace("dropTermScreen(", "dropTermScren(")));
  assert.deepEqual(call.findings, [{ kind: "call", name: "dropTermScren", file: "ui/shell-term.js", line: lineOfClock + 2, column: 5 }]);

  const object = checked(term.replace(clock, clock.replace("agentClock.", "agentClok.")));
  assert.deepEqual(object.findings, [{ kind: "read", name: "agentClok", file: "ui/shell-term.js", line: lineOfClock, column: 5 }]);
});
