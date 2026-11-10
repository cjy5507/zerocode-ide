import assert from "node:assert/strict";
import test from "node:test";

import { spawn } from "node:child_process";

import { END_GRACE_MS } from "./end-run.mjs";
import { createRunner, legacyOnlyName, REPORT } from "./window-runner.mjs";

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

test("the report is the lane's text contract: each suite's name over its PASS/FAIL lines, a blank, N/M passed", async () => {
  const { said, log } = lines();
  const runner = createRunner({ log, env: {}, argv: [] });
  runner.suite("one", async ({ ok }) => {
    ok("a check with detail", true, '{"seen":1}');
    ok("a check without detail", true);
    ok("a red check", false, "why");
  });
  runner.suite("two", async ({ ok }) => {
    ok("a check of the second suite", true);
  });
  await runner.run({});
  assert.equal(runner.report(), 1);
  assert.deepEqual(said, [
    "SUITE  one",
    'PASS  a check with detail  — {"seen":1}',
    "PASS  a check without detail",
    "FAIL  a red check  — why",
    "SUITE  two",
    "PASS  a check of the second suite",
    "",
    "3/4 passed",
  ]);
  // Spelled in one table: the lane's parser and this report read the same words.
  assert.deepEqual(REPORT, { pass: "PASS", fail: "FAIL", suite: "SUITE", gap: "  ", detail: "  — " });
  assert.ok(Object.isFrozen(REPORT));
});

