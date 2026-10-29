/* 쉬는 판에서 Enter가 대기열에만 들어가고 안 나감 (t-84670, 사람 10-30 01:2x).
 *
 * v1.1.105 설치 재시작 직후 복원된 사람 판: 머리는 「● 대기 중」, 질문 카드는
 * 없음, 그런데 Enter를 치면 글이 「대기 메시지 1」에 들고 나가지 않았다 — ×로
 * 지우고 다시 쳐도 또 대기열. 턴이 아닌 붙잡음(판의 대화를 선으로 넘기는 중,
 * 답이 나간 질문 카드, 대기열이 보낸 글의 턴 시작)은 그것이 기다리는 일이
 * 일어나야만 풀렸고, 일어나지 않으면 아무것도 대기열을 다시 묻지 않았다.
 * 여기 장면마다: 붙잡힌 동안은 대기열이 왜 기다리는지 한 줄로 말하고, 붙잡음이
 * `COMPOSER_HOLD_MS`를 넘기면 창이 제 시계로 풀어 기다린 글을 판에 한 번 보내며,
 * 그 뒤의 Enter는 곧바로 나간다. 붙잡을 것이 없는 쉬는 판의 Enter는 처음부터
 * 곧바로 나간다.
 *
 * 시계: 긴 타이머(30 s 이상)는 잡아 두었다가 `Date.now`를 기한 너머로 옮긴 뒤
 * 부른다 — 1분을 실제로 기다리지 않고 창이 건 시계 그대로를 돌린다(옮긴 시계는
 * 장면 끝까지 그대로, 진짜 시계처럼 뒤로 가지 않는다). 대화는
 * 합성이다(사람의 기록이 아니다). */
import { openWindowTestPage } from "./window-boot.mjs";

/* The page's tools for one scene: what reached the pane, the long timers the
 * window armed, a hook to tell, the pane's composer to type into. */
const STAND = async () => {
  window.__HOLD__ = { sent: [], wires: 0, long: [] };
  const hold = window.__HOLD__;
  window.__ANSWER__.term_paste = (args) => (hold.sent.push(["paste", args.text]), null);
  window.__ANSWER__.term_key = (args) => (hold.sent.push(["key", args.press?.key ?? null]), null);
  window.__ANSWER__.answer_ask = () => null;
  window.__ANSWER__.answer_approval = () => null;
  const rows = [{ role: "user", text: "어제 하던 일" }, { role: "assistant", text: "끝났습니다." }];
  window.__ANSWER__.pane_log = (args) => ({
    found: true, next: 1, turns: args.after === null || args.after === undefined ? rows : [], skipped: false, more: false,
  });
  window.__ANSWER__.wire_stop = () => null;
  // The long clocks the window arms are kept, not run: the scene runs them
  // once it has moved the clock past them.
  const realTimeout = window.setTimeout.bind(window);
  window.setTimeout = (fn, ms, ...rest) => {
    if (Number(ms) >= 30_000) {
      hold.long.push({ fn, ms: Number(ms) });
      return -1 - hold.long.length;
    }
    return realTimeout(fn, ms, ...rest);
  };
  window.__TELL__ = (payload) => {
    for (const heard of window.__LISTENERS__["hook:agent"] ?? []) heard({ payload });
  };
  // The clock moves past the hold and stays there, as a real one does: the
  // page closes with the scene.
  window.__PASS__ = async () => {
    const realNow = Date.now;
    const ahead = (typeof COMPOSER_HOLD_MS === "number" ? COMPOSER_HOLD_MS : 60_000) + 1000;
    Date.now = () => realNow() + ahead;
    for (const timer of hold.long.splice(0)) timer.fn();
    for (let beat = 0; beat < 3; beat += 1) await window.__PAINTED__();
    await new Promise((done) => realTimeout(done, 900));
  };
  for (let tries = 0; tries < 200 && !installedAgents().some((row) => row.id === "claude" && row.wire_resumes); tries += 1) {
    await new Promise((done) => realTimeout(done, 25));
  }
  // The pane the restart brought back: its agent, its session, and the rest
  // it reported — a conversation stands on it by default (2차 B).
  const term = await openTermTab({ placement: "tab" });
  paneAgents.set(term, "claude");
  paneSessions.set(term, { agent: "claude", session: { key: "session_id", id: "s-restored" }, resumable: true });
  window.__TELL__({ term, state: "done", agent: "claude", session_boundary: true });
  setActiveTab(tabOfTerm(term).id);
  await setPaneChat(term, true, { wire: false });
  await pollHelperPages();
  for (let beat = 0; beat < 3; beat += 1) await window.__PAINTED__();
  window.__TERM__ = term;
  return term;
};

