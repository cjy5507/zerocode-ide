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

/* chat-list-keyed (2026-11-10, v1.1.141): the list follows the agents' news on the beat every agent paint
 * goes through (`flushAgentPaint`), not only a press or a setting, and puts its groups and rows in place by
 * their keys — a pane whose state moved is the one row built again, the other rows and the groups keep their
 * nodes, and a row whose time alone moved keeps its node with its time words changed where they stand. Read
 * by what the page shows, never by a time. */
export async function testChatListKeyed(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.evaluate(standConversations);
    const seen = await page.evaluate(async () => {
      const rows = () => new Map([...document.querySelectorAll("#chat-list .chat-row")].map((row) => [row.dataset.term, row]));
      const groups = () => new Map([...document.querySelectorAll("#chat-list .chat-group")].map((group) => [group.dataset.group, group]));
      const beat = async () => {
        scheduleAgentPaint(["cards"]);
        for (let frame = 0; frame < 3; frame += 1) await window.__PAINTED__();
      };
      const { waiting, working } = window.__TERMS__;
      // 「오늘 끝남」 open too, so three rows stand: the one that moves and two that must not be built again.
      document.querySelector('.chat-group[data-group="today"] .chat-group-head').click();
      for (let frame = 0; frame < 2; frame += 1) await window.__PAINTED__();
      const rowsBefore = rows();
      const groupsBefore = groups();
      // One pane asks for the person: told by the hook's state, drawn on the agents' beat — no call to the list.
      hookStates.set(working, "needs-attention");
      hookStates.observe(working, Date.now());
      await beat();
      const rowsAfter = rows();
      const groupsAfter = groups();
      const moved = {
        inWaiting: Boolean(document.querySelector(`.chat-group[data-group="waiting"] .chat-row[data-term="${working}"]`)),
        gone: !document.querySelector(`.chat-group[data-group="working"] .chat-row[data-term="${working}"]`),
        others: [...rowsBefore].filter(([term]) => term !== String(working) && rowsAfter.has(term))
          .map(([term, node]) => rowsAfter.get(term) === node),
        groups: [...groupsBefore].filter(([name]) => groupsAfter.has(name)).map(([name, node]) => groupsAfter.get(name) === node),
        waitingCount: document.querySelector('.chat-group[data-group="waiting"] .chat-group-count')?.textContent ?? null,
      };
      // Its time alone moves: the row keeps its node, and its words say the new time.
      const row = rows().get(String(waiting));
      const wordBefore = row?.querySelector(".chat-row-word")?.textContent ?? null;
      hookStates.set(waiting, "working");
      hookStates.observe(waiting, Date.now());
      await beat();
      const working2 = rows().get(String(waiting));
      paneBorn.set(waiting, Date.now() - 5 * 60_000);
      await beat();
      const aged = rows().get(String(waiting));
      const time = {
        sameNode: Boolean(working2) && aged === working2,
        word: aged?.querySelector(".chat-row-word")?.textContent ?? null,
        tip: aged?.querySelector(".chat-row-open")?.dataset.tip ?? "",
        wordBefore,
      };
      return { moved, time, expected: shortAgo(Date.now() - 5 * 60_000, Date.now()) };
    });
    ok("chat-list-keyed: a pane that comes to ask for the person stands in 「기다림」 on the agents' own beat — nobody called the list — and every other row and every group keeps its node",
      seen.moved.inWaiting && seen.moved.gone && seen.moved.others.length >= 2 && seen.moved.others.every(Boolean) &&
        seen.moved.groups.length >= 2 && seen.moved.groups.every(Boolean) && seen.moved.waitingCount === "2",
      JSON.stringify(seen.moved));
    ok("chat-list-keyed: a row whose time alone moved keeps its node, its time words and its tooltip saying the new time",
      seen.time.sameNode && seen.time.word === seen.expected && seen.time.tip.includes(seen.expected), JSON.stringify(seen));
    ok("chat-list-keyed: no page errors", faults.length === 0, faults.join("\n"));
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
    // With the sidebar's list standing the page keeps the conversations that wait for the person (2026-11-10,
    // window artifact p-1c1bbf75726ff227, decision 5); with the list off, the five of 2026-11-09 as before.
    const unlisted = await page.evaluate(() => {
      const view = window.__NEW_CHAT_VIEW__();
      delete document.documentElement.dataset.chatList;
      paintNewChatViews();
      const seen = {
        recent: [...view.querySelectorAll(".newchat-recent-row")].map((row) => row.dataset.state),
        head: view.querySelector(".newchat-recent-head").textContent,
      };
      document.documentElement.dataset.chatList = "on";
      paintNewChatViews();
      return seen;
    });
    ok("new-chat: beside the sidebar's list the page keeps only the conversations that wait for the person; with the list off, the ones going stand waiting first, then at work, then ended",
      start.recentHead === "나를 기다리는 대화" && start.recent.join(",") === "waiting" &&
        unlisted.head === "이어서 하기" && unlisted.recent.join(",") === "waiting,working,done",
      JSON.stringify({ listed: [start.recentHead, start.recent], unlisted }));

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

/* The start page as the person passed it on 2026-11-10 (window artifact p-1c1bbf75726ff227, 「새 대화 화면」):
 * the block a little above the page's middle, the keys the box takes on a line under it, and — under the box —
 * what the checkout could use next, read off what the window holds of it and written into the box by a press. */
export async function testNewChatStart(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.evaluate(standConversations);
    await page.evaluate(installNewChatHands);
    await page.evaluate(() => {
      window.__ANSWER__.agent_models = () => [];
      window.__ANSWER__.agent_effort_ladder = () => [];
      window.__ANSWER__.path_kinds = ({ paths }) => paths.map(() => "file");
      window.__REVIEW_ASKS__ = [];
      window.__REVIEWS__ = [];
      // The backend answers when the check lets it, so the page can be read while it still asks.
      window.__ANSWER__.github_review_states = (args) => {
        window.__REVIEW_ASKS__.push(args.worktrees);
        return new Promise((answer) => { window.__REVIEW_ANSWER__ = () => answer(window.__REVIEWS__); });
      };
      document.getElementById("chat-list-new").click();
    });
    await page.waitForSelector(".newchat-view:not([hidden]) .newchat-box");
    const place = async () => page.evaluate(async () => {
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      const view = window.__NEW_CHAT_VIEW__();
      const block = view.querySelector(".newchat-center").getBoundingClientRect();
      const page = view.getBoundingClientRect();
      return {
        over: Math.round(block.top - page.top), under: Math.round(page.bottom - block.bottom), tall: Math.round(block.height),
        scrolls: view.scrollHeight > view.clientHeight + 1, top: Math.round(view.scrollTop),
        centred: Math.abs(Math.round(block.left + block.width / 2) - Math.round(page.left + page.width / 2)) <= 1,
      };
    });
    await page.setViewportSize({ width: 1440, height: 900 });
    const roomy = await place();
    await page.setViewportSize({ width: 1440, height: 300 });
    const tight = await place();
    await page.setViewportSize({ width: 1280, height: 860 });
    ok("start: the block stands a little above the page's middle — two parts of the free room over it, three under — and on a page too short for it reads from its top, the 32px step over it",
      roomy.centred && roomy.over > 32 && Math.abs(roomy.over / (roomy.over + roomy.under) - 0.4) <= 0.03 && !roomy.scrolls &&
        tight.scrolls && tight.over === 32 && tight.top === 0,
      JSON.stringify({ roomy, tight }));

    const words = await page.evaluate(() => {
      const view = window.__NEW_CHAT_VIEW__();
      const keys = view.querySelector(".newchat-keys");
      const suggest = view.querySelector(".newchat-suggest");
      return {
        placeholder: view.querySelector(".newchat-box").placeholder, keys: keys.textContent, keysShown: keys.getClientRects().length > 0,
        keysAfterUnder: view.querySelector(".newchat-under").compareDocumentPosition(keys) === Node.DOCUMENT_POSITION_FOLLOWING,
        suggestHidden: suggest.hidden, suggestRoom: suggest.getClientRects().length, suggestName: suggest.getAttribute("aria-label"),
      };
    });
    ok("start: the box asks in three words and the keys it takes stand on a line under it — only the ones it takes",
      words.placeholder === "무엇이든 시키세요" && words.keys === "Enter 보내기 · Shift+Enter 줄바꿈 · + 파일 첨부" && words.keysShown && words.keysAfterUnder,
      JSON.stringify(words));
    ok("start: with nothing known of the checkout no offer stands, and its row takes no room",
      words.suggestHidden && words.suggestRoom === 0 && words.suggestName === "지금 할 수 있는 일", JSON.stringify(words));

    // The checkout as source control and the landing check know it: a clash ahead, unsaved changes, three commits the
    // compare ref lacks, two it gained — and no pull request.
    const offers = () => page.evaluate(() => [...window.__NEW_CHAT_VIEW__().querySelectorAll(".newchat-suggest:not([hidden]) > .newchat-suggestion")]
      .map((pill) => ({ id: pill.dataset.offer, words: pill.textContent, button: pill.tagName === "BUTTON" && pill.type === "button" })));
    const ref = await page.evaluate(() => {
      worktreeLandings.set(activeWorktreePath, { state: "unlanded", ahead: 3, dirty: true, behind: 2, compare_ref: "origin/main",
        conflict: { total: 1, files: ["src/a.rs"] } });
      paintNewChatViews();
      return landingRefName("origin/main");
    });
    await page.waitForFunction(() => window.__REVIEW_ASKS__.length === 1);
    const asking = await offers();
    await page.evaluate(() => window.__REVIEW_ANSWER__());
    await page.waitForFunction(() => window.__NEW_CHAT_VIEW__().querySelector('.newchat-suggestion[data-offer="pr"]') !== null);
    const known = await offers();
    const asked = await page.evaluate(() => window.__REVIEW_ASKS__.map((list) => list.length));
    ok("start: what the window holds of the checkout stands as offers — a clash to resolve, changes to review, commits to catch up with — and the pull request's only once the backend said there is none",
      asking.map((one) => one.id).join() === "conflict,dirty,behind" && known.map((one) => one.id).join() === "conflict,dirty,pr,behind" &&
        known.map((one) => one.words).join("|") === `충돌 1개 풀기|커밋 안 한 변경 검토|PR 설명 쓰기|${ref} 따라잡기` &&
        known.every((one) => one.button) && asked.join() === "1",
      JSON.stringify({ asking, known, asked, ref }));

    const filled = await page.evaluate(async () => {
      const view = window.__NEW_CHAT_VIEW__();
      const box = view.querySelector(".newchat-box");
      window.__LAUNCHED__ = null;
      view.querySelector('.newchat-suggestion[data-offer="pr"]').click();
      const first = { value: box.value, focus: document.activeElement === box, caret: box.selectionStart === box.value.length, sendOff: view.querySelector(".newchat-send").disabled };
      view.querySelector('.newchat-suggestion[data-offer="dirty"]').click();
      await new Promise((done) => setTimeout(done, 60));
      return { first, second: box.value, launched: window.__LAUNCHED__, draft: newChatChoice.draft === box.value };
    });
    ok("start: a press writes the offer's sentence into the box with the keyboard at its end and sends nothing; a second offer goes on a line of its own after it",
      filled.first.value === `${ref}에 없는 커밋 3개로 PR 설명을 써 줘.` && filled.first.focus && filled.first.caret && !filled.first.sendOff &&
        filled.second === `${ref}에 없는 커밋 3개로 PR 설명을 써 줘.\n커밋하지 않은 변경을 검토해 줘.` && filled.launched === null && filled.draft,
      JSON.stringify(filled));

    // A checkout that has its pull request is not offered one; a landed, clean checkout is offered nothing.
    const withReview = await page.evaluate(async () => {
      window.__REVIEWS__ = [{ worktree: activeWorktreePath, number: 7, state: "open", title: "fix" }];
      newChatReviews.clear();
      paintNewChatViews();
      await new Promise((done) => setTimeout(done, 40));
      window.__REVIEW_ANSWER__();
      await new Promise((done) => setTimeout(done, 80));
      const ids = [...window.__NEW_CHAT_VIEW__().querySelectorAll(".newchat-suggestion")].map((pill) => pill.dataset.offer);
      worktreeLandings.set(activeWorktreePath, { state: "landed", ahead: 0, dirty: false, behind: 0, compare_ref: "origin/main" });
      paintNewChatViews();
      const row = window.__NEW_CHAT_VIEW__().querySelector(".newchat-suggest");
      return { ids, hidden: row.hidden, room: row.getClientRects().length };
    });
    ok("start: a checkout that has its pull request is not offered one, and a checkout with nothing to do is offered nothing — no empty row left",
      withReview.ids.join() === "conflict,dirty,behind" && withReview.hidden && withReview.room === 0, JSON.stringify(withReview));

    // One line each at 1024, 1440 and 2560, the offers standing. At 1024 the stage between the two side
    // columns is 433px, narrower than four offers: they take a second line rather than leave the column.
    await page.evaluate(() => {
      worktreeLandings.set(activeWorktreePath, { state: "unlanded", ahead: 3, dirty: true, behind: 2, compare_ref: "origin/main",
        conflict: { total: 1, files: ["src/a.rs"] } });
      window.__REVIEWS__ = [];
      newChatReviews.clear();
      paintNewChatViews();
    });
    await page.waitForFunction(() => window.__REVIEW_ASKS__.length === 3);
    await page.evaluate(() => window.__REVIEW_ANSWER__());
    await page.waitForFunction(() => window.__NEW_CHAT_VIEW__().querySelectorAll(".newchat-suggestion").length === 4);
    const lines = {};
    for (const width of [1024, 1440, 2560]) {
      await page.setViewportSize({ width, height: 900 });
      await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
      lines[width] = {
        tools: await page.evaluate(newChatOneLine, ".worker-composer-tools"), under: await page.evaluate(newChatOneLine, ".newchat-under"),
        offers: await page.evaluate(newChatOneLine, ".newchat-suggest"),
        offersInside: await page.evaluate(() => {
          const view = window.__NEW_CHAT_VIEW__();
          const column = view.querySelector(".newchat-center").getBoundingClientRect();
          const row = view.querySelector(".newchat-suggest").getBoundingClientRect();
          return [...view.querySelectorAll(".newchat-suggestion")].every((pill) => pill.getBoundingClientRect().right <= column.right + 1) &&
            row.height <= 2 * 28 + 8 + 1;
        }),
        keys: await page.evaluate(() => {
          const keys = window.__NEW_CHAT_VIEW__().querySelector(".newchat-keys");
          return keys.getBoundingClientRect().height <= parseFloat(getComputedStyle(keys).lineHeight) + 1;
        }),
        sideways: await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth),
      };
    }
    ok("start: at 1024, 1440 and 2560 the chip line, the line under the box and the keys each stay on one line; four offers are one line from 1440 up and two inside the column at 1024; the page does not move sideways",
      Object.values(lines).every((one) => one.tools && one.under && one.keys && one.offersInside && one.sideways <= 0) &&
        lines[1440].offers && lines[2560].offers && !lines[1024].offers,
      JSON.stringify(lines));
    ok("start: no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}
