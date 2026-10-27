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
      const reads = new Map();
      window.__ANSWER__.pane_log = (args) => {
        reads.set(args.term, (reads.get(args.term) ?? 0) + 1);
        return { found: true, next: transcript.length, turns: transcript.slice(args.after ?? 0) };
      };
      const wires = [];
      window.__ANSWER__.wire_start = (args) => { wires.push(args); return null; };
      const seen = {};
      const term = await openTermTab({ placement: "tab" });
      await window.__PAINTED__();
      const host = document.querySelector(`.pane-slot[data-term="${term}"]`)?.closest(".panes");
      seen.headBeforeAgent = Boolean(host?.querySelector(":scope > .pane-head:not([hidden])"));
      tell("hook:agent", { term, state: "working", agent: "claude", session: { id: "s-head-0001" }, resumable: true, model: "claude-fable-5-1" });
      await window.__PAINTED__();
      await settle();
      // The head stands first in the tab; its line (the conversation's line and the
      // switch) travels to the conversation's own head while that stands.
      const head = host.querySelector(":scope > .pane-head");
      const line = head.__line;
      seen.headFirst = host.firstElementChild === head;
      seen.toggleInHead = line.contains(el("view-toggle")) && !el("view-toggle").hidden;
      seen.toggleOffTitlebar = !document.querySelector(".titlebar").contains(el("view-toggle"));
      seen.pickSlot = Boolean(line.querySelector(".pane-head-pick"));
      seen.pick = line.querySelector(".pane-head-pick")?.textContent ?? "";
      seen.model = head.querySelector(".pane-head-model")?.textContent ?? "";
      seen.state = head.querySelector(".pane-head-state")?.textContent ?? "";
      // The conversation stands by default — read, not handed to a wire.
      seen.chatByDefault = paneChatOn(term);
      seen.chatPressed = el("view-toggle-chat").getAttribute("aria-pressed");
      seen.wires = wires.length;
      const chat = document.querySelector(`.pane-slot[data-term="${term}"] .pane-chat`);
      seen.chatShown = Boolean(chat) && !chat.hidden;
      // One head line in the conversation (the draft's item 8): the page's own head says who, its
      // model and state, and wears the pane head's conversation line and the switch beside them.
      // A conversation set behind its pane's screen is clipped to nothing (`.is-behind`).
      const shown = (node) => Boolean(node) && !node.closest(".is-behind") && node.getClientRects().length > 0 &&
        node.getBoundingClientRect().height > 0;
      const lines = [...host.querySelectorAll(".pane-head, .chat-head")].filter(shown);
      seen.headLinesInChat = lines.length;
      seen.oneLineHolds = lines.length === 1 && lines[0].contains(el("view-toggle")) &&
        Boolean(lines[0].querySelector(".pane-head-pick")) && lines[0].textContent.includes("claude-fable-5-1");
      // The switch reads 대화 | 터미널, the draft's order.
      seen.switchOrder = [...el("view-toggle").querySelectorAll("button")].map((button) => button.id).join(",");
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
      seen.dockRadius = dock ? getComputedStyle(dock).borderBottomLeftRadius : "";
      seen.dockAligned = dockBox && turnBox ? Math.abs(dockBox.left - turnBox.left) <= 1 && dockBox.width <= 761 : false;
      // A session that asks nothing says so by a triangle and the CLI's word, and no colour:
      // red is the window's word for a failure.
      tell("hook:agent", { term, state: "idle", agent: "claude", session: { id: "s-head-0001" }, resumable: true, permission_mode: "bypassPermissions" });
      await window.__PAINTED__();
      await settle();
      const mode = dock?.querySelector(".worker-composer-mode");
      seen.modeShown = Boolean(mode) && !mode.hidden;
      seen.modeMark = mode?.querySelector("use")?.getAttribute("href") ?? "";
      seen.modeWords = mode?.textContent ?? "";
      const probe = document.createElement("span");
      probe.style.background = "var(--signal-halt)";
      dock.appendChild(probe);
      const halt = getComputedStyle(probe).backgroundColor;
      probe.style.background = "var(--chat-send-bg)";
      const send = getComputedStyle(probe).backgroundColor;
      probe.remove();
      // Read once the send has eased from the stop's colour to its own.
      await new Promise((done) => setTimeout(done, 400));
      const sendButton = dock?.querySelector(".worker-composer-send:not(.is-stop)");
      seen.sendGround = sendButton ? getComputedStyle(sendButton).backgroundColor : "";
      seen.sendNotHalt = seen.sendGround !== halt && seen.sendGround === send;
      seen.modeInk = mode ? getComputedStyle(mode).color : "";
      seen.modeNotHalt = seen.modeInk !== halt;
      // The person picks the screen: it stays theirs through the next hook event.
      el("view-toggle-term").click();
      await settle();
      tell("hook:agent", { term, state: "working", agent: "claude", session: { id: "s-head-0001" }, resumable: true });
      await window.__PAINTED__();
      seen.screenKept = !paneChatOn(term);
      seen.whoShownOnScreen = getComputedStyle(head.querySelector(".pane-head-who")).display !== "none";
      const screenLines = [...host.querySelectorAll(".pane-head, .chat-head")].filter(shown);
      seen.headLinesOnScreen = screenLines.length;
      seen.screenLineHolds = screenLines[0] === head && head.contains(el("view-toggle")) && Boolean(head.querySelector(".pane-head-pick"));
      // A second agent pane whose question is only on its screen opens on the screen.
      const other = await openTermTab({ placement: "tab" });
      await window.__PAINTED__();
      tell("hook:agent", { term: other, state: "needs-attention", agent: "codex", session: { id: "s-head-0002" }, resumable: false });
      await window.__PAINTED__();
      seen.askOnScreen = !paneChatOn(other);
      // What a conversation standing by default costs at rest: a pane whose agent waits is read
      // when a hook says something moved, not once a second; one at work is followed.
      const quiet = await openTermTab({ placement: "tab" });
      await window.__PAINTED__();
      tell("hook:agent", { term: quiet, state: "idle", agent: "claude", session: { id: "s-head-0003" }, resumable: true });
      await window.__PAINTED__();
      // The read the page owes at once, and the one quick follow after news (160 ms), are over.
      const settled = () => new Promise((done) => setTimeout(done, 400));
      await settled();
      seen.quietChat = paneChatOn(quiet);
      const atRest = reads.get(quiet) ?? 0;
      await new Promise((done) => setTimeout(done, 3200));
      seen.quietReads = (reads.get(quiet) ?? 0) - atRest;
      tell("hook:agent", { term: quiet, state: "working", agent: "claude", session: { id: "s-head-0003" }, resumable: true });
      await window.__PAINTED__();
      const atWork = reads.get(quiet) ?? 0;
      await new Promise((done) => setTimeout(done, 2600));
      seen.workingReads = (reads.get(quiet) ?? 0) - atWork;
      tell("hook:agent", { term: quiet, state: "idle", agent: "claude", session: { id: "s-head-0003" }, resumable: true });
      await window.__PAINTED__();
      await settled();
      const atEnd = reads.get(quiet) ?? 0;
      await new Promise((done) => setTimeout(done, 2200));
      seen.endedReads = (reads.get(quiet) ?? 0) - atEnd;
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
        seen.screenKept && seen.whoShownOnScreen && seen.askOnScreen,
      JSON.stringify(seen),
    );
    ok(
      "the conversation wears one head line holding who, the model, the conversation's line and the 대화 | 터미널 switch; the screen wears the pane's",
      seen.headLinesInChat === 1 && seen.oneLineHolds && seen.switchOrder === "view-toggle-chat,view-toggle-term" &&
        seen.headLinesOnScreen === 1 && seen.screenLineHolds,
      JSON.stringify(seen),
    );
    ok(
      "a conversation standing by default reads nothing while its agent waits, follows it while it works, and stops again when the turn ends",
      seen.quietChat && seen.quietReads === 0 && seen.workingReads >= 2 && seen.endedReads === 0,
      JSON.stringify({ quietChat: seen.quietChat, quietReads: seen.quietReads, workingReads: seen.workingReads, endedReads: seen.endedReads }),
    );
    ok(
      "the answer reads at Geist 16 on 24 in one column of at most 760 px, code in a fixed-width chip, and the composer floats under that column",
      seen.proseSize === "16px" && seen.proseLine === "24px" && seen.turnWidth > 0 && seen.turnWidth <= 760 &&
        seen.columnWidth > 760 && /mono|Menlo|SF Mono/i.test(seen.codeFont) && seen.codeGround !== "rgba(0, 0, 0, 0)" &&
        seen.dockRadius === "12px" && seen.dockAligned,
      JSON.stringify(seen),
    );
    ok(
      "a session that asks no permission says so on the composer by a triangle and the CLI's own word, in no colour",
      seen.modeShown && seen.modeMark === "#i-alert" && seen.modeWords.includes("Bypass permissions") &&
        seen.sendNotHalt && seen.modeNotHalt,
      JSON.stringify({ modeShown: seen.modeShown, modeMark: seen.modeMark, modeWords: seen.modeWords, sendGround: seen.sendGround, modeInk: seen.modeInk }),
    );
  } finally { await page.close(); }
}
