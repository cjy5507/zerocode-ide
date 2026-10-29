import { createRequire } from "node:module";
import { openWindowTestPage } from "./window-boot.mjs";
import { axeViolations } from "./settings-quality.mjs";

/* 「당신 차례」 — the chip, the list that drops from it, and the group at the top
 * of the sidebar (IDE design pass 2, B: items 1, 4 and 14 of the passed draft
 * p-6893560125c34552).
 *
 * One number: the chip reads the attention column the dock badge reads
 * (`answer.attention_count`'s cards), never a count of its own. An answer
 * travels the roads a board card already takes — `answer_ask`, `answer_approval`,
 * both through the backend's answer door, which types only while the pane still
 * shows the question — and a question the program asked in prose goes in as the
 * composer's own words, but only to a pane that is not mid-turn. A refused
 * answer is said in the list, beside the question, and is not counted as sent.
 *
 *   WINDOW_SUITES=your-turn node ui/tests/window.mjs */

export const CHIP_WORDS = Object.freeze({
  ko: "당신 차례", en: "Your turn", ja: "あなたの番", zh: "轮到你", es: "Tu turno",
});

/* The attention column as the backend's grouping returns it: a question with
 * choices, a permission, and a question asked in prose. */
export const installTurns = () => {
  const ago = (minutes) => Date.now() - minutes * 60_000;
  window.__turnCards = () => [
    { pane: "term:6", agent: "zo", state: "needs-attention", heading: "한국어설명",
      worktree: "main", project: "/p", task: "", ask: "어느 쪽으로 이어 갈까요?",
      ask_prompt: { questions: [{ question: "어느 쪽으로 이어 갈까요?", multi_select: false,
        options: [{ label: "짧은 판으로", description: "600자" }, { label: "긴 판으로" }, { label: "둘 다 보여 줘" }] }] },
      unseen: true, changed_at: ago(1), at: ago(1) },
    { pane: "term:7", agent: "claude", state: "needs-attention", heading: "Agent Teams",
      worktree: "main", project: "/p", task: "", ask: "Bash · npm install",
      approval: { tool: "Bash", summary: "npm install" },
      unseen: true, changed_at: ago(2), at: ago(2) },
    { pane: "term:8", agent: "codex", state: "needs-attention", heading: "다음 과업",
      worktree: "main", project: "/p", task: "", ask: "다음 과업으로 넘어갈까요?",
      unseen: true, changed_at: ago(3), at: ago(3) },
  ];
  window.__setTurns = async (cards) => {
    window.__COLUMNS__ = [{ bucket: "attention", cards }];
    await refreshBoardBadge();
  };
  window.__chip = () => document.getElementById("your-turn");
  window.__list = () => document.getElementById("your-turn-list");
  window.__items = () => [...(window.__list()?.querySelectorAll(".your-turn-item") ?? [])];
  window.__group = () => document.getElementById("your-turn-group");
  window.__press = (key, extra = {}) => {
    const target = document.activeElement ?? document.body;
    target.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...extra }));
  };
  window.__settle = () => new Promise((done) => requestAnimationFrame(() => setTimeout(done, 0)));
  window.__pasted = [];
  window.__keys = [];
  window.__ANSWER__.term_paste = (args) => { window.__pasted.push(JSON.parse(JSON.stringify(args))); return null; };
  window.__ANSWER__.term_key = (args) => { window.__keys.push(JSON.parse(JSON.stringify(args))); return null; };
};

