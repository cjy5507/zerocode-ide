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
import { dirname, join } from "node:path";
import { setTimeout as sleep } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import { doorTextFromArgv, writeFixtureScene } from "./fixture-scene.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const REAL = join(HERE, "real.mjs");
const doorText = doorTextFromArgv();

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
/* An agent the API refuses: two retries, then an error result and a failing exit. */
const refused = await standIn("refused", ["cat > /dev/null", INIT,
  event('{"type":"system","subtype":"api_retry","attempt":1,"error_status":401,"error":"authentication_failed"}'),
  event('{"type":"system","subtype":"api_retry","attempt":2,"error_status":401,"error":"authentication_failed"}'),
  event('{"type":"assistant","message":{"id":"e1","content":[{"type":"text","text":"Failed to authenticate"}]}}'),
  event('{"type":"result","subtype":"success","is_error":true,"result":"Failed to authenticate: OAuth token revoked.","api_error_status":401,"num_turns":1,"duration_ms":10,"total_cost_usd":0,"usage":{"input_tokens":0,"output_tokens":0}}'),
  "exit 1"].join("\n"));
/* An agent that never ends, with a child of its own that must not outlive it. */
const spins = await standIn("spins", ["cat > /dev/null", INIT, "sleep 654321 &", "while :; do sleep 1; done"].join("\n"));
/* An agent that takes turn after turn. */
const chatters = await standIn("chatters", ["cat > /dev/null", INIT, ...["m1", "m2", "m3", "m4", "m5", "m6"].map((id) => turn(id, "zerocode-browser list")), "sleep 654322 &", "while :; do sleep 1; done"].join("\n"));

/* An agent whose stream breaks a Korean syllable (three bytes) across two writes. */
const splits = await standIn("splits", ["cat > /dev/null", INIT,
  "printf '%s' '{\"type\":\"assistant\",\"message\":{\"id\":\"m1\",\"content\":[{\"type\":\"tool_use\",\"id\":\"u1\",\"name\":\"Bash\",\"input\":{\"command\":\"echo '",
  "printf '\\355\\225'", "sleep 0.3", "printf '\\234'", "printf '%s\\n' '\"}}]}}'", RESULT].join("\n"));

/* One run of the runner, its out folder, its words. */
function run(claude, extra = [], out = fresh()) {
  const done = spawnSync(process.execPath, [REAL, "--scene", scene, "--model", "fake-model", "--out", out, "--door-text", doorText,
    "--claude", claude, ...extra], { encoding: "utf8", timeout: 120_000 });
  const row = JSON.parse((String(done.stdout).split("\n").find((line) => line.startsWith("RESULT ")) || "RESULT null").slice(7));
  return { code: done.status, stdout: String(done.stdout), stderr: String(done.stderr), out, row };
}
/* Whether a process whose command holds `marker` is still there, after the
 * short while a process that was told to go takes to do it. */
async function lingers(marker) {
  for (let waited = 0; waited < 2_000; waited += 100) {
    if (spawnSync("pgrep", ["-f", marker]).status !== 0) return false;
    await sleep(100);
  }
  return true;
}

