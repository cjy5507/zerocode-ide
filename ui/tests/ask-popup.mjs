import { openWindowTestPage } from "./window-boot.mjs";

/* The ask popup: every question the window puts to the person, in one place.
 *
 * A tool permission, a question an agent asks (zo's model switch), the
 * Computer Use confirm and its hand-over, and the window's own confirm
 * (`askConfirm`) are five KINDS of ask. They share one frame, one queue, one
 * keyboard and one seat in the middle of the window, and each kind wears its
 * own title and body. This suite drives the popup only through what the
 * backend sends (`session:frame`, `computer:confirm`, `computer:handoff`) and
 * what a person does (keys and clicks), and reads it back off the document —
 * never through the popup's own variables — so it says what the person sees.
 *
 *   WINDOW_SUITES=ask-popup node ui/tests/window.mjs */

/* The titles in the five languages the window speaks, pinned here in words so
 * a catalog that drifted into another language, or into a key nobody
 * translated, fails instead of agreeing with itself. */
export const ASK_TITLES = Object.freeze({
  tool: { ko: "이 도구를 실행할까요?", en: "Run this tool?", ja: "このツールを実行しますか？", zh: "要运行这个工具吗？", es: "¿Ejecutar esta herramienta?" },
  model_switch: { ko: "모델을 바꿀까요?", en: "Switch models?", ja: "モデルを切り替えますか？", zh: "要切换模型吗？", es: "¿Cambiar de modelo?" },
  declined_images: { ko: "이미지 없이 다시 시도할까요?", en: "Retry without the images?", ja: "画像なしで再試行しますか？", zh: "要不带图片重试吗？", es: "¿Reintentar sin las imágenes?" },
  question: { ko: "에이전트가 묻습니다", en: "The agent has a question", ja: "エージェントから質問があります", zh: "智能体有个问题", es: "El agente tiene una pregunta" },
  payment: { ko: "결제 단추를 누를까요?", en: "Press the payment control?", ja: "支払いボタンを押しますか？", zh: "要按下支付按钮吗？", es: "¿Pulsar el control de pago?" },
  transfer: { ko: "이체 단추를 누를까요?", en: "Press the transfer control?", ja: "振込ボタンを押しますか？", zh: "要按下转账按钮吗？", es: "¿Pulsar el control de transferencia?" },
  delete: { ko: "삭제 단추를 누를까요?", en: "Press the delete control?", ja: "削除ボタンを押しますか？", zh: "要按下删除按钮吗？", es: "¿Pulsar el control de borrado?" },
  handoff: { ko: "사람이 할 차례", en: "Your turn", ja: "あなたの番です", zh: "轮到你了", es: "Te toca a ti" },
});

/* The hands every scenario shares, put on the page once: what the backend
 * says (`frame`, `event`), what the popup shows (`look`), and the two
 * answers it can be given (`sent`: a permission's, `answered`: Computer
 * Use's). */
const installHands = () => {
  const T = {
    settle: (ms = 90) => new Promise((done) => setTimeout(done, ms)),
    fire(name, payload) {
      for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload });
    },
    frame(session, body) {
      T.fire("session:frame", { session, frame: body });
    },
    ended(session) {
      T.fire("session:ended", { session, reason: null });
    },
    sent: [],
    answered: [],
    reset() {
      T.sent.length = 0;
      T.answered.length = 0;
    },
    /* What the person would read off the popup right now. A part that is
     * hidden reads as null, so "no tool chip" is `mono === null`. */
    look() {
      const scrim = document.getElementById("ask-scrim");
      const section = scrim?.querySelector(".permission");
      if (!scrim || !section) return { present: false, shown: false };
      const said = (id) => {
        const node = document.getElementById(id);
        return node && !node.hidden && !node.closest("[hidden]") ? node.textContent.trim() : null;
      };
      const rect = section.getBoundingClientRect();
      const buttons = [...section.querySelectorAll("button")].filter((one) => !one.closest("[hidden]"));
      const focus = document.activeElement;
      return {
        present: true,
        shown: !scrim.hidden && !scrim.classList.contains("is-closing"),
        kind: section.dataset.kind ?? null,
        agent: said("ask-agent"),
        title: said("ask-title"),
        mono: said("ask-body"),
        why: said("ask-why"),
        clock: said("ask-clock"),
        more: said("ask-more"),
        buttons: buttons.map((one) => one.textContent.trim()),
        focus: focus && scrim.contains(focus)
          ? (focus.tagName === "BUTTON" ? focus.textContent.trim() : "(dialog)")
          : null,
        rect: {
          cx: Math.round(rect.left + rect.width / 2),
          cy: Math.round(rect.top + rect.height / 2),
          top: Math.round(rect.top),
          bottom: Math.round(rect.bottom),
          width: Math.round(rect.width),
          vw: innerWidth,
          vh: innerHeight,
        },
      };
    },
  };
  window.__ASK_T__ = T;
  window.__ANSWER__.respond_permission = (args) => {
    T.sent.push(JSON.parse(JSON.stringify(args)));
    return null;
  };
  window.__ANSWER__.computer_confirm_answer = (args) => {
    T.answered.push(JSON.parse(JSON.stringify(args)));
    return true;
  };
};

/* Runs one scenario on a fresh page and turns a throw into a failed check
 * with its reason, so one broken scenario does not hide the rest. */
async function scenario(browser, origin, name, ok, body) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.evaluate(installHands);
    await body(page, faults);
  } catch (error) {
    ok(`the ${name} scenario ran to its end`, false, String(error?.stack ?? error).split("\n").slice(0, 3).join(" | "));
  } finally {
    await page.close();
  }
}

/* ------------------------------------------------------------------ kinds */

