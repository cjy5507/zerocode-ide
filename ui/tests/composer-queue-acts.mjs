/* 대기열의 행마다 고치기·끼워 넣기·바로 보내기·지우기 (t-86909, 사람 10-31 「지금은
 * 큐에만 올라가는데 에디트 삭제 등」 · 「우리 디자인에 맞게」).
 *
 * 일하는 판에 보낸 글은 입력칸 위 「대기 N」 목록에 선다. 행마다:
 *   - 고치기   — 그 글이 입력칸으로 올라오고, 보내면 같은 자리로 돌아간다. 취소하면
 *                글은 제자리에 남고, 입력칸에 쓰던 글은 덮이지 않는다.
 *   - 끼워 넣기 — 턴을 끊지 않고 지금 턴에 넣는다(Steer). 에이전트마다 받는 길이 다르고,
 *                확인된 곳(Claude Code 판·선, Codex 선, zo)에만 단추가 있다.
 *   - 바로 보내기 — 턴을 멈추고 이 글을 맨 앞에서 보낸다(Send Now). 멈춤은 사람이 Esc를
 *                누른 것과 같은 길이다.
 *   - 지우기   — 있던 ✕.
 *
 * 안전(어기면 실패): 이 창의 쪽은 판에 키를 직접 치지 않는다 — `term_paste`·`term_key`는
 * 끼워 넣기·바로 보내기가 부르지 않고, 백엔드의 문(`pane_steer`·`pane_interrupt`, 멈춘 뒤의
 * 글은 `pane_send_now`, 선은 `wire_steer`·`wire_interrupt`)만 부른다. 그 문이 판을 다시
 * 보고(메뉴·입력줄·원장의 자리) 거절하면 글은 대기열에 그대로 있고 까닭이 한 줄로 보인다.
 * 원장 워커·코디의 판에는 끼워 넣기·바로 보내기가 없다. 질문·권한 메뉴가 떠 있으면 없다.
 * r2: 단추를 누른 뒤에도 키는 판에 닿지 않는다 — 행이 사라지거나 단추가 꺼져도 초점은 입력줄
 * 안에 있다(마우스로 눌러도, WebKit처럼 단추에 초점이 안 가도).
 *
 * 진짜 CLI는 띄우지 않는다: 문의 답과 판의 상태는 모두 합성이다(사람의 기록이 아니다).
 *
 *   WINDOW_SUITES=composer-queue-acts node ui/tests/window.mjs */
import { openWindowTestPage } from "./window-boot.mjs";
import { reportedPlatformScript } from "./test-platform.mjs";

const ENTER = `key:${JSON.stringify({ key: "Enter", ctrl: false, alt: false })}`;
const ESCAPE_KEY = `key:${JSON.stringify({ key: "Escape", ctrl: false, alt: false })}`;

/* 문이 판에 대고 거절할 때 쓰는 낱말(백엔드 `answer_door`의 `TURN_*`)과, 창이 스스로 더하는 둘. */
const DOOR_REFUSALS = [
  "turn-screen", "turn-seat", "turn-ledger-unread", "turn-idle", "turn-unsupported", "turn-in-flight", "turn-denied",
  // r2: the words the delivery after a stop, the pane that was not reading and the unreadable terminal added.
  "turn-line", "turn-running", "turn-not-reading", "turn-no-session", "turn-no-terminal", "turn-empty",
];

/* 판이나 선에 닿은 것 — 키·글자·붙여넣기. 문을 부른 것은 여기 들지 않는다. */
const REACHED = (sent) => sent.filter((one) => one.startsWith("key:") || one.startsWith("text:") || one.startsWith("paste:"));

/* 한 장면의 도구: 판에 닿은 것, 문이 받은 부름, 시계. 판 합성은 composer-hold와 같은 꼴이다. */
const STAND = async () => {
  window.__ACTS__ = { sent: [], calls: [], steer: null, stop: null, front: null, wireSteer: null, wireStop: null, long: [] };
  const acts = window.__ACTS__;
  window.__ANSWER__.term_paste = (args) => (acts.sent.push(`paste:${args.text}`), null);
  window.__ANSWER__.term_key = (args) => (acts.sent.push(`key:${JSON.stringify(args.press ?? null)}`), null);
  window.__ANSWER__.answer_ask = (args) => (acts.sent.push(`answer:${JSON.stringify(args.selections ?? null)}`), null);
  window.__ANSWER__.answer_approval = () => null;
  window.__ANSWER__.pane_log = (args) => ({
    found: true, next: 1, skipped: false, more: false,
    turns: args.after === null || args.after === undefined ? [{ role: "user", text: "어제 하던 일" }, { role: "assistant", text: "끝났습니다." }] : [],
  });
  window.__ANSWER__.pane_steer = (args) => {
    acts.calls.push({ door: "pane_steer", ...args });
    return acts.steer ? acts.steer(args) : null;
  };
  window.__ANSWER__.pane_interrupt = (args) => {
    acts.calls.push({ door: "pane_interrupt", ...args });
    return acts.stop ? acts.stop(args) : null;
  };
  window.__ANSWER__.wire_steer = (args) => {
    acts.calls.push({ door: "wire_steer", ...args });
    return acts.wireSteer ? acts.wireSteer(args) : null;
  };
  window.__ANSWER__.wire_interrupt = (args) => {
    acts.calls.push({ door: "wire_interrupt", ...args });
    return acts.wireStop ? acts.wireStop(args) : null;
  };
  window.__ANSWER__.wire_send = (args) => {
    acts.calls.push({ door: "wire_send", ...args });
    return null;
  };
  // 사람이 판에 글자를 쳤을 때 가는 길(`term_text`) — 단추 뒤에 새는 키가 여기로도 오므로 기록한다.
  window.__ANSWER__.term_text = (args) => (acts.sent.push(`text:${args.text}`), null);
  // 멈춘 뒤의 글을 판에 치는 문(r2) — 판은 입력줄이 비었는지 다시 보고, 아니면 까닭을 낱말로 거절한다.
  window.__ANSWER__.pane_send_now = (args) => {
    acts.calls.push({ door: "pane_send_now", ...args });
    return acts.front ? acts.front(args) : null;
  };
  window.__ROWBTN__ = (at, act) => window.__COMPOSER__()?.querySelectorAll(".composer-queue-item")[at]?.querySelector(`button[data-act="${act}"]`) ?? null;
  // 입력칸 안에서 ⌘/Ctrl+Alt+Enter(와 Shift)를 친다 — `extra`로 AltGr 같은 것을 더한다.
  window.__CHORD__ = (shift, extra = {}) => {
    const box = window.__COMPOSER__().querySelector(".worker-composer-box");
    box.focus();
    const event = new KeyboardEvent("keydown", { key: "Enter", code: "Enter", altKey: true, shiftKey: shift, bubbles: true, cancelable: true, ...window.__TEST_PRIMARY_EVENT__, ...extra });
    box.dispatchEvent(event);
    return event.defaultPrevented;
  };
  window.__ANSWER__.wire_stop = () => null;
  // 10 s 이상의 시계(바로 보내기의 멈춤 확인, 붙잡음)는 잡아 두었다가 장면이 돌린다.
  const realTimeout = window.setTimeout.bind(window);
  window.__REAL_TIMEOUT__ = realTimeout;
  window.setTimeout = (fn, ms, ...rest) => {
    if (Number(ms) >= 10_000) {
      acts.long.push({ fn, ms: Number(ms) });
      return -1 - acts.long.length;
    }
    return realTimeout(fn, ms, ...rest);
  };
  window.__TELL__ = (payload) => {
    for (const heard of window.__LISTENERS__["hook:agent"] ?? []) heard({ payload });
  };
  // 거절 낱말이 창의 한 문장이 되는 길(아직 없으면 빈 말 — 단언이 틀리지, 장면이 죽지 않는다).
  window.__WORDS__ = (word) => (typeof composerQueueRefusalWords === "function" ? composerQueueRefusalWords(word) : "");
  window.__PAINT__ = async (beats = 3) => {
    for (let beat = 0; beat < beats; beat += 1) await window.__PAINTED__();
  };
  // 눈에 보이는 입력칸 하나 — 판의 대화든 선의 페이지든.
  window.__COMPOSER__ = () => [...document.querySelectorAll(".worker-composer")].find((form) => form.offsetParent !== null) ?? null;
  for (let tries = 0; tries < 200 && !installedAgents().some((row) => row.id === "claude" && row.wire_resumes); tries += 1) {
    await new Promise((done) => realTimeout(done, 25));
  }
  // zo의 행: 목록에 없는 에이전트는 이 장면이 더한다. 끼워 넣는 길은 채널, 멈춤은 Esc.
  if (!agentRows.some((row) => row.id === "zo")) {
    agentRows.push({
      id: "zo", name: "zo", favicon_domain: "", homepage_url: "", installed: true, found_as: "zo", unsupported_here: false,
      missing_requirement: null, takes_a_paste: true, ready: "quiet", glyph: "◐", busy_word: "Working…", models_provider: null,
      permission_road: "/permissions", permission_modes: [], compact_command: "/compact", read_offset_base: 0,
      interrupt_key: "Escape", steer_road: "channel", stop_door: true, spinner_verbs: [], todo_tool: "TodoWrite",
    });
  }
};

/* 판 하나를 열어 대화를 입히고, `state`(훅의 낱말)로 둔다. */
async function openPane(page, agent, state) {
  return page.evaluate(async ({ agent, state }) => {
    const term = await openTermTab({ placement: "tab" });
    paneAgents.set(term, agent);
    paneSessions.set(term, { agent, session: { key: "session_id", id: `s-${agent}` }, resumable: true });
    window.__TELL__({ term, state: "done", agent, session_boundary: true });
    setActiveTab(tabOfTerm(term).id);
    await setPaneChat(term, true, { wire: false });
    await pollHelperPages();
    await window.__PAINT__();
    window.__TERM__ = term;
    if (state) {
      window.__TELL__({ term, state, agent });
      await window.__PAINT__(2);
    }
    return term;
  }, { agent, state });
}

/* 일하는 판에서 `words`를 입력칸에 치고 Enter — 글은 대기열에 선다. */
async function queueWords(page, ...words) {
  for (const one of words) {
    const box = page.locator(".worker-composer-box:visible").first();
    await box.click();
    await page.keyboard.type(one);
    await page.keyboard.press("Enter");
    await page.waitForTimeout(120);
  }
}

/* 지금 장면이 본 것. 없는 것은 null·빈 배열이라 단언이 틀리지, 장면이 죽지 않는다. */
const LOOK = () => {
  const form = window.__COMPOSER__();
  const run = form?.__workerRun ?? null;
  const queue = form?.querySelector(".composer-queue");
  const rows = [...(queue?.querySelectorAll(".composer-queue-item") ?? [])].map((row) => ({
    words: row.querySelector(".composer-queue-words")?.textContent ?? null,
    // 보이는 단추만 — 숨긴 단추는 이 행이 할 수 없는 것이다.
    acts: [...row.querySelectorAll("button[data-act]")].filter((button) => !button.hidden).map((button) => button.dataset.act),
    editing: row.dataset.editing === "true",
    acting: row.dataset.acting ?? null,
    off: [...row.querySelectorAll("button[data-act]")].filter((button) => !button.hidden && button.disabled).map((button) => button.dataset.act),
  }));
  const active = document.activeElement;
  const lines = [...(queue?.querySelectorAll(".composer-queue-item") ?? [])];
  return {
    focus: {
      body: active === document.body || active === null,
      inComposer: Boolean(form?.contains(active)),
      inBox: active?.classList?.contains("worker-composer-box") === true,
      act: active?.dataset?.act ?? null,
      row: active?.closest?.(".composer-queue-item") ? lines.indexOf(active.closest(".composer-queue-item")) : null,
    },
    queue: [...(run?.queue ?? [])],
    rows,
    shown: Boolean(queue) && !queue.hidden,
    why: queue?.querySelector(".composer-queue-why")?.textContent ?? null,
    note: queue?.querySelector(".composer-queue-note")?.textContent ?? "",
    box: form?.querySelector(".worker-composer-box")?.value ?? null,
    sendLabel: form?.querySelector(".worker-composer-send")?.getAttribute("aria-label") ?? null,
    sent: [...window.__ACTS__.sent],
    calls: window.__ACTS__.calls.map((call) => ({ ...call })),
    toasts: [...document.querySelectorAll(".toast")].map((note) => note.textContent),
    uncertain: Boolean(run?.sendUncertain),
  };
};

/* 행 `at`의 단추 `act`를 손이 누르듯 누른다. 단추가 있었는지를 돌려준다(없으면 단언이 틀린다). */
async function press(page, act, at) {
  const found = await page.evaluate(({ act, at }) => {
    const row = window.__COMPOSER__()?.querySelectorAll(".composer-queue-item")[at];
    const button = row?.querySelector(`button[data-act="${act}"]`);
    button?.click();
    return Boolean(button);
  }, { act, at });
  await page.waitForTimeout(150);
  return found;
}

const SAME = (left, right) => JSON.stringify(left) === JSON.stringify(right);
const ACTS = ["edit", "steer", "now", "drop"];

async function scene(browser, origin, ok, name, body, { width = 1440, height = 900, before = null } = {}) {
  const { page, faults } = await openWindowTestPage(browser, origin, before ? { before } : {});
  try {
    await page.setViewportSize({ width, height });
    await page.evaluate(STAND);
    await body(page);
  } finally {
    // Said whether the scene finished or stopped: when it stops, the page's first error is usually the cause and the
    // one that stopped it its echo (Windows CI 2026-11-05: "tabs is not defined" after shell-term.js stopped while
    // loading, and the load's own error was never printed — t-94962).
    ok(`${name}: no page errors`, faults.length === 0, faults.slice(0, 3).join("\n"));
    await page.close();
  }
}

export async function testComposerQueueActs(browser, origin, ok) {
  await testRowAnatomy(browser, origin, ok);
  await testEdit(browser, origin, ok);
  await testEditSwitch(browser, origin, ok);
  await testEditHoldsTheQueue(browser, origin, ok);
  await testSteerPane(browser, origin, ok);
  await testSteerRefused(browser, origin, ok);
  await testSendNowPane(browser, origin, ok);
  await testSendNowRefused(browser, origin, ok);
  await testSendNowUnconfirmed(browser, origin, ok);
  await testZoPane(browser, origin, ok);
  await testNoDoorWhereNoneIsVerified(browser, origin, ok);
  await testWireSteer(browser, origin, ok);
  await testWireSendNow(browser, origin, ok);
  await testShortcuts(browser, origin, ok);
  await testNarrowAndWide(browser, origin, ok);
  await testFiveLanguages(browser, origin, ok);
  await testManyRows(browser, origin, ok);
  // r2 (비저자 검수 r1)
  await testFocusAfterPress(browser, origin, ok);
  await testKeyboardFollowsTheScreen(browser, origin, ok);
  await testSendNowLock(browser, origin, ok);
  await testSendNowDelivery(browser, origin, ok);
  await testUnsureRow(browser, origin, ok);
  await testNoteFollowsTheState(browser, origin, ok);
  await testAltEnter(browser, origin, ok);
  await testEscapeCancelsAnEdit(browser, origin, ok);
  await testNoWordsLostToANotice(browser, origin, ok);
  await testUncertainBand(browser, origin, ok);
  await testTurnBoundaryKeepsTheRows(browser, origin, ok);
  await testRefusalsInFiveLanguages(browser, origin, ok);
  await testFailedDeliveryKeepsTheWords(browser, origin, ok);
  await testComposerQueueActsR3(browser, origin, ok);
  await testComposerQueueActsR4(browser, origin, ok);
}

/* ① 행의 모양: 고치기·끼워 넣기·바로 보내기·지우기가 이 차례로 서고, 각 단추는 이름을
 * 두 번(말풍선·접근 이름) 말하며, 지우기는 키 하나 치지 않고 그 행만 뺀다. */
async function testRowAnatomy(browser, origin, ok) {
  await scene(browser, origin, ok, "row anatomy", async (page) => {
    await openPane(page, "claude", "working");
    await queueWords(page, "하나", "둘", "셋");
    const seen = {};
    seen.rows = await page.evaluate(LOOK);
    seen.buttons = await page.evaluate(() => [...window.__COMPOSER__().querySelectorAll(".composer-queue-item")].map((row) => (
      [...row.querySelectorAll("button")].map((button) => ({
        act: button.dataset.act ?? null,
        tag: button.tagName,
        type: button.getAttribute("type"),
        label: button.getAttribute("aria-label"),
        tip: button.dataset.tip ?? null,
        words: button.dataset.words ?? null,
        tabindex: button.getAttribute("tabindex"),
      }))
    )));
    seen.names = await page.evaluate(() => ({
      steer: t("composer.queue.steer", "지금 턴에 끼워 넣기 — 멈추지 않고 방향만 바꿉니다"),
      now: t("composer.queue.now", "턴을 멈추고 바로 보내기"),
    }));
    // 키보드: 첫 행의 첫 단추에서 Tab으로 같은 행의 나머지를 차례로 지난다.
    seen.tabs = await page.evaluate(async () => {
      const first = window.__COMPOSER__()?.querySelector(".composer-queue-item button");
      first?.focus();
      return first ? document.activeElement === first : false;
    });
    const order = [];
    for (let step = 0; step < 3; step += 1) {
      await page.keyboard.press("Tab");
      order.push(await page.evaluate(() => document.activeElement?.dataset?.act ?? null));
    }
    seen.tabOrder = order;
    await press(page, "drop", 1);
    seen.afterDrop = await page.evaluate(LOOK);
    ok(
      "a_queued_row_wears_edit_steer_send_now_and_drop_in_that_order: three queued words stand as three rows, each with its four buttons in the order 고치기 · 끼워 넣기 · 바로 보내기 · 지우기",
      seen.rows.rows.length === 3 && seen.rows.rows.every((row) => SAME(row.acts, ACTS)),
      JSON.stringify(seen.rows.rows),
    );
    ok(
      "every_row_button_is_a_named_real_button: each is a <button type=button> reachable by the keyboard whose tooltip and accessible name are the same words, the four names differ, and the three that carry a visible word keep it in `data-words` (the drop's ✕ carries none)",
      seen.buttons.length === 3 && seen.buttons.every((row) => row.length === 4 && row.every((button) =>
        button.tag === "BUTTON" && button.type === "button" && button.tabindex !== "-1" &&
        typeof button.label === "string" && button.label.length >= 2 && button.label === button.tip) &&
        new Set(row.map((button) => button.label)).size === 4 &&
        row.filter((button) => button.act !== "drop").every((button) => typeof button.words === "string" && button.words.length >= 2) &&
        row.find((button) => button.act === "drop").words === null),
      JSON.stringify(seen.buttons[0]),
    );
    ok(
      "the_steer_and_send_now_names_say_what_they_do_not_the_english_word: the steer name says it goes into the running turn without stopping it and the send-now name says it stops the turn first — and neither is the bare English 「Steer」 or 「Send Now」",
      seen.buttons[0]?.find((button) => button.act === "steer")?.label?.startsWith(seen.names.steer.split(" — ")[0]) === true &&
        seen.buttons[0]?.find((button) => button.act === "now")?.label?.startsWith(seen.names.now) === true &&
        !/^(steer|send now)$/i.test(seen.buttons[0]?.find((button) => button.act === "steer")?.words ?? "") &&
        !/^(steer|send now)$/i.test(seen.buttons[0]?.find((button) => button.act === "now")?.words ?? ""),
      JSON.stringify({ names: seen.names, row: seen.buttons[0] }),
    );
    ok(
      "the_keyboard_walks_a_rows_buttons_in_order: Tab from the first button goes to steer, send now, then drop",
      seen.tabs && SAME(seen.tabOrder, ["steer", "now", "drop"]),
      JSON.stringify({ tabs: seen.tabs, order: seen.tabOrder }),
    );
    ok(
      "the_drop_takes_out_its_own_row_and_types_nothing: ✕ on the middle row leaves the first and last in their order, no key or paste reached the pane, and no door was asked",
      SAME(seen.afterDrop.queue, ["하나", "셋"]) && SAME(seen.afterDrop.rows.map((row) => row.words), ["하나", "셋"]) &&
        seen.afterDrop.sent.length === 0 && seen.afterDrop.calls.length === 0,
      JSON.stringify(seen.afterDrop),
    );
  });
}

