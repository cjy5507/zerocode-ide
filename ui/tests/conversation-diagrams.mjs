/* A ```mermaid fence in the conversation's prose draws as a diagram (t-58437), on run-4275's rule for
 * a view that repaints on every streamed frame: a fence draws once it has settled.
 *   1. An answer streamed over 30 frames asks `render_mermaid` once. The block still being written is
 *      repainted each frame and keeps its fence as code — open, or closed with no blank line after it
 *      yet — and the settled part, drawn once (`settledCut`), holds the diagram.
 *   2. The answer's turn, standing where its stream was, draws the diagram from the layout already
 *      made: in the paint that stands it, with no second request.
 *   3. A list that follows its foot keeps to it when the diagram lands, though the drawing stands far
 *      taller than the code it replaces: the list turns the browser's own anchoring off.
 *   4. A reader who left the foot keeps the row they read where it stood when a diagram lands in a row
 *      above the view, and again when that row, sent back to the shelf, is dressed again — which draws
 *      it from the layout already made, with no request.
 *   5. A label holding `<img src=x onerror=...>` arrives as text.
 *
 * Synthetic conversations, with `render_mermaid` answered by the page (`standDiagrams`): the drawings
 * named `높은-…` stand 420 px tall, far past the list's 50 px of slack at its foot.
 *
 *   WINDOW_SUITES='^conversation-diagrams$' node ui/tests/window.mjs */
import { openWindowTestPage } from "./window-boot.mjs";
import { openConversation } from "./conversation-parity.mjs";
import { standDiagrams } from "./markdown-diagrams.mjs";

/* An answer long enough to take most of a screen, so a few dozen of them make a list that scrolls. */
function longAnswer(at) {
  return [
    `### 단계 ${at}`,
    "",
    `원인은 \`paint_${at}\`가 폴마다 목록을 다시 세운 것입니다. 고친 뒤에는 새 턴만 잇고, 조용한 폴은 DOM을 건드리지 않습니다.`,
    "",
    "- 첫째: 상한에서 지운 행만 앞에서 지운다",
    "- 둘째: 살아 있는 호출만 다시 입힌다",
    "- 셋째: 스크롤은 읽는 사람의 것이다",
    "",
    `둘째 문단 ${at}: 닫힌 블록은 한 번 그리고, 쓰는 중인 블록만 프레임마다 다시 그립니다. 그래서 긴 답도 프레임 하나에 한 문단만 다시 그립니다.`,
    "",
    `셋째 문단 ${at}: 선반은 먼 행의 몸을 내려놓고 높이만 남깁니다. 가까워지면 몸을 다시 입히고, 위쪽 행이 자라면 그만큼 목록을 옮깁니다.`,
  ].join("\n");
}

