#!/usr/bin/env node
/* The low-spec profile: every performance number the window harness gives,
 * taken twice — on this machine's full power and on a slow one — with the
 * power state written down beside each run (t-19683).
 *
 *   node tools/low-spec/profile.mjs run --level normal|low|cpu4x [--runs 5] [--start 1] [--probe a,b] [--out DIR]
 *   node tools/low-spec/profile.mjs report DIR_NORMAL DIR_LOW [DIR_CPU4X]   the table, markdown
 *
 * Levels:
 *   normal  the harness as it is.
 *   low     `taskpolicy -b`: the process and everything it launches (node, the
 *           Playwright browser) run on the efficiency cores only, at background
 *           QoS — the closest a 12-core Apple-silicon machine comes to a
 *           small laptop without a second machine. A probe's `calibrate` row
 *           says how much slower this made a plain CPU loop.
 *   cpu4x   no taskpolicy; the page's CPU throttled 4x through CDP
 *           (`WINDOW_CPU_THROTTLE=4`, window-boot.mjs). Chromium only. Why a
 *           second level: `taskpolicy -b` also puts the process under timer
 *           coalescing — headless Chromium's rAF then ticks at ~105 ms, a
 *           figure no slow laptop has (smoke run 2026-10-01, terminal-throughput
 *           idle control p50 105 ms). So `low` shows what a starved, throttled
 *           machine does to the harness, `cpu4x` shows what pure script slowness
 *           does to the page's own costs. Read frame-gap rows from `cpu4x`, CPU
 *           cost rows from either.
 *
 * Every run records: power source and charge, low power mode, thermal warning,
 * one-minute load against the core count (a run started over its core count
 * is flagged `loud`, never silently averaged in). Run it as a job on the
 * build line (`# what:` naming the measurement), one level per job.
 *
 * Caveat: `taskpolicy -b` reaches children that this process starts. Playwright's
 * Chromium is one; WebKit's WebContent helpers start under launchd and may not
 * be — the webkit probe's numbers are therefore an upper bound on the slowdown
 * it shows, not a floor. */
import { spawnSync } from "node:child_process";
import { cpus, loadavg } from "node:os";
import { mkdirSync, readFileSync, readdirSync, writeFileSync, existsSync } from "node:fs";
import { join, resolve } from "node:path";

const ROOT = resolve(new URL("../..", import.meta.url).pathname);
const CORES = cpus().length;
/** One probe's wall: under the build line's 900 s job cap, so a stuck probe ends as a row, not a capped job. */
const PROBE_TIMEOUT_S = 880;
/** One display frame at 60 Hz: a level whose empty page cannot make a frame inside it cannot judge a frame metric. */
export const DISPLAY_FRAME_MS = 16.7;
const WINDOW = (suite) => ({ cmd: ["node", "ui/tests/window.mjs", "--suite", suite] });
const JSON_OUT = (script, ...more) => ({ cmd: ["node", `ui/tests/${script}`, "--rounds", "1", ...more, "--json", "{json}"], json: true });

/** The probes, each one run `--runs` times. `calibrate` first: it is the ruler. */
export const PROBES = {
  calibrate: { cmd: ["node", "tools/low-spec/spin.mjs"] },
  "frame-supply": { cmd: ["node", "tools/low-spec/frame-supply.mjs"] },
  "conversation-perf": JSON_OUT("conversation-perf.mjs"),
  "conversation-perf-webkit": JSON_OUT("conversation-perf.mjs", "--engine", "webkit"),
  "board-live-perf": JSON_OUT("board-live-perf.mjs", "--polls", "30"),
  "coordinator-desk-perf": JSON_OUT("coordinator-desk-perf.mjs", "--polls", "40"),
  "restart-back": WINDOW("restart-same-panes"),
  "terminal-throughput": WINDOW("terminal-throughput"),
  "terminal-cadence": WINDOW("terminal-cadence"),
  "terminal-pacing": WINDOW("terminal-pacing"),
  window: { cmd: ["node", "ui/tests/window.mjs", "--suite", "^window$"] },
};