/* ② 고치기: 올라온 글을 고쳐 보내면 같은 자리, 취소하면 제자리, 쓰던 글은 그대로. */
async function testEdit(browser, origin, ok) {
  await scene(browser, origin, ok, "edit", async (page) => {
    await openPane(page, "claude", "working");
    await queueWords(page, "하나", "둘", "셋");
    const seen = {};
    seen.pressed = await press(page, "edit", 1);
    seen.up = await page.evaluate(LOOK);
    const box = page.locator(".worker-composer-box:visible").first();
    await box.click();
    await page.keyboard.press("End");
    await page.keyboard.type(" 고침");
    await page.keyboard.press("Enter");
    await page.waitForTimeout(200);
    seen.saved = await page.evaluate(LOOK);
    ok(
      "an_edit_brings_the_words_up_into_the_box_and_marks_the_row: 고치기 on the middle row puts its words in the box, marks only that row, and leaves the queue as it was",
      seen.pressed && seen.up.box === "둘" && SAME(seen.up.rows.map((row) => row.editing), [false, true, false]) &&
        SAME(seen.up.queue, ["하나", "둘", "셋"]) && seen.up.sent.length === 0,
      JSON.stringify(seen.up),
    );
    ok(
      "a_saved_edit_goes_back_to_the_same_place: Enter on the changed words puts them where the row stood — not at the end, not sent — and the box and the marks are clear again",
      SAME(seen.saved.queue, ["하나", "둘 고침", "셋"]) && seen.saved.box === "" && seen.saved.rows.every((row) => !row.editing) &&
        seen.saved.sent.length === 0,
      JSON.stringify(seen.saved),
    );

    // 취소: Esc는 고치기를 끝낼 뿐 턴을 멈추지 않는다.
    await press(page, "edit", 2);
    await box.click();
    await page.keyboard.press("End");
    await page.keyboard.type(" 버릴 글");
    seen.mid = await page.evaluate(LOOK);
    await page.keyboard.press("Escape");
    await page.waitForTimeout(200);
    seen.cancelled = await page.evaluate(LOOK);
    ok(
      "a_cancelled_edit_leaves_the_words_where_they_stood: Esc after changing the words ends the edit — the row keeps its original words in its place, the box is empty again, and the Esc did not stop the turn (no key, no interrupt)",
      seen.mid.box === "셋 버릴 글" && SAME(seen.cancelled.queue, ["하나", "둘 고침", "셋"]) && seen.cancelled.box === "" &&
        seen.cancelled.rows.every((row) => !row.editing) && seen.cancelled.sent.length === 0 && !seen.cancelled.sent.includes(ESCAPE_KEY) &&
        seen.cancelled.calls.length === 0,
      JSON.stringify({ mid: seen.mid.box, cancelled: seen.cancelled }),
    );

    // 쓰던 글: 덮이지 않고, 저장·취소 어느 쪽 뒤에도 돌아온다.
    await box.click();
    await page.keyboard.type("쓰던 글");
    await press(page, "edit", 0);
    seen.withDraft = await page.evaluate(LOOK);
    await box.click();
    await page.keyboard.press("End");
    await page.keyboard.type("!");
    await page.keyboard.press("Enter");
    await page.waitForTimeout(200);
    seen.draftBackAfterSave = await page.evaluate(LOOK);
    await press(page, "edit", 1);
    await page.keyboard.press("Escape");
    await page.waitForTimeout(200);
    seen.draftBackAfterCancel = await page.evaluate(LOOK);
    ok(
      "an_edit_never_overwrites_what_was_being_written: with a draft in the box, 고치기 shows the row's words, and after the save the draft is back in the box untouched — and again after a cancel",
      seen.withDraft.box === "하나" && SAME(seen.draftBackAfterSave.queue, ["하나!", "둘 고침", "셋"]) &&
        seen.draftBackAfterSave.box === "쓰던 글" && seen.draftBackAfterCancel.box === "쓰던 글" &&
        SAME(seen.draftBackAfterCancel.queue, ["하나!", "둘 고침", "셋"]),
      JSON.stringify({ withDraft: seen.withDraft.box, save: seen.draftBackAfterSave.box, cancel: seen.draftBackAfterCancel.box }),
    );

    // 비운 채 저장: 글이 말없이 사라지지 않는다 — 그대로 두고 까닭을 말한다.
    await box.fill("");
    await press(page, "edit", 2);
    await page.locator(".worker-composer-box:visible").first().fill("");
    await page.locator(".worker-composer-box:visible").first().press("Enter");
    await page.waitForTimeout(200);
    seen.empty = await page.evaluate(LOOK);
    ok(
      "saving_an_edit_with_nothing_in_it_loses_no_words: Enter on an emptied box keeps all three rows as they were, the row is still being edited, and a line says what to do",
      SAME(seen.empty.queue, ["하나!", "둘 고침", "셋"]) && seen.empty.rows[2]?.editing === true && seen.empty.note.length >= 8,
      JSON.stringify(seen.empty),
    );
    // 고치는 중인 행을 지우면 고치기도 끝나고, 상자는 돌아온다.
    await press(page, "drop", 2);
    seen.droppedEdited = await page.evaluate(LOOK);
    ok(
      "dropping_the_row_being_edited_ends_the_edit: ✕ on the edited row removes it, clears the edit mark and gives the box back",
      SAME(seen.droppedEdited.queue, ["하나!", "둘 고침"]) && seen.droppedEdited.rows.every((row) => !row.editing) && seen.droppedEdited.box === "",
      JSON.stringify(seen.droppedEdited),
    );
  });
}

/* ②-1 한 행을 고치다 다른 행의 연필을 누르면: 바꾼 것이 없으면 옮겨 가고, 바꾼 것이 있으면 그 글을
 * 버리지 않고 까닭을 말한다. */
async function testEditSwitch(browser, origin, ok) {
  await scene(browser, origin, ok, "edit switch", async (page) => {
    await openPane(page, "claude", "working");
    await queueWords(page, "가", "나", "다");
    const seen = {};
    await press(page, "edit", 0);
    await press(page, "edit", 1);
    seen.moved = await page.evaluate(LOOK);
    const box = page.locator(".worker-composer-box:visible").first();
    await box.click();
    await page.keyboard.press("End");
    await page.keyboard.type("!");
    await press(page, "edit", 2);
    seen.blocked = await page.evaluate(LOOK);
    seen.said = await page.evaluate(() => window.__WORDS__("turn-editing"));
    await box.click();
    await page.keyboard.press("Escape");
    await page.waitForTimeout(150);
    seen.after = await page.evaluate(LOOK);
    ok(
      "a_second_pencil_moves_the_edit_when_nothing_was_changed: with the first row in the box and untouched, 고치기 on the second row brings the second row up in its place",
      seen.moved.box === "나" && SAME(seen.moved.rows.map((row) => row.editing), [false, true, false]) && SAME(seen.moved.queue, ["가", "나", "다"]),
      JSON.stringify(seen.moved),
    );
    ok(
      "a_second_pencil_does_not_throw_away_unsaved_changes: with changes in the box, 고치기 on another row leaves the box and the row being edited as they are and says why — and Esc then gives the row its original words",
      seen.blocked.box === "나!" && SAME(seen.blocked.rows.map((row) => row.editing), [false, true, false]) &&
        seen.said.length >= 8 && seen.blocked.note === seen.said && SAME(seen.after.queue, ["가", "나", "다"]) && seen.after.box === "",
      JSON.stringify({ blocked: seen.blocked, said: seen.said, after: seen.after }),
    );
  });
}

/* ②-2 고치는 동안 대기열은 그 글을 몰래 보내지 않는다: 턴이 끝나도 머리 행은 상자에 있고,
 * 저장하면 그때 나간다(자리 순서는 그대로). */
async function testEditHoldsTheQueue(browser, origin, ok) {
  await scene(browser, origin, ok, "edit holds the queue", async (page) => {
    const term = await openPane(page, "claude", "working");
    await queueWords(page, "하나", "둘");
    await press(page, "edit", 0);
    await page.evaluate(async (term) => {
      window.__TELL__({ term, state: "done", agent: "claude" });
      await window.__PAINT__(3);
    }, term);
    await page.waitForTimeout(300);
    const held = await page.evaluate(LOOK);
    const box = page.locator(".worker-composer-box:visible").first();
    await box.click();
    await page.keyboard.press("End");
    await page.keyboard.type(" 고침");
    await page.keyboard.press("Enter");
    await page.waitForTimeout(900);
    const sent = await page.evaluate(LOOK);
    ok(
      "a_row_being_edited_is_not_sent_behind_the_persons_back: the turn ended while the first row was in the box — nothing reached the pane, both rows stood, and the queue said it waits for the edit",
      held.sent.length === 0 && SAME(held.queue, ["하나", "둘"]) && held.rows[0]?.editing === true && (held.why ?? "").length >= 8,
      JSON.stringify(held),
    );
    ok(
      "the_saved_edit_goes_first_once_the_turn_is_over: Enter on the edit puts the changed words in their place and the queue sends them at once — paste, then Enter — leaving the second row to wait for the next turn",
      SAME(sent.sent, ["paste:하나 고침", ENTER]) && SAME(sent.queue, ["둘"]),
      JSON.stringify(sent),
    );
  });
}

/* ③ 끼워 넣기, Claude Code 판: 키는 백엔드의 문 하나가 친다. */
async function testSteerPane(browser, origin, ok) {
  await scene(browser, origin, ok, "steer pane", async (page) => {
    const term = await openPane(page, "claude", "working");
    await queueWords(page, "하나", "둘", "셋");
    const seen = {};
    seen.pressed = await press(page, "steer", 1);
    seen.after = await page.evaluate(LOOK);
    seen.words = await page.evaluate(() => t("composer.queue.steered", "지금 턴에 끼워 넣었습니다 — 에이전트가 다음 단계에서 읽습니다"));
    ok(
      "steer_on_a_pane_asks_only_the_backends_door_with_that_rows_words: 끼워 넣기 on the middle row asks `pane_steer` once with the pane and exactly its words, and the window typed nothing itself (no paste, no key)",
      seen.pressed && SAME(seen.after.calls, [{ door: "pane_steer", term, text: "둘" }]) && seen.after.sent.length === 0,
      JSON.stringify(seen.after),
    );
    ok(
      "a_steered_row_leaves_the_queue_and_says_where_it_went: the row goes, the others keep their order, and a toast says the words went into the running turn",
      SAME(seen.after.queue, ["하나", "셋"]) && seen.after.toasts.some((words) => words.includes(seen.words)),
      JSON.stringify({ after: seen.after, words: seen.words }),
    );

    // 한 번에 하나: 문이 답하기 전의 두 번째 누름은 아무것도 부르지 않는다.
    await page.evaluate(() => {
      window.__ACTS__.calls.length = 0;
      window.__ACTS__.steer = () => new Promise((done) => { window.__ACTS__.release = done; });
    });
    await press(page, "steer", 0);
    await press(page, "steer", 1);
    await press(page, "now", 1);
    await press(page, "drop", 0);
    seen.flight = await page.evaluate(LOOK);
    await page.evaluate(() => window.__ACTS__.release?.(null));
    await page.waitForTimeout(250);
    seen.flightDone = await page.evaluate(LOOK);
    ok(
      "one_action_at_a_time_on_a_queue: while a steer waits for its door, a second steer, a send now and a drop call nothing and remove nothing, and the row says it is being sent",
      seen.flight.calls.length === 1 && SAME(seen.flight.queue, ["하나", "셋"]) && seen.flight.rows[0]?.acting === "steer" &&
        SAME(seen.flightDone.queue, ["셋"]),
      JSON.stringify({ flight: seen.flight, done: seen.flightDone.queue }),
    );

    // 글이 판의 줄에는 닿았는데 Enter가 못 나갔다: 글은 판에 있다 — 대기열에서 뺀다, 말없이가 아니라 확인하라는 띠와 함께.
    await page.evaluate(() => { window.__ACTS__.steer = () => { throw "turn-landed"; }; });
    await queueWords(page, "넷");
    await press(page, "steer", 1);
    seen.landed = await page.evaluate(LOOK);
    ok(
      "words_that_reached_the_panes_line_but_were_not_sent_are_not_queued_twice: when the door says the paste landed and the Enter did not follow, the row leaves the queue (the words are on the pane's line) and the page asks the person to check there",
      SAME(seen.landed.queue, ["셋"]) && seen.landed.uncertain === true,
      JSON.stringify(seen.landed),
    );
  });
}

/* ③-2 문이 거절하면: 글은 제자리에, 까닭은 한 문장으로, 판에는 아무것도 치지 않는다. */
async function testSteerRefused(browser, origin, ok) {
  await scene(browser, origin, ok, "steer refused", async (page) => {
    await openPane(page, "claude", "working");
    await queueWords(page, "하나", "둘", "셋");
    const seen = { words: {}, kept: {}, tried: [] };
    for (const word of DOOR_REFUSALS) {
      await page.evaluate((word) => { window.__ACTS__.steer = () => { throw word; }; }, word);
      const pressed = await press(page, "steer", 1);
      const look = await page.evaluate(LOOK);
      seen.words[word] = await page.evaluate((word) => window.__WORDS__(word), word);
      seen.kept[word] = look.note;
      seen.tried.push({ word, pressed, queue: look.queue, sent: look.sent.length, acts: look.rows[1]?.acts ?? null });
    }
    ok(
      "a_refused_steer_keeps_the_words_in_their_place_and_types_nothing: for each of the door's seven refusals the queue is untouched, the row still has its buttons, and the window typed nothing",
      seen.tried.every((one) => one.pressed && SAME(one.queue, ["하나", "둘", "셋"]) && one.sent === 0 && SAME(one.acts, ACTS)),
      JSON.stringify(seen.tried),
    );
    ok(
      "every_refusal_is_said_as_a_sentence_of_its_own: the line under the queue is the sentence for that refusal — not the bare word, not empty — and the seven sentences are all different",
      DOOR_REFUSALS.every((word) => seen.words[word].length >= 8 && seen.words[word] !== word && !seen.words[word].includes("turn-") && seen.kept[word] === seen.words[word]) &&
        new Set(Object.values(seen.words)).size === DOOR_REFUSALS.length,
      JSON.stringify({ words: seen.words, kept: seen.kept }),
    );
    // 모르는 오류는 그 글 그대로 말한다(꾸며 대지 않는다).
    await page.evaluate(() => { window.__ACTS__.steer = () => { throw new Error("pty closed"); }; });
    await press(page, "steer", 0);
    const odd = await page.evaluate(LOOK);
    ok(
      "an_error_the_window_does_not_know_is_said_as_it_came: an unknown failure from the door shows its own words and still loses nothing",
      odd.note.includes("pty closed") && SAME(odd.queue, ["하나", "둘", "셋"]),
      JSON.stringify(odd),
    );
  });
}

/* ④ 바로 보내기, 판: 문이 Esc를 치고(메뉴가 없을 때만), 글은 맨 앞으로 올라 멈춘 뒤 먼저 나간다. */
async function testSendNowPane(browser, origin, ok) {
  await scene(browser, origin, ok, "send now pane", async (page) => {
    const term = await openPane(page, "claude", "working");
    await queueWords(page, "가", "나", "다");
    const seen = {};
    seen.pressed = await press(page, "now", 1);
    seen.stopping = await page.evaluate(LOOK);
    await page.evaluate(async (term) => {
      window.__TELL__({ term, state: "done", agent: "claude" });
      await window.__PAINT__(3);
    }, term);
    await page.waitForFunction(() => window.__ACTS__.calls.some((call) => call.door === "pane_send_now"), null, { timeout: 4000 }).catch(() => null);
    seen.sent = await page.evaluate(LOOK);
    ok(
      "send_now_on_a_pane_stops_through_the_backends_door_and_moves_the_row_to_the_front: 바로 보내기 on the second row asks `pane_interrupt` once, the window types no Esc itself, the row stands first and the others keep their order — and nothing is pasted until the turn has stopped",
      seen.pressed && SAME(seen.stopping.calls, [{ door: "pane_interrupt", term }]) && !seen.stopping.sent.includes(ESCAPE_KEY) &&
        seen.stopping.sent.length === 0 && SAME(seen.stopping.queue, ["나", "가", "다"]),
      JSON.stringify(seen.stopping),
    );
    ok(
      "the_row_sent_now_goes_first_and_the_rest_wait_their_turn: once the pane reports the turn done, the promoted words go to the backend's door `pane_send_now` — once, with the pane and exactly those words, the page pasting and pressing nothing itself (the door looks at the line again) — and the other two keep waiting for the pane's next turn",
      SAME(seen.sent.calls.filter((call) => call.door === "pane_send_now"), [{ door: "pane_send_now", term, text: "나" }]) &&
        seen.sent.sent.length === 0 && SAME(seen.sent.queue, ["가", "다"]),
      JSON.stringify(seen.sent),
    );
  });
}

/* ④-2 문이 거절하면 — 메뉴가 서 있다 — 순서는 처음으로 돌아가고 글은 하나도 안 사라진다. */
async function testSendNowRefused(browser, origin, ok) {
  await scene(browser, origin, ok, "send now refused", async (page) => {
    await openPane(page, "claude", "working");
    await queueWords(page, "가", "나", "다");
    const seen = { tried: [] };
    for (const word of DOOR_REFUSALS) {
      await page.evaluate((word) => { window.__ACTS__.stop = () => { throw word; }; }, word);
      const pressed = await press(page, "now", 1);
      const look = await page.evaluate(LOOK);
      const said = await page.evaluate((word) => window.__WORDS__(word), word);
      seen.tried.push({ word, pressed, queue: look.queue, sent: look.sent.length, note: look.note === said && said.length >= 8 });
    }
    ok(
      "a_refused_send_now_puts_the_order_back_and_says_why: for each refusal the queue is in its first order again, the window typed nothing, and the line under the queue is that refusal's sentence",
      seen.tried.every((one) => one.pressed && SAME(one.queue, ["가", "나", "다"]) && one.sent === 0 && one.note),
      JSON.stringify(seen.tried),
    );
  });
}

