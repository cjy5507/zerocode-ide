/* The box a real agent runs in on the form bench (t-41387): what it may call,
 * what it can see of the machine, and how long and how far it may go — as
 * plain data and pure functions, so a test can hold the box to its rules
 * without asking a model.
 *
 * The rules (the coordinator's, m-41406):
 * - the only commands it can call are the bench's stand-in for the window's
 *   door, under the two names the door has (`zerocode-browser`, and
 *   `zerocode-computer` for the person's turn): the tool list is narrowed to
 *   them and the skill that teaches them — never Bash as a whole, never a flag
 *   that skips permission checks;
 * - its environment is built from nothing (`env -i`): the stand-in's folder
 *   and the system's own on PATH, a home and a config folder of its own under
 *   the run's folder, and no login at all — the real-model road is closed until
 *   a person decides how it logs in (m-41479), so nothing here names an account
 *   folder, a token or a key; the real window's commands are not on PATH, and
 *   the run checks that before it starts;
 * - no more than a turn limit and a time limit, and nothing left running.
 */

import { spawnSync } from "node:child_process";
import { accessSync, constants } from "node:fs";
import { mkdir, symlink, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const SHIM = resolve(dirname(fileURLToPath(import.meta.url)), "bin", "zerocode-door");

/* The system's own folders: the shell and curl the stand-in's shim uses. */
export const SYSTEM_PATH = ["/usr/bin", "/bin", "/usr/sbin", "/sbin"];
/* The names the stand-in answers to, and the names of the real window's
 * commands it must be the only road to — the others must not be found at all. */
export const STAND_IN_NAMES = ["zerocode-browser", "zerocode-computer"];
export const REAL_ONLY_NAMES = ["zerocode-emulator", "zerocode-orc", "zerocode-ssh", "zerocode-find"];
/* What the agent may use: Bash for the stand-in alone, and the skill that
 * teaches it — as the window installs the skill into each agent's home. */
export const TOOLS = ["Bash", "Skill"];
export const ALLOWED_TOOLS = ["Bash(zerocode-browser *)", "Bash(zerocode-computer *)", "Skill"];
/* The limits of one run: turns, and seconds. */
export const MAX_TURNS = 60;
export const CAP_SECONDS = 600;
/* Every flag that would widen the box, which no run may carry. */
export const WIDENING_FLAGS = ["--dangerously-skip-permissions", "--allow-dangerously-skip-permissions", "--permission-mode", "--add-dir", "--settings"];

/* The stand-in's folder: the shim under each of the door's two names, and
 * beside it the manual each prints for `--help`, as the core words it
 * (`door_text`'s `usage`). */
export async function installStandIn({ bin, doorText }) {
  await mkdir(bin, { recursive: true });
  for (const door of ["browser", "computer"]) {
    await symlink(SHIM, join(bin, `zerocode-${door}`));
    const manual = JSON.parse(spawnSync(doorText, [], { input: JSON.stringify({ op: "usage", door }), encoding: "utf8" }).stdout);
    await writeFile(join(bin, `${door}.help`), manual.words);
  }
}

/* The arguments of one run of `claude -p`: its prompt arrives on stdin. */
export function claudeArgs({ model, maxTurns = MAX_TURNS }) {
  return ["-p", "--model", model, "--output-format", "stream-json", "--verbose", "--max-turns", String(maxTurns),
    "--tools", TOOLS.join(","), "--allowedTools", ...ALLOWED_TOOLS,
    "--strict-mcp-config", "--mcp-config", JSON.stringify({ mcpServers: {} }), "--no-session-persistence"];
}

/* The whole environment of a run — nothing is inherited, and no login is
 * named: a CLI in a home and a config folder of its own finds none, so it
 * reaches no model. (A route that logs in is a person's decision, m-41479.) */
export function boxEnv({ bin, home, config, tmp, desk, user }) {
  return {
    PATH: [bin, ...SYSTEM_PATH].join(":"),
    HOME: home,
    TMPDIR: tmp,
    USER: user,
    LOGNAME: user,
    LANG: "en_US.UTF-8",
    SHELL: "/bin/zsh",
    CLAUDE_CONFIG_DIR: config,
    FORM_DESK: desk,
    // Nothing leaves for the vendor but the model's own requests.
    CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: "1",
    DISABLE_AUTOUPDATER: "1",
    DISABLE_TELEMETRY: "1",
    DISABLE_ERROR_REPORTING: "1",
  };
}

/* Where a name is found on a PATH, as a shell finds it: the first folder
 * holding an executable of that name, or null. */
export function foundOn(path, name) {
  for (const folder of path.split(":").filter(Boolean)) {
    try {
      accessSync(join(folder, name), constants.X_OK);
      return join(folder, name);
    } catch { /* not here */ }
  }
  return null;
}

/* The check a run makes before it starts, and prints: each stand-in name is
 * found in the stand-in's folder and nowhere before it, and none of the real
 * window's commands is found at all. Answers the lines to print and whether
 * the box holds. */
export function checkPath(path, bin) {
  const lines = [];
  let holds = true;
  for (const name of STAND_IN_NAMES) {
    const at = foundOn(path, name);
    const ours = at === join(bin, name);
    holds = holds && ours;
    lines.push(`PATH ${name} -> ${at ?? "not found"}${ours ? "" : "   REFUSED: not the stand-in"}`);
  }
  for (const name of REAL_ONLY_NAMES) {
    const at = foundOn(path, name);
    holds = holds && at === null;
    lines.push(`PATH ${name} -> ${at ?? "not found"}${at === null ? "" : "   REFUSED: the window's own"}`);
  }
  return { holds, lines };
}

/* The rules of the arguments themselves: the tool list is the narrow one and
 * no flag widens the box. Answers what is wrong, empty when nothing is. */
export function argsProblems(args) {
  const problems = [];
  for (const flag of WIDENING_FLAGS) if (args.includes(flag)) problems.push(`carries ${flag}`);
  // The words after the flag, up to the next flag.
  const allowed = [];
  for (const word of args.slice(args.indexOf("--allowedTools") + 1)) {
    if (word.startsWith("--")) break;
    allowed.push(word);
  }
  if (JSON.stringify(allowed) !== JSON.stringify(ALLOWED_TOOLS)) problems.push(`allows ${JSON.stringify(allowed)}`);
  if (args[args.indexOf("--tools") + 1] !== TOOLS.join(",")) problems.push("offers tools beyond the narrow list");
  if (!args.includes("--strict-mcp-config")) problems.push("loads the machine's MCP servers");
  return problems;
}
