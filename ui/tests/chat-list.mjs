import { openWindowTestPage } from "./window-boot.mjs";

/* The T3 Code chat view, stages 2 and 3 (2026-11-08).
 *
 *  - chat-list: with `chat_list_sidebar` on, the sidebar opens with the
 *    person's conversations — 기다림 · 작업 중 · 오늘 끝남 (folded) — a pinned
 *    one heading them, a put-away one gone with five seconds to take it back,
 *    a worker the ledger seated left to its coordinator; the destinations fold
 *    to one line of marks that keep their names; off, the sidebar it was.
 *  - new-chat: 「새 대화」 opens the start page — the box, the workspace, the
 *    agent with its catalog's models and its launch table's efforts — and
 *    Enter opens the conversation on the engine chosen: a pane launched with
 *    the first words, the model and the effort; or a GUI session started
 *    with them, the words going down its wire. The page closes behind it.
 *
 *   WINDOW_SUITES=chat-list,new-chat node ui/tests/window.mjs */

/* Four panes: one waiting for the person, one at work, one that ended today,
 * and a worker the ledger seated a task on. A page function. */
async function standConversations() {
  const stand = async (agent, state) => {
    const term = await openTermTab({ placement: "tab" });
    paneAgents.set(term, agent);
    hookStates.set(term, state);
    hookStates.observe(term, Date.now());
    // A minute ago, but never before today's midnight: a run that crosses it must still read 「오늘」.
    if (state === "done") hookStamps.set(term, Math.max(new Date().setHours(0, 0, 1, 0), Date.now() - 60_000));
    const tab = tabOfTerm(term);
    tab.paneTitles = { ...(tab.paneTitles ?? {}), [term]: `대화 ${agent} ${state}` };
    return term;
  };
  const waiting = await stand("claude", "needs-attention");
  const working = await stand("codex", "working");
  const ended = await stand("claude", "done");
  const worker = await stand("claude", "working");
  paneLedger.set(worker, { task: "워커 과업", taskId: "t-1" });
  window.__TERMS__ = { waiting, working, ended, worker };
  chatListSidebar = true;
  paintChatListSidebar();
  scheduleAgentPaint(["cards"]);
  await window.__PAINTED__();
  paintChatList();
}

function readList() {
  const host = document.getElementById("chat-list");
  const groups = [...host.querySelectorAll(".chat-group")].map((group) => ({
    name: group.dataset.group,
    open: group.querySelector(".chat-group-head").getAttribute("aria-expanded") === "true",
    count: group.querySelector(".chat-group-count").textContent,
    rows: [...group.querySelectorAll(".chat-row")].map((row) => ({
      term: Number(row.dataset.term),
      title: row.querySelector(".chat-row-title").textContent,
      word: row.querySelector(".chat-row-word").textContent,
      mono: row.querySelector(".chat-row-mono").textContent,
    })),
  }));
  return { shown: !host.hidden, mark: document.documentElement.dataset.chatList ?? "", groups };
}