/* ④-3 문은 쳤는데 판이 멈췄다는 말이 오지 않는다: 시계가 지나면 사실을 말하고, 글은 맨 앞에 남는다. */
async function testSendNowUnconfirmed(browser, origin, ok) {
  await scene(browser, origin, ok, "send now unconfirmed", async (page) => {
    const term = await openPane(page, "claude", "working");
    await queueWords(page, "가", "나");
    await press(page, "now", 1);
    const due = await page.evaluate(() => (typeof SEND_NOW_STOP_MS === "number" ? SEND_NOW_STOP_MS : null));
    const held = await page.evaluate(({ due }) => {
      const timer = window.__ACTS__.long.find((one) => one.ms === due);
      timer?.fn();
      return Boolean(timer);
    }, { due });
    await page.waitForTimeout(250);
    const watched = await page.evaluate(LOOK);
    const said = await page.evaluate(() => window.__WORDS__("turn-unconfirmed"));
    await page.evaluate(async (term) => {
      window.__TELL__({ term, state: "done", agent: "claude" });
      await window.__PAINT__(3);
    }, term);
    await page.waitForTimeout(900);
    const later = await page.evaluate(LOOK);
    ok(
      "a_stop_nobody_confirms_is_said_after_its_bound_and_loses_nothing: the stop was typed but the pane kept working — after `SEND_NOW_STOP_MS` the queue says the stop was not seen, the promoted words still stand first, and nothing was pasted",
      typeof due === "number" && due >= 10_000 && due < 30_000 && held && watched.note === said && said.length >= 8 &&
        SAME(watched.queue, ["나", "가"]) && watched.sent.length === 0,
      JSON.stringify({ due, held, watched, said }),
    );
    ok(
      "the_promoted_row_still_goes_first_when_the_turn_does_end: when the pane finally reports the turn done, the words that were sent-now go — through the door, not by paste and key — before the one that was ahead of them",
      SAME(later.calls.filter((call) => call.door === "pane_send_now"), [{ door: "pane_send_now", term, text: "나" }]) &&
        later.sent.length === 0 && SAME(later.queue, ["가"]),
      JSON.stringify(later),
    );
  });
}

/* ⑤ zo: 끼워 넣는 길은 채널이라 키가 없고, 같은 문이 부른다. */
async function testZoPane(browser, origin, ok) {
  await scene(browser, origin, ok, "zo pane", async (page) => {
    const term = await openPane(page, "zo", "working");
    await queueWords(page, "하나", "둘");
    const seen = {};
    seen.look = await page.evaluate(LOOK);
    await press(page, "steer", 0);
    seen.steered = await page.evaluate(LOOK);
    ok(
      "a_zo_pane_steers_through_the_same_door: the zo pane's rows have all four buttons, and 끼워 넣기 asks `pane_steer` for that row — the backend takes zo's own channel there, the window types nothing",
      seen.look.rows.every((row) => SAME(row.acts, ACTS)) && SAME(seen.steered.calls, [{ door: "pane_steer", term, text: "하나" }]) &&
        SAME(seen.steered.queue, ["둘"]) && seen.steered.sent.length === 0,
      JSON.stringify({ look: seen.look.rows, steered: seen.steered }),
    );
  });
}

/* ⑥ 확인되지 않은 곳엔 단추가 없고 까닭 한 줄이 있다: Codex 판, 워커·코디의 판, 질문이 떠 있을 때, 쉬는 판. */
async function testNoDoorWhereNoneIsVerified(browser, origin, ok) {
  await scene(browser, origin, ok, "no door where none is verified", async (page) => {
    const seen = {};
    await openPane(page, "codex", "working");
    await queueWords(page, "하나");
    seen.codex = await page.evaluate(LOOK);

    const worker = await openPane(page, "claude", "working");
    await page.evaluate(async (worker) => {
      paneLedger.set(worker, { run: "run-1", worker: "w-1", task: "", ledger: "working" });
      syncWorkerComposers(window.__COMPOSER__().__workerRun);
      await window.__PAINT__(2);
    }, worker);
    await queueWords(page, "워커에게");
    seen.worker = await page.evaluate(LOOK);

    const seat = await openPane(page, "claude", "working");
    await page.evaluate(async (seat) => {
      coordinatorSeats = [...coordinatorSeats, { run: "run-1", name: "coordinator", term: seat, held: true, generation: 1, vacatedMs: null }];
      syncWorkerComposers(window.__COMPOSER__().__workerRun);
      await window.__PAINT__(2);
    }, seat);
    await queueWords(page, "코디에게");
    seen.coordinator = await page.evaluate(LOOK);

    await openPane(page, "claude", "working");
    await queueWords(page, "묻는 중");
    await page.evaluate(async () => {
      window.__TELL__({ term: window.__TERM__, state: "waiting", agent: "claude",
        ask_prompt: { questions: [{ question: "어느 쪽으로 할까요?", header: "방향",
          options: [{ label: "A", description: "가" }, { label: "B", description: "나" }], multi_select: false }] } });
      await window.__PAINT__(3);
    });
    seen.asking = await page.evaluate(LOOK);

    await openPane(page, "claude", "done");
    await page.evaluate(() => {
      const run = window.__COMPOSER__().__workerRun;
      run.queue = ["쉬는 판의 글"];
      run.queueArmed = false;
      syncWorkerComposers(run);
    });
    seen.resting = await page.evaluate(LOOK);

    const only = (look) => look.rows.length >= 1 && look.rows.every((row) => SAME(row.acts, ["edit", "drop"]));
    ok(
      "a_codex_pane_has_no_steer_or_send_now_and_says_why: the Codex pane's row keeps 고치기 and 지우기 only, and one line under the queue says this agent's way of taking words mid-turn is not verified here",
      only(seen.codex) && seen.codex.note.length >= 8,
      JSON.stringify(seen.codex),
    );
    ok(
      "a_worker_pane_and_a_coordinator_pane_never_get_the_two_buttons: a pane a run's worker or coordinator sits in has 고치기 and 지우기 only, and says the ledger drives it — and its sentence is not the Codex one",
      only(seen.worker) && only(seen.coordinator) && seen.worker.note.length >= 8 && seen.coordinator.note.length >= 8 && seen.worker.note !== seen.codex.note,
      JSON.stringify({ worker: seen.worker, coordinator: seen.coordinator }),
    );
    ok(
      "a_standing_question_takes_the_two_buttons_away: while the pane's question stands as its card, the rows have 고치기 and 지우기 only and the line says to answer the question first",
      only(seen.asking) && seen.asking.note.length >= 8,
      JSON.stringify(seen.asking),
    );
    ok(
      "a_queue_that_waits_for_something_else_than_the_turn_offers_no_steer: a resting pane's queue (waiting for the sent turn to start) has no steer or send now — there is no running turn to steer or stop",
      only(seen.resting),
      JSON.stringify(seen.resting),
    );
  });
}

/* 선(wire) 하나를 세우고 `status`·`steer`(그 프로토콜이 끼워 넣기를 받는가)로 둔다. */
async function openWire(page, { protocol, steer, status }) {
  await openPane(page, "claude", "done");
  await page.evaluate(async ({ protocol, steer, status }) => {
    window.__WIRE_LOG__ = {
      found: true, skipped: false, next: 0, turns: [], status, asks: [], live: [], agent: "claude", protocol,
      models: [], modes: [], commands: [], version: "2.1.316", steer,
    };
    window.__ANSWER__.wire_start = (args) => ({ id: 91, agent: args.agent, protocol, version: "2.1.316", model: null, session: args.resume });
    window.__ANSWER__.wire_log = () => window.__WIRE_LOG__;
    window.__ANSWER__.wire_history = () => ({ found: false, next: 0, turns: [] });
    el("view-toggle-chat").click();
    for (let tries = 0; tries < 300 && !tabs.some((tab) => tab.id === "wire:91"); tries += 1) {
      await new Promise((done) => window.__REAL_TIMEOUT__(done, 10));
    }
    await pollHelperPages();
    await window.__PAINT__();
  }, { protocol, steer, status });
}

/* ⑦ 끼워 넣기, 선: Claude Code(stream-json)와 Codex(app-server)는 같은 문 `wire_steer`를 부르고,
 * 그 문이 프로토콜마다 제 말(user 줄, `turn/steer`)로 바꾼다. ACP는 받지 않아 단추가 없다. */
async function testWireSteer(browser, origin, ok) {
  for (const protocol of ["claude-stream", "app-server"]) {
    await scene(browser, origin, ok, `wire steer ${protocol}`, async (page) => {
      await openWire(page, { protocol, steer: true, status: "working" });
      await queueWords(page, "선에 하나", "선에 둘");
      const seen = {};
      seen.look = await page.evaluate(LOOK);
      await press(page, "steer", 1);
      seen.steered = await page.evaluate(LOOK);
      await page.evaluate(() => { window.__ACTS__.wireSteer = () => { throw "turn-idle"; }; });
      await press(page, "steer", 0);
      seen.refused = await page.evaluate(LOOK);
      const said = await page.evaluate(() => window.__WORDS__("turn-idle"));
      ok(
        `a_${protocol.replace("-", "_")}_wire_steers_through_wire_steer: while the wire works the rows have all four buttons, and 끼워 넣기 asks \`wire_steer\` with the wire's id and that row's words — the page sends no \`wire_send\` and types nothing`,
        seen.look.rows.length === 2 && seen.look.rows.every((row) => SAME(row.acts, ACTS)) &&
          SAME(seen.steered.calls, [{ door: "wire_steer", id: 91, text: "선에 둘" }]) && SAME(seen.steered.queue, ["선에 하나"]) &&
          seen.steered.sent.length === 0,
        JSON.stringify({ look: seen.look.rows, steered: seen.steered }),
      );
      ok(
        `a_${protocol.replace("-", "_")}_wire_that_says_no_turn_runs_keeps_the_words: the door answering that no turn is running leaves the row where it stood and says so`,
        SAME(seen.refused.queue, ["선에 하나"]) && seen.refused.note === said && said.length >= 8,
        JSON.stringify({ refused: seen.refused, said }),
      );
    });
  }
  await scene(browser, origin, ok, "wire steer acp", async (page) => {
    await openWire(page, { protocol: "acp", steer: false, status: "working" });
    await queueWords(page, "ACP에");
    const look = await page.evaluate(LOOK);
    ok(
      "a_wire_that_cannot_take_words_mid_turn_has_no_steer: the wire's own log says it does not steer (ACP has no such request) — the row has no 끼워 넣기, keeps 바로 보내기, and a line says why",
      look.rows.length === 1 && SAME(look.rows[0].acts, ["edit", "now", "drop"]) && look.note.length >= 8,
      JSON.stringify(look),
    );
  });
}

/* ⑧ 바로 보내기, 선: `wire_interrupt` 다음 선이 쉬어야 그 글이 `wire_send`로 나간다 — 멈추기 전에 보내면
 * Codex의 열린 턴에 섞여 들어간다. */
async function testWireSendNow(browser, origin, ok) {
  await scene(browser, origin, ok, "wire send now", async (page) => {
    await openWire(page, { protocol: "app-server", steer: true, status: "working" });
    await queueWords(page, "첫째", "둘째", "셋째");
    const seen = {};
    seen.pressed = await press(page, "now", 2);
    seen.stopping = await page.evaluate(LOOK);
    await page.evaluate(async () => {
      window.__WIRE_LOG__ = { ...window.__WIRE_LOG__, status: "idle" };
      await pollHelperPages();
      await window.__PAINT__(3);
    });
    await page.waitForTimeout(500);
    seen.stopped = await page.evaluate(LOOK);
    ok(
      "send_now_on_a_wire_interrupts_first_and_sends_nothing_until_the_wire_rests: 바로 보내기 on the third row asks `wire_interrupt` once, sends no `wire_send` while the turn still runs, and the row stands first",
      seen.pressed && SAME(seen.stopping.calls, [{ door: "wire_interrupt", id: 91 }]) && SAME(seen.stopping.queue, ["셋째", "첫째", "둘째"]),
      JSON.stringify(seen.stopping),
    );
    ok(
      "the_promoted_words_go_out_once_the_wire_has_stopped: when the wire reports it rests, the promoted words are sent through `wire_send` — one request — and the other two keep waiting",
      SAME(seen.stopped.calls.filter((call) => call.door === "wire_send"), [{ door: "wire_send", id: 91, text: "셋째" }]) &&
        SAME(seen.stopped.queue, ["첫째", "둘째"]),
      JSON.stringify(seen.stopped),
    );
  });
  await scene(browser, origin, ok, "wire asking", async (page) => {
    await openWire(page, { protocol: "claude-stream", steer: true, status: "working" });
    await queueWords(page, "질문 아래");
    await page.evaluate(async () => {
      window.__WIRE_LOG__ = { ...window.__WIRE_LOG__, status: "asking", asks: [{ id: "a1", kind: "approval", tool: "Bash", summary: "ls", options: [] }] };
      await pollHelperPages();
      await window.__PAINT__(3);
    });
    const look = await page.evaluate(LOOK);
    ok(
      "a_wire_waiting_for_an_answer_takes_the_two_buttons_away: with an approval standing as the wire's card the row keeps 고치기 and 지우기 only and says to answer first",
      look.rows.length === 1 && SAME(look.rows[0].acts, ["edit", "drop"]) && look.note.length >= 8,
      JSON.stringify(look),
    );
  });
}

/* ⑨ 단축키: 첫 행에 듣는다. 이 창의 기존 단축키·입력칸의 키와 겹치지 않는다. */
async function testShortcuts(browser, origin, ok) {
  await scene(browser, origin, ok, "shortcuts", async (page) => {
    const term = await openPane(page, "claude", "working");
    const seen = {};
    seen.table = await page.evaluate(() => {
      const chords = ACTIONS.flatMap((action) => chordsFor(action).map((chord) => ({ id: action.id, chord })));
      const byChord = new Map();
      for (const one of chords) byChord.set(one.chord, [...(byChord.get(one.chord) ?? []), one.id]);
      return {
        steer: chordsForId("composer.queueSteer"),
        now: chordsForId("composer.queueSendNow"),
        shared: [...byChord.entries()].filter(([, ids]) => ids.length > 1).map(([chord, ids]) => ({ chord, ids })),
        // 입력칸이 스스로 쓰는 키: Enter(보내기)·Ctrl+J·Shift+Tab·화살표·Esc는 mod 없는 키라 겹칠 수 없고,
        // mod를 쓰는 입력칸의 키는 없다 — 그 사실을 표로 못 박는다.
        composerOwn: ["enter", "ctrl+j", "shift+tab", "arrowup", "arrowdown", "escape"],
      };
    });
    // Nor does any other place in the window's source spell either chord: the scripts as the page loaded them.
    seen.literals = await page.evaluate(async () => {
      const sources = await Promise.all([...document.scripts]
        .filter((script) => script.src && !script.src.endsWith("shell-i18n.js"))
        .map((script) => fetch(script.src).then((response) => response.text())));
      const count = (chord) => sources.reduce((sum, text) => sum + text.split(`"${chord}"`).length - 1, 0);
      return { steer: count("mod+alt+enter"), now: count("mod+alt+shift+enter"), scripts: sources.length };
    });
    ok(
      "the_two_chords_are_named_actions_and_no_chord_in_the_window_is_shared: 끼워 넣기 and 바로 보내기 are registered actions (⌥⌘↵ and ⇧⌥⌘↵ on the Mac, Ctrl+Alt+Enter and Ctrl+Alt+Shift+Enter elsewhere), no two actions share a chord, and no other place in the window's scripts spells either",
      SAME(seen.table.steer, ["mod+alt+enter"]) && SAME(seen.table.now, ["mod+alt+shift+enter"]) && seen.table.shared.length === 0 &&
        seen.literals.scripts >= 20 && seen.literals.steer === 1 && seen.literals.now === 1,
      JSON.stringify({ table: seen.table, literals: seen.literals }),
    );
    await queueWords(page, "첫 글", "둘째 글");
    const press = (shift) => page.evaluate(({ shift }) => {
      const box = window.__COMPOSER__().querySelector(".worker-composer-box");
      box.focus();
      const event = new KeyboardEvent("keydown", { key: "Enter", code: "Enter", altKey: true, shiftKey: shift, bubbles: true, cancelable: true, ...window.__TEST_PRIMARY_EVENT__ });
      box.dispatchEvent(event);
      return event.defaultPrevented;
    }, { shift });
    seen.steerPrevented = await press(false);
    await page.waitForTimeout(250);
    seen.steered = await page.evaluate(LOOK);
    await page.evaluate(() => { window.__ACTS__.calls.length = 0; });
    seen.nowPrevented = await press(true);
    await page.waitForTimeout(250);
    seen.stopped = await page.evaluate(LOOK);
    ok(
      "the_steer_chord_acts_on_the_first_queued_row: ⌥⌘↵ in the box steers the first row — `pane_steer` with its words — and the key is not left to type a newline or send the box",
      seen.steerPrevented && SAME(seen.steered.calls, [{ door: "pane_steer", term, text: "첫 글" }]) && SAME(seen.steered.queue, ["둘째 글"]) &&
        seen.steered.box === "",
      JSON.stringify(seen.steered),
    );
    ok(
      "the_send_now_chord_acts_on_the_first_queued_row: ⇧⌥⌘↵ in the box asks `pane_interrupt` for the first row",
      seen.nowPrevented && SAME(seen.stopped.calls, [{ door: "pane_interrupt", term }]),
      JSON.stringify(seen.stopped),
    );
    // 대기열이 없으면 키는 아무것도 하지 않고, 상자의 글을 보내지도 않는다.
    await page.evaluate(() => { window.__COMPOSER__().__workerRun.queue.length = 0; syncWorkerComposers(window.__COMPOSER__().__workerRun); });
    await page.locator(".worker-composer-box:visible").first().fill("초안");
    await page.evaluate(() => { window.__ACTS__.calls.length = 0; });
    seen.idleSteer = await press(false);
    seen.idleNow = await press(true);
    await page.waitForTimeout(200);
    seen.idle = await page.evaluate(LOOK);
    ok(
      "the_chords_do_nothing_without_a_queue_and_never_send_the_box: with an empty queue neither chord calls a door, and the draft in the box is neither sent nor queued",
      seen.idle.calls.length === 0 && seen.idle.box === "초안" && seen.idle.queue.length === 0 && seen.idle.sent.length === 0,
      JSON.stringify(seen.idle),
    );
    // 첫 행 단추의 말풍선이 그 키를 말한다(다른 행은 말하지 않는다).
    await queueWords(page, "또 하나", "또 둘");
    seen.tips = await page.evaluate(() => [...window.__COMPOSER__().querySelectorAll(".composer-queue-item")].map((row) => ({
      steer: row.querySelector('[data-act="steer"]')?.dataset.tip ?? "",
      now: row.querySelector('[data-act="now"]')?.dataset.tip ?? "",
    })));
    seen.labels = await page.evaluate(() => ({ steer: chordLabel("mod+alt+enter"), now: chordLabel("mod+alt+shift+enter") }));
    ok(
      "only_the_first_rows_tips_name_the_chord: the first row's steer and send-now tips end with the chord as the window writes it, and the second row's tips name none",
      seen.tips.length === 2 && seen.tips[0].steer.includes(seen.labels.steer) && seen.tips[0].now.includes(seen.labels.now) &&
        !seen.tips[1].steer.includes(seen.labels.steer) && !seen.tips[1].now.includes(seen.labels.now),
      JSON.stringify({ tips: seen.tips, labels: seen.labels }),
    );
  });
}

