// 새로 고침을 누른 뒤 사용량이 막대에 서기까지(t-6583).
//
// 백엔드의 읽기는 `landMs` 뒤에 내려앉는다 — OAuth 한 왕복(09-24 실측 300~450 ms)
// 이나 숨은 터미널(수 초)처럼. 누름에서 새 수치가 막대에 서기까지를 재고, 그 사이
// 직전 수치가 서 있고 막대의 새로 고침이 도는지도 본다: 읽는 동안 수치가 비면
// 사람은 「안 가져온다」고 읽는다.
//
// 재는 값은 `USAGE_REFRESH_REPORT=1`일 때 한 줄씩 찍힌다 — 보고서의 전/후 표가
// 이 줄에서 나온다.
export async function testUsageRefresh(browser, origin, standBackend, ok) {
  const page = await browser.newPage();
  await standBackend(page);
  await page.addInitScript(() => {
    const base = window.__TAURI__.core.invoke;
    window.__USAGE_READ__ = { enabled: false, landMs: 0, outAt: null, asks: 0, held: null, fresh: null };
    window.__TAURI__.core.invoke = (name, args) => {
      const read = window.__USAGE_READ__;
      if (name !== "claude_usage" || !read.enabled) return base(name, args);
      read.asks += 1;
      if (args?.force && read.outAt === null) read.outAt = performance.now();
      const out = read.outAt !== null && performance.now() - read.outAt < read.landMs;
      const landed = read.outAt !== null && !out;
      return Promise.resolve({ usage: landed ? read.fresh : read.held, fetching: out });
    };
  });
  await page.goto(`${origin}/index.html`);
  await page.waitForFunction(() => typeof BOUND !== "undefined" && BOUND.size > 0);

  const pressAndWait = (landMs) =>
    page.evaluate(async (landMs) => {
      const read = window.__USAGE_READ__;
      const figure = (percent) => ({
        provider: "claude",
        session: { used_percent: percent, window_minutes: 300, resets_at: Date.now() + 3_600_000 },
        weekly: null,
        fable_weekly: null,
        updated_at: Date.now(),
        error: null,
        status: "ok",
      });
      read.held = figure(20);
      read.fresh = figure(61);
      read.landMs = landMs;
      read.outAt = null;
      read.asks = 0;
      read.enabled = true;
      try {
        // The bar has been reading all along: the held figure stands first.
        await refreshClaudeUsage(false);
        const pct = document.getElementById("sb-claude-pct");
        const button = document.getElementById("sb-usage-refresh");
        const heldText = pct.textContent;
        read.asks = 0;
        const started = performance.now();
        button.click();
        await new Promise((done) => setTimeout(done, 60));
        const during = {
          text: pct.textContent,
          shown: !pct.hidden,
          spins: button.classList.contains("is-refreshing"),
          disabled: button.disabled,
        };
        while (!pct.textContent.includes("61") && performance.now() - started < 15_000) {
          await new Promise((done) => requestAnimationFrame(done));
        }
        const paintedMs = Math.round(performance.now() - started);
        return { landMs, heldText, during, paintedMs, asks: read.asks, text: pct.textContent };
      } finally {
        read.enabled = false;
        for (const handle of usageAskTimers.values()) clearTimeout(handle);
        usageAskTimers.clear();
      }
    }, landMs);

  const oauth = [];
  for (let run = 0; run < 3; run += 1) oauth.push(await pressAndWait(450));
  const terminal = await pressAndWait(5_000);
  if (process.env.USAGE_REFRESH_REPORT) {
    for (const row of [...oauth, terminal]) {
      console.log(`USAGE_REFRESH land=${row.landMs} painted=${row.paintedMs} asks=${row.asks}`);
    }
  }

  // The figure, not the countdown beside it: the reset clock may turn over a
  // minute between two reads of the same snapshot.
  const first = oauth[0];
  ok(
    "while a read is out the previous figure stands and the bar's refresh spins",
    first.heldText.startsWith("20%") &&
      first.during.text.startsWith("20%") &&
      first.during.shown &&
      first.during.spins &&
      first.during.disabled,
    JSON.stringify(first),
  );
  ok(
    "the fresh figure reaches the bar",
    [...oauth, terminal].every((row) => row.text.includes("61")),
    JSON.stringify([...oauth, terminal]),
  );
  // An OAuth read lands in under half a second; the bar must not stand on the
  // old figure for a fixed two seconds after it did (t-6583).
  const slowest = Math.max(...oauth.map((row) => row.paintedMs));
  ok(
    "a read that lands in 450 ms is on the bar within 1.2 s of the press",
    slowest < 1_200,
    JSON.stringify(oauth.map((row) => row.paintedMs)),
  );
  // And a long read is painted when it always was, and is not polled without
  // end: quick asks for the first two seconds, then the old two-second ticks
  // counted from the press — about eleven asks where the fixed cadence made
  // four, and a quick poll left running would make twenty.
  ok(
    "a five-second read is painted on the old tick and asked about fewer than fifteen times",
    terminal.asks < 15 && terminal.paintedMs < 6_600,
    JSON.stringify(terminal),
  );
  await page.close();
}
