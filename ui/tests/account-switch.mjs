// The Claude account switch, at the window's boundary (t-7538).
//
// Three contracts, each measured against the fixture's counters rather than
// against words on screen:
//
//   1. a manual switch touches NO pane — the old road queued every claude
//      pane and, for each one at `done`, launched a replacement and closed
//      the old shell (2026-09-24 21:2x: five workers `worker_died`). Here
//      seven panes stand (five working, one done, one waiting) and the
//      switch launches nothing and closes nothing.
//   2. `ask` proposes and applies only on the press, with the plan's own
//      token; `off` proposes nothing; `auto` applies once per token; a
//      `wait` decision says so and offers no button.
//   3. the window never moves a pane itself: the backend's answer names the
//      one walled pane it moved, and the window's launch/close counters
//      stay at zero.
//   4. (t-46106) every Claude pane says which account it runs as and that
//      account's share — the bar the default's, each sidebar row its own
//      login's — and a pane on another account says where it stands; a
//      switch's toast says what became of the panes in four counts.
export async function testAccountSwitch(browser, origin, standBackend, ok) {
  const page = await browser.newPage();
  await standBackend(page);
  await page.goto(`${origin}/index.html`);
  await page.waitForFunction(() => typeof BOUND !== "undefined" && BOUND.size > 0);

  const accounts = {
    accounts: [
      { id: "a-fixture", email: "a@example.test", label: "a@example.test", organization_type: "claude_max", signed_in: true, login_expired: false, added_at: 1, organization_uuid: "org-a", account_uuid: "acct-a" },
      { id: "b-fixture", email: "b@example.test", label: "b@example.test", organization_type: "claude_team", signed_in: true, login_expired: false, added_at: 2, organization_uuid: "org-b", account_uuid: "acct-b" },
    ],
    active: "a-fixture",
    can_add: true,
  };

  // ---- 1. a manual switch keeps working panes too --------------------
  const manual = await page.evaluate(async (accounts) => {
    window.__ACCOUNTS__ = accounts;
    accountReport = accounts;
    const terms = [];
    for (let at = 0; at < 7; at += 1) terms.push(await openTermTab({ placement: "tab", door: "terminal" }));
    const states = ["working", "working", "working", "working", "working", "done", "needs-attention"];
    terms.forEach((term, at) => {
      paneAgents.set(term, "claude");
      hookStates.set(term, states[at]);
      paneSessions.set(term, { session: { key: "session_id", id: `s-${term}` }, resumable: true });
    });
    window.__LAUNCHES__ = 0;
    window.__CLOSED__ = [];
    await pickClaudeAccount("b-fixture");
    // The done pane's later turns end too — the old road's drain point.
    hookStates.set(terms[0], "done");
    hookStates.set(terms[5], "done");
    await new Promise((done) => setTimeout(done, 50));
    const said = {
      launches: window.__LAUNCHES__,
      closed: window.__CLOSED__.length,
      active: accountReport.active,
      queue: typeof accountHandoffQueue,
      panes: terms.filter((term) => paneAgents.get(term) === "claude").length,
    };
    for (const term of terms) {
      const tab = tabOfTerm(term);
      if (tab) dropTab(tab.id);
      dropTermView(term);
      hookStates.delete(term);
      paneAgents.delete(term);
      paneSessions.delete(term);
    }
    return said;
  }, accounts);
  ok(
    "a manual switch moves no pane from the window: seven claude panes, a switch, and not one launch or close from the window — following the default is the backend's road (t-46106)",
    manual.launches === 0 && manual.closed === 0 && manual.active === "b-fixture" &&
      manual.queue === "undefined" && manual.panes === 7,
    JSON.stringify(manual),
  );

  // ---- 2. ask / off / auto / wait -------------------------------------
  const plan = (mode, decision, token, extra = {}) => ({
    accounts: [
      { id: "a-fixture", organization_type: "claude_max", active: true,
        usage: { provider: "claude", session: { used_percent: 95, window_minutes: 300, resets_at: Date.now() + 3_600_000 }, weekly: { used_percent: 96, window_minutes: 10080, resets_at: Date.now() + 6 * 86_400_000 }, fable_weekly: null, updated_at: Date.now(), error: null, status: "ok", account: "a-fixture" },
        fetching: false },
      { id: "b-fixture", organization_type: "claude_team", active: false,
        usage: { provider: "claude", session: { used_percent: 10, window_minutes: 300, resets_at: Date.now() + 3_600_000 }, weekly: { used_percent: 20, window_minutes: 10080, resets_at: Date.now() + 6 * 86_400_000 }, fable_weekly: null, updated_at: Date.now(), error: null, status: "ok", account: "b-fixture" },
        fetching: false },
    ],
    plan: {
      mode,
      active: "a-fixture",
      decision,
      landing: decision.kind === "switch" ? decision.to : "a-fixture",
      fitness: [
        { id: "a-fixture", room_percent: 4, unfit: null, next_reset_ms: Date.now() + 3_600_000 },
        { id: "b-fixture", room_percent: 80, unfit: null, next_reset_ms: null },
      ],
      next: { id: "b-fixture", room_percent: 80, unfit: null, next_reset_ms: null },
      walled: [{
        worker: "w-1", term: 4242, account: "a-fixture", model: "claude-opus-5-5", dispatch: "dp-1", generation: 1,
        verdict: decision.kind === "switch"
          ? { kind: "switch", from: "a-fixture", to: decision.to, reason: "walled" }
          : { kind: "stay", why: decision.why ?? "off" },
      }],
      last_switch_ms: extra.lastSwitchMs ?? null,
      cooldown_until_ms: extra.cooldownUntilMs ?? null,
      failed_recently: [],
      picked_at_ms: extra.pickedAtMs ?? null,
      token,
      now_ms: Date.now(),
    },
    sent: 0,
    fetching: false,
  });
  const switching = { kind: "switch", from: "a-fixture", to: "b-fixture", reason: "walled" };

  const asked = await page.evaluate(async ({ report }) => {
    window.__ACCOUNTS__ = { ...window.__ACCOUNTS__, active: "a-fixture" };
    accountReport = window.__ACCOUNTS__;
    window.__ACCOUNT_USAGE__ = report;
    window.__SWITCH_APPLIES__ = [];
    window.__SWITCH_APPLIED__ = {
      from: "a-fixture", to: "b-fixture", switched_default: true, default_ms: 3,
      panes: [{ worker: "w-1", from_term: 4242, to_term: 4243, ok: true, why: null, ms: 120 }],
      receipts: 2, total_ms: 130,
    };
    window.__LAUNCHES__ = 0;
    window.__CLOSED__ = [];
    showSettingsPane("provider-accounts");
    await refreshClaudeAccountUsage(false);
    const note = document.getElementById("account-switch-note");
    const button = document.getElementById("account-switch-now");
    // The same proposal read again offers no second notice.
    await refreshClaudeAccountUsage(false);
    const notices = [...document.querySelectorAll(".toast")]
      .filter((one) => one.querySelector(".toast-action") && one.textContent.includes("b-fixture"));
    const before = {
      notices: notices.length,
      noticeSaid: notices[0]?.querySelector(".toast-text")?.textContent ?? "",
      noteShown: !note.hidden,
      buttonShown: !button.hidden,
      said: document.getElementById("account-switch-said").textContent,
      applies: window.__SWITCH_APPLIES__.length,
      gaugeB: document.querySelector('[data-account="b-fixture"]')?.textContent ?? "",
      gaugeA: document.querySelector('[data-account="a-fixture"]')?.textContent ?? "",
      next: document.getElementById("sb-claude-next").textContent,
      nextShown: !document.getElementById("sb-claude-next").hidden,
    };
    button.click();
    await new Promise((done) => setTimeout(done, 80));
    const applied = window.__SWITCH_APPLIES__[0] ?? null;
    return {
      before,
      applies: window.__SWITCH_APPLIES__.length,
      by: applied?.by ?? null,
      token: applied?.token ?? null,
      carried: Object.keys(applied ?? {}).sort().join(","),
      launches: window.__LAUNCHES__,
      closed: window.__CLOSED__.length,
      active: accountReport.active,
      toast: document.querySelector(".toast:last-of-type")?.textContent ?? "",
    };
  }, { report: plan("ask", switching, "tok-ask-1") });
  ok(
    "ask mode proposes with the plan's words — in the accounts pane and once as a notice with its button — and applies only on the press, by its token and nothing else (what the panes run is the backend's to read); the window launches and closes nothing",
    asked.before.noteShown && asked.before.buttonShown && asked.before.applies === 0 &&
      asked.before.said.includes("b-fixture") &&
      asked.before.notices === 1 && asked.before.noticeSaid === asked.before.said &&
      asked.before.gaugeB.includes("10%") && asked.before.gaugeB.includes("80%") &&
      asked.before.gaugeA.includes("96%") &&
      asked.before.nextShown && asked.before.next.includes("b-fixture") && asked.before.next.includes("80%") &&
      asked.applies === 1 && asked.by === "ask" && asked.token === "tok-ask-1" &&
      asked.carried === "by,token" &&
      asked.launches === 0 && asked.closed === 0,
    JSON.stringify(asked),
  );
  ok(
    "the new surfaces name an account by its plan type and id — no address in the proposal, the bar or the gauge line",
    !asked.before.said.includes("@") && !asked.before.next.includes("@") &&
      !asked.before.gaugeB.includes("@") && asked.before.said.includes("b-fixture"),
    JSON.stringify(asked.before),
  );

  const off = await page.evaluate(async ({ report }) => {
    window.__ACCOUNT_USAGE__ = report;
    window.__SWITCH_APPLIES__ = [];
    await refreshClaudeAccountUsage(false);
    return {
      noteShown: !document.getElementById("account-switch-note").hidden,
      applies: window.__SWITCH_APPLIES__.length,
      next: document.getElementById("sb-claude-next").textContent,
    };
  }, { report: plan("off", { kind: "stay", why: "off" }, "tok-off-1") });
  ok(
    "off mode proposes nothing and applies nothing, and the bar still names the next account with its room",
    !off.noteShown && off.applies === 0 && off.next.includes("80%"),
    JSON.stringify(off),
  );

  const auto = await page.evaluate(async ({ report }) => {
    window.__ACCOUNT_USAGE__ = report;
    window.__SWITCH_APPLIES__ = [];
    window.__LAUNCHES__ = 0;
    window.__CLOSED__ = [];
    await refreshClaudeAccountUsage(false);
    await new Promise((done) => setTimeout(done, 80));
    const once = window.__SWITCH_APPLIES__.length;
    // The same plan again — the same token — applies nothing more.
    await refreshClaudeAccountUsage(false);
    await new Promise((done) => setTimeout(done, 80));
    return {
      once,
      twice: window.__SWITCH_APPLIES__.length,
      by: window.__SWITCH_APPLIES__[0]?.by ?? null,
      noteShown: !document.getElementById("account-switch-note").hidden,
      launches: window.__LAUNCHES__,
      closed: window.__CLOSED__.length,
    };
  }, { report: plan("auto", switching, "tok-auto-1") });
  ok(
    "auto mode applies the plan once per token without a press, proposes nothing, and the window still launches and closes nothing",
    auto.once === 1 && auto.twice === 1 && auto.by === "auto" && !auto.noteShown &&
      auto.launches === 0 && auto.closed === 0,
    JSON.stringify(auto),
  );

  const waiting = await page.evaluate(async ({ report }) => {
    window.__ACCOUNT_USAGE__ = report;
    window.__SWITCH_APPLIES__ = [];
    await refreshClaudeAccountUsage(false);
    await new Promise((done) => setTimeout(done, 50));
    return {
      noteShown: !document.getElementById("account-switch-note").hidden,
      buttonShown: !document.getElementById("account-switch-now").hidden,
      said: document.getElementById("account-switch-said").textContent,
      applies: window.__SWITCH_APPLIES__.length,
      next: document.getElementById("sb-claude-next").textContent,
    };
  }, { report: plan("auto", { kind: "wait", until_ms: Date.now() + 300_000, minutes: 5 }, "tok-wait-1") });
  ok(
    "a wait decision is said with its minutes, offers no button and applies nothing — even under auto",
    waiting.noteShown && !waiting.buttonShown && waiting.said.includes("5") &&
      waiting.applies === 0 && waiting.next.includes("5"),
    JSON.stringify(waiting),
  );

  // The words the table says when it has no number to show: nobody to move
  // to, a switch resting, a reading that could not be trusted — each said,
  // never a 0% or an empty chip (astra C2).
  const words = await page.evaluate(async ({ none, cooling, unread }) => {
    const read = async (report) => {
      window.__ACCOUNT_USAGE__ = report;
      window.__SWITCH_APPLIES__ = [];
      await refreshClaudeAccountUsage(false);
      return {
        next: document.getElementById("sb-claude-next").textContent,
        shown: !document.getElementById("sb-claude-next").hidden,
        applies: window.__SWITCH_APPLIES__.length,
      };
    };
    const noneReport = { ...none, plan: { ...none.plan, next: null } };
    return {
      none: await read(noneReport),
      cooling: await read(cooling),
      unread: await read(unread),
    };
  }, {
    none: plan("ask", { kind: "stay", why: "no_candidate" }, "tok-none"),
    cooling: plan("auto", { kind: "stay", why: "cooldown" }, "tok-cool", {
      lastSwitchMs: Date.now() - 60_000, cooldownUntilMs: Date.now() + 4 * 60_000,
    }),
    unread: plan("ask", { kind: "stay", why: "unread" }, "tok-unread"),
  });
  ok(
    "no candidate, a resting switch and an unread gauge are each said in words, with nothing applied",
    words.none.shown && /없음|none|no next/i.test(words.none.next) && !/\d+%/.test(words.none.next) &&
      words.cooling.shown && /4/.test(words.cooling.next) && words.cooling.applies === 0 &&
      words.unread.shown && /읽지 못함|unread/i.test(words.unread.next) && words.unread.applies === 0,
    JSON.stringify(words),
  );

  // An account the person picked themselves (t-78096): past the warn line
  // the table leaves it until its wall — the bar and the accounts screen say
  // why, there is no button, and `auto` applies nothing.
  const picked = await page.evaluate(async ({ report }) => {
    window.__ACCOUNT_USAGE__ = report;
    window.__SWITCH_APPLIES__ = [];
    showSettingsPane("provider-accounts");
    await refreshClaudeAccountUsage(false);
    return {
      next: document.getElementById("sb-claude-next").textContent,
      noteShown: !document.getElementById("account-switch-note").hidden,
      buttonShown: !document.getElementById("account-switch-now").hidden,
      said: document.getElementById("account-switch-said").textContent,
      applies: window.__SWITCH_APPLIES__.length,
    };
  }, {
    report: plan("auto", { kind: "stay", why: "picked" }, "tok-picked", {
      pickedAtMs: Date.now() - 60_000,
    }),
  });
  ok(
    "an account the person picked is kept and said so: the bar and the accounts line say why, no button, nothing applied under auto (t-78096)",
    picked.noteShown && !picked.buttonShown && /고른|picked/i.test(picked.said) &&
      /고른|pick/i.test(picked.next) && picked.applies === 0,
    JSON.stringify(picked),
  );
  // 쉬운 말(t-80303 ④): 「벽」은 코드베이스 말이라 화면 문구에서 뺀다 — 한도라고 쓴다.
  ok(
    "the picked-account words say 한도, never 벽 (t-80303)",
    !/벽/.test(picked.said) && !/벽/.test(picked.next) && /한도/.test(picked.said) && /한도/.test(picked.next),
    JSON.stringify(picked),
  );
  // 상태 바(t-80303 ⑤): 고른 계정이 머무는 동안 「다음 B 80%」는 곧 옮긴다는 뜻으로
  // 읽히므로 서지 않는다 — 고른 조각 하나가 그 자리에 선다.
  ok(
    "while a picked account is kept the bar shows no 「다음 계정」 piece beside it (t-80303)",
    !/다음|next/i.test(picked.next) && !/80%/.test(picked.next) && /고른|pick/i.test(picked.next),
    JSON.stringify(picked),
  );

  // 고른 계정의 표시는 90% 아래에서도 보인다(t-80303 ⑥): 활성 계정 행이 「몇 시에 직접
  // 골랐고 한도까지 유지된다」고 말한다. 자동 전환이 꺼져 있거나 고른 적이 없으면 말하지 않는다.
  const pickedRow = await page.evaluate(async ({ asRoom, asOff, asNone }) => {
    const read = async (report) => {
      window.__ACCOUNT_USAGE__ = report;
      showSettingsPane("provider-accounts");
      await refreshClaudeAccountUsage(false);
      const gauge = (id) => document.querySelector(`.account-gauge[data-account="${id}"]`)?.textContent ?? "";
      return { active: gauge("a-fixture"), other: gauge("b-fixture") };
    };
    return { room: await read(asRoom), off: await read(asOff), none: await read(asNone) };
  }, {
    asRoom: plan("auto", { kind: "stay", why: "room" }, "tok-room", { pickedAtMs: Date.now() - 60_000 }),
    asOff: plan("off", { kind: "stay", why: "off" }, "tok-off-pick", { pickedAtMs: Date.now() - 60_000 }),
    asNone: plan("auto", { kind: "stay", why: "room" }, "tok-room-none"),
  });
  const pickedWords = /직접 고름|picked by you/i;
  ok(
    "a pick shows on the active row below the warn line, only there, only while the switch is on (t-80303)",
    pickedWords.test(pickedRow.room.active) && /한도|limit/i.test(pickedRow.room.active) &&
      !pickedWords.test(pickedRow.room.other) && !pickedWords.test(pickedRow.off.active) &&
      !pickedWords.test(pickedRow.none.active),
    JSON.stringify(pickedRow),
  );

  // A refused apply (the situation changed) surfaces as an error and the
  // window re-reads the accounts rather than pretending.
  const refused = await page.evaluate(async ({ report }) => {
    window.__ACCOUNT_USAGE__ = report;
    window.__SWITCH_APPLIES__ = [];
    window.__SWITCH_REFUSES__ = "상황이 바뀌어 전환하지 않았습니다 — 다시 확인하세요";
    await refreshClaudeAccountUsage(false);
    document.getElementById("account-switch-now").click();
    await new Promise((done) => setTimeout(done, 80));
    delete window.__SWITCH_REFUSES__;
    return { applies: window.__SWITCH_APPLIES__.length, active: accountReport.active };
  }, { report: plan("ask", switching, "tok-ask-2") });
  ok(
    "a refused apply is one refused call, and the window's account stays what the backend says",
    refused.applies === 1 && refused.active === "a-fixture",
    JSON.stringify(refused),
  );

  // ---- 4. a proposal's button applies that proposal and no other (astra R1) ----
  // The notice for proposal B is still on screen when the next beat brings
  // proposal C. B's button must not apply C: it carries B's token, and the
  // window — which already holds C — sends nothing for it. B's notice is
  // withdrawn when C arrives; C's own button applies C, once. A mode change
  // retires C's button the same way.
  const bound = await page.evaluate(async ({ b, c, off }) => {
    // A proposal's notice — not a switch's receipt, whose one button opens the
    // accounts and applies nothing (t-46106).
    const noticeOf = (who) => [...document.querySelectorAll(".toast")]
      .find((one) => one.querySelector(".toast-action") && !one.dataset.switchReceipt &&
        one.textContent.includes(who)) ?? null;
    window.__SWITCH_APPLIES__ = [];
    const errors = [];
    const showing = window.showError;
    window.showError = (error) => errors.push(String(error));
    try {
      window.__ACCOUNT_USAGE__ = b;
      await refreshClaudeAccountUsage(false);
      const noticeB = noticeOf("b-fixture");
      const buttonB = noticeB?.querySelector(".toast-action") ?? null;
      window.__ACCOUNT_USAGE__ = c;
      await refreshClaudeAccountUsage(false);
      const noticeC = noticeOf("c-fixture");
      const buttonC = noticeC?.querySelector(".toast-action") ?? null;
      const withdrawn = Boolean(noticeB) && !noticeB.isConnected;
      buttonB?.click();
      await new Promise((done) => setTimeout(done, 60));
      const afterB = window.__SWITCH_APPLIES__.map((one) => one.token);
      const refusedSaid = errors.length;
      buttonC?.click();
      await new Promise((done) => setTimeout(done, 60));
      const afterC = window.__SWITCH_APPLIES__.map((one) => one.token);
      // The mode moves to `off`: C's notice goes, and its button retires too.
      window.__ACCOUNT_USAGE__ = off;
      window.__SWITCH_APPLIES__ = [];
      await refreshClaudeAccountUsage(false);
      buttonC?.click();
      await new Promise((done) => setTimeout(done, 60));
      return {
        hadB: Boolean(buttonB), hadC: Boolean(buttonC), withdrawn,
        afterB, refusedSaid, afterC,
        cWithdrawn: Boolean(noticeC) && !noticeC.isConnected,
        afterOff: window.__SWITCH_APPLIES__.map((one) => one.token),
      };
    } finally {
      window.showError = showing;
    }
  }, {
    b: plan("ask", { kind: "switch", from: "a-fixture", to: "b-fixture", reason: "walled" }, "tok-proposal-b"),
    c: plan("ask", { kind: "switch", from: "a-fixture", to: "c-fixture", reason: "walled" }, "tok-proposal-c"),
    off: plan("off", { kind: "stay", why: "off" }, "tok-proposal-off"),
  });
  ok(
    "a proposal's button applies that proposal and no other: B's button pressed after C arrived applies nothing and says the proposal changed, B's notice is withdrawn, C's button applies C once, and a mode change retires C's button too",
    bound.hadB && bound.hadC && bound.withdrawn &&
      bound.afterB.length === 0 && bound.refusedSaid === 1 &&
      bound.afterC.length === 1 && bound.afterC[0] === "tok-proposal-c" &&
      bound.cWithdrawn && bound.afterOff.length === 0,
    JSON.stringify(bound),
  );

  // ---- 5. a quiet poll is quiet on screen too (astra E1) ----------------
  // The backend's quiet poll writes nothing; the window's answer to the
  // same report must change nothing on screen either. The same meaning read
  // again: zero DOM mutations in the accounts list, its count and note, the
  // proposal line and the bar's "next". A figure that moved rewrites its
  // gauge line; a countdown that crossed a minute rewrites its gauge line;
  // neither throws a row away.
  const quiet = await page.evaluate(async ({ report, moved, sooner }) => {
    showSettingsPane("provider-accounts");
    window.__ACCOUNT_USAGE__ = report;
    // Whatever the earlier sections set moving lands first.
    await new Promise((done) => setTimeout(done, 250));
    await refreshClaudeAccountUsage(false);
    await new Promise((done) => setTimeout(done, 250));
    const watched = ["account-list", "account-count", "account-note", "account-switch-note", "sb-claude-next", "account-add"]
      .map((id) => document.getElementById(id))
      .filter(Boolean);
    const seen = [];
    const watcher = new MutationObserver((records) => seen.push(...records));
    for (const node of watched) {
      watcher.observe(node, { subtree: true, childList: true, attributes: true, characterData: true });
    }
    const flush = async () => {
      await new Promise((done) => setTimeout(done, 30));
      const taken = seen.length + watcher.takeRecords().length;
      seen.length = 0;
      return taken;
    };
    const rowOf = (id) => document.querySelector(`#account-list [data-account-row="${id}"]`);
    const gaugeOf = (id) => document.querySelector(`#account-list .account-gauge[data-account="${id}"]`)?.textContent ?? "";
    const rowsBefore = [rowOf("a-fixture"), rowOf("b-fixture")];
    await refreshClaudeAccountUsage(false);
    await refreshClaudeAccountUsage(false);
    const same = await flush();
    window.__ACCOUNT_USAGE__ = moved;
    await refreshClaudeAccountUsage(false);
    const figure = await flush();
    const figureSaid = gaugeOf("b-fixture");
    window.__ACCOUNT_USAGE__ = sooner;
    await refreshClaudeAccountUsage(false);
    const countdown = await flush();
    const countdownSaid = gaugeOf("a-fixture");
    watcher.disconnect();
    const rowsAfter = [rowOf("a-fixture"), rowOf("b-fixture")];
    return {
      same, figure, figureSaid, countdown, countdownSaid,
      rowsKept: rowsBefore.every(Boolean) && rowsBefore.every((row, at) => row === rowsAfter[at]),
    };
  }, (() => {
    // Half a minute into each countdown's minute, so the test itself never
    // crosses one.
    const at = Date.now() + 30_000;
    const report = plan("ask", { kind: "stay", why: "room" }, "tok-quiet");
    for (const row of report.accounts) {
      row.usage.session.resets_at = at + 3 * 3_600_000;
      row.usage.weekly.resets_at = at + 6 * 86_400_000;
    }
    const moved = structuredClone(report);
    moved.accounts[1].usage.session.used_percent = 11;
    const sooner = structuredClone(moved);
    sooner.accounts[0].usage.session.resets_at -= 60_000;
    return { report, moved, sooner };
  })());
  ok(
    "the same report read again changes nothing on screen — zero mutations in the accounts list, its count and note, the proposal line and the bar — while a figure or a countdown that moved rewrites its own gauge line and keeps every row",
    quiet.same === 0 && quiet.figure > 0 && quiet.figureSaid.includes("11%") &&
      quiet.countdown > 0 && quiet.countdownSaid.includes("2h 59m") && quiet.rowsKept,
    JSON.stringify(quiet),
  );

  // ---- 6. a person's pick the ledger did not record is said (astra R4) ----
  const unrecorded = await page.evaluate(async (accounts) => {
    const errors = [];
    const showing = window.showError;
    window.showError = (error) => errors.push(String(error));
    try {
      window.__ACCOUNTS__ = { ...accounts, active: "a-fixture", switch_unrecorded: "원장이 전환 영수증을 거절했습니다: NotDurable" };
      accountReport = window.__ACCOUNTS__;
      await pickClaudeAccount("b-fixture");
      await pickSystemClaudeLogin();
      window.__ACCOUNTS__ = { ...accounts, active: "a-fixture" };
      accountReport = window.__ACCOUNTS__;
      await pickClaudeAccount("b-fixture");
      return { errors };
    } finally {
      window.showError = showing;
    }
  }, accounts);
  ok(
    "a person's pick the ledger refused to record is said as such — for a managed account and for the machine's own login — and a recorded pick says nothing",
    unrecorded.errors.length === 2 &&
      unrecorded.errors.every((said) => said.includes("NotDurable")),
    JSON.stringify(unrecorded),
  );

  // ---- 7. every pane says its account and share (t-46106) ----------------
  // The bar names the default by its short id; each Claude pane's row names
  // its own login's account and share; a pane left on the old account says
  // where it stands — the person's pane in a person's hands, a working one
  // moving at its turn's end. A pane the plan does not count (not Claude's)
  // wears nothing.
  const NEW = "acct-fixture-0554";
  const OLD = "acct-fixture-6252";
  const census = (panes, active = NEW) => ({
    accounts: [
      { id: NEW, organization_type: "claude_team", active: active === NEW,
        usage: { provider: "claude", session: { used_percent: 12, window_minutes: 300, resets_at: Date.now() + 3_600_000 }, weekly: { used_percent: 25, window_minutes: 10080, resets_at: Date.now() + 6 * 86_400_000 }, fable_weekly: null, updated_at: Date.now(), error: null, status: "ok", account: NEW },
        fetching: false },
      { id: OLD, organization_type: "claude_max", active: active === OLD,
        usage: { provider: "claude", session: { used_percent: 98, window_minutes: 300, resets_at: Date.now() + 3_600_000 }, weekly: { used_percent: 60, window_minutes: 10080, resets_at: Date.now() + 6 * 86_400_000 }, fable_weekly: null, updated_at: Date.now(), error: null, status: "ok", account: OLD },
        fetching: false },
    ],
    plan: {
      mode: "auto", active, decision: { kind: "stay", why: "cooldown" }, landing: active,
      fitness: [
        { id: NEW, room_percent: 75, unfit: null, next_reset_ms: null },
        { id: OLD, room_percent: 0, unfit: "blocked", next_reset_ms: Date.now() + 3_600_000 },
      ],
      next: null, walled: [], panes, last_switch_ms: Date.now() - 60_000,
      cooldown_until_ms: Date.now() + 4 * 60_000, failed_recently: [], token: `tok-census-${panes.length}`,
      now_ms: Date.now(),
    },
    sent: 0, fetching: false,
  });
  const shown = await page.evaluate(async ({ report, NEW }) => {
    window.__ACCOUNTS__ = { ...window.__ACCOUNTS__, active: NEW };
    accountReport = window.__ACCOUNTS__;
    window.__ACCOUNT_USAGE__ = report;
    window.__SWITCH_APPLIES__ = [];
    await refreshClaudeAccountUsage(false);
    claudeUsage = { ...report.accounts[0].usage };
    paintUsageSegments();
    const bar = document.getElementById("sb-claude-account");
    const chipOf = (term, agent = "claude") => makeAgentRow({
      term, state: "done", agent, depth: 0, tab: { worktree: null, id: `t-${term}` },
    }).querySelector(".wt-agent-account");
    const said = (term, agent) => {
      const chip = chipOf(term, agent);
      return chip ? { words: chip.textContent, tip: chip.dataset.tip ?? "" } : null;
    };
    const live = makeAgentRow({
      term: 4602, state: "working", agent: "claude", depth: 0, tab: { worktree: null, id: "t-4602" },
    }).querySelector(".wt-agent-meta .wt-agent-account");
    return {
      bar: bar.textContent, barShown: !bar.hidden, barTip: bar.dataset.tip ?? "",
      current: said(4601), turnEnd: said(4602), person: said(4603), left: said(4604),
      notCounted: said(4609, "codex"), liveInMeta: Boolean(live),
      next: document.getElementById("sb-claude-next").textContent,
      applies: window.__SWITCH_APPLIES__.length,
    };
  }, {
    report: census([
      { term: 4601, worker: "w-1", account: NEW, stance: "current" },
      { term: 4602, worker: "w-2", account: OLD, stance: "turn_end" },
      { term: 4603, worker: "w-3", account: OLD, stance: "person" },
      { term: 4604, worker: null, account: null, stance: "left" },
    ]),
    NEW,
  });
  ok(
    "the bar names the default account by its short id, each Claude pane's row says its own login's account and share, a pane on the old account says where it stands — the person's in a person's hands, a working one at its turn's end — and every chip explains itself on hover",
    shown.barShown && shown.bar === "a…0554" && shown.barTip.includes(NEW) &&
      // The fullest window is the share a pane is held to — the week's 25%,
      // over the session's 12%, as the bar's own figure picks.
      shown.current?.words === "a…0554 25%" && shown.current.tip.includes(NEW) &&
      shown.turnEnd?.words === "옛 계정 a…6252 98% — 턴 끝에 옮김" && shown.turnEnd.tip.includes(OLD) &&
      shown.person?.words === "옛 계정 a…6252 98% — 사람이 쓰는 중" && shown.person.tip.length > 0 &&
      shown.left?.words === "계정 모름" && shown.left.tip.length > 0 &&
      shown.notCounted === null && shown.liveInMeta &&
      /옛 계정 판 2|old accounts 2/.test(shown.next) && shown.applies === 0,
    JSON.stringify(shown),
  );

  // Two accounts whose ids end alike are told apart: the short name grows
  // until it is one account's alone.
  const alike = await page.evaluate(() => {
    window.__ACCOUNTS__ = {
      ...window.__ACCOUNTS__,
      accounts: [{ id: "acct-one-0554" }, { id: "acct-two-0554" }],
    };
    accountReport = window.__ACCOUNTS__;
    accountUsageReport = null;
    return [accountShortId("acct-one-0554"), accountShortId("acct-two-0554"), accountShortId("a-1")];
  });
  ok(
    "two account ids that end alike get short names that still tell them apart, and a short id is shown whole",
    alike[0] !== alike[1] && alike[0].startsWith("a…") && alike[0].endsWith("0554") && alike[2] === "a-1",
    JSON.stringify(alike),
  );

  // ---- 8. a switch's receipt says its four counts (t-46106) ---------------
  // The beat's switch and the person's pick say the same thing: what moved,
  // what moves at its turn's end, what is the person's, what is left — the
  // meaning on hover, and one real button that opens the accounts.
  const receipt = await page.evaluate(async ({ report, accounts, NEW, OLD }) => {
    const receipts = () => [...document.querySelectorAll(".toast[data-switch-receipt]")];
    for (const one of receipts()) one.remove();
    window.__ACCOUNTS__ = { ...accounts, accounts: [...accounts.accounts, { id: NEW }, { id: OLD }], active: OLD };
    accountReport = window.__ACCOUNTS__;
    window.__ACCOUNT_USAGE__ = report;
    window.__SWITCH_APPLIES__ = [];
    window.__SWITCH_APPLIED__ = {
      key: "switch-1", from: OLD, to: NEW, switched_default: true, default_ms: 3,
      panes: [], census: { moved: 5, at_turn_end: 2, person: 1, left: 0 }, receipts: 6, total_ms: 40,
    };
    await refreshClaudeAccountUsage(false);
    await new Promise((done) => setTimeout(done, 80));
    const beat = receipts().at(-1) ?? null;
    const button = beat?.querySelector(".toast-action") ?? null;
    const said = {
      words: beat?.querySelector(".toast-text")?.textContent ?? "",
      tip: beat?.dataset.tip ?? "",
      button: button?.tagName ?? null,
      buttonType: button?.getAttribute("type") ?? null,
      buttonTip: button?.dataset.tip ?? "",
    };
    button?.click();
    await new Promise((done) => setTimeout(done, 80));
    said.opened = !document.getElementById("settings-view")?.hidden && settingsPane === "provider-accounts";
    // The person's pick says the same four counts, out of the pick's answer.
    for (const one of receipts()) one.remove();
    window.__ACCOUNTS__ = { ...window.__ACCOUNTS__, active: OLD };
    accountReport = window.__ACCOUNTS__;
    const picked = window.__ACCOUNTS__;
    window.__ACCOUNTS__ = { ...picked, switched: { moved: 2, at_turn_end: 1, person: 0, left: 1 } };
    accountReport = { ...picked };
    await pickClaudeAccount(NEW);
    const pick = receipts().at(-1) ?? null;
    said.pick = pick?.querySelector(".toast-text")?.textContent ?? "";
    delete window.__SWITCH_APPLIED__;
    return said;
  }, {
    report: (() => {
      const switching = census([], OLD);
      switching.plan.decision = { kind: "switch", from: OLD, to: NEW, reason: { near_limit: { window: "session", used_percent: 92 } } };
      switching.plan.landing = NEW;
      switching.plan.token = "tok-census-switch";
      return switching;
    })(),
    accounts,
    NEW,
    OLD,
  });
  ok(
    "a switch's receipt says what became of the panes in four counts — moved, at their turn's end, the person's, left — with what each means on hover and a real button that opens the accounts; the person's pick says the same",
    /옮긴 판 5 · 턴 끝에 옮길 판 2 · 사람 판 1 · 남은 판 0/.test(receipt.words) &&
      receipt.tip.includes("턴 끝에 옮길 판") && receipt.button === "BUTTON" && receipt.buttonType === "button" &&
      receipt.buttonTip.length > 0 && receipt.opened &&
      /옮긴 판 2 · 턴 끝에 옮길 판 1 · 사람 판 0 · 남은 판 1/.test(receipt.pick),
    JSON.stringify(receipt),
  );

  await page.close();
}