const sh = (cmd, args) => spawnSync(cmd, args, { encoding: "utf8" }).stdout ?? "";

/** The power state a run was taken in. */
export function powerState() {
  const batt = sh("pmset", ["-g", "batt"]);
  const all = sh("pmset", ["-g"]);
  const therm = sh("pmset", ["-g", "therm"]);
  const load = loadavg()[0];
  return {
    source: /AC Power/.test(batt) ? "AC" : /Battery Power/.test(batt) ? "battery" : "unknown",
    charge: (batt.match(/(\d+)%/) ?? [])[1] ?? null,
    // Newer macOS says `powermode 1` (0 automatic, 1 low power, 2 high power); older says `lowpowermode 1`.
    lowPowerMode: /^\s*(?:low)?powermode\s+1\b/m.test(all),
    thermalWarning: /CPU_Speed_Limit\s*=\s*(\d+)/.test(therm) ? Number(therm.match(/CPU_Speed_Limit\s*=\s*(\d+)/)[1]) < 100 : false,
    load1: Number(load.toFixed(2)),
    cores: CORES,
    loud: load > CORES,
    chip: sh("sysctl", ["-n", "machdep.cpu.brand_string"]).trim(),
    memGiB: Math.round(Number(sh("sysctl", ["-n", "hw.memsize"])) / 2 ** 30),
  };
}

const NUM = "-?\\d+(?:\\.\\d+)?";

