/* The doors to a conversation, clicked the way a person clicks them.
 *
 * A helper's own page — the screen that shows what it was asked, what it did and what it reported —
 * opens from the sidebar's reading hand on a helper's row (`workers.mjs` and `sidebar-agents.mjs` press
 * that hand and the row), and from the board, where a card opens the look at its pane. This file presses
 * the board's doors: the card, then the footer's 「대화로 보기」 of the look, and the inspector's button.
 *
 *   - a card for a pane (a ledger-seated worker included) → the pane's conversation, up, in its own tab;
 *   - a card for a helper → the helper's page; a helper whose vendor left nothing to read falls back to
 *     the parent's pane and says why in one line, as the sidebar does;
 *   - a card only the ledger knows (`worker:`) has no conversation this window can read, and its button
 *     says so and opens nothing;
 *   - the card's own press is still the look at the pane's screen, and the title bar's 「터미널 | 대화」
 *     toggle and the parent's input are not changed by any of it.
 * Every fixture is built from the window's own roads (`hook:agent`, `hook:subagent`, a card from
 * `boardCardNode`), and the page dies with the suite. */
import { pathToFileURL } from "node:url";
import { resolve } from "node:path";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";

export async function testHelperDoors(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    const seen = await page.evaluate(async () => {
      const seen = {};
      const wait = (ms) => new Promise((done) => setTimeout(done, ms));
      const tell = (name, payload) => {
        for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload });
      };
      const toasts = () => [...document.querySelectorAll(".toasts .toast")].map((node) => node.textContent);
      const visible = (node) => Boolean(node) && !node.hidden && node.getClientRects().length > 0;
      // The inspector may stand collapsed (nothing selected): its button is judged by its own state, not by the layout.
      const standing = (node) => Boolean(node) && !node.hidden;
      const term = await openTermTab({ placement: "tab" });
      const owner = tabOfTerm(term);
      // Before any card is pressed: the page carries neither door. They are made the first time a card wants
      // one, so the page's own element and listener counts are what they were without the doors.
      seen.boot = {
        peek: document.getElementById("peek-chat") === null,
        inspector: document.querySelector(".agent-inspector-chat") === null,
      };
      paneAgents.set(term, "zo");
      tell("hook:agent", { term, state: "working", agent: "zo", session: "s-doors" });
      // The ledger seated a worker in this pane: its task is the card's heading.
      paneLedger.set(term, { run: "run-doors", worker: "w-77", task: "도우미 대화창 구현", taskId: "t-1", ledger: "working", reported: false, failed: false, review: null, closed: null });
      tell("hook:subagent", { term, rows: [
        { id: "h-read", name: "price-research", state: "running", tool_calls: 4 },
        { id: "h-none", name: "quiet-helper", state: "running", tool_calls: 1 },
      ] });
      await window.__PAINTED__();
      window.__ANSWER__.subagent_log = (args) => args.id === "h-read"
        ? { found: true, next: 2, turns: [{ role: "user", text: "Read-only audit. Do not edit files." }, { role: "assistant", text: "reading" }] }
        : { found: false };
      window.__ANSWER__.pane_log = () => ({
        found: true, next: 2, skipped: false, more: false, folded: false, model: "claude-opus-5",
        turns: [{ role: "user", text: "도우미 대화창을 마무리해 줘." }, { role: "assistant", text: "확인했습니다." }],
      });
      const cardOf = (pane, heading) => ({
        pane, agent: "zo", state: "working", ledger: "working", heading, worktree: owner.worktree, project: "", task: heading,
        you: "", said: "", ask: "", ask_prompt: null, approval: null, parent: "", lineage: { depth: 0, root: String(term), parent: null },
        unseen: false, changed_at: 0, at: Date.now(), autonomy: null,
      });
      const press = async (card) => {
        const node = boardCardNode(card, Date.now(), "working", null, null);
        node.click();
        await wait(160);
        return { opened: !document.getElementById("peek-scrim").hidden, door: document.getElementById("peek-chat") };
      };
      const shut = () => {
        if (!document.getElementById("peek-scrim").hidden) document.getElementById("peek-close").click();
      };
      const reset = () => {
        const chat = paneChats.get(term);
        if (chat?.on) void setPaneChat(term, false);
        setActiveTab("board");
      };

      // 1. A card for the pane — the ledger's worker seated in it. The card still opens the look at the pane's
      //    screen; the footer's door opens its conversation.
      openTab({ id: "board", kind: "board" });
      const termCard = cardOf(`term:${term}`, "도우미 대화창 구현");
      const first = await press(termCard);
      seen.term = {
        peeked: first.opened,
        shown: visible(first.door),
        words: first.door?.textContent ?? null,
        want: t("view.conversationTip", "대화로 보기"),
        open: visible(document.getElementById("peek-open")),
      };
      first.door?.click();
      await wait(200);
      seen.term.closed = document.getElementById("peek-scrim").hidden;
      seen.term.chatOn = paneChatOn(term);
      seen.term.tab = activeTabId === owner.id;
      seen.term.pageUp = document.querySelector(`.pane-slot[data-term="${term}"] .pane-chat .helper-turns`) !== null;
      shut();
      reset();
      await wait(80);

      // 2. A card for a helper that left a transcript → the helper's page.
      const helperCard = cardOf(`sub:${term}:h-read`, "price-research");
      const second = await press(helperCard);
      seen.sub = {
        peeked: second.opened,
        shown: visible(second.door),
        words: second.door?.textContent ?? null,
        want: t("session.subagentOpenTranscript", "헬퍼 대화 보기"),
      };
      second.door?.click();
      await wait(240);
      seen.sub.closed = document.getElementById("peek-scrim").hidden;
      seen.sub.page = activeTabId === `helper:${term}:h-read`;
      seen.sub.name = document.querySelector("#worker-view .helper-head .worker-name")?.textContent ?? null;
      seen.sub.brief = document.querySelector("#worker-view .helper-brief") !== null;
      shut();
      reset();
      await wait(80);

      // 3. A helper whose vendor left nothing to read → the parent's pane, and one line saying why.
      const quietCard = cardOf(`sub:${term}:h-none`, "quiet-helper");
      const third = await press(quietCard);
      const before = toasts().length;
      const thirdShown = visible(third.door);
      third.door?.click();
      await wait(240);
      seen.none = {
        shown: thirdShown,
        noPage: !tabs.some((tab) => tab.id === `helper:${term}:h-none`),
        parent: activeTabId === owner.id,
        said: toasts().slice(before),
        want: t("session.subagentNoTranscript", "이 에이전트는 따로 볼 기록을 남기지 않아 부모 에이전트의 화면을 엽니다"),
      };
      shut();
      reset();
      await wait(80);

      // 4. The inspector's button, on the board's own view: the same doors, the same words, and the card that only
      //    the ledger knows says there is no conversation and opens nothing.
      leavePagesForStage();
      openBoard();
      await paintBoardView(boardTab(), { force: true });
      const view = docHost(boardTab().pane, "board");
      const chat = () => view.querySelector(".agent-inspector-chat");
      const entityOf = (card) => ({ type: "agent", card, bucket: "working", facts: { identity: card.heading }, place: null });
      paintAgentInspectorDestinations(view, entityOf(termCard));
      seen.inspectorTerm = { shown: standing(chat()), words: chat()?.textContent ?? null, disabled: chat()?.getAttribute("aria-disabled") ?? null };
      chat()?.click();
      await wait(200);
      seen.inspectorTerm.chatOn = paneChatOn(term) && activeTabId === owner.id;
      reset();
      leavePagesForStage();
      openBoard();
      await paintBoardView(boardTab(), { force: true });
      paintAgentInspectorDestinations(view, entityOf(helperCard));
      seen.inspectorSub = { shown: standing(chat()), words: chat()?.textContent ?? null };
      chat()?.click();
      await wait(240);
      seen.inspectorSub.page = activeTabId === `helper:${term}:h-read`;
      reset();
      leavePagesForStage();
      openBoard();
      await paintBoardView(boardTab(), { force: true });
      const tabsBefore = tabs.length;
      const ledgerOnly = cardOf("worker:w-9", "원장에만 있는 일");
      paintAgentInspectorDestinations(view, entityOf(ledgerOnly));
      seen.ledgerOnly = {
        shown: standing(chat()),
        words: chat()?.textContent ?? null,
        want: t("board.graph.noConversation", "대화 기록 없음 — 원장에만 있는 실행입니다"),
        disabled: chat()?.getAttribute("aria-disabled") ?? null,
        pressable: chat() ? chat().onclick !== null : null,
      };
      chat()?.click();
      await wait(120);
      seen.ledgerOnly.opened = tabs.length !== tabsBefore || paneChatOn(term) || tabs.some((tab) => tab.id.startsWith("helper:") && tab.id !== `helper:${term}:h-read`);
      // A card with no pane we can open, or a lane, has no door at all.
      paneAgents.delete(term);
      paintAgentInspectorDestinations(view, entityOf(termCard));
      seen.noAgent = { shown: standing(chat()) };
      paneAgents.set(term, "zo");
      paintAgentInspectorDestinations(view, entityOf(cardOf("lane:L-1", "a lane")));
      seen.lane = { shown: standing(chat()) };

      // 5. Made once, and worn in the language in force. The look opened over and over leaves one footer door that
      //    answers a press once; the inspector painted over and over leaves one button; and a change of language is
      //    worn by the doors — the look's, the helper's, and the one that takes no press — in the catalog's words.
      let opened = 0;
      const openedBy = typeof openBoardConversation === "function" ? openBoardConversation : null;
      if (openedBy) {
        openBoardConversation = (card) => {
          opened += 1;
          return openedBy(card);
        };
      }
      for (let time = 0; time < 3; time += 1) {
        await press(termCard);
        shut();
      }
      const again = await press(termCard);
      const footDoors = document.querySelectorAll("#peek-chat").length;
      again.door?.click();
      await wait(200);
      shut();
      reset();
      leavePagesForStage();
      openBoard();
      await paintBoardView(boardTab(), { force: true });
      for (let time = 0; time < 3; time += 1) {
        paintAgentInspectorDestinations(view, entityOf(termCard));
        paintAgentInspectorDestinations(view, entityOf(helperCard));
      }
      seen.again = { footDoors, opened, inspectorDoors: view.querySelectorAll(".agent-inspector-chat").length };
      if (openedBy) openBoardConversation = openedBy;
      const wore = locale;
      const worn = async (code) => {
        setLocale(code, { refresh: false, persist: false });
        const looked = await press(termCard);
        const peek = looked.door?.textContent ?? null;
        shut();
        paintAgentInspectorDestinations(view, entityOf(helperCard));
        const sub = chat()?.textContent ?? null;
        paintAgentInspectorDestinations(view, entityOf(ledgerOnly));
        return {
          peek, sub, ledger: chat()?.textContent ?? null, disabled: chat()?.getAttribute("aria-disabled") ?? null,
          wantPeek: t("view.conversationTip", "대화로 보기"),
          wantSub: t("session.subagentOpenTranscript", "헬퍼 대화 보기"),
          wantLedger: t("board.graph.noConversation", "대화 기록 없음 — 원장에만 있는 실행입니다"),
        };
      };
      seen.lang = { ko: await worn("ko") };
      for (const code of ["en", "ja", "zh", "es"]) seen.lang[code] = await worn(code);
      setLocale(wore, { refresh: false, persist: false });
      reset();

      // 6. A pane whose agent can be driven on a wire (the catalog's Claude Code): the door is a look. Its CLI keeps
      //    running — no wire starts and the pane's tab stays — where the title bar's toggle would have handed it over.
      const wired = await openTermTab({ placement: "tab" });
      paneAgents.set(wired, "claude");
      tell("hook:agent", { term: wired, state: "idle", agent: "claude", session: "s-wired" });
      paneSessions.set(wired, { agent: "claude", session: { key: "session_id", id: "s-wired" }, resumable: true });
      let wireStarts = 0;
      window.__ANSWER__.wire_start = (args) => {
        wireStarts += 1;
        return { id: 77, agent: args.agent, protocol: "claude-stream", version: "2.1.280", model: null, session: null };
      };
      setActiveTab("board");
      const fourth = await press(cardOf(`term:${wired}`, "a worker on its wire"));
      const fourthShown = visible(fourth.door);
      fourth.door?.click();
      await wait(260);
      seen.wired = {
        shown: fourthShown,
        chatOn: paneChatOn(wired),
        wireStarts,
        paneStays: tabOfTerm(wired) !== null,
        noWireTab: !tabs.some((tab) => tab.kind === "worker" && tab.worker.wire),
      };
      shut();
      delete window.__ANSWER__.wire_start;

      delete window.__ANSWER__.subagent_log;
      delete window.__ANSWER__.pane_log;
      paneLedger.delete(term);
      tell("hook:subagent", { term, rows: [] });
      tell("term:exited", { term });
      window.__PANES__ = [];
      for (const tab of [...tabs]) dropTab(tab.id);
      for (const at of [...termViews.keys()]) dropTermView(at);
      return seen;
    });
    ok(
      "the page carries neither conversation door until a card wants one — no button in the look's footer and none in the inspector's — so its own element and listener counts stay what they were without the doors",
      seen.boot.peek && seen.boot.inspector,
      JSON.stringify(seen.boot),
    );
    ok(
      "a board card for a pane the ledger seated a worker in still opens the look at its screen, and the look's footer carries 「대화로 보기」 — pressed, the look closes and the pane's tab stands with its conversation up",
      seen.term.peeked && seen.term.open && seen.term.shown && seen.term.words === seen.term.want &&
        seen.term.closed && seen.term.chatOn && seen.term.tab && seen.term.pageUp,
      JSON.stringify(seen.term),
    );
    ok(
      "a board card for a helper that left a transcript carries the helper-conversation door, and pressing it opens the helper's own page — its name in the head, what it was asked in the card",
      seen.sub.peeked && seen.sub.shown && seen.sub.words === seen.sub.want && seen.sub.closed && seen.sub.page &&
        seen.sub.name === "price-research" && seen.sub.brief,
      JSON.stringify(seen.sub),
    );
    ok(
      "a helper whose vendor left nothing to read still has the door, and it falls back the way the sidebar's does — the parent's pane in front, one line saying why, no page made up",
      seen.none.shown && seen.none.noPage && seen.none.parent && seen.none.said.length === 1 && seen.none.said[0].includes(seen.none.want),
      JSON.stringify(seen.none),
    );
    ok(
      "the inspector's own button opens the same doors — a pane's conversation, a helper's page — with the catalog's words for each",
      seen.inspectorTerm.shown && seen.inspectorTerm.words === seen.term.want && seen.inspectorTerm.disabled === "false" && seen.inspectorTerm.chatOn &&
        seen.inspectorSub.shown && seen.inspectorSub.words === seen.sub.want && seen.inspectorSub.page,
      JSON.stringify({ term: seen.inspectorTerm, sub: seen.inspectorSub }),
    );
    ok(
      "a card only the ledger knows says there is no conversation to read here — in the button's own words, taking no press and opening nothing — and a pane with no agent or a lane has no button at all",
      seen.ledgerOnly.shown && seen.ledgerOnly.words === seen.ledgerOnly.want && seen.ledgerOnly.disabled === "true" &&
        seen.ledgerOnly.pressable === false && seen.ledgerOnly.opened === false &&
        seen.noAgent.shown === false && seen.lane.shown === false,
      JSON.stringify({ ledgerOnly: seen.ledgerOnly, noAgent: seen.noAgent, lane: seen.lane }),
    );
    ok(
      "the board's door on a pane that can be driven on a wire is a look — the pane's conversation stands up in its own tab, the pane's CLI is left running, no wire is started",
      seen.wired.shown && seen.wired.chatOn && seen.wired.wireStarts === 0 && seen.wired.paneStays && seen.wired.noWireTab,
      JSON.stringify(seen.wired),
    );
    ok(
      "the doors are made once — the look opened four times leaves one footer door that answers a press once, and the inspector painted over and over leaves one button",
      seen.again.footDoors === 1 && seen.again.opened === 1 && seen.again.inspectorDoors === 1,
      JSON.stringify(seen.again),
    );
    ok(
      "a change of language is worn by the doors in the catalog's words — the look's, the helper's and the one that takes no press — and none is left in the language it was made in",
      Object.entries(seen.lang).every(([code, one]) =>
        one.peek === one.wantPeek && one.sub === one.wantSub && one.ledger === one.wantLedger && one.disabled === "true" &&
        (code === "ko" || (one.peek !== seen.lang.ko.peek && one.sub !== seen.lang.ko.sub && one.ledger !== seen.lang.ko.ledger))),
      JSON.stringify(seen.lang),
    );
    ok("the board's doors raised no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  let failed = 0;
  try {
    await testHelperDoors(browser, origin, (name, pass, details = "") => {
      console.log(`${pass ? "PASS" : "FAIL"} ${name}${!pass ? `\n  ${details}` : ""}`);
      if (!pass) failed += 1;
    });
  } finally {
    await browser.close();
    files.close();
  }
  if (failed) process.exitCode = 1;
}
