#!/usr/bin/env node
/* Phase 0, the numbers: every timing file under a run folder (dom-timing-*.json,
 * gpui-timing-*.json, any depth), pooled by renderer, level and case.
 *
 *   node tools/native-terminal-probe/phase0/report.mjs RUN_DIR [--out DIR]
 *
 * Percentiles come from the raw per-frame samples of every round together (the
 * floor frame 0 left out, as in each file's own summary). Beside them, each
 * round's own p95, so the spread between repeated runs is in the table and a
 * difference between the renderers can be held against it.
 *
 * Only intervals that measure the same span are put side by side:
 *
 *   frame build   DOM: feed → the message posted in that frame's rAF (after its style, layout, paint)
 *                 GPUI: feed → the end of that frame's draw (layout, prepaint, paint into the scene)
 *   apply         DOM: the window's applyFrames (model and DOM writes); GPUI: the screen model update
 *   decode        DOM only: bytes handed over → apply start (TextDecoder, JSON.parse); GPUI has none
 *   submit        GPUI only: feed → the end of the platform submission; the DOM's compositor is not visible
 *   gap           the interval between display frames while frames are fed
 *
 * Neither number includes the GPU's execution or the display's scanout.
 *
 * Only passes that count under rounds.mjs (the rule run.sh measured by) are
 * pooled; every other pass is listed with why it does not count. The renderers
 * are ranked only when each has at least RANK_ROUNDS counted passes at a level. */
import { readFileSync, readdirSync, statSync, writeFileSync, mkdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";

import { judgeRound } from "./rounds.mjs";

const run = resolve(process.argv[2] ?? ".");
const outAt = process.argv.indexOf("--out");
const out = resolve(outAt > 0 ? process.argv[outAt + 1] : run);
const RANK_ROUNDS = Number(process.env.PHASE0_RANK_ROUNDS ?? 5);

function files(dir) {
  const found = [];
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) found.push(...files(path));
    else if (/^(dom|gpui)-timing-.+\.json$/.test(name)) found.push(path);
  }
  return found.sort();
}

const pct = (values) => {
  const sorted = values.filter((v) => typeof v === "number" && Number.isFinite(v)).sort((a, b) => a - b);
  if (!sorted.length) return null;
  const at = (p) => sorted[Math.min(sorted.length - 1, Math.max(0, Math.ceil(sorted.length * p) - 1))];
  const r = (n) => Math.round(n * 100) / 100;
  return { n: sorted.length, p50: r(at(0.5)), p95: r(at(0.95)), p99: r(at(0.99)), max: r(sorted.at(-1)) };
};

/* The steady samples of one result, by interval name. */
function intervals(result, renderer) {
  const steady = result.samples.filter((s) => (renderer === "dom" ? s.fed : s.kind === "feed") && s.k > 0);
  if (renderer === "dom") {
    return {
      frameBuild: steady.map((s) => s.feedToDoneMs),
      apply: steady.map((s) => s.applyMs),
      decode: steady.map((s) => s.decodeMs),
      gap: steady.map((s) => s.gapMs),
      idleGap: result.samples.filter((s) => !s.fed).map((s) => s.gapMs),
      idleFrameBuild: result.samples.filter((s) => !s.fed).map((s) => s.feedToDoneMs),
    };
  }
  return {
    frameBuild: steady.map((s) => s.feedToDrawEndMs),
    submit: steady.map((s) => s.feedToPresentEndMs),
    apply: steady.map((s) => s.applyMs),
    draw: steady.map((s) => s.drawMs),
    paint: steady.map((s) => s.paintMs),
    present: steady.map((s) => s.presentMs),
    gap: steady.map((s) => s.gapMs),
    redrawDraw: result.samples.filter((s) => s.kind === "redraw").map((s) => s.drawMs),
    idleGap: result.samples.filter((s) => s.kind === "idle").map((s) => s.gapMs),
  };
}

