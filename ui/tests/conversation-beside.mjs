/* An answer's changes beside the conversation — what the person passed on
 * 2026-11-12 (window artifact p-5e000c2b7e067271, decision 1): on a page with
 * the room for it, 「검토」 stands the turn's changes in a panel at the page's
 * right, so the answer is read where it stood and the rows that changed are
 * read next to it; a page without the room opens them in the card, as before.
 * A file's own line still opens in the card. Each check reads the page a
 * browser laid out. The conversations are synthetic.
 *
 *   WINDOW_SUITES='^conversation-beside$' node ui/tests/window.mjs */
import { openAfterScene } from "./conversation-after.mjs";
import { foldConversations, openWindowTestPage } from "./window-boot.mjs";

/* A window whose pane has the room: the page keeps a column beside the panel. */
const WIDE = { context: { viewport: { width: 1920, height: 1080 } } };
/* The harness's own window, whose pane has not. */
const NARROW = { width: 1280, height: 860 };

/* Where the page, its panel, its card and its composer stand, and what they say. */
function readBeside() {
  const host = document.querySelector(".pane-chat");
  const list = host.querySelector(".helper-turns");
  const box = (node) => {
    if (!node) return null;
    const at = node.getBoundingClientRect();
    return { left: Math.round(at.left), right: Math.round(at.right), top: Math.round(at.top), bottom: Math.round(at.bottom), width: Math.round(at.width) };
  };
  const cardOf = (row) => {
    const card = row?.querySelector(":scope > .chat-result") ?? null;
    const all = card?.querySelector(".chat-result-all") ?? null;
    return card && {
      box: box(card),
      rows: card.querySelectorAll(".chat-result-diff .diff-line").length,
      all: all?.textContent ?? null,
      allOpen: all?.getAttribute("aria-expanded") ?? null,
      allControls: all?.getAttribute("aria-controls") ?? null,
      beside: card.dataset.beside ?? null,
      waits: card.dataset.review ?? null,
      paths: [...card.querySelectorAll(".chat-result-path")].map((path) => path.textContent),
    };
  };
  const panel = host.querySelector(":scope > .chat-beside");
  const answers = [...list.querySelectorAll(":scope > .helper-turn.is-assistant.is-turn-answer")];
  const latest = list.querySelector(":scope > .helper-turn.is-latest");
  const style = getComputedStyle(host);
  return {
    page: box(host),
    padded: parseFloat(style.paddingRight) || 0,
    width: parseFloat(style.getPropertyValue("--chat-beside-w")),
    column: parseFloat(style.getPropertyValue("--chat-beside-page-min")),
    marked: host.classList.contains("has-beside"),
    panels: host.querySelectorAll(".chat-beside").length,
    panel: box(panel),
    panelId: panel?.id ?? null,
    panelName: panel?.getAttribute("aria-label") ?? null,
    panelTitle: panel?.querySelector(".chat-beside-head .chat-result-title")?.textContent ?? null,
    panelAdded: panel?.querySelector(".chat-beside-head .chat-result-added")?.textContent ?? null,
    panelRemoved: panel?.querySelector(".chat-beside-head .chat-result-removed")?.textContent ?? null,
    panelFiles: [...(panel?.querySelectorAll(".chat-result-file") ?? [])].map((line) => ({
      path: line.querySelector(".chat-result-path").textContent,
      open: line.getAttribute("aria-expanded"),
    })),
    panelRows: panel?.querySelectorAll(".chat-result-diff .diff-line").length ?? 0,
    panelHeads: panel?.querySelectorAll(".chat-result-head").length ?? 0,
    panelTests: panel?.querySelector(".chat-result-tests")?.firstChild?.textContent ?? null,
    panelClose: panel?.querySelector(".chat-beside-close")?.textContent ?? null,
    head: box(host.querySelector(":scope > .worker-head")),
    panelHead: box(panel?.querySelector(".chat-beside-head")),
    list: box(list),
    dock: box(host.querySelector(".chat-dock")),
    prose: box(latest?.querySelector(":scope > .helper-said")),
    latest: cardOf(latest),
    older: cardOf(answers.find((row) => row !== latest)),
    focusOn: document.activeElement?.className ?? null,
    focusInPanel: Boolean(panel?.contains(document.activeElement)),
    sideways: document.documentElement.scrollWidth - document.documentElement.clientWidth,
    listSideways: list.scrollWidth - list.clientWidth,
  };
}

