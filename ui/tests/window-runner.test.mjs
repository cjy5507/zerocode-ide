import assert from "node:assert/strict";
import test from "node:test";

import { spawn } from "node:child_process";

import { END_GRACE_MS } from "./end-run.mjs";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { createRunner, legacyOnlyName, planShares, REPORT } from "./window-runner.mjs";
import { mergeShares, runShards, shardCount } from "./window-shards.mjs";

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

/* ---- a run split across browsers (window-shards.mjs) ------------------------------------------------------------ */

test("the split puts every suite in exactly one share, heaviest first into the lightest, each share in registration order", () => {
  const names = ["window", "a", "b", "c", "d", "e", "unknown"];
  const weights = new Map([["window", 400], ["a", 130], ["b", 110], ["c", 60], ["d", 60], ["e", 10]]);
  const plan = planShares(names, weights, 3);
  assert.deepEqual(plan.flat().sort(), [...names].sort(), "every suite once");
  assert.deepEqual(plan, planShares(names, weights, 3), "every process plans the same split");
  for (const share of plan) {
    assert.deepEqual(share, [...share].sort((x, y) => names.indexOf(x) - names.indexOf(y)), "registration order inside a share");
  }
  // window alone outweighs the rest, so it stands in a share of its own; a suite the times do not name weighs what the
  // middle of the named ones does (110, the upper middle of six).
  assert.deepEqual(plan[0], ["window"]);
  const load = (share) => share.reduce((sum, name) => sum + (weights.get(name) ?? 110), 0);
  assert.deepEqual(plan.slice(1).map(load), [250, 230], JSON.stringify(plan));
  assert.deepEqual(planShares(["x", "y", "z"], new Map(), 2).flat().sort(), ["x", "y", "z"], "no times still splits every suite");
});

test("a run is split only when it asks and names no suite — the lane's solo judgment and a share itself never are", () => {
  assert.equal(shardCount({}, []), 1);
  assert.equal(shardCount({ WINDOW_SHARDS: "4" }, []), 4);
  assert.equal(shardCount({ WINDOW_SHARDS: "1" }, []), 1);
  assert.equal(shardCount({ WINDOW_SHARDS: "four" }, []), 1);
  assert.equal(shardCount({ WINDOW_SHARDS: "4", WINDOW_SUITES: "board-orbit" }, []), 1);
  assert.equal(shardCount({ WINDOW_SHARDS: "4" }, ["--suite", "workers"]), 1);
  assert.equal(shardCount({ WINDOW_SHARDS: "4", EDITOR_RECOVERY_ONLY: "1" }, []), 1);
  assert.equal(shardCount({ WINDOW_SHARDS: "4", WINDOW_SHARD: "2/4" }, []), 1);
});

test("a share runs only its own suites, prints no report, and leaves its plan before its first suite and its checks after", async () => {
  const folder = await mkdtemp(join(tmpdir(), "window-share-test-"));
  try {
    const weights = join(folder, "weights.json");
    await writeFile(weights, JSON.stringify([{ suite: "heavy", ms: 100 }, { suite: "x", ms: 10 }, { suite: "y", ms: 10 }]));
    const file = join(folder, "share.json");
    const { said, log } = lines();
    const runner = createRunner({ log, argv: [], env: { WINDOW_SHARD: "2/2", WINDOW_SHARD_OUT: file, WINDOW_SHARD_WEIGHTS: weights } });
    const ran = [];
    let planBeforeFirst = null;
    for (const name of ["heavy", "x", "focused", "y"]) {
      runner.suite(name, async ({ ok }) => {
        planBeforeFirst ??= JSON.parse(await readFile(file, "utf8"));
        ran.push(name);
        ok(`${name} speaks`, name !== "y");
      }, { onlyByName: name === "focused" });
    }
    await runner.run({});
    assert.deepEqual(ran, ["x", "y"], "heavy is share 1's; a suite run only by name is no share's");
    assert.deepEqual(planBeforeFirst.plan, ["x", "y"]);
    assert.deepEqual(planBeforeFirst.order, ["heavy", "x", "y"]);
    assert.deepEqual(planBeforeFirst.ended, []);
    assert.equal(runner.report(), 1, "the share's failed count is its exit code's truth");
    assert.deepEqual(said, [], "a share prints no report");
    const left = JSON.parse(await readFile(file, "utf8"));
    assert.deepEqual(left.ended, ["x", "y"]);
    assert.deepEqual(left.results.map((one) => [one.suite, one.pass]), [["x", true], ["y", false]]);
    assert.deepEqual(left.times.map((one) => one.suite), ["x", "y"]);
  } finally {
    await rm(folder, { recursive: true, force: true });
  }
});

