/* 쉬는 판에서 Enter가 대기열에만 들어가고 안 나감 (t-84670, 사람 10-30 01:2x).
 *
 * v1.1.105 설치 재시작 직후 복원된 사람 판: 머리는 「● 대기 중」, 질문 카드는
 * 없음, 그런데 Enter를 치면 글이 「대기 메시지 1」에 들고 나가지 않았다 — ×로
 * 지우고 다시 쳐도 또 대기열. 판의 대화를 선으로 넘기는 동안(「대화」 단추) 보낸
 * 글은 넘김이 끝나야 나가는데, 넘김이 끝나지 않으면 아무것도 대기열을 다시 묻지
 * 않았다. 여기 장면들:
 *   - 붙잡을 것이 없는 쉬는 판의 Enter는 곧바로 나간다.
 *   - 넘김의 기록 읽기는 큰 전사에서도 한 번의 상한 읽기다.
 *   - 넘김이 굳으면(읽기·선 시작이 답하지 않음) 대기열은 왜 기다리는지 한 줄로
 *     말하고, `COMPOSER_HOLD_MS`가 지나면 창이 제 시계로 기다린 글을 판에 한 번
 *     보내며, 그 뒤의 Enter는 곧바로 나간다. 선에는 마감(`deadlineMs`)이 실려
 *     늦게 선 선은 백엔드가 닫는다.
 *   - 답이 나간 질문 카드와 보낸 글의 턴 시작은 시계로 풀리지 않는다 — 카드 아래
 *     메뉴에 글이 떨어지는 길(t-81786 P0)을 다시 열지 않는다. 판이 움직이면 나간다.
 *
 * 시계: 긴 타이머(30 s 이상)는 잡아 두었다가 `Date.now`를 기한 너머로 옮긴 뒤
 * 부른다 — 1분을 실제로 기다리지 않고 창이 건 시계 그대로를 돌린다(옮긴 시계는
 * 장면 끝까지 그대로, 진짜 시계처럼 뒤로 가지 않는다). 대화는 합성이다(사람의
 * 기록이 아니다). */
import { openWindowTestPage } from "./window-boot.mjs";

/* The page's tools for one scene: what reached the pane, what the wire was
 * asked with, the long timers the window armed, a hook to tell. */
const STAND = async () => {
  window.__HOLD__ = { sent: [], wires: [], long: [] };
  const hold = window.__HOLD__;
  window.__ANSWER__.term_paste = (args) => (hold.sent.push(`paste:${args.text}`), null);
  window.__ANSWER__.term_key = (args) => (hold.sent.push(`key:${JSON.stringify(args.press ?? null)}`), null);
  window.__ANSWER__.answer_ask = (args) => (hold.sent.push(`answer:${JSON.stringify(args.selections ?? null)}`), null);
  window.__ANSWER__.answer_approval = () => null;
  const rows = [{ role: "user", text: "어제 하던 일" }, { role: "assistant", text: "끝났습니다." }];
  window.__ANSWER__.pane_log = (args) => ({
    found: true, next: 1, turns: args.after === null || args.after === undefined ? rows : [], skipped: false, more: false,
  });
  window.__ANSWER__.wire_stop = () => null;
  // The long clocks the window arms are kept, not run: the scene runs them
  // once it has moved the clock past them.
  const realTimeout = window.setTimeout.bind(window);
  window.__REAL_TIMEOUT__ = realTimeout;
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
    sent: [...window.__HOLD__.sent],
    wires: window.__HOLD__.wires.length,
    refused: chat?.querySelector('.pane-chat-notice[data-says="worker.wireRefused"]')?.textContent ?? null,
    exitTyped: chat?.querySelector('.pane-chat-notice[data-says="worker.handOverExitTyped"]')?.textContent ?? null,
    box: composer?.querySelector(".worker-composer-box")?.value ?? null,
    toasts: [...document.querySelectorAll(".toast")].map((note) => ({ words: note.textContent, tip: note.dataset.tip ?? null })),
  };
};

const ENTER = `key:${JSON.stringify({ key: "Enter", ctrl: false, alt: false })}`;
const keys = (sent) => sent.filter((one) => one.startsWith("key:")).length;

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

/* Until `count` Enters reached the pane, or four seconds. */
async function untilKeys(page, count) {
  await page.waitForFunction((count) => window.__HOLD__.sent.filter((one) => one.startsWith("key:")).length >= count, count, { timeout: 4000 }).catch(() => null);
}

