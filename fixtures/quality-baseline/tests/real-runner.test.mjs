import assert from "node:assert/strict";
import { mkdtemp, mkdir, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { realOptions, requireExactLaunch, realRecord, realSummary, runRealBaseline } from "../real-runner.mjs";

const args = ["--lane", "b", "--authorize-real-run", "--agent", "claude", "--model", "sonnet", "--effort", "max"];
const request = { ...realOptions(args), run: "run-test" };
const launch = { taskId: "t-test", workerId: "w-test", dispatchId: "dp-test", agent: "claude", model: "sonnet", effort: "max", worktree: true };

test("real evaluation requires authorization, an exact launch and bounded work", () => {
  assert.equal(realOptions(args).repeat, 1);
  for (const invalid of [
    args.filter(value => value !== "--authorize-real-run"),
    ["--lane", "b", "--authorize-real-run"],
    [...args, "--repeat", "4"], [...args, "--timeout-ms", "900001"],
    [...args, "--task", "Q5"],
  ]) assert.throws(() => realOptions(invalid));
  for (const changed of [{ agent: "other" }, { model: "other" }, { effort: "high" }, { worktree: false }]) {
    assert.throws(() => requireExactLaunch({ ...launch, ...changed }, request));
  }
});

test("an uncommitted runner cannot contact the ledger or a provider", async () => {
  await assert.rejects(runRealBaseline(args, { command: async (file, words) => {
    assert.equal(file, "git", "dirty-source refusal must happen before any ledger operation");
    return { stdout: words[0] === "rev-parse" ? "a".repeat(40) + "\n" : " M runner.mjs\n", stderr: "" };
  } }), /commit the runner changes/);
});

test("real success requires both completion and an independent passing judge", () => {
  for (const completed of [false, true]) {
    for (const judgeRc of [0, 1]) {
      const row = realRecord({ request, launch, completed, judgeRc, elapsedMs: 12.4, repeat: 1 });
      assert.equal(row.verified, completed && judgeRc === 0);
      assert.equal(row.elapsed_ms, 12);
      for (const unknown of ["tokens", "cost_usd", "interventions", "first_visible_ms", "rss_peak_mb"]) assert.equal(row[unknown], null);
      assert.equal(row.model.effective, null);
    }
  }
});

test("real report uses the observed distribution and counts missing measurements", () => {
  const rows = [10, 100, 900].map(elapsedMs => realRecord({ request, launch, completed: true, judgeRc: 0, elapsedMs, repeat: 1 }));
  assert.deepEqual(realSummary(rows), {
    runs: 3, verified: 3, elapsed_p50_ms: 100, elapsed_p95_ms: 900,
    intervention_samples: 0, token_samples: 0, cost_samples: 0,
  });
});

// Synthetic transport only: these tests never contact a provider, execute
// cargo, or publish their scores as real workflow evidence.
async function scenario({ judgeRc = 0, completed = true, changedAttempt = false, substitute = false,
  droppedLaunch = false, takenOver = false, isolated = true, stopFailure = false, stopFailsOnce = false,
  takeoverAtStop = false, abortAfterStart = false, commandTime = 0, loseEveryLaunch = false,
  repeat = 1, timeoutMs = 1000, workerChanges = {}, lostRecordedLaunch = false, replacementDuringRecovery = false } = {}) {
  const parent = await mkdtemp(join(tmpdir(), "real-baseline-test-"));
  const out = join(parent, "output");
  const common = join(parent, "common");
  const workerDirectory = join(parent, "worker");
  const workerGit = join(common, "worktrees", "worker");
  await Promise.all([mkdir(workerGit, { recursive: true }), mkdir(workerDirectory)]);
  const calls = [];
  let clock = 0;
  let judges = 0;
  let starts = 0;
  let stopped = false;
  let lateTakeover = false;
  let accepted = false;
  let stopCalls = 0;
  const controller = new AbortController();
  const result = value => ({ stdout: JSON.stringify(value), stderr: "" });
  const command = async (file, words, options) => {
    calls.push({ file, words, options });
    if (file === "git") return { stdout: words[0] === "-C" ? `${workerDirectory}\n${workerGit}\n${common}\n`
      : words.includes("--git-common-dir") ? common + "\n" : words[0] === "rev-parse" ? "a".repeat(40) + "\n" : "", stderr: "" };
    if (file === "python3") {
      if (words[1] === "prepare") { await mkdir(join(words[2], "evidence"), { recursive: true }); return result(null); }
      const rc = ++judges === 1 ? 1 : judgeRc;
      if (rc) throw Object.assign(new Error("judge rejected"), { code: rc, stdout: "", stderr: "judge rejected" });
      return result(null);
    }
    assert.equal(file, "zerocode-orc");
    switch (words[0]) {
      case "run-current": return result({ runId: "run-test", seated: true });
      case "run-show": return result({ auto: null, handover: null });
      case "task-create": return result({ taskId: launch.taskId });
      case "worker-start":
        if (loseEveryLaunch || (droppedLaunch && starts++ === 0)) throw Object.assign(new Error("response lost"), { code: 1 });
        accepted = true;
        if (lostRecordedLaunch && starts++ === 0) throw Object.assign(new Error("accepted response lost"), { code: 1 });
        if (abortAfterStart) controller.abort();
        return result({ ...launch, ...(substitute ? { model: "other" } : {}) });
      case "dispatch-show":
        if (loseEveryLaunch) throw Object.assign(new Error("recovery bridge unavailable"), { code: "ENOENT" });
        return result({ taskId: launch.taskId, workerId: accepted ? launch.workerId : null,
          dispatchId: accepted ? launch.dispatchId : null, open: !completed,
          time: { task: { attempts: replacementDuringRecovery ? 2 : 1 } } });
      case "worker-show":
        clock += commandTime;
        return result({ ...launch, checkout: workerDirectory, inWorktree: isolated, dispatchId: completed || stopped ? null : launch.dispatchId,
          state: stopped ? "released" : completed ? "reclaimable" : "active", takenOver: takenOver || lateTakeover, ...workerChanges });
      case "task-list":
        clock += commandTime;
        return result({ tasks: [{ taskId: launch.taskId, attempt: changedAttempt ? "dp-replacement" : launch.dispatchId,
          status: stopped ? "ready" : completed ? "completed" : "dispatched" }] });
      case "worker-stop":
        stopCalls += 1;
        if (takeoverAtStop) {
          lateTakeover = true;
          throw Object.assign(new Error("taken over by the person"), { code: 1 });
        }
        if (stopFailure || (stopFailsOnce && stopCalls === 1)) throw Object.assign(new Error("bridge is unavailable"), { code: "ENOENT" });
        if (changedAttempt) throw Object.assign(new Error("attempt changed"), { code: 1 });
        if (completed) throw Object.assign(new Error("attempt already cleared"), { code: 1 });
        stopped = true;
        return result({ state: "stop_pending" });
      case "task-update":
        if (loseEveryLaunch || replacementDuringRecovery) throw Object.assign(new Error("a recorded attempt exists"), { code: 1 });
        return result({ taskId: launch.taskId, status: "closed" });
      default: throw new Error(`unexpected command ${words[0]}`);
    }
  };
  try {
    const value = await runRealBaseline([...args, "--out", out, "--timeout-ms", String(timeoutMs), "--repeat", String(repeat)], {
      command, now: () => clock, wait: async milliseconds => { clock += milliseconds; }, signal: controller.signal,
    });
    const stopEvidence = await readFile(join(out, "Q1-1/evidence/stop.json"), "utf8").then(JSON.parse).catch(() => null);
    const checkoutProof = await readFile(join(out, "Q1-1/evidence/worker-checkout.json"), "utf8").then(JSON.parse).catch(() => null);
    return { ...value, calls, stopEvidence, checkoutProof, report: await readFile(join(out, "report.md"), "utf8") };
  } finally {
    await rm(parent, { recursive: true, force: true });
  }
}

test("a durable completed task counts after the worker dispatch is cleared", async () => {
  const result = await scenario();
  assert.equal(result.rows[0].verified, true);
  assert.equal(result.calls.some(call => call.words[0] === "worker-stop"), false);
  assert.equal(result.checkoutProof.workerId, "w-test");
  assert.match(result.report, /unobserved \(null\)/);
});

test("an agent completion claim cannot override a failed independent judge", async () => {
  assert.equal((await scenario({ judgeRc: 1 })).rows[0].verified, false);
});

test("timeouts and changed attempts never stop a replacement dispatch", async () => {
  for (const options of [{ completed: false }, { changedAttempt: true }]) {
    const result = await scenario(options);
    assert.equal(result.rows[0].verified, false);
    const stop = result.calls.find(call => call.words[0] === "worker-stop");
    assert.equal(stop.words[stop.words.indexOf("--dispatch") + 1], "dp-test");
  }
});

test("a substituted launch is stopped and cannot be scored as verified", async () => {
  const result = await scenario({ substitute: true });
  assert.equal(result.rows[0].verified, false);
  assert.match(result.rows[0].reason, /substituted/);
  assert(result.calls.some(call => call.words[0] === "worker-stop"));
});

test("a dropped launch response replays exactly one idempotent request", async () => {
  const result = await scenario({ droppedLaunch: true });
  const starts = result.calls.filter(call => call.words[0] === "worker-start");
  assert.equal(starts.length, 2);
  assert.deepEqual(starts[0].words, starts[1].words);
  assert.equal(result.rows[0].verified, true);
});

test("a pane the person took over is left running and its score is not verified", async () => {
  const result = await scenario({ completed: false, takenOver: true });
  assert.equal(result.rows[0].verified, false);
  assert.equal(result.calls.some(call => call.words[0] === "worker-stop"), false);
  assert.equal(result.calls.some(call => call.words[0] === "task-update"), false);
});

test("automatic assignment is refused before creating an evaluation task", async () => {
  await assert.rejects(runRealBaseline(args, { command: async (file, words) => {
    if (file === "git") return { stdout: words.includes("--git-common-dir") ? tmpdir() + "\n" : words[0] === "rev-parse" ? "a".repeat(40) + "\n" : "" };
    if (words[0] === "run-current") return { stdout: JSON.stringify({ runId: "run-test", seated: true }) };
    assert.equal(words[0], "run-show", "no task or paid worker should be created");
    return { stdout: JSON.stringify({ auto: { max: 1 } }) };
  } }), /automatic assignment off/);
});

test("a worktree request without a reported checkout cannot be verified", async () => {
  const result = await scenario({ workerChanges: { checkout: null } });
  assert.equal(result.rows[0].verified, false);
  assert.match(result.rows[0].reason, /not reported a verified checkout/);
  assert.equal(result.checkoutProof, null);
  assert(result.calls.some(call => call.words[0] === "worker-stop"));
});

test("active calls share the remaining deadline, with separate bounded cleanup", async () => {
  const result = await scenario({ completed: false, commandTime: 250 });
  const taskRead = result.calls.find(call => call.words[0] === "task-list");
  assert.equal(taskRead.options.timeout, 750);
  const stop = result.calls.find(call => call.words[0] === "worker-stop");
  assert.equal(stop.options.timeout, 5000);
  assert.equal(result.rows[0].verified, false);
  const close = result.calls.find(call => call.words[0] === "task-update");
  assert.equal(close.words[close.words.indexOf("--attempt") + 1], "dp-test");
});

test("cancellation stops only the recorded attempt, saves a row and prevents more workers", async () => {
  const result = await scenario({ abortAfterStart: true, completed: false, repeat: 3 });
  assert.equal(result.rows.length, 1);
  assert.equal(result.rows[0].verified, false);
  assert.match(result.rows[0].reason, /cancelled/);
  assert.equal(result.calls.filter(call => call.words[0] === "worker-start").length, 1);
  const stop = result.calls.find(call => call.words[0] === "worker-stop");
  assert.equal(stop.words[stop.words.indexOf("--dispatch") + 1], "dp-test");
  assert.equal(stop.options.signal, undefined, "cleanup must not inherit the cancelled signal");
  assert(result.calls.some(call => call.words[0] === "task-update"));
});

test("an unavailable stop bridge preserves failure evidence and the partial report", async () => {
  const result = await scenario({ completed: false, stopFailure: true });
  assert.equal(result.rows[0].verified, false);
  assert.match(result.rows[0].reason, /could not be stopped/);
  assert.equal(result.stopEvidence.rc, null);
  assert.match(result.stopEvidence.error, /bridge is unavailable/);
  assert.equal(result.calls.some(call => call.words[0] === "task-update"), false);
});

test("a stop refused after human takeover is never retried or followed by task closure", async () => {
  const result = await scenario({ completed: false, takeoverAtStop: true });
  assert.equal(result.rows[0].verified, false);
  assert.match(result.rows[0].reason, /person took over/);
  assert.equal(result.calls.filter(call => call.words[0] === "worker-stop").length, 1);
  assert.equal(result.calls.some(call => call.words[0] === "task-update"), false);
});

test("an unconfirmed launch is unverified and retains its idempotent recovery request", async () => {
  const result = await scenario({ loseEveryLaunch: true, repeat: 3 });
  assert.equal(result.rows[0].verified, false);
  assert.equal(result.rows[0].receipt.dispatch, null);
  assert.match(result.rows[0].reason, /recover from launch-request.json/);
  const starts = result.calls.filter(call => call.words[0] === "worker-start");
  assert.equal(starts.length, 1, "do not replay a launch while even its recovery state is unreadable");
  assert.equal(result.calls.some(call => call.words[0] === "worker-stop"), false);
  const close = result.calls.find(call => call.words[0] === "task-update");
  assert.equal(close.words[close.words.indexOf("--attempt") + 1], "none");
  assert.match(result.rows[0].reason, /task closure could not be confirmed/);
});

test("handover policies are refused before creating an evaluation task", async () => {
  await assert.rejects(runRealBaseline(args, { command: async (file, words) => {
    if (file === "git") return { stdout: words.includes("--git-common-dir") ? tmpdir() + "\n" : words[0] === "rev-parse" ? "a".repeat(40) + "\n" : "" };
    if (words[0] === "run-current") return { stdout: JSON.stringify({ runId: "run-test", seated: true }) };
    assert.equal(words[0], "run-show", "no task or paid worker should be created");
    return { stdout: JSON.stringify({ auto: null, handover: { agent: "other" } }) };
  } }), /automatic handover off/);
});

test("the observed worker cannot change the requested agent, model or effort", async () => {
  for (const workerChanges of [{ agent: "other" }, { model: "other" }, { effort: "high" }, { inWorktree: false }]) {
    const result = await scenario({ workerChanges });
    assert.equal(result.rows[0].verified, false);
    assert(result.calls.some(call => call.words[0] === "worker-stop"));
  }
});

test("launch uses the remaining workflow budget rather than a fixed ten seconds", async () => {
  const result = await scenario({ timeoutMs: 20000 });
  assert.equal(result.calls.find(call => call.words[0] === "worker-start").options.timeout, 20000);
  assert.equal(result.rows[0].verified, true);
});

test("a lost response reconnects to the first recorded attempt without a second launch", async () => {
  const result = await scenario({ lostRecordedLaunch: true });
  assert.equal(result.rows[0].verified, true);
  assert.equal(result.calls.filter(call => call.words[0] === "worker-start").length, 1);
  assert.equal(result.rows[0].receipt.dispatch, "dp-test");
});

test("launch recovery never adopts or stops a later task attempt", async () => {
  const result = await scenario({ lostRecordedLaunch: true, replacementDuringRecovery: true });
  assert.equal(result.rows[0].verified, false);
  assert.match(result.rows[0].reason, /more than one attempt/);
  assert.equal(result.calls.some(call => call.words[0] === "worker-stop"), false);
});

test("a transient stop failure replays only the same idempotent stop once", async () => {
  const result = await scenario({ completed: false, stopFailsOnce: true });
  const stops = result.calls.filter(call => call.words[0] === "worker-stop");
  assert.equal(stops.length, 2);
  assert.deepEqual(stops[0].words, stops[1].words);
  assert(result.calls.some(call => call.words[0] === "task-update"));
});

test("completion observed during timeout cleanup is preserved on the ledger", async () => {
  const result = await scenario({ commandTime: 600 });
  assert.equal(result.rows[0].verified, false);
  assert.match(result.rows[0].reason, /timeout/);
  assert.equal(result.calls.some(call => call.words[0] === "task-update"), false);
});

test("every runner child drops inherited Git plumbing without losing other context", async () => {
  const poison = Object.fromEntries(["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY", "GIT_NAMESPACE", "GIT_PREFIX"].map(key => [key, "/fixture/poison"]));
  poison.gIt_DiR = "/fixture/poison";
  poison.ZEROCODE_CONTEXT_SENTINEL = "fixture";
  const prior = Object.fromEntries(Object.keys(poison).map(key => [key, process.env[key]]));
  Object.assign(process.env, poison);
  try {
    const result = await scenario();
    assert.equal(result.rows[0].verified, true);
    for (const call of result.calls) {
      for (const key of Object.keys(poison).filter(key => key.toUpperCase().startsWith("GIT_"))) assert.equal(call.options.env[key], undefined, key);
      assert.equal(call.options.env.ZEROCODE_CONTEXT_SENTINEL, "fixture");
    }
  } finally {
    for (const [key, value] of Object.entries(prior)) {
      if (value === undefined) delete process.env[key];
      else process.env[key] = value;
    }
  }
});
