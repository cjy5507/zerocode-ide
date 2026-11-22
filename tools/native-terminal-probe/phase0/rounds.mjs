#!/usr/bin/env node
/* Phase 0: whether a timing pass counts. One rule, used by run.sh while it runs
 * the rounds and by report.mjs when it pools them.
 *
 *   node rounds.mjs gate     ROUND_JSON LEVEL ATTEMPT CASES met|skipped "idle=… load1=… thermal=… displayAsleep=… freeGiB=…"
 *   node rounds.mjs pass     ROUND_JSON RENDERER CODE SINCE_MS END_MS IDLE_END_S THERMAL_BEFORE THERMAL_AFTER
 *   node rounds.mjs judge    ROUND_DIR      prints one line a renderer: "dom valid" or "gpui invalid: …"
 *   node rounds.mjs selftest RUN_DIR        the rule on every round of a run, one line each
 *
 * A round is one attempt folder (LEVEL/rN). run.sh writes its round.json: the
 * gate's facts, then for each pass its exit code, its start and end (epoch ms),
 * the HID idle time at its end and macOS's thermal state (0 nominal … 3
 * critical) before and after it.
 *
 * A pass counts when all of these hold:
 *   - it exited 0;
 *   - no keyboard, mouse or trackpad event came during it: the HID idle at its
 *     end is at least its length (a GPUI pass also ends itself on input);
 *   - macOS reported the thermal state nominal before and after it, and for
 *     GPUI before and after every case;
 *   - every expected case ran to its end, each once;
 *   - no case began with the one-minute load above the core count (loud);
 *   - for GPUI: the window was reported visible when the first frame was fed
 *     and never reported not visible after it.
 * A pass made before this rule (no round.json, no visibility or thermal record)
 * does not count. GPUI folds visibility changes and delivers them after the
 * turn in progress, so "visible throughout" means none was reported and no
 * frame stopped coming, not that none happened. */
