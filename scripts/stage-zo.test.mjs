import assert from "node:assert/strict";
import { mkdtemp, mkdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { builtZo, stageZo } from "./stage-zo.mjs";

function artifact(executable, name = "zo") {
  return JSON.stringify({ reason: "compiler-artifact", target: { name, kind: ["bin"] }, executable });
}

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