await test("a run hands the agent the product's launch prompt, an environment from nothing and the skill, and measures what it did", async () => {
  const done = run(finishes);
  assert(done.code === 0 && done.stdout.includes("ORACLE pass"), "the run ended and the oracle passed", done.stdout.slice(-600));
  const prompt = await readFile(join(done.out, "prompt.txt"), "utf8");
  assert(prompt.startsWith("Book a size Large for me.") && prompt.includes("- Name: Kim") && prompt.includes("ZeroCode surfaces:")
    && prompt.includes("a web form is one `fields` and one `fill` a step") && prompt.includes("사이트: http://127.0.0.1:"), "the person's words, then the product's paragraphs", prompt.slice(0, 200));
  assert(await readFile(join(done.out, "work", "prompt-seen.txt"), "utf8") === prompt, "and the agent was handed exactly that on stdin");
  const env = (await readFile(join(done.out, "work", "env-seen.txt"), "utf8")).split("\n").filter(Boolean);
  const names = env.map((line) => line.split("=")[0]);
  assert(names.every((name) => ["PATH", "HOME", "TMPDIR", "USER", "LOGNAME", "LANG", "SHELL", "CLAUDE_CONFIG_DIR", "FORM_DESK",
    "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "DISABLE_AUTOUPDATER", "DISABLE_TELEMETRY", "DISABLE_ERROR_REPORTING",
    // What the shell and the system add by themselves.
    "PWD", "SHLVL", "_", "OLDPWD", "__CF_USER_TEXT_ENCODING", "COMMAND_MODE"].includes(name)), "nothing is inherited", names);
  assert(env.includes(`PATH=${join(done.out, "bin")}:/usr/bin:/bin:/usr/sbin:/sbin`), "the stand-in's folder and the system's own on PATH", env.find((line) => line.startsWith("PATH=")));
  assert(env.includes(`CLAUDE_CONFIG_DIR=${join(done.out, "config")}`) && !names.some((name) => /SECURESTORAGE|API_KEY|TOKEN/.test(name)), "a config folder of its own, and no login named");
  assert(existsSync(join(done.out, "config", "skills", "computer-use", "SKILL.md")), "the computer-use skill is installed in that folder");
  const row = done.row;
  assert(row.modelCalls === 3 && row.turns === 3 && row.tokens.input === 100 && row.tokens.output === 20 && row.tokens.cacheRead === 5 && row.tokens.cacheWrite === 7 && row.costUsd === 0.01,
    "model calls, tokens and cost are read from the stream", row);
  assert(row.commands["zerocode-browser fields"] === 1 && row.commands["zerocode-browser fill"] === 1 && row.tools.Bash === 3, "the commands the agent ran are counted", [row.commands, row.tools]);
  assert(row.door.requests >= 3 && row.door.verbs["browser/fill"] === 1 && row.stopped === null && row.leftover === false && row.oracle.pass === true, "the door's trail, the run's end and the oracle", row.door);
  assert(row.init.tools.join() === "Bash,Skill" && row.init.mcpServers.length === 0 && row.init.slashCommands === 2, "what the agent was given is read from its init line", row.init);
  return `${row.modelCalls} calls, ${row.door.requests} door requests, ${row.wallMs} ms`;
});

await test("a stream that breaks a character across two writes is read whole, and its last line is taken", async () => {
  const done = run(splits);
  assert(done.code === 0 && done.row.commands["echo 한"] === 1, "the syllable came through whole", done.row.commands);
  assert(done.row.tokens.input === 100 && done.row.finalSubtype === "success", "and the result line that follows is read", done.row.tokens);
  return JSON.stringify(done.row.commands);
});

await test("a run the API refuses says why: the error, its status, the tries — and no tokens", async () => {
  const done = run(refused);
  const row = done.row;
  assert(done.code === 0 && row.isError === true && row.apiErrorStatus === 401 && row.apiRetries === 2, "the refusal is read from the stream", [done.code, row.isError, row.apiErrorStatus, row.apiRetries]);
  assert(row.finalWords.includes("OAuth token revoked") && row.apiRetryErrors.join() === "authentication_failed", "with its own words", [row.finalWords, row.apiRetryErrors]);
  assert(row.tokens.input === 0 && row.exit.code === 1 && done.stdout.includes("AGENT-ERROR status=401 retries=2"), "and the log says it plainly", done.stdout.slice(-400));
  return row.finalWords;
});

await test("a run that never ends is stopped at its time cap and leaves nothing running", async () => {
  const done = run(spins, ["--cap-seconds", "3"]);
  assert(done.code === 0 && done.row.stopped === "time", "stopped by the time cap", [done.code, done.row.stopped]);
  assert(done.row.wallMs >= 2_500 && done.row.wallMs < 15_000, "at about the cap", done.row.wallMs);
  assert(done.row.leftover === false && !(await lingers("sleep 654321")), "no process of the agent's is left", done.row.leftover);
  assert(done.row.oracle.pass === false && done.row.oracle.tookResult === false, "and the oracle says nothing was taken");
  return `${done.row.wallMs} ms`;
});

