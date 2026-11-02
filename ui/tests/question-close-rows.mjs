import { openWindowTestPage } from "./window-boot.mjs";
import { installBoardWaits } from "./board-waits.mjs";
import { coordinatorDeskFixture } from "./coordinator-desk.mjs";
import { draftScene } from "./board-now.mjs";
import { standTeamScene } from "./sidebar-team.mjs";

/* Close a question without an answer from the rows that show it (t-88847, from the t-85293 review's P3-7).
 *
 * The inbox had the door (`desk_question_close`, t-83281): one line of why, 200 letters at most, the ledger's
 * `question-close` on the run's seat. Asked 2026-11-02: the person should not have to open the inbox to use it —
 * the board's 지금 tab (the 당신 차례 rows) and the sidebar's team (a worker row that has an open question) get
 * the same door, the same limit and the same refusal sentences, through the same function.
 *   ① a question row on the board wears 답 없이 닫기 beside 답하기, and it asks for the why before it closes;
 *   ② no why, no call; one why, one call, with the name of one receipt; the row says it was closed;
 *   ③ a refusal says the one sentence the inbox says and keeps the ledger's own words as its hover;
 *   ④ a window without the run's seat shows no button;
 *   ⑤ the sidebar's worker row shows the control only while it is pointed at or focused, over the row — the row
 *      keeps its height and width — and only for a worker the ledger says is asking; the press reads the ledger once,
 *      closes the question it finds (never an old copy's), names it on the form, and says closed, answered first, or gone;
 *   ⑥ the keyboard reaches it with Tab and leaves with Esc.
 * The fixture is a fake ledger; nothing is closed anywhere. */

async function stand(page) {
  await page.evaluate(() => {
    window.__CLOSED__ = [];
    window.__CLOSE_FAIL__ = null;
    window.__ANSWER__.desk_question_close = (args) => {
      window.__CLOSED__.push(args);
      if (window.__CLOSE_FAIL__) throw new Error(window.__CLOSE_FAIL__);
      return { closed: true, already: false };
    };
  });
}

/* The two scenes stand apart: one that cannot go on says so and the other still runs. */
export async function testQuestionCloseRows(suite) {
  for (const [name, scene] of [["the board's rows", boardScene], ["the sidebar's worker row", sidebarScene]]) {
    try {
      await scene(suite);
    } catch (error) {
      suite.ok(`${name} close a question without an answer to the end of their scene`, false, String(error).split("\n")[0]);
    }
  }
}

