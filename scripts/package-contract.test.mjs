import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { chmod, cp, mkdtemp, mkdir, readdir, readFile, readlink, rm, stat, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  artifactUploadName,
  cargoPackageName,
  cargoBinaryNames,
  cargoWorkspaceVersion,
  compareSemver,
  oneArtifact,
  packageContract,
} from "./package-contract.mjs";
import {
  downloadPreviousPackage,
  selectBaselineArtifact,
  selectBaselineRun,
  verifyArchiveDigest,
} from "./package-baseline.mjs";
import { assertBundledHelpers, assertChromiumBundle } from "./check-package-artifacts.mjs";
import { CHROMIUM_FRAMEWORK, CHROMIUM_HELPERS, chromiumHelperPlist, stageChromium } from "./stage-chromium.mjs";
import { STAGE_MANIFEST, verifyStageManifest, writeStageManifest } from "./package-stage-manifest.mjs";

test("Cargo package identity comes only from the package section", () => {
  const toml = `[workspace]\nmembers = []\n\n[package]\nname = "actual-bin"\nversion = "1.0.0"\n\n[dependencies]\nname = "not-this"\n`;
  assert.equal(cargoPackageName(toml), "actual-bin");
  assert.throws(() => cargoPackageName("[workspace]\nname = \"wrong\"\n"), /package.*name/i);
  assert.equal(cargoWorkspaceVersion("[workspace.package]\nversion = \"1.2.3\"\n"), "1.2.3");
});

test("release ordering and artifact names are deterministic", () => {
  assert.equal(compareSemver("1.2.3-beta.2", "1.2.3-beta.10"), -1);
  assert.equal(compareSemver("1.2.3", "1.2.3-rc.1"), 1);
  assert.equal(artifactUploadName("macos", "v1.2.3", "ZeroCode"), "zerocode-release-macos-v1.2.3");
  assert.equal(artifactUploadName("windows", "unsigned", "ZeroCode"), "zerocode-unsigned-package-smoke-windows");
});