/* What the scene sees of the pane's conversation now. */
const LOOK = () => {
  const term = window.__TERM__;
  const chat = document.querySelector(`.pane-slot[data-term="${term}"] .pane-chat`);
  const composer = chat?.querySelector(".worker-composer");
  return {
    head: chat?.querySelector(".chat-head .chat-run .worker-state")?.textContent ?? null,
    card: Boolean(chat?.querySelector(".pane-chat-ask")),
    queue: [...(paneChats.get(term)?.run?.queue ?? [])],
    why: composer?.querySelector(".composer-queue-why")?.textContent ?? null,
    sent: window.__HOLD__.sent.map((one) => one.join(":")),
    wires: window.__HOLD__.wires,
    refused: chat?.querySelector('.pane-chat-notice[data-says="worker.wireRefused"]')?.textContent ?? null,
  };
};

/* Type `words` into the conversation's box and press Enter, as the person did. */
async function enter(page, words) {
  const box = page.locator(".pane-chat .worker-composer-box");
  await box.click();
  await page.keyboard.type(words);
  await page.keyboard.press("Enter");
  await page.waitForTimeout(250);
}

/* The × of the queue's first item. */
async function dropFirst(page) {
  await page.locator(".pane-chat .composer-queue-drop").first().click();
  await page.waitForTimeout(100);
}

/* Enter, ×, Enter again — both wait — then the hold's bound passes. */
async function heldThenPassed(page) {
  const seen = {};
  await enter(page, "첫 글");
  seen.first = await page.evaluate(LOOK);
  await dropFirst(page);
  await enter(page, "다시 친 글");
  seen.again = await page.evaluate(LOOK);
  await page.evaluate(() => window.__PASS__());
  await page.waitForFunction(() => window.__HOLD__.sent.some(([kind]) => kind === "key"), null, { timeout: 4000 }).catch(() => null);
  seen.passed = await page.evaluate(LOOK);
  await enter(page, "곧바로");
  await page.waitForFunction(() => window.__HOLD__.sent.filter(([kind]) => kind === "key").length >= 2, null, { timeout: 4000 }).catch(() => null);
  seen.after = await page.evaluate(LOOK);
  return seen;
}

const WENT = ["paste:다시 친 글", "key:Enter"];
const BOTH_WENT = [...WENT, "paste:곧바로", "key:Enter"];

export async function testComposerHold(browser, origin, ok) {
  await testNothingHolds(browser, origin, ok);
  await testHandOverReadStalls(browser, origin, ok);
  await testWireStartStalls(browser, origin, ok);
  await testAnsweredCardStands(browser, origin, ok);
  await testSentTurnNeverSeen(browser, origin, ok);
}

async function scene(browser, origin, ok, name, body) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.evaluate(STAND);
    await body(page);
    ok(`${name}: no page errors`, faults.length === 0, faults.slice(0, 3).join("\n"));
  } finally {
    await page.close();
  }
}

/* The resting pane with nothing holding its queue: Enter goes at once. */
async function testNothingHolds(browser, origin, ok) {
  await scene(browser, origin, ok, "nothing holds", async (page) => {
    const words = await page.evaluate(() => ({ idle: t("worker.idle", "대기 중") }));
    await enter(page, "곧바로");
    await page.waitForFunction(() => window.__HOLD__.sent.some(([kind]) => kind === "key"), null, { timeout: 4000 }).catch(() => null);
    const seen = await page.evaluate(LOOK);
    ok(
      "enter_on_a_resting_pane_with_nothing_holding_goes_at_once: a pane the restart brought back, its head 「대기 중」 and no card, sends the words straight down its own road — paste, then Enter — and its queue stays empty",
      seen.head === words.idle && !seen.card && seen.queue.length === 0 &&
        JSON.stringify(seen.sent) === JSON.stringify(["paste:곧바로", "key:Enter"]),
      JSON.stringify({ seen, words }),
    );
  });
}

