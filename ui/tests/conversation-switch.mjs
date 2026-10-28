/* 대화 바꾸기·판의 「대화」·늦은 첫 읽기 (t-54304).
 *
 * 사람(10-14): 「대화 창 전환시 cc나 codex 데스크탑처럼」. 세 앱(Claude·Codex·
 * Dots)은 대화를 바꿀 때 가진 것으로 먼저 그리고, 떠난 화면의 모양을 기억해
 * 돌아오면 같은 자리를 바로 그린다(정적 역분석, reports/t-54304/findings-*.md —
 * 저장소 밖). 여기 세 묶음:
 *   A. 한 판에서 대화를 바꿨다가 돌아오면 그 화면이 그대로 돌아온다 — 행 노드도,
 *      읽던 행도. 떼어 두는 화면은 판마다 상한(개수·시간)이 있고, 탭을 닫으면
 *      같이 간다. 떼어 둔 동안 칠하지 못한 코드 블록은 돌아오면 칠해진다.
 *   B. 판의 「대화」는 누른 다음 프레임들 안에 전사를 보인다 — 선(wire) 인계는
 *      뒤에서. 인계 중에 보낸 글은 한 번만, 순서대로 간다: 선이 서면 선으로,
 *      서지 못하면 판으로. 선으로 바뀌는 순간 읽던 행은 그대로다.
 *   C. 첫 읽기가 늦으면 「대화를 읽는 중…」이 토큰(`--chat-late-ms`)만큼 늦게
 *      나타나고(움직임 줄이기에서도), 읽기가 오면 사라진다.
 * 대화는 모두 합성이다(사람의 기록이 아니다). */
import { conversationFixture, standSwitcher } from "./conversation-perf.mjs";
import { openWindowTestPage, WINDOW_MOTION_REST } from "./window-boot.mjs";

export async function testConversationSwitch(browser, origin, ok) {
  await testParkedPages(browser, origin, ok);
  await testPageBehind(browser, origin, ok);
  await testKeepingAcrossLeaves(browser, origin, ok);
  await testBehindUnseen(browser, origin, ok);
  await testBehindTheScreen(browser, origin, ok);
  await testBehindAcrossKinds(browser, origin, ok);
  await testFocusPassesAside(browser, origin, ok);
  await testColoursAfterAside(browser, origin, ok);
  await testPaneHandover(browser, origin, ok, { refuse: false });
  await testPaneHandover(browser, origin, ok, { refuse: true });
  await testPressReadsNoSize(browser, origin, ok);
  await testLateFirstRead(browser, origin, ok);
}

/* The leaf's host and what it keeps aside, read without assuming the keeping
 * exists (a red tree has none: the checks fail at their assertions). */
const PARKED = () => {
  const host = document.querySelector("#worker-view") ?? [...document.querySelectorAll(".worker-view")].find((one) => one.checkVisibility() && !one.closest('[aria-hidden="true"]')) ?? null;
  const parked = host?.__parked ?? null;
  return { size: parked ? parked.size : null, ids: parked ? [...parked.keys()] : null };
};

async function testParkedPages(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await standSwitcher(page);
    const [a, b] = await page.evaluate(
      async ({ a, b }) => [await window.__SWITCH__.open(a), await window.__SWITCH__.open(b)],
      { a: conversationFixture({ blocks: 20 }), b: conversationFixture({ blocks: 10 }) },
    );
    const seen = {};
    // --- returning_to_a_conversation_keeps_its_page
    seen.backToA = await page.evaluate((tab) => window.__SWITCH__.hop(tab), a);
    seen.backToB = await page.evaluate((tab) => window.__SWITCH__.hop(tab), b);
    ok(
      "returning_to_a_conversation_keeps_its_page: a conversation looked away from (another conversation of the same leaf opened over it) comes back with every row it stood with — no row is built again — and so does the other one",
      seen.backToA.rows > 300 && seen.backToA.rowsKept === seen.backToA.rows && seen.backToA.rowsBuilt === 0 &&
        seen.backToB.rows > 150 && seen.backToB.rowsKept === seen.backToB.rows && seen.backToB.rowsBuilt === 0,
      JSON.stringify(seen),
    );

    // --- the_reader_comes_back_to_the_row_they_left
    await page.evaluate((tab) => window.__SWITCH__.hop(tab), a);
    const box = await page.evaluate(() => window.__SWITCH__.box());
    await page.mouse.move(box.x, box.y);
    await page.mouse.wheel(0, -Math.round(box.height / 2));
    await page.evaluate(() => window.__SWITCH__.still());
    seen.place = await page.evaluate(() => window.__SWITCH__.reading());
    await page.evaluate((tab) => window.__SWITCH__.hop(tab), b);
    seen.readerBack = await page.evaluate((tab) => window.__SWITCH__.hop(tab), a);
    await page.evaluate(() => window.__SWITCH__.still());
    seen.returned = await page.evaluate(() => window.__SWITCH__.reading());
    seen.door = await page.evaluate(() => {
      const list = [...document.querySelectorAll(".helper-turns")].find((one) => one.checkVisibility() && !one.closest('[aria-hidden="true"]'));
      const door = list?.parentElement.querySelector(":scope > .chat-foot-door");
      return door ? door.classList.contains("is-shown") : null;
    });
    ok(
      "the_reader_comes_back_to_the_row_they_left: a conversation wheeled up, looked away from and back to stands on the same row at the same height (within a pixel), with its rows kept and the way back to the foot shown",
      seen.place !== null && seen.returned !== null && seen.place.turn === seen.returned.turn &&
        Math.abs(seen.place.offset - seen.returned.offset) <= 1 && seen.readerBack.rowsBuilt === 0 && seen.door === true,
      JSON.stringify({ place: seen.place, returned: seen.returned, kept: seen.readerBack.rowsKept, door: seen.door }),
    );

    // --- a_leaf_keeps_two_conversations_aside_and_lets_the_oldest_go
    const [c, d] = await page.evaluate(
      async ({ c, d }) => [await window.__SWITCH__.open(c), await window.__SWITCH__.open(d)],
      { c: conversationFixture({ blocks: 5 }), d: conversationFixture({ blocks: 5 }) },
    );
    // B went aside first (A came back over it), then A (C opened over it), then
    // C (D over it): B is the one let go.
    seen.keptAside = await page.evaluate(PARKED);
    seen.oldest = await page.evaluate((tab) => window.__SWITCH__.hop(tab), b);
    seen.recent = await page.evaluate((tab) => window.__SWITCH__.hop(tab), c);
    seen.limit = await page.evaluate(() => (typeof CHAT_PARK === "undefined" ? null : CHAT_PARK.pages));
    ok(
      "a_leaf_keeps_two_conversations_aside_and_lets_the_oldest_go: with four conversations opened in one leaf the leaf keeps `CHAT_PARK.pages` (2) of them aside — the two looked at last — so the oldest is built again when it comes back and a recent one is not",
      seen.limit === 2 && seen.keptAside.size === 2 && seen.oldest.rowsKept === 0 && seen.oldest.rows > 150 &&
        seen.recent.rowsBuilt === 0 && seen.recent.rows > 0,
      JSON.stringify({ limit: seen.limit, keptAside: seen.keptAside, oldest: seen.oldest.rowsKept, recent: seen.recent.rowsBuilt }),
    );

    // --- a_conversation_kept_aside_goes_when_its_tab_closes_or_its_time_runs_out
    seen.beforeClose = await page.evaluate(PARKED);
    seen.closed = await page.evaluate((tab) => {
      closeTab(tab);
      const host = document.querySelector("#worker-view");
      return { size: host?.__parked?.size ?? null, holds: host?.__parked?.has(tab) ?? null };
    }, d);
    seen.expired = await page.evaluate(() => {
      const host = document.querySelector("#worker-view");
      if (typeof sweepParkedPages !== "function" || typeof CHAT_PARK === "undefined") return null;
      const before = host.__parked.size;
      sweepParkedPages(host, Date.now() + CHAT_PARK.ttlMs + 1);
      return { before, after: host.__parked.size, ttlMs: CHAT_PARK.ttlMs };
    });
    ok(
      "a_conversation_kept_aside_goes_when_its_tab_closes_or_its_time_runs_out: closing a conversation's tab lets its kept page go at once, and a kept page older than `CHAT_PARK.ttlMs` (ten minutes, Dots' 600 s) is let go by the sweep",
      seen.beforeClose.ids?.includes(d) === true && seen.closed.holds === false &&
        seen.expired !== null && seen.expired.before > 0 && seen.expired.after === 0 && seen.expired.ttlMs === 600_000,
      JSON.stringify({ beforeClose: seen.beforeClose, closed: seen.closed, expired: seen.expired }),
    );
    ok("parked pages: no page errors", faults.length === 0, faults.slice(0, 3).join("\n"));
  } finally {
    await page.close();
  }
}

