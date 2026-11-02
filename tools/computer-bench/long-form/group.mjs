/* A child with everything under it, ended as one (t-89544).
 *
 * `child.kill()` signals the one pid, so an agent's tool commands and a browser it
 * started run on after the bench is done. A group spawned here is ended whole, and
 * ends with this process however this process ends, short of a KILL: a timer's
 * ALRM, a TERM, an interrupt, a closed terminal or plain leaving. */

import { spawn } from "node:child_process";

const GRACE_MS = 5000;
const ENDING_SIGNALS = ["SIGTERM", "SIGINT", "SIGHUP", "SIGALRM"];
const SIGNAL_EXIT_BASE = 128;

const groups = new Set();

function signalGroup(child, name) {
  try {
    process.kill(-child.pid, name);
    return true;
  } catch {
    return false; // the group is gone
  }
}

let installed = false;
function endEveryGroupWithThisProcess() {
  if (installed) return;
  installed = true;
  process.on("exit", () => {
    for (const child of groups) signalGroup(child, "SIGKILL");
  });
  for (const name of ENDING_SIGNALS) {
    process.once(name, () => {
      for (const child of groups) signalGroup(child, "SIGTERM");
      const number = { SIGTERM: 15, SIGINT: 2, SIGHUP: 1, SIGALRM: 14 }[name];
      process.exit(SIGNAL_EXIT_BASE + number);
    });
  }
}

/** `spawn`, leading a process group of its own, tied to this process. */
export function spawnGroup(command, args, options = {}) {
  endEveryGroupWithThisProcess();
  const child = spawn(command, args, { ...options, detached: true });
  groups.add(child);
  child.once("exit", () => {
    // What it left running is ended with it; the group's name is free once empty.
    signalGroup(child, "SIGTERM");
    setTimeout(() => {
      signalGroup(child, "SIGKILL");
      groups.delete(child);
    }, GRACE_MS).unref();
  });
  return child;
}

/** TERM the group now, KILL whatever stands after the grace. */
export function endGroup(child, graceMs = GRACE_MS) {
  if (!signalGroup(child, "SIGTERM")) return;
  setTimeout(() => signalGroup(child, "SIGKILL"), graceMs).unref();
}
