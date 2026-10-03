import { installBoardWaits } from "./board-waits.mjs";
import { openWindowTestPage } from "./window-boot.mjs";

/* The answer door, as the board's page meets it (t-26594).
 *
 * A card is answered by typing into the pane its agent stopped in, and the
 * backend types only if the question is still the one on that pane's screen
 * (`answer_door`). What this page owes is what a person sees around that:
 *
 *  - a numbered menu another CLI only signalled arrives as the card the
 *    backend drew off its screen, and its choice travels under that CLI's own
 *    name;
 *  - an answer the backend refuses because the question changed says so in
 *    the person's words and repaints from the pane as it is now — which is how
 *    the question that replaced the old one reaches them — for a permission's
 *    Allow and Deny as for a question's rows;
 *  - a question read off a screen can run over several rows and is shown as
 *    the rows it is, where an agent's one-line question stays one line;
 *  - a wait the hook only signalled is asked about again as the program's
 *    screen settles, three times and no more.
 *
 *   WINDOW_SUITES=answer-door node ui/tests/window.mjs */

/* The words, in the five languages the window speaks, pinned here so a catalog
 * that drifted into another language, or into a key nobody translated, fails
 * instead of agreeing with itself. */
export const REFUSAL_WORDS = Object.freeze({
  ko: "화면에서 이 질문을 찾지 못해 답을 보내지 않았어요 — 터미널을 확인해 주세요",
  en: "That question is no longer on the pane's screen, so nothing was sent — check the terminal",
  ja: "その質問は画面で見つからないため回答は送られませんでした — ターミナルを確認してください",
  zh: "在屏幕上找不到该问题，未发送回答 — 请查看终端",
  es: "Esa pregunta ya no está en la pantalla del panel, así que no se envió nada — revisa el terminal",
});

/* One attention card the way the board's grouping returns it. */
const installCards = () => {
  window.__asked = (question, labels, agent = "kimi") => [{ bucket: "attention", cards: [
    { pane: "term:6", agent, state: "needs-attention", heading: "메뉴 대기",
      worktree: "main", project: "/p", task: "", ask: question,
      ask_prompt: { questions: [{ question, multi_select: false,
        options: labels.map((label) => ({ label })) }] },
      unseen: false, changed_at: 1, at: 1 }] }];
  window.__permission = (agent = "kimi") => [{ bucket: "attention", cards: [
    { pane: "term:6", agent, state: "needs-attention", heading: "권한 대기",
      worktree: "main", project: "/p", task: "", ask: "Bash · make build",
      approval: { tool: "Bash", summary: "make build" },
      unseen: false, changed_at: 1, at: 1 }] }];
  window.__board = () => [...document.querySelectorAll("#board-view, .file-view")]
    .find((one) => one.querySelector(".agent-graph-layout") && !one.hidden);
  window.__card = () => window.__board().querySelector(".board-card");
  window.__open = async (panel) => {
    await paintBoardView();
    await window.__BOARD_SETTLED__();
    if (!window.__card().querySelector(panel)) window.__card().querySelector(".board-card-ask").click();
    await window.__BOARD_SETTLED__();
  };
};

