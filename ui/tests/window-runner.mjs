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

/* The report's words, in one table: the report spells its lines with it, and
 * the release lane's parser (`tools/release/lane.sh` failed_tests) is held to
 * what it writes by tools/release/tests/test_lane.py, which runs this runner
 * and reads its report through that parser. */
export const REPORT = Object.freeze({ pass: "PASS", fail: "FAIL", suite: "SUITE", gap: "  ", detail: "  — " });

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

export function createRunner({ log = console.log, env = process.env, argv = process.argv.slice(2) } = {}) {
  const suites = [];
  const results = [];
  // The suite running now: what the runner's own `ok` credits a check to. A
  // suite's body gets hands of its own that name it whenever they speak.
  let running = null;
  // `WINDOW_TRACE=1` names each check on stderr as it lands — the road to a
  // suite that hangs: the last line named is the check before the hang.
  const record = (suite, name, pass, detail = "") => {
    results.push({ suite, name, pass: !!pass, detail });
    if (env.WINDOW_TRACE) process.stderr.write(`ok: ${pass ? REPORT.pass : REPORT.fail} ${name.slice(0, 110)}\n`);
  };
  const ok = (name, pass, detail = "") => record(running, name, pass, detail);

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
    if (entries.length === 0) return { taken: suites.filter((one) => !one.onlyByName), unmatched: [] };
    const taken = suites.filter((one) => entries.some((entry) => matches(entry, one.name)));
    const unmatched = entries.filter((entry) => !suites.some((one) => matches(entry, one.name)));
    return { taken, unmatched };
  };

  const run = async (context = {}) => {
    const { taken, unmatched } = chosen();
    for (const entry of unmatched) {
      ok(`a chosen suite exists: ${entry}`, false, `known suites: ${suites.map((one) => one.name).join(", ")}`);
    }
    for (const one of taken) {
      running = one.name;
      const hands = (name, pass, detail) => record(one.name, name, pass, detail);
      try {
        await one.body({ ...context, ok: hands, suite: one.name });
      } catch (error) {
        hands(`the ${one.name} suite ran to its end`, false, oneLine(error));
      }
    }
    running = null;
    return { results, failed: results.filter((one) => !one.pass).length };
  };

  /* Prints the report; returns how many checks failed (the exit code's truth).
   * A suite's name heads its lines, again whenever a check of it lands after
   * another suite's; checks of no suite come first, so none reads as another's. */
  const report = () => {
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
  };

  return { suite, ok, run, report, results, chosen };
}
