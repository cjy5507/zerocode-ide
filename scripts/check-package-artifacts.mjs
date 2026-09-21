import { readFile, stat } from "node:fs/promises";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { packageContract, packageIdentity } from "./package-contract.mjs";
import { CHROMIUM_FRAMEWORK, CHROMIUM_HELPERS, chromiumHelperPlist } from "./stage-chromium.mjs";

async function nonempty(path) {
  const held = await stat(path);
  if (!held.isFile() || held.size === 0) throw new Error(`empty package artifact: ${path}`);
}

export const requiredRoutes = [
  "/favicon.ico",
  "/favicon.png",
  "/index.html",
  "/shell-boot.js",
  "/shell-browser.js",
  "/shell-doc.js",
  "/shell-i18n.js",
  "/shell-input.js",
  "/shell-knowledge.js",
  "/shell-remote.js",
  "/shell-sftp.js",
  "/shell-scm.js",
  "/shell-settings.js",
  "/shell-flow.js",
  "/shell-status.js",
  "/shell-term.js",
  "/shell-workspace.js",
  "/shell.css",
  "/shell.js",
  "/tokens.css",
  "/vendor/cm6-LICENSE",
  "/vendor/cm6.js",
];

const forbiddenRoutes = [
  "/.DS_Store",
  "/prototype/",
  "/tests/",
  "/vendor/build-cm6.mjs",
];

export async function assertEmbeddedRoutes(path, label = "release binary") {
  await nonempty(path);
  const bytes = await readFile(path);
  for (const route of requiredRoutes) {
    if (!bytes.includes(Buffer.from(route))) throw new Error(`${label} is missing ${route}`);
  }
  for (const route of forbiddenRoutes) {
    // Built-in skills cite source paths such as ui/tests/settings.mjs. Those
    // are documentation, not root asset keys. The exact dist allowlist is
    // checked separately by build-ui-dist; this is a binary smoke check.
    const needle = Buffer.from(route);
    for (let at = bytes.indexOf(needle); at !== -1; at = bytes.indexOf(needle, at + 1)) {
      const previous = at > 0 ? String.fromCharCode(bytes[at - 1]) : "";
      if (previous && /[A-Za-z0-9_./\\-]/.test(previous)) continue;
      throw new Error(`${label} shipped forbidden ${route}`);
    }
  }
}

// The manifest is the list the bundler must carry, not only the main binary.
export async function assertBundledHelpers(binaryPath, names = null) {
  const helpers = names ?? (await packageIdentity()).helperBinaries;
  const suffix = binaryPath.toLowerCase().endsWith(".exe") ? ".exe" : "";
  for (const name of helpers) {
    const path = join(dirname(binaryPath), name + suffix);
    await nonempty(path);
    if (!suffix && process.platform !== "win32" && ((await stat(path)).mode & 0o111) === 0) {
      throw new Error(`package helper is not executable: ${path}`);
    }
  }
}

export async function assertChromiumBundle(app, identity) {
  const frameworks = join(app, "Contents", "Frameworks");
  await nonempty(join(frameworks, CHROMIUM_FRAMEWORK, "Chromium Embedded Framework"));
  await nonempty(join(frameworks, CHROMIUM_FRAMEWORK, "Resources", "Info.plist"));
  for (const name of CHROMIUM_HELPERS) {
    const contents = join(frameworks, `${name}.app`, "Contents");
    const executable = join(contents, "MacOS", name);
    await assertBundledHelpers(executable, [name]);
    const info = await readFile(join(contents, "Info.plist"), "utf8");
    const expected = chromiumHelperPlist(name, identity);
    for (const key of ["CFBundleExecutable", "CFBundleIdentifier", "CFBundleVersion", "LSUIElement"]) {
      const field = expected.match(new RegExp(`<key>${key}</key>(?:<string>[^<]*</string>|<true/>)`))?.[0];
      if (!field || !info.includes(field)) throw new Error(`Chromium helper ${key} is invalid: ${contents}`);
    }
  }
  const resources = join(app, "Contents", "Resources", "chromium");
  await nonempty(join(resources, "CREDITS.html"));
  await nonempty(join(resources, "CEF-LICENSE.txt"));
  const stage = JSON.parse(await readFile(join(resources, "stage.json"), "utf8"));
  if (stage.generator !== "zerocode-chromium-stage-v1" || stage.appVersion !== identity.version || !stage.cefVersion) {
    throw new Error("Chromium staging identity does not match the app bundle");
  }
}

