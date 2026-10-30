/* Finding in a conversation (draft C8, t-82196): ⌘F with a conversation in
 * front opens the file finder's bar over the conversation — not the
 * terminal's behind it — counts 「N/M」 in every turn the page holds, an
 * answer shelved far from view and a step folded in its pill among them,
 * walks with ↑ ↓ (⌘G), opens a pill a hit stands in, paints with the CSS
 * Highlight API and closes on Escape without touching the agent's turn. A
 * pane turned back to its terminal finds in the terminal again.
 *
 * `node ui/tests/conversation-find.mjs [--rounds N]` prints the search's
 * times on the 400-turn page as JSON after the checks. */
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { conversationFixture, openFixtureConversation } from "./conversation-perf.mjs";
import { openPaneConversation } from "./conversation-parity.mjs";
import { chromium, createWindowServer, foldConversations, openWindowTestPage } from "./window-boot.mjs";

/* A search on the 400-turn page answers inside a frame. */
const FIND_FRAME_MS = 16;

/* What the bar and the page show, read in the page. */
const READ_FIND = `
  window.__FIND__ = () => {
    const page = document.querySelector("#worker-view");
    const bar = page?.querySelector(".chat-find") ?? null;
    const list = page?.querySelector(".helper-turns") ?? null;
    const all = CSS.highlights.get("chat-find-match");
    const active = CSS.highlights.get("chat-find-active-match");
    const range = active ? [...active][0] : null;
    const row = range?.startContainer.parentElement?.closest(".helper-turn") ?? null;
    const box = range?.getBoundingClientRect();
    const view = list?.getBoundingClientRect();
    return {
      open: bar !== null && !bar.hidden,
      // The page's own box never scrolls: its head stays where it stood.
      hostScroll: page?.scrollTop ?? null,
      focused: document.activeElement === bar?.querySelector(".file-find-field"),
      count: bar?.querySelector(".file-find-count")?.textContent ?? "",
      painted: all?.size ?? 0,
      active: active?.size ?? 0,
      activeWords: range ? range.toString() : "",
      activeTurn: row ? Number(row.dataset.turn) : null,
      activeInPill: Boolean(range?.startContainer.parentElement?.closest(".helper-group-body")),
      activeInView: Boolean(box && view && box.top >= view.top && box.bottom <= view.bottom),
      openPills: list ? [...list.querySelectorAll(":scope > .helper-group")].filter((pill) => pill.__open).length : 0,
      termBars: [...document.querySelectorAll(".term-find:not(.chat-find)")].filter((one) => !one.hidden).length,
    };
  };
  window.__FIND_TYPE__ = (words) => {
    const field = document.querySelector("#worker-view .chat-find .file-find-field");
    if (!field) return Number.POSITIVE_INFINITY;
    field.value = words;
    const from = performance.now();
    field.dispatchEvent(new Event("input", { bubbles: true }));
    return performance.now() - from;
  };
`;

const percentile = (values, share) => {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.min(sorted.length - 1, Math.floor(share * sorted.length))];
};

