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
async function scenario({ judgeRc = 0, completed = true, changedAttempt = false, substitute = false, droppedLaunch = false, takenOver = false } = {}) {
  const parent = await mkdtemp(join(tmpdir(), "real-baseline-test-"));
  const out = join(parent, "output");
  const calls = [];
  let clock = 0;
  let judges = 0;
  let starts = 0;
  const result = value => ({ stdout: JSON.stringify(value), stderr: "" });
  const command = async (file, words) => {
    calls.push({ file, words });
    if (file === "git") return { stdout: words[0] === "rev-parse" ? "a".repeat(40) + "\n" : "", stderr: "" };
    if (file === "python3") {
      if (words[1] === "prepare") { await mkdir(join(words[2], "evidence"), { recursive: true }); return result(null); }
      const rc = ++judges === 1 ? 1 : judgeRc;
      if (rc) throw Object.assign(new Error("judge rejected"), { code: rc, stdout: "", stderr: "judge rejected" });
      return result(null);
    }
    assert.equal(file, "zerocode-orc");
    switch (words[0]) {
      case "run-current": return result({ runId: "run-test", seated: true });
      case "task-create": return result({ taskId: launch.taskId });
      case "worker-start":
        if (droppedLaunch && starts++ === 0) throw Object.assign(new Error("response lost"), { code: 1 });
        return result({ ...launch, ...(substitute ? { model: "other" } : {}) });
      case "worker-show": return result({ dispatchId: completed ? null : launch.dispatchId, state: completed ? "reclaimable" : "active", takenOver });
      case "task-list": return result({ tasks: [{ taskId: launch.taskId, attempt: changedAttempt ? "dp-replacement" : launch.dispatchId, status: completed ? "completed" : "dispatched" }] });
      case "worker-stop": return result({ stopped: !changedAttempt });
      default: throw new Error(`unexpected command ${words[0]}`);
    }
  };
  try {
    const value = await runRealBaseline([...args, "--out", out, "--timeout-ms", "1000"], {
      command, now: () => clock, wait: async milliseconds => { clock += milliseconds; },
    });
    return { ...value, calls, report: await readFile(join(out, "report.md"), "utf8") };
  } finally {
    await rm(parent, { recursive: true, force: true });
  }
}

test("a durable completed task counts after the worker dispatch is cleared", async () => {
  const result = await scenario();
  assert.equal(result.rows[0].verified, true);
  assert.equal(result.calls.some(call => call.words[0] === "worker-stop"), false);
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
});