export async function assertAppBundle(app) {
  const identity = await packageIdentity();
  const binary = join(app, "Contents", "MacOS", identity.binaryName);
  await nonempty(binary);
  await assertBundledHelpers(binary, identity.helperBinaries);
  await assertChromiumBundle(app, identity);
  const helper = join(app, "Contents", "Resources", "ZeroCode Computer Use.app");
  const helperBinary = join(helper, "Contents", "MacOS", "zerocode-computer-use-macos");
  const helperPlist = join(helper, "Contents", "Info.plist");
  await nonempty(helperBinary);
  await nonempty(helperPlist);
  const info = await readFile(helperPlist, "utf8");
  if (!info.includes("dev.zerocode.app.computer-use") || !info.includes("<key>LSUIElement</key><true/>")) {
    throw new Error(`Computer Use helper identity is invalid: ${helperPlist}`);
  }
  await assertPrivacyUsageDescriptions(app);
}

/* t-5587: the window raises macOS's own microphone and camera dialogs for the
 * 「macOS 권한」 page. macOS terminates any process that asks without the
 * matching usage description, so the merge of crates/zerocode-shell/Info.plist
 * is not cosmetic — if it silently stopped happening, the button would kill
 * the window. Proven here on the built bundle, not on the source file. */
async function assertPrivacyUsageDescriptions(app) {
  const plist = join(app, "Contents", "Info.plist");
  const info = await readFile(plist, "utf8");
  for (const key of ["NSMicrophoneUsageDescription", "NSCameraUsageDescription"]) {
    if (!new RegExp(`<key>${key}</key>\\s*<string>[^<]+</string>`).test(info)) {
      throw new Error(`${key} did not reach the bundle: ${plist}`);
    }
  }
}

async function main() {
  const [platform, binaryPath] = process.argv.slice(2);
  if (platform === "--app-bundle" && binaryPath) {
    await assertAppBundle(resolve(binaryPath));
    console.log(`checked app bundle and helper executables: ${binaryPath}`);
    return;
  }
  if (platform === "--helpers" && binaryPath) {
    await assertBundledHelpers(resolve(binaryPath));
    return;
  }
  if (platform === "--binary" && binaryPath) {
    await assertEmbeddedRoutes(resolve(binaryPath), "installed package binary");
    return;
  }
  if (platform !== "macos" && platform !== "windows") {
    throw new Error("usage: node scripts/check-package-artifacts.mjs <macos|windows>|--binary path");
  }

  const contract = await packageContract(platform);
  await assertEmbeddedRoutes(contract.releaseBinary);
  await assertBundledHelpers(contract.releaseBinary);

  const artifacts = Object.values(contract.artifacts);
  if (platform === "macos") {
    await nonempty(contract.infoPlist);
    await assertEmbeddedRoutes(contract.appBinary, "macOS app binary");
    await assertAppBundle(contract.artifacts.app);
  }
  for (const artifact of artifacts) {
    if ((await stat(artifact)).isFile()) await nonempty(artifact);
  }
  for (const [kind, path] of Object.entries(contract.artifacts)) {
    if (kind === "app") continue;
    const escaped = contract.version.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    if (!new RegExp(`(^|\\D)${escaped}(\\D|$)`).test(basename(path))) {
      throw new Error(`${kind} filename does not carry package version ${contract.version}: ${path}`);
    }
  }

  console.log(`checked ${platform} package: ${artifacts.join(", ")}`);
}

if (resolve(process.argv[1] ?? "") === fileURLToPath(import.meta.url)) {
  await main();
}