export async function testConversationFind(browser, origin, ok, { rounds = 3 } = {}) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await foldConversations(page);
    const turns = conversationFixture();
    await openFixtureConversation(page, turns);
    await page.addScriptTag({ content: READ_FIND });
    const seen = await page.evaluate(async (rounds) => {
      const seen = {};
      const settle = async () => {
        await window.__PAINTED__();
        await window.__PAINTED__();
      };
      // Whatever ends the agent's turn is counted: Escape in the bar must not.
      const stops = [];
      window.__ANSWER__.wire_interrupt = (args) => stops.push(args);
      const list = document.querySelector("#worker-view .helper-turns");
      seen.rows = list.querySelectorAll(":scope > [data-turn]").length;
      // What the page holds: the newest 400 turns of the transcript.
      const held = list.__run.helper.turns;
      seen.answers = held.filter((turn) => turn.role === "assistant" && turn.text.includes("상한에서 지운")).length;
      seen.shelved = list.querySelectorAll(":scope > .is-shelved").length;
      seen.wanted = {
        none: t("file.findNone", "일치 없음"),
        placeholder: t("worker.find", "대화에서 찾기"),
      };
      openFind();
      await settle();
      seen.opened = window.__FIND__();
      seen.placeholder = document.querySelector("#worker-view .chat-find .file-find-field")?.placeholder ?? null;
      // Every answer says this line: the walk opens on the newest of them.
      window.__FIND_TYPE__("상한에서 지운");
      await settle();
      seen.newest = window.__FIND__();
      stepFind(1);
      await settle();
      seen.wrapped = window.__FIND__();
      stepFind(-1);
      await settle();
      seen.back = window.__FIND__();
      // The first answer's own path, far up the page and shelved there.
      const first = [...list.querySelectorAll(":scope > .helper-turn.is-assistant")][0];
      seen.firstShelved = first.classList.contains("is-shelved");
      seen.firstPath = /ui\/shell\.js:\d+/.exec(first.__turn.text)?.[0] ?? "";
      window.__FIND_TYPE__(seen.firstPath);
      await settle();
      seen.far = { ...window.__FIND__(), firstTurn: Number(first.dataset.turn), firstShelvedAfter: first.classList.contains("is-shelved") };
      // A step's line, folded in its pill: the pill opens to it.
      seen.pillsOpenBefore = window.__FIND__().openPills;
      window.__FIND_TYPE__("명령 cargo test -p module_193");
      await settle();
      seen.step = window.__FIND__();
      window.__FIND_TYPE__("이런 말은 어디에도 없다");
      await settle();
      seen.none = window.__FIND__();
      // The bar's Escape is the bar's.
      window.__FIND_TYPE__("상한에서");
      await settle();
      const field = document.querySelector("#worker-view .chat-find .file-find-field");
      field?.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
      await settle();
      seen.closed = { ...window.__FIND__(), stops: stops.length, highlights: CSS.highlights.has("chat-find-match") || CSS.highlights.has("chat-find-active-match") };
      // What a search costs as it is typed: each letter of three queries,
      // `rounds` times over.
      openFind();
      await settle();
      const times = [];
      for (let round = 0; round < rounds; round += 1) {
        for (const query of ["상한에서 지운 행만", "module_19", "ui/shell.js:10", "고치고 시험을"]) {
          for (let end = 1; end <= query.length; end += 1) times.push(window.__FIND_TYPE__(query.slice(0, end)));
          await settle();
        }
      }
      seen.times = times;
      return seen;
    }, rounds);
    const answers = seen.answers;
    ok("C8: ⌘F with a conversation in front opens the conversation's own bar — the file finder's — over the list, focused, saying 「대화에서 찾기」, the page not moved, and no terminal's bar",
      seen.opened.open && seen.opened.focused && seen.opened.termBars === 0 && seen.placeholder === seen.wanted.placeholder && seen.opened.hostScroll === 0,
      JSON.stringify(seen.opened));
    ok("C8: a word every answer says is counted in every answer the page holds and the walk opens on the newest — 「M/M」, painted, the one it stands on in view",
      answers > 0 && seen.newest.count === `${answers}/${answers}` && seen.newest.painted > 0 && seen.newest.active === 1 &&
        seen.newest.activeWords === "상한에서 지운" && seen.newest.activeInView && seen.newest.hostScroll === 0,
      JSON.stringify({ answers, seen: seen.newest }));
    ok("C8: ⌘G walks on and wraps to the first, ⇧⌘G walks back",
      seen.wrapped.count === `1/${answers}` && seen.wrapped.activeInView && seen.back.count === `${answers}/${answers}`,
      JSON.stringify([seen.wrapped, seen.back]));
    ok("C8: a hit in an answer shelved far up the page is found, dressed and brought into view",
      seen.firstShelved && seen.far.count === "1/1" && seen.far.activeTurn === seen.far.firstTurn && !seen.far.firstShelvedAfter &&
        seen.far.activeWords === seen.firstPath && seen.firstPath !== "" && seen.far.activeInView && seen.far.hostScroll === 0,
      JSON.stringify({ firstShelved: seen.firstShelved, far: seen.far }));
    ok("C8: a hit in a step folded in its pill opens the pill and stands on the step's line — across the line's kind and its target",
      seen.step.count === "1/1" && seen.step.activeInPill && seen.step.activeWords === "명령 cargo test -p module_193" &&
        seen.step.openPills === seen.pillsOpenBefore + 1 && seen.step.activeInView,
      JSON.stringify({ before: seen.pillsOpenBefore, step: seen.step }));
    ok("C8: words nowhere in the conversation say 「일치 없음」 and paint nothing",
      seen.none.count === seen.wanted.none && seen.none.painted === 0 && seen.none.active === 0, JSON.stringify(seen.none));
    ok("C8: Escape in the bar closes it and takes its paint away — and does not stop the agent's turn",
      !seen.closed.open && !seen.closed.highlights && seen.closed.stops === 0, JSON.stringify(seen.closed));
    const p95 = percentile(seen.times, 0.95);
    ok(`C8: on the 400-turn page a search answers within a frame as it is typed — p95 ${p95.toFixed(2)} ms < ${FIND_FRAME_MS} ms over ${seen.times.length} letters`,
      seen.rows >= 100 && p95 < FIND_FRAME_MS, JSON.stringify({ rows: seen.rows, shelved: seen.shelved, p95, max: Math.max(...seen.times) }));
    ok("C8: the 400-turn page raised no errors", faults.length === 0, faults.join("\n"));
    return { rows: seen.rows, shelved: seen.shelved, letters: seen.times.length, p50: percentile(seen.times, 0.5), p95, max: Math.max(...seen.times) };
  } finally {
    await page.close();
  }
}