test("artifact lookup rejects ambiguous package output", async () => {
  const root = await mkdtemp(join(tmpdir(), "zerocode-package-contract-"));
  try {
    await writeFile(join(root, "one.dmg"), "one");
    assert.equal(await oneArtifact(root, ".dmg"), join(root, "one.dmg"));
    await writeFile(join(root, "two.dmg"), "two");
    await assert.rejects(oneArtifact(root, ".dmg"), /found 2/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("one contract resolves package identity, artifacts and migration fixtures", async () => {
  const root = await mkdtemp(join(tmpdir(), "zerocode-package-contract-"));
  try {
    const shell = join(root, "crates", "zerocode-shell");
    const target = join(root, "target", "release");
    await mkdir(shell, { recursive: true });
    await writeFile(
      join(shell, "tauri.conf.json"),
      JSON.stringify({ productName: "Fixture", identifier: "dev.fixture.app", version: "1.2.3" }),
    );
    await writeFile(
      join(shell, "tauri.windows.conf.json"),
      JSON.stringify({ bundle: { windows: { wix: { upgradeCode: "00000000-0000-0000-0000-000000000123" } } } }),
    );
    await writeFile(join(shell, "Cargo.toml"), `[package]\nname = "fixture-bin"\n`);
    await writeFile(join(root, "Cargo.toml"), `[workspace.package]\nversion = "1.2.3"\n`);

    await mkdir(join(target, "bundle", "macos", "Fixture.app"), { recursive: true });
    await mkdir(join(target, "bundle", "dmg"), { recursive: true });
    await writeFile(join(target, "bundle", "dmg", "Fixture.dmg"), "dmg");
    const mac = await packageContract("macos", { root, target });
    assert.equal(mac.executable, "fixture-bin");
    assert.equal(mac.version, "1.2.3");
    assert.equal(mac.artifacts.app, join(target, "bundle", "macos", "Fixture.app"));
    assert.equal(mac.infoPlist, join(mac.artifacts.app, "Contents", "Info.plist"));

    await mkdir(join(target, "bundle", "msi"), { recursive: true });
    await mkdir(join(target, "bundle", "nsis"), { recursive: true });
    await writeFile(join(target, "bundle", "msi", "Fixture.msi"), "msi");
    await writeFile(join(target, "bundle", "nsis", "Fixture-setup.exe"), "nsis");
    const windows = await packageContract("windows", { root, target });
    assert.equal(windows.executable, "fixture-bin.exe");
    assert.equal(windows.wixUpgradeCode, "00000000-0000-0000-0000-000000000123");
    assert.equal(windows.migration.legacyDirectory, ".zerocode/state");

    const before = JSON.parse(Buffer.from(windows.migration.beforeBase64, "base64"));
    const after = JSON.parse(Buffer.from(windows.migration.afterBase64, "base64"));
    assert.equal(before[0].id, "native-package-smoke-before");
    assert.equal(after[0].id, "native-package-smoke-after");
    assert.notDeepEqual(before, after);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("stage manifest binds identity, version, revision, filenames and bytes", async () => {
  const root = await mkdtemp(join(tmpdir(), "zerocode-package-stage-"));
  try {
    const shell = join(root, "crates", "zerocode-shell");
    const target = join(root, "target", "release");
    const stage = join(root, "stage");
    await mkdir(shell, { recursive: true });
    await writeFile(join(root, "Cargo.toml"), `[workspace.package]\nversion = "2.0.0"\n`);
    await writeFile(join(shell, "Cargo.toml"), `[package]\nname = "fixture-bin"\n`);
    await writeFile(
      join(shell, "tauri.conf.json"),
      JSON.stringify({ productName: "Fixture", identifier: "dev.fixture.app", version: "2.0.0" }),
    );
    await writeFile(
      join(shell, "tauri.windows.conf.json"),
      JSON.stringify({ bundle: { windows: { wix: { upgradeCode: "00000000-0000-0000-0000-000000000123" } } } }),
    );
    await mkdir(join(target, "bundle", "msi"), { recursive: true });
    await mkdir(join(target, "bundle", "nsis"), { recursive: true });
    await writeFile(join(target, "bundle", "msi", "Fixture_2.0.0_x64.msi"), "msi-source");
    await writeFile(join(target, "bundle", "nsis", "Fixture_2.0.0_x64-setup.exe"), "nsis-source");
    await mkdir(stage);
    await writeFile(join(stage, "Fixture_2.0.0_x64.msi"), "msi-stage");
    await writeFile(join(stage, "Fixture_2.0.0_x64-setup.exe"), "nsis-stage");

    const manifest = await writeStageManifest("windows", stage, {
      root,
      target,
      sourceRevision: "a".repeat(40),
    });
    assert.equal(manifest.version, "2.0.0");
    assert.equal(manifest.wixUpgradeCode, "00000000-0000-0000-0000-000000000123");
    assert.equal(
      (await verifyStageManifest("windows", stage, { root, target, revision: "a".repeat(40) })).version,
      "2.0.0",
    );
    const previous = { ...manifest, version: "1.9.9" };
    await writeFile(join(stage, STAGE_MANIFEST), `${JSON.stringify(previous)}\n`);
    assert.equal(
      (await verifyStageManifest("windows", stage, {
        root,
        target,
        previous: true,
        revision: "a".repeat(40),
      })).version,
      "1.9.9",
    );
    await writeFile(join(stage, manifest.artifacts.msi.file), "tampered");
    await assert.rejects(verifyStageManifest("windows", stage, { root, target, previous: true }), /digest differs/);
    assert.match(await readFile(join(stage, STAGE_MANIFEST), "utf8"), /"sha256"/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("baseline selection binds a successful release workflow and its artifact", () => {
  const expectedRun = {
    id: 77,
    workflow_id: 55,
    event: "push",
    status: "completed",
    conclusion: "success",
    head_branch: "v1.0.0",
    head_sha: "abc",
    path: ".github/workflows/verify.yml@main",
    run_attempt: 2,
    created_at: "2026-01-02T00:00:00Z",
  };
  assert.equal(selectBaselineRun([
    { ...expectedRun, id: 76, run_attempt: 1 },
    expectedRun,
    { ...expectedRun, id: 78, conclusion: "failure" },
    { ...expectedRun, id: 79, workflow_id: 99 },
  ], "v1.0.0", "abc", 55).id, 77);
  assert.throws(
    () => selectBaselineRun([{ ...expectedRun, conclusion: "failure" }], "v1.0.0", "abc", 55),
    /release blocker/,
  );

  const expected = {
    id: 2,
    name: "zerocode-release-macos-v1.0.0",
    expired: false,
    created_at: "2026-01-02T00:00:00Z",
    workflow_run: { id: 77, head_sha: "abc" },
  };
  const selected = selectBaselineArtifact([
    { ...expected, id: 1, created_at: "2026-01-01T00:00:00Z" },
    expected,
    { ...expected, id: 3, workflow_run: { id: 77, head_sha: "other" } },
    { ...expected, id: 4, workflow_run: { id: 88, head_sha: "abc" } },
  ], expected.name, "abc", 77);
  assert.equal(selected.id, 2);
  assert.throws(() => selectBaselineArtifact([], expected.name, "abc", 77), /release blocker/);

  const bytes = Buffer.from("artifact archive");
  verifyArchiveDigest(bytes, "sha256:7d1a555def1c8906081a38eaca77c08725e70ca2705cac9bcc6f3405612c4a58");
  assert.throws(() => verifyArchiveDigest(bytes, null), /digest is missing/);
  assert.throws(() => verifyArchiveDigest(bytes, `sha256:${"0".repeat(64)}`), /digest differs/);
});

test("the first real release tag seeds rather than inventing an upgrade baseline", async () => {
  const root = await mkdtemp(join(tmpdir(), "zerocode-package-baseline-"));
  try {
    const shell = join(root, "crates", "zerocode-shell");
    await mkdir(shell, { recursive: true });
    await writeFile(join(root, "Cargo.toml"), `[workspace.package]\nversion = "1.0.0"\n`);
    await writeFile(join(shell, "Cargo.toml"), `[package]\nname = "fixture-bin"\n`);
    await writeFile(
      join(shell, "tauri.conf.json"),
      JSON.stringify({ productName: "Fixture", identifier: "dev.fixture.app", version: "1.0.0" }),
    );
    await writeFile(
      join(shell, "tauri.windows.conf.json"),
      JSON.stringify({ bundle: { windows: { wix: { upgradeCode: "00000000-0000-0000-0000-000000000123" } } } }),
    );
    const runGit = (...args) => execFileSync("git", args, { cwd: root, encoding: "utf8" }).trim();
    runGit("init", "-q");
    runGit("config", "user.name", "Package Test");
    runGit("config", "user.email", "package-test@example.invalid");
    runGit("add", ".");
    runGit("commit", "-qm", "first release");
    runGit("tag", "v1.0.0");
    const revision = runGit("rev-parse", "HEAD");

    const baseline = await downloadPreviousPackage("macos", join(root, "unused.zip"), {
      root,
      currentTag: "v1.0.0",
      workflowRevision: revision,
    });
    assert.deepEqual(baseline, { available: false, currentTag: "v1.0.0" });
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});


test("every helper source is explicitly declared for the bundler", async () => {
  const shell = new URL("../crates/zerocode-shell/", import.meta.url);
  const manifest = await readFile(new URL("Cargo.toml", shell), "utf8");
  const binaries = cargoBinaryNames(manifest);
  assert.ok(binaries.includes(cargoPackageName(manifest)));
  for (const file of await readdir(new URL("src/bin/", shell))) {
    if (file.endsWith(".rs")) assert.ok(binaries.includes(file.slice(0, -3)), `${file} needs an explicit [[bin]]`);
  }
  assert.equal(new Set(binaries).size, binaries.length);
});

test("installed helper validation catches a missing, empty or non-executable picker", async () => {
  const root = await mkdtemp(join(tmpdir(), "zerocode-package-helpers-"));
  try {
    for (const suffix of ["", ".exe"]) {
      const main = join(root, `fixture-main${suffix}`);
      const helper = join(root, `fixture-pick${suffix}`);
      await assert.rejects(assertBundledHelpers(main, ["fixture-pick"]), /fixture-pick/);
      await writeFile(helper, "");
      await assert.rejects(assertBundledHelpers(main, ["fixture-pick"]), /empty package artifact/);
      await writeFile(helper, "fixture helper", { mode: 0o755 });
      await chmod(helper, 0o755);
      await assertBundledHelpers(main, ["fixture-pick"]);
      if (!suffix && process.platform !== "win32") {
        await chmod(helper, 0o644);
        await assert.rejects(assertBundledHelpers(main, ["fixture-pick"]), /not executable/);
      }
    }
  } finally { await rm(root, { recursive: true, force: true }); }
});


test("Chromium helpers have distinct identities and escaped plist values", () => {
  const identity = { identifier: "dev.fixture.app", version: "1.3.4" };
  const identifiers = new Set();
  for (const name of CHROMIUM_HELPERS) {
    const plist = chromiumHelperPlist(name, identity);
    assert.ok(plist.includes(`<key>CFBundleExecutable</key><string>${name}</string>`));
    assert.ok(plist.includes("<key>LSUIElement</key><true/>"));
    identifiers.add(plist.match(/<key>CFBundleIdentifier<\/key><string>([^<]+)<\/string>/)[1]);
  }
  assert.equal(identifiers.size, 5);
  assert.throws(() => chromiumHelperPlist("../other", identity), /unknown Chromium helper/);
  assert.ok(chromiumHelperPlist(CHROMIUM_HELPERS[0], { ...identity, version: "<&" }).includes("&lt;&amp;"));
});

async function chromiumFixture(root) {
  const cefDir = join(root, "cef");
  const framework = join(cefDir, CHROMIUM_FRAMEWORK);
  await mkdir(join(cefDir, "include"), { recursive: true });
  await mkdir(join(framework, "Versions", "A", "Resources"), { recursive: true });
  await writeFile(join(cefDir, "include", "cef_version.h"), '// Redistribution and use are permitted. WARRANTIES ARE DISCLAIMED.\n// ---------------------------------------------------------------------------\n#define CEF_VERSION "152.0.5+fixture"\n');
  await writeFile(join(cefDir, "CREDITS.html"), "fixture third-party notices");
  await writeFile(join(framework, "Versions", "A", "Chromium Embedded Framework"), "fixture native engine");
  await writeFile(join(framework, "Versions", "A", "Resources", "Info.plist"), "fixture framework metadata");
  await symlink("A", join(framework, "Versions", "Current"));
  await symlink("Versions/Current/Chromium Embedded Framework", join(framework, "Chromium Embedded Framework"));
  await symlink("Versions/Current/Resources", join(framework, "Resources"));
  const helper = join(root, "zerocode-cef-helper");
  await writeFile(helper, "fixture sandbox helper");
  return {
    cefDir, helper, destination: join(root, "stage"), expectedVersion: "152.0.5",
    identity: { identifier: "dev.fixture.app", version: "1.3.4" },
  };
}

test("Chromium staging preserves framework links, ships five helpers and rejects broken bundles", { skip: process.platform === "win32" }, async () => {
  const root = await mkdtemp(join(tmpdir(), "zerocode-chromium-package-"));
  try {
    const fixture = await chromiumFixture(root);
    const result = await stageChromium(fixture);
    assert.equal(result.cefVersion, "152.0.5+fixture");
    assert.equal(await readlink(join(fixture.destination, "Frameworks", CHROMIUM_FRAMEWORK, "Resources")), "Versions/Current/Resources");
    for (const name of CHROMIUM_HELPERS) {
      assert.ok((await stat(join(fixture.destination, "Frameworks", `${name}.app`, "Contents", "MacOS", name))).mode & 0o111);
    }
    await stageChromium(fixture);
    assert.equal((await readdir(root)).filter((name) => name.startsWith(".chromium-stage-")).length, 0);
    const app = join(root, "Fixture.app");
    await mkdir(join(app, "Contents", "Resources", "chromium"), { recursive: true });
    await cp(join(fixture.destination, "Frameworks"), join(app, "Contents", "Frameworks"), { recursive: true, verbatimSymlinks: true });
    for (const name of ["CREDITS.html", "CEF-LICENSE.txt", "stage.json"]) {
      await cp(join(fixture.destination, name), join(app, "Contents", "Resources", "chromium", name));
    }
    await assertChromiumBundle(app, fixture.identity);
    const renderer = join(app, "Contents", "Frameworks", "ZeroCode Helper (Renderer).app", "Contents", "MacOS", "ZeroCode Helper (Renderer)");
    await rm(renderer);
    await assert.rejects(assertChromiumBundle(app, fixture.identity), /Renderer/);
  } finally { await rm(root, { recursive: true, force: true }); }
});

test("Chromium staging rejects a mismatched distribution and never replaces foreign data", { skip: process.platform === "win32" }, async () => {
  const root = await mkdtemp(join(tmpdir(), "zerocode-chromium-package-"));
  try {
    const fixture = await chromiumFixture(root);
    await assert.rejects(stageChromium({ ...fixture, expectedVersion: "151.0.0" }), /does not match pinned/);
    await mkdir(fixture.destination);
    await writeFile(join(fixture.destination, "stage.json"), JSON.stringify({ generator: "somebody-else" }));
    await writeFile(join(fixture.destination, "keep"), "not ours");
    await assert.rejects(stageChromium(fixture), /refusing to replace unowned/);
    assert.equal(await readFile(join(fixture.destination, "keep"), "utf8"), "not ours");
    await rm(join(fixture.cefDir, "CREDITS.html"));
    await assert.rejects(stageChromium(fixture), /CREDITS/);
  } finally { await rm(root, { recursive: true, force: true }); }
});

test("release packaging carries Chromium runtime, helpers, notices and JIT entitlement", async () => {
  const config = JSON.parse(await readFile(new URL("../crates/zerocode-shell/tauri.release.conf.json", import.meta.url), "utf8"));
  assert.ok(config.bundle.macOS.frameworks.some((path) => path.endsWith(CHROMIUM_FRAMEWORK)));
  for (const name of CHROMIUM_HELPERS) {
    assert.equal(config.bundle.macOS.files[`Frameworks/${name}.app`], `bin/chromium/Frameworks/${name}.app`);
  }
  for (const name of ["CREDITS.html", "CEF-LICENSE.txt", "stage.json"]) {
    assert.equal(config.bundle.macOS.files[`Resources/chromium/${name}`], `bin/chromium/${name}`);
  }
  const entitlement = await readFile(new URL(`../crates/zerocode-shell/${config.bundle.macOS.entitlements}`, import.meta.url), "utf8");
  assert.ok(entitlement.includes("com.apple.security.cs.allow-jit"));
  assert.ok(!entitlement.includes("disable-library-validation"));
  const pkg = JSON.parse(await readFile(new URL("../package.json", import.meta.url), "utf8"));
  assert.ok(pkg.scripts["tauri:release"].indexOf("stage-chromium.mjs") < pkg.scripts["tauri:release"].indexOf("tauri build"));
});

test("binary smoke checks distinguish source references from root test routes", async () => {
  const { assertEmbeddedRoutes, requiredRoutes } = await import("./check-package-artifacts.mjs");
  const root = await mkdtemp(join(tmpdir(), "zerocode-routes-"));
  const binary = join(root, "window");
  try {
    const required = requiredRoutes.join("\0");
    await writeFile(binary, `${required}\0See ui/tests/settings.mjs and ui/tests/window.mjs for verification.`);
    await assertEmbeddedRoutes(binary);
    await writeFile(binary, `${required}\0/tests/window.mjs\0`);
    await assert.rejects(assertEmbeddedRoutes(binary), /shipped forbidden/);
  } finally { await rm(root, { recursive: true, force: true }); }
});