async function tool(browser, origin, ok) {
  await scenario(browser, origin, "tool permission", ok, async (page, faults) => {
    const seen = await page.evaluate(async () => {
      const T = window.__ASK_T__;
      T.frame("s-a", {
        type: "permission_prompt",
        prompt_id: 11,
        tool_name: "bash",
        reasoning: "git status를 실행해 작업 트리 상태를 확인합니다.",
        audit_hint: "risk: medium; explicitly unblock with [y] Allow once",
        choices: [
          { key: "y", label: "한 번 허용", decision: "allow_once" },
          { key: "n", label: "거부", decision: "deny" },
        ],
      });
      await T.settle();
      return { ...T.look(), shortSession: shortSession("s-a") };
    });
    ok(
      "a tool permission wears its own title, its tool chip and its risk line, and the key that opens it is the first choice",
      seen.shown && seen.kind === "tool" && seen.title === ASK_TITLES.tool.ko && seen.mono === "bash"
        && seen.why.includes("git status") && seen.why.includes("risk: medium")
        && seen.agent.includes(seen.shortSession) && seen.buttons.join() === "한 번 허용,거부"
        && seen.focus === "한 번 허용" && seen.more === null,
      JSON.stringify(seen),
    );
    await page.keyboard.press("Enter");
    await page.waitForTimeout(90);
    const answered = await page.evaluate(() => window.__ASK_T__.sent.slice());
    ok(
      "Enter answers the tool permission with the choice it opened on, by the permission road",
      answered.length === 1 && answered[0].session === "s-a" && answered[0].promptId === 11 && answered[0].decision === "allow_once",
      JSON.stringify(answered),
    );

    // A frame that names the classifier's decline as its tool but says nothing
    // of its kind is a tool prompt: the window never recognises a question by
    // a tool string.
    const decline = await page.evaluate(async () => {
      const T = window.__ASK_T__;
      T.reset();
      T.frame("s-a", {
        type: "permission_prompt",
        prompt_id: 12,
        tool_name: "safety-classifier decline",
        reasoning: "declined",
        audit_hint: null,
        choices: [{ key: "y", label: "허용", decision: "allow_once" }, { key: "n", label: "거부", decision: "deny" }],
      });
      await T.settle();
      return T.look();
    });
    ok(
      "a frame with no kind is a tool prompt whatever its tool is called",
      decline.kind === "tool" && decline.title === ASK_TITLES.tool.ko && decline.mono === "safety-classifier decline",
      JSON.stringify(decline),
    );
    await page.keyboard.press("Escape");
    await page.waitForTimeout(90);
    const escaped = await page.evaluate(() => window.__ASK_T__.sent.slice());
    ok(
      "Escape refuses a tool permission: the deny choice goes back for that very prompt",
      escaped.length === 1 && escaped[0].promptId === 12 && escaped[0].decision === "deny",
      JSON.stringify(escaped),
    );
    const explicit = await page.evaluate(async () => {
      const T = window.__ASK_T__;
      T.frame("s-a", { type: "permission_prompt", prompt_id: 13, kind: "tool", tool_name: "write", reasoning: "", audit_hint: null });
      await T.settle();
      const seen = T.look();
      T.reset();
      return seen;
    });
    ok(
      "a frame that says kind tool is the same tool prompt, with the default allow and deny when it lists no choices",
      explicit.kind === "tool" && explicit.title === ASK_TITLES.tool.ko && explicit.mono === "write"
        && explicit.buttons.join() === "허용,거부" && explicit.why === null,
      JSON.stringify(explicit),
    );
    ok("the tool permission scenario raised no renderer errors", faults.length === 0, faults.join("\n"));
  });
}

async function question(browser, origin, ok) {
  await scenario(browser, origin, "model switch", ok, async (page, faults) => {
    const seen = await page.evaluate(async () => {
      const T = window.__ASK_T__;
      T.frame("s-zo", {
        type: "permission_prompt",
        prompt_id: 21,
        kind: "question",
        topic: "model_switch",
        title: "Switch models?",
        tool_name: "",
        reasoning: "이 요청은 지금 쓰는 모델의 안전 분류기가 거절했습니다. 다른 모델로 바꿔 다시 시도할까요?",
        audit_hint: null,
        choices: [
          { key: "y", label: "모델 바꾸기", decision: "allow_once" },
          { key: "n", label: "그대로 두기", decision: "deny" },
        ],
      });
      await T.settle();
      return { ...T.look(), shortSession: shortSession("s-zo") };
    });
    ok(
      "zo's model-switch question reads 「모델을 바꿀까요?」 with no tool chip and no risk line",
      seen.shown && seen.kind === "question" && seen.title === ASK_TITLES.model_switch.ko && seen.mono === null
        && seen.why === "이 요청은 지금 쓰는 모델의 안전 분류기가 거절했습니다. 다른 모델로 바꿔 다시 시도할까요?"
        && !JSON.stringify(seen).includes("risk") && !JSON.stringify(seen).includes("이 도구"),
      JSON.stringify(seen),
    );
    ok(
      "the question keeps its own choices, opens on the first, and names who asks",
      seen.buttons.join() === "모델 바꾸기,그대로 두기" && seen.focus === "모델 바꾸기" && seen.agent.includes(seen.shortSession),
      JSON.stringify(seen),
    );
    await page.locator("#ask-scrim").getByRole("button", { name: "그대로 두기", exact: true }).click();
    await page.waitForTimeout(90);
    const declined = await page.evaluate(() => window.__ASK_T__.sent.slice());
    ok(
      "the question is answered by the permission road with the decision tag of the choice pressed",
      declined.length === 1 && declined[0].session === "s-zo" && declined[0].promptId === 21 && declined[0].decision === "deny",
      JSON.stringify(declined),
    );

    // An audit_hint on a question is not drawn: a question has no risk to name.
    const hinted = await page.evaluate(async () => {
      const T = window.__ASK_T__;
      T.reset();
      T.frame("s-zo", {
        type: "permission_prompt",
        prompt_id: 22,
        kind: "question",
        topic: "declined_images",
        title: "Retry without the images?",
        tool_name: "read_file",
        reasoning: "이미지를 빼고 다시 시도할까요?",
        audit_hint: "risk: high; explicitly unblock with [y] Retry",
      });
      await T.settle();
      return T.look();
    });
    ok(
      "a question never draws a tool chip or a risk line even if the frame carries them, and the other topic has its own title",
      hinted.kind === "question" && hinted.title === ASK_TITLES.declined_images.ko && hinted.mono === null
        && hinted.why === "이미지를 빼고 다시 시도할까요?",
      JSON.stringify(hinted),
    );
    await page.keyboard.press("Escape");
    await page.waitForTimeout(90);
    const refused = await page.evaluate(() => window.__ASK_T__.sent.slice());
    ok(
      "Escape refuses a question by the same road, for that question",
      refused.length === 1 && refused[0].promptId === 22 && refused[0].decision === "deny",
      JSON.stringify(refused),
    );

    // A topic the window has no words for falls back to the frame's own title;
    // a question with neither says only that it is one.
    const unknown = await page.evaluate(async () => {
      const T = window.__ASK_T__;
      T.reset();
      T.frame("s-zo", { type: "permission_prompt", prompt_id: 23, kind: "question", topic: "made_up_topic", title: "Do the thing?", reasoning: "질문" });
      await T.settle();
      return T.look();
    });
    await page.keyboard.press("Escape");
    await page.waitForTimeout(260);
    const nameless = await page.evaluate(async () => {
      const T = window.__ASK_T__;
      T.frame("s-zo", { type: "permission_prompt", prompt_id: 24, kind: "question", reasoning: "말없는 질문" });
      await T.settle();
      return T.look();
    });
    ok(
      "an unknown topic falls back to the frame's own title, and a question with neither has the generic one",
      unknown.title === "Do the thing?" && unknown.kind === "question" && nameless.title === ASK_TITLES.question.ko,
      JSON.stringify({ unknown, nameless }),
    );
    // The nameless question is still up: put it away before the next.
    await page.keyboard.press("Escape");
    await page.waitForTimeout(260);
    // Enter gives the choice that applies ONCE, wherever the frame lists it —
    // never the one that stays ("from now on"), which is picked by a Tab or a click.
    const once = await page.evaluate(async () => {
      const T = window.__ASK_T__;
      T.reset();
      T.frame("s-zo", {
        type: "permission_prompt", prompt_id: 25, kind: "question", topic: "model_switch", reasoning: "질문", audit_hint: null,
        choices: [
          { key: "a", label: "이후로 계속", decision: "allow_always" },
          { key: "y", label: "이번만", decision: "allow_once" },
          { key: "n", label: "그대로 두기", decision: "deny" },
        ],
      });
      await T.settle(300);
      return T.look();
    });
    await page.keyboard.press("Enter");
    await page.waitForTimeout(150);
    const onceSent = await page.evaluate(() => window.__ASK_T__.sent.slice());
    ok(
      "Enter on a question gives the choice that applies once, wherever the frame lists it — never the one that stays",
      once.buttons.join() === "이후로 계속,이번만,그대로 두기" && once.focus === "이번만"
        && onceSent.length === 1 && onceSent[0].promptId === 25 && onceSent[0].decision === "allow_once",
      JSON.stringify({ once, onceSent }),
    );
    // With no once-only choice Enter gets the plain refusal, and with nothing safe it gets nothing.
    const stays = await page.evaluate(async () => {
      const T = window.__ASK_T__;
      T.reset();
      T.frame("s-zo", {
        type: "permission_prompt", prompt_id: 26, kind: "question", topic: "model_switch", reasoning: "질문", audit_hint: null,
        choices: [
          { key: "a", label: "이후로 계속", decision: "allow_always" },
          { key: "n", label: "그대로 두기", decision: "deny" },
        ],
      });
      await T.settle(300);
      return T.look().focus;
    });
    await page.keyboard.press("Enter");
    await page.waitForTimeout(150);
    const staysSent = await page.evaluate(() => window.__ASK_T__.sent.slice());
    const nothing = await page.evaluate(async () => {
      const T = window.__ASK_T__;
      T.reset();
      T.frame("s-zo", {
        type: "permission_prompt", prompt_id: 27, kind: "question", topic: "model_switch", reasoning: "질문", audit_hint: null,
        choices: [{ key: "a", label: "이후로 계속", decision: "allow_always" }],
      });
      await T.settle(300);
      return T.look().focus;
    });
    await page.keyboard.press("Enter");
    await page.waitForTimeout(150);
    const nothingSent = await page.evaluate(() => window.__ASK_T__.sent.slice());
    ok(
      "a question with no once-only choice hands Enter its refusal, and one with nothing safe hands it nothing",
      stays === "그대로 두기" && staysSent.length === 1 && staysSent[0].decision === "deny"
        && nothing === "(dialog)" && nothingSent.length === 0,
      JSON.stringify({ stays, staysSent, nothing, nothingSent }),
    );
    ok("the model switch scenario raised no renderer errors", faults.length === 0, faults.join("\n"));
  });
}

