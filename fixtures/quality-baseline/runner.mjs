import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { access, mkdir, readFile, writeFile } from "node:fs/promises";
import { delimiter, dirname, join, resolve } from "node:path";
import { homedir } from "node:os";
import { fileURLToPath } from "node:url";
import { run } from "./window-tools.mjs";
export const BASE = dirname(fileURLToPath(import.meta.url));
export const ROOT = resolve(BASE, "../..");
export const table = JSON.parse(await readFile(join(BASE, "baseline.json"), "utf8"));
export function options(args) {
  const result = { lane: "a", repeat: table.Limits.default_repeat, task: null };
  for (let i = 0; i < args.length; i += 2) {
    assert(["--lane", "--task", "--repeat"].includes(args[i]) && args[i + 1], "usage: just baseline [--lane a] [--task Q1..Q6] [--repeat k]");
    result[args[i].slice(2)] = args[i + 1];
  }
  assert.equal(result.lane, "a", "Lane B is defined but not implemented or authorized; no provider was contacted");
  assert(!result.task || table.Tasks[result.task], "unknown task");
  result.repeat = Number(result.repeat);
  assert(Number.isInteger(result.repeat) && result.repeat > 0 && result.repeat <= table.Limits.max_repeat, "invalid repeat");
  return result;
}
export async function newOutput() {
  const path = join(BASE, "out", `${new Date().toISOString().replaceAll(":", "-")}-${process.pid}`);
  await mkdir(path, { recursive: true });
  return path;
}
export async function build(output) {
  const wrapper = join(homedir(), "Library/Caches/dev.zerocode.app/build-coordination/run.py");
  let coordinated = true;
  try { await access(wrapper); } catch { assert.equal(process.env.CI, "true", "local build coordinator is required"); coordinated = false; }
  const cargo = (args, cwd) => coordinated
    ? run("python3", [wrapper, "cargo", ...args], { cwd })
    : run("cargo", args, { cwd });
  const shims = join(output, "shims");
  const transport = await cargo(["run", "-p", "zerocode-core", "--example", "quality_baseline_shims", "--", shims], ROOT);
  await writeFile(join(output, "shims-build.log"), transport.stdout + transport.stderr);
  process.env.PATH = `${shims}${delimiter}${process.env.PATH}`;
  const result = await cargo(["test", "-p", "zo-ide", "--test", "quality_baseline", "--no-run", "--message-format=json"], join(ROOT, "zo-ide"));
  await writeFile(join(output, "build.log"), result.stdout + result.stderr);
  const artifacts = result.stdout.split("\n").flatMap(line => { try { return [JSON.parse(line)]; } catch { return []; } });
  const executable = artifacts.find(row => row.reason === "compiler-artifact" && row.target.name === "quality_baseline" && row.executable)?.executable;
  assert(executable, "quality_baseline test executable was not built");
  return executable;
}
export async function execute(executable, args, env) {
  const code = await new Promise((done, reject) => {
    const child = spawn(executable, args, { cwd: ROOT, env: { ...process.env, ...env }, stdio: "inherit" });
    child.on("error", reject); child.on("close", done);
  });
  assert.equal(code, 0, `${executable} exited ${code}`);
}
export async function zo(executable, output, task, repeat) {
  await execute(executable, ["--test-threads=1"], { BASELINE_OUT: output, BASELINE_REPEAT: String(repeat), BASELINE_TASK: task ?? "" });
}
export async function report(output) {
  const rows = (await readFile(join(output, "runs.jsonl"), "utf8")).trim().split("\n").filter(Boolean).map(JSON.parse);
  const models = [...new Set(rows.map(row => row.account.provider + ":" + row.model.effective))].sort();
  const tasks = [...new Set(rows.map(row => row.task))].sort();
  let text = "# Lane A — deterministic, zero provider spend\n\n";
  text += "Each cell: verified k/k · interventions (sum) · elapsed ms (min of N) · tokens in/out/cached (sum) · queue max · paint p95 ms (min of N) · live heap MiB (max sampled) · host RAM GiB. First-visible means first PTY render byte after submission, including composer echo.\n\n";
  text += `| Task | ${models.join(" | ")} |\n|---|${models.map(() => "---").join("|")}|\n`;
  for (const task of tasks) {
    const cells = models.map(model => {
      const selected = rows.filter(row => row.task === task && `${row.account.provider}:${row.model.effective}` === model);
      if (!selected.length) return "—";
      const sum = f => selected.reduce((n, row) => n + f(row), 0);
      const min = f => Math.min(...selected.map(f)).toFixed(3);
      const max = f => Math.max(...selected.map(f));
      return `${selected.filter(row => row.verified).length}/${selected.length} · ${sum(r => r.interventions)} · ${Math.min(...selected.map(r => r.elapsed_ms))} ms · ${sum(r => r.tokens.in)}/${sum(r => r.tokens.out)}/${sum(r => r.tokens.cached)} · q ${max(r => r.stream.queue_max)} · paint ${min(r => r.stream.paint_p95_ms)} ms · heap ${max(r => r.rss_peak_mb).toFixed(3)} MiB · RAM ${(max(r => r.host.mem) / table.Limits.bytes_per_mb / 1024).toFixed(1)} GiB`;
    });
    text += `| ${task} | ${cells.join(" | ")} |\n`;
  }
  text += "\nEvery row is validated with baseline::Fields (serde). zo paint is frame draw-to-flush time (probe enabled); queue counts pending render-channel blocks plus rendered lines. Window paint is pane submission-to-engine-Paint completion, from CDP timeline marks; queue counts submitted pane frames awaiting paint. Window rows use an isolated pane replay of each verified task result, not native WKWebView timing. The existing rss_peak_mb field is the maximum sampled turn-end live malloc bytes from exact heap --showSizes classes, not process RSS or a continuous high-water mark; window rows take the larger zo/renderer sample, not their sum. Chromium PartitionAlloc/V8 mappings are outside its malloc histogram. Heap collection overhead is excluded from workflow elapsed time. Host CPU is architecture; host RAM is bytes from hw.memsize. Heap sampling currently requires macOS and fails explicitly elsewhere (no RSS fallback). Q5 is a scripted window tool sequence and consumes zero model tokens. Browser and ledger transports use isolated deterministic endpoints; Chromium renders real DOM/PNG, while native WKWebView and live ledger persistence are outside lane A. Q3 destroys the renderer, restores the interrupted pane and resumes a SIGTERM-interrupted zo conversation. Lane B's grid and spend cap are defined only; no real provider runs occurred.\n";
  await writeFile(join(output, "report.md"), text);
  return rows;
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const opts = options(process.argv.slice(2));
  const output = await newOutput();
  console.log(`Baseline evidence: ${output}`);
  try {
    const binary = await build(output);
    await zo(binary, output, opts.task, opts.repeat);
    await execute("node", [join(ROOT, "ui/tests/baseline.mjs"), "--out", output, ...(opts.task ? ["--task", opts.task] : []), "--repeat", String(opts.repeat)], {});
    await execute(binary, ["record_schema_is_closed_and_secret_free", "--test-threads=1"], { BASELINE_VALIDATE: join(output, "runs.jsonl") });
    const rows = await report(output);
    assert(rows.every(row => row.verified && row.interventions === 0), "failed baseline verdict");
    console.log(join(output, "report.md"));
  } catch (error) {
    try { await report(output); } catch { /* Build/setup failed before any run record existed. */ }
    throw error;
  }
}