export async function testAnswerDoor(browser, origin, ok) {
  const { page } = await openWindowTestPage(browser, origin);
  try {
    await installBoardWaits(page);
    await page.evaluate(installCards);
    await page.evaluate(async () => {
      if (typeof agentBoardMode !== "undefined") agentBoardMode = "graph";
      if (typeof setAgentOrbitChoice === "function") setAgentOrbitChoice(document.getElementById("board-view"), "cards");
      window.__COLUMNS__ = window.__asked("Select an approach", ["Rebase", "Merge", "Cancel"]);
      openBoard();
      await window.__BOARD_SETTLED__();
    });

    /* A numbered menu another CLI only signalled, then a refused answer. */
    const menuCard = await page.evaluate(async () => {
      const settle = () => window.__BOARD_SETTLED__();
      const seen = {};
      window.__ANSWER_REFUSES__ = null;
      window.__ANSWERED__ = null;
      window.__COLUMNS__ = window.__asked("Select an approach", ["Rebase", "Merge", "Cancel"]);
      await window.__open(".board-ask");
      seen.labels = [...window.__card().querySelectorAll(".board-ask-option-label")]
        .map((one) => one.textContent);
      window.__card().querySelectorAll(".board-ask-option")[1].click();
      await settle();
      window.__card().querySelector(".board-ask-send").click();
      await settle();
      seen.sent = window.__ANSWERED__;
      // Another question on the same pane, and the pane moves on from it
      // before the next press.
      window.__ANSWER_REFUSES__ = "question-changed";
      window.__ANSWERED__ = null;
      window.__COLUMNS__ = window.__asked("Pick a style", ["Squash", "Rebase", "Merge"]);
      await window.__open(".board-ask");
      window.__card().querySelectorAll(".board-ask-option")[2].click();
      await settle();
      for (const note of document.querySelectorAll(".toast")) note.remove();
      window.__COLUMNS__ = window.__asked("Which branch?", ["main", "dev"]);
      window.__card().querySelector(".board-ask-send").click();
      await settle();
      seen.refusedSent = window.__ANSWERED__;
      seen.toast = [...document.querySelectorAll(".toast")].map((node) => node.textContent).join(" ");
      seen.nowAsks = window.__card().querySelector(".board-ask-question")?.textContent ?? "";
      seen.rowsLive = window.__card().querySelector(".board-ask-option")?.disabled === false;
      window.__ANSWER_REFUSES__ = null;
      return seen;
    });
    ok(
      "a numbered menu another CLI only signalled is answered from its card, and a refused answer says so and shows the pane's question now",
      menuCard.labels.join(",") === "Rebase,Merge,Cancel" &&
        menuCard.sent?.term === 6 &&
        menuCard.sent?.agent === "kimi" &&
        menuCard.sent?.prompt?.questions?.[0]?.question === "Select an approach" &&
        menuCard.sent?.selections?.[0]?.indices?.join(",") === "1" &&
        menuCard.refusedSent?.prompt?.questions?.[0]?.question === "Pick a style" &&
        menuCard.refusedSent?.selections?.[0]?.indices?.join(",") === "2" &&
        menuCard.toast.includes(REFUSAL_WORDS.ko) &&
        menuCard.nowAsks === "Which branch?" &&
        menuCard.rowsLive,
      JSON.stringify(menuCard),
    );

    /* A permission refused for the same reason: Allow and Deny say so too, and
     * the panel is usable again for the permission that replaced it. */
    const permission = await page.evaluate(async () => {
      const settle = () => window.__BOARD_SETTLED__();
      const seen = {};
      window.__APPROVED__ = null;
      window.__ANSWER_REFUSES__ = "question-changed";
      window.__COLUMNS__ = window.__permission();
      await window.__open(".board-approve");
      for (const note of document.querySelectorAll(".toast")) note.remove();
      window.__card().querySelector(".board-approve .is-allow").click();
      await settle();
      seen.sent = window.__APPROVED__;
      seen.toast = [...document.querySelectorAll(".toast")].map((node) => node.textContent).join(" ");
      window.__ANSWER_REFUSES__ = null;
      for (const note of document.querySelectorAll(".toast")) note.remove();
      return seen;
    });
    ok(
      "a permission answer the backend refused says the question changed",
      permission.sent?.term === 6 &&
        permission.sent?.allow === true &&
        permission.toast.includes(REFUSAL_WORDS.ko),
      JSON.stringify(permission),
    );

    /* The other two words the backend refuses with, and anything else as it is. */
    const words = await page.evaluate(() => (typeof answerRefusalWords !== "function" ? {} : {
      inFlight: answerRefusalWords("Error: answer-in-flight"),
      needsRow: answerRefusalWords("menu-needs-a-row"),
      other: answerRefusalWords("터미널이 떠 있지 않습니다"),
      changed: answerRefusalWords(new Error("question-changed")),
    }));
    ok(
      "a refusal's word becomes the window's own sentence and anything else is left as it was said",
      words.inFlight === "앞서 누른 답을 보내는 중이에요" &&
        words.needsRow === "이 메뉴는 목록에서 고른 답만 받아요" &&
        words.other === "터미널이 떠 있지 않습니다" &&
        words.changed === REFUSAL_WORDS.ko,
      JSON.stringify(words),
    );

    /* A question read off a screen runs over several rows; an agent's own is one line. */
    const rows = await page.evaluate(async () => {
      const seen = {};
      window.__COLUMNS__ = window.__asked("make build\nBuild the project\nDo you want to proceed?", ["Yes", "No"]);
      await window.__open(".board-ask");
      const title = window.__card().querySelector(".board-ask-question");
      seen.multiline = title.classList.contains("is-multiline");
      seen.wrap = getComputedStyle(title).whiteSpace;
      window.__COLUMNS__ = window.__asked("Which way?", ["Left", "Right"]);
      await window.__open(".board-ask");
      const one = window.__card().querySelector(".board-ask-question");
      seen.oneLine = !one.classList.contains("is-multiline") && getComputedStyle(one).whiteSpace === "nowrap";
      return seen;
    });
    ok(
      "a question read off a screen is shown as its rows, and an agent's one-line question stays one line",
      rows.multiline && rows.wrap === "pre-line" && rows.oneLine,
      JSON.stringify(rows),
    );

    /* A signalled wait is asked about again as the screen settles. */
    const watch = await page.evaluate(async () => {
      const seen = {};
      if (typeof watchSignalledWait !== "function") return seen;
      const before = window.__COUNTS__.pane_agents ?? 0;
      watchSignalledWait(6);
      seen.armed = signalledWaitTimers.get(6)?.length ?? 0;
      watchSignalledWait(6);
      seen.armedAgain = signalledWaitTimers.get(6)?.length ?? 0;
      // Longer than the last of the three asks: all of them have fired by then.
      await new Promise((done) => setTimeout(done, Math.max(...SIGNALLED_WAIT_REPAINT_MS) + 300));
      seen.asked = (window.__COUNTS__.pane_agents ?? 0) - before;
      // A spent watch leaves nothing behind in the map.
      seen.selfCleaned = !signalledWaitTimers.has(6);
      watchSignalledWait(6);
      clearSignalledWait(6);
      seen.cleared = !signalledWaitTimers.has(6);
      seen.schedule = [...SIGNALLED_WAIT_REPAINT_MS];
      return seen;
    });
    ok(
      "a wait the hook only signalled is asked about again as the screen settles — three times, never stacked",
      watch.armed === 3 &&
        watch.armedAgain === 3 &&
        watch.asked >= 3 &&
        watch.selfCleaned &&
        watch.cleared &&
        watch.schedule.length === 3,
      JSON.stringify(watch),
    );
  } finally { await page.close(); }
}

if (process.argv[1] && import.meta.url === (await import("node:url")).pathToFileURL(process.argv[1]).href) {
  const { chromium, createWindowServer } = await import("./window-boot.mjs");
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch();
  try {
    await testAnswerDoor(browser, origin, (name, pass, detail) => {
      console.log(`${pass ? "PASS" : "FAIL"} ${name} ${detail}`);
      if (!pass) process.exitCode = 1;
    });
  } finally { await browser.close(); files.close(); }
}