/* ① The 「대화」 press hands the conversation to its wire, and the read before
 * the wire is asked never answers — the head rests, no card stands, and every
 * Enter went into the queue for good (the person's screen, 10-30 01:2x). */
async function testHandOverReadStalls(browser, origin, ok) {
  await scene(browser, origin, ok, "hand-over read stalls", async (page) => {
    await page.evaluate(async () => {
      window.__ANSWER__.pane_log = () => new Promise(() => {});
      window.__ANSWER__.wire_start = () => {
        window.__HOLD__.wires += 1;
        return new Promise(() => {});
      };
      el("view-toggle-chat").click();
      for (let beat = 0; beat < 3; beat += 1) await window.__PAINTED__();
    });
    const words = await page.evaluate(() => ({
      idle: t("worker.idle", "대기 중"),
      why: t("composer.queue.why.handing", "대화를 실시간으로 잇는 중 — 이어지면, 늦어도 {{s}}초 뒤 보냅니다", { s: 60 }),
      stalled: t("worker.handOverStalled", "대화 기록 읽기가 {{s}}초 안에 끝나지 않았습니다", { s: 60 }),
    }));
    const seen = await heldThenPassed(page);
    ok(
      "a_hand_over_whose_read_never_answers_holds_the_queue_under_a_resting_head: after the 「대화」 press, with the read before the wire never answering, the head says 「대기 중」, no card stands, Enter puts the words in the queue and nothing reaches the pane — and after × the next Enter does the same",
      seen.first.head === words.idle && !seen.first.card &&
        JSON.stringify(seen.first.queue) === JSON.stringify(["첫 글"]) && seen.first.sent.length === 0 &&
        JSON.stringify(seen.again.queue) === JSON.stringify(["다시 친 글"]) && seen.again.sent.length === 0,
      JSON.stringify({ seen, words }),
    );
    ok(
      "the_held_queue_says_why_in_one_line: while the hand-over holds it, the queue says under its count that the conversation is being continued live and that the words go in 60 s at the latest",
      seen.again.why === words.why,
      JSON.stringify({ why: seen.again.why, want: words.why }),
    );
    ok(
      "a_hand_over_stalled_past_its_hold_lets_the_queue_go_to_the_pane: once `COMPOSER_HOLD_MS` passes on the window's own clock the hand-over is given up without asking the wire, the page says why, the words that waited go to the pane once, and the next Enter goes straight",
      JSON.stringify(seen.passed.sent) === JSON.stringify(WENT) && seen.passed.queue.length === 0 &&
        seen.passed.wires === 0 && typeof seen.passed.refused === "string" && seen.passed.refused.includes(words.stalled) &&
        JSON.stringify(seen.after.sent) === JSON.stringify(BOTH_WENT) && seen.after.queue.length === 0,
      JSON.stringify({ passed: seen.passed, after: seen.after, words }),
    );
  });
}

/* ② The read answers, and the wire's start never does. */
async function testWireStartStalls(browser, origin, ok) {
  await scene(browser, origin, ok, "wire start stalls", async (page) => {
    await page.evaluate(async () => {
      window.__ANSWER__.wire_start = () => {
        window.__HOLD__.wires += 1;
        return new Promise(() => {});
      };
      el("view-toggle-chat").click();
      for (let beat = 0; beat < 3; beat += 1) await window.__PAINTED__();
    });
    const seen = await heldThenPassed(page);
    ok(
      "a_wire_that_never_stands_holds_the_queue_no_longer_than_its_hold: with the wire asked and never answering, Enter (and Enter again after ×) waits in the queue; past `COMPOSER_HOLD_MS` the window lets it go to the pane once, and the next Enter goes straight",
      JSON.stringify(seen.again.queue) === JSON.stringify(["다시 친 글"]) && seen.again.sent.length === 0 && seen.again.wires === 1 &&
        JSON.stringify(seen.passed.sent) === JSON.stringify(WENT) && seen.passed.queue.length === 0 &&
        JSON.stringify(seen.after.sent) === JSON.stringify(BOTH_WENT),
      JSON.stringify(seen),
    );
  });
}

