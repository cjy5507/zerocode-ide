/* This machine's own Jev count for the dashboard's evidence (t-5807, t-73988).
 *
 * Kept apart from `jev-dashboard.mjs` so a test can hold it to its promise
 * without a browser: what the gate sees must not depend on whose home it runs
 * in. */
import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

/* This machine's own ledgers, counted by the zo the window would run
 * (`~/.local/bin/zo`, or JEV_ZO_BIN) over the project JEV_PROJECT_DIR names
 * (else `root`), with the recent list when that zo knows the flag.
 * `null` when no zo answers — the fixture then stands in, and the evidence
 * says so. */
export function realSummary({ env = process.env, home = homedir(), root = process.cwd() } = {}) {
  const zo = env.JEV_ZO_BIN || join(home, ".local", "bin", "zo");
  if (!existsSync(zo)) return null;
  const project = env.JEV_PROJECT_DIR || root;
  const sessions = env.JEV_SESSIONS_DIR
    || join(home, "Library", "Application Support", "dev.zerocode.app", "computer-use", "sessions");
  const base = ["jev", "summary", "--json", "--cwd", project];
  if (existsSync(sessions)) base.push("--computer-use", sessions);
  for (const args of [[...base, "--recent", "12"], base]) {
    const ran = spawnSync(zo, args, { encoding: "utf8", timeout: 20_000 });
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