/* The conversation list in front: shown, and not in a page set aside behind
 * it (t-55369 — a page set aside stands in the document under the one in
 * front, hidden from assistive technology, so `checkVisibility` alone would
 * find it). */
const FRONT_LIST = () => [...document.querySelectorAll(".helper-turns")]
  .find((one) => one.checkVisibility() && !one.closest('[aria-hidden="true"]')) ?? null;

/* 2단계(t-55369): 돌아온 대화가 아직 37 ms(CPU×4 127 ms, WebKit 86 ms)였던
 * 것은 떼어 둔 화면을 다시 붙이는 순간 요소 약 2,800개의 스타일과 배치를
 * 처음부터 계산했기 때문이다 — 문서에서 뗀 요소는 계산된 스타일을 잃는다.
 * 방금 떠난 대화는 이제 문서 안에, 앞 화면 뒤에 선 채로 남는다: 돌아올 때
 * 문서에 다시 들어가는 노드가 없다. 그 전에 떠난 것은 1단계처럼 문서 밖에서
 * 기다린다(렌더된 화면 한 장은 약 11 MB, 뗀 화면은 약 2.9 MB). */
async function testPageBehind(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await standSwitcher(page);
    const seen = await page.evaluate(async ({ a, b, c, list }) => {
      const front = new Function(`return (${list})()`);
      const tabA = await window.__SWITCH__.open(a);
      const listA = front();
      const tabB = await window.__SWITCH__.open(b);
      const listB = front();
      const hostOf = (one) => one?.closest(".worker-view") ?? null;
      const hostA = hostOf(listA);
      const hostB = hostOf(listB);
      const box = hostB.getBoundingClientRect();
      const hit = document.elementFromPoint(box.left + box.width / 2, box.top + box.height / 2);
      const behind = {
        connected: listA.isConnected,
        ownHost: hostA !== hostB,
        sameLeaf: hostA?.parentElement === hostB?.parentElement,
        hidden: hostA?.getAttribute("aria-hidden") ?? null,
        frontId: hostB.id,
        frontHit: hostB.contains(hit),
        asideHit: hostA?.contains(hit) ?? null,
      };
      // Back to A: whatever moves in the leaf, no node that holds A's rows goes
      // into it — the page never left.
      const leaf = hostB.parentElement;
      const put = [];
      const watch = new MutationObserver((records) => {
        for (const record of records) for (const node of record.addedNodes) if (node.contains?.(listA)) put.push(node.className ?? node.nodeName);
      });
      watch.observe(leaf, { childList: true, subtree: true });
      const backA = await window.__SWITCH__.hop(tabA);
      await new Promise((done) => setTimeout(done, 0));
      watch.disconnect();
      const back = { put, frontId: hostOf(front())?.id ?? null, sameList: front() === listA, rowsBuilt: backA.rowsBuilt, bHidden: hostB.getAttribute("aria-hidden") };
      // Two kept aside: the one left last behind the front, the one before it
      // out of the document.
      await window.__SWITCH__.hop(tabB);
      const tabC = await window.__SWITCH__.open(c);
      const tiers = { b: listB.isConnected, a: listA.isConnected, kept: document.querySelector("#worker-view")?.__parked?.size ?? null };
      const fromOut = await window.__SWITCH__.hop(tabA);
      const fromBehind = await window.__SWITCH__.hop(tabC);
      return { behind, back, tiers, fromOut: { rowsBuilt: fromOut.rowsBuilt, rows: fromOut.rows }, fromBehind: { rowsBuilt: fromBehind.rowsBuilt, rows: fromBehind.rows } };
    }, { a: conversationFixture({ blocks: 20 }), b: conversationFixture({ blocks: 10 }), c: conversationFixture({ blocks: 5 }), list: FRONT_LIST.toString() });
    ok(
      "a_conversation_looked_away_from_stays_in_the_document_behind_the_one_in_front: the conversation left a moment ago keeps its own page in the same leaf, under the one in front — its list still in the document, its page hidden from assistive technology and under no pointer — and the window's `#worker-view` is the page in front",
      seen.behind.connected && seen.behind.ownHost && seen.behind.sameLeaf && seen.behind.hidden === "true" &&
        seen.behind.frontId === "worker-view" && seen.behind.frontHit && seen.behind.asideHit === false,
      JSON.stringify(seen.behind),
    );
    ok(
      "coming_back_puts_no_node_back_into_the_document: returning to the conversation left last brings its own list to the front with no node holding its rows put into the leaf (nothing to style or lay out again), the one it covered set aside in turn",
      seen.back.put.length === 0 && seen.back.sameList && seen.back.frontId === "worker-view" && seen.back.rowsBuilt === 0 && seen.back.bHidden === "true",
      JSON.stringify(seen.back),
    );
    ok(
      "of_two_conversations_kept_aside_the_older_waits_out_of_the_document: with a third conversation opened the leaf still keeps two aside — the one left last stays behind the front, the one before it waits out of the document — and both come back with every row they stood with",
      seen.tiers.b === true && seen.tiers.a === false && seen.tiers.kept === 2 &&
        seen.fromOut.rowsBuilt === 0 && seen.fromOut.rows > 300 && seen.fromBehind.rowsBuilt === 0 && seen.fromBehind.rows > 0,
      JSON.stringify({ tiers: seen.tiers, fromOut: seen.fromOut, fromBehind: seen.fromBehind }),
    );
    ok("page behind: no page errors", faults.length === 0, faults.slice(0, 3).join("\n"));
  } finally {
    await page.close();
  }
}