async function boardScene({ browser, origin, ok, faults }) {
  const { page } = await openWindowTestPage(browser, origin, { faults });
  try {
    await installBoardWaits(page);
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.evaluate(coordinatorDeskFixture, { tasks: 0, workers: 1, mail: 1 });
    await stand(page);
    await draftScene(page);

    // ① The row on the board.
    const row = '[data-desk-block="asks"] .board-asks > li.board-desk-letter';
    const first = await page.evaluate((row) => {
      const node = document.querySelector(row);
      const close = node?.querySelector(".board-desk-letter-close");
      const act = node?.querySelector(".board-desk-letter-act");
      const lead = node?.querySelector(".board-desk-letter-lead");
      const box = (one) => (one ? { w: Math.round(one.getBoundingClientRect().width), h: Math.round(one.getBoundingClientRect().height) } : null);
      return {
        rows: document.querySelectorAll(row).length,
        close: close ? { text: close.textContent, shown: close.checkVisibility(), box: box(close) } : null,
        act: act ? { text: act.textContent, box: box(act) } : null,
        lead: box(lead),
        form: node?.querySelector(".board-desk-close")?.checkVisibility() ?? false,
        row: box(node),
      };
    }, row);
    ok("a question on the board wears 답 없이 닫기 beside 답하기, and the line stays one button high",
      first.rows === 1 && first.close?.shown === true && first.close.text === "답 없이 닫기" && first.act?.text === "답하기"
        && first.lead !== null && first.lead.h <= first.act.box.h + 2 && first.form === false,
      JSON.stringify(first));

    // ①b On a narrow board the question keeps its width: the text takes the line and the two buttons stand together
    //    under it — at 360 px, in each of the five languages.
    const narrow = {};
    for (const locale of ["ko", "en", "ja", "zh", "es"]) {
      await page.setViewportSize({ width: 360, height: 900 });
      narrow[locale] = await page.evaluate(async ([row, locale]) => {
        setLocale(locale, { persist: false });
        scheduleDeskPaint();
        await window.__PAINTED__();
        const node = document.querySelector(row);
        const box = (one) => one.getBoundingClientRect();
        const line = box(node);
        const body = box(node.querySelector(".board-desk-letter-body"));
        const act = box(node.querySelector(".board-desk-letter-act"));
        const close = box(node.querySelector(".board-desk-letter-close"));
        return {
          bodyShare: Math.round((body.width / line.width) * 100),
          inside: act.right <= line.right + 1 && close.right <= line.right + 1 && close.left >= line.left - 1,
          under: act.top >= body.bottom - 1 && close.top >= body.bottom - 1,
          overflow: node.scrollWidth > node.clientWidth + 1,
        };
      }, [row, locale]);
    }
    await page.evaluate(() => setLocale("ko", { persist: false }));
    await page.setViewportSize({ width: 1440, height: 900 });
    ok("on a narrow board (360 px) the question keeps most of the line and the two buttons stand under it, in five languages",
      Object.values(narrow).every((one) => one.bodyShare >= 70 && one.inside && one.under && !one.overflow), JSON.stringify(narrow));

    await page.click(`${row} .board-desk-letter-close`);
    const asked = await page.evaluate((row) => {
      const form = document.querySelector(`${row} .board-desk-close`);
      const field = form?.querySelector(".board-desk-close-field");
      return {
        shown: form?.checkVisibility() ?? false,
        focused: document.activeElement === field,
        max: field?.maxLength ?? null,
        want: INBOX_CLOSE_REASON_MAX,
        label: field?.getAttribute("aria-label") ?? "",
        sentBy: form?.querySelector(".board-desk-close-send")?.textContent ?? "",
        cancelBy: form?.querySelector(".board-desk-close-cancel")?.textContent ?? "",
      };
    }, row);
    ok("it asks for the why first: a field on the inbox's limit, named, with the cursor in it",
      asked.shown && asked.focused && asked.max === asked.want && asked.max === 200 && asked.label === "닫는 까닭"
        && asked.sentBy === "닫기" && asked.cancelBy === "취소",
      JSON.stringify(asked));

    // ② No why, no call; one why, one call.
    await page.click(`${row} .board-desk-close-send`);
    const empty = await page.evaluate((row) => ({
      calls: window.__CLOSED__.length,
      error: document.querySelector(`${row} .board-desk-close-error`)?.textContent ?? "",
    }), row);
    ok("an empty why closes nothing and says it is needed", empty.calls === 0 && empty.error === "닫는 까닭을 한 줄 적어야 닫힙니다.", JSON.stringify(empty));

    await page.fill(`${row} .board-desk-close-field`, "물은 판이 사라졌습니다");
    await page.keyboard.press("Enter");
    const sent = await page.evaluate(async (row) => {
      await window.__PAINTED__();
      return {
        asked: deskLedger.mail[0].id,
        calls: window.__CLOSED__.map((call) => ({ ...call, retryRequest: String(call.retryRequest).startsWith("ui-") })),
        note: document.querySelector(`${row} .board-desk-letter-note`)?.textContent ?? "",
        buttons: [...document.querySelectorAll(`${row} .board-desk-letter-act, ${row} .board-desk-letter-close`)].map((one) => !one.hidden),
      };
    }, row);
    ok("a why and Enter make one call on the inbox's door with one receipt name, and the row says it was closed",
      sent.calls.length === 1 && sent.calls[0].run === "run-desk" && sent.calls[0].message === sent.asked
        && sent.calls[0].reason === "물은 판이 사라졌습니다" && sent.calls[0].retryRequest === true
        && /^답 없이 닫음/.test(sent.note) && sent.buttons.every((shown) => shown === false),
      JSON.stringify(sent));

    // ③ A refusal says the inbox's sentence; the ledger's words are the hover. A second question stands for it.
    const second = '[data-desk-block="asks"] .board-asks > li[data-letter="run-desk/m-902"]';
    await page.evaluate(async () => {
      window.__CLOSED__.length = 0;
      window.__CLOSE_FAIL__ = "this window does not hold the run's coordinator seat";
      const [asked] = deskLedger.mail;
      deskLedger = { ...deskLedger, mail: [...deskLedger.mail, { ...asked, id: "m-902", body: "둘째 질문" }] };
      scheduleDeskPaint();
      await window.__PAINTED__();
    });
    await page.click(`${second} .board-desk-letter-close`);
    await page.fill(`${second} .board-desk-close-field`, "시험");
    await page.click(`${second} .board-desk-close-send`);
    const refused = await page.evaluate(async (second) => {
      await window.__PAINTED__();
      const error = document.querySelector(`${second} .board-desk-close-error`);
      return { text: error?.textContent ?? "", tip: error?.dataset.tip ?? "", calls: window.__CLOSED__.length };
    }, second);
    ok("a refusal reads the inbox's one sentence and keeps the ledger's own words as its hover",
      refused.calls === 1 && refused.text === "답 없이 닫지 못했습니다 — 이 런의 코디 자리를 이 창이 쥐고 있지 않습니다. 코디 자리가 있는 창에서 닫아 주세요."
        && refused.tip.includes("coordinator seat"),
      JSON.stringify(refused));

    // ③b A question somebody answered first: the ledger says `already` with the answer's id, and the row says
    //    answered — not closed (t-84271's word, on both surfaces).
    const third = '[data-desk-block="asks"] .board-asks > li[data-letter="run-desk/m-903"]';
    await page.evaluate(async () => {
      window.__CLOSED__.length = 0;
      window.__CLOSE_FAIL__ = null;
      window.__ANSWER__.desk_question_close = (args) => {
        window.__CLOSED__.push(args);
        return { closed: false, already: true, answerId: "a-77" };
      };
      const [asked] = deskLedger.mail;
      deskLedger = { ...deskLedger, mail: [...deskLedger.mail, { ...asked, id: "m-903", body: "셋째 질문" }] };
      scheduleDeskPaint();
      await window.__PAINTED__();
    });
    await page.click(`${third} .board-desk-letter-close`);
    await page.fill(`${third} .board-desk-close-field`, "답이 먼저 왔는지 본다");
    await page.click(`${third} .board-desk-close-send`);
    const answeredFirst = await page.evaluate(async (third) => {
      await window.__PAINTED__();
      return { note: document.querySelector(`${third} .board-desk-letter-note`)?.textContent ?? "", calls: window.__CLOSED__.length };
    }, third);
    ok("a question answered first reads 답함, not 답 없이 닫음, on the board's row",
      answeredFirst.calls === 1 && answeredFirst.note === "그 사이 다른 쪽에서 답했어요 · 원장에 적히면 목록에서 빠져요",
      JSON.stringify(answeredFirst));

    // ④ No seat, no button.
    const unseated = await page.evaluate(async (row) => {
      deskLedger = { ...deskLedger, runs: deskLedger.runs.map((run) => ({ ...run, seat: false })) };
      scheduleDeskPaint();
      await window.__PAINTED__();
      const closes = [...document.querySelectorAll('[data-desk-block="asks"] .board-desk-letter-close')];
      return { shown: closes.some((close) => !close.hidden && close.checkVisibility()), rows: closes.length };
    }, row);
    ok("a window that does not hold the run's seat shows no 답 없이 닫기", unseated.shown === false, JSON.stringify(unseated));
  } finally {
    await page.close();
  }
}

