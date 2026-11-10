/* The conversation the person passed on 2026-11-10 (window artifact
 * p-1c1bbf75726ff227): what a turn changed stands under its answer as a card on
 * the reading column — never as a panel across the pane; the head's two marks
 * are one named 「보기」; an answer's actions are marks that say their names; the
 * turn at work is one card with a quiet line under it; and a new answer starts
 * under the question that asked for it. Each check reads the page a browser
 * laid out. The conversations are synthetic.
 *
 *   WINDOW_SUITES='^conversation-(result|view-menu|pin)$' node ui/tests/window.mjs */
import { afterTurns, openAfterScene } from "./conversation-after.mjs";
import { openConversation, openPaneConversation, standZo } from "./conversation-parity.mjs";
import { foldConversations, openWindowTestPage } from "./window-boot.mjs";

/* Where the card, the column and the composer stand, and what the card says. */
function readResult() {
  const host = document.querySelector(".pane-chat");
  const list = host.querySelector(".helper-turns");
  const box = (node) => {
    if (!node) return null;
    const at = node.getBoundingClientRect();
    return { left: Math.round(at.left), right: Math.round(at.right), top: Math.round(at.top), bottom: Math.round(at.bottom), width: Math.round(at.width) };
  };
  // The newest answer's card — an older answer's stands too once its own 「변경 N」 was pressed.
  const card = list.querySelector(":scope > .helper-turn.is-latest > .chat-result") ?? list.querySelector(".chat-result");
  const answer = card?.closest(".helper-turn");
  return {
    card: box(card),
    prose: box(answer?.querySelector(":scope > .helper-said")),
    dock: box(host.querySelector(".chat-dock")),
    list: box(list),
    latest: answer?.classList.contains("is-latest") ?? null,
    cards: list.querySelectorAll(".chat-result").length,
    panel: host.querySelectorAll(".chat-review").length,
    title: card?.querySelector(".chat-result-title")?.textContent ?? null,
    added: card?.querySelector(".chat-result-head .chat-result-added")?.textContent ?? null,
    removed: card?.querySelector(".chat-result-head .chat-result-removed")?.textContent ?? null,
    files: [...(card?.querySelectorAll(".chat-result-file") ?? [])].map((line) => ({
      path: line.querySelector(".chat-result-path").textContent,
      open: line.getAttribute("aria-expanded"),
      name: line.getAttribute("aria-label"),
      // Its share of added lines, for the eye only.
      share: line.querySelector(".chat-result-bar")?.style.getPropertyValue("--chat-result-share") ?? null,
      shareSaid: line.querySelector(".chat-result-bar")?.getAttribute("aria-hidden") ?? null,
    })),
    rows: card?.querySelectorAll(".chat-result-diff .diff-line").length ?? 0,
    // The line's own words: its first text, before the command that ran the tests.
    tests: card?.querySelector(".chat-result-tests")?.firstChild?.textContent ?? null,
    command: card?.querySelector(".chat-result-command")?.textContent ?? null,
    testsState: card?.querySelector(".chat-result-tests")?.dataset.state ?? null,
    all: card?.querySelector(".chat-result-all")?.textContent ?? null,
    waits: card?.dataset.review ?? null,
    sideways: document.documentElement.scrollWidth - document.documentElement.clientWidth,
    listSideways: list.scrollWidth - list.clientWidth,
  };
}