/* The window's share of the keeping: three leaves side by side, each with a
 * conversation set aside under another. Only `CHAT_PARK.leaves` (2) of them —
 * the ones that set theirs aside last — keep it standing behind the front; the
 * first leaf's waits out of the document. When the window must give memory
 * back (`sweepParkedPages(…, { tight: true })`) every page behind goes out of
 * the document, still kept, and comes back with every row it stood with. */
async function testKeepingAcrossLeaves(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await standSwitcher(page);
    const seen = await page.evaluate(async ({ one }) => {
      const leaves = [focusedPane];
      for (let k = 0; k < 2; k += 1) {
        const group = nextGroupId;
        nextGroupId += 1;
        setStageTree(splitStageLeaf(stageTree(), leaves.at(-1), group, "horizontal", "second"));
        leaves.push(group);
      }
      const listIn = (leaf) => groups.get(leaf)?.workerView?.querySelector(".helper-turns") ?? null;
      const first = [];
      for (const leaf of leaves) {
        focusedPane = leaf;
        const tab = await window.__SWITCH__.open(one);
        first.push({ tab, leaf, list: listIn(leaf), opened: tabs.find((held) => held.id === tab)?.pane });
        await window.__SWITCH__.open(one);
      }
      const where = () => first.map((held) => ({
        behind: held.list?.isConnected === true,
        kept: groups.get(held.leaf)?.workerView?.__parked?.size ?? null,
        rows: held.list?.querySelectorAll(":scope > [data-turn]").length ?? 0,
      }));
      const capped = where();
      if (typeof sweepParkedPages === "function") sweepParkedPages(groups.get(leaves[2]).workerView, Date.now(), { tight: true });
      const tight = where();
      focusedPane = leaves[2];
      await window.__SWITCH__.hop(first[2].tab);
      return {
        leaves: typeof CHAT_PARK === "undefined" ? null : CHAT_PARK.leaves,
        opened: first.map((held) => held.opened === held.leaf),
        capped,
        tight,
        back: { same: listIn(leaves[2]) === first[2].list, rows: first[2].list?.querySelectorAll(":scope > [data-turn]").length ?? 0, before: capped[2].rows },
      };
    }, { one: conversationFixture({ blocks: 2 }) });
    ok(
      "a_window_keeps_pages_behind_in_its_last_two_leaves: with a conversation set aside in each of three leaves, the two leaves that set theirs aside last (`CHAT_PARK.leaves`) keep it standing behind the front and the first leaf's waits out of the document — each leaf still keeping its one",
      seen.leaves === 2 && seen.opened.every(Boolean) &&
        JSON.stringify(seen.capped.map((one) => [one.behind, one.kept])) === JSON.stringify([[false, 1], [true, 1], [true, 1]]),
      JSON.stringify({ leaves: seen.leaves, opened: seen.opened, capped: seen.capped }),
    );
    ok(
      "a_tight_sweep_takes_every_page_behind_out_of_the_document: `sweepParkedPages(host, now, { tight: true })` — the window giving memory back — leaves no page standing behind in any leaf, keeps every one of them, and the one asked for again comes back with its own rows",
      seen.tight.every((one) => one.behind === false && one.kept === 1) && seen.back.same && seen.back.rows === seen.back.before && seen.back.rows > 0,
      JSON.stringify({ tight: seen.tight, back: seen.back }),
    );
    ok("keeping across leaves: no page errors", faults.length === 0, faults.slice(0, 3).join("\n"));
  } finally {
    await page.close();
  }
}

/* A page behind the front is in the document and nowhere else a person or a
 * program meets it: not in the accessibility tree a screen reader reads, not
 * answering to the window's own name for the page in front (`#worker-view`,
 * `docHost`), and lending no id a second time. Its words are its own, so
 * the tree is read for them. */
async function testBehindUnseen(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await standSwitcher(page);
    const said = (word) => [
      { role: "user", text: `${word} 부탁: 이 대화만의 낱말이다.`, at_ms: 1_790_000_000_000 },
      { role: "assistant", text: `${word} 답: 이 대화만의 낱말로 답한다.`, at_ms: 1_790_000_001_000 },
    ];
    const seen = await page.evaluate(async ({ a, b, list }) => {
      const front = new Function(`return (${list})()`);
      await window.__SWITCH__.open(a);
      const listA = front();
      await window.__SWITCH__.open(b);
      const listB = front();
      const ids = [...document.querySelectorAll("[id]")].map((node) => node.id);
      const view = docHost(focusedPane, "worker");
      return {
        behindInDocument: listA?.isConnected === true,
        named: document.querySelectorAll("#worker-view").length,
        namedFront: document.querySelector("#worker-view")?.contains(listB) === true,
        viewFront: view.contains(listB) && !view.contains(listA),
        duplicateIds: ids.filter((id, at) => ids.indexOf(id) !== at),
      };
    }, { a: said("갈매기"), b: said("두루미"), list: FRONT_LIST.toString() });
    const cdp = await page.context().newCDPSession(page);
    const { nodes } = await cdp.send("Accessibility.getFullAXTree");
    const read = nodes.filter((node) => !node.ignored).map((node) => `${node.name?.value ?? ""} ${node.value?.value ?? ""}`).join("\n");
    seen.readBehind = read.includes("갈매기");
    seen.readFront = read.includes("두루미");
    ok(
      "a_page_behind_is_not_read_named_or_doubled: the conversation standing behind the front is absent from the accessibility tree (its words unread while the front's are read), `#worker-view` and the leaf's view (`docHost`) are the page in front alone, and no id in the window stands twice",
      seen.behindInDocument && seen.readBehind === false && seen.readFront === true && seen.named === 1 && seen.namedFront && seen.viewFront &&
        seen.duplicateIds.length === 0,
      JSON.stringify(seen),
    );
    ok("page behind unseen: no page errors", faults.length === 0, faults.slice(0, 3).join("\n"));
  } finally {
    await page.close();
  }
}

