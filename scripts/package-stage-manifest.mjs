import { createHash } from "node:crypto";
import { lstat, readFile, readdir, writeFile } from "node:fs/promises";
import { basename, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { compareSemver, packageContract } from "./package-contract.mjs";

export const STAGE_MANIFEST = "release-manifest.json";

function expectedKinds(platform) {
  return platform === "macos" ? ["app", "dmg"] : ["msi", "nsis"];
}

function artifactSuffix(kind) {
  return {
    app: ".app.zip",
    dmg: ".dmg",
    msi: ".msi",
    nsis: "-setup.exe",
  }[kind];
}

async function fileEvidence(path) {
  const held = await lstat(path);
  if (!held.isFile() || held.isSymbolicLink() || held.size === 0) {
    throw new Error(`staged artifact must be a nonempty regular file: ${path}`);
  }
  const bytes = await readFile(path);
  return {
    size: held.size,
    sha256: createHash("sha256").update(bytes).digest("hex"),
  };
}

function safeStageName(name, kind) {
  if (basename(name) !== name || name.includes("\\") || !name.endsWith(artifactSuffix(kind))) {
    throw new Error(`invalid staged ${kind} filename: ${name}`);
  }
}

async function exactStageEntries(stage, expected) {
  const actual = (await readdir(stage)).sort();
  const wanted = [...expected].sort();
  if (JSON.stringify(actual) !== JSON.stringify(wanted)) {
    throw new Error(`staged artifact set differs: expected ${wanted.join(", ")}; found ${actual.join(", ")}`);
  }
}

export async function writeStageManifest(platform, stage, options = {}) {
  const contract = await packageContract(platform, options);
  const directory = resolve(stage);
  const kinds = expectedKinds(platform);
  const names = contract.stagedArtifacts;
  await exactStageEntries(directory, kinds.map((kind) => names[kind]));

  const artifacts = {};
  for (const kind of kinds) {
    const file = names[kind];
    safeStageName(file, kind);
    artifacts[kind] = { file, ...await fileEvidence(join(directory, file)) };
  }

  const manifest = {
    schemaVersion: 1,
    platform,
    productName: contract.productName,
    identifier: contract.identifier,
    version: contract.version,
    binaryName: contract.binaryName,
    wixUpgradeCode: platform === "windows" ? contract.wixUpgradeCode : null,
    sourceRevision: options.sourceRevision ?? process.env.GITHUB_SHA ?? null,
    artifacts,
  };
  await writeFile(join(directory, STAGE_MANIFEST), `${JSON.stringify(manifest, null, 2)}\n`, { flag: "wx" });
  return manifest;
}

function requireString(value, label) {
  if (typeof value !== "string" || value.length === 0) throw new Error(`manifest ${label} is missing`);
  return value;
}

export async function verifyStageManifest(platform, stage, options = {}) {
  const contract = await packageContract(platform, options);
  const directory = resolve(stage);
  const manifestPath = join(directory, STAGE_MANIFEST);
  const manifestStat = await lstat(manifestPath);
  if (!manifestStat.isFile() || manifestStat.isSymbolicLink()) {
    throw new Error("stage manifest must be a regular file");
  }
  const manifest = JSON.parse(await readFile(manifestPath, "utf8"));
  const kinds = expectedKinds(platform);

  if (manifest.schemaVersion !== 1 || manifest.platform !== platform) {
    throw new Error("stage manifest schema or platform differs");
  }
  for (const field of ["productName", "identifier", "binaryName"]) {
    if (requireString(manifest[field], field) !== contract[field]) {
      throw new Error(`stage manifest ${field} differs from the current package contract`);
    }
  }
  requireString(manifest.version, "version");
  if (options.previous) {
    if (compareSemver(manifest.version, contract.version) >= 0) {
      throw new Error(`upgrade baseline ${manifest.version} is not older than ${contract.version}`);
    }
  } else if (manifest.version !== contract.version) {
    throw new Error(`stage manifest version ${manifest.version} differs from ${contract.version}`);
  }
  if (platform === "windows" && manifest.wixUpgradeCode !== contract.wixUpgradeCode) {
    throw new Error("stage manifest WiX upgrade code differs from the pinned package contract");
  }
  if (options.revision && manifest.sourceRevision !== options.revision) {
    throw new Error("stage manifest source revision differs from the tagged baseline commit");
  }

  if (!manifest.artifacts || Object.keys(manifest.artifacts).sort().join(",") !== kinds.sort().join(",")) {
    throw new Error("stage manifest artifact kinds differ");
  }
  const names = kinds.map((kind) => requireString(manifest.artifacts[kind]?.file, `${kind}.file`));
  await exactStageEntries(directory, [STAGE_MANIFEST, ...names]);
  for (const kind of kinds) {
    const entry = manifest.artifacts[kind];
    safeStageName(entry.file, kind);
    const evidence = await fileEvidence(join(directory, entry.file));
    if (entry.size !== evidence.size || entry.sha256 !== evidence.sha256) {
      throw new Error(`stage manifest digest differs for ${entry.file}`);
    }
  }
  return manifest;
}

function fieldValue(value, path) {
  const held = path.split(".").reduce((current, key) => current?.[key], value);
  if (held === undefined || held === null || typeof held === "object") {
    throw new Error(`manifest field is not scalar: ${path}`);
  }
  return held;
}

async function main() {
  const [operation, platform, stage, ...args] = process.argv.slice(2);
  if (!operation || !platform || !stage) {
    throw new Error("usage: node scripts/package-stage-manifest.mjs <write|verify> <macos|windows> <stage> [--previous] [--revision sha] [--field path]");
  }
  if (operation === "write") {
    if (args.length !== 0) throw new Error("write does not accept extra arguments");
    await writeStageManifest(platform, stage);
    return;
  }
  if (operation !== "verify") throw new Error(`unknown stage manifest operation: ${operation}`);

  const options = {};
  let field;
  for (let index = 0; index < args.length; index += 1) {
    if (args[index] === "--previous") options.previous = true;
    else if (args[index] === "--revision" && args[index + 1]) options.revision = args[++index];
    else if (args[index] === "--field" && args[index + 1]) field = args[++index];
    else throw new Error(`unknown stage manifest argument: ${args[index]}`);
  }
  const manifest = await verifyStageManifest(platform, stage, options);
  process.stdout.write(field ? `${fieldValue(manifest, field)}\n` : `${JSON.stringify(manifest)}\n`);
}

if (resolve(process.argv[1] ?? "") === fileURLToPath(import.meta.url)) {
  await main();
}