export async function testConversationResult(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await openAfterScene(page);
    await foldConversations(page);
    await page.evaluate(() => window.__PAINTED__());
    const term = await page.evaluate(() => activePaneOf(currentTab()));
    const start = await page.evaluate(readResult);
    ok("result: the newest finished answer wears one card of what its turn changed — the files with their lines, and the turn's test line — and no panel stands over the composer",
      start.cards === 1 && start.latest === true && start.panel === 0 && start.title === "변경된 파일 1개" &&
        start.added === "+2" && start.removed === "−1" && start.files.length === 1 && start.files[0].path === "src/session.rs" &&
        start.files[0].open === "false" && start.files[0].share === "60%" && start.files[0].shareSaid === "true" &&
        start.tests === "✓ 테스트 3개 통과" && start.command === "cargo test" && start.testsState === "passed",
      JSON.stringify(start));
    ok("result: the card stands on the reading column, under the answer's words and over the composer",
      start.card.left === start.prose.left && start.card.right === start.prose.right && start.card.top >= start.prose.bottom &&
        start.card.bottom <= start.dock.top,
      JSON.stringify(start));
    const waiting = await page.evaluate((term) => paneReviewWaiting(term), term);
    ok("result: unopened, the changes wait for the person — the card's 「검토」 wears the one blue and the sidebar counts the file",
      start.waits === "waiting" && waiting === 1, JSON.stringify({ waits: start.waits, waiting }));

    await page.locator(".pane-chat .chat-result-file").click();
    const opened = await page.evaluate(readResult);
    const spent = await page.evaluate((term) => paneReviewWaiting(term), term);
    ok("result: a press on a file's line opens its rows inside the card, and the changes no longer wait",
      opened.files[0].open === "true" && opened.rows === 3 && opened.all === "접기" && opened.waits === null && spent === 0,
      JSON.stringify({ opened, spent }));
    await page.locator(".pane-chat .chat-result-file").click();
    const closed = await page.evaluate(readResult);
    ok("result: the same press lets the rows go again", closed.files[0].open === "false" && closed.rows === 0 && closed.all === "검토", JSON.stringify(closed));

    // The head's 「변경 N」 says the files and their lines, and brings the card with every file open.
    const head = await page.evaluate(() => {
      const chip = document.querySelector(".pane-chat .chat-head-review");
      return { words: chip.querySelector(".chat-head-review-words").textContent, added: chip.querySelector(".chat-result-added").textContent,
        removed: chip.querySelector(".chat-result-removed").textContent, hidden: chip.hidden };
    });
    await page.locator(".pane-chat .chat-head-review").click();
    const fromHead = await page.evaluate(readResult);
    ok("result: the head's 「변경 1 +2 −1」 opens every file of the newest answer's card",
      head.words === "변경 1" && head.added === "+2" && head.removed === "−1" && !head.hidden &&
        fromHead.files.every((file) => file.open === "true") && fromHead.rows === 3 && fromHead.panel === 0,
      JSON.stringify({ head, fromHead }));

    // An older answer says its changes on its action row, and a press stands its card.
    const older = await page.evaluate(async () => {
      const list = document.querySelector(".pane-chat .helper-turns");
      const first = [...list.querySelectorAll(":scope > .helper-turn.is-assistant.is-turn-answer")][0];
      first.dispatchEvent(new PointerEvent("pointerover", { bubbles: true }));
      await window.__PAINTED__();
      const chip = first.querySelector(".chat-answer-changes");
      const before = { cards: list.querySelectorAll(".chat-result").length, hidden: chip?.hidden ?? null,
        words: chip?.querySelector(".chat-answer-changes-words").textContent, name: chip?.getAttribute("aria-label") };
      chip?.click();
      await window.__PAINTED__();
      const card = first.querySelector(":scope > .chat-result");
      return { before, card: Boolean(card), path: card?.querySelector(".chat-result-path")?.textContent ?? null,
        open: card?.querySelector(".chat-result-file")?.getAttribute("aria-expanded") ?? null, chipHidden: chip?.hidden ?? null };
    });
    ok("result: an older answer says 「변경 1」 on its action row, and a press stands its own card with the file open",
      older.before.cards === 1 && older.before.hidden === false && older.before.words === "변경 1" &&
        older.before.name === "이 턴의 변경 검토 · 파일 1개" && older.card && older.path === "docs.md" && older.open === "true" && older.chipHidden === true,
      JSON.stringify(older));

    // The marks under an answer say their names twice. The card that just stood asked its owner whether the
    // turn can be undone, and the answer's own way back and fork waited for that answer: they come back by themselves.
    const readMarks = () => page.evaluate(() => {
      const list = document.querySelector(".pane-chat .helper-turns");
      return [...list.__actions.querySelectorAll(".chat-answer-icon")].map((mark) => ({
        tip: mark.dataset.tip ?? "", name: mark.getAttribute("aria-label") ?? "", words: mark.textContent.trim(), icon: Boolean(mark.querySelector("svg")),
        hidden: mark.hidden, disabled: mark.disabled,
      }));
    });
    await page.waitForFunction(() => [...document.querySelector(".pane-chat .helper-turns").__actions
      .querySelectorAll("[data-conversation-control]")].every((mark) => !mark.disabled));
    const marks = await readMarks();
    ok("result: an answer's actions are marks — the copy, the turn's way back, its fork — each named by its tooltip and its label",
      marks.length === 3 && marks.every((mark) => mark.icon && mark.words === "" && mark.tip !== "" && mark.tip === mark.name && !mark.disabled) &&
        marks.map((mark) => mark.name).join("|") === "답 복사|이 턴 되돌리기|분기",
      JSON.stringify(marks));
    // While the pane works the two wait, and say their name before the reason.
    await page.evaluate((term) => {
      const list = document.querySelector(".pane-chat .helper-turns");
      hookStates.set(term, "working");
      updateConversationActions(list.__actions, list, list.__actions.__row);
    }, term);
    const waitingMarks = await readMarks();
    await page.evaluate((term) => {
      const list = document.querySelector(".pane-chat .helper-turns");
      hookStates.set(term, "done");
      updateConversationActions(list.__actions, list, list.__actions.__row);
    }, term);
    ok("result: a mark that waits is still named — its name first, then why it waits",
      waitingMarks[1].disabled && waitingMarks[2].disabled && waitingMarks[1].name.startsWith("이 턴 되돌리기 — ") &&
        waitingMarks[2].name.startsWith("분기 — ") && waitingMarks.every((mark) => mark.tip === mark.name) && !waitingMarks[0].disabled,
      JSON.stringify(waitingMarks));

    // On a wide window the card keeps to the column (the person's 2560pt window, 2026-11-10); on a narrow one,
    // and with wider glyphs than this machine's, nothing pushes the page sideways.
    for (const [width, height] of [[2560, 1332], [1440, 900]]) {
      await page.setViewportSize({ width, height });
      await page.evaluate(async () => { await window.__PAINTED__(); await window.__PAINTED__(); });
      const wide = await page.evaluate(readResult);
      ok(`result: at ${width}px the card and its open rows stand on the column, as wide as the composer's column and no wider`,
        wide.card.left === wide.prose.left && wide.card.right === wide.prose.right && wide.card.width <= wide.dock.width + 1 &&
          wide.card.left >= wide.dock.left - 1 && wide.card.right <= wide.dock.right + 1 && wide.rows === 3 && wide.sideways <= 0,
        JSON.stringify(wide));
    }
    await page.setViewportSize({ width: 390, height: 800 });
    for (const spaced of [false, true]) {
      if (spaced) await page.addStyleTag({ content: "* { letter-spacing: 0.1em !important }" });
      await page.evaluate(async () => { await window.__PAINTED__(); await window.__PAINTED__(); });
      const narrow = await page.evaluate(readResult);
      ok(`result: at 390px${spaced ? " with wider glyphs" : ""} the card stays inside the list and nothing pushes the page sideways`,
        narrow.card.left >= narrow.list.left - 1 && narrow.card.right <= narrow.list.right + 1 && narrow.sideways <= 0 && narrow.listSideways <= 0,
        JSON.stringify(narrow));
    }
    ok("result: no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
  await testConversationResultMany(browser, origin, ok);
}

/* A turn that touched many files: the card stands its first eight lines and one line that says how many wait. */
async function testConversationResultMany(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    const files = Array.from({ length: 11 }, (_, at) => `src/part_${String(at).padStart(2, "0")}.rs`);
    const turns = [{ role: "user", text: "열한 파일을 고쳐 줘." }];
    for (const [at, path] of files.entries()) {
      turns.push({ role: "tool", text: `Edit ${path}`, tool: { call_id: `e${at}`, name: "Edit", kind: "edit", input: path, file: { path },
        edits: [{ path, lines: [{ kind: "add", text: `// ${at}` }] }] } });
      turns.push({ role: "tool_result", text: "updated", tool: { call_id: `e${at}`, is_error: false } });
    }
    turns.push({ role: "assistant", text: "열한 파일을 고쳤습니다." });
    await standZo(page);
    await openPaneConversation(page, turns);
    await page.evaluate(async () => {
      const term = activePaneOf(currentTab());
      hookStates.set(term, "done");
      await pollHelperPages();
      paintPaneChat(term);
      await window.__PAINTED__();
    });
    await foldConversations(page);
    await page.evaluate(() => window.__PAINTED__());
    const read = () => page.evaluate(() => {
      const card = document.querySelector(".pane-chat .helper-turn.is-latest > .chat-result");
      const more = card?.querySelector(".chat-result-more");
      const lines = [...(card?.querySelectorAll(".chat-result-file") ?? [])];
      return {
        title: card?.querySelector(".chat-result-title").textContent ?? null, lines: lines.length,
        more: more?.textContent ?? null, moreIsButton: more?.tagName === "BUTTON",
        open: lines.filter((line) => line.getAttribute("aria-expanded") === "true").length,
        focus: document.activeElement?.querySelector?.(".chat-result-path")?.textContent ?? null,
        elements: card?.querySelectorAll("*").length ?? 0,
      };
    });
    const start = await read();
    ok("result: a turn that touched eleven files stands eight lines and one that says how many wait — the head still counts them all",
      start.title === "변경된 파일 11개" && start.lines === 8 && start.more === "파일 3개 더 보기" && start.moreIsButton,
      JSON.stringify(start));
    await page.locator(".pane-chat .chat-result-more").focus();
    await page.keyboard.press("Enter");
    const shown = await read();
    ok("result: Enter on that line stands the rest in its place, and the keyboard goes on from the first of them",
      shown.lines === 11 && shown.more === null && shown.focus === "src/part_08.rs" && shown.open === 0, JSON.stringify(shown));
    // 「검토」 opens every file — the ones that waited too — on a card that stands anew.
    await page.evaluate(async () => {
      const list = document.querySelector(".pane-chat .helper-turns");
      list.querySelector(".chat-result").remove();
      syncHelperTurns(list, list.__run);
      await window.__PAINTED__();
    });
    const again = await read();
    await page.locator(".pane-chat .chat-result-all").click();
    const all = await read();
    ok("result: 「검토」 on a card whose lines still wait stands them and opens every file",
      again.lines === 8 && again.more === "파일 3개 더 보기" && all.lines === 11 && all.more === null && all.open === 11, JSON.stringify({ again, all }));
    ok("result: the long card raised no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* The head's 「보기」: one named control for how much of its work a conversation
 * shows, and where a new answer starts. */
export async function testConversationViewMenu(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    // The after scene with a thought at the head of each turn's work.
    await standZo(page);
    await openPaneConversation(page, thoughtfulTurns());
    await page.evaluate(async () => {
      const term = activePaneOf(currentTab());
      paneSessions.set(term, { agent: "zo", session: { id: "after-session" } });
      hookStates.set(term, "done");
      await pollHelperPages();
      paintPaneChat(term);
      await window.__PAINTED__();
    });
    await foldConversations(page);
    await page.evaluate(() => {
      window.__VIEW_SETS__ = [];
      window.__ANSWER__.set_conversation_timeline_view = (args) => {
        window.__VIEW_SETS__.push(args.on);
        return null;
      };
    });
    const read = () => page.evaluate(() => {
      const host = document.querySelector(".pane-chat");
      const list = host.querySelector(".helper-turns");
      const button = host.querySelector(".chat-view");
      return {
        word: button.querySelector(".chat-view-mode").textContent,
        name: button.querySelector(".chat-view-name").textContent,
        tip: button.dataset.tip, label: button.getAttribute("aria-label"), expanded: button.getAttribute("aria-expanded"),
        marks: host.querySelectorAll(".chat-head :is(.chat-answers-only, .worker-focus)").length,
        timeline: conversationTimelineView,
        answersOnly: document.documentElement.dataset.chatAnswersOnly ?? null,
        pillsShown: [...list.querySelectorAll(":scope > .helper-group")].filter((row) => row.getClientRects().length > 0).length,
        steps: list.querySelectorAll(":scope > .helper-turn.is-tool, :scope > .helper-turn.is-run").length,
        thoughts: list.querySelectorAll(".helper-group-thoughts > .is-thinking").length,
        follow: chatFollowMode,
      };
    });
    const menu = async () => {
      await page.locator(".pane-chat .chat-view").click();
      await page.waitForSelector(".composer-menu");
      return page.evaluate(() => ({
        heads: [...document.querySelectorAll(".composer-menu .composer-menu-head")].map((head) => head.textContent),
        rows: [...document.querySelectorAll(".composer-menu .composer-menu-item")].map((row) => ({
          name: row.querySelector(".composer-menu-name").textContent, sub: row.querySelector(".composer-menu-sub")?.textContent ?? "",
          current: row.getAttribute("aria-current") === "true",
        })),
      }));
    };
    const pick = async (words) => {
      await menu();
      await page.locator(".composer-menu .composer-menu-item", { hasText: words }).first().click();
      await page.evaluate(() => window.__PAINTED__());
    };
    const start = await read();
    ok("view: the head wears one named control — 「보기」 and the reading in words — and neither of the two marks it replaced",
      start.name === "보기" && start.word === "기본" && start.tip === "보기 방식 고르기" && start.label === "보기 방식 고르기 — 기본" &&
        start.expanded === "false" && start.marks === 0,
      JSON.stringify(start));
    const listed = await menu();
    ok("view: its menu names the four readings and the two ways a new answer starts, each with a line of what it does, the worn ones ticked",
      listed.heads.join("|") === "보기|새 답이 올 때" &&
        listed.rows.map((row) => row.name).join("|") === "기본|생각 포함|전체|결과만|질문 아래에서 시작|늘 맨 아래 따라가기" &&
        listed.rows.every((row) => row.sub !== "") &&
        listed.rows.filter((row) => row.current).map((row) => row.name).join("|") === "기본|질문 아래에서 시작",
      JSON.stringify(listed));
    await page.keyboard.press("Escape");
    const escaped = await page.evaluate(() => ({ menus: document.querySelectorAll(".composer-menu").length, focus: document.activeElement?.className ?? "" }));
    ok("view: Escape closes the menu and the keyboard is back on the control", escaped.menus === 0 && escaped.focus.includes("chat-view"), JSON.stringify(escaped));

    await pick("전체");
    const all = await read();
    const sets = await page.evaluate(() => window.__VIEW_SETS__);
    ok("view: 「전체」 stands every step as a row, writes the timeline's own setting, and the control's name says the reading it wears now",
      all.word === "전체" && all.label === "보기 방식 고르기 — 전체" && all.timeline === true && all.steps > 0 && sets.join() === "true",
      JSON.stringify({ all, sets }));
    await pick("결과만");
    const answers = await read();
    ok("view: 「결과만」 takes the work out of sight and leaves the timeline's setting as it stood",
      answers.word === "결과만" && answers.answersOnly === "on" && answers.timeline === true, JSON.stringify(answers));
    await pick("생각 포함");
    const thoughts = await read();
    ok("view: 「생각 포함」 folds the work again and stands each turn's thoughts under its line",
      thoughts.word === "생각 포함" && thoughts.answersOnly === null && thoughts.timeline === false &&
        thoughts.pillsShown > 0 && thoughts.steps === 0 && thoughts.thoughts === thoughts.pillsShown,
      JSON.stringify(thoughts));
    await pick("기본");
    const normal = await read();
    ok("view: 「기본」 is the folded page with no thought standing", normal.word === "기본" && normal.thoughts === 0 && normal.pillsShown > 0, JSON.stringify(normal));
    await pick("늘 맨 아래 따라가기");
    const foot = await read();
    await pick("질문 아래에서 시작");
    const question = await read();
    ok("view: where a new answer starts is picked in the same menu", foot.follow === "foot" && question.follow === "question", JSON.stringify({ foot, question }));
    ok("view: no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* A turn's thoughts for the view suite: `afterTurns` with a thought at the head of each turn's work. */
export function thoughtfulTurns() {
  const turns = [];
  for (const turn of afterTurns) {
    turns.push(turn);
    if (turn.role === "user") turns.push({ role: "thinking", text: `**먼저 읽기**\n${turn.text}` });
  }
  return turns.map((turn, message_index) => ({ ...turn, message_index, session_id: "after-session" }));
}

/* A new answer starts under its question: the words the person sends go to the
 * list's top, the answer grows under them without the list moving, and the
 * list follows again once the person is at the words' foot. */
export async function testConversationPin(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    const long = (at) => Array.from({ length: 14 }, (_, line) => `${at}번째 답의 ${line + 1}번째 문단입니다. 목록이 스크롤될 만큼 길어야 합니다.`).join("\n\n");
    const history = [];
    for (let at = 0; at < 4; at += 1) history.push({ role: "user", text: `질문 ${at}` }, { role: "assistant", text: long(at) });
    await openConversation(page, history, { status: "idle" });
    await foldConversations(page);
    await page.evaluate(() => {
      window.__ANSWER__.wire_send = () => null;
    });
    const stand = () => page.evaluate(() => {
      const list = document.querySelector("#worker-view .helper-turns");
      const people = list.querySelectorAll(":scope > .helper-turn.is-user");
      const last = people[people.length - 1];
      const state = list.__follow;
      return {
        top: Math.round(list.scrollTop),
        gap: Math.round(list.scrollHeight - list.scrollTop - list.clientHeight - (state?.room ?? 0)),
        under: Math.round(last.getBoundingClientRect().top - list.getBoundingClientRect().top),
        room: state?.room ?? 0,
        roomWritten: list.style.getPropertyValue("--chat-pin-room"),
        pinned: Boolean(state?.pin),
        door: list.__door?.classList.contains("is-shown") ?? null,
        words: last.querySelector(".helper-said").textContent,
      };
    });
    const send = (words) => page.evaluate(async (words) => {
      const face = document.querySelector("#worker-view");
      const input = face.querySelector(".worker-composer-box");
      input.value = words;
      input.dispatchEvent(new Event("input", { bubbles: true }));
      face.querySelector(".worker-composer").requestSubmit();
      await new Promise((done) => setTimeout(done, 30));
    }, words);
    const arrive = (turns, status) => page.evaluate(async ({ turns, status }) => {
      window.__CONVERSATION__.turns.push(...turns);
      window.__CONVERSATION__.status = status;
      await pollHelperPages();
      await window.__PAINTED__();
      await window.__PAINTED__();
    }, { turns, status });

    const before = await stand();
    await send("다음 부탁");
    const sent = await stand();
    // A paint before the words stand — the poll that has not read them yet — pins nothing: the question
    // that stood before the send is not the one that was sent.
    await arrive([], "working");
    const waited = await stand();
    await arrive([{ role: "user", text: "다음 부탁" }], "working");
    const pinned = await stand();
    ok("pin: sent, the list goes to its foot and an earlier question is not taken for the one sent; once the words stand they go to its top, with room kept under them for the answer and no button back to a foot that holds nothing",
      before.gap <= 1 && sent.gap <= 1 && !waited.pinned && waited.gap <= 1 && waited.room === 0 &&
        pinned.words === "다음 부탁" && pinned.pinned && Math.abs(pinned.under - 12) <= 2 &&
        pinned.room > 0 && pinned.roomWritten === `${pinned.room}px` && pinned.door === false,
      JSON.stringify({ before, sent, waited, pinned }));

    // The answer's first words: shorter than the screen, so room is still kept — less of it — and nothing moved.
    await arrive([{ role: "assistant", text: "먼저 세 줄만 답합니다.\n\n둘째 줄.\n\n셋째 줄." }], "working");
    const short = await stand();
    ok("pin: the answer grows under the words and the list does not move — the room under them shrinks by what the answer took",
      short.top === pinned.top && Math.abs(short.under - 12) <= 2 && short.room > 0 && short.room < pinned.room &&
        short.roomWritten === `${short.room}px` && short.pinned && short.door === false,
      JSON.stringify({ pinned, short }));
    // The turn ends past one screen: its live line goes, the room is gone, and the list still stands on the question.
    await arrive([{ role: "assistant", text: `${long(9)}\n\n${long(10)}` }], "idle");
    const grown = await stand();
    ok("pin: past one screen the room is gone, the list still has not moved, and the button back to the foot stands",
      grown.top === pinned.top && Math.abs(grown.under - 12) <= 2 && grown.room === 0 && grown.roomWritten === "" && grown.gap > 50 && grown.door === true,
      JSON.stringify({ pinned, grown }));

    // Down at the words' foot the person follows them again.
    await page.evaluate(async () => {
      const list = document.querySelector("#worker-view .helper-turns");
      list.scrollTop = list.scrollHeight;
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
    });
    const atFoot = await stand();
    await arrive([{ role: "user", text: "하나 더" }, { role: "assistant", text: long(10) }], "idle");
    const followed = await stand();
    ok("pin: a reader who went down to the words' foot follows them again — words they did not send here pin nothing",
      atFoot.gap <= 1 && !atFoot.pinned && followed.gap <= 1 && !followed.pinned && followed.room === 0,
      JSON.stringify({ atFoot, followed }));

    // Pinned under a short turn, words that came by another road let the pin go: the list follows its foot as before.
    await send("짧게 물음");
    await arrive([{ role: "user", text: "짧게 물음" }, { role: "assistant", text: "짧은 답." }], "idle");
    const shortPin = await stand();
    await arrive([{ role: "user", text: "다른 길로 온 말" }, { role: "assistant", text: long(11) }], "idle");
    const outrun = await stand();
    ok("pin: a pin is its question's — newer words that were not sent from here let it go, and a reader who had the whole turn in view follows the foot",
      shortPin.pinned && shortPin.room > 0 && shortPin.words === "짧게 물음" &&
        !outrun.pinned && outrun.room === 0 && outrun.roomWritten === "" && outrun.gap <= 1 && outrun.words === "다른 길로 온 말",
      JSON.stringify({ shortPin, outrun }));

    // A reader who left for the rows above is not taken to the words they queued.
    await page.evaluate(async () => {
      const list = document.querySelector("#worker-view .helper-turns");
      list.dispatchEvent(new WheelEvent("wheel", { deltaY: -400, bubbles: true }));
      list.scrollTop = 200;
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
    });
    await page.evaluate(() => { document.querySelector("#worker-view .helper-turns").__follow.pinNext = true; });
    const away = await stand();
    await arrive([{ role: "user", text: "위를 읽는 중" }], "working");
    const kept = await stand();
    ok("pin: words that stand while the reader is up the list move nothing", kept.top === away.top && !kept.pinned && kept.room === 0, JSON.stringify({ away, kept }));

    // The terminal's way, one pick away.
    await page.evaluate(() => setChatFollowMode("foot"));
    await arrive([{ role: "assistant", text: "짧은 답" }], "idle");
    await send("바닥으로");
    await arrive([{ role: "user", text: "바닥으로" }], "working");
    // The reader was up the list: the send glides home (`returnToFoot`), and the check waits for the glide's end, not for a time.
    await page.waitForFunction(() => {
      const list = document.querySelector("#worker-view .helper-turns");
      return list.scrollHeight - list.scrollTop - list.clientHeight <= 1;
    });
    const footMode = await stand();
    ok("pin: with 「늘 맨 아래 따라가기」 picked, sent words leave the list at its foot with no room under them",
      footMode.gap <= 1 && !footMode.pinned && footMode.room === 0 && footMode.roomWritten === "", JSON.stringify(footMode));
    await page.evaluate(() => setChatFollowMode("question"));
    ok("pin: no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}