const groups = new Map();
const runs = [];
const excluded = [];
const counted = {};
const judged = new Map();
for (const path of files(run)) {
  const file = JSON.parse(readFileSync(path, "utf8"));
  const renderer = file.renderer;
  const level = file.level;
  // The round folder holds round.json beside the renderers' folders.
  const roundDir = dirname(dirname(path));
  if (!judged.has(roundDir)) judged.set(roundDir, judgeRound(roundDir).verdicts);
  const verdict = judged.get(roundDir)[renderer] ?? { valid: false, reasons: ["not judged"] };
  if (!verdict.valid) {
    excluded.push({ path, renderer, level, reasons: verdict.reasons });
    continue;
  }
  counted[`${renderer}|${level}`] = (counted[`${renderer}|${level}`] ?? 0) + 1;
  runs.push({ path, renderer, level, calibrationMs: file.calibration?.ms ?? null, stalled: file.stalled ?? null, engine: file.engine, window: file.window ?? null, scale: file.scale ?? file.window?.scale ?? null });
  for (const result of file.results ?? []) {
    const id = `${result.case}|${level}|${renderer}`;
    const group = groups.get(id) ?? { case: result.case, level, renderer, rounds: [], pooled: {}, flags: { notSameFrame: 0, notOneApply: 0, notDrawn: 0, notSameTurn: 0, loud: 0, frames: 0 }, rss: [] };
    const named = intervals(result, renderer);
    for (const [name, values] of Object.entries(named)) (group.pooled[name] ??= []).push(...values);
    const before = result.before?.power ?? result.before ?? {};
    const load1 = Number(before.load1 ?? String(before.loadavg ?? "").replace(/[{}]/g, "").trim().split(/\s+/)[0]);
    const cores = Number(before.cores ?? 0);
    const loud = Number.isFinite(load1) && cores > 0 && load1 > cores;
    group.rounds.push({ path, round: result.round, frameBuildP95: pct(named.frameBuild)?.p95 ?? null, load1, cores, loud });
    group.flags.loud += loud ? 1 : 0;
    group.flags.frames += result.frames ?? 0;
    const s = result.summary ?? {};
    group.flags.notSameFrame += s.notSameFrame ?? 0;
    group.flags.notOneApply += s.notOneApply ?? 0;
    group.flags.notDrawn += s.notDrawn ?? 0;
    group.flags.notSameTurn += s.notSameTurn ?? 0;
    if (s.rssPeakKiB) group.rss.push(s.rssPeakKiB);
    groups.set(id, group);
  }
}

const table = [...groups.values()].map((group) => ({
  case: group.case,
  level: group.level,
  renderer: group.renderer,
  rounds: group.rounds.length,
  roundP95: group.rounds.map((r) => r.frameBuildP95),
  loudRounds: group.flags.loud,
  flags: group.flags,
  rssPeakKiB: group.rss.length ? Math.max(...group.rss) : null,
  stats: Object.fromEntries(Object.entries(group.pooled).map(([name, values]) => [name, pct(values)])),
}));

const calibration = {};
for (const one of runs) {
  if (one.calibrationMs == null) continue;
  (calibration[`${one.renderer}|${one.level}`] ??= []).push(one.calibrationMs);
}
const median = (xs) => { const s = [...xs].sort((a, b) => a - b); return s.length ? s[s.length >> 1] : null; };
const calib = Object.fromEntries(Object.entries(calibration).map(([key, values]) => [key, Math.round(median(values))]));
const calibRange = Object.fromEntries(Object.entries(calibration).map(([key, values]) => [key, [Math.round(Math.min(...values)), Math.round(Math.max(...values))]]));

// Ranked only where each renderer has enough counted passes at that level (cpu4x has no GPUI pass).
const levels = [...new Set([...Object.keys(counted), ...excluded.map((one) => `${one.renderer}|${one.level}`)].map((key) => key.split("|")[1]))].sort();
const ranking = Object.fromEntries(levels.map((level) => {
  const dom = counted[`dom|${level}`] ?? 0;
  const gpui = counted[`gpui|${level}`] ?? 0;
  return [level, { dom, gpui, ranked: dom >= RANK_ROUNDS && gpui >= RANK_ROUNDS }];
}));

