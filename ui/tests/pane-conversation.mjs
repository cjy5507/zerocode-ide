/* The pane's own conversation — the `>_ 터미널 · ◐ 대화` toggle.
 *
 * This is the surface a person reaches for on an agent that has no wire (zo,
 * Codex, anything the voice table gives `wire: None`): the transcript view.
 * The wire's page streams; this one used to wait for a turn to close and then
 * put the whole answer on screen at once, because every streaming road was
 * gated behind `run.wire`. These pin the two things that made it read as a
 * terminal rather than as a chat panel. */
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
    ok("what the person said sits on the right, and what the agent said stays on the left",
      placed.alignSelf === "flex-end" &&
      placed.userRightGap < placed.userLeftGap &&
      placed.answerLeftGap <= placed.userLeftGap,
      JSON.stringify(placed));

    // ------------------------------------------------------ revealed, not dumped
    const reveal = await page.evaluate(async (text) => {
      const list = document.querySelector(".helper-turns");
      // `innerText`, not `textContent`: the turn's own row is in the DOM the
      // whole time, hidden behind the reveal, and `textContent` would read it
      // and call the lump a pass.
      const at = () => list.innerText;
      window.__LOG__ = { next: 2, turns: [{ role: "assistant", text }] };
      await pollHelperPages();
      window.__LOG__ = { next: 2, turns: [] };
      const held = window.__CHAT__.worker.helper.turns.length;
      // Two frames: enough for the reveal to have started, far too few for a
      // hundred-odd characters at one word per 60 ms.
      await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));
      const early = at();
      const rows = list.querySelectorAll("[data-turn]").length;
      await new Promise((r) => setTimeout(r, 4000));
      return {
        held,
        rows,
        earlyHasWhole: early.includes(text),
        earlyTail: early.slice(-40),
        lateHasWhole: at().includes(text),
        doubled: at().split(text.slice(0, 20)).length - 1,
      };
    }, ARRIVING);
    ok("an answer that arrived whole is released word by word, not put up in one lump",
      !reveal.earlyHasWhole && reveal.rows === reveal.held, JSON.stringify(reveal));
    ok("and the whole answer is there when the reveal drains, exactly once",
      reveal.lateHasWhole && reveal.doubled === 1, JSON.stringify(reveal));

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
