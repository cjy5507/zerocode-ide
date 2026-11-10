/* The window harness's one runner (t-4017).
 *
 * `ui/tests/window.mjs` used to be a single top-level flow: `ok()` pushed
 * into an array, the report was printed at the file's end, and an uncaught
 * exception anywhere lost every line after it AND the report — the lane saw
 * a red recipe with no failure name to judge solo (2b641238). Twelve
 * `*_ONLY` blocks each copied the selection and the report loop.
 *
 * This is the one place that registers suites, chooses which run, collects
 * their checks, turns a suite's uncaught exception into one FAIL line and
 * goes on to the next suite, and prints the report the lane and the
 * coordinator's watch read as text:
 *
 *   SUITE  <suite>
 *   PASS  <name>  — <detail>
 *   FAIL  <name>  — <detail>
 *
 *   N/M passed
 *
 * Each check stands under the name of the suite that made it (t-9741): a lane
 * that reads one FAIL line re-runs that suite alone (`WINDOW_SUITES`), where
 * the whole harness took eight minutes a run. A check made outside every
 * suite — a chosen name that matched none — comes first, under no name.
 *
 * The exit code is the caller's (`process.exit(report() ? 1 : 0)`), so the
 * runner itself stays testable without leaving the process.
 *
 * Choosing suites — by name or by pattern (a JavaScript regular expression
 * tested against the name), several separated by commas:
 *
 *   WINDOW_SUITES=workers node ui/tests/window.mjs
 *   node ui/tests/window.mjs --suite workers --suite 'editor-.*'
 *   EDITOR_RECOVERY_ONLY=1 node ui/tests/window.mjs   (the older switch, derived)
 *
 * A suite registered with `{ onlyByName: true }` is skipped by a default
 * run and reached only by choosing it — for focused iteration whose checks
 * the full run covers elsewhere. */

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

/* The report's words, in one table: the report spells its lines with it, and
 * the release lane's parser (`tools/release/lane.sh` failed_tests) is held to
 * what it writes by tools/release/tests/test_lane.py, which runs this runner
 * and reads its report through that parser. */
export const REPORT = Object.freeze({ pass: "PASS", fail: "FAIL", suite: "SUITE", gap: "  ", detail: "  — " });

/* A run split across several browsers at once (window-shards.mjs) is several processes of this runner, each told
 * its share: `WINDOW_SHARD=<i>/<n>` is the i-th of n, and `WINDOW_SHARD_OUT` the file it leaves its checks in for
 * the run that started it — a share prints no report of its own. */
export const SHARD_ENV = "WINDOW_SHARD";
export const SHARD_OUT_ENV = "WINDOW_SHARD_OUT";

/* The time each suite took in a whole run, as `WINDOW_SUITE_TIMES` writes it: what the split is planned from. A
 * suite the file does not name weighs what its middle suite does. `WINDOW_SHARD_WEIGHTS` names another file. */
export const WEIGHTS_FILE = join(dirname(fileURLToPath(import.meta.url)), "window-suite-weights.json");

/* `{ index, count }` for a process started as one share (`2/4` is the second of four), else null. */
export function shardOf(env = process.env) {
  const said = /^(\d+)\/(\d+)$/.exec(env[SHARD_ENV] ?? "");
  if (said === null) return null;
  const index = Number(said[1]);
  const count = Number(said[2]);
  return index >= 1 && index <= count ? { index, count } : null;
}

/* The weights a times file holds, by suite. A file that cannot be read plans with every suite the same weight. */
export function readWeights(file = WEIGHTS_FILE) {
  try {
    return new Map(JSON.parse(readFileSync(file, "utf8")).map((one) => [one.suite, one.ms]));
  } catch {
    return new Map();
  }
}

/* The split: `names`, in registration order, into `count` shares — heaviest first into the share lightest so far,
 * ties to the earlier suite and the lower share, so every process plans the same split from the same suites. Each
 * share keeps its suites in registration order. */
export function planShares(names, weights, count) {
  const known = [...weights.values()].filter(Number.isFinite).sort((a, b) => a - b);
  const middle = known.length > 0 ? known[Math.floor(known.length / 2)] : 1;
  const weight = (name) => (Number.isFinite(weights.get(name)) ? weights.get(name) : middle);
  const order = new Map(names.map((name, at) => [name, at]));
  const heaviestFirst = [...names].sort((a, b) => weight(b) - weight(a) || order.get(a) - order.get(b));
  const shares = Array.from({ length: count }, () => ({ load: 0, names: [] }));
  for (const name of heaviestFirst) {
    const lightest = shares.reduce((low, share) => (share.load < low.load ? share : low));
    lightest.names.push(name);
    lightest.load += weight(name);
  }
  return shares.map((share) => share.names.sort((a, b) => order.get(a) - order.get(b)));
}

