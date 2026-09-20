/* The pane's own conversation — the `>_ 터미널 · ◐ 대화` toggle.
 *
 * This is the surface a person reaches for on an agent that has no wire (zo,
 * Codex, anything the voice table gives `wire: None`): the transcript view.
 * The wire's page streams what is being said; this one gets a turn when the
 * vendor writes it, and stands it whole the moment it arrives — the terminal
 * already said those words, and releasing them again a word at a time (the
 * 09-16 reveal) only lagged behind it (09-20). These pin the box the person's
 * words wear and that an answer is on screen by the next frame, once. */
import { pathToFileURL } from "node:url";
import { resolve } from "node:path";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";

const OPENING = [
  { role: "user", text: "이 화면 버그 좀 봐줘." },
  { role: "assistant", text: "확인했습니다." },
];
const ARRIVING =
  "원인은 스트리밍 경로가 와이어에만 붙어 있는 것입니다 그래서 전사 뷰는 턴이 닫힐 때까지 기다렸다가 한 번에 그렸습니다";

/* A pane running an agent with no wire, its conversation view open. */
async function openPaneChat(page) {
  return page.evaluate(async (turns) => {
    const term = await openTermTab({ placement: "tab" });
    paneAgents.set(term, "zo");
    hookStates.set(term, "working");
    window.__TERM__ = term;
    window.__ANSWER__.pane_log = () => ({
      found: true, next: 1, turns, skipped: false, more: false, folded: false,
      model: "claude-opus-5",
    });
    await setPaneChat(term, true);
    // From here the transcript answers out of a box the test fills, on the
    // same road the real one travels: the poll.
    window.__LOG__ = { next: 1, turns: [] };
    window.__ANSWER__.pane_log = () => ({
      found: true, next: window.__LOG__.next, turns: window.__LOG__.turns,
      skipped: false, more: false, folded: false, model: "claude-opus-5",
    });
    window.__CHAT__ = paneChatOf(term).tab;
    return wireRowFor(term)?.id ?? null;
  }, turns_(OPENING));
}

function turns_(rows) {
  return rows;
}

export async function testPaneConversation(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    const wire = await openPaneChat(page);
    ok("a zo pane has no wire, so its conversation view is the transcript one",
      wire === null, `wireRowFor returned ${wire}`);

    await page.waitForSelector(".helper-turn.is-user");

    // ---------------------------------------------------------- right aligned
    const placed = await page.evaluate(() => {
      const list = document.querySelector(".helper-turns");
      const user = document.querySelector(".helper-turn.is-user");
      const answer = document.querySelector(".helper-turn.is-assistant");
      const listBox = list.getBoundingClientRect();
      const userBox = user.getBoundingClientRect();
      const answerBox = answer.getBoundingClientRect();
      return {
        alignSelf: getComputedStyle(user).alignSelf,
        userLeftGap: Math.round(userBox.left - listBox.left),
        userRightGap: Math.round(listBox.right - userBox.right),
        answerLeftGap: Math.round(answerBox.left - listBox.left),
      };
    });
    ok("what the person said is the extension's box on the left, sized to its words, and what the agent said stands on the rail beside it",
      placed.alignSelf === "flex-start" &&
      placed.userLeftGap < placed.userRightGap &&
      placed.answerLeftGap <= placed.userLeftGap,
      JSON.stringify(placed));

    // ------------------------------------------------------- whole, by the next frame
    const arrival = await page.evaluate(async (text) => {
      const list = document.querySelector(".helper-turns");
      window.__LOG__ = { next: 2, turns: [{ role: "assistant", text }] };
      const t0 = performance.now();
      await pollHelperPages();
      window.__LOG__ = { next: 2, turns: [] };
      await new Promise((r) => requestAnimationFrame(r));
      const ms = +(performance.now() - t0).toFixed(1);
      const said = list.innerText;
      return {
        ms,
        shown: said.includes(text),
        rows: list.querySelectorAll("[data-turn]").length,
        held: window.__CHAT__.worker.helper.turns.length,
        once: said.split(text.slice(0, 20)).length - 1,
        streamingRows: list.querySelectorAll(".is-streaming").length,
      };
    }, ARRIVING);
    ok("an answer that arrived whole stands whole by the next frame — the terminal already said it — once, with no streaming row left behind",
      arrival.shown && arrival.rows === arrival.held && arrival.once === 1 && arrival.streamingRows === 0,
      JSON.stringify(arrival));

    // A pane nobody has sent a model-bearing hook for still knows what it is
    // on, because its own transcript says so. The chip itself is this exact
    // read (`paneModels.get(run.term)`, ui/shell-composer.js) — it is not
    // drawn here because the stubbed agent catalog has no zo to draw it for.
    const chip = await page.evaluate(() => ({
      model: paneModels.get(window.__TERM__) ?? null,
    }));
    ok("a pane learns its model from the transcript when no hook has carried one",
      chip.model === "claude-opus-5", JSON.stringify(chip));

    await page.screenshot({ path: "output/playwright/pane-conversation/chat.png" });
    ok("the conversation raised no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  let failed = 0;
  try {
    await testPaneConversation(browser, origin, (name, pass, details = "") => {
      console.log(`${pass ? "PASS" : "FAIL"} ${name}${!pass ? `\n  ${details}` : ""}`);
      if (!pass) failed++;
    });
  } finally {
    await browser.close();
    files.close();
  }
  if (failed) process.exitCode = 1;
}