export async function testChatList(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    const off = await page.evaluate(() => ({
      shown: !document.getElementById("chat-list").hidden,
      mark: document.documentElement.dataset.chatList ?? "",
      columns: getComputedStyle(document.querySelector(".nav-rows")).gridTemplateColumns.split(" ").length,
    }));
    ok("chat-list: off, the sidebar is the one it was — no list, the destinations two to a line",
      !off.shown && off.mark === "" && off.columns === 2, JSON.stringify(off));

    await page.evaluate(standConversations);
    const on = await page.evaluate(readList);
    const names = on.groups.map((group) => group.name);
    const terms = await page.evaluate(() => window.__TERMS__);
    const rowsOf = (name) => on.groups.find((group) => group.name === name)?.rows ?? [];
    ok("chat-list: on, the conversations head the column — waiting, at work, ended today (folded) — and the worker is not one",
      on.shown && on.mark === "on" && names.join(",") === "waiting,working,today" &&
        rowsOf("waiting").map((row) => row.term).join(",") === String(terms.waiting) &&
        rowsOf("working").map((row) => row.term).join(",") === String(terms.working) &&
        on.groups.find((group) => group.name === "today")?.open === false &&
        on.groups.find((group) => group.name === "today")?.count === "1" &&
        !on.groups.some((group) => group.rows.some((row) => row.term === terms.worker)),
      JSON.stringify({ on, terms }));
    ok("chat-list: a row says its project's letters, its name, and its wait or its clock",
      rowsOf("waiting")[0]?.word === "나를 기다림" && rowsOf("waiting")[0]?.title === "대화 claude needs-attention" &&
        /^[A-Z0-9]{1,2}$/.test(rowsOf("working")[0]?.mono ?? "") && /^(now|\d+[mhd])$/.test(rowsOf("working")[0]?.word ?? ""),
      JSON.stringify(on.groups));

    const fold = await page.evaluate(() => {
      const rows = [...document.querySelectorAll(".nav-rows > .nav-row:not([hidden])")];
      const tops = new Set(rows.map((row) => Math.round(row.getBoundingClientRect().top)));
      const named = rows.every((row) => {
        const words = row.querySelector(":scope > span:not(.nav-badge)");
        return words && words.textContent.trim() !== "" && words.getBoundingClientRect().width <= 1 && row.dataset.tip;
      });
      const search = document.getElementById("sidebar-search").getBoundingClientRect();
      const list = document.getElementById("chat-list").getBoundingClientRect();
      const strip = rows[0]?.getBoundingClientRect();
      return {
        rows: rows.length,
        oneLine: tops.size === 1,
        named,
        order: search.bottom <= list.top + 1 && list.bottom <= (strip?.top ?? 0) + 1,
        yourTurn: getComputedStyle(document.getElementById("your-turn-group")).display,
      };
    });
    ok("chat-list: the destinations fold under the list to one line of marks, each still named for a screen reader and the tooltip",
      fold.rows >= 6 && fold.oneLine && fold.named && fold.order && fold.yourTurn === "none", JSON.stringify(fold));

    const pinned = await page.evaluate(async () => {
      const row = document.querySelector('.chat-group[data-group="working"] .chat-row');
      row.querySelector(".chat-row-pin").click();
      await new Promise((done) => setTimeout(done, 20));
      const after = document.querySelector('.chat-group[data-group="pinned"] .chat-row');
      let kept = null;
      try {
        kept = JSON.parse(localStorage.getItem("zerocode.chatList") ?? "{}").pinned ?? null;
      } catch {
        kept = null;
      }
      return {
        first: document.querySelector(".chat-group")?.dataset.group,
        term: after?.dataset.term,
        pressed: after?.querySelector(".chat-row-pin").getAttribute("aria-pressed"),
        kept,
      };
    });
    ok("chat-list: a pinned conversation heads the list whatever its state, and the note outlives a reload",
      pinned.first === "pinned" && pinned.term === String(terms.working) && pinned.pressed === "true" &&
        Array.isArray(pinned.kept) && pinned.kept.length === 1, JSON.stringify(pinned));

    const away = await page.evaluate(async () => {
      const row = document.querySelector('.chat-group[data-group="waiting"] .chat-row');
      const term = row.dataset.term;
      row.querySelector(".chat-row-away").click();
      await new Promise((done) => setTimeout(done, 20));
      const gone = !document.querySelector(`.chat-row[data-term="${term}"]`);
      const toastNode = [...document.querySelectorAll(".toast")].find((note) => note.textContent.includes("목록에서 치웠습니다"));
      toastNode?.querySelector(".toast-action")?.click();
      await new Promise((done) => setTimeout(done, 20));
      const back = Boolean(document.querySelector(`.chat-group[data-group="waiting"] .chat-row[data-term="${term}"]`));
      // Put away again: it stays away while its state does not move, and comes back when it does.
      document.querySelector(`.chat-row[data-term="${term}"] .chat-row-away`).click();
      await new Promise((done) => setTimeout(done, 20));
      const stays = !document.querySelector(`.chat-row[data-term="${term}"]`);
      hookStates.set(Number(term), "working");
      paintChatList();
      const returns = Boolean(document.querySelector(`.chat-group[data-group="working"] .chat-row[data-term="${term}"]`));
      return { gone, toast: Boolean(toastNode), back, stays, returns };
    });
    ok("chat-list: a conversation put away leaves with five seconds to take it back, and comes back when its state moves",
      away.gone && away.toast && away.back && away.stays && away.returns, JSON.stringify(away));

    const opened = await page.evaluate(async () => {
      const term = window.__TERMS__.ended;
      document.querySelector('.chat-group[data-group="today"] .chat-group-head').click();
      await new Promise((done) => setTimeout(done, 20));
      const row = document.querySelector(`.chat-row[data-term="${term}"] .chat-row-open`);
      row?.click();
      await new Promise((done) => setTimeout(done, 50));
      return { found: Boolean(row), active: activeTabId, want: tabOfTerm(term)?.id };
    });
    ok("chat-list: 「오늘 끝남」 opens on a press, and a row brings its conversation's tab forward",
      opened.found && opened.active === opened.want, JSON.stringify(opened));

    const back = await page.evaluate(() => {
      chatListSidebar = false;
      paintChatListSidebar();
      return {
        shown: !document.getElementById("chat-list").hidden,
        columns: getComputedStyle(document.querySelector(".nav-rows")).gridTemplateColumns.split(" ").length,
        yourTurn: getComputedStyle(document.getElementById("your-turn-group")).display !== "none" ||
          document.getElementById("your-turn-group").hidden,
      };
    });
    ok("chat-list: turned off, the list goes and the destinations stand two to a line again",
      !back.shown && back.columns === 2 && back.yourTurn, JSON.stringify(back));
    ok("chat-list: no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

export async function testNewChat(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.evaluate(() => {
      window.__ANSWER__.agent_models = ({ agent }) => agent === "claude"
        ? [{ id: "opus", display_name: "Opus 5.5", provider: "claude" }, { id: "sonnet", display_name: "Sonnet 5.5", provider: "claude" }]
        : [{ id: "gpt-6", display_name: "GPT-6", provider: "openai" }];
      window.__ANSWER__.agent_effort_ladder = ({ agent }) => agent === "claude" ? ["low", "medium", "high", "xhigh", "max"] : ["low", "medium", "high"];
      chatListSidebar = true;
      paintChatListSidebar();
      document.getElementById("chat-list-new").click();
    });
    await page.waitForSelector(".newchat-view:not([hidden]) .newchat-box");
    await page.waitForFunction(() => document.querySelector(".newchat-view:not([hidden]) .newchat-model select")?.options.length > 1);
    const start = await page.evaluate(() => {
      const view = document.querySelector(".newchat-view:not([hidden])");
      const select = (name) => view.querySelector(`.newchat-${name} select`);
      return {
        tab: tabs.find((tab) => tab.kind === "newchat")?.id ?? null,
        label: tabLabel(tabs.find((tab) => tab.kind === "newchat")),
        heading: view.querySelector(".newchat-heading").textContent,
        focus: document.activeElement === view.querySelector(".newchat-box"),
        where: select("where").value === activeWorktreePath,
        agents: [...select("agent").options].map((option) => option.value),
        models: [...select("model").options].map((option) => option.value),
        efforts: [...select("effort").options].map((option) => option.value),
        engine: view.querySelector(".newchat-engine").textContent,
      };
    });
    ok("new-chat: 「새 대화」 opens the start page — the box in focus, the workspace in front, the agent's models and efforts",
      start.tab === "newchat" && start.label === "새 대화" && start.heading === "무엇을 할까요?" && start.focus && start.where &&
        start.agents.includes("claude") && start.models.join(",") === ",opus,sonnet" &&
        start.efforts.join(",") === ",low,medium,high,xhigh,max" && start.engine.startsWith("터미널 판으로"),
      JSON.stringify(start));

    const pane = await page.evaluate(async () => {
      const view = document.querySelector(".newchat-view:not([hidden])");
      const agent = view.querySelector(".newchat-agent select");
      agent.value = "claude";
      agent.dispatchEvent(new Event("change"));
      await new Promise((done) => setTimeout(done, 30));
      const model = view.querySelector(".newchat-model select");
      model.value = "opus";
      model.dispatchEvent(new Event("change"));
      const effort = view.querySelector(".newchat-effort select");
      effort.value = "high";
      effort.dispatchEvent(new Event("change"));
      const box = view.querySelector(".newchat-box");
      box.value = "로그인 화면의 오류를 고쳐 줘";
      box.dispatchEvent(new Event("input"));
      window.__LAUNCHED__ = null;
      box.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
      for (let wait = 0; wait < 40 && !window.__LAUNCHED__; wait += 1) await new Promise((done) => setTimeout(done, 25));
      await new Promise((done) => setTimeout(done, 50));
      return {
        launched: window.__LAUNCHED__,
        startOpen: tabs.some((tab) => tab.kind === "newchat"),
        front: tabs.find((tab) => tab.id === activeTabId)?.kind ?? null,
      };
    });
    ok("new-chat: Enter on the pane engine launches the agent with the first words, the model and the effort, and the page closes behind it",
      pane.launched?.agent === "claude" && pane.launched?.prompt === "로그인 화면의 오류를 고쳐 줘" &&
        pane.launched?.model === "opus" && pane.launched?.effort === "high" && !pane.startOpen && pane.front === "term",
      JSON.stringify(pane));

    const wire = await page.evaluate(async () => {
      window.__WIRE_START__ = null;
      window.__WIRE_SENT__ = [];
      window.__ANSWER__.wire_start = (args) => {
        window.__WIRE_START__ = args;
        return { id: 41, agent: args.agent, protocol: "claude-stream", version: "2.1.319", model: null, session: null };
      };
      window.__ANSWER__.wire_send = (args) => {
        window.__WIRE_SENT__.push(args);
        return null;
      };
      window.__ANSWER__.wire_log = () => ({
        found: true, skipped: false, next: 0, turns: [], status: "working", asks: [], live: [], agent: "claude",
        protocol: "claude-stream", models: [], modes: [], commands: [], tasks: [], steer: true, session: "1b2c3d4e-0000-4000-8000-000000000001",
      });
      newChatEngine = "wire";
      startNewConversation();
      await new Promise((done) => setTimeout(done, 50));
      const view = document.querySelector(".newchat-view:not([hidden])");
      const engine = view.querySelector(".newchat-engine").textContent;
      const agent = view.querySelector(".newchat-agent select");
      agent.value = "claude";
      agent.dispatchEvent(new Event("change"));
      await new Promise((done) => setTimeout(done, 30));
      const effort = view.querySelector(".newchat-effort select");
      effort.value = "max";
      effort.dispatchEvent(new Event("change"));
      const box = view.querySelector(".newchat-box");
      box.value = "테스트를 돌려 줘";
      box.dispatchEvent(new Event("input"));
      view.querySelector(".newchat-send").click();
      for (let wait = 0; wait < 60 && window.__WIRE_SENT__.length === 0; wait += 1) await new Promise((done) => setTimeout(done, 25));
      const tab = tabs.find((one) => one.kind === "worker" && one.worker?.wire === 41);
      return {
        engine,
        start: window.__WIRE_START__,
        sent: window.__WIRE_SENT__.map((one) => one.text ?? one.message ?? ""),
        tab: Boolean(tab),
        startOpen: tabs.some((one) => one.kind === "newchat"),
      };
    });
    ok("new-chat: on the GUI-session engine, Enter starts a wire with the effort and sends the first words down it",
      wire.engine.startsWith("GUI 세션으로") && wire.start?.agent === "claude" && wire.start?.effort === "max" &&
        wire.tab && !wire.startOpen && wire.sent.some((said) => String(said).includes("테스트를 돌려 줘")),
      JSON.stringify(wire));

    const session = await page.evaluate(async () => {
      const tab = tabs.find((one) => one.kind === "worker" && one.worker?.wire === 41);
      for (let wait = 0; wait < 40 && !tab?.worker.session; wait += 1) {
        await pollHelperPages?.();
        await new Promise((done) => setTimeout(done, 25));
      }
      return { session: tab?.worker.session ?? null };
    });
    ok("new-chat: a fresh wire learns its Claude session from its log, so its 「터미널」 door can continue it in a pane",
      session.session === "1b2c3d4e-0000-4000-8000-000000000001", JSON.stringify(session));

    const listed = await page.evaluate(() => {
      paintChatList();
      const row = [...document.querySelectorAll(".chat-row")].find((one) => one.querySelector(".chat-row-title")?.textContent === "테스트를 돌려 줘");
      return { row: Boolean(row), group: row?.closest(".chat-group")?.dataset.group ?? null };
    });
    ok("new-chat: the conversation on the wire stands in the sidebar's list with the others, at work",
      listed.row && listed.group === "working", JSON.stringify(listed));
    ok("new-chat: no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}