/** Every number a harness line carries, named: `"key":1.5` pairs and `label: 12 ms` pairs. */
export function numbersOf(text) {
  const found = {};
  for (const line of text.split("\n")) {
    if (!/^(METRIC|MEASURE|PASS|FAIL)\b/.test(line)) continue;
    const title = line.replace(/^(METRIC|MEASURE|PASS|FAIL)\s+/, "").split(/[:—(]/)[0].trim().slice(0, 60);
    for (const [, key, value] of line.matchAll(new RegExp(`"([A-Za-z0-9_]+)"\\s*:\\s*(${NUM})(?![\\d.])`, "g"))) found[`${title} · ${key}`] = Number(value);
    for (const [, label, value] of line.matchAll(new RegExp(`([A-Za-z][A-Za-z /-]{2,40}?)\\s+(${NUM})\\s*ms\\b`, "g"))) found[`${title} · ${label.trim()} ms`] = Number(value);
  }
  return found;
}

/** Every number of a --json summary, flattened. */
function flatten(value, prefix = "", into = {}) {
  if (typeof value === "number" && Number.isFinite(value)) into[prefix] = value;
  else if (value && typeof value === "object" && !Array.isArray(value)) for (const [key, one] of Object.entries(value)) flatten(one, prefix ? `${prefix}.${key}` : key, into);
  return into;
}

export const checksOf = (text) => text.split("\n").filter((l) => /^(PASS|FAIL)\b/.test(l)).map((l) => ({ pass: l.startsWith("PASS"), name: l.replace(/^(PASS|FAIL)\s+/, "").split(" — ")[0].slice(0, 120) }));

function runProbe(name, level, index, dir) {
  const probe = PROBES[name];
  const jsonPath = join(dir, `${name}.${index}.json`);
  const cmd = probe.cmd.map((one) => one.replace("{json}", jsonPath));
  const argv = level !== "low" ? cmd : ["taskpolicy", "-b", ...cmd];
  const env = { ...process.env, ...(level === "cpu4x" ? { WINDOW_CPU_THROTTLE: "4" } : {}) };
  const power = powerState();
  const started = Date.now();
  const done = spawnSync(argv[0], argv.slice(1), { cwd: ROOT, env, encoding: "utf8", maxBuffer: 1 << 28, timeout: PROBE_TIMEOUT_S * 1000 });
  const log = `${done.stdout ?? ""}\n${done.stderr ?? ""}`;
  writeFileSync(join(dir, `${name}.${index}.log`), log);
  const numbers = numbersOf(log);
  if (probe.json && existsSync(jsonPath)) {
    const parsed = JSON.parse(readFileSync(jsonPath, "utf8"));
    Object.assign(numbers, flatten(parsed.summary ?? parsed));
  }
  const spin = log.match(/CALIBRATE (\d+(?:\.\d+)?) ms/);
  if (spin) numbers["calibrate · loop ms"] = Number(spin[1]);
  const checks = checksOf(log);
  for (const [, what] of log.matchAll(/^NOT ZERO: (.+)$/gm)) checks.push({ pass: false, name: `NOT ZERO: ${what}` });
  if (probe.json && existsSync(jsonPath)) {
    const budget = JSON.parse(readFileSync(jsonPath, "utf8")).summary?.budget ?? JSON.parse(readFileSync(jsonPath, "utf8")).budget ?? {};
    for (const [key, held] of Object.entries(budget)) if (held === false) checks.push({ pass: false, name: `budget ${key}` });
  }
  if (done.error?.code === "ETIMEDOUT" || done.signal) checks.push({ pass: false, name: `did not finish in ${PROBE_TIMEOUT_S} s (${done.signal ?? "timeout"})` });
  return { probe: name, level, index, rc: done.status ?? "timeout", wallMs: Date.now() - started, power, checks, numbers };
}

function run(argv) {
  const opt = (name, fallback) => { const at = argv.indexOf(name); return at >= 0 ? argv[at + 1] : fallback; };
  const level = opt("--level", "normal");
  if (!["normal", "low", "cpu4x"].includes(level)) throw new Error(`--level normal|low|cpu4x, not ${level}`);
  const runs = Number(opt("--runs", "5"));
  const names = (opt("--probe", "") || Object.keys(PROBES).join(",")).split(",").filter(Boolean);
  const dir = resolve(opt("--out", join(ROOT, "output/low-spec", `${new Date().toISOString().replace(/[:.]/g, "-")}-${level}`)));
  mkdirSync(dir, { recursive: true });
  // A level's runs may arrive as several jobs into one directory: they append.
  const rows = existsSync(join(dir, "runs.json")) ? JSON.parse(readFileSync(join(dir, "runs.json"), "utf8")) : [];
  const first = Number(opt("--start", "1"));
  for (const name of names) {
    if (!PROBES[name]) throw new Error(`unknown probe ${name}; known: ${Object.keys(PROBES).join(", ")}`);
    for (let index = first; index < first + runs; index += 1) {
      const row = runProbe(name, level, index, dir);
      // Read again before writing: another job of the same level may have appended meanwhile
      // (an in-flight job once wrote back its stale copy and brought stripped rows back, 2026-10-01).
      const now = existsSync(join(dir, "runs.json")) ? JSON.parse(readFileSync(join(dir, "runs.json"), "utf8")) : [];
      now.push(row);
      writeFileSync(join(dir, "runs.json"), JSON.stringify(now, null, 1));
      console.log(`${level} ${name} #${index}: rc ${row.rc}, ${(row.wallMs / 1000).toFixed(1)} s, ${row.power.source}${row.power.lowPowerMode ? " LPM" : ""}, load ${row.power.load1}/${CORES}${row.power.loud ? " LOUD" : ""}, ${row.checks.filter((c) => !c.pass).length} failing`);
    }
  }
  console.log(`runs written to ${dir}/runs.json`);
}

const median = (xs) => { const s = [...xs].sort((a, b) => a - b); const m = s.length >> 1; return s.length ? (s.length % 2 ? s[m] : (s[m - 1] + s[m]) / 2) : null; };
const fmt = (n) => (n === null || n === undefined ? "—" : Math.abs(n) >= 100 ? n.toFixed(0) : n.toFixed(2).replace(/\.?0+$/, ""));

/** The budgets the suites hold, by metric-name pattern — read from the code, cited. */
export const BUDGETS = [
  [/frame|paint|scroll.*p95|over16|over20/i, "none in the suite (conversation-perf says it is not a gate); frame budget 96 ms exists only in board-live-perf, unjudged when load > cores (machine-load.mjs)"],
];

function report(dirs) {
  const loaded = dirs.map((dir) => JSON.parse(readFileSync(join(resolve(dir), "runs.json"), "utf8")));
  const levels = loaded.map((rows) => rows[0]?.level ?? "?");
  const keys = new Map();
  for (const rows of loaded) for (const row of rows) for (const key of Object.keys(row.numbers)) keys.set(`${row.probe} | ${key}`, true);
  // The control: a level whose empty page cannot make a frame inside one display frame
  // (DISPLAY_FRAME_MS) cannot judge a frame metric.
  const supply = loaded.map((rows) => median(rows.filter((r) => r.probe === "frame-supply").map((r) => r.numbers["frame supply · controlGapP50"]).filter((n) => n !== undefined)));
  const FRAME_ROW = /scroll|over16|over20|frame|프레임|놓친|gap/i;
  const out = ["Frame supply (empty page rAF gap p50, ms): " + levels.map((l, at) => `${l} ${fmt(supply[at])}${supply[at] > DISPLAY_FRAME_MS ? ` (${(1000 / supply[at]).toFixed(0)} Hz)` : ""}`).join(", "), "",
    "| probe | metric | " + levels.join(" | ") + " | budget | low-spec |", "|---|---|" + levels.map(() => "---:").join("|") + "|---|---|"];
  for (const full of keys.keys()) {
    const [probe, metric] = full.split(" | ");
    const cells = loaded.map((rows) => median(rows.filter((r) => r.probe === probe).map((r) => r.numbers[metric]).filter((n) => n !== undefined)));
    const budget = BUDGETS.find(([pattern]) => pattern.test(metric))?.[1] ?? "none";
    const lowRows = (loaded[1] ?? []).filter((r) => r.probe === probe);
    const failing = lowRows.filter((r) => r.checks.some((c) => !c.pass)).length;
    out.push(`| ${probe} | ${metric} | ${cells.map((n, at) => (FRAME_ROW.test(metric) && supply[at] > DISPLAY_FRAME_MS ? `${fmt(n)} (판정 불가: 프레임 공급 ${(1000 / supply[at]).toFixed(0)}Hz)` : fmt(n))).join(" | ")} | ${budget} | ${lowRows.length ? (failing ? `FAIL ${failing}/${lowRows.length} runs` : "pass") : "—"} |`);
  }
  out.push("", "Checks that fail on a level but pass on normal:");
  for (const full of new Set(loaded.flatMap((rows) => rows.map((r) => r.probe)))) {
    const names = (rows) => new Set(rows.filter((r) => r.probe === full).flatMap((r) => r.checks.filter((c) => !c.pass).map((c) => c.name)));
    const base = names(loaded[0]);
    loaded.slice(1).forEach((rows, at) => { for (const n of names(rows)) if (!base.has(n)) out.push(`- [${full}] ${levels[at + 1]}: ${n}`); });
  }
  out.push("", "Runs: " + loaded.map((rows, at) => `${levels[at]} ${rows.length} (${[...new Set(rows.map((r) => r.power.source + (r.power.lowPowerMode ? "+LPM" : "")))].join("/")}, loud ${rows.filter((r) => r.power.loud).length}, rc≠0 ${rows.filter((r) => r.rc).length})`).join("; "));
  console.log(out.join("\n"));
}

if (process.argv[1] && resolve(process.argv[1]) === new URL(import.meta.url).pathname) {
  const [command, ...rest] = process.argv.slice(2);
  if (command === "run") run(rest);
  else if (command === "report") report(rest);
  else { console.error("usage: profile.mjs run --level normal|low|cpu4x [--runs 5] [--probe a,b] [--out DIR] | report DIR_NORMAL DIR_LOW [DIR_CPU4X]"); process.exit(2); }
}