/* ⑩ 좁은 폭(390): 단추는 아이콘만 보이고 이름은 남는다. 넓으면 짧은 낱말이 아이콘 옆에 보인다. */
async function testNarrowAndWide(browser, origin, ok) {
  const measure = (page) => page.evaluate(() => {
    const row = window.__COMPOSER__()?.querySelector(".composer-queue-item");
    const queue = window.__COMPOSER__()?.querySelector(".composer-queue");
    return {
      buttons: [...(row?.querySelectorAll("button[data-act]") ?? [])].map((button) => {
        const after = getComputedStyle(button, "::after");
        const box = button.getBoundingClientRect();
        return {
          act: button.dataset.act,
          visibleWord: after.display !== "none" && after.content !== "none" && after.content !== "normal" ? after.content.replace(/^"|"$/g, "") : "",
          label: button.getAttribute("aria-label"),
          width: Math.round(box.width),
          height: Math.round(box.height),
        };
      }),
      overflow: row ? row.scrollWidth - row.clientWidth : null,
      queueOverflow: queue ? queue.scrollWidth - queue.clientWidth : null,
    };
  });
  for (const [name, width, height] of [["wide", 1440, 900], ["narrow", 390, 844]]) {
    await scene(browser, origin, ok, `row at ${name}`, async (page) => {
      await openPane(page, "claude", "working");
      await queueWords(page, "아주 긴 대기 글은 한 줄에서 잘려야 하고 단추는 어느 폭에서도 눌릴 수 있어야 합니다");
      const seen = await measure(page);
      const wants = await page.evaluate(() => ({ steer: t("composer.queue.steer.short", "끼워 넣기"), now: t("composer.queue.now.short", "바로 보내기"), edit: t("composer.queue.edit.short", "고치기") }));
      const worded = seen.buttons.filter((button) => button.act !== "drop");
      if (name === "wide") {
        ok(
          "the_wide_row_shows_each_action_in_a_short_word_beside_its_icon: at a wide pane 고치기, 끼워 넣기 and 바로 보내기 each show their short word, the ✕ shows none, and the row does not overflow",
          worded.length === 3 && SAME(worded.map((button) => button.visibleWord), [wants.edit, wants.steer, wants.now]) &&
            seen.buttons.find((button) => button.act === "drop")?.visibleWord === "" && seen.overflow === 0,
          JSON.stringify({ seen, wants }),
        );
      } else {
        ok(
          "the_narrow_row_shows_icons_only_and_keeps_every_name: at 390 px the four buttons show no word, each still has its accessible name, each is at least 24 px square to press, and nothing overflows sideways",
          seen.buttons.length === 4 && seen.buttons.every((button) => button.visibleWord === "" && (button.label ?? "").length >= 2 && button.width >= 24 && button.height >= 24) &&
            (seen.overflow ?? 1) <= 0 && (seen.queueOverflow ?? 1) <= 0,
          JSON.stringify(seen),
        );
      }
    }, { width, height });
  }
}

/* ⑪ 다섯 언어: 단추의 말이 영어로 남지 않는다(영어 판만 영어). */
async function testFiveLanguages(browser, origin, ok) {
  const words = {};
  for (const locale of ["ko", "en", "ja", "zh", "es"]) {
    await scene(browser, origin, ok, `language ${locale}`, async (page) => {
      await page.evaluate(async (locale) => { await setLocale(locale, { refresh: false, persist: false }); }, locale);
      await openPane(page, "claude", "working");
      await queueWords(page, "x");
      words[locale] = await page.evaluate(() => [...window.__COMPOSER__().querySelectorAll(".composer-queue-item button[data-act]")].map((button) => ({
        act: button.dataset.act, label: button.getAttribute("aria-label") ?? "", word: button.dataset.words ?? "",
      })));
    });
  }
  // A language chosen while rows stand: the sweep that says every key-bearing tooltip again says theirs.
  await scene(browser, origin, ok, "language switched mid-session", async (page) => {
    await openPane(page, "claude", "working");
    await queueWords(page, "x");
    const read = () => page.evaluate(() => [...window.__COMPOSER__().querySelectorAll(".composer-queue-item button[data-act]")].map((button) => ({
      act: button.dataset.act, label: button.getAttribute("aria-label") ?? "", word: button.dataset.words ?? "",
    })));
    const before = await read();
    await page.evaluate(async () => { await setLocale("ja", { refresh: false, persist: false }); });
    await page.waitForTimeout(150);
    words.switched = { before, after: await read() };
  });
  const english = /steer|send now/i;
  const named = (locale) => words[locale]?.filter((one) => one.act !== "drop") ?? [];
  ok(
    "every_language_names_the_three_actions: in each of ko, en, ja, zh and es the edit, steer and send-now buttons carry a label and a short word",
    ["ko", "en", "ja", "zh", "es"].every((locale) => named(locale).length === 3 && named(locale).every((one) => one.label.length >= 2 && one.word.length >= 1)),
    JSON.stringify(words),
  );
  ok(
    "no_language_but_english_shows_the_english_words: the ko, ja, zh and es labels and short words never contain 「Steer」 or 「Send Now」, and each differs from the English one",
    named("en").length === 3 && ["ko", "ja", "zh", "es"].every((locale) => named(locale).length === 3 && named(locale).every((one, at) =>
      !english.test(one.label) && !english.test(one.word) && one.label !== named("en")[at]?.label && one.word !== named("en")[at]?.word)),
    JSON.stringify(words),
  );
  ok(
    "a_language_chosen_while_rows_stand_says_their_buttons_again: the rows drawn in Korean are said in Japanese after the switch — every label and short word of the three actions equals what a page opened in Japanese says",
    words.switched?.before?.length === 4 && words.switched.after.length === 4 &&
      SAME(words.switched.after.filter((one) => one.act !== "drop").map((one) => [one.act, one.word]), named("ja").map((one) => [one.act, one.word])) &&
      words.switched.after.filter((one) => one.act !== "drop").every((one, at) => one.label.startsWith(named("ja")[at].label.split(" — ")[0].split(" (")[0])) &&
      words.switched.after.some((one, at) => one.label !== words.switched.before[at].label),
    JSON.stringify(words.switched),
  );
  ok(
    "the_english_page_says_steer_and_send_now_in_english: the en labels and words are English — the steer one says steer, the send-now one says send now",
    /steer/i.test(named("en").find((one) => one.act === "steer")?.word ?? "") && /send now/i.test(named("en").find((one) => one.act === "now")?.word ?? ""),
    JSON.stringify(named("en")),
  );
}

/* ⑫ 행이 많아도: 이어 붙이기·하나 지우기는 그 행만 건드리고, 아무것도 안 바뀐 그림은 한 글자도 쓰지 않는다. */
async function testManyRows(browser, origin, ok) {
  await scene(browser, origin, ok, "many rows", async (page) => {
    await openPane(page, "claude", "working");
    const seen = await page.evaluate(async () => {
      const run = window.__COMPOSER__().__workerRun;
      const queue = () => window.__COMPOSER__().querySelector(".composer-queue");
      const watch = () => {
        const records = [];
        const observer = new MutationObserver((list) => records.push(...list));
        observer.observe(queue(), { subtree: true, childList: true, attributes: true, characterData: true });
        return { stop: () => { observer.takeRecords().forEach((one) => records.push(one)); observer.disconnect(); return records; } };
      };
      const weigh = (nodes) => [...nodes].reduce((n, node) => n + (node.nodeType === 1 ? 1 + node.querySelectorAll("*").length : 0), 0);
      const count = (records) => ({
        records: records.length,
        added: records.reduce((sum, one) => sum + weigh(one.addedNodes), 0),
        removed: records.reduce((sum, one) => sum + [...one.removedNodes].filter((node) => node.nodeType === 1).length, 0),
      });
      const rows = 200;
      const began = performance.now();
      run.queue = Array.from({ length: rows }, (_, at) => `대기 글 ${at}`);
      syncWorkerComposers(run);
      await window.__PAINT__(2);
      const first = performance.now() - began;
      const items = queue().querySelectorAll(".composer-queue-item").length;
      const elements = [...queue().querySelectorAll(".composer-queue-item")].reduce((sum, row) => sum + 1 + row.querySelectorAll("*").length, 0);
      let watching = watch();
      for (let beat = 0; beat < 3; beat += 1) syncWorkerComposers(run);
      await window.__PAINT__(1);
      const quiet = count(watching.stop());
      watching = watch();
      const dropAt = performance.now();
      queue().querySelectorAll(".composer-queue-item")[100].querySelector("button[data-act=drop]")?.click();
      await window.__PAINT__(1);
      const dropMs = performance.now() - dropAt;
      const dropped = count(watching.stop());
      watching = watch();
      run.queue.push("맨 끝에 붙는 글");
      syncWorkerComposers(run);
      await window.__PAINT__(1);
      const appended = count(watching.stop());
      return { rows, items, elements, perRow: Math.round((elements / Math.max(items, 1)) * 10) / 10, first: Math.round(first), dropMs: Math.round(dropMs), quiet, dropped, appended, left: run.queue.length };
    });
    ok(
      "a_queue_of_two_hundred_rows_stays_light_and_changes_only_what_moved: 200 rows draw at most 14 elements each; an unchanged repaint writes nothing; dropping one middle row removes exactly that row and creates no node; appending one row creates one row's nodes only",
      seen.items === 200 && seen.perRow <= 14 && seen.quiet.records === 0 &&
        seen.dropped.added === 0 && seen.dropped.removed === 1 && seen.dropped.records <= 6 && seen.left === 200 &&
        seen.appended.added <= 14 && seen.appended.removed === 0,
      JSON.stringify(seen),
    );
  });
}


/* ---- r2: 비저자 검수 r1 (P1-1, P2-1, P2-2, P3) --------------------------------------------- */

/* WebKit(macOS)은 마우스로 누른 단추에 초점을 주지 않고, 누른 순간 있던 초점을 거둔다 — Playwright의
 * Chromium은 단추에 초점을 준다. 앱은 WKWebView에서 도니, 시험이 그 모양을 흉내 낸다. */
const WEBKIT_BUTTONS = () => {
  document.addEventListener("mousedown", (event) => {
    const button = event.target instanceof Element ? event.target.closest("button") : null;
    if (!button) return;
    event.preventDefault();
    document.activeElement?.blur?.();
  }, true);
};

/* ⑬ P1-1 — 단추를 누른 뒤 행이 사라지거나 단추가 꺼져도 초점은 입력칸 안에 있고, 이어서 치는 키는 어느
 * 판에도 닿지 않는다. 키보드(Enter), 마우스, WebKit처럼 단추에 초점이 안 가는 마우스, 네 단추 모두.
 * 이전에는 초점이 body로 떨어져 다음 키가 `rearmKeySink`를 거쳐 대화 뒤의 판으로 갔다(`answer_door` 없이). */
async function testFocusAfterPress(browser, origin, ok) {
  for (const how of ["keyboard", "mouse", "mouse like WebKit"]) {
    for (const act of ["edit", "steer", "now", "drop"]) {
      await scene(browser, origin, ok, `${how} ${act}`, async (page) => {
        const term = await openPane(page, "claude", "working");
        await queueWords(page, "하나", "둘", "셋");
        if (how === "mouse like WebKit") await page.evaluate(WEBKIT_BUTTONS);
        if (how === "keyboard") {
          await page.evaluate((act) => window.__ROWBTN__(1, act).focus(), act);
          await page.keyboard.press("Enter");
        } else {
          await page.locator(".composer-queue-item").nth(1).locator(`button[data-act="${act}"]`).click();
        }
        await page.waitForTimeout(250);
        const pressed = await page.evaluate(LOOK);
        await page.keyboard.press("x");
        const typed = await page.evaluate(LOOK);
        if (typed.focus.inBox) await page.keyboard.press("Enter");
        await page.waitForTimeout(150);
        const end = await page.evaluate(LOOK);
        const next = act === "drop" && how !== "mouse like WebKit";
        const door = { edit: [], steer: [{ door: "pane_steer", term, text: "둘" }], now: [{ door: "pane_interrupt", term }], drop: [] }[act];
        const queue = { edit: ["하나", "둘", "셋"], steer: ["하나", "셋"], now: ["둘", "하나", "셋"], drop: ["하나", "셋"] }[act];
        ok(
          `after_a_${act}_pressed_by_${how.replaceAll(" ", "_")}_the_focus_stays_in_the_composer_and_no_key_reaches_a_pane: the press did its work (${act}), the focus is on ${next ? "the next row's ✕" : "the box"} — never on the page — and a letter and an Enter typed after it reached no pane (no key, no text, no paste)`,
          SAME(pressed.queue, queue) && SAME(pressed.calls, door) && !pressed.focus.body && pressed.focus.inComposer &&
            (next ? pressed.focus.act === "drop" && pressed.focus.row === 1 : pressed.focus.inBox) &&
            (act !== "edit" || pressed.box === "둘") && REACHED(end.sent).length === 0 && !end.focus.body && end.focus.inComposer,
          JSON.stringify({ pressed: { queue: pressed.queue, calls: pressed.calls, focus: pressed.focus, box: pressed.box }, typed: typed.focus, end: { sent: end.sent, focus: end.focus } }),
        );
      });
    }
  }
}

/* ⑭ P1-1의 뿌리 — 대화가 판 앞에 서 있으면 키보드는 대화의 것이다. 아무것도 초점이 없을 때 치는 키가 보이지 않는
 * 판으로 가지 않고(`frontKeyboardTarget`), 판의 화면이 앞이면 키는 그대로 판으로 간다. */
async function testKeyboardFollowsTheScreen(browser, origin, ok) {
  await scene(browser, origin, ok, "keyboard follows the screen", async (page) => {
    const term = await openPane(page, "claude", "working");
    await page.evaluate(() => document.activeElement?.blur?.());
    for (const key of ["x", "Enter", "ArrowUp", "ArrowDown", "Backspace"]) await page.keyboard.press(key);
    await page.waitForTimeout(150);
    // r3 P3-8: the first letter now takes the focus to the conversation's own box (it used to land in the terminal's sink and be
    // lost), so "on the page" is the body, the sink or that box — never a terminal.
    const chat = await page.evaluate(() => ({
      sent: [...window.__ACTS__.sent],
      onPage: document.activeElement === document.body || document.activeElement === keySink ||
        document.activeElement?.classList?.contains("worker-composer-box") === true,
    }));
    // 판의 화면으로 돌렸다: 같은 키가 이제는 판의 것이다.
    await page.evaluate(async (term) => {
      await setPaneChat(term, false);
      await window.__PAINT__(2);
      window.__ACTS__.sent.length = 0;
      keySink.focus();
    }, term);
    await page.keyboard.press("y");
    await page.keyboard.press("Enter");
    await page.waitForTimeout(150);
    const screen = await page.evaluate(() => [...window.__ACTS__.sent]);
    ok(
      "no_key_goes_to_a_screen_nobody_sees: with the pane's conversation in front and nothing focused, a letter, Enter, the arrows and Backspace reach no pane — they would land on a terminal under the conversation (r2 P1-1, the root: the keyboard follows the screen)",
      chat.onPage && REACHED(chat.sent).length === 0,
      JSON.stringify(chat),
    );
    ok(
      "the_panes_screen_in_front_still_takes_its_keys: with the pane's own screen in front the same keys reach the pane as before — a letter as text, Enter as a key",
      screen.some((one) => one === "text:y") && screen.some((one) => one.startsWith("key:") && one.includes("Enter")),
      JSON.stringify(screen),
    );
  });
}

/* ⑮ P2-1 — 바로 보내기의 멈춤이 나간 뒤에는 턴이 끝나거나 12초 시계가 끝날 때까지 그 대기열의 끼워 넣기와 바로
 * 보내기가 꺼진다. 더블클릭의 둘째 눌림도, 단축키도, 직접 부른 함수도 문을 다시 부르지 않고, 쉬는 입력줄에
 * Esc 두 번이 가는 길이 없다. */
async function testSendNowLock(browser, origin, ok) {
  await scene(browser, origin, ok, "send now lock", async (page) => {
    const term = await openPane(page, "claude", "working");
    await queueWords(page, "가", "나", "다");
    const seen = {};
    seen.pressed = await press(page, "now", 1);
    seen.first = await page.evaluate(LOOK);
    // 둘째 눌림이 갈 수 있는 모든 길: 더블클릭, 두 단축키, 직접 부른 함수.
    await page.locator(".composer-queue-item").nth(2).locator('button[data-act="now"]').dblclick({ force: true }).catch(() => null);
    await page.evaluate(() => {
      window.__CHORD__(false);
      window.__CHORD__(true);
      const run = window.__COMPOSER__().__workerRun;
      for (const [act, at] of [["now", 0], ["steer", 0], ["now", 2], ["steer", 1]]) actOnComposerQueue(run, act, at);
    });
    await page.waitForTimeout(300);
    seen.locked = await page.evaluate(LOOK);
    ok(
      "after_the_stop_went_out_no_second_stop_or_steer_reaches_the_pane: 바로 보내기 asked `pane_interrupt` once; a double click, both chords and direct calls afterwards asked no door, typed no key (not even a second Esc a resting line would turn into the CLI's rewind), and the line under the queue did not say the first stop was not sent",
      seen.pressed && SAME(seen.first.calls, [{ door: "pane_interrupt", term }]) && SAME(seen.locked.calls, [{ door: "pane_interrupt", term }]) &&
        REACHED(seen.locked.sent).length === 0 && SAME(seen.locked.queue, ["나", "가", "다"]) && seen.locked.note === "",
      JSON.stringify({ first: seen.first.calls, locked: { calls: seen.locked.calls, sent: seen.locked.sent, queue: seen.locked.queue, note: seen.locked.note } }),
    );
    ok(
      "while_the_stop_is_out_every_rows_steer_and_send_now_are_off_and_edit_and_drop_are_not: the buttons of all three rows that can steer or send now are disabled, and 고치기 and ✕ stay pressable",
      seen.locked.rows.length === 3 && seen.locked.rows.every((row) => SAME(row.off, ["steer", "now"])),
      JSON.stringify(seen.locked.rows),
    );
    // 12초 시계: 멈춤을 못 봤다고 말하고, 단추가 돌아오고, 두 번째 멈춤은 그때부터 가능하다.
    const due = await page.evaluate(() => SEND_NOW_STOP_MS);
    await page.evaluate((due) => { window.__ACTS__.long.find((one) => one.ms === due)?.fn(); }, due);
    await page.waitForTimeout(250);
    seen.released = await page.evaluate(LOOK);
    const said = await page.evaluate(() => window.__WORDS__("turn-unconfirmed"));
    await press(page, "now", 2);
    seen.second = await page.evaluate(LOOK);
    ok(
      "when_the_clock_runs_out_the_buttons_come_back_and_a_second_stop_may_go: after the bound the queue says the stop was not seen, no button is off, and 바로 보내기 on another row asks the door again",
      seen.released.note === said && seen.released.rows.every((row) => row.off.length === 0) &&
        seen.second.calls.filter((call) => call.door === "pane_interrupt").length === 2 && REACHED(seen.second.sent).length === 0,
      JSON.stringify({ note: seen.released.note, off: seen.released.rows.map((row) => row.off), calls: seen.second.calls }),
    );
  });
  // 더블클릭: 첫 눌림이 행을 맨 앞으로 올리고, 같은 자리의 둘째 눌림은 다른 글을 올리지 않는다. 진짜 문은 몇
  // ms 뒤에 답하고 둘째 눌림은 그 뒤에 온다 — 스텁은 바로 답하니, 둘 사이의 틈은 여기서 만든다.
  await scene(browser, origin, ok, "double click", async (page) => {
    const term = await openPane(page, "claude", "working");
    await queueWords(page, "가", "나", "다");
    const spot = await page.locator(".composer-queue-item").nth(2).locator('button[data-act="now"]').boundingBox();
    const at = { x: spot.x + spot.width / 2, y: spot.y + spot.height / 2 };
    await page.mouse.click(at.x, at.y);
    await page.waitForTimeout(120);
    await page.mouse.click(at.x, at.y);
    await page.waitForTimeout(300);
    const seen = await page.evaluate(LOOK);
    ok(
      "a_double_click_on_send_now_sends_one_stop_and_raises_one_row: the second press of a double click did not raise the row that took the first one's place — one `pane_interrupt`, the clicked row first, the others in their order",
      SAME(seen.calls, [{ door: "pane_interrupt", term }]) && SAME(seen.queue, ["다", "가", "나"]) && seen.note === "",
      JSON.stringify({ calls: seen.calls, queue: seen.queue, note: seen.note }),
    );
  });
  // 턴이 끝나면 걸쇠는 풀린다: 글이 나가고, 다음 턴이 돌 때 남은 행의 두 단추가 다시 눌린다.
  await scene(browser, origin, ok, "lock ends with the turn", async (page) => {
    const term = await openPane(page, "claude", "working");
    await queueWords(page, "가", "나", "다");
    await press(page, "now", 1);
    await page.evaluate(async (term) => {
      window.__TELL__({ term, state: "done", agent: "claude" });
      await window.__PAINT__(3);
    }, term);
    await page.waitForTimeout(300);
    await page.evaluate(async (term) => {
      window.__TELL__({ term, state: "working", agent: "claude" });
      await window.__PAINT__(3);
    }, term);
    const seen = await page.evaluate(LOOK);
    await press(page, "steer", 0);
    seen.steered = await page.evaluate(LOOK);
    ok(
      "the_lock_ends_with_the_turn_that_was_stopped: the stopped turn's end sent the promoted words, and when the next turn runs the remaining rows steer and send now again",
      seen.calls.filter((call) => call.door === "pane_send_now").length === 1 && seen.rows.every((row) => row.off.length === 0) &&
        SAME(seen.steered.calls.filter((call) => call.door === "pane_steer"), [{ door: "pane_steer", term, text: "가" }]),
      JSON.stringify({ rows: seen.rows, calls: seen.steered.calls }),
    );
  });
}