/* Enter, ×, Enter again — both wait — then the hold's bound passes, then
 * Enter once more. */
async function heldThenPassed(page) {
  const seen = {};
  await enter(page, "첫 글");
  seen.first = await page.evaluate(LOOK);
  await dropFirst(page);
  await enter(page, "다시 친 글");
  seen.again = await page.evaluate(LOOK);
  await page.evaluate(() => window.__PASS__());
  await untilKeys(page, 1);
  seen.passed = await page.evaluate(LOOK);
  await enter(page, "곧바로");
  await untilKeys(page, 2);
  seen.after = await page.evaluate(LOOK);
  return seen;
}

const WENT = ["paste:다시 친 글", ENTER];
const BOTH_WENT = [...WENT, "paste:곧바로", ENTER];

export async function testComposerHold(browser, origin, ok) {
  await testNothingHolds(browser, origin, ok);
  await testHandOverReadsOnce(browser, origin, ok);
  await testHandOverReadStalls(browser, origin, ok);
  await testHandOverReadAnswersLate(browser, origin, ok);
  await testWireStartStalls(browser, origin, ok);
  await testCoordinatorSeatRefused(browser, origin, ok);
  await testExitTypedThenRefused(browser, origin, ok);
  await testPaneLeftAndWireRefused(browser, origin, ok);
  await testAnsweredCardHolds(browser, origin, ok);
  await testSentTurnHolds(browser, origin, ok);
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
    await untilKeys(page, 1);
    const seen = await page.evaluate(LOOK);
    ok(
      "enter_on_a_resting_pane_with_nothing_holding_goes_at_once: a pane the restart brought back, its head 「대기 중」 and no card, sends the words straight down its own road — paste, then Enter — and its queue stays empty",
      seen.head === words.idle && !seen.card && seen.queue.length === 0 &&
        JSON.stringify(seen.sent) === JSON.stringify(["paste:곧바로", ENTER]),
      JSON.stringify({ seen, words }),
    );
  });
}

/* The hand-over's read on a transcript tens of MiB long: the read back from
 * the end stops two MiB short of it, before a line still being written, and
 * every read on from a cursor brings one turn of the next MiB — the old walk
 * to the file's end read them all (3 reads, 2.25 MiB here; one per MiB on a
 * longer stretch). The hand-over reads once, back from the end, the bounded
 * read the view opened with, and asks its wire within the second. */
