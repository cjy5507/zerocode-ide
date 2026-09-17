import assert from "node:assert/strict";
import test from "node:test";

import { createRunner, legacyOnlyName } from "./window-runner.mjs";

/* The window harness's runner, on its own (t-4017). What it promises the lane
 * and the coordinator's watch is a text contract — `^PASS `/`^FAIL ` lines,
 * `N/M passed`, an exit code — and what it promises the person is that one
 * suite throwing does not take the rest of the run, or the report, with it. */

const lines = () => {
  const said = [];
  return { said, log: (line) => said.push(line) };
};

test("a suite that throws is one FAIL line, and the suites after it still run and report", async () => {
  const { said, log } = lines();
  const runner = createRunner({ log, env: {}, argv: [] });
  const ran = [];
  runner.suite("first", async ({ ok }) => {
    ran.push("first");
    ok("first speaks", true, "1");
  });
  runner.suite("broken", async ({ ok }) => {
    ran.push("broken");
    ok("broken spoke once", true);
    throw new TypeError("withHelper[1] is undefined");
  });
  runner.suite("last", async ({ ok }) => {
    ran.push("last");
    ok("last speaks", true);
  });
  const { results, failed } = await runner.run({});
  assert.deepEqual(ran, ["first", "broken", "last"]);
  assert.equal(failed, 1);
  const thrown = results.find((one) => !one.pass);
  assert.match(thrown.name, /broken/);
  assert.match(thrown.detail, /TypeError: withHelper\[1\] is undefined/);
  // One line per result, whatever the stack looked like.
  assert.ok(!thrown.detail.includes("\n"));
  assert.equal(runner.report(), 1);
  assert.deepEqual(said.filter((line) => /^FAIL /.test(line)).length, 1);
  assert.equal(said.at(-1), "3/4 passed");
});

test("the report is the lane's text contract: PASS/FAIL lines, a blank, N/M passed", async () => {
  const { said, log } = lines();
  const runner = createRunner({ log, env: {}, argv: [] });
  runner.suite("one", async ({ ok }) => {
    ok("a check with detail", true, '{"seen":1}');
    ok("a check without detail", true);
    ok("a red check", false, "why");
  });
  await runner.run({});
  assert.equal(runner.report(), 1);
  assert.deepEqual(said, [
    'PASS  a check with detail  — {"seen":1}',
    "PASS  a check without detail",
    "FAIL  a red check  — why",
    "",
    "2/3 passed",
  ]);
});

test("the suite body sees the run's context plus ok and its own name", async () => {
  const runner = createRunner({ log() {}, env: {}, argv: [] });
  let seen = null;
  runner.suite("ctx", async (ctx) => {
    seen = ctx;
  });
  await runner.run({ browser: "B", origin: "http://o" });
  assert.equal(seen.browser, "B");
  assert.equal(seen.origin, "http://o");
  assert.equal(seen.suite, "ctx");
  assert.equal(typeof seen.ok, "function");
});

test("a default run takes every suite in registration order and skips the ones that run only by name", async () => {
  const runner = createRunner({ log() {}, env: {}, argv: [] });
  const ran = [];
  runner.suite("a", async () => ran.push("a"));
  runner.suite("focused", async () => ran.push("focused"), { onlyByName: true });
  runner.suite("b", async () => ran.push("b"));
  await runner.run({});
  assert.deepEqual(ran, ["a", "b"]);
});

test("WINDOW_SUITES chooses by exact name or pattern, and reaches a suite that runs only by name", async () => {
  const runner = createRunner({ log() {}, env: { WINDOW_SUITES: "task-board,focused,-waits$" }, argv: [] });
  const ran = [];
  for (const name of ["task-board", "autonomy-board", "board-waits", "window"]) {
    runner.suite(name, async () => ran.push(name));
  }
  runner.suite("focused", async () => ran.push("focused"), { onlyByName: true });
  await runner.run({});
  assert.deepEqual(ran, ["task-board", "board-waits", "focused"]);
});

test("--suite on the command line chooses the same way, repeatable", async () => {
  const runner = createRunner({ log() {}, env: {}, argv: ["--suite", "crash", "--suite=editor-.*"] });
  const ran = [];
  for (const name of ["crash", "editor-recovery", "editor-selection", "window"]) {
    runner.suite(name, async () => ran.push(name));
  }
  await runner.run({});
  assert.deepEqual(ran, ["crash", "editor-recovery", "editor-selection"]);
});

test("the older <NAME>_ONLY switches still choose their suite", async () => {
  assert.equal(legacyOnlyName("editor-recovery"), "EDITOR_RECOVERY_ONLY");
  assert.equal(legacyOnlyName("attach"), "ATTACH_ONLY");
  const runner = createRunner({ log() {}, env: { EDITOR_RECOVERY_ONLY: "1" }, argv: [] });
  const ran = [];
  for (const name of ["crash", "editor-recovery", "window"]) {
    runner.suite(name, async () => ran.push(name));
  }
  await runner.run({});
  assert.deepEqual(ran, ["editor-recovery"]);
});

test("a chosen name that matches no suite is a FAIL, not a quiet 0/0", async () => {
  const { said, log } = lines();
  const runner = createRunner({ log, env: { WINDOW_SUITES: "wrokers" }, argv: [] });
  runner.suite("workers", async ({ ok }) => ok("never", true));
  const { results, failed } = await runner.run({});
  assert.equal(failed, 1);
  assert.equal(results.length, 1);
  assert.match(results[0].name, /wrokers/);
  runner.report();
  assert.equal(said.at(-1), "0/1 passed");
});

test("two suites cannot share a name", () => {
  const runner = createRunner({ log() {}, env: {}, argv: [] });
  runner.suite("twice", async () => {});
  assert.throws(() => runner.suite("twice", async () => {}), /twice/);
});