/* The other two returns a person makes to a conversation (t-55369, the
 * coordinator 09:04: 「사람이 느끼는 흐름은 셋 다 같은 돌아오기」): a pane's
 * 「대화」 turned to the pane's screen and back, and a conversation tab left for
 * a terminal tab of the same leaf and back. Both used to hide the page
 * (`hidden`, display: none), which drops its styles and layout: back, every
 * element was styled and laid out again (a 400-turn pane's conversation 18.7 ms,
 * a conversation tab 28.1 ms). Now the page stands behind what is shown, as a
 * conversation set aside does, and comes back with no node put into the
 * document and its list where its reader left it. */
async function testBehindTheScreen(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await standSwitcher(page);
    const seen = await page.evaluate(async ({ a }) => {
      const settle = async () => {
        for (let beat = 0; beat < 3; beat += 1) await window.__PAINTED__();
      };
      const off = (node) => node.hidden || node.getAttribute("aria-hidden") === "true";
      const putting = (root, list) => {
        const put = [];
        const watch = new MutationObserver((records) => {
          for (const record of records) for (const node of record.addedNodes) if (node.contains?.(list)) put.push(node.className ?? node.nodeName);
        });
        watch.observe(root, { childList: true, subtree: true });
        return () => {
          watch.disconnect();
          return put;
        };
      };
      // A pane's conversation, turned to the screen and back.
      const rows = [];
      for (let at = 0; at < 200; at += 1) {
        rows.push({ role: "user", text: `부탁 ${at}`, at_ms: 1_790_000_000_000 + at * 4000 });
        rows.push({ role: "assistant", text: `답 ${at}: ${"긴 문장이 여러 줄로 이어진다. ".repeat(4)}`, at_ms: 1_790_000_002_000 + at * 4000 });
      }
      window.__ANSWER__.pane_log = (args) => ({ found: true, next: 1, turns: args.after === null || args.after === undefined ? rows : [], skipped: false, more: false, folded: false, model: "claude-opus-5" });
      const term = await openTermTab({ placement: "tab" });
      paneAgents.set(term, "claude");
      hookStates.set(term, "idle");
      setActiveTab(tabOfTerm(term).id);
      await setPaneChat(term, true, { wire: false });
      await pollHelperPages();
      await settle();
      const slot = document.querySelector(`.pane-slot[data-term="${term}"]`);
      const chat = slot.querySelector(".pane-chat");
      const list = chat.querySelector(".helper-turns");
      list.scrollTop = Math.round(list.scrollHeight / 2);
      await settle();
      const top = list.scrollTop;
      await setPaneChat(term, false);
      await settle();
      const pane = {
        screen: termViews.get(term)?.host.hidden === false,
        off: off(chat),
        hidden: chat.hidden,
        behind: chat.getAttribute("aria-hidden"),
        listIn: list.isConnected,
      };
      let done = putting(slot, list);
      await setPaneChat(term, true, { wire: false });
      await settle();
      Object.assign(pane, { put: done(), back: !off(chat) && chat.querySelector(".helper-turns") === list, top: list.scrollTop === top });
      // A conversation tab of a leaf, left for a terminal tab of the same leaf.
      const tab = await window.__SWITCH__.open(a);
      const view = docHost(focusedPane, "worker");
      const listA = view.querySelector(".helper-turns");
      listA.scrollTop = Math.round(listA.scrollHeight / 3);
      await settle();
      const topA = listA.scrollTop;
      setActiveTab(tabOfTerm(term).id);
      await settle();
      const leaf = { off: off(view), hidden: view.hidden, behind: view.getAttribute("aria-hidden"), listIn: listA.isConnected, front: view.id };
      done = putting(view.parentElement, listA);
      setActiveTab(tab);
      await settle();
      Object.assign(leaf, { put: done(), back: !off(view) && view.querySelector(".helper-turns") === listA, top: listA.scrollTop === topA });
      // Closed while it stands behind the terminal tab: the view lets the page
      // go and stands behind no longer.
      setActiveTab(tabOfTerm(term).id);
      await settle();
      closeTab(tab);
      await settle();
      leaf.closed = { behind: view.getAttribute("aria-hidden"), empty: view.querySelector(".helper-turns") === null, listIn: listA.isConnected };
      return { pane, leaf };
    }, { a: conversationFixture({ blocks: 20 }) });
    ok(
      "a_pane_conversation_turned_to_its_screen_stands_behind_it: a pane's 「대화」 turned to the pane's screen stays rendered behind the screen — not hidden, hidden from assistive technology, its list in the document — and 「대화」 again brings back the same list, where its reader left it, with no node holding its rows put into the slot",
      seen.pane.screen && seen.pane.off && seen.pane.hidden === false && seen.pane.behind === "true" && seen.pane.listIn &&
        seen.pane.put.length === 0 && seen.pane.back && seen.pane.top,
      JSON.stringify(seen.pane),
    );
    ok(
      "a_conversation_tab_left_for_a_terminal_tab_stands_behind_it: a conversation tab whose leaf turns to a terminal tab stays rendered behind it — not hidden, hidden from assistive technology, still the window's `#worker-view` — and comes back to the same list, where its reader left it, with no node holding its rows put into the leaf; closed while it stands behind, it lets its page go and stands behind no longer",
      seen.leaf.off && seen.leaf.hidden === false && seen.leaf.behind === "true" && seen.leaf.listIn && seen.leaf.front === "worker-view" &&
        seen.leaf.put.length === 0 && seen.leaf.back && seen.leaf.top &&
        seen.leaf.closed.behind === null && seen.leaf.closed.empty && seen.leaf.closed.listIn === false,
      JSON.stringify(seen.leaf),
    );
    ok("behind the screen: no page errors", faults.length === 0, faults.slice(0, 3).join("\n"));
  } finally {
    await page.close();
  }
}