import { existsSync, readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { basename, join } from "node:path";
import { pathToFileURL } from "node:url";

const RENDERERS = ["dom", "gpui"];

function readJson(path) {
  return existsSync(path) ? JSON.parse(readFileSync(path, "utf8")) : null;
}

/* "idle=612 load1=7.1 thermal=0 displayAsleep=0 freeGiB=21" → numbers by name. */
function facts(line) {
  const out = {};
  for (const part of String(line ?? "").trim().split(/\s+/)) {
    const [name, value] = part.split("=");
    if (name && value !== undefined) out[name] = Number(value);
  }
  return out;
}

function loudAtStart(result) {
  const before = result.before?.power ?? result.before ?? {};
  const load1 = Number(before.load1 ?? String(before.loadavg ?? "").replace(/[{}]/g, "").trim().split(/\s+/)[0]);
  const cores = Number(before.cores ?? 0);
  return Number.isFinite(load1) && cores > 0 && load1 > cores;
}

/* The verdict on one renderer's pass: { valid, reasons }. */
export function judgePass({ renderer, file, pass, expectCases }) {
  const reasons = [];
  if (!pass) {
    reasons.push("no pass record (a run made before the gate)");
  } else {
    if (pass.code !== 0) reasons.push(`exit ${pass.code}${file?.aborted ? ` (${file.aborted})` : ""}`);
    const lengthS = Math.floor((pass.endMs - pass.sinceMs) / 1000);
    if (!(pass.idleEndS >= lengthS)) reasons.push(`input during the pass (HID idle ${pass.idleEndS} s at its end; the pass took ${lengthS} s)`);
    if (pass.thermalBefore !== 0 || pass.thermalAfter !== 0) reasons.push(`thermal state ${pass.thermalBefore} → ${pass.thermalAfter} (0 is nominal)`);
  }
  if (!file) {
    reasons.push("no timing file");
    return { valid: false, reasons };
  }
  if (file.stalled) reasons.push(`ended early: ${file.stalled}`);
  const results = file.results ?? [];
  // A DOM pass writes a case only once it ran to its end; a GPUI pass marks each.
  const complete = results.filter((result) => renderer === "dom" || result.complete === true);
  const names = complete.map((result) => result.case);
  const distinct = new Set(names);
  if (distinct.size !== names.length) reasons.push("a case ran more than once");
  if (expectCases != null && distinct.size !== expectCases) reasons.push(`${distinct.size}/${expectCases} cases ran to their end`);
  const loud = results.filter(loudAtStart).map((result) => result.case);
  if (loud.length) reasons.push(`loud at the start of ${loud.join(", ")}`);
  if (renderer === "gpui") {
    if (!Array.isArray(file.visibility)) {
      reasons.push("no visibility record");
    } else if (file.firstFeedMs == null) {
      reasons.push("no frame was fed");
    } else {
      const first = file.firstFeedMs;
      const before = file.visibility.filter((event) => event.atMs <= first);
      const atFirst = before.length ? before.at(-1).visible : file.window?.visibleAtOpen === true;
      if (atFirst !== true) reasons.push("not reported visible when the first frame was fed");
      const hidden = file.visibility.find((event) => event.atMs > first && event.visible === false);
      if (hidden) reasons.push(`reported not visible ${Math.round(hidden.atMs - first)} ms after the first fed frame`);
    }
    if (results.some((result) => !result.thermal)) {
      reasons.push("no per-case thermal record");
    } else {
      const warm = results
        .filter((result) => result.thermal.before !== "Nominal" || (result.thermal.after != null && result.thermal.after !== "Nominal"))
        .map((result) => result.case);
      if (warm.length) reasons.push(`thermal state not nominal in ${warm.join(", ")}`);
    }
  }
  return { valid: reasons.length === 0, reasons };
}

/* The verdicts on every pass of one round folder, by renderer. */
export function judgeRound(dir) {
  const round = readJson(join(dir, "round.json"));
  const level = round?.level ?? basename(join(dir, ".."));
  const verdicts = {};
  for (const renderer of RENDERERS) {
    const file = readJson(join(dir, renderer, `${renderer}-timing-${level}.json`));
    const pass = round?.passes?.[renderer] ?? null;
    if (!file && !pass) continue;
    verdicts[renderer] = judgePass({ renderer, file, pass, expectCases: round?.expectCases ?? null });
  }
  return { gate: round?.gate ?? null, verdicts };
}

function line(renderer, verdict) {
  return verdict.valid ? `${renderer} valid` : `${renderer} invalid: ${verdict.reasons.join("; ")}`;
}

function main(argv) {
  const [verb, ...rest] = argv;
  if (verb === "gate") {
    const [path, level, attempt, cases, state, factsLine] = rest;
    const gate = { state, ...facts(factsLine), atMs: Date.now() };
    writeFileSync(path, `${JSON.stringify({ level, attempt: Number(attempt), expectCases: Number(cases), gate, passes: {} }, null, 1)}\n`);
    return 0;
  }
  if (verb === "pass") {
    const [path, renderer, code, sinceMs, endMs, idleEndS, thermalBefore, thermalAfter] = rest;
    const round = readJson(path);
    if (!round) throw new Error(`${path}: no round record to add a pass to`);
    round.passes[renderer] = {
      code: Number(code), sinceMs: Number(sinceMs), endMs: Number(endMs), idleEndS: Number(idleEndS),
      thermalBefore: Number(thermalBefore), thermalAfter: Number(thermalAfter),
    };
    writeFileSync(path, `${JSON.stringify(round, null, 1)}\n`);
    return 0;
  }
  if (verb === "judge") {
    const { verdicts } = judgeRound(rest[0]);
    for (const [renderer, verdict] of Object.entries(verdicts)) console.log(line(renderer, verdict));
    return 0;
  }
  if (verb === "selftest") {
    const run = rest[0];
    for (const level of readdirSync(run).filter((name) => /^(normal|low|cpu4x)$/.test(name) && statSync(join(run, name)).isDirectory())) {
      const rounds = readdirSync(join(run, level)).filter((name) => /^r\d+$/.test(name)).sort((a, b) => Number(a.slice(1)) - Number(b.slice(1)));
      for (const round of rounds) {
        const { verdicts } = judgeRound(join(run, level, round));
        for (const [renderer, verdict] of Object.entries(verdicts)) console.log(`${level} ${round} ${line(renderer, verdict)}`);
      }
    }
    return 0;
  }
  console.error("usage: rounds.mjs gate|pass|judge|selftest … (see the head of this file)");
  return 2;
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) process.exitCode = main(process.argv.slice(2));