export async function testYourTurn(browser, origin, ok) {
  const { page } = await openWindowTestPage(browser, origin);
  try {
    await page.evaluate(installTurns);

    /* The chip: grey at nothing, blue with the count when something waits. */
    const chip = await page.evaluate(async () => {
      const seen = {};
      await window.__setTurns([]);
      const node = window.__chip();
      seen.exists = node !== null;
      if (!node) return seen;
      seen.idleText = node.textContent.replace(/\s+/g, " ").trim();
      seen.idleLive = node.classList.contains("is-live");
      seen.idleColor = getComputedStyle(node.querySelector(".your-turn-count")).color;
      seen.idleGround = getComputedStyle(node).backgroundColor;
      const idleChevron = node.querySelector(".your-turn-chevron");
      seen.idleChevron = Boolean(idleChevron) && getComputedStyle(idleChevron).display !== "none";
      await window.__setTurns(window.__turnCards());
      seen.liveText = node.textContent.replace(/\s+/g, " ").trim();
      seen.live = node.classList.contains("is-live");
      const probe = document.createElement("span");
      probe.style.color = "var(--ink-on-solid)";
      probe.style.background = "var(--signal-you-solid)";
      document.body.appendChild(probe);
      seen.onSolid = getComputedStyle(probe).color;
      seen.solid = getComputedStyle(probe).backgroundColor;
      probe.remove();
      // The colour eases in over the shared motion pair; read it once it has.
      await new Promise((done) => setTimeout(done, 250));
      // The draft's lit chip: a filled you-blue pill, its words and count white, a chevron down.
      seen.liveColor = getComputedStyle(node.querySelector(".your-turn-count")).color;
      seen.liveGround = getComputedStyle(node).backgroundColor;
      const chevron = node.querySelector(".your-turn-chevron");
      seen.chevron = Boolean(chevron) && getComputedStyle(chevron).display !== "none" && chevron.getBoundingClientRect().width > 0;
      // A shape beside the colour: a ring at nothing, a filled dot when something waits.
      seen.dotFilled = getComputedStyle(node.querySelector(".your-turn-dot")).backgroundColor !== "rgba(0, 0, 0, 0)";
      seen.leftOfTabs = node.getBoundingClientRect().right <= document.getElementById("tabstrip").getBoundingClientRect().left + 1;
      seen.listBuilt = (window.__list()?.childElementCount ?? 0);
      return seen;
    });
    ok(
      "the chip reads the attention column: a grey ring and 0 at nothing, a filled you-blue pill with white words, the count and a chevron when three wait",
      chip.exists && chip.idleText.endsWith("0") && !chip.idleLive && chip.idleColor !== chip.onSolid &&
        chip.idleGround !== chip.solid && !chip.idleChevron &&
        chip.live && chip.liveText.endsWith("3") && chip.liveColor === chip.onSolid && chip.liveGround === chip.solid &&
        chip.chevron && chip.dotFilled && chip.leftOfTabs && chip.listBuilt === 0,
      JSON.stringify(chip),
    );

    /* The list: built when it opens, emptied when it closes, keyboard all the way. */
    const list = await page.evaluate(async () => {
      const seen = {};
      window.__chip().click();
      await window.__settle();
      const host = window.__list();
      seen.open = host && !host.hidden;
      seen.expanded = window.__chip().getAttribute("aria-expanded");
      seen.items = window.__items().map((item) => item.dataset.kind);
      seen.options = [...window.__items()[0].querySelectorAll(".board-ask-option")].map((one) => one.textContent.replace(/\s+/g, " ").trim());
      seen.focusInList = host.contains(document.activeElement);
      // The keys say as many choices as the questions hold, not a fixed 1–9.
      seen.keys = host.querySelector(".your-turn-keys")?.textContent ?? "";
      window.__press("ArrowDown");
      seen.afterDown = document.activeElement?.closest(".your-turn-item")?.dataset.kind ?? null;
      window.__press("ArrowUp");
      seen.afterUp = document.activeElement?.closest(".your-turn-item")?.dataset.kind ?? null;
      window.__press("Escape");
      await window.__settle();
      seen.closedHidden = host.hidden;
      seen.closedChildren = host.childElementCount;
      seen.focusBack = document.activeElement === window.__chip();
      return seen;
    });
    ok(
      "the list opens under the chip with a question, a permission and a prose question, walks with ↑↓, and Esc empties it and gives the focus back",
      list.open && list.expanded === "true" && list.items.join(",") === "question,permission,question" &&
        list.options.length === 3 && list.options[0].startsWith("1") && list.options[0].includes("짧은 판으로") &&
        list.focusInList && list.keys.includes("1–3") && !list.keys.includes("1–9") && list.afterDown === "permission" && list.afterUp === "question" &&
        list.closedHidden && list.closedChildren === 0 && list.focusBack,
      JSON.stringify(list),
    );

    /* A number answers the question through the answer door, and the chip counts down. */
    const answered = await page.evaluate(async () => {
      const seen = {};
      window.__ANSWERED__ = null;
      window.__ANSWER_REFUSES__ = null;
      window.__chip().click();
      await window.__settle();
      window.__items()[0].focus();
      window.__press("2");
      await window.__settle();
      await window.__settle();
      seen.sent = window.__ANSWERED__;
      seen.count = window.__chip().querySelector(".your-turn-count").textContent;
      seen.items = window.__items().map((item) => item.dataset.kind);
      return seen;
    });
    ok(
      "pressing 2 on a question sends its second choice to that pane through answer_ask, and the chip counts one fewer",
      answered.sent?.term === 6 && answered.sent?.prompt?.questions?.[0]?.question === "어느 쪽으로 이어 갈까요?" &&
        answered.sent?.selections?.[0]?.indices?.join(",") === "1" &&
        answered.count === "2" && answered.items.join(",") === "permission,question",
      JSON.stringify(answered),
    );

    /* A refused allow is said in the list, and the turn stays counted. */
    const refused = await page.evaluate(async () => {
      const seen = {};
      window.__APPROVED__ = null;
      window.__ANSWER_REFUSES__ = "question-changed";
      window.__items()[0].focus();
      window.__press("Enter", { metaKey: true });
      await window.__settle();
      await window.__settle();
      seen.sent = window.__APPROVED__;
      seen.said = window.__items()[0]?.querySelector(".your-turn-refusal")?.textContent ?? "";
      seen.count = window.__chip().querySelector(".your-turn-count").textContent;
      window.__ANSWER_REFUSES__ = null;
      window.__APPROVED__ = null;
      window.__items()[0].focus();
      window.__press("Enter", { metaKey: true });
      await window.__settle();
      await window.__settle();
      seen.allowed = window.__APPROVED__;
      seen.countAfter = window.__chip().querySelector(".your-turn-count").textContent;
      return seen;
    });
    ok(
      "⌘↵ allows a permission through answer_approval; a refusal is said beside it and keeps it counted, and the allow that lands counts it off",
      refused.sent?.term === 7 && refused.sent?.allow === true &&
        refused.said.includes("이 질문은 이미 지나갔습니다") && refused.count === "2" &&
        refused.allowed?.term === 7 && refused.allowed?.allow === true && refused.countAfter === "1",
      JSON.stringify(refused),
    );

    /* A prose question: words go in only to a pane that can take them now. */
    const prose = await page.evaluate(async () => {
      const seen = {};
      hookStates.set(8, "working");
      const item = window.__items()[0];
      const field = item.querySelector(".your-turn-field");
      field.value = "네, 넘어가 주세요";
      field.dispatchEvent(new Event("input", { bubbles: true }));
      item.querySelector(".your-turn-send").click();
      await window.__settle();
      seen.busyPasted = window.__pasted.length;
      seen.busySaid = window.__items()[0]?.querySelector(".your-turn-refusal")?.textContent ?? "";
      seen.busyCount = window.__chip().querySelector(".your-turn-count").textContent;
      hookStates.set(8, "needs-attention");
      window.__items()[0].querySelector(".your-turn-send").click();
      await new Promise((done) => setTimeout(done, COMPOSER_SUBMIT_DELAY_MS + 100));
      seen.pasted = window.__pasted.map((one) => [one.term, one.text]);
      seen.entered = window.__keys.filter((one) => one.term === 8 && one.press?.key === "Enter").length;
      seen.count = window.__chip().querySelector(".your-turn-count").textContent;
      return seen;
    });
    ok(
      "a prose question is not typed into a pane mid-turn — the list says it cannot take words now — and goes in as paste and Enter once the pane waits",
      prose.busyPasted === 0 && prose.busySaid.length > 0 && prose.busyCount === "1" &&
        JSON.stringify(prose.pasted) === JSON.stringify([[8, "네, 넘어가 주세요"]]) &&
        prose.entered === 1 && prose.count === "0",
      JSON.stringify(prose),
    );

    /* Later: N puts a turn away for ten minutes, and the chip stops counting it. */
    const later = await page.evaluate(async () => {
      const seen = {};
      window.__press("Escape");
      // The panes moved on from the turns answered above, and asked again.
      await window.__setTurns([]);
      await window.__setTurns(window.__turnCards());
      window.__chip().click();
      await window.__settle();
      window.__items()[1].focus();
      window.__press("n");
      await window.__settle();
      seen.items = window.__items().map((item) => item.dataset.kind);
      seen.count = window.__chip().querySelector(".your-turn-count").textContent;
      seen.until = Math.round((yourTurnLater.get(yourTurnKey(window.__turnCards()[1])) - Date.now()) / 60_000);
      window.__press("Escape");
      yourTurnLater.clear();
      return seen;
    });
    ok(
      "N puts a turn away for ten minutes: the list and the chip stop counting it",
      later.items.join(",") === "question,question" && later.count === "2" && later.until === 10,
      JSON.stringify(later),
    );

    /* The sidebar group: there while something waits, gone at nothing. */
    await page.evaluate(() => window.__setTurns(window.__turnCards()));
    // No control inside another: a row is not a button around its allow and deny.
    const axeModule = createRequire(import.meta.url)("@axe-core/playwright");
    const groupViolations = await axeViolations(page, axeModule.default ?? axeModule, "#your-turn-group")
      .catch((error) => [String(error)]);
    const group = await page.evaluate(async () => {
      const seen = {};
      const host = window.__group();
      seen.shown = host && !host.hidden;
      seen.rows = [...(host?.querySelectorAll(".your-turn-row") ?? [])].map((row) => row.dataset.kind);
      // The draft's group: its head says how long the oldest has waited; a row opens its
      // conversation itself, and only a permission wears buttons — allow and deny.
      seen.head = host?.querySelector(".your-turn-group-head")?.parentElement?.textContent.replace(/\s+/g, " ") ?? "";
      seen.openButtons = host?.querySelectorAll(".your-turn-open").length ?? -1;
      seen.rowButtons = [...(host?.querySelectorAll(".your-turn-row") ?? [])].map((row) => row.querySelectorAll("button").length);
      seen.rowRoles = [...(host?.querySelectorAll(".your-turn-row") ?? [])].map((row) => row.getAttribute("role"));
      const opened = [];
      const real = openBoardConversation;
      openBoardConversation = async (card) => { opened.push(card.pane); };
      host.querySelector(".your-turn-row[data-kind=question] .your-turn-row-name")?.click();
      host.querySelector(".your-turn-row[data-kind=permission]")?.click();
      openBoardConversation = real;
      seen.opened = opened;
      seen.aboveProjects = host && host.compareDocumentPosition(document.getElementById("sidebar-title")) & Node.DOCUMENT_POSITION_FOLLOWING;
      window.__APPROVED__ = null;
      host.querySelector(".your-turn-row[data-kind=permission] .your-turn-deny")?.click();
      await window.__settle();
      seen.denied = window.__APPROVED__;
      await window.__setTurns([]);
      seen.goneHidden = host.hidden;
      seen.goneChildren = host.childElementCount;
      return seen;
    });
    ok(
      "the sidebar group nests no control in another (axe), says the oldest wait, and a row opens its conversation with only allow and deny as buttons",
      groupViolations.length === 0 && group.head.includes("가장 오래") && group.openButtons === 0 &&
        group.rowButtons.join(",") === "1,3,1" && group.rowRoles.every((role) => role !== "button") &&
        group.opened.join(",") === "term:6,term:7",
      JSON.stringify({ groupViolations, ...group }),
    );
    ok(
      "the sidebar's top group lists the agents waiting, answers a permission from its row, and leaves nothing behind at nothing",
      group.shown && group.rows.join(",") === "question,permission,question" && Boolean(group.aboveProjects) &&
        group.denied?.term === 7 && group.denied?.allow === false && group.goneHidden && group.goneChildren === 0,
      JSON.stringify(group),
    );

    /* Five languages, light, and motion that stands still when asked. */
    const words = await page.evaluate(async (expected) => {
      const seen = {};
      for (const code of Object.keys(expected)) {
        setLocale(code);
        seen[code] = window.__chip().querySelector(".your-turn-label").textContent.trim();
      }
      setLocale("ko");
      return seen;
    }, CHIP_WORDS);
    ok(
      "the chip speaks all five languages",
      Object.entries(CHIP_WORDS).every(([code, word]) => words[code] === word),
      JSON.stringify(words),
    );

    await page.emulateMedia({ reducedMotion: "reduce", colorScheme: "light" });
    const still = await page.evaluate(async () => {
      document.documentElement.dataset.theme = "light";
      await window.__setTurns(window.__turnCards());
      const node = window.__chip();
      const probe = document.createElement("span");
      probe.style.background = "var(--signal-you-solid)";
      document.body.appendChild(probe);
      const blue = getComputedStyle(probe).backgroundColor;
      probe.remove();
      node.click();
      await window.__settle();
      const seen = {
        blue,
        color: await new Promise((done) => setTimeout(() => done(getComputedStyle(node).backgroundColor), 250)),
        motion: getComputedStyle(node).transitionDuration,
        listMotion: getComputedStyle(window.__list()).transitionDuration,
      };
      window.__items()[0].focus();
      return seen;
    });
    // A real key, so the ring is the keyboard's: ↓ walks to the next turn.
    await page.keyboard.press("ArrowDown");
    still.focusRing = await page.evaluate(() => {
      const ring = getComputedStyle(document.activeElement).outlineStyle;
      window.__press("Escape");
      delete document.documentElement.dataset.theme;
      return ring;
    });
    ok(
      "in light the chip keeps the you-blue, motion stands still when reduced, and a focused turn wears the ring",
      still.color === still.blue && /^0s(, 0s)*$/.test(still.motion) && /^0s(, 0s)*$/.test(still.listMotion) &&
        still.focusRing !== "none",
      JSON.stringify(still),
    );
  } finally { await page.close(); }
}
