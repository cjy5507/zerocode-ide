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
//
// A frame that a slower machine slows along with everything else is judged against references measured in the same
// run instead (referencesHold, t-63638): the paths frame at 1020 nodes read 84 ms and 121.8 ms on the same code nine
// minutes apart on 10-18, both at loads under the core count. A GitHub-hosted runner records that verdict too until CI
// runs show its margins (referencesNote says which way it went). Power and memory pressure are noted beside such a
// verdict (powerNote, memoryNote) and never judge it.
import { execFileSync } from "node:child_process";
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

/** The midpoint of a frame's two references measured in the same run, in ms: the cheaper one costs what the frame is
 * meant to cost, the dearer one is the design the frame replaced. */
export function referenceMidpoint(lowMs, highMs) {
  return (lowMs + highMs) / 2;
}

/** A frame judged against two references measured in the same run, in the same moments, holds below their midpoint,
 * or when the machine cannot judge (see unjudgedBecause). A machine that slows the page slows the frame and both
 * references alike, so only the frame's own cost crosses the midpoint. A GitHub-hosted runner records the verdict, as
 * it records an absolute budget: the frame and its references are separate readings, and a shared VM's steal time can
 * land on one of them (run-11955). */
export function referencesHold(ms, lowMs, highMs, opts = {}) {
  if (onHostedRunner(opts.env)) return true;
  return ms < referenceMidpoint(lowMs, highMs) || machineIsLoudNow(opts);
}

/** The margin of a frame judged against two references, midpoint minus frame in ms, for a METRIC line: `margin 8ms`.
 * The midpoint is rounded as the line prints it, so the margin is the difference of two printed numbers. On a
 * GitHub-hosted runner it also says which way the recorded verdict went:
 * `margin -58ms, crossed (hosted runner — recorded)`. */
export function referencesNote(ms, lowMs, highMs, opts = {}) {
  const midpoint = Math.round(referenceMidpoint(lowMs, highMs) * 100) / 100;
  const reading = `margin ${Math.round((midpoint - ms) * 100) / 100}ms`;
  if (!onHostedRunner(opts.env)) return reading;
  return `${reading}, ${ms < referenceMidpoint(lowMs, highMs) ? "held" : "crossed"} (hosted runner — recorded)`;
}

/** A command's output, or "" when it cannot run: a note then says unknown instead of failing the run. */
function outputOf(command, args) {
  try {
    return execFileSync(command, args, { encoding: "utf8", timeout: 5000, stdio: ["ignore", "pipe", "ignore"] });
  } catch {
    return "";
  }
}

/** The power source (`pmset -g batt`) and the power mode (`pmset -g`: `powermode` 0 automatic, 1 low power, 2 high
 * power, or `lowpowermode` on older systems), for a METRIC line: `power AC, powermode 0`. No load reading shows the
 * power mode — on 09-29 battery low-power mode slowed the knowledge suite about 40% at the same load — so a timing line
 * says it in pmset's own words. The pmset text is this machine's unless a test passes its own. */
export function powerNote({ platform = process.platform, settings, battery } = {}) {
  if (platform !== "darwin") return `power unreadable on ${platform}`;
  const mode = /^\s*((?:low)?powermode)\s+(\d+)/m.exec(settings ?? outputOf("pmset", ["-g"]));
  const source = /drawing from '([^']+)'/.exec(battery ?? outputOf("pmset", ["-g", "batt"]))?.[1];
  const named = { "AC Power": "AC", "Battery Power": "battery" }[source] ?? source ?? "unknown";
  return `power ${named}, ${mode ? `${mode[1]} ${mode[2]}` : "powermode unknown"}`;
}

/** The kernel's memory-pressure level (`kern.memorystatus_vm_pressure_level`: 1 normal, 2 warn, 4 critical) and the
 * swap in use (`vm.swapusage`), for a METRIC line: `memory normal, swap 4750/6144 MB`. No load reading shows paging
 * either — t-62848's heap drift (10-18) ran with 4.75 of 6.14 GB of swap in use — so a timing line says it. The sysctl
 * text is this machine's unless a test passes its own. */
export function memoryNote({ platform = process.platform, level, swap } = {}) {
  if (platform !== "darwin") return `memory unreadable on ${platform}`;
  const read = level ?? outputOf("sysctl", ["-n", "kern.memorystatus_vm_pressure_level"]);
  const pressure = { 1: "normal", 2: "warn", 4: "critical" }[Number(read)] ?? "unknown";
  const usage = swap ?? outputOf("sysctl", ["-n", "vm.swapusage"]);
  const megabytes = (name) => {
    const hit = new RegExp(`${name} = ([\\d.]+)([MG])`).exec(usage);
    return hit ? Math.round(Number(hit[1]) * (hit[2] === "G" ? 1024 : 1)) : null;
  };
  const used = megabytes("used");
  const total = megabytes("total");
  return `memory ${pressure}, swap ${used === null || total === null ? "unknown" : `${used}/${total} MB`}`;
}
