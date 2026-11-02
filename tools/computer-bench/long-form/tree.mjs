/* A child with everything under it, ended as one (t-89544).
 *
 * `child.kill()` signals the one pid, so an agent's tool commands and a browser it
 * started run on after the bench is done. A child spawned here is ended with its
 * whole tree, each pid found by its parent, never by a name: TERM, a grace, then
 * KILL for what still stands. That happens on a cap, on every signal this process
 * can catch (a timer's ALRM, a TERM, an interrupt, a closed terminal), and on plain
 * leaving.
 *
 * The child stays in this process's group, as cargo-turn keeps its command: whoever
 * ends the bench's group (the build line's cap and leftover sweep, an interrupt at
 * a terminal) reaches the server and the agent as well. A group of their own hid
 * them from all of that. A KILL of this process cannot be caught; what it leaves is
 * still in that group, and the form server leaves by itself when its stdin closes.
 * What a child leaves after it ended on its own was moved under pid 1 and is no
 * longer found by parent: it too is still in the group. */

import { spawn, spawnSync, execFileSync } from "node:child_process";

const GRACE_MS = 5000;
const STEP_MS = 100;
const ENDING_SIGNALS = { SIGTERM: 15, SIGINT: 2, SIGHUP: 1, SIGALRM: 14 };
const SIGNAL_EXIT_BASE = 128;

const children = new Set();

function childrenOf(pid) {
  try {
    const listed = execFileSync("pgrep", ["-P", String(pid)], { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] });
    return listed.split(/\s+/).filter(Boolean).map(Number);
  } catch {
    return []; // pgrep answers 1 when nothing is under it
  }
}

/** `pid` and everything under it, the deepest first. */
function treeOf(pid) {
  return [...childrenOf(pid).flatMap(treeOf), pid];
}

function send(pids, name) {
  for (const pid of pids) {
    try {
      process.kill(pid, name);
    } catch {
      // already gone
    }
  }
}

/** The pids that still run: a zombie has ended and only waits for its parent. */
function running(pids) {
  if (!pids.length) return [];
  const listed = spawnSync("ps", ["-o", "pid=,stat=", "-p", pids.join(",")], { encoding: "utf8" }).stdout || "";
  return listed.split("\n").map((line) => line.trim().split(/\s+/)).filter(([pid, stat]) => pid && stat && !stat.startsWith("Z")).map(([pid]) => Number(pid));
}

function standing(child) {
  return child.exitCode === null && child.signalCode === null;
}

const sleep = (ms) => new Promise((done) => setTimeout(done, ms));

/** TERM the child's tree now, KILL whatever of it stands after the grace. */
export async function endTree(child, graceMs = GRACE_MS) {
  if (!standing(child)) return;
  const pids = treeOf(child.pid);
  send(pids, "SIGTERM");
  const deadline = Date.now() + graceMs;
  while (Date.now() < deadline && running(pids).length) await sleep(STEP_MS);
  send(running(pids), "SIGKILL");
}

/** The same, for the moment this process leaves: nothing can wait on a timer there. */
function endTreeNow(child, graceMs = GRACE_MS) {
  if (!standing(child)) return;
  const pids = treeOf(child.pid);
  send(pids, "SIGTERM");
  const deadline = Date.now() + graceMs;
  const pause = new Int32Array(new SharedArrayBuffer(4));
  while (Date.now() < deadline && running(pids).length) Atomics.wait(pause, 0, 0, STEP_MS);
  send(running(pids), "SIGKILL");
}

let installed = false;
function endEveryTreeWithThisProcess() {
  if (installed) return;
  installed = true;
  process.on("exit", () => {
    for (const child of children) endTreeNow(child);
  });
  let ending = false;
  for (const [name, number] of Object.entries(ENDING_SIGNALS)) {
    process.on(name, async () => {
      if (ending) return; // a second signal does not cut the first one's grace short
      ending = true;
      await Promise.all([...children].map((child) => endTree(child)));
      process.exit(SIGNAL_EXIT_BASE + number);
    });
  }
}

/** `spawn`, tied to this process: its tree ends when this process does. */
export function spawnTied(command, args, options = {}) {
  endEveryTreeWithThisProcess();
  const child = spawn(command, args, options);
  children.add(child);
  child.once("exit", () => children.delete(child));
  return child;
}

/** The form server, which also leaves by itself when this process's end closes its stdin. */
export function spawnFormServer(server, out) {
  return spawnTied("python3", [server, "--out", out, "--until-stdin-closes"], { stdio: ["pipe", "pipe", "inherit"] });
}