/* ⑯ P2-2 — 바로 보내기로 앞에 온 글은 멈춘 뒤 문(`pane_send_now`)이 입력줄이 비었는지 다시 보고 친다. 문이 거절하면
 * 글은 대기열 맨 앞에 남고 까닭이 보이며, 창은 키도 붙여넣기도 치지 않는다. */
async function testSendNowDelivery(browser, origin, ok) {
  const stopped = async (page, term) => {
    await page.evaluate(async (term) => {
      window.__TELL__({ term, state: "done", agent: "claude" });
      await window.__PAINT__(3);
    }, term);
    await page.waitForTimeout(350);
  };
  const again = async (page, term) => {
    await page.evaluate(async (term) => {
      window.__TELL__({ term, state: "working", agent: "claude" });
      await window.__PAINT__(2);
      window.__TELL__({ term, state: "done", agent: "claude" });
      await window.__PAINT__(3);
    }, term);
    await page.waitForTimeout(350);
  };
  await scene(browser, origin, ok, "send now delivery refused", async (page) => {
    const term = await openPane(page, "claude", "working");
    await queueWords(page, "가", "나", "다");
    await page.evaluate(() => { window.__ACTS__.front = () => { throw "turn-line"; }; });
    await press(page, "now", 1);
    await stopped(page, term);
    const held = await page.evaluate(LOOK);
    const said = await page.evaluate(() => window.__WORDS__("turn-line"));
    await page.waitForTimeout(700);
    const quiet = await page.evaluate(LOOK);
    await page.evaluate(() => { window.__ACTS__.front = null; });
    await again(page, term);
    const sent = await page.evaluate(LOOK);
    const front = (look) => look.calls.filter((call) => call.door === "pane_send_now");
    ok(
      "a_refused_delivery_keeps_the_words_first_in_the_queue_and_says_why: the door said the line holds words, so the sent-now words stood first again, the window pasted and pressed nothing, and the line under the queue is the sentence for that refusal",
      SAME(front(held), [{ door: "pane_send_now", term, text: "나" }]) && SAME(held.queue, ["나", "가", "다"]) && held.sent.length === 0 &&
        held.note === said && said.length >= 8 && !said.includes("turn-"),
      JSON.stringify({ calls: front(held), queue: held.queue, sent: held.sent, note: held.note, said }),
    );
    ok(
      "a_refused_delivery_does_not_ask_again_on_its_own_and_goes_when_the_pane_next_moves: no timer asked the door a second time; the pane's next turn edge did, the door took the words, and the others kept waiting",
      front(quiet).length === 1 && front(sent).length === 2 && SAME(sent.queue, ["가", "다"]) && sent.sent.length === 0,
      JSON.stringify({ quiet: front(quiet).length, sent: front(sent).length, queue: sent.queue, raw: sent.sent }),
    );
  });
  // 앞서 끼워 넣은 글과 붙어 나가는 경우: A를 끼워 넣고, B를 바로 보내면, 멈춤이 A를 입력줄로 되돌려 문이 B를 거절한다.
  await scene(browser, origin, ok, "steered then sent now", async (page) => {
    const term = await openPane(page, "claude", "working");
    await queueWords(page, "A 글", "B 글");
    await press(page, "steer", 0);
    await press(page, "now", 0);
    await page.evaluate(() => { window.__ACTS__.front = () => { throw "turn-line"; }; });
    await stopped(page, term);
    const seen = await page.evaluate(LOOK);
    ok(
      "words_steered_before_a_send_now_are_never_joined_by_it: A was steered, B was sent now and the CLI took A back onto its line — the door refused B, the window typed nothing behind A, and B stands first in the queue",
      SAME(seen.calls.map((call) => call.door), ["pane_steer", "pane_interrupt", "pane_send_now"]) && REACHED(seen.sent).length === 0 && SAME(seen.queue, ["B 글"]),
      JSON.stringify(seen),
    );
  });
  // 그 밖의 거절·오류: 글은 어느 것에서도 사라지지 않는다 — 한 행이 맨 앞에서 세 가지 끝을 차례로 맞는다.
  await scene(browser, origin, ok, "send now delivery other ends", async (page) => {
    const term = await openPane(page, "claude", "working");
    await queueWords(page, "가", "나");
    const seen = {};
    await press(page, "now", 1);
    await page.evaluate(() => { window.__ACTS__.front = () => { throw new Error("pty closed"); }; });
    await stopped(page, term);
    seen.odd = await page.evaluate(LOOK);
    await page.evaluate(() => { window.__ACTS__.front = () => { throw "turn-running"; }; });
    await again(page, term);
    seen.running = await page.evaluate(LOOK);
    await page.evaluate(() => { window.__ACTS__.front = () => { throw "turn-landed"; }; });
    await again(page, term);
    seen.landed = await page.evaluate(LOOK);
    ok(
      "a_delivery_that_failed_before_anything_was_typed_keeps_the_words_first_and_says_what_came: an error the window does not know and the door's turn-running both leave the words first in the queue, the first said as it came and the second in a sentence of its own, and nothing was pasted or pressed",
      SAME(seen.odd.queue, ["나", "가"]) && seen.odd.note.includes("pty closed") && SAME(seen.running.queue, ["나", "가"]) &&
        seen.running.note.length >= 8 && !seen.running.note.includes("turn-") && REACHED(seen.running.sent).length === 0,
      JSON.stringify({ odd: { queue: seen.odd.queue, note: seen.odd.note }, running: { queue: seen.running.queue, note: seen.running.note, sent: seen.running.sent } }),
    );
    ok(
      "words_that_reached_the_line_and_whose_enter_did_not_follow_leave_the_queue_and_ask_the_person_to_look: the door's turn-landed takes the row out (the words are on the pane's line) and raises the band — and the other row stays",
      SAME(seen.landed.queue, ["가"]) && seen.landed.uncertain === true,
      JSON.stringify(seen.landed),
    );
  });
}

/* ⑰ P3-5 — 응답이 없으면 글이 들어갔는지 모른다: 그렇게 말하고(다섯 언어), 그 행은 두 번 보내지지 않는다. */
async function testUnsureRow(browser, origin, ok) {
  for (const road of ["pane", "wire"]) {
    await scene(browser, origin, ok, `unsure ${road}`, async (page) => {
      let term = null;
      if (road === "pane") term = await openPane(page, "claude", "working");
      else await openWire(page, { protocol: "app-server", steer: true, status: "working" });
      await queueWords(page, "하나", "둘", "셋");
      const door = road === "pane" ? "steer" : "wireSteer";
      const doorName = road === "pane" ? "pane_steer" : "wire_steer";
      await page.evaluate((door) => { window.__ACTS__[door] = () => { throw "turn-unknown"; }; }, door);
      await press(page, "steer", 1);
      const seen = { unsure: await page.evaluate(LOOK) };
      const said = await page.evaluate(() => window.__WORDS__("turn-unknown"));
      await page.evaluate((door) => { window.__ACTS__.calls.length = 0; window.__ACTS__[door] = null; }, door);
      await press(page, "steer", 1);
      await press(page, "now", 1);
      seen.again = await page.evaluate(LOOK);
      await press(page, "steer", 0);
      seen.other = await page.evaluate(LOOK);
      // 턴이 끝나도 의심스러운 행(지금 맨 앞)은 저절로 나가지 않는다.
      await page.evaluate(async ({ term, road }) => {
        if (road === "pane") window.__TELL__({ term, state: "done", agent: "claude" });
        else window.__WIRE_LOG__ = { ...window.__WIRE_LOG__, status: "idle" };
        await pollHelperPages();
        await window.__PAINT__(3);
      }, { term, road });
      await page.waitForTimeout(500);
      seen.ended = await page.evaluate(LOOK);
      await press(page, "drop", 0);
      await page.waitForTimeout(900);
      seen.after = await page.evaluate(LOOK);
      // On a pane the row that went on was typed by the door: another row was steered earlier, so the line is doubted (r3 P2-1).
      seen.wentOn = road === "pane"
        ? SAME(seen.after.calls.filter((call) => call.door === "pane_send_now"), [{ door: "pane_send_now", term, text: "셋" }]) &&
          seen.after.sent.filter((one) => one.startsWith("paste:")).length === 0
        : SAME(seen.after.calls.filter((call) => call.door === "wire_send"), [{ door: "wire_send", id: 91, text: "셋" }]);
      ok(
        `an_unanswered_steer_on_a_${road}_says_it_does_not_know_and_keeps_the_row_from_going_twice: the row stays, the line says it is not known whether the words went in, steering or sending it now asks no door again, another row still steers, the turn's end does not send it, and dropping it lets the queue go on`,
        said.length >= 8 && !said.includes("turn-") && SAME(seen.unsure.queue, ["하나", "둘", "셋"]) && seen.unsure.note === said &&
          seen.unsure.rows[1].off.includes("steer") && seen.unsure.rows[1].off.includes("now") && !seen.unsure.rows[0].off.includes("steer") &&
          seen.again.calls.length === 0 && SAME(seen.other.calls.map((call) => call.door), [doorName]) && SAME(seen.other.queue, ["둘", "셋"]) &&
          seen.ended.calls.filter((call) => call.door !== doorName).length === 0 && REACHED(seen.ended.sent).length === 0 && SAME(seen.ended.queue, ["둘", "셋"]) &&
          (seen.ended.why ?? "").length >= 8 && SAME(seen.after.queue, []) && seen.wentOn,
        JSON.stringify({ unsure: { queue: seen.unsure.queue, note: seen.unsure.note, off: seen.unsure.rows.map((row) => row.off) }, again: seen.again.calls, other: seen.other.calls, ended: { queue: seen.ended.queue, why: seen.ended.why, sent: seen.ended.sent, calls: seen.ended.calls }, after: seen.after.queue }),
      );
    });
  }
}

/* ⑱ P3-11 — 거절 이유 줄은 상태가 바뀌면 지워진다: 질문이 뜨고 답이 간 뒤에도 「질문이 떠 있다」가 남지 않는다. */
async function testNoteFollowsTheState(browser, origin, ok) {
  await scene(browser, origin, ok, "note follows the state", async (page) => {
    const term = await openPane(page, "claude", "working");
    await queueWords(page, "하나", "둘");
    await page.evaluate(() => { window.__ACTS__.steer = () => { throw "turn-screen"; }; });
    await press(page, "steer", 0);
    const refused = await page.evaluate(LOOK);
    const said = await page.evaluate(() => window.__WORDS__("turn-screen"));
    // 질문이 떠서 상태가 바뀌었다.
    await page.evaluate(async (term) => {
      window.__TELL__({ term, state: "waiting", agent: "claude",
        ask_prompt: { questions: [{ question: "어느 쪽으로 할까요?", header: "방향", options: [{ label: "A", description: "가" }, { label: "B", description: "나" }], multi_select: false }] } });
      await window.__PAINT__(3);
    }, term);
    const asking = await page.evaluate(LOOK);
    // 질문에 답이 가고 턴이 다시 돈다.
    await page.evaluate(async (term) => {
      window.__TELL__({ term, state: "working", agent: "claude" });
      await window.__PAINT__(3);
    }, term);
    const back = await page.evaluate(LOOK);
    ok(
      "a_refusal_line_goes_when_the_state_moves: the refusal stood under the queue; once a question stood, that line was not the old refusal (the queue says to answer first), and after the question went the old refusal did not come back",
      refused.note === said && said.length >= 8 && asking.note !== said && asking.note.length >= 8 && back.note !== said,
      JSON.stringify({ refused: refused.note, asking: asking.note, back: back.note }),
    );
  });
}

/* ⑲ P3-9 — 입력칸의 Alt+Enter는 원래대로 보낸다(⌘/Ctrl이 같이 눌릴 때만 새 단축키). AltGr(Windows의 Ctrl+Alt)은 끼워 넣기를 부르지 않는다. */
/* The two platforms a page can report, for a scene that has to run both branches of what depends on it (r4 P3-7): the
 * AltGr reading below is `!usesCommandModifier && AltGraph`, and a machine runs only its own branch. The page is told its
 * platform before any script of the window reads it (`reportedPlatformScript`, as the settings suites do). */
const REPORTED_PLATFORMS = [["macOS", "MacIntel"], ["Windows", "Win32"]];

async function testAltEnter(browser, origin, ok) {
  for (const [os, reported] of REPORTED_PLATFORMS) {
    await scene(browser, origin, ok, `alt enter on ${os}`, async (page) => {
      const term = await openPane(page, "claude", "working");
      await queueWords(page, "첫 글");
      // On macOS WebKit may report the Option key as AltGraph (r3 P3-7): there the chord still steers.
      // Only where AltGr is a Ctrl+Alt of its own (Windows, Linux) does it send the box as an Enter.
      const onMac = await page.evaluate(() => usesCommandModifier);
      ok(
        `the_page_reports_${os.toLowerCase()}_to_this_scene: the platform the scene asked for is the one the window's key reading uses, so both branches of the AltGr reading ran whatever machine this is`,
        onMac === (os === "macOS"),
        JSON.stringify({ os, reported, usesCommandModifier: onMac }),
      );
      // The ⌘/Ctrl of THIS page — not the machine's `__TEST_PRIMARY_EVENT__`.
      const key = (init) => page.evaluate(({ init, onMac }) => {
        const box = window.__COMPOSER__().querySelector(".worker-composer-box");
        box.focus();
        const primary = init.primary ? (onMac ? { metaKey: true } : { ctrlKey: true }) : {};
        const event = new KeyboardEvent("keydown", { key: "Enter", code: "Enter", bubbles: true, cancelable: true, ...init, ...primary });
        box.dispatchEvent(event);
        return event.defaultPrevented;
      }, { init, onMac });
      const box = page.locator(".worker-composer-box:visible").first();
      await box.fill("알트로 보낸 글");
      await key({ altKey: true });
      await page.waitForTimeout(200);
      const alt = await page.evaluate(LOOK);
      await box.fill("지알로 보낸 글");
      await key({ altKey: true, primary: true, modifierAltGraph: true });
      await page.waitForTimeout(250);
      const altGr = await page.evaluate(LOOK);
      await box.fill("초안");
      await key({ altKey: true, primary: true });
      await page.waitForTimeout(250);
      const chord = await page.evaluate(LOOK);
      ok(
        `alt_enter_alone_sends_the_box_as_it_did_before_the_chords_on_${os.toLowerCase()}: Alt+Enter with no ⌘/Ctrl queued the draft (the turn runs) and cleared the box`,
        SAME(alt.queue, ["첫 글", "알트로 보낸 글"]) && alt.box === "" && alt.calls.length === 0,
        JSON.stringify(alt),
      );
      if (onMac) {
        ok(
          "on_macos_a_chord_the_browser_calls_altgraph_still_steers: ⌘+Option+Enter that WebKit may report as AltGraph is still the queue's chord — it steered the first row and left the draft in the box; AltGr is Windows's Ctrl+Alt and means nothing on a Mac",
          SAME(altGr.calls, [{ door: "pane_steer", term, text: "첫 글" }]) && altGr.box === "지알로 보낸 글" && SAME(altGr.queue, ["알트로 보낸 글"]),
          JSON.stringify(altGr),
        );
        ok(
          "on_macos_the_real_chord_still_steers_the_first_row_and_leaves_the_box: ⌘+Alt+Enter steered the next row and the draft stayed in the box",
          SAME(chord.calls.slice(1), [{ door: "pane_steer", term, text: "알트로 보낸 글" }]) && chord.box === "초안",
          JSON.stringify(chord),
        );
      } else {
        ok(
          "on_windows_altgr_enter_asks_the_queue_for_nothing: Ctrl+Alt+Enter that the browser says is AltGr asked no door; it is no chord, so the box sent as an Enter",
          altGr.calls.length === 0 && SAME(altGr.queue, ["첫 글", "알트로 보낸 글", "지알로 보낸 글"]),
          JSON.stringify(altGr),
        );
        ok(
          "on_windows_the_real_chord_still_steers_the_first_row_and_leaves_the_box: Ctrl+Alt+Enter steered the first row and the draft stayed in the box",
          SAME(chord.calls, [{ door: "pane_steer", term, text: "첫 글" }]) && chord.box === "초안",
          JSON.stringify(chord),
        );
      }
    }, { before: (page) => page.addInitScript(reportedPlatformScript, reported) });
  }
}

/* ⑳ P3-10 — 고치는 중에 초점이 다른 행의 단추에 있어도 Esc는 고치기를 취소하고, 턴은 멈추지 않는다. */
async function testEscapeCancelsAnEdit(browser, origin, ok) {
  await scene(browser, origin, ok, "escape on a button", async (page) => {
    await openPane(page, "claude", "working");
    await queueWords(page, "하나", "둘");
    await press(page, "edit", 0);
    await page.evaluate(() => window.__ROWBTN__(1, "drop").focus());
    await page.keyboard.press("Escape");
    await page.waitForTimeout(250);
    const seen = await page.evaluate(LOOK);
    ok(
      "escape_on_another_rows_button_cancels_the_edit_and_does_not_stop_the_turn: the edit ended, the row kept its words, the box is empty again, no Escape was typed at the pane and no stop was asked",
      seen.rows.every((row) => !row.editing) && SAME(seen.queue, ["하나", "둘"]) && seen.box === "" && REACHED(seen.sent).length === 0 && seen.calls.length === 0 && !seen.focus.body,
      JSON.stringify(seen),
    );
    // 고치는 중이 아니면 Esc는 지금처럼 턴을 멈춘다.
    await page.evaluate(() => window.__ROWBTN__(1, "drop").focus());
    await page.keyboard.press("Escape");
    await page.waitForTimeout(250);
    const stop = await page.evaluate(LOOK);
    ok(
      "without_an_edit_escape_on_a_button_still_stops_the_turn: Esc stopped the turn the way it does everywhere on the page",
      stop.sent.filter((one) => one.startsWith("key:") && one.includes("Escape")).length === 1,
      JSON.stringify(stop.sent),
    );
  });
}