const settle = (page) => page.evaluate(async () => {
  await window.__PAINTED__();
  await window.__PAINTED__();
});

export async function testConversationBeside(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin, WIDE);
  try {
    await openAfterScene(page);
    await foldConversations(page);
    await settle(page);
    const term = await page.evaluate(() => activePaneOf(currentTab()));
    const start = await page.evaluate(readBeside);
    ok("beside: the scene's pane has the room — its page is as wide as the panel and the column it must keep, and no panel stands before a press",
      Number.isFinite(start.width) && Number.isFinite(start.column) && start.page.width - start.width >= start.column &&
        start.panels === 0 && start.padded === 0 && !start.marked && start.latest?.all === "검토",
      JSON.stringify(start));
    const waiting = await page.evaluate((term) => paneReviewWaiting(term), term);

    // By the keyboard: a press with the pointer leaves the keyboard where the engine puts it (WebKit: not on a button).
    await page.locator(".pane-chat .helper-turn.is-latest .chat-result-all").focus();
    await page.keyboard.press("Enter");
    await settle(page);
    const open = await page.evaluate(readBeside);
    ok("beside: 「검토」 stands the turn's changes in one panel — its file open with its rows, the turn's test line under them, no second head — and the card's own rows stay shut",
      open.panels === 1 && open.panelFiles.length === 1 && open.panelFiles[0].path === "src/session.rs" && open.panelFiles[0].open === "true" &&
        open.panelRows > 0 && open.panelTests === "✓ 테스트 3개 통과" && open.panelHeads === 0 && open.latest.rows === 0,
      JSON.stringify(open));
    ok("beside: the card says its changes stand beside it and names the panel, and the keyboard stays on the press",
      open.latest.all === "옆에서 보는 중" && open.latest.allOpen === "true" && open.latest.allControls === open.panelId &&
        open.latest.beside === "true" && String(open.focusOn).includes("chat-result-all"),
      JSON.stringify(open));
    const spent = await page.evaluate((term) => paneReviewWaiting(term), term);
    ok("beside: changes read beside the conversation are changes looked at — the card's blue and the sidebar's mark are spent",
      waiting === 1 && spent === 0 && open.latest.waits === null,
      JSON.stringify({ waiting, spent, waits: open.latest.waits }));
    ok("beside: the panel says what the card's head says, under a name of its own, with a close that says 「닫기」",
      open.panelTitle === "변경된 파일 1개" && open.panelAdded === "+2" && open.panelRemoved === "−1" &&
        open.panelName === "이 턴이 바꾼 것" && open.panelClose === "닫기",
      JSON.stringify(open));
    ok("beside: the panel is its token wide at the page's right edge, top to bottom, and its head ends on the conversation head's rule",
      open.panel.width === open.width && open.panel.right === open.page.right && open.panel.top === open.page.top &&
        open.panel.bottom === open.page.bottom && open.panelHead.bottom === open.head.bottom,
      JSON.stringify(open));
    ok("beside: the conversation keeps to its side — its head, list, prose and composer all end before the panel — and nothing scrolls sideways",
      open.marked && open.padded === open.width && open.head.right <= open.panel.left && open.list.right <= open.panel.left &&
        open.prose.right <= open.panel.left && open.dock.right <= open.panel.left && open.dock.left >= open.page.left &&
        open.sideways === 0 && open.listSideways === 0,
      JSON.stringify(open));
    ok("beside: the composer stands under the column it serves, as it did before the panel",
      Math.abs((open.dock.left + open.dock.right) / 2 - (open.prose.left + open.prose.right) / 2) <= Math.abs((start.dock.left + start.dock.right) / 2 - (start.prose.left + start.prose.right) / 2) + 1,
      JSON.stringify({ start: [start.dock, start.prose], open: [open.dock, open.prose] }));

    // A file's line in the panel folds and opens its own rows.
    await page.locator(".pane-chat .chat-beside .chat-result-file").click();
    await settle(page);
    const folded = await page.evaluate(readBeside);
    await page.locator(".pane-chat .chat-beside .chat-result-file").click();
    await settle(page);
    const unfolded = await page.evaluate(readBeside);
    ok("beside: a file's line in the panel folds its rows and opens them again",
      folded.panelFiles[0].open === "false" && folded.panelRows === 0 && unfolded.panelFiles[0].open === "true" && unfolded.panelRows === open.panelRows,
      JSON.stringify({ folded: folded.panelFiles, unfolded: unfolded.panelFiles }));

    // A press inside the panel puts the keyboard in the panel, whatever the engine does with a pressed button: Escape then closes it.
    ok("beside: after a press inside the panel the keyboard is the panel's", unfolded.focusInPanel, JSON.stringify(unfolded.focusOn));
    await page.keyboard.press("Escape");
    await settle(page);
    const keyed = await page.evaluate(readBeside);
    ok("beside: Escape after a press inside the panel closes it and gives the keyboard back to the card",
      keyed.panels === 0 && !keyed.marked && keyed.latest.all === "검토" && String(keyed.focusOn).includes("chat-result-all"),
      JSON.stringify({ panels: keyed.panels, latest: keyed.latest, focusOn: keyed.focusOn }));
    await page.locator(".pane-chat .helper-turn.is-latest .chat-result-all").click();
    await settle(page);

    // Escape in the panel is the panel's: it closes, and the key never reaches the page (where it stops a turn).
    const escaped = await page.evaluate(async () => {
      const host = document.querySelector(".pane-chat");
      let reached = 0;
      const count = (event) => { if (event.key === "Escape") reached += 1; };
      host.addEventListener("keydown", count);
      const close = host.querySelector(".chat-beside-close");
      close.focus();
      const event = new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true });
      close.dispatchEvent(event);
      host.removeEventListener("keydown", count);
      await window.__PAINTED__();
      return { reached, prevented: event.defaultPrevented };
    });
    const closed = await page.evaluate(readBeside);
    ok("beside: Escape in the panel closes the panel and is not the page's key — the card says 「검토」 again and takes the keyboard back",
      escaped.reached === 0 && escaped.prevented && closed.panels === 0 && !closed.marked && closed.padded === 0 &&
        closed.latest.all === "검토" && closed.latest.allOpen === "false" && closed.latest.allControls === null && closed.latest.beside === null &&
        String(closed.focusOn).includes("chat-result-all"),
      JSON.stringify({ escaped, closed }));
    ok("beside: the page closed is the page as it stood — the composer and the column where they were",
      closed.dock.left === start.dock.left && closed.dock.right === start.dock.right && closed.prose.left === start.prose.left && closed.prose.right === start.prose.right,
      JSON.stringify({ start: [start.dock, start.prose], closed: [closed.dock, closed.prose] }));

    // The press on the card whose changes stand beside it takes the panel down; 「닫기」 does too.
    const all = page.locator(".pane-chat .helper-turn.is-latest .chat-result-all");
    await all.click();
    await settle(page);
    await all.click();
    await settle(page);
    const toggled = await page.evaluate(readBeside);
    await all.click();
    await settle(page);
    await page.locator(".pane-chat .chat-beside-close").click();
    await settle(page);
    const shut = await page.evaluate(readBeside);
    ok("beside: a second press on the card takes the panel down, and so does 「닫기」",
      toggled.panels === 0 && toggled.latest.all === "검토" && shut.panels === 0 && shut.latest.all === "검토" && String(shut.focusOn).includes("chat-result-all"),
      JSON.stringify({ toggled: toggled.latest, shut: shut.latest }));

    // A file's own line is still the quick look: its rows open in the card and no panel stands.
    await page.locator(".pane-chat .helper-turn.is-latest .chat-result-file").click();
    await settle(page);
    const quick = await page.evaluate(readBeside);
    ok("beside: a file's own line opens its rows in the card, with no panel",
      quick.panels === 0 && quick.latest.rows > 0 && quick.latest.all === "접기",
      JSON.stringify(quick.latest));
    await page.locator(".pane-chat .helper-turn.is-latest .chat-result-file").click();
    await settle(page);

    // The head's 「변경 N」 brings the newest answer's changes: beside, and the keyboard goes on in the panel.
    await page.locator(".pane-chat .chat-head-review").click();
    await settle(page);
    const fromHead = await page.evaluate(readBeside);
    ok("beside: the head's 「변경 N」 stands the newest answer's changes beside the conversation and gives the keyboard to the panel's first file",
      fromHead.panels === 1 && fromHead.panelFiles[0]?.path === "src/session.rs" && fromHead.latest.beside === "true" && fromHead.latest.rows === 0 &&
        fromHead.focusInPanel && String(fromHead.focusOn).includes("chat-result-file"),
      JSON.stringify(fromHead));

    // An older answer's own 「변경 N」 turns the one panel to that answer.
    // Its action row stands under the pointer, as conversation-result reaches it.
    await page.evaluate(async () => {
      const list = document.querySelector(".pane-chat .helper-turns");
      const first = [...list.querySelectorAll(":scope > .helper-turn.is-assistant.is-turn-answer")][0];
      first.dispatchEvent(new PointerEvent("pointerover", { bubbles: true }));
      await window.__PAINTED__();
      first.querySelector(".chat-answer-changes").click();
    });
    await settle(page);
    const turned = await page.evaluate(readBeside);
    ok("beside: an older answer's 「변경 N」 turns the one panel to that answer — its card says so, and the newest card says 「검토」 again",
      turned.panels === 1 && turned.panelFiles.length === 1 && turned.panelFiles[0].path === "docs.md" && turned.panelTests === null &&
        turned.older?.beside === "true" && turned.older.all === "옆에서 보는 중" && turned.latest.beside === null && turned.latest.all === "검토",
      JSON.stringify(turned));

    // The pane turned to its terminal and back keeps what stood.
    const back = await page.evaluate(async (read) => {
      const term = activePaneOf(currentTab());
      await setPaneChat(term, false);
      await window.__PAINTED__();
      await setPaneChat(term, true);
      await window.__PAINTED__();
      await window.__PAINTED__();
      return new Function(`return (${read})()`)();
    }, readBeside.toString());
    ok("beside: a pane turned to its terminal and back keeps its panel — the page still gives it its room, and the same answer's card still says its changes stand beside it",
      back.panels === 1 && back.marked && back.padded === back.width && back.panelFiles[0]?.path === "docs.md" &&
        back.older?.beside === "true" && back.older.all === "옆에서 보는 중" && back.older.rows === 0,
      JSON.stringify(back));

    // A page that loses the room reads the same rows in the card.
    const before = await page.evaluate(readBeside);
    await page.setViewportSize(NARROW);
    await settle(page);
    await settle(page);
    const narrow = await page.evaluate(readBeside);
    ok("beside: a page that loses the room takes the panel down and opens the same rows in the card — nothing is padded for a panel that is gone",
      before.panels === 1 && before.older?.beside === "true" && narrow.panels === 0 && !narrow.marked && narrow.padded === 0 &&
        narrow.older?.rows > 0 && narrow.older.beside === null && narrow.page.width - narrow.width < narrow.column && narrow.sideways === 0,
      JSON.stringify({ before: [before.latest, before.older], narrow: [narrow.page, narrow.latest, narrow.older] }));

    // And on that page 「검토」 is the card's, as before.
    await page.locator(".pane-chat .helper-turn.is-latest .chat-result-all").click();
    await settle(page);
    const inline = await page.evaluate(readBeside);
    await page.setViewportSize(WIDE.context.viewport);
    await settle(page);
    await settle(page);
    const wide = await page.evaluate(readBeside);
    ok("beside: on a page without the room 「검토」 opens every file in the card, and a page that gets the room back opens no panel by itself",
      inline.panels === 0 && inline.latest.rows > 0 && inline.latest.allOpen === "true" && wide.panels === 0 && wide.padded === 0,
      JSON.stringify({ inline: inline.latest, wide: wide.latest }));
    ok("beside: the page raised no errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}
