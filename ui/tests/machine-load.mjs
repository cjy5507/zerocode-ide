// The machine's load, read once per run — the one place a frame budget asks
// whether a wall-clock number can be judged at all.
//
// A frame budget (a worst gap, a highlight frame) measures this machine, not
// the page: three worker builds beside the harness put the same page over the
// same budget by 2 ms (97.8 vs 96, 2026-09-23) and by 100 ms under load 66.
// So a budget is judged only on a quiet machine — one whose one-minute load
// is at most its core count — and is otherwise recorded, never failed. The
// functional part of every such test (paths lit, nodes drawn, contracts) is
// still judged. See the release lane's own calm wait (`lane.sh`), which uses
// the same reading.
import { cpus, loadavg } from "node:os";

/** The worst frame gap a scene may show on a quiet machine, in ms: twelve frames of eight. */
export const FRAME_BUDGET_MS = 12 * 8;

const LOAD_1_MIN = loadavg()[0];
const CORES = cpus().length;

/** True when the one-minute load exceeds the core count — a budget cannot be judged. */
export const machineIsLoud = LOAD_1_MIN > CORES;

/** A budget holds when it is met, or when the machine is too loud to judge it. */
export function frameBudgetHolds(ms, limitMs = FRAME_BUDGET_MS) {
  return ms < limitMs || machineIsLoud;
}

/** The load reading, for a METRIC line: `load 3.2/12` or `load 41.0/12 (loud — budget unjudged)`. */
export function loadNote() {
  const reading = `load ${LOAD_1_MIN.toFixed(1)}/${CORES}`;
  return machineIsLoud ? `${reading} (loud — budget unjudged)` : reading;
}
