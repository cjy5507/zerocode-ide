/* 방의 깨우기 셈은 사람 손의 입력에만 되돌아간다 (review t-105587).
 *
 * 창의 `invoke`는 그 순간의 손을 `zerocode-hand` 머리글로 싣고, 백엔드는 그 손이
 * 사람일 때만 판의 셈을 되돌린다. 그런데 입력기가 확정한 한국어와 조합을 끝내는
 * Enter는 조합 사건에서 나가고, ⌘V 붙여넣기는 클립보드를 기다린 뒤에 나간다 —
 * 둘 다 그 순간에는 사람의 사건 밖이다. 그래서 터미널의 글 · 키는 그 입력을
 * 시작한 키의 손을, 붙여넣기는 시작할 때의 손을 싣는다.
 *
 * 사람의 키는 CDP로 보낸다 — 브라우저가 믿는(trusted) 키다. 조합 사건은 입력기가
 * 내는 모양 그대로 쏜다. 에이전트의 누름은 창 자신을 다루는 누름(`agentPressing`)
 * 안에서 시작한다. */
import { openWindowTestPage } from "./window-boot.mjs";

export async function testRoomHand(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    const cdp = await page.context().newCDPSession(page);
    await page.evaluate(async () => {
      window.__ROOM_HANDS__ = [];
      const record = (command) => (args, options) => {
        window.__ROOM_HANDS__.push({
          command,
          hand: options?.headers?.["zerocode-hand"] ?? null,
          what: args.text ?? args.press?.key ?? "",
        });
        return null;
      };
      window.__ANSWER__.term_text = record("term_text");
      window.__ANSWER__.term_key = record("term_key");
      window.__ANSWER__.term_paste = record("term_paste");
      await openTermTab();
      document.getElementById("key-sink").focus();
    });
    const settle = () => page.waitForTimeout(160);
    const hands = () => page.evaluate(() => window.__ROOM_HANDS__.splice(0));
    const compose = (text) => page.evaluate((data) => {
      const sink = document.getElementById("key-sink");
      sink.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true, data: "" }));
      sink.value = data;
      sink.dispatchEvent(new CompositionEvent("compositionupdate", { bubbles: true, data }));
      sink.dispatchEvent(new InputEvent("input", { isComposing: true, data, bubbles: true }));
    }, text);
    const commit = (text) => page.evaluate((data) => {
      const sink = document.getElementById("key-sink");
      sink.value = data;
      sink.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true, data }));
    }, text);

    // 사람이 「가」를 조합하고, 조합을 끝내는 Enter(입력기가 내는 229)를 친다.
    await compose("가");
    await cdp.send("Input.dispatchKeyEvent", {
      type: "rawKeyDown", key: "Enter", code: "Enter", windowsVirtualKeyCode: 229, nativeVirtualKeyCode: 229,
    });
    await cdp.send("Input.dispatchKeyEvent", { type: "keyUp", key: "Enter", code: "Enter", windowsVirtualKeyCode: 13 });
    await commit("가");
    await settle();
    const typed = await hands();
    ok(
      "the person's committed Korean and the Enter that committed it carry the person's hand",
      typed.some((one) => one.command === "term_text" && one.what === "가" && one.hand === "person")
        && typed.some((one) => one.command === "term_key" && one.what === "Enter" && one.hand === "person")
        && typed.every((one) => one.hand === "person"),
      JSON.stringify(typed),
    );

    // 사람의 ⌘V(또는 Ctrl+V): 클립보드를 기다린 뒤 나가는 붙여넣기.
    const chord = await page.evaluate(() => {
      window.__CLIPBOARD_TEXT__ = "붙여 넣은 글";
      return usesCommandModifier ? "Meta+V" : "Control+V";
    });
    await page.keyboard.press(chord);
    await settle();
    const pasted = await hands();
    ok(
      "the person's clipboard paste carries the person's hand past the clipboard's wait",
      pasted.some((one) => one.command === "term_paste" && one.what === "붙여 넣은 글" && one.hand === "person"),
      JSON.stringify(pasted),
    );

    // 에이전트의 누름에서 시작한 조합과 붙여넣기는 에이전트의 손이다.
    const agent = await page.evaluate(async () => {
      const sink = document.getElementById("key-sink");
      agentPressing = true;
      try {
        sink.dispatchEvent(new KeyboardEvent("keydown", { key: "Process", keyCode: 229, bubbles: true, cancelable: true }));
        sink.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true, data: "" }));
        sink.value = "나";
        sink.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true, data: "나" }));
        window.__CLIPBOARD_TEXT__ = "에이전트의 글";
        void pasteClipboardAt(keyboardTarget());
      } finally {
        agentPressing = false;
      }
      await new Promise((done) => setTimeout(done, 160));
      return window.__ROOM_HANDS__.splice(0);
    });
    ok(
      "a composition and a paste an agent's press began carry the agent's hand",
      agent.some((one) => one.command === "term_text" && one.hand === "agent")
        && agent.some((one) => one.command === "term_paste" && one.hand === "agent")
        && agent.every((one) => one.hand === "agent"),
      JSON.stringify(agent),
    );
    // 사람이 키를 친 뒤, keydown 없이 에이전트의 걸음이 터미널에 글을 넣는다: 마지막
    // 키의 손(사람)이 아니라 지금의 손(에이전트)이다.
    await cdp.send("Input.dispatchKeyEvent", { type: "rawKeyDown", key: "Shift", code: "ShiftLeft", windowsVirtualKeyCode: 16 });
    await cdp.send("Input.dispatchKeyEvent", { type: "keyUp", key: "Shift", code: "ShiftLeft", windowsVirtualKeyCode: 16 });
    await settle();
    await hands(); // the Shift's own key, the person's — not this scene's question
    const stale = await page.evaluate(async () => {
      agentPressing = true;
      try {
        routeText("에이전트가 넣은 글");
      } finally {
        agentPressing = false;
      }
      await new Promise((done) => setTimeout(done, 80));
      return window.__ROOM_HANDS__.splice(0);
    });
    ok(
      "an agent's step that types with no keydown of its own carries the agent's hand, not the last key's",
      stale.length > 0 && stale.every((one) => one.command === "term_text" && one.hand === "agent"),
      JSON.stringify(stale),
    );
    ok("the scenes raised no renderer errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}
