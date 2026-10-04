/* The runner of a real agent (real.mjs) held to its own rules (t-41387),
 * without a model and without a login: a stand-in for `claude -p` — a shell
 * script that reads its prompt on stdin, says what the CLI says (the init line,
 * assistant turns with tool uses, the result line with its tokens) and uses the
 * door as an agent does — stands where the CLI stands. What is checked is the
 * runner: the prompt the agent is handed, the environment it gets, what the
 * run measures, the oracle, the limits (a time cap, a turn cap, nothing left
 * running) and the run limit it keeps across runs.
 *
 *   node test-real.mjs --door-text PATH
 */

import { spawnSync } from "node:child_process";
import { mkdtemp, mkdir, readFile, writeFile, chmod } from "node:fs/promises";
import { existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { writeFixtureScene } from "./fixture-scene.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const REAL = join(HERE, "real.mjs");
const doorText = process.argv[process.argv.indexOf("--door-text") + 1];
if (!doorText || doorText.startsWith("--")) { console.error("--door-text PATH is required"); process.exit(2); }

const results = [];
async function test(name, run) {
  try { results.push({ name, pass: true, detail: (await run()) ?? "" }); }
  catch (error) { results.push({ name, pass: false, detail: error?.stack ?? String(error) }); }
}
function assert(condition, message, detail = undefined) {
  if (!condition) throw new Error(detail === undefined ? message : `${message}: ${JSON.stringify(detail)}`);
}

const root = await mkdtemp(join(tmpdir(), "form-real-"));
const scene = join(root, "scene");
await writeFixtureScene(scene, { name: "Kim", size: "l", agree: true });
const account = join(root, "account");
await mkdir(account);
let runs = 0;
const fresh = () => join(root, `run-${(runs += 1)}`);

/* A stand-in for the CLI: a shell script, because the box's PATH holds no node. */
async function standIn(name, body) {
  const path = join(root, `${name}.sh`);
  await writeFile(path, `#!/bin/sh\n${body}\n`);
  await chmod(path, 0o755);
  return path;
}
const event = (line) => `printf '%s\\n' '${line}'`;
const INIT = event('{"type":"system","subtype":"init","tools":["Bash","Skill"],"mcp_servers":[],"model":"fake","apiKeySource":"none","permissionMode":"default","slash_commands":["a","b"],"plugins":[],"skills":["computer-use"]}');
const turn = (id, command) => event(`{"type":"assistant","message":{"id":"${id}","content":[{"type":"tool_use","id":"use-${id}","name":"Bash","input":{"command":"${command}"}}]}}`);
const RESULT = event('{"type":"result","subtype":"success","num_turns":3,"duration_ms":1000,"duration_api_ms":800,"total_cost_usd":0.01,"usage":{"input_tokens":100,"output_tokens":20,"cache_read_input_tokens":5,"cache_creation_input_tokens":7},"permission_denials":[]}');

const finishes = await standIn("finishes", [
  "cat > \"$PWD/prompt-seen.txt\"", "env | sort > \"$PWD/env-seen.txt\"", INIT,
  "zerocode-browser fields browser-1 > /dev/null", turn("m1", "zerocode-browser fields browser-1"),
  "printf '%s' '{\"#name\":\"Kim\",\"#size\":\"Large\",\"#agree\":true}' | zerocode-browser fill browser-1 --value-stdin > /dev/null",
  turn("m2", "zerocode-browser fill browser-1 --value-stdin"),
  "zerocode-browser click browser-1 '#book' > /dev/null", turn("m3", "zerocode-browser click browser-1 #book"), RESULT,
].join("\n"));
/* An agent that never ends, with a child of its own that must not outlive it. */
const spins = await standIn("spins", ["cat > /dev/null", INIT, "sleep 654321 &", "while :; do sleep 1; done"].join("\n"));
/* An agent that takes turn after turn. */
const chatters = await standIn("chatters", ["cat > /dev/null", INIT, ...["m1", "m2", "m3", "m4", "m5", "m6"].map((id) => turn(id, "zerocode-browser list")), "sleep 654322 &", "while :; do sleep 1; done"].join("\n"));

/* One run of the runner, its out folder, its words. */
function run(claude, extra = [], out = fresh()) {
  const done = spawnSync(process.execPath, [REAL, "--scene", scene, "--model", "fake-model", "--out", out, "--door-text", doorText,
    "--claude", claude, "--account", account, ...extra], { encoding: "utf8", timeout: 120_000 });
  const row = JSON.parse((String(done.stdout).split("\n").find((line) => line.startsWith("RESULT ")) || "RESULT null").slice(7));
  return { code: done.status, stdout: String(done.stdout), stderr: String(done.stderr), out, row };
}
const alive = (marker) => spawnSync("pgrep", ["-f", marker]).status === 0;

await test("a run hands the agent the product's launch prompt, an environment from nothing and the skill, and measures what it did", async () => {
  const done = run(finishes);
  assert(done.code === 0 && done.stdout.includes("ORACLE pass"), "the run ended and the oracle passed", done.stdout.slice(-600));
  const prompt = await readFile(join(done.out, "prompt.txt"), "utf8");
  assert(prompt.startsWith("Book a size Large for me.") && prompt.includes("- Name: Kim") && prompt.includes("ZeroCode surfaces:")
    && prompt.includes("a web form is one `fields` and one `fill` a step") && prompt.includes("사이트: http://127.0.0.1:"), "the person's words, then the product's paragraphs", prompt.slice(0, 200));
  assert(await readFile(join(done.out, "work", "prompt-seen.txt"), "utf8") === prompt, "and the agent was handed exactly that on stdin");
  const env = (await readFile(join(done.out, "work", "env-seen.txt"), "utf8")).split("\n").filter(Boolean);
  const names = env.map((line) => line.split("=")[0]);
  assert(names.every((name) => ["PATH", "HOME", "TMPDIR", "USER", "LOGNAME", "LANG", "SHELL", "CLAUDE_CONFIG_DIR", "CLAUDE_SECURESTORAGE_CONFIG_DIR", "FORM_DESK",
    "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "DISABLE_AUTOUPDATER", "DISABLE_TELEMETRY", "DISABLE_ERROR_REPORTING",
    // What the shell and the system add by themselves.
    "PWD", "SHLVL", "_", "OLDPWD", "__CF_USER_TEXT_ENCODING", "COMMAND_MODE"].includes(name)), "nothing is inherited", names);
  assert(env.includes(`PATH=${join(done.out, "bin")}:/usr/bin:/bin:/usr/sbin:/sbin`), "the stand-in's folder and the system's own on PATH", env.find((line) => line.startsWith("PATH=")));
  assert(env.includes(`CLAUDE_CONFIG_DIR=${join(done.out, "config")}`) && env.includes(`CLAUDE_SECURESTORAGE_CONFIG_DIR=${resolve(account)}`), "a config folder of its own; the login by path");
  assert(existsSync(join(done.out, "config", "skills", "computer-use", "SKILL.md")), "the computer-use skill is installed in that folder");
  const row = done.row;
  assert(row.modelCalls === 3 && row.turns === 3 && row.tokens.input === 100 && row.tokens.output === 20 && row.tokens.cacheRead === 5 && row.tokens.cacheWrite === 7 && row.costUsd === 0.01,
    "model calls, tokens and cost are read from the stream", row);
  assert(row.commands["zerocode-browser fields"] === 1 && row.commands["zerocode-browser fill"] === 1 && row.tools.Bash === 3, "the commands the agent ran are counted", [row.commands, row.tools]);
  assert(row.door.requests >= 3 && row.door.verbs["browser/fill"] === 1 && row.stopped === null && row.leftover === false && row.oracle.pass === true, "the door's trail, the run's end and the oracle", row.door);
  assert(row.init.tools.join() === "Bash,Skill" && row.init.mcpServers.length === 0 && row.init.slashCommands === 2, "what the agent was given is read from its init line", row.init);
  return `${row.modelCalls} calls, ${row.door.requests} door requests, ${row.wallMs} ms`;
});

await test("a run that never ends is stopped at its time cap and leaves nothing running", async () => {
  const done = run(spins, ["--cap-seconds", "3"]);
  assert(done.code === 0 && done.row.stopped === "time", "stopped by the time cap", [done.code, done.row.stopped]);
  assert(done.row.wallMs >= 2_500 && done.row.wallMs < 15_000, "at about the cap", done.row.wallMs);
  assert(done.row.leftover === false && !alive("sleep 654321"), "no process of the agent's is left", done.row.leftover);
  assert(done.row.oracle.pass === false && done.row.oracle.tookResult === false, "and the oracle says nothing was taken");
  return `${done.row.wallMs} ms`;
});

await test("a run past its turn limit is stopped by the runner, whatever the agent does", async () => {
  const done = run(chatters, ["--max-turns", "3", "--cap-seconds", "30"]);
  assert(done.row.stopped === "turns" && done.row.modelCalls > 3, "stopped by the turn limit", [done.row.stopped, done.row.modelCalls]);
  assert(done.row.leftover === false && !alive("sleep 654322"), "and nothing is left running");
  return `${done.row.modelCalls} calls seen`;
});

await test("the runner keeps its own limits: no scene on a model twice, a limit on all runs, a used folder is refused", async () => {
  const spent = join(root, "spent.jsonl");
  const first = run(finishes, ["--spent", spent, "--runs-max", "2"]);
  assert(first.code === 0 && (await readFile(spent, "utf8")).trim().split("\n").length === 1, "the first run is logged", first.stdout.slice(-300));
  const again = run(finishes, ["--spent", spent, "--runs-max", "2"]);
  assert(again.code === 4 && again.stdout.includes("already ran"), "the same scene on the same model is refused", [again.code, again.stdout]);
  const other = spawnSync(process.execPath, [REAL, "--scene", scene, "--model", "other-model", "--out", fresh(), "--door-text", doorText, "--claude", finishes,
    "--account", account, "--spent", spent, "--runs-max", "1"], { encoding: "utf8", timeout: 120_000 });
  assert(other.status === 4 && String(other.stdout).includes("run limit (1) is spent"), "and so is a run past the limit", [other.status, other.stdout]);
  const used = run(finishes, [], first.out);
  assert(used.code === 2 && used.stdout.includes("--out is not empty"), "a folder already used is refused, not cleared", [used.code, used.stdout]);
  return "ok";
});

await test("--dry says what would be launched and starts nothing", async () => {
  const done = run(finishes, ["--dry"]);
  assert(done.code === 0 && done.stdout.includes("DRY: nothing was started") && done.stdout.includes("ARGS claude -p --model fake-model"), "the arguments are said", done.stdout.slice(0, 300));
  assert(!existsSync(join(done.out, "prompt.txt")) && !existsSync(join(done.out, "work", "prompt-seen.txt")), "no agent ran");
  return "ok";
});

let failed = 0;
for (const result of results) {
  if (!result.pass) failed += 1;
  console.log(`${result.pass ? "PASS" : "FAIL"}  ${result.name}${result.detail ? `  — ${result.detail}` : ""}`);
}
console.log(`\n${results.length - failed}/${results.length} passed`);
process.exit(failed ? 1 : 0);