/* ③ A question's card answered from the box, and no report ever says the
 * pane moved past it: the card stands with its answer gone. */
async function testAnsweredCardStands(browser, origin, ok) {
  await scene(browser, origin, ok, "answered card stands", async (page) => {
    await page.evaluate(async () => {
      window.__TELL__({ term: window.__TERM__, state: "waiting", agent: "claude",
        ask_prompt: { questions: [{ question: "어느 쪽으로 할까요?", header: "방향",
          options: [{ label: "A", description: "가" }, { label: "B", description: "나" }], multi_select: false }] } });
      for (let beat = 0; beat < 3; beat += 1) await window.__PAINTED__();
    });
    await enter(page, "B로 가 주세요");
    const words = await page.evaluate(() => ({
      why: t("composer.queue.why.answered", "질문에 보낸 답이 닿기를 기다립니다 — 늦어도 {{s}}초 뒤 보냅니다", { s: 60 }),
    }));
    const seen = await heldThenPassed(page);
    ok(
      "an_answered_card_no_report_moves_past_holds_the_queue_no_longer_than_its_hold: the card's answer went and the pane never said it moved — Enter (and Enter again after ×) waits and says why; past `COMPOSER_HOLD_MS` the window sends it to the pane once, and the next Enter goes straight",
      seen.again.card && JSON.stringify(seen.again.queue) === JSON.stringify(["다시 친 글"]) &&
        !seen.again.sent.some((one) => one.startsWith("paste:")) && seen.again.why === words.why &&
        JSON.stringify(seen.passed.sent.filter((one) => one !== "key:null")) === JSON.stringify(WENT) && seen.passed.queue.length === 0 &&
        JSON.stringify(seen.after.sent.filter((one) => one !== "key:null")) === JSON.stringify(BOTH_WENT),
      JSON.stringify({ seen, words }),
    );
  });
}

/* ④ The queue sent its first message and the turn it should start is never
 * seen: what still waited stayed there for good. */
async function testSentTurnNeverSeen(browser, origin, ok) {
  await scene(browser, origin, ok, "sent turn never seen", async (page) => {
    await page.evaluate(async () => {
      window.__TELL__({ term: window.__TERM__, state: "working", agent: "claude" });
      for (let beat = 0; beat < 2; beat += 1) await window.__PAINTED__();
    });
    await enter(page, "하나");
    await enter(page, "둘");
    await page.evaluate(async () => {
      window.__TELL__({ term: window.__TERM__, state: "done", agent: "claude" });
      for (let beat = 0; beat < 2; beat += 1) await window.__PAINTED__();
    });
    await page.waitForFunction(() => window.__HOLD__.sent.some(([kind]) => kind === "key"), null, { timeout: 4000 }).catch(() => null);
    await page.waitForTimeout(200);
    const words = await page.evaluate(() => ({
      why: t("composer.queue.why.started", "앞서 보낸 글의 턴이 시작되기를 기다립니다 — 늦어도 {{s}}초 뒤 보냅니다", { s: 60 }),
    }));
    const held = await page.evaluate(LOOK);
    await page.evaluate(() => window.__PASS__());
    await page.waitForFunction(() => window.__HOLD__.sent.filter(([kind]) => kind === "key").length >= 2, null, { timeout: 4000 }).catch(() => null);
    const passed = await page.evaluate(LOOK);
    ok(
      "a_queue_whose_sent_turn_is_never_seen_sends_the_rest_after_its_hold: the first held message went at the turn's end and no report ever said its turn started — the second waits and says why, and past `COMPOSER_HOLD_MS` the window sends it once",
      JSON.stringify(held.sent) === JSON.stringify(["paste:하나", "key:Enter"]) && JSON.stringify(held.queue) === JSON.stringify(["둘"]) &&
        held.why === words.why &&
        JSON.stringify(passed.sent) === JSON.stringify(["paste:하나", "key:Enter", "paste:둘", "key:Enter"]) && passed.queue.length === 0,
      JSON.stringify({ held, passed, words }),
    );
  });
}