async function computerConfirm(browser, origin, ok) {
  await scenario(browser, origin, "Computer Use confirm", ok, async (page, faults) => {
    const seen = await page.evaluate(async () => {
      const T = window.__ASK_T__;
      T.fire("computer:confirm", { id: "confirm-1", kind: "payment", label: "Place order", verb: "mouse-click", timeoutMs: 120000 });
      await T.settle();
      const look = T.look();
      return { ...look, oldPillIsNode: document.getElementById("computer-confirm") !== null };
    });
    ok(
      "the Computer Use confirm is a popup of its own kind: its title, the control it will press, and its clock",
      seen.shown && seen.kind === "computer-confirm" && seen.title === ASK_TITLES.payment.ko
        && seen.why.includes("Place order") && seen.clock.includes("120") && seen.agent === "컴퓨터 사용",
      JSON.stringify(seen),
    );
    ok(
      "it stands in the middle of the window, not along the top edge",
      Math.abs(seen.rect.cx - seen.rect.vw / 2) <= 2 && Math.abs(seen.rect.cy - seen.rect.vh / 2) <= seen.rect.vh * 0.1
        && seen.rect.top > seen.rect.vh * 0.2,
      JSON.stringify(seen.rect),
    );
    ok(
      "the old pill at the top of the window is gone — the question lives in the one popup",
      seen.oldPillIsNode === false,
      JSON.stringify({ oldPill: seen.oldPillIsNode }),
    );
    ok(
      "it keeps its default of refusing: the refusal is first, has the focus, and the clock counts toward it",
      seen.buttons.join() === "거부,허용" && seen.focus === "거부" && seen.clock.includes("거부"),
      JSON.stringify(seen),
    );

    // The clock counts down on its own.
    const ticking = await page.evaluate(async () => {
      const T = window.__ASK_T__;
      T.fire("computer:confirm-closed", { id: "confirm-1", decision: "timedout" });
      await T.settle(260);
      T.fire("computer:confirm", { id: "confirm-2", kind: "delete", label: "Delete account", verb: "click", timeoutMs: 6000 });
      await T.settle();
      const first = T.look().clock;
      await T.settle(1300);
      return { first, later: T.look().clock, title: T.look().title };
    });
    const seconds = (words) => Number.parseInt((words ?? "").replace(/\D+/g, ""), 10);
    ok(
      "the clock is repainted every second, and a delete confirm has its own title",
      seconds(ticking.later) < seconds(ticking.first) && ticking.title === ASK_TITLES.delete.ko,
      JSON.stringify(ticking),
    );

    // Enter, Escape and a click each answer by id.
    await page.keyboard.press("Enter");
    await page.waitForTimeout(90);
    await page.evaluate(() => window.__ASK_T__.fire("computer:confirm", { id: "confirm-3", kind: "transfer", label: "Send", timeoutMs: 120000 }));
    await page.waitForTimeout(260);
    await page.keyboard.press("Escape");
    await page.waitForTimeout(90);
    await page.evaluate(() => window.__ASK_T__.fire("computer:confirm", { id: "confirm-4", kind: "payment", label: "Pay", timeoutMs: 120000 }));
    await page.waitForTimeout(260);
    await page.locator("#ask-scrim").getByRole("button", { name: "허용", exact: true }).click();
    await page.waitForTimeout(90);
    const answered = await page.evaluate(() => window.__ASK_T__.answered.slice());
    ok(
      "Enter refuses, Escape refuses, and only pressing 허용 allows — each answers by the id it was asked under",
      JSON.stringify(answered) === JSON.stringify([
        { id: "confirm-2", allow: false },
        { id: "confirm-3", allow: false },
        { id: "confirm-4", allow: true },
      ]),
      JSON.stringify(answered),
    );
    ok("the Computer Use confirm scenario raised no renderer errors", faults.length === 0, faults.join("\n"));
  });
}

