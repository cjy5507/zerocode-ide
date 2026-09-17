// A slow authority reply must stay one request even as clock and event paints arrive.
export async function testLedgerPoll(browser, origin, standBackend, ok) {
  const page = await browser.newPage();
  await standBackend(page);
  await page.addInitScript(() => {
    const base = window.__TAURI__.core.invoke;
    window.__LEDGER_PROBE__ = { enabled: false, count: 0, active: 0, peak: 0, releases: [] };
    window.__TAURI__.core.invoke = (name, args) => {
      const probe = window.__LEDGER_PROBE__;
      if (name !== "ledger_agents" || !probe.enabled) return base(name, args);
      probe.count++; probe.active++; probe.peak = Math.max(probe.peak, probe.active);
      return new Promise((resolve, reject) => probe.releases.push((fail = false) => { probe.active--; fail ? reject(new Error("fixture delayed authority failure")) : resolve([]); }));
    };
  });
  await page.goto(`${origin}/index.html`);
  await page.waitForFunction(() => typeof BOUND !== "undefined" && BOUND.size > 0);
  const result = await page.evaluate(async () => {
    await new Promise(resolve => setTimeout(resolve, 0));
    const probe = window.__LEDGER_PROBE__;
    probe.enabled = true;
    const releases = probe.releases;
    try {
      const paints = [boardCards(), boardCards(), refreshPaneLedger()];
      // A pending answer outlives two real one-second poll periods.
      for (let tick = 0; tick < 2; tick++) {
        await new Promise(resolve => setTimeout(resolve, 1000));
        paints.push(boardCards(), refreshPaneLedger());
      }
      await Promise.resolve();
      const pending = { count: probe.count, peak: probe.peak };
      releases.splice(0).forEach(done => done());
      await Promise.all(paints);
      const next = boardCards();
      await Promise.resolve();
      const fresh = probe.count;
      releases.splice(0).forEach(done => done());
      await next;
      const rejected = [readLedgerAgents(), readLedgerAgents()];
      const failures = Promise.allSettled(rejected);
      releases.splice(0).forEach(done => done(true));
      const rejectedCount = (await failures).filter(row => row.status === "rejected").length;
      const retry = readLedgerAgents();
      const retried = probe.count;
      releases.splice(0).forEach(done => done());
      await retry;
      return { ...pending, fresh, rejectedCount, retried };
    } finally {
      releases.splice(0).forEach(done => done());
      probe.enabled = false;
    }
  });
  ok("ledger polls across board and navigator share one pending answer", result.count === 1 && result.peak === 1, JSON.stringify(result));
  ok("the next ledger poll reads again after completion", result.fresh === 2, JSON.stringify(result));
  ok("a rejected shared poll releases the slot for a fresh retry", result.rejectedCount === 2 && result.retried === 4, JSON.stringify(result));
  await page.close();
}