/* The window counts every kind of page standing behind alike — a conversation
 * set aside, a leaf's view behind a terminal tab, a pane's conversation behind
 * the pane's screen, each leaf or pane one of `CHAT_PARK.leaves` (2): with one
 * of each, the one that went behind first hides (a view, as it did before
 * t-55369), and a tight sweep leaves none standing. */
async function testBehindAcrossKinds(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await standSwitcher(page);
    const seen = await page.evaluate(async ({ one }) => {
      const settle = async () => {
        for (let beat = 0; beat < 3; beat += 1) await window.__PAINTED__();
      };
      const leaves = [focusedPane];
      for (let k = 0; k < 2; k += 1) {
        const group = nextGroupId;
        nextGroupId += 1;
        setStageTree(splitStageLeaf(stageTree(), leaves.at(-1), group, "horizontal", "second"));
        leaves.push(group);
      }
      const state = (host) => (!host ? null : host.isConnected === false ? "out" : host.hidden ? "hidden" : host.getAttribute("aria-hidden") === "true" ? "behind" : "shown");
      // Leaf 0: a conversation, then a terminal tab over it.
      focusedPane = leaves[0];
      await window.__SWITCH__.open(one);
      const view0 = docHost(leaves[0], "worker");
      const shell0 = await openTermTab({ placement: "tab" });
      setActiveTab(tabOfTerm(shell0).id);
      await settle();
      // Leaf 1: a pane's conversation turned to its screen.
      focusedPane = leaves[1];
      const said = [];
      for (let at = 0; at < 20; at += 1) {
        said.push({ role: "user", text: `부탁 ${at}`, at_ms: 1_790_000_000_000 + at * 4000 });
        said.push({ role: "assistant", text: `답 ${at}`, at_ms: 1_790_000_002_000 + at * 4000 });
      }
      window.__ANSWER__.pane_log = (args) => ({ found: true, next: 1, turns: args.after === null || args.after === undefined ? said : [], skipped: false, more: false, folded: false, model: "claude-opus-5" });
      const term = await openTermTab({ placement: "tab" });
      paneAgents.set(term, "claude");
      hookStates.set(term, "idle");
      setActiveTab(tabOfTerm(term).id);
      await setPaneChat(term, true, { wire: false });
      await pollHelperPages();
      await settle();
      const chat1 = document.querySelector(`.pane-slot[data-term="${term}"] .pane-chat`);
      await setPaneChat(term, false);
      await settle();
      // Leaf 2: a conversation set aside under another.
      focusedPane = leaves[2];
      await window.__SWITCH__.open(one);
      const aside2 = docHost(leaves[2], "worker");
      await window.__SWITCH__.open(one);
      await settle();
      const capped = { view0: state(view0), chat1: state(chat1), aside2: state(aside2) };
      if (typeof sweepParkedPages === "function") sweepParkedPages(docHost(leaves[2], "worker"), Date.now(), { tight: true });
      await settle();
      const tight = { view0: state(view0), chat1: state(chat1), aside2: state(aside2) };
      return { leaves: typeof CHAT_PARK === "undefined" ? null : CHAT_PARK.leaves, capped, tight };
    }, { one: conversationFixture({ blocks: 2 }) });
    ok(
      "the_window_counts_every_page_behind_alike: a leaf's conversation behind a terminal tab, a pane's conversation behind its screen and a conversation set aside under another stand behind as long as the window keeps two (`CHAT_PARK.leaves`) — the one that went behind first hides — and a tight sweep leaves none standing: views and panes hidden, the page set aside out of the document",
      seen.leaves === 2 &&
        JSON.stringify(seen.capped) === JSON.stringify({ view0: "hidden", chat1: "behind", aside2: "behind" }) &&
        JSON.stringify(seen.tight) === JSON.stringify({ view0: "hidden", chat1: "hidden", aside2: "out" }),
      JSON.stringify(seen),
    );
    ok("behind across kinds: no page errors", faults.length === 0, faults.slice(0, 3).join("\n"));
  } finally {
    await page.close();
  }
}

/* A page behind the front is in the document, so its controls could take the
 * keyboard's focus: one pressed on before the switch, a `focus()` a late answer
 * asks for, a Tab from the page in front. None of them stays there — the focus
 * goes on to the next control past the page (or back before it), as if the
 * page were not there. */
async function testFocusPassesAside(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await standSwitcher(page);
    const tabs = await page.evaluate(
      async ({ a, b }) => [await window.__SWITCH__.open(a), await window.__SWITCH__.open(b)],
      { a: conversationFixture({ blocks: 2 }), b: conversationFixture({ blocks: 2 }) },
    );
    // A in front, its composer focused — then B comes over it.
    await page.evaluate((tab) => window.__SWITCH__.hop(tab), tabs[0]);
    await page.focus("#worker-view .worker-composer-box");
    const seen = {};
    seen.before = await page.evaluate(() => document.activeElement?.className ?? null);
    await page.evaluate((tab) => window.__SWITCH__.hop(tab), tabs[1]);
    const aside = () => document.activeElement?.closest?.('.worker-view[aria-hidden="true"]') !== null && document.activeElement?.closest?.('.worker-view[aria-hidden="true"]') !== undefined;
    seen.switched = await page.evaluate(`(${aside})()`);
    // A late `focus()` on the page behind.
    seen.asked = await page.evaluate(`(() => {
      const behind = document.querySelector('.worker-view[aria-hidden="true"] .worker-composer-box');
      if (!behind) return { behind: false };
      behind.focus();
      return { behind: true, stayed: (${aside})() };
    })()`);
    // Tab from the last control of the page in front.
    seen.tabbed = await page.evaluate(`(() => {
      const stops = [...document.querySelectorAll("#worker-view " + "button, textarea, summary, [tabindex]:not([tabindex='-1']), a[href]")].filter((one) => one.getClientRects().length > 0);
      const last = stops.at(-1);
      last?.focus();
      return { focused: document.activeElement === last };
    })()`);
    await page.keyboard.press("Tab");
    seen.afterTab = await page.evaluate(`(${aside})()`);
    await page.keyboard.press("Shift+Tab");
    seen.afterBack = await page.evaluate(`(${aside})()`);
    ok(
      "focus_never_stays_in_a_conversation_set_aside: the focus the conversation held when another came over it, a `focus()` asked on its page behind the front, and a Tab or a Shift+Tab across it never leave the keyboard on the page set aside",
      seen.before === "worker-composer-box" && seen.switched === false &&
        (seen.asked.behind === false || seen.asked.stayed === false) && seen.tabbed.focused && seen.afterTab === false && seen.afterBack === false,
      JSON.stringify(seen),
    );
    ok("focus passes aside: no page errors", faults.length === 0, faults.slice(0, 3).join("\n"));
  } finally {
    await page.close();
  }
}

