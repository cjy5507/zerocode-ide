import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { copyFile, mkdtemp, mkdir, readFile, rm, stat, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { builtZo, stageZo } from "./stage-zo.mjs";

function artifact(executable, name = "zo") {
  return JSON.stringify({ reason: "compiler-artifact", target: { name, kind: ["bin"] }, executable });
}

test("the CLI runs through a linked path instead of silently skipping staging", async () => {
  const root = await mkdtemp(join(tmpdir(), "zo-stage-entry-"));
  try {
    const scripts = join(root, "scripts");
    await mkdir(scripts);
    for (const name of ["stage-zo.mjs", "native-signing.mjs", "package-contract.mjs"]) {
      await copyFile(new URL(name, import.meta.url), join(scripts, name));
    }
    const linked = join(root, "linked-scripts");
    await symlink(scripts, linked, process.platform === "win32" ? "junction" : "dir");
    // A missing input must be refused by the real CLI, without invoking Cargo.
    const result = spawnSync(process.execPath, [join(linked, "stage-zo.mjs")], {
      encoding: "utf8", env: { ...process.env, ZO_STAGE_BIN: join(root, "missing-zo") },
    });
    assert.equal(result.status, 1, result.stderr);
    assert.match(result.stderr, /no zo binary at/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("zo staging copies the exact Windows executable Cargo reported", async () => {
  const root = await mkdtemp(join(tmpdir(), "zo-stage-windows-"));
  try {
    const executable = join(root, "custom-target", "custom-profile", "zo.exe");
    await mkdir(join(root, "custom-target", "custom-profile"), { recursive: true });
    await writeFile(executable, "windows-executable-fixture");
    await writeFile(join(root, "Cargo.toml"), '[workspace.package]\nversion = "1.2.3"\n');
    const calls = [];
    const run = (file, args) => {
      calls.push({ file, args });
      if (file === "cargo") return { status: 0, stdout: artifact(executable) + "\n" };
      if (file === executable) return { status: 0, stdout: "zo 1.2.3\n" };
      throw new Error(`unexpected program: ${file}`);
    };
    const staged = await stageZo({ root, env: { ZO_STAGE_PROFILE: "custom-profile" }, run, sign: () => false });
    assert.equal(staged.source, executable);
    assert.equal(await readFile(staged.destination, "utf8"), "windows-executable-fixture");
    assert.equal(calls.filter(call => call.file === "cargo").length, 1);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("zo build refuses success without its executable artifact", async () => {
  await assert.rejects(builtZo({ run: () => ({ status: 0, stdout: artifact("/fixture/other", "other") }) }), /executable/i);
});

test("zo build reports a failed Cargo spawn", async () => {
  await assert.rejects(builtZo({ run: () => ({ error: new Error("cargo unavailable"), status: null }) }), /cargo unavailable/);
});

test("zo build uses a cached Unix binary and rejects ambiguous output", async () => {
  const executable = "/fixture/target/release/zo";
  assert.equal(await builtZo({ run: () => ({ status: 0, stdout: artifact(executable) + "\n" + artifact(executable) }) }), executable);
  await assert.rejects(builtZo({ run: () => ({ status: 0, stdout: artifact(executable) + "\n" + artifact("/fixture/another/zo") }) }), /exactly one/);
});

test("failed signing leaves the prior staged binary intact and removes temporary bytes", async () => {
  const root = await mkdtemp(join(tmpdir(), "zo-stage-signing-"));
  try {
    const executable = join(root, "zo.exe");
    const destination = join(root, "crates", "zerocode-shell", "bin", "zo");
    await mkdir(join(root, "crates", "zerocode-shell", "bin"), { recursive: true });
    await writeFile(join(root, "Cargo.toml"), '[workspace.package]\nversion = "1.2.3"\n');
    await writeFile(executable, "new");
    await writeFile(destination, "previous");
    await assert.rejects(stageZo({
      root, env: { ZO_STAGE_BIN: executable },
      run: () => ({ status: 0, stdout: "zo 1.2.3\n" }),
      sign: () => { throw new Error("signer refused"); },
    }), /signer refused/);
    assert.equal(await readFile(destination, "utf8"), "previous");
    assert.equal(await stat(`${destination}.new`).then(() => true, () => false), false);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
