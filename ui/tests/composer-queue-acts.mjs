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
 * 끼워 넣기·바로 보내기가 부르지 않고, 백엔드의 문(`pane_steer`·`pane_interrupt`, 선은
 * `wire_steer`·`wire_interrupt`)만 부른다. 그 문이 판을 다시 보고(메뉴·입력줄·원장의 자리)
 * 거절하면 글은 대기열에 그대로 있고 까닭이 한 줄로 보인다. 원장 워커·코디의 판에는
 * 끼워 넣기·바로 보내기가 없다. 질문·권한 메뉴가 떠 있으면 없다.
 *
 * 진짜 CLI는 띄우지 않는다: 문의 답과 판의 상태는 모두 합성이다(사람의 기록이 아니다).
 *
 *   WINDOW_SUITES=composer-queue-acts node ui/tests/window.mjs */
import { openWindowTestPage } from "./window-boot.mjs";

const ENTER = `key:${JSON.stringify({ key: "Enter", ctrl: false, alt: false })}`;
const ESCAPE_KEY = `key:${JSON.stringify({ key: "Escape", ctrl: false, alt: false })}`;

/* 문이 판에 대고 거절할 때 쓰는 낱말(백엔드 `answer_door`의 `TURN_*`)과, 창이 스스로 더하는 둘. */
const DOOR_REFUSALS = ["turn-screen", "turn-seat", "turn-ledger-unread", "turn-idle", "turn-unsupported", "turn-in-flight", "turn-denied"];

/* 한 장면의 도구: 판에 닿은 것, 문이 받은 부름, 시계. 판 합성은 composer-hold와 같은 꼴이다. */
const STAND = async () => {
  window.__ACTS__ = { sent: [], calls: [], steer: null, stop: null, wireSteer: null, wireStop: null, long: [] };
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
    acts: [...row.querySelectorAll("button[data-act]")].map((button) => button.dataset.act),
    editing: row.dataset.editing === "true",
    acting: row.dataset.acting ?? null,
  }));
  return {
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
    ok(`${name}: no page errors`, faults.length === 0, faults.slice(0, 3).join("\n"));
  } finally {
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
    await page.waitForFunction(() => window.__ACTS__.sent.filter((one) => one.startsWith("key:")).length >= 1, null, { timeout: 4000 }).catch(() => null);
    seen.sent = await page.evaluate(LOOK);
    ok(
      "send_now_on_a_pane_stops_through_the_backends_door_and_moves_the_row_to_the_front: 바로 보내기 on the second row asks `pane_interrupt` once, the window types no Esc itself, the row stands first and the others keep their order — and nothing is pasted until the turn has stopped",
      seen.pressed && SAME(seen.stopping.calls, [{ door: "pane_interrupt", term }]) && !seen.stopping.sent.includes(ESCAPE_KEY) &&
        seen.stopping.sent.length === 0 && SAME(seen.stopping.queue, ["나", "가", "다"]),
      JSON.stringify(seen.stopping),
    );
    ok(
      "the_row_sent_now_goes_first_and_the_rest_wait_their_turn: once the pane reports the turn done, the promoted words go down the ordinary road — paste, then Enter — and the other two keep waiting for the pane's next turn",
      SAME(seen.sent.sent, ["paste:나", ENTER]) && SAME(seen.sent.queue, ["가", "다"]),
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
      "the_promoted_row_still_goes_first_when_the_turn_does_end: when the pane finally reports the turn done, the words that were sent-now go before the one that was ahead of them",
      SAME(later.sent, ["paste:나", ENTER]) && SAME(later.queue, ["가"]),
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