/* A conversation looked away from before its code blocks took their colours:
 * the colouring meets those blocks while the page is out of the document and
 * lets them go, so a page that comes back as it stood must put them in line
 * again — a page built again would have. The page's idle slices are held (a
 * stand-in `requestIdleCallback`) so the order is the one a quick press meets:
 * opened, left before an idle moment, coloured while aside, come back to. The
 * blocks' code is its own (`aside_`), so no colours are remembered for it. */
async function testColoursAfterAside(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await standSwitcher(page);
    const fenced = JSON.parse(JSON.stringify(conversationFixture({ blocks: 2 })).replaceAll("paint_", "aside_"));
    const seen = await page.evaluate(async ({ fenced, other }) => {
      const list = () => [...document.querySelectorAll(".helper-turns")].find((one) => one.checkVisibility() && !one.closest('[aria-hidden="true"]')) ?? null;
      const fences = () => [...(list()?.querySelectorAll("pre.md-block") ?? [])];
      const read = () => ({ fences: fences().length, coloured: fences().filter((pre) => pre.querySelector("[class^='chat-code-']") !== null).length });
      const idle = [];
      const held = window.requestIdleCallback;
      window.requestIdleCallback = (slice) => idle.push(slice);
      const runIdle = () => {
        for (let beat = 0; beat < 400 && idle.length > 0; beat += 1) idle.shift()({ didTimeout: false, timeRemaining: () => 50 });
      };
      try {
        const aside = await window.__SWITCH__.open(fenced);
        // The page as it stands once its shelf has judged the rows it opened
        // with (a frame and the watcher's answer): rows far from the view give
        // up their bodies, fences and all, whether or not the page is looked
        // away from after.
        await window.__PAINTED__();
        await new Promise((done) => setTimeout(done, 0));
        await window.__PAINTED__();
        const opened = read();
        await window.__SWITCH__.open(other);
        runIdle();
        await window.__SWITCH__.hop(aside);
        runIdle();
        return { opened, back: read() };
      } finally {
        window.requestIdleCallback = held;
      }
    }, { fenced, other: conversationFixture({ blocks: 1 }) });
    ok(
      "a_code_block_left_uncoloured_while_its_page_was_aside_takes_its_colours_when_it_comes_back: a conversation left before an idle moment coloured its code blocks, and coloured by the slices that ran while it was aside, comes back with every block it stands with coloured",
      seen.opened.fences > 0 && seen.opened.coloured < seen.opened.fences &&
        seen.back.fences === seen.opened.fences && seen.back.coloured === seen.back.fences,
      JSON.stringify(seen),
    );
    ok("colours after aside: no page errors", faults.length === 0, faults.slice(0, 3).join("\n"));
  } finally {
    await page.close();
  }
}

/* A Claude pane whose 「대화」 hands its conversation to a wire that takes
 * `standMs` to stand (or is refused after it), what the window pasted into the
 * pane and what it sent down the wire written down as it happens. */
async function standHandover(page, { refuse, standMs }) {
  return page.evaluate(async ({ refuse, standMs }) => {
    const hand = { paste: [], wire: [], refuse, standMs };
    window.__HAND__ = hand;
    const rows = [];
    for (let at = 0; at < 40; at += 1) {
      rows.push({ role: "user", text: `부탁 ${at}`, at_ms: 1_790_000_000_000 + at * 4000 });
      rows.push({ role: "assistant", text: `답 ${at}: ${"긴 문장이 여러 줄로 이어진다. ".repeat(6)}`, at_ms: 1_790_000_002_000 + at * 4000 });
    }
    window.__ANSWER__.pane_log = (args) => ({
      found: true, next: 1, turns: args.after === null || args.after === undefined ? rows : [], skipped: false, more: false, folded: false, model: "claude-opus-5",
    });
    window.__ANSWER__.term_paste = (args) => {
      hand.paste.push(args.text);
      return null;
    };
    window.__ANSWER__.term_key = () => null;
    let status = "idle";
    window.__ANSWER__.wire_start = (args) => new Promise((done, fail) => setTimeout(() => (hand.refuse
      ? fail(new Error("the wire was refused in the test"))
      : done({ id: 77, agent: args.agent, protocol: "claude-stream", version: "2.1.280", model: null, session: args.resume })), hand.standMs));
    // A sent message turns the session to work and back, each said on `wire:update`
    // as the backend says it — the queue sends its next message only after it saw
    // the turn start (`settleComposerQueue`).
    const told = (id) => {
      for (const heard of window.__LISTENERS__["wire:update"] ?? []) heard({ payload: { id } });
    };
    window.__ANSWER__.wire_send = (args) => {
      hand.wire.push(args.text);
      setTimeout(() => {
        status = "working";
        told(args.id);
      }, 20);
      setTimeout(() => {
        status = "idle";
        told(args.id);
      }, 160);
      return null;
    };
    window.__ANSWER__.wire_log = () => ({
      found: true, skipped: false, next: 0, turns: [], status, asks: [], live: [], agent: "claude",
      protocol: "claude-stream", models: [], modes: [], commands: [], version: "2.1.280",
    });
    window.__ANSWER__.wire_stop = () => null;
    const term = await openTermTab({ placement: "tab" });
    paneAgents.set(term, "claude");
    hookStates.set(term, "idle");
    paneSessions.set(term, { session: { key: "session_id", id: "s-handover" }, resumable: true });
    hand.term = term;
    setActiveTab(tabOfTerm(term).id);
    paintViewToggle();
    return term;
  }, { refuse, standMs });
}

/* The conversation list on screen and the row its reader reads, by its words. */
const READING = () => {
  const list = [...document.querySelectorAll(".helper-turns")].find((one) => one.checkVisibility() && !one.closest('[aria-hidden="true"]')) ?? null;
  if (!list) return null;
  const top = list.getBoundingClientRect().top;
  const row = [...list.querySelectorAll(":scope > [data-turn]:not(.is-user)")].find((one) => one.getBoundingClientRect().bottom > top);
  return row ? { words: row.textContent.trim().slice(0, 40), offset: row.getBoundingClientRect().top - top, wire: Boolean(list.closest("#worker-view")) } : null;
};