/* ㉑ P3-1, P3-12 — 알림이 대기열 전체를 가져가도 고치던 글은 사라지지 않고, 고치는 글에는 첨부 칩이 말없이 붙지 않는다. */
async function testNoWordsLostToANotice(browser, origin, ok) {
  await scene(browser, origin, ok, "edit survives a notice", async (page) => {
    await openPane(page, "claude", "working");
    await queueWords(page, "하나", "둘");
    await press(page, "edit", 0);
    const box = page.locator(".worker-composer-box:visible").first();
    await box.click();
    await page.keyboard.press("End");
    await page.keyboard.type(" 고침");
    // 판의 CLI가 끝나고 대화로 잇기가 거절된 뒤: 기다리던 글이 상자로 돌아온다.
    await page.evaluate(() => returnComposerQueueToBox(window.__COMPOSER__().__workerRun));
    const back = await page.evaluate(LOOK);
    ok(
      "words_being_edited_come_back_with_the_queue_and_are_not_thrown_away: the unsaved change (「하나 고침」) is in the draft the queue returned to the box — not the row's old words — beside the other row",
      back.box.includes("하나 고침") && back.box.includes("둘") && SAME(back.queue, []),
      JSON.stringify(back),
    );
  });
  await scene(browser, origin, ok, "edit survives a lost pane", async (page) => {
    await openPane(page, "claude", "working");
    await queueWords(page, "하나", "둘");
    await press(page, "edit", 0);
    const box = page.locator(".worker-composer-box:visible").first();
    await box.click();
    await page.keyboard.press("End");
    await page.keyboard.type(" 고침");
    await page.evaluate(() => sayComposerQueueLost(window.__COMPOSER__().__workerRun));
    const tips = await page.evaluate(() => [...document.querySelectorAll(".toast")].map((note) => note.dataset.tip ?? ""));
    ok(
      "words_being_edited_are_said_with_the_lost_queue: the notice that shows the words of a pane that closed carries the changed words (「하나 고침」) and the other row",
      tips.some((tip) => tip.includes("하나 고침") && tip.includes("둘")),
      JSON.stringify(tips),
    );
  });
  await scene(browser, origin, ok, "edit and chips", async (page) => {
    await page.evaluate(() => { window.__ANSWER__.path_kinds = ({ paths }) => paths.map(() => "file"); });
    await openPane(page, "claude", "working");
    await queueWords(page, "하나", "둘");
    await page.evaluate(async () => { await composerAttachmentsOf(window.__COMPOSER__()).add(["/tmp/work/plan.md"]); });
    await press(page, "edit", 0);
    const box = page.locator(".worker-composer-box:visible").first();
    await box.click();
    await page.keyboard.press("End");
    await page.keyboard.type(" 고침");
    await page.keyboard.press("Enter");
    await page.waitForTimeout(250);
    const seen = await page.evaluate(() => ({ queue: [...window.__COMPOSER__().__workerRun.queue], chips: composerAttachmentsOf(window.__COMPOSER__()).count() }));
    ok(
      "a_chip_waiting_in_the_composer_is_not_attached_to_the_row_being_edited: saving the edit put only the words back; the chip is still in the composer for the next message",
      SAME(seen.queue, ["하나 고침", "둘"]) && seen.chips === 1,
      JSON.stringify(seen),
    );
  });
}

/* ㉒ P3-2 — 불확실 띠: 이 판의 대화에서는 「부모」라 말하지 않고, 「확인 후 입력칸 다시 사용」은 기다리던 글을 다시 돌린다. */
async function testUncertainBand(browser, origin, ok) {
  await scene(browser, origin, ok, "uncertain band", async (page) => {
    await openPane(page, "claude", "done");
    const seen = {};
    seen.words = await page.evaluate(() => {
      const run = window.__COMPOSER__().__workerRun;
      run.sendUncertain = true;
      run.queue = ["기다리던 글"];
      syncWorkerComposers(run);
      return window.__COMPOSER__().querySelector(".worker-delivery span")?.textContent ?? "";
    });
    seen.hidden = await page.evaluate(() => window.__COMPOSER__().querySelector(".worker-delivery").hidden);
    await page.evaluate(() => window.__COMPOSER__().querySelector(".worker-delivery button").click());
    await page.waitForTimeout(900);
    seen.after = await page.evaluate(LOOK);
    ok(
      "the_pane_conversations_band_does_not_say_parent: the band over a pane's own conversation names this pane — no 「부모」 (a helper's page, whose words go to its parent, keeps that word)",
      !seen.hidden && seen.words.length >= 8 && !seen.words.includes("부모"),
      JSON.stringify({ hidden: seen.hidden, words: seen.words }),
    );
    ok(
      "the_resume_button_runs_the_queue_again: after 확인 후 입력칸 다시 사용 the words that waited went to the idle pane — paste, then Enter — and the queue is empty",
      SAME(seen.after.sent, ["paste:기다리던 글", ENTER]) && SAME(seen.after.queue, []),
      JSON.stringify(seen.after),
    );
  });
}

/* ㉓ P3-8 — 턴의 가장자리는 행을 다시 만들지 않는다: 두 단추가 숨고 돌아올 뿐이고, 같은 행 요소가 그대로다. 말과
 * 단축키 말풍선을 다시 입히는 쓸기(`paintComposerQueueTips`)도 같다. (언어를 바꾸면 이 앱은 대화 페이지를 통째로 다시
 * 세우므로 그것은 여기서 주장하지 않는다 — 그 뒤의 말은 「language switched mid-session」이 본다.) */
async function testTurnBoundaryKeepsTheRows(browser, origin, ok) {
  await scene(browser, origin, ok, "turn boundary", async (page) => {
    const term = await openPane(page, "claude", "working");
    await queueWords(page, "하나", "둘", "셋");
    await press(page, "edit", 0);
    const seen = await page.evaluate(async (term) => {
      const queue = window.__COMPOSER__().querySelector(".composer-queue");
      // Asked of the live composer every time: a composer that was rebuilt would show unmarked rows,
      // not the marked ones of the old node.
      const live = () => window.__COMPOSER__().querySelector(".composer-queue");
      [...queue.querySelectorAll(".composer-queue-item")].forEach((row, at) => { row.__mark = at; });
      const marks = () => [...live().querySelectorAll(".composer-queue-item")].map((row) => row.__mark ?? null);
      const shown = () => [...live().querySelectorAll(".composer-queue-item")].map((row) => [...row.querySelectorAll("button[data-act]")].filter((button) => !button.hidden).map((button) => button.dataset.act));
      const watched = { added: 0, removed: 0 };
      const observer = new MutationObserver((list) => {
        for (const one of list) {
          watched.added += [...one.addedNodes].filter((node) => node.nodeType === 1).length;
          watched.removed += [...one.removedNodes].filter((node) => node.nodeType === 1).length;
        }
      });
      observer.observe(queue, { subtree: true, childList: true });
      const before = { marks: marks(), shown: shown() };
      window.__TELL__({ term, state: "done", agent: "claude" });
      await window.__PAINT__(3);
      const resting = { marks: marks(), shown: shown() };
      window.__TELL__({ term, state: "working", agent: "claude" });
      await window.__PAINT__(3);
      const running = { marks: marks(), shown: shown() };
      const tips = () => [...live().querySelectorAll(".composer-queue-item button[data-tip]")].map((button) => button.dataset.tip);
      const tipsBefore = tips();
      paintComposerQueueTips();
      await window.__PAINT__(3);
      const sweep = { marks: marks(), shown: shown(), same: tips().length === tipsBefore.length && tips().every((tip, at) => tip === tipsBefore[at]) };
      observer.disconnect();
      return { before, resting, running, sweep, watched, same: live() === queue };
    }, term);
    const all = ["edit", "steer", "now", "drop"];
    ok(
      "a_turn_edge_and_the_relabel_sweep_redraw_no_row: the same three row elements stand before the turn ends, while the pane rests, when the next turn runs and after the sweep that says the tooltips again — no element was added or removed in the queue, the tooltips are the same words — and the two buttons hide while no turn runs and come back with it",
      seen.same && SAME(seen.before.marks, [0, 1, 2]) && SAME(seen.resting.marks, [0, 1, 2]) && SAME(seen.running.marks, [0, 1, 2]) && SAME(seen.sweep.marks, [0, 1, 2]) &&
        seen.watched.added === 0 && seen.watched.removed === 0 && seen.sweep.same &&
        seen.before.shown.every((acts) => SAME(acts, all)) && seen.resting.shown.every((acts) => SAME(acts, ["edit", "drop"])) &&
        seen.running.shown.every((acts) => SAME(acts, all)) && seen.sweep.shown.every((acts) => SAME(acts, all)),
      JSON.stringify(seen),
    );
  });
}

/* ㉔ 새 거절 낱말은 다섯 언어의 문장이 된다 — 낱말 그대로 보이지 않고, 영어가 아닌 말에 영어가 남지 않는다. */
async function testRefusalsInFiveLanguages(browser, origin, ok) {
  const words = [...DOOR_REFUSALS, "turn-unconfirmed", "turn-editing", "turn-unknown"];
  const said = {};
  for (const locale of ["ko", "en", "ja", "zh", "es"]) {
    await scene(browser, origin, ok, `refusals in ${locale}`, async (page) => {
      await page.evaluate(async (locale) => { await setLocale(locale, { refresh: false, persist: false }); }, locale);
      said[locale] = await page.evaluate((words) => Object.fromEntries(words.map((word) => [word, window.__WORDS__(word)])), words);
    });
  }
  const problems = [];
  for (const word of words) {
    for (const locale of ["ko", "en", "ja", "zh", "es"]) {
      const one = said[locale]?.[word] ?? "";
      if (one.length < 8 || one === word || one.includes("turn-")) problems.push(`${locale}/${word}: ${one}`);
      if (locale !== "en" && one === said.en?.[word]) problems.push(`${locale}/${word} is the English sentence`);
      if (locale !== "ko" && one === said.ko?.[word]) problems.push(`${locale}/${word} is the Korean sentence`);
    }
  }
  ok(
    "every_refusal_word_is_a_sentence_in_each_of_the_five_languages: the words the door added (a line that holds words, a turn running again, a CLI not reading, no session, no terminal, empty words, an answer that never came) say a sentence of their own in ko, en, ja, zh and es — never the bare word, never another language's sentence",
    problems.length === 0,
    problems.slice(0, 6).join("\n"),
  );
}

/* ㉕ 보내려고 대기열에서 뺀 글이 판에 닿기도 전에 실패하면 — 붙여넣기 오류, 판이 없음 — 글은 맨 앞에 되돌아오고 오류가
 * 말해진다. 예전에는 토스트 하나를 남기고 글이 사라졌다. 판이 다시 괜찮아지면 다음 가장자리에 같은 글이 먼저 나간다. */
async function testFailedDeliveryKeepsTheWords(browser, origin, ok) {
  await scene(browser, origin, ok, "failed delivery", async (page) => {
    const term = await openPane(page, "claude", "working");
    await queueWords(page, "가", "나");
    await page.evaluate(() => { window.__ANSWER__.term_paste = () => { throw new Error("pty closed"); }; });
    await page.evaluate(async (term) => {
      window.__TELL__({ term, state: "done", agent: "claude" });
      await window.__PAINT__(3);
    }, term);
    await page.waitForTimeout(400);
    const failed = await page.evaluate(LOOK);
    await page.evaluate(async (term) => {
      window.__ANSWER__.term_paste = (args) => (window.__ACTS__.sent.push(`paste:${args.text}`), null);
      window.__TELL__({ term, state: "working", agent: "claude" });
      await window.__PAINT__(2);
      window.__TELL__({ term, state: "done", agent: "claude" });
      await window.__PAINT__(3);
    }, term);
    await page.waitForTimeout(900);
    const later = await page.evaluate(LOOK);
    ok(
      "a_delivery_that_failed_before_anything_reached_the_pane_keeps_the_words_first: the paste failed, so both messages stand in the queue in their order, nothing was typed, and the failure was said to the person",
      SAME(failed.queue, ["가", "나"]) && REACHED(failed.sent).length === 0 && failed.toasts.some((words) => words.includes("pty closed")),
      JSON.stringify({ queue: failed.queue, sent: failed.sent, toasts: failed.toasts }),
    );
    ok(
      "the_kept_words_go_first_at_the_panes_next_edge: once the pane takes a paste again, the next turn's end sends the same first message — paste, then Enter — and the second keeps waiting",
      SAME(later.sent, ["paste:가", ENTER]) && SAME(later.queue, ["나"]),
      JSON.stringify({ sent: later.sent, queue: later.queue }),
    );
  });
}

/* ============================ r3 (비저자 검수 r2, 헤드 54f08414d) ============================
 *
 * 실제 Claude Code 2.1.316을 가짜 서버로 재어 본 것(crates/zerocode-shell/tests/fixtures/claude-code-2.1.316-*.reads):
 * Esc는 끼워 넣은 글을 입력줄로 되돌리지 않고 CLI가 곧바로 다음 턴으로 보낸다. 그래도 입력줄이 의심스러운 길은
 * 모두 문(`pane_send_now`)을 지난다 — 글이 입력줄 뒤에 붙어 한 메시지로 나가는 것은 시험이 알 수 없는 CLI에서도
 * 일어나서는 안 되기 때문이다. */
export async function testComposerQueueActsR3(browser, origin, ok) {
  await testDoubtfulLine(browser, origin, ok);
  await testWireStartWithoutAnswer(browser, origin, ok);
  await testCarryDuringAnEdit(browser, origin, ok);
  await testWaitingForThePerson(browser, origin, ok);
  await testBoxStaysOpenAtTheDoor(browser, origin, ok);
  await testDropIsOnePerPress(browser, origin, ok);
  await testStopIsOneWhileTheStopIsOut(browser, origin, ok);
  await testTypingIntoTheConversation(browser, origin, ok);
  await testRowIdentity(browser, origin, ok);
}

/* 턴이 끝났다고 판이 말한다. 문이 답하고 대기열이 한 걸음 가도록 조금 기다린다. */
async function turnEnds(page, term, wait = 450) {
  await page.evaluate(async (term) => {
    window.__TELL__({ term, state: "done", agent: "claude" });
    await window.__PAINT__(3);
  }, term);
  await page.waitForTimeout(wait);
}

const DOOR_CALLS = (look) => look.calls.filter((call) => call.door === "pane_send_now");
const PASTES = (look) => look.sent.filter((one) => one.startsWith("paste:"));

/* P2-1 — 입력줄이 의심스러운 길은 모두 문을 지난다. 끼워 넣은 글이 CLI 대기열에 있는 동안 어떤 키가 그 글을 입력줄로
 * 되돌릴 수 있고, 문이 `turn-line`으로 거절한 뒤의 입력줄에는 글이 있다. 그 뒤의 글은 붙여넣기(`term_paste`)로 가지 않는다:
 * 0번. 입력칸의 전송도 대기 글을 앞지르지 않는다. */
async function testDoubtfulLine(browser, origin, ok) {
  await scene(browser, origin, ok, "doubtful line: the stop button", async (page) => {
    const term = await openPane(page, "claude", "working");
    await queueWords(page, "A 글", "B 글");
    await press(page, "steer", 0);
    // 사람이 대화의 중지 단추를 누른다 — 판에 CLI의 중지 키가 간다(사람의 손이다).
    await page.locator(".worker-composer-send:visible").first().click();
    await page.waitForTimeout(150);
    const stopped = await page.evaluate(LOOK);
    await turnEnds(page, term);
    const seen = await page.evaluate(LOOK);
    ok(
      "after_a_steer_the_next_row_goes_through_the_door_even_when_the_stop_was_the_persons: A was steered, the page's stop button typed the CLI's own stop key, and B — the next row — was typed by the door that looks at the line again; the window pasted nothing and pressed no Enter",
      SAME(stopped.sent, [ESCAPE_KEY]) && SAME(DOOR_CALLS(seen), [{ door: "pane_send_now", term, text: "B 글" }]) &&
        PASTES(seen).length === 0 && SAME(seen.sent, [ESCAPE_KEY]) && SAME(seen.queue, []),
      JSON.stringify({ stopped: stopped.sent, calls: seen.calls, sent: seen.sent, queue: seen.queue }),
    );
  });
  await scene(browser, origin, ok, "doubtful line: refused, then the box", async (page) => {
    const term = await openPane(page, "claude", "working");
    await queueWords(page, "가", "나");
    await press(page, "now", 1);
    await page.evaluate(() => { window.__ACTS__.front = () => { throw "turn-line"; }; });
    await turnEnds(page, term);
    const held = await page.evaluate(LOOK);
    await queueWords(page, "다");
    await page.waitForTimeout(700);
    const seen = await page.evaluate(LOOK);
    ok(
      "a_send_from_the_box_never_overtakes_a_refused_row_and_never_pastes: the door refused 나 (the line holds words), the pane rests, and 다 sent from the box stands behind 나 and 가 — nothing was pasted, no Enter was pressed, the box is empty again",
      SAME(held.queue, ["나", "가"]) && SAME(seen.queue, ["나", "가", "다"]) && PASTES(seen).length === 0 && REACHED(seen.sent).length === 0 && seen.box === "",
      JSON.stringify({ held: held.queue, queue: seen.queue, sent: seen.sent, box: seen.box }),
    );
  });
  await scene(browser, origin, ok, "doubtful line: the refused row is dropped", async (page) => {
    const term = await openPane(page, "claude", "working");
    await queueWords(page, "가", "나");
    await press(page, "now", 1);
    await page.evaluate(() => { window.__ACTS__.front = () => { throw "turn-line"; }; });
    await turnEnds(page, term);
    await page.evaluate(() => { window.__ACTS__.calls.length = 0; });
    await press(page, "drop", 0);
    await page.waitForTimeout(500);
    const seen = await page.evaluate(LOOK);
    ok(
      "deleting_the_refused_row_does_not_open_the_pastes_road_for_the_next: with 나 deleted, 가 is the next row and the line is still doubted — the door was asked for it, the window pasted nothing and pressed nothing",
      SAME(DOOR_CALLS(seen), [{ door: "pane_send_now", term, text: "가" }]) && PASTES(seen).length === 0 && REACHED(seen.sent).length === 0,
      JSON.stringify({ calls: seen.calls, sent: seen.sent, queue: seen.queue }),
    );
  });
  await scene(browser, origin, ok, "doubtful line: the box after a steer", async (page) => {
    const term = await openPane(page, "claude", "working");
    await queueWords(page, "하나");
    await press(page, "steer", 0);
    await turnEnds(page, term);
    await page.evaluate(() => { window.__ACTS__.calls.length = 0; window.__ACTS__.sent.length = 0; });
    await queueWords(page, "둘");
    await page.waitForTimeout(700);
    const doubted = await page.evaluate(LOOK);
    await page.evaluate(() => { window.__ACTS__.calls.length = 0; window.__ACTS__.sent.length = 0; });
    await queueWords(page, "셋");
    await page.waitForTimeout(1000);
    const plain = await page.evaluate(LOOK);
    ok(
      "a_box_send_after_a_steer_goes_through_the_door: the steered row left the queue and the turn ended; 둘 sent from the idle pane's box was typed by the door — no paste, no Enter of the window's",
      SAME(DOOR_CALLS(doubted), [{ door: "pane_send_now", term, text: "둘" }]) && PASTES(doubted).length === 0 && REACHED(doubted.sent).length === 0 && SAME(doubted.queue, []) && doubted.box === "",
      JSON.stringify({ calls: doubted.calls, sent: doubted.sent, queue: doubted.queue, box: doubted.box }),
    );
    ok(
      "the_doubt_ends_when_the_door_has_typed_a_row: after the door typed 둘 the line was seen empty, so the next box send is the ordinary road again — paste, then Enter, and no door",
      DOOR_CALLS(plain).length === 0 && SAME(plain.sent, ["paste:셋", ENTER]),
      JSON.stringify({ calls: plain.calls, sent: plain.sent }),
    );
  });
}

