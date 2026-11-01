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
 *      keeps its height and width — and only for a worker the window already reads an open question of;
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
        calls: window.__CLOSED__.map((call) => ({ ...call, retryRequest: String(call.retryRequest).startsWith("ui-") })),
        note: document.querySelector(`${row} .board-desk-letter-note`)?.textContent ?? "",
        buttons: [...document.querySelectorAll(`${row} .board-desk-letter-act, ${row} .board-desk-letter-close`)].map((one) => !one.hidden),
      };
    }, row);
    ok("a why and Enter make one call on the inbox's door with one receipt name, and the row says it was closed",
      sent.calls.length === 1 && sent.calls[0].run === "run-desk" && sent.calls[0].message === "m-901"
        && sent.calls[0].reason === "물은 판이 사라졌습니다" && sent.calls[0].retryRequest === true
        && /^닫음/.test(sent.note) && sent.buttons.every((shown) => shown === false),
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

// ⑤ ⑥ The sidebar's worker row.
async function sidebarScene({ browser, origin, ok, faults }) {
  const { page: side } = await openWindowTestPage(browser, origin, { faults });
  try {
    await side.setViewportSize({ width: 1280, height: 1600 });
    await standTeamScene(side, { now: Date.now() });
    await stand(side);
    await side.evaluate(() => {
      deskLedger = {
        runs: [{ run: "run-team-b", name: "판정 런", seat: true }, { run: "run-team-a", name: "묶음 런", seat: false }],
        mail: [
          { run: "run-team-b", id: "m-q1", kind: "question", worker: "w-b1", task_id: "t-2001", body: "어느 쪽으로 갈까요?", created_ms: Date.now() - 60_000 },
          { run: "run-team-a", id: "m-q2", kind: "question", worker: "w-a1", task_id: "t-1001", body: "다른 런의 질문", created_ms: Date.now() - 60_000 },
        ],
        news: [], counts: { mail: 2, news: 0, folded: 0 }, tasks: [], stages: [],
      };
    });
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
        top: Math.round(box.top), bottom: Math.round(box.bottom), text: one.textContent, tag: one.tagName };
    });

    const rest = await geometry(rowOf("w-b1"));
    await side.mouse.move(2, 2);
    const idle = await control();
    ok("at rest a worker row shows no 답 없이 닫기", idle.shown === false, JSON.stringify(idle));

    await side.hover(rowOf("w-b1"));
    const hovered = await control();
    const same = await geometry(rowOf("w-b1"));
    ok("pointing at a worker with an open question shows the control over the row, which keeps its height and width",
      hovered.shown && hovered.tag === "BUTTON" && hovered.text === "답 없이 닫기"
        && hovered.left >= same.left && hovered.right <= same.left + same.w && hovered.top >= same.top && hovered.bottom <= same.top + same.h
        && same.w === rest.w && same.h === rest.h,
      JSON.stringify({ rest, same, hovered }));

    await side.hover(rowOf("w-b2"));
    const none = await control();
    ok("a worker the window reads no open question of gets no control", none.shown === false, JSON.stringify(none));

    await side.hover(rowOf("w-a1"));
    const stranger = await control();
    ok("a worker of a run whose seat this window does not hold gets no control", stranger.shown === false, JSON.stringify(stranger));

    // ⑥ The keyboard: the row, Tab, Enter, Esc.
    await side.mouse.move(2, 2);
    await side.focus(rowOf("w-b1"));
    const focused = await control();
    await side.keyboard.press("Tab");
    const tabbed = await side.evaluate(() => ({ onControl: document.activeElement?.classList.contains("team-close") ?? false }));
    ok("focusing the row shows the control and Tab reaches it", focused.shown && tabbed.onControl, JSON.stringify({ focused, tabbed }));

    await side.keyboard.press("Enter");
    const open = await side.evaluate(() => {
      const form = document.querySelector(".team-close-form");
      const field = form?.querySelector(".team-close-field");
      return { shown: form?.checkVisibility() ?? false, focused: document.activeElement === field, max: field?.maxLength ?? null,
        want: INBOX_CLOSE_REASON_MAX };
    });
    ok("Enter opens the why, on the inbox's limit, with the cursor in it", open.shown && open.focused && open.max === 200 && open.max === open.want, JSON.stringify(open));

    await side.keyboard.press("Enter");
    const empty = await side.evaluate(() => ({ calls: window.__CLOSED__.length, error: document.querySelector(".team-close-error")?.textContent ?? "" }));
    ok("an empty why closes nothing and says it is needed", empty.calls === 0 && empty.error === "닫는 까닭을 한 줄 적어야 닫힙니다.", JSON.stringify(empty));

    await side.keyboard.type("답이 필요 없어졌습니다");
    await side.keyboard.press("Enter");
    const closed = await side.evaluate(async () => {
      await window.__PAINTED__();
      return { calls: window.__CLOSED__.map((call) => ({ run: call.run, message: call.message, reason: call.reason })) };
    });
    ok("a why and Enter close that worker's question through the same door",
      JSON.stringify(closed.calls) === JSON.stringify([{ run: "run-team-b", message: "m-q1", reason: "답이 필요 없어졌습니다" }]),
      JSON.stringify(closed));

    await side.focus(rowOf("w-b1"));
    await side.evaluate(() => {
      deskLedger = { ...deskLedger, mail: deskLedger.mail.map((letter) => ({ ...letter })) };
    });
    await side.keyboard.press("Tab");
    await side.keyboard.press("Enter");
    await side.keyboard.press("Escape");
    const left = await side.evaluate(() => ({
      form: document.querySelector(".team-close-form")?.checkVisibility() ?? false,
      onRow: document.activeElement?.dataset.teamKey ?? "",
    }));
    ok("Esc puts the why away and the focus goes back to the row", left.form === false && left.onRow === "w:run-team-b/w-b1", JSON.stringify(left));
    ok("the scenes raise no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await side.close();
  }
}
