import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { setTimeout as pause } from "node:timers/promises";
import { promisify } from "node:util";
import { fileURLToPath } from "node:url";

const BASE = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(BASE, "../..");
const execute = promisify(execFile);
const TERMINAL = new Set(["reclaimable", "released", "orphaned", "release_pending", "release_unknown"]);

export function realOptions(args) {
  const options = { task: "Q1", repeat: 1, timeoutMs: 300_000, authorized: false };
  const names = new Map([
    ["--lane", "lane"], ["--task", "task"], ["--repeat", "repeat"],
    ["--timeout-ms", "timeoutMs"], ["--agent", "agent"], ["--model", "model"],
    ["--effort", "effort"], ["--run", "run"], ["--out", "out"],
  ]);
  for (let index = 0; index < args.length; index += 1) {
    if (args[index] === "--authorize-real-run") { options.authorized = true; continue; }
    const name = names.get(args[index]);
    assert(name && args[index + 1], `unknown or incomplete real baseline option: ${args[index]}`);
    options[name] = args[++index];
  }
  assert.equal(options.lane, "b", "real baseline requires --lane b");
  assert(options.authorized, "real baseline requires explicit --authorize-real-run");
  assert(options.agent && options.model && options.effort, "name the exact --agent, --model and --effort");
  assert(["Q1", "Q4"].includes(options.task), "real baseline currently supports independently judged Q1 and Q4");
  options.repeat = Number(options.repeat);
  options.timeoutMs = Number(options.timeoutMs);
  assert(Number.isInteger(options.repeat) && options.repeat >= 1 && options.repeat <= 3, "real repeats must be 1..3");
  assert(Number.isInteger(options.timeoutMs) && options.timeoutMs >= 1000 && options.timeoutMs <= 900_000, "real timeout must be 1000..900000 ms");
  return options;
}

export function requireExactLaunch(launch, request) {
  assert(launch.workerId && launch.dispatchId, "the ledger did not create a worker and attempt");
  assert.equal(launch.agent, request.agent, "the ledger substituted the requested agent");
  assert.equal(launch.model, request.model, "the ledger substituted the requested model");
  assert.equal(launch.effort, request.effort, "the ledger substituted the requested effort");
  assert.equal(launch.worktree, true, "a real evaluation worker needs its own worktree");
}

export function realRecord({ request, launch, elapsedMs, completed, judgeRc, repeat, reason = null }) {
  assert(Number.isFinite(elapsedMs) && elapsedMs >= 0, "elapsed time must be observed");
  return {
    schema: 1, lane: "real", task: request.task, repeat,
    agent: launch.agent, model: { requested: request.model, launched: launch.model, effective: null },
    effort: launch.effort,
    verified: completed && judgeRc === 0, completed, judge_rc: judgeRc,
    elapsed_ms: Math.round(elapsedMs),
    // The ledger proves the launch and completion, not provider billing or
    // human-origin writes. Missing observations must not become free/zero.
    interventions: null, tokens: null, cost_usd: null,
    first_visible_ms: null, rss_peak_mb: null,
    receipt: { run: request.run, task: launch.taskId, worker: launch.workerId, dispatch: launch.dispatchId },
    reason,
  };
}

export function realSummary(rows) {
  const observed = key => rows.map(row => row[key]).filter(value => typeof value === "number").sort((a, b) => a - b);
  const percentile = (values, fraction) => values.length ? values[Math.ceil(values.length * fraction) - 1] : null;
  const elapsed = observed("elapsed_ms");
  return {
    runs: rows.length,
    verified: rows.filter(row => row.verified).length,
    elapsed_p50_ms: percentile(elapsed, 0.5),
    elapsed_p95_ms: percentile(elapsed, 0.95),
    intervention_samples: observed("interventions").length,
    token_samples: rows.filter(row => row.tokens !== null).length,
    cost_samples: observed("cost_usd").length,
  };
}

