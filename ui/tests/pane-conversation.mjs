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
import { standZo } from "./conversation-parity.mjs";

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

    // ------------------------------------------------- the person's sticky row
    const placed = await page.evaluate(() => {
      const list = document.querySelector(".helper-turns");
      const user = document.querySelector(".helper-turn.is-user");
      const box = user.querySelector(".helper-said");
      const answer = document.querySelector(".helper-turn.is-assistant");
      const listBox = list.getBoundingClientRect();
      const userBox = box.getBoundingClientRect();
      const answerBox = answer.getBoundingClientRect();
      const gutter = parseFloat(getComputedStyle(list).paddingLeft);
      return {
        sticky: getComputedStyle(user).position,
        stickyTop: getComputedStyle(user).top,
        boxLeftGap: Math.round(userBox.left - listBox.left),
        boxRightGap: Math.round(listBox.right - userBox.right),
        gutter: Math.round(gutter),
        boxEdge: getComputedStyle(box).borderTopWidth,
        answerLeftGap: Math.round(answerBox.left - listBox.left),
      };
    });
    ok("what the person said is the extension's sticky header — the row sticks at the list's top, the box spans the list between its gutters with the input's 1px edge — and what the agent said stands on the rail beside it",
      placed.sticky === "sticky" && placed.stickyTop === "0px" &&
      placed.boxLeftGap === placed.gutter && placed.boxRightGap === placed.gutter &&
      placed.boxEdge === "1px" &&
      placed.answerLeftGap <= placed.boxLeftGap,
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

    // ------------------------------------------------ the channel's live text
    // zo streams what it is saying on its own channel (`session:frame`,
    // `text_delta` / `reasoning`): the page draws it as it arrives, by the
    // next frame; a finished piece stands until the transcript's turn carries
    // those words, and leaves in that same paint.
    const streamed = await page.evaluate(async () => {
      const tell = (name, payload) => {
        for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload });
      };
      const frame = () => new Promise((done) => requestAnimationFrame(done));
      const flat = (text) => text.replace(/\s+/g, " ").trim();
      const term = window.__TERM__;
      paneSessions.set(term, { agent: "zo", session: { key: "session_id", id: "s-zo-live" }, resumable: true });
      const list = document.querySelector(".helper-turns");
      const rows = () => [...list.querySelectorAll(":scope > .is-streaming")].map((row) => row.dataset.role);
      const seen = {};
      const say = (frame) => tell("session:frame", { session: "s-zo-live", frame });
      say({ type: "turn", turn_id: 9, phase: "start" });
      say({ type: "reasoning", id: 1, text: "먼저 파일을 읽고", done: false });
      say({ type: "reasoning", id: 1, text: " 고친다.", done: true });
      say({ type: "text_delta", id: 2, text: "원인은 ", done: false });
      await frame();
      seen.rowsAfterFirst = rows().join(",");
      // A thought that is going is one closed line — its newest sentence —
      // and what has been thought so far is one press away.
      const thinking = list.querySelector('.is-streaming[data-role="thinking"]');
      seen.thoughtLine = thinking?.open === false &&
        thinking.querySelector(".helper-step-target")?.textContent === "먼저 파일을 읽고 고친다";
      if (thinking) thinking.open = true;
      await frame();
      await frame();
      seen.thoughtShown = flat(thinking?.querySelector(".helper-thought-body")?.textContent ?? "") === "먼저 파일을 읽고 고친다.";
      say({ type: "text_delta", id: 2, text: "폴백이 끈적한 것", done: false });
      say({ type: "text_delta", id: 2, text: "입니다.", done: true });
      await frame();
      const live = () => flat(list.querySelector('.is-streaming[data-role="assistant"]')?.textContent ?? "");
      seen.answerByNextFrame = live() === "원인은 폴백이 끈적한 것입니다.";
      seen.doneStays = rows().join(",") === "thinking,assistant";
      seen.tailBeforeStatus = list.lastElementChild?.classList.contains("helper-status") === true &&
        list.querySelector(":scope > .is-streaming")?.nextElementSibling !== null;
      // The transcript brings the turns: the streaming rows go, the turn rows
      // stand, the words once.
      window.__LOG__ = { next: 3, turns: [{ role: "thinking", text: "먼저 파일을 읽고 고친다." }, { role: "assistant", text: "원인은 폴백이 끈적한 것입니다." }] };
      await pollHelperPages();
      window.__LOG__ = { next: 3, turns: [] };
      await frame();
      seen.settled = rows().length === 0 &&
        list.innerText.split("원인은 폴백이").length - 1 === 1 &&
        list.querySelectorAll("[data-turn]").length === 5;
      // A new turn's start clears whatever was left standing.
      say({ type: "text_delta", id: 3, text: "남은 조각", done: true });
      await frame();
      seen.leftover = rows().length;
      say({ type: "turn", turn_id: 10, phase: "start" });
      await frame();
      seen.clearedOnTurnStart = rows().length === 0 && paneLive.has(term) === false;
      return seen;
    });
    ok("a zo pane streams what its channel says — a thought and then the answer stand under the last turn by the next frame, a finished piece stays until the transcript's turn carries its words and leaves in that paint, the status row keeps the list's tail, and a new turn's start clears what was left",
      streamed.rowsAfterFirst === "thinking,assistant" && streamed.thoughtLine && streamed.thoughtShown &&
      streamed.answerByNextFrame && streamed.doneStays && streamed.tailBeforeStatus &&
      streamed.settled && streamed.leftover === 1 && streamed.clearedOnTurnStart,
      JSON.stringify(streamed));

    // ---------------------------------------- the lifetime of the streaming rows (the pane's own channel)
    // zo opens and closes blocks it says nothing in, and ends a thought with a frame of its own: a block with no
    // word leaves no piece and no row, a thought that is over is a thought and not 「생각 중…」, and the foot line
    // follows the newest thought that goes — on the frame, not on the next poll.
    const lifetime = await page.evaluate(async () => {
      const tell = (name, payload) => {
        for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload });
      };
      const frame = () => new Promise((done) => requestAnimationFrame(done));
      const term = window.__TERM__;
      const list = document.querySelector(".helper-turns");
      const thoughts = () => [...list.querySelectorAll(":scope > .is-streaming.is-thinking")];
      const part = (row, name) => row.firstElementChild.querySelector(name)?.textContent ?? null;
      const now = () => list.querySelector(":scope > .helper-status .helper-status-now")?.textContent ?? "";
      const say = (body) => tell("session:frame", { session: "s-zo-live", frame: body });
      const going = t("worker.thinking", "생각 중…");
      const over = t("worker.thought", "생각");
      const seen = { going, over };
      say({ type: "turn", turn_id: 40, phase: "start" });
      await frame();
      say({ type: "reasoning", id: 1, text: "Gathering task details", done: false });
      say({ type: "reasoning", id: 1, text: "", done: true });
      for (let id = 2; id <= 21; id += 1) {
        say({ type: "reasoning", id, text: "", done: false });
        say({ type: "reasoning", id, text: "", done: true });
      }
      say({ type: "reasoning", id: 22, text: "ok", done: true });
      await frame();
      await frame();
      seen.kept = (paneLive.get(term) ?? []).length;
      seen.finished = {
        rows: thoughts().length,
        titles: thoughts().map((row) => part(row, ".helper-step-target")),
        labels: thoughts().map((row) => part(row, ".helper-step-res")),
        now: now(),
      };
      say({ type: "reasoning", id: 23, text: "**Checking the current task**\nThen the cells.", done: false });
      await frame();
      await frame();
      seen.fresh = { rows: thoughts().length, now: now(), label: part(thoughts().at(-1), ".helper-step-res") };
      say({ type: "reasoning", id: 23, text: " One more look.", done: true });
      await frame();
      await frame();
      seen.ended = { rows: thoughts().length, now: now(), label: part(thoughts().at(-1), ".helper-step-res") };
      say({ type: "reasoning", id: 24, text: "**Checking the cells**\nSlowly.", done: false });
      await frame();
      await frame();
      seen.next = { rows: thoughts().length, now: now() };
      // The transcript brings the thoughts that were over: their rows go on that paint, the one that goes stays.
      window.__LOG__ = { next: 5, turns: [{ role: "thinking", text: "Gathering task details" }, { role: "thinking", text: "ok" }] };
      await pollHelperPages();
      window.__LOG__ = { next: 5, turns: [] };
      await frame();
      await frame();
      seen.settled = {
        rows: thoughts().length,
        titles: thoughts().map((row) => part(row, ".helper-step-target")),
        now: now(),
        pieces: (paneLive.get(term) ?? []).map((piece) => piece.id),
      };
      // The words of a finished answer stand while the transcript is late.
      say({ type: "text_delta", id: 25, text: "Final words.", done: true });
      await frame();
      await pollHelperPages();
      await frame();
      await frame();
      const answers = [...list.querySelectorAll(':scope > .is-streaming[data-role="assistant"]')];
      seen.answer = answers.length === 1 && answers[0].textContent.includes("Final words.");
      say({ type: "turn", turn_id: 41, phase: "start" });
      await frame();
      // What a delta carries is kept as it came — the indentation or the blank lines that open an answer
      // are its start, a space between two deltas is a space — and a piece of nothing but blanks is not drawn.
      // This runs on the page the turn's start left empty: the rows it draws are the only streaming ones.
      say({ type: "text_delta", id: 30, text: "    ", done: false });
      say({ type: "text_delta", id: 30, text: "const x = 1;", done: false });
      say({ type: "text_delta", id: 31, text: "\n\n", done: false });
      say({ type: "text_delta", id: 31, text: "```js\ncode\n```", done: true });
      say({ type: "text_delta", id: 32, text: "hello", done: false });
      say({ type: "text_delta", id: 32, text: " ", done: false });
      say({ type: "text_delta", id: 32, text: "world", done: true });
      say({ type: "text_delta", id: 33, text: "  ", done: true });
      say({ type: "text_delta", id: 34, text: "", done: true });
      await frame();
      await frame();
      const byId = (id) => (paneLive.get(term) ?? []).find((piece) => piece.id === id)?.text ?? null;
      seen.raw = {
        indented: byId(30), fenced: byId(31), spaced: byId(32), blank: byId(33), empty: byId(34),
        drawn: list.querySelectorAll(':scope > .is-streaming[data-role="assistant"]').length,
        streaming: list.querySelectorAll(":scope > .is-streaming").length,
      };
      say({ type: "turn", turn_id: 42, phase: "start" });
      await frame();
      return seen;
    });
    ok(
      "a zo pane's blocks that said nothing leave no piece and no row — twenty of them beside two thoughts leave two pieces, two rows, each in its own words and called 「생각」, and the foot line names none of them",
      lifetime.kept === 2 && lifetime.finished.rows === 2 &&
        lifetime.finished.titles.join("|") === "Gathering task details|ok" &&
        lifetime.finished.labels.every((label) => label === lifetime.over) && !/Gathering|ok$/.test(lifetime.finished.now),
      JSON.stringify(lifetime.finished),
    );
    ok(
      "the foot line follows the thought that goes on the frame it began — its heading — and drops it on the frame it ended; the next one that goes is the one it names, never an older one",
      lifetime.fresh.rows === 3 && lifetime.fresh.label === lifetime.going && lifetime.fresh.now.endsWith("Checking the current task") &&
        lifetime.ended.rows === 3 && lifetime.ended.label === lifetime.over && !/Gathering|Checking/.test(lifetime.ended.now) &&
        lifetime.next.rows === 4 && lifetime.next.now.endsWith("Checking the cells") && !lifetime.next.now.includes("Gathering"),
      JSON.stringify({ fresh: lifetime.fresh, ended: lifetime.ended, next: lifetime.next }),
    );
    ok(
      "a delta's whitespace is kept as it came — indentation, the blank lines before a fence, a lone space between two words — a piece of nothing but blanks is kept but not drawn, and a delta with no text at all is not kept",
      lifetime.raw.indented === "    const x = 1;" && lifetime.raw.fenced === "\n\n```js\ncode\n```" && lifetime.raw.spaced === "hello world" &&
        lifetime.raw.blank === "  " && lifetime.raw.empty === null && lifetime.raw.drawn === 3 && lifetime.raw.streaming === 3,
      JSON.stringify(lifetime.raw),
    );
    ok(
      "the transcript's thoughts take the rows of the thoughts that were over in that paint — the one that goes stays, named — and the words of a finished answer stand until its turn comes",
      lifetime.settled.rows === 1 && lifetime.settled.titles[0] === "Checking the cells" &&
        lifetime.settled.now.endsWith("Checking the cells") && lifetime.settled.pieces.join() === "24" && lifetime.answer === true,
      JSON.stringify({ settled: lifetime.settled, answer: lifetime.answer }),
    );

    // ---------------------------------------------------------------- the dock
    const dock = await page.evaluate(async () => {
      const frame = () => new Promise((done) => requestAnimationFrame(done));
      await frame();
      await frame();
      const host = document.querySelector(".pane-chat.is-chat-page");
      const dock = host?.querySelector(":scope > .chat-dock");
      const list = host?.querySelector(".helper-turns");
      const style = dock ? getComputedStyle(dock) : null;
      const read = (name) => getComputedStyle(host).getPropertyValue(name).trim();
      return {
        floats: style?.position === "absolute",
        inset: style ? [style.bottom, style.left, style.right].join(",") : "",
        wantInset: [read("--chat-dock-inset"), read("--chat-dock-inset"), read("--chat-dock-inset")].join(","),
        measure: style?.maxWidth ?? "",
        wantMeasure: read("--chat-dock-max"),
        composerInDock: dock?.firstElementChild?.classList.contains("worker-composer") === true,
        roomUnder: list && dock
          ? parseFloat(getComputedStyle(list).paddingBottom) >= dock.offsetHeight + parseFloat(read("--chat-list-pad-bottom"))
          : false,
        fades: getComputedStyle(host, "::after").height === read("--chat-fade"),
        statusLast: list?.lastElementChild?.classList.contains("helper-status") === true,
      };
    });
    ok("the composer floats in the extension's dock — absolute at the foot, inset by the dock's margin on three sides, no wider than its measure, the list keeping room under its words for it and fading under it, the status row the list's last child",
      dock.floats && dock.inset === dock.wantInset && dock.measure === dock.wantMeasure && dock.composerInDock &&
      dock.roomUnder && dock.fades && dock.statusLast,
      JSON.stringify(dock));

    // ------------------------------------------------ only the list scrolls
    // Following the tail with `scrollIntoView` on the last row moved every
    // scrollable ancestor as well — the document included, `overflow: hidden`
    // notwithstanding — and a conversation standing a little below the
    // viewport dragged the tab strip and the side rail out of the window
    // (installed 1.1.11, 2026-09-21). The page is stood a little taller and
    // wider than its viewport here, the way a slot that mis-measures does, so
    // a document scroll would show; the list follows its tail, the document
    // does not move, and a person's row clicked home moves the list alone.
    const pageScroll = await page.evaluate(async () => {
      const frame = () => new Promise((done) => requestAnimationFrame(done));
      const list = document.querySelector(".helper-turns");
      const root = document.scrollingElement;
      const before = {
        overflowY: root.scrollHeight - root.clientHeight,
        overflowX: root.scrollWidth - root.clientWidth,
      };
      const spacer = document.createElement("div");
      spacer.style.cssText = "position:absolute;left:100%;top:100%;width:400px;height:120px";
      document.body.appendChild(spacer);
      try {
        list.scrollTop = list.scrollHeight;
        const turns = [];
        for (let i = 0; i < 30; i += 1) {
          turns.push({ role: "user", text: `질문 ${i}` });
          turns.push({ role: "assistant", text: `답 ${i} ` + "긴 줄 ".repeat(120) });
        }
        window.__LOG__ = { next: 100, turns };
        await pollHelperPages();
        window.__LOG__ = { next: 100, turns: [] };
        await frame();
        await frame();
        const followed = {
          docTop: root.scrollTop,
          docLeft: root.scrollLeft,
          listAtBottom: Math.abs(list.scrollTop + list.clientHeight - list.scrollHeight) <= 2,
        };
        // A person's row stuck at the top, clicked, comes home in the list.
        const users = [...list.querySelectorAll(".helper-turn.is-user")];
        const stuck = users.find((row) =>
          Math.round(row.getBoundingClientRect().top) <= Math.round(list.getBoundingClientRect().top));
        stuck?.click();
        await frame();
        const home = stuck
          ? Math.round(stuck.getBoundingClientRect().top) - Math.round(list.getBoundingClientRect().top)
          : null;
        return { before, ...followed, home, docTopAfterClick: root.scrollTop, docLeftAfterClick: root.scrollLeft };
      } finally {
        spacer.remove();
      }
    });
    ok("only the list scrolls — the tail is followed and a person's row is clicked home without the document moving, on a page stood taller and wider than its viewport",
      pageScroll.docTop === 0 && pageScroll.docLeft === 0 && pageScroll.listAtBottom &&
      pageScroll.home === 0 && pageScroll.docTopAfterClick === 0 && pageScroll.docLeftAfterClick === 0,
      JSON.stringify(pageScroll));
    ok("the conversation page itself does not overflow its viewport",
      pageScroll.before.overflowY === 0 && pageScroll.before.overflowX === 0,
      JSON.stringify(pageScroll.before));

    // A pane nobody has sent a model-bearing hook for still knows what it is
    // on, because its own transcript says so. The chip itself is this exact
    // read (`paneModels.get(run.term)`, ui/shell-composer.js) — it is not
    // drawn here because the stubbed agent catalog has no zo to draw it for.
    const chip = await page.evaluate(() => ({
      model: paneModels.get(window.__TERM__) ?? null,
    }));
    ok("a pane learns its model from the transcript when no hook has carried one",
      chip.model === "claude-opus-5", JSON.stringify(chip));

    // ------------------------------------ what the pane is doing, with no row to say it
    // zo says what its working row says in every `session_status` frame — `activity`: a verb, a
    // target, a phase. With no step or thought out to name, the foot line says it in the window's own
    // words: the step rows' kind words for a tool, the page's own for waiting, reconnecting and
    // thinking, the window's 「작업 중…」 when nothing is known — never zo's English 「Working…」
    // on the Korean page. The turn's end (or its next start) takes it away. It stands last: the zo row it
    // installs is the catalog the rest of this test's page was drawn without.
    await standZo(page);
    // The page was drawn before the row stood — in the window the agent list is the boot's own, read before
    // any page is drawn — so it is drawn again, now that zo has its voice.
    await page.evaluate(async () => {
      paintPaneChat(window.__TERM__);
      await new Promise((done) => requestAnimationFrame(done));
    });
    const nowLine = await page.evaluate(async () => {
      const tell = (name, payload) => {
        for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload });
      };
      const frame = () => new Promise((done) => requestAnimationFrame(done));
      const term = window.__TERM__;
      const session = "s-zo-live";
      const status = () => document.querySelector(".pane-chat .helper-status");
      const words = () => status()?.querySelector(".helper-status-now")?.textContent ?? "";
      const seeing = () => (status()?.innerText ?? "").replace(/\s+/g, " ").trim();
      const state = () => ({ now: words(), seeing: seeing(), naming: status()?.classList.contains("is-naming") ?? false });
      const say = async (activity) => {
        tell("session:frame", { session, frame: { type: "session_status", session, model: "claude-opus-5", ...(activity && { activity }) } });
        await frame();
        await frame();
        return state();
      };
      const voice = agentVoice("zo");
      const now = t("worker.now", "지금");
      const seen = {
        busy: voice.busy_word,
        wordNode: status()?.querySelector(".helper-status-word")?.textContent ?? "",
        wantNothing: `${now} · ${t("worker.busy", "작업 중…")}`,
        wantRead: `${now} · ${t("worker.stepRead", "파일 읽기")} src/tui/view.rs`,
        wantWaiting: `${now} · ${t("worker.nowWaiting", "답을 기다리는 중")}`,
        wantReconnecting: `${now} · ${t("worker.nowReconnecting", "다시 연결하는 중")}`,
        wantThinking: `${now} · ${t("worker.nowThinking", "생각하는 중")}`,
      };
      tell("session:frame", { session, frame: { type: "turn", turn_id: 20, phase: "start" } });
      await frame();
      await frame();
      seen.nothing = await say(null);
      seen.read = await say({ verb: "read", target: "src/tui/view.rs", phase: "started", elapsed_secs: 2 });
      seen.bash = await say({ verb: "bash", target: "cd /Users/dev/shop-app && make test", phase: "started", elapsed_secs: 3 });
      seen.waiting = await say({ verb: "waiting", target: "model", phase: "started", elapsed_secs: 4 });
      seen.reconnecting = await say({ verb: "reconnecting", target: "attempt 2 in 5s", phase: "started", elapsed_secs: 5 });
      seen.thinking = await say({ verb: "reasoning silently", phase: "started", elapsed_secs: 6 });
      seen.quiet = await say({ verb: "quiet", target: "last: read a.rs", phase: "started", elapsed_secs: 7 });
      // A frame that carries no activity (zo leaves it out while the pane is idle and in the first
      // token's grace period) clears the line.
      await say({ verb: "grep", target: "SCREEN_WIDTH", phase: "started", elapsed_secs: 1 });
      seen.cleared = await say(null);
      // The turn's end clears it too, whatever the last frame said.
      await say({ verb: "grep", target: "SCREEN_WIDTH", phase: "started", elapsed_secs: 1 });
      tell("session:frame", { session, frame: { type: "turn", turn_id: 20, phase: "end" } });
      await frame();
      await frame();
      seen.ended = state();
      // And a new turn's start clears what was left standing.
      await say({ verb: "grep", target: "SCREEN_WIDTH", phase: "started", elapsed_secs: 1 });
      tell("session:frame", { session, frame: { type: "turn", turn_id: 21, phase: "start" } });
      await frame();
      await frame();
      seen.restarted = state();
      // Another session's frames are not this pane's.
      tell("session:frame", { session: "s-elsewhere", frame: { type: "session_status", session: "s-elsewhere", activity: { verb: "read", target: "elsewhere.rs", phase: "started", elapsed_secs: 1 } } });
      await frame();
      await frame();
      seen.elsewhere = state();
      return seen;
    });
    const nothingKnown = (one) => one.now === nowLine.wantNothing && one.naming && !one.seeing.includes(nowLine.busy);
    ok("a zo pane with no step out and no activity known says the window's own 「지금 · 작업 중…」 — zo's English word is not on the page, though the word's own node still holds it",
      nowLine.busy !== "" && nothingKnown(nowLine.nothing) && nowLine.wordNode === nowLine.busy,
      JSON.stringify(nowLine.nothing));
    ok("the activity in zo's session status is said on the foot line — a file read in the step rows' words, a shell command by its title (the command's first words and the folder, no cd) — and waiting, reconnecting and thinking in the page's own words",
      nowLine.read.now === nowLine.wantRead && nowLine.bash.now.includes("make test") &&
      nowLine.bash.now.endsWith(" · shop-app") && !/(^|\s)cd\s/.test(nowLine.bash.now) &&
      nowLine.waiting.now === nowLine.wantWaiting && nowLine.reconnecting.now === nowLine.wantReconnecting &&
      nowLine.thinking.now === nowLine.wantThinking &&
      [nowLine.read, nowLine.bash, nowLine.waiting, nowLine.reconnecting, nowLine.thinking].every((one) => one.naming && !one.seeing.includes(nowLine.busy)),
      JSON.stringify(nowLine));
    ok("a status that says quiet names nothing the page can name, and a status frame with no activity takes the line back to 「작업 중…」",
      nothingKnown(nowLine.quiet) && nothingKnown(nowLine.cleared),
      JSON.stringify({ quiet: nowLine.quiet, cleared: nowLine.cleared }));
    ok("the turn's end and the next turn's start take the activity away, and another session's status is not this pane's",
      nothingKnown(nowLine.ended) && nothingKnown(nowLine.restarted) && nothingKnown(nowLine.elsewhere),
      JSON.stringify({ ended: nowLine.ended, restarted: nowLine.restarted, elsewhere: nowLine.elsewhere }));

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
