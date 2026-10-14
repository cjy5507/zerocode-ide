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
  await testColoursAfterAside(browser, origin, ok);
  await testPaneHandover(browser, origin, ok, { refuse: false });
  await testPaneHandover(browser, origin, ok, { refuse: true });
  await testLateFirstRead(browser, origin, ok);
}

/* The leaf's host and what it keeps aside, read without assuming the keeping
 * exists (a red tree has none: the checks fail at their assertions). */
const PARKED = () => {
  const host = document.querySelector("#worker-view") ?? [...document.querySelectorAll(".worker-view")].find((one) => one.checkVisibility()) ?? null;
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
      const list = [...document.querySelectorAll(".helper-turns")].find((one) => one.checkVisibility());
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
      const host = [...document.querySelectorAll(".worker-view")].find((one) => one.checkVisibility());
      return { size: host?.__parked?.size ?? null, holds: host?.__parked?.has(tab) ?? null };
    }, d);
    seen.expired = await page.evaluate(() => {
      const host = [...document.querySelectorAll(".worker-view")].find((one) => one.checkVisibility());
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
      const list = () => [...document.querySelectorAll(".helper-turns")].find((one) => one.checkVisibility()) ?? null;
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
  const list = [...document.querySelectorAll(".helper-turns")].find((one) => one.checkVisibility()) ?? null;
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
        const list = [...document.querySelectorAll(".helper-turns")].find((one) => one.checkVisibility());
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
      says: Boolean(document.querySelector(`.pane-slot[data-term="${term}"] .pane-chat .pane-chat-notice[data-says="worker.handingOver"]`)),
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