async function computerHandoff(browser, origin, ok) {
  await scenario(browser, origin, "Computer Use hand-over", ok, async (page, faults) => {
    const seen = await page.evaluate(async () => {
      const T = window.__ASK_T__;
      T.fire("computer:handoff", { id: "handoff-1", reason: "휴대폰의 2FA 코드를 입력해 주세요", timeoutMs: 600000 });
      await T.settle();
      return { ...T.look(), oldCard: document.getElementById("computer-handoff") !== null };
    });
    ok(
      "the person's turn is a popup of its own kind: its title, the reason, and its clock",
      seen.shown && seen.kind === "computer-handoff" && seen.title === ASK_TITLES.handoff.ko
        && seen.why.includes("2FA") && seen.clock.includes("600") && seen.agent === "컴퓨터 사용" && seen.oldCard === false,
      JSON.stringify(seen),
    );
    ok(
      "it stands in the middle of the window, and the keyboard starts on the frame, not on either answer",
      Math.abs(seen.rect.cx - seen.rect.vw / 2) <= 2 && Math.abs(seen.rect.cy - seen.rect.vh / 2) <= seen.rect.vh * 0.1
        && seen.buttons.join() === "취소,다 했어요" && seen.focus === "(dialog)",
      JSON.stringify(seen),
    );
    // Enter says nothing: 「다 했어요」 is the person saying they did a thing on
    // their desktop, and a stray Enter must not say it for them.
    await page.keyboard.press("Enter");
    await page.waitForTimeout(120);
    const enterOnly = await page.evaluate(() => ({ answered: window.__ASK_T__.answered.slice(), shown: window.__ASK_T__.look().shown }));
    await page.keyboard.press("Tab");
    await page.keyboard.press("Tab");
    const onDone = await page.evaluate(() => window.__ASK_T__.look().focus);
    await page.keyboard.press("Enter");
    await page.waitForTimeout(90);
    // The window's own reason for a covered press comes as a key and its
    // arguments and is said in the language of the window.
    const covered = await page.evaluate(async () => {
      const T = window.__ASK_T__;
      T.fire("computer:handoff", { id: "handoff-2", reason: "", reasonKey: "computer.cover.ask", reasonArgs: { app: "Safari" }, timeoutMs: 600000 });
      await T.settle(260);
      return T.look();
    });
    await page.keyboard.press("Escape");
    await page.waitForTimeout(90);
    const answered = await page.evaluate(() => window.__ASK_T__.answered.slice());
    ok(
      "Enter alone answers nothing; Tab reaches 「다 했어요」 and Enter there says done; Escape hands the desk back unfinished",
      enterOnly.answered.length === 0 && enterOnly.shown === true && onDone === "다 했어요"
        && JSON.stringify(answered) === JSON.stringify([{ id: "handoff-1", allow: true }, { id: "handoff-2", allow: false }]),
      JSON.stringify({ enterOnly, onDone, answered }),
    );
    ok(
      "a covered press is said in the window's words, with the app that covers it",
      covered.why.includes("Safari") && covered.why.includes("다 했어요"),
      JSON.stringify(covered),
    );
    ok("the hand-over scenario raised no renderer errors", faults.length === 0, faults.join("\n"));
  });
}

async function confirm(browser, origin, ok) {
  await scenario(browser, origin, "window confirm", ok, async (page, faults) => {
    const seen = await page.evaluate(async () => {
      const T = window.__ASK_T__;
      window.__RESULT__ = "pending";
      void askConfirm({ title: "프로젝트를 옮길까요?", body: "에이전트 2개가 아직 일하고 있습니다.", confirm: "옮기기", deny: "취소", cancel: false })
        .then((answer) => { window.__RESULT__ = answer; });
      await T.settle();
      return T.look();
    });
    ok(
      "the window's own confirm is the same popup, in the middle, with the words its caller gave",
      seen.shown && seen.kind === "confirm" && seen.title === "프로젝트를 옮길까요?" && seen.mono === "에이전트 2개가 아직 일하고 있습니다."
        && seen.buttons.join() === "옮기기,취소" && seen.focus === "옮기기"
        && Math.abs(seen.rect.cx - seen.rect.vw / 2) <= 2 && Math.abs(seen.rect.cy - seen.rect.vh / 2) <= seen.rect.vh * 0.1,
      JSON.stringify(seen),
    );
    await page.keyboard.press("Escape");
    await page.waitForTimeout(260);
    const escaped = await page.evaluate(() => window.__RESULT__);
    ok("Escape closes a confirm as a no-answer, exactly as before", escaped === null, JSON.stringify(escaped));
    // And Enter is the confirm's own yes, as it always was: the person asked for this question.
    const entered = await page.evaluate(async () => {
      window.__RESULT__ = "pending";
      void askConfirm({ title: "저장할까요?", body: "설정", confirm: "저장", deny: "취소" }).then((answer) => { window.__RESULT__ = answer; });
      await window.__ASK_T__.settle();
      return window.__ASK_T__.look().focus;
    });
    await page.keyboard.press("Enter");
    await page.waitForTimeout(260);
    const yes = await page.evaluate(() => window.__RESULT__);
    ok("Enter on a confirm the person asked for is its yes", entered === "저장" && yes === true, JSON.stringify({ entered, yes }));

    // Behind a permission that is already on screen, the confirm waits its turn.
    const waited = await page.evaluate(async () => {
      const T = window.__ASK_T__;
      window.__RESULT__ = "pending";
      T.frame("s-a", { type: "permission_prompt", prompt_id: 31, tool_name: "bash", reasoning: "", audit_hint: null });
      await T.settle();
      void askConfirm({ title: "이 워크스페이스의 준비 스크립트를 실행할까요?", body: "npm ci", confirm: "실행", deny: "건너뛰기" })
        .then((answer) => { window.__RESULT__ = answer; });
      await T.settle();
      return { first: T.look(), pending: window.__RESULT__ };
    });
    ok(
      "a confirm asked while a permission stands waits behind it, and the count says so",
      waited.first.kind === "tool" && waited.first.more === "1개 더 대기 중" && waited.pending === "pending",
      JSON.stringify(waited),
    );
    await page.keyboard.press("Enter");
    await page.waitForTimeout(150);
    const next = await page.evaluate(() => ({ look: window.__ASK_T__.look(), sent: window.__ASK_T__.sent.slice() }));
    ok(
      "answering the permission brings up the confirm in the same popup, and the answer to the permission went first",
      next.look.kind === "confirm" && next.look.title === "이 워크스페이스의 준비 스크립트를 실행할까요?" && next.look.more === null
        && next.sent.length === 1 && next.sent[0].promptId === 31,
      JSON.stringify(next),
    );
    await page.locator("#ask-scrim").getByRole("button", { name: "실행", exact: true }).click();
    await page.waitForTimeout(260);
    const done = await page.evaluate(() => ({ result: window.__RESULT__, look: window.__ASK_T__.look() }));
    ok(
      "the confirm's caller gets its answer, and the popup goes down when nothing is left",
      done.result === true && done.look.shown === false,
      JSON.stringify(done),
    );
    ok("the confirm scenario raised no renderer errors", faults.length === 0, faults.join("\n"));
  });
}

