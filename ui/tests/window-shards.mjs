/* The window harness split across several browsers at once (`WINDOW_SHARDS=<n>`).
 *
 * The harness is one process that takes its suites one after another in one browser: 1,701 s for 216 suites on a
 * quiet Mac (2026-11-10), the root gate's longest recipe. Split four ways the same suites took 647 s that day. A
 * suite does not change: each share is this same script with its share named (window-runner.mjs `SHARD_ENV`), its
 * own file server and its own browser, and it runs the suites of its share as a whole run would.
 *
 * What the lane reads stays one report (window-runner.mjs `REPORT`). A share prints none; it leaves its checks in a
 * file, and the run that started the shares prints them all as one report, suite by suite. A share that died or was
 * cut does not let a suite pass unseen: every suite of its plan that never ended is a FAIL under that suite's own
 * name, so the lane judges it alone (WINDOW_SUITES, solo ×3) as it judges any red suite. A run that names its suites
 * — `WINDOW_SUITES`, `--suite`, an `<NAME>_ONLY` switch, the lane's solo judgment among them — is never split, and
 * neither is a run that does not ask: `WINDOW_SHARDS` unset is one browser, as before. */
import { spawn } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { SHARD_ENV, SHARD_OUT_ENV, shardOf, writeReport } from "./window-runner.mjs";

export const SHARDS_ENV = "WINDOW_SHARDS";

/* Whether a run names the suites it takes. */
export const choosesSuites = (env = process.env, argv = process.argv.slice(2)) =>
  (env.WINDOW_SUITES ?? "").trim() !== "" ||
  argv.some((word) => word === "--suite" || word.startsWith("--suite=")) ||
  Object.keys(env).some((name) => name.endsWith("_ONLY") && env[name]);

/* How many shares this run is split into: `WINDOW_SHARDS` when it asks for more than one and is neither a share
 * itself nor a run that names its suites; one otherwise. */
export function shardCount(env = process.env, argv = process.argv.slice(2)) {
  const asked = Number.parseInt(env[SHARDS_ENV] ?? "", 10);
  if (!Number.isInteger(asked) || asked <= 1 || shardOf(env) !== null || choosesSuites(env, argv)) return 1;
  return asked;
}

/* Start `count` shares of `script`, wait for every one, print one report of all their checks, and answer the exit
 * code — 1 when a check failed, a share's suite went missing or the run was cut. A share's own output (everything
 * but a report) is passed on whole when that share ends, so two shares' lines never interleave. `WINDOW_SUITE_TIMES`
 * gets every share's suites, as a whole run writes them. */
export async function runShards({ count, script, argv = [], env = process.env, log = console.log,
  out = process.stdout, err = process.stderr, spawnShare = spawn } = {}) {
  const folder = await mkdtemp(join(tmpdir(), "window-shards-"));
  const began = Date.now();
  const children = [];
  const { WINDOW_SUITE_TIMES: timesFile, ...shareEnv } = env;
  const ended = Array.from({ length: count }, (_, at) => new Promise((done) => {
    const file = join(folder, `share-${at + 1}.json`);
    const child = spawnShare(process.execPath, [script, ...argv], {
      env: { ...shareEnv, [SHARD_ENV]: `${at + 1}/${count}`, [SHARD_OUT_ENV]: file },
      stdio: ["ignore", "pipe", "pipe"],
    });
    children.push(child);
    const said = [];
    const heard = [];
    child.stdout.on("data", (bytes) => said.push(bytes));
    child.stderr.on("data", (bytes) => heard.push(bytes));
    child.on("close", (code, signal) => {
      out.write(Buffer.concat(said));
      err.write(Buffer.concat(heard));
      err.write(`window shard ${at + 1}/${count}: ended ${signal ?? `rc ${code}`} after ${Math.round((Date.now() - began) / 1000)} s\n`);
      done({ index: at + 1, file, code, signal });
    });
  }));
  /* A cut reaches every share, and the report still says what landed (window-runner.mjs `reportWhenCut`). */
  let cut = null;
  const handlers = ["SIGINT", "SIGTERM"].map((signal) => {
    const handler = () => {
      cut = signal;
      for (const child of children) child.kill(signal);
    };
    process.once(signal, handler);
    return [signal, handler];
  });
  const shares = await Promise.all(ended);
  for (const [signal, handler] of handlers) process.removeListener(signal, handler);
  const { results, times } = await mergeShares(shares, count);
  if (cut !== null) results.push({ suite: null, name: "the run reached its end", pass: false, detail: `cut by ${cut}` });
  if (timesFile) await writeFile(timesFile, `${JSON.stringify(times, null, 1)}\n`);
  const failed = writeReport(results, log);
  await new Promise((done) => out.write("", done));
  await rm(folder, { recursive: true, force: true });
  return failed > 0 || cut !== null ? 1 : 0;
}

/* Every share's checks as one list in the run's order of suites, and a FAIL for each thing that would otherwise go
 * missing: a share that left no plan, a split the shares did not agree on, a planned suite that never ended. */
export async function mergeShares(shares, count) {
  const records = await Promise.all(shares.map(async (share) => {
    try {
      return { ...share, record: JSON.parse(await readFile(share.file, "utf8")) };
    } catch {
      return { ...share, record: null };
    }
  }));
  const ending = (share) => `share ${share.index}/${count} ended ${share.signal ?? `rc ${share.code}`}`;
  const results = [];
  const order = records.find((share) => share.record !== null)?.record.order ?? [];
  for (const share of records.filter((one) => one.record === null)) {
    results.push({ suite: null, name: `the window share ${share.index}/${count} ran to its end`, pass: false,
      detail: `${ending(share)} before it planned its suites` });
  }
  const planned = records.flatMap((share) => share.record?.plan ?? []);
  const whole = records.every((share) => share.record === null ||
      JSON.stringify(share.record.order) === JSON.stringify(order)) &&
    planned.length === new Set(planned).size &&
    (records.some((share) => share.record === null) || planned.length === order.length);
  if (!whole) {
    results.push({ suite: null, name: "the window shares split the run whole", pass: false,
      detail: `${planned.length} suites planned across the shares, ${new Set(planned).size} distinct, against the ${order.length} the run takes` });
  }
  const bySuite = new Map(order.map((name) => [name, []]));
  const outside = [];
  for (const share of records) {
    for (const result of share.record?.results ?? []) (bySuite.get(result.suite) ?? outside).push(result);
    const done = new Set(share.record?.ended ?? []);
    for (const suite of share.record?.plan ?? []) {
      if (done.has(suite)) continue;
      (bySuite.get(suite) ?? outside).push({ suite, name: `the ${suite} suite ran to its end`, pass: false,
        detail: `its ${ending(share)} before the suite did` });
    }
  }
  const timesBySuite = new Map(records.flatMap((share) => share.record?.times ?? []).map((one) => [one.suite, one]));
  return {
    results: [...results, ...outside, ...[...bySuite.values()].flat()],
    times: order.filter((name) => timesBySuite.has(name)).map((name) => timesBySuite.get(name)),
  };
}