async function testHandOverReadsOnce(browser, origin, ok) {
  await scene(browser, origin, ok, "hand-over reads once", async (page) => {
    const seen = await page.evaluate(async () => {
      const MIB = 1024 * 1024;
      const size = 64 * MIB;
      const tailBytes = 256 * 1024;
      const reads = [];
      // Where a read back from the end stops: short of a line still being
      // written, two MiB before the file's end.
      const tailCursor = size - 2 * MIB;
      window.__ANSWER__.pane_log = (args) => {
        if (args.after === null || args.after === undefined) {
          reads.push({ after: null, bytes: tailBytes });
          return { found: true, start: tailCursor - tailBytes, next: tailCursor, more: true, skipped: false,
            turns: [{ role: "user", text: "긴 대화의 끝" }, { role: "assistant", text: "끝부분" }] };
        }
        if (args.after < tailCursor) {
          // The view following its cursor: nothing new.
          return { found: true, next: args.after, more: false, skipped: false, turns: [] };
        }
        reads.push({ after: args.after, bytes: MIB });
        return { found: true, next: Math.min(size, args.after + MIB), more: args.after + MIB < size, skipped: false,
          turns: [{ role: "assistant", text: `MiB ${args.after / MIB}` }] };
      };
      window.__ANSWER__.wire_start = (args) => {
        window.__HOLD__.wires.push({ ...args, askedAt: performance.now() });
        return { id: 91, agent: args.agent, protocol: "claude-stream", version: "2.1.312", model: null, session: args.resume };
      };
      window.__ANSWER__.wire_log = () => ({
        found: true, skipped: false, next: 0, turns: [], status: "idle", asks: [], live: [], agent: "claude",
        protocol: "claude-stream", models: [], modes: [], commands: [], version: "2.1.312",
      });
      window.__ANSWER__.wire_history = () => ({ found: false, next: 0, turns: [] });
      const reading = await (async () => {
        const from = performance.now();
        el("view-toggle-chat").click();
        for (let tries = 0; tries < 200 && window.__HOLD__.wires.length === 0; tries += 1) {
          await new Promise((done) => window.__REAL_TIMEOUT__(done, 10));
        }
        const asked = window.__HOLD__.wires[0] ?? null;
        return { from, askedMs: asked ? Math.round(asked.askedAt - from) : null, asked };
      })();
      return {
        handOverReads: reads.length,
        tailReads: reads.filter((one) => one.after === null).length,
        bytes: reads.reduce((sum, one) => sum + one.bytes, 0),
        tailBytes,
        askedMs: reading.askedMs,
        wireTurns: tabs.find((tab) => tab.id === "wire:91")?.worker.helper.turns.map((turn) => turn.text) ?? null,
      };
    });
    ok(
      "a_hand_over_reads_a_long_transcript_once_and_asks_its_wire_within_a_second: on a 64 MiB transcript whose end is still being written, the 「대화」 press reads the pane's transcript once — back from the end, no more bytes than that one bounded read — asks its wire under a second after the press, and the wire's page opens on the turns that read brought",
      seen.handOverReads === 1 && seen.tailReads === 1 && seen.bytes <= seen.tailBytes &&
        seen.askedMs !== null && seen.askedMs < 1000 &&
        Array.isArray(seen.wireTurns) && seen.wireTurns.includes("긴 대화의 끝") && !seen.wireTurns.some((text) => text.startsWith("MiB ")),
      JSON.stringify(seen),
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
      window.__ANSWER__.wire_start = (args) => {
        window.__HOLD__.wires.push({ ...args, askedAt: Date.now() });
        return new Promise(() => {});
      };
      el("view-toggle-chat").click();
      for (let beat = 0; beat < 3; beat += 1) await window.__PAINTED__();
    });
    const words = await page.evaluate(() => ({
      idle: t("worker.idle", "대기 중"),
      why: t("composer.queue.why.handing", "대화를 실시간으로 잇는 중 — 이어지면, 늦어도 {{s}}초 뒤 보냅니다", { s: 60 }),
      stalled: t("worker.handOverStalled", "대화 기록 읽기가 {{s}}초 안에 끝나지 않았습니다", { s: 45 }),
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

/* The read answers, but only after the hand-over's deadline: the wire is not
 * asked — no wire process starts only for the backend to stop it. */
async function testHandOverReadAnswersLate(browser, origin, ok) {
  await scene(browser, origin, ok, "hand-over read answers late", async (page) => {
    const seen = await page.evaluate(async () => {
      let answer = null;
      window.__ANSWER__.pane_log = () => new Promise((done) => { answer = done; });
      window.__ANSWER__.wire_start = (args) => {
        window.__HOLD__.wires.push({ ...args, askedAt: Date.now() });
        return new Promise(() => {});
      };
      el("view-toggle-chat").click();
      for (let beat = 0; beat < 3; beat += 1) await window.__PAINTED__();
      for (let tries = 0; tries < 100 && answer === null; tries += 1) {
        await new Promise((done) => window.__REAL_TIMEOUT__(done, 10));
      }
      const realNow = Date.now;
      const due = typeof COMPOSER_HAND_OVER_DUE_MS === "number" ? COMPOSER_HAND_OVER_DUE_MS : 45_000;
      Date.now = () => realNow() + due + 1000;
      answer?.({ found: true, start: 0, next: 1, more: false, skipped: false, turns: [{ role: "user", text: "늦은 읽기" }] });
      for (let beat = 0; beat < 3; beat += 1) await window.__PAINTED__();
      await new Promise((done) => window.__REAL_TIMEOUT__(done, 200));
      return { read: answer !== null, wires: window.__HOLD__.wires.length, handing: paneChats.get(window.__TERM__)?.handing ?? null };
    });
    const refused = await page.evaluate(LOOK);
    ok(
      "a_read_that_answers_after_the_deadline_asks_no_wire: the hand-over's read answered only after its deadline — the wire is never asked, the hand-over ends and the page says why",
      seen.read && seen.wires === 0 && seen.handing === false && typeof refused.refused === "string" && refused.refused.length > 0,
      JSON.stringify({ seen, refused: refused.refused }),
    );
  });
}

/* ② The read answers, and the wire's start never does. */
async function testWireStartStalls(browser, origin, ok) {
  // The conversation goes while its queue waits on a hand-over: its release
  // clock goes with it.
  await scene(browser, origin, ok, "forgotten while held", async (page) => {
    const forgotten = await page.evaluate(async () => {
      window.__ANSWER__.wire_start = () => new Promise(() => {});
      el("view-toggle-chat").click();
      for (let beat = 0; beat < 3; beat += 1) await window.__PAINTED__();
      const run = paneChats.get(window.__TERM__).run;
      run.queue = ["남은 것"];
      armComposerRelease(run);
      const armed = Number.isFinite(run.queueRelease?.at);
      forgetPaneChat(window.__TERM__);
      return { armed, after: run.queueRelease ?? null };
    }).catch((error) => ({ error: String(error) }));
    ok(
      "a_forgotten_conversation_leaves_no_release_clock: a conversation forgotten while its queue waited on a hand-over drops the clock that would have asked it again",
      forgotten.armed === true && forgotten.after === null,
      JSON.stringify(forgotten),
    );
  });
  await scene(browser, origin, ok, "wire start stalls", async (page) => {
    await page.evaluate(async () => {
      window.__ANSWER__.wire_start = (args) => {
        window.__HOLD__.wires.push({ ...args, askedAt: Date.now() });
        return new Promise(() => {});
      };
      el("view-toggle-chat").click();
      for (let beat = 0; beat < 3; beat += 1) await window.__PAINTED__();
    });
    const asked = await page.evaluate(() => {
      const held = paneChats.get(window.__TERM__);
      const wire = window.__HOLD__.wires[0] ?? null;
      return {
        term: window.__TERM__,
        fromPane: wire?.fromPane ?? null,
        snakeKeys: wire ? Object.keys(wire).filter((key) => key.includes("_")) : null,
        deadline: wire?.deadlineMs ?? null,
        want: typeof COMPOSER_HAND_OVER_DUE_MS === "number" && Number.isFinite(held?.handingSince)
          ? held.handingSince + COMPOSER_HAND_OVER_DUE_MS : null,
        due: typeof COMPOSER_HAND_OVER_DUE_MS === "number" ? COMPOSER_HAND_OVER_DUE_MS : null,
        hold: typeof COMPOSER_HOLD_MS === "number" ? COMPOSER_HOLD_MS : null,
      };
    });
    const seen = await heldThenPassed(page);
    ok(
      "a_wire_that_never_stands_holds_the_queue_no_longer_than_its_hold: with the wire asked and never answering, Enter (and Enter again after ×) waits in the queue; past `COMPOSER_HOLD_MS` the window lets it go to the pane once, and the next Enter goes straight",
      JSON.stringify(seen.again.queue) === JSON.stringify(["다시 친 글"]) && seen.again.sent.length === 0 && seen.again.wires === 1 &&
        JSON.stringify(seen.passed.sent) === JSON.stringify(WENT) && seen.passed.queue.length === 0 &&
        JSON.stringify(seen.after.sent) === JSON.stringify(BOTH_WENT),
      JSON.stringify(seen),
    );
    ok(
      "the_wire_is_asked_with_a_deadline_inside_the_hold: the hand-over asks `wire_start` with `deadlineMs` — its start plus `COMPOSER_HAND_OVER_DUE_MS`, which leaves the pane's 8 s exit wait inside the 60 s hold — so a wire standing later is the backend's to stop",
      asked.deadline !== null && asked.deadline === asked.want && asked.hold === 60_000 && asked.due !== null && asked.due <= asked.hold - 8_000,
      JSON.stringify(asked),
    );
    ok(
      "the_wire_is_asked_for_the_pane_it_continues: the hand-over names its pane as `fromPane` — tauri's spelling of the backend's `from_pane` — so the backend types the pane's exit command and writes its receipt; no key goes in snake case, which tauri drops (10-30: `from_pane` arrived as nothing and no pane ever left)",
      asked.fromPane === asked.term && Array.isArray(asked.snakeKeys) && asked.snakeKeys.length === 0,
      JSON.stringify(asked),
    );
  });
}

/* The pane is a run's coordinator seat, and the backend refuses its
 * hand-over at once (t-84850): the ledger brings that run's mail to its
 * screen, and the hand-over now really ends a pane's CLI. Nothing holds the
 * words — no 60 s, no line saying the conversation is being continued live —
 * and the page says why in the backend's words. */
async function testCoordinatorSeatRefused(browser, origin, ok) {
  // The refusals the backend gives before anything is typed (t-84850 r2): a
  // CLI started from the pane's shell, one that has taken no input since it
  // began (a pane restored a moment ago), one at work, one whose line holds
  // words, a ledger that cannot be read. Each takes the coordinator's road.
  for (const [name, reason] of [
    ["coordinator seat refused", "a coordinator's pane stays a pane: the ledger brings its run's mail to that screen"],
    ["shell pane refused", "a CLI started from the pane's shell stays in the pane: the terminal is its live view"],
    ["waking pane refused", "the pane's CLI has taken no input since it started: try again in a moment"],
    ["line with words refused", "the pane's input line is not shown empty — words may be written there: nothing was typed"],
    ["unread ledger refused", "the ledger cannot be read now, so this pane may be a coordinator's: it stays a pane"],
  ]) await refusedBeforeTyping(browser, origin, ok, name, reason);
}

async function refusedBeforeTyping(browser, origin, ok, name, reason) {
  await scene(browser, origin, ok, name, async (page) => {
    await page.evaluate(async (reason) => {
      window.__ANSWER__.wire_start = (args) => {
        window.__HOLD__.wires.push(args);
        throw new Error(reason);
      };
      el("view-toggle-chat").click();
      for (let beat = 0; beat < 3; beat += 1) await window.__PAINTED__();
    }, reason);
    await enter(page, "곧바로");
    await untilKeys(page, 1);
    const seen = await page.evaluate(LOOK);
    ok(
      `a_pane_refused_before_typing_keeps_its_conversation_and_the_words_go_straight_to_it (${name}): the hand-over is refused at once — Enter sends the words to the pane straight (paste, then Enter), nothing waits in the queue and no line says the conversation is being continued, and the page says why in the backend's words`,
      seen.wires === 1 && JSON.stringify(seen.sent) === JSON.stringify(["paste:곧바로", ENTER]) && seen.queue.length === 0 &&
        (seen.why ?? "") === "" && typeof seen.refused === "string" && seen.refused.includes(reason) && seen.exitTyped === null,
      JSON.stringify(seen),
    );
  });
}

/* The wire's answer the scene gives when it says so: `wire_start` tells the
 * window the pane's exit command was typed (`wire:exit-typed`, as the backend
 * does right after its first key) and then waits. */
const EXIT_TYPED = "the exit command was typed at the pane and the pane still stands";
const typedAndWaiting = () => {
  window.__ANSWER__.wire_start = (args) => {
    window.__HOLD__.wires.push(args);
    for (const heard of window.__LISTENERS__["wire:exit-typed"] ?? []) heard({ payload: { term: args.fromPane } });
    return new Promise((done, fail) => { window.__WIRE__ = { done, fail }; });
  };
};
const paneLeft = () => {
  for (const heard of window.__LISTENERS__["term:exited"] ?? []) heard({ payload: { term: window.__TERM__, code: 0 } });
};

/* ② The pane's exit command was typed and the pane does not leave — a CLI
 * that reads its line late (t-84850 r2 B, P2-5). The hold has no clock now:
 * past `COMPOSER_HOLD_MS` nothing goes to the pane, whose line may still hold
 * `/exit`. Refused at the end of the backend's watch, what waited comes back
 * to the box as one draft, one line says why, and nothing was ever typed at
 * the pane. When the CLI then reads `/exit` and the pane leaves, the draft is
 * said with a way to copy it rather than gone. */
async function testExitTypedThenRefused(browser, origin, ok) {
  await scene(browser, origin, ok, "exit typed then refused", async (page) => {
    await page.evaluate(typedAndWaiting);
    await page.evaluate(async () => {
      el("view-toggle-chat").click();
      for (let beat = 0; beat < 3; beat += 1) await window.__PAINTED__();
    });
    const words = await page.evaluate(() => ({
      waiting: t("composer.queue.why.exitTyped", "판의 CLI가 끝나기를 기다립니다 — 끝나면 대화로 보내고, 아니면 입력칸에 돌려 둡니다"),
      line: t("worker.handOverExitTyped", "종료 명령을 보냈지만 판이 아직 떠 있습니다 — 판을 확인한 뒤 입력칸의 글을 다시 보내세요."),
    }));
    await enter(page, "첫 글");
    const held = await page.evaluate(LOOK);
    await page.evaluate(() => window.__PASS__());
    await enter(page, "둘째 글");
    const past = await page.evaluate(LOOK);
    await page.evaluate((reason) => window.__WIRE__.fail(new Error(`${reason} 60s after \`/exit\` — check the pane, then send again`)), EXIT_TYPED);
    for (let beat = 0; beat < 3; beat += 1) await page.evaluate(() => window.__PAINTED__());
    const back = await page.evaluate(LOOK);
    ok(
      "a_typed_exit_holds_the_words_with_no_clock: once the pane's exit command is typed, what is sent waits in the queue under its own line — and past the hold's 60 s it still waits: nothing is typed at a pane that may yet read `/exit`",
      JSON.stringify(held.queue) === JSON.stringify(["첫 글"]) && held.why === words.waiting && held.sent.length === 0 &&
        JSON.stringify(past.queue) === JSON.stringify(["첫 글", "둘째 글"]) && past.sent.length === 0,
      JSON.stringify({ held, past, words }),
    );
    ok(
      "a_refusal_after_the_exit_was_typed_gives_the_words_back_to_the_box: the backend refuses after typing the exit command — the queue empties into the box as one draft in the order sent, one line says the pane is still open, no other refusal line stands, and nothing ever reached the pane",
      back.queue.length === 0 && back.box === "첫 글\n\n둘째 글" && back.exitTyped !== null && back.exitTyped.includes(words.line) &&
        back.refused === null && back.sent.length === 0,
      JSON.stringify({ back, words }),
    );
    await page.evaluate(paneLeft);
    for (let beat = 0; beat < 3; beat += 1) await page.evaluate(() => window.__PAINTED__());
    const gone = await page.evaluate(LOOK);
    const lost = await page.evaluate(() => t("composer.queue.lost", "판이 닫혀 기다리던 글 {{n}}개를 보내지 못했습니다", { n: 2 }));
    ok(
      "a_pane_that_leaves_with_the_words_in_its_box_says_them: the CLI reads `/exit` late and the pane leaves — the words its box held are said in a toast that keeps them to copy, and still nothing was typed at the pane",
      gone.toasts.some((note) => note.words.includes(lost) && note.tip === "첫 글\n\n둘째 글") && gone.sent.length === 0,
      JSON.stringify({ gone, lost }),
    );
  });
}

/* The pane leaves while its hand-over runs and the wire is refused anyway
 * (t-84850 r2 P2-1): no box and no screen are left, so the words that waited
 * are said with a way to copy them — never dropped without a word. */
async function testPaneLeftAndWireRefused(browser, origin, ok) {
  await scene(browser, origin, ok, "pane left and wire refused", async (page) => {
    await page.evaluate(typedAndWaiting);
    await page.evaluate(async () => {
      el("view-toggle-chat").click();
      for (let beat = 0; beat < 3; beat += 1) await window.__PAINTED__();
    });
    await enter(page, "남은 글");
    await page.evaluate(paneLeft);
    await page.evaluate(() => window.__WIRE__.fail(new Error("the wire closed")));
    for (let beat = 0; beat < 3; beat += 1) await page.evaluate(() => window.__PAINTED__());
    const seen = await page.evaluate(() => ({
      sent: [...window.__HOLD__.sent],
      toasts: [...document.querySelectorAll(".toast")].map((note) => ({ words: note.textContent, tip: note.dataset.tip ?? null })),
      lost: t("composer.queue.lost", "판이 닫혀 기다리던 글 {{n}}개를 보내지 못했습니다", { n: 1 }),
    }));
    ok(
      "words_that_waited_on_a_pane_that_left_are_said_when_the_wire_is_refused: the pane leaves during the hand-over and the wire is refused — the word that waited is said in a toast that keeps it to copy, and nothing went anywhere silently",
      seen.toasts.some((note) => note.words.includes(seen.lost) && note.tip === "남은 글") && seen.sent.length === 0,
      JSON.stringify(seen),
    );
  });
}

/* ③ A question's card answered from the box, and no report says the pane
 * moved past it: the card stands with its answer gone. The clock does not
 * let the words go under it (t-81786 P0); the pane moving does. */
async function testAnsweredCardHolds(browser, origin, ok) {
  await scene(browser, origin, ok, "answered card holds", async (page) => {
    await page.evaluate(async () => {
      window.__TELL__({ term: window.__TERM__, state: "waiting", agent: "claude",
        ask_prompt: { questions: [{ question: "어느 쪽으로 할까요?", header: "방향",
          options: [{ label: "A", description: "가" }, { label: "B", description: "나" }], multi_select: false }] } });
      for (let beat = 0; beat < 3; beat += 1) await window.__PAINTED__();
    });
    await enter(page, "B로 가 주세요");
    const words = await page.evaluate(() => ({
      why: t("composer.queue.why.answered", "질문에 보낸 답이 닿기를 기다립니다 — 판이 넘어가면 보냅니다"),
    }));
    await enter(page, "다음 지시");
    const held = await page.evaluate(LOOK);
    await page.evaluate(() => window.__PASS__());
    const passed = await page.evaluate(LOOK);
    await page.evaluate(async () => {
      window.__TELL__({ term: window.__TERM__, state: "working", agent: "claude" });
      for (let beat = 0; beat < 2; beat += 1) await window.__PAINTED__();
      window.__TELL__({ term: window.__TERM__, state: "done", agent: "claude" });
      for (let beat = 0; beat < 2; beat += 1) await window.__PAINTED__();
    });
    await untilKeys(page, 1);
    const moved = await page.evaluate(LOOK);
    const answer = `answer:${JSON.stringify([{ indices: [], other: "B로 가 주세요" }])}`;
    ok(
      "an_answered_card_holds_the_queue_until_the_pane_moves_never_by_the_clock: the card's answer went and the pane said nothing — Enter waits and says why; past `COMPOSER_HOLD_MS` nothing reaches the pane under the card; once the pane moves on the words go once",
      held.card && JSON.stringify(held.queue) === JSON.stringify(["다음 지시"]) && held.why === words.why &&
        JSON.stringify(held.sent) === JSON.stringify([answer]) &&
        passed.card && JSON.stringify(passed.queue) === JSON.stringify(["다음 지시"]) && JSON.stringify(passed.sent) === JSON.stringify([answer]) &&
        !moved.card && moved.queue.length === 0 &&
        JSON.stringify(moved.sent) === JSON.stringify([answer, "paste:다음 지시", ENTER]) && keys(moved.sent) === 1,
      JSON.stringify({ held, passed, moved, words }),
    );
  });
}

/* ④ The queue sent its first message and no report says the turn it should
 * start began: the rest waits for the pane, never for the clock. */
async function testSentTurnHolds(browser, origin, ok) {
  await scene(browser, origin, ok, "sent turn holds", async (page) => {
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
    await untilKeys(page, 1);
    await page.waitForTimeout(200);
    const words = await page.evaluate(() => ({
      why: t("composer.queue.why.started", "앞서 보낸 글의 턴이 시작되기를 기다립니다"),
    }));
    const held = await page.evaluate(LOOK);
    await page.evaluate(() => window.__PASS__());
    const passed = await page.evaluate(LOOK);
    await page.evaluate(async () => {
      window.__TELL__({ term: window.__TERM__, state: "working", agent: "claude" });
      for (let beat = 0; beat < 2; beat += 1) await window.__PAINTED__();
      window.__TELL__({ term: window.__TERM__, state: "done", agent: "claude" });
      for (let beat = 0; beat < 2; beat += 1) await window.__PAINTED__();
    });
    await untilKeys(page, 2);
    const moved = await page.evaluate(LOOK);
    ok(
      "a_queue_whose_sent_turn_is_not_seen_waits_for_the_pane_not_the_clock: the first held message went at the turn's end; the second waits and says why, still waits past `COMPOSER_HOLD_MS`, and goes once the pane's next turn starts and ends",
      JSON.stringify(held.sent) === JSON.stringify(["paste:하나", ENTER]) && JSON.stringify(held.queue) === JSON.stringify(["둘"]) &&
        held.why === words.why &&
        JSON.stringify(passed.sent) === JSON.stringify(["paste:하나", ENTER]) && JSON.stringify(passed.queue) === JSON.stringify(["둘"]) &&
        JSON.stringify(moved.sent) === JSON.stringify(["paste:하나", ENTER, "paste:둘", ENTER]) && moved.queue.length === 0,
      JSON.stringify({ held, passed, moved, words }),
    );
  });
}
