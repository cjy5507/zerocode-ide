/* One run of an agent on a form scene (t-41387): the scene's page in the
 * bench's own Chromium behind the stand-in window (door-desk.mjs), and
 * `claude -p` — the official CLI — handed the launch prompt the product builds (the person's task and card
 * values, then the paragraphs `with_agent_selection_contract` adds), the
 * computer-use skill as the window installs it, and nothing else
 * (agent-box.mjs holds the rules). It measures wall time, the model's requests
 * and tokens, the door's round trips and the oracle's verdict on what the page
 * took, and leaves the run's own folder for a reader.
 *
 *   node real.mjs --scene DIR --model ID --out DIR --door-text PATH --claude PATH
 *                 [--max-turns N] [--cap-seconds N] [--spent FILE --runs-max N] [--dry]
 *
 * CLOSED (m-41479): no login is named — the box holds none, so the CLI reaches
 * no model — and `--account` is refused before anything starts. The real-model
 * road opens only when a person decides how it logs in (a key made for
 * measuring, or an account used for nothing else); until then this runs
 * against a stand-in for the CLI, which is how its own test holds it.
 *
 * `--dry` builds the box and says what it would launch (the arguments, the
 * names of the environment, the check of PATH) without starting a page or the
 * agent. Exit 0 once the run is over, whatever the oracle says of it; 2 for
 * arguments, 3 when the box does not hold, 4 when the run limit is spent.
 */

