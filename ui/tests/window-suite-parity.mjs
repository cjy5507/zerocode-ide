/* A suite alone says what it says in the whole run (t-4017).
 *
 * The point of a suite on its own page is that WHERE it runs no longer decides
 * WHAT it measures. This is that claim, checked: run the suite alone, run the
 * whole harness, and every check the suite reports must carry the same verdict
 * in both — same names, same PASS/FAIL. The seconds each run took are printed
 * beside the verdicts, so the solo run's cost is a number and not a feeling.
 *
 *   node ui/tests/window-suite-parity.mjs                      the workers suite
 *   node ui/tests/window-suite-parity.mjs --suite editor-recovery
 *   node ui/tests/window-suite-parity.mjs --full out/full.log   reuse a full run's log
 *
 * Exit 0 when the verdicts agree, 1 when they differ or the suite reported
 * nothing. Not a gate recipe: it runs the whole harness once on its own. */
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const HARNESS = resolve(HERE, "window.mjs");

const argument = (flag, fallback) => {
  const at = process.argv.indexOf(flag);
  return at >= 0 && at + 1 < process.argv.length ? process.argv[at + 1] : fallback;
};
const suite = argument("--suite", "workers");
const fullLog = argument("--full", null);

/* `PASS  <name>  — <detail>` / `FAIL  <name>  — <detail>`, the runner's own
 * line shape — the detail is cut at the runner's separator (two spaces, a
 * dash, a space), which a name's own dashes never wear. */
const verdicts = (text) => {
  const seen = new Map();
  for (const line of text.split("\n")) {
    const said = /^(PASS|FAIL)  (.*)$/.exec(line);
    if (!said) continue;
    seen.set(said[2].split("  — ")[0], said[1]);
  }
  return seen;
};

const run = (env) => {
  const started = Date.now();
  const child = spawnSync(process.execPath, [HARNESS], {
    env: { ...process.env, ...env }, encoding: "utf8", maxBuffer: 64 * 1024 * 1024, stdio: ["ignore", "pipe", "pipe"],
  });
  return { text: child.stdout + child.stderr, code: child.status, seconds: (Date.now() - started) / 1000 };
};

const solo = run({ WINDOW_SUITES: suite });
console.log(`TIME solo ${suite} ${solo.seconds.toFixed(1)} s (exit ${solo.code})`);
const full = fullLog
  ? { text: readFileSync(fullLog, "utf8"), code: null, seconds: null }
  : run({ WINDOW_SUITES: "" });
if (full.seconds !== null) console.log(`TIME full ${full.seconds.toFixed(1)} s (exit ${full.code})`);

const alone = verdicts(solo.text);
const whole = verdicts(full.text);
const differences = [];
for (const [name, verdict] of alone) {
  const inFull = whole.get(name);
  if (inFull === undefined) differences.push(`missing from the full run: ${name}`);
  else if (inFull !== verdict) differences.push(`${verdict} alone, ${inFull} in the full run: ${name}`);
}
if (alone.size === 0) differences.push(`the ${suite} suite reported nothing alone`);
for (const line of differences) console.log(`DIFF ${line}`);
const tally = (seen) => `${[...seen.values()].filter((one) => one === "PASS").length}/${seen.size} passed`;
console.log(
  `PARITY ${suite}: ${alone.size} checks, ${differences.length === 0 ? "verdicts identical" : `${differences.length} differ`}` +
  ` (alone ${tally(alone)}; full run ${tally(whole)})`,
);
process.exit(differences.length === 0 ? 0 : 1);
