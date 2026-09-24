import { createHash } from "node:crypto";
import {
  copyFile,
  mkdir,
  readdir,
  readFile,
  rename,
  rm,
  stat,
} from "node:fs/promises";
import { dirname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const SOURCE = join(ROOT, "ui");
const DIST = join(SOURCE, "dist");

// Production web assets, and only production web assets. Keep this list
// explicit: copying ui/ recursively also ships browser fixtures, prototypes,
// build scripts and local filesystem debris inside the desktop binary.
const FILES = Object.freeze([
  "favicon.ico",
  "favicon.png",
  "index.html",
  "shell-boot.js",
  "shell-browser.js",
  "shell-computer.js",
  "shell-flow.js",
  "shell-doc.js",
  "shell-i18n.js",
  "shell-input.js",
  "shell-knowledge.js",
  "shell-knowledge-3d.js",
  "shell-knowledge-supply.js",
  "shell-explorer-search.js",
  "shell-explorer-tree.js",
  "shell-attach.js",
  "shell-board.js",
  "shell-composer.js",
  "shell-conversation-view.js",
  "shell-path-browser.js",
  "shell-remote.js",
  "shell-sftp.js",
  "shell-scm.js",
  "shell-settings.js",
  "shell-jev.js",
  "shell-status.js",
  "shell-term.js",
  "shell-term-selection.js",
  "shell-update.js",
  "shell-workspace.js",
  "shell.css",
  "shell.js",
  "tokens.css",
  "vendor/cm6-LICENSE",
  "vendor/cm6.js",
]);

// The HTML is the consumer of this allowlist. Matching two producer lists
// cannot detect a newly loaded script omitted by both of them.
const markup = await readFile(join(SOURCE, "index.html"), "utf8");
for (const [tag] of markup.matchAll(/<(?:script|link)\b[^>]*>/gi)) {
  const reference = tag.match(/\b(?:src|href)\s*=\s*["']([^"']+)["']/i)?.[1];
  if (!reference || /^(?:[a-z]+:|\/\/|#)/i.test(reference)) continue;
  const name = reference.replace(/^\.\//, "").split(/[?#]/)[0];
  if (!FILES.includes(name)) throw new Error(`index.html references an unshipped UI asset: ${name}`);
}

const portable = (path) => path.split(sep).join("/");

async function filesBelow(root) {
  const found = [];
  const visit = async (directory) => {
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      const path = join(directory, entry.name);
      if (entry.isDirectory()) await visit(path);
      else if (entry.isFile()) found.push(portable(relative(root, path)));
      else throw new Error(`ui dist contains a non-file entry: ${path}`);
    }
  };
  await visit(root);
  return found.sort();
}

async function assertExact(root) {
  const actual = await filesBelow(root);
  const expected = [...FILES].sort();
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      `ui dist allowlist mismatch\nexpected: ${expected.join(", ")}\nactual:   ${actual.join(", ")}`,
    );
  }

  const digest = createHash("sha256");
  for (const name of expected) {
    const source = await readFile(join(SOURCE, name));
    const built = await readFile(join(root, name));
    if (!source.equals(built)) throw new Error(`ui dist changed ${name}`);
    digest.update(name);
    digest.update("\0");
    digest.update(built);
    digest.update("\0");
  }
  return digest.digest("hex");
}

async function build() {
  // Stage beside the destination so the final rename cannot cross devices.
  const stage = join(SOURCE, `.dist-${process.pid}`);
  await rm(stage, { recursive: true, force: true });
  try {
    for (const name of FILES) {
      const source = join(SOURCE, name);
      if (!(await stat(source)).isFile()) throw new Error(`ui asset is not a file: ${source}`);
      const destination = join(stage, name);
      await mkdir(dirname(destination), { recursive: true });
      await copyFile(source, destination);
    }
    const digest = await assertExact(stage);
    await rm(DIST, { recursive: true, force: true });
    await rename(stage, DIST);
    return digest;
  } finally {
    await rm(stage, { recursive: true, force: true });
  }
}

const checkOnly = process.argv.slice(2).includes("--check");
const digest = checkOnly ? await assertExact(DIST) : await build();
console.log(`${checkOnly ? "checked" : "built"} ui/dist (${FILES.length} files, sha256 ${digest})`);