test("a check keeps the suite that made it — one that lands after its suite has ended stands under its own name again, and one made outside every suite comes first, under none", async () => {
  const { said, log } = lines();
  const runner = createRunner({ log, env: {}, argv: [] });
  let late = null;
  runner.suite("early", async ({ ok }) => {
    ok("early speaks", true);
    late = ok;
  });
  runner.suite("next", async ({ ok }) => {
    ok("next speaks", true);
    // The first suite's own hands, still held, speak while this one runs.
    late("early speaks late", false, "late");
  });
  await runner.run({});
  runner.ok("said outside every suite", false, "loose");
  runner.report();
  assert.deepEqual(said.slice(0, -2), [
    "FAIL  said outside every suite  — loose",
    "SUITE  early",
    "PASS  early speaks",
    "SUITE  next",
    "PASS  next speaks",
    "SUITE  early",
    "FAIL  early speaks late  — late",
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

test("a run keeps each suite's wall time and its checks beside the report, in the order it took them", async () => {
  const { said, log } = lines();
  const runner = createRunner({ log, env: {}, argv: [] });
  runner.suite("quick", async ({ ok }) => ok("quick speaks", true));
  runner.suite("slow", async ({ ok }) => {
    await new Promise((done) => setTimeout(done, 40));
    ok("slow speaks", true);
    ok("slow speaks again", true);
  });
  runner.suite("broken", async () => {
    throw new Error("no");
  });
  await runner.run({});
  // A suite that threw took its time too, and its one line is its FAIL.
  assert.deepEqual(runner.times.map((one) => [one.suite, one.checks]), [["quick", 1], ["slow", 2], ["broken", 1]]);
  assert.ok(runner.times[1].ms >= 30, `the slow suite's ${runner.times[1].ms} ms`);
  assert.ok(runner.times[0].ms < runner.times[1].ms, JSON.stringify(runner.times));
  // The report is the lane's text contract: no time is written into it.
  runner.report();
  assert.equal(said.at(-1), "3/4 passed");
  assert.deepEqual(said.filter((line) => line.startsWith(REPORT.suite)), ["SUITE  quick", "SUITE  slow", "SUITE  broken"]);
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

/* A report larger than a pipe holds reaches a slow reader whole, and the exit
 * code still says it failed (end-run.mjs). The public CI's macOS leg cut the
 * window suite's report at the pipe's 64 KiB because the harness called
 * process.exit() with the rest still queued; the reader here sleeps first so
 * the pipe fills the same way. */
test("a report bigger than the pipe reaches a slow reader whole and the run still fails", async () => {
  const lines = 2400;
  const script = [
    `import { endRun } from ${JSON.stringify(new URL("./end-run.mjs", import.meta.url).href)};`,
    `const pad = "x".repeat(400);`,
    `for (let i = 0; i < ${lines}; i += 1) console.log("PASS  check " + i + "  — " + pad);`,
    `console.log("");`,
    `console.log("${lines}/${lines} passed");`,
    `endRun(1);`,
  ].join("\n");
  const started = Date.now();
  const child = spawn(process.execPath, ["--input-type=module", "-e", script], { stdio: ["ignore", "pipe", "inherit"] });
  const closed = new Promise((resolve) => child.on("close", resolve));
  child.stdout.pause();
  await new Promise((resolve) => setTimeout(resolve, 1500));
  const chunks = [];
  child.stdout.on("data", (chunk) => chunks.push(chunk));
  child.stdout.resume();
  const code = await closed;
  const said = Buffer.concat(chunks).toString("utf8").trimEnd().split("\n");
  assert.equal(said.filter((line) => line.startsWith("PASS  ")).length, lines);
  assert.equal(said.at(-1), `${lines}/${lines} passed`);
  assert.equal(code, 1);
  assert.ok(Date.now() - started < END_GRACE_MS, "the run ended when its writes were done, not by the grace");
});

test("a suite that never ends is one FAIL line naming how far it got, and the suites after it still run", async () => {
  const { log } = lines();
  const runner = createRunner({ log, env: { WINDOW_SUITE_STALL_MS: "40" }, argv: [] });
  runner.suite("stuck", async ({ ok }) => {
    ok("stuck spoke once", true);
    await new Promise(() => {});
  });
  runner.suite("last", async ({ ok }) => ok("last speaks", true));
  const { results, failed } = await runner.run({});
  assert.equal(failed, 1);
  const overdue = results.find((one) => !one.pass);
  assert.equal(overdue.suite, "stuck");
  assert.match(overdue.name, /the stuck suite ran to its end/);
  assert.match(overdue.detail, /^Error: still not done: no check landed for 40 ms; the last check it landed: stuck spoke once/);
  assert.ok(results.some((one) => one.name === "last speaks" && one.pass));
});

/* The watch is a silence, not a total (SUITE_STALL_MS): a suite that keeps landing checks runs as long as it takes —
 * here five times the stall in all, a check every 10 ms against a stall of 40 — and the next suite's watch starts fresh. */
test("a suite that keeps landing checks is not stuck however long it runs in all", async () => {
  const { log } = lines();
  const runner = createRunner({ log, env: { WINDOW_SUITE_STALL_MS: "40" }, argv: [] });
  const started = Date.now();
  runner.suite("long", async ({ ok }) => {
    for (let at = 0; at < 20; at += 1) {
      await new Promise((done) => setTimeout(done, 10));
      ok(`long check ${at}`, true);
    }
  });
  runner.suite("next", async ({ ok }) => {
    await new Promise((done) => setTimeout(done, 25));
    ok("next speaks", true);
  });
  const { failed, results } = await runner.run({});
  assert.equal(failed, 0, results.filter((one) => !one.pass).map((one) => one.detail).join(" | "));
  assert.ok(Date.now() - started >= 200, "the long suite ran past five stalls in all");
  assert.equal(results.filter((one) => one.suite === "long").length, 20);
  assert.ok(results.some((one) => one.name === "next speaks" && one.pass));
});

test("a suite run only by name is a measurement: it takes no limit", async () => {
  const { log } = lines();
  const runner = createRunner({ log, env: { WINDOW_SUITES: "slow", WINDOW_SUITE_STALL_MS: "5" }, argv: [] });
  runner.suite("slow", async ({ ok }) => {
    await new Promise((done) => setTimeout(done, 40));
    ok("slow landed", true);
  }, { onlyByName: true });
  const { failed, results } = await runner.run({});
  assert.equal(failed, 0);
  assert.deepEqual(results.map((one) => one.name), ["slow landed"]);
});

for (const signal of ["SIGINT", "SIGTERM"]) {
  // On Windows `kill` ends a process outright, whatever the signal's name: no handler runs, so nothing can print.
  test(`a run cut by ${signal} prints what landed and how far it got, and ends red`, { skip: process.platform === "win32" && "no signal handler runs on Windows" }, async () => {
    const script = `
      import { createRunner } from ${JSON.stringify(new URL("./window-runner.mjs", import.meta.url).href)};
      const runner = createRunner({ env: {}, argv: [] });
      runner.reportWhenCut();
      runner.ok("a flow check landed", true);
      setInterval(() => {}, 1000);
      runner.suite("stuck", async ({ ok }) => {
        ok("stuck spoke once", true);
        process.stderr.write("standing\\n");
        await new Promise(() => {});
      });
      await runner.run({});
    `;
    const child = spawn(process.execPath, ["--input-type=module", "-e", script], { stdio: ["ignore", "pipe", "pipe"] });
    let out = "";
    child.stdout.on("data", (chunk) => { out += chunk; });
    await new Promise((standing) => child.stderr.on("data", (chunk) => {
      if (String(chunk).includes("standing")) standing();
    }));
    child.kill(signal);
    const [code] = await new Promise((ended) => child.on("close", (...how) => ended(how)));
    assert.notEqual(code, 0);
    assert.match(out, /^PASS {2}a flow check landed/m);
    assert.match(out, /^PASS {2}stuck spoke once/m);
    assert.match(out, new RegExp(`^FAIL {2}the run reached its end.*${signal}.*stuck spoke once`, "m"));
    assert.match(out, /^2\/3 passed$/m);
  });
}
