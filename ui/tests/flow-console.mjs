import { readFile } from "node:fs/promises";
import { openWindowTestPage } from "./window-boot.mjs";

export async function testFlowConsole(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.evaluate(async () => { await openBrowserTab("https://fixture.example"); });
    const mirror = await page.evaluate(() => {
      const tab = currentTab();
      window.__FLOW_MIRROR_HOST__ = groupOf(tab.pane).browserView;
      return tab.id;
    });
    await page.evaluate(() => {
      flowData = { flows: [{ slug: "sample", name: "Sample", policy: "dry", evidence: "full", checks: { n: 1 }, fingerprint: {} }], policies: [], levels: [] };
      flowSelected = "sample";
      openFlowConsole();
      window.__FLOW_EMIT__ = (name, payload) => window.dispatchEvent(new CustomEvent(name, { detail: payload }));
      const emit = window.__FLOW_EMIT__;
      emit("flow:begin", { dir: "/evidence/a", name: "sample", kind: "recipe-run", cwd: activeWorktreePath });
      for (const n of [3, 1, 2, 2]) emit("flow:step", ["/evidence/a", { n, tool: "browser", verb: "click", argv: ["click", "page", "#ok"], ok: true, observation: { act_ms: 7, judgment: { asked: false }, retry: { name: "sample", step: n + 3, cwd: activeWorktreePath } } }]);
      emit("flow:step", ["/evidence/b", { n: 1, tool: "browser", verb: "type", argv: ["<img src=x>"], ok: false }]);
    });
    await page.waitForSelector('.flow-step[data-n="3"]');
    ok("Flow console reuses the existing browser pane and offers its mirror", await page.evaluate((id) =>
      el("flow-console-mirror").value === id && groupOf(currentTab().pane).browserView === window.__FLOW_MIRROR_HOST__, mirror));
    const mirrorBounds = await page.evaluate(() => {
      const body = groupOf(currentTab().pane).browserView.querySelector(".browser-body").getBoundingClientRect();
      const consoleBox = el("flow-console").getBoundingClientRect();
      return { right: body.right, consoleLeft: consoleBox.left };
    });
    ok("The native mirror's placement rectangle ends before the cards", mirrorBounds.right <= mirrorBounds.consoleLeft + 1, JSON.stringify(mirrorBounds));
    ok("Flow console orders and deduplicates the writer's numbers within its folder",
      JSON.stringify(await page.locator(".flow-step").evaluateAll((rows) => rows.map((row) => Number(row.dataset.n)))) === "[1,2,3]");
    ok("Recipe steps say no judgment, measured action ms, and do not invent look timing",
      await page.locator(".flow-step").first().textContent().then((text) => text.includes("판단 없음") && text.includes("7 ms") && text.includes("미측정")));
    for (const locale of ["ko", "en", "ja", "zh", "es"]) {
      await page.evaluate((code) => setLocale(code, { refresh: false, persist: false }), locale);
      ok(`Flow console ${locale} has translated labels`, await page.locator("#flow-console").textContent().then((text) => !text.includes("flow.live.") && (locale === "ko" || !text.includes("판단 없음"))));
    }
    await page.evaluate(() => {
      window.__FLOW_EMIT__("flow:walk", ["/evidence/a", { name: "sample", kind: "recipe-run", done: false, stop: { kind: "needs_a_look" }, stoppedAt: 5, ran: [{ evidence_n: 2, step: 5, phases: { act: 9, verify: 4 } }] }]);
    });
    await page.waitForSelector('.flow-step[data-n="2"] .flow-step-retry');
    await page.waitForFunction(() => el("flow-console-status").textContent === "needs_a_look");
    ok("Flow console paints the existing RecipeStop and its exact replay line",
      await page.locator("#flow-console").textContent().then((text) => text.includes("needs_a_look")) && await page.locator('.flow-step[data-n="2"] .flow-step-retry').isVisible());
    const replay = await page.evaluate(async () => {
      window.__FLOW_ARGV__ = [];
      window.__ANSWER__.flow_execute = ({ argv }) => { window.__FLOW_ARGV__.push(argv); return { done: true }; };
      el("flow-console-steps").querySelector('.flow-step[data-n="2"] .flow-step-retry').click();
      await flowLaunchTail;
      return window.__FLOW_ARGV__[0];
    });
    ok("Retry uses the existing runner with an exact one-line range", JSON.stringify(replay) === JSON.stringify(["recipe-run", "--name", "sample", "--start", "5", "--end", "5"]));
    const queued = await page.evaluate(async () => {
      const sent = []; const release = [];
      window.__ANSWER__.flow_execute = ({ argv }) => new Promise((done) => { sent.push(argv); release.push(done); });
      const first = flowLaunch(["recipe-run", "--name", "first"], "First");
      const second = flowLaunch(["recipe-run", "--name", "second"], "Second");
      await new Promise((done) => setTimeout(done, 0));
      const before = { sent: sent.length, pending: flowPending.length };
      release[0]({ done: true });
      await new Promise((done) => setTimeout(done, 0));
      const middle = { sent: sent.length, pending: flowPending.length };
      release[1]({ done: true }); await Promise.all([first, second]);
      return { before, middle, after: flowPending.length };
    });
    ok("The queue waits for the actual runner reply before submitting the next Flow", queued.before.sent === 1 && queued.before.pending === 2 && queued.middle.sent === 2 && queued.middle.pending === 1 && queued.after === 0, JSON.stringify(queued));
    const mobileGoals = await page.evaluate(async () => {
      const original = currentTab().id;
      const sent = [];
      window.__ANSWER__.flow_execute = (args) => { sent.push(args); return { done: true }; };
      for (const platform of ["ios", "android"]) {
        const id = `emulator:goal-${platform}`;
        openTab({ id, kind: "emulator", platform, deviceId: `${platform}-device`,
          udid: "transport-address", working: false, live: false, interactive: false, emulatorEpoch: 0 });
        setActiveTab(id);
        el("flow-goal").value = "일반 화면 열기";
        el("flow-goal-form").requestSubmit();
        await flowLaunchTail;
        closeTab(id);
      }
      setActiveTab(original);
      return sent;
    });
    ok("Mobile goals retain the selected platform and stable device identity",
      JSON.stringify(mobileGoals.map((call) => call.argv)) === JSON.stringify(["ios", "android"].map((platform) =>
        ["walk", "--goal", "일반 화면 열기", "--platform", platform, "--device", `${platform}-device`])), JSON.stringify(mobileGoals));
    const context = await page.evaluate(async () => {
      const original = activeWorktreePath;
      const sent = []; const release = [];
      window.__ANSWER__.flow_execute = ({ argv, worktree }) => new Promise((done) => {
        sent.push({ argv, workspace: worktree ?? activeWorktreePath }); release.push(done);
      });
      const first = flowLaunch(["recipe-run", "--name", "first"], "First");
      const second = flowLaunch(["recipe-run", "--name", "second"], "Second");
      await new Promise((done) => setTimeout(done, 0));
      activeWorktreePath = "/different-worktree";
      release[0]({ done: true });
      await new Promise((done) => setTimeout(done, 0));
      release[1]({ done: true }); await Promise.all([first, second]);
      activeWorktreePath = original;
      return { original, sent };
    });
    ok("A queued Flow keeps the worktree whose Jev consent the person selected",
      context.sent.length === 2 && context.sent.every((sent) => sent.workspace === context.original), JSON.stringify(context));
    const retryContext = await page.evaluate(async () => {
      const original = activeWorktreePath;
      const run = flowRuns.get("/evidence/a");
      flowRunDir = run.dir;
      activeWorktreePath = "/different-worktree";
      paintFlowConsole();
      let workspace;
      window.__ANSWER__.flow_execute = ({ worktree }) => { workspace = worktree ?? activeWorktreePath; return { done: true }; };
      el("flow-console-steps").querySelector('.flow-step[data-n="2"] .flow-step-retry').click();
      await flowLaunchTail;
      activeWorktreePath = original;
      return { original, workspace };
    });
    ok("Retry retains the recorded run's worktree after a project switch",
      retryContext.workspace === retryContext.original, JSON.stringify(retryContext));
    const legacyRetryDisabled = await page.evaluate(async () => {
      const run = flowRuns.get("/evidence/a");
      window.__ANSWER__.flow_evidence = () => ({ dir: "/legacy", report: "/legacy/report.html", steps: [...run.steps.values()].map((step) => ({ ...step, observation: { ...step.observation, retry: undefined } })), walk: { ...run.walk } });
      await readFlowEvidence("/legacy/report.html");
      const retry = el("flow-console-steps").querySelector('.flow-step[data-n="2"] .flow-step-retry');
      const disabled = !retry || retry.disabled;
      flowRunDir = "/evidence/a"; paintFlowConsole();
      return disabled;
    });
    ok("History without a recorded worktree cannot borrow the current project for retry", legacyRetryDisabled);
    await page.evaluate(() => {
      locale = "ko";
      window.__FLOW_EMIT__("flow:step", ["/evidence/a", {
        n: 4, tool: "browser", verb: "click", argv: ["<img src=x onerror=alert(1)>"], ok: true,
        observation: { look_ms: 12, act_ms: 8, judgment: { asked: true, ms: 34, confidence: 0.7 } },
      }]);
    });
    await page.waitForSelector('.flow-step[data-n="4"]');
    ok("Recovery shows its measured judgment and confidence, with log text escaped",
      await page.locator('.flow-step[data-n="4"]').textContent().then((text) => text.includes("34 ms") && text.includes("0.7") && !text.includes("판단 없음"))
      && await page.locator(".flow-step img").count() === 0);
    const noVerdict = await page.evaluate(() => {
      flowRuns.get(flowRunDir).walk = { kind: "recipe-run", done: true, complete: false, flow: { verdict: { pass: true } } };
      paintFlowConsole();
      return el("flow-console-status").textContent;
    });
    ok("A selected range cannot paint a whole Flow pass", noVerdict === "불합격");
    await page.evaluate(() => {
      window.__FLOW_EMIT__("flow:step", ["/evidence/a", {
        n: 5, tool: "browser", verb: "find", argv: ["find", "page", "Ready"], ok: true,
        observation: { elapsed_ms: 11, judgment: { asked: false } },
      }]);
    });
    await page.waitForSelector('.flow-step[data-n="5"]');
    ok("An oracle card shows its recorded command duration without inventing a look or press", await page.locator('.flow-step[data-n="5"]').textContent().then((text) => text.includes("명령 11 ms") && text.includes("판단 없음")));
    await testRecordedRetry(page, ok);
    await page.screenshot({ path: process.env.FLOW_CONSOLE_SCREENSHOT ?? "/tmp/t4849-flow-console.png" });
    const boundedHistory = await page.evaluate(async () => {
      const dir = "/bounded-history";
      const step = (n) => ({ n, tool: "browser", verb: "click", argv: [], ok: true });
      const snapshot = Array.from({ length: FLOW_LIVE_STEPS }, (_, n) => step(n + 1));
      for (const row of snapshot) window.__FLOW_EMIT__("flow:step", [dir, row]);
      let finish;
      window.__ANSWER__.flow_evidence = () => new Promise((resolve) => { finish = resolve; });
      const reading = readFlowEvidence(`${dir}/report.html`);
      for (let n = 1; n <= 3; n += 1) window.__FLOW_EMIT__("flow:step", [dir, step(FLOW_LIVE_STEPS + n)]);
      finish({ dir, report: `${dir}/report.html`, steps: snapshot, walk: null });
      await reading;
      const numbers = [...flowRuns.get(dir).steps.keys()];
      return { count: numbers.length, first: Math.min(...numbers), last: Math.max(...numbers), cap: FLOW_LIVE_STEPS };
    });
    ok("A late history snapshot keeps the cache bounded and retains newer live steps",
      boundedHistory.count === boundedHistory.cap && boundedHistory.first === 4
        && boundedHistory.last === boundedHistory.cap + 3, JSON.stringify(boundedHistory));
    const newerRunWins = await page.evaluate(async () => {
      let finish;
      window.__ANSWER__.flow_evidence = () => new Promise((resolve) => { finish = resolve; });
      const read = readFlowEvidence("/old/report.html");
      window.__FLOW_EMIT__("flow:begin", { dir: "/new", name: "new", kind: "recipe-run" });
      finish({ dir: "/old", report: "/old/report.html", steps: [], walk: null });
      await read;
      return flowRunDir;
    });
    ok("A late history read cannot replace a new live run", newerRunWins === "/new");
    await page.evaluate(() => closeFlowConsole());
    ok("Flow console closes without a second mirror stream", await page.locator("#flow-console").isHidden());
    ok("Flow console has no renderer faults", faults.length === 0, faults.join(" | "));
  } finally { await page.close(); }
}

