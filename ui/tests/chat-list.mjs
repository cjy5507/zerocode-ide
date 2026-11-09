import { openWindowTestPage } from "./window-boot.mjs";

/* The T3 Code chat view, stages 2 and 3 (2026-11-08).
 *
 *  - chat-list: with `chat_list_sidebar` on, the sidebar opens with the
 *    person's conversations — 기다림 · 작업 중 · 오늘 끝남 (folded) — a pinned
 *    one heading them, a put-away one gone with five seconds to take it back,
 *    a worker the ledger seated left to its coordinator; the destinations fold
 *    to one line of marks that keep their names; off, the sidebar it was.
 *  - new-chat: 「새 대화」 opens the start page — the conversation's own
 *    composer on the conversation's ground (2026-11-09, the person's
 *    「CC·Codex처럼, Geist로」): the box in the window's type, the agent·model
 *    and effort chips with their menus, the `+` of the attachments; under it
 *    the workspace and engine chips on one line at any width; the
 *    conversations going, waiting ones first. Enter opens the conversation on
 *    the engine chosen: a pane launched with the first words (and the paths
 *    attached), the model and the effort; or a GUI session started with them,
 *    the words going down its wire. The page closes behind it.
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
      face: [...row.querySelector(".chat-row-mono").childNodes].map((child) => child.className ?? "#text").join(","),
      sub: row.querySelector(".chat-row-sub").textContent,
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
    ok("chat-list: a row wears its agent's mark where the project's letters stood, says its name, its project and branch, and its wait or its clock",
      rowsOf("waiting")[0]?.word === "나를 기다림" && rowsOf("waiting")[0]?.title === "대화 claude needs-attention" &&
        /^agent-ico\b/.test(rowsOf("working")[0]?.face ?? "") && !(rowsOf("working")[0]?.face ?? "").includes(",") &&
        rowsOf("working")[0]?.sub === "zerocode · main" && /^(now|\d+[mhd])$/.test(rowsOf("working")[0]?.word ?? ""),
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

    const fill = await page.evaluate(async () => {
      const settle = () => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      const root = document.documentElement;
      const was = root.style.getPropertyValue("--sidebar-width");
      const out = {};
      for (const width of [200, 280, 400]) {
        root.style.setProperty("--sidebar-width", `${width}px`);
        await settle();
        const nav = document.querySelector(".nav-rows");
        const end = nav.getBoundingClientRect().right - parseFloat(getComputedStyle(nav).paddingRight);
        const lines = new Map();
        for (const mark of nav.querySelectorAll(":scope > .nav-row:not([hidden])")) {
          const box = mark.getBoundingClientRect();
          const top = Math.round(box.top);
          lines.set(top, [...(lines.get(top) ?? []), box.right]);
        }
        out[width] = [...lines.values()].map((rights) => ({ marks: rights.length, empty: Math.round(end - Math.max(...rights)) }));
      }
      root.style.setProperty("--sidebar-width", was);
      await settle();
      return out;
    });
    ok("chat-list: the line of marks fills its column at any width — no empty column after the last mark — and a column too narrow for one line splits them evenly over two",
      Object.values(fill).every((lines) => lines[0].empty <= 1 && lines.every((line) => lines[0].marks - line.marks <= 1)) &&
        fill[280].length === 1 && fill[400].length === 1 && fill[200].length <= 2,
      JSON.stringify(fill));

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

/* Page hands for the start page's chips: open a chip's menu, wait for it to
 * stand, press the row that says `words`. A page function. */
function installNewChatHands() {
  const settle = (ms) => new Promise((done) => setTimeout(done, ms));
  window.__NEW_CHAT_VIEW__ = () => document.querySelector(".newchat-view:not([hidden])");
  window.__NEW_CHAT_MENU__ = async (chip) => {
    window.__NEW_CHAT_VIEW__().querySelector(chip).click();
    let menu = null;
    for (let wait = 0; wait < 40 && !(menu = document.querySelector(".composer-menu")); wait += 1) await settle(25);
    return menu;
  };
  window.__NEW_CHAT_PICK__ = async (chip, words) => {
    const menu = await window.__NEW_CHAT_MENU__(chip);
    const row = [...(menu?.querySelectorAll(".composer-menu-item") ?? [])]
      .find((item) => item.querySelector(".composer-menu-name")?.textContent === words);
    row?.click();
    await settle(30);
    return Boolean(row);
  };
  window.__NEW_CHAT_WORDS__ = (chip) => window.__NEW_CHAT_VIEW__().querySelector(`${chip} .worker-composer-pill-words`)?.textContent ?? "";
}