await test("a run past its turn limit is stopped by the runner, whatever the agent does", async () => {
  const done = run(chatters, ["--max-turns", "3", "--cap-seconds", "30"]);
  assert(done.row.stopped === "turns" && done.row.modelCalls > 3, "stopped by the turn limit", [done.row.stopped, done.row.modelCalls]);
  assert(done.row.leftover === false && !(await lingers("sleep 654322")), "and nothing is left running", { leftover: done.row.leftover, stopped: done.row.stopped });
  return `${done.row.modelCalls} calls seen`;
});

await test("the runner keeps its own limits: no scene on a model twice, a limit on all runs, a used folder is refused", async () => {
  const spent = join(root, "spent.jsonl");
  const first = run(finishes, ["--spent", spent, "--runs-max", "2"]);
  assert(first.code === 0 && (await readFile(spent, "utf8")).trim().split("\n").length === 1, "the first run is logged", first.stdout.slice(-300));
  const again = run(finishes, ["--spent", spent, "--runs-max", "2"]);
  assert(again.code === 4 && again.stdout.includes("already ran"), "the same scene on the same model is refused", [again.code, again.stdout]);
  const other = spawnSync(process.execPath, [REAL, "--scene", scene, "--model", "other-model", "--out", fresh(), "--door-text", doorText, "--claude", finishes,
    "--spent", spent, "--runs-max", "1"], { encoding: "utf8", timeout: 120_000 });
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

/* The real-model road is closed until a person decides how it logs in (m-41479):
 * a run that points a live account folder at the CLI as its login is refused
 * before anything starts — not the box, not the stand-in window, not the CLI. */
await test("a run that names an account folder as its login is refused before the agent starts: the real-model road is closed", async () => {
  const marker = join(root, "started.txt");
  const watcher = await standIn("watcher", `echo started > "${marker}"\ncat > /dev/null`);
  const out = fresh();
  const done = spawnSync(process.execPath, [REAL, "--scene", scene, "--model", "fake-model", "--out", out, "--door-text", doorText,
    "--claude", watcher, "--account", account], { encoding: "utf8", timeout: 120_000 });
  assert(done.status === 2, "refused as a wrong argument is", [done.status, String(done.stdout).slice(-300), String(done.stderr).slice(-300)]);
  const said = `${done.stdout}${done.stderr}`.split("\n").filter((line) => line.startsWith("REFUSED"));
  assert(said.length === 1 && said[0].includes("closed"), "in one line that says the road is closed", said);
  assert(!existsSync(marker), "the stand-in for the CLI was never started");
  assert(!existsSync(out), "nor was the run's folder made");
  return said[0];
});

await test("an account named in any form is refused before the agent starts, with a value or without", async () => {
  const marker = join(root, "started-again.txt");
  const watcher = await standIn("watcher-again", `echo started > "${marker}"\ncat > /dev/null`);
  for (const naming of [["--account"], ["--account", ""], [`--account=${account}`], ["--account", account]]) {
    const out = fresh();
    const done = spawnSync(process.execPath, [REAL, "--scene", scene, "--model", "fake-model", "--out", out, "--door-text", doorText,
      "--claude", watcher, ...naming], { encoding: "utf8", timeout: 120_000 });
    const said = `${done.stdout}${done.stderr}`.split("\n").filter((line) => line.startsWith("REFUSED"));
    assert(done.status === 2 && said.length === 1 && said[0].includes("closed"), "refused in one line that says the road is closed", [naming, done.status, said]);
    assert(!existsSync(marker) && !existsSync(out), "and nothing was started or made", naming);
  }
  return "ok";
});

let failed = 0;
for (const result of results) {
  if (!result.pass) failed += 1;
  console.log(`${result.pass ? "PASS" : "FAIL"}  ${result.name}${result.detail ? `  — ${result.detail}` : ""}`);
}
console.log(`\n${results.length - failed}/${results.length} passed`);
process.exit(failed ? 1 : 0);