// ⑤ ⑥ The sidebar's worker row (r2: the control reads the ledger at the press, not a copy the board may never have taken).
async function sidebarScene({ browser, origin, ok, faults }) {
  const { page: side } = await openWindowTestPage(browser, origin, { faults });
  try {
    await side.setViewportSize({ width: 1280, height: 1600 });
    await standTeamScene(side, { now: Date.now() });
    await stand(side);
    await side.evaluate(() => {
      // The board was never opened in this window: its own copy of the desk is empty. What the control reads is the
      // ledger at the press.
      deskLedger = null;
      window.__DESK_READS__ = 0;
      window.__LETTER__ = (id, worker, body, minutes, over = {}) => ({
        run: "run-team-b", id, kind: "question", worker, task_id: worker === "w-b1" ? "t-2001" : "t-2002", body,
        created_ms: Date.now() - minutes * 60_000, ...over,
      });
      window.__FRESH__ = {
        runs: [{ run: "run-team-b", name: "판정 런", seat: true }],
        mail: [window.__LETTER__("m-q1", "w-b1", "어느 쪽으로 갈까요?\n둘째 줄은 폼의 머리에 들어가지 않습니다", 5)],
        news: [], counts: { mail: 1, news: 0, folded: 0 }, tasks: [], stages: [],
      };
      window.__ANSWER__.board_desk = () => {
        window.__DESK_READS__ += 1;
        return window.__FRESH__;
      };
      // A worker of another run asks too, in a run whose seat this window does not hold.
      window.__LEDGER__.find((row) => row.worker === "w-a1").asking = true;
      window.__SEATS__ = window.__SEATS__.map((seat) => (seat.run === "run-team-a" ? { ...seat, seat: false } : seat));
    });
    await side.evaluate(() => window.__LEDGER_MOVED__());
    const rowOf = (worker) => `#sidebar-team [data-team-key$="/${worker}"]`;
    const geometry = (selector) => side.evaluate((selector) => {
      const node = document.querySelector(selector);
      const box = node.getBoundingClientRect();
      return { w: Math.round(box.width), h: Math.round(box.height), top: Math.round(box.top), left: Math.round(box.left) };
    }, selector);
    const control = () => side.evaluate(() => {
      const one = document.querySelector(".team-close");
      if (!one) return { present: false, shown: false };
      const box = one.getBoundingClientRect();
      return { present: true, shown: one.checkVisibility() && box.width > 0, left: Math.round(box.left), right: Math.round(box.right),
        top: Math.round(box.top), bottom: Math.round(box.bottom), text: one.textContent, tag: one.tagName,
        label: one.getAttribute("aria-label") ?? "" };
    });
    const toasts = () => side.evaluate(() => [...document.querySelectorAll(".toast")].map((node) => node.textContent));

    const rest = await geometry(rowOf("w-b1"));
    await side.mouse.move(2, 2);
    const idle = await control();
    ok("at rest a worker row shows no 답 없이 닫기", idle.shown === false, JSON.stringify(idle));

    await side.hover(rowOf("w-b1"));
    const hovered = await control();
    const same = await geometry(rowOf("w-b1"));
    const readsAtHover = await side.evaluate(() => ({ reads: window.__DESK_READS__, desk: deskLedger }));
    ok("pointing at an asking worker shows the control over the row — with the board never opened, and no ledger read of its own — and the row keeps its height and width",
      hovered.shown && hovered.tag === "BUTTON" && hovered.text === "답 없이 닫기" && readsAtHover.desk === null && readsAtHover.reads === 0
        && hovered.left >= same.left && hovered.right <= same.left + same.w && hovered.top >= same.top && hovered.bottom <= same.top + same.h
        && same.w === rest.w && same.h === rest.h,
      JSON.stringify({ rest, same, hovered, readsAtHover }));
    ok("the control's name says which worker's question it closes",
      hovered.label.startsWith("답 없이 닫기") && hovered.label.includes("Jev: 판정이 실제로 일어나게"), hovered.label);

    await side.hover(rowOf("w-b2"));
    const none = await control();
    ok("a worker the ledger does not say is asking gets no control", none.shown === false, JSON.stringify(none));

    await side.hover(rowOf("w-a1"));
    const stranger = await control();
    ok("an asking worker of a run whose seat this window does not hold gets no control", stranger.shown === false, JSON.stringify(stranger));

    // The window's copy of the desk is old: it still holds a question the worker has since been answered.
    await side.evaluate(() => {
      deskLedger = { runs: [{ run: "run-team-b", name: "판정 런", seat: true }], mail: [window.__LETTER__("m-old", "w-b1", "옛 질문", 40)],
        news: [], counts: { mail: 1, news: 0, folded: 0 }, tasks: [], stages: [] };
    });

    // ⑥ The keyboard: the row, Tab, Enter, Esc.
    await side.mouse.move(2, 2);
    await side.focus(rowOf("w-b1"));
    const focused = await control();
    await side.keyboard.press("Tab");
    const tabbed = await side.evaluate(() => ({ onControl: document.activeElement?.classList.contains("team-close") ?? false }));
    ok("focusing the row shows the control and Tab reaches it", focused.shown && tabbed.onControl, JSON.stringify({ focused, tabbed }));

    await side.keyboard.press("Enter");
    await side.waitForFunction(() => document.querySelector(".team-close-form")?.checkVisibility() === true);
    const open = await side.evaluate(() => {
      const form = document.querySelector(".team-close-form");
      const field = form?.querySelector(".team-close-field");
      return { shown: form?.checkVisibility() ?? false, focused: document.activeElement === field, max: field?.maxLength ?? null,
        want: INBOX_CLOSE_REASON_MAX, reads: window.__DESK_READS__,
        who: form?.querySelector(".team-close-who")?.textContent ?? "", ask: form?.querySelector(".team-close-ask")?.textContent ?? "",
        count: form?.querySelector(".team-close-count")?.textContent ?? "", tip: form?.querySelector(".team-close-head")?.dataset.tip ?? "" };
    });
    ok("Enter reads the ledger once, and the why opens on the inbox's limit with the cursor in it",
      open.shown && open.focused && open.max === 200 && open.max === open.want && open.reads === 1, JSON.stringify(open));
    ok("the form says whose question it closes and what it says — the first line, with the whole of it as the hover — and not a count for one question",
      open.who === "Jev: 판정이 실제로 일어나게" && open.ask === "어느 쪽으로 갈까요?" && open.tip.includes("둘째 줄") && open.count === "",
      JSON.stringify(open));

    await side.keyboard.press("Enter");
    const empty = await side.evaluate(() => ({ calls: window.__CLOSED__.length, error: document.querySelector(".team-close-error")?.textContent ?? "" }));
    ok("an empty why closes nothing and says it is needed", empty.calls === 0 && empty.error === "닫는 까닭을 한 줄 적어야 닫힙니다.", JSON.stringify(empty));

    await side.keyboard.type("답이 필요 없어졌습니다");
    await side.keyboard.press("Enter");
    const closed = await side.evaluate(async () => {
      await window.__PAINTED__();
      return {
        calls: window.__CLOSED__.map((call) => ({ run: call.run, message: call.message, reason: call.reason })),
        shown: [...document.querySelectorAll(".toast")].map((node) => node.textContent),
      };
    });
    ok("a why and Enter close the question the ledger holds now — not the one the window's old copy held — through the same door",
      JSON.stringify(closed.calls) === JSON.stringify([{ run: "run-team-b", message: "m-q1", reason: "답이 필요 없어졌습니다" }]),
      JSON.stringify(closed));
    ok("the sidebar says once that it was closed",
      closed.shown.some((words) => words.startsWith("답 없이 닫음")), JSON.stringify(closed.shown));

    // A question answered first: the ledger says `already` with the answer's id, and the sidebar says answered.
    await side.evaluate(() => {
      window.__FRESH__ = { ...window.__FRESH__, mail: [window.__LETTER__("m-q3", "w-b2", "둘째 워커의 질문", 3)] };
      window.__LEDGER__.find((row) => row.worker === "w-b2").asking = true;
      window.__CLOSED__.length = 0;
      window.__ANSWER__.desk_question_close = (args) => {
        window.__CLOSED__.push(args);
        return { closed: false, already: true, answerId: "a-9" };
      };
    });
    await side.evaluate(() => window.__LEDGER_MOVED__());
    await side.hover(rowOf("w-b2"));
    await side.click(".team-close");
    await side.waitForFunction(() => document.querySelector(".team-close-form")?.checkVisibility() === true);
    await side.keyboard.type("먼저 답이 왔는지 본다");
    await side.keyboard.press("Enter");
    await side.waitForFunction(() => window.__CLOSED__.length === 1);
    const answeredFirst = await toasts();
    ok("a question answered first is told as answered, not as closed",
      answeredFirst.some((words) => words.startsWith("그 사이 다른 쪽에서 답했어요")),
      JSON.stringify(answeredFirst));

    // A worker the ledger no longer holds an open question of: the press says so and closes nothing.
    await side.evaluate(() => {
      window.__FRESH__ = { ...window.__FRESH__, mail: [] };
      window.__CLOSED__.length = 0;
    });
    await side.mouse.move(2, 2);
    await side.hover(rowOf("w-b1"));
    await side.click(".team-close");
    const gone = await side.evaluate(async () => {
      await window.__PAINTED__();
      return { form: document.querySelector(".team-close-form")?.checkVisibility() ?? false, calls: window.__CLOSED__.length,
        shown: [...document.querySelectorAll(".toast")].map((node) => node.textContent) };
    });
    ok("a press that finds no open question of that worker says so and opens no form",
      gone.form === false && gone.calls === 0 && gone.shown.some((words) => words === "이 워커의 열린 질문이 원장에 없습니다 — 이미 답했거나 닫혔을 수 있어요"),
      JSON.stringify(gone));

    // Two questions of one worker: the oldest, said so.
    await side.evaluate(() => {
      window.__FRESH__ = { ...window.__FRESH__, mail: [
        window.__LETTER__("m-q5", "w-b1", "나중 질문", 1), window.__LETTER__("m-q4", "w-b1", "먼저 한 질문", 9)] };
      window.__CLOSED__.length = 0;
    });
    await side.mouse.move(2, 2);
    await side.hover(rowOf("w-b1"));
    await side.click(".team-close");
    await side.waitForFunction(() => document.querySelector(".team-close-form")?.checkVisibility() === true);
    const two = await side.evaluate(() => ({
      ask: document.querySelector(".team-close-ask")?.textContent ?? "", count: document.querySelector(".team-close-count")?.textContent ?? "" }));
    ok("with two open questions the form names the oldest and says there are two",
      two.ask === "먼저 한 질문" && two.count === "2건 중 가장 오래된 질문", JSON.stringify(two));

    // Esc from a button of the form puts it away as well, and the focus goes back to the row.
    await side.focus(".team-close-cancel");
    await side.keyboard.press("Escape");
    const escaped = await side.evaluate(() => ({ form: document.querySelector(".team-close-form")?.checkVisibility() ?? false,
      onRow: document.activeElement?.dataset.teamKey ?? "", calls: window.__CLOSED__.length }));
    ok("Esc on the form's own button puts the why away, closes nothing, and the focus goes back to the row",
      escaped.form === false && escaped.onRow === "w:run-team-b/w-b1" && escaped.calls === 0, JSON.stringify(escaped));

    // The rows move under the control: it and the form go with the row they were opened for.
    await side.evaluate(() => {
      window.__LEDGER__.find((row) => row.worker === "w-b2").asking = true;
      window.__FRESH__ = { ...window.__FRESH__, mail: [window.__LETTER__("m-q6", "w-b1", "움직이는 행의 질문", 4)] };
    });
    await side.evaluate(() => window.__LEDGER_MOVED__());
    await side.mouse.move(2, 2);
    await side.hover(rowOf("w-b1"));
    await side.click(".team-close");
    await side.waitForFunction(() => document.querySelector(".team-close-form")?.checkVisibility() === true);
    await side.evaluate(async () => {
      // The ledger lists the second worker first: both ask, so the order is the ledger's.
      window.__LEDGER__.reverse();
      await window.__LEDGER_MOVED__();
    });
    const followed = await side.evaluate(() => {
      const row = document.querySelector('#sidebar-team [data-team-key$="/w-b1"]').getBoundingClientRect();
      const form = document.querySelector(".team-close-form").getBoundingClientRect();
      return { rowTop: Math.round(row.top), rowBottom: Math.round(row.bottom), formTop: Math.round(form.top), formBottom: Math.round(form.bottom),
        who: document.querySelector(".team-close-who")?.textContent ?? "" };
    });
    ok("when the rows change order the form follows its worker's row (under it, or above it when the window ends first) and still names that worker",
      (Math.abs(followed.formTop - followed.rowBottom) <= 2 || Math.abs(followed.formBottom - followed.rowTop) <= 2)
        && followed.who === "Jev: 판정이 실제로 일어나게",
      JSON.stringify(followed));

    // Leaving the team view takes the control and the form away with it.
    await side.evaluate(() => setSidebarView({ layout: "branches" }));
    await side.evaluate(() => window.__PAINTED__());
    const away = await side.evaluate(() => ({
      button: document.querySelector(".team-close")?.checkVisibility() ?? false,
      form: document.querySelector(".team-close-form")?.checkVisibility() ?? false }));
    ok("leaving the team view takes the control and the form away", away.button === false && away.form === false, JSON.stringify(away));
    ok("the scenes raise no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await side.close();
  }
}
