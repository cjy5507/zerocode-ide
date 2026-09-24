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
    "a manual switch keeps working panes too: seven claude panes, a switch, and not one launch or close from the window",
    manual.launches === 0 && manual.closed === 0 && manual.active === "b-fixture" &&
      manual.queue === "undefined" && manual.panes === 7,
    JSON.stringify(manual),
  );

  // ---- 2. ask / off / auto / wait -------------------------------------
  const plan = (mode, decision, token) => ({
    accounts: [
      { id: "a-fixture", label: "a@example.test", organization_type: "claude_max", active: true,
        usage: { provider: "claude", session: { used_percent: 95, window_minutes: 300, resets_at: Date.now() + 3_600_000 }, weekly: { used_percent: 96, window_minutes: 10080, resets_at: Date.now() + 6 * 86_400_000 }, fable_weekly: null, updated_at: Date.now(), error: null, status: "ok", account: "a-fixture" },
        fetching: false },
      { id: "b-fixture", label: "b@example.test", organization_type: "claude_team", active: false,
        usage: { provider: "claude", session: { used_percent: 10, window_minutes: 300, resets_at: Date.now() + 3_600_000 }, weekly: { used_percent: 20, window_minutes: 10080, resets_at: Date.now() + 6 * 86_400_000 }, fable_weekly: null, updated_at: Date.now(), error: null, status: "ok", account: "b-fixture" },
        fetching: false },
    ],
    plan: {
      mode,
      active: "a-fixture",
      decision,
      fitness: [
        { id: "a-fixture", room_percent: 4, unfit: null, next_reset_ms: Date.now() + 3_600_000 },
        { id: "b-fixture", room_percent: 80, unfit: null, next_reset_ms: null },
      ],
      walled: [{ worker: "w-1", term: 4242, account: "a-fixture", dispatch: "dp-1", generation: 1 }],
      last_switch_ms: null,
      failed_recently: [],
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
    paneModels.set(4242, "claude-fable-5-1");
    showSettingsPane("provider-accounts");
    await refreshClaudeAccountUsage(false);
    const note = document.getElementById("account-switch-note");
    const button = document.getElementById("account-switch-now");
    const before = {
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
      model: applied?.models?.["4242"] ?? null,
      launches: window.__LAUNCHES__,
      closed: window.__CLOSED__.length,
      active: accountReport.active,
    };
  }, { report: plan("ask", switching, "tok-ask-1") });
  ok(
    "ask mode proposes with the plan's words and applies only on the press — by its token, with the panes' models, and the window launches and closes nothing",
    asked.before.noteShown && asked.before.buttonShown && asked.before.applies === 0 &&
      asked.before.said.includes("b@example.test") &&
      asked.before.gaugeB.includes("10%") && asked.before.gaugeB.includes("80%") &&
      asked.before.gaugeA.includes("96%") &&
      asked.before.nextShown && asked.before.next.includes("b@example.test") && asked.before.next.includes("80%") &&
      asked.applies === 1 && asked.by === "ask" && asked.token === "tok-ask-1" &&
      asked.model === "claude-fable-5-1" &&
      asked.launches === 0 && asked.closed === 0,
    JSON.stringify(asked),
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

  await page.close();
}
