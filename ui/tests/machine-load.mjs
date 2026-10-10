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
//
// A GitHub-hosted runner is a shared VM: its loadavg does not show the host's contention, so a quiet reading there
// says nothing about speed. Its absolute frame budgets are recorded and never judged (t-43414, run-4275's rule); the
// same env pair is what justfile's windows package smoke reads (justfile:218). The ratio of a round's own pair still
// judges, and so does a self-hosted runner.
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

/** The one name for a GitHub-hosted runner, by the env pair justfile:218 reads. Read when asked; a test passes its own. */
export function onHostedRunner(env = process.env) {
  return env.GITHUB_ACTIONS === "true" && env.RUNNER_ENVIRONMENT === "github-hosted";
}

/** A budget holds when it is met, or when it cannot be judged: the machine cannot judge it (see unjudgedBecause), or the
 * runner is GitHub-hosted and an absolute budget there is recorded, never judged (onHostedRunner). */
export function frameBudgetHolds(ms, limitMs = FRAME_BUDGET_MS, opts = {}) {
  if (onHostedRunner(opts.env)) return true;
  return ms < limitMs || machineIsLoudNow(opts);
}

/** The load reading, for a METRIC line: `load 3.2/12`, `load 41.0/12 (loud — budget unjudged)`, `load 3.2/3 (hosted runner
 * — budget unjudged)`, or on a platform that cannot read it, `load unreadable on win32 — budget unjudged`. The numbers stay
 * on the line on every runner: only the judgement is withheld. */
export function loadNote(opts = {}) {
  const { platform = process.platform, load = loadNow() } = opts;
  const why = unjudgedBecause({ platform, load });
  if (why === "unreadable") return `load unreadable on ${platform} — budget unjudged`;
  const reading = `load ${load.toFixed(1)}/${CORES}`;
  if (onHostedRunner(opts.env)) return `${reading} (hosted runner — budget unjudged)`;
  return why === "loud" ? `${reading} (loud — budget unjudged)` : reading;
}