/* P2-2 — 선(wire)의 `wire_send`도 답이 없으면 글이 들어갔는지 모른다: 그 행은 표시된 채 대기열에 남고 두 번 가지 않는다. */
async function testWireStartWithoutAnswer(browser, origin, ok) {
  await scene(browser, origin, ok, "wire start without an answer", async (page) => {
    await openWire(page, { protocol: "app-server", steer: true, status: "working" });
    await queueWords(page, "선에 하나", "선에 둘");
    await page.evaluate(() => {
      window.__ANSWER__.wire_send = (args) => {
        window.__ACTS__.calls.push({ door: "wire_send", ...args });
        throw "turn-unknown";
      };
    });
    const edge = async (status) => {
      await page.evaluate(async (status) => {
        window.__WIRE_LOG__ = { ...window.__WIRE_LOG__, status };
        await pollHelperPages();
        await window.__PAINT__(3);
      }, status);
      await page.waitForTimeout(450);
    };
    await edge("idle");
    const first = await page.evaluate(LOOK);
    const said = await page.evaluate(() => window.__WORDS__("turn-unknown"));
    // The wire runs again: the row's two buttons are there, and off — it is not known whether its words went in.
    await edge("working");
    const running = await page.evaluate(LOOK);
    await edge("idle");
    const later = await page.evaluate(LOOK);
    const sends = (look) => look.calls.filter((call) => call.door === "wire_send");
    ok(
      "a_start_with_no_answer_keeps_the_row_marked_and_says_it_does_not_know: the wire_send of the first row got the word for an answer that never came — the row stands first in the queue, the line says the words may have gone in, and when the wire runs again its steer and send now are off while the row behind it keeps both",
      SAME(sends(first), [{ door: "wire_send", id: 91, text: "선에 하나" }]) && SAME(first.queue, ["선에 하나", "선에 둘"]) &&
        first.note === said && said.length >= 8 && running.rows[0].off.includes("steer") && running.rows[0].off.includes("now") && running.rows[1].off.length === 0,
      JSON.stringify({ sends: sends(first), queue: first.queue, note: first.note, off: running.rows.map((row) => row.off) }),
    );
    ok(
      "the_same_words_do_not_go_to_the_wire_twice: two more turn edges later the unanswered row was not sent again and the row behind it did not go past it",
      SAME(sends(later), [{ door: "wire_send", id: 91, text: "선에 하나" }]) && SAME(later.queue, ["선에 하나", "선에 둘"]),
      JSON.stringify({ sends: sends(later), queue: later.queue }),
    );
    await page.evaluate(() => {
      window.__ANSWER__.wire_send = (args) => (window.__ACTS__.calls.push({ door: "wire_send", ...args }), null);
    });
    await press(page, "drop", 0);
    await page.waitForTimeout(500);
    const after = await page.evaluate(LOOK);
    ok(
      "deleting_the_unanswered_row_lets_the_queue_go_on: with the row gone the next one was sent once",
      SAME(sends(after).slice(1), [{ door: "wire_send", id: 91, text: "선에 둘" }]) && SAME(after.queue, []),
      JSON.stringify({ sends: sends(after), queue: after.queue }),
    );
  });
}

/* P2-3 — 고치는 중에 대화가 선으로 넘어가면 고치던 글이 대기열 제자리에 들어가고 곁에 둔 초안이 상자로 돌아온다. */
/* The backend a pane's hand-over to a wire asks: a wire at rest that stands at once. */
async function standWireForHandOver(page) {
  await page.evaluate(() => {
    window.__WIRE_LOG__ = {
      found: true, skipped: false, next: 0, turns: [], status: "idle", asks: [], live: [], agent: "claude", protocol: "app-server",
      models: [], modes: [], commands: [], version: "2.1.316", steer: true,
    };
    window.__ANSWER__.wire_start = (args) => ({ id: 91, agent: args.agent, protocol: "app-server", version: "2.1.316", model: null, session: args.resume });
    window.__ANSWER__.wire_log = () => window.__WIRE_LOG__;
    window.__ANSWER__.wire_history = () => ({ found: false, next: 0, turns: [] });
  });
}

async function testCarryDuringAnEdit(browser, origin, ok) {
  await scene(browser, origin, ok, "carry keeps the unsure mark", async (page) => {
    const term = await openPane(page, "claude", "working");
    await standWireForHandOver(page);
    await queueWords(page, "하나", "둘");
    await page.evaluate(() => { window.__ACTS__.steer = () => { throw "turn-unknown"; }; });
    await press(page, "steer", 0);
    await page.evaluate(() => { window.__ACTS__.steer = null; });
    await page.evaluate(async (term) => { await handPaneToWire(term); }, term);
    await turnEnds(page, term, 200);
    await page.waitForFunction(() => tabs.some((tab) => tab.id === "wire:91"), null, { timeout: 6000 }).catch(() => null);
    await page.waitForTimeout(900);
    const carried = await page.evaluate(LOOK);
    await press(page, "drop", 0);
    await page.waitForTimeout(500);
    const after = await page.evaluate(LOOK);
    const sends = (look) => look.calls.filter((call) => call.door === "wire_send");
    ok(
      "a_row_of_unknown_outcome_stays_unsure_when_the_queue_goes_to_the_wire: the pane's steer of 「하나」 got no answer; after the hand-over the wire's page holds both rows, 「하나」 is first and was not sent, and deleting it lets 「둘」 go to the wire once",
      SAME(carried.queue, ["하나", "둘"]) && sends(carried).length === 0 && (carried.why ?? "").length >= 8 &&
        SAME(sends(after), [{ door: "wire_send", id: 91, text: "둘" }]) && SAME(after.queue, []),
      JSON.stringify({ carried: { queue: carried.queue, sends: sends(carried) }, after: { queue: after.queue, sends: sends(after) } }),
    );
  });
  await scene(browser, origin, ok, "carry during an edit", async (page) => {
    const term = await openPane(page, "claude", "working");
    await standWireForHandOver(page);
    await queueWords(page, "하나", "둘");
    const box = page.locator(".worker-composer-box:visible").first();
    await box.click();
    await page.keyboard.type("초안");
    await press(page, "edit", 0);
    await box.click();
    await page.keyboard.press("End");
    await page.keyboard.type(" 고침");
    // 턴 한가운데서 연 대화: 선으로 넘기기가 턴 끝으로 미뤄진다. 턴이 끝나면 선이 서고 대기열이 옮겨 간다.
    const pending = await page.evaluate(async (term) => {
      await handPaneToWire(term);
      return paneChats.get(term)?.pendingWire === true;
    }, term);
    await turnEnds(page, term, 200);
    await page.waitForFunction(() => tabs.some((tab) => tab.id === "wire:91"), null, { timeout: 6000 }).catch(() => null);
    await page.waitForTimeout(900);
    const seen = await page.evaluate(LOOK);
    seen.editing = await page.evaluate(() => window.__COMPOSER__()?.__workerRun?.queueEdit ?? null);
    const sends = seen.calls.filter((call) => call.door === "wire_send");
    ok(
      "an_edit_in_the_box_goes_to_the_wire_as_the_edited_row_and_the_draft_comes_back: the hand-over waited for the turn, then the wire's first message was 「하나 고침」 — the change in the box, not the old 「하나」 — the draft set aside (「초안」) stands in the new page's box, and nothing is still being edited",
      pending && SAME(sends, [{ door: "wire_send", id: 91, text: "하나 고침" }]) && seen.box === "초안" && SAME(seen.queue, ["둘"]) && seen.editing === null,
      JSON.stringify({ pending, sends, box: seen.box, queue: seen.queue, editing: seen.editing }),
    );
  });
}

/* P3-1 — 문이 거절한 뒤 판이 쉬면 대기열 밑줄이 「곧 보냅니다」라 하지 않는다: 이 글은 사람이 풀어 줄 때까지 기다린다. */
async function testWaitingForThePerson(browser, origin, ok) {
  const said = {};
  for (const locale of ["ko", "en", "ja", "zh", "es"]) {
    await scene(browser, origin, ok, `waiting for the person in ${locale}`, async (page) => {
      await page.evaluate(async (locale) => { await setLocale(locale, { refresh: false, persist: false }); }, locale);
      const term = await openPane(page, "claude", "working");
      await queueWords(page, "가", "나");
      await press(page, "now", 1);
      await page.evaluate(() => { window.__ACTS__.front = () => { throw "turn-line"; }; });
      await turnEnds(page, term);
      said[locale] = await page.evaluate(() => ({
        why: document.querySelector(".composer-queue-why")?.textContent ?? null,
        going: t("composer.queue.why.going", "곧 보냅니다"),
        refused: t("composer.queue.why.refused", "이 글은 아직 보내지 않았습니다 — 아래 까닭을 풀면 이어서 보냅니다"),
        note: document.querySelector(".composer-queue-note")?.textContent ?? "",
      }));
    });
  }
  const problems = [];
  for (const locale of ["ko", "en", "ja", "zh", "es"]) {
    const one = said[locale];
    if (!one) { problems.push(`${locale}: no scene`); continue; }
    if (one.refused.length < 8) problems.push(`${locale}: no sentence for a refused row`);
    if (one.why !== one.refused) problems.push(`${locale}: the line under the count is 「${one.why}」, not the refused row's sentence`);
    if (one.why === one.going) problems.push(`${locale}: the line says it goes soon`);
    if (one.note.length < 8) problems.push(`${locale}: the refusal's line is missing`);
    if (locale !== "en" && one.refused === said.en?.refused) problems.push(`${locale}: the English sentence`);
    if (locale !== "ko" && one.refused === said.ko?.refused) problems.push(`${locale}: the Korean sentence`);
  }
  ok(
    "a_row_the_door_refused_waits_for_the_person_and_says_so_in_five_languages: after turn-line the pane rests but the line under the count no longer says the row goes soon — it says the row is waiting for the reason below to be cleared, in ko, en, ja, zh and es",
    problems.length === 0,
    problems.join("\n"),
  );
}

/* P3-2 — 문이 기다리는 동안 입력칸은 잠기지 않는다: 그 사이 친 글자가 사라지지 않는다. */
async function testBoxStaysOpenAtTheDoor(browser, origin, ok) {
  await scene(browser, origin, ok, "box open at the door", async (page) => {
    const term = await openPane(page, "claude", "working");
    await queueWords(page, "가");
    await press(page, "now", 0);
    // 문이 입력줄을 기다리는 1.2초 — 진짜 문은 최대 2초 기다린다.
    await page.evaluate(() => { window.__ACTS__.front = () => new Promise((done) => window.__REAL_TIMEOUT__(() => done(null), 1200)); });
    await page.evaluate(async (term) => {
      window.__TELL__({ term, state: "done", agent: "claude" });
      await window.__PAINT__(3);
    }, term);
    await page.waitForTimeout(250);
    const waiting = await page.evaluate(() => ({
      readOnly: window.__COMPOSER__().querySelector(".worker-composer-box").readOnly,
      doors: window.__ACTS__.calls.filter((call) => call.door === "pane_send_now").length,
    }));
    const box = page.locator(".worker-composer-box:visible").first();
    await box.click();
    await page.keyboard.type("그 사이에 쓴 글");
    await page.waitForTimeout(1400);
    const after = await page.evaluate(LOOK);
    ok(
      "the_box_takes_words_while_the_door_waits_for_the_line: the door was asked and had not answered, the box was not read-only, the words typed meanwhile are in it afterwards, and the queue went out",
      waiting.doors === 1 && waiting.readOnly === false && after.box === "그 사이에 쓴 글" && SAME(after.queue, []),
      JSON.stringify({ waiting, box: after.box, queue: after.queue }),
    );
  });
}

/* P3-3 — ✕는 한 번 누름에 한 행만 지운다. 되돌리기는 없으므로 이 단추의 누름은 의도한 한 번이어야 한다: 더블클릭의 둘째
 * 눌림(click의 detail이 2)과 누르고 있는 키의 반복(keydown의 repeat — 브라우저는 반복되는 Enter마다 click을 낸다, 그리고
 * 지운 행의 자리에는 다음 행의 ✕가 초점을 받는다)은 아무 행도 지우지 않는다. Playwright는 키 반복의 기본 동작(click)을 못
 * 만들므로, 이 시험은 처리기가 받는 이벤트 그대로를 보낸다. */
async function testDropIsOnePerPress(browser, origin, ok) {
  await scene(browser, origin, ok, "drop is one per press", async (page) => {
    await openPane(page, "claude", "working");
    await queueWords(page, "하나", "둘", "셋", "넷", "다섯");
    const click = (detail) => page.evaluate((detail) => {
      const button = window.__ROWBTN__(0, "drop");
      button?.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, detail }));
      return [...window.__COMPOSER__().__workerRun.queue];
    }, detail);
    const repeating = await page.evaluate(() => {
      const button = window.__ROWBTN__(0, "drop");
      button.focus();
      const event = new KeyboardEvent("keydown", { key: "Enter", code: "Enter", repeat: true, bubbles: true, cancelable: true });
      button.dispatchEvent(event);
      return { prevented: event.defaultPrevented, queue: [...window.__COMPOSER__().__workerRun.queue] };
    });
    const second = await click(2);
    const third = await click(3);
    const first = await click(1);
    const keyboard = await click(0);
    ok(
      "a_pressed_cross_removes_one_row_and_the_second_press_of_a_double_click_removes_none: a press with a click count of 1 and the click a key makes (count 0) each removed the first row; a click that is the second or third of a quick run removed nothing",
      SAME(second, ["하나", "둘", "셋", "넷", "다섯"]) && SAME(third, ["하나", "둘", "셋", "넷", "다섯"]) && SAME(first, ["둘", "셋", "넷", "다섯"]) && SAME(keyboard, ["셋", "넷", "다섯"]),
      JSON.stringify({ second, third, first, keyboard }),
    );
    ok(
      "a_held_key_on_a_queue_button_is_stopped_before_it_presses_the_button: the repeating Enter that would click the focused cross again and again was prevented, and the queue is as it was",
      repeating.prevented === true && SAME(repeating.queue, ["하나", "둘", "셋", "넷", "다섯"]),
      JSON.stringify(repeating),
    );
    // 진짜 손: 더블클릭과 누르고 있는 Enter는 어느 쪽이든 한 행 넘게 지우지 못한다.
    await page.locator(".composer-queue-item").nth(0).locator('button[data-act="drop"]').dblclick({ force: true }).catch(() => null);
    await page.waitForTimeout(250);
    const dbl = await page.evaluate(LOOK);
    await page.evaluate(() => window.__ROWBTN__(0, "drop")?.focus());
    await page.keyboard.down("Enter");
    for (let again = 0; again < 4; again += 1) {
      await page.keyboard.down("Enter");
      await page.waitForTimeout(40);
    }
    await page.keyboard.up("Enter");
    await page.waitForTimeout(250);
    const held = await page.evaluate(LOOK);
    ok(
      "a_real_double_click_and_a_real_held_enter_remove_at_most_one_row_each: from three rows a double click on the first cross left at least two, and Enter held on the focused cross left at least one fewer than that — never the whole queue",
      dbl.queue.length >= 2 && held.queue.length >= dbl.queue.length - 1,
      JSON.stringify({ dbl: dbl.queue, held: held.queue }),
    );
  });
}

/* P3-4 — 바로 보내기의 멈춤이 나가 있는 동안 대화의 Esc와 중지 단추는 아무것도 치지 않는다: 둘째 Esc는 쉬는 입력줄에서 CLI의 되감기를 연다. */
async function testStopIsOneWhileTheStopIsOut(browser, origin, ok) {
  await scene(browser, origin, ok, "stop is one", async (page) => {
    const term = await openPane(page, "claude", "working");
    await queueWords(page, "가", "나");
    await press(page, "now", 1);
    const out = await page.evaluate(LOOK);
    await page.evaluate(() => window.__ROWBTN__(0, "edit").focus());
    await page.keyboard.press("Escape");
    await page.waitForTimeout(150);
    // 상자가 비어 있고 턴이 도는 동안의 단추는 중지다 — 그것도 멈춤이 나가 있는 동안은 누름이 아니다.
    await page.locator(".worker-composer-send:visible").first().click({ force: true, timeout: 2000 }).catch(() => null);
    await page.waitForTimeout(250);
    const seen = await page.evaluate(LOOK);
    ok(
      "while_the_stop_of_a_send_now_is_out_esc_and_the_stop_button_type_nothing: the queue's stop went through its door once; an Esc on the page and a press of the stop button afterwards typed no key at the pane and asked no second stop",
      SAME(out.calls, [{ door: "pane_interrupt", term }]) && SAME(seen.calls, [{ door: "pane_interrupt", term }]) && REACHED(seen.sent).length === 0,
      JSON.stringify({ calls: seen.calls, sent: seen.sent }),
    );
  });
}

/* P3-8 — 대화의 빈 곳을 누른 뒤 친 글자는 입력칸으로 간다(판으로도, 아무 데로도 아니다). */
async function testTypingIntoTheConversation(browser, origin, ok) {
  await scene(browser, origin, ok, "typing into the conversation", async (page) => {
    await openPane(page, "claude", "done");
    await page.evaluate(() => { document.activeElement?.blur?.(); });
    const before = await page.evaluate(() => document.activeElement === document.body || document.activeElement === null);
    await page.keyboard.type("x");
    await page.waitForTimeout(200);
    const seen = await page.evaluate(LOOK);
    ok(
      "a_letter_typed_after_nothing_has_the_focus_lands_in_the_conversations_box: with the page's blank under the pointer (no field focused) the letter is in the composer's box and nothing was typed at the pane",
      before && seen.box === "x" && REACHED(seen.sent).length === 0 && seen.focus.inBox,
      JSON.stringify({ before, box: seen.box, sent: seen.sent, focus: seen.focus }),
    );
  });
}

/* P3-9 — 의심스러운 행은 글 내용이 아니라 행으로 찾는다: 같은 글이 또 있어도 그 행만 꺼지고, 그 곁의 행을 지워도 풀리지 않는다. */
async function testRowIdentity(browser, origin, ok) {
  await scene(browser, origin, ok, "row identity", async (page) => {
    const term = await openPane(page, "claude", "working");
    await queueWords(page, "같은 글", "같은 글", "다른 글");
    await page.evaluate(() => { window.__ACTS__.steer = () => { throw "turn-unknown"; }; });
    await press(page, "steer", 1);
    await page.evaluate(() => { window.__ACTS__.steer = null; window.__ACTS__.calls.length = 0; });
    const first = await page.evaluate(LOOK);
    await queueWords(page, "같은 글");
    const added = await page.evaluate(LOOK);
    await press(page, "drop", 0);
    const dropped = await page.evaluate(LOOK);
    await turnEnds(page, term, 600);
    const ended = await page.evaluate(LOOK);
    const off = (row) => row.off.includes("steer") && row.off.includes("now");
    const free = (row) => !row.off.includes("steer") && !row.off.includes("now");
    ok(
      "only_the_row_of_unknown_outcome_is_off_not_every_row_with_its_words: the second 「같은 글」 is off, the first 「같은 글」 beside it is not, and neither is the same words queued again",
      first.rows.length === 3 && off(first.rows[1]) && free(first.rows[0]) && free(first.rows[2]) && added.rows.length === 4 && free(added.rows[3]),
      JSON.stringify({ first: first.rows.map((row) => row.off), added: added.rows.map((row) => row.off) }),
    );
    ok(
      "deleting_a_twin_does_not_free_the_row_of_unknown_outcome_and_the_turns_end_does_not_send_it: with the first 「같은 글」 deleted the row that moved up is still off, and when the turn ended nothing went to the pane",
      SAME(dropped.queue, ["같은 글", "다른 글", "같은 글"]) && off(dropped.rows[0]) && free(dropped.rows[2]) &&
        REACHED(ended.sent).length === 0 && SAME(ended.queue, ["같은 글", "다른 글", "같은 글"]),
      JSON.stringify({ dropped: dropped.rows.map((row) => row.off), ended: { queue: ended.queue, sent: ended.sent } }),
    );
  });
}

