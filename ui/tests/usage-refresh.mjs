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

/* 수치가 없을 때 조각이 서는 낱말 (t-7170).
 *
 * 실보고(2026-09-24 「지금 계속 claude는 사용량 표시가 안됨」「그록도」): 계정 전환·
 * 재시작 직후 상태 바의 Claude 조각이 아이콘만 남았고(읽기가 401로 끝나면 삼각형만),
 * Grok 조각은 「···」로만 섰다 — 만료 안내는 툴팁에만 있었다. 여기서는 백엔드의 답을
 * 바꿔 가며 조각을 읽는다: 아직 답이 없음, 읽는 중, 오류(낡은 토큰·망·만료), 읽을
 * 수 없음, 빈 답, 로그아웃. 어느 경우에도 보이는 조각은 낱말 없이 서지 않는다.
 *
 * 재는 값 셋은 `USAGE_REFRESH_REPORT=1`일 때 한 줄씩 찍힌다 — 부팅 뒤 첫 Claude
 * 수치까지, 계정 전환 뒤 새 계정의 수치까지, Grok 안내가 보이기까지. */
export async function testUsageWords(browser, origin, standBackend, ok) {
  const page = await browser.newPage();
  await standBackend(page);
  // 부팅의 첫 물음: 백엔드의 읽기는 첫 물음이 나간 뒤 `landMs` 뒤에 내려앉는다
  // (OAuth 한 왕복, 09-24 실측 300~450 ms). 그 전까지는 읽는 중이고 캐시는 없다.
  await page.addInitScript(() => {
    const base = window.__TAURI__.core.invoke;
    const figure = (account, percent) => ({
      provider: "claude",
      session: { used_percent: percent, window_minutes: 300, resets_at: Date.now() + 3_600_000 },
      weekly: null,
      fable_weekly: null,
      updated_at: Date.now(),
      error: null,
      status: "ok",
      account,
    });
    window.__FIGURE__ = figure;
    window.__BOOT_READ__ = { landMs: 450, outAt: null, on: true };
    window.__TAURI__.core.invoke = (name, args) => {
      const read = window.__BOOT_READ__;
      if (name !== "claude_usage" || !read.on) return base(name, args);
      if (read.outAt === null) read.outAt = performance.now();
      const out = performance.now() - read.outAt < read.landMs;
      return Promise.resolve({ usage: out ? null : figure("a", 61), fetching: out });
    };
    // 조각 하나를 읽는 손: 보이는 낱말(수치 자리 하나가 수치도 낱말도 든다),
    // 삼각형, 호버.
    window.__SEG__ = (id) => {
      const segment = document.getElementById(id);
      const figure = document.getElementById(`${id}-pct`);
      const said = figure.hidden ? "" : figure.textContent.trim();
      return {
        hidden: segment.hidden,
        said,
        warn: !document.getElementById(`${id}-warn`).hidden,
        tip: segment.dataset.tip ?? "",
      };
    };
  });
  await page.goto(`${origin}/index.html`);
  await page.waitForFunction(() => typeof BOUND !== "undefined" && BOUND.size > 0);

  const clearAsks = () => page.evaluate(() => {
    for (const handle of usageAskTimers.values()) clearTimeout(handle);
    usageAskTimers.clear();
  });

  // ---- 재시작 뒤 첫 수치까지 ------------------------------------------------
  const boot = await page.evaluate(async () => {
    const pct = document.getElementById("sb-claude-pct");
    const started = performance.now();
    const during = window.__SEG__("sb-claude");
    while (!pct.textContent.includes("61%") && performance.now() - started < 15_000) {
      await new Promise((done) => requestAnimationFrame(done));
    }
    const painted = performance.now();
    const read = window.__BOOT_READ__;
    read.on = false;
    return {
      during,
      fromAskMs: read.outAt === null ? null : Math.round(painted - read.outAt),
      fromNavigationMs: Math.round(painted),
      text: pct.textContent,
    };
  });
  await clearAsks();
  ok(
    "at boot, while the first Claude read is out, the segment says it is checking rather than standing empty",
    !boot.during.hidden && boot.during.said === "확인 중…",
    JSON.stringify(boot.during),
  );
  ok(
    "the first Claude figure reaches the bar after the boot read lands",
    boot.text.includes("61%") && boot.fromAskMs !== null && boot.fromAskMs < 1_200,
    JSON.stringify(boot),
  );

  // ---- 낱말 없는 조각은 없다 ------------------------------------------------
  const states = await page.evaluate(async () => {
    const figure = window.__FIGURE__;
    const claude = (fields) => ({
      ...figure("a", 0),
      session: null,
      ...fields,
    });
    const answered = [
      ["nothing read yet", { usage: null, fetching: false }],
      ["a read out with no cache", { usage: null, fetching: true }],
      ["a stale token", { usage: claude({ status: "error", failure_kind: "stale-token", error: "Claude 로그인이 만료되었습니다" }), fetching: false }],
      ["the network down", { usage: claude({ status: "error", failure_kind: "network", error: "서버에 닿지 못했습니다" }), fetching: false }],
      ["an error of no named kind", { usage: claude({ status: "error", error: "알 수 없는 오류" }), fetching: false }],
      ["nothing set up", { usage: claude({ status: "unavailable", error: "사용량을 읽을 수 없습니다" }), fetching: false }],
      ["an answer with no window", { usage: claude({ status: "ok" }), fetching: false }],
      ["a signed-out login", { usage: claude({ status: "signed_out", error: "로그인되어 있지 않습니다" }), fetching: false }],
      ["a figure", { usage: figure("a", 61), fetching: false }],
    ];
    const seen = [];
    for (const [name, answer] of answered) {
      window.__ANSWER__.claude_usage = () => answer;
      await refreshClaudeUsage(false);
      for (const handle of usageAskTimers.values()) clearTimeout(handle);
      usageAskTimers.clear();
      seen.push({ name, ...window.__SEG__("sb-claude") });
    }
    delete window.__ANSWER__.claude_usage;
    return seen;
  });
  const wordless = states.filter((one) => !one.hidden && (one.said === "" || one.said === "···"));
  ok(
    "a_claude_segment_never_stands_empty_without_a_word: every state of the Claude segment carries a word",
    wordless.length === 0,
    JSON.stringify(wordless),
  );
  const said = Object.fromEntries(states.map((one) => [one.name, one]));
  ok(
    "the segment says what to do — checking, sign in again, retry, sign in — by the read's own kind",
    said["nothing read yet"].said === "확인 중…" &&
      said["a read out with no cache"].said === "확인 중…" &&
      said["a stale token"].said === "Claude 다시 로그인" && said["a stale token"].warn &&
      said["the network down"].said === "Claude 읽기 실패 — 다시 시도" && said["the network down"].warn &&
      said["an error of no named kind"].said === "Claude 읽기 실패 — 다시 시도" &&
      said["a signed-out login"].said === "Claude 로그인 필요" &&
      said["a figure"].said.startsWith("61%") && !said["a figure"].warn,
    JSON.stringify(said),
  );
  ok(
    "the read's own sentence stays on hover where the word is short",
    said["a stale token"].tip === "Claude 로그인이 만료되었습니다" &&
      said["the network down"].tip === "서버에 닿지 못했습니다",
    JSON.stringify([said["a stale token"].tip, said["the network down"].tip]),
  );

  // ---- 설정 스냅샷은 조각을 숨기는 유일한 길이다 --------------------------------
  const snapshot = await page.evaluate(() => {
    const segment = document.getElementById("sb-claude");
    const before = segment.hidden;
    applyAppearanceSettingsSnapshot({ status_bar_items: ["codex", "ports"] }, false);
    const off = segment.hidden;
    applyAppearanceSettingsSnapshot({}, false);
    const untouched = segment.hidden;
    applyAppearanceSettingsSnapshot({ status_bar_items: ["claude", "codex", "ports"] }, false);
    const on = segment.hidden;
    return { before, off, untouched, on };
  });
  ok(
    "the Claude segment hides only when the settings snapshot leaves it off, and a snapshot without the key leaves it alone",
    !snapshot.before && snapshot.off && snapshot.untouched && !snapshot.on,
    JSON.stringify(snapshot),
  );

  // ---- 계정 전환 뒤 새 계정의 수치까지 ------------------------------------------
  const switched = await page.evaluate(async (landMs) => {
    const figure = window.__FIGURE__;
    let outAt = null;
    let forced = 0;
    window.__ANSWER__.claude_usage = (args) => {
      if (args?.force) forced += 1;
      if (args?.force && outAt === null) outAt = performance.now();
      const out = outAt === null || performance.now() - outAt < landMs;
      // 백엔드는 전환에서 스냅샷을 잊는다(`forget_claude_usage`): 강제 읽기가 나가
      // 있는 동안 답은 비어 있고, 내려앉으면 새 계정의 수치다.
      return { usage: out ? null : figure("b", 7), fetching: out };
    };
    const pct = document.getElementById("sb-claude-pct");
    const started = performance.now();
    claudeLoginMoved();
    await new Promise((done) => setTimeout(done, 60));
    const during = window.__SEG__("sb-claude");
    while (!pct.textContent.startsWith("7%") && performance.now() - started < 15_000) {
      await new Promise((done) => requestAnimationFrame(done));
    }
    const paintedMs = Math.round(performance.now() - started);
    delete window.__ANSWER__.claude_usage;
    for (const handle of usageAskTimers.values()) clearTimeout(handle);
    usageAskTimers.clear();
    return { forced, during, paintedMs, text: pct.textContent };
  }, 450);
  ok(
    "a switched account forces one re-read, and the segment says it is checking until the new account's figure lands",
    switched.forced >= 1 && switched.during.said === "확인 중…" && switched.text.startsWith("7%") && switched.paintedMs < 1_200,
    JSON.stringify(switched),
  );

  // ---- Grok 만료 -------------------------------------------------------------
  const grok = await page.evaluate(async () => {
    const expired = {
      provider: "grok",
      session: null,
      weekly: null,
      fable_weekly: null,
      updated_at: Date.now(),
      error: "Grok 로그인이 만료되었습니다 — 이 컴퓨터에서 grok을 한 번 실행하세요",
      status: "error",
      failure_kind: "delegated-refresh-required",
      account: null,
    };
    window.__ANSWER__.grok_usage = () => ({ usage: expired, fetching: false });
    // The fixture boots with Grok off the bar; the person's bar carries it.
    applyAppearanceSettingsSnapshot({ status_bar_items: ["claude", "codex", "grok", "ports"] }, false);
    const started = performance.now();
    await refreshProviderUsage(usageProvider("grok"), false);
    const bare = { ...window.__SEG__("sb-grok"), wordMs: Math.round(performance.now() - started) };
    // 표가 프로그램의 이름을 안다: 안내의 낱말은 CLI 로그인 표의 행이 든 이름이다.
    window.__ANSWER__.cli_login_list = () => ({
      rows: [{
        agent: "grok", name: "Grok", program: "grok-cli", installed: true, signed_in: true,
        known: true, proof: "file", opens: "agent", home: "~/.grok", road: "verb", logout_road: "verb",
        account: "person@example.com",
      }],
    });
    await refreshCliLogins();
    paintUsageSegments();
    const named = window.__SEG__("sb-grok");
    // 로스터의 행: 만료된 Grok에는 손이 있고, 그 손은 계정 화면으로 간다.
    setUsageOpen(true);
    const row = document.querySelector('#usage-body .usage-roster-row[data-provider="grok"]');
    const button = row?.querySelector(".usage-roster-signin");
    const roster = {
      says: row?.querySelector(".usage-roster-says")?.textContent ?? null,
      button: button?.textContent ?? null,
    };
    button?.click();
    const landed = { settingsShown: !settingsView.hidden, pane: settingsPane };
    setSettingsOpen(false);
    if (!document.getElementById("usage-pop").hidden) setUsageOpen(false);
    delete window.__ANSWER__.grok_usage;
    delete window.__ANSWER__.cli_login_list;
    return { bare, named, roster, landed };
  });
  ok(
    "an_expired_grok_login_says_what_to_run_on_the_bar: the Grok segment says the login expired and names the program to run, inline and not only on hover",
    !grok.bare.hidden && grok.bare.said === "Grok 로그인 만료 — grok 실행" && grok.bare.warn &&
      grok.bare.tip === "Grok 로그인이 만료되었습니다 — 이 컴퓨터에서 grok을 한 번 실행하세요",
    JSON.stringify(grok.bare),
  );
  ok(
    "the program in the Grok word is the CLI login table's own name for it",
    grok.named.said === "Grok 로그인 만료 — grok-cli 실행",
    JSON.stringify(grok.named),
  );
  ok(
    "the roster's Grok row offers to run the program once, and the offer opens the accounts pane",
    grok.roster.button === "grok-cli 한 번 실행" && grok.landed.settingsShown && grok.landed.pane === "provider-accounts",
    JSON.stringify({ roster: grok.roster, landed: grok.landed }),
  );

  if (process.env.USAGE_REFRESH_REPORT) {
    console.log(`USAGE_WORDS boot first_figure_from_ask_ms=${boot.fromAskMs} from_navigation_ms=${boot.fromNavigationMs}`);
    console.log(`USAGE_WORDS switch new_figure_ms=${switched.paintedMs} during=${JSON.stringify(switched.during.said)}`);
    console.log(`USAGE_WORDS grok word_ms=${grok.bare.wordMs} said=${JSON.stringify(grok.bare.said)}`);
  }
  await page.close();
}