export async function testConversationDiagrams(browser, origin, ok) {
  /* ---- 1 + 2 + 3 + 5: a streamed answer, then its turn ------------------------------------------ */
  {
    const { page, faults } = await openWindowTestPage(browser, origin);
    try {
      await standDiagrams(page);
      const history = [];
      for (let at = 0; at < 12; at += 1) {
        history.push({ role: "user", text: `${at}번째 부탁: 그림으로 설명해 줘.` });
        history.push({ role: "assistant", text: longAnswer(at) });
      }
      history.push({ role: "user", text: "흐름을 그림으로 보여 줘." });
      await openConversation(page, history, { status: "working" });
      const stream = await page.evaluate(async () => {
        const seen = window.__DIAGRAMS__;
        // The words the answer streams: a paragraph, a fence whose label holds the hostile markup, two
        // paragraphs.
        const text = ["첫 단락: 닫기 요청은 남은 일이 있는지부터 봅니다.", "", "```mermaid", "flowchart LR",
          `  A[높은흐름-그림 ${window.__HOSTILE__}] --> B{남은 일이 있나}`, "  B -- 아니요 --> C[바로 닫기]", "```", "",
          "둘째 단락: 남은 일이 있으면 최대 세 건을 처리하고 닫습니다.", "", "셋째 단락: 시간을 넘기면 남은 일을 기록합니다.", ""].join("\n");
        const face = document.querySelector("#worker-view");
        const list = face?.querySelector(".helper-turns") ?? null;
        const streaming = () => list?.querySelector(':scope > .is-streaming[data-role="assistant"]') ?? null;
        const fenceOpen = (words) => (words.match(/^```/gm) ?? []).length % 2 === 1;
        seen.hold = true;
        const asks = seen.asked.length;
        const frames = [];
        const deltas = 30;
        for (let at = 1; at <= deltas; at += 1) {
          const words = text.slice(0, Math.round((text.length * at) / deltas));
          window.__CONVERSATION__.live = [{ role: "assistant", text: words }];
          await pollHelperPages();
          // The stream is painted once a frame (`paintLiveAnswer`); this frame's paint has run.
          await new Promise(requestAnimationFrame);
          const tail = streaming()?.querySelector(".helper-said-tail") ?? null;
          // The open fence is code whatever its info string says so far: a frame can end on
          // "```mer", whose block is not yet named mermaid.
          frames.push({
            open: fenceOpen(words), asks: seen.asked.length - asks,
            tailDrawn: Boolean(tail?.querySelector(".mmd-block, svg.mmd")), tailCode: tail?.querySelectorAll("pre.md-block").length ?? null,
          });
        }
        await window.__FRAMES__(3);
        const row = streaming();
        const settled = row?.querySelector(".helper-said-settled") ?? null;
        const held = { asks: seen.asked.length - asks, code: window.__CODE__(settled), drawn: window.__DRAWN__(settled) };
        // The layout lands; the list that followed its foot keeps to it.
        let grew = null;
        list?.addEventListener("mmd-landed", (event) => {
          grew = event.grew;
        }, { once: true });
        window.__RELEASE__("높은흐름-그림");
        const landed = await window.__UNTIL__(() => window.__DRAWN__(settled) === 1);
        await window.__FRAMES__(2);
        const foot = list ? list.scrollHeight - list.scrollTop - list.clientHeight : null;
        const drawn = window.__READ_DIAGRAM__(settled, "높은흐름-그림");
        const tail = row?.querySelector(".helper-said-tail") ?? null;
        const settledEnd = { drawn: window.__DRAWN__(settled), tailDrawn: window.__DRAWN__(tail) };
        // The stream closes into its turn, which stands where the stream was.
        const asksBeforeTurn = seen.asked.length;
        window.__CONVERSATION__.turns.push({ role: "assistant", text });
        window.__CONVERSATION__.live = [];
        window.__CONVERSATION__.status = "idle";
        await pollHelperPages();
        // Read in the paint that stood the turn: no frame has passed.
        const turn = [...(list?.querySelectorAll(":scope > .helper-turn.is-assistant:not(.is-streaming)") ?? [])].at(-1) ?? null;
        const said = turn?.querySelector(":scope > .helper-said") ?? null;
        const atTurn = {
          turn: said?.textContent.includes("셋째 단락") ?? false, streaming: streaming() !== null,
          drawn: window.__DRAWN__(said), code: window.__CODE__(said),
        };
        await window.__FRAMES__(3);
        atTurn.asked = seen.asked.length - asksBeforeTurn;
        atTurn.read = window.__READ_DIAGRAM__(said, "높은흐름-그림");
        seen.hold = false;
        const slack = typeof CHAT_FOLLOW === "object" ? CHAT_FOLLOW.slack : 50;
        return { frames, held, landed, grew, slack, foot, drawn, settledEnd, atTurn };
      });
      const openFrames = stream.frames.filter((one) => one.open);
      ok(
        "an answer streamed over 30 frames asks render_mermaid once: the block still being written never draws, and while its fence is open it stands there as code",
        stream.frames.length === 30 && openFrames.length >= 3 && openFrames.every((one) => one.tailCode === 1)
          && stream.frames.every((one) => !one.tailDrawn) && stream.held.asks === 1
          && stream.frames.every((one) => one.asks <= 1),
        JSON.stringify({ frames: stream.frames, held: stream.held }),
      );
      ok(
        "the diagram ends in the settled part: the settled fence waits as code while its layout is on its way, then the diagram stands where it stood, its label holding <img src=x onerror=...> as text, and the block still being written holds none",
        stream.held.code === 1 && stream.held.drawn === 0 && stream.landed && stream.settledEnd.drawn === 1 && stream.settledEnd.tailDrawn === 0
          && stream.drawn.drawn && stream.drawn.labelled && stream.drawn.before === "첫 단락: 닫기 요청은 남은 일이 있는지부터 봅니다."
          && stream.drawn.after === "둘째 단락: 남은 일이 있으면 최대 세 건을 처리하고 닫습니다." && stream.drawn.contained
          && stream.drawn.markup === 0 && stream.drawn.pwned === null,
        JSON.stringify({ held: stream.held, drawn: stream.drawn, settledEnd: stream.settledEnd }),
      );
      ok(
        "a list that follows its foot keeps to it when the diagram lands, though the drawing grew the row by more than the foot's slack",
        stream.landed && typeof stream.grew === "number" && stream.grew > stream.slack && stream.foot !== null && stream.foot <= 1,
        JSON.stringify({ grew: stream.grew, slack: stream.slack, foot: stream.foot }),
      );
      ok(
        "the answer's turn, standing where its stream was, holds the diagram in the paint that stands it, with no second request",
        stream.atTurn.turn && !stream.atTurn.streaming && stream.atTurn.drawn === 1 && stream.atTurn.code === 0 && stream.atTurn.asked === 0
          && stream.atTurn.read.labelled && stream.atTurn.read.markup === 0 && stream.atTurn.read.pwned === null,
        JSON.stringify(stream.atTurn),
      );
      ok("the streamed conversation raised no errors", faults.length === 0, faults.slice(0, 3).join(" | "));
    } finally {
      await page.close();
    }
  }

  /* ---- 4: a reader above, a diagram landing over them, and the shelf -------------------------- */
  {
    const { page, faults } = await openWindowTestPage(browser, origin);
    try {
      await standDiagrams(page);
      const history = [];
      for (let at = 0; at < 40; at += 1) {
        history.push({ role: "user", text: `${at}번째 부탁: 계속.` });
        const words = longAnswer(at);
        history.push({
          role: "assistant",
          text: at === 3 ? `${words}\n\n\`\`\`mermaid\nflowchart TD\n  A[높은위-그림] --> B[끝]\n\`\`\`\n\n그림 뒤 문단.` : words,
        });
      }
      await openConversation(page, history);
      const reader = await page.evaluate(async () => {
        const seen = window.__DIAGRAMS__;
        const face = document.querySelector("#worker-view");
        const list = face?.querySelector(".helper-turns") ?? null;
        const frames = async (count) => {
          for (let beat = 0; beat < count; beat += 1) await window.__PAINTED__();
        };
        await frames(4);
        await new Promise((done) => setTimeout(done, 50));
        const row = [...(list?.querySelectorAll(":scope > .helper-turn.is-assistant") ?? [])]
          .find((one) => one.__turn?.text.includes("높은위-그림")) ?? null;
        const born = { found: row !== null, shelved: Boolean(row?.__shelved) };
        const waits = () => seen.waiting.some((one) => one.name === "높은위-그림");
        // The row a reader reads: the first under the list's top, and how far under it stands.
        const reading = () => {
          const top = list.getBoundingClientRect().top;
          const read = [...list.querySelectorAll(":scope > [data-turn]")].find((one) => one.getBoundingClientRect().bottom > top);
          return read ? { turn: read.dataset.turn, offset: read.getBoundingClientRect().top - top } : null;
        };
        const drift = (from, to) => (from && to && from.turn === to.turn ? Math.abs(to.offset - from.offset) : null);
        // The shelf has nothing left to dress: every row it found within reach has its body.
        const shelfStill = () => window.__UNTIL__(() => list && !list.__shelfFrame && !(list.__shelfWaiting?.size > 0));
        // To the top, as a reader drags the list there: the row stands in the view, takes its body
        // back and asks for its layout, whose answer is held.
        seen.hold = true;
        if (list) list.scrollTop = 0;
        const asked = await window.__UNTIL__(() => waits());
        await frames(2);
        // Down until the row's foot stands half a screen above the view: within the shelf's reach
        // (two screens), so it keeps its body, while its layout is still on its way.
        if (row && list) list.scrollTop += row.getBoundingClientRect().bottom - list.getBoundingClientRect().top + list.clientHeight / 2;
        await frames(2);
        const settled = await shelfStill();
        const held = { body: Boolean(row) && !row.__shelved, code: window.__CODE__(row), waiting: waits(), settled };
        const before = reading();
        // Where the row stands as its layout lands: above the view, its foot at or over the view's top.
        const aboveAtRelease = row && list ? row.getBoundingClientRect().bottom <= list.getBoundingClientRect().top : null;
        let grew = null;
        list?.addEventListener("mmd-landed", (event) => {
          grew = event.grew;
        }, { once: true });
        window.__RELEASE__("높은위-그림");
        seen.hold = false;
        const landed = await window.__UNTIL__(() => window.__DRAWN__(row) === 1);
        await frames(2);
        const after = reading();
        const place = list?.scrollTop ?? 0;
        // Sent back to the shelf: the reader goes to the foot, far past reach, until the row has given
        // up its body; then back to where they read.
        const asks = seen.asked.length;
        if (list) list.scrollTop = list.scrollHeight;
        const shelvedAgain = await window.__UNTIL__(() => Boolean(row?.__shelved));
        if (list) list.scrollTop = place;
        const dressed = await window.__UNTIL__(() => row && !row.__shelved && window.__DRAWN__(row) === 1);
        await frames(2);
        await shelfStill();
        const back = reading();
        return {
          born, asked, held, aboveAtRelease, grew, landed, drift: drift(before, after),
          shelf: { shelvedAgain, dressed, asked: seen.asked.length - asks, code: window.__CODE__(row), drift: drift(after, back) },
        };
      });
      ok(
        "a reader who left the foot keeps the row they read where it stood when a diagram lands in a row above the view",
        reader.born.found && reader.born.shelved && reader.asked && reader.held.body && reader.held.code === 1 && reader.held.waiting
          && reader.held.settled && reader.aboveAtRelease === true && reader.landed
          && typeof reader.grew === "number" && reader.grew > 0 && reader.drift !== null && reader.drift <= 1,
        JSON.stringify(reader),
      );
      ok(
        "a row sent back to the shelf and dressed again draws its diagram from the layout already made, with no request, and the reader's row stays where it stood",
        reader.shelf.shelvedAgain && reader.shelf.dressed && reader.shelf.asked === 0 && reader.shelf.code === 0
          && reader.shelf.drift !== null && reader.shelf.drift <= 1,
        JSON.stringify(reader.shelf),
      );
      ok("the long conversation raised no errors", faults.length === 0, faults.slice(0, 3).join(" | "));
    } finally {
      await page.close();
    }
  }
}

/* ---- the person's own pane: the conversation read off the transcript ---------------------------
 * A pane without a wire (zo, or any agent the window did not start itself) draws its answers from
 * the transcript the engine reads (`pane_log`), not from `wire_log`. Its answer takes the same road
 * as the wire's — `paintHelperProse` — so a ```mermaid fence in it draws once, from one request. */
export async function testConversationPaneDiagrams(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await standDiagrams(page);
    const answer = ["흐름은 아래와 같습니다.", "", "```mermaid", "flowchart LR", "  A[판흐름-그림] --> B[끝]", "```", "", "여기까지입니다."].join("\n");
    const seen = await page.evaluate(async (text) => {
      const term = await openTermTab({ placement: "tab" });
      paneAgents.set(term, "zo");
      hookStates.set(term, "idle");
      window.__ANSWER__.pane_log = () => ({
        found: true, next: 2, skipped: false, more: false, folded: false, model: "claude-opus-5",
        turns: [{ role: "user", text: "흐름을 그림으로 보여 줘." }, { role: "assistant", text }],
      });
      await setPaneChat(term, true);
      const asked = () => window.__DIAGRAMS__.asked.filter((name) => name === "판흐름-그림").length;
      const landed = await window.__UNTIL__(() => document.querySelectorAll(".helper-turns .mmd-block svg.mmd").length === 1, 60);
      await window.__FRAMES__(3);
      const said = [...document.querySelectorAll(".helper-turns .helper-turn.is-assistant .helper-said")].at(-1);
      return {
        wire: wireRowFor(term)?.id ?? null, landed, asked: asked(),
        drawn: window.__DRAWN__(said), code: window.__CODE__(said), after: said?.lastElementChild?.textContent ?? "",
      };
    }, answer);
    ok("a pane's conversation read off the transcript (no wire) draws a ```mermaid fence as a diagram, from one request, with no code left",
      seen.wire === null && seen.landed && seen.asked === 1 && seen.drawn === 1 && seen.code === 0 && seen.after === "여기까지입니다.",
      JSON.stringify(seen));
    ok("the page raised no errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}
