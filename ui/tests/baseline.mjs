/* Separate quality suite, sharing exactly the full window gate's boot. */
import assert from "node:assert/strict";
import { readFile, writeFile, readdir, stat } from "node:fs/promises";
import { resolve, join } from "node:path";
import { BOOT, chromium, standBackend, createWindowServer } from "./window-boot.mjs";
import { browserEvidence, validResult, run } from "../../fixtures/quality-baseline/window-tools.mjs";
import { paneProbe } from "./baseline-probes.mjs";
import { BASE, table, options, newOutput, build, zo, execute, report } from "../../fixtures/quality-baseline/runner.mjs";

async function diskBytes(path) {
  const info = await stat(path);
  if (!info.isDirectory()) return info.size;
  const values = await Promise.all((await readdir(path)).map(name => diskBytes(join(path, name))));
  return values.reduce((sum, value) => sum + value, 0);
}

async function boot(browser, origin) {
  const page = await browser.newPage({ viewport: { width: 1280, height: 860 } });
  const faults = [];
  page.on("pageerror", error => faults.push(String(error)));
  await standBackend(page, BOOT);
  await page.goto(`${origin}/index.html`);
  await page.waitForFunction(() => typeof BOUND !== "undefined" && BOUND.size > 0);
  assert.deepEqual(faults, []);
  return page;
}
async function restoredWindow(root, id) {
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch();
  try {
    let page = await boot(browser, origin);
    const saved = await page.evaluate(sessionId => {
      const term = 890001;
      const tab = { id: "baseline-interrupted", kind: "term", worktree: "/baseline", layout: { type: "leaf", term }, activePane: term };
      const saves = [];
      window.__ANSWER__.save_pane_layouts = args => (saves.push(args), null);
      tabs.push(tab);
      for (const listener of window.__LISTENERS__["hook:agent"] ?? []) listener({ payload: { term, state: "working", agent: "zo", resumable: true, session: { key: "session_id", id: sessionId } } });
      const record = JSON.parse(JSON.stringify(saves[0]?.layouts?.[0]?.agents?.[0]));
      tabs.splice(tabs.indexOf(tab), 1);
      return record;
    }, id);
    assert.equal(saved.interrupted, true);
    await writeFile(join(root, "evidence/window-layout.json"), JSON.stringify(saved));
    await page.close();
    page = await boot(browser, origin);
    const result = await page.evaluate(async wake => {
      const calls = [];
      window.__ANSWER__.resume_session = args => (calls.push(args), { term: 890002, standing: false });
      const resumed = await spawnStoredLeaf(wake, null);
      const calm = await spawnStoredLeaf({ ...wake, interrupted: false }, null);
      return { resumed, calm, calls };
    }, JSON.parse(await readFile(join(root, "evidence/window-layout.json"), "utf8")));
    assert.equal(result.resumed.woke, true);
    assert.equal(result.calls[0].interrupted, true);
    assert.equal(result.calls[0].session.id, id);
    assert.equal(result.calls[1].interrupted, false, "idle restore must not receive a nudge");
    await writeFile(join(root, "evidence/window-resume.json"), JSON.stringify({ interrupted: result.calls[0].interrupted, id: result.calls[0].session.id, restarted: true, calm_interrupted: result.calls[1].interrupted }));
  } finally { await browser.close(); await new Promise(done => files.close(done)); }
}

