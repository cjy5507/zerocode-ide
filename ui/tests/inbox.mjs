// 받은 편지함 (t-67483) — what waits on the person, from every project, on one page.
//
// Rust reads it (`board_inbox`: the ledger's desk and the panes, counted once). This
// suite stands in for that answer and checks the window: the sidebar's one door and its
// number, the page's tabs and their numbers, the list beside the preview, the answer given
// where the letter is read (the ledger's own `desk_reply`), a question closed without an
// answer (the ledger's own `question-close`, t-83281), a read that costs no frame, and the
// 100-letter paint. The numbers come from Rust's `counts`; this screen never recounts.
//
//   WINDOW_SUITES=inbox node ui/tests/window.mjs
//   INBOX_REPORT=1 prints the measured numbers (screens crossed, read delay, 100-letter paint).
const MINUTE = 60_000;
const PAINT_ROWS = 100;
const PAINT_RUNS = 9;
/* Q5 of the draft: three milliseconds for a hundred letters, a fifth of a frame. */
const PAINT_BUDGET_MS = 3;

/* A synthetic ledger letter (acme, example.com, /Users/dev). */
function letter(now, over = {}) {
  return {
    run: "run-a", id: "m-q1", kind: "question", from: "pane:team-a/%7", worker: "w-2043", task_id: "t-2041",
    task: "결제 화면 문구", body: "이 변경을 main에 바로 올려도 될까요?", reason: null, resets_at_ms: null,
    since_ms: null, category: null, routed: null, rung: null, switched_to: null, scope: null,
    created_ms: now - 4 * MINUTE, delivery: "delivered", delivery_id: "d-q", batch: 1, notices: 1, ...over,
  };
}

function item(tab, what, over = {}) {
  return { id: `${over.run ?? "run-a"}/${over.letter?.id ?? what}`, tab, what, run: "run-a", project: "acme-web", at_ms: 0, seat: true, ...over };
}

/* The page's example: three questions over two runs (one run this window holds no seat of),
 * two hand-ins, a stopped pane and a quota wall, three notices (two letters in one batch, and
 * a landing line). `counts` is what Rust says; it is deliberately not derived here. */
export function inboxFixture(now = Date.now()) {
  const q = (id, run, seat, ago, body, extra = {}) => item("question", "question", {
    id: `${run}/${id}`, run, seat, at_ms: now - ago * MINUTE, project: run === "run-a" ? "acme-web" : "acme-api",
    letter: letter(now, { run, id, created_ms: now - ago * MINUTE, body, ...extra }),
  });
  const hand = (id, title, ago, attempt, withdrawn = null) => item("handoff", "handoff", {
    id: `handoff:run-a/${id}`, at_ms: now - ago * MINUTE,
    task: {
      run: "run-a", id, title, stage: "reported", parent: null, parent_title: null, gate: null, blocked_by: [], closed: null,
      created_ms: now - ago * MINUTE, cost: null, writing: null, left: null,
      progress: { stage: "report_only", attempt, source: "a1b2c3d", ...(withdrawn === null ? {} : { withdrawn }) },
      times: { started_ms: now - 60 * MINUTE, handed_ms: now - ago * MINUTE },
      hand_in: { conditions: [{ number: 1, text: "시험 12개가 통과함", state: "checked" }], decisions: "오류 문구를 한 줄로", blocked: "", next: "" },
    },
  });
  const news = (tab, what, id, ago, extra = {}) => item(tab, what, {
    id: `run-a/${id}`, at_ms: now - ago * MINUTE,
    letter: letter(now, { id, kind: what, body: "", created_ms: now - ago * MINUTE, delivery_id: "d-n", batch: 2, worker: "w-2024", ...extra }),
  });
  const items = [
    q("m-q1", "run-a", true, 4, "이 변경을 main에 바로 올려도 될까요?"),
    q("m-q2", "run-a", true, 11, "느린 쿼리 두 개 중 어느 쪽을 먼저 고칠까요?", { delivery_id: "d-q2" }),
    q("m-q3", "run-b", false, 38, "알림 끄기 버튼을 설정 맨 위로 옮겨도 되나요?"),
    hand("t-2033", "로그인 오류 문구", 7, "dp-2033"),
    // The coordinator took back its verification of this one (t-81913).
    hand("t-2025", "홈 화면 빈 상태", 26, "dp-2025", "빈 상태 그림이 어두운 화면에서 안 보임"),
    item("blocked", "pane_blocked", {
      id: "pane:term:5@1", run: null, project: "wt-c", at_ms: now - 2 * MINUTE, seat: false,
      pane: { pane: "term:5", agent: "claude", worktree: "/Users/dev/acme-api/wt-c", waiting: "blocked", since_ms: now - 2 * MINUTE },
    }),
    news("blocked", "quota_walled", "m-wall", 19, { delivery: "pending", delivery_id: null, batch: null, resets_at_ms: now + 40 * MINUTE }),
    news("notice", "went_quiet", "m-n1", 5, { reason: "stalled" }),
    news("notice", "model_deviated", "m-n3", 15, { switched_to: "claude-sonnet-5-5", scope: "session", category: "cyber" }),
    item("notice", "landing", { id: "landing:run-a:2", at_ms: 0, landing: 2 }),
  ];
  const counts = { all: 10, question: 3, handoff: 2, blocked: 2, notice: 3, waiting: 7 };
  return { items, counts };
}