/* ------------------------------------------------------------ one at a time */

async function queue(browser, origin, ok) {
  await scenario(browser, origin, "ask queue", ok, async (page, faults) => {
    const seen = await page.evaluate(async () => {
      const T = window.__ASK_T__;
      const trail = [];
      const note = () => { const one = T.look(); trail.push({ kind: one.kind, title: one.title, more: one.more }); return one; };
      // Four kinds at once, in the order they arrive.
      T.frame("s-a", { type: "permission_prompt", prompt_id: 41, tool_name: "bash", reasoning: "", audit_hint: null });
      T.frame("s-b", { type: "permission_prompt", prompt_id: 42, kind: "question", topic: "model_switch", tool_name: "", reasoning: "질문", audit_hint: null });
      T.fire("computer:confirm", { id: "confirm-9", kind: "payment", label: "Pay", timeoutMs: 120000 });
      T.fire("computer:handoff", { id: "handoff-9", reason: "코드를 입력해 주세요", timeoutMs: 600000 });
      await T.settle();
      const first = note();
      const visibleDialogs = [...document.querySelectorAll('[role="dialog"], [role="alertdialog"]')]
        .filter((node) => node.getClientRects().length > 0 && !node.closest("[hidden]")).length;
      const stackedAtOnce = visibleDialogs;
      // A backend that closes a waiting ask takes it out of the line without touching the one on screen.
      T.fire("computer:handoff-closed", { id: "handoff-9", decision: "timedout" });
      await T.settle();
      const afterClose = note();
      // Answer the first: the next kind takes its place, in arrival order.
      document.querySelector("#ask-choices button").click();
      await T.settle(150);
      const second = note();
      document.querySelector("#ask-choices button").click();
      await T.settle(150);
      const third = note();
      return { first, stackedAtOnce, afterClose, second, third, trail, sent: T.sent.slice() };
    });
    ok(
      "asks of every kind show one at a time, oldest first, and the count says how many wait",
      seen.first.kind === "tool" && seen.first.more === "3개 더 대기 중" && seen.stackedAtOnce === 1
        && seen.afterClose.more === "2개 더 대기 중" && seen.second.kind === "question" && seen.second.more === "1개 더 대기 중"
        && seen.third.kind === "computer-confirm" && seen.third.more === null,
      JSON.stringify(seen),
    );
    ok(
      "each answer goes back for its own prompt, in the order the asks were put",
      seen.sent.length === 2 && seen.sent[0].promptId === 41 && seen.sent[0].session === "s-a"
        && seen.sent[1].promptId === 42 && seen.sent[1].session === "s-b",
      JSON.stringify(seen.sent),
    );

    // A session that ends takes its waiting asks with it; a Computer Use ask
    // has no session and is not touched.
    const ended = await page.evaluate(async () => {
      const T = window.__ASK_T__;
      T.reset();
      T.frame("s-c", { type: "permission_prompt", prompt_id: 51, tool_name: "rm", reasoning: "", audit_hint: null });
      T.frame("s-d", { type: "permission_prompt", prompt_id: 52, tool_name: "git", reasoning: "", audit_hint: null });
      await T.settle();
      const before = T.look().more;
      T.ended("s-d");
      await T.settle();
      const withdrawn = T.look();
      return { before, withdrawn };
    });
    ok(
      "a session that ends leaves the line, and a Computer Use ask that was already up is not touched by it",
      ended.before === "2개 더 대기 중" && ended.withdrawn.kind === "computer-confirm" && ended.withdrawn.more === "1개 더 대기 중",
      JSON.stringify(ended),
    );

    // A key press that lands twice is one answer; a failed delivery keeps the ask.
    const flight = await page.evaluate(async () => {
      const T = window.__ASK_T__;
      T.reset();
      // Empty the line first: the computer confirm is answered, the remaining permission is next.
      document.querySelector("#ask-choices button:last-child").click();
      await T.settle(200);
      const now = T.look();
      let attempts = 0;
      window.__ANSWER__.respond_permission = async (args) => {
        attempts += 1;
        await new Promise((done) => setTimeout(done, 120));
        if (attempts === 1) throw new Error("channel gone");
        T.sent.push(JSON.parse(JSON.stringify(args)));
        return null;
      };
      const button = document.querySelector("#ask-choices button");
      button?.click();
      button?.click();
      await T.settle(30);
      const disabledInFlight = document.querySelector("#ask-choices button")?.disabled === true;
      await T.settle(200);
      const failed = T.look();
      const stillThere = failed.shown && failed.why?.includes("channel gone");
      document.querySelector("#ask-choices button")?.click();
      await T.settle(300);
      return { now, attempts, disabledInFlight, stillThere, sentAfterRetry: T.sent.length, shownAfterRetry: T.look().shown };
    });
    ok(
      "an answer in flight cannot be pressed twice, a failed delivery keeps the ask with its reason, and the retry goes through",
      flight.now.kind === "tool" && flight.attempts === 2 && flight.disabledInFlight && flight.stillThere
        && flight.sentAfterRetry === 1 && flight.shownAfterRetry === false,
      JSON.stringify(flight),
    );
    ok("the queue scenario raised no renderer errors", faults.length === 0, faults.join("\n"));
  });
}