/* Whether a row of controls stands on one line: every shown child's top the same. */
function newChatOneLine(selector) {
  const row = document.querySelector(`.newchat-view:not([hidden]) ${selector}`);
  const tops = [...row.children]
    .filter((node) => !node.hidden && !node.classList.contains("newchat-under-gap") && node.getClientRects().length > 0)
    .map((node) => Math.round(node.getBoundingClientRect().top + node.getBoundingClientRect().height / 2));
  return tops.length > 0 && new Set(tops).size === 1;
}

export async function testNewChat(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.evaluate(standConversations);
    await page.evaluate(installNewChatHands);
    await page.evaluate(() => {
      window.__ANSWER__.agent_models = ({ agent }) => agent === "claude"
        ? [{ id: "opus", display_name: "Opus 5.5", provider: "claude" }, { id: "sonnet", display_name: "Sonnet 5.5", provider: "claude" }]
        : [{ id: "gpt-6", display_name: "GPT-6", provider: "openai" }];
      window.__ANSWER__.agent_effort_ladder = ({ agent }) => agent === "claude" ? ["low", "medium", "high", "xhigh", "max"] : ["low", "medium", "high"];
      window.__ANSWER__.path_kinds = ({ paths }) => paths.map(() => "file");
      window.__ENGINE_SET__ = [];
      window.__ANSWER__.set_new_chat_engine = (args) => {
        window.__ENGINE_SET__.push(args.engine);
        return null;
      };
      document.getElementById("chat-list-new").click();
    });
    await page.waitForSelector(".newchat-view:not([hidden]) .newchat-box");
    await page.waitForFunction(() => !document.querySelector(".newchat-view:not([hidden]) .newchat-effort").hidden);
    const start = await page.evaluate(() => {
      const view = window.__NEW_CHAT_VIEW__();
      const box = view.querySelector(".newchat-box");
      const probe = document.createElement("div");
      probe.style.background = "var(--chat-ground)";
      view.append(probe);
      const ground = getComputedStyle(probe).backgroundColor;
      probe.remove();
      return {
        tab: tabs.find((tab) => tab.kind === "newchat")?.id ?? null,
        label: tabLabel(tabs.find((tab) => tab.kind === "newchat")),
        heading: view.querySelector(".newchat-heading").textContent,
        focus: document.activeElement === box,
        ground: getComputedStyle(view).backgroundColor === ground,
        font: getComputedStyle(box).fontFamily,
        selects: view.querySelectorAll("select").length,
        plus: Boolean(view.querySelector(".worker-composer-tools > .composer-attach-plus")),
        agent: window.__NEW_CHAT_WORDS__(".newchat-agent"),
        effort: window.__NEW_CHAT_WORDS__(".newchat-effort"),
        where: window.__NEW_CHAT_WORDS__(".newchat-where"),
        whereWants: worktreeDisplayName(worktreeAt(activeWorktreePath)),
        engine: window.__NEW_CHAT_WORDS__(".newchat-engine"),
        sendOff: view.querySelector(".newchat-send").disabled,
        recent: [...view.querySelectorAll(".newchat-recent-row")].map((row) => row.dataset.state),
        recentHead: view.querySelector(".newchat-recent-head").textContent,
      };
    });
    ok("new-chat: 「새 대화」 opens the start page — the box in focus on the conversation's ground, in the window's type, no select left in it",
      start.tab === "newchat" && start.label === "새 대화" && start.heading === "무엇을 할까요?" && start.focus &&
        start.ground && /^"?Geist/.test(start.font) && start.selects === 0, JSON.stringify(start));
    ok("new-chat: the composer's chips say their values — the agent, the effort's default, the workspace, the engine — and the `+` stands first",
      start.plus && start.agent.length > 0 && start.effort === "기본" && start.where.endsWith(start.whereWants) &&
        start.engine === "터미널 판" && start.sendOff, JSON.stringify(start));
    ok("new-chat: the conversations going stand waiting first, then at work, then ended",
      start.recentHead === "이어서 하기" && start.recent.join(",") === "waiting,working,done", JSON.stringify(start.recent));

    const menus = await page.evaluate(async () => {
      const names = (menu) => [...(menu?.querySelectorAll(".composer-menu-item") ?? [])]
        .map((item) => (item.getAttribute("aria-current") === "true" ? "*" : "") + item.querySelector(".composer-menu-name").textContent);
      const agent = names(await window.__NEW_CHAT_MENU__(".newchat-agent"));
      closeComposerMenu();
      const effort = names(await window.__NEW_CHAT_MENU__(".newchat-effort"));
      closeComposerMenu();
      const engine = names(await window.__NEW_CHAT_MENU__(".newchat-engine"));
      closeComposerMenu();
      return { agent, effort, engine };
    });
    ok("new-chat: the chips' menus are the composer's — the models under the CLI's default with the agents after, the effort ladder, the two engines",
      menus.agent.join(",").startsWith("*기본,Opus 5.5,Sonnet 5.5,") && menus.agent.includes("*Claude") && menus.agent.includes("Codex") &&
        menus.effort.join(",") === "*기본,low,medium,high,xhigh,max" && menus.engine.join(",") === "*터미널 판,GUI 세션",
      JSON.stringify(menus));

    const narrow = await page.evaluate(newChatOneLine, ".worker-composer-tools") &&
      await page.evaluate(newChatOneLine, ".newchat-under");
    await page.setViewportSize({ width: 1024, height: 760 });
    await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
    const narrowAfter = await page.evaluate(newChatOneLine, ".worker-composer-tools") &&
      await page.evaluate(newChatOneLine, ".newchat-under");
    await page.setViewportSize({ width: 1280, height: 860 });
    ok("new-chat: the chip line and the line under the box stay one line each, in a wide window and at 1024px",
      narrow && narrowAfter, JSON.stringify({ narrow, narrowAfter }));

    const pane = await page.evaluate(async () => {
      const view = window.__NEW_CHAT_VIEW__();
      const picked = [
        await window.__NEW_CHAT_PICK__(".newchat-agent", "Claude"),
        await window.__NEW_CHAT_PICK__(".newchat-agent", "Opus 5.5"),
        await window.__NEW_CHAT_PICK__(".newchat-effort", "high"),
      ];
      const chips = { agent: view.querySelector(".newchat-agent").textContent, effort: window.__NEW_CHAT_WORDS__(".newchat-effort") };
      const form = view.querySelector(".newchat-composer");
      await composerAttachmentsOf(form).add(["/tmp/zerocode-window-test/README.md"]);
      const box = view.querySelector(".newchat-box");
      box.value = "로그인 화면의 오류를 고쳐 줘";
      box.dispatchEvent(new Event("input"));
      const sendOn = !view.querySelector(".newchat-send").disabled;
      window.__LAUNCHED__ = null;
      box.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
      for (let wait = 0; wait < 40 && !window.__LAUNCHED__; wait += 1) await new Promise((done) => setTimeout(done, 25));
      await new Promise((done) => setTimeout(done, 50));
      return {
        picked,
        chips,
        sendOn,
        launched: window.__LAUNCHED__,
        startOpen: tabs.some((tab) => tab.kind === "newchat"),
        front: tabs.find((tab) => tab.id === activeTabId)?.kind ?? null,
      };
    });
    ok("new-chat: picked from the chips' menus, the chips say the agent·model and the effort",
      pane.picked.every(Boolean) && pane.chips.agent.includes("Claude") && pane.chips.agent.includes("Opus 5.5") && pane.chips.effort === "high",
      JSON.stringify(pane));
    ok("new-chat: Enter on the pane engine launches the agent with the first words and the attached path, the model and the effort, and the page closes behind it",
      pane.sendOn && pane.launched?.agent === "claude" && pane.launched?.prompt?.startsWith("로그인 화면의 오류를 고쳐 줘") &&
        pane.launched?.prompt?.includes("/tmp/zerocode-window-test/README.md") &&
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
      startNewConversation();
      await new Promise((done) => setTimeout(done, 50));
      const view = window.__NEW_CHAT_VIEW__();
      const engined = await window.__NEW_CHAT_PICK__(".newchat-engine", "GUI 세션");
      const engine = window.__NEW_CHAT_WORDS__(".newchat-engine");
      await window.__NEW_CHAT_PICK__(".newchat-effort", "max");
      const box = view.querySelector(".newchat-box");
      box.value = "테스트를 돌려 줘";
      box.dispatchEvent(new Event("input"));
      view.querySelector(".newchat-send").click();
      for (let wait = 0; wait < 60 && window.__WIRE_SENT__.length === 0; wait += 1) await new Promise((done) => setTimeout(done, 25));
      const tab = tabs.find((one) => one.kind === "worker" && one.worker?.wire === 41);
      return {
        engined,
        engine,
        saved: window.__ENGINE_SET__,
        radio: document.querySelector('#new-chat-engine input[value="wire"]')?.checked ?? null,
        start: window.__WIRE_START__,
        sent: window.__WIRE_SENT__.map((one) => one.text ?? one.message ?? ""),
        tab: Boolean(tab),
        startOpen: tabs.some((one) => one.kind === "newchat"),
      };
    });
    ok("new-chat: the engine chip writes the setting the settings page writes, and its radio follows",
      wire.engined && wire.engine === "GUI 세션" && wire.saved.at(-1) === "wire" && wire.radio === true, JSON.stringify(wire));
    ok("new-chat: on the GUI-session engine, Enter starts a wire with the effort and sends the first words down it",
      wire.start?.agent === "claude" && wire.start?.effort === "max" &&
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
