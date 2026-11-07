import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { mkdir, readFile, realpath, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { setTimeout as pause } from "node:timers/promises";
import { promisify } from "node:util";
import { fileURLToPath } from "node:url";

const BASE = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(BASE, "../..");
const execute = promisify(execFile);
const TERMINAL = new Set(["reclaimable", "released", "orphaned", "release_pending", "release_unknown"]);

// Same repository-boundary rule as zerocode-orchestrator: a hook/rebase's
// plumbing variables must not redirect this measurement's Git commands.
export function gitEnvironment(environment = process.env) {
  const clean = { ...environment };
  const plumbing = new Set(["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY", "GIT_NAMESPACE", "GIT_PREFIX"]);
  // Windows environment names are case-insensitive, even after copying them
  // into an ordinary JavaScript object.
  for (const key of Object.keys(clean)) if (plumbing.has(key.toUpperCase())) delete clean[key];
  return clean;
}

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

function requireWorker(worker, launch, request) {
  assert.equal(worker.workerId, launch.workerId, "the ledger returned another worker");
  for (const key of ["agent", "model", "effort"]) {
    assert.equal(worker[key], request[key], `the worker substituted the requested ${key}`);
  }
  assert.equal(worker.inWorktree, true, "the ledger did not record a worktree request");
}

export async function inspectWorkerCheckout(checkout, expected, call) {
  assert(typeof checkout === "string" && checkout.trim(), "the worker has not reported its checkout");
  const directory = await realpath(checkout);
  assert.notEqual(directory, expected.root, "the worker occupies the runner's own checkout");
  const inspected = await call("git", ["-C", directory, "rev-parse", "--path-format=absolute",
    "--show-toplevel", "--absolute-git-dir", "--git-common-dir"]);
  assert.equal(inspected.rc, 0, `could not inspect the worker checkout: ${inspected.error ?? inspected.stderr}`);
  const paths = inspected.stdout.trim().split(/\r?\n/);
  assert.equal(paths.length, 3, "Git did not report three checkout paths");
  const [root, gitDir, common] = await Promise.all(paths.map(path => realpath(path)));
  assert.equal(root, directory, "the worker checkout is not the worktree root");
  assert.equal(common, expected.common, "the worker checkout belongs to another repository");
  assert.notEqual(gitDir, common, "the worker occupies the main checkout, not a linked worktree");
  return { root, gitDir, common };
}

export function realRecord({ request, launch, elapsedMs, completed, judgeRc, repeat, reason = null }) {
  assert(Number.isFinite(elapsedMs) && elapsedMs >= 0, "elapsed time must be observed");
  return {
    schema: 1, lane: "real", task: request.task, repeat,
    runner_head: request.runnerHead ?? null, window_build: null,
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

export async function runRealBaseline(args, { command = execute, wait = pause, now = () => performance.now(), signal } = {}) {
  const request = realOptions(args);
  let deadline;
  const remaining = () => deadline === undefined ? request.timeoutMs : deadline - now();
  const checkActive = () => {
    assert(!signal?.aborted, "evaluation cancelled");
    assert(remaining() > 0, "evaluation timeout");
  };
  const call = async (file, words, options = {}) => {
    try {
      if (!Object.hasOwn(options, "signal")) checkActive();
      const result = await command(file, words, { cwd: ROOT, encoding: "utf8", maxBuffer: 8 * 1024 * 1024,
        env: gitEnvironment(), timeout: Math.max(1, Math.ceil(remaining())), signal, ...options });
      return { rc: 0, stdout: result.stdout ?? "", stderr: result.stderr ?? "" };
    } catch (error) {
      // ENOENT, a child timeout and an aborted bridge are evidence too. In
      // particular, losing the stop command must not lose the failure row.
      return { rc: typeof error.code === "number" ? error.code : null,
        stdout: error.stdout ?? "", stderr: error.stderr ?? "", error: error.message };
    }
  };
  const ledger = async (words, options) => {
    if (!options) checkActive();
    const result = await call("zerocode-orc", [...words, ...(request.run ? ["--run", request.run] : [])], options);
    assert.equal(result.rc, 0, `ledger refused ${words[0]}: ${result.error ?? result.stderr}`);
    return JSON.parse(result.stdout);
  };
  checkActive();
  const head = await call("git", ["rev-parse", "HEAD"]);
  const dirty = await call("git", ["status", "--porcelain", "--untracked-files=no"]);
  assert(head.rc === 0 && /^[a-f0-9]{40}$/.test(head.stdout.trim()), "the runner needs an identifiable source commit");
  assert(dirty.rc === 0 && dirty.stdout.trim() === "", "commit the runner changes before a real measurement");
  request.runnerHead = head.stdout.trim();
  const common = await call("git", ["rev-parse", "--path-format=absolute", "--git-common-dir"]);
  assert.equal(common.rc, 0, "the runner needs its Git common directory");
  request.checkout = { root: await realpath(ROOT), common: await realpath(common.stdout.trim()) };
  const current = await ledger(["run-current"]);
  assert(current.runId && current.seated, "run Lane B from a live coordinator pane with a run");
  request.run = current.runId;
  const runState = await ledger(["run-show"]);
  assert.equal(runState.auto, null, "real evaluation requires a run with automatic assignment off");
  assert.equal(runState.handover, null, "real evaluation requires a run with automatic handover off");
  const output = resolve(request.out ?? join(BASE, "out", `real-${now()}-${process.pid}`));
  // A fresh folder prevents an old run's receipts or fixture from being reused.
  await mkdir(dirname(output), { recursive: true });
  await mkdir(output, { recursive: false });
  const rows = [];
  for (let repeat = 1; repeat <= request.repeat; repeat += 1) {
    checkActive();
    const root = join(output, `${request.task}-${repeat}`);
    const prepared = await call("python3", [join(BASE, "fixture.py"), "prepare", root]);
    assert.equal(prepared.rc, 0, "fixture preparation failed");
    const before = await call("python3", [join(BASE, "fixture.py"), "verify", request.task, root]);
    await writeFile(join(root, "evidence", "judge-before.log"), `exit_code=${before.rc}\n${before.stdout}${before.stderr}`);
    assert.equal(before.rc, 1, "the planted fixture must fail its independent judge before the model runs");
    const taskPrompt = await readFile(join(BASE, "tasks", request.task, "prompt.txt"), "utf8");
    const worktree = request.task === "Q4"
      ? `Create branch baseline-fix as a git worktree at ${join(root, "worktree")} from ${join(root, "repo")}. Fix and commit there. Keep the original repo clean and its HEAD unchanged.`
      : "Edit only src/lib.rs in the fixture repository. Do not commit or create other files.";
    const prompt = [
      `Real workflow evaluation ${request.task}. Work only in this disposable fixture: ${join(root, "repo")}.`,
      taskPrompt.trim(), worktree,
      "Do not edit the ZeroCode checkout, use answer patches, read the baseline's answers directory, change tests, or launch other agents.",
      "Do not build ZeroCode or run its gates/pre-handin. Run only the fixture's own cargo test --offline -p fixture in the fixture repository.",
      "Run the fixture tests. Send worker_done through this run with ok:true only if the task is done; otherwise ok:false. The coordinator independently judges the files.",
    ].join("\n");
    const promptFile = join(root, "prompt.md");
    await writeFile(promptFile, prompt);
    const retry = `real-baseline-${process.pid}-${now()}-${repeat}`;
    const task = await ledger(["task-create", "--title", `Real baseline ${request.task}`, "--spec-file", promptFile, "--retry-request", `${retry}-task`]);
    const started = now();
    deadline = started + request.timeoutMs;
    const launchWords = [
      "worker-start", "--agent", request.agent, "--model", request.model, "--effort", request.effort,
      "--worktree", "--task", task.taskId, "--prompt-file", promptFile, "--retry-request", `${retry}-start`,
    ];
    await writeFile(join(root, "evidence", "launch-request.json"), JSON.stringify({ run: request.run, args: launchWords }, null, 2));
    const recoverLaunch = async optionsForCall => {
      const ask = words => ledger(words, optionsForCall?.());
      const attempt = await ask(["dispatch-show", "--task", task.taskId]);
      if (attempt.dispatchId === null) return null;
      assert.equal(attempt.taskId, task.taskId, "launch recovery returned another task");
      // The task was created by this measurement with auto/handover off. Only
      // its first recorded attempt can be this lost launch. Never attach to
      // or stop a later attempt that somebody else put on the task.
      assert.equal(attempt.time?.task?.attempts, 1, "launch recovery found more than one attempt");
      assert(attempt.dispatchId && attempt.workerId, "launch recovery has no worker identity");
      const state = await ask(["task-list", "--task", task.taskId, "--brief"]);
      assert.equal(state.tasks.find(row => row.taskId === task.taskId)?.attempt, attempt.dispatchId, "task attempt changed during launch recovery");
      const worker = await ask(["worker-show", "--worker", attempt.workerId]);
      assert.equal(worker.workerId, attempt.workerId, "launch recovery returned another worker");
      assert(worker.dispatchId === attempt.dispatchId || (worker.dispatchId === null && attempt.open === false), "worker attempt changed during launch recovery");
      const recovered = { taskId: task.taskId, workerId: worker.workerId, dispatchId: attempt.dispatchId,
        agent: worker.agent, model: worker.model, effort: worker.effort, worktree: worker.inWorktree, recovered: true };
      await writeFile(join(root, "evidence", "launch-recovery.json"), JSON.stringify(recovered, null, 2));
      return recovered;
    };
    let launch = { taskId: task.taskId, workerId: null, dispatchId: null, agent: null, model: null, effort: null };
    let completed = false;
    let reason = null;
    let mayStop = true;
    let checkedCheckout;
    try {
      try {
        checkActive();
        launch = await ledger(launchWords);
      } catch (error) {
        await writeFile(join(root, "evidence", "launch-error.log"), error.message);
        // A failed response does not prove no pane opened. Recover the first
        // recorded attempt before considering an idempotent replay.
        checkActive();
        launch = await recoverLaunch() ?? await ledger(launchWords);
      }
      requireExactLaunch(launch, request);
      await writeFile(join(root, "evidence", "launch.json"), JSON.stringify({
        taskId: task.taskId, workerId: launch.workerId, dispatchId: launch.dispatchId,
        agent: launch.agent, model: launch.model, effort: launch.effort, worktree: launch.worktree,
      }, null, 2));
      while (remaining() > 0) {
        const worker = await ledger(["worker-show", "--worker", launch.workerId]);
        if (worker.takenOver) { mayStop = false; reason = "the person took over the evaluation pane"; break; }
        requireWorker(worker, launch, request);
        if (worker.checkout && worker.checkout !== checkedCheckout && [null, launch.dispatchId].includes(worker.dispatchId)) {
          const proof = await inspectWorkerCheckout(worker.checkout, request.checkout, call);
          await writeFile(join(root, "evidence", "worker-checkout.json"), JSON.stringify({ ...proof,
            workerId: worker.workerId, dispatchId: launch.dispatchId }, null, 2));
          checkedCheckout = worker.checkout;
        }
        const state = await ledger(["task-list", "--task", task.taskId, "--brief"]);
        const taskState = state.tasks.find(row => row.taskId === task.taskId);
        if (taskState?.attempt !== launch.dispatchId) { reason = "task attempt changed"; break; }
        if (taskState?.status === "completed") {
          checkActive();
          assert(checkedCheckout, "the worker has not reported a verified checkout");
          completed = true; break;
        }
        if (worker.dispatchId !== launch.dispatchId) { reason = "worker attempt changed"; break; }
        if (TERMINAL.has(worker.state) || ["failed", "blocked", "ready"].includes(taskState?.status)) {
          reason = `worker ended without completion (${worker.state})`; break;
        }
        await wait(Math.min(1000, Math.max(1, remaining())), undefined, { signal });
      }
      if (!completed && !reason) reason = "evaluation timeout";
    } catch (error) {
      reason = signal?.aborted ? "evaluation cancelled" : remaining() <= 0 ? "evaluation timeout" : error.message;
    }
    // Cleanup has one separate, bounded grace period. It must still run when
    // the work deadline or the caller's signal has expired.
    const cleanupUntil = now() + 5000;
    const cleanupOptions = () => ({ signal: undefined, timeout: Math.max(1, Math.ceil(cleanupUntil - now())) });
    if (!completed && !launch.dispatchId) {
      try { launch = await recoverLaunch(cleanupOptions) ?? launch; }
      catch (error) { reason += `; launch recovery failed (${error.message})`; }
    }
    if (!completed && !launch.dispatchId) {
      reason += "; launch could not be confirmed; recover from launch-request.json";
      try {
        const why = join(root, "stop-reason.md");
        await writeFile(why, reason);
        // If no launch reached the ledger, close the unassigned task. `none`
        // is checked atomically and refuses to close any recorded attempt.
        const closed = await ledger(["task-update", "--task", task.taskId, "--attempt", "none",
          "--status", "closed", "--closed-as", "outdated", "--why-file", why,
          "--retry-request", `${retry}-close`], cleanupOptions());
        await writeFile(join(root, "evidence", "task-close.json"), JSON.stringify(closed));
      } catch (error) {
        reason += `; task closure could not be confirmed (${error.message})`;
      }
    }
    // Stop only this attempt. The server checks both --dispatch and human
    // takeover again when applying the stop; never abandon a person's pane.
    if (!completed && mayStop && launch.workerId && launch.dispatchId) {
      const reasonFile = join(root, "stop-reason.md");
      await writeFile(reasonFile, reason ?? "evaluation did not complete");
      let settled = false;
      let stop;
      try {
        const stopWords = ["worker-stop", "--run", request.run, "--worker", launch.workerId,
          "--dispatch", launch.dispatchId, "--reason-file", reasonFile, "--retry-request", `${retry}-stop`];
        const stops = [];
        const stopOnce = async () => {
          stop = await call("zerocode-orc", stopWords, cleanupOptions());
          stops.push(stop);
          await writeFile(join(root, "evidence", "stop.json"), JSON.stringify(stop));
          await writeFile(join(root, "evidence", "stop-attempts.json"), JSON.stringify(stops));
        };
        await stopOnce();
        // A successful bridge response can mean stop_pending, not stopped.
        // Observe settlement before closing the disposable measurement task.
        while (now() < cleanupUntil) {
          const worker = await ledger(["worker-show", "--worker", launch.workerId], cleanupOptions());
          if (worker.takenOver) { mayStop = false; reason = "the person took over the evaluation pane"; break; }
          const state = await ledger(["task-list", "--task", task.taskId, "--brief"], cleanupOptions());
          const held = state.tasks.find(row => row.taskId === task.taskId);
          if (held?.attempt !== launch.dispatchId) break;
          if (held?.status === "completed") { settled = true; break; }
          if (worker.dispatchId !== launch.dispatchId && held?.status !== "dispatched") {
            const closed = await ledger(["task-update", "--task", task.taskId, "--attempt", launch.dispatchId,
              "--status", "closed", "--closed-as", "outdated", "--why-file", reasonFile,
              "--retry-request", `${retry}-close`], cleanupOptions());
            await writeFile(join(root, "evidence", "task-close.json"), JSON.stringify(closed));
            settled = true;
            break;
          }
          if (stop.rc !== 0) {
            if (stops.length === 1 && worker.dispatchId === launch.dispatchId && held?.status === "dispatched") {
              await stopOnce();
              continue;
            }
            break;
          }
          await wait(Math.min(100, Math.max(1, cleanupUntil - now())));
        }
      } catch (error) {
        reason += `; cleanup could not be confirmed (${error.message})`;
      }
      if (!settled && mayStop) reason += stop?.rc !== 0
        ? "; recorded attempt could not be stopped (see stop.json)"
        : "; stop settlement could not be confirmed";
    }
    // A cancelled/timed-out workflow is already false. Do not run another
    // full command budget, or judge files while its worker may still edit.
    const judged = completed
      ? await call("python3", [join(BASE, "fixture.py"), "verify", request.task, root])
      : { rc: null, stdout: "", stderr: "not judged: workflow did not complete" };
    if (completed && (signal?.aborted || remaining() <= 0)) {
      judged.rc = null;
      reason = signal?.aborted ? "evaluation cancelled" : "evaluation timeout";
    }
    await writeFile(join(root, "evidence", "judge-after.log"), `exit_code=${judged.rc}\n${judged.stdout}${judged.stderr}`);
    if (judged.error) reason = `independent judge could not finish: ${judged.error}`;
    const row = realRecord({ request, launch: { ...launch, taskId: task.taskId }, completed,
      elapsedMs: now() - started, judgeRc: judged.rc, repeat, reason });
    rows.push(row);
    await writeFile(join(output, "runs.jsonl"), rows.map(row => JSON.stringify(row)).join("\n") + "\n");
    deadline = undefined;
    // An unverified attempt may still be running if the bridge is down. Do
    // not create another paid worker or erase the partial report on cancel.
    if (!row.verified || signal?.aborted) break;
  }
  const summary = realSummary(rows);
  await writeFile(join(output, "summary.json"), JSON.stringify(summary, null, 2) + "\n");
  await writeFile(join(output, "report.md"), [
    "# Lane B — real provider workflow", "",
    `Verified ${summary.verified}/${summary.runs}. Completion requires both a durable ledger task completion and the independent fixture judge.`, "",
    "| Task | Agent / model / effort | Verified | Elapsed ms | Judge rc |",
    "|---|---|---|---:|---:|",
    ...rows.map(row => `| ${row.task} | ${row.agent} / ${row.model.launched} / ${row.effort} | ${row.verified} | ${row.elapsed_ms} | ${row.judge_rc} |`),
    "", "Elapsed covers launch, task completion, the independent judge and any failure cleanup. The ledger records the launch model; provider-effective model is unobserved.",
    `Runner source: ${request.runnerHead}. The live window build is unobserved; this is not a native candidate-build acceptance.`,
    "Human intervention, tokens, cost, first-visible and memory are unobserved (null). They are not zero and no dollar spend-cap claim is made.",
    "Q1/Q4 measure real coding and worktree completion. They do not measure native UI or live-browser quality.", "",
  ].join("\n"));
  return { output, rows, summary };
}
