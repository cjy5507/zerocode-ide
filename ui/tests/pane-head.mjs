import { openWindowTestPage } from "./window-boot.mjs";

/* The centre pane, as the passed draft p-6893560125c34552 reads it (IDE design
 * pass 2, B: items 8, 9 and 11).
 *
 *  - A terminal tab whose pane runs an agent wears a head of its own, above its
 *    panes: the agent's name, its model, what it is doing, the line a resumed
 *    conversation names (t-68069's place, `paintPaneHeadPick`) and the
 *    터미널 | 대화 switch — beside the pane it speaks for, not in the title bar.
 *  - The pane reads as its conversation by default: the transcript view, no
 *    hand-over to a wire. The person's pick of the screen is theirs and is not
 *    undone by the next hook event; a question only on the screen still brings
 *    the screen back.
 *  - The answer reads as prose: Geist 16 px on a 24 px line in one 760 px column,
 *    code in a fixed-width chip; the composer floats under that column.
 *
 *   WINDOW_SUITES=pane-head node ui/tests/window.mjs */

const TRANSCRIPT = [
  { role: "user", text: "일정 탭을 B로 바꿔 줘" },
  { role: "assistant", text: "보고서와 사진은 `REPORT.md`에 있습니다. 다음은 게이트입니다." },
];

export async function testPaneHead(browser, origin, ok) {
  const { page } = await openWindowTestPage(browser, origin);
  try {
    await page.setViewportSize({ width: 1700, height: 1000 });
    const seen = await page.evaluate(async (transcript) => {
      const tell = (name, payload) => {
        for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload });
      };
      const settle = () => new Promise((done) => setTimeout(done, 80));
      window.__ANSWER__.pane_log = (args) => ({ found: true, next: transcript.length, turns: transcript.slice(args.after ?? 0) });
      const wires = [];
      window.__ANSWER__.wire_start = (args) => { wires.push(args); return null; };
      const seen = {};
      const term = await openTermTab({ placement: "tab" });
      await window.__PAINTED__();
      const host = document.querySelector(`.pane-slot[data-term="${term}"]`)?.closest(".panes");
      seen.headBeforeAgent = Boolean(host?.querySelector(":scope > .pane-head:not([hidden])"));
      tell("hook:agent", { term, state: "working", agent: "claude", session: "s-head-0001", resumable: true, model: "claude-fable-5-1" });
      await window.__PAINTED__();
      await settle();
      const head = host.querySelector(":scope > .pane-head");
      seen.headFirst = host.firstElementChild === head && !head.hidden;
      seen.toggleInHead = head.contains(el("view-toggle")) && !el("view-toggle").hidden;
      seen.toggleOffTitlebar = !document.querySelector(".titlebar").contains(el("view-toggle"));
      seen.pickSlot = Boolean(head.querySelector("#pane-head-pick, .pane-head-pick"));
      seen.pick = head.querySelector(".pane-head-pick")?.textContent ?? "";
      seen.model = head.querySelector(".pane-head-model")?.textContent ?? "";
      seen.state = head.querySelector(".pane-head-state")?.textContent ?? "";
      // The conversation stands by default — read, not handed to a wire.
      seen.chatByDefault = paneChatOn(term);
      seen.chatPressed = el("view-toggle-chat").getAttribute("aria-pressed");
      seen.wires = wires.length;
      const chat = document.querySelector(`.pane-slot[data-term="${term}"] .pane-chat`);
      seen.chatShown = Boolean(chat) && !chat.hidden;
      // In the conversation the page's own head says who; the pane's head keeps the switch.
      seen.whoHiddenInChat = getComputedStyle(head.querySelector(".pane-head-who")).display === "none";
      // Prose: 16 px on 24, in one column of at most 760 px; code is a fixed-width chip.
      await new Promise((done) => setTimeout(done, 300));
      const said = chat?.querySelector(".helper-turn.is-assistant .helper-said");
      const prose = said ? getComputedStyle(said) : null;
      seen.proseSize = prose?.fontSize;
      seen.proseLine = prose?.lineHeight;
      const column = chat?.querySelector(".helper-turns");
      const turn = chat?.querySelector(".helper-turn.is-assistant");
      seen.turnWidth = Math.round(turn?.getBoundingClientRect().width ?? 0);
      seen.columnWidth = Math.round(column?.getBoundingClientRect().width ?? 0);
      const code = said?.querySelector("code");
      seen.codeFont = code ? getComputedStyle(code).fontFamily : "";
      seen.codeGround = code ? getComputedStyle(code).backgroundColor : "";
      const dock = chat?.querySelector(".worker-composer");
      const dockBox = dock?.getBoundingClientRect();
      const turnBox = turn?.getBoundingClientRect();
      seen.dockRadius = dock ? getComputedStyle(dock).borderTopLeftRadius : "";
      seen.dockAligned = dockBox && turnBox ? Math.abs(dockBox.left - turnBox.left) <= 1 && dockBox.width <= 761 : false;
      // The person picks the screen: it stays theirs through the next hook event.
      el("view-toggle-term").click();
      await settle();
      tell("hook:agent", { term, state: "working", agent: "claude", session: "s-head-0001", resumable: true });
      await window.__PAINTED__();
      seen.screenKept = !paneChatOn(term);
      seen.whoShownOnScreen = getComputedStyle(head.querySelector(".pane-head-who")).display !== "none";
      // A second agent pane whose question is only on its screen opens on the screen.
      const other = await openTermTab({ placement: "tab" });
      await window.__PAINTED__();
      tell("hook:agent", { term: other, state: "needs-attention", agent: "codex", session: "s-head-0002", resumable: false });
      await window.__PAINTED__();
      seen.askOnScreen = !paneChatOn(other);
      return seen;
    }, TRANSCRIPT);
    ok(
      "an agent's terminal tab wears its own head above its panes, with the model, the state, the resumed conversation's line and the view switch",
      !seen.headBeforeAgent && seen.headFirst && seen.toggleInHead && seen.toggleOffTitlebar && seen.pickSlot &&
        seen.pick.includes("s-head-0001".slice(-4)) && seen.model.includes("claude-fable-5-1") && seen.state.length > 0,
      JSON.stringify(seen),
    );
    ok(
      "the pane reads as its conversation by default, without a hand-over, and the person's pick of the screen survives the next hook",
      seen.chatByDefault && seen.chatPressed === "true" && seen.wires === 0 && seen.chatShown &&
        seen.whoHiddenInChat && seen.screenKept && seen.whoShownOnScreen && seen.askOnScreen,
      JSON.stringify(seen),
    );
    ok(
      "the answer reads at Geist 16 on 24 in one column of at most 760 px, code in a fixed-width chip, and the composer floats under that column",
      seen.proseSize === "16px" && seen.proseLine === "24px" && seen.turnWidth > 0 && seen.turnWidth <= 760 &&
        seen.columnWidth > 760 && /mono|Menlo|SF Mono/i.test(seen.codeFont) && seen.codeGround !== "rgba(0, 0, 0, 0)" &&
        seen.dockRadius === "12px" && seen.dockAligned,
      JSON.stringify(seen),
    );
  } finally { await page.close(); }
}