test("shares that died, disagreed or never planned leave FAILs under the suites they owed, never a quiet green", async () => {
  const folder = await mkdtemp(join(tmpdir(), "window-merge-test-"));
  try {
    const at = (name) => join(folder, name);
    const order = ["a", "b", "c", "d"];
    await writeFile(at("1.json"), JSON.stringify({ order, plan: ["a", "c"], ended: ["a"],
      results: [{ suite: "a", name: "a speaks", pass: true, detail: "" }, { suite: "c", name: "c spoke once", pass: true, detail: "" }],
      times: [{ suite: "a", ms: 5, checks: 1 }] }));
    await writeFile(at("2.json"), JSON.stringify({ order, plan: ["b", "d"], ended: ["b", "d"],
      results: [{ suite: "b", name: "b speaks", pass: true, detail: "" }, { suite: "d", name: "d speaks", pass: true, detail: "" }],
      times: [{ suite: "b", ms: 5, checks: 1 }, { suite: "d", ms: 5, checks: 1 }] }));
    const { results, times } = await mergeShares([
      { index: 1, file: at("1.json"), code: null, signal: "SIGKILL" },
      { index: 2, file: at("2.json"), code: 0, signal: null },
    ], 2);
    assert.deepEqual(results.map((one) => [one.suite, one.name, one.pass]), [
      ["a", "a speaks", true],
      ["b", "b speaks", true],
      ["c", "c spoke once", true],
      ["c", "the c suite ran to its end", false],
      ["d", "d speaks", true],
    ]);
    assert.match(results[3].detail, /share 1\/2 ended SIGKILL/);
    assert.deepEqual(times.map((one) => one.suite), ["a", "b", "d"]);
    // A share that left no file at all owes the run its plan: a FAIL of its own, and the split cannot be called whole.
    const lost = await mergeShares([
      { index: 1, file: at("1.json"), code: 0, signal: null },
      { index: 2, file: at("missing.json"), code: 9, signal: null },
    ], 2);
    assert.ok(lost.results.some((one) => one.suite === null && /share 2\/2 ran to its end/.test(one.name) && !one.pass));
    // Two shares that planned the same suite, or planned fewer than the run takes, are not a whole split.
    await writeFile(at("3.json"), JSON.stringify({ order, plan: ["a", "b"], ended: ["a", "b"], results: [], times: [] }));
    const doubled = await mergeShares([
      { index: 1, file: at("1.json"), code: 0, signal: null },
      { index: 2, file: at("3.json"), code: 0, signal: null },
    ], 2);
    assert.ok(doubled.results.some((one) => one.name === "the window shares split the run whole" && !one.pass));
  } finally {
    await rm(folder, { recursive: true, force: true });
  }
});

test("a split run prints one report of every share's checks and ends red when a share died mid-suite", async () => {
  const folder = await mkdtemp(join(tmpdir(), "window-shards-test-"));
  try {
    const weights = join(folder, "weights.json");
    await writeFile(weights, JSON.stringify(["s1", "s2", "s3", "s4", "s5", "s6"].map((suite) => ({ suite, ms: 10 }))));
    const script = join(folder, "fake-window.mjs");
    await writeFile(script, `
      import { createRunner } from ${JSON.stringify(new URL("./window-runner.mjs", import.meta.url).href)};
      const runner = createRunner();
      for (const name of ["s1", "s2", "s3", "s4", "s5", "s6"]) {
        runner.suite(name, async ({ ok }) => {
          ok(name + " speaks", true);
          console.log("NOISE from " + name);
          if (name === process.env.FAKE_DIES_IN) process.exit(3);
        });
      }
      await runner.run({});
      process.exitCode = runner.report() ? 1 : 0;
    `);
    const run = async (env) => {
      const { said, log } = lines();
      let out = "";
      let err = "";
      const code = await runShards({ count: 3, script, env: { ...process.env, WINDOW_SHARD_WEIGHTS: weights, ...env }, log,
        out: { write: (bytes, done) => { out += String(bytes); if (typeof done === "function") done(); return true; } },
        err: { write: (bytes) => { err += String(bytes); return true; } } });
      return { code, said, out, err };
    };
    const green = await run({});
    assert.equal(green.code, 0, green.err);
    assert.equal(green.said.at(-1), "6/6 passed");
    assert.deepEqual(green.said.filter((line) => line.startsWith(REPORT.suite)), ["s1", "s2", "s3", "s4", "s5", "s6"].map((name) => `SUITE  ${name}`));
    assert.equal((green.out.match(/^NOISE from /gm) ?? []).length, 6, "every share's own lines are passed on");
    assert.doesNotMatch(green.out, /passed$/m, "no share printed a report of its own");
    const died = await run({ FAKE_DIES_IN: "s3" });
    assert.equal(died.code, 1);
    assert.match(died.said.join("\n"), /^FAIL {2}the s3 suite ran to its end {2}— its share \d\/3 ended rc 3 before the suite did$/m);
    // The suites planned after s3 in its share never spoke: each is a FAIL under its own name, not a missing line.
    const owed = died.said.filter((line) => /^FAIL {2}the s\d suite ran to its end/.test(line));
    assert.ok(owed.length >= 1, died.said.join("\n"));
    const reported = new Set(died.said.filter((line) => line.startsWith(REPORT.suite)).map((line) => line.slice(7)));
    assert.deepEqual([...reported].sort(), ["s1", "s2", "s3", "s4", "s5", "s6"], "every suite is in the report, red or green");
  } finally {
    await rm(folder, { recursive: true, force: true });
  }
});
