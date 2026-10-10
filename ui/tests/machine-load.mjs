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
//
// A platform that cannot read its load is never quiet by reading: Node's loadavg is [0, 0, 0] on Windows, so every
// budget there is recorded and never judged (t-43414). Unknown is said, never judged calm — the rule lane.sh's
// wait_for_calm keeps.
import { cpus, loadavg } from "node:os";

/** The worst frame gap a scene may show on a quiet machine, in ms: twelve frames of eight. */
export const FRAME_BUDGET_MS = 12 * 8;

const CORES = cpus().length;

/** The one-minute load right now — read when a budget is judged, not once at
 * import: a suite that runs for minutes beside worker builds sees the load
 * move (11 → 37 within one knowledge run, 2026-09-24), and a reading taken at
 * import judged a 100 ms frame against a quiet-machine budget that no longer
 * applied. */
export function loadNow() {
  return loadavg()[0];
}

/** The import-time reading, for a runner that judges a whole run by the load it started under. */
export const machineIsLoud = machineIsLoudNow();

/** Whether a platform's own load reading means anything. Node's loadavg is [0, 0, 0] on Windows, so a "quiet" reading
 * there is no reading at all (t-43414). */
export function loadReadable(platform = process.platform) {
  return platform !== "win32";
}

/** Why a budget cannot be judged right now, or null when it can. This is the one rule every frame budget asks: a
 * platform that cannot read its load is never quiet ("unreadable"), and a machine whose one-minute load is above its
 * core count is too loud ("loud"). The platform and the load are this machine's unless a test passes its own. */
function unjudgedBecause({ platform = process.platform, load = loadNow() } = {}) {
  if (!loadReadable(platform)) return "unreadable";
  if (load > CORES) return "loud";
  return null;
}

/** True when the machine cannot judge a budget right now (see unjudgedBecause). */
export function machineIsLoudNow(opts = {}) {
  return unjudgedBecause(opts) !== null;
}

/** A budget holds when it is met, or when the machine cannot judge it. */
export function frameBudgetHolds(ms, limitMs = FRAME_BUDGET_MS, opts = {}) {
  return ms < limitMs || machineIsLoudNow(opts);
}

/** The load reading, for a METRIC line: `load 3.2/12`, `load 41.0/12 (loud — budget unjudged)`, or on a platform that
 * cannot read it, `load unreadable on win32 — budget unjudged`. */
export function loadNote(opts = {}) {
  const { platform = process.platform, load = loadNow() } = opts;
  const why = unjudgedBecause({ platform, load });
  if (why === "unreadable") return `load unreadable on ${platform} — budget unjudged`;
  const reading = `load ${load.toFixed(1)}/${CORES}`;
  return why === "loud" ? `${reading} (loud — budget unjudged)` : reading;
}
