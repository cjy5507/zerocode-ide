import { readdir, readFile } from "node:fs/promises";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");

const PLATFORMS = new Set(["macos", "windows"]);
const MIGRATION_ARTIFACT = "automation-runs.json";
const SEMVER = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z.-]+)?$/;

function migrationRow(id) {
  return {
    id,
    automation_id: "native-package-smoke",
    at_epoch_ms: 0,
    root: "native-package-smoke",
    term: 0,
    made_worktree: null,
    skipped: null,
    ended_at_epoch_ms: null,
    exit_code: null,
  };
}

function fixtureBytes(id) {
  return Buffer.from(`${JSON.stringify([migrationRow(id)], null, 2)}\n`).toString("base64");
}

function tomlSectionValue(toml, section, key) {
  const header = toml.match(new RegExp(`^\\[${section.replace(".", "\\.")}\\]\\s*$`, "m"));
  if (!header) throw new Error(`Cargo.toml [${section}].${key} is missing`);
  const afterHeader = toml.slice(header.index + header[0].length);
  const nextSection = afterHeader.search(/^\[/m);
  const packageSection = nextSection === -1 ? afterHeader : afterHeader.slice(0, nextSection);
  const value = packageSection.match(new RegExp(`^${key}\\s*=\\s*"([^"]+)"\\s*$`, "m"))?.[1];
  if (!value) throw new Error(`Cargo.toml [${section}].${key} is missing`);
  return value;
}

export function cargoPackageName(toml) {
  return tomlSectionValue(toml, "package", "name");
}

export function cargoBinaryNames(toml) {
  return toml.split(/^\[\[bin\]\][ \t]*$/m).slice(1)
    .map((section) => section.split(/^\[/m)[0].match(/^name\s*=\s*"([^"\n]+)"/m)?.[1])
    .filter(Boolean);
}

export function cargoWorkspaceVersion(toml) {
  return tomlSectionValue(toml, "workspace.package", "version");
}

function parsedSemver(version) {
  const match = version.match(SEMVER);
  if (!match) throw new Error(`package version is not semantic versioning: ${version}`);
  return {
    numbers: match.slice(1, 4).map(Number),
    prerelease: match[4]?.split(".") ?? [],
  };
}

export function compareSemver(left, right) {
  const a = parsedSemver(left);
  const b = parsedSemver(right);
  for (let index = 0; index < a.numbers.length; index += 1) {
    if (a.numbers[index] !== b.numbers[index]) return a.numbers[index] < b.numbers[index] ? -1 : 1;
  }
  if (a.prerelease.length === 0 || b.prerelease.length === 0) {
    return a.prerelease.length === b.prerelease.length ? 0 : a.prerelease.length === 0 ? 1 : -1;
  }
  const length = Math.max(a.prerelease.length, b.prerelease.length);
  for (let index = 0; index < length; index += 1) {
    const aPart = a.prerelease[index];
    const bPart = b.prerelease[index];
    if (aPart === undefined || bPart === undefined) return aPart === undefined ? -1 : 1;
    if (aPart === bPart) continue;
    const aNumber = /^\d+$/.test(aPart);
    const bNumber = /^\d+$/.test(bPart);
    if (aNumber && bNumber) return Number(aPart) < Number(bPart) ? -1 : 1;
    if (aNumber !== bNumber) return aNumber ? -1 : 1;
    return aPart < bPart ? -1 : 1;
  }
  return 0;
}

export function artifactUploadName(platform, releaseTag, productName) {
  if (!PLATFORMS.has(platform)) throw new Error("platform must be macos or windows");
  const namespace = productName?.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "");
  if (!namespace) throw new Error("product name cannot form an artifact namespace");
  if (releaseTag === "unsigned") return `${namespace}-unsigned-package-smoke-${platform}`;
  if (!/^v[^/]+$/.test(releaseTag)) throw new Error(`invalid release tag: ${releaseTag}`);
  return `${namespace}-release-${platform}-${releaseTag}`;
}

export async function packageIdentity(options = {}) {
  const root = resolve(options.root ?? ROOT);
  const shell = join(root, "crates", "zerocode-shell");
  const config = JSON.parse(await readFile(join(shell, "tauri.conf.json"), "utf8"));
  const windowsConfig = JSON.parse(await readFile(join(shell, "tauri.windows.conf.json"), "utf8"));
  const configuredBinary = config.mainBinaryName?.trim();
  const manifest = await readFile(join(shell, "Cargo.toml"), "utf8");
  const cargoName = cargoPackageName(manifest);
  const workspaceVersion = cargoWorkspaceVersion(await readFile(join(root, "Cargo.toml"), "utf8"));
  const binaryName = configuredBinary || cargoName;
  const version = config.version?.trim();
  const wixUpgradeCode = windowsConfig.bundle?.windows?.wix?.upgradeCode?.trim();

  if (!config.productName || !config.identifier || !binaryName || !version || !wixUpgradeCode) {
    throw new Error("Tauri package identity, version, or pinned WiX upgrade code is incomplete");
  }
  parsedSemver(version);
  if (version !== workspaceVersion) {
    throw new Error(`Tauri version ${version} differs from Cargo workspace version ${workspaceVersion}`);
  }

  return {
    productName: config.productName,
    identifier: config.identifier,
    version,
    binaryName,
    helperBinaries: cargoBinaryNames(manifest).filter((name) => name !== cargoName),
    wixUpgradeCode,
  };
}

export async function oneArtifact(directory, suffix, kind = "file") {
  const entries = (await readdir(directory, { withFileTypes: true })).filter((entry) => {
    const rightKind = kind === "directory" ? entry.isDirectory() : entry.isFile();
    return rightKind && entry.name.endsWith(suffix);
  });
  if (entries.length !== 1) {
    throw new Error(`expected exactly one ${suffix} in ${directory}, found ${entries.length}`);
  }
  return join(directory, entries[0].name);
}

export async function packageContract(platform, options = {}) {
  if (!PLATFORMS.has(platform)) {
    throw new Error("platform must be macos or windows");
  }

  const root = resolve(options.root ?? ROOT);
  const target = resolve(options.target ?? join(root, "target", "release"));
  const identity = await packageIdentity({ root });
  const { binaryName } = identity;

  const bundle = join(target, "bundle");
  const artifacts = platform === "macos"
    ? {
        app: await oneArtifact(join(bundle, "macos"), ".app", "directory"),
        dmg: await oneArtifact(join(bundle, "dmg"), ".dmg"),
      }
    : {
        msi: await oneArtifact(join(bundle, "msi"), ".msi"),
        nsis: await oneArtifact(join(bundle, "nsis"), "-setup.exe"),
      };

  const executable = platform === "macos" ? binaryName : `${binaryName}.exe`;
  const releaseBinary = join(target, executable);
  const appBinary = platform === "macos"
    ? join(artifacts.app, "Contents", "MacOS", executable)
    : undefined;
  const infoPlist = platform === "macos"
    ? join(artifacts.app, "Contents", "Info.plist")
    : undefined;

  return {
    platform,
    ...identity,
    executable,
    releaseBinary,
    appBinary,
    infoPlist,
    artifacts,
    stagedArtifacts: platform === "macos"
      ? {
          app: `${identity.productName}.app.zip`,
          dmg: basename(artifacts.dmg),
        }
      : {
          msi: basename(artifacts.msi),
          nsis: basename(artifacts.nsis),
        },
    migration: {
      legacyDirectory: ".zerocode/state",
      class: "local-data",
      artifact: MIGRATION_ARTIFACT,
      beforeBase64: fixtureBytes("native-package-smoke-before"),
      afterBase64: fixtureBytes("native-package-smoke-after"),
    },
  };
}

async function main() {
  const [platform, flag, field] = process.argv.slice(2);
  if (flag === "--artifact-name") {
    if (!field) throw new Error("release tag or unsigned channel is required");
    const identity = await packageIdentity();
    process.stdout.write(`${artifactUploadName(platform, field, identity.productName)}\n`);
    return;
  }
  if (flag === "--identity-field") {
    if (!field) throw new Error("identity field is required");
    const identity = await packageIdentity();
    const value = field.split(".").reduce((held, key) => held?.[key], identity);
    if (typeof value !== "string") throw new Error(`identity field is not a string: ${field}`);
    process.stdout.write(`${value}\n`);
    return;
  }
  const contract = await packageContract(platform);
  if (!flag) {
    process.stdout.write(`${JSON.stringify(contract)}\n`);
    return;
  }
  if (flag !== "--field" || !field) {
    throw new Error("usage: node scripts/package-contract.mjs <macos|windows> [--field path|--identity-field path|--artifact-name tag]");
  }
  const value = field.split(".").reduce((held, key) => held?.[key], contract);
  if (typeof value !== "string") throw new Error(`contract field is not a string: ${field}`);
  process.stdout.write(`${value}\n`);
}

if (resolve(process.argv[1] ?? "") === fileURLToPath(import.meta.url)) {
  await main();
}
