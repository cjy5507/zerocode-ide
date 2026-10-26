/* This machine's own Jev count for the dashboard's evidence (t-5807, t-73988).
 *
 * Kept apart from `jev-dashboard.mjs` so a test can hold it to its promise
 * without a browser: what the gate sees must not depend on whose home it runs
 * in. */
import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

/* The most the whole step may wait on zo, both asks together: well under the
 * window gate's 30 s per check, so a zo that does not answer leaves the
 * fixture in instead of failing the gate. */
const ASK_BUDGET_MS = 10_000;

/* This machine's own ledgers, counted by the zo a person NAMED in JEV_ZO_BIN
 * over the project JEV_PROJECT_DIR names (else `root`), with the recent list
 * when that zo knows the flag. With no zo named it is `null`: the gate does
 * not run the zo installed in the home it happens to run in, and does not
 * read that home's `~/.zo` (t-73988). `null` too when the named zo does not
 * answer in time — the fixture then stands in, and the evidence says so. */
export function realSummary({ env = process.env, home = homedir(), root = process.cwd() } = {}) {
  const zo = env.JEV_ZO_BIN;
  if (!zo || !existsSync(zo)) return null;
  const project = env.JEV_PROJECT_DIR || root;
  const sessions = env.JEV_SESSIONS_DIR
    || join(home, "Library", "Application Support", "dev.zerocode.app", "computer-use", "sessions");
  const base = ["jev", "summary", "--json", "--cwd", project];
  if (existsSync(sessions)) base.push("--computer-use", sessions);
  const until = Date.now() + ASK_BUDGET_MS;
  for (const args of [[...base, "--recent", "12"], base]) {
    const left = until - Date.now();
    if (left <= 0) break;
    const ran = spawnSync(zo, args, { encoding: "utf8", timeout: left });
    if (ran.status !== 0) continue;
    try {
      const seats = JSON.parse(ran.stdout).seats;
      if (Array.isArray(seats) && seats.length > 0) return { seats, zo, recent: args.includes("--recent") };
    } catch {
      // An answer this reader does not understand: try the next shape.
    }
  }
  return null;
}