if (process.argv[2] === "--restore") {
  await restoredWindow(resolve(process.argv[3]), process.argv[4]);
  console.log("PASS Q3 interrupted renderer restart and restore nudge");
} else {
  const args = process.argv.slice(2);
  const outAt = args.indexOf("--out");
  let output;
  if (outAt >= 0) { output = resolve(args[outAt + 1]); args.splice(outAt, 2); }
  const opts = options(args);
  const ownsOutput = !output;
  output ??= await newOutput();
  let binary;
  if (ownsOutput) {
    binary = await build(output);
    const tasks = opts.task ? [opts.task] : ["Q3", "Q6"];
    for (const task of tasks) await zo(binary, output, task, opts.repeat);
  }
  const template = JSON.parse(await readFile(join(output, "record-template.json"), "utf8"));
  let rows;
  try { rows = (await readFile(join(output, "runs.jsonl"), "utf8")).trim().split("\n").filter(Boolean).map(JSON.parse); } catch { rows = []; }
  // Result schema must reject a plausible but incomplete handoff before acceptance.
  assert(!validResult({ ok: true, summary: "done" }));
  assert(!validResult({ ok: true, files: ["x", "x"], summary: "done" }));
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch();
  try {
    const panePage = await boot(browser, origin);
    for (let repeat = 1; repeat <= opts.repeat; repeat++) {
      for (const task of ["Q3", "Q5", "Q6"]) {
        if (opts.task && opts.task !== task) continue;
        const root = join(output, `${task}-${repeat}`);
        if (task === "Q5") {
          await run("python3", [join(BASE, "fixture.py"), "prepare", root]);
          const diskBefore = await diskBytes(root);
          const start = performance.now();
          await run("git", ["apply", join(BASE, "answers/Q5.patch")], { cwd: join(root, "repo") });
          const visible = await browserEvidence(browser, root);
          const png = await readFile(join(root, "evidence/page.png"));
          assert.equal(png.subarray(0, 8).toString("hex"), "89504e470d0a1a0a");
          let verified = true;
          try { await run(join(BASE, "tasks/Q5/verify.sh"), [root]); } catch { verified = false; }
          rows.push({ ...structuredClone(template), task, surface: "window", repeat, verified, model: { requested: "scripted-window", effective: "scripted-window", effort: null }, account: { provider: "none", label: "hermetic" }, first_visible_ms: visible, elapsed_ms: Math.round(performance.now() - start), disk_delta_mb: ((await diskBytes(root)) - diskBefore) / (1024 * 1024), reason: verified ? null : "judge_failed" });
        } else {
          const row = rows.find(row => row.task === task && row.repeat === repeat);
          assert(row, `missing ${task}-${repeat} process run`);
          if (task === "Q3") {
            const restore = JSON.parse(await readFile(join(root, "evidence/window-resume.json"), "utf8"));
            assert(restore.interrupted && restore.restarted && !restore.calm_interrupted);
          } else {
            const ledger = JSON.parse(await readFile(join(root, "evidence/ledger.json"), "utf8"));
            assert(validResult(ledger.result));
            assert.deepEqual(ledger.commands, ["send", "task-update"]);
          }
          await run(join(BASE, `tasks/${task}/verify.sh`), [root]);
        }
        const row = rows.find(row => row.task === task && row.repeat === repeat);
        // Replay the task's visible result on the real pane renderer. This
        // isolates window paint from provider/tool timing and unrelated tabs.
        const measurement = await paneProbe(panePage, `${task} ${row.verified ? "verified" : "failed"}`, root, table);
        row.stream = measurement.stream;
        row.rss_peak_mb = Math.max(row.rss_peak_mb ?? 0, measurement.rss_peak_mb);
        row.host.mem = measurement.host_mem;
        assert(Number.isFinite(row.stream.paint_p95_ms) && row.stream.queue_max > 0);
        assert(row.rss_peak_mb > 0 && row.host.mem > 0);
      }
    }
  } finally { await browser.close(); await new Promise(done => files.close(done)); }
  const verdicts = new Map();
  for (const row of rows) {
    const verdict = JSON.stringify([row.verified, row.interventions, Object.keys(row).sort()]);
    if (verdicts.has(row.task)) assert.equal(verdicts.get(row.task), verdict, `non-deterministic ${row.task}`);
    verdicts.set(row.task, verdict);
  }
  await writeFile(join(output, "runs.jsonl"), rows.map(row => JSON.stringify(row)).join("\n") + "\n");
  if (binary) await execute(binary, ["record_schema_is_closed_and_secret_free", "--test-threads=1"], { BASELINE_VALIDATE: join(output, "runs.jsonl") });
  await report(output);
  assert(rows.every(row => row.verified && row.interventions === 0), "window judge failed");
  console.log(`PASS window baseline: ${output}`);
}
