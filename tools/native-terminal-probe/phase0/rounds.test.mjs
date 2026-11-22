/* node --test tools/native-terminal-probe/phase0/rounds.test.mjs — the rule a timing pass is held to. */
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

import { judgePass, judgeRound } from "./rounds.mjs";

const quiet = { cores: 12, loadavg: "{ 4.10 5.00 6.00 }" };
const pass = (over = {}) => ({ code: 0, sinceMs: 1_000_000, endMs: 1_030_000, idleEndS: 640, thermalBefore: 0, thermalAfter: 0, ...over });
const gpuiCase = (name, over = {}) => ({ case: name, complete: true, thermal: { before: "Nominal", after: "Nominal" }, before: quiet, ...over });
const domCase = (name, over = {}) => ({ case: name, before: { power: { load1: 4.1, cores: 12 } }, ...over });
const gpuiFile = (over = {}) => ({
  window: { visibleAtOpen: false },
  visibility: [{ atMs: 0, visible: false }, { atMs: 40, visible: true }],
  firstFeedMs: 120,
  stalled: null,
  results: [gpuiCase("hangul"), gpuiCase("sparse-log")],
  ...over,
});
const domFile = (over = {}) => ({ results: [domCase("hangul"), domCase("sparse-log")], ...over });

test("a clean pass counts", () => {
  assert.deepEqual(judgePass({ renderer: "gpui", file: gpuiFile(), pass: pass(), expectCases: 2 }), { valid: true, reasons: [] });
  assert.deepEqual(judgePass({ renderer: "dom", file: domFile(), pass: pass(), expectCases: 2 }), { valid: true, reasons: [] });
});

test("input during the pass does not count", () => {
  const verdict = judgePass({ renderer: "dom", file: domFile(), pass: pass({ idleEndS: 12 }), expectCases: 2 });
  assert.equal(verdict.valid, false);
  assert.match(verdict.reasons.join(), /input during the pass/);
  assert.equal(judgePass({ renderer: "gpui", file: gpuiFile({ aborted: "input" }), pass: pass({ code: 5 }), expectCases: 2 }).valid, false);
});

test("a window hidden after the first fed frame does not count; one hidden before it does", () => {
  const after = gpuiFile({ visibility: [{ atMs: 0, visible: true }, { atMs: 900, visible: false }] });
  assert.match(judgePass({ renderer: "gpui", file: after, pass: pass(), expectCases: 2 }).reasons.join(), /not visible 780 ms after/);
  assert.equal(judgePass({ renderer: "gpui", file: gpuiFile(), pass: pass(), expectCases: 2 }).valid, true);
});

test("a window never reported visible by the first frame does not count", () => {
  const never = gpuiFile({ visibility: [{ atMs: 0, visible: false }] });
  assert.match(judgePass({ renderer: "gpui", file: never, pass: pass(), expectCases: 2 }).reasons.join(), /not reported visible when the first frame was fed/);
});

test("thermal state, loud cases, missing or repeated cases do not count", () => {
  assert.match(judgePass({ renderer: "dom", file: domFile(), pass: pass({ thermalAfter: 1 }), expectCases: 2 }).reasons.join(), /thermal state 0 → 1/);
  const warm = gpuiFile({ results: [gpuiCase("hangul", { thermal: { before: "Nominal", after: "Fair" } }), gpuiCase("sparse-log")] });
  assert.match(judgePass({ renderer: "gpui", file: warm, pass: pass(), expectCases: 2 }).reasons.join(), /not nominal in hangul/);
  const loud = domFile({ results: [domCase("hangul", { before: { power: { load1: 13.2, cores: 12 } } }), domCase("sparse-log")] });
  assert.match(judgePass({ renderer: "dom", file: loud, pass: pass(), expectCases: 2 }).reasons.join(), /loud at the start of hangul/);
  assert.match(judgePass({ renderer: "dom", file: domFile(), pass: pass(), expectCases: 8 }).reasons.join(), /2\/8 cases/);
  const twice = domFile({ results: [domCase("hangul"), domCase("hangul")] });
  assert.match(judgePass({ renderer: "dom", file: twice, pass: pass(), expectCases: 2 }).reasons.join(), /more than once/);
  const cut = gpuiFile({ results: [gpuiCase("hangul"), gpuiCase("sparse-log", { complete: false, aborted: "hidden" })] });
  assert.match(judgePass({ renderer: "gpui", file: cut, pass: pass(), expectCases: 2 }).reasons.join(), /1\/2 cases/);
});

test("a pass made before the gate does not count", () => {
  const old = { results: [{ case: "hangul", before: quiet }], stalled: null };
  const verdict = judgePass({ renderer: "gpui", file: old, pass: null, expectCases: 1 });
  assert.equal(verdict.valid, false);
  assert.match(verdict.reasons.join(), /no pass record.*no visibility record.*no per-case thermal record/);
});

test("a round folder is judged from its round.json and the renderers' files", () => {
  const dir = mkdtempSync(join(tmpdir(), "phase0-round-"));
  try {
    mkdirSync(join(dir, "gpui"));
    mkdirSync(join(dir, "dom"));
    writeFileSync(join(dir, "round.json"), JSON.stringify({ level: "normal", attempt: 1, expectCases: 2, gate: { state: "met" }, passes: { gpui: pass(), dom: pass({ idleEndS: 3 }) } }));
    writeFileSync(join(dir, "gpui", "gpui-timing-normal.json"), JSON.stringify(gpuiFile()));
    writeFileSync(join(dir, "dom", "dom-timing-normal.json"), JSON.stringify(domFile()));
    const { verdicts } = judgeRound(dir);
    assert.equal(verdicts.gpui.valid, true);
    assert.equal(verdicts.dom.valid, false);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