/* ------------------------------------------------------ keyboard and focus */

async function keyboard(browser, origin, ok) {
  await scenario(browser, origin, "ask keyboard", ok, async (page, faults) => {
    await page.evaluate(async () => {
      window.__ASK_T__.frame("s-k", {
        type: "permission_prompt",
        prompt_id: 61,
        tool_name: "bash",
        reasoning: "",
        audit_hint: null,
        choices: [
          { key: "y", label: "한 번 허용", decision: "allow_once" },
          { key: "a", label: "항상 허용", decision: "allow_always" },
          { key: "n", label: "거부", decision: "deny" },
        ],
      });
      await window.__ASK_T__.settle();
    });
    const order = [];
    for (let step = 0; step < 4; step += 1) {
      order.push(await page.evaluate(() => window.__ASK_T__.look().focus));
      await page.keyboard.press("Tab");
    }
    ok(
      "Tab walks the popup's controls and wraps inside it",
      order.join() === "한 번 허용,항상 허용,거부,한 번 허용",
      JSON.stringify(order),
    );
    // Now on the second control: back to the first, and back once more wraps to the last.
    await page.keyboard.press("Shift+Tab");
    await page.keyboard.press("Shift+Tab");
    const backwards = await page.evaluate(() => window.__ASK_T__.look().focus);
    ok("Shift+Tab wraps backwards to the last control", backwards === "거부", JSON.stringify(backwards));
    // Two Tabs on: the focus rests on a control that is not the refusal.
    await page.keyboard.press("Tab");
    await page.keyboard.press("Tab");
    const resting = await page.evaluate(() => window.__ASK_T__.look().focus);
    await page.keyboard.press("Escape");
    await page.waitForTimeout(90);
    const escaped = await page.evaluate(() => window.__ASK_T__.sent.slice());
    ok(
      "Escape with the focus on a control that allows still answers the safe choice, refusing",
      resting === "항상 허용" && escaped.length === 1 && escaped[0].decision === "deny" && escaped[0].promptId === 61,
      JSON.stringify({ resting, escaped }),
    );
    // The focus goes back to where it was, and a click outside a permission is not an answer.
    const outside = await page.evaluate(async () => {
      const T = window.__ASK_T__;
      T.reset();
      T.frame("s-k", { type: "permission_prompt", prompt_id: 62, tool_name: "bash", reasoning: "", audit_hint: null });
      await T.settle();
      document.getElementById("ask-scrim").dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
      await T.settle(150);
      return { shown: T.look().shown, sent: T.sent.length };
    });
    ok(
      "a click on the backdrop answers nothing for an agent's ask — only the person's keys and buttons do",
      outside.shown === true && outside.sent === 0,
      JSON.stringify(outside),
    );
    ok("the keyboard scenario raised no renderer errors", faults.length === 0, faults.join("\n"));
  });
}

/* --------------------------------------------------- five languages and ink */

async function languages(browser, origin, ok) {
  await scenario(browser, origin, "ask languages", ok, async (page, faults) => {
    const titles = await page.evaluate(async (want) => {
      const T = window.__ASK_T__;
      const seen = {};
      for (const code of ["ko", "en", "ja", "zh", "es"]) {
        setLocale(code, { persist: false, refresh: false });
        seen[code] = {};
        T.frame(`s-${code}`, { type: "permission_prompt", prompt_id: 100, tool_name: "bash", reasoning: "", audit_hint: null });
        await T.settle(120);
        seen[code].tool = T.look().title;
        T.frame(`s-${code}`, { type: "prompt_resolved", prompt_id: 100, by: "pane" });
        await T.settle(260);
        T.frame(`s-${code}`, { type: "permission_prompt", prompt_id: 101, kind: "question", topic: "model_switch", reasoning: "?", audit_hint: null });
        await T.settle(120);
        seen[code].model_switch = T.look().title;
        // The language changes under an ask that is already up: it is redrawn, not consumed.
        const others = code === "ko" ? "en" : "ko";
        setLocale(others, { persist: false, refresh: false });
        seen[code].redrawn = T.look().title;
        setLocale(code, { persist: false, refresh: false });
        T.frame(`s-${code}`, { type: "prompt_resolved", prompt_id: 101, by: "pane" });
        await T.settle(260);
        T.frame(`s-${code}`, { type: "permission_prompt", prompt_id: 102, kind: "question", topic: "declined_images", reasoning: "?", audit_hint: null });
        await T.settle(120);
        seen[code].declined_images = T.look().title;
        T.frame(`s-${code}`, { type: "prompt_resolved", prompt_id: 102, by: "pane" });
        await T.settle(260);
        for (const kind of ["payment", "transfer", "delete"]) {
          T.fire("computer:confirm", { id: `c-${code}-${kind}`, kind, label: "x", timeoutMs: 120000 });
          await T.settle(120);
          seen[code][kind] = T.look().title;
          T.fire("computer:confirm-closed", { id: `c-${code}-${kind}`, decision: "refused" });
          await T.settle(260);
        }
        T.fire("computer:handoff", { id: `h-${code}`, reason: "x", timeoutMs: 600000 });
        await T.settle(120);
        seen[code].handoff = T.look().title;
        T.fire("computer:handoff-closed", { id: `h-${code}`, decision: "refused" });
        await T.settle(260);
      }
      setLocale("ko", { persist: false, refresh: false });
      return seen;
    }, ASK_TITLES);
    const wrong = [];
    for (const code of ["ko", "en", "ja", "zh", "es"]) {
      for (const kind of ["tool", "model_switch", "declined_images", "payment", "transfer", "delete", "handoff"]) {
        if (titles[code]?.[kind] !== ASK_TITLES[kind][code]) {
          wrong.push(`${code}/${kind}: ${JSON.stringify(titles[code]?.[kind])} != ${JSON.stringify(ASK_TITLES[kind][code])}`);
        }
      }
      const other = code === "ko" ? "en" : "ko";
      if (titles[code]?.redrawn !== ASK_TITLES.model_switch[other]) wrong.push(`${code}: an ask on screen did not follow a language change`);
    }
    ok("every kind of ask is titled in its own words in all five languages, and a language change redraws an ask already up", wrong.length === 0, wrong.join(" | "));
    ok("the ask languages scenario raised no renderer errors", faults.length === 0, faults.join("\n"));
  });
}