import { spawn } from "node:child_process";
import { appendFile, copyFile, mkdir, readFile, writeFile } from "node:fs/promises";
import { existsSync, readdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { StringDecoder } from "node:string_decoder";
import { setTimeout as sleep } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import { AGENT_CONTEXT } from "../../../ui/tests/browser-scripts.mjs";
import { CAP_SECONDS, MAX_TURNS, argsProblems, boxEnv, checkPath, claudeArgs, installStandIn } from "./agent-box.mjs";
import { startFormDesk } from "./door-desk.mjs";
import { extraKeys, readScene, sameWhole, wrongKeys } from "./scene-kit.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(HERE, "../../..");
/* The grace a stopped agent gets before it is killed outright. */
const KILL_AFTER_MS = 5_000;
/* How long the agent's group gets to go once its leader has: the rest of it
 * goes a moment after the leader does, so a process still there after this
 * has outlived its agent. */
const GROUP_GRACE_MS = 2_000;
const GROUP_POLL_MS = 100;
/* How long its last words get to arrive once the agent is gone: its stdout can
 * still hold the result line when its process has ended. */
const STREAM_GRACE_MS = 2_000;
/* How much of the agent's last message a run keeps. */
const FINAL_WORDS_CAP = 400;

/* The one line a refused login is answered with. */
const CLOSED = "REFUSED: --account — the real-model road is closed until a person decides how it logs in (a key made for measuring, or an account used for nothing else); nothing was started";

function args(argv) {
  // Named in any form — `--account DIR`, `--account=DIR`, `--account` alone — it is refused before
  // the arguments are read at all.
  if (argv.some((word) => word === "--account" || word.startsWith("--account="))) { console.log(CLOSED); process.exit(2); }
  const parsed = {};
  for (let at = 0; at < argv.length; at += 1) {
    const key = argv[at].replace(/^--/, "").replace(/-(\w)/g, (_, letter) => letter.toUpperCase());
    if (key === "dry") parsed.dry = true;
    else { parsed[key] = argv[at + 1]; at += 1; }
  }
  for (const need of ["scene", "model", "out", "doorText", "claude"]) {
    if (!parsed[need]) { console.error(`REFUSED: --${need.replace(/[A-Z]/g, (letter) => `-${letter.toLowerCase()}`)} is required`); process.exit(2); }
  }
  parsed.maxTurns = Number(parsed.maxTurns || MAX_TURNS);
  parsed.capSeconds = Number(parsed.capSeconds || CAP_SECONDS);
  parsed.runsMax = parsed.runsMax === undefined ? null : Number(parsed.runsMax);
  return parsed;
}

/* What the person says: the task, where the page is, and the card's facts
 * each under the words the page shows beside its field. */
const personSays = (card, url) => [card.task, `사이트: ${url}`, "내가 알려 주는 정보:",
  ...card.facts.map((fact) => `- ${fact.says}: ${typeof fact.value === "boolean" ? (fact.value ? "예" : "아니오") : fact.value}`)].join("\n");

/* A line of the agent's stream, if it is one. */
const parsedLine = (line) => { try { return JSON.parse(line); } catch { return null; } };

const options = args(process.argv.slice(2));
const out = resolve(options.out);
const doorText = resolve(options.doorText);
const scene = await readScene(options.scene);
// The runs already made: a limit on all of them, and no scene and model twice.
const spentRows = options.spent && existsSync(options.spent)
  ? (await readFile(options.spent, "utf8")).split("\n").filter(Boolean).map((line) => JSON.parse(line)) : [];
if (options.runsMax !== null && spentRows.length >= options.runsMax) {
  console.log(`REFUSED: the run limit (${options.runsMax}) is spent`); process.exit(4);
}
if (spentRows.some((one) => one.scene === scene.name && one.model === options.model)) {
  console.log(`REFUSED: ${scene.name} on ${options.model} already ran`); process.exit(4);
}

// A run's folder is its own: a used one is refused, never cleared.
if (existsSync(out) && readdirSync(out).length) { console.log("REFUSED: --out is not empty"); process.exit(2); }
// The box: the run's own folders, the stand-in's names and manuals, the skill.
const dirs = { bin: join(out, "bin"), work: join(out, "work"), home: join(out, "home"), tmp: join(out, "tmp"), config: join(out, "config") };
for (const folder of Object.values(dirs)) await mkdir(folder, { recursive: true });
await mkdir(join(dirs.config, "skills", "computer-use"), { recursive: true });
await copyFile(join(ROOT, "skills", "computer-use", "SKILL.md"), join(dirs.config, "skills", "computer-use", "SKILL.md"));
await installStandIn({ bin: dirs.bin, doorText });
const argv = claudeArgs({ model: options.model, maxTurns: options.maxTurns });
const env = (desk) => boxEnv({ ...dirs, bin: dirs.bin, desk, user: process.env.USER || "" });
const path = env("").PATH;
const check = checkPath(path, dirs.bin);
const problems = argsProblems(argv);
for (const line of check.lines) console.log(line);
console.log(`ARGS claude ${argv.map((word) => (/[\s{}"]/.test(word) ? JSON.stringify(word) : word)).join(" ")}`);
console.log(`ENV ${Object.keys(env("")).join(" ")}`);
if (problems.length) console.log(`REFUSED: the arguments ${problems.join("; ")}`);
if (!check.holds || problems.length) process.exit(3);
if (options.dry) { console.log("DRY: nothing was started"); process.exit(0); }

// The scene and its stand-in window, then the agent.
const desk = await startFormDesk({ scene, doorText });
const prompt = `${personSays(scene.card, desk.url)}\n\n${AGENT_CONTEXT}`;
await writeFile(join(out, "prompt.txt"), prompt);
const child = spawn(resolve(options.claude), argv, { cwd: dirs.work, env: env(`http://127.0.0.1:${desk.port}`), stdio: ["pipe", "pipe", "pipe"], detached: true });
const group = (signal) => { try { process.kill(-child.pid, signal); } catch { /* the group is gone */ } };
const groupAlive = () => { try { process.kill(-child.pid, 0); return true; } catch { return false; } };
const raw = [];
const errors = [];
const seen = { calls: new Set(), toolUses: new Set(), tools: {}, commands: {}, retries: [], init: null, final: null };
let pending = "";
let stopped = null;
/* What one line of the agent's stream says: its init, an assistant turn and the
 * tools it used, the result. */
function take(event) {
  if (event.type === "system" && event.subtype === "init") seen.init = event;
  if (event.type === "system" && event.subtype === "api_retry") seen.retries.push(event.error ?? event.error_status ?? "retry");
  if (event.type === "result") seen.final = event;
  if (event.type !== "assistant") return;
  seen.calls.add(event.message?.id ?? `event-${seen.calls.size}`);
  for (const block of event.message?.content || []) {
    if (block.type !== "tool_use" || seen.toolUses.has(block.id)) continue;
    seen.toolUses.add(block.id);
    seen.tools[block.name] = (seen.tools[block.name] || 0) + 1;
    if (block.name === "Bash") {
      const [command, verb] = String(block.input?.command || "").trim().split(/\s+/);
      const key = `${command} ${verb ?? ""}`.trim();
      seen.commands[key] = (seen.commands[key] || 0) + 1;
    }
  }
  // A turn past the limit stops the run, whatever the CLI itself does.
  if (seen.calls.size > options.maxTurns && !stopped) { stopped = "turns"; group("SIGTERM"); }
}
// Whole characters: a chunk can end inside one.
const decoder = new StringDecoder("utf8");
child.stdout.on("data", (chunk) => {
  raw.push(chunk);
  pending += decoder.write(chunk);
  let cut;
  while ((cut = pending.indexOf("\n")) >= 0) {
    const event = parsedLine(pending.slice(0, cut));
    pending = pending.slice(cut + 1);
    if (event) take(event);
  }
});
child.stderr.on("data", (chunk) => errors.push(chunk));
child.on("error", (error) => { console.log(`REFUSED: the agent did not start: ${error.message}`); process.exit(5); });
const began = Date.now();
// An agent that ends before it has read its prompt is a run that ended, not a fault.
child.stdin.on("error", () => {});
child.stdin.end(prompt);
const timer = setTimeout(() => {
  stopped = stopped || "time";
  group("SIGTERM");
  setTimeout(() => group("SIGKILL"), KILL_AFTER_MS).unref();
}, options.capSeconds * 1000);
const exit = await new Promise((done) => child.on("exit", (code, signal) => done({ code, signal })));
clearTimeout(timer);
const wallMs = Date.now() - began;
// Nothing is left running: anything still in the agent's group after its grace is ended and counted.
let leftover = false;
for (let waited = 0; groupAlive() && waited < GROUP_GRACE_MS; waited += GROUP_POLL_MS) await sleep(GROUP_POLL_MS);
if (groupAlive()) { leftover = true; group("SIGKILL"); }
await Promise.race([new Promise((done) => (child.stdout.readableEnded ? done() : child.stdout.once("end", done))), sleep(STREAM_GRACE_MS)]);
// A last line that came without its newline.
const last = parsedLine(pending + decoder.end());
if (last) take(last);

const result = await desk.result();
const wrong = wrongKeys(result, scene.expected);
const usage = seen.final?.usage || {};
const tokens = {
  input: usage.input_tokens ?? null, output: usage.output_tokens ?? null,
  cacheRead: usage.cache_read_input_tokens ?? null, cacheWrite: usage.cache_creation_input_tokens ?? null,
};
const denials = seen.final?.permission_denials ?? [];
const row = {
  schema: 1, scene: scene.name, model: options.model,
  // The two rules of success side by side (the keys the card expects; the page's whole result as the expected one) and what only the page recorded.
  oracle: { pass: wrong.length === 0, wrong, tookResult: result !== null, whole: result !== null && sameWhole(result, scene.expected), extra: extraKeys(result, scene.expected) },
  wallMs, personMs: desk.tally.personMs, stopped, exit, leftover,
  modelCalls: seen.calls.size, turns: seen.final?.num_turns ?? null, apiMs: seen.final?.duration_api_ms ?? null,
  tokens, costUsd: seen.final?.total_cost_usd ?? null, finalSubtype: seen.final?.subtype ?? null,
  // A run the CLI ended with an error (a refused login, an API error): what it said, and how often it tried again.
  isError: seen.final?.is_error ?? null, apiErrorStatus: seen.final?.api_error_status ?? null,
  finalWords: String(seen.final?.result ?? "").slice(0, FINAL_WORDS_CAP), apiRetries: seen.retries.length, apiRetryErrors: [...new Set(seen.retries)],
  tools: seen.tools, commands: seen.commands, permissionDenials: denials.length,
  door: { requests: desk.tally.requests, verbs: desk.tally.verbs, refusals: desk.tally.refusals, unwired: desk.tally.unwired,
    handoffs: desk.tally.handoffs, faults: desk.tally.faults, blocked: desk.tally.blocked, dialogs: desk.tally.dialogs, popups: desk.tally.popups },
  init: seen.init ? { tools: seen.init.tools, mcpServers: seen.init.mcp_servers, model: seen.init.model, apiKeySource: seen.init.apiKeySource,
    permissionMode: seen.init.permissionMode, slashCommands: (seen.init.slash_commands || []).length, plugins: seen.init.plugins ?? null,
    skills: seen.init.skills ?? null, agents: seen.init.agents ?? null } : null,
};
await writeFile(join(out, "events.ndjson"), Buffer.concat(raw));
await writeFile(join(out, "agent.stderr"), Buffer.concat(errors));
await writeFile(join(out, "door-trail.jsonl"), desk.trail.map((one) => JSON.stringify(one)).join("\n") + "\n");
await writeFile(join(out, "result.json"), `${JSON.stringify(row, null, 2)}\n`);
if (options.spent) await appendFile(options.spent, `${JSON.stringify({ at: new Date().toISOString(), scene: row.scene, model: row.model, modelCalls: row.modelCalls, tokens, costUsd: row.costUsd })}\n`);
await desk.close();
console.log(`INIT ${JSON.stringify(row.init)}`);
console.log(`TOKENS model=${row.model} scene=${row.scene} input=${tokens.input} output=${tokens.output} cacheRead=${tokens.cacheRead} cacheWrite=${tokens.cacheWrite} costUsd=${row.costUsd}`);
console.log(`ORACLE ${row.oracle.pass ? "pass" : "fail"} scene=${row.scene} model=${row.model} wrong=${wrong.join(",") || "-"} took=${result !== null} wallMs=${wallMs} modelCalls=${row.modelCalls} doorCalls=${row.door.requests} stopped=${stopped ?? "-"} leftover=${leftover}`);
if (row.isError) console.log(`AGENT-ERROR status=${row.apiErrorStatus} retries=${row.apiRetries} said=${JSON.stringify(row.finalWords)}`);
console.log(`RESULT ${JSON.stringify(row)}`);
process.exit(0);