/* A pane turned to its conversation: ⌘F finds in the conversation; turned
 * back to its terminal, ⌘F finds in the terminal. */
export async function testConversationFindPane(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await openPaneConversation(page, [
      { role: "user", text: "로그에서 오류를 찾아 줘" },
      { role: "assistant", text: "오류는 두 줄입니다." },
    ]);
    const seen = await page.evaluate(async () => {
      const settle = async () => {
        await window.__PAINTED__();
        await window.__PAINTED__();
      };
      const tab = currentTab();
      const term = activePaneOf(tab);
      const chat = () => document.querySelector(`.pane-chat[data-term="${term}"] .chat-find`);
      const termBar = () => termViews.get(term)?.host.querySelector(".term-find:not(.chat-find)") ?? null;
      const seen = { kind: tab?.kind, on: paneChatOn(term) };
      openFind();
      await settle();
      seen.chatOpen = Boolean(chat() && !chat().hidden);
      seen.termOpenWhileChat = Boolean(termBar() && !termBar().hidden);
      const field = chat()?.querySelector(".file-find-field");
      if (field) {
        field.value = "오류";
        field.dispatchEvent(new Event("input", { bubbles: true }));
      }
      await settle();
      seen.count = chat()?.querySelector(".file-find-count")?.textContent ?? "";
      field?.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
      await setPaneChat(term, false);
      await settle();
      openFind();
      await settle();
      seen.termOpenAfter = Boolean(termBar() && !termBar().hidden);
      return seen;
    });
    ok("C8: a pane turned to its conversation finds in the conversation — not in the terminal behind it — and turned back finds in the terminal again",
      seen.kind === "term" && seen.on && seen.chatOpen && !seen.termOpenWhileChat && seen.count === "2/2" && seen.termOpenAfter,
      JSON.stringify(seen));
    ok("C8: the pane's find raised no errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const roundsAt = process.argv.indexOf("--rounds");
  const rounds = roundsAt >= 0 ? Number(process.argv[roundsAt + 1]) : 3;
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  let failed = 0;
  const report = (name, pass, details = "") => {
    console.log(`${pass ? "PASS" : "FAIL"} ${name}${!pass ? `\n  ${details}` : ""}`);
    if (!pass) failed++;
  };
  try {
    const times = await testConversationFind(browser, origin, report, { rounds });
    await testConversationFindPane(browser, origin, report);
    console.log(JSON.stringify({ find: times }));
  } finally {
    await browser.close();
    files.close();
  }
  if (failed) process.exitCode = 1;
}
