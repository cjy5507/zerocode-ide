import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { mkdtemp, mkdir, realpath, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { promisify } from "node:util";
import { inspectWorkerCheckout } from "../real-runner.mjs";

const execute = promisify(execFile);
async function call(file, args) {
  try { return { rc: 0, ...await execute(file, args, { encoding: "utf8" }) }; }
  catch (error) { return { rc: error.code ?? null, stdout: error.stdout ?? "", stderr: error.stderr ?? error.message }; }
}

test("checkout proof distinguishes actual Git worktrees without a provider or compiler", async t => {
  const parent = await mkdtemp(join(tmpdir(), "checkout-proof-"));
  const main = join(parent, "main");
  const runner = join(parent, "runner");
  const worker = join(parent, "worker");
  const foreign = join(parent, "foreign");
  const foreignWorker = join(parent, "foreign-worker");
  const hooks = join(parent, "empty-hooks");
  await mkdir(hooks);
  const git = async args => {
    const result = await call("git", args);
    assert.equal(result.rc, 0, result.stderr);
  };
  const init = async directory => {
    await git(["init", "-q", directory]);
    await git(["-C", directory, "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
      "-c", "commit.gpgsign=false", "-c", `core.hooksPath=${hooks}`, "commit", "--allow-empty", "-qm", "Fixture"]);
  };
  try {
    await init(main);
    await git(["-C", main, "worktree", "add", "--detach", "-q", runner]);
    await git(["-C", main, "worktree", "add", "--detach", "-q", worker]);
    const expected = { root: await realpath(runner), common: await realpath(join(main, ".git")) };
    await t.test("a separate linked checkout in the same repository is accepted", async () => {
      const found = await inspectWorkerCheckout(worker, expected, call);
      assert.equal(found.root, await realpath(worker));
      assert.equal(found.common, expected.common);
    });
    await t.test("the main checkout is refused", () => assert.rejects(inspectWorkerCheckout(main, expected, call), /main checkout/));
    await t.test("the runner's own linked checkout is refused", () => assert.rejects(inspectWorkerCheckout(runner, expected, call), /runner's own/));
    await t.test("a folder nested inside a worktree is refused", async () => {
      const nested = join(worker, "nested");
      await mkdir(nested);
      await assert.rejects(inspectWorkerCheckout(nested, expected, call), /not the worktree root/);
    });
    await t.test("a missing checkout report is refused", () => assert.rejects(inspectWorkerCheckout(null, expected, call), /not reported/));
    await t.test("a linked checkout from another repository is refused", async () => {
      await init(foreign);
      await git(["-C", foreign, "worktree", "add", "--detach", "-q", foreignWorker]);
      await assert.rejects(inspectWorkerCheckout(foreignWorker, expected, call), /another repository/);
    });
  } finally {
    await rm(parent, { recursive: true, force: true });
  }
});