export async function runRealBaseline(args, { command = execute, wait = pause, now = () => performance.now() } = {}) {
  const request = realOptions(args);
  const call = async (file, words, options = {}) => {
    try {
      const result = await command(file, words, { cwd: ROOT, encoding: "utf8", maxBuffer: 8 * 1024 * 1024,
        timeout: request.timeoutMs, ...options });
      return { rc: 0, stdout: result.stdout ?? "", stderr: result.stderr ?? "" };
    } catch (error) {
      if (typeof error.code === "number") return { rc: error.code, stdout: error.stdout ?? "", stderr: error.stderr ?? "" };
      throw error;
    }
  };
  const ledger = async words => {
    const result = await call("zerocode-orc", [...words, ...(request.run ? ["--run", request.run] : [])]);
    assert.equal(result.rc, 0, `ledger refused ${words[0]}: ${result.stderr}`);
    return JSON.parse(result.stdout);
  };
  const current = await ledger(["run-current"]);
  assert(current.runId && current.seated, "run Lane B from a live coordinator pane with a run");
  request.run = current.runId;
  const output = resolve(request.out ?? join(BASE, "out", `real-${now()}-${process.pid}`));
  // A fresh folder prevents an old run's receipts or fixture from being reused.
  await mkdir(dirname(output), { recursive: true });
  await mkdir(output, { recursive: false });
  const rows = [];
  for (let repeat = 1; repeat <= request.repeat; repeat += 1) {
    const root = join(output, `${request.task}-${repeat}`);
    const prepared = await call("python3", [join(BASE, "fixture.py"), "prepare", root]);
    assert.equal(prepared.rc, 0, "fixture preparation failed");
    const before = await call("python3", [join(BASE, "fixture.py"), "verify", request.task, root]);
    await writeFile(join(root, "evidence", "judge-before.log"), before.stdout + before.stderr);
    assert.equal(before.rc, 1, "the planted fixture must fail its independent judge before the model runs");
    const taskPrompt = await readFile(join(BASE, "tasks", request.task, "prompt.txt"), "utf8");
    const worktree = request.task === "Q4"
      ? `Create branch baseline-fix as a git worktree at ${join(root, "worktree")} from ${join(root, "repo")}. Fix and commit there. Keep the original repo clean and its HEAD unchanged.`
      : "Edit only src/lib.rs in the fixture repository. Do not commit or create other files.";
    const prompt = [
      `Real workflow evaluation ${request.task}. Work only in this disposable fixture: ${join(root, "repo")}.`,
      taskPrompt.trim(), worktree,
      "Do not edit the ZeroCode checkout, use answer patches, read the baseline's answers directory, change tests, or launch other agents.",
      "Run the fixture tests. Send worker_done through this run with ok:true only if the task is done; otherwise ok:false. The coordinator independently judges the files.",
    ].join("\n");
    const promptFile = join(root, "prompt.md");
    await writeFile(promptFile, prompt);
    const retry = `real-baseline-${process.pid}-${now()}-${repeat}`;
    const task = await ledger(["task-create", "--title", `Real baseline ${request.task}`, "--spec-file", promptFile, "--retry-request", `${retry}-task`]);
    const started = now();
    const launch = await ledger([
      "worker-start", "--agent", request.agent, "--model", request.model, "--effort", request.effort,
      "--worktree", "--task", task.taskId, "--prompt-file", promptFile, "--retry-request", `${retry}-start`,
    ]);
    let completed = false;
    let reason = null;
    try {
      requireExactLaunch(launch, request);
      await writeFile(join(root, "evidence", "launch.json"), JSON.stringify({
        taskId: task.taskId, workerId: launch.workerId, dispatchId: launch.dispatchId,
        agent: launch.agent, model: launch.model, effort: launch.effort, worktree: launch.worktree,
      }, null, 2));
      while (now() - started < request.timeoutMs) {
        const worker = await ledger(["worker-show", "--worker", launch.workerId]);
        const state = await ledger(["task-list", "--task", task.taskId, "--brief"]);
        const taskState = state.tasks.find(row => row.taskId === task.taskId);
        if (taskState?.attempt !== launch.dispatchId) { reason = "task attempt changed"; break; }
        if (taskState?.status === "completed") { completed = true; break; }
        if (worker.dispatchId !== launch.dispatchId) { reason = "worker attempt changed"; break; }
        if (TERMINAL.has(worker.state) || ["failed", "blocked", "ready"].includes(taskState?.status)) {
          reason = `worker ended without completion (${worker.state})`; break;
        }
        await wait(Math.min(1000, Math.max(1, request.timeoutMs - (now() - started))));
      }
      if (!completed && !reason) reason = "evaluation timeout";
    } catch (error) {
      reason = error.message;
    }
    // Stop only this attempt. A later dispatch on a reused worker is protected
    // by --dispatch; a refusal is retained rather than treated as a stop.
    if (!completed && launch.workerId && launch.dispatchId) {
      const reasonFile = join(root, "stop-reason.md");
      await writeFile(reasonFile, reason ?? "evaluation did not complete");
      const stop = await call("zerocode-orc", ["worker-stop", "--run", request.run, "--worker", launch.workerId,
        "--dispatch", launch.dispatchId, "--reason-file", reasonFile, "--retry-request", `${retry}-stop`]);
      await writeFile(join(root, "evidence", "stop.json"), JSON.stringify(stop));
    }
    const judged = await call("python3", [join(BASE, "fixture.py"), "verify", request.task, root]);
    await writeFile(join(root, "evidence", "judge-after.log"), judged.stdout + judged.stderr);
    const row = realRecord({ request, launch: { ...launch, taskId: task.taskId }, completed,
      elapsedMs: now() - started, judgeRc: judged.rc, repeat, reason });
    rows.push(row);
    await writeFile(join(output, "runs.jsonl"), rows.map(row => JSON.stringify(row)).join("\n") + "\n");
  }
  const summary = realSummary(rows);
  await writeFile(join(output, "summary.json"), JSON.stringify(summary, null, 2) + "\n");
  await writeFile(join(output, "report.md"), [
    "# Lane B — real provider workflow", "",
    `Verified ${summary.verified}/${summary.runs}. Completion requires both a durable ledger task completion and the independent fixture judge.`, "",
    "| Task | Agent / model / effort | Verified | Elapsed ms | Judge rc |",
    "|---|---|---|---:|---:|",
    ...rows.map(row => `| ${row.task} | ${row.agent} / ${row.model.launched} / ${row.effort} | ${row.verified} | ${row.elapsed_ms} | ${row.judge_rc} |`),
    "", "Elapsed covers launch, task completion and the independent judge. The ledger records the launch model; provider-effective model is unobserved.",
    "Human intervention, tokens, cost, first-visible and memory are unobserved (null). They are not zero and no dollar spend-cap claim is made.",
    "Q1/Q4 measure real coding and worktree completion. They do not measure native UI or live-browser quality.", "",
  ].join("\n"));
  return { output, rows, summary };
}