/* The report the lane reads: a suite's name heads its lines, again whenever a check of it lands after another
 * suite's; checks of no suite come first, so none reads as another's. Answers how many checks failed. One writer
 * for a whole run and for the run that prints its shares' checks as one (window-shards.mjs). */
export function writeReport(results, log = console.log) {
  let failed = 0;
  let under = null;
  const loose = results.filter((one) => one.suite === null);
  for (const result of [...loose, ...results.filter((one) => one.suite !== null)]) {
    if (!result.pass) failed += 1;
    if (result.suite !== null && result.suite !== under) {
      under = result.suite;
      log(`${REPORT.suite}${REPORT.gap}${under}`);
    }
    const detail = result.detail ? `${REPORT.detail}${result.detail}` : "";
    log(`${result.pass ? REPORT.pass : REPORT.fail}${REPORT.gap}${result.name}${detail}`);
  }
  log("");
  log(`${results.length - failed}/${results.length} passed`);
  return failed;
}

/* `editor-recovery` → `EDITOR_RECOVERY_ONLY`: the switch each focused block
 * used to read, derived from the suite's name rather than kept as a table. */
export const legacyOnlyName = (name) => `${name.toUpperCase().replace(/-/g, "_")}_ONLY`;

/* The `--suite <name>` / `--suite=<name>` arguments, in order. */
const suitesOnCommandLine = (argv) => {
  const chosen = [];
  for (let at = 0; at < argv.length; at += 1) {
    if (argv[at] === "--suite" && at + 1 < argv.length) {
      chosen.push(argv[at += 1]);
    } else if (argv[at].startsWith("--suite=")) {
      chosen.push(argv[at].slice("--suite=".length));
    }
  }
  return chosen;
};

/* A stack as one line: the report is one line per result, and the lane's
 * readers take `^FAIL ` lines — a stack's own lines must not become more. */
const oneLine = (error) =>
  String(error?.stack ?? error).split("\n").map((line) => line.trim()).filter(Boolean).join(" | ");

/* How long a suite may go without landing a check before the run calls it stuck, says the last check it landed and goes
 * on (2026-11-02): a scene waiting on something that never comes held the whole run, with no line to read, until the
 * gate's cap cut it. It is a silence, not a total: the shared flow (`window`, 999 checks) takes some 430 s on a quiet
 * Mac and about 560 s on the hosted macOS runner, and a 600 s cap on its whole length cut it on a Windows runner at
 * its 975th check while it was still landing checks (run 41582467888; its last 24 checks all passed after the cut).
 * The longest gap between two of its checks is 13 s locally, so 120 s of silence is a scene that is not coming back —
 * and it is said sooner than the old total ever said it. `WINDOW_SUITE_STALL_MS` moves it. A suite run only by name is
 * a measurement and takes none. */
export const SUITE_STALL_MS = 120_000;

/* How long a cut run gives the browser's own handler to close it before it ends by itself. The gate's cap kills
 * eight seconds after its interrupt; the browser's exit hook takes what is still open with it. */
const CUT_GRACE_MS = 2_000;

/* The exit code a run cut by a signal ends with: 128 and the signal's number, as a shell reads it. */
const CUT_CODES = { SIGINT: 130, SIGTERM: 143, SIGALRM: 142 };