async function testPaneHandover(browser, origin, ok, { refuse }) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  const standMs = refuse ? 800 : 1500;
  try {
    const term = await standHandover(page, { refuse, standMs });
    const seen = {};
    // The press, and two frames after it.
    seen.early = await page.evaluate(async (term) => {
      const from = performance.now();
      el("view-toggle-chat").click();
      await window.__PAINTED__();
      const list = document.querySelector(`.pane-slot[data-term="${term}"] .pane-chat .helper-turns`);
      return {
        ms: performance.now() - from,
        rows: list && list.checkVisibility() ? list.querySelectorAll(":scope > [data-turn]").length : 0,
        composer: Boolean(document.querySelector(`.pane-slot[data-term="${term}"] .pane-chat .worker-composer-box`)),
      };
    }, term);
    // While the wire stands up: the words the person sends, then a wheel up.
    const words = refuse ? ["셋"] : ["하나", "둘"];
    const box = `.pane-slot[data-term="${term}"] .pane-chat .worker-composer-box`;
    if (seen.early.composer) {
      for (const said of words) {
        await page.focus(box);
        await page.keyboard.type(said);
        await page.keyboard.press("Enter");
        await page.evaluate(() => window.__PAINTED__());
      }
      const at = await page.evaluate(() => {
        const list = [...document.querySelectorAll(".helper-turns")].find((one) => one.checkVisibility() && !one.closest('[aria-hidden="true"]'));
        const rect = list.getBoundingClientRect();
        return { x: rect.left + rect.width / 2, y: rect.top + rect.height / 3 };
      });
      await page.mouse.move(at.x, at.y);
      await page.mouse.wheel(0, -700);
      await page.waitForTimeout(250);
    }
    seen.place = await page.evaluate(READING);
    seen.whileHanding = await page.evaluate((term) => ({
      paste: [...window.__HAND__.paste],
      wire: [...window.__HAND__.wire],
      // The head's running chip says it under the pointer (t-81525 C5).
      says: document.querySelector(`.pane-slot[data-term="${term}"] .pane-chat .chat-head .chat-run`)?.dataset.tip ===
        t("worker.handingOver", "실시간 대화로 잇는 중입니다 — 그동안 보낸 글은 이어진 뒤에 갑니다."),
    }), term);
    // The wire stands (or is refused), and every word that waited has gone.
    await page.waitForTimeout(standMs + 200);
    await page.waitForFunction((count) => window.__HAND__.wire.length + window.__HAND__.paste.length >= count, words.length, { timeout: 4000 }).catch(() => null);
    await page.waitForTimeout(300);
    seen.after = await page.evaluate(() => ({
      active: activeTabId,
      paste: [...window.__HAND__.paste],
      wire: [...window.__HAND__.wire],
      refusedSays: Boolean(document.querySelector('.pane-chat-notice[data-says="worker.wireRefused"]')),
    }));
    seen.returned = await page.evaluate(READING);
    if (!refuse) {
      ok(
        "pressing_conversation_shows_the_transcript_before_the_wire_stands: a Claude pane's 「대화」 stands its transcript view, rows and composer, within two frames of the press — the wire, which takes 1.5 s to stand here, takes over behind it and says so",
        seen.early.rows > 0 && seen.early.composer && seen.whileHanding.says && seen.after.active === "wire:77",
        JSON.stringify({ early: seen.early, says: seen.whileHanding.says, active: seen.after.active }),
      );
      ok(
        "words_sent_while_the_wire_takes_over_go_once_in_order: what the person sends while the hand-over is under way waits — nothing reaches the pane — and goes down the wire once the wire stands, each message once and in the order it was sent",
        JSON.stringify(seen.whileHanding.wire) === "[]" && JSON.stringify(seen.whileHanding.paste) === "[]" &&
          JSON.stringify(seen.after.wire) === JSON.stringify(words) && JSON.stringify(seen.after.paste) === "[]",
        JSON.stringify({ whileHanding: seen.whileHanding, after: seen.after }),
      );
      ok(
        "the_live_session_opens_on_the_row_the_reader_left: the wire's page that replaces the transcript view stands on the row its reader was reading, at the same height (within two pixels)",
        seen.place !== null && seen.returned !== null && seen.returned.wire && seen.place.words === seen.returned.words &&
          Math.abs(seen.place.offset - seen.returned.offset) <= 2,
        JSON.stringify({ place: seen.place, returned: seen.returned }),
      );
    } else {
      ok(
        "a_refused_hand_over_sends_the_waiting_words_to_the_pane_once: when the wire is refused the transcript view stays and says why, and the words that waited go to the pane — once each, never down a wire",
        seen.early.rows > 0 && JSON.stringify(seen.whileHanding.paste) === "[]" &&
          JSON.stringify(seen.after.paste) === JSON.stringify(words) && JSON.stringify(seen.after.wire) === "[]" &&
          seen.after.refusedSays && seen.after.active !== "wire:77",
        JSON.stringify({ early: seen.early, whileHanding: seen.whileHanding, after: seen.after }),
      );
    }
    ok(`pane hand-over (${refuse ? "refused" : "stands"}): no page errors`, faults.length === 0, faults.slice(0, 3).join("\n"));
  } finally {
    await page.close();
  }
}

/* A page that stands — a pane's 「대화」 — is laid out by its first frame, and
 * once: no size is read off it in the press that builds it nor in the task that
 * brings its turns (each such read laid the new page out once more; six of them
 * stood in one press, t-55369), the frame placing it at its foot by itself. And
 * a page built after another starts with the room its dock last took, so the
 * dock's first measure has nothing to rewrite (a new value on the list restyles
 * every row it holds). */