/* WCAG relative luminance and the ratio between two colours, so the ink of
 * each part is judged against what it is actually drawn on. */
const measureInk = () => {
  // A colour the engine derived by mixing comes back as `color(srgb 1 1 1 / 0.9)`,
  // in 0..1; the rest come back as `rgba(23, 23, 23, 0.9)`, in 0..255.
  const parse = (value) => {
    const [r, g, b, a = 1] = value.match(/-?[\d.]+/g).map(Number);
    const scale = value.startsWith("color(") ? 255 : 1;
    return { r: r * scale, g: g * scale, b: b * scale, a };
  };
  const over = (top, bottom) => {
    const a = top.a + bottom.a * (1 - top.a);
    const mix = (t, b) => (t * top.a + b * bottom.a * (1 - top.a)) / (a || 1);
    return { r: mix(top.r, bottom.r), g: mix(top.g, bottom.g), b: mix(top.b, bottom.b), a };
  };
  const light = (c) => {
    const lin = (v) => { const s = v / 255; return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4; };
    return 0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b);
  };
  const ratio = (a, b) => { const [hi, lo] = [light(a), light(b)].sort((x, y) => y - x); return (hi + 0.05) / (lo + 0.05); };
  const ground = (node) => {
    const chain = [];
    for (let at = node; at; at = at.parentElement) chain.push(getComputedStyle(at).backgroundColor);
    let colour = { r: 255, g: 255, b: 255, a: 1 };
    for (const value of chain.reverse()) {
      const one = parse(value);
      if (one.a > 0) colour = over(one, { ...colour, a: 1 });
    }
    return colour;
  };
  const rows = {};
  for (const id of ["ask-agent", "ask-title", "ask-body", "ask-why", "ask-clock", "ask-more"]) {
    const node = document.getElementById(id);
    if (!node || node.hidden || node.closest("[hidden]")) continue;
    const ink = parse(getComputedStyle(node).color);
    rows[id] = Math.round(ratio(over(ink, ground(node)), ground(node)) * 100) / 100;
  }
  document.querySelectorAll("#ask-scrim button").forEach((button, at) => {
    if (button.closest("[hidden]")) return;
    const ink = parse(getComputedStyle(button).color);
    const face = ground(button);
    rows[`button-${at}`] = Math.round(ratio(over(ink, face), face) * 100) / 100;
  });
  return rows;
};

async function ink(browser, origin, ok) {
  await scenario(browser, origin, "ask ink", ok, async (page, faults) => {
    const rows = {};
    for (const theme of ["dark", "light"]) {
      await page.evaluate((one) => { document.documentElement.dataset.theme = one; }, theme);
      for (const [name, raise] of [
        ["tool", () => window.__ASK_T__.frame("s-i", { type: "permission_prompt", prompt_id: 201, tool_name: "bash", reasoning: "명령을 실행합니다", audit_hint: "risk: medium" })],
        ["question", () => window.__ASK_T__.frame("s-i", { type: "permission_prompt", prompt_id: 202, kind: "question", topic: "model_switch", reasoning: "모델을 바꿔 다시 시도할까요?", audit_hint: null })],
        ["confirm", () => window.__ASK_T__.fire("computer:confirm", { id: "ink-c", kind: "payment", label: "Pay", timeoutMs: 120000 })],
        ["handoff", () => window.__ASK_T__.fire("computer:handoff", { id: "ink-h", reason: "코드를 입력해 주세요", timeoutMs: 600000 })],
      ]) {
        await page.evaluate(raise);
        await page.waitForTimeout(320);
        rows[`${theme}/${name}`] = await page.evaluate(measureInk);
        await page.keyboard.press("Escape");
        await page.waitForTimeout(260);
      }
    }
    const low = Object.entries(rows).flatMap(([where, parts]) =>
      Object.entries(parts).filter(([, value]) => value < 4.5).map(([part, value]) => `${where} ${part} ${value}`));
    const counted = Object.values(rows).reduce((sum, parts) => sum + Object.keys(parts).length, 0);
    ok(
      "every part of every kind of ask reads at 4.5:1 or better against what it is drawn on, in the dark and the light window",
      low.length === 0 && counted >= 24,
      JSON.stringify({ low, counted }),
    );
    ok("the ink scenario raised no renderer errors", faults.length === 0, faults.join("\n"));
  });
}

async function motion(browser, origin, ok) {
  await scenario(browser, origin, "ask motion", ok, async (page, faults) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    const seen = await page.evaluate(async () => {
      const T = window.__ASK_T__;
      T.frame("s-m", { type: "permission_prompt", prompt_id: 301, tool_name: "bash", reasoning: "", audit_hint: null });
      await T.settle(120);
      const section = document.querySelector("#ask-scrim .permission");
      const scrim = document.getElementById("ask-scrim");
      return {
        section: getComputedStyle(section).animationName,
        scrim: getComputedStyle(scrim).animationName,
        transition: getComputedStyle(document.querySelector("#ask-choices button")).transitionDuration,
      };
    });
    ok(
      "with reduced motion the popup arrives without its entrance",
      seen.section === "none" && seen.scrim === "none" && /^0s(, 0s)*$/.test(seen.transition),
      JSON.stringify(seen),
    );
    await page.emulateMedia({ reducedMotion: "no-preference" });
    ok("the ask motion scenario raised no renderer errors", faults.length === 0, faults.join("\n"));
  });
}

/* ---------------------------------------------- what is not an ask stays put */

async function status(browser, origin, ok) {
  await scenario(browser, origin, "status that asks nothing", ok, async (page, faults) => {
    const seen = await page.evaluate(async () => {
      const T = window.__ASK_T__;
      T.fire("computer:activity", { active: true, verb: "mouse-click", actions: 3, stopped: null });
      await T.settle();
      const band = document.getElementById("computer-band");
      const rect = band.getBoundingClientRect();
      return { bandShown: !band.hidden, bandTop: Math.round(rect.top), popup: T.look().shown };
    });
    ok(
      "the Computer Use band says it is working where it always was and raises no popup",
      seen.bandShown && seen.bandTop < 80 && seen.popup === false,
      JSON.stringify(seen),
    );
    ok("the status scenario raised no renderer errors", faults.length === 0, faults.join("\n"));
  });
}

/* -------------------------------------------------------------- who is asking */

