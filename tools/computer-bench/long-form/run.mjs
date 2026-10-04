/* One run of the long-form bench (t-37883): the page served, the fake desk
 * standing in for the window, one agent CLI handed the card, and what the
 * run cost — wall time, model round trips and their first-byte and stream
 * times, tokens per request, the hand's calls and waits, the pictures'
 * bytes — beside the oracle's score and the person's time for the same form.
 *
 *   node run.mjs --agent zo --scenario chrome|url --out DIR [--cap-seconds N]
 *                [--model ID] [--bin PATH] [--note WORDS]
 *
 * `chrome`: the form stands open in the desktop browser, as it did in the
 * person's session; `url`: the agent is handed the address and chooses its
 * own road. The agent runs with an allowlisted environment: the bench's fake
 * shims first on PATH, no road to the person's window, its own state folders
 * under DIR (so nothing is written into the person's ~/.zo), and the one
 * login named by CLAUDE_SECURESTORAGE_CONFIG_DIR.
 *
 * For the low-spec profile, run the whole command under `taskpolicy -b`:
 * the desk's Chromium, the server and the agent inherit it.
 */

import { spawn, spawnSync } from "node:child_process";
import { mkdir, readFile, writeFile, readdir, symlink } from "node:fs/promises";
import { existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { startDesk } from "./desk.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const SPEC = JSON.parse(await readFile(join(HERE, "spec.json"), "utf8"));
// The person's own time for the upload, which the fake person spends on a
// hand-off: the yardstick's row for the file field (klm.py).
const KLM = JSON.parse(spawnSync("python3", [join(HERE, "klm.py"), "--json"], { encoding: "utf8" }).stdout);
const PERSON_ATTACH_MS = Math.round(KLM.rows.find((row) => row.id === "passport_scan").seconds * 1000);
const DEFAULT_CAP_SECONDS = 1500;
// What each scenario tells the agent about where the form stands.
const WHERE = {
  chrome: "The form is already open in Google Chrome on this Mac.",
  url: "",
};
// The environment an agent keeps from the job's: who and where it is, and
// the one login it runs on — never a road to the person's window.
const KEPT_ENV = ["HOME", "USER", "LOGNAME", "LANG", "LC_ALL", "TMPDIR", "CLAUDE_SECURESTORAGE_CONFIG_DIR"];
const SYSTEM_PATH = ["/usr/bin", "/bin", "/usr/sbin", "/sbin"];

function args(argv) {
  const parsed = { agent: "zo", scenario: "chrome", capSeconds: DEFAULT_CAP_SECONDS, note: "" };
  for (let at = 0; at < argv.length; at += 2) {
    const [flag, value] = [argv[at], argv[at + 1]];
    const key = flag.replace(/^--/, "").replace(/-(\w)/g, (_, letter) => letter.toUpperCase());
    parsed[key] = key === "capSeconds" ? Number(value) : value;
  }
  if (!parsed.out) throw new Error("--out DIR is required");
  if (!(parsed.scenario in WHERE)) throw new Error(`--scenario is one of ${Object.keys(WHERE).join(", ")}`);
  return parsed;
}

function card(url, scenario) {
  const lines = SPEC.card.text.map((line) => line.replace("{url}", url));
  return [...lines, WHERE[scenario]].filter(Boolean).join(" ");
}

/* Each agent CLI behind one interface: how it is started, where it keeps
 * its per-request timings, and how its tool calls are counted. */
const AGENTS = {
  zo: {
    bin: (options) => options.bin || "zo",
    argv: (options, work) => ["--json", "--no-spawn", "--permission-mode", "danger-full-access", "--cwd", work,
      "--last-message", join(options.out, "last-message.txt"), ...(options.model ? ["--model", options.model] : [])],
    env: (options) => ({
      ZO_CONFIG_HOME: join(options.out, "zo-home"), ZO_STATE_DIR: join(options.out, "zo-state"),
      ZO_SESSION_ROOT: join(options.out, "zo-sessions"), ZO_DISABLE_KEYCHAIN: "1", ZEROCODE_SECOND_BRAIN: "off",
      ZO_DREAM: "0", ZO_AUTO_VERIFY: "0", ZO_PROFILE_DISABLE_HOOK_REPORTER: "1",
      ZO_TURN_DEADLINE_SECS: String(options.capSeconds),
    }),
    // One line on stdin, then closed: zo's unattended run.
    stdin: (prompt) => `${prompt.replace(/\n/g, " ")}\n`,
    timings: async (options) => jsonl(await found(join(options.out, "zo-state"), "timings.jsonl")),
    tokens: async (options) => jsonl(await found(join(options.out, "zo-home"), "requests.jsonl")),
    toolCalls: (lines) => lines.filter((line) => line.type === "tool_call").map((line) => line.name),
  },
};

async function found(root, name) {
  const hits = [];
  const walk = async (folder) => {
    if (!existsSync(folder)) return;
    for (const entry of await readdir(folder, { withFileTypes: true })) {
      const path = join(folder, entry.name);
      if (entry.isDirectory()) await walk(path);
      else if (entry.name === name) hits.push(path);
    }
  };
  await walk(root);
  return hits;
}

async function jsonl(paths) {
  const rows = [];
  for (const path of paths) {
    for (const line of (await readFile(path, "utf8")).split("\n")) {
      if (line.trim()) rows.push(JSON.parse(line));
    }
  }
  return rows;
}

function quantile(values, share) {
  if (!values.length) return null;
  const ordered = [...values].sort((a, b) => a - b);
  return ordered[Math.min(ordered.length - 1, Math.floor(share * ordered.length))];
}

function spread(values) {
  return { n: values.length, p50: quantile(values, 0.5), p90: quantile(values, 0.9),
    max: values.length ? Math.max(...values) : null, sum: values.reduce((total, value) => total + value, 0) };
}

async function serve(out) {
  const server = spawn("python3", [join(HERE, "server.py"), "--out", out], { stdio: ["ignore", "pipe", "inherit"] });
  const url = await new Promise((done, failed) => {
    server.stdout.on("data", (chunk) => {
      const said = String(chunk).match(/listening (\S+)/);
      if (said) done(said[1]);
    });
    server.on("exit", (code) => failed(new Error(`the form server left (${code})`)));
  });
  return { server, url };
}

async function shims(out) {
  const bin = join(out, "bin");
  await mkdir(bin, { recursive: true });
  for (const name of ["zerocode-computer", "zerocode-browser"]) {
    await symlink(join(HERE, "bin", "zerocode-door"), join(bin, name));
  }
  return bin;
}

async function main() {
  const options = args(process.argv.slice(2));
  options.out = resolve(options.out);
  const agent = AGENTS[options.agent];
  if (!agent) throw new Error(`--agent is one of ${Object.keys(AGENTS).join(", ")} (others are not wired yet)`);
  const work = join(options.out, "work");
  await mkdir(work, { recursive: true });
  const { server, url } = await serve(options.out);
  const desk = await startDesk({ url, out: options.out, personAttachMs: PERSON_ATTACH_MS });
  const bin = await shims(options.out);
  const env = Object.fromEntries(KEPT_ENV.filter((name) => process.env[name]).map((name) => [name, process.env[name]]));
  Object.assign(env, agent.env(options), {
    PATH: [bin, dirname(process.execPath), ...SYSTEM_PATH].join(":"),
    LONG_FORM_DESK: `http://127.0.0.1:${desk.port}`,
    SHELL: "/bin/zsh",
  });
  const prompt = card(url, options.scenario);
  const started = Date.now();
  const child = spawn(agent.bin(options), agent.argv(options, work), { cwd: work, env, stdio: ["pipe", "pipe", "pipe"] });
  const lines = [];
  const requestSentAt = [];
  let pending = "";
  const raw = [];
  child.stdout.on("data", (chunk) => {
    raw.push(chunk);
    pending += chunk;
    let cut;
    while ((cut = pending.indexOf("\n")) >= 0) {
      const line = pending.slice(0, cut);
      pending = pending.slice(cut + 1);
      try {
        const parsed = JSON.parse(line);
        lines.push(parsed);
        if (parsed.type === "stream_phase" && parsed.phase === "request_sent") requestSentAt.push(Date.now() - started);
      } catch { /* a line that is not JSON is the agent's own words */ }
    }
  });
  const errors = [];
  child.stderr.on("data", (chunk) => errors.push(chunk));
  child.stdin.end(agent.stdin(prompt));
  const capped = setTimeout(() => child.kill("SIGTERM"), options.capSeconds * 1000);
  const exit = await new Promise((done) => child.on("exit", (code, signal) => done({ code, signal })));
  clearTimeout(capped);
  const wallMs = Date.now() - started;
  // What the page held when the agent stopped — the progress of a run cut
  // off before it submitted (the page's own state, as it would post it).
  const held = await desk.page.evaluate(() => (typeof state === "object" ? state : null)).catch(() => null);
  await writeFile(join(options.out, "progress.json"), JSON.stringify(held));
  await desk.close();
  server.kill();
  await writeFile(join(options.out, "agent.ndjson"), Buffer.concat(raw));
  await writeFile(join(options.out, "agent.stderr"), Buffer.concat(errors));
  const oracle = JSON.parse(spawnSync("python3", [join(HERE, "oracle.py"), "--out", options.out, "--json",
    "--progress", join(options.out, "progress.json")], { encoding: "utf8" }).stdout);
  const timings = await agent.timings(options);
  const tokens = await agent.tokens(options);
  const calls = agent.toolCalls(lines);
  const result = {
    schema: 1, agent: options.agent, model: options.model || null, scenario: options.scenario, note: options.note,
    profile: process.env.LONG_FORM_PROFILE || "normal",
    wallSeconds: wallMs / 1000, exit, capSeconds: options.capSeconds,
    personSeconds: KLM.total_seconds, overPerson: wallMs / 1000 / KLM.total_seconds,
    oracle: { pass: oracle.pass, ok: oracle.ok_fields, of: oracle.total_fields, wrong: oracle.wrong, missing: oracle.missing,
      submits: oracle.submits, handoffs: oracle.handoffs, reasons: oracle.reasons, progress: oracle.progress || null },
    requests: {
      roundTrips: timings.length, requestsSent: requestSentAt.length,
      firstByteMs: spread(timings.map((row) => row.ttfb_ms).filter(Number.isFinite)),
      streamMs: spread(timings.map((row) => row.stream_ms).filter(Number.isFinite)),
      assembleMs: spread(timings.map((row) => row.assemble_ms).filter(Number.isFinite)),
      inputTokens: spread(tokens.map((row) => (row.input_uncached || 0) + (row.cache_read || 0) + (row.cache_creation || 0))),
      outputTokens: spread(tokens.map((row) => row.output || 0)),
    },
    tools: Object.entries(calls.reduce((tally, name) => ({ ...tally, [name]: (tally[name] || 0) + 1 }), {})),
    hand: { verbs: desk.tally.verbs, batchSteps: desk.tally.steps, waitMs: desk.tally.waitMs, handMs: desk.tally.handMs,
      settleMs: desk.tally.settleMs, looks: desk.tally.looks, pictureBytes: spread(desk.tally.pngBytes), handoffs: desk.tally.handoffs },
  };
  result.requests.modelShare = result.requests.streamMs.sum / wallMs;
  await writeFile(join(options.out, "result.json"), `${JSON.stringify(result, null, 2)}\n`);
  console.log(`RESULT ${JSON.stringify(result)}`);
}

await main();
