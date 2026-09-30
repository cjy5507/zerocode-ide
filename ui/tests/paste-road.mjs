import { pathToFileURL } from "node:url";
import { resolve } from "node:path";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";

/* A paste never stops the window's main thread (t-19409).
 *
 * WebKit serves `clipboardData` reads as synchronous messages on the UI
 * process's main thread, so a slow pasteboard owner held the whole window for
 * 2030 ms at 23:48 on 2026-09-30. The window's paste road therefore never
 * reads `clipboardData` (the spy below counts every touch), waits for the
 * clipboard in the background, and lands the text in the place the paste
 * was aimed at. The fake clipboard's hold (`__CLIPBOARD_READ_HOLD__`) is the
 * slow owner. */
export async function testPasteRoad(browser, origin, ok) {
  const { page } = await openWindowTestPage(browser, origin);
  try {
    /* 클립보드의 이미지(1-g64), t-19409 뒤: paste 이벤트는 더 이상 `clipboardData`를
     * 읽지 않는다 — 웹킷이 그 읽기를 UI 프로세스의 주 스레드에서 동기로 받아, 판의
     * 주인이 느리면 창 전체가 선다(2026-09-30 23:48, 2030 ms). 이벤트는 판을 붙잡는
     * 신호일 뿐이고, 글자는 배경 스레드의 clipboardManager로, 그림은 백엔드
     * `save_clipboard_image`(배경 스레드, 임시 파일)로 온다. 글자가 있으면 글자가
     * 이기고, 터미널에는 경로가 기존 paste 문으로 붙는다. */
    const imagePaste = await page.evaluate(async () => {
      const seen = {};
      const term = await openTermTab();
      const saved = [];
      const asked = [];
      const pastes = [];
      window.__ANSWER__.save_pasted_image = (args) => (saved.push(args), "/tmp/zerocode-paste/never.png");
      window.__ANSWER__.save_clipboard_image = () =>
        (asked.push(1), "/tmp/zerocode-paste/paste-1755300000000.png");
      window.__ANSWER__.term_paste = (args) => (pastes.push({ ...args }), null);
      const sink = document.getElementById("key-sink");
      const touched = [];
      const spy = {
        getData: () => (touched.push("getData"), "echo never-read"),
        get items() { touched.push("items"); return []; },
        get files() { touched.push("files"); return []; },
        get types() { touched.push("types"); return []; },
      };
      const paste = () => {
        const event = new ClipboardEvent("paste", { bubbles: true, cancelable: true });
        Object.defineProperty(event, "clipboardData", { value: spy });
        sink.dispatchEvent(event);
        return event;
      };
      // 글자 없는(그림만 든) 클립보드: 글자 길이 빈손이면 그림 문이 열린다.
      window.__CLIPBOARD_TEXT__ = "";
      const pictureEvent = paste();
      await new Promise((done) => setTimeout(done, 160));
      seen.imageLands =
        pictureEvent.defaultPrevented && touched.length === 0 && saved.length === 0 && asked.length === 1 &&
        pastes.at(-1)?.text === "/tmp/zerocode-paste/paste-1755300000000.png" &&
        pastes.at(-1)?.term === term;
      // 글자가 있는 클립보드는 글자의 길 그대로 — 그림 문은 열리지 않는다.
      window.__CLIPBOARD_TEXT__ = "echo hello";
      paste();
      await new Promise((done) => setTimeout(done, 120));
      seen.textStillWins = asked.length === 1 && touched.length === 0 && pastes.at(-1)?.text === "echo hello";
      delete window.__ANSWER__.save_pasted_image;
      delete window.__ANSWER__.save_clipboard_image;
      delete window.__ANSWER__.term_paste;
      window.__CLIPBOARD_TEXT__ = "";
      closeTab(activeTabId);
      await new Promise((done) => setTimeout(done, 120));
      return seen;
    });
    ok(
      "a paste event never reads clipboardData: a picture-only clipboard lands as a temp-file path from the backend, and words win",
      imagePaste.imageLands && imagePaste.textStillWins,
      JSON.stringify(imagePaste),
    );

    /* t-19409: 판의 주인이 느려도(Universal Clipboard·남의 앱의 큰 그림) 붙여넣기는
     * 창의 주 스레드를 세우지 않는다 — 읽기는 붙여넣기가 시작된 때의 판을 붙잡은 채
     * 배경에서 기다리고, 그동안 초점이 다른 판으로 옮겨도 글자는 붙잡은 판에
     * 도착한다. 대기 중의 두 번째 붙여넣기는 두 번 붙이지 않고, 잠시 뒤 카탈로그의
     * 말로 알린다. 일반 입력칸은 기본 동작 대신 같은 길로 받고 ⌘Z가 붙여넣기를
     * 되돌린다. 조합 중에는 끼어들지 않는다. */
    const slowPaste = await page.evaluate(async () => {
      const seen = {};
      const settle = (ms) => new Promise((done) => setTimeout(done, ms));
      const term = await openTermTab();
      const pastes = [];
      window.__ANSWER__.term_paste = (args) => (pastes.push({ ...args }), null);
      const touched = [];
      const spy = {
        getData: () => (touched.push("getData"), "never read"),
        get items() { touched.push("items"); return []; },
        get files() { touched.push("files"); return []; },
        get types() { touched.push("types"); return []; },
      };
      const fire = (node) => {
        const event = new ClipboardEvent("paste", { bubbles: true, cancelable: true });
        Object.defineProperty(event, "clipboardData", { value: spy });
        node.dispatchEvent(event);
        return event;
      };
      const sink = document.getElementById("key-sink");
      const readsBefore = window.__CLIPBOARD_READS__;
      window.__CLIPBOARD_TEXT__ = "slow words";
      window.__CLIPBOARD_READ_HOLD__ = true;
      const first = fire(sink);
      seen.prevented = first.defaultPrevented;
      // 읽는 동안 새 터미널이 초점을 가져가고, 그 위에 두 번째 붙여넣기가 온다.
      const other = await openTermTab();
      const second = fire(sink);
      seen.secondPrevented = second.defaultPrevented;
      await settle(900);
      const slowWords = t("clipboard.pasteSlow", "");
      seen.slowNotice = slowWords !== "" &&
        [...document.querySelectorAll(".toast")].some((node) => node.textContent.includes(slowWords));
      window.__CLIPBOARD_READ_HOLD__ = false;
      window.__CLIPBOARD_RELEASE__?.();
      await settle(80);
      seen.readOnce = window.__CLIPBOARD_READS__ - readsBefore === 1;
      seen.untouched = touched.length === 0;
      seen.landsOnce = pastes.length === 1 && pastes[0].text === "slow words" && pastes[0].term === term &&
        pastes[0].term !== other;
      // 끝난 붙여넣기 뒤에는 다음 붙여넣기가 다시 받아진다.
      window.__CLIPBOARD_TEXT__ = "again";
      fire(sink);
      await settle(80);
      seen.nextAllowed = pastes.length === 2 && pastes[1].text === "again";

      // 일반 입력칸: 기본 동작(웹킷의 동기 읽기) 대신 같은 길 — 붙잡은 칸에 도착하고 ⌘Z가 되돌린다.
      const field = document.createElement("textarea");
      field.value = "keep ";
      document.body.append(field);
      field.focus();
      field.setSelectionRange(field.value.length, field.value.length);
      window.__CLIPBOARD_TEXT__ = "pasted";
      window.__CLIPBOARD_READ_HOLD__ = true;
      const fieldEvent = fire(field);
      seen.fieldPrevented = fieldEvent.defaultPrevented;
      seen.fieldWaits = field.value === "keep ";
      window.__CLIPBOARD_READ_HOLD__ = false;
      window.__CLIPBOARD_RELEASE__?.();
      await settle(80);
      seen.fieldLands = field.value === "keep pasted";
      document.execCommand("undo");
      seen.fieldUndo = field.value === "keep ";
      // 조합 중의 붙여넣기는 창이 가로채지 않는다 — 입력기가 제 일을 마치게 둔다.
      field.focus();
      field.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true }));
      const composing = fire(field);
      seen.composingLeftAlone = !composing.defaultPrevented;
      field.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true }));
      field.remove();

      // 상한(30 s)을 넘겨 온 답은 버린다 — 사람은 이미 포기했거나 다시 붙였을 수 있다.
      // 시계를 31 s 앞으로 감아 그 사이에 답한 것으로 친다; 상한 뒤의 새 붙여넣기는 된다.
      const realNow = performance.now.bind(performance);
      let skew = 0;
      performance.now = () => realNow() + skew;
      try {
        const before = pastes.length;
        window.__CLIPBOARD_TEXT__ = "too late";
        window.__CLIPBOARD_READ_HOLD__ = true;
        fire(sink);
        await settle(20);
        skew += 31_000;
        window.__CLIPBOARD_READ_HOLD__ = false;
        window.__CLIPBOARD_RELEASE__?.();
        await settle(80);
        seen.lateDropped = pastes.length === before;
        window.__CLIPBOARD_TEXT__ = "fresh";
        fire(sink);
        await settle(80);
        seen.afterCeilingTaken = pastes.length === before + 1 && pastes.at(-1)?.text === "fresh";
      } finally {
        performance.now = realNow;
      }

      window.__CLIPBOARD_TEXT__ = "";
      delete window.__ANSWER__.term_paste;
      closeTab(activeTabId);
      closeTab(activeTabId);
      await settle(120);
      return seen;
    });
    ok(
      "a slow clipboard never holds the window: the paste waits in the background, lands in the pane that had the focus, pastes once, says so after a moment, drops an answer that comes past the ceiling, and the field road undoes",
      slowPaste.prevented && slowPaste.secondPrevented && slowPaste.slowNotice && slowPaste.readOnce &&
        slowPaste.untouched && slowPaste.landsOnce && slowPaste.nextAllowed && slowPaste.fieldPrevented &&
        slowPaste.fieldWaits && slowPaste.fieldLands && slowPaste.fieldUndo && slowPaste.composingLeftAlone &&
        slowPaste.lateDropped && slowPaste.afterCeilingTaken,
      JSON.stringify(slowPaste),
    );

  } finally {
    await page.close();
  }
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  let failures = 0;
  try {
    await testPasteRoad(browser, origin, (name, pass, detail = "") => {
      console.log(`${pass ? "PASS" : "FAIL"} ${name}${!pass && detail ? `\n${detail}` : ""}`);
      if (!pass) failures++;
    });
  } finally { await browser.close(); files.close(); }
  if (failures) process.exitCode = 1;
}