mkdirSync(out, { recursive: true });
writeFileSync(join(out, "report.json"), JSON.stringify({ run, rankRounds: RANK_ROUNDS, ranking, runs, excluded, calibration: calib, calibrationRange: calibRange, table }, null, 1));

const fmt = (stat) => (stat ? `${stat.p50} / ${stat.p95} / ${stat.p99}` : "—");
const spread = (list) => {
  const xs = list.filter((x) => typeof x === "number");
  return xs.length ? `${Math.min(...xs)}–${Math.max(...xs)}` : "—";
};
const md = [
  "# Phase 0 timing",
  "",
  `Run folder: \`${run}\``,
  "",
  ...levels.map((level) => {
    const one = ranking[level];
    return `- ${level}: ${one.dom} DOM and ${one.gpui} GPUI passes count. ${one.ranked ? "Both have at least " + RANK_ROUNDS + ": the renderers may be ranked at this level." : "No ranking at this level: each renderer needs " + RANK_ROUNDS + " passes that count."}`;
  }),
  "",
  "Each cell is p50 / p95 / p99 in ms over every steady frame of every counted pass (frame 0, the full floor, left out). `round p95` is each counted pass's own p95 of the frame build — the spread a difference has to beat.",
  "",
  "| case | level | renderer | rounds | frames | frame build | round p95 | apply | decode (DOM) | submit (GPUI) | gap | redraw unchanged (GPUI draw) | idle gap | flags | RSS peak MiB |",
  "|---|---|---|---:|---:|---|---|---|---|---|---|---|---|---|---:|",
];
for (const row of table.sort((a, b) => a.case.localeCompare(b.case) || a.level.localeCompare(b.level) || a.renderer.localeCompare(b.renderer))) {
  const flags = row.renderer === "dom"
    ? `late ${row.flags.notSameFrame} · ≠1 apply ${row.flags.notOneApply}`
    : `undrawn ${row.flags.notDrawn} · late ${row.flags.notSameTurn}`;
  md.push(`| ${row.case} | ${row.level} | ${row.renderer} | ${row.rounds} | ${row.flags.frames} | ${fmt(row.stats.frameBuild)} | ${spread(row.roundP95)} | ${fmt(row.stats.apply)} | ${fmt(row.stats.decode)} | ${fmt(row.stats.submit)} | ${fmt(row.stats.gap)} | ${fmt(row.stats.redrawDraw)} | ${fmt(row.stats.idleGap)} | ${flags}${row.loudRounds ? ` · loud ${row.loudRounds}` : ""} | ${row.rssPeakKiB ? Math.round(row.rssPeakKiB / 1024) : "—"} |`);
}
md.push("", "Calibration (the plain CPU loop of tools/low-spec/spin.mjs in each renderer's own process, counted passes, median and range in ms; the two runtimes differ, so compare levels within a renderer, not renderers): " + Object.entries(calib).map(([key, ms]) => `${key} ${ms} (${calibRange[key][0]}–${calibRange[key][1]})`).join(", "));
md.push("", "Not measured: the GPU's execution, the display's scanout, and real input (keys, IME, paste, mouse); the DOM is headless Chromium, not the app's WKWebView or WebView2.");
md.push("", "Counted passes:", ...runs.map((one) => `- ${one.renderer} ${one.level}: ${one.path}`));
md.push("", "Passes that do not count:", ...(excluded.length ? excluded.map((one) => `- ${one.renderer} ${one.level}: ${one.path} — ${one.reasons.join("; ")}`) : ["- none"]));
writeFileSync(join(out, "report.md"), `${md.join("\n")}\n`);
console.log(md.join("\n"));