export async function testInbox(browser, origin, standBackend, ok) {
  const page = await browser.newPage();
  await standBackend(page);
  await page.addInitScript(() => {
    const base = window.__TAURI__.core.invoke;
    window.__INBOX__ = { snapshot: { items: [], counts: { all: 0, question: 0, handoff: 0, blocked: 0, notice: 0, waiting: 0 } }, calls: [], fail: new Set(), already: new Set(), asks: 0 };
    window.__TAURI__.core.invoke = (name, args) => {
      const state = window.__INBOX__;
      if (name === "board_inbox") {
        state.asks += 1;
        return Promise.resolve(JSON.parse(JSON.stringify(state.snapshot)));
      }
      if (["desk_reply", "desk_ack", "desk_review_pass", "desk_question_close", "clear_finish_mark"].includes(name)) {
        state.calls.push({ name, args: JSON.parse(JSON.stringify(args ?? {})) });
        const key = `${name}:${args?.message ?? args?.delivery ?? args?.task ?? ""}`;
        if (state.fail.has(key)) {
          state.fail.delete(key);
          return Promise.reject("the ledger refused");
        }
        if (name === "desk_question_close") {
          // The verb's own answer: the close's record, or the standing one with `already`.
          const already = state.already.has(args.message);
          return Promise.resolve({
            questionId: args.message, runId: args.run, closed: true, already,
            closeId: `m-close-${args.message}`, closedBy: `run:${args.run}`, ...(already ? {} : { note: args.reason }),
          });
        }
        return Promise.resolve(name === "clear_finish_mark" ? false : { receipt: "r-1" });
      }
      return base(name, args);
    };
  });
  await page.goto(`${origin}/index.html`);
  await page.waitForFunction(() => typeof BOUND !== "undefined" && BOUND.size > 0);

  const fixture = inboxFixture();
  const setSnapshot = (snapshot) =>
    page.evaluate(async (snapshot) => {
      window.__INBOX__.snapshot = snapshot;
      await refreshInbox();
    }, snapshot);
  await setSnapshot(fixture);

  // The sidebar keeps one line for it, and the two lists it absorbs are gone.
  const door = await page.evaluate(() => {
    const row = document.getElementById("nav-inbox");
    return {
      row: Boolean(row),
      inNav: row?.parentElement?.classList.contains("nav-rows") === true,
      badge: row?.querySelector(".nav-badge")?.textContent ?? null,
      badgeShown: row ? !row.querySelector(".nav-badge").hidden : false,
      oldList: Boolean(document.getElementById("waiting-list")),
      oldRow: Boolean(document.getElementById("nav-attention")),
    };
  });
  ok("the sidebar has one 받은 편지함 row in the shortcut rows", door.row && door.inNav, JSON.stringify(door));
  ok("its number is what Rust counts as waiting (7), not a recount", door.badge === "7" && door.badgeShown, JSON.stringify(door));
  ok("「나를 기다림」 and 「주의 필요」 left the sidebar", !door.oldList && !door.oldRow, JSON.stringify(door));

  // One screen from the start to the answer sent (the old road crossed three: sidebar card,
  // task board, the answer box). A screen is the active tab; its changes are counted.
  const screens = await page.evaluate(() => {
    const seen = [];
    const note = () => {
      if (seen.at(-1) !== activeTabId) seen.push(activeTabId);
    };
    note();
    document.getElementById("nav-inbox").click();
    note();
    return seen;
  });
  const opened = await page.waitForSelector(".inbox-view:not([hidden]) .inbox-list .inbox-row", { timeout: 5000 }).then(() => true, () => false);
  ok("the door opens the page with the list in it", opened);
  const sendOne = await page.evaluate(() => {
    const view = document.querySelector(".inbox-view:not([hidden])");
    const seen = [activeTabId];
    const note = () => {
      if (seen.at(-1) !== activeTabId) seen.push(activeTabId);
    };
    view.querySelector('.inbox-tab[data-inbox-tab="question"]').click();
    note();
    view.querySelector('.inbox-row[data-id="run-a/m-q1"]').click();
    note();
    const field = view.querySelector(".inbox-reply-field");
    field.value = "검토 뒤에 올려 주세요.";
    field.dispatchEvent(new Event("input", { bubbles: true }));
    view.querySelector('[data-inbox-act="reply"]').click();
    note();
    return seen;
  });
  const crossed = screens.length - 1 + (sendOne.length - 1);
  ok("a question is answered on one screen (3 before)", crossed === 1, `screens=${JSON.stringify([...screens, ...sendOne])}`);
  const replied = await page.evaluate(() => window.__INBOX__.calls.filter((call) => call.name === "desk_reply"));
  ok(
    "the answer goes by the ledger's own reply, with a retry name",
    replied.length === 1 && replied[0].args.run === "run-a" && replied[0].args.message === "m-q1"
      && replied[0].args.body === "검토 뒤에 올려 주세요." && /^ui-inbox-reply-/.test(replied[0].args.retryRequest ?? ""),
    JSON.stringify(replied),
  );

  // The tabs say Rust's numbers, even where the rows in hand are fewer (the desk sends a window).
  await setSnapshot({ ...inboxFixture(), counts: { all: 40, question: 7, handoff: 2, blocked: 2, notice: 3, waiting: 11 } });
  const said = await page.evaluate(() => ({
    tabs: Object.fromEntries([...document.querySelectorAll(".inbox-view:not([hidden]) .inbox-tab")].map((tab) => [tab.dataset.inboxTab, tab.querySelector(".inbox-tab-count").textContent])),
    badge: document.querySelector("#nav-inbox .nav-badge").textContent,
  }));
  ok(
    "the tabs and the sidebar draw Rust's counts and never count the rows themselves",
    said.tabs.all === "40" && said.tabs.question === "7" && said.tabs.notice === "3" && said.badge === "11",
    JSON.stringify(said),
  );
  await setSnapshot(inboxFixture());

  // Tabs narrow the list; blocked rows are marked by a shape and a word, not colour alone.
  const byTab = await page.evaluate(() => {
    const view = document.querySelector(".inbox-view:not([hidden])");
    const out = {};
    for (const tab of ["all", "question", "handoff", "blocked", "notice"]) {
      view.querySelector(`.inbox-tab[data-inbox-tab="${tab}"]`).click();
      out[tab] = [...view.querySelectorAll(".inbox-row")].map((row) => row.dataset.kind);
    }
    return out;
  });
  ok(
    "the tabs narrow the list to their own kind, and 전체 keeps the page's order",
    byTab.all.length === 10 && byTab.question.every((kind) => kind === "question") && byTab.question.length === 3
      && byTab.handoff.length === 2 && byTab.blocked.length === 2 && byTab.notice.length === 3
      && byTab.all.slice(0, 3).every((kind) => kind === "question"),
    JSON.stringify(byTab),
  );
  const blockedMark = await page.evaluate(() => {
    const view = document.querySelector(".inbox-view:not([hidden])");
    view.querySelector('.inbox-tab[data-inbox-tab="blocked"]').click();
    return [...view.querySelectorAll(".inbox-row")].map((row) => ({
      word: row.querySelector(".inbox-kind").textContent,
      shape: Boolean(row.querySelector(".inbox-kind svg, .inbox-kind[data-shape]")),
    }));
  });
  ok("a blocked row says 막힘 in words and in a shape", blockedMark.every((mark) => mark.word === "막힘" && mark.shape), JSON.stringify(blockedMark));

  // A blocked pane gets one door: open the pane. Allow and deny are never pressed from here.
  const blockedPane = await page.evaluate(() => {
    const view = document.querySelector(".inbox-view:not([hidden])");
    view.querySelector('.inbox-row[data-id="pane:term:5@1"]').click();
    const shown = [...view.querySelectorAll(".inbox-preview [data-inbox-act]")].filter((button) => button.checkVisibility());
    const acts = shown.map((button) => button.dataset.inboxAct);
    return { acts, allowDeny: /허용|거절|allow|deny/i.test(shown.map((button) => button.textContent).join(" ")) };
  });
  ok(
    "a stopped pane has only 판 열기 — no allow or deny from the page",
    JSON.stringify(blockedPane.acts) === JSON.stringify(["open-pane"]) && !blockedPane.allowDeny,
    JSON.stringify(blockedPane),
  );

  // A hand-in is passed by the ledger's own pass, naming the attempt and the source the page read.
  const passed = await page.evaluate(() => {
    const view = document.querySelector(".inbox-view:not([hidden])");
    view.querySelector('.inbox-tab[data-inbox-tab="handoff"]').click();
    view.querySelector('.inbox-row[data-id="handoff:run-a/t-2033"]').click();
    view.querySelector('[data-inbox-act="pass"]').click();
    return window.__INBOX__.calls.filter((call) => call.name === "desk_review_pass");
  });
  ok(
    "확인 on a hand-in passes it with its attempt and source",
    passed.length === 1 && passed[0].args.run === "run-a" && passed[0].args.task === "t-2033"
      && passed[0].args.attempt === "dp-2033" && passed[0].args.source === "a1b2c3d",
    JSON.stringify(passed),
  );

  // A hand-in whose verification the coordinator withdrew takes no pass here: the page says it
  // was withdrawn and why, and that a new review or a new hand-in is what passes it (t-81913).
  const withdrawn = await page.evaluate(() => {
    const view = document.querySelector(".inbox-view:not([hidden])");
    window.__INBOX__.calls.length = 0;
    view.querySelector('.inbox-tab[data-inbox-tab="handoff"]').click();
    view.querySelector('.inbox-row[data-id="handoff:run-a/t-2025"]').click();
    const pass = view.querySelector('[data-inbox-act="pass"]');
    const shown = pass.checkVisibility();
    pass.click();
    return {
      title: view.querySelector(".inbox-pv-title").textContent,
      note: view.querySelector(".inbox-preview .inbox-note")?.textContent ?? "",
      shown,
      sent: window.__INBOX__.calls.filter((call) => call.name === "desk_review_pass").length,
    };
  });
  ok(
    "a withdrawn hand-in shows the withdrawal and why in place of 확인, and a press sends nothing",
    !withdrawn.shown && withdrawn.sent === 0
      && withdrawn.title === "홈 화면 빈 상태 검증이 철회됐습니다 — 빈 상태 그림이 어두운 화면에서 안 보임"
      && withdrawn.note === "새 검수 결과를 적거나 새로 넘겨야 다시 통과됩니다.",
    JSON.stringify(withdrawn),
  );

  // A question of a run this window holds no seat of cannot be answered here, and says so.
  const noSeat = await page.evaluate(() => {
    const view = document.querySelector(".inbox-view:not([hidden])");
    view.querySelector('.inbox-tab[data-inbox-tab="question"]').click();
    view.querySelector('.inbox-row[data-id="run-b/m-q3"]').click();
    return {
      field: view.querySelector(".inbox-reply-field").checkVisibility(),
      reply: view.querySelector('[data-inbox-act="reply"]').checkVisibility(),
      closeField: view.querySelector(".inbox-close-field")?.checkVisibility() ?? false,
      close: view.querySelector('[data-inbox-act="close"]')?.checkVisibility() ?? false,
      note: view.querySelector(".inbox-preview .inbox-note")?.textContent ?? "",
    };
  });
  ok("without the run's seat there is no answer box, only the reason", !noSeat.field && !noSeat.reply && noSeat.note.length > 0, JSON.stringify(noSeat));
  ok("without the run's seat there is no 답 없이 닫기 either", !noSeat.closeField && !noSeat.close, JSON.stringify(noSeat));

  // A failed answer keeps its retry name, so pressing again replays one receipt.
  const retried = await page.evaluate(async () => {
    window.__INBOX__.fail.add("desk_reply:m-q2");
    window.__INBOX__.calls.length = 0;
    const view = document.querySelector(".inbox-view:not([hidden])");
    view.querySelector('.inbox-row[data-id="run-a/m-q2"]').click();
    const field = view.querySelector(".inbox-reply-field");
    field.value = "색인부터 고치세요.";
    field.dispatchEvent(new Event("input", { bubbles: true }));
    view.querySelector('[data-inbox-act="reply"]').click();
    await new Promise((resolve) => setTimeout(resolve, 50));
    view.querySelector('[data-inbox-act="reply"]').click();
    await new Promise((resolve) => setTimeout(resolve, 50));
    return window.__INBOX__.calls.filter((call) => call.name === "desk_reply").map((call) => call.args.retryRequest);
  });
  ok("a refused answer is retried under the same name", retried.length === 2 && retried[0] === retried[1], JSON.stringify(retried));

  // A question nobody will answer closes without an answer (t-83281): one line of why, one call
  // to the ledger's own `question-close` as the run's seat, and the row reads 「닫힘 · 까닭」 until
  // the ledger's next reading leaves it out of the list and out of Rust's numbers.
  const withOpen = (() => {
    const base = inboxFixture();
    const now = Date.now();
    const open = (id, ago, body) => ({
      ...base.items[0], id: `run-a/${id}`, at_ms: now - ago * MINUTE,
      letter: { ...base.items[0].letter, id, body, created_ms: now - ago * MINUTE, delivery_id: `d-${id}` },
    });
    return {
      items: [open("m-q4", 1, "예전 결제 화면도 같이 고칠까요?"), open("m-q5", 2, "로그 파일을 지워도 될까요?"), ...base.items],
      counts: { ...base.counts, all: base.counts.all + 2, question: base.counts.question + 2, waiting: base.counts.waiting + 2 },
    };
  })();
  await setSnapshot(withOpen);
  const closing = await page.evaluate(async () => {
    const view = document.querySelector(".inbox-view:not([hidden])");
    window.__INBOX__.calls.length = 0;
    view.querySelector('.inbox-tab[data-inbox-tab="question"]').click();
    view.querySelector('.inbox-row[data-id="run-a/m-q4"]').click();
    const field = () => view.querySelector(".inbox-close-field");
    const button = () => view.querySelector('[data-inbox-act="close"]');
    const shown = (node) => node?.checkVisibility() ?? false;
    const offered = shown(field()) && shown(button()) && button().textContent === "답 없이 닫기";
    // Without a reason nothing leaves: the page asks for the one line instead.
    button()?.click();
    await new Promise((resolve) => setTimeout(resolve, 20));
    const unreasoned = {
      sent: window.__INBOX__.calls.length,
      error: view.querySelector(".inbox-close .inbox-error")?.textContent ?? "",
    };
    if (field()) {
      field().value = "이미 다른 과업에서 끝난 일";
      field().dispatchEvent(new Event("input", { bubbles: true }));
    }
    button()?.click();
    await new Promise((resolve) => setTimeout(resolve, 50));
    const row = view.querySelector('.inbox-row[data-id="run-a/m-q4"]');
    return {
      offered, unreasoned,
      calls: window.__INBOX__.calls.filter((call) => call.name === "desk_question_close").map((call) => call.args),
      rowTitle: row?.querySelector(".inbox-row-title").textContent ?? null,
      title: view.querySelector(".inbox-pv-title").textContent,
      left: [shown(field()), shown(button()), shown(view.querySelector(".inbox-reply-field")), shown(view.querySelector('[data-inbox-act="reply"]'))],
      note: view.querySelector(".inbox-preview .inbox-note")?.textContent ?? "",
    };
  });
  ok("a question in a run this window sits for offers 답 없이 닫기 beside one line of why", closing.offered, JSON.stringify(closing));
  ok("a close without a reason sends nothing and says the one line is needed", closing.unreasoned.sent === 0 && closing.unreasoned.error.length > 0, JSON.stringify(closing.unreasoned));
  ok(
    "답 없이 닫기 calls the ledger's question-close once, with the run, the question, the reason and a retry name",
    closing.calls.length === 1 && closing.calls[0].run === "run-a" && closing.calls[0].message === "m-q4"
      && closing.calls[0].reason === "이미 다른 과업에서 끝난 일" && /^ui-inbox-close-/.test(closing.calls[0].retryRequest ?? ""),
    JSON.stringify(closing.calls),
  );
  ok(
    "the closed question reads 「닫힘 · 까닭」 in its row and its preview, with nothing left to answer or close",
    closing.rowTitle === "닫힘 · 이미 다른 과업에서 끝난 일" && closing.title === closing.rowTitle
      && closing.left.every((one) => one === false) && closing.note === "답 없이 닫음 · 원장에 적히면 목록에서 빠져요",
    JSON.stringify(closing),
  );

  // A close the ledger refused keeps its name, so pressing again replays one receipt; a question the
  // ledger already holds closed answers `already` and reads the same, with no error left.
  const again = await page.evaluate(async () => {
    const view = document.querySelector(".inbox-view:not([hidden])");
    window.__INBOX__.fail.add("desk_question_close:m-q5");
    window.__INBOX__.already.add("m-q5");
    view.querySelector('.inbox-row[data-id="run-a/m-q5"]').click();
    const field = view.querySelector(".inbox-close-field");
    if (field) {
      field.value = "질문한 판이 사라짐";
      field.dispatchEvent(new Event("input", { bubbles: true }));
    }
    view.querySelector('[data-inbox-act="close"]')?.click();
    await new Promise((resolve) => setTimeout(resolve, 50));
    const refused = view.querySelector(".inbox-close .inbox-error")?.textContent ?? "";
    view.querySelector('[data-inbox-act="close"]')?.click();
    await new Promise((resolve) => setTimeout(resolve, 50));
    return {
      refused,
      names: window.__INBOX__.calls.filter((call) => call.name === "desk_question_close" && call.args.message === "m-q5").map((call) => call.args.retryRequest),
      rowTitle: view.querySelector('.inbox-row[data-id="run-a/m-q5"] .inbox-row-title')?.textContent ?? null,
      error: view.querySelector(".inbox-close .inbox-error")?.checkVisibility() ?? false,
    };
  });
  ok("a refused close says so and is retried under the same name", again.refused.length > 0 && again.names.length === 2 && again.names[0] === again.names[1], JSON.stringify(again));
  ok("a question the ledger already holds closed reads 「닫힘 · 까닭」 the same, with no error", again.rowTitle === "닫힘 · 질문한 판이 사라짐" && !again.error, JSON.stringify(again));

  // The ledger's next reading leaves both out; the numbers are Rust's again, one fewer each.
  await setSnapshot(inboxFixture());
  const closedOut = await page.evaluate(() => {
    const view = document.querySelector(".inbox-view:not([hidden])");
    return {
      rows: ["run-a/m-q4", "run-a/m-q5"].filter((id) => view.querySelector(`.inbox-row[data-id="${id}"]`)),
      count: view.querySelector('.inbox-tab[data-inbox-tab="question"] .inbox-tab-count').textContent,
      badge: document.querySelector("#nav-inbox .nav-badge").textContent,
    };
  });
  ok("closed questions leave the list and the open numbers when the ledger reads them out", closedOut.rows.length === 0 && closedOut.count === "3" && closedOut.badge === "7", JSON.stringify(closedOut));

  // An agent pressing in the window (t-74825): the page answers a look with the marks it draws, and the
  // backend refuses a control whose marks say it is the person's (core `judge`). This stands in for that
  // refusal with the one mark under test, then presses what is left through the page's own road. A row
  // that opens a letter reads it, and a read leaves the ledger's ack — so no row may be an agent's to press.
  await setSnapshot(inboxFixture());
  const agent = await page.evaluate(() => {
    const view = document.querySelector(".inbox-view:not([hidden])");
    view.querySelector('.inbox-tab[data-inbox-tab="notice"]').click();
    window.__INBOX__.calls.length = 0;
    const look = selfActAnswer({ kind: "look" });
    const pressed = [];
    let refused = 0;
    let rowControls = 0;
    for (const control of look.controls) {
      const node = selfActLooks.get(look.lookId)[control.id];
      if (!node.classList.contains("inbox-row")) continue;
      rowControls += 1;
      const said = selfActAnswer({ kind: "facts", lookId: look.lookId, id: control.id });
      if (said.facts.personOnly || said.facts.asking || said.facts.secret || said.facts.terminal || said.facts.setting) {
        refused += 1;
        continue;
      }
      pressed.push(selfActAnswer({ kind: "do", lookId: look.lookId, id: control.id, act: "click", facts: said.facts }).done);
    }
    const dots = [...view.querySelectorAll(".inbox-row")].map((row) => row.dataset.unread);
    return {
      rowControls, refused, pressed: pressed.length,
      acks: window.__INBOX__.calls.filter((call) => call.name === "desk_ack").length,
      calls: window.__INBOX__.calls.length, dots,
    };
  });
  ok("a look of the inbox shows its rows as controls (3 in the notice tab)", agent.rowControls === 3, JSON.stringify(agent));
  ok("every row of the inbox is the person's: an agent's look finds none it may press", agent.refused === agent.rowControls && agent.pressed === 0, JSON.stringify(agent));
  ok("an agent pressing the inbox sends no desk_ack and reads no letter", agent.acks === 0 && agent.calls === 0 && agent.dots.every((dot) => dot === "true"), JSON.stringify(agent));

  // Reading is immediate: the dot is off in the frame the letter opens, and the ack leaves in
  // the same task — but only when every letter of its batch is read (the ledger acks a batch).
  await setSnapshot(inboxFixture());
  const reading = await page.evaluate(() => {
    const view = document.querySelector(".inbox-view:not([hidden])");
    view.querySelector('.inbox-tab[data-inbox-tab="notice"]').click();
    window.__INBOX__.calls.length = 0;
    const row = (id) => view.querySelector(`.inbox-row[data-id="${id}"]`);
    const before = row("run-a/m-n1").dataset.unread;
    row("run-a/m-n1").click();
    const afterFirst = { unread: row("run-a/m-n1").dataset.unread, acks: window.__INBOX__.calls.filter((call) => call.name === "desk_ack").length };
    row("run-a/m-n3").click();
    const acks = window.__INBOX__.calls.filter((call) => call.name === "desk_ack");
    return { before, afterFirst, secondUnread: row("run-a/m-n3").dataset.unread, acks: acks.map((call) => call.args) };
  });
  ok("opening a letter turns its unread dot off in the same frame", reading.before === "true" && reading.afterFirst.unread === "false" && reading.secondUnread === "false", JSON.stringify(reading));
  ok(
    "the ack leaves with the last read letter of its batch, once, by the ledger's batch id",
    reading.afterFirst.acks === 0 && reading.acks.length === 1 && reading.acks[0].delivery === "d-n" && reading.acks[0].run === "run-a",
    JSON.stringify(reading),
  );

  // 안 읽음만 hides what was read.
  const unreadOnly = await page.evaluate(() => {
    const view = document.querySelector(".inbox-view:not([hidden])");
    view.querySelector('.inbox-tab[data-inbox-tab="notice"]').click();
    const all = view.querySelectorAll(".inbox-row").length;
    view.querySelector(".inbox-unread-switch").click();
    const shown = [...view.querySelectorAll(".inbox-row")].map((row) => row.dataset.unread);
    const on = view.querySelector(".inbox-unread-switch").getAttribute("aria-checked");
    view.querySelector(".inbox-unread-switch").click();
    return { all, shown, on };
  });
  ok("「안 읽음만」 keeps only the unread letters", unreadOnly.on === "true" && unreadOnly.shown.every((one) => one === "true") && unreadOnly.shown.length < unreadOnly.all, JSON.stringify(unreadOnly));

  // The 「주의 필요」 chord opens the page on its 막힘 tab (draft Q2: absorbed, not kept beside it).
  const chord = await page.evaluate(() => {
    document.querySelector('.inbox-view:not([hidden]) .inbox-tab[data-inbox-tab="all"]').click();
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "A", ctrlKey: true, shiftKey: true, bubbles: true, cancelable: true }));
    const pressed = document.querySelector('.inbox-view:not([hidden]) .inbox-tab[aria-pressed="true"]');
    return { tab: pressed?.dataset.inboxTab ?? null, modal: !document.getElementById("action-hub-scrim")?.hidden };
  });
  ok("the 주의 필요 chord opens the inbox on 막힘, not the old modal", chord.tab === "blocked" && chord.modal === false, JSON.stringify(chord));

  // 100 letters, median of nine. Gated: the worst case, where every letter's words changed
  // (a minute turned, or the ledger moved them all). Reported: the steady repaint, where the
  // keyed rows are reused as they stand.
  const timings = await page.evaluate(({ rows, runs }) => {
    const base = inboxSnapshot.items;
    const bulk = Array.from({ length: rows }, (_, index) => {
      const source = base[index % base.length];
      return { ...source, id: `${source.id}#${index}` };
    });
    inboxSnapshot = { items: bulk, counts: { all: rows, question: rows, handoff: 0, blocked: 0, notice: 0, waiting: rows } };
    document.querySelector('.inbox-view:not([hidden]) .inbox-tab[data-inbox-tab="all"]').click();
    const measure = (before) => {
      const samples = [];
      for (let run = 0; run < runs; run += 1) {
        before(run);
        const started = performance.now();
        paintInboxViews();
        samples.push(performance.now() - started);
      }
      return samples.sort((left, right) => left - right);
    };
    const changed = measure((run) => {
      for (const item of bulk) item.at_ms += 1 + run;
    });
    const steady = measure(() => {});
    return { changed, steady };
  }, { rows: PAINT_ROWS, runs: PAINT_RUNS });
  const middle = (samples) => samples[Math.floor(samples.length / 2)];
  const median = middle(timings.changed);
  ok(`${PAINT_ROWS} letters, every word rewritten, paint in ${PAINT_BUDGET_MS} ms or less (median of ${PAINT_RUNS})`, median <= PAINT_BUDGET_MS, `median=${median.toFixed(2)}ms`);
  if (process.env.INBOX_REPORT === "1") {
    console.log(
      `INBOX_REPORT screens=${crossed} paint_rows=${PAINT_ROWS} rewritten_ms_median=${median.toFixed(2)} rewritten_ms_max=${timings.changed.at(-1).toFixed(2)} steady_ms_median=${middle(timings.steady).toFixed(2)}`,
    );
  }
  await page.close();
}
