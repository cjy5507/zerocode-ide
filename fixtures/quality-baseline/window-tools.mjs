/* Deterministic window boundary: production CLI transports, isolated loopback
 * endpoint, real DOM/PNG in Chromium, and a durable test ledger. No live window
 * credentials or provider endpoints enter these child environments. */
import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { createServer } from "node:http";
import { readFile, writeFile } from "node:fs/promises";
import { resolve, join } from "node:path";
import { fileURLToPath } from "node:url";
const exec = promisify(execFile);
export const run = (file, args, options = {}) => exec(file, args, { ...options, maxBuffer: 8 * 1024 * 1024 });
export function validResult(value) {
  return value && JSON.stringify(Object.keys(value).sort()) === JSON.stringify(["files", "ok", "summary"])
    && value.ok === true && typeof value.summary === "string" && value.summary.length > 0
    && Array.isArray(value.files) && value.files.every(p => typeof p === "string")
    && new Set(value.files).size === value.files.length;
}
export async function endpoint(handler) {
  const server = createServer(async (req, res) => {
    try {
      const chunks = [];
      for await (const chunk of req) chunks.push(chunk);
      const args = Buffer.concat(chunks).toString().split("\x1f").filter(Boolean);
      const result = await handler(req.url, args);
      res.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify(result));
    } catch (error) { res.writeHead(400).end(JSON.stringify({ error: String(error) })); }
  });
  await new Promise(done => server.listen(0, "127.0.0.1", done));
  const env = { PATH: process.env.PATH, HOME: process.env.HOME, TMPDIR: process.env.TMPDIR,
    ZEROCODE_HOOK_PORT: String(server.address().port), ZEROCODE_HOOK_TOKEN: "baseline-canary",
    ZEROCODE_BROWSER_TOKEN: "baseline-canary", ZEROCODE_AGENT_TEAM_TOKEN: "baseline-canary",
    ZEROCODE_AGENT_TEAM_ID: "baseline", ZEROCODE_AGENT_TEAM_PANE: "%1" };
  return { env, close: () => new Promise(done => server.close(done)) };
}
export async function handoff(root) {
  const ledger = { task: "Q6", result: null, receipts: [], commands: [] };
  const retries = new Map();
  const api = await endpoint(async (url, args) => {
    assert.equal(url, "/agent-teams");
    const option = name => args[args.indexOf(name) + 1];
    assert(args.includes("--retry-request"));
    const retry = option("--retry-request");
    if (retries.has(retry)) return retries.get(retry);
    if (args[0] === "send") {
      assert.equal(option("--type"), "worker_done");
      const body = JSON.parse(option("--body"));
      assert.deepEqual(body, { ok: true });
      ledger.receipts.push({ type: "worker_done", body });
    } else {
      assert.equal(args[0], "task-update");
      assert.equal(option("--task"), "Q6");
      const result = JSON.parse(option("--result"));
      assert(validResult(result), "result schema");
      ledger.result = result;
    }
    ledger.commands.push(args[0]);
    await writeFile(join(root, "evidence/ledger.json"), JSON.stringify(ledger));
    const receipt = { ok: true, task: ledger.task };
    retries.set(retry, receipt);
    return receipt;
  });
  try {
    const { stdout } = await run("git", ["diff", "--name-only"], { cwd: join(root, "repo") });
    const result = { ok: true, files: stdout.trim().split("\n").filter(Boolean), summary: "Updated handoff status" };
    await run("zerocode-orc", ["send", "--type", "worker_done", "--retry-request", "baseline-done", "--body", JSON.stringify({ ok: true })], { env: api.env });
    await run("zerocode-orc", ["task-update", "--task", "Q6", "--retry-request", "baseline-result", "--result", JSON.stringify(result)], { env: api.env });
  } finally { await api.close(); }
  return ledger;
}
export async function browserEvidence(browser, root) {
  const page = await browser.newPage();
  const files = createServer(async (_req, res) => res.end(await readFile(join(root, "repo/js/page.html"))));
  await new Promise(done => files.listen(0, "127.0.0.1", done));
  const url = `http://127.0.0.1:${files.address().port}/`;
  const api = await endpoint(async (path, args) => {
    assert.equal(path, "/browser");
    if (args[0] === "open") { assert.equal(args[1], url); await page.goto(url); return { label: "baseline" }; }
    assert.equal(args[1], "baseline");
    if (args[0] === "read") return { text: await page.locator("#status").innerText() };
    assert.equal(args[0], "screenshot");
    const png = resolve(args[args.indexOf("--out") + 1]);
    assert.equal(png, resolve(root, "evidence/page.png"));
    await page.screenshot({ path: png });
    return { path: png };
  });
  try {
    const submitted = performance.now();
    await run("zerocode-browser", ["open", url], { env: api.env });
    await page.evaluate(() => new Promise(done => requestAnimationFrame(() => requestAnimationFrame(done))));
    const visible = performance.now() - submitted;
    const read = await run("zerocode-browser", ["read", "baseline", "#status"], { env: api.env });
    await writeFile(join(root, "evidence/browser.json"), JSON.stringify({ dom: JSON.parse(read.stdout).text, command: "zerocode-browser read" }));
    await run("zerocode-browser", ["screenshot", "baseline", "--out", join(root, "evidence/page.png"), "--json"], { env: api.env });
    return visible;
  } finally { await api.close(); await page.close(); await new Promise(done => files.close(done)); }
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  assert.equal(process.argv[2], "handoff");
  await handoff(resolve(process.argv[3]));
}