/* ---- r4 (검수 r3) ---------------------------------------------------------------------------------------------- */

export async function testComposerQueueActsR4(browser, origin, ok) {
  await testResumeAfterLanded(browser, origin, ok);
  await testStartClock(browser, origin, ok);
  await testInFlightAskedOnce(browser, origin, ok);
  await testSinkLetterToTheConversation(browser, origin, ok);
}

/* `ms`이고 코드가 `mark`를 담은 setTimeout 하나를 잡아 둔다 — 장면이 시계를 직접 돌리고(`held[n].fn()`), 지워졌는지
 * (`cleared`) 본다. 다른 시계는 그대로 간다. 잡힌 시계의 번호는 진짜 시계의 번호와 겹치지 않는다. */
async function holdClocks(page, ms, mark) {
  await page.evaluate(({ ms, mark }) => {
    const held = [];
    window.__HELD__ = held;
    const inner = window.setTimeout;
    const innerClear = window.clearTimeout.bind(window);
    window.setTimeout = (fn, wait, ...rest) => {
      if (Number(wait) !== ms || !String(fn).includes(mark)) return inner(fn, wait, ...rest);
      held.push({ fn, ms: Number(wait), cleared: false });
      return 7_000_000 + held.length - 1;
    };
    window.clearTimeout = (id) => {
      const one = held[Number(id) - 7_000_000];
      if (one) one.cleared = true;
      else innerClear(id);
    };
  }, { ms, mark });
}

const HELD = (page) => page.evaluate(() => window.__HELD__.map((one) => ({ ms: one.ms, cleared: one.cleared })));

/* P2-2 — 문이 `turn-landed`로 끝난 뒤 「확인 후 입력칸 다시 사용」을 누르면, 입력줄이 의심스러운 채 입력칸에서 보낸 글이
 * 문으로 나간다. 문으로 나간 행이 내린 빗장을 올려 줄 턴이 없었기 때문에, 예전에는 그 글이 영영 나가지 않았다. */
async function testResumeAfterLanded(browser, origin, ok) {
  await scene(browser, origin, ok, "resume after landed", async (page) => {
    const term = await openPane(page, "claude", "working");
    await queueWords(page, "하나", "둘");
    // 「하나」는 CLI의 대기열에 들어갔다: 입력줄이 의심스럽다.
    await press(page, "steer", 0);
    // 턴이 끝나면 「둘」이 문으로 간다. 문은 글을 입력줄에 올린 뒤 Enter 전에 사람 손이 닿았다고 답한다.
    await page.evaluate(() => { window.__ACTS__.front = () => { throw "turn-landed"; }; });
    await turnEnds(page, term);
    const landed = await page.evaluate(LOOK);
    // 사람이 판의 글을 지우고(훅의 낱말은 그대로 쉼) 「확인 후 입력칸 다시 사용」을 누른 뒤 입력칸에서 글을 보낸다.
    await page.evaluate(() => { window.__ACTS__.front = null; window.__ACTS__.calls.length = 0; });
    await page.evaluate(() => window.__COMPOSER__().querySelector(".worker-delivery button").click());
    await queueWords(page, "셋");
    await page.waitForTimeout(500);
    const resumed = await page.evaluate(LOOK);
    ok(
      "after_the_door_ended_as_landed_the_person_resumes_and_the_next_words_go_through_the_door: 「둘」 was refused as landed (the band asks the person to look), and after 「확인 후 입력칸 다시 사용」 the words sent from the box (the line is still doubted) were typed by the door once — no paste, no Enter of the window's — and the queue is empty",
      landed.uncertain && SAME(DOOR_CALLS(landed), [{ door: "pane_send_now", term, text: "둘" }]) &&
        SAME(DOOR_CALLS(resumed), [{ door: "pane_send_now", term, text: "셋" }]) && PASTES(resumed).length === 0 &&
        REACHED(resumed.sent).length === 0 && SAME(resumed.queue, []) && !resumed.uncertain,
      JSON.stringify({ landed: { uncertain: landed.uncertain, calls: landed.calls }, resumed: { calls: resumed.calls, sent: resumed.sent, queue: resumed.queue } }),
    );
  });
  await scene(browser, origin, ok, "resume after landed, the person sent it", async (page) => {
    const term = await openPane(page, "claude", "working");
    await queueWords(page, "하나", "둘", "셋");
    await press(page, "steer", 0);
    await page.evaluate(() => { window.__ACTS__.front = () => { throw "turn-landed"; }; });
    await turnEnds(page, term);
    // 사람이 판에서 그 글을 Enter로 보냈다: 훅이 작업 중이라 말한다.
    await page.evaluate(async (term) => {
      window.__ACTS__.front = null;
      window.__TELL__({ term, state: "working", agent: "claude" });
      await window.__PAINT__(3);
    }, term);
    await page.evaluate(() => window.__COMPOSER__().querySelector(".worker-delivery button").click());
    await page.waitForTimeout(400);
    const working = await page.evaluate(LOOK);
    await turnEnds(page, term, 600);
    const ended = await page.evaluate(LOOK);
    ok(
      "when_the_person_sent_the_landed_words_themselves_the_resume_sends_nothing_twice: the hook says a turn runs after the person's Enter; pressing 「다시 사용」 sent nothing — 「둘」 went to the door once (landed) — and the turn's end sent 「셋」 through the door, once",
      SAME(DOOR_CALLS(working), [{ door: "pane_send_now", term, text: "둘" }]) && SAME(working.queue, ["셋"]) &&
        SAME(DOOR_CALLS(ended), [{ door: "pane_send_now", term, text: "둘" }, { door: "pane_send_now", term, text: "셋" }]) && SAME(ended.queue, []),
      JSON.stringify({ working: { calls: working.calls, queue: working.queue }, ended: { calls: ended.calls, queue: ended.queue } }),
    );
  });
}

/* P3-1 — 턴을 시작하지 않는 글(`/context` 같은, CLI가 스스로 답하는 글) 뒤의 글은 영영 기다리지 않는다. 보낸 뒤 5초 안에
 * 작업 중이 오지 않으면 빗장이 올라가 다음 글이 나간다. 시계는 턴이 시작되면, 대기열이 비면 지워진다. 이 장면의 첫 글은
 * 그냥 글이다 — 슬래시로 시작하면 입력칸의 팔레트가 Enter를 가져가고, 턴이 시작되지 않는다는 사실은 「작업 중」 훅이
 * 오지 않는 것으로 만든다. */
async function testStartClock(browser, origin, ok) {
  await scene(browser, origin, ok, "start clock runs out", async (page) => {
    await holdClocks(page, 5000, "queueStartWatch");
    const term = await openPane(page, "claude", "working");
    await queueWords(page, "첫 글", "다음 글");
    // The plain road keeps 500 ms between the paste and its Enter (`COMPOSER_SUBMIT_DELAY_MS`); the clock stands once the Enter went.
    await turnEnds(page, term, 1300);
    const waiting = await page.evaluate(LOOK);
    const clocks = await HELD(page);
    await page.evaluate(() => window.__HELD__[0].fn());
    await page.waitForTimeout(1300);
    const after = await page.evaluate(LOOK);
    ok(
      "a_row_that_starts_no_turn_does_not_hold_the_rows_behind_it_for_good: 「첫 글」 went to the pane when the turn ended and no working word came; 「다음 글」 waited behind it with one 5 s clock running, and when the clock ran out it went — paste, then Enter — and the queue is empty",
      SAME(waiting.queue, ["다음 글"]) && SAME(waiting.sent, ["paste:첫 글", ENTER]) && SAME(clocks, [{ ms: 5000, cleared: false }]) &&
        SAME(after.sent, ["paste:첫 글", ENTER, "paste:다음 글", ENTER]) && SAME(after.queue, []),
      JSON.stringify({ waiting: { queue: waiting.queue, sent: waiting.sent, why: waiting.why }, clocks, after: { queue: after.queue, sent: after.sent } }),
    );
  });
  await scene(browser, origin, ok, "start clock ends with the turn", async (page) => {
    await holdClocks(page, 5000, "queueStartWatch");
    const term = await openPane(page, "claude", "working");
    await queueWords(page, "첫 글", "다음 글");
    await turnEnds(page, term, 1300);
    const running = await HELD(page);
    // 보낸 글이 턴을 시작했다: 훅이 작업 중이라 말한다. 빗장은 상태로 올라가고 시계는 지워진다.
    await page.evaluate(async (term) => {
      window.__TELL__({ term, state: "working", agent: "claude" });
      await window.__PAINT__(3);
    }, term);
    const started = await HELD(page);
    const held = await page.evaluate(LOOK);
    await turnEnds(page, term, 1300);
    const ended = await page.evaluate(LOOK);
    ok(
      "the_start_clock_is_cleared_when_the_turn_starts_and_the_next_row_goes_with_the_turns_end: the clock stood while 「다음 글」 waited, the hook's working word cleared it, 「다음 글」 stayed in the queue during the turn and went when the turn ended",
      SAME(running, [{ ms: 5000, cleared: false }]) && SAME(started, [{ ms: 5000, cleared: true }]) && SAME(held.queue, ["다음 글"]) &&
        SAME(ended.sent, ["paste:첫 글", ENTER, "paste:다음 글", ENTER]) && SAME(ended.queue, []),
      JSON.stringify({ running, started, held: held.queue, ended: { queue: ended.queue, sent: ended.sent } }),
    );
  });
  await scene(browser, origin, ok, "start clock ends with the queue", async (page) => {
    await holdClocks(page, 5000, "queueStartWatch");
    const term = await openPane(page, "claude", "working");
    await queueWords(page, "첫 글", "다음 글");
    await turnEnds(page, term, 1300);
    const running = await HELD(page);
    // 기다리던 글을 사람이 지웠다: 기다릴 글이 없으니 시계도 없다.
    await press(page, "drop", 0);
    const dropped = await HELD(page);
    // 그 뒤 빗장이 내려간 채 대기열에 글이 서면(입력줄이 의심스러울 때) 시계가 다시 선다.
    await page.evaluate(() => { window.__COMPOSER__().__workerRun.lineDoubt = true; });
    await queueWords(page, "또 한 글");
    const queuedAgain = await HELD(page);
    const look = await page.evaluate(LOOK);
    ok(
      "the_start_clock_ends_with_an_empty_queue_and_stands_again_for_a_row_queued_behind_a_lowered_bolt: deleting the only waiting row cleared the clock; words queued afterwards behind the doubted line, with the bolt still down, started a clock of their own and waited in the queue",
      SAME(running, [{ ms: 5000, cleared: false }]) && SAME(dropped, [{ ms: 5000, cleared: true }]) &&
        SAME(queuedAgain, [{ ms: 5000, cleared: true }, { ms: 5000, cleared: false }]) && SAME(look.queue, ["또 한 글"]),
      JSON.stringify({ running, dropped, queuedAgain, queue: look.queue }),
    );
  });
}

/* P3-6 — 문이 「다른 입력이 지금 들어가는 중」(`turn-in-flight`)이라 거절하면, 쉬는 판의 행은 누를 단추가 없다. 큐가 임대가
 * 풀릴 때쯤 한 번 더 묻는다. 두 번째도 거절이면 더 묻지 않는다. */
async function testInFlightAskedOnce(browser, origin, ok) {
  await scene(browser, origin, ok, "in flight asked again", async (page) => {
    await holdClocks(page, 600, "settleComposerQueue");
    const term = await openPane(page, "claude", "working");
    await queueWords(page, "가", "나");
    await press(page, "now", 1);
    // 문은 처음에만 거절한다.
    await page.evaluate(() => {
      window.__ACTS__.front = () => {
        if (window.__ACTS__.calls.filter((call) => call.door === "pane_send_now").length === 1) throw "turn-in-flight";
        return null;
      };
    });
    await turnEnds(page, term, 250);
    const refused = await page.evaluate(LOOK);
    const waiting = await HELD(page);
    await page.evaluate(() => window.__HELD__[0].fn());
    await page.waitForTimeout(400);
    const asked = await page.evaluate(LOOK);
    ok(
      "a_door_refusal_for_a_key_still_going_in_is_asked_again_once_the_pane_has_let_go: the door said turn-in-flight, the row stood first again with its sentence and one 600 ms ask was set; when it ran the door was asked for the same words and took them",
      SAME(DOOR_CALLS(refused), [{ door: "pane_send_now", term, text: "나" }]) && SAME(refused.queue, ["나", "가"]) && refused.note.length >= 8 &&
        SAME(waiting, [{ ms: 600, cleared: false }]) &&
        SAME(DOOR_CALLS(asked), [{ door: "pane_send_now", term, text: "나" }, { door: "pane_send_now", term, text: "나" }]) && SAME(asked.queue, ["가"]) && PASTES(asked).length === 0,
      JSON.stringify({ refused: { calls: refused.calls, queue: refused.queue, note: refused.note }, waiting, asked: { calls: asked.calls, queue: asked.queue } }),
    );
  });
  await scene(browser, origin, ok, "in flight asked once", async (page) => {
    await holdClocks(page, 600, "settleComposerQueue");
    const term = await openPane(page, "claude", "working");
    await queueWords(page, "가", "나");
    await press(page, "now", 1);
    await page.evaluate(() => { window.__ACTS__.front = () => { throw "turn-in-flight"; }; });
    await turnEnds(page, term, 250);
    await page.evaluate(() => window.__HELD__[0].fn());
    await page.waitForTimeout(400);
    const twice = await page.evaluate(LOOK);
    const set = await HELD(page);
    ok(
      "a_door_that_stays_busy_is_asked_once_more_and_no_more: the second answer was turn-in-flight too; the queue asked no third time and set no second ask — the row stands first with the sentence the refusal gave",
      SAME(DOOR_CALLS(twice), [{ door: "pane_send_now", term, text: "나" }, { door: "pane_send_now", term, text: "나" }]) &&
        SAME(set, [{ ms: 600, cleared: false }]) && SAME(twice.queue, ["나", "가"]) && twice.note.length >= 8,
      JSON.stringify({ calls: twice.calls, set, queue: twice.queue, note: twice.note }),
    );
  });
}

/* P3-5 — 초점이 터미널 입력(싱크)에 있을 때 — 화살표를 누르면 싱크가 초점을 받는다 — 판의 대화가 앞이면 친 글자는 대화의
 * 입력칸으로 간다(예전에는 받을 판이 없어 말없이 사라졌다). 키를 먼저 가져야 하는 것들은 그대로 가진다. */
async function testSinkLetterToTheConversation(browser, origin, ok) {
  await scene(browser, origin, ok, "sink letter to the conversation", async (page) => {
    await openPane(page, "claude", "done");
    await page.evaluate(() => document.activeElement?.blur?.());
    await page.keyboard.press("ArrowDown");
    const armed = await page.evaluate(() => document.activeElement === keySink);
    await page.keyboard.type("x");
    await page.waitForTimeout(200);
    const seen = await page.evaluate(LOOK);
    ok(
      "a_letter_typed_while_the_sink_holds_the_focus_lands_in_the_conversations_box: an arrow key armed the terminal's sink, then the letter was typed — it is in the composer's box, the focus is there, and nothing was typed at the pane",
      armed && seen.box === "x" && seen.focus.inBox && REACHED(seen.sent).length === 0,
      JSON.stringify({ armed, box: seen.box, focus: seen.focus, sent: seen.sent }),
    );
  });
  await scene(browser, origin, ok, "sink letter keeps what has the keyboard first", async (page) => {
    await openPane(page, "claude", "done");
    const took = await page.evaluate(() => {
      const box = () => window.__COMPOSER__()?.querySelector(".worker-composer-box");
      // 싱크에 초점을 둔 채 키 하나를 `rearmKeySink`에 보이고, 입력칸이 초점을 가져갔는지 본다.
      const press = (init = {}) => {
        keySink.focus();
        rearmKeySink(new KeyboardEvent("keydown", { key: "x", code: "KeyX", bubbles: true, ...init }));
        const stolen = document.activeElement === box();
        keySink.focus();
        return stolen;
      };
      const out = { plain: press() };
      settingsView.hidden = false; out.settings = press(); settingsView.hidden = true;
      // 작업은 가운데 탭이다(tasks-view-geist): 키보드가 묻는 것은 「작업 탭이 초점 판의 앞에 서 있는가」
      // (`tasksInFront`)라서, 마크업만 드러내지 않고 사람이 여는 길로 열고 닫는다.
      setTaskOpen(true); out.tasks = press(); setTaskOpen(false);
      setWorkspaceBoardOpen(true); out.board = press(); setWorkspaceBoardOpen(false);
      termFloat.hidden = false; out.float = press(); termFloat.hidden = true;
      out.chord = press(usesCommandModifier ? { metaKey: true } : { ctrlKey: true });
      out.option = press({ altKey: true });
      out.arrow = press({ key: "ArrowDown", code: "ArrowDown" });
      // 치던 손이 제 터미널을 쥐고 있으면(백엔드가 화면을 옮겼다) 그 나머지는 상자로 가지 않는다. 떠 있는 셸의 터미널은 늘 있다.
      typingRun = { target: { kind: "term", term: FLOAT_TERM }, at: performance.now(), closed: false, moved: true };
      out.running = press();
      typingRun = null;
      keySink.focus();
      return out;
    });
    ok(
      "the_sink_gives_a_letter_to_the_conversation_only_where_nothing_else_has_the_keyboard: with the sink focused the box took the plain letter, and took nothing while the settings, the task view, the board or the floating shell was up, for a ⌘/Ctrl chord, an Option chord, an arrow, or a run of typing aimed at the pane",
      took.plain === true && !took.settings && !took.tasks && !took.board && !took.float && !took.chord && !took.option && !took.arrow && !took.running,
      JSON.stringify(took),
    );
  });
  await scene(browser, origin, ok, "sink letter with the pane's screen in front", async (page) => {
    const term = await openPane(page, "claude", "done");
    await page.evaluate(async (term) => {
      await setPaneChat(term, false);
      await window.__PAINT__(2);
      window.__ACTS__.sent.length = 0;
      keySink.focus();
    }, term);
    await page.keyboard.press("y");
    await page.waitForTimeout(150);
    const screen = await page.evaluate(() => ({ sent: [...window.__ACTS__.sent], sink: document.activeElement === keySink }));
    ok(
      "with_the_panes_own_screen_in_front_the_sink_keeps_its_letter: the same letter goes to the pane as text and the focus stays on the sink — the conversation is not in front, so it takes nothing",
      screen.sent.some((one) => one === "text:y") && screen.sink,
      JSON.stringify(screen),
    );
  });
}