export function createRunner({ log = console.log, env = process.env, argv = process.argv.slice(2) } = {}) {
  const suites = [];
  const results = [];
  /* This process's share of a split run (`WINDOW_SHARD`), or null: then it runs every suite a run takes. A share
   * keeps the suites the run takes (`order`), its own of them (`plan`) and the ones that ended, for its file. */
  const share = shardOf(env);
  const shareFile = share !== null ? env[SHARD_OUT_ENV] : undefined;
  const ended = [];
  let split = { order: [], plan: [] };
  // The suite running now: what the runner's own `ok` credits a check to. A
  // suite's body gets hands of its own that name it whenever they speak.
  let running = null;
  // `WINDOW_TRACE=1` names each check on stderr as it lands — the road to a
  // suite that hangs: the last line named is the check before the hang.
  // The running suite's stall watch, re-armed by each check that suite lands (see SUITE_STALL_MS).
  let heard = () => {};
  const record = (suite, name, pass, detail = "") => {
    results.push({ suite, name, pass: !!pass, detail });
    if (suite !== null && suite === running) heard();
    if (env.WINDOW_TRACE) process.stderr.write(`ok: ${pass ? REPORT.pass : REPORT.fail} ${name.slice(0, 110)}\n`);
  };
  const ok = (name, pass, detail = "") => record(running, name, pass, detail);
  /* Each suite's wall time and how many checks it landed, in the order the run took them. The report says nothing of
   * time — it is the lane's text contract — so this stands beside it: what a split of the run across several browsers
   * is planned from, and where a run that grew slower is read (`WINDOW_SUITE_TIMES`, window.mjs). */
  const times = [];

  const suite = (name, body, { onlyByName = false } = {}) => {
    if (suites.some((one) => one.name === name)) throw new Error(`suite "${name}" is already registered`);
    suites.push({ name, body, onlyByName });
  };

  /* What was asked for, as written: names or patterns. Empty means a default run. */
  const asked = () => [
    ...(env.WINDOW_SUITES ?? "").split(",").map((entry) => entry.trim()).filter(Boolean),
    ...suitesOnCommandLine(argv),
    ...suites.filter((one) => env[legacyOnlyName(one.name)]).map((one) => one.name),
  ];

  const matches = (entry, name) => {
    if (entry === name) return true;
    try {
      return new RegExp(entry).test(name);
    } catch {
      return false;
    }
  };

  /* The suites this run takes, in registration order — and the entries that
   * named nothing, so a typo is a FAIL rather than a quiet 0/0. */
  const chosen = () => {
    const entries = asked();
    if (entries.length === 0 && share !== null) {
      const whole = suites.filter((one) => !one.onlyByName);
      const order = whole.map((one) => one.name);
      const plan = planShares(order, readWeights(env.WINDOW_SHARD_WEIGHTS || WEIGHTS_FILE), share.count)[share.index - 1];
      return { taken: whole.filter((one) => plan.includes(one.name)), unmatched: [], order, plan };
    }
    if (entries.length === 0) return { taken: suites.filter((one) => !one.onlyByName), unmatched: [] };
    const taken = suites.filter((one) => entries.some((entry) => matches(entry, one.name)));
    const unmatched = entries.filter((entry) => !suites.some((one) => matches(entry, one.name)));
    return { taken, unmatched };
  };

  /* A share's file: its split, every check it landed, the suites that ended and their times — written before the
   * first suite (so a share that dies leaves its plan) and again at its report, or its cut. */
  const leave = () => {
    if (shareFile) writeFileSync(shareFile, `${JSON.stringify({ ...split, results, ended, times })}\n`);
  };

  const run = async (context = {}) => {
    const { taken, unmatched, order = [], plan = [] } = chosen();
    split = { order, plan };
    leave();
    for (const entry of unmatched) {
      ok(`a chosen suite exists: ${entry}`, false, `known suites: ${suites.map((one) => one.name).join(", ")}`);
    }
    for (const one of taken) {
      running = one.name;
      const hands = (name, pass, detail) => record(one.name, name, pass, detail);
      const limit = one.onlyByName ? 0 : Number(env.WINDOW_SUITE_STALL_MS ?? SUITE_STALL_MS);
      let clock = null;
      const overdue = new Promise((_, refuse) => {
        if (limit <= 0) return;
        const arm = () => {
          clearTimeout(clock);
          clock = setTimeout(() => {
            heard = () => {};
            const last = results.findLast((result) => result.suite === one.name)?.name ?? "none";
            refuse(new Error(`still not done: no check landed for ${limit} ms; the last check it landed: ${last.slice(0, 110)}`));
          }, limit);
        };
        heard = arm;
        arm();
      });
      const began = performance.now();
      try {
        await Promise.race([one.body({ ...context, ok: hands, suite: one.name }), overdue]);
      } catch (error) {
        heard = () => {};
        hands(`the ${one.name} suite ran to its end`, false, oneLine(error));
      } finally {
        heard = () => {};
        clearTimeout(clock);
        times.push({ suite: one.name, ms: Math.round(performance.now() - began),
          checks: results.filter((result) => result.suite === one.name).length });
        ended.push(one.name);
      }
    }
    running = null;
    return { results, failed: results.filter((one) => !one.pass).length };
  };

  /* Prints the report (`writeReport`); returns how many checks failed (the exit code's truth). A share prints none:
   * it leaves its checks in its file, and the run that started it prints them with the other shares'. */
  const report = () => {
    if (shareFile) {
      leave();
      return results.filter((one) => !one.pass).length;
    }
    return writeReport(results, log);
  };

  /* A run cut from outside — the gate's cap, a person's Ctrl-C — prints what landed and how far it got, and ends red,
   * where it used to end with no report at all (2026-11-02). */
  const reportWhenCut = () => {
    for (const signal of Object.keys(CUT_CODES)) {
      process.once(signal, () => {
        const last = results.at(-1)?.name ?? "none";
        record(running, "the run reached its end", false, `cut by ${signal}; the last check it landed: ${last.slice(0, 110)}`);
        report();
        process.exitCode = CUT_CODES[signal];
        setTimeout(() => process.exit(CUT_CODES[signal]), CUT_GRACE_MS);
      });
    }
  };

  return { suite, ok, run, report, results, times, chosen, reportWhenCut };
}
