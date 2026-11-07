#!/usr/bin/env node
/* What the window's scripts call and read must exist — checked by a parser.
 *
 * The window is not compiled. A call to a name that does not exist is silent
 * until the line runs: `⌘1…⌘9` called `stageLane` for the whole of v0.1, and the
 * lane never moved. This reads every script the page loads, as the page loads
 * them, and says which names are used with no declaration behind them.
 *
 * It replaces a scanner written by hand in Rust (`declared_names`,
 * `called_names`, and the parameter scopes beside them in
 * `main_unit_tests.rs`). That scanner read text: it took any `const`, `let` or
 * `function` anywhere as a declaration for the whole window, so a local name
 * hid a missing global elsewhere; it understood only plain parameters, so a
 * destructured or defaulted one was a false alarm; and each new corner (an
 * arrow inside an arrow, `e79bc58b2`) was another rule in it. A parser
 * (acorn) reads the grammar and a scope analyzer (eslint-scope, ESLint's own)
 * reads the scopes, so none of that is ours to maintain.
 *
 * The scripts are classic scripts and share ONE global scope, so they are
 * joined in the order `ui/index.html` loads them and read as one program. That
 * is also what finds a top-level `const` declared twice across two files: the
 * browser drops the later file at load with a SyntaxError, and so does this.
 * The list of scripts is the page's own: a script the page does not load is not
 * read, and a function defined in it is, to the page, not defined.
 *
 *   node scripts/window-scope.mjs [--root <folder>] [--json]
 *
 * Exit 0: the scope and ownership rules pass. Exit 1: a rule is violated. Exit 2:
 * the check could not be made (no index.html, a script that is missing, a
 * syntax error or a double declaration, a missing dependency) — in CI that is a
 * failure too, never a pass.
 *
 * Needs `acorn` and `eslint-scope` (devDependencies; `npm ci`). */
import { readFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

/* The names a script may use that nothing in the window declares, in three
 * kinds so that a new one is a decision: the language itself, the browser, and
 * the vendored bundles the page loads before the window. A name that is in
 * none of them is an error until it is added here, with the kind it is. */

/* ECMAScript (and ECMA-402): what any engine running this language has. */
export const ECMASCRIPT = Object.freeze([
  "Array", "Boolean", "DataView", "Date", "Error", "FinalizationRegistry", "Float32Array", "Float64Array",
  "Infinity", "Int32Array", "Int8Array", "Intl", "JSON", "Map", "Math", "NaN", "Number", "Object", "Promise",
  "Reflect", "RegExp", "Set", "String", "Symbol", "Uint32Array", "Uint8Array", "WeakMap", "WeakRef", "WeakSet",
  "decodeURI", "decodeURIComponent", "encodeURIComponent", "globalThis", "parseFloat", "parseInt", "undefined",
]);

/* The browser: the DOM and the web platform's own functions. */
export const BROWSER = Object.freeze([
  "Blob", "CSS", "CSSRule", "DOMMatrixReadOnly", "Element", "EncodedVideoChunk", "Event", "HTMLAnchorElement",
  "HTMLElement", "HTMLInputElement", "HTMLSelectElement", "HTMLTextAreaElement", "InputEvent",
  "IntersectionObserver", "KeyboardEvent", "MouseEvent", "MutationObserver", "Node", "NodeFilter",
  "ResizeObserver", "TextDecoder", "TextEncoder", "URL", "URLSearchParams", "VideoDecoder", "atob", "btoa",
  "cancelAnimationFrame", "clearInterval", "clearTimeout", "console", "crypto", "document", "fetch",
  "getComputedStyle", "innerWidth", "localStorage", "matchMedia", "navigator", "performance", "queueMicrotask",
  "requestAnimationFrame", "sessionStorage", "setInterval", "setTimeout", "structuredClone", "window",
]);

/* What a vendored bundle puts on the global object, by the script that does it.
 * The names are allowed only while `index.html` loads that script: a page that
 * stopped loading it would have the window calling something that is not there. */
export const VENDORED = Object.freeze({
  "vendor/cm6.js": ["CM6"],
  "vendor/three.js": ["THREE"],
});

/* A collection a pane keeps is declared with `paneLifecycle.map` or `.set`
 * (ui/shell-pane-lifecycle.js), which is what lets a pane's end release it. A
 * top-level `new Map()` or `new Set()` named like a pane's collection is the
 * way one gets forgotten, so it is an error — unless it is listed here, with
 * why a pane's end does not own it. This is a rule about names, not a proof: it
 * catches the common spelling, and a collection called something else is the
 * review's. An entry whose collection is no longer declared is an error too, so
 * the list cannot outlive what it excuses. */
export const PANE_NAMED_NOT_OWNED = Object.freeze({
  paneHosts: "keyed by tab id: the element a tab draws into, which ends with the tab (`dropTab`)",
  paneHeads: "keyed by tab id: an agent tab's head line, which ends with the tab (`dropTab`)",
  paneSaveTurns: "keyed by worktree path: which save of a workspace's tab set is the latest",
  paneSetsBehind: "keyed by worktree path: the stored set a workspace shows until somebody steps into it",
  paneLedger:
    "keyed by shell id, but a mirror of the backend's ledger, replaced whole on every read (`refreshPaneLedger`): "
    + "an ended pane's row goes at the next read, not at the pane's end",
});

const COLLECTIONS = new Set(["Map", "Set", "WeakMap", "WeakSet"]);
const PANE_NAME = /^pane[A-Z]/;

/* Every node below `node`, parents first. */
function walk(node, visit, parent = null) {
  if (!node || typeof node.type !== "string") return;
  visit(node, parent);
  for (const key of Object.keys(node)) {
    if (key === "loc" || key === "range") continue;
    const value = node[key];
    if (Array.isArray(value)) {
      for (const item of value) walk(item, visit, node);
    } else if (value && typeof value === "object") {
      walk(value, visit, node);
    }
  }
}

/* Where an offset of the joined program is: the script, and the line and column
 * inside that script. */
function placeOf(parts, offset) {
  let low = 0;
  let high = parts.length - 1;
  while (low < high) {
    const middle = Math.ceil((low + high) / 2);
    if (parts[middle].start <= offset) low = middle;
    else high = middle - 1;
  }
  const part = parts[low];
  const before = part.text.slice(0, offset - part.start);
  const line = before.split("\n").length;
  const column = offset - part.start - (before.lastIndexOf("\n") + 1) + 1;
  return { file: part.name, line, column };
}

/* The checks, on scripts already read: `scripts` is `[{ name, text }]` in the
 * order the page loads them, `vendored` the names of the vendored scripts the
 * page loads. Answers `{ error }` when the program cannot be read, else
 * `{ findings, paneNamed }` — each finding `{ kind, name, file, line, column }`,
 * kind one of `call` (a name is called or constructed), `read` (a name is read)
 * `duplicate` (more than one global definition), and `ownership` (a collection named like a pane's, declared outside the
 * lifecycle and not listed in `notOwned`); `paneNamed` is every such collection
 * the program declares, listed or not. */
export function checkWindowScope({ scripts, vendored = [], acorn, eslintScope, notOwned = {} }) {
  const parts = [];
  let joined = "";
  for (const { name, text } of scripts) {
    parts.push({ name, text, start: joined.length });
    joined += text.endsWith("\n") ? text : `${text}\n`;
  }

  let program;
  try {
    program = acorn.parse(joined, { ecmaVersion: "latest", sourceType: "script", locations: true, ranges: true });
  } catch (error) {
    const place = placeOf(parts, error.pos ?? 0);
    return { error: { kind: "syntax", message: String(error.message).replace(/ \(\d+:\d+\)$/, ""), ...place } };
  }

  const callee = new Set();
  const typeofOperand = new Set();
  walk(program, (node) => {
    if ((node.type === "CallExpression" || node.type === "NewExpression") && node.callee.type === "Identifier") {
      callee.add(node.callee);
    }
    if (node.type === "UnaryExpression" && node.operator === "typeof" && node.argument.type === "Identifier") {
      typeofOperand.add(node.argument);
    }
  });

  const manager = eslintScope.analyze(program, { ecmaVersion: 2025, sourceType: "script", fallback: "iteration" });
  const scope = manager.globalScope;
  // A script's global scope leaves unresolved every reference no function or
  // block resolved, its own declarations among them; ESLint settles those
  // against the global scope's variables afterwards, and so does this.
  const declared = new Set(scope.variables.map((variable) => variable.name));
  const allowed = new Set([...ECMASCRIPT, ...BROWSER, ...vendored.flatMap((script) => VENDORED[script] ?? [])]);

  const findings = [];
  // JavaScript permits repeated function/var declarations, but the window's
  // policy is one definition per global name. Use the same semantic scope and
  // loaded-script list as the reference checks, including destructuring.
  for (const variable of scope.variables) {
    const definitions = variable.defs.filter(definition => typeof definition.name?.start === "number");
    if (definitions.length < 2) continue;
    const previous = placeOf(parts, definitions[0].name.start);
    for (const definition of definitions.slice(1)) {
      findings.push({ kind: "duplicate", name: variable.name, previous, ...placeOf(parts, definition.name.start) });
    }
  }
  for (const reference of scope.through) {
    const { identifier } = reference;
    if (declared.has(identifier.name) || allowed.has(identifier.name)) continue;
    // `typeof name` is how a script asks whether a name exists.
    if (typeofOperand.has(identifier)) continue;
    findings.push({
      kind: callee.has(identifier) ? "call" : "read",
      name: identifier.name,
      ...placeOf(parts, identifier.start),
    });
  }

  const paneNamed = [];
  for (const statement of program.body) {
    if (statement.type !== "VariableDeclaration") continue;
    for (const declarator of statement.declarations) {
      const { id, init } = declarator;
      if (id.type !== "Identifier" || !PANE_NAME.test(id.name)) continue;
      if (init?.type === "NewExpression" && init.callee.type === "Identifier" && COLLECTIONS.has(init.callee.name)) {
        paneNamed.push(id.name);
        if (!(id.name in notOwned)) findings.push({ kind: "ownership", name: id.name, ...placeOf(parts, id.start) });
      }
    }
  }

  findings.sort((one, other) => (
    one.file === other.file ? one.line - other.line || one.column - other.column : (one.file < other.file ? -1 : 1)
  ));
  return { findings, paneNamed };
}

/* The scripts a page loads, in order, from the page itself. */
export function scriptsOf(markup) {
  const live = markup.replace(/<!--[\s\S]*?-->/g, "");
  const sources = [];
  for (const [tag] of live.matchAll(/<script\b[^>]*>/gi)) {
    const source = tag.match(/\bsrc\s*=\s*["']([^"']+)["']/i)?.[1];
    if (!source) continue;
    const type = tag.match(/\btype\s*=\s*["']([^"']*)["']/i)?.[1] ?? "";
    sources.push({ name: source.replace(/^\.\//, "").split(/[?#]/)[0], type });
  }
  return sources;
}

const SAYS = {
  call: "calls a name nothing declares",
  read: "reads a name nothing declares",
  ownership: "declares a pane's collection outside paneLifecycle",
  duplicate: "declares a global name more than once",
  stale: "is listed as not owned by a pane's end, but no collection of that name is declared",
};

async function loadParsers() {
  try {
    const [acorn, scopeModule] = await Promise.all([import("acorn"), import("eslint-scope")]);
    return { acorn, eslintScope: scopeModule.analyze ? scopeModule : scopeModule.default };
  } catch (error) {
    if (error?.code !== "ERR_MODULE_NOT_FOUND") throw error;
    return { missing: String(error.message).split("\n")[0] };
  }
}

/* The command: the page under `root`, read and checked, and what it found said
 * on `out`. Answers the exit code. */
export async function run({
  root,
  json = false,
  notOwned = PANE_NAMED_NOT_OWNED,
  out = (line) => process.stdout.write(`${line}\n`),
}) {
  const answer = (body, code) => {
    if (json) out(JSON.stringify(body, null, 2));
    return code;
  };
  const cannot = (message, extra = {}) => {
    if (!json) out(`window-scope: cannot check — ${message}`);
    return answer({ ok: false, root, error: { message, ...extra }, findings: [] }, 2);
  };

  const parsers = await loadParsers();
  if (parsers.missing) return cannot(`${parsers.missing} (run \`npm ci\`)`, { kind: "dependency" });

  let markup;
  try {
    markup = await readFile(join(root, "ui", "index.html"), "utf8");
  } catch {
    return cannot(`${join(root, "ui", "index.html")} cannot be read`, { kind: "index" });
  }
  const loaded = scriptsOf(markup);
  if (loaded.some((one) => one.type !== "" && one.type !== "text/javascript")) {
    return cannot("index.html loads a script that is not a classic script, which share one scope", { kind: "unsupported" });
  }
  const vendored = loaded.map((one) => one.name).filter((name) => name.startsWith("vendor/"));
  const scripts = [];
  for (const { name } of loaded.filter((one) => !one.name.startsWith("vendor/"))) {
    try {
      scripts.push({ name: `ui/${name}`, text: await readFile(join(root, "ui", name), "utf8") });
    } catch {
      return cannot(`index.html loads ui/${name}, which is not there`, { kind: "missing", file: `ui/${name}` });
    }
  }
  if (scripts.length === 0) return cannot("index.html loads no window script", { kind: "index" });

  const result = checkWindowScope({ scripts, vendored, notOwned, ...parsers });
  if (result.error) {
    const { file, line, column, message } = result.error;
    return cannot(`${file}:${line}:${column} ${message}`, result.error);
  }
  const { findings } = result;
  for (const name of Object.keys(notOwned)) {
    if (!result.paneNamed.includes(name)) findings.push({ kind: "stale", name, file: "scripts/window-scope.mjs", line: 0, column: 0 });
  }
  if (!json) {
    for (const found of findings) out(`${found.file}:${found.line}:${found.column}  ${found.kind}  ${found.name} — ${SAYS[found.kind]}`);
    out(findings.length === 0
      ? `window-scope: ${scripts.length} scripts, nothing used without a declaration`
      : `window-scope: ${findings.length} finding(s) in ${scripts.length} scripts`);
  }
  return answer({ ok: findings.length === 0, root, scripts: scripts.length, findings }, findings.length === 0 ? 0 : 1);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const argv = process.argv.slice(2);
  const at = argv.indexOf("--root");
  const root = resolve(at >= 0 ? argv[at + 1] ?? "." : join(dirname(fileURLToPath(import.meta.url)), ".."));
  process.exitCode = await run({ root, json: argv.includes("--json") });
}
