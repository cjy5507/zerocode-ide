import { spawnSync } from "node:child_process";
import { chmod, copyFile, cp, mkdir, mkdtemp, readFile, rename, rm, stat, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { ROOT, packageIdentity } from "./package-contract.mjs";
import { signForDistribution } from "./native-signing.mjs";

export const CHROMIUM_FRAMEWORK = "Chromium Embedded Framework.framework";
export const CHROMIUM_HELPERS = [
  "ZeroCode Helper",
  "ZeroCode Helper (GPU)",
  "ZeroCode Helper (Renderer)",
  "ZeroCode Helper (Plugin)",
  "ZeroCode Helper (Alerts)",
];
const GENERATOR = "zerocode-chromium-stage-v1";

function xml(value) {
  return String(value).replace(/[&<>"']/g, (character) => ({
    "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&apos;",
  })[character]);
}

export function chromiumHelperPlist(name, identity) {
  if (!CHROMIUM_HELPERS.includes(name)) throw new Error(`unknown Chromium helper: ${name}`);
  const suffix = name === CHROMIUM_HELPERS[0] ? "helper" : `helper.${name.match(/\(([^)]+)\)/)[1].toLowerCase()}`;
  return `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleExecutable</key><string>${xml(name)}</string>
  <key>CFBundleName</key><string>${xml(name)}</string>
  <key>CFBundleIdentifier</key><string>${xml(identity.identifier)}.${suffix}</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleVersion</key><string>${xml(identity.version)}</string>
  <key>CFBundleShortVersionString</key><string>${xml(identity.version)}</string>
  <key>LSUIElement</key><true/>
  <key>LSMinimumSystemVersion</key><string>14.0</string>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
  <key>LSEnvironment</key><dict><key>MallocNanoZone</key><string>0</string></dict>
</dict></plist>
`;
}

async function nonempty(path) {
  const info = await stat(path);
  if (!info.isFile() || info.size === 0) throw new Error(`missing Chromium runtime file: ${path}`);
}

export async function stageChromium({ cefDir, helper, destination, identity, expectedVersion }) {
  const header = await readFile(join(cefDir, "include", "cef_version.h"), "utf8");
  const version = header.match(/^#define CEF_VERSION "([^"]+)"/m)?.[1];
  if (!version || version.split("+")[0] !== expectedVersion) {
    throw new Error(`CEF distribution ${version ?? "unknown"} does not match pinned ${expectedVersion}`);
  }
  await Promise.all([
    nonempty(helper),
    nonempty(join(cefDir, CHROMIUM_FRAMEWORK, "Chromium Embedded Framework")),
    nonempty(join(cefDir, CHROMIUM_FRAMEWORK, "Resources", "Info.plist")),
    nonempty(join(cefDir, "CREDITS.html")),
  ]);
  const license = header.split("// ---------------------------------------------------------------------------")[0];
  if (!license.includes("Redistribution and use") || !license.includes("DISCLAIMED")) {
    throw new Error("CEF distribution license is missing from its version header");
  }
  const exists = await stat(destination).catch((error) => {
    if (error.code === "ENOENT") return null;
    throw error;
  });
  if (exists) {
    const marker = JSON.parse(await readFile(join(destination, "stage.json"), "utf8"));
    if (marker.generator !== GENERATOR) throw new Error(`refusing to replace unowned directory: ${destination}`);
  }
  await mkdir(dirname(destination), { recursive: true });
  const temporary = await mkdtemp(join(dirname(destination), ".chromium-stage-"));
  const previous = `${temporary}.previous`;
  let movedPrevious = false;
  try {
    const frameworks = join(temporary, "Frameworks");
    await mkdir(frameworks);
    // Framework symlinks must stay relative: the installed app cannot refer to the build machine.
    await cp(join(cefDir, CHROMIUM_FRAMEWORK), join(frameworks, CHROMIUM_FRAMEWORK), {
      recursive: true, verbatimSymlinks: true,
    });
    for (const name of CHROMIUM_HELPERS) {
      const contents = join(frameworks, `${name}.app`, "Contents");
      await mkdir(join(contents, "MacOS"), { recursive: true });
      const executable = join(contents, "MacOS", name);
      await copyFile(helper, executable);
      await chmod(executable, 0o755);
      await writeFile(join(contents, "Info.plist"), chromiumHelperPlist(name, identity));
      signForDistribution(dirname(contents), {
        entitlements: join(ROOT, "crates", "zerocode-shell", "chromium.entitlements.plist"),
      });
    }
    await copyFile(join(cefDir, "CREDITS.html"), join(temporary, "CREDITS.html"));
    await writeFile(join(temporary, "CEF-LICENSE.txt"), license.replace(/^\/\/ ?/gm, ""));
    await writeFile(join(temporary, "stage.json"), `${JSON.stringify({ generator: GENERATOR, cefVersion: version, appVersion: identity.version }, null, 2)}\n`);
    if (exists) {
      await rename(destination, previous);
      movedPrevious = true;
    }
    await rename(temporary, destination);
  } catch (error) {
    if (movedPrevious) await rename(previous, destination);
    throw error;
  } finally {
    await rm(temporary, { recursive: true, force: true });
  }
  if (movedPrevious) await rm(previous, { recursive: true });
  return { destination, cefVersion: version };
}

async function buildHelper() {
  const profile = process.env.CEF_STAGE_PROFILE ?? "release";
  const built = spawnSync("cargo", [
    "build", "--locked", "--profile", profile, "-p", "zerocode-shell",
    "--bin", "zerocode-cef-helper", "--message-format=json",
  ], { cwd: ROOT, encoding: "utf8", stdio: ["ignore", "pipe", "inherit"], maxBuffer: 64 * 1024 * 1024 });
  if (built.error) throw built.error;
  if (built.status !== 0) throw new Error(`Chromium helper build failed (${built.status})`);
  const messages = built.stdout.split("\n").filter((line) => line.startsWith("{")).map((line) => JSON.parse(line));
  const helper = messages.findLast((message) => message.reason === "compiler-artifact" && message.target?.name === "zerocode-cef-helper" && message.executable)?.executable;
  const paths = messages.filter((message) => message.reason === "build-script-executed" && message.package_id.includes("cef-dll-sys"))
    .flatMap((message) => message.linked_paths ?? []).map((path) => path.replace(/^native=/, ""));
  for (const cefDir of paths) {
    if ((await stat(join(cefDir, "archive.json")).catch(() => null))?.isFile() && helper) return { cefDir, helper };
  }
  throw new Error("Cargo did not report the built CEF helper and its matching distribution");
}

async function main() {
  if (process.platform !== "darwin") return;
  const manifest = await readFile(join(ROOT, "crates", "zerocode-shell", "Cargo.toml"), "utf8");
  const pinned = manifest.match(/^cef\s*=\s*\{[^\n]*version\s*=\s*"=([^"+]+)\+([^\"]+)"/m)?.[2];
  if (!pinned) throw new Error("CEF must be pinned to an exact Chromium distribution");
  const built = await buildHelper();
  const result = await stageChromium({
    ...built,
    identity: await packageIdentity(),
    destination: join(ROOT, "crates", "zerocode-shell", "bin", "chromium"),
    expectedVersion: pinned,
  });
  console.log(`staged Chromium ${result.cefVersion}: ${result.destination}`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((error) => { console.error(error.message); process.exitCode = 1; });
}
