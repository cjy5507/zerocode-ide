/* Rebuild `ui/vendor/cm6.js` — the editor, vendored as one file.
 *
 *   node ui/vendor/build-cm6.mjs
 *
 * The window has no bundler and no network at runtime: `index.html` is served
 * as it sits on disk, and the CSP is `default-src 'self'`. CodeMirror 6 ships
 * as ~40 ESM packages on npm, which is a shape this window cannot load. So the
 * npm half happens HERE, once, offline, in a scratch directory OUTSIDE the
 * checkout — and what lands in the repository is a single IIFE that a plain
 * `<script src>` can take, plus this script and the licence.
 *
 * Nothing about that is a runtime dependency. There is no `node_modules` in
 * this repository, no `package.json`, no install step in the build, and no
 * worker or `blob:` URL in the output — so the CSP is untouched.
 *
 * What comes out attaches to `window.CM6`. See ORDER below for what is in it
 * and why: this file is the whole definition of that surface, so a symbol the
 * window wants and cannot see is added here and the script re-run.
 *
 * Measured before committing anything: the byte count of the output is printed
 * at the end, and the ledger quotes it.
 */

import { spawnSync } from "node:child_process";
import { mkdirSync, writeFileSync, readFileSync, readdirSync, statSync, rmSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { tmpdir } from "node:os";

const VENDOR = dirname(fileURLToPath(import.meta.url));
const OUT = join(VENDOR, "cm6.js");
/* Outside the checkout on purpose. A scratch tree inside `ui/` would be
 * picked up by the dev server, the Rust gates that read `ui/**`, and git. */
const WORK = join(tmpdir(), "zerocode-cm6-build");

/* Pinned, so re-running this on another machine produces the same editor.
 * Bumping a version is a deliberate edit to this list, never a `^` drifting
 * underneath us. */
const DEPS = {
  "@codemirror/state": "6.7.1",
  "@codemirror/view": "6.43.8",
  "@codemirror/language": "6.12.4",
  "@codemirror/commands": "6.10.4",
  "@codemirror/search": "6.7.1",
  "@codemirror/autocomplete": "6.20.3",
  "@codemirror/merge": "6.12.2",
  "@lezer/highlight": "1.2.3",
  "@codemirror/legacy-modes": "6.5.3",
  "@codemirror/lang-cpp": "6.0.3",
  "@codemirror/lang-css": "6.3.1",
  "@codemirror/lang-go": "6.0.1",
  "@codemirror/lang-html": "6.4.12",
  "@codemirror/lang-java": "6.0.2",
  "@codemirror/lang-javascript": "6.2.5",
  "@codemirror/lang-json": "6.0.2",
  "@codemirror/lang-less": "6.0.2",
  "@codemirror/lang-liquid": "6.3.2",
  "@codemirror/lang-markdown": "6.5.2",
  "@codemirror/lang-php": "6.0.2",
  "@codemirror/lang-python": "6.2.1",
  "@codemirror/lang-rust": "6.0.2",
  "@codemirror/lang-sass": "6.0.2",
  "@codemirror/lang-sql": "6.10.0",
  // The one package still below 1.0 upstream. Pinned like the rest; it is a
  // grammar, and a grammar that changes under us changes what a file looks
  // like without anybody editing this repository.
  "@codemirror/lang-vue": "0.1.3",
  "@codemirror/lang-wast": "6.0.2",
  "@codemirror/lang-xml": "6.1.0",
  "@codemirror/lang-yaml": "6.1.3",
  esbuild: "0.28.2",
};

/* The entry. Written out rather than kept as a file beside this one because
 * it is only ever read by the esbuild run three lines below — a second file in
 * `ui/vendor/` would look like something the window loads, and it is not. */
const ENTRY = String.raw`
import {EditorState, EditorSelection, Compartment, StateEffect, StateField, Text, Prec} from "@codemirror/state"
import {
  EditorView, keymap, lineNumbers, highlightActiveLine, highlightActiveLineGutter,
  drawSelection, dropCursor, rectangularSelection, crosshairCursor, highlightSpecialChars,
  placeholder, ViewPlugin, Decoration, scrollPastEnd, tooltips,
} from "@codemirror/view"
import {
  defaultKeymap, history, historyKeymap, indentWithTab, undo, redo, standardKeymap,
  selectAll, cursorDocStart, cursorDocEnd,
} from "@codemirror/commands"
import {
  search, searchKeymap, openSearchPanel, closeSearchPanel, findNext, findPrevious,
  replaceNext, replaceAll, selectMatches, SearchQuery, setSearchQuery, getSearchQuery,
  searchPanelOpen, highlightSelectionMatches, selectNextOccurrence,
} from "@codemirror/search"
import {
  syntaxHighlighting, HighlightStyle, defaultHighlightStyle, foldGutter, foldKeymap,
  codeFolding, foldAll, unfoldAll, foldCode, unfoldCode, indentOnInput, bracketMatching,
  StreamLanguage, LanguageSupport, indentUnit, syntaxTree, foldable,
} from "@codemirror/language"
import {
  autocompletion, completionKeymap, closeBrackets, closeBracketsKeymap, acceptCompletion,
  completeAnyWord,
} from "@codemirror/autocomplete"
import {tags} from "@lezer/highlight"
import {MergeView, unifiedMergeView} from "@codemirror/merge"

import {cpp} from "@codemirror/lang-cpp"
import {css} from "@codemirror/lang-css"
import {go} from "@codemirror/lang-go"
import {html} from "@codemirror/lang-html"
import {java} from "@codemirror/lang-java"
import {javascript} from "@codemirror/lang-javascript"
import {json} from "@codemirror/lang-json"
import {less} from "@codemirror/lang-less"
import {liquid} from "@codemirror/lang-liquid"
import {markdown} from "@codemirror/lang-markdown"
import {php} from "@codemirror/lang-php"
import {python} from "@codemirror/lang-python"
import {rust} from "@codemirror/lang-rust"
import {sass} from "@codemirror/lang-sass"
import {sql, PostgreSQL} from "@codemirror/lang-sql"
import {vue} from "@codemirror/lang-vue"
import {wast} from "@codemirror/lang-wast"
import {xml} from "@codemirror/lang-xml"
import {yaml} from "@codemirror/lang-yaml"

import {c, csharp, scala, kotlin, objectiveC, objectiveCpp, dart} from "@codemirror/legacy-modes/mode/clike"
import {oCaml, fSharp, sml} from "@codemirror/legacy-modes/mode/mllike"
import {shell} from "@codemirror/legacy-modes/mode/shell"
import {toml} from "@codemirror/legacy-modes/mode/toml"
import {ruby} from "@codemirror/legacy-modes/mode/ruby"
import {swift} from "@codemirror/legacy-modes/mode/swift"
import {lua} from "@codemirror/legacy-modes/mode/lua"
import {haskell} from "@codemirror/legacy-modes/mode/haskell"
import {diff} from "@codemirror/legacy-modes/mode/diff"
import {dockerFile} from "@codemirror/legacy-modes/mode/dockerfile"
import {nginx} from "@codemirror/legacy-modes/mode/nginx"
import {properties} from "@codemirror/legacy-modes/mode/properties"
import {protobuf} from "@codemirror/legacy-modes/mode/protobuf"
import {r} from "@codemirror/legacy-modes/mode/r"
import {perl} from "@codemirror/legacy-modes/mode/perl"
import {powerShell} from "@codemirror/legacy-modes/mode/powershell"
import {erlang} from "@codemirror/legacy-modes/mode/erlang"
import {elm} from "@codemirror/legacy-modes/mode/elm"
import {clojure} from "@codemirror/legacy-modes/mode/clojure"
import {groovy} from "@codemirror/legacy-modes/mode/groovy"
import {julia} from "@codemirror/legacy-modes/mode/julia"
import {stex} from "@codemirror/legacy-modes/mode/stex"
import {vb} from "@codemirror/legacy-modes/mode/vb"
import {verilog} from "@codemirror/legacy-modes/mode/verilog"
import {vhdl} from "@codemirror/legacy-modes/mode/vhdl"
import {tcl} from "@codemirror/legacy-modes/mode/tcl"
import {scheme} from "@codemirror/legacy-modes/mode/scheme"
import {commonLisp} from "@codemirror/legacy-modes/mode/commonlisp"
import {crystal} from "@codemirror/legacy-modes/mode/crystal"
import {coffeeScript} from "@codemirror/legacy-modes/mode/coffeescript"
import {cmake} from "@codemirror/legacy-modes/mode/cmake"
import {gherkin} from "@codemirror/legacy-modes/mode/gherkin"
import {http} from "@codemirror/legacy-modes/mode/http"
import {turtle} from "@codemirror/legacy-modes/mode/turtle"
import {sparql} from "@codemirror/legacy-modes/mode/sparql"
import {textile} from "@codemirror/legacy-modes/mode/textile"
import {pascal} from "@codemirror/legacy-modes/mode/pascal"
import {fortran} from "@codemirror/legacy-modes/mode/fortran"
import {cobol} from "@codemirror/legacy-modes/mode/cobol"
import {nsis} from "@codemirror/legacy-modes/mode/nsis"
import {pug} from "@codemirror/legacy-modes/mode/pug"
import {haxe} from "@codemirror/legacy-modes/mode/haxe"
import {factor} from "@codemirror/legacy-modes/mode/factor"
import {forth} from "@codemirror/legacy-modes/mode/forth"

const legacy = (mode) => () => StreamLanguage.define(mode)

/* One table, read two ways: a name for the status line and the gates, and a
 * loader for the extension. The names are the language's own — "rust", not
 * "Rust" — because they are also what a test pins.
 *
 * The extensions are the keys, so a file is looked up in one step; the reverse
 * (name → extensions) is nowhere needed. Every entry is lower-case, and the
 * lookup lower-cases what it is handed, so "README.MD" finds markdown. */
const LANGS = {
  rs: ["rust", rust],
  toml: ["toml", legacy(toml)],
  lock: ["toml", legacy(toml)],

  js: ["javascript", () => javascript()],
  mjs: ["javascript", () => javascript()],
  cjs: ["javascript", () => javascript()],
  jsx: ["jsx", () => javascript({jsx: true})],
  ts: ["typescript", () => javascript({typescript: true})],
  mts: ["typescript", () => javascript({typescript: true})],
  cts: ["typescript", () => javascript({typescript: true})],
  tsx: ["tsx", () => javascript({jsx: true, typescript: true})],

  json: ["json", json],
  jsonc: ["json", json],
  json5: ["json", json],
  jsonl: ["json", json],
  ndjson: ["json", json],
  webmanifest: ["json", json],

  css: ["css", css],
  scss: ["sass", () => sass()],
  sass: ["sass", () => sass({indented: true})],
  less: ["less", less],

  html: ["html", html],
  htm: ["html", html],
  xhtml: ["html", html],
  vue: ["vue", vue],
  svelte: ["html", html],
  astro: ["html", html],
  liquid: ["liquid", liquid],
  pug: ["pug", legacy(pug)],
  jade: ["pug", legacy(pug)],

  xml: ["xml", xml],
  svg: ["xml", xml],
  xsl: ["xml", xml],
  xsd: ["xml", xml],
  plist: ["xml", xml],
  rss: ["xml", xml],
  atom: ["xml", xml],

  yaml: ["yaml", yaml],
  yml: ["yaml", yaml],

  md: ["markdown", markdown],
  markdown: ["markdown", markdown],
  mdx: ["markdown", markdown],
  mdown: ["markdown", markdown],

  py: ["python", python],
  pyi: ["python", python],
  pyw: ["python", python],

  go: ["go", go],
  java: ["java", java],
  kt: ["kotlin", legacy(kotlin)],
  kts: ["kotlin", legacy(kotlin)],
  scala: ["scala", legacy(scala)],
  sbt: ["scala", legacy(scala)],
  groovy: ["groovy", legacy(groovy)],
  gradle: ["groovy", legacy(groovy)],
  cs: ["csharp", legacy(csharp)],
  dart: ["dart", legacy(dart)],

  c: ["c", legacy(c)],
  h: ["c", legacy(c)],
  cpp: ["cpp", cpp],
  cxx: ["cpp", cpp],
  cc: ["cpp", cpp],
  hpp: ["cpp", cpp],
  hxx: ["cpp", cpp],
  hh: ["cpp", cpp],
  ino: ["cpp", cpp],
  m: ["objective-c", legacy(objectiveC)],
  mm: ["objective-c", legacy(objectiveCpp)],

  sh: ["shell", legacy(shell)],
  bash: ["shell", legacy(shell)],
  zsh: ["shell", legacy(shell)],
  ksh: ["shell", legacy(shell)],
  fish: ["shell", legacy(shell)],
  ps1: ["powershell", legacy(powerShell)],
  psm1: ["powershell", legacy(powerShell)],

  php: ["php", php],
  rb: ["ruby", legacy(ruby)],
  rake: ["ruby", legacy(ruby)],
  gemspec: ["ruby", legacy(ruby)],
  erb: ["ruby", legacy(ruby)],
  swift: ["swift", legacy(swift)],
  lua: ["lua", legacy(lua)],
  pl: ["perl", legacy(perl)],
  pm: ["perl", legacy(perl)],
  r: ["r", legacy(r)],
  jl: ["julia", legacy(julia)],
  hs: ["haskell", legacy(haskell)],
  ex: ["erlang", legacy(erlang)],
  exs: ["erlang", legacy(erlang)],
  erl: ["erlang", legacy(erlang)],
  hrl: ["erlang", legacy(erlang)],
  elm: ["elm", legacy(elm)],
  clj: ["clojure", legacy(clojure)],
  cljs: ["clojure", legacy(clojure)],
  cljc: ["clojure", legacy(clojure)],
  edn: ["clojure", legacy(clojure)],
  scm: ["scheme", legacy(scheme)],
  ss: ["scheme", legacy(scheme)],
  lisp: ["commonlisp", legacy(commonLisp)],
  el: ["commonlisp", legacy(commonLisp)],
  ml: ["ocaml", legacy(oCaml)],
  mli: ["ocaml", legacy(oCaml)],
  fs: ["fsharp", legacy(fSharp)],
  fsx: ["fsharp", legacy(fSharp)],
  sml: ["sml", legacy(sml)],
  cr: ["crystal", legacy(crystal)],
  coffee: ["coffeescript", legacy(coffeeScript)],
  hx: ["haxe", legacy(haxe)],
  vb: ["vb", legacy(vb)],
  pas: ["pascal", legacy(pascal)],
  f: ["fortran", legacy(fortran)],
  f90: ["fortran", legacy(fortran)],
  f95: ["fortran", legacy(fortran)],
  cob: ["cobol", legacy(cobol)],
  cbl: ["cobol", legacy(cobol)],
  tcl: ["tcl", legacy(tcl)],
  v: ["verilog", legacy(verilog)],
  sv: ["verilog", legacy(verilog)],
  svh: ["verilog", legacy(verilog)],
  vhd: ["vhdl", legacy(vhdl)],
  vhdl: ["vhdl", legacy(vhdl)],
  fth: ["forth", legacy(forth)],
  factor: ["factor", legacy(factor)],
  nsi: ["nsis", legacy(nsis)],

  sql: ["sql", () => sql({dialect: PostgreSQL})],
  psql: ["sql", () => sql({dialect: PostgreSQL})],
  proto: ["protobuf", legacy(protobuf)],
  wat: ["wast", wast],
  wast: ["wast", wast],

  diff: ["diff", legacy(diff)],
  patch: ["diff", legacy(diff)],
  ini: ["properties", legacy(properties)],
  cfg: ["properties", legacy(properties)],
  conf: ["properties", legacy(properties)],
  properties: ["properties", legacy(properties)],
  env: ["properties", legacy(properties)],
  editorconfig: ["properties", legacy(properties)],
  tex: ["latex", legacy(stex)],
  sty: ["latex", legacy(stex)],
  cls: ["latex", legacy(stex)],
  textile: ["textile", legacy(textile)],
  feature: ["gherkin", legacy(gherkin)],
  http: ["http", legacy(http)],
  rest: ["http", legacy(http)],
  ttl: ["turtle", legacy(turtle)],
  rq: ["sparql", legacy(sparql)],
  cmake: ["cmake", legacy(cmake)],
}

/* Files whose whole NAME is the language — a Dockerfile has no extension, and
 * extensionOf("Makefile") is the empty string. Looked up before the
 * extension, lower-cased the same way. */
const BY_NAME = {
  dockerfile: ["dockerfile", legacy(dockerFile)],
  containerfile: ["dockerfile", legacy(dockerFile)],
  "cargo.lock": ["toml", legacy(toml)],
  "cargo.toml": ["toml", legacy(toml)],
  gemfile: ["ruby", legacy(ruby)],
  rakefile: ["ruby", legacy(ruby)],
  cmakelists: ["cmake", legacy(cmake)],
  "cmakelists.txt": ["cmake", legacy(cmake)],
  ".gitignore": ["properties", legacy(properties)],
  ".gitattributes": ["properties", legacy(properties)],
  ".env": ["properties", legacy(properties)],
  ".editorconfig": ["properties", legacy(properties)],
  "nginx.conf": ["nginx", legacy(nginx)],
  ".bashrc": ["shell", legacy(shell)],
  ".zshrc": ["shell", legacy(shell)],
  ".profile": ["shell", legacy(shell)],
}

function entryFor(name) {
  const lower = String(name || "").toLowerCase()
  const base = lower.split("/").pop()
  if (BY_NAME[base]) return BY_NAME[base]
  const dot = base.lastIndexOf(".")
  if (dot <= 0 && !base.startsWith(".")) return null
  const ext = base.slice(dot + 1)
  return LANGS[ext] || null
}

/* The name this file's language goes by, or "" when nothing claims it. Kept
 * separate from the loader so a caller that only wants to LABEL a file does
 * not instantiate a parser to find out. */
function languageNameFor(name) {
  const found = entryFor(name)
  return found ? found[0] : ""
}

/* The extension that highlights this file, or null for plain text. */
function languageFor(name) {
  const found = entryFor(name)
  return found ? found[1]() : null
}

window.CM6 = {
  EditorState, EditorSelection, Compartment, StateEffect, StateField, Text, Prec,
  EditorView, keymap, lineNumbers, highlightActiveLine, highlightActiveLineGutter,
  drawSelection, dropCursor, rectangularSelection, crosshairCursor, highlightSpecialChars,
  placeholder, ViewPlugin, Decoration, scrollPastEnd, tooltips,
  defaultKeymap, history, historyKeymap, indentWithTab, undo, redo, standardKeymap,
  selectAll, cursorDocStart, cursorDocEnd,
  search, searchKeymap, openSearchPanel, closeSearchPanel, findNext, findPrevious,
  replaceNext, replaceAll, selectMatches, SearchQuery, setSearchQuery, getSearchQuery,
  searchPanelOpen, highlightSelectionMatches, selectNextOccurrence,
  syntaxHighlighting, HighlightStyle, defaultHighlightStyle, foldGutter, foldKeymap,
  codeFolding, foldAll, unfoldAll, foldCode, unfoldCode, indentOnInput, bracketMatching,
  StreamLanguage, LanguageSupport, indentUnit, syntaxTree, foldable,
  autocompletion, completionKeymap, closeBrackets, closeBracketsKeymap, acceptCompletion,
  completeAnyWord,
  tags,
  MergeView, unifiedMergeView,
  languageFor, languageNameFor,
}
`;

const run = (command, args, cwd) => {
  const done = spawnSync(command, args, { cwd, stdio: "inherit" });
  if (done.status !== 0) {
    console.error(`\n${command} ${args.join(" ")} failed (${done.status})`);
    process.exit(1);
  }
};

rmSync(WORK, { recursive: true, force: true });
mkdirSync(WORK, { recursive: true });
writeFileSync(
  join(WORK, "package.json"),
  `${JSON.stringify({ name: "zerocode-cm6-build", private: true, type: "module", dependencies: DEPS }, null, 2)}\n`,
);
writeFileSync(join(WORK, "entry.mjs"), ENTRY);

console.log(`installing into ${WORK} …`);
run("npm", ["install", "--no-audit", "--no-fund", "--silent"], WORK);

console.log("bundling …");
run(
  join(WORK, "node_modules", ".bin", "esbuild"),
  [
    "entry.mjs",
    "--bundle",
    "--format=iife",
    "--minify",
    "--legal-comments=none",
    "--target=safari15",
    `--outfile=${OUT}`,
  ],
  WORK,
);

/* The licence travels with the code.
 *
 * Every package that ends up inside `cm6.js` is MIT under the same holder, so
 * the text is stated once — but WHICH packages is a fact that changes when the
 * language table above does, so the list is read off the install rather than
 * typed. A licence naming packages the file no longer contains, or missing one
 * it does, is a licence that has stopped being a notice. */
const bundled = [];
const walk = (dir) => {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (!entry.isDirectory()) continue;
    const at = join(dir, entry.name);
    if (entry.name.startsWith("@")) {
      walk(at);
      continue;
    }
    try {
      const pkg = JSON.parse(readFileSync(join(at, "package.json"), "utf8"));
      // esbuild is the tool that made the file, not a thing inside it.
      if (pkg.name === "esbuild" || pkg.name?.startsWith("@esbuild/")) continue;
      bundled.push(`${pkg.name}@${pkg.version} (${pkg.license})`);
    } catch {
      /* not a package — a `.bin` directory, or a stray folder */
    }
  }
};
walk(join(WORK, "node_modules"));
bundled.sort();

const notMit = bundled.filter((line) => !line.endsWith("(MIT)"));
if (notMit.length) {
  console.error(`\nnot MIT, so this notice cannot cover it:\n  ${notMit.join("\n  ")}`);
  process.exit(1);
}

writeFileSync(
  join(VENDOR, "cm6-LICENSE"),
  `ui/vendor/cm6.js is a build of CodeMirror 6. It contains, in full or in\n` +
    `part, the following packages — every one of them MIT, under the notice\n` +
    `reproduced below. Regenerate both this file and the bundle with\n` +
    `\`node ui/vendor/build-cm6.mjs\`.\n\n` +
    `${bundled.map((line) => `  ${line}`).join("\n")}\n\n` +
    `${readFileSync(join(WORK, "node_modules", "@codemirror", "state", "LICENSE"), "utf8")}`,
);

console.log(`\n${OUT}  ${statSync(OUT).size} bytes`);
console.log(`cm6-LICENSE  ${bundled.length} packages, all MIT`);