/* These optional fixtures are produced by the real runner -> recorder Rust
 * regression. The default cases also keep the UI gate independently useful. */
async function testRecordedRetry(page, ok) {
  const retry = (name, step, cwd) => ({ name, step, cwd });
  const row = (n, origin) => ({ n, tool: "computer", verb: "key", argv: ["key", "--key", "tab"], ok: true, observation: origin ? { retry: origin } : {} });
  const origins = [retry("first", 1, "/original"), retry("first", 3, "/original"), retry("second", 2, "/other")];
  const defaults = [{
    case: "shared folder retains each actual origin", dir: "/provenance", steps: [row(2, origins[0]), row(3), row(4, origins[1]), row(5, origins[2])],
    walk: { name: "second", cwd: "/other", kind: "recipe-run", ran: [{ evidence_n: 2, step: 2 }, { evidence_n: 3, step: 1 }] },
    expected: [2, 4, 5].map((n, at) => ({ n, retry: origins[at] })),
  }];
  const fixtures = process.env.FLOW_RETRY_FIXTURES
    ? JSON.parse(await readFile(process.env.FLOW_RETRY_FIXTURES, "utf8")) : defaults;
  for (const fixture of fixtures) {
    for (const history of [false, true]) {
      const sent = await page.evaluate(async ({ fixture, history }) => {
        const calls = [];
        window.__ANSWER__.flow_execute = (args) => { calls.push(args); return { done: true }; };
        flowRuns.delete(fixture.dir);
        if (history) {
          window.__ANSWER__.flow_evidence = () => ({ ...fixture, report: `${fixture.dir}/report.html` });
          await readFlowEvidence(`${fixture.dir}/report.html`);
        } else {
          window.__FLOW_EMIT__("flow:begin", { dir: fixture.dir, name: fixture.walk.name, cwd: fixture.walk.cwd });
          for (const step of fixture.steps) window.__FLOW_EMIT__("flow:step", [fixture.dir, step]);
          window.__FLOW_EMIT__("flow:walk", [fixture.dir, fixture.walk]);
          paintFlowConsole();
        }
        const result = [];
        for (const step of fixture.steps) {
          const button = el("flow-console-steps").querySelector(`.flow-step[data-n="${step.n}"] .flow-step-retry`);
          if (!button || button.disabled) continue;
          const count = calls.length;
          button.click(); await flowLaunchTail;
          for (const call of calls.slice(count)) result.push({ n: step.n, ...call });
        }
        return result;
      }, { fixture, history });
      const expected = fixture.expected.map(({ n, retry: origin }) => ({ n,
        argv: ["recipe-run", "--name", origin.name, "--start", String(origin.step), "--end", String(origin.step)], worktree: origin.cwd }));
      ok(`Retry ${history ? "history" : "live"}: ${fixture.case} uses only the recorded line/name/cwd`, JSON.stringify(sent) === JSON.stringify(expected), JSON.stringify({ sent, expected }));
    }
  }
  const blocked = await page.evaluate(() => {
    const origins = [undefined, {}, { name: "r", step: 1 }, { name: "r", step: 0, cwd: "/a" }, { name: "r", step: 1.5, cwd: "/a" }, { name: "", step: 1, cwd: "/a" }, { name: "r", step: 1, cwd: "" }];
    const run = { name: "r", cwd: "/a", walk: { ran: [{ evidence_n: 1, step: 2 }] } };
    return origins.every((retry) => !flowStepCard(run, { n: 1, tool: "browser", verb: "find", observation: { retry } }).querySelector(".flow-step-retry"));
  });
  ok("Unknown, partial and invalid retry provenance cannot borrow a walk's mapping", blocked);
}