async function askers(browser, origin, ok) {
  await scenario(browser, origin, "who asks", ok, async (page, faults) => {
    const seen = await page.evaluate(async () => {
      const T = window.__ASK_T__;
      // An agent this window has never heard of by name: the chip says what the
      // catalog calls it, because no name is written into the popup.
      agentRows.push({ id: "acme", name: "Acme Agent" });
      paneSessions.set(83, { agent: "acme", session: { key: "session_id", id: "s-acme" }, resumable: true });
      T.frame("s-acme", { type: "permission_prompt", prompt_id: 401, tool_name: "bash", reasoning: "", audit_hint: null });
      await T.settle();
      const named = T.look().agent;
      T.frame("s-acme", { type: "prompt_resolved", prompt_id: 401, by: "pane" });
      await T.settle(260);
      T.frame("s-nobody", { type: "permission_prompt", prompt_id: 402, tool_name: "bash", reasoning: "", audit_hint: null });
      await T.settle();
      return { named, unnamed: T.look().agent, short: shortSession("s-nobody"), shortAcme: shortSession("s-acme") };
    });
    ok(
      "the chip names the asker from the agent catalog, and says only the session when no agent is known",
      seen.named.startsWith("Acme Agent") && seen.named.includes(seen.shortAcme)
        && seen.unnamed === seen.short && !/\bZO\b/i.test(seen.unnamed),
      JSON.stringify(seen),
    );
    ok("the who-asks scenario raised no renderer errors", faults.length === 0, faults.join("\n"));
  });
}

/* One key, one answer. Two asks stand; the key that answers the first, held
 * down, repeats — and a repeat is not an answer to the second, which its person
 * has not read. */
async function oneKey(browser, origin, ok) {
  await scenario(browser, origin, "one key one answer", ok, async (page, faults) => {
    await page.evaluate(async () => {
      const T = window.__ASK_T__;
      T.frame("s-r", { type: "permission_prompt", prompt_id: 71, tool_name: "first-tool", reasoning: "", audit_hint: null });
      T.frame("s-r", { type: "permission_prompt", prompt_id: 72, tool_name: "second-tool", reasoning: "", audit_hint: null });
      await T.settle(300);
    });
    // Playwright marks a second `down` of a key already down as a repeat, the
    // way the keyboard does when a key is held.
    await page.keyboard.down("Enter");
    await page.waitForTimeout(80);
    await page.keyboard.down("Enter");
    await page.waitForTimeout(80);
    await page.keyboard.down("Enter");
    await page.waitForTimeout(80);
    await page.keyboard.up("Enter");
    const held = await page.evaluate(() => ({ look: window.__ASK_T__.look(), sent: window.__ASK_T__.sent.slice() }));
    ok(
      "Enter held down answers the first ask only: the repeats leave the second on screen with its own choices",
      held.sent.length === 1 && held.sent[0].promptId === 71 && held.look.mono === "second-tool" && held.look.shown && held.look.more === null
        && held.look.focus === "허용",
      JSON.stringify(held),
    );
    await page.keyboard.press("Enter");
    await page.waitForTimeout(90);
    const then = await page.evaluate(() => window.__ASK_T__.sent.slice());
    ok(
      "and a press of its own answers the second, so the repeats had not used it up",
      then.length === 2 && then[1].promptId === 72 && then[1].decision === "allow_once",
      JSON.stringify(then),
    );

    // The same for Escape, which refuses.
    await page.evaluate(async () => {
      const T = window.__ASK_T__;
      T.reset();
      await T.settle(300);
      T.frame("s-r", { type: "permission_prompt", prompt_id: 81, tool_name: "one", reasoning: "", audit_hint: null });
      T.frame("s-r", { type: "permission_prompt", prompt_id: 82, tool_name: "two", reasoning: "", audit_hint: null });
      await T.settle(300);
    });
    await page.keyboard.down("Escape");
    await page.waitForTimeout(80);
    await page.keyboard.down("Escape");
    await page.waitForTimeout(80);
    await page.keyboard.up("Escape");
    const escapes = await page.evaluate(() => ({ look: window.__ASK_T__.look(), sent: window.__ASK_T__.sent.slice() }));
    ok(
      "Escape held down refuses the first ask only",
      escapes.sent.length === 1 && escapes.sent[0].promptId === 81 && escapes.sent[0].decision === "deny"
        && escapes.look.mono === "two" && escapes.look.shown,
      JSON.stringify(escapes),
    );
    ok("the one-key scenario raised no renderer errors", faults.length === 0, faults.join("\n"));
  });
}

/* The id of a prompt counts per channel, so two sessions each on their first
 * prompt both say 1. They are two asks: the second is not the first again. */
async function identity(browser, origin, ok) {
  await scenario(browser, origin, "ask identity", ok, async (page, faults) => {
    const seen = await page.evaluate(async () => {
      const T = window.__ASK_T__;
      const raise = (session, tool) => T.frame(session, { type: "permission_prompt", prompt_id: 1, tool_name: tool, reasoning: "", audit_hint: null });
      raise("s-x", "from-x");
      raise("s-y", "from-y");
      // The same channel hydrating again names the same prompt: still one ask.
      raise("s-y", "from-y");
      await T.settle(300);
      const first = T.look();
      document.querySelector("#ask-choices button").click();
      await T.settle(300);
      const second = T.look();
      document.querySelector("#ask-choices button").click();
      await T.settle(300);
      return { first, second, sent: T.sent.slice(), last: T.look().shown };
    });
    ok(
      "two sessions that both say prompt 1 are two asks, each answered to its own session, and a repeat from the same session is not a third",
      seen.first.mono === "from-x" && seen.first.more === "1개 더 대기 중" && seen.second.mono === "from-y" && seen.second.more === null
        && seen.sent.length === 2 && seen.sent[0].session === "s-x" && seen.sent[1].session === "s-y" && seen.last === false,
      JSON.stringify(seen),
    );
    ok("the ask identity scenario raised no renderer errors", faults.length === 0, faults.join("\n"));
  });
}

export async function testAskPopup(browser, origin, ok) {
  await tool(browser, origin, ok);
  await question(browser, origin, ok);
  await computerConfirm(browser, origin, ok);
  await computerHandoff(browser, origin, ok);
  await confirm(browser, origin, ok);
  await queue(browser, origin, ok);
  await keyboard(browser, origin, ok);
  await oneKey(browser, origin, ok);
  await identity(browser, origin, ok);
  await languages(browser, origin, ok);
  await ink(browser, origin, ok);
  await motion(browser, origin, ok);
  await status(browser, origin, ok);
  await askers(browser, origin, ok);
}