async function testPressReadsNoSize(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    const term = await standHandover(page, { refuse: false, standMs: 5000 });
    const seen = await page.evaluate(async (term) => {
      const reads = [];
      const counting = { on: false };
      const onPage = (node) => node instanceof Element && node.closest(".pane-chat") !== null;
      const spy = (proto, name, kind) => {
        const held = Object.getOwnPropertyDescriptor(proto, name);
        Object.defineProperty(proto, name, {
          ...held,
          [kind](...args) {
            if (counting.on && onPage(this)) reads.push(`${String(this.className).split(" ")[0] || this.tagName}.${name}`);
            return held[kind].apply(this, args);
          },
        });
        return () => Object.defineProperty(proto, name, held);
      };
      const undo = [
        spy(Element.prototype, "getBoundingClientRect", "value"),
        spy(Element.prototype, "getClientRects", "value"),
        spy(Element.prototype, "clientHeight", "get"),
        spy(Element.prototype, "clientWidth", "get"),
        spy(Element.prototype, "scrollHeight", "get"),
        spy(Element.prototype, "scrollTop", "get"),
        spy(HTMLElement.prototype, "offsetHeight", "get"),
        spy(HTMLElement.prototype, "offsetWidth", "get"),
      ];
      const list = () => document.querySelector(`.pane-slot[data-term="${term}"] .pane-chat .helper-turns`);
      // Counting stops at the start of the first frame that has the turns to
      // draw: a frame's own work — its callbacks, its layout, the watches it
      // wakes — is the one layout the page is owed.
      const stood = new Promise((done) => {
        const look = () => requestAnimationFrame(() => {
          if (list()?.querySelector(":scope > [data-turn]")) {
            counting.on = false;
            done();
          } else look();
        });
        look();
      });
      counting.on = true;
      el("view-toggle-chat").click();
      await stood;
      for (const one of undo) one();
      await window.__PAINTED__();
      await window.__PAINTED__();
      const held = list();
      const dock = document.querySelector(`.pane-slot[data-term="${term}"] .pane-chat .chat-dock`);
      return {
        reads,
        rows: held?.querySelectorAll(":scope > [data-turn]").length ?? 0,
        gap: held ? Math.round(held.scrollHeight - held.scrollTop - held.clientHeight) : null,
        dock: dock ? `${dock.offsetHeight}px` : null,
      };
    }, term);
    // A conversation opened after it: the room its readers wear the moment its
    // dock is first watched, before any frame measures that dock.
    seen.next = await page.evaluate(async (history) => {
      const watch = watchChatDock;
      let worn = null;
      window.watchChatDock = (host, dock) => {
        const made = watch(host, dock);
        worn ??= host.querySelector(":scope > .helper-body > .helper-turns")?.style.getPropertyValue("--chat-dock-h") ?? null;
        return made;
      };
      try {
        window.__ANSWER__.wire_start = (args) => ({ id: 78, agent: args.agent, protocol: "claude-stream", version: "2.1.280", model: null, session: null });
        await openWirePage("claude", "/tmp/zerocode-window-test", { history });
      } finally {
        window.watchChatDock = watch;
      }
      return worn;
    }, [{ role: "user", text: "다음 대화", at_ms: 1_790_000_000_000 }, { role: "assistant", text: "다음 답", at_ms: 1_790_000_001_000 }]);
    ok(
      "a_page_that_stands_reads_no_size_before_its_first_frame: from a Claude pane's 「대화」 press until the first frame that draws its turns, nothing reads a box off the new page — the press builds, the turns arrive, the frame lays it out once and stands it at its foot — and the next page built starts with the room its dock last took",
      seen.rows > 0 && seen.reads.length === 0 && seen.gap !== null && seen.gap <= 1 && seen.dock !== null && seen.next === seen.dock,
      JSON.stringify({ reads: seen.reads.slice(0, 12), count: seen.reads.length, rows: seen.rows, gap: seen.gap, dock: seen.dock, next: seen.next }),
    );
    ok("press reads no size: no page errors", faults.length === 0, faults.slice(0, 3).join("\n"));
  } finally {
    await page.close();
  }
}

async function testLateFirstRead(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.evaluate(() => {
      const rows = [{ role: "user", text: "늦은 읽기" }, { role: "assistant", text: "읽혔다." }];
      window.__LATE__ = 0;
      window.__ANSWER__.pane_log = (args) => new Promise((done) => setTimeout(() => done({
        found: true, next: 1, turns: args.after === null || args.after === undefined ? rows : [], skipped: false, more: false, folded: false,
      }), window.__LATE__));
    });
    // A pane's 「대화」 whose first read takes `ms`: what the reading line does
    // two frames in, at 450 ms, and once the turns are there.
    const read = (ms) => page.evaluate(async (ms) => {
      window.__LATE__ = ms;
      const notice = (term) => document.querySelector(`.pane-slot[data-term="${term}"] .pane-chat-notice[data-says="worker.readingTranscript"]`);
      const shown = (node) => Boolean(node) && node.checkVisibility({ visibilityProperty: true, opacityProperty: true });
      const term = await openTermTab({ placement: "tab" });
      paneAgents.set(term, "zo");
      hookStates.set(term, "idle");
      setActiveTab(tabOfTerm(term).id);
      const from = performance.now();
      void setPaneChat(term, true);
      await window.__PAINTED__();
      const early = { stands: Boolean(notice(term)), shown: shown(notice(term)) };
      await new Promise((done) => setTimeout(done, Math.max(0, 450 - (performance.now() - from))));
      const later = { stands: Boolean(notice(term)), shown: shown(notice(term)) };
      await new Promise((done) => setTimeout(done, ms + 150));
      await window.__PAINTED__();
      const done = { stands: Boolean(notice(term)), rows: document.querySelectorAll(`.pane-slot[data-term="${term}"] .helper-turns > [data-turn]`).length };
      return { early, later, done, token: getComputedStyle(document.documentElement).getPropertyValue("--chat-late-ms").trim() };
    }, ms);
    const slow = await read(900);
    // Reduced motion stills every animation (`reduced_motion_stills_the_whole_window`):
    // the wait must not be one.
    await page.emulateMedia({ reducedMotion: "reduce" });
    const stilled = await read(900);
    await page.emulateMedia({ reducedMotion: WINDOW_MOTION_REST });
    const quick = await read(0);
    const late = (one) => one.early.stands && !one.early.shown && one.later.stands && one.later.shown && !one.done.stands && one.done.rows === 2;
    ok(
      "a_late_first_read_says_it_is_reading_after_the_token_and_a_quick_one_never_does: while a pane's first read is out the page holds 「대화를 읽는 중…」 hidden for `--chat-late-ms` (200 ms) and shows it after, with reduced motion too, and takes it away when the turns arrive; a read that arrives at once leaves nothing standing",
      slow.token === "200ms" && late(slow) && late(stilled) && !quick.done.stands && quick.done.rows === 2,
      JSON.stringify({ slow, stilled, quick }),
    );
    ok("late first read: no page errors", faults.length === 0, faults.slice(0, 3).join("\n"));
  } finally {
    await page.close();
  }
}
